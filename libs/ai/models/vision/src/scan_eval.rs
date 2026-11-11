//! Host-side inputs and millimetre surface scores for synthetic MapAnything scans.

use crate::mapanything::{MapAnythingView, MapAnythingViewOutput, MAPANYTHING_SIZE};
use crate::mapanything_prep::{depth_z_to_along_ray, normalize_rgb8, pose_in_frame_of, rotation_to_quaternion};
use crate::scan_scenes::{error_stats, look_at, orbit, Camera, ErrorStats, Render, Shape};

/// The depth input a run gives the model, made from the true depth.
pub enum DepthInput {
    /// No depth input.
    None,
    /// Keep positive z depths on the stride grid, multiply by `1 + noise * u`,
    /// then convert to ray distance using the render's rays. All other pixels are zero.
    Sparse {
        /// Keep pixels where both x and y are multiples of this nonzero stride.
        stride: usize,
        /// Relative noise amplitude, with uniform `u` in [-1, 1].
        noise: f64,
        /// LCG seed, restarted per view; one draw per kept pixel in row-major order.
        seed: u64,
    },
}

/// Model inputs for the renders of one scene, one entry per view.
pub struct EvalViews {
    /// Planar RGB from `normalize_rgb8(render.rgb, 518, 518)`.
    pub images: Vec<Vec<f32>>,
    /// The render's rays, unchanged.
    pub rays: Vec<Vec<f32>>,
    /// Depth along each ray; absent for `DepthInput::None`.
    pub depth_along_ray: Vec<Option<Vec<f32>>>,
    /// Camera-to-world xyzw quaternion and translation in the scene's world frame.
    /// The model itself moves these into view 0's frame.
    pub poses: Vec<([f32; 4], [f32; 3])>,
}

fn check_render(render: &Render, pixels: usize) -> Result<(), String> {
    let triples = pixels.checked_mul(3).ok_or("image dimensions overflow usize")?;
    if render.rgb.len() != triples
        || render.rays.len() != triples
        || render.depth_z.len() != pixels
        || render.hit.len() != pixels
    {
        return Err("render buffer lengths do not match the image".into());
    }
    Ok(())
}

/// The evaluation's cameras: 518 px square, fx = fy = 400, principal point at the image centre (pixel
/// centres at x + 0.5), six views on an orbit of radius 0.40 m at elevation 0.25 over 120 degrees, looking at
/// a target 0.04 m above the ground. Every evaluation tool uses these, so their numbers compare.
pub fn eval_cameras() -> Vec<Camera> {
    let camera = Camera {
        width: MAPANYTHING_SIZE, height: MAPANYTHING_SIZE,
        fx: 400.0, fy: 400.0, cx: 259.0, cy: 259.0, skew: 0.0, distortion: [0.0; 5],
        cam_to_world: look_at([0.4, -0.25, 0.0], [0.0, -0.04, 0.0]),
    };
    orbit(&camera, 6, 0.40, 0.25, 0.04, 0.0, 2.0 * std::f64::consts::PI / 3.0, 0.0, 0.0, 1)
}

/// The evaluation's shapes, each named.
pub fn eval_shapes() -> Vec<(&'static str, Shape)> {
    vec![
        ("box", Shape::Box { half: [0.05, 0.04, 0.03] }),
        ("stepped", Shape::Stepped { base: [0.06, 0.02, 0.05], top: [0.03, 0.03, 0.025] }),
        ("sphere", Shape::Sphere { radius: 0.05 }),
        ("cylinder", Shape::Cylinder { radius: 0.04, height: 0.10 }),
    ]
}

/// The evaluation's input variants: name, depth input, and whether the poses are given.
/// Every variant gives the rays.
pub fn eval_variants() -> Vec<(&'static str, DepthInput, bool)> {
    vec![
        ("rays", DepthInput::None, false),
        ("rays+poses", DepthInput::None, true),
        ("rays+poses+depth", DepthInput::Sparse { stride: 8, noise: 0.01, seed: 7 }, true),
        ("rays+poses+fulldepth", DepthInput::Sparse { stride: 1, noise: 0.0, seed: 7 }, true),
    ]
}

