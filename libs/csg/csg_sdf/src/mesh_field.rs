//! A triangle mesh as a signed distance field: the exact unsigned distance to
//! its triangles (closest point by Voronoi region, in f32, one fixed formula a
//! GPU kernel can copy line by line) and the sign from the generalized winding
//! number (inside where it is over one half), which stays right for meshes with
//! small holes or overlaps, as generated meshes have.
//!
//! A bounding volume hierarchy over the triangles answers the closest triangle,
//! and the winding number far from a cluster takes the cluster's dipole (its
//! area-weighted normal at its centroid) instead of every triangle; near the
//! point every triangle counts exactly.

use crate::sdf::Sdf3;
use makepad_csg_math::Vec3d;

pub type V3 = [f32; 3];

/// A welded, indexed triangle set.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MeshData {
    pub positions: Vec<V3>,
    pub triangles: Vec<[u32; 3]>,
}

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn add_scaled(a: V3, b: V3, k: f32) -> V3 {
    [a[0] + b[0] * k, a[1] + b[1] * k, a[2] + b[2] * k]
}
fn len(a: V3) -> f32 {
    dot(a, a).sqrt()
}

/// The point of triangle `abc` closest to `p`, by its Voronoi regions in this order:
/// vertex a, vertex b, edge ab, vertex c, edge ac, edge bc, the face.
pub fn closest_on_tri(p: V3, a: V3, b: V3, c: V3) -> V3 {
    let ab = sub(b, a);
    let ac = sub(c, a);
    let ap = sub(p, a);
    let d1 = dot(ab, ap);
    let d2 = dot(ac, ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = sub(p, b);
    let d3 = dot(ab, bp);
    let d4 = dot(ac, bp);
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        return add_scaled(a, ab, v);
    }
    let cp = sub(p, c);
    let d5 = dot(ab, cp);
    let d6 = dot(ac, cp);
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        return add_scaled(a, ac, w);
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return add_scaled(b, sub(c, b), w);
    }
    let denom = 1.0 / (va + vb + vc);
    let v = vb * denom;
    let w = vc * denom;
    add_scaled(add_scaled(a, ab, v), ac, w)
}

/// [`closest_on_tri`] in f64: the CPU mesher's distances (an f32 distance's finite-
/// difference normals are too noisy to mesh from).
mod f64s {
    pub type D3 = [f64; 3];
    pub fn sub(a: D3, b: D3) -> D3 {
        [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
    }
    pub fn dot(a: D3, b: D3) -> f64 {
        a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
    }
    pub fn add_scaled(a: D3, b: D3, k: f64) -> D3 {
        [a[0] + b[0] * k, a[1] + b[1] * k, a[2] + b[2] * k]
    }
    pub fn len(a: D3) -> f64 {
        dot(a, a).sqrt()
    }

    /// The point of triangle `abc` closest to `p`, by its Voronoi regions in this order:
    /// vertex a, vertex b, edge ab, vertex c, edge ac, edge bc, the face.
    pub fn closest_on_tri64(p: D3, a: D3, b: D3, c: D3) -> D3 {
        let ab = sub(b, a);
        let ac = sub(c, a);
        let ap = sub(p, a);
        let d1 = dot(ab, ap);
        let d2 = dot(ac, ap);
        if d1 <= 0.0 && d2 <= 0.0 {
            return a;
        }
        let bp = sub(p, b);
        let d3 = dot(ab, bp);
        let d4 = dot(ac, bp);
        if d3 >= 0.0 && d4 <= d3 {
            return b;
        }
        let vc = d1 * d4 - d3 * d2;
        if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
            let v = d1 / (d1 - d3);
            return add_scaled(a, ab, v);
        }
        let cp = sub(p, c);
        let d5 = dot(ab, cp);
        let d6 = dot(ac, cp);
        if d6 >= 0.0 && d5 <= d6 {
            return c;
        }
        let vb = d5 * d2 - d1 * d6;
        if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
            let w = d2 / (d2 - d6);
            return add_scaled(a, ac, w);
        }
        let va = d3 * d6 - d5 * d4;
        if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
            let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
            return add_scaled(b, sub(c, b), w);
        }
        let denom = 1.0 / (va + vb + vc);
        let v = vb * denom;
        let w = vc * denom;
        add_scaled(add_scaled(a, ab, v), ac, w)
    }


}

