use crate::{
    makepad_math::{vec3, vec4f, Mat4f, Pose, Quat, Vec3f},
    makepad_micro_serde::*,
};
use std::{
    collections::{HashMap, VecDeque},
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        Arc, Mutex, OnceLock, RwLock,
    },
};

const XR_DEPTH_QUERY_MAX_PENDING: usize = 256;
pub const XR_DEPTH_MESH_DEFAULT_VOXEL_SIZE_METERS: f32 = 0.05;
const XR_DEPTH_ALIGN_MIN_WALL_SAMPLES: usize = 4;
const XR_DEPTH_ALIGN_MIN_WALL_FEATURES: usize = 2;
const XR_DEPTH_ALIGN_ACCEPT_MIN_MATCHED_SAMPLES: usize = 6;
const XR_DEPTH_ALIGN_ACCEPT_MIN_CONFIDENCE: f32 = 0.12;
const XR_DEPTH_ALIGN_ACCEPT_MIN_MATCHED_WALL_FEATURES: usize = 3;
const XR_DEPTH_ALIGN_ACCEPT_MIN_WALL_FEATURE_CONFIDENCE: f32 = 0.18;
const XR_DEPTH_ALIGN_TRANSLATION_VOTE_STEP_METERS: f32 = 0.08;
const XR_DEPTH_ALIGN_WALL_FEATURE_NORMAL_DOT_MIN: f32 = 0.94;
const XR_DEPTH_ALIGN_WALL_FEATURE_PLANE_RESIDUAL_MAX_METERS: f32 = 0.18;
const XR_DEPTH_ALIGN_WALL_FEATURE_HEIGHT_OVERLAP_MIN: f32 = 0.30;
const XR_DEPTH_ALIGN_WALL_FEATURE_PAIR_ANGLE_MIN_RADIANS: f32 = 0.28;

