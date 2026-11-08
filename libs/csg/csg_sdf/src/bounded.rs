//! A tree's distance read only as far as bounds need: `distance(p).clamp(lower, upper)`, bit for
//! bit, from leaves that are searched only as far as bounds of their own need (a mesh,
//! [`crate::mesh_field::MeshField`]'s bounded distance; tubes with no fillet, theirs).
//!
//! Down the tree each op passes its operand the bounds past which the op keeps its value
//! whatever the operand is: a scale or a rounding moves them, a union passes its own (past them
//! with the other operand past them too, by a quarter of the blend's radius, the union is past
//! them) and the other operand's plus the radius (past that the blend gives the other operand
//! exactly), and under its lower bound with the other under it the operand does not matter at
//! all. A cut and an intersection are unions of opposites. Every value an op reads is its
//! operand's clamped to the bounds it passed, so what a read proves (the value, or that the
//! operand is past a bound) holds of the value read and of the true one alike.
//!
//! An operand's bounds wait on what its siblings are. A combination reads the operands with no
//! leaf read under bounds in full first, then the others in order, each under bounds that take
//! those not read yet as passing the rest through. A read that ended at a bound (or was not
//! needed) is then proven from what all the reads prove, and read again under the proven bounds
//! where they are wider. A proof only gets easier as the others are read again (they prove more),
//! so after that every read is proven, and the combination's value is the one every operand read
//! in full would give, clamped. Margins of 2^-40 of the numbers (and of a blend's radius or a
//! rounding) cover f64 rounding.

use crate::sdf::{Hints, Sdf3};
use crate::shape::{distance, distance_many, framed, smin, v3, Compiled, SdfOp};
use makepad_csg_math::Vec3d;

/// Bounds a value is read under, `lo..=hi`: past either the reader keeps its own value whatever
/// the value is. `lo >= hi`: the value does not matter.
#[derive(Clone, Copy, Debug)]
struct Window {
    lo: f64,
    hi: f64,
}

impl Window {
    const ALL: Window = Window { lo: f64::NEG_INFINITY, hi: f64::INFINITY };
    const MOOT: Window = Window { lo: f64::INFINITY, hi: f64::NEG_INFINITY };

    /// A bound that is not a number is none.
    fn new(lo: f64, hi: f64) -> Window {
        Window { lo: if lo.is_nan() { f64::NEG_INFINITY } else { lo }, hi: if hi.is_nan() { f64::INFINITY } else { hi } }
    }

    fn moot(&self) -> bool {
        self.lo >= self.hi
    }

    fn neg(self) -> Window {
        Window { lo: -self.hi, hi: -self.lo }
    }

    fn clamp(&self, v: f64) -> f64 {
        v.clamp(self.lo, self.hi)
    }
}

/// What is known of an operand's value: it lies in `lo..=hi`.
#[derive(Clone, Copy, Debug)]
struct Known {
    lo: f64,
    hi: f64,
}

impl Known {
    const ANY: Known = Known { lo: f64::NEG_INFINITY, hi: f64::INFINITY };

    fn exact(v: f64) -> Known {
        if v.is_nan() {
            return Known::ANY;
        }
        Known { lo: v, hi: v }
    }

    /// What `v`, an operand read under `w` (so clamped to it), proves of the operand.
    fn read(v: f64, w: Window) -> Known {
        if v.is_nan() {
            Known::ANY
        } else if v >= w.hi {
            Known { lo: w.hi, hi: f64::INFINITY }
        } else if v <= w.lo {
            Known { lo: f64::NEG_INFINITY, hi: w.lo }
        } else {
            Known::exact(v)
        }
    }

    fn neg(self) -> Known {
        Known { lo: -self.hi, hi: -self.lo }
    }

    fn meet(self, o: Known) -> Known {
        Known { lo: self.lo.max(o.lo), hi: self.hi.min(o.hi) }
    }
}

/// 2^-40: f64 rounding of the few ops a bound passes (2^-53 each) stays far under it.
const REL: f64 = 1.0 / 1_099_511_627_776.0;
/// So that a bound moves even at 0.
const LEAST: f64 = 1e-300;

