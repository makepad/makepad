//! The box a field's solid fills, without meshing it, proven to hold the solid: each side of a
//! target frame starts where a box that holds the solid by construction puts it (the field's
//! own box; a shape's tree box, [`tight_bounds`]) and moves in only as far as the field proves,
//! never to the farthest solid point a search happened to find.
//!
//! A side is proven by columns across it: the region beyond a column's top is covered by balls
//! the field proves empty (a value `v` at a point clears the ball of radius `v / L` round it, `L`
//! how steep the field can be there outside its solid: along any line out of the ball the value
//! falls from `v` to 0 before it reaches solid, all of it at positive values). A ball centred on a column's axis clears the whole column
//! over the height its radius spans past the column's half-diagonal; centred farther out along
//! the side's direction it reaches a flat side across a wider column, so a flat side costs its
//! area over the square root of its gap, not over its square. A column the balls cannot take
//! down first looks down its axis for solid (a witness: the side's goal rises with it), then
//! splits. A side ends [`END`] of `tol` past a solid point once every column is down to three
//! quarters of a `tol` past it: under `tol` past the solid (by far more than a frame's arithmetic
//! rounds), and never past the box it started from (an exact tree box's side stays exact). Each
//! side also reports its witness: the solid reaches at least there, so a part rests on its
//! bottom's witness, its box's bottom at most what the proof left open below.
//! A field steeper than a distance (`L > 1`) closes a flat side only by balls of radius about
//! `tol / (L - 1)`. Most of such a field is far less steep all the same, a kernel only making the
//! values near it and an ellipsoid's estimate steep only well off its surface: a ball has the
//! radius its value has at the slope the pieces of its box prove at the values under it, all a
//! line out of it passes before the surface ([`crate::Chunks::slopes_in`]; the field's
//! measured `L` where a kernel or a sculpt leaves it
//! unbounded), the tree without its kernels ([`outer_tree`]) clears what lies far from them at
//! its own, and where the balls stall, the tree cut to a slab under the column
//! ([`crate::bounds::cut`]) proves the slab empty when its value there is positive. A tree
//! whose kernels are grids' reads ([`crate::SdfKernel::grid_read`]) has no balls to spend on
//! those: the grids' samples bound their solid outright ([`known_side`]).

use crate::bounds::{blend_reach, grow, meet, out_of, rot, tight_bounds, Bounds};
use crate::lipschitz::{has_grid_read, has_kernel};
use crate::sdf::Sdf3;
use crate::shape::{distance, SdfNode, SdfOp, SdfShape, P3};
use crate::strokes::Strokes;
use makepad_csg_math::Vec3d;
use std::collections::BinaryHeap;

/// Row-major affine `p' = m * p`.
pub type Affine = [[f64; 4]; 3];

/// How far past its witness a closed side ends, in `tol`: past the three quarters its columns
/// came down to, so at least an eighth past the solid, and short of `tol` by more than any
/// rounding, so a gap a closed proof leaves never reads as more than `tol`.
const END: f64 = 0.875;

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

/// How far past a ball's height over its column a read is taken, in column widths: a ball steps
/// a column down by at most its radius, and one column steps on again for one more read, so a
/// value past this costs reads of pieces no nearer step needs (a tree's reads stop at it:
/// [`Sdf3::distance_bounded`]).
const CAP: f64 = 128.0;

/// Work spent by the ball proof of a side. Grid sample walks are separate.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProofWork {
    pub evaluations: usize,
    /// The ball proof stopped with open columns because its budget ran out.
    pub budget_exhausted: bool,
}

impl ProofWork {
    fn add(self, other: Self) -> Self {
        Self { evaluations: self.evaluations + other.evaluations, budget_exhausted: self.budget_exhausted || other.budget_exhausted }
    }
}

/// A bottom proof in the requested frame, including the interval still open.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bottom {
    /// No solid point lies below this height.
    pub bound: f64,
    /// The lowest solid point found, if any.
    pub witness: Option<f64>,
    pub work: ProofWork,
}

/// A solid's box in a frame ([`shape_bounds`], [`surface_bounds`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Extent {
    /// No solid point lies outside `min..max`.
    pub min: P3,
    pub max: P3,
    /// Per side, the solid point its proof found nearest it (a side whose proof found none: the
    /// farthest any found), so the solid spans at least this box. A side's gap to `min..max` is
    /// what its proof left open: under `tol` where it closed. A side no change reaches keeps
    /// both to the bit. None: no solid point found.
    pub found: Option<(P3, P3)>,
    /// Ball proof work for the negative and positive sides of each axis.
    pub work: [[ProofWork; 3]; 2],
    /// Each side's proof, for the extent of a later shape no change of which reaches it
    /// ([`shape_bounds_after`]).
    pub proofs: [[Option<SideProof>; 3]; 2],
}

/// One side's proof as an extent keeps it: its direction (unit, the shape's own frame), how far
/// along it the solid reaches at most and the solid point that reaches farthest.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SideProof {
    dn: P3,
    side: Side,
}

/// The axis-aligned box, in the frame `m` maps into, that holds the solid (`field <= 0`) inside
/// the field's box `min..max`: each side at most `tol` past the solid (where its budget runs out:
/// what was proven). `lipschitz` bounds the field's slope (1 for a distance); a field steeper than
/// a distance is read only inside its box. None when the box holds no solid.
pub fn surface_bounds(field: &dyn Sdf3, min: P3, max: P3, lipschitz: f64, m: &Affine, tol: f64) -> Option<Extent> {
    let lip = lipschitz.max(1.0);
    let region = (min, max);
    let centres = (lip > 1.0).then_some(region);
    let hints = std::cell::RefCell::new(crate::sdf::Hints::default());
    let f = |p: P3, cap: f64| field.distance_bounded(v3(p), -cap, cap, &mut hints.borrow_mut());
    frame_bounds(m, |dn, budget| Prover { f: &f, outer: None, lip: &|| lip, slope: None, clip: region, centres, empty: None, tol, budget }.side(dn, f64::NEG_INFINITY))
}

/// [`surface_bounds`] of a shape's solid: each side from the shape's tree box ([`tight_bounds`];
/// the shape's box for a tree without one) moved in as far as proven. A sculpt's side is its
/// unsculpted node's (no stroke changes its proof, so a stroke that cannot reach a side leaves it
/// to the bit) unless a stroke can move it: the raised tubes are proven past it, and a lowering
/// stroke over the solid point that bounds it proves it again.
pub fn shape_bounds(shape: &SdfShape, m: &Affine, tol: f64) -> Option<Extent> {
    let mut side = shape_sides(shape, tol);
    frame_bounds(m, |dn, budget| side(dn, budget, None))
}

