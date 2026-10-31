//! A 2D geometric constraint solver for sketches.
//!
//! Unknowns are point coordinates and circle radii. Every constraint is a
//! residual function that is zero when it holds; all residuals are lengths
//! (angle residuals are scaled by the sketch's size), so one tolerance fits
//! them all. Jacobians are exact: each residual is evaluated once over dual
//! numbers that carry the derivatives of its few local unknowns.
//!
//! The solve is a damped, weighted minimum-norm Newton iteration:
//! `dx = -W⁻¹Jᵀ (J W⁻¹ Jᵀ + λI)⁻¹ r`. Minimum norm means an under-constrained
//! sketch moves as little as possible from where it was drawn, which is what
//! a person expects; the weights let a drag hold the dragged point near the
//! pointer while everything else follows. The damping (raised when a step
//! does not reduce the residual) makes it robust far from a solution and
//! with redundant constraints.
//!
//! After a solve, [`Report`] gives the degrees of freedom (from the rank of
//! the Jacobian), which unknowns are fully determined (zero in every
//! null-space direction: the sketch draws them as fixed), the redundant
//! constraints (their rows add no rank) and, when no solution was reached,
//! the constraints that still fail.
//!
//! Branches (which side of a line a tangent circle sits on, internal or
//! external circle tangency, the side of a point-line distance) are taken
//! from the geometry as it is when the solve starts, so a dimension edit
//! never flips the drawing over.

use crate::region::P2;
use std::f64::consts::PI;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PointId(pub usize);
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LineId(pub usize);
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CircleId(pub usize);
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ArcId(pub usize);

/// A line segment between two points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Line {
    pub a: PointId,
    pub b: PointId,
}

/// A circle: a centre point and a radius unknown (index into the unknowns).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Circle {
    pub center: PointId,
    pub r: usize,
}

/// An arc: a circle and two points on it, counter-clockwise from `start` to
/// `end`. Its two on-circle constraints are added with it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Arc {
    pub circle: CircleId,
    pub start: PointId,
    pub end: PointId,
}

/// One constraint. Dimensions (the variants with a value) are driving: the
/// value is a site in the program, the solver moves geometry to match it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Constraint {
    Coincident(PointId, PointId),
    /// The point stays at this position.
    Fix(PointId, P2),
    Horizontal(LineId),
    Vertical(LineId),
    /// Two points on one horizontal line.
    HorizontalPoints(PointId, PointId),
    /// Two points on one vertical line.
    VerticalPoints(PointId, PointId),
    Parallel(LineId, LineId),
    Perpendicular(LineId, LineId),
    EqualLength(LineId, LineId),
    EqualRadius(CircleId, CircleId),
    /// The line (extended) touches the circle.
    Tangent(LineId, CircleId),
    /// A line and a circle (an arc) that share an end point meet smoothly
    /// there: the line is perpendicular to the radius at that point. This
    /// is the well-conditioned form of a tangent join (use it with the
    /// point on both; `Tangent` alone has a double root there).
    TangentAt(LineId, CircleId, PointId),
    /// The circles touch (externally or internally, as drawn).
    TangentCircles(CircleId, CircleId),
    PointOnLine(PointId, LineId),
    PointOnCircle(PointId, CircleId),
    Midpoint(PointId, LineId),
    /// The two points mirror each other across the line.
    Symmetric(PointId, PointId, LineId),
    /// Point to point distance.
    Distance(PointId, PointId, f64),
    /// Signed horizontal distance `b.x - a.x`.
    DistanceX(PointId, PointId, f64),
    /// Signed vertical distance `b.y - a.y`.
    DistanceY(PointId, PointId, f64),
    /// Distance from a point to a line (extended), on the side it is drawn.
    PointLineDistance(PointId, LineId, f64),
    Length(LineId, f64),
    Radius(CircleId, f64),
    /// Angle from the first line's direction to the second's, radians,
    /// counter-clockwise.
    Angle(LineId, LineId, f64),
}

impl Constraint {
    /// The driving value of a dimension.
    pub fn value(&self) -> Option<f64> {
        use Constraint::*;
        match *self {
            Distance(_, _, v) | DistanceX(_, _, v) | DistanceY(_, _, v) | PointLineDistance(_, _, v) | Length(_, v)
            | Radius(_, v) | Angle(_, _, v) => Some(v),
            _ => None,
        }
    }

    /// Sets a dimension's driving value; false for a geometric constraint.
    pub fn set_value(&mut self, value: f64) -> bool {
        use Constraint::*;
        match self {
            Distance(_, _, v) | DistanceX(_, _, v) | DistanceY(_, _, v) | PointLineDistance(_, _, v) | Length(_, v)
            | Radius(_, v) | Angle(_, _, v) => {
                *v = value;
                true
            }
            _ => false,
        }
    }
}