/// `x` raised past what rounding of the ops it passes can take back: by 2^-40 of `|x|` and of
/// `k` (a blend's radius, a rounding: what the ops add to it), and by [`LEAST`]. An infinite `x`
/// stays as it is.
fn up(x: f64, k: f64) -> f64 {
    if !x.is_finite() {
        return x;
    }
    x + (x.abs() + k.abs()) * REL + LEAST
}

/// `x` lowered as [`up`] raises it.
fn down(x: f64, k: f64) -> f64 {
    if !x.is_finite() {
        return x;
    }
    x - (x.abs() + k.abs()) * REL - LEAST
}

/// The bounds `lo..=hi` an operand is read under for bounds of its reader's that are not one
/// value: only where a scale overflows them do they meet, and there it is read in full.
fn inner(lo: f64, hi: f64) -> Window {
    let w = Window::new(lo, hi);
    if w.moot() {
        Window::ALL
    } else {
        w
    }
}

/// The bounds for an operand `x` of `smin(x, y, k)` (or `smin(y, x, k)`) read under `w`, `y`
/// known as `y`. Past `y` by `k` (sharp: past `y`) the blend is `y` exactly; with `y` past
/// `w.hi` and a quarter of `k` too, so is `x` past there: the blend is past `w.hi` (it is at least
/// the lesser less a quarter of `k`); under `w.lo` the blend is under it (it is at most the
/// lesser), and with `y` under it `x` does not matter.
fn union_window(w: Window, y: Known, k: f64) -> Window {
    if w.moot() {
        return Window::MOOT;
    }
    if k.is_nan() {
        return Window::ALL;
    }
    let blend = k > 0.0;
    let lo = if blend { down(w.lo, k) } else { w.lo };
    if y.hi <= lo {
        return Window::MOOT;
    }
    let mut hi = if blend { up(y.hi + k, k) } else { up(y.hi, 0.0) };
    let past = if blend { up(w.hi + 0.25 * k, k) } else { w.hi };
    if y.lo >= past {
        hi = hi.min(past);
    }
    Window::new(lo, hi)
}

/// What is known of `smin(a, b, k)` from what is known of `a` and `b`.
fn union_known(a: Known, b: Known, k: f64) -> Known {
    if k.is_nan() {
        return Known::ANY;
    }
    if a.lo == a.hi && b.lo == b.hi {
        return Known::exact(smin(a.lo, b.lo, k));
    }
    if k > 0.0 {
        Known { lo: down(a.lo.min(b.lo) - 0.25 * k, k), hi: up(a.hi.min(b.hi), k) }
    } else {
        Known { lo: a.lo.min(b.lo), hi: a.hi.min(b.hi) }
    }
}

/// One step of a combination's fold, as [`crate::distance`] takes it.
fn step(op: SdfOp, a: f64, b: f64, k: f64) -> f64 {
    match op {
        SdfOp::Union => smin(a, b, k),
        SdfOp::Intersection => -smin(-a, -b, k),
        SdfOp::Difference => -smin(-a, b, k),
    }
}

fn step_known(op: SdfOp, a: Known, b: Known, k: f64) -> Known {
    match op {
        SdfOp::Union => union_known(a, b, k),
        SdfOp::Intersection => union_known(a.neg(), b.neg(), k).neg(),
        SdfOp::Difference => union_known(a.neg(), b, k).neg(),
    }
}

/// The bounds for a step's first operand read under `w`, its second known as `b`.
fn first_window(op: SdfOp, w: Window, b: Known, k: f64) -> Window {
    match op {
        SdfOp::Union => union_window(w, b, k),
        SdfOp::Intersection => union_window(w.neg(), b.neg(), k).neg(),
        SdfOp::Difference => union_window(w.neg(), b, k).neg(),
    }
}

/// The bounds for a step's second operand read under `w`, its first known as `a`.
fn second_window(op: SdfOp, w: Window, a: Known, k: f64) -> Window {
    match op {
        SdfOp::Union => union_window(w, a, k),
        SdfOp::Intersection => union_window(w.neg(), a.neg(), k).neg(),
        SdfOp::Difference => union_window(w.neg(), a.neg(), k),
    }
}

/// What an operand not read yet is taken as: one that passes the rest through.
fn passing(op: SdfOp, at: usize) -> f64 {
    match op {
        SdfOp::Union => f64::INFINITY,
        SdfOp::Intersection => f64::NEG_INFINITY,
        SdfOp::Difference if at == 0 => f64::NEG_INFINITY,
        SdfOp::Difference => f64::INFINITY,
    }
}

