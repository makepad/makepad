//! Field checks of a joint: the smallest gap between the two parts, whether
//! they touch in a pose, the joint's limits by sweeping one part through the
//! other's field, and the per-layer view a slicer gets (in-layer gap, beads
//! printed over the other part).
//!
//! The fields are 1-Lipschitz and never overestimate distance (exact for
//! primitives and meshes, a bound for CSG combinations), so the gap
//! `2 * min_p max(dA(p), dB(p))` found by branch and bound never exceeds the
//! true gap by more than the search tolerance.

use crate::geom::*;
use makepad_csg_sdf::Sdf3;

type Field<'a> = &'a (dyn Sdf3 + Sync);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Gap {
    /// mm; negative: the parts overlap.
    pub gap: f64,
    pub at: P3,
    pub evals: u64,
}

/// What a running check has found so far, step by step: the progress a bar shows and the live
/// view's marks (where it searches, the closest the parts come, the layers a bead could tack
/// them together in, the moving part's turn). Points are in the parts' frame (mm).
#[derive(Clone, Debug, PartialEq)]
pub enum Found {
    /// A few layers sliced first, spread over the joint's box, to time the layer check by (the
    /// check takes them as they are): `sliced` of `of` done (0: about to start), in `secs`, of
    /// the `layers` the box spans.
    Ahead { sliced: usize, of: usize, layers: usize, secs: f64 },
    /// The layer being sliced (ahead, or the next in turn): `share` of its raster done.
    Rows { share: f64 },
    /// The gap search: the share of it done (0 to 1, in its time: the time so far of that and the
    /// time its roots left take at the rate its roots took so far; it never goes back), and the
    /// smallest gap so far (mm; negative: the parts overlap; infinite: none seen yet) and where.
    Gap { searched: f64, gap: f64, at: P3 },
    /// Layer `k` of `n` sliced (1-based), at height `z` along up.
    Layer { k: usize, n: usize, z: f64 },
    /// A layer where one part's beads print over the other's.
    Tack(Tack),
    /// Turn `turn` of `turns` (a hinge's two ways, a ball's four) tests the moving part turned by
    /// `angle` (radians, signed) about `axis` (unit, through the joint's pivot); it goes up to
    /// `max` either way.
    Turn { turn: usize, turns: usize, axis: P3, angle: f64, max: f64 },
    /// That turn stops at `angle` (the last angle free), the parts touching at `contact`; None:
    /// it is free all the way to `max`.
    Stop(Turned),
}

/// Beads of one part printed over the other's with `empty` layers between (0: straight onto
/// it, fused) in the layer at height `z` along up (layer `layer` from the bed): `area` mm²,
/// centred at `at`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tack {
    pub layer: usize,
    pub z: f64,
    pub empty: usize,
    pub area: f64,
    pub at: P3,
}

/// Where a turn of a sweep stopped (see [`Found::Stop`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Turned {
    pub turn: usize,
    pub turns: usize,
    pub axis: P3,
    pub angle: f64,
    pub contact: Option<P3>,
}

/// A check's step callback: told what was found at each step, it answers whether to go on.
pub type Steps<'a> = &'a mut dyn FnMut(Found) -> bool;

/// A step callback that never stops and drops what it is told.
fn on(_: Found) -> bool {
    true
}

/// Gap search cells between two reports.
const GAP_REPORT: u64 = 64;

const SQ3: f64 = 1.7320508075688772;

#[derive(Default)]
struct Query {
    hint: Option<(makepad_csg_math::Vec3d, f64)>,
}

impl Query {
    fn at(&mut self, field: Field, p: P3, lower: f64, upper: f64) -> f64 {
        let p = v3(p);
        let d = field.distance_bounded(p, lower, upper, self.hint);
        // A clamped answer is a bound, not a signed sample. Never reuse its
        // sign. Keep the larger empty ball while sampling inside it.
        if d > lower && d < upper {
            if self.hint.is_none_or(|(q, dq)| len(sub(p3(p), p3(q))) >= dq.abs()) {
                self.hint = Some((p, d));
            }
        }
        d
    }
}

fn roots(lo: P3, hi: P3, s0: f64) -> Vec<(P3, f64)> {
    let n = |i: usize| (((hi[i] - lo[i]) / s0).ceil() as usize).max(1);
    let mut out = Vec::new();
    for i in 0..n(0) {
        for j in 0..n(1) {
            for k in 0..n(2) {
                out.push(([lo[0] + (i as f64 + 0.5) * s0, lo[1] + (j as f64 + 0.5) * s0, lo[2] + (k as f64 + 0.5) * s0], s0 * 0.5));
            }
        }
    }
    out
}