/// How a solve went.
#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    /// Every constraint holds within the tolerance.
    pub converged: bool,
    pub iterations: usize,
    /// Largest remaining residual (sketch units).
    pub residual: f64,
    /// Degrees of freedom left: unknowns minus the Jacobian's rank.
    pub dof: usize,
    /// Per unknown: fixed by the constraints (no free direction moves it).
    pub determined: Vec<bool>,
    /// Constraints that add nothing (already implied by earlier ones).
    pub redundant: Vec<usize>,
    /// Constraints that still fail (only when not converged).
    pub failing: Vec<usize>,
}

impl Report {
    /// Fully constrained, nothing redundant, nothing failing.
    pub fn well_constrained(&self) -> bool {
        self.converged && self.dof == 0 && self.redundant.is_empty()
    }
}

/// Solver settings.
#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// Residual tolerance relative to the sketch size (at least 1 unit).
    pub tolerance: f64,
    pub max_iterations: usize,
}

impl Default for Options {
    fn default() -> Self {
        Options { tolerance: 1e-10, max_iterations: 100 }
    }
}

/// A constraint sketch: geometry, unknowns and constraints.
#[derive(Clone, Debug, Default)]
pub struct System {
    /// The unknowns: point x, y pairs and radii.
    pub x: Vec<f64>,
    /// Per point, the index of its x unknown (y follows).
    pub points: Vec<usize>,
    pub lines: Vec<Line>,
    pub circles: Vec<Circle>,
    pub arcs: Vec<Arc>,
    pub constraints: Vec<Constraint>,
}

impl System {
    pub fn new() -> System {
        System::default()
    }

    pub fn point(&mut self, p: P2) -> PointId {
        self.points.push(self.x.len());
        self.x.extend_from_slice(&p);
        PointId(self.points.len() - 1)
    }

    pub fn line(&mut self, a: PointId, b: PointId) -> LineId {
        self.lines.push(Line { a, b });
        LineId(self.lines.len() - 1)
    }

    pub fn circle(&mut self, center: PointId, r: f64) -> CircleId {
        self.circles.push(Circle { center, r: self.x.len() });
        self.x.push(r);
        CircleId(self.circles.len() - 1)
    }

    /// An arc on a new circle through `start` and `end`; adds the two
    /// on-circle constraints.
    pub fn arc(&mut self, center: PointId, start: PointId, end: PointId) -> ArcId {
        let c = self.at(center);
        let s = self.at(start);
        let circle = self.circle(center, ((s[0] - c[0]).powi(2) + (s[1] - c[1]).powi(2)).sqrt());
        self.constrain(Constraint::PointOnCircle(start, circle));
        self.constrain(Constraint::PointOnCircle(end, circle));
        self.arcs.push(Arc { circle, start, end });
        ArcId(self.arcs.len() - 1)
    }

    pub fn constrain(&mut self, c: Constraint) -> usize {
        self.constraints.push(c);
        self.constraints.len() - 1
    }

    pub fn at(&self, p: PointId) -> P2 {
        let i = self.points[p.0];
        [self.x[i], self.x[i + 1]]
    }

    pub fn radius(&self, c: CircleId) -> f64 {
        self.x[self.circles[c.0].r]
    }

    /// The point's two unknowns are both determined in this report.
    pub fn point_determined(&self, report: &Report, p: PointId) -> bool {
        let i = self.points[p.0];
        report.determined.get(i).copied().unwrap_or(false) && report.determined.get(i + 1).copied().unwrap_or(false)
    }

    /// The sketch's size: residual scale for angles and the tolerance.
    fn scale(&self) -> f64 {
        let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
        for &i in &self.points {
            for k in 0..2 {
                lo[k] = lo[k].min(self.x[i + k]);
                hi[k] = hi[k].max(self.x[i + k]);
            }
        }
        let mut s: f64 = if self.points.is_empty() { 0.0 } else { (hi[0] - lo[0]).max(hi[1] - lo[1]) };
        for c in &self.circles {
            s = s.max(2.0 * self.x[c.r].abs());
        }
        s.max(1.0)
    }

    /// Solves from the current geometry.
    pub fn solve(&mut self, opts: &Options) -> Report {
        let w = vec![1.0; self.x.len()];
        self.solve_weighted(&w, opts)
    }

    /// Moves `p` to `target` and solves: first with `p` held exactly at the
    /// target while the rest follows with the least movement; if the
    /// constraints do not allow that, `p` is held only strongly (it slides
    /// along what the constraints leave free, staying nearest the target);
    /// if even that fails the sketch keeps its last solution.
    pub fn drag(&mut self, p: PointId, target: P2, opts: &Options) -> Report {
        let before = self.x.clone();
        let i = self.points[p.0];
        for hold in [f64::INFINITY, 1e8] {
            self.x.copy_from_slice(&before);
            self.x[i] = target[0];
            self.x[i + 1] = target[1];
            let mut w = vec![1.0; self.x.len()];
            w[i] = hold;
            w[i + 1] = hold;
            let report = self.solve_weighted(&w, opts);
            if report.converged {
                return report;
            }
        }
        self.x = before;
        self.solve(opts)
    }

