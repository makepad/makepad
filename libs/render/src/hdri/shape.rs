//! The shared shape core for shaped lights (phase 1b).
//!
//! Outlines are contours: point lists in a light's frame. The core answers
//! two questions about a point:
//! - how far it is from a filled outline, signed (negative inside), with the
//!   nonzero winding rule, so concave and self-crossing outlines fill as
//!   drawn and a reversed inner contour cuts a hole (`FillShape`);
//! - how far it is from a stroke, and how far along the stroke the nearest
//!   point lies: `t` in 0..1 by arc length (`Polyline2`, `ArcPath`).
//!
//! Two geometries share the ideas. Planar shapes (`Polyline2`, `FillShape`)
//! live in a light's gnomonic tangent plane, in tangent-plane units, and a
//! gnomonic frame only works for a shape under about 120° across. Strokes on
//! the sphere (`ArcPath`) are chains of great-circle arcs between unit
//! directions, with distances in radians, so a stroke may wrap round the sky.
//!
//! The filled distance follows the idea of iq's `sdPolygon` (the distance to
//! the nearest edge, signed by a crossing test over the edges), reimplemented
//! here with a winding number in place of its parity bit.
//!
//! Speed: a light tests its `CullCone` first (one dot product), and a shape
//! with more than `GRID_MIN_SEGMENTS` segments keeps them in uniform bins, so
//! a lookup visits only the segments near the point. Every distance comes
//! back capped at the shape's `reach`, the distance past which a light's
//! edge band has ended, so nothing farther can change a pixel. That cap is
//! what lets the bins skip far segments and still agree with brute force bit
//! for bit.
//!
//! Splines and SVG paths are flattened through `makepad-tween`
//! (`MotionPath::through`, `MotionPath::from_svg`, `MotionPath::flatten`).

use makepad_draw::*;
use makepad_tween::path::MotionPath;

/// Outlines as point lists: closed contours for a fill, polylines for a
/// stroke. For a light they sit in its unit box or its tangent plane; the
/// core itself only assumes a plane.
pub type Contours = Vec<Vec<[f32; 2]>>;

/// A shape with more segments than this bins them. Below it a plain loop over
/// the segments is cheaper than the bin lookup.
pub const GRID_MIN_SEGMENTS: usize = 64;

/// Great-circle arcs longer than this (5°, in radians) are cut into equal
/// pieces, so each arc's bounding box stays tight for the bins.
const MAX_ARC: f32 = 0.087_266_46;

/// Arcs shorter than this (radians) are dropped: the plane of an arc that
/// short is numerical noise.
const MIN_ARC: f32 = 1.0e-5;

/// The nearest point of a stroke.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StrokeHit {
    /// The distance to it, capped at the shape's reach.
    pub dist: f32,
    /// The arc-length fraction along the whole stroke there, 0..1; 0 when
    /// nothing is within reach.
    pub t: f32,
}

// ---- the per-light cull --------------------------------------------------------

/// A light's angular bound: every direction the light can touch lies within
/// the cone. One dot product rejects the rest of the sphere.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CullCone {
    pub axis: Vec3f,
    /// cos of the cone's half angle; below -1 lets every direction through.
    pub cos_max: f32,
}

impl CullCone {
    /// Lets every direction through.
    pub const ALL: CullCone = CullCone { axis: Vec3f { x: 0.0, y: 1.0, z: 0.0 }, cos_max: -2.0 };

    /// Every direction within `radius` radians of `axis`.
    pub fn new(axis: Vec3f, radius: f32) -> CullCone {
        // Written negated so a NaN radius lets everything through.
        if !(radius < std::f32::consts::PI) || axis.length_squared() < 1.0e-12 {
            return CullCone::ALL;
        }
        // A hair of slack: the cone must never cut a pixel the shape reaches.
        CullCone { axis: axis.normalize(), cos_max: (radius.max(0.0) + 1.0e-4).cos() }
    }

    /// The cone round the mean of `dirs` that holds all of them, widened by
    /// `margin` radians. Directions spread all round the sphere have no
    /// useful mean, and give `ALL`.
    pub fn around(dirs: &[Vec3f], margin: f32) -> CullCone {
        let mut sum = Vec3f::default();
        for d in dirs {
            sum += *d;
        }
        if dirs.is_empty() || sum.length() < 1.0e-3 * dirs.len() as f32 {
            return CullCone::ALL;
        }
        let axis = sum.normalize();
        let widest = dirs.iter().fold(0.0f32, |w, d| w.max(angle_between(axis, *d)));
        CullCone::new(axis, widest + margin)
    }

