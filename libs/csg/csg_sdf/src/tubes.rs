//! Tubes: smooth curves through points, each point with its own radius, as one distance field
//! (hair locks, tendrils, cables). A curve is a centripetal Catmull-Rom spline: each span between
//! two points is a cubic Bezier whose knot steps are the square roots of the distances between the
//! points, so it makes no loop or cusp however unevenly they are spaced; the radius runs along it
//! the same way. Each span is cut into parts (one per 10 degrees of turn of its control polygon,
//! 1 to 16) and each part into straight pieces, as many as keep the curve's turn along a piece
//! under a degree (1 to 64). The tube is the union of the round cones (the hulls of two balls)
//! along those pieces: where two pieces meet on the inside of a bend, the crease is under a degree.
//!
//! The field is `max(box, min(reach, nearest))`: exact wherever the surface is nearer than
//! `reach` (twice the widest radius), a lower bound beyond it, and 1-Lipschitz everywhere. The
//! pieces are grouped, near ones together, eight to a group measured side by side; a grid of cells
//! over the box lists for each cell the groups that come within reach of it, nearest first. A
//! query reads its cell's list until the next group is further than the nearest piece so far.
//!
//! With a `blend` k, curves meet in a smooth fillet: each curve's distance (its nearest piece)
//! joins the others nearest first by the quadratic smooth minimum, `m - k h² / 4` with
//! `h = 1 - (d - m) / k` while `d - m < k`. Each step weighs the two slopes between 0 and 1, so it
//! stays 1-Lipschitz; the order only changes where two distances are equal, where it gives the same
//! either way, so it is continuous; and a curve `k` or more beyond the running minimum adds
//! nothing, so the walk reads every piece within the nearest piece plus `k` and stops. The pieces
//! of one curve stay a hard union (they meet under a degree: a fillet there would bead the tube).
//! The fillet adds less than `k` to the solid: the box grows by `k`, and the cells list the groups
//! within `reach + 2k`.

use crate::shape::P3;
use makepad_csg_math::Vec3d;

/// A point of a curve and its radius there: x, y, z, r.
pub type P4 = [f64; 4];

/// One tube: a curve through `pts` (two or more distinct points; one point is a ball).
#[derive(Clone, Debug, PartialEq)]
pub struct TubeCurve {
    pub pts: Vec<P4>,
    /// Material slot (0: the enclosing one).
    pub slot: u8,
}

/// A round cone's distance, worked out once from its two balls: radius `ra` at a and `rb` at b.
#[derive(Clone, Copy, Debug)]
struct Cone {
    /// b - a, 1 / its length and 1 / its length squared.
    ba: P3,
    il: f64,
    il2: f64,
    /// The side's slope across (ny) and along (nx).
    nx: f64,
    ny: f64,
    /// Where on the slant the nearest point moves from the ball at a (below `lo`) onto the side,
    /// and from it onto the ball at b (above `hi`). A cone one of whose balls holds the other is
    /// that ball (a slope of 0, so a slant of 0, and both bounds past it).
    lo: f64,
    hi: f64,
    ra: f64,
    rb: f64,
}

impl Cone {
    /// The distance at q from `p` = q - a and `e` = q - b, and where on the cone it is measured
    /// from: the ball at a (0), at b (1), or the side (2). It has no branch, so eight run side by
    /// side.
    #[inline(always)]
    fn distance(&self, p: P3, e: P3) -> (f64, u8) {
        let along = dot(p, self.ba);
        let h = along * self.il2;
        let x = len([p[0] - self.ba[0] * h, p[1] - self.ba[1] * h, p[2] - self.ba[2] * h]);
        let y = along * self.il;
        let w = self.nx * y - self.ny * x;
        let at_a = w < self.lo;
        let end = if at_a { dot(p, p) } else { dot(e, e) }.sqrt() - if at_a { self.ra } else { self.rb };
        let side = self.nx * x + self.ny * y - self.ra;
        if at_a {
            (end, 0)
        } else if w > self.hi {
            (end, 1)
        } else {
            (side, 2)
        }
    }
}

/// A straight piece of a tube: the round cone a..b.
#[derive(Clone, Copy, Debug)]
struct Piece {
    a: P3,
    b: P3,
    cone: Cone,
    /// A ball holding the piece: its centre and radius.
    mid: P3,
    around: f64,
    /// Its curve.
    curve: u32,
}

/// How many pieces a group holds: a vector's lanes.
const W: usize = 8;

/// A group's pieces side by side, lane by lane (the last piece again in the lanes left over).
#[derive(Clone, Copy, Debug)]
struct Lanes {
    a: [[f64; W]; 3],
    b: [[f64; W]; 3],
    ba: [[f64; W]; 3],
    il: [f64; W],
    il2: [f64; W],
    nx: [f64; W],
    ny: [f64; W],
    lo: [f64; W],
    hi: [f64; W],
    ra: [f64; W],
    rb: [f64; W],
}

/// A group of pieces: `first..first + count`, and a ball holding them all.
#[derive(Clone, Copy, Debug)]
struct Group {
    c: P3,
    r: f64,
    first: u32,
    count: u32,
}

/// The most cells the grid has.
const CELLS: f64 = 262144.0;

/// Cells over the box grown by `reach`, each listing the groups within reach of it.
#[derive(Debug)]
struct Grid {
    lo: P3,
    step: f64,
    n: [usize; 3],
    /// Cell c's entries are `start[c]..start[c + 1]`.
    start: Vec<u32>,
    /// Each entry's group, and how near it comes: no point of the cell is nearer the group's
    /// ball. Nearest first in each cell.
    group: Vec<u32>,
    near: Vec<f64>,
}

