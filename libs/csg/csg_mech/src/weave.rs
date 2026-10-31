//! Interlocking structures printed in place: chains along a path, chainmail
//! fabrics clipped to a region, draped over a surface, coloured per link.
//!
//! Every link is a closed centreline polyline swept by a ball of radius `r`
//! (a chain of capsules). That one description is the printed solid (clipped
//! at the bed), the field the checks read and the box3d collision shape, so the
//! sim collides exactly what prints. Links are free rigid bodies held together
//! by contact only: no joints.
//!
//! The link ("arch square"): a square of half size `a`, corners and the two
//! sides along y on the bed, the two sides along x raised as bridges on legs
//! that rise at the leg angle. Links sit on a checkerboard lattice of spacing
//! `s` (centres at (i s, j s), i + j even); each one interlocks with its four
//! diagonal neighbours: at every overlapped corner one link's bridge passes
//! over the other's low side and the other's bridge over its low side, so each
//! pair has linking number +-1. A chain is the same lattice along one diagonal.
//!
//! Local weave frame: bed = xy, up = z. [`Frame`] maps it to the model frame.

use crate::geom::*;
use crate::joint::Profile;
use crate::mass::MassProps;

/// Sizing rules of a weave, from the printer profile.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WeaveProfile {
    /// Bar radius, mm.
    pub r: f64,
    /// In-plane clearance, mm.
    pub c_xy: f64,
    /// Clearance across layers, mm (whole layers).
    pub c_z: f64,
    pub layer: f64,
    /// Leg angle above the bed (45 deg: the steepest overhang left unsupported).
    pub leg: f64,
    /// Longest unsupported bridge, mm.
    pub bridge_max: f64,
    /// How far the bed bars sink into the bed (fraction of r): a flat foot to print on.
    pub foot: f64,
}

impl WeaveProfile {
    pub fn new(p: &Profile, r: Option<f64>) -> WeaveProfile {
        WeaveProfile {
            r: r.unwrap_or((2.0 * p.nozzle).max(0.6)),
            c_xy: p.c_xy(),
            c_z: p.c_z(),
            layer: p.layer,
            leg: 45f64.to_radians(),
            bridge_max: 10.0,
            foot: 0.25,
        }
    }
}

/// The arch-square link sized from a profile.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SquareLink {
    pub r: f64,
    /// Centreline half size along x (the bridges' direction).
    pub a: f64,
    /// Centreline half size along y (the bed bars' direction); `a` for a square link.
    pub b: f64,
    /// How far two interlocked neighbours overlap along each axis: neighbours of half sizes (a1, b1) and
    /// (a2, b2) sit (a1 + a2 - w, b1 + b2 - w) apart.
    pub w: f64,
    /// Lattice spacing (diagonal neighbours at (+-s, +-s)).
    pub s: f64,
    /// Where the bridge ends and the legs start (|x| <= x0 is high).
    pub x0: f64,
    /// Centreline heights of the bed bars and the bridges.
    pub lo: f64,
    pub hi: f64,
}

impl SquareLink {
    /// The smallest link that keeps every designed clearance (`a` may be larger).
    pub fn new(wp: &WeaveProfile, a: Option<f64>) -> SquareLink {
        let r = wp.r;
        let lo = r * (1.0 - wp.foot);
        let rise = 2.0 * r + wp.c_z;
        let hi = lo + rise;
        let run = rise / wp.leg.tan();
        // overlap of two diagonal neighbours: the parallel bed bars apart by 2r + c_xy, plus the leg's run
        let w = 2.0 * r + wp.c_xy + run;
        // links two apart on a row: their bars apart by c_xy
        let a_min = w + r + 0.5 * wp.c_xy;
        let a = a.unwrap_or(a_min).max(a_min);
        let s = 2.0 * a - w;
        SquareLink { r, a, b: a, w, s, x0: a - run, lo, hi }
    }

    /// The smallest half size that keeps the clearances to the links two apart.
    pub fn a_min(&self) -> f64 {
        let run = self.a - self.x0;
        // w = 2r + c_xy + run and a_min = w + r + c_xy / 2
        self.w + self.r + 0.5 * (self.w - 2.0 * self.r - run)
    }

    /// The same link (bar, clearances, legs) with half sizes `a` along x and `b` along y, each at least
    /// [`SquareLink::a_min`]. `s` stays the spacing to an identical neighbour.
    pub fn sized(&self, a: f64, b: f64) -> SquareLink {
        let run = self.a - self.x0;
        let a = a.max(self.a_min());
        let b = b.max(self.a_min());
        SquareLink { a, b, s: 2.0 * a - self.w, x0: a - run, ..*self }
    }

    /// The closed centreline (local link frame, centre at the origin), 8 points.
    pub fn centerline(&self) -> Vec<P3> {
        let (a, b, x0, lo, hi) = (self.a, self.b, self.x0, self.lo, self.hi);
        vec![
            [-a, b, lo],
            [-x0, b, hi],
            [x0, b, hi],
            [a, b, lo],
            [a, -b, lo],
            [x0, -b, hi],
            [-x0, -b, hi],
            [-a, -b, lo],
        ]
    }

    /// The printed centreline: [`SquareLink::centerline`] with every bend rounded into an arc of radius
    /// 1.25 r in steps of at most 30 degrees, so the tube sweeps smoothly and the capsules along it are the
    /// tube to within a few micrometres.
    pub fn loop_pts(&self) -> Vec<P3> {
        round_loop(&self.centerline(), 1.25 * self.r, 30f64.to_radians())
    }

    /// Distance between neighbours along a chain (the lattice diagonal).
    pub fn chain_pitch(&self) -> f64 {
        self.s * std::f64::consts::SQRT_2
    }

    pub fn bridge(&self) -> f64 {
        2.0 * self.x0
    }

    pub fn thickness(&self) -> f64 {
        self.hi + self.r
    }
}

/// The weave frame in the model frame: origin on the bed, `x`, `y` in the bed plane, `up`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub origin: P3,
    pub x: P3,
    pub y: P3,
    pub up: P3,
}

impl Frame {
    pub fn new(origin: P3, up: P3) -> Frame {
        let up = norm(up);
        // x: the model axis most across up
        let cand = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        let mut x = cand[0];
        let mut best = f64::MAX;
        for c in cand {
            let d = dot(c, up).abs();
            if d < best {
                best = d;
                x = c;
            }
        }
        let x = norm(madd(x, up, -dot(x, up)));
        Frame { origin, x, y: cross(up, x), up }
    }

    pub fn to_model(&self, p: P3) -> P3 {
        add(self.origin, add(add(scale(self.x, p[0]), scale(self.y, p[1])), scale(self.up, p[2])))
    }

    pub fn to_local(&self, p: P3) -> P3 {
        let q = sub(p, self.origin);
        [dot(q, self.x), dot(q, self.y), dot(q, self.up)]
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    /// Closed centreline, model frame.
    pub pts: Vec<P3>,
    pub r: f64,
    /// Lattice cell (i, j) or chain index (i, 0).
    pub cell: (i32, i32),
    /// Filament slot (0: the default).
    pub slot: u8,
}

impl Link {
    pub fn segments(&self) -> impl Iterator<Item = (P3, P3)> + '_ {
        let n = self.pts.len();
        (0..n).map(move |i| (self.pts[i], self.pts[(i + 1) % n]))
    }

    pub fn center(&self) -> P3 {
        let mut c = [0.0; 3];
        for p in &self.pts {
            c = add(c, *p);
        }
        scale(c, 1.0 / self.pts.len() as f64)
    }

