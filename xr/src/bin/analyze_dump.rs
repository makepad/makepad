#![allow(dead_code, unused_mut, unused_variables)]

use makepad_xr::*;
use std::{
    env,
    f32::consts::{PI, TAU},
    fs,
    path::{Path, PathBuf},
    time::{Instant, SystemTime},
};

#[derive(Clone, Copy, Debug, Default)]
struct AnalyzeOptions {
    solve: bool,
    latest_count: usize,
}

const FLOOR_BIN_METERS: f32 = 0.04;
const FLOOR_MIN_SUPPORT_RATIO: f32 = 0.005;
const FLOOR_MIN_SUPPORT_CELLS: usize = 24;
const FLOOR_SUPPORT_WINDOW_BINS: usize = 2;
const SHAPE_OCCUPIED_MIN_HEIGHT_METERS: f32 = 0.18;
const SHAPE_MATCH_DISTANCE_TRUNCATE_METERS: f32 = 0.60;
const SHAPE_GLOBAL_CELL_METERS: f32 = 0.24;
const SHAPE_REFINE_CELL_METERS: f32 = 0.12;
const SHAPE_FINAL_CELL_METERS: f32 = 0.06;
const SHAPE_GLOBAL_YAW_STEP_RADIANS: f32 = 10.0_f32.to_radians();
const SHAPE_REFINE_YAW_WINDOW_RADIANS: f32 = 12.0_f32.to_radians();
const SHAPE_REFINE_YAW_STEP_RADIANS: f32 = 2.0_f32.to_radians();
const SHAPE_FINAL_YAW_WINDOW_RADIANS: f32 = 3.0_f32.to_radians();
const SHAPE_FINAL_YAW_STEP_RADIANS: f32 = 0.5_f32.to_radians();
const SHAPE_GLOBAL_TOP_K: usize = 12;
const SHAPE_REFINE_TOP_K: usize = 8;
const SHAPE_FINAL_TOP_K: usize = 6;
const SHAPE_MIN_CONTOUR_POINTS: usize = 12;
const SHAPE_FURNITURE_MIN_HEIGHT_METERS: f32 = 0.22;
const SHAPE_FURNITURE_MAX_HEIGHT_METERS: f32 = 1.10;
const SHAPE_FURNITURE_MIN_COMPONENT_AREA_METERS2: f32 = 0.05;
const SHAPE_SCAN_FLOOR_OFFSETS_METERS: [f32; 5] = [-0.10, -0.05, 0.0, 0.05, 0.10];
const SHAPE_SEEDED_PRIOR_TRANSLATION_SIGMA_METERS: f32 = 0.32;
const SHAPE_SEEDED_PRIOR_YAW_SIGMA_RADIANS: f32 = 9.0_f32.to_radians();
const SHAPE_SEEDED_PRIOR_FLOOR: f32 = 0.55;
const SHAPE_SEEDED_SIGNED_EMPTY_PENALTY: f32 = 1.10;
const SHAPE_SEEDED_UNSUPPORTED_PENALTY: f32 = 2.50;
const SHAPE_SEEDED_INTERIOR_BONUS_CAP_METERS: f32 = 0.18;
const SHAPE_SEEDED_LOCAL_TRANSLATION_WINDOW_METERS: f32 = 0.42;
const SHAPE_SEEDED_LOCAL_YAW_WINDOW_RADIANS: f32 = 16.0_f32.to_radians();
const SHAPE_SEEDED_LOCAL_YAW_STEP_RADIANS: f32 = 1.0_f32.to_radians();
const SHAPE_HOUGH_THETA_BINS: usize = 180;
const SHAPE_HOUGH_TOP_PEAKS: usize = 4;
const SHAPE_HOUGH_THETA_SUPPRESS_RADIANS: f32 = 8.0_f32.to_radians();
const SHAPE_COMPONENT_CONTEXT_RADIAL_BINS: usize = 3;
const SHAPE_COMPONENT_CONTEXT_ANGULAR_BINS: usize = 12;
const SHAPE_COMPONENT_CONTEXT_CHANNELS: usize = 3;
const SHAPE_COMPONENT_CONTEXT_RADIUS_METERS: f32 = 2.40;
const SHAPE_COMPONENT_CONTEXT_SIGNATURE_LEN: usize = SHAPE_COMPONENT_CONTEXT_CHANNELS
    * SHAPE_COMPONENT_CONTEXT_RADIAL_BINS
    * SHAPE_COMPONENT_CONTEXT_ANGULAR_BINS;
const SHAPE_COMPONENT_SIGNATURE_BINS: usize = SHAPE_COMPONENT_CONTEXT_ANGULAR_BINS;
const SHAPE_CORR_COARSE_CELL_METERS: f32 = 0.48;
const SHAPE_CORR_MEDIUM_CELL_METERS: f32 = 0.24;
const SHAPE_CORR_FINE_CELL_METERS: f32 = 0.12;
const SHAPE_CORR_FINAL_CELL_METERS: f32 = 0.06;
const SHAPE_CORR_COARSE_YAW_STEP_RADIANS: f32 = 8.0_f32.to_radians();
const SHAPE_CORR_MEDIUM_YAW_STEP_RADIANS: f32 = 3.0_f32.to_radians();
const SHAPE_CORR_FINE_YAW_STEP_RADIANS: f32 = 1.0_f32.to_radians();
const SHAPE_CORR_FINAL_YAW_STEP_RADIANS: f32 = 0.5_f32.to_radians();
const SHAPE_CORR_COARSE_TOP_K: usize = 96;
const SHAPE_CORR_MEDIUM_TOP_K: usize = 64;
const SHAPE_CORR_FINE_TOP_K: usize = 32;
const SHAPE_CORR_FINAL_TOP_K: usize = 12;
const ANALYSIS_MAX_THREADS: usize = 16;
const FOCUSED_HOUGH_TOP_LINES: usize = 10;
const FOCUSED_HOUGH_RHO_SUPPRESS_METERS: f32 = 0.16;
const FOCUSED_HOUGH_ANGLE_SIGMA_RADIANS: f32 = 5.0_f32.to_radians();
const FOCUSED_HOUGH_RHO_SIGMA_METERS: f32 = 0.18;
const FOCUSED_COMPONENT_MAX_DISTANCE_METERS: f32 = 0.70;
const FOCUSED_COMPONENT_DISTANCE_SIGMA_METERS: f32 = 0.24;
const FOCUSED_LOCAL_TRANSLATION_WINDOW_METERS: f32 = 0.30;
const FOCUSED_LOCAL_YAW_WINDOW_RADIANS: f32 = 10.0_f32.to_radians();
const FOCUSED_LOCAL_YAW_STEP_RADIANS: f32 = 1.0_f32.to_radians();
const FOCUSED_REFINE_TRANSLATION_WINDOW_METERS: f32 = 0.12;
const FOCUSED_REFINE_YAW_WINDOW_RADIANS: f32 = 2.0_f32.to_radians();
const FOCUSED_REFINE_YAW_STEP_RADIANS: f32 = 0.25_f32.to_radians();
const FOCUSED_SEARCH_TOP_K: usize = 24;
const FOCUSED_FINAL_TOP_K: usize = 6;
const ANALYZE_TSDF_CALLBACK_INTERVAL_MILLIS: u64 = 8;
const ANALYZE_TSDF_CALLBACK_BUDGET_MILLIS: u64 = 40;
const ANALYZE_TSDF_CALLBACK_MAX_STEPS: usize = 4096;

#[derive(Clone, Copy, Debug, Default)]
struct ManualPose {
    shift_x_meters: f32,
    shift_y_meters: f32,
    rotation_radians: f32,
}

#[derive(Clone, Debug)]
struct ShapeMatchGrid {
    origin_x: f32,
    origin_z: f32,
    cell_size_meters: f32,
    size_x: usize,
    size_z: usize,
    occupied_mask: Vec<bool>,
    occupied_weight: Vec<f32>,
    occupied_points: Vec<(f32, f32)>,
    distance_meters: Vec<f32>,
    signed_distance_meters: Vec<f32>,
    contour_points: Vec<(f32, f32)>,
    support_mask: Vec<bool>,
    support_points: Vec<(f32, f32)>,
    free_mask: Vec<bool>,
}

#[derive(Clone, Debug)]
struct ShapeBandSpec {
    label: String,
    min_height_meters: f32,
    max_height_meters: Option<f32>,
    floor_offset_meters: f32,
    drop_border_components: bool,
}

#[derive(Clone, Copy, Debug, Default)]
struct ShapeMatchCandidate {
    score: f32,
    feature_score: f32,
    support_score: f32,
    coverage: f32,
    close_ratio: f32,
    mean_distance_meters: f32,
    yaw_radians: f32,
    translation_x: f32,
    translation_z: f32,
    in_bounds_points: usize,
}

#[derive(Clone, Debug)]
struct ShapeMatchReport {
    label: String,
    finalists: Vec<ShapeMatchCandidate>,
    manual_candidate: Option<ShapeMatchCandidate>,
}

#[derive(Clone, Copy, Debug)]
struct HoughLine {
    theta_radians: f32,
    rho_meters: f32,
    strength: f32,
}

#[derive(Clone, Copy, Debug)]
struct FocusedCueWeights {
    label: &'static str,
    runtime_weight: f32,
    signed_weight: f32,
    line_weight: f32,
    blob_weight: f32,
}

#[derive(Clone, Copy, Debug, Default)]
struct FocusedPoseEvidence {
    pose: ShapeMatchCandidate,
    signed_score: f32,
    line_score: f32,
    blob_score: f32,
    runtime_score: f32,
    runtime_residual_meters: f32,
    seed_prior: f32,
}

#[derive(Clone, Copy, Debug, Default)]
struct FocusedMatchCandidate {
    pose: ShapeMatchCandidate,
    score: f32,
    runtime_score: f32,
    runtime_residual_meters: f32,
    signed_score: f32,
    line_score: f32,
    blob_score: f32,
    seed_prior: f32,
}

#[derive(Clone, Debug)]
struct FocusedMatchReport {
    label: String,
    finalists: Vec<FocusedMatchCandidate>,
    manual_candidate: Option<FocusedMatchCandidate>,
}

#[derive(Clone, Copy, Debug)]
struct NearestManualFinalist {
    candidate: ShapeMatchCandidate,
    planar_delta_meters: f32,
    yaw_delta_radians: f32,
    combined_distance: f32,
}

#[derive(Clone, Debug)]
struct ShapeBandScanResult {
    report: ShapeMatchReport,
    nearest_manual_finalist: Option<NearestManualFinalist>,
}

#[derive(Clone)]
struct BandGridPair {
    local: ShapeMatchGrid,
    remote: ShapeMatchGrid,
}

#[derive(Clone, Copy, Debug)]
struct ShapeComponent {
    centroid_x: f32,
    centroid_z: f32,
    area_m2: f32,
    score: f32,
    compactness: f32,
    radial_signature: [f32; SHAPE_COMPONENT_SIGNATURE_BINS],
    context_signature: [f32; SHAPE_COMPONENT_CONTEXT_SIGNATURE_LEN],
}

fn analysis_thread_count(work_items: usize) -> usize {
    let available = std::thread::available_parallelism()
        .map(|parallelism| parallelism.get())
        .unwrap_or(1);
    work_items.max(1).min(available).min(ANALYSIS_MAX_THREADS)
}

fn latest_dump_paths(count: usize) -> Vec<PathBuf> {
    let dump_dir = PathBuf::from("xr/util/dumps");
    let mut entries = fs::read_dir(dump_dir)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let metadata = entry.metadata().ok()?;
            metadata
                .is_file()
                .then_some((entry.path(), metadata.modified().ok()?))
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| right.1.cmp(&left.1));
    entries
        .into_iter()
        .filter_map(|(path, _)| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".bin") && name != "manual-smoke.bin")
                .then_some(path)
        })
        .take(count.max(1))
        .collect()
}

fn parse_args() -> Result<(AnalyzeOptions, Vec<PathBuf>), String> {
    let mut options = AnalyzeOptions {
        latest_count: 1,
        ..AnalyzeOptions::default()
    };
    let mut paths = Vec::<PathBuf>::new();
    let mut args = env::args_os().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--solve" {
            options.solve = true;
            continue;
        }
        if arg == "--latest" {
            let Some(count) = args.next() else {
                return Err("expected a count after --latest".to_string());
            };
            let count = count
                .to_string_lossy()
                .parse::<usize>()
                .map_err(|_| "failed to parse --latest count".to_string())?;
            options.latest_count = count.max(1);
            continue;
        }
        paths.push(PathBuf::from(arg));
    }
    if paths.is_empty() {
        paths = latest_dump_paths(options.latest_count);
    }
    if paths.is_empty() {
        return Err("no dump files found in xr/util/dumps".to_string());
    }
    Ok((options, paths))
}

fn count_valid_heights(height_map: &XrDepthAlignHeightMap) -> usize {
    height_map
        .heights_meters
        .iter()
        .filter(|height| height.is_finite())
        .count()
}

fn min_max_height(height_map: &XrDepthAlignHeightMap) -> Option<(f32, f32)> {
    let mut min_height = f32::INFINITY;
    let mut max_height = f32::NEG_INFINITY;
    for height in height_map
        .heights_meters
        .iter()
        .copied()
        .filter(|height| height.is_finite())
    {
        min_height = min_height.min(height);
        max_height = max_height.max(height);
    }
    (min_height.is_finite() && max_height.is_finite()).then_some((min_height, max_height))
}

fn sorted_heights(height_map: &XrDepthAlignHeightMap) -> Vec<f32> {
    let mut heights = height_map
        .heights_meters
        .iter()
        .copied()
        .filter(|height| height.is_finite())
        .collect::<Vec<_>>();
    heights.sort_by(|left, right| left.total_cmp(right));
    heights
}

fn percentile(sorted_heights: &[f32], t: f32) -> Option<f32> {
    let last = sorted_heights.len().checked_sub(1)?;
    let index = (last as f32 * t.clamp(0.0, 1.0)).round() as usize;
    sorted_heights.get(index).copied()
}

fn recompute_floor_y(height_map: &XrDepthAlignHeightMap) -> Option<f32> {
    let heights = sorted_heights(height_map);
    if heights.is_empty() {
        return None;
    }
    let bottom_y = height_map.bottom_y_meters;
    let top_y = height_map.top_y_meters;
    let span = (top_y - bottom_y).max(1.0e-3);
    let bin_size = FLOOR_BIN_METERS.max(span / 256.0).min(span);
    let bin_count = ((span / bin_size).ceil() as usize).max(1) + 1;
    let mut bins = vec![0usize; bin_count];
    for height in &heights {
        let bin = (((*height - bottom_y) / bin_size).floor() as isize)
            .clamp(0, bin_count.saturating_sub(1) as isize) as usize;
        bins[bin] += 1;
    }
    let min_support = FLOOR_MIN_SUPPORT_CELLS
        .max((heights.len() as f32 * FLOOR_MIN_SUPPORT_RATIO).ceil() as usize);
    let support_window_bins = FLOOR_SUPPORT_WINDOW_BINS.max(1);
    for start_bin in 0..bin_count {
        let end_bin = (start_bin + support_window_bins).min(bin_count);
        let support = bins[start_bin..end_bin].iter().copied().sum::<usize>();
        if support < min_support {
            continue;
        }
        let low = bottom_y + start_bin as f32 * bin_size;
        let high = bottom_y + end_bin as f32 * bin_size;
        let window = heights
            .iter()
            .copied()
            .filter(|height| *height >= low && *height <= high)
            .collect::<Vec<_>>();
        if !window.is_empty() {
            return Some(window.iter().copied().sum::<f32>() / window.len() as f32);
        }
    }
    let fallback_count = ((heights.len() as f32 * 0.01).ceil() as usize).clamp(1, 32);
    let fallback = &heights[..fallback_count];
    Some(fallback.iter().copied().sum::<f32>() / fallback.len() as f32)
}

fn print_height_map_stats(label: &str, height_map: Option<&XrDepthAlignHeightMap>) {
    let Some(height_map) = height_map else {
        println!("{label}: no height map");
        return;
    };
    let valid_cells = count_valid_heights(height_map);
    let cell_count = height_map.cell_count();
    let min_max = min_max_height(height_map);
    println!(
        "{label}: {}x{} cells, valid {} / {}, cell {:.3} m, bounds_y [{:.3}, {:.3}], floor_y {:.3}",
        height_map.size_x,
        height_map.size_z,
        valid_cells,
        cell_count,
        height_map.cell_size_meters,
        height_map.bottom_y_meters,
        height_map.top_y_meters,
        height_map.floor_y_meters,
    );
    if let Some((min_height, max_height)) = min_max {
        println!(
            "{label}: observed height range [{:.3}, {:.3}]",
            min_height, max_height
        );
    }
    let sorted = sorted_heights(height_map);
    if !sorted.is_empty() {
        println!(
            "{label}: percentiles p01 {:.3} | p05 {:.3} | p08 {:.3} | p10 {:.3} | p20 {:.3}",
            percentile(&sorted, 0.01).unwrap_or(f32::NAN),
            percentile(&sorted, 0.05).unwrap_or(f32::NAN),
            percentile(&sorted, 0.08).unwrap_or(f32::NAN),
            percentile(&sorted, 0.10).unwrap_or(f32::NAN),
            percentile(&sorted, 0.20).unwrap_or(f32::NAN),
        );
    }
    if let Some(recomputed_floor_y) = recompute_floor_y(height_map) {
        println!(
            "{label}: recomputed floor_y {:.3} (delta {:.3})",
            recomputed_floor_y,
            recomputed_floor_y - height_map.floor_y_meters
        );
    }
}

fn print_descriptor_stats(label: &str, descriptor: &XrDepthAlignDescriptor) {
    println!(
        "{label}: floor_y {:.3}, samples {} (walls {} floors {}), vertical {}",
        descriptor.floor_y,
        descriptor.samples.len(),
        descriptor
            .samples
            .iter()
            .filter(|sample| sample.kind == XrDepthAlignSampleKind::Wall)
            .count(),
        descriptor
            .samples
            .iter()
            .filter(|sample| sample.kind == XrDepthAlignSampleKind::Floor)
            .count(),
        descriptor.vertical_descriptor.is_some()
    );
    print_height_map_stats(label, descriptor.height_map.as_ref());
    if let Some(markers) = xr_depth_align_test_markers(descriptor) {
        println!(
            "{label}: markers ({:.2}, {:.2}, {:.2}) and ({:.2}, {:.2}, {:.2})",
            markers[0].x, markers[0].y, markers[0].z, markers[1].x, markers[1].y, markers[1].z
        );
    } else {
        println!("{label}: no test markers");
    }
}

fn explain_outcome(diagnostic: &XrDepthAlignSolveDiagnostic) -> &'static str {
    match diagnostic.outcome() {
        XrDepthAlignSolveOutcome::MissingSamples => "missing samples",
        XrDepthAlignSolveOutcome::NoCandidate => "no candidate survived matching",
        XrDepthAlignSolveOutcome::Rejected => "solver found a candidate but rejected it",
        XrDepthAlignSolveOutcome::Accepted => "accepted",
    }
}

