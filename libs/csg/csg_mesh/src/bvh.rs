//! Triangle queries for closest points, rays and signed distances.

use crate::TriMesh;
use makepad_csg_math::{dvec3, Vec3d};

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closest_matches_brute_force_and_keeps_ties_stable() {
        let mesh = crate::make_unit_cube();
        let bvh = Bvh::build(&mesh);
        for p in [dvec3(0.0, 0.0, 0.0), dvec3(0.2, 0.1, 0.5), dvec3(0.8, 0.2, 0.1), dvec3(-0.8, 0.7, -0.9), dvec3(0.5, 0.5, 0.5)] {
            let brute = (0..mesh.triangle_count()).map(|ti| {
                let (a, b, c) = mesh.triangle_vertices(ti);
                closest_on_tri(p, a, b, c).distance(p)
            }).fold(f64::INFINITY, f64::min);
            let hit = bvh.closest(p).expect("box closest: a nonempty mesh has a closest point");
            assert!((hit.0 - brute).abs() < 1e-12, "box closest at {p:?}: distance agrees with brute force");
            let (a, b, c) = mesh.triangle_vertices(hit.2 as usize);
            assert_eq!(hit.1, closest_on_tri(p, a, b, c), "box closest at {p:?}: point belongs to the returned triangle");
            assert_eq!(Bvh::build(&mesh).closest(p), Some(hit), "box closest at {p:?}: rebuilding preserves ties");
        }
        let o = dvec3(0.1, 0.2, 2.0);
        let d = dvec3(0.0, 0.0, -1.0);
        let (distance, ti) = bvh.ray(o, d, 10.0, None).expect("box ray: first face is hit");
        assert!((distance - 1.5).abs() < 1e-12, "box ray: first hit distance survives the move");
        assert!((bvh.ray(o, d, 10.0, Some(ti)).unwrap().0 - 2.5).abs() < 1e-12, "box ray: skipping the first triangle reaches the back face");
        assert_eq!(bvh.hits(o, d), 2, "box ray: both faces are counted");
        assert!(bvh.signed_distance(dvec3(0.1, 0.2, 0.0)) < 0.0, "box signed distance: an interior point is negative");
        assert!(Bvh::build(&TriMesh::new()).closest(o).is_none(), "empty closest: no triangle is returned");
    }
}

