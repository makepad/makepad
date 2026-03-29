use crate::tsdf_query::{depth_query_plane_quad, DepthQuerySupportPlane};
use crate::*;
use std::{
    collections::{hash_map::DefaultHasher, HashSet},
    hash::{Hash, Hasher},
};

const XR_DEBUG_DEPTH_FLOATS_PER_VERTEX: usize = 8;
const XR_DEBUG_TSDF_MESH_CHUNK_EDGE_VOXELS: i32 = 24;
const XR_DEBUG_TSDF_MESH_CHUNK_OVERLAP_VOXELS: i32 = 4;
const XR_DEBUG_TSDF_MESH_CELL_STRIDE_VOXELS: i32 = 1;
const XR_DEBUG_TSDF_MIN_MESH_CONFIDENCE: u8 = 3;
const XR_DEBUG_TSDF_RECENT_MESH_CONFIDENCE: u8 = 1;
const XR_DEBUG_TSDF_RECENT_MESH_GENERATIONS: u64 = 6;
const XR_DEBUG_TSDF_RECENT_MESH_MAX_ABS_DISTANCE: f32 = 0.6;
const XR_DEBUG_TSDF_DENSE_HOLE_FILL_MAX_PASSES: usize = 2;

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
struct VoxelCoord {
    x: i32,
    y: i32,
    z: i32,
}

impl VoxelCoord {
    const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }
}

impl core::ops::Add for VoxelCoord {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self::new(self.x + rhs.x, self.y + rhs.y, self.z + rhs.z)
    }
}

