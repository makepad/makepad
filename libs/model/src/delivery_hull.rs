//! Fitted collision proxies. Hulls are built from support points in a fixed
//! direction set once an object is dense, which bounds both the work and the
//! delivered triangle count while staying inside the true hull.
use makepad_csg_math::portable::PortableFloat;
use crate::transform::*;

const DIRECTIONS: usize = 256;

fn support_points(points: &[[f64; 3]]) -> Vec<[f64; 3]> {
    if points.len() <= DIRECTIONS { return points.to_vec(); }
    let golden = std::f64::consts::PI * (3. - 5f64.sqrt());
    let mut out: Vec<[f64; 3]> = Vec::with_capacity(DIRECTIONS);
    for i in 0..DIRECTIONS {
        let y = 1. - 2. * (i as f64 + 0.5) / DIRECTIONS as f64;
        let r = (1. - y * y).sqrt();
        let d = [r * (golden * i as f64).pcos(), y, r * (golden * i as f64).psin()];
        let best = points.iter().copied().max_by(|a, b| dot(*a, d).total_cmp(&dot(*b, d))).unwrap();
        if !out.contains(&best) { out.push(best); }
    }
    out
}

/// Incremental convex hull. Returns positions and outward triangles, or None
/// for flat/degenerate input.
pub(crate) fn convex_hull(points: &[[f64; 3]]) -> Option<(Vec<[f64; 3]>, Vec<[u32; 3]>)> {
    let p = support_points(points);
    if p.len() < 4 { return None; }
    let scale = p.iter().map(|v| length(*v)).fold(1e-6, f64::max);
    let eps = scale * 1e-9;
    let i0 = (0..p.len()).min_by(|&a, &b| p[a][0].total_cmp(&p[b][0]))?;
    let i1 = (0..p.len()).max_by(|&a, &b| length(sub(p[a], p[i0])).total_cmp(&length(sub(p[b], p[i0]))))?;
    let line = |q: [f64; 3]| length(cross(sub(q, p[i0]), sub(p[i1], p[i0])));
    let i2 = (0..p.len()).max_by(|&a, &b| line(p[a]).total_cmp(&line(p[b])))?;
    let plane_n = cross(sub(p[i1], p[i0]), sub(p[i2], p[i0]));
    let plane = |q: [f64; 3]| dot(sub(q, p[i0]), plane_n).abs();
    let i3 = (0..p.len()).max_by(|&a, &b| plane(p[a]).total_cmp(&plane(p[b])))?;
    if line(p[i2]) <= eps || plane(p[i3]) <= eps * scale { return None; }
    let inside = mul(add(add(p[i0], p[i1]), add(p[i2], p[i3])), 0.25);
    let mut faces: Vec<[usize; 3]> = Vec::new();
    let orient = |f: [usize; 3]| -> [usize; 3] {
        let n = cross(sub(p[f[1]], p[f[0]]), sub(p[f[2]], p[f[0]]));
        if dot(n, sub(p[f[0]], inside)) < 0. { [f[0], f[2], f[1]] } else { f }
    };
    for f in [[i0, i1, i2], [i0, i1, i3], [i0, i2, i3], [i1, i2, i3]] { faces.push(orient(f)); }
    let above = |f: &[usize; 3], q: [f64; 3]| { let n = cross(sub(p[f[1]], p[f[0]]), sub(p[f[2]], p[f[0]])); let l = length(n); l > 0. && dot(n, sub(q, p[f[0]])) / l > eps };
    for (i, &q) in p.iter().enumerate() {
        if [i0, i1, i2, i3].contains(&i) { continue; }
        let visible: Vec<bool> = faces.iter().map(|f| above(f, q)).collect();
        if !visible.iter().any(|v| *v) { continue; }
        let mut edges = std::collections::BTreeSet::new();
        for (f, _) in faces.iter().zip(&visible).filter(|(_, v)| **v) { for k in 0..3 { edges.insert((f[k], f[(k + 1) % 3])); } }
        let mut next: Vec<[usize; 3]> = faces.iter().zip(&visible).filter(|(_, v)| !**v).map(|(f, _)| *f).collect();
        for &(a, b) in &edges { if !edges.contains(&(b, a)) { next.push([a, b, i]); } }
        faces = next;
    }
    let mut remap = std::collections::BTreeMap::new();
    let mut positions = Vec::new();
    let triangles = faces.iter().map(|f| f.map(|v| *remap.entry(v).or_insert_with(|| { positions.push(p[v]); (positions.len() - 1) as u32 }))).collect();
    Some((positions, triangles))
}

/// A closed capsule around the longest bounds axis, covering the other two.
pub(crate) fn capsule(bounds: [[f64; 3]; 2]) -> (Vec<[f64; 3]>, Vec<[u32; 3]>) {
    let half: [f64; 3] = std::array::from_fn(|d| (bounds[1][d] - bounds[0][d]) * 0.5);
    let center: [f64; 3] = std::array::from_fn(|d| (bounds[1][d] + bounds[0][d]) * 0.5);
    let axis = (0..3).max_by(|&a, &b| half[a].total_cmp(&half[b])).unwrap();
    let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
    let radius = half[u].phypot(half[v]).min(half[axis]).max(1e-4);
    let core = (half[axis] - radius).max(0.);
    let (segments, rings) = (12usize, 3usize);
    let mut positions = Vec::new();
    let mut ring_ids: Vec<Vec<u32>> = Vec::new();
    let ring = |positions: &mut Vec<[f64; 3]>, along: f64, r: f64| -> Vec<u32> {
        (0..segments).map(|j| { let t = std::f64::consts::TAU * j as f64 / segments as f64; let mut q = center; q[axis] += along; q[u] += r * t.pcos(); q[v] += r * t.psin(); positions.push(q); (positions.len() - 1) as u32 }).collect()
    };
    for k in 1..=rings { let phi = -std::f64::consts::FRAC_PI_2 + std::f64::consts::FRAC_PI_2 * k as f64 / rings as f64; ring_ids.push(ring(&mut positions, -core + radius * phi.psin(), radius * phi.pcos())); }
    for k in 0..rings { let phi = std::f64::consts::FRAC_PI_2 * k as f64 / rings as f64; ring_ids.push(ring(&mut positions, core + radius * phi.psin(), radius * phi.pcos())); }
    let mut pole = |sign: f64| { let mut q = center; q[axis] += sign * (core + radius); positions.push(q); (positions.len() - 1) as u32 };
    let (south, north) = (pole(-1.), pole(1.));
    let mut triangles = Vec::new();
    for j in 0..segments {
        let k = (j + 1) % segments;
        triangles.push([south, ring_ids[0][k], ring_ids[0][j]]);
        for r in 0..ring_ids.len() - 1 { let (a, b) = (&ring_ids[r], &ring_ids[r + 1]); triangles.push([a[j], a[k], b[k]]); triangles.push([a[j], b[k], b[j]]); }
        let last = &ring_ids[ring_ids.len() - 1];
        triangles.push([last[j], last[k], north]);
    }
    // Orientation follows the (u, v, axis) handedness; flip to face outward.
    let outward = { let t = triangles[1]; let n = cross(sub(positions[t[1] as usize], positions[t[0] as usize]), sub(positions[t[2] as usize], positions[t[0] as usize])); dot(n, sub(positions[t[0] as usize], center)) > 0. };
    if !outward { for t in &mut triangles { t.swap(1, 2); } }
    (positions, triangles)
}
