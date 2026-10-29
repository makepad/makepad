//! Measuring meshes: a bounding-volume hierarchy for first-hit rays and
//! closest points, wall thickness, clearance between parts, connected shells
//! and slivers, and plane sections (outlines, areas, widths, a raster).

use crate::region::{regions_from_loops, Loop, Region, P2};
use makepad_csg_math::{dvec3, Vec3d};
use makepad_csg_mesh::mesh::TriMesh;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug)]
struct Aabb {
    lo: Vec3d,
    hi: Vec3d,
}

impl Aabb {
    fn empty() -> Aabb {
        Aabb { lo: dvec3(f64::INFINITY, f64::INFINITY, f64::INFINITY), hi: dvec3(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY) }
    }
    fn add(&mut self, p: Vec3d) {
        self.lo = self.lo.min(p);
        self.hi = self.hi.max(p);
    }
    fn join(&mut self, o: &Aabb) {
        self.lo = self.lo.min(o.lo);
        self.hi = self.hi.max(o.hi);
    }
    fn center(&self) -> Vec3d {
        (self.lo + self.hi) * 0.5
    }
    /// Distance squared from a point to the box (0 inside).
    fn dist2(&self, p: Vec3d) -> f64 {
        let d = dvec3((self.lo.x - p.x).max(0.0).max(p.x - self.hi.x), (self.lo.y - p.y).max(0.0).max(p.y - self.hi.y), (self.lo.z - p.z).max(0.0).max(p.z - self.hi.z));
        d.dot(d)
    }
    /// Entry distance of a ray, if it hits within `tmax`.
    fn ray(&self, o: Vec3d, inv: Vec3d, tmax: f64) -> Option<f64> {
        let mut t0 = 0.0f64;
        let mut t1 = tmax;
        for (lo, hi, oc, ic) in [(self.lo.x, self.hi.x, o.x, inv.x), (self.lo.y, self.hi.y, o.y, inv.y), (self.lo.z, self.hi.z, o.z, inv.z)] {
            let (a, b) = ((lo - oc) * ic, (hi - oc) * ic);
            let (a, b) = if a < b { (a, b) } else { (b, a) };
            t0 = t0.max(a);
            t1 = t1.min(b);
            if t0 > t1 {
                return None;
            }
        }
        Some(t0)
    }
}

struct Node {
    bb: Aabb,
    /// Leaf: range into `order`; inner: children.
    left: u32,
    right: u32,
    start: u32,
    count: u32,
}

/// A BVH over a mesh's triangles (it keeps its own copy of the mesh, so it
/// can be shared across threads).
pub struct Bvh {
    mesh: std::sync::Arc<TriMesh>,
    nodes: Vec<Node>,
    order: Vec<u32>,
}

const LEAF: usize = 4;

impl Bvh {
    pub fn build(mesh: &TriMesh) -> Bvh {
        let mesh = std::sync::Arc::new(mesh.clone());
        let n = mesh.triangles.len();
        let mut order: Vec<u32> = (0..n as u32).collect();
        let boxes: Vec<Aabb> = mesh.triangles.iter().map(|t| {
            let mut b = Aabb::empty();
            for &i in t {
                b.add(mesh.vertices[i as usize]);
            }
            b
        }).collect();
        let mut nodes = Vec::with_capacity(2 * n / LEAF + 1);
        if n > 0 {
            Self::split(&mut nodes, &mut order, &boxes, 0, n);
        }
        Bvh { mesh, nodes, order }
    }

    /// Whether `p` is inside the (closed) mesh: hit parity along three
    /// skewed rays, the majority wins (a ray grazing an edge counts wrong).
    pub fn inside(&self, p: Vec3d) -> bool {
        let dirs = [dvec3(0.5773, 0.5774, 0.5775), dvec3(-0.6012, 0.3421, 0.7221), dvec3(0.2113, -0.8832, 0.4188)];
        let odd = dirs.iter().filter(|d| self.hits(p, **d) % 2 == 1).count();
        odd >= 2
    }

    /// How many triangles a ray from `o` along `d` crosses.
    pub fn hits(&self, o: Vec3d, d: Vec3d) -> usize {
        if self.nodes.is_empty() {
            return 0;
        }
        let inv = dvec3(1.0 / d.x, 1.0 / d.y, 1.0 / d.z);
        let mut n = 0;
        let mut stack = vec![0u32];
        while let Some(ni) = stack.pop() {
            let node = &self.nodes[ni as usize];
            if node.bb.ray(o, inv, f64::INFINITY).is_none() {
                continue;
            }
            if node.count > 0 {
                for &t in &self.order[node.start as usize..(node.start + node.count) as usize] {
                    let (a, b, c) = self.tri(t);
                    if ray_tri(o, d, a, b, c).is_some_and(|h| h > 1e-12) {
                        n += 1;
                    }
                }
            } else {
                stack.push(node.left);
                stack.push(node.right);
            }
        }
        n
    }

    /// Signed distance to the mesh surface: negative inside. When the
    /// closest point lies inside a face, that face's normal gives the sign;
    /// near an edge or a corner (where normals disagree) ray parity does.
    pub fn signed_distance(&self, p: Vec3d) -> f64 {
        let Some((d, q, t)) = self.closest(p) else { return f64::INFINITY };
        let (a, b, c) = self.tri(t);
        let n = (b - a).cross(c - a);
        let area2 = n.length();
        if area2 > 1e-18 && d > 1e-12 {
            // Barycentric weights of q: all clear of zero means the face interior.
            let wa = (b - q).cross(c - q).length() / area2;
            let wb = (c - q).cross(a - q).length() / area2;
            let wc = (a - q).cross(b - q).length() / area2;
            if wa.min(wb).min(wc) > 1e-6 {
                return if (p - q).dot(n) < 0.0 { -d } else { d };
            }
        }
        if self.inside(p) { -d } else { d }
    }