fn system_time_hint(path: &Path) -> String {
    fs::metadata(path)
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|modified| modified.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

fn manual_sidecar_path(dump_path: &Path) -> PathBuf {
    let stem = dump_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("align-pair");
    dump_path.with_file_name(format!("{stem}.manual_pose.ron"))
}

fn load_manual_pose(dump_path: &Path) -> Option<ManualPose> {
    let text = fs::read_to_string(manual_sidecar_path(dump_path)).ok()?;
    let mut pose = ManualPose::default();
    for line in text.lines() {
        let (key, value) = line.split_once(':')?;
        let value = value.trim();
        match key.trim() {
            "shift_x_meters" => pose.shift_x_meters = value.parse().ok()?,
            "shift_y_meters" => pose.shift_y_meters = value.parse().ok()?,
            "rotation_radians" => pose.rotation_radians = value.parse().ok()?,
            _ => {}
        }
    }
    Some(pose)
}

fn manual_pose_solution(
    local: &XrDepthAlignDescriptor,
    remote: &XrDepthAlignDescriptor,
    pose: ManualPose,
) -> XrDepthAlignSolution {
    XrDepthAlignSolution {
        yaw_radians: pose.rotation_radians,
        translation: vec3(
            pose.shift_x_meters,
            local.floor_y - remote.floor_y,
            pose.shift_y_meters,
        ),
        confidence: 0.0,
        symmetry_confidence: 0.0,
        residual_meters: f32::INFINITY,
        matched_samples: 0,
    }
}

fn manual_pose_delta_text(solution: XrDepthAlignSolution, manual_pose: ManualPose) -> String {
    let manual_yaw = manual_pose.rotation_radians;
    let manual_translation = vec3(
        manual_pose.shift_x_meters,
        solution.translation.y,
        manual_pose.shift_y_meters,
    );
    format!(
        "manual_delta: yaw {:.3} rad ({:.1} deg) | planar {:.3} m | dy {:.3} m",
        wrap_angle(solution.yaw_radians - manual_yaw),
        wrap_angle(solution.yaw_radians - manual_yaw).to_degrees(),
        vec3(
            solution.translation.x - manual_translation.x,
            0.0,
            solution.translation.z - manual_translation.z,
        )
        .length(),
        solution.translation.y - manual_translation.y,
    )
}

impl ShapeMatchGrid {
    fn cell_index(&self, x: usize, z: usize) -> usize {
        x + z * self.size_x
    }

    fn max_x(&self) -> f32 {
        self.origin_x + self.size_x as f32 * self.cell_size_meters
    }

    fn max_z(&self) -> f32 {
        self.origin_z + self.size_z as f32 * self.cell_size_meters
    }

    fn sample_distance_nearest(&self, world_x: f32, world_z: f32) -> Option<f32> {
        let local_x = (world_x - self.origin_x) / self.cell_size_meters;
        let local_z = (world_z - self.origin_z) / self.cell_size_meters;
        if !local_x.is_finite()
            || !local_z.is_finite()
            || local_x < 0.0
            || local_z < 0.0
            || local_x >= self.size_x as f32
            || local_z >= self.size_z as f32
        {
            return None;
        }
        let x = local_x.floor() as usize;
        let z = local_z.floor() as usize;
        self.distance_meters.get(self.cell_index(x, z)).copied()
    }

    fn sample_free_nearest(&self, world_x: f32, world_z: f32) -> Option<bool> {
        let local_x = (world_x - self.origin_x) / self.cell_size_meters;
        let local_z = (world_z - self.origin_z) / self.cell_size_meters;
        if !local_x.is_finite()
            || !local_z.is_finite()
            || local_x < 0.0
            || local_z < 0.0
            || local_x >= self.size_x as f32
            || local_z >= self.size_z as f32
        {
            return None;
        }
        let x = local_x.floor() as usize;
        let z = local_z.floor() as usize;
        self.free_mask.get(self.cell_index(x, z)).copied()
    }

    fn sample_support_nearest(&self, world_x: f32, world_z: f32) -> Option<bool> {
        let local_x = (world_x - self.origin_x) / self.cell_size_meters;
        let local_z = (world_z - self.origin_z) / self.cell_size_meters;
        if !local_x.is_finite()
            || !local_z.is_finite()
            || local_x < 0.0
            || local_z < 0.0
            || local_x >= self.size_x as f32
            || local_z >= self.size_z as f32
        {
            return None;
        }
        let x = local_x.floor() as usize;
        let z = local_z.floor() as usize;
        self.support_mask.get(self.cell_index(x, z)).copied()
    }

    fn sample_occupied_nearest(&self, world_x: f32, world_z: f32) -> Option<bool> {
        let local_x = (world_x - self.origin_x) / self.cell_size_meters;
        let local_z = (world_z - self.origin_z) / self.cell_size_meters;
        if !local_x.is_finite()
            || !local_z.is_finite()
            || local_x < 0.0
            || local_z < 0.0
            || local_x >= self.size_x as f32
            || local_z >= self.size_z as f32
        {
            return None;
        }
        let x = local_x.floor() as usize;
        let z = local_z.floor() as usize;
        self.occupied_mask.get(self.cell_index(x, z)).copied()
    }

    fn sample_occupied_weight_nearest(&self, world_x: f32, world_z: f32) -> Option<f32> {
        let local_x = (world_x - self.origin_x) / self.cell_size_meters;
        let local_z = (world_z - self.origin_z) / self.cell_size_meters;
        if !local_x.is_finite()
            || !local_z.is_finite()
            || local_x < 0.0
            || local_z < 0.0
            || local_x >= self.size_x as f32
            || local_z >= self.size_z as f32
        {
            return None;
        }
        let x = local_x.floor() as usize;
        let z = local_z.floor() as usize;
        self.occupied_weight.get(self.cell_index(x, z)).copied()
    }

    fn sample_signed_distance_nearest(&self, world_x: f32, world_z: f32) -> Option<f32> {
        let local_x = (world_x - self.origin_x) / self.cell_size_meters;
        let local_z = (world_z - self.origin_z) / self.cell_size_meters;
        if !local_x.is_finite()
            || !local_z.is_finite()
            || local_x < 0.0
            || local_z < 0.0
            || local_x >= self.size_x as f32
            || local_z >= self.size_z as f32
        {
            return None;
        }
        let x = local_x.floor() as usize;
        let z = local_z.floor() as usize;
        self.signed_distance_meters
            .get(self.cell_index(x, z))
            .copied()
    }
}

fn wrap_angle(mut angle: f32) -> f32 {
    while angle > PI {
        angle -= TAU;
    }
    while angle <= -PI {
        angle += TAU;
    }
    angle
}

fn rotate_xz(yaw: f32, x: f32, z: f32) -> (f32, f32) {
    let (sin_yaw, cos_yaw) = yaw.sin_cos();
    (x * cos_yaw - z * sin_yaw, x * sin_yaw + z * cos_yaw)
}

fn line_theta(theta_radians: f32) -> f32 {
    theta_radians.rem_euclid(PI)
}

fn line_angle_delta(left: f32, right: f32) -> f32 {
    let delta = (line_theta(left) - line_theta(right)).abs().rem_euclid(PI);
    delta.min(PI - delta)
}

fn signature_shift_for_angle(angle_radians: f32, bin_count: usize) -> usize {
    ((wrap_angle(angle_radians).rem_euclid(TAU) / TAU) * bin_count as f32)
        .round()
        .rem_euclid(bin_count.max(1) as f32) as usize
}

fn seed_prior_factor(candidate: ShapeMatchCandidate, seed: ShapeMatchCandidate) -> f32 {
    let translation_delta = ((candidate.translation_x - seed.translation_x).powi(2)
        + (candidate.translation_z - seed.translation_z).powi(2))
    .sqrt();
    let yaw_delta = wrap_angle(candidate.yaw_radians - seed.yaw_radians).abs();
    let translation_term =
        -0.5 * (translation_delta / SHAPE_SEEDED_PRIOR_TRANSLATION_SIGMA_METERS).powi(2);
    let yaw_term = -0.5 * (yaw_delta / SHAPE_SEEDED_PRIOR_YAW_SIGMA_RADIANS).powi(2);
    let prior = (translation_term + yaw_term).exp().clamp(0.0, 1.0);
    SHAPE_SEEDED_PRIOR_FLOOR + (1.0 - SHAPE_SEEDED_PRIOR_FLOOR) * prior
}

fn shape_band_obstacle() -> ShapeBandSpec {
    ShapeBandSpec {
        label: "shape".to_string(),
        min_height_meters: SHAPE_OCCUPIED_MIN_HEIGHT_METERS,
        max_height_meters: None,
        floor_offset_meters: 0.0,
        drop_border_components: false,
    }
}

fn shape_band_furniture() -> ShapeBandSpec {
    ShapeBandSpec {
        label: "shape_furniture".to_string(),
        min_height_meters: SHAPE_FURNITURE_MIN_HEIGHT_METERS,
        max_height_meters: Some(SHAPE_FURNITURE_MAX_HEIGHT_METERS),
        floor_offset_meters: 0.0,
        drop_border_components: true,
    }
}

fn shape_band_scans() -> Vec<ShapeBandSpec> {
    let mut bands = Vec::new();
    for &floor_offset in &SHAPE_SCAN_FLOOR_OFFSETS_METERS {
        for min_height in [0.12f32, 0.20, 0.28, 0.36] {
            for max_height in [0.70f32, 0.90, 1.10, 1.35] {
                if max_height - min_height < 0.26 {
                    continue;
                }
                bands.push(ShapeBandSpec {
                    label: format!(
                        "shape_scan_{min_height:.2}_{max_height:.2}_f{floor_offset:+.2}"
                    ),
                    min_height_meters: min_height,
                    max_height_meters: Some(max_height),
                    floor_offset_meters: floor_offset,
                    drop_border_components: true,
                });
            }
        }
    }
    bands
}

fn seeded_consensus_bands() -> Vec<ShapeBandSpec> {
    vec![
        ShapeBandSpec {
            label: "cons_low".to_string(),
            min_height_meters: 0.12,
            max_height_meters: Some(0.70),
            floor_offset_meters: 0.0,
            drop_border_components: true,
        },
        ShapeBandSpec {
            label: "cons_mid".to_string(),
            min_height_meters: 0.20,
            max_height_meters: Some(1.10),
            floor_offset_meters: 0.0,
            drop_border_components: true,
        },
        ShapeBandSpec {
            label: "cons_upper".to_string(),
            min_height_meters: 0.60,
            max_height_meters: Some(1.80),
            floor_offset_meters: 0.0,
            drop_border_components: false,
        },
    ]
}

fn filter_shape_components(
    occupied: &[bool],
    size_x: usize,
    size_z: usize,
    cell_size_meters: f32,
    drop_border_components: bool,
) -> Vec<bool> {
    let mut filtered = vec![false; occupied.len()];
    let mut visited = vec![false; occupied.len()];
    let min_component_cells = (SHAPE_FURNITURE_MIN_COMPONENT_AREA_METERS2
        / (cell_size_meters * cell_size_meters).max(1.0e-4))
    .ceil()
    .max(1.0) as usize;
    for start in 0..occupied.len() {
        if visited[start] || !occupied[start] {
            continue;
        }
        let mut stack = vec![start];
        let mut component = Vec::new();
        let mut touches_border = false;
        visited[start] = true;
        while let Some(index) = stack.pop() {
            component.push(index);
            let x = index % size_x;
            let z = index / size_x;
            if x == 0 || z == 0 || x + 1 == size_x || z + 1 == size_z {
                touches_border = true;
            }
            for (dx, dz) in [(-1isize, 0isize), (1, 0), (0, -1), (0, 1)] {
                let nx = x as isize + dx;
                let nz = z as isize + dz;
                if nx < 0 || nz < 0 || nx >= size_x as isize || nz >= size_z as isize {
                    continue;
                }
                let neighbor = nx as usize + nz as usize * size_x;
                if visited[neighbor] || !occupied[neighbor] {
                    continue;
                }
                visited[neighbor] = true;
                stack.push(neighbor);
            }
        }
        if component.len() < min_component_cells {
            continue;
        }
        if drop_border_components && touches_border {
            continue;
        }
        for index in component {
            filtered[index] = true;
        }
    }
    filtered
}

fn erode_mask(mask: &[bool], size_x: usize, size_z: usize) -> Vec<bool> {
    let mut eroded = vec![false; mask.len()];
    for z in 0..size_z {
        for x in 0..size_x {
            let index = x + z * size_x;
            if !mask[index] {
                continue;
            }
            let mut keep = true;
            for dz in -1isize..=1 {
                for dx in -1isize..=1 {
                    let nx = x as isize + dx;
                    let nz = z as isize + dz;
                    if nx < 0
                        || nz < 0
                        || nx >= size_x as isize
                        || nz >= size_z as isize
                        || !mask[nx as usize + nz as usize * size_x]
                    {
                        keep = false;
                        break;
                    }
                }
                if !keep {
                    break;
                }
            }
            eroded[index] = keep;
        }
    }
    eroded
}

fn dilate_mask(mask: &[bool], size_x: usize, size_z: usize) -> Vec<bool> {
    let mut dilated = vec![false; mask.len()];
    for z in 0..size_z {
        for x in 0..size_x {
            let mut on = false;
            for dz in -1isize..=1 {
                for dx in -1isize..=1 {
                    let nx = x as isize + dx;
                    let nz = z as isize + dz;
                    if nx < 0 || nz < 0 || nx >= size_x as isize || nz >= size_z as isize {
                        continue;
                    }
                    if mask[nx as usize + nz as usize * size_x] {
                        on = true;
                        break;
                    }
                }
                if on {
                    break;
                }
            }
            dilated[x + z * size_x] = on;
        }
    }
    dilated
}

fn clean_binary_mask(mask: &[bool], size_x: usize, size_z: usize) -> Vec<bool> {
    let opened = dilate_mask(&erode_mask(mask, size_x, size_z), size_x, size_z);
    erode_mask(&dilate_mask(&opened, size_x, size_z), size_x, size_z)
}

fn build_support_frontier_mask(grid: &ShapeMatchGrid) -> Vec<bool> {
    let mut frontier = vec![false; grid.support_mask.len()];
    for z in 0..grid.size_z {
        for x in 0..grid.size_x {
            let index = x + z * grid.size_x;
            if !grid.support_mask[index] {
                continue;
            }
            let mut edge = false;
            for (dx, dz) in [(-1isize, 0isize), (1, 0), (0, -1), (0, 1)] {
                let nx = x as isize + dx;
                let nz = z as isize + dz;
                if nx < 0
                    || nz < 0
                    || nx >= grid.size_x as isize
                    || nz >= grid.size_z as isize
                    || !grid.support_mask[nx as usize + nz as usize * grid.size_x]
                {
                    edge = true;
                    break;
                }
            }
            frontier[index] = edge;
        }
    }
    frontier
}

fn component_context_index(channel: usize, radial: usize, angular: usize) -> usize {
    channel * SHAPE_COMPONENT_CONTEXT_RADIAL_BINS * SHAPE_COMPONENT_CONTEXT_ANGULAR_BINS
        + radial * SHAPE_COMPONENT_CONTEXT_ANGULAR_BINS
        + angular
}

fn build_component_context_signature(
    grid: &ShapeMatchGrid,
    support_frontier_mask: &[bool],
    centroid_x: f32,
    centroid_z: f32,
) -> [f32; SHAPE_COMPONENT_CONTEXT_SIGNATURE_LEN] {
    let mut signature = [0.0f32; SHAPE_COMPONENT_CONTEXT_SIGNATURE_LEN];
    let mut channel_totals = [0.0f32; SHAPE_COMPONENT_CONTEXT_CHANNELS];
    for z in 0..grid.size_z {
        for x in 0..grid.size_x {
            let index = x + z * grid.size_x;
            if !grid.support_mask[index] {
                continue;
            }
            let world_x = grid.origin_x + (x as f32 + 0.5) * grid.cell_size_meters;
            let world_z = grid.origin_z + (z as f32 + 0.5) * grid.cell_size_meters;
            let dx = world_x - centroid_x;
            let dz = world_z - centroid_z;
            let distance = (dx * dx + dz * dz).sqrt();
            if distance <= grid.cell_size_meters * 0.5
                || distance > SHAPE_COMPONENT_CONTEXT_RADIUS_METERS
            {
                continue;
            }
            let radial = (((distance / SHAPE_COMPONENT_CONTEXT_RADIUS_METERS)
                * SHAPE_COMPONENT_CONTEXT_RADIAL_BINS as f32)
                .floor() as isize)
                .clamp(
                    0,
                    SHAPE_COMPONENT_CONTEXT_RADIAL_BINS.saturating_sub(1) as isize,
                ) as usize;
            let angle = wrap_angle(dz.atan2(dx));
            let angular = (((angle + PI) / (TAU / SHAPE_COMPONENT_CONTEXT_ANGULAR_BINS as f32))
                .floor() as isize)
                .rem_euclid(SHAPE_COMPONENT_CONTEXT_ANGULAR_BINS as isize)
                as usize;
            if grid.occupied_mask[index] {
                let weight = grid.occupied_weight[index].max(0.35);
                signature[component_context_index(0, radial, angular)] += weight;
                channel_totals[0] += weight;
            } else {
                signature[component_context_index(1, radial, angular)] += 1.0;
                channel_totals[1] += 1.0;
            }
            if support_frontier_mask[index] {
                signature[component_context_index(2, radial, angular)] += 1.0;
                channel_totals[2] += 1.0;
            }
        }
    }
    for channel in 0..SHAPE_COMPONENT_CONTEXT_CHANNELS {
        let total = channel_totals[channel];
        if total <= 1.0e-6 {
            continue;
        }
        let start = component_context_index(channel, 0, 0);
        let end =
            start + SHAPE_COMPONENT_CONTEXT_RADIAL_BINS * SHAPE_COMPONENT_CONTEXT_ANGULAR_BINS;
        for value in &mut signature[start..end] {
            *value /= total;
        }
    }
    signature
}

fn extract_shape_components(grid: &ShapeMatchGrid, max_components: usize) -> Vec<ShapeComponent> {
    let mut visited = vec![false; grid.occupied_mask.len()];
    let support_frontier_mask = build_support_frontier_mask(grid);
    let mut components = Vec::new();

    // threshold the occupied mask to only deep thick blobs to sever simple noisy connections
    let mut deep_occupied = vec![false; grid.occupied_mask.len()];
    for i in 0..grid.occupied_mask.len() {
        deep_occupied[i] = grid.occupied_mask[i] && grid.distance_meters[i] > 0.50;
        // at least 50cm inside
    }

    for start in 0..deep_occupied.len() {
        if visited[start] || !deep_occupied[start] {
            continue;
        }
        let mut stack = vec![start];
        let mut component = Vec::new();
        let mut area_cells = 0usize;
        let mut perimeter_edges = 0usize;
        let mut sum_x = 0.0f32;
        let mut sum_z = 0.0f32;
        visited[start] = true;
        while let Some(index) = stack.pop() {
            component.push(index);
            area_cells += 1;
            let x = index % grid.size_x;
            let z = index / grid.size_x;
            let world_x = grid.origin_x + (x as f32 + 0.5) * grid.cell_size_meters;
            let world_z = grid.origin_z + (z as f32 + 0.5) * grid.cell_size_meters;
            sum_x += world_x;
            sum_z += world_z;
            for (dx, dz) in [(-1isize, 0isize), (1, 0), (0, -1), (0, 1)] {
                let nx = x as isize + dx;
                let nz = z as isize + dz;
                if nx < 0 || nz < 0 || nx >= grid.size_x as isize || nz >= grid.size_z as isize {
                    perimeter_edges += 1;
                    continue;
                }
                let neighbor = nx as usize + nz as usize * grid.size_x;
                if !deep_occupied[neighbor] {
                    perimeter_edges += 1;
                    continue;
                }
                if visited[neighbor] {
                    continue;
                }
                visited[neighbor] = true;
                stack.push(neighbor);
            }
        }
        if area_cells == 0 {
            continue;
        }
        let area_m2 = area_cells as f32 * grid.cell_size_meters * grid.cell_size_meters;
        let perimeter_m = perimeter_edges as f32 * grid.cell_size_meters;
        let compactness = if perimeter_m > 1.0e-6 {
            (4.0 * PI * area_m2 / (perimeter_m * perimeter_m)).clamp(0.0, 1.25)
        } else {
            0.0
        };
        let centroid_x = sum_x / area_cells as f32;
        let centroid_z = sum_z / area_cells as f32;
        let mut radial_sum = [0.0f32; SHAPE_COMPONENT_SIGNATURE_BINS];
        let mut radial_count = [0usize; SHAPE_COMPONENT_SIGNATURE_BINS];
        for &index in &component {
            let x = index % grid.size_x;
            let z = index / grid.size_x;
            let world_x = grid.origin_x + (x as f32 + 0.5) * grid.cell_size_meters;
            let world_z = grid.origin_z + (z as f32 + 0.5) * grid.cell_size_meters;
            let dx = world_x - centroid_x;
            let dz = world_z - centroid_z;
            let angle = wrap_angle(dz.atan2(dx));
            let sector =
                (((angle + PI) / (TAU / SHAPE_COMPONENT_SIGNATURE_BINS as f32)).floor() as isize)
                    .rem_euclid(SHAPE_COMPONENT_SIGNATURE_BINS as isize) as usize;
            radial_sum[sector] += (dx * dx + dz * dz).sqrt();
            radial_count[sector] += 1;
        }
        let mut radial_signature = [0.0f32; SHAPE_COMPONENT_SIGNATURE_BINS];
        for sector in 0..SHAPE_COMPONENT_SIGNATURE_BINS {
            radial_signature[sector] = if radial_count[sector] > 0 {
                radial_sum[sector] / radial_count[sector] as f32
            } else {
                0.0
            };
        }
        let radial_norm = radial_signature
            .iter()
            .copied()
            .fold(0.0f32, f32::max)
            .max(1.0e-4);
        for value in &mut radial_signature {
            *value /= radial_norm;
        }
        let area_score = area_m2.sqrt().min(1.5);
        let score = area_score * (0.25 + compactness * 0.75);
        let context_signature =
            build_component_context_signature(grid, &support_frontier_mask, centroid_x, centroid_z);
        components.push(ShapeComponent {
            centroid_x,
            centroid_z,
            area_m2,
            score,
            compactness,
            radial_signature,
            context_signature,
        });
    }
    components.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| right.area_m2.total_cmp(&left.area_m2))
    });
    components.truncate(max_components.max(1));
    components
}

fn build_shape_match_grid(
    height_map: &XrDepthAlignHeightMap,
    coarse_cell_size_meters: f32,
    band: &ShapeBandSpec,
) -> Option<ShapeMatchGrid> {
    let size_x = ((height_map.extent_x_meters() / coarse_cell_size_meters).ceil() as usize).max(1);
    let size_z = ((height_map.extent_z_meters() / coarse_cell_size_meters).ceil() as usize).max(1);
    let mut occupied = vec![false; size_x * size_z];
    let mut support_mask = vec![false; size_x * size_z];
    let mut free_mask = vec![true; size_x * size_z];
    let cutout_center = height_map.player_cutout_center;
    let cutout_radius = (height_map.player_cutout_radius_meters + height_map.cell_size_meters)
        .max(height_map.cell_size_meters * 2.0);
    let coarse_cell_size = coarse_cell_size_meters.max(height_map.cell_size_meters);
    let band_floor_y = height_map.floor_y_meters + band.floor_offset_meters;
    for src_z in 0..height_map.size_z_usize() {
        for src_x in 0..height_map.size_x_usize() {
            let index = height_map.cell_index(src_x, src_z);
            let height = *height_map.heights_meters.get(index)?;
            if !height.is_finite() {
                continue;
            }
            let world_x = height_map.origin_x + (src_x as f32 + 0.5) * height_map.cell_size_meters;
            let world_z = height_map.origin_z + (src_z as f32 + 0.5) * height_map.cell_size_meters;
            if cutout_center.is_some_and(|center| {
                let dx = world_x - center.x;
                let dz = world_z - center.y;
                (dx * dx + dz * dz).sqrt() <= cutout_radius
            }) {
                continue;
            }
            let coarse_x = ((world_x - height_map.origin_x) / coarse_cell_size)
                .floor()
                .clamp(0.0, size_x.saturating_sub(1) as f32) as usize;
            let coarse_z = ((world_z - height_map.origin_z) / coarse_cell_size)
                .floor()
                .clamp(0.0, size_z.saturating_sub(1) as f32) as usize;
            let coarse_index = coarse_x + coarse_z * size_x;
            support_mask[coarse_index] = true;
            let relative_height = height - band_floor_y;
            if relative_height >= SHAPE_OCCUPIED_MIN_HEIGHT_METERS {
                free_mask[coarse_index] = false;
            }
            if relative_height < band.min_height_meters {
                continue;
            }
            if band
                .max_height_meters
                .is_some_and(|max_height| relative_height > max_height)
            {
                continue;
            }
            occupied[coarse_index] = true;
        }
    }
    support_mask = filter_shape_components(&support_mask, size_x, size_z, coarse_cell_size, false);
    support_mask = clean_binary_mask(&support_mask, size_x, size_z);
    occupied = filter_shape_components(
        &occupied,
        size_x,
        size_z,
        coarse_cell_size,
        band.drop_border_components,
    );
    occupied = clean_binary_mask(&occupied, size_x, size_z);
    if occupied.iter().all(|value| !*value) {
        return None;
    }

    for i in 0..free_mask.len() {
        free_mask[i] = free_mask[i] && support_mask[i];
    }
    // DO NOT ERODE free_mask! Narrow door streaks vanished because of erosion!

    let mut contour_mask = vec![false; occupied.len()];
    for z in 0..size_z {
        for x in 0..size_x {
            let index = x + z * size_x;
            if !occupied[index] {
                continue;
            }
            let mut edge = false;
            for (dx, dz) in [(-1isize, 0isize), (1, 0), (0, -1), (0, 1)] {
                let nx = x as isize + dx;
                let nz = z as isize + dz;
                if nx < 0
                    || nz < 0
                    || nx >= size_x as isize
                    || nz >= size_z as isize
                    || !occupied[nx as usize + nz as usize * size_x]
                {
                    edge = true;
                    break;
                }
            }
            contour_mask[index] = edge;
        }
    }
    if contour_mask.iter().filter(|value| **value).count() < SHAPE_MIN_CONTOUR_POINTS {
        contour_mask.copy_from_slice(&occupied);
    }

    let distance_meters =
        chamfer_distance_transform(&contour_mask, size_x, size_z, coarse_cell_size);
    let signed_distance_meters = distance_meters
        .iter()
        .enumerate()
        .map(|(index, distance)| {
            if occupied[index] {
                *distance
            } else {
                -*distance
            }
        })
        .collect::<Vec<_>>();
    let occupied_weight = occupied
        .iter()
        .enumerate()
        .map(|(index, is_occupied)| {
            if !*is_occupied {
                return 0.0;
            }
            let interior_bonus =
                (distance_meters[index] / SHAPE_SEEDED_INTERIOR_BONUS_CAP_METERS).clamp(0.0, 1.0);
            1.0 + interior_bonus * 0.65
        })
        .collect::<Vec<_>>();
    let mut contour_points = Vec::new();
    let mut occupied_points = Vec::new();
    for z in 0..size_z {
        for x in 0..size_x {
            if !occupied[x + z * size_x] {
                continue;
            }
            occupied_points.push((
                height_map.origin_x + (x as f32 + 0.5) * coarse_cell_size,
                height_map.origin_z + (z as f32 + 0.5) * coarse_cell_size,
            ));
        }
    }
    for z in 0..size_z {
        for x in 0..size_x {
            if !contour_mask[x + z * size_x] {
                continue;
            }
            contour_points.push((
                height_map.origin_x + (x as f32 + 0.5) * coarse_cell_size,
                height_map.origin_z + (z as f32 + 0.5) * coarse_cell_size,
            ));
        }
    }
    let mut support_points = Vec::new();
    for z in 0..size_z {
        for x in 0..size_x {
            if !support_mask[x + z * size_x] {
                continue;
            }
            support_points.push((
                height_map.origin_x + (x as f32 + 0.5) * coarse_cell_size,
                height_map.origin_z + (z as f32 + 0.5) * coarse_cell_size,
            ));
        }
    }
    if contour_points.len() < SHAPE_MIN_CONTOUR_POINTS {
        return None;
    }
    Some(ShapeMatchGrid {
        origin_x: height_map.origin_x,
        origin_z: height_map.origin_z,
        cell_size_meters: coarse_cell_size,
        size_x,
        size_z,
        occupied_mask: occupied,
        occupied_weight,
        occupied_points,
        distance_meters,
        signed_distance_meters,
        contour_points,
        support_mask,
        support_points,
        free_mask,
    })
}

fn chamfer_distance_transform(
    contour_mask: &[bool],
    size_x: usize,
    size_z: usize,
    cell_size_meters: f32,
) -> Vec<f32> {
    let len = size_x * size_z;
    let inf = 1.0e6f32;
    let diag = 2.0f32.sqrt();
    let mut distance = vec![inf; len];
    for (index, occupied) in contour_mask.iter().enumerate() {
        if *occupied {
            distance[index] = 0.0;
        }
    }
    for z in 0..size_z {
        for x in 0..size_x {
            let index = x + z * size_x;
            let mut best = distance[index];
            if x > 0 {
                best = best.min(distance[index - 1] + 1.0);
            }
            if z > 0 {
                best = best.min(distance[index - size_x] + 1.0);
                if x > 0 {
                    best = best.min(distance[index - size_x - 1] + diag);
                }
                if x + 1 < size_x {
                    best = best.min(distance[index - size_x + 1] + diag);
                }
            }
            distance[index] = best;
        }
    }
    for z in (0..size_z).rev() {
        for x in (0..size_x).rev() {
            let index = x + z * size_x;
            let mut best = distance[index];
            if x + 1 < size_x {
                best = best.min(distance[index + 1] + 1.0);
            }
            if z + 1 < size_z {
                best = best.min(distance[index + size_x] + 1.0);
                if x > 0 {
                    best = best.min(distance[index + size_x - 1] + diag);
                }
                if x + 1 < size_x {
                    best = best.min(distance[index + size_x + 1] + diag);
                }
            }
            distance[index] = best;
        }
    }
    for value in &mut distance {
        *value *= cell_size_meters;
    }
    distance
}

fn candidate_is_distinct(
    left: ShapeMatchCandidate,
    right: ShapeMatchCandidate,
    translation_epsilon: f32,
    yaw_epsilon: f32,
) -> bool {
    (left.translation_x - right.translation_x).abs() > translation_epsilon
        || (left.translation_z - right.translation_z).abs() > translation_epsilon
        || wrap_angle(left.yaw_radians - right.yaw_radians).abs() > yaw_epsilon
}

fn push_shape_candidate(
    finalists: &mut Vec<ShapeMatchCandidate>,
    candidate: ShapeMatchCandidate,
    limit: usize,
    translation_epsilon: f32,
    yaw_epsilon: f32,
) {
    if !candidate.score.is_finite() || candidate.score <= 0.0 {
        return;
    }
    if let Some(existing) = finalists.iter_mut().find(|existing| {
        !candidate_is_distinct(**existing, candidate, translation_epsilon, yaw_epsilon)
    }) {
        if candidate.score > existing.score {
            *existing = candidate;
        }
    } else {
        finalists.push(candidate);
    }
    finalists.sort_by(|left, right| {
        right.score.total_cmp(&left.score).then_with(|| {
            left.mean_distance_meters
                .total_cmp(&right.mean_distance_meters)
        })
    });
    finalists.truncate(limit.max(1));
}

fn score_shape_alignment_one_way(
    local: &ShapeMatchGrid,
    remote_points: &[(f32, f32)],
    yaw_radians: f32,
    translation_x: f32,
    translation_z: f32,
) -> ShapeMatchCandidate {
    if remote_points.is_empty() {
        return ShapeMatchCandidate::default();
    }
    let mut in_bounds_points = 0usize;
    let mut close_points = 0usize;
    let mut similarity_sum = 0.0;
    let mut distance_sum = 0.0;
    let mut free_spill = 0usize;
    for &(remote_x, remote_z) in remote_points {
        let (rotated_x, rotated_z) = rotate_xz(yaw_radians, remote_x, remote_z);
        let world_x = rotated_x + translation_x;
        let world_z = rotated_z + translation_z;
        let Some(distance) = local.sample_distance_nearest(world_x, world_z) else {
            continue;
        };
        in_bounds_points += 1;
        if local.sample_free_nearest(world_x, world_z).unwrap_or(false) {
            free_spill += 1;
            similarity_sum -= 5.0;
            distance_sum += SHAPE_MATCH_DISTANCE_TRUNCATE_METERS;
            continue;
        }
        let clamped_distance = distance.min(SHAPE_MATCH_DISTANCE_TRUNCATE_METERS);
        similarity_sum += 1.0 - clamped_distance / SHAPE_MATCH_DISTANCE_TRUNCATE_METERS;
        distance_sum += distance;

        if distance <= (local.cell_size_meters * 1.5 + 0.06) {
            close_points += 1;
        }
    }

    if free_spill as f32 / in_bounds_points.max(1) as f32 > 0.05 {
        return ShapeMatchCandidate::default();
    }

    if in_bounds_points < SHAPE_MIN_CONTOUR_POINTS.min(remote_points.len()) {
        return ShapeMatchCandidate::default();
    }
    let coverage = in_bounds_points as f32 / remote_points.len().max(1) as f32;
    let close_ratio = close_points as f32 / remote_points.len().max(1) as f32;
    let mean_similarity = similarity_sum / in_bounds_points as f32;
    ShapeMatchCandidate {
        score: mean_similarity * coverage.sqrt() * (0.20 + 0.80 * close_ratio.sqrt()),
        feature_score: mean_similarity * coverage.sqrt() * (0.20 + 0.80 * close_ratio.sqrt()),
        support_score: 0.0,
        coverage,
        close_ratio,
        mean_distance_meters: distance_sum / in_bounds_points as f32,
        yaw_radians,
        translation_x,
        translation_z,
        in_bounds_points,
    }
}

fn score_support_iou_symmetric(
    local: &ShapeMatchGrid,
    remote: &ShapeMatchGrid,
    yaw_radians: f32,
    translation_x: f32,
    translation_z: f32,
) -> f32 {
    let mut intersection = 0usize;
    let mut remote_area = 0usize;

    // We iterate over remote support points
    for &(remote_x, remote_z) in &remote.support_points {
        remote_area += 1;
        let (rotated_x, rotated_z) = rotate_xz(yaw_radians, remote_x, remote_z);
        let world_x = rotated_x + translation_x;
        let world_z = rotated_z + translation_z;
        if local
            .sample_support_nearest(world_x, world_z)
            .unwrap_or(false)
        {
            intersection += 1;
        }
    }

    let local_area = local.support_points.len();
    if local_area == 0 || remote_area == 0 || intersection == 0 {
        return 0.0;
    }

    let union = local_area + remote_area - intersection;
    intersection as f32 / union.max(1) as f32
}

fn score_support_overlap_one_way(
    local: &ShapeMatchGrid,
    remote_points: &[(f32, f32)],
    yaw_radians: f32,
    translation_x: f32,
    translation_z: f32,
) -> (f32, f32, f32, usize) {
    if remote_points.is_empty() {
        return (0.0, 0.0, 0.0, 0);
    }
    let mut in_bounds = 0usize;
    let mut hits = 0usize;
    for &(remote_x, remote_z) in remote_points {
        let (rotated_x, rotated_z) = rotate_xz(yaw_radians, remote_x, remote_z);
        let world_x = rotated_x + translation_x;
        let world_z = rotated_z + translation_z;
        let Some(is_supported) = local.sample_support_nearest(world_x, world_z) else {
            continue;
        };
        in_bounds += 1;
        if is_supported {
            hits += 1;
        }
    }
    if in_bounds < SHAPE_MIN_CONTOUR_POINTS.min(remote_points.len()) {
        return (0.0, 0.0, 0.0, in_bounds);
    }
    let coverage = in_bounds as f32 / remote_points.len().max(1) as f32;
    let hit_ratio = hits as f32 / in_bounds.max(1) as f32;
    (coverage.sqrt() * hit_ratio, coverage, hit_ratio, in_bounds)
}

fn score_combined_alignment_one_way(
    local: &ShapeMatchGrid,
    remote: &ShapeMatchGrid,
    yaw_radians: f32,
    translation_x: f32,
    translation_z: f32,
) -> ShapeMatchCandidate {
    let feature = score_shape_alignment_one_way(
        local,
        &remote.contour_points,
        yaw_radians,
        translation_x,
        translation_z,
    );
    let (support_score, support_coverage, support_hit_ratio, support_points) =
        score_support_overlap_one_way(
            local,
            &remote.support_points,
            yaw_radians,
            translation_x,
            translation_z,
        );
    if feature.score <= 0.0 && support_score <= 0.0 {
        return ShapeMatchCandidate::default();
    }
    ShapeMatchCandidate {
        score: feature.score * 0.72 + support_score * 0.28,
        feature_score: feature.score,
        support_score,
        coverage: feature.coverage * 0.7 + support_coverage * 0.3,
        close_ratio: feature.close_ratio * 0.65 + support_hit_ratio * 0.35,
        mean_distance_meters: feature.mean_distance_meters,
        yaw_radians,
        translation_x,
        translation_z,
        in_bounds_points: feature
            .in_bounds_points
            .min(support_points.max(feature.in_bounds_points)),
    }
}

fn score_shape_alignment_symmetric(
    local: &ShapeMatchGrid,
    remote: &ShapeMatchGrid,
    yaw_radians: f32,
    translation_x: f32,
    translation_z: f32,
) -> ShapeMatchCandidate {
    let forward =
        score_combined_alignment_one_way(local, remote, yaw_radians, translation_x, translation_z);
    if forward.score <= 0.0 {
        return forward;
    }
    let inverse = score_combined_alignment_one_way(
        remote,
        local,
        -yaw_radians,
        -translation_x,
        -translation_z,
    );
    if inverse.score <= 0.0 {
        return forward;
    }
    let feature_score = (forward.feature_score * inverse.feature_score).sqrt();
    let support_score = (forward.support_score * inverse.support_score).sqrt();
    ShapeMatchCandidate {
        score: feature_score * 0.72 + support_score * 0.28,
        feature_score,
        support_score,
        coverage: 0.5 * (forward.coverage + inverse.coverage),
        close_ratio: 0.5 * (forward.close_ratio + inverse.close_ratio),
        mean_distance_meters: 0.5 * (forward.mean_distance_meters + inverse.mean_distance_meters),
        yaw_radians,
        translation_x,
        translation_z,
        in_bounds_points: forward.in_bounds_points.min(inverse.in_bounds_points),
    }
}

fn dominant_hough_orientations(grid: &ShapeMatchGrid) -> Vec<f32> {
    if grid.contour_points.is_empty() {
        return Vec::new();
    }
    let center_x = grid.origin_x + grid.size_x as f32 * grid.cell_size_meters * 0.5;
    let center_z = grid.origin_z + grid.size_z as f32 * grid.cell_size_meters * 0.5;
    let half_extent_x = grid.size_x as f32 * grid.cell_size_meters * 0.5;
    let half_extent_z = grid.size_z as f32 * grid.cell_size_meters * 0.5;
    let max_rho = (half_extent_x * half_extent_x + half_extent_z * half_extent_z)
        .sqrt()
        .max(grid.cell_size_meters);
    let rho_step = (grid.cell_size_meters * 1.5).max(0.06);
    let rho_bins = ((2.0 * max_rho) / rho_step).ceil().max(1.0) as usize + 1;
    let mut accum = vec![0.0f32; SHAPE_HOUGH_THETA_BINS * rho_bins];
    let theta_step = PI / SHAPE_HOUGH_THETA_BINS as f32;

    for &(world_x, world_z) in &grid.contour_points {
        let x = world_x - center_x;
        let z = world_z - center_z;
        for theta_index in 0..SHAPE_HOUGH_THETA_BINS {
            let theta = theta_index as f32 * theta_step;
            let (sin_theta, cos_theta) = theta.sin_cos();
            let rho = x * cos_theta + z * sin_theta;
            let rho_index = ((rho + max_rho) / rho_step)
                .round()
                .clamp(0.0, rho_bins.saturating_sub(1) as f32) as usize;
            accum[theta_index * rho_bins + rho_index] += 1.0;
        }
    }

    let mut theta_scores = vec![0.0f32; SHAPE_HOUGH_THETA_BINS];
    for theta_index in 0..SHAPE_HOUGH_THETA_BINS {
        let row = &accum[theta_index * rho_bins..(theta_index + 1) * rho_bins];
        let mut best = 0.0f32;
        let mut second = 0.0f32;
        for &value in row {
            if value > best {
                second = best;
                best = value;
            } else if value > second {
                second = value;
            }
        }
        theta_scores[theta_index] = best * 0.8 + second * 0.2;
    }

    let mut peaks = Vec::<(f32, f32)>::new();
    for theta_index in 0..SHAPE_HOUGH_THETA_BINS {
        let theta = theta_index as f32 * theta_step;
        let score = theta_scores[theta_index];
        if score <= 0.0 {
            continue;
        }
        let mut suppressed = false;
        for &(existing_theta, existing_score) in &peaks {
            let delta = wrap_angle(theta - existing_theta)
                .abs()
                .min(wrap_angle(theta - existing_theta + PI).abs());
            if delta < SHAPE_HOUGH_THETA_SUPPRESS_RADIANS && existing_score >= score {
                suppressed = true;
                break;
            }
        }
        if suppressed {
            continue;
        }
        peaks.retain(|(existing_theta, existing_score)| {
            let delta = wrap_angle(theta - *existing_theta)
                .abs()
                .min(wrap_angle(theta - *existing_theta + PI).abs());
            !(delta < SHAPE_HOUGH_THETA_SUPPRESS_RADIANS && score > *existing_score)
        });
        peaks.push((theta, score));
        peaks.sort_by(|left, right| right.1.total_cmp(&left.1));
        peaks.truncate(SHAPE_HOUGH_TOP_PEAKS.max(1));
    }
    peaks
        .into_iter()
        .map(|(theta, _)| wrap_angle(theta + PI * 0.5))
        .collect()
}