/// Z depth kept where positive on the pixels whose x and y are both multiples of `stride`, each multiplied by
/// `1 + noise * u` with u in [-1, 1] from an LCG started at `seed` (one draw per kept pixel, row-major); zero
/// elsewhere. Rejects a zero stride or a buffer that is not `width * height` long.
pub fn sparse_depth_z(depth_z: &[f32], width: usize, height: usize, stride: usize, noise: f64, seed: u64) -> Result<Vec<f32>, String> {
    if stride == 0 {
        return Err("sparse depth stride must be nonzero".into());
    }
    if width.checked_mul(height) != Some(depth_z.len()) {
        return Err("depth buffer length does not match the image".into());
    }
    let mut state = seed;
    let mut sparse = vec![0.0; depth_z.len()];
    for y in (0..height).step_by(stride) {
        for x in (0..width).step_by(stride) {
            let i = y * width + x;
            if depth_z[i] > 0.0 {
                state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                let u = 2.0 * (state >> 11) as f64 / (1_u64 << 53) as f64 - 1.0;
                sparse[i] = (f64::from(depth_z[i]) * (1.0 + noise * u)) as f32;
            }
        }
    }
    Ok(sparse)
}

/// Prepares inputs, rejecting no views, mismatched camera/render counts, cameras
/// other than `MAPANYTHING_SIZE` square, mismatched buffers, and a zero stride.
pub fn eval_views(cameras: &[Camera], renders: &[Render], depth: &DepthInput) -> Result<EvalViews, String> {
    if cameras.is_empty() {
        return Err("at least one view is required".into());
    }
    if cameras.len() != renders.len() {
        return Err("camera and render counts differ".into());
    }
    if matches!(depth, DepthInput::Sparse { stride: 0, .. }) {
        return Err("sparse depth stride must be nonzero".into());
    }
    let pixels = MAPANYTHING_SIZE * MAPANYTHING_SIZE;
    let mut inputs = EvalViews {
        images: Vec::with_capacity(cameras.len()),
        rays: Vec::with_capacity(cameras.len()),
        depth_along_ray: Vec::with_capacity(cameras.len()),
        poses: Vec::with_capacity(cameras.len()),
    };
    for (index, (camera, render)) in cameras.iter().zip(renders).enumerate() {
        if camera.width != MAPANYTHING_SIZE || camera.height != MAPANYTHING_SIZE {
            return Err(format!("view {index}: camera must be {MAPANYTHING_SIZE}x{MAPANYTHING_SIZE}"));
        }
        check_render(render, pixels).map_err(|error| format!("view {index}: {error}"))?;
        inputs.images.push(normalize_rgb8(&render.rgb, MAPANYTHING_SIZE, MAPANYTHING_SIZE)?);
        inputs.rays.push(render.rays.clone());
        let depths = match *depth {
            DepthInput::None => None,
            DepthInput::Sparse { stride, noise, seed } => {
                let sparse = sparse_depth_z(&render.depth_z, MAPANYTHING_SIZE, MAPANYTHING_SIZE, stride, noise, seed)?;
                Some(depth_z_to_along_ray(&sparse, &render.rays)?)
            }
        };
        inputs.depth_along_ray.push(depths);
        let rotation = std::array::from_fn(|i| std::array::from_fn(|j| camera.cam_to_world[i][j] as f32));
        let translation = std::array::from_fn(|i| camera.cam_to_world[i][3] as f32);
        inputs.poses.push((rotation_to_quaternion(rotation), translation));
    }
    Ok(inputs)
}