/// One operand of a combination at one point: its value as read (clamped to `used`), and what
/// is known of it.
#[derive(Clone, Copy, Debug)]
struct Operand {
    v: f64,
    known: Known,
    used: Window,
}

impl Operand {
    /// Not read: whatever it is, the combination's value is the same. Its value stands in finite
    /// (two infinite operands would blend to no number), and it proves nothing.
    fn unread(op: SdfOp, at: usize) -> Operand {
        Operand { v: passing(op, at).clamp(-f64::MAX, f64::MAX), known: Known::ANY, used: Window::MOOT }
    }

    /// Its read under `used` is proven by `w`, the bounds what is known of the others proves.
    fn proven(&self, w: Window) -> bool {
        if w.moot() {
            return true;
        }
        if self.used.moot() {
            return false;
        }
        if self.v >= self.used.hi {
            return w.hi <= self.used.hi;
        }
        if self.v <= self.used.lo {
            return w.lo >= self.used.lo;
        }
        true
    }
}

/// The bounds operand `at` of a fold by `op` (blend `k`) of `of` is read under for the fold to be
/// read under `w`, from what is known of the others.
fn window_of(op: SdfOp, k: f64, of: &[Operand], at: usize, w: Window) -> Window {
    let mut w = w;
    for i in (at + 1..of.len()).rev() {
        w = first_window(op, w, of[i].known, k);
        if w.moot() {
            return w;
        }
    }
    if at == 0 {
        return w;
    }
    let mut before = of[0].known;
    for o in &of[1..at] {
        before = step_known(op, before, o.known, k);
    }
    second_window(op, w, before, k)
}

impl Compiled {
    /// `distance(p).clamp(lower, upper)` bit for bit, read only as far as the bounds need (see
    /// the module), where the distance is a number (a value that is none is not carried past
    /// bounds). Panics as `f64::clamp` does where `lower > upper`.
    pub(crate) fn bounded(&self, p: Vec3d, lower: f64, upper: f64, hints: &mut Hints) -> f64 {
        assert!(lower <= upper, "bounds {lower}..={upper}");
        self.read(p, Window { lo: lower, hi: upper }, hints)
    }

    /// [`Compiled::bounded`] at each of `ps`, one stream of queries in order: operands with no
    /// leaf read under bounds are read for all the points in one call.
    pub(crate) fn bounded_many(&self, ps: &[Vec3d], lower: f64, upper: f64, out: &mut [f64], hints: &mut Hints) {
        assert!(lower <= upper, "bounds {lower}..={upper}");
        self.read_many(ps, Window { lo: lower, hi: upper }, out, hints)
    }

    /// The value at `p` clamped to `w` (`w.lo <= w.hi`).
    fn read(&self, p: Vec3d, w: Window, hints: &mut Hints) -> f64 {
        if w.lo >= w.hi {
            return w.lo;
        }
        match self {
            Self::Leaf(n) => w.clamp(distance(n, p)),
            Self::Mesh(m, part) => m.distance_bounded(p, w.lo, w.hi, hints.part(*part)),
            Self::Tubes(t) => t.distance_bounded(p, w.lo, w.hi),
            Self::Combine(op, k, of, bounded) => {
                if !bounded.contains(&true) {
                    return w.clamp(self.distance(p));
                }
                Fold { op: *op, k: *k, of, bounded }.read(p, w, hints, |j| of[j].distance(p))
            }
            Self::Shell(t, of) => {
                // |x| - t past w.hi wherever |x| is past h; with h at 0 or under, everywhere
                let h = up(w.hi + t, *t);
                if h.is_nan() {
                    return w.clamp(self.distance(p));
                }
                if h <= 0.0 {
                    return w.hi;
                }
                w.clamp(of.read(p, Window::new(-h, h), hints).abs() - t)
            }
            Self::Round(r, of) => w.clamp(of.read(p, inner(down(w.lo + r, *r), up(w.hi + r, *r)), hints) - r),
            Self::Transform { at, rotation, scale, of } => {
                let q = framed(at, rotation, *scale, p);
                let s = *scale;
                if !(s > 0.0 && s.is_finite()) {
                    return w.clamp(of.distance(q) * s);
                }
                w.clamp(of.read(q, inner(down(w.lo / s, 0.0), up(w.hi / s, 0.0)), hints) * s)
            }
            Self::Instance(places, of) => {
                // the nearest copy: one past the nearest so far (or past w.hi) is not read past
                // there, and one under w.lo ends it
                let mut best = f64::INFINITY;
                for pl in places {
                    let (s, q) = (pl.scale, v3(pl.into([p.x, p.y, p.z])));
                    let x = if s > 0.0 && s.is_finite() {
                        of.read(q, inner(down(w.lo / s, 0.0), up(w.hi.min(best) / s, 0.0)), hints)
                    } else {
                        of.distance(q)
                    };
                    best = best.min(x * s);
                    if best <= w.lo {
                        break;
                    }
                }
                w.clamp(best)
            }
            Self::Deform(deform, lipschitz, of) => {
                let q = deform.warp(p);
                let l = *lipschitz;
                if !(l > 0.0 && l.is_finite()) {
                    return w.clamp(of.distance(q) / l);
                }
                w.clamp(of.read(q, inner(down(w.lo * l, 0.0), up(w.hi * l, 0.0)), hints) / l)
            }
            Self::Sculpt(..) | Self::Texture(..) => w.clamp(self.distance(p)),
        }
    }