    /// Branch signs per constraint, from the current geometry.
    fn branches(&self) -> Vec<f64> {
        let sign = |v: f64| if v < 0.0 { -1.0 } else { 1.0 };
        self.constraints.iter().map(|c| match *c {
            Constraint::Tangent(l, c) => {
                let (a, b, ctr) = (self.at(self.lines[l.0].a), self.at(self.lines[l.0].b), self.at(self.circles[c.0].center));
                sign((b[0] - a[0]) * (ctr[1] - a[1]) - (b[1] - a[1]) * (ctr[0] - a[0]))
            }
            Constraint::PointLineDistance(p, l, _) => {
                let (a, b, q) = (self.at(self.lines[l.0].a), self.at(self.lines[l.0].b), self.at(p));
                sign((b[0] - a[0]) * (q[1] - a[1]) - (b[1] - a[1]) * (q[0] - a[0]))
            }
            Constraint::TangentCircles(c1, c2) => {
                let (p, q) = (self.at(self.circles[c1.0].center), self.at(self.circles[c2.0].center));
                let d = ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2)).sqrt();
                let (r1, r2) = (self.radius(c1), self.radius(c2));
                // 0: external; ±1: internal with the sign of r1 - r2.
                if (d - (r1 + r2)).abs() <= (d - (r1 - r2).abs()).abs() { 0.0 } else { sign(r1 - r2) }
            }
            _ => 0.0,
        }).collect()
    }

    fn solve_weighted(&mut self, w: &[f64], opts: &Options) -> Report {
        let n = self.x.len();
        let scale = self.scale();
        let tol = opts.tolerance * scale;
        let branches = self.branches();
        let winv: Vec<f64> = w.iter().map(|w| 1.0 / w).collect();
        let (mut r, mut jac, mut rows) = self.evaluate(&self.x, scale, &branches);
        let mut lambda = 1e-9;
        let mut iterations = 0;
        while iterations < opts.max_iterations && max_abs(&r) > tol {
            iterations += 1;
            let m = r.len();
            // A = J W⁻¹ Jᵀ
            let mut a = vec![0.0; m * m];
            let mut max_diag: f64 = 0.0;
            for i in 0..m {
                for j in 0..=i {
                    let mut s = 0.0;
                    for k in 0..n {
                        s += jac[i * n + k] * winv[k] * jac[j * n + k];
                    }
                    a[i * m + j] = s;
                    a[j * m + i] = s;
                }
                max_diag = max_diag.max(a[i * m + i]);
            }
            let cost = dot(&r, &r);
            let mut accepted = false;
            while lambda < 1e12 {
                let mut damped = a.clone();
                for i in 0..m {
                    damped[i * m + i] += lambda * max_diag.max(1e-30) + 1e-14 * max_diag;
                }
                if let Some(y) = cholesky_solve(&mut damped, &r, m) {
                    let mut trial = self.x.clone();
                    for k in 0..n {
                        let mut s = 0.0;
                        for i in 0..m {
                            s += jac[i * n + k] * y[i];
                        }
                        trial[k] -= winv[k] * s;
                    }
                    let (tr, tj, trows) = self.evaluate(&trial, scale, &branches);
                    if dot(&tr, &tr) < cost {
                        self.x = trial;
                        r = tr;
                        jac = tj;
                        rows = trows;
                        lambda = (lambda * 0.1).max(1e-12);
                        accepted = true;
                        break;
                    }
                }
                lambda *= 10.0;
            }
            if !accepted {
                break;
            }
        }
        let residual = max_abs(&r);
        let converged = residual <= tol;
        let failing = if converged {
            Vec::new()
        } else {
            let mut f: Vec<usize> = rows.iter().zip(&r).filter(|(_, v)| v.abs() > tol).map(|(c, _)| *c).collect();
            f.dedup();
            f
        };
        let (rank, determined) = analyse(&jac, r.len(), n);
        let redundant = redundant_constraints(&jac, &rows, n);
        Report { converged, iterations, residual, dof: n - rank, determined, redundant, failing }
    }

    /// Residuals, their Jacobian (row-major, rows x unknowns) and each row's
    /// constraint index.
    fn evaluate(&self, x: &[f64], scale: f64, branches: &[f64]) -> (Vec<f64>, Vec<f64>, Vec<usize>) {
        let n = x.len();
        let (mut r, mut jac, mut rows) = (Vec::new(), Vec::new(), Vec::new());
        for (ci, c) in self.constraints.iter().enumerate() {
            let mut ctx = Ctx { x, sys: self, locals: Vec::new() };
            let out = ctx.residuals(c, scale, branches[ci]);
            for d in out.iter() {
                r.push(d.v);
                let base = jac.len();
                jac.resize(base + n, 0.0);
                for (k, &var) in ctx.locals.iter().enumerate() {
                    jac[base + var] += d.g[k];
                }
                rows.push(ci);
            }
        }
        (r, jac, rows)
    }
}

