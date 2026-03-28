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
const XR_DEPTH_ALIGN_HEIGHT_MAP_GRADIENT_MIN_METERS: f32 = 0.05;
const XR_DEPTH_ALIGN_HEIGHT_MAP_MIN_SPACING_METERS: f32 = 0.14;
const XR_DEPTH_ALIGN_SIGNAL_MATCH_RADIUS_METERS: f32 = 0.42;
const XR_DEPTH_ALIGN_SIGNAL_MATCH_MIN_DIRECTION_DOT: f32 = 0.45;
const XR_DEPTH_ALIGN_SIGNAL_MATCH_MAX_HEIGHT_DELTA_METERS: f32 = 0.75;
const XR_DEPTH_ALIGN_SEEDED_LOCK_MIN_CONFIDENCE: f32 = 0.20;
const XR_DEPTH_ALIGN_SEEDED_LOCK_MIN_SYMMETRY_CONFIDENCE: f32 = 0.18;
const XR_DEPTH_ALIGN_SEEDED_LOCK_MIN_OVERLAP: f32 = 0.35;
const XR_DEPTH_ALIGN_SEEDED_LOCK_MAX_TRANSLATION_JUMP_METERS: f32 = 0.75;
const XR_DEPTH_ALIGN_SEEDED_LOCK_MAX_YAW_JUMP_RADIANS: f32 = 0.45;

#[derive(Clone, Copy, Debug)]
struct HeightMapSignalCell {
    point: Vec3f,
    height: f32,
    gradient: Vec3f,
    weight: f32,
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
    let local_signal = selected_descriptor_signal_cells(local);
    let remote_signal = selected_descriptor_signal_cells(remote);
    let remote_dense_signal = descriptor_signal_cells(remote);

    let diagnostic = XrDepthAlignSolveDiagnostic {
        local_vertical_descriptor: local.vertical_descriptor.is_some(),
        remote_vertical_descriptor: remote.vertical_descriptor.is_some(),
        local_wall_samples: local_signal.len(),
        remote_wall_samples: remote_signal.len(),
        ..XrDepthAlignSolveDiagnostic::default()
    };

    if local_signal.len() < XR_DEPTH_ALIGN_MIN_SIGNAL_SAMPLES
        || remote_signal.len() < XR_DEPTH_ALIGN_MIN_SIGNAL_SAMPLES
    {
        return diagnostic;
    }

    let floor_y = local.floor_y - remote.floor_y;
    let local_map = local.height_map.as_ref();
    let remote_map = remote.height_map.as_ref();
    let local_histogram =
        build_height_map_signal_histogram(&local_signal, XR_DEPTH_ALIGN_HEIGHT_MAP_HISTOGRAM_BINS);
    let remote_histogram =
        build_height_map_signal_histogram(&remote_signal, XR_DEPTH_ALIGN_HEIGHT_MAP_HISTOGRAM_BINS);
    let local_signal_refs = local_signal.iter().collect::<Vec<_>>();
    let remote_signal_refs = remote_signal.iter().collect::<Vec<_>>();

