//! The box a field's solid fills, without meshing it, proven to hold the solid: each side of a
//! target frame starts where a box that holds the solid by construction puts it (the field's
//! own box; a shape's tree box, [`tight_bounds`]) and moves in only as far as the field proves,
//! never to the farthest solid point a search happened to find.
//!
//! A side is proven by columns across it: the region beyond a column's top is covered by balls
//! the field proves empty (a value `v` at a point clears the ball of radius `v / L` round it,
//! `L` the field's Lipschitz bound). A ball centred on a column's axis clears the whole column
//! over the height its radius spans past the column's half-diagonal; centred farther out along
//! the side's direction it reaches a flat side across a wider column, so a flat side costs its
//! area over the square root of its gap, not over its square. A column the balls cannot take
//! down first looks down its axis for solid (a witness: the side's goal rises with it), then
//! splits. A side ends `tol` past a solid point once every column is down to three quarters of a
//! `tol` past it: at most `tol` past the solid, and never past the box it started from (an exact
//! tree box's side stays exact). Each side also reports its witness: the solid reaches at least there, so
//! a part rests on its bottom's witness, its box's bottom at most what the proof left open below.
//! A field steeper than a distance (`L > 1`) closes a flat side only by balls of radius about
//! `tol / (L - 1)`, so the bottom is proven first and with the largest budget. Most such sides
//! are a distance all the same, a kernel only carving what lies inside it: the tree without its
//! kernels ([`outer_tree`]) clears those with its own slope of 1, and the field is read only near
//! the solid. A tree whose kernels are grids' reads ([`crate::SdfKernel::grid_read`]) has no balls
//! to spend on those: the grids' samples bound their solid outright ([`known_side`]).

use crate::bounds::{blend_reach, grow, meet, out_of, rot, tight_bounds, Bounds};
use crate::lipschitz::{has_grid_read, has_kernel};
use crate::sdf::Sdf3;
use crate::shape::{distance, SdfNode, SdfOp, SdfShape, P3};
use crate::strokes::Strokes;
use makepad_csg_math::Vec3d;
use std::collections::BinaryHeap;

/// Row-major affine `p' = m * p`.
pub type Affine = [[f64; 4]; 3];

/// Field evaluations one side may spend; past them it answers with what it proved (the highest
/// column top left), a bound looser than `tol` but still one.
const BUDGET: usize = 250_000;

/// The bottom's (the frame's -z: the side a part rests on), proven first: what it leaves open is
/// how far the part may sit into the bed. A field steeper than a distance (`L > 1`) closes a flat
/// side only by balls of radius about `tol / (L - 1)`, so its gap shrinks as the root of this.
const BOTTOM_BUDGET: usize = 4 * BUDGET;

/// A grid's walk spends one part in this of a side's evaluations: a piece of a cell costs about
/// what a field read does, and past that a side turned from the grid's axes (an axis's closes in
/// a piece) gets tighter than its cells' sizes only for a price no edit should pay.
const WALK_SHARE: usize = 64;

/// A solid's box in a frame ([`shape_bounds`], [`surface_bounds`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Extent {
    /// No solid point lies outside `min..max`.
    pub min: P3,
    pub max: P3,
    /// Per side, the solid point its proof found nearest it (a side whose proof found none: the
    /// farthest any found), so the solid spans at least this box. A side's gap to `min..max` is
    /// what its proof left open: at most `tol` where it closed. A side no change reaches keeps
    /// both to the bit. None: no solid point found.
    pub found: Option<(P3, P3)>,
}

/// The axis-aligned box, in the frame `m` maps into, that holds the solid (`field <= 0`) inside
/// the field's box `min..max`: each side at most `tol` past the solid (where its budget runs out:
/// what was proven). `lipschitz` bounds the field's slope (1 for a distance); a field steeper than
/// a distance is read only inside its box. None when the box holds no solid.
pub fn surface_bounds(field: &dyn Sdf3, min: P3, max: P3, lipschitz: f64, m: &Affine, tol: f64) -> Option<Extent> {
    let lip = lipschitz.max(1.0);
    let region = (min, max);
    let centres = (lip > 1.0).then_some(region);
    let f = |p: P3| field.distance(v3(p));
    frame_bounds(m, |dn, budget| Prover { f: &f, outer: None, lip, clip: region, centres, tol, budget }.side(dn, f64::NEG_INFINITY))
}

/// [`surface_bounds`] of a shape's solid: each side from the shape's tree box ([`tight_bounds`];
/// the shape's box for a tree without one) moved in as far as proven. A sculpt's side is its
/// unsculpted node's (no stroke changes its proof, so a stroke that cannot reach a side leaves it
/// to the bit) unless a stroke can move it: the raised tubes are proven past it, and a lowering
/// stroke over the solid point that bounds it proves it again.
pub fn shape_bounds(shape: &SdfShape, m: &Affine, tol: f64) -> Option<Extent> {
    let lip = std::cell::OnceCell::new();
    let within = (shape.min, shape.max);
    frame_bounds(m, |dn, budget| {
        // A tree its grids' samples bound has its side from them: no proof could close it for less.
        if let Some(k) = has_grid_read(&shape.root).then(|| grid_side(&shape.root, dn, tol, budget)).flatten() {
            if k.top == f64::NEG_INFINITY {
                return None;
            }
            let top = k.top.min(support(&clip_of(&shape.root, &within), dn));
            return Some(match k.seed {
                Some((s, p)) if s + 0.75 * tol >= top => Side { bound: (s + tol).min(top.max(s)), witness: Some(p) },
                seed => Side { bound: top, witness: seed.map(|(_, p)| p) },
            });
        }
        let lip = *lip.get_or_init(|| shape.lipschitz().max(1.0));
        node_side(&shape.root, within, lip, (lip > 1.0).then_some(within), dn, tol, budget)
    })
}

/// What a tree holding grid reads knows of a side without its proof.
#[derive(Clone, Copy, Debug)]
struct Known {
    /// Nothing solid lies past this along the direction (`NEG_INFINITY`: nothing solid at all).
    top: f64,
    /// The highest solid point found, in the root's frame, and how high in the root's frame.
    seed: Option<(f64, P3)>,
}

/// The side `dn` (unit) of `root`, from its grid reads' samples and its kernel-free operands' proofs.
/// None: the tree holds something else these do not bound, so its proof is all there is.
fn grid_side(root: &SdfNode, dn: P3, tol: f64, budget: usize) -> Option<Known> {
    let accept = |p: P3| distance(root, v3(p)) <= 0.0;
    known_side(root, dn, f64::INFINITY, tol, budget, &|p| p, &|p| (dot(dn, p), accept(p)), &|_, _| false)
}

