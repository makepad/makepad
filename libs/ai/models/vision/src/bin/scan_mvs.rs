//! Run `scan-mvs [options] <model.safetensors> <model-id> <f32|bf16> <iterations>...` on a CUDA box to
//! score multi-view PatchMatch on the synthetic evaluation scans in mm. With `--supersample n` every pixel's colour
//! is the mean of n x n rays over the pixel (`render_supersampled`), for the model and PatchMatch alike; default 1.
//!
//! Every view is a reference against the other five. Each run starts from random hypotheses (`init=random`),
//! from the model's depth on rays + poses + sparse depth rescaled to the camera centres (`init=model`), or from
//! the true depth (`init=truth`); normals start from the starting depth's central differences. Depth maps are
//! lifted with the true poses and scored against the analytic shape over the object pixels of the truth render,
//! all of them and those whose photometric cost is below `KEEP_COST`. `INIT` lines score the starting depth the
//! same way, so the lines compare. After all six PatchMatch maps for an (init, iterations) pair are ready,
//! host geometric consistency fuses each view against the other five. `FUSED` lines pool fused object points
//! over all six views, reporting total object pixels, kept pixels and analytic shape errors in mm.
//! Options precede the positional arguments: `--supersample <n>`; `--surface <textured|uniform|glossy>` for the
//! object (default textured; uniform is one albedo with shading only, glossy adds a mild highlight); the PatchMatch window `--radius <n>`,
//! `--step <n>` and `--top-k <n>` (defaults as `PatchMatchParams`); the fusion gates `--min-views <n>` (agreeing
//! other views, default 2) and `--max-relative-depth <f>` (default as `ConsistencyParams`). A `SETTINGS` line
//! records them. `FUSED` lines also score the same points against the whole scene (the nearer of the object and the
//! ground plane y = 0, `scene_*`) and count `ground_px`, the fused object-pixel points nearer the ground than the
//! object: a pixel whose centre ray hits the object can still fuse, consistently across views, onto the ground.
//! `--pose-noise-mm <f>` and `--pose-noise-deg <f>` give the poses a tracker would report: every view but the first
//! is rotated by exactly that angle about a random axis through its centre and moved by exactly that distance in a
//! random direction (seeded; view 0 stays exact and fixes the frame). The model, PatchMatch, fusion and the lifting
//! all use these poses; renders and scoring use the true geometry.
//! `--refine-poses` alternates rounds after each PatchMatch pass: refine the poses on that pass's maps by the same
//! window cost (`refine_poses`: photometric, from the round's poses, with its prior on the tracked poses, view 0
//! fixed), then rerun PatchMatch and fusion with the refined poses. Every round writes `MVS`, `FUSED` and `POSE` lines
//! tagged `round=<r>` (round 0 at the tracked poses; `POSE` is each view's camera-centre error in mm and rotation
//! error in degrees against the truth), and an `ALIGN` line for its refinement, with the selected window count and
//! mean window cost before and after, and the largest pose change from the round's poses. Per shape,
//! `POSE <shape> maps=truth` first scores the refinement from the tracked poses on the true depth (normals from its
//! central differences): the floor without PatchMatch error. Refinement uses the PatchMatch window with every other
//! view as a source (`top_k` = 5). `--shapes <a,b,..>` and `--inits <a,b,..>` run only the named shapes and starts.
//! The rounds stop when a
//! refinement changes no pose by more than `--pose-step-mm <f>` and `--pose-step-deg <f>` (`stop=converged`; its
//! poses are then within the step of the last round's), or when a refinement's change, in units of those steps, is
//! no smaller than the one before (`stop=stalled`).
//! `--coarse-levels <n>` first runs such rounds on the views downsampled by 2^n, 2^(n-1), ... 2, without fusion: a
//! pose error moves a point by fewer of those pixels, so they correct errors too large for the full images. Their steps
//! scale with the pixels and the stride of their windows halves per level past the first, down to 1. A `COARSE` line
//! per round, a `POSE ... level=<l>` line per level; the rounds above then start from their poses.
//! `FILLED` lines follow each `FUSED` line: every view's fused map completed with the model's depth where fusion
//! kept nothing and the model depth has no edge (`complete_view`). A fused point's expected error is `fused_errors_cuda`'
//! with the PatchMatch window (the largest of its spread, the depth change that raises its cost measurably and, on an
//! edge window, the depth extrapolated over the window's lever), raised where it disagrees with the model by more than
//! both errors (the model's taken as the p95 over the whole view) and, beside an edge of the model's depth, to the jump
//! there; a model point's is the p95 of the model's relative distance from the fused points in the tiles near it times
//! its distance from the camera, or the model's depth jump there if larger, and a model point with no fused points near
//! enough gets none
//! (`unsupported_px`, over all pixels). Scored over the object pixels by source: the model points' errors
//! (`prior_*`), each source's median expected error, its share of points within it by the object error (`*_covered`)
//! and by the scene error (`*_covered_scene`), and `*_s95`, the p95 of object error over expected error (1 is
//! calibrated at 95%, 0.5 twice too loose); then the object pixels left empty and the p95 of object error over both
//! sources. The fused errors are measured on a thread per view; refinement runs on `--threads <n>` threads (default all
//! cores); its result is the same at any count, and `poses_hash` in the `ALIGN` line (FNV-1a over the bits of the
//! refined poses) shows it.