#[derive(Clone, Copy, Debug, Default, PartialEq, SerBin, DeBin)]
pub struct XrDepthAlignWallFeature {
    pub center: Vec3f,
    pub normal: Vec3f,
    pub along_axis: Vec3f,
    pub plane_distance: f32,
    pub half_extent_along: f32,
    pub min_y: f32,
    pub max_y: f32,
    pub area: f32,
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

#[derive(Clone, Debug, Default)]
pub struct XrDepthMeshChunk {
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

impl XrDepthMeshChunk {
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }
}

#[derive(Clone, Debug, Default)]
pub struct XrDepthMesh {
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
    pub alignment_debug: XrDepthAlignDebug,
    pub alignment_preview: XrDepthAlignPreview,
    pub dirty_chunk_keys: Vec<ChunkKey>,
    pub removed_chunk_keys: Vec<ChunkKey>,
    pub mesh_generation: u64,
    pub mesh_vertex_count: usize,
    pub mesh_triangle_count: usize,
}

#[derive(Clone, Debug, Default)]
pub struct XrDepthMeshState {
    pub latest_mesh: Option<Arc<XrDepthMesh>>,
    pub stats: XrDepthMeshStats,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct XrDepthAlignState {
    pub update_sequence: u64,
    pub descriptor: Option<XrDepthAlignDescriptor>,
    pub debug: XrDepthAlignDebug,
    pub preview: XrDepthAlignPreview,
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
pub struct XrDepthAlignDescriptor {
    pub voxel_size_meters: f32,
    pub floor_y: f32,
    pub wall_normal_histogram: Vec<f32>,
    pub wall_features: Vec<XrDepthAlignWallFeature>,
    pub samples: Vec<XrDepthAlignSample>,
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
    pub used_wall_features: bool,
    pub local_wall_features: usize,
    pub remote_wall_features: usize,
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
        self.best_solution.filter(|solution| {
            if self.used_wall_features {
                solution.matched_samples >= XR_DEPTH_ALIGN_ACCEPT_MIN_MATCHED_WALL_FEATURES
                    && solution.confidence > XR_DEPTH_ALIGN_ACCEPT_MIN_WALL_FEATURE_CONFIDENCE
            } else {
                solution.matched_samples >= XR_DEPTH_ALIGN_ACCEPT_MIN_MATCHED_SAMPLES
                    && solution.confidence > XR_DEPTH_ALIGN_ACCEPT_MIN_CONFIDENCE
            }
        })
    }

    pub fn outcome(&self) -> XrDepthAlignSolveOutcome {
        if self.used_wall_features {
            if self.local_wall_features < XR_DEPTH_ALIGN_MIN_WALL_FEATURES
                || self.remote_wall_features < XR_DEPTH_ALIGN_MIN_WALL_FEATURES
            {
                return XrDepthAlignSolveOutcome::MissingSamples;
            }
        } else if self.local_wall_samples < XR_DEPTH_ALIGN_MIN_WALL_SAMPLES
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

pub fn xr_depth_align_loopback_preview_solution() -> XrDepthAlignSolution {
    XrDepthAlignSolution {
        yaw_radians: 0.58,
        translation: vec3(-0.82, 0.0, 0.67),
        confidence: 1.0,
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
    for wall in &mut descriptor.wall_features {
        let center = transform
            .transform_vec4(vec4f(wall.center.x, wall.center.y, wall.center.z, 1.0))
            .to_vec3f();
        let along_start = transform
            .transform_vec4(vec4f(
                wall.center.x - wall.along_axis.x * wall.half_extent_along,
                wall.center.y - 0.0,
                wall.center.z - wall.along_axis.z * wall.half_extent_along,
                1.0,
            ))
            .to_vec3f();
        let along_end = transform
            .transform_vec4(vec4f(
                wall.center.x + wall.along_axis.x * wall.half_extent_along,
                wall.center.y + 0.0,
                wall.center.z + wall.along_axis.z * wall.half_extent_along,
                1.0,
            ))
            .to_vec3f();
        let bottom = transform
            .transform_vec4(vec4f(wall.center.x, wall.min_y, wall.center.z, 1.0))
            .to_vec3f();
        let top = transform
            .transform_vec4(vec4f(wall.center.x, wall.max_y, wall.center.z, 1.0))
            .to_vec3f();
        wall.center = center;
        wall.normal = align_safe_normalize(transform_dir(wall.normal)).unwrap_or(wall.normal);
        wall.along_axis = align_safe_normalize(along_end - along_start).unwrap_or_else(|| {
            align_safe_normalize(transform_dir(wall.along_axis)).unwrap_or(wall.along_axis)
        });
        wall.half_extent_along = 0.5 * (along_end - along_start).length();
        wall.min_y = bottom.y.min(top.y);
        wall.max_y = bottom.y.max(top.y);
        wall.plane_distance = wall.center.dot(wall.normal);
    }
    descriptor.floor_y = transform
        .transform_vec4(vec4f(0.0, descriptor.floor_y, 0.0, 1.0))
        .to_vec3f()
        .y;
    descriptor.wall_normal_histogram = if descriptor.wall_features.is_empty() {
        xr_depth_align_build_wall_normal_histogram(
            &descriptor.samples,
            descriptor.wall_normal_histogram.len(),
        )
    } else {
        xr_depth_align_build_wall_feature_normal_histogram(
            &descriptor.wall_features,
            descriptor.wall_normal_histogram.len(),
        )
    };
    descriptor
}

pub fn xr_depth_align_test_markers(descriptor: &XrDepthAlignDescriptor) -> Option<[Vec3f; 2]> {
    let wall_samples = descriptor_samples_of_kind(descriptor, XrDepthAlignSampleKind::Wall);
    let samples = if wall_samples.len() >= 2 {
        wall_samples
    } else {
        descriptor
            .samples
            .iter()
            .filter(|sample| sample.kind != XrDepthAlignSampleKind::Unknown)
            .collect::<Vec<_>>()
    };
    let mut best = None::<(f32, f32, Vec3f, Vec3f)>;
    for (index, first) in samples.iter().enumerate() {
        for second in samples.iter().skip(index + 1) {
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
    let local_wall_features = descriptor_wall_features(local);
    let remote_wall_features = descriptor_wall_features(remote);
    let local_floor_samples = descriptor_samples_of_kind(local, XrDepthAlignSampleKind::Floor);
    let local_wall_samples = descriptor_samples_of_kind(local, XrDepthAlignSampleKind::Wall);
    let remote_floor_samples = descriptor_samples_of_kind(remote, XrDepthAlignSampleKind::Floor);
    let remote_wall_samples = descriptor_samples_of_kind(remote, XrDepthAlignSampleKind::Wall);
    let mut diagnostic = XrDepthAlignSolveDiagnostic {
        local_wall_features: local_wall_features.len(),
        remote_wall_features: remote_wall_features.len(),
        local_floor_samples: local_floor_samples.len(),
        local_wall_samples: local_wall_samples.len(),
        remote_floor_samples: remote_floor_samples.len(),
        remote_wall_samples: remote_wall_samples.len(),
        ..XrDepthAlignSolveDiagnostic::default()
    };
    if local_wall_features.len() >= XR_DEPTH_ALIGN_MIN_WALL_FEATURES
        && remote_wall_features.len() >= XR_DEPTH_ALIGN_MIN_WALL_FEATURES
    {
        diagnostic.used_wall_features = true;
        let mut best = None::<XrDepthAlignSolution>;
        let floor_y = local.floor_y - remote.floor_y;
        let yaw_candidates = candidate_wall_feature_yaws(
            &local.wall_normal_histogram,
            &remote.wall_normal_histogram,
            &local_wall_features,
            &remote_wall_features,
        );
        diagnostic.yaw_candidate_count = yaw_candidates.len();
        for yaw in yaw_candidates {
            let translations = candidate_wall_feature_translations(
                &local_wall_features,
                &remote_wall_features,
                floor_y,
                yaw,
            );
            diagnostic.pose_candidate_count += translations.len();
            for translation in translations {
                let (refined_yaw, refined_translation) = refine_wall_feature_alignment(
                    &local_wall_features,
                    &remote_wall_features,
                    yaw,
                    translation,
                );
                let candidate = score_wall_feature_alignment(
                    &local_wall_features,
                    &remote_wall_features,
                    &local_wall_samples,
                    &remote_wall_samples,
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
        diagnostic.best_solution = best;
        return diagnostic;
    }

    if local_wall_samples.len() < XR_DEPTH_ALIGN_MIN_WALL_SAMPLES
        || remote_wall_samples.len() < XR_DEPTH_ALIGN_MIN_WALL_SAMPLES
    {
        return diagnostic;
    }

    let floor_y = local.floor_y - remote.floor_y;
    let mut best = None::<XrDepthAlignSolution>;
    let yaw_candidates = candidate_yaws(
        &local.wall_normal_histogram,
        &remote.wall_normal_histogram,
        &local_wall_samples,
        &remote_wall_samples,
    );
    diagnostic.yaw_candidate_count = yaw_candidates.len();
    for yaw in yaw_candidates {
        let translations =
            candidate_translations(&local_wall_samples, &remote_wall_samples, floor_y, yaw);
        diagnostic.pose_candidate_count += translations.len();
        for translation in translations {
            let (refined_yaw, refined_translation) = refine_alignment(
                &local_wall_samples,
                &remote_wall_samples,
                floor_y,
                yaw,
                translation,
            );
            let candidate = score_alignment_solution(
                &local_wall_samples,
                &remote_wall_samples,
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
    diagnostic.best_solution = best;
    diagnostic
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

pub fn xr_depth_align_build_wall_feature_normal_histogram(
    features: &[XrDepthAlignWallFeature],
    bin_count: usize,
) -> Vec<f32> {
    let bin_count = bin_count.max(1);
    let mut histogram = vec![0.0; bin_count];
    for feature in features {
        let Some(axis) = xz_axis(feature.normal) else {
            continue;
        };
        let angle = axis.x.atan2(-axis.z);
        let normalized = (angle + std::f32::consts::PI) / std::f32::consts::TAU;
        let bin = (normalized * bin_count as f32).floor() as isize;
        histogram[bin.rem_euclid(bin_count as isize) as usize] += wall_feature_weight(feature);
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

fn descriptor_wall_features<'a>(
    descriptor: &'a XrDepthAlignDescriptor,
) -> Vec<&'a XrDepthAlignWallFeature> {
    let mut walls = descriptor.wall_features.iter().collect::<Vec<_>>();
    walls.sort_by(|a, b| b.area.total_cmp(&a.area));
    walls
}

fn wall_feature_height(feature: &XrDepthAlignWallFeature) -> f32 {
    (feature.max_y - feature.min_y).max(0.0)
}

fn wall_feature_width(feature: &XrDepthAlignWallFeature) -> f32 {
    (feature.half_extent_along * 2.0).max(0.0)
}

fn wall_feature_weight(feature: &XrDepthAlignWallFeature) -> f32 {
    feature.area.max(0.05)
}

fn interval_overlap_ratio(first_min: f32, first_max: f32, second_min: f32, second_max: f32) -> f32 {
    let overlap = (first_max.min(second_max) - first_min.max(second_min)).max(0.0);
    let union = (first_max.max(second_max) - first_min.min(second_min)).max(1.0e-3);
    (overlap / union).clamp(0.0, 1.0)
}

fn wall_feature_interval_on_axis(
    center: Vec3f,
    along_axis: Vec3f,
    half_extent_along: f32,
    axis: Vec3f,
) -> (f32, f32) {
    let left = (center - along_axis.scale(half_extent_along)).dot(axis);
    let right = (center + along_axis.scale(half_extent_along)).dot(axis);
    if left <= right {
        (left, right)
    } else {
        (right, left)
    }
}

#[derive(Clone, Copy)]
struct WallFeaturePairCandidate<'a> {
    local_index: usize,
    remote_index: usize,
    local: &'a XrDepthAlignWallFeature,
    remote: &'a XrDepthAlignWallFeature,
    score_hint: f32,
}

#[derive(Clone, Copy)]
struct WallFeatureMatch<'a> {
    local_index: usize,
    remote_index: usize,
    local: &'a XrDepthAlignWallFeature,
    remote: &'a XrDepthAlignWallFeature,
    transformed_remote_center: Vec3f,
    rotated_remote_normal: Vec3f,
    rotated_remote_axis: Vec3f,
    transformed_remote_plane_distance: f32,
    plane_residual: f32,
    alignment: f32,
    height_overlap: f32,
    horizontal_overlap: f32,
    width_ratio: f32,
    score: f32,
}

fn candidate_wall_feature_yaws(
    local_histogram: &[f32],
    remote_histogram: &[f32],
    local_walls: &[&XrDepthAlignWallFeature],
    remote_walls: &[&XrDepthAlignWallFeature],
) -> Vec<f32> {
    let mut candidates = vec![0.0, std::f32::consts::PI];
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
            let angle = wrap_angle(shift as f32 * std::f32::consts::TAU / bins as f32);
            candidates.push(angle);
            candidates.push(wrap_angle(angle + std::f32::consts::PI));
        }
    }

    for local_wall in local_walls.iter().take(12) {
        let Some(local_axis) = xz_axis(local_wall.normal) else {
            continue;
        };
        for remote_wall in remote_walls.iter().take(12) {
            let Some(remote_axis) = xz_axis(remote_wall.normal) else {
                continue;
            };
            let angle = wrap_angle(signed_xz_angle(remote_axis, local_axis));
            candidates.push(angle);
            candidates.push(wrap_angle(angle + std::f32::consts::PI));
        }
    }
    dedupe_angles(candidates, 0.05)
}

fn collect_wall_feature_pair_candidates<'a>(
    local_walls: &[&'a XrDepthAlignWallFeature],
    remote_walls: &[&'a XrDepthAlignWallFeature],
    yaw: f32,
) -> Vec<WallFeaturePairCandidate<'a>> {
    let mut candidates = Vec::new();
    for (local_index, local) in local_walls.iter().enumerate() {
        for (remote_index, remote) in remote_walls.iter().enumerate() {
            let rotated_remote_normal =
                align_safe_normalize(rotate_y(yaw, remote.normal)).unwrap_or(remote.normal);
            let alignment = local.normal.dot(rotated_remote_normal);
            if alignment < XR_DEPTH_ALIGN_WALL_FEATURE_NORMAL_DOT_MIN {
                continue;
            }
            let height_overlap =
                interval_overlap_ratio(local.min_y, local.max_y, remote.min_y, remote.max_y);
            if height_overlap < XR_DEPTH_ALIGN_WALL_FEATURE_HEIGHT_OVERLAP_MIN {
                continue;
            }
            let height_ratio = (wall_feature_height(local).min(wall_feature_height(remote))
                / wall_feature_height(local)
                    .max(wall_feature_height(remote))
                    .max(0.05))
            .clamp(0.0, 1.0);
            let width_ratio = (wall_feature_width(local).min(wall_feature_width(remote))
                / wall_feature_width(local)
                    .max(wall_feature_width(remote))
                    .max(0.05))
            .clamp(0.0, 1.0);
            let score_hint = (wall_feature_weight(local) * wall_feature_weight(remote)).sqrt()
                * alignment.powf(2.0)
                * (0.35 + 0.65 * height_overlap)
                * (0.40 + 0.60 * height_ratio)
                * (0.30 + 0.70 * width_ratio);
            candidates.push(WallFeaturePairCandidate {
                local_index,
                remote_index,
                local,
                remote,
                score_hint,
            });
        }
    }
    candidates.sort_by(|a, b| b.score_hint.total_cmp(&a.score_hint));
    candidates
}

fn solve_translation_from_wall_constraints<'a, I>(constraints: I, floor_y: f32) -> Option<Vec3f>
where
    I: IntoIterator<Item = (Vec3f, f32, f32)>,
{
    let mut a00 = 0.0;
    let mut a01 = 0.0;
    let mut a11 = 0.0;
    let mut b0 = 0.0;
    let mut b1 = 0.0;
    for (normal, rhs, weight) in constraints {
        let Some(axis) = xz_axis(normal) else {
            continue;
        };
        let weight = weight.max(0.001);
        a00 += weight * axis.x * axis.x;
        a01 += weight * axis.x * axis.z;
        a11 += weight * axis.z * axis.z;
        b0 += weight * axis.x * rhs;
        b1 += weight * axis.z * rhs;
    }
    let det = a00 * a11 - a01 * a01;
    if det.abs() <= 1.0e-4 {
        return None;
    }
    let inv_det = det.recip();
    let tx = (b0 * a11 - b1 * a01) * inv_det;
    let tz = (a00 * b1 - a01 * b0) * inv_det;
    Some(vec3(tx, floor_y, tz))
}

fn candidate_wall_feature_translations(
    local_walls: &[&XrDepthAlignWallFeature],
    remote_walls: &[&XrDepthAlignWallFeature],
    floor_y: f32,
    yaw: f32,
) -> Vec<Vec3f> {
    let pair_candidates = collect_wall_feature_pair_candidates(local_walls, remote_walls, yaw);
    if pair_candidates.is_empty() {
        return Vec::new();
    }

    let mut translations = Vec::<Vec3f>::new();
    let max_candidates = pair_candidates.len().min(18);
    for first_index in 0..max_candidates {
        let first = pair_candidates[first_index];
        for second in pair_candidates
            .iter()
            .take(max_candidates)
            .skip(first_index + 1)
        {
            if first.local_index == second.local_index || first.remote_index == second.remote_index
            {
                continue;
            }
            let angle_cos = first.local.normal.dot(second.local.normal).abs();
            if angle_cos > XR_DEPTH_ALIGN_WALL_FEATURE_PAIR_ANGLE_MIN_RADIANS.cos() {
                continue;
            }
            let Some(translation) = solve_translation_from_wall_constraints(
                [
                    (
                        first.local.normal,
                        first.local.plane_distance - first.remote.plane_distance,
                        first.score_hint,
                    ),
                    (
                        second.local.normal,
                        second.local.plane_distance - second.remote.plane_distance,
                        second.score_hint,
                    ),
                ],
                floor_y,
            ) else {
                continue;
            };
            translations.push(translation);
        }
    }

    if translations.is_empty() {
        let lsq = solve_translation_from_wall_constraints(
            pair_candidates
                .iter()
                .take(max_candidates)
                .map(|candidate| {
                    (
                        candidate.local.normal,
                        candidate.local.plane_distance - candidate.remote.plane_distance,
                        candidate.score_hint,
                    )
                }),
            floor_y,
        );
        if let Some(translation) = lsq {
            translations.push(translation);
        }
    }

    dedupe_translations(translations, 0.05)
}

fn collect_unique_wall_feature_matches<'a>(
    local_walls: &[&'a XrDepthAlignWallFeature],
    remote_walls: &[&'a XrDepthAlignWallFeature],
    yaw: f32,
    translation: Vec3f,
) -> Vec<WallFeatureMatch<'a>> {
    let mut candidates = Vec::<WallFeatureMatch<'a>>::new();
    for (remote_index, remote) in remote_walls.iter().enumerate() {
        let transformed_remote_center = rotate_y(yaw, remote.center) + translation;
        let rotated_remote_normal =
            align_safe_normalize(rotate_y(yaw, remote.normal)).unwrap_or(remote.normal);
        let rotated_remote_axis =
            align_safe_normalize(rotate_y(yaw, remote.along_axis)).unwrap_or(remote.along_axis);
        let transformed_remote_plane_distance =
            rotated_remote_normal.dot(transformed_remote_center);
        let transformed_remote_min_y = remote.min_y + translation.y;
        let transformed_remote_max_y = remote.max_y + translation.y;
        for (local_index, local) in local_walls.iter().enumerate() {
            let alignment = local.normal.dot(rotated_remote_normal);
            if alignment < XR_DEPTH_ALIGN_WALL_FEATURE_NORMAL_DOT_MIN {
                continue;
            }
            let plane_residual =
                (local.normal.dot(transformed_remote_center) - local.plane_distance).abs();
            if plane_residual > XR_DEPTH_ALIGN_WALL_FEATURE_PLANE_RESIDUAL_MAX_METERS {
                continue;
            }
            let height_overlap = interval_overlap_ratio(
                local.min_y,
                local.max_y,
                transformed_remote_min_y,
                transformed_remote_max_y,
            );
            if height_overlap < XR_DEPTH_ALIGN_WALL_FEATURE_HEIGHT_OVERLAP_MIN {
                continue;
            }
            let (local_min_u, local_max_u) = wall_feature_interval_on_axis(
                local.center,
                local.along_axis,
                local.half_extent_along,
                local.along_axis,
            );
            let (remote_min_u, remote_max_u) = wall_feature_interval_on_axis(
                transformed_remote_center,
                rotated_remote_axis,
                remote.half_extent_along,
                local.along_axis,
            );
            let horizontal_overlap =
                interval_overlap_ratio(local_min_u, local_max_u, remote_min_u, remote_max_u);
            let width_ratio = (wall_feature_width(local).min(wall_feature_width(remote))
                / wall_feature_width(local)
                    .max(wall_feature_width(remote))
                    .max(0.05))
            .clamp(0.0, 1.0);
            let score = (wall_feature_weight(local) * wall_feature_weight(remote)).sqrt()
                * alignment.powf(2.0)
                * (-plane_residual / 0.08).exp()
                * (0.35 + 0.65 * height_overlap)
                * (0.55 + 0.45 * horizontal_overlap)
                * (0.35 + 0.65 * width_ratio);
            candidates.push(WallFeatureMatch {
                local_index,
                remote_index,
                local,
                remote,
                transformed_remote_center,
                rotated_remote_normal,
                rotated_remote_axis,
                transformed_remote_plane_distance,
                plane_residual,
                alignment,
                height_overlap,
                horizontal_overlap,
                width_ratio,
                score,
            });
        }
    }

    candidates.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.plane_residual.total_cmp(&b.plane_residual))
    });

    let mut used_local = vec![false; local_walls.len()];
    let mut used_remote = vec![false; remote_walls.len()];
    let mut matches = Vec::<WallFeatureMatch<'a>>::new();
    for candidate in candidates {
        if used_local[candidate.local_index] || used_remote[candidate.remote_index] {
            continue;
        }
        used_local[candidate.local_index] = true;
        used_remote[candidate.remote_index] = true;
        matches.push(candidate);
    }
    matches
}

fn refine_wall_feature_alignment(
    local_walls: &[&XrDepthAlignWallFeature],
    remote_walls: &[&XrDepthAlignWallFeature],
    yaw: f32,
    translation: Vec3f,
) -> (f32, Vec3f) {
    let mut refined_yaw = yaw;
    let mut refined_translation = translation;
    for _ in 0..3 {
        let matches = collect_unique_wall_feature_matches(
            local_walls,
            remote_walls,
            refined_yaw,
            refined_translation,
        );
        if matches.is_empty() {
            break;
        }

        if let Some(next_translation) = solve_translation_from_wall_constraints(
            matches.iter().map(|matched| {
                (
                    matched.local.normal,
                    matched.local.plane_distance - matched.remote.plane_distance,
                    matched.score,
                )
            }),
            refined_translation.y,
        ) {
            refined_translation = next_translation;
        }

        let mut yaw_sin = 0.0;
        let mut yaw_cos = 0.0;
        let mut yaw_weight_sum = 0.0;
        for matched in &matches {
            let Some(local_axis) = xz_axis(matched.local.normal) else {
                continue;
            };
            let Some(remote_axis) = xz_axis(matched.rotated_remote_normal) else {
                continue;
            };
            let delta_yaw = wrap_angle(signed_xz_angle(remote_axis, local_axis));
            let weight = matched.score.max(0.001);
            yaw_sin += delta_yaw.sin() * weight;
            yaw_cos += delta_yaw.cos() * weight;
            yaw_weight_sum += weight;
        }
        if yaw_weight_sum > 0.0 {
            refined_yaw = wrap_angle(refined_yaw + yaw_sin.atan2(yaw_cos));
        }
    }
    (refined_yaw, refined_translation)
}

fn score_wall_feature_alignment(
    local_walls: &[&XrDepthAlignWallFeature],
    remote_walls: &[&XrDepthAlignWallFeature],
    local_wall_samples: &[&XrDepthAlignSample],
    remote_wall_samples: &[&XrDepthAlignSample],
    yaw: f32,
    translation: Vec3f,
) -> XrDepthAlignSolution {
    let matches = collect_unique_wall_feature_matches(local_walls, remote_walls, yaw, translation);
    let total_weight = remote_walls
        .iter()
        .map(|feature| wall_feature_weight(feature))
        .sum::<f32>()
        .max(0.01);
    let mut matched_weight = 0.0;
    let mut residual_sum = 0.0;
    let mut alignment_sum = 0.0;
    let mut overlap_sum = 0.0;
    for matched in &matches {
        let weight = wall_feature_weight(matched.remote);
        matched_weight += weight;
        residual_sum += matched.plane_residual * weight;
        alignment_sum += matched.alignment * weight;
        overlap_sum += (matched.height_overlap * matched.horizontal_overlap * matched.width_ratio)
            .sqrt()
            * weight;
    }
    let residual_meters = if matched_weight > 0.0 {
        residual_sum / matched_weight
    } else {
        f32::INFINITY
    };
    let coverage = (matched_weight / total_weight).clamp(0.0, 1.0);
    let mean_alignment = if matched_weight > 0.0 {
        alignment_sum / matched_weight
    } else {
        0.0
    };
    let overlap_quality = if matched_weight > 0.0 {
        overlap_sum / matched_weight
    } else {
        0.0
    };
    let corner_consistency = wall_feature_corner_consistency(&matches);
    let residual_confidence = if residual_meters.is_finite() {
        (1.0 - (residual_meters / XR_DEPTH_ALIGN_WALL_FEATURE_PLANE_RESIDUAL_MAX_METERS))
            .clamp(0.0, 1.0)
    } else {
        0.0
    };
    let mut confidence = (coverage.sqrt()
        * mean_alignment.powf(1.4).clamp(0.0, 1.0)
        * (0.25 + 0.75 * overlap_quality)
        * (0.70 + 0.30 * corner_consistency)
        * residual_confidence.max(0.05))
    .clamp(0.0, 1.0);
    let mut blended_residual = residual_meters;
    if !local_wall_samples.is_empty() && !remote_wall_samples.is_empty() {
        let sample_solution =
            score_alignment_solution(local_wall_samples, remote_wall_samples, yaw, translation);
        let sample_support = match sample_solution.matched_samples {
            matches if matches >= 3 => 0.35 + 0.65 * sample_solution.confidence,
            2 => 0.28 + 0.72 * sample_solution.confidence,
            1 => 0.55,
            _ => 0.22,
        };
        confidence = (confidence * sample_support.clamp(0.0, 1.0)).clamp(0.0, 1.0);
        if sample_solution.residual_meters.is_finite() {
            blended_residual = if blended_residual.is_finite() {
                blended_residual * 0.78 + sample_solution.residual_meters * 0.22
            } else {
                sample_solution.residual_meters
            };
        }
    }

    XrDepthAlignSolution {
        yaw_radians: wrap_angle(yaw),
        translation,
        confidence,
        residual_meters: blended_residual,
        matched_samples: matches.len(),
    }
}

fn wall_feature_signed_offset(
    center: Vec3f,
    along_axis: Vec3f,
    half_extent_along: f32,
    point: Vec3f,
) -> f32 {
    (point - center).dot(along_axis) / half_extent_along.max(0.05)
}

fn wall_feature_corner_consistency(matches: &[WallFeatureMatch<'_>]) -> f32 {
    if matches.len() < 2 {
        return 0.5;
    }
    let mut score_sum = 0.0;
    let mut weight_sum = 0.0;
    for (index, first) in matches.iter().enumerate() {
        for second in matches.iter().skip(index + 1) {
            let angle_cos = first.local.normal.dot(second.local.normal).abs();
            if angle_cos > XR_DEPTH_ALIGN_WALL_FEATURE_PAIR_ANGLE_MIN_RADIANS.cos() {
                continue;
            }
            let Some(local_corner) = solve_translation_from_wall_constraints(
                [
                    (first.local.normal, first.local.plane_distance, 1.0),
                    (second.local.normal, second.local.plane_distance, 1.0),
                ],
                0.0,
            ) else {
                continue;
            };
            let Some(remote_corner) = solve_translation_from_wall_constraints(
                [
                    (
                        first.rotated_remote_normal,
                        first.transformed_remote_plane_distance,
                        1.0,
                    ),
                    (
                        second.rotated_remote_normal,
                        second.transformed_remote_plane_distance,
                        1.0,
                    ),
                ],
                0.0,
            ) else {
                continue;
            };
            let local_first_offset = wall_feature_signed_offset(
                first.local.center,
                first.local.along_axis,
                first.local.half_extent_along,
                local_corner,
            );
            let local_second_offset = wall_feature_signed_offset(
                second.local.center,
                second.local.along_axis,
                second.local.half_extent_along,
                local_corner,
            );
            let remote_first_offset = wall_feature_signed_offset(
                first.transformed_remote_center,
                first.rotated_remote_axis,
                first.remote.half_extent_along,
                remote_corner,
            );
            let remote_second_offset = wall_feature_signed_offset(
                second.transformed_remote_center,
                second.rotated_remote_axis,
                second.remote.half_extent_along,
                remote_corner,
            );
            let sign_score_first =
                wall_feature_signed_offset_consistency(local_first_offset, remote_first_offset);
            let sign_score_second =
                wall_feature_signed_offset_consistency(local_second_offset, remote_second_offset);
            let magnitude_score = (1.0
                - ((local_first_offset.abs() - remote_first_offset.abs()).abs()
                    + (local_second_offset.abs() - remote_second_offset.abs()).abs())
                    / 4.0)
                .clamp(0.0, 1.0);
            let local_coverage = (1.0
                - ((local_first_offset.abs() - 1.0).max(0.0)
                    + (local_second_offset.abs() - 1.0).max(0.0))
                    * 0.5)
                .clamp(0.0, 1.0);
            let remote_coverage = (1.0
                - ((remote_first_offset.abs() - 1.0).max(0.0)
                    + (remote_second_offset.abs() - 1.0).max(0.0))
                    * 0.5)
                .clamp(0.0, 1.0);
            if local_coverage < 0.25 || remote_coverage < 0.25 {
                continue;
            }
            let pair_weight =
                (wall_feature_weight(first.remote) * wall_feature_weight(second.remote)).sqrt();
            let pair_score = sign_score_first
                * sign_score_second
                * (0.75 + 0.25 * magnitude_score)
                * (local_coverage * remote_coverage).sqrt();
            score_sum += pair_score * pair_weight;
            weight_sum += pair_weight;
        }
    }
    if weight_sum > 0.0 {
        (score_sum / weight_sum).clamp(0.0, 1.0)
    } else {
        0.5
    }
}

fn wall_feature_signed_offset_consistency(local: f32, remote: f32) -> f32 {
    if local.abs() <= 0.20 || remote.abs() <= 0.20 {
        1.0
    } else if local.signum() == remote.signum() {
        1.0
    } else {
        0.05
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

fn descriptor_samples_of_kind<'a>(
    descriptor: &'a XrDepthAlignDescriptor,
    kind: XrDepthAlignSampleKind,
) -> Vec<&'a XrDepthAlignSample> {
    let mut samples = descriptor
        .samples
        .iter()
        .filter(|sample| sample.kind == kind)
        .collect::<Vec<_>>();
    samples.sort_by(|a, b| b.weight.total_cmp(&a.weight));
    samples
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
        .confidence
        .partial_cmp(&current.confidence)
        .unwrap_or(std::cmp::Ordering::Equal)
        .then_with(|| candidate.matched_samples.cmp(&current.matched_samples))
        .then_with(|| {
            current
                .residual_meters
                .partial_cmp(&candidate.residual_meters)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .is_gt()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum XrDepthPlaneKind {
    Floor,
    Table,
    Ceiling,
    Wall,
    #[default]
    Unknown,
}

#[derive(Clone, Debug, Default)]
pub struct XrDepthPlanePatch {
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

#[derive(Clone, Copy, Debug)]
pub struct XrDepthMeshQuery {
    pub key: u64,
    pub center: Vec3f,
    pub predicted_center: Vec3f,
    pub velocity: Vec3f,
    pub radius: f32,
    pub max_distance: f32,
    pub include_planar_patches: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct XrDepthMeshQuerySurfaceHit {
    pub distance: f32,
    pub point: Vec3f,
    pub normal: Vec3f,
    pub from_planar_patch: bool,
    pub triangle: [Vec3f; 3],
    pub patch: [Vec3f; 4],
    pub chunk_key: ChunkKey,
}

#[derive(Clone, Copy, Debug)]
pub struct XrDepthMeshQuerySupportPlane {
    pub point: Vec3f,
    pub normal: Vec3f,
    pub tangent: Vec3f,
    pub bitangent: Vec3f,
    pub half_extent_tangent: f32,
    pub half_extent_bitangent: f32,
}

#[derive(Clone, Debug)]
pub enum XrDepthMeshQueryColliderGeometry {
    HalfSpace(XrDepthMeshQuerySupportPlane),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum XrDepthMeshQueryColliderRole {
    Support,
    Impact,
}

#[derive(Clone, Debug)]
pub struct XrDepthMeshQueryCollider {
    pub fingerprint: u64,
    pub geometry: XrDepthMeshQueryColliderGeometry,
    pub role: XrDepthMeshQueryColliderRole,
    pub restitution: f32,
}

impl XrDepthMeshQueryCollider {
    pub fn vertex_count(&self) -> usize {
        0
    }

    pub fn triangle_count(&self) -> usize {
        0
    }
}

#[derive(Clone, Debug)]
pub struct XrDepthMeshQueryResolvedSurface {
    pub surface: XrDepthMeshQuerySurfaceHit,
    pub collider: XrDepthMeshQueryCollider,
}

#[derive(Clone, Debug)]
pub struct XrDepthMeshQueryHit {
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

#[derive(Clone, Debug)]
pub enum XrDepthMeshQueryResult {
    Hit(XrDepthMeshQueryHit),
    Miss {
        key: u64,
        version: u64,
        mesh_generation: u64,
    },
}

impl XrDepthMeshQueryResult {
    pub fn key(&self) -> u64 {
        match self {
            Self::Hit(hit) => hit.key,
            Self::Miss { key, .. } => *key,
        }
    }

    pub fn version(&self) -> u64 {
        match self {
            Self::Hit(hit) => hit.version,
            Self::Miss { version, .. } => *version,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct XrDepthMeshPendingQuery {
    pub query: XrDepthMeshQuery,
    pub version: u64,
}

#[derive(Default)]
struct XrDepthMeshQueryState {
    next_versions: HashMap<u64, u64>,
    pending: HashMap<u64, XrDepthMeshPendingQuery>,
    pending_order: VecDeque<u64>,
    results: HashMap<u64, XrDepthMeshQueryResult>,
}

#[derive(Clone)]
pub struct XrDepthMeshStore {
    state: Arc<RwLock<XrDepthMeshState>>,
    alignment_state: Arc<RwLock<Arc<XrDepthAlignState>>>,
    queries: Arc<Mutex<XrDepthMeshQueryState>>,
    reset_generation: Arc<AtomicU64>,
    mesh_enabled: Arc<AtomicBool>,
    plane_scan_enabled: Arc<AtomicBool>,
    surface_analysis_enabled: Arc<AtomicBool>,
    alignment_preview_enabled: Arc<AtomicBool>,
    voxel_size_meters_bits: Arc<AtomicU32>,
}

impl Default for XrDepthMeshStore {
    fn default() -> Self {
        Self {
            state: Arc::new(RwLock::new(XrDepthMeshState::default())),
            alignment_state: Arc::new(RwLock::new(Arc::new(XrDepthAlignState::default()))),
            queries: Arc::new(Mutex::new(XrDepthMeshQueryState::default())),
            reset_generation: Arc::new(AtomicU64::new(0)),
            mesh_enabled: Arc::new(AtomicBool::new(false)),
            plane_scan_enabled: Arc::new(AtomicBool::new(false)),
            surface_analysis_enabled: Arc::new(AtomicBool::new(false)),
            alignment_preview_enabled: Arc::new(AtomicBool::new(false)),
            voxel_size_meters_bits: Arc::new(AtomicU32::new(
                XR_DEPTH_MESH_DEFAULT_VOXEL_SIZE_METERS.to_bits(),
            )),
        }
    }
}

impl XrDepthMeshStore {
    fn keeps_latest_mesh_alive(&self) -> bool {
        self.mesh_enabled() || self.surface_analysis_enabled() || self.plane_scan_enabled()
    }

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

    pub fn set_mesh_enabled(&self, enabled: bool) {
        let was_enabled = self.mesh_enabled.swap(enabled, Ordering::AcqRel);
        if was_enabled && !enabled && !self.keeps_latest_mesh_alive() {
            if let Ok(mut state) = self.state.write() {
                state.latest_mesh = None;
            }
        }
    }

    pub fn mesh_enabled(&self) -> bool {
        self.mesh_enabled.load(Ordering::Acquire)
    }

    pub fn set_plane_scan_enabled(&self, enabled: bool) {
        let was_enabled = self.plane_scan_enabled.swap(enabled, Ordering::AcqRel);
        if was_enabled && !enabled && !self.keeps_latest_mesh_alive() {
            if let Ok(mut state) = self.state.write() {
                state.latest_mesh = None;
            }
        }
    }

    pub fn plane_scan_enabled(&self) -> bool {
        self.plane_scan_enabled.load(Ordering::Acquire)
    }

    pub fn set_surface_analysis_enabled(&self, enabled: bool) {
        let was_enabled = self
            .surface_analysis_enabled
            .swap(enabled, Ordering::AcqRel);
        if was_enabled && !enabled && !self.keeps_latest_mesh_alive() {
            if let Ok(mut state) = self.state.write() {
                state.latest_mesh = None;
            }
        }
    }

    pub fn surface_analysis_enabled(&self) -> bool {
        self.surface_analysis_enabled.load(Ordering::Acquire)
    }

    pub fn set_alignment_preview_enabled(&self, enabled: bool) {
        self.alignment_preview_enabled
            .store(enabled, Ordering::Release);
    }

    pub fn alignment_preview_enabled(&self) -> bool {
        self.alignment_preview_enabled.load(Ordering::Acquire)
    }

    pub fn state(&self) -> Arc<RwLock<XrDepthMeshState>> {
        self.state.clone()
    }

    pub fn latest_mesh(&self) -> Option<Arc<XrDepthMesh>> {
        self.state
            .read()
            .ok()
            .and_then(|state| state.latest_mesh.clone())
    }

    pub fn latest_alignment_state(&self) -> Arc<XrDepthAlignState> {
        self.alignment_state
            .read()
            .map(|state| state.clone())
            .unwrap_or_else(|_| Arc::new(XrDepthAlignState::default()))
    }

    pub fn submit_query(&self, query: XrDepthMeshQuery) -> Option<u64> {
        let Ok(mut state) = self.queries.lock() else {
            return None;
        };
        let version = state
            .next_versions
            .entry(query.key)
            .and_modify(|version| *version = version.saturating_add(1))
            .or_insert(1);
        let version = *version;

        if let Some(pending) = state.pending.get_mut(&query.key) {
            pending.query = query;
            pending.version = version;
            return Some(version);
        }

        if state.pending.len() >= XR_DEPTH_QUERY_MAX_PENDING {
            return None;
        }

        state
            .pending
            .insert(query.key, XrDepthMeshPendingQuery { query, version });
        state.pending_order.push_back(query.key);
        Some(version)
    }

    pub fn latest_query_result(&self, key: u64) -> Option<XrDepthMeshQueryResult> {
        self.queries
            .lock()
            .ok()
            .and_then(|state| state.results.get(&key).cloned())
    }

    pub fn clear_query(&self, key: u64) {
        if let Ok(mut state) = self.queries.lock() {
            state.pending.remove(&key);
            state
                .pending_order
                .retain(|pending_key| *pending_key != key);
            state.results.remove(&key);
            state.next_versions.remove(&key);
        }
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
    pub(crate) fn publish(&self, mesh: XrDepthMesh) {
        if let Ok(mut state) = self.state.write() {
            state.latest_mesh = Some(Arc::new(mesh));
            state.stats.frames_meshed += 1;
            state.last_error = None;
        }
    }

    #[allow(dead_code)]
    pub(crate) fn publish_alignment_state(&self, alignment_state: XrDepthAlignState) {
        if let Ok(mut state) = self.alignment_state.write() {
            *state = Arc::new(alignment_state);
        }
    }

    #[allow(dead_code)]
    pub(crate) fn drain_pending_queries(&self, max_queries: usize) -> Vec<XrDepthMeshPendingQuery> {
        let Ok(mut state) = self.queries.lock() else {
            return Vec::new();
        };
        let mut drained = Vec::with_capacity(max_queries.min(state.pending.len()));
        for _ in 0..max_queries {
            let Some(key) = state.pending_order.pop_front() else {
                break;
            };
            let Some(query) = state.pending.remove(&key) else {
                continue;
            };
            drained.push(query);
        }
        drained
    }

    #[allow(dead_code)]
    pub(crate) fn has_pending_queries(&self) -> bool {
        self.queries
            .lock()
            .map(|state| !state.pending.is_empty())
            .unwrap_or(false)
    }

    #[allow(dead_code)]
    pub(crate) fn publish_query_results(&self, results: Vec<XrDepthMeshQueryResult>) {
        if results.is_empty() {
            return;
        }
        if let Ok(mut state) = self.queries.lock() {
            for result in results {
                state.results.insert(result.key(), result);
            }
        }
    }

    #[allow(dead_code)]
    pub(crate) fn clear(&self) {
        if let Ok(mut state) = self.state.write() {
            state.latest_mesh = None;
            state.last_error = None;
            state.stats = XrDepthMeshStats::default();
        }
        if let Ok(mut alignment_state) = self.alignment_state.write() {
            *alignment_state = Arc::new(XrDepthAlignState::default());
        }
        if let Ok(mut queries) = self.queries.lock() {
            *queries = XrDepthMeshQueryState::default();
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

    fn make_wall_feature(
        center: Vec3f,
        normal: Vec3f,
        half_extent_along: f32,
        min_y: f32,
        max_y: f32,
    ) -> XrDepthAlignWallFeature {
        let normal = normal.normalize();
        let along_axis = vec3(-normal.z, 0.0, normal.x).normalize();
        XrDepthAlignWallFeature {
            center,
            normal,
            along_axis,
            plane_distance: center.dot(normal),
            half_extent_along,
            min_y,
            max_y,
            area: (half_extent_along * 2.0) * (max_y - min_y).max(0.0),
        }
    }

    fn make_wall_sample(point: Vec3f, normal: Vec3f, weight: f32) -> XrDepthAlignSample {
        XrDepthAlignSample {
            kind: XrDepthAlignSampleKind::Wall,
            point,
            normal: normal.normalize(),
            weight,
        }
    }

    fn make_wall_samples_from_feature(
        feature: &XrDepthAlignWallFeature,
        weight: f32,
    ) -> [XrDepthAlignSample; 2] {
        [
            make_wall_sample(
                vec3(
                    feature.center.x - feature.along_axis.x * feature.half_extent_along * 0.55,
                    feature.min_y + 0.18,
                    feature.center.z - feature.along_axis.z * feature.half_extent_along * 0.55,
                ),
                feature.normal,
                weight,
            ),
            make_wall_sample(
                vec3(
                    feature.center.x + feature.along_axis.x * feature.half_extent_along * 0.55,
                    feature.max_y - 0.18,
                    feature.center.z + feature.along_axis.z * feature.half_extent_along * 0.55,
                ),
                feature.normal,
                weight * 0.96,
            ),
        ]
    }

    fn make_asymmetric_wall_descriptor() -> XrDepthAlignDescriptor {
        let wall_features = vec![
            make_wall_feature(
                vec3(-1.10, 0.95, -0.90),
                vec3(1.0, 0.0, 0.0),
                0.46,
                0.52,
                1.38,
            ),
            make_wall_feature(
                vec3(0.98, 0.88, -1.26),
                vec3(1.0, 0.0, 0.0),
                0.34,
                0.54,
                1.22,
            ),
            make_wall_feature(
                vec3(0.08, 0.98, -2.12),
                vec3(0.0, 0.0, 1.0),
                0.38,
                0.58,
                1.34,
            ),
            make_wall_feature(
                vec3(0.36, 0.92, -0.42),
                vec3(0.0, 0.0, 1.0),
                0.30,
                0.60,
                1.24,
            ),
        ];
        let mut samples = Vec::new();
        for (index, feature) in wall_features.iter().enumerate() {
            samples.extend(make_wall_samples_from_feature(
                feature,
                0.92 - index as f32 * 0.04,
            ));
        }
        XrDepthAlignDescriptor {
            voxel_size_meters: 0.05,
            floor_y: 0.0,
            wall_normal_histogram: xr_depth_align_build_wall_feature_normal_histogram(
                &wall_features,
                48,
            ),
            wall_features,
            samples,
        }
    }

    fn make_box_room_descriptor_with_patch_asymmetry() -> XrDepthAlignDescriptor {
        let wall_features = vec![
            make_wall_feature(vec3(1.42, 1.0, 0.18), vec3(1.0, 0.0, 0.0), 1.18, 0.0, 2.0),
            make_wall_feature(vec3(-1.08, 1.0, 0.22), vec3(-1.0, 0.0, 0.0), 1.18, 0.0, 2.0),
            make_wall_feature(vec3(0.16, 1.0, 1.54), vec3(0.0, 0.0, 1.0), 1.25, 0.0, 2.0),
            make_wall_feature(vec3(0.14, 1.0, -0.96), vec3(0.0, 0.0, -1.0), 1.25, 0.0, 2.0),
        ];
        let samples = vec![
            make_wall_sample(vec3(1.42, 1.28, -0.52), vec3(1.0, 0.0, 0.0), 0.96),
            make_wall_sample(vec3(1.42, 0.42, -0.18), vec3(1.0, 0.0, 0.0), 0.90),
            make_wall_sample(vec3(0.78, 1.14, 1.54), vec3(0.0, 0.0, 1.0), 0.92),
            make_wall_sample(vec3(0.36, 0.52, 1.54), vec3(0.0, 0.0, 1.0), 0.88),
        ];
        XrDepthAlignDescriptor {
            voxel_size_meters: 0.05,
            floor_y: 0.0,
            wall_normal_histogram: xr_depth_align_build_wall_feature_normal_histogram(
                &wall_features,
                48,
            ),
            wall_features,
            samples,
        }
    }

    fn reflection_x() -> Mat4f {
        Mat4f {
            v: [
                -1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
            ],
        }
    }

    fn angle_error(a: f32, b: f32) -> f32 {
        wrap_angle(a - b).abs()
    }

    #[test]
    fn disabling_mesh_keeps_latest_mesh_while_surface_analysis_is_enabled() {
        let store = XrDepthMeshStore::default();
        store.publish(XrDepthMesh::default());
        store.set_surface_analysis_enabled(true);
        store.set_mesh_enabled(true);
        store.set_mesh_enabled(false);
        assert!(store.latest_mesh().is_some());
    }

    #[test]
    fn disabling_last_surface_consumer_clears_latest_mesh() {
        let store = XrDepthMeshStore::default();
        store.publish(XrDepthMesh::default());
        store.set_surface_analysis_enabled(true);
        store.set_surface_analysis_enabled(false);
        assert!(store.latest_mesh().is_none());
    }

    #[test]
    fn wall_solver_recovers_asymmetric_pose() {
        let local = make_asymmetric_wall_descriptor();
        let remote_to_local = Pose::new(
            Quat::from_axis_angle(vec3(0.0, 1.0, 0.0), 0.58),
            vec3(-0.82, 0.0, 0.67),
        )
        .to_mat4();
        let local_to_remote = remote_to_local.invert();
        let mut remote = xr_depth_align_transform_descriptor(&local, &local_to_remote);
        remote.samples = remote
            .samples
            .into_iter()
            .enumerate()
            .filter_map(|(index, mut sample)| {
                if index % 5 == 2 {
                    return None;
                }
                sample.point += vec3(
                    ((index % 4) as f32 - 1.5) * 0.012,
                    (((index * 3) % 5) as f32 - 2.0) * 0.020,
                    (((index * 5) % 7) as f32 - 3.0) * 0.012,
                );
                sample.weight =
                    (sample.weight * (0.84 + 0.03 * (index % 4) as f32)).clamp(0.1, 1.0);
                Some(sample)
            })
            .collect();
        remote.wall_normal_histogram =
            xr_depth_align_build_wall_feature_normal_histogram(&remote.wall_features, 48);

        let diagnostic = xr_depth_align_analyze_remote_to_local(&local, &remote);
        let solution = diagnostic.accepted_solution().unwrap_or_else(|| {
            panic!("solver should recover asymmetric wall pose: {diagnostic:?}")
        });

        assert!(
            angle_error(solution.yaw_radians, 0.58) < 0.08,
            "{solution:?}"
        );
        assert!(
            (solution.translation - vec3(-0.82, 0.0, 0.67)).length() < 0.12,
            "{solution:?}"
        );
        assert!(solution.confidence > 0.20, "{solution:?}");
        assert!(solution.matched_samples >= 2, "{solution:?}");
    }

    #[test]
    fn wall_solver_rejects_mirrored_asymmetric_descriptor() {
        let local = make_asymmetric_wall_descriptor();
        let mirrored = xr_depth_align_transform_descriptor(&local, &reflection_x());
        let diagnostic = xr_depth_align_analyze_remote_to_local(&local, &mirrored);

        assert!(
            diagnostic.accepted_solution().is_none(),
            "mirrored descriptor should not be accepted: {diagnostic:?}"
        );
    }

    #[test]
    fn wall_solver_ignores_along_wall_patch_sliding() {
        let local = make_asymmetric_wall_descriptor();
        let remote_to_local = Pose::new(
            Quat::from_axis_angle(vec3(0.0, 1.0, 0.0), 0.44),
            vec3(-0.46, 0.0, 0.58),
        )
        .to_mat4();
        let local_to_remote = remote_to_local.invert();
        let mut remote = xr_depth_align_transform_descriptor(&local, &local_to_remote);

        for (index, feature) in remote.wall_features.iter_mut().enumerate() {
            let slide = match index {
                0 => 0.32,
                1 => -0.24,
                2 => 0.28,
                _ => -0.18,
            };
            feature.center += feature.along_axis.scale(slide);
            feature.plane_distance = feature.center.dot(feature.normal);
        }
        for (index, sample) in remote.samples.iter_mut().enumerate() {
            let slide = match index / 2 {
                0 => 0.32,
                1 => -0.24,
                2 => 0.28,
                _ => -0.18,
            };
            let axis = vec3(-sample.normal.z, 0.0, sample.normal.x).normalize();
            sample.point += axis.scale(slide);
        }
        remote.wall_normal_histogram =
            xr_depth_align_build_wall_feature_normal_histogram(&remote.wall_features, 48);

        let diagnostic = xr_depth_align_analyze_remote_to_local(&local, &remote);
        let solution = diagnostic.accepted_solution().unwrap_or_else(|| {
            panic!(
                "solver should still recover transform from orthogonal wall offsets: {diagnostic:?}"
            )
        });

        assert!(
            angle_error(solution.yaw_radians, 0.44) < 0.08,
            "{solution:?}"
        );
        assert!(
            (solution.translation - vec3(-0.46, 0.0, 0.58)).length() < 0.08,
            "{solution:?}"
        );
        assert!(solution.confidence > 0.18, "{solution:?}");
        assert!(solution.matched_samples >= 2, "{solution:?}");
    }

    #[test]
    fn wall_solver_uses_patch_asymmetry_to_break_box_room_flip() {
        let local = make_box_room_descriptor_with_patch_asymmetry();
        let expected_yaw = 0.37;
        let expected_translation = vec3(-0.62, 0.0, 0.48);
        let remote_to_local = Pose::new(
            Quat::from_axis_angle(vec3(0.0, 1.0, 0.0), expected_yaw),
            expected_translation,
        )
        .to_mat4();
        let local_to_remote = remote_to_local.invert();
        let mut remote = xr_depth_align_transform_descriptor(&local, &local_to_remote);
        remote.wall_normal_histogram =
            xr_depth_align_build_wall_feature_normal_histogram(&remote.wall_features, 48);

        let diagnostic = xr_depth_align_analyze_remote_to_local(&local, &remote);
        let solution = diagnostic.accepted_solution().unwrap_or_else(|| {
            panic!("room-box solver should accept the patch-supported pose: {diagnostic:?}")
        });
        assert!(
            angle_error(solution.yaw_radians, expected_yaw) < 0.08,
            "{solution:?}"
        );
        assert!(
            (solution.translation - expected_translation).length() < 0.10,
            "{solution:?}"
        );

        let local_walls = descriptor_wall_features(&local);
        let remote_walls = descriptor_wall_features(&remote);
        let local_samples = descriptor_samples_of_kind(&local, XrDepthAlignSampleKind::Wall);
        let remote_samples = descriptor_samples_of_kind(&remote, XrDepthAlignSampleKind::Wall);
        let correct = score_wall_feature_alignment(
            &local_walls,
            &remote_walls,
            &local_samples,
            &remote_samples,
            expected_yaw,
            expected_translation,
        );
        let flipped_yaw = wrap_angle(expected_yaw + std::f32::consts::PI);
        let flipped =
            candidate_wall_feature_translations(&local_walls, &remote_walls, 0.0, flipped_yaw)
                .into_iter()
                .map(|translation| {
                    score_wall_feature_alignment(
                        &local_walls,
                        &remote_walls,
                        &local_samples,
                        &remote_samples,
                        flipped_yaw,
                        translation,
                    )
                })
                .max_by(|left, right| {
                    left.confidence
                        .total_cmp(&right.confidence)
                        .then_with(|| left.matched_samples.cmp(&right.matched_samples))
                })
                .expect("expected flipped box-room hypothesis");
        assert!(
            correct.confidence > flipped.confidence + 0.06,
            "patch asymmetry should make the correct box-room pose rank above the flipped one: correct={correct:?} flipped={flipped:?}"
        );
    }
}
