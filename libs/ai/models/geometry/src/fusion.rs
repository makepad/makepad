//! Deterministic host fusion using f64 multi-view geometric consistency, the fused points' expected errors, and
//! completion of a fused view with a prior depth map where fusion kept nothing, the prior's error measured against the
//! fusion near where it is used; each point is marked with its source and expected error.

use crate::{patchmatch::{view_cost, DepthNormalMap, PatchMatchParams}, views::GrayView};

/// Multi-view geometric consistency settings.
pub struct ConsistencyParams {
    /// Largest round-trip reprojection error in reference pixels (default 1.0).
    pub max_reprojection_px: f64,
    /// Largest relative difference between the round-trip depth and the reference depth (default 0.01).
    pub max_relative_depth: f64,
    /// Hypotheses with a higher photometric cost take no part, in the reference or the other view (default 0.3).
    pub max_cost: f64,
    /// Fewest other views that must agree for a pixel to be kept (default 2).
    pub min_views: usize,
}

impl Default for ConsistencyParams {
    fn default() -> Self {
        Self { max_reprojection_px: 1.0, max_relative_depth: 0.01, max_cost: 0.3, min_views: 2 }
    }
}

/// Per reference pixel, row-major: the fused world point, the number of agreeing other views and the spread of the
/// fused points.
pub struct FusedMap {
    pub width: usize,
    pub height: usize,
    pub points: Vec<Option<[f64; 3]>>,
    /// Counts saturate at 255; fusion and the minimum-view gate use the full count.
    pub agreeing: Vec<u8>,
    /// Where `points` has a point, the root-mean-square distance of the points it is the mean of from it, in world
    /// units; zero elsewhere.
    pub spread: Vec<f64>,
}

fn world_point(view: &GrayView, x: usize, y: usize, depth: f64) -> [f64; 3] {
    let p = [(x as f64 - view.k[2]) / view.k[0] * depth,
        (y as f64 - view.k[3]) / view.k[1] * depth, depth];
    let m = &view.cam_to_world;
    std::array::from_fn(|i| m[i][0] * p[0] + m[i][1] * p[1] + m[i][2] * p[2] + m[i][3])
}

/// Project a world point to pixel x, pixel y, and camera z.
fn project(view: &GrayView, point: [f64; 3]) -> Option<[f64; 3]> {
    let m = &view.cam_to_world;
    let delta = [point[0] - m[0][3], point[1] - m[1][3], point[2] - m[2][3]];
    let p: [f64; 3] = std::array::from_fn(|i| {
        m[0][i] * delta[0] + m[1][i] * delta[1] + m[2][i] * delta[2]
    });
    if p[2] <= 0.0 { return None; }
    let result = [view.k[0] * (p[0] / p[2]) + view.k[2],
        view.k[1] * (p[1] / p[2]) + view.k[3], p[2]];
    result.iter().all(|v| v.is_finite()).then_some(result)
}

fn valid_sample(map: &DepthNormalMap, index: usize, max_cost: f64) -> bool {
    map.depth[index].is_finite() && map.depth[index] > 0.0
        && map.cost[index].is_finite() && map.cost[index] <= max_cost
}

/// Fuse camera-z depth maps using round-trip geometric consistency, entirely on
/// the host in f64. `maps[i]` belongs to `views[i]`; normals are not used.
/// Pixel centres are integers, as in [`GrayView`].
///
/// For each reference pixel p=(x,y) with positive finite depth d and finite cost
/// <= `max_cost`, lift X = cam_to_world_r (d K_r^-1 [x,y,1]). Project X into
/// each other camera, requiring positive camera z, and round to the nearest
/// integer pixel q (halfway values round away from zero). Skip out-of-image q,
/// nonpositive or nonfinite depths, nonfinite costs, and costs > `max_cost`.
/// Lift Y = cam_to_world_j (d_j K_j^-1 [q,1]) and project it back into the
/// reference. Y agrees when its reference camera z is positive, its Euclidean
/// reprojection distance from p is <= `max_reprojection_px`, and
/// |z_r(Y)-d|/d <= `max_relative_depth`. Nonfinite geometry is skipped.
///
/// The fused world point is the mean of X and every agreeing Y, its spread their root-mean-square distance from
/// the mean. A pixel has
/// `Some(point)` only when at least `min_views` other views agree; its agreement
/// count is recorded even if it is not kept. Invalid reference hypotheses have
/// no point and zero agreements. Repeated indices in `others` count only once.
/// Counts exposed as u8 saturate at 255, without limiting fusion or the gate.
/// Empty `others` and `min_views == 0` are valid (the latter can keep X alone).
///
/// Returns Err for out-of-range indices, the reference in `others`, unequal
/// map/view counts, map dimensions or depth/normal/cost buffer lengths that do
/// not match their view, or nonfinite/negative floating-point parameters.
/// Every view, including unused ones, must pass [`GrayView::check`].
pub fn fuse_view(
    views: &[GrayView], maps: &[DepthNormalMap], reference: usize, others: &[usize],
    params: &ConsistencyParams,
) -> Result<FusedMap, String> {
    if maps.len() != views.len() { return Err("map and view counts do not match".into()); }
    let r = views.get(reference).ok_or("reference index out of range")?;
    let mut seen = vec![false; views.len()];
    let mut sources = Vec::new();
    for &index in others {
        if index >= views.len() { return Err("other view index out of range".into()); }
        if index == reference { return Err("reference cannot be an other view".into()); }
        if !seen[index] {
            seen[index] = true;
            sources.push(index);
        }
    }
    if [params.max_reprojection_px, params.max_relative_depth, params.max_cost]
        .iter().any(|v| !v.is_finite() || *v < 0.0)
    { return Err("consistency thresholds must be finite and nonnegative".into()); }
    for (view, map) in views.iter().zip(maps) {
        view.check()?;
        let size = view.pixels.len();
        if map.width != view.width || map.height != view.height || map.depth.len() != size
            || map.normal.len() != size || map.cost.len() != size
        { return Err("map dimensions or buffer lengths do not match their view".into()); }
    }
    let mut fused = FusedMap { width: r.width, height: r.height,
        points: vec![None; r.pixels.len()], agreeing: vec![0; r.pixels.len()], spread: vec![0.0; r.pixels.len()] };
    let reference_map = &maps[reference];
    for y in 0..r.height {
        for x in 0..r.width {
            let i = y * r.width + x;
            if !valid_sample(reference_map, i, params.max_cost) { continue; }
            let depth = reference_map.depth[i];
            let point = world_point(r, x, y, depth);
            if point.iter().any(|v| !v.is_finite()) { continue; }
            let mut mean = point;
            // The sum of squared distances from the mean, updated with it (Welford).
            let mut squares = 0.0;
            let mut count = 0;
            for &index in &sources {
                let view = &views[index];
                let Some(projected) = project(view, point) else { continue; };
                let u = projected[0].round();
                let v = projected[1].round();
                if u < 0.0 || v < 0.0 || u >= view.width as f64 || v >= view.height as f64 {
                    continue;
                }
                let (qx, qy) = (u as usize, v as usize);
                let q = qy * view.width + qx;
                let map = &maps[index];
                if !valid_sample(map, q, params.max_cost) { continue; }
                let other = world_point(view, qx, qy, map.depth[q]);
                let Some(back) = project(r, other) else { continue; };
                if (back[0] - x as f64).hypot(back[1] - y as f64) > params.max_reprojection_px
                    || (back[2] - depth).abs() / depth > params.max_relative_depth
                { continue; }
                count += 1;
                // An online mean avoids overflowing a sum of otherwise finite points.
                let weight = 1.0 / (count as f64 + 1.0);
                let before = mean;
                for axis in 0..3 { mean[axis] = mean[axis] * (1.0 - weight) + other[axis] * weight; }
                squares += (0..3).map(|axis| (other[axis] - before[axis]) * (other[axis] - mean[axis])).sum::<f64>();
            }
            fused.agreeing[i] = count.min(u8::MAX as usize) as u8;
            if count >= params.min_views {
                fused.points[i] = Some(mean);
                fused.spread[i] = (squares.max(0.0) / (count as f64 + 1.0)).sqrt();
            }
        }
    }
    Ok(fused)
}