fn extract_hough_lines(grid: &ShapeMatchGrid, max_lines: usize) -> Vec<HoughLine> {
    if grid.contour_points.is_empty() || max_lines == 0 {
        return Vec::new();
    }
    let max_rho = grid
        .contour_points
        .iter()
        .map(|(x, z)| (x * x + z * z).sqrt())
        .fold(0.0f32, f32::max)
        .max(grid.cell_size_meters);
    let rho_step = (grid.cell_size_meters * 1.25).max(0.05);
    let rho_bins = ((2.0 * max_rho) / rho_step).ceil().max(1.0) as usize + 1;
    let theta_step = PI / SHAPE_HOUGH_THETA_BINS as f32;
    let mut accum = vec![0.0f32; SHAPE_HOUGH_THETA_BINS * rho_bins];

    for &(world_x, world_z) in &grid.contour_points {
        for theta_index in 0..SHAPE_HOUGH_THETA_BINS {
            let theta = theta_index as f32 * theta_step;
            let (sin_theta, cos_theta) = theta.sin_cos();
            let rho = world_x * cos_theta + world_z * sin_theta;
            let rho_index = ((rho + max_rho) / rho_step)
                .round()
                .clamp(0.0, rho_bins.saturating_sub(1) as f32) as usize;
            accum[theta_index * rho_bins + rho_index] += 1.0;
        }
    }

    let best_score = accum.iter().copied().fold(0.0f32, f32::max);
    if best_score <= 0.0 {
        return Vec::new();
    }
    let min_score = (best_score * 0.18).max(6.0);
    let mut peaks = Vec::<HoughLine>::new();
    for theta_index in 0..SHAPE_HOUGH_THETA_BINS {
        let theta = theta_index as f32 * theta_step;
        for rho_index in 0..rho_bins {
            let score = accum[theta_index * rho_bins + rho_index];
            if score < min_score {
                continue;
            }
            let rho = rho_index as f32 * rho_step - max_rho;
            let mut suppressed = false;
            for existing in &peaks {
                if line_angle_delta(existing.theta_radians, theta)
                    < SHAPE_HOUGH_THETA_SUPPRESS_RADIANS
                    && (existing.rho_meters - rho).abs() < FOCUSED_HOUGH_RHO_SUPPRESS_METERS
                    && existing.strength >= score
                {
                    suppressed = true;
                    break;
                }
            }
            if suppressed {
                continue;
            }
            peaks.retain(|existing| {
                !(line_angle_delta(existing.theta_radians, theta)
                    < SHAPE_HOUGH_THETA_SUPPRESS_RADIANS
                    && (existing.rho_meters - rho).abs() < FOCUSED_HOUGH_RHO_SUPPRESS_METERS
                    && score > existing.strength)
            });
            peaks.push(HoughLine {
                theta_radians: theta,
                rho_meters: rho,
                strength: score,
            });
            peaks.sort_by(|left, right| right.strength.total_cmp(&left.strength));
            peaks.truncate(max_lines.max(1));
        }
    }
    let norm = peaks
        .first()
        .map(|line| line.strength.max(1.0e-4))
        .unwrap_or(1.0);
    for line in &mut peaks {
        line.theta_radians = line_theta(line.theta_radians);
        line.strength = (line.strength / norm).clamp(0.0, 1.0);
    }
    peaks
}

fn transform_hough_line(
    line: HoughLine,
    yaw_radians: f32,
    translation_x: f32,
    translation_z: f32,
) -> HoughLine {
    let raw_theta = line.theta_radians + yaw_radians;
    let (sin_theta, cos_theta) = raw_theta.sin_cos();
    let mut theta_radians = raw_theta.rem_euclid(TAU);
    let mut rho_meters = line.rho_meters + translation_x * cos_theta + translation_z * sin_theta;
    if theta_radians >= PI {
        theta_radians -= PI;
        rho_meters = -rho_meters;
    }
    HoughLine {
        theta_radians,
        rho_meters,
        strength: line.strength,
    }
}

fn score_hough_line_alignment_one_way(
    local_lines: &[HoughLine],
    remote_lines: &[HoughLine],
    yaw_radians: f32,
    translation_x: f32,
    translation_z: f32,
) -> f32 {
    if local_lines.is_empty() || remote_lines.is_empty() {
        return 0.0;
    }
    let total_weight = remote_lines
        .iter()
        .map(|line| line.strength.max(0.05))
        .sum::<f32>()
        .max(1.0e-6);
    let mut score_sum = 0.0f32;
    let mut matched_weight = 0.0f32;
    for &remote_line in remote_lines {
        let transformed =
            transform_hough_line(remote_line, yaw_radians, translation_x, translation_z);
        let mut best = 0.0f32;
        for &local_line in local_lines {
            let angle_delta = line_angle_delta(local_line.theta_radians, transformed.theta_radians);
            if angle_delta > 16.0_f32.to_radians() {
                continue;
            }
            let rho_delta = (local_line.rho_meters - transformed.rho_meters).abs();
            if rho_delta > 0.80 {
                continue;
            }
            let angle_score =
                (-0.5 * (angle_delta / FOCUSED_HOUGH_ANGLE_SIGMA_RADIANS).powi(2)).exp();
            let rho_score = (-0.5 * (rho_delta / FOCUSED_HOUGH_RHO_SIGMA_METERS).powi(2)).exp();
            best = best.max(angle_score * rho_score * local_line.strength.sqrt());
        }
        let weight = remote_line.strength.max(0.05);
        score_sum += best * weight;
        matched_weight += best.sqrt() * weight;
    }
    let coverage = (matched_weight / total_weight).clamp(0.0, 1.0);
    (score_sum / total_weight) * coverage.sqrt()
}

fn score_hough_line_alignment_symmetric(
    local_lines: &[HoughLine],
    remote_lines: &[HoughLine],
    yaw_radians: f32,
    translation_x: f32,
    translation_z: f32,
) -> f32 {
    let forward = score_hough_line_alignment_one_way(
        local_lines,
        remote_lines,
        yaw_radians,
        translation_x,
        translation_z,
    );
    if forward <= 0.0 {
        return 0.0;
    }
    let inverse = score_hough_line_alignment_one_way(
        remote_lines,
        local_lines,
        -yaw_radians,
        -translation_x,
        -translation_z,
    );
    if inverse <= 0.0 {
        return 0.0;
    }
    (forward * inverse).sqrt()
}

fn projection_histogram_overlap(
    local_points: &[(f32, f32)],
    remote_points: &[(f32, f32)],
    axis_x: f32,
    axis_z: f32,
    bin_size_meters: f32,
) -> f32 {
    if local_points.is_empty() || remote_points.is_empty() {
        return 0.0;
    }
    let mut min_projection = f32::INFINITY;
    let mut max_projection = f32::NEG_INFINITY;
    for &(x, z) in local_points.iter().chain(remote_points.iter()) {
        let projection = x * axis_x + z * axis_z;
        min_projection = min_projection.min(projection);
        max_projection = max_projection.max(projection);
    }
    if !min_projection.is_finite() || !max_projection.is_finite() {
        return 0.0;
    }
    let bin_size_meters = bin_size_meters.max(0.04);
    let bin_count =
        (((max_projection - min_projection) / bin_size_meters).ceil() as usize).max(1) + 1;
    let mut local_hist = vec![0.0f32; bin_count];
    let mut remote_hist = vec![0.0f32; bin_count];
    for &(x, z) in local_points {
        let projection = x * axis_x + z * axis_z;
        let index = ((projection - min_projection) / bin_size_meters)
            .floor()
            .clamp(0.0, bin_count.saturating_sub(1) as f32) as usize;
        local_hist[index] += 1.0;
    }
    for &(x, z) in remote_points {
        let projection = x * axis_x + z * axis_z;
        let index = ((projection - min_projection) / bin_size_meters)
            .floor()
            .clamp(0.0, bin_count.saturating_sub(1) as f32) as usize;
        remote_hist[index] += 1.0;
    }
    let mut intersection = 0.0f32;
    let mut union = 0.0f32;
    for index in 0..bin_count {
        intersection += local_hist[index].min(remote_hist[index]);
        union += local_hist[index].max(remote_hist[index]);
    }
    intersection / union.max(1.0e-6)
}

fn score_wall_profile_alignment(
    local_points: &[(f32, f32)],
    remote_points: &[(f32, f32)],
    yaw_radians: f32,
    translation_x: f32,
    translation_z: f32,
    bin_size_meters: f32,
) -> f32 {
    if local_points.len() < SHAPE_MIN_CONTOUR_POINTS
        || remote_points.len() < SHAPE_MIN_CONTOUR_POINTS
    {
        return 0.0;
    }
    let transformed_remote = remote_points
        .iter()
        .map(|&(x, z)| {
            let (rotated_x, rotated_z) = rotate_xz(yaw_radians, x, z);
            (rotated_x + translation_x, rotated_z + translation_z)
        })
        .collect::<Vec<_>>();
    let (axis_z, axis_x) = yaw_radians.sin_cos();
    let score_parallel = projection_histogram_overlap(
        local_points,
        &transformed_remote,
        axis_x,
        axis_z,
        bin_size_meters,
    );
    let score_perp = projection_histogram_overlap(
        local_points,
        &transformed_remote,
        -axis_z,
        axis_x,
        bin_size_meters,
    );
    (score_parallel * score_perp).sqrt()
}

fn hough_yaw_candidates(local: &ShapeMatchGrid, remote: &ShapeMatchGrid) -> Vec<f32> {
    let local_orientations = dominant_hough_orientations(local);
    let remote_orientations = dominant_hough_orientations(remote);
    let mut yaws = Vec::<ShapeMatchCandidate>::new();
    for &local_theta in &local_orientations {
        for &remote_theta in &remote_orientations {
            push_shape_candidate(
                &mut yaws,
                ShapeMatchCandidate {
                    yaw_radians: wrap_angle(local_theta - remote_theta),
                    score: 1.0,
                    ..ShapeMatchCandidate::default()
                },
                12,
                0.01,
                4.0_f32.to_radians(),
            );
        }
    }
    yaws.into_iter()
        .map(|candidate| candidate.yaw_radians)
        .collect()
}

fn rotated_bounds(points: &[(f32, f32)], yaw_radians: f32) -> Option<(f32, f32, f32, f32)> {
    let mut min_x = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut min_z = f32::INFINITY;
    let mut max_z = f32::NEG_INFINITY;
    for &(x, z) in points {
        let (rotated_x, rotated_z) = rotate_xz(yaw_radians, x, z);
        min_x = min_x.min(rotated_x);
        max_x = max_x.max(rotated_x);
        min_z = min_z.min(rotated_z);
        max_z = max_z.max(rotated_z);
    }
    (min_x.is_finite() && max_x.is_finite() && min_z.is_finite() && max_z.is_finite())
        .then_some((min_x, max_x, min_z, max_z))
}

fn search_shape_global(
    local: &ShapeMatchGrid,
    remote: &ShapeMatchGrid,
    top_k: usize,
) -> Vec<ShapeMatchCandidate> {
    let local_min_x = local.origin_x;
    let local_max_x = local.max_x();
    let local_min_z = local.origin_z;
    let local_max_z = local.max_z();
    let step = local.cell_size_meters.max(1.0e-3);
    let translation_tolerance = step * 1.5;
    let yaw_tolerance = SHAPE_GLOBAL_YAW_STEP_RADIANS * 1.25;
    let local_top_k = top_k.saturating_mul(4).max(top_k);
    let mut yaws = Vec::new();
    let mut yaw = -PI;
    while yaw < PI {
        yaws.push(yaw);
        yaw += SHAPE_GLOBAL_YAW_STEP_RADIANS;
    }
    let worker_count = analysis_thread_count(yaws.len());
    let chunk_size = yaws.len().div_ceil(worker_count);
    let partials = std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for yaw_chunk in yaws.chunks(chunk_size.max(1)) {
            let yaw_chunk = yaw_chunk.to_vec();
            handles.push(scope.spawn(move || {
                let mut finalists = Vec::new();
                for yaw in yaw_chunk {
                    let Some((remote_min_x, remote_max_x, remote_min_z, remote_max_z)) =
                        rotated_bounds(&remote.contour_points, yaw)
                    else {
                        continue;
                    };
                    let translation_min_x = local_min_x - remote_max_x;
                    let translation_max_x = local_max_x - remote_min_x;
                    let translation_min_z = local_min_z - remote_max_z;
                    let translation_max_z = local_max_z - remote_min_z;
                    let steps_x = ((translation_max_x - translation_min_x) / step)
                        .ceil()
                        .max(0.0) as usize;
                    let steps_z = ((translation_max_z - translation_min_z) / step)
                        .ceil()
                        .max(0.0) as usize;
                    for x_step in 0..=steps_x {
                        let translation_x = translation_min_x + x_step as f32 * step;
                        for z_step in 0..=steps_z {
                            let translation_z = translation_min_z + z_step as f32 * step;
                            let candidate = score_combined_alignment_one_way(
                                local,
                                remote,
                                yaw,
                                translation_x,
                                translation_z,
                            );
                            push_shape_candidate(
                                &mut finalists,
                                candidate,
                                local_top_k,
                                translation_tolerance,
                                yaw_tolerance,
                            );
                        }
                    }
                }
                finalists
            }));
        }
        let mut partials = Vec::new();
        for handle in handles {
            partials.push(handle.join().expect("shape-global worker should not panic"));
        }
        partials
    });
    let mut finalists = Vec::new();
    for partial in partials {
        for candidate in partial {
            push_shape_candidate(
                &mut finalists,
                candidate,
                top_k,
                translation_tolerance,
                yaw_tolerance,
            );
        }
    }
    finalists
}

fn search_shape_global_yaws(
    local: &ShapeMatchGrid,
    remote: &ShapeMatchGrid,
    yaw_candidates: &[f32],
    top_k: usize,
) -> Vec<ShapeMatchCandidate> {
    let local_min_x = local.origin_x;
    let local_max_x = local.max_x();
    let local_min_z = local.origin_z;
    let local_max_z = local.max_z();
    let step = local.cell_size_meters.max(1.0e-3);
    let translation_tolerance = step * 1.5;
    let yaw_tolerance = SHAPE_GLOBAL_YAW_STEP_RADIANS * 1.25;
    let local_top_k = top_k.saturating_mul(4).max(top_k);
    let worker_count = analysis_thread_count(yaw_candidates.len());
    let chunk_size = yaw_candidates.len().max(1).div_ceil(worker_count);
    let partials = std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for yaw_chunk in yaw_candidates.chunks(chunk_size.max(1)) {
            let yaw_chunk = yaw_chunk.to_vec();
            handles.push(scope.spawn(move || {
                let mut finalists = Vec::new();
                for yaw in yaw_chunk {
                    let Some((remote_min_x, remote_max_x, remote_min_z, remote_max_z)) =
                        rotated_bounds(&remote.contour_points, yaw)
                    else {
                        continue;
                    };
                    let translation_min_x = local_min_x - remote_max_x;
                    let translation_max_x = local_max_x - remote_min_x;
                    let translation_min_z = local_min_z - remote_max_z;
                    let translation_max_z = local_max_z - remote_min_z;
                    let steps_x = ((translation_max_x - translation_min_x) / step)
                        .ceil()
                        .max(0.0) as usize;
                    let steps_z = ((translation_max_z - translation_min_z) / step)
                        .ceil()
                        .max(0.0) as usize;
                    for x_step in 0..=steps_x {
                        let translation_x = translation_min_x + x_step as f32 * step;
                        for z_step in 0..=steps_z {
                            let translation_z = translation_min_z + z_step as f32 * step;
                            let candidate = score_combined_alignment_one_way(
                                local,
                                remote,
                                yaw,
                                translation_x,
                                translation_z,
                            );
                            push_shape_candidate(
                                &mut finalists,
                                candidate,
                                local_top_k,
                                translation_tolerance,
                                yaw_tolerance,
                            );
                        }
                    }
                }
                finalists
            }));
        }
        let mut partials = Vec::new();
        for handle in handles {
            partials.push(
                handle
                    .join()
                    .expect("shape-global-yaw worker should not panic"),
            );
        }
        partials
    });
    let mut finalists = Vec::new();
    for partial in partials {
        for candidate in partial {
            push_shape_candidate(
                &mut finalists,
                candidate,
                top_k,
                translation_tolerance,
                yaw_tolerance,
            );
        }
    }
    finalists
}

fn search_shape_refine(
    local: &ShapeMatchGrid,
    remote: &ShapeMatchGrid,
    seeds: &[ShapeMatchCandidate],
    translation_window_meters: f32,
    yaw_window_radians: f32,
    yaw_step_radians: f32,
    top_k: usize,
    symmetric: bool,
) -> Vec<ShapeMatchCandidate> {
    let step = local.cell_size_meters.max(1.0e-3);
    let translation_steps = (translation_window_meters / step).ceil().max(1.0) as isize;
    let yaw_steps = (yaw_window_radians / yaw_step_radians).ceil().max(1.0) as isize;
    let translation_tolerance = step * 1.25;
    let yaw_tolerance = yaw_step_radians * 1.25;
    let local_top_k = top_k.saturating_mul(4).max(top_k);
    let worker_count = analysis_thread_count(seeds.len());
    let chunk_size = seeds.len().max(1).div_ceil(worker_count);
    let partials = std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for seed_chunk in seeds.chunks(chunk_size.max(1)) {
            let seed_chunk = seed_chunk.to_vec();
            handles.push(scope.spawn(move || {
                let mut finalists = Vec::new();
                for seed in seed_chunk {
                    for yaw_step in -yaw_steps..=yaw_steps {
                        let yaw_radians =
                            wrap_angle(seed.yaw_radians + yaw_step as f32 * yaw_step_radians);
                        for x_step in -translation_steps..=translation_steps {
                            let translation_x = seed.translation_x + x_step as f32 * step;
                            for z_step in -translation_steps..=translation_steps {
                                let translation_z = seed.translation_z + z_step as f32 * step;
                                let candidate = if symmetric {
                                    score_shape_alignment_symmetric(
                                        local,
                                        remote,
                                        yaw_radians,
                                        translation_x,
                                        translation_z,
                                    )
                                } else {
                                    score_combined_alignment_one_way(
                                        local,
                                        remote,
                                        yaw_radians,
                                        translation_x,
                                        translation_z,
                                    )
                                };
                                push_shape_candidate(
                                    &mut finalists,
                                    candidate,
                                    local_top_k,
                                    translation_tolerance,
                                    yaw_tolerance,
                                );
                            }
                        }
                    }
                }
                finalists
            }));
        }
        let mut partials = Vec::new();
        for handle in handles {
            partials.push(handle.join().expect("shape-refine worker should not panic"));
        }
        partials
    });
    let mut finalists = Vec::new();
    for partial in partials {
        for candidate in partial {
            push_shape_candidate(
                &mut finalists,
                candidate,
                top_k,
                translation_tolerance,
                yaw_tolerance,
            );
        }
    }
    finalists
}

fn manual_shape_candidate(
    local: &ShapeMatchGrid,
    remote: &ShapeMatchGrid,
    manual_pose: ManualPose,
) -> ShapeMatchCandidate {
    score_shape_alignment_symmetric(
        local,
        remote,
        manual_pose.rotation_radians,
        manual_pose.shift_x_meters,
        manual_pose.shift_y_meters,
    )
}

fn score_mask_overlap_one_way(
    local: &ShapeMatchGrid,
    remote_points: &[(f32, f32)],
    yaw_radians: f32,
    translation_x: f32,
    translation_z: f32,
) -> ShapeMatchCandidate {
    if remote_points.is_empty() {
        return ShapeMatchCandidate::default();
    }
    let mut in_bounds = 0usize;
    let mut hits = 0f32;
    let mut spill = 0f32;
    let mut unsupported = 0usize;
    let mut free_spill = 0usize;
    for &(remote_x, remote_z) in remote_points {
        let (rotated_x, rotated_z) = rotate_xz(yaw_radians, remote_x, remote_z);
        let world_x = rotated_x + translation_x;
        let world_z = rotated_z + translation_z;
        let Some(in_support) = local.sample_support_nearest(world_x, world_z) else {
            continue;
        };
        in_bounds += 1;
        if !in_support {
            unsupported += 1;
            continue;
        }
        if local.sample_free_nearest(world_x, world_z).unwrap_or(false) {
            free_spill += 1;
            spill += 8.0;
            continue;
        }
        if let Some(dist) = local.sample_distance_nearest(world_x, world_z) {
            if dist < 0.15 {
                hits += 1.0;
            } else {
                spill += 1.0;
            }
        } else {
            spill += 1.0;
        }
    }

    if free_spill as f32 / in_bounds.max(1) as f32 > 0.05 {
        return ShapeMatchCandidate::default();
    }

    if in_bounds < SHAPE_MIN_CONTOUR_POINTS.min(remote_points.len()) {
        return ShapeMatchCandidate::default();
    }

    let coverage = in_bounds as f32 / remote_points.len().max(1) as f32;
    let false_positives = spill + (unsupported as f32 * SHAPE_SEEDED_UNSUPPORTED_PENALTY);
    let precision = hits / (hits + false_positives).max(1.0);
    let score = coverage.sqrt() * precision;

    ShapeMatchCandidate {
        score,
        feature_score: score,
        support_score: 0.0,
        coverage,
        close_ratio: precision,
        mean_distance_meters: 1.0 - precision,
        yaw_radians,
        translation_x,
        translation_z,
        in_bounds_points: in_bounds,
    }
}

fn score_mask_overlap_symmetric(
    local: &ShapeMatchGrid,
    remote: &ShapeMatchGrid,
    yaw_radians: f32,
    translation_x: f32,
    translation_z: f32,
) -> ShapeMatchCandidate {
    let forward = score_mask_overlap_one_way(
        local,
        &remote.occupied_points,
        yaw_radians,
        translation_x,
        translation_z,
    );
    if forward.score <= 0.0 {
        return forward;
    }
    let inverse = score_mask_overlap_one_way(
        remote,
        &local.occupied_points,
        -yaw_radians,
        -translation_x,
        -translation_z,
    );
    if inverse.score <= 0.0 {
        return forward;
    }
    let score = (forward.score * inverse.score).sqrt();
    ShapeMatchCandidate {
        score,
        feature_score: score,
        support_score: 0.0,
        coverage: 0.5 * (forward.coverage + inverse.coverage),
        close_ratio: 0.5 * (forward.close_ratio + inverse.close_ratio),
        mean_distance_meters: 1.0 - 0.5 * (forward.close_ratio + inverse.close_ratio),
        yaw_radians,
        translation_x,
        translation_z,
        in_bounds_points: forward.in_bounds_points.min(inverse.in_bounds_points),
    }
}

fn score_signed_overlap_one_way(
    local: &ShapeMatchGrid,
    remote: &ShapeMatchGrid,
    remote_points: &[(f32, f32)],
    yaw_radians: f32,
    translation_x: f32,
    translation_z: f32,
) -> ShapeMatchCandidate {
    if remote_points.is_empty() {
        return ShapeMatchCandidate::default();
    }
    let mut in_bounds_points = 0usize;
    let mut in_bounds_weight = 0.0f32;
    let mut supported_weight = 0.0f32;
    let mut free_weight = 0.0f32;
    let mut reward_sum = 0.0f32;
    let mut penalty_sum = 0.0f32;
    let mut close_weight = 0.0f32;
    let mut total_weight = 0.0f32;
    let mut distance_sum = 0.0f32;
    for &(remote_x, remote_z) in remote_points {
        let remote_weight = remote
            .sample_occupied_weight_nearest(remote_x, remote_z)
            .unwrap_or(1.0)
            .max(0.25);
        total_weight += remote_weight;
        let (rotated_x, rotated_z) = rotate_xz(yaw_radians, remote_x, remote_z);
        let world_x = rotated_x + translation_x;
        let world_z = rotated_z + translation_z;
        let Some(in_support) = local.sample_support_nearest(world_x, world_z) else {
            continue;
        };
        in_bounds_points += 1;
        in_bounds_weight += remote_weight;
        if !in_support {
            penalty_sum += remote_weight * SHAPE_SEEDED_UNSUPPORTED_PENALTY;
            distance_sum += SHAPE_MATCH_DISTANCE_TRUNCATE_METERS * remote_weight * 0.85;
            continue;
        }
        if local.sample_free_nearest(world_x, world_z).unwrap_or(false) {
            free_weight += remote_weight;
            // Dramatically scale the penalty so that a 1-2% violation (solid wall blocking a doorway)
            // drops the score far enough to break the mirror alias tie.
            penalty_sum += remote_weight * 35.0;
            distance_sum += SHAPE_MATCH_DISTANCE_TRUNCATE_METERS * remote_weight * 15.0;
            continue;
        }
        supported_weight += remote_weight;
        let signed_distance = local
            .sample_signed_distance_nearest(world_x, world_z)
            .unwrap_or(-SHAPE_MATCH_DISTANCE_TRUNCATE_METERS);
        if signed_distance >= 0.0 {
            let local_weight = local
                .sample_occupied_weight_nearest(world_x, world_z)
                .unwrap_or(1.0)
                .max(0.25);
            let interior_bonus =
                (signed_distance / SHAPE_SEEDED_INTERIOR_BONUS_CAP_METERS).clamp(0.0, 1.0);
            reward_sum += (remote_weight * local_weight).sqrt() * (1.0 + interior_bonus * 0.35);
            distance_sum += (SHAPE_SEEDED_INTERIOR_BONUS_CAP_METERS - signed_distance)
                .max(0.0)
                .min(SHAPE_MATCH_DISTANCE_TRUNCATE_METERS)
                * remote_weight;
            if signed_distance >= local.cell_size_meters * 0.75 {
                close_weight += remote_weight;
            }
        } else {
            let empty_distance = (-signed_distance).min(SHAPE_MATCH_DISTANCE_TRUNCATE_METERS);
            let severity = empty_distance / SHAPE_MATCH_DISTANCE_TRUNCATE_METERS;
            penalty_sum += remote_weight * (0.22 + severity * SHAPE_SEEDED_SIGNED_EMPTY_PENALTY);
            distance_sum += empty_distance * remote_weight;
        }
    }

    // HARD VETO: If more than 5% of our points landed squarely in the remote's known empty space,
    // this pose is physically impossible (a table inside an empty hallway). Zero it entirely.
    let free_ratio = free_weight / in_bounds_weight.max(1.0e-6);
    if free_ratio > 0.07 {
        if free_ratio > 0.15 {
            // println!("Hard Veto applied! free_ratio={:.3}", free_ratio);
        }
        return ShapeMatchCandidate::default();
    }

    if in_bounds_points < SHAPE_MIN_CONTOUR_POINTS.min(remote_points.len())
        || total_weight <= 1.0e-6
    {
        return ShapeMatchCandidate::default();
    }
    let coverage = (in_bounds_weight / total_weight).clamp(0.0, 1.0);
    let support_ratio = (supported_weight / in_bounds_weight.max(1.0e-6)).clamp(0.0, 1.0);
    let close_ratio = (close_weight / total_weight).clamp(0.0, 1.0);
    let reward_norm = reward_sum / total_weight.max(1.0e-6);
    let penalty_norm = penalty_sum / total_weight.max(1.0e-6);
    let raw = (reward_norm - penalty_norm * 0.72).max(0.0);
    let score = raw * coverage.sqrt() * (0.30 + 0.70 * support_ratio.sqrt());
    ShapeMatchCandidate {
        score,
        feature_score: raw,
        support_score: support_ratio,
        coverage,
        close_ratio,
        mean_distance_meters: distance_sum / total_weight.max(1.0e-6),
        yaw_radians,
        translation_x,
        translation_z,
        in_bounds_points,
    }
}

fn score_signed_overlap_symmetric(
    local: &ShapeMatchGrid,
    remote: &ShapeMatchGrid,
    yaw_radians: f32,
    translation_x: f32,
    translation_z: f32,
) -> ShapeMatchCandidate {
    let forward = score_signed_overlap_one_way(
        local,
        remote,
        &remote.occupied_points,
        yaw_radians,
        translation_x,
        translation_z,
    );
    if forward.score <= 0.0 {
        return forward;
    }
    let inverse = score_signed_overlap_one_way(
        remote,
        local,
        &local.occupied_points,
        -yaw_radians,
        -translation_x,
        -translation_z,
    );
    if inverse.score <= 0.0 {
        return forward;
    }
    ShapeMatchCandidate {
        score: (forward.score * inverse.score).sqrt(),
        feature_score: (forward.feature_score * inverse.feature_score).sqrt(),
        support_score: (forward.support_score * inverse.support_score).sqrt(),
        coverage: 0.5 * (forward.coverage + inverse.coverage),
        close_ratio: 0.5 * (forward.close_ratio + inverse.close_ratio),
        mean_distance_meters: 0.5 * (forward.mean_distance_meters + inverse.mean_distance_meters),
        yaw_radians,
        translation_x,
        translation_z,
        in_bounds_points: forward.in_bounds_points.min(inverse.in_bounds_points),
    }
}

