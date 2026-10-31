//! Solids from sketch regions: extrude (with draft), revolve, sweep along a
//! path and loft between sections. Every result is a closed, consistently
//! outward-oriented triangle mesh with shared vertices.
//!
//! Frame: the sketch lies in the XY plane; extrusion goes along +Z;
//! revolve turns the sketch about its Y axis (x is the radius).

use crate::region::{area, offset_loop, resample, signed_area2, Loop, Region, P2};
use crate::sketch::Tolerance;
use makepad_csg_math::{dvec3, Vec3d};
use makepad_csg_mesh::mesh::TriMesh;
use std::f64::consts::TAU;

/// Signed volume (positive when the triangles face outward).
pub fn signed_volume(m: &TriMesh) -> f64 {
    m.triangles.iter().map(|t| {
        let (a, b, c) = (m.vertices[t[0] as usize], m.vertices[t[1] as usize], m.vertices[t[2] as usize]);
        a.dot(b.cross(c))
    }).sum::<f64>() / 6.0
}

/// Turn the mesh outward if it came out inside-out.
pub fn orient_outward(m: &mut TriMesh) {
    if signed_volume(m) < 0.0 {
        for t in &mut m.triangles {
            t.swap(1, 2);
        }
    }
}

fn cap(m: &mut TriMesh, region: &Region, ids: &[u32], flip: bool) -> Result<(), String> {
    let (_, tris) = region.triangulate();
    if tris.is_empty() {
        return Err("a cap of the sketch could not be triangulated".into());
    }
    for t in tris {
        let (a, b, c) = (ids[t[0] as usize], ids[t[1] as usize], ids[t[2] as usize]);
        if flip {
            m.add_triangle(a, c, b);
        } else {
            m.add_triangle(a, b, c);
        }
    }
    Ok(())
}

/// Extrude regions `height` along +Z. `draft_deg` tapers the side walls
/// inward going up (negative flares them out); `both` centres the extrusion
/// on the sketch plane.
pub fn extrude(regions: &[Region], height: f64, draft_deg: f64, both: bool) -> Result<TriMesh, String> {
    if !(height > 0.0) {
        return Err("the extrusion height must be positive".into());
    }
    if draft_deg.abs() >= 80.0 {
        return Err("a draft must stay under 80 degrees".into());
    }
    let (z0, z1) = if both { (-height / 2.0, height / 2.0) } else { (0.0, height) };
    let inset = height * draft_deg.to_radians().tan();
    let mut m = TriMesh::new();
    for (ri, r) in regions.iter().enumerate() {
        // Draft: the top outline moved inward by `inset` (the bottom stays).
        let top = if inset == 0.0 { r.clone() } else {
            r.offset(-inset).ok_or_else(|| format!("region {ri}: a {draft_deg} degree draft closes the top over {height} mm"))?
        };
        if top.holes.len() != r.holes.len() {
            return Err(format!("region {ri}: the draft closes a hole before the top"));
        }
        let bottom_ids: Vec<u32> = r.loops().flat_map(|l| l.iter()).map(|p| m.add_vertex(dvec3(p[0], p[1], z0))).collect();
        let top_ids: Vec<u32> = top.loops().flat_map(|l| l.iter()).map(|p| m.add_vertex(dvec3(p[0], p[1], z1))).collect();
        cap(&mut m, r, &bottom_ids, true)?;
        cap(&mut m, &top, &top_ids, false)?;
        let mut base = 0;
        for l in r.loops() {
            let n = l.len();
            for i in 0..n {
                let j = (i + 1) % n;
                let (b0, b1, t0, t1) = (bottom_ids[base + i], bottom_ids[base + j], top_ids[base + i], top_ids[base + j]);
                // Outer loops are counter-clockwise, holes clockwise: in both
                // cases the material is on the left, so (b0, b1, t1) faces out.
                m.add_triangle(b0, b1, t1);
                m.add_triangle(b0, t1, t0);
            }
            base += n;
        }
    }
    orient_outward(&mut m);
    Ok(m)
}

