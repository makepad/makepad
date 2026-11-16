//! Measuring meshes: a bounding-volume hierarchy for first-hit rays and
//! closest points, wall thickness, clearance between parts, connected shells
//! and slivers, and plane sections (outlines, areas, widths, a raster).

use crate::region::{regions_from_loops, Loop, Region, P2};
use makepad_csg_math::{dvec3, Vec3d};
use makepad_csg_mesh::mesh::TriMesh;
use makepad_csg_mesh::bvh::Bvh;
use std::collections::HashMap;

/// A fitted circle; lengths use the input coordinates' units. `rms` is the
/// root mean square distance to the circle, including out-of-plane error.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Circle3 {
    pub center: [f64; 3],
    pub normal: [f64; 3],
    pub radius: f64,
    pub rms: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlaneGap {
    pub distance: f64,
    /// Unoriented angle between planes, in [0, 90] degrees.
    pub angle_deg: f64,
    pub parallel: bool,
}

fn xyz(p: [f64; 3]) -> Vec3d {
    dvec3(p[0], p[1], p[2])
}

fn array(p: Vec3d) -> [f64; 3] {
    [p.x, p.y, p.z]
}

/// Polyline length. Repeat the first point at the end for a closed chain.
pub fn edge_length(points: &[[f64; 3]]) -> f64 {
    points.windows(2).map(|w| xyz(w[0]).distance(xyz(w[1]))).sum()
}

/// Angle between directions in degrees; an undefined direction yields NaN.
pub fn angle_between(a_dir: [f64; 3], b_dir: [f64; 3]) -> f64 {
    let (a, b) = (xyz(a_dir), xyz(b_dir));
    let len = a.length() * b.length();
    if len == 0.0 { return f64::NAN; }
    (a.dot(b) / len).clamp(-1.0, 1.0).acos().to_degrees()
}

/// Parallel (including opposed) normals within 0.5 degrees give the gap
/// along `na`. Otherwise distance is between the picked points.
pub fn plane_distance(pa: [f64; 3], na: [f64; 3], pb: [f64; 3], nb: [f64; 3]) -> PlaneGap {
    let angle = angle_between(na, nb);
    let angle_deg = if angle > 90.0 { 180.0 - angle } else { angle };
    let parallel = angle_deg <= 0.5;
    let delta = xyz(pb) - xyz(pa);
    let distance = if parallel { delta.dot(xyz(na).normalize()).abs() } else { delta.length() };
    PlaneGap { distance, angle_deg, parallel }
}

fn solve3(mut a: [[f64; 3]; 3], mut b: [f64; 3]) -> Option<[f64; 3]> {
    let scale = a.iter().flatten().fold(0.0f64, |s, v| s.max(v.abs()));
    for k in 0..3 {
        let pivot = (k..3).max_by(|&i, &j| a[i][k].abs().total_cmp(&a[j][k].abs()))?;
        if a[pivot][k].abs() <= scale * 1e-14 { return None; }
        a.swap(k, pivot);
        b.swap(k, pivot);
        for i in k + 1..3 {
            let f = a[i][k] / a[k][k];
            for j in k..3 { a[i][j] -= f * a[k][j]; }
            b[i] -= f * b[k];
        }
    }
    let mut x = [0.0; 3];
    for i in (0..3).rev() {
        x[i] = (b[i] - (i + 1..3).map(|j| a[i][j] * x[j]).sum::<f64>()) / a[i][i];
    }
    x.iter().all(|v| v.is_finite()).then_some(x)
}