    /// Exact distance of the swept solid (unclipped).
    pub fn distance(&self, p: P3) -> f64 {
        self.segments().map(|(a, b)| point_segment(p, a, b)).fold(f64::MAX, f64::min) - self.r
    }

    pub fn bounds(&self) -> (P3, P3) {
        let mut lo = [f64::MAX; 3];
        let mut hi = [f64::MIN; 3];
        for p in &self.pts {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k] - self.r);
                hi[k] = hi[k].max(p[k] + self.r);
            }
        }
        (lo, hi)
    }
}

impl Weave {
    /// A weave from printed links (the evaluator's view): the interlocks are the neighbour pairs that link.
    pub fn of_links(links: &makepad_csg_sdf::Links, frame: Frame, link: SquareLink) -> Weave {
        let ls: Vec<Link> = links.loops.iter().enumerate().map(|(i, l)| Link { pts: l.pts.clone(), r: links.radius, cell: (i as i32, 0), slot: l.slot }).collect();
        let pairs = neighbours(&ls, 0.0).into_iter().filter(|(a, b)| linking_number(&ls[*a as usize].pts, &ls[*b as usize].pts) != 0).collect();
        Weave { links: ls, pairs, frame, link, dropped: 0 }
    }

    /// Connected pieces (by interlocks) and the links in none (loose).
    pub fn pieces(&self) -> (usize, usize) {
        let n = self.links.len();
        let mut parent: Vec<usize> = (0..n).collect();
        fn root(p: &mut Vec<usize>, mut i: usize) -> usize {
            while p[i] != i {
                p[i] = p[p[i]];
                i = p[i];
            }
            i
        }
        let mut degree = vec![0usize; n];
        for &(a, b) in &self.pairs {
            degree[a as usize] += 1;
            degree[b as usize] += 1;
            let (ra, rb) = (root(&mut parent, a as usize), root(&mut parent, b as usize));
            parent[ra] = rb;
        }
        let mut roots: Vec<usize> = (0..n).map(|i| root(&mut parent, i)).collect();
        roots.sort_unstable();
        roots.dedup();
        (roots.len(), degree.iter().filter(|d| **d == 0).count())
    }

    /// The printed solid: every link a tube, clipped at the bed, slots kept.
    pub fn links(&self) -> makepad_csg_sdf::Links {
        let loops = self.links.iter().map(|l| makepad_csg_sdf::LinkLoop { pts: l.pts.clone(), slot: l.slot }).collect();
        let up = self.frame.up;
        makepad_csg_sdf::Links::new(loops, self.link.r, Some((up, dot(up, self.frame.origin))))
    }
}

/// A generated interlocking structure.
#[derive(Clone, Debug, PartialEq)]
pub struct Weave {
    pub links: Vec<Link>,
    /// The designed interlocks (link indices).
    pub pairs: Vec<(u32, u32)>,
    pub frame: Frame,
    pub link: SquareLink,
    /// Links the generator dropped (outside the region, or not connected to the rest).
    pub dropped: usize,
}

fn rot2(p: [f64; 2], ang: f64) -> [f64; 2] {
    let (s, c) = ang.sin_cos();
    [c * p[0] - s * p[1], s * p[0] + c * p[1]]
}

/// A closed polyline with each bend replaced by a circular arc of radius `rho` (smaller where a segment is
/// too short), in steps of at most `step` radians.
pub fn round_loop(pts: &[P3], rho: f64, step: f64) -> Vec<P3> {
    let n = pts.len();
    let mut out = Vec::new();
    for i in 0..n {
        let (prev, p, next) = (pts[(i + n - 1) % n], pts[i], pts[(i + 1) % n]);
        let (lin, lout) = (len(sub(p, prev)), len(sub(next, p)));
        let (din, dout) = (norm(sub(p, prev)), norm(sub(next, p)));
        let theta = dot(din, dout).clamp(-1.0, 1.0).acos();
        if theta < 1e-6 || lin < 1e-12 || lout < 1e-12 {
            out.push(p);
            continue;
        }
        // trim at most 45% of each neighbouring segment
        let trim = (rho * (0.5 * theta).tan()).min(0.45 * lin).min(0.45 * lout);
        let r = trim / (0.5 * theta).tan();
        let a = madd(p, din, -trim);
        let b = madd(p, dout, trim);
        let inward = norm(sub(dout, din));
        let c = madd(p, inward, r / (0.5 * theta).cos());
        let (e1, e2) = (norm(sub(a, c)), norm(sub(b, c)));
        let steps = (theta / step).ceil().max(1.0) as usize;
        for k in 0..=steps {
            let t = k as f64 / steps as f64;
            // slerp from e1 to e2 (they are theta apart)
            let w1 = ((1.0 - t) * theta).sin() / theta.sin().max(1e-12);
            let w2 = (t * theta).sin() / theta.sin().max(1e-12);
            let e = if theta.sin() < 1e-9 { e1 } else { add(scale(e1, w1), scale(e2, w2)) };
            out.push(madd(c, e, r));
        }
    }
    out
}

fn place(link: &SquareLink, frame: &Frame, at: [f64; 2], ang: f64) -> Vec<P3> {
    link.loop_pts()
        .into_iter()
        .map(|p| {
            let q = rot2([p[0], p[1]], ang);
            frame.to_model([at[0] + q[0], at[1] + q[1], p[2]])
        })
        .collect()
}

/// Chainmail over a 2D region of the bed (`region(p) <= 0` inside, local weave coordinates), within `lo..hi`.
/// `angle` turns the weave in the bed plane. A link is kept when its whole footprint is inside; then only the
/// largest connected piece is kept.
pub fn mail(wp: &WeaveProfile, link: SquareLink, frame: Frame, lo: [f64; 2], hi: [f64; 2], angle: f64, region: &dyn Fn([f64; 2]) -> f64) -> Weave {
    mail_graded(wp, link, frame, lo, hi, angle, region, &|_| link.a, &|_| link.a)
}

/// Lattice lines outward from 0: positions and half sizes. Neighbouring lines of half sizes h1, h2 are
/// h1 + h2 - w apart (interlocked links overlap by w); `size(at)` is the half size wanted at a position.
fn grade_lines(link: &SquareLink, reach: f64, size: &dyn Fn(f64) -> f64) -> Vec<(i32, f64, f64)> {
    let fit = |h: f64| link.sized(h, h).a;
    let mut out = vec![(0, 0.0, fit(size(0.0)))];
    for dir in [1.0, -1.0] {
        let (mut at, mut h): (f64, f64) = (0.0, out[0].2);
        let mut i = 0;
        while at.abs() <= reach {
            // the next line's size where it lands: a few fixed-point rounds settle it
            let mut next = h;
            for _ in 0..4 {
                next = fit(size(at + dir * (h + next - link.w)));
            }
            at += dir * (h + next - link.w);
            h = next;
            i += if dir > 0.0 { 1 } else { -1 };
            out.push((i, at, h));
        }
    }
    out
}

