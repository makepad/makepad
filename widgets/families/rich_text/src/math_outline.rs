//! The outlines a formula fills as one glyph shape. An assembled glyph (a grown
//! delimiter or arrow) has pieces that overlap where they meet. The glyph shader
//! adds up coverage where outlines overlap, and an edge inside a shape still
//! weighs on its anti-aliasing, so each piece is cut at its joints and the cut
//! edges that face each other are taken out: the joint then fills like the
//! inside of one glyph.

use crate::*;
use makepad_draw::text::glyph_outline::Command;

/// The span `lo..=hi` of one axis (0 x, 1 y) that a piece keeps.
#[derive(Clone, Copy)]
pub(crate) struct Slab {
    pub axis: usize,
    pub lo: f32,
    pub hi: f32,
}

/// A line, quadratic or cubic segment: its first `n` points.
#[derive(Clone, Copy)]
struct Seg {
    n: usize,
    p: [[f32; 2]; 4],
}

impl Seg {
    fn new(points: &[[f32; 2]]) -> Self {
        let mut p = [[0.0; 2]; 4];
        p[..points.len()].copy_from_slice(points);
        Self { n: points.len(), p }
    }

    fn start(&self) -> [f32; 2] {
        self.p[0]
    }

    fn end(&self) -> [f32; 2] {
        self.p[self.n - 1]
    }

    /// The two halves at `t` (de Casteljau).
    fn split(&self, t: f32) -> (Seg, Seg) {
        let mut row = self.p;
        let (mut left, mut right) = (*self, *self);
        for k in 0..self.n {
            left.p[k] = row[0];
            right.p[self.n - 1 - k] = row[self.n - 1 - k];
            for j in 0..self.n - 1 - k {
                for a in 0..2 {
                    row[j][a] += (row[j + 1][a] - row[j][a]) * t;
                }
            }
        }
        (left, right)
    }

    fn at(&self, t: f32) -> [f32; 2] {
        self.split(t).0.end()
    }

    /// Where the segment crosses `axis == c`, for `t` in (0, 1).
    fn crossings(&self, axis: usize, c: f32, out: &mut Vec<f32>) {
        const STEPS: usize = 16;
        let f = |t: f32| self.at(t)[axis] - c;
        let (mut t0, mut f0) = (0.0, f(0.0));
        for i in 1..=STEPS {
            let t1 = i as f32 / STEPS as f32;
            let f1 = f(t1);
            if f0 != 0.0 && f1 != 0.0 && (f0 < 0.0) != (f1 < 0.0) {
                let (mut a, mut b, mut fa) = (t0, t1, f0);
                for _ in 0..30 {
                    let m = (a + b) * 0.5;
                    let fm = f(m);
                    if (fm < 0.0) == (fa < 0.0) {
                        a = m;
                        fa = fm;
                    } else {
                        b = m;
                    }
                }
                out.push((a + b) * 0.5);
            } else if f1 == 0.0 && i < STEPS {
                out.push(t1);
            }
            (t0, f0) = (t1, f1);
        }
    }
}

/// Segments that fill as one shape (nonzero): the shader sums each segment's
/// part, so how they chain into contours does not matter.
#[derive(Default)]
pub(crate) struct Outline {
    segs: Vec<Seg>,
}

impl Outline {
    /// A glyph's outline with its origin at `(x, y)` (y downward; the outline's
    /// y grows upward), `scale` layout units per font unit, kept to `slab`:
    /// what lies beyond it is pressed flat onto its edge, a straight cut.
    pub fn add_glyph(&mut self, commands: &[Command], x: f32, y: f32, scale: f32, slab: Option<Slab>) {
        let at = |px: f32, py: f32| [x + px * scale, y - py * scale];
        let mut contour = Vec::new();
        let (mut first, mut pen) = ([0.0; 2], [0.0; 2]);
        for command in commands {
            match command {
                Command::MoveTo(p) => {
                    self.add_contour(&mut contour, pen, first, slab);
                    first = at(p.x, p.y);
                    pen = first;
                }
                Command::LineTo(p) => {
                    contour.push(Seg::new(&[pen, at(p.x, p.y)]));
                    pen = at(p.x, p.y);
                }
                Command::QuadTo(c, p) => {
                    contour.push(Seg::new(&[pen, at(c.x, c.y), at(p.x, p.y)]));
                    pen = at(p.x, p.y);
                }
                Command::CurveTo(c1, c2, p) => {
                    contour.push(Seg::new(&[pen, at(c1.x, c1.y), at(c2.x, c2.y), at(p.x, p.y)]));
                    pen = at(p.x, p.y);
                }
                Command::Close => {
                    self.add_contour(&mut contour, pen, first, slab);
                    pen = first;
                }
            }
        }
        self.add_contour(&mut contour, pen, first, slab);
    }