fn score_signed_support_symmetric(
    local: &ShapeMatchGrid,
    remote: &ShapeMatchGrid,
    yaw_radians: f32,
    translation_x: f32,
    translation_z: f32,
) -> ShapeMatchCandidate {
    let signed =
        score_signed_overlap_symmetric(local, remote, yaw_radians, translation_x, translation_z);
    if signed.score <= 0.0 {
        return signed;
    }
    let (support_forward, support_cov_forward, support_hit_forward, support_points_forward) =
        score_support_overlap_one_way(
            local,
            &remote.support_points,
            yaw_radians,
            translation_x,
            translation_z,
        );
    let (support_inverse, support_cov_inverse, support_hit_inverse, support_points_inverse) =
        score_support_overlap_one_way(
            remote,
            &local.support_points,
            -yaw_radians,
            -translation_x,
            -translation_z,
        );
    let support_score = if support_forward > 0.0 && support_inverse > 0.0 {
        (support_forward * support_inverse).sqrt()
    } else {
        0.0
    };
    ShapeMatchCandidate {
        score: signed.score * 0.62 + support_score * 0.38,
        feature_score: signed.score,
        support_score,
        coverage: signed.coverage * 0.65 + 0.35 * (support_cov_forward + support_cov_inverse) * 0.5,
        close_ratio: signed.close_ratio * 0.65
            + 0.35 * (support_hit_forward + support_hit_inverse) * 0.5,
        mean_distance_meters: signed.mean_distance_meters,
        yaw_radians,
        translation_x,
        translation_z,
        in_bounds_points: signed
            .in_bounds_points
            .min(support_points_forward.max(signed.in_bounds_points))
            .min(support_points_inverse.max(signed.in_bounds_points)),
    }
}

fn apply_seed_prior(
    mut candidate: ShapeMatchCandidate,
    seed: ShapeMatchCandidate,
) -> ShapeMatchCandidate {
    let translation_delta = ((candidate.translation_x - seed.translation_x).powi(2)
        + (candidate.translation_z - seed.translation_z).powi(2))
    .sqrt();
    let yaw_delta = wrap_angle(candidate.yaw_radians - seed.yaw_radians).abs();
    let translation_term =
        -0.5 * (translation_delta / SHAPE_SEEDED_PRIOR_TRANSLATION_SIGMA_METERS).powi(2);
    let yaw_term = -0.5 * (yaw_delta / SHAPE_SEEDED_PRIOR_YAW_SIGMA_RADIANS).powi(2);
    let prior = (translation_term + yaw_term).exp().clamp(0.0, 1.0);
    let factor = SHAPE_SEEDED_PRIOR_FLOOR + (1.0 - SHAPE_SEEDED_PRIOR_FLOOR) * prior;
    candidate.score *= factor;
    candidate.support_score = candidate.support_score * 0.75 + prior * 0.25;
    candidate
}

fn search_mask_overlap_global(
    local: &ShapeMatchGrid,
    remote: &ShapeMatchGrid,
    top_k: usize,
) -> Vec<ShapeMatchCandidate> {
    let mut finalists = Vec::new();
    let local_min_x = local.origin_x;
    let local_max_x = local.max_x();
    let local_min_z = local.origin_z;
    let local_max_z = local.max_z();
    let mut yaw = -PI;
    while yaw < PI {
        let Some((remote_min_x, remote_max_x, remote_min_z, remote_max_z)) =
            rotated_bounds(&remote.occupied_points, yaw)
        else {
            yaw += SHAPE_GLOBAL_YAW_STEP_RADIANS;
            continue;
        };
        let translation_min_x = local_min_x - remote_max_x;
        let translation_max_x = local_max_x - remote_min_x;
        let translation_min_z = local_min_z - remote_max_z;
        let translation_max_z = local_max_z - remote_min_z;
        let step = local.cell_size_meters.max(1.0e-3);
        let steps_x = ((translation_max_x - translation_min_x) / step)
            .ceil()
            .max(0.0) as usize;
        let steps_z = ((translation_max_z - translation_min_z) / step)
            .ceil()
            .max(0.0) as usize;
        for x_step in 0..=steps_x {
            let translation_x = translation_min_x + x_step as f32 * step;
            for z_step in 0..=steps_z {
                let translation_z = translation_min_z + z_step as f32 * step;
                let candidate = score_mask_overlap_one_way(
                    local,
                    &remote.occupied_points,
                    yaw,
                    translation_x,
                    translation_z,
                );
                push_shape_candidate(
                    &mut finalists,
                    candidate,
                    top_k,
                    step * 1.5,
                    SHAPE_GLOBAL_YAW_STEP_RADIANS * 1.25,
                );
            }
        }
        yaw += SHAPE_GLOBAL_YAW_STEP_RADIANS;
    }
    finalists
}

fn search_mask_overlap_refine(
    local: &ShapeMatchGrid,
    remote: &ShapeMatchGrid,
    seeds: &[ShapeMatchCandidate],
    translation_window_meters: f32,
    yaw_window_radians: f32,
    yaw_step_radians: f32,
    top_k: usize,
) -> Vec<ShapeMatchCandidate> {
    let mut finalists = Vec::new();
    let step = local.cell_size_meters.max(1.0e-3);
    let translation_steps = (translation_window_meters / step).ceil().max(1.0) as isize;
    let yaw_steps = (yaw_window_radians / yaw_step_radians).ceil().max(1.0) as isize;
    for seed in seeds {
        for yaw_step in -yaw_steps..=yaw_steps {
            let yaw_radians = wrap_angle(seed.yaw_radians + yaw_step as f32 * yaw_step_radians);
            for x_step in -translation_steps..=translation_steps {
                let translation_x = seed.translation_x + x_step as f32 * step;
                for z_step in -translation_steps..=translation_steps {
                    let translation_z = seed.translation_z + z_step as f32 * step;
                    let candidate = score_mask_overlap_symmetric(
                        local,
                        remote,
                        yaw_radians,
                        translation_x,
                        translation_z,
                    );
                    push_shape_candidate(
                        &mut finalists,
                        candidate,
                        top_k,
                        step * 1.25,
                        yaw_step_radians * 1.25,
                    );
                }
            }
        }
    }
    finalists
}

fn search_mask_overlap_translation_only(
    local: &ShapeMatchGrid,
    remote: &ShapeMatchGrid,
    seed: ShapeMatchCandidate,
    translation_window_meters: f32,
    top_k: usize,
) -> Vec<ShapeMatchCandidate> {
    let mut finalists = Vec::new();
    let step = local.cell_size_meters.max(1.0e-3);
    let translation_steps = (translation_window_meters / step).ceil().max(1.0) as isize;
    for x_step in -translation_steps..=translation_steps {
        let translation_x = seed.translation_x + x_step as f32 * step;
        for z_step in -translation_steps..=translation_steps {
            let translation_z = seed.translation_z + z_step as f32 * step;
            let candidate = score_mask_overlap_symmetric(
                local,
                remote,
                seed.yaw_radians,
                translation_x,
                translation_z,
            );
            push_shape_candidate(&mut finalists, candidate, top_k, step * 1.25, 0.02);
        }
    }
    finalists
}

fn search_signed_overlap_seeded(
    local: &ShapeMatchGrid,
    remote: &ShapeMatchGrid,
    seeds: &[ShapeMatchCandidate],
    translation_window_meters: f32,
    yaw_window_radians: f32,
    yaw_step_radians: f32,
    top_k: usize,
) -> Vec<ShapeMatchCandidate> {
    let mut finalists = Vec::new();
    let step = local.cell_size_meters.max(1.0e-3);
    let translation_steps = (translation_window_meters / step).ceil().max(1.0) as isize;
    let yaw_steps = (yaw_window_radians / yaw_step_radians).ceil().max(1.0) as isize;
    for seed in seeds {
        for yaw_step in -yaw_steps..=yaw_steps {
            let yaw_radians = wrap_angle(seed.yaw_radians + yaw_step as f32 * yaw_step_radians);
            for x_step in -translation_steps..=translation_steps {
                let translation_x = seed.translation_x + x_step as f32 * step;
                for z_step in -translation_steps..=translation_steps {
                    let translation_z = seed.translation_z + z_step as f32 * step;
                    let candidate = apply_seed_prior(
                        score_signed_overlap_symmetric(
                            local,
                            remote,
                            yaw_radians,
                            translation_x,
                            translation_z,
                        ),
                        *seed,
                    );
                    push_shape_candidate(
                        &mut finalists,
                        candidate,
                        top_k,
                        step * 1.25,
                        yaw_step_radians * 1.25,
                    );
                }
            }
        }
    }
    finalists
}

fn search_signed_support_seeded(
    local: &ShapeMatchGrid,
    remote: &ShapeMatchGrid,
    seeds: &[ShapeMatchCandidate],
    translation_window_meters: f32,
    yaw_window_radians: f32,
    yaw_step_radians: f32,
    top_k: usize,
) -> Vec<ShapeMatchCandidate> {
    let mut finalists = Vec::new();
    let step = local.cell_size_meters.max(1.0e-3);
    let translation_steps = (translation_window_meters / step).ceil().max(1.0) as isize;
    let yaw_steps = (yaw_window_radians / yaw_step_radians).ceil().max(1.0) as isize;
    for seed in seeds {
        for yaw_step in -yaw_steps..=yaw_steps {
            let yaw_radians = wrap_angle(seed.yaw_radians + yaw_step as f32 * yaw_step_radians);
            for x_step in -translation_steps..=translation_steps {
                let translation_x = seed.translation_x + x_step as f32 * step;
                for z_step in -translation_steps..=translation_steps {
                    let translation_z = seed.translation_z + z_step as f32 * step;
                    let candidate = apply_seed_prior(
                        score_signed_support_symmetric(
                            local,
                            remote,
                            yaw_radians,
                            translation_x,
                            translation_z,
                        ),
                        *seed,
                    );
                    push_shape_candidate(
                        &mut finalists,
                        candidate,
                        top_k,
                        step * 1.25,
                        yaw_step_radians * 1.25,
                    );
                }
            }
        }
    }
    finalists
}

fn nearest_manual_finalist(
    finalists: &[ShapeMatchCandidate],
    manual_pose: ManualPose,
) -> Option<NearestManualFinalist> {
    finalists
        .iter()
        .copied()
        .map(|candidate| {
            let planar_delta_meters = ((candidate.translation_x - manual_pose.shift_x_meters)
                .powi(2)
                + (candidate.translation_z - manual_pose.shift_y_meters).powi(2))
            .sqrt();
            let yaw_delta_radians =
                wrap_angle(candidate.yaw_radians - manual_pose.rotation_radians).abs();
            let combined_distance = planar_delta_meters + yaw_delta_radians * 0.35;
            NearestManualFinalist {
                candidate,
                planar_delta_meters,
                yaw_delta_radians,
                combined_distance,
            }
        })
        .min_by(|left, right| {
            left.combined_distance
                .total_cmp(&right.combined_distance)
                .then_with(|| right.candidate.score.total_cmp(&left.candidate.score))
        })
}

fn run_shape_match_analysis(
    local: &XrDepthAlignDescriptor,
    remote: &XrDepthAlignDescriptor,
    manual_pose: Option<ManualPose>,
    band: &ShapeBandSpec,
) -> Option<ShapeMatchReport> {
    let (Some(local_map), Some(remote_map)) =
        (local.height_map.as_ref(), remote.height_map.as_ref())
    else {
        return None;
    };

    let local_global = build_shape_match_grid(local_map, SHAPE_GLOBAL_CELL_METERS, band)?;
    let remote_global = build_shape_match_grid(remote_map, SHAPE_GLOBAL_CELL_METERS, band)?;
    let global = search_shape_global(&local_global, &remote_global, SHAPE_GLOBAL_TOP_K);
    if global.is_empty() {
        return None;
    }

    let local_refine = build_shape_match_grid(local_map, SHAPE_REFINE_CELL_METERS, band)?;
    let remote_refine = build_shape_match_grid(remote_map, SHAPE_REFINE_CELL_METERS, band)?;
    let refined = search_shape_refine(
        &local_refine,
        &remote_refine,
        &global,
        0.60,
        SHAPE_REFINE_YAW_WINDOW_RADIANS,
        SHAPE_REFINE_YAW_STEP_RADIANS,
        SHAPE_REFINE_TOP_K,
        false,
    );
    if refined.is_empty() {
        return None;
    }

    let local_final = build_shape_match_grid(local_map, SHAPE_FINAL_CELL_METERS, band)?;
    let remote_final = build_shape_match_grid(remote_map, SHAPE_FINAL_CELL_METERS, band)?;
    let finalists = search_shape_refine(
        &local_final,
        &remote_final,
        &refined,
        0.24,
        SHAPE_FINAL_YAW_WINDOW_RADIANS,
        SHAPE_FINAL_YAW_STEP_RADIANS,
        SHAPE_FINAL_TOP_K,
        true,
    );
    if finalists.is_empty() {
        return None;
    }

    let manual_candidate = manual_pose
        .map(|manual_pose| manual_shape_candidate(&local_final, &remote_final, manual_pose));
    Some(ShapeMatchReport {
        label: band.label.clone(),
        finalists,
        manual_candidate,
    })
}

fn run_hough_shape_analysis(
    local: &XrDepthAlignDescriptor,
    remote: &XrDepthAlignDescriptor,
    manual_pose: Option<ManualPose>,
) -> Option<ShapeMatchReport> {
    let (Some(local_map), Some(remote_map)) =
        (local.height_map.as_ref(), remote.height_map.as_ref())
    else {
        return None;
    };
    let band = shape_band_obstacle();
    let local_global = build_shape_match_grid(local_map, SHAPE_GLOBAL_CELL_METERS, &band)?;
    let remote_global = build_shape_match_grid(remote_map, SHAPE_GLOBAL_CELL_METERS, &band)?;
    let yaw_candidates = hough_yaw_candidates(&local_global, &remote_global);
    if yaw_candidates.is_empty() {
        return None;
    }
    let global = search_shape_global_yaws(
        &local_global,
        &remote_global,
        &yaw_candidates,
        SHAPE_GLOBAL_TOP_K,
    );
    if global.is_empty() {
        return None;
    }
    let local_refine = build_shape_match_grid(local_map, SHAPE_REFINE_CELL_METERS, &band)?;
    let remote_refine = build_shape_match_grid(remote_map, SHAPE_REFINE_CELL_METERS, &band)?;
    let refined = search_shape_refine(
        &local_refine,
        &remote_refine,
        &global,
        0.60,
        SHAPE_REFINE_YAW_WINDOW_RADIANS,
        SHAPE_REFINE_YAW_STEP_RADIANS,
        SHAPE_REFINE_TOP_K,
        false,
    );
    if refined.is_empty() {
        return None;
    }
    let local_final = build_shape_match_grid(local_map, SHAPE_FINAL_CELL_METERS, &band)?;
    let remote_final = build_shape_match_grid(remote_map, SHAPE_FINAL_CELL_METERS, &band)?;
    let finalists = search_shape_refine(
        &local_final,
        &remote_final,
        &refined,
        0.24,
        SHAPE_FINAL_YAW_WINDOW_RADIANS,
        SHAPE_FINAL_YAW_STEP_RADIANS,
        SHAPE_FINAL_TOP_K,
        true,
    );
    if finalists.is_empty() {
        return None;
    }
    let manual_candidate = manual_pose
        .map(|manual_pose| manual_shape_candidate(&local_final, &remote_final, manual_pose));
    Some(ShapeMatchReport {
        label: "shape_hough".to_string(),
        finalists,
        manual_candidate,
    })
}

fn print_shape_match_report(
    local: &XrDepthAlignDescriptor,
    remote: &XrDepthAlignDescriptor,
    report: &ShapeMatchReport,
    elapsed_millis: u128,
) {
    println!("{}_match_ms: {}", report.label, elapsed_millis);
    for (index, candidate) in report.finalists.iter().take(3).enumerate() {
        println!(
            "{}_best_{}: yaw {:.3} rad ({:.1} deg) | translation ({:.3}, {:.3}, {:.3}) | score {:.3} | iou {:.3} | coverage {:.3} | close {:.3} | dist {:.3} m | points {}",
            report.label,
            index + 1,
            candidate.yaw_radians,
            candidate.yaw_radians.to_degrees(),
            candidate.translation_x,
            local.floor_y - remote.floor_y,
            candidate.translation_z,
            candidate.score,
            candidate.support_score,
            candidate.coverage,
            candidate.close_ratio,
            candidate.mean_distance_meters,
            candidate.in_bounds_points,
        );
    }
    if let Some(manual_candidate) = report.manual_candidate {
        let finalist_rank = report
            .finalists
            .iter()
            .position(|candidate| candidate.score < manual_candidate.score)
            .map(|rank| rank + 1)
            .unwrap_or(report.finalists.len() + 1);
        println!(
            "{}_manual: yaw {:.3} rad ({:.1} deg) | translation ({:.3}, {:.3}, {:.3}) | score {:.3} | iou {:.3} | coverage {:.3} | close {:.3} | dist {:.3} m | finalist_rank {}",
            report.label,
            manual_candidate.yaw_radians,
            manual_candidate.yaw_radians.to_degrees(),
            manual_candidate.translation_x,
            local.floor_y - remote.floor_y,
            manual_candidate.translation_z,
            manual_candidate.score,
            manual_candidate.support_score,
            manual_candidate.coverage,
            manual_candidate.close_ratio,
            manual_candidate.mean_distance_meters,
            finalist_rank,
        );
    } else {
        println!("{}_manual: none", report.label);
    }
}

fn print_focused_match_report(
    local: &XrDepthAlignDescriptor,
    remote: &XrDepthAlignDescriptor,
    report: &FocusedMatchReport,
    elapsed_millis: u128,
) {
    println!("{}_match_ms: {}", report.label, elapsed_millis);
    for (index, candidate) in report.finalists.iter().take(3).enumerate() {
        println!(
            "{}_best_{}: yaw {:.3} rad ({:.1} deg) | translation ({:.3}, {:.3}, {:.3}) | score {:.3} | runtime {:.3} | base {:.3} | line {:.3} | blob {:.3} | prior {:.3} | residual {:.3} m | points {}",
            report.label,
            index + 1,
            candidate.pose.yaw_radians,
            candidate.pose.yaw_radians.to_degrees(),
            candidate.pose.translation_x,
            local.floor_y - remote.floor_y,
            candidate.pose.translation_z,
            candidate.score,
            candidate.runtime_score,
            candidate.signed_score,
            candidate.line_score,
            candidate.blob_score,
            candidate.seed_prior,
            candidate.runtime_residual_meters,
            candidate.pose.in_bounds_points,
        );
    }
    if let Some(manual_candidate) = report.manual_candidate {
        let finalist_rank = report
            .finalists
            .iter()
            .position(|candidate| candidate.score < manual_candidate.score)
            .map(|rank| rank + 1)
            .unwrap_or(report.finalists.len() + 1);
        println!(
            "{}_manual: yaw {:.3} rad ({:.1} deg) | translation ({:.3}, {:.3}, {:.3}) | score {:.3} | runtime {:.3} | base {:.3} | line {:.3} | blob {:.3} | prior {:.3} | residual {:.3} m | finalist_rank {}",
            report.label,
            manual_candidate.pose.yaw_radians,
            manual_candidate.pose.yaw_radians.to_degrees(),
            manual_candidate.pose.translation_x,
            local.floor_y - remote.floor_y,
            manual_candidate.pose.translation_z,
            manual_candidate.score,
            manual_candidate.runtime_score,
            manual_candidate.signed_score,
            manual_candidate.line_score,
            manual_candidate.blob_score,
            manual_candidate.seed_prior,
            manual_candidate.runtime_residual_meters,
            finalist_rank,
        );
    } else {
        println!("{}_manual: none", report.label);
    }
}

fn focused_variant_score(
    evidence: FocusedPoseEvidence,
    weights: FocusedCueWeights,
    has_blob_cue: bool,
) -> FocusedMatchCandidate {
    let mut score_sum = weights.runtime_weight * evidence.runtime_score
        + weights.signed_weight * evidence.signed_score
        + weights.line_weight * evidence.line_score;
    let mut total_weight = weights.runtime_weight + weights.signed_weight + weights.line_weight;
    if has_blob_cue && weights.blob_weight > 0.0 {
        score_sum += weights.blob_weight * evidence.blob_score;
        total_weight += weights.blob_weight;
    }
    let score = (score_sum / total_weight.max(1.0e-6)) * evidence.seed_prior;
    FocusedMatchCandidate {
        pose: evidence.pose,
        score,
        runtime_score: evidence.runtime_score,
        runtime_residual_meters: evidence.runtime_residual_meters,
        signed_score: evidence.signed_score,
        line_score: evidence.line_score,
        blob_score: evidence.blob_score,
        seed_prior: evidence.seed_prior,
    }
}

fn push_focused_evidence(
    finalists: &mut Vec<FocusedPoseEvidence>,
    candidate: FocusedPoseEvidence,
    limit: usize,
    translation_epsilon: f32,
    yaw_epsilon: f32,
    has_blob_cue: bool,
) {
    let candidate_score = focused_variant_score(
        candidate,
        FocusedCueWeights {
            label: "coarse",
            runtime_weight: 0.0,
            signed_weight: 0.58,
            line_weight: 0.28,
            blob_weight: 0.14,
        },
        has_blob_cue,
    )
    .score;
    if !candidate_score.is_finite() || candidate_score <= 0.0 {
        return;
    }
    if let Some(existing) = finalists.iter_mut().find(|existing| {
        !candidate_is_distinct(
            existing.pose,
            candidate.pose,
            translation_epsilon,
            yaw_epsilon,
        )
    }) {
        let existing_score = focused_variant_score(
            *existing,
            FocusedCueWeights {
                label: "coarse",
                runtime_weight: 0.0,
                signed_weight: 0.58,
                line_weight: 0.28,
                blob_weight: 0.14,
            },
            has_blob_cue,
        )
        .score;
        if candidate_score > existing_score {
            *existing = candidate;
        }
    } else {
        finalists.push(candidate);
    }
    finalists.sort_by(|left, right| {
        focused_variant_score(
            *right,
            FocusedCueWeights {
                label: "coarse",
                runtime_weight: 0.0,
                signed_weight: 0.58,
                line_weight: 0.28,
                blob_weight: 0.14,
            },
            has_blob_cue,
        )
        .score
        .total_cmp(
            &focused_variant_score(
                *left,
                FocusedCueWeights {
                    label: "coarse",
                    runtime_weight: 0.0,
                    signed_weight: 0.58,
                    line_weight: 0.28,
                    blob_weight: 0.14,
                },
                has_blob_cue,
            )
            .score,
        )
        .then_with(|| {
            left.pose
                .mean_distance_meters
                .total_cmp(&right.pose.mean_distance_meters)
        })
    });
    finalists.truncate(limit.max(1));
}

fn score_focused_pose_evidence(
    local: &XrDepthAlignDescriptor,
    remote: &XrDepthAlignDescriptor,
    wall_local: &ShapeMatchGrid,
    wall_remote: &ShapeMatchGrid,
    wall_local_points: &[(f32, f32)],
    wall_remote_points: &[(f32, f32)],
    blob_local_points: &[(f32, f32)],
    blob_remote_points: &[(f32, f32)],
    local_components: &[ShapeComponent],
    remote_components: &[ShapeComponent],
    seed_pose: ShapeMatchCandidate,
    yaw_radians: f32,
    translation_x: f32,
    translation_z: f32,
    with_runtime: bool,
) -> Option<FocusedPoseEvidence> {
    let base = score_shape_alignment_symmetric(
        wall_local,
        wall_remote,
        yaw_radians,
        translation_x,
        translation_z,
    );
    if base.score <= 0.0 {
        return None;
    }
    let line_score = score_wall_profile_alignment(
        wall_local_points,
        wall_remote_points,
        yaw_radians,
        translation_x,
        translation_z,
        wall_local.cell_size_meters,
    );
    let blob_score = if blob_local_points.len() >= SHAPE_MIN_CONTOUR_POINTS
        && blob_remote_points.len() >= SHAPE_MIN_CONTOUR_POINTS
    {
        score_wall_profile_alignment(
            blob_local_points,
            blob_remote_points,
            yaw_radians,
            translation_x,
            translation_z,
            SHAPE_REFINE_CELL_METERS,
        )
    } else {
        0.0
    };
    let pose = ShapeMatchCandidate {
        yaw_radians,
        translation_x,
        translation_z,
        coverage: base.coverage,
        close_ratio: base.close_ratio,
        mean_distance_meters: base.mean_distance_meters,
        in_bounds_points: base.in_bounds_points,
        ..ShapeMatchCandidate::default()
    };
    let mut evidence = FocusedPoseEvidence {
        pose,
        signed_score: base.score,
        line_score,
        blob_score,
        runtime_score: 0.0,
        runtime_residual_meters: base.mean_distance_meters,
        seed_prior: seed_prior_factor(pose, seed_pose),
    };
    if with_runtime {
        let rescored = xr_depth_align_rescore_remote_to_local(
            local,
            remote,
            XrDepthAlignSolution {
                yaw_radians,
                translation: vec3(translation_x, local.floor_y - remote.floor_y, translation_z),
                ..XrDepthAlignSolution::default()
            },
        );
        evidence.runtime_score = rescored.ranking_confidence();
        evidence.runtime_residual_meters = rescored.residual_meters;
        evidence.pose.mean_distance_meters = if rescored.residual_meters.is_finite() {
            rescored.residual_meters
        } else {
            base.mean_distance_meters
        };
    }
    Some(evidence)
}

fn search_focused_pose_evidences(
    local: &XrDepthAlignDescriptor,
    remote: &XrDepthAlignDescriptor,
    wall_local: &ShapeMatchGrid,
    wall_remote: &ShapeMatchGrid,
    wall_local_points: &[(f32, f32)],
    wall_remote_points: &[(f32, f32)],
    blob_local_points: &[(f32, f32)],
    blob_remote_points: &[(f32, f32)],
    local_components: &[ShapeComponent],
    remote_components: &[ShapeComponent],
    seeds: &[ShapeMatchCandidate],
    translation_window_meters: f32,
    translation_step_meters: f32,
    yaw_window_radians: f32,
    yaw_step_radians: f32,
    top_k: usize,
) -> Vec<FocusedPoseEvidence> {
    if seeds.is_empty() {
        return Vec::new();
    }
    let has_blob_cue = blob_local_points.len() >= SHAPE_MIN_CONTOUR_POINTS
        && blob_remote_points.len() >= SHAPE_MIN_CONTOUR_POINTS;
    let worker_count = analysis_thread_count(seeds.len());
    let chunk_size = seeds.len().div_ceil(worker_count.max(1));
    let partials = std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for seed_chunk in seeds.chunks(chunk_size.max(1)) {
            let seed_chunk = seed_chunk.to_vec();
            handles.push(scope.spawn(move || {
                let mut finalists = Vec::new();
                let translation_steps = (translation_window_meters / translation_step_meters)
                    .ceil()
                    .max(1.0) as isize;
                let yaw_steps = (yaw_window_radians / yaw_step_radians).ceil().max(1.0) as isize;
                for seed in &seed_chunk {
                    for yaw_step in -yaw_steps..=yaw_steps {
                        let yaw_radians =
                            wrap_angle(seed.yaw_radians + yaw_step as f32 * yaw_step_radians);
                        for x_step in -translation_steps..=translation_steps {
                            let translation_x =
                                seed.translation_x + x_step as f32 * translation_step_meters;
                            for z_step in -translation_steps..=translation_steps {
                                let translation_z =
                                    seed.translation_z + z_step as f32 * translation_step_meters;
                                let Some(evidence) = score_focused_pose_evidence(
                                    local,
                                    remote,
                                    wall_local,
                                    wall_remote,
                                    wall_local_points,
                                    wall_remote_points,
                                    blob_local_points,
                                    blob_remote_points,
                                    local_components,
                                    remote_components,
                                    *seed,
                                    yaw_radians,
                                    translation_x,
                                    translation_z,
                                    false,
                                ) else {
                                    continue;
                                };
                                push_focused_evidence(
                                    &mut finalists,
                                    evidence,
                                    top_k,
                                    translation_step_meters * 0.9,
                                    yaw_step_radians * 1.25,
                                    has_blob_cue,
                                );
                            }
                        }
                    }
                }
                finalists
            }));
        }
        let mut partials = Vec::new();
        for handle in handles {
            partials.push(
                handle
                    .join()
                    .expect("focused local-search worker should not panic"),
            );
        }
        partials
    });

    let mut finalists = Vec::new();
    for partial in partials {
        for candidate in partial {
            push_focused_evidence(
                &mut finalists,
                candidate,
                top_k,
                translation_step_meters * 0.9,
                yaw_step_radians * 1.25,
                has_blob_cue,
            );
        }
    }
    finalists
}

fn build_focused_match_report(
    local: &XrDepthAlignDescriptor,
    remote: &XrDepthAlignDescriptor,
    wall_local: &ShapeMatchGrid,
    wall_remote: &ShapeMatchGrid,
    wall_local_points: &[(f32, f32)],
    wall_remote_points: &[(f32, f32)],
    blob_local_points: &[(f32, f32)],
    blob_remote_points: &[(f32, f32)],
    local_components: &[ShapeComponent],
    remote_components: &[ShapeComponent],
    label: &str,
    weights: FocusedCueWeights,
    seed_pose: ShapeMatchCandidate,
    evidences: &[FocusedPoseEvidence],
    manual_pose: Option<ManualPose>,
) -> FocusedMatchReport {
    let has_blob_cue = blob_local_points.len() >= SHAPE_MIN_CONTOUR_POINTS
        && blob_remote_points.len() >= SHAPE_MIN_CONTOUR_POINTS;
    let mut finalists = evidences
        .iter()
        .copied()
        .map(|evidence| focused_variant_score(evidence, weights, has_blob_cue))
        .collect::<Vec<_>>();
    finalists.sort_by(|left, right| {
        right.score.total_cmp(&left.score).then_with(|| {
            left.runtime_residual_meters
                .total_cmp(&right.runtime_residual_meters)
        })
    });
    finalists.truncate(FOCUSED_FINAL_TOP_K);
    let manual_candidate = manual_pose.and_then(|manual_pose| {
        score_focused_pose_evidence(
            local,
            remote,
            wall_local,
            wall_remote,
            wall_local_points,
            wall_remote_points,
            blob_local_points,
            blob_remote_points,
            local_components,
            remote_components,
            seed_pose,
            manual_pose.rotation_radians,
            manual_pose.shift_x_meters,
            manual_pose.shift_y_meters,
            true,
        )
        .map(|evidence| focused_variant_score(evidence, weights, has_blob_cue))
    });
    FocusedMatchReport {
        label: label.to_string(),
        finalists,
        manual_candidate,
    }
}