/// The distance from `p` to triangle `abc`.
pub fn tri_dist(p: V3, a: V3, b: V3, c: V3) -> f32 {
    len(sub(p, closest_on_tri(p, a, b, c)))
}

/// The solid angle triangle `abc` spans seen from `p`, over 4 pi (Van Oosterom and
/// Strackee): the triangle's share of the winding number at `p`.
pub fn tri_winding(p: V3, a: V3, b: V3, c: V3) -> f32 {
    let (x, y, z) = (sub(a, p), sub(b, p), sub(c, p));
    let (lx, ly, lz) = (len(x), len(y), len(z));
    let det = dot(x, cross(y, z));
    let div = lx * ly * lz + dot(x, y) * lz + dot(x, z) * ly + dot(y, z) * lx;
    det.atan2(div) * (1.0 / (2.0 * std::f32::consts::PI))
}

#[derive(Clone, Debug)]
struct Node {
    min: V3,
    max: V3,
    /// Children (an inner node) or the range of `order` (a leaf).
    left: u32,
    right: u32,
    leaf: bool,
    /// The cluster's area-weighted normal (half the sum of the cross products) and centroid.
    normal: V3,
    center: V3,
    radius: f32,
}

/// A mesh prepared for distance queries.
#[derive(Clone, Debug)]
pub struct MeshField {
    pub data: MeshData,
    nodes: Vec<Node>,
    order: Vec<u32>,
    /// Every edge shared by exactly two triangles, in opposite directions.
    pub watertight: bool,
    pub min: V3,
    pub max: V3,
    key: u64,
}

const LEAF: usize = 8;