/// Least-squares plane (smallest covariance eigenvector), Kasa circle in
/// that plane, then geometric Gauss-Newton refinement. Collinear or invalid
/// inputs have no circle. Centering and scaling keep small parts well conditioned.
pub fn fit_circle3(points: &[[f64; 3]]) -> Option<Circle3> {
    if points.len() < 3 || points.iter().flatten().any(|v| !v.is_finite()) { return None; }
    let origin = xyz(points[0]);
    let mean = points.iter().fold(dvec3(0.0, 0.0, 0.0), |s, p| s + (xyz(*p) - origin)) / points.len() as f64;
    let center = origin + mean;
    let scale = points.iter().map(|p| (xyz(*p) - center).length()).fold(0.0f64, f64::max);
    if scale == 0.0 || !scale.is_finite() { return None; }
    let ps: Vec<Vec3d> = points.iter().map(|p| (xyz(*p) - center) / scale).collect();
    let mut cov = [[0.0; 3]; 3];
    for p in &ps {
        let p = array(*p);
        for i in 0..3 { for j in 0..3 { cov[i][j] += p[i] * p[j]; } }
    }
    let mut axes = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    // Jacobi rotations diagonalise a symmetric 3x3 covariance matrix.
    for _ in 0..32 {
        let (i, j) = [(0, 1), (0, 2), (1, 2)].into_iter()
            .max_by(|&(a, b), &(c, d)| cov[a][b].abs().total_cmp(&cov[c][d].abs()))?;
        if cov[i][j].abs() <= 1e-15 * (cov[0][0] + cov[1][1] + cov[2][2]) { break; }
        let theta = 0.5 * (2.0 * cov[i][j]).atan2(cov[j][j] - cov[i][i]);
        let (s, c) = theta.sin_cos();
        let (ai, aj, off) = (cov[i][i], cov[j][j], cov[i][j]);
        cov[i][i] = c * c * ai - 2.0 * s * c * off + s * s * aj;
        cov[j][j] = s * s * ai + 2.0 * s * c * off + c * c * aj;
        cov[i][j] = 0.0;
        cov[j][i] = 0.0;
        for k in 0..3 {
            if k != i && k != j {
                let (a, b) = (cov[k][i], cov[k][j]);
                cov[k][i] = c * a - s * b;
                cov[i][k] = cov[k][i];
                cov[k][j] = s * a + c * b;
                cov[j][k] = cov[k][j];
            }
            let (a, b) = (axes[k][i], axes[k][j]);
            axes[k][i] = c * a - s * b;
            axes[k][j] = s * a + c * b;
        }
    }
    let mut order = [0, 1, 2];
    order.sort_by(|&i, &j| cov[i][i].total_cmp(&cov[j][j]));
    if cov[order[1]][order[1]] <= cov[order[2]][order[2]] * 1e-12 { return None; }
    let axis = |i: usize| dvec3(axes[0][i], axes[1][i], axes[2][i]).normalize();
    let n = axis(order[0]);
    let u = axis(order[2]);
    let v = n.cross(u).normalize();
    let xy: Vec<[f64; 2]> = ps.iter().map(|p| [p.dot(u), p.dot(v)]).collect();
    let mut a = [[0.0; 3]; 3];
    let mut b = [0.0; 3];
    for &[x, y] in &xy {
        let row = [2.0 * x, 2.0 * y, 1.0];
        for i in 0..3 {
            b[i] += row[i] * (x * x + y * y);
            for j in 0..3 { a[i][j] += row[i] * row[j]; }
        }
    }
    let [mut cx, mut cy, k] = solve3(a, b)?;
    let mut radius = (k + cx * cx + cy * cy).sqrt();
    let error = |cx: f64, cy: f64, r: f64| xy.iter().map(|p| ((p[0] - cx).hypot(p[1] - cy) - r).powi(2)).sum::<f64>();
    for _ in 0..8 {
        let mut a = [[0.0; 3]; 3];
        let mut b = [0.0; 3];
        for &[x, y] in &xy {
            let d = (cx - x).hypot(cy - y);
            if d <= f64::EPSILON { continue; }
            let row = [(cx - x) / d, (cy - y) / d, -1.0];
            for i in 0..3 {
                b[i] -= row[i] * (d - radius);
                for j in 0..3 { a[i][j] += row[i] * row[j]; }
            }
        }
        let Some(step) = solve3(a, b) else { break };
        let old_error = error(cx, cy, radius);
        let mut f = 1.0;
        for _ in 0..8 {
            if radius + f * step[2] > 0.0 && error(cx + f * step[0], cy + f * step[1], radius + f * step[2]) <= old_error { break; }
            f *= 0.5;
        }
        if radius + f * step[2] <= 0.0 || error(cx + f * step[0], cy + f * step[1], radius + f * step[2]) > old_error { break; }
        cx += f * step[0];
        cy += f * step[1];
        radius += f * step[2];
        if step.iter().all(|s| (f * s).abs() < 1e-14) { break; }
    }
    if !radius.is_finite() || radius <= 0.0 { return None; }
    let rms = ((error(cx, cy, radius) + ps.iter().map(|p| p.dot(n).powi(2)).sum::<f64>()) / ps.len() as f64).sqrt() * scale;
    Some(Circle3 { center: array(center + (u * cx + v * cy) * scale), normal: array(n), radius: radius * scale, rms })
}