fn dot(a: P3, b: P3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// [`grid_side`] at a node: `dn` and `ceiling` (the most what surrounds it leaves of its solid) and
/// `tol` in its frame, `to_root` into the root's, `solid` a root's point's height and whether the
/// whole tree is solid there, `clear` whether what surrounds it leaves nothing of it in a ball.
#[allow(clippy::too_many_arguments)]
fn known_side(n: &SdfNode, dn: P3, ceiling: f64, tol: f64, budget: usize, to_root: &dyn Fn(P3) -> P3, solid: &dyn Fn(P3) -> (f64, bool), clear: &dyn Fn(P3, f64) -> bool) -> Option<Known> {
    let seed = |w: P3| {
        let p = to_root(w);
        let (z, inside) = solid(p);
        inside.then_some((z, p))
    };
    if !has_kernel(n) {
        // a distance: its own proof, which its slope of 1 closes in a few reads
        let f = |p: P3| distance(n, v3(p));
        let prover = Prover { f: &f, outer: None, lip: 1.0, clip: tight_bounds(n)?, centres: None, tol, budget };
        return Some(prover.side(dn, f64::NEG_INFINITY).map_or(Known { top: f64::NEG_INFINITY, seed: None }, |s| Known { top: s.bound, seed: s.witness.and_then(seed) }));
    }
    let higher = |a: Option<(f64, P3)>, b: Option<(f64, P3)>| match (a, b) {
        (Some(x), Some(y)) => Some(if y.0 > x.0 { y } else { x }),
        (x, y) => x.or(y),
    };
    match n {
        SdfNode::Kernel(k) => {
            let grid = k.solid.as_ref()?;
            let mut witness = None;
            let side = grid.solid_side(dn, ceiling, 0.75 * tol, budget / WALK_SHARE, clear, &mut |w| {
                let found = seed(w);
                witness = higher(witness, found);
                found.is_some()
            });
            Some(side.map_or(Known { top: f64::NEG_INFINITY, seed: None }, |s| Known { top: s.top, seed: witness }))
        }
        SdfNode::Transform { at, rotate, scale, of } => {
            let r = rot(*rotate);
            let inner: P3 = std::array::from_fn(|i| (0..3).map(|k| r[k][i] * dn[k]).sum());
            let out = out_of(*at, *rotate, *scale);
            let along = dot(dn, *at);
            let k = known_side(of, inner, (ceiling - along) / scale, tol / scale, budget, &|p| to_root(out(p)), solid, &|p, r| clear(out(p), r * scale))?;
            Some(Known { top: if k.top == f64::NEG_INFINITY { k.top } else { k.top * scale + along }, seed: k.seed })
        }
        SdfNode::Id { of, .. } | SdfNode::Material { of, .. } | SdfNode::Bone { of, .. } => known_side(of, dn, ceiling, tol, budget, to_root, solid, clear),
        SdfNode::Combine { op: SdfOp::Union, blend, of } => {
            let mut all = Known { top: f64::NEG_INFINITY, seed: None };
            for c in of {
                let k = known_side(c, dn, ceiling, tol, budget, to_root, solid, clear)?;
                all = Known { top: all.top.max(k.top), seed: higher(all.seed, k.seed) };
            }
            if *blend <= 0.0 || all.top == f64::NEG_INFINITY {
                return Some(all);
            }
            // Each smooth step sags at most a blend reach past its operands, and with two operands
            // only where both come within a blend and a quarter.
            let sagged = all.top + blend_reach(*blend) * of.len().saturating_sub(1) as f64;
            let top = match of.as_slice() {
                [x, y] => match meet(&grow(tight_bounds(x)?, 1.25 * blend), &grow(tight_bounds(y)?, 1.25 * blend)) {
                    Some(both) => support(&both, dn).min(sagged).max(all.top),
                    None => all.top,
                },
                _ => sagged,
            };
            Some(Known { top, ..all })
        }
        // Solid in all of them: those without kernels first, whose sides the reads stop under.
        SdfNode::Combine { op: SdfOp::Intersection, of, .. } => {
            let mut all = Known { top: ceiling, seed: None };
            let (plain, read): (Vec<&SdfNode>, Vec<&SdfNode>) = of.iter().partition(|c| !has_kernel(c));
            // a smooth maximum is at least either operand, so past one's ball nothing is left
            let cleared = |p: P3, r: f64| clear(p, r) || plain.iter().any(|c| distance(c, v3(p)) > r);
            for c in plain.iter().chain(&read) {
                let k = known_side(c, dn, all.top, tol, budget, to_root, solid, &cleared)?;
                all = Known { top: all.top.min(k.top), seed: higher(all.seed, k.seed) };
            }
            Some(all)
        }
        // A cut only takes away: deep inside one nothing is left.
        SdfNode::Combine { op: SdfOp::Difference, of, .. } => {
            let cleared = |p: P3, r: f64| clear(p, r) || of[1..].iter().any(|c| !has_kernel(c) && -distance(c, v3(p)) > r);
            known_side(&of[0], dn, ceiling, tol, budget, to_root, solid, &cleared)
        }
        _ => None,
    }
}

/// Per axis of `m`'s frame and per sign, `side` along that unit direction (in the field's frame,
/// with the evaluations it may spend), read back in the frame's coordinates: the bottom first.
fn frame_bounds(m: &Affine, mut side: impl FnMut(P3, usize) -> Option<Side>) -> Option<Extent> {
    let mut lo = [0.0; 3];
    let mut hi = [0.0; 3];
    // each side's witness, and the box of all of them
    let mut sides = [[None; 3]; 2];
    let mut all: Option<(P3, P3)> = None;
    for (axis, sign) in [(2, -1.0), (2, 1.0), (0, -1.0), (0, 1.0), (1, -1.0), (1, 1.0)] {
        let d = [m[axis][0], m[axis][1], m[axis][2]];
        let len = norm(d);
        if len == 0.0 {
            // a row that reads nothing: the solid's every point maps to its offset
            lo[axis] = m[axis][3];
            hi[axis] = m[axis][3];
            continue;
        }
        let s = side(d.map(|x| sign * x / len), if sign < 0.0 && axis == 2 { BOTTOM_BUDGET } else { BUDGET })?;
        let at = sign * s.bound * len + m[axis][3];
        if sign > 0.0 { hi[axis] = at } else { lo[axis] = at }
        if let Some(w) = s.witness {
            let q: P3 = std::array::from_fn(|r| m[r][0] * w[0] + m[r][1] * w[1] + m[r][2] * w[2] + m[r][3]);
            sides[(sign > 0.0) as usize][axis] = Some(q[axis]);
            all = Some(all.map_or((q, q), |(a, b)| (std::array::from_fn(|k| a[k].min(q[k])), std::array::from_fn(|k| b[k].max(q[k])))));
        }
    }
    let found = all.map(|(a, b)| (std::array::from_fn(|k| sides[0][k].unwrap_or(a[k])), std::array::from_fn(|k| sides[1][k].unwrap_or(b[k]))));
    Some(Extent { min: lo, max: hi, found })
}

/// One side of a solid along a unit direction.
#[derive(Clone, Copy, Debug)]
struct Side {
    /// No solid point reaches past this along the direction.
    bound: f64,
    /// The solid point farthest along the direction found: within `tol` of `bound` where the
    /// proof closed (None: none found, the bound being the floor it was asked to prove past or
    /// what a budget left).
    witness: Option<P3>,
}

/// Side `dn` of node `n`'s solid (`within`: where a node without a tree box has it).
fn node_side(n: &SdfNode, within: Bounds, lip: f64, centres: Option<Bounds>, dn: P3, tol: f64, budget: usize) -> Option<Side> {
    let f = |p: P3| distance(n, v3(p));
    let tree = (lip > 1.0).then(|| outer_tree(n)).flatten();
    let g = tree.as_ref().map(|t| move |p: P3| distance(t, v3(p)));
    let outer = g.as_ref().map(|g| g as &dyn Fn(P3) -> f64);
    // A field steeper than a distance is read only inside `within`, so a side starts no farther
    // out: no ball could bring it in from past there.
    let readable = |b: Bounds| centres.map_or(Some(b), |c| meet(&b, &c));
    let whole = || Prover { f: &f, outer, lip, clip: readable(clip_of(n, &within))?, centres, tol, budget }.side(dn, f64::NEG_INFINITY);
    let SdfNode::Sculpt { strokes, of } = n else {
        return whole();
    };
    // Raising adds to the solid only within its tubes' reach and lowering takes away only within
    // its own: outside them the unsculpted solid stays, so its side holds while its witness is
    // out of every lowering's reach.
    let base = match node_side(of, within, lip, centres, dn, tol, budget) {
        Some(b) if b.witness.is_some_and(|w| !lowered(strokes, w, tol)) => Some(b),
        None => None,
        Some(_) => return whole(),
    };
    let Some(raised) = strokes.raised_bounds().and_then(readable) else {
        return base;
    };
    let floor = base.map_or(f64::NEG_INFINITY, |b| b.bound);
    if support(&raised, dn) <= floor {
        return base;
    }
    match (Prover { f: &f, outer, lip, clip: raised, centres, tol, budget }.side(dn, floor)) {
        // the base's witness is solid still, should the raised proof have found none
        Some(s) if s.bound > floor => Some(Side { witness: s.witness.or(base.and_then(|b| b.witness)), ..s }),
        _ => base,
    }
}

/// Whether a lowering stroke (or a crease) of `strokes` can have taken solid away at `p`.
fn lowered(strokes: &Strokes, p: P3, tol: f64) -> bool {
    strokes.applied().any(|(si, s)| s.tube().is_some_and(|t| !t.raise) && strokes.boxes(si..si + 1).iter().any(|b| (0..3).all(|k| p[k] >= b.0[k] - tol && p[k] <= b.1[k] + tol)))
}

/// The node's tree box ([`tight_bounds`]), or `within` for a node without one. (Never the two
/// met: a sculpt's shape box grows with its strokes, and its unsculpted node's sides must not.)
fn clip_of(n: &SdfNode, within: &Bounds) -> Bounds {
    tight_bounds(n).unwrap_or(*within)
}

/// The most `d · p` reaches over box `b`.
fn support(b: &Bounds, d: P3) -> f64 {
    (0..3).map(|k| d[k] * if d[k] > 0.0 { b.1[k] } else { b.0[k] }).sum()
}

fn v3(p: P3) -> Vec3d {
    Vec3d::new(p[0], p[1], p[2])
}

fn norm(d: P3) -> f64 {
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

fn cross(a: P3, b: P3) -> P3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn inside(b: &Bounds, p: P3) -> bool {
    (0..3).all(|k| p[k] >= b.0[k] && p[k] <= b.1[k])
}

/// The exact distance to box `b` (negative inside).
fn box_distance(b: &Bounds, p: P3) -> f64 {
    let q: P3 = std::array::from_fn(|k| (p[k] - 0.5 * (b.0[k] + b.1[k])).abs() - 0.5 * (b.1[k] - b.0[k]));
    norm(q.map(|x| x.max(0.0))) + q[0].max(q[1]).max(q[2]).min(0.0)
}

/// A column of a side: the cell `a ± ha, b ± hb` across it, and the top down to which
/// everything along the side beyond it is proven empty.
#[derive(Clone, Copy)]
struct Col {
    top: f64,
    a: f64,
    b: f64,
    ha: f64,
    hb: f64,
    /// How far out along the side the ball that took its parent down was centred (a flat side's
    /// best centre is about as far out for every column over it): tried first.
    out: f64,
}

impl PartialEq for Col {
    fn eq(&self, o: &Self) -> bool {
        self.top == o.top
    }
}
impl Eq for Col {}
impl PartialOrd for Col {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Col {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        self.top.total_cmp(&o.top)
    }
}

/// A tree without kernels whose field is nowhere above `n`'s, so its solid holds `n`'s and its
/// slope is a distance's whatever the kernels' bound. An intersection keeps the operands that have
/// one (a smooth maximum is at least either operand) and a difference only the cuts without
/// kernels (cutting less leaves more solid); what stays is monotone in its operands. None where a
/// kernel's solid has no such limit: a union with one, or one under a shell, sculpt or texture.
fn outer_tree(n: &SdfNode) -> Option<SdfNode> {
    if !has_kernel(n) {
        return Some(n.clone());
    }
    let inner = |of: &SdfNode| outer_tree(of).map(Box::new);
    match n {
        SdfNode::Combine { op: SdfOp::Union, blend, of } => {
            Some(SdfNode::Combine { op: SdfOp::Union, blend: *blend, of: of.iter().map(outer_tree).collect::<Option<_>>()? })
        }
        SdfNode::Combine { op: SdfOp::Intersection, blend, of } => {
            let kept: Vec<SdfNode> = of.iter().filter_map(outer_tree).collect();
            (!kept.is_empty()).then_some(SdfNode::Combine { op: SdfOp::Intersection, blend: *blend, of: kept })
        }
        SdfNode::Combine { op: SdfOp::Difference, blend, of } => {
            let kept = std::iter::once(outer_tree(&of[0])?).chain(of[1..].iter().filter(|c| !has_kernel(c)).cloned()).collect();
            Some(SdfNode::Combine { op: SdfOp::Difference, blend: *blend, of: kept })
        }
        SdfNode::Round { radius, of } => Some(SdfNode::Round { radius: *radius, of: inner(of)? }),
        SdfNode::Transform { at, rotate, scale, of } => Some(SdfNode::Transform { at: *at, rotate: *rotate, scale: *scale, of: inner(of)? }),
        SdfNode::Instance { places, of } => Some(SdfNode::Instance { places: places.clone(), of: inner(of)? }),
        SdfNode::Deform { deform, lipschitz, of } => Some(SdfNode::Deform { deform: *deform, lipschitz: *lipschitz, of: inner(of)? }),
        SdfNode::Material { of, .. } | SdfNode::Bone { of, .. } | SdfNode::Id { of, .. } => outer_tree(of),
        _ => None,
    }
}

/// The proof of one side over a field cut to a box.
struct Prover<'a> {
    f: &'a dyn Fn(P3) -> f64,
    /// [`outer_tree`]'s field: at most `f`, of slope 1, and readable anywhere.
    outer: Option<&'a dyn Fn(P3) -> f64>,
    /// The field's Lipschitz bound (at least 1: the cut's box is a distance).
    lip: f64,
    /// The solid that counts: the field's inside this box.
    clip: Bounds,
    /// Where the field's value may centre a ball (None: anywhere); elsewhere only the box's
    /// distance does.
    centres: Option<Bounds>,
    tol: f64,
    /// Field evaluations it may spend.
    budget: usize,
}

impl Prover<'_> {
    /// The field cut to the box: its solid is the field's inside the box.
    fn value(&self, p: P3) -> f64 {
        (self.f)(p).max(box_distance(&self.clip, p))
    }

    /// The most `dn · p` (unit `dn`) reaches over the solid, proven past `floor` only: a side at
    /// or under it answers `floor` (no witness). None: no solid in the box and no floor.
    fn side(&self, dn: P3, floor: f64) -> Option<Side> {
        // across the side: `u` and `v`, the axes `dn` leans on least first
        let e = (0..3).min_by(|&i, &j| dn[i].abs().total_cmp(&dn[j].abs())).unwrap_or(0);
        let mut ek = [0.0; 3];
        ek[e] = 1.0;
        let u = cross(dn, ek);
        let ul = norm(u);
        let u = u.map(|x| x / ul);
        let v = cross(dn, u);
        let dot = |a: P3, b: P3| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
        let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
        for c in 0..8 {
            let p: P3 = std::array::from_fn(|k| if c >> k & 1 == 1 { self.clip.1[k] } else { self.clip.0[k] });
            for (k, w) in [u, v, dn].into_iter().enumerate() {
                lo[k] = lo[k].min(dot(p, w));
                hi[k] = hi[k].max(dot(p, w));
            }
        }
        let at = |a: f64, b: f64, z: f64| -> P3 { std::array::from_fn(|k| a * u[k] + b * v[k] + z * dn[k]) };
        let scale = (0..3).map(|k| hi[k] - lo[k]).fold(0.0f64, f64::max).max(1e-9);
        // the radius a value of slope 1 clears, and one of the field's, a little short for the rounding
        let clear = |v: f64| v * (1.0 - 1e-9) - scale * 1e-12;
        let ball = |v: f64| clear(v / self.lip);
        // a side is done once every column is down to this past the best solid point: the side
        // then ends `tol` past that point, at least a quarter `tol` past the solid
        let near = 0.75 * self.tol;
        let mut best: Option<(f64, P3)> = None;
        let goal = |best: &Option<(f64, P3)>| {
            let g = floor.max(lo[2]);
            best.map_or(g, |(s, _)| g.max(s + near))
        };
        let raise = |best: &mut Option<(f64, P3)>, z: f64, p: P3| {
            if best.map_or(true, |(s, _)| z > s) {
                *best = Some((z, p));
            }
        };
        let mut heap = BinaryHeap::new();
        heap.push(Col { top: hi[2], a: 0.5 * (lo[0] + hi[0]), b: 0.5 * (lo[1] + hi[1]), ha: 0.5 * (hi[0] - lo[0]), hb: 0.5 * (hi[1] - lo[1]), out: 0.0 });
        let mut evals = 0usize;
        let mut spent = None;
        while let Some(col) = heap.pop() {
            if col.top <= goal(&best) {
                // the highest column is down: every one is
                break;
            }
            if evals >= self.budget {
                spent = Some(col.top);
                break;
            }
            let rho = col.ha.hypot(col.hb);
            // The lowest the balls on the axis clear the column to: one at its top (a sphere
            // trace's step) and, where that steps less than the column is wide, ones farther out.
            let mut low = col.top;
            let mut out = col.out;
            for (i, k) in [-1.0, 0.0, 2.0, 8.0, 32.0, 128.0].into_iter().enumerate() {
                let lam = if i == 0 { col.out } else { k * rho };
                if i > 0 && lam == col.out {
                    continue;
                }
                let z = col.top + lam;
                let c = at(col.a, col.b, z);
                // The outer tree first: past the column's reach it answers alone, without a kernel.
                // Nearer the solid the field may clear farther, and only it finds solid.
                let limit = self.outer.map_or(f64::NEG_INFINITY, |g| {
                    evals += 1;
                    g(c).max(box_distance(&self.clip, c))
                });
                let r = if limit > rho {
                    clear(limit)
                } else if self.centres.map_or(true, |b| inside(&b, c)) {
                    let val = self.value(c);
                    evals += 1;
                    if val <= 0.0 {
                        // solid at the column's top
                        raise(&mut best, z, c);
                        break;
                    }
                    ball(val).max(clear(limit))
                } else {
                    ball(box_distance(&self.clip, c)).max(clear(limit))
                };
                if r > rho && z - (r * r - rho * rho).sqrt() < low {
                    low = z - (r * r - rho * rho).sqrt();
                    out = lam;
                }
                if low <= goal(&best) || col.top - low >= rho || (i > 0 && r < lam) {
                    break;
                }
            }
            if low <= goal(&best) {
                continue;
            }
            // (a column of no width is cleared by no step: it has to go on and spend the budget)
            if col.top - low >= rho && low < col.top {
                heap.push(Col { top: low, out, ..col });
                continue;
            }
            // Stuck near the solid: the column looks down its axis for solid, which bounds the side
            // from below, as far as the best solid point so far (anything above it raises the
            // goal). It only finds witnesses: what is proven is still the balls' alone.
            let stop = best.map_or(f64::NEG_INFINITY, |(s, _)| s).max(floor - near).max(lo[2]);
            let mut z = low;
            while z > stop && evals < self.budget {
                let c = at(col.a, col.b, z);
                let val = self.value(c);
                evals += 1;
                if val <= 0.0 {
                    raise(&mut best, z, c);
                    break;
                }
                z -= (val / self.lip).max(0.125 * self.tol);
            }
            if low <= goal(&best) {
                continue;
            }
            let (ha, hb) = (col.ha, col.hb);
            let (na, nb) = if ha > 1.5 * hb { (2, 1) } else if hb > 1.5 * ha { (1, 2) } else { (2, 2) };
            let (sa, sb) = (ha / na as f64, hb / nb as f64);
            for i in 0..na {
                for j in 0..nb {
                    let a = col.a - ha + sa * (2 * i + 1) as f64;
                    let b = col.b - hb + sb * (2 * j + 1) as f64;
                    heap.push(Col { top: low, a, b, ha: sa, hb: sb, out });
                }
            }
        }
        Some(match (best, spent) {
            (_, Some(top)) if top <= floor => Side { bound: floor, witness: None },
            (best, Some(top)) => Side { bound: top, witness: best.map(|b| b.1) },
            // never past the box's own side, which holds the solid as it is
            (Some((s, p)), None) if s + near >= floor => Side { bound: (s + self.tol).min(hi[2]), witness: Some(p) },
            (_, None) if floor > f64::NEG_INFINITY => Side { bound: floor, witness: None },
            (None, None) => return None,
            (Some(_), None) => unreachable!("a witness under the floor leaves the floor finite"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SdfOp, SdfShape};

    /// The box, its found solid points within `tol` inside each side.
    fn bounds(root: SdfNode, min: [f64; 3], max: [f64; 3], m: &Affine) -> ([f64; 3], [f64; 3]) {
        let s = SdfShape { root, min, max, materials: Vec::new() };
        let e = shape_bounds(&s, m, 0.005).expect("a solid");
        let (lo, hi) = e.found.expect("solid points");
        assert!((0..3).all(|k| lo[k] >= e.min[k] && lo[k] <= e.min[k] + 0.005 && hi[k] <= e.max[k] && hi[k] >= e.max[k] - 0.005), "{e:?}");
        (e.min, e.max)
    }

    const I: Affine = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]];

    /// Each side at most `tol` (0.005) past `want` and never short of it.
    fn tight(lo: [f64; 3], hi: [f64; 3], want_lo: [f64; 3], want_hi: [f64; 3], tol: f64) -> bool {
        (0..3).all(|i| lo[i] <= want_lo[i] && lo[i] >= want_lo[i] - tol && hi[i] >= want_hi[i] && hi[i] <= want_hi[i] + tol)
    }

    #[test]
    fn a_sphere_and_a_box_fill_their_boxes() {
        let (lo, hi) = bounds(SdfNode::Sphere { radius: 10.0 }, [-13.0; 3], [12.0; 3], &I);
        assert!(tight(lo, hi, [-10.0; 3], [10.0; 3], 0.005), "{lo:?} {hi:?}");
        let (lo, hi) = bounds(SdfNode::Box { size: [20.0, 10.0, 6.0], radius: 0.0 }, [-15.0; 3], [15.0; 3], &I);
        assert!(tight(lo, hi, [-10.0, -5.0, -3.0], [10.0, 5.0, 3.0], 0.005), "{lo:?} {hi:?}");
    }

    /// A field steeper than a distance is read only inside its box, which may be smaller than
    /// its tree's: its sides are proven inside the box, and a prover started past it (no ball can
    /// bring it in) ends on its budget.
    #[test]
    fn a_side_past_where_the_field_may_be_read_ends() {
        let root = SdfNode::Box { size: [20.0; 3], radius: 0.0 };
        let within = ([-8.0; 3], [8.0; 3]);
        let side = node_side(&root, within, 1.4, Some(within), [0.0, 0.0, 1.0], 0.01, 5_000).expect("a solid");
        assert!(side.bound <= 8.0 && side.bound >= 7.9, "{side:?}");
        let f = |p: P3| distance(&root, v3(p));
        let past = Prover { f: &f, outer: None, lip: 1.4, clip: ([-10.0; 3], [10.0; 3]), centres: Some(within), tol: 0.01, budget: 5_000 };
        assert!(past.side([0.0, 0.0, 1.0], f64::NEG_INFINITY).is_some_and(|s| s.bound <= 10.0));
    }

    /// A rippled kernel steeper than a distance, solid in half of every cell, its reads counted.
    struct Ripple(std::sync::Arc<std::sync::atomic::AtomicUsize>);
    impl Sdf3 for Ripple {
        fn distance(&self, p: Vec3d) -> f64 {
            self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            0.9 * ((0.9 * p.x).sin() + (0.9 * p.y).sin() + (0.9 * p.z).sin())
        }
    }

    fn ripple(reads: &std::sync::Arc<std::sync::atomic::AtomicUsize>) -> SdfNode {
        SdfNode::Kernel(crate::shape::SdfKernel { key: 1, field: std::sync::Arc::new(Ripple(reads.clone())), splash: None, bounds: None, solid: None })
    }

    fn at(p: [f64; 3], of: SdfNode) -> SdfNode {
        SdfNode::Transform { at: p, rotate: [0.0; 3], scale: 1.0, of: Box::new(of) }
    }

    /// A plate as a web of a kernel inside its skin, a flush boss blended on, a kernel cut through.
    fn webbed(reads: &std::sync::Arc<std::sync::atomic::AtomicUsize>) -> SdfNode {
        let plate = SdfNode::Box { size: [64.0, 10.0, 46.0], radius: 1.2 };
        let skin = SdfNode::Shell { thickness: 1.2, of: Box::new(SdfNode::Round { radius: -1.2, of: Box::new(plate) }) };
        let web = SdfNode::Combine { op: SdfOp::Intersection, blend: 0.3, of: vec![skin, ripple(reads)] };
        let boss = at([15.0, 0.0, 0.0], SdfNode::Cylinder { radius: 7.0, top_radius: 7.0, height: 10.0 });
        let joined = SdfNode::Combine { op: SdfOp::Union, blend: 1.5, of: vec![web, boss] };
        let hole = at([-15.0, 0.0, 0.0], SdfNode::Combine { op: SdfOp::Intersection, blend: 0.0, of: vec![SdfNode::Sphere { radius: 3.0 }, ripple(reads)] });
        SdfNode::Combine { op: SdfOp::Difference, blend: 0.5, of: vec![joined, hole] }
    }

    /// Against the field by brute force, for every node the tree keeps through, and the nodes it
    /// has no limit for.
    #[test]
    fn an_outer_tree_is_nowhere_above_its_field() {
        let reads = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let ball = || SdfNode::Sphere { radius: 9.0 };
        let carved = |op: SdfOp, blend: f64| SdfNode::Combine { op, blend, of: vec![ball(), ripple(&reads)] };
        let trees = [
            webbed(&reads),
            carved(SdfOp::Intersection, 2.0),
            carved(SdfOp::Difference, 2.0),
            SdfNode::Combine { op: SdfOp::Intersection, blend: 1.0, of: vec![ripple(&reads), ball(), SdfNode::Box { size: [12.0; 3], radius: 0.0 }] },
            SdfNode::Combine { op: SdfOp::Union, blend: 3.0, of: vec![carved(SdfOp::Intersection, 1.0), at([5.0, 0.0, 0.0], ball())] },
            SdfNode::Round { radius: -0.7, of: Box::new(at([1.0, 2.0, 3.0], carved(SdfOp::Intersection, 1.0))) },
            SdfNode::Id { id: 3, of: Box::new(SdfNode::Material { slot: 1, within: None, of: Box::new(carved(SdfOp::Difference, 0.0)) }) },
        ];
        for root in &trees {
            let tree = outer_tree(root).expect("a limit");
            assert!(!has_kernel(&tree));
            for k in 0..4000 {
                let t = k as f64;
                let p = Vec3d::new(14.0 * (t * 1.37).sin(), 14.0 * (t * 2.11).sin(), 14.0 * (t * 0.73).sin());
                let (limit, field) = (distance(&tree, p), distance(root, p));
                assert!(limit <= field + 1e-9, "{limit} over the field's {field} at {p:?} in {root:?}");
            }
        }
        for root in [
            ripple(&reads),
            SdfNode::Combine { op: SdfOp::Union, blend: 0.0, of: vec![ball(), ripple(&reads)] },
            SdfNode::Combine { op: SdfOp::Difference, blend: 0.0, of: vec![ripple(&reads), ball()] },
            SdfNode::Shell { thickness: 1.0, of: Box::new(carved(SdfOp::Intersection, 0.0)) },
        ] {
            assert!(outer_tree(&root).is_none(), "{root:?}");
        }
    }

    /// A flat side a kernel only carves closes with kernels in the tree, for few of its reads, and
    /// the box holds the solid.
    #[test]
    fn a_kernel_inside_a_distance_leaves_its_sides_closed() {
        let reads = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let root = webbed(&reads);
        let (min, max) = crate::bounds::node_bounds(&root).expect("bounded");
        let shape = SdfShape { root, min, max, materials: Vec::new() };
        assert!(shape.lipschitz() > 1.2, "steeper than a distance");
        let measuring = reads.swap(0, std::sync::atomic::Ordering::Relaxed);
        let e = shape_bounds(&shape, &I, 0.01).expect("a solid");
        let proving = reads.load(std::sync::atomic::Ordering::Relaxed) - measuring;
        let (lo, hi) = e.found.expect("solid points");
        assert!((0..3).all(|k| lo[k] - e.min[k] <= 0.01 && e.max[k] - hi[k] <= 0.01), "open: {:?} {:?} found {lo:?} {hi:?}", e.min, e.max);
        assert!(proving < 5_000, "{proving} kernel reads");
        holds(&shape, e.min, e.max, 2.3, 0.37);
    }

    /// A 6 mm cube as an f32 kernel computes it (points rounded to f32, f32 arithmetic).
    struct F32Cube;
    impl Sdf3 for F32Cube {
        fn distance(&self, p: Vec3d) -> f64 {
            let q = [p.x as f32, p.y as f32, p.z as f32 - 3.0].map(|c| c.abs() - 3.0);
            let out = (q[0].max(0.0).powi(2) + q[1].max(0.0).powi(2) + q[2].max(0.0).powi(2)).sqrt();
            (out + q[0].max(q[1]).max(q[2]).min(0.0)) as f64
        }
    }

    #[test]
    fn a_flat_top_holds_the_solid() {
        // A flat side flush with nothing: the columns over it come down to it from balls centred
        // far out, and it ends within `tol` past the solid, never short of it.
        for tol in [0.005, 0.05] {
            let e = surface_bounds(&F32Cube, [-4.0, -4.0, -1.0], [4.5, 4.5, 7.5], 1.0, &I, tol).expect("a solid");
            let (lo, hi) = (e.min, e.max);
            // (the f32 field's own grid: its solid ends up to half an ulp past 6)
            assert!(hi[2] >= 6.0 && hi[2] <= 6.0 + tol + 1e-6, "top {} (tol {tol})", hi[2]);
            assert!(lo[2] <= 0.0 && lo[2] >= -tol - 1e-6, "bottom {} (tol {tol})", lo[2]);
        }
    }

    #[test]
    fn a_turned_box_stands_on_its_lowest_corner() {
        // 30 degrees about X: the box's corners reach 5 cos + 3 sin down and up.
        let (c, s) = (30f64.to_radians().cos(), 30f64.to_radians().sin());
        let m: Affine = [[1.0, 0.0, 0.0, 1.0], [0.0, c, -s, 2.0], [0.0, s, c, 3.0]];
        let (lo, hi) = bounds(SdfNode::Box { size: [20.0, 10.0, 6.0], radius: 0.0 }, [-15.0; 3], [15.0; 3], &m);
        let (y, z) = (5.0 * c + 3.0 * s, 5.0 * s + 3.0 * c);
        assert!(tight(lo, hi, [-9.0, 2.0 - y, 3.0 - z], [11.0, 2.0 + y, 3.0 + z], 0.005), "{lo:?} {hi:?}");
    }

    #[test]
    fn a_pillar_on_a_ledge_reaches_past_the_ledge_top() {
        // A 10 mm square ledge 1.5 mm tall under a 4 mm square pillar 3 mm tall: the ledge's
        // flat top is the surface nearest the cells around the pillar's foot.
        let at = |y: f64, size: [f64; 3]| SdfNode::Transform { at: [0.0, y, 0.0], rotate: [0.0; 3], scale: 1.0, of: Box::new(SdfNode::Box { size, radius: 0.0 }) };
        let step = SdfNode::Combine { op: SdfOp::Union, blend: 0.0, of: vec![at(0.75, [10.0, 1.5, 10.0]), at(1.5, [4.0, 3.0, 4.0])] };
        let (lo, hi) = bounds(step, [-6.0, -1.0, -6.0], [6.0, 4.0, 6.0], &I);
        assert!(tight(lo, hi, [-5.0, 0.0, -5.0], [5.0, 3.0, 5.0], 0.005), "{lo:?} {hi:?}");
    }

    #[test]
    fn a_hollow_blend_fills_its_outside() {
        // A vase-like skin: an ellipsoid blended with a cylinder, hollowed to a 1.6 mm wall.
        let body = SdfNode::Combine {
            op: SdfOp::Union,
            blend: 4.0,
            of: vec![SdfNode::Ellipsoid { radii: [21.0, 25.0, 21.0] }, SdfNode::Transform { at: [0.0, 20.0, 0.0], rotate: [0.0; 3], scale: 1.0, of: Box::new(SdfNode::Cylinder { radius: 9.0, top_radius: 9.0, height: 30.0 }) }],
        };
        let shell = SdfNode::Shell { thickness: 0.8, of: Box::new(body) };
        let (lo, hi) = bounds(shell, [-30.0, -30.0, -30.0], [30.0, 60.0, 30.0], &I);
        assert!(tight([lo[0], lo[2], 0.0], [hi[0], hi[2], 0.0], [-21.8, -21.8, 0.0], [21.8, 21.8, 0.0], 0.006), "{lo:?} {hi:?}");
        assert!(lo[1] <= -25.8 && lo[1] >= -25.806, "{lo:?}");
    }

    /// The vase-like skin of [`a_hollow_blend_fills_its_outside`] with a flat foot cut, its box.
    fn footed() -> SdfShape {
        let at = |p: [f64; 3], of: SdfNode| SdfNode::Transform { at: p, rotate: [0.0; 3], scale: 1.0, of: Box::new(of) };
        let body = SdfNode::Combine { op: SdfOp::Union, blend: 4.0, of: vec![SdfNode::Ellipsoid { radii: [21.0, 25.0, 21.0] }, at([0.0, 20.0, 0.0], SdfNode::Cylinder { radius: 9.0, top_radius: 9.0, height: 30.0 })] };
        let cut = SdfNode::Combine { op: SdfOp::Difference, blend: 0.0, of: vec![body, at([0.0, -30.0, 0.0], SdfNode::Box { size: [60.0, 20.0, 60.0], radius: 0.0 })] };
        let root = SdfNode::Shell { thickness: 0.8, of: Box::new(cut) };
        let (min, max) = crate::bounds::node_bounds(&root).expect("bounded");
        SdfShape { root, min, max, materials: Vec::new() }
    }

    /// No solid past a side: on a grid across each side, just past it and a little farther.
    fn holds(shape: &SdfShape, lo: [f64; 3], hi: [f64; 3], step: f64, jitter: f64) {
        let f = shape.field();
        for axis in 0..3 {
            let (j, k) = ((axis + 1) % 3, (axis + 2) % 3);
            let n = |a: usize| ((shape.max[a] - shape.min[a]) / step) as usize + 1;
            for (side, sign) in [(hi[axis], 1.0), (lo[axis], -1.0)] {
                for off in [1e-9, 0.02, 0.3] {
                    for s in 0..n(j) {
                        for t in 0..n(k) {
                            let mut p = [0.0; 3];
                            p[axis] = side + sign * off;
                            p[j] = shape.min[j] + (s as f64 + jitter) * step;
                            p[k] = shape.min[k] + (t as f64 + jitter) * step;
                            assert!(f.distance(Vec3d::new(p[0], p[1], p[2])) > 0.0, "solid at {p:?} past {lo:?}..{hi:?}");
                        }
                    }
                }
            }
        }
    }

    /// Sculpted skins (raised, lowered and creased by random strokes, some over the foot and
    /// the rim): the box holds the solid; a side no stroke reaches stays the unsculpted one's.
    #[test]
    fn sculpted_bounds_hold_the_solid() {
        use crate::strokes::{StrokeOp, Strokes};
        let base = footed();
        let Extent { min: blo, max: bhi, found: bfound } = shape_bounds(&base, &I, 0.005).expect("a solid");
        let bfound = bfound.expect("solid points");
        holds(&base, blo, bhi, 0.5, 0.25);
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        let mut rnd = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 11) as f64 / (1u64 << 53) as f64
        };
        for case in 0..6 {
            let mut strokes = Strokes::new();
            for _ in 0..1 + (rnd() * 3.0) as usize {
                let op = [StrokeOp::Raise, StrokeOp::Raise, StrokeOp::Lower, StrokeOp::Crease][(rnd() * 4.0) as usize];
                let (radius, strength) = (1.0 + 7.0 * rnd(), 0.2 + 0.8 * rnd());
                // from a point on the skin's outside, a few steps of about a radius
                let (th, y) = (rnd() * std::f64::consts::TAU, -6.0 + 50.0 * rnd());
                let r = if y < 30.0 { 21.8 * (1.0 - (y / 25.8).powi(2)).max(0.1).sqrt() } else { 9.8 };
                let mut p = [r * th.cos(), y, r * th.sin()];
                let mut points = vec![p];
                for _ in 0..1 + (rnd() * 4.0) as usize {
                    p = std::array::from_fn(|k| p[k] + radius * (rnd() - 0.5));
                    points.push(p);
                }
                strokes.push(op, radius, strength, &points);
            }
            let shape = base.clone().sculpted(std::sync::Arc::new(strokes.clone()));
            let Extent { min: lo, max: hi, found } = shape_bounds(&shape, &I, 0.005).expect("a solid");
            let found = found.expect("solid points");
            holds(&shape, lo, hi, 0.5, 0.1 + 0.8 * rnd());
            // a side the raised tubes stay short of and no lowering reaches keeps its value and its
            // solid point (what a part rests on)
            let reach = strokes.raised_bounds();
            for axis in 0..3 {
                let clear_hi = reach.map_or(true, |r| r.1[axis] < bhi[axis]) && !strokes.applied().any(|(si, s)| s.tube().is_some_and(|t| !t.raise) && strokes.boxes(si..si + 1).iter().any(|b| b.1[axis] >= bhi[axis] - 0.01));
                if clear_hi {
                    assert_eq!((hi[axis], found.1[axis]), (bhi[axis], bfound.1[axis]), "case {case}: side +{axis} moved");
                }
                let clear_lo = reach.map_or(true, |r| r.0[axis] > blo[axis]) && !strokes.applied().any(|(si, s)| s.tube().is_some_and(|t| !t.raise) && strokes.boxes(si..si + 1).iter().any(|b| b.0[axis] <= blo[axis] + 0.01));
                if clear_lo {
                    assert_eq!((lo[axis], found.0[axis]), (blo[axis], bfound.0[axis]), "case {case}: side -{axis} moved");
                }
            }
        }
    }

    /// A grid's read as a kernel that counts its reads.
    struct Read(std::sync::Arc<crate::SdfGrid>, std::sync::Arc<std::sync::atomic::AtomicUsize>);
    impl Sdf3 for Read {
        fn distance(&self, p: Vec3d) -> f64 {
            self.1.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let g = &self.0;
            let n = g.dims.map(|d| d as usize);
            let at = |i: usize, j: usize, k: usize| g.data[i + n[0] * (j + n[1] * k)] as f64;
            let cell = |x: f64, a: usize| -> (usize, f64) {
                let u = ((x - g.origin[a] as f64) / g.h as f64).clamp(0.0, (n[a] - 1) as f64);
                let c = (u.floor() as usize).min(n[a] - 2);
                (c, u - c as f64)
            };
            let ((i, u), (j, v), (k, w)) = (cell(p.x, 0), cell(p.y, 1), cell(p.z, 2));
            let lerp = |a: f64, b: f64, t: f64| a + (b - a) * t;
            let y0 = lerp(lerp(at(i, j, k), at(i + 1, j, k), u), lerp(at(i, j + 1, k), at(i + 1, j + 1, k), u), v);
            let y1 = lerp(lerp(at(i, j, k + 1), at(i + 1, j, k + 1), u), lerp(at(i, j + 1, k + 1), at(i + 1, j + 1, k + 1), u), v);
            lerp(y0, y1, w)
        }
    }

    /// A grid over -30..30 at a 2 spacing, sampling `f` scaled steeper than a distance as a grown
    /// field is, as a kernel; and the count of its reads.
    fn read_of(f: impl Fn([f64; 3]) -> f64) -> (SdfNode, std::sync::Arc<crate::SdfGrid>, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        let n = 31usize;
        let mut data = Vec::with_capacity(n * n * n);
        for k in 0..n {
            for j in 0..n {
                for i in 0..n {
                    data.push((1.3 * f([-30.0 + 2.0 * i as f64, -30.0 + 2.0 * j as f64, -30.0 + 2.0 * k as f64])) as f32);
                }
            }
        }
        let grid = std::sync::Arc::new(crate::SdfGrid::new([-30.0; 3], 2.0, [n as u32; 3], 0.0, data.into()));
        let reads = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let bounds = grid.solid_box();
        let kernel = crate::shape::SdfKernel { key: grid.hash(), field: std::sync::Arc::new(Read(grid.clone(), reads.clone())), splash: None, bounds, solid: Some(grid.clone()) };
        (SdfNode::Kernel(kernel), grid, reads)
    }

    /// Three balls meeting, a flat cut through them, and a thin wire: a shape with flat and curved
    /// sides that do not reach the grid's edge.
    fn lumpy(p: [f64; 3]) -> f64 {
        let ball = |c: [f64; 3], r: f64| ((p[0] - c[0]).powi(2) + (p[1] - c[1]).powi(2) + (p[2] - c[2]).powi(2)).sqrt() - r;
        let wire = (p[0] - 3.0).hypot(p[1] + 4.0) - 1.1;
        ball([-8.0, 3.0, 2.0], 11.0).min(ball([9.0, -6.0, -4.0], 8.0)).min(ball([4.0, 12.0, 8.0], 7.0)).min(wire.max(p[2].abs() - 21.0)).max(p[1] - 15.0)
    }

    fn turned(rx: f64, rz: f64) -> Affine {
        let (sx, cx) = rx.to_radians().sin_cos();
        let (sz, cz) = rz.to_radians().sin_cos();
        [[cz, -sz * cx, sz * sx, 0.0], [sz, cz * cx, -cz * sx, 0.0], [0.0, sx, cx, 0.0]]
    }

    /// Under each frame the box of a grid's read holds every solid point of a lattice far finer
    /// than the grid's and reads the field at most a few thousand times a side: the samples bound
    /// it, the field only says a point is solid. Along the grid's axes it is at most `tol` past a
    /// solid point it found, and turned from them a small part of a cell.
    #[test]
    fn a_grids_read_is_bounded_by_its_samples() {
        let (root, grid, reads) = read_of(lumpy);
        let field = Read(grid.clone(), Default::default());
        let tol = 0.01;
        for (rx, rz) in [(0.0, 0.0), (0.0, 30.0), (20.0, 40.0), (90.0, 0.0), (45.0, 45.0)] {
            let m = turned(rx, rz);
            let shape = SdfShape { root: root.clone(), min: [-30.0; 3], max: [30.0; 3], materials: Vec::new() };
            reads.store(0, std::sync::atomic::Ordering::Relaxed);
            let e = shape_bounds(&shape, &m, tol).expect("a solid");
            let read = reads.load(std::sync::atomic::Ordering::Relaxed);
            assert!(read < 40_000, "{rx} {rz}: {read} reads");
            let (lo, hi) = e.found.expect("solid points");
            let gap = (0..3).map(|k| (lo[k] - e.min[k]).max(e.max[k] - hi[k])).fold(0.0f64, f64::max);
            let axes = rx % 90.0 == 0.0 && rz % 90.0 == 0.0;
            println!("turned {rx} {rz}: {read} reads, {gap:.4} past its solid points");
            assert!(gap <= if axes { tol + 1e-9 } else { 0.25 }, "{rx} {rz}: {gap} past: {e:?}");
            let steps = 240;
            let mut solid = 0;
            let mut most = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
            for k in 0..=steps {
                for j in 0..=steps {
                    for i in 0..=steps {
                        let p = [-30.0 + 0.25 * i as f64, -30.0 + 0.25 * j as f64, -30.0 + 0.25 * k as f64];
                        if field.distance(v3(p)) <= 0.0 {
                            solid += 1;
                            for r in 0..3 {
                                let q = m[r][0] * p[0] + m[r][1] * p[1] + m[r][2] * p[2];
                                most.0[r] = most.0[r].min(q);
                                most.1[r] = most.1[r].max(q);
                            }
                        }
                    }
                }
            }
            assert!(solid > 1000);
            assert!((0..3).all(|r| most.0[r] >= e.min[r] && most.1[r] <= e.max[r]), "{rx} {rz}: {most:?} past {e:?}");
        }
    }

    /// A flat free side closes without a proof, however flat: a field steeper than a distance
    /// would spend its budget there. A box round the read holds its side to the bit.
    #[test]
    fn a_read_stops_at_what_cuts_it() {
        let (read, _, reads) = read_of(|p| p[2] - 5.3);
        let cut = SdfNode::Combine { op: SdfOp::Intersection, blend: 0.0, of: vec![SdfNode::Box { size: [40.0, 30.0, 80.0], radius: 0.0 }, read.clone()] };
        let shape = SdfShape { root: cut, min: [-30.0; 3], max: [30.0; 3], materials: Vec::new() };
        let e = shape_bounds(&shape, &I, 0.01).expect("a solid");
        assert!((e.max[2] - 5.3).abs() <= 0.01 && e.max[2] >= 5.3, "{e:?}");
        // its other sides are the box's, and the solid goes on past the grid's low edge, into the box
        assert!(e.min[0] == -20.0 && e.max[0] == 20.0 && e.min[1] == -15.0 && e.max[1] == 15.0 && e.min[2] == -40.0, "{e:?}");
        assert!(reads.load(std::sync::atomic::Ordering::Relaxed) < 2_000, "{} reads", reads.load(std::sync::atomic::Ordering::Relaxed));
    }

    /// With a solid of its own beside it, the higher of the two says; a cut that takes the read's
    /// top leaves a bound that holds; a read with no solid sample is proven empty.
    #[test]
    fn a_read_among_others() {
        let (read, _, reads) = read_of(|p| (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt() - 10.0);
        let ball = SdfNode::Transform { at: [0.0, 0.0, 15.0], rotate: [0.0; 3], scale: 1.0, of: Box::new(SdfNode::Sphere { radius: 3.0 }) };
        let both = SdfNode::Combine { op: SdfOp::Union, blend: 0.0, of: vec![read.clone(), ball] };
        let shape = SdfShape { root: both, min: [-30.0; 3], max: [30.0; 3], materials: Vec::new() };
        let e = shape_bounds(&shape, &I, 0.01).expect("a solid");
        assert!((e.max[2] - 18.0).abs() <= 0.01, "{e:?}");
        assert!((e.max[0] - 10.0).abs() <= 0.5 && (e.min[2] + 10.0).abs() <= 0.5, "{e:?}");
        assert!(reads.load(std::sync::atomic::Ordering::Relaxed) < 2_000, "{} reads", reads.load(std::sync::atomic::Ordering::Relaxed));

        let cutter = SdfNode::Transform { at: [0.0, 0.0, 10.0], rotate: [0.0; 3], scale: 1.0, of: Box::new(SdfNode::Sphere { radius: 3.0 }) };
        let cut = SdfNode::Combine { op: SdfOp::Difference, blend: 0.0, of: vec![read, cutter] };
        let shape = SdfShape { root: cut.clone(), min: [-30.0; 3], max: [30.0; 3], materials: Vec::new() };
        let e = shape_bounds(&shape, &I, 0.01).expect("a solid");
        let (_, hi) = e.found.expect("solid points");
        assert!(e.max[2] >= hi[2] && crate::shape::distance(&cut, v3([0.0, 8.5, hi[2].min(5.0)])) <= 0.0, "{e:?}");

        let (none, _, _) = read_of(|_| 1.0);
        let shape = SdfShape { root: none, min: [-30.0; 3], max: [30.0; 3], materials: Vec::new() };
        assert!(shape_bounds(&shape, &I, 0.01).is_none(), "nothing solid in it");
    }
}