/// [`shape_bounds`], each side taken from an earlier proof that no difference of the shapes
/// reaches (`before`: earlier shapes and their extents under the same `m`). Where the region the
/// two solids may differ in ([`crate::bounds::changed`], and where their boxes differ) lies wholly
/// under a side's bound and off its witness, no solid point of the new shape lies past the bound
/// (none of the old did, and the new are the old's there) and the witness is solid still: the side
/// is the old proof to the bit, at no reads. Where the region reaches past the bound but not the
/// witness, the side is proven over the region alone, past the old bound. Every other side is
/// proven anew.
pub fn shape_bounds_after(shape: &SdfShape, m: &Affine, tol: f64, before: &[(&SdfShape, &Extent)]) -> Option<Extent> {
    // each earlier shape's region, the first time a side asks it
    let tight = std::cell::OnceCell::new();
    let regions: Vec<std::cell::OnceCell<Vec<Bounds>>> = before.iter().map(|_| std::cell::OnceCell::new()).collect();
    let tight = &tight;
    let mut prove = shape_sides(shape, tol);
    frame_bounds(m, |dn, budget| {
        // each earlier side whose witness no difference reaches, with the boxes of the differences
        let earlier = || {
            before.iter().zip(&regions).filter_map(move |((old, e), region)| {
                let p = e.proofs.iter().flatten().flatten().find(|p| p.dn == dn)?;
                let w = p.side.witness?;
                let region = region.get_or_init(|| moved(old, shape, tight));
                region.iter().all(|r| !inside(r, w)).then_some((region.as_slice(), Side { work: ProofWork::default(), ..p.side }))
            })
        };
        // none reaching past it: the side to the bit, at no reads
        if let Some((_, side)) = earlier().find(|(region, side)| region.iter().all(|r| support(r, dn) <= side.bound)) {
            return Some(side);
        }
        prove(dn, budget, earlier().next())
    })
}

/// Boxes that hold every point where the solids of `old` and `new` may differ: the trees' changed
/// subtrees ([`crate::bounds::changed`]) and, where the shapes' boxes differ, the slabs between
/// their sides (the solid counts inside the box) inside both trees' boxes ([`tight_bounds`]):
/// off the changed subtrees a point is solid in both trees or in neither, and each tree's box
/// holds its solid, so only a point inside both can count in one shape's box and not the other's.
/// (`tight`: the new tree's box, made once for every earlier shape.)
fn moved(old: &SdfShape, new: &SdfShape, tight: &std::cell::OnceCell<Option<Bounds>>) -> Vec<Bounds> {
    let mut region: Vec<Bounds> = crate::bounds::changed(&old.root, &new.root).into_iter().collect();
    if old.min == new.min && old.max == new.max {
        return region;
    }
    let hull: Bounds = (std::array::from_fn(|k| old.min[k].min(new.min[k])), std::array::from_fn(|k| old.max[k].max(new.max[k])));
    let Some(held) = [tight_bounds(&old.root), *tight.get_or_init(|| tight_bounds(&new.root))].into_iter().flatten().try_fold(hull, |b, t| meet(&b, &t)) else {
        return region;
    };
    for k in 0..3 {
        for (a, b) in [(old.min[k], new.min[k]), (old.max[k], new.max[k])] {
            let mut slab = held;
            (slab.0[k], slab.1[k]) = (slab.0[k].max(a.min(b)), slab.1[k].min(a.max(b)));
            if a != b && slab.0[k] < slab.1[k] {
                region.push(slab);
            }
        }
    }
    region
}

/// The bottom bound and solid witness in `m`, identical to [`shape_bounds`]' bottom side.
/// Rest-only callers need no other sides when the bottom finds a witness. If it finds none,
/// the full proof supplies the same cross-side witness as `shape_bounds`.
pub fn shape_bottom(shape: &SdfShape, m: &Affine, tol: f64) -> Option<Bottom> {
    let d = [m[2][0], m[2][1], m[2][2]];
    let len = norm(d);
    let mut side = shape_sides(shape, tol);
    if len != 0.0 {
        let bottom = side(d.map(|x| -x / len), BOTTOM_BUDGET, None)?;
        if let Some(w) = bottom.witness {
            return Some(Bottom { bound: -bottom.bound * len + m[2][3], witness: Some(m[2][0] * w[0] + m[2][1] * w[1] + m[2][2] * w[2] + m[2][3]), work: bottom.work });
        }
    }
    let e = frame_bounds(m, |dn, budget| side(dn, budget, None))?;
    Some(Bottom { bound: e.min[2], witness: e.found.map(|f| f.0[2]), work: e.work[0][2] })
}

/// Each side of a shape's solid, `past` as [`node_side`]'s (a tree its grids bound proves the
/// side whole).
fn shape_sides(shape: &SdfShape, tol: f64) -> impl FnMut(P3, usize, Option<(&[Bounds], Side)>) -> Option<Side> + '_ {
    let reads = std::cell::OnceCell::new();
    let within = (shape.min, shape.max);
    move |dn, budget, past| {
        // A tree its grids' samples bound has its side from them: no proof could close it for less.
        if let Some(k) = has_grid_read(&shape.root).then(|| grid_side(&shape.root, dn, tol, budget)).flatten() {
            if k.top == f64::NEG_INFINITY {
                return None;
            }
            let top = k.top.min(support(&clip_of(&shape.root, &within), dn));
            return Some(match k.seed {
                Some((s, p)) if s + 0.75 * tol >= top => Side { bound: (s + END * tol).min(top.max(s)), witness: Some(p), work: k.work },
                seed => Side { bound: top, witness: seed.map(|(_, p)| p), work: k.work },
            });
        }
        let r: &Reads = reads.get_or_init(|| Reads::of_shape(shape));
        node_side(&shape.root, r, within, &|| r.lip(), r.steep().then_some(within), dn, tol, budget, past)
    }
}

/// A tree's reads for its sides' proofs, made once for all of them: the tree and its
/// [`outer_tree`] in chunks over its box and a quarter round it ([`crate::Chunks`]: the many reads
/// near a side make their values from only the pieces near it, bit for bit the tree's), and the
/// bound on its slope outside its solid where its chunks prove none.
struct Reads {
    field: std::sync::Arc<crate::chunks::Chunks>,
    outer: Option<crate::chunks::Chunks>,
    /// The bound, at least 1, where the tree's pieces prove one over the chunks' box; None where
    /// a piece proves none (a kernel, a sculpt): then measured over `measured` through the chunks,
    /// the first time a ball needs it ([`Reads::lip`]).
    proven: Option<f64>,
    measured: Bounds,
    lip: std::sync::OnceLock<f64>,
}

impl Reads {
    /// `proven` the bound its pieces prove over the chunks' box, if any.
    fn new(n: &SdfNode, within: &Bounds, proven: impl FnOnce(&Bounds) -> Option<f64>, measured: Bounds) -> Reads {
        let clip = clip_of(n, within);
        let region = grow(clip, 0.25 * extent(&clip));
        let field = std::sync::Arc::new(chunked(n, region));
        let proven = proven(&region);
        // a tree without kernels is its own outer tree
        let outer = (proven.map_or(true, |l| l > 1.0) && has_kernel(n)).then(|| outer_tree(n)).flatten().map(|t| chunked(&t, region));
        Reads { field, outer, proven, measured, lip: std::sync::OnceLock::new() }
    }