impl MeshField {
    /// Prepare `data` (welded positions, indexed triangles; degenerate ones dropped).
    pub fn new(mut data: MeshData) -> MeshField {
        data.triangles.retain(|t| t[0] != t[1] && t[1] != t[2] && t[0] != t[2] && (t[0].max(t[1]).max(t[2]) as usize) < data.positions.len());
        // The key: FNV-1a over the positions' and indices' bits.
        let mut key = 0xcbf2_9ce4_8422_2325u64;
        let mut eat = |w: u32| {
            for b in w.to_le_bytes() {
                key = (key ^ b as u64).wrapping_mul(0x100_0000_01b3);
            }
        };
        for p in &data.positions {
            p.iter().for_each(|x| eat(x.to_bits()));
        }
        for t in &data.triangles {
            t.iter().for_each(|i| eat(*i));
        }
        let mut edges = std::collections::HashMap::<(u32, u32), i32>::new();
        for t in &data.triangles {
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                *edges.entry((a.min(b), a.max(b))).or_default() += if a < b { 1 } else { -1 };
            }
        }
        let watertight = !data.triangles.is_empty() && edges.values().all(|v| *v == 0);
        let mut field = MeshField { nodes: Vec::new(), order: (0..data.triangles.len() as u32).collect(), watertight, min: [f32::MAX; 3], max: [f32::MIN; 3], key, data };
        for p in &field.data.positions {
            for k in 0..3 {
                field.min[k] = field.min[k].min(p[k]);
                field.max[k] = field.max[k].max(p[k]);
            }
        }
        if !field.order.is_empty() {
            let n = field.order.len();
            field.build(0, n);
        }
        field
    }

    /// The mesh's identity (its geometry's bits).
    pub fn key(&self) -> u64 {
        self.key
    }

    fn tri(&self, t: u32) -> (V3, V3, V3) {
        let i = self.data.triangles[t as usize];
        (self.data.positions[i[0] as usize], self.data.positions[i[1] as usize], self.data.positions[i[2] as usize])
    }

    fn build(&mut self, from: usize, to: usize) -> u32 {
        let (mut min, mut max) = ([f32::MAX; 3], [f32::MIN; 3]);
        let (mut normal, mut center, mut area) = ([0.0f32; 3], [0.0f32; 3], 0.0f32);
        for &t in &self.order[from..to] {
            let (a, b, c) = self.tri(t);
            for p in [a, b, c] {
                for k in 0..3 {
                    min[k] = min[k].min(p[k]);
                    max[k] = max[k].max(p[k]);
                }
            }
            let n = cross(sub(b, a), sub(c, a));
            normal = add_scaled(normal, n, 0.5);
            let w = len(n) * 0.5;
            area += w;
            center = add_scaled(center, [(a[0] + b[0] + c[0]) / 3.0, (a[1] + b[1] + c[1]) / 3.0, (a[2] + b[2] + c[2]) / 3.0], w);
        }
        if area > 0.0 {
            center = [center[0] / area, center[1] / area, center[2] / area];
        } else {
            center = [(min[0] + max[0]) * 0.5, (min[1] + max[1]) * 0.5, (min[2] + max[2]) * 0.5];
        }
        let radius = len(sub(max, min)) * 0.5 + len(sub(center, [(min[0] + max[0]) * 0.5, (min[1] + max[1]) * 0.5, (min[2] + max[2]) * 0.5]));
        let index = self.nodes.len() as u32;
        self.nodes.push(Node { min, max, left: from as u32, right: to as u32, leaf: true, normal, center, radius });
        if to - from <= LEAF {
            return index;
        }
        // Split at the median along the longest axis of the triangles' centres.
        let axis = (0..3).max_by(|&x, &y| (max[x] - min[x]).total_cmp(&(max[y] - min[y]))).unwrap();
        let mid = (from + to) / 2;
        let centre = |t: u32, s: &MeshField| {
            let (a, b, c) = s.tri(t);
            a[axis] + b[axis] + c[axis]
        };
        let mut part: Vec<(f32, u32)> = self.order[from..to].iter().map(|&t| (centre(t, self), t)).collect();
        part.select_nth_unstable_by(mid - from, |x, y| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1)));
        for (k, (_, t)) in part.into_iter().enumerate() {
            self.order[from + k] = t;
        }
        let l = self.build(from, mid);
        let r = self.build(mid, to);
        let n = &mut self.nodes[index as usize];
        n.leaf = false;
        n.left = l;
        n.right = r;
        index
    }

    fn box_dist2(n: &Node, p: V3) -> f32 {
        let mut d = 0.0;
        for k in 0..3 {
            let e = (n.min[k] - p[k]).max(0.0).max(p[k] - n.max[k]);
            d += e * e;
        }
        d
    }

    /// The exact distance from `p` to the nearest triangle.
    pub fn unsigned(&self, p: V3) -> f32 {
        self.closest(p).0
    }

    /// The distance to the nearest triangle, the nearest point on it and the triangle.
    pub fn closest(&self, p: V3) -> (f32, V3, u32) {
        if self.nodes.is_empty() {
            return (f32::INFINITY, p, 0);
        }
        let mut best = f32::INFINITY;
        let mut at = p;
        let mut tri = 0u32;
        let mut stack = vec![0u32];
        while let Some(i) = stack.pop() {
            let n = &self.nodes[i as usize];
            if Self::box_dist2(n, p) >= best * best {
                continue;
            }
            if n.leaf {
                for &t in &self.order[n.left as usize..n.right as usize] {
                    let (a, b, c) = self.tri(t);
                    let q = closest_on_tri(p, a, b, c);
                    let d = len(sub(p, q));
                    if d < best {
                        best = d;
                        at = q;
                        tri = t;
                    }
                }
            } else {
                // Nearer child first.
                let (l, r) = (&self.nodes[n.left as usize], &self.nodes[n.right as usize]);
                if Self::box_dist2(l, p) < Self::box_dist2(r, p) {
                    stack.push(n.right);
                    stack.push(n.left);
                } else {
                    stack.push(n.left);
                    stack.push(n.right);
                }
            }
        }
        (best, at, tri)
    }

    /// The generalized winding number at `p`: about 1 inside, 0 outside. A cluster seen
    /// from farther than twice its radius counts as its dipole.
    pub fn winding(&self, p: V3) -> f32 {
        if self.nodes.is_empty() {
            return 0.0;
        }
        let mut w = 0.0f32;
        let mut stack = vec![0u32];
        while let Some(i) = stack.pop() {
            let n = &self.nodes[i as usize];
            let d = sub(n.center, p);
            let dl = len(d);
            if dl > 2.0 * n.radius && dl > 0.0 {
                w += dot(n.normal, d) / (4.0 * std::f32::consts::PI * dl * dl * dl);
                continue;
            }
            if n.leaf {
                for &t in &self.order[n.left as usize..n.right as usize] {
                    let (a, b, c) = self.tri(t);
                    w += tri_winding(p, a, b, c);
                }
            } else {
                stack.push(n.left);
                stack.push(n.right);
            }
        }
        w
    }

    /// The signed distance: negative inside.
    pub fn signed(&self, p: V3) -> f32 {
        let d = self.unsigned(p);
        if self.winding(p) > 0.5 { -d } else { d }
    }
}

