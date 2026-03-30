use crate::algorithms::tsdf_query::{depth_query_plane_quad, DepthQuerySupportPlane};
use crate::prelude::*;
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    sync::Arc,
};

const XR_DEBUG_DEPTH_FLOATS_PER_VERTEX: usize = 8;
const XR_DEBUG_TSDF_MESH_CHUNK_WORLD_SIZE_METERS: f32 = 1.0;
const XR_DEBUG_TSDF_MESH_VIEW_DISTANCE_METERS: f32 = 5.5;
const XR_DEBUG_TSDF_MESH_NEAR_VISIBILITY_METERS: f32 = 1.35;
const XR_DEBUG_TSDF_MESH_VIEW_CONE_DOT: f32 = 0.309_016_88;
const XR_DEBUG_TSDF_MESH_CELL_STRIDE_VOXELS: i32 = 1;
const XR_DEBUG_TSDF_MESH_CHUNK_OVERLAP_CELLS: i32 = 2;
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

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct DebugDepthMeshLayout {
    pub(crate) chunk_edge_voxels: i32,
    pub(crate) overlap_voxels: i32,
    pub(crate) stride_voxels: i32,
    pub(crate) chunk_world_size_meters: f32,
    pub(crate) view_distance_meters: f32,
    pub(crate) near_distance_meters: f32,
    pub(crate) view_cone_dot: f32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct DebugDepthMeshChunkSignature {
    pub(crate) sources: Vec<(ChunkKey, usize)>,
}

#[derive(Clone, Debug)]
pub(crate) struct DebugDepthMeshChunkPlan {
    pub(crate) chunk_key: ChunkKey,
    pub(crate) signature: DebugDepthMeshChunkSignature,
}

#[derive(Clone, Debug)]
pub(crate) struct DebugDepthMeshViewPlan {
    pub(crate) layout: DebugDepthMeshLayout,
    pub(crate) visible_chunks: Vec<DebugDepthMeshChunkPlan>,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct SurfaceMesh32 {
    positions: Vec<[f32; 3]>,
    indices: Vec<u32>,
}

#[derive(Clone, Debug)]
pub(crate) struct DebugDepthMeshChunk {
    pub(crate) chunk_key: ChunkKey,
    pub(crate) fingerprint: u64,
    pub(crate) indices: Vec<u32>,
    pub(crate) vertices: Vec<f32>,
}

#[derive(Default)]
pub(crate) struct DebugDepthMeshTriangulator {
    dense: Vec<f32>,
    fill_scratch: Vec<f32>,
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

const DEBUG_DEPTH_TRIANGLE_BARYCENTRICS: [[f32; 3]; 3] =
    [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

fn quantize_f32(value: f32, quantum: f32) -> i32 {
    (value / quantum.max(f32::EPSILON)).round() as i32
}

#[cfg(test)]
fn depth_tsd_distance_meters(voxel_size_meters: f32) -> f32 {
    voxel_size_meters * 2.0
}

fn push_debug_depth_vertex(vertices: &mut Vec<f32>, position: Vec3f, barycentric: [f32; 3]) {
    vertices.extend_from_slice(&[
        position.x,
        position.y,
        position.z,
        1.0,
        barycentric[0],
        barycentric[1],
        barycentric[2],
        0.0,
    ]);
}

fn pack_surface_mesh_debug_vertices(mesh: &SurfaceMesh32) -> (Vec<u32>, Vec<f32>) {
    let mut indices = Vec::with_capacity(mesh.indices.len());
    let mut vertices = Vec::with_capacity(mesh.indices.len() * XR_DEBUG_DEPTH_FLOATS_PER_VERTEX);
    for triangle in mesh.indices.chunks_exact(3) {
        let base = (vertices.len() / XR_DEBUG_DEPTH_FLOATS_PER_VERTEX) as u32;
        for (corner, vertex_index) in triangle.iter().enumerate() {
            let position = mesh.positions[*vertex_index as usize];
            push_debug_depth_vertex(
                &mut vertices,
                vec3f(position[0], position[1], position[2]),
                DEBUG_DEPTH_TRIANGLE_BARYCENTRICS[corner],
            );
        }
        indices.extend_from_slice(&[base, base + 1, base + 2]);
    }
    (indices, vertices)
}

fn push_debug_depth_triangle(indices: &mut Vec<u32>, vertices: &mut Vec<f32>, tri: [Vec3f; 3]) {
    let base = (vertices.len() / XR_DEBUG_DEPTH_FLOATS_PER_VERTEX) as u32;
    for (corner, position) in tri.into_iter().enumerate() {
        push_debug_depth_vertex(
            vertices,
            position,
            DEBUG_DEPTH_TRIANGLE_BARYCENTRICS[corner],
        );
    }
    indices.extend_from_slice(&[base, base + 1, base + 2]);
}

fn push_debug_depth_quad(indices: &mut Vec<u32>, vertices: &mut Vec<f32>, quad: [Vec3f; 4]) {
    push_debug_depth_triangle(indices, vertices, [quad[0], quad[1], quad[2]]);
    push_debug_depth_triangle(indices, vertices, [quad[0], quad[2], quad[3]]);
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

fn snapshot_meshing_distance(grid: &SparseTsdGridReadSnapshot, coord: VoxelCoord) -> Option<f32> {
    let (chunk_key, id) = grid.chunk_key_and_local_index_xyz(coord.x, coord.y, coord.z);
    let chunk = grid.chunks.get(&chunk_key)?;
    chunk.value(id)
}

fn extract_dense_region_into(
    grid: &SparseTsdGridReadSnapshot,
    start: VoxelCoord,
    extent: VoxelCoord,
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
                let value = snapshot_meshing_distance(grid, coord).unwrap_or(f32::NEG_INFINITY);
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

fn scaled_voxel_count(voxel_count: VoxelCoord, stride: i32) -> Option<VoxelCoord> {
    if voxel_count.x <= 1 || voxel_count.y <= 1 || voxel_count.z <= 1 {
        return None;
    }
    let stride = stride.max(1);
    let scaled_count = VoxelCoord::new(
        voxel_count.x.div_euclid(stride),
        voxel_count.y.div_euclid(stride),
        voxel_count.z.div_euclid(stride),
    );
    (scaled_count.x > 1 && scaled_count.y > 1 && scaled_count.z > 1).then_some(scaled_count)
}

fn surface_net_mesh_from_dense(
    volume: &[f32],
    voxel_count: VoxelCoord,
    voxel_size: f32,
    start_coord: VoxelCoord,
    stride: i32,
) -> Option<SurfaceMesh32> {
    let stride = stride.max(1);
    let scaled_count = scaled_voxel_count(voxel_count, stride)?;

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
                let mut crossings = 0u8;
                let mut bad_crossings = 0u8;

                for (a_idx, b_idx) in SURFACE_NET_EDGES {
                    let coord_a = dense_corner_coord(coord, SURFACE_NET_CORNERS[a_idx], stride);
                    let coord_b = dense_corner_coord(coord, SURFACE_NET_CORNERS[b_idx], stride);
                    let value_a = sample_value(coord_a);
                    let value_b = sample_value(coord_b);
                    let change = value_a - value_b;
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
                let vertex_index = positions.len() as u32;
                positions.push([world.x, world.y, world.z]);
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
        Some(SurfaceMesh32 { positions, indices })
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

fn mesh_chunk_extent(layout: DebugDepthMeshLayout) -> VoxelCoord {
    let edge = layout.chunk_edge_voxels.max(1);
    let overlap = layout.overlap_voxels.max(0);
    VoxelCoord::new(edge + overlap, edge + overlap, edge + overlap)
}

fn mesh_chunk_start_coord(chunk_key: ChunkKey, layout: DebugDepthMeshLayout) -> VoxelCoord {
    let edge = layout.chunk_edge_voxels.max(1);
    VoxelCoord::new(chunk_key.x * edge, chunk_key.y * edge, chunk_key.z * edge)
}

fn mesh_chunk_world_bounds(
    voxel_size: f32,
    chunk_key: ChunkKey,
    layout: DebugDepthMeshLayout,
) -> (Vec3f, Vec3f) {
    let start = mesh_chunk_start_coord(chunk_key, layout);
    let edge = layout.chunk_edge_voxels.max(1);
    (
        vec3f(
            start.x as f32 * voxel_size,
            start.y as f32 * voxel_size,
            start.z as f32 * voxel_size,
        ),
        vec3f(
            (start.x + edge) as f32 * voxel_size,
            (start.y + edge) as f32 * voxel_size,
            (start.z + edge) as f32 * voxel_size,
        ),
    )
}

fn aabb_intersects(min_a: Vec3f, max_a: Vec3f, min_b: Vec3f, max_b: Vec3f) -> bool {
    min_a.x <= max_b.x
        && max_a.x >= min_b.x
        && min_a.y <= max_b.y
        && max_a.y >= min_b.y
        && min_a.z <= max_b.z
        && max_a.z >= min_b.z
}

fn chunk_visible_from_head(
    head_position: Vec3f,
    head_forward: Vec3f,
    world_min: Vec3f,
    world_max: Vec3f,
    layout: DebugDepthMeshLayout,
) -> bool {
    let center = (world_min + world_max).scale(0.5);
    let half_extent = (world_max - world_min).scale(0.5);
    let radius = half_extent.length();
    let to_center = center - head_position;
    let distance = to_center.length();
    if distance > layout.view_distance_meters + radius {
        return false;
    }
    if distance <= layout.near_distance_meters + radius || distance <= 1.0e-4 {
        return true;
    }
    let direction = to_center.scale(1.0 / distance);
    direction.dot(head_forward) >= layout.view_cone_dot - (radius / distance).min(0.35)
}

fn snapshot_region_signature(
    grid: &SparseTsdGridReadSnapshot,
    start: VoxelCoord,
    extent: VoxelCoord,
) -> DebugDepthMeshChunkSignature {
    if extent.x <= 0 || extent.y <= 0 || extent.z <= 0 || grid.chunks.is_empty() {
        return DebugDepthMeshChunkSignature::default();
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
    let mut sources = Vec::new();
    for z in min_chunk.z..=max_chunk.z {
        for y in min_chunk.y..=max_chunk.y {
            for x in min_chunk.x..=max_chunk.x {
                let key = ChunkKey::new(x, y, z);
                let Some(chunk) = grid.chunks.get(&key) else {
                    continue;
                };
                sources.push((key, Arc::as_ptr(chunk) as usize));
            }
        }
    }
    DebugDepthMeshChunkSignature { sources }
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

pub(crate) fn snapshot_debug_mesh_layout(snapshot: &TsdfPublishedSnapshot) -> DebugDepthMeshLayout {
    let voxel_size = snapshot.grid.voxel_size.max(1.0e-5);
    let stride = XR_DEBUG_TSDF_MESH_CELL_STRIDE_VOXELS.max(1);
    let approx_edge = (XR_DEBUG_TSDF_MESH_CHUNK_WORLD_SIZE_METERS / voxel_size).ceil() as i32;
    let chunk_edge_voxels = align_extent(approx_edge.max(stride * 2), stride);
    let overlap_voxels = stride * XR_DEBUG_TSDF_MESH_CHUNK_OVERLAP_CELLS.max(1);
    DebugDepthMeshLayout {
        chunk_edge_voxels,
        overlap_voxels,
        stride_voxels: stride,
        chunk_world_size_meters: chunk_edge_voxels as f32 * voxel_size,
        view_distance_meters: XR_DEBUG_TSDF_MESH_VIEW_DISTANCE_METERS,
        near_distance_meters: XR_DEBUG_TSDF_MESH_NEAR_VISIBILITY_METERS,
        view_cone_dot: XR_DEBUG_TSDF_MESH_VIEW_CONE_DOT,
    }
}

pub(crate) fn debug_depth_mesh_view_plan(
    snapshot: &TsdfPublishedSnapshot,
    head_pose: Pose,
) -> DebugDepthMeshViewPlan {
    let layout = snapshot_debug_mesh_layout(snapshot);
    let Some((active_min, active_max)) = snapshot.grid.active_bounds else {
        return DebugDepthMeshViewPlan {
            layout,
            visible_chunks: Vec::new(),
        };
    };

    let mut head_forward = head_pose.orientation.rotate_vec3(&vec3f(0.0, 0.0, -1.0));
    if head_forward.length() > 1.0e-4 {
        head_forward = head_forward.normalize();
    } else {
        head_forward = vec3f(0.0, 0.0, -1.0);
    }

    let (head_voxel_x, head_voxel_y, head_voxel_z) =
        snapshot.grid.world_to_voxel_xyz(head_pose.position);
    let head_chunk = VoxelCoord::new(
        head_voxel_x.div_euclid(layout.chunk_edge_voxels.max(1)),
        head_voxel_y.div_euclid(layout.chunk_edge_voxels.max(1)),
        head_voxel_z.div_euclid(layout.chunk_edge_voxels.max(1)),
    );
    let search_radius =
        (layout.view_distance_meters / layout.chunk_world_size_meters.max(0.1)).ceil() as i32 + 1;
    let extent = mesh_chunk_extent(layout);

    #[derive(Clone)]
    struct VisibilityCandidate {
        forward_depth: f32,
        distance: f32,
        plan: DebugDepthMeshChunkPlan,
    }

    let mut candidates = Vec::<VisibilityCandidate>::new();
    for z in (head_chunk.z - search_radius)..=(head_chunk.z + search_radius) {
        for y in (head_chunk.y - search_radius)..=(head_chunk.y + search_radius) {
            for x in (head_chunk.x - search_radius)..=(head_chunk.x + search_radius) {
                let chunk_key = ChunkKey::new(x, y, z);
                let (world_min, world_max) =
                    mesh_chunk_world_bounds(snapshot.grid.voxel_size, chunk_key, layout);
                if !aabb_intersects(world_min, world_max, active_min, active_max) {
                    continue;
                }
                if !chunk_visible_from_head(
                    head_pose.position,
                    head_forward,
                    world_min,
                    world_max,
                    layout,
                ) {
                    continue;
                }

                let signature = snapshot_region_signature(
                    snapshot.grid.as_ref(),
                    mesh_chunk_start_coord(chunk_key, layout),
                    extent,
                );
                if signature.sources.is_empty() {
                    continue;
                }

                let center = (world_min + world_max).scale(0.5);
                let to_center = center - head_pose.position;
                candidates.push(VisibilityCandidate {
                    forward_depth: to_center.dot(head_forward),
                    distance: to_center.length(),
                    plan: DebugDepthMeshChunkPlan {
                        chunk_key,
                        signature,
                    },
                });
            }
        }
    }

    candidates.sort_by(|left, right| {
        left.distance
            .total_cmp(&right.distance)
            .then_with(|| right.forward_depth.total_cmp(&left.forward_depth))
            .then_with(|| {
                (
                    left.plan.chunk_key.x,
                    left.plan.chunk_key.y,
                    left.plan.chunk_key.z,
                )
                    .cmp(&(
                        right.plan.chunk_key.x,
                        right.plan.chunk_key.y,
                        right.plan.chunk_key.z,
                    ))
            })
    });

    DebugDepthMeshViewPlan {
        layout,
        visible_chunks: candidates
            .into_iter()
            .map(|candidate| candidate.plan)
            .collect(),
    }
}

impl DebugDepthMeshTriangulator {
    pub(crate) fn build_chunk(
        &mut self,
        snapshot: &TsdfPublishedSnapshot,
        layout: DebugDepthMeshLayout,
        plan: &DebugDepthMeshChunkPlan,
    ) -> Option<DebugDepthMeshChunk> {
        if plan.signature.sources.is_empty() {
            return None;
        }
        let start = mesh_chunk_start_coord(plan.chunk_key, layout);
        let extent = mesh_chunk_extent(layout);
        extract_dense_region_into(snapshot.grid.as_ref(), start, extent, &mut self.dense);
        repair_dense_meshing_holes(&mut self.dense, &mut self.fill_scratch, extent);
        let mesh = surface_net_mesh_from_dense(
            &self.dense,
            extent,
            snapshot.grid.voxel_size,
            start,
            layout.stride_voxels,
        )?;
        let fingerprint = mesh_chunk_fingerprint(plan.chunk_key, &mesh);
        let (indices, vertices) = pack_surface_mesh_debug_vertices(&mesh);
        if indices.is_empty() || vertices.is_empty() {
            return None;
        }
        Some(DebugDepthMeshChunk {
            chunk_key: plan.chunk_key,
            fingerprint,
            indices,
            vertices,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::HashMap, sync::Arc};

    fn packed_position(vertices: &[f32], vertex_index: usize) -> Vec3f {
        let base = vertex_index * XR_DEBUG_DEPTH_FLOATS_PER_VERTEX;
        vec3f(vertices[base], vertices[base + 1], vertices[base + 2])
    }

    fn packed_barycentric(vertices: &[f32], vertex_index: usize) -> Vec3f {
        let base = vertex_index * XR_DEBUG_DEPTH_FLOATS_PER_VERTEX;
        vec3f(vertices[base + 4], vertices[base + 5], vertices[base + 6])
    }

    fn set_normalized_distance(
        chunks: &mut HashMap<ChunkKey, Arc<SparseTsdReadChunk>>,
        chunk_edge: i32,
        coord: VoxelCoord,
        normalized_distance: f32,
        confidence: u8,
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
        chunk.set_value(id, normalized_distance, confidence, 1);
    }

    fn make_flat_floor_snapshot_with_confidence(
        voxel_size: f32,
        confidence: u8,
    ) -> TsdfPublishedSnapshot {
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
                        confidence,
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

    fn make_flat_floor_snapshot(voxel_size: f32) -> TsdfPublishedSnapshot {
        make_flat_floor_snapshot_with_confidence(voxel_size, 8)
    }

    #[test]
    fn pack_surface_mesh_debug_vertices_expands_to_triangle_barycentrics() {
        let mesh = SurfaceMesh32 {
            positions: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [1.0, 1.0, 0.0],
                [0.0, 1.0, 0.0],
            ],
            indices: vec![0, 1, 2, 0, 2, 3],
        };

        let (indices, vertices) = pack_surface_mesh_debug_vertices(&mesh);

        assert_eq!(indices, vec![0, 1, 2, 3, 4, 5]);
        assert_eq!(vertices.len(), 6 * XR_DEBUG_DEPTH_FLOATS_PER_VERTEX);
        assert_eq!(packed_position(&vertices, 0), vec3f(0.0, 0.0, 0.0));
        assert_eq!(packed_position(&vertices, 2), vec3f(1.0, 1.0, 0.0));
        assert_eq!(packed_position(&vertices, 5), vec3f(0.0, 1.0, 0.0));
        assert_eq!(packed_barycentric(&vertices, 0), vec3f(1.0, 0.0, 0.0));
        assert_eq!(packed_barycentric(&vertices, 1), vec3f(0.0, 1.0, 0.0));
        assert_eq!(packed_barycentric(&vertices, 2), vec3f(0.0, 0.0, 1.0));
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

        assert_eq!(indices, vec![0, 1, 2, 3, 4, 5]);
        assert_eq!(vertices.len(), 6 * XR_DEBUG_DEPTH_FLOATS_PER_VERTEX);
        let first = packed_position(&vertices, 0);
        let sixth = packed_position(&vertices, 5);
        assert_eq!(first, vec3f(-0.25, 0.5, -0.10));
        assert_eq!(sixth, vec3f(-0.25, 0.5, 0.10));
        assert_eq!(packed_barycentric(&vertices, 3), vec3f(1.0, 0.0, 0.0));
        assert_eq!(packed_barycentric(&vertices, 4), vec3f(0.0, 1.0, 0.0));
        assert_eq!(packed_barycentric(&vertices, 5), vec3f(0.0, 0.0, 1.0));
    }

    #[test]
    fn debug_depth_mesh_view_plan_extracts_visible_chunk_meshes() {
        let snapshot = make_flat_floor_snapshot(0.05);
        let view_plan =
            debug_depth_mesh_view_plan(&snapshot, Pose::new(Quat::default(), vec3f(0.0, 1.4, 0.0)));

        assert!(
            !view_plan.visible_chunks.is_empty(),
            "expected at least one visible debug chunk"
        );

        let mut triangulator = DebugDepthMeshTriangulator::default();
        let built_chunks: Vec<_> = view_plan
            .visible_chunks
            .iter()
            .filter_map(|plan| triangulator.build_chunk(&snapshot, view_plan.layout, plan))
            .collect();

        assert!(
            !built_chunks.is_empty(),
            "expected triangulated visible chunks for a flat floor snapshot"
        );
        assert!(built_chunks
            .iter()
            .all(|chunk| !chunk.indices.is_empty() && !chunk.vertices.is_empty()));
    }

    #[test]
    fn debug_depth_mesh_includes_low_confidence_valid_voxels() {
        let snapshot = make_flat_floor_snapshot_with_confidence(0.05, 1);
        let view_plan =
            debug_depth_mesh_view_plan(&snapshot, Pose::new(Quat::default(), vec3f(0.0, 1.4, 0.0)));

        let mut triangulator = DebugDepthMeshTriangulator::default();
        let built_chunks: Vec<_> = view_plan
            .visible_chunks
            .iter()
            .filter_map(|plan| triangulator.build_chunk(&snapshot, view_plan.layout, plan))
            .collect();

        assert!(
            !built_chunks.is_empty(),
            "expected triangulated visible chunks even from low-confidence valid TSDF voxels"
        );
    }
}
