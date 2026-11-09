//! Photometric pose refinement for calibrated views with PatchMatch depth and normal maps: the PatchMatch window
//! cost (one minus bilateral NCC, as [`ncc_cost`](crate::patchmatch::ncc_cost)) of every view's plane hypotheses
//! against its best sources, minimised over the poses and each sampled hypothesis's depth, with a Gaussian prior that
//! holds each pose near a given pose. Because PatchMatch minimises the same cost over the hypotheses, alternating the
//! two lowers one objective. Host only, f64, spread over the caller's threads with the same result at any thread
//! count.

use crate::{
    parallel::{fold_chunks, FanOut},
    patchmatch::{DepthNormalMap, PatchMatchParams},
    views::GrayView,
};

/// Settings of [`refine_poses`].
pub struct AlignParams {
    /// Most damped Gauss-Newton steps tried over all source selections (default 200); the solve normally ends on the
    /// tolerances first.
    pub max_steps: usize,
    /// Pixel stride of the sampled windows in each view (default 4).
    pub stride: usize,
    /// Hypotheses with a higher map cost are not sampled (default 0.3).
    pub max_cost: f64,
    /// Standard deviation of the image noise in grey levels of [0, 1] (default 0.004): a window's weight is its
    /// reference intensity variance over this squared, so the information it adds grows with its gradient energy and
    /// a flat window adds none.
    pub image_noise: f64,
    /// Scale of the Cauchy loss on each window's cost, in cost units (default 0.1).
    pub robust_scale: f64,
    /// Prior standard deviation of each pose's rotation from its prior pose, in radians (default 0.5 degrees).
    pub prior_rotation: f64,
    /// Prior standard deviation of each pose's translation from its prior pose about the pivot, in metres
    /// (default 0.005).
    pub prior_translation: f64,
    /// A solve on fixed sources ends when a step rotates no pose by more than this, in radians, and the whole
    /// refinement when a selection's solve does (default 1e-6) ...
    pub rotation_tolerance: f64,
    /// ... and moves no pose by more than this, in metres (default 1e-6).
    pub translation_tolerance: f64,
}

impl Default for AlignParams {
    fn default() -> Self {
        Self {
            max_steps: 200,
            stride: 4,
            max_cost: 0.3,
            image_noise: 0.004,
            robust_scale: 0.1,
            prior_rotation: 0.5_f64.to_radians(),
            prior_translation: 0.005,
            rotation_tolerance: 1e-6,
            translation_tolerance: 1e-6,
        }
    }
}

/// The result of [`refine_poses`].
pub struct Alignment {
    /// Refined row-major camera-to-world poses, one per view; the fixed view's is returned unchanged.
    pub cam_to_world: Vec<[[f64; 4]; 4]>,
    /// Accepted steps.
    pub steps: usize,
    /// Source selections (each followed by a solve with its sources fixed).
    pub selections: usize,
    /// Whether the refinement ended on the tolerances rather than on `max_steps`.
    pub converged: bool,
    /// Windows (sample, source pairs) selected at the starting poses ...
    pub start_windows: usize,
    /// ... and their mean cost.
    pub start_cost: f64,
    /// Windows selected at the refined poses and depths ...
    pub windows: usize,
    /// ... and their mean cost.
    pub cost: f64,
}

type Mat3 = [[f64; 3]; 3];

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn mul_vec(m: &Mat3, v: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|i| dot(m[i], v))
}

fn mul_transpose_vec(m: &Mat3, v: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|i| m[0][i] * v[0] + m[1][i] * v[1] + m[2][i] * v[2])
}

fn mul(a: &Mat3, b: &Mat3) -> Mat3 {
    std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| a[i][k] * b[k][j]).sum()))
}

fn mul_transpose(a: &Mat3, b: &Mat3) -> Mat3 {
    std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| a[i][k] * b[j][k]).sum()))
}

/// Rotation matrix of a rotation vector (Rodrigues).
fn exp_rotation(w: [f64; 3]) -> Mat3 {
    let angle = dot(w, w).sqrt();
    // Second-order series below 1e-8 rad, where the closed form loses precision.
    let (a, b) = if angle < 1e-8 {
        (1.0 - angle * angle / 6.0, 0.5 - angle * angle / 24.0)
    } else {
        (angle.sin() / angle, (1.0 - angle.cos()) / (angle * angle))
    };
    let k = [[0.0, -w[2], w[1]], [w[2], 0.0, -w[0]], [-w[1], w[0], 0.0]];
    let k2 = mul(&k, &k);
    std::array::from_fn(|i| std::array::from_fn(|j| f64::from(u8::from(i == j)) + a * k[i][j] + b * k2[i][j]))
}

/// Rotation vector of a rotation matrix, for angles below pi.
fn log_rotation(r: &Mat3) -> [f64; 3] {
    let skew = [r[2][1] - r[1][2], r[0][2] - r[2][0], r[1][0] - r[0][1]];
    let cosine = ((r[0][0] + r[1][1] + r[2][2] - 1.0) * 0.5).clamp(-1.0, 1.0);
    let angle = cosine.acos();
    let factor = if angle < 1e-8 { 0.5 + angle * angle / 12.0 } else { angle / (2.0 * angle.sin()) };
    skew.map(|v| v * factor)
}

#[derive(Clone, Copy)]
struct Pose {
    rotation: Mat3,
    centre: [f64; 3],
}

impl Pose {
    fn of(m: &[[f64; 4]; 4]) -> Self {
        Self { rotation: std::array::from_fn(|i| [m[i][0], m[i][1], m[i][2]]), centre: [m[0][3], m[1][3], m[2][3]] }
    }

    fn matrix(&self) -> [[f64; 4]; 4] {
        let r = &self.rotation;
        let c = self.centre;
        [[r[0][0], r[0][1], r[0][2], c[0]], [r[1][0], r[1][1], r[1][2], c[1]], [r[2][0], r[2][1], r[2][2], c[2]], [0.0, 0.0, 0.0, 1.0]]
    }

    /// The pose moved by the world-frame motion X -> pivot + exp(w)(X - pivot) + t.
    fn moved(&self, w: [f64; 3], t: [f64; 3], pivot: [f64; 3]) -> Self {
        let r = exp_rotation(w);
        let offset = mul_vec(&r, sub(self.centre, pivot));
        Self { rotation: mul(&r, &self.rotation), centre: std::array::from_fn(|i| pivot[i] + offset[i] + t[i]) }
    }