    #[inline]
    pub fn contains(&self, dir: Vec3f) -> bool {
        dir.dot(self.axis) >= self.cos_max
    }
}

/// The angle between two unit vectors, as atan2 of its sine and cosine:
/// accurate for tiny angles, where acos of the dot product is not.
#[inline]
fn angle_between(a: Vec3f, b: Vec3f) -> f32 {
    Vec3f::cross(a, b).length().atan2(a.dot(b))
}

/// A usable reach: finite and positive, since the bins pad by it.
fn sanitize_reach(reach: f32) -> f32 {
    if reach.is_finite() && reach > 1.0e-6 {
        reach
    } else {
        1.0e-6
    }
}

// ---- uniform bins --------------------------------------------------------------

/// Uniform bins over a D-dimensional box. Each item (a segment's bounding
/// box, padded by the reach) is listed in every cell its box touches. The
/// cells are one flat list: `start[c]..start[c + 1]` indexes `items`, and
/// the items of a cell come in increasing index order.
#[derive(Clone, Debug)]
struct BinGrid<const D: usize> {
    lo: [f32; D],
    scale: [f32; D],
    n: [usize; D],
    start: Vec<u32>,
    items: Vec<u32>,
}

impl<const D: usize> BinGrid<D> {
    /// Bins `boxes` (min and max corners) over their union, `n` cells per axis.
    fn new(boxes: &[([f32; D], [f32; D])], n: [usize; D]) -> BinGrid<D> {
        let mut lo = [f32::MAX; D];
        let mut hi = [f32::MIN; D];
        for (a, b) in boxes {
            for k in 0..D {
                lo[k] = lo[k].min(a[k]);
                hi[k] = hi[k].max(b[k]);
            }
        }
        let mut scale = [0.0f32; D];
        for k in 0..D {
            // A flat extent (every box in one plane) still needs a finite scale.
            scale[k] = n[k] as f32 / (hi[k] - lo[k]).max(1.0e-9);
        }
        let cells: usize = n.iter().product();
        let mut grid = BinGrid { lo, scale, n, start: vec![0u32; cells + 1], items: Vec::new() };
        // Count per cell, turn the counts into offsets, then fill: one flat
        // list instead of a Vec per cell.
        for (a, b) in boxes {
            let (i0, i1) = grid.span(a, b);
            for_cells(n, i0, i1, |c| grid.start[c + 1] += 1);
        }
        for c in 0..cells {
            grid.start[c + 1] += grid.start[c];
        }
        let mut fill: Vec<u32> = grid.start[..cells].to_vec();
        let mut items = vec![0u32; grid.start[cells] as usize];
        for (index, (a, b)) in boxes.iter().enumerate() {
            let (i0, i1) = grid.span(a, b);
            for_cells(n, i0, i1, |c| {
                items[fill[c] as usize] = index as u32;
                fill[c] += 1;
            });
        }
        grid.items = items;
        grid
    }

    /// The cell index along axis `k`, held inside the grid.
    fn axis_cell(&self, k: usize, x: f32) -> usize {
        let i = ((x - self.lo[k]) * self.scale[k]).floor();
        if i <= 0.0 {
            0
        } else {
            (i as usize).min(self.n[k] - 1)
        }
    }

    /// The cells a box touches, per axis, inclusive.
    fn span(&self, a: &[f32; D], b: &[f32; D]) -> ([usize; D], [usize; D]) {
        let mut i0 = [0usize; D];
        let mut i1 = [0usize; D];
        for k in 0..D {
            i0[k] = self.axis_cell(k, a[k]);
            i1[k] = self.axis_cell(k, b[k]).max(i0[k]);
        }
        (i0, i1)
    }

    /// The cell holding `p`, or None outside the grid (then no item's padded
    /// box holds `p`: every segment is at least `reach` away).
    fn cell_of(&self, p: [f32; D]) -> Option<usize> {
        let mut lin = 0usize;
        let mut stride = 1usize;
        for k in 0..D {
            let i = ((p[k] - self.lo[k]) * self.scale[k]).floor();
            // Written negated so NaN lands outside.
            if !(i >= 0.0 && i < self.n[k] as f32) {
                return None;
            }
            lin += i as usize * stride;
            stride *= self.n[k];
        }
        Some(lin)
    }

    fn items_at(&self, cell: usize) -> &[u32] {
        &self.items[self.start[cell] as usize..self.start[cell + 1] as usize]
    }
}

