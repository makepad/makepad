use crate::{
    makepad_math::{vec2f, vec3f, vec4f, Mat4f, Vec2f, Vec3f, Vec4f},
    os::linux::{
        openxr::CxOpenXrFrame,
        vulkan::{CxVulkan, CxVulkanOpenXrSessionData},
    },
    thread::SignalToUI,
    xr_depth_mesh::{
        empty_bounds, xr_depth_align_build_wall_feature_normal_histogram,
        xr_depth_align_loopback_preview_solution, xr_depth_align_solve_remote_to_local,
        xr_depth_align_test_markers, xr_depth_align_transform_descriptor, xr_depth_mesh_store,
        ChunkKey, XrDepthAlignDebug, XrDepthAlignDescriptor, XrDepthAlignPreview,
        XrDepthAlignSample, XrDepthAlignSampleKind, XrDepthAlignSolution,
        XrDepthAlignVerticalDescriptor, XrDepthAlignWallFeature, XrDepthMesh, XrDepthMeshChunk,
        XrDepthMeshQuery, XrDepthMeshQueryCollider, XrDepthMeshQueryColliderGeometry,
        XrDepthMeshQueryColliderRole, XrDepthMeshQueryHit, XrDepthMeshQueryResolvedSurface,
        XrDepthMeshQueryResult, XrDepthMeshQuerySupportPlane, XrDepthMeshQuerySurfaceHit,
        XrDepthMeshStore, XrDepthPlaneKind, XrDepthPlanePatch,
        XR_DEPTH_MESH_DEFAULT_VOXEL_SIZE_METERS,
    },
};
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
const DEPTH_VOXEL_SIZE_METERS: f32 = XR_DEPTH_MESH_DEFAULT_VOXEL_SIZE_METERS;
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
const DEPTH_TSD_DENSE_HOLE_FILL_MAX_PASSES: usize = 2;
const DEPTH_TSD_DENSE_HOLE_FILL_MIN_AXIS_PAIRS: usize = 2;
const DEPTH_TSD_STABLE_CONFIDENCE: u8 = 8;
const DEPTH_PLAYER_CLEAR_MAX_CONFIDENCE: u8 = 2;
const DEPTH_PLAYER_EXCLUDE_RADIUS_METERS: f32 = 0.32;
const DEPTH_PLAYER_EXCLUDE_TOP_METERS: f32 = 0.12;
const DEPTH_PLAYER_EXCLUDE_BOTTOM_METERS: f32 = 1.30;
const DEPTH_MESH_UPDATE_DISTANCE_METERS: f32 = 4.0;
const DEPTH_SURFACE_MESH_CHUNKS_PER_TICK: usize = 1;
const DEPTH_PLANE_SCAN_CHUNKS_PER_TICK: usize = 1;
const DEPTH_SURFACE_MESH_IDLE_WAIT_MILLIS: u64 = 8;
const DEPTH_QUERY_BATCH_PER_TICK: usize = 24;
const DEPTH_QUERY_MAX_SURFACES_PER_QUERY: usize = 1;
const DEPTH_DEBUG_LOG_CHUNK_MESH_TIMING: bool = false;
const DEPTH_QUERY_MIN_OPPOSING_NORMAL_DOT: f32 = 0.2;
const DEPTH_QUERY_SUPPORT_NORMAL_Y_MIN: f32 = 0.25;
const DEPTH_QUERY_LATERAL_NORMAL_Y_MAX: f32 = 0.80;
const DEPTH_QUERY_SUPPORT_DISTINCT_RADIUS_SCALE: f32 = 0.18;
const DEPTH_QUERY_SUPPORT_DISTINCT_RADIUS_MIN: f32 = 0.01;
const DEPTH_QUERY_DISTINCT_RADIUS_SCALE: f32 = 0.35;
const DEPTH_QUERY_DISTINCT_RADIUS_MIN: f32 = 0.02;
const DEPTH_QUERY_SUPPORT_PLANE_RADIUS_SCALE: f32 = 3.2;
const DEPTH_QUERY_SUPPORT_PLANE_RADIUS_MIN: f32 = 0.08;
const DEPTH_QUERY_SUPPORT_PLANE_RADIUS_MAX: f32 = 0.26;
const DEPTH_QUERY_SUPPORT_PLANE_HEIGHT_TOLERANCE_SCALE: f32 = 0.45;
const DEPTH_QUERY_SUPPORT_PLANE_HEIGHT_TOLERANCE_MIN: f32 = 0.015;
const DEPTH_QUERY_SUPPORT_PLANE_HEIGHT_TOLERANCE_MAX: f32 = 0.05;
const DEPTH_QUERY_SUPPORT_PLANE_NORMAL_DOT_MIN: f32 = 0.90;
const DEPTH_QUERY_SUPPORT_PLANE_DEBUG_HALF_EXTENT_MIN: f32 = 0.05;
const DEPTH_QUERY_TSDF_SUPPORT_GRID_DIM: usize = 5;
const DEPTH_QUERY_TSDF_SUPPORT_MAX_SAMPLES: usize =
    DEPTH_QUERY_TSDF_SUPPORT_GRID_DIM * DEPTH_QUERY_TSDF_SUPPORT_GRID_DIM;
const DEPTH_QUERY_TSDF_SUPPORT_MIN_SAMPLES: usize = 4;
const DEPTH_QUERY_TRAJECTORY_SAMPLE_COUNT: usize = 5;
const DEPTH_QUERY_TSDF_SUPPORT_NORMAL_Y_MIN: f32 = 0.60;
const DEPTH_QUERY_TSDF_SUPPORT_RADIUS_SCALE: f32 = 1.15;
const DEPTH_QUERY_TSDF_SUPPORT_RADIUS_MIN: f32 = 0.04;
const DEPTH_QUERY_TSDF_SUPPORT_RADIUS_MAX: f32 = 0.12;
const DEPTH_QUERY_TSDF_SUPPORT_EXTENT_PADDING_SCALE: f32 = 0.22;
const DEPTH_QUERY_TSDF_IMPACT_MIN_SPEED: f32 = 0.55;
const DEPTH_QUERY_TSDF_IMPACT_MIN_HORIZONTAL_SPEED: f32 = 0.40;
const DEPTH_QUERY_TSDF_IMPACT_MIN_UPWARD_SPEED: f32 = 0.55;
const DEPTH_QUERY_TSDF_IMPACT_NORMAL_Y_MAX: f32 = 0.72;
const DEPTH_QUERY_TSDF_IMPACT_CEILING_NORMAL_Y_MIN: f32 = 0.82;
const DEPTH_QUERY_TSDF_IMPACT_RAY_STEP_SCALE: f32 = 0.40;
const DEPTH_QUERY_TSDF_IMPACT_RAY_STEP_MIN: f32 = 0.02;
const DEPTH_QUERY_TSDF_IMPACT_EXTENT_SCALE: f32 = 1.20;
const DEPTH_QUERY_TSDF_IMPACT_EXTENT_MIN: f32 = 0.05;
const DEPTH_QUERY_TSDF_IMPACT_EXTENT_MAX: f32 = 0.16;
const DEPTH_QUERY_TSDF_IMPACT_RESTITUTION: f32 = 0.38;
const DEPTH_PLANE_HORIZONTAL_NORMAL_Y_MIN: f32 = 0.82;
const DEPTH_PLANE_VERTICAL_NORMAL_Y_MAX: f32 = 0.35;
const DEPTH_PLANE_SCAN_NEAR_SURFACE_BAND_SCALE: f32 = 1.25;
const DEPTH_PLANE_SCAN_NEAR_SURFACE_BAND_MIN: f32 = 0.04;
const DEPTH_PLANE_SCAN_MIN_CONFIDENCE: u8 = 6;
const DEPTH_PLANE_SCAN_VERTICAL_DISTANCE_BIN_METERS: f32 = 0.08;
const DEPTH_PLANE_SCAN_VERTICAL_NORMAL_QUANTUM: f32 = 0.14;
const DEPTH_PLANE_SCAN_MAX_RECTS_PER_GROUP: usize = 8;
const DEPTH_PLANE_SCAN_MERGE_NORMAL_DOT: f32 = 0.97;
const DEPTH_PLANE_SCAN_MERGE_DISTANCE_METERS: f32 = 0.10;
const DEPTH_PLANE_SCAN_MERGE_GAP_METERS: f32 = 0.16;
const DEPTH_PLANE_SCAN_VERTICAL_MIN_AREA_METERS2: f32 = 0.25;
const DEPTH_PLANE_SCAN_VERTICAL_MIN_DIM_METERS: f32 = 0.50;
const DEPTH_PLANE_STABLE_TRACK_MATCH_NORMAL_DOT: f32 = 0.97;
const DEPTH_PLANE_STABLE_TRACK_MATCH_DISTANCE_METERS: f32 = 0.14;
const DEPTH_PLANE_STABLE_TRACK_MATCH_ALONG_GAP_METERS: f32 = 0.40;
const DEPTH_PLANE_STABLE_TRACK_MATCH_HEIGHT_GAP_METERS: f32 = 0.35;
const DEPTH_PLANE_STABLE_TRACK_MISSING_UPDATES_MAX: u8 = 8;
const DEPTH_PLANE_STABLE_TRACK_MIN_SEEN_COUNT: u8 = 2;
const DEPTH_PLANE_STABLE_TRACK_SMOOTH_ALPHA: f32 = 0.16;
const DEPTH_ROOM_WALL_AXIS_ASSIGN_DOT_MIN: f32 = 0.90;
const DEPTH_ROOM_WALL_ORTHOGONAL_DOT_MAX: f32 = 0.45;
const DEPTH_ROOM_WALL_GROUP_DISTANCE_METERS: f32 = 0.22;
const DEPTH_ROOM_WALL_TRACK_MATCH_NORMAL_DOT: f32 = 0.96;
const DEPTH_ROOM_WALL_TRACK_MATCH_DISTANCE_METERS: f32 = 0.28;
const DEPTH_ROOM_WALL_TRACK_MISSING_UPDATES_MAX: u8 = u8::MAX;
const DEPTH_ROOM_WALL_TRACK_SMOOTH_ALPHA: f32 = 0.12;
const DEPTH_ROOM_WALL_TRACK_EXPAND_ALPHA: f32 = 0.30;
const DEPTH_ROOM_WALL_TRACK_SHRINK_ALPHA: f32 = 0.08;
const DEPTH_ROOM_WALL_HEIGHT_METERS: f32 = 2.0;
const DEPTH_ROOM_WALL_MAX_TRACKS: usize = 4;
const DEPTH_PLANE_SIMPLIFY_REGION_NORMAL_DOT: f32 = 0.95;
const DEPTH_PLANE_SIMPLIFY_REGION_DISTANCE_METERS: f32 = 0.10;
const DEPTH_PLANE_SIMPLIFY_MIN_AREA_METERS2: f32 = 0.12;
const DEPTH_PLANE_MIN_AREA_METERS2: f32 = 0.35;
const DEPTH_PLANE_MIN_DIM_METERS: f32 = 0.30;
const DEPTH_PLANE_MAX_PATCHES: usize = 24;
const DEPTH_PLANE_REGION_VERTICAL_NORMAL_DOT: f32 = 0.94;
const DEPTH_PLANE_SUPPORT_CELL_METERS: f32 = 0.12;
const DEPTH_PLANE_SUPPORT_GROW_WEIGHT: u8 = 4;
const DEPTH_PLANE_SUPPORT_MAX_WEIGHT: u8 = 10;
const DEPTH_PLANE_SUPPORT_OCCUPIED_WEIGHT: u8 = 2;
const DEPTH_MESH_PLANAR_SIMPLIFY_MIN_AREA_METERS2: f32 = 0.45;
const DEPTH_MESH_PLANAR_SIMPLIFY_MIN_RECT_AREA_METERS2: f32 = 0.12;
const DEPTH_MESH_PLANAR_SIMPLIFY_MAX_RECTS_PER_REGION: usize = 24;
const DEPTH_ENABLE_REDUCED_PLANAR_PATCHES: bool = true;
const DEPTH_ALIGN_WALL_SAMPLE_GRID_METERS: f32 = 0.24;
const DEPTH_ALIGN_WALL_HISTOGRAM_BINS: usize = 48;
const DEPTH_ALIGN_MAX_WALL_FEATURES: usize = 24;
const DEPTH_ALIGN_MAX_WALL_SAMPLES: usize = 96;
const DEPTH_ALIGN_VERTICAL_DESCRIPTOR_SIZE: usize = 64;
const DEPTH_ALIGN_VERTICAL_DESCRIPTOR_SLICES: usize = 8;
const DEPTH_ALIGN_VERTICAL_DESCRIPTOR_BOUNDS_PADDING_METERS: f32 = 0.45;
const DEPTH_ALIGN_VERTICAL_DESCRIPTOR_MAX_EXTENT_METERS: f32 = 7.0;
const DEPTH_ALIGN_VERTICAL_DESCRIPTOR_SURFACE_NORMALIZED_BAND: f32 = 0.26;
const DEPTH_ALIGN_VERTICAL_DESCRIPTOR_FREE_NORMALIZED_MIN: f32 = 0.60;
const DEPTH_ALIGN_VERTICAL_DESCRIPTOR_MIN_CONFIDENCE: u8 = 2;
const DEPTH_ALIGN_VERTICAL_DESCRIPTOR_SURFACE_Y_MIN: f32 = 0.10;
const DEPTH_ALIGN_VERTICAL_DESCRIPTOR_SURFACE_Y_MAX: f32 = 2.00;
const DEPTH_ALIGN_VERTICAL_DESCRIPTOR_FREE_Y_MIN: f32 = 0.20;
const DEPTH_ALIGN_VERTICAL_DESCRIPTOR_FREE_Y_MAX: f32 = 2.00;
const DEPTH_ALIGN_VERTICAL_DESCRIPTOR_CLUTTER_Y_MAX: f32 = 1.45;
const DEPTH_ALIGN_VERTICAL_DESCRIPTOR_VERTICAL_NORMAL_Y_MAX: f32 = 0.45;
const DEPTH_ALIGN_DESCRIPTOR_UPDATE_INTERVAL_MILLIS: u64 = 250;
const DEPTH_ALIGN_PREVIEW_SOLUTION_LERP_MIN: f32 = 0.16;
const DEPTH_ALIGN_PREVIEW_SOLUTION_LERP_MAX: f32 = 0.36;
const DEPTH_ALIGN_PREVIEW_MARKER_LERP: f32 = 0.24;
const DEPTH_ALIGN_PREVIEW_BIG_JUMP_TRANSLATION_METERS: f32 = 0.16;
const DEPTH_ALIGN_PREVIEW_BIG_JUMP_YAW_RADIANS: f32 = 0.14;
const DEPTH_ALIGN_PREVIEW_JUMP_MATCH_TRANSLATION_METERS: f32 = 0.08;
const DEPTH_ALIGN_PREVIEW_JUMP_MATCH_YAW_RADIANS: f32 = 0.08;
const DEPTH_ALIGN_PREVIEW_BIG_JUMP_CONFIRM_UPDATES: u8 = 2;
const DEPTH_ALIGN_PREVIEW_HOLD_MISSING_UPDATES: u8 = 2;

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