    /// A tree's own: what its pieces prove outside its solid over the chunks' box, or measured over
    /// `measured` through the chunks where a piece proves nothing (a kernel, a sculpt).
    fn of_tree(n: &SdfNode, within: &Bounds, measured: Bounds) -> Reads {
        Reads::new(n, within, |region| Some(crate::bounds::slopes(n, region).outside).filter(|s| s.is_finite()).map(|s| s.max(1.0)), measured)
    }

    /// How steep the field can be outside its solid where its chunks prove no slope.
    fn lip(&self) -> f64 {
        self.proven.unwrap_or_else(|| {
            *self.lip.get_or_init(|| {
                let f: std::sync::Arc<dyn Sdf3 + Send + Sync> = self.field.clone();
                crate::lipschitz::measure(&f, v3(self.measured.0), v3(self.measured.1))
            })
        })
    }

    /// Whether the field may be steeper than a distance (a measured bound is: at least 1.1).
    fn steep(&self) -> bool {
        self.proven.map_or(true, |l| l > 1.0)
    }

    /// A shape's, measured over its box.
    fn of_shape(shape: &SdfShape) -> Reads {
        Reads::of_tree(&shape.root, &(shape.min, shape.max), (shape.min, shape.max))
    }
}

/// What a tree holding grid reads knows of a side without its proof.
#[derive(Clone, Copy, Debug)]
struct Known {
    /// Nothing solid lies past this along the direction (`NEG_INFINITY`: nothing solid at all).
    top: f64,
    /// The highest solid point found, in the root's frame, and how high in the root's frame.
    seed: Option<(f64, P3)>,
    work: ProofWork,
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
        // its own proof, through its chunks at the slopes they prove: a distance's of 1 closes in
        // a few reads
        let clip = tight_bounds(n)?;
        let reads = Reads::of_tree(n, &clip, clip);
        let side = node_side(n, &reads, clip, &|| reads.lip(), reads.steep().then_some(clip), dn, tol, budget, None);
        return Some(side.map_or(Known { top: f64::NEG_INFINITY, seed: None, work: ProofWork::default() }, |s| Known { top: s.bound, seed: s.witness.and_then(seed), work: s.work }));
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
            Some(side.map_or(Known { top: f64::NEG_INFINITY, seed: None, work: ProofWork::default() }, |s| Known { top: s.top, seed: witness, work: ProofWork::default() }))
        }
        SdfNode::Transform { at, rotate, scale, of } => {
            let r = rot(*rotate);
            let inner: P3 = std::array::from_fn(|i| (0..3).map(|k| r[k][i] * dn[k]).sum());
            let out = out_of(*at, *rotate, *scale);
            let along = dot(dn, *at);
            let k = known_side(of, inner, (ceiling - along) / scale, tol / scale, budget, &|p| to_root(out(p)), solid, &|p, r| clear(out(p), r * scale))?;
            Some(Known { top: if k.top == f64::NEG_INFINITY { k.top } else { k.top * scale + along }, seed: k.seed, work: k.work })
        }
        SdfNode::Id { of, .. } | SdfNode::Material { of, .. } | SdfNode::Bone { of, .. } => known_side(of, dn, ceiling, tol, budget, to_root, solid, clear),
        SdfNode::Combine { op: SdfOp::Union, blend, of } => {
            let mut all = Known { top: f64::NEG_INFINITY, seed: None, work: ProofWork::default() };
            for c in of {
                let k = known_side(c, dn, ceiling, tol, budget, to_root, solid, clear)?;
                all = Known { top: all.top.max(k.top), seed: higher(all.seed, k.seed), work: all.work.add(k.work) };
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
            let mut all = Known { top: ceiling, seed: None, work: ProofWork::default() };
            let (plain, read): (Vec<&SdfNode>, Vec<&SdfNode>) = of.iter().partition(|c| !has_kernel(c));
            // a smooth maximum is at least either operand, so past one's ball (at its slope outside
            // it) nothing is left
            let slopes: Vec<f64> = plain.iter().map(|c| crate::bounds::slopes_anywhere(c).outside.max(1.0)).collect();
            let cleared = |p: P3, r: f64| clear(p, r) || plain.iter().zip(&slopes).any(|(c, s)| distance(c, v3(p)) > r * s);
            for c in plain.iter().chain(&read) {
                let k = known_side(c, dn, all.top, tol, budget, to_root, solid, &cleared)?;
                all = Known { top: all.top.min(k.top), seed: higher(all.seed, k.seed), work: all.work.add(k.work) };
            }
            Some(all)
        }
        // A cut only takes away: deep inside one nothing is left.
        SdfNode::Combine { op: SdfOp::Difference, of, .. } => {
            // (at its slope inside it: none deep inside an ellipsoid, whose estimate jumps at its centre)
            let cuts: Vec<(&SdfNode, f64)> = of[1..].iter().filter(|c| !has_kernel(c)).map(|c| (c, crate::bounds::slopes_anywhere(c).inside.max(1.0))).collect();
            let cleared = |p: P3, r: f64| clear(p, r) || cuts.iter().any(|(c, s)| -distance(c, v3(p)) > r * s);
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
    let mut work = [[ProofWork::default(); 3]; 2];
    let mut proofs = [[None; 3]; 2];
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
        let dn = d.map(|x| sign * x / len);
        let s = side(dn, if sign < 0.0 && axis == 2 { BOTTOM_BUDGET } else { BUDGET })?;
        work[(sign > 0.0) as usize][axis] = s.work;
        proofs[(sign > 0.0) as usize][axis] = Some(SideProof { dn, side: s });
        let at = sign * s.bound * len + m[axis][3];
        if sign > 0.0 { hi[axis] = at } else { lo[axis] = at }
        if let Some(w) = s.witness {
            let q: P3 = std::array::from_fn(|r| m[r][0] * w[0] + m[r][1] * w[1] + m[r][2] * w[2] + m[r][3]);
            sides[(sign > 0.0) as usize][axis] = Some(q[axis]);
            all = Some(all.map_or((q, q), |(a, b)| (std::array::from_fn(|k| a[k].min(q[k])), std::array::from_fn(|k| b[k].max(q[k])))));
        }
    }
    let found = all.map(|(a, b)| (std::array::from_fn(|k| sides[0][k].unwrap_or(a[k])), std::array::from_fn(|k| sides[1][k].unwrap_or(b[k]))));
    Some(Extent { min: lo, max: hi, found, work, proofs })
}

/// One side of a solid along a unit direction.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Side {
    /// No solid point reaches past this along the direction.
    bound: f64,
    /// The solid point farthest along the direction found: within `tol` of `bound` where the
    /// proof closed (None: none found, the bound being the floor it was asked to prove past or
    /// what a budget left).
    witness: Option<P3>,
    work: ProofWork,
}

