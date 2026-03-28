use crate::{
    makepad_math::{vec2f, vec3f, vec4f, Mat4f, Vec2f, Vec3f, Vec4f},
    os::linux::{
        openxr::CxOpenXrFrame,
        vulkan::{CxVulkan, CxVulkanOpenXrSessionData},
    },
    thread::SignalToUI,
    xr_depth_mesh::{
        empty_bounds, xr_depth_mesh_store,
        ChunkKey, SparseTsdGridReadSnapshot, SparseTsdReadChunk, TsdfPublishedSnapshot,
        XrDepthAlignHeightMap,
        XrDepthAlignSample, XrDepthAlignSampleKind, XrDepthAlignSlicePreview, XrDepthMeshChunk, XrDepthMeshStore,
        XrDepthPlanePatch,
    },
};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    hash::Hash,
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
const DEPTH_VOXEL_MIN_DISTANCE_METERS: f32 = 0.08;
const DEPTH_VOXEL_MAX_DISTANCE_METERS: f32 = 6.0;
const DEPTH_TSD_MIN_UPDATE_DISTANCE_METERS: f32 = 0.5;
const DEPTH_TSD_UPDATE_IDLE_INTERVAL_MILLIS: u64 = 200;
const DEPTH_TSD_UPDATE_MOVING_INTERVAL_MILLIS: u64 = 33;
const DEPTH_TSD_UPDATE_TRANSLATION_TRIGGER_METERS: f32 = 0.04;
const DEPTH_TSD_UPDATE_ROTATION_TRIGGER_DOT: f32 = 0.999;
const DEPTH_VOXEL_MIN_DEPTH_VALUE: f32 = 1.0 / 65535.0;
const DEPTH_VOXEL_MAX_DEPTH_VALUE: f32 = 0.9995;
const DEPTH_TSD_MIN_NORMAL_DOT: f32 = 0.3;
const DEPTH_TSD_APPLY_DELTA_EPSILON: f32 = 0.01;
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
const DEPTH_SURFACE_MESH_IDLE_WAIT_MILLIS: u64 = 8;
const DEPTH_PUBLISHED_HEIGHT_MAP_INTERVAL_MILLIS: u64 = 1000;
const DEPTH_ALIGN_MAX_WALL_SAMPLES: usize = 96;
const DEPTH_ALIGN_HEIGHT_MAP_BOUNDS_PADDING_METERS: f32 = 0.45;
const DEPTH_ALIGN_VECTOR_SLICE_TOP_Y_METERS: f32 = 2.00;
const DEPTH_ALIGN_VECTOR_SLICE_ISO_HEIGHT_METERS: f32 = 0.50;
const DEPTH_ALIGN_VECTOR_SLICE_MIN_PROJECT_Y_METERS: f32 = 0.00;
const DEPTH_ALIGN_VECTOR_SLICE_MIN_HORIZONTAL_NORMAL: f32 = 0.55;
const DEPTH_ALIGN_VECTOR_SLICE_PLAYER_CUTOUT_RADIUS_METERS: f32 =
    DEPTH_PLAYER_EXCLUDE_RADIUS_METERS + 0.12;
const DEPTH_ALIGN_VECTOR_SLICE_MIN_SEGMENT_METERS: f32 = 0.05;
const DEPTH_ALIGN_VECTOR_SLICE_SAMPLES_PER_CELL: f32 = 1.35;
const DEPTH_ALIGN_VECTOR_SLICE_MAX_SAMPLES_PER_SEGMENT: usize = 3;
const DEPTH_ALIGN_PROJECTED_HEIGHT_SAMPLES_PER_TICK: usize = 512;
const DEPTH_ALIGN_PROJECTED_HEIGHT_DIRTY_SWIZZLE: usize = 8;

const fn depth_tsd_distance_meters(voxel_size_meters: f32) -> f32 {
    voxel_size_meters * 2.0
}

const fn depth_tsd_refresh_clearance_meters(voxel_size_meters: f32) -> f32 {
    voxel_size_meters * 1.5
}

const fn depth_normal_neighbor_max_distance_delta_meters(voxel_size_meters: f32) -> f32 {
    voxel_size_meters * 2.5
}

const fn depth_carve_neighbor_max_distance_delta_meters(voxel_size_meters: f32) -> f32 {
    voxel_size_meters * 1.5
}

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

