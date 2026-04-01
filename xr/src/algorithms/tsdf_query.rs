use crate::prelude::*;
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
};

const TSDF_QUERY_MIN_OPPOSING_NORMAL_DOT: f32 = 0.2;
const TSDF_QUERY_TSDF_SUPPORT_GRID_DIM: usize = 5;
const TSDF_QUERY_TSDF_SUPPORT_MAX_SAMPLES: usize =
    TSDF_QUERY_TSDF_SUPPORT_GRID_DIM * TSDF_QUERY_TSDF_SUPPORT_GRID_DIM;
const TSDF_QUERY_TSDF_SUPPORT_MIN_SAMPLES: usize = 4;
const TSDF_QUERY_TRAJECTORY_SAMPLE_COUNT: usize = 5;
const TSDF_QUERY_TSDF_SUPPORT_NORMAL_Y_MIN: f32 = 0.60;
const TSDF_QUERY_TSDF_SUPPORT_RADIUS_SCALE: f32 = 1.15;
const TSDF_QUERY_TSDF_SUPPORT_RADIUS_MIN: f32 = 0.04;
const TSDF_QUERY_TSDF_SUPPORT_RADIUS_MAX: f32 = 0.12;
const TSDF_QUERY_TSDF_SUPPORT_EXTENT_PADDING_SCALE: f32 = 0.22;
const TSDF_QUERY_TSDF_IMPACT_MIN_SPEED: f32 = 0.55;
const TSDF_QUERY_TSDF_IMPACT_MIN_HORIZONTAL_SPEED: f32 = 0.40;
const TSDF_QUERY_TSDF_IMPACT_MIN_UPWARD_SPEED: f32 = 0.55;
const TSDF_QUERY_TSDF_IMPACT_NORMAL_Y_MAX: f32 = 0.72;
const TSDF_QUERY_TSDF_IMPACT_CEILING_NORMAL_Y_MIN: f32 = 0.82;
const TSDF_QUERY_TSDF_IMPACT_RAY_STEP_SCALE: f32 = 0.40;
const TSDF_QUERY_TSDF_IMPACT_RAY_STEP_MIN: f32 = 0.02;
const TSDF_QUERY_TSDF_IMPACT_EXTENT_SCALE: f32 = 1.20;
const TSDF_QUERY_TSDF_IMPACT_EXTENT_MIN: f32 = 0.05;
const TSDF_QUERY_TSDF_IMPACT_EXTENT_MAX: f32 = 0.16;
const TSDF_QUERY_CHUNK_CACHE_SLOTS: usize = 32;
pub(crate) const TSDF_QUERY_IMPACT_RESTITUTION: f32 = 0.38;