    /// The world-frame motion (w, t) about `pivot` that takes `start` to this pose.
    fn motion_from(&self, start: &Pose, pivot: [f64; 3]) -> ([f64; 3], [f64; 3]) {
        let r = mul_transpose(&self.rotation, &start.rotation);
        let offset = mul_vec(&r, sub(start.centre, pivot));
        (log_rotation(&r), std::array::from_fn(|i| self.centre[i] - pivot[i] - offset[i]))
    }
}

/// The camera ray through the (sub)pixel position (u, v), with camera z 1.
fn camera_ray(view: &GrayView, u: f64, v: f64) -> [f64; 3] {
    [(u - view.k[2]) / view.k[0], (v - view.k[3]) / view.k[1], 1.0]
}

fn to_world(pose: &Pose, p: [f64; 3]) -> [f64; 3] {
    let q = mul_vec(&pose.rotation, p);
    std::array::from_fn(|i| q[i] + pose.centre[i])
}

/// The bilinear sample at (u, v), as [`GrayView::sample`] computes it, and its gradient within the pixel cell;
/// None outside the closed image rectangle or for nonfinite positions.
fn bilinear(view: &GrayView, u: f64, v: f64) -> Option<(f64, [f64; 2])> {
    if !(u >= 0.0 && v >= 0.0 && u <= (view.width - 1) as f64 && v <= (view.height - 1) as f64) {
        return None;
    }
    let (x0, y0) = (u.floor() as usize, v.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(view.width - 1), (y0 + 1).min(view.height - 1));
    let pixel = |x: usize, y: usize| f64::from(view.pixels[y * view.width + x]);
    let (tx, ty) = (u - x0 as f64, v - y0 as f64);
    let (a, b, c, d) = (pixel(x0, y0), pixel(x1, y0), pixel(x0, y1), pixel(x1, y1));
    let value = (a * (1.0 - tx) + b * tx) * (1.0 - ty) + (c * (1.0 - tx) + d * tx) * ty;
    Some((value, [(1.0 - ty) * (b - a) + ty * (d - c), (1.0 - tx) * (c - a) + tx * (d - b)]))
}

/// A sampled window of one view's map and its fixed reference data. Its plane at parameters p = (l, a, b) passes
/// through (1 + l) `centre` with normal along `normal` + a `tangents[0]` + b `tangents[1]`.
struct Point {
    view: usize,
    /// Camera rays (camera z 1) of the window pixels.
    rays: Vec<[f64; 3]>,
    /// The camera-frame point at the map depth on the window's centre pixel ray.
    centre: [f64; 3],
    /// The unit map normal in the camera frame, and two unit tangents completing a right-handed basis.
    normal: [f64; 3],
    tangents: [[f64; 3]; 2],
    /// Bilateral weights over their sum.
    weights: Vec<f64>,
    /// Reference values minus their weighted mean, over their weighted standard deviation.
    reference: Vec<f64>,
    /// Data weight: the weighted reference variance over the image noise variance.
    weight: f64,
}

/// The sampled windows of every view: on the stride grid, inside the image, with a usable hypothesis (positive finite
/// depth, finite normal facing the camera across the window, cost <= `max_cost`) and a reference variance of at least
/// 1e-6 (below which [`ncc_cost`](crate::patchmatch::ncc_cost) rejects the window).
fn points(fan_out: FanOut, views: &[GrayView], maps: &[DepthNormalMap], window: &PatchMatchParams, params: &AlignParams) -> Vec<Point> {
    let r = window.radius;
    let offsets: Vec<f64> = (0..=2 * r).step_by(window.step).map(|o| o as f64 - r as f64).collect();
    let view_points = |k: usize| {
        let (view, map) = (&views[k], &maps[k]);
        let mut out = Vec::new();
        let pixel = |x: f64, y: f64| f64::from(view.pixels[y as usize * view.width + x as usize]);
        for y in (r..view.height.saturating_sub(r)).step_by(params.stride) {
            for x in (r..view.width.saturating_sub(r)).step_by(params.stride) {
                let index = y * view.width + x;
                let (depth, normal) = (map.depth[index], map.normal[index]);
                let length = dot(normal, normal).sqrt();
                if !(depth.is_finite() && depth > 0.0 && map.cost[index] <= params.max_cost && length.is_finite() && length > 0.0) {
                    continue;
                }
                let n = normal.map(|v| v / length);
                let centre = camera_ray(view, x as f64, y as f64).map(|c| c * depth);
                let (mut rays, mut weights, mut values) = (Vec::new(), Vec::new(), Vec::new());
                for &dy in &offsets {
                    for &dx in &offsets {
                        let (qx, qy) = (x as f64 + dx, y as f64 + dy);
                        let value = pixel(qx, qy);
                        let dc = value - pixel(x as f64, y as f64);
                        rays.push(camera_ray(view, qx, qy));
                        weights.push((-0.5 * ((dx / window.sigma_spatial).powi(2) + (dy / window.sigma_spatial).powi(2) + (dc / window.sigma_color).powi(2))).exp());
                        values.push(value);
                    }
                }
                // The plane must face the camera across the whole window.
                if !(dot(n, centre) < 0.0) || rays.iter().any(|&ray| !(dot(n, ray) < 0.0)) {
                    continue;
                }
                let total: f64 = weights.iter().sum();
                weights.iter_mut().for_each(|w| *w /= total);
                let mean: f64 = weights.iter().zip(&values).map(|(w, v)| w * v).sum();
                let variance = weights.iter().zip(&values).map(|(w, v)| w * v * v).sum::<f64>() - mean * mean;
                if !(variance >= 1e-6) {
                    continue;
                }
                let deviation = variance.sqrt();
                let axis = if n[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
                let t0 = cross(n, axis);
                let t0 = t0.map(|v| v / dot(t0, t0).sqrt());
                out.push(Point {
                    view: k,
                    rays,
                    centre,
                    normal: n,
                    tangents: [t0, cross(n, t0)],
                    weights,
                    reference: values.iter().map(|v| (v - mean) / deviation).collect(),
                    weight: variance / (params.image_noise * params.image_noise),
                });
            }
        }
        out
    };
    fold_chunks(fan_out, views.len(), 1, Vec::new(), |range| view_points(range.start), |all: &mut Vec<Point>, view| all.extend(view))
}

/// Columns of a window's Jacobian: reference rotation and translation, source rotation and translation, plane.
const COLUMNS: usize = 15;

/// The views and their poses a window is evaluated at, and the pivot the motions turn about.
#[derive(Clone, Copy)]
struct Frame<'a> {
    views: &'a [GrayView],
    poses: &'a [Pose],
    pivot: [f64; 3],
}