/// [`mail`] with links graded in size: `width(u)` is the half size along the lattice's first axis of the
/// links in the column at `u`, `height(v)` along the second axis of the row at `v` (both mm from the region's
/// centre, in the lattice frame turned by `angle`). Interlocked neighbours overlap by the same `w` whatever
/// their sizes, which a square lattice keeps only when sizes vary per column and per row: a link's width is
/// its column's, its depth its row's (a size map that is not separable has no interlocked lattice).
#[allow(clippy::too_many_arguments)]
pub fn mail_graded(wp: &WeaveProfile, link: SquareLink, frame: Frame, lo: [f64; 2], hi: [f64; 2], angle: f64, region: &dyn Fn([f64; 2]) -> f64, width: &dyn Fn(f64) -> f64, height: &dyn Fn(f64) -> f64) -> Weave {
    let span = len([hi[0] - lo[0], hi[1] - lo[1], 0.0]);
    let mid = [0.5 * (lo[0] + hi[0]), 0.5 * (lo[1] + hi[1])];
    let cols = grade_lines(&link, 0.5 * span + link.a, width);
    let rows = grade_lines(&link, 0.5 * span + link.a, height);
    let mut cells = Vec::new();
    for &(j, v, hb) in &rows {
        for &(i, u, ha) in &cols {
            if (i + j).rem_euclid(2) != 0 {
                continue;
            }
            let c = rot2([u, v], angle);
            let c = [mid[0] + c[0], mid[1] + c[1]];
            if c[0] < lo[0] || c[0] > hi[0] || c[1] < lo[1] || c[1] > hi[1] {
                continue;
            }
            let (ea, eb) = (ha + link.r, hb + link.r);
            let inside = [[1.0, 1.0], [-1.0, 1.0], [1.0, -1.0], [-1.0, -1.0], [0.0, 0.0]].iter().all(|d| {
                let q = rot2([d[0] * ea, d[1] * eb], angle);
                region([c[0] + q[0], c[1] + q[1]]) <= 0.0
            });
            if inside {
                cells.push(((i, j), c, link.sized(ha, hb)));
            }
        }
    }
    let total = cells.len();
    let index: std::collections::HashMap<(i32, i32), usize> = cells.iter().enumerate().map(|(k, (ij, _, _))| (*ij, k)).collect();
    let mut pairs = Vec::new();
    for (k, ((i, j), _, _)) in cells.iter().enumerate() {
        for d in [(1, 1), (1, -1)] {
            if let Some(&m) = index.get(&(i + d.0, j + d.1)) {
                pairs.push((k as u32, m as u32));
            }
        }
    }
    // largest connected piece
    let mut parent: Vec<usize> = (0..cells.len()).collect();
    fn root(p: &mut Vec<usize>, mut i: usize) -> usize {
        while p[i] != i {
            p[i] = p[p[i]];
            i = p[i];
        }
        i
    }
    for &(a, b) in &pairs {
        let (ra, rb) = (root(&mut parent, a as usize), root(&mut parent, b as usize));
        parent[ra] = rb;
    }
    let mut count = vec![0usize; cells.len()];
    for k in 0..cells.len() {
        let r = root(&mut parent, k);
        count[r] += 1;
    }
    let best = (0..cells.len()).max_by_key(|k| count[*k]).map(|k| root(&mut parent, k));
    let mut remap = vec![u32::MAX; cells.len()];
    let mut links = Vec::new();
    for (k, (ij, c, sized)) in cells.iter().enumerate() {
        if Some(root(&mut parent, k)) == best {
            remap[k] = links.len() as u32;
            links.push(Link { pts: place(sized, &frame, *c, angle), r: wp.r, cell: *ij, slot: 0 });
        }
    }
    let pairs = pairs.into_iter().filter_map(|(a, b)| Some((remap[a as usize], remap[b as usize])).filter(|(a, b)| *a != u32::MAX && *b != u32::MAX)).collect();
    let dropped = total - links.len();
    Weave { links, pairs, frame, link, dropped }
}

/// The leaning link of a volume fill ("trellis"): a flat rectangle standing on the bed on its bottom bar (along
/// y), its two side bars leaning at `lean` from the bed toward +x (an R link) or -x (an L link), a short top
/// bar across them. Nothing of it hangs over air but the top bar, a short bridge between its own side bars.
///
/// Rows of R and L links alternate along x, `d` apart, so every R leans across the L rows ahead of it and the
/// two planes meet on a horizontal line. Along y the bars sit on a grid of pitch `q` (one in-layer clearance
/// apart): R bars on two grid lines out of four, L bars on the other two, so an R and an L bar never share a
/// plane and pass each other in the layer, never over each other. Each R spans three grid steps and holds one
/// bar of each of the two L links half a period beside it, the 4-in-1 pattern, at every L row it crosses.
/// A rise of (n + 1) d tan(lean) puts every top bar half a row pitch off the other family's tops and at least
/// d tan(lean) over any bar beneath it; a bottom or top bar keeps d sin(lean) from the leaning bars beside it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LeanLink {
    pub r: f64,
    /// Bar grid pitch along y; a link is 3 q wide (centreline), its period 4 q.
    pub q: f64,
    /// Row pitch along x between neighbouring R and L bottoms.
    pub d: f64,
    /// Lean of the side bars above the bed, radians.
    pub lean: f64,
    /// Centreline height of the bottom bar.
    pub lo: f64,
    /// Radius of the rounded corners (centreline). A corner cuts inside the link's own plane only along the
    /// bars it joins, so a bottom corner moves toward the next bar plane by up to this much.
    pub bend: f64,
}

impl LeanLink {
    /// The tightest trellis for a profile at `lean` (at least the profile's leg angle: steeper never overhangs).
    pub fn new(wp: &WeaveProfile, lean: f64) -> LeanLink {
        let r = wp.r;
        let lean = lean.clamp(wp.leg, 85f64.to_radians());
        let (s, t) = (lean.sin(), lean.tan());
        let q = 2.0 * r + wp.c_xy;
        // a top bar bridges over the nearest bar beneath it, d tan(lean) lower: the layer gap plus a layer of sag
        let stacked = (2.0 * r + wp.c_z + wp.layer) / t;
        // a bottom or top bar and the other family's leaning bar beside it come closest across that bar,
        // d sin(lean) apart: the layer gap and half a layer more (the gap runs partly across layers)
        let across = (2.0 * r + wp.c_z + 0.5 * wp.layer) / s;
        // bottom bars of neighbouring rows side by side on the bed (+0.1 for the first layers' squish)
        let bed = 2.0 * r + wp.c_xy + 0.1;
        // parallel bars of one family, 2 d apart along x: across their planes and in the layer
        let parallel = (0.5 * (2.0 * r + wp.c_xy) / s).max(0.5 * (2.0 * r / s + wp.c_xy));
        LeanLink { r, q, d: stacked.max(across).max(bed).max(parallel), lean, lo: r * (1.0 - wp.foot), bend: 1.25 * r }
    }

    pub fn half_width(&self) -> f64 {
        1.5 * self.q
    }

    pub fn period(&self) -> f64 {
        4.0 * self.q
    }

    /// The centreline rise of a link crossing `n + 1` rows of the other family.
    pub fn rise(&self, n: u32) -> f64 {
        (n as f64 + 1.0) * self.d * self.lean.tan()
    }

    pub fn run(&self, n: u32) -> f64 {
        (n as f64 + 1.0) * self.d
    }

    /// The closed centreline of a link with its bottom bar centred on (x, y): `dir` +1 leans toward +x (R),
    /// -1 toward -x (L).
    pub fn centerline(&self, x: f64, y: f64, n: u32, dir: f64) -> Vec<P3> {
        let (b, lo, hi, xt) = (self.half_width(), self.lo, self.lo + self.rise(n), x + dir * self.run(n));
        vec![[x, y - b, lo], [x, y + b, lo], [xt, y + b, hi], [xt, y - b, hi]]
    }

    pub fn loop_pts(&self, x: f64, y: f64, n: u32, dir: f64) -> Vec<P3> {
        round_loop(&self.centerline(x, y, n, dir), self.bend, 30f64.to_radians())
    }

    /// Longest bridge (the top bar between its side bars, centreline).
    pub fn bridge(&self) -> f64 {
        2.0 * self.half_width()
    }
}