/// Dual number with derivatives over up to 8 local unknowns.
#[derive(Clone, Copy, Debug)]
struct D {
    v: f64,
    g: [f64; 8],
}

impl D {
    fn c(v: f64) -> D {
        D { v, g: [0.0; 8] }
    }
    fn map(self, v: f64, dv: f64) -> D {
        let mut g = self.g;
        for x in &mut g {
            *x *= dv;
        }
        D { v, g }
    }
    fn sqrt(self) -> D {
        let s = self.v.max(1e-300).sqrt();
        self.map(s, 0.5 / s)
    }
    fn atan2(self, x: D) -> D {
        let den = (self.v * self.v + x.v * x.v).max(1e-300);
        let mut g = [0.0; 8];
        for k in 0..8 {
            g[k] = (x.v * self.g[k] - self.v * x.g[k]) / den;
        }
        D { v: self.v.atan2(x.v), g }
    }
}

impl std::ops::Add for D {
    type Output = D;
    fn add(self, o: D) -> D {
        let mut g = self.g;
        for k in 0..8 {
            g[k] += o.g[k];
        }
        D { v: self.v + o.v, g }
    }
}
impl std::ops::Sub for D {
    type Output = D;
    fn sub(self, o: D) -> D {
        let mut g = self.g;
        for k in 0..8 {
            g[k] -= o.g[k];
        }
        D { v: self.v - o.v, g }
    }
}
impl std::ops::Mul for D {
    type Output = D;
    fn mul(self, o: D) -> D {
        let mut g = [0.0; 8];
        for k in 0..8 {
            g[k] = self.g[k] * o.v + self.v * o.g[k];
        }
        D { v: self.v * o.v, g }
    }
}
impl std::ops::Div for D {
    type Output = D;
    fn div(self, o: D) -> D {
        let mut g = [0.0; 8];
        let inv = 1.0 / o.v;
        for k in 0..8 {
            g[k] = (self.g[k] - self.v * inv * o.g[k]) * inv;
        }
        D { v: self.v * inv, g }
    }
}
impl std::ops::Mul<f64> for D {
    type Output = D;
    fn mul(self, s: f64) -> D {
        self.map(self.v * s, s)
    }
}
impl std::ops::Sub<f64> for D {
    type Output = D;
    fn sub(self, s: f64) -> D {
        D { v: self.v - s, g: self.g }
    }
}

type V2 = (D, D);

fn cross(a: V2, b: V2) -> D {
    a.0 * b.1 - a.1 * b.0
}
fn dotd(a: V2, b: V2) -> D {
    a.0 * b.0 + a.1 * b.1
}
fn sub(a: V2, b: V2) -> V2 {
    (a.0 - b.0, a.1 - b.1)
}
fn len(a: V2) -> D {
    dotd(a, a).sqrt()
}

struct Ctx<'a> {
    x: &'a [f64],
    sys: &'a System,
    locals: Vec<usize>,
}

impl<'a> Ctx<'a> {
    fn var(&mut self, i: usize) -> D {
        let k = match self.locals.iter().position(|&l| l == i) {
            Some(k) => k,
            None => {
                self.locals.push(i);
                self.locals.len() - 1
            }
        };
        let mut d = D::c(self.x[i]);
        d.g[k] = 1.0;
        d
    }
    fn pt(&mut self, p: PointId) -> V2 {
        let i = self.sys.points[p.0];
        (self.var(i), self.var(i + 1))
    }
    fn line(&mut self, l: LineId) -> (V2, V2) {
        let Line { a, b } = self.sys.lines[l.0];
        (self.pt(a), self.pt(b))
    }
    fn circle(&mut self, c: CircleId) -> (V2, D) {
        let Circle { center, r } = self.sys.circles[c.0];
        (self.pt(center), self.var(r))
    }