    let mut sample_diagnostic = diagnostic;
    let seeded_candidate = previous_solution.map(|seed| {
        sample_diagnostic.yaw_candidate_count += 1;
        sample_diagnostic.pose_candidate_count += 1;
        refine_seed_alignment_solution(
            &local_signal_refs,
            &remote_signal_refs,
            local_map,
            remote_map,
            &remote_dense_signal,
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
            remote_signal.len(),
            local_map,
            remote_map,
        ) {
            return sample_diagnostic;
        }
    }

    let mut best = seeded_candidate;
    let yaw_candidates = candidate_signal_yaws(
        &local_histogram,
        &remote_histogram,
        &local_signal,
        &remote_signal,
    );
    sample_diagnostic.yaw_candidate_count = yaw_candidates.len();
    for yaw in yaw_candidates {
        let translations =
            candidate_signal_translations(&local_signal, &remote_signal, floor_y, yaw);
        sample_diagnostic.pose_candidate_count += translations.len();
        for translation in translations {
            let (mut refined_yaw, mut refined_translation) = refine_signal_alignment(
                &local_signal_refs,
                &remote_signal_refs,
                floor_y,
                yaw,
                translation,
            );
            if let (Some(local_map), Some(remote_map)) = (local_map, remote_map) {
                if !remote_dense_signal.is_empty() {
                    (refined_yaw, refined_translation) = refine_height_map_alignment(
                        local_map,
                        remote_map,
                        &remote_dense_signal,
                        refined_yaw,
                        refined_translation,
                    );
                    refined_translation.y = floor_y;
                }
            }
            let candidate = score_full_alignment_solution(
                &local_signal_refs,
                &remote_signal_refs,
                local_map,
                remote_map,
                &remote_dense_signal,
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
    sample_diagnostic
}

pub fn xr_depth_align_rescore_remote_to_local(
    local: &XrDepthAlignDescriptor,
    remote: &XrDepthAlignDescriptor,
    solution: XrDepthAlignSolution,
) -> XrDepthAlignSolution {
    let local_signal = selected_descriptor_signal_cells(local);
    let remote_signal = selected_descriptor_signal_cells(remote);
    let remote_dense_signal = descriptor_signal_cells(remote);
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
    let local_signal_refs = local_signal.iter().collect::<Vec<_>>();
    let remote_signal_refs = remote_signal.iter().collect::<Vec<_>>();
    apply_height_map_alignment_support(
        score_signal_alignment_solution(
            &local_signal_refs,
            &remote_signal_refs,
            solution.yaw_radians,
            solution.translation,
        ),
        local.height_map.as_ref(),
        remote.height_map.as_ref(),
        &remote_dense_signal,
    )
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
    let center = decode_height_map_height(
        height_map,
        height_map.height_u16[height_map.cell_index(x, z)],
    )?;
    let left = decode_height_map_height(
        height_map,
        height_map.height_u16[height_map.cell_index(x - 1, z)],
    )?;
    let right = decode_height_map_height(
        height_map,
        height_map.height_u16[height_map.cell_index(x + 1, z)],
    )?;
    let up = decode_height_map_height(
        height_map,
        height_map.height_u16[height_map.cell_index(x, z - 1)],
    )?;
    let down = decode_height_map_height(
        height_map,
        height_map.height_u16[height_map.cell_index(x, z + 1)],
    )?;
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
    if size_x < 3 || size_z < 3 || height_map.height_u16.len() != size_x * size_z {
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
        let Some(axis) = xz_axis(cell.gradient) else {
            continue;
        };
        let angle = axis.x.atan2(-axis.z);
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

fn descriptor_signal_cells(descriptor: &XrDepthAlignDescriptor) -> Vec<HeightMapSignalCell> {
    descriptor
        .height_map
        .as_ref()
        .map(build_height_map_signal_cells)
        .unwrap_or_default()
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

fn sample_height_map_signal_nearest(
    height_map: &XrDepthAlignHeightMap,
    world_x: f32,
    world_z: f32,
) -> Option<(f32, Vec3f)> {
    let size_x = height_map.size_x_usize();
    let size_z = height_map.size_z_usize();
    if size_x < 3 || size_z < 3 || height_map.height_u16.len() != size_x * size_z {
        return None;
    }
    let cell_size = height_map.cell_size_meters.max(1.0e-3);
    let grid_x = ((world_x - height_map.origin_x) / cell_size).round() as isize;
    let grid_z = ((world_z - height_map.origin_z) / cell_size).round() as isize;
    if grid_x <= 0 || grid_z <= 0 || grid_x + 1 >= size_x as isize || grid_z + 1 >= size_z as isize
    {
        return None;
    }
    height_map_cell_signal(height_map, grid_x as usize, grid_z as usize)
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

    let h00 = decode_height_map_height(
        height_map,
        height_map.height_u16[height_map.cell_index(x0, z0)],
    );
    let h10 = decode_height_map_height(
        height_map,
        height_map.height_u16[height_map.cell_index(x1, z0)],
    );
    let h01 = decode_height_map_height(
        height_map,
        height_map.height_u16[height_map.cell_index(x0, z1)],
    );
    let h11 = decode_height_map_height(
        height_map,
        height_map.height_u16[height_map.cell_index(x1, z1)],
    );
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
        let height_similarity = (1.0 - diff / 0.45).clamp(0.0, 1.0);
        let direction_similarity = sample_height_map_signal_nearest(local_map, mapped.x, mapped.z)
            .and_then(|(_, local_gradient)| {
                let local_axis = xz_axis(local_gradient)?;
                let remote_axis = xz_axis(rotate_y(yaw, cell.gradient))?;
                Some(local_axis.dot(remote_axis).clamp(0.0, 1.0))
            })
            .unwrap_or(0.5);
        let similarity = (height_similarity * 0.65 + direction_similarity * 0.35).clamp(0.0, 1.0);
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
    local_signal: &[&HeightMapSignalCell],
    remote_signal: &[&HeightMapSignalCell],
    local_map: Option<&XrDepthAlignHeightMap>,
    remote_map: Option<&XrDepthAlignHeightMap>,
    remote_dense_signal: &[HeightMapSignalCell],
    yaw: f32,
    translation: Vec3f,
) -> XrDepthAlignSolution {
    apply_height_map_alignment_support(
        score_signal_alignment_solution(local_signal, remote_signal, yaw, translation),
        local_map,
        remote_map,
        remote_dense_signal,
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
        let Some(local_axis) = xz_axis(local_cell.gradient) else {
            continue;
        };
        for remote_cell in remote_signal.iter().take(16) {
            let Some(remote_axis) = xz_axis(remote_cell.gradient) else {
                continue;
            };
            candidates.push(wrap_angle(signed_xz_angle(remote_axis, local_axis)));
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
    local_signal: &[&'a HeightMapSignalCell],
    remote_signal: &[&'a HeightMapSignalCell],
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
    local_signal: &[&HeightMapSignalCell],
    remote_signal: &[&HeightMapSignalCell],
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
    local_signal: &[&HeightMapSignalCell],
    remote_signal: &[&HeightMapSignalCell],
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

fn refine_seed_alignment_solution(
    local_signal: &[&HeightMapSignalCell],
    remote_signal: &[&HeightMapSignalCell],
    local_map: Option<&XrDepthAlignHeightMap>,
    remote_map: Option<&XrDepthAlignHeightMap>,
    remote_dense_signal: &[HeightMapSignalCell],
    floor_y: f32,
    seed: XrDepthAlignSolution,
) -> XrDepthAlignSolution {
    let mut best_yaw = wrap_angle(seed.yaw_radians);
    let mut best_translation = seed.translation;
    best_translation.y = floor_y;
    if let (Some(local_map), Some(remote_map)) = (local_map, remote_map) {
        if !remote_dense_signal.is_empty() {
            (best_yaw, best_translation) = refine_height_map_alignment(
                local_map,
                remote_map,
                remote_dense_signal,
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
        let mut height_u16 = vec![0u16; size * size];
        for z in 0..size {
            for x in 0..size {
                let point = vec2f(
                    origin + (x as f32 + 0.5) * cell_size_meters,
                    origin + (z as f32 + 0.5) * cell_size_meters,
                );
                let scene_point = map_to_scene
                    .transform_vec4(vec4f(point.x, 0.0, point.y, 1.0))
                    .to_vec3f();
                height_u16[x + z * size] = encode_test_height(
                    synthetic_scene_height(vec2f(scene_point.x, scene_point.z)),
                    bottom_y_meters,
                    top_y_meters,
                );
            }
        }
        XrDepthAlignDescriptor {
            voxel_size_meters: 0.05,
            floor_y: 0.0,
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
                player_cutout_center: None,
                player_cutout_radius_meters: 0.0,
                height_u16,
            }),
        }
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
    fn dense_seeded_solver_falls_back_when_seed_is_stale() {
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
    fn dense_height_map_solver_handles_random_noisy_inputs() {
        let summary = run_random_noisy_cases(0x5eed_cafe_d15c_a11e, 4, 120, 114);
        assert!(summary.accepted_cases >= 4, "summary={summary:?}");
        assert!(summary.seeded_reuse_cases >= 2, "summary={summary:?}");
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
