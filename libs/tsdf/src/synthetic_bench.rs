use crate::{
    apply_preprocessed_depth_mesh, depth_ndc_to_world_ray, makepad_math, preprocess_depth_mesh,
    DepthMeshJob, DepthMeshVolume, DepthPreprocessWorkerState, TsdfPublishedSnapshot,
    DEPTH_VOXEL_MAX_DEPTH_VALUE, DEPTH_VOXEL_MAX_DISTANCE_METERS, DEPTH_VOXEL_MIN_DEPTH_VALUE,
    DEPTH_VOXEL_MIN_DISTANCE_METERS,
};
use makepad_math::{vec3f, vec4f, CameraFov, Mat4f, Vec3f};
use std::sync::Arc;

#[derive(Clone, Copy, Debug)]
struct SyntheticTsdfBenchStats {
    voxel_size_meters: f32,
    frame_count: usize,
    build_ms_per_frame: f64,
    preprocess_ms_per_frame: f64,
    apply_ms_per_frame: f64,
    publish_ms_per_frame: f64,
    staged_samples_per_frame: f64,
    reduced_samples_per_frame: f64,
    chunk_count: usize,
    active_voxel_count: usize,
    heap_mib: f64,
}

#[derive(Clone, Copy)]
struct SyntheticBox {
    min: Vec3f,
    max: Vec3f,
}

impl SyntheticBox {
    const fn new(min: Vec3f, max: Vec3f) -> Self {
        Self { min, max }
    }

    fn hit_distance(self, origin: Vec3f, direction: Vec3f) -> Option<f32> {
        let mut t_min = f32::NEG_INFINITY;
        let mut t_max = f32::INFINITY;
        for (origin_axis, direction_axis, min_axis, max_axis) in [
            (origin.x, direction.x, self.min.x, self.max.x),
            (origin.y, direction.y, self.min.y, self.max.y),
            (origin.z, direction.z, self.min.z, self.max.z),
        ] {
            if direction_axis.abs() <= 1.0e-6 {
                if origin_axis < min_axis || origin_axis > max_axis {
                    return None;
                }
                continue;
            }
            let inv_direction = direction_axis.recip();
            let t0 = (min_axis - origin_axis) * inv_direction;
            let t1 = (max_axis - origin_axis) * inv_direction;
            t_min = t_min.max(t0.min(t1));
            t_max = t_max.min(t0.max(t1));
            if t_max < t_min {
                return None;
            }
        }
        if !t_max.is_finite() || t_max < 0.0 {
            return None;
        }
        let entry = t_min.max(0.0);
        (entry.is_finite() && entry <= t_max).then_some(entry)
    }
}

fn synthetic_scene_boxes() -> Vec<SyntheticBox> {
    vec![
        SyntheticBox::new(vec3f(-2.60, -0.10, -5.20), vec3f(2.60, 0.0, 0.70)),
        SyntheticBox::new(vec3f(-2.60, 2.45, -5.20), vec3f(2.60, 2.55, 0.70)),
        SyntheticBox::new(vec3f(-2.60, 0.0, -5.20), vec3f(-2.50, 2.55, 0.70)),
        SyntheticBox::new(vec3f(2.50, 0.0, -5.20), vec3f(2.60, 2.55, 0.70)),
        SyntheticBox::new(vec3f(-2.60, 0.0, -5.20), vec3f(2.60, 2.55, -5.10)),
        SyntheticBox::new(vec3f(-0.45, 0.0, -2.65), vec3f(0.45, 1.15, -1.85)),
        SyntheticBox::new(vec3f(1.15, 0.0, -3.55), vec3f(1.85, 1.65, -2.95)),
        SyntheticBox::new(vec3f(-1.85, 0.0, -4.20), vec3f(-1.25, 0.95, -3.55)),
    ]
}