use makepad_ai_geometry::alignment::{refine_poses, AlignParams, Alignment};
use makepad_ai_geometry::parallel::FanOut;
use makepad_ai_geometry::cuda::{fused_errors_cuda, patchmatch_cuda};
use makepad_ai_geometry::fusion::{complete_view, fuse_view, CompletionParams, ConsistencyParams, FusedErrorParams, PointSource};
use makepad_ai_geometry::patchmatch::{DepthNormalMap, PatchMatchParams};
use makepad_ai_geometry::views::GrayView;
use makepad_ai_vision::mapanything::{MapAnything, MapAnythingPrecision};
use makepad_ai_vision::scan_eval::{eval_cameras, eval_shapes, eval_variants, eval_views, scale_from_camera_centres};
use makepad_ai_vision::scan_scenes::{distance_to_shape, error_stats, render_with, Camera, ErrorStats, Render, RenderOptions, Scene, Surface};
use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

const DEPTH_MIN: f64 = 0.15;
const DEPTH_MAX: f64 = 2.0;
const KEEP_COST: f64 = 0.3;
const POSE_NOISE_SEED: u64 = 11;
const POSE_STEP_MM: f64 = 0.01;
const POSE_STEP_DEG: f64 = 0.001;

/// SplitMix64 in [0, 1).
fn uniform(state: &mut u64) -> f64 {
    *state = state.wrapping_add(0x9e3779b97f4a7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
    z ^= z >> 31;
    (z >> 11) as f64 / 9007199254740992.0
}

/// A unit vector uniform on the sphere.
fn random_direction(state: &mut u64) -> [f64; 3] {
    let z = 2.0 * uniform(state) - 1.0;
    let azimuth = 2.0 * std::f64::consts::PI * uniform(state);
    let r = (1.0 - z * z).max(0.0).sqrt();
    [r * azimuth.cos(), r * azimuth.sin(), z]
}

/// The cameras with tracker-like pose error, as the doc comment describes.
fn tracked_cameras(cameras: &[Camera], mm: f64, degrees: f64) -> Vec<Camera> {
    let mut state = POSE_NOISE_SEED;
    let (sin, cos) = degrees.to_radians().sin_cos();
    cameras
        .iter()
        .enumerate()
        .map(|(index, camera)| {
            let mut camera = camera.clone();
            if index > 0 {
                let a = random_direction(&mut state);
                let shift = random_direction(&mut state);
                // Rodrigues: R = cos I + sin [a]x + (1 - cos) a a^T.
                let cross = [[0.0, -a[2], a[1]], [a[2], 0.0, -a[0]], [-a[1], a[0], 0.0]];
                let rotation: [[f64; 3]; 3] = std::array::from_fn(|i| {
                    std::array::from_fn(|j| (if i == j { cos } else { 0.0 }) + sin * cross[i][j] + (1.0 - cos) * a[i] * a[j])
                });
                let m = camera.cam_to_world;
                for i in 0..3 {
                    for j in 0..3 {
                        camera.cam_to_world[i][j] = (0..3).map(|k| rotation[i][k] * m[k][j]).sum();
                    }
                    camera.cam_to_world[i][3] = m[i][3] + shift[i] * mm / 1000.0;
                }
            }
            camera
        })
        .collect()
}
const MODEL_VARIANT: &str = "rays+poses+depth";

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn ray(view: &GrayView, x: usize, y: usize) -> [f64; 3] {
    [(x as f64 - view.k[2]) / view.k[0], (y as f64 - view.k[3]) / view.k[1], 1.0]
}

fn gray_view(camera: &Camera, render: &Render) -> Result<GrayView, String> {
    if camera.skew != 0.0 || camera.distortion != [0.0; 5] {
        return Err("PatchMatch views need undistorted cameras without skew".into());
    }
    let pixels = render
        .rgb
        .chunks_exact(3)
        .map(|p| ((0.299 * f32::from(p[0]) + 0.587 * f32::from(p[1]) + 0.114 * f32::from(p[2])) / 255.0).min(1.0))
        .collect();
    // The renders put pixel centres at x + 0.5; the views put them at x.
    Ok(GrayView { width: camera.width, height: camera.height, pixels, k: [camera.fx, camera.fy, camera.cx - 0.5, camera.cy - 0.5], cam_to_world: camera.cam_to_world })
}

/// Hypotheses from a camera-z depth map: depths clamped into the search range (the range's geometric mean where
/// the depth is missing), normals from central differences facing the camera, else fronto-parallel.
fn hypotheses(view: &GrayView, z: &[f64]) -> DepthNormalMap {
    let (w, h) = (view.width, view.height);
    let valid = |x: usize, y: usize| z[y * w + x].is_finite() && z[y * w + x] > 0.0;
    let point = |x: usize, y: usize| ray(view, x, y).map(|v| v * z[y * w + x]);
    let mut map = DepthNormalMap { width: w, height: h, depth: Vec::with_capacity(w * h), normal: Vec::with_capacity(w * h), cost: vec![2.0; w * h] };
    for y in 0..h {
        for x in 0..w {
            let r = ray(view, x, y);
            let length = dot(r, r).sqrt();
            let mut normal = r.map(|v| -v / length);
            if x > 0 && y > 0 && x + 1 < w && y + 1 < h && valid(x - 1, y) && valid(x + 1, y) && valid(x, y - 1) && valid(x, y + 1) {
                let c = cross(sub(point(x + 1, y), point(x - 1, y)), sub(point(x, y + 1), point(x, y - 1)));
                let l = dot(c, c).sqrt();
                if l.is_finite() && l > 0.0 {
                    let c = c.map(|v| v / l);
                    let c = if dot(c, r) > 0.0 { c.map(|v| -v) } else { c };
                    // A grazing normal is a depth edge, not a surface: keep the fronto-parallel one there.
                    if dot(c, r) < -0.05 * length {
                        normal = c;
                    }
                }
            }
            let d = z[y * w + x];
            map.depth.push(if valid(x, y) { d.clamp(DEPTH_MIN, DEPTH_MAX) } else { (DEPTH_MIN * DEPTH_MAX).sqrt() });
            map.normal.push(normal);
        }
    }
    map
}

/// World points of a view's object pixels from its depth, and whether each passes the cost threshold.
fn lift(view: &GrayView, render: &Render, depth: &[f64], cost: Option<&[f64]>, points: &mut Vec<[f64; 3]>, kept: &mut Vec<bool>) {
    let m = &view.cam_to_world;
    for (i, &hit) in render.hit.iter().enumerate() {
        if hit != 1 {
            continue;
        }
        let p = ray(view, i % view.width, i / view.width).map(|v| v * depth[i]);
        points.push(std::array::from_fn(|r| m[r][0] * p[0] + m[r][1] * p[1] + m[r][2] * p[2] + m[r][3]));
        kept.push(cost.is_none_or(|c| c[i] < KEEP_COST));
    }
}

fn stats(tag: &str, error: &ErrorStats) -> String {
    format!("{tag}px={} {tag}mean_mm={:.4} {tag}p95_mm={:.4} {tag}max_mm={:.4}", error.count, error.mean_mm, error.p95_mm, error.max_mm)
}

/// Mean, nearest-rank p95 and max of millimetre errors, as `error_stats` computes them; zeros for none.
fn summary(mut errors: Vec<f64>) -> (f64, f64, f64) {
    let count = errors.len();
    if count == 0 {
        return (0.0, 0.0, 0.0);
    }
    let mean = errors.iter().sum::<f64>() / count as f64;
    errors.sort_by(f64::total_cmp);
    (mean, errors[count - count / 20 - 1], errors[count - 1])
}

/// The camera-centre distance in mm and the rotation angle in degrees between two camera-to-world poses.
fn pose_difference(a: &[[f64; 4]; 4], b: &[[f64; 4]; 4]) -> (f64, f64) {
    let centre = 1000.0 * dot(sub([a[0][3], a[1][3], a[2][3]], [b[0][3], b[1][3], b[2][3]]), sub([a[0][3], a[1][3], a[2][3]], [b[0][3], b[1][3], b[2][3]])).sqrt();
    // The relative rotation A B^T: its trace and skew part give the angle without acos's loss near zero.
    let r: [[f64; 3]; 3] = std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| a[i][k] * b[j][k]).sum()));
    let skew = [r[2][1] - r[1][2], r[0][2] - r[2][0], r[1][0] - r[0][1]];
    let angle = (0.5 * dot(skew, skew).sqrt()).atan2(0.5 * (r[0][0] + r[1][1] + r[2][2] - 1.0));
    (centre, angle.to_degrees())
}