/// Tubes as a field.
#[derive(Debug)]
pub struct Tubes {
    pub curves: Vec<TubeCurve>,
    /// How wide the fillet is where two curves meet (0: a hard union).
    pub blend: f64,
    /// The box holding them all.
    pub min: P3,
    pub max: P3,
    /// Exact within this distance of the surface: twice the widest radius.
    pub reach: f64,
    key: [u8; 32],
    /// The pieces, group by group; the groups, and the same side by side.
    pieces: Vec<Piece>,
    groups: Vec<Group>,
    lanes: Vec<Lanes>,
    grid: Grid,
}

fn sub(a: P3, b: P3) -> P3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: P3, b: P3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn len(a: P3) -> f64 {
    dot(a, a).sqrt()
}

fn xyz(p: P4) -> P3 {
    [p[0], p[1], p[2]]
}

fn unit(a: P3) -> P3 {
    let l = len(a).max(1e-30);
    a.map(|x| x / l)
}

fn bezier(q: &[P4; 4], t: f64) -> P4 {
    let u = 1.0 - t;
    let (w0, w1, w2, w3) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
    std::array::from_fn(|i| q[0][i] * w0 + q[1][i] * w1 + q[2][i] * w2 + q[3][i] * w3)
}

fn bezier_tangent(q: &[P4; 4], t: f64) -> P4 {
    let u = 1.0 - t;
    std::array::from_fn(|i| (q[1][i] - q[0][i]) * (3.0 * u * u) + (q[2][i] - q[1][i]) * (6.0 * u * t) + (q[3][i] - q[2][i]) * (3.0 * t * t))
}

/// The knot step across a..b: the square root of the distance between them.
fn knot(a: P4, b: P4) -> f64 {
    len(sub(xyz(b), xyz(a))).max(1e-30).sqrt()
}

/// Span `k` of the curve through `pts` as its Bezier p1, c1, c2, p2 (past an end, the end
/// mirrored).
fn span(pts: &[P4], k: usize) -> [P4; 4] {
    let (p1, p2) = (pts[k], pts[k + 1]);
    let p0 = if k > 0 { pts[k - 1] } else { std::array::from_fn(|i| p1[i] * 2.0 - p2[i]) };
    let p3 = if k + 2 < pts.len() { pts[k + 2] } else { std::array::from_fn(|i| p2[i] * 2.0 - p1[i]) };
    let (k0, k1, k2) = (knot(p0, p1), knot(p1, p2), knot(p2, p3));
    let c1: P4 = std::array::from_fn(|i| p1[i] + ((p1[i] - p0[i]) / k0 - (p2[i] - p0[i]) / (k0 + k1) + (p2[i] - p1[i]) / k1) * (k1 / 3.0));
    let c2: P4 = std::array::from_fn(|i| p2[i] - ((p2[i] - p1[i]) / k1 - (p3[i] - p1[i]) / (k1 + k2) + (p3[i] - p2[i]) / k2) * (k1 / 3.0));
    [p1, c1, c2, p2]
}

/// A span's parts: one per 10 degrees of its control polygon's turn (each turn's chord between
/// unit directions, at least the angle's 2 sin(a / 2)), 1 to 16, each as its own Bezier.
fn parts_of(q: &[P4; 4]) -> Vec<[P4; 4]> {
    let u: [P3; 3] = std::array::from_fn(|i| unit(sub(xyz(q[i + 1]), xyz(q[i]))));
    let turn = len(sub(u[0], u[1])) + len(sub(u[1], u[2]));
    let n = ((turn * 5.729578).ceil() as usize).clamp(1, 16);
    let s = n as f64;
    let third = |d: P4| d.map(|x| x / (s * 3.0));
    let (mut e0, mut d0) = (q[0], third(bezier_tangent(q, 0.0)));
    (0..n)
        .map(|j| {
            let t = (j + 1) as f64 / s;
            let (e1, d1) = (if j + 1 == n { q[3] } else { bezier(q, t) }, third(bezier_tangent(q, t)));
            let part = [e0, std::array::from_fn(|i| e0[i] + d0[i]), std::array::from_fn(|i| e1[i] - d1[i]), e1];
            (e0, d0) = (e1, d1);
            part
        })
        .collect()
}

/// How many straight pieces a part takes for it to turn at most a degree along each. Its
/// derivative is 3 times the quadratic Bezier of its steps h0, h1, h2, its second 6 times the line
/// from h1 - h0 to h2 - h1; so its tangent turns at most 2 max(|h1 - h0|, |h2 - h1|) over the
/// least any step reaches along their sum, per unit of t (1 to 64; 64 when a step turns back).
fn pieces_of(q: &[P4; 4]) -> usize {
    let h: [P3; 3] = std::array::from_fn(|i| sub(xyz(q[i + 1]), xyz(q[i])));
    let m = unit([h[0][0] + h[1][0] + h[2][0], h[0][1] + h[1][1] + h[2][1], h[0][2] + h[1][2] + h[2][2]]);
    let ahead = dot(h[0], m).min(dot(h[1], m)).min(dot(h[2], m));
    let bend = len(sub(h[1], h[0])).max(len(sub(h[2], h[1]))) * 2.0;
    if ahead > 0.0 {
        ((bend / ahead * 57.29578).ceil() as usize).clamp(1, 64)
    } else {
        64
    }
}

fn piece(a: P4, b: P4, curve: u32) -> Piece {
    let (ra, rb) = (a[3].max(0.0), b[3].max(0.0));
    let (a, b) = (xyz(a), xyz(b));
    let ba = sub(b, a);
    let l2 = dot(ba, ba).max(1e-30);
    let l = l2.sqrt();
    let ny = (ra - rb) / l;
    let nx = (1.0 - ny * ny).max(0.0).sqrt();
    let (nx, ny, lo, hi) = if (ra - rb).abs() < l * 0.99609375 {
        (nx, ny, 0.0, nx * l)
    } else if ra >= rb {
        (0.0, 0.0, f64::INFINITY, f64::INFINITY)
    } else {
        (0.0, 0.0, f64::NEG_INFINITY, f64::NEG_INFINITY)
    };
    Piece {
        a,
        b,
        cone: Cone { ba, il: 1.0 / l, il2: 1.0 / l2, nx, ny, lo, hi, ra, rb },
        mid: std::array::from_fn(|i| (a[i] + b[i]) * 0.5),
        around: l * 0.5 + ra.max(rb),
        curve,
    }
}