/// Closest point on an open polyline (repeat the first point to close it).
pub fn closest_on_chain(points: &[[f64; 3]], at: [f64; 3]) -> Option<[f64; 3]> {
    let at = xyz(at);
    points.windows(2).map(|w| {
        let (a, b) = (xyz(w[0]), xyz(w[1]));
        let d = b - a;
        let t = if d.length_sq() > 0.0 { ((at - a).dot(d) / d.length_sq()).clamp(0.0, 1.0) } else { 0.0 };
        a + d * t
    }).min_by(|a, b| a.distance(at).total_cmp(&b.distance(at))).map(array)
}

/// Nearest feature chain, using the solid edge walk in both directions.
/// Positions use millimetres, matching `feature_edges`' welding tolerance.
/// The closing segment is explicit. No mesh types cross this API boundary.
pub fn feature_edge_chain(positions: &[[f64; 3]], indices: &[u32], at: [f64; 3]) -> Option<Vec<[f64; 3]>> {
    if indices.len() % 3 != 0 || indices.iter().any(|&i| i as usize >= positions.len()) { return None; }
    let mesh = TriMesh {
        vertices: positions.iter().map(|p| xyz(*p)).collect(),
        triangles: indices.chunks_exact(3).map(|t| [t[0], t[1], t[2]]).collect(),
        tags: Vec::new(),
        corner_normals: Vec::new(),
    };
    crate::feature_edges(&mesh, 20.0).into_iter().map(|edge| {
        let mut points: Vec<_> = edge.points.into_iter().map(array).collect();
        if edge.closed && !points.is_empty() { points.push(points[0]); }
        points
    }).filter_map(|points| {
        let p = closest_on_chain(&points, at)?;
        Some((xyz(p).distance(xyz(at)), points))
    }).min_by(|a, b| a.0.total_cmp(&b.0)).map(|(_, points)| points)
}

#[cfg(test)]
mod geometry_tests {
    use super::*;

    #[test]
    fn tilted_circle_36_points() {
        let center = dvec3(12.0, -4.0, 25.0);
        let u = dvec3(0.8, 0.6, 0.0);
        let v = dvec3(-0.36, 0.48, 0.8);
        let points: Vec<_> = (0..36).map(|i| {
            let t = std::f64::consts::TAU * i as f64 / 36.0;
            array(center + (u * t.cos() + v * t.sin()) * 7.5)
        }).collect();
        let c = fit_circle3(&points).unwrap();
        assert!((c.radius - 7.5).abs() < 1e-9, "{c:?}");
        assert!(xyz(c.center).distance(center) < 1e-9);
        assert!(xyz(c.normal).dot(u.cross(v)).abs() > 1.0 - 1e-12);
        assert!(c.rms < 1e-9);
    }