fn split(c: P3, h: f64, stack: &mut Vec<(P3, f64)>) {
    let q = h * 0.5;
    for dx in [-q, q] {
        for dy in [-q, q] {
            for dz in [-q, q] {
                stack.push(([c[0] + dx, c[1] + dy, c[2] + dz], q));
            }
        }
    }
}

/// How far a gap search has got, in its time. With the roots nearest both parts searched first
/// ([`nearest_first`]), the smallest gap is found at once, and what a root takes from then on goes
/// by how much nearer than that gap both parts come to its centre: a root farther than the gap by
/// its half-diagonal is one cell, one nearer is split down to the tolerance around the gap's
/// level, nearest the most. The roots not searched yet are counted by those bands up front (their
/// distances are the order's); each band's time is the mean of its roots searched so far (a band
/// none of whose roots is done yet takes the band nearer's). The share done is the time so far, of
/// that and the time left, and grows with every report by its share of the time left: it never
/// goes back and never sits still.
struct GapWork {
    /// Per root, in the stack's order: the larger of both parts' distances at its centre (as
    /// [`nearest_first`] tells them, up to three half-sizes), farthest first.
    near: Vec<f64>,
    /// The roots' half-size.
    h: f64,
    /// Roots not started yet: the bottom `unstarted` of the stack.
    unstarted: usize,
    /// The root being searched: its band and when it started (s).
    current: Option<(usize, f64)>,
    /// Per band: roots done and their time (s).
    done: [(u32, f64); BANDS],
    started: std::time::Instant,
    /// The share told last and when (s).
    told: (f64, f64),
}

/// [`GapWork`]'s bands, by how much nearer than the gap so far both parts come to a root's centre,
/// in half-sizes: less than half the half-diagonal, less than the half-diagonal, farther (one cell).
const BANDS: usize = 3;

impl GapWork {
    fn new(near: Vec<f64>, h: f64) -> GapWork {
        GapWork { unstarted: near.len(), near, h, current: None, done: [(0, 0.0); BANDS], started: std::time::Instant::now(), told: (0.0, 0.0) }
    }

    /// The band a root `near` its centre falls in with `best` the smallest gap so far (half, as the
    /// search keeps it).
    fn band(&self, near: f64, best: f64) -> usize {
        let x = (near - best) / self.h;
        if x < 0.5 * SQ3 {
            0
        } else if x < SQ3 {
            1
        } else {
            2
        }
    }

    /// A cell came off the stack, `left` cells under it: the roots are at its bottom, so one at
    /// the bottom's top starts a root.
    fn popped(&mut self, left: usize, best: f64) {
        if left < self.unstarted {
            let now = self.started.elapsed().as_secs_f64();
            if let Some((band, since)) = self.current.take() {
                self.done[band].0 += 1;
                self.done[band].1 += now - since;
            }
            self.unstarted = left;
            self.current = Some((self.band(self.near[left], best), now));
        }
    }

    /// The share of the search done, 0 to 1 (see [`GapWork`]).
    fn share(&mut self, best: f64) -> f64 {
        let now = self.started.elapsed().as_secs_f64();
        // a band's mean time, else the nearer band's; none done yet: no time to go by
        let mut mean = [None; BANDS];
        for band in 0..BANDS {
            let (n, t) = self.done[band];
            mean[band] = if n > 0 { Some(t / n as f64) } else if band > 0 { mean[band - 1] } else { None };
        }
        let Some(_) = mean[0].or(mean[1]).or(mean[2]) else { return self.told.0 };
        let mean = |band: usize| mean[band].or(mean.iter().flatten().next().copied()).unwrap_or(0.0);
        // the roots not started, by band: `near` grows down the stack's bottom, so each band's
        // roots are a run of it
        let roots = &self.near[..self.unstarted];
        let edge = |x: f64| roots.partition_point(|n| *n >= best + x * self.h);
        let (far, mid) = (edge(SQ3), edge(0.5 * SQ3));
        let mut left = far as f64 * mean(2) + (mid - far) as f64 * mean(1) + (roots.len() - mid) as f64 * mean(0);
        if let Some((band, since)) = self.current {
            left += (mean(band) - (now - since)).max(0.0);
        }
        let (share, then) = self.told;
        let since = now - then;
        let share = if since + left > 0.0 { share + (1.0 - share) * since / (since + left) } else { share };
        self.told = (share.min(1.0), now);
        self.told.0
    }
}

