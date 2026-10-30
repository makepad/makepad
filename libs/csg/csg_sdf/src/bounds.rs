//! Where an [`SdfNode`]'s solid can be, for a renderer or a slicer that
//! skips the nodes a region does not reach:
//!
//! - [`node_bounds`]: a box holding the node's solid (`f <= 0`), smooth
//!   blends included (a blend of width `k` reaches `k / 4` beyond the hard
//!   result); `None` for a node without bounds (a kernel, a gyroid);
//! - [`cull`]: the node reduced to what can matter inside a region (a layer
//!   slab, a brick), with the same sign and zero set there;
//! - [`changed`]: where an edit can change the surface, from the old and the
//!   new node (dirty layers, brick re-bakes).
//!
//! Every analytic node is 1-Lipschitz (the ellipsoid: a bound), so these hold
//! for the field the slicer samples.

use crate::shape::{SdfDeform, SdfNode, SdfOp, P3};

/// An axis-aligned box `(min, max)`.
pub type Bounds = (P3, P3);

fn union(a: Bounds, b: Bounds) -> Bounds {
    (std::array::from_fn(|i| a.0[i].min(b.0[i])), std::array::from_fn(|i| a.1[i].max(b.1[i])))
}

fn grow(b: Bounds, r: f64) -> Bounds {
    (b.0.map(|x| x - r), b.1.map(|x| x + r))
}

fn around(h: P3) -> Bounds {
    (h.map(|x| -x), h)
}

pub fn overlaps(a: &Bounds, b: &Bounds) -> bool {
    (0..3).all(|i| a.0[i] <= b.1[i] && b.0[i] <= a.1[i])
}

/// The box round `b`'s corners mapped by `f`.
pub fn map_bounds(b: &Bounds, f: impl Fn(P3) -> P3) -> Bounds {
    let corner = |i: usize| f([if i & 1 == 0 { b.0[0] } else { b.1[0] }, if i & 2 == 0 { b.0[1] } else { b.1[1] }, if i & 4 == 0 { b.0[2] } else { b.1[2] }]);
    (1..8).fold((corner(0), corner(0)), |acc, i| {
        let p = corner(i);
        union(acc, (p, p))
    })
}

/// The largest amount a smooth blend of width `blend` lies below the hard
/// minimum: a quarter of the width.
pub fn blend_reach(blend: f64) -> f64 {
    blend.max(0.0) * 0.25
}

fn rot(deg: P3) -> [[f64; 3]; 3] {
    let (sx, cx) = deg[0].to_radians().sin_cos();
    let (sy, cy) = deg[1].to_radians().sin_cos();
    let (sz, cz) = deg[2].to_radians().sin_cos();
    let rx = [[1.0, 0.0, 0.0], [0.0, cx, -sx], [0.0, sx, cx]];
    let ry = [[cy, 0.0, sy], [0.0, 1.0, 0.0], [-sy, 0.0, cy]];
    let rz = [[cz, -sz, 0.0], [sz, cz, 0.0], [0.0, 0.0, 1.0]];
    let m = |a: [[f64; 3]; 3], b: [[f64; 3]; 3]| -> [[f64; 3]; 3] { std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| a[i][k] * b[k][j]).sum())) };
    m(rz, m(ry, rx))
}

/// `p` of a transform's inner node into the outer frame.
fn out_of(at: P3, rotate: P3, scale: f64) -> impl Fn(P3) -> P3 {
    let r = rot(rotate);
    move |q: P3| std::array::from_fn(|i| (0..3).map(|k| r[i][k] * q[k] * scale).sum::<f64>() + at[i])
}

/// `p` of the outer frame into a transform's inner node.
fn into(at: P3, rotate: P3, scale: f64) -> impl Fn(P3) -> P3 {
    let r = rot(rotate);
    move |p: P3| {
        let d = [p[0] - at[0], p[1] - at[1], p[2] - at[2]];
        std::array::from_fn(|i| (0..3).map(|k| r[k][i] * d[k]).sum::<f64>() / scale)
    }
}