impl Piece {
    /// The round cone's distance at `q`, and from where on it (see `Cone::distance`).
    fn distance(&self, q: P3) -> (f64, u8) {
        self.cone.distance(sub(q, self.a), sub(q, self.b))
    }

    /// The distance's gradient at `q` from where `distance` measured it.
    fn gradient(&self, q: P3, from: u8) -> P3 {
        let c = &self.cone;
        match from {
            0 => unit(sub(q, self.a)),
            1 => unit(sub(q, self.b)),
            _ => {
                let pa = sub(q, self.a);
                let h = dot(pa, c.ba) * c.il2;
                let across = unit([pa[0] - c.ba[0] * h, pa[1] - c.ba[1] * h, pa[2] - c.ba[2] * h]);
                std::array::from_fn(|i| c.nx * across[i] + c.ny * c.ba[i] * c.il)
            }
        }
    }
}

impl Lanes {
    /// `pieces` (1 to W) side by side.
    fn new(pieces: &[Piece]) -> Lanes {
        let at = |k: usize| &pieces[k.min(pieces.len() - 1)];
        let each = |f: &dyn Fn(&Piece) -> f64| -> [f64; W] { std::array::from_fn(|k| f(at(k))) };
        Lanes {
            a: std::array::from_fn(|i| each(&|p| p.a[i])),
            b: std::array::from_fn(|i| each(&|p| p.b[i])),
            ba: std::array::from_fn(|i| each(&|p| p.cone.ba[i])),
            il: each(&|p| p.cone.il),
            il2: each(&|p| p.cone.il2),
            nx: each(&|p| p.cone.nx),
            ny: each(&|p| p.cone.ny),
            lo: each(&|p| p.cone.lo),
            hi: each(&|p| p.cone.hi),
            ra: each(&|p| p.cone.ra),
            rb: each(&|p| p.cone.rb),
        }
    }

    /// The nearest of the lanes' cones at `q`: its distance and lane.
    fn nearest(&self, q: P3) -> (f64, usize) {
        let d = self.distances(q);
        let (mut m, mut least) = (0, d[0]);
        for (k, &x) in d.iter().enumerate().skip(1) {
            if x < least {
                (m, least) = (k, x);
            }
        }
        (least, m)
    }

    /// Each lane's cone's distance at `q`.
    #[inline(always)]
    fn distances(&self, q: P3) -> [f64; W] {
        let mut d = [0.0f64; W];
        for (k, d) in d.iter_mut().enumerate() {
            let cone = Cone {
                ba: [self.ba[0][k], self.ba[1][k], self.ba[2][k]],
                il: self.il[k],
                il2: self.il2[k],
                nx: self.nx[k],
                ny: self.ny[k],
                lo: self.lo[k],
                hi: self.hi[k],
                ra: self.ra[k],
                rb: self.rb[k],
            };
            let p = [q[0] - self.a[0][k], q[1] - self.a[1][k], q[2] - self.a[2][k]];
            let e = [q[0] - self.b[0][k], q[1] - self.b[1][k], q[2] - self.b[2][k]];
            *d = cone.distance(p, e).0;
        }
        d
    }
}

/// Each curve's nearest piece so far: its curve, distance and piece; sixteen in place, any more
/// in `more`.
struct Near {
    n: usize,
    at: [(u32, f64, u32); 16],
    more: Vec<(u32, f64, u32)>,
}

impl Near {
    fn new() -> Near {
        Near { n: 0, at: [(0, 0.0, 0); 16], more: Vec::new() }
    }

    fn add(&mut self, curve: u32, d: f64, piece: u32) {
        let (n, at, more) = (self.n, &mut self.at, &mut self.more);
        if let Some(e) = at[..n].iter_mut().chain(more.iter_mut()).find(|e| e.0 == curve) {
            if d < e.1 {
                (e.1, e.2) = (d, piece);
            }
        } else if n < at.len() {
            at[n] = (curve, d, piece);
            self.n += 1;
        } else {
            more.push((curve, d, piece));
        }
    }

    /// Nearest first.
    fn sorted(&mut self) -> &[(u32, f64, u32)] {
        let all = if self.more.is_empty() {
            &mut self.at[..self.n]
        } else {
            self.more.extend_from_slice(&self.at[..self.n]);
            &mut self.more[..]
        };
        all.sort_unstable_by(|a, b| a.1.total_cmp(&b.1));
        all
    }
}

/// A curve's pieces in order along it: a point the same as the one before adds nothing; one
/// point is a ball.
fn curve_pieces(c: &TubeCurve, curve: u32) -> Vec<Piece> {
    let mut pts = c.pts.clone();
    pts.dedup_by(|b, a| xyz(*a) == xyz(*b));
    let beziers: Vec<[P4; 4]> = match pts.len() {
        0 => Vec::new(),
        1 => return vec![piece(pts[0], pts[0], curve)],
        n => (0..n - 1).flat_map(|k| parts_of(&span(&pts, k))).collect(),
    };
    let mut out = Vec::new();
    for q in beziers {
        let n = pieces_of(&q);
        let mut prev = q[0];
        for j in 1..=n {
            let cur = if j == n { q[3] } else { bezier(&q, j as f64 / n as f64) };
            out.push(piece(prev, cur, curve));
            prev = cur;
        }
    }
    out
}

/// The least ball holding two balls.
fn hold(a: (P3, f64), b: (P3, f64)) -> (P3, f64) {
    let d = len(sub(b.0, a.0));
    if d + b.1 <= a.1 {
        return a;
    }
    if d + a.1 <= b.1 {
        return b;
    }
    let r = (d + a.1 + b.1) * 0.5;
    let t = (r - a.1) / d;
    (std::array::from_fn(|i| a.0[i] + (b.0[i] - a.0[i]) * t), r)
}

