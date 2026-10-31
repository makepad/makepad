//! Edges of a solid mesh and fillets / chamfers on them, also on boolean
//! results: the mesh's sharp edges are found from its faces, chained into
//! straight lines and circles, and each chain gets a tool solid built from
//! its cross-section, subtracted from a convex edge or added to a concave
//! one with the exact booleans.
//!
//! A fillet tool's arc is drawn a hair ([`arc_dip`]) larger than asked so it
//! cuts each face at a shallow angle instead of touching it tangentially,
//! which a mesh boolean cannot resolve; the faces end within that distance
//! of where an exact fillet would meet them.

use crate::region::{oriented, Region, P2};
use crate::sketch::Tolerance;
use crate::solids::{extrude, orient_outward, revolve};
use makepad_csg_math::{dvec3, Vec3d};
use makepad_csg_mesh::mesh::TriMesh;
use std::collections::HashMap;
use std::f64::consts::PI;

/// How much larger a fillet tool's arc is than the fillet: 2% of the radius,
/// at least 0.02 mm (the tool then crosses each face at about 11 degrees,
/// well clear of what the booleans can resolve on faceted faces).
fn arc_dip(r: f64) -> f64 {
    (r * 0.02).max(0.02)
}
/// How far a line tool runs past each end of its edge (mm).
const LINE_OVERRUN: f64 = 0.02;

/// One sharp edge chain of a mesh.
#[derive(Clone, Debug)]
pub struct EdgeChain {
    pub points: Vec<Vec3d>,
    pub closed: bool,
    pub shape: EdgeShape,
    /// The material angle between the two faces exceeds 180 degrees (an
    /// inside corner): a fillet there adds material.
    pub concave: bool,
    /// Outward normal of the face on each side (at the first segment).
    pub n1: Vec3d,
    pub n2: Vec3d,
    /// The angle between the two faces' normals, degrees (0 = flat).
    pub bend_deg: f64,
    /// Each segment's face normals (n1 left of the chain's direction), one
    /// per segment (a closed chain's last one joins the end to the start).
    pub segment_normals: Vec<(Vec3d, Vec3d)>,
}

#[derive(Clone, Debug)]
pub enum EdgeShape {
    Line { a: Vec3d, b: Vec3d },
    Circle { center: Vec3d, axis: Vec3d, radius: f64 },
    /// Neither straight nor a full circle.
    Curve,
}

impl EdgeChain {
    pub fn length(&self) -> f64 {
        let mut l: f64 = self.points.windows(2).map(|w| w[0].distance(w[1])).sum();
        if self.closed && self.points.len() > 2 {
            l += self.points[self.points.len() - 1].distance(self.points[0]);
        }
        l
    }

    pub fn midpoint(&self) -> Vec3d {
        match &self.shape {
            EdgeShape::Line { a, b } => (*a + *b) * 0.5,
            EdgeShape::Circle { center, .. } => *center,
            EdgeShape::Curve => self.points[self.points.len() / 2],
        }
    }

    /// The edge's direction for a straight edge (unit), else `None`.
    pub fn direction(&self) -> Option<Vec3d> {
        match &self.shape {
            EdgeShape::Line { a, b } => Some((*b - *a).normalize()),
            _ => None,
        }
    }

    pub fn kind_name(&self) -> &'static str {
        match self.shape {
            EdgeShape::Line { .. } => "line",
            EdgeShape::Circle { .. } => "circle",
            EdgeShape::Curve => "curve",
        }
    }
}

/// What to do to an edge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EdgeOp {
    Fillet(f64),
    Chamfer(f64),
}

fn quantize(v: Vec3d, q: f64) -> (i64, i64, i64) {
    ((v.x / q).round() as i64, (v.y / q).round() as i64, (v.z / q).round() as i64)
}