/// Puts the roots nearest both parts on top of the stack, so they are searched first: the search
/// then finds its smallest gap at once, and cuts every cell that cannot beat it. Searched in the
/// lattice's order, it finds smaller gaps one root at a time, and splits the cells around each of
/// those down to `tol` before the next. A root's nearness is the larger of both parts' distances at
/// its centre, told up to three half-sizes (farther ones keep the lattice's order); those are
/// returned in the stack's new order.
fn nearest_first(a: Field, b: Field, roots: &mut Vec<(P3, f64)>) -> Vec<f64> {
    let reach = 3.0 * roots.first().map_or(0.0, |r| r.1);
    let (mut qa, mut qb) = (Query::default(), Query::default());
    let mut keyed: Vec<(f64, (P3, f64))> = roots
        .iter()
        .map(|&(c, h)| {
            let da = qa.at(a, c, f64::NEG_INFINITY, reach);
            (if da >= reach { reach } else { da.max(qb.at(b, c, f64::NEG_INFINITY, reach)) }, (c, h))
        })
        .collect();
    // the stack pops from its end: the nearest last
    keyed.sort_by(|x, y| y.0.total_cmp(&x.0));
    *roots = keyed.iter().map(|k| k.1).collect();
    keyed.into_iter().map(|k| k.0).collect()
}

/// The smallest gap between `a` and `b` inside the box `lo..hi`, found to within `tol` (the result may
/// overstate the true gap by up to 1.8 tol, never more).
pub fn min_gap(a: Field, b: Field, lo: P3, hi: P3, tol: f64) -> Gap {
    min_gap_steps(a, b, lo, hi, tol, &mut on).expect("a search that never stops ends")
}

/// [`min_gap`], telling `step` how far it has searched ([`Found::Gap`]) every few cells and at
/// the end; None when `step` stops it.
pub fn min_gap_steps(a: Field, b: Field, lo: P3, hi: P3, tol: f64, step: Steps) -> Option<Gap> {
    let mut best = f64::INFINITY;
    let mut at = lo;
    let mut evals = 0u64;
    let (mut qa, mut qb) = (Query::default(), Query::default());
    let s0 = (hi[0] - lo[0]).max(hi[1] - lo[1]).max(hi[2] - lo[2]).min(1.0).max(tol * 4.0);
    let mut stack = roots(lo, hi, s0);
    let near = nearest_first(a, b, &mut stack);
    let mut work = GapWork::new(near, stack.first().map_or(1.0, |r| r.1));
    let mut cells = 0u64;
    while let Some((c, h)) = stack.pop() {
        work.popped(stack.len(), best);
        cells += 1;
        if cells % GAP_REPORT == 0 && !step(Found::Gap { searched: work.share(best), gap: 2.0 * best, at }) {
            return None;
        }
        let cutoff = (best + h * SQ3 - tol * 0.25).next_up();
        let da = qa.at(a, c, f64::NEG_INFINITY, cutoff);
        evals += 1;
        if da == cutoff {
            continue;
        }
        let db = qb.at(b, c, da, cutoff);
        if db == cutoff {
            continue;
        }
        let g = da.max(db);
        if g < best {
            best = g;
            at = c;
        }
        if h * 2.0 > tol && g - h * SQ3 < best - tol * 0.25 {
            split(c, h, &mut stack);
        }
    }
    if !step(Found::Gap { searched: 1.0, gap: 2.0 * best, at }) {
        return None;
    }
    Some(Gap { gap: 2.0 * best, at, evals })
}

/// Whether `a` and `b` come closer than `gap` anywhere in the box (early out on the first hit).
pub fn closer_than(a: Field, b: Field, lo: P3, hi: P3, gap: f64) -> bool {
    let target = 0.5 * gap;
    let s0 = (hi[0] - lo[0]).max(hi[1] - lo[1]).max(hi[2] - lo[2]).min(2.0);
    let mut stack = roots(lo, hi, s0);
    let (mut qa, mut qb) = (Query::default(), Query::default());
    while let Some((c, h)) = stack.pop() {
        let cutoff = (target + h * SQ3).next_up();
        let lower = target.next_down();
        let da = qa.at(a, c, lower, cutoff);
        if da == cutoff { continue; }
        let db = qb.at(b, c, lower, cutoff);
        if db == cutoff { continue; }
        let g = da.max(db);
        if g < target {
            return true;
        }
        if g - h * SQ3 < target && h > 0.01 {
            split(c, h, &mut stack);
        }
    }
    false
}

/// `f` turned by `ang` about the unit `axis` through `pivot`.
pub fn posed<'a>(f: Field<'a>, pivot: P3, axis: P3, ang: f64) -> impl Sdf3 + Sync + 'a {
    struct Posed<'a> {
        field: Field<'a>,
        pivot: P3,
        axis: P3,
        ang: f64,
    }
    impl Posed<'_> {
        fn local(&self, p: makepad_csg_math::Vec3d) -> makepad_csg_math::Vec3d {
            let p = sub(p3(p), self.pivot);
            v3(add(self.pivot, rotate(p, self.axis, -self.ang)))
        }
    }
    impl Sdf3 for Posed<'_> {
        fn distance(&self, p: makepad_csg_math::Vec3d) -> f64 {
            self.field.distance(self.local(p))
        }
        fn distance_bounded(&self, p: makepad_csg_math::Vec3d, lower: f64, upper: f64, hint: Option<(makepad_csg_math::Vec3d, f64)>) -> f64 {
            self.field.distance_bounded(self.local(p), lower, upper, hint.map(|(q, d)| (self.local(q), d)))
        }
    }
    Posed { field: f, pivot, axis, ang }
}

