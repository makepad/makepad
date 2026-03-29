use crate::{
    empty_bounds, ChunkKey, SparseTsdGridReadSnapshot, SparseTsdReadChunk, TsdfPublishedSnapshot,
    XrDepthAlignHeightMap,
};
use makepad_math::{vec2f, vec3f, vec4f, Mat4f, Vec3f, Vec4f};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
    time::Instant,
};

pub const DEPTH_VOXEL_EYE_INDEX: usize = 0;
pub const DEPTH_VOXEL_SAMPLE_STEP: u32 = 1;
const DEPTH_TSD_CHUNK_EDGE_VOXELS: i32 = 8;
const DEPTH_TSD_CHUNK_VOLUME: usize = (DEPTH_TSD_CHUNK_EDGE_VOXELS as usize)
    * (DEPTH_TSD_CHUNK_EDGE_VOXELS as usize)
    * (DEPTH_TSD_CHUNK_EDGE_VOXELS as usize);
const DEPTH_TSD_CHUNK_VALID_WORDS: usize =
    (DEPTH_TSD_CHUNK_VOLUME + u64::BITS as usize - 1) / u64::BITS as usize;
const DEPTH_IMAGE_EDGE_MARGIN_PIXELS: usize = 32;
pub const DEPTH_VOXEL_MIN_DISTANCE_METERS: f32 = 0.08;
pub const DEPTH_VOXEL_MAX_DISTANCE_METERS: f32 = 6.0;
const DEPTH_TSD_MIN_UPDATE_DISTANCE_METERS: f32 = 0.5;
pub const DEPTH_TSD_TARGET_INTEGRATION_INTERVAL_MILLIS: u64 = 1000;
const DEPTH_TSD_SUBMIT_SAMPLE_GRID_X: usize = 6;
const DEPTH_TSD_SUBMIT_SAMPLE_GRID_Y: usize = 4;
const DEPTH_TSD_SUBMIT_SAMPLE_NDC_MARGIN: f32 = 0.82;
const DEPTH_TSD_SUBMIT_FRONTIER_HORIZON_METERS: f32 = 3.0;
const DEPTH_TSD_SUBMIT_FRONTIER_SURFACE_ABS_DISTANCE: f32 = 0.4;
const DEPTH_TSD_SUBMIT_MEDIUM_NOVELTY_SCORE: f32 = 3.0;
const DEPTH_TSD_SUBMIT_STRONG_NOVELTY_SCORE: f32 = 7.0;
const DEPTH_TSD_SUBMIT_MEDIUM_INTERVAL_MILLIS: u64 = 500;
const DEPTH_TSD_SUBMIT_STRONG_INTERVAL_MILLIS: u64 = 250;
const DEPTH_TSD_SUBMIT_VERTICAL_BOUNDS_PADDING_METERS: f32 = 1.25;
const DEPTH_TSD_NOVELTY_SAMPLE_GRID_X: usize = 16;
const DEPTH_TSD_NOVELTY_SAMPLE_GRID_Y: usize = 12;
const DEPTH_TSD_NOVELTY_BOUNDS_PADDING_VOXELS: i32 = 2;
const DEPTH_TSD_NOVELTY_UNKNOWN_WEIGHT: f32 = 1.0;
const DEPTH_TSD_NOVELTY_LOW_CONFIDENCE_WEIGHT: f32 = 0.35;
const DEPTH_TSD_NOVELTY_SURFACE_MISMATCH_WEIGHT: f32 = 0.2;
const DEPTH_TSD_NOVELTY_SURFACE_MISMATCH_ABS_DISTANCE: f32 = 0.35;
const DEPTH_TSD_NOVELTY_OUTSIDE_BOUNDS_WEIGHT: f32 = 2.0;
pub const DEPTH_VOXEL_MIN_DEPTH_VALUE: f32 = 1.0 / 65535.0;
pub const DEPTH_VOXEL_MAX_DEPTH_VALUE: f32 = 0.9995;
const DEPTH_TSD_MIN_NORMAL_DOT: f32 = 0.3;
const DEPTH_TSD_APPLY_DELTA_EPSILON: f32 = 0.01;
const DEPTH_TSD_MAX_CONFIDENCE: u8 = 32;
const DEPTH_TSD_STABLE_CONFIDENCE: u8 = 8;
const DEPTH_PLAYER_CLEAR_MAX_CONFIDENCE: u8 = 2;
const DEPTH_PLAYER_EXCLUDE_RADIUS_METERS: f32 = 0.32;
const DEPTH_PLAYER_EXCLUDE_TOP_METERS: f32 = 0.12;
const DEPTH_PLAYER_EXCLUDE_BOTTOM_METERS: f32 = 1.30;
const DEPTH_MESH_UPDATE_DISTANCE_METERS: f32 = 4.0;
pub const DEPTH_PUBLISHED_HEIGHT_MAP_INTERVAL_MILLIS: u64 = 1000;
pub const DEPTH_PROJECTED_HEIGHT_REFRESH_INTERVAL_MILLIS: u64 = 33;
const DEPTH_ALIGN_HEIGHT_MAP_BOUNDS_PADDING_METERS: f32 = 0.45;
const DEPTH_ALIGN_HEIGHT_MAP_FLOOR_BIN_METERS: f32 = 0.04;
const DEPTH_ALIGN_HEIGHT_MAP_FLOOR_MIN_SUPPORT_RATIO: f32 = 0.005;
const DEPTH_ALIGN_HEIGHT_MAP_FLOOR_MIN_SUPPORT_CELLS: usize = 24;
const DEPTH_ALIGN_HEIGHT_MAP_FLOOR_SUPPORT_WINDOW_BINS: usize = 2;
const DEPTH_ALIGN_VECTOR_SLICE_TOP_Y_METERS: f32 = 2.00;
const DEPTH_ALIGN_VECTOR_SLICE_ISO_HEIGHT_METERS: f32 = 0.50;
const DEPTH_ALIGN_VECTOR_SLICE_PLAYER_CUTOUT_RADIUS_METERS: f32 =
    DEPTH_PLAYER_EXCLUDE_RADIUS_METERS + 0.12;
const DEPTH_ALIGN_PROJECTED_HEIGHT_SAMPLES_PER_TICK: usize = 512;
const DEPTH_ALIGN_PROJECTED_HEIGHT_MAX_SAMPLES_PER_SLICE: usize =
    DEPTH_ALIGN_PROJECTED_HEIGHT_SAMPLES_PER_TICK * 8;
pub const DEPTH_ALIGN_PROJECTED_HEIGHT_MAX_SLICE_CREDITS: usize = 4;
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

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Ord, PartialOrd)]
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