impl EvalViews {
    /// Borrows image, rays and any depth for `MapAnything::forward`, with poses
    /// only when requested. All views are metric. Parallel fields must have one entry per view.
    pub fn views(&self, with_poses: bool) -> Vec<MapAnythingView<'_>> {
        self.images.iter().zip(&self.rays).zip(&self.depth_along_ray).zip(&self.poses)
            .map(|(((image, rays), depth), &pose)| MapAnythingView {
                image,
                rays: Some(rays),
                depth_along_ray: depth.as_deref(),
                pose: with_poses.then_some(pose),
                metric: true,
            })
            .collect()
    }
}

fn check_output(output: &MapAnythingViewOutput, pixels: usize) -> Result<(), String> {
    let triples = pixels.checked_mul(3).ok_or("image dimensions overflow usize")?;
    if output.rays.len() != triples || output.depth_along_ray.len() != pixels
        || output.confidence.len() != pixels || output.mask_logits.len() != pixels
    {
        return Err("output buffer lengths do not match the image".into());
    }
    Ok(())
}

/// Least-squares scale mapping predicted camera centres onto given centres in view 0's frame.
/// Uses `pose_in_frame_of` for the given translations and accumulates
/// `sum p . g / sum |p|^2` over views 1.. in f64.
/// Rejects differing counts, fewer than two views, or zero predicted squared length.
pub fn scale_from_camera_centres(
    given: &[([f32; 4], [f32; 3])],
    predicted: &[MapAnythingViewOutput],
) -> Result<f64, String> {
    if given.len() != predicted.len() {
        return Err("given and predicted camera counts differ".into());
    }
    if given.len() < 2 {
        return Err("at least two camera centres are required".into());
    }
    let (q0, t0) = given[0];
    let mut numerator = 0.0;
    let mut denominator = 0.0;
    for (&(q, t), output) in given.iter().zip(predicted).skip(1) {
        let (_, truth) = pose_in_frame_of(q0, t0, q, t);
        for (p, g) in output.trans.into_iter().zip(truth) {
            let p = f64::from(p);
            numerator += p * f64::from(g);
            denominator += p * p;
        }
    }
    if denominator == 0.0 {
        return Err("predicted camera centres have zero squared length".into());
    }
    Ok(numerator / denominator)
}

/// Least-squares scale mapping predicted points onto truth points at the same pixels.
/// Accumulates `sum p . g / sum |p|^2` in f64 over `hit == 1` and
/// `mask_logits > 0` pixels in all views. Predictions come from `points_world()`;
/// truth ray/z-depth points are moved through world space into view 0's frame.
/// Rejects count or buffer-length mismatches, no kept object pixels, or zero
/// predicted squared length.
pub fn scale_from_truth_points(
    cameras: &[Camera],
    renders: &[Render],
    outputs: &[MapAnythingViewOutput],
) -> Result<f64, String> {
    if cameras.len() != renders.len() || cameras.len() != outputs.len() {
        return Err("camera, render and output counts differ".into());
    }
    let camera0 = cameras.first().ok_or("at least one view is required")?;
    let mut numerator = 0.0;
    let mut denominator = 0.0;
    let mut kept = 0;
    for (index, ((camera, render), output)) in cameras.iter().zip(renders).zip(outputs).enumerate() {
        let pixels = camera.width.checked_mul(camera.height).ok_or("image dimensions overflow usize")?;
        check_render(render, pixels).map_err(|error| format!("view {index}: {error}"))?;
        check_output(output, pixels).map_err(|error| format!("view {index}: {error}"))?;
        let points = output.points_world();
        for (k, point) in points.chunks_exact(3).enumerate() {
            if !(render.hit[k] == 1 && output.mask_logits[k] > 0.0) {
                continue;
            }
            let ray = &render.rays[3 * k..3 * k + 3];
            let distance = f64::from(render.depth_z[k]) / f64::from(ray[2]);
            let local = [f64::from(ray[0]) * distance, f64::from(ray[1]) * distance, f64::from(ray[2]) * distance];
            let world: [f64; 3] = std::array::from_fn(|i| {
                let row = camera.cam_to_world[i];
                row[0] * local[0] + row[1] * local[1] + row[2] * local[2] + row[3]
            });
            // Inverse rigid transform: R0^T * (world - t0).
            for (i, &p) in point.iter().enumerate() {
                let g = (0..3).map(|j| camera0.cam_to_world[j][i] * (world[j] - camera0.cam_to_world[j][3])).sum::<f64>();
                let p = f64::from(p);
                numerator += p * g;
                denominator += p * p;
            }
            kept += 1;
        }
    }
    if kept == 0 {
        return Err("no kept object pixels for truth scale".into());
    }
    if denominator == 0.0 {
        return Err("predicted object points have zero squared length".into());
    }
    Ok(numerator / denominator)
}