#[derive(Clone, Copy)]
struct SweepCell {
    center: P3,
    half: f64,
    radius: f64,
    a: Option<f64>,
    // Angle and bounds on B's distance at this cell's centre.
    b: Option<(f64, f64, f64)>,
    children: Option<usize>,
}

struct SweepSearch<'a> {
    a: Field<'a>,
    b: Field<'a>,
    pivot: P3,
    axis: P3,
    dir: f64,
    cells: Vec<SweepCell>,
    roots: usize,
    qa: Query,
    radius: f64,
}

impl<'a> SweepSearch<'a> {
    fn new(a: Field<'a>, b: Field<'a>, pivot: P3, axis: P3, dir: f64, lo: P3, hi: P3) -> Self {
        let s0 = (hi[0] - lo[0]).max(hi[1] - lo[1]).max(hi[2] - lo[2]).min(2.0);
        let mut search = Self { a, b, pivot, axis, dir, cells: Vec::new(), roots: 0, qa: Query::default(), radius: 0.0 };
        for (center, half) in roots(lo, hi, s0) {
            search.add_cell(center, half);
            // Include the entire root cubes, including the existing grid's
            // overhang at hi, in the angular displacement certificate.
            for dx in [-half, half] {
                for dy in [-half, half] {
                    for dz in [-half, half] {
                        search.radius = search.radius.max(len(cross(sub(add(center, [dx, dy, dz]), pivot), axis)) * dir.abs());
                    }
                }
            }
        }
        search.roots = search.cells.len();
        search
    }

    fn add_cell(&mut self, center: P3, half: f64) {
        self.cells.push(SweepCell {
            center, half, radius: len(cross(sub(center, self.pivot), self.axis)) * self.dir.abs(),
            a: None, b: None, children: None,
        });
    }

    // Ok: no sampled hit, with a lower bound on min(max(dA,dB)) over
    // every cell. Err: an upper bound at a witness below target, and the
    // witness (the cell's centre: where the parts touch).
    fn clearance(&mut self, angle: f64, target: f64) -> Result<f64, (f64, P3)> {
        let posed = posed(self.b, self.pivot, self.axis, angle * self.dir);
        let mut qb = Query::default();
        let mut stack: Vec<usize> = (0..self.roots).collect();
        let mut lower = f64::INFINITY;
        while let Some(i) = stack.pop() {
            let cell = self.cells[i];
            let radius = cell.half * SQ3;
            let da = match cell.a {
                Some(d) => d,
                None => {
                    let d = self.qa.at(self.a, cell.center, f64::NEG_INFINITY, f64::INFINITY);
                    self.cells[i].a = Some(d);
                    d
                }
            };
            if da - radius >= target {
                lower = lower.min(da - radius);
                continue;
            }
            let (mut bl, mut bu) = cell.b.map_or((f64::NEG_INFINITY, f64::INFINITY), |(at, l, u)| {
                // A point at radius r moves at most r * delta_angle. Reuse
                // both exact samples and clamped bounds across poses.
                let motion = cell.radius * (angle - at).abs();
                (l - motion, u + motion)
            });
            if da.max(bl) - radius < target && da.max(bu) >= target {
                let floor = target.next_down();
                let ceiling = (target + radius).next_up();
                let d = qb.at(&posed, cell.center, floor, ceiling);
                bl = bl.max(if d == floor { f64::NEG_INFINITY } else { d });
                bu = bu.min(if d == ceiling { f64::INFINITY } else { d });
                self.cells[i].b = Some((angle, bl, bu));
            }
            if da.max(bu) < target {
                return Err((da.max(bu), cell.center));
            }
            let bound = da.max(bl) - radius;
            if bound >= target || cell.half <= 0.01 {
                lower = lower.min(bound);
                continue;
            }
            let children = match cell.children {
                Some(start) => start,
                None => {
                    let start = self.cells.len();
                    let q = cell.half * 0.5;
                    for dx in [-q, q] {
                        for dy in [-q, q] {
                            for dz in [-q, q] {
                                self.add_cell(add(cell.center, [dx, dy, dz]), q);
                            }
                        }
                    }
                    self.cells[i].children = Some(start);
                    start
                }
            };
            stack.extend(children..children + 8);
        }
        Ok(lower)
    }
}