    fn residuals(&mut self, c: &Constraint, scale: f64, branch: f64) -> Vec<D> {
        use Constraint::*;
        match *c {
            Coincident(p, q) => {
                let (a, b) = (self.pt(p), self.pt(q));
                vec![a.0 - b.0, a.1 - b.1]
            }
            Fix(p, at) => {
                let a = self.pt(p);
                vec![a.0 - at[0], a.1 - at[1]]
            }
            Horizontal(l) => {
                let (a, b) = self.line(l);
                vec![b.1 - a.1]
            }
            Vertical(l) => {
                let (a, b) = self.line(l);
                vec![b.0 - a.0]
            }
            HorizontalPoints(p, q) => {
                let (a, b) = (self.pt(p), self.pt(q));
                vec![b.1 - a.1]
            }
            VerticalPoints(p, q) => {
                let (a, b) = (self.pt(p), self.pt(q));
                vec![b.0 - a.0]
            }
            Parallel(l1, l2) => {
                let (a, b) = self.line(l1);
                let (c, d) = self.line(l2);
                let (u, v) = (sub(b, a), sub(d, c));
                vec![cross(u, v) / (len(u) * len(v)) * scale]
            }
            Perpendicular(l1, l2) => {
                let (a, b) = self.line(l1);
                let (c, d) = self.line(l2);
                let (u, v) = (sub(b, a), sub(d, c));
                vec![dotd(u, v) / (len(u) * len(v)) * scale]
            }
            EqualLength(l1, l2) => {
                let (a, b) = self.line(l1);
                let (c, d) = self.line(l2);
                vec![len(sub(b, a)) - len(sub(d, c))]
            }
            EqualRadius(c1, c2) => {
                let (_, r1) = self.circle(c1);
                let (_, r2) = self.circle(c2);
                vec![r1 - r2]
            }
            Tangent(l, c) => {
                let (a, b) = self.line(l);
                let (ctr, r) = self.circle(c);
                let u = sub(b, a);
                vec![cross(u, sub(ctr, a)) / len(u) - r * branch]
            }
            TangentAt(l, c, p) => {
                let (a, b) = self.line(l);
                let (ctr, _) = self.circle(c);
                let q = self.pt(p);
                let u = sub(b, a);
                vec![dotd(u, sub(q, ctr)) / len(u)]
            }
            TangentCircles(c1, c2) => {
                let (p, r1) = self.circle(c1);
                let (q, r2) = self.circle(c2);
                let d = len(sub(p, q));
                if branch == 0.0 { vec![d - (r1 + r2)] } else { vec![d - (r1 - r2) * branch] }
            }
            PointOnLine(p, l) => {
                let q = self.pt(p);
                let (a, b) = self.line(l);
                let u = sub(b, a);
                vec![cross(u, sub(q, a)) / len(u)]
            }
            PointOnCircle(p, c) => {
                let q = self.pt(p);
                let (ctr, r) = self.circle(c);
                vec![len(sub(q, ctr)) - r]
            }
            Midpoint(p, l) => {
                let q = self.pt(p);
                let (a, b) = self.line(l);
                vec![q.0 * 2.0 - (a.0 + b.0), q.1 * 2.0 - (a.1 + b.1)]
            }
            Symmetric(p, q, l) => {
                let (s, t) = (self.pt(p), self.pt(q));
                let (a, b) = self.line(l);
                let u = sub(b, a);
                let m = ((s.0 + t.0) * 0.5, (s.1 + t.1) * 0.5);
                vec![cross(u, sub(m, a)) / len(u), dotd(sub(t, s), u) / len(u)]
            }
            Distance(p, q, v) => {
                let (a, b) = (self.pt(p), self.pt(q));
                vec![len(sub(b, a)) - v]
            }
            DistanceX(p, q, v) => {
                let (a, b) = (self.pt(p), self.pt(q));
                vec![b.0 - a.0 - v]
            }
            DistanceY(p, q, v) => {
                let (a, b) = (self.pt(p), self.pt(q));
                vec![b.1 - a.1 - v]
            }
            PointLineDistance(p, l, v) => {
                let q = self.pt(p);
                let (a, b) = self.line(l);
                let u = sub(b, a);
                vec![cross(u, sub(q, a)) / len(u) * branch - v]
            }
            Length(l, v) => {
                let (a, b) = self.line(l);
                vec![len(sub(b, a)) - v]
            }
            Radius(c, v) => {
                let (_, r) = self.circle(c);
                vec![r - v]
            }
            Angle(l1, l2, v) => {
                let (a, b) = self.line(l1);
                let (c, d) = self.line(l2);
                let (u, w) = (sub(b, a), sub(d, c));
                let phi = cross(u, w).atan2(dotd(u, w));
                // Wrapped to (-pi, pi]: the value jumps only half a turn
                // away from the solution, the slope is unchanged.
                let mut e = phi - v;
                e.v = (e.v + PI).rem_euclid(2.0 * PI) - PI;
                vec![e * scale]
            }
        }
    }
}