#[derive(Clone, Copy, Debug, Default)]
pub struct DepthFrameNovelty {
    pub score: f32,
    pub valid_samples: usize,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SubmitDepthFrameNovelty {
    pub score: f32,
    pub valid_rays: usize,
    pub force_readback: bool,
}

fn point_inside_bounds_xz(point: Vec3f, min: Vec3f, max: Vec3f) -> bool {
    point.x >= min.x && point.x <= max.x && point.z >= min.z && point.z <= max.z
}

fn ray_aabb_exit_distance(origin: Vec3f, direction: Vec3f, min: Vec3f, max: Vec3f) -> Option<f32> {
    let mut t_min = f32::NEG_INFINITY;
    let mut t_max = f32::INFINITY;
    for (origin_axis, direction_axis, min_axis, max_axis) in [
        (origin.x, direction.x, min.x, max.x),
        (origin.y, direction.y, min.y, max.y),
        (origin.z, direction.z, min.z, max.z),
    ] {
        if direction_axis.abs() <= 1.0e-6 {
            if origin_axis < min_axis || origin_axis > max_axis {
                return None;
            }
            continue;
        }
        let inv_direction = 1.0 / direction_axis;
        let t0 = (min_axis - origin_axis) * inv_direction;
        let t1 = (max_axis - origin_axis) * inv_direction;
        t_min = t_min.max(t0.min(t1));
        t_max = t_max.min(t0.max(t1));
        if t_max < t_min {
            return None;
        }
    }
    (t_max.is_finite() && t_max >= 0.0).then_some(t_max.max(0.0))
}

fn depth_ndc_to_view_with_inv_proj(
    inv_depth_proj: Mat4f,
    ndc_x: f32,
    ndc_y: f32,
    ndc_z: f32,
) -> Option<Vec3f> {
    let view = inv_depth_proj.transform_vec4(vec4f(ndc_x, ndc_y, ndc_z, 1.0));
    if !view.w.is_finite() || view.w.abs() < 1.0e-6 {
        return None;
    }
    let inv_w = 1.0 / view.w;
    let point = vec3f(view.x * inv_w, view.y * inv_w, view.z * inv_w);
    (point.x.is_finite() && point.y.is_finite() && point.z.is_finite()).then_some(point)
}

pub fn depth_ndc_to_world_ray(
    inv_depth_proj: Mat4f,
    world_from_depth_view: Mat4f,
    ndc_x: f32,
    ndc_y: f32,
) -> Option<Vec3f> {
    let view = depth_ndc_to_view_with_inv_proj(inv_depth_proj, ndc_x, ndc_y, 1.0)?;
    let view_direction = view.normalize();
    if view_direction.length() <= 1.0e-4 {
        return None;
    }
    let world_direction = world_from_depth_view.transform_vec4(vec4f(
        view_direction.x,
        view_direction.y,
        view_direction.z,
        0.0,
    ));
    let direction = vec3f(world_direction.x, world_direction.y, world_direction.z).normalize();
    (direction.length() > 1.0e-4).then_some(direction)
}

pub fn score_submit_depth_frame_novelty(
    grid: &SparseTsdGridReadSnapshot,
    camera_world: Vec3f,
    inv_depth_proj: Mat4f,
    world_from_depth_view: Mat4f,
) -> SubmitDepthFrameNovelty {
    let Some((mut bounds_min, mut bounds_max)) =
        grid.world_bounds(DEPTH_TSD_NOVELTY_BOUNDS_PADDING_VOXELS)
    else {
        return SubmitDepthFrameNovelty {
            force_readback: true,
            ..Default::default()
        };
    };
    bounds_min.y -= DEPTH_TSD_SUBMIT_VERTICAL_BOUNDS_PADDING_METERS;
    bounds_max.y += DEPTH_TSD_SUBMIT_VERTICAL_BOUNDS_PADDING_METERS;
    if !point_inside_bounds_xz(camera_world, bounds_min, bounds_max) {
        return SubmitDepthFrameNovelty {
            force_readback: true,
            ..Default::default()
        };
    }

    let mut novelty = SubmitDepthFrameNovelty::default();
    let lookahead_distance = (grid.voxel_size * 2.0).max(0.1);
    for y in 0..DEPTH_TSD_SUBMIT_SAMPLE_GRID_Y {
        let y_t = if DEPTH_TSD_SUBMIT_SAMPLE_GRID_Y == 1 {
            0.5
        } else {
            y as f32 / (DEPTH_TSD_SUBMIT_SAMPLE_GRID_Y - 1) as f32
        };
        let ndc_y =
            -DEPTH_TSD_SUBMIT_SAMPLE_NDC_MARGIN + y_t * DEPTH_TSD_SUBMIT_SAMPLE_NDC_MARGIN * 2.0;
        for x in 0..DEPTH_TSD_SUBMIT_SAMPLE_GRID_X {
            let x_t = if DEPTH_TSD_SUBMIT_SAMPLE_GRID_X == 1 {
                0.5
            } else {
                x as f32 / (DEPTH_TSD_SUBMIT_SAMPLE_GRID_X - 1) as f32
            };
            let ndc_x = -DEPTH_TSD_SUBMIT_SAMPLE_NDC_MARGIN
                + x_t * DEPTH_TSD_SUBMIT_SAMPLE_NDC_MARGIN * 2.0;
            let Some(ray_direction) =
                depth_ndc_to_world_ray(inv_depth_proj, world_from_depth_view, ndc_x, ndc_y)
            else {
                continue;
            };
            novelty.valid_rays += 1;
            let Some(exit_distance) =
                ray_aabb_exit_distance(camera_world, ray_direction, bounds_min, bounds_max)
            else {
                continue;
            };
            if exit_distance > DEPTH_TSD_SUBMIT_FRONTIER_HORIZON_METERS {
                continue;
            }
            let boundary_distance = (exit_distance - grid.voxel_size).max(0.0);
            let boundary_world = camera_world + ray_direction.scale(boundary_distance);
            let (boundary_x, boundary_y, boundary_z) = grid.world_to_voxel_xyz(boundary_world);
            let boundary_distance_value =
                grid.normalized_distance_xyz(boundary_x, boundary_y, boundary_z);
            let boundary_confidence = grid.confidence_xyz(boundary_x, boundary_y, boundary_z);
            let known_frontier_surface = boundary_distance_value.is_some_and(|distance| {
                boundary_confidence >= DEPTH_TSD_STABLE_CONFIDENCE
                    && distance.abs() <= DEPTH_TSD_SUBMIT_FRONTIER_SURFACE_ABS_DISTANCE
            });
            if known_frontier_surface {
                continue;
            }
            novelty.score +=
                1.0 - (exit_distance / DEPTH_TSD_SUBMIT_FRONTIER_HORIZON_METERS).clamp(0.0, 1.0);
            if boundary_distance_value.is_none() {
                novelty.score += 0.75;
            } else {
                let confidence_gap = DEPTH_TSD_STABLE_CONFIDENCE.saturating_sub(boundary_confidence)
                    as f32
                    / DEPTH_TSD_STABLE_CONFIDENCE as f32;
                novelty.score += 0.35 * confidence_gap;
            }
            let beyond_frontier_world =
                camera_world + ray_direction.scale(exit_distance + lookahead_distance);
            let (beyond_x, beyond_y, beyond_z) = grid.world_to_voxel_xyz(beyond_frontier_world);
            if grid
                .normalized_distance_xyz(beyond_x, beyond_y, beyond_z)
                .is_none()
            {
                novelty.score += 0.5;
            }
        }
    }
    novelty
}

fn submit_depth_readback_min_interval_millis(novelty: SubmitDepthFrameNovelty) -> u64 {
    if novelty.force_readback {
        DEPTH_TSD_SUBMIT_STRONG_INTERVAL_MILLIS
    } else if novelty.score >= DEPTH_TSD_SUBMIT_STRONG_NOVELTY_SCORE {
        DEPTH_TSD_SUBMIT_STRONG_INTERVAL_MILLIS
    } else if novelty.score >= DEPTH_TSD_SUBMIT_MEDIUM_NOVELTY_SCORE {
        DEPTH_TSD_SUBMIT_MEDIUM_INTERVAL_MILLIS
    } else {
        DEPTH_TSD_TARGET_INTEGRATION_INTERVAL_MILLIS
    }
}

pub fn submit_should_readback_depth_frame(
    snapshot: Option<&TsdfPublishedSnapshot>,
    camera_world: Vec3f,
    inv_depth_proj: Mat4f,
    world_from_depth_view: Mat4f,
    now: Instant,
    last_depth_readback_at: Option<Instant>,
) -> bool {
    let Some(snapshot) = snapshot else {
        return last_depth_readback_at.is_none_or(|last| {
            now.duration_since(last)
                >= std::time::Duration::from_millis(DEPTH_TSD_SUBMIT_STRONG_INTERVAL_MILLIS)
        });
    };
    if snapshot.grid.is_empty() {
        return last_depth_readback_at.is_none_or(|last| {
            now.duration_since(last)
                >= std::time::Duration::from_millis(DEPTH_TSD_SUBMIT_STRONG_INTERVAL_MILLIS)
        });
    }
    let novelty = score_submit_depth_frame_novelty(
        &snapshot.grid,
        camera_world,
        inv_depth_proj,
        world_from_depth_view,
    );
    let min_interval_millis = submit_depth_readback_min_interval_millis(novelty);
    last_depth_readback_at.is_none_or(|last| {
        now.duration_since(last) >= std::time::Duration::from_millis(min_interval_millis)
    })
}

fn tsd_chunk_key_and_local_id(coord: VoxelCoord) -> (VoxelCoord, u16) {
    let chunk_key = VoxelCoord::new(
        coord.x.div_euclid(DEPTH_TSD_CHUNK_EDGE_VOXELS),
        coord.y.div_euclid(DEPTH_TSD_CHUNK_EDGE_VOXELS),
        coord.z.div_euclid(DEPTH_TSD_CHUNK_EDGE_VOXELS),
    );
    let lx = coord.x.rem_euclid(DEPTH_TSD_CHUNK_EDGE_VOXELS) as usize;
    let ly = coord.y.rem_euclid(DEPTH_TSD_CHUNK_EDGE_VOXELS) as usize;
    let lz = coord.z.rem_euclid(DEPTH_TSD_CHUNK_EDGE_VOXELS) as usize;
    let edge = DEPTH_TSD_CHUNK_EDGE_VOXELS as usize;
    let local_id = lx + ly * edge + lz * edge * edge;
    (chunk_key, local_id as u16)
}

fn tsd_voxel_coord_from_chunk_key_and_local_id(chunk_key: VoxelCoord, local_id: u16) -> VoxelCoord {
    let edge = DEPTH_TSD_CHUNK_EDGE_VOXELS as usize;
    let local_id = local_id as usize;
    let lx = local_id % edge;
    let ly = (local_id / edge) % edge;
    let lz = local_id / (edge * edge);
    VoxelCoord::new(
        chunk_key.x * DEPTH_TSD_CHUNK_EDGE_VOXELS + lx as i32,
        chunk_key.y * DEPTH_TSD_CHUNK_EDGE_VOXELS + ly as i32,
        chunk_key.z * DEPTH_TSD_CHUNK_EDGE_VOXELS + lz as i32,
    )
}

#[inline(always)]
fn ray_voxel_axis_step(
    point_axis: f32,
    direction_axis: f32,
    voxel_size: f32,
    coord_axis: i32,
    start_distance: f32,
) -> (i32, f32, f32) {
    if direction_axis > 1.0e-6 {
        let next_boundary = (coord_axis + 1) as f32 * voxel_size;
        (
            1,
            start_distance + (next_boundary - point_axis) / direction_axis,
            voxel_size / direction_axis,
        )
    } else if direction_axis < -1.0e-6 {
        let next_boundary = coord_axis as f32 * voxel_size;
        (
            -1,
            start_distance + (next_boundary - point_axis) / direction_axis,
            -voxel_size / direction_axis,
        )
    } else {
        (0, f32::INFINITY, f32::INFINITY)
    }
}

#[derive(Clone, Debug)]
struct SparseTsdChunk {
    data: Arc<SparseTsdReadChunk>,
    live_count: usize,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SparseTsdWriteResult {
    pub state_changed: bool,
    pub value_changed: bool,
    pub became_live: bool,
}

impl SparseTsdChunk {
    fn new(chunk_volume: usize) -> Self {
        Self {
            data: Arc::new(SparseTsdReadChunk::new(chunk_volume)),
            live_count: 0,
        }
    }

