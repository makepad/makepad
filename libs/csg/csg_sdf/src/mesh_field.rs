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

use crate::sdf::{Ball, Hints, Sdf3};
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

/// Portable atan2, copied in mod.sdf.Eval.mesh_atan2. Only unfused f32
/// arithmetic and comparisons: first reduce to [0, 1], then to [-tan(pi/8),
/// tan(pi/8)]. The degree-17 alternating series has remainder < 3e-9 there;
/// f32 rounding and quadrant reconstruction keep the angular error below 1e-6.
/// Reciprocal comparisons distinguish signed zeros without platform intrinsics.
pub fn mesh_atan2(y: f32, x: f32) -> f32 {
    let yn = y < 0.0 || (y == 0.0 && 1.0 / y < 0.0);
    let xn = x < 0.0 || (x == 0.0 && 1.0 / x < 0.0);
    if y == 0.0 {
        if xn { return if yn { -3.1415927410125732 } else { 3.1415927410125732 }; }
        return y;
    }
    if x == 0.0 { return if yn { -1.5707963705062866 } else { 1.5707963705062866 }; }
    let ax = if xn { 0.0 - x } else { x };
    let ay = if yn { 0.0 - y } else { y };
    let swap = ay > ax;
    let mut r = if ax == ay { 1.0 } else if swap { ax / ay } else { ay / ax };
    let mut base = 0.0;
    if r > 0.4142135679721832 {
        r = (r - 1.0) / (r + 1.0);
        base = 0.7853981852531433;
    }
    let z = r * r;
    let p = -0.3333333333333333 + z * (0.2 + z * (-0.14285714285714285 + z * (0.1111111111111111 + z * (-0.09090909090909091 + z * (0.07692307692307693 + z * (-0.06666666666666667 + z * 0.058823529411764705))))));
    let mut a = base + (r + (r * z) * p);
    if swap { a = 1.5707963705062866 - a; }
    if xn { a = 3.1415927410125732 - a; }
    if yn { a * -1.0 } else { a }
}

/// The solid angle triangle `abc` spans seen from `p`, over 4 pi (Van Oosterom and
/// Strackee): the triangle's share of the winding number at `p`.
pub fn tri_winding(p: V3, a: V3, b: V3, c: V3) -> f32 {
    let (x, y, z) = (sub(a, p), sub(b, p), sub(c, p));
    let (lx, ly, lz) = (len(x), len(y), len(z));
    let det = dot(x, cross(y, z));
    let div = lx * ly * lz + dot(x, y) * lz + dot(x, z) * ly + dot(y, z) * lx;
    mesh_atan2(det, div) * (1.0 / (2.0 * std::f32::consts::PI))
}

/// Whether `p` lies on triangle `abc` as [`tri_winding`] sees it, with its operations: exactly in
/// its plane (the determinant zero) and not outside it (the denominator not positive). There
/// the triangle's share is a half, or a quarter on an edge, whose sign only a zero's sign gives.
fn on_tri(p: V3, a: V3, b: V3, c: V3) -> bool {
    let (x, y, z) = (sub(a, p), sub(b, p), sub(c, p));
    let (lx, ly, lz) = (len(x), len(y), len(z));
    let det = dot(x, cross(y, z));
    let div = lx * ly * lz + dot(x, y) * lz + dot(x, z) * ly + dot(y, z) * lx;
    det == 0.0 && div <= 0.0
}

/// [`mesh_atan2`] with its branches taken as selects: every path computed, the same
/// operations on the same values, so the same bits (a zero's sign read from its bit, which is
/// what the reciprocal's sign tells). Branch-free, so that lanes can run it side by side.
#[inline(always)]
fn mesh_atan2_select(y: f32, x: f32) -> f32 {
    let yn = (y < 0.0) | ((y == 0.0) & y.is_sign_negative());
    let xn = (x < 0.0) | ((x == 0.0) & x.is_sign_negative());
    let ax = if xn { 0.0 - x } else { x };
    let ay = if yn { 0.0 - y } else { y };
    let swap = ay > ax;
    let q = if swap { ax } else { ay } / if swap { ay } else { ax };
    let r = if ax == ay { 1.0 } else { q };
    let big = r > 0.4142135679721832;
    let reduced = (r - 1.0) / (r + 1.0);
    let r = if big { reduced } else { r };
    let base = if big { 0.7853981852531433 } else { 0.0 };
    let z = r * r;
    let p = -0.3333333333333333 + z * (0.2 + z * (-0.14285714285714285 + z * (0.1111111111111111 + z * (-0.09090909090909091 + z * (0.07692307692307693 + z * (-0.06666666666666667 + z * 0.058823529411764705))))));
    let a = base + (r + (r * z) * p);
    let a = if swap { 1.5707963705062866 - a } else { a };
    let a = if xn { 3.1415927410125732 - a } else { a };
    let a = if yn { a * -1.0 } else { a };
    let on_x = if xn { if yn { -3.1415927410125732 } else { 3.1415927410125732 } } else { y };
    let on_y = if yn { -1.5707963705062866 } else { 1.5707963705062866 };
    if y == 0.0 { on_x } else if x == 0.0 { on_y } else { a }
}

/// A leaf's triangles across eight lanes: coordinate `k` of corner `v` of the leaf's triangle
/// `l` at `[3 * v + k][l]` (lanes past the leaf's triangles hold zeros).
#[derive(Clone, Debug)]
struct Lanes([[f32; LEAF]; 9]);