fn dot(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn max_abs(a: &[f64]) -> f64 {
    a.iter().fold(0.0, |m, v| m.max(v.abs()))
}

/// Solves `A y = b` for a symmetric positive definite `A` (overwritten).
fn cholesky_solve(a: &mut [f64], b: &[f64], m: usize) -> Option<Vec<f64>> {
    for j in 0..m {
        let mut d = a[j * m + j];
        for k in 0..j {
            d -= a[j * m + k] * a[j * m + k];
        }
        if !(d > 0.0) {
            return None;
        }
        let d = d.sqrt();
        a[j * m + j] = d;
        for i in j + 1..m {
            let mut s = a[i * m + j];
            for k in 0..j {
                s -= a[i * m + k] * a[j * m + k];
            }
            a[i * m + j] = s / d;
        }
    }
    let mut y = b.to_vec();
    for i in 0..m {
        for k in 0..i {
            y[i] -= a[i * m + k] * y[k];
        }
        y[i] /= a[i * m + i];
    }
    for i in (0..m).rev() {
        for k in i + 1..m {
            y[i] -= a[k * m + i] * y[k];
        }
        y[i] /= a[i * m + i];
    }
    Some(y)
}

/// Rank of the Jacobian and, per unknown, whether every null-space
/// direction leaves it still (it is determined).
fn analyse(jac: &[f64], m: usize, n: usize) -> (usize, Vec<bool>) {
    let mut a = jac.to_vec();
    let eps = 1e-9 * a.iter().fold(1.0f64, |s, v| s.max(v.abs()));
    let mut pivots: Vec<usize> = Vec::new();
    let mut row = 0;
    for col in 0..n {
        if row >= m {
            break;
        }
        let (mut best, mut best_v) = (row, 0.0);
        for i in row..m {
            if a[i * n + col].abs() > best_v {
                best = i;
                best_v = a[i * n + col].abs();
            }
        }
        if best_v <= eps {
            continue;
        }
        for k in 0..n {
            a.swap(row * n + k, best * n + k);
        }
        let p = a[row * n + col];
        for k in 0..n {
            a[row * n + k] /= p;
        }
        for i in 0..m {
            if i != row {
                let f = a[i * n + col];
                if f != 0.0 {
                    for k in 0..n {
                        a[i * n + k] -= f * a[row * n + k];
                    }
                }
            }
        }
        pivots.push(col);
        row += 1;
    }
    let rank = pivots.len();
    // Null-space basis: one vector per free column f, v[f] = 1 and
    // v[pivot_k] = -R[k][f]. An unknown is determined when it is zero in all.
    let mut determined = vec![true; n];
    for f in 0..n {
        if pivots.contains(&f) {
            continue;
        }
        determined[f] = false;
        for (k, &pc) in pivots.iter().enumerate() {
            if a[k * n + f].abs() > 1e-7 {
                determined[pc] = false;
            }
        }
    }
    (rank, determined)
}

/// Constraints whose rows lie in the span of the rows before them.
fn redundant_constraints(jac: &[f64], rows: &[usize], n: usize) -> Vec<usize> {
    let mut basis: Vec<Vec<f64>> = Vec::new();
    let mut out = Vec::new();
    for (i, &c) in rows.iter().enumerate() {
        let mut v = jac[i * n..(i + 1) * n].to_vec();
        let norm0 = dot(&v, &v).sqrt();
        if norm0 == 0.0 {
            if out.last() != Some(&c) {
                out.push(c);
            }
            continue;
        }
        for _ in 0..2 {
            for b in &basis {
                let p = dot(&v, b);
                for k in 0..n {
                    v[k] -= p * b[k];
                }
            }
        }
        let norm = dot(&v, &v).sqrt();
        if norm <= 1e-8 * norm0 {
            if out.last() != Some(&c) {
                out.push(c);
            }
        } else {
            for k in 0..n {
                v[k] /= norm;
            }
            basis.push(v);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: P2, b: P2) -> bool {
        (a[0] - b[0]).abs() < 1e-7 && (a[1] - b[1]).abs() < 1e-7
    }

    /// A rough quad, its four corners and four sides.
    fn quad(s: &mut System) -> ([PointId; 4], [LineId; 4]) {
        let p = [s.point([0.5, -0.3]), s.point([37.0, 1.2]), s.point([41.0, 18.0]), s.point([-1.0, 23.0])];
        let l = [s.line(p[0], p[1]), s.line(p[1], p[2]), s.line(p[2], p[3]), s.line(p[3], p[0])];
        (p, l)
    }

    fn rect(s: &mut System) -> ([PointId; 4], [LineId; 4]) {
        let (p, l) = quad(s);
        s.constrain(Constraint::Horizontal(l[0]));
        s.constrain(Constraint::Vertical(l[1]));
        s.constrain(Constraint::Horizontal(l[2]));
        s.constrain(Constraint::Vertical(l[3]));
        (p, l)
    }

    #[test]
    fn rectangle_fully_constrained() {
        let mut s = System::new();
        let (p, l) = rect(&mut s);
        s.constrain(Constraint::Fix(p[0], [0.0, 0.0]));
        s.constrain(Constraint::Length(l[0], 40.0));
        s.constrain(Constraint::Length(l[1], 20.0));
        let r = s.solve(&Options::default());
        assert!(r.well_constrained(), "{r:?}");
        assert!(close(s.at(p[2]), [40.0, 20.0]), "{:?}", s.at(p[2]));
        assert!((0..4).all(|i| s.point_determined(&r, p[i])));
    }

    #[test]
    fn under_constrained_moves_least_and_reports_freedom() {
        let mut s = System::new();
        let (p, _) = rect(&mut s);
        s.constrain(Constraint::Fix(p[0], [0.0, 0.0]));
        let r = s.solve(&Options::default());
        assert!(r.converged);
        assert_eq!(r.dof, 2);
        assert!(s.point_determined(&r, p[0]));
        assert!(!s.point_determined(&r, p[2]));
        // Close to where it was drawn.
        let c = s.at(p[2]);
        assert!((c[0] - 40.0).abs() < 2.5 && (c[1] - 20.0).abs() < 3.5, "{c:?}");
    }

    #[test]
    fn redundant_and_conflicting() {
        let mut s = System::new();
        let (p, l) = rect(&mut s);
        s.constrain(Constraint::Fix(p[0], [0.0, 0.0]));
        s.constrain(Constraint::Length(l[0], 40.0));
        s.constrain(Constraint::Length(l[1], 20.0));
        let extra = s.constrain(Constraint::Parallel(l[0], l[2]));
        let r = s.solve(&Options::default());
        assert!(r.converged && r.redundant == vec![extra], "{r:?}");
        let bad = s.constrain(Constraint::Distance(p[0], p[1], 50.0));
        let r = s.solve(&Options::default());
        assert!(!r.converged);
        assert!(r.failing.contains(&bad) || r.redundant.contains(&bad), "{r:?}");
    }

    #[test]
    fn right_triangle_from_lengths() {
        let mut s = System::new();
        let a = s.point([0.0, 0.0]);
        let b = s.point([3.5, 0.2]);
        let c = s.point([3.2, 3.6]);
        let (ab, bc, ca) = (s.line(a, b), s.line(b, c), s.line(c, a));
        s.constrain(Constraint::Fix(a, [0.0, 0.0]));
        s.constrain(Constraint::Horizontal(ab));
        s.constrain(Constraint::Length(ab, 3.0));
        s.constrain(Constraint::Length(bc, 4.0));
        s.constrain(Constraint::Length(ca, 5.0));
        let r = s.solve(&Options::default());
        assert!(r.well_constrained(), "{r:?}");
        assert!(close(s.at(c), [3.0, 4.0]), "{:?}", s.at(c));
    }

    #[test]
    fn angle_and_perpendicular() {
        let mut s = System::new();
        let o = s.point([0.0, 0.0]);
        let x = s.point([10.0, 0.0]);
        let q = s.point([8.0, 3.0]);
        let (l1, l2) = (s.line(o, x), s.line(o, q));
        s.constrain(Constraint::Fix(o, [0.0, 0.0]));
        s.constrain(Constraint::Fix(x, [10.0, 0.0]));
        s.constrain(Constraint::Length(l2, 10.0));
        let ang = s.constrain(Constraint::Angle(l1, l2, 30f64.to_radians()));
        let r = s.solve(&Options::default());
        assert!(r.well_constrained(), "{r:?}");
        assert!(close(s.at(q), [10.0 * 30f64.to_radians().cos(), 5.0]));
        s.constraints[ang].set_value(150f64.to_radians());
        assert!(s.solve(&Options::default()).converged);
        assert!(close(s.at(q), [10.0 * 150f64.to_radians().cos(), 5.0]), "{:?}", s.at(q));
        s.constraints[ang] = Constraint::Perpendicular(l1, l2);
        assert!(s.solve(&Options::default()).well_constrained());
        assert!(close(s.at(q), [0.0, 10.0]), "{:?}", s.at(q));
    }

    #[test]
    fn tangent_line_keeps_its_side() {
        let mut s = System::new();
        let c = s.point([0.0, 0.0]);
        let circle = s.circle(c, 9.0);
        let a = s.point([-20.0, -12.0]);
        let b = s.point([20.0, -11.0]);
        let l = s.line(a, b);
        s.constrain(Constraint::Fix(c, [0.0, 0.0]));
        s.constrain(Constraint::Radius(circle, 10.0));
        s.constrain(Constraint::Horizontal(l));
        s.constrain(Constraint::Tangent(l, circle));
        assert!(s.solve(&Options::default()).converged);
        assert!((s.at(a)[1] + 10.0).abs() < 1e-7 && (s.at(b)[1] + 10.0).abs() < 1e-7, "{:?}", s.at(a));
    }

    #[test]
    fn slot_from_arcs_and_tangent_lines() {
        // Two half circles joined by tangent lines: a stadium of width 2r.
        let mut s = System::new();
        let c1 = s.point([0.0, 0.0]);
        let c2 = s.point([28.0, 1.0]);
        let (p1, p2, p3, p4) = (s.point([0.0, -5.5]), s.point([29.0, -4.0]), s.point([27.0, 6.0]), s.point([1.0, 5.0]));
        let a1 = s.arc(c2, p2, p3);
        let a2 = s.arc(c1, p4, p1);
        let (bottom, top) = (s.line(p1, p2), s.line(p3, p4));
        let (k1, k2) = (s.arcs[a1.0].circle, s.arcs[a2.0].circle);
        s.constrain(Constraint::Fix(c1, [0.0, 0.0]));
        s.constrain(Constraint::HorizontalPoints(c1, c2));
        s.constrain(Constraint::Distance(c1, c2, 30.0));
        s.constrain(Constraint::EqualRadius(k1, k2));
        s.constrain(Constraint::Radius(k1, 5.0));
        s.constrain(Constraint::TangentAt(bottom, k2, p1));
        s.constrain(Constraint::TangentAt(bottom, k1, p2));
        s.constrain(Constraint::TangentAt(top, k1, p3));
        s.constrain(Constraint::TangentAt(top, k2, p4));
        let r = s.solve(&Options::default());
        assert!(r.well_constrained(), "{r:?}");
        assert!((s.at(p1)[1] + 5.0).abs() < 1e-7 && (s.at(p2)[1] + 5.0).abs() < 1e-7);
        assert!((s.at(p3)[1] - 5.0).abs() < 1e-7 && (s.at(p4)[1] - 5.0).abs() < 1e-7);
        assert!(close(s.at(c2), [30.0, 0.0]));
    }

    #[test]
    fn midpoint_symmetric_equal() {
        let mut s = System::new();
        let a = s.point([0.0, 0.0]);
        let b = s.point([0.0, 10.0]);
        let axis = s.line(a, b);
        let p = s.point([-4.0, 3.0]);
        let q = s.point([5.0, 3.5]);
        let m = s.point([1.0, 1.0]);
        s.constrain(Constraint::Fix(a, [0.0, 0.0]));
        s.constrain(Constraint::Fix(b, [0.0, 10.0]));
        s.constrain(Constraint::Symmetric(p, q, axis));
        s.constrain(Constraint::Midpoint(m, axis));
        let pq = s.line(p, q);
        s.constrain(Constraint::EqualLength(pq, axis));
        s.constrain(Constraint::DistanceY(a, p, 3.0));
        let r = s.solve(&Options::default());
        assert!(r.well_constrained(), "{r:?}");
        assert!(close(s.at(p), [-5.0, 3.0]) && close(s.at(q), [5.0, 3.0]), "{:?} {:?}", s.at(p), s.at(q));
        assert!(close(s.at(m), [0.0, 5.0]));
    }

    #[test]
    fn drag_follows_pointer_and_keeps_constraints() {
        let mut s = System::new();
        let (p, l) = rect(&mut s);
        s.constrain(Constraint::Fix(p[0], [0.0, 0.0]));
        s.solve(&Options::default());
        let r = s.drag(p[2], [55.0, 31.0], &Options::default());
        assert!(r.converged);
        assert!(close(s.at(p[2]), [55.0, 31.0]), "{:?}", s.at(p[2]));
        assert!(close(s.at(p[1]), [55.0, 0.0]) && close(s.at(p[3]), [0.0, 31.0]));
        // A dimensioned side cannot follow: the drag settles nearest.
        s.constrain(Constraint::Length(l[0], 40.0));
        let r = s.drag(p[2], [60.0, 25.0], &Options::default());
        assert!(r.converged);
        assert!(close(s.at(p[1]), [40.0, 0.0]));
        assert!((s.at(p[2])[1] - 25.0).abs() < 1e-6, "{:?}", s.at(p[2]));
    }

    #[test]
    fn long_chain_converges() {
        // A 60-link zigzag pinned at one end with lengths and angles.
        let mut s = System::new();
        let mut prev = s.point([0.0, 0.0]);
        s.constrain(Constraint::Fix(prev, [0.0, 0.0]));
        let mut last_line: Option<LineId> = None;
        for i in 0..60 {
            let p = s.point([(i + 1) as f64 * 4.7, if i % 2 == 0 { 2.5 } else { -0.4 }]);
            let l = s.line(prev, p);
            s.constrain(Constraint::Length(l, 5.0));
            match last_line {
                Some(ll) => s.constrain(Constraint::Angle(ll, l, if i % 2 == 0 { 0.6 } else { -0.6 })),
                None => s.constrain(Constraint::Horizontal(l)),
            };
            last_line = Some(l);
            prev = p;
        }
        let r = s.solve(&Options::default());
        assert!(r.converged && r.dof == 0, "{:?} {} {}", r.converged, r.dof, r.residual);
    }
}
