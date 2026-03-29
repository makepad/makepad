use crate::*;
use makepad_widgets::makepad_platform::makepad_micro_serde::*;
use std::collections::HashMap;

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
    pub remote_dense_wall_samples: usize,
    pub remote_refine_wall_samples: usize,
    pub yaw_candidate_count: usize,
    pub pose_candidate_count: usize,
    pub shortlisted_pose_count: usize,
    pub signal_build_ms: u32,
    pub yaw_candidate_ms: u32,
    pub translation_vote_ms: u32,
    pub signal_refine_ms: u32,
    pub signal_score_ms: u32,
    pub height_refine_ms: u32,
    pub final_score_ms: u32,
    pub wall_profile_ms: u32,
    pub total_compute_ms: u32,
    pub step_count: u32,
    pub max_step_ms: u32,
    pub best_solution: Option<XrDepthAlignSolution>,
}

impl XrDepthAlignSolveDiagnostic {
    pub fn accepted_solution(&self) -> Option<XrDepthAlignSolution> {
        self.best_solution
            .filter(|solution| xr_depth_align_solution_is_accepted(self, *solution))
    }

    pub fn outcome(&self) -> XrDepthAlignSolveOutcome {
        if self.local_wall_samples < XR_DEPTH_ALIGN_MIN_SIGNAL_SAMPLES
            || self.remote_wall_samples < XR_DEPTH_ALIGN_MIN_SIGNAL_SAMPLES
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

const XR_DEPTH_ALIGN_MIN_SIGNAL_SAMPLES: usize = 4;
const XR_DEPTH_ALIGN_ACCEPT_MIN_MATCHED_SAMPLES: usize = 6;
const XR_DEPTH_ALIGN_ACCEPT_MIN_CONFIDENCE: f32 = 0.12;
const XR_DEPTH_ALIGN_ACCEPT_MIN_SYMMETRY_CONFIDENCE: f32 = 0.10;
const XR_DEPTH_ALIGN_TRANSLATION_VOTE_STEP_METERS: f32 = 0.08;
const XR_DEPTH_ALIGN_HEIGHT_MAP_HISTOGRAM_BINS: usize = 48;
const XR_DEPTH_ALIGN_HEIGHT_MAP_MAX_SAMPLES: usize = 96;
const XR_DEPTH_ALIGN_SIGNAL_SHORTLIST_MAX_CANDIDATES: usize = 80;
const XR_DEPTH_ALIGN_SIGNAL_SHORTLIST_PER_YAW: usize = 2;
const XR_DEPTH_ALIGN_SIGNAL_SHORTLIST_YAW_EPSILON_RADIANS: f32 = 1.0_f32.to_radians();
const XR_DEPTH_ALIGN_SIGNAL_SHORTLIST_TRANSLATION_EPSILON_METERS: f32 = 0.10;
const XR_DEPTH_ALIGN_HEIGHT_REFINE_MAX_DENSE_SAMPLES: usize = 1536;
const XR_DEPTH_ALIGN_HEIGHT_MAP_GRADIENT_MIN_METERS: f32 = 0.05;
const XR_DEPTH_ALIGN_HEIGHT_MAP_MIN_SPACING_METERS: f32 = 0.14;
const XR_DEPTH_ALIGN_VERTICAL_OFFSET_BIN_METERS: f32 = 0.02;
const XR_DEPTH_ALIGN_VERTICAL_OFFSET_MAX_DELTA_METERS: f32 = 0.80;
const XR_DEPTH_ALIGN_VERTICAL_OFFSET_MIN_MATCHES: usize = 24;
const XR_DEPTH_ALIGN_VERTICAL_OFFSET_MIN_SUPPORT_RATIO: f32 = 0.08;
const XR_DEPTH_ALIGN_VERTICAL_OFFSET_SUPPORT_WINDOW_BINS: usize = 3;
const XR_DEPTH_ALIGN_SIGNAL_MATCH_RADIUS_METERS: f32 = 0.42;
const XR_DEPTH_ALIGN_SIGNAL_MATCH_MIN_DIRECTION_DOT: f32 = 0.45;
const XR_DEPTH_ALIGN_SIGNAL_MATCH_MAX_HEIGHT_DELTA_METERS: f32 = 0.75;
const XR_DEPTH_ALIGN_SEEDED_LOCK_MIN_CONFIDENCE: f32 = 0.20;
const XR_DEPTH_ALIGN_SEEDED_LOCK_MIN_SYMMETRY_CONFIDENCE: f32 = 0.18;
const XR_DEPTH_ALIGN_SEEDED_LOCK_MIN_OVERLAP: f32 = 0.35;
const XR_DEPTH_ALIGN_SEEDED_LOCK_MAX_TRANSLATION_JUMP_METERS: f32 = 0.75;
const XR_DEPTH_ALIGN_SEEDED_LOCK_MAX_YAW_JUMP_RADIANS: f32 = 0.45;
const XR_DEPTH_ALIGN_WALL_PROFILE_MIN_HEIGHT_METERS: f32 = 0.60;
const XR_DEPTH_ALIGN_WALL_PROFILE_MAX_HEIGHT_METERS: f32 = 1.80;
const XR_DEPTH_ALIGN_WALL_PROFILE_CELL_METERS: f32 = 0.09;
const XR_DEPTH_ALIGN_WALL_PROFILE_MIN_POINTS: usize = 16;
const XR_DEPTH_ALIGN_WALL_PROFILE_YAW_WINDOW_RADIANS: f32 = 10.0_f32.to_radians();
const XR_DEPTH_ALIGN_WALL_PROFILE_YAW_STEP_RADIANS: f32 = 0.5_f32.to_radians();
const XR_DEPTH_ALIGN_WALL_PROFILE_TRANSLATION_WINDOW_METERS: f32 = 0.18;
const XR_DEPTH_ALIGN_WALL_PROFILE_TRANSLATION_STEP_METERS: f32 = 0.06;
const XR_DEPTH_ALIGN_WALL_PROFILE_TRANSLATION_SIGMA_METERS: f32 = 0.12;
const XR_DEPTH_ALIGN_WALL_PROFILE_MIN_YAW_DELTA_RADIANS: f32 = 1.2_f32.to_radians();
const XR_DEPTH_ALIGN_WALL_PROFILE_MIN_SCORE_GAIN: f32 = 0.010;
const XR_DEPTH_ALIGN_WALL_PROFILE_MIN_SCORE_RATIO: f32 = 1.02;
const XR_DEPTH_ALIGN_WALL_PROFILE_MIN_CONFIDENCE_RATIO: f32 = 0.82;
const XR_DEPTH_ALIGN_WALL_PROFILE_MAX_RESIDUAL_INCREASE_METERS: f32 = 0.08;
const XR_DEPTH_ALIGN_WALL_PROFILE_MAX_MATCH_LOSS: usize = 6;

#[derive(Clone, Copy, Debug)]
struct HeightMapSignalCell {
    point: Vec3f,
    height: f32,
    gradient: Vec3f,
    axis_xz: Vec2f,
    weight: f32,
}

#[derive(Clone, Copy, Debug)]
struct DenseHeightMapSignalCell {
    point_x: f32,
    point_z: f32,
    height: f32,
    axis_x: f32,
    axis_z: f32,
    weight: f32,
}

#[derive(Clone, Copy, Debug)]
struct RotatedHeightMapSignalCell {
    point_x: f32,
    point_z: f32,
    axis_x: f32,
    axis_z: f32,
    height: f32,
    weight: f32,
}

#[derive(Clone, Copy, Debug)]
struct ProjectionBounds {
    min: f32,
    max: f32,
}

#[derive(Clone, Debug)]
struct HeightMapSampleCache {
    origin_x: f32,
    origin_z: f32,
    inv_cell_size_meters: f32,
    size_x: usize,
    size_z: usize,
    signal_axes: Vec<Vec2f>,
}

fn duration_ms_u32(duration: std::time::Duration) -> u32 {
    duration.as_millis().min(u32::MAX as u128) as u32
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
    normalize_histogram(&mut histogram);
    histogram
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

pub fn xr_depth_align_test_markers(descriptor: &XrDepthAlignDescriptor) -> Option<[Vec3f; 2]> {
    let signal = selected_descriptor_signal_cells(descriptor);
    let mut best = None::<(f32, f32, Vec3f, Vec3f)>;
    for (index, first) in signal.iter().enumerate() {
        for second in signal.iter().skip(index + 1) {
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
    XrDepthAlignMatcher::new(local, remote, previous_solution).finish()
}

pub fn xr_depth_align_rescore_remote_to_local(
    local: &XrDepthAlignDescriptor,
    remote: &XrDepthAlignDescriptor,
    solution: XrDepthAlignSolution,
) -> XrDepthAlignSolution {
    let local_signal = selected_descriptor_signal_cells(local);
    let remote_signal = selected_descriptor_signal_cells(remote);
    let remote_dense_signal = descriptor_dense_signal_cells(remote);
    let local_sample_cache = local.height_map.as_ref().and_then(build_height_map_sample_cache);
    if local_signal.is_empty() || remote_signal.is_empty() {
        return XrDepthAlignSolution {
            yaw_radians: solution.yaw_radians,
            translation: solution.translation,
            confidence: 0.0,
            symmetry_confidence: 0.0,
            residual_meters: f32::INFINITY,
            matched_samples: 0,
        };
    }
    let rescored = score_signal_alignment_solution(
        &local_signal,
        &remote_signal,
        solution.yaw_radians,
        solution.translation,
    );
    let rescored = match (local.height_map.as_ref(), remote.height_map.as_ref()) {
        (Some(local_map), Some(remote_map)) => refine_solution_vertical_offset_from_overlap(
            local_map,
            remote_map,
            &remote_dense_signal,
            rescored,
        ),
        _ => rescored,
    };
    apply_height_map_alignment_support(
        rescored,
        local.height_map.as_ref(),
        local_sample_cache.as_ref(),
        remote.height_map.as_ref(),
        &remote_dense_signal,
    )
}

const XR_DEPTH_ALIGN_HEIGHT_REFINE_PHASES: [(f32, f32); 5] = [
    (0.18, 0.36),
    (0.08, 0.14),
    (0.03, 0.06),
    (0.012, 0.025),
    (0.005, 0.01),
];

const XR_DEPTH_ALIGN_SEEDED_LOCAL_SEARCH_PHASES: [(f32, f32); 4] =
    [(0.10, 0.18), (0.04, 0.07), (0.015, 0.03), (0.006, 0.012)];

const XR_DEPTH_ALIGN_TRANSLATION_ONLY_SEARCH_PHASES: [(f32, f32); 3] =
    [(0.0, 0.06), (0.0, 0.025), (0.0, 0.01)];

#[derive(Debug)]
struct FullAlignmentSearchTask {
    best: XrDepthAlignSolution,
    floor_y: f32,
    phases: &'static [(f32, f32)],
    translation_only: bool,
    phase_index: usize,
    yaw_index: usize,
    tx_index: usize,
    tz_index: usize,
    improved_in_pass: bool,
}

impl FullAlignmentSearchTask {
    fn new(
        best: XrDepthAlignSolution,
        floor_y: f32,
        phases: &'static [(f32, f32)],
        translation_only: bool,
    ) -> Self {
        Self {
            best,
            floor_y,
            phases,
            translation_only,
            phase_index: 0,
            yaw_index: 0,
            tx_index: 0,
            tz_index: 0,
            improved_in_pass: false,
        }
    }

    fn step(
        &mut self,
        local_signal: &[HeightMapSignalCell],
        remote_signal: &[HeightMapSignalCell],
        local_map: Option<&XrDepthAlignHeightMap>,
        local_sample_cache: Option<&HeightMapSampleCache>,
        remote_map: Option<&XrDepthAlignHeightMap>,
        remote_dense_signal: &[DenseHeightMapSignalCell],
    ) -> Option<XrDepthAlignSolution> {
        let translation_deltas = [-1.0f32, 0.0, 1.0];
        loop {
            let Some(&(yaw_step, translation_step)) = self.phases.get(self.phase_index) else {
                return Some(self.best);
            };
            let yaw_count = if self.translation_only { 1 } else { 3 };
            while self.yaw_index < yaw_count {
                let yaw_delta = if self.translation_only {
                    0.0
                } else {
                    [-yaw_step, 0.0, yaw_step][self.yaw_index]
                };
                let tx_delta = translation_deltas[self.tx_index] * translation_step;
                let tz_delta = translation_deltas[self.tz_index] * translation_step;
                self.advance_indices(yaw_count);
                if yaw_delta == 0.0 && tx_delta == 0.0 && tz_delta == 0.0 {
                    continue;
                }
                let candidate = score_full_alignment_solution(
                    local_signal,
                    remote_signal,
                    local_map,
                    local_sample_cache,
                    remote_map,
                    remote_dense_signal,
                    wrap_angle(self.best.yaw_radians + yaw_delta),
                    vec3(
                        self.best.translation.x + tx_delta,
                        self.floor_y,
                        self.best.translation.z + tz_delta,
                    ),
                );
                if alignment_solution_better(&candidate, &self.best) {
                    self.best = candidate;
                    self.improved_in_pass = true;
                }
                return None;
            }
            if self.improved_in_pass {
                self.improved_in_pass = false;
                self.reset_pass();
                continue;
            }
            self.phase_index += 1;
            self.reset_pass();
        }
    }

    fn advance_indices(&mut self, yaw_count: usize) {
        self.tz_index += 1;
        if self.tz_index < 3 {
            return;
        }
        self.tz_index = 0;
        self.tx_index += 1;
        if self.tx_index < 3 {
            return;
        }
        self.tx_index = 0;
        self.yaw_index += 1;
        if self.yaw_index > yaw_count {
            self.yaw_index = yaw_count;
        }
    }

    fn reset_pass(&mut self) {
        self.yaw_index = 0;
        self.tx_index = 0;
        self.tz_index = 0;
    }
}

#[derive(Debug)]
struct HeightMapRefineTask {
    best_yaw: f32,
    best_translation: Vec3f,
    best_score: (f32, f32, usize),
    initialized: bool,
    phase_index: usize,
    yaw_index: usize,
    tx_index: usize,
    tz_index: usize,
    improved_in_pass: bool,
    rotated_remote_signal: Vec<RotatedHeightMapSignalCell>,
    rotated_signal_yaw: Option<f32>,
}

impl HeightMapRefineTask {
    fn new(yaw: f32, translation: Vec3f) -> Self {
        Self {
            best_yaw: wrap_angle(yaw),
            best_translation: translation,
            best_score: (0.0, f32::INFINITY, 0),
            initialized: false,
            phase_index: 0,
            yaw_index: 0,
            tx_index: 0,
            tz_index: 0,
            improved_in_pass: false,
            rotated_remote_signal: Vec::new(),
            rotated_signal_yaw: None,
        }
    }

    fn step(
        &mut self,
        local_map: &XrDepthAlignHeightMap,
        local_sample_cache: Option<&HeightMapSampleCache>,
        remote_map: &XrDepthAlignHeightMap,
        remote_signal: &[DenseHeightMapSignalCell],
    ) -> Option<(f32, Vec3f)> {
        if !self.initialized {
            self.best_score = score_height_map_alignment(
                local_map,
                local_sample_cache,
                remote_map,
                remote_signal,
                self.best_yaw,
                self.best_translation,
            );
            self.initialized = true;
            return None;
        }
        let translation_deltas = [-1.0f32, 0.0, 1.0];
        loop {
            let Some(&(yaw_step, translation_step)) =
                XR_DEPTH_ALIGN_HEIGHT_REFINE_PHASES.get(self.phase_index)
            else {
                return Some((self.best_yaw, self.best_translation));
            };
            while self.yaw_index < 3 {
                let yaw_delta = [-yaw_step, 0.0, yaw_step][self.yaw_index];
                let tx_delta = translation_deltas[self.tx_index] * translation_step;
                let tz_delta = translation_deltas[self.tz_index] * translation_step;
                self.advance_indices();
                if yaw_delta == 0.0 && tx_delta == 0.0 && tz_delta == 0.0 {
                    continue;
                }
                let candidate_yaw = wrap_angle(self.best_yaw + yaw_delta);
                if self.rotated_signal_yaw != Some(candidate_yaw) {
                    fill_rotated_height_map_signal(
                        &mut self.rotated_remote_signal,
                        remote_signal,
                        candidate_yaw,
                    );
                    self.rotated_signal_yaw = Some(candidate_yaw);
                }
                let candidate_translation = vec3(
                    self.best_translation.x + tx_delta,
                    self.best_translation.y,
                    self.best_translation.z + tz_delta,
                );
                let candidate_score = score_rotated_height_map_alignment(
                    local_map,
                    local_sample_cache,
                    remote_map,
                    &self.rotated_remote_signal,
                    candidate_yaw,
                    candidate_translation,
                );
                if height_map_score_better(candidate_score, self.best_score) {
                    self.best_yaw = candidate_yaw;
                    self.best_translation = candidate_translation;
                    self.best_score = candidate_score;
                    self.improved_in_pass = true;
                }
                return None;
            }
            if self.improved_in_pass {
                self.improved_in_pass = false;
                self.reset_pass();
                continue;
            }
            self.phase_index += 1;
            self.reset_pass();
        }
    }

    fn advance_indices(&mut self) {
        self.tz_index += 1;
        if self.tz_index < 3 {
            return;
        }
        self.tz_index = 0;
        self.tx_index += 1;
        if self.tx_index < 3 {
            return;
        }
        self.tx_index = 0;
        self.yaw_index += 1;
        if self.yaw_index > 3 {
            self.yaw_index = 3;
        }
    }

    fn reset_pass(&mut self) {
        self.yaw_index = 0;
        self.tx_index = 0;
        self.tz_index = 0;
    }
}

#[derive(Debug)]
enum WallProfileTaskStage {
    BuildProfiles,
    SearchYaw {
        current_profile_score: f32,
        best_profile_yaw: f32,
        best_profile_score: f32,
        next_yaw_index: isize,
        yaw_steps: isize,
    },
    ScoreCorrectedInitial {
        current_profile_score: f32,
        best_profile_yaw: f32,
        best_profile_score: f32,
    },
    RefineTranslation {
        current_profile_score: f32,
        best_profile_score: f32,
        search: FullAlignmentSearchTask,
    },
    Done(XrDepthAlignSolution),
}

#[derive(Debug)]
struct WallProfileTask {
    current: XrDepthAlignSolution,
    floor_y: f32,
    local_profile: Vec<(f32, f32)>,
    remote_profile: Vec<(f32, f32)>,
    stage: WallProfileTaskStage,
}

impl WallProfileTask {
    fn new(current: XrDepthAlignSolution, floor_y: f32) -> Self {
        Self {
            current,
            floor_y,
            local_profile: Vec::new(),
            remote_profile: Vec::new(),
            stage: WallProfileTaskStage::BuildProfiles,
        }
    }

    fn step(
        &mut self,
        local_signal: &[HeightMapSignalCell],
        remote_signal: &[HeightMapSignalCell],
        local_map: &XrDepthAlignHeightMap,
        local_sample_cache: Option<&HeightMapSampleCache>,
        remote_map: &XrDepthAlignHeightMap,
        remote_dense_signal: &[DenseHeightMapSignalCell],
    ) -> Option<XrDepthAlignSolution> {
        loop {
            match &mut self.stage {
                WallProfileTaskStage::BuildProfiles => {
                    self.local_profile = build_wall_profile_contour_points(local_map);
                    self.remote_profile = build_wall_profile_contour_points(remote_map);
                    if self.local_profile.len() < XR_DEPTH_ALIGN_WALL_PROFILE_MIN_POINTS
                        || self.remote_profile.len() < XR_DEPTH_ALIGN_WALL_PROFILE_MIN_POINTS
                    {
                        self.stage = WallProfileTaskStage::Done(self.current);
                        continue;
                    }
                    let current_profile_score = score_wall_profile_alignment_with_translation_prior(
                        &self.local_profile,
                        &self.remote_profile,
                        self.current.yaw_radians,
                        self.current.translation,
                    );
                    let yaw_step = XR_DEPTH_ALIGN_WALL_PROFILE_YAW_STEP_RADIANS;
                    let yaw_steps = (XR_DEPTH_ALIGN_WALL_PROFILE_YAW_WINDOW_RADIANS / yaw_step)
                        .ceil()
                        .max(1.0) as isize;
                    self.stage = WallProfileTaskStage::SearchYaw {
                        current_profile_score,
                        best_profile_yaw: self.current.yaw_radians,
                        best_profile_score: current_profile_score,
                        next_yaw_index: -yaw_steps,
                        yaw_steps,
                    };
                    return None;
                }
                WallProfileTaskStage::SearchYaw {
                    current_profile_score,
                    best_profile_yaw,
                    best_profile_score,
                    next_yaw_index,
                    yaw_steps,
                } => {
                    if *next_yaw_index > *yaw_steps {
                        if wrap_angle(*best_profile_yaw - self.current.yaw_radians).abs()
                            < XR_DEPTH_ALIGN_WALL_PROFILE_MIN_YAW_DELTA_RADIANS
                        {
                            self.stage = WallProfileTaskStage::Done(self.current);
                            continue;
                        }
                        self.stage = WallProfileTaskStage::ScoreCorrectedInitial {
                            current_profile_score: *current_profile_score,
                            best_profile_yaw: *best_profile_yaw,
                            best_profile_score: *best_profile_score,
                        };
                        continue;
                    }
                    let yaw = wrap_angle(
                        self.current.yaw_radians
                            + *next_yaw_index as f32 * XR_DEPTH_ALIGN_WALL_PROFILE_YAW_STEP_RADIANS,
                    );
                    let score = score_wall_profile_alignment_with_translation_prior(
                        &self.local_profile,
                        &self.remote_profile,
                        yaw,
                        self.current.translation,
                    );
                    if score > *best_profile_score {
                        *best_profile_score = score;
                        *best_profile_yaw = yaw;
                    }
                    *next_yaw_index += 1;
                    return None;
                }
                WallProfileTaskStage::ScoreCorrectedInitial {
                    current_profile_score,
                    best_profile_yaw,
                    best_profile_score,
                } => {
                    let corrected_initial = score_full_alignment_solution(
                        local_signal,
                        remote_signal,
                        Some(local_map),
                        local_sample_cache,
                        Some(remote_map),
                        remote_dense_signal,
                        *best_profile_yaw,
                        vec3(
                            self.current.translation.x,
                            self.floor_y,
                            self.current.translation.z,
                        ),
                    );
                    let search = FullAlignmentSearchTask::new(
                        corrected_initial,
                        self.floor_y,
                        &XR_DEPTH_ALIGN_TRANSLATION_ONLY_SEARCH_PHASES,
                        true,
                    );
                    self.stage = WallProfileTaskStage::RefineTranslation {
                        current_profile_score: *current_profile_score,
                        best_profile_score: *best_profile_score,
                        search,
                    };
                    return None;
                }
                WallProfileTaskStage::RefineTranslation {
                    current_profile_score,
                    best_profile_score,
                    search,
                } => {
                    let Some(corrected) = search.step(
                        local_signal,
                        remote_signal,
                        Some(local_map),
                        local_sample_cache,
                        Some(remote_map),
                        remote_dense_signal,
                    ) else {
                        return None;
                    };
                    let result = if alignment_solution_better(&corrected, &self.current)
                        || wall_profile_yaw_sidecar_is_safe(
                            self.current,
                            corrected,
                            *current_profile_score,
                            *best_profile_score,
                        )
                    {
                        corrected
                    } else {
                        self.current
                    };
                    self.stage = WallProfileTaskStage::Done(result);
                    continue;
                }
                WallProfileTaskStage::Done(result) => return Some(*result),
            }
        }
    }
}

#[derive(Debug)]
enum XrDepthAlignMatcherState {
    BuildSignals,
    SeedStart,
    SeedHeightRefine(HeightMapRefineTask),
    SeedSignalRefine {
        best_yaw: f32,
        best_translation: Vec3f,
    },
    SeedScore {
        best_yaw: f32,
        best_translation: Vec3f,
    },
    SeedLocalSearch(FullAlignmentSearchTask),
    SeedWallProfile(WallProfileTask),
    SeedVerticalOffset {
        candidate: XrDepthAlignSolution,
    },
    SeedComplete {
        candidate: XrDepthAlignSolution,
    },
    BuildYawCandidates,
    BuildTranslations {
        yaw_index: usize,
    },
    ScoreTranslations {
        yaw_index: usize,
        yaw: f32,
        translations: Vec<Vec3f>,
        translation_index: usize,
        yaw_shortlist: Vec<XrDepthAlignSolution>,
    },
    HeightRefineCandidate {
        shortlist_index: usize,
        signal_candidate: XrDepthAlignSolution,
        task: HeightMapRefineTask,
    },
    FinalScoreCandidate {
        shortlist_index: usize,
        refined_yaw: f32,
        refined_translation: Vec3f,
    },
    FinalWallProfile(WallProfileTask),
    FinalVerticalOffset {
        candidate: XrDepthAlignSolution,
    },
    Finish,
    Done,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct XrDepthAlignMatcherProgress {
    pub stage_label: &'static str,
    pub step_count: u32,
    pub yaw_candidate_count: u32,
    pub current_yaw_index: u32,
    pub current_translation_index: u32,
    pub current_translation_count: u32,
    pub shortlist_count: u32,
    pub current_shortlist_index: u32,
}

#[derive(Debug)]
pub struct XrDepthAlignMatcher {
    local: XrDepthAlignDescriptor,
    remote: XrDepthAlignDescriptor,
    previous_solution: Option<XrDepthAlignSolution>,
    state: XrDepthAlignMatcherState,
    diagnostic: XrDepthAlignSolveDiagnostic,
    floor_y: f32,
    local_signal: Vec<HeightMapSignalCell>,
    remote_signal: Vec<HeightMapSignalCell>,
    remote_dense_signal: Vec<DenseHeightMapSignalCell>,
    remote_refine_signal: Vec<DenseHeightMapSignalCell>,
    local_histogram: Vec<f32>,
    remote_histogram: Vec<f32>,
    local_sample_cache: Option<HeightMapSampleCache>,
    use_height_refine: bool,
    yaw_candidates: Vec<f32>,
    shortlist: Vec<XrDepthAlignSolution>,
    best: Option<XrDepthAlignSolution>,
    translation_vote_time: std::time::Duration,
    signal_refine_time: std::time::Duration,
    signal_score_time: std::time::Duration,
    height_refine_time: std::time::Duration,
    final_score_time: std::time::Duration,
    wall_profile_time: std::time::Duration,
    total_compute_time: std::time::Duration,
    max_step_time: std::time::Duration,
}

impl XrDepthAlignMatcher {
    pub fn new(
        local: &XrDepthAlignDescriptor,
        remote: &XrDepthAlignDescriptor,
        previous_solution: Option<XrDepthAlignSolution>,
    ) -> Self {
        Self {
            local: local.clone(),
            remote: remote.clone(),
            previous_solution,
            state: XrDepthAlignMatcherState::BuildSignals,
            diagnostic: XrDepthAlignSolveDiagnostic::default(),
            floor_y: 0.0,
            local_signal: Vec::new(),
            remote_signal: Vec::new(),
            remote_dense_signal: Vec::new(),
            remote_refine_signal: Vec::new(),
            local_histogram: Vec::new(),
            remote_histogram: Vec::new(),
            local_sample_cache: None,
            use_height_refine: false,
            yaw_candidates: Vec::new(),
            shortlist: Vec::new(),
            best: None,
            translation_vote_time: std::time::Duration::ZERO,
            signal_refine_time: std::time::Duration::ZERO,
            signal_score_time: std::time::Duration::ZERO,
            height_refine_time: std::time::Duration::ZERO,
            final_score_time: std::time::Duration::ZERO,
            wall_profile_time: std::time::Duration::ZERO,
            total_compute_time: std::time::Duration::ZERO,
            max_step_time: std::time::Duration::ZERO,
        }
    }

    pub fn is_finished(&self) -> bool {
        matches!(self.state, XrDepthAlignMatcherState::Done)
    }

    pub fn diagnostic(&self) -> Option<XrDepthAlignSolveDiagnostic> {
        self.is_finished().then_some(self.diagnostic)
    }

    pub fn finish(mut self) -> XrDepthAlignSolveDiagnostic {
        while self.step() {}
        self.diagnostic
    }

    pub fn progress(&self) -> XrDepthAlignMatcherProgress {
        let mut progress = XrDepthAlignMatcherProgress {
            step_count: self.diagnostic.step_count,
            yaw_candidate_count: self.yaw_candidates.len() as u32,
            shortlist_count: self.shortlist.len() as u32,
            ..XrDepthAlignMatcherProgress::default()
        };
        match &self.state {
            XrDepthAlignMatcherState::BuildSignals => {
                progress.stage_label = "signals";
            }
            XrDepthAlignMatcherState::SeedStart => {
                progress.stage_label = "seed-start";
            }
            XrDepthAlignMatcherState::SeedHeightRefine(_) => {
                progress.stage_label = "seed-height";
            }
            XrDepthAlignMatcherState::SeedSignalRefine { .. } => {
                progress.stage_label = "seed-signal";
            }
            XrDepthAlignMatcherState::SeedScore { .. } => {
                progress.stage_label = "seed-score";
            }
            XrDepthAlignMatcherState::SeedLocalSearch(_) => {
                progress.stage_label = "seed-search";
            }
            XrDepthAlignMatcherState::SeedWallProfile(_) => {
                progress.stage_label = "seed-wall";
            }
            XrDepthAlignMatcherState::SeedVerticalOffset { .. } => {
                progress.stage_label = "seed-vertical";
            }
            XrDepthAlignMatcherState::SeedComplete { .. } => {
                progress.stage_label = "seed-complete";
            }
            XrDepthAlignMatcherState::BuildYawCandidates => {
                progress.stage_label = "yaw";
            }
            XrDepthAlignMatcherState::BuildTranslations { yaw_index } => {
                progress.stage_label = "vote";
                progress.current_yaw_index = (*yaw_index + 1) as u32;
            }
            XrDepthAlignMatcherState::ScoreTranslations {
                yaw_index,
                translations,
                translation_index,
                ..
            } => {
                progress.stage_label = "score";
                progress.current_yaw_index = (*yaw_index + 1) as u32;
                progress.current_translation_count = translations.len() as u32;
                progress.current_translation_index =
                    ((*translation_index).min(translations.len().saturating_sub(1)) + 1) as u32;
            }
            XrDepthAlignMatcherState::HeightRefineCandidate { shortlist_index, .. } => {
                progress.stage_label = "height";
                progress.current_shortlist_index = (*shortlist_index + 1) as u32;
            }
            XrDepthAlignMatcherState::FinalScoreCandidate { shortlist_index, .. } => {
                progress.stage_label = "final-score";
                progress.current_shortlist_index = (*shortlist_index + 1) as u32;
            }
            XrDepthAlignMatcherState::FinalWallProfile(_) => {
                progress.stage_label = "wall";
            }
            XrDepthAlignMatcherState::FinalVerticalOffset { .. } => {
                progress.stage_label = "vertical";
            }
            XrDepthAlignMatcherState::Finish => {
                progress.stage_label = "finish";
            }
            XrDepthAlignMatcherState::Done => {
                progress.stage_label = "done";
            }
        }
        progress
    }

    pub fn step_for_budget(&mut self, budget: std::time::Duration, max_steps: usize) -> u32 {
        if self.is_finished() || max_steps == 0 {
            return 0;
        }
        let started = std::time::Instant::now();
        let mut steps = 0u32;
        while steps < max_steps as u32 {
            if steps != 0 && !budget.is_zero() && started.elapsed() >= budget {
                break;
            }
            if !self.step() {
                break;
            }
            steps = steps.saturating_add(1);
        }
        steps
    }

    pub fn step(&mut self) -> bool {
        if self.is_finished() {
            return false;
        }
        let step_started = std::time::Instant::now();
        let did_work = loop {
            let state = std::mem::replace(&mut self.state, XrDepthAlignMatcherState::Done);
            match state {
                XrDepthAlignMatcherState::BuildSignals => {
                    let started = std::time::Instant::now();
                    self.local_signal = selected_descriptor_signal_cells(&self.local);
                    self.remote_signal = selected_descriptor_signal_cells(&self.remote);
                    self.remote_dense_signal = descriptor_dense_signal_cells(&self.remote);
                    self.remote_refine_signal = decimate_signal_cells(
                        &self.remote_dense_signal,
                        XR_DEPTH_ALIGN_HEIGHT_REFINE_MAX_DENSE_SAMPLES,
                    );
                    self.local_histogram = build_height_map_signal_histogram(
                        &self.local_signal,
                        XR_DEPTH_ALIGN_HEIGHT_MAP_HISTOGRAM_BINS,
                    );
                    self.remote_histogram = build_height_map_signal_histogram(
                        &self.remote_signal,
                        XR_DEPTH_ALIGN_HEIGHT_MAP_HISTOGRAM_BINS,
                    );
                    self.local_sample_cache =
                        self.local.height_map.as_ref().and_then(build_height_map_sample_cache);
                    self.floor_y = self.local.floor_y - self.remote.floor_y;
                    self.use_height_refine = self.local.height_map.is_some()
                        && self.remote.height_map.is_some()
                        && !self.remote_refine_signal.is_empty();
                    self.diagnostic = XrDepthAlignSolveDiagnostic {
                        local_vertical_descriptor: self.local.vertical_descriptor.is_some(),
                        remote_vertical_descriptor: self.remote.vertical_descriptor.is_some(),
                        local_wall_samples: self.local_signal.len(),
                        remote_wall_samples: self.remote_signal.len(),
                        remote_dense_wall_samples: self.remote_dense_signal.len(),
                        remote_refine_wall_samples: self.remote_refine_signal.len(),
                        signal_build_ms: duration_ms_u32(started.elapsed()),
                        ..XrDepthAlignSolveDiagnostic::default()
                    };
                    self.state = if self.local_signal.len() < XR_DEPTH_ALIGN_MIN_SIGNAL_SAMPLES
                        || self.remote_signal.len() < XR_DEPTH_ALIGN_MIN_SIGNAL_SAMPLES
                    {
                        XrDepthAlignMatcherState::Finish
                    } else if self.previous_solution.is_some() {
                        XrDepthAlignMatcherState::SeedStart
                    } else {
                        XrDepthAlignMatcherState::BuildYawCandidates
                    };
                    break true;
                }
                XrDepthAlignMatcherState::SeedStart => {
                    self.diagnostic.yaw_candidate_count += 1;
                    self.diagnostic.pose_candidate_count += 1;
                    let seed = self.previous_solution.expect("seeded state requires seed");
                    let translation = vec3(seed.translation.x, self.floor_y, seed.translation.z);
                    self.state = if self.use_height_refine {
                        XrDepthAlignMatcherState::SeedHeightRefine(HeightMapRefineTask::new(
                            seed.yaw_radians,
                            translation,
                        ))
                    } else {
                        XrDepthAlignMatcherState::SeedSignalRefine {
                            best_yaw: wrap_angle(seed.yaw_radians),
                            best_translation: translation,
                        }
                    };
                    continue;
                }
                XrDepthAlignMatcherState::SeedHeightRefine(mut task) => {
                    let started = std::time::Instant::now();
                    let Some(local_map) = self.local_map() else {
                        self.state = XrDepthAlignMatcherState::SeedSignalRefine {
                            best_yaw: task.best_yaw,
                            best_translation: task.best_translation,
                        };
                        continue;
                    };
                    let Some(remote_map) = self.remote_map() else {
                        self.state = XrDepthAlignMatcherState::SeedSignalRefine {
                            best_yaw: task.best_yaw,
                            best_translation: task.best_translation,
                        };
                        continue;
                    };
                    let completed = task.step(
                        local_map,
                        self.local_sample_cache.as_ref(),
                        remote_map,
                        &self.remote_refine_signal,
                    );
                    self.height_refine_time += started.elapsed();
                    if let Some((best_yaw, best_translation)) = completed {
                        self.state = XrDepthAlignMatcherState::SeedSignalRefine {
                            best_yaw,
                            best_translation,
                        };
                    } else {
                        self.state = XrDepthAlignMatcherState::SeedHeightRefine(task);
                    }
                    break true;
                }
                XrDepthAlignMatcherState::SeedSignalRefine {
                    best_yaw,
                    best_translation,
                } => {
                    let started = std::time::Instant::now();
                    let (signal_refined_yaw, signal_refined_translation) = refine_signal_alignment(
                        &self.local_signal,
                        &self.remote_signal,
                        self.floor_y,
                        best_yaw,
                        best_translation,
                    );
                    self.signal_refine_time += started.elapsed();
                    let signal_translation_jump = vec3(
                        signal_refined_translation.x - best_translation.x,
                        0.0,
                        signal_refined_translation.z - best_translation.z,
                    )
                    .length();
                    let (next_yaw, next_translation) =
                        if wrap_angle(signal_refined_yaw - best_yaw).abs() <= 0.18
                            && signal_translation_jump <= 0.28
                        {
                            (signal_refined_yaw, signal_refined_translation)
                        } else {
                            (best_yaw, best_translation)
                        };
                    self.state = XrDepthAlignMatcherState::SeedScore {
                        best_yaw: next_yaw,
                        best_translation: next_translation,
                    };
                    break true;
                }
                XrDepthAlignMatcherState::SeedScore {
                    best_yaw,
                    best_translation,
                } => {
                    let started = std::time::Instant::now();
                    let best = score_full_alignment_solution(
                        &self.local_signal,
                        &self.remote_signal,
                        self.local_map(),
                        self.local_sample_cache.as_ref(),
                        self.remote_map(),
                        &self.remote_dense_signal,
                        best_yaw,
                        best_translation,
                    );
                    self.final_score_time += started.elapsed();
                    self.state = XrDepthAlignMatcherState::SeedLocalSearch(
                        FullAlignmentSearchTask::new(
                            best,
                            self.floor_y,
                            &XR_DEPTH_ALIGN_SEEDED_LOCAL_SEARCH_PHASES,
                            false,
                        ),
                    );
                    break true;
                }
                XrDepthAlignMatcherState::SeedLocalSearch(mut task) => {
                    let started = std::time::Instant::now();
                    let completed = task.step(
                        &self.local_signal,
                        &self.remote_signal,
                        self.local_map(),
                        self.local_sample_cache.as_ref(),
                        self.remote_map(),
                        &self.remote_dense_signal,
                    );
                    self.final_score_time += started.elapsed();
                    if let Some(candidate) = completed {
                        self.state = if self.local_map().is_some() && self.remote_map().is_some() {
                            XrDepthAlignMatcherState::SeedWallProfile(WallProfileTask::new(
                                candidate,
                                self.floor_y,
                            ))
                        } else {
                            XrDepthAlignMatcherState::SeedComplete { candidate }
                        };
                    } else {
                        self.state = XrDepthAlignMatcherState::SeedLocalSearch(task);
                    }
                    break true;
                }
                XrDepthAlignMatcherState::SeedWallProfile(mut task) => {
                    let Some(local_map) = self.local_map() else {
                        self.state = XrDepthAlignMatcherState::SeedComplete {
                            candidate: task.current,
                        };
                        continue;
                    };
                    let Some(remote_map) = self.remote_map() else {
                        self.state = XrDepthAlignMatcherState::SeedComplete {
                            candidate: task.current,
                        };
                        continue;
                    };
                    let started = std::time::Instant::now();
                    let completed = task.step(
                        &self.local_signal,
                        &self.remote_signal,
                        local_map,
                        self.local_sample_cache.as_ref(),
                        remote_map,
                        &self.remote_dense_signal,
                    );
                    self.wall_profile_time += started.elapsed();
                    if let Some(candidate) = completed {
                        self.state = XrDepthAlignMatcherState::SeedVerticalOffset { candidate };
                    } else {
                        self.state = XrDepthAlignMatcherState::SeedWallProfile(task);
                    }
                    break true;
                }
                XrDepthAlignMatcherState::SeedVerticalOffset { candidate } => {
                    let started = std::time::Instant::now();
                    let candidate =
                        if let (Some(local_map), Some(remote_map)) = (self.local_map(), self.remote_map())
                        {
                            refine_solution_vertical_offset_from_overlap(
                                local_map,
                                remote_map,
                                &self.remote_dense_signal,
                                candidate,
                            )
                        } else {
                            candidate
                        };
                    self.final_score_time += started.elapsed();
                    self.state = XrDepthAlignMatcherState::SeedComplete { candidate };
                    break true;
                }
                XrDepthAlignMatcherState::SeedComplete { candidate } => {
                    self.best = Some(candidate);
                    if let Some(seed) = self.previous_solution {
                        if seeded_alignment_lock_is_strong(
                            &self.diagnostic,
                            candidate,
                            seed,
                            self.remote_signal.len(),
                            self.local_map(),
                            self.remote_map(),
                        ) {
                            self.state = XrDepthAlignMatcherState::Finish;
                            continue;
                        }
                    }
                    self.state = XrDepthAlignMatcherState::BuildYawCandidates;
                    continue;
                }
                XrDepthAlignMatcherState::BuildYawCandidates => {
                    let started = std::time::Instant::now();
                    self.yaw_candidates = candidate_signal_yaws(
                        &self.local_histogram,
                        &self.remote_histogram,
                        &self.local_signal,
                        &self.remote_signal,
                    );
                    self.diagnostic.yaw_candidate_ms = duration_ms_u32(started.elapsed());
                    self.diagnostic.yaw_candidate_count = self.yaw_candidates.len();
                    self.state = if self.yaw_candidates.is_empty() {
                        XrDepthAlignMatcherState::Finish
                    } else {
                        XrDepthAlignMatcherState::BuildTranslations { yaw_index: 0 }
                    };
                    break true;
                }
                XrDepthAlignMatcherState::BuildTranslations { yaw_index } => {
                    let yaw = self.yaw_candidates[yaw_index];
                    let started = std::time::Instant::now();
                    let translations = candidate_signal_translations(
                        &self.local_signal,
                        &self.remote_signal,
                        self.floor_y,
                        yaw,
                    );
                    self.translation_vote_time += started.elapsed();
                    self.diagnostic.pose_candidate_count += translations.len();
                    self.state = XrDepthAlignMatcherState::ScoreTranslations {
                        yaw_index,
                        yaw,
                        translations,
                        translation_index: 0,
                        yaw_shortlist: Vec::new(),
                    };
                    break true;
                }
                XrDepthAlignMatcherState::ScoreTranslations {
                    yaw_index,
                    yaw,
                    translations,
                    translation_index,
                    mut yaw_shortlist,
                } => {
                    if translation_index >= translations.len() {
                        for signal_candidate in yaw_shortlist {
                            push_shortlisted_alignment_solution(
                                &mut self.shortlist,
                                signal_candidate,
                                XR_DEPTH_ALIGN_SIGNAL_SHORTLIST_MAX_CANDIDATES,
                                XR_DEPTH_ALIGN_SIGNAL_SHORTLIST_TRANSLATION_EPSILON_METERS,
                                XR_DEPTH_ALIGN_SIGNAL_SHORTLIST_YAW_EPSILON_RADIANS,
                            );
                        }
                        self.state = if yaw_index + 1 < self.yaw_candidates.len() {
                            XrDepthAlignMatcherState::BuildTranslations {
                                yaw_index: yaw_index + 1,
                            }
                        } else if self.use_height_refine && !self.shortlist.is_empty() {
                            self.diagnostic.shortlisted_pose_count = self.shortlist.len();
                            let signal_candidate = self.shortlist[0];
                            XrDepthAlignMatcherState::HeightRefineCandidate {
                                shortlist_index: 0,
                                signal_candidate,
                                task: HeightMapRefineTask::new(
                                    signal_candidate.yaw_radians,
                                    signal_candidate.translation,
                                ),
                            }
                        } else if let Some(best) = self.best {
                            if self.local_map().is_some() && self.remote_map().is_some() {
                                XrDepthAlignMatcherState::FinalWallProfile(WallProfileTask::new(
                                    best,
                                    self.floor_y,
                                ))
                            } else {
                                XrDepthAlignMatcherState::Finish
                            }
                        } else {
                            XrDepthAlignMatcherState::Finish
                        };
                        continue;
                    }

                    let translation = translations[translation_index];
                    let signal_refine_started = std::time::Instant::now();
                    let (refined_yaw, refined_translation) = refine_signal_alignment(
                        &self.local_signal,
                        &self.remote_signal,
                        self.floor_y,
                        yaw,
                        translation,
                    );
                    self.signal_refine_time += signal_refine_started.elapsed();

                    let signal_score_started = std::time::Instant::now();
                    let signal_candidate = score_signal_alignment_solution(
                        &self.local_signal,
                        &self.remote_signal,
                        refined_yaw,
                        refined_translation,
                    );
                    self.signal_score_time += signal_score_started.elapsed();

                    if self.use_height_refine {
                        push_shortlisted_alignment_solution(
                            &mut yaw_shortlist,
                            signal_candidate,
                            XR_DEPTH_ALIGN_SIGNAL_SHORTLIST_PER_YAW,
                            XR_DEPTH_ALIGN_SIGNAL_SHORTLIST_TRANSLATION_EPSILON_METERS,
                            XR_DEPTH_ALIGN_SIGNAL_SHORTLIST_YAW_EPSILON_RADIANS,
                        );
                    } else if self
                        .best
                        .as_ref()
                        .is_none_or(|current| alignment_solution_better(&signal_candidate, current))
                    {
                        self.best = Some(signal_candidate);
                    }
                    self.state = XrDepthAlignMatcherState::ScoreTranslations {
                        yaw_index,
                        yaw,
                        translations,
                        translation_index: translation_index + 1,
                        yaw_shortlist,
                    };
                    break true;
                }
                XrDepthAlignMatcherState::HeightRefineCandidate {
                    shortlist_index,
                    signal_candidate,
                    mut task,
                } => {
                    let Some(local_map) = self.local_map() else {
                        self.state = XrDepthAlignMatcherState::FinalScoreCandidate {
                            shortlist_index,
                            refined_yaw: signal_candidate.yaw_radians,
                            refined_translation: signal_candidate.translation,
                        };
                        continue;
                    };
                    let Some(remote_map) = self.remote_map() else {
                        self.state = XrDepthAlignMatcherState::FinalScoreCandidate {
                            shortlist_index,
                            refined_yaw: signal_candidate.yaw_radians,
                            refined_translation: signal_candidate.translation,
                        };
                        continue;
                    };
                    let started = std::time::Instant::now();
                    let completed = task.step(
                        local_map,
                        self.local_sample_cache.as_ref(),
                        remote_map,
                        &self.remote_refine_signal,
                    );
                    self.height_refine_time += started.elapsed();
                    if let Some((refined_yaw, refined_translation)) = completed {
                        self.state = XrDepthAlignMatcherState::FinalScoreCandidate {
                            shortlist_index,
                            refined_yaw,
                            refined_translation,
                        };
                    } else {
                        self.state = XrDepthAlignMatcherState::HeightRefineCandidate {
                            shortlist_index,
                            signal_candidate,
                            task,
                        };
                    }
                    break true;
                }
                XrDepthAlignMatcherState::FinalScoreCandidate {
                    shortlist_index,
                    refined_yaw,
                    refined_translation,
                } => {
                    let started = std::time::Instant::now();
                    let candidate = score_full_alignment_solution(
                        &self.local_signal,
                        &self.remote_signal,
                        self.local_map(),
                        self.local_sample_cache.as_ref(),
                        self.remote_map(),
                        &self.remote_dense_signal,
                        refined_yaw,
                        vec3(refined_translation.x, self.floor_y, refined_translation.z),
                    );
                    self.final_score_time += started.elapsed();
                    if self
                        .best
                        .as_ref()
                        .is_none_or(|current| alignment_solution_better(&candidate, current))
                    {
                        self.best = Some(candidate);
                    }
                    self.state = if shortlist_index + 1 < self.shortlist.len() {
                        let signal_candidate = self.shortlist[shortlist_index + 1];
                        XrDepthAlignMatcherState::HeightRefineCandidate {
                            shortlist_index: shortlist_index + 1,
                            signal_candidate,
                            task: HeightMapRefineTask::new(
                                signal_candidate.yaw_radians,
                                signal_candidate.translation,
                            ),
                        }
                    } else if let Some(best) = self.best {
                        if self.local_map().is_some() && self.remote_map().is_some() {
                            XrDepthAlignMatcherState::FinalWallProfile(WallProfileTask::new(
                                best,
                                self.floor_y,
                            ))
                        } else {
                            XrDepthAlignMatcherState::Finish
                        }
                    } else {
                        XrDepthAlignMatcherState::Finish
                    };
                    break true;
                }
                XrDepthAlignMatcherState::FinalWallProfile(mut task) => {
                    let Some(local_map) = self.local_map() else {
                        self.state = XrDepthAlignMatcherState::Finish;
                        continue;
                    };
                    let Some(remote_map) = self.remote_map() else {
                        self.state = XrDepthAlignMatcherState::Finish;
                        continue;
                    };
                    let started = std::time::Instant::now();
                    let completed = task.step(
                        &self.local_signal,
                        &self.remote_signal,
                        local_map,
                        self.local_sample_cache.as_ref(),
                        remote_map,
                        &self.remote_dense_signal,
                    );
                    self.wall_profile_time += started.elapsed();
                    if let Some(candidate) = completed {
                        self.state = XrDepthAlignMatcherState::FinalVerticalOffset { candidate };
                    } else {
                        self.state = XrDepthAlignMatcherState::FinalWallProfile(task);
                    }
                    break true;
                }
                XrDepthAlignMatcherState::FinalVerticalOffset { candidate } => {
                    let started = std::time::Instant::now();
                    self.best = Some(
                        if let (Some(local_map), Some(remote_map)) = (self.local_map(), self.remote_map())
                        {
                            refine_solution_vertical_offset_from_overlap(
                                local_map,
                                remote_map,
                                &self.remote_dense_signal,
                                candidate,
                            )
                        } else {
                            candidate
                        },
                    );
                    self.final_score_time += started.elapsed();
                    self.state = XrDepthAlignMatcherState::Finish;
                    break true;
                }
                XrDepthAlignMatcherState::Finish => {
                    self.finalize_diagnostic();
                    self.state = XrDepthAlignMatcherState::Done;
                    continue;
                }
                XrDepthAlignMatcherState::Done => {
                    self.state = XrDepthAlignMatcherState::Done;
                    return false;
                }
            }
        };
        if did_work {
            let elapsed = step_started.elapsed();
            self.total_compute_time += elapsed;
            self.max_step_time = self.max_step_time.max(elapsed);
            self.diagnostic.step_count = self.diagnostic.step_count.saturating_add(1);
        }
        did_work
    }

    fn local_map(&self) -> Option<&XrDepthAlignHeightMap> {
        self.local.height_map.as_ref()
    }

    fn remote_map(&self) -> Option<&XrDepthAlignHeightMap> {
        self.remote.height_map.as_ref()
    }

    fn finalize_diagnostic(&mut self) {
        self.diagnostic.translation_vote_ms = duration_ms_u32(self.translation_vote_time);
        self.diagnostic.signal_refine_ms = duration_ms_u32(self.signal_refine_time);
        self.diagnostic.signal_score_ms = duration_ms_u32(self.signal_score_time);
        self.diagnostic.height_refine_ms = duration_ms_u32(self.height_refine_time);
        self.diagnostic.final_score_ms = duration_ms_u32(self.final_score_time);
        self.diagnostic.wall_profile_ms = duration_ms_u32(self.wall_profile_time);
        self.diagnostic.total_compute_ms = duration_ms_u32(self.total_compute_time);
        self.diagnostic.max_step_ms = duration_ms_u32(self.max_step_time);
        self.diagnostic.best_solution = self.best;
    }
}

fn transform_height_map(
    height_map: &XrDepthAlignHeightMap,
    transform: &Mat4f,
) -> Option<XrDepthAlignHeightMap> {
    let size_x = height_map.size_x_usize();
    let size_z = height_map.size_z_usize();
    if size_x == 0 || size_z == 0 || height_map.heights_meters.len() != size_x * size_z {
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
        floor_y_meters: height_map.floor_y_meters + y_offset,
        player_cutout_center: height_map.player_cutout_center.map(|center| {
            let mapped = transform
                .transform_vec4(vec4f(center.x, 0.0, center.y, 1.0))
                .to_vec3f();
            vec2f(mapped.x, mapped.z)
        }),
        player_cutout_radius_meters: height_map.player_cutout_radius_meters,
        heights_meters: vec![f32::NAN; target_size_x * target_size_z],
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
            transformed.heights_meters[target_index] = height + y_offset;
        }
    }
    Some(transformed)
}

fn build_wall_profile_contour_points(height_map: &XrDepthAlignHeightMap) -> Vec<(f32, f32)> {
    let coarse_cell = XR_DEPTH_ALIGN_WALL_PROFILE_CELL_METERS
        .max(height_map.cell_size_meters)
        .max(1.0e-3);
    let size_x = ((height_map.extent_x_meters() / coarse_cell).ceil() as usize).max(1);
    let size_z = ((height_map.extent_z_meters() / coarse_cell).ceil() as usize).max(1);
    let mut occupied = vec![false; size_x * size_z];
    let cutout_center = height_map.player_cutout_center;
    let cutout_radius = (height_map.player_cutout_radius_meters + height_map.cell_size_meters)
        .max(height_map.cell_size_meters * 2.0);
    for src_z in 0..height_map.size_z_usize() {
        for src_x in 0..height_map.size_x_usize() {
            let index = height_map.cell_index(src_x, src_z);
            let Some(height) = height_map_cell_height(height_map, index) else {
                continue;
            };
            let world_x = height_map.origin_x + (src_x as f32 + 0.5) * height_map.cell_size_meters;
            let world_z = height_map.origin_z + (src_z as f32 + 0.5) * height_map.cell_size_meters;
            if cutout_center.is_some_and(|center| {
                let dx = world_x - center.x;
                let dz = world_z - center.y;
                (dx * dx + dz * dz).sqrt() <= cutout_radius
            }) {
                continue;
            }
            let relative_height = height - height_map.floor_y_meters;
            if relative_height < XR_DEPTH_ALIGN_WALL_PROFILE_MIN_HEIGHT_METERS
                || relative_height > XR_DEPTH_ALIGN_WALL_PROFILE_MAX_HEIGHT_METERS
            {
                continue;
            }
            let coarse_x = ((world_x - height_map.origin_x) / coarse_cell)
                .floor()
                .clamp(0.0, size_x.saturating_sub(1) as f32) as usize;
            let coarse_z = ((world_z - height_map.origin_z) / coarse_cell)
                .floor()
                .clamp(0.0, size_z.saturating_sub(1) as f32) as usize;
            occupied[coarse_x + coarse_z * size_x] = true;
        }
    }

    let mut contour_points = Vec::new();
    for z in 0..size_z {
        for x in 0..size_x {
            let index = x + z * size_x;
            if !occupied[index] {
                continue;
            }
            let mut is_edge = false;
            for (dx, dz) in [(-1isize, 0isize), (1, 0), (0, -1), (0, 1)] {
                let nx = x as isize + dx;
                let nz = z as isize + dz;
                if nx < 0
                    || nz < 0
                    || nx >= size_x as isize
                    || nz >= size_z as isize
                    || !occupied[nx as usize + nz as usize * size_x]
                {
                    is_edge = true;
                    break;
                }
            }
            if is_edge {
                contour_points.push((
                    height_map.origin_x + (x as f32 + 0.5) * coarse_cell,
                    height_map.origin_z + (z as f32 + 0.5) * coarse_cell,
                ));
            }
        }
    }
    if contour_points.len() >= XR_DEPTH_ALIGN_WALL_PROFILE_MIN_POINTS {
        contour_points
    } else {
        Vec::new()
    }
}

fn projection_bounds(
    local_points: &[(f32, f32)],
    remote_points: &[(f32, f32)],
    axis_x: f32,
    axis_z: f32,
) -> Option<ProjectionBounds> {
    let mut min_projection = f32::INFINITY;
    let mut max_projection = f32::NEG_INFINITY;
    for &(x, z) in local_points.iter().chain(remote_points.iter()) {
        let projection = x * axis_x + z * axis_z;
        min_projection = min_projection.min(projection);
        max_projection = max_projection.max(projection);
    }
    (min_projection.is_finite() && max_projection.is_finite()).then_some(ProjectionBounds {
        min: min_projection,
        max: max_projection,
    })
}

fn projection_histogram_overlap(
    local_points: &[(f32, f32)],
    remote_points: &[(f32, f32)],
    axis_x: f32,
    axis_z: f32,
    bin_size_meters: f32,
) -> f32 {
    let Some(bounds) = projection_bounds(local_points, remote_points, axis_x, axis_z) else {
        return 0.0;
    };
    let bin_size = bin_size_meters.max(0.04);
    let bin_count = (((bounds.max - bounds.min) / bin_size).ceil() as usize).max(1) + 1;
    let mut local_hist = vec![0.0f32; bin_count];
    let mut remote_hist = vec![0.0f32; bin_count];
    for &(x, z) in local_points {
        let projection = x * axis_x + z * axis_z;
        let bin = ((projection - bounds.min) / bin_size)
            .floor()
            .clamp(0.0, bin_count.saturating_sub(1) as f32) as usize;
        local_hist[bin] += 1.0;
    }
    for &(x, z) in remote_points {
        let projection = x * axis_x + z * axis_z;
        let bin = ((projection - bounds.min) / bin_size)
            .floor()
            .clamp(0.0, bin_count.saturating_sub(1) as f32) as usize;
        remote_hist[bin] += 1.0;
    }
    let mut intersection = 0.0f32;
    let mut union = 0.0f32;
    for index in 0..bin_count {
        intersection += local_hist[index].min(remote_hist[index]);
        union += local_hist[index].max(remote_hist[index]);
    }
    intersection / union.max(1.0e-6)
}

fn score_wall_profile_alignment_at_translation(
    local_points: &[(f32, f32)],
    remote_points: &[(f32, f32)],
    yaw: f32,
    translation_x: f32,
    translation_z: f32,
    bin_size_meters: f32,
) -> f32 {
    if local_points.len() < XR_DEPTH_ALIGN_WALL_PROFILE_MIN_POINTS
        || remote_points.len() < XR_DEPTH_ALIGN_WALL_PROFILE_MIN_POINTS
    {
        return 0.0;
    }
    let transformed_remote = remote_points
        .iter()
        .map(|&(x, z)| {
            let (rx, rz) = rotate_xz(yaw, x, z);
            (rx + translation_x, rz + translation_z)
        })
        .collect::<Vec<_>>();
    let (axis_z, axis_x) = yaw.sin_cos();
    let parallel = projection_histogram_overlap(
        local_points,
        &transformed_remote,
        axis_x,
        axis_z,
        bin_size_meters,
    );
    let perpendicular = projection_histogram_overlap(
        local_points,
        &transformed_remote,
        -axis_z,
        axis_x,
        bin_size_meters,
    );
    (parallel * perpendicular).sqrt()
}

fn score_wall_profile_alignment_with_translation_prior(
    local_points: &[(f32, f32)],
    remote_points: &[(f32, f32)],
    yaw: f32,
    base_translation: Vec3f,
) -> f32 {
    let step = XR_DEPTH_ALIGN_WALL_PROFILE_TRANSLATION_STEP_METERS.max(1.0e-3);
    let window = XR_DEPTH_ALIGN_WALL_PROFILE_TRANSLATION_WINDOW_METERS.max(step);
    let steps = (window / step).ceil().max(1.0) as isize;
    let mut best = 0.0f32;
    for x_step in -steps..=steps {
        let translation_x = base_translation.x + x_step as f32 * step;
        for z_step in -steps..=steps {
            let translation_z = base_translation.z + z_step as f32 * step;
            let translation_delta = ((translation_x - base_translation.x).powi(2)
                + (translation_z - base_translation.z).powi(2))
            .sqrt();
            let prior = (-0.5
                * (translation_delta / XR_DEPTH_ALIGN_WALL_PROFILE_TRANSLATION_SIGMA_METERS)
                    .powi(2))
            .exp()
            .clamp(0.0, 1.0);
            let score = score_wall_profile_alignment_at_translation(
                local_points,
                remote_points,
                yaw,
                translation_x,
                translation_z,
                XR_DEPTH_ALIGN_WALL_PROFILE_CELL_METERS,
            ) * prior;
            best = best.max(score);
        }
    }
    best
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

fn height_map_cell_height(height_map: &XrDepthAlignHeightMap, index: usize) -> Option<f32> {
    height_map
        .heights_meters
        .get(index)
        .copied()
        .filter(|height| height.is_finite())
}

fn height_map_signal_weight(
    height_map: &XrDepthAlignHeightMap,
    height: f32,
    gradient_magnitude: f32,
    straightness: f32,
) -> f32 {
    let height_span = (height_map.top_y_meters - height_map.bottom_y_meters).max(1.0e-3);
    let height_bias = ((height - height_map.bottom_y_meters) / height_span).clamp(0.0, 1.0);
    (gradient_magnitude * height_map.cell_size_meters.max(1.0e-3) * 2.0).clamp(0.08, 2.8)
        * (0.85 + 0.15 * height_bias)
        * (0.75 + 0.60 * straightness.clamp(0.0, 1.0))
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
    let center = height_map_cell_height(height_map, height_map.cell_index(x, z))?;
    let left = height_map_cell_height(height_map, height_map.cell_index(x - 1, z))?;
    let right = height_map_cell_height(height_map, height_map.cell_index(x + 1, z))?;
    let up = height_map_cell_height(height_map, height_map.cell_index(x, z - 1))?;
    let down = height_map_cell_height(height_map, height_map.cell_index(x, z + 1))?;
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
    if size_x < 3 || size_z < 3 || height_map.heights_meters.len() != size_x * size_z {
        return Vec::new();
    }
    let mut signal = Vec::<HeightMapSignalCell>::new();
    for z in (1..size_z - 1).step_by(2) {
        for x in (1..size_x - 1).step_by(2) {
            let Some((height, gradient)) = height_map_cell_signal(height_map, x, z) else {
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
            let weight = height_map_signal_weight(height_map, height, magnitude, straightness);
            if weight < 0.10 {
                continue;
            }
            signal.push(HeightMapSignalCell {
                point: vec3(
                    height_map.origin_x + (x as f32 + 0.5) * height_map.cell_size_meters,
                    0.0,
                    height_map.origin_z + (z as f32 + 0.5) * height_map.cell_size_meters,
                ),
                height,
                gradient,
                axis_xz: vec2f(normal.x, normal.z),
                weight,
            });
        }
    }
    signal
}

fn select_height_map_alignment_signal_cells(
    signal_cells: &[HeightMapSignalCell],
) -> Vec<HeightMapSignalCell> {
    let mut candidates = signal_cells.to_vec();
    candidates.sort_by(|a, b| b.weight.total_cmp(&a.weight));
    let mut selected = Vec::<HeightMapSignalCell>::new();
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

fn build_height_map_signal_histogram(
    signal_cells: &[HeightMapSignalCell],
    bin_count: usize,
) -> Vec<f32> {
    let bin_count = bin_count.max(1);
    let mut histogram = vec![0.0; bin_count];
    for cell in signal_cells {
        let angle = cell.axis_xz.x.atan2(-cell.axis_xz.y);
        let normalized = (angle + std::f32::consts::PI) / std::f32::consts::TAU;
        let bin = (normalized * bin_count as f32).floor() as isize;
        histogram[bin.rem_euclid(bin_count as isize) as usize] += cell.weight.max(0.01);
    }
    normalize_histogram(&mut histogram);
    histogram
}

fn normalize_histogram(histogram: &mut [f32]) {
    let total = histogram.iter().copied().sum::<f32>();
    if total > 0.0 {
        for value in histogram {
            *value = (*value / total * 100.0).round() / 100.0;
        }
    }
}

fn decimate_signal_cells<T: Copy>(signal_cells: &[T], max_samples: usize) -> Vec<T> {
    if signal_cells.len() <= max_samples {
        return signal_cells.to_vec();
    }
    let stride = signal_cells.len().div_ceil(max_samples);
    signal_cells.iter().copied().step_by(stride.max(1)).collect()
}

fn build_height_map_sample_cache(height_map: &XrDepthAlignHeightMap) -> Option<HeightMapSampleCache> {
    let size_x = height_map.size_x_usize();
    let size_z = height_map.size_z_usize();
    if size_x < 3 || size_z < 3 || height_map.heights_meters.len() != size_x * size_z {
        return None;
    }
    let mut signal_axes = vec![vec2f(f32::NAN, f32::NAN); size_x * size_z];
    for z in 1..size_z - 1 {
        for x in 1..size_x - 1 {
            let Some((_height, gradient)) = height_map_cell_signal(height_map, x, z) else {
                continue;
            };
            let Some(axis) = xz_axis(gradient) else {
                continue;
            };
            let index = height_map.cell_index(x, z);
            signal_axes[index] = vec2f(axis.x, axis.z);
        }
    }
    let cell_size_meters = height_map.cell_size_meters.max(1.0e-3);
    Some(HeightMapSampleCache {
        origin_x: height_map.origin_x,
        origin_z: height_map.origin_z,
        inv_cell_size_meters: cell_size_meters.recip(),
        size_x,
        size_z,
        signal_axes,
    })
}

fn descriptor_signal_cells(descriptor: &XrDepthAlignDescriptor) -> Vec<HeightMapSignalCell> {
    descriptor
        .height_map
        .as_ref()
        .map(build_height_map_signal_cells)
        .unwrap_or_default()
}

fn descriptor_dense_signal_cells(descriptor: &XrDepthAlignDescriptor) -> Vec<DenseHeightMapSignalCell> {
    descriptor_signal_cells(descriptor)
        .into_iter()
        .map(|cell| DenseHeightMapSignalCell {
            point_x: cell.point.x,
            point_z: cell.point.z,
            height: cell.height,
            axis_x: cell.axis_xz.x,
            axis_z: cell.axis_xz.y,
            weight: cell.weight,
        })
        .collect()
}

fn selected_descriptor_signal_cells(
    descriptor: &XrDepthAlignDescriptor,
) -> Vec<HeightMapSignalCell> {
    select_height_map_alignment_signal_cells(&descriptor_signal_cells(descriptor))
}

fn sample_height_map_nearest(
    height_map: &XrDepthAlignHeightMap,
    world_x: f32,
    world_z: f32,
) -> Option<f32> {
    let size_x = height_map.size_x_usize();
    let size_z = height_map.size_z_usize();
    if size_x == 0 || size_z == 0 || height_map.heights_meters.len() != size_x * size_z {
        return None;
    }
    let cell_size = height_map.cell_size_meters.max(1.0e-3);
    let grid_x = ((world_x - height_map.origin_x) / cell_size).floor() as isize;
    let grid_z = ((world_z - height_map.origin_z) / cell_size).floor() as isize;
    if grid_x < 0 || grid_z < 0 || grid_x >= size_x as isize || grid_z >= size_z as isize {
        return None;
    }
    height_map_cell_height(
        height_map,
        height_map.cell_index(grid_x as usize, grid_z as usize),
    )
}

fn sample_height_map_bilinear(
    height_map: &XrDepthAlignHeightMap,
    world_x: f32,
    world_z: f32,
) -> Option<f32> {
    let size_x = height_map.size_x_usize();
    let size_z = height_map.size_z_usize();
    if size_x == 0 || size_z == 0 || height_map.heights_meters.len() != size_x * size_z {
        return None;
    }
    if size_x == 1 && size_z == 1 {
        return height_map_cell_height(height_map, 0);
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

    let h00 = height_map_cell_height(height_map, height_map.cell_index(x0, z0));
    let h10 = height_map_cell_height(height_map, height_map.cell_index(x1, z0));
    let h01 = height_map_cell_height(height_map, height_map.cell_index(x0, z1));
    let h11 = height_map_cell_height(height_map, height_map.cell_index(x1, z1));
    match (h00, h10, h01, h11) {
        (Some(h00), Some(h10), Some(h01), Some(h11)) => {
            let hx0 = h00 + (h10 - h00) * fx;
            let hx1 = h01 + (h11 - h01) * fx;
            Some(hx0 + (hx1 - hx0) * fz)
        }
        _ => sample_height_map_nearest(height_map, world_x, world_z),
    }
}

#[inline(always)]
fn sample_height_map_bilinear_and_axis_fast(
    height_map: &XrDepthAlignHeightMap,
    sample_cache: &HeightMapSampleCache,
    world_x: f32,
    world_z: f32,
) -> Option<(f32, Option<Vec2f>)> {
    let size_x = sample_cache.size_x;
    let size_z = sample_cache.size_z;
    let heights = &height_map.heights_meters;
    if size_x == 0 || size_z == 0 || heights.len() != size_x * size_z {
        return None;
    }
    if size_x == 1 && size_z == 1 {
        let height = *heights.first()?;
        return height.is_finite().then_some((height, None));
    }

    let raw_x = (world_x - sample_cache.origin_x) * sample_cache.inv_cell_size_meters;
    let raw_z = (world_z - sample_cache.origin_z) * sample_cache.inv_cell_size_meters;
    let sample_x = (raw_x - 0.5).clamp(0.0, size_x as f32 - 1.0);
    let sample_z = (raw_z - 0.5).clamp(0.0, size_z as f32 - 1.0);
    let x0 = sample_x.floor() as usize;
    let z0 = sample_z.floor() as usize;
    let x1 = (x0 + 1).min(size_x - 1);
    let z1 = (z0 + 1).min(size_z - 1);
    let fx = (sample_x - x0 as f32).clamp(0.0, 1.0);
    let fz = (sample_z - z0 as f32).clamp(0.0, 1.0);

    let index00 = x0 + z0 * size_x;
    let index10 = x1 + z0 * size_x;
    let index01 = x0 + z1 * size_x;
    let index11 = x1 + z1 * size_x;
    let h00 = heights[index00];
    let h10 = heights[index10];
    let h01 = heights[index01];
    let h11 = heights[index11];
    let height = if h00.is_finite() && h10.is_finite() && h01.is_finite() && h11.is_finite() {
        let hx0 = h00 + (h10 - h00) * fx;
        let hx1 = h01 + (h11 - h01) * fx;
        hx0 + (hx1 - hx0) * fz
    } else {
        let grid_x = raw_x.floor() as isize;
        let grid_z = raw_z.floor() as isize;
        if grid_x < 0 || grid_z < 0 || grid_x >= size_x as isize || grid_z >= size_z as isize {
            return None;
        }
        let height = heights[grid_x as usize + grid_z as usize * size_x];
        if !height.is_finite() {
            return None;
        }
        height
    };

    let axis = {
        let grid_x = raw_x.round() as isize;
        let grid_z = raw_z.round() as isize;
        if grid_x <= 0
            || grid_z <= 0
            || grid_x + 1 >= size_x as isize
            || grid_z + 1 >= size_z as isize
        {
            None
        } else {
            let index = grid_x as usize + grid_z as usize * size_x;
            let axis = sample_cache.signal_axes[index];
            (axis.x.is_finite() && axis.y.is_finite()).then_some(axis)
        }
    };

    Some((height, axis))
}

fn score_height_map_alignment(
    local_map: &XrDepthAlignHeightMap,
    local_sample_cache: Option<&HeightMapSampleCache>,
    remote_map: &XrDepthAlignHeightMap,
    remote_signal: &[DenseHeightMapSignalCell],
    yaw: f32,
    translation: Vec3f,
) -> (f32, f32, usize) {
    score_height_map_alignment_with_stride(
        local_map,
        local_sample_cache,
        remote_map,
        remote_signal,
        yaw,
        translation,
        1,
    )
}

fn fill_rotated_height_map_signal(
    rotated: &mut Vec<RotatedHeightMapSignalCell>,
    remote_signal: &[DenseHeightMapSignalCell],
    yaw: f32,
) {
    let (sin_yaw, cos_yaw) = yaw.sin_cos();
    rotated.clear();
    rotated.reserve(remote_signal.len());
    fill_rotated_height_map_signal_scalar(rotated, remote_signal, sin_yaw, cos_yaw)
}

fn fill_rotated_height_map_signal_scalar(
    rotated: &mut Vec<RotatedHeightMapSignalCell>,
    remote_signal: &[DenseHeightMapSignalCell],
    sin_yaw: f32,
    cos_yaw: f32,
) {
    for cell in remote_signal {
        let (point_x, point_z) =
            rotate_xz_quat_with_sin_cos(sin_yaw, cos_yaw, cell.point_x, cell.point_z);
        let (axis_x, axis_z) =
            rotate_xz_quat_with_sin_cos(sin_yaw, cos_yaw, cell.axis_x, cell.axis_z);
        rotated.push(RotatedHeightMapSignalCell {
            point_x,
            point_z,
            axis_x,
            axis_z,
            height: cell.height,
            weight: cell.weight,
        });
    }
}

fn score_height_map_alignment_with_stride(
    local_map: &XrDepthAlignHeightMap,
    local_sample_cache: Option<&HeightMapSampleCache>,
    remote_map: &XrDepthAlignHeightMap,
    remote_signal: &[DenseHeightMapSignalCell],
    yaw: f32,
    translation: Vec3f,
    sample_stride: usize,
) -> (f32, f32, usize) {
    if remote_signal.is_empty() {
        return (0.0, f32::INFINITY, 0);
    }
    let (sin_yaw, cos_yaw) = yaw.sin_cos();
    let mapped_remote_cutout_center = remote_map
        .player_cutout_center
        .map(|center| rotate_xz_quat_with_sin_cos(sin_yaw, cos_yaw, center.x, center.y));
    let mapped_remote_cutout_center = mapped_remote_cutout_center
        .map(|(center_x, center_z)| (center_x + translation.x, center_z + translation.z));
    let mapped_remote_cutout_radius =
        (remote_map.player_cutout_radius_meters + 0.14).max(remote_map.cell_size_meters * 2.0);
    let mapped_remote_cutout_radius_sq = mapped_remote_cutout_radius * mapped_remote_cutout_radius;
    let mut support_sum = 0.0;
    let mut weight_sum = 0.0;
    let mut total_weight = 0.0;
    let mut residual_sum = 0.0;
    let mut matched = 0usize;
    let sample_stride = sample_stride.max(1);
    if let Some(sample_cache) = local_sample_cache {
        for cell in remote_signal.iter().step_by(sample_stride) {
            total_weight += cell.weight;
            let (rotated_x, rotated_z) =
                rotate_xz_quat_with_sin_cos(sin_yaw, cos_yaw, cell.point_x, cell.point_z);
            let mapped_x = rotated_x + translation.x;
            let mapped_z = rotated_z + translation.z;
            if mapped_remote_cutout_center.is_some_and(|(center_x, center_z)| {
                let delta_x = mapped_x - center_x;
                let delta_z = mapped_z - center_z;
                delta_x * delta_x + delta_z * delta_z <= mapped_remote_cutout_radius_sq
            }) {
                continue;
            }
            let Some((local_height, local_axis)) =
                sample_height_map_bilinear_and_axis_fast(local_map, sample_cache, mapped_x, mapped_z)
            else {
                continue;
            };
            let diff = (local_height - cell.height).abs();
            let height_similarity = (1.0 - diff / 0.45).clamp(0.0, 1.0);
            let (remote_axis_x, remote_axis_z) =
                rotate_xz_quat_with_sin_cos(sin_yaw, cos_yaw, cell.axis_x, cell.axis_z);
            let direction_similarity = local_axis
                .map(|local_axis| (local_axis.x * remote_axis_x + local_axis.y * remote_axis_z).clamp(0.0, 1.0))
                .unwrap_or(0.5);
            let similarity =
                (height_similarity * 0.65 + direction_similarity * 0.35).clamp(0.0, 1.0);
            support_sum += cell.weight * similarity;
            weight_sum += cell.weight;
            residual_sum += diff;
            matched += 1;
        }
    } else {
        for cell in remote_signal.iter().step_by(sample_stride) {
            total_weight += cell.weight;
            let (rotated_x, rotated_z) =
                rotate_xz_quat_with_sin_cos(sin_yaw, cos_yaw, cell.point_x, cell.point_z);
            let mapped_x = rotated_x + translation.x;
            let mapped_z = rotated_z + translation.z;
            if mapped_remote_cutout_center.is_some_and(|(center_x, center_z)| {
                let delta_x = mapped_x - center_x;
                let delta_z = mapped_z - center_z;
                delta_x * delta_x + delta_z * delta_z <= mapped_remote_cutout_radius_sq
            }) {
                continue;
            }
            let Some(local_height) = sample_height_map_bilinear(local_map, mapped_x, mapped_z) else {
                continue;
            };
            let diff = (local_height - cell.height).abs();
            let height_similarity = (1.0 - diff / 0.45).clamp(0.0, 1.0);
            let similarity = (height_similarity * 0.65 + 0.5 * 0.35).clamp(0.0, 1.0);
            support_sum += cell.weight * similarity;
            weight_sum += cell.weight;
            residual_sum += diff;
            matched += 1;
        }
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

fn score_rotated_height_map_alignment(
    local_map: &XrDepthAlignHeightMap,
    local_sample_cache: Option<&HeightMapSampleCache>,
    remote_map: &XrDepthAlignHeightMap,
    rotated_remote_signal: &[RotatedHeightMapSignalCell],
    yaw: f32,
    translation: Vec3f,
) -> (f32, f32, usize) {
    if rotated_remote_signal.is_empty() {
        return (0.0, f32::INFINITY, 0);
    }
    let (sin_yaw, cos_yaw) = yaw.sin_cos();
    let mapped_remote_cutout_center = remote_map
        .player_cutout_center
        .map(|center| rotate_xz_quat_with_sin_cos(sin_yaw, cos_yaw, center.x, center.y));
    let mapped_remote_cutout_center = mapped_remote_cutout_center
        .map(|(center_x, center_z)| (center_x + translation.x, center_z + translation.z));
    let mapped_remote_cutout_radius =
        (remote_map.player_cutout_radius_meters + 0.14).max(remote_map.cell_size_meters * 2.0);
    let mapped_remote_cutout_radius_sq = mapped_remote_cutout_radius * mapped_remote_cutout_radius;
    let mut support_sum = 0.0;
    let mut weight_sum = 0.0;
    let mut total_weight = 0.0;
    let mut residual_sum = 0.0;
    let mut matched = 0usize;
    if let Some(sample_cache) = local_sample_cache {
        for cell in rotated_remote_signal {
            total_weight += cell.weight;
            let mapped_x = cell.point_x + translation.x;
            let mapped_z = cell.point_z + translation.z;
            if mapped_remote_cutout_center.is_some_and(|(center_x, center_z)| {
                let delta_x = mapped_x - center_x;
                let delta_z = mapped_z - center_z;
                delta_x * delta_x + delta_z * delta_z <= mapped_remote_cutout_radius_sq
            }) {
                continue;
            }
            let Some((local_height, local_axis)) =
                sample_height_map_bilinear_and_axis_fast(local_map, sample_cache, mapped_x, mapped_z)
            else {
                continue;
            };
            let diff = (local_height - cell.height).abs();
            let height_similarity = (1.0 - diff / 0.45).clamp(0.0, 1.0);
            let direction_similarity = local_axis
                .map(|local_axis| (local_axis.x * cell.axis_x + local_axis.y * cell.axis_z).clamp(0.0, 1.0))
                .unwrap_or(0.5);
            let similarity =
                (height_similarity * 0.65 + direction_similarity * 0.35).clamp(0.0, 1.0);
            support_sum += cell.weight * similarity;
            weight_sum += cell.weight;
            residual_sum += diff;
            matched += 1;
        }
    } else {
        for cell in rotated_remote_signal {
            total_weight += cell.weight;
            let mapped_x = cell.point_x + translation.x;
            let mapped_z = cell.point_z + translation.z;
            if mapped_remote_cutout_center.is_some_and(|(center_x, center_z)| {
                let delta_x = mapped_x - center_x;
                let delta_z = mapped_z - center_z;
                delta_x * delta_x + delta_z * delta_z <= mapped_remote_cutout_radius_sq
            }) {
                continue;
            }
            let Some(local_height) = sample_height_map_bilinear(local_map, mapped_x, mapped_z) else {
                continue;
            };
            let diff = (local_height - cell.height).abs();
            let height_similarity = (1.0 - diff / 0.45).clamp(0.0, 1.0);
            let similarity = (height_similarity * 0.65 + 0.5 * 0.35).clamp(0.0, 1.0);
            support_sum += cell.weight * similarity;
            weight_sum += cell.weight;
            residual_sum += diff;
            matched += 1;
        }
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

fn estimate_height_map_vertical_offset(
    local_map: &XrDepthAlignHeightMap,
    remote_map: &XrDepthAlignHeightMap,
    remote_signal: &[DenseHeightMapSignalCell],
    yaw: f32,
    translation: Vec3f,
) -> Option<f32> {
    if remote_signal.len() < XR_DEPTH_ALIGN_VERTICAL_OFFSET_MIN_MATCHES {
        return None;
    }
    let (sin_yaw, cos_yaw) = yaw.sin_cos();
    let mapped_remote_cutout_center = remote_map
        .player_cutout_center
        .map(|center| rotate_xz_quat_with_sin_cos(sin_yaw, cos_yaw, center.x, center.y));
    let mapped_remote_cutout_center = mapped_remote_cutout_center
        .map(|(center_x, center_z)| (center_x + translation.x, center_z + translation.z));
    let mapped_remote_cutout_radius =
        (remote_map.player_cutout_radius_meters + 0.14).max(remote_map.cell_size_meters * 2.0);
    let mapped_remote_cutout_radius_sq = mapped_remote_cutout_radius * mapped_remote_cutout_radius;
    let bin_size = XR_DEPTH_ALIGN_VERTICAL_OFFSET_BIN_METERS.max(1.0e-3);
    let min_delta = translation.y - XR_DEPTH_ALIGN_VERTICAL_OFFSET_MAX_DELTA_METERS;
    let max_delta = translation.y + XR_DEPTH_ALIGN_VERTICAL_OFFSET_MAX_DELTA_METERS;
    let bin_count = (((max_delta - min_delta) / bin_size).ceil() as usize).max(1) + 1;
    let mut bins = vec![0.0f32; bin_count];
    let mut deltas = Vec::<(f32, f32)>::new();
    let mut total_weight = 0.0f32;

    for cell in remote_signal {
        let (rotated_x, rotated_z) =
            rotate_xz_quat_with_sin_cos(sin_yaw, cos_yaw, cell.point_x, cell.point_z);
        let mapped_x = rotated_x + translation.x;
        let mapped_z = rotated_z + translation.z;
        if mapped_remote_cutout_center.is_some_and(|(center_x, center_z)| {
            let delta_x = mapped_x - center_x;
            let delta_z = mapped_z - center_z;
            delta_x * delta_x + delta_z * delta_z <= mapped_remote_cutout_radius_sq
        }) {
            continue;
        }
        let Some(local_height) = sample_height_map_bilinear(local_map, mapped_x, mapped_z) else {
            continue;
        };
        let delta = local_height - cell.height;
        if !delta.is_finite() || delta < min_delta || delta > max_delta {
            continue;
        }
        let weight = cell.weight.max(0.01);
        let bin = ((delta - min_delta) / bin_size)
            .floor()
            .clamp(0.0, bin_count.saturating_sub(1) as f32) as usize;
        bins[bin] += weight;
        total_weight += weight;
        deltas.push((delta, weight));
    }

    if deltas.len() < XR_DEPTH_ALIGN_VERTICAL_OFFSET_MIN_MATCHES || total_weight <= 1.0e-4 {
        return None;
    }
    let window_bins = XR_DEPTH_ALIGN_VERTICAL_OFFSET_SUPPORT_WINDOW_BINS.max(1);
    let mut best_start_bin = 0usize;
    let mut best_support = 0.0f32;
    for start_bin in 0..bin_count {
        let end_bin = (start_bin + window_bins).min(bin_count);
        let support = bins[start_bin..end_bin].iter().copied().sum::<f32>();
        if support > best_support {
            best_support = support;
            best_start_bin = start_bin;
        }
    }
    if best_support < total_weight * XR_DEPTH_ALIGN_VERTICAL_OFFSET_MIN_SUPPORT_RATIO {
        return None;
    }

    let low = min_delta + best_start_bin as f32 * bin_size;
    let high = min_delta + (best_start_bin + window_bins).min(bin_count) as f32 * bin_size;
    let mut weighted_sum = 0.0f32;
    let mut weight_sum = 0.0f32;
    for (delta, weight) in &deltas {
        if *delta >= low && *delta <= high {
            weighted_sum += *delta * *weight;
            weight_sum += *weight;
        }
    }
    if weight_sum <= 1.0e-4 {
        return None;
    }

    let coarse_mean = weighted_sum / weight_sum;
    let refine_radius = (window_bins as f32 * bin_size * 0.75).max(bin_size);
    let mut refined_sum = 0.0f32;
    let mut refined_weight_sum = 0.0f32;
    for (delta, weight) in &deltas {
        if (*delta - coarse_mean).abs() <= refine_radius {
            refined_sum += *delta * *weight;
            refined_weight_sum += *weight;
        }
    }
    if refined_weight_sum <= 1.0e-4 {
        return Some(coarse_mean);
    }
    Some(refined_sum / refined_weight_sum)
}

fn refine_solution_vertical_offset_from_overlap(
    local_map: &XrDepthAlignHeightMap,
    remote_map: &XrDepthAlignHeightMap,
    remote_signal: &[DenseHeightMapSignalCell],
    current: XrDepthAlignSolution,
) -> XrDepthAlignSolution {
    let Some(refined_y) = estimate_height_map_vertical_offset(
        local_map,
        remote_map,
        remote_signal,
        current.yaw_radians,
        current.translation,
    ) else {
        return current;
    };
    let mut corrected = current;
    corrected.translation.y = refined_y;
    corrected
}

fn apply_height_map_alignment_support(
    candidate: XrDepthAlignSolution,
    local_map: Option<&XrDepthAlignHeightMap>,
    local_sample_cache: Option<&HeightMapSampleCache>,
    remote_map: Option<&XrDepthAlignHeightMap>,
    remote_signal: &[DenseHeightMapSignalCell],
) -> XrDepthAlignSolution {
    let (Some(local_map), Some(remote_map)) = (local_map, remote_map) else {
        return candidate;
    };
    let (support, residual, matched) = score_height_map_alignment(
        local_map,
        local_sample_cache,
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
    local_signal: &[HeightMapSignalCell],
    remote_signal: &[HeightMapSignalCell],
    local_map: Option<&XrDepthAlignHeightMap>,
    local_sample_cache: Option<&HeightMapSampleCache>,
    remote_map: Option<&XrDepthAlignHeightMap>,
    remote_dense_signal: &[DenseHeightMapSignalCell],
    yaw: f32,
    translation: Vec3f,
) -> XrDepthAlignSolution {
    apply_height_map_alignment_support(
        score_signal_alignment_solution(local_signal, remote_signal, yaw, translation),
        local_map,
        local_sample_cache,
        remote_map,
        remote_dense_signal,
    )
}

#[allow(dead_code)]
fn refine_translation_only_for_yaw(
    local_signal: &[HeightMapSignalCell],
    remote_signal: &[HeightMapSignalCell],
    local_map: Option<&XrDepthAlignHeightMap>,
    local_sample_cache: Option<&HeightMapSampleCache>,
    remote_map: Option<&XrDepthAlignHeightMap>,
    remote_dense_signal: &[DenseHeightMapSignalCell],
    floor_y: f32,
    initial: XrDepthAlignSolution,
) -> XrDepthAlignSolution {
    let mut best = initial;
    for translation_step in [0.06, 0.025, 0.01] {
        loop {
            let mut improved = false;
            for tx_delta in [-translation_step, 0.0, translation_step] {
                for tz_delta in [-translation_step, 0.0, translation_step] {
                    if tx_delta == 0.0 && tz_delta == 0.0 {
                        continue;
                    }
                    let candidate = score_full_alignment_solution(
                        local_signal,
                        remote_signal,
                        local_map,
                        local_sample_cache,
                        remote_map,
                        remote_dense_signal,
                        best.yaw_radians,
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
            if !improved {
                break;
            }
        }
    }
    best
}

fn wall_profile_yaw_sidecar_is_safe(
    current: XrDepthAlignSolution,
    corrected: XrDepthAlignSolution,
    current_profile_score: f32,
    best_profile_score: f32,
) -> bool {
    let yaw_delta = wrap_angle(corrected.yaw_radians - current.yaw_radians).abs();
    if yaw_delta < XR_DEPTH_ALIGN_WALL_PROFILE_MIN_YAW_DELTA_RADIANS {
        return false;
    }
    if best_profile_score < current_profile_score + XR_DEPTH_ALIGN_WALL_PROFILE_MIN_SCORE_GAIN
        && best_profile_score
            < current_profile_score * XR_DEPTH_ALIGN_WALL_PROFILE_MIN_SCORE_RATIO
    {
        return false;
    }
    corrected.confidence
        >= current.confidence * XR_DEPTH_ALIGN_WALL_PROFILE_MIN_CONFIDENCE_RATIO
        && corrected.matched_samples + XR_DEPTH_ALIGN_WALL_PROFILE_MAX_MATCH_LOSS
            >= current.matched_samples
        && corrected.residual_meters.is_finite()
        && current.residual_meters.is_finite()
        && corrected.residual_meters
            <= current.residual_meters + XR_DEPTH_ALIGN_WALL_PROFILE_MAX_RESIDUAL_INCREASE_METERS
}

#[allow(dead_code)]
fn refine_solution_with_wall_profile_yaw_sidecar(
    local_signal: &[HeightMapSignalCell],
    remote_signal: &[HeightMapSignalCell],
    local_map: &XrDepthAlignHeightMap,
    local_sample_cache: Option<&HeightMapSampleCache>,
    remote_map: &XrDepthAlignHeightMap,
    remote_dense_signal: &[DenseHeightMapSignalCell],
    floor_y: f32,
    current: XrDepthAlignSolution,
) -> XrDepthAlignSolution {
    let local_profile = build_wall_profile_contour_points(local_map);
    let remote_profile = build_wall_profile_contour_points(remote_map);
    if local_profile.len() < XR_DEPTH_ALIGN_WALL_PROFILE_MIN_POINTS
        || remote_profile.len() < XR_DEPTH_ALIGN_WALL_PROFILE_MIN_POINTS
    {
        return current;
    }

    let current_profile_score = score_wall_profile_alignment_with_translation_prior(
        &local_profile,
        &remote_profile,
        current.yaw_radians,
        current.translation,
    );
    let mut best_profile_yaw = current.yaw_radians;
    let mut best_profile_score = current_profile_score;
    let yaw_step = XR_DEPTH_ALIGN_WALL_PROFILE_YAW_STEP_RADIANS;
    let yaw_steps = (XR_DEPTH_ALIGN_WALL_PROFILE_YAW_WINDOW_RADIANS / yaw_step)
        .ceil()
        .max(1.0) as isize;
    for yaw_index in -yaw_steps..=yaw_steps {
        let yaw = wrap_angle(current.yaw_radians + yaw_index as f32 * yaw_step);
        let score = score_wall_profile_alignment_with_translation_prior(
            &local_profile,
            &remote_profile,
            yaw,
            current.translation,
        );
        if score > best_profile_score {
            best_profile_score = score;
            best_profile_yaw = yaw;
        }
    }
    if wrap_angle(best_profile_yaw - current.yaw_radians).abs()
        < XR_DEPTH_ALIGN_WALL_PROFILE_MIN_YAW_DELTA_RADIANS
    {
        return current;
    }

    let corrected_initial = score_full_alignment_solution(
        local_signal,
        remote_signal,
        Some(local_map),
        local_sample_cache,
        Some(remote_map),
        remote_dense_signal,
        best_profile_yaw,
        vec3(current.translation.x, floor_y, current.translation.z),
    );
    let corrected = refine_translation_only_for_yaw(
        local_signal,
        remote_signal,
        Some(local_map),
        local_sample_cache,
        Some(remote_map),
        remote_dense_signal,
        floor_y,
        corrected_initial,
    );
    if alignment_solution_better(&corrected, &current)
        || wall_profile_yaw_sidecar_is_safe(
            current,
            corrected,
            current_profile_score,
            best_profile_score,
        )
    {
        corrected
    } else {
        current
    }
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

#[allow(dead_code)]
fn refine_height_map_alignment(
    local_map: &XrDepthAlignHeightMap,
    local_sample_cache: Option<&HeightMapSampleCache>,
    remote_map: &XrDepthAlignHeightMap,
    remote_signal: &[DenseHeightMapSignalCell],
    yaw: f32,
    translation: Vec3f,
) -> (f32, Vec3f) {
    let mut best_yaw = wrap_angle(yaw);
    let mut best_translation = translation;
    let mut best_score = score_height_map_alignment(
        local_map,
        local_sample_cache,
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
        let mut rotated_remote_signal = Vec::<RotatedHeightMapSignalCell>::new();
        loop {
            let mut improved = false;
            let mut rotated_signal_yaw = f32::NAN;
            for yaw_delta in [-yaw_step, 0.0, yaw_step] {
                for tx_delta in [-translation_step, 0.0, translation_step] {
                    for tz_delta in [-translation_step, 0.0, translation_step] {
                        if yaw_delta == 0.0 && tx_delta == 0.0 && tz_delta == 0.0 {
                            continue;
                        }
                        let candidate_yaw = wrap_angle(best_yaw + yaw_delta);
                        if !rotated_signal_yaw.is_finite()
                            || wrap_angle(candidate_yaw - rotated_signal_yaw).abs() > 1.0e-6
                        {
                            fill_rotated_height_map_signal(
                                &mut rotated_remote_signal,
                                remote_signal,
                                candidate_yaw,
                            );
                            rotated_signal_yaw = candidate_yaw;
                        }
                        let candidate_translation = vec3(
                            best_translation.x + tx_delta,
                            best_translation.y,
                            best_translation.z + tz_delta,
                        );
                        let candidate_score = score_rotated_height_map_alignment(
                            local_map,
                            local_sample_cache,
                            remote_map,
                            &rotated_remote_signal,
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

fn candidate_signal_yaws(
    local_histogram: &[f32],
    remote_histogram: &[f32],
    local_signal: &[HeightMapSignalCell],
    remote_signal: &[HeightMapSignalCell],
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
        for (_, shift) in shifts.into_iter().take(8) {
            candidates.push(wrap_angle(
                shift as f32 * std::f32::consts::TAU / bins as f32,
            ));
        }
    }

    for local_cell in local_signal.iter().take(16) {
        for remote_cell in remote_signal.iter().take(16) {
            candidates.push(wrap_angle(signed_xz_angle_2d(
                remote_cell.axis_xz.x,
                remote_cell.axis_xz.y,
                local_cell.axis_xz.x,
                local_cell.axis_xz.y,
            )));
        }
    }
    dedupe_angles(candidates, 0.05)
}

fn candidate_signal_translations(
    local_signal: &[HeightMapSignalCell],
    remote_signal: &[HeightMapSignalCell],
    floor_y: f32,
    yaw: f32,
) -> Vec<Vec3f> {
    let mut votes = HashMap::<(i32, i32), TranslationVote>::new();
    for local_cell in local_signal.iter().take(64) {
        let Some(local_axis) = xz_axis(local_cell.gradient) else {
            continue;
        };
        for remote_cell in remote_signal.iter().take(64) {
            let Some(remote_axis) = xz_axis(remote_cell.gradient) else {
                continue;
            };
            let rotated_remote_axis = rotate_y(yaw, remote_axis);
            let alignment = local_axis.dot(rotated_remote_axis);
            if alignment < XR_DEPTH_ALIGN_SIGNAL_MATCH_MIN_DIRECTION_DOT {
                continue;
            }
            let height_delta = (local_cell.height - remote_cell.height).abs();
            if height_delta > XR_DEPTH_ALIGN_SIGNAL_MATCH_MAX_HEIGHT_DELTA_METERS {
                continue;
            }
            let delta = local_cell.point - rotate_y(yaw, remote_cell.point);
            if delta.x.abs() > 8.0 || delta.z.abs() > 8.0 {
                continue;
            }
            let height_factor = (1.0
                - height_delta / XR_DEPTH_ALIGN_SIGNAL_MATCH_MAX_HEIGHT_DELTA_METERS)
                .clamp(0.0, 1.0);
            let weight = (local_cell.weight * remote_cell.weight).sqrt()
                * (0.30 + 0.70 * alignment)
                * (0.35 + 0.65 * height_factor);
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
        .take(12)
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

#[derive(Default)]
struct TranslationVote {
    score: f32,
    weight_sum: f32,
    sum_x: f32,
    sum_z: f32,
    count: usize,
}

#[derive(Clone, Copy)]
struct HeightMapSignalMatch<'a> {
    local: &'a HeightMapSignalCell,
    remote: &'a HeightMapSignalCell,
    planar_distance: f32,
    height_distance: f32,
    alignment: f32,
    score: f32,
}

fn collect_unique_signal_matches<'a>(
    local_signal: &'a [HeightMapSignalCell],
    remote_signal: &'a [HeightMapSignalCell],
    yaw: f32,
    translation: Vec3f,
) -> Vec<HeightMapSignalMatch<'a>> {
    #[derive(Clone, Copy)]
    struct Candidate<'a> {
        local_index: usize,
        remote_index: usize,
        local: &'a HeightMapSignalCell,
        remote: &'a HeightMapSignalCell,
        planar_distance: f32,
        height_distance: f32,
        alignment: f32,
        score: f32,
    }

    let mut candidates = Vec::<Candidate<'a>>::new();
    for (remote_index, remote_cell) in remote_signal.iter().enumerate() {
        let transformed_point = rotate_y(yaw, remote_cell.point) + translation;
        let Some(transformed_axis) = xz_axis(rotate_y(yaw, remote_cell.gradient)) else {
            continue;
        };
        for (local_index, local_cell) in local_signal.iter().enumerate() {
            let Some(local_axis) = xz_axis(local_cell.gradient) else {
                continue;
            };
            let alignment = local_axis.dot(transformed_axis);
            if alignment < XR_DEPTH_ALIGN_SIGNAL_MATCH_MIN_DIRECTION_DOT {
                continue;
            }
            let planar_delta = local_cell.point - transformed_point;
            let planar_distance =
                (planar_delta.x * planar_delta.x + planar_delta.z * planar_delta.z).sqrt();
            if planar_distance > XR_DEPTH_ALIGN_SIGNAL_MATCH_RADIUS_METERS {
                continue;
            }
            let height_distance = (local_cell.height - remote_cell.height).abs();
            if height_distance > XR_DEPTH_ALIGN_SIGNAL_MATCH_MAX_HEIGHT_DELTA_METERS {
                continue;
            }
            let height_factor = (1.0
                - height_distance / XR_DEPTH_ALIGN_SIGNAL_MATCH_MAX_HEIGHT_DELTA_METERS)
                .clamp(0.0, 1.0);
            let score = (local_cell.weight * remote_cell.weight).sqrt()
                * (0.30 + 0.70 * alignment)
                * (-planar_distance / XR_DEPTH_ALIGN_SIGNAL_MATCH_RADIUS_METERS.max(0.05)).exp()
                * (0.35 + 0.65 * height_factor);
            candidates.push(Candidate {
                local_index,
                remote_index,
                local: local_cell,
                remote: remote_cell,
                planar_distance,
                height_distance,
                alignment,
                score,
            });
        }
    }
    candidates.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.planar_distance.total_cmp(&b.planar_distance))
    });

    let mut used_local = vec![false; local_signal.len()];
    let mut used_remote = vec![false; remote_signal.len()];
    let mut matches = Vec::<HeightMapSignalMatch<'a>>::new();
    for candidate in candidates {
        if used_local[candidate.local_index] || used_remote[candidate.remote_index] {
            continue;
        }
        used_local[candidate.local_index] = true;
        used_remote[candidate.remote_index] = true;
        matches.push(HeightMapSignalMatch {
            local: candidate.local,
            remote: candidate.remote,
            planar_distance: candidate.planar_distance,
            height_distance: candidate.height_distance,
            alignment: candidate.alignment,
            score: candidate.score,
        });
    }
    matches
}

fn refine_signal_alignment(
    local_signal: &[HeightMapSignalCell],
    remote_signal: &[HeightMapSignalCell],
    floor_y: f32,
    yaw: f32,
    translation: Vec3f,
) -> (f32, Vec3f) {
    let mut refined_yaw = yaw;
    let mut refined_translation = translation;
    refined_translation.y = floor_y;
    for _ in 0..2 {
        let matches = collect_unique_signal_matches(
            local_signal,
            remote_signal,
            refined_yaw,
            refined_translation,
        );
        let mut translation_sum = vec3(0.0, 0.0, 0.0);
        let mut translation_weight_sum = 0.0;
        let mut yaw_sin = 0.0;
        let mut yaw_cos = 0.0;
        let mut yaw_weight_sum = 0.0;
        for matched in matches {
            let Some(local_axis) = xz_axis(matched.local.gradient) else {
                continue;
            };
            let Some(remote_axis) = xz_axis(matched.remote.gradient) else {
                continue;
            };
            let height_factor = (1.0
                - matched.height_distance / XR_DEPTH_ALIGN_SIGNAL_MATCH_MAX_HEIGHT_DELTA_METERS)
                .clamp(0.0, 1.0);
            let weight = (matched.local.weight * matched.remote.weight).sqrt()
                * (0.30 + 0.70 * matched.alignment)
                * (0.35 + 0.65 * height_factor);
            let candidate_translation =
                matched.local.point - rotate_y(refined_yaw, matched.remote.point);
            translation_sum += candidate_translation * weight;
            translation_weight_sum += weight;
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

fn score_signal_alignment_solution(
    local_signal: &[HeightMapSignalCell],
    remote_signal: &[HeightMapSignalCell],
    yaw: f32,
    translation: Vec3f,
) -> XrDepthAlignSolution {
    let matches = collect_unique_signal_matches(local_signal, remote_signal, yaw, translation);
    let mut total_score = 0.0;
    let mut residual_sum = 0.0;
    let max_score = remote_signal
        .iter()
        .map(|cell| cell.weight.max(0.01))
        .sum::<f32>()
        .max(0.01);
    for matched in &matches {
        total_score += matched.score;
        residual_sum += matched.planar_distance * 0.65 + matched.height_distance * 0.35;
    }
    let matched_samples = matches.len();
    let residual_meters = if matched_samples > 0 {
        residual_sum / matched_samples as f32
    } else {
        f32::INFINITY
    };
    let coverage = (matched_samples as f32 / remote_signal.len().max(1) as f32).clamp(0.0, 1.0);
    let residual_confidence = if residual_meters.is_finite() {
        (1.0 - (residual_meters / 0.50)).clamp(0.0, 1.0)
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

#[allow(dead_code)]
fn refine_seed_alignment_solution(
    local_signal: &[HeightMapSignalCell],
    remote_signal: &[HeightMapSignalCell],
    local_map: Option<&XrDepthAlignHeightMap>,
    local_sample_cache: Option<&HeightMapSampleCache>,
    remote_map: Option<&XrDepthAlignHeightMap>,
    remote_dense_signal: &[DenseHeightMapSignalCell],
    remote_refine_signal: &[DenseHeightMapSignalCell],
    floor_y: f32,
    seed: XrDepthAlignSolution,
) -> XrDepthAlignSolution {
    let mut best_yaw = wrap_angle(seed.yaw_radians);
    let mut best_translation = seed.translation;
    best_translation.y = floor_y;
    if let (Some(local_map), Some(remote_map)) = (local_map, remote_map) {
        if !remote_refine_signal.is_empty() {
            (best_yaw, best_translation) = refine_height_map_alignment(
                local_map,
                local_sample_cache,
                remote_map,
                remote_refine_signal,
                best_yaw,
                best_translation,
            );
            best_translation.y = floor_y;
        }
    }
    let (signal_refined_yaw, signal_refined_translation) = refine_signal_alignment(
        local_signal,
        remote_signal,
        floor_y,
        best_yaw,
        best_translation,
    );
    let signal_translation_jump = vec3(
        signal_refined_translation.x - best_translation.x,
        0.0,
        signal_refined_translation.z - best_translation.z,
    )
    .length();
    if wrap_angle(signal_refined_yaw - best_yaw).abs() <= 0.18 && signal_translation_jump <= 0.28 {
        best_yaw = signal_refined_yaw;
        best_translation = signal_refined_translation;
    }
    let mut best = score_full_alignment_solution(
        local_signal,
        remote_signal,
        local_map,
        local_sample_cache,
        remote_map,
        remote_dense_signal,
        best_yaw,
        best_translation,
    );
    for (yaw_step, translation_step) in [(0.10, 0.18), (0.04, 0.07), (0.015, 0.03), (0.006, 0.012)]
    {
        loop {
            let mut improved = false;
            for yaw_delta in [-yaw_step, 0.0, yaw_step] {
                for tx_delta in [-translation_step, 0.0, translation_step] {
                    for tz_delta in [-translation_step, 0.0, translation_step] {
                        if yaw_delta == 0.0 && tx_delta == 0.0 && tz_delta == 0.0 {
                            continue;
                        }
                        let candidate = score_full_alignment_solution(
                            local_signal,
                            remote_signal,
                            local_map,
                            local_sample_cache,
                            remote_map,
                            remote_dense_signal,
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
    remote_signal_count: usize,
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
    let overlap = candidate.matched_samples as f32 / remote_signal_count.max(1) as f32;
    if overlap < XR_DEPTH_ALIGN_SEEDED_LOCK_MIN_OVERLAP {
        return false;
    }
    let max_cell_size = local_map
        .map(|map| map.cell_size_meters)
        .unwrap_or(0.03)
        .max(remote_map.map(|map| map.cell_size_meters).unwrap_or(0.03));
    // Dense gradient matching keeps some soft residual even on a stable lock, so
    // the seeded fast-path needs a slightly looser gate than the old wall-only solver.
    let max_residual_meters = (max_cell_size * 6.0).clamp(0.12, 0.26);
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

fn alignment_solution_is_distinct(
    candidate: XrDepthAlignSolution,
    current: XrDepthAlignSolution,
    translation_epsilon: f32,
    yaw_epsilon: f32,
) -> bool {
    let translation_delta = vec3(
        candidate.translation.x - current.translation.x,
        0.0,
        candidate.translation.z - current.translation.z,
    )
    .length();
    let yaw_delta = wrap_angle(candidate.yaw_radians - current.yaw_radians).abs();
    translation_delta > translation_epsilon || yaw_delta > yaw_epsilon
}

fn push_shortlisted_alignment_solution(
    shortlist: &mut Vec<XrDepthAlignSolution>,
    candidate: XrDepthAlignSolution,
    max_candidates: usize,
    translation_epsilon: f32,
    yaw_epsilon: f32,
) {
    if shortlist.iter().any(|existing| {
        !alignment_solution_is_distinct(candidate, *existing, translation_epsilon, yaw_epsilon)
    }) {
        if let Some(existing) = shortlist.iter_mut().find(|existing| {
            !alignment_solution_is_distinct(candidate, **existing, translation_epsilon, yaw_epsilon)
        }) {
            if alignment_solution_better(&candidate, existing) {
                *existing = candidate;
            }
        }
    } else {
        shortlist.push(candidate);
    }
    shortlist.sort_by(|left, right| {
        right
            .ranking_confidence()
            .partial_cmp(&left.ranking_confidence())
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| right.matched_samples.cmp(&left.matched_samples))
            .then_with(|| {
                left.residual_meters
                    .partial_cmp(&right.residual_meters)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    });
    if shortlist.len() > max_candidates {
        shortlist.truncate(max_candidates);
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

fn rotate_xz(yaw: f32, x: f32, z: f32) -> (f32, f32) {
    let (sin_yaw, cos_yaw) = yaw.sin_cos();
    (x * cos_yaw - z * sin_yaw, x * sin_yaw + z * cos_yaw)
}

#[inline(always)]
fn rotate_xz_quat_with_sin_cos(sin_yaw: f32, cos_yaw: f32, x: f32, z: f32) -> (f32, f32) {
    (x * cos_yaw + z * sin_yaw, z * cos_yaw - x * sin_yaw)
}

fn signed_xz_angle_2d(from_x: f32, from_z: f32, to_x: f32, to_z: f32) -> f32 {
    let cross = from_z * to_x - from_x * to_z;
    let dot = from_x * to_x + from_z * to_z;
    cross.atan2(dot)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::{Path, PathBuf}, time::Instant};

    #[derive(Clone, Copy, Debug, Default)]
    struct HeightMapTestArtifacts {
        cutout_center: Option<Vec2f>,
        occlusion_center: Option<Vec2f>,
        extra_blob_center: Option<Vec2f>,
        noise_seed: f32,
        noise_scale: f32,
        height_bias: f32,
    }

    #[derive(Clone, Copy, Debug)]
    struct TestRng(u64);

    #[derive(Debug, Default)]
    struct RandomCaseSummary {
        case_count: usize,
        accepted_cases: usize,
        seeded_reuse_cases: usize,
        max_yaw_error: f32,
        max_translation_error: f32,
        failures: Vec<String>,
    }

    impl RandomCaseSummary {
        fn merge(&mut self, other: Self) {
            self.case_count += other.case_count;
            self.accepted_cases += other.accepted_cases;
            self.seeded_reuse_cases += other.seeded_reuse_cases;
            self.max_yaw_error = self.max_yaw_error.max(other.max_yaw_error);
            self.max_translation_error =
                self.max_translation_error.max(other.max_translation_error);
            self.failures.extend(other.failures);
        }
    }

    impl TestRng {
        fn new(seed: u64) -> Self {
            Self(seed)
        }

        fn next_u32(&mut self) -> u32 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (self.0 >> 32) as u32
        }

        fn next_f32(&mut self) -> f32 {
            self.next_u32() as f32 / u32::MAX as f32
        }

        fn range_f32(&mut self, min: f32, max: f32) -> f32 {
            min + (max - min) * self.next_f32()
        }

        fn chance(&mut self, probability: f32) -> bool {
            self.next_f32() <= probability
        }
    }

    fn angle_error(a: f32, b: f32) -> f32 {
        wrap_angle(a - b).abs()
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

    fn deterministic_height_noise(point: Vec2f, seed: f32) -> f32 {
        ((point.x * 2.73 + point.y * 1.91 + seed * 0.37).sin() * 0.021)
            + ((point.x * 0.84 - point.y * 3.14 + seed * 0.61).cos() * 0.017)
    }

    fn random_artifacts(rng: &mut TestRng) -> HeightMapTestArtifacts {
        HeightMapTestArtifacts {
            cutout_center: rng
                .chance(0.8)
                .then(|| vec2f(rng.range_f32(-0.55, 0.55), rng.range_f32(-0.55, 0.55))),
            occlusion_center: rng
                .chance(0.7)
                .then(|| vec2f(rng.range_f32(-1.15, 1.15), rng.range_f32(-0.95, 0.95))),
            extra_blob_center: None,
            noise_seed: rng.range_f32(-8.0, 8.0),
            noise_scale: rng.range_f32(0.25, 0.85),
            height_bias: rng.range_f32(-0.05, 0.05),
        }
    }

    fn make_height_map_descriptor_with_artifacts(
        map_to_scene: Mat4f,
        artifacts: HeightMapTestArtifacts,
        size_x: usize,
        size_z: usize,
    ) -> XrDepthAlignDescriptor {
        let cell_size_meters = 0.05;
        let extent_x = size_x as f32 * cell_size_meters;
        let extent_z = size_z as f32 * cell_size_meters;
        let origin_x = -extent_x * 0.5;
        let origin_z = -extent_z * 0.5;
        let bottom_y_meters = 0.0;
        let top_y_meters = 2.3;
        let floor_y_meters = 0.0;
        let mut heights_meters = vec![f32::NAN; size_x * size_z];
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
                heights_meters[x + z * size_x] = height;
            }
        }
        XrDepthAlignDescriptor {
            voxel_size_meters: 0.05,
            floor_y: floor_y_meters,
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
                floor_y_meters,
                player_cutout_center: artifacts.cutout_center,
                player_cutout_radius_meters: 0.36,
                heights_meters,
            }),
        }
    }

    fn run_random_noisy_cases(
        seed: u64,
        case_count: usize,
        size_x: usize,
        size_z: usize,
    ) -> RandomCaseSummary {
        let mut rng = TestRng::new(seed);
        let mut summary = RandomCaseSummary {
            case_count,
            ..RandomCaseSummary::default()
        };

        for case_index in 0..case_count {
            let expected_yaw = rng.range_f32(-0.85, 0.85);
            let expected_translation =
                vec3(rng.range_f32(-0.75, 0.75), 0.0, rng.range_f32(-0.75, 0.75));
            let remote_to_local = Pose::new(
                Quat::from_axis_angle(vec3(0.0, 1.0, 0.0), expected_yaw),
                expected_translation,
            )
            .to_mat4();

            let mut local_artifacts = random_artifacts(&mut rng);
            let remote_artifacts = random_artifacts(&mut rng);
            if let Some(remote_cutout_center) = remote_artifacts.cutout_center {
                let mapped_remote_center = rotate_y(
                    expected_yaw,
                    vec3(remote_cutout_center.x, 0.0, remote_cutout_center.y),
                ) + expected_translation;
                local_artifacts.extra_blob_center =
                    Some(vec2f(mapped_remote_center.x, mapped_remote_center.z));
            }

            let local = make_height_map_descriptor_with_artifacts(
                Mat4f::identity(),
                local_artifacts,
                size_x,
                size_z,
            );
            let remote = make_height_map_descriptor_with_artifacts(
                remote_to_local,
                remote_artifacts,
                size_x,
                size_z,
            );

            let diagnostic = xr_depth_align_analyze_remote_to_local(&local, &remote);
            let Some(solution) = diagnostic.accepted_solution() else {
                summary.failures.push(format!(
                    "case {case_index}: no accepted solution for yaw={expected_yaw:.3} translation=({:.3},{:.3}) diag={diagnostic:?}",
                    expected_translation.x, expected_translation.z
                ));
                continue;
            };

            let yaw_error = angle_error(solution.yaw_radians, expected_yaw);
            let translation_error = (solution.translation - expected_translation).length();
            summary.max_yaw_error = summary.max_yaw_error.max(yaw_error);
            summary.max_translation_error = summary.max_translation_error.max(translation_error);
            if yaw_error > 0.18 || translation_error > 0.26 {
                summary.failures.push(format!(
                    "case {case_index}: large error yaw={yaw_error:.3} translation={translation_error:.3} solution={solution:?}"
                ));
                continue;
            }

            summary.accepted_cases += 1;

            let seeded =
                xr_depth_align_analyze_remote_to_local_seeded(&local, &remote, Some(solution));
            if seeded.yaw_candidate_count == 1 && seeded.pose_candidate_count == 1 {
                summary.seeded_reuse_cases += 1;
            }
        }

        summary
    }

    fn make_height_map_descriptor(map_to_scene: Mat4f) -> XrDepthAlignDescriptor {
        let size = 120usize;
        let cell_size_meters = 0.05;
        let extent = size as f32 * cell_size_meters;
        let origin = -extent * 0.5;
        let bottom_y_meters = 0.0;
        let top_y_meters = 2.3;
        let floor_y_meters = 0.0;
        let mut heights_meters = vec![f32::NAN; size * size];
        for z in 0..size {
            for x in 0..size {
                let point = vec2f(
                    origin + (x as f32 + 0.5) * cell_size_meters,
                    origin + (z as f32 + 0.5) * cell_size_meters,
                );
                let scene_point = map_to_scene
                    .transform_vec4(vec4f(point.x, 0.0, point.y, 1.0))
                    .to_vec3f();
                heights_meters[x + z * size] =
                    synthetic_scene_height(vec2f(scene_point.x, scene_point.z));
            }
        }
        XrDepthAlignDescriptor {
            voxel_size_meters: 0.05,
            floor_y: floor_y_meters,
            wall_normal_histogram: Vec::new(),
            samples: Vec::new(),
            vertical_descriptor: None,
            height_map: Some(XrDepthAlignHeightMap {
                origin_x: origin,
                origin_z: origin,
                cell_size_meters,
                size_x: size as u16,
                size_z: size as u16,
                bottom_y_meters,
                top_y_meters,
                floor_y_meters,
                player_cutout_center: None,
                player_cutout_radius_meters: 0.0,
                heights_meters,
            }),
        }
    }

    #[derive(Clone, Copy, Debug, Default)]
    struct ReferenceManualPose {
        shift_x_meters: f32,
        shift_z_meters: f32,
        rotation_radians: f32,
    }

    fn reference_dump_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("util/dumps/align-pair-226a39e4b300-r0097-1774792873191.bin")
    }

    fn reference_manual_pose_path(dump_path: &Path) -> PathBuf {
        let stem = dump_path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("align-pair");
        dump_path.with_file_name(format!("{stem}.manual_pose.ron"))
    }

    fn load_reference_manual_pose(dump_path: &Path) -> Option<ReferenceManualPose> {
        let text = fs::read_to_string(reference_manual_pose_path(dump_path)).ok()?;
        let mut pose = ReferenceManualPose::default();
        for line in text.lines() {
            let (key, value) = line.split_once(':')?;
            let value = value.trim();
            match key.trim() {
                "shift_x_meters" => pose.shift_x_meters = value.parse().ok()?,
                "shift_y_meters" => pose.shift_z_meters = value.parse().ok()?,
                "rotation_radians" => pose.rotation_radians = value.parse().ok()?,
                _ => {}
            }
        }
        Some(pose)
    }

    #[test]
    fn dense_height_map_solver_recovers_rotated_translated_pose() {
        let local = make_height_map_descriptor(Mat4f::identity());
        let expected_yaw = 0.58;
        let expected_translation = vec3f(-0.82, 0.0, 0.67);
        let remote_to_local = Pose::new(
            Quat::from_axis_angle(vec3f(0.0, 1.0, 0.0), expected_yaw),
            expected_translation,
        )
        .to_mat4();
        let remote = xr_depth_align_transform_descriptor(&local, &remote_to_local.invert());

        let solution = xr_depth_align_analyze_remote_to_local(&local, &remote)
            .accepted_solution()
            .expect("dense solver should recover the pose");

        assert!(
            angle_error(solution.yaw_radians, expected_yaw) < 0.12,
            "{solution:?}"
        );
        assert!(
            (solution.translation - expected_translation).length() < 0.18,
            "{solution:?}"
        );
        assert!(solution.confidence > 0.14, "{solution:?}");
        assert!(solution.matched_samples >= 6, "{solution:?}");
    }

    #[test]
    fn dense_height_map_solver_refines_vertical_offset_from_overlap() {
        let local = make_height_map_descriptor(Mat4f::identity());
        let expected_yaw = 0.41;
        let expected_translation = vec3f(-0.46, 0.24, 0.58);
        let remote_to_local = Pose::new(
            Quat::from_axis_angle(vec3f(0.0, 1.0, 0.0), expected_yaw),
            expected_translation,
        )
        .to_mat4();
        let mut remote = xr_depth_align_transform_descriptor(&local, &remote_to_local.invert());
        let poisoned_floor_bias = 0.33;
        remote.floor_y += poisoned_floor_bias;
        if let Some(height_map) = &mut remote.height_map {
            height_map.floor_y_meters += poisoned_floor_bias;
        }

        let solution = xr_depth_align_analyze_remote_to_local(&local, &remote)
            .accepted_solution()
            .expect("dense solver should recover vertical offset from overlap");

        assert!(
            angle_error(solution.yaw_radians, expected_yaw) < 0.12,
            "{solution:?}"
        );
        assert!(
            vec3(
                solution.translation.x - expected_translation.x,
                0.0,
                solution.translation.z - expected_translation.z,
            )
            .length()
                < 0.18,
            "{solution:?}"
        );
        assert!(
            (solution.translation.y - expected_translation.y).abs() < 0.06,
            "{solution:?}"
        );
        assert!(
            (solution.translation.y - (local.floor_y - remote.floor_y)).abs() > 0.12,
            "{solution:?}"
        );
    }

    #[test]
    fn dense_seeded_solver_reuses_stable_lock() {
        let local = make_height_map_descriptor(Mat4f::identity());
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

        assert_eq!(diagnostic.yaw_candidate_count, 1, "{diagnostic:?}");
        assert_eq!(diagnostic.pose_candidate_count, 1, "{diagnostic:?}");
        assert!(
            angle_error(solution.yaw_radians, 0.34) < 0.03,
            "{solution:?}"
        );
        assert!(
            (solution.translation - vec3(-0.52, 0.0, 0.46)).length() < 0.05,
            "{solution:?}"
        );
    }

    #[test]
    fn dense_seeded_solver_recovers_after_seed_mismatch() {
        let local = make_height_map_descriptor(Mat4f::identity());
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
    fn dense_height_map_solver_handles_random_noisy_inputs() {
        let summary = run_random_noisy_cases(0x5eed_cafe_d15c_a11e, 4, 120, 114);
        assert!(summary.accepted_cases >= 4, "summary={summary:?}");
        assert!(summary.seeded_reuse_cases >= 2, "summary={summary:?}");
    }

    #[test]
    fn timesliced_solver_matches_reference_dump_and_keeps_steps_bounded() {
        let dump_path = reference_dump_path();
        let bytes = fs::read(&dump_path).expect("reference dump should exist");
        let pair = XrNetAlignmentDescriptorDumpPair::from_file_bytes(&bytes)
            .expect("reference dump should decode");
        let manual_pose =
            load_reference_manual_pose(&dump_path).expect("reference manual pose should exist");
        let local = &pair.local_descriptor.descriptor;
        let remote = &pair.remote_descriptor.descriptor;
        let mut matcher = XrDepthAlignMatcher::new(local, remote, None);
        let mut measured_steps = 0u32;
        let mut max_step = std::time::Duration::ZERO;

        while !matcher.is_finished() {
            let started = Instant::now();
            let progressed = matcher.step();
            let elapsed = started.elapsed();
            if progressed {
                measured_steps = measured_steps.saturating_add(1);
                max_step = max_step.max(elapsed);
            }
        }

        let diagnostic = matcher
            .diagnostic()
            .expect("timesliced matcher should finish with a diagnostic");
        let solution = diagnostic
            .accepted_solution()
            .expect("reference dump should produce an accepted solution");
        let manual_translation = vec3(
            manual_pose.shift_x_meters,
            solution.translation.y,
            manual_pose.shift_z_meters,
        );
        let planar_error = vec3(
            solution.translation.x - manual_translation.x,
            0.0,
            solution.translation.z - manual_translation.z,
        )
        .length();
        let yaw_error = wrap_angle(solution.yaw_radians - manual_pose.rotation_radians).abs();

        assert_eq!(diagnostic.step_count, measured_steps, "{diagnostic:?}");
        assert!(
            diagnostic.max_step_ms <= 50,
            "diagnostic={diagnostic:?} measured_max_step={:?}",
            max_step
        );
        assert!(yaw_error <= 0.10, "{solution:?} manual={manual_pose:?}");
        assert!(planar_error <= 0.08, "{solution:?} manual={manual_pose:?}");
    }

    #[test]
    #[ignore = "expensive randomized stress run"]
    fn dense_height_map_solver_handles_random_noisy_inputs_parallel_stress() {
        let mut handles = Vec::new();
        for thread_index in 0..16u64 {
            handles.push(std::thread::spawn(move || {
                run_random_noisy_cases(
                    0x9e37_79b9_7f4a_7c15 ^ (thread_index.wrapping_mul(0x94d0_49bb_1331_11eb)),
                    1,
                    200,
                    150,
                )
            }));
        }

        let mut summary = RandomCaseSummary::default();
        for handle in handles {
            summary.merge(
                handle
                    .join()
                    .expect("random stress worker should not panic"),
            );
        }

        assert!(
            summary.accepted_cases >= 14,
            "parallel stress summary={summary:?}"
        );
        assert!(
            summary.seeded_reuse_cases >= 8,
            "parallel stress summary={summary:?}"
        );
    }
}