    #[test]
    fn noisy_three_points_and_degenerate_inputs() {
        let c = fit_circle3(&[[1.001, 0.002, 0.001], [0.001, 0.999, -0.002], [-0.998, -0.001, 0.002]]).unwrap();
        assert!((c.radius - 1.0).abs() < 0.01, "{c:?}");
        assert!(c.rms < 1e-10);
        assert!(fit_circle3(&[[0.0; 3], [1.0; 3]]).is_none());
        assert!(fit_circle3(&[[0.0; 3], [1.0; 3], [2.0; 3]]).is_none());
        assert!(fit_circle3(&[[1.0; 3]; 3]).is_none());
        assert!(fit_circle3(&[[f64::NAN; 3]; 3]).is_none());
    }

    #[test]
    fn circle_rms_includes_nonplanarity() {
        let points: Vec<_> = (0..36).map(|i| {
            let t = std::f64::consts::TAU * i as f64 / 36.0;
            [7.5 * t.cos(), 7.5 * t.sin(), if i % 2 == 0 { 0.01 } else { -0.01 }]
        }).collect();
        let c = fit_circle3(&points).unwrap();
        assert!((c.radius - 7.5).abs() < 1e-9);
        assert!((c.rms - 0.01).abs() < 1e-9);
    }

    #[test]
    fn opposite_box_faces_are_20_mm_apart() {
        let gap = plane_distance([0.0; 3], [-2.0, 0.0, 0.0], [20.0, 3.0, 4.0], [1.0, 0.0, 0.0]);
        assert!(gap.parallel);
        assert_eq!(gap.distance, 20.0);
        assert_eq!(gap.angle_deg, 0.0);
    }

    #[test]
    fn faces_at_30_degrees_report_angle_and_point_distance() {
        let t = 30f64.to_radians();
        let gap = plane_distance([0.0; 3], [1.0, 0.0, 0.0], [3.0, 4.0, 0.0], [t.cos(), t.sin(), 0.0]);
        assert!(!gap.parallel);
        assert!((gap.angle_deg - 30.0).abs() < 1e-12);
        assert_eq!(gap.distance, 5.0);
        assert!((angle_between([2.0, 0.0, 0.0], [0.0, 3.0, 0.0]) - 90.0).abs() < 1e-12);
    }

    #[test]
    fn edge_length_sums_the_chain() {
        assert_eq!(edge_length(&[]), 0.0);
        assert_eq!(edge_length(&[[0.0; 3]]), 0.0);
        assert_eq!(edge_length(&[[0.0; 3], [3.0, 0.0, 0.0], [3.0, 4.0, 0.0]]), 7.0);
    }
}

/// Wall thickness at one place: from `p` (on the surface, outward normal
/// `n`) straight in to the far side. `None` when the ray leaves the solid
/// (an open mesh).
pub fn thickness_at(bvh: &Bvh, p: Vec3d, n: Vec3d, skip: Option<u32>) -> Option<f64> {
    bvh.ray(p - n * 1e-6, n * -1.0, 1e4, skip).map(|(t, _)| t)
}

/// One thickness sample of a mesh.
#[derive(Clone, Copy, Debug)]
pub struct ThicknessSample {
    pub at: Vec3d,
    pub normal: Vec3d,
    pub thickness: f64,
    /// Surface area this sample stands for (mm²).
    pub area: f64,
    /// The far side faces back within 30 degrees: a wall of this thickness. Otherwise a wedge
    /// (a thread's ridge, a chamfered edge), which only narrows at its tip.
    pub wall: bool,
}

/// Thickness from every triangle's centroid (at most `max` samples, the
/// largest triangles first) straight into the material.
pub fn thickness_samples(mesh: &TriMesh, max: usize) -> Vec<ThicknessSample> {
    let bvh = Bvh::build(mesh);
    let mut idx: Vec<usize> = (0..mesh.triangles.len()).collect();
    let area = |i: usize| mesh.triangle_area(i);
    if idx.len() > max {
        idx.sort_by(|a, b| area(*b).total_cmp(&area(*a)));
        idx.truncate(max);
    }
    let mut out = Vec::with_capacity(idx.len());
    for i in idx {
        let (a, b, c) = mesh.triangle_vertices(i);
        let n = mesh.triangle_normal(i);
        if !(n.length() > 0.5) {
            continue;
        }
        let at = (a + b + c) / 3.0;
        if let Some((t, far)) = bvh.ray(at - n * 1e-6, n * -1.0, 1e4, Some(i as u32)) {
            let wall = mesh.triangle_normal(far as usize).dot(n) < -(30f64.to_radians().cos());
            out.push(ThicknessSample { at, normal: n, thickness: t, area: area(i), wall });
        }
    }
    out
}