/// The scale that best maps the true camera centres to those of `poses`, both about the first view's, which the
/// images cannot fix: only the poses' priors do.
fn scale_of(poses: &[[[f64; 4]; 4]], cameras: &[Camera]) -> f64 {
    let centre = |m: &[[f64; 4]; 4]| [m[0][3], m[1][3], m[2][3]];
    let (first, truth) = (centre(&poses[0]), centre(&cameras[0].cam_to_world));
    let (top, bottom) = poses.iter().zip(cameras).fold((0.0, 0.0), |(top, bottom), (pose, camera)| {
        let (a, b) = (sub(centre(pose), first), sub(centre(&camera.cam_to_world), truth));
        (top + dot(a, b), bottom + dot(b, b))
    });
    top / bottom
}

/// Writes a `POSE` line: each view's pose error against the true camera, the poses' scale and the largest centre error
/// left at that scale.
fn write_pose(out: &mut impl Write, name: &str, tag: &str, poses: &[[[f64; 4]; 4]], cameras: &[Camera]) -> std::io::Result<()> {
    let scale = scale_of(poses, cameras);
    let centre = |m: &[[f64; 4]; 4]| [m[0][3], m[1][3], m[2][3]];
    let scaled_mm = poses.iter().zip(cameras).fold(0.0_f64, |most, (pose, camera)| {
        let (a, b) = (sub(centre(pose), centre(&poses[0])), sub(centre(&camera.cam_to_world), centre(&cameras[0].cam_to_world)));
        let left = sub(a, b.map(|v| v * scale));
        most.max(1000.0 * dot(left, left).sqrt())
    });
    let errors: Vec<(f64, f64)> = poses.iter().zip(cameras).map(|(p, c)| pose_difference(p, &c.cam_to_world)).collect();
    let list = |pick: fn(&(f64, f64)) -> f64| errors.iter().map(|e| format!("{:.4}", pick(e))).collect::<Vec<_>>().join(",");
    let (mm, deg) = errors.iter().fold((0.0_f64, 0.0_f64), |m, e| (m.0.max(e.0), m.1.max(e.1)));
    writeln!(out, "POSE {name} {tag} max_mm={mm:.4} max_deg={deg:.5} scale_ppm={:.1} scaled_max_mm={scaled_mm:.4} view_mm={} view_deg={}", 1e6 * (scale - 1.0), list(|e| e.0), list(|e| e.1))
}

