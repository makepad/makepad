use crate::{
    makepad_math::{vec2f, vec3f, vec4f, Mat4f, Vec2f, Vec3f, Vec4f},
    os::linux::{
        openxr::CxOpenXrFrame,
        vulkan::{CxVulkan, CxVulkanOpenXrSessionData},
    },
    thread::SignalToUI,
    xr_depth_mesh::{
        empty_bounds, xr_depth_mesh_store, XrDepthMesh, XrDepthMeshChunk, XrDepthMeshQuery,
        XrDepthMeshQueryHit, XrDepthMeshQueryResult, XrDepthMeshStore, XrDepthPlaneKind,
        XrDepthPlanePatch,
    },
};
use parry3d::math::IVector;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    hash::{Hash, Hasher},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{channel, Receiver, RecvTimeoutError, Sender},
        Arc,
    },
    time::{Duration, Instant},
};

const DEPTH_VOXEL_EYE_INDEX: usize = 0;
const DEPTH_VOXEL_SAMPLE_STEP: u32 = 1;
const DEPTH_IMAGE_EDGE_MARGIN_PIXELS: usize = 32;
const DEPTH_VOXEL_SIZE_METERS: f32 = 0.10;
const DEPTH_VOXEL_MIN_DISTANCE_METERS: f32 = 0.08;
const DEPTH_VOXEL_MAX_DISTANCE_METERS: f32 = 6.0;
const DEPTH_TSD_MIN_UPDATE_DISTANCE_METERS: f32 = 0.5;
const DEPTH_TSD_UPDATE_IDLE_INTERVAL_MILLIS: u64 = 200;
const DEPTH_TSD_UPDATE_MOVING_INTERVAL_MILLIS: u64 = 33;
const DEPTH_TSD_UPDATE_TRANSLATION_TRIGGER_METERS: f32 = 0.04;
const DEPTH_TSD_UPDATE_ROTATION_TRIGGER_DOT: f32 = 0.999;
const DEPTH_VOXEL_MIN_DEPTH_VALUE: f32 = 1.0 / 65535.0;
const DEPTH_VOXEL_MAX_DEPTH_VALUE: f32 = 0.9995;
const DEPTH_TSD_DISTANCE_METERS: f32 = DEPTH_VOXEL_SIZE_METERS * 2.0;
const DEPTH_TSD_MIN_NORMAL_DOT: f32 = 0.3;
const DEPTH_TSD_APPLY_DELTA_EPSILON: f32 = 0.01;
const DEPTH_TSD_REFRESH_CLEARANCE_METERS: f32 = DEPTH_VOXEL_SIZE_METERS * 1.5;
const DEPTH_NORMAL_NEIGHBOR_MAX_DISTANCE_DELTA_METERS: f32 = DEPTH_VOXEL_SIZE_METERS * 2.5;
const DEPTH_CARVE_NEIGHBOR_MAX_DISTANCE_DELTA_METERS: f32 = DEPTH_VOXEL_SIZE_METERS * 1.5;
const DEPTH_TSD_MAX_CONFIDENCE: u8 = 32;
const DEPTH_TSD_MIN_MESH_CONFIDENCE: u8 = 3;
const DEPTH_TSD_RECENT_MESH_CONFIDENCE: u8 = 1;
const DEPTH_TSD_RECENT_MESH_GENERATIONS: u64 = 6;
const DEPTH_TSD_RECENT_MESH_MAX_ABS_DISTANCE: f32 = 0.6;
const DEPTH_TSD_STABLE_CONFIDENCE: u8 = 8;
const DEPTH_PLAYER_CLEAR_MAX_CONFIDENCE: u8 = 2;
const DEPTH_PLAYER_EXCLUDE_RADIUS_METERS: f32 = 0.32;
const DEPTH_PLAYER_EXCLUDE_TOP_METERS: f32 = 0.12;
const DEPTH_PLAYER_EXCLUDE_BOTTOM_METERS: f32 = 1.30;
const DEPTH_MESH_UPDATE_DISTANCE_METERS: f32 = 4.0;
const DEPTH_SURFACE_MESH_CHUNKS_PER_TICK: usize = 1;
const DEPTH_SURFACE_MESH_IDLE_WAIT_MILLIS: u64 = 8;
const DEPTH_QUERY_BATCH_PER_TICK: usize = 8;
const DEPTH_DEBUG_LOG_CHUNK_MESH_TIMING: bool = false;
const DEPTH_QUERY_PATCH_RADIUS_METERS: f32 = 0.24;
const DEPTH_QUERY_PATCH_PLANE_TOLERANCE_METERS: f32 = 0.035;
const DEPTH_QUERY_PATCH_NORMAL_DOT: f32 = 0.93;
const DEPTH_QUERY_PATCH_MARGIN_METERS: f32 = 0.025;
const DEPTH_QUERY_PATCH_MIN_HALF_EXTENT_METERS: f32 = 0.05;
const DEPTH_QUERY_MIN_OPPOSING_NORMAL_DOT: f32 = 0.2;
const DEPTH_PLANE_REBUILD_INTERVAL_MILLIS: u64 = 250;
const DEPTH_PLANE_HORIZONTAL_NORMAL_Y_MIN: f32 = 0.82;
const DEPTH_PLANE_VERTICAL_NORMAL_Y_MAX: f32 = 0.35;
const DEPTH_PLANE_DISTANCE_BIN_METERS: f32 = 0.08;
const DEPTH_PLANE_VERTEX_LINK_METERS: f32 = DEPTH_VOXEL_SIZE_METERS * 0.75;
const DEPTH_PLANE_MIN_AREA_METERS2: f32 = 0.35;
const DEPTH_PLANE_MIN_DIM_METERS: f32 = 0.30;
const DEPTH_PLANE_MAX_PATCHES: usize = 24;
const DEPTH_PLANE_WALL_YAW_BINS: i32 = 32;
const DEPTH_PLANE_REGION_VERTICAL_NORMAL_DOT: f32 = 0.94;
const DEPTH_PLANE_TRACK_MATCH_NORMAL_DOT: f32 = 0.95;
const DEPTH_PLANE_TRACK_MATCH_CENTER_DISTANCE_METERS: f32 = 0.45;
const DEPTH_PLANE_TRACK_MATCH_PLANE_DISTANCE_METERS: f32 = 0.20;
const DEPTH_PLANE_TRACK_MAX_MISSES: u32 = 10;
const DEPTH_PLANE_TRACK_STABLE_HORIZONTAL_MAX_MISSES: u32 = 30;
const DEPTH_PLANE_TRACK_STABLE_HITS: u32 = 2;
const DEPTH_PLANE_TRACK_WALL_MATCH_NORMAL_DOT: f32 = 0.93;
const DEPTH_PLANE_TRACK_WALL_MATCH_CENTER_DISTANCE_METERS: f32 = 0.65;
const DEPTH_PLANE_TRACK_WALL_MATCH_PLANE_DISTANCE_METERS: f32 = 0.25;
const DEPTH_PLANE_TRACK_HORIZONTAL_EXPAND_ALPHA: f32 = 0.30;
const DEPTH_PLANE_TRACK_HORIZONTAL_SHRINK_ALPHA: f32 = 0.08;
const DEPTH_PLANE_TRACK_HORIZONTAL_STABLE_SHRINK_ALPHA: f32 = 0.02;
const DEPTH_PLANE_TRACK_CENTER_ALPHA: f32 = 0.18;
const DEPTH_PLANE_TRACK_STABLE_CENTER_ALPHA: f32 = 0.07;
const DEPTH_PLANE_PATCH_MERGE_GAP_METERS: f32 = 0.35;
const DEPTH_PLANE_SUPPORT_CELL_METERS: f32 = 0.12;
const DEPTH_PLANE_SUPPORT_GROW_WEIGHT: u8 = 4;
const DEPTH_PLANE_SUPPORT_DECAY_WEIGHT: u8 = 1;
const DEPTH_PLANE_SUPPORT_MAX_WEIGHT: u8 = 10;
const DEPTH_PLANE_SUPPORT_OCCUPIED_WEIGHT: u8 = 2;

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct VoxelCoord {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl VoxelCoord {
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
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
pub struct SurfaceMesh32 {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct SparseVoxelMeshingConfig {
    pub mesh_chunk_edge_voxels: i32,
    pub mesh_chunk_overlap_voxels: i32,
    pub cell_stride_voxels: i32,
}

impl SparseVoxelMeshingConfig {
    pub fn mesh_chunk_edge_voxels(self) -> i32 {
        self.mesh_chunk_edge_voxels.max(1)
    }

    pub fn mesh_chunk_overlap_voxels(self) -> i32 {
        self.mesh_chunk_overlap_voxels.max(0)
    }

    pub fn cell_stride_voxels(self) -> i32 {
        self.cell_stride_voxels.max(1)
    }

    pub fn max_padding_voxels(self) -> i32 {
        self.mesh_chunk_overlap_voxels()
    }
}

#[derive(Clone, Debug)]
struct SparseTsdChunk {
    values: Vec<f32>,
    valid: Vec<u8>,
    confidence: Vec<u8>,
    observed_generation: Vec<u64>,
    live_count: usize,
}

impl SparseTsdChunk {
    fn new(chunk_volume: usize) -> Self {
        Self {
            values: vec![0.0; chunk_volume],
            valid: vec![0; chunk_volume],
            confidence: vec![0; chunk_volume],
            observed_generation: vec![0; chunk_volume],
            live_count: 0,
        }
    }

    fn value(&self, id: usize) -> Option<f32> {
        if self.valid[id] == 0 {
            None
        } else {
            Some(self.values[id])
        }
    }

    fn meshing_value(&self, id: usize, current_generation: u64) -> Option<f32> {
        if self.valid[id] == 0 {
            None
        } else if self.confidence[id] >= DEPTH_TSD_MIN_MESH_CONFIDENCE {
            Some(self.values[id])
        } else if self.confidence[id] >= DEPTH_TSD_RECENT_MESH_CONFIDENCE
            && current_generation.saturating_sub(self.observed_generation[id])
                <= DEPTH_TSD_RECENT_MESH_GENERATIONS
            && self.values[id].abs() <= DEPTH_TSD_RECENT_MESH_MAX_ABS_DISTANCE
        {
            Some(self.values[id])
        } else {
            None
        }
    }

    fn confidence(&self, id: usize) -> u8 {
        if self.valid[id] == 0 {
            0
        } else {
            self.confidence[id]
        }
    }

    fn accumulate(&mut self, id: usize, value: f32, generation: u64) -> (bool, bool) {
        let previous = self.value(id);
        let next_value = if let Some(previous) = previous {
            let delta = (previous - value).abs();
            let mut confidence = self.confidence[id].max(1);
            if delta < 0.08 {
                confidence = confidence.saturating_add(2).min(DEPTH_TSD_MAX_CONFIDENCE);
            } else if delta > 0.35 {
                confidence = confidence.saturating_sub(2).max(1);
            }
            let confidence = confidence as f32;
            previous + (value - previous) / (confidence + 1.0)
        } else {
            value
        };
        let changed = previous
            .map(|previous| (previous - next_value).abs() > 1.0e-4)
            .unwrap_or(true);
        self.values[id] = next_value;
        self.valid[id] = 1;
        if let Some(previous) = previous {
            let delta = (previous - value).abs();
            let confidence = &mut self.confidence[id];
            if delta < 0.08 {
                *confidence = confidence.saturating_add(2).min(DEPTH_TSD_MAX_CONFIDENCE);
            } else if delta < 0.18 {
                *confidence = confidence.saturating_add(1).min(DEPTH_TSD_MAX_CONFIDENCE);
            } else if delta > 0.35 {
                *confidence = confidence.saturating_sub(2).max(1);
            } else {
                *confidence = confidence.saturating_sub(1).max(1);
            }
        } else {
            self.confidence[id] = 1;
        }
        self.observed_generation[id] = generation;
        if previous.is_none() {
            self.live_count += 1;
        }
        (changed, previous.is_none())
    }

    fn overwrite(&mut self, id: usize, value: f32, generation: u64) -> (bool, bool) {
        let previous = self.value(id);
        let changed = previous
            .map(|previous| (previous - value).abs() > 1.0e-4)
            .unwrap_or(true);
        self.values[id] = value;
        self.valid[id] = 1;
        self.confidence[id] = DEPTH_TSD_MAX_CONFIDENCE;
        self.observed_generation[id] = generation;
        if previous.is_none() {
            self.live_count += 1;
        }
        (changed, previous.is_none())
    }
}

#[derive(Clone, Debug)]
pub struct SparseTsdGrid {
    voxel_size: f32,
    chunk_edge: i32,
    chunk_volume: usize,
    chunks: HashMap<VoxelCoord, SparseTsdChunk>,
    active_value_count: usize,
}

impl SparseTsdGrid {
    pub fn new(voxel_size: f32, chunk_edge: i32) -> Self {
        Self {
            voxel_size,
            chunk_edge,
            chunk_volume: (chunk_edge as usize).pow(3),
            chunks: HashMap::new(),
            active_value_count: 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.active_value_count == 0
    }

    pub fn normalized_distance(&self, coord: VoxelCoord) -> Option<f32> {
        let (chunk_key, local_id) = self.chunk_key_and_id(coord);
        let chunk = self.chunks.get(&chunk_key)?;
        chunk.value(local_id)
    }

    fn meshing_distance(&self, coord: VoxelCoord, generation: u64) -> Option<f32> {
        let (chunk_key, local_id) = self.chunk_key_and_id(coord);
        let chunk = self.chunks.get(&chunk_key)?;
        chunk.meshing_value(local_id, generation)
    }

    fn confidence(&self, coord: VoxelCoord) -> u8 {
        let (chunk_key, local_id) = self.chunk_key_and_id(coord);
        self.chunks
            .get(&chunk_key)
            .map(|chunk| chunk.confidence(local_id))
            .unwrap_or(0)
    }

    pub fn accumulate_normalized_distance(
        &mut self,
        coord: VoxelCoord,
        value: f32,
        generation: u64,
    ) -> bool {
        let (chunk_key, local_id) = self.chunk_key_and_id(coord);
        let chunk = self
            .chunks
            .entry(chunk_key)
            .or_insert_with(|| SparseTsdChunk::new(self.chunk_volume));
        let (changed, became_live) = chunk.accumulate(local_id, value, generation);
        if became_live {
            self.active_value_count += 1;
        }
        changed
    }

    pub fn overwrite_normalized_distance(
        &mut self,
        coord: VoxelCoord,
        value: f32,
        generation: u64,
    ) -> bool {
        let (chunk_key, local_id) = self.chunk_key_and_id(coord);
        let chunk = self
            .chunks
            .entry(chunk_key)
            .or_insert_with(|| SparseTsdChunk::new(self.chunk_volume));
        let (changed, became_live) = chunk.overwrite(local_id, value, generation);
        if became_live {
            self.active_value_count += 1;
        }
        changed
    }

    pub fn world_to_voxel_coord(&self, point: Vec3f) -> VoxelCoord {
        VoxelCoord::new(
            (point.x / self.voxel_size).floor() as i32,
            (point.y / self.voxel_size).floor() as i32,
            (point.z / self.voxel_size).floor() as i32,
        )
    }

    pub fn world_to_mesh_chunk_key(&self, point: Vec3f, mesh_chunk_edge_voxels: i32) -> VoxelCoord {
        let edge_world = self.voxel_size * mesh_chunk_edge_voxels.max(1) as f32;
        VoxelCoord::new(
            (point.x / edge_world).floor() as i32,
            (point.y / edge_world).floor() as i32,
            (point.z / edge_world).floor() as i32,
        )
    }

    pub fn voxel_center_world(&self, coord: VoxelCoord) -> Vec3f {
        vec3f(
            (coord.x as f32 + 0.5) * self.voxel_size,
            (coord.y as f32 + 0.5) * self.voxel_size,
            (coord.z as f32 + 0.5) * self.voxel_size,
        )
    }

    pub fn world_bounds(&self, padding_voxels: i32) -> Option<(Vec3f, Vec3f)> {
        if self.chunks.is_empty() {
            return None;
        }
        let mut min = VoxelCoord::new(i32::MAX, i32::MAX, i32::MAX);
        let mut max = VoxelCoord::new(i32::MIN, i32::MIN, i32::MIN);
        for (key, chunk) in &self.chunks {
            if chunk.live_count == 0 {
                continue;
            }
            min.x = min.x.min(key.x * self.chunk_edge);
            min.y = min.y.min(key.y * self.chunk_edge);
            min.z = min.z.min(key.z * self.chunk_edge);
            max.x = max.x.max((key.x + 1) * self.chunk_edge);
            max.y = max.y.max((key.y + 1) * self.chunk_edge);
            max.z = max.z.max((key.z + 1) * self.chunk_edge);
        }
        if min.x == i32::MAX {
            return None;
        }
        Some((
            vec3f(
                (min.x - padding_voxels) as f32 * self.voxel_size,
                (min.y - padding_voxels) as f32 * self.voxel_size,
                (min.z - padding_voxels) as f32 * self.voxel_size,
            ),
            vec3f(
                (max.x + padding_voxels) as f32 * self.voxel_size,
                (max.y + padding_voxels) as f32 * self.voxel_size,
                (max.z + padding_voxels) as f32 * self.voxel_size,
            ),
        ))
    }

    pub fn lasertag_surface_net_chunk_mesh_with_scratch(
        &self,
        chunk_key: VoxelCoord,
        config: SparseVoxelMeshingConfig,
        current_generation: u64,
        dense: &mut Vec<f32>,
    ) -> Option<SurfaceMesh32> {
        let edge = config.mesh_chunk_edge_voxels();
        let overlap = config.mesh_chunk_overlap_voxels();
        let stride = config.cell_stride_voxels();
        let start = VoxelCoord::new(chunk_key.x * edge, chunk_key.y * edge, chunk_key.z * edge);
        let extent = VoxelCoord::new(edge + overlap, edge + overlap, edge + overlap);
        if !self.region_has_surface(start, extent) {
            return None;
        }
        let dense_size = VoxelCoord::new(
            align_extent(extent.x, stride),
            align_extent(extent.y, stride),
            align_extent(extent.z, stride),
        );
        self.extract_dense_region_into(start, dense_size, current_generation, dense);
        lasertag_surface_net_mesh_from_dense(dense, dense_size, self.voxel_size, start, stride)
    }

    fn region_has_surface(&self, start: VoxelCoord, extent: VoxelCoord) -> bool {
        if self.is_empty() {
            return false;
        }
        let max = VoxelCoord::new(
            start.x + extent.x.saturating_sub(1),
            start.y + extent.y.saturating_sub(1),
            start.z + extent.z.saturating_sub(1),
        );
        let min_chunk = VoxelCoord::new(
            start.x.div_euclid(self.chunk_edge),
            start.y.div_euclid(self.chunk_edge),
            start.z.div_euclid(self.chunk_edge),
        );
        let max_chunk = VoxelCoord::new(
            max.x.div_euclid(self.chunk_edge),
            max.y.div_euclid(self.chunk_edge),
            max.z.div_euclid(self.chunk_edge),
        );
        for z in min_chunk.z..=max_chunk.z {
            for y in min_chunk.y..=max_chunk.y {
                for x in min_chunk.x..=max_chunk.x {
                    if self
                        .chunks
                        .get(&VoxelCoord::new(x, y, z))
                        .is_some_and(|chunk| chunk.live_count != 0)
                    {
                        return true;
                    }
                }
            }
        }
        false
    }

    fn extract_dense_region_into(
        &self,
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
                    let value = self
                        .meshing_distance(coord, current_generation)
                        .unwrap_or(f32::NEG_INFINITY);
                    dense[(x as usize) + (y as usize) * sx + (z as usize) * sx * sy] = value;
                }
            }
        }
    }

    fn chunk_key_and_id(&self, coord: VoxelCoord) -> (VoxelCoord, usize) {
        let cx = coord.x.div_euclid(self.chunk_edge);
        let cy = coord.y.div_euclid(self.chunk_edge);
        let cz = coord.z.div_euclid(self.chunk_edge);
        let lx = coord.x.rem_euclid(self.chunk_edge) as usize;
        let ly = coord.y.rem_euclid(self.chunk_edge) as usize;
        let lz = coord.z.rem_euclid(self.chunk_edge) as usize;
        let edge = self.chunk_edge as usize;
        let id = lx + ly * edge + lz * edge * edge;
        (VoxelCoord::new(cx, cy, cz), id)
    }
}

#[derive(Debug)]
struct DepthMeshVolume {
    generation: u64,
    latest_topology_generation: u64,
    eye_index: usize,
    image_width: u32,
    image_height: u32,
    sample_step: u32,
    voxel_size_meters: f32,
    bounds_min: Vec3f,
    bounds_max: Vec3f,
    mesh_grid: SparseTsdGrid,
    mesh_config: SparseVoxelMeshingConfig,
    mesh_chunks: Vec<XrDepthMeshChunk>,
    mesh_generation: u64,
    update_sequence: u64,
    dirty_chunk_keys: Vec<IVector>,
    removed_chunk_keys: Vec<IVector>,
    mesh_vertex_count: usize,
    mesh_triangle_count: usize,
    plane_generation: u64,
    plane_patches: Vec<XrDepthPlanePatch>,
    tracked_plane_patches: Vec<TrackedPlanePatch>,
    next_plane_stable_id: u64,
    last_plane_rebuild_at: Instant,
    pending_mesh_dirty_chunks: HashSet<IVector>,
    pending_mesh_chunk_queue: VecDeque<IVector>,
}

impl DepthMeshVolume {
    fn new(sample_step: u32, voxel_size_meters: f32) -> Self {
        let mesh_config = SparseVoxelMeshingConfig {
            mesh_chunk_edge_voxels: 24,
            mesh_chunk_overlap_voxels: 4,
            cell_stride_voxels: 1,
        };
        Self {
            generation: 0,
            latest_topology_generation: 0,
            eye_index: 0,
            image_width: 0,
            image_height: 0,
            sample_step,
            voxel_size_meters,
            bounds_min: vec3f(0.0, 0.0, 0.0),
            bounds_max: vec3f(0.0, 0.0, 0.0),
            mesh_grid: SparseTsdGrid::new(voxel_size_meters, 8),
            mesh_config,
            mesh_chunks: Vec::new(),
            mesh_generation: 0,
            update_sequence: 0,
            dirty_chunk_keys: Vec::new(),
            removed_chunk_keys: Vec::new(),
            mesh_vertex_count: 0,
            mesh_triangle_count: 0,
            plane_generation: 0,
            plane_patches: Vec::new(),
            tracked_plane_patches: Vec::new(),
            next_plane_stable_id: 1,
            last_plane_rebuild_at: Instant::now(),
            pending_mesh_dirty_chunks: HashSet::new(),
            pending_mesh_chunk_queue: VecDeque::new(),
        }
    }

    fn reset_mesh_state(&mut self) {
        self.mesh_chunks.clear();
        self.mesh_generation = self.mesh_generation.saturating_add(1);
        self.update_sequence = self.update_sequence.saturating_add(1);
        self.dirty_chunk_keys.clear();
        self.removed_chunk_keys.clear();
        self.mesh_vertex_count = 0;
        self.mesh_triangle_count = 0;
        if !self.plane_patches.is_empty() {
            self.plane_patches.clear();
            self.plane_generation = self.plane_generation.saturating_add(1);
        }
        self.tracked_plane_patches.clear();
        self.pending_mesh_dirty_chunks.clear();
        self.pending_mesh_chunk_queue.clear();
    }

    fn update_bounds(&mut self) {
        if let Some((min, max)) = self.mesh_grid.world_bounds(0) {
            self.bounds_min = min;
            self.bounds_max = max;
        } else {
            (self.bounds_min, self.bounds_max) = empty_bounds();
        }
    }

    fn snapshot(&self) -> XrDepthMesh {
        XrDepthMesh {
            generation: self.generation,
            latest_topology_generation: self.latest_topology_generation,
            update_sequence: self.update_sequence,
            eye_index: self.eye_index,
            image_width: self.image_width,
            image_height: self.image_height,
            sample_step: self.sample_step,
            voxel_size_meters: self.voxel_size_meters,
            bounds_min: self.bounds_min,
            bounds_max: self.bounds_max,
            mesh_chunks: self.mesh_chunks.clone(),
            dirty_chunk_keys: self.dirty_chunk_keys.clone(),
            removed_chunk_keys: self.removed_chunk_keys.clone(),
            mesh_generation: self.mesh_generation,
            mesh_vertex_count: self.mesh_vertex_count,
            mesh_triangle_count: self.mesh_triangle_count,
            plane_generation: self.plane_generation,
            plane_patches: self.plane_patches.clone(),
        }
    }

    fn record_dirty_chunk(&mut self, chunk_key: IVector) {
        push_unique_chunk_key(&mut self.dirty_chunk_keys, chunk_key);
        self.removed_chunk_keys.retain(|key| *key != chunk_key);
    }

    fn record_removed_chunk(&mut self, chunk_key: IVector) {
        push_unique_chunk_key(&mut self.removed_chunk_keys, chunk_key);
        self.dirty_chunk_keys.retain(|key| *key != chunk_key);
    }
}

pub(super) struct CxOpenXrDepthMeshJob {
    generation: u64,
    eye_index: usize,
    width: u32,
    height: u32,
    sample_step: u32,
    camera_world: Vec3f,
    depth_proj: Mat4f,
    inv_depth_proj: Mat4f,
    depth_view_from_world: Mat4f,
    world_from_depth_view: Mat4f,
    depth: Vec<u16>,
}

struct CxOpenXrPreparedDepthMeshJob {
    generation: u64,
    eye_index: usize,
    width: u32,
    height: u32,
    sample_step: u32,
    camera_world: Vec3f,
    depth_proj: Mat4f,
    inv_depth_proj: Mat4f,
    depth_view_from_world: Mat4f,
    frame_tsd_samples: HashMap<VoxelCoord, f32>,
    visible_world_min: Vec3f,
    visible_world_max: Vec3f,
    depth: Vec<u16>,
}

#[derive(Clone, Copy, Debug, Default)]
struct FrameTsdSampleAccum {
    sum: f32,
    count: u16,
}

pub(super) struct CxOpenXrDepthMeshPipeline {
    sender: Sender<CxOpenXrDepthMeshJob>,
    busy: Arc<AtomicBool>,
    store: XrDepthMeshStore,
    next_generation: u64,
    last_submit_at: Option<Instant>,
    last_camera_world: Option<Vec3f>,
    last_camera_forward: Option<Vec3f>,
}

impl CxOpenXrDepthMeshPipeline {
    pub fn new() -> Self {
        let store = xr_depth_mesh_store();
        let busy = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = channel::<CxOpenXrDepthMeshJob>();
        let (prepared_sender, prepared_receiver) = channel::<CxOpenXrPreparedDepthMeshJob>();
        std::thread::spawn({
            let busy = busy.clone();
            let store = store.clone();
            move || depth_preprocess_worker(receiver, prepared_sender, busy, store)
        });
        std::thread::spawn({
            let store = store.clone();
            move || depth_mesher_worker(prepared_receiver, store)
        });
        Self {
            sender,
            busy,
            store,
            next_generation: 1,
            last_submit_at: None,
            last_camera_world: None,
            last_camera_forward: None,
        }
    }

    pub fn submit(
        &mut self,
        vulkan: &mut CxVulkan,
        render_targets: &CxVulkanOpenXrSessionData,
        frame: &CxOpenXrFrame,
        depth_image_index: usize,
    ) -> Result<(), String> {
        self.store.record_seen();
        let now = Instant::now();
        let pose_result = (|| {
            let width = render_targets.depth_width;
            let height = render_targets.depth_height;
            if width == 0 || height == 0 {
                return Err("OpenXR depth readback dimensions are zero".to_string());
            }
            let world_from_depth_view = frame.eyes[DEPTH_VOXEL_EYE_INDEX].depth_view_mat.invert();
            let camera_world = world_from_depth_view.transform_vec4(vec4f(0.0, 0.0, 0.0, 1.0));
            if !camera_world.w.is_finite() || camera_world.w.abs() < 1.0e-6 {
                return Err("OpenXR depth camera transform is invalid".to_string());
            }
            let camera_forward = world_from_depth_view.transform_vec4(vec4f(0.0, 0.0, -1.0, 0.0));
            let camera_forward =
                vec3f(camera_forward.x, camera_forward.y, camera_forward.z).normalize();
            let camera_world = vec3f(
                camera_world.x / camera_world.w,
                camera_world.y / camera_world.w,
                camera_world.z / camera_world.w,
            );
            Ok((
                width,
                height,
                world_from_depth_view,
                camera_world,
                camera_forward,
            ))
        })();

        let (width, height, world_from_depth_view, camera_world, camera_forward) = match pose_result
        {
            Ok(parts) => parts,
            Err(err) => {
                self.store.set_error(err.clone());
                return Err(err);
            }
        };

        let moved = self
            .last_camera_world
            .map(|last| {
                (camera_world - last).length() >= DEPTH_TSD_UPDATE_TRANSLATION_TRIGGER_METERS
            })
            .unwrap_or(true);
        let rotated = self
            .last_camera_forward
            .map(|last| camera_forward.dot(last) <= DEPTH_TSD_UPDATE_ROTATION_TRIGGER_DOT)
            .unwrap_or(true);
        let min_interval = if moved || rotated {
            DEPTH_TSD_UPDATE_MOVING_INTERVAL_MILLIS
        } else {
            DEPTH_TSD_UPDATE_IDLE_INTERVAL_MILLIS
        };

        if self
            .last_submit_at
            .is_some_and(|last| now.duration_since(last) < Duration::from_millis(min_interval))
        {
            self.store.record_drop();
            return Ok(());
        }
        if self
            .busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            self.store.record_drop();
            return Ok(());
        }

        let generation = self.next_generation;
        self.next_generation += 1;

        let job_result: Result<CxOpenXrDepthMeshJob, String> = (|| {
            let depth = vulkan.read_openxr_depth_image(
                render_targets,
                depth_image_index,
                DEPTH_VOXEL_EYE_INDEX,
            )?;
            Ok(CxOpenXrDepthMeshJob {
                generation,
                eye_index: DEPTH_VOXEL_EYE_INDEX,
                width,
                height,
                sample_step: DEPTH_VOXEL_SAMPLE_STEP,
                camera_world,
                depth_proj: frame.eyes[DEPTH_VOXEL_EYE_INDEX].depth_proj_mat,
                inv_depth_proj: frame.eyes[DEPTH_VOXEL_EYE_INDEX].depth_proj_mat.invert(),
                depth_view_from_world: frame.eyes[DEPTH_VOXEL_EYE_INDEX].depth_view_mat,
                world_from_depth_view,
                depth,
            })
        })();

        let job = match job_result {
            Ok(job) => job,
            Err(err) => {
                self.busy.store(false, Ordering::Release);
                self.store.set_error(err.clone());
                return Err(err);
            }
        };

        if let Err(err) = self.sender.send(job) {
            let err = format!("OpenXR depth worker is unavailable: {err}");
            self.busy.store(false, Ordering::Release);
            self.store.set_error(err.clone());
            return Err(err);
        }

        self.last_submit_at = Some(now);
        self.last_camera_world = Some(camera_world);
        self.last_camera_forward = Some(camera_forward);
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
struct DepthGridSample {
    world: Vec3f,
    normal: Vec3f,
    ray_distance: f32,
    valid: bool,
    has_normal: bool,
}

#[derive(Default)]
struct DepthPreprocessWorkerState {
    sampled_depth: Vec<DepthGridSample>,
    depth_width: usize,
    depth_height: usize,
}

#[derive(Default)]
struct DepthMesherWorkerState {
    mesh_scratch: Vec<f32>,
}

fn depth_preprocess_worker(
    receiver: Receiver<CxOpenXrDepthMeshJob>,
    sender: Sender<CxOpenXrPreparedDepthMeshJob>,
    busy: Arc<AtomicBool>,
    store: XrDepthMeshStore,
) {
    let mut worker_state = DepthPreprocessWorkerState::default();
    while let Ok(job) = receiver.recv() {
        let result = preprocess_depth_mesh(job, &mut worker_state);
        busy.store(false, Ordering::Release);
        match result {
            Ok(job) => {
                if let Err(err) = sender.send(job) {
                    store.set_error(format!("OpenXR depth mesher is unavailable: {err}"));
                    break;
                }
            }
            Err(err) => {
                store.set_error(err);
            }
        }
    }
}

fn depth_mesher_worker(receiver: Receiver<CxOpenXrPreparedDepthMeshJob>, store: XrDepthMeshStore) {
    let mut worker_state = DepthMesherWorkerState::default();
    let mut volume = DepthMeshVolume::new(DEPTH_VOXEL_SAMPLE_STEP, DEPTH_VOXEL_SIZE_METERS);
    loop {
        let mut applied_update = false;
        match receiver.recv_timeout(Duration::from_millis(DEPTH_SURFACE_MESH_IDLE_WAIT_MILLIS)) {
            Ok(mut job) => {
                while let Ok(newer) = receiver.try_recv() {
                    job = newer;
                }
                apply_preprocessed_depth_mesh(job, &mut volume);
                applied_update = true;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }

        let mesh_changed = process_incremental_surface_mesh(
            &mut volume,
            &mut worker_state,
            DEPTH_SURFACE_MESH_CHUNKS_PER_TICK,
        );
        let plane_changed = maybe_rebuild_plane_patches(&mut volume, mesh_changed);
        let query_changed = process_geometry_queries(&volume, &store, DEPTH_QUERY_BATCH_PER_TICK);
        if applied_update || mesh_changed || plane_changed || query_changed {
            store.publish(volume.snapshot());
            SignalToUI::set_ui_signal();
        }
    }
}

fn preprocess_depth_mesh(
    job: CxOpenXrDepthMeshJob,
    worker_state: &mut DepthPreprocessWorkerState,
) -> Result<CxOpenXrPreparedDepthMeshJob, String> {
    rebuild_sampled_depth_grid(&job, worker_state);

    let mut frame_tsd_accum = HashMap::<VoxelCoord, FrameTsdSampleAccum>::new();
    let mut observed_world_min = vec3f(f32::INFINITY, f32::INFINITY, f32::INFINITY);
    let mut observed_world_max = vec3f(f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
    let sample_step = job.sample_step.max(1) as usize;
    let ray_step = (DEPTH_VOXEL_SIZE_METERS * 0.5).max(0.02);

    for y in (0..worker_state.depth_height).step_by(sample_step) {
        for x in (0..worker_state.depth_width).step_by(sample_step) {
            if !depth_pixel_inside_margin(worker_state.depth_width, worker_state.depth_height, x, y)
            {
                continue;
            }
            let sample = worker_state.sampled_depth[y * worker_state.depth_width + x];
            if !sample.valid || !sample.has_normal {
                continue;
            }
            if point_inside_player_exclusion(job.camera_world, sample.world) {
                continue;
            }

            let surface_ray = sample.world - job.camera_world;
            let surface_distance = sample.ray_distance;
            if !surface_distance.is_finite()
                || !(DEPTH_TSD_MIN_UPDATE_DISTANCE_METERS..=DEPTH_VOXEL_MAX_DISTANCE_METERS)
                    .contains(&surface_distance)
            {
                continue;
            }

            let ray_dir = surface_ray.normalize();
            let norm_dot = if sample.has_normal {
                (-sample.normal.dot(ray_dir)).clamp(0.0, 1.0)
            } else {
                1.0
            };
            if sample.has_normal && norm_dot <= DEPTH_TSD_MIN_NORMAL_DOT {
                continue;
            }

            observed_world_min = Vec3f::min_componentwise(observed_world_min, sample.world);
            observed_world_max = Vec3f::max_componentwise(observed_world_max, sample.world);

            let start_distance = (surface_distance - DEPTH_TSD_DISTANCE_METERS)
                .max(DEPTH_TSD_MIN_UPDATE_DISTANCE_METERS);
            let end_distance =
                (surface_distance + DEPTH_TSD_DISTANCE_METERS).min(DEPTH_VOXEL_MAX_DISTANCE_METERS);
            let mut last_coord = None;
            let mut distance = start_distance;
            while distance <= end_distance {
                let sample_world = job.camera_world + ray_dir.scale(distance);
                let coord = VoxelCoord::new(
                    (sample_world.x / DEPTH_VOXEL_SIZE_METERS).floor() as i32,
                    (sample_world.y / DEPTH_VOXEL_SIZE_METERS).floor() as i32,
                    (sample_world.z / DEPTH_VOXEL_SIZE_METERS).floor() as i32,
                );
                if last_coord == Some(coord) {
                    distance += ray_step;
                    continue;
                }
                last_coord = Some(coord);

                let voxel_world = vec3f(
                    (coord.x as f32 + 0.5) * DEPTH_VOXEL_SIZE_METERS,
                    (coord.y as f32 + 0.5) * DEPTH_VOXEL_SIZE_METERS,
                    (coord.z as f32 + 0.5) * DEPTH_VOXEL_SIZE_METERS,
                );
                if point_inside_player_exclusion(job.camera_world, voxel_world) {
                    distance += ray_step;
                    continue;
                }

                let voxel_distance = (voxel_world - job.camera_world).dot(ray_dir);
                if !voxel_distance.is_finite() {
                    distance += ray_step;
                    continue;
                }
                let normalized = ((surface_distance - voxel_distance) / DEPTH_TSD_DISTANCE_METERS)
                    .clamp(-1.0, 1.0);
                frame_tsd_accum
                    .entry(coord)
                    .and_modify(|current| {
                        current.sum += normalized;
                        current.count = current.count.saturating_add(1);
                    })
                    .or_insert(FrameTsdSampleAccum {
                        sum: normalized,
                        count: 1,
                    });
                distance += ray_step;
            }
        }
    }

    let frame_tsd_samples = frame_tsd_accum
        .into_iter()
        .map(|(coord, accum)| (coord, accum.sum / accum.count.max(1) as f32))
        .collect();

    let (visible_world_min, visible_world_max) = if observed_world_min.x.is_finite()
        && observed_world_min.y.is_finite()
        && observed_world_min.z.is_finite()
        && observed_world_max.x.is_finite()
        && observed_world_max.y.is_finite()
        && observed_world_max.z.is_finite()
    {
        let padding = vec3f(
            DEPTH_TSD_DISTANCE_METERS,
            DEPTH_TSD_DISTANCE_METERS,
            DEPTH_TSD_DISTANCE_METERS,
        );
        (observed_world_min - padding, observed_world_max + padding)
    } else {
        depth_visible_world_bounds(&job).unwrap_or((vec3f(0.0, 0.0, 0.0), vec3f(0.0, 0.0, 0.0)))
    };

    Ok(CxOpenXrPreparedDepthMeshJob {
        generation: job.generation,
        eye_index: job.eye_index,
        width: job.width,
        height: job.height,
        sample_step: job.sample_step,
        camera_world: job.camera_world,
        depth_proj: job.depth_proj,
        inv_depth_proj: job.inv_depth_proj,
        depth_view_from_world: job.depth_view_from_world,
        frame_tsd_samples,
        visible_world_min,
        visible_world_max,
        depth: job.depth,
    })
}

fn apply_preprocessed_depth_mesh(job: CxOpenXrPreparedDepthMeshJob, volume: &mut DepthMeshVolume) {
    volume.generation = job.generation;
    volume.eye_index = job.eye_index;
    volume.image_width = job.width;
    volume.image_height = job.height;
    volume.sample_step = job.sample_step;

    let mut topology_changes = apply_tsd_samples(volume, &job.frame_tsd_samples);
    topology_changes += refresh_visible_free_space(volume, &job);
    topology_changes += clear_player_exclusion_volume(volume, job.camera_world);
    if topology_changes != 0 {
        volume.latest_topology_generation = job.generation;
    }
    volume.update_bounds();
    enqueue_visible_mesh_chunks(volume, job.visible_world_min, job.visible_world_max);
}

fn rebuild_sampled_depth_grid(
    job: &CxOpenXrDepthMeshJob,
    worker_state: &mut DepthPreprocessWorkerState,
) {
    let width = job.width as usize;
    let height = job.height as usize;
    worker_state.depth_width = width;
    worker_state.depth_height = height;
    worker_state.sampled_depth.clear();
    worker_state.sampled_depth.resize(
        width * height,
        DepthGridSample {
            world: vec3f(0.0, 0.0, 0.0),
            normal: vec3f(0.0, 1.0, 0.0),
            ray_distance: 0.0,
            valid: false,
            has_normal: false,
        },
    );

    for y in 0..height {
        for x in 0..width {
            if !depth_pixel_inside_margin(width, height, x, y) {
                continue;
            }
            let raw_depth = job.depth[y * width + x] as f32 / u16::MAX as f32;
            if !(DEPTH_VOXEL_MIN_DEPTH_VALUE..DEPTH_VOXEL_MAX_DEPTH_VALUE).contains(&raw_depth) {
                continue;
            }
            let Some(world) = depth_pixel_to_world(job, x as u32, y as u32) else {
                continue;
            };
            worker_state.sampled_depth[y * width + x] = DepthGridSample {
                world,
                normal: vec3f(0.0, 1.0, 0.0),
                ray_distance: (world - job.camera_world).length(),
                valid: true,
                has_normal: false,
            };
        }
    }

    for y in 0..height {
        for x in 0..width {
            let index = y * width + x;
            if !worker_state.sampled_depth[index].valid {
                continue;
            }
            let world = worker_state.sampled_depth[index].world;
            let sample_x =
                sampled_depth_at_pixel(worker_state, (x + 2).min(width - 1) as u32, y as u32);
            let sample_y =
                sampled_depth_at_pixel(worker_state, x as u32, (y + 2).min(height - 1) as u32);
            if !sample_x.valid || !sample_y.valid {
                continue;
            }
            let ray_distance = worker_state.sampled_depth[index].ray_distance;
            if (sample_x.ray_distance - ray_distance).abs()
                > DEPTH_NORMAL_NEIGHBOR_MAX_DISTANCE_DELTA_METERS
                || (sample_y.ray_distance - ray_distance).abs()
                    > DEPTH_NORMAL_NEIGHBOR_MAX_DISTANCE_DELTA_METERS
            {
                continue;
            }
            let h_deriv = sample_x.world - world;
            let v_deriv = sample_y.world - world;
            if h_deriv.length() <= 1.0e-4 || v_deriv.length() <= 1.0e-4 {
                continue;
            }
            let mut normal = Vec3f::cross(h_deriv, v_deriv).normalize().scale(-1.0);
            if normal.length() <= 1.0e-4 {
                continue;
            }
            let view_dir = (job.camera_world - world).normalize();
            if normal.dot(view_dir) < 0.0 {
                normal = normal.scale(-1.0);
            }
            worker_state.sampled_depth[index].normal = normal;
            worker_state.sampled_depth[index].has_normal = true;
        }
    }
}

fn sampled_depth_at_pixel(
    worker_state: &DepthPreprocessWorkerState,
    pixel_x: u32,
    pixel_y: u32,
) -> DepthGridSample {
    if worker_state.depth_width == 0 || worker_state.depth_height == 0 {
        return DepthGridSample {
            world: vec3f(0.0, 0.0, 0.0),
            normal: vec3f(0.0, 1.0, 0.0),
            ray_distance: 0.0,
            valid: false,
            has_normal: false,
        };
    }
    let x = pixel_x.min(worker_state.depth_width.saturating_sub(1) as u32) as usize;
    let y = pixel_y.min(worker_state.depth_height.saturating_sub(1) as u32) as usize;
    worker_state.sampled_depth[y * worker_state.depth_width + x]
}

fn depth_pixel_inside_margin(width: usize, height: usize, x: usize, y: usize) -> bool {
    let margin = DEPTH_IMAGE_EDGE_MARGIN_PIXELS;
    if width <= margin * 2 || height <= margin * 2 {
        return true;
    }
    x >= margin && y >= margin && x + margin < width && y + margin < height
}

fn depth_ndc_to_view(
    job: &CxOpenXrDepthMeshJob,
    ndc_x: f32,
    ndc_y: f32,
    ndc_z: f32,
) -> Option<Vec3f> {
    let view = job
        .inv_depth_proj
        .transform_vec4(vec4f(ndc_x, ndc_y, ndc_z, 1.0));
    if !view.w.is_finite() || view.w.abs() < 1.0e-6 {
        return None;
    }
    let inv_w = 1.0 / view.w;
    let point = vec3f(view.x * inv_w, view.y * inv_w, view.z * inv_w);
    (point.x.is_finite() && point.y.is_finite() && point.z.is_finite()).then_some(point)
}

fn depth_view_to_world(job: &CxOpenXrDepthMeshJob, view: Vec3f) -> Option<Vec3f> {
    let world = job
        .world_from_depth_view
        .transform_vec4(vec4f(view.x, view.y, view.z, 1.0));
    if !world.w.is_finite() || world.w.abs() < 1.0e-6 {
        return None;
    }
    let inv_w = 1.0 / world.w;
    let point = vec3f(world.x * inv_w, world.y * inv_w, world.z * inv_w);
    (point.x.is_finite() && point.y.is_finite() && point.z.is_finite()).then_some(point)
}

fn depth_pixel_to_world(job: &CxOpenXrDepthMeshJob, x: u32, y: u32) -> Option<Vec3f> {
    let view = depth_pixel_to_view(
        &job.depth,
        job.width,
        job.height,
        job.inv_depth_proj,
        x as usize,
        y as usize,
    )?;
    let world = job.world_from_depth_view.transform_vec4(view);
    if !world.w.is_finite() || world.w.abs() < 1.0e-6 {
        return None;
    }
    let inv_w = 1.0 / world.w;
    let point = vec3f(world.x * inv_w, world.y * inv_w, world.z * inv_w);
    (point.x.is_finite() && point.y.is_finite() && point.z.is_finite()).then_some(point)
}

fn depth_pixel_to_view(
    depth: &[u16],
    width: u32,
    height: u32,
    inv_depth_proj: Mat4f,
    x: usize,
    y: usize,
) -> Option<Vec4f> {
    let raw_depth = *depth.get(y * width as usize + x)? as f32 / u16::MAX as f32;
    if !(DEPTH_VOXEL_MIN_DEPTH_VALUE..DEPTH_VOXEL_MAX_DEPTH_VALUE).contains(&raw_depth) {
        return None;
    }
    let uv_x = (x as f32 + 0.5) / width as f32;
    let uv_y = (y as f32 + 0.5) / height as f32;
    let clip = vec4f(
        uv_x * 2.0 - 1.0,
        uv_y * 2.0 - 1.0,
        raw_depth * 2.0 - 1.0,
        1.0,
    );
    let view = inv_depth_proj.transform_vec4(clip);
    if !view.w.is_finite() || view.w.abs() < 1.0e-6 {
        return None;
    }
    let view = vec4f(view.x / view.w, view.y / view.w, view.z / view.w, 1.0);
    let distance = view.to_vec3f().length();
    if !distance.is_finite()
        || !(DEPTH_VOXEL_MIN_DISTANCE_METERS..=DEPTH_VOXEL_MAX_DISTANCE_METERS).contains(&distance)
    {
        return None;
    }
    Some(view)
}

fn depth_world_to_pixel(
    world: Vec3f,
    width: u32,
    height: u32,
    depth_view_from_world: Mat4f,
    depth_proj: Mat4f,
) -> Option<(usize, usize, f32)> {
    let view = depth_view_from_world.transform_vec4(vec4f(world.x, world.y, world.z, 1.0));
    if !view.w.is_finite() || view.w.abs() < 1.0e-6 {
        return None;
    }
    let view = vec4f(view.x / view.w, view.y / view.w, view.z / view.w, 1.0);
    let view_pos = view.to_vec3f();
    let ray_distance = view_pos.length();
    if !ray_distance.is_finite()
        || !(DEPTH_VOXEL_MIN_DISTANCE_METERS..=DEPTH_VOXEL_MAX_DISTANCE_METERS)
            .contains(&ray_distance)
    {
        return None;
    }
    let clip = depth_proj.transform_vec4(view);
    if !clip.w.is_finite() || clip.w.abs() < 1.0e-6 {
        return None;
    }
    let ndc_x = clip.x / clip.w;
    let ndc_y = clip.y / clip.w;
    let ndc_z = clip.z / clip.w;
    if !ndc_x.is_finite() || !ndc_y.is_finite() || !ndc_z.is_finite() {
        return None;
    }
    if !(-1.0..=1.0).contains(&ndc_x)
        || !(-1.0..=1.0).contains(&ndc_y)
        || !(-1.0..=1.0).contains(&ndc_z)
    {
        return None;
    }
    let pixel_x = ((ndc_x * 0.5 + 0.5) * width as f32).floor() as isize;
    let pixel_y = ((ndc_y * 0.5 + 0.5) * height as f32).floor() as isize;
    if pixel_x < 0 || pixel_y < 0 || pixel_x >= width as isize || pixel_y >= height as isize {
        return None;
    }
    Some((pixel_x as usize, pixel_y as usize, ray_distance))
}

fn depth_pixel_is_reliable_for_carve(
    job: &CxOpenXrPreparedDepthMeshJob,
    pixel_x: usize,
    pixel_y: usize,
    observed_distance: f32,
) -> bool {
    let width = job.width as usize;
    let height = job.height as usize;
    if !depth_pixel_inside_margin(width, height, pixel_x, pixel_y) {
        return false;
    }

    let mut agreeing_neighbors = 0u8;
    for (nx, ny) in [
        (pixel_x.saturating_sub(1), pixel_y),
        ((pixel_x + 1).min(width.saturating_sub(1)), pixel_y),
        (pixel_x, pixel_y.saturating_sub(1)),
        (pixel_x, (pixel_y + 1).min(height.saturating_sub(1))),
    ] {
        if nx == pixel_x && ny == pixel_y {
            continue;
        }
        let Some(neighbor_view) = depth_pixel_to_view(
            &job.depth,
            job.width,
            job.height,
            job.inv_depth_proj,
            nx,
            ny,
        ) else {
            continue;
        };
        let neighbor_distance = neighbor_view.to_vec3f().length();
        if (neighbor_distance - observed_distance).abs()
            <= DEPTH_CARVE_NEIGHBOR_MAX_DISTANCE_DELTA_METERS
        {
            agreeing_neighbors = agreeing_neighbors.saturating_add(1);
        }
    }
    agreeing_neighbors >= 2
}

fn apply_tsd_samples(
    volume: &mut DepthMeshVolume,
    frame_tsd_samples: &HashMap<VoxelCoord, f32>,
) -> usize {
    let mut changed = 0;
    for (&coord, &normalized) in frame_tsd_samples {
        let previous = volume.mesh_grid.normalized_distance(coord).unwrap_or(2.0);
        if volume
            .mesh_grid
            .accumulate_normalized_distance(coord, normalized, volume.generation)
        {
            let current = volume
                .mesh_grid
                .normalized_distance(coord)
                .unwrap_or(previous);
            if (previous - current).abs() >= DEPTH_TSD_APPLY_DELTA_EPSILON {
                mark_mesh_chunk_dirty(volume, coord);
                changed += 1;
            }
        }
    }
    changed
}

fn refresh_visible_free_space(
    volume: &mut DepthMeshVolume,
    job: &CxOpenXrPreparedDepthMeshJob,
) -> usize {
    let min_coord = volume.mesh_grid.world_to_voxel_coord(job.visible_world_min);
    let max_coord = volume.mesh_grid.world_to_voxel_coord(job.visible_world_max);
    let mut changed = 0;

    for z in min_coord.z..=max_coord.z {
        for y in min_coord.y..=max_coord.y {
            for x in min_coord.x..=max_coord.x {
                let coord = VoxelCoord::new(x, y, z);
                let Some(previous) = volume.mesh_grid.normalized_distance(coord) else {
                    continue;
                };
                if previous >= 1.0 - 1.0e-4 {
                    continue;
                }
                let voxel_world = volume.mesh_grid.voxel_center_world(coord);
                let Some((pixel_x, pixel_y, voxel_distance)) = depth_world_to_pixel(
                    voxel_world,
                    job.width,
                    job.height,
                    job.depth_view_from_world,
                    job.depth_proj,
                ) else {
                    continue;
                };
                if !depth_pixel_inside_margin(
                    job.width as usize,
                    job.height as usize,
                    pixel_x,
                    pixel_y,
                ) {
                    continue;
                }
                let Some(observed_view) = depth_pixel_to_view(
                    &job.depth,
                    job.width,
                    job.height,
                    job.inv_depth_proj,
                    pixel_x,
                    pixel_y,
                ) else {
                    continue;
                };
                let observed_distance = observed_view.to_vec3f().length();
                if !depth_pixel_is_reliable_for_carve(job, pixel_x, pixel_y, observed_distance) {
                    continue;
                }
                let clearance = observed_distance - voxel_distance;
                if !observed_distance.is_finite() || clearance < DEPTH_TSD_REFRESH_CLEARANCE_METERS
                {
                    continue;
                }
                let confidence = volume.mesh_grid.confidence(coord);
                if confidence >= DEPTH_TSD_STABLE_CONFIDENCE
                    && previous <= 0.25
                    && clearance < DEPTH_TSD_DISTANCE_METERS
                {
                    continue;
                }
                if volume
                    .mesh_grid
                    .accumulate_normalized_distance(coord, 1.0, volume.generation)
                {
                    let current = volume
                        .mesh_grid
                        .normalized_distance(coord)
                        .unwrap_or(previous);
                    if (previous - current).abs() >= DEPTH_TSD_APPLY_DELTA_EPSILON {
                        mark_mesh_chunk_dirty(volume, coord);
                        changed += 1;
                    }
                }
            }
        }
    }

    changed
}

fn clear_player_exclusion_volume(volume: &mut DepthMeshVolume, camera_world: Vec3f) -> usize {
    let min_world = vec3f(
        camera_world.x - DEPTH_PLAYER_EXCLUDE_RADIUS_METERS,
        camera_world.y - DEPTH_PLAYER_EXCLUDE_BOTTOM_METERS,
        camera_world.z - DEPTH_PLAYER_EXCLUDE_RADIUS_METERS,
    );
    let max_world = vec3f(
        camera_world.x + DEPTH_PLAYER_EXCLUDE_RADIUS_METERS,
        camera_world.y + DEPTH_PLAYER_EXCLUDE_TOP_METERS,
        camera_world.z + DEPTH_PLAYER_EXCLUDE_RADIUS_METERS,
    );
    let min_coord = volume.mesh_grid.world_to_voxel_coord(min_world);
    let max_coord = volume.mesh_grid.world_to_voxel_coord(max_world);
    let mut changed = 0;
    for z in min_coord.z..=max_coord.z {
        for y in min_coord.y..=max_coord.y {
            for x in min_coord.x..=max_coord.x {
                let coord = VoxelCoord::new(x, y, z);
                let center = volume.mesh_grid.voxel_center_world(coord);
                if !point_inside_player_exclusion(camera_world, center) {
                    continue;
                }
                let Some(previous) = volume.mesh_grid.normalized_distance(coord) else {
                    continue;
                };
                if volume.mesh_grid.confidence(coord) > DEPTH_PLAYER_CLEAR_MAX_CONFIDENCE {
                    continue;
                }
                if previous >= 1.0 - 1.0e-4 {
                    continue;
                }
                if volume
                    .mesh_grid
                    .overwrite_normalized_distance(coord, 1.0, volume.generation)
                {
                    mark_mesh_chunk_dirty(volume, coord);
                    changed += 1;
                }
            }
        }
    }
    changed
}

fn mark_mesh_chunk_dirty(volume: &mut DepthMeshVolume, voxel: VoxelCoord) {
    let padding = volume.mesh_config.max_padding_voxels();
    let edge = volume.mesh_config.mesh_chunk_edge_voxels();
    let min_chunk = VoxelCoord::new(
        (voxel.x - padding).div_euclid(edge),
        (voxel.y - padding).div_euclid(edge),
        (voxel.z - padding).div_euclid(edge),
    );
    let max_chunk = VoxelCoord::new(
        (voxel.x + padding).div_euclid(edge),
        (voxel.y + padding).div_euclid(edge),
        (voxel.z + padding).div_euclid(edge),
    );
    for z in min_chunk.z..=max_chunk.z {
        for y in min_chunk.y..=max_chunk.y {
            for x in min_chunk.x..=max_chunk.x {
                let key = IVector::new(x, y, z);
                if volume.pending_mesh_dirty_chunks.insert(key) {
                    volume.pending_mesh_chunk_queue.push_back(key);
                }
            }
        }
    }
}

fn depth_visible_world_bounds(job: &CxOpenXrDepthMeshJob) -> Option<(Vec3f, Vec3f)> {
    let Some(bottom_left) = depth_ndc_to_view(job, -1.0, -1.0, 0.0) else {
        return None;
    };
    let Some(bottom_right) = depth_ndc_to_view(job, 1.0, -1.0, 0.0) else {
        return None;
    };
    let Some(top_right) = depth_ndc_to_view(job, 1.0, 1.0, 0.0) else {
        return None;
    };
    let Some(top_left) = depth_ndc_to_view(job, -1.0, 1.0, 0.0) else {
        return None;
    };
    let mut corners = [bottom_left, bottom_right, top_right, top_left];
    for corner in &mut corners {
        let scale = DEPTH_MESH_UPDATE_DISTANCE_METERS / (-corner.z).max(1.0e-6);
        *corner = corner.scale(scale);
    }

    let mut world_min = vec3f(f32::INFINITY, f32::INFINITY, f32::INFINITY);
    let mut world_max = vec3f(f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
    for corner in corners {
        let Some(world) = depth_view_to_world(job, corner) else {
            continue;
        };
        world_min = Vec3f::min_componentwise(world_min, world);
        world_max = Vec3f::max_componentwise(world_max, world);
    }
    (world_min.x.is_finite()
        && world_min.y.is_finite()
        && world_min.z.is_finite()
        && world_max.x.is_finite()
        && world_max.y.is_finite()
        && world_max.z.is_finite())
    .then_some((world_min, world_max))
}

fn enqueue_visible_mesh_chunks(volume: &mut DepthMeshVolume, world_min: Vec3f, world_max: Vec3f) {
    let edge = volume.mesh_config.mesh_chunk_edge_voxels();
    let min_key = volume.mesh_grid.world_to_mesh_chunk_key(world_min, edge);
    let max_key = volume.mesh_grid.world_to_mesh_chunk_key(world_max, edge);
    let meshed_keys: HashSet<IVector> = volume
        .mesh_chunks
        .iter()
        .map(|chunk| chunk.chunk_key)
        .collect();
    for z in (min_key.z - 1)..=(max_key.z + 1) {
        for y in (min_key.y - 1)..=(max_key.y + 1) {
            for x in (min_key.x - 1)..=(max_key.x + 1) {
                let key = IVector::new(x, y, z);
                if !meshed_keys.contains(&key) && volume.pending_mesh_dirty_chunks.insert(key) {
                    volume.pending_mesh_chunk_queue.push_back(key);
                }
            }
        }
    }
}

fn point_inside_player_exclusion(camera_world: Vec3f, world: Vec3f) -> bool {
    let dx = world.x - camera_world.x;
    let dz = world.z - camera_world.z;
    let horizontal_sq = dx * dx + dz * dz;
    horizontal_sq <= DEPTH_PLAYER_EXCLUDE_RADIUS_METERS * DEPTH_PLAYER_EXCLUDE_RADIUS_METERS
        && world.y <= camera_world.y + DEPTH_PLAYER_EXCLUDE_TOP_METERS
        && world.y >= camera_world.y - DEPTH_PLAYER_EXCLUDE_BOTTOM_METERS
}

fn process_incremental_surface_mesh(
    volume: &mut DepthMeshVolume,
    worker_state: &mut DepthMesherWorkerState,
    max_mesh_jobs: usize,
) -> bool {
    if volume.mesh_grid.is_empty() {
        if !volume.mesh_chunks.is_empty() {
            volume.reset_mesh_state();
            return true;
        }
        return false;
    }

    let mut mesh_changed = false;
    for _ in 0..max_mesh_jobs {
        let Some(chunk_key) = volume.pending_mesh_chunk_queue.pop_front() else {
            break;
        };
        volume.pending_mesh_dirty_chunks.remove(&chunk_key);
        let started = Instant::now();
        let mesh = volume
            .mesh_grid
            .lasertag_surface_net_chunk_mesh_with_scratch(
                voxel_coord_from_ivector(chunk_key),
                volume.mesh_config,
                volume.generation,
                &mut worker_state.mesh_scratch,
            );
        if DEPTH_DEBUG_LOG_CHUNK_MESH_TIMING {
            let elapsed = started.elapsed().as_secs_f32() * 1000.0;
            let triangles = mesh
                .as_ref()
                .map(|mesh| mesh.indices.len() / 3)
                .unwrap_or(0);
            crate::log!(
                "OpenXR depth meshed chunk ({}, {}, {}) in {:.2}ms tri={} pending={}",
                chunk_key.x,
                chunk_key.y,
                chunk_key.z,
                elapsed,
                triangles,
                volume.pending_mesh_chunk_queue.len()
            );
        }
        let update = update_incremental_mesh_chunk(volume, chunk_key, mesh);
        if !mesh_changed && !matches!(update, MeshChunkUpdate::Unchanged) {
            volume.dirty_chunk_keys.clear();
            volume.removed_chunk_keys.clear();
        }
        match update {
            MeshChunkUpdate::Unchanged => {}
            MeshChunkUpdate::Upserted => {
                volume.record_dirty_chunk(chunk_key);
                mesh_changed = true;
            }
            MeshChunkUpdate::Removed => {
                volume.record_removed_chunk(chunk_key);
                mesh_changed = true;
            }
        }
    }

    if mesh_changed {
        volume.mesh_chunks.sort_by(|a, b| {
            (a.chunk_key.x, a.chunk_key.y, a.chunk_key.z).cmp(&(
                b.chunk_key.x,
                b.chunk_key.y,
                b.chunk_key.z,
            ))
        });
        volume.mesh_vertex_count = volume.mesh_chunks.iter().map(|c| c.vertices.len()).sum();
        volume.mesh_triangle_count = volume
            .mesh_chunks
            .iter()
            .map(XrDepthMeshChunk::triangle_count)
            .sum();
        volume.mesh_generation = volume.mesh_generation.saturating_add(1);
        volume.update_sequence = volume.update_sequence.saturating_add(1);
    }
    mesh_changed
}

enum MeshChunkUpdate {
    Unchanged,
    Upserted,
    Removed,
}

fn update_incremental_mesh_chunk(
    volume: &mut DepthMeshVolume,
    chunk_key: IVector,
    mesh: Option<SurfaceMesh32>,
) -> MeshChunkUpdate {
    let existing_index = volume
        .mesh_chunks
        .iter()
        .position(|chunk| chunk.chunk_key == chunk_key);
    let new_chunk = mesh
        .and_then(|mesh| depth_mesh_chunk_from_surface_mesh(chunk_key, volume.generation, mesh));
    match (existing_index, new_chunk) {
        (Some(index), Some(chunk)) => {
            volume.mesh_chunks[index] = chunk;
            MeshChunkUpdate::Upserted
        }
        (Some(index), None) => {
            volume.mesh_chunks.swap_remove(index);
            MeshChunkUpdate::Removed
        }
        (None, Some(chunk)) => {
            volume.mesh_chunks.push(chunk);
            MeshChunkUpdate::Upserted
        }
        (None, None) => MeshChunkUpdate::Unchanged,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum ExtractedPlaneGroup {
    HorizontalUp,
    HorizontalDown,
    Vertical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct ExtractedPlaneBucketKey {
    group: ExtractedPlaneGroup,
    normal_bin: i32,
    distance_bin: i32,
}

#[derive(Clone, Debug)]
struct ExtractedPlaneTriangle {
    area: f32,
    normal: Vec3f,
    centroid: Vec3f,
    vertices: [Vec3f; 3],
}

#[derive(Clone, Debug)]
struct ExtractedPlaneBucket {
    group: ExtractedPlaneGroup,
    triangles: Vec<ExtractedPlaneTriangle>,
}

#[derive(Clone, Debug)]
struct ExtractedPlanePatchCandidate {
    patch: XrDepthPlanePatch,
    support_triangles_world: Vec<[Vec3f; 3]>,
}

#[derive(Clone, Debug)]
struct TrackedPlanePatch {
    stable_id: u64,
    patch: XrDepthPlanePatch,
    hit_count: u32,
    miss_count: u32,
    support_mask: PlaneSupportMask,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct ExtractedPlaneVertexKey {
    x: i32,
    y: i32,
    z: i32,
}

#[derive(Clone, Copy, Debug)]
struct OrientedRect2 {
    center: Vec2f,
    axis_u: Vec2f,
    axis_v: Vec2f,
    half_u: f32,
    half_v: f32,
}

#[derive(Clone, Debug, Default)]
struct PlaneSupportMask {
    cells: HashMap<PlaneSupportCellKey, u8>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
struct PlaneSupportCellKey {
    u: i32,
    v: i32,
}

#[derive(Clone, Debug, Default)]
struct PlaneSupportComponent {
    cells: Vec<PlaneSupportCellKey>,
    min_u: i32,
    max_u: i32,
    min_v: i32,
    max_v: i32,
    total_weight: u32,
    centroid_u: f32,
    centroid_v: f32,
    contains_anchor: bool,
}

fn maybe_rebuild_plane_patches(volume: &mut DepthMeshVolume, mesh_changed: bool) -> bool {
    if volume.mesh_chunks.is_empty() {
        if volume.plane_patches.is_empty() && volume.tracked_plane_patches.is_empty() {
            return false;
        }
        volume.plane_patches.clear();
        volume.tracked_plane_patches.clear();
        volume.plane_generation = volume.plane_generation.saturating_add(1);
        volume.update_sequence = volume.update_sequence.saturating_add(1);
        volume.last_plane_rebuild_at = Instant::now();
        return true;
    }

    let now = Instant::now();
    let due = volume.plane_patches.is_empty()
        || now.duration_since(volume.last_plane_rebuild_at)
            >= Duration::from_millis(DEPTH_PLANE_REBUILD_INTERVAL_MILLIS);
    if !mesh_changed || !due {
        return false;
    }

    let plane_patches = rebuild_plane_patches_from_mesh(&volume.mesh_chunks);
    let next_generation = volume.plane_generation.saturating_add(1);
    volume.tracked_plane_patches = track_plane_patches(
        core::mem::take(&mut volume.tracked_plane_patches),
        plane_patches,
        &mut volume.next_plane_stable_id,
    );
    volume.plane_patches = volume
        .tracked_plane_patches
        .iter()
        .filter(|tracked| tracked.hit_count >= DEPTH_PLANE_TRACK_STABLE_HITS || tracked.miss_count == 0)
        .map(|tracked| {
            let mut patch = tracked.patch.clone();
            patch.generation = next_generation;
            patch
        })
        .collect();
    volume.plane_generation = next_generation;
    volume.update_sequence = volume.update_sequence.saturating_add(1);
    volume.last_plane_rebuild_at = now;
    true
}

fn rebuild_plane_patches_from_mesh(mesh_chunks: &[XrDepthMeshChunk]) -> Vec<ExtractedPlanePatchCandidate> {
    let mut buckets = HashMap::<ExtractedPlaneBucketKey, ExtractedPlaneBucket>::new();

    for chunk in mesh_chunks {
        for triangle in chunk.indices.chunks_exact(3) {
            let a = chunk.vertices[triangle[0] as usize];
            let b = chunk.vertices[triangle[1] as usize];
            let c = chunk.vertices[triangle[2] as usize];
            let normal_area = Vec3f::cross(b - a, c - a);
            let area_twice = normal_area.length();
            if area_twice <= 1.0e-5 {
                continue;
            }
            let area = area_twice * 0.5;
            if area <= 0.0025 {
                continue;
            }
            let normal = normal_area.scale(1.0 / area_twice);
            let centroid = (a + b + c).scale(1.0 / 3.0);

            let classified = if normal.y >= DEPTH_PLANE_HORIZONTAL_NORMAL_Y_MIN {
                Some((
                    ExtractedPlaneBucketKey {
                        group: ExtractedPlaneGroup::HorizontalUp,
                        normal_bin: 0,
                        distance_bin: (centroid.y / DEPTH_PLANE_DISTANCE_BIN_METERS).round() as i32,
                    },
                    ExtractedPlaneGroup::HorizontalUp,
                ))
            } else if normal.y <= -DEPTH_PLANE_HORIZONTAL_NORMAL_Y_MIN {
                Some((
                    ExtractedPlaneBucketKey {
                        group: ExtractedPlaneGroup::HorizontalDown,
                        normal_bin: 0,
                        distance_bin: (centroid.y / DEPTH_PLANE_DISTANCE_BIN_METERS).round() as i32,
                    },
                    ExtractedPlaneGroup::HorizontalDown,
                ))
            } else if normal.y.abs() <= DEPTH_PLANE_VERTICAL_NORMAL_Y_MAX {
                let horizontal = vec3f(normal.x, 0.0, normal.z);
                let horizontal_len = horizontal.length();
                if horizontal_len <= 1.0e-5 {
                    None
                } else {
                    let yaw = horizontal.z.atan2(horizontal.x);
                    let yaw_step = std::f32::consts::TAU / DEPTH_PLANE_WALL_YAW_BINS as f32;
                    let yaw_bin = (yaw / yaw_step).round() as i32;
                    let snapped_yaw = yaw_bin as f32 * yaw_step;
                    let bucket_normal = vec3f(snapped_yaw.cos(), 0.0, snapped_yaw.sin());
                    let distance = centroid.dot(bucket_normal);
                    Some((
                        ExtractedPlaneBucketKey {
                            group: ExtractedPlaneGroup::Vertical,
                            normal_bin: yaw_bin,
                            distance_bin: (distance / DEPTH_PLANE_DISTANCE_BIN_METERS).round()
                                as i32,
                        },
                        ExtractedPlaneGroup::Vertical,
                    ))
                }
            } else {
                None
            };

            let Some((key, group)) = classified else {
                continue;
            };

            let bucket = buckets.entry(key).or_insert_with(|| ExtractedPlaneBucket {
                group,
                triangles: Vec::new(),
            });
            bucket.triangles.push(ExtractedPlaneTriangle {
                area,
                normal,
                centroid,
                vertices: [a, b, c],
            });
        }
    }

    let mut patches = Vec::new();
    for bucket in buckets.into_values() {
        patches.extend(extract_plane_patches_from_bucket(bucket));
    }
    classify_plane_patch_kinds(&mut patches);
    patches.sort_by(|a, b| {
        b.patch
            .area
            .partial_cmp(&a.patch.area)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    patches.truncate(DEPTH_PLANE_MAX_PATCHES);
    patches
}

fn extract_plane_patches_from_bucket(bucket: ExtractedPlaneBucket) -> Vec<ExtractedPlanePatchCandidate> {
    let mut vertex_links = HashMap::<ExtractedPlaneVertexKey, Vec<usize>>::new();
    for (index, triangle) in bucket.triangles.iter().enumerate() {
        for &vertex in &triangle.vertices {
            vertex_links
                .entry(quantize_plane_vertex(vertex))
                .or_default()
                .push(index);
        }
    }

    let mut visited = vec![false; bucket.triangles.len()];
    let mut result = Vec::new();

    for start_index in 0..bucket.triangles.len() {
        if visited[start_index] {
            continue;
        }

        let mut stack = vec![start_index];
        let mut region_triangle_indices = Vec::new();
        let seed_vertical_normal = bucket.triangles[start_index].normal;
        while let Some(triangle_index) = stack.pop() {
            if visited[triangle_index] {
                continue;
            }
            let triangle = &bucket.triangles[triangle_index];
            if bucket.group == ExtractedPlaneGroup::Vertical {
                let current_vertical_normal = align_direction(seed_vertical_normal, triangle.normal);
                if seed_vertical_normal.length() > 1.0e-5
                    && current_vertical_normal.length() > 1.0e-5
                    && seed_vertical_normal
                        .normalize()
                        .dot(current_vertical_normal.normalize())
                        < DEPTH_PLANE_REGION_VERTICAL_NORMAL_DOT
                {
                    continue;
                }
            }

            visited[triangle_index] = true;
            region_triangle_indices.push(triangle_index);
            for &vertex in &triangle.vertices {
                if let Some(neighbors) = vertex_links.get(&quantize_plane_vertex(vertex)) {
                    for &neighbor_index in neighbors {
                        if !visited[neighbor_index] {
                            stack.push(neighbor_index);
                        }
                    }
                }
            }
        }

        if region_triangle_indices.is_empty() {
            continue;
        }

        if let Some(patch) =
            fit_plane_patch_from_region(bucket.group, &bucket.triangles, &region_triangle_indices)
        {
            let kind = match bucket.group {
                ExtractedPlaneGroup::HorizontalUp => XrDepthPlaneKind::Unknown,
                ExtractedPlaneGroup::HorizontalDown => XrDepthPlaneKind::Ceiling,
                ExtractedPlaneGroup::Vertical => XrDepthPlaneKind::Wall,
            };
            result.push(ExtractedPlanePatchCandidate {
                patch: XrDepthPlanePatch {
                    generation: 0,
                    kind,
                    center: patch.center,
                    normal: patch.normal,
                    tangent: patch.tangent,
                    bitangent: patch.bitangent,
                    half_extent_tangent: patch.half_extent_tangent,
                    half_extent_bitangent: patch.half_extent_bitangent,
                    area: patch.area,
                    support_triangles: patch.support_triangles,
                },
                support_triangles_world: region_triangle_indices
                    .iter()
                    .map(|&triangle_index| bucket.triangles[triangle_index].vertices)
                    .collect(),
            });
        }
    }

    result
}

fn fit_plane_patch_from_region(
    group: ExtractedPlaneGroup,
    triangles: &[ExtractedPlaneTriangle],
    region: &[usize],
) -> Option<XrDepthPlanePatch> {
    match group {
        ExtractedPlaneGroup::HorizontalUp | ExtractedPlaneGroup::HorizontalDown => {
            fit_horizontal_plane_patch(group, triangles, region)
        }
        ExtractedPlaneGroup::Vertical => fit_vertical_plane_patch(triangles, region),
    }
}

fn fit_horizontal_plane_patch(
    group: ExtractedPlaneGroup,
    triangles: &[ExtractedPlaneTriangle],
    region: &[usize],
) -> Option<XrDepthPlanePatch> {
    let normal = match group {
        ExtractedPlaneGroup::HorizontalUp => vec3f(0.0, 1.0, 0.0),
        ExtractedPlaneGroup::HorizontalDown => vec3f(0.0, -1.0, 0.0),
        ExtractedPlaneGroup::Vertical => return None,
    };

    let mut area_sum = 0.0f32;
    let mut y_sum = 0.0f32;
    let mut points = Vec::with_capacity(region.len() * 3);
    for &triangle_index in region {
        let triangle = &triangles[triangle_index];
        area_sum += triangle.area;
        y_sum += triangle.centroid.y * triangle.area;
        for &vertex in &triangle.vertices {
            points.push(vec2f(vertex.x, vertex.z));
        }
    }
    if area_sum < DEPTH_PLANE_MIN_AREA_METERS2 || points.len() < 3 {
        return None;
    }

    let rect = fit_min_area_rect_2d(&points)?;
    let width = rect.half_u * 2.0;
    let height = rect.half_v * 2.0;
    if width < DEPTH_PLANE_MIN_DIM_METERS || height < DEPTH_PLANE_MIN_DIM_METERS {
        return None;
    }

    let mut tangent = vec3f(rect.axis_u.x, 0.0, rect.axis_u.y).normalize();
    let mut bitangent = vec3f(rect.axis_v.x, 0.0, rect.axis_v.y).normalize();
    if tangent.length() <= 1.0e-5 || bitangent.length() <= 1.0e-5 {
        tangent = vec3f(1.0, 0.0, 0.0);
        bitangent = vec3f(0.0, 0.0, 1.0);
    }
    if Vec3f::cross(tangent, bitangent).dot(normal) < 0.0 {
        bitangent = bitangent.scale(-1.0);
    }

    Some(XrDepthPlanePatch {
        generation: 0,
        kind: XrDepthPlaneKind::Unknown,
        center: vec3f(rect.center.x, y_sum / area_sum.max(f32::EPSILON), rect.center.y),
        normal,
        tangent,
        bitangent,
        half_extent_tangent: rect.half_u,
        half_extent_bitangent: rect.half_v,
        area: width * height,
        support_triangles: region.len(),
    })
}

fn fit_vertical_plane_patch(
    triangles: &[ExtractedPlaneTriangle],
    region: &[usize],
) -> Option<XrDepthPlanePatch> {
    let mut area_sum = 0.0f32;
    let mut normal_sum = Vec3f::default();
    let mut seed_normal = None::<Vec3f>;
    for &triangle_index in region {
        let triangle = &triangles[triangle_index];
        let aligned_normal = if let Some(seed_normal) = seed_normal {
            align_direction(seed_normal, triangle.normal)
        } else {
            seed_normal = Some(triangle.normal);
            triangle.normal
        };
        if aligned_normal.length() > 1.0e-5 {
            normal_sum += aligned_normal.normalize().scale(triangle.area);
            area_sum += triangle.area;
        }
    }
    if area_sum < DEPTH_PLANE_MIN_AREA_METERS2 || normal_sum.length() <= 1.0e-5 {
        return None;
    }

    let normal = normal_sum.normalize();
    let mut tangent = (vec3f(0.0, 1.0, 0.0) - normal.scale(normal.y)).normalize();
    if tangent.length() <= 1.0e-5 {
        tangent = orthogonal_tangent(normal);
    }
    let bitangent = Vec3f::cross(normal, tangent).normalize();
    if bitangent.length() <= 1.0e-5 {
        return None;
    }

    let mut distance_sum = 0.0f32;
    let mut min_u = f32::INFINITY;
    let mut max_u = f32::NEG_INFINITY;
    let mut min_v = f32::INFINITY;
    let mut max_v = f32::NEG_INFINITY;
    for &triangle_index in region {
        let triangle = &triangles[triangle_index];
        distance_sum += triangle.centroid.dot(normal) * triangle.area;
        for &vertex in &triangle.vertices {
            let u = vertex.dot(bitangent);
            let v = vertex.y;
            min_u = min_u.min(u);
            max_u = max_u.max(u);
            min_v = min_v.min(v);
            max_v = max_v.max(v);
        }
    }

    let width = max_u - min_u;
    let height = max_v - min_v;
    if width < DEPTH_PLANE_MIN_DIM_METERS || height < DEPTH_PLANE_MIN_DIM_METERS {
        return None;
    }

    let center_u = (min_u + max_u) * 0.5;
    let center_v = (min_v + max_v) * 0.5;
    let distance = distance_sum / area_sum.max(f32::EPSILON);
    Some(XrDepthPlanePatch {
        generation: 0,
        kind: XrDepthPlaneKind::Wall,
        center: normal.scale(distance) + bitangent.scale(center_u) + tangent.scale(center_v),
        normal,
        tangent,
        bitangent,
        half_extent_tangent: height * 0.5,
        half_extent_bitangent: width * 0.5,
        area: width * height,
        support_triangles: region.len(),
    })
}

fn quantize_plane_vertex(vertex: Vec3f) -> ExtractedPlaneVertexKey {
    let inv = 1.0 / DEPTH_PLANE_VERTEX_LINK_METERS.max(1.0e-5);
    ExtractedPlaneVertexKey {
        x: (vertex.x * inv).round() as i32,
        y: (vertex.y * inv).round() as i32,
        z: (vertex.z * inv).round() as i32,
    }
}

fn fit_min_area_rect_2d(points: &[Vec2f]) -> Option<OrientedRect2> {
    if points.is_empty() {
        return None;
    }

    let hull = convex_hull_2d(points);
    let points = if hull.is_empty() { points } else { &hull };
    if points.len() == 1 {
        return Some(OrientedRect2 {
            center: points[0],
            axis_u: vec2f(1.0, 0.0),
            axis_v: vec2f(0.0, 1.0),
            half_u: 0.0,
            half_v: 0.0,
        });
    }

    let mut best_rect = None::<OrientedRect2>;
    let mut best_area = f32::INFINITY;
    for edge_index in 0..points.len() {
        let a = points[edge_index];
        let b = points[(edge_index + 1) % points.len()];
        let edge = b - a;
        let edge_length = edge.length();
        if edge_length <= 1.0e-5 {
            continue;
        }
        let axis_u = edge / edge_length;
        let axis_v = vec2f(-axis_u.y, axis_u.x);
        let mut min_u = f32::INFINITY;
        let mut max_u = f32::NEG_INFINITY;
        let mut min_v = f32::INFINITY;
        let mut max_v = f32::NEG_INFINITY;
        for &point in points {
            let u = dot2(point, axis_u);
            let v = dot2(point, axis_v);
            min_u = min_u.min(u);
            max_u = max_u.max(u);
            min_v = min_v.min(v);
            max_v = max_v.max(v);
        }
        let width = max_u - min_u;
        let height = max_v - min_v;
        let area = width * height;
        if area >= best_area {
            continue;
        }
        best_area = area;
        best_rect = Some(OrientedRect2 {
            center: axis_u * ((min_u + max_u) * 0.5) + axis_v * ((min_v + max_v) * 0.5),
            axis_u,
            axis_v,
            half_u: width * 0.5,
            half_v: height * 0.5,
        });
    }
    best_rect
}

fn convex_hull_2d(points: &[Vec2f]) -> Vec<Vec2f> {
    if points.len() <= 3 {
        return points.to_vec();
    }

    let mut sorted = points.to_vec();
    sorted.sort_by(|a, b| {
        a.x.partial_cmp(&b.x)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.y.partial_cmp(&b.y).unwrap_or(std::cmp::Ordering::Equal))
    });
    sorted.dedup_by(|a, b| (a.x - b.x).abs() <= 1.0e-4 && (a.y - b.y).abs() <= 1.0e-4);
    if sorted.len() <= 3 {
        return sorted;
    }

    let mut lower = Vec::with_capacity(sorted.len());
    for &point in &sorted {
        while lower.len() >= 2
            && orient2(lower[lower.len() - 2], lower[lower.len() - 1], point) <= 0.0
        {
            lower.pop();
        }
        lower.push(point);
    }

    let mut upper = Vec::with_capacity(sorted.len());
    for &point in sorted.iter().rev() {
        while upper.len() >= 2
            && orient2(upper[upper.len() - 2], upper[upper.len() - 1], point) <= 0.0
        {
            upper.pop();
        }
        upper.push(point);
    }

    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

fn dot2(a: Vec2f, b: Vec2f) -> f32 {
    a.x * b.x + a.y * b.y
}

fn cross2(a: Vec2f, b: Vec2f) -> f32 {
    a.x * b.y - a.y * b.x
}

fn orient2(a: Vec2f, b: Vec2f, c: Vec2f) -> f32 {
    cross2(b - a, c - a)
}

fn classify_plane_patch_kinds(patches: &mut [ExtractedPlanePatchCandidate]) {
    let mut floor_y = None;
    let mut floor_area = 0.0f32;
    let mut min_up_y = f32::INFINITY;
    for patch in patches.iter() {
        if patch.patch.normal.y > DEPTH_PLANE_HORIZONTAL_NORMAL_Y_MIN {
            min_up_y = min_up_y.min(patch.patch.center.y);
        }
    }
    for patch in patches.iter() {
        if patch.patch.normal.y > DEPTH_PLANE_HORIZONTAL_NORMAL_Y_MIN
            && patch.patch.center.y <= min_up_y + 0.25
            && patch.patch.area > floor_area
        {
            floor_area = patch.patch.area;
            floor_y = Some(patch.patch.center.y);
        }
    }

    for patch in patches.iter_mut() {
        if patch.patch.normal.y > DEPTH_PLANE_HORIZONTAL_NORMAL_Y_MIN {
            patch.patch.kind = if floor_y
                .map(|y| patch.patch.center.y <= y + 0.18)
                .unwrap_or(false)
            {
                XrDepthPlaneKind::Floor
            } else {
                XrDepthPlaneKind::Table
            };
        } else if patch.patch.normal.y < -DEPTH_PLANE_HORIZONTAL_NORMAL_Y_MIN {
            patch.patch.kind = XrDepthPlaneKind::Ceiling;
        } else if patch.patch.normal.y.abs() <= DEPTH_PLANE_VERTICAL_NORMAL_Y_MAX {
            patch.patch.kind = XrDepthPlaneKind::Wall;
        } else {
            patch.patch.kind = XrDepthPlaneKind::Unknown;
        }
    }
}

fn track_plane_patches(
    existing: Vec<TrackedPlanePatch>,
    incoming: Vec<ExtractedPlanePatchCandidate>,
    next_plane_stable_id: &mut u64,
) -> Vec<TrackedPlanePatch> {
    let mut horizontal_incoming = Vec::new();
    let mut wall_incoming = Vec::new();
    for patch in incoming {
        if patch.patch.kind == XrDepthPlaneKind::Wall {
            wall_incoming.push(patch);
        } else {
            horizontal_incoming.push(patch);
        }
    }
    let mut incoming = horizontal_incoming;
    incoming.extend(merge_coplanar_patches(wall_incoming));

    let mut matched_existing = vec![false; existing.len()];
    let mut tracked = Vec::with_capacity(existing.len().max(incoming.len()));
    let mut grouped_incoming = vec![Vec::<ExtractedPlanePatchCandidate>::new(); existing.len()];
    let mut unmatched_incoming = Vec::new();

    for patch in incoming {
        let mut best_index = None;
        let mut best_score = f32::INFINITY;
        for (index, current) in existing.iter().enumerate() {
            let Some(score) = plane_patch_match_score(&current.patch, &patch.patch) else {
                continue;
            };
            if score < best_score {
                best_score = score;
                best_index = Some(index);
            }
        }

        if let Some(index) = best_index {
            matched_existing[index] = true;
            grouped_incoming[index].push(patch);
        } else {
            unmatched_incoming.push(patch);
        }
    }

    for (index, current) in existing.into_iter().enumerate() {
        if matched_existing[index] {
            tracked.push(update_tracked_plane_patch(&current, &grouped_incoming[index]));
            continue;
        }
        let miss_count = current.miss_count.saturating_add(1);
        let max_misses = if is_horizontal_plane_kind(current.patch.kind)
            && current.hit_count >= DEPTH_PLANE_TRACK_STABLE_HITS
        {
            DEPTH_PLANE_TRACK_STABLE_HORIZONTAL_MAX_MISSES
        } else {
            DEPTH_PLANE_TRACK_MAX_MISSES
        };
        if miss_count > max_misses {
            continue;
        }
        if current.hit_count < DEPTH_PLANE_TRACK_STABLE_HITS {
            continue;
        }
        tracked.push(TrackedPlanePatch {
            stable_id: current.stable_id,
            patch: current.patch,
            hit_count: current.hit_count,
            miss_count,
            support_mask: current.support_mask,
        });
    }

    for patch in unmatched_incoming {
        tracked.push(create_tracked_plane_patch(*next_plane_stable_id, patch));
        *next_plane_stable_id = next_plane_stable_id.saturating_add(1);
    }

    tracked.sort_by(|a, b| {
        b.patch
            .area
            .partial_cmp(&a.patch.area)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.stable_id.cmp(&b.stable_id))
    });
    tracked.truncate(DEPTH_PLANE_MAX_PATCHES.saturating_mul(2));
    tracked
}

fn create_tracked_plane_patch(
    stable_id: u64,
    incoming: ExtractedPlanePatchCandidate,
) -> TrackedPlanePatch {
    let mut support_mask = PlaneSupportMask::default();
    rasterize_support_triangles_into_mask(
        &mut support_mask,
        &incoming.support_triangles_world,
        incoming.patch.tangent,
        incoming.patch.bitangent,
    );
    let center_u = incoming.patch.center.dot(incoming.patch.tangent);
    let center_v = incoming.patch.center.dot(incoming.patch.bitangent);
    let anchor = quantize_plane_support_cell(center_u, center_v);
    let patch = select_plane_support_component(&support_mask, anchor, center_u, center_v)
        .and_then(|component| {
            retain_plane_support_component(&mut support_mask, &component);
            fit_patch_from_support_component(
                incoming.patch.kind,
                incoming.patch.normal,
                incoming.patch.tangent,
                incoming.patch.bitangent,
                incoming.patch.center.dot(incoming.patch.normal),
                &component,
                incoming.patch.support_triangles,
            )
        })
        .unwrap_or(incoming.patch);
    TrackedPlanePatch {
        stable_id,
        patch,
        hit_count: 1,
        miss_count: 0,
        support_mask,
    }
}

fn update_tracked_plane_patch(
    current: &TrackedPlanePatch,
    incoming_group: &[ExtractedPlanePatchCandidate],
) -> TrackedPlanePatch {
    if incoming_group.is_empty() {
        return current.clone();
    }

    let (normal, tangent, bitangent) = tracked_plane_basis(current, incoming_group);
    let mut support_mask = current.support_mask.clone();
    if current.patch.kind == XrDepthPlaneKind::Wall
        || current.hit_count < DEPTH_PLANE_TRACK_STABLE_HITS
    {
        decay_plane_support_mask(&mut support_mask);
    }

    let mut support_triangles = current.patch.support_triangles;
    let mut kind = current.patch.kind;
    let mut fallback = None::<XrDepthPlanePatch>;

    for incoming in incoming_group {
        let reframed = reframe_patch_onto_basis(&incoming.patch, normal, tangent, bitangent);
        support_triangles = support_triangles.max(incoming.patch.support_triangles);
        kind = choose_plane_kind(kind, incoming.patch.kind);
        rasterize_support_triangles_into_mask(
            &mut support_mask,
            &incoming.support_triangles_world,
            tangent,
            bitangent,
        );
        fallback = Some(match fallback {
            Some(existing) => {
                if kind == XrDepthPlaneKind::Wall {
                    merge_vertical_patch_pair(&existing, &reframed)
                } else {
                    merge_reframed_horizontal_patches(
                        &existing,
                        &reframed,
                        normal,
                        tangent,
                        bitangent,
                    )
                }
            }
            None => reframed,
        });
    }

    let current_center_u = current.patch.center.dot(tangent);
    let current_center_v = current.patch.center.dot(bitangent);
    let anchor = quantize_plane_support_cell(current_center_u, current_center_v);
    let next_patch = if let Some(component) = select_plane_support_component(
        &support_mask,
        anchor,
        current_center_u,
        current_center_v,
    ) {
        let plane_distance = plane_distance_for_component(
            current,
            incoming_group,
            normal,
            tangent,
            bitangent,
            &component,
        );
        retain_plane_support_component(&mut support_mask, &component);
        fit_patch_from_support_component(
            kind,
            normal,
            tangent,
            bitangent,
            plane_distance,
            &component,
            support_triangles,
        )
        .or(fallback)
        .unwrap_or_else(|| current.patch.clone())
    } else {
        fallback.unwrap_or_else(|| current.patch.clone())
    };

    TrackedPlanePatch {
        stable_id: current.stable_id,
        patch: blend_tracked_plane_patch(current, &next_patch),
        hit_count: current.hit_count.saturating_add(1),
        miss_count: 0,
        support_mask,
    }
}

fn tracked_plane_basis(
    current: &TrackedPlanePatch,
    incoming_group: &[ExtractedPlanePatchCandidate],
) -> (Vec3f, Vec3f, Vec3f) {
    if current.patch.kind != XrDepthPlaneKind::Wall {
        return (
            current.patch.normal,
            current.patch.tangent,
            current.patch.bitangent,
        );
    }

    let mut normal_sum = current
        .patch
        .normal
        .scale(current.patch.area.max(0.001));
    for incoming in incoming_group {
        let aligned = align_direction(current.patch.normal, incoming.patch.normal);
        normal_sum += aligned.scale(incoming.patch.area.max(0.001));
    }
    let normal = if normal_sum.length() > 1.0e-5 {
        normal_sum.normalize()
    } else {
        current.patch.normal
    };
    let mut tangent = current.patch.tangent - normal.scale(current.patch.tangent.dot(normal));
    if tangent.length() <= 1.0e-5 {
        tangent = (vec3f(0.0, 1.0, 0.0) - normal.scale(normal.y)).normalize();
    } else {
        tangent = tangent.normalize();
    }
    if tangent.length() <= 1.0e-5 {
        tangent = orthogonal_tangent(normal);
    }
    let mut bitangent = Vec3f::cross(normal, tangent).normalize();
    if bitangent.length() <= 1.0e-5 {
        bitangent = current.patch.bitangent;
    }
    (normal, tangent, bitangent)
}

fn decay_plane_support_mask(mask: &mut PlaneSupportMask) {
    mask.cells.retain(|_, weight| {
        *weight = weight.saturating_sub(DEPTH_PLANE_SUPPORT_DECAY_WEIGHT);
        *weight > 0
    });
}

fn rasterize_support_triangles_into_mask(
    mask: &mut PlaneSupportMask,
    triangles_world: &[[Vec3f; 3]],
    tangent: Vec3f,
    bitangent: Vec3f,
) {
    for triangle in triangles_world {
        let a = vec2f(triangle[0].dot(tangent), triangle[0].dot(bitangent));
        let b = vec2f(triangle[1].dot(tangent), triangle[1].dot(bitangent));
        let c = vec2f(triangle[2].dot(tangent), triangle[2].dot(bitangent));
        rasterize_triangle_2d_into_support_mask(mask, a, b, c);
    }
}

fn rasterize_triangle_2d_into_support_mask(
    mask: &mut PlaneSupportMask,
    a: Vec2f,
    b: Vec2f,
    c: Vec2f,
) {
    let min_u = a.x.min(b.x).min(c.x);
    let max_u = a.x.max(b.x).max(c.x);
    let min_v = a.y.min(b.y).min(c.y);
    let max_v = a.y.max(b.y).max(c.y);
    let min_cell = quantize_plane_support_cell(min_u, min_v);
    let max_cell = quantize_plane_support_cell(max_u - 1.0e-4, max_v - 1.0e-4);
    let cell_radius = DEPTH_PLANE_SUPPORT_CELL_METERS * std::f32::consts::SQRT_2 * 0.5;
    for u in min_cell.u..=max_cell.u {
        for v in min_cell.v..=max_cell.v {
            let center = vec2f(
                (u as f32 + 0.5) * DEPTH_PLANE_SUPPORT_CELL_METERS,
                (v as f32 + 0.5) * DEPTH_PLANE_SUPPORT_CELL_METERS,
            );
            if !point_in_triangle_2d(center, a, b, c)
                && point_segment_distance_2d(center, a, b) > cell_radius
                && point_segment_distance_2d(center, b, c) > cell_radius
                && point_segment_distance_2d(center, c, a) > cell_radius
            {
                continue;
            }
            let weight = mask
                .cells
                .entry(PlaneSupportCellKey { u, v })
                .or_insert(0);
            *weight = weight
                .saturating_add(DEPTH_PLANE_SUPPORT_GROW_WEIGHT)
                .min(DEPTH_PLANE_SUPPORT_MAX_WEIGHT);
        }
    }
}

fn quantize_plane_support_cell(u: f32, v: f32) -> PlaneSupportCellKey {
    let inv = 1.0 / DEPTH_PLANE_SUPPORT_CELL_METERS.max(1.0e-5);
    PlaneSupportCellKey {
        u: (u * inv).floor() as i32,
        v: (v * inv).floor() as i32,
    }
}

fn select_plane_support_component(
    mask: &PlaneSupportMask,
    anchor: PlaneSupportCellKey,
    anchor_u: f32,
    anchor_v: f32,
) -> Option<PlaneSupportComponent> {
    let occupied: HashMap<PlaneSupportCellKey, u8> = mask
        .cells
        .iter()
        .filter_map(|(&key, &weight)| {
            (weight >= DEPTH_PLANE_SUPPORT_OCCUPIED_WEIGHT).then_some((key, weight))
        })
        .collect();
    if occupied.is_empty() {
        return None;
    }

    let mut visited = HashSet::new();
    let mut best = None::<PlaneSupportComponent>;
    let mut best_anchor = false;
    let mut best_weight = 0u32;
    let mut best_distance = f32::INFINITY;

    for &start in occupied.keys() {
        if !visited.insert(start) {
            continue;
        }

        let mut queue = VecDeque::from([start]);
        let mut component = PlaneSupportComponent {
            min_u: start.u,
            max_u: start.u,
            min_v: start.v,
            max_v: start.v,
            ..PlaneSupportComponent::default()
        };
        let mut weighted_u_sum = 0.0f32;
        let mut weighted_v_sum = 0.0f32;

        while let Some(cell) = queue.pop_front() {
            component.cells.push(cell);
            component.min_u = component.min_u.min(cell.u);
            component.max_u = component.max_u.max(cell.u);
            component.min_v = component.min_v.min(cell.v);
            component.max_v = component.max_v.max(cell.v);
            if cell == anchor {
                component.contains_anchor = true;
            }
            let weight = *occupied.get(&cell).unwrap_or(&0) as u32;
            component.total_weight += weight;
            weighted_u_sum += (cell.u as f32 + 0.5) * DEPTH_PLANE_SUPPORT_CELL_METERS * weight as f32;
            weighted_v_sum += (cell.v as f32 + 0.5) * DEPTH_PLANE_SUPPORT_CELL_METERS * weight as f32;

            for du in -1..=1 {
                for dv in -1..=1 {
                    if du == 0 && dv == 0 {
                        continue;
                    }
                    let neighbor = PlaneSupportCellKey {
                        u: cell.u + du,
                        v: cell.v + dv,
                    };
                    if occupied.contains_key(&neighbor) && visited.insert(neighbor) {
                        queue.push_back(neighbor);
                    }
                }
            }
        }

        if component.total_weight == 0 {
            continue;
        }
        component.centroid_u = weighted_u_sum / component.total_weight as f32;
        component.centroid_v = weighted_v_sum / component.total_weight as f32;

        let anchor_distance = ((component.centroid_u - anchor_u).powi(2)
            + (component.centroid_v - anchor_v).powi(2))
        .sqrt();
        if component.contains_anchor && !best_anchor
            || component.contains_anchor == best_anchor
                && (component.total_weight > best_weight
                    || component.total_weight == best_weight && anchor_distance < best_distance)
        {
            best_anchor = component.contains_anchor;
            best_weight = component.total_weight;
            best_distance = anchor_distance;
            best = Some(component);
        }
    }

    best
}

fn retain_plane_support_component(mask: &mut PlaneSupportMask, component: &PlaneSupportComponent) {
    let mut keep = HashSet::new();
    for &cell in &component.cells {
        for du in -1..=1 {
            for dv in -1..=1 {
                keep.insert(PlaneSupportCellKey {
                    u: cell.u + du,
                    v: cell.v + dv,
                });
            }
        }
    }
    mask.cells.retain(|key, _| keep.contains(key));
}

fn plane_distance_for_component(
    current: &TrackedPlanePatch,
    incoming_group: &[ExtractedPlanePatchCandidate],
    normal: Vec3f,
    tangent: Vec3f,
    bitangent: Vec3f,
    component: &PlaneSupportComponent,
) -> f32 {
    let mut distance_sum = 0.0f32;
    let mut weight_sum = 0.0f32;
    for incoming in incoming_group {
        let reframed = reframe_patch_onto_basis(&incoming.patch, normal, tangent, bitangent);
        let cell = quantize_plane_support_cell(
            reframed.center.dot(tangent),
            reframed.center.dot(bitangent),
        );
        if cell.u < component.min_u
            || cell.u > component.max_u
            || cell.v < component.min_v
            || cell.v > component.max_v
        {
            continue;
        }
        let weight = reframed.area.max(0.001);
        distance_sum += reframed.center.dot(normal) * weight;
        weight_sum += weight;
    }
    if weight_sum > 0.0 {
        distance_sum / weight_sum
    } else {
        current.patch.center.dot(normal)
    }
}

fn fit_patch_from_support_component(
    kind: XrDepthPlaneKind,
    normal: Vec3f,
    tangent: Vec3f,
    bitangent: Vec3f,
    plane_distance: f32,
    component: &PlaneSupportComponent,
    support_triangles: usize,
) -> Option<XrDepthPlanePatch> {
    let (min_u_cell, max_u_cell, min_v_cell, max_v_cell) = if kind == XrDepthPlaneKind::Table {
        largest_supported_rectangle(component).unwrap_or((
            component.min_u,
            component.max_u,
            component.min_v,
            component.max_v,
        ))
    } else {
        (
            component.min_u,
            component.max_u,
            component.min_v,
            component.max_v,
        )
    };
    let min_u = min_u_cell as f32 * DEPTH_PLANE_SUPPORT_CELL_METERS;
    let max_u = (max_u_cell + 1) as f32 * DEPTH_PLANE_SUPPORT_CELL_METERS;
    let min_v = min_v_cell as f32 * DEPTH_PLANE_SUPPORT_CELL_METERS;
    let max_v = (max_v_cell + 1) as f32 * DEPTH_PLANE_SUPPORT_CELL_METERS;
    let width = max_u - min_u;
    let height = max_v - min_v;
    if width < DEPTH_PLANE_MIN_DIM_METERS || height < DEPTH_PLANE_MIN_DIM_METERS {
        return None;
    }

    let center_u = (min_u + max_u) * 0.5;
    let center_v = (min_v + max_v) * 0.5;
    Some(XrDepthPlanePatch {
        generation: 0,
        kind,
        center: normal.scale(plane_distance) + tangent.scale(center_u) + bitangent.scale(center_v),
        normal,
        tangent,
        bitangent,
        half_extent_tangent: width * 0.5,
        half_extent_bitangent: height * 0.5,
        area: width * height,
        support_triangles,
    })
}

fn largest_supported_rectangle(
    component: &PlaneSupportComponent,
) -> Option<(i32, i32, i32, i32)> {
    let width = (component.max_u - component.min_u + 1).max(0) as usize;
    let height = (component.max_v - component.min_v + 1).max(0) as usize;
    if width == 0 || height == 0 {
        return None;
    }

    let mut occupied = vec![false; width * height];
    for cell in &component.cells {
        let x = (cell.u - component.min_u) as usize;
        let y = (cell.v - component.min_v) as usize;
        occupied[y * width + x] = true;
    }

    let mut heights = vec![0usize; width];
    let mut best_area = 0usize;
    let mut best = None;
    for y in 0..height {
        for x in 0..width {
            if occupied[y * width + x] {
                heights[x] += 1;
            } else {
                heights[x] = 0;
            }
        }

        let mut stack = Vec::<usize>::new();
        for x in 0..=width {
            let current_height = if x < width { heights[x] } else { 0 };
            while let Some(&top) = stack.last() {
                if heights[top] <= current_height {
                    break;
                }
                let h = heights[top];
                stack.pop();
                let left = stack.last().map(|&idx| idx + 1).unwrap_or(0);
                let rect_width = x - left;
                let area = h * rect_width;
                if area > best_area && h > 0 && rect_width > 0 {
                    best_area = area;
                    best = Some((
                        component.min_u + left as i32,
                        component.min_u + x as i32 - 1,
                        component.min_v + y as i32 - h as i32 + 1,
                        component.min_v + y as i32,
                    ));
                }
            }
            stack.push(x);
        }
    }
    best
}

fn point_in_triangle_2d(p: Vec2f, a: Vec2f, b: Vec2f, c: Vec2f) -> bool {
    let ab = orient2(a, b, p);
    let bc = orient2(b, c, p);
    let ca = orient2(c, a, p);
    (ab >= 0.0 && bc >= 0.0 && ca >= 0.0) || (ab <= 0.0 && bc <= 0.0 && ca <= 0.0)
}

fn point_segment_distance_2d(p: Vec2f, a: Vec2f, b: Vec2f) -> f32 {
    let ab = b - a;
    let length2 = dot2(ab, ab);
    if length2 <= 1.0e-6 {
        return (p - a).length();
    }
    let t = (dot2(p - a, ab) / length2).clamp(0.0, 1.0);
    let projected = a + ab * t;
    (p - projected).length()
}

fn plane_patch_match_score(existing: &XrDepthPlanePatch, incoming: &XrDepthPlanePatch) -> Option<f32> {
    if !plane_kinds_match(existing.kind, incoming.kind) {
        return None;
    }

    let is_wall = existing.kind == XrDepthPlaneKind::Wall || incoming.kind == XrDepthPlaneKind::Wall;
    let min_normal_dot = if is_wall {
        DEPTH_PLANE_TRACK_WALL_MATCH_NORMAL_DOT
    } else {
        DEPTH_PLANE_TRACK_MATCH_NORMAL_DOT
    };
    let base_center_distance = if is_wall {
        DEPTH_PLANE_TRACK_WALL_MATCH_CENTER_DISTANCE_METERS
    } else {
        DEPTH_PLANE_TRACK_MATCH_CENTER_DISTANCE_METERS
    };
    let max_plane_distance = if is_wall {
        DEPTH_PLANE_TRACK_WALL_MATCH_PLANE_DISTANCE_METERS
    } else {
        DEPTH_PLANE_TRACK_MATCH_PLANE_DISTANCE_METERS
    };

    let dynamic_center_distance = existing
        .half_extent_tangent
        .max(existing.half_extent_bitangent)
        + incoming
            .half_extent_tangent
            .max(incoming.half_extent_bitangent)
        + 0.35;
    let max_center_distance = base_center_distance.max(dynamic_center_distance);

    let normal_dot = existing.normal.dot(incoming.normal);
    if normal_dot < min_normal_dot {
        return None;
    }

    let center_delta = incoming.center - existing.center;
    let center_distance = center_delta.length();
    let overlap_ratio = plane_patch_overlap_ratio(existing, incoming);
    if center_distance > max_center_distance && overlap_ratio < 0.05 {
        return None;
    }

    let plane_distance = center_delta.dot(existing.normal).abs();
    if plane_distance > max_plane_distance {
        return None;
    }

    let lateral_distance = (center_delta - existing.normal.scale(center_delta.dot(existing.normal)))
        .length();
    let lateral_limit = existing
        .half_extent_tangent
        .max(existing.half_extent_bitangent)
        .max(incoming.half_extent_tangent.max(incoming.half_extent_bitangent))
        + 0.25;
    if lateral_distance > lateral_limit {
        return None;
    }

    let extent_delta = (existing.half_extent_tangent - incoming.half_extent_tangent).abs()
        + (existing.half_extent_bitangent - incoming.half_extent_bitangent).abs();
    Some(
        plane_distance * 2.0
            + lateral_distance * 0.75
            + extent_delta * 0.25
            + (1.0 - overlap_ratio).max(0.0) * 0.2
            + (1.0 - normal_dot) * 2.0,
    )
}

fn plane_patch_overlap_ratio(existing: &XrDepthPlanePatch, incoming: &XrDepthPlanePatch) -> f32 {
    let reframed = reframe_patch_onto_basis(
        incoming,
        existing.normal,
        existing.tangent,
        existing.bitangent,
    );
    let existing_center_u = existing.center.dot(existing.tangent);
    let existing_center_v = existing.center.dot(existing.bitangent);
    let incoming_center_u = reframed.center.dot(existing.tangent);
    let incoming_center_v = reframed.center.dot(existing.bitangent);
    let overlap_u = overlap_extent(
        existing_center_u - existing.half_extent_tangent,
        existing_center_u + existing.half_extent_tangent,
        incoming_center_u - reframed.half_extent_tangent,
        incoming_center_u + reframed.half_extent_tangent,
    );
    let overlap_v = overlap_extent(
        existing_center_v - existing.half_extent_bitangent,
        existing_center_v + existing.half_extent_bitangent,
        incoming_center_v - reframed.half_extent_bitangent,
        incoming_center_v + reframed.half_extent_bitangent,
    );
    if overlap_u <= 0.0 || overlap_v <= 0.0 {
        return 0.0;
    }
    let overlap_area = overlap_u * overlap_v;
    let min_area = existing.area.min(reframed.area).max(1.0e-4);
    (overlap_area / min_area).clamp(0.0, 1.0)
}

fn overlap_extent(a0: f32, a1: f32, b0: f32, b1: f32) -> f32 {
    (a1.min(b1) - a0.max(b0)).max(0.0)
}

fn plane_kinds_match(a: XrDepthPlaneKind, b: XrDepthPlaneKind) -> bool {
    if a == b {
        return true;
    }
    matches!(
        (a, b),
        (XrDepthPlaneKind::Floor, XrDepthPlaneKind::Table)
            | (XrDepthPlaneKind::Table, XrDepthPlaneKind::Floor)
            | (XrDepthPlaneKind::Floor, XrDepthPlaneKind::Unknown)
            | (XrDepthPlaneKind::Unknown, XrDepthPlaneKind::Floor)
            | (XrDepthPlaneKind::Table, XrDepthPlaneKind::Unknown)
            | (XrDepthPlaneKind::Unknown, XrDepthPlaneKind::Table)
    )
}

fn merge_coplanar_patches(
    mut patches: Vec<ExtractedPlanePatchCandidate>,
) -> Vec<ExtractedPlanePatchCandidate> {
    let mut merged = true;
    while merged {
        merged = false;
        'outer: for i in 0..patches.len() {
            for j in (i + 1)..patches.len() {
                if !can_merge_plane_patches(&patches[i].patch, &patches[j].patch) {
                    continue;
                }
                let merged_patch =
                    merge_plane_patch_candidate_pair(&patches[i], &patches[j]);
                patches[i] = merged_patch;
                patches.swap_remove(j);
                merged = true;
                break 'outer;
            }
        }
    }
    patches
}

fn merge_plane_patch_candidate_pair(
    a: &ExtractedPlanePatchCandidate,
    b: &ExtractedPlanePatchCandidate,
) -> ExtractedPlanePatchCandidate {
    let mut support_triangles_world =
        Vec::with_capacity(a.support_triangles_world.len() + b.support_triangles_world.len());
    support_triangles_world.extend_from_slice(&a.support_triangles_world);
    support_triangles_world.extend_from_slice(&b.support_triangles_world);
    ExtractedPlanePatchCandidate {
        patch: merge_plane_patch_pair(&a.patch, &b.patch),
        support_triangles_world,
    }
}

fn can_merge_plane_patches(a: &XrDepthPlanePatch, b: &XrDepthPlanePatch) -> bool {
    if !plane_kinds_match(a.kind, b.kind) {
        return false;
    }

    let normal_dot = a.normal.dot(b.normal);
    let min_normal_dot = if a.kind == XrDepthPlaneKind::Wall || b.kind == XrDepthPlaneKind::Wall {
        DEPTH_PLANE_TRACK_WALL_MATCH_NORMAL_DOT
    } else {
        DEPTH_PLANE_TRACK_MATCH_NORMAL_DOT
    };
    if normal_dot < min_normal_dot {
        return false;
    }

    let center_delta = b.center - a.center;
    let plane_distance = center_delta.dot(a.normal).abs();
    if plane_distance
        > if a.kind == XrDepthPlaneKind::Wall || b.kind == XrDepthPlaneKind::Wall {
            DEPTH_PLANE_TRACK_WALL_MATCH_PLANE_DISTANCE_METERS
        } else {
            DEPTH_PLANE_TRACK_MATCH_PLANE_DISTANCE_METERS
        }
    {
        return false;
    }

    let lateral_distance = (center_delta - a.normal.scale(center_delta.dot(a.normal))).length();
    let lateral_limit = a
        .half_extent_tangent
        .max(a.half_extent_bitangent)
        + b.half_extent_tangent.max(b.half_extent_bitangent)
        + DEPTH_PLANE_PATCH_MERGE_GAP_METERS;
    lateral_distance <= lateral_limit
}

fn merge_plane_patch_pair(a: &XrDepthPlanePatch, b: &XrDepthPlanePatch) -> XrDepthPlanePatch {
    if a.kind == XrDepthPlaneKind::Wall || b.kind == XrDepthPlaneKind::Wall {
        merge_vertical_patch_pair(a, b)
    } else {
        let reference = if a.area >= b.area { a } else { b };
        let reframed_a = reframe_patch_onto_basis(a, reference.normal, reference.tangent, reference.bitangent);
        let reframed_b = reframe_patch_onto_basis(b, reference.normal, reference.tangent, reference.bitangent);
        merge_reframed_horizontal_patches(&reframed_a, &reframed_b, reference.normal, reference.tangent, reference.bitangent)
    }
}

fn reframe_patch_onto_basis(
    patch: &XrDepthPlanePatch,
    normal: Vec3f,
    tangent: Vec3f,
    bitangent: Vec3f,
) -> XrDepthPlanePatch {
    let corners = plane_patch_corners(patch);
    let mut min_u = f32::INFINITY;
    let mut max_u = f32::NEG_INFINITY;
    let mut min_v = f32::INFINITY;
    let mut max_v = f32::NEG_INFINITY;
    let mut plane_distance_sum = 0.0;
    for corner in corners {
        let u = corner.dot(tangent);
        let v = corner.dot(bitangent);
        min_u = min_u.min(u);
        max_u = max_u.max(u);
        min_v = min_v.min(v);
        max_v = max_v.max(v);
        plane_distance_sum += corner.dot(normal);
    }
    let center_u = (min_u + max_u) * 0.5;
    let center_v = (min_v + max_v) * 0.5;
    let plane_distance = plane_distance_sum * 0.25;
    XrDepthPlanePatch {
        generation: patch.generation,
        kind: patch.kind,
        center: normal.scale(plane_distance) + tangent.scale(center_u) + bitangent.scale(center_v),
        normal,
        tangent,
        bitangent,
        half_extent_tangent: (max_u - min_u) * 0.5,
        half_extent_bitangent: (max_v - min_v) * 0.5,
        area: (max_u - min_u) * (max_v - min_v),
        support_triangles: patch.support_triangles,
    }
}

fn merge_reframed_horizontal_patches(
    a: &XrDepthPlanePatch,
    b: &XrDepthPlanePatch,
    normal: Vec3f,
    tangent: Vec3f,
    bitangent: Vec3f,
) -> XrDepthPlanePatch {
    let mut min_u = f32::INFINITY;
    let mut max_u = f32::NEG_INFINITY;
    let mut min_v = f32::INFINITY;
    let mut max_v = f32::NEG_INFINITY;
    let mut y_sum = 0.0;
    let mut weight_sum = 0.0;
    for patch in [a, b] {
        let center_u = patch.center.dot(tangent);
        let center_v = patch.center.dot(bitangent);
        min_u = min_u.min(center_u - patch.half_extent_tangent);
        max_u = max_u.max(center_u + patch.half_extent_tangent);
        min_v = min_v.min(center_v - patch.half_extent_bitangent);
        max_v = max_v.max(center_v + patch.half_extent_bitangent);
        y_sum += patch.center.y * patch.area.max(0.001);
        weight_sum += patch.area.max(0.001);
    }
    let center_u = (min_u + max_u) * 0.5;
    let center_v = (min_v + max_v) * 0.5;
    let center_y = y_sum / weight_sum.max(f32::EPSILON);
    XrDepthPlanePatch {
        generation: a.generation.max(b.generation),
        kind: choose_plane_kind(a.kind, b.kind),
        center: vec3f(
            tangent.x * center_u + bitangent.x * center_v,
            center_y,
            tangent.z * center_u + bitangent.z * center_v,
        ),
        normal,
        tangent,
        bitangent,
        half_extent_tangent: (max_u - min_u) * 0.5,
        half_extent_bitangent: (max_v - min_v) * 0.5,
        area: (max_u - min_u) * (max_v - min_v),
        support_triangles: a.support_triangles + b.support_triangles,
    }
}

fn merge_vertical_patch_pair(a: &XrDepthPlanePatch, b: &XrDepthPlanePatch) -> XrDepthPlanePatch {
    let normal = blend_direction(a.normal, b.normal, 0.5);
    let mut tangent = (vec3f(0.0, 1.0, 0.0) - normal.scale(normal.y)).normalize();
    if tangent.length() <= 1.0e-5 {
        tangent = orthogonal_tangent(normal);
    }
    let bitangent = Vec3f::cross(normal, tangent).normalize();
    let reframed_a = reframe_patch_onto_basis(a, normal, tangent, bitangent);
    let reframed_b = reframe_patch_onto_basis(b, normal, tangent, bitangent);
    let mut min_u = f32::INFINITY;
    let mut max_u = f32::NEG_INFINITY;
    let mut min_v = f32::INFINITY;
    let mut max_v = f32::NEG_INFINITY;
    let mut plane_distance_sum = 0.0;
    let mut weight_sum = 0.0;
    for patch in [&reframed_a, &reframed_b] {
        let center_u = patch.center.dot(bitangent);
        let center_v = patch.center.dot(tangent);
        min_u = min_u.min(center_u - patch.half_extent_bitangent);
        max_u = max_u.max(center_u + patch.half_extent_bitangent);
        min_v = min_v.min(center_v - patch.half_extent_tangent);
        max_v = max_v.max(center_v + patch.half_extent_tangent);
        plane_distance_sum += patch.center.dot(normal) * patch.area.max(0.001);
        weight_sum += patch.area.max(0.001);
    }
    let center_u = (min_u + max_u) * 0.5;
    let center_v = (min_v + max_v) * 0.5;
    let plane_distance = plane_distance_sum / weight_sum.max(f32::EPSILON);
    XrDepthPlanePatch {
        generation: a.generation.max(b.generation),
        kind: XrDepthPlaneKind::Wall,
        center: normal.scale(plane_distance) + bitangent.scale(center_u) + tangent.scale(center_v),
        normal,
        tangent,
        bitangent,
        half_extent_tangent: (max_v - min_v) * 0.5,
        half_extent_bitangent: (max_u - min_u) * 0.5,
        area: (max_u - min_u) * (max_v - min_v),
        support_triangles: a.support_triangles + b.support_triangles,
    }
}

fn choose_plane_kind(a: XrDepthPlaneKind, b: XrDepthPlaneKind) -> XrDepthPlaneKind {
    match (a, b) {
        (XrDepthPlaneKind::Floor, _) | (_, XrDepthPlaneKind::Floor) => XrDepthPlaneKind::Floor,
        (XrDepthPlaneKind::Ceiling, _) | (_, XrDepthPlaneKind::Ceiling) => XrDepthPlaneKind::Ceiling,
        (XrDepthPlaneKind::Table, _) | (_, XrDepthPlaneKind::Table) => XrDepthPlaneKind::Table,
        (XrDepthPlaneKind::Wall, _) | (_, XrDepthPlaneKind::Wall) => XrDepthPlaneKind::Wall,
        _ => a,
    }
}

fn plane_patch_corners(patch: &XrDepthPlanePatch) -> [Vec3f; 4] {
    let du = patch.tangent.scale(patch.half_extent_tangent);
    let dv = patch.bitangent.scale(patch.half_extent_bitangent);
    [
        patch.center - du - dv,
        patch.center - du + dv,
        patch.center + du + dv,
        patch.center + du - dv,
    ]
}

fn blend_tracked_plane_patch(current: &TrackedPlanePatch, incoming: &XrDepthPlanePatch) -> XrDepthPlanePatch {
    if current.patch.kind != XrDepthPlaneKind::Wall && incoming.kind != XrDepthPlaneKind::Wall {
        return blend_horizontal_tracked_plane_patch(current, incoming);
    }

    let alpha = if current.patch.kind == XrDepthPlaneKind::Wall || incoming.kind == XrDepthPlaneKind::Wall {
        if current.hit_count < DEPTH_PLANE_TRACK_STABLE_HITS {
            0.28
        } else {
            0.12
        }
    } else {
        if current.hit_count < DEPTH_PLANE_TRACK_STABLE_HITS {
            0.45
        } else {
            0.22
        }
    };
    let one_minus_alpha = 1.0 - alpha;

    let normal = blend_direction(current.patch.normal, incoming.normal, alpha);
    let mut tangent = blend_direction(current.patch.tangent, incoming.tangent, alpha);
    tangent = (tangent - normal.scale(tangent.dot(normal))).normalize();
    if tangent.length() <= 1.0e-5 {
        tangent = orthogonal_tangent(normal);
    }
    let mut bitangent = Vec3f::cross(normal, tangent).normalize();
    let aligned_incoming_bitangent = align_direction(current.patch.bitangent, incoming.bitangent);
    if bitangent.dot(aligned_incoming_bitangent) < 0.0 {
        tangent = tangent.scale(-1.0);
        bitangent = bitangent.scale(-1.0);
    }

    XrDepthPlanePatch {
        generation: incoming.generation,
        kind: incoming.kind,
        center: current.patch.center.scale(one_minus_alpha) + incoming.center.scale(alpha),
        normal,
        tangent,
        bitangent,
        half_extent_tangent: current.patch.half_extent_tangent * one_minus_alpha
            + incoming.half_extent_tangent * alpha,
        half_extent_bitangent: current.patch.half_extent_bitangent * one_minus_alpha
            + incoming.half_extent_bitangent * alpha,
        area: current.patch.area * one_minus_alpha + incoming.area * alpha,
        support_triangles: current.patch.support_triangles.max(incoming.support_triangles),
    }
}

fn blend_horizontal_tracked_plane_patch(
    current: &TrackedPlanePatch,
    incoming: &XrDepthPlanePatch,
) -> XrDepthPlanePatch {
    let target = reframe_patch_onto_basis(
        incoming,
        current.patch.normal,
        current.patch.tangent,
        current.patch.bitangent,
    );
    let stable = current.hit_count >= DEPTH_PLANE_TRACK_STABLE_HITS;
    let center_alpha = if !stable {
        DEPTH_PLANE_TRACK_CENTER_ALPHA * 1.5
    } else {
        DEPTH_PLANE_TRACK_STABLE_CENTER_ALPHA
    };
    let target_center = current.patch.center.scale(1.0 - center_alpha) + target.center.scale(center_alpha);
    let next_half_tangent = blend_extent_hysteretic(
        current.patch.half_extent_tangent,
        target.half_extent_tangent,
        stable,
    );
    let next_half_bitangent = blend_extent_hysteretic(
        current.patch.half_extent_bitangent,
        target.half_extent_bitangent,
        stable,
    );

    XrDepthPlanePatch {
        generation: incoming.generation,
        kind: if stable && is_horizontal_plane_kind(current.patch.kind) {
            current.patch.kind
        } else {
            choose_plane_kind(current.patch.kind, incoming.kind)
        },
        center: target_center,
        normal: current.patch.normal,
        tangent: current.patch.tangent,
        bitangent: current.patch.bitangent,
        half_extent_tangent: next_half_tangent,
        half_extent_bitangent: next_half_bitangent,
        area: (next_half_tangent * 2.0) * (next_half_bitangent * 2.0),
        support_triangles: current.patch.support_triangles.max(incoming.support_triangles),
    }
}

fn blend_extent_hysteretic(current: f32, target: f32, stable: bool) -> f32 {
    let alpha = if target >= current {
        DEPTH_PLANE_TRACK_HORIZONTAL_EXPAND_ALPHA
    } else if stable {
        DEPTH_PLANE_TRACK_HORIZONTAL_STABLE_SHRINK_ALPHA
    } else {
        DEPTH_PLANE_TRACK_HORIZONTAL_SHRINK_ALPHA
    };
    current * (1.0 - alpha) + target * alpha
}

fn is_horizontal_plane_kind(kind: XrDepthPlaneKind) -> bool {
    matches!(
        kind,
        XrDepthPlaneKind::Floor | XrDepthPlaneKind::Table | XrDepthPlaneKind::Ceiling
    )
}

fn blend_direction(current: Vec3f, incoming: Vec3f, alpha: f32) -> Vec3f {
    let incoming = align_direction(current, incoming);
    (current.scale(1.0 - alpha) + incoming.scale(alpha)).normalize()
}

fn align_direction(reference: Vec3f, candidate: Vec3f) -> Vec3f {
    if reference.dot(candidate) < 0.0 {
        candidate.scale(-1.0)
    } else {
        candidate
    }
}

fn orthogonal_tangent(normal: Vec3f) -> Vec3f {
    let fallback = if normal.y.abs() < 0.9 {
        vec3f(0.0, 1.0, 0.0)
    } else {
        vec3f(1.0, 0.0, 0.0)
    };
    Vec3f::cross(fallback, normal).normalize()
}

fn process_geometry_queries(
    volume: &DepthMeshVolume,
    store: &XrDepthMeshStore,
    max_queries: usize,
) -> bool {
    let pending = store.drain_pending_queries(max_queries);
    if pending.is_empty() {
        return false;
    }

    let mut results = Vec::with_capacity(pending.len());
    for pending_query in pending {
        results.push(evaluate_geometry_query(
            volume,
            pending_query.query,
            pending_query.version,
        ));
    }
    store.publish_query_results(results);
    true
}

fn evaluate_geometry_query(
    volume: &DepthMeshVolume,
    query: XrDepthMeshQuery,
    version: u64,
) -> XrDepthMeshQueryResult {
    let travel = query.predicted_center - query.center;
    let travel_distance = travel.length();
    let motion_dir = if travel_distance > 1.0e-4 {
        travel.scale(1.0 / travel_distance)
    } else {
        vec3f(0.0, 0.0, 0.0)
    };
    let max_search_distance = (query.radius + query.max_distance + travel_distance).max(0.0);
    let max_search_distance_sq = max_search_distance * max_search_distance;
    let sweep_bounds_min = vec3f(
        query.center.x.min(query.predicted_center.x),
        query.center.y.min(query.predicted_center.y),
        query.center.z.min(query.predicted_center.z),
    );
    let sweep_bounds_max = vec3f(
        query.center.x.max(query.predicted_center.x),
        query.center.y.max(query.predicted_center.y),
        query.center.z.max(query.predicted_center.z),
    );
    let mut best_hit: Option<XrDepthMeshQueryHit> = None;
    let mut best_hit_score = f32::INFINITY;
    let mid_point = query.center + travel.scale(0.5);
    let sweep_radius = query.radius + query.max_distance;
    let sweep_radius_sq = sweep_radius * sweep_radius;

    for chunk in &volume.mesh_chunks {
        if aabb_aabb_distance_sq(sweep_bounds_min, sweep_bounds_max, chunk.bounds_min, chunk.bounds_max)
            > max_search_distance_sq
        {
            continue;
        }
        for triangle in chunk.indices.chunks_exact(3) {
            let a = chunk.vertices[triangle[0] as usize];
            let b = chunk.vertices[triangle[1] as usize];
            let c = chunk.vertices[triangle[2] as usize];
            let raw_normal = Vec3f::cross(b - a, c - a);
            if raw_normal.length() <= 1.0e-6 {
                continue;
            }
            let mut best_sample_progress = 0.0;
            let mut best_sample_score = f32::INFINITY;
            let mut best_closest = vec3f(0.0, 0.0, 0.0);
            let mut best_distance_sq = f32::INFINITY;

            for (sample_point, progress) in [
                (query.center, 0.0f32),
                (mid_point, 0.5f32),
                (query.predicted_center, 1.0f32),
            ] {
                let closest = closest_point_on_triangle(sample_point, a, b, c);
                let delta = closest - sample_point;
                let distance_sq = delta.dot(delta);
                if distance_sq > max_search_distance_sq {
                    continue;
                }
                let lateral_sq = point_segment_distance_sq(closest, query.center, query.predicted_center);
                if lateral_sq > sweep_radius_sq {
                    continue;
                }
                let distance = distance_sq.sqrt();
                let mut score = distance;
                if travel_distance > 1.0e-4 {
                    let forward = (closest - query.center).dot(motion_dir);
                    if forward < -query.radius || forward > travel_distance + query.radius {
                        continue;
                    }

                    let mut candidate_normal = raw_normal.normalize();
                    if candidate_normal.dot(sample_point - closest) < 0.0 {
                        candidate_normal = candidate_normal.scale(-1.0);
                    }
                    let opposing = candidate_normal.dot(-motion_dir);
                    if opposing <= DEPTH_QUERY_MIN_OPPOSING_NORMAL_DOT {
                        continue;
                    }
                    score -= progress * travel_distance * 0.35;
                    score -= forward.clamp(0.0, travel_distance) * 0.15;
                    score -= opposing * 0.08;
                    score += lateral_sq.sqrt() * 0.2;
                }
                if score < best_sample_score {
                    best_sample_score = score;
                    best_sample_progress = progress;
                    best_closest = closest;
                    best_distance_sq = distance_sq;
                }
            }

            if !best_distance_sq.is_finite() {
                continue;
            }

            let mut normal = raw_normal.normalize();
            if normal.length() <= 1.0e-6 {
                continue;
            }
            let mut hit_triangle = [a, b, c];
            let facing_point = query.center + travel.scale(best_sample_progress);
            if normal.dot(facing_point - best_closest) < 0.0 {
                normal = normal.scale(-1.0);
                hit_triangle.swap(1, 2);
            }
            if best_sample_score < best_hit_score {
                best_hit_score = best_sample_score;
                best_hit = Some(XrDepthMeshQueryHit {
                    key: query.key,
                    version,
                    mesh_generation: volume.mesh_generation,
                    distance: best_distance_sq.sqrt(),
                    point: best_closest,
                    normal,
                    triangle: hit_triangle,
                    patch: [best_closest; 4],
                    chunk_key: chunk.chunk_key,
                });
            }
        }
    }

    if let Some(mut hit) = best_hit {
        hit.patch = fit_support_patch(volume, &query, &hit);
        hit.triangle = fit_support_triangle(&query, &hit);
        XrDepthMeshQueryResult::Hit(hit)
    } else {
        XrDepthMeshQueryResult::Miss {
            key: query.key,
            version,
            mesh_generation: volume.mesh_generation,
        }
    }
}

fn fit_support_patch(
    volume: &DepthMeshVolume,
    query: &XrDepthMeshQuery,
    hit: &XrDepthMeshQueryHit,
) -> [Vec3f; 4] {
    let travel_distance = (query.predicted_center - query.center).length();
    let patch_radius = (DEPTH_QUERY_PATCH_RADIUS_METERS + travel_distance * 0.5)
        .max(query.radius * 2.5 + query.max_distance)
        .min(0.42);
    let search_radius = patch_radius + DEPTH_QUERY_PATCH_PLANE_TOLERANCE_METERS;
    let search_radius_sq = search_radius * search_radius;

    let mut points = Vec::with_capacity(48);
    let mut normal_sum = hit.normal;

    for chunk in &volume.mesh_chunks {
        if point_aabb_distance_sq(hit.point, chunk.bounds_min, chunk.bounds_max) > search_radius_sq {
            continue;
        }
        for triangle in chunk.indices.chunks_exact(3) {
            let a = chunk.vertices[triangle[0] as usize];
            let b = chunk.vertices[triangle[1] as usize];
            let c = chunk.vertices[triangle[2] as usize];
            let mut tri_normal = Vec3f::cross(b - a, c - a);
            let tri_normal_len = tri_normal.length();
            if tri_normal_len <= 1.0e-6 {
                continue;
            }
            tri_normal = tri_normal.scale(1.0 / tri_normal_len);
            let alignment = tri_normal.dot(hit.normal);
            if alignment.abs() < DEPTH_QUERY_PATCH_NORMAL_DOT {
                continue;
            }
            if alignment < 0.0 {
                tri_normal = tri_normal.scale(-1.0);
            }

            let centroid = (a + b + c).scale(1.0 / 3.0);
            let centroid_offset = centroid - hit.point;
            let plane_distance = centroid_offset.dot(hit.normal).abs();
            if plane_distance > DEPTH_QUERY_PATCH_PLANE_TOLERANCE_METERS {
                continue;
            }

            let planar_offset =
                centroid_offset - hit.normal.scale(centroid_offset.dot(hit.normal));
            if planar_offset.dot(planar_offset) > patch_radius * patch_radius {
                continue;
            }

            points.push(a);
            points.push(b);
            points.push(c);
            normal_sum = normal_sum + tri_normal;
        }
    }

    if points.is_empty() {
        points.extend_from_slice(&hit.triangle);
    }

    let fitted_normal = if normal_sum.length() > 1.0e-6 {
        normal_sum.normalize()
    } else {
        hit.normal
    };
    let preferred_tangent = if query.velocity.length() > 1.0e-4 {
        query.velocity
    } else {
        hit.triangle[1] - hit.triangle[0]
    };
    let (tangent, bitangent) = plane_basis(fitted_normal, preferred_tangent);

    let mut min_u = f32::INFINITY;
    let mut max_u = f32::NEG_INFINITY;
    let mut min_v = f32::INFINITY;
    let mut max_v = f32::NEG_INFINITY;
    for point in &points {
        let offset = *point - hit.point;
        let plane_offset = offset - fitted_normal.scale(offset.dot(fitted_normal));
        let u = plane_offset.dot(tangent);
        let v = plane_offset.dot(bitangent);
        min_u = min_u.min(u);
        max_u = max_u.max(u);
        min_v = min_v.min(v);
        max_v = max_v.max(v);
    }

    if !min_u.is_finite() || !max_u.is_finite() || !min_v.is_finite() || !max_v.is_finite() {
        return fallback_support_patch(hit.point, tangent, bitangent, query.radius);
    }

    let travel_along_tangent = query.velocity.dot(tangent).abs() * 0.12;
    let travel_along_bitangent = query.velocity.dot(bitangent).abs() * 0.12;
    let min_half_extent = DEPTH_QUERY_PATCH_MIN_HALF_EXTENT_METERS.max(query.radius * 1.5);
    let min_half_extent_u = min_half_extent + travel_along_tangent;
    let min_half_extent_v = min_half_extent + travel_along_bitangent;

    min_u = (min_u - DEPTH_QUERY_PATCH_MARGIN_METERS).min(-min_half_extent_u);
    max_u = (max_u + DEPTH_QUERY_PATCH_MARGIN_METERS).max(min_half_extent_u);
    min_v = (min_v - DEPTH_QUERY_PATCH_MARGIN_METERS).min(-min_half_extent_v);
    max_v = (max_v + DEPTH_QUERY_PATCH_MARGIN_METERS).max(min_half_extent_v);

    [
        hit.point + tangent.scale(min_u) + bitangent.scale(min_v),
        hit.point + tangent.scale(max_u) + bitangent.scale(min_v),
        hit.point + tangent.scale(max_u) + bitangent.scale(max_v),
        hit.point + tangent.scale(min_u) + bitangent.scale(max_v),
    ]
}

fn fallback_support_patch(
    point: Vec3f,
    tangent: Vec3f,
    bitangent: Vec3f,
    radius: f32,
) -> [Vec3f; 4] {
    let half_extent = DEPTH_QUERY_PATCH_MIN_HALF_EXTENT_METERS.max(radius * 1.75)
        + DEPTH_QUERY_PATCH_MARGIN_METERS;
    [
        point + tangent.scale(-half_extent) + bitangent.scale(-half_extent),
        point + tangent.scale(half_extent) + bitangent.scale(-half_extent),
        point + tangent.scale(half_extent) + bitangent.scale(half_extent),
        point + tangent.scale(-half_extent) + bitangent.scale(half_extent),
    ]
}

fn fit_support_triangle(query: &XrDepthMeshQuery, hit: &XrDepthMeshQueryHit) -> [Vec3f; 3] {
    let preferred_forward = if query.velocity.length() > 1.0e-4 {
        query.velocity
    } else {
        hit.patch[1] - hit.patch[0]
    };
    let (forward, lateral) = plane_basis(hit.normal, preferred_forward);
    let center = hit.point;

    let mut min_forward = f32::INFINITY;
    let mut max_forward = f32::NEG_INFINITY;
    let mut min_lateral = f32::INFINITY;
    let mut max_lateral = f32::NEG_INFINITY;
    for corner in hit.patch {
        let offset = corner - center;
        let plane_offset = offset - hit.normal.scale(offset.dot(hit.normal));
        let f = plane_offset.dot(forward);
        let l = plane_offset.dot(lateral);
        min_forward = min_forward.min(f);
        max_forward = max_forward.max(f);
        min_lateral = min_lateral.min(l);
        max_lateral = max_lateral.max(l);
    }

    let min_extent = DEPTH_QUERY_PATCH_MIN_HALF_EXTENT_METERS.max(query.radius * 1.2);
    let velocity_boost = query.velocity.length() * 0.08;
    let front_extent =
        (max_forward + DEPTH_QUERY_PATCH_MARGIN_METERS + velocity_boost).max(min_extent);
    let back_extent = ((-min_forward) + DEPTH_QUERY_PATCH_MARGIN_METERS).max(min_extent);
    let side_extent = (max_lateral.abs().max(min_lateral.abs()) + DEPTH_QUERY_PATCH_MARGIN_METERS)
        .max(min_extent);

    let mut triangle = [
        center + forward.scale(front_extent),
        center - forward.scale(back_extent) + lateral.scale(side_extent),
        center - forward.scale(back_extent) - lateral.scale(side_extent),
    ];
    let tri_normal = Vec3f::cross(triangle[1] - triangle[0], triangle[2] - triangle[0]);
    if tri_normal.dot(hit.normal) < 0.0 {
        triangle.swap(1, 2);
    }
    triangle
}

fn plane_basis(normal: Vec3f, preferred_tangent: Vec3f) -> (Vec3f, Vec3f) {
    let projected_tangent = preferred_tangent - normal.scale(preferred_tangent.dot(normal));
    let tangent = if projected_tangent.length() > 1.0e-5 {
        projected_tangent.normalize()
    } else {
        let fallback_axis = if normal.y.abs() < 0.9 {
            vec3f(0.0, 1.0, 0.0)
        } else {
            vec3f(1.0, 0.0, 0.0)
        };
        Vec3f::cross(fallback_axis, normal).normalize()
    };
    let bitangent = Vec3f::cross(normal, tangent).normalize();
    (tangent, bitangent)
}

fn point_aabb_distance_sq(point: Vec3f, bounds_min: Vec3f, bounds_max: Vec3f) -> f32 {
    let dx = if point.x < bounds_min.x {
        bounds_min.x - point.x
    } else if point.x > bounds_max.x {
        point.x - bounds_max.x
    } else {
        0.0
    };
    let dy = if point.y < bounds_min.y {
        bounds_min.y - point.y
    } else if point.y > bounds_max.y {
        point.y - bounds_max.y
    } else {
        0.0
    };
    let dz = if point.z < bounds_min.z {
        bounds_min.z - point.z
    } else if point.z > bounds_max.z {
        point.z - bounds_max.z
    } else {
        0.0
    };
    dx * dx + dy * dy + dz * dz
}

fn aabb_aabb_distance_sq(
    a_min: Vec3f,
    a_max: Vec3f,
    b_min: Vec3f,
    b_max: Vec3f,
) -> f32 {
    let dx = if a_max.x < b_min.x {
        b_min.x - a_max.x
    } else if b_max.x < a_min.x {
        a_min.x - b_max.x
    } else {
        0.0
    };
    let dy = if a_max.y < b_min.y {
        b_min.y - a_max.y
    } else if b_max.y < a_min.y {
        a_min.y - b_max.y
    } else {
        0.0
    };
    let dz = if a_max.z < b_min.z {
        b_min.z - a_max.z
    } else if b_max.z < a_min.z {
        a_min.z - b_max.z
    } else {
        0.0
    };
    dx * dx + dy * dy + dz * dz
}

fn point_segment_distance_sq(point: Vec3f, start: Vec3f, end: Vec3f) -> f32 {
    let segment = end - start;
    let segment_length_sq = segment.dot(segment);
    if segment_length_sq <= 1.0e-8 {
        let delta = point - start;
        return delta.dot(delta);
    }
    let t = ((point - start).dot(segment) / segment_length_sq).clamp(0.0, 1.0);
    let closest = start + segment.scale(t);
    let delta = point - closest;
    delta.dot(delta)
}

fn closest_point_on_triangle(point: Vec3f, a: Vec3f, b: Vec3f, c: Vec3f) -> Vec3f {
    let ab = b - a;
    let ac = c - a;
    let ap = point - a;
    let d1 = ab.dot(ap);
    let d2 = ac.dot(ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }

    let bp = point - b;
    let d3 = ab.dot(bp);
    let d4 = ac.dot(bp);
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }

    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3).max(f32::EPSILON);
        return a + ab.scale(v);
    }

    let cp = point - c;
    let d5 = ab.dot(cp);
    let d6 = ac.dot(cp);
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }

    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6).max(f32::EPSILON);
        return a + ac.scale(w);
    }

    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let edge = c - b;
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6)).max(f32::EPSILON);
        return b + edge.scale(w);
    }

    let denom = (va + vb + vc).max(f32::EPSILON);
    let v = vb / denom;
    let w = vc / denom;
    a + ab.scale(v) + ac.scale(w)
}

fn depth_mesh_chunk_from_surface_mesh(
    chunk_key: IVector,
    generation: u64,
    mesh: SurfaceMesh32,
) -> Option<XrDepthMeshChunk> {
    if mesh.positions.is_empty() || mesh.indices.is_empty() {
        return None;
    }
    let mut bounds_min = vec3f(
        mesh.positions[0][0],
        mesh.positions[0][1],
        mesh.positions[0][2],
    );
    let mut bounds_max = bounds_min;
    let mut vertices = Vec::with_capacity(mesh.positions.len());
    let mut normals = Vec::with_capacity(mesh.normals.len());
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    chunk_key.hash(&mut hasher);

    for (position, normal) in mesh.positions.iter().zip(mesh.normals.iter()) {
        let position = vec3f(position[0], position[1], position[2]);
        let normal = vec3f(normal[0], normal[1], normal[2]);
        bounds_min = Vec3f::min_componentwise(bounds_min, position);
        bounds_max = Vec3f::max_componentwise(bounds_max, position);
        quantize_f32(position.x, 0.01).hash(&mut hasher);
        quantize_f32(position.y, 0.01).hash(&mut hasher);
        quantize_f32(position.z, 0.01).hash(&mut hasher);
        vertices.push(position);
        normals.push(normal);
    }
    mesh.indices.hash(&mut hasher);

    Some(XrDepthMeshChunk {
        generation,
        chunk_key,
        fingerprint: hasher.finish(),
        bounds_min,
        bounds_max,
        vertices,
        normals,
        indices: mesh.indices,
    })
}

fn voxel_coord_from_ivector(key: IVector) -> VoxelCoord {
    VoxelCoord::new(key.x, key.y, key.z)
}

fn push_unique_chunk_key(keys: &mut Vec<IVector>, key: IVector) {
    if !keys.contains(&key) {
        keys.push(key);
    }
}

fn quantize_f32(value: f32, quantum: f32) -> i32 {
    (value / quantum.max(f32::EPSILON)).round() as i32
}

fn align_extent(extent: i32, stride: i32) -> i32 {
    let stride = stride.max(1);
    if extent <= 0 {
        0
    } else {
        ((extent + stride - 1) / stride) * stride
    }
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

fn lasertag_surface_net_mesh_from_dense(
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
        if !raw.is_finite() {
            0.0
        } else {
            raw
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
        lasertag_tris_for_axis(
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
        lasertag_tris_for_axis(
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
        lasertag_tris_for_axis(
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

fn dense_corner_coord(base: VoxelCoord, corner: VoxelCoord, stride: i32) -> VoxelCoord {
    VoxelCoord::new(
        base.x * stride + corner.x * stride,
        base.y * stride + corner.y * stride,
        base.z * stride + corner.z * stride,
    )
}

fn flatten_coord(coord: VoxelCoord, size: VoxelCoord) -> usize {
    coord.x as usize
        + coord.y as usize * size.x as usize
        + coord.z as usize * size.x as usize * size.y as usize
}

fn lasertag_tris_for_axis(
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