/// The smallest gap between two meshes (closest pair over every vertex of
/// each against the other's surface): (gap, point on a, point on b). Zero
/// or less when they touch or overlap (the sign is not resolved).
pub fn clearance(a: &TriMesh, b: &TriMesh) -> Option<(f64, Vec3d, Vec3d)> {
    let (ba, bb) = (Bvh::build(a), Bvh::build(b));
    let mut best: Option<(f64, Vec3d, Vec3d)> = None;
    for v in &a.vertices {
        if let Some((d, q, _)) = bb.closest(*v) {
            if best.map_or(true, |x| d < x.0) {
                best = Some((d, *v, q));
            }
        }
    }
    for v in &b.vertices {
        if let Some((d, q, _)) = ba.closest(*v) {
            if best.map_or(true, |x| d < x.0) {
                best = Some((d, q, *v));
            }
        }
    }
    best
}

/// One connected shell of a mesh.
#[derive(Clone, Debug)]
pub struct Shell {
    pub triangles: usize,
    pub volume: f64,
    pub lo: Vec3d,
    pub hi: Vec3d,
}

/// The mesh's connected pieces (triangles sharing a vertex position), largest volume first.
pub fn shells(mesh: &TriMesh) -> Vec<Shell> {
    let q = 1e-6;
    let mut ids: HashMap<(i64, i64, i64), u32> = HashMap::new();
    let key = |v: &Vec3d| ((v.x / q).round() as i64, (v.y / q).round() as i64, (v.z / q).round() as i64);
    let pos: Vec<u32> = mesh.vertices.iter().map(|v| {
        let n = ids.len() as u32;
        *ids.entry(key(v)).or_insert(n)
    }).collect();
    let mut parent: Vec<u32> = (0..ids.len() as u32).collect();
    fn find(p: &mut Vec<u32>, x: u32) -> u32 {
        let mut r = x;
        while p[r as usize] != r {
            r = p[r as usize];
        }
        let mut y = x;
        while p[y as usize] != r {
            let next = p[y as usize];
            p[y as usize] = r;
            y = next;
        }
        r
    }
    for t in &mesh.triangles {
        let (a, b, c) = (pos[t[0] as usize], pos[t[1] as usize], pos[t[2] as usize]);
        let (ra, rb, rc) = (find(&mut parent, a), find(&mut parent, b), find(&mut parent, c));
        parent[rb as usize] = ra;
        let rc2 = find(&mut parent, rc);
        parent[rc2 as usize] = ra;
    }
    let mut groups: HashMap<u32, Shell> = HashMap::new();
    for t in &mesh.triangles {
        let r = find(&mut parent, pos[t[0] as usize]);
        let (a, b, c) = (mesh.vertices[t[0] as usize], mesh.vertices[t[1] as usize], mesh.vertices[t[2] as usize]);
        let s = groups.entry(r).or_insert(Shell { triangles: 0, volume: 0.0, lo: a, hi: a });
        s.triangles += 1;
        s.volume += a.dot(b.cross(c)) / 6.0;
        for p in [a, b, c] {
            s.lo = s.lo.min(p);
            s.hi = s.hi.max(p);
        }
    }
    let mut out: Vec<Shell> = groups.into_values().collect();
    out.sort_by(|a, b| b.volume.abs().total_cmp(&a.volume.abs()));
    out
}