/// The `ALIGN` line's fields for a refinement that took `seconds`, without the pose change.
fn align_fields(a: &Alignment, seconds: f64) -> String {
    let hash = a.cam_to_world.iter().flatten().flatten().fold(0xcbf29ce484222325_u64, |h, v| {
        v.to_bits().to_le_bytes().iter().fold(h, |h, &byte| (h ^ u64::from(byte)).wrapping_mul(0x100000001b3))
    });
    format!("steps={} selections={} converged={} start_windows={} start_cost={:.5} windows={} cost={:.5} align_s={seconds:.3} poses_hash={hash:016x}",
        a.steps, a.selections, a.converged, a.start_windows, a.start_cost, a.windows, a.cost)
}

/// A fan-out over `count` scoped threads taking indices in turn.
fn threads(count: usize) -> impl Fn(usize, &(dyn Fn(usize) + Sync)) + Sync {
    move |len, f| {
        let next = AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for _ in 0..count {
                scope.spawn(|| loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= len {
                        break;
                    }
                    f(i);
                });
            }
        });
    }
}

/// Every view but `reference` of `count`.
fn others(count: usize, reference: usize) -> Vec<usize> {
    (0..count).filter(|&s| s != reference).collect()
}

/// Camera-z depth `z` of an image `width` pixels wide with `factor` times fewer pixels a side, as
/// [`GrayView::downsampled`]: each new depth is the mean of its block's finite positive ones.
fn downsampled_depth(z: &[f64], width: usize, factor: usize) -> Vec<f64> {
    let height = z.len() / width;
    (0..height / factor)
        .flat_map(|y| (0..width / factor).map(move |x| (x * factor, y * factor)))
        .map(|(x, y)| {
            let block = (y..y + factor).flat_map(|row| &z[row * width + x..row * width + x + factor]).filter(|d| d.is_finite() && **d > 0.0);
            let (sum, count) = block.fold((0.0, 0), |(sum, count), d| (sum + d, count + 1));
            if count > 0 { sum / f64::from(count) } else { f64::NAN }
        })
        .collect()
}

/// What stays the same over the rounds of one shape, start and iteration count.
struct Case<'a> {
    name: &'a str,
    scene: &'a Scene,
    renders: &'a [Render],
    starts: Option<&'a Vec<DepthNormalMap>>,
    params: PatchMatchParams,
    consistency: &'a ConsistencyParams,
    /// The model's camera-z depth per view, which completes the fused views.
    prior: &'a [Vec<f64>],
    /// The true cameras, for the scale of the views' poses.
    cameras: &'a [Camera],
}