    fn is_valid(&self, id: usize) -> bool {
        SparseTsdReadChunk::is_valid_index(&self.data.valid_bits, id)
    }

    fn value(&self, id: usize) -> Option<f32> {
        if !self.is_valid(id) {
            None
        } else {
            Some(SparseTsdReadChunk::decode_normalized_distance(
                self.data.values[id],
            ))
        }
    }

    fn confidence(&self, id: usize) -> u8 {
        if !self.is_valid(id) {
            0
        } else {
            self.data.confidence[id]
        }
    }

    fn accumulate(&mut self, id: usize, value: f32, generation: u64) -> SparseTsdWriteResult {
        let previous_valid = self.is_valid(id);
        let previous_confidence = self.data.confidence[id];
        let previous_generation = self.data.observed_generation[id];
        let previous = self.value(id);
        let value = value.clamp(-1.0, 1.0);
        let next_value = if let Some(previous) = previous {
            let delta = (previous - value).abs();
            let mut confidence = self.data.confidence[id].max(1);
            if delta < 0.08 {
                confidence = confidence.saturating_add(2).min(DEPTH_TSD_MAX_CONFIDENCE);
            } else if delta > 0.35 {
                confidence = confidence.saturating_sub(2).max(1);
            }
            let confidence = confidence as f32;
            previous + (value - previous) / (confidence + 1.0)
        } else {
            value
        }
        .clamp(-1.0, 1.0);
        let changed = previous
            .map(|previous| (previous - next_value).abs() > 1.0e-4)
            .unwrap_or(true);
        let data = Arc::make_mut(&mut self.data);
        data.values[id] = SparseTsdReadChunk::encode_normalized_distance(next_value);
        SparseTsdReadChunk::set_valid_index(&mut data.valid_bits, id);
        if let Some(previous) = previous {
            let delta = (previous - value).abs();
            let confidence = &mut data.confidence[id];
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
            data.confidence[id] = 1;
        }
        data.observed_generation[id] = SparseTsdReadChunk::encode_generation_tag(generation);
        if previous.is_none() {
            self.live_count += 1;
        }
        SparseTsdWriteResult {
            state_changed: !previous_valid
                || changed
                || previous_confidence != data.confidence[id]
                || previous_generation != data.observed_generation[id],
            value_changed: changed,
            became_live: previous.is_none(),
        }
    }