/// [`tri_winding`] of a leaf's triangles, lane `l` its triangle `l`: in every lane the same
/// operations in the same order (so the same bits as one triangle at a time).
fn tri_winding_lanes(p: V3, t: &Lanes) -> [f32; LEAF] {
    let t = &t.0;
    let mut out = [0.0f32; LEAF];
    for l in 0..LEAF {
        let x = [t[0][l] - p[0], t[1][l] - p[1], t[2][l] - p[2]];
        let y = [t[3][l] - p[0], t[4][l] - p[1], t[5][l] - p[2]];
        let z = [t[6][l] - p[0], t[7][l] - p[1], t[8][l] - p[2]];
        let (lx, ly, lz) = (len(x), len(y), len(z));
        let det = dot(x, cross(y, z));
        let div = lx * ly * lz + dot(x, y) * lz + dot(x, z) * ly + dot(y, z) * lx;
        out[l] = mesh_atan2_select(det, div) * (1.0 / (2.0 * std::f32::consts::PI));
    }
    out
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
    /// A leaf's triangles in `MeshField::lanes`.
    lanes: u32,
}

/// A mesh prepared for distance queries.
#[derive(Clone, Debug)]
pub struct MeshField {
    pub data: MeshData,
    nodes: Vec<Node>,
    order: Vec<u32>,
    /// The leaves' triangles, for the winding number eight at a time.
    lanes: Vec<Lanes>,
    /// Every edge shared by exactly two triangles, in opposite directions.
    pub watertight: bool,
    pub min: V3,
    pub max: V3,
    key: u64,
}

const LEAF: usize = 8;

// The median tree has at most 32 levels for its u32 triangle addressing
// (see query_words). A depth-first traversal needs one pending node per level.
struct NodeStack {
    nodes: [u32; 32],
    len: usize,
}