/// Calls `f` with the flat index of every cell in the inclusive index box
/// `a..=b` (axis 0 fastest), like an odometer.
fn for_cells<const D: usize>(n: [usize; D], a: [usize; D], b: [usize; D], mut f: impl FnMut(usize)) {
    let mut idx = a;
    loop {
        let mut lin = 0usize;
        let mut stride = 1usize;
        for k in 0..D {
            lin += idx[k] * stride;
            stride *= n[k];
        }
        f(lin);
        let mut k = 0;
        loop {
            if k == D {
                return;
            }
            if idx[k] < b[k] {
                idx[k] += 1;
                break;
            }
            idx[k] = a[k];
            k += 1;
        }
    }
}

// ---- planar strokes and outlines -----------------------------------------------

#[derive(Clone, Copy, Debug)]
struct Seg2 {
    a: Vec2f,
    b: Vec2f,
    /// b - a.
    e: Vec2f,
    inv_len2: f32,
    /// Arc length from the start of the first contour to `a`.
    s0: f32,
    len: f32,
}

/// Planar segments with their cumulative length: the stroke distance and
/// `t`, and for a closed outline the edges a fill tests.
#[derive(Clone, Debug)]
pub struct Polyline2 {
    segs: Vec<Seg2>,
    total: f32,
    reach: f32,
    grid: Option<BinGrid<2>>,
}

impl Polyline2 {
    /// The segments along each contour in order, closed back to its first
    /// point when `closed`. Zero-length and non-finite segments are dropped.
    /// `reach` is the distance past which nothing matters to the caller.
    pub fn new(contours: &[Vec<[f32; 2]>], closed: bool, reach: f32) -> Polyline2 {
        let reach = sanitize_reach(reach);
        let mut segs = Vec::new();
        let mut s = 0.0f32;
        for c in contours {
            let n = c.len();
            if n < 2 {
                continue;
            }
            let count = if closed { n } else { n - 1 };
            for i in 0..count {
                let j = (i + 1) % n;
                let a = vec2f(c[i][0], c[i][1]);
                let b = vec2f(c[j][0], c[j][1]);
                let e = b - a;
                let len2 = e.x * e.x + e.y * e.y;
                // Written so NaN is dropped too.
                if !(len2 > 1.0e-20) || !len2.is_finite() {
                    continue;
                }
                let len = len2.sqrt();
                segs.push(Seg2 { a, b, e, inv_len2: 1.0 / len2, s0: s, len });
                s += len;
            }
        }
        let grid = if segs.len() > GRID_MIN_SEGMENTS {
            let side = ((segs.len() as f32).sqrt().ceil() as usize).clamp(4, 64);
            let boxes: Vec<([f32; 2], [f32; 2])> = segs
                .iter()
                .map(|g| {
                    (
                        [g.a.x.min(g.b.x) - reach, g.a.y.min(g.b.y) - reach],
                        [g.a.x.max(g.b.x) + reach, g.a.y.max(g.b.y) + reach],
                    )
                })
                .collect();
            Some(BinGrid::new(&boxes, [side, side]))
        } else {
            None
        };
        Polyline2 { segs, total: s, reach, grid }
    }

    pub fn segment_count(&self) -> usize {
        self.segs.len()
    }

    /// The total length of every segment.
    pub fn length(&self) -> f32 {
        self.total
    }

    pub fn reach(&self) -> f32 {
        self.reach
    }

    /// The farthest segment end from the origin (a light's centre).
    pub fn bounding_radius(&self) -> f32 {
        self.segs.iter().fold(0.0f32, |r, g| r.max(g.a.length()).max(g.b.length()))
    }

    /// Squared distance to segment `i`, and the arc length at its nearest point.
    #[inline]
    fn seg_nearest(&self, i: usize, p: Vec2f) -> (f32, f32) {
        let g = &self.segs[i];
        let w = p - g.a;
        let h = ((w.x * g.e.x + w.y * g.e.y) * g.inv_len2).clamp(0.0, 1.0);
        let dx = w.x - g.e.x * h;
        let dy = w.y - g.e.y * h;
        (dx * dx + dy * dy, g.s0 + h * g.len)
    }

    fn far(&self) -> StrokeHit {
        StrokeHit { dist: self.reach, t: 0.0 }
    }

    fn finish(&self, best: Option<(f32, f32)>) -> StrokeHit {
        match best {
            Some((d2, s)) if d2.sqrt() < self.reach => {
                StrokeHit { dist: d2.sqrt(), t: if self.total > 0.0 { s / self.total } else { 0.0 } }
            }
            _ => self.far(),
        }
    }