/// Sweep the original three-degree sampling lattice, refining the first hit
/// with seven bisections. Skip lattice sites only when the clearance bound
/// certifies the entire angular interval as contact-free.
pub fn sweep_limit(a: Field, b: Field, pivot: P3, axis: P3, dir: f64, lo: P3, hi: P3, contact: f64, max: f64) -> f64 {
    sweep_steps(a, b, pivot, axis, dir, lo, hi, contact, max, (0, 1), &mut on).expect("a sweep that never stops ends").angle.abs()
}

/// [`sweep_limit`] as turn `turn.0` of `turn.1`, telling `step` each angle it tests
/// ([`Found::Turn`]) and where it stops ([`Found::Stop`]: the angle, signed, and where the parts
/// touch there); None when `step` stops it.
#[allow(clippy::too_many_arguments)]
pub fn sweep_steps(a: Field, b: Field, pivot: P3, axis: P3, dir: f64, lo: P3, hi: P3, contact: f64, max: f64, turn: (usize, usize), step: Steps) -> Option<Turned> {
    let (n, turns) = turn;
    let sweep = 3f64.to_radians();
    let target = contact * 0.5;
    let mut search = SweepSearch::new(a, b, pivot, axis, dir, lo, hi);
    let test = |search: &mut SweepSearch, ang: f64, step: &mut dyn FnMut(Found) -> bool| {
        if step(Found::Turn { turn: n, turns, axis, angle: dir * ang, max }) { Some(search.clearance(ang, target)) } else { None }
    };
    let mut prev = 0.0;
    let mut ang = sweep;
    while ang <= max + 1e-9 {
        match test(&mut search, ang, step)? {
            Err((_, at)) => {
                let (mut l, mut h, mut touch) = (prev, ang, at);
                for _ in 0..7 {
                    let m = 0.5 * (l + h);
                    match test(&mut search, m, step)? {
                        Err((_, at)) => {
                            h = m;
                            touch = at;
                        }
                        Ok(_) => l = m,
                    }
                }
                let stop = Turned { turn: n, turns, axis, angle: dir * l, contact: Some(touch) };
                return step(Found::Stop(stop)).then_some(stop);
            }
            Ok(lower) => {
                let safe = if search.radius > 0.0 { (lower - target) / search.radius } else { 0.0 };
                let until = ang + safe;
                prev = ang;
                ang += sweep;
                // Keep the original additions and last three-degree bracket.
                // This loop visits no fields, and never steps past max.
                while ang + sweep < until && ang + sweep <= max + 1e-9 {
                    prev = ang;
                    ang += sweep;
                }
            }
        }
    }
    let stop = Turned { turn: n, turns, axis, angle: dir * max, contact: None };
    step(Found::Stop(stop)).then_some(stop)
}

/// The slicer's view of a joint region.
#[derive(Clone, Debug, PartialEq)]
pub struct LayerReport {
    /// Smallest in-layer gap between the two parts' sections (raster estimate, +- res).
    pub layer_gap: f64,
    /// Height (along up, from the region's bottom) of the layer with that gap.
    pub layer_gap_at: f64,
    /// mm^2 of each part printed without its own support with the other part n empty layers below: [n = 0, 1, 2, 3].
    pub tack: [f64; 4],
    /// Layers looked at.
    pub layers: usize,
    /// Where `tack` is: each layer's beads printed over the other part, by layer and by how many
    /// empty layers lie between, bottom up.
    pub tacks: Vec<Tack>,
}

/// Rasterise both parts per layer inside the box, in layers of `layer` along `up`, at `res` in plane.
pub fn layer_check(a: Field, b: Field, lo: P3, hi: P3, up: P3, layer: f64, res: f64, bed: f64) -> LayerReport {
    layer_check_steps(a, b, lo, hi, up, layer, res, bed, &mut on).expect("a check that never stops ends")
}

/// [`layer_check`], telling `step` each layer it slices ([`Found::Layer`]) and each tack it
/// finds ([`Found::Tack`]); None when `step` stops it.
#[allow(clippy::too_many_arguments)]
pub fn layer_check_steps(a: Field, b: Field, lo: P3, hi: P3, up: P3, layer: f64, res: f64, bed: f64, step: Steps) -> Option<LayerReport> {
    Slicer::new(a, b, lo, hi, up, layer, res, bed).check(Ahead::default(), step)
}

/// The layers a box around a joint prints in, each sliced into a raster of both parts' sections:
/// what [`layer_check`] looks at.
pub struct Slicer<'a> {
    a: Field<'a>,
    b: Field<'a>,
    up: P3,
    u: P3,
    v: P3,
    u0: f64,
    v0: f64,
    nx: usize,
    ny: usize,
    /// The box's first layer on the bed's grid, and how many it spans.
    k0: i64,
    n: usize,
    layer: f64,
    res: f64,
    bed: f64,
}

/// A few layers a [`Slicer`] sliced ahead, spread over its box, to time the rest by: by layer (from
/// the box's first), and the seconds they took.
#[derive(Default)]
pub struct Ahead {
    rows: Vec<(usize, Vec<u8>)>,
    pub secs: f64,
}