/// Triangles thinner than `min_altitude` (their smallest height): slivers.
pub fn sliver_count(mesh: &TriMesh, min_altitude: f64) -> usize {
    (0..mesh.triangles.len()).filter(|&i| {
        let (a, b, c) = mesh.triangle_vertices(i);
        let area2 = (b - a).cross(c - a).length();
        let longest = a.distance(b).max(b.distance(c)).max(c.distance(a));
        longest > 0.0 && area2 / longest < min_altitude
    }).count()
}

/// The mesh cut by the plane `axis = value` (axis 0 x, 1 y, 2 z), as
/// regions in that plane's (u, v): x-cut (y, z), y-cut (x, z), z-cut (x, y).
pub fn section(mesh: &TriMesh, axis: usize, value: f64) -> Vec<Region> {
    let comp = |p: &Vec3d| p.component(axis);
    let uv = |p: Vec3d| -> P2 {
        match axis {
            0 => [p.y, p.z],
            1 => [p.x, p.z],
            _ => [p.x, p.y],
        }
    };
    // Segments where triangles cross the plane (vertices exactly on it nudged).
    let eps = 1e-9;
    let mut segs: Vec<(P2, P2)> = Vec::new();
    for t in &mesh.triangles {
        let ps: Vec<Vec3d> = t.iter().map(|&i| mesh.vertices[i as usize]).collect();
        let d: Vec<f64> = ps.iter().map(|p| {
            let x = comp(p) - value;
            if x.abs() < eps { eps } else { x }
        }).collect();
        let mut pts = Vec::with_capacity(2);
        for k in 0..3 {
            let (i, j) = (k, (k + 1) % 3);
            if (d[i] > 0.0) != (d[j] > 0.0) {
                let f = d[i] / (d[i] - d[j]);
                pts.push(uv(ps[i] + (ps[j] - ps[i]) * f));
            }
        }
        if pts.len() == 2 {
            segs.push((pts[0], pts[1]));
        }
    }
    // Chain the segments into loops by their quantised end points.
    let q = 1e-6;
    let key = |p: P2| ((p[0] / q).round() as i64, (p[1] / q).round() as i64);
    let mut by_end: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (i, s) in segs.iter().enumerate() {
        by_end.entry(key(s.0)).or_default().push(i);
        by_end.entry(key(s.1)).or_default().push(i);
    }
    let mut used = vec![false; segs.len()];
    let mut loops: Vec<Loop> = Vec::new();
    for start in 0..segs.len() {
        if used[start] {
            continue;
        }
        used[start] = true;
        let mut l = vec![segs[start].0, segs[start].1];
        loop {
            let tip = *l.last().unwrap();
            let next = by_end.get(&key(tip)).and_then(|list| list.iter().copied().find(|&i| !used[i]));
            let Some(i) = next else { break };
            used[i] = true;
            let s = segs[i];
            l.push(if key(s.0) == key(tip) { s.1 } else { s.0 });
            if key(*l.last().unwrap()) == key(l[0]) {
                l.pop();
                break;
            }
        }
        if l.len() >= 3 {
            loops.push(l);
        }
    }
    regions_from_loops(loops)
}

/// The narrowest place of a section: from points along each outline,
/// straight across the material to the far boundary. (width, at).
pub fn section_min_width(regions: &[Region], step: f64) -> Option<(f64, P2)> {
    let edges: Vec<(P2, P2)> = regions.iter().flat_map(|r| r.loops().flat_map(|l| (0..l.len()).map(move |i| (l[i], l[(i + 1) % l.len()])))).collect();
    let mut best: Option<(f64, P2)> = None;
    for (a, b) in &edges {
        let len = (b[0] - a[0]).hypot(b[1] - a[1]);
        if len < 1e-12 {
            continue;
        }
        // Inward: left of a counter-clockwise outer loop, left of a clockwise hole too.
        let n = [-(b[1] - a[1]) / len, (b[0] - a[0]) / len];
        let k = (len / step).ceil().max(1.0) as usize;
        for s in 0..k {
            let f = (s as f64 + 0.5) / k as f64;
            let p = [a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f];
            let o = [p[0] + n[0] * 1e-7, p[1] + n[1] * 1e-7];
            let mut hit = f64::INFINITY;
            for (c, d) in &edges {
                if let Some(t) = ray_seg(o, n, *c, *d) {
                    if t > 1e-9 {
                        hit = hit.min(t);
                    }
                }
            }
            if hit.is_finite() && best.map_or(true, |x| hit < x.0) {
                best = Some((hit, p));
            }
        }
    }
    best
}

