//! Sketches as distance fields: a sketch's regions (outlines and their holes, as straight edges
//! within a chord of the curves) pulled along their plane's normal or turned about Y, with a true
//! distance bound.
//!
//! The outline's 2D distance is its nearest edge's, signed by the side the point is on: the edge's
//! own side, or at a corner the side of the two edges' normals summed (outer loops run
//! counter-clockwise and holes clockwise, so the material is on the left of every edge). Regions
//! that overlap join by their least distance: exact outside them all, a bound inside the overlap.
//! A grid of cells over each region lists, nearest first, the edges that can be the nearest
//! anywhere in the cell; a query reads its cell's list until the next is further than the nearest
//! so far. Outside, a region's distance is the larger of its box's and the nearest edge's, that
//! held to `reach`: exact within reach of the outline, a lower bound beyond it, 1-Lipschitz
//! everywhere; the grid covers the box grown by reach, and past it the box alone is the bound.
//!
//! In 3D an extrusion combines the outline's distance across (x) with the caps' along (y) as
//! `min(max(x, y), 0) + |(max(x, 0), max(y, 0))|`, exact. A draft draws the walls in by its
//! tangent per unit of height: the side is the outline's distance plus that inset, times the
//! draft's cosine, and the whole over the square root of one plus its sine (the side's slope then
//! leans into the caps'), a bound. The inset is the outline offset as its distance offsets it:
//! drawn in, its inner (reflex) corners round; drawn out, its outer ones. A revolve reads the
//! outline at (the distance from Y, the height), exact; less than a full turn meets the
//! half-planes of its two ends (a bound).

use crate::shape::P3;
use makepad_csg_math::Vec3d;

/// A sketch point: u, v.
pub type P2 = [f64; 2];

/// One edge: from `a` along `ab` (`il2` = 1 / |ab|²), its outward unit normal (to the right of
/// its run), and the edges before and after it in its loop.
#[derive(Clone, Copy, Debug)]
struct Edge {
    a: P2,
    ab: P2,
    il2: f64,
    n: P2,
    prev: u32,
    next: u32,
}

impl Edge {
    /// The squared distance from q, and where along the edge its nearest point is (0 to 1).
    fn nearest(&self, q: P2) -> (f64, f64) {
        let aq = [q[0] - self.a[0], q[1] - self.a[1]];
        let t = ((aq[0] * self.ab[0] + aq[1] * self.ab[1]) * self.il2).clamp(0.0, 1.0);
        let d = [aq[0] - t * self.ab[0], aq[1] - t * self.ab[1]];
        (d[0] * d[0] + d[1] * d[1], t)
    }
}

/// Up to [`GROUP`] neighbouring edges of one loop, inside a ball.
struct Group {
    c: P2,
    r: f64,
    first: u32,
    count: u32,
}

const GROUP: usize = 8;

/// A region's cells: cell (i, j)'s entries are `start[c]..start[c + 1]` (c = j * n[0] + i), each
/// an edge and how near it comes (no point of the cell is nearer it), nearest first.
#[derive(Debug)]
struct Grid {
    lo: P2,
    step: f64,
    n: [usize; 2],
    start: Vec<u32>,
    edge: Vec<u32>,
    near: Vec<f64>,
}

#[derive(Debug)]
struct Region {
    min: P2,
    max: P2,
    grid: Grid,
}

/// A sketch's regions as a signed 2D distance.
#[derive(Debug)]
pub struct Outline {
    edges: Vec<Edge>,
    regions: Vec<Region>,
    /// The box round every region.
    pub min: P2,
    pub max: P2,
    /// Exact within this distance of the outline: a quarter of the box's longer side.
    pub reach: f64,
}

fn sub(a: P2, b: P2) -> P2 {
    [a[0] - b[0], a[1] - b[1]]
}

fn dot(a: P2, b: P2) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

fn len(a: P2) -> f64 {
    a[0].hypot(a[1])
}

/// How far q is outside the box (0 inside).
fn outside(min: P2, max: P2, q: P2) -> f64 {
    let d = |k: usize| (min[k] - q[k]).max(q[k] - max[k]).max(0.0);
    d(0).hypot(d(1))
}