impl Ahead {
    /// Layers sliced.
    pub fn sliced(&self) -> usize {
        self.rows.len()
    }
}

impl<'a> Slicer<'a> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(a: Field<'a>, b: Field<'a>, lo: P3, hi: P3, up: P3, layer: f64, res: f64, bed: f64) -> Slicer<'a> {
        let up = norm(up);
        let (u, v) = basis(up);
        let corners: Vec<P3> = (0..8).map(|i| [if i & 1 == 0 { lo[0] } else { hi[0] }, if i & 2 == 0 { lo[1] } else { hi[1] }, if i & 4 == 0 { lo[2] } else { hi[2] }]).collect();
        let range = |d: P3| corners.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(l, h), c| (l.min(dot(*c, d)), h.max(dot(*c, d))));
        let (u0, u1) = range(u);
        let (v0, v1) = range(v);
        let (z0, z1) = range(up);
        let nx = ((u1 - u0) / res).ceil().max(1.0) as usize;
        let ny = ((v1 - v0) / res).ceil().max(1.0) as usize;
        // layers on the bed's grid: layer k spans bed + k*layer .. bed + (k+1)*layer along up
        let k0 = ((z0 - bed) / layer).floor().max(0.0) as i64;
        let k1 = ((z1 - bed) / layer).ceil() as i64;
        Slicer { a, b, up, u, v, u0, v0, nx, ny, k0, n: (k1.max(k0) - k0) as usize, layer, res, bed }
    }

    /// The layers the box spans.
    pub fn layers(&self) -> usize {
        self.n
    }

    /// The middle of layer `kk` of the box along up.
    fn z(&self, kk: usize) -> f64 {
        self.bed + ((self.k0 + kk as i64) as f64 + 0.5) * self.layer
    }

    /// Layer `kk` of the box sliced: each raster cell 1 inside part a, 2 inside b, else 0; told to
    /// `step` row by row of blocks ([`Found::Rows`]), None when it stops it.
    fn row(&self, kk: usize, step: Steps) -> Option<Vec<u8>> {
        let (a, b, res, nx, ny) = (self.a, self.b, self.res, self.nx, self.ny);
        let z = self.z(kk);
        let mut row = vec![0u8; nx * ny];
        let at = |i: f64, j: f64| v3(add(add(scale(self.u, self.u0 + i * res), scale(self.v, self.v0 + j * res)), scale(self.up, z)));
        let class = |q| if a.distance(q) < 0.0 { 1 } else if b.distance(q) < 0.0 { 2 } else { 0 };
        // Blocks of 8 x 8 cells: one sample at the block centre classifies the whole block when both fields are
        // farther than the block's half diagonal from their surfaces (1-Lipschitz), else every cell is sampled.
        const B: usize = 8;
        let half = res * B as f64 * 0.5 * std::f64::consts::SQRT_2;
        for bj in (0..ny).step_by(B) {
            for bi in (0..nx).step_by(B) {
                let (ei, ej) = ((bi + B).min(nx), (bj + B).min(ny));
                let q = at((bi + ei) as f64 * 0.5, (bj + ej) as f64 * 0.5);
                let (da, db) = (a.distance(q), b.distance(q));
                if da.abs() > half && db.abs() > half {
                    let c = if da < 0.0 { 1 } else if db < 0.0 { 2 } else { 0 };
                    for j in bj..ej {
                        for i in bi..ei {
                            row[j * nx + i] = c;
                        }
                    }
                    continue;
                }
                for j in bj..ej {
                    for i in bi..ei {
                        row[j * nx + i] = class(at(i as f64 + 0.5, j as f64 + 0.5));
                    }
                }
            }
            if bj + B < ny && !step(Found::Rows { share: (bj + B) as f64 / ny as f64 }) {
                return None;
            }
        }
        Some(row)
    }

    /// `count` layers sliced ahead, spread over the box (the middles of its `count` equal
    /// spans), told to `step` before the first and as each is done ([`Found::Ahead`]);
    /// [`Slicer::check`] slices them no more. None when `step` stops it.
    pub fn ahead(&self, count: usize, step: Steps) -> Option<Ahead> {
        let count = count.min(self.n);
        let started = std::time::Instant::now();
        let mut ahead = Ahead::default();
        if count > 0 && !step(Found::Ahead { sliced: 0, of: count, layers: self.n, secs: 0.0 }) {
            return None;
        }
        for i in 0..count {
            let kk = (2 * i + 1) * self.n / (2 * count);
            let row = self.row(kk, step)?;
            ahead.rows.push((kk, row));
            ahead.secs = started.elapsed().as_secs_f64();
            if !step(Found::Ahead { sliced: i + 1, of: count, layers: self.n, secs: ahead.secs }) {
                return None;
            }
        }
        Some(ahead)
    }

    /// The layer check (see [`layer_check_steps`]), with the layers sliced `ahead` taken as
    /// they are.
    pub fn check(&self, ahead: Ahead, step: Steps) -> Option<LayerReport> {
        let (nx, ny, res, layer, bed, k0, n) = (self.nx, self.ny, self.res, self.layer, self.bed, self.k0, self.n);
        let (u, v, up, u0, v0) = (self.u, self.v, self.up, self.u0, self.v0);
        let mut ahead = ahead.rows;
        let mut occ: Vec<Vec<u8>> = Vec::new();
        for kk in 0..n {
            let row = match ahead.iter().position(|(k, _)| *k == kk) {
                Some(i) => ahead.swap_remove(i).1,
                None => self.row(kk, step)?,
            };
            occ.push(row);
            if !step(Found::Layer { k: occ.len(), n, z: self.z(kk) }) {
                return None;
            }
        }
        let mut layer_gap = f64::INFINITY;
        let mut layer_gap_at = 0.0;
        let reach = (1.0 / res).ceil() as isize;
        for (kk, row) in occ.iter().enumerate() {
            let edge = |m: u8| -> Vec<(isize, isize)> {
                let mut out = Vec::new();
                for j in 0..ny {
                    for i in 0..nx {
                        if row[j * nx + i] != m {
                            continue;
                        }
                        let on_edge = [(1isize, 0isize), (-1, 0), (0, 1), (0, -1)].iter().any(|(di, dj)| {
                            let (ii, jj) = (i as isize + di, j as isize + dj);
                            ii < 0 || jj < 0 || ii >= nx as isize || jj >= ny as isize || row[jj as usize * nx + ii as usize] != m
                        });
                        if on_edge {
                            out.push((i as isize, j as isize));
                        }
                    }
                }
                out
            };
            let ea = edge(1);
            let eb = edge(2);
            if ea.is_empty() || eb.is_empty() {
                continue;
            }
            let mut buckets: std::collections::HashMap<(isize, isize), Vec<(isize, isize)>> = Default::default();
            for c in &eb {
                buckets.entry((c.0 / reach, c.1 / reach)).or_default().push(*c);
            }
            for &(i, j) in &ea {
                for di in -1..=1 {
                    for dj in -1..=1 {
                        if let Some(l) = buckets.get(&(i / reach + di, j / reach + dj)) {
                            for &(bi, bj) in l {
                                let d = ((((bi - i) * (bi - i) + (bj - j) * (bj - j)) as f64).sqrt() - 1.0) * res;
                                if d < layer_gap {
                                    layer_gap = d;
                                    layer_gap_at = (k0 + kk as i64) as f64 * layer;
                                }
                            }
                        }
                    }
                }
            }
        }
        let mut tack = [0.0f64; 4];
        let mut tacks = Vec::new();
        let lat = (layer / res).round().max(1.0) as isize;
        for kk in 1..occ.len() {
            // this layer's tacked cells by empty layers below: their area and the sum of their places
            let mut here = [(0.0f64, [0.0f64; 3]); 4];
            for j in 0..ny {
                for i in 0..nx {
                    let m = occ[kk][j * nx + i];
                    if m == 0 {
                        continue;
                    }
                    let mut supported = false;
                    'n: for dj in -lat..=lat {
                        for di in -lat..=lat {
                            let (ii, jj) = (i as isize + di, j as isize + dj);
                            if ii >= 0 && jj >= 0 && (ii as usize) < nx && (jj as usize) < ny && occ[kk - 1][jj as usize * nx + ii as usize] == m {
                                supported = true;
                                break 'n;
                            }
                        }
                    }
                    if supported {
                        continue;
                    }
                    let mut empty = 0usize;
                    let mut k = kk as isize - 1;
                    while k >= 0 && occ[k as usize][j * nx + i] == 0 && empty < 4 {
                        empty += 1;
                        k -= 1;
                    }
                    if k >= 0 && empty < 4 {
                        let below = occ[k as usize][j * nx + i];
                        if below != 0 && below != m {
                            tack[empty] += res * res;
                            let z = bed + ((k0 + kk as i64) as f64 + 0.5) * layer;
                            let p = add(add(scale(u, u0 + (i as f64 + 0.5) * res), scale(v, v0 + (j as f64 + 0.5) * res)), scale(up, z));
                            here[empty].0 += res * res;
                            here[empty].1 = add(here[empty].1, p);
                        }
                    }
                }
            }
            for (empty, (area, sum)) in here.into_iter().enumerate().filter(|(_, (a, _))| *a > 0.0) {
                let cells = area / (res * res);
                let t = Tack { layer: (k0 + kk as i64) as usize, z: bed + ((k0 + kk as i64) as f64 + 0.5) * layer, empty, area, at: scale(sum, 1.0 / cells) };
                if !step(Found::Tack(t)) {
                    return None;
                }
                tacks.push(t);
            }
        }
        Some(LayerReport { layer_gap, layer_gap_at, tack, layers: occ.len(), tacks })
    }
}