/// Revolve regions about the sketch's Y axis by `angle_deg` (360: a full
/// turn). The sketch must stay on one side of the axis (x >= 0); points on
/// the axis close the solid there.
pub fn revolve(regions: &[Region], angle_deg: f64, tol: &Tolerance) -> Result<TriMesh, String> {
    if !(angle_deg > 0.0) || angle_deg > 360.0 {
        return Err("the revolve angle must be in (0, 360] degrees".into());
    }
    let full = (angle_deg - 360.0).abs() < 1e-9;
    let angle = angle_deg.to_radians();
    let max_r = regions.iter().flat_map(|r| r.loops().flat_map(|l| l.iter())).map(|p| p[0]).fold(0.0, f64::max);
    for r in regions {
        if r.loops().flat_map(|l| l.iter()).any(|p| p[0] < -1e-9) {
            return Err("a revolve sketch must stay on the +X side of its axis".into());
        }
    }
    let steps = tol.segments(max_r.max(0.1), angle).max(3);
    let rings = if full { steps } else { steps + 1 };
    let on_axis = |p: &P2| p[0].abs() < 1e-9;
    let at = |p: &P2, k: usize| {
        let a = angle * k as f64 / steps as f64;
        dvec3(p[0] * a.cos(), p[1], -p[0] * a.sin())
    };
    let mut m = TriMesh::new();
    for r in regions {
        // ids[k][point]: the vertex of each sketch point at step k (axis points shared).
        let pts: Vec<P2> = r.loops().flat_map(|l| l.iter().copied()).collect();
        let mut axis_ids: Vec<Option<u32>> = vec![None; pts.len()];
        for (i, p) in pts.iter().enumerate() {
            if on_axis(p) {
                axis_ids[i] = Some(m.add_vertex(dvec3(0.0, p[1], 0.0)));
            }
        }
        let ids: Vec<Vec<u32>> = (0..rings).map(|k| pts.iter().enumerate().map(|(i, p)| axis_ids[i].unwrap_or_else(|| m.add_vertex(at(p, k)))).collect()).collect();
        let mut base = 0;
        for l in r.loops() {
            let n = l.len();
            for i in 0..n {
                let j = (i + 1) % n;
                if on_axis(&pts[base + i]) && on_axis(&pts[base + j]) {
                    continue;
                }
                for k in 0..steps {
                    let k1 = if full { (k + 1) % steps } else { k + 1 };
                    let (a0, a1, b0, b1) = (ids[k][base + i], ids[k][base + j], ids[k1][base + i], ids[k1][base + j]);
                    // Turning toward -Z: with the material left of a loop,
                    // (a0, b0, a1) faces away from it.
                    if a0 != b0 {
                        m.add_triangle(a0, b0, a1);
                    }
                    if a1 != b1 {
                        m.add_triangle(a1, b0, b1);
                    }
                }
            }
            base += n;
        }
        if !full {
            cap(&mut m, r, &ids[0], false)?;
            cap(&mut m, r, &ids[steps], true)?;
        }
    }
    drop_degenerate(&mut m);
    orient_outward(&mut m);
    Ok(m)
}

fn drop_degenerate(m: &mut TriMesh) {
    m.triangles.retain(|t| t[0] != t[1] && t[1] != t[2] && t[0] != t[2]);
}

/// Rotation-minimising frames along a polyline (double reflection): for each
/// point, (tangent, normal, binormal).
pub fn frames(path: &[Vec3d], closed: bool) -> Vec<(Vec3d, Vec3d, Vec3d)> {
    let n = path.len();
    let tangent = |i: usize| -> Vec3d {
        let prev = if i > 0 { path[i - 1] } else if closed { path[n - 1] } else { path[i] };
        let next = if i + 1 < n { path[i + 1] } else if closed { path[0] } else { path[i] };
        let t = next - prev;
        if t.length() < 1e-12 { dvec3(0.0, 0.0, 1.0) } else { t.normalize() }
    };
    let t0 = tangent(0);
    let helper = if t0.x.abs() < 0.9 { dvec3(1.0, 0.0, 0.0) } else { dvec3(0.0, 1.0, 0.0) };
    let mut r = t0.cross(helper).cross(t0).normalize();
    let mut out = Vec::with_capacity(n);
    out.push((t0, r, t0.cross(r)));
    for i in 0..n - 1 {
        let ti = tangent(i);
        let tj = tangent(i + 1);
        let v1 = path[i + 1] - path[i];
        let c1 = v1.dot(v1);
        if c1 < 1e-24 {
            out.push((tj, r, tj.cross(r)));
            continue;
        }
        let r_l = r - v1 * (2.0 / c1 * v1.dot(r));
        let t_l = ti - v1 * (2.0 / c1 * v1.dot(ti));
        let v2 = tj - t_l;
        let c2 = v2.dot(v2);
        r = if c2 < 1e-24 { r_l } else { r_l - v2 * (2.0 / c2 * v2.dot(r_l)) };
        out.push((tj, r, tj.cross(r)));
    }
    out
}