/// A volume filled with leaning links ([`LeanLink`]): every link stands on the bed, so nothing prints on air
/// but each top bar's short bridge. `inside(p)` is the solid's distance (local weave frame, negative inside);
/// the box `lo..hi` (local) bounds it. Each link takes the tallest rise of one parity that keeps its bars
/// `margin` inside, so tops step down where the solid's top does; then only the largest connected piece stays.
/// `angle` turns the rows in the bed plane.
#[allow(clippy::too_many_arguments)]
pub fn trellis(wp: &WeaveProfile, link: LeanLink, frame: Frame, lo: P3, hi: P3, angle: f64, margin: f64, inside: &dyn Fn(P3) -> f64) -> Weave {
    let mid = [0.5 * (lo[0] + hi[0]), 0.5 * (lo[1] + hi[1])];
    let reach = 0.5 * len([hi[0] - lo[0], hi[1] - lo[1], 0.0]) + link.period();
    let height = hi[2] - link.lo - wp.r - margin;
    let tan = link.lean.tan();
    // the tallest rise in the box sets the parity every link keeps (tops of R and L then never meet)
    let n_max = (height / (link.d * tan)).floor() as i64 - 1;
    if n_max < 0 {
        return Weave { links: Vec::new(), pairs: Vec::new(), frame, link: SquareLink::new(wp, None), dropped: 0 };
    }
    let n_max = n_max as u32;
    let cols = (reach / link.d).ceil() as i32;
    let rows = (reach / link.period()).ceil() as i32 + 1;
    let place = |p: P3| {
        let q = rot2([p[0], p[1]], angle);
        [mid[0] + q[0], mid[1] + q[1], p[2]]
    };
    let fits = |pts: &[P3]| {
        pts.iter().all(|p| {
            let g = place(*p);
            if g[0] < lo[0] || g[0] > hi[0] || g[1] < lo[1] || g[1] > hi[1] {
                return false;
            }
            // the bottom bar sits in the bed: judge it a bar radius up
            inside([g[0], g[1], g[2].max(wp.r + margin)]) <= -margin
        })
    };
    // (column, row, rise index, local centreline)
    let mut cells: Vec<(i32, i32, u32)> = Vec::new();
    let mut loops: Vec<Vec<P3>> = Vec::new();
    for j in -cols..=cols {
        let dir = if j.rem_euclid(2) == 0 { 1.0 } else { -1.0 };
        let x = j as f64 * link.d;
        for i in -rows..=rows {
            let y = i as f64 * link.period() + if dir < 0.0 { 0.5 * link.period() } else { 0.0 };
            let mut n = n_max as i64;
            while n >= 0 {
                let pts = link.loop_pts(x, y, n as u32, dir);
                if fits(&pts) {
                    cells.push((j, i, n as u32));
                    loops.push(pts);
                    break;
                }
                n -= 2;
            }
        }
    }
    // designed interlocks: an R and an L whose planes meet below both tops, the L half a period to either side
    let index: std::collections::HashMap<(i32, i32), usize> = cells.iter().enumerate().map(|(k, c)| ((c.0, c.1), k)).collect();
    let mut pairs = Vec::new();
    for (k, &(j, i, n)) in cells.iter().enumerate() {
        if j.rem_euclid(2) != 0 {
            continue;
        }
        let mut jj = j + 1;
        while ((jj - j) as f64) * 0.5 <= n as f64 + 1.0 {
            for ii in [i, i - 1] {
                if let Some(&m) = index.get(&(jj, ii)) {
                    let meet = (jj - j) as f64 * 0.5 * link.d * tan;
                    if meet < link.rise(n).min(link.rise(cells[m].2)) {
                        pairs.push((k as u32, m as u32));
                    }
                }
            }
            jj += 2;
        }
    }
    let total = cells.len();
    let keep = largest_piece(total, &pairs);
    let mut remap = vec![u32::MAX; total];
    let mut links = Vec::new();
    for k in 0..total {
        if keep[k] {
            remap[k] = links.len() as u32;
            let (j, i, _) = cells[k];
            links.push(Link { pts: loops[k].iter().map(|p| frame.to_model(place(*p))).collect(), r: wp.r, cell: (j, i), slot: 0 });
        }
    }
    let pairs = pairs.into_iter().filter_map(|(a, b)| Some((remap[a as usize], remap[b as usize])).filter(|(a, b)| *a != u32::MAX && *b != u32::MAX)).collect();
    Weave { links, pairs, frame, link: SquareLink::new(wp, None), dropped: total - keep.iter().filter(|k| **k).count() }
}

/// Which of `n` nodes belong to the largest piece the pairs connect.
fn largest_piece(n: usize, pairs: &[(u32, u32)]) -> Vec<bool> {
    let mut parent: Vec<usize> = (0..n).collect();
    fn root(p: &mut Vec<usize>, mut i: usize) -> usize {
        while p[i] != i {
            p[i] = p[p[i]];
            i = p[i];
        }
        i
    }
    for &(a, b) in pairs {
        let (ra, rb) = (root(&mut parent, a as usize), root(&mut parent, b as usize));
        parent[ra] = rb;
    }
    let mut count = vec![0usize; n];
    for k in 0..n {
        let r = root(&mut parent, k);
        count[r] += 1;
    }
    let best = (0..n).max_by_key(|k| count[*k]).map(|k| root(&mut parent, k));
    (0..n).map(|k| Some(root(&mut parent, k)) == best).collect()
}

/// A chain along a bed path (local weave coordinates, a polyline). Each link turns its diagonal along the chord
/// between its neighbours. `closed`: a loop (a bracelet), the last link interlocks with the first.
pub fn chain(wp: &WeaveProfile, link: SquareLink, frame: Frame, path: &[[f64; 2]], closed: bool) -> Weave {
    chain_graded(wp, link, frame, path, closed, &|_| link.a)
}

/// [`chain`] with links graded in size: `size(t)` is the half size of the link `t` mm along the path. Two
/// square links of half sizes a1, a2 interlock corner to corner (a1 + a2 - w) sqrt 2 apart.
pub fn chain_graded(wp: &WeaveProfile, link: SquareLink, frame: Frame, path: &[[f64; 2]], closed: bool, size: &dyn Fn(f64) -> f64) -> Weave {
    // resample by arc length
    let mut pts = path.to_vec();
    if closed && pts.len() > 2 {
        pts.push(pts[0]);
    }
    let mut seg_len = Vec::new();
    let mut total = 0.0;
    for w in pts.windows(2) {
        let l = ((w[1][0] - w[0][0]).powi(2) + (w[1][1] - w[0][1]).powi(2)).sqrt();
        seg_len.push(l);
        total += l;
    }
    let fit = |t: f64| link.sized(size(t), size(t)).a;
    let pitch = |a1: f64, a2: f64| (a1 + a2 - link.w) * std::f64::consts::SQRT_2;
    // links along the path while they fit: (position, half size)
    let mut placed = vec![(0.0, fit(0.0))];
    loop {
        let (t, h) = *placed.last().unwrap();
        let mut next = h;
        for _ in 0..4 {
            next = fit(t + pitch(h, next));
        }
        let t2 = t + pitch(h, next);
        // a loop also needs room for the step from its last link back to the first
        let room = if closed { total - pitch(next, placed[0].1) } else { total };
        if t2 > room + 1e-9 {
            break;
        }
        placed.push((t2, next));
    }
    if closed && placed.len() > 1 {
        // a loop spreads the rest of its length evenly over its steps
        let (t, h) = *placed.last().unwrap();
        let used = t + pitch(h, placed[0].1);
        let k = total / used;
        for p in &mut placed {
            p.0 *= k;
        }
    }
    let at = |t: f64| -> [f64; 2] {
        let mut t = t;
        for (k, l) in seg_len.iter().enumerate() {
            if t <= *l || k + 1 == seg_len.len() {
                let f = if *l > 0.0 { (t / l).min(1.0) } else { 0.0 };
                return [pts[k][0] + f * (pts[k + 1][0] - pts[k][0]), pts[k][1] + f * (pts[k + 1][1] - pts[k][1])];
            }
            t -= l;
        }
        pts[0]
    };
    let centers: Vec<[f64; 2]> = placed.iter().map(|p| at(p.0)).collect();
    let n = centers.len();
    let mut links = Vec::new();
    for k in 0..n {
        let prev = if k > 0 { centers[k - 1] } else if closed { centers[n - 1] } else { centers[k] };
        let next = if k + 1 < n { centers[k + 1] } else if closed { centers[0] } else { centers[k] };
        let d = [next[0] - prev[0], next[1] - prev[1]];
        // the link's diagonal (1, 1) along the chord: neighbours overlap corner to corner as on the lattice
        let ang = d[1].atan2(d[0]) - std::f64::consts::FRAC_PI_4;
        let sized = link.sized(placed[k].1, placed[k].1);
        links.push(Link { pts: place(&sized, &frame, centers[k], ang), r: wp.r, cell: (k as i32, 0), slot: 0 });
    }
    let mut pairs: Vec<(u32, u32)> = (1..n).map(|k| ((k - 1) as u32, k as u32)).collect();
    if closed && n > 2 {
        pairs.push(((n - 1) as u32, 0));
    }
    Weave { links, pairs, frame, link, dropped: 0 }
}