/// A box holding the node's solid, or `None` when it has none.
pub fn node_bounds(n: &SdfNode) -> Option<Bounds> {
    Some(match n {
        SdfNode::Sphere { radius } => around([*radius; 3]),
        SdfNode::Box { size, .. } => around(size.map(|s| s * 0.5)),
        SdfNode::Cylinder { radius, top_radius, height } => {
            let r = radius.max(*top_radius);
            around([r, height * 0.5, r])
        }
        SdfNode::Torus { radius, tube } => around([radius + tube, *tube, radius + tube]),
        SdfNode::Capsule { a, b, radius } => grow(union((*a, *a), (*b, *b)), *radius),
        SdfNode::Ellipsoid { radii } => around(*radii),
        SdfNode::Gyroid { .. } | SdfNode::Kernel(_) => return None,
        SdfNode::Mesh(m) => (m.min.map(|x| x as f64), m.max.map(|x| x as f64)),
        SdfNode::Combine { op, blend, of } => {
            let b = match op {
                SdfOp::Union => of.iter().map(node_bounds).try_fold(None, |acc: Option<Bounds>, b| b.map(|b| Some(acc.map_or(b, |a| union(a, b)))))?.expect("a combine has operands"),
                // Inside the first operand; an intersection inside any bounded one.
                SdfOp::Difference => node_bounds(&of[0])?,
                SdfOp::Intersection => of.iter().filter_map(node_bounds).reduce(|a, b| (std::array::from_fn(|i| a.0[i].max(b.0[i])), std::array::from_fn(|i| a.1[i].min(b.1[i]))))?,
            };
            grow(b, blend_reach(*blend))
        }
        SdfNode::Shell { thickness, of } => grow(node_bounds(of)?, *thickness),
        SdfNode::Round { radius, of } => grow(node_bounds(of)?, *radius),
        SdfNode::Transform { at, rotate, scale, of } => map_bounds(&node_bounds(of)?, out_of(*at, *rotate, *scale)),
        SdfNode::Material { of, .. } | SdfNode::Bone { of, .. } | SdfNode::Id { of, .. } => node_bounds(of)?,
        SdfNode::Instance { places, of } => {
            let b = node_bounds(of)?;
            places.iter().map(|pl| map_bounds(&b, |q| pl.out_of(q))).reduce(union)?
        }
        SdfNode::Deform { deform, of, .. } => deform_bounds(deform, &node_bounds(of)?),
    })
}

/// A box holding a warped node whose own box is `b`.
pub fn deform_bounds(deform: &SdfDeform, b: &Bounds) -> Bounds {
    let ext = |i: usize| b.0[i].abs().max(b.1[i].abs());
    match *deform {
        // Turns keep the distance from the axis.
        SdfDeform::Twist { .. } => {
            let r = ext(0).hypot(ext(2));
            ([-r, b.0[1], -r], [r, b.1[1], r])
        }
        SdfDeform::Bend { .. } => {
            let r = ext(0).hypot(ext(1));
            ([-r, -r, b.0[2]], [r, r, b.1[2]])
        }
        SdfDeform::Taper { rate } => {
            let s = (1.0 + rate * b.0[1]).abs().max((1.0 + rate * b.1[1]).abs());
            ([-ext(0) * s, b.0[1], -ext(2) * s], [ext(0) * s, b.1[1], ext(2) * s])
        }
        SdfDeform::Mirror { axis } => {
            let a = axis.min(2) as usize;
            let e = b.1[a].max(0.0);
            let (mut lo, mut hi) = *b;
            lo[a] = -e;
            hi[a] = e;
            (lo, hi)
        }
    }
}

fn reaches(n: &SdfNode, region: &Bounds) -> bool {
    node_bounds(n).map_or(true, |b| overlaps(&b, region))
}