/// Buffers that window evaluations reuse: one set per chunk of windows, so evaluating allocates nothing once they
/// have grown. After an evaluation with the Jacobian, `residual` holds its residual e (cost = |e|^2 / 2) and
/// `jacobian` de/d[reference w, reference t, source w, source t, plane l, a, b] per window pixel, for world-frame
/// motions about the pivot.
#[derive(Default)]
struct Scratch {
    values: Vec<f64>,
    derivatives: Vec<[f64; COLUMNS]>,
    normalised: Vec<f64>,
    residual: Vec<f64>,
    jacobian: Vec<[f64; COLUMNS]>,
}

/// The cost (1 - NCC) of `point` with plane parameters `plane` against view `source`, with the residual and
/// Jacobian in `scratch` when `jacobian` is set; None when the plane does not face the reference camera across the
/// window, a window pixel falls behind the source camera or outside its image, or the source variance is below 1e-6.
fn evaluate(point: &Point, plane: [f64; 3], frame: &Frame, source: usize, jacobian: bool, scratch: &mut Scratch) -> Option<f64> {
    let Frame { views, poses, pivot } = *frame;
    let (pr, ps, view) = (&poses[point.view], &poses[source], &views[source]);
    let [t0, t1] = point.tangents;
    let m: [f64; 3] = std::array::from_fn(|i| point.normal[i] + plane[1] * t0[i] + plane[2] * t1[i]);
    let length = dot(m, m).sqrt();
    let normal = m.map(|v| v / length);
    let centre = point.centre.map(|c| c * (1.0 + plane[0]));
    let offset = dot(normal, centre);
    if !(1.0 + plane[0] > 0.0 && offset < 0.0) {
        return None;
    }
    let Scratch { values, derivatives, normalised, residual, jacobian: rows } = scratch;
    values.clear();
    derivatives.clear();
    for &ray in &point.rays {
        let facing = dot(normal, ray);
        if !(facing < 0.0) {
            return None;
        }
        let local = ray.map(|c| c * offset / facing);
        let world = to_world(pr, local);
        let y = mul_transpose_vec(&ps.rotation, sub(world, ps.centre));
        if !(y[2] > 0.0) {
            return None;
        }
        let (u, v) = (view.k[0] * y[0] / y[2] + view.k[2], view.k[1] * y[1] / y[2] + view.k[3]);
        let (value, gradient) = bilinear(view, u, v)?;
        values.push(value);
        if jacobian {
            // d(value)/d(source camera point), rotated to the world frame.
            let iz = 1.0 / y[2];
            let (gu, gv) = (gradient[0] * view.k[0] * iz, gradient[1] * view.k[1] * iz);
            let g = mul_vec(&ps.rotation, [gu, gv, -(gu * y[0] + gv * y[1]) * iz]);
            let ag = cross(sub(world, pivot), g);
            // The window point moves along its ray: with the depth in proportion, and by t . (centre - point) / (L n . ray)
            // for a tilt along tangent t.
            let along = |s: f64| dot(g, mul_vec(&pr.rotation, ray.map(|c| c * s)));
            let shift = sub(centre, local);
            derivatives.push([
                ag[0], ag[1], ag[2], g[0], g[1], g[2], -ag[0], -ag[1], -ag[2], -g[0], -g[1], -g[2],
                along(offset / facing / (1.0 + plane[0])),
                along(dot(t0, shift) / (facing * length)),
                along(dot(t1, shift) / (facing * length)),
            ]);
        }
    }
    let mean: f64 = point.weights.iter().zip(values.iter()).map(|(w, s)| w * s).sum();
    let variance = point.weights.iter().zip(values.iter()).map(|(w, s)| w * s * s).sum::<f64>() - mean * mean;
    if !(variance >= 1e-6) {
        return None;
    }
    let deviation = variance.sqrt();
    normalised.clear();
    normalised.extend(values.iter().map(|s| (s - mean) / deviation));
    let ncc: f64 = point.weights.iter().zip(normalised.iter()).zip(&point.reference).map(|((w, s), r)| w * s * r).sum();
    let cost = 1.0 - ncc;
    if !jacobian {
        return Some(cost);
    }
    residual.clear();
    residual.extend(point.weights.iter().zip(normalised.iter()).zip(&point.reference).map(|((w, s), r)| w.sqrt() * (s - r)));
    // d(normalised_k) = (d s_k - sum_j w_j d s_j - normalised_k sum_j w_j normalised_j d s_j) / deviation.
    let mut mean_derivative = [0.0; COLUMNS];
    let mut correlated = [0.0; COLUMNS];
    for ((w, s), d) in point.weights.iter().zip(normalised.iter()).zip(derivatives.iter()) {
        for c in 0..COLUMNS {
            mean_derivative[c] += w * d[c];
            correlated[c] += w * s * d[c];
        }
    }
    rows.clear();
    rows.extend(
        point
            .weights
            .iter()
            .zip(normalised.iter())
            .zip(derivatives.iter())
            .map(|((w, s), d)| std::array::from_fn(|c| w.sqrt() * (d[c] - mean_derivative[c] - s * correlated[c]) / deviation)),
    );
    Some(cost)
}

/// Cauchy loss of a window cost with scale b, and its derivative.
fn cauchy(cost: f64, b: f64) -> (f64, f64) {
    (b * (1.0 + cost / b).ln(), 1.0 / (1.0 + cost / b))
}

/// Solves the symmetric positive definite system a x = b in place by Cholesky; None if it is not positive definite.
fn solve_spd(n: usize, a: &mut [f64], b: &mut [f64]) -> Option<()> {
    for j in 0..n {
        let mut d = a[j * n + j];
        for k in 0..j {
            d -= a[j * n + k] * a[j * n + k];
        }
        if !(d > 0.0) {
            return None;
        }
        let d = d.sqrt();
        a[j * n + j] = d;
        for i in j + 1..n {
            let mut s = a[i * n + j];
            for k in 0..j {
                s -= a[i * n + k] * a[j * n + k];
            }
            a[i * n + j] = s / d;
        }
    }
    for i in 0..n {
        let mut s = b[i];
        for k in 0..i {
            s -= a[i * n + k] * b[k];
        }
        b[i] = s / a[i * n + i];
    }
    for i in (0..n).rev() {
        let mut s = b[i];
        for k in i + 1..n {
            s -= a[k * n + i] * b[k];
        }
        b[i] = s / a[i * n + i];
    }
    Some(())
}

/// Windows per chunk of the work spread over the threads.
const CHUNK: usize = 256;