impl core::ops::Sub for VoxelCoord {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self::Output {
        Self::new(self.x - rhs.x, self.y - rhs.y, self.z - rhs.z)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
struct SurfaceMesh32 {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    indices: Vec<u32>,
}

#[derive(Clone, Debug)]
pub(crate) struct DebugDepthMeshChunk {
    pub(crate) chunk_key: ChunkKey,
    pub(crate) fingerprint: u64,
    pub(crate) indices: Vec<u32>,
    pub(crate) vertices: Vec<f32>,
}

const SURFACE_NET_CORNERS: [VoxelCoord; 8] = [
    VoxelCoord::new(0, 0, 0),
    VoxelCoord::new(1, 0, 0),
    VoxelCoord::new(1, 0, 1),
    VoxelCoord::new(0, 0, 1),
    VoxelCoord::new(0, 1, 0),
    VoxelCoord::new(1, 1, 0),
    VoxelCoord::new(1, 1, 1),
    VoxelCoord::new(0, 1, 1),
];

const SURFACE_NET_EDGES: [(usize, usize); 12] = [
    (0, 1),
    (1, 2),
    (2, 3),
    (3, 0),
    (4, 5),
    (5, 6),
    (6, 7),
    (7, 4),
    (0, 4),
    (1, 5),
    (2, 6),
    (3, 7),
];

fn quantize_f32(value: f32, quantum: f32) -> i32 {
    (value / quantum.max(f32::EPSILON)).round() as i32
}

#[cfg(test)]
fn depth_tsd_distance_meters(voxel_size_meters: f32) -> f32 {
    voxel_size_meters * 2.0
}

fn chunk_key_and_id(grid: &SparseTsdGridReadSnapshot, coord: VoxelCoord) -> (ChunkKey, usize) {
    let cx = coord.x.div_euclid(grid.chunk_edge);
    let cy = coord.y.div_euclid(grid.chunk_edge);
    let cz = coord.z.div_euclid(grid.chunk_edge);
    let lx = coord.x.rem_euclid(grid.chunk_edge) as usize;
    let ly = coord.y.rem_euclid(grid.chunk_edge) as usize;
    let lz = coord.z.rem_euclid(grid.chunk_edge) as usize;
    let edge = grid.chunk_edge as usize;
    let id = lx + ly * edge + lz * edge * edge;
    (ChunkKey::new(cx, cy, cz), id)
}

fn snapshot_meshing_distance(
    grid: &SparseTsdGridReadSnapshot,
    coord: VoxelCoord,
    current_generation: u64,
) -> Option<f32> {
    let (chunk_key, id) = chunk_key_and_id(grid, coord);
    let chunk = grid.chunks.get(&chunk_key)?;
    let confidence = chunk.confidence(id);
    let value = chunk.value(id)?;
    if confidence >= XR_DEBUG_TSDF_MIN_MESH_CONFIDENCE {
        return Some(value);
    }
    let observed_generation = chunk.observed_generation(id, current_generation);
    (confidence >= XR_DEBUG_TSDF_RECENT_MESH_CONFIDENCE
        && current_generation.saturating_sub(observed_generation)
            <= XR_DEBUG_TSDF_RECENT_MESH_GENERATIONS
        && value.abs() <= XR_DEBUG_TSDF_RECENT_MESH_MAX_ABS_DISTANCE)
        .then_some(value)
}

fn push_debug_depth_vertex(vertices: &mut Vec<f32>, position: Vec3f, normal: Vec3f) {
    vertices.extend_from_slice(&[
        position.x, position.y, position.z, 1.0, normal.x, normal.y, normal.z, 0.0,
    ]);
}

fn pack_surface_mesh_debug_vertices(mesh: &SurfaceMesh32) -> (Vec<u32>, Vec<f32>) {
    let mut vertices = Vec::with_capacity(mesh.positions.len() * XR_DEBUG_DEPTH_FLOATS_PER_VERTEX);
    for (position, normal) in mesh.positions.iter().zip(mesh.normals.iter()) {
        push_debug_depth_vertex(
            &mut vertices,
            vec3f(position[0], position[1], position[2]),
            vec3f(normal[0], normal[1], normal[2]),
        );
    }
    (mesh.indices.clone(), vertices)
}

fn push_debug_depth_quad(indices: &mut Vec<u32>, vertices: &mut Vec<f32>, quad: [Vec3f; 4]) {
    let base = (vertices.len() / XR_DEBUG_DEPTH_FLOATS_PER_VERTEX) as u32;
    let raw_normal = Vec3f::cross(quad[1] - quad[0], quad[2] - quad[0]);
    let normal = if raw_normal.length() > 1.0e-6 {
        raw_normal.normalize()
    } else {
        vec3f(0.0, 1.0, 0.0)
    };
    for position in quad {
        push_debug_depth_vertex(vertices, position, normal);
    }
    indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
}

pub(crate) fn push_debug_depth_plane(
    indices: &mut Vec<u32>,
    vertices: &mut Vec<f32>,
    plane: DepthQuerySupportPlane,
) {
    push_debug_depth_quad(indices, vertices, depth_query_plane_quad(plane));
}

fn align_extent(extent: i32, stride: i32) -> i32 {
    let stride = stride.max(1);
    if extent <= 0 {
        0
    } else {
        ((extent + stride - 1) / stride) * stride
    }
}

fn flatten_coord(coord: VoxelCoord, size: VoxelCoord) -> usize {
    coord.x as usize
        + coord.y as usize * size.x as usize
        + coord.z as usize * size.x as usize * size.y as usize
}

fn dense_corner_coord(base: VoxelCoord, corner: VoxelCoord, stride: i32) -> VoxelCoord {
    VoxelCoord::new(
        base.x * stride + corner.x * stride,
        base.y * stride + corner.y * stride,
        base.z * stride + corner.z * stride,
    )
}

fn snapshot_region_has_surface(
    grid: &SparseTsdGridReadSnapshot,
    start: VoxelCoord,
    extent: VoxelCoord,
) -> bool {
    if grid.chunks.is_empty() {
        return false;
    }
    let max = VoxelCoord::new(
        start.x + extent.x.saturating_sub(1),
        start.y + extent.y.saturating_sub(1),
        start.z + extent.z.saturating_sub(1),
    );
    let min_chunk = VoxelCoord::new(
        start.x.div_euclid(grid.chunk_edge),
        start.y.div_euclid(grid.chunk_edge),
        start.z.div_euclid(grid.chunk_edge),
    );
    let max_chunk = VoxelCoord::new(
        max.x.div_euclid(grid.chunk_edge),
        max.y.div_euclid(grid.chunk_edge),
        max.z.div_euclid(grid.chunk_edge),
    );
    for z in min_chunk.z..=max_chunk.z {
        for y in min_chunk.y..=max_chunk.y {
            for x in min_chunk.x..=max_chunk.x {
                if grid.chunks.contains_key(&ChunkKey::new(x, y, z)) {
                    return true;
                }
            }
        }
    }
    false
}

fn extract_dense_region_into(
    grid: &SparseTsdGridReadSnapshot,
    start: VoxelCoord,
    extent: VoxelCoord,
    current_generation: u64,
    dense: &mut Vec<f32>,
) {
    let sx = extent.x.max(0) as usize;
    let sy = extent.y.max(0) as usize;
    let sz = extent.z.max(0) as usize;
    dense.clear();
    dense.resize(sx * sy * sz, f32::NEG_INFINITY);
    for z in 0..extent.z.max(0) {
        for y in 0..extent.y.max(0) {
            for x in 0..extent.x.max(0) {
                let coord = VoxelCoord::new(start.x + x, start.y + y, start.z + z);
                let value = snapshot_meshing_distance(grid, coord, current_generation)
                    .unwrap_or(f32::NEG_INFINITY);
                dense[(x as usize) + (y as usize) * sx + (z as usize) * sx * sy] = value;
            }
        }
    }
}

fn repair_dense_meshing_holes(dense: &mut Vec<f32>, scratch: &mut Vec<f32>, extent: VoxelCoord) {
    let sx = extent.x.max(0) as usize;
    let sy = extent.y.max(0) as usize;
    let sz = extent.z.max(0) as usize;
    if sx < 3 || sy < 3 || sz < 3 || dense.len() != sx * sy * sz {
        return;
    }

    scratch.clear();
    scratch.resize(dense.len(), f32::NEG_INFINITY);

    for _ in 0..XR_DEBUG_TSDF_DENSE_HOLE_FILL_MAX_PASSES {
        scratch.as_mut_slice().copy_from_slice(dense.as_slice());
        let mut changed = false;

        for z in 1..(sz - 1) {
            for y in 1..(sy - 1) {
                for x in 1..(sx - 1) {
                    let coord = VoxelCoord::new(x as i32, y as i32, z as i32);
                    let index = flatten_coord(coord, extent);
                    if dense[index].is_finite() {
                        continue;
                    }

                    let mut pair_sum = 0.0f32;
                    let mut pair_count = 0usize;
                    let mut sign_vote = 0i32;

                    for (a, b) in [
                        (VoxelCoord::new(-1, 0, 0), VoxelCoord::new(1, 0, 0)),
                        (VoxelCoord::new(0, -1, 0), VoxelCoord::new(0, 1, 0)),
                        (VoxelCoord::new(0, 0, -1), VoxelCoord::new(0, 0, 1)),
                    ] {
                        let value_a = dense[flatten_coord(coord + a, extent)];
                        let value_b = dense[flatten_coord(coord + b, extent)];
                        if !value_a.is_finite() || !value_b.is_finite() {
                            continue;
                        }
                        pair_sum += value_a + value_b;
                        pair_count += 1;
                        sign_vote += if value_a + value_b >= 0.0 { 1 } else { -1 };
                    }

                    if pair_count < 2 {
                        continue;
                    }

                    let filled = pair_sum / (pair_count as f32 * 2.0);
                    if !filled.is_finite() {
                        continue;
                    }
                    scratch[index] = if sign_vote < 0 {
                        filled.min(-1.0e-4)
                    } else if sign_vote > 0 {
                        filled.max(1.0e-4)
                    } else {
                        filled
                    };
                    changed = true;
                }
            }
        }

        if !changed {
            break;
        }
        dense.as_mut_slice().copy_from_slice(scratch.as_slice());
    }
}

fn surface_net_mesh_from_dense(
    volume: &[f32],
    voxel_count: VoxelCoord,
    voxel_size: f32,
    start_coord: VoxelCoord,
    stride: i32,
) -> Option<SurfaceMesh32> {
    if voxel_count.x <= 1 || voxel_count.y <= 1 || voxel_count.z <= 1 {
        return None;
    }
    let stride = stride.max(1);
    let scaled_count = VoxelCoord::new(
        voxel_count.x.div_euclid(stride),
        voxel_count.y.div_euclid(stride),
        voxel_count.z.div_euclid(stride),
    );
    if scaled_count.x <= 1 || scaled_count.y <= 1 || scaled_count.z <= 1 {
        return None;
    }

    let sample_value = |coord: VoxelCoord| -> f32 {
        let raw = volume[flatten_coord(coord, voxel_count)];
        if raw.is_finite() {
            raw
        } else {
            0.0
        }
    };
    let raw_value = |coord: VoxelCoord| -> f32 { volume[flatten_coord(coord, voxel_count)] };

    let mut positions = Vec::<[f32; 3]>::new();
    let mut normals = Vec::<[f32; 3]>::new();
    let mut indices = Vec::<u32>::new();
    let mut coord_vert_map =
        vec![i32::MIN; (scaled_count.x * scaled_count.y * scaled_count.z) as usize];
    let mut vert_coords = Vec::<VoxelCoord>::new();

    for z in 0..scaled_count.z {
        for y in 0..scaled_count.y {
            for x in 0..scaled_count.x {
                if x == scaled_count.x - 1 || y == scaled_count.y - 1 || z == scaled_count.z - 1 {
                    continue;
                }
                let coord = VoxelCoord::new(x, y, z);
                let mut pos_coord = vec3f(0.0, 0.0, 0.0);
                let mut direction = vec3f(0.0, 0.0, 0.0);
                let mut crossings = 0u8;
                let mut bad_crossings = 0u8;

                for (a_idx, b_idx) in SURFACE_NET_EDGES {
                    let coord_a = dense_corner_coord(coord, SURFACE_NET_CORNERS[a_idx], stride);
                    let coord_b = dense_corner_coord(coord, SURFACE_NET_CORNERS[b_idx], stride);
                    let value_a = sample_value(coord_a);
                    let value_b = sample_value(coord_b);
                    let change = value_a - value_b;
                    direction += vec3f(
                        (coord_a.x - coord_b.x) as f32,
                        (coord_a.y - coord_b.y) as f32,
                        (coord_a.z - coord_b.z) as f32,
                    )
                    .scale(change);
                    if (value_a < 0.0) == (value_b < 0.0) || change.abs() <= f32::EPSILON {
                        continue;
                    }
                    if !raw_value(coord_a).is_finite() || !raw_value(coord_b).is_finite() {
                        bad_crossings = bad_crossings.saturating_add(1);
                    }
                    let t = value_a / change;
                    pos_coord += vec3f(
                        coord_a.x as f32 + (coord_b.x - coord_a.x) as f32 * t,
                        coord_a.y as f32 + (coord_b.y - coord_a.y) as f32 * t,
                        coord_a.z as f32 + (coord_b.z - coord_a.z) as f32 * t,
                    );
                    crossings = crossings.saturating_add(1);
                }

                if crossings < 3 || crossings == bad_crossings {
                    continue;
                }

                pos_coord = pos_coord.scale(1.0 / crossings as f32);
                let world = vec3f(
                    (start_coord.x as f32 + pos_coord.x + 0.5) * voxel_size,
                    (start_coord.y as f32 + pos_coord.y + 0.5) * voxel_size,
                    (start_coord.z as f32 + pos_coord.z + 0.5) * voxel_size,
                );
                let normal = if direction.length() > 1.0e-6 {
                    direction.normalize()
                } else {
                    vec3f(0.0, 1.0, 0.0)
                };
                let vertex_index = positions.len() as u32;
                positions.push([world.x, world.y, world.z]);
                normals.push([normal.x, normal.y, normal.z]);
                coord_vert_map[flatten_coord(coord, scaled_count)] = vertex_index as i32;
                vert_coords.push(coord);
            }
        }
    }

    for coord in vert_coords {
        surface_net_tris_for_axis(
            &mut indices,
            &coord_vert_map,
            scaled_count,
            &sample_value,
            coord,
            VoxelCoord::new(1, 0, 0),
            VoxelCoord::new(0, 0, 1),
            VoxelCoord::new(0, 1, 0),
            stride,
        );
        surface_net_tris_for_axis(
            &mut indices,
            &coord_vert_map,
            scaled_count,
            &sample_value,
            coord,
            VoxelCoord::new(0, 1, 0),
            VoxelCoord::new(1, 0, 0),
            VoxelCoord::new(0, 0, 1),
            stride,
        );
        surface_net_tris_for_axis(
            &mut indices,
            &coord_vert_map,
            scaled_count,
            &sample_value,
            coord,
            VoxelCoord::new(0, 0, 1),
            VoxelCoord::new(0, 1, 0),
            VoxelCoord::new(1, 0, 0),
            stride,
        );
    }

    if indices.is_empty() {
        None
    } else {
        Some(SurfaceMesh32 {
            positions,
            normals,
            indices,
        })
    }
}

fn surface_net_tris_for_axis(
    indices: &mut Vec<u32>,
    coord_vert_map: &[i32],
    size: VoxelCoord,
    sample_value: &impl Fn(VoxelCoord) -> f32,
    coord: VoxelCoord,
    axis: VoxelCoord,
    d1: VoxelCoord,
    d2: VoxelCoord,
    stride: i32,
) {
    if coord.x - d1.x < 0
        || coord.y - d1.y < 0
        || coord.z - d1.z < 0
        || coord.x - d2.x < 0
        || coord.y - d2.y < 0
        || coord.z - d2.z < 0
    {
        return;
    }
    let scaled = VoxelCoord::new(coord.x * stride, coord.y * stride, coord.z * stride);
    let value_a = sample_value(scaled);
    let value_b =
        sample_value(scaled + VoxelCoord::new(axis.x * stride, axis.y * stride, axis.z * stride));
    if (value_a < 0.0) == (value_b < 0.0) {
        return;
    }
    let a = coord_vert_map[flatten_coord(coord, size)];
    let b = coord_vert_map[flatten_coord(coord - d1, size)];
    let c = coord_vert_map[flatten_coord(coord - d1 - d2, size)];
    let d = coord_vert_map[flatten_coord(coord - d2, size)];
    if a < 0 || b < 0 || c < 0 || d < 0 {
        return;
    }
    let (a, b, c, d) = (a as u32, b as u32, c as u32, d as u32);
    if value_a < 0.0 {
        indices.extend_from_slice(&[c, b, a, d, c, a]);
    } else {
        indices.extend_from_slice(&[a, c, d, a, b, c]);
    }
}

fn snapshot_surface_net_chunk_mesh(
    snapshot: &TsdfPublishedSnapshot,
    chunk_key: ChunkKey,
    dense: &mut Vec<f32>,
    fill_scratch: &mut Vec<f32>,
) -> Option<SurfaceMesh32> {
    let edge = XR_DEBUG_TSDF_MESH_CHUNK_EDGE_VOXELS.max(1);
    let overlap = XR_DEBUG_TSDF_MESH_CHUNK_OVERLAP_VOXELS.max(0);
    let stride = XR_DEBUG_TSDF_MESH_CELL_STRIDE_VOXELS.max(1);
    let start = VoxelCoord::new(chunk_key.x * edge, chunk_key.y * edge, chunk_key.z * edge);
    let extent = VoxelCoord::new(edge + overlap, edge + overlap, edge + overlap);
    if !snapshot_region_has_surface(snapshot.grid.as_ref(), start, extent) {
        return None;
    }
    let dense_size = VoxelCoord::new(
        align_extent(extent.x, stride),
        align_extent(extent.y, stride),
        align_extent(extent.z, stride),
    );
    extract_dense_region_into(
        snapshot.grid.as_ref(),
        start,
        dense_size,
        snapshot.generation,
        dense,
    );
    repair_dense_meshing_holes(dense, fill_scratch, dense_size);
    surface_net_mesh_from_dense(dense, dense_size, snapshot.grid.voxel_size, start, stride)
}

fn mesh_chunk_fingerprint(chunk_key: ChunkKey, mesh: &SurfaceMesh32) -> u64 {
    let mut hasher = DefaultHasher::new();
    chunk_key.hash(&mut hasher);
    for position in &mesh.positions {
        quantize_f32(position[0], 0.01).hash(&mut hasher);
        quantize_f32(position[1], 0.01).hash(&mut hasher);
        quantize_f32(position[2], 0.01).hash(&mut hasher);
    }
    mesh.indices.hash(&mut hasher);
    hasher.finish()
}

fn snapshot_debug_mesh_chunk_keys(snapshot: &TsdfPublishedSnapshot) -> Vec<ChunkKey> {
    let tsdf_edge = snapshot.grid.chunk_edge.max(1);
    let mesh_edge = XR_DEBUG_TSDF_MESH_CHUNK_EDGE_VOXELS.max(1);
    let overlap = XR_DEBUG_TSDF_MESH_CHUNK_OVERLAP_VOXELS.max(0);
    let mut keys = HashSet::new();

    for chunk_key in snapshot.grid.chunks.keys() {
        let min_voxel = VoxelCoord::new(
            chunk_key.x * tsdf_edge - overlap,
            chunk_key.y * tsdf_edge - overlap,
            chunk_key.z * tsdf_edge - overlap,
        );
        let max_voxel = VoxelCoord::new(
            (chunk_key.x + 1) * tsdf_edge - 1 + overlap,
            (chunk_key.y + 1) * tsdf_edge - 1 + overlap,
            (chunk_key.z + 1) * tsdf_edge - 1 + overlap,
        );
        let min_mesh = VoxelCoord::new(
            min_voxel.x.div_euclid(mesh_edge),
            min_voxel.y.div_euclid(mesh_edge),
            min_voxel.z.div_euclid(mesh_edge),
        );
        let max_mesh = VoxelCoord::new(
            max_voxel.x.div_euclid(mesh_edge),
            max_voxel.y.div_euclid(mesh_edge),
            max_voxel.z.div_euclid(mesh_edge),
        );
        for z in min_mesh.z..=max_mesh.z {
            for y in min_mesh.y..=max_mesh.y {
                for x in min_mesh.x..=max_mesh.x {
                    keys.insert(ChunkKey::new(x, y, z));
                }
            }
        }
    }

    let mut keys: Vec<_> = keys.into_iter().collect();
    keys.sort_by_key(|key| (key.x, key.y, key.z));
    keys
}

pub(crate) fn build_tsdf_snapshot_debug_mesh_chunks(
    snapshot: &TsdfPublishedSnapshot,
) -> Vec<DebugDepthMeshChunk> {
    let mut dense = Vec::new();
    let mut fill_scratch = Vec::new();
    let mut chunks = Vec::new();

    for chunk_key in snapshot_debug_mesh_chunk_keys(snapshot) {
        let Some(mesh) =
            snapshot_surface_net_chunk_mesh(snapshot, chunk_key, &mut dense, &mut fill_scratch)
        else {
            continue;
        };
        let fingerprint = mesh_chunk_fingerprint(chunk_key, &mesh);
        let (indices, vertices) = pack_surface_mesh_debug_vertices(&mesh);
        if indices.is_empty() || vertices.is_empty() {
            continue;
        }
        chunks.push(DebugDepthMeshChunk {
            chunk_key,
            fingerprint,
            indices,
            vertices,
        });
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::HashMap, sync::Arc};

    fn packed_position(vertices: &[f32], vertex_index: usize) -> Vec3f {
        let base = vertex_index * XR_DEBUG_DEPTH_FLOATS_PER_VERTEX;
        vec3f(vertices[base], vertices[base + 1], vertices[base + 2])
    }

    fn set_normalized_distance(
        chunks: &mut HashMap<ChunkKey, Arc<SparseTsdReadChunk>>,
        chunk_edge: i32,
        coord: VoxelCoord,
        normalized_distance: f32,
    ) {
        let chunk_key = ChunkKey::new(
            coord.x.div_euclid(chunk_edge),
            coord.y.div_euclid(chunk_edge),
            coord.z.div_euclid(chunk_edge),
        );
        let lx = coord.x.rem_euclid(chunk_edge) as usize;
        let ly = coord.y.rem_euclid(chunk_edge) as usize;
        let lz = coord.z.rem_euclid(chunk_edge) as usize;
        let edge = chunk_edge as usize;
        let id = lx + ly * edge + lz * edge * edge;

        let chunk = Arc::make_mut(
            chunks
                .entry(chunk_key)
                .or_insert_with(|| Arc::new(SparseTsdReadChunk::new(edge * edge * edge))),
        );
        chunk.set_value(id, normalized_distance, 8, 1);
    }

    fn make_flat_floor_snapshot(voxel_size: f32) -> TsdfPublishedSnapshot {
        let chunk_edge = 8;
        let mut chunks = HashMap::new();
        let tsd_distance_meters = depth_tsd_distance_meters(voxel_size);
        let mut active_value_count = 0usize;
        for z in -6..=6 {
            for y in -6..=6 {
                for x in -6..=6 {
                    let world_y = (y as f32 + 0.5) * voxel_size;
                    let normalized = (world_y / tsd_distance_meters).clamp(-1.0, 1.0);
                    set_normalized_distance(
                        &mut chunks,
                        chunk_edge,
                        VoxelCoord::new(x, y, z),
                        normalized,
                    );
                    active_value_count += 1;
                }
            }
        }
        TsdfPublishedSnapshot {
            generation: 1,
            latest_topology_generation: 1,
            update_sequence: 1,
            grid: Arc::new(SparseTsdGridReadSnapshot {
                voxel_size,
                chunk_edge,
                chunk_edge_shift: Some(chunk_edge.trailing_zeros() as u8),
                chunk_edge_mask: chunk_edge - 1,
                chunk_volume: (chunk_edge as usize).pow(3),
                active_value_count,
                active_bounds: Some((
                    vec3f(-6.0 * voxel_size, -6.0 * voxel_size, -6.0 * voxel_size),
                    vec3f(7.0 * voxel_size, 7.0 * voxel_size, 7.0 * voxel_size),
                )),
                chunks,
            }),
            height_map: None,
        }
    }

    #[test]
    fn pack_surface_mesh_debug_vertices_keeps_shared_indexed_vertices() {
        let mesh = SurfaceMesh32 {
            positions: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [1.0, 1.0, 0.0],
                [0.0, 1.0, 0.0],
            ],
            normals: vec![[0.0, 0.0, 1.0]; 4],
            indices: vec![0, 1, 2, 0, 2, 3],
        };

        let (indices, vertices) = pack_surface_mesh_debug_vertices(&mesh);

        assert_eq!(indices, vec![0, 1, 2, 0, 2, 3]);
        assert_eq!(vertices.len(), 4 * XR_DEBUG_DEPTH_FLOATS_PER_VERTEX);
        assert_eq!(packed_position(&vertices, 0), vec3f(0.0, 0.0, 0.0));
        assert_eq!(packed_position(&vertices, 2), vec3f(1.0, 1.0, 0.0));
    }

