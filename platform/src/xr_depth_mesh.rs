use crate::{
    makepad_math::{vec2f, vec3, vec4f, Mat4f, Pose, Quat, Vec2f, Vec3f},
    makepad_micro_serde::*,
};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        Arc, OnceLock, RwLock,
    },
};

#[allow(dead_code)]
const XR_DEPTH_QUERY_MAX_PENDING: usize = 256;
pub const XR_DEPTH_MESH_DEFAULT_VOXEL_SIZE_METERS: f32 = 0.03;
const XR_DEPTH_ALIGN_MIN_WALL_SAMPLES: usize = 4;
const XR_DEPTH_ALIGN_ACCEPT_MIN_MATCHED_SAMPLES: usize = 6;
const XR_DEPTH_ALIGN_ACCEPT_MIN_CONFIDENCE: f32 = 0.12;
const XR_DEPTH_ALIGN_ACCEPT_MIN_SYMMETRY_CONFIDENCE: f32 = 0.10;
const XR_DEPTH_ALIGN_TRANSLATION_VOTE_STEP_METERS: f32 = 0.08;
#[cfg(test)]
const XR_DEPTH_ALIGN_VERTICAL_DESCRIPTOR_MIN_OVERLAP: f32 = 0.18;
const XR_DEPTH_ALIGN_HEIGHT_MAP_HISTOGRAM_BINS: usize = 48;
const XR_DEPTH_ALIGN_HEIGHT_MAP_MAX_SAMPLES: usize = 96;
const XR_DEPTH_ALIGN_HEIGHT_MAP_GRADIENT_MIN_METERS: f32 = 0.10;
const XR_DEPTH_ALIGN_HEIGHT_MAP_MIN_SPACING_METERS: f32 = 0.14;
const XR_DEPTH_ALIGN_HEIGHT_MAP_WALL_BIAS_MIN_HEIGHT_METERS: f32 = 1.2;
const XR_DEPTH_ALIGN_SEEDED_LOCK_MIN_CONFIDENCE: f32 = 0.20;
const XR_DEPTH_ALIGN_SEEDED_LOCK_MIN_SYMMETRY_CONFIDENCE: f32 = 0.18;
const XR_DEPTH_ALIGN_SEEDED_LOCK_MIN_OVERLAP: f32 = 0.35;
const XR_DEPTH_ALIGN_SEEDED_LOCK_MAX_TRANSLATION_JUMP_METERS: f32 = 0.75;
const XR_DEPTH_ALIGN_SEEDED_LOCK_MAX_YAW_JUMP_RADIANS: f32 = 0.45;

#[derive(Clone, Copy, Debug)]
struct HeightMapSignalCell {
    point: Vec3f,
    height: f32,
    weight: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ChunkKey {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl ChunkKey {
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }
}

#[derive(Clone, Debug, Default)]
pub struct XrDepthMeshStats {
    pub frames_seen: u64,
    pub frames_meshed: u64,
    pub frames_dropped: u64,
}

#[allow(dead_code)]
#[derive(Clone, Debug, Default)]
pub(crate) struct XrDepthMeshChunk {
    pub generation: u64,
    pub chunk_key: ChunkKey,
    pub fingerprint: u64,
    pub bounds_min: Vec3f,
    pub bounds_max: Vec3f,
    pub vertices: Vec<Vec3f>,
    pub normals: Vec<Vec3f>,
    pub indices: Vec<u32>,
    pub planar_patches: Vec<XrDepthPlanePatch>,
}

#[allow(dead_code)]
impl XrDepthMeshChunk {
    pub(crate) fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }
}

#[allow(dead_code)]
#[derive(Clone, Debug, Default)]
pub(crate) struct XrDepthMesh {
    pub generation: u64,
    pub latest_topology_generation: u64,
    pub update_sequence: u64,
    pub eye_index: usize,
    pub image_width: u32,
    pub image_height: u32,
    pub sample_step: u32,
    pub voxel_size_meters: f32,
    pub bounds_min: Vec3f,
    pub bounds_max: Vec3f,
    pub mesh_chunks: Vec<XrDepthMeshChunk>,
    pub plane_patches: Vec<XrDepthPlanePatch>,
    pub alignment_descriptor: Option<XrDepthAlignDescriptor>,
    pub alignment_descriptor_change_score: f32,
    pub alignment_debug: XrDepthAlignDebug,
    pub alignment_slice_preview: Option<XrDepthAlignSlicePreview>,
    pub alignment_preview: XrDepthAlignPreview,
    pub dirty_chunk_keys: Vec<ChunkKey>,
    pub removed_chunk_keys: Vec<ChunkKey>,
    pub mesh_generation: u64,
    pub mesh_vertex_count: usize,
    pub mesh_triangle_count: usize,
    pub tsdf_chunk_count: usize,
    pub tsdf_live_voxel_count: usize,
    pub tsdf_memory_bytes: u64,
}

#[derive(Clone, Debug, Default)]
pub struct XrDepthMeshState {
    pub stats: XrDepthMeshStats,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct SparseTsdReadChunk {
    pub values: Vec<f32>,
    pub valid: Vec<u8>,
    pub confidence: Vec<u8>,
    pub observed_generation: Vec<u64>,
}

impl SparseTsdReadChunk {
    pub fn heap_bytes(&self) -> u64 {
        (self.values.capacity() * std::mem::size_of::<f32>()
            + self.valid.capacity() * std::mem::size_of::<u8>()
            + self.confidence.capacity() * std::mem::size_of::<u8>()
            + self.observed_generation.capacity() * std::mem::size_of::<u64>()) as u64
    }
}

#[derive(Clone, Debug, Default)]
pub struct SparseTsdGridReadSnapshot {
    pub voxel_size: f32,
    pub chunk_edge: i32,
    pub chunk_volume: usize,
    pub active_value_count: usize,
    pub active_bounds: Option<(Vec3f, Vec3f)>,
    pub chunks: HashMap<ChunkKey, Arc<SparseTsdReadChunk>>,
}

impl SparseTsdGridReadSnapshot {
    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    pub fn heap_bytes(&self) -> u64 {
        let chunk_bytes = self
            .chunks
            .values()
            .map(|chunk| chunk.heap_bytes())
            .sum::<u64>();
        let table_bytes =
            self.chunks.capacity() as u64 * std::mem::size_of::<(ChunkKey, Arc<SparseTsdReadChunk>)>() as u64;
        chunk_bytes + table_bytes
    }
}

#[derive(Clone, Debug, Default)]
pub struct TsdfPublishedSnapshot {
    pub generation: u64,
    pub latest_topology_generation: u64,
    pub update_sequence: u64,
    pub grid: Arc<SparseTsdGridReadSnapshot>,
    pub height_map: Option<XrDepthAlignHeightMap>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, SerBin, DeBin)]
pub enum XrDepthAlignSampleKind {
    Floor,
    Wall,
    #[default]
    Unknown,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, SerBin, DeBin)]
pub struct XrDepthAlignSample {
    pub kind: XrDepthAlignSampleKind,
    pub point: Vec3f,
    pub normal: Vec3f,
    pub weight: f32,
}

#[derive(Clone, Debug, Default, PartialEq, SerBin, DeBin)]
pub struct XrDepthAlignHeightMap {
    pub origin_x: f32,
    pub origin_z: f32,
    pub cell_size_meters: f32,
    pub size_x: u16,
    pub size_z: u16,
    pub bottom_y_meters: f32,
    pub top_y_meters: f32,
    pub player_cutout_center: Option<Vec2f>,
    pub player_cutout_radius_meters: f32,
    pub height_u16: Vec<u16>,
}

impl XrDepthAlignHeightMap {
    pub fn cell_count(&self) -> usize {
        self.size_x as usize * self.size_z as usize
    }

    pub fn size_x_usize(&self) -> usize {
        self.size_x as usize
    }

    pub fn size_z_usize(&self) -> usize {
        self.size_z as usize
    }

    pub fn extent_x_meters(&self) -> f32 {
        self.cell_size_meters * self.size_x.max(1) as f32
    }

    pub fn extent_z_meters(&self) -> f32 {
        self.cell_size_meters * self.size_z.max(1) as f32
    }

    pub fn cell_index(&self, x: usize, z: usize) -> usize {
        x + z * self.size_x_usize()
    }

    pub fn is_empty(&self) -> bool {
        self.size_x == 0
            || self.size_z == 0
            || self.height_u16.len() != self.cell_count()
            || self.height_u16.iter().all(|value| *value == 0)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct XrDepthAlignSlicePreview {
    pub height_map: XrDepthAlignHeightMap,
    pub cutout_center: Option<Vec2f>,
    pub cutout_forward: Option<Vec2f>,
    pub cutout_radius_meters: f32,
}

#[derive(Clone, Debug, Default, PartialEq, SerBin, DeBin)]
pub struct XrDepthAlignVerticalDescriptor {
    pub origin_x: f32,
    pub origin_z: f32,
    pub cell_size_meters: f32,
    pub size: u16,
    pub vertical_surface_masks: Vec<u8>,
    pub clutter_surface_masks: Vec<u8>,
    pub free_space_masks: Vec<u8>,
    pub height_u8: Vec<u8>,
}

impl XrDepthAlignVerticalDescriptor {
    pub fn cell_count(&self) -> usize {
        self.size as usize * self.size as usize
    }

    pub fn is_empty(&self) -> bool {
        self.size == 0
            || self.vertical_surface_masks.len() != self.cell_count()
            || self.clutter_surface_masks.len() != self.cell_count()
            || self.free_space_masks.len() != self.cell_count()
            || self.height_u8.len() != self.cell_count()
            || self
                .vertical_surface_masks
                .iter()
                .zip(self.clutter_surface_masks.iter())
                .zip(self.free_space_masks.iter())
                .zip(self.height_u8.iter())
                .all(|(((vertical, clutter), free), height)| {
                    *vertical == 0 && *clutter == 0 && *free == 0 && *height == 0
                })
    }
}

#[derive(Clone, Debug, Default, PartialEq, SerBin, DeBin)]
pub struct XrDepthAlignDescriptor {
    pub voxel_size_meters: f32,
    pub floor_y: f32,
    pub wall_normal_histogram: Vec<f32>,
    pub samples: Vec<XrDepthAlignSample>,
    pub vertical_descriptor: Option<XrDepthAlignVerticalDescriptor>,
    pub height_map: Option<XrDepthAlignHeightMap>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, SerBin, DeBin)]
pub struct XrDepthAlignDebug {
    pub near_surface_voxel_count: u32,
    pub floor_candidate_count: u32,
    pub wall_candidate_count: u32,
    pub floor_sample_count: u32,
    pub wall_sample_count: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct XrDepthAlignSolution {
    pub yaw_radians: f32,
    pub translation: Vec3f,
    pub confidence: f32,
    pub symmetry_confidence: f32,
    pub residual_meters: f32,
    pub matched_samples: usize,
}

impl XrDepthAlignSolution {
    pub fn remote_to_local_transform(&self) -> Mat4f {
        Pose::new(
            Quat::from_axis_angle(vec3(0.0, 1.0, 0.0), self.yaw_radians),
            self.translation,
        )
        .to_mat4()
    }

    pub fn map_point(&self, point: Vec3f) -> Vec3f {
        self.remote_to_local_transform()
            .transform_vec4(vec4f(point.x, point.y, point.z, 1.0))
            .to_vec3f()
    }