impl Outline {
    /// The outline of `regions`, each its loops (the outer counter-clockwise, then its holes
    /// clockwise; no repeated last point).
    pub fn new(regions: &[Vec<Vec<P2>>]) -> Result<Outline, String> {
        let mut edges: Vec<Edge> = Vec::new();
        let mut groups: Vec<Vec<Group>> = Vec::new();
        let mut boxes = Vec::new();
        for (ri, loops) in regions.iter().enumerate() {
            let mut own = Vec::new();
            let (mut min, mut max) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
            for (li, l) in loops.iter().enumerate() {
                let first = edges.len();
                for (i, &a) in l.iter().enumerate() {
                    let b = l[(i + 1) % l.len()];
                    let ab = sub(b, a);
                    let d = len(ab);
                    if d < 1e-12 {
                        continue;
                    }
                    for k in 0..2 {
                        min[k] = min[k].min(a[k]);
                        max[k] = max[k].max(a[k]);
                    }
                    edges.push(Edge { a, ab, il2: 1.0 / (d * d), n: [ab[1] / d, -ab[0] / d], prev: 0, next: 0 });
                }
                let count = edges.len() - first;
                if count < 3 {
                    return Err(format!("region {ri}, loop {li}: an outline needs three corners or more"));
                }
                for i in 0..count {
                    edges[first + i].prev = (first + (i + count - 1) % count) as u32;
                    edges[first + i].next = (first + (i + 1) % count) as u32;
                }
                for at in (first..edges.len()).step_by(GROUP) {
                    let to = (at + GROUP).min(edges.len());
                    let ends: Vec<P2> = edges[at..to].iter().flat_map(|e| [e.a, [e.a[0] + e.ab[0], e.a[1] + e.ab[1]]]).collect();
                    let lo = ends.iter().fold([f64::INFINITY; 2], |m, p| [m[0].min(p[0]), m[1].min(p[1])]);
                    let hi = ends.iter().fold([f64::NEG_INFINITY; 2], |m, p| [m[0].max(p[0]), m[1].max(p[1])]);
                    let c = [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5];
                    let r = ends.iter().map(|p| len(sub(*p, c))).fold(0.0, f64::max);
                    own.push(Group { c, r, first: at as u32, count: (to - at) as u32 });
                }
            }
            if own.is_empty() {
                return Err(format!("region {ri} has no outline"));
            }
            groups.push(own);
            boxes.push((min, max));
        }
        if boxes.is_empty() {
            return Err("a sketch field needs a region".into());
        }
        let min = boxes.iter().fold([f64::INFINITY; 2], |m, b| [m[0].min(b.0[0]), m[1].min(b.0[1])]);
        let max = boxes.iter().fold([f64::NEG_INFINITY; 2], |m, b| [m[0].max(b.1[0]), m[1].max(b.1[1])]);
        let reach = ((max[0] - min[0]).max(max[1] - min[1]) * 0.25).max(1e-3);
        let regions = boxes.iter().zip(&groups).map(|(&(min, max), g)| Region { min, max, grid: grid(&edges, g, min, max, reach) }).collect();
        Ok(Outline { edges, regions, min, max, reach })
    }

    /// How many straight edges make it.
    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    /// The signed distance at q: negative inside.
    pub fn distance(&self, q: P2) -> f64 {
        let mut d = f64::INFINITY;
        for r in &self.regions {
            let b = outside(r.min, r.max, q);
            // a region is at least its box away
            if b > 0.0 && b >= d {
                continue;
            }
            d = d.min(self.region(r, q, b));
        }
        d
    }

    fn region(&self, r: &Region, q: P2, b: f64) -> f64 {
        let g = &r.grid;
        let at = |k: usize| ((q[k] - g.lo[k]) / g.step).floor();
        let (i, j) = (at(0), at(1));
        if i < 0.0 || j < 0.0 || i >= g.n[0] as f64 || j >= g.n[1] as f64 {
            // past the grid: more than reach away, the box bounds it
            return b;
        }
        let c = j as usize * g.n[0] + i as usize;
        let (mut best, mut edge, mut t) = (f64::INFINITY, 0usize, 0.0);
        for k in g.start[c] as usize..g.start[c + 1] as usize {
            let near = g.near[k];
            if near * near >= best {
                break;
            }
            let e = g.edge[k] as usize;
            let (d2, s) = self.edges[e].nearest(q);
            if d2 < best {
                (best, edge, t) = (d2, e, s);
            }
        }
        let d = best.sqrt();
        let e = &self.edges[edge];
        let (from, n) = match t {
            t if t <= 0.0 => (e.a, add(e.n, self.edges[e.prev as usize].n)),
            t if t >= 1.0 => ([e.a[0] + e.ab[0], e.a[1] + e.ab[1]], add(e.n, self.edges[e.next as usize].n)),
            _ => (e.a, e.n),
        };
        if dot(sub(q, from), n) < 0.0 {
            -d
        } else {
            b.max(d.min(self.reach))
        }
    }
}