impl NodeStack {
    fn root() -> Self { Self { nodes: [0; 32], len: 1 } }
    fn pop(&mut self) -> Option<u32> {
        if self.len == 0 { return None; }
        self.len -= 1;
        Some(self.nodes[self.len])
    }
    fn push(&mut self, node: u32) {
        self.nodes[self.len] = node;
        self.len += 1;
    }
}

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
        let mut field = MeshField { nodes: Vec::new(), order: (0..data.triangles.len() as u32).collect(), lanes: Vec::new(), watertight, min: [f32::MAX; 3], max: [f32::MIN; 3], key, data };
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
        for i in 0..field.nodes.len() {
            let n = &field.nodes[i];
            if n.leaf {
                let mut lanes = Lanes([[0.0; LEAF]; 9]);
                for (l, &t) in field.order[n.left as usize..n.right as usize].iter().enumerate() {
                    let (a, b, c) = field.tri(t);
                    for k in 0..3 {
                        (lanes.0[k][l], lanes.0[3 + k][l], lanes.0[6 + k][l]) = (a[k], b[k], c[k]);
                    }
                }
                field.nodes[i].lanes = field.lanes.len() as u32;
                field.lanes.push(lanes);
            }
        }
        field
    }

    /// The mesh's identity (its geometry's bits).
    pub fn key(&self) -> u64 {
        self.key
    }

    /// Immutable GPU query data, in vec4u records. Header: node count and
    /// offsets of positions, triangles and order. A node occupies four records:
    /// min/left, max/right, normal/leaf, center/radius. Floats retain their bits.
    /// Median splitting with eight triangles per leaf needs at most 30 pending
    /// nodes for a u32 triangle count; consumers provide 32 stack entries.
    pub fn query_words(&self) -> Vec<[u32; 4]> {
        fn depth(nodes: &[Node], i: usize) -> usize {
            let n = &nodes[i];
            if n.leaf { 1 } else { 1 + depth(nodes, n.left as usize).max(depth(nodes, n.right as usize)) }
        }
        assert!(self.nodes.is_empty() || depth(&self.nodes, 0) <= 32, "mesh query stack exceeds 32 entries");
        let positions = 1 + self.nodes.len() * 4;
        let triangles = positions + self.data.positions.len();
        let order = triangles + self.data.triangles.len();
        let count = order + self.order.len();
        assert!(u32::try_from(count).is_ok(), "mesh query buffer exceeds u32 addressing");
        let mut words = Vec::with_capacity(count);
        words.push([self.nodes.len() as u32, positions as u32, triangles as u32, order as u32]);
        let record = |v: V3, w| [v[0].to_bits(), v[1].to_bits(), v[2].to_bits(), w];
        for n in &self.nodes {
            words.push(record(n.min, n.left));
            words.push(record(n.max, n.right));
            words.push(record(n.normal, n.leaf as u32));
            words.push(record(n.center, n.radius.to_bits()));
        }
        words.extend(self.data.positions.iter().map(|p| record(*p, 0)));
        words.extend(self.data.triangles.iter().map(|t| [t[0], t[1], t[2], 0]));
        words.extend(self.order.iter().map(|i| [*i, 0, 0, 0]));
        words
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
        self.nodes.push(Node { min, max, left: from as u32, right: to as u32, leaf: true, normal, center, radius, lanes: 0 });
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
        let mut stack = NodeStack::root();
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
        let mut stack = NodeStack::root();
        while let Some(i) = stack.pop() {
            let n = &self.nodes[i as usize];
            let d = sub(n.center, p);
            let dl = len(d);
            if dl > 2.0 * n.radius && dl > 0.0 {
                w += dot(n.normal, d) / (4.0 * std::f32::consts::PI * dl * dl * dl);
                continue;
            }
            if n.leaf {
                // the leaf's shares summed in its order, as one triangle at a time
                let shares = tri_winding_lanes(p, &self.lanes[n.lanes as usize]);
                for &s in &shares[..(n.right - n.left) as usize] {
                    w += s;
                }
            } else {
                stack.push(n.left);
                stack.push(n.right);
            }
        }
        w
    }

    /// The nearest triangle's distance when it is under `bound` (else `bound`).
    fn unsigned_under(&self, p: V3, bound: f32) -> f32 {
        let mut best = bound;
        let mut stack = NodeStack::root();
        while let Some(i) = stack.pop() {
            let n = &self.nodes[i as usize];
            if Self::box_dist2(n, p) >= best * best {
                continue;
            }
            if n.leaf {
                for &t in &self.order[n.left as usize..n.right as usize] {
                    let (a, b, c) = self.tri(t);
                    best = best.min(len(sub(p, closest_on_tri(p, a, b, c))));
                }
            } else {
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
        best
    }

    /// [`MeshField::signed`] at many points, nearby points in a row (a brick's samples). Each
    /// point's distance is at most the previous one's plus the step between them, which bounds
    /// its search; a point inside the previous point's empty ball has its sign.
    pub fn signed_many(&self, ps: &[V3]) -> Vec<f32> {
        if self.nodes.is_empty() {
            return vec![f32::INFINITY; ps.len()];
        }
        let mut out = Vec::with_capacity(ps.len());
        let mut prev: Option<(V3, f32, bool)> = None;
        for &p in ps {
            let (d, inside) = match prev {
                Some((q, dq, inq)) => {
                    let step = len(sub(p, q));
                    // the slack keeps the bound's own triangle inside the search
                    let d = self.unsigned_under(p, (dq + step) * 1.0001 + 1e-6);
                    (d, if step < dq { inq } else { self.winding(p) > 0.5 })
                }
                None => (self.unsigned(p), self.winding(p) > 0.5),
            };
            prev = Some((p, d, inside));
            out.push(if inside { -d } else { d });
        }
        out
    }

    /// The signed distance: negative inside.
    pub fn signed(&self, p: V3) -> f32 {
        let d = self.unsigned(p);
        if self.winding(p) > 0.5 { -d } else { d }
    }
}

impl MeshField {
    /// The signed distance in f64 (the CPU mesher's): negative inside, where the f32 point lies
    /// on a face ([`MeshField::on_face`]) or its winding number is over a half.
    pub fn signed64(&self, p: Vec3d) -> f64 {
        let at = [p.x, p.y, p.z];
        let pf = [p.x as f32, p.y as f32, p.z as f32];
        let mut near = 0;
        let d = self.unsigned64(at, f64::NEG_INFINITY, f64::INFINITY, &mut near);
        // the surface is at least this far from the f32 point
        let clear = d - f64s::len(f64s::sub(at, [pf[0] as f64, pf[1] as f64, pf[2] as f64])) - BALL_ROUNDING;
        let inside = (clear <= self.on_reach() && self.on_face(pf, near)) || self.winding(pf) > 0.5;
        if inside { -d } else { d }
    }

    /// How far from a triangle a point can lie that [`on_tri`] still puts on it, many times over
    /// (mm): 1e-5 of the mesh's largest coordinate, where f32 rounding is under 1.2e-7 of it.
    fn on_reach(&self) -> f64 {
        let m = (0..3).map(|k| self.min[k].abs().max(self.max[k].abs())).fold(0.0f32, f32::max);
        1e-5 * (1.0 + m as f64)
    }

    /// Whether `p` lies on a face: on a triangle as [`on_tri`] tells, and within
    /// [`MeshField::on_reach`] of it. Such a point is inside by the closed-set rule (a solid
    /// holds its boundary), where the winding number would take its side from a zero's sign.
    /// `near` (a triangle plus one; zero: none) is tried first, then every triangle near enough.
    fn on_face(&self, p: V3, near: u32) -> bool {
        if self.nodes.is_empty() {
            return false;
        }
        let reach = self.on_reach();
        let w = |v: V3| [v[0] as f64, v[1] as f64, v[2] as f64];
        let on = |t: u32| {
            let (a, b, c) = self.tri(t);
            on_tri(p, a, b, c) && f64s::len(f64s::sub(w(p), f64s::closest_on_tri64(w(p), w(a), w(b), w(c)))) <= reach
        };
        if near.checked_sub(1).is_some_and(|t| (t as usize) < self.data.triangles.len() && on(t)) {
            return true;
        }
        // the boxes within reach, with room for their f32 rounding
        let r = (2.0 * reach) as f32;
        let mut stack = NodeStack::root();
        while let Some(i) = stack.pop() {
            let n = &self.nodes[i as usize];
            if Self::box_dist2(n, p) > r * r {
                continue;
            }
            if n.leaf {
                if self.order[n.left as usize..n.right as usize].iter().any(|&t| on(t)) {
                    return true;
                }
            } else {
                stack.push(n.left);
                stack.push(n.right);
            }
        }
        false
    }

    /// The distance from `p` to the nearest triangle, exact when under `cap` (else at least
    /// `cap`). Once a triangle within `enough` is found, that triangle's distance (then no more
    /// than `enough`, and not necessarily the nearest). `near`: a triangle plus one to start from
    /// (zero: none), left at the nearest one found.
    fn unsigned64(&self, p: f64s::D3, enough: f64, cap: f64, near: &mut u32) -> f64 {
        if self.nodes.is_empty() {
            return f64::INFINITY;
        }
        let pf = [p[0] as f32, p[1] as f32, p[2] as f32];
        let w = |v: V3| [v[0] as f64, v[1] as f64, v[2] as f64];
        // The boxes are measured in f32 from `pf`: a triangle's box lies no farther from it than
        // the triangle from `p` plus the way between the two points, and f32 rounding adds under
        // a millionth. A box beyond that from the best so far (or the cap) holds no nearer
        // triangle.
        let moved = f64s::len(f64s::sub(p, w(pf))) + 1e-9;
        let mut best = f64::INFINITY;
        // A triangle's distance bounds the nearest one's from the start.
        if let Some(t) = near.checked_sub(1).filter(|t| (*t as usize) < self.data.triangles.len()) {
            let (a, b, c) = self.tri(t);
            best = f64s::len(f64s::sub(p, f64s::closest_on_tri64(p, w(a), w(b), w(c))));
            if best <= enough {
                return best;
            }
        }
        let mut stack = NodeStack::root();
        while let Some(i) = stack.pop() {
            let n = &self.nodes[i as usize];
            let reach = best.min(cap) + moved;
            if (Self::box_dist2(n, pf) as f64) >= reach * reach * (1.0 + 1e-6) {
                continue;
            }
            if n.leaf {
                for &t in &self.order[n.left as usize..n.right as usize] {
                    let (a, b, c) = self.tri(t);
                    let q = f64s::closest_on_tri64(p, w(a), w(b), w(c));
                    let d = f64s::len(f64s::sub(p, q));
                    if d < best {
                        (best, *near) = (d, t + 1);
                    }
                }
                if best <= enough {
                    return best;
                }
            } else {
                // Nearer child first: its triangles shrink `best` before the farther box is tested.
                let (l, r) = (&self.nodes[n.left as usize], &self.nodes[n.right as usize]);
                if Self::box_dist2(l, pf) < Self::box_dist2(r, pf) {
                    stack.push(n.right);
                    stack.push(n.left);
                } else {
                    stack.push(n.left);
                    stack.push(n.right);
                }
            }
        }
        best
    }
}

/// What a ball's radius gives up for the search's rounding (mm).
const BALL_ROUNDING: f64 = 1e-9;

impl MeshField {
    /// Whether ball `b`'s sign is the sign at the f32 point `w` (in f64). The sign is taken at
    /// f32 points: with both `w` and the centre's f32 point in the ball, the segment between them
    /// crosses no surface; `w` deeper in it than [`MeshField::on_reach`] lies on no face.
    fn holds(&self, b: &Ball, w: f64s::D3) -> bool {
        let c = [b.center.x, b.center.y, b.center.z];
        let f = [c[0] as f32 as f64, c[1] as f32 as f64, c[2] as f32 as f64];
        f64s::len(f64s::sub(w, c)) < b.radius - self.on_reach() && f64s::len(f64s::sub(f, c)) < b.radius
    }
}

impl Sdf3 for MeshField {
    fn distance(&self, p: Vec3d) -> f64 {
        self.signed64(p)
    }

    /// The distance clamped to `lower..upper`, searched only as far as the interval needs: a
    /// distance past a bound clamps to it, and the sign is taken only where the two signs clamp
    /// apart. On a watertight mesh a ball of the stream's that holds `p` gives the sign, and how
    /// far the surface is at least (no search when that is past the bound); else a point on a
    /// face is inside ([`MeshField::on_face`]); else the ball the search proves round `p` takes
    /// the sign of any ball of the stream's it meets (together they are one surface-free
    /// region), and only then the winding number gives it. Either way that ball is left for the
    /// next queries.
    fn distance_bounded(&self, p: Vec3d, lower: f64, upper: f64, hints: &mut Hints) -> f64 {
        let at = [p.x, p.y, p.z];
        let pf = [p.x as f32, p.y as f32, p.z as f32];
        let w = [pf[0] as f64, pf[1] as f64, pf[2] as f64];
        // the newest holding ball's sign, and the farthest any holding ball puts the surface
        let mut held: Option<(bool, f64)> = None;
        if self.watertight {
            for b in hints.balls().filter(|b| self.holds(b, w)) {
                let away = b.radius - f64s::len(f64s::sub(at, [b.center.x, b.center.y, b.center.z]));
                held = Some(held.map_or((b.inside, away), |(inside, far)| (inside, far.max(away))));
            }
        }
        // The sign after a search: `d` from a search capped at `cap` puts the surface at least
        // `d.min(cap)` from `p`, so a ball that far round the f32 point the winding number would
        // be taken at (less the way there) is free of it.
        let sign = |hints: &mut Hints, d: f64, cap: f64| -> bool {
            if let Some((inside, _)) = held {
                return inside;
            }
            let radius = d.min(cap) - f64s::len(f64s::sub(at, w)) - BALL_ROUNDING;
            // on a face: inside, and no ball (the surface is at the point)
            if radius <= self.on_reach() && self.on_face(pf, hints.near) {
                return true;
            }
            if !self.watertight || radius <= 0.0 {
                return self.winding(pf) > 0.5;
            }
            let met = hints.balls().find(|b| f64s::len(f64s::sub(w, [b.center.x, b.center.y, b.center.z])) < radius + b.radius).map(|b| b.inside);
            let inside = met.unwrap_or_else(|| self.winding(pf) > 0.5);
            hints.leave(Ball { center: Vec3d::new(w[0], w[1], w[2]), radius, inside });
            inside
        };
        if lower >= 0.0 {
            // Within `lower` of the surface, or inside: `lower` either way. Outside: the
            // distance, up to `upper`.
            match held {
                Some((true, _)) => return lower,
                Some((false, away)) if away >= upper => return upper,
                _ => {}
            }
            let d = self.unsigned64(at, lower, upper, &mut hints.near);
            if d <= lower || sign(hints, d, upper) { lower } else { d.min(upper) }
        } else if upper <= 0.0 {
            // The mirror image: within `-upper`, or outside, `upper`.
            match held {
                Some((false, _)) => return upper,
                Some((true, away)) if away >= -lower => return lower,
                _ => {}
            }
            let d = self.unsigned64(at, -upper, -lower, &mut hints.near);
            if d <= -upper || !sign(hints, d, -lower) { upper } else { (-d).max(lower) }
        } else if let Some((inside, away)) = held {
            // Either sign keeps its distance, to its own bound.
            let cap = if inside { -lower } else { upper };
            if away >= cap {
                return if inside { lower } else { upper };
            }
            let d = self.unsigned64(at, f64::NEG_INFINITY, cap, &mut hints.near);
            if inside { (-d).max(lower) } else { d.min(upper) }
        } else {
            // Searched to the outside's bound first: outside, that is all; inside, the distance
            // is searched on to the inside's bound when it went past the first.
            let d = self.unsigned64(at, f64::NEG_INFINITY, upper, &mut hints.near);
            if !sign(hints, d, upper) {
                return d.min(upper);
            }
            let d = if d < upper || -lower <= upper { d } else { self.unsigned64(at, f64::NEG_INFINITY, -lower, &mut hints.near) };
            (-d).max(lower)
        }
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
    use crate::sdf::HINT_BALLS;

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
    fn bounded_queries_preserve_signed_distances_and_overlap() {
        for open in [false, true] {
            let f = MeshField::new(cube(open));
            let mut hints = Hints::default();
            // Cross the surface, travel inside it, and leave through the
            // (possibly open) lid. The bounds cover both signs and contact.
            for p in [[3.0, 0.0, 0.0], [2.8, 0.0, 0.0], [1.05, 0.0, 0.0], [0.95, 0.0, 0.0], [0.0, 0.0, 0.0], [0.0, 0.2, 0.0], [0.0, 1.2, 0.0]] {
                let p = Vec3d::new(p[0], p[1], p[2]);
                let expected = f.distance(p);
                for (lower, upper) in [(f64::NEG_INFINITY, f64::INFINITY), (-0.2, 0.2), (0.1, 0.4), (-0.4, -0.1), (f64::NEG_INFINITY, 0.5), (-0.5, f64::INFINITY), (0.0, 0.0), (1.9, 2.5), (-2.0, -0.5)] {
                    assert_eq!(f.distance_bounded(p, lower, upper, &mut hints), expected.clamp(lower, upper), "open={open}, {p:?}, {lower}..{upper}");
                }
            }
        }
        let mut data = cube(false);
        let count = data.positions.len() as u32;
        data.positions.extend(data.positions.clone().iter().map(|p| [p[0] + 1.0, p[1], p[2]]));
        data.triangles.extend(data.triangles.clone().iter().map(|t| [t[0] + count, t[1] + count, t[2] + count]));
        let f = MeshField::new(data);
        let (q, p) = (Vec3d::new(0.4, 0.0, 0.0), Vec3d::new(0.5, 0.0, 0.0));
        let mut hints = Hints::default();
        assert_eq!(f.distance_bounded(q, f64::NEG_INFINITY, f64::INFINITY, &mut hints), f.distance(q));
        assert!(hints.balls().next().is_some_and(|b| b.inside && f.holds(b, [p.x, p.y, p.z])), "{:?}", hints.balls().next());
        assert_eq!(f.distance_bounded(p, -1.0, 1.0, &mut hints), f.distance(p));
        assert!(f.distance(p) < 0.0, "overlapping closed shells remain inside");
    }

    #[test]
    fn a_ball_answers_a_sign_inside_it_or_where_a_proven_ball_meets_it_and_only_on_a_watertight_mesh() {
        // A ball free of the cube's surface claiming the inside where the cube's outside is: it
        // answers wherever it is used. Along x the surface is at 1, the ball spans 1.5..3.5.
        let wrong = Ball { center: Vec3d::new(2.5, 0.0, 0.0), radius: 1.0, inside: true };
        let e = 1.0 / 1024.0;
        let closed = MeshField::new(cube(false));
        let stream = || {
            let mut hints = Hints::default();
            hints.leave(wrong);
            hints
        };
        let at = |x: f64| Vec3d::new(x, 0.0, 0.0);
        // held: just inside it
        let mut hints = stream();
        assert_eq!(closed.distance_bounded(at(1.5 + e), f64::NEG_INFINITY, f64::INFINITY, &mut hints), -(0.5 + e));
        assert_eq!(hints.balls().copied().collect::<Vec<_>>(), vec![wrong], "a held sign leaves no ball");
        // met: outside it, the surface proven far enough away that the two balls overlap (past 1.25)
        let mut hints = stream();
        assert_eq!(closed.distance_bounded(at(1.25 + e), f64::NEG_INFINITY, f64::INFINITY, &mut hints), -(0.25 + e));
        assert_eq!(hints.balls().next().map(|b| (b.center, b.inside)), Some((at(1.25 + e), true)), "a met sign leaves its ball");
        // neither: just short of meeting it, the winding number decides
        let mut hints = stream();
        assert_eq!(closed.distance_bounded(at(1.25 - e), f64::NEG_INFINITY, f64::INFINITY, &mut hints), 0.25 - e);
        assert_eq!(hints.balls().next().map(|b| (b.center, b.inside)), Some((at(1.25 - e), false)), "the winding number's sign leaves its ball");
        // A held ball that puts the surface past the bound answers the bound.
        let mut hints = Hints::default();
        hints.leave(Ball { center: Vec3d::new(3.0, 0.0, 0.0), radius: 1.0, inside: false });
        assert_eq!(closed.distance_bounded(Vec3d::new(3.25, 0.0, 0.0), f64::NEG_INFINITY, 0.5, &mut hints), 0.5);
        // An open mesh never takes a sign from a ball, nor leaves one.
        let open = MeshField::new(cube(true));
        for x in [1.5 + e, 1.25 + e] {
            let mut hints = stream();
            assert_eq!(open.distance_bounded(at(x), f64::NEG_INFINITY, f64::INFINITY, &mut hints), open.distance(at(x)));
            assert!(open.distance(at(x)) > 0.0);
        }
        let mut hints = Hints::default();
        for p in [[3.0, 0.0, 0.0], [0.0, -0.5, 0.0], [0.0, 3.0, 0.0]] {
            let p = Vec3d::new(p[0], p[1], p[2]);
            assert_eq!(open.distance_bounded(p, f64::NEG_INFINITY, f64::INFINITY, &mut hints), open.distance(p));
        }
        assert!(hints.balls().next().is_none());
    }

    #[test]
    fn a_ball_reaches_only_as_far_as_the_search_proved() {
        let f = MeshField::new(cube(false));
        let p = Vec3d::new(3.0, 0.0, 0.0);
        // exact: the whole distance; capped: only the cap, though the surface is farther
        let mut hints = Hints::default();
        assert_eq!(f.distance_bounded(p, f64::NEG_INFINITY, f64::INFINITY, &mut hints), 2.0);
        assert_eq!(hints.balls().next(), Some(&Ball { center: p, radius: 2.0 - BALL_ROUNDING, inside: false }));
        let mut hints = Hints::default();
        assert_eq!(f.distance_bounded(p, 0.1, 0.5, &mut hints), 0.5);
        assert_eq!(hints.balls().next(), Some(&Ball { center: p, radius: 0.5 - BALL_ROUNDING, inside: false }));
        // a query within its lower bound takes no sign and leaves no ball
        let mut hints = Hints::default();
        assert_eq!(f.distance_bounded(Vec3d::new(1.25, 0.0, 0.0), 0.5, 1.0, &mut hints), 0.5);
        assert!(hints.balls().next().is_none());
        // a stream keeps its newest few balls
        for k in 0..HINT_BALLS + 3 {
            hints.leave(Ball { center: Vec3d::new(k as f64, 0.0, 0.0), radius: 0.1, inside: false });
        }
        let kept: Vec<f64> = hints.balls().map(|b| b.center.x).collect();
        assert_eq!(kept, (3..HINT_BALLS + 3).rev().map(|k| k as f64).collect::<Vec<_>>());
    }

    #[test]
    fn a_point_on_a_face_is_inside() {
        // The closed-set rule: on the cube's faces, edges and corners the sign is the inside's,
        // which way ever the winding number's zeros fall; a hair off them, the winding number's.
        let f = MeshField::new(cube(false));
        let on = [[0.25, 1.0, -0.5], [1.0, 0.3, 0.7], [-0.6, -0.2, -1.0], [0.0, 1.0, 0.0], [0.5, -1.0, 0.5], [-1.0, 0.75, -0.25], [0.3, 0.6, 1.0], [1.0, 1.0, 0.2], [1.0, 1.0, 1.0]];
        let mut outside_by_winding = 0;
        let mut hints = Hints::default();
        for p in on {
            let pf = [p[0] as f32, p[1] as f32, p[2] as f32];
            assert!(f.on_face(pf, 0), "{p:?}");
            outside_by_winding += (f.winding(pf) <= 0.5) as usize;
            let v = Vec3d::new(p[0], p[1], p[2]);
            assert!(f.distance(v).is_sign_negative(), "{p:?}: {}", f.distance(v));
            for (lower, upper) in [(f64::NEG_INFINITY, f64::INFINITY), (-0.5, 0.5), (-0.5, -0.1), (0.1, 0.5)] {
                assert_eq!(f.distance_bounded(v, lower, upper, &mut hints).to_bits(), f.distance(v).clamp(lower, upper).to_bits(), "{p:?} {lower}..{upper}");
            }
            assert!(hints.balls().all(|b| f64s::len(f64s::sub([b.center.x, b.center.y, b.center.z], p)) >= b.radius), "no ball holds a face");
            for (off, inside) in [(1.0 / 1024.0, false), (-1.0 / 1024.0, true)] {
                // off every face the point is on (each coordinate at +-1)
                let q = p.map(|x| if x.abs() == 1.0 { x + off * x } else { x });
                assert!(!f.on_face([q[0] as f32, q[1] as f32, q[2] as f32], 0), "{q:?}");
                assert_eq!(f.distance(Vec3d::new(q[0], q[1], q[2])) < 0.0, inside, "{q:?}");
            }
        }
        assert!(outside_by_winding > 0, "some of these points the winding number alone puts outside");
    }

    #[test]
    fn on_tri_puts_on_a_face_only_points_within_reach_of_it() {
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 11) as f64 / (1u64 << 53) as f64
        };
        let (mut flat_on, mut tilted_on) = (0, 0);
        for k in 0..30000 {
            // a triangle within 200 mm of the origin, 20 mm across: in a coordinate plane, or tilted
            let a = [(next() * 400.0 - 200.0) as f32, (next() * 400.0 - 200.0) as f32, (next() * 400.0 - 200.0) as f32];
            let mut b = [a[0] + (next() * 40.0 - 20.0) as f32, a[1] + (next() * 40.0 - 20.0) as f32, a[2] + (next() * 40.0 - 20.0) as f32];
            let mut c = [a[0] + (next() * 40.0 - 20.0) as f32, a[1] + (next() * 40.0 - 20.0) as f32, a[2] + (next() * 40.0 - 20.0) as f32];
            let flat = k % 2 == 0;
            if flat {
                (b[1], c[1]) = (a[1], a[1]);
            }
            // a point in its plane, inside, on or past its edges, and one a few ulps off it
            let (u, v) = (next() * 1.2 - 0.1, next() * 1.2 - 0.1);
            let mut p = [0.0f32; 3];
            for i in 0..3 {
                p[i] = a[i] + (u as f32) * (b[i] - a[i]) + (v as f32) * (c[i] - a[i]);
            }
            let ulps = (next() * 8.0) as u32;
            let mut q = p;
            q[1] = f32::from_bits(q[1].to_bits().wrapping_add(ulps));
            let m = [a, b, c].iter().flatten().fold(0.0f32, |m, x| m.max(x.abs()));
            let reach = 1e-5 * (1.0 + m as f64);
            let w = |v: V3| [v[0] as f64, v[1] as f64, v[2] as f64];
            for p in [p, q] {
                if on_tri(p, a, b, c) {
                    let d = f64s::len(f64s::sub(w(p), f64s::closest_on_tri64(w(p), w(a), w(b), w(c))));
                    assert!(d <= reach, "{p:?} on {a:?} {b:?} {c:?} at {d} past {reach}");
                    if flat { flat_on += 1 } else { tilted_on += 1 }
                }
            }
        }
        assert!(flat_on > 4000, "points in a flat face's plane, in it, are on it: {flat_on} (tilted: {tilted_on})");
    }

    #[test]
    fn bounded_searches_find_the_nearest_triangle_exactly() {
        // A 4 x 4 x 4 lattice of small cubes far from the origin, where f64 points are not f32
        // ones: every search, capped or ended early, against every triangle.
        let mut data = MeshData::default();
        let one = cube(false);
        for k in 0..64u32 {
            let at = [100.0 + (k % 4) as f32 * 2.7, -60.0 + (k / 4 % 4) as f32 * 2.9, 30.0 + (k / 16) as f32 * 3.1];
            let base = data.positions.len() as u32;
            data.positions.extend(one.positions.iter().map(|p| [at[0] + p[0] * 0.6, at[1] + p[1] * 0.6, at[2] + p[2] * 0.6]));
            data.triangles.extend(one.triangles.iter().map(|t| [t[0] + base, t[1] + base, t[2] + base]));
        }
        let f = MeshField::new(data.clone());
        let w = |v: V3| [v[0] as f64, v[1] as f64, v[2] as f64];
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 11) as f64 / (1u64 << 53) as f64
        };
        let mut carried = 0u32;
        for _ in 0..400 {
            let p = [97.0 + next() * 14.0, -63.0 + next() * 15.0, 27.0 + next() * 16.0];
            let exact = data.triangles.iter().map(|t| {
                let q = f64s::closest_on_tri64(p, w(data.positions[t[0] as usize]), w(data.positions[t[1] as usize]), w(data.positions[t[2] as usize]));
                f64s::len(f64s::sub(p, q))
            }).fold(f64::INFINITY, f64::min);
            assert_eq!(f.unsigned64(p, f64::NEG_INFINITY, f64::INFINITY, &mut 0), exact, "{p:?}");
            assert_eq!(f.unsigned64(p, f64::NEG_INFINITY, f64::INFINITY, &mut carried), exact, "{p:?} from the last point's nearest");
            let cap = next() * 2.0;
            let capped = f.unsigned64(p, f64::NEG_INFINITY, cap, &mut 0);
            if exact < cap { assert_eq!(capped, exact, "{p:?} under {cap}") } else { assert!(capped >= cap, "{p:?} over {cap}") }
            let enough = next() * 2.0;
            let early = f.unsigned64(p, enough, f64::INFINITY, &mut 0);
            if exact <= enough { assert!(early <= enough, "{p:?} within {enough}") } else { assert_eq!(early, exact, "{p:?} past {enough}") }
            let v = Vec3d::new(p[0], p[1], p[2]);
            let (a, b) = (next() * 3.0 - 1.5, next() * 3.0 - 1.5);
            let (lower, upper) = (a.min(b), a.max(b));
            assert_eq!(f.distance_bounded(v, lower, upper, &mut Hints::default()), f.distance(v).clamp(lower, upper), "{p:?}, {lower}..{upper}");
        }
        // A walk in small steps, in and out of the cubes, its queries one stream.
        let mut hints = Hints::default();
        for i in 0..2000 {
            let t = i as f64 * 0.007;
            let v = Vec3d::new(99.0 + t, -61.0 + t * 0.9, 29.0 + t * 1.1);
            for (lower, upper) in [(f64::NEG_INFINITY, f64::INFINITY), (-0.3, 0.3), (0.05, 0.2), (-0.2, -0.05)] {
                assert_eq!(f.distance_bounded(v, lower, upper, &mut hints), f.distance(v).clamp(lower, upper), "{v:?}, {lower}..{upper}");
            }
        }
    }

    #[test]
    fn the_select_atan2_is_mesh_atan2_bit_for_bit() {
        // Every special value against every other, then many over a wide range of exponents.
        let tan8 = 0.4142135679721832f32;
        let special = [0.0f32, -0.0, 1.0, -1.0, 0.5, -2.0, tan8, tan8.next_up(), tan8.next_down(), f32::MIN_POSITIVE, -f32::MIN_POSITIVE, 1e-40, -1e-40, 1e30, -1e30, f32::MAX, f32::INFINITY, f32::NEG_INFINITY, f32::NAN];
        let same = |y: f32, x: f32| {
            let (a, b) = (mesh_atan2(y, x), mesh_atan2_select(y, x));
            assert!(a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan()), "atan2({y:e}, {x:e}): {a:e} vs {b:e}");
        };
        for &y in &special {
            for &x in &special {
                same(y, x);
                same(y * x, x);
            }
        }
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        let mut bits = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        // a sign and mantissa with an exponent of 2^-20..2^19
        let f = |w: u64| f32::from_bits((w as u32 & 0x807f_ffff) | ((107 + (w >> 40) as u32 % 40) << 23));
        for _ in 0..200_000 {
            let b = bits();
            let (y, x) = (f(b), f(b.rotate_left(23)));
            same(y, x);
            same(y, if b & 1 == 0 { y } else { -y });
        }
    }

    #[test]
    fn leaf_lanes_wind_as_one_triangle_at_a_time() {
        // The winding number one triangle at a time, as the leaves summed before their lanes.
        let one_at_a_time = |f: &MeshField, p: V3| -> f32 {
            let mut w = 0.0f32;
            let mut stack = NodeStack::root();
            while let Some(i) = stack.pop() {
                let n = &f.nodes[i as usize];
                let d = sub(n.center, p);
                let dl = len(d);
                if dl > 2.0 * n.radius && dl > 0.0 {
                    w += dot(n.normal, d) / (4.0 * std::f32::consts::PI * dl * dl * dl);
                    continue;
                }
                if n.leaf {
                    for &t in &f.order[n.left as usize..n.right as usize] {
                        let (a, b, c) = f.tri(t);
                        w += tri_winding(p, a, b, c);
                    }
                } else {
                    stack.push(n.left);
                    stack.push(n.right);
                }
            }
            w
        };
        // A lattice of small cubes far from the origin, and a single cube: at random points, and
        // on the triangles themselves (corners, edge middles, centroids, points in a face's plane).
        let mut lattice = MeshData::default();
        let one = cube(false);
        for k in 0..64u32 {
            let at = [100.0 + (k % 4) as f32 * 2.7, -60.0 + (k / 4 % 4) as f32 * 2.9, 30.0 + (k / 16) as f32 * 3.1];
            let base = lattice.positions.len() as u32;
            lattice.positions.extend(one.positions.iter().map(|p| [at[0] + p[0] * 0.6, at[1] + p[1] * 0.6, at[2] + p[2] * 0.6]));
            lattice.triangles.extend(one.triangles.iter().map(|t| [t[0] + base, t[1] + base, t[2] + base]));
        }
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 40) as f32 / (1u64 << 24) as f32
        };
        for data in [lattice, one] {
            let f = MeshField::new(data.clone());
            let (lo, hi) = (f.min, f.max);
            let mut points: Vec<V3> = (0..3000).map(|_| [lo[0] - 1.0 + next() * (hi[0] - lo[0] + 2.0), lo[1] - 1.0 + next() * (hi[1] - lo[1] + 2.0), lo[2] - 1.0 + next() * (hi[2] - lo[2] + 2.0)]).collect();
            for t in &data.triangles {
                let [a, b, c] = t.map(|i| data.positions[i as usize]);
                points.extend([a, b, c]);
                points.push([(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5, (a[2] + b[2]) * 0.5]);
                points.push([(a[0] + b[0] + c[0]) / 3.0, (a[1] + b[1] + c[1]) / 3.0, (a[2] + b[2] + c[2]) / 3.0]);
                let (u, v) = (next(), next());
                points.push(add_scaled(add_scaled(a, sub(b, a), u * 0.5), sub(c, a), v * 0.5));
            }
            for p in points {
                assert_eq!(f.winding(p).to_bits(), one_at_a_time(&f, p).to_bits(), "{p:?}");
            }
        }
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