/// How likely a joint is to print fused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Risk {
    Low,
    Medium,
    High,
    Fused,
}

impl Risk {
    pub fn name(self) -> &'static str {
        match self {
            Risk::Low => "low",
            Risk::Medium => "medium",
            Risk::High => "high",
            Risk::Fused => "fused",
        }
    }
}

/// Provisional weights (to be calibrated with a printed coupon): a part printed over the other with no empty
/// layer between is fused, over one empty layer it tacks hard, over two lightly, over three rarely.
pub fn risk(gap: f64, layer_gap: f64, tack: &[f64; 4], c_xy: f64) -> (Risk, f64) {
    let score = tack[0] * 3.0 + tack[1] + tack[2] * 0.3 + tack[3] * 0.05;
    let r = if gap <= 0.0 || tack[0] > 0.5 || layer_gap < 0.4 * c_xy {
        Risk::Fused
    } else if layer_gap < 0.7 * c_xy || score > 40.0 {
        Risk::High
    } else if score > 10.0 {
        Risk::Medium
    } else {
        Risk::Low
    };
    (r, score)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Prim;

    struct P(Prim);
    impl Sdf3 for P {
        fn distance(&self, p: makepad_csg_math::Vec3d) -> f64 {
            self.0.distance(p3(p))
        }
    }

    #[test]
    fn gap_between_spheres() {
        let a = P(Prim::Sphere { c: [0.0, 0.0, 0.0], r: 1.0 });
        let b = P(Prim::Sphere { c: [2.35, 0.0, 0.0], r: 1.0 });
        let g = min_gap(&a, &b, [-1.5, -1.5, -1.5], [4.0, 1.5, 1.5], 0.005);
        assert!((g.gap - 0.35).abs() < 0.01, "{g:?}");
        assert!(closer_than(&a, &b, [-1.5, -1.5, -1.5], [4.0, 1.5, 1.5], 0.4));
        assert!(!closer_than(&a, &b, [-1.5, -1.5, -1.5], [4.0, 1.5, 1.5], 0.3));
    }

    #[test]
    fn sweep_hits_a_wall() {
        // a bar along +x from the origin turning about z hits a block standing at y in 3..5 beyond x 2
        let bar = P(Prim::Frustum { a: [0.5, 0.0, 0.0], b: [4.0, 0.0, 0.0], ra: 0.3, rb: 0.3 });
        let block = P(Prim::Frustum { a: [2.5, 3.0, -1.0], b: [2.5, 3.0, 1.0], ra: 0.5, rb: 0.5 });
        let lim = sweep_limit(&block, &bar, [0.0; 3], [0.0, 0.0, 1.0], 1.0, [-5.0, -5.0, -2.0], [5.0, 5.0, 2.0], 0.02, std::f64::consts::PI);
        let expect = (3.0f64).atan2(2.5) - (0.8f64 / (2.5f64 * 2.5 + 9.0).sqrt()).asin();
        assert!((lim - expect).abs() < 2f64.to_radians(), "{} vs {}", lim.to_degrees(), expect.to_degrees());
    }

    #[test]
    fn sweep_has_a_known_contact_angle() {
        let a = P(Prim::Sphere { c: [0.0, 4.0, 0.0], r: 0.5 });
        let b = P(Prim::Sphere { c: [4.0, 0.0, 0.0], r: 0.5 });
        // Centres on a radius-4 circle: their chord is 8*sin(delta/2).
        // Contact occurs at chord 0.5 + 0.5 + 0.05.
        let expected = std::f64::consts::FRAC_PI_2 - 2.0 * (1.05f64 / 8.0).asin();
        let (lo, hi) = ([-1.0; 3], [5.0, 5.0, 1.0]);
        let limit = sweep_limit(&a, &b, [0.0; 3], [0.0, 0.0, 1.0], 1.0, lo, hi, 0.05, std::f64::consts::PI);
        println!("known sweep: {limit:.12}, analytic {expected:.12}");
        assert!((limit - expected).abs() <= 3f64.to_radians() / 128.0, "{limit} vs {expected}");
        assert_eq!(sweep_limit(&a, &b, [0.0; 3], [0.0, 0.0, 1.0], -1.0, lo, hi, 0.05, 0.4), 0.4, "rotation away from the obstacle is free");
        assert_eq!(sweep_limit(&a, &a, [0.0; 3], [0.0, 0.0, 1.0], 1.0, lo, hi, 0.05, 1.0), 0.0, "initial overlap has no free sweep");
    }
}