/// The node reduced to what can matter inside `region`: union operands whose
/// solid misses the region grown by the blend width, and difference cutters
/// that miss it, are dropped. Inside the region the result has the same sign
/// everywhere and the same zero set (values away from the surface may
/// differ). `None`: no solid reaches the region.
pub fn cull(n: &SdfNode, region: &Bounds) -> Option<SdfNode> {
    if !reaches(n, region) {
        return None;
    }
    Some(match n {
        SdfNode::Combine { op, blend, of } => {
            let near = grow(*region, blend.max(0.0));
            match op {
                SdfOp::Union => {
                    let kept: Vec<SdfNode> = of.iter().filter_map(|c| cull(c, &near)).collect();
                    match kept.len() {
                        0 => return None,
                        1 if *blend <= 0.0 => kept.into_iter().next().unwrap(),
                        _ => SdfNode::Combine { op: *op, blend: *blend, of: kept },
                    }
                }
                SdfOp::Difference => {
                    let mut kept = vec![cull(&of[0], region)?];
                    kept.extend(of[1..].iter().filter_map(|c| cull(c, &near)));
                    if kept.len() == 1 {
                        kept.pop().unwrap()
                    } else {
                        SdfNode::Combine { op: *op, blend: *blend, of: kept }
                    }
                }
                // Every operand bounds an intersection: none can be dropped.
                SdfOp::Intersection => SdfNode::Combine { op: *op, blend: *blend, of: of.iter().map(|c| cull(c, &near)).collect::<Option<Vec<_>>>()? },
            }
        }
        SdfNode::Shell { thickness, of } => SdfNode::Shell { thickness: *thickness, of: Box::new(cull(of, &grow(*region, *thickness))?) },
        SdfNode::Round { radius, of } => SdfNode::Round { radius: *radius, of: Box::new(cull(of, &grow(*region, *radius))?) },
        SdfNode::Transform { at, rotate, scale, of } => {
            let inner = map_bounds(region, into(*at, *rotate, *scale));
            SdfNode::Transform { at: *at, rotate: *rotate, scale: *scale, of: Box::new(cull(of, &inner)?) }
        }
        SdfNode::Material { slot, within, of } => SdfNode::Material { slot: *slot, within: within.clone(), of: Box::new(cull(of, region)?) },
        SdfNode::Id { id, of } => SdfNode::Id { id: *id, of: Box::new(cull(of, region)?) },
        SdfNode::Bone { id, rest, of } => SdfNode::Bone { id: *id, rest: *rest, of: Box::new(cull(of, region)?) },
        // The copies that reach the region; `of` stays whole (one key for every copy).
        SdfNode::Instance { places, of } => {
            let b = node_bounds(of);
            let kept: Vec<_> = places.iter().filter(|pl| b.map_or(true, |b| overlaps(&map_bounds(&b, |q| pl.out_of(q)), region))).copied().collect();
            if kept.is_empty() {
                return None;
            }
            SdfNode::Instance { places: kept, of: of.clone() }
        }
        // A warp's preimage of a box is no box: kept whole.
        leaf => leaf.clone(),
    })
}

/// Structural equality, kernels by their key.
pub fn same(a: &SdfNode, b: &SdfNode) -> bool {
    use SdfNode::*;
    match (a, b) {
        (Sphere { radius: x }, Sphere { radius: y }) => x == y,
        (Box { size: s, radius: r }, Box { size: t, radius: q }) => s == t && r == q,
        (Cylinder { radius: a1, top_radius: a2, height: a3 }, Cylinder { radius: b1, top_radius: b2, height: b3 }) => a1 == b1 && a2 == b2 && a3 == b3,
        (Torus { radius: a1, tube: a2 }, Torus { radius: b1, tube: b2 }) => a1 == b1 && a2 == b2,
        (Capsule { a: a1, b: a2, radius: a3 }, Capsule { a: b1, b: b2, radius: b3 }) => a1 == b1 && a2 == b2 && a3 == b3,
        (Ellipsoid { radii: x }, Ellipsoid { radii: y }) => x == y,
        (Gyroid { period: a1, wall: a2 }, Gyroid { period: b1, wall: b2 }) => a1 == b1 && a2 == b2,
        (Combine { op: o1, blend: k1, of: c1 }, Combine { op: o2, blend: k2, of: c2 }) => o1 == o2 && k1 == k2 && c1.len() == c2.len() && c1.iter().zip(c2).all(|(x, y)| same(x, y)),
        (Shell { thickness: t1, of: x }, Shell { thickness: t2, of: y }) => t1 == t2 && same(x, y),
        (Round { radius: r1, of: x }, Round { radius: r2, of: y }) => r1 == r2 && same(x, y),
        (Transform { at: a1, rotate: r1, scale: s1, of: x }, Transform { at: a2, rotate: r2, scale: s2, of: y }) => a1 == a2 && r1 == r2 && s1 == s2 && same(x, y),
        (Kernel(x), Kernel(y)) => x.key == y.key,
        (Mesh(x), Mesh(y)) => x.key() == y.key(),
        (Material { slot: s1, within: w1, of: x }, Material { slot: s2, within: w2, of: y }) => {
            s1 == s2 && same(x, y) && match (w1, w2) { (None, None) => true, (Some(a), Some(b)) => same(a, b), _ => false }
        }
        (Id { id: i1, of: x }, Id { id: i2, of: y }) => i1 == i2 && same(x, y),
        (Bone { id: i1, rest: r1, of: x }, Bone { id: i2, rest: r2, of: y }) => i1 == i2 && r1 == r2 && same(x, y),
        (Instance { places: p1, of: x }, Instance { places: p2, of: y }) => p1 == p2 && same(x, y),
        (Deform { deform: d1, lipschitz: l1, of: x }, Deform { deform: d2, lipschitz: l2, of: y }) => d1 == d2 && l1 == l2 && same(x, y),
        _ => false,
    }
}

