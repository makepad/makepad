//! Deterministic, serial PatchMatch with f64 geometry and f32 grayscale images.
//! OpenCV cameras have x right, y down, z forward; pixel centres are (x, y),
//! intrinsics [fx, fy, cx, cy], and row-major rigid camera-to-world poses in
//! metres. Depth is camera z. Normals are unit vectors in the reference camera
//! frame, facing it (n dot K^-1 [x, y, 1] < 0). Images are row-major in [0, 1].

use crate::views::{relative_pose, GrayView};

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn ray(view: &GrayView, x: usize, y: usize) -> [f64; 3] {
    [(x as f64 - view.k[2]) / view.k[0], (y as f64 - view.k[3]) / view.k[1], 1.0]
}

fn normalise(v: [f64; 3]) -> [f64; 3] {
    let length = dot(v, v).sqrt();
    v.map(|x| x / length)
}

fn facing(mut n: [f64; 3], direction: [f64; 3]) -> [f64; 3] {
    if dot(n, direction) >= 0.0 {
        n = n.map(|v| -v);
    }
    n
}

/// Plane homography from reference to source pixels. For the plane through
/// X = depth K_r^-1 [x, y, 1] with normal n, delta = -n dot X and
/// H = K_s (R - t n^T / delta) K_r^-1, using `relative_pose` for (R, t).
/// The result is row-major f64; inputs must describe a nondegenerate plane.
pub fn plane_homography(
    reference: &GrayView, source: &GrayView, x: usize, y: usize,
    depth: f64, normal: [f64; 3],
) -> [[f64; 3]; 3] {
    let (r, t) = relative_pose(reference, source);
    let delta = -depth * dot(normal, ray(reference, x, y));
    let mut a = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            a[i][j] = r[i][j] - t[i] * normal[j] / delta;
        }
    }
    // Right multiplication by K_r^-1, then left multiplication by K_s.
    for row in &mut a {
        row[0] /= reference.k[0];
        row[1] /= reference.k[1];
        row[2] -= row[0] * reference.k[2] + row[1] * reference.k[3];
    }
    for j in 0..3 {
        a[0][j] = source.k[0] * a[0][j] + source.k[2] * a[2][j];
        a[1][j] = source.k[1] * a[1][j] + source.k[3] * a[2][j];
    }
    a
}

/// Window, depth range, and deterministic search settings.
pub struct PatchMatchParams {
    /// Window offsets start at -radius and end at or before radius (default 4).
    pub radius: usize,
    /// Offset stride in both axes (default 2, giving 25 samples at radius 4).
    pub step: usize,
    /// Spatial bilateral standard deviation in pixels (default 4.0).
    pub sigma_spatial: f64,
    /// Reference intensity bilateral standard deviation (default 0.2).
    pub sigma_color: f64,
    /// Minimum depth in metres (default 0.1); sampling is uniform in inverse depth.
    pub depth_min: f64,
    /// Maximum depth in metres (default 10.0).
    pub depth_max: f64,
    /// Mean of the best min(top_k, valid) source costs (default 3).
    pub top_k: usize,
    /// Iterations, each a red and a black half-sweep with refinement (default 6).
    pub iterations: usize,
    /// Initialisation and refinement counter-based seed (default 1).
    /// Each draw is SplitMix64 of
    /// `z = seed ^ (iteration << 48) ^ (slot << 40) ^ pixel`, where
    /// `pixel = y * width + x < 2^40`, `iteration < 2^16`, and `slot < 2^8`.
    /// With wrapping u64 arithmetic:
    /// `z += 0x9e3779b97f4a7c15; z = (z ^ (z >> 30)) * 0xbf58476d1ce4e5b9;`
    /// `z = (z ^ (z >> 27)) * 0x94d049bb133111eb; z ^= z >> 31;`
    /// then `u = 2 * (z >> 11) / 2^53 - 1` in [-1, 1).
    /// Initialisation uses iteration 0xffff and slots 0 (depth), 1 and 2 (normal).
    /// Refinement uses the iteration number for both colour passes and slots
    /// 0 (inverse depth), 1..=3 (normal perturbation), 4 (fresh depth), and
    /// 5 and 6 (fresh normal).
    pub seed: u64,
}