/// Sweep regions along a 3D polyline. The sketch's X follows the path's
/// normal and Y its binormal; `twist_deg` turns the section evenly along the
/// way and `scale_end` scales it from 1 at the start to this at the end. A
/// closed path (first point repeated or `closed`) joins end to start.
pub fn sweep(regions: &[Region], path: &[Vec3d], closed: bool, twist_deg: f64, scale_end: f64) -> Result<TriMesh, String> {
    let mut path: Vec<Vec3d> = path.to_vec();
    path.dedup_by(|a, b| a.distance(*b) < 1e-9);
    let mut closed = closed;
    if path.len() > 2 && path[0].distance(*path.last().unwrap()) < 1e-9 {
        path.pop();
        closed = true;
    }
    if path.len() < 2 {
        return Err("a sweep path needs two or more distinct points".into());
    }
    // A twist or a taper needs stations along each straight run: at most 5
    // degrees of twist and 2% of scale per step.
    if twist_deg != 0.0 || scale_end != 1.0 {
        let steps = ((twist_deg.abs() / 5.0).max((scale_end - 1.0).abs() / 0.02).ceil() as usize).max(1);
        let total: f64 = path.windows(2).map(|w| w[0].distance(w[1])).sum();
        let max_step = total / steps as f64;
        let mut fine = vec![path[0]];
        for w in path.windows(2) {
            let n = (w[0].distance(w[1]) / max_step).ceil().max(1.0) as usize;
            for k in 1..=n {
                fine.push(w[0].lerp(w[1], k as f64 / n as f64));
            }
        }
        path = fine;
    }
    let fr = frames(&path, closed);
    let n = path.len();
    // Closing a loop: spread the frame's leftover rotation over the way.
    let mut fix = 0.0;
    if closed {
        let (_, r_end, b_end) = fr[n - 1];
        let r0 = fr[0].1;
        fix = r0.dot(b_end).atan2(r0.dot(r_end));
    }
    let total_len: f64 = (0..n - 1).map(|i| path[i].distance(path[i + 1])).sum::<f64>();
    let mut cum = vec![0.0];
    for i in 0..n - 1 {
        cum.push(cum[i] + path[i].distance(path[i + 1]));
    }
    let place = |p: &P2, i: usize| -> Vec3d {
        let f = if total_len > 0.0 { cum[i] / total_len } else { 0.0 };
        let a = twist_deg.to_radians() * f + if closed { fix * f } else { 0.0 };
        let s = 1.0 + (scale_end - 1.0) * if closed { 0.0 } else { f };
        let (x, y) = (s * (p[0] * a.cos() - p[1] * a.sin()), s * (p[0] * a.sin() + p[1] * a.cos()));
        let (_, nrm, bin) = fr[i];
        path[i] + nrm * x + bin * y
    };
    let mut m = TriMesh::new();
    for r in regions {
        let pts: Vec<P2> = r.loops().flat_map(|l| l.iter().copied()).collect();
        let ids: Vec<Vec<u32>> = (0..n).map(|i| pts.iter().map(|p| m.add_vertex(place(p, i))).collect()).collect();
        let stations = if closed { n } else { n - 1 };
        let mut base = 0;
        for l in r.loops() {
            let len = l.len();
            for k in 0..len {
                let j = (k + 1) % len;
                for s in 0..stations {
                    let s1 = (s + 1) % n;
                    let (a0, a1, b0, b1) = (ids[s][base + k], ids[s][base + j], ids[s1][base + k], ids[s1][base + j]);
                    m.add_triangle(a0, a1, b1);
                    m.add_triangle(a0, b1, b0);
                }
            }
            base += len;
        }
        if !closed {
            cap(&mut m, r, &ids[0], true)?;
            cap(&mut m, r, &ids[n - 1], false)?;
        }
    }
    orient_outward(&mut m);
    Ok(m)
}