    /// The nearest point, through the bins when there are many segments.
    pub fn nearest(&self, p: Vec2f) -> StrokeHit {
        let Some(grid) = &self.grid else {
            return self.nearest_brute(p);
        };
        let Some(cell) = grid.cell_of([p.x, p.y]) else {
            return self.far();
        };
        let mut best: Option<(f32, f32)> = None;
        for &i in grid.items_at(cell) {
            let (d2, s) = self.seg_nearest(i as usize, p);
            if best.map_or(true, |b| d2 < b.0) {
                best = Some((d2, s));
            }
        }
        self.finish(best)
    }

    /// The nearest point over every segment; the bins must agree with it.
    pub fn nearest_brute(&self, p: Vec2f) -> StrokeHit {
        let mut best: Option<(f32, f32)> = None;
        for i in 0..self.segs.len() {
            let (d2, s) = self.seg_nearest(i, p);
            if best.map_or(true, |b| d2 < b.0) {
                best = Some((d2, s));
            }
        }
        self.finish(best)
    }

    /// The distance to the nearest segment with no reach cap, by brute force:
    /// for set-up work, not per pixel. f32::MAX without segments.
    pub fn exact_distance(&self, p: Vec2f) -> f32 {
        (0..self.segs.len()).map(|i| self.seg_nearest(i, p).0).fold(f32::MAX, f32::min).sqrt()
    }

    /// The nonzero winding number around `p` over the given edges. Only an
    /// edge that crosses p's row can count, and a crossing counts +1 upward
    /// with `p` on its left, -1 downward with `p` on its right.
    fn winding_in(&self, p: Vec2f, edges: impl Iterator<Item = usize>) -> i32 {
        let mut wn = 0;
        for i in edges {
            let g = &self.segs[i];
            let side = g.e.x * (p.y - g.a.y) - g.e.y * (p.x - g.a.x);
            if g.a.y <= p.y {
                if g.b.y > p.y && side > 0.0 {
                    wn += 1;
                }
            } else if g.b.y <= p.y && side < 0.0 {
                wn -= 1;
            }
        }
        wn
    }
}

/// A filled outline: closed contours, the signed distance to them (negative
/// inside, nonzero winding), capped at the reach.
#[derive(Clone, Debug)]
pub struct FillShape {
    outline: Polyline2,
    /// The edges binned into rows by their y extent, so the winding test
    /// visits only the edges that can cross p's row (Some past GRID_MIN_SEGMENTS).
    bands: Option<BinGrid<1>>,
}

impl FillShape {
    /// Every contour is closed back to its first point.
    pub fn new(contours: &[Vec<[f32; 2]>], reach: f32) -> FillShape {
        let outline = Polyline2::new(contours, true, reach);
        let bands = if outline.segs.len() > GRID_MIN_SEGMENTS {
            let rows = (outline.segs.len() / 2).clamp(4, 256);
            let boxes: Vec<([f32; 1], [f32; 1])> =
                outline.segs.iter().map(|g| ([g.a.y.min(g.b.y)], [g.a.y.max(g.b.y)])).collect();
            Some(BinGrid::new(&boxes, [rows]))
        } else {
            None
        };
        FillShape { outline, bands }
    }

    pub fn outline(&self) -> &Polyline2 {
        &self.outline
    }

    /// The winding number around `p`, through the row bins.
    pub fn winding(&self, p: Vec2f) -> i32 {
        match &self.bands {
            None => self.winding_brute(p),
            Some(bands) => match bands.cell_of([p.y]) {
                // Above or below every edge: no edge crosses this row.
                None => 0,
                Some(row) => self.outline.winding_in(p, bands.items_at(row).iter().map(|&i| i as usize)),
            },
        }
    }

    pub fn winding_brute(&self, p: Vec2f) -> i32 {
        self.outline.winding_in(p, 0..self.outline.segs.len())
    }

    /// Signed distance to the outline, negative inside, |d| capped at the reach.
    pub fn sdf(&self, p: Vec2f) -> f32 {
        let d = self.outline.nearest(p).dist;
        if self.winding(p) != 0 {
            -d
        } else {
            d
        }
    }

    pub fn sdf_brute(&self, p: Vec2f) -> f32 {
        let d = self.outline.nearest_brute(p).dist;
        if self.winding_brute(p) != 0 {
            -d
        } else {
            d
        }
    }
}

// ---- strokes on the sphere -----------------------------------------------------

#[derive(Clone, Copy, Debug)]
struct Arc3 {
    a: Vec3f,
    b: Vec3f,
    /// The unit normal of the arc's great circle, a × b normalised.
    n: Vec3f,
    /// The arc's angle, radians.
    angle: f32,
    /// Arc length from the path's start to `a`, radians.
    s0: f32,
}

/// A stroke on the sphere: great-circle arcs between unit directions, each
/// at most 5°, with distances and lengths in radians.
#[derive(Clone, Debug)]
pub struct ArcPath {
    arcs: Vec<Arc3>,
    total: f32,
    reach: f32,
    grid: Option<BinGrid<3>>,
    cull: CullCone,
}