impl Default for PatchMatchParams {
    fn default() -> Self {
        Self { radius: 4, step: 2, sigma_spatial: 4.0, sigma_color: 0.2,
            depth_min: 0.1, depth_max: 10.0, top_k: 3, iterations: 6, seed: 1 }
    }
}

/// One minus weighted NCC. Bilateral weights are
/// exp(-d^2/(2 sigma_spatial^2) - (I(q)-I(p))^2/(2 sigma_color^2)), using
/// reference intensities and pixel distances. Means, variances and covariance
/// are divided by the weight sum. Returns 2.0 for any out-of-image sample,
/// nonpositive homogeneous w, or weighted variance below 1e-6; otherwise [0, 2].
/// Nonfinite mappings and invalid window parameters also return 2.0.
pub fn ncc_cost(
    reference: &GrayView, source: &GrayView, h: &[[f64; 3]; 3],
    x: usize, y: usize, params: &PatchMatchParams,
) -> f64 {
    if params.step == 0 || !params.sigma_spatial.is_finite() || params.sigma_spatial <= 0.0
        || !params.sigma_color.is_finite() || params.sigma_color <= 0.0
    {
        return 2.0;
    }
    let Some(span) = params.radius.checked_mul(2) else { return 2.0; };
    let Some(centre) = reference.sample(x as f64, y as f64) else { return 2.0; };
    let mut weight_sum = 0.0;
    let mut sum_r = 0.0;
    let mut sum_s = 0.0;
    let mut sum_rr = 0.0;
    let mut sum_ss = 0.0;
    let mut sum_rs = 0.0;
    for oy in (0..=span).step_by(params.step) {
        for ox in (0..=span).step_by(params.step) {
            let dx = ox as f64 - params.radius as f64;
            let dy = oy as f64 - params.radius as f64;
            let qx = x as f64 + dx;
            let qy = y as f64 + dy;
            let Some(r) = reference.sample(qx, qy) else { return 2.0; };
            let w = h[2][0] * qx + h[2][1] * qy + h[2][2];
            if !w.is_finite() || w <= 0.0 { return 2.0; }
            let sx = (h[0][0] * qx + h[0][1] * qy + h[0][2]) / w;
            let sy = (h[1][0] * qx + h[1][1] * qy + h[1][2]) / w;
            let Some(s) = source.sample(sx, sy) else { return 2.0; };
            let r = f64::from(r);
            let s = f64::from(s);
            let dc = r - f64::from(centre);
            let weight = (-0.5 * ((dx / params.sigma_spatial).powi(2)
                + (dy / params.sigma_spatial).powi(2)
                + (dc / params.sigma_color).powi(2))).exp();
            weight_sum += weight;
            sum_r += weight * r;
            sum_s += weight * s;
            sum_rr += weight * r * r;
            sum_ss += weight * s * s;
            sum_rs += weight * r * s;
        }
    }
    let mean_r = sum_r / weight_sum;
    let mean_s = sum_s / weight_sum;
    let variance_r = sum_rr / weight_sum - mean_r * mean_r;
    let variance_s = sum_ss / weight_sum - mean_s * mean_s;
    if !variance_r.is_finite() || !variance_s.is_finite() || variance_r < 1e-6 || variance_s < 1e-6 {
        return 2.0;
    }
    let covariance = sum_rs / weight_sum - mean_r * mean_s;
    (1.0 - covariance / (variance_r * variance_s).sqrt()).clamp(0.0, 2.0)
}

/// Mean of the best min(top_k, valid) source NCC costs below 2.0, with a
/// separate plane homography for each source; 2.0 if none is valid.
/// Sources index `views` and must not include the reference. Invalid indices,
/// a nonpositive depth, or a normal that does not face the camera give 2.0.
pub fn view_cost(
    views: &[GrayView], reference: usize, sources: &[usize], x: usize, y: usize,
    depth: f64, normal: [f64; 3], params: &PatchMatchParams,
) -> f64 {
    let Some(r) = views.get(reference) else { return 2.0; };
    if !depth.is_finite() || depth <= 0.0 || normal.iter().any(|v| !v.is_finite())
        || dot(normal, ray(r, x, y)) >= 0.0 || params.top_k == 0
    { return 2.0; }
    let mut costs = Vec::with_capacity(sources.len());
    for &index in sources {
        let Some(source) = views.get(index) else { return 2.0; };
        if index == reference { return 2.0; }
        let h = plane_homography(r, source, x, y, depth, normal);
        let cost = ncc_cost(r, source, &h, x, y, params);
        if cost < 2.0 { costs.push(cost); }
    }
    costs.sort_by(f64::total_cmp);
    let count = params.top_k.min(costs.len());
    if count == 0 { 2.0 } else { costs[..count].iter().sum::<f64>() / count as f64 }
}