fn add(a: P2, b: P2) -> P2 {
    [a[0] + b[0], a[1] + b[1]]
}

/// A region's grid over its box grown by `reach`, about two cells per edge (64 to 4096).
fn grid(edges: &[Edge], groups: &[Group], min: P2, max: P2, reach: f64) -> Grid {
    let lo = [min[0] - reach, min[1] - reach];
    let size = [max[0] - min[0] + 2.0 * reach, max[1] - min[1] + 2.0 * reach];
    let count: usize = groups.iter().map(|g| g.count as usize).sum();
    let cells = (2 * count).clamp(64, 4096) as f64;
    let step = (size[0] * size[1] / cells).sqrt().max(1e-6);
    let n = [((size[0] / step).ceil() as usize).max(1), ((size[1] / step).ceil() as usize).max(1)];
    let half = step * std::f64::consts::FRAC_1_SQRT_2;
    let mut start = Vec::with_capacity(n[0] * n[1] + 1);
    let (mut edge, mut near) = (Vec::new(), Vec::new());
    let mut list: Vec<(f64, u32)> = Vec::new();
    for j in 0..n[1] {
        for i in 0..n[0] {
            start.push(edge.len() as u32);
            let c = [lo[0] + (i as f64 + 0.5) * step, lo[1] + (j as f64 + 0.5) * step];
            // no point of the cell is further than `most` from its nearest edge
            let most = groups.iter().map(|g| len(sub(c, g.c)) + g.r).fold(f64::INFINITY, f64::min) + half;
            list.clear();
            let mut most_edge = most;
            for g in groups {
                if len(sub(c, g.c)) - g.r - half > most {
                    continue;
                }
                for e in g.first..g.first + g.count {
                    let d = edges[e as usize].nearest(c).0.sqrt();
                    most_edge = most_edge.min(d + half);
                    list.push((d - half, e));
                }
            }
            list.retain(|(lower, _)| *lower <= most_edge);
            list.sort_by(|a, b| a.0.total_cmp(&b.0));
            for (lower, e) in &list {
                edge.push(*e);
                near.push(lower.max(0.0));
            }
        }
    }
    start.push(edge.len() as u32);
    Grid { lo, step, n, start, edge, near }
}

/// How a sketch becomes a solid.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Form {
    /// Pulled along `w` from `z0` to `z1` (`u` and `v`: the sketch's axes in the model, `w` their
    /// normal), the walls drawn in by `draft` degrees.
    Extrude { u: P3, v: P3, w: P3, z0: f64, z1: f64, draft: f64 },
    /// Turned about +Y from +X towards -Z through `angle` degrees: u the radius, v the height.
    Revolve { angle: f64 },
}

/// A sketch's solid as a field.
#[derive(Debug)]
pub struct SketchSolid {
    pub outline: Outline,
    pub form: Form,
    /// The box holding it.
    pub min: P3,
    pub max: P3,
    key: [u8; 32],
    /// A draft's tangent, cosine and the side's slope into the caps (1 without one).
    tan: f64,
    cos: f64,
    lean: f64,
}