/// Side `dn` of node `n`'s solid (`within`: where a node without a tree box has it), read through
/// `reads` (made for `n`).
///
/// `past`: an earlier shape's side and the boxes outside which its solid is this one's, its
/// witness outside them. The solid outside them lies under its bound and holds its witness, so
/// the side is proven over the boxes alone, past it.
fn node_side(n: &SdfNode, reads: &Reads, within: Bounds, lip: &dyn Fn() -> f64, centres: Option<Bounds>, dn: P3, tol: f64, budget: usize, past: Option<(&[Bounds], Side)>) -> Option<Side> {
    // one stream of nearby reads each, the side's
    let hints = std::cell::RefCell::new(crate::sdf::Hints::default());
    let f = |p: P3, cap: f64| reads.field.distance_bounded(v3(p), -cap, cap, &mut hints.borrow_mut());
    // the outer tree's ball: its value over its slope outside its solid round the point (none
    // where its chunks prove none)
    let outer_hints = std::cell::RefCell::new(crate::sdf::Hints::default());
    let g = reads.outer.as_ref().map(|t| {
        let outer_hints = &outer_hints;
        move |p: P3, cap: f64| {
            let v = t.distance_bounded(v3(p), -cap, cap, &mut outer_hints.borrow_mut());
            if v <= 0.0 {
                return v;
            }
            let s = t.slopes_in(&(p.map(|x| x - v), p.map(|x| x + v)), v).outside;
            if s.is_finite() {
                v / s.max(1.0)
            } else {
                f64::NEG_INFINITY
            }
        }
    });
    let outer = g.as_ref().map(|g| g as &dyn Fn(P3, f64) -> f64);
    let slope = |b: &Bounds, reach: f64| reads.field.slopes_in(b, reach).outside;
    // A field steeper than a distance is read only inside `within`, so a side starts no farther
    // out: no ball could bring it in from past there.
    let readable = |b: Bounds| centres.map_or(Some(b), |c| meet(&b, &c));
    // The side of the solid in `region` past `base`'s, where the solid outside the region lies
    // under `base`'s bound and holds its witness (none: no solid outside). The work is both's.
    let over = |region: Bounds, base: Option<Side>| -> Option<Side> {
        let floor = base.map_or(f64::NEG_INFINITY, |b| b.bound);
        if support(&region, dn) <= floor {
            return base;
        }
        let empty = |b: &Bounds| slab_empty(|m| reads.field.above(m), &region, b);
        match (Prover { f: &f, outer, lip, slope: Some(&slope), clip: region, centres, empty: Some(&empty), tol, budget }.side(dn, floor)) {
            // the base's witness is solid still, should the region's proof have found none
            Some(s) if s.bound > floor => Some(Side { witness: s.witness.or(base.and_then(|b| b.witness)), work: s.work.add(base.map_or(ProofWork::default(), |b| b.work)), ..s }),
            Some(s) => base.map(|b| Side { work: b.work.add(s.work), ..b }),
            None => base,
        }
    };
    let clip = || readable(clip_of(n, &within));
    let whole = || over(clip()?, None);
    if let Some((regions, base)) = past {
        let clip = clip()?;
        return regions.iter().filter_map(|r| meet(r, &clip)).fold(Some(base), |b, r| over(r, b));
    }
    let SdfNode::Sculpt { strokes, of } = n else {
        return whole();
    };
    // Raising adds to the solid only within its tubes' reach and lowering takes away only within
    // its own: outside them the unsculpted solid stays, so its side holds while its witness is
    // out of every lowering's reach.
    let base = match node_side(of, &Reads::new(of, &within, |_| Some(lip()), within), within, lip, centres, dn, tol, budget, None) {
        Some(b) if b.witness.is_some_and(|w| !lowered(strokes, w, tol)) => Some(b),
        None => None,
        Some(_) => return whole(),
    };
    strokes.raised_bounds().and_then(readable).map_or(base, |raised| over(raised, base))
}

/// `n` read in chunks of `region` ([`crate::Chunks`]): the proof's many reads near a side make
/// their values from only the pieces near it, bit for bit the tree's.
fn chunked(n: &SdfNode, region: Bounds) -> crate::chunks::Chunks {
    let volume = (0..3).map(|k| region.1[k] - region.0[k]).product::<f64>();
    crate::chunks::Chunks::new(std::sync::Arc::new(n.clone()), region, (volume / 16384.0).cbrt())
}

/// The longest side of `b`.
fn extent(b: &Bounds) -> f64 {
    (0..3).map(|k| b.1[k] - b.0[k]).fold(0.0, f64::max)
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
    /// The field's value at a point, clamped to a cap (past it a read proves no more than the
    /// cap: its sign is kept).
    f: &'a dyn Fn(P3, f64) -> f64,
    /// The radius [`outer_tree`]'s field clears at a point (its value, clamped to the cap, over
    /// its slope there): its field is at most `f`, and readable anywhere.
    outer: Option<&'a dyn Fn(P3, f64) -> f64>,
    /// How steep the field can be outside its solid where `slope` proves no bound (at least 1:
    /// the cut's box is a distance), asked only where a ball needs it.
    lip: &'a dyn Fn() -> f64,
    /// How steep the field can be in a box at the values from 0 to a reach (infinite: no bound
    /// but `lip`): a ball clears its value over the slope of the box it lies in at the values
    /// under it, all a line out of it passes before the surface. None: `lip` everywhere.
    slope: Option<&'a dyn Fn(&Bounds, f64) -> f64>,
    /// The solid that counts: the field's inside this box.
    clip: Bounds,
    /// Where the field's value may centre a ball (None: anywhere); elsewhere only the box's
    /// distance does.
    centres: Option<Bounds>,
    /// Whether the field is proven above 0 everywhere in a box ([`slab_empty`]: its tree's
    /// bounds there), counted as `SLAB_COST` evaluations.
    empty: Option<&'a dyn Fn(&Bounds) -> bool>,
    tol: f64,
    /// Field evaluations it may spend.
    budget: usize,
}

/// What a slab's bounds cost in evaluations: a tree's bounds over a box read each of its pieces
/// once, about what a few points of its chunks do.
const SLAB_COST: usize = 4;

/// Whether a tree holds no solid in box `b` inside `clip`: `above`, its bounds over their common
/// box ([`crate::Chunks::above`]), are above 0. A box a flat side's cutter makes, or any piece
/// whose value its formula bounds over a box, is proven empty however near the side it comes,
/// where a ball proves only what a point's value reaches.
fn slab_empty(above: impl Fn(&Bounds) -> bool, clip: &Bounds, b: &Bounds) -> bool {
    meet(b, clip).map_or(true, |m| above(&m))
}