/// The mesh's sharp edges (faces meeting at more than `min_bend_deg`),
/// chained through smooth vertices.
pub fn feature_edges(mesh: &TriMesh, min_bend_deg: f64) -> Vec<EdgeChain> {
    // Weld by position so boolean output with duplicated vertices still has adjacency.
    let q = 1e-5;
    let mut ids: HashMap<(i64, i64, i64), u32> = HashMap::new();
    let mut pos: Vec<Vec3d> = Vec::new();
    let remap: Vec<u32> = mesh.vertices.iter().map(|v| {
        *ids.entry(quantize(*v, q)).or_insert_with(|| {
            pos.push(*v);
            pos.len() as u32 - 1
        })
    }).collect();
    let tris: Vec<[u32; 3]> = mesh.triangles.iter().map(|t| [remap[t[0] as usize], remap[t[1] as usize], remap[t[2] as usize]]).filter(|t| t[0] != t[1] && t[1] != t[2] && t[0] != t[2]).collect();
    let normal = |t: &[u32; 3]| -> Vec3d {
        let (a, b, c) = (pos[t[0] as usize], pos[t[1] as usize], pos[t[2] as usize]);
        let n = (b - a).cross(c - a);
        if n.length() < 1e-30 { n } else { n.normalize() }
    };
    // Directed edge -> triangle.
    let mut directed: HashMap<(u32, u32), usize> = HashMap::new();
    for (ti, t) in tris.iter().enumerate() {
        for k in 0..3 {
            directed.insert((t[k], t[(k + 1) % 3]), ti);
        }
    }
    let cos_min = min_bend_deg.to_radians().cos();
    // Sharp edges, each once as (a, b) where the triangle with a -> b is side 1.
    let mut sharp: Vec<(u32, u32, Vec3d, Vec3d, bool)> = Vec::new();
    for (&(a, b), &t1) in &directed {
        let Some(&t2) = directed.get(&(b, a)) else { continue };
        if a > b {
            continue;
        }
        let (n1, n2) = (normal(&tris[t1]), normal(&tris[t2]));
        if n1.dot(n2) > cos_min {
            continue;
        }
        let other = tris[t2].iter().copied().find(|v| *v != a && *v != b).unwrap_or(a);
        let concave = n1.dot(pos[other as usize] - pos[a as usize]) > 1e-12;
        sharp.push((a, b, n1, n2, concave));
    }
    // A HashMap's order changes from run to run: sort, so the chains (their
    // starts, seams and tools) are the same every time.
    sharp.sort_by(|x, y| (x.0, x.1).cmp(&(y.0, y.1)));
    // Chain through vertices where exactly two sharp edges meet, turning little.
    let mut at: HashMap<u32, Vec<usize>> = HashMap::new();
    for (i, e) in sharp.iter().enumerate() {
        at.entry(e.0).or_default().push(i);
        at.entry(e.1).or_default().push(i);
    }
    let mut seg_of: HashMap<(u32, u32), usize> = HashMap::new();
    for (i, e) in sharp.iter().enumerate() {
        seg_of.insert((e.0.min(e.1), e.0.max(e.1)), i);
    }
    let mut used = vec![false; sharp.len()];
    let mut chains = Vec::new();
    let smooth_turn = 35f64.to_radians().cos();
    for start in 0..sharp.len() {
        if used[start] {
            continue;
        }
        used[start] = true;
        let (a, b, n1, n2, concave) = sharp[start];
        // Grow forward from b and backward from a.
        let mut verts: Vec<u32> = vec![a, b];
        for forward in [true, false] {
            loop {
                let (tip, prev) = if forward { (verts[verts.len() - 1], verts[verts.len() - 2]) } else { (verts[0], verts[1]) };
                let Some(list) = at.get(&tip) else { break };
                if list.len() != 2 {
                    break;
                }
                let next_edge = list.iter().copied().find(|&e| !used[e]);
                let Some(e) = next_edge else { break };
                let (ea, eb, _, _, ec) = sharp[e];
                if ec != concave {
                    break;
                }
                let next = if ea == tip { eb } else { ea };
                let d0 = (pos[tip as usize] - pos[prev as usize]).normalize();
                let d1 = (pos[next as usize] - pos[tip as usize]).normalize();
                if d0.dot(d1) < smooth_turn {
                    break;
                }
                used[e] = true;
                if forward {
                    verts.push(next);
                } else {
                    verts.insert(0, next);
                }
                if verts.first() == verts.last() {
                    break;
                }
            }
            if verts.first() == verts.last() {
                break;
            }
        }
        let closed = verts.len() > 3 && verts.first() == verts.last();
        if closed {
            verts.pop();
        }
        let points: Vec<Vec3d> = verts.iter().map(|v| pos[*v as usize]).collect();
        let segs = if closed { verts.len() } else { verts.len() - 1 };
        let segment_normals: Vec<(Vec3d, Vec3d)> = (0..segs).map(|i| {
            let (u, v) = (verts[i], verts[(i + 1) % verts.len()]);
            match seg_of.get(&(u.min(v), u.max(v))) {
                Some(&k) => {
                    let (ea, _, en1, en2, _) = sharp[k];
                    if ea == u { (en1, en2) } else { (en2, en1) }
                }
                None => (n1, n2),
            }
        }).collect();
        let bend_deg = n1.dot(n2).clamp(-1.0, 1.0).acos().to_degrees();
        // Side 1 must be the face left of the chain's direction: the seed
        // edge ran a -> b, which is the chain's order unless it was reversed.
        let seed_at = verts.iter().position(|v| *v == a).unwrap_or(0);
        let forward_seed = verts.get(seed_at + 1) == Some(&b) || (closed && seed_at + 1 == verts.len() && verts[0] == b);
        let (n1, n2) = if forward_seed { (n1, n2) } else { (n2, n1) };
        let (n1, n2) = segment_normals.first().copied().unwrap_or((n1, n2));
        let shape = classify(&points, closed);
        chains.push(EdgeChain { points, closed, shape, concave, n1, n2, bend_deg, segment_normals });
    }
    chains
}