/// Per-pixel depth, normal and cost maps in reference-image row-major order.
pub struct DepthNormalMap {
    /// Number of pixel columns.
    pub width: usize,
    /// Number of pixel rows.
    pub height: usize,
    /// Camera-z depths in metres.
    pub depth: Vec<f64>,
    /// Unit, camera-facing normals in the reference camera frame.
    pub normal: Vec<[f64; 3]>,
    /// Mean best-source NCC costs in [0, 2]; borders retain 2.0.
    pub cost: Vec<f64>,
}

const INIT_ITERATION: u64 = 0xffff;

fn draw(seed: u64, iteration: u64, pixel: u64, slot: u64) -> f64 {
    let mut z = seed ^ (iteration << 48) ^ (slot << 40) ^ pixel;
    z = z.wrapping_add(0x9e3779b97f4a7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
    z ^= z >> 31;
    2.0 * (z >> 11) as f64 / 9007199254740992.0 - 1.0
}

fn random_depth(params: &PatchMatchParams, u: f64) -> f64 {
    let low = 1.0 / params.depth_max;
    1.0 / (low + (1.0 / params.depth_min - low) * (u + 1.0) * 0.5)
}

fn random_normal(direction: [f64; 3], u1: f64, u2: f64) -> [f64; 3] {
    let axis = normalise(direction).map(|v| -v);
    // The ray has nonzero z, so this tangent never degenerates.
    let tangent = normalise([axis[2], 0.0, -axis[0]]);
    let bitangent = [axis[1] * tangent[2] - axis[2] * tangent[1],
        axis[2] * tangent[0] - axis[0] * tangent[2],
        axis[0] * tangent[1] - axis[1] * tangent[0]];
    let cosine = (1.0 - u1) * 0.5;
    let azimuth = std::f64::consts::PI * (u2 + 1.0);
    let sine = (1.0 - cosine * cosine).sqrt();
    normalise(std::array::from_fn(|i| axis[i] * cosine
        + sine * (tangent[i] * azimuth.cos() + bitangent[i] * azimuth.sin())))
}

fn update_region(view: &GrayView, radius: usize, x: usize, y: usize) -> bool {
    x >= radius && y >= radius && x < view.width.saturating_sub(radius)
        && y < view.height.saturating_sub(radius)
}

// All neighbours have the opposite colour; only this pixel is written.
fn update_pixel(
    views: &[GrayView], reference: usize, sources: &[usize], map: &mut DepthNormalMap,
    params: &PatchMatchParams, iteration: u64, scale: f64, x: usize, y: usize,
) {
    let r = &views[reference];
    let i = y * r.width + x;
    let u = |slot| draw(params.seed, iteration, i as u64, slot);
    let mut d = map.depth[i];
    let mut n = map.normal[i];
    let mut cost = map.cost[i];
    for (dx, dy) in [(0, -1), (0, 1), (-1, 0), (1, 0),
        (0, -5), (0, 5), (-5, 0), (5, 0)] {
        let (Some(nx), Some(ny)) = (x.checked_add_signed(dx), y.checked_add_signed(dy))
            else { continue; };
        if !update_region(r, params.radius, nx, ny) { continue; }
        let j = ny * r.width + nx;
        let candidate = view_cost(views, reference, sources, x, y, map.depth[j], map.normal[j], params);
        if candidate < cost { d = map.depth[j]; n = map.normal[j]; cost = candidate; }
    }
    let inverse = (1.0 / d + scale * 0.25
        * (1.0 / params.depth_min - 1.0 / params.depth_max) * u(0))
        .clamp(1.0 / params.depth_max, 1.0 / params.depth_min);
    let dp = 1.0 / inverse;
    let perturbed = std::array::from_fn(|j| n[j] + scale * 0.5 * u(j as u64 + 1));
    let np = if dot(perturbed, perturbed) > 0.0 {
        facing(normalise(perturbed), ray(r, x, y))
    } else { n };
    let dr = random_depth(params, u(4));
    let nr = random_normal(ray(r, x, y), u(5), u(6));
    map.depth[i] = d;
    map.normal[i] = n;
    for (td, tn) in [(dp, n), (d, np), (dp, np), (dr, n), (d, nr)] {
        let candidate = view_cost(views, reference, sources, x, y, td, tn, params);
        if candidate < cost { map.depth[i] = td; map.normal[i] = tn; cost = candidate; }
    }
    map.cost[i] = cost;
}

pub(crate) fn check_arguments(
    views: &[GrayView], reference: usize, sources: &[usize], init: Option<&DepthNormalMap>,
    params: &PatchMatchParams,
) -> Result<(), String> {
    let r = views.get(reference).ok_or("reference index out of range")?;
    r.check()?;
    if sources.is_empty() { return Err("sources must not be empty".into()); }
    for &index in sources {
        if index == reference { return Err("sources must not include the reference".into()); }
        views.get(index).ok_or("source index out of range")?.check()?;
    }
    if params.step == 0 || params.radius == 0 || params.radius.checked_mul(2).is_none()
        || params.top_k == 0 || !params.depth_min.is_finite() || params.depth_min <= 0.0
        || !params.depth_max.is_finite() || params.depth_max <= params.depth_min
        || !(1.0 / params.depth_min).is_finite()
        || !params.sigma_spatial.is_finite() || params.sigma_spatial <= 0.0
        || !params.sigma_color.is_finite() || params.sigma_color <= 0.0
    { return Err("inconsistent PatchMatch parameters".into()); }
    let size = r.pixels.len();
    if let Some(initial) = init {
        if initial.width != r.width || initial.height != r.height || initial.depth.len() != size
            || initial.normal.len() != size || initial.cost.len() != size
        { return Err("initial map dimensions or buffer lengths do not match".into()); }
        for i in 0..size {
            let d = initial.depth[i];
            let n = initial.normal[i];
            if !d.is_finite() || d < params.depth_min || d > params.depth_max
                || n.iter().any(|v| !v.is_finite()) || (dot(n, n) - 1.0).abs() > 1e-6
                || dot(n, ray(r, i % r.width, i / r.width)) >= 0.0
            { return Err("initial depths or normals are invalid".into()); }
        }
    }
    Ok(())
}

/// Red-black PatchMatch. Initial depth/normal maps are copied and costs
/// recomputed; otherwise per-pixel draws initialise inverse-depth-uniform
/// depths and normals uniform on the hemisphere around -ray. Border pixels
/// within `radius` of an edge retain their hypotheses and cost 2.0.
///
/// Each iteration visits red then black pixels, in row-major order. At a pixel,
/// the current hypothesis competes with (0,-1), (0,+1), (-1,0), (+1,0),
/// (0,-5), (0,+5), (-5,0), (+5,0) neighbours inside the update region, using
/// their depth/normal unchanged. Ties retain the earlier hypothesis.
/// Refinement draws an inverse-depth perturbation, three normal perturbations,
/// a fresh depth, then two draws for a fresh hemisphere normal. With
/// s = 0.5^iteration, inverse depth changes by s*0.25*(1/depth_min-1/depth_max)*u
/// and is clamped; the normal is normalise(n+s*0.5*(u1,u2,u3)), flipped to face
/// the camera. Trials are (d_p,n), (d,n_p), (d_p,n_p), (d_r,n), (d,n_r), all
/// relative to the propagation winner. A degenerate perturbed normal retains n.
/// Draws depend only on (seed, iteration, pixel, slot), as specified by
/// [`PatchMatchParams::seed`]. Each colour reads only the other colour's
/// hypotheses, so visiting order within a colour pass does not affect the map.
///
/// Returns Err for invalid views or indices, empty sources or reference in
/// sources, zero radius/step/top_k, nonfinite or nonpositive sigmas/depths,
/// depth_max <= depth_min, or an unrepresentable inverse-depth range. Initial
/// maps must match image dimensions and buffer lengths and contain finite,
/// in-range depths and unit camera-facing normals. Initial costs are ignored.
pub fn patchmatch_reference(
    views: &[GrayView], reference: usize, sources: &[usize], init: Option<&DepthNormalMap>,
    params: &PatchMatchParams,
) -> Result<DepthNormalMap, String> {
    check_arguments(views, reference, sources, init, params)?;
    let r = &views[reference];
    let size = r.pixels.len();
    let mut map = DepthNormalMap { width: r.width, height: r.height,
        depth: Vec::with_capacity(size), normal: Vec::with_capacity(size), cost: vec![2.0; size] };
    for y in 0..r.height {
        for x in 0..r.width {
            let i = y * r.width + x;
            let (d, n) = if let Some(initial) = init { (initial.depth[i], initial.normal[i]) }
                else {
                    let u = |slot| draw(params.seed, INIT_ITERATION, i as u64, slot);
                    (random_depth(params, u(0)), random_normal(ray(r, x, y), u(1), u(2)))
                };
            map.depth.push(d);
            map.normal.push(n);
            if update_region(r, params.radius, x, y) {
                map.cost[i] = view_cost(views, reference, sources, x, y, d, n, params);
            }
        }
    }
    let mut scale = 1.0;
    for iteration in 0..params.iterations {
        for colour in 0..2 {
            for y in params.radius..r.height.saturating_sub(params.radius) {
                for x in params.radius..r.width.saturating_sub(params.radius) {
                    if (x % 2 + y % 2) % 2 != colour { continue; }
                    update_pixel(views, reference, sources, &mut map, params, iteration as u64, scale, x, y);
                }
            }
        }
        scale *= 0.5;
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synthetic::textured_plane;

    #[test]
    fn counter_draws_are_bounded_and_distinguish_pixels_and_slots() {
        assert_eq!(draw(1, 0, 0, 0).to_bits(), 0.1331231503445618_f64.to_bits(),
            "fixed draw for parallel implementations");
        for (seed, iteration, pixel, slot) in [
            (1, 0, 0, 0), (0, 1, 42, 3),
            (u64::MAX, INIT_ITERATION, (1_u64 << 40) - 2, 254),
        ] {
            let value = draw(seed, iteration, pixel, slot);
            let next_pixel = draw(seed, iteration, pixel + 1, slot);
            let next_slot = draw(seed, iteration, pixel, slot + 1);
            for u in [value, next_pixel, next_slot] {
                assert!((-1.0..1.0).contains(&u), "draw must be in [-1, 1): {u}");
            }
            assert_ne!(value.to_bits(), next_pixel.to_bits(), "neighbouring pixels");
            assert_ne!(value.to_bits(), next_slot.to_bits(), "neighbouring slots");
        }
    }

    #[test]
    fn patchmatch_is_independent_of_visiting_order_within_each_colour() {
        let params = PatchMatchParams { depth_min: 0.3, depth_max: 1.0, seed: 1, ..Default::default() };
        let (views, _) = textured_plane(96, 120.0, 0.0, &[-0.10, 0.10]);
        let expected = patchmatch_reference(&views, 0, &[1, 2], None, &params).expect("forward passes");
        let init_params = PatchMatchParams { iterations: 0, ..params };
        let mut reversed = patchmatch_reference(&views, 0, &[1, 2], None, &init_params).expect("initialisation");
        let r = &views[0];
        let mut scale = 1.0;
        for iteration in 0..params.iterations {
            for colour in 0..2 {
                for y in (params.radius..r.height - params.radius).rev() {
                    for x in (params.radius..r.width - params.radius).rev() {
                        if (x % 2 + y % 2) % 2 != colour { continue; }
                        update_pixel(&views, 0, &[1, 2], &mut reversed, &params, iteration as u64, scale, x, y);
                    }
                }
            }
            scale *= 0.5;
        }
        for i in 0..expected.depth.len() {
            assert_eq!(expected.depth[i].to_bits(), reversed.depth[i].to_bits(), "depth at pixel {i}");
            assert_eq!(expected.normal[i].map(f64::to_bits), reversed.normal[i].map(f64::to_bits),
                "normal at pixel {i}");
            assert_eq!(expected.cost[i].to_bits(), reversed.cost[i].to_bits(), "cost at pixel {i}");
        }
    }

    #[test]
    fn plane_homography_matches_exact_projection_on_both_planes() {
        for degrees in [0.0_f64, 30.0] {
            let (views, truth) = textured_plane(96, 120.0, degrees.to_radians(), &[-0.10, 0.10]);
            for v in &views { assert!(v.check().is_ok()); }
            for (x, y) in [(24, 31), (48, 48), (70, 65)] {
                let i = y * 96 + x;
                let point = ray(&views[0], x, y).map(|v| v * truth.depth[i]);
                for source in &views[1..] {
                    let source_point: [f64; 3] = std::array::from_fn(|j| (0..3).map(|k|
                        source.cam_to_world[k][j] * (point[k] - source.cam_to_world[k][3])).sum());
                    let exact = [source.k[0] * source_point[0] / source_point[2] + source.k[2],
                        source.k[1] * source_point[1] / source_point[2] + source.k[3]];
                    let h = plane_homography(&views[0], source, x, y, truth.depth[i], truth.normal[i]);
                    let mapped = h.map(|row| row[0] * x as f64 + row[1] * y as f64 + row[2]);
                    for j in 0..2 {
                        assert!((mapped[j] / mapped[2] - exact[j]).abs() < 1e-9,
                            "{degrees} degree homography axis {j}");
                    }
                }
            }
        }
    }

    #[test]
    fn ncc_prefers_true_depth_and_rejects_invalid_windows() {
        let params = PatchMatchParams { depth_min: 0.3, depth_max: 1.0, seed: 1, ..Default::default() };
        for degrees in [0.0_f64, 30.0] {
            let (mut views, truth) = textured_plane(96, 120.0, degrees.to_radians(), &[-0.10, 0.10]);
            let i = 48 * 96 + 48;
            let h = plane_homography(&views[0], &views[1], 48, 48, truth.depth[i], truth.normal[i]);
            let correct = ncc_cost(&views[0], &views[1], &h, 48, 48, &params);
            let wrong_h = plane_homography(&views[0], &views[1], 48, 48, truth.depth[i] * 1.03, truth.normal[i]);
            let wrong = ncc_cost(&views[0], &views[1], &wrong_h, 48, 48, &params);
            eprintln!("plane {degrees}: true NCC {correct:.6}, wrong-depth NCC {wrong:.6}");
            assert!(correct < 0.05, "{degrees} degree true NCC: {correct}");
            assert!(wrong > correct, "{degrees} degree wrong depth must cost more: {wrong} <= {correct}");
            let outside = [[1.0, 0.0, 100.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
            assert_eq!(ncc_cost(&views[0], &views[1], &outside, 48, 48, &params), 2.0);
            let behind = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, -1.0]];
            assert_eq!(ncc_cost(&views[0], &views[1], &behind, 48, 48, &params), 2.0);
            assert_eq!(ncc_cost(&views[0], &views[1], &h, 0, 0, &params), 2.0);
            let original = views[0].pixels.clone();
            views[0].pixels.fill(0.5);
            assert_eq!(ncc_cost(&views[0], &views[1], &h, 48, 48, &params), 2.0);
            views[0].pixels = original;
        }
    }

    #[test]
    fn view_cost_averages_the_best_valid_sources() {
        let params = PatchMatchParams { depth_min: 0.3, depth_max: 1.0, seed: 1, ..Default::default() };
        for degrees in [0.0_f64, 30.0] {
            let (mut views, truth) = textured_plane(96, 120.0, degrees.to_radians(), &[-0.10, 0.10]);
            let i = 48 * 96 + 48;
            let source_costs: Vec<f64> = views[1..].iter().map(|source| {
                let h = plane_homography(&views[0], source, 48, 48, truth.depth[i], truth.normal[i]);
                ncc_cost(&views[0], source, &h, 48, 48, &params)
            }).collect();
            let cost = view_cost(&views, 0, &[1, 2], 48, 48, truth.depth[i], truth.normal[i], &params);
            assert!((cost - source_costs.iter().sum::<f64>() / 2.0).abs() < 1e-12);
            let best_params = PatchMatchParams { top_k: 1, ..params };
            assert_eq!(view_cost(&views, 0, &[1, 2], 48, 48, truth.depth[i], truth.normal[i], &best_params),
                source_costs[0].min(source_costs[1]));
            let original = views[2].pixels.clone();
            views[2].pixels.fill(0.5);
            assert_eq!(view_cost(&views, 0, &[1, 2], 48, 48, truth.depth[i], truth.normal[i], &params), source_costs[0]);
            assert_eq!(view_cost(&views, 0, &[2], 48, 48, truth.depth[i], truth.normal[i], &params), 2.0);
            views[2].pixels = original;

        }
    }

    // Nearest-rank percentiles of the updatable pixels, sorted independently.
    fn print_statistics(degrees: f64, initialisation: &str, result: &DepthNormalMap,
        truth: &DepthNormalMap, reference: &GrayView, params: &PatchMatchParams) {
        let mut depths = Vec::new();
        let mut normals = Vec::new();
        let mut costs = Vec::new();
        for y in params.radius..reference.height - params.radius {
            for x in params.radius..reference.width - params.radius {
                let i = y * reference.width + x;
                depths.push((result.depth[i] / truth.depth[i] - 1.0).abs());
                normals.push(dot(result.normal[i], truth.normal[i]).clamp(-1.0, 1.0).acos().to_degrees());
                costs.push(result.cost[i]);
            }
        }
        depths.sort_by(f64::total_cmp);
        normals.sort_by(f64::total_cmp);
        costs.sort_by(f64::total_cmp);
        let p50 = (depths.len() * 50).div_ceil(100) - 1;
        let p95 = (depths.len() * 95).div_ceil(100) - 1;
        eprintln!("PM {degrees} {initialisation} depth_p50={:.6} depth_p95={:.6} (relative) normal_p50_deg={:.6} normal_p95_deg={:.6} cost_p50={:.6}",
            depths[p50], depths[p95], normals[p50], normals[p95], costs[p50]);
    }

    #[test]
    fn patchmatch_converges_from_random_initialisation_deterministically() {
        let params = PatchMatchParams { depth_min: 0.3, depth_max: 1.0, seed: 1, ..Default::default() };
        let mut coverage = Vec::new();
        for degrees in [0.0_f64, 30.0] {
            let (views, truth) = textured_plane(96, 120.0, degrees.to_radians(), &[-0.10, 0.10]);
            let result = patchmatch_reference(&views, 0, &[1, 2], None, &params).expect("valid random initialisation");
            let refined = patchmatch_reference(&views, 0, &[1, 2], Some(&truth), &params).expect("truth initialisation");
            print_statistics(degrees, "random", &result, &truth, &views[0], &params);
            print_statistics(degrees, "truth", &refined, &truth, &views[0], &params);
            let repeated = patchmatch_reference(&views, 0, &[1, 2], None, &params).expect("repeat");
            assert_eq!(result.depth, repeated.depth, "same-seed depths");
            assert_eq!(result.normal, repeated.normal, "same-seed normals");
            assert_eq!(result.cost, repeated.cost, "same-seed costs");
            let mut objective_good = 0;
            let mut depth_good = 0;
            let mut normal_good = 0;
            let mut count = 0;
            for y in 0..96 {
                for x in 0..96 {
                    let i = y * 96 + x;
                    if !update_region(&views[0], params.radius, x, y) {
                        assert_eq!(result.cost[i], 2.0, "border cost");
                        continue;
                    }
                    count += 1;
                    objective_good += usize::from(result.cost[i] <= refined.cost[i] + 0.02);
                    depth_good += usize::from((result.depth[i] / truth.depth[i] - 1.0).abs() <= 0.01);
                    normal_good += usize::from(dot(result.normal[i], truth.normal[i]) >= 20.0_f64.to_radians().cos());
                    assert!((dot(result.normal[i], result.normal[i]) - 1.0).abs() < 1e-12, "unit normal");
                    assert!(dot(result.normal[i], ray(&views[0], x, y)) < 0.0, "facing normal");
                }
            }
            eprintln!("plane {degrees}: objective {objective_good}/{count}, depth {depth_good}/{count}, normal {normal_good}/{count}");
            coverage.push((degrees, objective_good, depth_good, normal_good, count));
        }
        // Report both scenes before checking acceptance, including on a failure.
        for (degrees, objective_good, depth_good, normal_good, count) in coverage {
            assert!(objective_good * 100 >= count * 95, "{degrees} degree objective minimum: {objective_good}/{count} < 95%");
            assert!(depth_good * 100 >= count * 95, "{degrees} degree depth within 1%: {depth_good}/{count} < 95%");
            assert!(normal_good * 100 >= count * 90, "{degrees} degree normals within 20 degrees: {normal_good}/{count} < 90%");
        }
    }

    #[test]
    fn patchmatch_never_increases_truth_initialisation_cost() {
        let params = PatchMatchParams { depth_min: 0.3, depth_max: 1.0, seed: 1, ..Default::default() };
        for degrees in [0.0_f64, 30.0] {
            let (views, mut truth) = textured_plane(96, 120.0, degrees.to_radians(), &[-0.10, 0.10]);
            for y in params.radius..96 - params.radius {
                for x in params.radius..96 - params.radius {
                    let i = y * 96 + x;
                    truth.cost[i] = view_cost(&views, 0, &[1, 2], x, y, truth.depth[i], truth.normal[i], &params);
                }
            }
            let refined = patchmatch_reference(&views, 0, &[1, 2], Some(&truth), &params).expect("truth initialisation");
            for i in 0..truth.cost.len() {
                assert!(refined.cost[i] <= truth.cost[i], "{degrees} degree truth cost increased at {i}");
            }
        }
    }

    #[test]
    fn patchmatch_rejects_invalid_views_sources_parameters_and_initial_maps() {
        let params = PatchMatchParams { depth_min: 0.3, depth_max: 1.0, seed: 1, ..Default::default() };
        let (mut views, mut truth) = textured_plane(96, 120.0, 0.0, &[-0.10, 0.10]);
        assert!(patchmatch_reference(&views, 0, &[], None, &params).is_err(), "empty sources");
        assert!(patchmatch_reference(&views, 0, &[0], None, &params).is_err(), "reference source");
        assert!(patchmatch_reference(&views, 3, &[1], None, &params).is_err(), "reference index");
        assert!(patchmatch_reference(&views, 0, &[3], None, &params).is_err(), "source index");
        for index in 0..3 {
            let pixel = views[index].pixels.pop().expect("fixture pixel");
            assert!(patchmatch_reference(&views, 0, &[1, 2], None, &params).is_err(), "invalid view {index}");
            views[index].pixels.push(pixel);
        }
        for invalid in [
            PatchMatchParams { step: 0, ..params }, PatchMatchParams { radius: 0, ..params },
            PatchMatchParams { top_k: 0, ..params }, PatchMatchParams { depth_min: 0.0, ..params },
            PatchMatchParams { depth_min: -1.0, ..params }, PatchMatchParams { depth_max: 0.3, ..params },
            PatchMatchParams { depth_max: 0.2, ..params }, PatchMatchParams { depth_min: f64::NAN, ..params },
            PatchMatchParams { depth_max: f64::INFINITY, ..params },
            PatchMatchParams { depth_min: f64::from_bits(1), ..params },
            PatchMatchParams { sigma_spatial: 0.0, ..params }, PatchMatchParams { sigma_color: -1.0, ..params },
            PatchMatchParams { sigma_spatial: f64::NAN, ..params },
            PatchMatchParams { sigma_color: f64::INFINITY, ..params },
            PatchMatchParams { radius: usize::MAX, ..params },
        ] {
            assert!(patchmatch_reference(&views, 0, &[1, 2], None, &invalid).is_err(), "invalid parameters");
        }
        truth.width = 95;
        assert!(patchmatch_reference(&views, 0, &[1], Some(&truth), &params).is_err(), "initial dimensions");
        truth.width = 96;
        truth.cost.pop();
        assert!(patchmatch_reference(&views, 0, &[1], Some(&truth), &params).is_err(), "initial buffer");
        truth.cost.push(2.0);
        truth.depth[0] = f64::NAN;
        assert!(patchmatch_reference(&views, 0, &[1], Some(&truth), &params).is_err(), "initial depth");
        truth.depth[0] = 0.5;
        for normal in [[0.0, 0.0, 1.0], [0.0; 3], [0.0, 0.0, -2.0], [f64::NAN; 3]] {
            truth.normal[0] = normal;
            assert!(patchmatch_reference(&views, 0, &[1], Some(&truth), &params).is_err(), "initial normal");
        }
    }

}
