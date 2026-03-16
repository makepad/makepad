use crate::{
    makepad_math::{vec3f, vec4f, Mat4f, Vec3f},
    os::linux::{
        openxr::CxOpenXrFrame,
        vulkan::{CxVulkan, CxVulkanOpenXrSessionData},
    },
    thread::SignalToUI,
    xr_depth_mesh::{
        empty_bounds, xr_depth_mesh_store, XrDepthMesh, XrDepthMeshChunk, XrDepthMeshStore,
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
const DEPTH_TSD_MAX_CONFIDENCE: u8 = 32;
const DEPTH_TSD_MIN_MESH_CONFIDENCE: u8 = 3;
const DEPTH_TSD_STABLE_CONFIDENCE: u8 = 8;
const DEPTH_TSD_STALE_LOW_CONFIDENCE_GENERATIONS: u64 = 12;
const DEPTH_PLAYER_CLEAR_MAX_CONFIDENCE: u8 = 2;
const DEPTH_PLAYER_EXCLUDE_RADIUS_METERS: f32 = 0.32;
const DEPTH_PLAYER_EXCLUDE_TOP_METERS: f32 = 0.12;
const DEPTH_PLAYER_EXCLUDE_BOTTOM_METERS: f32 = 1.30;
const DEPTH_MESH_UPDATE_DISTANCE_METERS: f32 = 4.0;
const DEPTH_SURFACE_MESH_CHUNKS_PER_TICK: usize = 1;
const DEPTH_SURFACE_MESH_IDLE_WAIT_MILLIS: u64 = 8;
const DEPTH_DEBUG_LOG_CHUNK_MESH_TIMING: bool = true;

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
        if self.valid[id] == 0 || self.confidence[id] < DEPTH_TSD_MIN_MESH_CONFIDENCE {
            None
        } else if self.confidence[id] < DEPTH_TSD_STABLE_CONFIDENCE
            && current_generation.saturating_sub(self.observed_generation[id])
                > DEPTH_TSD_STALE_LOW_CONFIDENCE_GENERATIONS
        {
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
    inv_depth_proj: Mat4f,
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
    frame_tsd_samples: HashMap<VoxelCoord, f32>,
    visible_world_min: Vec3f,
    visible_world_max: Vec3f,
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
                inv_depth_proj: frame.eyes[DEPTH_VOXEL_EYE_INDEX].depth_proj_mat.invert(),
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
        if applied_update || mesh_changed {
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
        frame_tsd_samples,
        visible_world_min,
        visible_world_max,
    })
}

fn apply_preprocessed_depth_mesh(job: CxOpenXrPreparedDepthMeshJob, volume: &mut DepthMeshVolume) {
    volume.generation = job.generation;
    volume.eye_index = job.eye_index;
    volume.image_width = job.width;
    volume.image_height = job.height;
    volume.sample_step = job.sample_step;

    let mut topology_changes = apply_tsd_samples(volume, job.frame_tsd_samples);
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
    let raw_depth = *job
        .depth
        .get(y as usize * job.width as usize + x as usize)? as f32
        / u16::MAX as f32;
    if !(DEPTH_VOXEL_MIN_DEPTH_VALUE..DEPTH_VOXEL_MAX_DEPTH_VALUE).contains(&raw_depth) {
        return None;
    }
    let uv_x = (x as f32 + 0.5) / job.width as f32;
    let uv_y = (y as f32 + 0.5) / job.height as f32;
    let clip = vec4f(
        uv_x * 2.0 - 1.0,
        uv_y * 2.0 - 1.0,
        raw_depth * 2.0 - 1.0,
        1.0,
    );
    let view = job.inv_depth_proj.transform_vec4(clip);
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
    let world = job.world_from_depth_view.transform_vec4(view);
    if !world.w.is_finite() || world.w.abs() < 1.0e-6 {
        return None;
    }
    let inv_w = 1.0 / world.w;
    let point = vec3f(world.x * inv_w, world.y * inv_w, world.z * inv_w);
    (point.x.is_finite() && point.y.is_finite() && point.z.is_finite()).then_some(point)
}

fn apply_tsd_samples(
    volume: &mut DepthMeshVolume,
    frame_tsd_samples: HashMap<VoxelCoord, f32>,
) -> usize {
    let mut changed = 0;
    for (coord, normalized) in frame_tsd_samples {
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
    let meshed_generations: HashMap<IVector, u64> = volume
        .mesh_chunks
        .iter()
        .map(|chunk| (chunk.chunk_key, chunk.generation))
        .collect();
    for z in (min_key.z - 1)..=(max_key.z + 1) {
        for y in (min_key.y - 1)..=(max_key.y + 1) {
            for x in (min_key.x - 1)..=(max_key.x + 1) {
                let key = IVector::new(x, y, z);
                let needs_refresh = meshed_generations
                    .get(&key)
                    .map(|generation| {
                        volume.generation.saturating_sub(*generation)
                            > DEPTH_TSD_STALE_LOW_CONFIDENCE_GENERATIONS
                    })
                    .unwrap_or(true);
                if needs_refresh && volume.pending_mesh_dirty_chunks.insert(key) {
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