#[derive(Clone, Debug)]
struct SparseTsdChunk {
    values: Vec<f32>,
    valid: Vec<u8>,
    confidence: Vec<u8>,
    observed_generation: Vec<u64>,
    live_count: usize,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SparseTsdWriteResult {
    state_changed: bool,
    value_changed: bool,
    became_live: bool,
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

    fn confidence(&self, id: usize) -> u8 {
        if self.valid[id] == 0 {
            0
        } else {
            self.confidence[id]
        }
    }

    fn accumulate(&mut self, id: usize, value: f32, generation: u64) -> SparseTsdWriteResult {
        let previous_valid = self.valid[id];
        let previous_confidence = self.confidence[id];
        let previous_generation = self.observed_generation[id];
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
        SparseTsdWriteResult {
            state_changed: previous_valid == 0
                || changed
                || previous_confidence != self.confidence[id]
                || previous_generation != self.observed_generation[id],
            value_changed: changed,
            became_live: previous.is_none(),
        }
    }

    fn overwrite(&mut self, id: usize, value: f32, generation: u64) -> SparseTsdWriteResult {
        let previous_valid = self.valid[id];
        let previous_confidence = self.confidence[id];
        let previous_generation = self.observed_generation[id];
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
        SparseTsdWriteResult {
            state_changed: previous_valid == 0
                || changed
                || previous_confidence != self.confidence[id]
                || previous_generation != self.observed_generation[id],
            value_changed: changed,
            became_live: previous.is_none(),
        }
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

    pub fn normalized_distance(&self, coord: VoxelCoord) -> Option<f32> {
        let (chunk_key, local_id) = self.chunk_key_and_id(coord);
        let chunk = self.chunks.get(&chunk_key)?;
        chunk.value(local_id)
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
    ) -> SparseTsdWriteResult {
        let (chunk_key, local_id) = self.chunk_key_and_id(coord);
        let chunk = self
            .chunks
            .entry(chunk_key)
            .or_insert_with(|| SparseTsdChunk::new(self.chunk_volume));
        let result = chunk.accumulate(local_id, value, generation);
        if result.became_live {
            self.active_value_count += 1;
        }
        result
    }

    pub fn overwrite_normalized_distance(
        &mut self,
        coord: VoxelCoord,
        value: f32,
        generation: u64,
    ) -> SparseTsdWriteResult {
        let (chunk_key, local_id) = self.chunk_key_and_id(coord);
        let chunk = self
            .chunks
            .entry(chunk_key)
            .or_insert_with(|| SparseTsdChunk::new(self.chunk_volume));
        let result = chunk.overwrite(local_id, value, generation);
        if result.became_live {
            self.active_value_count += 1;
        }
        result
    }

    pub fn world_to_voxel_coord(&self, point: Vec3f) -> VoxelCoord {
        VoxelCoord::new(
            (point.x / self.voxel_size).floor() as i32,
            (point.y / self.voxel_size).floor() as i32,
            (point.z / self.voxel_size).floor() as i32,
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

    fn copy_read_chunk(&self, chunk_key: VoxelCoord) -> Option<SparseTsdReadChunk> {
        let chunk = self.chunks.get(&chunk_key)?;
        Some(SparseTsdReadChunk {
            values: chunk.values.clone(),
            valid: chunk.valid.clone(),
            confidence: chunk.confidence.clone(),
            observed_generation: chunk.observed_generation.clone(),
        })
    }

    fn build_read_snapshot(
        &self,
        previous: Option<&SparseTsdGridReadSnapshot>,
        dirty_chunk_keys: &HashSet<VoxelCoord>,
    ) -> SparseTsdGridReadSnapshot {
        let previous = previous.filter(|previous| {
            previous.chunk_edge == self.chunk_edge
                && previous.chunk_volume == self.chunk_volume
                && (previous.voxel_size - self.voxel_size).abs() <= f32::EPSILON
        });
        let mut chunks = HashMap::with_capacity(self.chunks.len());
        for (&chunk_key, chunk) in &self.chunks {
            if chunk.live_count == 0 {
                continue;
            }
            let read_chunk_key = voxel_coord_to_chunk_key(chunk_key);
            let read_chunk = previous
                .filter(|_| !dirty_chunk_keys.contains(&chunk_key))
                .and_then(|previous| previous.chunks.get(&read_chunk_key).cloned())
                .unwrap_or_else(|| {
                    Arc::new(
                        self.copy_read_chunk(chunk_key)
                            .expect("mutable chunk should exist while publishing TSDF snapshot"),
                    )
                });
            chunks.insert(read_chunk_key, read_chunk);
        }
        SparseTsdGridReadSnapshot {
            voxel_size: self.voxel_size,
            chunk_edge: self.chunk_edge,
            chunk_volume: self.chunk_volume,
            active_value_count: self.active_value_count,
            active_bounds: self.world_bounds(0),
            chunks,
        }
    }
}

fn voxel_coord_to_chunk_key(coord: VoxelCoord) -> ChunkKey {
    ChunkKey::new(coord.x, coord.y, coord.z)
}

impl SparseTsdReadChunk {
    pub fn value(&self, id: usize) -> Option<f32> {
        if self.valid[id] == 0 {
            None
        } else {
            Some(self.values[id])
        }
    }

    pub fn meshing_value(&self, id: usize, current_generation: u64) -> Option<f32> {
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

    pub fn confidence(&self, id: usize) -> u8 {
        if self.valid[id] == 0 {
            0
        } else {
            self.confidence[id]
        }
    }
}

impl SparseTsdGridReadSnapshot {
    pub fn is_empty(&self) -> bool {
        self.active_value_count == 0
    }

    pub fn normalized_distance(&self, coord: VoxelCoord) -> Option<f32> {
        let (chunk_key, local_id) = self.chunk_key_and_id(coord);
        let chunk = self.chunks.get(&chunk_key)?;
        chunk.value(local_id)
    }

    pub fn meshing_distance(&self, coord: VoxelCoord, current_generation: u64) -> Option<f32> {
        let (chunk_key, local_id) = self.chunk_key_and_id(coord);
        let chunk = self.chunks.get(&chunk_key)?;
        chunk.meshing_value(local_id, current_generation)
    }

    pub fn confidence(&self, coord: VoxelCoord) -> u8 {
        let (chunk_key, local_id) = self.chunk_key_and_id(coord);
        self.chunks
            .get(&chunk_key)
            .map(|chunk| chunk.confidence(local_id))
            .unwrap_or(0)
    }

    pub fn world_to_voxel_coord(&self, point: Vec3f) -> VoxelCoord {
        VoxelCoord::new(
            (point.x / self.voxel_size).floor() as i32,
            (point.y / self.voxel_size).floor() as i32,
            (point.z / self.voxel_size).floor() as i32,
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
        let (min, max) = self.active_bounds?;
        let padding = padding_voxels as f32 * self.voxel_size;
        Some((
            vec3f(min.x - padding, min.y - padding, min.z - padding),
            vec3f(max.x + padding, max.y + padding, max.z + padding),
        ))
    }

    pub fn chunk_key_and_id(&self, coord: VoxelCoord) -> (ChunkKey, usize) {
        let chunk_coord = VoxelCoord::new(
            coord.x.div_euclid(self.chunk_edge),
            coord.y.div_euclid(self.chunk_edge),
            coord.z.div_euclid(self.chunk_edge),
        );
        let lx = coord.x.rem_euclid(self.chunk_edge) as usize;
        let ly = coord.y.rem_euclid(self.chunk_edge) as usize;
        let lz = coord.z.rem_euclid(self.chunk_edge) as usize;
        let edge = self.chunk_edge as usize;
        let id = lx + ly * edge + lz * edge * edge;
        (voxel_coord_to_chunk_key(chunk_coord), id)
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
    dirty_tsdf_chunk_keys: HashSet<VoxelCoord>,
    mesh_chunks: Vec<XrDepthMeshChunk>,
    update_sequence: u64,
    dirty_chunk_keys: Vec<ChunkKey>,
    removed_chunk_keys: Vec<ChunkKey>,
    mesh_vertex_count: usize,
    mesh_triangle_count: usize,
    plane_patches: Vec<XrDepthPlanePatch>,
    latest_camera_world: Option<Vec3f>,
    latest_camera_forward: Option<Vec3f>,
    projected_height_field: Option<ProjectedHeightField>,
    projected_height_layout_rebuild_pending: bool,
    published_height_map: Option<XrDepthAlignHeightMap>,
    pending_mesh_dirty_chunks: HashSet<ChunkKey>,
    pending_mesh_chunk_queue: VecDeque<ChunkKey>,
    pending_plane_scan_dirty_chunks: HashSet<ChunkKey>,
    pending_plane_scan_chunk_queue: VecDeque<ChunkKey>,
    pending_projected_height_dirty_samples: HashSet<usize>,
    pending_projected_height_sample_queue: VecDeque<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ProjectedHeightFieldLayout {
    origin_x: f32,
    origin_z: f32,
    cell_size_meters: f32,
    size_x: usize,
    size_z: usize,
    top_y_meters: f32,
    bottom_y_meters: f32,
}

#[derive(Clone, Debug)]
struct ProjectedHeightField {
    layout: ProjectedHeightFieldLayout,
    player_cutout_center: Option<Vec3f>,
    heights_meters: Vec<f32>,
    valid: Vec<u8>,
}

impl ProjectedHeightField {
    fn new(layout: ProjectedHeightFieldLayout, player_cutout_center: Option<Vec3f>) -> Self {
        let sample_count = (layout.size_x + 1) * (layout.size_z + 1);
        Self {
            layout,
            player_cutout_center,
            heights_meters: vec![0.0; sample_count],
            valid: vec![0; sample_count],
        }
    }

    fn sample_size_x(&self) -> usize {
        self.layout.size_x + 1
    }

    fn sample_size_z(&self) -> usize {
        self.layout.size_z + 1
    }
}

impl DepthMeshVolume {
    fn new(sample_step: u32, voxel_size_meters: f32) -> Self {
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
            dirty_tsdf_chunk_keys: HashSet::new(),
            mesh_chunks: Vec::new(),
            update_sequence: 0,
            dirty_chunk_keys: Vec::new(),
            removed_chunk_keys: Vec::new(),
            mesh_vertex_count: 0,
            mesh_triangle_count: 0,
            plane_patches: Vec::new(),
            latest_camera_world: None,
            latest_camera_forward: None,
            projected_height_field: None,
            projected_height_layout_rebuild_pending: false,
            published_height_map: None,
            pending_mesh_dirty_chunks: HashSet::new(),
            pending_mesh_chunk_queue: VecDeque::new(),
            pending_plane_scan_dirty_chunks: HashSet::new(),
            pending_plane_scan_chunk_queue: VecDeque::new(),
            pending_projected_height_dirty_samples: HashSet::new(),
            pending_projected_height_sample_queue: VecDeque::new(),
        }
    }

    fn update_bounds(&mut self) {
        if let Some((min, max)) = self.mesh_grid.world_bounds(0) {
            self.bounds_min = min;
            self.bounds_max = max;
        } else {
            (self.bounds_min, self.bounds_max) = empty_bounds();
        }
    }

    fn published_tsdf_snapshot(
        &self,
        previous: Option<&TsdfPublishedSnapshot>,
    ) -> TsdfPublishedSnapshot {
        let grid = if self.dirty_tsdf_chunk_keys.is_empty() {
            previous
                .map(|previous| previous.grid.clone())
                .unwrap_or_else(|| {
                    Arc::new(self.mesh_grid.build_read_snapshot(None, &self.dirty_tsdf_chunk_keys))
                })
        } else {
            Arc::new(self.mesh_grid.build_read_snapshot(
                previous.map(|previous| previous.grid.as_ref()),
                &self.dirty_tsdf_chunk_keys,
            ))
        };
        TsdfPublishedSnapshot {
            generation: self.generation,
            latest_topology_generation: self.latest_topology_generation,
            update_sequence: self.update_sequence,
            grid,
            height_map: self.published_height_map.clone(),
        }
    }

    fn clear_published_tsdf_dirty_state(&mut self) {
        self.dirty_tsdf_chunk_keys.clear();
    }

    fn clear_published_height_map(&mut self) -> bool {
        if self.published_height_map.is_none() {
            return false;
        }
        self.published_height_map = None;
        self.update_sequence = self.update_sequence.saturating_add(1);
        true
    }

    fn discard_obsolete_surface_state(&mut self) {
        self.mesh_chunks.clear();
        self.dirty_chunk_keys.clear();
        self.removed_chunk_keys.clear();
        self.mesh_vertex_count = 0;
        self.mesh_triangle_count = 0;
        self.pending_mesh_dirty_chunks.clear();
        self.pending_mesh_chunk_queue.clear();
        self.plane_patches.clear();
        self.pending_plane_scan_dirty_chunks.clear();
        self.pending_plane_scan_chunk_queue.clear();
    }

}

pub(super) struct CxOpenXrDepthMeshJob {
    reset_generation: u64,
    generation: u64,
    eye_index: usize,
    width: u32,
    height: u32,
    sample_step: u32,
    voxel_size_meters: f32,
    camera_world: Vec3f,
    camera_forward: Vec3f,
    depth_proj: Mat4f,
    inv_depth_proj: Mat4f,
    depth_view_from_world: Mat4f,
    world_from_depth_view: Mat4f,
    depth: Vec<u16>,
}

struct CxOpenXrPreparedDepthMeshJob {
    reset_generation: u64,
    generation: u64,
    eye_index: usize,
    width: u32,
    height: u32,
    sample_step: u32,
    voxel_size_meters: f32,
    camera_world: Vec3f,
    camera_forward: Vec3f,
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
    last_reset_generation: u64,
    last_submit_at: Option<Instant>,
    last_camera_world: Option<Vec3f>,
    last_camera_forward: Option<Vec3f>,
}

impl CxOpenXrDepthMeshPipeline {
    pub fn new() -> Self {
        let store = xr_depth_mesh_store();
        let busy = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = channel::<CxOpenXrDepthMeshJob>();
        std::thread::spawn({
            let busy = busy.clone();
            let store = store.clone();
            move || depth_preprocess_tsdf_writer_worker(receiver, busy, store)
        });
        Self {
            sender,
            busy,
            store,
            next_generation: 1,
            last_reset_generation: 0,
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
        let reset_generation = self.store.reset_generation();
        if self.last_reset_generation != reset_generation {
            self.last_reset_generation = reset_generation;
            self.last_submit_at = None;
            self.last_camera_world = None;
            self.last_camera_forward = None;
        }
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
        let voxel_size_meters = self.store.voxel_size_meters();

        let job_result: Result<CxOpenXrDepthMeshJob, String> = (|| {
            let depth = vulkan.read_openxr_depth_image(
                render_targets,
                depth_image_index,
                DEPTH_VOXEL_EYE_INDEX,
            )?;
            Ok(CxOpenXrDepthMeshJob {
                reset_generation,
                generation,
                eye_index: DEPTH_VOXEL_EYE_INDEX,
                width,
                height,
                sample_step: DEPTH_VOXEL_SAMPLE_STEP,
                voxel_size_meters,
                camera_world,
                camera_forward,
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

fn depth_preprocess_tsdf_writer_worker(
    receiver: Receiver<CxOpenXrDepthMeshJob>,
    busy: Arc<AtomicBool>,
    store: XrDepthMeshStore,
) {
    let mut preprocess_state = DepthPreprocessWorkerState::default();
    let mut volume = DepthMeshVolume::new(DEPTH_VOXEL_SAMPLE_STEP, store.voxel_size_meters());
    let mut next_height_map_publish_at = Instant::now();
    let mut applied_reset_generation = store.reset_generation();
    loop {
        let configured_voxel_size = store.voxel_size_meters();
        if (volume.voxel_size_meters - configured_voxel_size).abs() > f32::EPSILON {
            volume = DepthMeshVolume::new(DEPTH_VOXEL_SAMPLE_STEP, configured_voxel_size);
            next_height_map_publish_at = Instant::now();
        }
        let requested_reset_generation = store.reset_generation();
        if applied_reset_generation != requested_reset_generation {
            applied_reset_generation = requested_reset_generation;
            volume = DepthMeshVolume::new(DEPTH_VOXEL_SAMPLE_STEP, configured_voxel_size);
            next_height_map_publish_at = Instant::now();
        }
        let mut applied_update = false;
        match receiver.recv_timeout(Duration::from_millis(DEPTH_SURFACE_MESH_IDLE_WAIT_MILLIS)) {
            Ok(job) => {
                if job.reset_generation != store.reset_generation() {
                    busy.store(false, Ordering::Release);
                    continue;
                }
                if (job.voxel_size_meters - store.voxel_size_meters()).abs() > f32::EPSILON {
                    busy.store(false, Ordering::Release);
                    continue;
                }
                let result = preprocess_depth_mesh(job, &mut preprocess_state);
                busy.store(false, Ordering::Release);
                match result {
                    Ok(job) => {
                        if job.reset_generation != store.reset_generation() {
                            continue;
                        }
                        if (job.voxel_size_meters - store.voxel_size_meters()).abs() > f32::EPSILON
                        {
                            continue;
                        }
                        apply_preprocessed_depth_mesh(job, &mut volume);
                        volume.discard_obsolete_surface_state();
                        applied_update = true;
                    }
                    Err(err) => {
                        store.set_error(err);
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }

        let surface_analysis_enabled = store.surface_analysis_enabled();
        let now = Instant::now();
        if surface_analysis_enabled {
            sync_projected_height_field_layout(&mut volume);
            sync_projected_height_field_player_cutout(&mut volume);
            refresh_projected_height_field(
                &mut volume,
                DEPTH_ALIGN_PROJECTED_HEIGHT_SAMPLES_PER_TICK,
            );
        }
        let height_map_changed = if surface_analysis_enabled {
            let ready_to_publish = !volume.projected_height_layout_rebuild_pending
                && volume.pending_projected_height_sample_queue.is_empty();
            if ready_to_publish
                && (volume.published_height_map.is_none() || now >= next_height_map_publish_at)
            {
                next_height_map_publish_at =
                    now + Duration::from_millis(DEPTH_PUBLISHED_HEIGHT_MAP_INTERVAL_MILLIS);
                update_published_height_map(&mut volume)
            } else {
                false
            }
        } else {
            next_height_map_publish_at = now;
            volume.clear_published_height_map()
        };
        let snapshot_changed = applied_update || height_map_changed;
        if snapshot_changed {
            let previous_snapshot = store.latest_tsdf_snapshot();
            let snapshot = volume.published_tsdf_snapshot(previous_snapshot.as_deref());
            store.publish_tsdf_snapshot(snapshot);
            volume.clear_published_tsdf_dirty_state();
            SignalToUI::set_ui_signal();
        }
    }
}

fn preprocess_depth_mesh(
    job: CxOpenXrDepthMeshJob,
    worker_state: &mut DepthPreprocessWorkerState,
) -> Result<CxOpenXrPreparedDepthMeshJob, String> {
    rebuild_sampled_depth_grid(&job, worker_state);

    let voxel_size_meters = job.voxel_size_meters;
    let tsd_distance_meters = depth_tsd_distance_meters(voxel_size_meters);
    let mut frame_tsd_accum = HashMap::<VoxelCoord, FrameTsdSampleAccum>::new();
    let mut observed_world_min = vec3f(f32::INFINITY, f32::INFINITY, f32::INFINITY);
    let mut observed_world_max = vec3f(f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
    let sample_step = job.sample_step.max(1) as usize;
    let ray_step = (voxel_size_meters * 0.5).max(0.02);

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

            let start_distance =
                (surface_distance - tsd_distance_meters).max(DEPTH_TSD_MIN_UPDATE_DISTANCE_METERS);
            let end_distance =
                (surface_distance + tsd_distance_meters).min(DEPTH_VOXEL_MAX_DISTANCE_METERS);
            let mut last_coord = None;
            let mut distance = start_distance;
            while distance <= end_distance {
                let sample_world = job.camera_world + ray_dir.scale(distance);
                let coord = VoxelCoord::new(
                    (sample_world.x / voxel_size_meters).floor() as i32,
                    (sample_world.y / voxel_size_meters).floor() as i32,
                    (sample_world.z / voxel_size_meters).floor() as i32,
                );
                if last_coord == Some(coord) {
                    distance += ray_step;
                    continue;
                }
                last_coord = Some(coord);

                let voxel_world = vec3f(
                    (coord.x as f32 + 0.5) * voxel_size_meters,
                    (coord.y as f32 + 0.5) * voxel_size_meters,
                    (coord.z as f32 + 0.5) * voxel_size_meters,
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
                let normalized =
                    ((surface_distance - voxel_distance) / tsd_distance_meters).clamp(-1.0, 1.0);
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
            tsd_distance_meters,
            tsd_distance_meters,
            tsd_distance_meters,
        );
        (observed_world_min - padding, observed_world_max + padding)
    } else {
        depth_visible_world_bounds(&job).unwrap_or((vec3f(0.0, 0.0, 0.0), vec3f(0.0, 0.0, 0.0)))
    };

    Ok(CxOpenXrPreparedDepthMeshJob {
        reset_generation: job.reset_generation,
        generation: job.generation,
        eye_index: job.eye_index,
        width: job.width,
        height: job.height,
        sample_step: job.sample_step,
        voxel_size_meters,
        camera_world: job.camera_world,
        camera_forward: job.camera_forward,
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
    volume.latest_camera_world = Some(job.camera_world);
    volume.latest_camera_forward = Some(job.camera_forward);

    let mut topology_changes = apply_tsd_samples(volume, &job.frame_tsd_samples);
    topology_changes += refresh_visible_free_space(volume, &job);
    topology_changes += clear_player_exclusion_volume(volume, job.camera_world);
    if topology_changes != 0 {
        volume.latest_topology_generation = job.generation;
    }
    volume.update_bounds();
}

fn update_published_height_map(volume: &mut DepthMeshVolume) -> bool {
    let (_, next_height_map, _) = build_tsdf_vector_slice_preview_and_samples(volume);
    if volume.published_height_map == next_height_map {
        return false;
    }
    volume.published_height_map = next_height_map;
    volume.update_sequence = volume.update_sequence.saturating_add(1);
    true
}

fn projected_height_field_layout(volume: &DepthMeshVolume) -> Option<ProjectedHeightFieldLayout> {
    let (bounds_min, bounds_max) = volume.mesh_grid.world_bounds(1)?;
    let cell_size_meters = volume.voxel_size_meters.max(1.0e-5);
    let padding_cells =
        (DEPTH_ALIGN_HEIGHT_MAP_BOUNDS_PADDING_METERS / cell_size_meters).ceil() as i32;
    let min_cell_x = (bounds_min.x / cell_size_meters).floor() as i32 - padding_cells;
    let max_cell_x = (bounds_max.x / cell_size_meters).ceil() as i32 + padding_cells;
    let min_cell_z = (bounds_min.z / cell_size_meters).floor() as i32 - padding_cells;
    let max_cell_z = (bounds_max.z / cell_size_meters).ceil() as i32 + padding_cells;
    let size_x = (max_cell_x - min_cell_x).max(1) as usize;
    let size_z = (max_cell_z - min_cell_z).max(1) as usize;
    if size_x > u16::MAX as usize || size_z > u16::MAX as usize {
        return None;
    }
    let origin_x = min_cell_x as f32 * cell_size_meters;
    let origin_z = min_cell_z as f32 * cell_size_meters;
    let top_y_meters = vector_slice_projection_top_y(volume)?;
    let bottom_y_meters = vector_slice_projection_bottom_y(volume, top_y_meters)?;
    Some(ProjectedHeightFieldLayout {
        origin_x,
        origin_z,
        cell_size_meters,
        size_x,
        size_z,
        top_y_meters,
        bottom_y_meters,
    })
}

fn projected_height_cutout_center_matches(a: Option<Vec3f>, b: Option<Vec3f>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => (a - b).length() <= 1.0e-4,
        _ => false,
    }
}

fn enqueue_projected_height_sample_dirty(volume: &mut DepthMeshVolume, sample_index: usize) {
    if volume
        .pending_projected_height_dirty_samples
        .insert(sample_index)
    {
        volume
            .pending_projected_height_sample_queue
            .push_back(sample_index);
    }
}

fn projected_height_sample_index(sample_size_x: usize, x: usize, z: usize) -> usize {
    x + z * sample_size_x
}

fn mark_all_projected_height_samples_dirty(volume: &mut DepthMeshVolume) {
    let Some((sample_size_x, sample_size_z)) = volume
        .projected_height_field
        .as_ref()
        .map(|field| (field.sample_size_x(), field.sample_size_z()))
    else {
        return;
    };
    let swizzle = DEPTH_ALIGN_PROJECTED_HEIGHT_DIRTY_SWIZZLE.max(1);
    for z_phase in 0..swizzle {
        for x_phase in 0..swizzle {
            for z in (z_phase..sample_size_z).step_by(swizzle) {
                for x in (x_phase..sample_size_x).step_by(swizzle) {
                    enqueue_projected_height_sample_dirty(
                        volume,
                        projected_height_sample_index(sample_size_x, x, z),
                    );
                }
            }
        }
    }
}

fn projected_height_layout_sample_offset(
    source: ProjectedHeightFieldLayout,
    target: ProjectedHeightFieldLayout,
) -> Option<(isize, isize)> {
    let cell_size = source.cell_size_meters.max(1.0e-5);
    if (source.cell_size_meters - target.cell_size_meters).abs() > 1.0e-5 {
        return None;
    }
    let offset_x = (source.origin_x - target.origin_x) / cell_size;
    let offset_z = (source.origin_z - target.origin_z) / cell_size;
    let snapped_x = offset_x.round();
    let snapped_z = offset_z.round();
    ((offset_x - snapped_x).abs() <= 1.0e-4 && (offset_z - snapped_z).abs() <= 1.0e-4)
        .then_some((snapped_x as isize, snapped_z as isize))
}

fn copy_projected_height_field_overlap(
    previous: &ProjectedHeightField,
    next: &mut ProjectedHeightField,
) -> bool {
    let Some((offset_x, offset_z)) =
        projected_height_layout_sample_offset(previous.layout, next.layout)
    else {
        return false;
    };
    let previous_size_x = previous.sample_size_x();
    let previous_size_z = previous.sample_size_z();
    let next_size_x = next.sample_size_x();
    let next_size_z = next.sample_size_z();
    for previous_z in 0..previous_size_z {
        let next_z = previous_z as isize + offset_z;
        if next_z < 0 || next_z >= next_size_z as isize {
            continue;
        }
        for previous_x in 0..previous_size_x {
            let next_x = previous_x as isize + offset_x;
            if next_x < 0 || next_x >= next_size_x as isize {
                continue;
            }
            let previous_index = projected_height_sample_index(previous_size_x, previous_x, previous_z);
            let next_index =
                projected_height_sample_index(next_size_x, next_x as usize, next_z as usize);
            next.valid[next_index] = previous.valid[previous_index];
            next.heights_meters[next_index] = previous.heights_meters[previous_index];
        }
    }
    true
}

fn remap_projected_height_dirty_samples(
    previous: &ProjectedHeightField,
    next: &ProjectedHeightField,
    previous_dirty_samples: &[usize],
) -> Vec<usize> {
    let Some((offset_x, offset_z)) =
        projected_height_layout_sample_offset(previous.layout, next.layout)
    else {
        return Vec::new();
    };
    let previous_size_x = previous.sample_size_x();
    let next_size_x = next.sample_size_x();
    let next_size_z = next.sample_size_z();
    let mut remapped = Vec::new();
    for previous_index in previous_dirty_samples {
        let previous_x = previous_index % previous_size_x;
        let previous_z = previous_index / previous_size_x;
        let next_x = previous_x as isize + offset_x;
        let next_z = previous_z as isize + offset_z;
        if next_x < 0 || next_z < 0 || next_x >= next_size_x as isize || next_z >= next_size_z as isize {
            continue;
        }
        remapped.push(projected_height_sample_index(
            next_size_x,
            next_x as usize,
            next_z as usize,
        ));
    }
    remapped
}

fn sync_projected_height_field_layout(volume: &mut DepthMeshVolume) {
    let Some(layout) = projected_height_field_layout(volume) else {
        volume.projected_height_field = None;
        volume.projected_height_layout_rebuild_pending = false;
        volume.pending_projected_height_dirty_samples.clear();
        volume.pending_projected_height_sample_queue.clear();
        return;
    };
    let needs_rebuild = volume
        .projected_height_field
        .as_ref()
        .map(|field| field.layout != layout)
        .unwrap_or(true);
    if !needs_rebuild {
        return;
    }
    let previous_field = volume.projected_height_field.take();
    let previous_dirty_samples = volume
        .pending_projected_height_sample_queue
        .iter()
        .copied()
        .collect::<Vec<_>>();
    let mut next_field = ProjectedHeightField::new(layout, volume.latest_camera_world);
    let copied_overlap = previous_field
        .as_ref()
        .is_some_and(|previous_field| copy_projected_height_field_overlap(previous_field, &mut next_field));
    let remapped_dirty_samples = previous_field
        .as_ref()
        .map(|previous_field| {
            remap_projected_height_dirty_samples(
                previous_field,
                &next_field,
                &previous_dirty_samples,
            )
        })
        .unwrap_or_default();
    let missing_sample_indices = next_field
        .valid
        .iter()
        .enumerate()
        .filter_map(|(sample_index, valid)| (*valid == 0).then_some(sample_index))
        .collect::<Vec<_>>();
    volume.projected_height_field = Some(next_field);
    volume.projected_height_layout_rebuild_pending = true;
    volume.pending_projected_height_dirty_samples.clear();
    volume.pending_projected_height_sample_queue.clear();
    if copied_overlap {
        for sample_index in remapped_dirty_samples {
            enqueue_projected_height_sample_dirty(volume, sample_index);
        }
        for sample_index in missing_sample_indices {
            enqueue_projected_height_sample_dirty(volume, sample_index);
        }
    } else {
        mark_all_projected_height_samples_dirty(volume);
    }
}

fn mark_projected_height_samples_dirty_world_rect(
    volume: &mut DepthMeshVolume,
    min_x: f32,
    max_x: f32,
    min_z: f32,
    max_z: f32,
) {
    let Some((origin_x, origin_z, cell_size_meters, sample_size_x, sample_size_z)) =
        volume.projected_height_field.as_ref().map(|field| {
            (
                field.layout.origin_x,
                field.layout.origin_z,
                field.layout.cell_size_meters,
                field.sample_size_x(),
                field.sample_size_z(),
            )
        })
    else {
        return;
    };
    let max_sample_x = sample_size_x as isize - 1;
    let max_sample_z = sample_size_z as isize - 1;
    let raw_min_x = ((min_x - origin_x) / cell_size_meters.max(1.0e-5)).floor() as isize;
    let raw_max_x = ((max_x - origin_x) / cell_size_meters.max(1.0e-5)).ceil() as isize;
    let raw_min_z = ((min_z - origin_z) / cell_size_meters.max(1.0e-5)).floor() as isize;
    let raw_max_z = ((max_z - origin_z) / cell_size_meters.max(1.0e-5)).ceil() as isize;
    if raw_max_x < 0
        || raw_max_z < 0
        || raw_min_x > max_sample_x
        || raw_min_z > max_sample_z
    {
        return;
    }
    let sample_min_x = raw_min_x.clamp(0, max_sample_x) as usize;
    let sample_max_x = raw_max_x.clamp(0, max_sample_x) as usize;
    let sample_min_z = raw_min_z.clamp(0, max_sample_z) as usize;
    let sample_max_z = raw_max_z.clamp(0, max_sample_z) as usize;
    for z in sample_min_z..=sample_max_z {
        for x in sample_min_x..=sample_max_x {
            enqueue_projected_height_sample_dirty(volume, x + z * sample_size_x);
        }
    }
}

fn mark_projected_height_samples_dirty_around_voxel(
    volume: &mut DepthMeshVolume,
    voxel: VoxelCoord,
) {
    if volume.projected_height_field.is_none() {
        return;
    }
    let center = volume.mesh_grid.voxel_center_world(voxel);
    let radius = volume.voxel_size_meters;
    mark_projected_height_samples_dirty_world_rect(
        volume,
        center.x - radius,
        center.x + radius,
        center.z - radius,
        center.z + radius,
    );
}

fn mark_projected_height_samples_dirty_around_point(
    volume: &mut DepthMeshVolume,
    point: Vec3f,
    radius: f32,
) {
    mark_projected_height_samples_dirty_world_rect(
        volume,
        point.x - radius,
        point.x + radius,
        point.z - radius,
        point.z + radius,
    );
}

fn sync_projected_height_field_player_cutout(volume: &mut DepthMeshVolume) {
    let Some(previous_cutout_center) = volume
        .projected_height_field
        .as_ref()
        .map(|field| field.player_cutout_center)
    else {
        return;
    };
    let next_cutout_center = volume.latest_camera_world;
    if projected_height_cutout_center_matches(previous_cutout_center, next_cutout_center) {
        return;
    }
    let dirty_radius =
        DEPTH_ALIGN_VECTOR_SLICE_PLAYER_CUTOUT_RADIUS_METERS + volume.voxel_size_meters;
    if let Some(center) = previous_cutout_center {
        mark_projected_height_samples_dirty_around_point(volume, center, dirty_radius);
    }
    if let Some(center) = next_cutout_center {
        mark_projected_height_samples_dirty_around_point(volume, center, dirty_radius);
    }
    if let Some(field) = volume.projected_height_field.as_mut() {
        field.player_cutout_center = next_cutout_center;
    }
}

fn refresh_projected_height_field(volume: &mut DepthMeshVolume, max_samples: usize) -> bool {
    let flush_all = max_samples == usize::MAX;
    let mut processed = 0usize;
    let mut changed = false;
    loop {
        if !flush_all && processed >= max_samples {
            break;
        }
        let Some(sample_index) = volume.pending_projected_height_sample_queue.pop_front() else {
            break;
        };
        volume
            .pending_projected_height_dirty_samples
            .remove(&sample_index);
        let Some((layout, cutout_center, sample_size_x, current_height, current_valid)) =
            volume.projected_height_field.as_ref().map(|field| {
                (
                    field.layout,
                    field.player_cutout_center,
                    field.sample_size_x(),
                    field.heights_meters[sample_index],
                    field.valid[sample_index] != 0,
                )
            })
        else {
            break;
        };
        let sample_x = sample_index % sample_size_x;
        let sample_z = sample_index / sample_size_x;
        let point = vector_slice_corner_world(
            layout.origin_x,
            layout.origin_z,
            layout.cell_size_meters,
            DEPTH_ALIGN_VECTOR_SLICE_ISO_HEIGHT_METERS,
            sample_x,
            sample_z,
        );
        let (next_valid, next_height) = if cutout_center.is_some_and(|camera_world| {
            vector_slice_point_inside_player_cutout(camera_world, point)
        }) {
            (false, 0.0)
        } else if let Some(height) = query_depth_grid_projected_column_height(
            volume,
            point.x,
            point.z,
            layout.top_y_meters,
            layout.bottom_y_meters,
        ) {
            (true, height)
        } else {
            (false, 0.0)
        };
        if current_valid != next_valid
            || (next_valid && (current_height - next_height).abs() > 1.0e-4)
            || (!next_valid && current_height != 0.0)
        {
            if let Some(field) = volume.projected_height_field.as_mut() {
                field.valid[sample_index] = next_valid as u8;
                field.heights_meters[sample_index] = next_height;
            }
            changed = true;
        }
        processed += 1;
    }
    if volume.pending_projected_height_sample_queue.is_empty() {
        volume.projected_height_layout_rebuild_pending = false;
    }
    changed
}

fn vector_slice_projection_top_y(volume: &DepthMeshVolume) -> Option<f32> {
    let (bounds_min, bounds_max) = volume.mesh_grid.world_bounds(1)?;
    let min_y = bounds_min.y + volume.voxel_size_meters;
    let max_y = bounds_max.y - volume.voxel_size_meters;
    if max_y <= min_y {
        return None;
    }
    Some(DEPTH_ALIGN_VECTOR_SLICE_TOP_Y_METERS.clamp(min_y, max_y))
}

fn vector_slice_projection_bottom_y(volume: &DepthMeshVolume, top_y: f32) -> Option<f32> {
    let (bounds_min, _) = volume.mesh_grid.world_bounds(1)?;
    let min_y = (bounds_min.y + volume.voxel_size_meters)
        .max(DEPTH_ALIGN_VECTOR_SLICE_MIN_PROJECT_Y_METERS);
    let max_y = top_y - volume.voxel_size_meters;
    (max_y > min_y).then_some(min_y)
}

fn vector_slice_corner_world(
    origin_x: f32,
    origin_z: f32,
    cell_size: f32,
    y: f32,
    x: usize,
    z: usize,
) -> Vec3f {
    vec3f(
        origin_x + x as f32 * cell_size,
        y,
        origin_z + z as f32 * cell_size,
    )
}

fn vector_slice_point_inside_player_cutout(camera_world: Vec3f, point: Vec3f) -> bool {
    let dx = point.x - camera_world.x;
    let dz = point.z - camera_world.z;
    dx * dx + dz * dz
        <= DEPTH_ALIGN_VECTOR_SLICE_PLAYER_CUTOUT_RADIUS_METERS
            * DEPTH_ALIGN_VECTOR_SLICE_PLAYER_CUTOUT_RADIUS_METERS
}

fn vector_slice_cell_inside_player_cutout(
    camera_world: Vec3f,
    origin_x: f32,
    origin_z: f32,
    cell_size: f32,
    x: usize,
    z: usize,
) -> bool {
    let center = vec3f(
        origin_x + (x as f32 + 0.5) * cell_size,
        DEPTH_ALIGN_VECTOR_SLICE_ISO_HEIGHT_METERS,
        origin_z + (z as f32 + 0.5) * cell_size,
    );
    let expanded_radius = DEPTH_ALIGN_VECTOR_SLICE_PLAYER_CUTOUT_RADIUS_METERS + cell_size * 0.75;
    let dx = center.x - camera_world.x;
    let dz = center.z - camera_world.z;
    dx * dx + dz * dz <= expanded_radius * expanded_radius
}

fn vector_slice_flatten_forward(forward: Vec3f) -> Option<Vec2f> {
    let flat = vec2f(forward.x, forward.z);
    let length = flat.length();
    (length > 1.0e-5).then_some(flat * length.recip())
}

fn query_depth_grid_projected_column_height(
    volume: &DepthMeshVolume,
    sample_x: f32,
    sample_z: f32,
    top_y: f32,
    bottom_y: f32,
) -> Option<f32> {
    let top_coord = volume
        .mesh_grid
        .world_to_voxel_coord(vec3f(sample_x, top_y, sample_z))
        .y;
    let bottom_coord = volume
        .mesh_grid
        .world_to_voxel_coord(vec3f(sample_x, bottom_y, sample_z))
        .y;
    if top_coord <= bottom_coord {
        return None;
    }
    let mut had_data = false;
    let mut topmost_valid_distance = None::<f32>;
    for y_coord in (bottom_coord + 1..=top_coord).rev() {
        let Some(above) = query_grid_bilinear_distance_at_y(volume, sample_x, sample_z, y_coord)
        else {
            continue;
        };
        let Some(below) =
            query_grid_bilinear_distance_at_y(volume, sample_x, sample_z, y_coord - 1)
        else {
            continue;
        };
        had_data = true;
        topmost_valid_distance.get_or_insert(above);
        if above <= 0.0 || below > 0.0 {
            continue;
        }

        let denom = above - below;
        let blend = if denom.abs() > 1.0e-6 {
            (above / denom).clamp(0.0, 1.0)
        } else {
            0.5
        };
        let y_above = voxel_center_axis(volume.voxel_size_meters, y_coord);
        let y_below = voxel_center_axis(volume.voxel_size_meters, y_coord - 1);
        return Some(y_above + (y_below - y_above) * blend);
    }
    if topmost_valid_distance.is_some_and(|distance| distance <= 0.0) {
        Some(top_y)
    } else if had_data {
        Some(bottom_y)
    } else {
        None
    }
}

fn vector_slice_sample_nearest(
    values: &[f32],
    size_x: usize,
    size_z: usize,
    x: isize,
    z: isize,
) -> f32 {
    let x = x.clamp(0, size_x.saturating_sub(1) as isize) as usize;
    let z = z.clamp(0, size_z.saturating_sub(1) as isize) as usize;
    values[x + z * size_x]
}

fn vector_slice_sample_gradient(
    values: &[f32],
    size_x: usize,
    size_z: usize,
    x: usize,
    z: usize,
    cell_size: f32,
) -> Vec3f {
    let x = x as isize;
    let z = z as isize;
    let dx = vector_slice_sample_nearest(values, size_x, size_z, x + 1, z)
        - vector_slice_sample_nearest(values, size_x, size_z, x - 1, z);
    let dz = vector_slice_sample_nearest(values, size_x, size_z, x, z + 1)
        - vector_slice_sample_nearest(values, size_x, size_z, x, z - 1);
    let gradient = vec3f(dx / cell_size.max(1.0e-5), 0.0, dz / cell_size.max(1.0e-5));
    let horizontal_length = gradient.length();
    if horizontal_length < DEPTH_ALIGN_VECTOR_SLICE_MIN_HORIZONTAL_NORMAL {
        vec3f(0.0, 0.0, 0.0)
    } else {
        gradient * (1.0 / horizontal_length)
    }
}

fn encode_projected_height_u16(height: f32, bottom_y: f32, top_y: f32) -> u16 {
    let span = (top_y - bottom_y).max(1.0e-5);
    let normalized = ((height - bottom_y) / span).clamp(0.0, 1.0);
    1 + (normalized * 65534.0).round() as u16
}

fn marching_squares_segment_edges(mask: u8) -> &'static [(u8, u8)] {
    match mask {
        0 | 15 => &[],
        1 => &[(3, 0)],
        2 => &[(0, 1)],
        3 => &[(3, 1)],
        4 => &[(1, 2)],
        5 => &[(3, 2), (0, 1)],
        6 => &[(0, 2)],
        7 => &[(3, 2)],
        8 => &[(2, 3)],
        9 => &[(0, 2)],
        10 => &[(0, 1), (2, 3)],
        11 => &[(1, 2)],
        12 => &[(1, 3)],
        13 => &[(0, 1)],
        14 => &[(3, 0)],
        _ => &[],
    }
}

fn marching_squares_edge_point(
    origin_x: f32,
    origin_z: f32,
    cell_size: f32,
    slice_y: f32,
    x: usize,
    z: usize,
    edge: u8,
    values: [f32; 4],
) -> Vec3f {
    let corners = [
        vector_slice_corner_world(origin_x, origin_z, cell_size, slice_y, x, z),
        vector_slice_corner_world(origin_x, origin_z, cell_size, slice_y, x + 1, z),
        vector_slice_corner_world(origin_x, origin_z, cell_size, slice_y, x + 1, z + 1),
        vector_slice_corner_world(origin_x, origin_z, cell_size, slice_y, x, z + 1),
    ];
    let (a_idx, b_idx) = match edge {
        0 => (0, 1),
        1 => (1, 2),
        2 => (3, 2),
        3 => (0, 3),
        _ => (0, 1),
    };
    let a = corners[a_idx];
    let b = corners[b_idx];
    let va = values[a_idx];
    let vb = values[b_idx];
    let t = if (vb - va).abs() > 1.0e-5 {
        (-va / (vb - va)).clamp(0.0, 1.0)
    } else {
        0.5
    };
    vec3f(a.x + (b.x - a.x) * t, slice_y, a.z + (b.z - a.z) * t)
}

fn append_projected_height_iso_samples(
    samples: &mut Vec<XrDepthAlignSample>,
    contour_height: f32,
    origin_x: f32,
    origin_z: f32,
    cell_size: f32,
    size_x: usize,
    size_z: usize,
    sample_size_x: usize,
    heights: &[f32],
    valid: &[u8],
    player_cutout_center: Option<Vec3f>,
    x: usize,
    z: usize,
) {
    let corner_indices = [
        x + z * sample_size_x,
        x + 1 + z * sample_size_x,
        x + 1 + (z + 1) * sample_size_x,
        x + (z + 1) * sample_size_x,
    ];
    if !corner_indices.iter().all(|index| valid[*index] != 0) {
        return;
    }
    let values = [
        heights[corner_indices[0]] - contour_height,
        heights[corner_indices[1]] - contour_height,
        heights[corner_indices[2]] - contour_height,
        heights[corner_indices[3]] - contour_height,
    ];
    let mask = values.iter().enumerate().fold(0u8, |mask, (bit, value)| {
        mask | (((*value >= 0.0) as u8) << bit)
    });
    if mask == 0 || mask == 15 {
        return;
    }
    for &(edge_a, edge_b) in marching_squares_segment_edges(mask) {
        let start = marching_squares_edge_point(
            origin_x,
            origin_z,
            cell_size,
            contour_height,
            x,
            z,
            edge_a,
            values,
        );
        let end = marching_squares_edge_point(
            origin_x,
            origin_z,
            cell_size,
            contour_height,
            x,
            z,
            edge_b,
            values,
        );
        let delta = end - start;
        let length = delta.length();
        if length < DEPTH_ALIGN_VECTOR_SLICE_MIN_SEGMENT_METERS {
            continue;
        }
        let tangent = delta.normalize();
        let sample_count = (((length / cell_size.max(1.0e-5))
            * DEPTH_ALIGN_VECTOR_SLICE_SAMPLES_PER_CELL)
            .ceil() as usize)
            .clamp(1, DEPTH_ALIGN_VECTOR_SLICE_MAX_SAMPLES_PER_SEGMENT);
        for sample_index in 0..sample_count {
            let t = (sample_index as f32 + 0.5) / sample_count as f32;
            let point = start + delta * t;
            if player_cutout_center.is_some_and(|camera_world| {
                vector_slice_point_inside_player_cutout(camera_world, point)
            }) {
                continue;
            }
            let sample_x = ((point.x - origin_x) / cell_size)
                .round()
                .clamp(0.0, size_x as f32) as usize;
            let sample_z = ((point.z - origin_z) / cell_size)
                .round()
                .clamp(0.0, size_z as f32) as usize;
            let gradient = vector_slice_sample_gradient(
                heights,
                size_x + 1,
                size_z + 1,
                sample_x,
                sample_z,
                cell_size,
            );
            let normal = if gradient.length() > 1.0e-5 {
                gradient
            } else {
                vec3f(-tangent.z, 0.0, tangent.x)
            };
            let weight = (length / sample_count as f32).max(cell_size * 0.35);
            samples.push(XrDepthAlignSample {
                kind: XrDepthAlignSampleKind::Wall,
                point: vec3f(point.x, contour_height, point.z),
                normal,
                weight,
            });
        }
    }
}

fn build_tsdf_vector_slice_preview_and_samples(
    volume: &mut DepthMeshVolume,
) -> (
    Option<XrDepthAlignSlicePreview>,
    Option<XrDepthAlignHeightMap>,
    Vec<XrDepthAlignSample>,
) {
    sync_projected_height_field_layout(volume);
    sync_projected_height_field_player_cutout(volume);
    let Some(field) = volume.projected_height_field.as_ref() else {
        return (None, None, Vec::new());
    };
    let origin_x = field.layout.origin_x;
    let origin_z = field.layout.origin_z;
    let cell_size = field.layout.cell_size_meters;
    let size_x = field.layout.size_x;
    let size_z = field.layout.size_z;
    let sample_size_x = field.sample_size_x();
    let heights = &field.heights_meters;
    let valid = &field.valid;
    let player_cutout_center = field.player_cutout_center;
    let player_cutout_forward = volume
        .latest_camera_forward
        .and_then(vector_slice_flatten_forward);
    let mut height_map = XrDepthAlignHeightMap {
        origin_x,
        origin_z,
        cell_size_meters: cell_size,
        size_x: size_x as u16,
        size_z: size_z as u16,
        bottom_y_meters: field.layout.bottom_y_meters,
        top_y_meters: field.layout.top_y_meters,
        player_cutout_center: player_cutout_center.map(|center| vec2f(center.x, center.z)),
        player_cutout_radius_meters: DEPTH_ALIGN_VECTOR_SLICE_PLAYER_CUTOUT_RADIUS_METERS,
        height_u16: vec![0; size_x * size_z],
    };
    let mut slice_preview = XrDepthAlignSlicePreview {
        height_map: height_map.clone(),
        cutout_center: player_cutout_center.map(|center| vec2f(center.x, center.z)),
        cutout_forward: player_cutout_forward,
        cutout_radius_meters: DEPTH_ALIGN_VECTOR_SLICE_PLAYER_CUTOUT_RADIUS_METERS,
    };
    let mut samples = Vec::<XrDepthAlignSample>::new();
    for z in 0..size_z {
        for x in 0..size_x {
            if player_cutout_center.is_some_and(|camera_world| {
                vector_slice_cell_inside_player_cutout(
                    camera_world,
                    origin_x,
                    origin_z,
                    cell_size,
                    x,
                    z,
                )
            }) {
                continue;
            }
            let corner_indices = [
                x + z * sample_size_x,
                x + 1 + z * sample_size_x,
                x + 1 + (z + 1) * sample_size_x,
                x + (z + 1) * sample_size_x,
            ];
            let preview_index = height_map.cell_index(x, z);
            let mut height_sum = 0.0;
            let mut height_count = 0usize;
            for corner_index in corner_indices {
                if valid[corner_index] == 0 {
                    continue;
                }
                height_sum += heights[corner_index];
                height_count += 1;
            }
            if height_count != 0 {
                let encoded = encode_projected_height_u16(
                    height_sum / height_count as f32,
                    field.layout.bottom_y_meters,
                    field.layout.top_y_meters,
                );
                height_map.height_u16[preview_index] = encoded;
            }
        }
    }
    slice_preview.height_map = height_map.clone();

    for z in 0..size_z {
        for x in 0..size_x {
            append_projected_height_iso_samples(
                &mut samples,
                DEPTH_ALIGN_VECTOR_SLICE_ISO_HEIGHT_METERS,
                origin_x,
                origin_z,
                cell_size,
                size_x,
                size_z,
                sample_size_x,
                heights,
                valid,
                player_cutout_center,
                x,
                z,
            );
        }
    }

    samples.sort_by(|a, b| b.weight.total_cmp(&a.weight));
    if samples.len() > DEPTH_ALIGN_MAX_WALL_SAMPLES {
        samples.truncate(DEPTH_ALIGN_MAX_WALL_SAMPLES);
    }
    (Some(slice_preview), Some(height_map), samples)
}

fn rebuild_sampled_depth_grid(
    job: &CxOpenXrDepthMeshJob,
    worker_state: &mut DepthPreprocessWorkerState,
) {
    let normal_neighbor_max_distance_delta_meters =
        depth_normal_neighbor_max_distance_delta_meters(job.voxel_size_meters);
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
                > normal_neighbor_max_distance_delta_meters
                || (sample_y.ray_distance - ray_distance).abs()
                    > normal_neighbor_max_distance_delta_meters
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
    let carve_neighbor_max_distance_delta_meters =
        depth_carve_neighbor_max_distance_delta_meters(job.voxel_size_meters);
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
        if (neighbor_distance - observed_distance).abs() <= carve_neighbor_max_distance_delta_meters
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
        let update = volume
            .mesh_grid
            .accumulate_normalized_distance(coord, normalized, volume.generation);
        if update.state_changed {
            mark_tsdf_chunk_dirty(volume, coord);
        }
        if update.value_changed {
            let current = volume
                .mesh_grid
                .normalized_distance(coord)
                .unwrap_or(previous);
            if (previous - current).abs() >= DEPTH_TSD_APPLY_DELTA_EPSILON {
                mark_projected_height_samples_dirty_around_voxel(volume, coord);
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
    let tsd_distance_meters = depth_tsd_distance_meters(job.voxel_size_meters);
    let tsd_refresh_clearance_meters = depth_tsd_refresh_clearance_meters(job.voxel_size_meters);
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
                if !observed_distance.is_finite() || clearance < tsd_refresh_clearance_meters {
                    continue;
                }
                let confidence = volume.mesh_grid.confidence(coord);
                if confidence >= DEPTH_TSD_STABLE_CONFIDENCE
                    && previous <= 0.25
                    && clearance < tsd_distance_meters
                {
                    continue;
                }
                let update = volume
                    .mesh_grid
                    .accumulate_normalized_distance(coord, 1.0, volume.generation);
                if update.state_changed {
                    mark_tsdf_chunk_dirty(volume, coord);
                }
                if update.value_changed {
                    let current = volume
                        .mesh_grid
                        .normalized_distance(coord)
                        .unwrap_or(previous);
                    if (previous - current).abs() >= DEPTH_TSD_APPLY_DELTA_EPSILON {
                        mark_projected_height_samples_dirty_around_voxel(volume, coord);
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
                let update = volume
                    .mesh_grid
                    .overwrite_normalized_distance(coord, 1.0, volume.generation);
                if update.state_changed {
                    mark_tsdf_chunk_dirty(volume, coord);
                }
                if update.value_changed {
                    mark_projected_height_samples_dirty_around_voxel(volume, coord);
                    changed += 1;
                }
            }
        }
    }
    changed
}

fn mark_tsdf_chunk_dirty(volume: &mut DepthMeshVolume, voxel: VoxelCoord) {
    let (chunk_key, _) = volume.mesh_grid.chunk_key_and_id(voxel);
    volume.dirty_tsdf_chunk_keys.insert(chunk_key);
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

fn point_inside_player_exclusion(camera_world: Vec3f, world: Vec3f) -> bool {
    let dx = world.x - camera_world.x;
    let dz = world.z - camera_world.z;
    let horizontal_sq = dx * dx + dz * dz;
    horizontal_sq <= DEPTH_PLAYER_EXCLUDE_RADIUS_METERS * DEPTH_PLAYER_EXCLUDE_RADIUS_METERS
        && world.y <= camera_world.y + DEPTH_PLAYER_EXCLUDE_TOP_METERS
        && world.y >= camera_world.y - DEPTH_PLAYER_EXCLUDE_BOTTOM_METERS
}


fn voxel_center_axis(voxel_size: f32, coord: i32) -> f32 {
    (coord as f32 + 0.5) * voxel_size
}

fn query_grid_bilinear_distance_at_y(
    volume: &DepthMeshVolume,
    sample_x: f32,
    sample_z: f32,
    y_coord: i32,
) -> Option<f32> {
    let voxel_size = volume.voxel_size_meters;
    let grid_x = sample_x / voxel_size - 0.5;
    let grid_z = sample_z / voxel_size - 0.5;
    let x0 = grid_x.floor() as i32;
    let z0 = grid_z.floor() as i32;
    let tx = grid_x - x0 as f32;
    let tz = grid_z - z0 as f32;

    let v00 = volume
        .mesh_grid
        .normalized_distance(VoxelCoord::new(x0, y_coord, z0))?;
    let v10 = volume
        .mesh_grid
        .normalized_distance(VoxelCoord::new(x0 + 1, y_coord, z0))?;
    let v01 = volume
        .mesh_grid
        .normalized_distance(VoxelCoord::new(x0, y_coord, z0 + 1))?;
    let v11 = volume
        .mesh_grid
        .normalized_distance(VoxelCoord::new(x0 + 1, y_coord, z0 + 1))?;

    let vx0 = v00 + (v10 - v00) * tx;
    let vx1 = v01 + (v11 - v01) * tx;
    Some(vx0 + (vx1 - vx0) * tz)
}