/// What stays fixed during a refinement.
struct Problem<'a> {
    fan_out: FanOut<'a>,
    views: &'a [GrayView],
    points: Vec<Point>,
    priors: Vec<Pose>,
    fixed: usize,
    pivot: [f64; 3],
    /// The parameter block of each view; None for the fixed view.
    slots: Vec<Option<usize>>,
    /// Pose parameters: six per free view.
    n: usize,
    top_k: usize,
    params: &'a AlignParams,
}

/// The poses and each point's plane parameters.
#[derive(Clone)]
struct State {
    poses: Vec<Pose>,
    planes: Vec<[f64; 3]>,
}

/// Each point's best `top_k` sources at `state` (lowest window costs), their count and mean cost.
fn select(problem: &Problem, state: &State) -> (Vec<Vec<usize>>, usize, f64) {
    type Selection = (Vec<Vec<usize>>, usize, f64);
    let frame = Frame { views: problem.views, poses: &state.poses, pivot: problem.pivot };
    let chunk = |range: std::ops::Range<usize>| -> Selection {
        let (mut count, mut total) = (0, 0.0);
        let mut scratch = Scratch::default();
        let sources = range
            .map(|i| {
                let (point, plane) = (&problem.points[i], state.planes[i]);
                let mut costs: Vec<(f64, usize)> = (0..problem.views.len())
                    .filter(|&s| s != point.view)
                    .filter_map(|s| evaluate(point, plane, &frame, s, false, &mut scratch).map(|cost| (cost, s)))
                    .collect();
                costs.sort_by(|a, b| a.0.total_cmp(&b.0));
                costs.truncate(problem.top_k);
                count += costs.len();
                total += costs.iter().map(|c| c.0).sum::<f64>();
                costs.into_iter().map(|c| c.1).collect()
            })
            .collect();
        (sources, count, total)
    };
    let all: Selection = (Vec::with_capacity(problem.points.len()), 0, 0.0);
    let (sources, count, total) = fold_chunks(problem.fan_out, problem.points.len(), CHUNK, all, chunk, |all: &mut Selection, part| {
        all.0.extend(part.0);
        all.1 += part.1;
        all.2 += part.2;
    });
    (sources, count, if count == 0 { 0.0 } else { total / count as f64 })
}

fn prior_cost(problem: &Problem, poses: &[Pose]) -> f64 {
    let p = problem.params;
    poses
        .iter()
        .zip(&problem.priors)
        .enumerate()
        .filter(|&(k, _)| k != problem.fixed)
        .map(|(_, (pose, prior))| {
            let (w, t) = pose.motion_from(prior, problem.pivot);
            0.5 * (dot(w, w) / (p.prior_rotation * p.prior_rotation) + dot(t, t) / (p.prior_translation * p.prior_translation))
        })
        .sum()
}

/// The objective at `state`: each selected window's weighted robust cost (that of cost 2 when it cannot be
/// evaluated, so that no step gains by pushing windows out of the images) plus the prior.
fn objective(problem: &Problem, state: &State, sources: &[Vec<usize>]) -> f64 {
    let frame = Frame { views: problem.views, poses: &state.poses, pivot: problem.pivot };
    let chunk = |range: std::ops::Range<usize>| {
        let mut total = 0.0;
        let mut scratch = Scratch::default();
        for i in range {
            let (point, plane) = (&problem.points[i], state.planes[i]);
            for &source in &sources[i] {
                let cost = evaluate(point, plane, &frame, source, false, &mut scratch).unwrap_or(2.0);
                total += point.weight * cauchy(cost, problem.params.robust_scale).0;
            }
        }
        total
    };
    fold_chunks(problem.fan_out, problem.points.len(), CHUNK, 0.0, chunk, |total: &mut f64, part| *total += part) + prior_cost(problem, &state.poses)
}

/// Gauss-Newton normal equations (IRLS weights) at a state: the pose block, and per point its plane block, gradient
/// and nonzero pose couplings (pose index, d2/dpose dplane).
struct System {
    h: Vec<f64>,
    g: Vec<f64>,
    planes: Vec<([[f64; 3]; 3], [f64; 3], Vec<(usize, [f64; 3])>)>,
}

fn normal_equations(problem: &Problem, state: &State, sources: &[Vec<usize>]) -> System {
    let n = problem.n;
    let frame = Frame { views: problem.views, poses: &state.poses, pivot: problem.pivot };
    let chunk = |range: std::ops::Range<usize>| {
        let mut scratch = Scratch::default();
        let mut h = vec![0.0; n * n];
        let mut g = vec![0.0; n];
        let mut planes = Vec::with_capacity(range.len());
        let mut coupling = vec![[0.0; 3]; n];
        for index in range {
            let (point, list, plane) = (&problem.points[index], &sources[index], state.planes[index]);
            let (mut hpp, mut gp) = ([[0.0; 3]; 3], [0.0; 3]);
            coupling.iter_mut().for_each(|c| *c = [0.0; 3]);
            for &source in list {
                let Some(cost) = evaluate(point, plane, &frame, source, true, &mut scratch) else { continue };
                let weight = point.weight * cauchy(cost, problem.params.robust_scale).1;
                // Columns 0..6 move the reference view, 6..12 the source, 12..15 the plane.
                let mut columns = [None; 12];
                for i in 0..6 {
                    columns[i] = problem.slots[point.view].map(|b| 6 * b + i);
                    columns[6 + i] = problem.slots[source].map(|b| 6 * b + i);
                }
                let mut jtj = [[0.0; COLUMNS]; COLUMNS];
                let mut jte = [0.0; COLUMNS];
                for (row, &e) in scratch.jacobian.iter().zip(&scratch.residual) {
                    for a in 0..COLUMNS {
                        jte[a] += row[a] * e;
                        for b in a..COLUMNS {
                            jtj[a][b] += row[a] * row[b];
                        }
                    }
                }
                for a in 0..12 {
                    let Some(ia) = columns[a] else { continue };
                    g[ia] += weight * jte[a];
                    for p in 0..3 {
                        coupling[ia][p] += weight * jtj[a][12 + p];
                    }
                    for b in 0..12 {
                        let Some(ib) = columns[b] else { continue };
                        h[ia * n + ib] += weight * if a <= b { jtj[a][b] } else { jtj[b][a] };
                    }
                }
                for p in 0..3 {
                    gp[p] += weight * jte[12 + p];
                    for q in 0..3 {
                        let (a, b) = (12 + p.min(q), 12 + p.max(q));
                        hpp[p][q] += weight * jtj[a][b];
                    }
                }
            }
            let nonzero = coupling.iter().enumerate().filter(|(_, c)| c.iter().any(|v| *v != 0.0)).map(|(i, c)| (i, *c)).collect();
            planes.push((hpp, gp, nonzero));
        }
        System { h, g, planes }
    };
    let all = System { h: vec![0.0; n * n], g: vec![0.0; n], planes: Vec::with_capacity(problem.points.len()) };
    let System { mut h, mut g, planes } = fold_chunks(problem.fan_out, problem.points.len(), CHUNK, all, chunk, |all: &mut System, part| {
        all.h.iter_mut().zip(&part.h).for_each(|(a, b)| *a += b);
        all.g.iter_mut().zip(&part.g).for_each(|(a, b)| *a += b);
        all.planes.extend(part.planes);
    });
    let p = problem.params;
    let (sr, st) = (1.0 / (p.prior_rotation * p.prior_rotation), 1.0 / (p.prior_translation * p.prior_translation));
    for (k, pose) in state.poses.iter().enumerate() {
        let Some(b) = problem.slots[k] else { continue };
        let (w, t) = pose.motion_from(&problem.priors[k], problem.pivot);
        for i in 0..3 {
            h[(6 * b + i) * n + 6 * b + i] += sr;
            h[(6 * b + 3 + i) * n + 6 * b + 3 + i] += st;
            g[6 * b + i] += sr * w[i];
            g[6 * b + 3 + i] += st * t[i];
        }
    }
    System { h, g, planes }
}