/// `pieces` (reordered in place; they start at `offset`) in groups of W, near ones together:
/// halved, in whole groups, at the middle along the widest spread of the pieces' middles.
fn group(groups: &mut Vec<Group>, pieces: &mut [Piece], offset: u32) {
    if pieces.len() <= W {
        let (c, r) = pieces.iter().map(|p| (p.mid, p.around)).reduce(hold).expect("a group holds a piece");
        groups.push(Group { c, r, first: offset, count: pieces.len() as u32 });
        return;
    }
    let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    for p in pieces.iter() {
        for k in 0..3 {
            lo[k] = lo[k].min(p.mid[k]);
            hi[k] = hi[k].max(p.mid[k]);
        }
    }
    let axis = (0..3).max_by(|&i, &j| (hi[i] - lo[i]).total_cmp(&(hi[j] - lo[j]))).expect("three axes");
    let half = pieces.len().div_ceil(W) / 2 * W;
    pieces.select_nth_unstable_by(half, |a, b| a.mid[axis].total_cmp(&b.mid[axis]));
    let (left, right) = pieces.split_at_mut(half);
    group(groups, left, offset);
    group(groups, right, offset + half as u32);
}

/// The grid over the box `min..max` grown by `reach`: cells half the reach across (larger if
/// there would be more than CELLS of them), each listing every group whose ball comes within
/// reach of some point of the cell, nearest first.
fn grid(min: P3, max: P3, reach: f64, groups: &[Group]) -> Grid {
    let lo = min.map(|x| x - reach);
    let size: P3 = std::array::from_fn(|k| max[k] + reach - lo[k]);
    let step = (reach * 0.5).max((size[0] * size[1] * size[2] / CELLS).cbrt());
    let n: [usize; 3] = std::array::from_fn(|k| ((size[k] / step).ceil() as usize).max(1));
    let half = step * 3f64.sqrt() * 0.5;
    let mut found: Vec<(u32, u32, f64)> = Vec::new();
    for (i, g) in groups.iter().enumerate() {
        let span = reach + g.r + half;
        let range: [(usize, usize); 3] = std::array::from_fn(|k| {
            let at = |x: f64| (((x - lo[k]) / step).floor().max(0.0) as usize).min(n[k] - 1);
            (at(g.c[k] - span), at(g.c[k] + span))
        });
        for z in range[2].0..=range[2].1 {
            for y in range[1].0..=range[1].1 {
                for x in range[0].0..=range[0].1 {
                    let centre = [lo[0] + (x as f64 + 0.5) * step, lo[1] + (y as f64 + 0.5) * step, lo[2] + (z as f64 + 0.5) * step];
                    let near = len(sub(centre, g.c)) - g.r - half;
                    if near < reach {
                        found.push((((z * n[1] + y) * n[0] + x) as u32, i as u32, near));
                    }
                }
            }
        }
    }
    let cells = n[0] * n[1] * n[2];
    let mut start = vec![0u32; cells + 1];
    for f in &found {
        start[f.0 as usize + 1] += 1;
    }
    let mut sum = 0;
    for s in start.iter_mut() {
        sum += *s;
        *s = sum;
    }
    let mut fill = start.clone();
    let mut entries = vec![(0u32, 0.0f64); found.len()];
    for f in &found {
        let at = &mut fill[f.0 as usize];
        entries[*at as usize] = (f.1, f.2);
        *at += 1;
    }
    for w in start.windows(2) {
        entries[w[0] as usize..w[1] as usize].sort_unstable_by(|a, b| a.1.total_cmp(&b.1));
    }
    Grid { lo, step, n, start, group: entries.iter().map(|e| e.0).collect(), near: entries.iter().map(|e| e.1).collect() }
}

impl Tubes {
    /// The tubes along `curves`, a fillet `blend` wide (0 or more) where two of them meet.
    pub fn new(curves: Vec<TubeCurve>, blend: f64) -> Tubes {
        // the canonical bytes: every curve's slot, point count and points
        let mut bytes: Vec<u8> = Vec::new();
        let mut pieces = Vec::new();
        let mut widest = 0.0f64;
        for (ci, c) in curves.iter().enumerate() {
            bytes.push(c.slot);
            bytes.extend_from_slice(&(c.pts.len() as u64).to_le_bytes());
            for p in &c.pts {
                p.iter().for_each(|x| bytes.extend_from_slice(&x.to_bits().to_le_bytes()));
                widest = widest.max(p[3]);
            }
            pieces.extend(curve_pieces(c, ci as u32));
        }
        bytes.extend_from_slice(&blend.to_bits().to_le_bytes());
        let key = makepad_core_util::sha256::sha256(&bytes);
        let reach = (2.0 * widest).max(1e-3);
        let (mut min, mut max) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
        for p in &pieces {
            for k in 0..3 {
                min[k] = min[k].min(p.a[k] - p.cone.ra).min(p.b[k] - p.cone.rb);
                max[k] = max[k].max(p.a[k] + p.cone.ra).max(p.b[k] + p.cone.rb);
            }
        }
        let mut groups = Vec::with_capacity(pieces.len().div_ceil(W));
        if pieces.is_empty() {
            min = [0.0; 3];
            max = [0.0; 3];
        } else {
            group(&mut groups, &mut pieces, 0);
            // the fillet adds less than its width
            min = min.map(|x| x - blend);
            max = max.map(|x| x + blend);
        }
        let lanes = groups.iter().map(|g| Lanes::new(&pieces[g.first as usize..(g.first + g.count) as usize])).collect();
        let grid = grid(min, max, reach + 2.0 * blend, &groups);
        Tubes { curves, blend, min, max, reach, key, pieces, groups, lanes, grid }
    }

    /// The tubes' identity: the sha256 of their canonical bytes (points, radii, slots).
    pub fn key(&self) -> [u8; 32] {
        self.key
    }