/// Drapes a weave over a surface: each link moves to the surface height under its centre and tilts with the
/// surface normal (`surface(p) -> (height, normal)` in local weave coordinates). Links over no surface stay.
pub fn drape(w: &mut Weave, surface: &dyn Fn([f64; 2]) -> Option<(f64, P3)>) {
    let frame = w.frame;
    for l in &mut w.links {
        let c = frame.to_local(l.center());
        let Some((h, n)) = surface([c[0], c[1]]) else { continue };
        let n = norm(n);
        let z = [0.0, 0.0, 1.0];
        let axis = cross(z, n);
        let ang = dot(z, n).clamp(-1.0, 1.0).acos();
        let pivot = [c[0], c[1], 0.0];
        for p in &mut l.pts {
            let q = sub(frame.to_local(*p), pivot);
            let q = if len(axis) > 1e-9 { rotate(q, norm(axis), ang) } else { q };
            *p = frame.to_model(add(add(q, pivot), [0.0, 0.0, h]));
        }
    }
}

/// Rests every link on a surface (`dist(p)`: the surface's distance, model frame, negative inside): moves it
/// along up until its closest approach to the surface is `gap`, the bars sampled at most `step` mm apart
/// (links farther than `gap` + 4 bar radii stay). A
/// printed form under draped mail then holds each link `gap` below its lowest bar, nowhere closer.
pub fn rest_on(w: &mut Weave, dist: &dyn Fn(P3) -> f64, gap: f64, step: f64) {
    let up = w.frame.up;
    for l in &mut w.links {
        let r = l.r;
        let n = l.pts.len();
        let mut samples = Vec::new();
        for i in 0..n {
            let (a, b) = (l.pts[i], l.pts[(i + 1) % n]);
            let k = (len(sub(b, a)) / step).ceil().max(1.0) as usize;
            for j in 0..k {
                samples.push(madd(a, sub(b, a), j as f64 / k as f64));
            }
        }
        // links nowhere near the surface (not draped) stay
        if samples.iter().map(|p| dist(*p)).fold(f64::MAX, f64::min) - r > gap + 4.0 * r {
            continue;
        }
        // a field's distance is at most the true one: a few rounds converge from either side
        let mut lift = 0.0;
        for _ in 0..6 {
            let closest = samples.iter().map(|p| dist(madd(*p, up, lift))).fold(f64::MAX, f64::min) - r;
            if (closest - gap).abs() < 1e-4 {
                break;
            }
            lift += gap - closest;
        }
        for p in &mut l.pts {
            *p = madd(*p, up, lift);
        }
    }
}

/// Colours each link from a map over the bed (local weave coordinates) to a filament slot.
pub fn paint(w: &mut Weave, slot: &dyn Fn([f64; 2]) -> u8) {
    let frame = w.frame;
    for l in &mut w.links {
        let c = frame.to_local(l.center());
        l.slot = slot([c[0], c[1]]);
    }
}

pub fn point_segment(p: P3, a: P3, b: P3) -> f64 {
    let ab = sub(b, a);
    let t = (dot(sub(p, a), ab) / dot(ab, ab).max(1e-300)).clamp(0.0, 1.0);
    len(sub(p, madd(a, ab, t)))
}

/// Closest points of two segments.
pub fn segment_segment(p1: P3, q1: P3, p2: P3, q2: P3) -> (P3, P3) {
    let d1 = sub(q1, p1);
    let d2 = sub(q2, p2);
    let r = sub(p1, p2);
    let a = dot(d1, d1);
    let e = dot(d2, d2);
    let f = dot(d2, r);
    let eps = 1e-12;
    let (s, t);
    if a <= eps && e <= eps {
        return (p1, p2);
    }
    if a <= eps {
        s = 0.0;
        t = (f / e).clamp(0.0, 1.0);
    } else {
        let c = dot(d1, r);
        if e <= eps {
            t = 0.0;
            s = (-c / a).clamp(0.0, 1.0);
        } else {
            let b = dot(d1, d2);
            let denom = a * e - b * b;
            let mut s0 = if denom > eps { ((b * f - c * e) / denom).clamp(0.0, 1.0) } else { 0.0 };
            let mut t0 = (b * s0 + f) / e;
            if t0 < 0.0 {
                t0 = 0.0;
                s0 = (-c / a).clamp(0.0, 1.0);
            } else if t0 > 1.0 {
                t0 = 1.0;
                s0 = ((b - c) / a).clamp(0.0, 1.0);
            }
            s = s0;
            t = t0;
        }
    }
    (madd(p1, d1, s), madd(p2, d2, t))
}

/// Gauss linking number of two closed polylines, from the crossings of a generic projection (exact for
/// polylines in general position).
pub fn linking_number(a: &[P3], b: &[P3]) -> i32 {
    // a fixed generic view direction (no edge of a lattice weave is parallel to it)
    let v = norm([0.1234, 0.2345, 0.9637]);
    let (u, w) = basis(v);
    let pr = |p: P3| [dot(p, u), dot(p, w), dot(p, v)];
    let pa: Vec<P3> = a.iter().map(|p| pr(*p)).collect();
    let pb: Vec<P3> = b.iter().map(|p| pr(*p)).collect();
    let mut sum = 0;
    for i in 0..pa.len() {
        let (a0, a1) = (pa[i], pa[(i + 1) % pa.len()]);
        for j in 0..pb.len() {
            let (b0, b1) = (pb[j], pb[(j + 1) % pb.len()]);
            let da = [a1[0] - a0[0], a1[1] - a0[1]];
            let db = [b1[0] - b0[0], b1[1] - b0[1]];
            let den = da[0] * db[1] - da[1] * db[0];
            if den.abs() < 1e-15 {
                continue;
            }
            let r = [b0[0] - a0[0], b0[1] - a0[1]];
            let s = (r[0] * db[1] - r[1] * db[0]) / den;
            let t = (r[0] * da[1] - r[1] * da[0]) / den;
            if !(0.0..1.0).contains(&s) || !(0.0..1.0).contains(&t) {
                continue;
            }
            let za = a0[2] + s * (a1[2] - a0[2]);
            let zb = b0[2] + t * (b1[2] - b0[2]);
            // count the crossings where a passes over b (towards the viewer)
            if za > zb {
                sum += if den > 0.0 { 1 } else { -1 };
            }
        }
    }
    sum
}