    fn overwrite(&mut self, id: usize, value: f32, generation: u64) -> SparseTsdWriteResult {
        let previous_valid = self.is_valid(id);
        let previous_confidence = self.data.confidence[id];
        let previous_generation = self.data.observed_generation[id];
        let previous = self.value(id);
        let value = value.clamp(-1.0, 1.0);
        let changed = previous
            .map(|previous| (previous - value).abs() > 1.0e-4)
            .unwrap_or(true);
        let data = Arc::make_mut(&mut self.data);
        data.values[id] = SparseTsdReadChunk::encode_normalized_distance(value);
        SparseTsdReadChunk::set_valid_index(&mut data.valid_bits, id);
        data.confidence[id] = DEPTH_TSD_MAX_CONFIDENCE;
        data.observed_generation[id] = SparseTsdReadChunk::encode_generation_tag(generation);
        if previous.is_none() {
            self.live_count += 1;
        }
        SparseTsdWriteResult {
            state_changed: !previous_valid
                || changed
                || previous_confidence != data.confidence[id]
                || previous_generation != data.observed_generation[id],
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

    fn build_read_snapshot(&self) -> SparseTsdGridReadSnapshot {
        let mut chunks = HashMap::with_capacity(self.chunks.len());
        for (&chunk_key, chunk) in &self.chunks {
            if chunk.live_count == 0 {
                continue;
            }
            let read_chunk_key = voxel_coord_to_chunk_key(chunk_key);
            chunks.insert(read_chunk_key, chunk.data.clone());
        }
        let chunk_edge_shift = self
            .chunk_edge
            .is_positive()
            .then_some(self.chunk_edge as u32)
            .filter(|edge| edge.is_power_of_two())
            .map(|edge| edge.trailing_zeros() as u8);
        SparseTsdGridReadSnapshot {
            voxel_size: self.voxel_size,
            chunk_edge: self.chunk_edge,
            chunk_edge_shift,
            chunk_edge_mask: if chunk_edge_shift.is_some() {
                self.chunk_edge - 1
            } else {
                0
            },
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

#[derive(Debug)]
pub struct DepthMeshVolume {
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
    update_sequence: u64,
    dirty_chunk_keys: Vec<ChunkKey>,
    removed_chunk_keys: Vec<ChunkKey>,
    latest_camera_world: Option<Vec3f>,
    latest_camera_forward: Option<Vec3f>,
    projected_height_field: Option<ProjectedHeightField>,
    projected_height_layout_rebuild_pending: bool,
    projected_height_publish_pending: bool,
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
    pub fn new(sample_step: u32, voxel_size_meters: f32) -> Self {
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
            mesh_grid: SparseTsdGrid::new(voxel_size_meters, DEPTH_TSD_CHUNK_EDGE_VOXELS),
            dirty_tsdf_chunk_keys: HashSet::new(),
            update_sequence: 0,
            dirty_chunk_keys: Vec::new(),
            removed_chunk_keys: Vec::new(),
            latest_camera_world: None,
            latest_camera_forward: None,
            projected_height_field: None,
            projected_height_layout_rebuild_pending: false,
            projected_height_publish_pending: false,
            published_height_map: None,
            pending_mesh_dirty_chunks: HashSet::new(),
            pending_mesh_chunk_queue: VecDeque::new(),
            pending_plane_scan_dirty_chunks: HashSet::new(),
            pending_plane_scan_chunk_queue: VecDeque::new(),
            pending_projected_height_dirty_samples: HashSet::new(),
            pending_projected_height_sample_queue: VecDeque::new(),
        }
    }

    pub fn voxel_size_meters(&self) -> f32 {
        self.voxel_size_meters
    }

    pub fn pending_projected_height_sample_count(&self) -> usize {
        self.pending_projected_height_sample_queue.len()
    }

    pub fn projected_height_publish_pending(&self) -> bool {
        self.projected_height_publish_pending
    }

    pub fn set_projected_height_publish_pending(&mut self, pending: bool) {
        self.projected_height_publish_pending = pending;
    }

    pub fn has_published_height_map(&self) -> bool {
        self.published_height_map.is_some()
    }

    fn update_bounds(&mut self) {
        if let Some((min, max)) = self.mesh_grid.world_bounds(0) {
            self.bounds_min = min;
            self.bounds_max = max;
        } else {
            (self.bounds_min, self.bounds_max) = empty_bounds();
        }
    }

    pub fn published_tsdf_snapshot(
        &self,
        previous: Option<&TsdfPublishedSnapshot>,
    ) -> TsdfPublishedSnapshot {
        let grid = if self.dirty_tsdf_chunk_keys.is_empty() {
            previous
                .map(|previous| previous.grid.clone())
                .unwrap_or_else(|| Arc::new(self.mesh_grid.build_read_snapshot()))
        } else {
            Arc::new(self.mesh_grid.build_read_snapshot())
        };
        TsdfPublishedSnapshot {
            generation: self.generation,
            latest_topology_generation: self.latest_topology_generation,
            update_sequence: self.update_sequence,
            grid,
            height_map: self.published_height_map.clone(),
        }
    }

    pub fn clear_published_tsdf_dirty_state(&mut self) {
        self.dirty_tsdf_chunk_keys.clear();
    }

    pub fn clear_published_height_map(&mut self) -> bool {
        if self.published_height_map.is_none() {
            return false;
        }
        self.published_height_map = None;
        self.update_sequence = self.update_sequence.saturating_add(1);
        true
    }

    pub fn discard_obsolete_surface_state(&mut self) {
        self.dirty_chunk_keys.clear();
        self.removed_chunk_keys.clear();
        self.pending_mesh_dirty_chunks.clear();
        self.pending_mesh_chunk_queue.clear();
        self.pending_plane_scan_dirty_chunks.clear();
        self.pending_plane_scan_chunk_queue.clear();
    }
}

#[derive(Clone, Debug)]
pub struct DepthMeshJob {
    pub reset_generation: u64,
    pub generation: u64,
    pub eye_index: usize,
    pub width: u32,
    pub height: u32,
    pub sample_step: u32,
    pub voxel_size_meters: f32,
    pub camera_world: Vec3f,
    pub camera_forward: Vec3f,
    pub depth_proj: Mat4f,
    pub inv_depth_proj: Mat4f,
    pub depth_view_from_world: Mat4f,
    pub world_from_depth_view: Mat4f,
    pub depth: Vec<u16>,
}

pub struct PreparedDepthMeshJob {
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
    visible_world_min: Vec3f,
    visible_world_max: Vec3f,
    depth: Vec<u16>,
}

impl PreparedDepthMeshJob {
    pub fn reset_generation(&self) -> u64 {
        self.reset_generation
    }

    pub fn voxel_size_meters(&self) -> f32 {
        self.voxel_size_meters
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct FrameTsdSample {
    chunk_key: VoxelCoord,
    local_id: u16,
    normalized: f32,
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
pub struct DepthPreprocessWorkerState {
    sampled_depth: Vec<DepthGridSample>,
    reduced_tsd_samples: Vec<FrameTsdSample>,
    frame_tsd_chunks: HashMap<VoxelCoord, FrameTsdAccumChunk>,
    frame_tsd_dirty_chunks: Vec<VoxelCoord>,
    staged_tsd_sample_visits: usize,
    depth_width: usize,
    depth_height: usize,
}

struct FrameTsdAccumChunk {
    sums: Box<[f32; DEPTH_TSD_CHUNK_VOLUME]>,
    counts: Box<[u16; DEPTH_TSD_CHUNK_VOLUME]>,
    touched_bits: [u64; DEPTH_TSD_CHUNK_VALID_WORDS],
    touched_count: usize,
}

impl FrameTsdAccumChunk {
    fn new() -> Self {
        Self {
            sums: Box::new([0.0; DEPTH_TSD_CHUNK_VOLUME]),
            counts: Box::new([0; DEPTH_TSD_CHUNK_VOLUME]),
            touched_bits: [0; DEPTH_TSD_CHUNK_VALID_WORDS],
            touched_count: 0,
        }
    }

    #[inline(always)]
    fn record(&mut self, local_id: usize, normalized: f32) {
        let (word_index, bit_mask) = SparseTsdReadChunk::valid_bit_parts(local_id);
        if (self.touched_bits[word_index] & bit_mask) == 0 {
            self.touched_bits[word_index] |= bit_mask;
            self.touched_count += 1;
        }
        self.sums[local_id] += normalized;
        self.counts[local_id] = self.counts[local_id].saturating_add(1);
    }

    fn clear_touched(&mut self) {
        for word_index in 0..self.touched_bits.len() {
            let mut pending_bits = self.touched_bits[word_index];
            while pending_bits != 0 {
                let bit_index = pending_bits.trailing_zeros() as usize;
                pending_bits &= pending_bits - 1;
                let local_id = word_index * u64::BITS as usize + bit_index;
                if local_id < DEPTH_TSD_CHUNK_VOLUME {
                    self.sums[local_id] = 0.0;
                    self.counts[local_id] = 0;
                }
            }
        }
        self.touched_bits.fill(0);
        self.touched_count = 0;
    }

    fn push_reduced_samples(
        &self,
        chunk_key: VoxelCoord,
        reduced_tsd_samples: &mut Vec<FrameTsdSample>,
    ) {
        for word_index in 0..self.touched_bits.len() {
            let mut pending_bits = self.touched_bits[word_index];
            while pending_bits != 0 {
                let bit_index = pending_bits.trailing_zeros() as usize;
                pending_bits &= pending_bits - 1;
                let local_id = word_index * u64::BITS as usize + bit_index;
                if local_id >= DEPTH_TSD_CHUNK_VOLUME {
                    continue;
                }
                let count = self.counts[local_id].max(1) as f32;
                reduced_tsd_samples.push(FrameTsdSample {
                    chunk_key,
                    local_id: local_id as u16,
                    normalized: self.sums[local_id] / count,
                });
            }
        }
    }
}

impl DepthPreprocessWorkerState {
    pub fn staged_tsd_sample_visits(&self) -> usize {
        self.staged_tsd_sample_visits
    }

    pub fn reduced_tsd_sample_count(&self) -> usize {
        self.reduced_tsd_samples.len()
    }

    fn clear_frame_tsd_accum(&mut self) {
        for chunk_key in self.frame_tsd_dirty_chunks.drain(..) {
            if let Some(chunk) = self.frame_tsd_chunks.get_mut(&chunk_key) {
                chunk.clear_touched();
            }
        }
        self.staged_tsd_sample_visits = 0;
        self.reduced_tsd_samples.clear();
    }

    #[inline(always)]
    fn record_tsd_sample(&mut self, coord: VoxelCoord, normalized: f32) {
        let (chunk_key, local_id) = tsd_chunk_key_and_local_id(coord);
        let chunk = self
            .frame_tsd_chunks
            .entry(chunk_key)
            .or_insert_with(FrameTsdAccumChunk::new);
        if chunk.touched_count == 0 {
            self.frame_tsd_dirty_chunks.push(chunk_key);
        }
        chunk.record(local_id as usize, normalized);
        self.staged_tsd_sample_visits += 1;
    }

    fn build_reduced_tsd_samples(&mut self) {
        let capacity = self
            .frame_tsd_dirty_chunks
            .iter()
            .filter_map(|chunk_key| self.frame_tsd_chunks.get(chunk_key))
            .map(|chunk| chunk.touched_count)
            .sum();
        self.reduced_tsd_samples.reserve(capacity);
        for &chunk_key in &self.frame_tsd_dirty_chunks {
            if let Some(chunk) = self.frame_tsd_chunks.get(&chunk_key) {
                chunk.push_reduced_samples(chunk_key, &mut self.reduced_tsd_samples);
            }
        }
        self.reduced_tsd_samples.sort_unstable_by(|left, right| {
            left.chunk_key
                .cmp(&right.chunk_key)
                .then(left.local_id.cmp(&right.local_id))
        });
    }
}

#[inline(always)]
fn append_tsd_samples_along_ray(
    worker_state: &mut DepthPreprocessWorkerState,
    camera_world: Vec3f,
    ray_dir: Vec3f,
    surface_distance: f32,
    start_distance: f32,
    end_distance: f32,
    voxel_size_meters: f32,
    tsd_distance_meters: f32,
) {
    debug_assert!(end_distance >= start_distance);
    let start_distance = start_distance.max(0.0);
    let end_distance = end_distance.max(start_distance);
    if !start_distance.is_finite() || !end_distance.is_finite() {
        return;
    }

    let segment_bias = (voxel_size_meters * 1.0e-4).min(1.0e-4);
    let traversal_start = (start_distance + segment_bias).min(end_distance);
    let start_point = camera_world + ray_dir.scale(traversal_start);
    let mut coord = VoxelCoord::new(
        (start_point.x / voxel_size_meters).floor() as i32,
        (start_point.y / voxel_size_meters).floor() as i32,
        (start_point.z / voxel_size_meters).floor() as i32,
    );
    let (step_x, mut t_max_x, t_delta_x) = ray_voxel_axis_step(
        start_point.x,
        ray_dir.x,
        voxel_size_meters,
        coord.x,
        traversal_start,
    );
    let (step_y, mut t_max_y, t_delta_y) = ray_voxel_axis_step(
        start_point.y,
        ray_dir.y,
        voxel_size_meters,
        coord.y,
        traversal_start,
    );
    let (step_z, mut t_max_z, t_delta_z) = ray_voxel_axis_step(
        start_point.z,
        ray_dir.z,
        voxel_size_meters,
        coord.z,
        traversal_start,
    );
    const CROSSING_EPSILON: f32 = 1.0e-5;

    loop {
        let voxel_world = vec3f(
            voxel_center_axis(voxel_size_meters, coord.x),
            voxel_center_axis(voxel_size_meters, coord.y),
            voxel_center_axis(voxel_size_meters, coord.z),
        );
        if !point_inside_player_exclusion(camera_world, voxel_world) {
            let voxel_distance = (voxel_world - camera_world).dot(ray_dir);
            if voxel_distance.is_finite() {
                let normalized =
                    ((surface_distance - voxel_distance) / tsd_distance_meters).clamp(-1.0, 1.0);
                worker_state.record_tsd_sample(coord, normalized);
            }
        }

        let next_crossing = t_max_x.min(t_max_y.min(t_max_z));
        if next_crossing > end_distance {
            break;
        }
        if t_max_x <= next_crossing + CROSSING_EPSILON {
            coord.x += step_x;
            t_max_x += t_delta_x;
        }
        if t_max_y <= next_crossing + CROSSING_EPSILON {
            coord.y += step_y;
            t_max_y += t_delta_y;
        }
        if t_max_z <= next_crossing + CROSSING_EPSILON {
            coord.z += step_z;
            t_max_z += t_delta_z;
        }
    }
}

pub fn score_depth_job_novelty(volume: &DepthMeshVolume, job: &DepthMeshJob) -> DepthFrameNovelty {
    let width = job.width as usize;
    let height = job.height as usize;
    if width == 0 || height == 0 {
        return DepthFrameNovelty::default();
    }
    let inner_width = width
        .saturating_sub(DEPTH_IMAGE_EDGE_MARGIN_PIXELS * 2)
        .max(1);
    let inner_height = height
        .saturating_sub(DEPTH_IMAGE_EDGE_MARGIN_PIXELS * 2)
        .max(1);
    let x_step = inner_width.div_ceil(DEPTH_TSD_NOVELTY_SAMPLE_GRID_X).max(1);
    let y_step = inner_height
        .div_ceil(DEPTH_TSD_NOVELTY_SAMPLE_GRID_Y)
        .max(1);
    let padded_bounds = volume
        .mesh_grid
        .world_bounds(DEPTH_TSD_NOVELTY_BOUNDS_PADDING_VOXELS);
    let mut novelty = DepthFrameNovelty::default();

    for y in (0..height).step_by(y_step) {
        for x in (0..width).step_by(x_step) {
            if !depth_pixel_inside_margin(width, height, x, y) {
                continue;
            }
            let Some(world) = depth_pixel_to_world(job, x as u32, y as u32) else {
                continue;
            };
            if point_inside_player_exclusion(job.camera_world, world) {
                continue;
            }
            novelty.valid_samples += 1;

            if padded_bounds.is_some_and(|(min, max)| {
                world.x < min.x
                    || world.x > max.x
                    || world.y < min.y
                    || world.y > max.y
                    || world.z < min.z
                    || world.z > max.z
            }) {
                novelty.score += DEPTH_TSD_NOVELTY_OUTSIDE_BOUNDS_WEIGHT;
                continue;
            }

            let coord = volume.mesh_grid.world_to_voxel_coord(world);
            if let Some(distance) = volume.mesh_grid.normalized_distance(coord) {
                let confidence = volume.mesh_grid.confidence(coord);
                let confidence_gap = DEPTH_TSD_STABLE_CONFIDENCE.saturating_sub(confidence) as f32
                    / DEPTH_TSD_STABLE_CONFIDENCE as f32;
                novelty.score += DEPTH_TSD_NOVELTY_LOW_CONFIDENCE_WEIGHT * confidence_gap;
                let distance_excess =
                    (distance.abs() - DEPTH_TSD_NOVELTY_SURFACE_MISMATCH_ABS_DISTANCE).max(0.0);
                if distance_excess > 0.0 {
                    novelty.score += DEPTH_TSD_NOVELTY_SURFACE_MISMATCH_WEIGHT
                        * (distance_excess
                            / (1.0 - DEPTH_TSD_NOVELTY_SURFACE_MISMATCH_ABS_DISTANCE));
                }
            } else {
                novelty.score += DEPTH_TSD_NOVELTY_UNKNOWN_WEIGHT;
            }
        }
    }

    novelty
}

pub fn preprocess_depth_mesh(
    job: DepthMeshJob,
    worker_state: &mut DepthPreprocessWorkerState,
) -> Result<PreparedDepthMeshJob, String> {
    rebuild_sampled_depth_grid(&job, worker_state);

    let voxel_size_meters = job.voxel_size_meters;
    let tsd_distance_meters = depth_tsd_distance_meters(voxel_size_meters);
    worker_state.clear_frame_tsd_accum();
    let mut observed_world_min = vec3f(f32::INFINITY, f32::INFINITY, f32::INFINITY);
    let mut observed_world_max = vec3f(f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
    let sample_step = job.sample_step.max(1) as usize;

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
            append_tsd_samples_along_ray(
                worker_state,
                job.camera_world,
                ray_dir,
                surface_distance,
                start_distance,
                end_distance,
                voxel_size_meters,
                tsd_distance_meters,
            );
        }
    }

    worker_state.build_reduced_tsd_samples();

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

    Ok(PreparedDepthMeshJob {
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
        visible_world_min,
        visible_world_max,
        depth: job.depth,
    })
}

pub fn apply_preprocessed_depth_mesh(
    job: PreparedDepthMeshJob,
    worker_state: &DepthPreprocessWorkerState,
    volume: &mut DepthMeshVolume,
) {
    volume.generation = job.generation;
    volume.eye_index = job.eye_index;
    volume.image_width = job.width;
    volume.image_height = job.height;
    volume.sample_step = job.sample_step;
    volume.latest_camera_world = Some(job.camera_world);
    volume.latest_camera_forward = Some(job.camera_forward);

    let mut topology_changes = apply_tsd_samples(volume, &worker_state.reduced_tsd_samples);
    topology_changes += refresh_visible_free_space(volume, &job);
    topology_changes += clear_player_exclusion_volume(volume, job.camera_world);
    if topology_changes != 0 {
        volume.latest_topology_generation = job.generation;
    }
    volume.update_bounds();
}

pub fn update_published_height_map(volume: &mut DepthMeshVolume) -> bool {
    let next_height_map = build_projected_height_map(volume);
    if volume.published_height_map == next_height_map {
        return false;
    }
    volume.published_height_map = next_height_map;
    volume.update_sequence = volume.update_sequence.saturating_add(1);
    true
}

pub fn projected_height_refresh_budget(
    pending_samples: usize,
    now: Instant,
    next_publish_at: Instant,
    slice_credits: usize,
) -> usize {
    if pending_samples == 0 || slice_credits == 0 {
        return 0;
    }
    let remaining_millis = next_publish_at.saturating_duration_since(now).as_millis() as usize;
    let slice_interval_millis = DEPTH_PROJECTED_HEIGHT_REFRESH_INTERVAL_MILLIS.max(1) as usize;
    let remaining_slices = remaining_millis.div_ceil(slice_interval_millis).max(1);
    let samples_per_slice = pending_samples.div_ceil(remaining_slices).clamp(
        DEPTH_ALIGN_PROJECTED_HEIGHT_SAMPLES_PER_TICK,
        DEPTH_ALIGN_PROJECTED_HEIGHT_MAX_SAMPLES_PER_SLICE,
    );
    samples_per_slice.saturating_mul(slice_credits).min(
        DEPTH_ALIGN_PROJECTED_HEIGHT_MAX_SAMPLES_PER_SLICE
            * DEPTH_ALIGN_PROJECTED_HEIGHT_MAX_SLICE_CREDITS,
    )
}

pub fn sync_projected_height_field_layout(volume: &mut DepthMeshVolume) {
    let Some(layout) = projected_height_field_layout(volume) else {
        volume.projected_height_field = None;
        volume.projected_height_layout_rebuild_pending = false;
        volume.projected_height_publish_pending = false;
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
    let copied_overlap = previous_field.as_ref().is_some_and(|previous_field| {
        copy_projected_height_field_overlap(previous_field, &mut next_field)
    });
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
    volume.projected_height_publish_pending = true;
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

pub fn sync_projected_height_field_player_cutout(volume: &mut DepthMeshVolume) {
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
    volume.projected_height_publish_pending = true;
}

pub fn refresh_projected_height_field(volume: &mut DepthMeshVolume, max_samples: usize) -> bool {
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
            let previous_index =
                projected_height_sample_index(previous_size_x, previous_x, previous_z);
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
        if next_x < 0
            || next_z < 0
            || next_x >= next_size_x as isize
            || next_z >= next_size_z as isize
        {
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
    if raw_max_x < 0 || raw_max_z < 0 || raw_min_x > max_sample_x || raw_min_z > max_sample_z {
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
    let min_y = bounds_min.y + volume.voxel_size_meters;
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

fn estimate_projected_height_floor_y(valid_cell_heights: &[f32], bottom_y: f32, top_y: f32) -> f32 {
    let heights = valid_cell_heights
        .iter()
        .copied()
        .filter(|value| value.is_finite())
        .collect::<Vec<_>>();
    if heights.is_empty() {
        return bottom_y;
    }
    let span = (top_y - bottom_y).max(1.0e-3);
    let bin_size = DEPTH_ALIGN_HEIGHT_MAP_FLOOR_BIN_METERS
        .max(span / 256.0)
        .min(span);
    let bin_count = ((span / bin_size).ceil() as usize).max(1) + 1;
    let mut bins = vec![0usize; bin_count];
    for height in &heights {
        let bin = (((*height - bottom_y) / bin_size).floor() as isize)
            .clamp(0, bin_count.saturating_sub(1) as isize) as usize;
        bins[bin] += 1;
    }

    let min_support = DEPTH_ALIGN_HEIGHT_MAP_FLOOR_MIN_SUPPORT_CELLS.max(
        (heights.len() as f32 * DEPTH_ALIGN_HEIGHT_MAP_FLOOR_MIN_SUPPORT_RATIO).ceil() as usize,
    );
    let support_window_bins = DEPTH_ALIGN_HEIGHT_MAP_FLOOR_SUPPORT_WINDOW_BINS.max(1);
    for start_bin in 0..bin_count {
        let end_bin = (start_bin + support_window_bins).min(bin_count);
        let support = bins[start_bin..end_bin].iter().copied().sum::<usize>();
        if support < min_support {
            continue;
        }
        let low = bottom_y + start_bin as f32 * bin_size;
        let high = bottom_y + end_bin as f32 * bin_size;
        let mut sum = 0.0;
        let mut count = 0usize;
        for height in &heights {
            if *height >= low && *height <= high {
                sum += *height;
                count += 1;
            }
        }
        if count > 0 {
            return (sum / count as f32).clamp(bottom_y, top_y);
        }
    }

    let mut sorted = heights;
    sorted.sort_by(|left, right| left.total_cmp(right));
    let fallback_count = ((sorted.len() as f32 * 0.01).ceil() as usize).clamp(1, 32);
    let fallback = &sorted[..fallback_count];
    (fallback.iter().copied().sum::<f32>() / fallback.len() as f32).clamp(bottom_y, top_y)
}

fn build_projected_height_map(volume: &mut DepthMeshVolume) -> Option<XrDepthAlignHeightMap> {
    sync_projected_height_field_layout(volume);
    sync_projected_height_field_player_cutout(volume);
    let field = volume.projected_height_field.as_ref()?;
    let origin_x = field.layout.origin_x;
    let origin_z = field.layout.origin_z;
    let cell_size = field.layout.cell_size_meters;
    let size_x = field.layout.size_x;
    let size_z = field.layout.size_z;
    let sample_size_x = field.sample_size_x();
    let heights = &field.heights_meters;
    let valid = &field.valid;
    let player_cutout_center = field.player_cutout_center;
    let mut height_map = XrDepthAlignHeightMap {
        origin_x,
        origin_z,
        cell_size_meters: cell_size,
        size_x: size_x as u16,
        size_z: size_z as u16,
        bottom_y_meters: field.layout.bottom_y_meters,
        top_y_meters: field.layout.top_y_meters,
        floor_y_meters: field.layout.bottom_y_meters,
        player_cutout_center: player_cutout_center.map(|center| vec2f(center.x, center.z)),
        player_cutout_radius_meters: DEPTH_ALIGN_VECTOR_SLICE_PLAYER_CUTOUT_RADIUS_METERS,
        heights_meters: vec![f32::NAN; size_x * size_z],
    };
    let mut valid_cell_heights = Vec::<f32>::with_capacity(size_x.saturating_mul(size_z));
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
                let height = height_sum / height_count as f32;
                height_map.heights_meters[preview_index] = height;
                valid_cell_heights.push(height);
            }
        }
    }
    height_map.floor_y_meters = estimate_projected_height_floor_y(
        &valid_cell_heights,
        field.layout.bottom_y_meters,
        field.layout.top_y_meters,
    );
    Some(height_map)
}

fn rebuild_sampled_depth_grid(job: &DepthMeshJob, worker_state: &mut DepthPreprocessWorkerState) {
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

fn depth_ndc_to_view(job: &DepthMeshJob, ndc_x: f32, ndc_y: f32, ndc_z: f32) -> Option<Vec3f> {
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

fn depth_view_to_world(job: &DepthMeshJob, view: Vec3f) -> Option<Vec3f> {
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

fn depth_pixel_to_world(job: &DepthMeshJob, x: u32, y: u32) -> Option<Vec3f> {
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
    job: &PreparedDepthMeshJob,
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

fn apply_tsd_samples(volume: &mut DepthMeshVolume, frame_tsd_samples: &[FrameTsdSample]) -> usize {
    debug_assert_eq!(volume.mesh_grid.chunk_edge, DEPTH_TSD_CHUNK_EDGE_VOXELS);
    let mut changed = 0;
    let mut changed_coords = Vec::new();
    let mut index = 0usize;
    while index < frame_tsd_samples.len() {
        let chunk_key = frame_tsd_samples[index].chunk_key;
        let mut chunk_dirty = false;
        let mut became_live = 0usize;
        changed_coords.clear();
        {
            let chunk = volume
                .mesh_grid
                .chunks
                .entry(chunk_key)
                .or_insert_with(|| SparseTsdChunk::new(volume.mesh_grid.chunk_volume));
            while index < frame_tsd_samples.len() && frame_tsd_samples[index].chunk_key == chunk_key
            {
                let sample = frame_tsd_samples[index];
                let local_id = sample.local_id as usize;
                let previous = chunk.value(local_id).unwrap_or(2.0);
                let update = chunk.accumulate(local_id, sample.normalized, volume.generation);
                if update.became_live {
                    became_live += 1;
                }
                chunk_dirty |= update.state_changed;
                if update.value_changed {
                    let current = chunk.value(local_id).unwrap_or(previous);
                    if (previous - current).abs() >= DEPTH_TSD_APPLY_DELTA_EPSILON {
                        changed_coords.push(tsd_voxel_coord_from_chunk_key_and_local_id(
                            chunk_key,
                            sample.local_id,
                        ));
                    }
                }
                index += 1;
            }
        }
        volume.mesh_grid.active_value_count += became_live;
        if chunk_dirty {
            volume.dirty_tsdf_chunk_keys.insert(chunk_key);
        }
        for coord in changed_coords.drain(..) {
            mark_projected_height_samples_dirty_around_voxel(volume, coord);
            changed += 1;
        }
    }
    changed
}

fn refresh_visible_free_space(volume: &mut DepthMeshVolume, job: &PreparedDepthMeshJob) -> usize {
    let tsd_distance_meters = depth_tsd_distance_meters(job.voxel_size_meters);
    let tsd_refresh_clearance_meters = depth_tsd_refresh_clearance_meters(job.voxel_size_meters);
    let min_coord = volume.mesh_grid.world_to_voxel_coord(job.visible_world_min);
    let max_coord = volume.mesh_grid.world_to_voxel_coord(job.visible_world_max);
    let (min_chunk_key, _) = tsd_chunk_key_and_local_id(min_coord);
    let (max_chunk_key, _) = tsd_chunk_key_and_local_id(max_coord);
    let visible_chunk_keys = volume
        .mesh_grid
        .chunks
        .keys()
        .copied()
        .filter(|chunk_key| {
            chunk_key.x >= min_chunk_key.x
                && chunk_key.x <= max_chunk_key.x
                && chunk_key.y >= min_chunk_key.y
                && chunk_key.y <= max_chunk_key.y
                && chunk_key.z >= min_chunk_key.z
                && chunk_key.z <= max_chunk_key.z
        })
        .collect::<Vec<_>>();
    let mut changed = 0;

    for chunk_key in visible_chunk_keys {
        let mut chunk_dirty = false;
        let mut changed_coords = Vec::new();
        {
            let Some(chunk) = volume.mesh_grid.chunks.get_mut(&chunk_key) else {
                continue;
            };
            if chunk.live_count == 0 {
                continue;
            }
            for word_index in 0..chunk.data.valid_bits.len() {
                let mut pending_bits = chunk.data.valid_bits[word_index];
                while pending_bits != 0 {
                    let bit_index = pending_bits.trailing_zeros() as usize;
                    pending_bits &= pending_bits - 1;
                    let local_id = word_index * u64::BITS as usize + bit_index;
                    if local_id >= volume.mesh_grid.chunk_volume {
                        continue;
                    }

                    let coord =
                        tsd_voxel_coord_from_chunk_key_and_local_id(chunk_key, local_id as u16);
                    if coord.x < min_coord.x
                        || coord.x > max_coord.x
                        || coord.y < min_coord.y
                        || coord.y > max_coord.y
                        || coord.z < min_coord.z
                        || coord.z > max_coord.z
                    {
                        continue;
                    }

                    let Some(previous) = chunk.value(local_id) else {
                        continue;
                    };
                    if previous >= 1.0 - 1.0e-4 {
                        continue;
                    }

                    let voxel_world = vec3f(
                        voxel_center_axis(job.voxel_size_meters, coord.x),
                        voxel_center_axis(job.voxel_size_meters, coord.y),
                        voxel_center_axis(job.voxel_size_meters, coord.z),
                    );
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
                    if !depth_pixel_is_reliable_for_carve(job, pixel_x, pixel_y, observed_distance)
                    {
                        continue;
                    }
                    let clearance = observed_distance - voxel_distance;
                    if !observed_distance.is_finite() || clearance < tsd_refresh_clearance_meters {
                        continue;
                    }
                    let confidence = chunk.confidence(local_id);
                    if confidence >= DEPTH_TSD_STABLE_CONFIDENCE
                        && previous <= 0.25
                        && clearance < tsd_distance_meters
                    {
                        continue;
                    }
                    let update = chunk.accumulate(local_id, 1.0, volume.generation);
                    chunk_dirty |= update.state_changed;
                    if update.value_changed {
                        let current = chunk.value(local_id).unwrap_or(previous);
                        if (previous - current).abs() >= DEPTH_TSD_APPLY_DELTA_EPSILON {
                            changed_coords.push(coord);
                        }
                    }
                }
            }
        }
        if chunk_dirty {
            volume.dirty_tsdf_chunk_keys.insert(chunk_key);
        }
        for coord in changed_coords.drain(..) {
            mark_projected_height_samples_dirty_around_voxel(volume, coord);
            changed += 1;
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
                let update =
                    volume
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

fn depth_visible_world_bounds(job: &DepthMeshJob) -> Option<(Vec3f, Vec3f)> {
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