    /// [`Compiled::read`] at each of `ps`.
    fn read_many(&self, ps: &[Vec3d], w: Window, out: &mut [f64], hints: &mut Hints) {
        if w.lo >= w.hi {
            out.iter_mut().for_each(|o| *o = w.lo);
            return;
        }
        match self {
            Self::Leaf(n) => {
                distance_many(n, ps, out);
                out.iter_mut().for_each(|o| *o = w.clamp(*o));
            }
            Self::Combine(op, k, of, bounded) => {
                if !bounded.contains(&true) {
                    self.distance_many(ps, out);
                    out.iter_mut().for_each(|o| *o = w.clamp(*o));
                    return;
                }
                // the operands read in full, for all the points at once
                let full: Vec<Vec<f64>> = of
                    .iter()
                    .zip(bounded)
                    .map(|(c, b)| {
                        let mut v = vec![0.0; if *b { 0 } else { ps.len() }];
                        if !*b {
                            c.distance_many(ps, &mut v);
                        }
                        v
                    })
                    .collect();
                let fold = Fold { op: *op, k: *k, of, bounded };
                for (i, (p, o)) in ps.iter().zip(out.iter_mut()).enumerate() {
                    *o = fold.read(*p, w, hints, |j| full[j][i]);
                }
            }
            Self::Shell(..) | Self::Instance(..) | Self::Mesh(..) | Self::Tubes(_) => {
                for (p, o) in ps.iter().zip(out.iter_mut()) {
                    *o = self.read(*p, w, hints);
                }
            }
            Self::Round(r, of) => {
                of.read_many(ps, inner(down(w.lo + r, *r), up(w.hi + r, *r)), out, hints);
                out.iter_mut().for_each(|o| *o = w.clamp(*o - r));
            }
            Self::Transform { at, rotation, scale, of } => {
                let q: Vec<Vec3d> = ps.iter().map(|p| framed(at, rotation, *scale, *p)).collect();
                let s = *scale;
                if s > 0.0 && s.is_finite() {
                    of.read_many(&q, inner(down(w.lo / s, 0.0), up(w.hi / s, 0.0)), out, hints);
                } else {
                    of.distance_many(&q, out);
                }
                out.iter_mut().for_each(|o| *o = w.clamp(*o * s));
            }
            Self::Deform(deform, lipschitz, of) => {
                let q: Vec<Vec3d> = ps.iter().map(|p| deform.warp(*p)).collect();
                let l = *lipschitz;
                if l > 0.0 && l.is_finite() {
                    of.read_many(&q, inner(down(w.lo * l, 0.0), up(w.hi * l, 0.0)), out, hints);
                } else {
                    of.distance_many(&q, out);
                }
                out.iter_mut().for_each(|o| *o = w.clamp(*o / l));
            }
            Self::Sculpt(..) | Self::Texture(..) => {
                self.distance_many(ps, out);
                out.iter_mut().for_each(|o| *o = w.clamp(*o));
            }
        }
    }
}

/// Operands a combination keeps in place; more go to the heap.
const IN_PLACE: usize = 8;