impl MeshField {
    /// The signed distance in f64 (the CPU mesher's), the sign from the winding number.
    pub fn signed64(&self, p: Vec3d) -> f64 {
        let d = self.unsigned64([p.x, p.y, p.z]);
        if self.winding([p.x as f32, p.y as f32, p.z as f32]) > 0.5 { -d } else { d }
    }

    fn unsigned64(&self, p: f64s::D3) -> f64 {
        if self.nodes.is_empty() {
            return f64::INFINITY;
        }
        let pf = [p[0] as f32, p[1] as f32, p[2] as f32];
        let mut best = f64::INFINITY;
        let mut stack = vec![0u32];
        while let Some(i) = stack.pop() {
            let n = &self.nodes[i as usize];
            if (Self::box_dist2(n, pf) as f64) >= best * best * 1.0001 + 1e-9 {
                continue;
            }
            if n.leaf {
                for &t in &self.order[n.left as usize..n.right as usize] {
                    let (a, b, c) = self.tri(t);
                    let w = |v: V3| [v[0] as f64, v[1] as f64, v[2] as f64];
                    let q = f64s::closest_on_tri64(p, w(a), w(b), w(c));
                    best = best.min(f64s::len(f64s::sub(p, q)));
                }
            } else {
                stack.push(n.left);
                stack.push(n.right);
            }
        }
        best
    }
}

impl Sdf3 for MeshField {
    fn distance(&self, p: Vec3d) -> f64 {
        self.signed64(p)
    }