/// Where the surface of `new` can differ from the surface of `old`: the
/// bounds of every changed subtree, old and new, grown by the blends,
/// shells and roundings above it and placed into the root frame. `None`
/// when the nodes are the same; an unbounded change is `Some` of an
/// infinite box.
pub fn changed(old: &SdfNode, new: &SdfNode) -> Option<Bounds> {
    if same(old, new) {
        return None;
    }
    let everywhere = ([f64::NEG_INFINITY; 3], [f64::INFINITY; 3]);
    let whole = || match (node_bounds(old), node_bounds(new)) {
        (Some(a), Some(b)) => union(a, b),
        _ => everywhere,
    };
    Some(match (old, new) {
        (SdfNode::Combine { op: oa, blend: ba, of: ca }, SdfNode::Combine { op: ob, blend: bb, of: cb }) if oa == ob && ba == bb && ca.len() == cb.len() => {
            let mut out: Option<Bounds> = None;
            for (a, b) in ca.iter().zip(cb) {
                if let Some(c) = changed(a, b) {
                    let c = grow(c, ba.max(0.0));
                    out = Some(out.map_or(c, |o| union(o, c)));
                }
            }
            out?
        }
        (SdfNode::Shell { thickness: ta, of: a }, SdfNode::Shell { thickness: tb, of: b }) if ta == tb => grow(changed(a, b)?, *ta),
        (SdfNode::Round { radius: ra, of: a }, SdfNode::Round { radius: rb, of: b }) if ra == rb => grow(changed(a, b)?, *ra),
        (SdfNode::Transform { at: aa, rotate: ra, scale: sa, of: a }, SdfNode::Transform { at: ab, rotate: rb, scale: sb, of: b }) if aa == ab && ra == rb && sa == sb => {
            map_bounds(&changed(a, b)?, out_of(*aa, *ra, *sa))
        }
        // A slot change is a material change on the same surface.
        (SdfNode::Material { of: a, .. }, SdfNode::Material { of: b, .. }) | (SdfNode::Id { of: a, .. }, SdfNode::Id { of: b, .. }) => changed(a, b).unwrap_or_else(whole),
        (SdfNode::Bone { id: ia, rest: ra, of: a }, SdfNode::Bone { id: ib, rest: rb, of: b }) if ia == ib && ra == rb => changed(a, b)?,
        (SdfNode::Instance { places: pa, of: a }, SdfNode::Instance { places: pb, of: b }) if same(a, b) => {
            // Only the moved, added or removed copies.
            let ob = node_bounds(a).unwrap_or(everywhere);
            let moved = |x: &[crate::shape::SdfPlace], y: &[crate::shape::SdfPlace]| x.iter().filter(|p| !y.contains(p)).map(|pl| map_bounds(&ob, |q| pl.out_of(q))).collect::<Vec<_>>();
            moved(pa, pb).into_iter().chain(moved(pb, pa)).reduce(union)?
        }
        _ => whole(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shape::distance;
    use makepad_csg_math::Vec3d;

    fn at(p: P3, of: SdfNode) -> SdfNode {
        SdfNode::Transform { at: p, rotate: [0.0; 3], scale: 1.0, of: Box::new(of) }
    }

    /// A blended union, a shell and a cut: a small vase.
    fn vase(neck: f64) -> SdfNode {
        let body = SdfNode::Combine {
            op: SdfOp::Union,
            blend: 4.0,
            of: vec![at([0.0, 10.0, 0.0], SdfNode::Sphere { radius: 10.0 }), at([0.0, 25.0, 0.0], SdfNode::Cylinder { radius: neck, top_radius: neck, height: 16.0 })],
        };
        SdfNode::Combine {
            op: SdfOp::Difference,
            blend: 0.0,
            of: vec![SdfNode::Shell { thickness: 1.0, of: Box::new(body) }, at([0.0, 34.0, 0.0], SdfNode::Cylinder { radius: 3.0, top_radius: 3.0, height: 8.0 })],
        }
    }

    fn grid(b: &Bounds, n: usize) -> Vec<Vec3d> {
        let t = |i: usize, s: usize| b.0[i] + (b.1[i] - b.0[i]) * s as f64 / n as f64;
        let mut out = Vec::new();
        for i in 0..=n {
            for j in 0..=n {
                for k in 0..=n {
                    out.push(Vec3d::new(t(0, i), t(1, j), t(2, k)));
                }
            }
        }
        out
    }

    fn inside(b: &Bounds, p: Vec3d) -> bool {
        p.x >= b.0[0] && p.x <= b.1[0] && p.y >= b.0[1] && p.y <= b.1[1] && p.z >= b.0[2] && p.z <= b.1[2]
    }

    #[test]
    fn bounds_hold_the_solid() {
        let n = vase(4.0);
        let b = node_bounds(&n).unwrap();
        for p in grid(&grow(b, 10.0), 40) {
            if distance(&n, p) <= 0.0 {
                assert!(inside(&b, p), "{p:?} outside {b:?}");
            }
        }
    }

    #[test]
    fn culled_node_keeps_the_sign() {
        let n = vase(4.0);
        let b = node_bounds(&n).unwrap();
        for y in [1.0, 9.0, 20.0, 33.0, 37.0] {
            let slab = ([b.0[0], y - 0.1, b.0[2]], [b.1[0], y + 0.1, b.1[2]]);
            let c = cull(&n, &slab);
            for p in grid(&slab, 24) {
                let d = distance(&n, p);
                match &c {
                    None => assert!(d > 0.0),
                    Some(c) => assert_eq!(distance(c, p) <= 0.0, d <= 0.0, "{p:?}"),
                }
            }
        }
    }

    #[test]
    fn changed_region_holds_the_change() {
        let (a, b) = (vase(4.0), vase(5.0));
        assert!(changed(&a, &vase(4.0)).is_none());
        let r = changed(&a, &b).expect("the neck changed");
        for p in grid(&grow(union(node_bounds(&a).unwrap(), node_bounds(&b).unwrap()), 2.0), 40) {
            if (distance(&a, p) <= 0.0) != (distance(&b, p) <= 0.0) {
                assert!(inside(&r, p), "{p:?} changed outside {r:?}");
            }
        }
        assert!(r.0[1] > 0.0, "the foot is untouched: {r:?}");
    }

    /// Dual contouring of the vase is a closed, consistently wound surface
    /// at fine and coarse cells (a coarse cell spans its thin wall).
    #[test]
    fn meshes_closed() {
        let n = vase(4.0);
        let b = node_bounds(&n).unwrap();
        let shape = crate::shape::SdfShape { root: n, min: b.0, max: b.1, materials: Vec::new() };
        for cell in [0.3, 0.6, 1.2, 2.4] {
            let m = crate::field_mesh::mesh_field(shape.field(), Vec3d::new(b.0[0], b.0[1], b.0[2]), Vec3d::new(b.1[0], b.1[1], b.1[2]), cell);
            let mut net = std::collections::HashMap::new();
            for t in &m.triangles {
                for i in 0..3 {
                    let (a, c) = (t[i], t[(i + 1) % 3]);
                    *net.entry((a.min(c), a.max(c))).or_insert(0i32) += if a < c { 1 } else { -1 };
                }
            }
            assert_eq!(net.values().filter(|v| **v != 0).count(), 0, "cell {cell}: {} triangles", m.triangles.len());
        }
    }
}