fn dot3(a: P3, b: P3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

impl SketchSolid {
    /// The solid `form` makes of `regions` (each its loops, the outer counter-clockwise, then its
    /// holes clockwise).
    pub fn new(regions: &[Vec<Vec<P2>>], form: Form) -> Result<SketchSolid, String> {
        let outline = Outline::new(regions)?;
        let mut bytes: Vec<u8> = Vec::new();
        let mut put = |x: f64| bytes.extend_from_slice(&x.to_bits().to_le_bytes());
        for loops in regions {
            put(loops.len() as f64);
            for l in loops {
                put(l.len() as f64);
                l.iter().flatten().for_each(|x| put(*x));
            }
        }
        let (mut tan, mut cos, mut lean) = (0.0, 1.0, 1.0);
        let (min, max) = match form {
            Form::Extrude { u, v, w, z0, z1, draft } => {
                if !(z1 > z0) {
                    return Err("an extrusion's height is more than 0".into());
                }
                if !(draft.abs() < 80.0) {
                    return Err("a draft stays under 80 degrees".into());
                }
                [0.0, u[0], u[1], u[2], v[0], v[1], v[2], w[0], w[1], w[2], z0, z1, draft].iter().for_each(|x| put(*x));
                let a = draft.to_radians();
                (tan, cos, lean) = (a.tan(), a.cos(), (1.0 + a.sin().abs()).sqrt());
                // an outward draft widens the top
                let grow = (-(z1 - z0) * tan).max(0.0);
                let (mut min, mut max) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
                for c in 0..8 {
                    let pu = if c & 1 == 0 { outline.min[0] - grow } else { outline.max[0] + grow };
                    let pv = if c & 2 == 0 { outline.min[1] - grow } else { outline.max[1] + grow };
                    let pz = if c & 4 == 0 { z0 } else { z1 };
                    for k in 0..3 {
                        let x = pu * u[k] + pv * v[k] + pz * w[k];
                        min[k] = min[k].min(x);
                        max[k] = max[k].max(x);
                    }
                }
                (min, max)
            }
            Form::Revolve { angle } => {
                if !(angle > 0.0 && angle <= 360.0) {
                    return Err(format!("a revolve turns more than 0 and up to 360 degrees, not {angle}"));
                }
                if outline.min[0] < -1e-9 {
                    return Err("a revolve's sketch stays on the +u side of its axis".into());
                }
                [1.0, angle].iter().for_each(|x| put(*x));
                let r = outline.max[0];
                ([-r, outline.min[1], -r], [r, outline.max[1], r])
            }
        };
        let key = makepad_core_util::sha256::sha256(&bytes);
        Ok(SketchSolid { outline, form, min, max, key, tan, cos, lean })
    }

    /// Its identity: the sha256 of its canonical bytes (the loops and the form).
    pub fn key(&self) -> [u8; 32] {
        self.key
    }

    /// The signed distance (a bound where the module says).
    pub fn distance(&self, p: Vec3d) -> f64 {
        let q = [p.x, p.y, p.z];
        match self.form {
            Form::Extrude { u, v, w, z0, z1, .. } => {
                let z = dot3(q, w);
                let x = (self.outline.distance([dot3(q, u), dot3(q, v)]) + (z - z0) * self.tan) * self.cos;
                let y = (z0 - z).max(z - z1);
                (x.max(y).min(0.0) + x.max(0.0).hypot(y.max(0.0))) / self.lean
            }
            Form::Revolve { angle } => {
                let d = self.outline.distance([q[0].hypot(q[2]), q[1]]);
                if angle >= 360.0 {
                    return d;
                }
                // the end at 0 (the +X half of the XY plane) and the end at `angle`, each 0 on its
                // plane and positive on the side away from the turn
                let (s, c) = angle.to_radians().sin_cos();
                let (start, end) = (q[2], -q[2] * c - q[0] * s);
                d.max(if angle <= 180.0 { start.max(end) } else { start.min(end) })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rnd(seed: &mut u64) -> f64 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        (*seed >> 11) as f64 / (1u64 << 53) as f64
    }

    /// A regular n-gon round c of circumradius r, counter-clockwise (clockwise: a hole).
    fn ngon(c: P2, r: f64, n: usize, ccw: bool) -> Vec<P2> {
        let mut l: Vec<P2> = (0..n).map(|i| {
            let a = std::f64::consts::TAU * i as f64 / n as f64;
            [c[0] + r * a.cos(), c[1] + r * a.sin()]
        }).collect();
        if !ccw {
            l.reverse();
        }
        l
    }

    /// The true signed distance to the loops: the nearest edge, inside where a ray crosses them an
    /// odd number of times.
    fn brute(regions: &[Vec<Vec<P2>>], q: P2) -> f64 {
        let (mut d, mut inside) = (f64::INFINITY, false);
        for l in regions.iter().flatten() {
            for i in 0..l.len() {
                let (a, b) = (l[i], l[(i + 1) % l.len()]);
                let ab = sub(b, a);
                let t = (dot(sub(q, a), ab) / dot(ab, ab)).clamp(0.0, 1.0);
                d = d.min(len(sub(q, [a[0] + t * ab[0], a[1] + t * ab[1]])));
                if (a[1] > q[1]) != (b[1] > q[1]) && q[0] < a[0] + (q[1] - a[1]) / (b[1] - a[1]) * ab[0] {
                    inside = !inside;
                }
            }
        }
        if inside { -d } else { d }
    }

    /// A plate with a hole and a notch, and an island apart: the field is the brute distance with
    /// its sign wherever that is within reach or inside, never above it, and 1-Lipschitz.
    #[test]
    fn an_outline_is_its_nearest_edge_signed_by_its_side() {
        let plate = vec![vec![[0.0, 0.0], [20.0, 0.0], [20.0, 12.0], [12.0, 12.0], [10.0, 6.0], [8.0, 12.0], [0.0, 12.0]], ngon([5.0, 5.0], 2.5, 40, false)];
        let island = vec![ngon([30.0, 6.0], 4.0, 90, true)];
        let regions = vec![plate, island];
        let o = Outline::new(&regions).unwrap();
        assert_eq!(o.edge_count(), 7 + 40 + 90);
        let mut seed = 0x9e3779b97f4a7c15u64;
        let (mut near, mut prev) = (0, None::<(P2, f64)>);
        for k in 0..20000 {
            let q = if k % 2 == 0 {
                [-15.0 + 65.0 * rnd(&mut seed), -15.0 + 42.0 * rnd(&mut seed)]
            } else {
                // a walk, for the slope
                let (p, _) = prev.unwrap_or(([0.0, 0.0], 0.0));
                [p[0] + rnd(&mut seed) - 0.5, p[1] + rnd(&mut seed) - 0.5]
            };
            let (d, e) = (o.distance(q), brute(&regions, q));
            assert!(d <= e + 1e-9, "never above the true distance: {d} {e} at {q:?}");
            if e < o.reach {
                assert!((d - e).abs() < 1e-9, "exact within reach: {d} {e} at {q:?}");
                near += 1;
            }
            if let (Some((p, dp)), 1) = (prev, k % 2) {
                assert!((d - dp).abs() <= len(sub(q, p)) + 1e-9, "1-Lipschitz at {q:?}");
            }
            prev = Some((q, d));
        }
        assert!(near > 6000, "{near}");
        // on a corner and an edge: 0
        assert!(o.distance([10.0, 6.0]).abs() < 1e-12 && o.distance([20.0, 5.0]).abs() < 1e-12);
        assert!(Outline::new(&[vec![vec![[0.0, 0.0], [1.0, 0.0]]]]).is_err());
    }

    fn solid(regions: &[Vec<Vec<P2>>], form: Form) -> SketchSolid {
        SketchSolid::new(regions, form).unwrap()
    }

    /// Points on a solid's surface `step` apart or nearer, and the distance from q to the nearest:
    /// at least the true distance, and at most the spacing more.
    fn sampled(surface: &[P3], q: P3) -> f64 {
        surface.iter().map(|s| ((q[0] - s[0]).powi(2) + (q[1] - s[1]).powi(2) + (q[2] - s[2]).powi(2)).sqrt()).fold(f64::INFINITY, f64::min)
    }

    /// The surface of a regular n-gon prism (circumradius r, on the XZ ground, from y0 to y1, its
    /// top drawn in by `inset`), sampled about `step` apart.
    fn prism_surface(r: f64, n: usize, y0: f64, y1: f64, inset: f64, step: f64) -> Vec<P3> {
        let apothem = r * (std::f64::consts::PI / n as f64).cos();
        let at = |y: f64| r * (apothem - inset * (y - y0) / (y1 - y0)) / apothem;
        let rings = ((y1 - y0) / step).ceil() as usize;
        let mut out = Vec::new();
        for k in 0..=rings {
            let y = y0 + (y1 - y0) * k as f64 / rings as f64;
            let l = ngon([0.0, 0.0], at(y), n, true);
            for i in 0..n {
                let (a, b) = (l[i], l[(i + 1) % n]);
                let m = (len(sub(b, a)) / step).ceil() as usize;
                for s in 0..m {
                    let t = s as f64 / m as f64;
                    // the sketch's v is -Z on the ground
                    out.push([a[0] + t * (b[0] - a[0]), y, -(a[1] + t * (b[1] - a[1]))]);
                }
            }
        }
        for (y, rr) in [(y0, at(y0)), (y1, at(y1))] {
            let m = (rr / step).ceil() as i64;
            for i in -m..=m {
                for j in -m..=m {
                    let p = [i as f64 * step, j as f64 * step];
                    if brute(&[vec![ngon([0.0, 0.0], rr, n, true)]], p) < 0.0 {
                        out.push([p[0], y, -p[1]]);
                    }
                }
            }
        }
        out
    }

    const GROUND: (P3, P3, P3) = ([1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]);

    /// A 24-gon pulled up 8 from the ground: within the samples' spacing of a dense sampling of its
    /// surface everywhere near it and never above it; drawn in by a draft, never above it; drawn
    /// out, its box grows by the top's spread.
    #[test]
    fn an_extrusion_is_exact_and_a_draft_a_bound() {
        let regions = vec![vec![ngon([0.0, 0.0], 10.0, 24, true)]];
        let (u, v, w) = GROUND;
        let step = 0.25;
        let mut seed = 0x2545f4914f6cdd1du64;
        for (draft, inset) in [(0.0, 0.0), (20.0, 8.0 * 20f64.to_radians().tan())] {
            let s = solid(&regions, Form::Extrude { u, v, w, z0: 0.0, z1: 8.0, draft });
            let surface = prism_surface(10.0, 24, 0.0, 8.0, inset, step);
            let mut near = 0;
            for _ in 0..1500 {
                let q = [-16.0 + 32.0 * rnd(&mut seed), -5.0 + 18.0 * rnd(&mut seed), -16.0 + 32.0 * rnd(&mut seed)];
                let d = s.distance(Vec3d::new(q[0], q[1], q[2]));
                let e = sampled(&surface, q);
                assert!(d.abs() <= e + 1e-9, "draft {draft}: never above the true distance: {d} {e} at {q:?}");
                let inside = q[1] > 0.0 && q[1] < 8.0 && brute(&regions, [q[0], -q[2]]) < 0.0;
                if draft == 0.0 {
                    assert_eq!(d < 0.0, inside, "draft 0: the sign at {q:?}: {d}");
                    if e < s.outline.reach {
                        assert!(e - d.abs() <= step, "draft 0: within the spacing of the samples: {d} {e} at {q:?}");
                        near += 1;
                    }
                }
            }
            assert!(draft != 0.0 || near > 800, "{near}");
            assert!(s.min[1] == 0.0 && s.max[1] == 8.0 && (s.max[0] - 10.0).abs() < 1e-9, "{:?} {:?}", s.min, s.max);
        }
        let out = solid(&regions, Form::Extrude { u, v, w, z0: 0.0, z1: 8.0, draft: -10.0 });
        assert!((out.max[0] - 10.0 - 8.0 * 10f64.to_radians().tan()).abs() < 1e-9 && out.distance(Vec3d::new(10.5, 7.9, 0.0)) < 0.0, "{:?}", out.max);
        assert!(SketchSolid::new(&regions, Form::Extrude { u, v, w, z0: 0.0, z1: 0.0, draft: 0.0 }).is_err());
        assert!(SketchSolid::new(&regions, Form::Extrude { u, v, w, z0: 0.0, z1: 1.0, draft: 80.0 }).is_err());
    }

    /// A 256-gon of radius 3 at 10 from the axis, turned: a torus to within the polygon's sag,
    /// exactly where it is near; a quarter turn is never above a dense sampling of its surface.
    #[test]
    fn a_revolve_is_a_torus_and_a_part_turn_a_bound() {
        let regions = vec![vec![ngon([10.0, 0.0], 3.0, 256, true)]];
        let sag = 3.0 * (1.0 - (std::f64::consts::PI / 256.0).cos());
        let full = solid(&regions, Form::Revolve { angle: 360.0 });
        let mut seed = 0x51af1d3c99a3b2e7u64;
        for _ in 0..4000 {
            let q = [-16.0 + 32.0 * rnd(&mut seed), -6.0 + 12.0 * rnd(&mut seed), -16.0 + 32.0 * rnd(&mut seed)];
            let torus = (q[0].hypot(q[2]) - 10.0).hypot(q[1]) - 3.0;
            let d = full.distance(Vec3d::new(q[0], q[1], q[2]));
            if torus < full.outline.reach {
                assert!((d - torus).abs() <= sag + 1e-9, "{d} {torus} at {q:?}");
            } else {
                assert!(d <= torus + sag, "{d} {torus} at {q:?}");
            }
        }
        // a quarter turn: the tube from +X to -Z and its two end discs
        let quarter = solid(&regions, Form::Revolve { angle: 90.0 });
        let step = 0.3;
        let mut surface = Vec::new();
        let rings = (std::f64::consts::FRAC_PI_2 * 13.0 / step).ceil() as usize;
        for k in 0..=rings {
            let a = std::f64::consts::FRAC_PI_2 * k as f64 / rings as f64;
            for i in 0..((std::f64::consts::TAU * 3.0 / step).ceil() as usize) {
                let b = i as f64 * step / 3.0;
                let r = 10.0 + 3.0 * b.cos();
                surface.push([r * a.cos(), 3.0 * b.sin(), -r * a.sin()]);
            }
        }
        for a in [0.0, std::f64::consts::FRAC_PI_2] {
            for i in -10..=10 {
                for j in -10..=10 {
                    let (x, y) = (i as f64 * step, j as f64 * step);
                    if x.hypot(y) < 3.0 {
                        surface.push([(10.0 + x) * f64::cos(a), y, -(10.0 + x) * f64::sin(a)]);
                    }
                }
            }
        }
        for _ in 0..1500 {
            let q = [-16.0 + 32.0 * rnd(&mut seed), -6.0 + 12.0 * rnd(&mut seed), -16.0 + 32.0 * rnd(&mut seed)];
            let d = quarter.distance(Vec3d::new(q[0], q[1], q[2]));
            let e = sampled(&surface, q);
            assert!(d.abs() <= e + sag + 1e-9, "never above the true distance: {d} {e} at {q:?}");
            let within = q[0] > 0.0 && q[2] < 0.0;
            if within && d < 0.0 {
                assert!((q[0].hypot(q[2]) - 10.0).hypot(q[1]) < 3.0 + sag, "inside the tube at {q:?}");
            }
            if !within {
                assert!(d > 0.0, "outside the quarter at {q:?}: {d}");
            }
        }
        assert!(SketchSolid::new(&[vec![ngon([1.0, 0.0], 3.0, 8, true)]], Form::Revolve { angle: 360.0 }).is_err(), "across the axis");
        assert!(SketchSolid::new(&regions, Form::Revolve { angle: 0.0 }).is_err());
        assert_ne!(full.key(), quarter.key());
    }

    /// In a smooth union with a capsule the solid meets it without a seam: along a walk through
    /// the joint the field is 1-Lipschitz, below both where both are near, and the union of them
    /// where the fillet does not reach.
    #[test]
    fn a_sketch_blends_with_a_capsule_without_a_seam() {
        use crate::shape::{distance, SdfNode, SdfOp};
        use std::sync::Arc;
        let (u, v, w) = GROUND;
        let s = Arc::new(solid(&[vec![ngon([0.0, 0.0], 10.0, 64, true)]], Form::Extrude { u, v, w, z0: 0.0, z1: 6.0, draft: 0.0 }));
        let capsule = SdfNode::Capsule { a: [0.0, 6.0, 0.0], b: [0.0, 20.0, 0.0], radius: 2.0 };
        let k = 3.0;
        let blend = SdfNode::Combine { op: SdfOp::Union, blend: k, of: vec![SdfNode::Sketch(s.clone()), capsule.clone()] };
        let mut prev: Option<(P3, f64)> = None;
        let mut filled = 0;
        for i in 0..4000 {
            let t = i as f64 / 4000.0;
            let q = [2.0 + 6.0 * t, 4.0 + 6.0 * (t * 9.0).sin().abs(), 3.0 * (t * 5.0).cos()];
            let p = Vec3d::new(q[0], q[1], q[2]);
            let (a, b, d) = (s.distance(p), distance(&capsule, p), distance(&blend, p));
            assert!(d <= a.min(b) + 1e-12, "a union at {q:?}");
            if (a - b).abs() < k * 0.5 {
                assert!(d < a.min(b) - 1e-6, "the fillet fills at {q:?}: {d} {a} {b}");
                filled += 1;
            }
            if (a - b).abs() >= k {
                assert!((d - a.min(b)).abs() < 1e-12, "beyond the fillet, the union at {q:?}");
            }
            if let Some((o, e)) = prev {
                let step = ((q[0] - o[0]).powi(2) + (q[1] - o[1]).powi(2) + (q[2] - o[2]).powi(2)).sqrt();
                assert!((d - e).abs() <= step + 1e-9, "no seam: 1-Lipschitz at {q:?}");
            }
            prev = Some((q, d));
        }
        assert!(filled > 100, "{filled}");
    }
}