    #[test]
    fn push_debug_depth_plane_emits_single_quad_mesh() {
        let plane = DepthQuerySupportPlane {
            point: vec3f(0.0, 0.5, 0.0),
            normal: vec3f(0.0, 1.0, 0.0),
            tangent: vec3f(1.0, 0.0, 0.0),
            bitangent: vec3f(0.0, 0.0, 1.0),
            half_extent_tangent: 0.25,
            half_extent_bitangent: 0.10,
        };
        let mut indices = Vec::new();
        let mut vertices = Vec::new();

        push_debug_depth_plane(&mut indices, &mut vertices, plane);

        assert_eq!(indices, vec![0, 1, 2, 0, 2, 3]);
        assert_eq!(vertices.len(), 4 * XR_DEBUG_DEPTH_FLOATS_PER_VERTEX);
        let first = packed_position(&vertices, 0);
        let third = packed_position(&vertices, 2);
        assert_eq!(first, vec3f(-0.25, 0.5, -0.10));
        assert_eq!(third, vec3f(0.25, 0.5, 0.10));
    }

    #[test]
    fn build_tsdf_snapshot_debug_mesh_chunks_extracts_surface_from_snapshot() {
        let snapshot = make_flat_floor_snapshot(0.05);
        let chunks = build_tsdf_snapshot_debug_mesh_chunks(&snapshot);

        assert!(
            !chunks.is_empty(),
            "expected debug mesh chunks for a flat floor snapshot"
        );
        assert!(chunks
            .iter()
            .all(|chunk| !chunk.indices.is_empty() && !chunk.vertices.is_empty()));
    }
}