fn classify(points: &[Vec3d], closed: bool) -> EdgeShape {
    let n = points.len();
    if n < 2 {
        return EdgeShape::Curve;
    }
    if !closed {
        let (a, b) = (points[0], points[n - 1]);
        let d = b - a;
        let len = d.length();
        if len > 1e-9 {
            let u = d / len;
            let straight = points.iter().all(|p| {
                let w = *p - a;
                (w - u * w.dot(u)).length() <= 1e-4 * len.max(1.0)
            });
            if straight {
                return EdgeShape::Line { a, b };
            }
        }
        return EdgeShape::Curve;
    }
    if n < 6 {
        return EdgeShape::Curve;
    }
    // A full circle: a plane through three spread points, equal radius.
    let (p0, p1, p2) = (points[0], points[n / 3], points[2 * n / 3]);
    let axis = (p1 - p0).cross(p2 - p0);
    if axis.length() < 1e-12 {
        return EdgeShape::Curve;
    }
    let axis = axis.normalize();
    let a = p1 - p0;
    let b = p2 - p0;
    let ab = a.cross(b);
    let center = p0 + (b * a.length_sq() - a * b.length_sq()).cross(ab) / (2.0 * ab.length_sq());
    let radius = center.distance(p0);
    let ok = points.iter().all(|p| ((*p - center).dot(axis)).abs() < 1e-3 * radius.max(1.0) && (p.distance(center) - radius).abs() < 2e-3 * radius.max(1.0));
    if ok {
        EdgeShape::Circle { center, axis, radius }
    } else {
        EdgeShape::Curve
    }
}