fn run_runtime_seeded_focus_suite(
    local: &XrDepthAlignDescriptor,
    remote: &XrDepthAlignDescriptor,
    manual_pose: Option<ManualPose>,
    seed: XrDepthAlignSolution,
) -> Vec<FocusedMatchReport> {
    let structural_band = shape_band_obstacle();
    let wall_band = ShapeBandSpec {
        label: "focused_wall".to_string(),
        min_height_meters: 0.60,
        max_height_meters: Some(1.80),
        floor_offset_meters: 0.0,
        drop_border_components: false,
    };
    let (Some(local_map), Some(remote_map)) =
        (local.height_map.as_ref(), remote.height_map.as_ref())
    else {
        return Vec::new();
    };
    let Some(structural_local) =
        build_shape_match_grid(local_map, SHAPE_FINAL_CELL_METERS, &structural_band)
    else {
        return Vec::new();
    };
    let Some(structural_remote) =
        build_shape_match_grid(remote_map, SHAPE_FINAL_CELL_METERS, &structural_band)
    else {
        return Vec::new();
    };
    let Some(wall_local) = build_shape_match_grid(local_map, SHAPE_FINAL_CELL_METERS, &wall_band)
    else {
        return Vec::new();
    };
    let Some(wall_remote) = build_shape_match_grid(remote_map, SHAPE_FINAL_CELL_METERS, &wall_band)
    else {
        return Vec::new();
    };
    let local_lines = extract_hough_lines(&wall_local, FOCUSED_HOUGH_TOP_LINES);
    let remote_lines = extract_hough_lines(&wall_remote, FOCUSED_HOUGH_TOP_LINES);

    let furniture_band = shape_band_furniture();
    let local_blob_grid =
        build_shape_match_grid(local_map, SHAPE_REFINE_CELL_METERS, &furniture_band);
    let remote_blob_grid =
        build_shape_match_grid(remote_map, SHAPE_REFINE_CELL_METERS, &furniture_band);
    let local_components = local_blob_grid
        .as_ref()
        .map(|grid| extract_shape_components(grid, 8))
        .unwrap_or_default();
    let remote_components = remote_blob_grid
        .as_ref()
        .map(|grid| extract_shape_components(grid, 8))
        .unwrap_or_default();
    let local_blob_points = local_blob_grid
        .as_ref()
        .map(|grid| grid.occupied_points.as_slice())
        .unwrap_or(&[]);
    let remote_blob_points = remote_blob_grid
        .as_ref()
        .map(|grid| grid.occupied_points.as_slice())
        .unwrap_or(&[]);
    let seed_pose = ShapeMatchCandidate {
        yaw_radians: seed.yaw_radians,
        translation_x: seed.translation.x,
        translation_z: seed.translation.z,
        ..ShapeMatchCandidate::default()
    };
    let mut seeds = vec![seed_pose];
    for candidate in search_mask_overlap_translation_only(
        &structural_local,
        &structural_remote,
        seed_pose,
        FOCUSED_LOCAL_TRANSLATION_WINDOW_METERS,
        6,
    ) {
        push_shape_candidate(
            &mut seeds,
            candidate,
            6,
            SHAPE_FINAL_CELL_METERS * 1.1,
            0.02,
        );
    }
    let coarse = search_focused_pose_evidences(
        local,
        remote,
        &structural_local,
        &structural_remote,
        &wall_local.contour_points,
        &wall_remote.contour_points,
        local_blob_points,
        remote_blob_points,
        &local_components,
        &remote_components,
        &seeds,
        FOCUSED_LOCAL_TRANSLATION_WINDOW_METERS,
        SHAPE_FINAL_CELL_METERS * 2.0,
        FOCUSED_LOCAL_YAW_WINDOW_RADIANS,
        FOCUSED_LOCAL_YAW_STEP_RADIANS,
        FOCUSED_SEARCH_TOP_K,
    );
    if coarse.is_empty() {
        return Vec::new();
    }
    let refine_seeds = coarse
        .iter()
        .map(|evidence| evidence.pose)
        .collect::<Vec<_>>();
    let mut refined = search_focused_pose_evidences(
        local,
        remote,
        &structural_local,
        &structural_remote,
        &wall_local.contour_points,
        &wall_remote.contour_points,
        local_blob_points,
        remote_blob_points,
        &local_components,
        &remote_components,
        &refine_seeds,
        FOCUSED_REFINE_TRANSLATION_WINDOW_METERS,
        SHAPE_FINAL_CELL_METERS * 0.5,
        FOCUSED_REFINE_YAW_WINDOW_RADIANS,
        FOCUSED_REFINE_YAW_STEP_RADIANS,
        FOCUSED_SEARCH_TOP_K,
    );
    if refined.is_empty() {
        refined = coarse;
    }
    for evidence in &mut refined {
        if let Some(runtime_evidence) = score_focused_pose_evidence(
            local,
            remote,
            &structural_local,
            &structural_remote,
            &wall_local.contour_points,
            &wall_remote.contour_points,
            local_blob_points,
            remote_blob_points,
            &local_components,
            &remote_components,
            seed_pose,
            evidence.pose.yaw_radians,
            evidence.pose.translation_x,
            evidence.pose.translation_z,
            true,
        ) {
            evidence.runtime_score = runtime_evidence.runtime_score;
            evidence.runtime_residual_meters = runtime_evidence.runtime_residual_meters;
            evidence.pose.mean_distance_meters = runtime_evidence.pose.mean_distance_meters;
        }
    }
    let mut locked_translation = search_focused_pose_evidences(
        local,
        remote,
        &structural_local,
        &structural_remote,
        &wall_local.contour_points,
        &wall_remote.contour_points,
        local_blob_points,
        remote_blob_points,
        &local_components,
        &remote_components,
        &[seed_pose],
        SHAPE_FINAL_CELL_METERS,
        SHAPE_FINAL_CELL_METERS,
        FOCUSED_LOCAL_YAW_WINDOW_RADIANS,
        FOCUSED_REFINE_YAW_STEP_RADIANS,
        FOCUSED_SEARCH_TOP_K,
    );
    for evidence in &mut locked_translation {
        if let Some(runtime_evidence) = score_focused_pose_evidence(
            local,
            remote,
            &structural_local,
            &structural_remote,
            &wall_local.contour_points,
            &wall_remote.contour_points,
            local_blob_points,
            remote_blob_points,
            &local_components,
            &remote_components,
            seed_pose,
            evidence.pose.yaw_radians,
            evidence.pose.translation_x,
            evidence.pose.translation_z,
            true,
        ) {
            evidence.runtime_score = runtime_evidence.runtime_score;
            evidence.runtime_residual_meters = runtime_evidence.runtime_residual_meters;
            evidence.pose.mean_distance_meters = runtime_evidence.pose.mean_distance_meters;
        }
    }
    let variants = [
        FocusedCueWeights {
            label: "runtime_seeded_signed_local",
            runtime_weight: 0.45,
            signed_weight: 0.55,
            line_weight: 0.0,
            blob_weight: 0.0,
        },
        FocusedCueWeights {
            label: "runtime_seeded_line_local",
            runtime_weight: 0.38,
            signed_weight: 0.37,
            line_weight: 0.25,
            blob_weight: 0.0,
        },
        FocusedCueWeights {
            label: "runtime_seeded_blob_local",
            runtime_weight: 0.38,
            signed_weight: 0.37,
            line_weight: 0.0,
            blob_weight: 0.25,
        },
        FocusedCueWeights {
            label: "runtime_seeded_line_blob_local",
            runtime_weight: 0.36,
            signed_weight: 0.34,
            line_weight: 0.22,
            blob_weight: 0.08,
        },
    ];
    let mut reports = vec![build_focused_match_report(
        local,
        remote,
        &structural_local,
        &structural_remote,
        &wall_local.contour_points,
        &wall_remote.contour_points,
        local_blob_points,
        remote_blob_points,
        &local_components,
        &remote_components,
        "runtime_seeded_line_blob_locktx",
        FocusedCueWeights {
            label: "runtime_seeded_line_blob_locktx",
            runtime_weight: 0.40,
            signed_weight: 0.20,
            line_weight: 0.28,
            blob_weight: 0.12,
        },
        seed_pose,
        &locked_translation,
        manual_pose,
    )];
    reports.extend(variants.into_iter().map(|weights| {
        build_focused_match_report(
            local,
            remote,
            &structural_local,
            &structural_remote,
            &wall_local.contour_points,
            &wall_remote.contour_points,
            local_blob_points,
            remote_blob_points,
            &local_components,
            &remote_components,
            weights.label,
            weights,
            seed_pose,
            &refined,
            manual_pose,
        )
    }));
    reports
}

fn run_shape_band_scan(
    local: &XrDepthAlignDescriptor,
    remote: &XrDepthAlignDescriptor,
    manual_pose: Option<ManualPose>,
) -> Vec<ShapeBandScanResult> {
    let bands = shape_band_scans();
    if bands.is_empty() {
        return Vec::new();
    }
    let worker_count = analysis_thread_count(bands.len());
    let chunk_size = bands.len().div_ceil(worker_count);
    std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for band_chunk in bands.chunks(chunk_size.max(1)) {
            let band_chunk = band_chunk.to_vec();
            handles.push(scope.spawn(move || {
                let mut results = Vec::new();
                for band in band_chunk {
                    let Some(report) = run_shape_match_analysis(local, remote, manual_pose, &band)
                    else {
                        continue;
                    };
                    let nearest_manual_finalist = manual_pose.and_then(|manual_pose| {
                        nearest_manual_finalist(&report.finalists, manual_pose)
                    });
                    results.push(ShapeBandScanResult {
                        report,
                        nearest_manual_finalist,
                    });
                }
                results
            }));
        }
        let mut results = Vec::new();
        for handle in handles {
            results.extend(handle.join().expect("shape-scan worker should not panic"));
        }
        results
    })
}

fn run_fixed_overlap_band_analysis(
    local: &XrDepthAlignDescriptor,
    remote: &XrDepthAlignDescriptor,
    manual_pose: Option<ManualPose>,
    band: &ShapeBandSpec,
) -> Option<ShapeMatchReport> {
    let (Some(local_map), Some(remote_map)) =
        (local.height_map.as_ref(), remote.height_map.as_ref())
    else {
        return None;
    };
    let local_global = build_shape_match_grid(local_map, SHAPE_GLOBAL_CELL_METERS, band)?;
    let remote_global = build_shape_match_grid(remote_map, SHAPE_GLOBAL_CELL_METERS, band)?;
    let global = search_mask_overlap_global(&local_global, &remote_global, SHAPE_GLOBAL_TOP_K);
    if global.is_empty() {
        return None;
    }
    let local_final = build_shape_match_grid(local_map, SHAPE_FINAL_CELL_METERS, band)?;
    let remote_final = build_shape_match_grid(remote_map, SHAPE_FINAL_CELL_METERS, band)?;
    let finalists = search_mask_overlap_refine(
        &local_final,
        &remote_final,
        &global,
        0.36,
        SHAPE_REFINE_YAW_WINDOW_RADIANS,
        SHAPE_REFINE_YAW_STEP_RADIANS,
        SHAPE_FINAL_TOP_K,
    );
    if finalists.is_empty() {
        return None;
    }
    let manual_candidate = manual_pose.map(|manual_pose| {
        score_mask_overlap_symmetric(
            &local_final,
            &remote_final,
            manual_pose.rotation_radians,
            manual_pose.shift_x_meters,
            manual_pose.shift_y_meters,
        )
    });
    Some(ShapeMatchReport {
        label: band.label.clone(),
        finalists,
        manual_candidate,
    })
}

fn run_seeded_overlap_band_analysis(
    local: &XrDepthAlignDescriptor,
    remote: &XrDepthAlignDescriptor,
    band: &ShapeBandSpec,
    seed: XrDepthAlignSolution,
) -> Option<ShapeMatchReport> {
    let (Some(local_map), Some(remote_map)) =
        (local.height_map.as_ref(), remote.height_map.as_ref())
    else {
        return None;
    };
    let local_final = build_shape_match_grid(local_map, SHAPE_FINAL_CELL_METERS, band)?;
    let remote_final = build_shape_match_grid(remote_map, SHAPE_FINAL_CELL_METERS, band)?;
    let seed_candidate = ShapeMatchCandidate {
        yaw_radians: seed.yaw_radians,
        translation_x: seed.translation.x,
        translation_z: seed.translation.z,
        ..ShapeMatchCandidate::default()
    };
    let finalists = search_mask_overlap_refine(
        &local_final,
        &remote_final,
        &[seed_candidate],
        0.36,
        10.0_f32.to_radians(),
        1.0_f32.to_radians(),
        SHAPE_FINAL_TOP_K,
    );
    if finalists.is_empty() {
        return None;
    }
    Some(ShapeMatchReport {
        label: format!("{}_seeded", band.label),
        finalists,
        manual_candidate: None,
    })
}

fn gather_seed_candidates(
    diagnostic: &XrDepthAlignSolveDiagnostic,
    reports: &[&ShapeMatchReport],
) -> Vec<ShapeMatchCandidate> {
    let mut seeds = Vec::new();
    if let Some(best) = diagnostic.best_solution {
        push_shape_candidate(
            &mut seeds,
            ShapeMatchCandidate {
                yaw_radians: best.yaw_radians,
                translation_x: best.translation.x,
                translation_z: best.translation.z,
                score: best.confidence.max(0.001),
                ..ShapeMatchCandidate::default()
            },
            8,
            0.06,
            1.0_f32.to_radians(),
        );
    }
    for report in reports {
        for candidate in report.finalists.iter().take(2) {
            push_shape_candidate(&mut seeds, *candidate, 8, 0.06, 1.0_f32.to_radians());
        }
    }
    seeds
}

fn run_seeded_signed_overlap_band_analysis(
    local: &XrDepthAlignDescriptor,
    remote: &XrDepthAlignDescriptor,
    manual_pose: Option<ManualPose>,
    band: &ShapeBandSpec,
    seeds: &[ShapeMatchCandidate],
) -> Option<ShapeMatchReport> {
    if seeds.is_empty() {
        return None;
    }
    let (Some(local_map), Some(remote_map)) =
        (local.height_map.as_ref(), remote.height_map.as_ref())
    else {
        return None;
    };
    let local_final = build_shape_match_grid(local_map, SHAPE_FINAL_CELL_METERS, band)?;
    let remote_final = build_shape_match_grid(remote_map, SHAPE_FINAL_CELL_METERS, band)?;
    let mut finalists = search_signed_overlap_seeded(
        &local_final,
        &remote_final,
        seeds,
        SHAPE_SEEDED_LOCAL_TRANSLATION_WINDOW_METERS,
        SHAPE_SEEDED_LOCAL_YAW_WINDOW_RADIANS,
        SHAPE_SEEDED_LOCAL_YAW_STEP_RADIANS,
        SHAPE_FINAL_TOP_K,
    );
    if finalists.is_empty() {
        return None;
    }
    let manual_candidate = manual_pose.map(|manual_pose| {
        let manual_candidate = ShapeMatchCandidate {
            yaw_radians: manual_pose.rotation_radians,
            translation_x: manual_pose.shift_x_meters,
            translation_z: manual_pose.shift_y_meters,
            ..ShapeMatchCandidate::default()
        };
        let nearest_seed = seeds
            .iter()
            .copied()
            .min_by(|left, right| {
                let left_delta = ((left.translation_x - manual_candidate.translation_x).powi(2)
                    + (left.translation_z - manual_candidate.translation_z).powi(2))
                .sqrt()
                    + wrap_angle(left.yaw_radians - manual_candidate.yaw_radians).abs() * 0.35;
                let right_delta = ((right.translation_x - manual_candidate.translation_x).powi(2)
                    + (right.translation_z - manual_candidate.translation_z).powi(2))
                .sqrt()
                    + wrap_angle(right.yaw_radians - manual_candidate.yaw_radians).abs() * 0.35;
                left_delta.total_cmp(&right_delta)
            })
            .unwrap_or(manual_candidate);
        apply_seed_prior(
            score_signed_overlap_symmetric(
                &local_final,
                &remote_final,
                manual_pose.rotation_radians,
                manual_pose.shift_x_meters,
                manual_pose.shift_y_meters,
            ),
            nearest_seed,
        )
    });

    Some(ShapeMatchReport {
        label: format!("{}_signed_seeded", band.label),
        finalists,
        manual_candidate,
    })
}

fn run_seeded_signed_support_band_analysis(
    local: &XrDepthAlignDescriptor,
    remote: &XrDepthAlignDescriptor,
    manual_pose: Option<ManualPose>,
    band: &ShapeBandSpec,
    seeds: &[ShapeMatchCandidate],
) -> Option<ShapeMatchReport> {
    if seeds.is_empty() {
        return None;
    }
    let (Some(local_map), Some(remote_map)) =
        (local.height_map.as_ref(), remote.height_map.as_ref())
    else {
        return None;
    };
    let local_final = build_shape_match_grid(local_map, SHAPE_FINAL_CELL_METERS, band)?;
    let remote_final = build_shape_match_grid(remote_map, SHAPE_FINAL_CELL_METERS, band)?;
    let finalists = search_signed_support_seeded(
        &local_final,
        &remote_final,
        seeds,
        SHAPE_SEEDED_LOCAL_TRANSLATION_WINDOW_METERS,
        SHAPE_SEEDED_LOCAL_YAW_WINDOW_RADIANS,
        SHAPE_SEEDED_LOCAL_YAW_STEP_RADIANS,
        SHAPE_FINAL_TOP_K,
    );
    if finalists.is_empty() {
        return None;
    }
    let manual_candidate = manual_pose.map(|manual_pose| {
        let manual_candidate = ShapeMatchCandidate {
            yaw_radians: manual_pose.rotation_radians,
            translation_x: manual_pose.shift_x_meters,
            translation_z: manual_pose.shift_y_meters,
            ..ShapeMatchCandidate::default()
        };
        let nearest_seed = seeds
            .iter()
            .copied()
            .min_by(|left, right| {
                let left_delta = ((left.translation_x - manual_candidate.translation_x).powi(2)
                    + (left.translation_z - manual_candidate.translation_z).powi(2))
                .sqrt()
                    + wrap_angle(left.yaw_radians - manual_candidate.yaw_radians).abs() * 0.35;
                let right_delta = ((right.translation_x - manual_candidate.translation_x).powi(2)
                    + (right.translation_z - manual_candidate.translation_z).powi(2))
                .sqrt()
                    + wrap_angle(right.yaw_radians - manual_candidate.yaw_radians).abs() * 0.35;
                left_delta.total_cmp(&right_delta)
            })
            .unwrap_or(manual_candidate);
        apply_seed_prior(
            score_signed_support_symmetric(
                &local_final,
                &remote_final,
                manual_pose.rotation_radians,
                manual_pose.shift_x_meters,
                manual_pose.shift_y_meters,
            ),
            nearest_seed,
        )
    });
    Some(ShapeMatchReport {
        label: format!("{}_signed_support_seeded", band.label),
        finalists,
        manual_candidate,
    })
}

fn build_seeded_consensus_band_grids(
    local: &XrDepthAlignDescriptor,
    remote: &XrDepthAlignDescriptor,
) -> Vec<BandGridPair> {
    build_consensus_band_grids_at_cell(local, remote, SHAPE_FINAL_CELL_METERS)
}

fn build_consensus_band_grids_at_cell(
    local: &XrDepthAlignDescriptor,
    remote: &XrDepthAlignDescriptor,
    cell_size_meters: f32,
) -> Vec<BandGridPair> {
    let (Some(local_map), Some(remote_map)) =
        (local.height_map.as_ref(), remote.height_map.as_ref())
    else {
        return Vec::new();
    };
    seeded_consensus_bands()
        .into_iter()
        .filter_map(|band| {
            let local_grid = build_shape_match_grid(local_map, cell_size_meters, &band)?;
            let remote_grid = build_shape_match_grid(remote_map, cell_size_meters, &band)?;
            Some(BandGridPair {
                local: local_grid,
                remote: remote_grid,
            })
        })
        .collect()
}

fn score_consensus_candidate(
    candidate: ShapeMatchCandidate,
    bands: &[BandGridPair],
) -> ShapeMatchCandidate {
    let mut scores = Vec::new();
    let mut coverage_sum = 0.0;
    let mut close_sum = 0.0;
    let mut dist_sum = 0.0;
    let mut points_sum = 0usize;
    for band in bands {
        let score = score_mask_overlap_symmetric(
            &band.local,
            &band.remote,
            candidate.yaw_radians,
            candidate.translation_x,
            candidate.translation_z,
        );
        scores.push(score.score);
        coverage_sum += score.coverage;
        close_sum += score.close_ratio;
        dist_sum += score.mean_distance_meters;
        points_sum += score.in_bounds_points;
    }
    if scores.is_empty() {
        return ShapeMatchCandidate::default();
    }

    // Evaluate pure floor-plan Support IoU to mathematically break symmetric wall mirror aliases.
    // The Flipped Room fills the unobserved "gap" of the true room, maximizing overlap but ALSO massively inflating the Union.
    let mut iou_score = 1.0f32;
    if let Some(longest_band) = bands.iter().max_by_key(|b| b.local.support_points.len()) {
        iou_score = score_support_iou_symmetric(
            &longest_band.local,
            &longest_band.remote,
            candidate.yaw_radians,
            candidate.translation_x,
            candidate.translation_z,
        );
    }

    let avg_score = scores.iter().copied().sum::<f32>() / scores.len() as f32;
    let min_score = scores.iter().copied().fold(f32::INFINITY, f32::min);

    // IOU is a huge separator for identical overlapping walls that are structurally flipped.
    let raw_score = (avg_score * 0.72 + min_score * 0.28) * iou_score.sqrt();

    ShapeMatchCandidate {
        score: raw_score,
        feature_score: avg_score,
        support_score: iou_score,
        coverage: coverage_sum / scores.len() as f32,
        close_ratio: close_sum / scores.len() as f32,
        mean_distance_meters: dist_sum / scores.len() as f32,
        yaw_radians: candidate.yaw_radians,
        translation_x: candidate.translation_x,
        translation_z: candidate.translation_z,
        in_bounds_points: points_sum / scores.len(),
    }
}

fn representative_band<'a>(bands: &'a [BandGridPair]) -> Option<&'a BandGridPair> {
    bands
        .iter()
        .max_by_key(|band| band.local.occupied_points.len())
}

fn search_consensus_global(
    bands: &[BandGridPair],
    yaw_step_radians: f32,
    top_k: usize,
) -> Vec<ShapeMatchCandidate> {
    let Some(representative) = representative_band(bands) else {
        return Vec::new();
    };
    let local = &representative.local;
    let remote = &representative.remote;
    let local_min_x = local.origin_x;
    let local_max_x = local.max_x();
    let local_min_z = local.origin_z;
    let local_max_z = local.max_z();
    let mut finalists = Vec::new();
    let mut yaw = -PI;
    while yaw < PI {
        let Some((remote_min_x, remote_max_x, remote_min_z, remote_max_z)) =
            rotated_bounds(&remote.occupied_points, yaw)
        else {
            yaw += yaw_step_radians;
            continue;
        };
        let translation_min_x = local_min_x - remote_max_x;
        let translation_max_x = local_max_x - remote_min_x;
        let translation_min_z = local_min_z - remote_max_z;
        let translation_max_z = local_max_z - remote_min_z;
        let step = local.cell_size_meters.max(1.0e-3);
        let steps_x = ((translation_max_x - translation_min_x) / step)
            .ceil()
            .max(0.0) as usize;
        let steps_z = ((translation_max_z - translation_min_z) / step)
            .ceil()
            .max(0.0) as usize;
        for x_step in 0..=steps_x {
            let translation_x = translation_min_x + x_step as f32 * step;
            for z_step in 0..=steps_z {
                let translation_z = translation_min_z + z_step as f32 * step;
                let candidate = score_consensus_candidate(
                    ShapeMatchCandidate {
                        yaw_radians: yaw,
                        translation_x,
                        translation_z,
                        ..ShapeMatchCandidate::default()
                    },
                    bands,
                );
                push_shape_candidate(
                    &mut finalists,
                    candidate,
                    top_k,
                    step * 1.5,
                    yaw_step_radians * 1.25,
                );
            }
        }
        yaw += yaw_step_radians;
    }
    finalists
}

fn search_consensus_refine(
    bands: &[BandGridPair],
    seeds: &[ShapeMatchCandidate],
    translation_window_meters: f32,
    yaw_window_radians: f32,
    yaw_step_radians: f32,
    top_k: usize,
) -> Vec<ShapeMatchCandidate> {
    let Some(representative) = representative_band(bands) else {
        return Vec::new();
    };
    let mut finalists = Vec::new();
    let step = representative.local.cell_size_meters.max(1.0e-3);
    let translation_steps = (translation_window_meters / step).ceil().max(1.0) as isize;
    let yaw_steps = (yaw_window_radians / yaw_step_radians).ceil().max(1.0) as isize;
    for seed in seeds {
        for yaw_step in -yaw_steps..=yaw_steps {
            let yaw_radians = wrap_angle(seed.yaw_radians + yaw_step as f32 * yaw_step_radians);
            for x_step in -translation_steps..=translation_steps {
                let translation_x = seed.translation_x + x_step as f32 * step;
                for z_step in -translation_steps..=translation_steps {
                    let translation_z = seed.translation_z + z_step as f32 * step;
                    let candidate = score_consensus_candidate(
                        ShapeMatchCandidate {
                            yaw_radians,
                            translation_x,
                            translation_z,
                            ..ShapeMatchCandidate::default()
                        },
                        bands,
                    );
                    push_shape_candidate(
                        &mut finalists,
                        candidate,
                        top_k,
                        step * 1.25,
                        yaw_step_radians * 1.25,
                    );
                }
            }
        }
    }
    finalists
}

fn run_seeded_multiband_consensus_analysis(
    local: &XrDepthAlignDescriptor,
    remote: &XrDepthAlignDescriptor,
    manual_pose: Option<ManualPose>,
    seed: XrDepthAlignSolution,
) -> Option<ShapeMatchReport> {
    let bands = build_seeded_consensus_band_grids(local, remote);
    if bands.is_empty() {
        return None;
    }

    let seed_candidate = ShapeMatchCandidate {
        yaw_radians: seed.yaw_radians,
        translation_x: seed.translation.x,
        translation_z: seed.translation.z,
        ..ShapeMatchCandidate::default()
    };

    let mut pooled = Vec::<ShapeMatchCandidate>::new();
    for band in &bands {
        let finalists = search_mask_overlap_translation_only(
            &band.local,
            &band.remote,
            seed_candidate,
            0.36,
            4,
        );
        for candidate in finalists {
            push_shape_candidate(
                &mut pooled,
                candidate,
                12,
                band.local.cell_size_meters * 1.25,
                0.02,
            );
        }
    }
    if pooled.is_empty() {
        return None;
    }

    let mut finalists = pooled
        .into_iter()
        .map(|candidate| score_consensus_candidate(candidate, &bands))
        .filter(|candidate| candidate.score.is_finite() && candidate.score > 0.0)
        .collect::<Vec<_>>();
    finalists.sort_by(|left, right| {
        right.score.total_cmp(&left.score).then_with(|| {
            left.mean_distance_meters
                .total_cmp(&right.mean_distance_meters)
        })
    });
    finalists.truncate(SHAPE_FINAL_TOP_K);
    if finalists.is_empty() {
        return None;
    }

    let manual_candidate = manual_pose.map(|manual_pose| {
        score_consensus_candidate(
            ShapeMatchCandidate {
                yaw_radians: manual_pose.rotation_radians,
                translation_x: manual_pose.shift_x_meters,
                translation_z: manual_pose.shift_y_meters,
                ..ShapeMatchCandidate::default()
            },
            &bands,
        )
    });

    Some(ShapeMatchReport {
        label: "shape_multiband_seeded".to_string(),
        finalists,
        manual_candidate,
    })
}