    /// How many straight pieces make them.
    pub fn piece_count(&self) -> usize {
        self.pieces.len()
    }

    /// Signed distance and the nearest tube's slot.
    pub fn distance_slot(&self, p: Vec3d) -> (f64, u8) {
        if self.blend > 0.0 {
            let (d, nearest, _) = self.blended([p.x, p.y, p.z], false);
            return (d, nearest.map_or(0, |i| self.curves[self.pieces[i as usize].curve as usize].slot));
        }
        match self.nearest([p.x, p.y, p.z]) {
            (d, Some(i)) => (d, self.curves[self.pieces[i as usize].curve as usize].slot),
            (d, None) => (d, 0),
        }
    }

    /// Signed distance and its gradient: away from the nearest piece where that decides, else the
    /// box's (0 where only the reach does).
    pub fn distance_gradient(&self, p: Vec3d) -> (f64, P3) {
        let q = [p.x, p.y, p.z];
        if self.blend > 0.0 {
            let (d, _, g) = self.blended(q, true);
            return (d, g);
        }
        match self.nearest(q) {
            (d, Some(i)) => {
                let piece = &self.pieces[i as usize];
                (d, piece.gradient(q, piece.distance(q).1))
            }
            (d, None) => {
                let (d_box, out) = self.outside(q);
                if d_box > 0.0 && d >= d_box {
                    (d, std::array::from_fn(|k| if q[k] < self.min[k] { -out[k] / d_box } else { out[k] / d_box }))
                } else {
                    (d, [0.0; 3])
                }
            }
        }
    }

    pub fn distance(&self, p: Vec3d) -> f64 {
        if self.blend > 0.0 {
            return self.blended([p.x, p.y, p.z], false).0;
        }
        self.nearest([p.x, p.y, p.z]).0
    }

    /// [`Tubes::distance`] clamped to `lower..=upper`, bit for bit: with no fillet no group past
    /// `upper` is read (a point whose groups all are costs its cell's first test).
    pub fn distance_bounded(&self, p: Vec3d, lower: f64, upper: f64) -> f64 {
        if self.blend > 0.0 {
            return self.distance(p).clamp(lower, upper);
        }
        self.nearest_under([p.x, p.y, p.z], self.reach.min(upper)).0.clamp(lower, upper)
    }

    /// How far outside the box q is, and by how much along each axis.
    fn outside(&self, q: P3) -> (f64, P3) {
        let out: P3 = std::array::from_fn(|k| (self.min[k] - q[k]).max(q[k] - self.max[k]).max(0.0));
        (len(out), out)
    }

    /// The signed distance at q, and the piece it comes from (none where the box or the reach
    /// gives it).
    fn nearest(&self, q: P3) -> (f64, Option<u32>) {
        self.nearest_under(q, self.reach)
    }

    /// [`Tubes::nearest`] where it is under `cap` (at most the reach), else at least `cap`: no
    /// piece at `cap` or past it is read, and those under it are all.
    fn nearest_under(&self, q: P3, cap: f64) -> (f64, Option<u32>) {
        // outside the box: its distance is a lower bound everywhere
        let (d_box, _) = self.outside(q);
        if d_box >= self.reach {
            return (d_box, None);
        }
        // the cell's groups nearest first, until the next is no nearer than the nearest piece:
        // every surface within reach of q is found, and nothing further is looked at
        let g = &self.grid;
        let cell = |k: usize| (((q[k] - g.lo[k]) / g.step) as usize).min(g.n[k] - 1);
        let c = (cell(2) * g.n[1] + cell(1)) * g.n[0] + cell(0);
        let mut best = cap;
        let mut found = None;
        for e in g.start[c] as usize..g.start[c + 1] as usize {
            if g.near[e] >= best {
                break;
            }
            let i = g.group[e] as usize;
            let group = &self.groups[i];
            // the group's ball no nearer than the nearest piece: |q - c| - r >= best
            let t = best + group.r;
            let o = sub(q, group.c);
            if t <= 0.0 || dot(o, o) >= t * t {
                continue;
            }
            let (d, lane) = self.lanes[i].nearest(q);
            #[cfg(test)]
            crate::mesh_field::work::add(u64::from(group.count));
            if d < best {
                best = d;
                found = Some(group.first + (lane as u32).min(group.count - 1));
            }
        }
        match found {
            // (the box holds every tube: its distance never passes the nearest piece's but by rounding)
            _ if d_box > 0.0 && d_box >= best => (d_box, None),
            Some(i) => (best, Some(i)),
            None => (cap, None),
        }
    }

    /// The box's distance at q and, with `slope`, its gradient (0 inside).
    fn box_at(&self, q: P3, slope: bool) -> (f64, P3) {
        let (d_box, out) = self.outside(q);
        let g = if slope && d_box > 0.0 { std::array::from_fn(|k| if q[k] < self.min[k] { -out[k] / d_box } else { out[k] / d_box }) } else { [0.0; 3] };
        (d_box, g)
    }