/// The tool's cross-section for an edge with outward face normals n1 (left
/// of the edge direction e) and n2, in the plane through the edge point P
/// across e. Returned in 3D (points of the section) and whether it adds
/// material.
fn section(e: Vec3d, n1: Vec3d, n2: Vec3d, concave: bool, op: EdgeOp, max_lift: f64, arc_steps: Option<usize>, tol: &Tolerance) -> Result<(Vec<Vec3d>, bool), String> {
    // Directions along each face, away from the edge.
    let f1 = n1.cross(e).normalize();
    let f2 = e.cross(n2).normalize();
    let theta = f1.dot(f2).clamp(-1.0, 1.0).acos();
    if theta < 5f64.to_radians() || theta > 175f64.to_radians() {
        return Err("the faces at this edge are almost flat or folded: nothing to round".into());
    }
    let w = (f1 + f2).normalize();
    // Outward (into the air) at the edge.
    let out = if concave { w } else { (n1 + n2).normalize() };
    let size = match op {
        EdgeOp::Fillet(r) | EdgeOp::Chamfer(r) => r,
    };
    if !(size > 0.0) {
        return Err("the size must be positive".into());
    }
    let lift = (size * 2.0 + 1.0).min(max_lift).max(size * 1.05);
    // Points of the section, starting on face 1 and ending on face 2.
    let (core, add) = match op {
        EdgeOp::Chamfer(d) => {
            let (a, b) = (f1 * d, f2 * d);
            let ab = (b - a).normalize();
            let eta = (d * 0.02).max(0.005);
            (vec![a - ab * eta, b + ab * eta], concave)
        }
        EdgeOp::Fillet(r) => {
            let t = r / (theta / 2.0).tan();
            let c = w * (r / (theta / 2.0).sin());
            let big_r = r + arc_dip(r);
            let delta = (big_r * big_r - r * r).sqrt();
            let x1 = f1 * (t - delta);
            let y1 = f2 * (t - delta);
            // The arc from x1 to y1 the short way round, on the edge's side.
            let ua = (x1 - c).normalize();
            let ub = (y1 - c).normalize();
            let axis = ua.cross(ub);
            let ang = ua.dot(ub).clamp(-1.0, 1.0).acos();
            let n_arc = arc_steps.unwrap_or_else(|| tol.segments(r, ang).max(2));
            let perp = axis.normalize().cross(ua);
            let mut pts = Vec::with_capacity(n_arc + 1);
            for k in 0..=n_arc {
                let s = ang * k as f64 / n_arc as f64;
                pts.push(c + (ua * s.cos() + perp * s.sin()) * big_r);
            }
            (pts, concave)
        }
    };
    let mut poly = core;
    let (last, first) = (*poly.last().unwrap(), poly[0]);
    if add {
        // Fill the inside corner, reaching back into the material.
        poly.push(last - n2 * lift);
        poly.push(out * -lift);
        poly.push(first - n1 * lift);
    } else {
        // Cut the outside corner, reaching out into the air.
        poly.push(last + n2 * lift);
        poly.push(out * lift);
        poly.push(first + n1 * lift);
    }
    Ok((poly, add))
}

/// The tool solid for one edge chain: (mesh, adds material).
pub fn edge_tool(chain: &EdgeChain, op: EdgeOp, tol: &Tolerance) -> Result<(TriMesh, bool), String> {
    match &chain.shape {
        // A long straight edge goes through the swept builder (stations along the run).
        EdgeShape::Line { a, b } if a.distance(*b) > (match op { EdgeOp::Fillet(r) | EdgeOp::Chamfer(r) => r * 3.0 }).max(1.0) => {
            let line = EdgeChain { points: vec![*a, *b], closed: false, segment_normals: vec![(chain.n1, chain.n2)], ..chain.clone() };
            swept_tool(&line, op, tol)
        }
        EdgeShape::Line { a, b } => {
            let e = (*b - *a).normalize();
            let (sec, add) = section(e, chain.n1, chain.n2, chain.concave, op, f64::INFINITY, None, tol)?;
            // Local frame: u = f1-ish, v = e x u, extrude along e.
            let u = chain.n1.cross(e).normalize();
            let v = e.cross(u);
            let pts: Vec<P2> = sec.iter().map(|p| [p.dot(u), p.dot(v)]).collect();
            let region = Region { outer: oriented(pts, true), holes: Vec::new() };
            let len = a.distance(*b) + 2.0 * LINE_OVERRUN;
            let mut m = extrude(&[region], len, 0.0, false)?;
            let origin = *a - e * LINE_OVERRUN;
            for p in &mut m.vertices {
                *p = origin + u * p.x + v * p.y + e * p.z;
            }
            orient_outward(&mut m);
            Ok((m, add))
        }
        EdgeShape::Circle { center, axis, radius } => {
            // The section at the first point, in (radial, axial) coordinates.
            let p0 = chain.points[0];
            let radial = (p0 - *center - *axis * (p0 - *center).dot(*axis)).normalize();
            // The chain runs round the circle; its tangent at p0.
            let next = chain.points[1];
            let mut e = (next - p0).normalize();
            e = (e - radial * e.dot(radial)).normalize();
            // A small circle: the tool must not reach across the axis.
            let (sec, add) = section(e, chain.n1, chain.n2, chain.concave, op, radius * 0.9, None, tol)?;
            let pts: Vec<P2> = sec.iter().map(|q| {
                let world = p0 + *q;
                let rel = world - *center;
                let ax = rel.dot(*axis);
                let rad = (rel - *axis * ax).length();
                [rad, ax]
            }).collect();
            if pts.iter().any(|p| p[0] < 0.0) {
                return Err(format!("the tool crosses the axis of this {:.2} mm circle: use a smaller size", 2.0 * radius));
            }
            let region = Region { outer: oriented(pts, true), holes: Vec::new() };
            let mut m = revolve(&[region], 360.0, tol)?;
            // revolve() turns about Y: map (x, y, z) -> centre + radial-plane basis.
            let ref_dir = radial;
            let other = axis.cross(ref_dir);
            for p in &mut m.vertices {
                *p = *center + ref_dir * p.x + *axis * p.y + other * (-p.z);
            }
            orient_outward(&mut m);
            Ok((m, add))
        }
        EdgeShape::Curve => swept_tool(chain, op, tol),
    }
}