/// Settings of [`fused_errors`].
pub struct FusedErrorParams {
    /// The cost rise a depth change must cause to be told apart (default 0.0025): a pixel's width is the depth change
    /// that raises its plane's cost by this much, by the parabola through the costs at its depth and `width_step` either
    /// side.
    pub width_rise: f64,
    /// Relative depth step of that parabola (default 0.002).
    pub width_step: f64,
    /// A window whose texture is less isotropic than this is an edge (default 0.1): isotropy is the smaller over the
    /// larger eigenvalue of the structure tensor of its gradients.
    pub edge_isotropy: f64,
    /// An edge fixes the depth along itself but not the plane's tilt across it, so the centre's depth is extrapolated
    /// from where the window's gradient energy lies: it is taken to be off by this times that lever on the surface
    /// (default 2.0).
    pub edge_slope: f64,
}

impl Default for FusedErrorParams {
    fn default() -> Self {
        Self { width_rise: 0.0025, width_step: 0.002, edge_isotropy: 0.1, edge_slope: 2.0 }
    }
}

/// The isotropy of a window's texture (the smaller over the larger eigenvalue of its gradients' structure tensor, 0
/// without gradients) and its lever: the distance in pixels from the centre to the gradient-energy-weighted mean offset
/// (the radius without gradients). Offsets run from -radius to radius by `step`; samples beyond the image take the
/// nearest pixel; gradients are central differences.
fn window_texture(view: &GrayView, x: usize, y: usize, radius: usize, step: usize) -> (f64, f64) {
    let (w, h) = (view.width as isize, view.height as isize);
    let pixel = |qx: isize, qy: isize| f64::from(view.pixels[(qy.clamp(0, h - 1) * w + qx.clamp(0, w - 1)) as usize]);
    let r = radius as isize;
    let (mut xx, mut xy, mut yy, mut mx, mut my) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for oy in (-r..=r).step_by(step) {
        for ox in (-r..=r).step_by(step) {
            let (qx, qy) = (x as isize + ox, y as isize + oy);
            let gx = 0.5 * (pixel(qx + 1, qy) - pixel(qx - 1, qy));
            let gy = 0.5 * (pixel(qx, qy + 1) - pixel(qx, qy - 1));
            xx += gx * gx;
            xy += gx * gy;
            yy += gy * gy;
            mx += (gx * gx + gy * gy) * ox as f64;
            my += (gx * gx + gy * gy) * oy as f64;
        }
    }
    let (trace, det) = (xx + yy, xx * yy - xy * xy);
    if trace <= 0.0 {
        return (0.0, radius as f64);
    }
    let root = (0.25 * trace * trace - det).max(0.0).sqrt();
    ((0.5 * trace - root) / (0.5 * trace + root), (mx / trace).hypot(my / trace))
}