/// How a gap between two links will print.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum GapClass {
    Ok,
    /// Prints, but tight: a light tack or a narrow in-layer gap.
    Tight,
    /// Bonds while printing.
    Fused,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WeaveCheck {
    pub links: usize,
    pub pairs: usize,
    /// Smallest gap across layers (one link over another) and in the layer, mm, with where.
    pub min_gap_z: (f64, P3),
    pub min_gap_xy: (f64, P3),
    /// Link pairs closer than they print free, with their gap and class.
    pub close: Vec<(u32, u32, f64, GapClass)>,
    /// Designed pairs that do not interlock (linking number 0).
    pub unlinked: Vec<(u32, u32)>,
    /// Neighbouring links that interlock but were not designed to.
    pub tangled: Vec<(u32, u32)>,
    /// Longest bridge, mm, and the shallowest leg above the bed (deg).
    pub bridge: f64,
    pub leg_deg: f64,
    pub verdict: GapClass,
}

/// Pairs of links whose bounds come within `margin` (a grid hash over the centres).
pub fn neighbours(links: &[Link], margin: f64) -> Vec<(u32, u32)> {
    if links.is_empty() {
        return Vec::new();
    }
    let bounds: Vec<(P3, P3)> = links.iter().map(|l| l.bounds()).collect();
    let mut cell = 0.0f64;
    for (lo, hi) in &bounds {
        for k in 0..3 {
            cell = cell.max(hi[k] - lo[k]);
        }
    }
    let cell = cell + margin;
    let key = |p: P3| ((p[0] / cell).floor() as i64, (p[1] / cell).floor() as i64, (p[2] / cell).floor() as i64);
    let mut grid: std::collections::HashMap<(i64, i64, i64), Vec<u32>> = std::collections::HashMap::new();
    for (i, (lo, _)) in bounds.iter().enumerate() {
        grid.entry(key(*lo)).or_default().push(i as u32);
    }
    let mut out = Vec::new();
    for (i, (lo, hi)) in bounds.iter().enumerate() {
        let k = key(*lo);
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    let Some(list) = grid.get(&(k.0 + dx, k.1 + dy, k.2 + dz)) else { continue };
                    for &j in list {
                        if j as usize <= i {
                            continue;
                        }
                        let (lo2, hi2) = bounds[j as usize];
                        if (0..3).all(|a| lo[a] - margin <= hi2[a] && lo2[a] - margin <= hi[a]) {
                            out.push((i as u32, j));
                        }
                    }
                }
            }
        }
    }
    out.sort_unstable();
    out
}

/// The smallest gap of two links: (gap, at, across layers).
pub fn link_gap(a: &Link, b: &Link, up: P3) -> (f64, P3, bool) {
    let mut best = (f64::MAX, [0.0; 3], false);
    for (a0, a1) in a.segments() {
        for (b0, b1) in b.segments() {
            let (p, q) = segment_segment(a0, a1, b0, b1);
            let d = sub(q, p);
            let l = len(d);
            let g = l - a.r - b.r;
            if g < best.0 {
                let stacked = l > 1e-12 && dot(d, up).abs() / l >= std::f64::consts::FRAC_1_SQRT_2;
                best = (g, madd(p, d, 0.5), stacked);
            }
        }
    }
    best
}

pub fn classify(gap: f64, stacked: bool, wp: &WeaveProfile) -> GapClass {
    if stacked {
        // whole empty layers: one tacks hard, two is a light tack (the design rule)
        if gap < wp.layer * 1.5 {
            GapClass::Fused
        } else if gap < wp.c_z - 0.02 {
            GapClass::Tight
        } else {
            GapClass::Ok
        }
    } else if gap < 0.15 {
        GapClass::Fused
    } else if gap < 0.25 {
        GapClass::Tight
    } else {
        GapClass::Ok
    }
}

/// Checks a weave from its exact geometry: gaps per neighbour pair (in-layer vs across layers), designed
/// interlocks really linked, nothing else tangled, bridges and legs printable.
pub fn check(w: &Weave, wp: &WeaveProfile) -> WeaveCheck {
    let up = w.frame.up;
    let mut out = WeaveCheck {
        links: w.links.len(),
        pairs: w.pairs.len(),
        min_gap_z: (f64::INFINITY, [0.0; 3]),
        min_gap_xy: (f64::INFINITY, [0.0; 3]),
        close: Vec::new(),
        unlinked: Vec::new(),
        tangled: Vec::new(),
        bridge: 0.0,
        leg_deg: 90.0,
        verdict: GapClass::Ok,
    };
    let designed: std::collections::HashSet<(u32, u32)> = w.pairs.iter().map(|&(a, b)| (a.min(b), a.max(b))).collect();
    for (i, j) in neighbours(&w.links, wp.c_xy.max(wp.c_z)) {
        let (a, b) = (&w.links[i as usize], &w.links[j as usize]);
        let (g, at, stacked) = link_gap(a, b, up);
        let slot = if stacked { &mut out.min_gap_z } else { &mut out.min_gap_xy };
        if g < slot.0 {
            *slot = (g, at);
        }
        let class = classify(g, stacked, wp);
        if class != GapClass::Ok {
            out.close.push((i, j, g, class));
            out.verdict = out.verdict.max(class);
        }
        let lk = linking_number(&a.pts, &b.pts);
        let want = designed.contains(&(i, j));
        if want && lk == 0 {
            out.unlinked.push((i, j));
        }
        if !want && lk != 0 {
            out.tangled.push((i, j));
        }
    }
    for &(i, j) in &w.pairs {
        // designed pairs too far apart for the neighbour pass never interlock
        let (a, b) = (&w.links[i as usize], &w.links[j as usize]);
        if linking_number(&a.pts, &b.pts) == 0 && !out.unlinked.contains(&(i.min(j), i.max(j))) {
            out.unlinked.push((i.min(j), i.max(j)));
        }
    }
    if !out.unlinked.is_empty() || !out.tangled.is_empty() {
        out.verdict = GapClass::Fused;
    }
    // bridges: runs of segments shallower than the leg angle off the bed; legs: the steep segments
    let bed = dot(up, w.frame.origin);
    let shallow = wp.leg.to_degrees() - 1.0;
    for l in &w.links {
        let segs: Vec<(f64, f64, f64)> = l
            .segments()
            .map(|(p, q)| {
                let d = sub(q, p);
                let h = dot(d, up);
                let flat = len(madd(d, up, -h));
                let low = dot(p, up).min(dot(q, up)) - bed;
                (h.abs().atan2(flat).to_degrees(), flat, low)
            })
            .collect();
        let off_bed = |low: f64| low > l.r * (1.0 - wp.foot) + 0.3 * l.r;
        let n = segs.len();
        // start the walk on a segment that is not part of a bridge
        let Some(start) = (0..n).find(|&i| !(segs[i].0 < shallow && off_bed(segs[i].2))) else { continue };
        let mut run = 0.0f64;
        for k in 1..=n {
            let (ang, flat, low) = segs[(start + k) % n];
            if ang < shallow && off_bed(low) {
                run += flat;
                out.bridge = out.bridge.max(run);
            } else {
                run = 0.0;
            }
            if ang >= shallow {
                out.leg_deg = out.leg_deg.min(ang);
            }
        }
    }
    out
}