const fn depth_plane_vertex_link_meters(voxel_size_meters: f32) -> f32 {
    voxel_size_meters * 0.75
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

#[derive(Clone, Debug, Default)]
struct ReducedSurfaceMesh {
    mesh: SurfaceMesh32,
    planar_patches: Vec<XrDepthPlanePatch>,
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

    pub fn surface_net_chunk_mesh_with_scratch(
        &self,
        chunk_key: VoxelCoord,
        config: SparseVoxelMeshingConfig,
        current_generation: u64,
        dense: &mut Vec<f32>,
        fill_scratch: &mut Vec<f32>,
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
        repair_dense_meshing_holes(dense, fill_scratch, dense_size);
        surface_net_mesh_from_dense(dense, dense_size, self.voxel_size, start, stride)
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

fn repair_dense_meshing_holes(dense: &mut Vec<f32>, scratch: &mut Vec<f32>, extent: VoxelCoord) {
    let sx = extent.x.max(0) as usize;
    let sy = extent.y.max(0) as usize;
    let sz = extent.z.max(0) as usize;
    if sx < 3 || sy < 3 || sz < 3 || dense.len() != sx * sy * sz {
        return;
    }

    scratch.clear();
    scratch.resize(dense.len(), f32::NEG_INFINITY);

    for _ in 0..DEPTH_TSD_DENSE_HOLE_FILL_MAX_PASSES {
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

                    for axis in [
                        VoxelCoord::new(1, 0, 0),
                        VoxelCoord::new(0, 1, 0),
                        VoxelCoord::new(0, 0, 1),
                    ] {
                        let a = dense[flatten_coord(coord - axis, extent)];
                        let b = dense[flatten_coord(coord + axis, extent)];
                        if !a.is_finite() || !b.is_finite() {
                            continue;
                        }
                        pair_sum += (a + b) * 0.5;
                        pair_count += 1;
                        let a_sign = a < 0.0;
                        let b_sign = b < 0.0;
                        if a_sign == b_sign {
                            sign_vote += if a_sign { -1 } else { 1 };
                        }
                    }

                    if pair_count < DEPTH_TSD_DENSE_HOLE_FILL_MIN_AXIS_PAIRS {
                        continue;
                    }

                    let mut fill = pair_sum / pair_count as f32;
                    if fill.abs() <= 1.0e-4 && sign_vote != 0 {
                        fill = if sign_vote < 0 { -0.02 } else { 0.02 };
                    }
                    if !fill.is_finite() {
                        continue;
                    }

                    scratch[index] = fill;
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
    dirty_chunk_keys: Vec<ChunkKey>,
    removed_chunk_keys: Vec<ChunkKey>,
    mesh_vertex_count: usize,
    mesh_triangle_count: usize,
    plane_generation: u64,
    plane_patches: Vec<XrDepthPlanePatch>,
    stable_wall_tracks: Vec<StableWallPatchTrack>,
    room_wall_tracks: Vec<StableRoomWallTrack>,
    plane_scan_chunks: HashMap<ChunkKey, ScannedPlaneChunk>,
    alignment_descriptor: Option<XrDepthAlignDescriptor>,
    alignment_debug: XrDepthAlignDebug,
    alignment_preview: XrDepthAlignPreview,
    alignment_preview_temporal: AlignmentPreviewTemporalState,
    pending_mesh_dirty_chunks: HashSet<ChunkKey>,
    pending_mesh_chunk_queue: VecDeque<ChunkKey>,
    pending_plane_scan_dirty_chunks: HashSet<ChunkKey>,
    pending_plane_scan_chunk_queue: VecDeque<ChunkKey>,
}

#[derive(Clone, Debug, Default)]
struct ScannedPlaneChunk {
    patches: Vec<XrDepthPlanePatch>,
    near_surface_voxel_count: u32,
    floor_candidate_count: u32,
    wall_candidate_count: u32,
}

#[derive(Clone, Debug)]
struct StableWallPatchTrack {
    patch: XrDepthPlanePatch,
    seen_count: u8,
    missing_updates: u8,
}

#[derive(Clone, Copy, Debug, Default)]
struct RoomWallCandidate {
    normal: Vec3f,
    plane_distance: f32,
    observed_along_min: f32,
    observed_along_max: f32,
    support_area: f32,
}

#[derive(Clone, Copy, Debug)]
struct RoomWallAccumulator {
    normal: Vec3f,
    weight_sum: f32,
    plane_distance_sum: f32,
    observed_along_min: f32,
    observed_along_max: f32,
    support_area: f32,
}

impl Default for RoomWallAccumulator {
    fn default() -> Self {
        Self {
            normal: Vec3f::default(),
            weight_sum: 0.0,
            plane_distance_sum: 0.0,
            observed_along_min: f32::INFINITY,
            observed_along_max: f32::NEG_INFINITY,
            support_area: 0.0,
        }
    }
}

#[derive(Clone, Debug)]
struct StableRoomWallTrack {
    normal: Vec3f,
    plane_distance: f32,
    observed_along_min: f32,
    observed_along_max: f32,
    support_area: f32,
    seen_count: u8,
    missing_updates: u8,
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
            stable_wall_tracks: Vec::new(),
            room_wall_tracks: Vec::new(),
            plane_scan_chunks: HashMap::new(),
            alignment_descriptor: None,
            alignment_debug: XrDepthAlignDebug::default(),
            alignment_preview: XrDepthAlignPreview::default(),
            alignment_preview_temporal: AlignmentPreviewTemporalState::default(),
            pending_mesh_dirty_chunks: HashSet::new(),
            pending_mesh_chunk_queue: VecDeque::new(),
            pending_plane_scan_dirty_chunks: HashSet::new(),
            pending_plane_scan_chunk_queue: VecDeque::new(),
        }
    }

    fn reset_render_mesh_state(&mut self) {
        self.mesh_chunks.clear();
        self.mesh_generation = self.mesh_generation.saturating_add(1);
        self.update_sequence = self.update_sequence.saturating_add(1);
        self.dirty_chunk_keys.clear();
        self.removed_chunk_keys.clear();
        self.mesh_vertex_count = 0;
        self.mesh_triangle_count = 0;
        self.pending_mesh_dirty_chunks.clear();
        self.pending_mesh_chunk_queue.clear();
    }

    fn reset_plane_scan_state(&mut self) -> bool {
        let had_plane_scan = !self.plane_scan_chunks.is_empty()
            || !self.plane_patches.is_empty()
            || !self.stable_wall_tracks.is_empty()
            || !self.room_wall_tracks.is_empty()
            || !self.pending_plane_scan_dirty_chunks.is_empty()
            || !self.pending_plane_scan_chunk_queue.is_empty();
        if !had_plane_scan {
            return false;
        }
        self.plane_scan_chunks.clear();
        self.plane_patches.clear();
        self.stable_wall_tracks.clear();
        self.room_wall_tracks.clear();
        self.pending_plane_scan_dirty_chunks.clear();
        self.pending_plane_scan_chunk_queue.clear();
        self.plane_generation = self.plane_generation.saturating_add(1);
        self.update_sequence = self.update_sequence.saturating_add(1);
        true
    }

    fn clear_alignment_descriptor(&mut self) -> bool {
        if self.alignment_descriptor.is_none()
            && self.alignment_debug == XrDepthAlignDebug::default()
        {
            return false;
        }
        self.alignment_descriptor = None;
        self.alignment_debug = XrDepthAlignDebug::default();
        self.update_sequence = self.update_sequence.saturating_add(1);
        true
    }

    fn clear_alignment_preview(&mut self) -> bool {
        if self.alignment_preview == XrDepthAlignPreview::default()
            && self
                .alignment_preview_temporal
                .pending_jump_solution
                .is_none()
            && self.alignment_preview_temporal.pending_jump_updates == 0
            && self.alignment_preview_temporal.missing_solution_updates == 0
        {
            return false;
        }
        self.alignment_preview = XrDepthAlignPreview::default();
        self.alignment_preview_temporal = AlignmentPreviewTemporalState::default();
        self.update_sequence = self.update_sequence.saturating_add(1);
        true
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
            plane_patches: self.plane_patches.clone(),
            alignment_descriptor: self.alignment_descriptor.clone(),
            alignment_debug: self.alignment_debug,
            alignment_preview: self.alignment_preview.clone(),
            dirty_chunk_keys: self.dirty_chunk_keys.clone(),
            removed_chunk_keys: self.removed_chunk_keys.clone(),
            mesh_generation: self.mesh_generation,
            mesh_vertex_count: self.mesh_vertex_count,
            mesh_triangle_count: self.mesh_triangle_count,
        }
    }

    fn record_dirty_chunk(&mut self, chunk_key: ChunkKey) {
        push_unique_chunk_key(&mut self.dirty_chunk_keys, chunk_key);
        self.removed_chunk_keys.retain(|key| *key != chunk_key);
    }

    fn record_removed_chunk(&mut self, chunk_key: ChunkKey) {
        push_unique_chunk_key(&mut self.removed_chunk_keys, chunk_key);
        self.dirty_chunk_keys.retain(|key| *key != chunk_key);
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
    mesh_fill_scratch: Vec<f32>,
}

fn depth_preprocess_worker(
    receiver: Receiver<CxOpenXrDepthMeshJob>,
    sender: Sender<CxOpenXrPreparedDepthMeshJob>,
    busy: Arc<AtomicBool>,
    store: XrDepthMeshStore,
) {
    let mut worker_state = DepthPreprocessWorkerState::default();
    while let Ok(job) = receiver.recv() {
        if job.reset_generation != store.reset_generation() {
            busy.store(false, Ordering::Release);
            continue;
        }
        if (job.voxel_size_meters - store.voxel_size_meters()).abs() > f32::EPSILON {
            busy.store(false, Ordering::Release);
            continue;
        }
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
    let mut volume = DepthMeshVolume::new(DEPTH_VOXEL_SAMPLE_STEP, store.voxel_size_meters());
    let mut next_alignment_descriptor_update_at = Instant::now();
    let mut applied_reset_generation = store.reset_generation();
    loop {
        let configured_voxel_size = store.voxel_size_meters();
        if (volume.voxel_size_meters - configured_voxel_size).abs() > f32::EPSILON {
            volume = DepthMeshVolume::new(DEPTH_VOXEL_SAMPLE_STEP, configured_voxel_size);
            next_alignment_descriptor_update_at = Instant::now();
        }
        let requested_reset_generation = store.reset_generation();
        if applied_reset_generation != requested_reset_generation {
            applied_reset_generation = requested_reset_generation;
            volume = DepthMeshVolume::new(DEPTH_VOXEL_SAMPLE_STEP, configured_voxel_size);
            next_alignment_descriptor_update_at = Instant::now();
        }
        let mut applied_update = false;
        match receiver.recv_timeout(Duration::from_millis(DEPTH_SURFACE_MESH_IDLE_WAIT_MILLIS)) {
            Ok(mut job) => {
                while let Ok(newer) = receiver.try_recv() {
                    job = newer;
                }
                if job.reset_generation != store.reset_generation() {
                    continue;
                }
                let configured_voxel_size = store.voxel_size_meters();
                if (job.voxel_size_meters - configured_voxel_size).abs() > f32::EPSILON {
                    continue;
                }
                apply_preprocessed_depth_mesh(job, &mut volume);
                applied_update = true;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }

        let query_changed = process_geometry_queries(&volume, &store, DEPTH_QUERY_BATCH_PER_TICK);
        let queries_pending = store.has_pending_queries();
        let mesh_enabled = store.mesh_enabled();
        let plane_scan_enabled = store.plane_scan_enabled();
        let surface_analysis_enabled = store.surface_analysis_enabled();
        let alignment_preview_enabled = store.alignment_preview_enabled();
        let now = Instant::now();
        let mesh_changed = if mesh_enabled {
            process_incremental_surface_mesh(
                &mut volume,
                &mut worker_state,
                DEPTH_SURFACE_MESH_CHUNKS_PER_TICK,
            )
        } else if !volume.mesh_chunks.is_empty()
            || !volume.pending_mesh_dirty_chunks.is_empty()
            || !volume.pending_mesh_chunk_queue.is_empty()
        {
            volume.reset_render_mesh_state();
            true
        } else {
            false
        };
        let plane_changed = if plane_scan_enabled {
            process_incremental_tsdf_plane_scan(&mut volume, DEPTH_PLANE_SCAN_CHUNKS_PER_TICK)
        } else if volume.reset_plane_scan_state() {
            true
        } else if mesh_enabled {
            update_reduced_planar_patches(&mut volume, mesh_changed)
        } else if !volume.plane_patches.is_empty() || !volume.room_wall_tracks.is_empty() {
            volume.plane_patches.clear();
            volume.room_wall_tracks.clear();
            volume.plane_generation = volume.plane_generation.saturating_add(1);
            volume.update_sequence = volume.update_sequence.saturating_add(1);
            true
        } else {
            false
        };
        let alignment_changed = if surface_analysis_enabled {
            if plane_changed
                || (!applied_update
                    && !queries_pending
                    && now >= next_alignment_descriptor_update_at)
            {
                next_alignment_descriptor_update_at =
                    now + Duration::from_millis(DEPTH_ALIGN_DESCRIPTOR_UPDATE_INTERVAL_MILLIS);
                update_tsdf_alignment_descriptor(&mut volume)
            } else {
                false
            }
        } else {
            next_alignment_descriptor_update_at = now;
            volume.clear_alignment_descriptor()
        };
        let preview_changed = if alignment_preview_enabled && surface_analysis_enabled {
            if alignment_changed
                || (volume.alignment_descriptor.is_some()
                    && volume.alignment_preview == XrDepthAlignPreview::default())
            {
                update_alignment_preview(&mut volume)
            } else {
                false
            }
        } else {
            volume.clear_alignment_preview()
        };
        if (mesh_enabled && applied_update)
            || mesh_changed
            || plane_changed
            || alignment_changed
            || preview_changed
            || query_changed
        {
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

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct AlignmentCandidateSample {
    kind: XrDepthAlignSampleKind,
    point: Vec3f,
    normal: Vec3f,
    weight: f32,
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
struct AlignmentWallKey {
    x: i32,
    y: i32,
    z: i32,
}

#[derive(Clone, Copy, Debug, Default)]
struct AlignmentPreviewTemporalState {
    pending_jump_solution: Option<XrDepthAlignSolution>,
    pending_jump_updates: u8,
    missing_solution_updates: u8,
}

fn update_tsdf_alignment_descriptor(volume: &mut DepthMeshVolume) -> bool {
    let (next_descriptor, next_debug) = build_tsdf_alignment_descriptor(volume);
    if volume.alignment_descriptor == next_descriptor && volume.alignment_debug == next_debug {
        return false;
    }
    volume.alignment_descriptor = next_descriptor;
    volume.alignment_debug = next_debug;
    volume.update_sequence = volume.update_sequence.saturating_add(1);
    true
}

fn update_alignment_preview(volume: &mut DepthMeshVolume) -> bool {
    let next_preview = volume
        .alignment_descriptor
        .as_ref()
        .map(build_alignment_preview)
        .unwrap_or_default();
    let next_preview = stabilize_alignment_preview(
        &volume.alignment_preview,
        next_preview,
        &mut volume.alignment_preview_temporal,
    );
    if volume.alignment_preview == next_preview {
        return false;
    }
    volume.alignment_preview = next_preview;
    volume.update_sequence = volume.update_sequence.saturating_add(1);
    true
}

fn build_alignment_preview(local: &XrDepthAlignDescriptor) -> XrDepthAlignPreview {
    let (local_floor_sample_count, local_wall_sample_count) = descriptor_sample_counts(local);
    let mut preview = XrDepthAlignPreview {
        local_markers: xr_depth_align_test_markers(local),
        local_sample_count: local.samples.len(),
        local_floor_sample_count,
        local_wall_sample_count,
        ..XrDepthAlignPreview::default()
    };

    let remote = build_loopback_preview_remote_descriptor(local);
    let (remote_floor_sample_count, remote_wall_sample_count) = descriptor_sample_counts(&remote);
    preview.remote_sample_count = remote.samples.len();
    preview.remote_floor_sample_count = remote_floor_sample_count;
    preview.remote_wall_sample_count = remote_wall_sample_count;

    let remote_markers = xr_depth_align_test_markers(&remote);
    let solution = xr_depth_align_solve_remote_to_local(local, &remote);
    if let (Some(local_markers), Some(remote_markers), Some(solution)) =
        (preview.local_markers, remote_markers, solution)
    {
        let mapped_remote = [
            solution.map_point(remote_markers[0]),
            solution.map_point(remote_markers[1]),
        ];
        let (remote_markers_local, _) = best_marker_assignment(local_markers, mapped_remote);
        preview.remote_markers_local = Some(remote_markers_local);
        preview.solution = Some(solution);
    }

    preview
}

fn stabilize_alignment_preview(
    previous: &XrDepthAlignPreview,
    mut next: XrDepthAlignPreview,
    temporal: &mut AlignmentPreviewTemporalState,
) -> XrDepthAlignPreview {
    next.local_markers = smooth_marker_pair(
        previous.local_markers,
        next.local_markers,
        DEPTH_ALIGN_PREVIEW_MARKER_LERP,
        false,
    );

    let mut hold_previous_remote = false;
    let mut remote_alpha = DEPTH_ALIGN_PREVIEW_MARKER_LERP;

    match (previous.solution, next.solution) {
        (Some(previous_solution), Some(next_solution)) => {
            temporal.missing_solution_updates = 0;
            let position_jump =
                (next_solution.translation - previous_solution.translation).length();
            let yaw_jump =
                wrap_preview_angle(next_solution.yaw_radians - previous_solution.yaw_radians).abs();
            let big_jump = position_jump > DEPTH_ALIGN_PREVIEW_BIG_JUMP_TRANSLATION_METERS
                || yaw_jump > DEPTH_ALIGN_PREVIEW_BIG_JUMP_YAW_RADIANS;
            let should_gate_jump = big_jump
                && previous_solution.confidence >= 0.20
                && previous_solution.matched_samples >= 6;

            if should_gate_jump {
                if temporal
                    .pending_jump_solution
                    .is_some_and(|pending| preview_solution_near(&pending, &next_solution))
                {
                    temporal.pending_jump_updates = temporal
                        .pending_jump_updates
                        .saturating_add(1)
                        .min(DEPTH_ALIGN_PREVIEW_BIG_JUMP_CONFIRM_UPDATES);
                } else {
                    temporal.pending_jump_solution = Some(next_solution);
                    temporal.pending_jump_updates = 1;
                }

                if temporal.pending_jump_updates < DEPTH_ALIGN_PREVIEW_BIG_JUMP_CONFIRM_UPDATES {
                    next.solution = Some(previous_solution);
                    hold_previous_remote = true;
                } else {
                    temporal.pending_jump_solution = None;
                    temporal.pending_jump_updates = 0;
                    let alpha = solution_lerp_alpha(&next_solution);
                    next.solution = Some(lerp_alignment_solution(
                        previous_solution,
                        next_solution,
                        alpha,
                    ));
                    remote_alpha = alpha.max(DEPTH_ALIGN_PREVIEW_MARKER_LERP);
                }
            } else {
                temporal.pending_jump_solution = None;
                temporal.pending_jump_updates = 0;
                let alpha = solution_lerp_alpha(&next_solution);
                next.solution = Some(lerp_alignment_solution(
                    previous_solution,
                    next_solution,
                    alpha,
                ));
                remote_alpha = alpha.max(DEPTH_ALIGN_PREVIEW_MARKER_LERP);
            }
        }
        (Some(previous_solution), None) => {
            temporal.pending_jump_solution = None;
            temporal.pending_jump_updates = 0;
            if temporal.missing_solution_updates < DEPTH_ALIGN_PREVIEW_HOLD_MISSING_UPDATES {
                temporal.missing_solution_updates =
                    temporal.missing_solution_updates.saturating_add(1);
                next.solution = Some(XrDepthAlignSolution {
                    confidence: previous_solution.confidence * 0.94,
                    symmetry_confidence: previous_solution.symmetry_confidence,
                    ..previous_solution
                });
                hold_previous_remote = true;
            } else {
                temporal.missing_solution_updates = 0;
            }
        }
        (None, Some(_)) => {
            temporal.pending_jump_solution = None;
            temporal.pending_jump_updates = 0;
            temporal.missing_solution_updates = 0;
        }
        (None, None) => {
            temporal.pending_jump_solution = None;
            temporal.pending_jump_updates = 0;
            temporal.missing_solution_updates = 0;
        }
    }

    next.remote_markers_local = smooth_marker_pair(
        previous.remote_markers_local,
        if hold_previous_remote {
            None
        } else {
            next.remote_markers_local
        },
        remote_alpha,
        hold_previous_remote,
    );
    next
}

fn solution_lerp_alpha(solution: &XrDepthAlignSolution) -> f32 {
    let confidence = solution.confidence.clamp(0.0, 1.0);
    let residual = if solution.residual_meters.is_finite() {
        (1.0_f32 - solution.residual_meters / 0.28_f32).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let matched = (solution.matched_samples as f32 / 16.0).clamp(0.0, 1.0);
    let quality = (confidence * 0.60 + residual * 0.20 + matched * 0.20).clamp(0.0, 1.0);
    DEPTH_ALIGN_PREVIEW_SOLUTION_LERP_MIN
        + (DEPTH_ALIGN_PREVIEW_SOLUTION_LERP_MAX - DEPTH_ALIGN_PREVIEW_SOLUTION_LERP_MIN) * quality
}

fn lerp_alignment_solution(
    previous: XrDepthAlignSolution,
    next: XrDepthAlignSolution,
    alpha: f32,
) -> XrDepthAlignSolution {
    let alpha = alpha.clamp(0.0, 1.0);
    let yaw_delta = wrap_preview_angle(next.yaw_radians - previous.yaw_radians);
    XrDepthAlignSolution {
        yaw_radians: wrap_preview_angle(previous.yaw_radians + yaw_delta * alpha),
        translation: previous.translation + (next.translation - previous.translation) * alpha,
        confidence: previous.confidence + (next.confidence - previous.confidence) * alpha,
        symmetry_confidence: previous.symmetry_confidence
            + (next.symmetry_confidence - previous.symmetry_confidence) * alpha,
        residual_meters: if previous.residual_meters.is_finite() && next.residual_meters.is_finite()
        {
            previous.residual_meters + (next.residual_meters - previous.residual_meters) * alpha
        } else {
            next.residual_meters
        },
        matched_samples: previous.matched_samples.max(next.matched_samples),
    }
}

fn preview_solution_near(previous: &XrDepthAlignSolution, next: &XrDepthAlignSolution) -> bool {
    (next.translation - previous.translation).length()
        <= DEPTH_ALIGN_PREVIEW_JUMP_MATCH_TRANSLATION_METERS
        && wrap_preview_angle(next.yaw_radians - previous.yaw_radians).abs()
            <= DEPTH_ALIGN_PREVIEW_JUMP_MATCH_YAW_RADIANS
}

fn smooth_marker_pair(
    previous: Option<[Vec3f; 2]>,
    next: Option<[Vec3f; 2]>,
    alpha: f32,
    hold_previous: bool,
) -> Option<[Vec3f; 2]> {
    match (previous, next) {
        (Some(previous), Some(next)) => {
            let (next, _) = best_marker_assignment(previous, next);
            Some([
                previous[0] + (next[0] - previous[0]) * alpha.clamp(0.0, 1.0),
                previous[1] + (next[1] - previous[1]) * alpha.clamp(0.0, 1.0),
            ])
        }
        (Some(previous), None) if hold_previous => Some(previous),
        (_, next) => next,
    }
}

fn wrap_preview_angle(mut angle: f32) -> f32 {
    while angle <= -std::f32::consts::PI {
        angle += std::f32::consts::TAU;
    }
    while angle > std::f32::consts::PI {
        angle -= std::f32::consts::TAU;
    }
    angle
}

fn build_loopback_preview_remote_descriptor(
    local: &XrDepthAlignDescriptor,
) -> XrDepthAlignDescriptor {
    let local_to_remote = loopback_preview_remote_to_local_transform().invert();
    let mut remote = xr_depth_align_transform_descriptor(local, &local_to_remote);
    let quant_step = local.voxel_size_meters.max(0.03) * 0.65;
    let mut degraded = Vec::new();
    for (index, mut sample) in remote.samples.into_iter().enumerate() {
        let keep = match sample.kind {
            XrDepthAlignSampleKind::Floor => true,
            XrDepthAlignSampleKind::Wall => index % 4 != 1,
            XrDepthAlignSampleKind::Unknown => false,
        };
        if !keep {
            continue;
        }
        match sample.kind {
            XrDepthAlignSampleKind::Floor => {
                sample.point.x += ((index % 5) as f32 - 2.0) * quant_step * 0.18;
                sample.point.z += (((index * 3) % 7) as f32 - 3.0) * quant_step * 0.12;
                sample.point.x = quantize_metric(sample.point.x, quant_step);
                sample.point.z = quantize_metric(sample.point.z, quant_step);
            }
            XrDepthAlignSampleKind::Wall => {
                let tangent = wall_tangent(sample.normal);
                let vertical = vec3f(0.0, 1.0, 0.0);
                let tangent_jitter = ((index % 5) as f32 - 2.0) * quant_step * 0.15;
                let vertical_jitter = (((index * 3) % 7) as f32 - 3.0) * quant_step * 0.10;
                sample.point += tangent * tangent_jitter + vertical * vertical_jitter;
                sample.point.x = quantize_metric(sample.point.x, quant_step);
                sample.point.y = quantize_metric(sample.point.y, quant_step);
                sample.point.z = quantize_metric(sample.point.z, quant_step);
            }
            XrDepthAlignSampleKind::Unknown => {}
        }
        sample.weight = (sample.weight * (0.84 + 0.04 * (index % 4) as f32)).clamp(0.1, 1.0);
        degraded.push(sample);
    }
    remote.samples = degraded;
    remote.wall_normal_histogram = if remote.wall_features.is_empty() {
        Vec::new()
    } else {
        xr_depth_align_build_wall_feature_normal_histogram(
            &remote.wall_features,
            local.wall_normal_histogram.len(),
        )
    };
    remote
}

fn loopback_preview_remote_to_local_transform() -> Mat4f {
    xr_depth_align_loopback_preview_solution().remote_to_local_transform()
}

fn descriptor_sample_counts(descriptor: &XrDepthAlignDescriptor) -> (usize, usize) {
    let mut floor_count = 0usize;
    let mut wall_count = 0usize;
    for sample in &descriptor.samples {
        match sample.kind {
            XrDepthAlignSampleKind::Floor => floor_count += 1,
            XrDepthAlignSampleKind::Wall => wall_count += 1,
            XrDepthAlignSampleKind::Unknown => {}
        }
    }
    (floor_count, wall_count)
}

fn best_marker_assignment(local: [Vec3f; 2], remote: [Vec3f; 2]) -> ([Vec3f; 2], f32) {
    let direct = (remote[0] - local[0]).length() + (remote[1] - local[1]).length();
    let swapped = (remote[1] - local[0]).length() + (remote[0] - local[1]).length();
    if swapped < direct {
        ([remote[1], remote[0]], swapped * 50.0)
    } else {
        (remote, direct * 50.0)
    }
}

fn wall_tangent(normal: Vec3f) -> Vec3f {
    let tangent = vec3f(-normal.z, 0.0, normal.x);
    if tangent.length() > 1.0e-5 {
        tangent.normalize()
    } else {
        vec3f(1.0, 0.0, 0.0)
    }
}

fn vertical_descriptor_bin(y: f32, min_y: f32, max_y: f32) -> Option<u8> {
    if !y.is_finite() || y < min_y || y > max_y {
        return None;
    }
    let normalized = ((y - min_y) / (max_y - min_y).max(1.0e-5)).clamp(0.0, 0.999_99);
    Some((normalized * DEPTH_ALIGN_VERTICAL_DESCRIPTOR_SLICES as f32).floor() as u8)
}

fn vertical_descriptor_height_u8(y: f32) -> u8 {
    let normalized = ((y - DEPTH_ALIGN_VERTICAL_DESCRIPTOR_SURFACE_Y_MIN)
        / (DEPTH_ALIGN_VERTICAL_DESCRIPTOR_SURFACE_Y_MAX
            - DEPTH_ALIGN_VERTICAL_DESCRIPTOR_SURFACE_Y_MIN)
            .max(1.0e-5))
    .clamp(0.0, 1.0);
    (normalized * 255.0).round() as u8
}

fn tsdf_surface_gradient(volume: &DepthMeshVolume, coord: VoxelCoord) -> Option<Vec3f> {
    let sample_axis = |dx: i32, dy: i32, dz: i32| -> Option<f32> {
        volume
            .mesh_grid
            .normalized_distance(VoxelCoord::new(coord.x + dx, coord.y + dy, coord.z + dz))
            .filter(|value| value.is_finite())
    };
    let x_plus = sample_axis(1, 0, 0)?;
    let x_minus = sample_axis(-1, 0, 0)?;
    let y_plus = sample_axis(0, 1, 0)?;
    let y_minus = sample_axis(0, -1, 0)?;
    let z_plus = sample_axis(0, 0, 1)?;
    let z_minus = sample_axis(0, 0, -1)?;
    let gradient = vec3f(x_plus - x_minus, y_plus - y_minus, z_plus - z_minus);
    (gradient.length() > 1.0e-5).then_some(gradient.normalize())
}

fn build_tsdf_vertical_descriptor(
    volume: &DepthMeshVolume,
) -> Option<XrDepthAlignVerticalDescriptor> {
    let (bounds_min, bounds_max) = volume.mesh_grid.world_bounds(1)?;
    let mut extent = (bounds_max.x - bounds_min.x)
        .max(bounds_max.z - bounds_min.z)
        .max(volume.voxel_size_meters * DEPTH_ALIGN_VERTICAL_DESCRIPTOR_SIZE as f32);
    extent = (extent + DEPTH_ALIGN_VERTICAL_DESCRIPTOR_BOUNDS_PADDING_METERS * 2.0)
        .min(DEPTH_ALIGN_VERTICAL_DESCRIPTOR_MAX_EXTENT_METERS);
    let center_x = (bounds_min.x + bounds_max.x) * 0.5;
    let center_z = (bounds_min.z + bounds_max.z) * 0.5;
    let origin_x = center_x - extent * 0.5;
    let origin_z = center_z - extent * 0.5;
    let cell_size = extent / DEPTH_ALIGN_VERTICAL_DESCRIPTOR_SIZE as f32;
    let cell_count = DEPTH_ALIGN_VERTICAL_DESCRIPTOR_SIZE * DEPTH_ALIGN_VERTICAL_DESCRIPTOR_SIZE;
    let mut descriptor = XrDepthAlignVerticalDescriptor {
        origin_x,
        origin_z,
        cell_size_meters: cell_size,
        size: DEPTH_ALIGN_VERTICAL_DESCRIPTOR_SIZE as u16,
        vertical_surface_masks: vec![0; cell_count],
        clutter_surface_masks: vec![0; cell_count],
        free_space_masks: vec![0; cell_count],
        height_u8: vec![0; cell_count],
    };

    for (chunk_key, chunk) in &volume.mesh_grid.chunks {
        if chunk.live_count == 0 {
            continue;
        }
        let edge = volume.mesh_grid.chunk_edge as usize;
        for local_z in 0..edge {
            for local_y in 0..edge {
                for local_x in 0..edge {
                    let id = local_x + local_y * edge + local_z * edge * edge;
                    if chunk.valid[id] == 0
                        || chunk.confidence[id] < DEPTH_ALIGN_VERTICAL_DESCRIPTOR_MIN_CONFIDENCE
                    {
                        continue;
                    }
                    let coord = VoxelCoord::new(
                        chunk_key.x * volume.mesh_grid.chunk_edge + local_x as i32,
                        chunk_key.y * volume.mesh_grid.chunk_edge + local_y as i32,
                        chunk_key.z * volume.mesh_grid.chunk_edge + local_z as i32,
                    );
                    let world = volume.mesh_grid.voxel_center_world(coord);
                    let grid_x = ((world.x - origin_x) / cell_size).floor() as isize;
                    let grid_z = ((world.z - origin_z) / cell_size).floor() as isize;
                    if grid_x < 0
                        || grid_z < 0
                        || grid_x >= DEPTH_ALIGN_VERTICAL_DESCRIPTOR_SIZE as isize
                        || grid_z >= DEPTH_ALIGN_VERTICAL_DESCRIPTOR_SIZE as isize
                    {
                        continue;
                    }
                    let index =
                        grid_x as usize + grid_z as usize * DEPTH_ALIGN_VERTICAL_DESCRIPTOR_SIZE;
                    let normalized_distance = chunk.values[id];

                    if normalized_distance >= DEPTH_ALIGN_VERTICAL_DESCRIPTOR_FREE_NORMALIZED_MIN {
                        if let Some(bin) = vertical_descriptor_bin(
                            world.y,
                            DEPTH_ALIGN_VERTICAL_DESCRIPTOR_FREE_Y_MIN,
                            DEPTH_ALIGN_VERTICAL_DESCRIPTOR_FREE_Y_MAX,
                        ) {
                            descriptor.free_space_masks[index] |= 1u8 << bin;
                        }
                    }

                    if world.y < DEPTH_ALIGN_VERTICAL_DESCRIPTOR_SURFACE_Y_MIN
                        || world.y > DEPTH_ALIGN_VERTICAL_DESCRIPTOR_SURFACE_Y_MAX
                        || normalized_distance.abs()
                            > DEPTH_ALIGN_VERTICAL_DESCRIPTOR_SURFACE_NORMALIZED_BAND
                    {
                        continue;
                    }
                    let Some(bin) = vertical_descriptor_bin(
                        world.y,
                        DEPTH_ALIGN_VERTICAL_DESCRIPTOR_SURFACE_Y_MIN,
                        DEPTH_ALIGN_VERTICAL_DESCRIPTOR_SURFACE_Y_MAX,
                    ) else {
                        continue;
                    };
                    descriptor.height_u8[index] =
                        descriptor.height_u8[index].max(vertical_descriptor_height_u8(world.y));
                    let Some(normal) = tsdf_surface_gradient(volume, coord) else {
                        descriptor.clutter_surface_masks[index] |= 1u8 << bin;
                        continue;
                    };
                    if normal.y.abs() <= DEPTH_ALIGN_VERTICAL_DESCRIPTOR_VERTICAL_NORMAL_Y_MAX {
                        descriptor.vertical_surface_masks[index] |= 1u8 << bin;
                    } else if world.y <= DEPTH_ALIGN_VERTICAL_DESCRIPTOR_CLUTTER_Y_MAX {
                        descriptor.clutter_surface_masks[index] |= 1u8 << bin;
                    }
                }
            }
        }
    }

    (!descriptor.is_empty()).then_some(descriptor)
}

fn build_tsdf_alignment_descriptor(
    volume: &DepthMeshVolume,
) -> (Option<XrDepthAlignDescriptor>, XrDepthAlignDebug) {
    let mut debug = XrDepthAlignDebug::default();
    for chunk in volume.plane_scan_chunks.values() {
        debug.near_surface_voxel_count = debug
            .near_surface_voxel_count
            .saturating_add(chunk.near_surface_voxel_count);
        debug.floor_candidate_count = debug
            .floor_candidate_count
            .saturating_add(chunk.floor_candidate_count);
        debug.wall_candidate_count = debug
            .wall_candidate_count
            .saturating_add(chunk.wall_candidate_count);
    }
    let wall_patches = visible_stable_wall_patches(&volume.stable_wall_tracks);
    let wall_features = visible_room_wall_features(&volume.room_wall_tracks);
    let vertical_descriptor = build_tsdf_vertical_descriptor(volume);
    if wall_patches.is_empty() && wall_features.is_empty() {
        return (None, debug);
    }

    let mut wall_samples = HashMap::<AlignmentWallKey, AlignmentCandidateSample>::new();
    for patch in &wall_patches {
        append_wall_patch_alignment_samples(&mut wall_samples, patch);
    }

    let mut samples = wall_samples
        .into_values()
        .map(quantized_align_sample)
        .collect::<Vec<_>>();
    samples.sort_by(|a, b| b.weight.total_cmp(&a.weight));
    if samples.len() > DEPTH_ALIGN_MAX_WALL_SAMPLES {
        samples.truncate(DEPTH_ALIGN_MAX_WALL_SAMPLES);
    }
    debug.wall_sample_count = samples.len() as u32;
    if wall_features.is_empty() {
        return (None, debug);
    }
    debug.wall_candidate_count = debug.wall_candidate_count.max(wall_features.len() as u32);
    let wall_histogram = xr_depth_align_build_wall_feature_normal_histogram(
        &wall_features,
        DEPTH_ALIGN_WALL_HISTOGRAM_BINS,
    );

    (
        Some(XrDepthAlignDescriptor {
            voxel_size_meters: volume.voxel_size_meters,
            floor_y: 0.0,
            wall_normal_histogram: wall_histogram,
            wall_features,
            samples: Vec::new(),
            vertical_descriptor,
        }),
        debug,
    )
}

fn append_wall_patch_alignment_samples(
    wall_samples: &mut HashMap<AlignmentWallKey, AlignmentCandidateSample>,
    patch: &XrDepthPlanePatch,
) {
    for_each_plane_patch_grid_sample(
        patch,
        DEPTH_ALIGN_WALL_SAMPLE_GRID_METERS,
        |point, edge_bias| {
            let key = AlignmentWallKey {
                x: quantize_f32(point.x, DEPTH_ALIGN_WALL_SAMPLE_GRID_METERS),
                y: quantize_f32(point.y, DEPTH_ALIGN_WALL_SAMPLE_GRID_METERS),
                z: quantize_f32(point.z, DEPTH_ALIGN_WALL_SAMPLE_GRID_METERS),
            };
            upsert_alignment_candidate(
                wall_samples,
                key,
                AlignmentCandidateSample {
                    kind: XrDepthAlignSampleKind::Wall,
                    point,
                    normal: patch.normal,
                    weight: plane_patch_alignment_sample_weight(
                        patch,
                        DEPTH_ALIGN_WALL_SAMPLE_GRID_METERS,
                        edge_bias,
                    ),
                },
            );
        },
    );
}

fn for_each_plane_patch_grid_sample<F>(patch: &XrDepthPlanePatch, step: f32, mut visitor: F)
where
    F: FnMut(Vec3f, f32),
{
    let plane_distance = patch.center.dot(patch.normal);
    let min_u = patch.center.dot(patch.tangent) - patch.half_extent_tangent;
    let max_u = patch.center.dot(patch.tangent) + patch.half_extent_tangent;
    let min_v = patch.center.dot(patch.bitangent) - patch.half_extent_bitangent;
    let max_v = patch.center.dot(patch.bitangent) + patch.half_extent_bitangent;
    let min_u_cell = (min_u / step.max(f32::EPSILON)).ceil() as i32;
    let max_u_cell = (max_u / step.max(f32::EPSILON)).floor() as i32;
    let min_v_cell = (min_v / step.max(f32::EPSILON)).ceil() as i32;
    let max_v_cell = (max_v / step.max(f32::EPSILON)).floor() as i32;
    let mut emitted = false;

    for u_cell in min_u_cell..=max_u_cell {
        for v_cell in min_v_cell..=max_v_cell {
            let u = u_cell as f32 * step;
            let v = v_cell as f32 * step;
            let edge_distance = (u - min_u).min(max_u - u).min(v - min_v).min(max_v - v);
            let edge_bias = 1.0 - (edge_distance / step.max(0.001)).clamp(0.0, 1.0);
            visitor(
                patch.normal.scale(plane_distance)
                    + patch.tangent.scale(u)
                    + patch.bitangent.scale(v),
                edge_bias,
            );
            emitted = true;
        }
    }

    if !emitted {
        visitor(patch.center, 1.0);
    }
}

fn plane_patch_alignment_sample_weight(
    patch: &XrDepthPlanePatch,
    step: f32,
    edge_bias: f32,
) -> f32 {
    let base = (patch.area / step.max(0.01)).sqrt().clamp(0.25, 6.0);
    base * (1.0 + edge_bias * 0.65)
}

fn upsert_alignment_candidate<K: Eq + std::hash::Hash>(
    map: &mut HashMap<K, AlignmentCandidateSample>,
    key: K,
    candidate: AlignmentCandidateSample,
) {
    match map.get_mut(&key) {
        Some(existing) if existing.weight >= candidate.weight => {}
        Some(existing) => *existing = candidate,
        None => {
            map.insert(key, candidate);
        }
    }
}

fn quantized_align_sample(sample: AlignmentCandidateSample) -> XrDepthAlignSample {
    let quantized_normal = vec3f(
        quantize_metric(sample.normal.x, 0.001),
        quantize_metric(sample.normal.y, 0.001),
        quantize_metric(sample.normal.z, 0.001),
    );
    XrDepthAlignSample {
        kind: sample.kind,
        point: vec3f(
            quantize_metric(sample.point.x, 0.01),
            quantize_metric(sample.point.y, 0.01),
            quantize_metric(sample.point.z, 0.01),
        ),
        normal: if quantized_normal.length() > 1.0e-5 {
            quantized_normal.normalize()
        } else {
            sample.normal
        },
        weight: quantize_metric(sample.weight, 0.01).max(0.01),
    }
}

fn query_depth_grid_normal_at_coord(volume: &DepthMeshVolume, coord: VoxelCoord) -> Option<Vec3f> {
    let center = volume.mesh_grid.normalized_distance(coord)?;
    let step = volume
        .voxel_size_meters
        .max(DEPTH_QUERY_TSDF_IMPACT_RAY_STEP_MIN);
    let dx = finite_difference_axis(
        center,
        volume
            .mesh_grid
            .normalized_distance(coord + VoxelCoord::new(1, 0, 0)),
        volume
            .mesh_grid
            .normalized_distance(coord - VoxelCoord::new(1, 0, 0)),
        step,
    )?;
    let dy = finite_difference_axis(
        center,
        volume
            .mesh_grid
            .normalized_distance(coord + VoxelCoord::new(0, 1, 0)),
        volume
            .mesh_grid
            .normalized_distance(coord - VoxelCoord::new(0, 1, 0)),
        step,
    )?;
    let dz = finite_difference_axis(
        center,
        volume
            .mesh_grid
            .normalized_distance(coord + VoxelCoord::new(0, 0, 1)),
        volume
            .mesh_grid
            .normalized_distance(coord - VoxelCoord::new(0, 0, 1)),
        step,
    )?;
    let gradient = vec3f(dx, dy, dz);
    (gradient.length() > 1.0e-5).then_some(gradient.normalize())
}

fn quantize_metric(value: f32, step: f32) -> f32 {
    if step <= 0.0 {
        return value;
    }
    (value / step).round() * step
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
                let key = ChunkKey::new(x, y, z);
                if volume.pending_mesh_dirty_chunks.insert(key) {
                    volume.pending_mesh_chunk_queue.push_back(key);
                }
                if volume.pending_plane_scan_dirty_chunks.insert(key) {
                    volume.pending_plane_scan_chunk_queue.push_back(key);
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
    let meshed_keys: HashSet<ChunkKey> = volume
        .mesh_chunks
        .iter()
        .map(|chunk| chunk.chunk_key)
        .collect();
    for z in (min_key.z - 1)..=(max_key.z + 1) {
        for y in (min_key.y - 1)..=(max_key.y + 1) {
            for x in (min_key.x - 1)..=(max_key.x + 1) {
                let key = ChunkKey::new(x, y, z);
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
            volume.reset_render_mesh_state();
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
        let mesh = volume.mesh_grid.surface_net_chunk_mesh_with_scratch(
            voxel_coord_from_chunk_key(chunk_key),
            volume.mesh_config,
            volume.generation,
            &mut worker_state.mesh_scratch,
            &mut worker_state.mesh_fill_scratch,
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
    chunk_key: ChunkKey,
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

#[derive(Clone, Debug)]
struct ExtractedPlaneTriangle {
    source_triangle_index: usize,
    group: ExtractedPlaneGroup,
    area: f32,
    normal: Vec3f,
    centroid: Vec3f,
    vertices: [Vec3f; 3],
}

#[derive(Clone, Debug)]
struct SimplifiedPlaneRegion {
    group: ExtractedPlaneGroup,
    source_triangle_indices: Vec<usize>,
    triangles: Vec<ExtractedPlaneTriangle>,
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

#[derive(Clone, Copy, Debug)]
struct PlanarRectCoverage {
    min_u: f32,
    max_u: f32,
    min_v: f32,
    max_v: f32,
}

#[derive(Clone, Debug)]
struct EmittedPlanarRegion {
    tangent: Vec3f,
    bitangent: Vec3f,
    rects: Vec<PlanarRectCoverage>,
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum TsdfPlaneScanGroup {
    Vertical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct TsdfPlaneScanBinKey {
    group: TsdfPlaneScanGroup,
    plane_bin: i32,
    normal_x_bin: i32,
    normal_z_bin: i32,
}

#[derive(Clone, Debug)]
struct TsdfPlaneScanBin {
    normal_sum: Vec3f,
    point_sum: Vec3f,
    point_weight_sum: f32,
    support_mask: PlaneSupportMask,
    support_sample_count: usize,
    support_samples: Vec<TsdfPlaneSupportSample>,
}

#[derive(Clone, Copy, Debug)]
struct TsdfPlaneSupportSample {
    point: Vec3f,
    cell: PlaneSupportCellKey,
}

impl Default for TsdfPlaneScanBin {
    fn default() -> Self {
        Self {
            normal_sum: Vec3f::default(),
            point_sum: Vec3f::default(),
            point_weight_sum: 0.0,
            support_mask: PlaneSupportMask::default(),
            support_sample_count: 0,
            support_samples: Vec::new(),
        }
    }
}

fn rebuild_reduced_planar_patches(volume: &mut DepthMeshVolume) -> bool {
    let mut plane_patches = volume
        .mesh_chunks
        .iter()
        .flat_map(|chunk| chunk.planar_patches.iter().cloned())
        .collect::<Vec<_>>();
    classify_plane_patch_kinds(&mut plane_patches);
    plane_patches.sort_by(|a, b| {
        b.area
            .partial_cmp(&a.area)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    plane_patches.truncate(DEPTH_PLANE_MAX_PATCHES);

    let changed = plane_patches.len() != volume.plane_patches.len()
        || plane_patches
            .iter()
            .zip(volume.plane_patches.iter())
            .any(|(next, current)| {
                next.kind != current.kind
                    || (next.center - current.center).length() > 1.0e-4
                    || next.normal.dot(current.normal) < 0.999
                    || (next.half_extent_tangent - current.half_extent_tangent).abs() > 1.0e-4
                    || (next.half_extent_bitangent - current.half_extent_bitangent).abs() > 1.0e-4
            });
    if !changed {
        return false;
    }

    let next_generation = volume.plane_generation.saturating_add(1);
    for patch in &mut plane_patches {
        patch.generation = next_generation;
    }
    volume.plane_patches = plane_patches;
    volume.plane_generation = next_generation;
    volume.update_sequence = volume.update_sequence.saturating_add(1);
    true
}

fn plane_patch_matches(left: &XrDepthPlanePatch, right: &XrDepthPlanePatch) -> bool {
    left.kind == right.kind
        && (left.center - right.center).length() <= 1.0e-4
        && left.normal.dot(right.normal) >= 0.999
        && left.tangent.dot(right.tangent) >= 0.999
        && left.bitangent.dot(right.bitangent) >= 0.999
        && (left.half_extent_tangent - right.half_extent_tangent).abs() <= 1.0e-4
        && (left.half_extent_bitangent - right.half_extent_bitangent).abs() <= 1.0e-4
}

fn scanned_plane_chunk_matches(left: &ScannedPlaneChunk, right: &ScannedPlaneChunk) -> bool {
    left.near_surface_voxel_count == right.near_surface_voxel_count
        && left.floor_candidate_count == right.floor_candidate_count
        && left.wall_candidate_count == right.wall_candidate_count
        && left.patches.len() == right.patches.len()
        && left
            .patches
            .iter()
            .zip(right.patches.iter())
            .all(|(left, right)| plane_patch_matches(left, right))
}

fn lerp_metric(from: f32, to: f32, alpha: f32) -> f32 {
    from + (to - from) * alpha.clamp(0.0, 1.0)
}

fn stable_wall_patch_tracking_score(
    track: &XrDepthPlanePatch,
    candidate: &XrDepthPlanePatch,
) -> Option<f32> {
    let aligned_normal = align_direction(track.normal, candidate.normal);
    let normal_dot = track.normal.dot(aligned_normal);
    if normal_dot < DEPTH_PLANE_STABLE_TRACK_MATCH_NORMAL_DOT {
        return None;
    }

    let track_plane_distance = track.center.dot(track.normal);
    let candidate_plane_distance = candidate.center.dot(track.normal);
    let plane_delta = (track_plane_distance - candidate_plane_distance).abs();
    if plane_delta > DEPTH_PLANE_STABLE_TRACK_MATCH_DISTANCE_METERS {
        return None;
    }

    let (track_min_t, track_max_t, track_min_b, track_max_b) =
        plane_patch_projected_extents(track, track.tangent, track.bitangent);
    let (cand_min_t, cand_max_t, cand_min_b, cand_max_b) =
        project_patch_extents_to_basis(candidate, track.tangent, track.bitangent);
    let height_gap = interval_gap(track_min_t, track_max_t, cand_min_t, cand_max_t);
    let along_gap = interval_gap(track_min_b, track_max_b, cand_min_b, cand_max_b);
    if height_gap > DEPTH_PLANE_STABLE_TRACK_MATCH_HEIGHT_GAP_METERS
        || along_gap > DEPTH_PLANE_STABLE_TRACK_MATCH_ALONG_GAP_METERS
    {
        return None;
    }

    let area_ratio =
        (track.area.min(candidate.area) / track.area.max(candidate.area).max(0.05)).clamp(0.0, 1.0);
    let gap_score =
        (1.0 - along_gap / DEPTH_PLANE_STABLE_TRACK_MATCH_ALONG_GAP_METERS).clamp(0.0, 1.0);
    let height_score =
        (1.0 - height_gap / DEPTH_PLANE_STABLE_TRACK_MATCH_HEIGHT_GAP_METERS).clamp(0.0, 1.0);
    Some(
        normal_dot.powf(2.0)
            * (-plane_delta / 0.05).exp()
            * (0.35 + 0.65 * area_ratio)
            * (0.30 + 0.70 * gap_score)
            * (0.35 + 0.65 * height_score),
    )
}

fn smooth_stable_wall_patch(
    current: &XrDepthPlanePatch,
    measurement: &XrDepthPlanePatch,
) -> XrDepthPlanePatch {
    let alpha = DEPTH_PLANE_STABLE_TRACK_SMOOTH_ALPHA;
    let aligned_normal = align_direction(current.normal, measurement.normal);
    let blended_normal = current.normal.scale(1.0 - alpha) + aligned_normal.scale(alpha);
    let normal = if blended_normal.length() > 1.0e-5 {
        blended_normal.normalize()
    } else {
        current.normal
    };
    let tangent = vec3f(0.0, 1.0, 0.0);
    let bitangent = Vec3f::cross(normal, tangent).normalize();
    let bitangent = if bitangent.length() > 1.0e-5 {
        bitangent
    } else {
        current.bitangent
    };

    let (cur_min_t, cur_max_t, cur_min_b, cur_max_b) =
        project_patch_extents_to_basis(current, tangent, bitangent);
    let (meas_min_t, meas_max_t, meas_min_b, meas_max_b) =
        project_patch_extents_to_basis(measurement, tangent, bitangent);
    let min_t = lerp_metric(cur_min_t, meas_min_t, alpha);
    let max_t = lerp_metric(cur_max_t, meas_max_t, alpha);
    let min_b = lerp_metric(cur_min_b, meas_min_b, alpha);
    let max_b = lerp_metric(cur_max_b, meas_max_b, alpha);
    let plane_distance = lerp_metric(
        current.center.dot(normal),
        measurement.center.dot(normal),
        alpha,
    );
    let center_t = (min_t + max_t) * 0.5;
    let center_b = (min_b + max_b) * 0.5;
    let half_extent_tangent = ((max_t - min_t) * 0.5).max(0.0);
    let half_extent_bitangent = ((max_b - min_b) * 0.5).max(0.0);
    XrDepthPlanePatch {
        generation: current.generation,
        kind: XrDepthPlaneKind::Wall,
        center: normal.scale(plane_distance) + tangent.scale(center_t) + bitangent.scale(center_b),
        normal,
        tangent,
        bitangent,
        half_extent_tangent,
        half_extent_bitangent,
        area: (half_extent_tangent * 2.0) * (half_extent_bitangent * 2.0),
        support_triangles: current.support_triangles.max(measurement.support_triangles),
    }
}

fn stabilize_wall_scan_patches(
    tracks: &mut Vec<StableWallPatchTrack>,
    patches: Vec<XrDepthPlanePatch>,
) -> Vec<XrDepthPlanePatch> {
    let mut candidate_pairs = Vec::<(f32, usize, usize)>::new();
    for (track_index, track) in tracks.iter().enumerate() {
        for (patch_index, patch) in patches.iter().enumerate() {
            if let Some(score) = stable_wall_patch_tracking_score(&track.patch, patch) {
                candidate_pairs.push((score, track_index, patch_index));
            }
        }
    }
    candidate_pairs.sort_by(|a, b| b.0.total_cmp(&a.0));

    let mut matched_tracks = vec![false; tracks.len()];
    let mut matched_patches = vec![false; patches.len()];
    let mut next_tracks = Vec::<StableWallPatchTrack>::with_capacity(tracks.len() + patches.len());

    for (_, track_index, patch_index) in candidate_pairs {
        if matched_tracks[track_index] || matched_patches[patch_index] {
            continue;
        }
        matched_tracks[track_index] = true;
        matched_patches[patch_index] = true;
        let track = &tracks[track_index];
        next_tracks.push(StableWallPatchTrack {
            patch: smooth_stable_wall_patch(&track.patch, &patches[patch_index]),
            seen_count: track.seen_count.saturating_add(1),
            missing_updates: 0,
        });
    }

    for (track_index, track) in tracks.iter().enumerate() {
        if matched_tracks[track_index] {
            continue;
        }
        if track.missing_updates < DEPTH_PLANE_STABLE_TRACK_MISSING_UPDATES_MAX {
            let mut track = track.clone();
            track.missing_updates = track.missing_updates.saturating_add(1);
            next_tracks.push(track);
        }
    }

    for (patch_index, patch) in patches.into_iter().enumerate() {
        if matched_patches[patch_index] {
            continue;
        }
        next_tracks.push(StableWallPatchTrack {
            patch,
            seen_count: 1,
            missing_updates: 0,
        });
    }

    next_tracks.sort_by(|a, b| b.patch.area.total_cmp(&a.patch.area));
    if next_tracks.len() > DEPTH_PLANE_MAX_PATCHES * 2 {
        next_tracks.truncate(DEPTH_PLANE_MAX_PATCHES * 2);
    }

    let mut visible = next_tracks
        .iter()
        .filter(|track| {
            track.seen_count >= DEPTH_PLANE_STABLE_TRACK_MIN_SEEN_COUNT
                || track.missing_updates == 0
        })
        .map(|track| track.patch.clone())
        .collect::<Vec<_>>();
    visible.sort_by(|a, b| b.area.total_cmp(&a.area));
    if visible.len() > DEPTH_PLANE_MAX_PATCHES {
        visible.truncate(DEPTH_PLANE_MAX_PATCHES);
    }

    *tracks = next_tracks;
    visible
}

fn visible_stable_wall_patches(tracks: &[StableWallPatchTrack]) -> Vec<XrDepthPlanePatch> {
    let mut visible = tracks
        .iter()
        .filter(|track| {
            track.seen_count >= DEPTH_PLANE_STABLE_TRACK_MIN_SEEN_COUNT
                || track.missing_updates == 0
        })
        .map(|track| track.patch.clone())
        .collect::<Vec<_>>();
    visible.sort_by(|a, b| b.area.total_cmp(&a.area));
    if visible.len() > DEPTH_PLANE_MAX_PATCHES * 2 {
        visible.truncate(DEPTH_PLANE_MAX_PATCHES * 2);
    }
    visible
}

fn canonicalize_room_axis(axis: Vec3f) -> Option<Vec3f> {
    let axis = vec3f(axis.x, 0.0, axis.z);
    let axis = if axis.length() > 1.0e-5 {
        axis.normalize()
    } else {
        return None;
    };
    if axis.x < -1.0e-5 || (axis.x.abs() <= 1.0e-5 && axis.z < 0.0) {
        Some(axis.scale(-1.0))
    } else {
        Some(axis)
    }
}

fn orthogonal_room_axis(axis: Vec3f) -> Vec3f {
    vec3f(-axis.z, 0.0, axis.x).normalize()
}

fn refine_room_wall_axis(
    patches: &[XrDepthPlanePatch],
    seed_axis: Vec3f,
    threshold_dot: f32,
) -> Vec3f {
    let mut axis_sum = Vec3f::default();
    let mut found = false;
    for patch in patches {
        let Some(axis) = canonicalize_room_axis(patch.normal) else {
            continue;
        };
        if axis.dot(seed_axis).abs() < threshold_dot {
            continue;
        }
        let aligned_axis = if axis.dot(seed_axis) < 0.0 {
            axis.scale(-1.0)
        } else {
            axis
        };
        axis_sum += aligned_axis.scale(patch.area.max(0.05));
        found = true;
    }
    if found && axis_sum.length() > 1.0e-5 {
        canonicalize_room_axis(axis_sum).unwrap_or(seed_axis)
    } else {
        seed_axis
    }
}

fn infer_room_wall_axes(patches: &[XrDepthPlanePatch]) -> Option<[Vec3f; 2]> {
    let primary_seed = patches
        .iter()
        .filter_map(|patch| canonicalize_room_axis(patch.normal).map(|axis| (patch.area, axis)))
        .max_by(|left, right| left.0.total_cmp(&right.0))
        .map(|(_, axis)| axis)?;
    let primary = refine_room_wall_axis(patches, primary_seed, 0.82);
    let secondary_seed = patches
        .iter()
        .filter_map(|patch| {
            let axis = canonicalize_room_axis(patch.normal)?;
            (axis.dot(primary).abs() <= DEPTH_ROOM_WALL_ORTHOGONAL_DOT_MAX)
                .then_some((patch.area, axis))
        })
        .max_by(|left, right| left.0.total_cmp(&right.0))
        .map(|(_, axis)| axis)
        .unwrap_or_else(|| orthogonal_room_axis(primary));
    let mut secondary = refine_room_wall_axis(patches, secondary_seed, 0.82);
    if secondary.dot(primary).abs() > DEPTH_ROOM_WALL_ORTHOGONAL_DOT_MAX {
        secondary = orthogonal_room_axis(primary);
    }
    Some([primary, secondary])
}

fn accumulate_room_wall_patch(
    accumulator: &mut RoomWallAccumulator,
    wall_normal: Vec3f,
    patch: &XrDepthPlanePatch,
) {
    let weight = patch.area.max(0.05);
    if accumulator.weight_sum <= 0.0 {
        accumulator.normal = wall_normal;
    }
    let along_axis = vec3f(-wall_normal.z, 0.0, wall_normal.x).normalize();
    let (_, _, along_min, along_max) =
        project_patch_extents_to_basis(patch, vec3f(0.0, 1.0, 0.0), along_axis);
    accumulator.weight_sum += weight;
    accumulator.plane_distance_sum += patch.center.dot(wall_normal) * weight;
    accumulator.support_area += patch.area.max(0.0);
    accumulator.observed_along_min = accumulator.observed_along_min.min(along_min);
    accumulator.observed_along_max = accumulator.observed_along_max.max(along_max);
}

fn build_room_wall_candidates(patches: &[XrDepthPlanePatch]) -> Vec<RoomWallCandidate> {
    let Some(room_axes) = infer_room_wall_axes(patches) else {
        return Vec::new();
    };
    let mut slot_groups = [
        Vec::<RoomWallAccumulator>::new(),
        Vec::<RoomWallAccumulator>::new(),
        Vec::<RoomWallAccumulator>::new(),
        Vec::<RoomWallAccumulator>::new(),
    ];
    for patch in patches {
        let Some(axis) = canonicalize_room_axis(patch.normal) else {
            continue;
        };
        let primary_dot = axis.dot(room_axes[0]).abs();
        let secondary_dot = axis.dot(room_axes[1]).abs();
        let (axis_index, best_dot) = if primary_dot >= secondary_dot {
            (0usize, primary_dot)
        } else {
            (1usize, secondary_dot)
        };
        if best_dot < DEPTH_ROOM_WALL_AXIS_ASSIGN_DOT_MIN {
            continue;
        }
        let axis = room_axes[axis_index];
        let axis_projection = patch.center.dot(axis);
        let normal_projection = patch.normal.dot(axis);
        let side_sign = if axis_projection.abs() >= 0.12 {
            if axis_projection >= 0.0 {
                1.0
            } else {
                -1.0
            }
        } else if normal_projection >= 0.0 {
            1.0
        } else {
            -1.0
        };
        let wall_normal = axis.scale(side_sign);
        let slot_index = axis_index * 2 + if side_sign >= 0.0 { 1 } else { 0 };
        let plane_distance = patch.center.dot(wall_normal);
        let mut best_group = None::<(usize, f32)>;
        for (group_index, group) in slot_groups[slot_index].iter().enumerate() {
            if group.weight_sum <= 0.0 {
                continue;
            }
            let group_distance = group.plane_distance_sum / group.weight_sum.max(1.0e-5);
            let delta = (group_distance - plane_distance).abs();
            if delta > DEPTH_ROOM_WALL_GROUP_DISTANCE_METERS {
                continue;
            }
            if best_group.is_none_or(|(_, best_delta)| delta < best_delta) {
                best_group = Some((group_index, delta));
            }
        }
        if let Some((group_index, _)) = best_group {
            accumulate_room_wall_patch(
                &mut slot_groups[slot_index][group_index],
                wall_normal,
                patch,
            );
        } else {
            let mut group = RoomWallAccumulator::default();
            accumulate_room_wall_patch(&mut group, wall_normal, patch);
            slot_groups[slot_index].push(group);
        }
    }

    let mut candidates = Vec::<RoomWallCandidate>::new();
    for groups in slot_groups {
        let Some(accumulator) = groups
            .into_iter()
            .max_by(|left, right| left.support_area.total_cmp(&right.support_area))
        else {
            continue;
        };
        if accumulator.weight_sum <= 0.0
            || !accumulator.observed_along_min.is_finite()
            || !accumulator.observed_along_max.is_finite()
        {
            continue;
        }
        candidates.push(RoomWallCandidate {
            normal: accumulator.normal,
            plane_distance: accumulator.plane_distance_sum / accumulator.weight_sum.max(1.0e-5),
            observed_along_min: accumulator.observed_along_min,
            observed_along_max: accumulator.observed_along_max,
            support_area: accumulator.support_area.max(0.0),
        });
    }
    candidates.sort_by(|left, right| right.support_area.total_cmp(&left.support_area));
    candidates
}

fn room_wall_track_score(
    track: &StableRoomWallTrack,
    candidate: &RoomWallCandidate,
) -> Option<f32> {
    let normal_dot = track.normal.dot(candidate.normal);
    if normal_dot < DEPTH_ROOM_WALL_TRACK_MATCH_NORMAL_DOT {
        return None;
    }
    let plane_delta = (track.plane_distance - candidate.plane_distance).abs();
    if plane_delta > DEPTH_ROOM_WALL_TRACK_MATCH_DISTANCE_METERS {
        return None;
    }
    let current_span = (track.observed_along_max - track.observed_along_min).max(0.05);
    let candidate_span = (candidate.observed_along_max - candidate.observed_along_min).max(0.05);
    let span_ratio =
        (current_span.min(candidate_span) / current_span.max(candidate_span)).clamp(0.0, 1.0);
    Some(
        normal_dot.powf(2.0)
            * (-plane_delta / 0.08).exp()
            * (0.35 + 0.65 * span_ratio)
            * (0.30
                + 0.70
                    * (track.support_area.min(candidate.support_area)
                        / track.support_area.max(candidate.support_area).max(0.05))
                    .clamp(0.0, 1.0)),
    )
}

fn smooth_room_wall_extent_bound(current: f32, measurement: f32, expands_when_lower: bool) -> f32 {
    let expands = if expands_when_lower {
        measurement < current
    } else {
        measurement > current
    };
    let alpha = if expands {
        DEPTH_ROOM_WALL_TRACK_EXPAND_ALPHA
    } else {
        DEPTH_ROOM_WALL_TRACK_SHRINK_ALPHA
    };
    lerp_metric(current, measurement, alpha)
}

fn smooth_room_wall_track(
    current: &StableRoomWallTrack,
    measurement: &RoomWallCandidate,
) -> StableRoomWallTrack {
    let alpha = DEPTH_ROOM_WALL_TRACK_SMOOTH_ALPHA;
    let aligned_normal = align_direction(current.normal, measurement.normal);
    let blended_normal = current.normal.scale(1.0 - alpha) + aligned_normal.scale(alpha);
    let normal = if blended_normal.length() > 1.0e-5 {
        blended_normal.normalize()
    } else {
        current.normal
    };
    StableRoomWallTrack {
        normal,
        plane_distance: lerp_metric(current.plane_distance, measurement.plane_distance, alpha),
        observed_along_min: smooth_room_wall_extent_bound(
            current.observed_along_min,
            measurement.observed_along_min,
            true,
        ),
        observed_along_max: smooth_room_wall_extent_bound(
            current.observed_along_max,
            measurement.observed_along_max,
            false,
        ),
        support_area: lerp_metric(current.support_area, measurement.support_area, alpha).max(0.05),
        seen_count: current.seen_count.saturating_add(1),
        missing_updates: 0,
    }
}

fn stabilize_room_wall_candidates(
    tracks: &mut Vec<StableRoomWallTrack>,
    candidates: Vec<RoomWallCandidate>,
) {
    let mut candidate_pairs = Vec::<(f32, usize, usize)>::new();
    for (track_index, track) in tracks.iter().enumerate() {
        for (candidate_index, candidate) in candidates.iter().enumerate() {
            if let Some(score) = room_wall_track_score(track, candidate) {
                candidate_pairs.push((score, track_index, candidate_index));
            }
        }
    }
    candidate_pairs.sort_by(|left, right| right.0.total_cmp(&left.0));

    let mut matched_tracks = vec![false; tracks.len()];
    let mut matched_candidates = vec![false; candidates.len()];
    let mut next_tracks =
        Vec::<StableRoomWallTrack>::with_capacity(tracks.len() + candidates.len());

    for (_, track_index, candidate_index) in candidate_pairs {
        if matched_tracks[track_index] || matched_candidates[candidate_index] {
            continue;
        }
        matched_tracks[track_index] = true;
        matched_candidates[candidate_index] = true;
        next_tracks.push(smooth_room_wall_track(
            &tracks[track_index],
            &candidates[candidate_index],
        ));
    }

    for (track_index, track) in tracks.iter().enumerate() {
        if matched_tracks[track_index] {
            continue;
        }
        if track.missing_updates < DEPTH_ROOM_WALL_TRACK_MISSING_UPDATES_MAX {
            let mut track = track.clone();
            track.missing_updates = track.missing_updates.saturating_add(1);
            next_tracks.push(track);
        }
    }

    for (candidate_index, candidate) in candidates.into_iter().enumerate() {
        if matched_candidates[candidate_index] {
            continue;
        }
        if tracks.len() >= DEPTH_ROOM_WALL_MAX_TRACKS {
            continue;
        }
        next_tracks.push(StableRoomWallTrack {
            normal: candidate.normal,
            plane_distance: candidate.plane_distance,
            observed_along_min: candidate.observed_along_min,
            observed_along_max: candidate.observed_along_max,
            support_area: candidate.support_area.max(0.05),
            seen_count: 1,
            missing_updates: 0,
        });
    }

    next_tracks.sort_by(|left, right| {
        right
            .seen_count
            .cmp(&left.seen_count)
            .then_with(|| left.missing_updates.cmp(&right.missing_updates))
            .then_with(|| right.support_area.total_cmp(&left.support_area))
    });
    if next_tracks.len() > DEPTH_ROOM_WALL_MAX_TRACKS {
        next_tracks.truncate(DEPTH_ROOM_WALL_MAX_TRACKS);
    }

    *tracks = next_tracks;
}

fn visible_room_wall_candidates(tracks: &[StableRoomWallTrack]) -> Vec<RoomWallCandidate> {
    let mut visible = tracks
        .iter()
        .map(|track| RoomWallCandidate {
            normal: track.normal,
            plane_distance: track.plane_distance,
            observed_along_min: track.observed_along_min,
            observed_along_max: track.observed_along_max,
            support_area: track.support_area,
        })
        .collect::<Vec<_>>();
    visible.sort_by(|left, right| right.support_area.total_cmp(&left.support_area));
    if visible.len() > DEPTH_ROOM_WALL_MAX_TRACKS {
        visible.truncate(DEPTH_ROOM_WALL_MAX_TRACKS);
    }
    visible
}

fn room_wall_feature_from_candidate(
    candidate: &RoomWallCandidate,
    all_walls: &[RoomWallCandidate],
    wall_index: usize,
) -> Option<XrDepthAlignWallFeature> {
    let normal = if candidate.normal.length() > 1.0e-5 {
        candidate.normal.normalize()
    } else {
        return None;
    };
    let along_axis = vec3f(-normal.z, 0.0, normal.x);
    let along_axis = if along_axis.length() > 1.0e-5 {
        along_axis.normalize()
    } else {
        return None;
    };
    let mut along_min = candidate.observed_along_min;
    let mut along_max = candidate.observed_along_max;
    let mut intersection_min = f32::INFINITY;
    let mut intersection_max = f32::NEG_INFINITY;
    let mut intersection_count = 0usize;
    for (other_index, other) in all_walls.iter().enumerate() {
        if other_index == wall_index {
            continue;
        }
        if other.normal.dot(normal).abs() > DEPTH_ROOM_WALL_ORTHOGONAL_DOT_MAX {
            continue;
        }
        let coordinate = other.normal.dot(along_axis) * other.plane_distance;
        if !coordinate.is_finite() {
            continue;
        }
        intersection_min = intersection_min.min(coordinate);
        intersection_max = intersection_max.max(coordinate);
        intersection_count += 1;
    }
    if intersection_count >= 2 {
        along_min = intersection_min;
        along_max = intersection_max;
    } else if intersection_count == 1 {
        along_min = along_min.min(intersection_min);
        along_max = along_max.max(intersection_max);
    }
    if !along_min.is_finite() || !along_max.is_finite() {
        return None;
    }
    let half_extent_along = ((along_max - along_min) * 0.5).max(0.25);
    let along_center = (along_min + along_max) * 0.5;
    Some(XrDepthAlignWallFeature {
        center: normal.scale(candidate.plane_distance)
            + along_axis.scale(along_center)
            + vec3f(0.0, DEPTH_ROOM_WALL_HEIGHT_METERS * 0.5, 0.0),
        normal,
        along_axis,
        plane_distance: candidate.plane_distance,
        half_extent_along,
        min_y: 0.0,
        max_y: DEPTH_ROOM_WALL_HEIGHT_METERS,
        area: candidate.support_area.max(0.25),
    })
}

fn visible_room_wall_features(tracks: &[StableRoomWallTrack]) -> Vec<XrDepthAlignWallFeature> {
    let walls = visible_room_wall_candidates(tracks);
    let mut features = Vec::<XrDepthAlignWallFeature>::new();
    for (index, wall) in walls.iter().enumerate() {
        if let Some(feature) = room_wall_feature_from_candidate(wall, &walls, index) {
            features.push(feature);
        }
    }
    features.sort_by(|left, right| right.area.total_cmp(&left.area));
    if features.len() > DEPTH_ALIGN_MAX_WALL_FEATURES {
        features.truncate(DEPTH_ALIGN_MAX_WALL_FEATURES);
    }
    features
}

fn process_incremental_tsdf_plane_scan(volume: &mut DepthMeshVolume, max_scan_jobs: usize) -> bool {
    if volume.mesh_grid.is_empty() {
        return volume.reset_plane_scan_state();
    }
    if volume.plane_scan_chunks.is_empty() && volume.pending_plane_scan_chunk_queue.is_empty() {
        enqueue_all_plane_scan_chunks(volume);
    }

    let mut chunks_changed = false;
    for _ in 0..max_scan_jobs {
        let Some(chunk_key) = volume.pending_plane_scan_chunk_queue.pop_front() else {
            break;
        };
        volume.pending_plane_scan_dirty_chunks.remove(&chunk_key);
        let next_chunk = scan_tsdf_plane_chunk(volume, chunk_key);
        match (volume.plane_scan_chunks.get(&chunk_key), next_chunk) {
            (Some(current), Some(next)) if scanned_plane_chunk_matches(current, &next) => {}
            (_, Some(next)) => {
                volume.plane_scan_chunks.insert(chunk_key, next);
                chunks_changed = true;
            }
            (Some(_), None) => {
                volume.plane_scan_chunks.remove(&chunk_key);
                chunks_changed = true;
            }
            (None, None) => {}
        }
    }

    if chunks_changed {
        rebuild_tsdf_plane_scan_patches(volume)
    } else {
        false
    }
}

fn enqueue_all_plane_scan_chunks(volume: &mut DepthMeshVolume) {
    let mesh_edge = volume.mesh_config.mesh_chunk_edge_voxels().max(1);
    let grid_edge = volume.mesh_grid.chunk_edge.max(1);
    let mut keys = HashSet::<ChunkKey>::new();
    for key in volume.mesh_grid.chunks.keys() {
        let min_voxel = VoxelCoord::new(key.x * grid_edge, key.y * grid_edge, key.z * grid_edge);
        let max_voxel = VoxelCoord::new(
            min_voxel.x + grid_edge - 1,
            min_voxel.y + grid_edge - 1,
            min_voxel.z + grid_edge - 1,
        );
        let min_chunk = VoxelCoord::new(
            min_voxel.x.div_euclid(mesh_edge),
            min_voxel.y.div_euclid(mesh_edge),
            min_voxel.z.div_euclid(mesh_edge),
        );
        let max_chunk = VoxelCoord::new(
            max_voxel.x.div_euclid(mesh_edge),
            max_voxel.y.div_euclid(mesh_edge),
            max_voxel.z.div_euclid(mesh_edge),
        );
        for z in min_chunk.z..=max_chunk.z {
            for y in min_chunk.y..=max_chunk.y {
                for x in min_chunk.x..=max_chunk.x {
                    keys.insert(ChunkKey::new(x, y, z));
                }
            }
        }
    }
    let mut keys = keys.into_iter().collect::<Vec<_>>();
    keys.sort_by_key(|key| (key.x, key.y, key.z));
    for key in keys {
        if volume.pending_plane_scan_dirty_chunks.insert(key) {
            volume.pending_plane_scan_chunk_queue.push_back(key);
        }
    }
}

fn rebuild_tsdf_plane_scan_patches(volume: &mut DepthMeshVolume) -> bool {
    let mut plane_patches = volume
        .plane_scan_chunks
        .values()
        .flat_map(|chunk| chunk.patches.iter().cloned())
        .collect::<Vec<_>>();
    plane_patches = merge_tsdf_plane_patches(plane_patches);
    classify_plane_patch_kinds(&mut plane_patches);
    plane_patches.retain(|patch| patch.kind == XrDepthPlaneKind::Wall);
    plane_patches.sort_by(|a, b| {
        b.area
            .partial_cmp(&a.area)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    plane_patches.truncate(DEPTH_PLANE_MAX_PATCHES);
    plane_patches = stabilize_wall_scan_patches(&mut volume.stable_wall_tracks, plane_patches);
    stabilize_room_wall_candidates(
        &mut volume.room_wall_tracks,
        build_room_wall_candidates(&plane_patches),
    );
    let changed = plane_patches.len() != volume.plane_patches.len()
        || plane_patches
            .iter()
            .zip(volume.plane_patches.iter())
            .any(|(next, current)| !plane_patch_matches(next, current));
    if !changed {
        return false;
    }

    let next_generation = volume.plane_generation.saturating_add(1);
    for patch in &mut plane_patches {
        patch.generation = next_generation;
    }
    volume.plane_patches = plane_patches;
    volume.plane_generation = next_generation;
    volume.update_sequence = volume.update_sequence.saturating_add(1);
    true
}

fn merge_tsdf_plane_patches(mut patches: Vec<XrDepthPlanePatch>) -> Vec<XrDepthPlanePatch> {
    patches.sort_by(|a, b| {
        b.area
            .partial_cmp(&a.area)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut merged = Vec::<XrDepthPlanePatch>::new();
    'outer: for patch in patches {
        for existing in &mut merged {
            if let Some(next) = try_merge_tsdf_plane_patches(existing, &patch) {
                *existing = next;
                continue 'outer;
            }
        }
        merged.push(patch);
    }
    merged
}

fn try_merge_tsdf_plane_patches(
    left: &XrDepthPlanePatch,
    right: &XrDepthPlanePatch,
) -> Option<XrDepthPlanePatch> {
    let aligned_normal = align_direction(left.normal, right.normal);
    if left.normal.dot(aligned_normal) < DEPTH_PLANE_SCAN_MERGE_NORMAL_DOT {
        return None;
    }

    let left_distance = left.center.dot(left.normal);
    let right_distance = right.center.dot(left.normal);
    if (left_distance - right_distance).abs() > DEPTH_PLANE_SCAN_MERGE_DISTANCE_METERS {
        return None;
    }

    let (left_min_u, left_max_u, left_min_v, left_max_v) =
        plane_patch_projected_extents(left, left.tangent, left.bitangent);
    let (right_min_u, right_max_u, right_min_v, right_max_v) =
        project_patch_extents_to_basis(right, left.tangent, left.bitangent);
    let gap_u = interval_gap(left_min_u, left_max_u, right_min_u, right_max_u);
    let gap_v = interval_gap(left_min_v, left_max_v, right_min_v, right_max_v);
    if gap_u > DEPTH_PLANE_SCAN_MERGE_GAP_METERS || gap_v > DEPTH_PLANE_SCAN_MERGE_GAP_METERS {
        return None;
    }
    if gap_u > 0.0 && gap_v > 0.0 {
        return None;
    }

    let min_u = left_min_u.min(right_min_u);
    let max_u = left_max_u.max(right_max_u);
    let min_v = left_min_v.min(right_min_v);
    let max_v = left_max_v.max(right_max_v);
    let width = max_u - min_u;
    let height = max_v - min_v;
    if width < DEPTH_PLANE_SCAN_VERTICAL_MIN_DIM_METERS
        || height < DEPTH_PLANE_SCAN_VERTICAL_MIN_DIM_METERS
        || width * height < DEPTH_PLANE_SCAN_VERTICAL_MIN_AREA_METERS2
    {
        return None;
    }

    let plane_distance = (left_distance * left.area.max(0.001)
        + right_distance * right.area.max(0.001))
        / (left.area.max(0.001) + right.area.max(0.001));
    let center_u = (min_u + max_u) * 0.5;
    let center_v = (min_v + max_v) * 0.5;
    Some(XrDepthPlanePatch {
        generation: 0,
        kind: classify_planar_patch_kind_from_normal(left.normal),
        center: left.normal.scale(plane_distance)
            + left.tangent.scale(center_u)
            + left.bitangent.scale(center_v),
        normal: left.normal,
        tangent: left.tangent,
        bitangent: left.bitangent,
        half_extent_tangent: width * 0.5,
        half_extent_bitangent: height * 0.5,
        area: width * height,
        support_triangles: left.support_triangles + right.support_triangles,
    })
}

fn plane_patch_projected_extents(
    patch: &XrDepthPlanePatch,
    tangent: Vec3f,
    bitangent: Vec3f,
) -> (f32, f32, f32, f32) {
    let corners = plane_patch_corners(patch);
    let mut min_u = f32::INFINITY;
    let mut max_u = f32::NEG_INFINITY;
    let mut min_v = f32::INFINITY;
    let mut max_v = f32::NEG_INFINITY;
    for corner in corners {
        let u = corner.dot(tangent);
        let v = corner.dot(bitangent);
        min_u = min_u.min(u);
        max_u = max_u.max(u);
        min_v = min_v.min(v);
        max_v = max_v.max(v);
    }
    (min_u, max_u, min_v, max_v)
}

fn project_patch_extents_to_basis(
    patch: &XrDepthPlanePatch,
    tangent: Vec3f,
    bitangent: Vec3f,
) -> (f32, f32, f32, f32) {
    plane_patch_projected_extents(patch, tangent, bitangent)
}

fn interval_gap(left_min: f32, left_max: f32, right_min: f32, right_max: f32) -> f32 {
    if left_max < right_min {
        right_min - left_max
    } else if right_max < left_min {
        left_min - right_max
    } else {
        0.0
    }
}

fn scan_tsdf_plane_chunk(
    volume: &DepthMeshVolume,
    chunk_key: ChunkKey,
) -> Option<ScannedPlaneChunk> {
    let near_surface_band = (volume.voxel_size_meters * DEPTH_PLANE_SCAN_NEAR_SURFACE_BAND_SCALE)
        .max(DEPTH_PLANE_SCAN_NEAR_SURFACE_BAND_MIN);
    let tsd_distance_meters = depth_tsd_distance_meters(volume.voxel_size_meters);
    let edge = volume.mesh_config.mesh_chunk_edge_voxels().max(1);
    let origin = VoxelCoord::new(chunk_key.x * edge, chunk_key.y * edge, chunk_key.z * edge);
    let mut bins = HashMap::<TsdfPlaneScanBinKey, TsdfPlaneScanBin>::new();
    let mut near_surface_voxel_count = 0u32;
    let floor_candidate_count = 0u32;
    let mut wall_candidate_count = 0u32;

    for local_z in 0..edge {
        for local_y in 0..edge {
            for local_x in 0..edge {
                let coord = origin + VoxelCoord::new(local_x, local_y, local_z);
                if volume.mesh_grid.confidence(coord) < DEPTH_PLANE_SCAN_MIN_CONFIDENCE {
                    continue;
                }
                let Some(distance) = volume.mesh_grid.normalized_distance(coord) else {
                    continue;
                };
                let signed_distance = distance * tsd_distance_meters;
                if signed_distance.abs() > near_surface_band {
                    continue;
                }
                let Some(surface_normal) = query_depth_grid_normal_at_coord(volume, coord) else {
                    continue;
                };
                near_surface_voxel_count = near_surface_voxel_count.saturating_add(1);
                let point = volume.mesh_grid.voxel_center_world(coord)
                    - surface_normal.scale(signed_distance);
                let Some((key, plane_normal, tangent, bitangent)) =
                    tsdf_plane_scan_key(point, surface_normal)
                else {
                    continue;
                };
                wall_candidate_count = wall_candidate_count.saturating_add(1);
                let bin = bins.entry(key).or_default();
                let aligned_normal = align_direction(plane_normal, surface_normal);
                let weight = volume.mesh_grid.confidence(coord) as f32;
                bin.normal_sum += aligned_normal.scale(weight);
                bin.point_sum += point.scale(weight);
                bin.point_weight_sum += weight;
                bin.support_sample_count += 1;
                let support_u = point.dot(tangent);
                let support_v = point.dot(bitangent);
                let cell = stamp_plane_support_sample(&mut bin.support_mask, support_u, support_v);
                bin.support_samples
                    .push(TsdfPlaneSupportSample { point, cell });
            }
        }
    }

    if near_surface_voxel_count == 0 {
        return None;
    }

    let mut patches = Vec::new();
    for (key, bin) in bins {
        patches.extend(emit_tsdf_plane_scan_bin_patches(key, &bin));
    }
    patches.sort_by(|a, b| {
        b.area
            .partial_cmp(&a.area)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    Some(ScannedPlaneChunk {
        patches,
        near_surface_voxel_count,
        floor_candidate_count,
        wall_candidate_count,
    })
}

fn tsdf_plane_scan_key(
    point: Vec3f,
    normal: Vec3f,
) -> Option<(TsdfPlaneScanBinKey, Vec3f, Vec3f, Vec3f)> {
    let canonical_normal = canonicalize_vertical_plane_normal(normal)?;
    if canonical_normal.y.abs() > DEPTH_PLANE_VERTICAL_NORMAL_Y_MAX {
        return None;
    }
    let tangent = vec3f(0.0, 1.0, 0.0);
    let bitangent = Vec3f::cross(canonical_normal, tangent).normalize();
    if bitangent.length() <= 1.0e-5 {
        return None;
    }
    let key = TsdfPlaneScanBinKey {
        group: TsdfPlaneScanGroup::Vertical,
        plane_bin: quantize_f32(
            point.dot(canonical_normal),
            DEPTH_PLANE_SCAN_VERTICAL_DISTANCE_BIN_METERS,
        ),
        normal_x_bin: quantize_f32(canonical_normal.x, DEPTH_PLANE_SCAN_VERTICAL_NORMAL_QUANTUM),
        normal_z_bin: quantize_f32(canonical_normal.z, DEPTH_PLANE_SCAN_VERTICAL_NORMAL_QUANTUM),
    };
    Some((key, canonical_normal, tangent, bitangent))
}

fn canonicalize_vertical_plane_normal(normal: Vec3f) -> Option<Vec3f> {
    let mut canonical = vec3f(normal.x, 0.0, normal.z);
    if canonical.length() <= 1.0e-5 {
        return None;
    }
    canonical = canonical.normalize();
    if canonical.x < 0.0 || (canonical.x.abs() <= 1.0e-5 && canonical.z < 0.0) {
        canonical = canonical.scale(-1.0);
    }
    Some(canonical)
}

fn stamp_plane_support_sample(mask: &mut PlaneSupportMask, u: f32, v: f32) -> PlaneSupportCellKey {
    let center = quantize_plane_support_cell(u, v);
    for du in -1..=1 {
        for dv in -1..=1 {
            let add = if du == 0 && dv == 0 {
                DEPTH_PLANE_SUPPORT_MAX_WEIGHT
            } else if du == 0 || dv == 0 {
                DEPTH_PLANE_SUPPORT_OCCUPIED_WEIGHT
            } else {
                1
            };
            let key = PlaneSupportCellKey {
                u: center.u + du,
                v: center.v + dv,
            };
            let weight = mask.cells.entry(key).or_insert(0);
            *weight = weight
                .saturating_add(add)
                .min(DEPTH_PLANE_SUPPORT_MAX_WEIGHT);
        }
    }
    center
}

fn emit_tsdf_plane_scan_bin_patches(
    key: TsdfPlaneScanBinKey,
    bin: &TsdfPlaneScanBin,
) -> Vec<XrDepthPlanePatch> {
    let Some((seed_normal, _tangent, _bitangent, _plane_distance)) =
        resolve_tsdf_plane_scan_bin_frame(key, bin)
    else {
        return Vec::new();
    };
    let mut patches = Vec::new();
    for mut component in decompose_support_mask_components(&bin.support_mask) {
        let mut emitted_rects = 0usize;
        loop {
            if emitted_rects >= DEPTH_PLANE_SCAN_MAX_RECTS_PER_GROUP {
                break;
            }
            let Some((min_u, max_u, min_v, max_v)) = largest_supported_rectangle(&component) else {
                break;
            };
            let width = (max_u - min_u + 1) as f32 * DEPTH_PLANE_SUPPORT_CELL_METERS;
            let height = (max_v - min_v + 1) as f32 * DEPTH_PLANE_SUPPORT_CELL_METERS;
            if width * height < DEPTH_PLANE_SCAN_VERTICAL_MIN_AREA_METERS2
                || width < DEPTH_PLANE_SCAN_VERTICAL_MIN_DIM_METERS
                || height < DEPTH_PLANE_SCAN_VERTICAL_MIN_DIM_METERS
            {
                break;
            }
            if let Some(patch) = fit_vertical_wall_patch_from_support_samples(
                &bin.support_samples,
                min_u,
                max_u,
                min_v,
                max_v,
                seed_normal,
            ) {
                patches.push(patch);
            }
            emitted_rects += 1;
            remove_rect_from_support_component(&mut component, min_u, max_u, min_v, max_v);
            if component.cells.is_empty() {
                break;
            }
        }
    }
    patches
}

fn resolve_tsdf_plane_scan_bin_frame(
    key: TsdfPlaneScanBinKey,
    bin: &TsdfPlaneScanBin,
) -> Option<(Vec3f, Vec3f, Vec3f, f32)> {
    let reference_normal = vec3f(
        key.normal_x_bin as f32 * DEPTH_PLANE_SCAN_VERTICAL_NORMAL_QUANTUM,
        0.0,
        key.normal_z_bin as f32 * DEPTH_PLANE_SCAN_VERTICAL_NORMAL_QUANTUM,
    );
    let reference_normal = if reference_normal.length() > 1.0e-5 {
        reference_normal.normalize()
    } else {
        vec3f(1.0, 0.0, 0.0)
    };
    let normal = if bin.normal_sum.length() > 1.0e-5 {
        align_direction(reference_normal, bin.normal_sum.normalize())
    } else {
        reference_normal
    };
    let tangent = vec3f(0.0, 1.0, 0.0);
    let bitangent = Vec3f::cross(normal, tangent).normalize();
    if bitangent.length() <= 1.0e-5 {
        return None;
    }
    let plane_distance = if bin.point_weight_sum > 0.0 {
        bin.point_sum
            .scale(bin.point_weight_sum.recip())
            .dot(normal)
    } else {
        key.plane_bin as f32 * DEPTH_PLANE_SCAN_VERTICAL_DISTANCE_BIN_METERS
    };
    Some((normal, tangent, bitangent, plane_distance))
}

fn fit_vertical_wall_patch_from_support_samples(
    support_samples: &[TsdfPlaneSupportSample],
    min_u_cell: i32,
    max_u_cell: i32,
    min_v_cell: i32,
    max_v_cell: i32,
    seed_normal: Vec3f,
) -> Option<XrDepthPlanePatch> {
    let mut points = Vec::<Vec3f>::new();
    for sample in support_samples {
        if sample.cell.u < min_u_cell
            || sample.cell.u > max_u_cell
            || sample.cell.v < min_v_cell
            || sample.cell.v > max_v_cell
        {
            continue;
        }
        points.push(sample.point);
    }
    if points.len() < 4 {
        return None;
    }

    let tangent = vec3f(0.0, 1.0, 0.0);
    let mut centroid = Vec3f::default();
    for point in &points {
        centroid += *point;
    }
    centroid = centroid.scale((points.len() as f32).recip());

    let mut cov_xx = 0.0;
    let mut cov_xz = 0.0;
    let mut cov_zz = 0.0;
    for point in &points {
        let dx = point.x - centroid.x;
        let dz = point.z - centroid.z;
        cov_xx += dx * dx;
        cov_xz += dx * dz;
        cov_zz += dz * dz;
    }

    let angle = 0.5 * (2.0 * cov_xz).atan2(cov_xx - cov_zz);
    let mut bitangent = vec3f(angle.cos(), 0.0, angle.sin());
    if bitangent.length() <= 1.0e-5 {
        bitangent = Vec3f::cross(seed_normal, tangent).normalize();
    } else {
        bitangent = bitangent.normalize();
    }
    if bitangent.length() <= 1.0e-5 {
        return None;
    }

    let mut normal = Vec3f::cross(tangent, bitangent).normalize();
    if normal.length() <= 1.0e-5 {
        return None;
    }
    normal = align_direction(seed_normal, normal);
    bitangent = Vec3f::cross(normal, tangent).normalize();
    if bitangent.length() <= 1.0e-5 {
        return None;
    }

    let plane_distance =
        points.iter().map(|point| normal.dot(*point)).sum::<f32>() / points.len() as f32;
    let mean_plane_residual = points
        .iter()
        .map(|point| (normal.dot(*point) - plane_distance).abs())
        .sum::<f32>()
        / points.len() as f32;
    if mean_plane_residual > DEPTH_PLANE_SCAN_VERTICAL_DISTANCE_BIN_METERS {
        return None;
    }

    let extent_padding = DEPTH_PLANE_SUPPORT_CELL_METERS * 0.5;
    let mut min_tangent = f32::INFINITY;
    let mut max_tangent = f32::NEG_INFINITY;
    let mut min_bitangent = f32::INFINITY;
    let mut max_bitangent = f32::NEG_INFINITY;
    for point in &points {
        let tangent_coord = point.dot(tangent);
        let bitangent_coord = point.dot(bitangent);
        min_tangent = min_tangent.min(tangent_coord);
        max_tangent = max_tangent.max(tangent_coord);
        min_bitangent = min_bitangent.min(bitangent_coord);
        max_bitangent = max_bitangent.max(bitangent_coord);
    }
    if !min_tangent.is_finite()
        || !max_tangent.is_finite()
        || !min_bitangent.is_finite()
        || !max_bitangent.is_finite()
    {
        return None;
    }

    min_tangent -= extent_padding;
    max_tangent += extent_padding;
    min_bitangent -= extent_padding;
    max_bitangent += extent_padding;
    let height = max_tangent - min_tangent;
    let width = max_bitangent - min_bitangent;
    if width < DEPTH_PLANE_SCAN_VERTICAL_MIN_DIM_METERS
        || height < DEPTH_PLANE_SCAN_VERTICAL_MIN_DIM_METERS
        || width * height < DEPTH_PLANE_SCAN_VERTICAL_MIN_AREA_METERS2
    {
        return None;
    }

    let center_tangent = (min_tangent + max_tangent) * 0.5;
    let center_bitangent = (min_bitangent + max_bitangent) * 0.5;
    Some(XrDepthPlanePatch {
        generation: 0,
        kind: XrDepthPlaneKind::Wall,
        center: normal.scale(plane_distance)
            + tangent.scale(center_tangent)
            + bitangent.scale(center_bitangent),
        normal,
        tangent,
        bitangent,
        half_extent_tangent: height * 0.5,
        half_extent_bitangent: width * 0.5,
        area: width * height,
        support_triangles: points.len(),
    })
}

fn simplify_plane_regions(
    triangles: Vec<ExtractedPlaneTriangle>,
    voxel_size_meters: f32,
) -> Vec<SimplifiedPlaneRegion> {
    if triangles.is_empty() {
        return Vec::new();
    }

    let mut vertex_links = HashMap::<ExtractedPlaneVertexKey, Vec<usize>>::new();
    for (index, triangle) in triangles.iter().enumerate() {
        for &vertex in &triangle.vertices {
            vertex_links
                .entry(quantize_plane_vertex(vertex, voxel_size_meters))
                .or_default()
                .push(index);
        }
    }

    let mut visited = vec![false; triangles.len()];
    let mut regions = Vec::new();

    for start_index in 0..triangles.len() {
        if visited[start_index] {
            continue;
        }
        let group = triangles[start_index].group;
        let mut queue = VecDeque::from([start_index]);
        let mut region_indices = Vec::new();
        let mut normal_sum = triangles[start_index]
            .normal
            .scale(triangles[start_index].area.max(0.001));
        let mut centroid_sum = triangles[start_index]
            .centroid
            .scale(triangles[start_index].area.max(0.001));
        let mut area_sum = triangles[start_index].area.max(0.001);

        while let Some(triangle_index) = queue.pop_front() {
            if visited[triangle_index] {
                continue;
            }
            let triangle = &triangles[triangle_index];
            if triangle.group != group {
                continue;
            }
            if !planar_region_accepts_triangle(group, triangle, normal_sum, centroid_sum, area_sum)
            {
                continue;
            }

            visited[triangle_index] = true;
            region_indices.push(triangle_index);

            let reference_normal = if normal_sum.length() > 1.0e-5 {
                normal_sum.normalize()
            } else {
                triangle.normal
            };
            let aligned_normal = align_direction(reference_normal, triangle.normal);
            normal_sum += aligned_normal.scale(triangle.area.max(0.001));
            centroid_sum += triangle.centroid.scale(triangle.area.max(0.001));
            area_sum += triangle.area.max(0.001);

            for &vertex in &triangle.vertices {
                if let Some(neighbors) =
                    vertex_links.get(&quantize_plane_vertex(vertex, voxel_size_meters))
                {
                    for &neighbor_index in neighbors {
                        if !visited[neighbor_index] {
                            queue.push_back(neighbor_index);
                        }
                    }
                }
            }
        }

        if region_indices.is_empty() {
            continue;
        }

        if let Some(region) = build_simplified_plane_region(group, &triangles, &region_indices) {
            regions.push(region);
        }
    }

    regions
}

fn collect_classified_plane_triangles_from_surface_mesh(
    mesh: &SurfaceMesh32,
) -> Vec<ExtractedPlaneTriangle> {
    let mut triangles = Vec::new();
    for (source_triangle_index, triangle) in mesh.indices.chunks_exact(3).enumerate() {
        let a = vec3f(
            mesh.positions[triangle[0] as usize][0],
            mesh.positions[triangle[0] as usize][1],
            mesh.positions[triangle[0] as usize][2],
        );
        let b = vec3f(
            mesh.positions[triangle[1] as usize][0],
            mesh.positions[triangle[1] as usize][1],
            mesh.positions[triangle[1] as usize][2],
        );
        let c = vec3f(
            mesh.positions[triangle[2] as usize][0],
            mesh.positions[triangle[2] as usize][1],
            mesh.positions[triangle[2] as usize][2],
        );
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
        let group = if normal.y >= DEPTH_PLANE_HORIZONTAL_NORMAL_Y_MIN {
            Some(ExtractedPlaneGroup::HorizontalUp)
        } else if normal.y <= -DEPTH_PLANE_HORIZONTAL_NORMAL_Y_MIN {
            Some(ExtractedPlaneGroup::HorizontalDown)
        } else if normal.y.abs() <= DEPTH_PLANE_VERTICAL_NORMAL_Y_MAX {
            Some(ExtractedPlaneGroup::Vertical)
        } else {
            None
        };
        let Some(group) = group else {
            continue;
        };
        triangles.push(ExtractedPlaneTriangle {
            source_triangle_index,
            group,
            area,
            normal,
            centroid,
            vertices: [a, b, c],
        });
    }
    triangles
}

fn planar_region_accepts_triangle(
    group: ExtractedPlaneGroup,
    triangle: &ExtractedPlaneTriangle,
    normal_sum: Vec3f,
    centroid_sum: Vec3f,
    area_sum: f32,
) -> bool {
    if area_sum <= 1.0e-5 || normal_sum.length() <= 1.0e-5 {
        return true;
    }

    let region_normal = simplified_region_normal(group, normal_sum);
    let aligned_normal = align_direction(region_normal, triangle.normal);
    let min_normal_dot = if group == ExtractedPlaneGroup::Vertical {
        DEPTH_PLANE_REGION_VERTICAL_NORMAL_DOT
    } else {
        DEPTH_PLANE_SIMPLIFY_REGION_NORMAL_DOT
    };
    if aligned_normal.dot(region_normal) < min_normal_dot {
        return false;
    }

    let region_center = centroid_sum.scale(1.0 / area_sum.max(f32::EPSILON));
    let plane_distance = region_center.dot(region_normal);
    let centroid_distance = (triangle.centroid.dot(region_normal) - plane_distance).abs();
    centroid_distance <= DEPTH_PLANE_SIMPLIFY_REGION_DISTANCE_METERS
}

fn simplified_region_normal(group: ExtractedPlaneGroup, normal_sum: Vec3f) -> Vec3f {
    match group {
        ExtractedPlaneGroup::HorizontalUp => vec3f(0.0, 1.0, 0.0),
        ExtractedPlaneGroup::HorizontalDown => vec3f(0.0, -1.0, 0.0),
        ExtractedPlaneGroup::Vertical => {
            let normal = if normal_sum.length() > 1.0e-5 {
                normal_sum.normalize()
            } else {
                vec3f(1.0, 0.0, 0.0)
            };
            normal.normalize()
        }
    }
}

fn build_simplified_plane_region(
    group: ExtractedPlaneGroup,
    triangles: &[ExtractedPlaneTriangle],
    region_indices: &[usize],
) -> Option<SimplifiedPlaneRegion> {
    let mut normal_sum = Vec3f::default();
    let mut centroid_sum = Vec3f::default();
    let mut area_sum = 0.0f32;

    for &triangle_index in region_indices {
        let triangle = &triangles[triangle_index];
        let reference = if normal_sum.length() > 1.0e-5 {
            normal_sum.normalize()
        } else {
            triangle.normal
        };
        let aligned_normal = align_direction(reference, triangle.normal);
        normal_sum += aligned_normal.scale(triangle.area.max(0.001));
        centroid_sum += triangle.centroid.scale(triangle.area.max(0.001));
        area_sum += triangle.area.max(0.001);
    }

    if area_sum < DEPTH_PLANE_SIMPLIFY_MIN_AREA_METERS2 {
        return Some(SimplifiedPlaneRegion {
            group,
            source_triangle_indices: region_indices
                .iter()
                .map(|&triangle_index| triangles[triangle_index].source_triangle_index)
                .collect(),
            triangles: region_indices
                .iter()
                .map(|&triangle_index| triangles[triangle_index].clone())
                .collect(),
        });
    }

    let normal = simplified_region_normal(group, normal_sum);
    let centroid = centroid_sum.scale(1.0 / area_sum.max(f32::EPSILON));
    let plane_distance = centroid.dot(normal);
    let mut simplified_triangles = Vec::with_capacity(region_indices.len());

    for &triangle_index in region_indices {
        let triangle = &triangles[triangle_index];
        let projected = [
            project_point_onto_plane(triangle.vertices[0], normal, plane_distance),
            project_point_onto_plane(triangle.vertices[1], normal, plane_distance),
            project_point_onto_plane(triangle.vertices[2], normal, plane_distance),
        ];
        let projected_normal_area =
            Vec3f::cross(projected[1] - projected[0], projected[2] - projected[0]);
        let projected_area_twice = projected_normal_area.length();
        if projected_area_twice <= 1.0e-5 {
            continue;
        }
        let projected_area = projected_area_twice * 0.5;
        simplified_triangles.push(ExtractedPlaneTriangle {
            source_triangle_index: triangle.source_triangle_index,
            group,
            area: projected_area,
            normal,
            centroid: (projected[0] + projected[1] + projected[2]).scale(1.0 / 3.0),
            vertices: projected,
        });
    }

    if simplified_triangles.is_empty() {
        return None;
    }

    Some(SimplifiedPlaneRegion {
        group,
        source_triangle_indices: region_indices
            .iter()
            .map(|&triangle_index| triangles[triangle_index].source_triangle_index)
            .collect(),
        triangles: simplified_triangles,
    })
}

fn project_point_onto_plane(point: Vec3f, normal: Vec3f, plane_distance: f32) -> Vec3f {
    point - normal.scale(point.dot(normal) - plane_distance)
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
        center: vec3f(
            rect.center.x,
            y_sum / area_sum.max(f32::EPSILON),
            rect.center.y,
        ),
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

fn quantize_plane_vertex(vertex: Vec3f, voxel_size_meters: f32) -> ExtractedPlaneVertexKey {
    let inv = 1.0 / depth_plane_vertex_link_meters(voxel_size_meters).max(1.0e-5);
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

fn classify_plane_patch_kinds(patches: &mut [XrDepthPlanePatch]) {
    let mut floor_y = None;
    let mut floor_area = 0.0f32;
    let mut min_up_y = f32::INFINITY;
    for patch in patches.iter() {
        if patch.normal.y > DEPTH_PLANE_HORIZONTAL_NORMAL_Y_MIN {
            min_up_y = min_up_y.min(patch.center.y);
        }
    }
    for patch in patches.iter() {
        if patch.normal.y > DEPTH_PLANE_HORIZONTAL_NORMAL_Y_MIN
            && patch.center.y <= min_up_y + 0.25
            && patch.area > floor_area
        {
            floor_area = patch.area;
            floor_y = Some(patch.center.y);
        }
    }

    for patch in patches.iter_mut() {
        if patch.normal.y > DEPTH_PLANE_HORIZONTAL_NORMAL_Y_MIN {
            patch.kind = if floor_y.map(|y| patch.center.y <= y + 0.18).unwrap_or(false) {
                XrDepthPlaneKind::Floor
            } else {
                XrDepthPlaneKind::Table
            };
        } else if patch.normal.y < -DEPTH_PLANE_HORIZONTAL_NORMAL_Y_MIN {
            patch.kind = XrDepthPlaneKind::Ceiling;
        } else if patch.normal.y.abs() <= DEPTH_PLANE_VERTICAL_NORMAL_Y_MAX {
            patch.kind = XrDepthPlaneKind::Wall;
        } else {
            patch.kind = XrDepthPlaneKind::Unknown;
        }
    }
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
            let weight = mask.cells.entry(PlaneSupportCellKey { u, v }).or_insert(0);
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

fn largest_supported_rectangle(component: &PlaneSupportComponent) -> Option<(i32, i32, i32, i32)> {
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

#[derive(Clone, Copy)]
struct ScoredQuerySurfaceHit {
    score: f32,
    surface: XrDepthMeshQuerySurfaceHit,
}

fn query_support_plane_radius(query_radius: f32) -> f32 {
    (query_radius * DEPTH_QUERY_SUPPORT_PLANE_RADIUS_SCALE).clamp(
        DEPTH_QUERY_SUPPORT_PLANE_RADIUS_MIN,
        DEPTH_QUERY_SUPPORT_PLANE_RADIUS_MAX,
    )
}

fn query_support_plane_height_tolerance(query_radius: f32) -> f32 {
    (query_radius * DEPTH_QUERY_SUPPORT_PLANE_HEIGHT_TOLERANCE_SCALE).clamp(
        DEPTH_QUERY_SUPPORT_PLANE_HEIGHT_TOLERANCE_MIN,
        DEPTH_QUERY_SUPPORT_PLANE_HEIGHT_TOLERANCE_MAX,
    )
}

fn query_tsdf_support_radius(query_radius: f32) -> f32 {
    (query_radius * DEPTH_QUERY_TSDF_SUPPORT_RADIUS_SCALE).clamp(
        DEPTH_QUERY_TSDF_SUPPORT_RADIUS_MIN,
        DEPTH_QUERY_TSDF_SUPPORT_RADIUS_MAX,
    )
}

fn query_support_plane_fallback_tangent(normal: Vec3f) -> Vec3f {
    let primary_axis = if normal.y.abs() < 0.9 {
        vec3f(0.0, 1.0, 0.0)
    } else {
        vec3f(1.0, 0.0, 0.0)
    };
    let tangent = Vec3f::cross(primary_axis, normal);
    if tangent.length() > 1.0e-6 {
        tangent.normalize()
    } else {
        Vec3f::cross(vec3f(0.0, 0.0, 1.0), normal).normalize()
    }
}

fn query_support_plane_seed_tangent(normal: Vec3f, surface: XrDepthMeshQuerySurfaceHit) -> Vec3f {
    let mut best_tangent = None;
    let mut best_length_sq = 0.0;
    let edges = [
        surface.triangle[1] - surface.triangle[0],
        surface.triangle[2] - surface.triangle[1],
        surface.triangle[0] - surface.triangle[2],
    ];
    for edge in edges {
        let projected = edge - normal.scale(edge.dot(normal));
        let length_sq = projected.dot(projected);
        if length_sq > best_length_sq && length_sq > 1.0e-8 {
            best_length_sq = length_sq;
            best_tangent = Some(projected.scale(length_sq.sqrt().recip()));
        }
    }
    best_tangent.unwrap_or_else(|| query_support_plane_fallback_tangent(normal))
}

fn solve_linear3(mut a: [[f32; 3]; 3], mut b: [f32; 3]) -> Option<[f32; 3]> {
    for pivot_index in 0..3 {
        let mut pivot_row = pivot_index;
        let mut pivot_abs = a[pivot_index][pivot_index].abs();
        for row in pivot_index + 1..3 {
            let candidate_abs = a[row][pivot_index].abs();
            if candidate_abs > pivot_abs {
                pivot_abs = candidate_abs;
                pivot_row = row;
            }
        }
        if pivot_abs <= 1.0e-8 {
            return None;
        }
        if pivot_row != pivot_index {
            a.swap(pivot_row, pivot_index);
            b.swap(pivot_row, pivot_index);
        }

        let pivot_inv = a[pivot_index][pivot_index].recip();
        for column in pivot_index..3 {
            a[pivot_index][column] *= pivot_inv;
        }
        b[pivot_index] *= pivot_inv;

        for row in 0..3 {
            if row == pivot_index {
                continue;
            }
            let factor = a[row][pivot_index];
            if factor.abs() <= 1.0e-8 {
                continue;
            }
            for column in pivot_index..3 {
                a[row][column] -= factor * a[pivot_index][column];
            }
            b[row] -= factor * b[pivot_index];
        }
    }
    Some(b)
}

fn visit_support_plane_triangles(
    volume: &DepthMeshVolume,
    surface: XrDepthMeshQuerySurfaceHit,
    query_radius: f32,
    mut visitor: impl FnMut([Vec3f; 3], Vec3f, f32, Vec3f),
) {
    let gather_radius = query_support_plane_radius(query_radius);
    let gather_radius_sq = gather_radius * gather_radius;
    let height_tolerance = query_support_plane_height_tolerance(query_radius);
    let seed_normal = surface.normal.normalize();
    let point_min = surface.point;
    let point_max = surface.point;

    for chunk in &volume.mesh_chunks {
        if aabb_aabb_distance_sq(point_min, point_max, chunk.bounds_min, chunk.bounds_max)
            > gather_radius_sq
        {
            continue;
        }
        for triangle in chunk.indices.chunks_exact(3) {
            let triangle = [
                chunk.vertices[triangle[0] as usize],
                chunk.vertices[triangle[1] as usize],
                chunk.vertices[triangle[2] as usize],
            ];
            let raw_normal = Vec3f::cross(triangle[1] - triangle[0], triangle[2] - triangle[0]);
            let raw_length = raw_normal.length();
            if raw_length <= 1.0e-6 {
                continue;
            }

            let mut normal = raw_normal.scale(raw_length.recip());
            if normal.dot(seed_normal) < 0.0 {
                normal = normal.scale(-1.0);
            }
            if normal.dot(seed_normal) < DEPTH_QUERY_SUPPORT_PLANE_NORMAL_DOT_MIN
                || normal.y < DEPTH_QUERY_SUPPORT_NORMAL_Y_MIN
            {
                continue;
            }

            let closest =
                closest_point_on_triangle(surface.point, triangle[0], triangle[1], triangle[2]);
            let delta = closest - surface.point;
            if delta.dot(delta) > gather_radius_sq {
                continue;
            }

            let max_plane_offset = triangle
                .iter()
                .map(|vertex| (seed_normal.dot(*vertex - surface.point)).abs())
                .fold(0.0f32, f32::max);
            if max_plane_offset > height_tolerance {
                continue;
            }

            let centroid = (triangle[0] + triangle[1] + triangle[2]).scale(1.0 / 3.0);
            visitor(triangle, normal, raw_length * 0.5, centroid);
        }
    }
}

fn query_support_plane_fingerprint(
    plane: &XrDepthMeshQuerySupportPlane,
    role: XrDepthMeshQueryColliderRole,
) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    role.hash(&mut hasher);
    [
        quantize_f32(plane.normal.x, 0.02),
        quantize_f32(plane.normal.y, 0.02),
        quantize_f32(plane.normal.z, 0.02),
        quantize_f32(plane.normal.dot(plane.point), 0.01),
    ]
    .hash(&mut hasher);
    hasher.finish()
}

fn make_query_halfspace_collider(
    plane: XrDepthMeshQuerySupportPlane,
    role: XrDepthMeshQueryColliderRole,
    restitution: f32,
) -> XrDepthMeshQueryCollider {
    XrDepthMeshQueryCollider {
        fingerprint: query_support_plane_fingerprint(&plane, role),
        geometry: XrDepthMeshQueryColliderGeometry::HalfSpace(plane),
        role,
        restitution: restitution.max(0.0),
    }
}

fn build_query_surface_halfspace_from_patch(
    surface: XrDepthMeshQuerySurfaceHit,
    query_radius: f32,
) -> XrDepthMeshQuerySupportPlane {
    let tangent_raw = surface.patch[1] - surface.patch[0];
    let bitangent_raw = surface.patch[3] - surface.patch[0];
    let tangent = if tangent_raw.length() > 1.0e-6 {
        tangent_raw.normalize()
    } else {
        query_support_plane_seed_tangent(surface.normal, surface)
    };
    let bitangent = if bitangent_raw.length() > 1.0e-6 {
        bitangent_raw.normalize()
    } else {
        Vec3f::cross(surface.normal, tangent).normalize()
    };
    let debug_half_extent = query_support_plane_radius(query_radius);
    XrDepthMeshQuerySupportPlane {
        point: closest_point_on_plane_patch(
            surface.point,
            &XrDepthPlanePatch {
                generation: 0,
                kind: XrDepthPlaneKind::Unknown,
                center: (surface.patch[0] + surface.patch[1] + surface.patch[2] + surface.patch[3])
                    .scale(0.25),
                normal: surface.normal,
                tangent,
                bitangent,
                half_extent_tangent: (surface.patch[1] - surface.patch[0]).length() * 0.5,
                half_extent_bitangent: (surface.patch[3] - surface.patch[0]).length() * 0.5,
                area: 0.0,
                support_triangles: 0,
            },
        ),
        normal: surface.normal,
        tangent,
        bitangent,
        half_extent_tangent: debug_half_extent,
        half_extent_bitangent: debug_half_extent,
    }
}

fn build_query_surface_halfspace_from_triangles(
    volume: &DepthMeshVolume,
    surface: XrDepthMeshQuerySurfaceHit,
    query_radius: f32,
) -> XrDepthMeshQuerySupportPlane {
    let mut sum_w = 0.0;
    let mut weighted_normal = vec3f(0.0, 0.0, 0.0);
    let mut sum_xx = 0.0;
    let mut sum_xz = 0.0;
    let mut sum_x = 0.0;
    let mut sum_zz = 0.0;
    let mut sum_z = 0.0;
    let mut sum_xy = 0.0;
    let mut sum_zy = 0.0;
    let mut sum_y = 0.0;

    visit_support_plane_triangles(
        volume,
        surface,
        query_radius,
        |triangle, normal, area, _centroid| {
            weighted_normal = weighted_normal + normal.scale(area);
            let vertex_weight = area * (1.0 / 3.0);
            for vertex in triangle {
                let local = vertex - surface.point;
                let x = local.x;
                let z = local.z;
                let y = local.y;
                sum_w += vertex_weight;
                sum_xx += vertex_weight * x * x;
                sum_xz += vertex_weight * x * z;
                sum_x += vertex_weight * x;
                sum_zz += vertex_weight * z * z;
                sum_z += vertex_weight * z;
                sum_xy += vertex_weight * x * y;
                sum_zy += vertex_weight * z * y;
                sum_y += vertex_weight * y;
            }
        },
    );

    let avg_normal = if weighted_normal.length() > 1.0e-6 {
        weighted_normal.normalize()
    } else {
        surface.normal.normalize()
    };
    let mut normal = solve_linear3(
        [
            [sum_xx, sum_xz, sum_x],
            [sum_xz, sum_zz, sum_z],
            [sum_x, sum_z, sum_w],
        ],
        [sum_xy, sum_zy, sum_y],
    )
    .map(|solution| vec3f(-solution[0], 1.0, -solution[1]).normalize())
    .unwrap_or(avg_normal);
    if normal.dot(avg_normal) < 0.0 {
        normal = normal.scale(-1.0);
    }
    normal = (normal + avg_normal.scale(0.75)).normalize();
    let up_blend = ((avg_normal.y - DEPTH_QUERY_SUPPORT_NORMAL_Y_MIN)
        / (1.0 - DEPTH_QUERY_SUPPORT_NORMAL_Y_MIN))
        .clamp(0.0, 1.0);
    normal = (normal + vec3f(0.0, 1.0, 0.0).scale(up_blend * 1.25)).normalize();

    let tangent = query_support_plane_seed_tangent(normal, surface);
    let bitangent = Vec3f::cross(normal, tangent).normalize();
    let debug_half_extent_max = query_support_plane_radius(query_radius);
    let debug_half_extent_min =
        (query_radius * 1.1).max(DEPTH_QUERY_SUPPORT_PLANE_DEBUG_HALF_EXTENT_MIN);
    let mut max_plane_offset = normal.dot(surface.point);
    let mut min_u = f32::INFINITY;
    let mut max_u = -f32::INFINITY;
    let mut min_v = f32::INFINITY;
    let mut max_v = -f32::INFINITY;

    visit_support_plane_triangles(
        volume,
        surface,
        query_radius,
        |triangle, _normal, _area, _centroid| {
            for vertex in triangle {
                max_plane_offset = max_plane_offset.max(normal.dot(vertex));
                let offset = vertex - surface.point;
                let u = offset.dot(tangent);
                let v = offset.dot(bitangent);
                min_u = min_u.min(u);
                max_u = max_u.max(u);
                min_v = min_v.min(v);
                max_v = max_v.max(v);
            }
        },
    );
    let point = surface.point - normal.scale(normal.dot(surface.point) - max_plane_offset);

    let half_extent_tangent = if min_u.is_finite() && max_u.is_finite() {
        ((max_u - min_u) * 0.5 + query_radius * 0.5)
            .clamp(debug_half_extent_min, debug_half_extent_max)
    } else {
        debug_half_extent_min
    };
    let half_extent_bitangent = if min_v.is_finite() && max_v.is_finite() {
        ((max_v - min_v) * 0.5 + query_radius * 0.5)
            .clamp(debug_half_extent_min, debug_half_extent_max)
    } else {
        debug_half_extent_min
    };

    XrDepthMeshQuerySupportPlane {
        point,
        normal,
        tangent,
        bitangent,
        half_extent_tangent,
        half_extent_bitangent,
    }
}

fn build_query_surface_collider(
    volume: &DepthMeshVolume,
    surface: XrDepthMeshQuerySurfaceHit,
    query_radius: f32,
) -> XrDepthMeshQueryCollider {
    let plane = if surface.from_planar_patch {
        build_query_surface_halfspace_from_patch(surface, query_radius)
    } else {
        build_query_surface_halfspace_from_triangles(volume, surface, query_radius)
    };
    let role = if query_surface_priority(surface) == 0 {
        XrDepthMeshQueryColliderRole::Support
    } else {
        XrDepthMeshQueryColliderRole::Impact
    };
    let restitution = if role == XrDepthMeshQueryColliderRole::Impact {
        DEPTH_QUERY_TSDF_IMPACT_RESTITUTION
    } else {
        0.0
    };
    make_query_halfspace_collider(plane, role, restitution)
}

fn build_query_surface_result(
    volume: &DepthMeshVolume,
    surface: XrDepthMeshQuerySurfaceHit,
    query_radius: f32,
) -> XrDepthMeshQueryResolvedSurface {
    XrDepthMeshQueryResolvedSurface {
        surface,
        collider: build_query_surface_collider(volume, surface, query_radius),
    }
}

#[derive(Clone, Copy)]
struct DepthGridSupportSample {
    point: Vec3f,
    radial_weight: f32,
}

fn voxel_center_axis(voxel_size: f32, coord: i32) -> f32 {
    (coord as f32 + 0.5) * voxel_size
}

fn make_query_halfspace_surface(
    distance: f32,
    plane: XrDepthMeshQuerySupportPlane,
    role: XrDepthMeshQueryColliderRole,
    restitution: f32,
) -> XrDepthMeshQueryResolvedSurface {
    let patch = query_support_plane_quad(plane);
    XrDepthMeshQueryResolvedSurface {
        surface: XrDepthMeshQuerySurfaceHit {
            distance,
            point: plane.point,
            normal: plane.normal,
            from_planar_patch: true,
            triangle: [patch[0], patch[1], patch[2]],
            patch,
            chunk_key: ChunkKey::new(0, 0, 0),
        },
        collider: make_query_halfspace_collider(plane, role, restitution),
    }
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

fn query_depth_grid_trilinear_distance(volume: &DepthMeshVolume, point: Vec3f) -> Option<f32> {
    let voxel_size = volume.voxel_size_meters;
    let tsd_distance_meters = depth_tsd_distance_meters(voxel_size);
    let grid_x = point.x / voxel_size - 0.5;
    let grid_y = point.y / voxel_size - 0.5;
    let grid_z = point.z / voxel_size - 0.5;
    let x0 = grid_x.floor() as i32;
    let y0 = grid_y.floor() as i32;
    let z0 = grid_z.floor() as i32;
    let tx = grid_x - x0 as f32;
    let ty = grid_y - y0 as f32;
    let tz = grid_z - z0 as f32;

    let sample = |x: i32, y: i32, z: i32| -> Option<f32> {
        volume
            .mesh_grid
            .normalized_distance(VoxelCoord::new(x, y, z))
            .map(|distance| distance * tsd_distance_meters)
    };

    let s000 = sample(x0, y0, z0)?;
    let s100 = sample(x0 + 1, y0, z0)?;
    let s010 = sample(x0, y0 + 1, z0)?;
    let s110 = sample(x0 + 1, y0 + 1, z0)?;
    let s001 = sample(x0, y0, z0 + 1)?;
    let s101 = sample(x0 + 1, y0, z0 + 1)?;
    let s011 = sample(x0, y0 + 1, z0 + 1)?;
    let s111 = sample(x0 + 1, y0 + 1, z0 + 1)?;

    let x00 = s000 + (s100 - s000) * tx;
    let x10 = s010 + (s110 - s010) * tx;
    let x01 = s001 + (s101 - s001) * tx;
    let x11 = s011 + (s111 - s011) * tx;
    let y0_mix = x00 + (x10 - x00) * ty;
    let y1_mix = x01 + (x11 - x01) * ty;
    Some(y0_mix + (y1_mix - y0_mix) * tz)
}

fn finite_difference_axis(
    center: f32,
    forward: Option<f32>,
    backward: Option<f32>,
    step: f32,
) -> Option<f32> {
    match (forward, backward) {
        (Some(forward), Some(backward)) => Some((forward - backward) / (step * 2.0)),
        (Some(forward), None) => Some((forward - center) / step),
        (None, Some(backward)) => Some((center - backward) / step),
        (None, None) => None,
    }
}

fn query_depth_grid_distance_gradient(volume: &DepthMeshVolume, point: Vec3f) -> Option<Vec3f> {
    let center = query_depth_grid_trilinear_distance(volume, point)?;
    let step = volume
        .voxel_size_meters
        .max(DEPTH_QUERY_TSDF_IMPACT_RAY_STEP_MIN);
    let dx = finite_difference_axis(
        center,
        query_depth_grid_trilinear_distance(volume, point + vec3f(step, 0.0, 0.0)),
        query_depth_grid_trilinear_distance(volume, point - vec3f(step, 0.0, 0.0)),
        step,
    )?;
    let dy = finite_difference_axis(
        center,
        query_depth_grid_trilinear_distance(volume, point + vec3f(0.0, step, 0.0)),
        query_depth_grid_trilinear_distance(volume, point - vec3f(0.0, step, 0.0)),
        step,
    )?;
    let dz = finite_difference_axis(
        center,
        query_depth_grid_trilinear_distance(volume, point + vec3f(0.0, 0.0, step)),
        query_depth_grid_trilinear_distance(volume, point - vec3f(0.0, 0.0, step)),
        step,
    )?;
    let gradient = vec3f(dx, dy, dz);
    (gradient.length() > 1.0e-5).then_some(gradient.normalize())
}

fn query_tsdf_impact_half_extent(query_radius: f32, voxel_size: f32) -> f32 {
    (query_radius * DEPTH_QUERY_TSDF_IMPACT_EXTENT_SCALE + voxel_size * 0.5).clamp(
        DEPTH_QUERY_TSDF_IMPACT_EXTENT_MIN,
        DEPTH_QUERY_TSDF_IMPACT_EXTENT_MAX,
    )
}

fn evaluate_depth_grid_impact_query(
    volume: &DepthMeshVolume,
    query: XrDepthMeshQuery,
) -> Option<XrDepthMeshQueryResolvedSurface> {
    let travel = query.predicted_center - query.center;
    let travel_distance = travel.length();
    let velocity_length = query.velocity.length();
    let horizontal_speed = vec2f(query.velocity.x, query.velocity.z).length();
    let upward_speed = query.velocity.y.max(0.0);
    if velocity_length < DEPTH_QUERY_TSDF_IMPACT_MIN_SPEED && travel_distance < 0.03 {
        return None;
    }
    if horizontal_speed < DEPTH_QUERY_TSDF_IMPACT_MIN_HORIZONTAL_SPEED
        && upward_speed < DEPTH_QUERY_TSDF_IMPACT_MIN_UPWARD_SPEED
    {
        return None;
    }

    let motion_dir = if velocity_length > 1.0e-4 {
        query.velocity.scale(1.0 / velocity_length)
    } else if travel_distance > 1.0e-4 {
        travel.scale(1.0 / travel_distance)
    } else {
        return None;
    };
    let max_search_distance = (travel_distance + query.radius + query.max_distance)
        .max(query.radius + volume.voxel_size_meters * 0.75);
    let step_distance = (volume.voxel_size_meters * DEPTH_QUERY_TSDF_IMPACT_RAY_STEP_SCALE)
        .max(DEPTH_QUERY_TSDF_IMPACT_RAY_STEP_MIN)
        .min(max_search_distance);
    let hit_threshold = query.radius + volume.voxel_size_meters * 0.20;
    let mut previous_t = 0.0f32;
    let mut t = step_distance;

    while t <= max_search_distance + 1.0e-4 {
        let sample_position = query.center + motion_dir.scale(t);
        let Some(sample_distance) = query_depth_grid_trilinear_distance(volume, sample_position)
        else {
            previous_t = t;
            t += step_distance;
            continue;
        };
        if sample_distance <= hit_threshold {
            let mut lo = previous_t;
            let mut hi = t;
            for _ in 0..5 {
                let mid = (lo + hi) * 0.5;
                let mid_position = query.center + motion_dir.scale(mid);
                if let Some(mid_distance) =
                    query_depth_grid_trilinear_distance(volume, mid_position)
                {
                    if mid_distance <= hit_threshold {
                        hi = mid;
                    } else {
                        lo = mid;
                    }
                }
            }

            let hit_position = query.center + motion_dir.scale(hi);
            let signed_distance = query_depth_grid_trilinear_distance(volume, hit_position)?;
            let mut normal = query_depth_grid_distance_gradient(volume, hit_position)?;
            let mut opposing_dot = normal.dot(motion_dir.scale(-1.0));
            if opposing_dot <= DEPTH_QUERY_MIN_OPPOSING_NORMAL_DOT {
                let flipped = normal.scale(-1.0);
                let flipped_opposing_dot = flipped.dot(motion_dir.scale(-1.0));
                if flipped_opposing_dot > DEPTH_QUERY_MIN_OPPOSING_NORMAL_DOT {
                    normal = flipped;
                    opposing_dot = flipped_opposing_dot;
                }
            }
            let is_lateral_impact = normal.y.abs() <= DEPTH_QUERY_TSDF_IMPACT_NORMAL_Y_MAX;
            let is_ceiling_impact = upward_speed >= DEPTH_QUERY_TSDF_IMPACT_MIN_UPWARD_SPEED
                && normal.y <= -DEPTH_QUERY_TSDF_IMPACT_CEILING_NORMAL_Y_MIN;
            if !(is_lateral_impact || is_ceiling_impact)
                || opposing_dot <= DEPTH_QUERY_MIN_OPPOSING_NORMAL_DOT
            {
                previous_t = t;
                t += step_distance;
                continue;
            }

            let tangent_raw = motion_dir - normal.scale(motion_dir.dot(normal));
            let tangent = if tangent_raw.length() > 1.0e-5 {
                tangent_raw.normalize()
            } else {
                query_support_plane_fallback_tangent(normal)
            };
            let bitangent = Vec3f::cross(normal, tangent).normalize();
            let half_extent = query_tsdf_impact_half_extent(query.radius, volume.voxel_size_meters);
            let plane = XrDepthMeshQuerySupportPlane {
                point: hit_position - normal.scale(signed_distance),
                normal,
                tangent,
                bitangent,
                half_extent_tangent: half_extent,
                half_extent_bitangent: half_extent,
            };
            return Some(make_query_halfspace_surface(
                signed_distance.abs(),
                plane,
                XrDepthMeshQueryColliderRole::Impact,
                DEPTH_QUERY_TSDF_IMPACT_RESTITUTION,
            ));
        }
        previous_t = t;
        t += step_distance;
    }

    None
}

fn query_depth_grid_first_support_height(
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

    None
}

fn query_support_plane_quad(plane: XrDepthMeshQuerySupportPlane) -> [Vec3f; 4] {
    let tangent = plane.tangent.scale(plane.half_extent_tangent);
    let bitangent = plane.bitangent.scale(plane.half_extent_bitangent);
    [
        plane.point - tangent - bitangent,
        plane.point + tangent - bitangent,
        plane.point + tangent + bitangent,
        plane.point - tangent + bitangent,
    ]
}

fn make_query_hit_from_resolved_surface(
    query: XrDepthMeshQuery,
    version: u64,
    mesh_generation: u64,
    surface: XrDepthMeshQueryResolvedSurface,
) -> XrDepthMeshQueryHit {
    XrDepthMeshQueryHit {
        key: query.key,
        version,
        mesh_generation,
        distance: surface.surface.distance,
        point: surface.surface.point,
        normal: surface.surface.normal,
        from_planar_patch: surface.surface.from_planar_patch,
        triangle: surface.surface.triangle,
        patch: surface.surface.patch,
        chunk_key: surface.surface.chunk_key,
        collider: surface.collider,
        additional_hits: Vec::new(),
    }
}

fn evaluate_depth_grid_support_query(
    volume: &DepthMeshVolume,
    query: XrDepthMeshQuery,
    version: u64,
) -> Option<XrDepthMeshQueryHit> {
    const GRID_LAST: f32 = (DEPTH_QUERY_TSDF_SUPPORT_GRID_DIM - 1) as f32;

    let trajectory_samples = query_trajectory_samples(query);
    let (trajectory_bounds_min, trajectory_bounds_max, travel_distance) =
        query_trajectory_bounds_and_length(&trajectory_samples);
    let support_radius = query_tsdf_support_radius(query.radius);
    let tsd_distance_meters = depth_tsd_distance_meters(volume.voxel_size_meters);
    let top_y = trajectory_bounds_max.y + query.radius + volume.voxel_size_meters;
    let bottom_y = trajectory_bounds_min.y
        - (query.radius + query.max_distance + travel_distance + tsd_distance_meters);
    let (_, search_sample, center_support_y) = trajectory_samples
        .iter()
        .filter_map(|sample| {
            let support_y = query_depth_grid_first_support_height(
                volume,
                sample.point.x,
                sample.point.z,
                top_y,
                bottom_y,
            )?;
            let support_point = vec3f(sample.point.x, support_y, sample.point.z);
            let score = (sample.point - support_point).length()
                - sample.progress * (query.radius + volume.voxel_size_meters) * 0.35;
            Some((score, *sample, support_y))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))?;
    let search_center = search_sample.point;

    let mut samples = [None; DEPTH_QUERY_TSDF_SUPPORT_MAX_SAMPLES];
    let mut sample_count = 0usize;
    let mut max_height = f32::NEG_INFINITY;

    for row in 0..DEPTH_QUERY_TSDF_SUPPORT_GRID_DIM {
        for column in 0..DEPTH_QUERY_TSDF_SUPPORT_GRID_DIM {
            let u = if GRID_LAST > 0.0 {
                column as f32 / GRID_LAST * 2.0 - 1.0
            } else {
                0.0
            };
            let v = if GRID_LAST > 0.0 {
                row as f32 / GRID_LAST * 2.0 - 1.0
            } else {
                0.0
            };
            let sample_x = search_center.x + u * support_radius;
            let sample_z = search_center.z + v * support_radius;
            let Some(sample_y) =
                query_depth_grid_first_support_height(volume, sample_x, sample_z, top_y, bottom_y)
            else {
                continue;
            };
            let radial_weight = 1.0 / (1.0 + (u * u + v * v) * 1.5);
            let point = vec3f(sample_x, sample_y, sample_z);
            max_height = max_height.max(sample_y);
            samples[sample_count] = Some(DepthGridSupportSample {
                point,
                radial_weight,
            });
            sample_count += 1;
        }
    }

    if sample_count < DEPTH_QUERY_TSDF_SUPPORT_MIN_SAMPLES {
        return None;
    }

    let mut height_tolerance =
        query_support_plane_height_tolerance(query.radius).max(volume.voxel_size_meters * 0.25);
    let mut selected_count = 0usize;
    for _ in 0..3 {
        selected_count = samples[..sample_count]
            .iter()
            .filter_map(|sample| *sample)
            .filter(|sample| max_height - sample.point.y <= height_tolerance)
            .count();
        if selected_count >= DEPTH_QUERY_TSDF_SUPPORT_MIN_SAMPLES {
            break;
        }
        height_tolerance += volume.voxel_size_meters * 0.35;
    }
    if selected_count < DEPTH_QUERY_TSDF_SUPPORT_MIN_SAMPLES {
        height_tolerance = f32::INFINITY;
        selected_count = sample_count;
    }

    let mut sum_w = 0.0;
    let mut sum_xx = 0.0;
    let mut sum_xz = 0.0;
    let mut sum_x = 0.0;
    let mut sum_zz = 0.0;
    let mut sum_z = 0.0;
    let mut sum_xy = 0.0;
    let mut sum_zy = 0.0;
    let mut sum_y = 0.0;

    for sample in samples[..sample_count].iter().filter_map(|sample| *sample) {
        if max_height - sample.point.y > height_tolerance {
            continue;
        }
        let local = sample.point - search_center;
        let weight = sample.radial_weight;
        sum_w += weight;
        sum_xx += weight * local.x * local.x;
        sum_xz += weight * local.x * local.z;
        sum_x += weight * local.x;
        sum_zz += weight * local.z * local.z;
        sum_z += weight * local.z;
        sum_xy += weight * local.x * local.y;
        sum_zy += weight * local.z * local.y;
        sum_y += weight * local.y;
    }

    if selected_count < 3 || sum_w <= 1.0e-5 {
        return None;
    }

    let mut normal = solve_linear3(
        [
            [sum_xx, sum_xz, sum_x],
            [sum_xz, sum_zz, sum_z],
            [sum_x, sum_z, sum_w],
        ],
        [sum_xy, sum_zy, sum_y],
    )
    .map(|solution| vec3f(-solution[0], 1.0, -solution[1]).normalize())
    .unwrap_or(vec3f(0.0, 1.0, 0.0));
    if normal.y < 0.0 {
        normal = normal.scale(-1.0);
    }
    normal = (normal + vec3f(0.0, 1.0, 0.0).scale(0.9)).normalize();
    if normal.y < DEPTH_QUERY_TSDF_SUPPORT_NORMAL_Y_MIN {
        return None;
    }

    let tangent = query_support_plane_fallback_tangent(normal);
    let bitangent = Vec3f::cross(normal, tangent).normalize();
    let mut plane_offset = f32::NEG_INFINITY;
    let mut min_u = f32::INFINITY;
    let mut max_u = f32::NEG_INFINITY;
    let mut min_v = f32::INFINITY;
    let mut max_v = f32::NEG_INFINITY;

    for sample in samples[..sample_count].iter().filter_map(|sample| *sample) {
        if max_height - sample.point.y > height_tolerance {
            continue;
        }
        plane_offset = plane_offset.max(normal.dot(sample.point));
        let offset = sample.point - search_center;
        let u = offset.dot(tangent);
        let v = offset.dot(bitangent);
        min_u = min_u.min(u);
        max_u = max_u.max(u);
        min_v = min_v.min(v);
        max_v = max_v.max(v);
    }

    let point = search_center - normal.scale(normal.dot(search_center) - plane_offset);
    let extent_padding = (query.radius * DEPTH_QUERY_TSDF_SUPPORT_EXTENT_PADDING_SCALE)
        .max(volume.voxel_size_meters * 0.45);
    let debug_half_extent_max = support_radius;
    let debug_half_extent_min = (query.radius * 0.9)
        .max(volume.voxel_size_meters * 0.35)
        .min(debug_half_extent_max);
    let half_extent_tangent = if min_u.is_finite() && max_u.is_finite() {
        ((max_u - min_u) * 0.5 + extent_padding).clamp(debug_half_extent_min, debug_half_extent_max)
    } else {
        debug_half_extent_min
    };
    let half_extent_bitangent = if min_v.is_finite() && max_v.is_finite() {
        ((max_v - min_v) * 0.5 + extent_padding).clamp(debug_half_extent_min, debug_half_extent_max)
    } else {
        debug_half_extent_min
    };

    let plane = XrDepthMeshQuerySupportPlane {
        point,
        normal,
        tangent,
        bitangent,
        half_extent_tangent,
        half_extent_bitangent,
    };
    let center_support_point = vec3f(search_center.x, center_support_y, search_center.z);
    let support_point =
        center_support_point - normal.scale(normal.dot(center_support_point) - plane_offset);
    let support_surface = make_query_halfspace_surface(
        (support_point - search_center).length(),
        XrDepthMeshQuerySupportPlane {
            point: support_point,
            ..plane
        },
        XrDepthMeshQueryColliderRole::Support,
        0.0,
    );
    let impact_surface = evaluate_depth_grid_impact_query(volume, query);
    let additional_hits = impact_surface.into_iter().collect::<Vec<_>>();

    Some(XrDepthMeshQueryHit {
        key: query.key,
        version,
        mesh_generation: volume.mesh_generation,
        distance: support_surface.surface.distance,
        point: support_surface.surface.point,
        normal: support_surface.surface.normal,
        from_planar_patch: support_surface.surface.from_planar_patch,
        triangle: support_surface.surface.triangle,
        patch: support_surface.surface.patch,
        chunk_key: support_surface.surface.chunk_key,
        collider: support_surface.collider,
        additional_hits,
    })
}

fn evaluate_geometry_query(
    volume: &DepthMeshVolume,
    query: XrDepthMeshQuery,
    version: u64,
) -> XrDepthMeshQueryResult {
    if !volume.mesh_grid.is_empty() {
        let impact_surface = evaluate_depth_grid_impact_query(volume, query);
        let prefer_impact = impact_surface.as_ref().is_some_and(|impact_surface| {
            let XrDepthMeshQueryColliderGeometry::HalfSpace(plane) =
                &impact_surface.collider.geometry;
            query.velocity.y >= DEPTH_QUERY_TSDF_IMPACT_MIN_UPWARD_SPEED
                && plane.normal.y <= -DEPTH_QUERY_TSDF_IMPACT_CEILING_NORMAL_Y_MIN
        });
        if prefer_impact {
            return impact_surface
                .map(|impact_surface| {
                    XrDepthMeshQueryResult::Hit(make_query_hit_from_resolved_surface(
                        query,
                        version,
                        volume.mesh_generation,
                        impact_surface,
                    ))
                })
                .unwrap_or(XrDepthMeshQueryResult::Miss {
                    key: query.key,
                    version,
                    mesh_generation: volume.mesh_generation,
                });
        }
        return evaluate_depth_grid_support_query(volume, query, version)
            .or_else(|| {
                impact_surface.map(|impact_surface| {
                    make_query_hit_from_resolved_surface(
                        query,
                        version,
                        volume.mesh_generation,
                        impact_surface,
                    )
                })
            })
            .map(XrDepthMeshQueryResult::Hit)
            .unwrap_or(XrDepthMeshQueryResult::Miss {
                key: query.key,
                version,
                mesh_generation: volume.mesh_generation,
            });
    }

    let trajectory_samples = query_trajectory_samples(query);
    let (sweep_bounds_min, sweep_bounds_max, travel_distance) =
        query_trajectory_bounds_and_length(&trajectory_samples);
    let max_search_distance = (query.radius + query.max_distance + travel_distance).max(0.0);
    let max_search_distance_sq = max_search_distance * max_search_distance;
    let mut best_hits = [None; DEPTH_QUERY_MAX_SURFACES_PER_QUERY];
    let sweep_radius = query.radius + query.max_distance;
    let sweep_radius_sq = sweep_radius * sweep_radius;

    if query.include_planar_patches {
        for patch in &volume.plane_patches {
            let patch_corners = plane_patch_corners(patch);
            let mut patch_bounds_min = patch_corners[0];
            let mut patch_bounds_max = patch_corners[0];
            for &corner in &patch_corners[1..] {
                patch_bounds_min = Vec3f::min_componentwise(patch_bounds_min, corner);
                patch_bounds_max = Vec3f::max_componentwise(patch_bounds_max, corner);
            }
            if aabb_aabb_distance_sq(
                sweep_bounds_min,
                sweep_bounds_max,
                patch_bounds_min,
                patch_bounds_max,
            ) > max_search_distance_sq
            {
                continue;
            }

            let mut best_sample_score = f32::INFINITY;
            let mut best_closest = vec3f(0.0, 0.0, 0.0);
            let mut best_distance_sq = f32::INFINITY;
            let mut best_facing_point = query.center;

            for sample in &trajectory_samples {
                let sample_point = sample.point;
                let closest = closest_point_on_plane_patch(sample_point, patch);
                let delta = closest - sample_point;
                let distance_sq = delta.dot(delta);
                if distance_sq > max_search_distance_sq {
                    continue;
                }
                let lateral_sq = point_polyline_distance_sq(closest, &trajectory_samples);
                if lateral_sq > sweep_radius_sq {
                    continue;
                }

                let distance = distance_sq.sqrt();
                let mut score = distance;
                if travel_distance > 1.0e-4 {
                    let motion = if sample.velocity.length() > 1.0e-4 {
                        sample.velocity.normalize()
                    } else {
                        let fallback = query.predicted_center - query.center;
                        if fallback.length() > 1.0e-4 {
                            fallback.normalize()
                        } else {
                            vec3f(0.0, 0.0, 0.0)
                        }
                    };
                    let mut candidate_normal = patch.normal;
                    if candidate_normal.dot(sample_point - closest) < 0.0 {
                        candidate_normal = candidate_normal.scale(-1.0);
                    }
                    let opposing = candidate_normal.dot(-motion);
                    if opposing <= DEPTH_QUERY_MIN_OPPOSING_NORMAL_DOT {
                        continue;
                    }
                    score -= sample.progress * travel_distance * 0.35;
                    score -= opposing * 0.08;
                    score += lateral_sq.sqrt() * 0.2;
                }
                if score < best_sample_score {
                    best_sample_score = score;
                    best_closest = closest;
                    best_distance_sq = distance_sq;
                    best_facing_point = sample_point;
                }
            }

            if !best_distance_sq.is_finite() {
                continue;
            }

            let mut normal = patch.normal;
            if normal.dot(best_facing_point - best_closest) < 0.0 {
                normal = normal.scale(-1.0);
            }

            let surface = XrDepthMeshQuerySurfaceHit {
                distance: best_distance_sq.sqrt(),
                point: best_closest,
                normal,
                from_planar_patch: true,
                triangle: [patch_corners[0], patch_corners[1], patch_corners[2]],
                patch: patch_corners,
                chunk_key: ChunkKey::new(0, 0, 0),
            };
            consider_query_surface_candidate(
                &mut best_hits,
                best_sample_score,
                surface,
                query.radius,
            );
        }
    }

    for chunk in &volume.mesh_chunks {
        if aabb_aabb_distance_sq(
            sweep_bounds_min,
            sweep_bounds_max,
            chunk.bounds_min,
            chunk.bounds_max,
        ) > max_search_distance_sq
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
            let mut best_sample_score = f32::INFINITY;
            let mut best_closest = vec3f(0.0, 0.0, 0.0);
            let mut best_distance_sq = f32::INFINITY;
            let mut best_facing_point = query.center;

            for sample in &trajectory_samples {
                let sample_point = sample.point;
                let closest = closest_point_on_triangle(sample_point, a, b, c);
                let delta = closest - sample_point;
                let distance_sq = delta.dot(delta);
                if distance_sq > max_search_distance_sq {
                    continue;
                }
                let lateral_sq = point_polyline_distance_sq(closest, &trajectory_samples);
                if lateral_sq > sweep_radius_sq {
                    continue;
                }
                let distance = distance_sq.sqrt();
                let mut score = distance;
                if travel_distance > 1.0e-4 {
                    let motion = if sample.velocity.length() > 1.0e-4 {
                        sample.velocity.normalize()
                    } else {
                        let fallback = query.predicted_center - query.center;
                        if fallback.length() > 1.0e-4 {
                            fallback.normalize()
                        } else {
                            vec3f(0.0, 0.0, 0.0)
                        }
                    };
                    let mut candidate_normal = raw_normal.normalize();
                    if candidate_normal.dot(sample_point - closest) < 0.0 {
                        candidate_normal = candidate_normal.scale(-1.0);
                    }
                    let opposing = candidate_normal.dot(-motion);
                    if opposing <= DEPTH_QUERY_MIN_OPPOSING_NORMAL_DOT {
                        continue;
                    }
                    score -= sample.progress * travel_distance * 0.35;
                    score -= opposing * 0.08;
                    score += lateral_sq.sqrt() * 0.2;
                }
                if score < best_sample_score {
                    best_sample_score = score;
                    best_closest = closest;
                    best_distance_sq = distance_sq;
                    best_facing_point = sample_point;
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
            if normal.dot(best_facing_point - best_closest) < 0.0 {
                normal = normal.scale(-1.0);
                hit_triangle.swap(1, 2);
            }
            let matched_planar_patch =
                matching_reduced_planar_patch(hit_triangle, normal, &chunk.planar_patches);
            let surface = XrDepthMeshQuerySurfaceHit {
                distance: best_distance_sq.sqrt(),
                point: best_closest,
                normal,
                from_planar_patch: matched_planar_patch.is_some(),
                triangle: hit_triangle,
                patch: matched_planar_patch
                    .map(|patch| plane_patch_corners(patch))
                    .unwrap_or([best_closest; 4]),
                chunk_key: chunk.chunk_key,
            };
            consider_query_surface_candidate(
                &mut best_hits,
                best_sample_score,
                surface,
                query.radius,
            );
        }
    }

    if let Some(primary_hit) = best_hits[0] {
        if query_surface_priority(primary_hit.surface) != 0 {
            return XrDepthMeshQueryResult::Miss {
                key: query.key,
                version,
                mesh_generation: volume.mesh_generation,
            };
        }
        let primary_resolved =
            build_query_surface_result(volume, primary_hit.surface, query.radius);
        let primary_surface = primary_resolved.surface;
        XrDepthMeshQueryResult::Hit(XrDepthMeshQueryHit {
            key: query.key,
            version,
            mesh_generation: volume.mesh_generation,
            distance: primary_surface.distance,
            point: primary_surface.point,
            normal: primary_surface.normal,
            from_planar_patch: primary_surface.from_planar_patch,
            triangle: primary_surface.triangle,
            patch: primary_surface.patch,
            chunk_key: primary_surface.chunk_key,
            collider: primary_resolved.collider,
            additional_hits: Vec::new(),
        })
    } else {
        XrDepthMeshQueryResult::Miss {
            key: query.key,
            version,
            mesh_generation: volume.mesh_generation,
        }
    }
}

fn consider_query_surface_candidate(
    best_hits: &mut [Option<ScoredQuerySurfaceHit>; DEPTH_QUERY_MAX_SURFACES_PER_QUERY],
    score: f32,
    surface: XrDepthMeshQuerySurfaceHit,
    query_radius: f32,
) {
    let candidate = ScoredQuerySurfaceHit { score, surface };

    for index in 0..best_hits.len() {
        if let Some(current) = best_hits[index] {
            let distinct_radius =
                query_surface_distinct_radius(query_radius, &current.surface, &candidate.surface);
            if !query_surface_hits_are_distinct(
                &current.surface,
                &candidate.surface,
                distinct_radius,
            ) {
                if scored_query_surface_is_better(&candidate, &current) {
                    remove_best_query_surface(best_hits, index);
                } else {
                    return;
                }
                break;
            }
        }
    }

    let mut insert_at = best_hits.len();
    for index in 0..best_hits.len() {
        match best_hits[index] {
            Some(current) => {
                if scored_query_surface_is_better(&candidate, &current) {
                    insert_at = index;
                    break;
                }
            }
            None => {
                insert_at = index;
                break;
            }
        }
    }

    if insert_at >= best_hits.len() {
        return;
    }

    for index in (insert_at + 1..best_hits.len()).rev() {
        best_hits[index] = best_hits[index - 1];
    }
    best_hits[insert_at] = Some(candidate);
}

fn query_surface_distinct_radius(
    query_radius: f32,
    a: &XrDepthMeshQuerySurfaceHit,
    b: &XrDepthMeshQuerySurfaceHit,
) -> f32 {
    let both_support = query_surface_priority(*a) == 0 && query_surface_priority(*b) == 0;
    if both_support {
        (query_radius * DEPTH_QUERY_SUPPORT_DISTINCT_RADIUS_SCALE)
            .max(DEPTH_QUERY_SUPPORT_DISTINCT_RADIUS_MIN)
    } else {
        (query_radius * DEPTH_QUERY_DISTINCT_RADIUS_SCALE).max(DEPTH_QUERY_DISTINCT_RADIUS_MIN)
    }
}

fn remove_best_query_surface(
    best_hits: &mut [Option<ScoredQuerySurfaceHit>; DEPTH_QUERY_MAX_SURFACES_PER_QUERY],
    index: usize,
) {
    for slot in index..best_hits.len().saturating_sub(1) {
        best_hits[slot] = best_hits[slot + 1];
    }
    if let Some(last) = best_hits.last_mut() {
        *last = None;
    }
}

fn scored_query_surface_is_better(
    candidate: &ScoredQuerySurfaceHit,
    current: &ScoredQuerySurfaceHit,
) -> bool {
    query_surface_priority(candidate.surface) < query_surface_priority(current.surface)
        || (query_surface_priority(candidate.surface) == query_surface_priority(current.surface)
            && candidate.score < current.score)
}

fn query_surface_priority(surface: XrDepthMeshQuerySurfaceHit) -> u8 {
    if surface.normal.y >= DEPTH_QUERY_SUPPORT_NORMAL_Y_MIN {
        0
    } else if surface.normal.y.abs() <= DEPTH_QUERY_LATERAL_NORMAL_Y_MAX {
        1
    } else {
        2
    }
}

fn query_surface_geometry_fingerprint(surface: &XrDepthMeshQuerySurfaceHit) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    surface.from_planar_patch.hash(&mut hasher);
    if surface.from_planar_patch {
        let mut vertices = surface.patch.map(|vertex| {
            [
                quantize_f32(vertex.x, 0.01),
                quantize_f32(vertex.y, 0.01),
                quantize_f32(vertex.z, 0.01),
            ]
        });
        vertices.sort_unstable();
        vertices.hash(&mut hasher);
    } else {
        let mut vertices = surface.triangle.map(|vertex| {
            [
                quantize_f32(vertex.x, 0.01),
                quantize_f32(vertex.y, 0.01),
                quantize_f32(vertex.z, 0.01),
            ]
        });
        vertices.sort_unstable();
        vertices.hash(&mut hasher);
    }
    hasher.finish()
}

fn query_surface_same_geometry(
    a: &XrDepthMeshQuerySurfaceHit,
    b: &XrDepthMeshQuerySurfaceHit,
) -> bool {
    query_surface_geometry_fingerprint(a) == query_surface_geometry_fingerprint(b)
}

fn query_surface_hits_are_distinct(
    a: &XrDepthMeshQuerySurfaceHit,
    b: &XrDepthMeshQuerySurfaceHit,
    radius: f32,
) -> bool {
    if query_surface_same_geometry(a, b) {
        return false;
    }
    if query_surface_priority(*a) == 0 && query_surface_priority(*b) == 0 {
        return true;
    }
    if a.normal.dot(b.normal).abs() < 0.85 {
        return true;
    }
    (a.point - b.point).length() > radius.max(0.02)
}

fn update_reduced_planar_patches(volume: &mut DepthMeshVolume, mesh_changed: bool) -> bool {
    if DEPTH_ENABLE_REDUCED_PLANAR_PATCHES {
        if mesh_changed {
            rebuild_reduced_planar_patches(volume)
        } else {
            false
        }
    } else if !volume.plane_patches.is_empty() {
        volume.plane_patches.clear();
        volume.plane_generation = volume.plane_generation.saturating_add(1);
        true
    } else {
        false
    }
}

fn closest_point_on_plane_patch(point: Vec3f, patch: &XrDepthPlanePatch) -> Vec3f {
    let offset = point - patch.center;
    let u = offset
        .dot(patch.tangent)
        .clamp(-patch.half_extent_tangent, patch.half_extent_tangent);
    let v = offset
        .dot(patch.bitangent)
        .clamp(-patch.half_extent_bitangent, patch.half_extent_bitangent);
    patch.center + patch.tangent.scale(u) + patch.bitangent.scale(v)
}

fn matching_reduced_planar_patch(
    triangle: [Vec3f; 3],
    normal: Vec3f,
    patches: &[XrDepthPlanePatch],
) -> Option<&XrDepthPlanePatch> {
    let centroid = (triangle[0] + triangle[1] + triangle[2]).scale(1.0 / 3.0);
    patches.iter().find(|patch| {
        if normal.dot(patch.normal).abs() < 0.995 {
            return false;
        }
        if (centroid - patch.center).dot(patch.normal).abs() > 0.01 {
            return false;
        }
        triangle.iter().all(|vertex| {
            let offset = *vertex - patch.center;
            if offset.dot(patch.normal).abs() > 0.015 {
                return false;
            }
            let u = offset.dot(patch.tangent);
            let v = offset.dot(patch.bitangent);
            u.abs() <= patch.half_extent_tangent + 0.02
                && v.abs() <= patch.half_extent_bitangent + 0.02
        })
    })
}

fn aabb_aabb_distance_sq(a_min: Vec3f, a_max: Vec3f, b_min: Vec3f, b_max: Vec3f) -> f32 {
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

#[derive(Clone, Copy, Debug)]
struct QueryTrajectorySample {
    progress: f32,
    point: Vec3f,
    velocity: Vec3f,
}

fn query_trajectory_time_seconds(query: XrDepthMeshQuery) -> Option<f32> {
    let horizontal_displacement = vec2f(
        query.predicted_center.x - query.center.x,
        query.predicted_center.z - query.center.z,
    );
    let horizontal_velocity = vec2f(query.velocity.x, query.velocity.z);
    let horizontal_speed_sq = horizontal_velocity.x * horizontal_velocity.x
        + horizontal_velocity.y * horizontal_velocity.y;
    if horizontal_speed_sq <= 1.0e-6 {
        return None;
    }
    let dt = (horizontal_displacement.x * horizontal_velocity.x
        + horizontal_displacement.y * horizontal_velocity.y)
        / horizontal_speed_sq;
    (dt.is_finite() && dt > 1.0e-4).then_some(dt)
}

fn query_trajectory_sample(query: XrDepthMeshQuery, progress: f32) -> QueryTrajectorySample {
    let progress = progress.clamp(0.0, 1.0);
    if let Some(total_time) = query_trajectory_time_seconds(query) {
        let t = total_time * progress;
        let dy = query.predicted_center.y - query.center.y;
        let accel_y = 2.0 * (dy - query.velocity.y * total_time) / (total_time * total_time);
        return QueryTrajectorySample {
            progress,
            point: vec3f(
                query.center.x + query.velocity.x * t,
                query.center.y + query.velocity.y * t + 0.5 * accel_y * t * t,
                query.center.z + query.velocity.z * t,
            ),
            velocity: vec3f(
                query.velocity.x,
                query.velocity.y + accel_y * t,
                query.velocity.z,
            ),
        };
    }

    let travel = query.predicted_center - query.center;
    QueryTrajectorySample {
        progress,
        point: query.center + travel.scale(progress),
        velocity: if query.velocity.length() > 1.0e-4 {
            query.velocity
        } else {
            travel
        },
    }
}

fn query_trajectory_samples(
    query: XrDepthMeshQuery,
) -> [QueryTrajectorySample; DEPTH_QUERY_TRAJECTORY_SAMPLE_COUNT] {
    std::array::from_fn(|index| {
        let progress = if DEPTH_QUERY_TRAJECTORY_SAMPLE_COUNT <= 1 {
            0.0
        } else {
            index as f32 / (DEPTH_QUERY_TRAJECTORY_SAMPLE_COUNT - 1) as f32
        };
        query_trajectory_sample(query, progress)
    })
}

fn query_trajectory_bounds_and_length(
    samples: &[QueryTrajectorySample; DEPTH_QUERY_TRAJECTORY_SAMPLE_COUNT],
) -> (Vec3f, Vec3f, f32) {
    let mut min = samples[0].point;
    let mut max = samples[0].point;
    let mut length = 0.0;
    for window in samples.windows(2) {
        let a = window[0].point;
        let b = window[1].point;
        min = Vec3f::min_componentwise(min, b);
        max = Vec3f::max_componentwise(max, b);
        length += (b - a).length();
    }
    (min, max, length)
}

fn point_polyline_distance_sq(
    point: Vec3f,
    samples: &[QueryTrajectorySample; DEPTH_QUERY_TRAJECTORY_SAMPLE_COUNT],
) -> f32 {
    let mut best = f32::INFINITY;
    for window in samples.windows(2) {
        best = best.min(point_segment_distance_sq(
            point,
            window[0].point,
            window[1].point,
        ));
    }
    best
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

fn simplify_surface_mesh_planar_regions(mesh: SurfaceMesh32) -> ReducedSurfaceMesh {
    if !DEPTH_ENABLE_REDUCED_PLANAR_PATCHES {
        return ReducedSurfaceMesh {
            mesh,
            planar_patches: Vec::new(),
        };
    }
    let regions = simplify_plane_regions(
        collect_classified_plane_triangles_from_surface_mesh(&mesh),
        DEPTH_VOXEL_SIZE_METERS,
    );
    if regions.is_empty() {
        return ReducedSurfaceMesh {
            mesh,
            planar_patches: Vec::new(),
        };
    }

    let mut consumed_triangles = HashSet::new();
    let mut positions = Vec::<[f32; 3]>::new();
    let mut normals = Vec::<[f32; 3]>::new();
    let mut indices = Vec::<u32>::new();
    let mut planar_patches = Vec::<XrDepthPlanePatch>::new();

    for region in &regions {
        let region_area: f32 = region.triangles.iter().map(|triangle| triangle.area).sum();
        if region_area < DEPTH_MESH_PLANAR_SIMPLIFY_MIN_AREA_METERS2 {
            continue;
        }
        if let Some(emitted) = emit_planar_region_rect_mesh(
            region,
            &mut positions,
            &mut normals,
            &mut indices,
            &mut planar_patches,
        ) {
            consumed_triangles
                .extend(covered_planar_region_triangles(region, &emitted).into_iter());
        }
    }

    if consumed_triangles.is_empty() {
        return ReducedSurfaceMesh {
            mesh,
            planar_patches: Vec::new(),
        };
    }

    for (triangle_index, triangle) in mesh.indices.chunks_exact(3).enumerate() {
        if consumed_triangles.contains(&triangle_index) {
            continue;
        }
        let vertices = [
            vec3f(
                mesh.positions[triangle[0] as usize][0],
                mesh.positions[triangle[0] as usize][1],
                mesh.positions[triangle[0] as usize][2],
            ),
            vec3f(
                mesh.positions[triangle[1] as usize][0],
                mesh.positions[triangle[1] as usize][1],
                mesh.positions[triangle[1] as usize][2],
            ),
            vec3f(
                mesh.positions[triangle[2] as usize][0],
                mesh.positions[triangle[2] as usize][1],
                mesh.positions[triangle[2] as usize][2],
            ),
        ];
        append_surface_mesh_triangle(&mut positions, &mut normals, &mut indices, vertices);
    }

    ReducedSurfaceMesh {
        mesh: SurfaceMesh32 {
            positions,
            normals,
            indices,
        },
        planar_patches,
    }
}

fn emit_planar_region_rect_mesh(
    region: &SimplifiedPlaneRegion,
    positions: &mut Vec<[f32; 3]>,
    normals: &mut Vec<[f32; 3]>,
    indices: &mut Vec<u32>,
    planar_patches: &mut Vec<XrDepthPlanePatch>,
) -> Option<EmittedPlanarRegion> {
    let region_indices: Vec<usize> = (0..region.triangles.len()).collect();
    let Some(patch) = fit_plane_patch_from_region(region.group, &region.triangles, &region_indices)
    else {
        return None;
    };

    let mut support_mask = PlaneSupportMask::default();
    let support_triangles = region
        .triangles
        .iter()
        .map(|triangle| triangle.vertices)
        .collect::<Vec<_>>();
    rasterize_support_triangles_into_mask(
        &mut support_mask,
        &support_triangles,
        patch.tangent,
        patch.bitangent,
    );

    let plane_distance = patch.center.dot(patch.normal);
    let mut rects = Vec::new();
    let mut emitted_rects = 0usize;
    for mut component in decompose_support_mask_components(&support_mask) {
        loop {
            if emitted_rects >= DEPTH_MESH_PLANAR_SIMPLIFY_MAX_RECTS_PER_REGION {
                break;
            }
            let Some((min_u, max_u, min_v, max_v)) = largest_supported_rectangle(&component) else {
                break;
            };
            let width = (max_u - min_u + 1) as f32 * DEPTH_PLANE_SUPPORT_CELL_METERS;
            let height = (max_v - min_v + 1) as f32 * DEPTH_PLANE_SUPPORT_CELL_METERS;
            if width * height < DEPTH_MESH_PLANAR_SIMPLIFY_MIN_RECT_AREA_METERS2 {
                break;
            }
            append_surface_mesh_quad_rect(
                positions,
                normals,
                indices,
                planar_patches,
                patch.normal,
                patch.tangent,
                patch.bitangent,
                plane_distance,
                min_u,
                max_u,
                min_v,
                max_v,
            );
            rects.push(PlanarRectCoverage {
                min_u: min_u as f32 * DEPTH_PLANE_SUPPORT_CELL_METERS,
                max_u: (max_u + 1) as f32 * DEPTH_PLANE_SUPPORT_CELL_METERS,
                min_v: min_v as f32 * DEPTH_PLANE_SUPPORT_CELL_METERS,
                max_v: (max_v + 1) as f32 * DEPTH_PLANE_SUPPORT_CELL_METERS,
            });
            emitted_rects += 1;
            remove_rect_from_support_component(&mut component, min_u, max_u, min_v, max_v);
            if component.cells.is_empty() {
                break;
            }
        }
    }

    (!rects.is_empty()).then_some(EmittedPlanarRegion {
        tangent: patch.tangent,
        bitangent: patch.bitangent,
        rects,
    })
}

fn covered_planar_region_triangles(
    region: &SimplifiedPlaneRegion,
    emitted: &EmittedPlanarRegion,
) -> HashSet<usize> {
    let mut covered = HashSet::new();
    for (index, triangle) in region.triangles.iter().enumerate() {
        let centroid_u = triangle.centroid.dot(emitted.tangent);
        let centroid_v = triangle.centroid.dot(emitted.bitangent);
        if emitted
            .rects
            .iter()
            .any(|rect| planar_rect_contains_uv(*rect, centroid_u, centroid_v))
        {
            covered.insert(region.source_triangle_indices[index]);
        }
    }
    covered
}

fn planar_rect_contains_uv(rect: PlanarRectCoverage, u: f32, v: f32) -> bool {
    let epsilon = DEPTH_PLANE_SUPPORT_CELL_METERS * 0.25;
    u >= rect.min_u - epsilon
        && u <= rect.max_u + epsilon
        && v >= rect.min_v - epsilon
        && v <= rect.max_v + epsilon
}

fn decompose_support_mask_components(mask: &PlaneSupportMask) -> Vec<PlaneSupportComponent> {
    let occupied = mask
        .cells
        .iter()
        .filter_map(|(&key, &weight)| {
            (weight >= DEPTH_PLANE_SUPPORT_OCCUPIED_WEIGHT).then_some(key)
        })
        .collect::<HashSet<_>>();
    if occupied.is_empty() {
        return Vec::new();
    }

    let mut visited = HashSet::new();
    let mut components = Vec::new();
    for &start in &occupied {
        if !visited.insert(start) {
            continue;
        }
        let mut queue = VecDeque::from([start]);
        let mut cells = Vec::new();
        while let Some(cell) = queue.pop_front() {
            cells.push(cell);
            for (du, dv) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                let neighbor = PlaneSupportCellKey {
                    u: cell.u + du,
                    v: cell.v + dv,
                };
                if occupied.contains(&neighbor) && visited.insert(neighbor) {
                    queue.push_back(neighbor);
                }
            }
        }
        if let Some(component) = rebuild_support_component_from_cells(cells) {
            components.push(component);
        }
    }
    components
}

fn rebuild_support_component_from_cells(
    cells: Vec<PlaneSupportCellKey>,
) -> Option<PlaneSupportComponent> {
    let first = *cells.first()?;
    let mut component = PlaneSupportComponent {
        cells,
        min_u: first.u,
        max_u: first.u,
        min_v: first.v,
        max_v: first.v,
    };
    for cell in &component.cells {
        component.min_u = component.min_u.min(cell.u);
        component.max_u = component.max_u.max(cell.u);
        component.min_v = component.min_v.min(cell.v);
        component.max_v = component.max_v.max(cell.v);
    }
    Some(component)
}

fn remove_rect_from_support_component(
    component: &mut PlaneSupportComponent,
    min_u: i32,
    max_u: i32,
    min_v: i32,
    max_v: i32,
) {
    let remaining = component
        .cells
        .iter()
        .copied()
        .filter(|cell| cell.u < min_u || cell.u > max_u || cell.v < min_v || cell.v > max_v)
        .collect::<Vec<_>>();
    if let Some(rebuilt) = rebuild_support_component_from_cells(remaining) {
        *component = rebuilt;
    } else {
        component.cells.clear();
    }
}

fn append_surface_mesh_quad_rect(
    positions: &mut Vec<[f32; 3]>,
    normals: &mut Vec<[f32; 3]>,
    indices: &mut Vec<u32>,
    planar_patches: &mut Vec<XrDepthPlanePatch>,
    normal: Vec3f,
    tangent: Vec3f,
    bitangent: Vec3f,
    plane_distance: f32,
    min_u_cell: i32,
    max_u_cell: i32,
    min_v_cell: i32,
    max_v_cell: i32,
) {
    let min_u = min_u_cell as f32 * DEPTH_PLANE_SUPPORT_CELL_METERS;
    let max_u = (max_u_cell + 1) as f32 * DEPTH_PLANE_SUPPORT_CELL_METERS;
    let min_v = min_v_cell as f32 * DEPTH_PLANE_SUPPORT_CELL_METERS;
    let max_v = (max_v_cell + 1) as f32 * DEPTH_PLANE_SUPPORT_CELL_METERS;
    let center_u = (min_u + max_u) * 0.5;
    let center_v = (min_v + max_v) * 0.5;
    let center = normal.scale(plane_distance) + tangent.scale(center_u) + bitangent.scale(center_v);
    let quad = [
        center + tangent.scale(min_u - center_u) + bitangent.scale(min_v - center_v),
        center + tangent.scale(max_u - center_u) + bitangent.scale(min_v - center_v),
        center + tangent.scale(max_u - center_u) + bitangent.scale(max_v - center_v),
        center + tangent.scale(min_u - center_u) + bitangent.scale(max_v - center_v),
    ];
    append_surface_mesh_quad(positions, normals, indices, quad, normal);
    planar_patches.push(XrDepthPlanePatch {
        generation: 0,
        kind: classify_planar_patch_kind_from_normal(normal),
        center,
        normal,
        tangent,
        bitangent,
        half_extent_tangent: (max_u - min_u) * 0.5,
        half_extent_bitangent: (max_v - min_v) * 0.5,
        area: (max_u - min_u) * (max_v - min_v),
        support_triangles: 2,
    });
}

fn classify_planar_patch_kind_from_normal(normal: Vec3f) -> XrDepthPlaneKind {
    if normal.y >= DEPTH_PLANE_HORIZONTAL_NORMAL_Y_MIN {
        XrDepthPlaneKind::Unknown
    } else if normal.y <= -DEPTH_PLANE_HORIZONTAL_NORMAL_Y_MIN {
        XrDepthPlaneKind::Ceiling
    } else if normal.y.abs() <= DEPTH_PLANE_VERTICAL_NORMAL_Y_MAX {
        XrDepthPlaneKind::Wall
    } else {
        XrDepthPlaneKind::Unknown
    }
}

fn append_surface_mesh_quad(
    positions: &mut Vec<[f32; 3]>,
    normals: &mut Vec<[f32; 3]>,
    indices: &mut Vec<u32>,
    quad: [Vec3f; 4],
    normal: Vec3f,
) {
    let base = positions.len() as u32;
    for vertex in quad {
        positions.push([vertex.x, vertex.y, vertex.z]);
        normals.push([normal.x, normal.y, normal.z]);
    }
    indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
}

fn append_surface_mesh_triangle(
    positions: &mut Vec<[f32; 3]>,
    normals: &mut Vec<[f32; 3]>,
    indices: &mut Vec<u32>,
    triangle: [Vec3f; 3],
) {
    let base = positions.len() as u32;
    let normal = Vec3f::cross(triangle[1] - triangle[0], triangle[2] - triangle[0]).normalize();
    for vertex in triangle {
        positions.push([vertex.x, vertex.y, vertex.z]);
        normals.push([normal.x, normal.y, normal.z]);
    }
    indices.extend_from_slice(&[base, base + 1, base + 2]);
}

fn depth_mesh_chunk_from_surface_mesh(
    chunk_key: ChunkKey,
    generation: u64,
    mesh: SurfaceMesh32,
) -> Option<XrDepthMeshChunk> {
    let reduced = simplify_surface_mesh_planar_regions(mesh);
    let mesh = reduced.mesh;
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
        planar_patches: reduced.planar_patches,
    })
}

fn voxel_coord_from_chunk_key(key: ChunkKey) -> VoxelCoord {
    VoxelCoord::new(key.x, key.y, key.z)
}

fn push_unique_chunk_key(keys: &mut Vec<ChunkKey>, key: ChunkKey) {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn quad_vertices(
        center: Vec3f,
        axis_u: Vec3f,
        axis_v: Vec3f,
        half_u: f32,
        half_v: f32,
    ) -> [Vec3f; 4] {
        let du = axis_u.normalize().scale(half_u);
        let dv = axis_v.normalize().scale(half_v);
        [
            center - du - dv,
            center + du - dv,
            center + du + dv,
            center - du + dv,
        ]
    }

    fn push_quad(
        vertices: &mut Vec<Vec3f>,
        normals: &mut Vec<Vec3f>,
        indices: &mut Vec<u32>,
        quad: [Vec3f; 4],
    ) {
        let base = vertices.len() as u32;
        let normal = Vec3f::cross(quad[1] - quad[0], quad[2] - quad[0]).normalize();
        vertices.extend_from_slice(&quad);
        normals.extend_from_slice(&[normal; 4]);
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    fn make_triangle_chunk(triangle: [Vec3f; 3]) -> XrDepthMeshChunk {
        make_triangle_chunk_with_key(ChunkKey::new(0, 0, 0), &[triangle], 1)
    }

    fn make_triangle_chunk_with_key(
        chunk_key: ChunkKey,
        triangles: &[[Vec3f; 3]],
        fingerprint: u64,
    ) -> XrDepthMeshChunk {
        let mut vertices = Vec::with_capacity(triangles.len() * 3);
        let mut normals = Vec::with_capacity(triangles.len() * 3);
        let mut indices = Vec::with_capacity(triangles.len() * 3);
        let mut bounds_min = triangles[0][0];
        let mut bounds_max = triangles[0][0];
        for triangle in triangles {
            let normal =
                Vec3f::cross(triangle[1] - triangle[0], triangle[2] - triangle[0]).normalize();
            let base = vertices.len() as u32;
            vertices.extend_from_slice(triangle);
            normals.extend_from_slice(&[normal; 3]);
            indices.extend_from_slice(&[base, base + 1, base + 2]);
            for vertex in triangle {
                bounds_min = Vec3f::min_componentwise(bounds_min, *vertex);
                bounds_max = Vec3f::max_componentwise(bounds_max, *vertex);
            }
        }

        XrDepthMeshChunk {
            generation: 1,
            chunk_key,
            fingerprint,
            bounds_min,
            bounds_max,
            vertices,
            normals,
            indices,
            planar_patches: Vec::new(),
        }
    }

    fn make_bulged_plane_chunk_with_key(
        chunk_key: ChunkKey,
        grid: usize,
        size: f32,
        bulge: f32,
        fingerprint: u64,
    ) -> XrDepthMeshChunk {
        let grid = grid.max(2);
        let step = size / (grid.saturating_sub(1) as f32);
        let half = size * 0.5;
        let mut triangles = Vec::with_capacity((grid - 1) * (grid - 1) * 2);
        let mut points = vec![vec3f(0.0, 0.0, 0.0); grid * grid];

        for z in 0..grid {
            for x in 0..grid {
                let px = -half + x as f32 * step;
                let pz = -half + z as f32 * step;
                let radial = ((px / half).powi(2) + (pz / half).powi(2)).clamp(0.0, 1.0);
                let py = bulge * (1.0 - radial);
                points[z * grid + x] = vec3f(px, py, pz);
            }
        }

        for z in 0..grid - 1 {
            for x in 0..grid - 1 {
                let p00 = points[z * grid + x];
                let p10 = points[z * grid + x + 1];
                let p01 = points[(z + 1) * grid + x];
                let p11 = points[(z + 1) * grid + x + 1];
                triangles.push([p00, p10, p01]);
                triangles.push([p10, p11, p01]);
            }
        }

        make_triangle_chunk_with_key(chunk_key, &triangles, fingerprint)
    }

    fn make_surface_mesh(quads: &[[Vec3f; 4]]) -> SurfaceMesh32 {
        let mut positions = Vec::new();
        let mut normals = Vec::new();
        let mut indices = Vec::new();
        for quad in quads {
            let base = positions.len() as u32;
            let normal = Vec3f::cross(quad[1] - quad[0], quad[2] - quad[0]).normalize();
            for vertex in quad {
                positions.push([vertex.x, vertex.y, vertex.z]);
                normals.push([normal.x, normal.y, normal.z]);
            }
            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
        SurfaceMesh32 {
            positions,
            normals,
            indices,
        }
    }

    fn plane_dense_field(size: VoxelCoord, y_plane: f32) -> Vec<f32> {
        let mut dense = vec![0.0; (size.x * size.y * size.z) as usize];
        for z in 0..size.z {
            for y in 0..size.y {
                for x in 0..size.x {
                    let coord = VoxelCoord::new(x, y, z);
                    dense[flatten_coord(coord, size)] = y as f32 - y_plane;
                }
            }
        }
        dense
    }

    fn fill_volume_signed_distance_field(
        volume: &mut DepthMeshVolume,
        min_coord: VoxelCoord,
        max_coord: VoxelCoord,
        signed_distance: impl Fn(Vec3f) -> f32,
    ) {
        let tsd_distance_meters = depth_tsd_distance_meters(volume.voxel_size_meters);
        for z in min_coord.z..=max_coord.z {
            for y in min_coord.y..=max_coord.y {
                for x in min_coord.x..=max_coord.x {
                    let coord = VoxelCoord::new(x, y, z);
                    let world = volume.mesh_grid.voxel_center_world(coord);
                    let normalized =
                        (signed_distance(world) / tsd_distance_meters).clamp(-1.0, 1.0);
                    volume.mesh_grid.overwrite_normalized_distance(
                        coord,
                        normalized,
                        volume.generation.max(1),
                    );
                }
            }
        }
        volume.update_bounds();
    }

    fn room_signed_distance(
        point: Vec3f,
        half_width_x: f32,
        half_width_z: f32,
        floor_y: f32,
        ceiling_y: f32,
    ) -> f32 {
        (point.y - floor_y)
            .min(ceiling_y - point.y)
            .min(point.x + half_width_x)
            .min(half_width_x - point.x)
            .min(point.z + half_width_z)
            .min(half_width_z - point.z)
    }

    fn scan_full_tsdf_plane_set(volume: &mut DepthMeshVolume) {
        enqueue_all_plane_scan_chunks(volume);
        let mut guard = 0usize;
        while !volume.pending_plane_scan_chunk_queue.is_empty() {
            process_incremental_tsdf_plane_scan(volume, 32);
            guard += 1;
            assert!(
                guard <= 1024,
                "plane scan did not drain pending queue, pending={}",
                volume.pending_plane_scan_chunk_queue.len()
            );
        }
    }

    fn describe_plane_patches(patches: &[XrDepthPlanePatch]) -> String {
        patches
            .iter()
            .map(|patch| {
                format!(
                    "{:?} center=({:.2},{:.2},{:.2}) normal=({:.2},{:.2},{:.2}) ext=({:.2},{:.2}) area={:.2}",
                    patch.kind,
                    patch.center.x,
                    patch.center.y,
                    patch.center.z,
                    patch.normal.x,
                    patch.normal.y,
                    patch.normal.z,
                    patch.half_extent_tangent * 2.0,
                    patch.half_extent_bitangent * 2.0,
                    patch.area,
                )
            })
            .collect::<Vec<_>>()
            .join(" | ")
    }

    fn make_wall_plane_patch(
        center: Vec3f,
        normal: Vec3f,
        half_height: f32,
        half_width: f32,
    ) -> XrDepthPlanePatch {
        let normal = vec3f(normal.x, 0.0, normal.z).normalize();
        let tangent = vec3f(0.0, 1.0, 0.0);
        let bitangent = Vec3f::cross(normal, tangent).normalize();
        XrDepthPlanePatch {
            generation: 1,
            kind: XrDepthPlaneKind::Wall,
            center,
            normal,
            tangent,
            bitangent,
            half_extent_tangent: half_height,
            half_extent_bitangent: half_width,
            area: (half_height * 2.0) * (half_width * 2.0),
            support_triangles: 8,
        }
    }

    fn bulged_plane_height(world_x: f32, world_z: f32, radius: f32, bulge: f32) -> f32 {
        let radial = ((world_x / radius).powi(2) + (world_z / radius).powi(2)).clamp(0.0, 1.0);
        bulge * (1.0 - radial)
    }

    fn box_signed_distance(point: Vec3f, center: Vec3f, half_extents: Vec3f) -> f32 {
        let local = point - center;
        let q = vec3f(
            local.x.abs() - half_extents.x,
            local.y.abs() - half_extents.y,
            local.z.abs() - half_extents.z,
        );
        let outside = vec3f(q.x.max(0.0), q.y.max(0.0), q.z.max(0.0)).length();
        let inside = q.x.max(q.y.max(q.z)).min(0.0);
        outside + inside
    }

    #[test]
    fn planar_surface_mesh_reduction_collapses_bumpy_wall() {
        if !DEPTH_ENABLE_REDUCED_PLANAR_PATCHES {
            return;
        }
        let mut quads = Vec::new();
        for y in 0..3 {
            for z in 0..3 {
                let y0 = y as f32 * 0.6;
                let y1 = y0 + 0.6;
                let z0 = z as f32 * 0.6;
                let z1 = z0 + 0.6;
                let x00 = 2.0 + if (y + z) % 2 == 0 { 0.03 } else { -0.02 };
                let x10 = 2.0 + if (y + z + 1) % 2 == 0 { 0.03 } else { -0.02 };
                let x11 = 2.0 + if (y + z + 2) % 2 == 0 { 0.03 } else { -0.02 };
                let x01 = 2.0 + if (y + z + 3) % 2 == 0 { 0.03 } else { -0.02 };
                quads.push([
                    vec3f(x00, y0, z0),
                    vec3f(x10, y0, z1),
                    vec3f(x11, y1, z1),
                    vec3f(x01, y1, z0),
                ]);
            }
        }
        let raw = make_surface_mesh(&quads);
        let simplified = simplify_surface_mesh_planar_regions(raw.clone());
        assert!(
            simplified.mesh.indices.len() < raw.indices.len(),
            "expected fewer triangles after planar reduction: raw={} simplified={}",
            raw.indices.len() / 3,
            simplified.mesh.indices.len() / 3
        );
        assert!(
            simplified.mesh.indices.len() / 3 <= 4,
            "bumpy wall should collapse close to one quad, got {} triangles",
            simplified.mesh.indices.len() / 3
        );
    }

    #[test]
    fn planar_surface_mesh_reduction_keeps_doorway_gap() {
        if !DEPTH_ENABLE_REDUCED_PLANAR_PATCHES {
            return;
        }
        let mut quads = Vec::new();
        for y in 0..4 {
            let y0 = y as f32 * 0.5;
            let y1 = y0 + 0.5;
            quads.push([
                vec3f(0.0, y0, 0.0),
                vec3f(0.0, y0, 0.8),
                vec3f(0.0, y1, 0.8),
                vec3f(0.0, y1, 0.0),
            ]);
            quads.push([
                vec3f(0.0, y0, 1.6),
                vec3f(0.0, y0, 2.4),
                vec3f(0.0, y1, 2.4),
                vec3f(0.0, y1, 1.6),
            ]);
        }
        for z in 0..2 {
            let z0 = 0.8 + z as f32 * 0.4;
            let z1 = z0 + 0.4;
            quads.push([
                vec3f(0.0, 2.0, z0),
                vec3f(0.0, 2.0, z1),
                vec3f(0.0, 2.4, z1),
                vec3f(0.0, 2.4, z0),
            ]);
        }
        let raw = make_surface_mesh(&quads);
        let simplified = simplify_surface_mesh_planar_regions(raw.clone());
        let simplified_triangles = simplified.mesh.indices.len() / 3;
        assert!(
            simplified_triangles < raw.indices.len() / 3,
            "expected doorway wall to simplify: raw={} simplified={}",
            raw.indices.len() / 3,
            simplified_triangles
        );
        assert!(
            simplified_triangles >= 6,
            "doorway should stay split into multiple wall quads, got {} triangles",
            simplified_triangles
        );
    }

    #[test]
    fn planar_surface_mesh_reduction_only_consumes_rect_covered_triangles() {
        if !DEPTH_ENABLE_REDUCED_PLANAR_PATCHES {
            return;
        }
        let region = SimplifiedPlaneRegion {
            group: ExtractedPlaneGroup::Vertical,
            source_triangle_indices: vec![11, 12],
            triangles: vec![
                ExtractedPlaneTriangle {
                    source_triangle_index: 11,
                    group: ExtractedPlaneGroup::Vertical,
                    area: 0.25,
                    normal: vec3f(1.0, 0.0, 0.0),
                    centroid: vec3f(0.0, 0.25, 0.25),
                    vertices: [
                        vec3f(0.0, 0.0, 0.0),
                        vec3f(0.0, 0.5, 0.0),
                        vec3f(0.0, 0.25, 0.5),
                    ],
                },
                ExtractedPlaneTriangle {
                    source_triangle_index: 12,
                    group: ExtractedPlaneGroup::Vertical,
                    area: 0.25,
                    normal: vec3f(1.0, 0.0, 0.0),
                    centroid: vec3f(0.0, 0.25, 1.25),
                    vertices: [
                        vec3f(0.0, 0.0, 1.0),
                        vec3f(0.0, 0.5, 1.0),
                        vec3f(0.0, 0.25, 1.5),
                    ],
                },
            ],
        };
        let emitted = EmittedPlanarRegion {
            tangent: vec3f(0.0, 1.0, 0.0),
            bitangent: vec3f(0.0, 0.0, 1.0),
            rects: vec![PlanarRectCoverage {
                min_u: 0.0,
                max_u: 0.6,
                min_v: 0.0,
                max_v: 0.6,
            }],
        };

        let covered = covered_planar_region_triangles(&region, &emitted);
        assert!(covered.contains(&11), "covered triangle should be consumed");
        assert!(
            !covered.contains(&12),
            "triangle outside emitted rect coverage must stay as raw mesh"
        );
    }

    #[test]
    fn dense_hole_repair_fills_isolated_missing_sample() {
        let size = VoxelCoord::new(6, 6, 6);
        let mut dense = plane_dense_field(size, 2.4);
        let missing = VoxelCoord::new(3, 2, 3);
        let missing_index = flatten_coord(missing, size);
        dense[missing_index] = f32::NEG_INFINITY;
        let mut scratch = Vec::new();
        repair_dense_meshing_holes(&mut dense, &mut scratch, size);
        assert!(
            dense[missing_index].is_finite(),
            "isolated dense puncture should be repaired for meshing"
        );
        assert!(
            (dense[missing_index] + 0.4).abs() < 0.11,
            "repaired sample drifted too far from local plane field: {}",
            dense[missing_index]
        );
    }

    #[test]
    fn geometry_query_skips_planar_patches_when_requested() {
        if !DEPTH_ENABLE_REDUCED_PLANAR_PATCHES {
            return;
        }
        let mut volume = DepthMeshVolume::new(1, 0.1);
        volume.mesh_generation = 7;
        volume.plane_patches.push(XrDepthPlanePatch {
            generation: 1,
            kind: XrDepthPlaneKind::Table,
            center: vec3f(0.0, 0.75, 0.0),
            normal: vec3f(0.0, 1.0, 0.0),
            tangent: vec3f(1.0, 0.0, 0.0),
            bitangent: vec3f(0.0, 0.0, 1.0),
            half_extent_tangent: 0.4,
            half_extent_bitangent: 0.3,
            area: 0.48,
            support_triangles: 2,
        });

        let planar_query = XrDepthMeshQuery {
            key: 1,
            center: vec3f(0.0, 0.83, 0.0),
            predicted_center: vec3f(0.0, 0.83, 0.0),
            velocity: vec3f(0.0, 0.0, 0.0),
            radius: 0.02,
            max_distance: 0.15,
            include_planar_patches: false,
        };
        assert!(
            matches!(
                evaluate_geometry_query(&volume, planar_query, 1),
                XrDepthMeshQueryResult::Miss { .. }
            ),
            "planar query should miss when planar patches are disabled"
        );

        let planar_hit = evaluate_geometry_query(
            &volume,
            XrDepthMeshQuery {
                include_planar_patches: true,
                ..planar_query
            },
            2,
        );
        match planar_hit {
            XrDepthMeshQueryResult::Hit(hit) => {
                assert!(hit.from_planar_patch, "expected a planar patch hit");
            }
            XrDepthMeshQueryResult::Miss { .. } => {
                panic!("expected a planar hit when planar patches are enabled");
            }
        }

        volume.mesh_chunks.push(make_triangle_chunk([
            vec3f(0.0, 0.4, 0.0),
            vec3f(0.2, 0.4, 0.0),
            vec3f(0.0, 0.4, 0.2),
        ]));
        let mesh_hit = evaluate_geometry_query(
            &volume,
            XrDepthMeshQuery {
                key: 2,
                center: vec3f(0.05, 0.5, 0.05),
                predicted_center: vec3f(0.05, 0.5, 0.05),
                velocity: vec3f(0.0, 0.0, 0.0),
                radius: 0.02,
                max_distance: 0.15,
                include_planar_patches: false,
            },
            3,
        );
        match mesh_hit {
            XrDepthMeshQueryResult::Hit(hit) => {
                assert!(!hit.from_planar_patch, "expected raw mesh fallback hit");
            }
            XrDepthMeshQueryResult::Miss { .. } => {
                panic!("expected a raw mesh hit with planar patches disabled");
            }
        }
    }

    #[test]
    fn geometry_query_preserves_exact_reduced_planar_mesh_triangle() {
        if !DEPTH_ENABLE_REDUCED_PLANAR_PATCHES {
            return;
        }
        let triangle = [
            vec3f(-0.25, 0.75, -0.20),
            vec3f(0.25, 0.75, -0.20),
            vec3f(-0.25, 0.75, 0.20),
        ];
        let mut chunk = make_triangle_chunk(triangle);
        chunk.planar_patches.push(XrDepthPlanePatch {
            generation: 1,
            kind: XrDepthPlaneKind::Table,
            center: vec3f(0.0, 0.75, 0.0),
            normal: vec3f(0.0, 1.0, 0.0),
            tangent: vec3f(1.0, 0.0, 0.0),
            bitangent: vec3f(0.0, 0.0, 1.0),
            half_extent_tangent: 0.25,
            half_extent_bitangent: 0.20,
            area: 0.20,
            support_triangles: 2,
        });

        let mut volume = DepthMeshVolume::new(1, 0.1);
        volume.mesh_generation = 9;
        volume.mesh_chunks.push(chunk);

        let result = evaluate_geometry_query(
            &volume,
            XrDepthMeshQuery {
                key: 7,
                center: vec3f(-0.05, 0.82, -0.05),
                predicted_center: vec3f(-0.05, 0.82, -0.05),
                velocity: vec3f(0.0, 0.0, 0.0),
                radius: 0.02,
                max_distance: 0.15,
                include_planar_patches: false,
            },
            1,
        );

        match result {
            XrDepthMeshQueryResult::Hit(hit) => {
                assert!(
                    hit.from_planar_patch,
                    "expected reduced planar mesh classification"
                );
                assert!(
                    hit.patch
                        .windows(2)
                        .any(|edge| (edge[1] - edge[0]).length() > 1.0e-4),
                    "expected reduced planar mesh hit to carry its full support quad"
                );
                for expected in &triangle {
                    assert!(
                        hit.triangle
                            .iter()
                            .any(|got| (*got - *expected).length() < 1.0e-4),
                        "expected exact triangle preservation: got {:?}, expected vertex {:?}",
                        hit.triangle,
                        expected
                    );
                }
            }
            XrDepthMeshQueryResult::Miss { .. } => {
                panic!("expected reduced planar mesh hit");
            }
        }
    }

    #[test]
    fn geometry_query_returns_support_halfspace_only() {
        let mut volume = DepthMeshVolume::new(1, 0.1);
        volume.mesh_generation = 10;
        volume.mesh_chunks.push(make_triangle_chunk([
            vec3f(-0.30, 0.0, -0.30),
            vec3f(0.30, 0.0, -0.30),
            vec3f(-0.30, 0.0, 0.30),
        ]));
        volume.mesh_chunks.push(make_triangle_chunk([
            vec3f(0.18, -0.20, -0.20),
            vec3f(0.18, 0.30, -0.20),
            vec3f(0.18, -0.20, 0.20),
        ]));

        let result = evaluate_geometry_query(
            &volume,
            XrDepthMeshQuery {
                key: 11,
                center: vec3f(0.05, 0.08, 0.0),
                predicted_center: vec3f(0.05, 0.08, 0.0),
                velocity: vec3f(0.0, 0.0, 0.0),
                radius: 0.12,
                max_distance: 0.20,
                include_planar_patches: false,
            },
            1,
        );

        match result {
            XrDepthMeshQueryResult::Hit(hit) => {
                assert!(
                    hit.normal.y >= DEPTH_QUERY_SUPPORT_NORMAL_Y_MIN,
                    "expected primary hit to be the support surface, got normal {:?}",
                    hit.normal
                );
                assert!(
                    hit.additional_hits.is_empty(),
                    "expected one support result only, got {:?}",
                    hit.additional_hits
                );
                let XrDepthMeshQueryColliderGeometry::HalfSpace(plane) = hit.collider.geometry;
                assert!(
                    plane.normal.y >= DEPTH_QUERY_SUPPORT_NORMAL_Y_MIN,
                    "expected support half-space, got normal {:?}",
                    plane.normal
                );
            }
            XrDepthMeshQueryResult::Miss { .. } => {
                panic!("expected support and lateral geometry hits");
            }
        }
    }

    #[test]
    fn geometry_query_returns_miss_for_lateral_only_geometry() {
        let mut volume = DepthMeshVolume::new(1, 0.1);
        volume.mesh_generation = 12;
        volume.mesh_chunks.push(make_triangle_chunk([
            vec3f(0.18, -0.20, -0.20),
            vec3f(0.18, 0.30, -0.20),
            vec3f(0.18, -0.20, 0.20),
        ]));

        let result = evaluate_geometry_query(
            &volume,
            XrDepthMeshQuery {
                key: 13,
                center: vec3f(0.06, 0.08, 0.0),
                predicted_center: vec3f(0.06, 0.08, 0.0),
                velocity: vec3f(0.0, 0.0, 0.0),
                radius: 0.04,
                max_distance: 0.12,
                include_planar_patches: false,
            },
            1,
        );

        assert!(
            matches!(result, XrDepthMeshQueryResult::Miss { .. }),
            "expected lateral-only geometry to return no support plane"
        );
    }

    #[test]
    fn geometry_query_builds_connected_support_halfspace_within_chunk() {
        let mut volume = DepthMeshVolume::new(1, 0.1);
        volume.mesh_generation = 14;
        volume.mesh_chunks.push(make_triangle_chunk_with_key(
            ChunkKey::new(0, 0, 0),
            &[
                [
                    vec3f(-0.20, 0.0, -0.20),
                    vec3f(0.00, 0.0, -0.20),
                    vec3f(-0.20, 0.0, 0.20),
                ],
                [
                    vec3f(0.00, 0.0, -0.20),
                    vec3f(0.20, 0.0, -0.20),
                    vec3f(0.00, 0.0, 0.20),
                ],
            ],
            2,
        ));

        let result = evaluate_geometry_query(
            &volume,
            XrDepthMeshQuery {
                key: 15,
                center: vec3f(0.01, 0.07, -0.18),
                predicted_center: vec3f(0.01, 0.07, -0.18),
                velocity: vec3f(0.0, 0.0, 0.0),
                radius: 0.04,
                max_distance: 0.12,
                include_planar_patches: false,
            },
            1,
        );

        match result {
            XrDepthMeshQueryResult::Hit(hit) => {
                let XrDepthMeshQueryColliderGeometry::HalfSpace(plane) = hit.collider.geometry;
                assert!(
                    plane.half_extent_tangent >= 0.10 || plane.half_extent_bitangent >= 0.10,
                    "expected connected support plane extents, got {:?}",
                    plane
                );
            }
            XrDepthMeshQueryResult::Miss { .. } => {
                panic!("expected support patch hit");
            }
        }
    }

    #[test]
    fn geometry_query_builds_connected_support_halfspace_across_shared_edge() {
        let mut volume = DepthMeshVolume::new(1, 0.1);
        volume.mesh_generation = 13;
        volume.mesh_chunks.push(make_triangle_chunk([
            vec3f(-0.20, 0.0, -0.20),
            vec3f(0.20, 0.0, -0.20),
            vec3f(-0.20, 0.0, 0.20),
        ]));
        volume.mesh_chunks.push(make_triangle_chunk([
            vec3f(0.20, 0.0, -0.20),
            vec3f(0.20, 0.0, 0.20),
            vec3f(-0.20, 0.0, 0.20),
        ]));

        let result = evaluate_geometry_query(
            &volume,
            XrDepthMeshQuery {
                key: 14,
                center: vec3f(0.0, 0.07, 0.0),
                predicted_center: vec3f(0.0, 0.07, 0.0),
                velocity: vec3f(0.0, 0.0, 0.0),
                radius: 0.04,
                max_distance: 0.12,
                include_planar_patches: false,
            },
            1,
        );

        match result {
            XrDepthMeshQueryResult::Hit(hit) => {
                let XrDepthMeshQueryColliderGeometry::HalfSpace(plane) = hit.collider.geometry;
                assert!(
                    plane.half_extent_tangent >= 0.12 || plane.half_extent_bitangent >= 0.12,
                    "expected support plane to remain connected across the shared edge, got {:?}",
                    plane
                );
            }
            XrDepthMeshQueryResult::Miss { .. } => {
                panic!("expected support hits across the shared edge");
            }
        }
    }

    #[test]
    fn geometry_query_stabilizes_bulged_support_plane() {
        let mut volume = DepthMeshVolume::new(1, 0.1);
        volume.mesh_generation = 15;
        volume.mesh_chunks.push(make_bulged_plane_chunk_with_key(
            ChunkKey::new(0, 0, 0),
            5,
            0.40,
            0.012,
            3,
        ));

        let query = XrDepthMeshQuery {
            key: 16,
            center: vec3f(0.0, 0.07, 0.0),
            predicted_center: vec3f(0.0, 0.07, 0.0),
            velocity: vec3f(0.0, 0.0, 0.0),
            radius: 0.05,
            max_distance: 0.12,
            include_planar_patches: false,
        };

        let result = evaluate_geometry_query(&volume, query, 1);
        match result {
            XrDepthMeshQueryResult::Hit(hit) => {
                let XrDepthMeshQueryColliderGeometry::HalfSpace(plane) = hit.collider.geometry;
                assert!(
                    plane.normal.y >= 0.985,
                    "expected nearly horizontal support plane on shallow bulge, got {:?}",
                    plane.normal
                );

                let support_surface = XrDepthMeshQuerySurfaceHit {
                    distance: hit.distance,
                    point: hit.point,
                    normal: hit.normal,
                    from_planar_patch: hit.from_planar_patch,
                    triangle: hit.triangle,
                    patch: hit.patch,
                    chunk_key: hit.chunk_key,
                };
                let mut max_outside = f32::NEG_INFINITY;
                visit_support_plane_triangles(
                    &volume,
                    support_surface,
                    query.radius,
                    |triangle, _normal, _area, _centroid| {
                        for vertex in triangle {
                            max_outside = max_outside.max(plane.normal.dot(vertex - plane.point));
                        }
                    },
                );
                assert!(
                    max_outside <= 0.003,
                    "support plane should stay on the local support envelope, got outside distance {}",
                    max_outside
                );
            }
            XrDepthMeshQueryResult::Miss { .. } => {
                panic!("expected support hit on bulged synthetic plane");
            }
        }
    }

    #[test]
    fn geometry_query_from_depth_grid_returns_support_halfspace() {
        let mut volume = DepthMeshVolume::new(1, 0.1);
        volume.generation = 21;
        fill_volume_signed_distance_field(
            &mut volume,
            VoxelCoord::new(-4, -4, -4),
            VoxelCoord::new(4, 4, 4),
            |world| world.y,
        );

        let result = evaluate_geometry_query(
            &volume,
            XrDepthMeshQuery {
                key: 101,
                center: vec3f(0.0, 0.09, 0.0),
                predicted_center: vec3f(0.0, 0.07, 0.0),
                velocity: vec3f(0.0, -0.2, 0.0),
                radius: 0.05,
                max_distance: 0.12,
                include_planar_patches: false,
            },
            1,
        );

        match result {
            XrDepthMeshQueryResult::Hit(hit) => {
                let XrDepthMeshQueryColliderGeometry::HalfSpace(plane) = hit.collider.geometry;
                assert!(
                    plane.normal.y >= 0.98,
                    "expected near-horizontal support plane from TSDF, got {:?}",
                    plane.normal
                );
                assert!(
                    plane.point.y.abs() <= 0.025,
                    "expected plane close to y=0, got {:?}",
                    plane.point
                );
            }
            XrDepthMeshQueryResult::Miss { .. } => {
                panic!("expected TSDF support hit");
            }
        }
    }

    #[test]
    fn tsdf_alignment_descriptor_extracts_room_walls_and_vertical_descriptor_without_meshing() {
        let mut volume = DepthMeshVolume::new(1, 0.05);
        volume.generation = 27;
        fill_volume_signed_distance_field(
            &mut volume,
            VoxelCoord::new(-18, -6, -18),
            VoxelCoord::new(18, 28, 18),
            |world| world.y.min(0.55 - world.x),
        );
        scan_full_tsdf_plane_set(&mut volume);

        let (descriptor, debug) = build_tsdf_alignment_descriptor(&volume);
        let descriptor = descriptor.unwrap_or_else(|| {
            panic!(
                "expected TSDF-native alignment descriptor, debug={debug:?}, chunks={}",
                volume.mesh_grid.chunks.len()
            )
        });

        assert!(
            volume.mesh_chunks.is_empty(),
            "TSDF descriptor test should not depend on render-mesh chunks"
        );
        assert!(
            !volume.plane_patches.is_empty(),
            "expected plane scan to populate plane patches"
        );
        assert!(
            descriptor.samples.is_empty(),
            "raw wall patch samples should stay local and not be published in the network descriptor"
        );
        assert!(
            !descriptor.wall_features.is_empty(),
            "expected inferred room-wall features in TSDF descriptor"
        );
        assert!(
            descriptor.floor_y.abs() <= 0.001,
            "expected wall-only descriptor floor_y to stay at 0, got {}",
            descriptor.floor_y
        );
        assert!(
            descriptor
                .wall_normal_histogram
                .iter()
                .copied()
                .sum::<f32>()
                > 0.0,
            "expected non-empty wall histogram from TSDF descriptor"
        );
        assert!(
            descriptor.vertical_descriptor.is_some(),
            "expected compact TSDF vertical descriptor for symmetry resolution"
        );
        assert!(
            debug.wall_sample_count > 0,
            "expected local wall patches to still contribute to room-wall building"
        );
    }

    #[test]
    fn tsdf_plane_scan_detects_box_room_walls() {
        let mut volume = DepthMeshVolume::new(1, 0.05);
        volume.generation = 31;
        let room_half_x = 1.40;
        let room_half_z = 1.10;
        let floor_y = 0.0;
        let ceiling_y = 2.40;
        fill_volume_signed_distance_field(
            &mut volume,
            VoxelCoord::new(-34, -8, -30),
            VoxelCoord::new(34, 56, 30),
            |world| room_signed_distance(world, room_half_x, room_half_z, floor_y, ceiling_y),
        );

        scan_full_tsdf_plane_set(&mut volume);
        let patches = &volume.plane_patches;
        assert!(
            !patches.is_empty(),
            "expected room plane scan to emit patches"
        );

        let wall_patches = patches
            .iter()
            .filter(|patch| patch.kind == XrDepthPlaneKind::Wall)
            .collect::<Vec<_>>();
        assert!(
            wall_patches.len() >= 3,
            "expected several wall patches, got {} from {}",
            wall_patches.len(),
            describe_plane_patches(patches)
        );
        assert!(
            wall_patches.len() == patches.len(),
            "expected wall-only plane scan output, got {}",
            describe_plane_patches(patches)
        );
        assert!(
            wall_patches
                .iter()
                .any(|patch| (patch.center.x.abs() - room_half_x).abs() <= 0.14),
            "expected wall near x bounds, got {}",
            describe_plane_patches(patches)
        );
        assert!(
            wall_patches
                .iter()
                .any(|patch| (patch.center.z.abs() - room_half_z).abs() <= 0.14),
            "expected wall near z bounds, got {}",
            describe_plane_patches(patches)
        );
        assert!(
            patches.iter().all(|patch| {
                patch.center.x.abs() <= room_half_x + 0.25
                    && patch.center.z.abs() <= room_half_z + 0.25
                    && patch.center.y >= floor_y - 0.25
                    && patch.center.y <= ceiling_y + 0.25
            }),
            "expected all room patches inside expanded room bounds, got {}",
            describe_plane_patches(patches)
        );
    }

    #[test]
    fn wall_track_stability_retains_wall_through_brief_miss() {
        let mut tracks = Vec::new();
        let wall =
            make_wall_plane_patch(vec3f(1.20, 1.10, -0.35), vec3f(1.0, 0.0, 0.0), 0.55, 0.45);

        let first = stabilize_wall_scan_patches(&mut tracks, vec![wall.clone()]);
        assert_eq!(first.len(), 1, "new wall should appear immediately");

        let second = stabilize_wall_scan_patches(&mut tracks, vec![wall.clone()]);
        assert_eq!(second.len(), 1, "wall should remain after confirmation");

        let missing = stabilize_wall_scan_patches(&mut tracks, Vec::new());
        assert_eq!(
            missing.len(),
            1,
            "stable wall should persist across a brief missing update"
        );
        assert!(
            (missing[0].center - wall.center).length() < 0.02,
            "missing update should not move the stable wall too far: {:?}",
            missing[0]
        );
    }

    #[test]
    fn wall_track_stability_damps_plane_distance_jitter() {
        let mut tracks = Vec::new();
        let base =
            make_wall_plane_patch(vec3f(1.00, 1.00, -0.20), vec3f(1.0, 0.0, 0.0), 0.60, 0.40);
        let jittered =
            make_wall_plane_patch(vec3f(1.10, 1.02, -0.12), vec3f(1.0, 0.0, 0.0), 0.58, 0.44);

        let _ = stabilize_wall_scan_patches(&mut tracks, vec![base.clone()]);
        let stabilized = stabilize_wall_scan_patches(&mut tracks, vec![jittered.clone()]);
        assert_eq!(stabilized.len(), 1);

        let stable_distance = stabilized[0].center.dot(stabilized[0].normal);
        let base_distance = base.center.dot(base.normal);
        let jittered_distance = jittered.center.dot(jittered.normal);
        assert!(
            stable_distance > base_distance && stable_distance < jittered_distance,
            "stable wall should move toward jittered measurement without fully snapping: stable={stable_distance} base={base_distance} jittered={jittered_distance}"
        );
        assert!(
            (stable_distance - base_distance) < (jittered_distance - base_distance),
            "stable wall should damp depth jitter"
        );
    }

    #[test]
    fn room_wall_candidates_pick_dominant_plane_group_per_side() {
        let east_major_a =
            make_wall_plane_patch(vec3f(1.20, 1.00, -0.85), vec3f(1.0, 0.0, 0.0), 0.60, 0.28);
        let east_major_b =
            make_wall_plane_patch(vec3f(1.22, 1.04, 0.15), vec3f(1.0, 0.0, 0.0), 0.58, 0.34);
        let east_alcove =
            make_wall_plane_patch(vec3f(1.48, 0.98, 0.95), vec3f(1.0, 0.0, 0.0), 0.54, 0.18);
        let west =
            make_wall_plane_patch(vec3f(-1.05, 1.02, -0.10), vec3f(-1.0, 0.0, 0.0), 0.62, 0.44);
        let north =
            make_wall_plane_patch(vec3f(0.10, 1.01, 1.32), vec3f(0.0, 0.0, 1.0), 0.60, 0.42);
        let south =
            make_wall_plane_patch(vec3f(-0.18, 0.99, -0.92), vec3f(0.0, 0.0, -1.0), 0.58, 0.40);

        let candidates = build_room_wall_candidates(&[
            east_major_a,
            east_major_b,
            east_alcove,
            west,
            north,
            south,
        ]);
        let east = candidates
            .iter()
            .find(|candidate| candidate.normal.x > 0.9)
            .copied()
            .expect("expected positive-x room wall candidate");
        assert!(
            (east.plane_distance - 1.21).abs() <= 0.08,
            "dominant east wall should stay on the major wall plane, got {east:?}"
        );
        assert!(
            (east.plane_distance - 1.48).abs() > 0.14,
            "alcove plane should not become the exported room wall, got {east:?}"
        );
    }

    #[test]
    fn room_wall_features_form_box_from_stable_tracks() {
        let mut room_tracks = Vec::new();
        let patches = vec![
            make_wall_plane_patch(vec3f(1.18, 1.00, -0.82), vec3f(1.0, 0.0, 0.0), 0.62, 0.30),
            make_wall_plane_patch(vec3f(1.19, 1.02, 0.28), vec3f(1.0, 0.0, 0.0), 0.58, 0.38),
            make_wall_plane_patch(vec3f(-1.08, 1.01, -0.06), vec3f(-1.0, 0.0, 0.0), 0.60, 0.42),
            make_wall_plane_patch(vec3f(0.06, 0.99, 1.36), vec3f(0.0, 0.0, 1.0), 0.58, 0.46),
            make_wall_plane_patch(vec3f(-0.12, 1.03, -0.94), vec3f(0.0, 0.0, -1.0), 0.60, 0.44),
        ];

        for _ in 0..2 {
            stabilize_room_wall_candidates(&mut room_tracks, build_room_wall_candidates(&patches));
        }
        let features = visible_room_wall_features(&room_tracks);
        assert_eq!(
            features.len(),
            4,
            "expected four inferred room walls from the patch box, got {features:?}"
        );
        let east = features
            .iter()
            .find(|feature| feature.normal.x > 0.9)
            .expect("expected positive-x inferred wall");
        assert!(
            east.half_extent_along >= 1.00,
            "orthogonal wall intersections should expand east wall span, got {east:?}"
        );
        assert!(
            east.min_y.abs() <= 1.0e-4
                && (east.max_y - DEPTH_ROOM_WALL_HEIGHT_METERS).abs() <= 1.0e-4,
            "room walls should use fixed height for debug and matching, got {east:?}"
        );
        assert!(
            (east.center.y - DEPTH_ROOM_WALL_HEIGHT_METERS * 0.5).abs() <= 1.0e-4,
            "room wall center should sit halfway up the fixed wall height, got {east:?}"
        );
    }

    #[test]
    fn room_wall_tracks_remain_visible_when_patch_updates_pause() {
        let mut room_tracks = Vec::new();
        let patches = vec![
            make_wall_plane_patch(vec3f(1.18, 1.00, -0.82), vec3f(1.0, 0.0, 0.0), 0.62, 0.30),
            make_wall_plane_patch(vec3f(1.19, 1.02, 0.28), vec3f(1.0, 0.0, 0.0), 0.58, 0.38),
            make_wall_plane_patch(vec3f(-1.08, 1.01, -0.06), vec3f(-1.0, 0.0, 0.0), 0.60, 0.42),
            make_wall_plane_patch(vec3f(0.06, 0.99, 1.36), vec3f(0.0, 0.0, 1.0), 0.58, 0.46),
            make_wall_plane_patch(vec3f(-0.12, 1.03, -0.94), vec3f(0.0, 0.0, -1.0), 0.60, 0.44),
        ];

        for _ in 0..2 {
            stabilize_room_wall_candidates(&mut room_tracks, build_room_wall_candidates(&patches));
        }
        for _ in 0..24 {
            stabilize_room_wall_candidates(&mut room_tracks, Vec::new());
        }

        let features = visible_room_wall_features(&room_tracks);
        assert_eq!(
            features.len(),
            4,
            "room walls should persist even when fresh patch evidence is briefly missing, got {features:?}"
        );
    }

    #[test]
    fn geometry_query_from_depth_grid_returns_lateral_impact_plane() {
        let mut volume = DepthMeshVolume::new(1, 0.1);
        volume.generation = 22;
        fill_volume_signed_distance_field(
            &mut volume,
            VoxelCoord::new(-4, -4, -4),
            VoxelCoord::new(4, 4, 4),
            |world| 0.18 - world.x,
        );

        let result = evaluate_geometry_query(
            &volume,
            XrDepthMeshQuery {
                key: 102,
                center: vec3f(0.06, 0.10, 0.0),
                predicted_center: vec3f(0.16, 0.10, 0.0),
                velocity: vec3f(0.8, 0.0, 0.0),
                radius: 0.05,
                max_distance: 0.12,
                include_planar_patches: false,
            },
            1,
        );

        match result {
            XrDepthMeshQueryResult::Hit(hit) => {
                let XrDepthMeshQueryColliderGeometry::HalfSpace(plane) = hit.collider.geometry;
                assert_eq!(
                    hit.collider.role,
                    XrDepthMeshQueryColliderRole::Impact,
                    "expected TSDF wall hit to resolve as an impact plane"
                );
                assert!(
                    plane.normal.x <= -0.85 && plane.normal.y.abs() <= 0.25,
                    "expected mostly vertical wall normal, got {:?}",
                    plane.normal
                );
                assert!(
                    hit.collider.restitution >= 0.3,
                    "expected impact plane restitution, got {}",
                    hit.collider.restitution
                );
            }
            XrDepthMeshQueryResult::Miss { .. } => {
                panic!("expected TSDF wall hit to produce an impact plane");
            }
        }
    }

    #[test]
    fn geometry_query_from_depth_grid_returns_ceiling_impact_plane() {
        let mut volume = DepthMeshVolume::new(1, 0.1);
        volume.generation = 23;
        fill_volume_signed_distance_field(
            &mut volume,
            VoxelCoord::new(-4, -4, -4),
            VoxelCoord::new(4, 4, 4),
            |world| world.y - 0.18,
        );

        let result = evaluate_geometry_query(
            &volume,
            XrDepthMeshQuery {
                key: 103,
                center: vec3f(0.0, 0.06, 0.0),
                predicted_center: vec3f(0.0, 0.16, 0.0),
                velocity: vec3f(0.0, 0.9, 0.0),
                radius: 0.05,
                max_distance: 0.12,
                include_planar_patches: false,
            },
            1,
        );

        match result {
            XrDepthMeshQueryResult::Hit(hit) => {
                let XrDepthMeshQueryColliderGeometry::HalfSpace(plane) = hit.collider.geometry;
                assert_eq!(
                    hit.collider.role,
                    XrDepthMeshQueryColliderRole::Impact,
                    "expected TSDF ceiling hit to resolve as an impact plane"
                );
                assert!(
                    plane.normal.y <= -0.85,
                    "expected mostly downward ceiling normal, got {:?}",
                    plane.normal
                );
                assert!(
                    hit.collider.restitution >= 0.3,
                    "expected impact plane restitution, got {}",
                    hit.collider.restitution
                );
            }
            XrDepthMeshQueryResult::Miss { .. } => {
                panic!("expected TSDF ceiling hit to produce an impact plane");
            }
        }
    }

    #[test]
    fn geometry_query_from_depth_grid_stabilizes_bulged_support_plane() {
        let mut volume = DepthMeshVolume::new(1, 0.1);
        volume.generation = 23;
        fill_volume_signed_distance_field(
            &mut volume,
            VoxelCoord::new(-5, -4, -5),
            VoxelCoord::new(5, 4, 5),
            |world| world.y - bulged_plane_height(world.x, world.z, 0.45, 0.02),
        );

        let result = evaluate_geometry_query(
            &volume,
            XrDepthMeshQuery {
                key: 103,
                center: vec3f(0.0, 0.08, 0.0),
                predicted_center: vec3f(0.0, 0.06, 0.0),
                velocity: vec3f(0.0, -0.2, 0.0),
                radius: 0.05,
                max_distance: 0.12,
                include_planar_patches: false,
            },
            1,
        );

        match result {
            XrDepthMeshQueryResult::Hit(hit) => {
                let XrDepthMeshQueryColliderGeometry::HalfSpace(plane) = hit.collider.geometry;
                let center_height = bulged_plane_height(0.0, 0.0, 0.45, 0.02);
                assert!(
                    plane.normal.y >= 0.985,
                    "expected nearly horizontal TSDF support plane on shallow bulge, got {:?}",
                    plane.normal
                );
                assert!(
                    plane.point.y >= center_height - 0.01 && plane.point.y <= center_height + 0.03,
                    "expected plane near bulged top envelope, got {:?} expected around {}",
                    plane.point,
                    center_height
                );
            }
            XrDepthMeshQueryResult::Miss { .. } => {
                panic!("expected TSDF support hit on bulged surface");
            }
        }
    }

    #[test]
    fn geometry_query_from_depth_grid_clears_near_table_overhang() {
        let mut volume = DepthMeshVolume::new(1, 0.1);
        volume.generation = 24;
        let table_center = vec3f(0.0, -0.05, 0.0);
        let table_half_extents = vec3f(0.20, 0.05, 0.20);
        fill_volume_signed_distance_field(
            &mut volume,
            VoxelCoord::new(-5, -4, -5),
            VoxelCoord::new(5, 4, 5),
            |world| box_signed_distance(world, table_center, table_half_extents),
        );

        let inside_result = evaluate_geometry_query(
            &volume,
            XrDepthMeshQuery {
                key: 104,
                center: vec3f(0.17, 0.08, 0.0),
                predicted_center: vec3f(0.17, 0.06, 0.0),
                velocity: vec3f(0.0, -0.2, 0.0),
                radius: 0.05,
                max_distance: 0.12,
                include_planar_patches: false,
            },
            1,
        );
        assert!(
            matches!(inside_result, XrDepthMeshQueryResult::Hit(_)),
            "expected support while still inside the tabletop footprint"
        );

        let outside_result = evaluate_geometry_query(
            &volume,
            XrDepthMeshQuery {
                key: 105,
                center: vec3f(0.24, 0.08, 0.0),
                predicted_center: vec3f(0.24, 0.06, 0.0),
                velocity: vec3f(0.0, -0.2, 0.0),
                radius: 0.05,
                max_distance: 0.12,
                include_planar_patches: false,
            },
            1,
        );
        assert!(
            matches!(outside_result, XrDepthMeshQueryResult::Miss { .. }),
            "expected support to clear once the center leaves the tabletop footprint"
        );
    }

    #[test]
    fn geometry_query_from_depth_grid_tracks_grazing_ballistic_support_into_table() {
        let mut volume = DepthMeshVolume::new(1, 0.05);
        volume.generation = 26;
        let table_center = vec3f(0.10, -0.05, 0.0);
        let table_half_extents = vec3f(0.10, 0.05, 0.20);
        fill_volume_signed_distance_field(
            &mut volume,
            VoxelCoord::new(-6, -5, -6),
            VoxelCoord::new(6, 5, 6),
            |world| box_signed_distance(world, table_center, table_half_extents),
        );

        let result = evaluate_geometry_query(
            &volume,
            XrDepthMeshQuery {
                key: 106,
                center: vec3f(-0.07, 0.08, 0.0),
                predicted_center: vec3f(0.10, 0.02, 0.0),
                velocity: vec3f(1.20, 0.0, 0.0),
                radius: 0.05,
                max_distance: 0.12,
                include_planar_patches: false,
            },
            1,
        );

        match result {
            XrDepthMeshQueryResult::Hit(hit) => {
                let XrDepthMeshQueryColliderGeometry::HalfSpace(plane) = hit.collider.geometry;
                assert!(
                    plane.normal.y >= 0.90,
                    "expected grazing ballistic support to resolve to an upright tabletop plane, got {:?}",
                    plane.normal
                );
                assert!(
                    plane.point.x >= -0.02 && plane.point.x <= 0.22,
                    "expected support point to land on the future tabletop footprint, got {:?}",
                    plane.point
                );
            }
            XrDepthMeshQueryResult::Miss { .. } => {
                panic!("expected grazing ballistic path to resolve future tabletop support");
            }
        }
    }

    #[test]
    fn geometry_query_from_depth_grid_handles_small_radius_without_panicking() {
        let mut volume = DepthMeshVolume::new(1, 0.1);
        volume.generation = 25;
        fill_volume_signed_distance_field(
            &mut volume,
            VoxelCoord::new(-4, -4, -4),
            VoxelCoord::new(4, 4, 4),
            |world| world.y,
        );

        let result = evaluate_geometry_query(
            &volume,
            XrDepthMeshQuery {
                key: 106,
                center: vec3f(0.0, 0.03, 0.0),
                predicted_center: vec3f(0.0, 0.02, 0.0),
                velocity: vec3f(0.0, -0.1, 0.0),
                radius: 0.01,
                max_distance: 0.12,
                include_planar_patches: false,
            },
            1,
        );

        assert!(
            matches!(
                result,
                XrDepthMeshQueryResult::Hit(_) | XrDepthMeshQueryResult::Miss { .. }
            ),
            "small-radius TSDF query should complete without panicking"
        );
    }

    #[test]
    fn plane_patch_corner_winding_matches_patch_normal() {
        let patch = XrDepthPlanePatch {
            generation: 1,
            kind: XrDepthPlaneKind::Table,
            center: vec3f(0.0, 0.75, 0.0),
            normal: vec3f(0.0, 1.0, 0.0),
            tangent: vec3f(1.0, 0.0, 0.0),
            bitangent: vec3f(0.0, 0.0, 1.0),
            half_extent_tangent: 0.3,
            half_extent_bitangent: 0.2,
            area: 0.24,
            support_triangles: 2,
        };
        let corners = plane_patch_corners(&patch);
        let tri_normal = Vec3f::cross(corners[1] - corners[0], corners[2] - corners[0]).normalize();
        assert!(
            tri_normal.dot(patch.normal) > 0.99,
            "patch quad winding should align with patch normal: tri_normal={tri_normal:?} patch_normal={:?}",
            patch.normal
        );
    }
}