    fn split(nodes: &mut Vec<Node>, order: &mut [u32], boxes: &[Aabb], start: usize, end: usize) -> u32 {
        let mut bb = Aabb::empty();
        for &t in &order[start..end] {
            bb.join(&boxes[t as usize]);
        }
        let id = nodes.len() as u32;
        nodes.push(Node { bb, left: 0, right: 0, start: start as u32, count: (end - start) as u32 });
        if end - start <= LEAF {
            return id;
        }
        let ext = bb.hi - bb.lo;
        let axis = if ext.x >= ext.y && ext.x >= ext.z { 0 } else if ext.y >= ext.z { 1 } else { 2 };
        let key = |t: &u32| boxes[*t as usize].center().component(axis);
        order[start..end].sort_by(|a, b| key(a).total_cmp(&key(b)));
        let mid = (start + end) / 2;
        let l = Self::split(nodes, order, boxes, start, mid);
        let r = Self::split(nodes, order, boxes, mid, end);
        let node = &mut nodes[id as usize];
        node.left = l;
        node.right = r;
        node.count = 0;
        id
    }

    fn tri(&self, t: u32) -> (Vec3d, Vec3d, Vec3d) {
        let tr = self.mesh.triangles[t as usize];
        (self.mesh.vertices[tr[0] as usize], self.mesh.vertices[tr[1] as usize], self.mesh.vertices[tr[2] as usize])
    }

    /// The first triangle a ray hits (Möller-Trumbore), skipping `skip`.
    pub fn ray(&self, o: Vec3d, d: Vec3d, tmax: f64, skip: Option<u32>) -> Option<(f64, u32)> {
        if self.nodes.is_empty() {
            return None;
        }
        let inv = dvec3(1.0 / d.x, 1.0 / d.y, 1.0 / d.z);
        let mut best: Option<(f64, u32)> = None;
        let mut stack = vec![0u32];
        while let Some(ni) = stack.pop() {
            let node = &self.nodes[ni as usize];
            let limit = best.map_or(tmax, |b| b.0);
            if node.bb.ray(o, inv, limit).is_none() {
                continue;
            }
            if node.count > 0 {
                for &t in &self.order[node.start as usize..(node.start + node.count) as usize] {
                    if Some(t) == skip {
                        continue;
                    }
                    let (a, b, c) = self.tri(t);
                    if let Some(h) = ray_tri(o, d, a, b, c) {
                        if h > 1e-9 && h < best.map_or(tmax, |b| b.0) {
                            best = Some((h, t));
                        }
                    }
                }
            } else {
                stack.push(node.left);
                stack.push(node.right);
            }
        }
        best
    }

    /// The closest point of the mesh to `p`: (distance, point, triangle).
    pub fn closest(&self, p: Vec3d) -> Option<(f64, Vec3d, u32)> {
        if self.nodes.is_empty() {
            return None;
        }
        let mut best: Option<(f64, Vec3d, u32)> = None;
        let mut stack = vec![0u32];
        while let Some(ni) = stack.pop() {
            let node = &self.nodes[ni as usize];
            let bd = best.map_or(f64::INFINITY, |b| b.0 * b.0);
            if node.bb.dist2(p) > bd {
                continue;
            }
            if node.count > 0 {
                for &t in &self.order[node.start as usize..(node.start + node.count) as usize] {
                    let (a, b, c) = self.tri(t);
                    let q = closest_on_tri(p, a, b, c);
                    let d = q.distance(p);
                    if best.map_or(true, |b| d < b.0) {
                        best = Some((d, q, t));
                    }
                }
            } else {
                // Nearer child first.
                let (l, r) = (node.left, node.right);
                let (dl, dr) = (self.nodes[l as usize].bb.dist2(p), self.nodes[r as usize].bb.dist2(p));
                if dl < dr {
                    stack.push(r);
                    stack.push(l);
                } else {
                    stack.push(l);
                    stack.push(r);
                }
            }
        }
        best
    }
}

fn ray_tri(o: Vec3d, d: Vec3d, a: Vec3d, b: Vec3d, c: Vec3d) -> Option<f64> {
    let e1 = b - a;
    let e2 = c - a;
    let p = d.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-14 {
        return None;
    }
    let inv = 1.0 / det;
    let s = o - a;
    let u = s.dot(p) * inv;
    if !(-1e-9..=1.0 + 1e-9).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = d.dot(q) * inv;
    if v < -1e-9 || u + v > 1.0 + 1e-9 {
        return None;
    }
    Some(e2.dot(q) * inv)
}

/// The closest point of triangle abc to p (Ericson).
pub fn closest_on_tri(p: Vec3d, a: Vec3d, b: Vec3d, c: Vec3d) -> Vec3d {
    let ab = b - a;
    let ac = c - a;
    let ap = p - a;
    let d1 = ab.dot(ap);
    let d2 = ac.dot(ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = p - b;
    let d3 = ab.dot(bp);
    let d4 = ac.dot(bp);
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return a + ab * (d1 / (d1 - d3));
    }
    let cp = p - c;
    let d5 = ab.dot(cp);
    let d6 = ac.dot(cp);
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return a + ac * (d2 / (d2 - d6));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        return b + (c - b) * ((d4 - d3) / ((d4 - d3) + (d5 - d6)));
    }
    let denom = 1.0 / (va + vb + vc);
    a + ab * (vb * denom) + ac * (vc * denom)
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
        if let Some(t) = thickness_at(&bvh, at, n, Some(i as u32)) {
            out.push(ThicknessSample { at, normal: n, thickness: t, area: area(i) });
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