/// One section of a loft: a closed outline in a plane parallel to XY at `z`,
/// offset by `at` in that plane.
#[derive(Clone, Debug)]
pub struct Section {
    pub outline: Loop,
    pub z: f64,
}

/// Loft through sections in order of z: every outline resampled to the same
/// count, started at the point nearest +X, joined by straight bands. Holes
/// are not lofted (one outline per section).
pub fn loft(sections: &[Section], tol: &Tolerance) -> Result<TriMesh, String> {
    if sections.len() < 2 {
        return Err("a loft needs two or more sections".into());
    }
    for w in sections.windows(2) {
        if !(w[1].z > w[0].z) {
            return Err("loft sections must rise in z".into());
        }
    }
    let longest = sections.iter().map(|s| perimeter(&s.outline)).fold(0.0, f64::max);
    let count = sections.iter().map(|s| s.outline.len()).max().unwrap_or(3).max(((longest / (tol.chord * 25.0).max(0.2)) as usize).clamp(48, 720));
    let mut rings: Vec<Loop> = Vec::new();
    for (i, s) in sections.iter().enumerate() {
        if s.outline.len() < 3 || area(&s.outline) < 1e-12 {
            return Err(format!("section {i} has no area"));
        }
        let ccw = if signed_area2(&s.outline) < 0.0 { s.outline.iter().rev().copied().collect() } else { s.outline.clone() };
        let c = centroid(&ccw);
        rings.push(resample(&ccw, count, Some([c[0] + 1e6, c[1]])));
    }
    let mut m = TriMesh::new();
    let ids: Vec<Vec<u32>> = rings.iter().zip(sections).map(|(r, s)| r.iter().map(|p| m.add_vertex(dvec3(p[0], p[1], s.z))).collect()).collect();
    for k in 0..rings.len() - 1 {
        for i in 0..count {
            let j = (i + 1) % count;
            let (a0, a1, b0, b1) = (ids[k][i], ids[k][j], ids[k + 1][i], ids[k + 1][j]);
            m.add_triangle(a0, a1, b1);
            m.add_triangle(a0, b1, b0);
        }
    }
    let first = Region { outer: rings[0].clone(), holes: Vec::new() };
    let last = Region { outer: rings[rings.len() - 1].clone(), holes: Vec::new() };
    cap(&mut m, &first, &ids[0], true)?;
    cap(&mut m, &last, &ids[rings.len() - 1], false)?;
    orient_outward(&mut m);
    Ok(m)
}

fn perimeter(l: &[P2]) -> f64 {
    let n = l.len();
    (0..n).map(|i| {
        let (a, b) = (l[i], l[(i + 1) % n]);
        (b[0] - a[0]).hypot(b[1] - a[1])
    }).sum()
}

fn centroid(l: &[P2]) -> P2 {
    let n = l.len() as f64;
    [l.iter().map(|p| p[0]).sum::<f64>() / n, l.iter().map(|p| p[1]).sum::<f64>() / n]
}

/// A circle of points for path helpers: a helix of `turns` round the Z axis.
pub fn helix(radius: f64, pitch: f64, turns: f64, tol: &Tolerance) -> Vec<Vec3d> {
    let steps = (tol.segments(radius, TAU) as f64 * turns).ceil().max(8.0) as usize;
    (0..=steps).map(|i| {
        let a = TAU * turns * i as f64 / steps as f64;
        dvec3(radius * a.cos(), radius * a.sin(), pitch * turns * i as f64 / steps as f64)
    }).collect()
}

/// Offset helper re-exported for drafts on single loops.
pub fn inset_loop(l: &[P2], d: f64) -> Option<Loop> {
    offset_loop(l, -d)
}