fn run_runtime_seeded_consensus_refine_analysis(
    local: &XrDepthAlignDescriptor,
    remote: &XrDepthAlignDescriptor,
    manual_pose: Option<ManualPose>,
    seed: XrDepthAlignSolution,
) -> Option<ShapeMatchReport> {
    let bands = build_seeded_consensus_band_grids(local, remote);
    if bands.is_empty() {
        return None;
    }

    let seed_candidate = ShapeMatchCandidate {
        yaw_radians: seed.yaw_radians,
        translation_x: seed.translation.x,
        translation_z: seed.translation.z,
        ..ShapeMatchCandidate::default()
    };
    let rescore_local_candidate = |candidate: ShapeMatchCandidate| {
        let mut rescored = score_consensus_candidate(candidate, &bands);
        // The runtime-seeded local experiment should not get zeroed out solely by the
        // harsh global Support-IoU term. Keep some feature score alive locally.
        rescored.score = rescored.score.max(rescored.feature_score * 0.35);
        apply_seed_prior(rescored, seed_candidate)
    };

    let mut pooled = Vec::<ShapeMatchCandidate>::new();
    for band in &bands {
        let finalists = search_mask_overlap_translation_only(
            &band.local,
            &band.remote,
            seed_candidate,
            0.28,
            6,
        );
        for candidate in finalists {
            push_shape_candidate(
                &mut pooled,
                candidate,
                16,
                band.local.cell_size_meters * 1.25,
                0.02,
            );
        }
    }
    if pooled.is_empty() {
        return None;
    }

    let mut finalists = pooled
        .into_iter()
        .map(&rescore_local_candidate)
        .filter(|candidate| candidate.score.is_finite() && candidate.score > 0.0)
        .collect::<Vec<_>>();
    finalists.sort_by(|left, right| {
        right.score.total_cmp(&left.score).then_with(|| {
            left.mean_distance_meters
                .total_cmp(&right.mean_distance_meters)
        })
    });
    finalists.truncate(6);
    if finalists.is_empty() {
        return None;
    }

    let mut refined = search_consensus_refine(
        &bands,
        &finalists,
        0.18,
        10.0_f32.to_radians(),
        1.0_f32.to_radians(),
        SHAPE_FINAL_TOP_K,
    )
    .into_iter()
    .map(&rescore_local_candidate)
    .collect::<Vec<_>>();
    if refined.is_empty() {
        refined = finalists;
    } else {
        refined.sort_by(|left, right| {
            right.score.total_cmp(&left.score).then_with(|| {
                left.mean_distance_meters
                    .total_cmp(&right.mean_distance_meters)
            })
        });
        refined.truncate(SHAPE_FINAL_TOP_K);
    }

    let manual_candidate = manual_pose.map(|manual_pose| {
        rescore_local_candidate(ShapeMatchCandidate {
            yaw_radians: manual_pose.rotation_radians,
            translation_x: manual_pose.shift_x_meters,
            translation_z: manual_pose.shift_y_meters,
            ..ShapeMatchCandidate::default()
        })
    });

    Some(ShapeMatchReport {
        label: "runtime_seeded_consensus".to_string(),
        finalists: refined,
        manual_candidate,
    })
}

fn run_seeded_multiband_vote_analysis(
    local: &XrDepthAlignDescriptor,
    remote: &XrDepthAlignDescriptor,
    manual_pose: Option<ManualPose>,
    seed: XrDepthAlignSolution,
) -> Option<ShapeMatchReport> {
    let bands = build_seeded_consensus_band_grids(local, remote);
    if bands.is_empty() {
        return None;
    }

    let seed_candidate = ShapeMatchCandidate {
        yaw_radians: seed.yaw_radians,
        translation_x: seed.translation.x,
        translation_z: seed.translation.z,
        ..ShapeMatchCandidate::default()
    };

    let mut weighted_sum_x = 0.0;
    let mut weighted_sum_z = 0.0;
    let mut weight_sum = 0.0;
    let mut candidates = Vec::new();
    for band in &bands {
        let finalists = search_mask_overlap_translation_only(
            &band.local,
            &band.remote,
            seed_candidate,
            0.36,
            3,
        );
        if let Some(best) = finalists.first().copied() {
            let weight = best.score.max(0.001);
            weighted_sum_x += best.translation_x * weight;
            weighted_sum_z += best.translation_z * weight;
            weight_sum += weight;
            candidates.push(best);
        }
    }
    if candidates.is_empty() || weight_sum <= 1.0e-6 {
        return None;
    }

    candidates.push(ShapeMatchCandidate {
        yaw_radians: seed.yaw_radians,
        translation_x: weighted_sum_x / weight_sum,
        translation_z: weighted_sum_z / weight_sum,
        ..ShapeMatchCandidate::default()
    });
    candidates.push(seed_candidate);

    let mut finalists = candidates
        .into_iter()
        .map(|candidate| score_consensus_candidate(candidate, &bands))
        .collect::<Vec<_>>();
    finalists.sort_by(|left, right| {
        right.score.total_cmp(&left.score).then_with(|| {
            left.mean_distance_meters
                .total_cmp(&right.mean_distance_meters)
        })
    });
    finalists.truncate(SHAPE_FINAL_TOP_K);
    let manual_candidate = manual_pose.map(|manual_pose| {
        score_consensus_candidate(
            ShapeMatchCandidate {
                yaw_radians: manual_pose.rotation_radians,
                translation_x: manual_pose.shift_x_meters,
                translation_z: manual_pose.shift_y_meters,
                ..ShapeMatchCandidate::default()
            },
            &bands,
        )
    });
    Some(ShapeMatchReport {
        label: "shape_multiband_vote_seeded".to_string(),
        finalists,
        manual_candidate,
    })
}

fn run_correlative_consensus_analysis(
    local: &XrDepthAlignDescriptor,
    remote: &XrDepthAlignDescriptor,
    manual_pose: Option<ManualPose>,
) -> Option<ShapeMatchReport> {
    let coarse_bands =
        build_consensus_band_grids_at_cell(local, remote, SHAPE_CORR_COARSE_CELL_METERS);
    if coarse_bands.is_empty() {
        return None;
    }
    let coarse = search_consensus_global(
        &coarse_bands,
        SHAPE_CORR_COARSE_YAW_STEP_RADIANS,
        SHAPE_CORR_COARSE_TOP_K,
    );
    if coarse.is_empty() {
        return None;
    }

    let medium_bands =
        build_consensus_band_grids_at_cell(local, remote, SHAPE_CORR_MEDIUM_CELL_METERS);
    if medium_bands.is_empty() {
        return None;
    }
    let medium = search_consensus_refine(
        &medium_bands,
        &coarse,
        SHAPE_CORR_COARSE_CELL_METERS * 1.5,
        SHAPE_CORR_COARSE_YAW_STEP_RADIANS * 1.5,
        SHAPE_CORR_MEDIUM_YAW_STEP_RADIANS,
        SHAPE_CORR_MEDIUM_TOP_K,
    );
    if medium.is_empty() {
        return None;
    }

    let fine_bands = build_consensus_band_grids_at_cell(local, remote, SHAPE_CORR_FINE_CELL_METERS);
    if fine_bands.is_empty() {
        return None;
    }
    let fine = search_consensus_refine(
        &fine_bands,
        &medium,
        SHAPE_CORR_MEDIUM_CELL_METERS * 1.25,
        SHAPE_CORR_MEDIUM_YAW_STEP_RADIANS * 1.5,
        SHAPE_CORR_FINE_YAW_STEP_RADIANS,
        SHAPE_CORR_FINE_TOP_K,
    );
    if fine.is_empty() {
        return None;
    }

    let final_bands =
        build_consensus_band_grids_at_cell(local, remote, SHAPE_CORR_FINAL_CELL_METERS);
    if final_bands.is_empty() {
        return None;
    }
    let mut finalists = search_consensus_refine(
        &final_bands,
        &fine,
        SHAPE_CORR_FINE_CELL_METERS * 1.0,
        SHAPE_CORR_FINE_YAW_STEP_RADIANS * 1.5,
        SHAPE_CORR_FINAL_YAW_STEP_RADIANS,
        SHAPE_CORR_FINAL_TOP_K,
    );
    if finalists.is_empty() {
        return None;
    }

    let manual_candidate = manual_pose.map(|manual_pose| {
        score_consensus_candidate(
            ShapeMatchCandidate {
                yaw_radians: manual_pose.rotation_radians,
                translation_x: manual_pose.shift_x_meters,
                translation_z: manual_pose.shift_y_meters,
                ..ShapeMatchCandidate::default()
            },
            &final_bands,
        )
    });
    Some(ShapeMatchReport {
        label: "shape_correlative_consensus".to_string(),
        finalists,
        manual_candidate,
    })
}

fn generate_component_pair_seeds(
    local: &ShapeMatchGrid,
    remote: &ShapeMatchGrid,
) -> Vec<ShapeMatchCandidate> {
    let local_components = extract_shape_components(local, 6);
    let remote_components = extract_shape_components(remote, 6);
    let mut seeds = Vec::new();

    for local_component in &local_components {
        for remote_component in &remote_components {
            let area_ratio = (local_component.area_m2 / remote_component.area_m2.max(1.0e-4))
                .max(remote_component.area_m2 / local_component.area_m2.max(1.0e-4));
            if area_ratio > 2.8 {
                continue;
            }
            push_shape_candidate(
                &mut seeds,
                ShapeMatchCandidate {
                    yaw_radians: 0.0,
                    translation_x: local_component.centroid_x - remote_component.centroid_x,
                    translation_z: local_component.centroid_z - remote_component.centroid_z,
                    score: local_component.score.min(remote_component.score),
                    ..ShapeMatchCandidate::default()
                },
                24,
                0.12,
                8.0_f32.to_radians(),
            );
        }
    }

    for local_i in 0..local_components.len() {
        for local_j in local_i + 1..local_components.len() {
            let local_dx =
                local_components[local_j].centroid_x - local_components[local_i].centroid_x;
            let local_dz =
                local_components[local_j].centroid_z - local_components[local_i].centroid_z;
            let local_dist = (local_dx * local_dx + local_dz * local_dz).sqrt();
            if local_dist < 0.18 {
                continue;
            }
            let local_angle = local_dz.atan2(local_dx);
            for remote_i in 0..remote_components.len() {
                for remote_j in remote_i + 1..remote_components.len() {
                    let remote_dx = remote_components[remote_j].centroid_x
                        - remote_components[remote_i].centroid_x;
                    let remote_dz = remote_components[remote_j].centroid_z
                        - remote_components[remote_i].centroid_z;
                    let remote_dist = (remote_dx * remote_dx + remote_dz * remote_dz).sqrt();
                    if remote_dist < 0.18 {
                        continue;
                    }
                    let distance_ratio = (local_dist / remote_dist.max(1.0e-4))
                        .max(remote_dist / local_dist.max(1.0e-4));
                    if distance_ratio > 1.8 {
                        continue;
                    }
                    let area_ratio_a = (local_components[local_i].area_m2
                        / remote_components[remote_i].area_m2.max(1.0e-4))
                    .max(
                        remote_components[remote_i].area_m2
                            / local_components[local_i].area_m2.max(1.0e-4),
                    );
                    let area_ratio_b = (local_components[local_j].area_m2
                        / remote_components[remote_j].area_m2.max(1.0e-4))
                    .max(
                        remote_components[remote_j].area_m2
                            / local_components[local_j].area_m2.max(1.0e-4),
                    );
                    if area_ratio_a > 2.8 || area_ratio_b > 2.8 {
                        continue;
                    }
                    let remote_angle = remote_dz.atan2(remote_dx);
                    let yaw_radians = wrap_angle(local_angle - remote_angle);
                    let (rotated_x, rotated_z) = rotate_xz(
                        yaw_radians,
                        remote_components[remote_i].centroid_x,
                        remote_components[remote_i].centroid_z,
                    );
                    let translation_x = local_components[local_i].centroid_x - rotated_x;
                    let translation_z = local_components[local_i].centroid_z - rotated_z;
                    let pair_score = local_components[local_i]
                        .score
                        .min(remote_components[remote_i].score)
                        + local_components[local_j]
                            .score
                            .min(remote_components[remote_j].score);
                    push_shape_candidate(
                        &mut seeds,
                        ShapeMatchCandidate {
                            yaw_radians,
                            translation_x,
                            translation_z,
                            score: pair_score,
                            ..ShapeMatchCandidate::default()
                        },
                        36,
                        0.12,
                        6.0_f32.to_radians(),
                    );
                }
            }
        }
    }

    seeds
}

fn component_signature_match(
    local: ShapeComponent,
    remote: ShapeComponent,
) -> Option<(usize, f32)> {
    let area_ratio = (local.area_m2 / remote.area_m2.max(1.0e-4))
        .max(remote.area_m2 / local.area_m2.max(1.0e-4));
    if area_ratio > 3.2 {
        return None;
    }
    let compactness_delta = (local.compactness - remote.compactness).abs();
    if compactness_delta > 0.65 {
        return None;
    }
    let mut best_shift = 0usize;
    let mut best_error = f32::INFINITY;
    for shift in 0..SHAPE_COMPONENT_SIGNATURE_BINS {
        let mut error = 0.0f32;
        for i in 0..SHAPE_COMPONENT_SIGNATURE_BINS {
            let j = (i + shift) % SHAPE_COMPONENT_SIGNATURE_BINS;
            error += (local.radial_signature[i] - remote.radial_signature[j]).abs();
        }
        if error < best_error {
            best_error = error;
            best_shift = shift;
        }
    }
    let signature_error = best_error / SHAPE_COMPONENT_SIGNATURE_BINS as f32;
    if signature_error > 0.42 {
        return None;
    }
    let quality = 1.0
        - ((area_ratio - 1.0) * 0.35 + compactness_delta * 0.45 + signature_error * 1.35)
            .clamp(0.0, 0.98);
    Some((best_shift, quality.max(0.001)))
}

fn component_context_signature_error(
    local: &ShapeComponent,
    remote: &ShapeComponent,
    shift: usize,
) -> f32 {
    let channel_weights = [1.0f32, 0.8f32, 0.6f32];
    let mut weighted_error = 0.0f32;
    let mut total_weight = 0.0f32;
    for (channel, channel_weight) in channel_weights.into_iter().enumerate() {
        for radial in 0..SHAPE_COMPONENT_CONTEXT_RADIAL_BINS {
            for angular in 0..SHAPE_COMPONENT_CONTEXT_ANGULAR_BINS {
                let local_index = component_context_index(channel, radial, angular);
                let remote_index = component_context_index(
                    channel,
                    radial,
                    (angular + shift) % SHAPE_COMPONENT_CONTEXT_ANGULAR_BINS,
                );
                weighted_error += channel_weight
                    * (local.context_signature[local_index]
                        - remote.context_signature[remote_index])
                        .abs();
                total_weight += channel_weight;
            }
        }
    }
    weighted_error / total_weight.max(1.0e-6)
}

fn component_context_match(local: ShapeComponent, remote: ShapeComponent) -> Option<(usize, f32)> {
    let area_ratio = (local.area_m2 / remote.area_m2.max(1.0e-4))
        .max(remote.area_m2 / local.area_m2.max(1.0e-4));
    if area_ratio > 3.4 {
        return None;
    }
    let compactness_delta = (local.compactness - remote.compactness).abs();
    if compactness_delta > 0.75 {
        return None;
    }
    let mut best_shift = 0usize;
    let mut best_blob_error = f32::INFINITY;
    let mut best_context_error = f32::INFINITY;
    let mut best_total_error = f32::INFINITY;
    for shift in 0..SHAPE_COMPONENT_CONTEXT_ANGULAR_BINS {
        let mut blob_error = 0.0f32;
        for i in 0..SHAPE_COMPONENT_SIGNATURE_BINS {
            let j = (i + shift) % SHAPE_COMPONENT_SIGNATURE_BINS;
            blob_error += (local.radial_signature[i] - remote.radial_signature[j]).abs();
        }
        blob_error /= SHAPE_COMPONENT_SIGNATURE_BINS as f32;
        let context_error = component_context_signature_error(&local, &remote, shift);
        let total_error = blob_error * 0.45 + context_error * 0.85;
        if total_error < best_total_error {
            best_total_error = total_error;
            best_blob_error = blob_error;
            best_context_error = context_error;
            best_shift = shift;
        }
    }
    if best_blob_error > 0.55 || best_context_error > 0.16 {
        return None;
    }
    let quality = 1.0
        - ((area_ratio - 1.0) * 0.22
            + compactness_delta * 0.25
            + best_blob_error * 0.60
            + best_context_error * 3.25)
            .clamp(0.0, 0.985);
    Some((best_shift, quality.max(0.001)))
}

fn component_radial_signature_error(
    local: &ShapeComponent,
    remote: &ShapeComponent,
    shift: usize,
) -> f32 {
    let mut error = 0.0f32;
    for index in 0..SHAPE_COMPONENT_SIGNATURE_BINS {
        let remote_index = (index + shift) % SHAPE_COMPONENT_SIGNATURE_BINS;
        error += (local.radial_signature[index] - remote.radial_signature[remote_index]).abs();
    }
    error / SHAPE_COMPONENT_SIGNATURE_BINS as f32
}

fn component_pair_alignment_score(
    local: &ShapeComponent,
    remote: &ShapeComponent,
    yaw_radians: f32,
    translation_x: f32,
    translation_z: f32,
) -> Option<f32> {
    let (rotated_x, rotated_z) = rotate_xz(yaw_radians, remote.centroid_x, remote.centroid_z);
    let planar_distance = ((local.centroid_x - (rotated_x + translation_x)).powi(2)
        + (local.centroid_z - (rotated_z + translation_z)).powi(2))
    .sqrt();
    if planar_distance > FOCUSED_COMPONENT_MAX_DISTANCE_METERS {
        return None;
    }
    let area_ratio = (local.area_m2 / remote.area_m2.max(1.0e-4))
        .max(remote.area_m2 / local.area_m2.max(1.0e-4));
    if area_ratio > 3.6 {
        return None;
    }
    let compactness_delta = (local.compactness - remote.compactness).abs();
    if compactness_delta > 0.80 {
        return None;
    }
    let base_sig_shift = signature_shift_for_angle(yaw_radians, SHAPE_COMPONENT_SIGNATURE_BINS);
    let base_ctx_shift =
        signature_shift_for_angle(yaw_radians, SHAPE_COMPONENT_CONTEXT_ANGULAR_BINS);
    let mut best_signature_error = f32::INFINITY;
    let mut best_context_error = f32::INFINITY;
    for sig_delta in [-1isize, 0, 1] {
        let signature_shift = (base_sig_shift as isize + sig_delta)
            .rem_euclid(SHAPE_COMPONENT_SIGNATURE_BINS as isize)
            as usize;
        let signature_error = component_radial_signature_error(local, remote, signature_shift);
        let ctx_delta = if sig_delta < 0 {
            -1
        } else if sig_delta > 0 {
            1
        } else {
            0
        };
        let context_shift = (base_ctx_shift as isize + ctx_delta)
            .rem_euclid(SHAPE_COMPONENT_CONTEXT_ANGULAR_BINS as isize)
            as usize;
        let context_error = component_context_signature_error(local, remote, context_shift);
        if signature_error + context_error * 0.8 < best_signature_error + best_context_error * 0.8 {
            best_signature_error = signature_error;
            best_context_error = context_error;
        }
    }
    if best_signature_error > 0.70 || best_context_error > 0.22 {
        return None;
    }
    let spatial_score =
        (-0.5 * (planar_distance / FOCUSED_COMPONENT_DISTANCE_SIGMA_METERS).powi(2)).exp();
    let shape_score = 1.0
        - ((area_ratio - 1.0) * 0.18
            + compactness_delta * 0.18
            + best_signature_error * 0.55
            + best_context_error * 2.70)
            .clamp(0.0, 0.985);
    let component_weight = local.score.min(remote.score).sqrt().max(0.05);
    Some((spatial_score * shape_score.max(0.0) * component_weight).clamp(0.0, 1.0))
}

fn score_component_alignment_one_way(
    local_components: &[ShapeComponent],
    remote_components: &[ShapeComponent],
    yaw_radians: f32,
    translation_x: f32,
    translation_z: f32,
) -> Option<f32> {
    if local_components.is_empty() || remote_components.is_empty() {
        return None;
    }
    #[derive(Clone, Copy)]
    struct PairCandidate {
        local_index: usize,
        remote_index: usize,
        score: f32,
        weight: f32,
    }
    let mut pair_candidates = Vec::<PairCandidate>::new();
    let total_remote_weight = remote_components
        .iter()
        .map(|component| component.score.max(0.05))
        .sum::<f32>()
        .max(1.0e-6);
    for (remote_index, remote_component) in remote_components.iter().enumerate() {
        for (local_index, local_component) in local_components.iter().enumerate() {
            let Some(score) = component_pair_alignment_score(
                local_component,
                remote_component,
                yaw_radians,
                translation_x,
                translation_z,
            ) else {
                continue;
            };
            pair_candidates.push(PairCandidate {
                local_index,
                remote_index,
                score,
                weight: remote_component.score.max(0.05),
            });
        }
    }
    if pair_candidates.is_empty() {
        return Some(0.0);
    }
    pair_candidates.sort_by(|left, right| right.score.total_cmp(&left.score));
    let mut used_local = vec![false; local_components.len()];
    let mut used_remote = vec![false; remote_components.len()];
    let mut score_sum = 0.0f32;
    let mut matched_weight = 0.0f32;
    for candidate in pair_candidates {
        if used_local[candidate.local_index] || used_remote[candidate.remote_index] {
            continue;
        }
        used_local[candidate.local_index] = true;
        used_remote[candidate.remote_index] = true;
        score_sum += candidate.score * candidate.weight;
        if candidate.score > 0.30 {
            matched_weight += candidate.weight;
        }
    }
    let coverage = (matched_weight / total_remote_weight).clamp(0.0, 1.0);
    Some((score_sum / total_remote_weight) * coverage.sqrt())
}

fn score_component_alignment_symmetric(
    local_components: &[ShapeComponent],
    remote_components: &[ShapeComponent],
    yaw_radians: f32,
    translation_x: f32,
    translation_z: f32,
) -> f32 {
    let Some(forward) = score_component_alignment_one_way(
        local_components,
        remote_components,
        yaw_radians,
        translation_x,
        translation_z,
    ) else {
        return 0.0;
    };
    if forward <= 0.0 {
        return 0.0;
    }
    let Some(inverse) = score_component_alignment_one_way(
        remote_components,
        local_components,
        -yaw_radians,
        -translation_x,
        -translation_z,
    ) else {
        return forward;
    };
    if inverse <= 0.0 {
        return forward;
    }
    (forward * inverse).sqrt()
}

fn generate_component_descriptor_seeds(
    local: &ShapeMatchGrid,
    remote: &ShapeMatchGrid,
) -> Vec<ShapeMatchCandidate> {
    let local_components = extract_shape_components(local, 6);
    let remote_components = extract_shape_components(remote, 6);
    let mut matches = Vec::<(usize, usize, usize, f32)>::new();
    for (local_index, &local_component) in local_components.iter().enumerate() {
        for (remote_index, &remote_component) in remote_components.iter().enumerate() {
            if let Some((shift, quality)) =
                component_signature_match(local_component, remote_component)
            {
                matches.push((local_index, remote_index, shift, quality));
            }
        }
    }
    matches.sort_by(|left, right| right.3.total_cmp(&left.3));
    matches.truncate(16);

    let mut seeds = Vec::new();
    for &(local_index, remote_index, shift, quality) in &matches {
        let local_component = local_components[local_index];
        let remote_component = remote_components[remote_index];
        let yaw_radians = shift as f32 * (TAU / SHAPE_COMPONENT_SIGNATURE_BINS as f32);
        let (rotated_x, rotated_z) = rotate_xz(
            yaw_radians,
            remote_component.centroid_x,
            remote_component.centroid_z,
        );
        push_shape_candidate(
            &mut seeds,
            ShapeMatchCandidate {
                yaw_radians,
                translation_x: local_component.centroid_x - rotated_x,
                translation_z: local_component.centroid_z - rotated_z,
                score: quality * local_component.score.min(remote_component.score),
                ..ShapeMatchCandidate::default()
            },
            32,
            0.12,
            12.0_f32.to_radians(),
        );
    }

    for left in 0..matches.len() {
        for right in left + 1..matches.len() {
            let (local_a_idx, remote_a_idx, _shift_a, qa) = matches[left];
            let (local_b_idx, remote_b_idx, _shift_b, qb) = matches[right];
            if local_a_idx == local_b_idx || remote_a_idx == remote_b_idx {
                continue;
            }
            let local_a = local_components[local_a_idx];
            let local_b = local_components[local_b_idx];
            let remote_a = remote_components[remote_a_idx];
            let remote_b = remote_components[remote_b_idx];
            let local_dx = local_b.centroid_x - local_a.centroid_x;
            let local_dz = local_b.centroid_z - local_a.centroid_z;
            let remote_dx = remote_b.centroid_x - remote_a.centroid_x;
            let remote_dz = remote_b.centroid_z - remote_a.centroid_z;
            let local_len = (local_dx * local_dx + local_dz * local_dz).sqrt();
            let remote_len = (remote_dx * remote_dx + remote_dz * remote_dz).sqrt();
            if local_len < 0.18 || remote_len < 0.18 {
                continue;
            }
            let len_ratio =
                (local_len / remote_len.max(1.0e-4)).max(remote_len / local_len.max(1.0e-4));
            if len_ratio > 1.9 {
                continue;
            }
            let yaw_radians = wrap_angle(local_dz.atan2(local_dx) - remote_dz.atan2(remote_dx));
            let (rotated_ax, rotated_az) =
                rotate_xz(yaw_radians, remote_a.centroid_x, remote_a.centroid_z);
            let translation_x = local_a.centroid_x - rotated_ax;
            let translation_z = local_a.centroid_z - rotated_az;
            let (rotated_bx, rotated_bz) =
                rotate_xz(yaw_radians, remote_b.centroid_x, remote_b.centroid_z);
            let closure_error = ((local_b.centroid_x - (rotated_bx + translation_x)).powi(2)
                + (local_b.centroid_z - (rotated_bz + translation_z)).powi(2))
            .sqrt();
            if closure_error > 0.42 {
                continue;
            }
            push_shape_candidate(
                &mut seeds,
                ShapeMatchCandidate {
                    yaw_radians,
                    translation_x,
                    translation_z,
                    score: (qa + qb) * 0.5 - closure_error * 0.8,
                    ..ShapeMatchCandidate::default()
                },
                40,
                0.12,
                8.0_f32.to_radians(),
            );
        }
    }

    seeds
}

fn generate_component_context_seeds(
    local: &ShapeMatchGrid,
    remote: &ShapeMatchGrid,
) -> Vec<ShapeMatchCandidate> {
    let local_components = extract_shape_components(local, 6);
    let remote_components = extract_shape_components(remote, 6);
    let mut matches = Vec::<(usize, usize, usize, f32)>::new();
    for (local_index, &local_component) in local_components.iter().enumerate() {
        for (remote_index, &remote_component) in remote_components.iter().enumerate() {
            if let Some((shift, quality)) =
                component_context_match(local_component, remote_component)
            {
                matches.push((local_index, remote_index, shift, quality));
            }
        }
    }
    matches.sort_by(|left, right| right.3.total_cmp(&left.3));
    matches.truncate(20);

    let mut seeds = Vec::new();
    for &(local_index, remote_index, shift, quality) in &matches {
        let local_component = local_components[local_index];
        let remote_component = remote_components[remote_index];
        let yaw_radians = shift as f32 * (TAU / SHAPE_COMPONENT_CONTEXT_ANGULAR_BINS as f32);
        let (rotated_x, rotated_z) = rotate_xz(
            yaw_radians,
            remote_component.centroid_x,
            remote_component.centroid_z,
        );
        push_shape_candidate(
            &mut seeds,
            ShapeMatchCandidate {
                yaw_radians,
                translation_x: local_component.centroid_x - rotated_x,
                translation_z: local_component.centroid_z - rotated_z,
                score: quality * local_component.score.min(remote_component.score),
                ..ShapeMatchCandidate::default()
            },
            36,
            0.12,
            10.0_f32.to_radians(),
        );
    }

    for left in 0..matches.len() {
        for right in left + 1..matches.len() {
            let (local_a_idx, remote_a_idx, _shift_a, qa) = matches[left];
            let (local_b_idx, remote_b_idx, _shift_b, qb) = matches[right];
            if local_a_idx == local_b_idx || remote_a_idx == remote_b_idx {
                continue;
            }
            let local_a = local_components[local_a_idx];
            let local_b = local_components[local_b_idx];
            let remote_a = remote_components[remote_a_idx];
            let remote_b = remote_components[remote_b_idx];
            let local_dx = local_b.centroid_x - local_a.centroid_x;
            let local_dz = local_b.centroid_z - local_a.centroid_z;
            let remote_dx = remote_b.centroid_x - remote_a.centroid_x;
            let remote_dz = remote_b.centroid_z - remote_a.centroid_z;
            let local_len = (local_dx * local_dx + local_dz * local_dz).sqrt();
            let remote_len = (remote_dx * remote_dx + remote_dz * remote_dz).sqrt();
            if local_len < 0.18 || remote_len < 0.18 {
                continue;
            }
            let len_ratio =
                (local_len / remote_len.max(1.0e-4)).max(remote_len / local_len.max(1.0e-4));
            if len_ratio > 1.9 {
                continue;
            }
            let yaw_radians = wrap_angle(local_dz.atan2(local_dx) - remote_dz.atan2(remote_dx));
            let (rotated_ax, rotated_az) =
                rotate_xz(yaw_radians, remote_a.centroid_x, remote_a.centroid_z);
            let translation_x = local_a.centroid_x - rotated_ax;
            let translation_z = local_a.centroid_z - rotated_az;
            let (rotated_bx, rotated_bz) =
                rotate_xz(yaw_radians, remote_b.centroid_x, remote_b.centroid_z);
            let closure_error = ((local_b.centroid_x - (rotated_bx + translation_x)).powi(2)
                + (local_b.centroid_z - (rotated_bz + translation_z)).powi(2))
            .sqrt();
            if closure_error > 0.48 {
                continue;
            }
            push_shape_candidate(
                &mut seeds,
                ShapeMatchCandidate {
                    yaw_radians,
                    translation_x,
                    translation_z,
                    score: (qa + qb) * 0.5 - closure_error * 0.7,
                    ..ShapeMatchCandidate::default()
                },
                42,
                0.12,
                8.0_f32.to_radians(),
            );
        }
    }

    seeds
}