    /// The gradient: away from the nearest point (into the solid's outside), or the
    /// nearest triangle's normal on the surface. Finite differences of an f32 distance
    /// are too noisy for the mesher.
    fn normal(&self, p: Vec3d) -> Vec3d {
        let q = [p.x as f32, p.y as f32, p.z as f32];
        let (d, at, t) = self.closest(q);
        let (a, b, c) = self.tri(t);
        let face = cross(sub(b, a), sub(c, a));
        let dir = if d > 1e-5 { sub(q, at) } else { face };
        let sign = if d > 1e-5 && self.winding(q) > 0.5 { -1.0 } else { 1.0 };
        let l = len(dir).max(1e-30);
        Vec3d::new((dir[0] / l * sign) as f64, (dir[1] / l * sign) as f64, (dir[2] / l * sign) as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cube(open_top: bool) -> MeshData {
        let positions: Vec<V3> = (0..8).map(|i| [if i & 1 == 0 { -1.0 } else { 1.0 }, if i & 2 == 0 { -1.0 } else { 1.0 }, if i & 4 == 0 { -1.0 } else { 1.0 }]).collect();
        let quads = [[0, 2, 3, 1], [4, 5, 7, 6], [0, 1, 5, 4], [2, 6, 7, 3], [0, 4, 6, 2], [1, 3, 7, 5]];
        let mut triangles = Vec::new();
        for (k, q) in quads.iter().enumerate() {
            // Quad 3 is the +Y face.
            if open_top && k == 3 {
                continue;
            }
            triangles.push([q[0], q[1], q[2]]);
            triangles.push([q[0], q[2], q[3]]);
        }
        MeshData { positions, triangles }
    }

    #[test]
    fn a_watertight_cube_is_an_exact_field() {
        let f = MeshField::new(cube(false));
        assert!(f.watertight);
        assert!((f.signed([0.0, 0.0, 0.0]) + 1.0).abs() < 1e-6);
        assert!((f.signed([3.0, 0.0, 0.0]) - 2.0).abs() < 1e-6);
        assert!((f.signed([2.0, 2.0, 0.0]) - 2f32.sqrt()).abs() < 1e-5);
        assert!((f.winding([0.2, -0.3, 0.1]) - 1.0).abs() < 1e-3);
        assert!(f.winding([5.0, 5.0, 5.0]).abs() < 1e-3);
    }

    #[test]
    fn an_open_lid_cup_keeps_its_sign_below_the_rim() {
        let f = MeshField::new(cube(true));
        assert!(!f.watertight);
        // Deep inside the cup the winding number is still over a half; above the open top it is not.
        assert!(f.signed([0.0, -0.5, 0.0]) < 0.0);
        assert!(f.signed([0.0, 3.0, 0.0]) > 0.0);
        assert!(f.signed([3.0, 0.0, 0.0]) > 0.0);
    }

    #[test]
    fn overlapping_shells_read_as_their_union() {
        // Two cubes, the second moved by 1 along X, as one self-intersecting mesh.
        let mut m = cube(false);
        let n = m.positions.len() as u32;
        let moved: Vec<V3> = m.positions.iter().map(|p| [p[0] + 1.0, p[1], p[2]]).collect();
        m.positions.extend(moved);
        let more: Vec<[u32; 3]> = m.triangles.iter().map(|t| [t[0] + n, t[1] + n, t[2] + n]).collect();
        m.triangles.extend(more);
        let f = MeshField::new(m);
        // Inside both (w = 2), inside one (w = 1), outside.
        assert!(f.signed([0.5, 0.0, 0.0]) < 0.0);
        assert!(f.signed([1.8, 0.0, 0.0]) < 0.0);
        assert!(f.signed([-0.8, 0.0, 0.0]) < 0.0);
        assert!(f.signed([2.5, 0.0, 0.0]) > 0.0);
    }

    #[test]
    fn the_closest_point_regions_match_brute_force() {
        let (a, b, c) = ([0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 2.0, 0.0]);
        for p in [[-1.0, -1.0, 0.5], [3.0, -1.0, 0.0], [1.0, -1.0, 0.0], [-1.0, 3.0, 0.0], [-1.0, 1.0, 0.0], [2.0, 2.0, 0.0], [0.5, 0.5, 1.0]] {
            let mut best = f32::MAX;
            for i in 0..=200 {
                for j in 0..=(200 - i) {
                    let (u, v) = (i as f32 / 200.0, j as f32 / 200.0);
                    let q = [a[0] + (b[0] - a[0]) * u + (c[0] - a[0]) * v, a[1] + (b[1] - a[1]) * u + (c[1] - a[1]) * v, 0.0];
                    best = best.min(len(sub(p, q)));
                }
            }
            assert!((tri_dist(p, a, b, c) - best).abs() < 0.02, "{p:?}: {} vs {best}", tri_dist(p, a, b, c));
        }
    }
}