    /// The blended field at q: `max(box, min(reach, the curves' smooth minimum))`, the piece
    /// nearest q where the curves decide it, and with `slope` its gradient.
    fn blended(&self, q: P3, slope: bool) -> (f64, Option<u32>, P3) {
        let k = self.blend;
        let (d_box, g_box) = self.box_at(q, slope);
        if d_box >= self.reach {
            return (d_box, None, g_box);
        }
        // every curve within the nearest piece plus k, each by its nearest piece: from a nearest
        // piece at reach + k or more the minimum is reach or more
        let g = &self.grid;
        let cell = |a: usize| (((q[a] - g.lo[a]) / g.step) as usize).min(g.n[a] - 1);
        let c = (cell(2) * g.n[1] + cell(1)) * g.n[0] + cell(0);
        let mut best = self.reach + k;
        let mut near = Near::new();
        for e in g.start[c] as usize..g.start[c + 1] as usize {
            let cut = best + k;
            if g.near[e] >= cut {
                break;
            }
            let i = g.group[e] as usize;
            let group = &self.groups[i];
            let t = cut + group.r;
            let o = sub(q, group.c);
            if t <= 0.0 || dot(o, o) >= t * t {
                continue;
            }
            let d = self.lanes[i].distances(q);
            for (lane, &x) in d.iter().enumerate().take(group.count as usize) {
                if x < cut {
                    let piece = group.first + lane as u32;
                    near.add(self.pieces[piece as usize].curve, x, piece);
                    best = best.min(x);
                }
            }
        }
        if best >= self.reach + k {
            return (self.reach, None, [0.0; 3]);
        }
        let list = near.sorted();
        let piece_slope = |piece: u32| {
            let p = &self.pieces[piece as usize];
            p.gradient(q, p.distance(q).1)
        };
        let (_, mut m, first) = list[0];
        let mut grad = if slope { piece_slope(first) } else { [0.0; 3] };
        for &(_, d, piece) in &list[1..] {
            if d - m >= k {
                break;
            }
            let h = 1.0 - (d - m) / k;
            m -= 0.25 * k * h * h;
            if slope {
                let gd = piece_slope(piece);
                grad = std::array::from_fn(|i| (1.0 - 0.5 * h) * grad[i] + 0.5 * h * gd[i]);
            }
        }
        match m {
            // (the box holds the fillets: its distance never passes theirs but by rounding)
            _ if d_box > 0.0 && d_box >= m.min(self.reach) => (d_box, None, g_box),
            m if m < self.reach => (m, Some(first), grad),
            _ => (self.reach, None, [0.0; 3]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A lock that bends unevenly, its radius swelling and narrowing.
    const LOCK: [P4; 5] = [[0.0, 0.0, 0.0, 3.0], [4.0, 10.0, 2.0, 4.0], [-3.0, 18.0, 5.0, 2.5], [2.0, 26.0, -4.0, 3.5], [10.0, 30.0, 0.0, 1.5]];

    /// The centripetal Catmull-Rom span at s as Barry and Goldman's pyramid of lerps over the
    /// knots: the reference the Bezier form follows.
    fn catmull(pts: &[P4], k: usize, s: f64) -> P4 {
        let (p1, p2) = (pts[k], pts[k + 1]);
        let p0 = if k > 0 { pts[k - 1] } else { std::array::from_fn(|i| p1[i] * 2.0 - p2[i]) };
        let p3 = if k + 2 < pts.len() { pts[k + 2] } else { std::array::from_fn(|i| p2[i] * 2.0 - p1[i]) };
        let lerp = |a: P4, b: P4, s: f64| -> P4 { std::array::from_fn(|i| a[i] + (b[i] - a[i]) * s) };
        let t1 = knot(p0, p1);
        let t2 = t1 + knot(p1, p2);
        let t3 = t2 + knot(p2, p3);
        let t = t1 + (t2 - t1) * s;
        let a1 = lerp(p0, p1, t / t1);
        let a2 = lerp(p1, p2, (t - t1) / (t2 - t1));
        let a3 = lerp(p2, p3, (t - t2) / (t3 - t2));
        let b1 = lerp(a1, a2, t / t2);
        let b2 = lerp(a2, a3, (t - t1) / (t3 - t1));
        lerp(b1, b2, (t - t1) / (t2 - t1))
    }

    /// Every piece in a brute-force scan, from the reference curve's own pieces.
    fn brute(t: &Tubes, q: P3) -> f64 {
        t.pieces.iter().map(|p| p.distance(q).0).fold(f64::INFINITY, f64::min)
    }

    /// A tube of one point is one ball: one piece, not 64 of no length.
    #[test]
    fn a_one_point_tube_is_one_ball() {
        let t = Tubes::new(vec![TubeCurve { pts: vec![[1.0, 2.0, 3.0, 0.5]; 2], slot: 0 }], 0.0);
        assert_eq!(t.pieces.len(), 1);
        assert!((t.distance(Vec3d::new(1.0, 2.0, 5.0)) - 1.5).abs() < 1e-9, "{}", t.distance(Vec3d::new(1.0, 2.0, 5.0)));
    }

    /// Its pieces meet at under a degree; it passes through every point at that point's radius
    /// and follows the reference curve to within the pieces' sag; near the surface it is the
    /// nearest piece exactly, further out a lower bound, and 1-Lipschitz along a long walk.
    #[test]
    fn a_tube_follows_its_curve_exactly_near_and_lipschitz_far() {
        let tubes = Tubes::new(vec![TubeCurve { pts: LOCK.to_vec(), slot: 2 }, TubeCurve { pts: vec![[30.0, 0.0, 0.0, 1.0], [30.0, 12.0, 0.0, 0.5]], slot: 0 }], 0.0);
        let lock = curve_pieces(&tubes.curves[0], 0);
        let mut most = 0.0f64;
        for w in lock.windows(2) {
            let (a, b) = (unit(w[0].cone.ba), unit(w[1].cone.ba));
            most = most.max(dot(a, b).clamp(-1.0, 1.0).acos().to_degrees());
        }
        assert!(most < 1.0, "pieces meet at {most} degrees");
        for k in 0..LOCK.len() - 1 {
            for i in 0..=8 {
                let c = catmull(&LOCK, k, i as f64 / 8.0);
                let at = Vec3d::new(c[0], c[1], c[2]);
                // on the axis: as deep as the radius there, to within the sag of a piece
                let d = tubes.distance(at);
                assert!((d + c[3]).abs() < 0.01, "span {k} at {i}: {d} against -{}", c[3]);
            }
        }
        let mut prev: Option<(P3, f64)> = None;
        for k in 0..6000 {
            let s = k as f64 * 0.013;
            let q = [-20.0 + s * 0.9, 15.0 + 22.0 * (s * 0.41).sin(), 12.0 * (s * 0.67).cos()];
            let d = tubes.distance(Vec3d::new(q[0], q[1], q[2]));
            let e = brute(&tubes, q);
            assert!(d <= e + 1e-9, "never above the true distance: {d} {e} at {q:?}");
            if e < tubes.reach {
                assert!((d - e).abs() < 1e-9, "exact near the surface: {d} {e} at {q:?}");
            }
            if let Some((p, dp)) = prev {
                assert!((d - dp).abs() <= len(sub(q, p)) + 1e-9, "1-Lipschitz at {q:?}");
            }
            prev = Some((q, d));
        }
        // the gradient is the distance's slope
        let q = Vec3d::new(6.0, 12.0, 4.0);
        let (d, g) = tubes.distance_gradient(q);
        let h = 1e-5;
        let fd = [Vec3d::new(h, 0.0, 0.0), Vec3d::new(0.0, h, 0.0), Vec3d::new(0.0, 0.0, h)].map(|e| (tubes.distance(q + e) - d) / h);
        assert!((0..3).all(|i| (g[i] - fd[i]).abs() < 1e-3), "{g:?} {fd:?}");
        assert_eq!(tubes.distance_slot(Vec3d::new(0.0, 0.0, 0.0)).1, 2);
        assert_ne!(tubes.key(), Tubes::new(vec![TubeCurve { pts: LOCK[..4].to_vec(), slot: 2 }], 0.0).key());
        assert_ne!(tubes.key(), Tubes::new(tubes.curves.clone(), 1.0).key());
    }

    /// Many locks side by side, some touching: at points all through and round them, the field
    /// is the nearest of all the pieces wherever that is within reach, never above it, and its
    /// slot and gradient are the nearest piece's.
    #[test]
    fn many_tubes_give_the_nearest_piece_everywhere_within_reach() {
        let tubes = Tubes::new(locks(), 0.0);
        let mut rnd = rnd();
        let (lo, hi) = (tubes.min.map(|x| x - 8.0), tubes.max.map(|x| x + 8.0));
        let mut near = 0;
        for _ in 0..20000 {
            let q: P3 = std::array::from_fn(|k| lo[k] + (hi[k] - lo[k]) * rnd());
            let p = Vec3d::new(q[0], q[1], q[2]);
            let d = tubes.distance(p);
            let (e, i) = tubes.pieces.iter().enumerate().map(|(i, x)| (x.distance(q).0, i)).fold((f64::INFINITY, 0), |a, b| if b.0 < a.0 { b } else { a });
            assert!(d <= e + 1e-9, "never above the nearest piece: {d} {e} at {q:?}");
            if e < tubes.reach {
                near += 1;
                assert!((d - e).abs() < 1e-9, "the nearest piece within reach: {d} {e} at {q:?}");
                let piece = &tubes.pieces[i];
                let (s, g) = (tubes.distance_slot(p).1, tubes.distance_gradient(p).1);
                // (a tie between pieces of two curves may name either)
                if tubes.pieces.iter().all(|x| x.curve == piece.curve || x.distance(q).0 > e + 1e-9) {
                    assert_eq!(s, tubes.curves[piece.curve as usize].slot, "slot at {q:?}");
                }
                let want = piece.gradient(q, piece.distance(q).1);
                assert!(len(sub(g, want)) < 1e-6 || tubes.pieces.iter().filter(|x| x.distance(q).0 < e + 1e-9).count() > 1, "gradient at {q:?}: {g:?} {want:?}");
            }
        }
        assert!(near > 4000, "{near} points within reach");
    }

    /// Fourteen locks fanned round a crown, some touching.
    fn locks() -> Vec<TubeCurve> {
        (0..14)
            .map(|i| {
                let a = i as f64 * 0.45;
                let pts = (0..5).map(|k| {
                    let t = k as f64 / 4.0;
                    let out = 15.0 + 7.0 * t;
                    let e = (60.0 - 90.0 * t).to_radians();
                    [out * e.cos() * a.cos(), out * e.sin(), out * e.cos() * a.sin(), 3.0 - 2.4 * t]
                });
                TubeCurve { pts: pts.collect(), slot: (i % 3) as u8 }
            })
            .collect()
    }

    /// Numbers in 0..1, the same every run.
    fn rnd() -> impl FnMut() -> f64 {
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 11) as f64 / (1u64 << 53) as f64
        }
    }

    /// The quadratic smooth minimum of a and b, k wide.
    fn smin(a: f64, b: f64, k: f64) -> f64 {
        let (m, d) = (a.min(b), a.max(b));
        if d - m >= k {
            return m;
        }
        let h = 1.0 - (d - m) / k;
        m - 0.25 * k * h * h
    }

    /// The blend by brute force: each curve's nearest piece of all its pieces, joined nearest
    /// first.
    fn brute_blend(t: &Tubes, q: P3) -> f64 {
        let mut each = vec![f64::INFINITY; t.curves.len()];
        for p in &t.pieces {
            each[p.curve as usize] = each[p.curve as usize].min(p.distance(q).0);
        }
        each.sort_by(f64::total_cmp);
        each[1..].iter().fold(each[0], |m, &d| smin(m, d, t.blend))
    }

    /// Two parallel tubes of radius 1, a radius apart, blended: round them the field is the
    /// quadratic smooth minimum of the two cylinders' distances exactly, so the groove between
    /// them fills as far as the fillet's width puts it and no further (3 wide: its floor 0.9014
    /// up, against tops at 1); unblended, the gap stays open.
    #[test]
    fn blended_tubes_meet_in_the_smooth_minimum_of_their_distances() {
        let pair = |k: f64| {
            let line = |x: f64, slot: u8| TubeCurve { pts: vec![[x, -20.0, 0.0, 1.0], [x, 20.0, 0.0, 1.0]], slot };
            Tubes::new(vec![line(-1.5, 1), line(1.5, 2)], k)
        };
        for k in [0.5, 2.0, 3.0] {
            let tubes = pair(k);
            let mut seen = 0;
            for x in -40..=40 {
                for z in -30..=30 {
                    for y in [-5.0, 0.0, 7.0] {
                        let q = [x as f64 * 0.1, y, z as f64 * 0.1];
                        let (d1, d2) = ((q[0] + 1.5).hypot(q[2]) - 1.0, (q[0] - 1.5).hypot(q[2]) - 1.0);
                        let want = smin(d1, d2, k);
                        let d = tubes.distance(Vec3d::new(q[0], q[1], q[2]));
                        if want < tubes.reach {
                            seen += 1;
                            assert!((d - want).abs() < 1e-9, "k {k} at {q:?}: {d} against {want}");
                        } else {
                            assert!(d <= want + 1e-9, "k {k} at {q:?}: {d} over {want}");
                        }
                    }
                }
            }
            assert!(seen > 10000, "{seen}");
        }
        let floor = (1.75f64 * 1.75 - 1.5 * 1.5).sqrt();
        assert!((floor - 0.9014).abs() < 1e-4);
        let three = pair(3.0);
        assert!(three.distance(Vec3d::new(0.0, 0.0, floor)).abs() < 1e-9, "the groove's floor");
        assert!((three.distance(Vec3d::new(0.0, 0.0, 0.0)) + 0.25).abs() < 1e-9, "bridged");
        assert!((pair(0.0).distance(Vec3d::new(0.0, 0.0, 0.0)) - 0.5).abs() < 1e-9, "open");
    }

    /// A blend joins curves, never a curve's own pieces: one lock is the same blended or not.
    #[test]
    fn a_blend_leaves_one_curve_as_it_is() {
        let (one, blended) = (Tubes::new(vec![TubeCurve { pts: LOCK.to_vec(), slot: 0 }], 0.0), Tubes::new(vec![TubeCurve { pts: LOCK.to_vec(), slot: 0 }], 2.0));
        let mut rnd = rnd();
        for _ in 0..5000 {
            let q: P3 = std::array::from_fn(|k| one.min[k] - 3.0 + (one.max[k] - one.min[k] + 6.0) * rnd());
            let p = Vec3d::new(q[0], q[1], q[2]);
            if brute(&one, q) < one.reach {
                assert!((one.distance(p) - blended.distance(p)).abs() < 1e-12, "at {q:?}");
            }
        }
    }

    /// Many locks blended: wherever it is within reach the field is the brute-force blend of
    /// every curve's nearest piece, never above it elsewhere, 1-Lipschitz along a long walk
    /// through them, and its gradient is its slope.
    #[test]
    fn blended_locks_are_the_smooth_minimum_everywhere_within_reach() {
        let tubes = Tubes::new(locks(), 2.0);
        let mut rnd = rnd();
        let (lo, hi) = (tubes.min.map(|x| x - 6.0), tubes.max.map(|x| x + 6.0));
        let (mut near, mut joined) = (0, 0);
        for _ in 0..20000 {
            let q: P3 = std::array::from_fn(|k| lo[k] + (hi[k] - lo[k]) * rnd());
            let d = tubes.distance(Vec3d::new(q[0], q[1], q[2]));
            let e = brute_blend(&tubes, q);
            assert!(d <= e + 1e-9, "never above the blend: {d} {e} at {q:?}");
            if e < tubes.reach {
                near += 1;
                assert!((d - e).abs() < 1e-9, "the blend within reach: {d} {e} at {q:?}");
                if e < brute(&tubes, q) - 1e-6 {
                    joined += 1;
                }
            }
        }
        assert!(near > 4000 && joined > 500, "{near} points within reach, {joined} in a fillet");
        let mut prev: Option<(P3, f64)> = None;
        for k in 0..8000 {
            let s = k as f64 * 0.011;
            let q = [17.0 * (s * 0.37).cos(), 12.0 + 9.0 * (s * 0.53).sin(), 17.0 * (s * 0.29).sin()];
            let d = tubes.distance(Vec3d::new(q[0], q[1], q[2]));
            if let Some((p, dp)) = prev {
                assert!((d - dp).abs() <= len(sub(q, p)) + 1e-9, "1-Lipschitz at {q:?}");
            }
            prev = Some((q, d));
        }
        // the gradient where two locks' fillet decides (off the pieces' creases)
        let (mut checked, mut slope) = (0, 0);
        for _ in 0..4000 {
            let q: P3 = std::array::from_fn(|k| lo[k] + (hi[k] - lo[k]) * rnd());
            let p = Vec3d::new(q[0], q[1], q[2]);
            let (d, g) = tubes.distance_gradient(p);
            if !(d < tubes.reach - 0.1 && brute_blend(&tubes, q) < brute(&tubes, q) - 0.05) {
                continue;
            }
            let h = 1e-6;
            let fd = [Vec3d::new(h, 0.0, 0.0), Vec3d::new(0.0, h, 0.0), Vec3d::new(0.0, 0.0, h)].map(|e| (tubes.distance(p + e) - tubes.distance(p - e)) / (2.0 * h));
            checked += 1;
            if (0..3).all(|i| (g[i] - fd[i]).abs() < 1e-4) {
                slope += 1;
            }
        }
        assert!(checked > 50 && slope * 10 >= checked * 9, "{slope} of {checked}");
    }

    /// One point is a ball; two are a straight round cone.
    #[test]
    fn a_point_is_a_ball_and_two_a_cone() {
        let ball = Tubes::new(vec![TubeCurve { pts: vec![[1.0, 2.0, 3.0, 2.0]], slot: 0 }], 0.0);
        assert!((ball.distance(Vec3d::new(1.0, 5.0, 3.0)) - 1.0).abs() < 1e-12);
        let cone = Tubes::new(vec![TubeCurve { pts: vec![[0.0, 0.0, 0.0, 2.0], [0.0, 10.0, 0.0, 1.0]], slot: 0 }], 0.0);
        assert_eq!(cone.piece_count(), 1);
        assert!((cone.distance(Vec3d::new(0.0, 5.0, 0.0)) + 1.5).abs() < 0.01);
    }
}