fn synthetic_camera_world(frame_index: usize, frame_count: usize) -> Vec3f {
    let t = if frame_count <= 1 {
        0.0
    } else {
        frame_index as f32 / (frame_count - 1) as f32
    };
    let x = (t * std::f32::consts::TAU * 1.35).sin() * 0.28;
    let y = 1.48 + (t * std::f32::consts::TAU * 2.10).sin() * 0.03;
    let z = 0.18 - t * 0.72;
    vec3f(x, y, z)
}

fn synthetic_depth_job(
    scene_boxes: &[SyntheticBox],
    width: u32,
    height: u32,
    generation: u64,
    voxel_size_meters: f32,
    camera_world: Vec3f,
) -> DepthMeshJob {
    let depth_proj = Mat4f::from_camera_fov(
        &CameraFov {
            angle_left: -0.92,
            angle_right: 0.92,
            angle_up: 0.72,
            angle_down: -0.72,
        },
        0.05,
        8.0,
    );
    let inv_depth_proj = depth_proj.invert();
    let world_from_depth_view = Mat4f::translation(camera_world);
    let depth_view_from_world = world_from_depth_view.invert();
    let depth = render_synthetic_depth(
        scene_boxes,
        width,
        height,
        camera_world,
        depth_proj,
        inv_depth_proj,
        world_from_depth_view,
        depth_view_from_world,
    );
    DepthMeshJob {
        reset_generation: 0,
        generation,
        eye_index: 0,
        width,
        height,
        voxel_size_meters,
        camera_world,
        depth_proj,
        inv_depth_proj,
        depth_view_from_world,
        world_from_depth_view,
        depth,
    }
}

fn render_synthetic_depth(
    scene_boxes: &[SyntheticBox],
    width: u32,
    height: u32,
    camera_world: Vec3f,
    depth_proj: Mat4f,
    inv_depth_proj: Mat4f,
    world_from_depth_view: Mat4f,
    depth_view_from_world: Mat4f,
) -> Vec<u16> {
    let mut depth = vec![0u16; width as usize * height as usize];
    for y in 0..height as usize {
        for x in 0..width as usize {
            let uv_x = (x as f32 + 0.5) / width as f32;
            let uv_y = (y as f32 + 0.5) / height as f32;
            let ndc_x = uv_x * 2.0 - 1.0;
            let ndc_y = uv_y * 2.0 - 1.0;
            let Some(ray_direction) =
                depth_ndc_to_world_ray(inv_depth_proj, world_from_depth_view, ndc_x, ndc_y)
            else {
                continue;
            };
            let nearest = scene_boxes
                .iter()
                .filter_map(|wall| wall.hit_distance(camera_world, ray_direction))
                .filter(|distance| {
                    distance.is_finite()
                        && *distance >= DEPTH_VOXEL_MIN_DISTANCE_METERS
                        && *distance <= DEPTH_VOXEL_MAX_DISTANCE_METERS
                })
                .min_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
            let Some(hit_distance) = nearest else {
                continue;
            };
            let world = camera_world + ray_direction.scale(hit_distance);
            let view = depth_view_from_world.transform_vec4(vec4f(world.x, world.y, world.z, 1.0));
            if !view.w.is_finite() || view.w.abs() < 1.0e-6 {
                continue;
            }
            let view = vec4f(view.x / view.w, view.y / view.w, view.z / view.w, 1.0);
            let clip = depth_proj.transform_vec4(view);
            if !clip.w.is_finite() || clip.w.abs() < 1.0e-6 {
                continue;
            }
            let ndc_z = clip.z / clip.w;
            let raw_depth = (ndc_z * 0.5 + 0.5).clamp(
                DEPTH_VOXEL_MIN_DEPTH_VALUE,
                DEPTH_VOXEL_MAX_DEPTH_VALUE - 1.0 / u16::MAX as f32,
            );
            depth[y * width as usize + x] = (raw_depth * u16::MAX as f32).round() as u16;
        }
    }
    depth
}