/// The expected error in world units of every point of `fused`, a fusion of `map` (the reference view's PatchMatch
/// map): the largest of
/// - its spread;
/// - its width: the depth change, with the map's normal held, that raises the PatchMatch cost over `sources` with
///   `window` (as [`view_cost`]) by `width_rise`, from the parabola through the costs at the map's depth `d` and
///   `d (1 +- width_step)`; left out where any of those costs is 2 or the parabola does not curve up;
/// - on an edge window (isotropy below `edge_isotropy`, see [`FusedErrorParams`]), `edge_slope` times the window's
///   lever times `d` over the mean focal length.
///
/// Zero where `fused` has no point. Returns Err for a reference or source index out of range, the reference among the
/// sources, a view failing [`GrayView::check`], a map or fused map whose size does not match the reference view, a
/// zero window step, or parameters that are negative or not finite (`width_step` must be positive).
pub fn fused_errors(
    views: &[GrayView], reference: usize, sources: &[usize], map: &DepthNormalMap, fused: &FusedMap,
    window: &PatchMatchParams, params: &FusedErrorParams,
) -> Result<Vec<f64>, String> {
    let view = views.get(reference).ok_or("reference index out of range")?;
    if sources.iter().any(|&s| s >= views.len() || s == reference) {
        return Err("sources must index other views".into());
    }
    for view in views {
        view.check()?;
    }
    let size = view.pixels.len();
    if map.width != view.width || map.height != view.height || map.depth.len() != size || map.normal.len() != size
        || fused.width != view.width || fused.height != view.height || fused.points.len() != size || fused.spread.len() != size
    {
        return Err("map or fused map size does not match the reference view".into());
    }
    if window.step == 0 || [params.width_rise, params.edge_isotropy, params.edge_slope].iter().any(|v| !(v.is_finite() && *v >= 0.0))
        || !(params.width_step.is_finite() && params.width_step > 0.0)
    {
        return Err("a zero window step, or error parameters negative or not finite".into());
    }
    let focal = 0.5 * (view.k[0] + view.k[1]);
    Ok((0..size)
        .map(|i| {
            if fused.points[i].is_none() {
                return 0.0;
            }
            let (x, y, depth, normal) = (i % view.width, i / view.width, map.depth[i], map.normal[i]);
            let cost = |d: f64| view_cost(views, reference, sources, x, y, d, normal, window);
            let (c0, up, down) = (cost(depth), cost(depth * (1.0 + params.width_step)), cost(depth * (1.0 - params.width_step)));
            let curvature = up + down - 2.0 * c0;
            let width = if c0 < 2.0 && up < 2.0 && down < 2.0 && curvature > 0.0 {
                params.width_step * depth * (2.0 * params.width_rise / curvature).sqrt()
            } else {
                0.0
            };
            let (isotropy, lever) = window_texture(view, x, y, window.radius, window.step);
            let edge = if isotropy < params.edge_isotropy { params.edge_slope * lever * depth / focal } else { 0.0 };
            fused.spread[i].max(width).max(edge)
        })
        .collect())
}

/// Settings of [`complete_view`].
pub struct CompletionParams {
    /// A prior depth is usable only where each of its eight neighbours' is usable too and within this relative
    /// step of it (default 0.05): across a depth edge a learned depth blends the two sides.
    pub max_relative_step: f64,
    /// Side in pixels of the square tiles the prior's error is measured over (default 16).
    pub tile: usize,
    /// Fewest overlap pixels a tile's neighbourhood needs to measure the prior's error there (default 30).
    pub min_overlap: usize,
    /// Neighbourhoods reach 1, 2, 4, ... tiles from the tile, up to this many (default 4).
    pub max_reach: usize,
    /// Quantile of a neighbourhood's overlap relative differences taken as the prior's relative error there (default
    /// 0.9): the differences include the fused points' own errors.
    pub error_quantile: f64,
    /// Quantile of the whole view's overlap relative differences that fused points are checked against (default 0.95).
    pub check_quantile: f64,
}

impl Default for CompletionParams {
    fn default() -> Self {
        Self { max_relative_step: 0.05, tile: 16, min_overlap: 30, max_reach: 4, error_quantile: 0.9, check_quantile: 0.95 }
    }
}

/// Where a point of a [`CompletedMap`] comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointSource {
    /// Multi-view fusion.
    Fused,
    /// The prior depth, where fusion kept nothing.
    Prior,
}

/// A world point of a [`CompletedMap`], its source and its expected error in world units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CompletedPoint {
    pub world: [f64; 3],
    pub source: PointSource,
    pub expected_error: f64,
}

/// Per reference pixel, row-major: the fused point, or else the prior's.
pub struct CompletedMap {
    pub width: usize,
    pub height: usize,
    pub points: Vec<Option<CompletedPoint>>,
    /// Pixels with both a fused point and a usable prior depth.
    pub overlap: usize,
    /// Pixels with a usable prior depth and no fused point that get no point: no fused support near them.
    pub unsupported: usize,
}

/// The nearest-rank `q` quantile of `values`, which must not be empty.
fn quantile(mut values: Vec<f64>, q: f64) -> f64 {
    values.sort_by(f64::total_cmp);
    values[((q * values.len() as f64).ceil() as usize).clamp(1, values.len()) - 1]
}