fn generate_component_graph_seeds(
    local: &ShapeMatchGrid,
    remote: &ShapeMatchGrid,
) -> Vec<ShapeMatchCandidate> {
    // Increase to 12 components to get more potential pairs
    let local_components = extract_shape_components(local, 12);
    let remote_components = extract_shape_components(remote, 12);
    let mut seeds = Vec::new();
    println!(
        "generate_component_graph_seeds: local_components {}, remote_components {}",
        local_components.len(),
        remote_components.len()
    );

    for local_a in 0..local_components.len() {
        for local_b in 0..local_components.len() {
            if local_b == local_a {
                continue;
            }
            let local_ab_x =
                local_components[local_b].centroid_x - local_components[local_a].centroid_x;
            let local_ab_z =
                local_components[local_b].centroid_z - local_components[local_a].centroid_z;
            let local_ab = (local_ab_x * local_ab_x + local_ab_z * local_ab_z).sqrt();
            if local_ab < 0.20 {
                continue; // Points too close to form a stable rotation vector
            }
            let local_angle = local_ab_z.atan2(local_ab_x);

            for remote_a in 0..remote_components.len() {
                for remote_b in 0..remote_components.len() {
                    if remote_b == remote_a {
                        continue;
                    }
                    let area_ratio_a = (local_components[local_a].area_m2
                        / remote_components[remote_a].area_m2.max(1.0e-4))
                    .max(
                        remote_components[remote_a].area_m2
                            / local_components[local_a].area_m2.max(1.0e-4),
                    );
                    let area_ratio_b = (local_components[local_b].area_m2
                        / remote_components[remote_b].area_m2.max(1.0e-4))
                    .max(
                        remote_components[remote_b].area_m2
                            / local_components[local_b].area_m2.max(1.0e-4),
                    );

                    // Slightly looser area tolerance since partial occlusion can shrink blobs
                    if area_ratio_a > 3.5 || area_ratio_b > 3.5 {
                        continue;
                    }

                    let remote_ab_x = remote_components[remote_b].centroid_x
                        - remote_components[remote_a].centroid_x;
                    let remote_ab_z = remote_components[remote_b].centroid_z
                        - remote_components[remote_a].centroid_z;
                    let remote_ab = (remote_ab_x * remote_ab_x + remote_ab_z * remote_ab_z).sqrt();
                    if remote_ab < 0.20 {
                        continue;
                    }

                    let length_ratio = (local_ab / remote_ab).max(remote_ab / local_ab);
                    // Segment lengths between the two centroids must match reasonably well
                    // Since it's just the distance between two objects, it should be quite stable
                    if length_ratio > 1.35 {
                        continue;
                    }

                    let remote_angle = remote_ab_z.atan2(remote_ab_x);
                    let yaw_radians = wrap_angle(local_angle - remote_angle);

                    let (rotated_ax, rotated_az) = rotate_xz(
                        yaw_radians,
                        remote_components[remote_a].centroid_x,
                        remote_components[remote_a].centroid_z,
                    );
                    let translation_x = local_components[local_a].centroid_x - rotated_ax;
                    let translation_z = local_components[local_a].centroid_z - rotated_az;

                    let seed_score = local_components[local_a]
                        .score
                        .min(remote_components[remote_a].score)
                        + local_components[local_b]
                            .score
                            .min(remote_components[remote_b].score)
                        + 1.0 / length_ratio; // Small bonus for perfectly matching distances

                    push_shape_candidate(
                        &mut seeds,
                        ShapeMatchCandidate {
                            yaw_radians,
                            translation_x,
                            translation_z,
                            score: seed_score.max(0.001),
                            ..ShapeMatchCandidate::default()
                        },
                        128,
                        0.15,
                        6.0_f32.to_radians(),
                    );
                }
            }
        }
    }
    seeds
}

fn search_consensus_seeded(
    bands: &[BandGridPair],
    seeds: &[ShapeMatchCandidate],
    translation_window_meters: f32,
    yaw_window_radians: f32,
    yaw_step_radians: f32,
    top_k: usize,
) -> Vec<ShapeMatchCandidate> {
    let Some(first_band) = bands.first() else {
        return Vec::new();
    };
    let mut finalists = Vec::new();
    let step = first_band.local.cell_size_meters.max(1.0e-3);
    let translation_steps = (translation_window_meters / step).ceil().max(1.0) as isize;
    let yaw_steps = (yaw_window_radians / yaw_step_radians).ceil().max(1.0) as isize;
    for seed in seeds {
        for yaw_step in -yaw_steps..=yaw_steps {
            let yaw_radians = wrap_angle(seed.yaw_radians + yaw_step as f32 * yaw_step_radians);
            for x_step in -translation_steps..=translation_steps {
                let translation_x = seed.translation_x + x_step as f32 * step;
                for z_step in -translation_steps..=translation_steps {
                    let translation_z = seed.translation_z + z_step as f32 * step;
                    let candidate = score_consensus_candidate(
                        ShapeMatchCandidate {
                            yaw_radians,
                            translation_x,
                            translation_z,
                            ..ShapeMatchCandidate::default()
                        },
                        bands,
                    );
                    push_shape_candidate(
                        &mut finalists,
                        candidate,
                        top_k,
                        step * 1.25,
                        yaw_step_radians * 1.25,
                    );
                }
            }
        }
    }
    finalists
}

fn run_component_pair_seeded_analysis(
    local: &XrDepthAlignDescriptor,
    remote: &XrDepthAlignDescriptor,
    manual_pose: Option<ManualPose>,
) -> Option<ShapeMatchReport> {
    let bands = build_seeded_consensus_band_grids(local, remote);
    if bands.is_empty() {
        return None;
    }
    let furniture_band = ShapeBandSpec {
        label: "component_seed_band".to_string(),
        min_height_meters: 0.20,
        max_height_meters: Some(1.20),
        floor_offset_meters: 0.0,
        drop_border_components: true,
    };
    let (Some(local_map), Some(remote_map)) =
        (local.height_map.as_ref(), remote.height_map.as_ref())
    else {
        return None;
    };
    let local_grid = build_shape_match_grid(local_map, SHAPE_GLOBAL_CELL_METERS, &furniture_band)?;
    let remote_grid =
        build_shape_match_grid(remote_map, SHAPE_GLOBAL_CELL_METERS, &furniture_band)?;
    let seeds = generate_component_pair_seeds(&local_grid, &remote_grid);
    if seeds.is_empty() {
        return None;
    }
    let finalists = search_consensus_seeded(
        &bands,
        &seeds,
        0.36,
        18.0_f32.to_radians(),
        1.5_f32.to_radians(),
        SHAPE_FINAL_TOP_K,
    );
    if finalists.is_empty() {
        return None;
    }
    let manual_candidate = manual_pose.map(|manual_pose| {
        score_consensus_candidate(
            ShapeMatchCandidate {
                yaw_radians: manual_pose.rotation_radians,
                translation_x: manual_pose.shift_x_meters,
                translation_z: manual_pose.shift_y_meters,
                ..ShapeMatchCandidate::default()
            },
            &bands,
        )
    });
    Some(ShapeMatchReport {
        label: "shape_component_pair_seeded".to_string(),
        finalists,
        manual_candidate,
    })
}

fn run_component_descriptor_seeded_analysis(
    local: &XrDepthAlignDescriptor,
    remote: &XrDepthAlignDescriptor,
    manual_pose: Option<ManualPose>,
) -> Option<ShapeMatchReport> {
    let bands = build_seeded_consensus_band_grids(local, remote);
    if bands.is_empty() {
        return None;
    }
    let furniture_band = ShapeBandSpec {
        label: "component_descriptor_band".to_string(),
        min_height_meters: 0.20,
        max_height_meters: Some(1.20),
        floor_offset_meters: 0.0,
        drop_border_components: true,
    };
    let (Some(local_map), Some(remote_map)) =
        (local.height_map.as_ref(), remote.height_map.as_ref())
    else {
        return None;
    };
    let local_grid = build_shape_match_grid(local_map, SHAPE_GLOBAL_CELL_METERS, &furniture_band)?;
    let remote_grid =
        build_shape_match_grid(remote_map, SHAPE_GLOBAL_CELL_METERS, &furniture_band)?;
    let seeds = generate_component_descriptor_seeds(&local_grid, &remote_grid);
    if seeds.is_empty() {
        return None;
    }
    let finalists = search_consensus_seeded(
        &bands,
        &seeds,
        0.30,
        16.0_f32.to_radians(),
        1.0_f32.to_radians(),
        SHAPE_FINAL_TOP_K,
    );
    if finalists.is_empty() {
        return None;
    }
    let manual_candidate = manual_pose.map(|manual_pose| {
        score_consensus_candidate(
            ShapeMatchCandidate {
                yaw_radians: manual_pose.rotation_radians,
                translation_x: manual_pose.shift_x_meters,
                translation_z: manual_pose.shift_y_meters,
                ..ShapeMatchCandidate::default()
            },
            &bands,
        )
    });
    Some(ShapeMatchReport {
        label: "shape_component_descriptor_seeded".to_string(),
        finalists,
        manual_candidate,
    })
}

fn run_component_context_seeded_analysis(
    local: &XrDepthAlignDescriptor,
    remote: &XrDepthAlignDescriptor,
    manual_pose: Option<ManualPose>,
) -> Option<ShapeMatchReport> {
    let bands = build_seeded_consensus_band_grids(local, remote);
    if bands.is_empty() {
        return None;
    }
    let furniture_band = ShapeBandSpec {
        label: "component_context_band".to_string(),
        min_height_meters: 0.20,
        max_height_meters: Some(1.20),
        floor_offset_meters: 0.0,
        drop_border_components: true,
    };
    let (Some(local_map), Some(remote_map)) =
        (local.height_map.as_ref(), remote.height_map.as_ref())
    else {
        return None;
    };
    let local_grid = build_shape_match_grid(local_map, SHAPE_GLOBAL_CELL_METERS, &furniture_band)?;
    let remote_grid =
        build_shape_match_grid(remote_map, SHAPE_GLOBAL_CELL_METERS, &furniture_band)?;
    let seeds = generate_component_context_seeds(&local_grid, &remote_grid);
    if seeds.is_empty() {
        return None;
    }
    let finalists = search_consensus_seeded(
        &bands,
        &seeds,
        0.30,
        16.0_f32.to_radians(),
        1.0_f32.to_radians(),
        SHAPE_FINAL_TOP_K,
    );
    if finalists.is_empty() {
        return None;
    }
    let manual_candidate = manual_pose.map(|manual_pose| {
        score_consensus_candidate(
            ShapeMatchCandidate {
                yaw_radians: manual_pose.rotation_radians,
                translation_x: manual_pose.shift_x_meters,
                translation_z: manual_pose.shift_y_meters,
                ..ShapeMatchCandidate::default()
            },
            &bands,
        )
    });
    Some(ShapeMatchReport {
        label: "shape_component_context_seeded".to_string(),
        finalists,
        manual_candidate,
    })
}

fn run_component_graph_seeded_analysis(
    local: &XrDepthAlignDescriptor,
    remote: &XrDepthAlignDescriptor,
    manual_pose: Option<ManualPose>,
) -> Option<ShapeMatchReport> {
    let bands = build_seeded_consensus_band_grids(local, remote);
    if bands.is_empty() {
        return None;
    }
    let furniture_band = ShapeBandSpec {
        label: "component_graph_band".to_string(),
        min_height_meters: 0.20,
        max_height_meters: Some(1.20),
        floor_offset_meters: 0.0,
        drop_border_components: false,
    };
    let (Some(local_map), Some(remote_map)) =
        (local.height_map.as_ref(), remote.height_map.as_ref())
    else {
        return None;
    };
    let local_grid = build_shape_match_grid(local_map, SHAPE_GLOBAL_CELL_METERS, &furniture_band)?;
    let remote_grid =
        build_shape_match_grid(remote_map, SHAPE_GLOBAL_CELL_METERS, &furniture_band)?;
    let seeds = generate_component_graph_seeds(&local_grid, &remote_grid);
    if seeds.is_empty() {
        return None;
    }
    let finalists = search_consensus_seeded(
        &bands,
        &seeds,
        0.28,
        12.0_f32.to_radians(),
        1.0_f32.to_radians(),
        SHAPE_FINAL_TOP_K,
    );
    if finalists.is_empty() {
        return None;
    }
    let manual_candidate = manual_pose.map(|manual_pose| {
        score_consensus_candidate(
            ShapeMatchCandidate {
                yaw_radians: manual_pose.rotation_radians,
                translation_x: manual_pose.shift_x_meters,
                translation_z: manual_pose.shift_y_meters,
                ..ShapeMatchCandidate::default()
            },
            &bands,
        )
    });
    Some(ShapeMatchReport {
        label: "shape_component_graph_seeded".to_string(),
        finalists,
        manual_candidate,
    })
}

fn print_shape_band_scan_results(
    local: &XrDepthAlignDescriptor,
    remote: &XrDepthAlignDescriptor,
    results: &[ShapeBandScanResult],
) {
    if results.is_empty() {
        println!("shape_scan: unavailable");
        return;
    }

    let mut global = results.iter().collect::<Vec<_>>();
    global.sort_by(|left, right| {
        right.report.finalists[0]
            .score
            .total_cmp(&left.report.finalists[0].score)
            .then_with(|| {
                left.report.finalists[0]
                    .mean_distance_meters
                    .total_cmp(&right.report.finalists[0].mean_distance_meters)
            })
    });
    for (index, result) in global.iter().take(3).enumerate() {
        let best = result.report.finalists[0];
        let (manual_planar, manual_yaw_deg) = result
            .nearest_manual_finalist
            .map(|nearest| {
                (
                    nearest.planar_delta_meters,
                    nearest.yaw_delta_radians.to_degrees(),
                )
            })
            .unwrap_or((f32::NAN, f32::NAN));
        println!(
            "shape_scan_best_{}: band {} | yaw {:.3} rad ({:.1} deg) | translation ({:.3}, {:.3}, {:.3}) | score {:.3} | close {:.3} | dist {:.3} m | manual_dxz {:.3} | manual_dyaw {:.1} deg",
            index + 1,
            result.report.label,
            best.yaw_radians,
            best.yaw_radians.to_degrees(),
            best.translation_x,
            local.floor_y - remote.floor_y,
            best.translation_z,
            best.score,
            best.close_ratio,
            best.mean_distance_meters,
            manual_planar,
            manual_yaw_deg,
        );
    }

    let mut nearest = results
        .iter()
        .filter_map(|result| {
            result
                .nearest_manual_finalist
                .map(|nearest| (result, nearest))
        })
        .collect::<Vec<_>>();
    nearest.sort_by(
        |(left_result, left_nearest), (right_result, right_nearest)| {
            left_nearest
                .combined_distance
                .total_cmp(&right_nearest.combined_distance)
                .then_with(|| {
                    right_nearest
                        .candidate
                        .score
                        .total_cmp(&left_nearest.candidate.score)
                })
                .then_with(|| {
                    right_result.report.finalists[0]
                        .score
                        .total_cmp(&left_result.report.finalists[0].score)
                })
        },
    );
    for (index, (result, nearest)) in nearest.iter().take(3).enumerate() {
        println!(
            "shape_scan_near_{}: band {} | yaw {:.3} rad ({:.1} deg) | translation ({:.3}, {:.3}, {:.3}) | score {:.3} | manual_dxz {:.3} | manual_dyaw {:.1} deg | global_score {:.3}",
            index + 1,
            result.report.label,
            nearest.candidate.yaw_radians,
            nearest.candidate.yaw_radians.to_degrees(),
            nearest.candidate.translation_x,
            local.floor_y - remote.floor_y,
            nearest.candidate.translation_z,
            nearest.candidate.score,
            nearest.planar_delta_meters,
            nearest.yaw_delta_radians.to_degrees(),
            result.report.finalists[0].score,
        );
    }
}

// --- Heightmap correlation matching ---
// Uses raw float heights (not binary masks) for SE(2) alignment.
// The key insight: binary occupancy is mirror-symmetric for rectangular rooms,
// but actual height values (table=0.75m vs bookshelf=1.8m) break that symmetry.

#[derive(Clone, Debug)]
struct HeightCorrelationGrid {
    origin_x: f32,
    origin_z: f32,
    cell_size_meters: f32,
    size_x: usize,
    size_z: usize,
    /// Floor-relative height per cell, NaN if unobserved
    heights: Vec<f32>,
    /// Whether this cell was observed at all
    observed: Vec<bool>,
}

fn build_height_correlation_grid(
    height_map: &XrDepthAlignHeightMap,
    cell_size_meters: f32,
) -> Option<HeightCorrelationGrid> {
    if height_map.is_empty() {
        return None;
    }
    let size_x = ((height_map.extent_x_meters() / cell_size_meters).ceil() as usize).max(1);
    let size_z = ((height_map.extent_z_meters() / cell_size_meters).ceil() as usize).max(1);
    let len = size_x * size_z;
    let mut heights = vec![f32::NAN; len];
    let mut weight_sum = vec![0.0f32; len];
    let mut height_sum = vec![0.0f32; len];

    let cutout_center = height_map.player_cutout_center;
    let cutout_radius = (height_map.player_cutout_radius_meters + height_map.cell_size_meters)
        .max(height_map.cell_size_meters * 2.0);
    let floor_y = height_map.floor_y_meters;

    for src_z in 0..height_map.size_z_usize() {
        for src_x in 0..height_map.size_x_usize() {
            let index = height_map.cell_index(src_x, src_z);
            let height = *height_map.heights_meters.get(index)?;
            if !height.is_finite() {
                continue;
            }
            let world_x = height_map.origin_x + (src_x as f32 + 0.5) * height_map.cell_size_meters;
            let world_z = height_map.origin_z + (src_z as f32 + 0.5) * height_map.cell_size_meters;
            if cutout_center.is_some_and(|center| {
                let dx = world_x - center.x;
                let dz = world_z - center.y;
                (dx * dx + dz * dz).sqrt() <= cutout_radius
            }) {
                continue;
            }
            let relative_height = height - floor_y;
            // Only include cells above the floor with some signal
            if relative_height < 0.05 {
                continue;
            }
            let cx = ((world_x - height_map.origin_x) / cell_size_meters)
                .floor()
                .clamp(0.0, size_x.saturating_sub(1) as f32) as usize;
            let cz = ((world_z - height_map.origin_z) / cell_size_meters)
                .floor()
                .clamp(0.0, size_z.saturating_sub(1) as f32) as usize;
            let ci = cx + cz * size_x;
            // Use max height in cell (captures furniture tops)
            let w = 1.0f32;
            weight_sum[ci] += w;
            height_sum[ci] += relative_height * w;
        }
    }

    let mut observed = vec![false; len];
    for i in 0..len {
        if weight_sum[i] > 0.0 {
            heights[i] = height_sum[i] / weight_sum[i];
            observed[i] = true;
        }
    }

    Some(HeightCorrelationGrid {
        origin_x: height_map.origin_x,
        origin_z: height_map.origin_z,
        cell_size_meters,
        size_x,
        size_z,
        heights,
        observed,
    })
}

/// Sample the height grid at a world position using nearest-neighbor lookup.
/// Returns None if out of bounds or unobserved.
fn sample_height_grid(grid: &HeightCorrelationGrid, world_x: f32, world_z: f32) -> Option<f32> {
    let local_x = (world_x - grid.origin_x) / grid.cell_size_meters;
    let local_z = (world_z - grid.origin_z) / grid.cell_size_meters;
    if local_x < 0.0
        || local_z < 0.0
        || local_x >= grid.size_x as f32
        || local_z >= grid.size_z as f32
    {
        return None;
    }
    let x = local_x.floor() as usize;
    let z = local_z.floor() as usize;
    let i = x + z * grid.size_x;
    if grid.observed[i] {
        Some(grid.heights[i])
    } else {
        None
    }
}

/// Score a pose hypothesis by normalized cross-correlation of overlapping height values.
/// Returns (ncc, overlap_count) where ncc in [-1, 1] and higher is better.
fn score_height_correlation(
    local_grid: &HeightCorrelationGrid,
    remote_grid: &HeightCorrelationGrid,
    yaw_radians: f32,
    translation_x: f32,
    translation_z: f32,
) -> (f32, usize) {
    let (sin_yaw, cos_yaw) = yaw_radians.sin_cos();
    let mut sum_l = 0.0f64;
    let mut sum_r = 0.0f64;
    let mut sum_ll = 0.0f64;
    let mut sum_rr = 0.0f64;
    let mut sum_lr = 0.0f64;
    let mut count = 0usize;

    for rz in 0..remote_grid.size_z {
        for rx in 0..remote_grid.size_x {
            let ri = rx + rz * remote_grid.size_x;
            if !remote_grid.observed[ri] {
                continue;
            }
            let remote_h = remote_grid.heights[ri];
            let world_rx = remote_grid.origin_x + (rx as f32 + 0.5) * remote_grid.cell_size_meters;
            let world_rz = remote_grid.origin_z + (rz as f32 + 0.5) * remote_grid.cell_size_meters;
            let rotated_x = world_rx * cos_yaw - world_rz * sin_yaw;
            let rotated_z = world_rx * sin_yaw + world_rz * cos_yaw;
            let local_x = rotated_x + translation_x;
            let local_z = rotated_z + translation_z;
            let Some(local_h) = sample_height_grid(local_grid, local_x, local_z) else {
                continue;
            };
            let lh = local_h as f64;
            let rh = remote_h as f64;
            sum_l += lh;
            sum_r += rh;
            sum_ll += lh * lh;
            sum_rr += rh * rh;
            sum_lr += lh * rh;
            count += 1;
        }
    }

    if count < 8 {
        return (0.0, count);
    }

    let n = count as f64;
    let mean_l = sum_l / n;
    let mean_r = sum_r / n;
    let var_l = (sum_ll / n - mean_l * mean_l).max(0.0);
    let var_r = (sum_rr / n - mean_r * mean_r).max(0.0);
    let cov_lr = sum_lr / n - mean_l * mean_r;
    let denom = (var_l * var_r).sqrt();
    if denom < 1.0e-8 {
        return (0.0, count);
    }
    let ncc = (cov_lr / denom) as f32;
    (ncc, count)
}

/// Convert height correlation score to a ShapeMatchCandidate for reporting.
fn height_correlation_candidate(
    local_grid: &HeightCorrelationGrid,
    remote_grid: &HeightCorrelationGrid,
    yaw_radians: f32,
    translation_x: f32,
    translation_z: f32,
) -> ShapeMatchCandidate {
    // Score forward
    let (ncc_fwd, count_fwd) = score_height_correlation(
        local_grid,
        remote_grid,
        yaw_radians,
        translation_x,
        translation_z,
    );
    // Score inverse
    let (ncc_inv, count_inv) = score_height_correlation(
        remote_grid,
        local_grid,
        -yaw_radians,
        // Inverse transform: rotate(-yaw) then translate(-tx, -tz)
        // But we need to negate translation in the rotated frame
        {
            let (s, c) = (-yaw_radians).sin_cos();
            -translation_x * c + translation_z * s
        },
        {
            let (s, c) = (-yaw_radians).sin_cos();
            -translation_x * s - translation_z * c
        },
    );

    let min_count = count_fwd.min(count_inv);
    let total_remote = remote_grid.observed.iter().filter(|&&o| o).count().max(1);
    let total_local = local_grid.observed.iter().filter(|&&o| o).count().max(1);
    let coverage_fwd = count_fwd as f32 / total_remote as f32;
    let coverage_inv = count_inv as f32 / total_local as f32;
    let coverage = (coverage_fwd * coverage_inv).sqrt();

    // Symmetric NCC
    let ncc = if ncc_fwd > 0.0 && ncc_inv > 0.0 {
        (ncc_fwd * ncc_inv).sqrt()
    } else {
        (ncc_fwd + ncc_inv) * 0.5
    };

    // Score combines correlation strength with coverage
    let score = ncc.max(0.0) * coverage.sqrt();

    ShapeMatchCandidate {
        score,
        feature_score: ncc,
        support_score: coverage,
        coverage,
        close_ratio: ncc.max(0.0),
        mean_distance_meters: (1.0 - ncc.max(0.0)).max(0.0),
        yaw_radians,
        translation_x,
        translation_z,
        in_bounds_points: min_count,
    }
}

const HEIGHT_CORR_COARSE_CELL_METERS: f32 = 0.20;
const HEIGHT_CORR_FINE_CELL_METERS: f32 = 0.10;
const HEIGHT_CORR_COARSE_YAW_STEP: f32 = 4.0_f32 * PI / 180.0;
const HEIGHT_CORR_FINE_YAW_STEP: f32 = 1.0_f32 * PI / 180.0;
const HEIGHT_CORR_FINAL_YAW_STEP: f32 = 0.25_f32 * PI / 180.0;
const HEIGHT_CORR_COARSE_TOP_K: usize = 24;
const HEIGHT_CORR_FINE_TOP_K: usize = 12;
const HEIGHT_CORR_FINAL_TOP_K: usize = 8;

fn run_height_correlation_analysis(
    local: &XrDepthAlignDescriptor,
    remote: &XrDepthAlignDescriptor,
    manual_pose: Option<ManualPose>,
) -> Option<ShapeMatchReport> {
    let (Some(local_map), Some(remote_map)) =
        (local.height_map.as_ref(), remote.height_map.as_ref())
    else {
        return None;
    };

    // Build coarse height grids
    let local_coarse = build_height_correlation_grid(local_map, HEIGHT_CORR_COARSE_CELL_METERS)?;
    let remote_coarse = build_height_correlation_grid(remote_map, HEIGHT_CORR_COARSE_CELL_METERS)?;

    // Stage 1: coarse global search over (yaw, tx, tz)
    let local_min_x = local_coarse.origin_x;
    let local_max_x =
        local_coarse.origin_x + local_coarse.size_x as f32 * local_coarse.cell_size_meters;
    let local_min_z = local_coarse.origin_z;
    let local_max_z =
        local_coarse.origin_z + local_coarse.size_z as f32 * local_coarse.cell_size_meters;

    let mut coarse_finalists = Vec::new();
    let t_step = HEIGHT_CORR_COARSE_CELL_METERS * 2.0;

    let mut yaw = -PI;
    while yaw < PI {
        let (sin_yaw, cos_yaw) = yaw.sin_cos();
        // Compute rotated remote bounds
        let mut rmin_x = f32::INFINITY;
        let mut rmax_x = f32::NEG_INFINITY;
        let mut rmin_z = f32::INFINITY;
        let mut rmax_z = f32::NEG_INFINITY;
        let corners = [
            (remote_coarse.origin_x, remote_coarse.origin_z),
            (
                remote_coarse.origin_x
                    + remote_coarse.size_x as f32 * remote_coarse.cell_size_meters,
                remote_coarse.origin_z,
            ),
            (
                remote_coarse.origin_x,
                remote_coarse.origin_z
                    + remote_coarse.size_z as f32 * remote_coarse.cell_size_meters,
            ),
            (
                remote_coarse.origin_x
                    + remote_coarse.size_x as f32 * remote_coarse.cell_size_meters,
                remote_coarse.origin_z
                    + remote_coarse.size_z as f32 * remote_coarse.cell_size_meters,
            ),
        ];
        for &(cx, cz) in &corners {
            let rx = cx * cos_yaw - cz * sin_yaw;
            let rz = cx * sin_yaw + cz * cos_yaw;
            rmin_x = rmin_x.min(rx);
            rmax_x = rmax_x.max(rx);
            rmin_z = rmin_z.min(rz);
            rmax_z = rmax_z.max(rz);
        }

        let t_min_x = local_min_x - rmax_x;
        let t_max_x = local_max_x - rmin_x;
        let t_min_z = local_min_z - rmax_z;
        let t_max_z = local_max_z - rmin_z;

        let steps_x = ((t_max_x - t_min_x) / t_step).ceil().max(0.0) as usize;
        let steps_z = ((t_max_z - t_min_z) / t_step).ceil().max(0.0) as usize;

        for xi in 0..=steps_x {
            let tx = t_min_x + xi as f32 * t_step;
            for zi in 0..=steps_z {
                let tz = t_min_z + zi as f32 * t_step;
                let candidate =
                    height_correlation_candidate(&local_coarse, &remote_coarse, yaw, tx, tz);
                push_shape_candidate(
                    &mut coarse_finalists,
                    candidate,
                    HEIGHT_CORR_COARSE_TOP_K,
                    t_step * 1.5,
                    HEIGHT_CORR_COARSE_YAW_STEP * 1.25,
                );
            }
        }
        yaw += HEIGHT_CORR_COARSE_YAW_STEP;
    }

    if coarse_finalists.is_empty() {
        return None;
    }

    // Stage 2: refine on finer grid
    let local_fine = build_height_correlation_grid(local_map, HEIGHT_CORR_FINE_CELL_METERS)?;
    let remote_fine = build_height_correlation_grid(remote_map, HEIGHT_CORR_FINE_CELL_METERS)?;

    let refine_t_step = HEIGHT_CORR_FINE_CELL_METERS;
    let refine_t_window = HEIGHT_CORR_COARSE_CELL_METERS * 2.5;
    let refine_yaw_window = HEIGHT_CORR_COARSE_YAW_STEP * 1.5;

    let mut fine_finalists = Vec::new();
    for seed in &coarse_finalists {
        let yaw_min = seed.yaw_radians - refine_yaw_window;
        let yaw_max = seed.yaw_radians + refine_yaw_window;
        let mut yaw = yaw_min;
        while yaw <= yaw_max {
            let t_steps = (refine_t_window / refine_t_step).ceil().max(1.0) as isize;
            for dxi in -t_steps..=t_steps {
                let tx = seed.translation_x + dxi as f32 * refine_t_step;
                for dzi in -t_steps..=t_steps {
                    let tz = seed.translation_z + dzi as f32 * refine_t_step;
                    let candidate =
                        height_correlation_candidate(&local_fine, &remote_fine, yaw, tx, tz);
                    push_shape_candidate(
                        &mut fine_finalists,
                        candidate,
                        HEIGHT_CORR_FINE_TOP_K,
                        refine_t_step * 1.5,
                        HEIGHT_CORR_FINE_YAW_STEP * 1.25,
                    );
                }
            }
            yaw += HEIGHT_CORR_FINE_YAW_STEP;
        }
    }

    if fine_finalists.is_empty() {
        return None;
    }

    // Stage 3: final sub-cell refinement
    let final_t_step = HEIGHT_CORR_FINE_CELL_METERS * 0.5;
    let final_t_window = HEIGHT_CORR_FINE_CELL_METERS * 2.0;
    let final_yaw_window = HEIGHT_CORR_FINE_YAW_STEP * 1.5;

    let mut final_finalists = Vec::new();
    for seed in &fine_finalists {
        let yaw_min = seed.yaw_radians - final_yaw_window;
        let yaw_max = seed.yaw_radians + final_yaw_window;
        let mut yaw = yaw_min;
        while yaw <= yaw_max {
            let t_steps = (final_t_window / final_t_step).ceil().max(1.0) as isize;
            for dxi in -t_steps..=t_steps {
                let tx = seed.translation_x + dxi as f32 * final_t_step;
                for dzi in -t_steps..=t_steps {
                    let tz = seed.translation_z + dzi as f32 * final_t_step;
                    let candidate =
                        height_correlation_candidate(&local_fine, &remote_fine, yaw, tx, tz);
                    push_shape_candidate(
                        &mut final_finalists,
                        candidate,
                        HEIGHT_CORR_FINAL_TOP_K,
                        final_t_step * 1.5,
                        HEIGHT_CORR_FINAL_YAW_STEP * 1.25,
                    );
                }
            }
            yaw += HEIGHT_CORR_FINAL_YAW_STEP;
        }
    }

    if final_finalists.is_empty() {
        return None;
    }

    let manual_candidate = manual_pose.map(|mp| {
        height_correlation_candidate(
            &local_fine,
            &remote_fine,
            mp.rotation_radians,
            mp.shift_x_meters,
            mp.shift_y_meters,
        )
    });

    Some(ShapeMatchReport {
        label: "height_correlation".to_string(),
        finalists: final_finalists,
        manual_candidate,
    })
}