/// A combination: by `op` (blend `k`) of `of`, those `bounded` read under bounds.
struct Fold<'a> {
    op: SdfOp,
    k: f64,
    of: &'a [Compiled],
    bounded: &'a [bool],
}

impl Fold<'_> {
    /// Its value at `p` clamped to `w`: the operands not `bounded` read in full (`full(j)`), the
    /// others under bounds, proven, and read again where a proof needs wider bounds (see the
    /// module).
    fn read(&self, p: Vec3d, w: Window, hints: &mut Hints, full: impl Fn(usize) -> f64) -> f64 {
        let Fold { op, k, of, bounded } = *self;
        let mut place = [Operand::unread(op, 0); IN_PLACE];
        let mut heap = Vec::new();
        let ops: &mut [Operand] = if of.len() <= IN_PLACE {
            &mut place[..of.len()]
        } else {
            heap.resize(of.len(), Operand::unread(op, 0));
            &mut heap
        };
        for (j, o) in ops.iter_mut().enumerate() {
            *o = if bounded[j] {
                // not read yet: taken as passing the rest through
                let v = passing(op, j);
                Operand { v, known: Known::exact(v), used: Window::MOOT }
            } else {
                let v = full(j);
                Operand { v, known: Known::exact(v), used: Window::ALL }
            };
        }
        let read = |ops: &mut [Operand], j: usize, under: Window, hints: &mut Hints, before: Known| {
            if under.moot() {
                ops[j] = Operand::unread(op, j);
                return;
            }
            let v = of[j].read(p, under, hints);
            ops[j] = Operand { v, known: Known::read(v, under).meet(before), used: under };
        };
        for j in (0..of.len()).filter(|&j| bounded[j]) {
            let under = window_of(op, k, ops, j, w);
            read(ops, j, under, hints, Known::ANY);
        }
        for j in (0..of.len()).filter(|&j| bounded[j]) {
            let under = window_of(op, k, ops, j, w);
            if !ops[j].proven(under) {
                let before = ops[j].known;
                read(ops, j, under, hints, before);
            }
        }
        let mut d = ops[0].v;
        for o in &ops[1..] {
            d = step(op, d, o.v, k);
        }
        w.clamp(d)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunks::Chunks;
    use crate::mesh_field::{MeshData, MeshField};
    use crate::shape::{SdfDeform, SdfKernel, SdfNode, SdfPlace};
    use crate::texture::{SdfTexture, TexProject};
    use crate::tubes::{TubeCurve, Tubes};
    use std::sync::Arc;

    /// A mesh ball of `rings` x `segments` and `radius`, its triangles turning counter-clockwise
    /// seen from outside.
    fn globe(rings: u32, segments: u32, radius: f64) -> SdfNode {
        let mut positions = vec![[0.0, radius as f32, 0.0]];
        for i in 1..rings {
            let phi = std::f64::consts::PI * i as f64 / rings as f64;
            for j in 0..segments {
                let theta = std::f64::consts::TAU * j as f64 / segments as f64;
                positions.push([(radius * phi.sin() * theta.cos()) as f32, (radius * phi.cos()) as f32, (radius * phi.sin() * theta.sin()) as f32]);
            }
        }
        positions.push([0.0, -radius as f32, 0.0]);
        let south = positions.len() as u32 - 1;
        let v = |i: u32, j: u32| 1 + (i - 1) * segments + j % segments;
        let mut triangles = Vec::new();
        for j in 0..segments {
            triangles.push([0, v(1, j + 1), v(1, j)]);
            for i in 1..rings - 1 {
                triangles.push([v(i, j), v(i + 1, j + 1), v(i + 1, j)]);
                triangles.push([v(i, j), v(i, j + 1), v(i + 1, j + 1)]);
            }
            triangles.push([south, v(rings - 1, j), v(rings - 1, j + 1)]);
        }
        SdfNode::Mesh(Arc::new(MeshField::new(MeshData { positions, triangles })))
    }

    /// A mesh cube of side `size` about the origin.
    fn cube_mesh(size: f32) -> SdfNode {
        let h = size * 0.5;
        let positions = (0..8).map(|i| [if i & 1 == 0 { -h } else { h }, if i & 2 == 0 { -h } else { h }, if i & 4 == 0 { -h } else { h }]).collect();
        let quads: [[u32; 4]; 6] = [[0, 2, 3, 1], [4, 5, 7, 6], [0, 1, 5, 4], [2, 6, 7, 3], [0, 4, 6, 2], [1, 3, 7, 5]];
        let triangles = quads.iter().flat_map(|q| [[q[0], q[1], q[2]], [q[0], q[2], q[3]]]).collect();
        SdfNode::Mesh(Arc::new(MeshField::new(MeshData { positions, triangles })))
    }

    /// `n` bent cables side by side from `at`, tubes with no fillet.
    fn cables(n: usize, at: [f64; 3]) -> SdfNode {
        let curves = (0..n)
            .map(|c| {
                let x = at[0] + c as f64 * 3.0;
                TubeCurve { pts: vec![[x, at[1], at[2], 0.8], [x + 1.0, at[1] + 4.0, at[2] + 1.0, 0.6], [x, at[1] + 8.0, at[2] - 1.0, 0.5], [x - 1.0, at[1] + 12.0, at[2], 0.7]], slot: 0 }
            })
            .collect();
        SdfNode::Tubes(Arc::new(Tubes::new(curves, 0.0)))
    }

    fn sphere(radius: f64) -> SdfNode {
        SdfNode::Sphere { radius }
    }

    /// A ball read as a kernel: its points in one call.
    fn kernel_ball(radius: f64) -> SdfKernel {
        SdfKernel { key: 0, field: Arc::new(crate::primitives::SdfSphere::new(Vec3d::new(0.0, 0.0, 0.0), radius)), splash: None, bounds: None, solid: None }
    }

    fn placed(at: [f64; 3], of: SdfNode) -> SdfNode {
        SdfNode::Transform { at, rotate: [0.0; 3], scale: 1.0, of: Box::new(of) }
    }

    fn fold(op: SdfOp, blend: f64, of: Vec<SdfNode>) -> SdfNode {
        SdfNode::Combine { op, blend, of }
    }

    /// Points on a lattice over `-reach..reach`, and the same nudged off it.
    fn points(reach: f64, n: usize) -> Vec<Vec3d> {
        let g = |i: usize| -reach + 2.0 * reach * i as f64 / (n - 1) as f64;
        let mut out = Vec::new();
        for x in 0..n {
            for y in 0..n {
                for z in 0..n {
                    out.push(Vec3d::new(g(x), g(y), g(z)));
                    out.push(Vec3d::new(g(x) + 0.137, g(y) - 0.291, g(z) + 0.053));
                }
            }
        }
        out
    }

    /// Two mesh cages blended into each other, then into solids, and a chain of tubes blended in.
    fn cages_and_cables() -> SdfNode {
        use SdfOp::Union;
        let pair = fold(Union, 1.0, vec![placed([1.0, -2.0, 0.5], globe(12, 24, 4.0)), placed([-4.0, 3.0, 1.0], globe(10, 20, 3.5))]);
        let cages = fold(Union, 1.5, vec![fold(Union, 1.5, vec![pair, placed([6.0, 0.0, 0.0], sphere(2.0))]), SdfNode::Kernel(kernel_ball(1.5))]);
        let chain = fold(Union, 2.8, vec![placed([0.0, -14.0, 0.0], sphere(2.0)), cables(3, [-4.0, -12.0, 0.0])]);
        fold(Union, 0.2, vec![fold(Union, 2.0, vec![fold(Union, 1.0, vec![chain, cages]), placed([0.0, 9.0, 0.0], sphere(2.0))]), placed([9.0, 9.0, 0.0], sphere(1.0))])
    }

    /// Bounds of every kind: none, about 0 (half a cell either side), outside only, inside only,
    /// one value, open either side, narrow.
    const WINDOWS: [(f64, f64); 9] = [(f64::NEG_INFINITY, f64::INFINITY), (-0.5, 0.5), (0.0, 2.0), (-3.0, -1.0), (1.0, 1.0), (f64::NEG_INFINITY, 0.0), (0.0, f64::INFINITY), (2.5, 7.0), (-1e-3, 1e-3)];

    /// A tree read under bounds gives `distance(p).clamp(lower, upper)` bit for bit, one point at
    /// a time (one stream of hints, and each point its own), many at a time, and through chunks:
    /// meshes blended into each other and into solids, kernels and tubes, sharp and smooth,
    /// cutting and cut, intersected, rounded, shelled, scaled and turned, copied, twisted and
    /// textured, under every kind of bounds.
    #[test]
    fn a_bounded_read_is_the_clamped_distance_bit_for_bit() {
        use SdfOp::{Difference, Intersection, Union};
        let g = || placed([1.0, -2.0, 0.5], globe(12, 24, 4.0));
        let c = || placed([-3.0, 2.0, 0.0], cube_mesh(5.0));
        let quarter = [0.0, 0.0, std::f64::consts::FRAC_PI_8.sin(), std::f64::consts::FRAC_PI_8.cos()];
        let knurl = Arc::new(SdfTexture::new(kernel_ball(0.3), TexProject::Axis(1), 1.0, 0.2, 0.01, None, None, 1.0, 0.0));
        let shapes: Vec<(&str, SdfNode)> = vec![
            ("cages and cables", cages_and_cables()),
            ("mesh", g()),
            ("tubes", cables(3, [-4.0, -6.0, 0.0])),
            ("sharp pair", fold(Union, 0.0, vec![g(), c(), sphere(2.0)])),
            ("mesh in the middle", fold(Union, 1.0, vec![placed([0.0, 7.0, 0.0], sphere(2.0)), g(), placed([0.0, -7.0, 0.0], sphere(2.0))])),
            ("mesh and kernel", fold(Union, 1.2, vec![g(), placed([4.0, 0.0, 0.0], SdfNode::Kernel(kernel_ball(2.5)))])),
            ("cut by meshes", fold(Difference, 0.8, vec![SdfNode::Box { size: [14.0; 3], radius: 0.5 }, g(), c()])),
            ("mesh cut", fold(Difference, 1.0, vec![g(), placed([3.0, 3.0, 0.0], sphere(3.0))])),
            ("sharp cut", fold(Difference, 0.0, vec![c(), g()])),
            ("meet", fold(Intersection, 0.7, vec![g(), c(), sphere(6.0)])),
            ("sharp meet", fold(Intersection, 0.0, vec![sphere(5.0), g()])),
            ("rounded", fold(Union, 0.5, vec![SdfNode::Round { radius: 1.2, of: Box::new(g()) }, sphere(1.0)])),
            ("shelled", fold(Union, 0.5, vec![SdfNode::Shell { thickness: 0.6, of: Box::new(c()) }, placed([7.0, 0.0, 0.0], sphere(1.0))])),
            ("scaled and turned", SdfNode::Transform { at: [0.5, 1.0, -1.0], rotate: [10.0, 30.0, -20.0], scale: 1.7, of: Box::new(fold(Union, 1.0, vec![g(), cables(2, [3.0, -5.0, 0.0])])) }),
            ("shrunk", SdfNode::Transform { at: [0.0; 3], rotate: [0.0; 3], scale: 0.4, of: Box::new(fold(Union, 2.0, vec![c(), g()])) }),
            ("copies", SdfNode::Instance { places: vec![SdfPlace { at: [6.0, 0.0, 0.0], rotate: [0.0, 0.0, 0.0, 1.0], scale: 0.5 }, SdfPlace::IDENTITY, SdfPlace { at: [-5.0, 4.0, 0.0], rotate: quarter, scale: 1.3 }], of: Box::new(fold(Union, 0.5, vec![g(), sphere(1.0)])) }),
            ("twisted", SdfNode::deform(SdfDeform::Twist { rate: 0.05 }, fold(Union, 1.0, vec![c(), sphere(2.0)]))),
            ("material and id", SdfNode::Material { slot: 2, within: None, of: Box::new(SdfNode::Id { id: 9, of: Box::new(fold(Union, 1.0, vec![g(), c()])) }) }),
            ("textured", fold(Union, 1.0, vec![SdfNode::Texture { tex: knurl, of: Box::new(g()) }, c()])),
        ];
        // a coarse lattice over all of them, a finer one through the meshes' surfaces
        let ps: Vec<Vec3d> = points(14.0, 9).into_iter().chain(points(8.0, 11)).collect();
        for (name, n) in &shapes {
            let f = Compiled::new(n);
            let chunks = Chunks::new(Arc::new(n.clone()), ([-15.0; 3], [15.0; 3]), 3.0);
            for (lo, hi) in WINDOWS {
                let want: Vec<u64> = ps.iter().map(|p| distance(n, *p).clamp(lo, hi).to_bits()).collect();
                let (mut stream, mut chunked) = (Hints::default(), Hints::default());
                for (i, p) in ps.iter().enumerate() {
                    for (how, got) in [("streamed", f.distance_bounded(*p, lo, hi, &mut stream)), ("alone", f.distance_bounded(*p, lo, hi, &mut Hints::default())), ("chunked", chunks.distance_bounded(*p, lo, hi, &mut chunked))] {
                        assert_eq!(got.to_bits(), want[i], "{name} {how} at {p:?} under {lo}..={hi}: {got} against {}", f64::from_bits(want[i]));
                    }
                }
                let mut out = vec![0.0; ps.len()];
                f.distance_many_bounded(&ps, lo, hi, &mut out, &mut Hints::default());
                assert!(out.iter().map(|v| v.to_bits()).eq(want.iter().copied()), "{name} many under {lo}..={hi}");
                chunks.distance_many_bounded(&ps, lo, hi, &mut out, &mut Hints::default());
                assert!(out.iter().map(|v| v.to_bits()).eq(want.iter().copied()), "{name} chunked many under {lo}..={hi}");
            }
        }
    }

    /// Tubes read under bounds give their distance clamped, bit for bit.
    #[test]
    fn bounded_tubes_are_their_clamped_distance() {
        let SdfNode::Tubes(t) = cables(4, [-6.0, -6.0, 0.0]) else { unreachable!() };
        for p in points(12.0, 13) {
            for (lo, hi) in WINDOWS {
                assert_eq!(t.distance_bounded(p, lo, hi).to_bits(), t.distance(p).clamp(lo, hi).to_bits(), "{p:?} under {lo}..={hi}");
            }
        }
    }

    /// Under bounds about the surface (half a cell either side of 0, all a volume's cube needs
    /// past its sign), meshes and tubes blended into solids cost a lattice's points a small part
    /// of the leaf work (mesh boxes opened, triangles and tube pieces measured) they cost read in
    /// full: one at a time, many at a time, and through chunks.
    #[test]
    fn a_bounded_read_costs_a_far_point_little() {
        let n = fold(SdfOp::Union, 1.0, vec![cages_and_cables(), placed([0.0, 0.0, 12.0], globe(97, 192, 5.0))]);
        let f = Compiled::new(&n);
        let chunks = Chunks::new(Arc::new(n.clone()), ([-20.0; 3], [20.0; 3]), 4.0);
        let ps = points(19.0, 15);
        let (lo, hi) = (-0.25, 0.25);
        let work = |read: &dyn Fn(&mut [f64])| {
            let mut out = vec![0.0; ps.len()];
            crate::mesh_field::work::take();
            read(&mut out);
            (crate::mesh_field::work::take(), out)
        };
        type Read<'a> = Box<dyn Fn(&mut [f64]) + 'a>;
        let rows: [(&str, Read, Read); 3] = [
            (
                "one at a time",
                Box::new(|o: &mut [f64]| ps.iter().zip(o.iter_mut()).for_each(|(p, o)| *o = f.distance(*p))),
                Box::new(|o: &mut [f64]| {
                    let mut h = Hints::default();
                    ps.iter().zip(o.iter_mut()).for_each(|(p, o)| *o = f.distance_bounded(*p, lo, hi, &mut h));
                }),
            ),
            ("many at a time", Box::new(|o: &mut [f64]| f.distance_many(&ps, o)), Box::new(|o: &mut [f64]| f.distance_many_bounded(&ps, lo, hi, o, &mut Hints::default()))),
            ("chunked", Box::new(|o: &mut [f64]| chunks.distance_many(&ps, o)), Box::new(|o: &mut [f64]| chunks.distance_many_bounded(&ps, lo, hi, o, &mut Hints::default()))),
        ];
        for (what, full, bounded) in &rows {
            let ((wf, vf), (wb, vb)) = (work(full), work(bounded));
            assert!(vf.iter().zip(&vb).all(|(a, b)| a.clamp(lo, hi).to_bits() == b.to_bits()), "{what}");
            assert!(wb * 4 < wf, "{what}: {wb} steps of leaf work under bounds against {wf} in full");
        }
    }
}