/// Completes a fused view of `view` with `prior`, a camera-z depth per pixel of that view (for instance a learned
/// model's), where `fused_error` is the fused points' expected error (as [`fused_errors`]). A prior depth is usable when
/// it and its eight neighbours' (those inside the image) are finite and positive and the neighbours' are within
/// `max_relative_step` of it.
///
/// The prior's error is measured against the fusion near where it is used. The overlap is the pixels with both a fused
/// point and a usable prior depth; each has the distance between the two points over the prior point's distance from
/// the camera. For a pixel in a `tile`-pixel tile, the prior's relative error is the `error_quantile` of the overlap's
/// in the tiles within 1, 2, 4, ... tiles of it (to `max_reach`), the first with at least `min_overlap`; with none it
/// is unmeasured. A prior point's expected error is that times its distance from the camera.
///
/// Every pixel with a fused point keeps it with its expected error, except where it fails the check against the
/// prior: when the two points are further apart than both expected errors together, one of them is wrong, and the
/// fused point's expected error becomes their distance plus the prior's. The check takes the prior's relative error
/// as the `check_quantile` of the whole view's overlap (with at least `min_overlap`; else there is no check): near
/// fused points that are wrong together, as on a highlight, the neighbourhood's would grow with them and pass them.
/// Every other pixel with a usable prior depth gets the prior's point where its error is measured, and no point where
/// it is not (`unsupported`).
///
/// Returns Err for a view failing [`GrayView::check`], a fused map, fused errors or prior whose size does not match the
/// view, a fused error that is negative or not finite where there is a point, a zero `tile` or `min_overlap`, an
/// `error_quantile` or `check_quantile` outside (0, 1], or a `max_relative_step` that is not finite and nonnegative.
pub fn complete_view(view: &GrayView, fused: &FusedMap, fused_error: &[f64], prior: &[f64], params: &CompletionParams) -> Result<CompletedMap, String> {
    view.check()?;
    let size = view.pixels.len();
    if fused.width != view.width || fused.height != view.height || fused.points.len() != size || fused_error.len() != size
        || prior.len() != size
    {
        return Err("fused map, fused errors or prior size does not match the view".into());
    }
    if fused.points.iter().zip(fused_error).any(|(p, e)| p.is_some() && !(e.is_finite() && *e >= 0.0)) {
        return Err("fused errors must be finite and nonnegative".into());
    }
    let in_unit = |q: f64| q > 0.0 && q <= 1.0;
    if params.tile == 0 || params.min_overlap == 0 || !in_unit(params.error_quantile) || !in_unit(params.check_quantile)
        || !(params.max_relative_step.is_finite() && params.max_relative_step >= 0.0)
    {
        return Err("tile and min_overlap must be positive, the quantiles in (0, 1] and max_relative_step finite and nonnegative".into());
    }
    let (width, height) = (view.width, view.height);
    let positive = |d: f64| d.is_finite() && d > 0.0;
    let usable: Vec<bool> = (0..size)
        .map(|i| {
            let (x, y, d) = (i % width, i / width, prior[i]);
            positive(d)
                && (y.saturating_sub(1)..(y + 2).min(height)).all(|ny| {
                    (x.saturating_sub(1)..(x + 2).min(width)).all(|nx| {
                        let n = prior[ny * width + nx];
                        positive(n) && (n - d).abs() <= params.max_relative_step * d
                    })
                })
        })
        .collect();
    let centre = [view.cam_to_world[0][3], view.cam_to_world[1][3], view.cam_to_world[2][3]];
    let distance = |a: [f64; 3], b: [f64; 3]| (0..3).map(|axis| (a[axis] - b[axis]).powi(2)).sum::<f64>().sqrt();
    let prior_point = |i: usize| world_point(view, i % width, i / width, prior[i]);
    let (columns, rows) = (width.div_ceil(params.tile), height.div_ceil(params.tile));
    let tile_of = |i: usize| (i / width / params.tile, i % width / params.tile);
    let mut tiles = vec![Vec::new(); columns * rows];
    let mut overlap = 0;
    for i in 0..size {
        if let (Some(f), true) = (fused.points[i], usable[i]) {
            let q = prior_point(i);
            let (row, column) = tile_of(i);
            tiles[row * columns + column].push(distance(f, q) / distance(q, centre));
            overlap += 1;
        }
    }
    let relative: Vec<Option<f64>> = (0..rows * columns)
        .map(|t| {
            let (row, column) = (t / columns, t % columns);
            let mut reach = 1;
            while reach <= params.max_reach {
                let near: Vec<f64> = (row.saturating_sub(reach)..(row + reach + 1).min(rows))
                    .flat_map(|r| (column.saturating_sub(reach)..(column + reach + 1).min(columns)).map(move |c| r * columns + c))
                    .flat_map(|n| tiles[n].iter().copied())
                    .collect();
                if near.len() >= params.min_overlap {
                    return Some(quantile(near, params.error_quantile));
                }
                reach *= 2;
            }
            None
        })
        .collect();
    let check = (overlap >= params.min_overlap).then(|| quantile(tiles.concat(), params.check_quantile));
    let mut unsupported = 0;
    let points = (0..size)
        .map(|i| {
            let (row, column) = tile_of(i);
            let measured = relative[row * columns + column].filter(|_| usable[i]);
            match (fused.points[i], measured) {
                (Some(world), _) => {
                    let mut expected_error = fused_error[i];
                    if let (Some(relative), true) = (check, usable[i]) {
                        let q = prior_point(i);
                        let (apart, prior_error) = (distance(world, q), relative * distance(q, centre));
                        if apart > expected_error + prior_error {
                            expected_error = apart + prior_error;
                        }
                    }
                    Some(CompletedPoint { world, source: PointSource::Fused, expected_error })
                }
                (None, Some(relative)) => {
                    let world = prior_point(i);
                    world.iter().all(|v| v.is_finite()).then(|| CompletedPoint {
                        world,
                        source: PointSource::Prior,
                        expected_error: relative * distance(world, centre),
                    })
                }
                (None, None) => {
                    unsupported += usize::from(usable[i]);
                    None
                }
            }
        })
        .collect();
    Ok(CompletedMap { width, height, points, overlap, unsupported })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synthetic::{plane_depth, textured_plane};

    fn scene(angle: f64) -> (Vec<GrayView>, Vec<DepthNormalMap>) {
        let (views, _) = textured_plane(96, 120.0, angle, &[-0.10, 0.10, 0.05]);
        let maps = views.iter().map(|view| DepthNormalMap {
            width: view.width, height: view.height, depth: plane_depth(view, angle),
            normal: vec![[0.0, 0.0, -1.0]; view.pixels.len()], cost: vec![0.0; view.pixels.len()],
        }).collect();
        (views, maps)
    }

    fn camera_point(view: &GrayView, world: [f64; 3]) -> [f64; 3] {
        let mut camera = [0.0; 3];
        for (axis, component) in camera.iter_mut().enumerate() {
            for (row, coordinate) in world.iter().enumerate() {
                *component += view.cam_to_world[row][axis] * (coordinate - view.cam_to_world[row][3]);
            }
        }
        camera
    }

    // Independently compute visibility from the world point, including rounding
    // before the image bounds check rather than clipping the subpixel position.
    fn landing(view: &GrayView, world: [f64; 3]) -> Option<(usize, usize)> {
        let camera = camera_point(view, world);
        let x = (view.k[0] * (camera[0] / camera[2]) + view.k[2]).round();
        let y = (view.k[1] * (camera[1] / camera[2]) + view.k[3]).round();
        (camera[2] > 0.0 && x >= 0.0 && y >= 0.0 && x < view.width as f64 && y < view.height as f64)
            .then_some((x as usize, y as usize))
    }

    fn in_block(x: usize, y: usize) -> bool { (32..64).contains(&x) && (32..64).contains(&y) }

    #[test]
    fn truth_fusion_keeps_visible_points_on_the_plane() {
        for degrees in [0.0_f64, 30.0] {
            let angle = degrees.to_radians();
            let (views, maps) = scene(angle);
            // Exercise rotated, translated references as well as the origin camera.
            for reference in 0..views.len() {
                let others: Vec<_> = (0..views.len()).filter(|&j| j != reference).collect();
                let params = ConsistencyParams::default();
                let fused = fuse_view(&views, &maps, reference, &others, &params).unwrap();
                assert_eq!((fused.width, fused.height), (96, 96), "output dimensions");
                let mut kept = 0;
                for i in 0..maps[reference].depth.len() {
                    let world = world_point(&views[reference], i % 96, i / 96, maps[reference].depth[i]);
                    let mut visible = 0;
                    let mut count = 0;
                    let mut sum = world;
                    let mut members = vec![world];
                    for &j in &others {
                        let Some((x, y)) = landing(&views[j], world) else { continue; };
                        visible += 1;
                        let other = world_point(&views[j], x, y, maps[j].depth[y * 96 + x]);
                        let back = camera_point(&views[reference], other);
                        let k = views[reference].k;
                        let dx = k[0] * (back[0] / back[2]) + k[2] - (i % 96) as f64;
                        let dy = k[1] * (back[1] / back[2]) + k[3] - (i / 96) as f64;
                        let relative_depth = (back[2] / maps[reference].depth[i] - 1.0).abs();
                        if dx.hypot(dy) <= params.max_reprojection_px && relative_depth <= params.max_relative_depth {
                            count += 1;
                            for axis in 0..3 { sum[axis] += other[axis]; }
                            members.push(other);
                        }
                    }
                    // In the requested centre-reference scenes every visible
                    // truth sample agrees. Across the two outer cameras, nearest
                    // pixel rounding can exceed the one-pixel round-trip gate.
                    if reference == 0 { assert_eq!(count, visible, "centre-reference truth visibility at {i}"); }
                    assert_eq!(fused.agreeing[i] as usize, count, "{degrees} degree view {reference} pixel {i}: agreements");
                    assert_eq!(fused.points[i].is_some(), count >= 2, "consistency gate at {i}");
                    if let Some(p) = fused.points[i] {
                        kept += 1;
                        let distance = angle.sin() * p[0] + angle.cos() * (p[2] - 0.5);
                        assert!(distance.abs() <= 1e-9, "fused point off plane at {i}: {distance}");
                        for axis in 0..3 {
                            assert!((p[axis] - sum[axis] / (count + 1) as f64).abs() <= 1e-12,
                                "{degrees} degree view {reference} mean at {i}, axis {axis}: {} vs {}",
                                p[axis], sum[axis] / (count + 1) as f64);
                        }
                        let squares: f64 = members.iter().map(|m| (0..3).map(|axis| (m[axis] - p[axis]).powi(2)).sum::<f64>()).sum();
                        let spread = (squares / members.len() as f64).sqrt();
                        assert!((fused.spread[i] - spread).abs() <= 1e-12, "spread at {i}: {} vs {spread}", fused.spread[i]);
                    } else {
                        assert_eq!(fused.spread[i], 0.0, "no point, no spread at {i}");
                    }
                }
                assert!(kept > 0, "plane must have fused points");
            }
        }
    }

    #[test]
    fn reference_depth_errors_and_costs_are_rejected() {
        for degrees in [0.0_f64, 30.0] {
            let (views, mut maps) = scene(degrees.to_radians());
            let params = ConsistencyParams { min_views: 1, ..Default::default() };
            for i in 0..96 * 96 {
                if in_block(i % 96, i / 96) { maps[0].depth[i] *= 1.05; }
            }
            let fused = fuse_view(&views, &maps, 0, &[1, 2, 3], &params).unwrap();
            for i in 0..96 * 96 {
                assert_eq!(fused.points[i].is_some(), !in_block(i % 96, i / 96),
                    "{degrees} degree reference depth gate at {i}");
            }
            maps[0].depth = plane_depth(&views[0], degrees.to_radians());
            for i in 0..96 * 96 {
                maps[0].cost[i] = if in_block(i % 96, i / 96) { 0.5 } else { params.max_cost };
            }
            let fused = fuse_view(&views, &maps, 0, &[1, 2, 3], &params).unwrap();
            for i in 0..96 * 96 {
                assert_eq!(fused.points[i].is_some(), !in_block(i % 96, i / 96),
                    "{degrees} degree reference cost gate at {i}");
                if in_block(i % 96, i / 96) { assert_eq!(fused.agreeing[i], 0, "cost rejects hypothesis"); }
            }
        }
    }

    #[test]
    fn source_errors_remove_only_that_views_agreement() {
        for degrees in [0.0_f64, 30.0] {
            let angle = degrees.to_radians();
            let (views, mut maps) = scene(angle);
            let params = ConsistencyParams::default();
            let baseline = fuse_view(&views, &maps, 0, &[1, 2, 3], &params).unwrap();
            // Both geometric disagreement and unusable source hypotheses must
            // remove precisely the affected view, leaving other evidence intact.
            for (scale, cost) in [(1.05, 0.0), (1.0, 0.5), (0.0, 0.0),
                (-1.0, 0.0), (f64::NAN, 0.0), (f64::INFINITY, 0.0), (1.0, f64::NAN)]
            {
                maps[1].depth = plane_depth(&views[1], angle);
                for i in 0..96 * 96 {
                    maps[1].cost[i] = params.max_cost;
                    if in_block(i % 96, i / 96) {
                        maps[1].depth[i] *= scale;
                        maps[1].cost[i] = cost;
                    }
                }
                let fused = fuse_view(&views, &maps, 0, &[1, 2, 3], &params).unwrap();
                let mut affected = 0;
                for i in 0..96 * 96 {
                    let world = world_point(&views[0], i % 96, i / 96, maps[0].depth[i]);
                    let lost = landing(&views[1], world).is_some_and(|(x, y)| in_block(x, y));
                    affected += usize::from(lost);
                    let expected = baseline.agreeing[i] - u8::from(lost);
                    assert_eq!(fused.agreeing[i], expected, "{degrees} degree source gate at {i}, scale={scale}, cost={cost}");
                    assert_eq!(fused.points[i].is_some(), expected >= 2, "remaining evidence at {i}");
                }
                assert!(affected > 0, "source block must be observed");
            }
        }
    }

    #[test]
    fn minimum_views_and_empty_or_repeated_sources() {
        for degrees in [0.0_f64, 30.0] {
            let (views, maps) = scene(degrees.to_radians());
            let params = ConsistencyParams { min_views: 4, ..Default::default() };
            let fused = fuse_view(&views, &maps, 0, &[1, 2, 3], &params).unwrap();
            assert!(fused.points.iter().all(Option::is_none), "four agreements are impossible");
            assert!(fused.agreeing.contains(&3), "counts survive rejection");
            let repeated = fuse_view(&views, &maps, 0, &[1, 2, 3, 1], &params).unwrap();
            assert_eq!(repeated.agreeing, fused.agreeing, "repeats do not add evidence");
            assert!(repeated.points.iter().all(Option::is_none), "repeats do not pass gate");
            let params = ConsistencyParams { min_views: 0, ..params };
            let fused = fuse_view(&views, &maps, 0, &[], &params).unwrap();
            for i in 0..96 * 96 {
                assert_eq!(fused.agreeing[i], 0, "empty source count");
                assert_eq!(fused.points[i], Some(world_point(&views[0], i % 96, i / 96, maps[0].depth[i])),
                    "no-evidence mean is the reference point");
            }
        }
    }

    #[test]
    fn completion_measures_the_prior_near_where_it_is_used_and_checks_the_fusion_against_it() {
        let (views, maps) = scene(30.0_f64.to_radians());
        let view = &views[1];
        let depth = &maps[1].depth;
        let size = depth.len();
        // Fused points on the true plane left of column 60, none to the right; one fused point 10% too deep.
        let mut points: Vec<Option<[f64; 3]>> = (0..size).map(|i| (i % 96 < 60).then(|| world_point(view, i % 96, i / 96, depth[i]))).collect();
        points[10 * 96 + 10] = Some(world_point(view, 10, 10, depth[10 * 96 + 10] * 1.1));
        let fused = FusedMap { width: 96, height: 96, points, agreeing: vec![2; size], spread: vec![0.0005; size] };
        let fused_error = vec![0.0005; size];
        // The prior is the true depth 2% deep above row 48 and 4% deep from it; a 50% step on the block x >= 70,
        // 20 <= y < 40, and one unknown depth at (80, 60).
        let mut prior: Vec<f64> = depth
            .iter()
            .enumerate()
            .map(|(i, &d)| {
                let (x, y) = (i % 96, i / 96);
                d * if y < 48 { 1.02 } else { 1.04 } * if x >= 70 && (20..40).contains(&y) { 1.5 } else { 1.0 }
            })
            .collect();
        prior[60 * 96 + 80] = f64::NAN;
        let completed = complete_view(view, &fused, &fused_error, &prior, &CompletionParams::default()).unwrap();
        assert_eq!(completed.overlap, 60 * 96, "every fused pixel has a usable prior");
        assert_eq!(completed.unsupported, 0, "every usable prior pixel is within four tiles of the overlap");
        let centre = [view.cam_to_world[0][3], view.cam_to_world[1][3], view.cam_to_world[2][3]];
        let reach = |world: [f64; 3]| (0..3).map(|axis| (world[axis] - centre[axis]).powi(2)).sum::<f64>().sqrt();
        let at = |x: usize, y: usize| completed.points[y * 96 + x];
        // A prior f times too deep along the ray is 1 - 1/f of its distance off. Tile columns 4 and 5 (x >= 64) reach the
        // overlap (x < 60) within one and two tiles. Tiles in rows 0-1 and 4-5 see only one of the two priors that near;
        // tile row 2 sees both, a third of them 4% deep, and takes the 90% quantile's.
        for (x, y, factor) in [(70, 5, 1.02), (90, 5, 1.02), (65, 45, 1.04), (70, 90, 1.04), (90, 90, 1.04)] {
            let world = world_point(view, x, y, prior[y * 96 + x]);
            let point = at(x, y).unwrap();
            assert_eq!((point.world, point.source), (world, PointSource::Prior), "prior at ({x}, {y})");
            let expected = (1.0 - 1.0 / factor) * reach(world);
            assert!((point.expected_error - expected).abs() < 1e-12, "prior error at ({x}, {y}): {} vs {expected}", point.expected_error);
        }
        for (x, y) in [(80, 19), (80, 20), (80, 39), (80, 40), (69, 30), (70, 30), (80, 60), (81, 61)] {
            assert_eq!(at(x, y), None, "no usable prior at ({x}, {y}): a depth step or an unknown depth next to it");
        }
        // Fused points are checked against the whole view's 95% quantile, the 4% deep prior's: those that agree within both
        // errors keep theirs; the one 10% deep does not.
        for (i, point) in completed.points.iter().enumerate() {
            let Some(world) = fused.points[i] else { continue };
            let point = point.unwrap();
            assert_eq!((point.world, point.source), (world, PointSource::Fused), "fused at {i}");
            if i != 10 * 96 + 10 {
                assert_eq!(point.expected_error, 0.0005, "fused error at {i}");
            }
        }
        let wrong = at(10, 10).unwrap();
        let q = world_point(view, 10, 10, prior[10 * 96 + 10]);
        let apart = (0..3).map(|axis| (wrong.world[axis] - q[axis]).powi(2)).sum::<f64>().sqrt();
        let expected = apart + (1.0 - 1.0 / 1.04) * reach(q);
        assert!((wrong.expected_error - expected).abs() < 1e-12, "checked fused error {} vs {expected}", wrong.expected_error);
        // Within one tile the right two tile columns have no overlap: their prior points are unsupported and dropped.
        let near = complete_view(view, &fused, &fused_error, &prior, &CompletionParams { max_reach: 1, ..Default::default() }).unwrap();
        assert!(near.points.iter().enumerate().all(|(i, p)| i % 96 < 80 || p.is_none()), "no point beyond the reach");
        assert_eq!(near.unsupported, (0..size).filter(|&i| i % 96 >= 80 && completed.points[i].is_some()).count(), "unsupported count");
        assert!(complete_view(view, &fused, &fused_error, &prior[1..], &CompletionParams::default()).is_err(), "prior length");
        assert!(complete_view(view, &fused, &fused_error[1..], &prior, &CompletionParams::default()).is_err(), "fused error length");
        let mut bad_error = fused_error.clone();
        bad_error[0] = f64::NAN;
        assert!(complete_view(view, &fused, &bad_error, &prior, &CompletionParams::default()).is_err(), "fused error value");
        assert!(complete_view(view, &FusedMap { width: 95, ..fused }, &fused_error, &prior, &CompletionParams::default()).is_err(), "fused size");
        let fused = FusedMap { width: 96, height: 96, points: vec![None; size], agreeing: vec![0; size], spread: vec![0.0; size] };
        let none = complete_view(view, &fused, &fused_error, &prior, &CompletionParams::default()).unwrap();
        assert!(none.points.iter().all(Option::is_none) && none.unsupported > 0, "without fusion the prior is unmeasured");
        for bad in [0.0, 1.5, f64::NAN] {
            assert!(complete_view(view, &fused, &fused_error, &prior, &CompletionParams { error_quantile: bad, ..Default::default() }).is_err(), "quantile {bad}");
            assert!(complete_view(view, &fused, &fused_error, &prior, &CompletionParams { check_quantile: bad, ..Default::default() }).is_err(), "check quantile {bad}");
        }
        for bad in [-0.1, f64::NAN, f64::INFINITY] {
            assert!(complete_view(view, &fused, &fused_error, &prior, &CompletionParams { max_relative_step: bad, ..Default::default() }).is_err(), "step {bad}");
        }
        assert!(complete_view(view, &fused, &fused_error, &prior, &CompletionParams { min_overlap: 0, ..Default::default() }).is_err(), "min_overlap");
        assert!(complete_view(view, &fused, &fused_error, &prior, &CompletionParams { tile: 0, ..Default::default() }).is_err(), "tile");
    }

    fn with_pixels(view: &GrayView, pixels: Vec<f32>) -> GrayView {
        GrayView { width: view.width, height: view.height, pixels, k: view.k, cam_to_world: view.cam_to_world }
    }

    #[test]
    fn window_texture_finds_edges_and_their_lever() {
        let (views, _) = textured_plane(96, 120.0, 0.0, &[0.1]);
        // A vertical step at column 40: central differences are nonzero at columns 39 and 40.
        let edge = with_pixels(&views[0], (0..96 * 96).map(|i| if i % 96 < 40 { 0.2 } else { 0.8 }).collect());
        let (isotropy, lever) = window_texture(&edge, 37, 48, 4, 1);
        assert!(isotropy < 1e-12 && (lever - 2.5).abs() < 1e-12, "edge: isotropy {isotropy}, lever {lever}");
        let (isotropy, lever) = window_texture(&edge, 37, 48, 4, 2);
        assert!(isotropy < 1e-12 && (lever - 2.0).abs() < 1e-12, "edge at step 2: isotropy {isotropy}, lever {lever}");
        let flat = with_pixels(&views[0], vec![0.5; 96 * 96]);
        assert_eq!(window_texture(&flat, 48, 48, 4, 1), (0.0, 4.0), "no gradients");
        assert_eq!(window_texture(&flat, 0, 95, 4, 1), (0.0, 4.0), "clamped at the corner");
        let textured: Vec<f64> = (0..200).map(|k| window_texture(&views[0], 20 + k % 50, 20 + k / 50 * 10, 4, 1).0).collect();
        let isotropic = textured.iter().filter(|&&v| v >= 0.1).count();
        assert!(isotropic * 10 >= textured.len() * 9, "the plane's texture is isotropic: {isotropic} of {}", textured.len());
    }

    #[test]
    fn fused_errors_cover_the_spread_the_cost_width_and_edges() {
        let (views, maps) = scene(0.0);
        let others = [1, 2, 3];
        let fused = fuse_view(&views, &maps, 0, &others, &ConsistencyParams::default()).unwrap();
        let window = PatchMatchParams::default();
        let params = FusedErrorParams::default();
        let errors = fused_errors(&views, 0, &others, &maps[0], &fused, &window, &params).unwrap();
        let no_edges = FusedErrorParams { edge_isotropy: 0.0, ..FusedErrorParams::default() };
        let plain = fused_errors(&views, 0, &others, &maps[0], &fused, &window, &no_edges).unwrap();
        // Without the spread (on the truth it is the fused points' lateral offsets, near a millimetre) and the edge term,
        // the error is the width alone.
        let unspread = FusedMap { width: 96, height: 96, points: fused.points.clone(), agreeing: fused.agreeing.clone(), spread: vec![0.0; 96 * 96] };
        let widths = fused_errors(&views, 0, &others, &maps[0], &unspread, &window, &no_edges).unwrap();
        let (mut kept, mut same, mut rises) = (0, 0, Vec::new());
        for i in 0..96 * 96 {
            if fused.points[i].is_none() {
                assert_eq!(errors[i], 0.0, "no point, no error at {i}");
                continue;
            }
            kept += 1;
            assert!(errors[i].is_finite() && errors[i] >= fused.spread[i] && errors[i] >= plain[i], "error at {i}: {}", errors[i]);
            assert_eq!(plain[i], fused.spread[i].max(widths[i]), "the spread or the width at {i}");
            same += usize::from(errors[i] == plain[i]);
            if widths[i] > 0.0 {
                // The width: the cost, with the normal held, rises by about width_rise that far either side.
                let (x, y, d) = (i % 96, i / 96, maps[0].depth[i]);
                let cost = |d: f64| view_cost(&views, 0, &others, x, y, d, maps[0].normal[i], &window);
                rises.push((0.5 * (cost(d + widths[i]) + cost(d - widths[i])) - cost(d)) / params.width_rise);
            }
        }
        rises.sort_by(f64::total_cmp);
        let median = rises.get(rises.len() / 2).copied().unwrap_or(f64::NAN);
        println!("fused {kept}, edge term unchanged {same}, widths {}, median cost rise over width_rise {median:.3}", rises.len());
        assert!(kept * 2 > 96 * 96, "fused {kept}");
        assert!(same * 10 >= kept * 9, "the edge term rarely fires on texture: {same} of {kept} unchanged");
        assert!(rises.len() * 2 >= kept, "most truth points have a width: {} of {kept}", rises.len());
        assert!((0.5..2.0).contains(&median), "median cost rise over width_rise {median}");
        // An edge in the reference: a pixel near it is extrapolated by the window's lever, a flat window by its radius.
        let edged: Vec<GrayView> = views
            .iter()
            .enumerate()
            .map(|(v, view)| with_pixels(view, if v == 0 { (0..96 * 96).map(|i| if i % 96 < 40 { 0.2 } else { 0.8 }).collect() } else { view.pixels.clone() }))
            .collect();
        let errors = fused_errors(&edged, 0, &others, &maps[0], &fused, &window, &params).unwrap();
        for (x, lever) in [(37, 2.0), (60, 4.0)] {
            let i = 48 * 96 + x;
            assert!(fused.points[i].is_some(), "({x}, 48) is fused");
            let edge = params.edge_slope * lever * maps[0].depth[i] / 120.0;
            assert!(errors[i] >= edge - 1e-12, "edge error at ({x}, 48): {} below {edge}", errors[i]);
        }
        assert!(fused_errors(&views, 0, &[0], &maps[0], &fused, &window, &params).is_err(), "reference as source");
        assert!(fused_errors(&views, 0, &[4], &maps[0], &fused, &window, &params).is_err(), "source index");
        assert!(fused_errors(&views, 4, &others, &maps[0], &fused, &window, &params).is_err(), "reference index");
        for bad in [
            FusedErrorParams { width_step: 0.0, ..FusedErrorParams::default() },
            FusedErrorParams { width_rise: f64::NAN, ..FusedErrorParams::default() },
            FusedErrorParams { edge_slope: -1.0, ..FusedErrorParams::default() },
        ] {
            assert!(fused_errors(&views, 0, &others, &maps[0], &fused, &window, &bad).is_err(), "bad parameters");
        }
        assert!(fused_errors(&views, 0, &others, &maps[0], &fused, &PatchMatchParams { step: 0, ..window }, &params).is_err(), "window step");
        assert!(fused_errors(&views, 0, &others, &maps[1], &FusedMap { width: 95, ..fused }, &PatchMatchParams::default(), &params).is_err(), "fused size");
    }

    #[test]
    fn invalid_arguments_are_errors() {
        let (mut views, mut maps) = scene(0.0);
        let params = ConsistencyParams::default();
        assert!(fuse_view(&[], &[], 0, &[], &params).is_err(), "empty views");
        assert!(fuse_view(&views, &maps, 4, &[1], &params).is_err(), "reference index");
        assert!(fuse_view(&views, &maps, 0, &[4], &params).is_err(), "other index");
        assert!(fuse_view(&views, &maps, 0, &[0], &params).is_err(), "reference in others");
        assert!(fuse_view(&views, &maps[..3], 0, &[1], &params).is_err(), "map count");
        for invalid in [-1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            for thresholds in [[invalid, 0.01, 0.3], [1.0, invalid, 0.3], [1.0, 0.01, invalid]] {
                let params = ConsistencyParams { max_reprojection_px: thresholds[0],
                    max_relative_depth: thresholds[1], max_cost: thresholds[2], min_views: 2 };
                assert!(fuse_view(&views, &maps, 0, &[1], &params).is_err(), "invalid thresholds");
            }
        }
        let zeros = ConsistencyParams { max_reprojection_px: 0.0, max_relative_depth: 0.0, max_cost: 0.0, min_views: 0 };
        assert!(fuse_view(&views, &maps, 0, &[], &zeros).is_ok(), "zero thresholds are valid");
        for index in 0..views.len() {
            views[index].k[0] = 0.0;
            assert!(fuse_view(&views, &maps, 0, &[1], &params).is_err(), "invalid view {index}, including unused views");
            views[index].k[0] = 120.0;
            maps[index].width -= 1;
            assert!(fuse_view(&views, &maps, 0, &[1], &params).is_err(), "map width {index}");
            maps[index].width += 1;
            maps[index].height -= 1;
            assert!(fuse_view(&views, &maps, 0, &[1], &params).is_err(), "map height {index}");
            maps[index].height += 1;
            let depth = maps[index].depth.pop().unwrap();
            assert!(fuse_view(&views, &maps, 0, &[1], &params).is_err(), "depth length {index}");
            maps[index].depth.push(depth);
            let normal = maps[index].normal.pop().unwrap();
            assert!(fuse_view(&views, &maps, 0, &[1], &params).is_err(), "normal length {index}");
            maps[index].normal.push(normal);
            maps[index].cost.push(0.0);
            assert!(fuse_view(&views, &maps, 0, &[1], &params).is_err(), "cost length {index}");
            maps[index].cost.pop();
        }
    }
}