/// Scales `output.points_world()` in view 0's frame, then maps into the scene's
/// world frame with camera 0's camera-to-world transform, in f64.
pub fn prediction_world_points(camera0: &Camera, output: &MapAnythingViewOutput, scale: f64) -> Vec<[f64; 3]> {
    output.points_world().chunks_exact(3).map(|p| {
        let p = [f64::from(p[0]) * scale, f64::from(p[1]) * scale, f64::from(p[2]) * scale];
        std::array::from_fn(|i| {
            let row = camera0.cam_to_world[i];
            row[0] * p[0] + row[1] * p[1] + row[2] * p[2] + row[3]
        })
    }).collect()
}

/// A score over object pixels.
pub struct ViewScore {
    /// Pixels where the truth render hit the object (`hit == 1`).
    pub object_pixels: usize,
    /// Object pixels kept by the prediction (`mask_logits > 0`).
    pub kept: usize,
    /// Surface errors over the kept predicted world points.
    pub error: ErrorStats,
}

/// Scores each view, then all kept points pooled in a single `error_stats` call.
/// Applies `scale` to the predictions in view 0's frame before scoring.
/// Rejects differing render/output counts or buffers that do not match the image.
pub fn score_views(
    shape: &Shape,
    camera0: &Camera,
    renders: &[Render],
    outputs: &[MapAnythingViewOutput],
    scale: f64,
) -> Result<(Vec<ViewScore>, ViewScore), String> {
    if renders.len() != outputs.len() {
        return Err("render and output counts differ".into());
    }
    let pixels = camera0.width.checked_mul(camera0.height).ok_or("image dimensions overflow usize")?;
    let mut scores = Vec::with_capacity(renders.len());
    let mut pooled = Vec::new();
    let mut object_pixels = 0;
    for (index, (render, output)) in renders.iter().zip(outputs).enumerate() {
        check_render(render, pixels).map_err(|error| format!("view {index}: {error}"))?;
        check_output(output, pixels).map_err(|error| format!("view {index}: {error}"))?;
        let start = pooled.len();
        let objects = render.hit.iter().filter(|&&hit| hit == 1).count();
        for ((point, &hit), &logit) in prediction_world_points(camera0, output, scale).into_iter().zip(&render.hit).zip(&output.mask_logits) {
            if hit == 1 && logit > 0.0 {
                pooled.push(point);
            }
        }
        let kept = pooled.len() - start;
        scores.push(ViewScore {
            object_pixels: objects,
            kept,
            error: error_stats(shape, &pooled[start..], &vec![true; kept]),
        });
        object_pixels += objects;
    }
    let all = ViewScore {
        object_pixels,
        kept: pooled.len(),
        error: error_stats(shape, &pooled, &vec![true; pooled.len()]),
    };
    Ok((scores, all))
}