/// The inverse of a symmetric 3x3 matrix; None unless it is positive definite.
fn inverse_spd3(a: &[[f64; 3]; 3]) -> Option<[[f64; 3]; 3]> {
    let c = [
        [a[1][1] * a[2][2] - a[1][2] * a[2][1], a[0][2] * a[2][1] - a[0][1] * a[2][2], a[0][1] * a[1][2] - a[0][2] * a[1][1]],
        [a[1][2] * a[2][0] - a[1][0] * a[2][2], a[0][0] * a[2][2] - a[0][2] * a[2][0], a[0][2] * a[1][0] - a[0][0] * a[1][2]],
        [a[1][0] * a[2][1] - a[1][1] * a[2][0], a[0][1] * a[2][0] - a[0][0] * a[2][1], a[0][0] * a[1][1] - a[0][1] * a[1][0]],
    ];
    let det = a[0][0] * c[0][0] + a[0][1] * c[1][0] + a[0][2] * c[2][0];
    let minor = a[0][0] * a[1][1] - a[0][1] * a[1][0];
    (a[0][0] > 0.0 && minor > 0.0 && det > 0.0).then(|| c.map(|row| row.map(|v| v / det)))
}

/// The Levenberg-Marquardt step with Marquardt damping `mu`, the planes eliminated by their Schur complement; None
/// when the reduced system is not positive definite. A point whose damped plane block is not positive definite keeps
/// its plane.
fn damped_step(problem: &Problem, system: &System, mu: f64) -> Option<(Vec<f64>, Vec<[f64; 3]>)> {
    let n = problem.n;
    let mut a = system.h.clone();
    for i in 0..n {
        a[i * n + i] += mu * system.h[i * n + i];
    }
    let b: Vec<f64> = system.g.iter().map(|v| -v).collect();
    type Reduced = (Vec<f64>, Vec<f64>, Vec<Option<Mat3>>);
    let chunk = |range: std::ops::Range<usize>| -> Reduced {
        let (mut a, mut b) = (vec![0.0; n * n], vec![0.0; n]);
        let inverses = system.planes[range]
            .iter()
            .map(|(hpp, gp, coupling)| {
                let mut damped = *hpp;
                for i in 0..3 {
                    damped[i][i] *= 1.0 + mu;
                }
                let inverse = inverse_spd3(&damped)?;
                let solved = mul_vec(&inverse, *gp);
                for &(i, ci) in coupling {
                    b[i] += dot(ci, solved);
                    let ci_inverse = mul_vec(&inverse, ci);
                    for &(j, cj) in coupling {
                        a[i * n + j] -= dot(ci_inverse, cj);
                    }
                }
                Some(inverse)
            })
            .collect();
        (a, b, inverses)
    };
    let all: Reduced = (a, b, Vec::with_capacity(system.planes.len()));
    let (mut a, mut b, inverses) = fold_chunks(problem.fan_out, system.planes.len(), CHUNK, all, chunk, |all: &mut Reduced, part| {
        all.0.iter_mut().zip(&part.0).for_each(|(x, y)| *x += y);
        all.1.iter_mut().zip(&part.1).for_each(|(x, y)| *x += y);
        all.2.extend(part.2);
    });
    solve_spd(n, &mut a, &mut b)?;
    let planes = system
        .planes
        .iter()
        .zip(&inverses)
        .map(|((_, gp, coupling), inverse)| match inverse {
            Some(inverse) => {
                let mut rhs = *gp;
                for &(i, ci) in coupling {
                    for p in 0..3 {
                        rhs[p] += ci[p] * b[i];
                    }
                }
                mul_vec(inverse, rhs).map(|v| -v)
            }
            None => [0.0; 3],
        })
        .collect();
    Some((b, planes))
}

/// Whether every pose of `a` is within the tolerances of the same pose of `b`.
fn within_tolerance(a: &[Pose], b: &[Pose], params: &AlignParams) -> bool {
    a.iter().zip(b).all(|(a, b)| {
        let w = log_rotation(&mul_transpose(&a.rotation, &b.rotation));
        let c = sub(a.centre, b.centre);
        dot(w, w).sqrt() <= params.rotation_tolerance && dot(c, c).sqrt() <= params.translation_tolerance
    })
}

fn rigid(m: &[[f64; 4]; 4]) -> bool {
    m.iter().flatten().all(|v| v.is_finite())
        && m[3] == [0.0, 0.0, 0.0, 1.0]
        && (0..3).all(|i| (0..3).all(|j| ((0..3).map(|k| m[k][i] * m[k][j]).sum::<f64>() - f64::from(u8::from(i == j))).abs() <= 1e-6))
}