// --- Blob RANSAC matching ---
// Extracts furniture blobs with mean heights from both scans, then exhaustively
// tries all 2-blob correspondence pairs to generate SE(2) hypotheses.
// Scores each hypothesis by heightmap correlation.

#[derive(Clone, Copy, Debug)]
struct HeightBlob {
    centroid_x: f32,
    centroid_z: f32,
    mean_height: f32,
    area_m2: f32,
}

fn extract_height_blobs(
    height_map: &XrDepthAlignHeightMap,
    cell_size_meters: f32,
) -> Vec<HeightBlob> {
    let size_x = ((height_map.extent_x_meters() / cell_size_meters).ceil() as usize).max(1);
    let size_z = ((height_map.extent_z_meters() / cell_size_meters).ceil() as usize).max(1);
    let len = size_x * size_z;
    let mut occupied = vec![false; len];
    let mut cell_height = vec![0.0f32; len];
    let mut cell_count = vec![0usize; len];

    let cutout_center = height_map.player_cutout_center;
    let cutout_radius = (height_map.player_cutout_radius_meters + height_map.cell_size_meters)
        .max(height_map.cell_size_meters * 2.0);
    let floor_y = height_map.floor_y_meters;
    let min_h = SHAPE_FURNITURE_MIN_HEIGHT_METERS;
    let max_h = SHAPE_FURNITURE_MAX_HEIGHT_METERS;

    for src_z in 0..height_map.size_z_usize() {
        for src_x in 0..height_map.size_x_usize() {
            let index = height_map.cell_index(src_x, src_z);
            let Some(&height) = height_map.heights_meters.get(index) else {
                continue;
            };
            if !height.is_finite() {
                continue;
            }
            let world_x = height_map.origin_x + (src_x as f32 + 0.5) * height_map.cell_size_meters;
            let world_z = height_map.origin_z + (src_z as f32 + 0.5) * height_map.cell_size_meters;
            if cutout_center.is_some_and(|center| {
                let dx = world_x - center.x;
                let dz = world_z - center.y;
                (dx * dx + dz * dz).sqrt() <= cutout_radius
            }) {
                continue;
            }
            let rel_h = height - floor_y;
            if rel_h < min_h || rel_h > max_h {
                continue;
            }
            let cx = ((world_x - height_map.origin_x) / cell_size_meters)
                .floor()
                .clamp(0.0, size_x.saturating_sub(1) as f32) as usize;
            let cz = ((world_z - height_map.origin_z) / cell_size_meters)
                .floor()
                .clamp(0.0, size_z.saturating_sub(1) as f32) as usize;
            let ci = cx + cz * size_x;
            occupied[ci] = true;
            cell_height[ci] += rel_h;
            cell_count[ci] += 1;
        }
    }

    // Connected component extraction with mean height
    let min_cells = (SHAPE_FURNITURE_MIN_COMPONENT_AREA_METERS2
        / (cell_size_meters * cell_size_meters).max(1.0e-4))
    .ceil()
    .max(1.0) as usize;

    let mut visited = vec![false; len];
    let mut blobs = Vec::new();

    for start in 0..len {
        if visited[start] || !occupied[start] {
            continue;
        }
        let mut stack = vec![start];
        let mut component = Vec::new();
        let mut touches_border = false;
        visited[start] = true;
        while let Some(idx) = stack.pop() {
            component.push(idx);
            let x = idx % size_x;
            let z = idx / size_x;
            if x == 0 || z == 0 || x + 1 == size_x || z + 1 == size_z {
                touches_border = true;
            }
            for (dx, dz) in [(-1isize, 0isize), (1, 0), (0, -1), (0, 1)] {
                let nx = x as isize + dx;
                let nz = z as isize + dz;
                if nx < 0 || nz < 0 || nx >= size_x as isize || nz >= size_z as isize {
                    continue;
                }
                let ni = nx as usize + nz as usize * size_x;
                if visited[ni] || !occupied[ni] {
                    continue;
                }
                visited[ni] = true;
                stack.push(ni);
            }
        }

        if component.len() < min_cells || touches_border {
            continue;
        }

        let mut sum_x = 0.0f32;
        let mut sum_z = 0.0f32;
        let mut sum_h = 0.0f32;
        let mut total_samples = 0usize;
        for &ci in &component {
            let x = ci % size_x;
            let z = ci / size_x;
            let wx = height_map.origin_x + (x as f32 + 0.5) * cell_size_meters;
            let wz = height_map.origin_z + (z as f32 + 0.5) * cell_size_meters;
            sum_x += wx * cell_count[ci] as f32;
            sum_z += wz * cell_count[ci] as f32;
            sum_h += cell_height[ci];
            total_samples += cell_count[ci];
        }
        if total_samples == 0 {
            continue;
        }
        let n = total_samples as f32;
        blobs.push(HeightBlob {
            centroid_x: sum_x / n,
            centroid_z: sum_z / n,
            mean_height: sum_h / n,
            area_m2: component.len() as f32 * cell_size_meters * cell_size_meters,
        });
    }

    blobs.sort_by(|a, b| b.area_m2.total_cmp(&a.area_m2));
    blobs.truncate(20);
    blobs
}

/// From two corresponding blob pairs (l0↔r0, l1↔r1), compute the SE(2) transform.
/// Returns (yaw, tx, tz) or None if degenerate.
fn se2_from_two_point_pairs(
    l0x: f32,
    l0z: f32,
    l1x: f32,
    l1z: f32,
    r0x: f32,
    r0z: f32,
    r1x: f32,
    r1z: f32,
) -> Option<(f32, f32, f32)> {
    let dl_x = l1x - l0x;
    let dl_z = l1z - l0z;
    let dr_x = r1x - r0x;
    let dr_z = r1z - r0z;
    let dl_len = (dl_x * dl_x + dl_z * dl_z).sqrt();
    let dr_len = (dr_x * dr_x + dr_z * dr_z).sqrt();
    if dl_len < 0.15 || dr_len < 0.15 {
        return None; // Too close together
    }
    // Check distance ratio - should be ~1.0 for rigid transform
    let ratio = dl_len / dr_len;
    if ratio < 0.6 || ratio > 1.67 {
        return None;
    }
    let angle_l = dl_z.atan2(dl_x);
    let angle_r = dr_z.atan2(dr_x);
    let yaw = wrap_angle(angle_l - angle_r);

    let (sin_yaw, cos_yaw) = yaw.sin_cos();
    let rotated_r0x = r0x * cos_yaw - r0z * sin_yaw;
    let rotated_r0z = r0x * sin_yaw + r0z * cos_yaw;
    let tx = l0x - rotated_r0x;
    let tz = l0z - rotated_r0z;
    Some((yaw, tx, tz))
}

fn run_blob_ransac_analysis(
    local: &XrDepthAlignDescriptor,
    remote: &XrDepthAlignDescriptor,
    manual_pose: Option<ManualPose>,
) -> Option<ShapeMatchReport> {
    let (Some(local_map), Some(remote_map)) =
        (local.height_map.as_ref(), remote.height_map.as_ref())
    else {
        return None;
    };

    let local_blobs = extract_height_blobs(local_map, 0.10);
    let remote_blobs = extract_height_blobs(remote_map, 0.10);

    if local_blobs.len() < 2 || remote_blobs.len() < 2 {
        return None;
    }

    // Build height correlation grids for scoring
    let local_grid = build_height_correlation_grid(local_map, HEIGHT_CORR_FINE_CELL_METERS)?;
    let remote_grid = build_height_correlation_grid(remote_map, HEIGHT_CORR_FINE_CELL_METERS)?;

    let mut finalists = Vec::new();

    // Exhaustive 2-pair RANSAC
    let nl = local_blobs.len().min(12);
    let nr = remote_blobs.len().min(12);

    for li0 in 0..nl {
        for li1 in (li0 + 1)..nl {
            for ri0 in 0..nr {
                for ri1 in 0..nr {
                    if ri0 == ri1 {
                        continue;
                    }
                    // Check height compatibility
                    let h_diff_0 =
                        (local_blobs[li0].mean_height - remote_blobs[ri0].mean_height).abs();
                    let h_diff_1 =
                        (local_blobs[li1].mean_height - remote_blobs[ri1].mean_height).abs();
                    if h_diff_0 > 0.35 || h_diff_1 > 0.35 {
                        continue;
                    }

                    let Some((yaw, tx, tz)) = se2_from_two_point_pairs(
                        local_blobs[li0].centroid_x,
                        local_blobs[li0].centroid_z,
                        local_blobs[li1].centroid_x,
                        local_blobs[li1].centroid_z,
                        remote_blobs[ri0].centroid_x,
                        remote_blobs[ri0].centroid_z,
                        remote_blobs[ri1].centroid_x,
                        remote_blobs[ri1].centroid_z,
                    ) else {
                        continue;
                    };

                    let candidate =
                        height_correlation_candidate(&local_grid, &remote_grid, yaw, tx, tz);
                    push_shape_candidate(
                        &mut finalists,
                        candidate,
                        HEIGHT_CORR_FINE_TOP_K,
                        0.30,
                        5.0_f32.to_radians(),
                    );
                }
            }
        }
    }

    if finalists.is_empty() {
        return None;
    }

    // Refine top candidates with local search
    let mut refined = Vec::new();
    let refine_step = HEIGHT_CORR_FINE_CELL_METERS * 0.5;
    let refine_window = HEIGHT_CORR_FINE_CELL_METERS * 3.0;
    let yaw_step = 0.5_f32.to_radians();
    let yaw_window = 3.0_f32.to_radians();

    for seed in finalists.iter().take(8) {
        let mut yaw = seed.yaw_radians - yaw_window;
        while yaw <= seed.yaw_radians + yaw_window {
            let t_steps = (refine_window / refine_step).ceil() as isize;
            for dxi in -t_steps..=t_steps {
                let tx = seed.translation_x + dxi as f32 * refine_step;
                for dzi in -t_steps..=t_steps {
                    let tz = seed.translation_z + dzi as f32 * refine_step;
                    let candidate =
                        height_correlation_candidate(&local_grid, &remote_grid, yaw, tx, tz);
                    push_shape_candidate(
                        &mut refined,
                        candidate,
                        HEIGHT_CORR_FINAL_TOP_K,
                        refine_step * 1.5,
                        yaw_step * 1.25,
                    );
                }
            }
            yaw += yaw_step;
        }
    }

    if refined.is_empty() {
        refined = finalists;
    }

    let manual_candidate = manual_pose.map(|mp| {
        height_correlation_candidate(
            &local_grid,
            &remote_grid,
            mp.rotation_radians,
            mp.shift_x_meters,
            mp.shift_y_meters,
        )
    });

    Some(ShapeMatchReport {
        label: "blob_ransac_height".to_string(),
        finalists: refined,
        manual_candidate,
    })
}

fn analyze_path(options: AnalyzeOptions, path: &Path) -> Result<(), String> {
    let bytes =
        fs::read(&path).map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    let pair = XrNetAlignmentDescriptorDumpPair::from_file_bytes(&bytes)
        .ok_or_else(|| format!("failed to decode {}", path.display()))?;

    println!("dump: {}", path.display());
    println!(
        "file_bytes: {} | file_mtime_unix: {} | format_version: {} | captured_at_unix_ms: {} | remote_peer_id: {}",
        bytes.len(),
        system_time_hint(&path),
        pair.format_version,
        pair.captured_at_unix_ms,
        pair.remote_peer_id.0
    );

    let mut local = pair.local_descriptor.descriptor.clone();
    let mut remote = pair.remote_descriptor.descriptor.clone();
    let local = &local;
    let remote = &remote;
    print_descriptor_stats("local", local);
    print_descriptor_stats("remote", remote);
    let manual_pose = load_manual_pose(path);
    if let Some(manual_pose) = manual_pose {
        println!(
            "manual_pose: x {:.3} | y {:.3} | rot {:.3} rad ({:.1} deg)",
            manual_pose.shift_x_meters,
            manual_pose.shift_y_meters,
            manual_pose.rotation_radians,
            manual_pose.rotation_radians.to_degrees()
        );
    } else {
        println!("manual_pose: none");
    }

    if !options.solve {
        println!("solve: skipped (pass --solve to run the full matcher)");
        return Ok(());
    }

    let solve_started = Instant::now();
    let callback_budget = std::time::Duration::from_millis(ANALYZE_TSDF_CALLBACK_BUDGET_MILLIS);
    let mut matcher = XrDepthAlignMatcher::new(local, remote, None);
    let mut callback_count = 0u32;
    let mut total_callback_steps = 0u32;
    let mut max_callback_steps = 0u32;
    let mut max_callback_elapsed = std::time::Duration::ZERO;
    while !matcher.is_finished() {
        let callback_started = Instant::now();
        let callback_steps =
            matcher.step_for_budget(callback_budget, ANALYZE_TSDF_CALLBACK_MAX_STEPS);
        if callback_steps == 0 {
            break;
        }
        callback_count = callback_count.saturating_add(1);
        total_callback_steps = total_callback_steps.saturating_add(callback_steps);
        max_callback_steps = max_callback_steps.max(callback_steps);
        max_callback_elapsed = max_callback_elapsed.max(callback_started.elapsed());
    }
    let diagnostic = matcher
        .diagnostic()
        .expect("timesliced analyzer matcher should produce a diagnostic");
    let solve_elapsed = solve_started.elapsed();
    let estimated_callback_wall_ms = diagnostic.total_compute_ms as u64
        + ANALYZE_TSDF_CALLBACK_INTERVAL_MILLIS * callback_count.saturating_sub(1) as u64;
    let estimated_callback_busy_ratio = if estimated_callback_wall_ms == 0 {
        0.0
    } else {
        diagnostic.total_compute_ms as f64 / estimated_callback_wall_ms as f64
    };
    println!(
        "diagnostic: outcome {} | solve_ms {} | total_match_ms {} | steps {} | max_step_ms {} | yaw candidates {} | pose candidates {} | shortlisted {} | local walls {} | remote walls {} | remote dense {}",
        explain_outcome(&diagnostic),
        solve_elapsed.as_millis(),
        diagnostic.total_compute_ms,
        diagnostic.step_count,
        diagnostic.max_step_ms,
        diagnostic.yaw_candidate_count,
        diagnostic.pose_candidate_count,
        diagnostic.shortlisted_pose_count,
        diagnostic.local_wall_samples,
        diagnostic.remote_wall_samples,
        diagnostic.remote_dense_wall_samples,
    );
    println!(
        "timing_ms: build {} | yaw {} | vote {} | signal_refine {} | signal_score {} | height_refine {} | final_score {} | wall_profile {}",
        diagnostic.signal_build_ms,
        diagnostic.yaw_candidate_ms,
        diagnostic.translation_vote_ms,
        diagnostic.signal_refine_ms,
        diagnostic.signal_score_ms,
        diagnostic.height_refine_ms,
        diagnostic.final_score_ms,
        diagnostic.wall_profile_ms,
    );
    println!(
        "callback_sim: budget_ms {} | interval_ms {} | callbacks {} | avg_steps_per_callback {:.1} | max_steps_per_callback {} | max_callback_ms {} | est_wall_ms {} | est_busy {:.0}%",
        ANALYZE_TSDF_CALLBACK_BUDGET_MILLIS,
        ANALYZE_TSDF_CALLBACK_INTERVAL_MILLIS,
        callback_count,
        if callback_count == 0 {
            0.0
        } else {
            total_callback_steps as f64 / callback_count as f64
        },
        max_callback_steps,
        max_callback_elapsed.as_millis(),
        estimated_callback_wall_ms,
        estimated_callback_busy_ratio * 100.0,
    );

    if let Some(best) = diagnostic.best_solution {
        println!(
            "best: yaw {:.3} rad ({:.1} deg) | translation ({:.3}, {:.3}, {:.3}) | conf {:.3} | sym {:.3} | residual {:.3} m | matched {}",
            best.yaw_radians,
            best.yaw_radians.to_degrees(),
            best.translation.x,
            best.translation.y,
            best.translation.z,
            best.confidence,
            best.symmetry_confidence,
            best.residual_meters,
            best.matched_samples
        );
    } else {
        println!("best: none");
    }

    if let Some(accepted) = diagnostic.accepted_solution() {
        println!(
            "accepted: yaw {:.3} rad ({:.1} deg) | translation ({:.3}, {:.3}, {:.3})",
            accepted.yaw_radians,
            accepted.yaw_radians.to_degrees(),
            accepted.translation.x,
            accepted.translation.y,
            accepted.translation.z,
        );
        if let Some(manual_pose) = manual_pose {
            println!("{}", manual_pose_delta_text(accepted, manual_pose));
        }
    } else {
        println!("accepted: none");
    }

    if let Some(manual_pose) = manual_pose {
        let manual_seed = manual_pose_solution(local, remote, manual_pose);
        let manual_scored = xr_depth_align_rescore_remote_to_local(local, remote, manual_seed);
        println!(
            "manual_scored: yaw {:.3} rad ({:.1} deg) | translation ({:.3}, {:.3}, {:.3}) | conf {:.3} | sym {:.3} | residual {:.3} m | matched {} | accepted {}",
            manual_scored.yaw_radians,
            manual_scored.yaw_radians.to_degrees(),
            manual_scored.translation.x,
            manual_scored.translation.y,
            manual_scored.translation.z,
            manual_scored.confidence,
            manual_scored.symmetry_confidence,
            manual_scored.residual_meters,
            manual_scored.matched_samples,
            xr_depth_align_solution_is_accepted(&diagnostic, manual_scored)
        );
    }
    let run_legacy_search_paths = false;
    if !run_legacy_search_paths {
        println!("legacy_search_paths: disabled while focusing on runtime matcher perf");
        return Ok(());
    }

    let overlap_band = ShapeBandSpec {
        label: "shape_overlap_0.60_1.80".to_string(),
        min_height_meters: 0.60,
        max_height_meters: Some(1.80),
        floor_offset_meters: 0.0,
        drop_border_components: false,
    };
    let obstacle_band = shape_band_obstacle();
    let furniture_band = shape_band_furniture();
    let (
        obstacle_result,
        furniture_result,
        hough_result,
        overlap_result,
        component_pair_result,
        component_descriptor_result,
        component_context_result,
        component_graph_result,
        correlative_result,
        height_corr_result,
        blob_ransac_result,
        shape_scan_result,
    ) = std::thread::scope(|scope| {
        let obstacle_band = obstacle_band.clone();
        let obstacle_handle = scope.spawn(move || {
            let started = Instant::now();
            let report = run_shape_match_analysis(local, remote, manual_pose, &obstacle_band);
            (started.elapsed().as_millis(), report)
        });
        let furniture_band = furniture_band.clone();
        let furniture_handle = scope.spawn(move || {
            let started = Instant::now();
            let report = run_shape_match_analysis(local, remote, manual_pose, &furniture_band);
            (started.elapsed().as_millis(), report)
        });
        let hough_handle = scope.spawn(|| {
            let started = Instant::now();
            let report = run_hough_shape_analysis(local, remote, manual_pose);
            (started.elapsed().as_millis(), report)
        });
        let overlap_band = overlap_band.clone();
        let overlap_handle = scope.spawn(move || {
            let started = Instant::now();
            let report = run_fixed_overlap_band_analysis(local, remote, manual_pose, &overlap_band);
            (started.elapsed().as_millis(), report)
        });
        let component_pair_handle = scope.spawn(|| {
            let started = Instant::now();
            let report = run_component_pair_seeded_analysis(local, remote, manual_pose);
            (started.elapsed().as_millis(), report)
        });
        let component_descriptor_handle = scope.spawn(|| {
            let started = Instant::now();
            let report = run_component_descriptor_seeded_analysis(local, remote, manual_pose);
            (started.elapsed().as_millis(), report)
        });
        let component_context_handle = scope.spawn(|| {
            let started = Instant::now();
            let report = run_component_context_seeded_analysis(local, remote, manual_pose);
            (started.elapsed().as_millis(), report)
        });
        let component_graph_handle = scope.spawn(|| {
            let started = Instant::now();
            let report = run_component_graph_seeded_analysis(local, remote, manual_pose);
            (started.elapsed().as_millis(), report)
        });
        let correlative_handle = scope.spawn(|| {
            let started = Instant::now();
            let report = run_correlative_consensus_analysis(local, remote, manual_pose);
            (started.elapsed().as_millis(), report)
        });
        let height_corr_handle = scope.spawn(|| {
            let started = Instant::now();
            let report = run_height_correlation_analysis(local, remote, manual_pose);
            (started.elapsed().as_millis(), report)
        });
        let blob_ransac_handle = scope.spawn(|| {
            let started = Instant::now();
            let report = run_blob_ransac_analysis(local, remote, manual_pose);
            (started.elapsed().as_millis(), report)
        });
        let shape_scan_handle = scope.spawn(|| {
            let started = Instant::now();
            let results = run_shape_band_scan(local, remote, manual_pose);
            (started.elapsed().as_millis(), results)
        });
        (
            obstacle_handle
                .join()
                .expect("obstacle analysis should not panic"),
            furniture_handle
                .join()
                .expect("furniture analysis should not panic"),
            hough_handle
                .join()
                .expect("hough analysis should not panic"),
            overlap_handle
                .join()
                .expect("overlap analysis should not panic"),
            component_pair_handle
                .join()
                .expect("component-pair analysis should not panic"),
            component_descriptor_handle
                .join()
                .expect("component-descriptor analysis should not panic"),
            component_context_handle
                .join()
                .expect("component-context analysis should not panic"),
            component_graph_handle
                .join()
                .expect("component-graph analysis should not panic"),
            correlative_handle
                .join()
                .expect("correlative analysis should not panic"),
            height_corr_handle
                .join()
                .expect("height-correlation analysis should not panic"),
            blob_ransac_handle
                .join()
                .expect("blob-ransac analysis should not panic"),
            shape_scan_handle
                .join()
                .expect("shape scan should not panic"),
        )
    });

    let mut primary_shape_reports = Vec::<ShapeMatchReport>::new();
    match obstacle_result {
        (elapsed_millis, Some(shape_report)) => {
            print_shape_match_report(local, remote, &shape_report, elapsed_millis);
            primary_shape_reports.push(shape_report);
        }
        _ => {
            println!("{}_match: unavailable", obstacle_band.label);
        }
    }
    match furniture_result {
        (elapsed_millis, Some(shape_report)) => {
            print_shape_match_report(local, remote, &shape_report, elapsed_millis);
            primary_shape_reports.push(shape_report);
        }
        _ => {
            println!("{}_match: unavailable", furniture_band.label);
        }
    }
    match hough_result {
        (elapsed_millis, Some(hough_report)) => {
            print_shape_match_report(local, remote, &hough_report, elapsed_millis);
            primary_shape_reports.push(hough_report);
        }
        _ => {
            println!("shape_hough_match: unavailable");
        }
    }
    match overlap_result {
        (elapsed_millis, Some(overlap_report)) => {
            print_shape_match_report(local, remote, &overlap_report, elapsed_millis);
            primary_shape_reports.push(overlap_report);
        }
        _ => {
            println!("{}_match: unavailable", overlap_band.label);
        }
    }
    if let Some(seed) = diagnostic.best_solution {
        let seed_candidates = gather_seed_candidates(
            &diagnostic,
            &primary_shape_reports.iter().collect::<Vec<_>>(),
        );
        let (
            overlap_seeded_result,
            consensus_result,
            vote_result,
            signed_seeded_result,
            signed_support_result,
        ) = std::thread::scope(|scope| {
            let overlap_seeded_band = overlap_band.clone();
            let overlap_seeded_handle = scope.spawn(move || {
                let started = Instant::now();
                let report =
                    run_seeded_overlap_band_analysis(local, remote, &overlap_seeded_band, seed);
                (started.elapsed().as_millis(), report)
            });
            let consensus_handle = scope.spawn(|| {
                let started = Instant::now();
                let report =
                    run_seeded_multiband_consensus_analysis(local, remote, manual_pose, seed);
                (started.elapsed().as_millis(), report)
            });
            let vote_handle = scope.spawn(|| {
                let started = Instant::now();
                let report = run_seeded_multiband_vote_analysis(local, remote, manual_pose, seed);
                (started.elapsed().as_millis(), report)
            });
            let signed_seeded_band = overlap_band.clone();
            let signed_seed_candidates = seed_candidates.clone();
            let signed_seeded_handle = scope.spawn(move || {
                let started = Instant::now();
                let report = run_seeded_signed_overlap_band_analysis(
                    local,
                    remote,
                    manual_pose,
                    &signed_seeded_band,
                    &signed_seed_candidates,
                );
                (started.elapsed().as_millis(), report)
            });
            let signed_support_band = overlap_band.clone();
            let signed_support_candidates = seed_candidates.clone();
            let signed_support_handle = scope.spawn(move || {
                let started = Instant::now();
                let report = run_seeded_signed_support_band_analysis(
                    local,
                    remote,
                    manual_pose,
                    &signed_support_band,
                    &signed_support_candidates,
                );
                (started.elapsed().as_millis(), report)
            });
            (
                overlap_seeded_handle
                    .join()
                    .expect("seeded overlap analysis should not panic"),
                consensus_handle
                    .join()
                    .expect("seeded consensus analysis should not panic"),
                vote_handle
                    .join()
                    .expect("seeded vote analysis should not panic"),
                signed_seeded_handle
                    .join()
                    .expect("seeded signed-overlap analysis should not panic"),
                signed_support_handle
                    .join()
                    .expect("seeded signed-support analysis should not panic"),
            )
        });

        match overlap_seeded_result {
            (elapsed_millis, Some(overlap_seeded_report)) => {
                print_shape_match_report(local, remote, &overlap_seeded_report, elapsed_millis);
            }
            _ => {
                println!("{}_seeded_match: unavailable", overlap_band.label);
            }
        }
        match consensus_result {
            (elapsed_millis, Some(consensus_report)) => {
                print_shape_match_report(local, remote, &consensus_report, elapsed_millis);
            }
            _ => {
                println!("shape_multiband_seeded_match: unavailable");
            }
        }
        match vote_result {
            (elapsed_millis, Some(vote_report)) => {
                print_shape_match_report(local, remote, &vote_report, elapsed_millis);
            }
            _ => {
                println!("shape_multiband_vote_seeded_match: unavailable");
            }
        }
        match signed_seeded_result {
            (elapsed_millis, Some(signed_seeded_report)) => {
                print_shape_match_report(local, remote, &signed_seeded_report, elapsed_millis);
            }
            _ => {
                println!("{}_signed_seeded_match: unavailable", overlap_band.label);
            }
        }
        match signed_support_result {
            (elapsed_millis, Some(signed_support_seeded_report)) => {
                print_shape_match_report(
                    local,
                    remote,
                    &signed_support_seeded_report,
                    elapsed_millis,
                );
            }
            _ => {
                println!(
                    "{}_signed_support_seeded_match: unavailable",
                    overlap_band.label
                );
            }
        }
    }

    match component_pair_result {
        (elapsed_millis, Some(component_pair_report)) => {
            print_shape_match_report(local, remote, &component_pair_report, elapsed_millis);
        }
        _ => {
            println!("shape_component_pair_seeded_match: unavailable");
        }
    }
    match component_descriptor_result {
        (elapsed_millis, Some(component_descriptor_report)) => {
            print_shape_match_report(local, remote, &component_descriptor_report, elapsed_millis);
        }
        _ => {
            println!("shape_component_descriptor_seeded_match: unavailable");
        }
    }
    match component_context_result {
        (elapsed_millis, Some(component_context_report)) => {
            print_shape_match_report(local, remote, &component_context_report, elapsed_millis);
        }
        _ => {
            println!("shape_component_context_seeded_match: unavailable");
        }
    }
    match component_graph_result {
        (elapsed_millis, Some(component_graph_report)) => {
            print_shape_match_report(local, remote, &component_graph_report, elapsed_millis);
        }
        _ => {
            println!("shape_component_graph_seeded_match: unavailable");
        }
    }
    match correlative_result {
        (elapsed_millis, Some(correlative_report)) => {
            print_shape_match_report(local, remote, &correlative_report, elapsed_millis);
        }
        _ => {
            println!("shape_correlative_consensus_match: unavailable");
        }
    }
    match height_corr_result {
        (elapsed_millis, Some(height_corr_report)) => {
            print_shape_match_report(local, remote, &height_corr_report, elapsed_millis);
        }
        _ => {
            println!("height_correlation_match: unavailable");
        }
    }
    match blob_ransac_result {
        (elapsed_millis, Some(blob_ransac_report)) => {
            print_shape_match_report(local, remote, &blob_ransac_report, elapsed_millis);
        }
        _ => {
            println!("blob_ransac_height_match: unavailable");
        }
    }

    let (shape_scan_elapsed_millis, shape_scan_results) = shape_scan_result;
    println!("shape_scan_ms: {}", shape_scan_elapsed_millis);
    print_shape_band_scan_results(local, remote, &shape_scan_results);

    Ok(())
}

fn main() -> Result<(), String> {
    let (options, paths) = parse_args()?;
    for (index, path) in paths.iter().enumerate() {
        if index != 0 {
            println!();
        }
        analyze_path(options, path)?;
    }
    Ok(())
}