#[derive(Clone, Copy, Debug)]
pub(crate) struct DepthQuery {
    pub(crate) key: u64,
    pub(crate) center: Vec3f,
    pub(crate) predicted_center: Vec3f,
    pub(crate) velocity: Vec3f,
    pub(crate) radius: f32,
    pub(crate) max_distance: f32,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct DepthQuerySurfaceHit {
    pub(crate) distance: f32,
    pub(crate) point: Vec3f,
    pub(crate) normal: Vec3f,
    pub(crate) triangle: [Vec3f; 3],
    pub(crate) patch: [Vec3f; 4],
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct DepthQuerySupportPlane {
    pub(crate) point: Vec3f,
    pub(crate) normal: Vec3f,
    pub(crate) tangent: Vec3f,
    pub(crate) bitangent: Vec3f,
    pub(crate) half_extent_tangent: f32,
    pub(crate) half_extent_bitangent: f32,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum DepthQueryColliderGeometry {
    HalfSpace(DepthQuerySupportPlane),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum DepthQueryColliderRole {
    Support,
    Impact,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct DepthQueryCollider {
    pub(crate) fingerprint: u64,
    pub(crate) geometry: DepthQueryColliderGeometry,
    pub(crate) role: DepthQueryColliderRole,
    pub(crate) restitution: f32,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct DepthQueryResolvedSurface {
    pub(crate) surface: DepthQuerySurfaceHit,
    pub(crate) collider: DepthQueryCollider,
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub(crate) struct DepthQueryHit {
    pub(crate) key: u64,
    pub(crate) distance: f32,
    pub(crate) point: Vec3f,
    pub(crate) normal: Vec3f,
    pub(crate) triangle: [Vec3f; 3],
    pub(crate) patch: [Vec3f; 4],
    pub(crate) collider: DepthQueryCollider,
    pub(crate) additional_hits: Vec<DepthQueryResolvedSurface>,
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub(crate) enum DepthQueryResult {
    Hit(DepthQueryHit),
    Miss { key: u64 },
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
struct TsdfVoxelCoord {
    x: i32,
    y: i32,
    z: i32,
}

impl TsdfVoxelCoord {
    const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }
}

impl core::ops::Add for TsdfVoxelCoord {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self::new(self.x + rhs.x, self.y + rhs.y, self.z + rhs.z)
    }
}

impl core::ops::Sub for TsdfVoxelCoord {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self::Output {
        Self::new(self.x - rhs.x, self.y - rhs.y, self.z - rhs.z)
    }
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug)]
struct QueryTrajectorySample {
    progress: f32,
    point: Vec3f,
    velocity: Vec3f,
}

#[derive(Clone, Copy)]
struct DepthGridSupportSample {
    point: Vec3f,
    radial_weight: f32,
}

#[derive(Clone, Copy)]
struct TsdfQueryChunkCacheEntry<'a> {
    key: ChunkKey,
    chunk: &'a SparseTsdReadChunk,
}

struct TsdfQuerySampler<'a> {
    grid: &'a SparseTsdGridReadSnapshot,
    tsd_distance_meters: f32,
    local_y_stride: usize,
    local_z_stride: usize,
    chunk_cache: [Option<TsdfQueryChunkCacheEntry<'a>>; TSDF_QUERY_CHUNK_CACHE_SLOTS],
}

#[derive(Clone, Copy)]
struct TsdfQueryVoxelColumn {
    chunk_x: i32,
    chunk_z: i32,
    local_base: usize,
}

#[derive(Clone, Copy)]
struct TsdfQueryBilinearColumn {
    v00: TsdfQueryVoxelColumn,
    v10: TsdfQueryVoxelColumn,
    v01: TsdfQueryVoxelColumn,
    v11: TsdfQueryVoxelColumn,
    tx: f32,
    tz: f32,
}

fn depth_tsd_distance_meters(voxel_size_meters: f32) -> f32 {
    voxel_size_meters * 2.0
}

fn quantize_f32(value: f32, quantum: f32) -> i32 {
    (value / quantum.max(f32::EPSILON)).round() as i32
}

fn tsdf_query_chunk_cache_slot(key: ChunkKey) -> usize {
    let hash = (key.x as u32).wrapping_mul(0x9E37_79B9)
        ^ (key.y as u32).wrapping_mul(0x85EB_CA6B)
        ^ (key.z as u32).wrapping_mul(0xC2B2_AE35);
    hash as usize & (TSDF_QUERY_CHUNK_CACHE_SLOTS - 1)
}

impl<'a> TsdfQuerySampler<'a> {
    fn new(grid: &'a SparseTsdGridReadSnapshot) -> Self {
        let chunk_edge = grid.chunk_edge.max(1) as usize;
        Self {
            grid,
            tsd_distance_meters: depth_tsd_distance_meters(grid.voxel_size),
            local_y_stride: chunk_edge,
            local_z_stride: chunk_edge * chunk_edge,
            chunk_cache: [None; TSDF_QUERY_CHUNK_CACHE_SLOTS],
        }
    }

    fn world_to_voxel_coord(&self, point: Vec3f) -> TsdfVoxelCoord {
        let (x, y, z) = self.grid.world_to_voxel_xyz(point);
        TsdfVoxelCoord::new(x, y, z)
    }

    fn chunk(&mut self, key: ChunkKey) -> Option<&'a SparseTsdReadChunk> {
        let slot = tsdf_query_chunk_cache_slot(key);
        if let Some(entry) = self.chunk_cache[slot] {
            if entry.key == key {
                return Some(entry.chunk);
            }
        }
        let chunk = self.grid.chunks.get(&key)?.as_ref();
        self.chunk_cache[slot] = Some(TsdfQueryChunkCacheEntry { key, chunk });
        Some(chunk)
    }

    fn voxel_column(&self, x: i32, z: i32) -> TsdfQueryVoxelColumn {
        let (chunk_x, local_x) = self.grid.chunk_axis_and_local(x);
        let (chunk_z, local_z) = self.grid.chunk_axis_and_local(z);
        TsdfQueryVoxelColumn {
            chunk_x,
            chunk_z,
            local_base: local_x + local_z * self.local_z_stride,
        }
    }

    fn normalized_distance_at_y(
        &mut self,
        column: TsdfQueryVoxelColumn,
        y_coord: i32,
    ) -> Option<f32> {
        let (chunk_y, local_y) = self.grid.chunk_axis_and_local(y_coord);
        let local_id = column.local_base + local_y * self.local_y_stride;
        self.chunk(ChunkKey::new(column.chunk_x, chunk_y, column.chunk_z))?
            .value(local_id)
    }

    fn bilinear_column(&self, sample_x: f32, sample_z: f32) -> TsdfQueryBilinearColumn {
        let voxel_size = self.grid.voxel_size;
        let grid_x = sample_x / voxel_size - 0.5;
        let grid_z = sample_z / voxel_size - 0.5;
        let x0 = grid_x.floor() as i32;
        let z0 = grid_z.floor() as i32;
        TsdfQueryBilinearColumn {
            v00: self.voxel_column(x0, z0),
            v10: self.voxel_column(x0 + 1, z0),
            v01: self.voxel_column(x0, z0 + 1),
            v11: self.voxel_column(x0 + 1, z0 + 1),
            tx: grid_x - x0 as f32,
            tz: grid_z - z0 as f32,
        }
    }

    fn bilinear_distance_at_y(
        &mut self,
        column: TsdfQueryBilinearColumn,
        y_coord: i32,
    ) -> Option<f32> {
        let v00 = self.normalized_distance_at_y(column.v00, y_coord)?;
        let v10 = self.normalized_distance_at_y(column.v10, y_coord)?;
        let v01 = self.normalized_distance_at_y(column.v01, y_coord)?;
        let v11 = self.normalized_distance_at_y(column.v11, y_coord)?;

        let vx0 = v00 + (v10 - v00) * column.tx;
        let vx1 = v01 + (v11 - v01) * column.tx;
        Some(vx0 + (vx1 - vx0) * column.tz)
    }

    fn trilinear_distance(&mut self, point: Vec3f) -> Option<f32> {
        let voxel_size = self.grid.voxel_size;
        let grid_y = point.y / voxel_size - 0.5;
        let y0 = grid_y.floor() as i32;
        let ty = grid_y - y0 as f32;
        let column = self.bilinear_column(point.x, point.z);
        let s0 = self.bilinear_distance_at_y(column, y0)? * self.tsd_distance_meters;
        let s1 = self.bilinear_distance_at_y(column, y0 + 1)? * self.tsd_distance_meters;
        Some(s0 + (s1 - s0) * ty)
    }

    fn distance_gradient(&mut self, point: Vec3f) -> Option<Vec3f> {
        let center = self.trilinear_distance(point)?;
        let step = self
            .grid
            .voxel_size
            .max(TSDF_QUERY_TSDF_IMPACT_RAY_STEP_MIN);
        let dx = finite_difference_axis(
            center,
            self.trilinear_distance(point + vec3f(step, 0.0, 0.0)),
            self.trilinear_distance(point - vec3f(step, 0.0, 0.0)),
            step,
        )?;
        let dy = finite_difference_axis(
            center,
            self.trilinear_distance(point + vec3f(0.0, step, 0.0)),
            self.trilinear_distance(point - vec3f(0.0, step, 0.0)),
            step,
        )?;
        let dz = finite_difference_axis(
            center,
            self.trilinear_distance(point + vec3f(0.0, 0.0, step)),
            self.trilinear_distance(point - vec3f(0.0, 0.0, step)),
            step,
        )?;
        let gradient = vec3f(dx, dy, dz);
        (gradient.length() > 1.0e-5).then_some(gradient.normalize())
    }
}

fn voxel_center_axis(voxel_size: f32, coord: i32) -> f32 {
    (coord as f32 + 0.5) * voxel_size
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

fn solve_linear3(mut a: [[f32; 3]; 3], mut b: [f32; 3]) -> Option<[f32; 3]> {
    for pivot in 0..3 {
        let mut best = pivot;
        let mut best_value = a[pivot][pivot].abs();
        for row in (pivot + 1)..3 {
            let candidate = a[row][pivot].abs();
            if candidate > best_value {
                best = row;
                best_value = candidate;
            }
        }
        if best_value <= 1.0e-6 {
            return None;
        }
        if best != pivot {
            a.swap(best, pivot);
            b.swap(best, pivot);
        }
        let inv = a[pivot][pivot].recip();
        for col in pivot..3 {
            a[pivot][col] *= inv;
        }
        b[pivot] *= inv;
        for row in 0..3 {
            if row == pivot {
                continue;
            }
            let factor = a[row][pivot];
            if factor.abs() <= 1.0e-6 {
                continue;
            }
            for col in pivot..3 {
                a[row][col] -= factor * a[pivot][col];
            }
            b[row] -= factor * b[pivot];
        }
    }
    Some(b)
}

fn query_support_plane_fallback_tangent(normal: Vec3f) -> Vec3f {
    let axis = if normal.y.abs() < 0.95 {
        vec3f(0.0, 1.0, 0.0)
    } else {
        vec3f(1.0, 0.0, 0.0)
    };
    Vec3f::cross(axis, normal).normalize()
}

fn query_tsdf_support_radius(query_radius: f32) -> f32 {
    (query_radius * TSDF_QUERY_TSDF_SUPPORT_RADIUS_SCALE).clamp(
        TSDF_QUERY_TSDF_SUPPORT_RADIUS_MIN,
        TSDF_QUERY_TSDF_SUPPORT_RADIUS_MAX,
    )
}

fn query_support_plane_height_tolerance(query_radius: f32, voxel_size: f32) -> f32 {
    (query_radius * 0.45)
        .clamp(0.015, 0.05)
        .max(voxel_size * 0.25)
}

fn query_tsdf_impact_half_extent(query_radius: f32, voxel_size: f32) -> f32 {
    (query_radius * TSDF_QUERY_TSDF_IMPACT_EXTENT_SCALE + voxel_size * 0.5).clamp(
        TSDF_QUERY_TSDF_IMPACT_EXTENT_MIN,
        TSDF_QUERY_TSDF_IMPACT_EXTENT_MAX,
    )
}

pub(crate) fn depth_query_plane_supports_body(
    plane: DepthQuerySupportPlane,
    body_position: Vec3f,
    query_radius: f32,
    lateral_margin: f32,
) -> bool {
    let offset = body_position - plane.point;
    let signed_height = offset.dot(plane.normal);
    if signed_height < -query_radius.max(0.0005) {
        return false;
    }
    if signed_height > query_radius + 0.12 + lateral_margin {
        return false;
    }
    let tangent_limit = plane.half_extent_tangent + lateral_margin;
    let bitangent_limit = plane.half_extent_bitangent + lateral_margin;
    offset.dot(plane.tangent).abs() <= tangent_limit
        && offset.dot(plane.bitangent).abs() <= bitangent_limit
}

pub(crate) fn depth_query_plane_quad(plane: DepthQuerySupportPlane) -> [Vec3f; 4] {
    let tangent = plane.tangent.scale(plane.half_extent_tangent);
    let bitangent = plane.bitangent.scale(plane.half_extent_bitangent);
    [
        plane.point - tangent - bitangent,
        plane.point + tangent - bitangent,
        plane.point + tangent + bitangent,
        plane.point - tangent + bitangent,
    ]
}

pub(crate) fn depth_query_might_need_impact_refresh(query: DepthQuery) -> bool {
    let travel = query.predicted_center - query.center;
    let travel_distance = travel.length();
    let velocity_length = query.velocity.length();
    if velocity_length < TSDF_QUERY_TSDF_IMPACT_MIN_SPEED && travel_distance < 0.03 {
        return false;
    }

    let horizontal_speed = vec2f(query.velocity.x, query.velocity.z).length();
    let upward_speed = query.velocity.y.max(0.0);
    horizontal_speed >= TSDF_QUERY_TSDF_IMPACT_MIN_HORIZONTAL_SPEED
        || upward_speed >= TSDF_QUERY_TSDF_IMPACT_MIN_UPWARD_SPEED
}

fn query_support_plane_fingerprint(
    plane: &DepthQuerySupportPlane,
    role: DepthQueryColliderRole,
) -> u64 {
    let mut hasher = DefaultHasher::new();
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
    plane: DepthQuerySupportPlane,
    role: DepthQueryColliderRole,
    restitution: f32,
) -> DepthQueryCollider {
    DepthQueryCollider {
        fingerprint: query_support_plane_fingerprint(&plane, role),
        geometry: DepthQueryColliderGeometry::HalfSpace(plane),
        role,
        restitution,
    }
}

fn make_query_halfspace_surface(
    distance: f32,
    plane: DepthQuerySupportPlane,
    role: DepthQueryColliderRole,
    restitution: f32,
) -> DepthQueryResolvedSurface {
    let patch = depth_query_plane_quad(plane);
    DepthQueryResolvedSurface {
        surface: DepthQuerySurfaceHit {
            distance,
            point: plane.point,
            normal: plane.normal,
            triangle: [patch[0], patch[1], patch[2]],
            patch,
        },
        collider: make_query_halfspace_collider(plane, role, restitution),
    }
}

fn query_first_support_height(
    sampler: &mut TsdfQuerySampler<'_>,
    sample_x: f32,
    sample_z: f32,
    top_y: f32,
    bottom_y: f32,
) -> Option<f32> {
    let top_coord = sampler
        .world_to_voxel_coord(vec3f(sample_x, top_y, sample_z))
        .y;
    let bottom_coord = sampler
        .world_to_voxel_coord(vec3f(sample_x, bottom_y, sample_z))
        .y;
    if top_coord <= bottom_coord {
        return None;
    }
    let column = sampler.bilinear_column(sample_x, sample_z);
    let mut above = sampler.bilinear_distance_at_y(column, top_coord);

    for y_coord in (bottom_coord + 1..=top_coord).rev() {
        let below = sampler.bilinear_distance_at_y(column, y_coord - 1);
        let (Some(above_distance), Some(below_distance)) = (above, below) else {
            above = below;
            continue;
        };
        if above_distance <= 0.0 || below_distance > 0.0 {
            above = below;
            continue;
        }
        let denom = above_distance - below_distance;
        let blend = if denom.abs() > 1.0e-6 {
            (above_distance / denom).clamp(0.0, 1.0)
        } else {
            0.5
        };
        let y_above = voxel_center_axis(sampler.grid.voxel_size, y_coord);
        let y_below = voxel_center_axis(sampler.grid.voxel_size, y_coord - 1);
        return Some(y_above + (y_below - y_above) * blend);
    }

    None
}

fn query_trajectory_time_seconds(query: DepthQuery) -> Option<f32> {
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

fn query_trajectory_sample(query: DepthQuery, progress: f32) -> QueryTrajectorySample {
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
    query: DepthQuery,
) -> [QueryTrajectorySample; TSDF_QUERY_TRAJECTORY_SAMPLE_COUNT] {
    std::array::from_fn(|index| {
        let progress = if TSDF_QUERY_TRAJECTORY_SAMPLE_COUNT <= 1 {
            0.0
        } else {
            index as f32 / (TSDF_QUERY_TRAJECTORY_SAMPLE_COUNT - 1) as f32
        };
        query_trajectory_sample(query, progress)
    })
}

fn query_trajectory_bounds_and_length(
    samples: &[QueryTrajectorySample; TSDF_QUERY_TRAJECTORY_SAMPLE_COUNT],
) -> (Vec3f, Vec3f, f32) {
    let mut min = samples[0].point;
    let mut max = samples[0].point;
    let mut length = 0.0;
    for window in samples.windows(2) {
        min = Vec3f::min_componentwise(min, window[1].point);
        max = Vec3f::max_componentwise(max, window[1].point);
        length += (window[1].point - window[0].point).length();
    }
    (min, max, length)
}

fn make_query_hit_from_resolved_surface(
    query: DepthQuery,
    surface: DepthQueryResolvedSurface,
) -> DepthQueryHit {
    DepthQueryHit {
        key: query.key,
        distance: surface.surface.distance,
        point: surface.surface.point,
        normal: surface.surface.normal,
        triangle: surface.surface.triangle,
        patch: surface.surface.patch,
        collider: surface.collider,
        additional_hits: Vec::new(),
    }
}

fn query_might_overlap_active_bounds(snapshot: &TsdfPublishedSnapshot, query: DepthQuery) -> bool {
    let Some((bounds_min, bounds_max)) = snapshot.grid.active_bounds else {
        return false;
    };
    let padding =
        query.radius + query.max_distance + depth_tsd_distance_meters(snapshot.grid.voxel_size);
    let padding_vec = vec3f(padding, padding, padding);
    let query_min = Vec3f::min_componentwise(query.center, query.predicted_center) - padding_vec;
    let query_max = Vec3f::max_componentwise(query.center, query.predicted_center) + padding_vec;
    query_max.x >= bounds_min.x
        && query_min.x <= bounds_max.x
        && query_max.y >= bounds_min.y
        && query_min.y <= bounds_max.y
        && query_max.z >= bounds_min.z
        && query_min.z <= bounds_max.z
}

fn evaluate_tsdf_impact_query(
    sampler: &mut TsdfQuerySampler<'_>,
    query: DepthQuery,
) -> Option<DepthQueryResolvedSurface> {
    let grid = sampler.grid;
    if !depth_query_might_need_impact_refresh(query) {
        return None;
    }
    let travel = query.predicted_center - query.center;
    let travel_distance = travel.length();
    let velocity_length = query.velocity.length();
    let upward_speed = query.velocity.y.max(0.0);

    let motion_dir = if velocity_length > 1.0e-4 {
        query.velocity.scale(1.0 / velocity_length)
    } else if travel_distance > 1.0e-4 {
        travel.scale(1.0 / travel_distance)
    } else {
        return None;
    };
    let max_search_distance = (travel_distance + query.radius + query.max_distance)
        .max(query.radius + grid.voxel_size * 0.75);
    let step_distance = (grid.voxel_size * TSDF_QUERY_TSDF_IMPACT_RAY_STEP_SCALE)
        .max(TSDF_QUERY_TSDF_IMPACT_RAY_STEP_MIN)
        .min(max_search_distance);
    let hit_threshold = query.radius + grid.voxel_size * 0.20;
    let mut previous_t = 0.0f32;
    let mut t = step_distance;

    while t <= max_search_distance + 1.0e-4 {
        let sample_position = query.center + motion_dir.scale(t);
        let Some(sample_distance) = sampler.trilinear_distance(sample_position) else {
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
                if let Some(mid_distance) = sampler.trilinear_distance(mid_position) {
                    if mid_distance <= hit_threshold {
                        hi = mid;
                    } else {
                        lo = mid;
                    }
                }
            }

            let hit_position = query.center + motion_dir.scale(hi);
            let signed_distance = sampler.trilinear_distance(hit_position)?;
            let mut normal = sampler.distance_gradient(hit_position)?;
            let mut opposing_dot = normal.dot(motion_dir.scale(-1.0));
            if opposing_dot <= TSDF_QUERY_MIN_OPPOSING_NORMAL_DOT {
                let flipped = normal.scale(-1.0);
                let flipped_opposing_dot = flipped.dot(motion_dir.scale(-1.0));
                if flipped_opposing_dot > TSDF_QUERY_MIN_OPPOSING_NORMAL_DOT {
                    normal = flipped;
                    opposing_dot = flipped_opposing_dot;
                }
            }
            let is_lateral_impact = normal.y.abs() <= TSDF_QUERY_TSDF_IMPACT_NORMAL_Y_MAX;
            let is_ceiling_impact = upward_speed >= TSDF_QUERY_TSDF_IMPACT_MIN_UPWARD_SPEED
                && normal.y <= -TSDF_QUERY_TSDF_IMPACT_CEILING_NORMAL_Y_MIN;
            if !(is_lateral_impact || is_ceiling_impact)
                || opposing_dot <= TSDF_QUERY_MIN_OPPOSING_NORMAL_DOT
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
            let half_extent = query_tsdf_impact_half_extent(query.radius, grid.voxel_size);
            let plane = DepthQuerySupportPlane {
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
                DepthQueryColliderRole::Impact,
                TSDF_QUERY_IMPACT_RESTITUTION,
            ));
        }
        previous_t = t;
        t += step_distance;
    }

    None
}

fn evaluate_tsdf_support_query(
    sampler: &mut TsdfQuerySampler<'_>,
    query: DepthQuery,
    impact_surface: Option<DepthQueryResolvedSurface>,
) -> Option<DepthQueryHit> {
    const GRID_LAST: f32 = (TSDF_QUERY_TSDF_SUPPORT_GRID_DIM - 1) as f32;

    let grid = sampler.grid;
    let trajectory_samples = query_trajectory_samples(query);
    let (trajectory_bounds_min, trajectory_bounds_max, travel_distance) =
        query_trajectory_bounds_and_length(&trajectory_samples);
    let support_radius = query_tsdf_support_radius(query.radius);
    let tsd_distance_meters = depth_tsd_distance_meters(grid.voxel_size);
    let top_y = trajectory_bounds_max.y + query.radius + grid.voxel_size;
    let bottom_y = trajectory_bounds_min.y
        - (query.radius + query.max_distance + travel_distance + tsd_distance_meters);
    let (_, search_sample, center_support_y) = trajectory_samples
        .iter()
        .filter_map(|sample| {
            let support_y = query_first_support_height(
                sampler,
                sample.point.x,
                sample.point.z,
                top_y,
                bottom_y,
            )?;
            let support_point = vec3f(sample.point.x, support_y, sample.point.z);
            let score = (sample.point - support_point).length()
                - sample.progress * (query.radius + grid.voxel_size) * 0.35;
            Some((score, *sample, support_y))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))?;
    let search_center = search_sample.point;

    let mut samples = [None; TSDF_QUERY_TSDF_SUPPORT_MAX_SAMPLES];
    let mut sample_count = 0usize;
    let mut max_height = f32::NEG_INFINITY;

    for row in 0..TSDF_QUERY_TSDF_SUPPORT_GRID_DIM {
        for column in 0..TSDF_QUERY_TSDF_SUPPORT_GRID_DIM {
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
                query_first_support_height(sampler, sample_x, sample_z, top_y, bottom_y)
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

    if sample_count < TSDF_QUERY_TSDF_SUPPORT_MIN_SAMPLES {
        return None;
    }

    let mut height_tolerance = query_support_plane_height_tolerance(query.radius, grid.voxel_size);
    let mut selected_count = 0usize;
    for _ in 0..3 {
        selected_count = samples[..sample_count]
            .iter()
            .filter_map(|sample| *sample)
            .filter(|sample| max_height - sample.point.y <= height_tolerance)
            .count();
        if selected_count >= TSDF_QUERY_TSDF_SUPPORT_MIN_SAMPLES {
            break;
        }
        height_tolerance += grid.voxel_size * 0.35;
    }
    if selected_count < TSDF_QUERY_TSDF_SUPPORT_MIN_SAMPLES {
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
    if normal.y < TSDF_QUERY_TSDF_SUPPORT_NORMAL_Y_MIN {
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
    let extent_padding =
        (query.radius * TSDF_QUERY_TSDF_SUPPORT_EXTENT_PADDING_SCALE).max(grid.voxel_size * 0.45);
    let debug_half_extent_max = support_radius;
    let debug_half_extent_min = (query.radius * 0.9)
        .max(grid.voxel_size * 0.35)
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

    let plane = DepthQuerySupportPlane {
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
        DepthQuerySupportPlane {
            point: support_point,
            ..plane
        },
        DepthQueryColliderRole::Support,
        0.0,
    );
    let additional_hits = impact_surface.into_iter().collect::<Vec<_>>();

    Some(DepthQueryHit {
        key: query.key,
        distance: support_surface.surface.distance,
        point: support_surface.surface.point,
        normal: support_surface.surface.normal,
        triangle: support_surface.surface.triangle,
        patch: support_surface.surface.patch,
        collider: support_surface.collider,
        additional_hits,
    })
}

pub(crate) fn evaluate_tsdf_query(
    snapshot: &TsdfPublishedSnapshot,
    query: DepthQuery,
) -> DepthQueryResult {
    if snapshot.grid.active_value_count == 0 || !query_might_overlap_active_bounds(snapshot, query)
    {
        return DepthQueryResult::Miss { key: query.key };
    }

    let mut sampler = TsdfQuerySampler::new(snapshot.grid.as_ref());
    let impact_surface = evaluate_tsdf_impact_query(&mut sampler, query);
    let prefer_impact = impact_surface.as_ref().is_some_and(|impact_surface| {
        let DepthQueryColliderGeometry::HalfSpace(plane) = &impact_surface.collider.geometry;
        query.velocity.y >= TSDF_QUERY_TSDF_IMPACT_MIN_UPWARD_SPEED
            && plane.normal.y <= -TSDF_QUERY_TSDF_IMPACT_CEILING_NORMAL_Y_MIN
    });
    if prefer_impact {
        return impact_surface
            .map(|impact_surface| {
                DepthQueryResult::Hit(make_query_hit_from_resolved_surface(query, impact_surface))
            })
            .unwrap_or(DepthQueryResult::Miss { key: query.key });
    }

    evaluate_tsdf_support_query(&mut sampler, query, impact_surface)
        .or_else(|| {
            impact_surface
                .map(|impact_surface| make_query_hit_from_resolved_surface(query, impact_surface))
        })
        .map(DepthQueryResult::Hit)
        .unwrap_or(DepthQueryResult::Miss { key: query.key })
}

#[cfg(test)]
include!("../tests/algorithms/tsdf_query.rs");