impl ArcPath {
    /// Arcs between consecutive `points` (normalised; repeats dropped),
    /// closed back to the first when `closed` and there are three or more.
    /// A pair that is (almost) opposite has no single great circle between
    /// it, so it goes through a point a right angle from both.
    pub fn new(points: &[Vec3f], closed: bool, reach: f32) -> ArcPath {
        let reach = sanitize_reach(reach);
        let mut pts: Vec<Vec3f> = Vec::with_capacity(points.len() + 1);
        for p in points {
            let q = p.normalize();
            // normalize() gives zero for a zero or NaN input.
            if !(q.length_squared() > 0.5) {
                continue;
            }
            if pts.last().map_or(true, |l| angle_between(*l, q) > MIN_ARC) {
                pts.push(q);
            }
        }
        if closed && pts.len() >= 3 {
            let first = pts[0];
            pts.push(first);
        }
        let mut arcs = Vec::new();
        let mut total = 0.0f32;
        for w in pts.windows(2) {
            let (a, b) = (w[0], w[1]);
            if angle_between(a, b) > std::f32::consts::PI - 1.0e-3 {
                let side = if a.x.abs() < 0.9 { vec3f(1.0, 0.0, 0.0) } else { vec3f(0.0, 1.0, 0.0) };
                let m = Vec3f::cross(a, side).normalize();
                push_arc(&mut arcs, &mut total, a, m);
                push_arc(&mut arcs, &mut total, m, b);
            } else {
                push_arc(&mut arcs, &mut total, a, b);
            }
        }
        let widest = arcs.iter().fold(0.0f32, |w, g| w.max(g.angle));
        let cull = {
            let mut dirs: Vec<Vec3f> = arcs.iter().map(|g| g.a).collect();
            if let Some(last) = arcs.last() {
                dirs.push(last.b);
            }
            // An arc's middle lies within half its length of an end.
            CullCone::around(&dirs, reach + 0.5 * widest)
        };
        let grid = if arcs.len() > GRID_MIN_SEGMENTS && reach < std::f32::consts::PI {
            // A direction within `reach` radians of an arc point is within this
            // straight-line distance of it; the arc itself bows out from its
            // chord by at most its sagitta, 1 - cos(angle / 2).
            let chord = 2.0 * (0.5 * reach).sin();
            let boxes: Vec<([f32; 3], [f32; 3])> = arcs
                .iter()
                .map(|g| {
                    let pad = (1.0 - (0.5 * g.angle).cos()) + chord;
                    (
                        [g.a.x.min(g.b.x) - pad, g.a.y.min(g.b.y) - pad, g.a.z.min(g.b.z) - pad],
                        [g.a.x.max(g.b.x) + pad, g.a.y.max(g.b.y) + pad, g.a.z.max(g.b.z) + pad],
                    )
                })
                .collect();
            // Cells stay large against a padded arc's box, so an arc is listed
            // in at most about 5 cells an axis however far the stroke reaches.
            // (A stroke as wide as the sky would otherwise list every arc in
            // every cell: 4096 arcs x 32^3 cells, hundreds of MB per light.)
            let widest_box = widest + 2.0 * ((1.0 - (0.5 * widest).cos()) + chord);
            let fit = ((8.0 / widest_box).floor() as usize).max(1);
            let side = (((arcs.len() as f32).cbrt() * 2.0).ceil() as usize).clamp(4, 32).min(fit);
            Some(BinGrid::new(&boxes, [side, side, side]))
        } else {
            None
        };
        ArcPath { arcs, total, reach, grid, cull }
    }

    pub fn arc_count(&self) -> usize {
        self.arcs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.arcs.is_empty()
    }

    /// The total length, radians.
    pub fn length(&self) -> f32 {
        self.total
    }

    pub fn reach(&self) -> f32 {
        self.reach
    }

    /// The same arcs with `margin` radians more reach: `nearest` then finds
    /// every direction within that much more of the path, which tells a scan
    /// whether a cell of that radius round a direction may touch the stroke.
    pub fn widened(&self, margin: f32) -> ArcPath {
        let mut points: Vec<Vec3f> = self.arcs.iter().map(|g| g.a).collect();
        if let Some(last) = self.arcs.last() {
            points.push(last.b);
        }
        ArcPath::new(&points, false, self.reach + margin.max(0.0))
    }

    /// Every direction within reach of the path lies in this cone.
    pub fn cull(&self) -> CullCone {
        self.cull
    }