fn ray_seg(o: P2, d: P2, a: P2, b: P2) -> Option<f64> {
    let e = [b[0] - a[0], b[1] - a[1]];
    let den = d[0] * e[1] - d[1] * e[0];
    if den.abs() < 1e-15 {
        return None;
    }
    let w = [a[0] - o[0], a[1] - o[1]];
    let t = (w[0] * e[1] - w[1] * e[0]) / den;
    let u = (w[0] * d[1] - w[1] * d[0]) / den;
    if (-1e-12..=1.0 + 1e-12).contains(&u) {
        Some(t)
    } else {
        None
    }
}

/// A section drawn as a grey image (material dark on light, a 10 mm grid):
/// (width, height, pixels row-major top-down, mm per pixel, origin u/v at pixel 0,0).
pub fn raster(regions: &[Region], max_side: usize) -> (usize, usize, Vec<u8>, f64, P2) {
    let mut lo = [f64::INFINITY; 2];
    let mut hi = [f64::NEG_INFINITY; 2];
    for r in regions {
        let (a, b) = r.bounds();
        for k in 0..2 {
            lo[k] = lo[k].min(a[k]);
            hi[k] = hi[k].max(b[k]);
        }
    }
    if !lo[0].is_finite() {
        return (1, 1, vec![255], 1.0, [0.0, 0.0]);
    }
    let margin = ((hi[0] - lo[0]).max(hi[1] - lo[1]) * 0.05).max(1.0);
    let (lo, hi) = ([lo[0] - margin, lo[1] - margin], [hi[0] + margin, hi[1] + margin]);
    let mm = ((hi[0] - lo[0]).max(hi[1] - lo[1]) / max_side as f64).max(1e-3);
    let w = ((hi[0] - lo[0]) / mm).ceil() as usize;
    let h = ((hi[1] - lo[1]) / mm).ceil() as usize;
    let mut px = vec![245u8; w * h];
    // Grid lines every 10 mm.
    for y in 0..h {
        let v = hi[1] - (y as f64 + 0.5) * mm;
        for x in 0..w {
            let u = lo[0] + (x as f64 + 0.5) * mm;
            if (u / 10.0 - (u / 10.0).round()).abs() * 10.0 < mm * 0.5 || (v / 10.0 - (v / 10.0).round()).abs() * 10.0 < mm * 0.5 {
                px[y * w + x] = 215;
            }
        }
    }
    // Even-odd scanline fill over every loop.
    let edges: Vec<(P2, P2)> = regions.iter().flat_map(|r| r.loops().flat_map(|l| (0..l.len()).map(move |i| (l[i], l[(i + 1) % l.len()])))).collect();
    for y in 0..h {
        let v = hi[1] - (y as f64 + 0.5) * mm;
        let mut xs: Vec<f64> = Vec::new();
        for (a, b) in &edges {
            if (a[1] > v) != (b[1] > v) {
                xs.push(a[0] + (v - a[1]) / (b[1] - a[1]) * (b[0] - a[0]));
            }
        }
        xs.sort_by(f64::total_cmp);
        for pair in xs.chunks(2) {
            if pair.len() < 2 {
                break;
            }
            let x0 = (((pair[0] - lo[0]) / mm).round().max(0.0) as usize).min(w);
            let x1 = (((pair[1] - lo[0]) / mm).round().max(0.0) as usize).min(w);
            for x in x0..x1 {
                px[y * w + x] = 60;
            }
        }
    }
    (w, h, px, mm, [lo[0], hi[1]])
}