/// PatchMatch for every view against the other five at the views' poses, then fusion and its completion with the
/// model's depth; writes the `MVS`, `FUSED` and `FILLED` lines after `tag` and returns the maps.
fn reconstruct(out: &mut impl Write, case: &Case, tag: &str, views: &[GrayView]) -> Result<Vec<DepthNormalMap>, Box<dyn std::error::Error>> {
    let Case { name, scene, renders, starts, params, consistency, prior, cameras } = case;
    let (mut points, mut kept) = (Vec::new(), Vec::new());
    let (mut seconds, mut score_s) = (0.0, 0.0);
    let mut maps = Vec::with_capacity(views.len());
    for reference in 0..views.len() {
        let start = Instant::now();
        let map = patchmatch_cuda(views, reference, &others(views.len(), reference), starts.map(|s| &s[reference]), params)?;
        seconds += start.elapsed().as_secs_f64();
        let start = Instant::now();
        lift(&views[reference], &renders[reference], &map.depth, Some(&map.cost), &mut points, &mut kept);
        score_s += start.elapsed().as_secs_f64();
        maps.push(map);
    }
    let start = Instant::now();
    let all = error_stats(&scene.shape, &points, &vec![true; points.len()]);
    let confident = error_stats(&scene.shape, &points, &kept);
    score_s += start.elapsed().as_secs_f64();
    writeln!(out, "MVS {name} {tag} {} {} pm_s={seconds:.3}", stats("", &all), stats("kept_", &confident))?;
    let start = Instant::now();
    let fused_maps = (0..views.len())
        .map(|reference| fuse_view(views, &maps, reference, &others(views.len(), reference), consistency))
        .collect::<Result<Vec<_>, _>>()?;
    let fuse_s = start.elapsed().as_secs_f64();
    let start = Instant::now();
    let errors = fused_maps
        .iter()
        .enumerate()
        .map(|(reference, fused)| {
            fused_errors_cuda(views, reference, &others(views.len(), reference), &maps[reference], fused, params, &FusedErrorParams::default())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let errors_s = start.elapsed().as_secs_f64();
    let mut complete_s = 0.0;
    let mut fused_points = Vec::new();
    let error_mm = |p: [f64; 3]| 1000.0 * distance_to_shape(&scene.shape, p);
    let scene_mm = |p: [f64; 3]| error_mm(p).min(1000.0 * p[1].abs());
    // Per source: (object error, scene error, expected error) in mm of each object-pixel point.
    let (mut from_fusion, mut from_prior) = (Vec::new(), Vec::new());
    let (mut empty, mut unsupported) = (0, 0);
    for (reference, (fused, fused_error)) in fused_maps.iter().zip(&errors).enumerate() {
        let start = Instant::now();
        let completed = complete_view(&views[reference], fused, fused_error, &prior[reference], &CompletionParams::default())?;
        complete_s += start.elapsed().as_secs_f64();
        let start = Instant::now();
        unsupported += completed.unsupported;
        for (i, (&hit, point)) in renders[reference].hit.iter().zip(&fused.points).enumerate() {
            if hit != 1 {
                continue;
            }
            if let Some(point) = point { fused_points.push(*point); }
            match completed.points[i] {
                Some(c) => {
                    let scored = (error_mm(c.world), scene_mm(c.world), 1000.0 * c.expected_error);
                    if c.source == PointSource::Fused { from_fusion.push(scored) } else { from_prior.push(scored) }
                }
                None => empty += 1,
            }
        }
        score_s += start.elapsed().as_secs_f64();
    }
    let start = Instant::now();
    let fused = error_stats(&scene.shape, &fused_points, &vec![true; fused_points.len()]);
    let ground = fused_points.iter().filter(|&&p| p[1].abs() < distance_to_shape(&scene.shape, p)).count();
    let (scene_mean, scene_p95, scene_max) = summary(fused_points.iter().map(|&p| scene_mm(p)).collect());
    let poses: Vec<[[f64; 4]; 4]> = views.iter().map(|view| view.cam_to_world).collect();
    let (scale, first) = (scale_of(&poses, cameras), [poses[0][0][3], poses[0][1][3], poses[0][2][3]]);
    let (_, scaled_p95, _) = summary(fused_points.iter().map(|&p| error_mm(std::array::from_fn(|i| first[i] + (p[i] - first[i]) / scale))).collect());
    score_s += start.elapsed().as_secs_f64();
    writeln!(out, "FUSED {name} {tag} min_views={} px={} kept_px={} mean_mm={:.4} p95_mm={:.4} max_mm={:.4} ground_px={ground} scene_mean_mm={scene_mean:.4} scene_p95_mm={scene_p95:.4} scene_max_mm={scene_max:.4} scaled_p95_mm={scaled_p95:.4}",
        consistency.min_views, all.count, fused.count, fused.mean_mm, fused.p95_mm, fused.max_mm)?;
    // A source's median expected error, its shares of points within it by the object and the scene error, and the p95
    // of object error over expected error; zeros for none as in `summary`.
    let calibration = |tag: &str, errors: &[(f64, f64, f64)]| {
        let share = |pick: fn(&(f64, f64, f64)) -> f64| errors.iter().filter(|e| pick(e) <= e.2).count() as f64 / errors.len().max(1) as f64;
        let mut expected: Vec<f64> = errors.iter().map(|e| e.2).collect();
        expected.sort_by(f64::total_cmp);
        let (_, s95, _) = summary(errors.iter().map(|e| e.0 / e.2).collect());
        format!("{tag}px={} {tag}expected_p50_mm={:.4} {tag}covered={:.3} {tag}covered_scene={:.3} {tag}s95={s95:.3}",
            errors.len(), expected.get(expected.len() / 2).copied().unwrap_or(0.0), share(|e| e.0), share(|e| e.1))
    };
    let (prior_mean, prior_p95, prior_max) = summary(from_prior.iter().map(|e| e.0).collect());
    let (_, all_p95, _) = summary(from_fusion.iter().chain(&from_prior).map(|e| e.0).collect());
    writeln!(out, "FILLED {name} {tag} px={} {} {} prior_mean_mm={prior_mean:.4} prior_p95_mm={prior_p95:.4} prior_max_mm={prior_max:.4} empty_px={empty} unsupported_px={unsupported} all_p95_mm={all_p95:.4}",
        all.count, calibration("fused_", &from_fusion), calibration("prior_", &from_prior))?;
    writeln!(out, "TIME {name} {tag} pm_s={seconds:.3} fuse_s={fuse_s:.3} errors_s={errors_s:.3} complete_s={complete_s:.3} score_s={score_s:.3}")?;
    Ok(maps)
}

fn option_value<T: std::str::FromStr>(args: &mut impl Iterator<Item = std::ffi::OsString>, usage: &str) -> Result<T, String> {
    args.next().and_then(|a| a.into_string().ok()).and_then(|s| s.parse().ok()).ok_or_else(|| usage.to_string())
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let usage = "usage: scan-mvs [--supersample <n>] [--surface <textured|uniform|glossy>] [--radius <n>] [--step <n>] [--top-k <n>] [--min-views <n>] [--max-relative-depth <f>] [--pose-noise-mm <f>] [--pose-noise-deg <f>] [--coarse-levels <n>] [--refine-poses] [--pose-step-mm <f>] [--pose-step-deg <f>] [--shapes <a,b,..>] [--inits <model,truth,random>] [--threads <n>] <model.safetensors> <model-id> <f32|bf16> <iterations>...";
    let mut args = std::env::args_os().skip(1).peekable();
    let mut supersample: usize = 1;
    let mut surface = Surface::Textured;
    let (mut noise_mm, mut noise_deg) = (0.0_f64, 0.0_f64);
    let mut refine = false;
    let (mut shapes, mut inits): (Option<Vec<String>>, Option<Vec<String>>) = (None, None);
    let (mut step_mm, mut step_deg) = (POSE_STEP_MM, POSE_STEP_DEG);
    let mut thread_count = std::thread::available_parallelism()?.get();
    let align = AlignParams::default();
    let mut coarse_levels: usize = 0;
    let mut window = PatchMatchParams { depth_min: DEPTH_MIN, depth_max: DEPTH_MAX, ..Default::default() };
    let mut consistency = ConsistencyParams::default();
    while let Some(option) = args.peek().and_then(|a| a.to_str()).map(str::to_owned) {
        if !option.starts_with("--") {
            break;
        }
        args.next();
        match option.as_str() {
            "--supersample" => supersample = option_value(&mut args, usage)?,
            "--surface" => {
                surface = match option_value::<String>(&mut args, usage)?.as_str() {
                    "textured" => Surface::Textured,
                    "uniform" => Surface::Uniform,
                    "glossy" => Surface::Glossy,
                    _ => return Err(usage.into()),
                }
            }
            "--radius" => window.radius = option_value(&mut args, usage)?,
            "--step" => window.step = option_value(&mut args, usage)?,
            "--top-k" => window.top_k = option_value(&mut args, usage)?,
            "--min-views" => consistency.min_views = option_value(&mut args, usage)?,
            "--max-relative-depth" => consistency.max_relative_depth = option_value(&mut args, usage)?,
            "--pose-noise-mm" => noise_mm = option_value(&mut args, usage)?,
            "--pose-noise-deg" => noise_deg = option_value(&mut args, usage)?,
            "--refine-poses" => refine = true,
            "--coarse-levels" => coarse_levels = option_value(&mut args, usage)?,
            "--shapes" => shapes = Some(option_value::<String>(&mut args, usage)?.split(',').map(str::to_owned).collect()),
            "--inits" => inits = Some(option_value::<String>(&mut args, usage)?.split(',').map(str::to_owned).collect()),
            "--pose-step-mm" => step_mm = option_value(&mut args, usage)?,
            "--pose-step-deg" => step_deg = option_value(&mut args, usage)?,
            "--threads" => thread_count = option_value(&mut args, usage)?,
            _ => return Err(usage.into()),
        }
    }
    let positive = |v: f64| v.is_finite() && v > 0.0;
    if supersample == 0 || thread_count == 0 || !noise_mm.is_finite() || noise_mm < 0.0 || !noise_deg.is_finite() || noise_deg < 0.0 || !positive(step_mm) || !positive(step_deg) {
        return Err(usage.into());
    }
    let path = args.next().ok_or(usage)?;
    let model_id = args.next().and_then(|id| id.into_string().ok()).ok_or(usage)?;
    let precision = match args.next().as_deref().and_then(std::ffi::OsStr::to_str) {
        Some("f32") => MapAnythingPrecision::StrictF32,
        Some("bf16") => MapAnythingPrecision::Bf16,
        _ => return Err(usage.into()),
    };
    let iterations: Vec<usize> = args.map(|a| a.into_string().ok().and_then(|s| s.parse().ok()).ok_or(usage)).collect::<Result<_, _>>()?;
    if iterations.is_empty() || iterations.iter().any(|&n| n == 0 || n >= 0xffff) {
        return Err(usage.into());
    }
    let (_, model_depth, model_poses) = eval_variants().into_iter().find(|(name, ..)| *name == MODEL_VARIANT).ok_or("no model variant")?;
    let model = MapAnything::load(path, &model_id, precision, None)?;
    let cameras = eval_cameras();
    let tracked = tracked_cameras(&cameras, noise_mm, noise_deg);
    let mut out = std::io::stdout().lock();
    let fan_out = threads(thread_count);
    let fan_out: FanOut = &fan_out;
    let surface_name = match surface {
        Surface::Textured => "textured",
        Surface::Uniform => "uniform",
        Surface::Glossy => "glossy",
    };
    writeln!(out, "SETTINGS supersample={supersample} surface={surface_name} radius={} step={} top_k={} depth_min={DEPTH_MIN} depth_max={DEPTH_MAX} min_views={} max_relative_depth={} max_reprojection_px={} max_cost={} pose_noise_mm={noise_mm} pose_noise_deg={noise_deg} pose_noise_seed={POSE_NOISE_SEED} refine_poses={refine} pose_step_mm={step_mm} pose_step_deg={step_deg} align_stride={} align_max_cost={} align_image_noise={} align_robust_scale={} align_prior_deg={:.3} align_prior_mm={:.3} threads={thread_count}",
        window.radius, window.step, window.top_k, consistency.min_views, consistency.max_relative_depth, consistency.max_reprojection_px, consistency.max_cost,
        align.stride, align.max_cost, align.image_noise, align.robust_scale, align.prior_rotation.to_degrees(), 1000.0 * align.prior_translation)?;
    for (name, shape) in eval_shapes().into_iter().filter(|(name, _)| shapes.as_ref().is_none_or(|list| list.iter().any(|s| s == name))) {
        let scene = Scene { shape, ground: true };
        let options = RenderOptions { supersample, surface };
        let renders: Vec<_> = cameras.iter().map(|camera| render_with(&scene, camera, &options)).collect();
        let views = tracked.iter().zip(&renders).map(|(c, r)| gray_view(c, r)).collect::<Result<Vec<_>, _>>()?;
        let priors: Vec<[[f64; 4]; 4]> = views.iter().map(|v| v.cam_to_world).collect();
        // The poses are refined against every source a window sees, not only its best: the best agree with whatever
        // poses PatchMatch was given.
        let every_source = PatchMatchParams { top_k: views.len() - 1, ..window };
        let inputs = eval_views(&tracked, &renders, &model_depth)?;
        let start = Instant::now();
        let output = model.forward(&inputs.views(model_poses), None, None)?;
        let forward_s = start.elapsed().as_secs_f64();
        let scale = scale_from_camera_centres(&inputs.poses, &output.views)?;
        writeln!(out, "MODEL {name} {MODEL_VARIANT} supersample={supersample} scale={scale:.6} forward_s={forward_s:.3}")?;
        let truth_z: Vec<Vec<f64>> = renders.iter().map(|r| r.depth_z.iter().map(|&d| f64::from(d)).collect()).collect();
        let model_z: Vec<Vec<f64>> = output
            .views
            .iter()
            .map(|v| v.depth_along_ray.iter().zip(v.rays.chunks_exact(3)).map(|(&d, r)| scale * f64::from(d) * f64::from(r[2])).collect())
            .collect();
        if refine {
            let truth_maps: Vec<DepthNormalMap> = views
                .iter()
                .zip(&truth_z)
                .zip(&renders)
                .map(|((view, z), render)| DepthNormalMap {
                    // Only depths inside the search range are kept: `hypotheses` clamps the rest.
                    cost: render.hit.iter().zip(z).map(|(&h, &d)| if h != 0 && (DEPTH_MIN..=DEPTH_MAX).contains(&d) { 0.0 } else { 2.0 }).collect(),
                    ..hypotheses(view, z)
                })
                .collect();
            let start = Instant::now();
            let alignment = refine_poses(&views, &truth_maps, 0, &priors, &every_source, &align, fan_out)?;
            writeln!(out, "ALIGN {name} maps=truth {}", align_fields(&alignment, start.elapsed().as_secs_f64()))?;
            write_pose(&mut out, name, "maps=truth", &alignment.cam_to_world, &cameras)?;
        }
        for (init, z) in [("model", Some(&model_z)), ("truth", Some(&truth_z)), ("random", None)] {
            if inits.as_ref().is_some_and(|list| !list.iter().any(|i| i == init)) {
                continue;
            }
            let starts: Option<Vec<DepthNormalMap>> = z.map(|z| views.iter().zip(z).map(|(v, z)| hypotheses(v, z)).collect());
            if let Some(starts) = &starts {
                let (mut points, mut kept) = (Vec::new(), Vec::new());
                for ((view, render), start) in views.iter().zip(&renders).zip(starts) {
                    lift(view, render, &start.depth, None, &mut points, &mut kept);
                }
                writeln!(out, "INIT {name} init={init} {}", stats("", &error_stats(&scene.shape, &points, &kept)))?;
            }
            for &n in &iterations {
                let case = Case { name, scene: &scene, renders: &renders, starts: starts.as_ref(), params: PatchMatchParams { iterations: n, ..window }, consistency: &consistency, prior: &model_z, cameras: &cameras };
                if !refine {
                    reconstruct(&mut out, &case, &format!("init={init} it={n}"), &views)?;
                    continue;
                }
                let mut current: Vec<GrayView> = views.iter().map(|v| GrayView { pixels: v.pixels.clone(), ..*v }).collect();
                for level in (1..=coarse_levels).rev() {
                    let factor = 1 << level;
                    let mut small = current.iter().map(|v| v.downsampled(factor)).collect::<Result<Vec<_>, _>>()?;
                    let small_starts: Option<Vec<DepthNormalMap>> = z.map(|z| small.iter().zip(z).map(|(v, z)| hypotheses(v, &downsampled_depth(z, current[0].width, factor))).collect());
                    let coarse_align = AlignParams { stride: (align.stride >> (level - 1)).max(1), ..align.downsampled(factor) };
                    let params = PatchMatchParams { iterations: n, ..window };
                    let scale = factor as f64;
                    let mut last_change = f64::INFINITY;
                    for round in 0.. {
                        let tag = format!("init={init} it={n} level={level} round={round}");
                        let start = Instant::now();
                        let maps = (0..small.len())
                            .map(|reference| patchmatch_cuda(&small, reference, &others(small.len(), reference), small_starts.as_ref().map(|s| &s[reference]), &params))
                            .collect::<Result<Vec<_>, _>>()?;
                        let pm_s = start.elapsed().as_secs_f64();
                        let start = Instant::now();
                        let alignment = refine_poses(&small, &maps, 0, &priors, &every_source, &coarse_align, fan_out)?;
                        let align_s = start.elapsed().as_secs_f64();
                        let (moved_mm, moved_deg) = alignment.cam_to_world.iter().zip(&small)
                            .map(|(a, v)| pose_difference(a, &v.cam_to_world))
                            .fold((0.0_f64, 0.0_f64), |m, e| (m.0.max(e.0), m.1.max(e.1)));
                        let change = (moved_mm / (scale * step_mm)).max(moved_deg / (scale * step_deg));
                        let stop = if change <= 1.0 { "converged" } else if change >= last_change { "stalled" } else { "continue" };
                        writeln!(out, "COARSE {name} {tag} {} pm_s={pm_s:.3} moved_mm={moved_mm:.4} moved_deg={moved_deg:.5} stop={stop}", align_fields(&alignment, align_s))?;
                        if stop != "stalled" {
                            for ((small, view), pose) in small.iter_mut().zip(&mut current).zip(alignment.cam_to_world) {
                                small.cam_to_world = pose;
                                view.cam_to_world = pose;
                            }
                        }
                        if stop != "continue" {
                            break;
                        }
                        last_change = change;
                    }
                    write_pose(&mut out, name, &format!("init={init} it={n} level={level}"), &current.iter().map(|v| v.cam_to_world).collect::<Vec<_>>(), &cameras)?;
                }
                let mut last_change = f64::INFINITY;
                for round in 0.. {
                    let tag = format!("init={init} it={n} round={round}");
                    let maps = reconstruct(&mut out, &case, &tag, &current)?;
                    write_pose(&mut out, name, &tag, &current.iter().map(|v| v.cam_to_world).collect::<Vec<_>>(), &cameras)?;
                    let start = Instant::now();
                    let alignment = refine_poses(&current, &maps, 0, &priors, &every_source, &align, fan_out)?;
                    let align_s = start.elapsed().as_secs_f64();
                    let (moved_mm, moved_deg) = alignment.cam_to_world.iter().zip(&current)
                        .map(|(a, v)| pose_difference(a, &v.cam_to_world))
                        .fold((0.0_f64, 0.0_f64), |m, e| (m.0.max(e.0), m.1.max(e.1)));
                    let change = (moved_mm / step_mm).max(moved_deg / step_deg);
                    let stop = if change <= 1.0 { "converged" } else if change >= last_change { "stalled" } else { "continue" };
                    writeln!(out, "ALIGN {name} {tag} {} moved_mm={moved_mm:.4} moved_deg={moved_deg:.5} stop={stop}", align_fields(&alignment, align_s))?;
                    if stop != "continue" {
                        break;
                    }
                    last_change = change;
                    for (view, pose) in current.iter_mut().zip(alignment.cam_to_world) {
                        view.cam_to_world = pose;
                    }
                }
            }
        }
    }
    out.flush()?;
    Ok(())
}

fn main() {
    makepad_ai_common::backend::metal::run_model_worker("scan_mvs", model_main);
}

fn model_main() {
    if let Err(error) = run() {
        eprintln!("scan-mvs: {error}");
        std::process::exit(1);
    }
}