/// Rotation error in degrees and Euclidean translation error in millimetres
/// between camera-to-world poses (xyzw quaternions, metres), evaluated in f64.
pub fn pose_error(truth: ([f32; 4], [f32; 3]), predicted: ([f32; 4], [f32; 3])) -> (f64, f64) {
    let [x, y, z, w] = truth.0.map(f64::from);
    let [a, b, c, d] = predicted.0.map(f64::from);
    // conjugate(truth) * predicted represents R_truth^T R_predicted.
    let relative = [w * a - x * d - y * c + z * b, w * b + x * c - y * d - z * a,
        w * c - x * b + y * a - z * d, w * d + x * a + y * b + z * c];
    let norm2 = relative.iter().map(|v| v * v).sum::<f64>();
    let vector2 = relative[..3].iter().map(|v| v * v).sum::<f64>();
    // (trace(R) - 1) / 2 = 1 - 2 * squared length of the unit quaternion's vector.
    let angle = (1.0 - 2.0 * vector2 / norm2).clamp(-1.0, 1.0).acos().to_degrees();
    let translation = truth.1.iter().zip(predicted.1).map(|(&t, p)| (f64::from(t) - f64::from(p)).powi(2)).sum::<f64>().sqrt() * 1000.0;
    (angle, translation)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mapanything_prep::{quaternion_multiply, quaternion_to_rotation};
    use crate::scan_scenes::{look_at, orbit, render, Scene};
    use std::sync::OnceLock;

    fn fixture() -> &'static (Vec<Camera>, Vec<Render>, Scene) {
        static FIXTURE: OnceLock<(Vec<Camera>, Vec<Render>, Scene)> = OnceLock::new();
        FIXTURE.get_or_init(|| {
            let camera = Camera { width: 518, height: 518, fx: 400.0, fy: 400.0, cx: 259.0, cy: 259.0,
                skew: 0.0, distortion: [0.0; 5], cam_to_world: look_at([0.4, -0.25, 0.0], [0.0, -0.04, 0.0]) };
            let cameras = orbit(&camera, 2, 0.40, 0.25, 0.04, 0.0, 2.0 * std::f64::consts::PI / 3.0, 0.0, 0.0, 1);
            let scene = Scene { shape: Shape::Box { half: [0.05, 0.04, 0.03] }, ground: true };
            let renders = cameras.iter().map(|camera| render(&scene, camera)).collect();
            (cameras, renders, scene)
        })
    }

    fn perfect_outputs(cameras: &[Camera], renders: &[Render]) -> Vec<MapAnythingViewOutput> {
        let inputs = eval_views(cameras, renders, &DepthInput::None).expect("fixture inputs");
        let (q0, t0) = inputs.poses[0];
        renders.iter().zip(inputs.poses).map(|(render, (q, t))| {
            let (quat, trans) = pose_in_frame_of(q0, t0, q, t);
            MapAnythingViewOutput {
                rays: render.rays.clone(),
                depth_along_ray: render.depth_z.iter().zip(render.rays.chunks_exact(3)).map(|(&z, ray)| z / ray[2]).collect(),
                confidence: vec![1.0; render.hit.len()],
                mask_logits: render.hit.iter().map(|&hit| if hit == 1 { 1.0 } else { -1.0 }).collect(),
                quat, trans,
            }
        }).collect()
    }

    #[test]
    fn sparse_depth_preserves_grid_ray_distance_and_seeded_noise_bounds() {
        let (cameras, renders, _) = fixture();
        let sparse = |noise, seed| eval_views(cameras, renders, &DepthInput::Sparse { stride: 8, noise, seed }).expect("fixture inputs");
        let clean = sparse(0.0, 1);
        let noisy1 = sparse(0.01, 1);
        let noisy2 = sparse(0.01, 2);
        assert_eq!(noisy1.depth_along_ray, sparse(0.01, 1).depth_along_ray, "seed is deterministic");
        for (view, render) in renders.iter().enumerate() {
            let clean = clean.depth_along_ray[view].as_ref().expect("sparse map");
            let noisy1 = noisy1.depth_along_ray[view].as_ref().expect("sparse map");
            let noisy2 = noisy2.depth_along_ray[view].as_ref().expect("sparse map");
            assert_ne!(noisy1, noisy2, "seeds change the depth map");
            let mut kept = 0;
            for i in 0..render.hit.len() {
                if i % 518 % 8 == 0 && i / 518 % 8 == 0 && render.depth_z[i] > 0.0 {
                    kept += 1;
                    let expected = render.depth_z[i] / render.rays[3 * i + 2];
                    assert!((clean[i] - expected).abs() <= 1e-6 * expected, "ray distance at {view}:{i}");
                    for noisy in [noisy1, noisy2] {
                        assert!((noisy[i] - clean[i]).abs() <= (0.01 + 1e-6) * clean[i], "noise bound at {view}:{i}");
                    }
                } else {
                    assert_eq!([clean[i], noisy1[i], noisy2[i]], [0.0; 3], "unkept pixel {view}:{i}");
                }
            }
            assert!(kept > 0, "nonempty sparse grid");
        }
    }

    #[test]
    fn images_are_planar_imagenet_and_poses_preserve_scene_cameras() {
        let (cameras, renders, _) = fixture();
        let inputs = eval_views(cameras, renders, &DepthInput::None).expect("fixture inputs");
        let pixels = 518 * 518;
        for (i, (camera, render)) in cameras.iter().zip(renders).enumerate() {
            assert_eq!(inputs.rays[i], render.rays, "rays unchanged");
            for pixel in [0, 259 * 518 + 259, pixels - 1] {
                for c in 0..3 {
                    let expected = (f64::from(render.rgb[3 * pixel + c]) / 255.0 - [0.485, 0.456, 0.406][c]) / [0.229, 0.224, 0.225][c];
                    assert!((f64::from(inputs.images[i][c * pixels + pixel]) - expected).abs() < 1e-6, "normalized channel {c}");
                }
            }
            let rotation = quaternion_to_rotation(inputs.poses[i].0);
            for (row, values) in rotation.iter().enumerate() {
                for (col, &value) in values.iter().enumerate() {
                    assert!((f64::from(value) - camera.cam_to_world[row][col]).abs() < 1e-6, "camera rotation");
                }
                assert!((f64::from(inputs.poses[i].1[row]) - camera.cam_to_world[row][3]).abs() < 1e-6, "camera translation");
            }
        }
    }

    #[test]
    fn views_select_poses_and_depth_and_always_use_metric_inputs() {
        let (cameras, renders, _) = fixture();
        for depth in [DepthInput::None, DepthInput::Sparse { stride: 8, noise: 0.0, seed: 1 }] {
            let inputs = eval_views(cameras, renders, &depth).expect("fixture inputs");
            for with_poses in [false, true] {
                let views = inputs.views(with_poses);
                assert_eq!(views.len(), cameras.len(), "one model view per render");
                for (i, view) in views.iter().enumerate() {
                    assert_eq!(view.pose, with_poses.then_some(inputs.poses[i]), "pose selection");
                    assert_eq!(view.depth_along_ray.is_none(), matches!(depth, DepthInput::None), "depth selection");
                    assert_eq!(view.depth_along_ray, inputs.depth_along_ray[i].as_deref(), "depth values");
                    assert_eq!(view.image, inputs.images[i], "image values");
                    assert_eq!(view.rays, Some(inputs.rays[i].as_slice()), "ray values");
                    assert!(view.metric, "metric inputs");
                }
            }
        }
    }

    #[test]
    fn perfect_prediction_scores_all_object_pixels_near_zero_in_world_frame() {
        let (cameras, renders, scene) = fixture();
        let outputs = perfect_outputs(cameras, renders);
        let (scores, all) = score_views(&scene.shape, &cameras[0], renders, &outputs, 1.0).expect("matching views");
        assert_eq!(all.object_pixels, scores.iter().map(|s| s.object_pixels).sum::<usize>(), "pooled objects");
        for (score, render) in scores.iter().zip(renders) {
            assert_eq!(score.object_pixels, render.hit.iter().filter(|&&hit| hit == 1).count(), "only object pixels");
        }
        for score in scores.iter().chain([&all]) {
            assert!(score.object_pixels > 0, "visible object");
            assert_eq!(score.kept, score.object_pixels, "all object points kept");
            assert_eq!(score.error.count, score.kept, "all kept points scored");
            assert!(score.error.mean_mm < 1e-3, "mean error: {:?}", score.error);
            assert!(score.error.max_mm < 1e-2, "max error: {:?}", score.error);
        }
    }

    #[test]
    fn scan_camera_centres_recover_twice_the_half_scale() {
        let (cameras, renders, _) = fixture();
        let inputs = eval_views(cameras, renders, &DepthInput::None).expect("fixture inputs");
        let mut outputs = perfect_outputs(cameras, renders);
        for output in &mut outputs {
            output.trans = output.trans.map(|value| value * 0.5);
        }
        let scale = scale_from_camera_centres(&inputs.poses, &outputs).expect("nonzero camera baseline");
        assert!((scale - 2.0).abs() < 1e-9, "half-size centres need scale 2: {scale}");
    }

    #[test]
    fn scan_truth_scale_recovers_quarter_scale_and_restores_surface_accuracy() {
        let (cameras, renders, scene) = fixture();
        let mut outputs = perfect_outputs(cameras, renders);
        for output in &mut outputs {
            output.trans = output.trans.map(|value| value * 0.25);
            for depth in &mut output.depth_along_ray { *depth *= 0.25; }
        }
        let scale = scale_from_truth_points(cameras, renders, &outputs).expect("kept object pixels");
        assert!((scale - 4.0).abs() < 1e-6, "quarter-size points need scale 4: {scale}");
        let (_, aligned) = score_views(&scene.shape, &cameras[0], renders, &outputs, 4.0).expect("matching views");
        let (_, raw) = score_views(&scene.shape, &cameras[0], renders, &outputs, 1.0).expect("matching views");
        assert!(aligned.error.mean_mm < 1e-3, "aligned mean error: {:?}", aligned.error);
        assert!(raw.error.mean_mm > 10.0, "raw mean error: {:?}", raw.error);
    }

    #[test]
    fn scan_camera_scale_rejects_single_view_and_zero_baseline() {
        let (cameras, renders, _) = fixture();
        let inputs = eval_views(cameras, renders, &DepthInput::None).expect("fixture inputs");
        let mut outputs = perfect_outputs(cameras, renders);
        assert!(scale_from_camera_centres(&inputs.poses[..1], &outputs[..1]).is_err(), "one view cannot determine scale");
        assert!(scale_from_camera_centres(&inputs.poses[..1], &outputs).is_err(), "camera counts must match");
        for output in &mut outputs { output.trans = [0.0; 3]; }
        assert!(scale_from_camera_centres(&inputs.poses, &outputs).is_err(), "zero baseline cannot determine scale");
    }

    #[test]
    fn rejected_masks_keep_no_points_and_produce_empty_errors() {
        let (cameras, renders, scene) = fixture();
        let mut outputs = perfect_outputs(cameras, renders);
        for output in &mut outputs { output.mask_logits.fill(-1.0); }
        assert!(scale_from_truth_points(cameras, renders, &outputs).is_err(), "no kept object points cannot determine scale");
        let (scores, all) = score_views(&scene.shape, &cameras[0], renders, &outputs, 1.0).expect("matching views");
        for score in scores.iter().chain([&all]) {
            assert!(score.object_pixels > 0, "truth object still visible");
            assert_eq!(score.kept, 0, "no kept pixels");
            assert_eq!(score.error.count, 0, "no scored points");
        }
    }

    #[test]
    fn pose_errors_measure_degrees_and_millimetres() {
        let (cameras, renders, _) = fixture();
        let inputs = eval_views(cameras, renders, &DepthInput::None).expect("fixture inputs");
        for &truth in &inputs.poses {
            let (rot, trans) = pose_error(truth, truth);
            assert!(rot.abs() < 1e-9 && trans.abs() < 1e-9, "identical poses: {rot}, {trans}");
            let (s, c) = (0.5_f64.to_radians()).sin_cos();
            let q = quaternion_multiply([0.0, 0.0, s as f32, c as f32], truth.0);
            let mut t = truth.1;
            t[0] += 0.001;
            let (rot, trans) = pose_error(truth, (q, t));
            assert!((rot - 1.0).abs() < 1e-4, "one degree: {rot}");
            assert!((trans - 1.0).abs() < 1e-4, "one millimetre: {trans}");
        }
    }

    #[test]
    fn scan_sparse_depth_keeps_positive_grid_pixels_and_is_shared_by_eval_views() {
        let depth: Vec<f32> = (0..20).map(|i| if i % 7 == 0 { 0.0 } else { 0.5 }).collect();
        let exact = sparse_depth_z(&depth, 5, 4, 2, 0.0, 3).expect("valid grid");
        for (i, (&kept, &truth)) in exact.iter().zip(&depth).enumerate() {
            let on_grid = (i % 5) % 2 == 0 && (i / 5) % 2 == 0;
            assert_eq!(kept, if on_grid { truth } else { 0.0 }, "pixel {i}");
        }
        let noisy = sparse_depth_z(&depth, 5, 4, 2, 0.01, 3).expect("valid grid");
        assert!(noisy.iter().zip(&exact).all(|(n, e)| (n - e).abs() <= 0.01 * e + 1e-7), "noise bound");
        assert!(sparse_depth_z(&depth, 5, 4, 0, 0.0, 3).is_err(), "zero stride");
        assert!(sparse_depth_z(&depth, 5, 5, 2, 0.0, 3).is_err(), "buffer length");
        let (cameras, renders, _) = fixture();
        let inputs = eval_views(cameras, renders, &DepthInput::Sparse { stride: 8, noise: 0.01, seed: 7 }).expect("inputs");
        let sparse = sparse_depth_z(&renders[0].depth_z, MAPANYTHING_SIZE, MAPANYTHING_SIZE, 8, 0.01, 7).expect("sparse");
        let along = depth_z_to_along_ray(&sparse, &renders[0].rays).expect("along ray");
        assert_eq!(inputs.depth_along_ray[0].as_deref(), Some(along.as_slice()), "eval_views uses the shared helper");
    }

    #[test]
    fn invalid_dimensions_stride_counts_and_buffers_are_rejected() {
        let (cameras, renders, scene) = fixture();
        let mut bad_cameras = cameras.clone();
        bad_cameras[0].width = 517;
        assert!(eval_views(&bad_cameras, renders, &DepthInput::None).is_err(), "wrong width");
        assert!(eval_views(cameras, renders, &DepthInput::Sparse { stride: 0, noise: 0.0, seed: 1 }).is_err(), "zero stride");
        assert!(eval_views(&cameras[..1], renders, &DepthInput::None).is_err(), "view counts");
        assert!(eval_views(&[], &[], &DepthInput::None).is_err(), "no views");
        let bad_render = Render { rgb: vec![], depth_z: vec![], rays: vec![], hit: vec![] };
        assert!(eval_views(&cameras[..1], std::slice::from_ref(&bad_render), &DepthInput::None).is_err(), "render buffers");
        let mut outputs = perfect_outputs(cameras, renders);
        assert!(scale_from_truth_points(&[], &[], &[]).is_err(), "no truth views");
        assert!(scale_from_truth_points(&cameras[..1], renders, &outputs).is_err(), "truth camera counts");
        assert!(scale_from_truth_points(cameras, renders, &outputs[..1]).is_err(), "truth output counts");
        assert!(scale_from_truth_points(&cameras[..1], std::slice::from_ref(&bad_render), &outputs[..1]).is_err(), "truth render buffers");
        assert!(score_views(&scene.shape, &cameras[0], renders, &outputs[..1], 1.0).is_err(), "output count");
        assert!(score_views(&scene.shape, &cameras[0], &[bad_render], &outputs[..1], 1.0).is_err(), "score render buffers");
        outputs[0].confidence.pop();
        assert!(scale_from_truth_points(cameras, renders, &outputs).is_err(), "truth output buffers");
        assert!(score_views(&scene.shape, &cameras[0], renders, &outputs, 1.0).is_err(), "output buffers");
    }
}