    pub fn ranking_confidence(&self) -> f32 {
        (self.confidence * (0.30 + 0.70 * self.symmetry_confidence.clamp(0.0, 1.0))).clamp(0.0, 1.0)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum XrDepthAlignSolveOutcome {
    #[default]
    MissingSamples,
    NoCandidate,
    Rejected,
    Accepted,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct XrDepthAlignSolveDiagnostic {
    pub local_vertical_descriptor: bool,
    pub remote_vertical_descriptor: bool,
    pub local_floor_samples: usize,
    pub local_wall_samples: usize,
    pub remote_floor_samples: usize,
    pub remote_wall_samples: usize,
    pub yaw_candidate_count: usize,
    pub pose_candidate_count: usize,
    pub best_solution: Option<XrDepthAlignSolution>,
}

impl XrDepthAlignSolveDiagnostic {
    pub fn accepted_solution(&self) -> Option<XrDepthAlignSolution> {
        self.best_solution
            .filter(|solution| xr_depth_align_solution_is_accepted(self, *solution))
    }

    pub fn outcome(&self) -> XrDepthAlignSolveOutcome {
        if self.local_wall_samples < XR_DEPTH_ALIGN_MIN_WALL_SAMPLES
            || self.remote_wall_samples < XR_DEPTH_ALIGN_MIN_WALL_SAMPLES
        {
            return XrDepthAlignSolveOutcome::MissingSamples;
        }
        if self.best_solution.is_none() {
            XrDepthAlignSolveOutcome::NoCandidate
        } else if self.accepted_solution().is_some() {
            XrDepthAlignSolveOutcome::Accepted
        } else {
            XrDepthAlignSolveOutcome::Rejected
        }
    }
}

pub fn xr_depth_align_solution_is_accepted(
    diagnostic: &XrDepthAlignSolveDiagnostic,
    solution: XrDepthAlignSolution,
) -> bool {
    solution.matched_samples >= XR_DEPTH_ALIGN_ACCEPT_MIN_MATCHED_SAMPLES
        && solution.confidence > XR_DEPTH_ALIGN_ACCEPT_MIN_CONFIDENCE
        && (!diagnostic.local_vertical_descriptor
            || !diagnostic.remote_vertical_descriptor
            || solution.symmetry_confidence > XR_DEPTH_ALIGN_ACCEPT_MIN_SYMMETRY_CONFIDENCE)
}

pub fn xr_depth_align_loopback_preview_solution() -> XrDepthAlignSolution {
    XrDepthAlignSolution {
        yaw_radians: 0.58,
        translation: vec3(-0.82, 0.0, 0.67),
        confidence: 1.0,
        symmetry_confidence: 1.0,
        residual_meters: 0.0,
        matched_samples: 0,
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct XrDepthAlignPreview {
    pub local_markers: Option<[Vec3f; 2]>,
    pub remote_markers_local: Option<[Vec3f; 2]>,
    pub solution: Option<XrDepthAlignSolution>,
    pub local_sample_count: usize,
    pub local_floor_sample_count: usize,
    pub local_wall_sample_count: usize,
    pub remote_sample_count: usize,
    pub remote_floor_sample_count: usize,
    pub remote_wall_sample_count: usize,
}

pub fn xr_depth_align_transform_descriptor(
    descriptor: &XrDepthAlignDescriptor,
    transform: &Mat4f,
) -> XrDepthAlignDescriptor {
    let transform_dir = |dir: Vec3f| {
        transform
            .transform_vec4(vec4f(dir.x, dir.y, dir.z, 0.0))
            .to_vec3f()
    };
    let mut descriptor = descriptor.clone();
    for sample in &mut descriptor.samples {
        sample.point = transform
            .transform_vec4(vec4f(sample.point.x, sample.point.y, sample.point.z, 1.0))
            .to_vec3f();
        sample.normal = align_safe_normalize(transform_dir(sample.normal)).unwrap_or(sample.normal);
    }
    descriptor.floor_y = transform
        .transform_vec4(vec4f(0.0, descriptor.floor_y, 0.0, 1.0))
        .to_vec3f()
        .y;
    descriptor.vertical_descriptor = descriptor
        .vertical_descriptor
        .as_ref()
        .and_then(|vertical| transform_vertical_descriptor(vertical, transform));
    descriptor.height_map = descriptor
        .height_map
        .as_ref()
        .and_then(|height_map| transform_height_map(height_map, transform));
    descriptor.wall_normal_histogram =
        if !descriptor.samples.is_empty() && !descriptor.wall_normal_histogram.is_empty() {
            xr_depth_align_build_wall_normal_histogram(
                &descriptor.samples,
                descriptor.wall_normal_histogram.len(),
            )
        } else {
            Vec::new()
        };
    descriptor
}

fn transform_height_map(
    height_map: &XrDepthAlignHeightMap,
    transform: &Mat4f,
) -> Option<XrDepthAlignHeightMap> {
    let size_x = height_map.size_x_usize();
    let size_z = height_map.size_z_usize();
    if size_x == 0 || size_z == 0 || height_map.height_u16.len() != size_x * size_z {
        return None;
    }
    let cell_size = height_map.cell_size_meters.max(1.0e-5);
    let extent_x = cell_size * size_x as f32;
    let extent_z = cell_size * size_z as f32;
    let corners = [
        vec3(height_map.origin_x, 0.0, height_map.origin_z),
        vec3(height_map.origin_x + extent_x, 0.0, height_map.origin_z),
        vec3(
            height_map.origin_x + extent_x,
            0.0,
            height_map.origin_z + extent_z,
        ),
        vec3(height_map.origin_x, 0.0, height_map.origin_z + extent_z),
    ];
    let mut min_x = f32::INFINITY;
    let mut min_z = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_z = f32::NEG_INFINITY;
    for corner in corners {
        let transformed = transform
            .transform_vec4(vec4f(corner.x, corner.y, corner.z, 1.0))
            .to_vec3f();
        min_x = min_x.min(transformed.x);
        max_x = max_x.max(transformed.x);
        min_z = min_z.min(transformed.z);
        max_z = max_z.max(transformed.z);
    }
    if !min_x.is_finite() || !min_z.is_finite() || !max_x.is_finite() || !max_z.is_finite() {
        return None;
    }
    let min_cell_x = (min_x / cell_size).floor() as i32;
    let max_cell_x = (max_x / cell_size).ceil() as i32;
    let min_cell_z = (min_z / cell_size).floor() as i32;
    let max_cell_z = (max_z / cell_size).ceil() as i32;
    let target_size_x = (max_cell_x - min_cell_x).max(1) as usize;
    let target_size_z = (max_cell_z - min_cell_z).max(1) as usize;
    if target_size_x > u16::MAX as usize || target_size_z > u16::MAX as usize {
        return None;
    }
    let origin_x = min_cell_x as f32 * cell_size;
    let origin_z = min_cell_z as f32 * cell_size;
    let y_offset = transform
        .transform_vec4(vec4f(0.0, 0.0, 0.0, 1.0))
        .to_vec3f()
        .y;
    let inverse = transform.invert();
    let mut transformed = XrDepthAlignHeightMap {
        origin_x,
        origin_z,
        cell_size_meters: cell_size,
        size_x: target_size_x as u16,
        size_z: target_size_z as u16,
        bottom_y_meters: height_map.bottom_y_meters + y_offset,
        top_y_meters: height_map.top_y_meters + y_offset,
        player_cutout_center: height_map.player_cutout_center.map(|center| {
            let mapped = transform
                .transform_vec4(vec4f(center.x, 0.0, center.y, 1.0))
                .to_vec3f();
            vec2f(mapped.x, mapped.z)
        }),
        player_cutout_radius_meters: height_map.player_cutout_radius_meters,
        height_u16: vec![0; target_size_x * target_size_z],
    };
    for z in 0..target_size_z {
        for x in 0..target_size_x {
            let world_x = origin_x + (x as f32 + 0.5) * transformed.cell_size_meters;
            let world_z = origin_z + (z as f32 + 0.5) * transformed.cell_size_meters;
            let source = inverse
                .transform_vec4(vec4f(world_x, 0.0, world_z, 1.0))
                .to_vec3f();
            let Some(height) = sample_height_map_nearest(height_map, source.x, source.z) else {
                continue;
            };
            let target_index = transformed.cell_index(x, z);
            transformed.height_u16[target_index] =
                encode_height_map_height(&transformed, height + y_offset);
        }
    }
    Some(transformed)
}

fn encode_height_map_height(height_map: &XrDepthAlignHeightMap, height: f32) -> u16 {
    let span = (height_map.top_y_meters - height_map.bottom_y_meters).max(1.0e-3);
    let normalized = ((height - height_map.bottom_y_meters) / span).clamp(0.0, 1.0);
    1 + (normalized * 65534.0).round() as u16
}

fn transform_vertical_descriptor(
    descriptor: &XrDepthAlignVerticalDescriptor,
    transform: &Mat4f,
) -> Option<XrDepthAlignVerticalDescriptor> {
    let size = descriptor.size as usize;
    if size == 0
        || descriptor.vertical_surface_masks.len() != size * size
        || descriptor.clutter_surface_masks.len() != size * size
        || descriptor.free_space_masks.len() != size * size
        || descriptor.height_u8.len() != size * size
    {
        return None;
    }
    let max = descriptor.origin_x + descriptor.cell_size_meters * size as f32;
    let max_z = descriptor.origin_z + descriptor.cell_size_meters * size as f32;
    let corners = [
        vec3(descriptor.origin_x, 0.0, descriptor.origin_z),
        vec3(max, 0.0, descriptor.origin_z),
        vec3(max, 0.0, max_z),
        vec3(descriptor.origin_x, 0.0, max_z),
    ];
    let mut min_x = f32::INFINITY;
    let mut min_z = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_z = f32::NEG_INFINITY;
    for corner in corners {
        let transformed = transform
            .transform_vec4(vec4f(corner.x, corner.y, corner.z, 1.0))
            .to_vec3f();
        min_x = min_x.min(transformed.x);
        max_x = max_x.max(transformed.x);
        min_z = min_z.min(transformed.z);
        max_z = max_z.max(transformed.z);
    }
    if !min_x.is_finite() || !min_z.is_finite() || !max_x.is_finite() || !max_z.is_finite() {
        return None;
    }
    let extent = (max_x - min_x)
        .max(max_z - min_z)
        .max(descriptor.cell_size_meters);
    let origin_x = (min_x + max_x) * 0.5 - extent * 0.5;
    let origin_z = (min_z + max_z) * 0.5 - extent * 0.5;
    let cell_size = extent / size as f32;
    let mut transformed = XrDepthAlignVerticalDescriptor {
        origin_x,
        origin_z,
        cell_size_meters: cell_size,
        size: descriptor.size,
        vertical_surface_masks: vec![0; size * size],
        clutter_surface_masks: vec![0; size * size],
        free_space_masks: vec![0; size * size],
        height_u8: vec![0; size * size],
    };
    for z in 0..size {
        for x in 0..size {
            let index = x + z * size;
            let active = descriptor.vertical_surface_masks[index] != 0
                || descriptor.clutter_surface_masks[index] != 0
                || descriptor.free_space_masks[index] != 0
                || descriptor.height_u8[index] != 0;
            if !active {
                continue;
            }
            let point = vec3(
                descriptor.origin_x + (x as f32 + 0.5) * descriptor.cell_size_meters,
                0.0,
                descriptor.origin_z + (z as f32 + 0.5) * descriptor.cell_size_meters,
            );
            let mapped = transform
                .transform_vec4(vec4f(point.x, point.y, point.z, 1.0))
                .to_vec3f();
            let tx = ((mapped.x - origin_x) / cell_size).floor() as isize;
            let tz = ((mapped.z - origin_z) / cell_size).floor() as isize;
            if tx < 0 || tz < 0 || tx >= size as isize || tz >= size as isize {
                continue;
            }
            let target_index = tx as usize + tz as usize * size;
            transformed.vertical_surface_masks[target_index] |=
                descriptor.vertical_surface_masks[index];
            transformed.clutter_surface_masks[target_index] |=
                descriptor.clutter_surface_masks[index];
            transformed.free_space_masks[target_index] |= descriptor.free_space_masks[index];
            transformed.height_u8[target_index] =
                transformed.height_u8[target_index].max(descriptor.height_u8[index]);
        }
    }
    Some(transformed)
}

fn decode_height_map_height(height_map: &XrDepthAlignHeightMap, encoded: u16) -> Option<f32> {
    if encoded == 0 {
        return None;
    }
    let span = (height_map.top_y_meters - height_map.bottom_y_meters).max(1.0e-3);
    let normalized = (encoded - 1) as f32 / 65534.0;
    Some(height_map.bottom_y_meters + normalized * span)
}

fn build_height_map_alignment_samples(
    height_map: &XrDepthAlignHeightMap,
) -> Vec<XrDepthAlignSample> {
    let size_x = height_map.size_x_usize();
    let size_z = height_map.size_z_usize();
    if size_x < 3 || size_z < 3 || height_map.height_u16.len() != size_x * size_z {
        return Vec::new();
    }

    let cell_size = height_map.cell_size_meters.max(1.0e-3);
    let mut candidates = Vec::<XrDepthAlignSample>::new();

    for z in 1..size_z - 1 {
        for x in 1..size_x - 1 {
            let Some((center, gradient)) = height_map_cell_signal(height_map, x, z) else {
                continue;
            };
            let magnitude = gradient.length();
            if magnitude < XR_DEPTH_ALIGN_HEIGHT_MAP_GRADIENT_MIN_METERS {
                continue;
            }
            let Some(normal) = align_safe_normalize(gradient) else {
                continue;
            };

            let straightness = height_map_straightness(height_map, x, z, normal);
            let weight = height_map_signal_weight(height_map, center, magnitude, straightness);
            candidates.push(XrDepthAlignSample {
                kind: XrDepthAlignSampleKind::Wall,
                // Matching is only solving x/z/yaw, so ignore absolute height drift.
                point: vec3(
                    height_map.origin_x + (x as f32 + 0.5) * cell_size,
                    0.0,
                    height_map.origin_z + (z as f32 + 0.5) * cell_size,
                ),
                normal,
                weight,
            });
        }
    }

    candidates.sort_by(|a, b| b.weight.total_cmp(&a.weight));
    let mut selected = Vec::<XrDepthAlignSample>::new();
    for candidate in candidates {
        if selected.iter().any(|existing| {
            let delta = existing.point - candidate.point;
            (delta.x * delta.x + delta.z * delta.z).sqrt()
                < XR_DEPTH_ALIGN_HEIGHT_MAP_MIN_SPACING_METERS
        }) {
            continue;
        }
        selected.push(candidate);
        if selected.len() >= XR_DEPTH_ALIGN_HEIGHT_MAP_MAX_SAMPLES {
            break;
        }
    }
    selected
}

fn height_map_signal_weight(
    height_map: &XrDepthAlignHeightMap,
    height: f32,
    gradient_magnitude: f32,
    straightness: f32,
) -> f32 {
    let height_span = (height_map.top_y_meters - height_map.bottom_y_meters).max(1.0e-3);
    let wall_bias_span =
        (height_map.top_y_meters - XR_DEPTH_ALIGN_HEIGHT_MAP_WALL_BIAS_MIN_HEIGHT_METERS).max(0.25);
    let height_bias = ((height - height_map.bottom_y_meters) / height_span).clamp(0.0, 1.0);
    let wall_bias = ((height - XR_DEPTH_ALIGN_HEIGHT_MAP_WALL_BIAS_MIN_HEIGHT_METERS)
        / wall_bias_span)
        .clamp(0.0, 1.0);
    (gradient_magnitude * height_map.cell_size_meters.max(1.0e-3) * 1.8).clamp(0.14, 2.4)
        * (0.70 + 0.30 * height_bias)
        * (1.0 + 0.35 * wall_bias)
        * (0.65 + 0.70 * straightness.clamp(0.0, 1.0))
}

fn height_map_cell_signal(
    height_map: &XrDepthAlignHeightMap,
    x: usize,
    z: usize,
) -> Option<(f32, Vec3f)> {
    let size_x = height_map.size_x_usize();
    let size_z = height_map.size_z_usize();
    if size_x < 3 || size_z < 3 || x == 0 || z == 0 || x + 1 >= size_x || z + 1 >= size_z {
        return None;
    }
    let center = decode_height_map_height(height_map, height_map.height_u16[height_map.cell_index(x, z)])?;
    let left = decode_height_map_height(height_map, height_map.height_u16[height_map.cell_index(x - 1, z)])?;
    let right = decode_height_map_height(height_map, height_map.height_u16[height_map.cell_index(x + 1, z)])?;
    let up = decode_height_map_height(height_map, height_map.height_u16[height_map.cell_index(x, z - 1)])?;
    let down = decode_height_map_height(height_map, height_map.height_u16[height_map.cell_index(x, z + 1)])?;
    let cell_size = height_map.cell_size_meters.max(1.0e-3);
    let gradient = vec3(
        (right - left) / (2.0 * cell_size),
        0.0,
        (down - up) / (2.0 * cell_size),
    );
    Some((center, gradient))
}

fn height_map_straightness(
    height_map: &XrDepthAlignHeightMap,
    x: usize,
    z: usize,
    normal: Vec3f,
) -> f32 {
    let mut alignment_sum = 0.0;
    let mut count = 0usize;
    for (nx, nz) in [(x - 1, z), (x + 1, z), (x, z - 1), (x, z + 1)] {
        let Some((_height, gradient)) = height_map_cell_signal(height_map, nx, nz) else {
            continue;
        };
        let magnitude = gradient.length();
        if magnitude < XR_DEPTH_ALIGN_HEIGHT_MAP_GRADIENT_MIN_METERS * 0.7 {
            continue;
        }
        let Some(neighbor_normal) = align_safe_normalize(gradient) else {
            continue;
        };
        alignment_sum += normal.dot(neighbor_normal).abs();
        count += 1;
    }
    if count == 0 {
        0.5
    } else {
        (alignment_sum / count as f32).clamp(0.0, 1.0)
    }
}

fn build_height_map_signal_cells(height_map: &XrDepthAlignHeightMap) -> Vec<HeightMapSignalCell> {
    let size_x = height_map.size_x_usize();
    let size_z = height_map.size_z_usize();
    if size_x < 3 || size_z < 3 {
        return Vec::new();
    }
    let mut signal = Vec::<HeightMapSignalCell>::new();
    for z in (1..size_z - 1).step_by(2) {
        for x in (1..size_x - 1).step_by(2) {
            let Some((height, gradient)) = height_map_cell_signal(height_map, x, z) else {
                continue;
            };
            let magnitude = gradient.length();
            let Some(normal) = align_safe_normalize(gradient) else {
                continue;
            };
            let straightness = height_map_straightness(height_map, x, z, normal);
            let weight = height_map_signal_weight(height_map, height, magnitude, straightness);
            if weight < 0.12 {
                continue;
            }
            signal.push(HeightMapSignalCell {
                point: vec3(
                    height_map.origin_x + (x as f32 + 0.5) * height_map.cell_size_meters,
                    0.0,
                    height_map.origin_z + (z as f32 + 0.5) * height_map.cell_size_meters,
                ),
                height,
                weight,
            });
        }
    }
    signal
}

fn descriptor_height_map_samples(descriptor: &XrDepthAlignDescriptor) -> Vec<XrDepthAlignSample> {
    descriptor
        .height_map
        .as_ref()
        .map(build_height_map_alignment_samples)
        .unwrap_or_default()
}

fn sample_height_map_nearest(
    height_map: &XrDepthAlignHeightMap,
    world_x: f32,
    world_z: f32,
) -> Option<f32> {
    let size_x = height_map.size_x_usize();
    let size_z = height_map.size_z_usize();
    if size_x == 0 || size_z == 0 || height_map.height_u16.len() != size_x * size_z {
        return None;
    }
    let cell_size = height_map.cell_size_meters.max(1.0e-3);
    let grid_x = ((world_x - height_map.origin_x) / cell_size).floor() as isize;
    let grid_z = ((world_z - height_map.origin_z) / cell_size).floor() as isize;
    if grid_x < 0 || grid_z < 0 || grid_x >= size_x as isize || grid_z >= size_z as isize {
        return None;
    }
    decode_height_map_height(
        height_map,
        height_map.height_u16[height_map.cell_index(grid_x as usize, grid_z as usize)],
    )
}

fn sample_height_map_bilinear(
    height_map: &XrDepthAlignHeightMap,
    world_x: f32,
    world_z: f32,
) -> Option<f32> {
    let size_x = height_map.size_x_usize();
    let size_z = height_map.size_z_usize();
    if size_x == 0 || size_z == 0 || height_map.height_u16.len() != size_x * size_z {
        return None;
    }
    if size_x == 1 && size_z == 1 {
        return decode_height_map_height(height_map, height_map.height_u16[0]);
    }

    let cell_size = height_map.cell_size_meters.max(1.0e-3);
    let sample_x =
        ((world_x - height_map.origin_x) / cell_size - 0.5).clamp(0.0, size_x as f32 - 1.0);
    let sample_z =
        ((world_z - height_map.origin_z) / cell_size - 0.5).clamp(0.0, size_z as f32 - 1.0);
    let x0 = sample_x.floor() as usize;
    let z0 = sample_z.floor() as usize;
    let x1 = (x0 + 1).min(size_x - 1);
    let z1 = (z0 + 1).min(size_z - 1);
    let fx = (sample_x - x0 as f32).clamp(0.0, 1.0);
    let fz = (sample_z - z0 as f32).clamp(0.0, 1.0);

    let h00 = decode_height_map_height(height_map, height_map.height_u16[height_map.cell_index(x0, z0)]);
    let h10 = decode_height_map_height(height_map, height_map.height_u16[height_map.cell_index(x1, z0)]);
    let h01 = decode_height_map_height(height_map, height_map.height_u16[height_map.cell_index(x0, z1)]);
    let h11 = decode_height_map_height(height_map, height_map.height_u16[height_map.cell_index(x1, z1)]);
    match (h00, h10, h01, h11) {
        (Some(h00), Some(h10), Some(h01), Some(h11)) => {
            let hx0 = h00 + (h10 - h00) * fx;
            let hx1 = h01 + (h11 - h01) * fx;
            Some(hx0 + (hx1 - hx0) * fz)
        }
        _ => sample_height_map_nearest(height_map, world_x, world_z),
    }
}

fn score_height_map_alignment(
    local_map: &XrDepthAlignHeightMap,
    remote_map: &XrDepthAlignHeightMap,
    remote_signal: &[HeightMapSignalCell],
    yaw: f32,
    translation: Vec3f,
) -> (f32, f32, usize) {
    if remote_signal.is_empty() {
        return (0.0, f32::INFINITY, 0);
    }
    let mapped_remote_cutout_center = remote_map
        .player_cutout_center
        .map(|center| rotate_y(yaw, vec3(center.x, 0.0, center.y)) + translation);
    let mapped_remote_cutout_radius =
        (remote_map.player_cutout_radius_meters + 0.14).max(remote_map.cell_size_meters * 2.0);
    let mut support_sum = 0.0;
    let mut weight_sum = 0.0;
    let mut total_weight = 0.0;
    let mut residual_sum = 0.0;
    let mut matched = 0usize;
    for cell in remote_signal {
        total_weight += cell.weight;
        let mapped = rotate_y(yaw, cell.point) + translation;
        if mapped_remote_cutout_center.is_some_and(|center| {
            let delta = mapped - center;
            (delta.x * delta.x + delta.z * delta.z).sqrt() <= mapped_remote_cutout_radius
        }) {
            continue;
        }
        let Some(local_height) = sample_height_map_bilinear(local_map, mapped.x, mapped.z) else {
            continue;
        };
        let diff = (local_height - cell.height).abs();
        let similarity = (1.0 - diff / 0.45).clamp(0.0, 1.0);
        support_sum += cell.weight * similarity;
        weight_sum += cell.weight;
        residual_sum += diff;
        matched += 1;
    }

    if matched < 8 || weight_sum <= 1.0e-4 || total_weight <= 1.0e-4 {
        return (0.0, f32::INFINITY, matched);
    }
    let coverage = (weight_sum / total_weight).clamp(0.0, 1.0);
    if coverage < 0.20 {
        return (0.0, f32::INFINITY, matched);
    }
    (
        (support_sum / weight_sum) * coverage.sqrt(),
        residual_sum / matched as f32,
        matched,
    )
}

fn apply_height_map_alignment_support(
    candidate: XrDepthAlignSolution,
    local_map: Option<&XrDepthAlignHeightMap>,
    remote_map: Option<&XrDepthAlignHeightMap>,
    remote_signal: &[HeightMapSignalCell],
) -> XrDepthAlignSolution {
    let (Some(local_map), Some(remote_map)) = (local_map, remote_map) else {
        return candidate;
    };
    let (support, residual, matched) = score_height_map_alignment(
        local_map,
        remote_map,
        remote_signal,
        candidate.yaw_radians,
        candidate.translation,
    );
    if matched == 0 {
        return candidate;
    }
    let mut candidate = candidate;
    candidate.confidence = (candidate.confidence * 0.45 + support * 0.55).clamp(0.0, 1.0);
    candidate.symmetry_confidence = support.clamp(0.0, 1.0);
    if residual.is_finite() {
        candidate.residual_meters = if candidate.residual_meters.is_finite() {
            candidate.residual_meters * 0.55 + residual * 0.45
        } else {
            residual
        };
    }
    candidate
}

fn score_full_alignment_solution(
    local_walls: &[&XrDepthAlignSample],
    remote_walls: &[&XrDepthAlignSample],
    local_map: Option<&XrDepthAlignHeightMap>,
    remote_map: Option<&XrDepthAlignHeightMap>,
    remote_signal: &[HeightMapSignalCell],
    yaw: f32,
    translation: Vec3f,
) -> XrDepthAlignSolution {
    apply_height_map_alignment_support(
        score_alignment_solution(local_walls, remote_walls, yaw, translation),
        local_map,
        remote_map,
        remote_signal,
    )
}

fn height_map_score_better(candidate: (f32, f32, usize), current: (f32, f32, usize)) -> bool {
    candidate
        .0
        .partial_cmp(&current.0)
        .unwrap_or(std::cmp::Ordering::Equal)
        .then_with(|| {
            current
                .1
                .partial_cmp(&candidate.1)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .then_with(|| candidate.2.cmp(&current.2))
        .is_gt()
}

fn refine_height_map_alignment(
    local_map: &XrDepthAlignHeightMap,
    remote_map: &XrDepthAlignHeightMap,
    remote_signal: &[HeightMapSignalCell],
    yaw: f32,
    translation: Vec3f,
) -> (f32, Vec3f) {
    let mut best_yaw = wrap_angle(yaw);
    let mut best_translation = translation;
    let mut best_score = score_height_map_alignment(
        local_map,
        remote_map,
        remote_signal,
        best_yaw,
        best_translation,
    );
    for (yaw_step, translation_step) in [
        (0.18, 0.36),
        (0.08, 0.14),
        (0.03, 0.06),
        (0.012, 0.025),
        (0.005, 0.01),
    ] {
        loop {
            let mut improved = false;
            for yaw_delta in [-yaw_step, 0.0, yaw_step] {
                for tx_delta in [-translation_step, 0.0, translation_step] {
                    for tz_delta in [-translation_step, 0.0, translation_step] {
                        if yaw_delta == 0.0 && tx_delta == 0.0 && tz_delta == 0.0 {
                            continue;
                        }
                        let candidate_yaw = wrap_angle(best_yaw + yaw_delta);
                        let candidate_translation = vec3(
                            best_translation.x + tx_delta,
                            best_translation.y,
                            best_translation.z + tz_delta,
                        );
                        let candidate_score = score_height_map_alignment(
                            local_map,
                            remote_map,
                            remote_signal,
                            candidate_yaw,
                            candidate_translation,
                        );
                        if height_map_score_better(candidate_score, best_score) {
                            best_yaw = candidate_yaw;
                            best_translation = candidate_translation;
                            best_score = candidate_score;
                            improved = true;
                        }
                    }
                }
            }
            if !improved {
                break;
            }
        }
    }
    (best_yaw, best_translation)
}

fn refine_seed_alignment_solution(
    local_walls: &[&XrDepthAlignSample],
    remote_walls: &[&XrDepthAlignSample],
    local_map: Option<&XrDepthAlignHeightMap>,
    remote_map: Option<&XrDepthAlignHeightMap>,
    remote_signal: &[HeightMapSignalCell],
    floor_y: f32,
    seed: XrDepthAlignSolution,
) -> XrDepthAlignSolution {
    let mut best_yaw = wrap_angle(seed.yaw_radians);
    let mut best_translation = seed.translation;
    best_translation.y = floor_y;
    if let (Some(local_map), Some(remote_map)) = (local_map, remote_map) {
        if !remote_signal.is_empty() {
            (best_yaw, best_translation) = refine_height_map_alignment(
                local_map,
                remote_map,
                remote_signal,
                best_yaw,
                best_translation,
            );
            best_translation.y = floor_y;
        }
    }
    let (wall_refined_yaw, wall_refined_translation) = refine_alignment(
        local_walls,
        remote_walls,
        floor_y,
        best_yaw,
        best_translation,
    );
    let wall_translation_jump = vec3(
        wall_refined_translation.x - best_translation.x,
        0.0,
        wall_refined_translation.z - best_translation.z,
    )
    .length();
    if wrap_angle(wall_refined_yaw - best_yaw).abs() <= 0.18 && wall_translation_jump <= 0.28 {
        best_yaw = wall_refined_yaw;
        best_translation = wall_refined_translation;
    }
    let mut best = score_full_alignment_solution(
        local_walls,
        remote_walls,
        local_map,
        remote_map,
        remote_signal,
        best_yaw,
        best_translation,
    );
    for (yaw_step, translation_step) in [
        (0.10, 0.18),
        (0.04, 0.07),
        (0.015, 0.03),
        (0.006, 0.012),
    ] {
        loop {
            let mut improved = false;
            for yaw_delta in [-yaw_step, 0.0, yaw_step] {
                for tx_delta in [-translation_step, 0.0, translation_step] {
                    for tz_delta in [-translation_step, 0.0, translation_step] {
                        if yaw_delta == 0.0 && tx_delta == 0.0 && tz_delta == 0.0 {
                            continue;
                        }
                        let candidate = score_full_alignment_solution(
                            local_walls,
                            remote_walls,
                            local_map,
                            remote_map,
                            remote_signal,
                            wrap_angle(best.yaw_radians + yaw_delta),
                            vec3(
                                best.translation.x + tx_delta,
                                floor_y,
                                best.translation.z + tz_delta,
                            ),
                        );
                        if alignment_solution_better(&candidate, &best) {
                            best = candidate;
                            improved = true;
                        }
                    }
                }
            }
            if !improved {
                break;
            }
        }
    }
    best
}

fn seeded_alignment_lock_is_strong(
    diagnostic: &XrDepthAlignSolveDiagnostic,
    candidate: XrDepthAlignSolution,
    previous: XrDepthAlignSolution,
    local_map: Option<&XrDepthAlignHeightMap>,
    remote_map: Option<&XrDepthAlignHeightMap>,
) -> bool {
    if !xr_depth_align_solution_is_accepted(diagnostic, candidate) {
        return false;
    }
    if candidate.confidence < XR_DEPTH_ALIGN_SEEDED_LOCK_MIN_CONFIDENCE
        || candidate.symmetry_confidence < XR_DEPTH_ALIGN_SEEDED_LOCK_MIN_SYMMETRY_CONFIDENCE
    {
        return false;
    }
    let overlap =
        candidate.matched_samples as f32 / diagnostic.remote_wall_samples.max(1) as f32;
    if overlap < XR_DEPTH_ALIGN_SEEDED_LOCK_MIN_OVERLAP {
        return false;
    }
    let max_cell_size = local_map
        .map(|map| map.cell_size_meters)
        .unwrap_or(0.03)
        .max(remote_map.map(|map| map.cell_size_meters).unwrap_or(0.03));
    let max_residual_meters = (max_cell_size * 4.0).clamp(0.08, 0.16);
    if !candidate.residual_meters.is_finite() || candidate.residual_meters > max_residual_meters {
        return false;
    }
    let translation_jump = vec3(
        candidate.translation.x - previous.translation.x,
        0.0,
        candidate.translation.z - previous.translation.z,
    )
    .length();
    translation_jump <= XR_DEPTH_ALIGN_SEEDED_LOCK_MAX_TRANSLATION_JUMP_METERS
        && wrap_angle(candidate.yaw_radians - previous.yaw_radians).abs()
            <= XR_DEPTH_ALIGN_SEEDED_LOCK_MAX_YAW_JUMP_RADIANS
}

pub fn xr_depth_align_test_markers(descriptor: &XrDepthAlignDescriptor) -> Option<[Vec3f; 2]> {
    let wall_samples = descriptor_height_map_samples(descriptor);
    let mut best = None::<(f32, f32, Vec3f, Vec3f)>;
    for (index, first) in wall_samples.iter().enumerate() {
        for second in wall_samples.iter().skip(index + 1) {
            let distance = (second.point - first.point).length();
            if distance < 0.18 {
                continue;
            }
            let weight = first.weight + second.weight;
            if best
                .as_ref()
                .is_none_or(|(best_distance, best_weight, _, _)| {
                    distance > *best_distance + 1.0e-4
                        || ((distance - *best_distance).abs() <= 1.0e-4 && weight > *best_weight)
                })
            {
                best = Some((distance, weight, first.point, second.point));
            }
        }
    }
    best.map(|(_, _, first, second)| [first, second])
}

pub fn xr_depth_align_solve_remote_to_local(
    local: &XrDepthAlignDescriptor,
    remote: &XrDepthAlignDescriptor,
) -> Option<XrDepthAlignSolution> {
    xr_depth_align_analyze_remote_to_local(local, remote).accepted_solution()
}

pub fn xr_depth_align_analyze_remote_to_local(
    local: &XrDepthAlignDescriptor,
    remote: &XrDepthAlignDescriptor,
) -> XrDepthAlignSolveDiagnostic {
    xr_depth_align_analyze_remote_to_local_seeded(local, remote, None)
}

pub fn xr_depth_align_analyze_remote_to_local_seeded(
    local: &XrDepthAlignDescriptor,
    remote: &XrDepthAlignDescriptor,
    previous_solution: Option<XrDepthAlignSolution>,
) -> XrDepthAlignSolveDiagnostic {
    let local_height_map_samples = descriptor_height_map_samples(local);
    let remote_height_map_samples = descriptor_height_map_samples(remote);
    let remote_height_map_signal =
        if !local_height_map_samples.is_empty() && !remote_height_map_samples.is_empty() {
            remote
                .height_map
                .as_ref()
                .map(build_height_map_signal_cells)
                .unwrap_or_default()
        } else {
            Vec::new()
        };
    let local_wall_samples = local_height_map_samples.iter().collect::<Vec<_>>();
    let remote_wall_samples = remote_height_map_samples.iter().collect::<Vec<_>>();
    let local_wall_histogram = if !local_height_map_samples.is_empty() {
        xr_depth_align_build_wall_normal_histogram(
            &local_height_map_samples,
            XR_DEPTH_ALIGN_HEIGHT_MAP_HISTOGRAM_BINS,
        )
    } else {
        Vec::new()
    };
    let remote_wall_histogram = if !remote_height_map_samples.is_empty() {
        xr_depth_align_build_wall_normal_histogram(
            &remote_height_map_samples,
            XR_DEPTH_ALIGN_HEIGHT_MAP_HISTOGRAM_BINS,
        )
    } else {
        Vec::new()
    };
    let local_vertical_descriptor = local.vertical_descriptor.as_ref();
    let remote_vertical_descriptor = remote.vertical_descriptor.as_ref();
    let diagnostic = XrDepthAlignSolveDiagnostic {
        local_vertical_descriptor: local_vertical_descriptor.is_some(),
        remote_vertical_descriptor: remote_vertical_descriptor.is_some(),
        local_wall_samples: local_wall_samples.len(),
        remote_wall_samples: remote_wall_samples.len(),
        ..XrDepthAlignSolveDiagnostic::default()
    };
    let mut best_diagnostic = None::<XrDepthAlignSolveDiagnostic>;
    if local_wall_samples.len() >= XR_DEPTH_ALIGN_MIN_WALL_SAMPLES
        && remote_wall_samples.len() >= XR_DEPTH_ALIGN_MIN_WALL_SAMPLES
    {
        let mut sample_diagnostic = diagnostic;
        let floor_y = local.floor_y - remote.floor_y;
        let local_map = local.height_map.as_ref();
        let remote_map = remote.height_map.as_ref();
        let seeded_candidate = previous_solution.map(|seed| {
            sample_diagnostic.yaw_candidate_count += 1;
            sample_diagnostic.pose_candidate_count += 1;
            refine_seed_alignment_solution(
                &local_wall_samples,
                &remote_wall_samples,
                local_map,
                remote_map,
                &remote_height_map_signal,
                floor_y,
                seed,
            )
        });
        if let (Some(seed), Some(candidate)) = (previous_solution, seeded_candidate) {
            sample_diagnostic.best_solution = Some(candidate);
            if seeded_alignment_lock_is_strong(
                &sample_diagnostic,
                candidate,
                seed,
                local_map,
                remote_map,
            ) {
                return sample_diagnostic;
            }
        }
        let mut best = seeded_candidate;
        let yaw_candidates = candidate_yaws(
            &local_wall_histogram,
            &remote_wall_histogram,
            &local_wall_samples,
            &remote_wall_samples,
        );
        sample_diagnostic.yaw_candidate_count = yaw_candidates.len();
        for yaw in yaw_candidates {
            let translations =
                candidate_translations(&local_wall_samples, &remote_wall_samples, floor_y, yaw);
            sample_diagnostic.pose_candidate_count += translations.len();
            for translation in translations {
                let (mut refined_yaw, mut refined_translation) = refine_alignment(
                    &local_wall_samples,
                    &remote_wall_samples,
                    floor_y,
                    yaw,
                    translation,
                );
                if let Some(local_map) = local_map {
                    if !remote_height_map_signal.is_empty() {
                        (refined_yaw, refined_translation) = refine_height_map_alignment(
                            local_map,
                            remote_map.unwrap(),
                            &remote_height_map_signal,
                            refined_yaw,
                            refined_translation,
                        );
                        refined_translation.y = floor_y;
                    }
                }
                let candidate = score_full_alignment_solution(
                    &local_wall_samples,
                    &remote_wall_samples,
                    local_map,
                    remote_map,
                    &remote_height_map_signal,
                    refined_yaw,
                    refined_translation,
                );
                if best
                    .as_ref()
                    .is_none_or(|current| alignment_solution_better(&candidate, current))
                {
                    best = Some(candidate);
                }
            }
        }
        sample_diagnostic.best_solution = best;
        best_diagnostic = Some(sample_diagnostic);
    }
    best_diagnostic.unwrap_or(diagnostic)
}

pub fn xr_depth_align_rescore_remote_to_local(
    local: &XrDepthAlignDescriptor,
    remote: &XrDepthAlignDescriptor,
    solution: XrDepthAlignSolution,
) -> XrDepthAlignSolution {
    let local_height_map_samples = descriptor_height_map_samples(local);
    let remote_height_map_samples = descriptor_height_map_samples(remote);
    let remote_height_map_signal =
        if !local_height_map_samples.is_empty() && !remote_height_map_samples.is_empty() {
            remote
                .height_map
                .as_ref()
                .map(build_height_map_signal_cells)
                .unwrap_or_default()
        } else {
            Vec::new()
        };
    let local_wall_samples = local_height_map_samples.iter().collect::<Vec<_>>();
    let remote_wall_samples = remote_height_map_samples.iter().collect::<Vec<_>>();
    if !local_wall_samples.is_empty() && !remote_wall_samples.is_empty() {
        return apply_height_map_alignment_support(
            score_alignment_solution(
                &local_wall_samples,
                &remote_wall_samples,
                solution.yaw_radians,
                solution.translation,
            ),
            local.height_map.as_ref(),
            remote.height_map.as_ref(),
            &remote_height_map_signal,
        );
    }
    XrDepthAlignSolution {
        yaw_radians: solution.yaw_radians,
        translation: solution.translation,
        confidence: 0.0,
        symmetry_confidence: 0.0,
        residual_meters: f32::INFINITY,
        matched_samples: 0,
    }
}

pub fn xr_depth_align_build_wall_normal_histogram(
    samples: &[XrDepthAlignSample],
    bin_count: usize,
) -> Vec<f32> {
    let bin_count = bin_count.max(1);
    let mut histogram = vec![0.0; bin_count];
    for sample in samples {
        if sample.kind != XrDepthAlignSampleKind::Wall {
            continue;
        }
        let Some(axis) = xz_axis(sample.normal) else {
            continue;
        };
        let angle = axis.x.atan2(-axis.z);
        let normalized = (angle + std::f32::consts::PI) / std::f32::consts::TAU;
        let bin = (normalized * bin_count as f32).floor() as isize;
        histogram[bin.rem_euclid(bin_count as isize) as usize] += sample.weight.max(0.01);
    }
    let total = histogram.iter().copied().sum::<f32>();
    if total > 0.0 {
        for value in &mut histogram {
            *value = (*value / total * 100.0).round() / 100.0;
        }
    }
    histogram
}

fn align_safe_normalize(v: Vec3f) -> Option<Vec3f> {
    let len = v.length();
    (len > 0.0001).then_some(v * (1.0 / len))
}

fn yaw_rotation(yaw: f32) -> Quat {
    Quat::from_axis_angle(vec3(0.0, 1.0, 0.0), yaw)
}

fn rotate_y(yaw: f32, vector: Vec3f) -> Vec3f {
    yaw_rotation(yaw).rotate_vec3(&vector)
}

fn xz_axis(vector: Vec3f) -> Option<Vec3f> {
    align_safe_normalize(vec3(vector.x, 0.0, vector.z))
}

fn signed_xz_angle(from: Vec3f, to: Vec3f) -> f32 {
    let cross = from.z * to.x - from.x * to.z;
    let dot = from.x * to.x + from.z * to.z;
    cross.atan2(dot)
}

fn wrap_angle(mut angle: f32) -> f32 {
    while angle <= -std::f32::consts::PI {
        angle += std::f32::consts::TAU;
    }
    while angle > std::f32::consts::PI {
        angle -= std::f32::consts::TAU;
    }
    angle
}

#[derive(Clone, Copy)]
#[cfg(test)]
struct VerticalDescriptorCell {
    vertical_mask: u8,
    clutter_mask: u8,
    free_mask: u8,
    height_u8: u8,
}

#[cfg(test)]
fn vertical_descriptor_cell_xy(
    descriptor: &XrDepthAlignVerticalDescriptor,
    x: isize,
    z: isize,
) -> Option<VerticalDescriptorCell> {
    let size = descriptor.size as usize;
    if size == 0
        || descriptor.vertical_surface_masks.len() != size * size
        || descriptor.clutter_surface_masks.len() != size * size
        || descriptor.free_space_masks.len() != size * size
        || descriptor.height_u8.len() != size * size
    {
        return None;
    }
    if x < 0 || z < 0 || x >= size as isize || z >= size as isize {
        return None;
    }
    let index = x as usize + z as usize * size;
    Some(VerticalDescriptorCell {
        vertical_mask: descriptor.vertical_surface_masks[index],
        clutter_mask: descriptor.clutter_surface_masks[index],
        free_mask: descriptor.free_space_masks[index],
        height_u8: descriptor.height_u8[index],
    })
}

#[cfg(test)]
fn vertical_descriptor_cell_center(
    descriptor: &XrDepthAlignVerticalDescriptor,
    index: usize,
) -> Vec3f {
    let size = descriptor.size as usize;
    let x = index % size;
    let z = index / size;
    vec3(
        descriptor.origin_x + (x as f32 + 0.5) * descriptor.cell_size_meters,
        0.0,
        descriptor.origin_z + (z as f32 + 0.5) * descriptor.cell_size_meters,
    )
}

#[cfg(test)]
fn vertical_descriptor_match_score(
    source: VerticalDescriptorCell,
    target: VerticalDescriptorCell,
) -> f32 {
    let vertical_support = (source.vertical_mask & target.vertical_mask).count_ones() as f32 * 1.75;
    let clutter_support = (source.clutter_mask & target.clutter_mask).count_ones() as f32 * 0.78;
    let free_support = (source.free_mask & target.free_mask).count_ones() as f32 * 0.08;
    let occupied_free_conflict = ((source.vertical_mask | source.clutter_mask) & target.free_mask)
        .count_ones() as f32
        * 0.18;
    let free_occupied_conflict = (source.free_mask & (target.vertical_mask | target.clutter_mask))
        .count_ones() as f32
        * 0.10;
    vertical_support + clutter_support + free_support
        - occupied_free_conflict
        - free_occupied_conflict
}

#[cfg(test)]
fn best_vertical_descriptor_cell(
    descriptor: &XrDepthAlignVerticalDescriptor,
    point: Vec3f,
    source: VerticalDescriptorCell,
) -> Option<VerticalDescriptorCell> {
    let base_x = ((point.x - descriptor.origin_x) / descriptor.cell_size_meters).floor() as isize;
    let base_z = ((point.z - descriptor.origin_z) / descriptor.cell_size_meters).floor() as isize;
    let mut best = None;
    let mut best_score = f32::NEG_INFINITY;
    for dz in -1..=1 {
        for dx in -1..=1 {
            let Some(target) = vertical_descriptor_cell_xy(descriptor, base_x + dx, base_z + dz)
            else {
                continue;
            };
            let score = vertical_descriptor_match_score(source, target);
            if score > best_score {
                best_score = score;
                best = Some(target);
            }
        }
    }
    best
}

#[cfg(test)]
fn score_vertical_descriptor_direction(
    source: &XrDepthAlignVerticalDescriptor,
    target: &XrDepthAlignVerticalDescriptor,
    yaw: f32,
    translation: Vec3f,
) -> Option<(f32, f32)> {
    if source.is_empty() || target.is_empty() {
        return None;
    }
    let mut support_score = 0.0;
    let mut support_max = 0.0;
    let mut conflict_score = 0.0;
    let mut height_error = 0.0;
    let mut height_weight = 0.0;
    let mut active_cells = 0usize;
    let mut overlapped_cells = 0usize;
    let cell_count = source.cell_count();
    for index in 0..cell_count {
        let source_cell = VerticalDescriptorCell {
            vertical_mask: source.vertical_surface_masks[index],
            clutter_mask: source.clutter_surface_masks[index],
            free_mask: source.free_space_masks[index],
            height_u8: source.height_u8[index],
        };
        if source_cell.vertical_mask == 0
            && source_cell.clutter_mask == 0
            && source_cell.free_mask == 0
            && source_cell.height_u8 == 0
        {
            continue;
        }
        active_cells += 1;
        let mapped = rotate_y(yaw, vertical_descriptor_cell_center(source, index)) + translation;
        let Some(target_cell) = best_vertical_descriptor_cell(target, mapped, source_cell) else {
            continue;
        };
        overlapped_cells += 1;
        let source_vertical = source_cell.vertical_mask.count_ones() as f32;
        let source_clutter = source_cell.clutter_mask.count_ones() as f32;
        let source_free = source_cell.free_mask.count_ones() as f32;
        support_score += vertical_descriptor_match_score(source_cell, target_cell).max(0.0);
        support_max += source_vertical * 1.75 + source_clutter * 0.78 + source_free * 0.08;
        conflict_score += ((source_cell.vertical_mask | source_cell.clutter_mask)
            & target_cell.free_mask)
            .count_ones() as f32
            * 0.06;
        conflict_score += (source_cell.free_mask
            & (target_cell.vertical_mask | target_cell.clutter_mask))
            .count_ones() as f32
            * 0.03;
        if source_cell.height_u8 > 0 && target_cell.height_u8 > 0 {
            height_error +=
                (source_cell.height_u8 as f32 - target_cell.height_u8 as f32).abs() / 255.0;
            height_weight += 1.0;
        }
    }
    let overlap_ratio = overlapped_cells as f32 / active_cells.max(1) as f32;
    if overlap_ratio < XR_DEPTH_ALIGN_VERTICAL_DESCRIPTOR_MIN_OVERLAP || support_max <= 0.0 {
        return None;
    }
    let score = ((support_score - conflict_score).max(0.0) / support_max).clamp(0.0, 1.0);
    let height_penalty = if height_weight > 0.0 {
        1.0 - (height_error / height_weight).clamp(0.0, 1.0)
    } else {
        0.75
    };
    let support =
        (score * (0.65 + 0.35 * overlap_ratio) * (0.70 + 0.30 * height_penalty)).clamp(0.0, 1.0);
    let residual = ((1.0 - support).max(0.0) * 0.30).clamp(0.0, 0.30);
    Some((support, residual))
}

#[cfg(test)]
fn score_vertical_descriptor_alignment(
    local: &XrDepthAlignVerticalDescriptor,
    remote: &XrDepthAlignVerticalDescriptor,
    yaw: f32,
    translation: Vec3f,
) -> (f32, f32) {
    let forward = score_vertical_descriptor_direction(remote, local, yaw, translation);
    let inverse_translation = rotate_y(-yaw, translation.scale(-1.0));
    let backward = score_vertical_descriptor_direction(local, remote, -yaw, inverse_translation);
    match (forward, backward) {
        (
            Some((forward_support, forward_residual)),
            Some((backward_support, backward_residual)),
        ) => (
            (forward_support + backward_support) * 0.5,
            (forward_residual + backward_residual) * 0.5,
        ),
        (Some(result), None) | (None, Some(result)) => result,
        (None, None) => (0.0, f32::INFINITY),
    }
}

fn candidate_yaws(
    local_histogram: &[f32],
    remote_histogram: &[f32],
    local_walls: &[&XrDepthAlignSample],
    remote_walls: &[&XrDepthAlignSample],
) -> Vec<f32> {
    let mut candidates = vec![0.0];
    if local_histogram.len() == remote_histogram.len() && !local_histogram.is_empty() {
        let bins = local_histogram.len();
        let mut shifts = Vec::<(f32, usize)>::new();
        for shift in 0..bins {
            let score = (0..bins)
                .map(|index| {
                    local_histogram[index] * remote_histogram[(index + bins - shift) % bins]
                })
                .sum::<f32>();
            shifts.push((score, shift));
        }
        shifts.sort_by(|a, b| b.0.total_cmp(&a.0));
        for (_, shift) in shifts.into_iter().take(6) {
            candidates.push(wrap_angle(
                shift as f32 * std::f32::consts::TAU / bins as f32,
            ));
        }
    }

    for local_sample in local_walls.iter().take(12) {
        let Some(local_axis) = xz_axis(local_sample.normal) else {
            continue;
        };
        for remote_sample in remote_walls.iter().take(12) {
            let Some(remote_axis) = xz_axis(remote_sample.normal) else {
                continue;
            };
            candidates.push(wrap_angle(signed_xz_angle(remote_axis, local_axis)));
        }
    }

    dedupe_angles(candidates, 0.06)
}

fn dedupe_angles(angles: Vec<f32>, epsilon: f32) -> Vec<f32> {
    let mut deduped = Vec::<f32>::new();
    for angle in angles {
        if deduped
            .iter()
            .any(|existing| wrap_angle(*existing - angle).abs() <= epsilon)
        {
            continue;
        }
        deduped.push(wrap_angle(angle));
    }
    deduped
}

fn candidate_translations(
    local_walls: &[&XrDepthAlignSample],
    remote_walls: &[&XrDepthAlignSample],
    floor_y: f32,
    yaw: f32,
) -> Vec<Vec3f> {
    let mut votes = HashMap::<(i32, i32), TranslationVote>::new();
    accumulate_translation_votes(&mut votes, local_walls, remote_walls, yaw, 0.88, 1.8);
    if votes.is_empty() {
        return vec![vec3(0.0, floor_y, 0.0)];
    }

    let mut ranked = votes.into_iter().collect::<Vec<_>>();
    ranked.sort_by(|a, b| {
        b.1.score
            .total_cmp(&a.1.score)
            .then_with(|| b.1.count.cmp(&a.1.count))
    });
    let translations = ranked
        .into_iter()
        .take(10)
        .filter_map(|(_, vote)| {
            (vote.weight_sum > 0.0).then_some(vec3(
                vote.sum_x / vote.weight_sum,
                floor_y,
                vote.sum_z / vote.weight_sum,
            ))
        })
        .collect::<Vec<_>>();
    dedupe_translations(translations, 0.08)
}

fn dedupe_translations(translations: Vec<Vec3f>, step: f32) -> Vec<Vec3f> {
    let mut deduped = Vec::<Vec3f>::new();
    for translation in translations {
        if deduped.iter().any(|existing| {
            let delta = *existing - translation;
            delta.x.abs() <= step && delta.y.abs() <= step && delta.z.abs() <= step
        }) {
            continue;
        }
        deduped.push(translation);
    }
    deduped
}

#[derive(Clone, Copy)]
struct WallSampleMatch<'a> {
    local: &'a XrDepthAlignSample,
    remote: &'a XrDepthAlignSample,
    distance: f32,
    alignment: f32,
    score: f32,
}

fn collect_unique_wall_matches<'a>(
    local_walls: &[&'a XrDepthAlignSample],
    remote_walls: &[&'a XrDepthAlignSample],
    yaw: f32,
    translation: Vec3f,
) -> Vec<WallSampleMatch<'a>> {
    #[derive(Clone, Copy)]
    struct Candidate<'a> {
        local_index: usize,
        remote_index: usize,
        local: &'a XrDepthAlignSample,
        remote: &'a XrDepthAlignSample,
        distance: f32,
        alignment: f32,
        score: f32,
    }

    let mut candidates = Vec::<Candidate<'a>>::new();
    for (remote_index, remote_sample) in remote_walls.iter().enumerate() {
        let transformed_point = rotate_y(yaw, remote_sample.point) + translation;
        let transformed_normal = align_safe_normalize(rotate_y(yaw, remote_sample.normal))
            .unwrap_or(remote_sample.normal);
        for (local_index, local_sample) in local_walls.iter().enumerate() {
            let alignment = local_sample.normal.dot(transformed_normal);
            if alignment < match_normal_dot(XrDepthAlignSampleKind::Wall) {
                continue;
            }
            let distance = (local_sample.point - transformed_point).length();
            if distance > match_radius(XrDepthAlignSampleKind::Wall) {
                continue;
            }
            let score = (sample_alignment_weight(local_sample)
                * sample_alignment_weight(remote_sample))
            .sqrt()
                * alignment
                * (-distance / match_radius(XrDepthAlignSampleKind::Wall).max(0.05)).exp();
            candidates.push(Candidate {
                local_index,
                remote_index,
                local: local_sample,
                remote: remote_sample,
                distance,
                alignment,
                score,
            });
        }
    }

    candidates.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.distance.total_cmp(&b.distance))
    });

    let mut used_local = vec![false; local_walls.len()];
    let mut used_remote = vec![false; remote_walls.len()];
    let mut matches = Vec::<WallSampleMatch<'a>>::new();
    for candidate in candidates {
        if used_local[candidate.local_index] || used_remote[candidate.remote_index] {
            continue;
        }
        used_local[candidate.local_index] = true;
        used_remote[candidate.remote_index] = true;
        matches.push(WallSampleMatch {
            local: candidate.local,
            remote: candidate.remote,
            distance: candidate.distance,
            alignment: candidate.alignment,
            score: candidate.score,
        });
    }
    matches
}

fn refine_alignment(
    local_walls: &[&XrDepthAlignSample],
    remote_walls: &[&XrDepthAlignSample],
    floor_y: f32,
    yaw: f32,
    translation: Vec3f,
) -> (f32, Vec3f) {
    let mut refined_yaw = yaw;
    let mut refined_translation = translation;
    refined_translation.y = floor_y;

    for _ in 0..2 {
        let mut translation_sum = vec3(0.0, 0.0, 0.0);
        let mut translation_weight_sum = 0.0;
        let mut yaw_sin = 0.0;
        let mut yaw_cos = 0.0;
        let mut yaw_weight_sum = 0.0;

        let matches = collect_unique_wall_matches(
            local_walls,
            remote_walls,
            refined_yaw,
            refined_translation,
        );
        for matched in matches {
            let local_sample = matched.local;
            let remote_sample = matched.remote;
            let weight = (local_sample.weight * remote_sample.weight).sqrt() * matched.alignment;
            let candidate_translation =
                local_sample.point - rotate_y(refined_yaw, remote_sample.point);
            translation_sum += candidate_translation * weight;
            translation_weight_sum += weight;
            let Some(local_axis) = xz_axis(local_sample.normal) else {
                continue;
            };
            let Some(remote_axis) = xz_axis(remote_sample.normal) else {
                continue;
            };
            let candidate_yaw = wrap_angle(signed_xz_angle(remote_axis, local_axis));
            yaw_sin += candidate_yaw.sin() * weight;
            yaw_cos += candidate_yaw.cos() * weight;
            yaw_weight_sum += weight;
        }

        if yaw_weight_sum > 0.0 {
            refined_yaw = wrap_angle(yaw_sin.atan2(yaw_cos));
        }
        if translation_weight_sum > 0.0 {
            refined_translation = translation_sum * (1.0 / translation_weight_sum);
            refined_translation.y = floor_y;
        }
    }

    (refined_yaw, refined_translation)
}

fn score_alignment_solution(
    local_walls: &[&XrDepthAlignSample],
    remote_walls: &[&XrDepthAlignSample],
    yaw: f32,
    translation: Vec3f,
) -> XrDepthAlignSolution {
    let mut total_score = 0.0;
    let mut residual_sum = 0.0;
    let max_score = remote_walls
        .iter()
        .map(|sample| sample_alignment_weight(sample))
        .sum::<f32>()
        .max(0.01);
    let matches = collect_unique_wall_matches(local_walls, remote_walls, yaw, translation);
    for matched in &matches {
        total_score += matched.score;
        residual_sum += matched.distance;
    }
    let matched_samples = matches.len();
    let residual_meters = if matched_samples > 0 {
        residual_sum / matched_samples as f32
    } else {
        f32::INFINITY
    };
    let coverage = (matched_samples as f32 / remote_walls.len().max(1) as f32).clamp(0.0, 1.0);
    let residual_confidence = if residual_meters.is_finite() {
        (1.0 - (residual_meters / 0.42)).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let confidence = ((total_score / max_score).clamp(0.0, 1.0)
        * coverage.sqrt()
        * residual_confidence.max(0.2))
    .clamp(0.0, 1.0);
    XrDepthAlignSolution {
        yaw_radians: wrap_angle(yaw),
        translation,
        confidence,
        symmetry_confidence: 1.0,
        residual_meters,
        matched_samples,
    }
}

#[derive(Default)]
struct TranslationVote {
    score: f32,
    weight_sum: f32,
    sum_x: f32,
    sum_z: f32,
    count: usize,
}

fn accumulate_translation_votes(
    votes: &mut HashMap<(i32, i32), TranslationVote>,
    local_samples: &[&XrDepthAlignSample],
    remote_samples: &[&XrDepthAlignSample],
    yaw: f32,
    normal_dot_min: f32,
    class_bias: f32,
) {
    for local_sample in local_samples.iter().take(64) {
        for remote_sample in remote_samples.iter().take(64) {
            let rotated_normal = align_safe_normalize(rotate_y(yaw, remote_sample.normal))
                .unwrap_or(remote_sample.normal);
            let alignment = local_sample.normal.dot(rotated_normal);
            if alignment < normal_dot_min {
                continue;
            }
            let delta = local_sample.point - rotate_y(yaw, remote_sample.point);
            if delta.x.abs() > 8.0 || delta.z.abs() > 8.0 {
                continue;
            }
            let weight =
                class_bias * (local_sample.weight * remote_sample.weight).sqrt() * alignment;
            let key = (
                quantize_translation_axis(delta.x, XR_DEPTH_ALIGN_TRANSLATION_VOTE_STEP_METERS),
                quantize_translation_axis(delta.z, XR_DEPTH_ALIGN_TRANSLATION_VOTE_STEP_METERS),
            );
            let vote = votes.entry(key).or_default();
            vote.score += weight;
            vote.weight_sum += weight;
            vote.sum_x += delta.x * weight;
            vote.sum_z += delta.z * weight;
            vote.count += 1;
        }
    }
}

fn sample_alignment_weight(sample: &XrDepthAlignSample) -> f32 {
    let kind_bias = match sample.kind {
        XrDepthAlignSampleKind::Floor => 1.0,
        XrDepthAlignSampleKind::Wall => 1.7,
        XrDepthAlignSampleKind::Unknown => 0.0,
    };
    sample.weight.max(0.01) * kind_bias
}

fn match_radius(kind: XrDepthAlignSampleKind) -> f32 {
    match kind {
        XrDepthAlignSampleKind::Floor => 0.42,
        XrDepthAlignSampleKind::Wall => 0.32,
        XrDepthAlignSampleKind::Unknown => 0.25,
    }
}

fn match_normal_dot(kind: XrDepthAlignSampleKind) -> f32 {
    match kind {
        XrDepthAlignSampleKind::Floor => 0.92,
        XrDepthAlignSampleKind::Wall => 0.86,
        XrDepthAlignSampleKind::Unknown => 0.80,
    }
}

fn quantize_translation_axis(value: f32, step: f32) -> i32 {
    (value / step.max(f32::EPSILON)).round() as i32
}

fn alignment_solution_better(
    candidate: &XrDepthAlignSolution,
    current: &XrDepthAlignSolution,
) -> bool {
    candidate
        .ranking_confidence()
        .partial_cmp(&current.ranking_confidence())
        .unwrap_or(std::cmp::Ordering::Equal)
        .then_with(|| {
            candidate
                .symmetry_confidence
                .partial_cmp(&current.symmetry_confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .then_with(|| {
            candidate
                .confidence
                .partial_cmp(&current.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .then_with(|| candidate.matched_samples.cmp(&current.matched_samples))
        .then_with(|| {
            current
                .residual_meters
                .partial_cmp(&candidate.residual_meters)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .is_gt()
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum XrDepthPlaneKind {
    Floor,
    Table,
    Ceiling,
    Wall,
    #[default]
    Unknown,
}

#[allow(dead_code)]
#[derive(Clone, Debug, Default)]
pub(crate) struct XrDepthPlanePatch {
    pub generation: u64,
    pub kind: XrDepthPlaneKind,
    pub center: Vec3f,
    pub normal: Vec3f,
    pub tangent: Vec3f,
    pub bitangent: Vec3f,
    pub half_extent_tangent: f32,
    pub half_extent_bitangent: f32,
    pub area: f32,
    pub support_triangles: usize,
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug)]
pub(crate) struct XrDepthMeshQuery {
    pub key: u64,
    pub center: Vec3f,
    pub predicted_center: Vec3f,
    pub velocity: Vec3f,
    pub radius: f32,
    pub max_distance: f32,
    pub include_planar_patches: bool,
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug)]
pub(crate) struct XrDepthMeshQuerySurfaceHit {
    pub distance: f32,
    pub point: Vec3f,
    pub normal: Vec3f,
    pub from_planar_patch: bool,
    pub triangle: [Vec3f; 3],
    pub patch: [Vec3f; 4],
    pub chunk_key: ChunkKey,
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug)]
pub(crate) struct XrDepthMeshQuerySupportPlane {
    pub point: Vec3f,
    pub normal: Vec3f,
    pub tangent: Vec3f,
    pub bitangent: Vec3f,
    pub half_extent_tangent: f32,
    pub half_extent_bitangent: f32,
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub(crate) enum XrDepthMeshQueryColliderGeometry {
    HalfSpace(XrDepthMeshQuerySupportPlane),
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum XrDepthMeshQueryColliderRole {
    Support,
    Impact,
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub(crate) struct XrDepthMeshQueryCollider {
    pub fingerprint: u64,
    pub geometry: XrDepthMeshQueryColliderGeometry,
    pub role: XrDepthMeshQueryColliderRole,
    pub restitution: f32,
}

#[allow(dead_code)]
impl XrDepthMeshQueryCollider {
    pub(crate) fn vertex_count(&self) -> usize {
        0
    }

    pub(crate) fn triangle_count(&self) -> usize {
        0
    }
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub(crate) struct XrDepthMeshQueryResolvedSurface {
    pub surface: XrDepthMeshQuerySurfaceHit,
    pub collider: XrDepthMeshQueryCollider,
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub(crate) struct XrDepthMeshQueryHit {
    pub key: u64,
    pub version: u64,
    pub mesh_generation: u64,
    pub distance: f32,
    pub point: Vec3f,
    pub normal: Vec3f,
    pub from_planar_patch: bool,
    pub triangle: [Vec3f; 3],
    pub patch: [Vec3f; 4],
    pub chunk_key: ChunkKey,
    pub collider: XrDepthMeshQueryCollider,
    pub additional_hits: Vec<XrDepthMeshQueryResolvedSurface>,
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub(crate) enum XrDepthMeshQueryResult {
    Hit(XrDepthMeshQueryHit),
    Miss {
        key: u64,
        version: u64,
        mesh_generation: u64,
    },
}

#[allow(dead_code)]
impl XrDepthMeshQueryResult {
    pub(crate) fn key(&self) -> u64 {
        match self {
            Self::Hit(hit) => hit.key,
            Self::Miss { key, .. } => *key,
        }
    }

    pub(crate) fn version(&self) -> u64 {
        match self {
            Self::Hit(hit) => hit.version,
            Self::Miss { version, .. } => *version,
        }
    }
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub(crate) struct XrDepthMeshPendingQuery {
    pub query: XrDepthMeshQuery,
    pub version: u64,
}

#[derive(Clone)]
pub struct XrDepthMeshStore {
    state: Arc<RwLock<XrDepthMeshState>>,
    published_snapshot: Arc<RwLock<Option<Arc<TsdfPublishedSnapshot>>>>,
    reset_generation: Arc<AtomicU64>,
    surface_analysis_enabled: Arc<AtomicBool>,
    voxel_size_meters_bits: Arc<AtomicU32>,
}

impl Default for XrDepthMeshStore {
    fn default() -> Self {
        Self {
            state: Arc::new(RwLock::new(XrDepthMeshState::default())),
            published_snapshot: Arc::new(RwLock::new(None)),
            reset_generation: Arc::new(AtomicU64::new(0)),
            surface_analysis_enabled: Arc::new(AtomicBool::new(false)),
            voxel_size_meters_bits: Arc::new(AtomicU32::new(
                XR_DEPTH_MESH_DEFAULT_VOXEL_SIZE_METERS.to_bits(),
            )),
        }
    }
}

#[allow(dead_code)]
impl XrDepthMeshStore {
    pub fn set_voxel_size_meters(&self, voxel_size_meters: f32) -> f32 {
        let voxel_size_meters = voxel_size_meters.clamp(0.03, 0.10);
        let previous = self
            .voxel_size_meters_bits
            .swap(voxel_size_meters.to_bits(), Ordering::AcqRel);
        if previous != voxel_size_meters.to_bits() {
            self.clear();
        }
        voxel_size_meters
    }

    pub fn voxel_size_meters(&self) -> f32 {
        f32::from_bits(self.voxel_size_meters_bits.load(Ordering::Acquire))
    }

    pub fn reset_generation(&self) -> u64 {
        self.reset_generation.load(Ordering::Acquire)
    }

    pub fn request_reset(&self) -> u64 {
        let generation = self.reset_generation.fetch_add(1, Ordering::AcqRel) + 1;
        self.clear();
        generation
    }

    pub fn set_surface_analysis_enabled(&self, enabled: bool) {
        self.surface_analysis_enabled.store(enabled, Ordering::Release);
    }

    pub fn surface_analysis_enabled(&self) -> bool {
        self.surface_analysis_enabled.load(Ordering::Acquire)
    }

    pub fn state(&self) -> Arc<RwLock<XrDepthMeshState>> {
        self.state.clone()
    }

    pub fn latest_tsdf_snapshot(&self) -> Option<Arc<TsdfPublishedSnapshot>> {
        self.published_snapshot
            .read()
            .ok()
            .and_then(|snapshot| snapshot.clone())
    }

    #[allow(dead_code)]
    pub(crate) fn record_seen(&self) {
        if let Ok(mut state) = self.state.write() {
            state.stats.frames_seen += 1;
        }
    }

    #[allow(dead_code)]
    pub(crate) fn record_drop(&self) {
        if let Ok(mut state) = self.state.write() {
            state.stats.frames_dropped += 1;
        }
    }

    #[allow(dead_code)]
    pub(crate) fn set_error(&self, error: String) {
        if let Ok(mut state) = self.state.write() {
            state.last_error = Some(error);
        }
    }

    #[allow(dead_code)]
    pub(crate) fn publish_tsdf_snapshot(&self, snapshot: TsdfPublishedSnapshot) {
        if let Ok(mut published_snapshot) = self.published_snapshot.write() {
            *published_snapshot = Some(Arc::new(snapshot));
        }
        if let Ok(mut state) = self.state.write() {
            state.stats.frames_meshed += 1;
            state.last_error = None;
        }
    }

    #[allow(dead_code)]
    pub(crate) fn clear(&self) {
        if let Ok(mut state) = self.state.write() {
            state.last_error = None;
            state.stats = XrDepthMeshStats::default();
        }
        if let Ok(mut published_snapshot) = self.published_snapshot.write() {
            *published_snapshot = None;
        }
    }
}

pub fn xr_depth_mesh_store() -> XrDepthMeshStore {
    static STORE: OnceLock<XrDepthMeshStore> = OnceLock::new();
    STORE.get_or_init(XrDepthMeshStore::default).clone()
}

#[allow(dead_code)]
pub(crate) fn empty_bounds() -> (Vec3f, Vec3f) {
    (vec3(0.0, 0.0, 0.0), vec3(0.0, 0.0, 0.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_vertical_descriptor(
        cells: &[(usize, usize, u8, u8, u8, u8)],
    ) -> XrDepthAlignVerticalDescriptor {
        let size = 16usize;
        let mut descriptor = XrDepthAlignVerticalDescriptor {
            origin_x: -1.6,
            origin_z: -1.6,
            cell_size_meters: 0.2,
            size: size as u16,
            vertical_surface_masks: vec![0; size * size],
            clutter_surface_masks: vec![0; size * size],
            free_space_masks: vec![0; size * size],
            height_u8: vec![0; size * size],
        };
        for &(x, z, vertical, clutter, free, height) in cells {
            let index = x + z * size;
            descriptor.vertical_surface_masks[index] = vertical;
            descriptor.clutter_surface_masks[index] = clutter;
            descriptor.free_space_masks[index] = free;
            descriptor.height_u8[index] = height;
        }
        descriptor
    }

    fn make_vertical_only_descriptor() -> XrDepthAlignDescriptor {
        XrDepthAlignDescriptor {
            voxel_size_meters: 0.05,
            floor_y: 0.0,
            wall_normal_histogram: Vec::new(),
            samples: Vec::new(),
            vertical_descriptor: Some(make_vertical_descriptor(&[
                (2, 2, 0b0011_1100, 0, 0b0000_0011, 164),
                (3, 2, 0b0011_1110, 0, 0b0000_0111, 172),
                (4, 2, 0b0011_1000, 0, 0b0000_0111, 168),
                (3, 3, 0b0111_1000, 0, 0b0000_0011, 188),
                (4, 3, 0b0011_1000, 0b0000_0010, 0b0000_0001, 176),
                (10, 5, 0, 0b0000_1110, 0b1110_0000, 98),
                (11, 5, 0, 0b0001_1110, 0b1110_0000, 108),
                (10, 6, 0, 0b0001_1110, 0b1111_0000, 114),
                (11, 6, 0, 0b0000_1110, 0b1111_0000, 106),
                (9, 10, 0b0111_0000, 0, 0b0000_0011, 208),
                (10, 10, 0b0111_1000, 0, 0b0000_0011, 216),
                (10, 11, 0b0011_0000, 0b0000_0011, 0b0000_0111, 154),
                (6, 11, 0, 0b0001_1110, 0b1111_0000, 120),
                (6, 12, 0, 0b0001_1110, 0b1111_1000, 124),
                (7, 12, 0b0001_1000, 0b0000_0110, 0b1110_0000, 138),
            ])),
            height_map: None,
        }
    }

    fn angle_error(a: f32, b: f32) -> f32 {
        wrap_angle(a - b).abs()
    }

    fn encode_test_height(height: f32, bottom_y: f32, top_y: f32) -> u16 {
        let span = (top_y - bottom_y).max(1.0e-3);
        let normalized = ((height - bottom_y) / span).clamp(0.0, 1.0);
        1 + (normalized * 65534.0).round() as u16
    }

    fn synthetic_scene_height(point: Vec2f) -> f32 {
        let mut height: f32 = 0.02;
        if point.x.abs() >= 2.05 && point.x.abs() <= 2.25 && point.y >= -2.30 && point.y <= 2.10 {
            height = height.max(2.15);
        }
        if point.y >= 1.75 && point.y <= 1.95 && point.x >= -2.25 && point.x <= 2.25 {
            height = height.max(2.15);
        }
        if point.y <= -1.95 && point.y >= -2.20 && point.x >= -2.25 && point.x <= 2.05 {
            height = height.max(2.15);
        }
        if point.x >= -0.95 && point.x <= -0.10 && point.y >= -0.42 && point.y <= 0.36 {
            height = height.max(0.84);
        }
        if point.x >= 0.52 && point.x <= 1.12 && point.y >= 0.52 && point.y <= 1.18 {
            height = height.max(1.38);
        }
        if point.x >= -1.58 && point.x <= -1.20 && point.y >= 0.92 && point.y <= 1.34 {
            height = height.max(1.62);
        }
        if point.x >= 0.96 && point.x <= 1.34 && point.y >= -1.42 && point.y <= -0.66 {
            height = height.max(0.68);
        }
        let wobble = ((point.x * 1.13 + point.y * 0.73).sin() * 0.018)
            + ((point.x * 0.41 - point.y * 1.51).cos() * 0.014);
        (height + wobble).clamp(0.0, 2.25)
    }

    #[derive(Clone, Copy, Debug, Default)]
    struct HeightMapTestArtifacts {
        cutout_center: Option<Vec2f>,
        occlusion_center: Option<Vec2f>,
        extra_blob_center: Option<Vec2f>,
        noise_seed: f32,
        noise_scale: f32,
        height_bias: f32,
    }

    fn deterministic_height_noise(point: Vec2f, seed: f32) -> f32 {
        ((point.x * 2.73 + point.y * 1.91 + seed * 0.37).sin() * 0.021)
            + ((point.x * 0.84 - point.y * 3.14 + seed * 0.61).cos() * 0.017)
    }

    fn make_height_map_descriptor_with_artifacts(
        map_to_scene: Mat4f,
        artifacts: HeightMapTestArtifacts,
    ) -> XrDepthAlignDescriptor {
        let size_x = 120usize;
        let size_z = 114usize;
        let cell_size_meters = 0.05;
        let extent_x = size_x as f32 * cell_size_meters;
        let extent_z = size_z as f32 * cell_size_meters;
        let origin_x = -extent_x * 0.5;
        let origin_z = -extent_z * 0.5;
        let bottom_y_meters = 0.0;
        let top_y_meters = 2.3;
        let mut height_u16 = vec![0u16; size_x * size_z];
        for z in 0..size_z {
            for x in 0..size_x {
                let map_point = vec2f(
                    origin_x + (x as f32 + 0.5) * cell_size_meters,
                    origin_z + (z as f32 + 0.5) * cell_size_meters,
                );
                if artifacts
                    .cutout_center
                    .is_some_and(|center| (map_point - center).length() <= 0.36)
                {
                    continue;
                }
                if artifacts.occlusion_center.is_some_and(|center| {
                    (map_point.x - center.x).abs() <= 0.52 && (map_point.y - center.y).abs() <= 0.44
                }) {
                    continue;
                }
                let scene_point = map_to_scene
                    .transform_vec4(vec4f(map_point.x, 0.0, map_point.y, 1.0))
                    .to_vec3f();
                let mut height = synthetic_scene_height(vec2f(scene_point.x, scene_point.z));
                if artifacts
                    .extra_blob_center
                    .is_some_and(|center| (map_point - center).length() <= 0.30)
                {
                    height = height.max(1.68);
                }
                height += deterministic_height_noise(map_point, artifacts.noise_seed)
                    * artifacts.noise_scale;
                height += artifacts.height_bias;
                height = height.clamp(0.0, 2.25);
                height_u16[x + z * size_x] =
                    encode_test_height(height, bottom_y_meters, top_y_meters);
            }
        }
        XrDepthAlignDescriptor {
            voxel_size_meters: 0.05,
            floor_y: 0.0,
            wall_normal_histogram: Vec::new(),
            samples: Vec::new(),
            vertical_descriptor: None,
            height_map: Some(XrDepthAlignHeightMap {
                origin_x,
                origin_z,
                cell_size_meters,
                size_x: size_x as u16,
                size_z: size_z as u16,
                bottom_y_meters,
                top_y_meters,
                player_cutout_center: artifacts.cutout_center,
                player_cutout_radius_meters: 0.36,
                height_u16,
            }),
        }
    }

    fn assert_height_map_case(
        expected_yaw: f32,
        expected_translation: Vec3f,
        mut local_artifacts: HeightMapTestArtifacts,
        remote_artifacts: HeightMapTestArtifacts,
    ) {
        let remote_to_local = Pose::new(
            Quat::from_axis_angle(vec3(0.0, 1.0, 0.0), expected_yaw),
            expected_translation,
        )
        .to_mat4();
        if let Some(remote_cutout_center) = remote_artifacts.cutout_center {
            let mapped_remote_center = rotate_y(
                expected_yaw,
                vec3(remote_cutout_center.x, 0.0, remote_cutout_center.y),
            ) + expected_translation;
            local_artifacts.extra_blob_center =
                Some(vec2f(mapped_remote_center.x, mapped_remote_center.z));
        }
        let local = make_height_map_descriptor_with_artifacts(Mat4f::identity(), local_artifacts);
        let remote = make_height_map_descriptor_with_artifacts(remote_to_local, remote_artifacts);

        let mut first_solution = None::<XrDepthAlignSolution>;
        for _ in 0..3 {
            let diagnostic = xr_depth_align_analyze_remote_to_local(&local, &remote);
            let solution = diagnostic.accepted_solution().unwrap_or_else(|| {
                panic!("height map solver should recover the pose: {diagnostic:?}")
            });
            assert!(
                angle_error(solution.yaw_radians, expected_yaw) < 0.14,
                "{solution:?}"
            );
            assert!(
                (solution.translation - expected_translation).length() < 0.22,
                "{solution:?}"
            );
            assert!(solution.confidence > 0.14, "{solution:?}");
            assert!(solution.matched_samples >= 6, "{solution:?}");
            if let Some(previous) = first_solution {
                assert!(
                    angle_error(solution.yaw_radians, previous.yaw_radians) < 1.0e-4,
                    "{previous:?} {solution:?}"
                );
                assert!(
                    (solution.translation - previous.translation).length() < 1.0e-4,
                    "{previous:?} {solution:?}"
                );
            } else {
                first_solution = Some(solution);
            }
        }
    }

    #[test]
    fn vertical_descriptor_scores_true_pose_above_shifted_pose() {
        let local = make_vertical_only_descriptor();
        let expected_yaw = 0.37;
        let expected_translation = vec3(-0.52, 0.0, 0.41);
        let remote_to_local = Pose::new(
            Quat::from_axis_angle(vec3(0.0, 1.0, 0.0), expected_yaw),
            expected_translation,
        )
        .to_mat4();
        let local_to_remote = remote_to_local.invert();
        let remote = xr_depth_align_transform_descriptor(&local, &local_to_remote);

        let (correct_support, correct_residual) = score_vertical_descriptor_alignment(
            local.vertical_descriptor.as_ref().unwrap(),
            remote.vertical_descriptor.as_ref().unwrap(),
            expected_yaw,
            expected_translation,
        );
        let shifted_translation = expected_translation + vec3(0.55, 0.0, -0.35);
        let (shifted_support, shifted_residual) = score_vertical_descriptor_alignment(
            local.vertical_descriptor.as_ref().unwrap(),
            remote.vertical_descriptor.as_ref().unwrap(),
            expected_yaw,
            shifted_translation,
        );

        assert!(
            correct_support > 0.50,
            "{correct_support} {correct_residual}"
        );
        assert!(correct_residual.is_finite(), "{correct_residual}");
        assert!(
            correct_support > shifted_support + 0.18,
            "correct_support={correct_support} shifted_support={shifted_support} correct_residual={correct_residual} shifted_residual={shifted_residual}"
        );
    }

    #[test]
    fn vertical_descriptor_scores_true_pose_above_flipped_pose() {
        let local = make_vertical_only_descriptor();
        let expected_yaw = -0.29;
        let expected_translation = vec3(0.44, 0.0, -0.57);
        let remote_to_local = Pose::new(
            Quat::from_axis_angle(vec3(0.0, 1.0, 0.0), expected_yaw),
            expected_translation,
        )
        .to_mat4();
        let local_to_remote = remote_to_local.invert();
        let remote = xr_depth_align_transform_descriptor(&local, &local_to_remote);

        let (correct_support, correct_residual) = score_vertical_descriptor_alignment(
            local.vertical_descriptor.as_ref().unwrap(),
            remote.vertical_descriptor.as_ref().unwrap(),
            expected_yaw,
            expected_translation,
        );
        let flipped_yaw = wrap_angle(expected_yaw + std::f32::consts::PI);
        let flipped_translation = vec3(-expected_translation.x, 0.0, -expected_translation.z);
        let (flipped_support, flipped_residual) = score_vertical_descriptor_alignment(
            local.vertical_descriptor.as_ref().unwrap(),
            remote.vertical_descriptor.as_ref().unwrap(),
            flipped_yaw,
            flipped_translation,
        );

        assert!(
            correct_support > 0.50,
            "{correct_support} {correct_residual}"
        );
        assert!(
            correct_support > flipped_support + 0.18,
            "correct_support={correct_support} flipped_support={flipped_support} correct_residual={correct_residual} flipped_residual={flipped_residual}"
        );
    }

    #[test]
    fn vertical_descriptor_reports_zero_support_when_overlap_is_missing() {
        let local = make_vertical_only_descriptor();
        let expected_yaw = 0.24;
        let expected_translation = vec3(-0.38, 0.0, 0.46);
        let remote_to_local = Pose::new(
            Quat::from_axis_angle(vec3(0.0, 1.0, 0.0), expected_yaw),
            expected_translation,
        )
        .to_mat4();
        let local_to_remote = remote_to_local.invert();
        let remote = xr_depth_align_transform_descriptor(&local, &local_to_remote);

        let (support, residual) = score_vertical_descriptor_alignment(
            local.vertical_descriptor.as_ref().unwrap(),
            remote.vertical_descriptor.as_ref().unwrap(),
            wrap_angle(expected_yaw + std::f32::consts::PI),
            expected_translation + vec3(4.0, 0.0, -4.0),
        );

        assert!(support <= 0.001, "{support} {residual}");
        assert!(!residual.is_finite(), "{support} {residual}");
    }

    #[test]
    fn vertical_descriptor_tolerates_extra_unmatched_clutter() {
        let local = make_vertical_only_descriptor();
        let expected_yaw = -0.33;
        let expected_translation = vec3(0.42, 0.0, -0.36);
        let remote_to_local = Pose::new(
            Quat::from_axis_angle(vec3(0.0, 1.0, 0.0), expected_yaw),
            expected_translation,
        )
        .to_mat4();
        let local_to_remote = remote_to_local.invert();
        let mut remote = xr_depth_align_transform_descriptor(&local, &local_to_remote);

        let descriptor = remote.vertical_descriptor.as_mut().unwrap();
        let size = descriptor.size as usize;
        let blob_cells = [(14usize, 4usize), (15, 4), (14, 5), (15, 5)];
        for (x, z) in blob_cells {
            let index = x + z * size;
            descriptor.clutter_surface_masks[index] |= 0b0001_1110;
            descriptor.height_u8[index] = descriptor.height_u8[index].max(132);
        }

        let (correct_support, correct_residual) = score_vertical_descriptor_alignment(
            local.vertical_descriptor.as_ref().unwrap(),
            remote.vertical_descriptor.as_ref().unwrap(),
            expected_yaw,
            expected_translation,
        );
        let flipped_yaw = wrap_angle(expected_yaw + std::f32::consts::PI);
        let flipped_translation = vec3(-expected_translation.x, 0.0, -expected_translation.z);
        let (flipped_support, flipped_residual) = score_vertical_descriptor_alignment(
            local.vertical_descriptor.as_ref().unwrap(),
            remote.vertical_descriptor.as_ref().unwrap(),
            flipped_yaw,
            flipped_translation,
        );

        assert!(
            correct_support > 0.22,
            "{correct_support} {correct_residual}"
        );
        assert!(
            correct_support > flipped_support + 0.10,
            "correct_support={correct_support} flipped_support={flipped_support} correct_residual={correct_residual} flipped_residual={flipped_residual}"
        );
    }

    #[test]
    fn rescoring_old_pose_against_resumed_descriptor_marks_it_stale() {
        let local = make_height_map_descriptor_with_artifacts(
            Mat4f::identity(),
            HeightMapTestArtifacts {
                cutout_center: Some(vec2f(-0.12, 0.16)),
                occlusion_center: Some(vec2f(0.96, -0.74)),
                noise_seed: 1.6,
                noise_scale: 0.55,
                ..HeightMapTestArtifacts::default()
            },
        );
        let first_remote_to_local = Pose::new(
            Quat::from_axis_angle(vec3(0.0, 1.0, 0.0), -0.41),
            vec3(0.58, 0.0, -0.44),
        )
        .to_mat4();
        let first_remote =
            xr_depth_align_transform_descriptor(&local, &first_remote_to_local.invert());
        let first_diagnostic = xr_depth_align_analyze_remote_to_local(&local, &first_remote);
        let previous_solution = first_diagnostic
            .accepted_solution()
            .expect("expected initial box-room alignment");

        let resumed_remote_to_local = Pose::new(
            Quat::from_axis_angle(vec3(0.0, 1.0, 0.0), 1.18),
            vec3(-0.62, 0.0, 0.71),
        )
        .to_mat4();
        let resumed_remote =
            xr_depth_align_transform_descriptor(&local, &resumed_remote_to_local.invert());
        let resumed_diagnostic = xr_depth_align_analyze_remote_to_local(&local, &resumed_remote);
        let resumed_solution = resumed_diagnostic
            .accepted_solution()
            .expect("expected resumed height-map alignment");
        let stale_solution =
            xr_depth_align_rescore_remote_to_local(&local, &resumed_remote, previous_solution);

        assert!(
            xr_depth_align_solution_is_accepted(&resumed_diagnostic, resumed_solution),
            "{resumed_diagnostic:?} {resumed_solution:?}"
        );
        assert!(
            resumed_solution.ranking_confidence() > stale_solution.ranking_confidence() + 0.25,
            "resumed descriptor should strongly outrank the stale pose: stale={stale_solution:?} resumed={resumed_solution:?}"
        );
        assert!(
            resumed_solution.symmetry_confidence > stale_solution.symmetry_confidence + 0.25,
            "resumed heightmap symmetry should strongly favor the new pose: stale={stale_solution:?} resumed={resumed_solution:?}"
        );
    }

    #[test]
    fn seeded_height_map_solver_reuses_stable_lock_locally() {
        let local = make_height_map_descriptor_with_artifacts(
            Mat4f::identity(),
            HeightMapTestArtifacts {
                cutout_center: Some(vec2f(-0.10, 0.14)),
                occlusion_center: Some(vec2f(0.94, -0.82)),
                noise_seed: 1.2,
                noise_scale: 0.55,
                ..HeightMapTestArtifacts::default()
            },
        );
        let first_remote_to_local = Pose::new(
            Quat::from_axis_angle(vec3(0.0, 1.0, 0.0), 0.34),
            vec3(-0.52, 0.0, 0.46),
        )
        .to_mat4();
        let first_remote =
            xr_depth_align_transform_descriptor(&local, &first_remote_to_local.invert());
        let first_solution = xr_depth_align_analyze_remote_to_local(&local, &first_remote)
            .accepted_solution()
            .expect("expected initial alignment");

        let diagnostic = xr_depth_align_analyze_remote_to_local_seeded(
            &local,
            &first_remote,
            Some(first_solution),
        );
        let solution = diagnostic
            .accepted_solution()
            .expect("expected seeded solve to reuse the stable lock");

        assert!(diagnostic.yaw_candidate_count == 1, "{diagnostic:?}");
        assert!(diagnostic.pose_candidate_count == 1, "{diagnostic:?}");
        assert!(angle_error(solution.yaw_radians, 0.34) < 0.02, "{solution:?}");
        assert!(
            (solution.translation - vec3(-0.52, 0.0, 0.46)).length() < 0.03,
            "{solution:?}"
        );
    }

    #[test]
    fn seeded_height_map_solver_falls_back_when_seed_is_stale() {
        let local = make_height_map_descriptor_with_artifacts(
            Mat4f::identity(),
            HeightMapTestArtifacts {
                cutout_center: Some(vec2f(0.14, -0.12)),
                occlusion_center: Some(vec2f(-0.98, 0.86)),
                noise_seed: 2.1,
                noise_scale: 0.6,
                ..HeightMapTestArtifacts::default()
            },
        );
        let first_remote_to_local = Pose::new(
            Quat::from_axis_angle(vec3(0.0, 1.0, 0.0), -0.28),
            vec3(0.42, 0.0, -0.34),
        )
        .to_mat4();
        let first_remote =
            xr_depth_align_transform_descriptor(&local, &first_remote_to_local.invert());
        let stale_seed = xr_depth_align_analyze_remote_to_local(&local, &first_remote)
            .accepted_solution()
            .expect("expected initial alignment");

        let resumed_yaw = 1.02;
        let resumed_translation = vec3(-0.64, 0.0, 0.78);
        let resumed_remote_to_local = Pose::new(
            Quat::from_axis_angle(vec3(0.0, 1.0, 0.0), resumed_yaw),
            resumed_translation,
        )
        .to_mat4();
        let resumed_remote =
            xr_depth_align_transform_descriptor(&local, &resumed_remote_to_local.invert());
        let seeded = xr_depth_align_analyze_remote_to_local_seeded(
            &local,
            &resumed_remote,
            Some(stale_seed),
        );
        let seeded_solution = seeded
            .accepted_solution()
            .expect("expected seeded solve to fall back to global search");
        let global = xr_depth_align_analyze_remote_to_local(&local, &resumed_remote);
        let global_solution = global
            .accepted_solution()
            .expect("expected global solve to recover resumed pose");

        assert!(seeded.yaw_candidate_count > 1, "{seeded:?}");
        assert!(
            angle_error(seeded_solution.yaw_radians, global_solution.yaw_radians) < 0.05,
            "{seeded_solution:?} {global_solution:?}"
        );
        assert!(
            (seeded_solution.translation - global_solution.translation).length() < 0.08,
            "{seeded_solution:?} {global_solution:?}"
        );
    }

    #[test]
    fn height_map_solver_recovers_rotated_translated_pose() {
        assert_height_map_case(
            0.43,
            vec3(-0.58, 0.0, 0.46),
            HeightMapTestArtifacts {
                cutout_center: Some(vec2f(0.18, -0.12)),
                ..HeightMapTestArtifacts::default()
            },
            HeightMapTestArtifacts {
                cutout_center: Some(vec2f(-0.20, 0.14)),
                occlusion_center: Some(vec2f(0.92, -0.74)),
                noise_seed: 1.3,
                noise_scale: 0.7,
                height_bias: 0.015,
                ..HeightMapTestArtifacts::default()
            },
        );
    }

    #[test]
    fn height_map_solver_tolerates_cutouts_and_partial_overlap() {
        assert_height_map_case(
            -0.37,
            vec3(0.64, 0.0, -0.52),
            HeightMapTestArtifacts {
                cutout_center: Some(vec2f(-0.08, 0.10)),
                occlusion_center: Some(vec2f(-1.12, -0.96)),
                noise_seed: 2.0,
                noise_scale: 0.4,
                ..HeightMapTestArtifacts::default()
            },
            HeightMapTestArtifacts {
                cutout_center: Some(vec2f(0.22, -0.18)),
                occlusion_center: Some(vec2f(1.06, 0.84)),
                noise_seed: 4.4,
                noise_scale: 0.8,
                height_bias: -0.018,
                ..HeightMapTestArtifacts::default()
            },
        );
    }

    #[test]
    fn height_map_solver_handles_noise_rotation_shift_matrix() {
        let cases = [
            (
                0.18,
                vec3(0.28, 0.0, -0.34),
                HeightMapTestArtifacts {
                    cutout_center: Some(vec2f(-0.16, 0.24)),
                    noise_seed: 0.7,
                    noise_scale: 0.6,
                    ..HeightMapTestArtifacts::default()
                },
                HeightMapTestArtifacts {
                    cutout_center: Some(vec2f(0.14, -0.18)),
                    occlusion_center: Some(vec2f(-0.88, 0.92)),
                    noise_seed: 3.7,
                    noise_scale: 0.9,
                    height_bias: 0.012,
                    ..HeightMapTestArtifacts::default()
                },
            ),
            (
                -0.62,
                vec3(-0.74, 0.0, 0.52),
                HeightMapTestArtifacts {
                    cutout_center: Some(vec2f(0.22, -0.12)),
                    occlusion_center: Some(vec2f(-1.22, 0.74)),
                    noise_seed: 1.8,
                    noise_scale: 0.5,
                    ..HeightMapTestArtifacts::default()
                },
                HeightMapTestArtifacts {
                    cutout_center: Some(vec2f(-0.18, 0.20)),
                    noise_seed: 5.1,
                    noise_scale: 0.85,
                    height_bias: -0.015,
                    ..HeightMapTestArtifacts::default()
                },
            ),
            (
                0.91,
                vec3(0.86, 0.0, 0.18),
                HeightMapTestArtifacts {
                    cutout_center: Some(vec2f(-0.24, -0.10)),
                    noise_seed: 2.4,
                    noise_scale: 0.7,
                    ..HeightMapTestArtifacts::default()
                },
                HeightMapTestArtifacts {
                    cutout_center: Some(vec2f(0.20, 0.18)),
                    occlusion_center: Some(vec2f(0.92, -1.06)),
                    noise_seed: 6.2,
                    noise_scale: 1.0,
                    height_bias: 0.02,
                    ..HeightMapTestArtifacts::default()
                },
            ),
            (
                -1.08,
                vec3(-0.42, 0.0, -0.78),
                HeightMapTestArtifacts {
                    cutout_center: Some(vec2f(0.10, 0.22)),
                    occlusion_center: Some(vec2f(1.14, 0.82)),
                    noise_seed: 3.0,
                    noise_scale: 0.55,
                    ..HeightMapTestArtifacts::default()
                },
                HeightMapTestArtifacts {
                    cutout_center: Some(vec2f(-0.26, -0.14)),
                    noise_seed: 7.4,
                    noise_scale: 0.95,
                    height_bias: -0.022,
                    ..HeightMapTestArtifacts::default()
                },
            ),
        ];

        for (yaw, translation, local_artifacts, remote_artifacts) in cases {
            assert_height_map_case(yaw, translation, local_artifacts, remote_artifacts);
        }
    }
}