/// A tool along any smooth chain: the cross-section at every chain point
/// (from that point's face normals), joined ring to ring.
fn swept_tool(chain: &EdgeChain, op: EdgeOp, tol: &Tolerance) -> Result<(TriMesh, bool), String> {
    let size = match op {
        EdgeOp::Fillet(r) | EdgeOp::Chamfer(r) => r,
    };
    // Boolean output leaves points a few microns apart along an edge: their
    // tangents are noise, and sections a hair apart but turned by degrees
    // cross each other. Keep points at least `gap` apart (the normals of the
    // segments dropped in between are averaged in).
    let gap = (size * 0.08).max(0.02);
    let mut pts: Vec<Vec3d> = Vec::new();
    let mut normals: Vec<(Vec3d, Vec3d)> = Vec::new();
    let mut acc = (dvec3(0.0, 0.0, 0.0), dvec3(0.0, 0.0, 0.0));
    let src = &chain.points;
    for (i, p) in src.iter().enumerate() {
        let s = chain.segment_normals.get(i).copied().unwrap_or(*chain.segment_normals.last().unwrap_or(&(chain.n1, chain.n2)));
        let last = i + 1 == src.len();
        if pts.last().map_or(true, |q: &Vec3d| q.distance(*p) >= gap) || (last && !chain.closed) {
            if !normals.is_empty() && acc.0.length() > 0.0 {
                let k = normals.len() - 1;
                normals[k] = (acc.0.normalize(), acc.1.normalize());
            }
            pts.push(*p);
            normals.push(s);
            acc = s;
        } else {
            acc = (acc.0 + s.0, acc.1 + s.1);
        }
    }
    if chain.closed && pts.len() > 2 && pts[0].distance(*pts.last().unwrap()) < gap {
        pts.pop();
        normals.pop();
    }
    // Long straight runs get stations every `run` mm: a band 47 mm long and
    // 0.1 mm wide meeting a face at a shallow angle defeats the booleans.
    let run = (size * 3.0).max(1.0);
    {
        let n0 = pts.len();
        let segs0 = if chain.closed { n0 } else { n0 - 1 };
        let mut p2 = Vec::with_capacity(n0);
        let mut n2 = Vec::with_capacity(n0);
        for i in 0..segs0 {
            let (a, b) = (pts[i], pts[(i + 1) % n0]);
            let k = (a.distance(b) / run).ceil().max(1.0) as usize;
            for j in 0..k {
                p2.push(a.lerp(b, j as f64 / k as f64));
                n2.push(normals[i]);
            }
        }
        if !chain.closed {
            p2.push(pts[n0 - 1]);
            n2.push(normals[n0 - 1]);
        }
        pts = p2;
        normals = n2;
    }
    // How far the tool may reach sideways: less than the tightest bend of
    // the chain (a hole's rim must not reach across the hole).
    let mut tightest = f64::INFINITY;
    {
        let n0 = pts.len();
        for i in 0..n0 {
            if !chain.closed && (i == 0 || i + 1 == n0) {
                continue;
            }
            let (a, b, c) = (pts[(i + n0 - 1) % n0], pts[i], pts[(i + 1) % n0]);
            let (ab, bc, ca) = (a.distance(b), b.distance(c), c.distance(a));
            let area2 = (b - a).cross(c - a).length();
            if area2 > 1e-12 {
                tightest = tightest.min(ab * bc * ca / (2.0 * area2));
            }
        }
    }
    let max_lift = (tightest * 0.8).max(size * 1.05);
    let pts = &pts;
    let n = pts.len();
    if n < 2 || (chain.closed && n < 3) {
        return Err("the edge is too short".into());
    }
    let segs = if chain.closed { n } else { n - 1 };
    let seg_normals = &normals;
    let seg_dir = |i: usize| (pts[(i + 1) % n] - pts[i]).normalize();
    let mut rings: Vec<Vec<Vec3d>> = Vec::with_capacity(n + 2);
    let mut frames: Vec<(Vec3d, Vec3d, Vec3d)> = Vec::with_capacity(n);
    let mut steps: Option<usize> = None;
    let mut add_all = None;
    for i in 0..n {
        // The segments either side of point i.
        let before = if i > 0 { Some(i - 1) } else if chain.closed { Some(segs - 1) } else { None };
        let after = if i < segs { Some(i) } else { None };
        let (mut e, mut n1, mut n2) = (dvec3(0.0, 0.0, 0.0), dvec3(0.0, 0.0, 0.0), dvec3(0.0, 0.0, 0.0));
        for s in [before, after].into_iter().flatten() {
            e = e + seg_dir(s);
            n1 = n1 + seg_normals[s].0;
            n2 = n2 + seg_normals[s].1;
        }
        let (e, n1, n2) = (e.normalize(), n1.normalize(), n2.normalize());
        if steps.is_none() {
            // Fix the arc's point count from the first section so rings match.
            if let EdgeOp::Fillet(r) = op {
                let f1 = n1.cross(e).normalize();
                let f2 = e.cross(n2).normalize();
                let theta = f1.dot(f2).clamp(-1.0, 1.0).acos();
                steps = Some(tol.segments(r, PI - theta).max(2));
            }
        }
        let (sec, add) = section(e, n1, n2, chain.concave, op, max_lift, steps, tol)?;
        if add_all.is_some_and(|a| a != add) {
            return Err("the edge changes from inside to outside corner".into());
        }
        add_all = Some(add);
        let u = n1.cross(e).normalize();
        let v = e.cross(u);
        frames.push((u, v, e));
        rings.push(sec.iter().map(|q| pts[i] + *q).collect());
    }
    // Open chains run a little past both ends.
    if !chain.closed {
        let (_, _, e0) = frames[0];
        let (_, _, e1) = frames[n - 1];
        for p in &mut rings[0] {
            *p = *p - e0 * LINE_OVERRUN;
        }
        for p in &mut rings[n - 1] {
            *p = *p + e1 * LINE_OVERRUN;
        }
    }
    // Orient every ring counter-clockwise in its frame (all the same way).
    let (u0, v0, _) = frames[0];
    let first: Vec<P2> = rings[0].iter().map(|p| [(*p - pts[0]).dot(u0), (*p - pts[0]).dot(v0)]).collect();
    if crate::region::signed_area2(&first) < 0.0 {
        for r in &mut rings {
            r.reverse();
        }
    }
    let k = rings[0].len();
    if rings.iter().any(|r| r.len() != k) {
        return Err("the edge's cross-section changes shape along the way".into());
    }
    let mut m = TriMesh::new();
    let ids: Vec<Vec<u32>> = rings.iter().map(|r| r.iter().map(|p| m.add_vertex(*p)).collect()).collect();
    let bands = if chain.closed { n } else { n - 1 };
    for i in 0..bands {
        let j = (i + 1) % n;
        for a in 0..k {
            let b = (a + 1) % k;
            m.add_triangle(ids[i][a], ids[i][b], ids[j][b]);
            m.add_triangle(ids[i][a], ids[j][b], ids[j][a]);
        }
    }
    if !chain.closed {
        for (end, flip) in [(0usize, true), (n - 1, false)] {
            let (u, v, _) = frames[end];
            let outline: Vec<P2> = rings[end].iter().map(|p| [(*p - pts[end]).dot(u), (*p - pts[end]).dot(v)]).collect();
            let region = Region { outer: outline, holes: Vec::new() };
            let (_, tris) = region.triangulate();
            for t in tris {
                let (a, b, c) = (ids[end][t[0] as usize], ids[end][t[1] as usize], ids[end][t[2] as usize]);
                if flip { m.add_triangle(a, c, b) } else { m.add_triangle(a, b, c) }
            }
        }
    }
    orient_outward(&mut m);
    Ok((m, add_all.unwrap_or(false)))
}