/// Mass properties of a link (capsule chain; overlaps at the corners ignored), `density` g/mm^3.
pub fn link_mass(l: &Link, density: f64) -> MassProps {
    let mut parts = Vec::new();
    for (a, b) in l.segments() {
        let len_ = len(sub(b, a));
        parts.push((a, b, std::f64::consts::PI * l.r * l.r * len_ * density));
    }
    let m: f64 = parts.iter().map(|p| p.2).sum();
    let mut com = [0.0; 3];
    for (a, b, mi) in &parts {
        com = madd(com, scale(add(*a, *b), 0.5), *mi);
    }
    let com = scale(com, 1.0 / m.max(1e-300));
    let mut inertia = [[0.0; 3]; 3];
    for (a, b, mi) in &parts {
        let ax = sub(*b, *a);
        let h = len(ax);
        let u = scale(ax, 1.0 / h.max(1e-300));
        let i_ax = 0.5 * mi * l.r * l.r;
        let i_perp = mi * (3.0 * l.r * l.r + h * h) / 12.0;
        let d = sub(scale(add(*a, *b), 0.5), com);
        let dd = dot(d, d);
        for r in 0..3 {
            for c in 0..3 {
                let e = if r == c { 1.0 } else { 0.0 };
                inertia[r][c] += i_ax * u[r] * u[c] + i_perp * (e - u[r] * u[c]) + mi * (dd * e - d[r] * d[c]);
            }
        }
    }
    MassProps { volume: m / density, mass: m, com, inertia }
}

/// A box3d world for a weave: free links, sleeping, contacts made a bar diameter early, speed capped so no
/// link moves more than that in one step at `hz`.
pub fn assembly(w: &Weave, density: f64, hz: f64) -> crate::sim::Assembly {
    let margin = 2.0 * w.link.r;
    crate::sim::Assembly {
        bodies: bodies(w, density),
        joints: Vec::new(),
        anchors: Vec::new(),
        up: w.frame.up,
        sleep: 2.0,
        workers: 1,
        contact_margin: margin,
        max_speed: margin * hz,
        // a hanging cloth is a long stack of light contacts: stiff contacts and 8 substeps keep 40 rows
        // interlocked (4 substeps let deep rows pass through each other)
        contact_hertz: 240.0,
        substeps: 8,
        ball: None,
    }
}

/// The weave's links as free bodies (capsule chains, no joints).
pub fn bodies(w: &Weave, density: f64) -> Vec<crate::sim::BodySpec> {
    w.links
        .iter()
        .enumerate()
        .map(|(i, l)| crate::sim::BodySpec {
            name: format!("link {}", i),
            mass: link_mass(l, density),
            hulls: Vec::new(),
            tubes: vec![(l.pts.clone(), l.r)],
        })
        .collect()
}

/// One printed bead (the slicer's extrusion between two points), model frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bead {
    pub a: P3,
    pub b: P3,
    pub width: f64,
    pub height: f64,
    /// Printed over air (a bridge or an overhang wall): it sags about half a layer.
    pub over_air: bool,
}

/// The as-printed verdict of a weave: every bead given to the link it builds, and the beads that come
/// within a fused gap of another link.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct BeadCheck {
    pub beads: usize,
    /// Beads near no link (support, skirt, other parts).
    pub stray: usize,
    /// Smallest gap from a bead to a link it does not belong to, mm, and where.
    pub min_gap: Option<(f64, P3)>,
    /// Link pairs a bead nearly bridges: (bead's link, other link, gap, where), worst first, one per pair.
    pub fused: Vec<(u32, u32, f64, P3)>,
    /// Beads that touch two links (a bridge line or support spanning both): (link, link, where).
    pub bridged: Vec<(u32, u32, P3)>,
}

/// Judges the sliced beads of a weave: each bead's gap to every link near it (bead section: a stadium
/// `width` x `height`, lowered half a layer when printed over air), the bead's own link the nearest; a
/// bead closer than `fused` (mm) to another link marks that pair.
pub fn check_beads(links: &makepad_csg_sdf::Links, beads: &[Bead], up: P3, fused: f64) -> BeadCheck {
    let up = norm(up);
    let mut out = BeadCheck { beads: beads.len(), ..Default::default() };
    let mut worst: std::collections::HashMap<(u32, u32), (f64, P3)> = std::collections::HashMap::new();
    let mut gaps: Vec<(f64, usize, P3)> = Vec::new();
    for bead in beads {
        let sag = if bead.over_air { 0.5 * bead.height } else { 0.0 };
        let (a, b) = (madd(bead.a, up, -sag), madd(bead.b, up, -sag));
        let t = norm(sub(b, a));
        let side = norm(cross(up, t));
        let reach = 0.5 * bead.width + links.radius + fused;
        let lo: P3 = std::array::from_fn(|k| a[k].min(b[k]) - reach);
        let hi: P3 = std::array::from_fn(|k| a[k].max(b[k]) + reach);
        gaps.clear();
        links.segments_near(lo, hi, |p, q, li| {
            let (x, y) = segment_segment(a, b, p, q);
            let d = sub(y, x);
            let l = len(d);
            let n = if l > 1e-12 { scale(d, 1.0 / l) } else { up };
            // the stadium's support toward the link: its flat core runs across the bead, in the layer
            let support = 0.5 * (bead.width - bead.height).max(0.0) * dot(n, side).abs() + 0.5 * bead.height;
            let g = l - links.radius - support;
            match gaps.iter_mut().find(|e| e.1 == li) {
                Some(e) if g < e.0 => *e = (g, li, madd(x, d, 0.5)),
                Some(_) => {}
                None => gaps.push((g, li, madd(x, d, 0.5))),
            }
        });
        if gaps.is_empty() {
            out.stray += 1;
            continue;
        }
        gaps.sort_by(|x, y| x.0.total_cmp(&y.0));
        let own = gaps[0].1 as u32;
        if gaps[0].0 > 0.5 * bead.width {
            // nearest link is not within a bead's reach: not one of the links' beads
            out.stray += 1;
            continue;
        }
        for &(g, li, at) in &gaps[1..] {
            let other = li as u32;
            if out.min_gap.map_or(true, |m| g < m.0) {
                out.min_gap = Some((g, at));
            }
            if g <= 0.0 {
                out.bridged.push((own.min(other), own.max(other), at));
            }
            if g < fused {
                let key = (own.min(other), own.max(other));
                let e = worst.entry(key).or_insert((g, at));
                if g < e.0 {
                    *e = (g, at);
                }
            }
        }
    }
    out.fused = worst.into_iter().map(|((a, b), (g, at))| (a, b, g, at)).collect();
    out.fused.sort_by(|x, y| x.2.total_cmp(&y.2));
    out.bridged.sort_by(|x, y| (x.0, x.1).cmp(&(y.0, y.1)));
    out.bridged.dedup_by(|x, y| x.0 == y.0 && x.1 == y.1);
    out
}

/// How a weave is tested in the sim.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WeaveScene {
    /// Dropped flat from `DROP_HEIGHT` onto a floor.
    Drop,
    /// Falls onto a ball under its middle and drapes over it.
    Drape,
    /// Held by the links along one end (the far end of its longer bed axis) and hangs from them.
    Hang,
}

impl WeaveScene {
    pub fn name(self) -> &'static str {
        match self {
            WeaveScene::Drop => "drop",
            WeaveScene::Drape => "drape",
            WeaveScene::Hang => "hang",
        }
    }
}

pub const DROP_HEIGHT: f64 = 20.0;