    fn far(&self) -> StrokeHit {
        StrokeHit { dist: self.reach, t: 0.0 }
    }

    fn finish(&self, best: Option<(f32, f32)>) -> StrokeHit {
        match best {
            Some((d, s)) if d < self.reach => StrokeHit { dist: d, t: if self.total > 0.0 { s / self.total } else { 0.0 } },
            _ => self.far(),
        }
    }

    /// The nearest point to the unit direction `dir`, through the bins when
    /// there are many arcs.
    pub fn nearest(&self, dir: Vec3f) -> StrokeHit {
        let Some(grid) = &self.grid else {
            return self.nearest_brute(dir);
        };
        let Some(cell) = grid.cell_of([dir.x, dir.y, dir.z]) else {
            return self.far();
        };
        let mut best: Option<(f32, f32)> = None;
        for &i in grid.items_at(cell) {
            let (d, s) = arc_nearest(&self.arcs[i as usize], dir);
            if best.map_or(true, |b| d < b.0) {
                best = Some((d, s));
            }
        }
        self.finish(best)
    }

    pub fn nearest_brute(&self, dir: Vec3f) -> StrokeHit {
        let mut best: Option<(f32, f32)> = None;
        for g in &self.arcs {
            let (d, s) = arc_nearest(g, dir);
            if best.map_or(true, |b| d < b.0) {
                best = Some((d, s));
            }
        }
        self.finish(best)
    }
}

/// Appends the great-circle arc a -> b, cut into equal pieces of at most
/// MAX_ARC along the circle (slerp, so the cut points lie on it exactly).
fn push_arc(arcs: &mut Vec<Arc3>, total: &mut f32, a: Vec3f, b: Vec3f) {
    let theta = angle_between(a, b);
    if theta < MIN_ARC {
        return;
    }
    let pieces = (theta / MAX_ARC).ceil().max(1.0) as usize;
    let inv_sin = 1.0 / theta.sin();
    let mut prev = a;
    for k in 1..=pieces {
        let next = if k == pieces {
            b
        } else {
            let f = k as f32 / pieces as f32;
            ((a * ((1.0 - f) * theta).sin() + b * (f * theta).sin()) * inv_sin).normalize()
        };
        let angle = angle_between(prev, next);
        if angle >= MIN_ARC {
            arcs.push(Arc3 { a: prev, b: next, n: Vec3f::cross(prev, next).normalize(), angle, s0: *total });
            *total += angle;
            prev = next;
        }
    }
}

/// Angular distance from unit `p` to one arc, and the arc length along the
/// path at the nearest point. `p` is projected onto the arc's great-circle
/// plane: when that lands between the ends, the distance is p's angle off
/// the plane; otherwise the nearer end is the nearest point.
#[inline]
fn arc_nearest(g: &Arc3, p: Vec3f) -> (f32, f32) {
    let pn = p.dot(g.n);
    let q = p - g.n * pn;
    let ql = q.length();
    if ql > 1.0e-6 {
        let qd = q * (1.0 / ql);
        let along = Vec3f::cross(g.a, qd).dot(g.n).atan2(g.a.dot(qd));
        if along >= 0.0 && along <= g.angle {
            return (pn.abs().atan2(ql), g.s0 + along);
        }
    }
    let da = angle_between(p, g.a);
    let db = angle_between(p, g.b);
    if da <= db {
        (da, g.s0)
    } else {
        (db, g.s0 + g.angle)
    }
}

// ---- flattening through makepad-tween --------------------------------------------

/// A tween path as contours, one per subpath, with repeated points dropped.
/// A line gives its end point, a curve `steps` points.
pub fn contours_from_path(path: &MotionPath, steps: u32) -> Contours {
    let mut out: Contours = Vec::new();
    path.flatten(steps.max(1), |p, starts| {
        if starts || out.is_empty() {
            out.push(Vec::new());
        }
        if let Some(c) = out.last_mut() {
            let q = [p[0] as f32, p[1] as f32];
            if c.last() != Some(&q) {
                c.push(q);
            }
        }
    });
    out.retain(|c| c.len() >= 2);
    out
}

/// SVG path data (M L H V C S Q T A Z) as contours. The error text carries
/// the byte offset, for a caller that hands it back to whoever wrote the path.
pub fn contours_from_svg(d: &str, steps: u32) -> Result<Contours, String> {
    let path = MotionPath::from_svg(d).map_err(|e| e.to_string())?;
    Ok(contours_from_path(&path, steps))
}