fn run_synthetic_tsdf_benchmark_case(
    voxel_size_meters: f32,
    width: u32,
    height: u32,
    frame_count: usize,
) -> SyntheticTsdfBenchStats {
    let scene_boxes = synthetic_scene_boxes();
    let mut worker_state = DepthPreprocessWorkerState::default();
    let mut volume = DepthMeshVolume::new(voxel_size_meters);
    let mut previous_snapshot = None::<Arc<TsdfPublishedSnapshot>>;

    let mut depth_build_total = std::time::Duration::ZERO;
    let mut preprocess_total = std::time::Duration::ZERO;
    let mut apply_total = std::time::Duration::ZERO;
    let mut publish_total = std::time::Duration::ZERO;
    let mut total_staged_samples = 0usize;
    let mut total_reduced_samples = 0usize;

    for frame_index in 0..frame_count {
        let camera_world = synthetic_camera_world(frame_index, frame_count);

        let started = std::time::Instant::now();
        let job = synthetic_depth_job(
            &scene_boxes,
            width,
            height,
            frame_index as u64 + 1,
            voxel_size_meters,
            camera_world,
        );
        depth_build_total += started.elapsed();

        let started = std::time::Instant::now();
        let prepared = preprocess_depth_mesh(job, &mut worker_state)
            .expect("synthetic depth preprocess should succeed");
        preprocess_total += started.elapsed();
        total_staged_samples += worker_state.staged_tsd_sample_visits();
        total_reduced_samples += worker_state.reduced_tsd_sample_count();

        let started = std::time::Instant::now();
        apply_preprocessed_depth_mesh(prepared, &worker_state, &mut volume);
        volume.discard_obsolete_surface_state();
        apply_total += started.elapsed();

        let started = std::time::Instant::now();
        let snapshot = volume.published_tsdf_snapshot(previous_snapshot.as_deref());
        publish_total += started.elapsed();
        volume.clear_published_tsdf_dirty_state();
        previous_snapshot = Some(Arc::new(snapshot));
    }

    let snapshot = previous_snapshot.expect("synthetic benchmark should publish a snapshot");
    let frames = frame_count as f64;
    SyntheticTsdfBenchStats {
        voxel_size_meters,
        frame_count,
        build_ms_per_frame: depth_build_total.as_secs_f64() * 1000.0 / frames,
        preprocess_ms_per_frame: preprocess_total.as_secs_f64() * 1000.0 / frames,
        apply_ms_per_frame: apply_total.as_secs_f64() * 1000.0 / frames,
        publish_ms_per_frame: publish_total.as_secs_f64() * 1000.0 / frames,
        staged_samples_per_frame: total_staged_samples as f64 / frames,
        reduced_samples_per_frame: total_reduced_samples as f64 / frames,
        chunk_count: snapshot.grid.chunk_count(),
        active_voxel_count: snapshot.grid.active_value_count,
        heap_mib: snapshot.grid.heap_bytes() as f64 / (1024.0 * 1024.0),
    }
}

#[test]
#[ignore = "Synthetic TSDF perf benchmark; run with cargo test --release -- --ignored --nocapture"]
fn synthetic_depthmap_tsdf_perf_release() {
    let width = 1280;
    let height = 1280;
    let frame_count = 6usize;

    for voxel_size_meters in [0.03, 0.05, 0.10] {
        let stats =
            run_synthetic_tsdf_benchmark_case(voxel_size_meters, width, height, frame_count);
        eprintln!(
            "synthetic_tsdf voxel={:.02} frames={} build_ms/frame={:.3} preprocess_ms/frame={:.3} apply_ms/frame={:.3} publish_ms/frame={:.3} staged/frame={:.0} reduced/frame={:.0} chunks={} active_voxels={} heap_mib={:.3}",
            stats.voxel_size_meters,
            stats.frame_count,
            stats.build_ms_per_frame,
            stats.preprocess_ms_per_frame,
            stats.apply_ms_per_frame,
            stats.publish_ms_per_frame,
            stats.staged_samples_per_frame,
            stats.reduced_samples_per_frame,
            stats.chunk_count,
            stats.active_voxel_count,
            stats.heap_mib,
        );

        assert!(stats.active_voxel_count > 0);
        assert!(stats.chunk_count > 0);
        assert!(stats.reduced_samples_per_frame > 256.0);
    }
}