impl Prover<'_> {
    /// The field cut to the box, clamped to `cap`: its solid is the field's inside the box.
    fn value(&self, p: P3, cap: f64) -> f64 {
        (self.f)(p, cap).max(box_distance(&self.clip, p))
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
        // a ball's radius by the slope round it outside the solid, `lip` where none is proven
        let radius = |c: P3, v: f64| -> f64 {
            // the ball lies in the box of side 2 v about its centre
            let local = self.slope.filter(|_| v > 0.0).map_or(f64::INFINITY, |slope| slope(&(c.map(|x| x - v), c.map(|x| x + v)), v));
            v / if local.is_finite() { local.max(1.0) } else { (self.lip)().max(1.0) }
        };
        let ball = |c: P3, v: f64| clear(radius(c, v));
        // a side is done once every column is down to this past the best solid point: the side
        // then ends `END` of `tol` past that point
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
                let cap = lam + CAP * rho;
                // The outer tree first: past the column's reach it answers alone, without a kernel.
                // Nearer the solid the field may clear farther, and only it finds solid.
                let limit = self.outer.map_or(f64::NEG_INFINITY, |g| {
                    evals += 1;
                    g(c, cap).max(box_distance(&self.clip, c))
                });
                let r = if limit > rho {
                    clear(limit)
                } else if self.centres.map_or(true, |b| inside(&b, c)) {
                    let val = self.value(c, cap);
                    evals += 1;
                    if val <= 0.0 {
                        // solid at the column's top
                        raise(&mut best, z, c);
                        break;
                    }
                    ball(c, val).max(clear(limit))
                } else {
                    // past the box nothing counts: its distance clears at its own slope of 1
                    clear(box_distance(&self.clip, c)).max(clear(limit))
                };
                if r > rho && z - (r * r - rho * rho).sqrt() < low {
                    low = z - (r * r - rho * rho).sqrt();
                    out = lam;
                }
                if low <= goal(&best) || col.top - low >= rho || (i > 0 && r < lam) {
                    break;
                }
            }
            // Where the balls stall, slabs under the column its tree proves empty: first all of
            // it down to the goal (a flat side closes on a witness so), then twice as deep each
            // while they hold.
            if let Some(empty) = self.empty.filter(|_| col.top - low < rho && low > goal(&best)) {
                let slab = |z0: f64, z1: f64| -> Bounds {
                    let (mut b0, mut b1) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
                    for c in 0..8 {
                        let p = at(col.a + if c & 1 == 0 { -col.ha } else { col.ha }, col.b + if c & 2 == 0 { -col.hb } else { col.hb }, if c & 4 == 0 { z0 } else { z1 });
                        for k in 0..3 {
                            b0[k] = b0[k].min(p[k]);
                            b1[k] = b1[k].max(p[k]);
                        }
                    }
                    (b0, b1)
                };
                let floor = goal(&best);
                evals += SLAB_COST;
                if empty(&slab(floor, low)) {
                    low = floor;
                } else {
                    let mut h = rho.max(near);
                    while low - h > floor && evals < self.budget {
                        evals += SLAB_COST;
                        if !empty(&slab(low - h, low)) {
                            break;
                        }
                        low -= h;
                        h *= 2.0;
                    }
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
                let val = self.value(c, CAP * rho.max(near));
                evals += 1;
                if val <= 0.0 {
                    raise(&mut best, z, c);
                    break;
                }
                z -= radius(c, val).max(0.125 * self.tol);
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
        if let Some(bound) = spent {
            let witness = best.map_or_else(|| "none".to_string(), |b| format!("{:.9}", b.0));
            let gap = best.map_or_else(|| "none".to_string(), |b| format!("{:.9}", bound - b.0));
            makepad_script::makepad_error_log::log!("field side proof exhausted: direction=[{:.9}, {:.9}, {:.9}] bound={bound:.9} witness={witness} gap={gap} evaluations={evals}", dn[0], dn[1], dn[2]);
        }
        // A side that closes on its box's face (no solid lies past it) finds its witness again
        // down the witness's column from the face, as a column looks for solid: the first solid
        // point there, so an edit that leaves the solid by the face as it was finds the same
        // one (a flat side: the same height), wherever its search first met the solid.
        if let (Some((s, p)), None) = (best, spent) {
            if s < hi[2] && s + END * self.tol >= hi[2] {
                let (a, b) = (dot(p, u), dot(p, v));
                let mut z = hi[2];
                while z > s && evals < self.budget {
                    let c = at(a, b, z);
                    let val = self.value(c, CAP * near);
                    evals += 1;
                    if val <= 0.0 {
                        best = Some((z, c));
                        break;
                    }
                    z -= radius(c, val).max(0.125 * self.tol);
                }
            }
        }
        let work = ProofWork { evaluations: evals, budget_exhausted: spent.is_some() };
        Some(match (best, spent) {
            (_, Some(top)) if top <= floor => Side { bound: floor, witness: None, work },
            (best, Some(top)) => Side { bound: top, witness: best.map(|b| b.1), work },
            // never past the box's own side, which holds the solid as it is
            (Some((s, p)), None) if s + near >= floor => Side { bound: (s + END * self.tol).min(hi[2]), witness: Some(p), work },
            (_, None) if floor > f64::NEG_INFINITY => Side { bound: floor, witness: None, work },
            (None, None) => return None,
            (Some(_), None) => unreachable!("a witness under the floor leaves the floor finite"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_exhausted_side_keeps_its_open_bound_and_solid_witness() {
        let f = |p: P3, cap: f64| (5.0 - p[2]).clamp(-cap, cap);
        let clip = ([-10.0, -10.0, 0.0], [10.0, 10.0, 10.0]);
        let s = Prover { f: &f, outer: None, lip: &|| 2.0, slope: None, clip, centres: Some(clip), empty: None, tol: 0.01, budget: 128 }
            .side([0.0, 0.0, -1.0], f64::NEG_INFINITY).expect("a bottom");
        let w = s.witness.expect("a solid witness");
        assert!(s.work.budget_exhausted, "this small budget must leave the flat side open");
        assert!(s.work.evaluations >= 128, "report the work spent");
        assert!(f(w, f64::INFINITY) <= 0.0, "the witness must be solid");
        assert!(-s.bound <= 5.0, "the bound must contain the flat bottom");
        assert!(w[2] + s.bound > 0.01, "the open interval must remain visible");
    }
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
        let side = node_side(&root, &Reads::new(&root, &within, |_| Some(1.4), within), within, &|| 1.4, Some(within), [0.0, 0.0, 1.0], 0.01, 5_000, None).expect("a solid");
        assert!(side.bound <= 8.0 && side.bound >= 7.9, "{side:?}");
        let f = |p: P3, cap: f64| distance(&root, v3(p)).clamp(-cap, cap);
        let past = Prover { f: &f, outer: None, lip: &|| 1.4, slope: None, clip: ([-10.0; 3], [10.0; 3]), centres: Some(within), empty: None, tol: 0.01, budget: 5_000 };
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
        // the proof's own reads of its slope, through its chunks (measured once a ball needs it)
        reads.swap(0, std::sync::atomic::Ordering::Relaxed);
        Reads::of_shape(&shape).lip();
        let measuring = reads.swap(0, std::sync::atomic::Ordering::Relaxed);
        let e = shape_bounds(&shape, &I, 0.01).expect("a solid");
        let proving = reads.load(std::sync::atomic::Ordering::Relaxed) - measuring;
        let (lo, hi) = e.found.expect("solid points");
        assert!((0..3).all(|k| lo[k] - e.min[k] <= 0.01 && e.max[k] - hi[k] <= 0.01), "open: {:?} {:?} found {lo:?} {hi:?}", e.min, e.max);
        assert!(proving < 5_000, "{proving} kernel reads");
        holds(&shape, e.min, e.max, 2.3, 0.37);
    }

    /// Twice a capsule's distance: a kernel twice as steep as a distance.
    struct SteepLeg;
    impl Sdf3 for SteepLeg {
        fn distance(&self, p: Vec3d) -> f64 {
            let y = p.y.clamp(3.0, 27.0);
            2.0 * ((p.x - 4.5).hypot(p.z).hypot(p.y - y) - 2.1)
        }
    }

    /// Feet of ellipsoids blended with a steep kernel's leg and carved flat by a box's top, as a
    /// figure stands: the bottom closes on the carved flat, within `tol` of it, though the field
    /// is steeper than a distance.
    #[test]
    fn a_carved_flat_bottom_closes_under_a_steep_kernel() {
        let leg = SdfNode::Kernel(crate::shape::SdfKernel { key: 7, field: std::sync::Arc::new(SteepLeg), splash: None, bounds: Some(([2.0, 0.0, -3.0], [7.0, 30.0, 3.0])), solid: None });
        let foot = |x: f64, z: f64, r: [f64; 3]| at([x, 2.0, z], SdfNode::Ellipsoid { radii: r });
        let feet = SdfNode::Combine { op: SdfOp::Union, blend: 2.0, of: vec![foot(7.0, -9.0, [6.0, 3.8, 6.5]), foot(9.5, 1.0, [8.5, 3.2, 13.0]), foot(16.0, 11.0, [3.6, 3.0, 4.2])] };
        let body = SdfNode::Combine { op: SdfOp::Union, blend: 1.5, of: vec![feet, leg] };
        let root = SdfNode::Combine { op: SdfOp::Difference, blend: 0.2, of: vec![body, at([0.0, -4.7, 0.0], SdfNode::Box { size: [80.0, 10.0, 80.0], radius: 0.0 })] };
        let (min, max) = crate::bounds::node_bounds(&root).expect("bounded");
        let shape = SdfShape { root, min, max, materials: Vec::new() };
        assert!(shape.lipschitz() > 1.5, "steeper than a distance");
        // the frame stands Y up
        let m: Affine = [[1.0, 0.0, 0.0, 0.0], [0.0, 0.0, -1.0, 0.0], [0.0, 1.0, 0.0, 0.0]];
        let tol = 0.01;
        let b = shape_bottom(&shape, &m, tol).expect("a bottom");
        let w = b.witness.expect("a solid point");
        assert!(!b.work.budget_exhausted && b.work.evaluations < 20_000, "{b:?}");
        assert!(b.bound <= 0.3 && b.bound >= 0.3 - tol && w >= 0.3 && w - b.bound <= tol, "{b:?}");
    }

    /// A figure standing on feet carved flat by a box's top, blended with a body and arms: its
    /// bottom is the cutter's face, its witness within a step of it, and replacing a heel that
    /// stands on the face keeps both to the bit, fresh or proven after the old shape: the part
    /// rests where it did.
    #[test]
    fn a_carved_figures_bottom_is_the_cutters_face_whatever_stands_on_it() {
        let figure = |heel: SdfNode| {
            let feet = SdfNode::Combine { op: SdfOp::Union, blend: 2.0, of: vec![at([-6.0, 2.0, -8.0], heel), at([-9.0, 1.8, 2.0], SdfNode::Ellipsoid { radii: [8.0, 3.0, 12.0] }), at([7.0, 2.0, -4.0], SdfNode::Ellipsoid { radii: [6.0, 3.8, 9.0] })] };
            let lower = SdfNode::Combine { op: SdfOp::Difference, blend: 0.2, of: vec![feet, at([0.0, -4.7, 0.0], SdfNode::Box { size: [80.0, 10.0, 80.0], radius: 0.0 })] };
            let body = SdfNode::Combine {
                op: SdfOp::Union,
                blend: 0.8,
                of: vec![
                    lower,
                    SdfNode::Capsule { a: [0.0, 12.0, -3.0], b: [0.0, 30.0, -3.0], radius: 4.0 },
                    SdfNode::Capsule { a: [-4.0, 26.0, -3.0], b: [-15.0, 18.0, -3.0], radius: 1.5 },
                    SdfNode::Capsule { a: [4.0, 26.0, -3.0], b: [15.0, 18.0, -3.0], radius: 1.5 },
                ],
            };
            let root = SdfNode::Combine { op: SdfOp::Union, blend: 0.2, of: vec![body, at([0.0, 38.0, -3.0], SdfNode::Sphere { radius: 6.0 })] };
            let (min, max) = crate::bounds::node_bounds(&root).expect("bounded");
            SdfShape { root, min, max, materials: Vec::new() }
        };
        let (old, new) = (figure(SdfNode::Ellipsoid { radii: [5.0, 3.5, 6.0] }), figure(SdfNode::Sphere { radius: 5.0 }));
        let face: f64 = -4.7 + 5.0;
        let tol = 0.01;
        let before = shape_bounds(&old, &I, tol).expect("a solid");
        let bottom = |e: &Extent| (e.min[1].to_bits(), e.found.expect("solid points").0[1].to_bits());
        let w = before.found.expect("solid points").0[1];
        assert!(before.min[1] == face && w >= face && w <= face + 0.125 * tol, "the bottom and its witness: {:?} {:?}", before.min, before.found);
        let fresh = shape_bounds(&new, &I, tol).expect("a solid");
        let after = shape_bounds_after(&new, &I, tol, &[(&old, &before)]).expect("a solid");
        assert_eq!((bottom(&fresh), bottom(&after)), (bottom(&before), bottom(&before)), "the heel replaced");
        holds(&new, after.min, after.max, 0.25, 0.37);
    }

    /// A figure as a modeller makes one, standing Y up: feet of ellipsoids blended with a steep
    /// kernel's legs and carved flat, a tapered torso, a skirt of sixty blended lumps, arms of
    /// capsules and a head, all in one smooth union. Its six sides close within `tol` of the
    /// solid, none on its budget, for a few thousand reads each.
    #[test]
    fn a_figure_with_a_steep_kernel_closes_every_side_for_few_reads() {
        let leg = SdfNode::Kernel(crate::shape::SdfKernel { key: 7, field: std::sync::Arc::new(SteepLeg), splash: None, bounds: Some(([2.0, 0.0, -3.0], [7.0, 30.0, 3.0])), solid: None });
        let foot = |x: f64, z: f64, r: [f64; 3]| at([x, 2.0, z], SdfNode::Ellipsoid { radii: r });
        let feet = SdfNode::Combine { op: SdfOp::Union, blend: 2.0, of: vec![foot(7.0, -9.0, [6.0, 3.8, 6.5]), foot(9.5, 1.0, [8.5, 3.2, 13.0]), foot(-7.0, -9.0, [6.0, 3.8, 6.5]), foot(-9.5, 1.0, [8.5, 3.2, 13.0])] };
        let lower = SdfNode::Combine { op: SdfOp::Difference, blend: 0.2, of: vec![SdfNode::Combine { op: SdfOp::Union, blend: 1.5, of: vec![feet, leg] }, at([0.0, -4.7, 0.0], SdfNode::Box { size: [80.0, 10.0, 80.0], radius: 0.0 })] };
        let torso = at([0.0, 48.0, 0.0], SdfNode::deform(crate::shape::SdfDeform::Taper { rate: -0.02 }, SdfNode::Box { size: [19.0, 22.0, 14.0], radius: 6.0 }));
        let skirt = SdfNode::Combine {
            op: SdfOp::Union,
            blend: 1.0,
            of: (0..60).map(|i| {
                let a = i as f64 * 2.399_963;
                at([14.0 * a.cos(), 22.0 + 0.25 * i as f64, 14.0 * a.sin()], SdfNode::Ellipsoid { radii: [4.0, 3.0, 3.5] })
            }).collect(),
        };
        let arm = |s: f64| SdfNode::Capsule { a: [s * 10.0, 56.0, 0.0], b: [s * 24.0, 36.0, 3.0], radius: 2.0 };
        let head = at([0.0, 80.0, 0.0], SdfNode::Ellipsoid { radii: [20.0, 19.0, 17.0] });
        let root = SdfNode::Combine { op: SdfOp::Union, blend: 0.8, of: vec![lower, torso, skirt, arm(1.0), arm(-1.0), head] };
        let (min, max) = crate::bounds::node_bounds(&root).expect("bounded");
        let shape = SdfShape { root, min, max, materials: Vec::new() };
        let m: Affine = [[1.0, 0.0, 0.0, 0.0], [0.0, 0.0, -1.0, 0.0], [0.0, 1.0, 0.0, 0.0]];
        let tol = 0.01;
        let e = shape_bounds(&shape, &m, tol).expect("a solid");
        let (lo, hi) = e.found.expect("solid points");
        let reads: usize = e.work.iter().flatten().map(|w| w.evaluations).sum();
        assert!(e.work.iter().flatten().all(|w| !w.budget_exhausted), "{:?}", e.work);
        assert!((0..3).all(|k| lo[k] - e.min[k] <= tol && e.max[k] - hi[k] <= tol), "open: {:?} {:?} found {lo:?} {hi:?}", e.min, e.max);
        assert!((e.min[2] - 0.3).abs() <= tol, "stands on its carved flat: {}", e.min[2]);
        assert!(reads < 60_000, "{reads} reads");
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

    /// A flat ellipsoid body with a ball for an arm, the arm grown: the five sides the arm stays
    /// short of keep their bounds and witnesses to the bit at no reads, and the side it reaches
    /// is proven anew, as a fresh proof proves it, past no solid of a dense sample and within
    /// `tol` of the farthest.
    #[test]
    fn an_edit_reproves_only_the_sides_it_reaches() {
        let figure = |arm: f64| body_and_ball(0.0, [18.0, 0.0, 0.0], arm);
        let (old, new) = (figure(3.0), figure(3.5));
        let after = reproves_only(&old, &new, (1, 0));
        // the farthest solid of a dense sample over the arm's end
        let f = new.field();
        let mut far = f64::NEG_INFINITY;
        for i in 0..=200 {
            for j in 0..=60 {
                for k in 0..=60 {
                    let p = Vec3d::new(21.0 + 0.0025 * i as f64, -1.5 + 0.05 * j as f64, -1.5 + 0.05 * k as f64);
                    if f.distance(p) <= 0.0 {
                        far = far.max(p.x);
                    }
                }
            }
        }
        assert!(far <= after.max[0] && after.max[0] <= far + 0.01 + 0.0025, "the arm's side {} holds the farthest solid sampled {far} within tol", after.max[0]);
    }

    /// The body with a ball blended on for a foot, the foot grown: it moves the bottom and the
    /// shape's box with it, and no other side. The blend reaches a step's width round the foot
    /// (not the bottom's slab across the box, nor past the body's narrow sides), so the other
    /// five sides keep their proofs to the bit, and the bottom closes on a dense sample.
    #[test]
    fn a_grown_foot_reproves_only_the_bottom() {
        let figure = |foot: f64| body_and_ball(1.0, [0.0, -7.0, 0.0], foot);
        let (old, new) = (figure(2.0), figure(2.5));
        assert!(new.min[1] < old.min[1], "the box grew down with the foot");
        let after = reproves_only(&old, &new, (0, 1));
        let f = new.field();
        let mut low = f64::INFINITY;
        for i in 0..=400 {
            for j in 0..=40 {
                for k in 0..=40 {
                    let p = Vec3d::new(-1.0 + 0.05 * j as f64, -9.75 + 0.0025 * i as f64, -1.0 + 0.05 * k as f64);
                    if f.distance(p) <= 0.0 {
                        low = low.min(p.y);
                    }
                }
            }
        }
        assert!(after.min[1] <= low && low <= after.min[1] + 0.01 + 0.0025, "the foot's side {} holds the lowest solid sampled {low} within tol", after.min[1]);
    }

    /// A small ball by the body's side, inside it along x, grown out past it: its region
    /// reaches past the side, off the side's old witness on the body, so the side is proven
    /// over the region alone, past the old bound: past no solid of a dense sample over the ball,
    /// within `tol` of the fresh proof, for fewer reads; the other five sides are kept.
    #[test]
    fn a_side_an_edit_passes_off_its_witness_is_proven_over_the_edit() {
        let figure = |r: f64| body_and_ball(0.0, [9.5, 4.0, 0.0], r);
        let (old, new) = (figure(0.3), figure(0.8));
        let tol = 0.01;
        let before = shape_bounds(&old, &I, tol).expect("a solid");
        let w = before.proofs[1][0].and_then(|p| p.side.witness).expect("a witness");
        assert!(w[1] < 3.0, "the old side is the body's: {w:?}");
        let after = shape_bounds_after(&new, &I, tol, &[(&old, &before)]).expect("a solid");
        let fresh = shape_bounds(&new, &I, tol).expect("a solid");
        let (_, hi) = fresh.found.expect("solid points");
        assert!(after.max[0] >= hi[0] && (after.max[0] - fresh.max[0]).abs() <= tol, "the ball's side {} by the fresh {} (solid to {})", after.max[0], fresh.max[0], hi[0]);
        let (reads, all) = (after.work[1][0].evaluations, fresh.work[1][0].evaluations);
        assert!(reads > 0 && 2 * reads < all, "proven over the ball's region: {reads} reads, the whole side {all}");
        for (s, a) in [(0, 0), (0, 1), (1, 1), (0, 2), (1, 2)] {
            assert!(after.proofs[s][a].map(|p| p.side.bound.to_bits()) == before.proofs[s][a].map(|p| p.side.bound.to_bits()) && after.work[s][a].evaluations == 0, "side {s} {a} kept");
        }
        holds(&new, after.min, after.max, 0.25, 0.37);
        let f = new.field();
        let mut far = f64::NEG_INFINITY;
        for i in 0..=80 {
            for j in 0..=20 {
                for k in 0..=20 {
                    let p = Vec3d::new(10.2 + 0.0025 * i as f64, 3.9 + 0.01 * j as f64, -0.1 + 0.01 * k as f64);
                    if f.distance(p) <= 0.0 {
                        far = far.max(p.x);
                    }
                }
            }
        }
        assert!(far <= after.max[0] && after.max[0] <= far + tol + 0.0025, "the side {} holds the farthest solid sampled {far} within tol", after.max[0]);
    }

    /// Random edits of a small figure from fixed seeds, each extent proven after the last few
    /// as an app proves them ([`shape_bounds_after`]): every side holds the fresh proof's solid
    /// points and lies past its bound or within `tol` of it, every witness is solid, no solid lies
    /// past a side on a grid across it, and a good share of the sides are kept.
    #[test]
    fn random_edits_keep_only_sound_sides() {
        use crate::bounds::tests::{Figure, Rng};
        let tol = 0.01;
        let (mut kept, mut sides) = (0, 0);
        for seed in 1..=4u64 {
            let mut r = Rng(0x2545_f491_4f6c_dd1d ^ (seed << 32));
            let mut fig = Figure::new();
            let mut recent: Vec<(SdfShape, Extent)> = Vec::new();
            for _ in 0..50 {
                let what = fig.edit(&mut r);
                let root = fig.node();
                let (min, max) = crate::bounds::node_bounds(&root).expect("bounded");
                let shape = SdfShape { root, min, max, materials: Vec::new() };
                let before: Vec<(&SdfShape, &Extent)> = recent.iter().rev().map(|(s, e)| (s, e)).collect();
                let after = shape_bounds_after(&shape, &I, tol, &before).expect("a solid");
                let fresh = shape_bounds(&shape, &I, tol).expect("a solid");
                let (lo, hi) = fresh.found.expect("solid points");
                for a in 0..3 {
                    assert!(after.min[a] <= lo[a] && after.max[a] >= hi[a], "seed {seed}, a {what} edit: {:?}..{:?} short of the fresh proof's solid {lo:?}..{hi:?}", after.min, after.max);
                    assert!(after.min[a] <= fresh.min[a] + tol && after.max[a] >= fresh.max[a] - tol, "seed {seed}, a {what} edit: {:?}..{:?} inside the fresh bounds {:?}..{:?} by more than tol", after.min, after.max, fresh.min, fresh.max);
                }
                // a side's proof as reuse compares it: direction, bound, witness
                let bits = |p: &SideProof| (p.dn.map(f64::to_bits), p.side.bound.to_bits(), p.side.witness.map(|w| w.map(f64::to_bits)));
                let f = shape.field();
                for p in after.proofs.iter().flatten() {
                    let p = p.expect("every side proven");
                    let w = p.side.witness.expect("a witness");
                    assert!(f.distance(v3(w)) <= 0.0, "seed {seed}, a {what} edit: side {:?}'s witness {w:?} is not solid", p.dn);
                    sides += 1;
                    if before.iter().any(|(_, e)| e.proofs.iter().flatten().flatten().any(|q| bits(q) == bits(&p))) {
                        kept += 1;
                    }
                }
                holds(&shape, after.min, after.max, 1.0, r.unit());
                recent.push((shape, after));
                if recent.len() > 8 {
                    recent.remove(0);
                }
            }
        }
        assert!(kept * 4 > sides, "{kept} of {sides} sides kept");
    }

    /// A flat ellipsoid body and a ball at `at` in a union of `blend`, in the tree's box.
    fn body_and_ball(blend: f64, at: [f64; 3], radius: f64) -> SdfShape {
        let ball = SdfNode::Transform { at, rotate: [0.0; 3], scale: 1.0, of: Box::new(SdfNode::Sphere { radius }) };
        let root = SdfNode::Combine { op: SdfOp::Union, blend, of: vec![SdfNode::Ellipsoid { radii: [10.0, 6.0, 4.0] }, ball] };
        let (min, max) = crate::bounds::node_bounds(&root).expect("bounded");
        SdfShape { root, min, max, materials: Vec::new() }
    }

    /// `new`'s extent after `old`'s: side `reached` (end, axis) proven anew as a fresh proof
    /// proves it, every other kept from `old`'s to the bit at no reads; the box holds `new`.
    fn reproves_only(old: &SdfShape, new: &SdfShape, reached: (usize, usize)) -> Extent {
        let before = shape_bounds(old, &I, 0.01).expect("a solid");
        let after = shape_bounds_after(new, &I, 0.01, &[(old, &before)]).expect("a solid");
        let fresh = shape_bounds(new, &I, 0.01).expect("a solid");
        let side = |e: &Extent, s: usize, a: usize| if s == 0 { e.min[a] } else { e.max[a] };
        // a side's bound and witness, to the bit
        let proof = |e: &Extent, s: usize, a: usize| e.proofs[s][a].map(|p| (p.dn.map(f64::to_bits), p.side.bound.to_bits(), p.side.witness.map(|w| w.map(f64::to_bits))));
        let (rs, ra) = reached;
        for s in 0..2 {
            for a in 0..3 {
                if (s, a) == reached {
                    assert!(side(&after, s, a).to_bits() == side(&fresh, s, a).to_bits() && after.proofs[s][a] == fresh.proofs[s][a], "the edited side {s} {a} is proven as a fresh proof proves it");
                    assert!(proof(&after, s, a) != proof(&before, s, a), "and differs from the old one");
                    assert!(after.work[s][a].evaluations > 0, "and read anew");
                } else {
                    assert!(side(&after, s, a).to_bits() == side(&before, s, a).to_bits() && proof(&after, s, a) == proof(&before, s, a), "side {s} {a} kept: {} of {} (edited {rs} {ra})", side(&after, s, a), side(&before, s, a));
                    assert!(after.work[s][a].evaluations == 0, "side {s} {a} read nothing");
                }
            }
        }
        holds(new, after.min, after.max, 0.25, 0.37);
        after
    }

    /// Sculpted skins (raised, lowered and creased by random strokes, some over the foot and
    /// the rim): the box holds the solid; a side no stroke reaches stays the unsculpted one's.
    #[test]
    fn sculpted_bounds_hold_the_solid() {
        use crate::strokes::{StrokeOp, Strokes};
        let base = footed();
        let Extent { min: blo, max: bhi, found: bfound, .. } = shape_bounds(&base, &I, 0.005).expect("a solid");
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
            let Extent { min: lo, max: hi, found, .. } = shape_bounds(&shape, &I, 0.005).expect("a solid");
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