/// Refines the camera-to-world poses of `views` (all but `fixed`, which fixes the frame) on the PatchMatch cost of
/// their maps (one per view: camera-z depth, camera-frame normals, costs), starting from the views' poses, with a
/// prior toward `priors` (one pose per view; the fixed view's is not used).
///
/// Samples are the windows of `window`'s radius and step on the `stride` grid of every view whose hypothesis is
/// usable (positive finite depth, a finite normal facing the camera across the window, map cost <= `max_cost`) and
/// whose reference values vary (weighted variance >= 1e-6). Each sample's plane, free in depth and tilt, is
/// compared with its best `window.top_k` sources by the bilateral NCC of [`ncc_cost`](crate::patchmatch::ncc_cost);
/// the sample weighs its reference variance over `image_noise` squared, so the information it adds grows with its
/// gradient energy. The objective is the weighted Cauchy loss (scale `robust_scale`) of the window costs plus a
/// Gaussian prior on each free pose's world-frame motion from its prior pose about a fixed pivot (the centroid of the
/// sampled window centres at the starting poses): `prior_rotation` and `prior_translation`, which also fix the scale, which the
/// images do not.
///
/// The refinement alternates selecting each sample's sources and Levenberg-Marquardt on the fixed selection
/// (Gauss-Newton on 1 - NCC = |e|^2 / 2 over the normalised windows, the planes eliminated per sample, Marquardt
/// damping; a step is kept only if it lowers the objective; a solve ends when a step moves every pose by at most the
/// tolerances). It converges when a selection's solve moves every pose by at most the tolerances, and otherwise
/// stops after `max_steps` tries.
///
/// The work is spread over `fan_out` in chunks of a fixed number of windows whose results are summed in chunk order,
/// so the result is the same, bit for bit, at any thread count.
///
/// Returns Err for unequal view, map and prior counts, `fixed` out of range, views failing [`GrayView::check`], maps
/// whose dimensions or buffer lengths do not match their view, priors that are not rigid, a zero stride, step or
/// `top_k`, or parameters that are not finite and positive (the tolerances and `max_cost` may be zero).
pub fn refine_poses(
    views: &[GrayView], maps: &[DepthNormalMap], fixed: usize, priors: &[[[f64; 4]; 4]], window: &PatchMatchParams,
    params: &AlignParams, fan_out: FanOut,
) -> Result<Alignment, String> {
    if views.len() != maps.len() || views.len() != priors.len() {
        return Err("view, map and prior counts do not match".into());
    }
    if fixed >= views.len() {
        return Err("fixed view index out of range".into());
    }
    if params.stride == 0 || window.step == 0 || window.top_k == 0 {
        return Err("stride, window step and top_k must be positive".into());
    }
    let positive = [params.image_noise, params.robust_scale, params.prior_rotation, params.prior_translation, window.sigma_spatial, window.sigma_color];
    let nonnegative = [params.max_cost, params.rotation_tolerance, params.translation_tolerance];
    if positive.iter().any(|v| !v.is_finite() || *v <= 0.0) || nonnegative.iter().any(|v| !v.is_finite() || *v < 0.0) {
        return Err("alignment parameters must be finite and positive".into());
    }
    for (view, map) in views.iter().zip(maps) {
        view.check()?;
        let size = view.pixels.len();
        if map.width != view.width || map.height != view.height || map.depth.len() != size
            || map.normal.len() != size || map.cost.len() != size
        {
            return Err("map dimensions or buffer lengths do not match their view".into());
        }
    }
    if !priors.iter().all(rigid) {
        return Err("priors must be rigid transforms".into());
    }
    let mut slots = vec![None; views.len()];
    let mut n = 0;
    for (k, slot) in slots.iter_mut().enumerate() {
        if k != fixed {
            *slot = Some(n / 6);
            n += 6;
        }
    }
    let poses: Vec<Pose> = views.iter().map(|v| Pose::of(&v.cam_to_world)).collect();
    let points = points(fan_out, views, maps, window, params);
    let mut pivot = [0.0; 3];
    for point in &points {
        let p = to_world(&poses[point.view], point.centre);
        for i in 0..3 {
            pivot[i] += p[i] / points.len() as f64;
        }
    }
    let problem = Problem {
        fan_out, views, priors: priors.iter().map(Pose::of).collect(), fixed, pivot, slots, n, top_k: window.top_k, params, points,
    };
    let mut state = State { planes: vec![[0.0; 3]; problem.points.len()], poses };
    let (mut sources, start_windows, start_cost) = select(&problem, &state);
    let (mut tries, mut steps, mut selections) = (0, 0, 0);
    let mut converged = n == 0;
    while !converged && tries < params.max_steps {
        if selections > 0 {
            sources = select(&problem, &state).0;
        }
        selections += 1;
        let before = state.poses.clone();
        let mut current = objective(&problem, &state, &sources);
        let mut system = normal_equations(&problem, &state, &sources);
        let mut mu = 1e-4;
        while tries < params.max_steps {
            tries += 1;
            let Some((step, planes)) = damped_step(&problem, &system, mu) else {
                mu *= 10.0;
                continue;
            };
            let candidate = State {
                poses: state
                    .poses
                    .iter()
                    .enumerate()
                    .map(|(k, pose)| match problem.slots[k] {
                        Some(b) => pose.moved([step[6 * b], step[6 * b + 1], step[6 * b + 2]], [step[6 * b + 3], step[6 * b + 4], step[6 * b + 5]], pivot),
                        None => *pose,
                    })
                    .collect(),
                planes: state.planes.iter().zip(&planes).map(|(p, d)| std::array::from_fn(|i| p[i] + d[i])).collect(),
            };
            let small = within_tolerance(&candidate.poses, &state.poses, params);
            let next = objective(&problem, &candidate, &sources);
            if next <= current {
                state = candidate;
                current = next;
                steps += 1;
                mu = (mu / 3.0).max(1e-12);
                if !small {
                    system = normal_equations(&problem, &state, &sources);
                }
            } else {
                mu *= 4.0;
            }
            // A small step ends the solve whether or not it was kept: a rejected one means no step lowers the cost.
            if small {
                break;
            }
        }
        converged = within_tolerance(&state.poses, &before, params);
    }
    let (_, windows, cost) = select(&problem, &state);
    Ok(Alignment {
        cam_to_world: state.poses.iter().map(Pose::matrix).collect(),
        steps,
        selections,
        converged,
        start_windows,
        start_cost,
        windows,
        cost,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parallel::{serial, tests::{reversed, threads}};
    use crate::patchmatch::{ncc_cost, plane_homography};
    use crate::synthetic::{plane_depth, textured_plane};

    const ANGLE_DEGREES: f64 = 30.0;

    fn uniform(state: &mut u64) -> f64 {
        *state = state.wrapping_add(0x9e3779b97f4a7c15);
        let mut z = *state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        z ^= z >> 31;
        (z >> 11) as f64 / 9007199254740992.0
    }

    fn direction(state: &mut u64) -> [f64; 3] {
        let z = 2.0 * uniform(state) - 1.0;
        let azimuth = 2.0 * std::f64::consts::PI * uniform(state);
        let r = (1.0 - z * z).sqrt();
        [r * azimuth.cos(), r * azimuth.sin(), z]
    }

    /// Five views of the synthetic textured plane, with exact depth and normal maps at the true poses.
    fn plane_scene() -> (Vec<GrayView>, Vec<DepthNormalMap>) {
        let angle = ANGLE_DEGREES.to_radians();
        let (views, _) = textured_plane(96, 120.0, angle, &[-0.10, 0.10, 0.05, -0.05]);
        let maps = views
            .iter()
            .map(|view| {
                let pose = Pose::of(&view.cam_to_world);
                let normal = mul_transpose_vec(&pose.rotation, [angle.sin(), 0.0, angle.cos()]);
                let normal = if normal[2] > 0.0 { normal.map(|v| -v) } else { normal };
                let size = view.width * view.height;
                DepthNormalMap { width: view.width, height: view.height, depth: plane_depth(view, angle), normal: vec![normal; size], cost: vec![0.0; size] }
            })
            .collect();
        (views, maps)
    }

    fn copy(views: &[GrayView]) -> Vec<GrayView> {
        views.iter().map(|v| GrayView { pixels: v.pixels.clone(), ..*v }).collect()
    }

    /// Every view but the first rotated by `degrees` about a random axis through its centre and moved by `mm` in a
    /// random direction.
    fn perturb(views: &mut [GrayView], mm: f64, degrees: f64, seed: u64) {
        let mut state = seed;
        for view in views.iter_mut().skip(1) {
            let axis = direction(&mut state);
            let shift = direction(&mut state);
            let pose = Pose::of(&view.cam_to_world);
            let moved = Pose {
                rotation: mul(&exp_rotation(axis.map(|v| v * degrees.to_radians())), &pose.rotation),
                centre: std::array::from_fn(|i| pose.centre[i] + shift[i] * mm / 1000.0),
            };
            view.cam_to_world = moved.matrix();
        }
    }

    /// The largest rotation (degrees) and camera-centre (mm) error against the truth.
    fn pose_error(poses: &[[[f64; 4]; 4]], truth: &[GrayView]) -> (f64, f64) {
        let (mut rotation, mut translation) = (0.0_f64, 0.0_f64);
        for (pose, view) in poses.iter().zip(truth) {
            let (p, t) = (Pose::of(pose), Pose::of(&view.cam_to_world));
            let w = log_rotation(&mul_transpose(&p.rotation, &t.rotation));
            let c = sub(p.centre, t.centre);
            rotation = rotation.max(dot(w, w).sqrt().to_degrees());
            translation = translation.max(dot(c, c).sqrt() * 1000.0);
        }
        (rotation, translation)
    }

    #[test]
    fn rotation_exp_and_log_are_inverse_and_poses_move_about_the_pivot() {
        for w in [[0.0, 0.0, 0.0], [1e-9, -2e-9, 0.5e-9], [0.3, -0.2, 0.1], [0.0, 2.5, 0.0]] {
            let back = log_rotation(&exp_rotation(w));
            assert!(sub(back, w).iter().all(|d| d.abs() < 1e-12), "{w:?} -> {back:?}");
        }
        let (views, _) = plane_scene();
        let start = Pose::of(&views[1].cam_to_world);
        let pivot = [0.02, 0.01, 0.4];
        let (w, t) = ([0.01, -0.02, 0.005], [0.001, 0.002, -0.003]);
        let (w2, t2) = start.moved(w, t, pivot).motion_from(&start, pivot);
        assert!(sub(w2, w).iter().chain(&sub(t2, t)).all(|d| d.abs() < 1e-12), "{w2:?} {t2:?}");
    }

    #[test]
    fn window_costs_match_the_patchmatch_cost() {
        let (views, maps) = plane_scene();
        let window_params = PatchMatchParams::default();
        let points = points(&serial, &views, &maps, &window_params, &AlignParams { stride: 7, ..AlignParams::default() });
        let poses: Vec<Pose> = views.iter().map(|v| Pose::of(&v.cam_to_world)).collect();
        let mut compared = 0;
        for point in &points {
            let (view, centre) = (&views[point.view], point.centre);
            let (x, y) = ((view.k[0] * centre[0] / centre[2] + view.k[2]).round() as usize, (view.k[1] * centre[1] / centre[2] + view.k[3]).round() as usize);
            let index = y * view.width + x;
            for source in (0..views.len()).filter(|&s| s != point.view) {
                let map = &maps[point.view];
                let h = plane_homography(view, &views[source], x, y, map.depth[index], map.normal[index]);
                let expected = ncc_cost(view, &views[source], &h, x, y, &window_params);
                let frame = Frame { views: &views, poses: &poses, pivot: [0.0; 3] };
                let got = evaluate(point, [0.0; 3], &frame, source, false, &mut Scratch::default());
                if expected < 2.0 {
                    let got = got.expect("a window PatchMatch scores is evaluated");
                    assert!((got - expected).abs() < 1e-5, "view {} ({x}, {y}) source {source}: {got} vs {expected}", point.view);
                    compared += 1;
                } else {
                    assert!(got.is_none(), "a window PatchMatch rejects is not evaluated");
                }
            }
        }
        assert!(compared > 500, "compared {compared} windows");
    }

    #[test]
    fn window_jacobian_matches_finite_differences() {
        let (truth, maps) = plane_scene();
        let mut views = copy(&truth);
        perturb(&mut views, 1.0, 0.1, 5);
        let window_params = PatchMatchParams::default();
        let points = points(&serial, &views, &maps, &window_params, &AlignParams { stride: 11, ..AlignParams::default() });
        let poses: Vec<Pose> = views.iter().map(|v| Pose::of(&v.cam_to_world)).collect();
        let pivot = [0.01, -0.02, 0.45];
        // Small enough that a window pixel rarely crosses a pixel edge, where the bilinear slope jumps.
        let step = 1e-8;
        let mut checked = 0;
        for point in points.iter().filter(|p| p.view == 1) {
            let plane = [0.002, 0.03, -0.02];
            let mut scratch = Scratch::default();
            let frame = Frame { views: &views, poses: &poses, pivot };
            if evaluate(point, plane, &frame, 2, true, &mut scratch).is_none() {
                continue;
            }
            let analytic: [f64; COLUMNS] = std::array::from_fn(|c| scratch.jacobian.iter().zip(&scratch.residual).map(|(j, e)| j[c] * e).sum());
            for c in 0..COLUMNS {
                let cost_at = |delta: f64| {
                    let mut moved = poses.clone();
                    let mut p = plane;
                    let mut motion = [0.0; 6];
                    if c < 12 {
                        motion[c % 6] = delta;
                    } else {
                        p[c - 12] += delta;
                    }
                    let view = if c < 6 { 1 } else { 2 };
                    if c < 12 {
                        moved[view] = poses[view].moved([motion[0], motion[1], motion[2]], [motion[3], motion[4], motion[5]], pivot);
                    }
                    evaluate(point, p, &Frame { views: &views, poses: &moved, pivot }, 2, false, &mut Scratch::default()).unwrap()
                };
                let numeric = (cost_at(step) - cost_at(-step)) / (2.0 * step);
                assert!((numeric - analytic[c]).abs() <= 1e-3 * analytic[c].abs().max(1.0), "column {c}: analytic {} numeric {numeric}", analytic[c]);
            }
            checked += 1;
        }
        assert!(checked > 10, "checked {checked} windows");
    }

    #[test]
    fn refinement_recovers_perturbed_poses_up_to_the_prior_scale() {
        let (truth, maps) = plane_scene();
        let mut views = copy(&truth);
        perturb(&mut views, 2.0, 0.2, 7);
        let priors: Vec<[[f64; 4]; 4]> = views.iter().map(|v| v.cam_to_world).collect();
        let before = pose_error(&priors, &truth);
        let params = AlignParams { stride: 2, ..AlignParams::default() };
        let result = refine_poses(&views, &maps, 0, &priors, &PatchMatchParams::default(), &params, &serial).unwrap();
        let after = pose_error(&result.cam_to_world, &truth);
        println!("ALIGN plane before_deg={:.5} before_mm={:.5} after_deg={:.5} after_mm={:.5} steps={} selections={} converged={} start_windows={} start_cost={:.5} windows={} cost={:.5}",
            before.0, before.1, after.0, after.1, result.steps, result.selections, result.converged, result.start_windows, result.start_cost,
            result.windows, result.cost);
        let (c0, t0) = (Pose::of(&result.cam_to_world[0]).centre, Pose::of(&truth[0].cam_to_world).centre);
        let pairs: Vec<([f64; 3], [f64; 3])> = result.cam_to_world.iter().zip(&truth)
            .map(|(r, t)| (sub(Pose::of(r).centre, c0), sub(Pose::of(&t.cam_to_world).centre, t0))).collect();
        let scale = pairs.iter().map(|(c, t)| dot(*c, *t)).sum::<f64>() / pairs.iter().map(|(c, _)| dot(*c, *c)).sum::<f64>();
        let scaled = pairs.iter().map(|(c, t)| { let d = sub(c.map(|v| v * scale), *t); dot(d, d).sqrt() * 1000.0 }).fold(0.0_f64, f64::max);
        println!("ALIGN plane scale={scale:.6} scaled_mm={scaled:.5}");
        assert!(result.converged, "the refinement converges");
        assert_eq!(result.cam_to_world[0], views[0].cam_to_world, "the fixed view does not move");
        assert!(result.cost < result.start_cost, "the window cost falls: {} -> {}", result.start_cost, result.cost);
        // Measured: 2 mm, 0.2 degrees -> 0.037 degrees, 1.0 mm, of which a 0.82% scale (the images do not fix
        // scale; the prior does, from the perturbed centres) leaves 0.34 mm; one plane leaves the rest loose. About a
        // third of the margin.
        assert!(after.0 < 0.4 * before.0 && scaled < 0.4 * before.1 && (scale - 1.0).abs() < 0.025, "{before:?} -> {after:?}, scale {scale}, {scaled} mm");
    }

    #[test]
    fn refinement_is_the_same_at_any_thread_count() {
        let (truth, maps) = plane_scene();
        let mut views = copy(&truth);
        perturb(&mut views, 2.0, 0.2, 3);
        let priors: Vec<[[f64; 4]; 4]> = views.iter().map(|v| v.cam_to_world).collect();
        let params = AlignParams { stride: 3, max_steps: 12, ..AlignParams::default() };
        let run = |fan_out: FanOut| {
            let a = refine_poses(&views, &maps, 0, &priors, &PatchMatchParams::default(), &params, fan_out).unwrap();
            (a.cam_to_world, a.steps, a.selections, a.start_windows, a.start_cost.to_bits(), a.windows, a.cost.to_bits())
        };
        let expected = run(&serial);
        assert!(expected.1 > 0 && expected.3 > 4 * CHUNK, "{} steps over {} windows", expected.1, expected.3);
        assert!(run(&reversed) == expected, "chunks folded out of arrival order");
        assert!(run(&threads(4)) == expected, "four threads");
    }

    #[test]
    fn invalid_arguments_are_errors() {
        let (views, maps) = plane_scene();
        let priors: Vec<[[f64; 4]; 4]> = views.iter().map(|v| v.cam_to_world).collect();
        let window = PatchMatchParams::default();
        let params = AlignParams::default();
        assert!(refine_poses(&views, &maps[..4], 0, &priors, &window, &params, &serial).is_err(), "map count");
        assert!(refine_poses(&views, &maps, 0, &priors[..4], &window, &params, &serial).is_err(), "prior count");
        assert!(refine_poses(&views, &maps, 5, &priors, &window, &params, &serial).is_err(), "fixed index");
        assert!(refine_poses(&views, &maps, 0, &priors, &window, &AlignParams { stride: 0, ..AlignParams::default() }, &serial).is_err(), "stride");
        assert!(refine_poses(&views, &maps, 0, &priors, &PatchMatchParams { top_k: 0, ..PatchMatchParams::default() }, &params, &serial).is_err(), "top_k");
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(refine_poses(&views, &maps, 0, &priors, &window, &AlignParams { image_noise: bad, ..AlignParams::default() }, &serial).is_err(), "noise {bad}");
            assert!(refine_poses(&views, &maps, 0, &priors, &window, &AlignParams { prior_translation: bad, ..AlignParams::default() }, &serial).is_err(), "prior {bad}");
        }
        let mut skewed = priors.clone();
        skewed[2][0][1] += 0.01;
        assert!(refine_poses(&views, &maps, 0, &skewed, &window, &params, &serial).is_err(), "non-rigid prior");
        let mut short: Vec<DepthNormalMap> = maps.iter().map(|m| DepthNormalMap { width: m.width, height: m.height, depth: m.depth.clone(),
            normal: m.normal.clone(), cost: m.cost.clone() }).collect();
        short[1].cost.pop();
        assert!(refine_poses(&views, &short, 0, &priors, &window, &params, &serial).is_err(), "buffer length");
        let one = refine_poses(&views[..1], &maps[..1], 0, &priors[..1], &window, &params, &serial).unwrap();
        assert!(one.converged && one.steps == 0 && one.cam_to_world[0] == views[0].cam_to_world, "a single view is already aligned");
    }
}