    fn add_contour(&mut self, contour: &mut Vec<Seg>, pen: [f32; 2], first: [f32; 2], slab: Option<Slab>) {
        if contour.is_empty() {
            return;
        }
        if pen != first {
            contour.push(Seg::new(&[pen, first]));
        }
        for seg in contour.drain(..) {
            let Some(slab) = slab else {
                self.segs.push(seg);
                continue;
            };
            let mut ts = Vec::new();
            seg.crossings(slab.axis, slab.lo, &mut ts);
            seg.crossings(slab.axis, slab.hi, &mut ts);
            ts.sort_by(f32::total_cmp);
            let (mut rest, mut done) = (seg, 0.0);
            for t in ts.into_iter().chain([1.0]) {
                let (piece, tail) = if t < 1.0 { rest.split((t - done) / (1.0 - done)) } else { (rest, rest) };
                let side = piece.at(0.5)[slab.axis];
                if side < slab.lo || side > slab.hi {
                    // Beyond the cut: a straight run along its edge.
                    let edge = if side < slab.lo { slab.lo } else { slab.hi };
                    let (mut from, mut to) = (piece.start(), piece.end());
                    from[slab.axis] = edge;
                    to[slab.axis] = edge;
                    self.segs.push(Seg::new(&[from, to]));
                } else {
                    self.segs.push(piece);
                }
                rest = tail;
                done = t;
            }
        }
    }

    /// Joins the pieces cut at `axis == c`: their straight runs along the cut
    /// are summed by direction, and only what does not cancel is kept (none,
    /// where the two pieces' cross-sections match).
    pub fn join(&mut self, axis: usize, c: f32) {
        let other = 1 - axis;
        let mut runs = Vec::new();
        self.segs.retain(|s| {
            let on_cut = s.n == 2 && s.p[0][axis] == c && s.p[1][axis] == c;
            if on_cut && s.p[0][other] != s.p[1][other] {
                runs.push((s.p[0][other], s.p[1][other]));
            }
            !on_cut
        });
        // How many times the runs cover each stretch of the cut, by direction.
        let mut stops: Vec<f32> = runs.iter().flat_map(|&(a, b)| [a, b]).collect();
        stops.sort_by(f32::total_cmp);
        stops.dedup();
        let point = |v: f32| {
            let mut p = [0.0; 2];
            p[axis] = c;
            p[other] = v;
            p
        };
        for w in stops.windows(2) {
            let mid = (w[0] + w[1]) * 0.5;
            let net: i32 = runs
                .iter()
                .map(|&(a, b)| match (a.min(b) < mid && mid < a.max(b), a < b) {
                    (false, _) => 0,
                    (true, true) => 1,
                    (true, false) => -1,
                })
                .sum();
            let (from, to) = if net > 0 { (w[0], w[1]) } else { (w[1], w[0]) };
            for _ in 0..net.abs() {
                self.segs.push(Seg::new(&[point(from), point(to)]));
            }
        }
    }

    /// Adds the segments to `dg`'s current path.
    pub fn fill(&self, dg: &mut DrawGlyph) {
        let mut pen = None;
        for seg in &self.segs {
            if seg.start() == seg.end() && seg.n == 2 {
                continue;
            }
            if pen != Some(seg.start()) {
                dg.move_to(seg.p[0][0], seg.p[0][1]);
            }
            let p = &seg.p;
            match seg.n {
                2 => dg.line_to(p[1][0], p[1][1]),
                3 => dg.quad_to(p[1][0], p[1][1], p[2][0], p[2][1]),
                _ => dg.bezier_to(p[1][0], p[1][1], p[2][0], p[2][1], p[3][0], p[3][1]),
            }
            pen = Some(seg.end());
        }
    }
}