/// The most edges one fillet or chamfer rounds: each edge is a tool joined by a
/// boolean, so a selector that picks a tessellated surface's every crease would
/// run for minutes.
pub const MAX_EDGES: usize = 96;

/// Fillet or chamfer these edges of `mesh`: every tool is built first, then
/// cut and added with the booleans (cuts first, then the fills). `cancelled` is
/// asked between tools.
pub fn apply_edges(mesh: &TriMesh, chains: &[&EdgeChain], op: EdgeOp, tol: &Tolerance, cancelled: Option<&dyn Fn() -> bool>) -> Result<TriMesh, String> {
    if chains.is_empty() {
        return Err("no edges were selected".into());
    }
    if chains.len() > MAX_EDGES {
        return Err(format!("the selector picks {} edges (at most {MAX_EDGES}): narrow it, e.g. {{top: true}} or {{dir: @y convex: true}}", chains.len()));
    }
    let mut cut: Option<TriMesh> = None;
    let mut fill: Option<TriMesh> = None;
    for (i, c) in chains.iter().enumerate() {
        if cancelled.is_some_and(|c| c()) {
            return Err("cancelled".into());
        }
        let (tool, add) = edge_tool(c, op, tol).map_err(|e| format!("edge {i} ({} at {:.1}, {:.1}, {:.1}): {e}", c.kind_name(), c.midpoint().x, c.midpoint().y, c.midpoint().z))?;
        let slot = if add { &mut fill } else { &mut cut };
        *slot = Some(match slot.take() {
            None => tool,
            Some(acc) => crate::bool3::union(&acc, &tool),
        });
    }
    let mut out = mesh.clone();
    if let Some(c) = cut {
        out = crate::bool3::difference(&out, &c);
    }
    if let Some(f) = fill {
        out = crate::bool3::union(&out, &f);
    }
    if out.triangles.is_empty() {
        return Err("the edge tools removed the whole solid".into());
    }
    Ok(out)
}

/// A unit vector by name for edge selectors.
pub fn named_dir(name: &str) -> Option<Vec3d> {
    Some(match name {
        "x" | "+x" => dvec3(1.0, 0.0, 0.0),
        "-x" => dvec3(-1.0, 0.0, 0.0),
        "y" | "+y" => dvec3(0.0, 1.0, 0.0),
        "-y" => dvec3(0.0, -1.0, 0.0),
        "z" | "+z" => dvec3(0.0, 0.0, 1.0),
        "-z" => dvec3(0.0, 0.0, -1.0),
        _ => return None,
    })
}

/// Angle between two directions ignoring sign, degrees.
pub fn axis_angle_deg(a: Vec3d, b: Vec3d) -> f64 {
    a.normalize().dot(b.normalize()).abs().clamp(0.0, 1.0).acos() * 180.0 / PI
}