/// A smooth curve through `points` (tween's `through`: 0 straight lines, 1 a
/// natural curve, 2 a loose one), flattened with `steps` points per span.
/// The curve passes exactly through every point; a last point equal to the
/// first closes it.
pub fn flatten_through(points: &[[f32; 2]], curviness: f32, steps: u32) -> Result<Vec<[f32; 2]>, String> {
    let pts: Vec<[f64; 2]> = points.iter().map(|p| [p[0] as f64, p[1] as f64]).collect();
    let path = MotionPath::through(&pts, curviness as f64).map_err(|e| e.to_string())?;
    Ok(contours_from_path(&path, steps).into_iter().next().unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hdri::dir_from_az_el;

    /// A small deterministic generator, so the brute-force comparisons are
    /// the same on every run.
    fn lcg(state: &mut u32) -> f32 {
        *state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (*state >> 8) as f32 / 16_777_216.0
    }

    /// A direction spread evenly over the sphere.
    fn random_dir(seed: &mut u32) -> Vec3f {
        let z = lcg(seed) * 2.0 - 1.0;
        let phi = lcg(seed) * std::f32::consts::TAU;
        let r = (1.0 - z * z).max(0.0).sqrt();
        vec3f(r * phi.cos(), z, r * phi.sin())
    }

    #[test]
    fn stroke_distance_and_t_are_exact_on_a_straight_segment() {
        let line = Polyline2::new(&vec![vec![[0.0, 0.0], [2.0, 0.0]]], false, 1.0);
        let hit = line.nearest(vec2f(0.5, 0.3));
        assert!((hit.dist - 0.3).abs() < 1.0e-6, "{hit:?}");
        assert!((hit.t - 0.25).abs() < 1.0e-6, "{hit:?}");
        // Past the end the nearest point is the end itself, so t is 1.
        let end = line.nearest(vec2f(2.4, 0.3));
        assert!((end.dist - 0.5).abs() < 1.0e-6 && (end.t - 1.0).abs() < 1.0e-6, "{end:?}");
        // Beyond the reach nothing is near: the distance is capped and t is 0.
        assert_eq!(line.nearest(vec2f(0.5, 3.0)), StrokeHit { dist: 1.0, t: 0.0 });
        assert!((line.exact_distance(vec2f(0.5, 3.0)) - 3.0).abs() < 1.0e-6, "the exact distance is not capped");
        assert!((line.length() - 2.0).abs() < 1.0e-6);
    }

    #[test]
    fn the_filled_distance_has_the_right_sign_on_a_concave_shape() {
        // An L: the notch at the top right lies inside the bounding box but outside the shape.
        let l: Contours = vec![vec![[0.0, 0.0], [2.0, 0.0], [2.0, 1.0], [1.0, 1.0], [1.0, 2.0], [0.0, 2.0]]];
        let fill = FillShape::new(&l, 10.0);
        assert!(fill.sdf(vec2f(0.5, 1.5)) < 0.0, "the upper arm is inside");
        assert!(fill.sdf(vec2f(1.5, 0.5)) < 0.0, "the lower arm is inside");
        let notch = fill.sdf(vec2f(1.5, 1.5));
        assert!((notch - 0.5).abs() < 1.0e-6, "the notch is outside, 0.5 from both inner edges: {notch}");
        assert!((fill.sdf(vec2f(0.5, 0.5)) + 0.5).abs() < 1.0e-6);
        // The same outline drawn clockwise fills the same.
        let mut cw = l.clone();
        cw[0].reverse();
        let fill_cw = FillShape::new(&cw, 10.0);
        for p in [vec2f(0.5, 1.5), vec2f(1.5, 1.5), vec2f(1.5, 0.5), vec2f(3.0, 3.0)] {
            assert!((fill.sdf(p) - fill_cw.sdf(p)).abs() < 1.0e-6, "{p:?}");
        }
    }

    #[test]
    fn a_reversed_inner_contour_cuts_a_hole() {
        let outer = vec![[-2.0, -2.0], [2.0, -2.0], [2.0, 2.0], [-2.0, 2.0]];
        let hole = vec![[-1.0, -1.0], [-1.0, 1.0], [1.0, 1.0], [1.0, -1.0]];
        let fill = FillShape::new(&vec![outer, hole], 10.0);
        assert_eq!(fill.winding(vec2f(0.0, 0.0)), 0);
        assert!(fill.sdf(vec2f(0.0, 0.0)) > 0.0);
        assert_eq!(fill.winding(vec2f(1.5, 0.0)), 1);
        assert!(fill.sdf(vec2f(1.5, 0.0)) < 0.0);
    }

    /// A wavy ring of 240 edges, well past GRID_MIN_SEGMENTS.
    fn wavy_ring() -> Contours {
        let n = 240;
        vec![(0..n)
            .map(|i| {
                let a = i as f32 / n as f32 * std::f32::consts::TAU;
                let r = 1.0 + 0.3 * (7.0 * a).sin();
                [r * a.cos(), r * a.sin()]
            })
            .collect()]
    }

    #[test]
    fn plane_bins_agree_with_brute_force() {
        let fill = FillShape::new(&wavy_ring(), 0.05);
        assert!(fill.outline.grid.is_some() && fill.bands.is_some(), "the ring must be binned");
        let open = Polyline2::new(&wavy_ring(), false, 0.08);
        assert!(open.grid.is_some());
        let mut seed = 7u32;
        for _ in 0..4000 {
            let p = vec2f(lcg(&mut seed) * 3.4 - 1.7, lcg(&mut seed) * 3.4 - 1.7);
            assert_eq!(fill.sdf(p), fill.sdf_brute(p), "fill at {p:?}");
            assert_eq!(fill.winding(p), fill.winding_brute(p), "winding at {p:?}");
            assert_eq!(open.nearest(p), open.nearest_brute(p), "stroke at {p:?}");
        }
    }

    #[test]
    fn great_circle_distance_and_t_are_exact_on_the_equator() {
        let path = ArcPath::new(&[dir_from_az_el(80.0, 0.0), dir_from_az_el(100.0, 0.0)], false, 0.2);
        assert!(path.arc_count() >= 4, "a 20° arc is cut into pieces of at most 5°");
        let hit = path.nearest(dir_from_az_el(85.0, 3.0));
        assert!((hit.dist - 3.0f32.to_radians()).abs() < 2.0e-5, "{hit:?}");
        assert!((hit.t - 0.25).abs() < 2.0e-5, "{hit:?}");
        assert!((path.length() - 20.0f32.to_radians()).abs() < 1.0e-5);
        // Past the start the nearest point is the start.
        let before = path.nearest(dir_from_az_el(78.0, 0.0));
        assert!((before.dist - 2.0f32.to_radians()).abs() < 2.0e-5 && before.t == 0.0, "{before:?}");
    }

    /// A spiral of 300 points, one every 3° of azimuth, climbing from -60° to 60°.
    fn spiral() -> Vec<Vec3f> {
        (0..300).map(|i| dir_from_az_el(i as f32 * 3.0, -60.0 + i as f32 * 0.4)).collect()
    }

    #[test]
    fn sphere_bins_agree_with_brute_force() {
        let path = ArcPath::new(&spiral(), false, 0.06);
        assert!(path.grid.is_some(), "the spiral must be binned");
        let mut seed = 11u32;
        let mut near = 0;
        for _ in 0..6000 {
            let d = random_dir(&mut seed);
            let brute = path.nearest_brute(d);
            assert_eq!(path.nearest(d), brute, "at {d:?}");
            if brute.dist < path.reach() {
                near += 1;
            }
        }
        assert!(near > 50, "the test must see directions near the path ({near})");
    }

    #[test]
    fn the_cull_cone_holds_everything_a_stroke_reaches() {
        let pts: Vec<Vec3f> = (0..40).map(|i| dir_from_az_el(10.0 + i as f32, 10.0 + 0.5 * i as f32)).collect();
        let path = ArcPath::new(&pts, false, 0.05);
        let cull = path.cull();
        assert!(cull.cos_max > 0.5, "a 40° stroke needs a narrow cone: {cull:?}");
        assert!(!cull.contains(dir_from_az_el(200.0, -10.0)));
        let mut seed = 3u32;
        for _ in 0..20000 {
            let d = random_dir(&mut seed);
            if path.nearest_brute(d).dist < path.reach() {
                assert!(cull.contains(d), "the cone dropped a lit direction {d:?}");
            }
        }
        assert!(CullCone::ALL.contains(vec3f(0.0, -1.0, 0.0)));
    }

    #[test]
    fn svg_paths_and_splines_flatten_through_tween() {
        let square = contours_from_svg("M0 0 L1 0 L1 1 Z", 8).unwrap();
        assert_eq!(square, vec![vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 0.0]]]);
        let err = contours_from_svg("M0 0 L1 0 X", 8).unwrap_err();
        assert!(err.contains("byte"), "the error names where it failed: {err}");
        let line = flatten_through(&[[0.0, 0.0], [10.0, 5.0], [20.0, 0.0]], 1.0, 8).unwrap();
        assert_eq!(line.len(), 17, "the start, then 8 points per curve");
        assert_eq!(line[0], [0.0, 0.0]);
        assert_eq!(line[8], [10.0, 5.0], "the curve passes through its points");
        assert_eq!(line[16], [20.0, 0.0]);
    }
}
