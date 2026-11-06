//! Joint geometry as a small solid tree over the original parts: primitives
//! (frusta, spheres, slabs) combined with the parts by union, difference and
//! intersection. One tree lowers to both a mesh solid (`makepad_csg_graph`
//! ops, exact cylinders for printing) and a distance field ([`SdfNode`]) for
//! the field checks, so the printed and the checked geometry are the same.
//!
//! Model frame, millimetres.

use makepad_csg_graph::{BoolMode, Graph, NodeId, Op};
use makepad_csg_math::Vec3d;
use makepad_csg_sdf::{Hints, Sdf3, SdfNode, SdfOp};

pub type P3 = [f64; 3];

pub fn add(a: P3, b: P3) -> P3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
pub fn sub(a: P3, b: P3) -> P3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
pub fn scale(a: P3, s: f64) -> P3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
pub fn dot(a: P3, b: P3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
pub fn cross(a: P3, b: P3) -> P3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
pub fn len(a: P3) -> f64 {
    dot(a, a).sqrt()
}
pub fn norm(a: P3) -> P3 {
    let l = len(a);
    if l > 0.0 {
        scale(a, 1.0 / l)
    } else {
        [0.0, 1.0, 0.0]
    }
}
pub fn madd(a: P3, b: P3, s: f64) -> P3 {
    add(a, scale(b, s))
}
pub fn v3(p: P3) -> Vec3d {
    Vec3d::new(p[0], p[1], p[2])
}
pub fn p3(v: Vec3d) -> P3 {
    [v.x, v.y, v.z]
}

/// Two unit vectors perpendicular to `n` and to each other (right-handed: u x v = n).
pub fn basis(n: P3) -> (P3, P3) {
    let n = norm(n);
    let t = if n[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
    let u = norm(cross(t, n));
    let v = cross(n, u);
    (u, v)
}

/// Rodrigues: `p` turned by `ang` radians about the unit `axis` (through the origin).
pub fn rotate(p: P3, axis: P3, ang: f64) -> P3 {
    let (s, c) = ang.sin_cos();
    let k = axis;
    add(add(scale(p, c), scale(cross(k, p), s)), scale(k, dot(k, p) * (1.0 - c)))
}

/// A row-major rotation matrix taking +Y to `dir`.
pub fn rot_y_to(dir: P3) -> [[f64; 3]; 3] {
    let d = norm(dir);
    let y = [0.0, 1.0, 0.0];
    let c = dot(y, d);
    if c > 1.0 - 1e-12 {
        return [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    }
    if c < -1.0 + 1e-12 {
        return [[1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, -1.0]];
    }
    let k = norm(cross(y, d));
    let ang = c.clamp(-1.0, 1.0).acos();
    let cols = [rotate([1.0, 0.0, 0.0], k, ang), rotate(y, k, ang), rotate([0.0, 0.0, 1.0], k, ang)];
    std::array::from_fn(|i| std::array::from_fn(|j| cols[j][i]))
}

/// Degrees `[x, y, z]` with `R = Rz Ry Rx` (the convention of csg_graph and csg_sdf transforms).
pub fn euler_xyz(r: &[[f64; 3]; 3]) -> P3 {
    let sy = (-r[2][0]).clamp(-1.0, 1.0);
    let y = sy.asin();
    if sy.abs() < 1.0 - 1e-9 {
        [r[2][1].atan2(r[2][2]).to_degrees(), y.to_degrees(), r[1][0].atan2(r[0][0]).to_degrees()]
    } else {
        [0.0, y.to_degrees(), (-r[0][1]).atan2(r[1][1]).to_degrees()]
    }
}

/// A primitive of a joint.
#[derive(Clone, Debug, PartialEq)]
pub enum Prim {
    /// A cone frustum from `a` (radius `ra`) to `b` (radius `rb`); a cylinder when they match.
    Frustum { a: P3, b: P3, ra: f64, rb: f64 },
    Sphere { c: P3, r: f64 },
    /// The points with `lo <= dot(n, p) <= hi` (`n` unit) within `half` of `c` across `n` (a box: a slab as
    /// wide as needed, not an unbounded one; huge boxes cost the mesh boolean its precision).
    Slab { n: P3, lo: f64, hi: f64, c: P3, half: f64 },
}

impl Prim {
    pub fn cyl(a: P3, b: P3, r: f64) -> Prim {
        Prim::Frustum { a, b, ra: r, rb: r }
    }

    /// Exact signed distance.
    pub fn distance(&self, p: P3) -> f64 {
        match self {
            Prim::Sphere { c, r } => len(sub(p, *c)) - r,
            Prim::Slab { n, lo, hi, c, half } => {
                let d = dot(*n, p);
                let (u, v) = basis(*n);
                let q = sub(p, *c);
                let (x, y, z) = (dot(q, u).abs() - half, (d - 0.5 * (lo + hi)).abs() - 0.5 * (hi - lo), dot(q, v).abs() - half);
                len([x.max(0.0), y.max(0.0), z.max(0.0)]) + x.max(y).max(z).min(0.0)
            }
            Prim::Frustum { a, b, ra, rb } => {
                let ax = sub(*b, *a);
                let h = len(ax);
                let u = scale(ax, 1.0 / h);
                let q = sub(p, *a);
                let t = dot(q, u);
                let rho = len(sub(q, scale(u, t)));
                capped_cone(rho, t - h * 0.5, h * 0.5, *ra, *rb)
            }
        }
    }

    /// The same solid as a csg_sdf node.
    pub fn to_node(&self) -> SdfNode {
        match self {
            Prim::Sphere { c, r } => SdfNode::Transform { at: *c, rotate: [0.0; 3], scale: 1.0, of: Box::new(SdfNode::Sphere { radius: *r }) },
            Prim::Slab { n, lo, hi, c, half } => SdfNode::Transform {
                at: slab_center(*n, *lo, *hi, *c),
                rotate: euler_xyz(&slab_frame(*n)),
                scale: 1.0,
                of: Box::new(SdfNode::Box { size: [2.0 * half, hi - lo, 2.0 * half], radius: 0.0 }),
            },
            Prim::Frustum { a, b, ra, rb } => {
                let ax = sub(*b, *a);
                SdfNode::Transform {
                    at: scale(add(*a, *b), 0.5),
                    rotate: euler_xyz(&rot_y_to(ax)),
                    scale: 1.0,
                    of: Box::new(SdfNode::Cylinder { radius: *ra, top_radius: *rb, height: len(ax) }),
                }
            }
        }
    }

    /// The same solid as csg_graph nodes (`segments` round a circle).
    pub fn lower(&self, g: &mut Graph, segments: u32) -> Result<NodeId, String> {
        match self {
            Prim::Sphere { c, r } => {
                let s = g.push(Op::Sphere { radius: *r, segments, rings: segments / 2 })?;
                g.push(Op::Transform { source: s, at: *c, rotate: [0.0; 3], scale: [1.0; 3] })
            }
            Prim::Slab { n, lo, hi, c, half } => {
                let s = g.push(Op::Box { size: [2.0 * half, hi - lo, 2.0 * half], radius: 0.0 })?;
                g.push(Op::Transform { source: s, at: slab_center(*n, *lo, *hi, *c), rotate: euler_xyz(&slab_frame(*n)), scale: [1.0; 3] })
            }
            Prim::Frustum { a, b, ra, rb } => {
                let ax = sub(*b, *a);
                let s = g.push(Op::Cylinder { radius: *ra, top_radius: *rb, height: len(ax), segments })?;
                g.push(Op::Transform { source: s, at: scale(add(*a, *b), 0.5), rotate: euler_xyz(&rot_y_to(ax)), scale: [1.0; 3] })
            }
        }
    }

    /// An axis-aligned box holding it (`None` for a slab).
    pub fn bounds(&self) -> Option<(P3, P3)> {
        match self {
            Prim::Sphere { c, r } => Some((sub(*c, [*r; 3]), add(*c, [*r; 3]))),
            Prim::Slab { n, lo, hi, c, half } => {
                let m = slab_center(*n, *lo, *hi, *c);
                let r = half * std::f64::consts::SQRT_2 + 0.5 * (hi - lo);
                Some((sub(m, [r; 3]), add(m, [r; 3])))
            }
            Prim::Frustum { a, b, ra, rb } => {
                let r = ra.max(*rb);
                Some((std::array::from_fn(|i| a[i].min(b[i]) - r), std::array::from_fn(|i| a[i].max(b[i]) + r)))
            }
        }
    }
}

/// The slab's box centre: `c` moved along `n` to the middle of `lo..hi`.
fn slab_center(n: P3, lo: f64, hi: f64, c: P3) -> P3 {
    madd(c, n, 0.5 * (lo + hi) - dot(n, c))
}

/// The slab box's frame: its Y along `n`, its X and Z along [`basis`] (so the distance above matches).
fn slab_frame(n: P3) -> [[f64; 3]; 3] {
    let (u, v) = basis(n);
    let n = norm(n);
    // columns u, n, v: X -> u, Y -> n, Z -> v (right-handed when u x n = v ... flip v if needed)
    let v = if dot(cross(u, n), v) < 0.0 { scale(v, -1.0) } else { v };
    [[u[0], n[0], v[0]], [u[1], n[1], v[1]], [u[2], n[2], v[2]]]
}

/// Signed distance of a capped cone in its (radial, axial) plane: axial `y` centred, half height `hh`,
/// radius `r1` at `-hh` and `r2` at `+hh`.
fn capped_cone(qx: f64, qy: f64, hh: f64, r1: f64, r2: f64) -> f64 {
    let (k1x, k1y) = (r2, hh);
    let (k2x, k2y) = (r2 - r1, 2.0 * hh);
    let cax = qx - qx.min(if qy < 0.0 { r1 } else { r2 });
    let cay = qy.abs() - hh;
    let t = (((k1x - qx) * k2x + (k1y - qy) * k2y) / (k2x * k2x + k2y * k2y)).clamp(0.0, 1.0);
    let cbx = qx - k1x + k2x * t;
    let cby = qy - k1y + k2y * t;
    let s = if cbx < 0.0 && cay < 0.0 { -1.0 } else { 1.0 };
    s * (cax * cax + cay * cay).min(cbx * cbx + cby * cby).sqrt()
}

/// A solid built from the original parts and joint primitives.
#[derive(Clone, Debug, PartialEq)]
pub enum Solid {
    /// The original part `i` (before any joint).
    Part(usize),
    Prim(Prim),
    Union(Vec<Solid>),
    /// The first with every next one cut away.
    Diff(Box<Solid>, Vec<Solid>),
    Inter(Vec<Solid>),
}

impl Solid {
    /// Conservative bounds for joint tools, which do not read an original part.
    pub(crate) fn tool_bounds(&self) -> Option<makepad_csg_math::BBox3d> {
        use makepad_csg_math::BBox3d;
        match self {
            Solid::Part(_) => None,
            Solid::Prim(Prim::Sphere { c, r }) => {
                Some(BBox3d::from_min_max(v3(sub(*c, [*r; 3])), v3(add(*c, [*r; 3]))))
            }
            Solid::Prim(Prim::Frustum { a, b, ra, rb }) => {
                let r = Vec3d::new(ra.max(*rb), ra.max(*rb), ra.max(*rb));
                Some(BBox3d::from_min_max(v3(*a).min(v3(*b)) - r, v3(*a).max(v3(*b)) + r))
            }
            Solid::Prim(Prim::Slab { n, lo, hi, c, half }) => {
                let frame = slab_frame(*n);
                let center = slab_center(*n, *lo, *hi, *c);
                let extent = [*half, (hi - lo) * 0.5, *half];
                let radius: P3 = std::array::from_fn(|i| (0..3).map(|j| frame[i][j].abs() * extent[j]).sum());
                Some(BBox3d::from_min_max(v3(sub(center, radius)), v3(add(center, radius))))
            }
            Solid::Union(list) => {
                let mut bounds = BBox3d::empty();
                for s in list { bounds = bounds.union(s.tool_bounds()?); }
                Some(bounds)
            }
            // The minuend and every intersection operand are upper bounds.
            Solid::Diff(a, _) => a.tool_bounds(),
            Solid::Inter(list) => list.iter().find_map(Solid::tool_bounds),
        }
    }

    pub(crate) fn separate_tools(&self, other: &Solid) -> bool {
        if let (Some(a), Some(b)) = (self.tool_bounds(), other.tool_bounds()) {
            if makepad_csg_graph::disjoint_tool_bounds(a, b) { return true; }
        }
        // A union's box can span the space between distant tools. Prove each
        // component separately before declaring that this is an overlap.
        match (self, other) {
            (Solid::Union(list), _) => list.iter().all(|s| s.separate_tools(other)),
            (_, Solid::Union(list)) => list.iter().all(|s| self.separate_tools(s)),
            _ => false,
        }
    }

    pub fn diff(a: Solid, cuts: Vec<Solid>) -> Solid {
        Solid::Diff(Box::new(a), cuts)
    }

    pub fn prims(list: Vec<Prim>) -> Vec<Solid> {
        list.into_iter().map(Solid::Prim).collect()
    }

    /// The distance with `parts[i]` the original parts' fields.
    pub fn distance(&self, parts: &[&(dyn Sdf3 + Sync)], p: P3) -> f64 {
        match self {
            Solid::Part(i) => parts[*i].distance(v3(p)),
            Solid::Prim(q) => q.distance(p),
            Solid::Union(l) => l.iter().fold(f64::INFINITY, |m, s| m.min(s.distance(parts, p))),
            Solid::Inter(l) => l.iter().fold(f64::NEG_INFINITY, |m, s| m.max(s.distance(parts, p))),
            Solid::Diff(a, l) => l.iter().fold(a.distance(parts, p), |m, s| m.max(-s.distance(parts, p))),
        }
    }

    /// The same solid distance, restricted to the interval the caller needs; `hints`, the query
    /// stream's, holds part `i`'s in its slot `i`.
    pub fn distance_bounded(&self, parts: &[&(dyn Sdf3 + Sync)], p: P3, lower: f64, upper: f64, hints: &mut Hints) -> f64 {
        match self {
            Solid::Part(i) => parts[*i].distance_bounded(v3(p), lower, upper, hints.part(*i)),
            Solid::Prim(q) => q.distance(p).clamp(lower, upper),
            Solid::Union(l) => {
                let mut best = upper;
                // Added joint primitives follow the original part. They are
                // cheap and often already establish the union's distance.
                for s in l.iter().rev() {
                    best = s.distance_bounded(parts, p, lower, best, hints);
                    if best == lower { break; }
                }
                best
            }
            Solid::Inter(l) => {
                let mut best = lower;
                for s in l.iter().rev() {
                    best = s.distance_bounded(parts, p, best, upper, hints);
                    if best == upper { break; }
                }
                best
            }
            Solid::Diff(a, cuts) => {
                // The joint's cuts often exclude a point without querying the
                // original mesh at all. Negation also reverses the interval.
                let mut best = lower;
                for s in cuts {
                    best = -s.distance_bounded(parts, p, -upper, -best, hints);
                    if best == upper { return best; }
                }
                a.distance_bounded(parts, p, best, upper, hints)
            }
        }
    }

    /// A csg_sdf node with `parts[i]` the original parts' nodes.
    pub fn to_node(&self, parts: &[SdfNode]) -> SdfNode {
        let combine = |op: SdfOp, of: Vec<SdfNode>| SdfNode::Combine { op, blend: 0.0, of };
        match self {
            Solid::Part(i) => parts[*i].clone(),
            Solid::Prim(q) => q.to_node(),
            Solid::Union(l) => combine(SdfOp::Union, l.iter().map(|s| s.to_node(parts)).collect()),
            Solid::Inter(l) => combine(SdfOp::Intersection, l.iter().map(|s| s.to_node(parts)).collect()),
            Solid::Diff(a, l) => combine(SdfOp::Difference, std::iter::once(a.to_node(parts)).chain(l.iter().map(|s| s.to_node(parts))).collect()),
        }
    }

    /// Whether it reads an original part.
    fn has_part(&self) -> bool {
        match self {
            Solid::Part(_) => true,
            Solid::Prim(_) => false,
            Solid::Union(l) | Solid::Inter(l) => l.iter().any(Solid::has_part),
            Solid::Diff(a, l) => a.has_part() || l.iter().any(Solid::has_part),
        }
    }

    /// The solid a chain of joint edits starts from and its tools in the order they apply
    /// (`true`: cut away). Only tools that read no part count: a union of two parts ends it.
    fn chain(&self) -> (&Solid, Vec<(bool, &Solid)>) {
        let mut ops = Vec::new();
        let mut s = self;
        loop {
            match s {
                Solid::Diff(a, cuts) if a.has_part() && !cuts.iter().any(Solid::has_part) => {
                    ops.extend(cuts.iter().rev().map(|c| (true, c)));
                    s = a;
                }
                Solid::Union(l) if l.len() > 1 && l[0].has_part() && !l[1..].iter().any(Solid::has_part) => {
                    ops.extend(l[1..].iter().rev().map(|t| (false, t)));
                    s = &l[0];
                }
                _ => break,
            }
        }
        ops.reverse();
        (s, ops)
    }

    /// csg_graph nodes with `parts[i]` the original parts' nodes.
    ///
    /// A part's joint edits cross its whole mesh once per group, not once per tool: cuts in a
    /// row are one difference by their union and additions in a row one union, and an edit moves
    /// back past the other kind's group before it when it misses every tool there (a cut that
    /// misses an addition takes nothing from it). The solid is the same; only the mesh Booleans
    /// are grouped. Distances (`to_node`, `distance_bounded`) keep the tree as built.
    pub fn lower(&self, g: &mut Graph, parts: &[NodeId], segments: u32) -> Result<NodeId, String> {
        let (base, ops) = self.chain();
        if ops.len() > 1 {
            let mut groups: Vec<(bool, Vec<&Solid>)> = Vec::new();
            for (cut, tool) in ops {
                let n = groups.len();
                if n > 0 && groups[n - 1].0 == cut {
                    groups[n - 1].1.push(tool);
                } else if n > 1 && groups[n - 2].0 == cut && groups[n - 1].1.iter().all(|t| t.separate_tools(tool)) {
                    groups[n - 2].1.push(tool);
                } else {
                    groups.push((cut, vec![tool]));
                }
            }
            let mut acc = base.lower(g, parts, segments)?;
            for (cut, tools) in groups {
                let tool = match tools.as_slice() {
                    [one] => one.lower(g, parts, segments)?,
                    _ => Solid::Union(tools.into_iter().cloned().collect()).lower(g, parts, segments)?,
                };
                let mode = if cut { BoolMode::Difference } else { BoolMode::Union };
                acc = g.push(Op::Boolean { mode, a: acc, b: vec![tool] })?;
            }
            return Ok(acc);
        }
        let many = |g: &mut Graph, l: &[Solid]| -> Result<Vec<NodeId>, String> { l.iter().map(|s| s.lower(g, parts, segments)).collect() };
        match self {
            Solid::Part(i) => Ok(parts[*i]),
            Solid::Prim(q) => q.lower(g, segments),
            Solid::Union(l) | Solid::Inter(l) => {
                let mode = if matches!(self, Solid::Union(_)) { BoolMode::Union } else { BoolMode::Intersection };
                let mut ids = many(g, l)?;
                if ids.len() == 1 {
                    return Ok(ids[0]);
                }
                let a = ids.remove(0);
                g.push(Op::Boolean { mode, a, b: ids })
            }
            Solid::Diff(a, l) => {
                let a = a.lower(g, parts, segments)?;
                let b = many(g, l)?;
                if b.is_empty() {
                    return Ok(a);
                }
                g.push(Op::Boolean { mode: BoolMode::Difference, a, b })
            }
        }
    }

    /// The parts it reads.
    pub fn parts(&self, out: &mut Vec<usize>) {
        match self {
            Solid::Part(i) => out.push(*i),
            Solid::Prim(_) => {}
            Solid::Union(l) | Solid::Inter(l) => l.iter().for_each(|s| s.parts(out)),
            Solid::Diff(a, l) => {
                a.parts(out);
                l.iter().for_each(|s| s.parts(out));
            }
        }
    }
}

/// A field from a closure (posed parts, combinations in the checks).
pub struct FnField<F: Fn(P3) -> f64 + Sync>(pub F);

impl<F: Fn(P3) -> f64 + Sync> Sdf3 for FnField<F> {
    fn distance(&self, p: Vec3d) -> f64 {
        (self.0)(p3(p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frustum_matches_csg_sdf_cylinder() {
        let f = Prim::Frustum { a: [1.0, 2.0, 3.0], b: [4.0, -1.0, 5.0], ra: 2.0, rb: 0.5 };
        let n = f.to_node();
        for p in [[0.0, 0.0, 0.0], [2.0, 1.0, 4.0], [5.0, 5.0, 5.0], [1.0, 2.5, 3.0], [-3.0, 1.0, 2.0]] {
            let a = f.distance(p);
            let b = makepad_csg_sdf::distance(&n, v3(p));
            assert!((a - b).abs() < 1e-9, "{p:?}: {a} vs {b}");
        }
    }

    #[test]
    fn slab_matches_node() {
        let s = Prim::Slab { n: norm([0.3, 1.0, -0.2]), lo: -1.0, hi: 2.5, c: [0.5, 0.0, 0.2], half: 3.0 };
        let n = s.to_node();
        for p in [[0.0, 0.0, 0.0], [0.0, 3.0, 0.0], [1.0, -4.0, 2.0], [4.0, 1.0, 0.0], [0.0, 0.5, -5.0]] {
            assert!((s.distance(p) - makepad_csg_sdf::distance(&n, v3(p))).abs() < 1e-9);
        }
    }

    #[test]
    fn euler_round_trip() {
        for d in [[0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.3, -0.5, 0.8], [0.0, -1.0, 0.0], [0.0, 1.0, 0.0]] {
            let r = rot_y_to(d);
            let back = makepad_csg_graph::eval::rotation_xyz(euler_xyz(&r));
            let y = [back[0][1], back[1][1], back[2][1]];
            let dn = norm(d);
            assert!(len(sub(y, dn)) < 1e-9, "{d:?} -> {y:?}");
        }
    }

    #[test]
    fn bounded_csg_matches_the_full_signed_expression() {
        let original = FnField(|p| len(p) - 2.0);
        let sphere = |c, r| Solid::Prim(Prim::Sphere { c, r });
        let solid = Solid::Inter(vec![
            Solid::Union(vec![Solid::Part(0), sphere([2.0, 0.0, 0.0], 1.0)]),
            Solid::diff(sphere([0.0; 3], 3.0), vec![sphere([0.0; 3], 0.5), sphere([0.0, 1.0, 0.0], 0.4)]),
        ]);
        let parts: &[&(dyn Sdf3 + Sync)] = &[&original];
        for p in [[0.0; 3], [0.3, 0.2, 0.1], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0], [3.1, 0.0, 0.0]] {
            for (lo, hi) in [(f64::NEG_INFINITY, f64::INFINITY), (-0.2, 0.2), (-0.4, -0.1), (0.01, 0.04)] {
                assert_eq!(solid.distance_bounded(parts, p, lo, hi, &mut Hints::default()), solid.distance(parts, p).clamp(lo, hi), "{p:?}, {lo}..{hi}");
            }
        }
    }
}
