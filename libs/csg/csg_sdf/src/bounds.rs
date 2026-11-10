//! Where an [`SdfNode`]'s solid can be, for a renderer or a slicer that
//! skips the nodes a region does not reach:
//!
//! - [`node_bounds`]: a box holding the node's solid (`f <= 0`), smooth
//!   blends included (a blend of width `k` reaches `k / 4` beyond the hard
//!   result); `None` for a node without bounds (a gyroid, a kernel with no
//!   declared box);
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

pub(crate) fn grow(b: Bounds, r: f64) -> Bounds {
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

/// How far an operand of a smooth fold of `n` operands can reach the
/// result: the fold is left to right (`smin(smin(a, b), c)`, not
/// associative), so an operand changes the blend of its neighbours, and that
/// change carries on down the fold. An operand passes through at most
/// `n - 1` of the fold's steps (the first two through all of them), and a
/// step's result moves with an operand only where the two differ by less
/// than `1.25·blend` (the blend's width, and its sag of at most `blend / 4`;
/// past the width the step is the other operand, bit for bit). So where the
/// result is `v`, an operand changes it only where its own value is within
/// `|v| + 1.25·blend·(n - 1)`: an exact field changed only inside a region
/// changes the surface (`v` = 0) no farther than this reach of it, in any
/// order of the fold. A hard fold (`blend` 0) reaches nothing.
pub fn fold_reach(blend: f64, n: usize) -> f64 {
    if blend <= 0.0 {
        0.0
    } else {
        1.25 * blend * n.saturating_sub(1) as f64
    }
}

pub(crate) fn rot(deg: P3) -> [[f64; 3]; 3] {
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
pub(crate) fn out_of(at: P3, rotate: P3, scale: f64) -> impl Fn(P3) -> P3 {
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

/// A box holding the node's solid, or `None` when it has none. It holds by construction, from
/// each node's own formula (see [`reach`]): a blend's sag, a shell's or a rounding's growth, a
/// warp's stretch and a sculpt's raised tubes included.
pub fn node_bounds(n: &SdfNode) -> Option<Bounds> {
    reach(n, 0.0, false).map(|(b, m)| grow(b, m))
}

/// [`node_bounds`] narrowed where the tree proves more, at more cost (a union asks its operands
/// three times, so nested unions multiply): a blend sags only where two of its operands come
/// within its reach, so a side one operand alone holds keeps that operand's; a
/// difference by a box (an exact one, unturned) that spans a side of what it cuts moves that side
/// to the box (a cut foot is flat on the cutter's face).
pub fn tight_bounds(n: &SdfNode) -> Option<Bounds> {
    reach(n, 0.0, true).map(|(b, m)| grow(b, m))
}

fn aabb((b, m): (Bounds, f64)) -> Bounds {
    grow(b, m)
}

pub(crate) fn meet(a: &Bounds, b: &Bounds) -> Option<Bounds> {
    let lo: P3 = std::array::from_fn(|k| a.0[k].max(b.0[k]));
    let hi: P3 = std::array::from_fn(|k| a.1[k].min(b.1[k]));
    (0..3).all(|k| lo[k] <= hi[k]).then_some((lo, hi))
}

/// The box of an exact box node (sharp corners, under unturned frames): None for any other.
fn exact_box(n: &SdfNode) -> Option<Bounds> {
    match n {
        SdfNode::Box { size, radius } if *radius == 0.0 => Some(around(size.map(|s| s * 0.5))),
        SdfNode::Transform { at, rotate, scale, of } if *rotate == [0.0; 3] && *scale > 0.0 => {
            exact_box(of).map(|(lo, hi)| (std::array::from_fn(|k| lo[k] * scale + at[k]), std::array::from_fn(|k| hi[k] * scale + at[k])))
        }
        SdfNode::Id { of, .. } | SdfNode::Material { of, .. } | SdfNode::Bone { of, .. } => exact_box(of),
        _ => None,
    }
}

/// `b` less what box `cut` takes when it spans `b` across two axes and covers one end of the
/// third (a cut that would take all of `b` leaves it). It spans past `b`'s faces, so all it takes
/// is inside it, none on its faces.
fn cut_by(mut b: Bounds, cut: &Bounds) -> Bounds {
    for i in 0..3 {
        let spans = (0..3).filter(|&j| j != i).all(|j| cut.0[j] < b.0[j] && cut.1[j] > b.1[j]);
        if !spans {
            continue;
        }
        if cut.0[i] < b.0[i] && cut.1[i] > b.0[i] && cut.1[i] < b.1[i] {
            b.0[i] = cut.1[i];
        } else if cut.1[i] > b.1[i] && cut.0[i] < b.1[i] && cut.0[i] > b.0[i] {
            b.1[i] = cut.0[i];
        }
    }
    b
}

/// Where a node's value is at most `delta` (`delta = 0`: its solid): every such point lies within
/// `m` of the box `b` (`Some((b, m))`), or anywhere (`None`). A blend's operands are asked for
/// its sag on top of the blend's own `delta`, a shell's and a rounding's operand for its
/// thickness on top; a scaled frame divides it, a warp multiplies it by its Lipschitz bound.
/// The margin `m` is a ball's, so it turns with its box: a distance's level set at `delta` is its
/// solid grown by `delta` in every direction. A field that is no distance (an ellipsoid's bound,
/// a warp, a texture) folds its growth into its box. A kernel's value counts as a distance
/// outside its declared box. `tight`: [`tight_bounds`]'s rules too.
fn reach(n: &SdfNode, delta: f64, tight: bool) -> Option<(Bounds, f64)> {
    let delta = delta.max(0.0);
    Some(match n {
        SdfNode::Sphere { radius } => (around([*radius; 3]), delta),
        SdfNode::Box { size, .. } => (around(size.map(|s| s * 0.5)), delta),
        SdfNode::Cylinder { radius, top_radius, height } => {
            let r = radius.max(*top_radius);
            (around([r, height * 0.5, r]), delta)
        }
        SdfNode::Torus { radius, tube } => (around([radius + tube, *tube, radius + tube]), delta),
        SdfNode::Capsule { a, b, radius } => (grow(union((*a, *a), (*b, *b)), *radius), delta),
        // k0 (k0 - 1) / k1 with k1 <= k0 / min(r): at most `delta` only where
        // k0 <= 1 + delta / min(r), the ellipsoid scaled by that
        SdfNode::Ellipsoid { radii } => {
            let rmin = radii.iter().fold(f64::INFINITY, |a, &r| a.min(r.abs())).max(1e-12);
            (around(radii.map(|r| r.abs() * (1.0 + delta / rmin))), 0.0)
        }
        SdfNode::Kernel(k) => (k.bounds?, delta),
        SdfNode::Gyroid { .. } => return None,
        SdfNode::Mesh(m) => ((m.min.map(|x| x as f64), m.max.map(|x| x as f64)), delta),
        SdfNode::Links(l) => ((l.min, l.max), delta),
        SdfNode::Tubes(t) => ((t.min, t.max), delta),
        SdfNode::Sketch(s) => ((s.min, s.max), delta),
        SdfNode::Combine { op, blend, of } => match op {
            // Each of the n - 1 smooth steps sags at most a blend reach below the least operand.
            SdfOp::Union => {
                let d = delta + blend_reach(*blend) * of.len().saturating_sub(1) as f64;
                let all = of.iter().map(|c| reach(c, d, tight)).try_fold(None, |acc: Option<(Bounds, f64)>, r| r.map(|(b, m)| Some(acc.map_or((b, m), |(a, k)| (union(a, b), k.max(m))))))?.expect("a combine has operands");
                if !(tight && of.len() > 1) {
                    all
                } else {
                    // A step smin(s, x) of the fold is under delta with both over it only where
                    // they differ by under the blend and the less is under delta + blend / 4: both
                    // under delta + 5/4 blend. Its fold side s is then under that and the steps
                    // before it sag at most a quarter blend each, so an operand there is under
                    // `far`: a point is in an operand's box at delta (no step sags there, or the
                    // last that does is overtaken) or in two operands' boxes at `far`.
                    let far = delta + 1.25 * blend + blend_reach(*blend) * of.len().saturating_sub(2) as f64;
                    let alone = of.iter().map(|c| reach(c, delta, tight).map(aabb)).reduce(|a, b| Some(union(a?, b?))).expect("a combine has operands")?;
                    let near: Vec<Bounds> = if *blend > 0.0 { of.iter().map(|c| reach(c, far, tight).map(aabb)).collect::<Option<_>>()? } else { Vec::new() };
                    let sag = (0..near.len()).flat_map(|i| (i + 1..near.len()).map(move |j| (i, j))).filter_map(|(i, j)| meet(&near[i], &near[j])).fold(alone, union);
                    match meet(&aabb(all), &sag) {
                        Some(b) => (b, 0.0),
                        None => all,
                    }
                }
            }
            // A smooth max never lies below the hard one: inside the first operand; an
            // intersection inside every bounded one. A cutter is at least `delta` deep only
            // inside its box shrunk by `delta`, which the cut leaves out.
            SdfOp::Difference => {
                let first = reach(&of[0], delta, tight)?;
                let cuts: Vec<Bounds> = if tight { of[1..].iter().filter_map(exact_box).map(|b| grow(b, -delta)).collect() } else { Vec::new() };
                if cuts.is_empty() {
                    first
                } else {
                    (cuts.iter().fold(aabb(first), |b, c| cut_by(b, c)), 0.0)
                }
            }
            SdfOp::Intersection => (of.iter().filter_map(|c| reach(c, delta, tight)).map(aabb).reduce(|a, b| (std::array::from_fn(|i| a.0[i].max(b.0[i])), std::array::from_fn(|i| a.1[i].min(b.1[i]))))?, 0.0),
        },
        SdfNode::Shell { thickness, of } => reach(of, thickness.abs() + delta, tight)?,
        SdfNode::Round { radius, of } => reach(of, radius + delta, tight)?,
        SdfNode::Transform { at, rotate, scale, of } => {
            let (b, m) = reach(of, delta / scale, tight)?;
            (map_bounds(&b, out_of(*at, *rotate, *scale)), m * scale)
        }
        SdfNode::Material { of, .. } | SdfNode::Bone { of, .. } | SdfNode::Id { of, .. } => reach(of, delta, tight)?,
        SdfNode::Instance { places, of } => {
            // one reach per scale the copies use (most share one)
            let mut by_scale: Vec<(f64, (Bounds, f64))> = Vec::new();
            let mut out: Option<(Bounds, f64)> = None;
            for pl in places {
                let (b, m) = match by_scale.iter().find(|(s, _)| *s == pl.scale) {
                    Some((_, r)) => *r,
                    None => {
                        let r = reach(of, delta / pl.scale, tight)?;
                        by_scale.push((pl.scale, r));
                        r
                    }
                };
                let placed = (map_bounds(&b, |q| pl.out_of(q)), m * pl.scale);
                out = Some(out.map_or(placed, |(a, k)| (union(a, placed.0), k.max(placed.1))));
            }
            out?
        }
        SdfNode::Deform { deform, lipschitz, of } => (deform_bounds(deform, &aabb(reach(of, delta * lipschitz, tight)?)), 0.0),
        SdfNode::Texture { tex, of } => (crate::texture::texture_bounds(tex, aabb(reach(of, delta * tex.lipschitz, tight)?)), 0.0),
        // Raised tubes add to the solid (lowered ones only take away): where a raise changes
        // the solid its tube is under its blend (its capsule's box grown by its reach). The
        // brick floor bounds the value by at least a brick.
        SdfNode::Sculpt { strokes, of } => {
            let base = reach(of, delta, tight)?;
            match strokes.raised_bounds() {
                Some(_) if delta >= crate::strokes::BRICK => return None,
                Some(s) => (union(base.0, s), base.1.max(delta)),
                None => base,
            }
        }
    })
}

/// A sculpt culled to `region`: `of` culled there, then each shape stroke that reaches it, in
/// order, as a Combine of `of` so far and the stroke's tube (the sharp union of its capsules
/// there). `None`: no solid reaches the region.
pub fn sculpt_in(strokes: &crate::strokes::Strokes, of: &SdfNode, region: &Bounds) -> Option<SdfNode> {
    sculpt_on(strokes, cull(of, region), region)
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
        SdfDeform::Stretch { scale } => (std::array::from_fn(|i| b.0[i] * scale[i]), std::array::from_fn(|i| b.1[i] * scale[i])),
    }
}

/// The box [`SdfDeform::warp`] takes box `b` into: where a deformed node reads its source for the
/// points of `b`.
fn warp_bounds(deform: &SdfDeform, b: &Bounds) -> Bounds {
    let ext = |i: usize| b.0[i].abs().max(b.1[i].abs());
    match *deform {
        // turns about an axis keep the distance from it and the height along it
        SdfDeform::Twist { .. } => {
            let r = ext(0).hypot(ext(2));
            ([-r, b.0[1], -r], [r, b.1[1], r])
        }
        SdfDeform::Bend { .. } => {
            let r = ext(0).hypot(ext(1));
            ([-r, -r, b.0[2]], [r, r, b.1[2]])
        }
        // x and z times k = 1 / max(1 + rate y, TAPER_MIN), which is monotone in y: its least
        // and most are at the box's two heights, and a product of ranges is at their ends
        SdfDeform::Taper { rate } => {
            let k = |y: f64| 1.0 / (1.0 + rate * y).max(crate::shape::TAPER_MIN);
            let (k0, k1) = (k(b.0[1]).min(k(b.1[1])), k(b.0[1]).max(k(b.1[1])));
            let scaled = |i: usize| {
                let c = [b.0[i] * k0, b.0[i] * k1, b.1[i] * k0, b.1[i] * k1];
                (c.iter().fold(f64::INFINITY, |a, &v| a.min(v)), c.iter().fold(f64::NEG_INFINITY, |a, &v| a.max(v)))
            };
            let ((x0, x1), (z0, z1)) = (scaled(0), scaled(2));
            ([x0, b.0[1], z0], [x1, b.1[1], z1])
        }
        SdfDeform::Mirror { axis } => {
            let a = axis.min(2) as usize;
            let (least, most) = abs_range(b.0[a], b.1[a]);
            let (mut lo, mut hi) = *b;
            lo[a] = least;
            hi[a] = most;
            (lo, hi)
        }
        SdfDeform::Stretch { scale } => (std::array::from_fn(|i| b.0[i] / scale[i]), std::array::from_fn(|i| b.1[i] / scale[i])),
    }
}

/// The node reduced to what makes its value inside `region`: the same value at every point of
/// it, bit for bit ([`cut`]). `None`: no solid reaches the region (the value is positive all
/// over it).
pub fn cull(n: &SdfNode, region: &Bounds) -> Option<SdfNode> {
    let c = cut(n, region);
    (c.lo <= 0.0).then_some(c.node)
}

/// [`cull`] makes the same node of `n` in region `a` as in region `b`.
pub fn culls_alike(n: &SdfNode, a: &Bounds, b: &Bounds) -> bool {
    match (cull(n, a), cull(n, b)) {
        (None, None) => true,
        (Some(x), Some(y)) => same(&x, &y),
        _ => false,
    }
}

/// A node reduced to a region, and what its value can be there.
pub struct Cut {
    /// The node with every operand, cutter and copy dropped that cannot change its value
    /// anywhere in the region: it has the same value at every point of the region, bit for bit.
    pub node: SdfNode,
    /// Its value over the region lies in `lo..=hi` (either may be infinite: no bound).
    pub lo: f64,
    pub hi: f64,
}

/// A region as a box and a ball holding it: the box bounds the culls of kernels and strokes,
/// the ball the values (a centre's value and a slope).
#[derive(Clone, Copy)]
struct Near {
    b: Bounds,
    c: P3,
    r: f64,
}

impl Near {
    fn of(b: &Bounds) -> Near {
        let c: P3 = std::array::from_fn(|k| 0.5 * (b.0[k] + b.1[k]));
        let r = (0..3).map(|k| (0.5 * (b.1[k] - b.0[k])).powi(2)).sum::<f64>().sqrt();
        Near { b: *b, c, r }
    }

    fn grown(&self, m: f64) -> Near {
        Near { b: grow(self.b, m), c: self.c, r: self.r + m }
    }

    /// Into a transform's frame (the ball's centre exactly as [`distance`](crate::distance) maps
    /// a point).
    fn into(&self, at: P3, rotate: P3, scale: f64) -> Near {
        let c = crate::shape::transform_into(at, rotate, scale, v3(self.c));
        Near { b: map_bounds(&self.b, into(at, rotate, scale)), c: [c.x, c.y, c.z], r: self.r / scale.abs() }
    }

    fn placed(&self, pl: &crate::shape::SdfPlace) -> Near {
        Near { b: map_bounds(&self.b, |p| pl.into(p)), c: pl.into(self.c), r: self.r / pl.scale.abs() }
    }

    /// The box `b` misses the ball.
    fn misses(&self, b: &Bounds) -> bool {
        box_distance(b, self.c) > self.r
    }
}

fn v3(p: P3) -> makepad_csg_math::Vec3d {
    makepad_csg_math::Vec3d::new(p[0], p[1], p[2])
}

/// How far `p` is from box `b` (0 inside).
fn box_distance(b: &Bounds, p: P3) -> f64 {
    (0..3).map(|k| (b.0[k] - p[k]).max(p[k] - b.1[k]).max(0.0).powi(2)).sum::<f64>().sqrt()
}

/// The least and the most `|x|` over `lo..=hi`.
fn abs_range(lo: f64, hi: f64) -> (f64, f64) {
    let least = if lo <= 0.0 && hi >= 0.0 { 0.0 } else { lo.abs().min(hi.abs()) };
    (least, lo.abs().max(hi.abs()))
}

/// How far apart two boxes are (0 where they meet).
fn boxes_apart(a: &Bounds, b: &Bounds) -> f64 {
    (0..3).map(|k| (a.0[k] - b.1[k]).max(b.0[k] - a.1[k]).max(0.0).powi(2)).sum::<f64>().sqrt()
}

/// A distance leaf's value over box `b` (its own frame), from its formula: each is a function
/// rising in the ranges of its point's coordinates' sizes (and of the distance from its axis),
/// so the least and the most of those give its least and most. None: a leaf this does not read.
fn leaf_range(n: &SdfNode, b: &Bounds) -> Option<(f64, f64)> {
    let ax: [(f64, f64); 3] = std::array::from_fn(|i| abs_range(b.0[i], b.1[i]));
    let len = |v: [f64; 3]| (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    // |max(q, 0)| + min(max q, 0): rises in each q
    let boxed = |q: [f64; 3]| len(q.map(|x| x.max(0.0))) + q[0].max(q[1]).max(q[2]).min(0.0);
    let ring = |lo: f64, hi: f64| -> (f64, f64) { (lo.max(0.0), hi) };
    let range = match n {
        SdfNode::Sphere { radius } => (len([ax[0].0, ax[1].0, ax[2].0]) - radius, len([ax[0].1, ax[1].1, ax[2].1]) - radius),
        SdfNode::Box { size, radius } => {
            let q = |pick: fn((f64, f64)) -> f64| -> [f64; 3] { std::array::from_fn(|i| pick(ax[i]) - size[i] * 0.5 + radius) };
            (boxed(q(|a| a.0)) - radius, boxed(q(|a| a.1)) - radius)
        }
        SdfNode::Cylinder { radius, top_radius, height } if radius == top_radius => {
            let (rxz_lo, rxz_hi) = ring((ax[0].0 * ax[0].0 + ax[2].0 * ax[2].0).sqrt(), (ax[0].1 * ax[0].1 + ax[2].1 * ax[2].1).sqrt());
            let f = |dx: f64, dy: f64| (dx.max(0.0) * dx.max(0.0) + dy.max(0.0) * dy.max(0.0)).sqrt() + dx.max(dy).min(0.0);
            (f(rxz_lo - radius, ax[1].0 - height * 0.5), f(rxz_hi - radius, ax[1].1 - height * 0.5))
        }
        SdfNode::Torus { radius, tube } => {
            let (rxz_lo, rxz_hi) = ((ax[0].0 * ax[0].0 + ax[2].0 * ax[2].0).sqrt(), (ax[0].1 * ax[0].1 + ax[2].1 * ax[2].1).sqrt());
            let q = abs_range(rxz_lo - radius, rxz_hi - radius);
            ((q.0 * q.0 + ax[1].0 * ax[1].0).sqrt() - tube, (q.1 * q.1 + ax[1].1 * ax[1].1).sqrt() - tube)
        }
        _ => return None,
    };
    // the formulas in floating point: a hair's room
    Some((range.0 - 1e-9 * (1.0 + range.0.abs()), range.1 + 1e-9 * (1.0 + range.1.abs())))
}

/// What a node's value is over a [`Near`] ball: `lo..=hi` everywhere in it, `at` at its centre and
/// at most `slope` times as steep as a distance there (infinite: no bound, and `at` unused).
struct Span {
    node: SdfNode,
    lo: f64,
    hi: f64,
    at: f64,
    slope: f64,
}

impl Span {
    /// A node whose value at the centre and slope bound it.
    fn by_slope(node: SdfNode, at: f64, slope: f64, r: f64) -> Span {
        Span { node, lo: at - slope * r, hi: at + slope * r, at, slope }
    }

    /// No bound but `lo`.
    fn unbounded(node: SdfNode, lo: f64) -> Span {
        Span { node, lo, hi: f64::INFINITY, at: f64::NAN, slope: f64::INFINITY }
    }

    /// The value times `scale` (a frame's: a negative one turns the field inside out).
    fn scaled(self, scale: f64) -> Span {
        let (lo, hi) = if scale >= 0.0 { (self.lo * scale, self.hi * scale) } else { (self.hi * scale, self.lo * scale) };
        Span { lo, hi, at: self.at * scale, ..self }
    }

    /// The bounds narrowed by the centre's value and the slope, where those are known.
    fn narrowed(mut self, r: f64) -> Span {
        if self.slope.is_finite() {
            self.lo = self.lo.max(self.at - self.slope * r);
            self.hi = self.hi.min(self.at + self.slope * r);
        }
        self
    }
}

/// How sure a decision is: bounds computed in floating point, and a field a renderer evaluates in
/// single precision, keep this much more than the blend to drop an operand.
fn slack(a: f64, b: f64, k: f64) -> f64 {
    1e-6 * (1.0 + a.abs() + b.abs() + k)
}

/// The node reduced to `region` with its value's bounds there. Exact: at every point of the
/// region the reduced node's value is the node's, bit for bit, as [`distance`](crate::distance)
/// computes it.
///
/// A smooth fold `a = smin(a, x, k)` (union; an intersection and a difference fold the negated
/// values the same way) leaves `a` as it is, bit for bit, where `x >= a + k`, and becomes `x`
/// exactly where `x <= a - k` ([`smin`](crate::shape::smin) returns the nearer operand outside
/// the blend). So an operand whose least value over the region is a blend above the fold's most
/// so far is dropped, and an operand whose most is a blend below the fold's least drops every
/// operand before it (a union's or an intersection's: a difference keeps its base). The rest of
/// the fold sees the same values either way. Each operand's bounds come from its value at the
/// region's centre and a slope bound (every engine node is at most as steep as a distance; an
/// ellipsoid's estimate by [`crate::lipschitz::ellipsoid_slope_near`] and its radii), or from its
/// declared box (a kernel's value counts as a distance outside it).
pub fn cut(n: &SdfNode, region: &Bounds) -> Cut {
    let g = Near::of(region);
    if !(g.r.is_finite() && g.c.iter().all(|c| c.is_finite())) {
        // everywhere: nothing can be dropped
        return Cut { node: n.clone(), lo: f64::NEG_INFINITY, hi: f64::INFINITY };
    }
    let s = span(n, &g);
    Cut { node: s.node, lo: s.lo, hi: s.hi }
}

/// How steep a node can be over a region, as many times as a distance: at the points where its
/// value is positive (outside its solid) and where it is negative (inside). Infinite: no bound
/// there, as where a sculpt makes the value, a kernel's values may lie past the box it is
/// measured over, a deform's stretch is not bounded, or deep inside an ellipsoid (its estimate
/// jumps at its centre).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Slopes {
    pub outside: f64,
    pub inside: f64,
}

/// `n`'s [`Slopes`] over `region` (its frame); a cut's node ([`cut`]) has the tree's there, for
/// less to read.
pub fn slopes(n: &SdfNode, region: &Bounds) -> Slopes {
    slopes_within(n, region, f64::INFINITY)
}

/// [`slopes`] at only the values within `reach` of 0: outside at `0..=reach`, inside at
/// `-reach..=0`. A value `v` falls to 0 along a line only through the values between, so it
/// stays clear of the surface for `|v|` over the slope at those, a window an ellipsoid's estimate
/// is barely steeper than a distance in near its surface.
pub fn slopes_within(n: &SdfNode, region: &Bounds, reach: f64) -> Slopes {
    let g = Near::of(region);
    let g = (g.r.is_finite() && g.c.iter().all(|c| c.is_finite())).then_some(g);
    Slopes { outside: steep(n, g.as_ref(), (0.0, reach)), inside: steep(n, g.as_ref(), (-reach, 0.0)) }
}

/// `n`'s [`Slopes`] anywhere, at only the values within `reach` of 0: outside at `0..=reach`,
/// inside at `-reach..=0`, as [`slopes_within`].
pub fn slopes_anywhere(n: &SdfNode, reach: f64) -> Slopes {
    Slopes { outside: steep(n, None, (0.0, reach)), inside: steep(n, None, (-reach, 0.0)) }
}

/// `w` times `f` (a negative `f` turns it over).
fn times(w: (f64, f64), f: f64) -> (f64, f64) {
    if f >= 0.0 {
        (w.0 * f, w.1 * f)
    } else {
        (w.1 * f, w.0 * f)
    }
}

/// How steep `n` can be at the points of `g` (None: anywhere) where its value lies in `w.0..=w.1`
/// (0 where it has none). A node's gradient is an operand's, or where it blends a mix of its
/// operands' that sums to 1, so it is at most the steepest of theirs over the values they have
/// where they weigh in:
/// - a smooth fold `a = smin(a, x, k)` is under both, and an operand weighs in only under the
///   fold's value plus `k` (at weight `h` it is `k (1 - h)^2` over it): over `m` operands the
///   `i`-th folded value lies in `w.0..=w.1 + (m - max(i, 1)) k`, the fold so far weighing in
///   only under the next one's value plus `k` (an intersection and a difference fold negated
///   values, [`fold`]);
/// - a shell `|x| - t` reads `x` over `w + t` and `-(w + t)`, a rounding `x - r` over `w + r`, a
///   frame `s x` over `w / s`, a deform `x / L` over `L w`, steeper by the warp's stretch over the
///   region over `L`;
/// - an ellipsoid's estimate by its radii, the region's `k0` and the window
///   ([`crate::lipschitz::ellipsoid_slope_within`]);
/// - a kernel's as it measured round its own box ([`kernel_steep`]).
fn steep(n: &SdfNode, g: Option<&Near>, w: (f64, f64)) -> f64 {
    use SdfNode::*;
    if !(w.0 <= w.1) {
        return 0.0;
    }
    match n {
        Sphere { .. } | Box { .. } | Cylinder { .. } | Torus { .. } | Capsule { .. } | Mesh(_) | Links(_) | Tubes(_) | Sketch(_) => 1.0,
        Ellipsoid { radii } => crate::lipschitz::ellipsoid_slope_within(*radii, g.map_or((0.0, f64::INFINITY), |g| ellipsoid_k(radii, g)), w),
        Gyroid { .. } => GYROID_SLOPE,
        Kernel(k) => kernel_steep(k, g, w),
        Sculpt { .. } => f64::INFINITY,
        Combine { op, blend, of } => {
            let (a, b) = if *op == SdfOp::Union { w } else { (-w.1, -w.0) };
            let m = of.len();
            of.iter()
                .enumerate()
                .map(|(i, c)| {
                    let folded = (a, b + blend.max(0.0) * (m - i.max(1)) as f64);
                    steep(c, g, if *op == SdfOp::Union || (*op == SdfOp::Difference && i > 0) { folded } else { (-folded.1, -folded.0) })
                })
                .fold(0.0, f64::max)
        }
        Shell { thickness: t, of } => steep(of, g, (w.0 + t, w.1 + t)).max(steep(of, g, (-w.1 - t, -w.0 - t))),
        Round { radius: r, of } => steep(of, g, (w.0 + r, w.1 + r)),
        Material { of, .. } | Id { of, .. } | Bone { of, .. } => steep(of, g, w),
        // (a frame of scale 0 reads its source nowhere it could be bounded)
        Transform { scale, .. } if *scale == 0.0 => f64::INFINITY,
        Transform { at, rotate, scale, of } => steep(of, g.map(|g| g.into(*at, *rotate, *scale)).as_ref(), times(w, 1.0 / scale)),
        Instance { places, of } => places.iter().map(|pl| if pl.scale == 0.0 { f64::INFINITY } else { steep(of, g.map(|g| g.placed(pl)).as_ref(), times(w, 1.0 / pl.scale)) }).fold(0.0, f64::max),
        Deform { deform, lipschitz, of } => match g {
            Some(g) => {
                let wb = warp_bounds(deform, &g.b);
                let s = steep(of, Some(&Near::of(&wb)), times(w, *lipschitz));
                if s == 0.0 {
                    0.0
                } else {
                    s * deform.lipschitz(Some(&wb)) / lipschitz
                }
            }
            None => f64::INFINITY,
        },
        // its own bound keeps it a distance's over a source no steeper ([`texture_slope`])
        Texture { tex, of } => texture_slope(steep(of, g.map(|g| g.grown(tex.reach())).as_ref(), (f64::NEG_INFINITY, f64::INFINITY))),
    }
}

/// How steep a kernel can be at the points of `g` (None: anywhere) where its value lies in `w`:
/// as it measured over its grown box ([`crate::lipschitz::kernel_measured`]). Outside its declared
/// box its value counts as at least its distance to that box (as [`node_bounds`] takes it), so a
/// region farther from the box than `w.1` holds none of the window's values, and a window under
/// the margin the box was grown by finds them all in the measured box, as a region inside it
/// does. Infinite otherwise, and for a kernel it does not measure.
fn kernel_steep(k: &crate::shape::SdfKernel, g: Option<&Near>, w: (f64, f64)) -> f64 {
    let Some(b) = k.bounds else { return f64::INFINITY };
    if g.is_some_and(|g| boxes_apart(&b, &g.b) > w.1) {
        return 0.0;
    }
    let Some((grown, m)) = crate::lipschitz::kernel_measured(k) else { return f64::INFINITY };
    let margin = (0..3).map(|i| (b.0[i] - grown.0[i]).min(grown.1[i] - b.1[i])).fold(f64::INFINITY, f64::min);
    let inside = g.is_some_and(|g| (0..3).all(|i| g.b.0[i] >= grown.0[i] && g.b.1[i] <= grown.1[i]));
    if inside || w.1 < margin {
        m.within(w)
    } else {
        f64::INFINITY
    }
}

/// The ellipsoid's `k0 = |p / r|` over the region: within its ball's reach of the centre's (`k0`
/// moves at most `1 / min(r)` per unit) and its box's axes' ranges.
fn ellipsoid_k(radii: &P3, g: &Near) -> (f64, f64) {
    let rmin = radii.iter().fold(f64::INFINITY, |a, &r| a.min(r));
    let k0 = (0..3).map(|i| (g.c[i] / radii[i]).powi(2)).sum::<f64>().sqrt();
    let axes: [(f64, f64); 3] = std::array::from_fn(|i| abs_range(g.b.0[i], g.b.1[i]));
    let k_box = ((0..3).map(|i| (axes[i].0 / radii[i]).powi(2)).sum::<f64>().sqrt(), (0..3).map(|i| (axes[i].1 / radii[i]).powi(2)).sum::<f64>().sqrt());
    ((k0 - g.r / rmin).max(k_box.0).max(0.0), (k0 + g.r / rmin).min(k_box.1))
}

/// `|g| / k` of a gyroid `g` (three products of a sine and a cosine of `k` times a coordinate)
/// changes at most `2 √3` times as fast as the point moves.
const GYROID_SLOPE: f64 = 3.464_101_615_137_755;

/// A texture keeps its source 1-Lipschitz ([`crate::texture`]): no bound for a steeper one.
fn texture_slope(of: f64) -> f64 {
    if of <= 1.0 {
        1.0
    } else {
        f64::INFINITY
    }
}

fn span(n: &SdfNode, g: &Near) -> Span {
    use crate::shape::distance;
    match n {
        SdfNode::Sphere { .. } | SdfNode::Box { .. } | SdfNode::Cylinder { .. } | SdfNode::Torus { .. } | SdfNode::Capsule { .. } | SdfNode::Mesh(_) | SdfNode::Links(_) | SdfNode::Tubes(_) | SdfNode::Sketch(_) => {
            let s = Span::by_slope(n.clone(), distance(n, v3(g.c)), 1.0, g.r);
            match leaf_range(n, &g.b) {
                Some((lo, hi)) => Span { lo: s.lo.max(lo), hi: s.hi.min(hi), ..s },
                None => s,
            }
        }
        // k0 (k0 - 1) / k1 with k0 = |p / r| and k0 / k1 between the least and the most radius:
        // bounded by k0's range over the region (over the ball k0 moves at most 1 / min(r) per
        // unit; over the box it is its axes' ranges). At the centre itself the estimate reads 0.
        SdfNode::Ellipsoid { radii } => {
            let at = distance(n, v3(g.c));
            let (rmin, rmax) = (radii.iter().fold(f64::INFINITY, |a, &r| a.min(r)), radii.iter().fold(0.0f64, |a, &r| a.max(r)));
            let (k_lo, k_hi) = ellipsoid_k(radii, g);
            let low = |k: f64| (k - 1.0) * if k >= 1.0 { rmin } else { rmax };
            let high = |k: f64| (k - 1.0) * if k >= 1.0 { rmax } else { rmin };
            let hi = if k_lo <= 1e-9 { high(k_hi).max(0.0) } else { high(k_hi) };
            let slope = crate::lipschitz::ellipsoid_slope_near(*radii, g.c, g.r);
            Span { node: n.clone(), lo: low(k_lo), hi, at, slope }.narrowed(g.r)
        }
        // at most 3 / k - wall / 2 anywhere
        SdfNode::Gyroid { period, wall } => {
            let s = Span::by_slope(n.clone(), distance(n, v3(g.c)), GYROID_SLOPE, g.r);
            Span { lo: s.lo.max(-0.5 * wall), hi: s.hi.min(3.0 * period / std::f64::consts::TAU - 0.5 * wall), ..s }
        }
        // A gridded kernel with the piece of its grid the region reads. Its value counts as a
        // distance outside its declared box (as [`node_bounds`] takes it).
        SdfNode::Kernel(k) => {
            let lo = match k.bounds {
                Some(b) if g.misses(&b) => (box_distance(&b, g.c) - g.r).max(boxes_apart(&b, &g.b)),
                Some(b) if boxes_apart(&b, &g.b) > 0.0 => boxes_apart(&b, &g.b),
                _ => f64::NEG_INFINITY,
            };
            Span::unbounded(SdfNode::Kernel(k.cropped(&g.b)), lo)
        }
        SdfNode::Combine { op, blend, of } => fold(*op, *blend, of, g),
        SdfNode::Shell { thickness, of } => {
            let s = span(of, g);
            let (lo, hi) = if s.lo >= 0.0 {
                (s.lo, s.hi)
            } else if s.hi <= 0.0 {
                (-s.hi, -s.lo)
            } else {
                (0.0, s.hi.max(-s.lo))
            };
            Span { node: SdfNode::Shell { thickness: *thickness, of: Box::new(s.node) }, lo: lo - thickness, hi: hi - thickness, at: s.at.abs() - thickness, slope: s.slope }
        }
        SdfNode::Round { radius, of } => {
            let s = span(of, g);
            Span { node: SdfNode::Round { radius: *radius, of: Box::new(s.node) }, lo: s.lo - radius, hi: s.hi - radius, at: s.at - radius, slope: s.slope }
        }
        SdfNode::Transform { at, rotate, scale, of } => {
            let s = span(of, &g.into(*at, *rotate, *scale)).scaled(*scale);
            Span { node: SdfNode::Transform { at: *at, rotate: *rotate, scale: *scale, of: Box::new(s.node) }, ..s }
        }
        // The slot's mask decides by its sign: where it is all positive the slot is not there,
        // where it is all at or under 0 it is there whole.
        SdfNode::Material { slot, within, of } => {
            let s = span(of, g);
            let within = match within {
                Some(w) => {
                    let m = span(w, g);
                    if m.lo > 0.0 {
                        return s;
                    }
                    (m.hi > 0.0).then(|| Box::new(m.node))
                }
                None => None,
            };
            Span { node: SdfNode::Material { slot: *slot, within, of: Box::new(s.node) }, ..s }
        }
        SdfNode::Id { id, of } => {
            let s = span(of, g);
            Span { node: SdfNode::Id { id: *id, of: Box::new(s.node) }, ..s }
        }
        SdfNode::Bone { id, rest, of } => {
            let s = span(of, g);
            Span { node: SdfNode::Bone { id: *id, rest: *rest, of: Box::new(s.node) }, ..s }
        }
        // The nearest copy: one whose least is at or over another's most changes no minimum.
        // `of` stays whole (one key for every copy).
        SdfNode::Instance { places, of } => {
            let each: Vec<Span> = places.iter().map(|pl| span(of, &g.placed(pl)).scaled(pl.scale)).collect();
            let (best, most) = each.iter().enumerate().fold((0, f64::INFINITY), |(b, m), (i, s)| if s.hi < m { (i, s.hi) } else { (b, m) });
            let keep: Vec<usize> = (0..places.len()).filter(|&i| i == best || each[i].lo < most).collect();
            let lo = keep.iter().map(|&i| each[i].lo).fold(f64::INFINITY, f64::min);
            let at = keep.iter().map(|&i| each[i].at).fold(f64::INFINITY, f64::min);
            let slope = keep.iter().map(|&i| each[i].slope).fold(0.0, f64::max);
            Span { node: SdfNode::Instance { places: keep.iter().map(|&i| places[i]).collect(), of: of.clone() }, lo, hi: most, at, slope }.narrowed(g.r)
        }
        // The source over the box the warp reads for the region, its value divided as the node
        // divides it. Its slope: the source's times the warp's stretch over that box (which
        // grows away from the warp's axis past what the node divides by).
        SdfNode::Deform { deform, lipschitz, of } => {
            let w = warp_bounds(deform, &g.b);
            let s = span(of, &Near::of(&w));
            let node = SdfNode::Deform { deform: *deform, lipschitz: *lipschitz, of: Box::new(s.node) };
            let (lo, hi) = (s.lo / lipschitz, s.hi / lipschitz);
            if s.slope.is_finite() {
                let at = distance(&node, v3(g.c));
                Span { node, lo, hi, at, slope: s.slope * deform.lipschitz(Some(&w)) / lipschitz }.narrowed(g.r)
            } else {
                Span { node, lo, hi, at: f64::NAN, slope: f64::INFINITY }
            }
        }
        // The strokes on the reduced source (a point reads the strokes of its own brick); the
        // brick floor bounds nothing here.
        SdfNode::Sculpt { strokes, of } => {
            let lo = if node_bounds(n).is_some_and(|b| g.misses(&b)) { 0.0 } else { f64::NEG_INFINITY };
            Span::unbounded(SdfNode::Sculpt { strokes: strokes.clone(), of: Box::new(span(of, g).node) }, lo)
        }
        // The source exact where the texture reads it: its displacement and gradient samples.
        SdfNode::Texture { tex, of } => {
            let s = span(of, &g.grown(tex.reach()));
            let node = SdfNode::Texture { tex: tex.clone(), of: Box::new(s.node) };
            let slope = texture_slope(s.slope);
            if slope.is_finite() {
                let at = distance(&node, v3(g.c));
                Span::by_slope(node, at, slope, g.r)
            } else {
                Span::unbounded(node, f64::NEG_INFINITY)
            }
        }
    }
}

/// A Combine reduced to the ball (see [`cut`]): `a_0 = s0 x_0`, `a_i = smin(a_(i-1), s x_i)`,
/// the value `sr a`, with the signs of the op.
fn fold(op: SdfOp, blend: f64, of: &[SdfNode], g: &Near) -> Span {
    fold_spans(op, blend, of.iter().map(|c| span(c, g)), g)
}

/// [`fold`] of its operands' spans, in order (a difference's base first).
fn fold_spans(op: SdfOp, blend: f64, of: impl ExactSizeIterator<Item = Span>, g: &Near) -> Span {
    let (s0, s, sr) = match op {
        SdfOp::Union => (1.0, 1.0, 1.0),
        SdfOp::Intersection => (-1.0, -1.0, -1.0),
        SdfOp::Difference => (-1.0, 1.0, -1.0),
    };
    let k = blend.max(0.0);
    // the fold so far: its bounds and the operands it keeps
    let mut kept: Vec<Span> = Vec::with_capacity(of.len());
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for (i, x) in of.enumerate() {
        let sign = if i == 0 { s0 } else { s };
        let (xlo, xhi) = if sign > 0.0 { (x.lo, x.hi) } else { (-x.hi, -x.lo) };
        if i > 0 {
            if xlo >= hi + k + slack(hi, xlo, k) {
                continue;
            }
            if op != SdfOp::Difference && xhi <= lo - k - slack(lo, xhi, k) {
                kept.clear();
            }
        }
        if kept.is_empty() {
            (lo, hi) = (xlo, xhi);
        } else {
            // smin rises in both operands: its least and most are at theirs (a hair's room for
            // the rounding)
            let (a, b) = (bound_smin(lo, xlo, blend), bound_smin(hi, xhi, blend));
            (lo, hi) = (a - 1e-12 * (1.0 + a.abs()), b + 1e-12 * (1.0 + b.abs()));
        }
        kept.push(x);
    }
    // The value at the centre, folded as `distance` folds it. The first kept operand takes the
    // first sign: a difference never drops its base, and a union's and an intersection's signs
    // are all one.
    let mut at = s0 * kept[0].at;
    for x in &kept[1..] {
        at = crate::shape::smin(at, s * x.at, blend);
    }
    let slope = kept.iter().map(|x| x.slope).fold(0.0, f64::max);
    let (lo, hi) = if sr > 0.0 { (lo, hi) } else { (-hi, -lo) };
    let node = if kept.len() == 1 {
        kept.pop().unwrap().node
    } else {
        SdfNode::Combine { op, blend, of: kept.into_iter().map(|x| x.node).collect() }
    };
    Span { node, lo, hi, at: sr * at, slope }.narrowed(g.r)
}

/// [`smin`](crate::shape::smin) of bounds, an infinite one included (no bound: the other).
fn bound_smin(a: f64, b: f64, k: f64) -> f64 {
    if a == f64::INFINITY {
        b
    } else if b == f64::INFINITY {
        a
    } else if a == f64::NEG_INFINITY || b == f64::NEG_INFINITY {
        f64::NEG_INFINITY
    } else {
        crate::shape::smin(a, b, k)
    }
}

/// A sculpt on `of` reduced to `region` (see [`sculpt_in`]).
fn sculpt_on(strokes: &crate::strokes::Strokes, mut node: Option<SdfNode>, region: &Bounds) -> Option<SdfNode> {
    for (t, caps) in strokes.shape_near(region.0, region.1) {
        let mut tube: Vec<SdfNode> = caps.into_iter().map(|(a, b)| SdfNode::Capsule { a, b, radius: t.radius }).collect();
        let tube = if tube.len() == 1 { tube.pop().unwrap() } else { SdfNode::Combine { op: SdfOp::Union, blend: 0.0, of: tube } };
        node = match (node, t.raise) {
            (Some(n), true) => Some(SdfNode::Combine { op: SdfOp::Union, blend: t.blend, of: vec![n, tube] }),
            (None, true) => Some(tube),
            (Some(n), false) => Some(SdfNode::Combine { op: SdfOp::Difference, blend: t.blend, of: vec![n, tube] }),
            (None, false) => None,
        };
    }
    node
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
        (Kernel(x), Kernel(y)) => x.key == y.key && x.bounds == y.bounds,
        (Mesh(x), Mesh(y)) => x.key() == y.key(),
        (Links(x), Links(y)) => x.key() == y.key(),
        (Tubes(x), Tubes(y)) => x.key() == y.key(),
        (Sketch(x), Sketch(y)) => x.key() == y.key(),
        (Material { slot: s1, within: w1, of: x }, Material { slot: s2, within: w2, of: y }) => {
            s1 == s2 && same(x, y) && match (w1, w2) { (None, None) => true, (Some(a), Some(b)) => same(a, b), _ => false }
        }
        (Id { id: i1, of: x }, Id { id: i2, of: y }) => i1 == i2 && same(x, y),
        (Bone { id: i1, rest: r1, of: x }, Bone { id: i2, rest: r2, of: y }) => i1 == i2 && r1 == r2 && same(x, y),
        (Instance { places: p1, of: x }, Instance { places: p2, of: y }) => p1 == p2 && same(x, y),
        (Deform { deform: d1, lipschitz: l1, of: x }, Deform { deform: d2, lipschitz: l2, of: y }) => d1 == d2 && l1 == l2 && same(x, y),
        (Sculpt { strokes: s1, of: x }, Sculpt { strokes: s2, of: y }) => s1.shape_key() == s2.shape_key() && same(x, y),
        (Texture { tex: t1, of: x }, Texture { tex: t2, of: y }) => t1.same(t2) && same(x, y),
        _ => false,
    }
}

/// Where two stroke logs on the same field can differ: the moved strokes' boxes, grown by
/// the widest blend.
fn strokes_changed(sa: &crate::strokes::Strokes, sb: &crate::strokes::Strokes) -> Vec<Bounds> {
    let k = sa.applied().chain(sb.applied()).filter_map(|(_, s)| s.tube()).map(|t| t.blend).fold(0.0, f64::max);
    sa.changed_boxes(sb, false).into_iter().map(|b| grow(b, k)).collect()
}

/// [`changes`] in one box: `None` when the nodes are the same; an unbounded change is `Some` of
/// an infinite box.
pub fn changed(old: &SdfNode, new: &SdfNode) -> Option<Bounds> {
    changes(old, new).into_iter().reduce(union)
}

/// Where the surface of `new` can differ from the surface of `old`, a box for each change: the
/// bounds of a changed subtree, old and new, grown by the blends, shells and roundings above it
/// and placed into the root frame. Each change reaches only its own box whatever the others do,
/// so past all the boxes the surface is the same. None when the nodes are the same; an unbounded
/// change is an infinite box.
pub fn changes(old: &SdfNode, new: &SdfNode) -> Vec<Bounds> {
    if same(old, new) {
        return Vec::new();
    }
    let everywhere = ([f64::NEG_INFINITY; 3], [f64::INFINITY; 3]);
    let whole = || match (node_bounds(old), node_bounds(new)) {
        (Some(a), Some(b)) => vec![union(a, b)],
        _ => vec![everywhere],
    };
    let grown = |boxes: Vec<Bounds>, r: f64| boxes.into_iter().map(|b| grow(b, r)).collect::<Vec<_>>();
    match (old, new) {
        (SdfNode::Combine { op: oa, blend: ba, of: ca }, SdfNode::Combine { op: ob, blend: bb, of: cb }) if oa == ob && ba == bb && ca.len() == cb.len() => {
            grown(ca.iter().zip(cb).flat_map(|(a, b)| changes(a, b)).collect(), fold_reach(*ba, ca.len()))
        }
        (SdfNode::Shell { thickness: ta, of: a }, SdfNode::Shell { thickness: tb, of: b }) if ta == tb => grown(changes(a, b), *ta),
        (SdfNode::Round { radius: ra, of: a }, SdfNode::Round { radius: rb, of: b }) if ra == rb => grown(changes(a, b), *ra),
        (SdfNode::Transform { at: aa, rotate: ra, scale: sa, of: a }, SdfNode::Transform { at: ab, rotate: rb, scale: sb, of: b }) if aa == ab && ra == rb && sa == sb => {
            changes(a, b).iter().map(|c| map_bounds(c, out_of(*aa, *ra, *sa))).collect()
        }
        // A slot change is a material change on the same surface.
        (SdfNode::Material { of: a, .. }, SdfNode::Material { of: b, .. }) | (SdfNode::Id { of: a, .. }, SdfNode::Id { of: b, .. }) => match changes(a, b) {
            none if none.is_empty() => whole(),
            boxes => boxes,
        },
        (SdfNode::Bone { id: ia, rest: ra, of: a }, SdfNode::Bone { id: ib, rest: rb, of: b }) if ia == ib && ra == rb => changes(a, b),
        // The source's change, as far as the texture reads it.
        (SdfNode::Texture { tex: ta, of: a }, SdfNode::Texture { tex: tb, of: b }) if ta.same(tb) => grown(changes(a, b), ta.reach()),
        // Only where the strokes moved (one drawn, one undone).
        (SdfNode::Sculpt { strokes: sa, of: a }, SdfNode::Sculpt { strokes: sb, of: b }) if same(a, b) => strokes_changed(sa, sb),
        // The first stroke (or the last one undone): only where it reaches.
        (a, SdfNode::Sculpt { strokes, of: b }) if same(a, b) => strokes_changed(&crate::strokes::Strokes::new(), strokes),
        (SdfNode::Sculpt { strokes, of: a }, b) if same(a, b) => strokes_changed(strokes, &crate::strokes::Strokes::new()),
        (SdfNode::Instance { places: pa, of: a }, SdfNode::Instance { places: pb, of: b }) if same(a, b) => {
            // Only the moved, added or removed copies.
            let ob = node_bounds(a).unwrap_or(everywhere);
            let moved = |x: &[crate::shape::SdfPlace], y: &[crate::shape::SdfPlace]| x.iter().filter(|p| !y.contains(p)).map(|pl| map_bounds(&ob, |q| pl.out_of(q))).collect::<Vec<_>>();
            moved(pa, pb).into_iter().chain(moved(pb, pa)).collect()
        }
        _ => whole(),
    }
}

/// What [`cut_within`] reads of one tree for one window, made once per tree ([`window`]): each
/// node's window, and for each node the cut may leave out (the tree itself, a union's operand, a
/// difference's cutter) the box in the tree's frame round every point where its value lies under
/// its window's top.
pub struct Window {
    w: f64,
    at: Ann,
}

/// One node's part of a [`Window`].
struct Ann {
    /// Its values that can move the tree's within the window lie under `hi` (see [`window`]).
    hi: f64,
    /// The cut may leave it out where it lies at or over `hi` all over the region.
    point: bool,
    /// Every point where its value lies under `hi` lies within `.1` of the box `.0`, in the
    /// tree's frame ([`reach`] placed out). None: anywhere, or not a point.
    reach: Option<(Bounds, f64)>,
    /// Its operands', in order: none for a node the cut reads whole ([`cut`]: its own value).
    of: Vec<Ann>,
}

/// The [`Window`] of tree `n` for the values within `w` of 0 ([`cut_within`]). A node's values
/// can move the tree's only in `lo..hi`, from the tree's `-w..w` down:
/// - a union's `i`-th of `m` operands reads its `lo..hi + 1.25 k (m - max(i, 1))` (`k` its
///   blend): a smooth step `smin(a, x)` is the nearer operand bit for bit where they differ by
///   `k` or more and at most `k / 4` under the less elsewhere, so where the step lies under `h`
///   the operands that move it lie under `h + 1.25 k`, and where it lies at or over `h` an
///   operand at or over `h + 1.25 k` leaves it at or over `h`; the fold's step `j` is `m - 1 - j`
///   steps from its end, and its first two operands enter at its first step;
/// - a difference folds its base's and its cutters' negations the same way (its cutters read
///   `-hi..-lo` widened at the top, its base `lo..hi` widened at the foot), an intersection
///   every operand's (each `lo..hi` widened at the foot);
/// - a frame of scale `s > 0` reads `lo / s..hi / s`, a rounding by `r` `lo + r..hi + r`, a
///   material, id or bone its own `lo..hi`;
/// - any other node (a primitive, a shell, copies, a warp, a texture, a sculpt) is read whole.
pub fn window(n: &SdfNode, w: f64) -> Window {
    Window { w, at: annotate(n, -w, w, true, &mut Vec::new()) }
}

fn annotate(n: &SdfNode, lo: f64, hi: f64, point: bool, frames: &mut Vec<(P3, P3, f64)>) -> Ann {
    use SdfNode::*;
    let reach = if point { reach(n, hi, false).map(|(b, m)| placed_out(b, m, frames)) } else { None };
    // (a hair's room for the steps' rounding)
    let step = |k: f64, m: usize, i: usize| {
        let d = 1.25 * k * m.saturating_sub(i.max(1)) as f64;
        d + 1e-9 * (1.0 + lo.abs().max(hi.abs()) + d)
    };
    let of = match n {
        Combine { op, blend, of } => {
            let (k, m) = (blend.max(0.0), of.len());
            of.iter()
                .enumerate()
                .map(|(i, c)| match op {
                    SdfOp::Union => annotate(c, lo, hi + step(k, m, i), true, frames),
                    SdfOp::Difference if i > 0 => annotate(c, -hi, -lo + step(k, m, i), true, frames),
                    _ => annotate(c, lo - step(k, m, i), hi, false, frames),
                })
                .collect()
        }
        Transform { at, rotate, scale, of } if *scale > 0.0 => {
            frames.push((*at, *rotate, *scale));
            let a = annotate(of, lo / scale, hi / scale, false, frames);
            frames.pop();
            vec![a]
        }
        Round { radius, of } => vec![annotate(of, lo + radius, hi + radius, false, frames)],
        Material { of, .. } | Id { of, .. } | Bone { of, .. } => vec![annotate(of, lo, hi, false, frames)],
        _ => Vec::new(),
    };
    Ann { hi, point, reach, of }
}

/// A box and ball margin in a node's frame placed out through the frames over it, innermost
/// last.
fn placed_out(mut b: Bounds, mut m: f64, frames: &[(P3, P3, f64)]) -> (Bounds, f64) {
    for &(at, rotate, scale) in frames.iter().rev() {
        b = map_bounds(&b, out_of(at, rotate, scale));
        m *= scale;
    }
    (b, m)
}

/// The region (the tree's frame) lies farther than `r`'s margin from its box: no point of it.
fn misses(region: &Bounds, r: &(Bounds, f64)) -> bool {
    boxes_apart(region, &r.0) > r.1 + 1e-9 * (1.0 + r.1)
}

/// The tree `n` reduced to `region` (its frame) for the values within `w` of 0, `win` its
/// [`window`] for `w`: [`cut`] after leaving out each union operand and difference cutter that
/// lies at or over its window's top all over the region (by its box there, [`reach`], or by the
/// bounds of its own cut). At every point of the region the reduced node's value is the tree's
/// clamped alike to `-w..=w`, bit for bit:
/// - where the tree's value lies over `-w` and under `w`, the reduced node's is the same bits;
/// - where it is `w` or more, the reduced node's is `w` or more (outside);
/// - where it is `-w` or less, the reduced node's is `-w` or less (inside).
///
/// No more: past the window the two differ, and a material's slot is the tree's only where the
/// values agree. `lo..=hi` bounds the reduced node's value over the region, not the tree's.
/// None: the tree's value is `w` or more all over the region (its clamp reads `+w`, outside),
/// as where every operand of its union is left out. A union's operand or a cutter is left out
/// only where it cannot win: an operand of an intersection, a difference's base and a node read
/// whole (a warp, copies, a shell) are never reduced below their own box ([`reach`]), so a loose
/// bound keeps more and drops nothing it must not. Two trees reduce alike where
/// [`changed_within`] says their changes do not [`reaches`] the region.
pub fn cut_within(n: &SdfNode, win: &Window, region: &Bounds) -> Option<Cut> {
    let g = Near::of(region);
    if !(g.r.is_finite() && g.c.iter().all(|c| c.is_finite())) {
        // everywhere: nothing can be dropped
        return Some(Cut { node: n.clone(), lo: f64::NEG_INFINITY, hi: f64::INFINITY });
    }
    wspan(n, &win.at, &g, region).map(|s| Cut { node: s.node, lo: s.lo, hi: s.hi })
}

/// [`span`] for the values under `a`'s window's top ([`cut_within`]); None: at or over it all
/// over the region.
fn wspan(n: &SdfNode, a: &Ann, g: &Near, region: &Bounds) -> Option<Span> {
    use SdfNode::*;
    if a.point && a.reach.as_ref().is_some_and(|r| misses(region, r)) {
        return None;
    }
    let s = match n {
        _ if a.of.is_empty() => span(n, g),
        Combine { op, blend, of } => {
            // a point (a union's operand, a cutter) at or over its top changes no value under
            // the fold's; any other operand there leaves the fold there too
            let mut kept = Vec::with_capacity(of.len());
            for (c, ca) in of.iter().zip(&a.of) {
                match wspan(c, ca, g, region) {
                    Some(x) => kept.push(x),
                    None if ca.point => {}
                    None => return None,
                }
            }
            if kept.is_empty() {
                return None;
            }
            fold_spans(*op, *blend, kept.into_iter(), g)
        }
        Transform { at, rotate, scale, of } => {
            let s = wspan(of, &a.of[0], &g.into(*at, *rotate, *scale), region)?.scaled(*scale);
            Span { node: Transform { at: *at, rotate: *rotate, scale: *scale, of: std::boxed::Box::new(s.node) }, ..s }
        }
        Round { radius, of } => {
            let s = wspan(of, &a.of[0], g, region)?;
            Span { node: Round { radius: *radius, of: std::boxed::Box::new(s.node) }, lo: s.lo - radius, hi: s.hi - radius, at: s.at - radius, slope: s.slope }
        }
        Material { slot, within, of } => {
            let s = wspan(of, &a.of[0], g, region)?;
            match within {
                Some(w) => {
                    let m = span(w, g);
                    if m.lo > 0.0 {
                        s
                    } else {
                        let within = (m.hi > 0.0).then(|| std::boxed::Box::new(m.node));
                        Span { node: Material { slot: *slot, within, of: std::boxed::Box::new(s.node) }, ..s }
                    }
                }
                None => Span { node: Material { slot: *slot, within: None, of: std::boxed::Box::new(s.node) }, ..s },
            }
        }
        Id { id, of } => {
            let s = wspan(of, &a.of[0], g, region)?;
            Span { node: Id { id: *id, of: std::boxed::Box::new(s.node) }, ..s }
        }
        Bone { id, rest, of } => {
            let s = wspan(of, &a.of[0], g, region)?;
            Span { node: Bone { id: *id, rest: *rest, of: std::boxed::Box::new(s.node) }, ..s }
        }
        _ => span(n, g),
    };
    (s.lo < a.hi).then_some(s)
}

/// Where [`cut_within`] can reduce two trees differently ([`changed_within`], [`reaches`]), in
/// the trees' frame: boxes and ball margins.
#[derive(Clone, Debug, Default)]
pub struct WindowChange {
    /// The boxes in both trees of the nearest node over each change that the cut may leave out:
    /// a region must lie farther than every margin from these, so the cut leaves it out of both.
    near: Vec<(Bounds, f64)>,
    /// The boxes in the old and the new tree of the nodes the cut may leave out over a change,
    /// where they differ (a group's grows as a piece of it moves away): a region must lie
    /// outside both or within both, so the cut treats the node alike in both.
    over: Vec<(Option<(Bounds, f64)>, Option<(Bounds, f64)>)>,
}

/// Where [`cut_within`] can reduce `old` and `new` (`win_old` and `win_new` their [`window`]s
/// for one `w`) differently: a region it does not reach ([`reaches`] false) reduces both to the
/// same tree (or both to None), and so to the same clamped values. Each change is followed down
/// through the nodes the cut reads operand by operand while both trees have the same node
/// there; the nearest node over it that the cut may leave out (a union's operand, a cutter, the
/// tree) gives its boxes in both trees, and the nodes it may leave out over that, the boxes that
/// change with it. No boxes: the same trees. None: anywhere.
pub fn changed_within(old: &SdfNode, win_old: &Window, new: &SdfNode, win_new: &Window) -> Option<WindowChange> {
    if win_old.w.to_bits() != win_new.w.to_bits() {
        return None;
    }
    let mut out = WindowChange::default();
    if moved(old, &win_old.at, new, &win_new.at, &mut out) {
        return Some(out);
    }
    Some(WindowChange { near: vec![win_old.at.reach?, win_new.at.reach?], over: Vec::new() })
}

/// The boxes of the changes under `a` and `b` into `out` ([`changed_within`]); false: a change
/// no point under them covers (the caller's covers it).
fn moved(a: &SdfNode, aa: &Ann, b: &SdfNode, ab: &Ann, out: &mut WindowChange) -> bool {
    use SdfNode::*;
    if same(a, b) {
        return true;
    }
    let kids: Vec<(&SdfNode, &SdfNode)> = match (a, b) {
        (Combine { op: o1, blend: k1, of: c1 }, Combine { op: o2, blend: k2, of: c2 }) if o1 == o2 && k1 == k2 && c1.len() == c2.len() => c1.iter().zip(c2).collect(),
        (Transform { at: a1, rotate: r1, scale: s1, of: x }, Transform { at: a2, rotate: r2, scale: s2, of: y }) if a1 == a2 && r1 == r2 && s1 == s2 => vec![(&**x, &**y)],
        (Round { radius: r1, of: x }, Round { radius: r2, of: y }) if r1 == r2 => vec![(&**x, &**y)],
        (Id { id: i1, of: x }, Id { id: i2, of: y }) if i1 == i2 => vec![(&**x, &**y)],
        (Bone { id: i1, rest: r1, of: x }, Bone { id: i2, rest: r2, of: y }) if i1 == i2 && r1 == r2 => vec![(&**x, &**y)],
        (Material { slot: s1, within: w1, of: x }, Material { slot: s2, within: w2, of: y })
            if s1 == s2 && match (w1, w2) {
                (None, None) => true,
                (Some(p), Some(q)) => same(p, q),
                _ => false,
            } =>
        {
            vec![(&**x, &**y)]
        }
        _ => return false,
    };
    // only through nodes the cut reads operand by operand in both
    if aa.of.len() != kids.len() || ab.of.len() != kids.len() {
        return false;
    }
    if aa.point && aa.reach != ab.reach {
        out.over.push((aa.reach, ab.reach));
    }
    for (i, (x, y)) in kids.into_iter().enumerate() {
        let (xa, ya) = (&aa.of[i], &ab.of[i]);
        if moved(x, xa, y, ya, out) {
            continue;
        }
        match (xa.point && ya.point, xa.reach, ya.reach) {
            (true, Some(p), Some(q)) => out.near.extend([p, q]),
            _ => return false,
        }
    }
    true
}

/// [`cut_within`] may reduce the two trees of [`changed_within`] differently in `region` (the
/// trees' frame).
pub fn reaches(region: &Bounds, change: &WindowChange) -> bool {
    let gone = |r: &Option<(Bounds, f64)>| r.as_ref().is_some_and(|r| misses(region, r));
    change.near.iter().any(|r| !misses(region, r)) || change.over.iter().any(|(a, b)| gone(a) != gone(b))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::shape::distance;
    use makepad_csg_math::Vec3d;

    fn at(p: P3, of: SdfNode) -> SdfNode {
        SdfNode::Transform { at: p, rotate: [0.0; 3], scale: 1.0, of: Box::new(of) }
    }

    /// A one-point stroke culls to a dab on the solid: its capsule has no length.
    #[test]
    fn a_one_point_stroke_culls_to_a_dab() {
        let mut strokes = crate::strokes::Strokes::new();
        strokes.push(crate::strokes::StrokeOp::Raise, 2.0, 1.0, &[[10.0, 0.0, 0.0]]);
        let node = sculpt_in(&strokes, &SdfNode::Sphere { radius: 10.0 }, &([-20.0; 3], [20.0; 3])).expect("the solid");
        let d = |x: f64| distance(&node, Vec3d::new(x, 0.0, 0.0));
        assert!(d(0.0) < -9.0, "inside the solid: {}", d(0.0));
        assert!(d(10.0) < 0.0, "the dab holds the stroke's point: {}", d(10.0));
        assert!(d(30.0) > 15.0, "far off: {}", d(30.0));
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

    /// A small xorshift, for random edits from fixed seeds.
    pub(crate) struct Rng(pub u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        /// Uniform in `0..1`.
        pub fn unit(&mut self) -> f64 {
            (self.next() >> 11) as f64 / (1u64 << 53) as f64
        }

        pub fn below(&mut self, n: usize) -> usize {
            (self.next() % n as u64) as usize
        }
    }

    /// A small figure to edit at random: a tall flat ellipsoid body and four pieces in one
    /// smooth union, an arm of three more in a union of its own inside it, and a flat cut
    /// bottom. A piece is a ball, a rounded box, an ellipsoid or a capsule.
    #[derive(Clone)]
    pub(crate) struct Figure {
        kind: [u8; 7],
        size: [f64; 7],
        at: [P3; 7],
        blend: [f64; 2],
    }

    impl Figure {
        pub fn new() -> Figure {
            Figure {
                kind: [0, 1, 2, 3, 0, 2, 1],
                size: [3.0, 2.5, 3.5, 2.0, 2.5, 2.0, 1.8],
                at: [[11.0, -12.0, 0.0], [-10.0, -4.0, 1.0], [0.0, 21.0, 0.0], [3.0, 6.0, 5.5], [8.0, 12.0, 0.0], [11.0, 15.0, 0.0], [13.5, 17.5, 1.0]],
                blend: [1.5, 1.0],
            }
        }

        fn piece(&self, i: usize) -> SdfNode {
            let s = self.size[i];
            let node = match self.kind[i] {
                0 => SdfNode::Sphere { radius: s },
                1 => SdfNode::Box { size: [1.6 * s, 1.2 * s, 1.4 * s], radius: 0.3 * s },
                2 => SdfNode::Ellipsoid { radii: [s, 0.7 * s, 1.2 * s] },
                _ => SdfNode::Capsule { a: [-s, 0.0, 0.0], b: [s, 0.5 * s, 0.0], radius: 0.6 * s },
            };
            at(self.at[i], node)
        }

        pub fn node(&self) -> SdfNode {
            let arm = SdfNode::Combine { op: SdfOp::Union, blend: self.blend[1], of: (4..7).map(|i| self.piece(i)).collect() };
            let mut of = vec![SdfNode::Ellipsoid { radii: [10.0, 22.0, 6.0] }];
            of.extend((0..4).map(|i| self.piece(i)));
            of.push(arm);
            let body = SdfNode::Combine { op: SdfOp::Union, blend: self.blend[0], of };
            SdfNode::Combine { op: SdfOp::Difference, blend: 0.2, of: vec![body, at([0.0, -25.0, 0.0], SdfNode::Box { size: [60.0, 10.0, 60.0], radius: 0.0 })] }
        }

        /// One random edit, its kind's name: a piece's size, a piece replaced by another kind,
        /// a piece moved, or a blend's radius.
        pub fn edit(&mut self, r: &mut Rng) -> &'static str {
            let i = r.below(7);
            match r.below(4) {
                0 => {
                    self.size[i] = (self.size[i] * (0.8 + 0.45 * r.unit())).clamp(1.0, 5.0);
                    "size"
                }
                1 => {
                    self.kind[i] = ((self.kind[i] as usize + 1 + r.below(3)) % 4) as u8;
                    "replace"
                }
                2 => {
                    for k in 0..3 {
                        self.at[i][k] = (self.at[i][k] + 6.0 * (r.unit() - 0.5)).clamp(-18.0, 24.0);
                    }
                    "move"
                }
                _ => {
                    self.blend[r.below(2)] = 0.3 + 2.7 * r.unit();
                    "blend"
                }
            }
        }
    }

    /// Random edits of [`Figure`] from fixed seeds: no point a dense sample round the change
    /// region (jittered from edit to edit) changes side outside the region. The far reaches of
    /// a fold's change are thin, so the sample is dense near the region and only there.
    #[test]
    fn random_edits_change_the_solid_only_inside_the_region() {
        let mut checked = 0;
        for seed in 1..=4u64 {
            let mut r = Rng(0x9e37_79b9_7f4a_7c15 ^ (seed << 32));
            let mut fig = Figure::new();
            for _ in 0..50 {
                let old = fig.node();
                let what = fig.edit(&mut r);
                let new = fig.node();
                let Some(region) = changed(&old, &new) else { continue };
                let hull = union(node_bounds(&old).unwrap(), node_bounds(&new).unwrap());
                let Some(near) = meet(&grow(region, 4.0), &grow(hull, 0.5)) else { continue };
                let n = 36;
                let j: P3 = std::array::from_fn(|_| r.unit());
                let step: P3 = std::array::from_fn(|k| (near.1[k] - near.0[k]) / n as f64);
                for a in 0..n {
                    for b in 0..n {
                        for c in 0..n {
                            let p = Vec3d::new(near.0[0] + (a as f64 + j[0]) * step[0], near.0[1] + (b as f64 + j[1]) * step[1], near.0[2] + (c as f64 + j[2]) * step[2]);
                            if inside(&region, p) {
                                continue;
                            }
                            checked += 1;
                            assert!((distance(&old, p) <= 0.0) == (distance(&new, p) <= 0.0), "seed {seed}: a {what} edit changed {p:?} outside {region:?}");
                        }
                    }
                }
            }
        }
        assert!(checked > 500_000, "{checked} points outside the regions");
    }

    /// Random pairs of edits of [`Figure`] at once, each pair told by a box per change: no point a
    /// dense sample round each box holds changes side outside all of them, and [`changed`] is
    /// the box round them.
    #[test]
    fn random_edits_two_at_once_change_the_solid_only_inside_their_boxes() {
        let (mut checked, mut apart) = (0, 0);
        for seed in 1..=2u64 {
            let mut r = Rng(0x2545_f491_4f6c_dd1d ^ (seed << 32));
            let mut fig = Figure::new();
            for _ in 0..40 {
                let old = fig.node();
                let what = [fig.edit(&mut r), fig.edit(&mut r)];
                let new = fig.node();
                let boxes = changes(&old, &new);
                let round = boxes.iter().copied().reduce(union);
                assert!(round.map(|(lo, hi)| (lo.map(f64::to_bits), hi.map(f64::to_bits))) == changed(&old, &new).map(|(lo, hi)| (lo.map(f64::to_bits), hi.map(f64::to_bits))), "seed {seed}: {what:?}: changed is the box round {boxes:?}");
                apart += usize::from(boxes.iter().enumerate().any(|(i, a)| boxes[i + 1..].iter().any(|b| meet(a, b).is_none())));
                let hull = union(node_bounds(&old).unwrap(), node_bounds(&new).unwrap());
                for b in &boxes {
                    let Some(near) = meet(&grow(*b, 4.0), &grow(hull, 0.5)) else { continue };
                    let n = 24;
                    let j: P3 = std::array::from_fn(|_| r.unit());
                    let step: P3 = std::array::from_fn(|k| (near.1[k] - near.0[k]) / n as f64);
                    for a in 0..n {
                        for c in 0..n {
                            for d in 0..n {
                                let p = Vec3d::new(near.0[0] + (a as f64 + j[0]) * step[0], near.0[1] + (c as f64 + j[1]) * step[1], near.0[2] + (d as f64 + j[2]) * step[2]);
                                if boxes.iter().any(|b| inside(b, p)) {
                                    continue;
                                }
                                checked += 1;
                                assert!((distance(&old, p) <= 0.0) == (distance(&new, p) <= 0.0), "seed {seed}: {what:?} changed {p:?} outside {boxes:?}");
                            }
                        }
                    }
                }
            }
        }
        assert!(checked > 100_000, "{checked} points outside the boxes");
        assert!(apart >= 5, "{apart} pairs of edits made boxes apart");
    }

    /// Two balls grown at the two ends of a blended chain in a turned frame: a box for each, apart,
    /// each holding where its own ball moves the surface, necks included, and no point outside
    /// them changing side.
    #[test]
    fn two_changes_in_a_turned_blend_stay_in_their_boxes() {
        let ball = |x: f64, r: f64| at([x, 0.0, 0.0], SdfNode::Sphere { radius: r });
        let chain = |a: f64, b: f64| SdfNode::Transform {
            at: [3.0, -2.0, 5.0],
            rotate: [25.0, 50.0, 10.0],
            scale: 1.0,
            of: Box::new(SdfNode::Combine { op: SdfOp::Union, blend: 2.0, of: vec![ball(-60.0, a), ball(-55.0, 2.0), ball(0.0, 2.0), ball(55.0, 2.0), ball(60.0, b)] }),
        };
        let (old, new) = (chain(3.0, 3.0), chain(3.5, 3.5));
        let boxes = changes(&old, &new);
        assert_eq!(boxes.len(), 2, "{boxes:?}");
        assert!(meet(&boxes[0], &boxes[1]).is_none(), "the two are apart: {boxes:?}");
        for b in &boxes {
            let mut moved = 0;
            for p in grid(&grow(*b, 3.0), 70) {
                if (distance(&old, p) <= 0.0) != (distance(&new, p) <= 0.0) {
                    assert!(inside(b, p), "{p:?} changed outside {b:?}");
                    moved += 1;
                }
            }
            assert!(moved > 0, "the surface moved in {b:?}");
        }
    }

    /// A smooth fold carries a change on through its later steps: growing the first of three
    /// balls in a chain moves the surface at the neck of the other two, past one step's reach
    /// (a blend and its sag) of the grown ball, and nowhere past the fold's reach of it.
    #[test]
    fn a_fold_carries_a_change_through_its_later_steps() {
        let ball = |x: f64, r: f64| at([x, 0.0, 0.0], SdfNode::Sphere { radius: r });
        let chain = |r: f64| SdfNode::Combine { op: SdfOp::Union, blend: 2.0, of: vec![ball(-4.5, r), ball(0.0, 1.0), ball(4.5, 3.0)] };
        let (a, b) = (chain(3.0), chain(3.5));
        let r = changed(&a, &b).expect("the first ball grew");
        let grown: Bounds = ([-8.0, -3.5, -3.5], [-1.0, 3.5, 3.5]);
        let want = grow(grown, fold_reach(2.0, 3));
        assert!((0..3).all(|k| (r.0[k] - want.0[k]).abs() < 1e-9 && (r.1[k] - want.1[k]).abs() < 1e-9), "the grown ball's box, grown by the fold's two steps: {r:?}");
        // the change is a thin sliver at the far neck: a fine slice through the chain's axis,
        // and a coarser grid round the whole region
        let slice = (0..=850).flat_map(|i| (0..=400).map(move |j| Vec3d::new(-8.5 + 0.02 * i as f64, -4.0 + 0.02 * j as f64, 0.0)));
        let mut farthest = 0.0f64;
        for p in slice.chain(grid(&grow(r, 1.0), 60)) {
            if (distance(&a, p) <= 0.0) != (distance(&b, p) <= 0.0) {
                assert!(inside(&r, p), "{p:?} changed outside {r:?}");
                let q = [p.x, p.y, p.z];
                let gap = (0..3).map(|k| (grown.0[k] - q[k]).max(q[k] - grown.1[k])).fold(0.0, f64::max);
                farthest = farthest.max(gap);
            }
        }
        assert!(farthest > 1.25 * 2.0, "the change reaches past one step of the ball: {farthest}");
    }

    /// Both boxes hold the solid: the vase, and the vase with a flat foot cut by a box and a lip
    /// blended on (the tight box keeps the foot on the cutter's face).
    #[test]
    fn bounds_hold_the_solid() {
        let footed = SdfNode::Combine {
            op: SdfOp::Union,
            blend: 1.5,
            of: vec![
                SdfNode::Combine { op: SdfOp::Difference, blend: 0.0, of: vec![vase(4.0), at([0.0, -5.0, 0.0], SdfNode::Box { size: [40.0, 10.0, 40.0], radius: 0.0 })] },
                at([0.0, 41.0, 0.0], SdfNode::Torus { radius: 5.0, tube: 1.0 }),
            ],
        };
        for n in [vase(4.0), footed.clone()] {
            for b in [node_bounds(&n).unwrap(), tight_bounds(&n).unwrap()] {
                for p in grid(&grow(b, 10.0), 40) {
                    if distance(&n, p) <= 0.0 {
                        assert!(inside(&b, p), "{p:?} outside {b:?}");
                    }
                }
            }
        }
        let (loose, tight) = (node_bounds(&footed).unwrap(), tight_bounds(&footed).unwrap());
        assert!(tight.0[1] == 0.0 && loose.0[1] < -0.3, "the foot: {} tight, {} loose", tight.0[1], loose.0[1]);
    }

    /// A blend of many operands sags only where two of them come near: a cut foot blended with
    /// a body and two arms well above it keeps its tight bottom on the cutter's face, and the
    /// tight box holds the solid.
    #[test]
    fn a_blend_of_many_keeps_a_cut_foot_on_the_cutters_face() {
        let foot = SdfNode::Combine {
            op: SdfOp::Difference,
            blend: 0.2,
            of: vec![
                SdfNode::Combine { op: SdfOp::Union, blend: 1.5, of: vec![at([-5.0, 2.0, 0.0], SdfNode::Ellipsoid { radii: [6.0, 3.8, 6.5] }), at([5.0, 2.0, 0.0], SdfNode::Sphere { radius: 5.0 })] },
                at([0.0, -4.7, 0.0], SdfNode::Box { size: [80.0, 10.0, 80.0], radius: 0.0 }),
            ],
        };
        let many = SdfNode::Combine {
            op: SdfOp::Union,
            blend: 0.8,
            of: vec![
                foot,
                SdfNode::Capsule { a: [0.0, 12.0, 0.0], b: [0.0, 20.0, 0.0], radius: 3.0 },
                SdfNode::Capsule { a: [-3.0, 18.0, 0.0], b: [-12.0, 12.0, 0.0], radius: 1.5 },
                SdfNode::Capsule { a: [3.0, 18.0, 0.0], b: [12.0, 12.0, 0.0], radius: 1.5 },
            ],
        };
        let n = SdfNode::Combine { op: SdfOp::Union, blend: 0.2, of: vec![many, at([0.0, 25.0, 0.0], SdfNode::Sphere { radius: 5.0 })] };
        let tight = tight_bounds(&n).unwrap();
        assert!(tight.0[1] == -4.7 + 5.0, "the foot: {} tight", tight.0[1]);
        for p in grid(&grow(tight, 4.0), 60) {
            if distance(&n, p) <= 0.0 {
                assert!(inside(&tight, p), "{p:?} outside {tight:?}");
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

    /// A smooth fold of three: dropping the first operand changes the blend
    /// of the other two far beyond the blend width. Culled to thin slabs, the
    /// values `m` deep inside each slab are exact wherever they are under `m`.
    #[test]
    fn culled_smooth_fold_keeps_the_values() {
        let n = SdfNode::Combine {
            op: SdfOp::Union,
            blend: 2.0,
            of: vec![
                at([0.0, 2.5, 0.0], SdfNode::Box { size: [40.0, 5.0, 20.0], radius: 0.0 }),
                at([0.0, 20.0, -8.0], SdfNode::Box { size: [40.0, 40.0, 4.0], radius: 0.0 }),
                at([0.0, 12.0, -4.0], SdfNode::Box { size: [4.0, 16.0, 4.0], radius: 0.0 }),
            ],
        };
        let m = 1.5;
        for y in [6.0, 8.0, 8.8, 10.0, 14.0, 30.0] {
            let slab = ([-25.0, y - m, -15.0], [25.0, y + m, 15.0]);
            let c = cull(&n, &slab).unwrap();
            for p in grid(&([-22.0, y, -12.0], [22.0, y, 12.0]), 60) {
                let d = distance(&n, p);
                if d.abs() < m {
                    assert_eq!(distance(&c, p), d, "slab {y}: {p:?}");
                }
            }
        }
    }

    fn turned(p: P3, rotate: P3, scale: f64, of: SdfNode) -> SdfNode {
        SdfNode::Transform { at: p, rotate, scale, of: Box::new(of) }
    }

    /// A splitmix step in 0..1.
    pub(crate) fn unit(s: &mut u64) -> f64 {
        *s = s.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = *s;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        ((z ^ (z >> 31)) >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Many pieces in smooth folds: ellipsoids `flat` to 1 between their axes, turned and
    /// scaled (inside out too), a hard step after smooth ones, a cut, an intersection, a shell,
    /// a twist, a turned sketch and copies.
    pub(crate) fn crowd(seed: u64, flat: [f64; 3]) -> SdfNode {
        let mut s = seed;
        let mut lumps = Vec::new();
        for i in 0..120 {
            let a = i as f64 * 2.399_963;
            let at = [12.0 * a.cos(), 0.5 * i as f64, 12.0 * a.sin()];
            let radii = [2.0 + 3.0 * unit(&mut s), (2.0 + 3.0 * unit(&mut s)) / flat[i % 3], 2.0 + 2.0 * unit(&mut s)];
            let piece = match i % 5 {
                0 => SdfNode::Box { size: [3.0, 2.0, 4.0], radius: 0.4 },
                1 => SdfNode::Capsule { a: [0.0, 0.0, 0.0], b: [0.0, 3.0, 1.0], radius: 1.2 },
                _ => SdfNode::Ellipsoid { radii },
            };
            lumps.push(turned(at, [360.0 * unit(&mut s), 360.0 * unit(&mut s), 0.0], 0.6 + unit(&mut s), piece));
        }
        let smooth = SdfNode::Combine { op: SdfOp::Union, blend: 1.5, of: lumps.split_off(60) };
        lumps.push(turned([0.0, 40.0, -12.0], [0.0; 3], -1.0, SdfNode::Combine { op: SdfOp::Difference, blend: 0.0, of: vec![SdfNode::Sphere { radius: 60.0 }, SdfNode::Sphere { radius: 3.0 }] }));
        lumps.push(SdfNode::deform(crate::shape::SdfDeform::Twist { rate: 0.1 }, at([0.0, 10.0, 12.0], SdfNode::Box { size: [2.0, 8.0, 3.0], radius: 0.0 })));
        let sketch = crate::sketches::SketchSolid::new(&[vec![vec![[0.5, -1.0], [4.0, -1.0], [3.0, 3.0]]]], crate::sketches::Form::Revolve { angle: 270.0 }).expect("a sketch");
        lumps.push(turned([-11.0, 26.0, 4.0], [30.0, 0.0, 15.0], 1.0, SdfNode::Sketch(std::sync::Arc::new(sketch))));
        let rest = SdfNode::Combine { op: SdfOp::Union, blend: 0.7, of: lumps };
        let core = SdfNode::Capsule { a: [0.0, -2.0, 0.0], b: [0.0, 62.0, 0.0], radius: 9.0 };
        let hard = SdfNode::Combine { op: SdfOp::Union, blend: 0.0, of: vec![smooth, at([0.0, 30.0, 14.0], SdfNode::Sphere { radius: 4.0 })] };
        let body = SdfNode::Combine { op: SdfOp::Union, blend: 2.5, of: vec![core, hard, rest] };
        let cut = SdfNode::Combine {
            op: SdfOp::Difference,
            blend: 1.0,
            of: vec![body, at([0.0, 20.0, 12.0], SdfNode::Sphere { radius: 5.0 }), at([30.0, 50.0, 0.0], SdfNode::Box { size: [4.0; 3], radius: 0.0 })],
        };
        let place = |x: f64| crate::shape::SdfPlace { at: [x, 64.0, 0.0], ..crate::shape::SdfPlace::IDENTITY };
        SdfNode::Combine {
            op: SdfOp::Union,
            blend: 0.5,
            of: vec![
                SdfNode::Combine { op: SdfOp::Intersection, blend: 0.8, of: vec![cut, at([0.0, 30.0, 0.0], SdfNode::Box { size: [60.0, 70.0, 60.0], radius: 0.0 })] },
                SdfNode::Shell { thickness: 0.6, of: Box::new(at([0.0, 70.0, 0.0], SdfNode::Torus { radius: 8.0, tube: 2.0 })) },
                SdfNode::Instance { places: vec![place(-10.0), place(0.0), place(10.0)], of: Box::new(SdfNode::Ellipsoid { radii: [3.0, 0.2, 2.0] }) },
            ],
        }
    }

    fn pieces(n: &SdfNode) -> usize {
        match n {
            SdfNode::Combine { of, .. } => of.iter().map(pieces).sum(),
            SdfNode::Shell { of, .. } | SdfNode::Round { of, .. } | SdfNode::Transform { of, .. } | SdfNode::Material { of, .. } | SdfNode::Id { of, .. } | SdfNode::Bone { of, .. } | SdfNode::Instance { of, .. } => pieces(of),
            _ => 1,
        }
    }

    /// The cut is exact: at random points of random boxes (bricks, slabs, near the surface and
    /// far from it, inside and out) the reduced node's value is the whole node's, bit for bit,
    /// and inside the cut's bounds; and a brick keeps under a third of the pieces.
    #[test]
    fn a_cut_keeps_every_value() {
        let mut s = 99u64;
        let (mut kept, mut boxes) = (0usize, 0usize);
        for seed in 0..4 {
            // extreme ellipsoids test the bounds; the count of pieces kept is for ones as a figure has
            let n = crowd(seed, if seed < 2 { [1.0, 6.0, 40.0] } else { [1.0, 2.0, 3.0] });
            let all = pieces(&n);
            for b in 0..400 {
                let size = [0.5, 2.0, 4.0, 12.0][b % 4];
                let c: P3 = [-30.0 + 60.0 * unit(&mut s), -10.0 + 90.0 * unit(&mut s), -30.0 + 60.0 * unit(&mut s)];
                let half = [size * (0.2 + unit(&mut s)), size * (0.2 + unit(&mut s)), size * (0.2 + unit(&mut s))];
                let region = (std::array::from_fn(|k| c[k] - half[k]), std::array::from_fn(|k| c[k] + half[k]));
                let got = cut(&n, &region);
                for _ in 0..40 {
                    let p = Vec3d::new(region.0[0] + (region.1[0] - region.0[0]) * unit(&mut s), region.0[1] + (region.1[1] - region.0[1]) * unit(&mut s), region.0[2] + (region.1[2] - region.0[2]) * unit(&mut s));
                    let (want, have) = (distance(&n, p), distance(&got.node, p));
                    assert_eq!(want.to_bits(), have.to_bits(), "{p:?} in {region:?}: {want} whole, {have} cut ({} of {all} pieces)", pieces(&got.node));
                    assert!(want >= got.lo && want <= got.hi, "{p:?}: {want} outside {}..{}", got.lo, got.hi);
                    assert_eq!(crate::shape::distance_slot(&n, p).1, crate::shape::distance_slot(&got.node, p).1);
                }
            }
            // what a 3 mm brick on the surface keeps
            for _ in 0..(if seed >= 2 { 2000 } else { 0 }) {
                let c: P3 = [-20.0 + 40.0 * unit(&mut s), 70.0 * unit(&mut s), -20.0 + 40.0 * unit(&mut s)];
                let got = cut(&n, &(c.map(|v| v - 1.5), c.map(|v| v + 1.5)));
                if got.lo <= 0.0 && got.hi >= 0.0 {
                    kept += pieces(&got.node);
                    boxes += 1;
                }
            }
        }
        let (mean, all) = (kept as f64 / boxes as f64, pieces(&crowd(2, [1.0, 2.0, 3.0])));
        assert!(mean < all as f64 / 3.0, "a 3 mm brick on the surface keeps {mean:.1} of {all} pieces on average");
    }

    /// A crowd of ellipsoids up to 100 to 1 with flat ones cut out of a ball and met with a box,
    /// a plate of one met with another, and a lens of two cut by a third, smooth and hard.
    fn steep_crowd(seed: u64) -> SdfNode {
        let thin = |p: P3, rotate: P3, radii: P3| turned(p, rotate, 1.0, SdfNode::Ellipsoid { radii });
        let carved = SdfNode::Combine {
            op: SdfOp::Difference,
            blend: 0.6,
            of: vec![at([0.0, 30.0, -20.0], SdfNode::Sphere { radius: 9.0 }), thin([0.0, 30.0, -14.0], [0.0, 30.0, 0.0], [12.0, 0.12, 9.0]), thin([3.0, 26.0, -20.0], [20.0, 0.0, 70.0], [11.0, 0.3, 0.2])],
        };
        let met = SdfNode::Combine { op: SdfOp::Intersection, blend: 0.8, of: vec![thin([0.0, 50.0, 20.0], [0.0, 0.0, 30.0], [16.0, 9.0, 12.0]), thin([0.0, 52.0, 20.0], [45.0, 0.0, 0.0], [30.0, 0.3, 30.0])] };
        let lens = SdfNode::Combine {
            op: SdfOp::Difference,
            blend: 0.0,
            of: vec![SdfNode::Combine { op: SdfOp::Intersection, blend: 0.0, of: vec![thin([20.0, 40.0, 0.0], [0.0; 3], [10.0, 1.0, 10.0]), thin([20.0, 40.5, 0.0], [0.0; 3], [9.0, 2.0, 9.0])] }, thin([20.0, 40.0, 0.0], [90.0, 0.0, 0.0], [0.1, 0.2, 10.0])],
        };
        SdfNode::Combine { op: SdfOp::Union, blend: 0.5, of: vec![crowd(seed, [1.0, 10.0, 100.0]), carved, met, lens] }
    }

    /// Over steep crowds, at random points of random regions: the field is never steeper than the
    /// region's [`slopes`] on the point's side of the surface, nor than its [`slopes_within`] a
    /// reach where the value lies within it, and the ball of a point's value over its box's slope
    /// at the values within it holds nothing across the surface, in the direction the field falls
    /// fastest and in others. (The differences step a ten-millionth: their rounding is the slack.)
    #[test]
    fn slopes_hold_over_steep_crowds() {
        let mut s = 4242u64;
        let (mut steps, mut windowed, mut balls) = ([0usize; 2], [0usize; 2], [0usize; 2]);
        let gradient = |n: &SdfNode, p: Vec3d, h: f64| -> f64 {
            let d = |e: Vec3d| (distance(n, p + e) - distance(n, p - e)) / (2.0 * h);
            let g = [d(Vec3d::new(h, 0.0, 0.0)), d(Vec3d::new(0.0, h, 0.0)), d(Vec3d::new(0.0, 0.0, h))];
            (g[0] * g[0] + g[1] * g[1] + g[2] * g[2]).sqrt()
        };
        for seed in 0..3 {
            let root = steep_crowd(seed);
            for b in 0..300 {
                let size = [0.5, 2.0, 4.0, 12.0][b % 4];
                let c: P3 = [-30.0 + 60.0 * unit(&mut s), -10.0 + 90.0 * unit(&mut s), -30.0 + 60.0 * unit(&mut s)];
                let half: P3 = std::array::from_fn(|_| size * (0.2 + unit(&mut s)));
                let region: Bounds = (std::array::from_fn(|k| c[k] - half[k]), std::array::from_fn(|k| c[k] + half[k]));
                let got = cut(&root, &region);
                let sl = slopes(&got.node, &region);
                let reach = [0.05, 0.3, 1.0, 3.0][(b / 4) % 4];
                let near = slopes_within(&got.node, &region, reach);
                for _ in 0..40 {
                    let p = Vec3d::new(region.0[0] + (region.1[0] - region.0[0]) * unit(&mut s), region.0[1] + (region.1[1] - region.0[1]) * unit(&mut s), region.0[2] + (region.1[2] - region.0[2]) * unit(&mut s));
                    let v = distance(&got.node, p);
                    let bound = if v > 0.0 { sl.outside } else { sl.inside };
                    let pa = [p.x, p.y, p.z];
                    let edge = (0..3).map(|k| (pa[k] - region.0[k]).min(region.1[k] - pa[k])).fold(f64::INFINITY, f64::min);
                    if !bound.is_finite() || v.abs() < 1e-3 || edge < 1e-6 {
                        continue;
                    }
                    let g = gradient(&got.node, p, 1e-7);
                    assert!(g <= bound * (1.0 + 1e-6) + 1e-6, "{p:?} in {region:?}: {g} over {bound} at {v}");
                    steps[(v > 0.0) as usize] += 1;
                    let within = if v > 0.0 { near.outside } else { near.inside };
                    if v.abs() <= reach && within.is_finite() {
                        assert!(g <= within * (1.0 + 1e-6) + 1e-6, "{p:?} in {region:?}: {g} over {within} within {reach} at {v}");
                        windowed[(v > 0.0) as usize] += 1;
                    }
                }
            }
            // balls about points near the surface, each on its own box of its value
            for _ in 0..1500 {
                let p = Vec3d::new(-30.0 + 60.0 * unit(&mut s), -10.0 + 90.0 * unit(&mut s), -30.0 + 60.0 * unit(&mut s));
                let v = distance(&root, p);
                if !(v.abs() > 1e-3 && v.abs() < 3.0) {
                    continue;
                }
                let around: Bounds = ([p.x - v.abs(), p.y - v.abs(), p.z - v.abs()], [p.x + v.abs(), p.y + v.abs(), p.z + v.abs()]);
                let got = cut(&root, &around);
                let sl = slopes_within(&got.node, &around, v.abs());
                let bound = if v > 0.0 { sl.outside } else { sl.inside };
                if !bound.is_finite() {
                    continue;
                }
                let r = v.abs() / bound.max(1.0) * (1.0 - 1e-9);
                let h = 1e-7;
                let d = |e: Vec3d| (distance(&root, p + e) - distance(&root, p - e)) / (2.0 * h);
                let fall = Vec3d::new(d(Vec3d::new(h, 0.0, 0.0)), d(Vec3d::new(0.0, h, 0.0)), d(Vec3d::new(0.0, 0.0, h)));
                let fall = fall * (-v.signum() / fall.length().max(1e-30));
                for k in 0..24 {
                    let dir = if k == 0 { fall } else { Vec3d::new(2.0 * unit(&mut s) - 1.0, 2.0 * unit(&mut s) - 1.0, 2.0 * unit(&mut s) - 1.0) };
                    let dir = dir * (1.0 / dir.length().max(1e-30));
                    for t in [1.0, 0.75, 0.5, 0.25] {
                        let q = p + dir * (r * t);
                        let w = distance(&root, q);
                        assert!(w * v.signum() > 0.0, "{w} at {q:?} within {r} of {p:?} (value {v}, slope {bound})");
                    }
                }
                balls[(v > 0.0) as usize] += 1;
            }
        }
        assert!(steps[0] > 1_000 && steps[1] > 10_000 && windowed[0] > 100 && windowed[1] > 1_000 && balls[0] > 20 && balls[1] > 60, "{steps:?} points, {windowed:?} within a reach, {balls:?} balls");
    }

    /// Brick-sized regions that [`culls_alike`] calls alike cull to the same node, over a vase
    /// beside a row of copies; neighbours inside one solid's reach are alike, ones across a
    /// copy's edge are not.
    #[test]
    fn alike_regions_cull_the_same() {
        use crate::shape::SdfPlace;
        let place = |x: f64| SdfPlace { at: [x, 4.0, 30.0], ..SdfPlace::IDENTITY };
        let n = SdfNode::Combine { op: SdfOp::Union, blend: 0.0, of: vec![
            vase(4.0),
            SdfNode::Instance { places: vec![place(-12.0), place(0.0), place(12.0)], of: Box::new(SdfNode::Sphere { radius: 3.0 }) },
        ] };
        let cell = |i: i32, j: i32, k: i32| -> Bounds {
            let lo = [i as f64 * 4.0, j as f64 * 4.0, k as f64 * 4.0];
            (lo, [lo[0] + 4.0, lo[1] + 4.0, lo[2] + 4.0])
        };
        let (mut alike, mut differ) = (0, 0);
        for i in -5..5 {
            for j in -1..12 {
                for k in -4..10 {
                    let (a, b) = (cell(i, j, k), cell(i + 1, j, k));
                    if !culls_alike(&n, &a, &b) {
                        differ += 1;
                        continue;
                    }
                    alike += 1;
                    match (cull(&n, &a), cull(&n, &b)) {
                        (None, None) => {}
                        (Some(x), Some(y)) => assert!(same(&x, &y), "{a:?} and {b:?} cull apart"),
                        (x, y) => panic!("{a:?} and {b:?}: {} and {}", x.is_some(), y.is_some()),
                    }
                }
            }
        }
        assert!(alike > 0 && differ > 0, "alike {alike}, differ {differ}");
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
            let m = crate::field_mesh::mesh_field(shape.field(), Vec3d::new(b.0[0], b.0[1], b.0[2]), Vec3d::new(b.1[0], b.1[1], b.1[2]), cell, shape.clearance());
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

    /// `n`'s value clamped to `-w..=w`, and a windowed cut's (None: `+w`).
    fn clamped(n: Option<&SdfNode>, p: Vec3d, w: f64) -> f64 {
        n.map_or(w, |n| distance(n, p).clamp(-w, w))
    }

    /// A windowed cut against the tree at one point: the clamped values the same bits, and the
    /// sides kept (in the window: the same bits; at or over its top: at or over it; at or under
    /// its foot: at or under it).
    fn holds(n: &SdfNode, cut: &Option<Cut>, p: Vec3d, w: f64, what: &str) {
        let want = distance(n, p);
        let node = cut.as_ref().map(|c| &c.node);
        assert_eq!(clamped(Some(n), p, w).to_bits(), clamped(node, p, w).to_bits(), "{what}: {p:?} at window {w}: {want} whole, {:?} cut", node.map(|c| distance(c, p)));
        let have = node.map_or(f64::INFINITY, |c| distance(c, p));
        if want > -w && want < w {
            assert_eq!(want.to_bits(), have.to_bits(), "{what}: {p:?} within {w}");
        } else if want >= w {
            assert!(have >= w, "{what}: {p:?} outside, {want} whole, {have} cut");
        } else {
            assert!(have <= -w, "{what}: {p:?} inside, {want} whole, {have} cut");
        }
        if let Some(c) = cut {
            assert!(have >= c.lo && have <= c.hi, "{what}: {p:?}: {have} outside the cut's {}..{}", c.lo, c.hi);
        }
    }

    /// A windowed cut is the tree clamped to its window, bit for bit, over random boxes of
    /// crowds (smooth and hard unions nested three deep and more, an intersection, differences,
    /// a twist, a shell, copies, frames turned inside out, flat ellipsoids) and of a figure with
    /// a taper, at windows from a third of a millimetre to 8 and at a window that is some point's
    /// value exactly; and a far box keeps fewer pieces than the plain cut does.
    #[test]
    fn a_windowed_cut_is_the_tree_clamped_to_its_window() {
        let mut s = 7u64;
        let tapered = SdfNode::Combine {
            op: SdfOp::Union,
            blend: 1.0,
            of: vec![Figure::new().node(), SdfNode::deform(crate::shape::SdfDeform::Taper { rate: 0.05 }, at([0.0, 30.0, 0.0], SdfNode::Box { size: [6.0, 10.0, 6.0], radius: 0.5 }))],
        };
        let trees = [crowd(0, [1.0, 6.0, 40.0]), crowd(3, [1.0, 2.0, 3.0]), steep_crowd(5), tapered];
        let (mut plain, mut windowed, mut none) = (0usize, 0usize, 0usize);
        for (t, n) in trees.iter().enumerate() {
            for (j, &w) in [0.3, 1.0, 3.0, 8.0].iter().enumerate() {
                let win = window(n, w);
                for b in 0..150 {
                    let size = [0.5, 2.0, 4.0, 16.0][b % 4];
                    let c: P3 = [-40.0 + 80.0 * unit(&mut s), -15.0 + 100.0 * unit(&mut s), -40.0 + 80.0 * unit(&mut s)];
                    let half: P3 = std::array::from_fn(|_| size * (0.2 + unit(&mut s)));
                    let region: Bounds = (std::array::from_fn(|k| c[k] - half[k]), std::array::from_fn(|k| c[k] + half[k]));
                    let pick = |s: &mut u64| Vec3d::new(region.0[0] + (region.1[0] - region.0[0]) * unit(s), region.0[1] + (region.1[1] - region.0[1]) * unit(s), region.0[2] + (region.1[2] - region.0[2]) * unit(s));
                    let got = cut_within(n, &win, &region);
                    let what = format!("tree {t} window {w} box {b}");
                    for _ in 0..30 {
                        let p = pick(&mut s);
                        holds(n, &got, p, w, &what);
                    }
                    // a window that is a point's value, either sign: the point sits on its edge
                    let p = pick(&mut s);
                    let edge = distance(n, p).abs();
                    if edge > 1e-3 && edge < 30.0 {
                        let cut = cut_within(n, &window(n, edge), &region);
                        holds(n, &cut, p, edge, &format!("{what} at its value"));
                        for _ in 0..10 {
                            holds(n, &cut, pick(&mut s), edge, &format!("{what} at its value"));
                        }
                    }
                    if j == 3 && size >= 16.0 {
                        plain += pieces(&cut(n, &region).node);
                        windowed += got.as_ref().map_or(0, |c| pieces(&c.node));
                        none += got.is_none() as usize;
                    }
                }
            }
        }
        assert!(none > 0, "some boxes lie outside every piece's window");
        assert!(10 * windowed < 9 * plain, "the window leaves pieces out: {windowed} of the plain cut's {plain}");
    }

    /// Each rule that leaves a piece out, against the tree clamped: a union's operand only past
    /// its window's top widened by the blend, a cutter only where it cannot win the max (its
    /// negation under the window's foot), an intersection's operand never, a warp only by its
    /// placed box; and a union whose every operand is left out reads `+w`.
    #[test]
    fn a_windowed_cut_leaves_out_only_what_cannot_win() {
        let w = 2.0;
        let check = |n: &SdfNode, region: &Bounds, what: &str| -> Option<Cut> {
            let got = cut_within(n, &window(n, w), region);
            for p in grid(region, 6) {
                holds(n, &got, p, w, what);
            }
            got
        };
        let cube = |c: P3| -> Bounds { (c.map(|v| v - 0.5), c.map(|v| v + 0.5)) };
        // a union: the ball 3 past the long box is left out of a hard fold at window 2 (the
        // plain cut keeps it: the first ball reads up to 6 there), kept in a smooth one whose two
        // steps of blend 2 widen its window by 5
        let pair = |k: f64| SdfNode::Combine { op: SdfOp::Union, blend: k, of: vec![SdfNode::Sphere { radius: 3.0 }, at([15.0, 0.0, 0.0], SdfNode::Sphere { radius: 3.0 }), at([0.0, 0.0, 40.0], SdfNode::Sphere { radius: 1.0 })] };
        let long: Bounds = ([2.0, -0.5, -0.5], [9.0, 0.5, 0.5]);
        assert_eq!(pieces(&cut(&pair(0.0), &long).node), 2, "the plain cut keeps the far ball");
        assert_eq!(pieces(&check(&pair(0.0), &long, "hard union").unwrap().node), 1);
        assert_eq!(pieces(&check(&pair(2.0), &long, "smooth union").unwrap().node), 2, "the blend widens the window");
        // every operand left out: outside, +w
        assert!(check(&pair(0.0), &cube([60.0, 0.0, 0.0]), "far union").is_none());
        // a difference: deep in the base a cutter 8 away cannot win the max at window 2 (its
        // negation is under -2 where the base is), though it moves the value there
        let carved = SdfNode::Combine { op: SdfOp::Difference, blend: 0.5, of: vec![SdfNode::Sphere { radius: 30.0 }, at([0.0, 0.0, 12.0], SdfNode::Sphere { radius: 3.0 })] };
        let o = Vec3d::new(0.0, 0.0, 0.0);
        assert!(distance(&carved, o) != distance(&SdfNode::Sphere { radius: 30.0 }, o), "the cutter moves the deep value");
        assert_eq!(pieces(&cut(&carved, &cube([0.0; 3])).node), 2, "the plain cut keeps the cutter");
        assert_eq!(pieces(&check(&carved, &cube([0.0; 3]), "carved deep").unwrap().node), 1, "the cutter is left out");
        assert_eq!(pieces(&check(&carved, &cube([0.0, 0.0, 7.5]), "carved near").unwrap().node), 2, "a box near the cutter keeps it");
        // an intersection never leaves an operand out: far from its second operand that one is
        // the value (outside), so the cut reads +w there and never the first alone
        let far = SdfNode::Combine { op: SdfOp::Intersection, blend: 0.5, of: vec![SdfNode::Sphere { radius: 30.0 }, at([100.0, 0.0, 0.0], SdfNode::Box { size: [20.0; 3], radius: 0.0 })] };
        assert!(check(&far, &cube([0.0; 3]), "far intersection").is_none());
        let met = SdfNode::Combine { op: SdfOp::Intersection, blend: 0.8, of: vec![SdfNode::Sphere { radius: 30.0 }, at([0.0, 0.0, 40.0], SdfNode::Box { size: [100.0; 3], radius: 0.0 })] };
        check(&met, &cube([0.0, 0.0, -9.5]), "intersection at its face");
        // a bar twisted about its own axis, 5.5 past the long box: left out by its placed box
        // (the plain cut keeps it), kept in a box near both
        let bar = SdfNode::Combine { op: SdfOp::Union, blend: 0.5, of: vec![SdfNode::Sphere { radius: 3.0 }, SdfNode::deform(crate::shape::SdfDeform::Twist { rate: 0.2 }, at([0.0, 10.0, 0.0], SdfNode::Box { size: [2.0, 8.0, 2.0], radius: 0.0 }))] };
        assert_eq!(pieces(&cut(&bar, &long).node), 2, "the plain cut keeps the bar");
        assert_eq!(pieces(&check(&bar, &long, "twist far").unwrap().node), 1);
        assert_eq!(pieces(&check(&bar, &cube([0.0, 4.5, 0.0]), "twist near").unwrap().node), 2);
    }

    /// A piece of a group moved far from a big region still changes the group's box, and with it
    /// whether the cut leaves the whole group out there, while another piece of the group, whose
    /// own window reaches past the group's, stays kept: the two trees reduce differently, their
    /// clamped values alike. The change reaches the region. Its own boxes alone do not.
    #[test]
    fn a_group_whose_box_moves_with_a_piece_reaches_the_regions_it_leaves() {
        // (turned, so the region's box in its frame holds the ball's centre: its bounds say nothing there)
        let (w, ball) = (2.0, |at: P3| SdfNode::Transform { at, rotate: [45.0, 0.0, 0.0], scale: 1.0, of: Box::new(SdfNode::Sphere { radius: 1.0 }) });
        let tree = |far: P3| SdfNode::Combine { op: SdfOp::Union, blend: 0.0, of: vec![ball([0.0, 30.0, 0.0]), SdfNode::Combine { op: SdfOp::Union, blend: 2.0, of: vec![ball([0.0; 3]), ball(far)] }] };
        let (old, new) = (tree([20.0, 20.0, 0.0]), tree([20.0, 0.0, 0.0]));
        let (wo, wn) = (window(&old, w), window(&new, w));
        let region: Bounds = ([-8.0, 4.0, -8.0], [8.0, 5.0, 8.0]);
        let (a, b) = (cut_within(&old, &wo, &region), cut_within(&new, &wn, &region));
        assert!(a.is_some() && b.is_none(), "the group's box leaves the region: {} {}", a.is_some(), b.is_some());
        for p in grid(&region, 3) {
            assert_eq!(clamped(a.as_ref().map(|c| &c.node), p, w).to_bits(), clamped(b.as_ref().map(|c| &c.node), p, w).to_bits(), "{p:?}");
        }
        let change = changed_within(&old, &wo, &new, &wn).expect("a bounded change");
        assert!(change.near.iter().all(|r| misses(&region, r)), "the moved piece's own boxes miss the region: {change:?}");
        assert!(reaches(&region, &change), "the change reaches a region its group's box leaves: {change:?}");
        assert!(!reaches(&([-8.0, 40.0, -8.0], [8.0, 41.0, 8.0]), &change), "and not one past both its boxes: {change:?}");
    }

    /// Random edits of [`Figure`]: every 16 mm by 1 mm cell and every random box the change does
    /// not reach ([`changed_within`]) reduces the old and the new tree to the same node (or both
    /// to None), so to the same clamped values, bit for bit; and the change leaves most cells out.
    #[test]
    fn a_windowed_cut_outside_the_change_is_the_same_tree() {
        let w = 8.0;
        let (mut kept, mut cells) = (0usize, 0usize);
        for seed in 1..=4u64 {
            let mut r = Rng(0x2545_f491_4f6c_dd1d ^ (seed << 40));
            let mut fig = Figure::new();
            for e in 0..30 {
                let old = fig.node();
                let what = fig.edit(&mut r);
                let new = fig.node();
                let (wo, wn) = (window(&old, w), window(&new, w));
                let boxes = changed_within(&old, &wo, &new, &wn).unwrap_or_else(|| panic!("seed {seed} edit {e}: a {what} edit reaches everywhere"));
                let mut regions: Vec<Bounds> = Vec::new();
                for y in -3..3 {
                    for x in -3..3 {
                        for z in -40..40 {
                            regions.push(([x as f64 * 16.0, z as f64, y as f64 * 16.0], [(x + 1) as f64 * 16.0, z as f64 + 1.0, (y + 1) as f64 * 16.0]));
                        }
                    }
                }
                for _ in 0..60 {
                    let c: P3 = [-30.0 + 60.0 * r.unit(), -35.0 + 70.0 * r.unit(), -30.0 + 60.0 * r.unit()];
                    let h = 0.5 + 8.0 * r.unit();
                    regions.push((c.map(|v| v - h), c.map(|v| v + h)));
                }
                for region in &regions {
                    cells += 1;
                    if reaches(region, &boxes) {
                        continue;
                    }
                    kept += 1;
                    let (a, b) = (cut_within(&old, &wo, region), cut_within(&new, &wn, region));
                    match (&a, &b) {
                        (None, None) => {}
                        (Some(x), Some(y)) => assert!(same(&x.node, &y.node), "seed {seed} edit {e}: a {what} edit changed the cut of {region:?}, which it does not reach"),
                        _ => panic!("seed {seed} edit {e}: a {what} edit changed whether {region:?} lies outside"),
                    }
                    for p in grid(region, 3) {
                        let (x, y) = (clamped(a.as_ref().map(|c| &c.node), p, w), clamped(b.as_ref().map(|c| &c.node), p, w));
                        assert_eq!(x.to_bits(), y.to_bits(), "seed {seed} edit {e}: {p:?}");
                        assert_eq!(clamped(Some(&old), p, w).to_bits(), clamped(Some(&new), p, w).to_bits(), "seed {seed} edit {e}: a {what} edit changed {p:?}'s clamped value, which it does not reach");
                    }
                }
            }
        }
        assert!(2 * kept > cells, "the change leaves most cells out: {kept} of {cells}");
    }
}