/// A scene set up: the assembly (anchors in it), gravity, the floor level along up, and a ball (centre,
/// radius) to drape over.
#[derive(Clone, Debug)]
pub struct SceneSetup {
    pub asm: crate::sim::Assembly,
    pub gravity: P3,
    pub floor: Option<f64>,
    pub ball: Option<(P3, f64)>,
    /// How far up a view lifts the scene so it plays above the bed (the drop height, the ball's height).
    pub show_lift: f64,
}

/// The longer bed axis of a weave (local x or y, as a model direction) and its extent along it.
fn long_axis(w: &Weave) -> (P3, f64, f64) {
    let (mut lx, mut hx, mut ly, mut hy) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
    for l in &w.links {
        let c = w.frame.to_local(l.center());
        lx = lx.min(c[0]);
        hx = hx.max(c[0]);
        ly = ly.min(c[1]);
        hy = hy.max(c[1]);
    }
    if hx - lx >= hy - ly {
        (w.frame.x, lx, hx)
    } else {
        (w.frame.y, ly, hy)
    }
}

pub fn scene(w: &Weave, density: f64, scene: WeaveScene) -> SceneSetup {
    let mut asm = assembly(w, density, crate::runner::HZ);
    let up = w.frame.up;
    let bed = dot(up, w.frame.origin);
    match scene {
        WeaveScene::Hang => {
            let (axis, _, hi) = long_axis(w);
            let reach = 0.5 * w.link.chain_pitch().max(w.link.s);
            for (i, l) in w.links.iter().enumerate() {
                if dot(l.center(), axis) - dot(w.frame.origin, axis) > hi - reach {
                    asm.anchors.push(crate::sim::AnchorSpec { body: i, pivot: asm.bodies[i].mass.com, kind: crate::sim::AnchorKind::Fixed, friction: crate::sim::Friction::default() });
                }
            }
            SceneSetup { asm, gravity: scale(axis, -crate::sim::GRAVITY), floor: None, ball: None, show_lift: 0.0 }
        }
        WeaveScene::Drop => SceneSetup { asm, gravity: scale(up, -crate::sim::GRAVITY), floor: Some(bed - DROP_HEIGHT), ball: None, show_lift: DROP_HEIGHT },
        WeaveScene::Drape => {
            let n = w.links.len().max(1) as f64;
            let c = scale(w.links.iter().fold([0.0; 3], |s, l| add(s, l.center())), 1.0 / n);
            let (_, lo, hi) = long_axis(w);
            let r = (0.3 * (hi - lo)).max(10.0);
            let centre = madd(madd(c, up, -dot(c, up) + bed), up, -r - 2.0);
            asm.ball = Some((centre, r));
            SceneSetup { asm, gravity: scale(up, -crate::sim::GRAVITY), floor: Some(bed - 2.0 * r - 10.0), ball: Some((centre, r)), show_lift: 2.0 * r + 12.0 }
        }
    }
}

/// What a weave did in a scene.
#[derive(Clone, Debug, PartialEq)]
pub struct WeaveSimReport {
    pub scene: &'static str,
    pub links: usize,
    /// Interlocked pairs that came apart (their links passed through each other: a sim failure, or links
    /// too small for their bars).
    pub unlinked: usize,
    /// The deepest two links pressed into each other, mm (negative: overlap).
    pub deepest: f64,
    /// When every link came to rest, s.
    pub settled: Option<f64>,
    /// The extent along the bed axes and up at the end, mm.
    pub size: P3,
    /// How far the lowest link fell from where it started, mm.
    pub drop: f64,
    /// How far interlocked links moved against each other, mm: the median over the designed pairs of the change
    /// in one link's centre seen from the other (0 for links that print or sim as one body).
    pub pair_motion: f64,
    pub seconds: f64,
    pub wall_ms: f64,
    pub step_ms: f64,
    pub cancelled: bool,
}

/// Runs a weave in a scene for `seconds` (simulated; at most 30), the same every run.
pub fn simulate(w: &Weave, density: f64, which: WeaveScene, seconds: f64, cancelled: &dyn Fn() -> bool) -> WeaveSimReport {
    let started = std::time::Instant::now();
    let setup = scene(w, density, which);
    let rest: Vec<P3> = setup.asm.bodies.iter().map(|b| b.mass.com).collect();
    let start_pts: Vec<Vec<P3>> = setup.asm.bodies.iter().map(|b| b.tubes[0].0.clone()).collect();
    let mut mw = crate::sim::MechWorld::new(&setup.asm, setup.gravity, setup.floor);
    let hz = crate::runner::HZ;
    let dt = 1.0 / hz;
    let steps = (seconds.clamp(0.0, 30.0) * hz) as usize;
    let mut settled = None;
    let mut done = 0;
    let mut stepping = 0.0;
    for k in 0..steps {
        if cancelled() {
            break;
        }
        let t = std::time::Instant::now();
        mw.step(dt);
        stepping += t.elapsed().as_secs_f64();
        done = k + 1;
        if settled.is_none() && k % 24 == 23 && mw.max_speed() < 2.0 {
            settled = Some(mw.time());
        }
    }
    let poses = mw.poses();
    let up = w.frame.up;
    let posed: Vec<Link> = w
        .links
        .iter()
        .enumerate()
        .map(|(i, l)| Link { pts: start_pts[i].iter().map(|p| poses[i].apply(rest[i], *p)).collect(), r: l.r, cell: l.cell, slot: l.slot })
        .collect();
    let unlinked = w.pairs.iter().filter(|(a, b)| linking_number(&posed[*a as usize].pts, &posed[*b as usize].pts) == 0).count();
    let mut deepest = f64::INFINITY;
    for (i, j) in neighbours(&posed, 0.1) {
        deepest = deepest.min(link_gap(&posed[i as usize], &posed[j as usize], up).0);
    }
    let (mut lo, mut hi) = ([f64::MAX; 3], [f64::MIN; 3]);
    for l in &posed {
        for p in &l.pts {
            let q = w.frame.to_local(*p);
            for k in 0..3 {
                lo[k] = lo[k].min(q[k]);
                hi[k] = hi[k].max(q[k]);
            }
        }
    }
    let centre = |pts: &[P3]| scale(pts.iter().fold([0.0; 3], |s, p| add(s, *p)), 1.0 / pts.len().max(1) as f64);
    let mut motion: Vec<f64> = w
        .pairs
        .iter()
        .map(|&(a, b)| {
            let (a, b) = (a as usize, b as usize);
            // b's centre in a's frame, before and after (a's rotation undone)
            let before = sub(centre(&start_pts[b]), centre(&start_pts[a]));
            let after = sub(centre(&posed[b].pts), centre(&posed[a].pts));
            let rel = poses[a].unrotate(after);
            len(sub(rel, before))
        })
        .collect();
    motion.sort_by(f64::total_cmp);
    let pair_motion = motion.get(motion.len() / 2).copied().unwrap_or(0.0);
    let low_start = start_pts.iter().flatten().map(|p| dot(*p, setup.gravity)).fold(f64::MIN, f64::max);
    let low_end = posed.iter().flat_map(|l| l.pts.iter()).map(|p| dot(*p, setup.gravity)).fold(f64::MIN, f64::max);
    WeaveSimReport {
        scene: which.name(),
        links: w.links.len(),
        unlinked,
        deepest: if deepest.is_finite() { deepest } else { 0.0 },
        settled,
        size: [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]],
        drop: (low_end - low_start) / crate::sim::GRAVITY,
        pair_motion,
        seconds: done as f64 * dt,
        wall_ms: started.elapsed().as_secs_f64() * 1e3,
        step_ms: if done > 0 { stepping * 1e3 / done as f64 } else { 0.0 },
        cancelled: done < steps,
    }
}
