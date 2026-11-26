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
//! a person expects. The damping (raised when a step does not reduce the
//! residual) makes it robust far from a solution and with redundant
//! constraints.
//!
//! A drag ([`Drag`], or the one-call [`System::drag`]) has a solver of its
//! own: a trust-region Newton method on the constraint surface that moves the
//! dragged point toward the pointer and keeps the sketch on the side it had at
//! the press.
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

mod drag;

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

/// How one drag move ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DragOutcome {
    /// The dragged point is within half the tolerance of the target, and the constraints hold.
    Reached,
    /// The move stopped at the nearest pose the constraints allow: no free direction brings the
    /// point closer by more than the tolerance. A reachable target can also end here, when the
    /// stop declines a last step of at most 2/3 of the tolerance. So a cue for a pointer past the
    /// sketch's limit reads the remaining distance (more than the tolerance), not this outcome.
    Nearest,
    /// Neither: the move ended at its last accepted pose.
    NotReached,
}

/// What one drag move did.
#[derive(Clone, Debug, PartialEq)]
pub struct DragReport {
    /// The solve report at the pose the move ended on (`converged`: the constraints hold).
    pub report: Report,
    pub outcome: DragOutcome,
    /// The drag keeps the sketch on the side (assembly mode) it had at the press. False where
    /// nothing separates the two sides, so a flip is not prevented there.
    pub side_enforced: bool,
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

/// A drag from a press to its release. It holds what is fixed at the press, so the moves
/// of one drag agree with each other.
#[derive(Clone, Debug)]
pub struct Drag {
    point: PointId,
    opts: Options,
    /// The pose at the press, for `cancel`.
    press: Vec<f64>,
    /// The press frame: σ_press, the branches and the spare class, held across the moves of one drag.
    frame: drag::PressFrame,
    layout: Layout,
}

/// The sketch a drag's frame was built on: a change (a constraint added, removed or edited)
/// means the frame's row offsets no longer apply.
#[derive(Clone, Debug)]
struct Layout {
    unknowns: usize,
    points: Vec<usize>,
    lines: Vec<Line>,
    circles: Vec<Circle>,
    arcs: Vec<Arc>,
    constraints: Vec<Constraint>,
}

impl Layout {
    fn of(s: &System) -> Layout {
        Layout {
            unknowns: s.x.len(), points: s.points.clone(), lines: s.lines.clone(),
            circles: s.circles.clone(), arcs: s.arcs.clone(), constraints: s.constraints.clone(),
        }
    }

    fn matches(&self, s: &System) -> bool {
        self.unknowns == s.x.len() && self.points == s.points && self.lines == s.lines
            && self.circles == s.circles && self.arcs == s.arcs && self.constraints == s.constraints
    }
}

impl Drag {
    /// Moves the dragged point toward `target` from the current pose.
    pub fn to(&mut self, s: &mut System, target: P2) -> DragReport {
        if self.point.0 >= s.points.len() {
            let scale = s.scale();
            let tol = self.opts.tolerance * scale;
            // If the constraints also changed, their current branches replace the stale row mapping.
            let current_branches;
            let branches = if self.layout.constraints == s.constraints {
                &self.frame.branches
            } else {
                current_branches = s.branches();
                &current_branches
            };
            let (r, jac, rows) = s.evaluate(&s.x, scale, branches);
            let report = s.report_from(r, jac, rows, 0, tol);
            return DragReport { report, outcome: DragOutcome::NotReached, side_enforced: self.frame.side_enforced() };
        }
        if !self.layout.matches(s) {
            eprintln!("constrain: the sketch changed during a drag; its press frame is rebuilt at the current pose");
            self.frame = drag::PressFrame::new(s, self.point);
            self.layout = Layout::of(s);
        }
        let scale = s.scale();
        let tol = self.opts.tolerance * scale;
        let end = if target.iter().all(|v| v.is_finite()) {
            drag::drag_move(s, self.point, target, &self.opts, &mut self.frame)
        } else {
            drag::MoveEnd { rounds: 0, outcome: DragOutcome::NotReached }
        };
        let (r, jac, rows) = s.evaluate(&s.x, scale, &self.frame.branches);
        let report = s.report_from(r, jac, rows, end.rounds, tol);
        DragReport { report, outcome: end.outcome, side_enforced: self.frame.side_enforced() }
    }

    /// Restores the press pose if the number of unknowns is unchanged.
    pub fn cancel(&mut self, s: &mut System) {
        if s.x.len() == self.press.len() {
            s.x.copy_from_slice(&self.press);
        }
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

    /// Starts a drag of `p` at the current pose.
    pub fn start_drag(&self, p: PointId, opts: &Options) -> Drag {
        Drag { point: p, opts: *opts, press: self.x.clone(), frame: drag::PressFrame::new(self, p), layout: Layout::of(self) }
    }

    /// The one-call form of [`Self::start_drag`] and [`Drag::to`].
    pub fn drag(&mut self, p: PointId, target: P2, opts: &Options) -> DragReport {
        let mut drag = self.start_drag(p, opts);
        drag.to(self, target)
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
        let scale = self.scale();
        let tol = opts.tolerance * scale;
        let branches = self.branches();
        let (r, jac, rows, iterations) = self.iterate_weighted(w, scale, &branches, tol, opts.max_iterations);
        self.report_from(r, jac, rows, iterations, tol)
    }

    fn report_from(&self, r: Vec<f64>, jac: Vec<f64>, rows: Vec<usize>, iterations: usize, tol: f64) -> Report {
        let n = self.x.len();
        let residual = max_abs(&r);
        let converged = residual <= tol;
        let failing = if converged {
            Vec::new()
        } else {
            let mut f: Vec<usize> = rows.iter().zip(&r).filter(|(_, v)| v.abs() > tol).map(|(c, _)| *c).collect();
            f.dedup();
            f
        };
        let qr = DragQr::new(&jac, n);
        let dof = n - qr.rank();
        let determined = (0..n).map(|j| qr.determined(j)).collect();
        let redundant = redundant_constraints(&jac, &rows, n);
        Report { converged, iterations, residual, dof, determined, redundant, failing }
    }

    /// The weighted, damped min-norm Newton iteration of [`Self::solve_weighted`] without its
    /// report: the last residuals, Jacobian, each row's constraint and the iteration count.
    fn iterate_weighted(
        &mut self, w: &[f64], scale: f64, branches: &[f64], tol: f64, max_iterations: usize,
    ) -> (Vec<f64>, Vec<f64>, Vec<usize>, usize) {
        let n = self.x.len();
        let winv: Vec<f64> = w.iter().map(|w| 1.0 / w).collect();
        let (mut r, mut jac, mut rows) = self.evaluate(&self.x, scale, branches);
        let mut lambda = 1e-9;
        let mut iterations = 0;
        while iterations < max_iterations && max_abs(&r) > tol {
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
                    let (tr, tj, trows) = self.evaluate(&trial, scale, branches);
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
        (r, jac, rows, iterations)
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

/// Pivot cut of the drag's QR, relative to the first pivot (Q7).
const RANK_CUT: f64 = 1e-9;

/// Column-pivoted Householder QR of Jᵀ. The independent columns give the drag
/// its constraint normals Y. Each stores R above the tail of its reflector.
struct DragQr {
    columns: Vec<Vec<f64>>,
    /// Original row index of each column, in pivot order.
    order: Vec<usize>,
    tau: Vec<f64>,
    /// Columns of Z, the orthonormal directions in which the drag can move to first order.
    z: Vec<Vec<f64>>,
}

impl DragQr {
    /// Keep pivots larger than 1e-9 times the first pivot's magnitude.
    fn new(jac: &[f64], n: usize) -> Self {
        let mut columns: Vec<_> = jac.chunks_exact(n.max(1)).map(|row| row.to_vec()).collect();
        let m = columns.len();
        let mut order: Vec<usize> = (0..m).collect();
        let mut tau = Vec::new();
        for k in 0..n.min(columns.len()) {
            let (mut best, mut squared) = (k, 0.0);
            for (j, col) in columns.iter().enumerate().skip(k) {
                let length2 = dot(&col[k..], &col[k..]);
                if length2 > squared {
                    (best, squared) = (j, length2);
                }
            }
            let length = squared.sqrt();
            if length == 0.0 || (k > 0 && length <= RANK_CUT * columns[0][0].abs()) {
                break;
            }
            columns.swap(k, best);
            order.swap(k, best);
            let (done, trailing) = columns.split_at_mut(k + 1);
            let v = &mut done[k];
            let first = v[k];
            let pivot = -length.copysign(first);
            let t = (pivot - first) / pivot;
            for entry in &mut v[k + 1..] {
                *entry /= first - pivot;
            }
            v[k] = pivot;
            tau.push(t);
            for col in trailing {
                Self::reflect(v, t, k, col);
            }
        }
        let rank = tau.len();
        let mut qr = Self { columns, order, tau, z: Vec::with_capacity(n - rank) };
        for j in rank..n {
            let mut v = vec![0.0; n];
            v[j] = 1.0;
            qr.apply_q(&mut v);
            qr.z.push(v);
        }
        qr
    }

    fn reflect(v: &[f64], tau: f64, k: usize, x: &mut [f64]) {
        let a = tau * (x[k] + dot(&v[k + 1..], &x[k + 1..]));
        x[k] -= a;
        for (xi, vi) in x[k + 1..].iter_mut().zip(&v[k + 1..]) {
            *xi -= a * vi;
        }
    }

    fn apply_q(&self, x: &mut [f64]) {
        for k in (0..self.rank()).rev() {
            Self::reflect(&self.columns[k], self.tau[k], k, x);
        }
    }

    fn rank(&self) -> usize {
        self.tau.len()
    }

    /// An unknown is determined if its row in the orthonormal null basis has length below 1e-7.
    fn determined(&self, j: usize) -> bool {
        self.z.iter().map(|v| v[j] * v[j]).sum::<f64>().sqrt() < 1e-7
    }
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
        assert!(r.report.converged);
        assert_eq!(r.outcome, DragOutcome::Reached, "the free corner should reach the pointer");
        assert!(close(s.at(p[2]), [55.0, 31.0]), "{:?}", s.at(p[2]));
        assert!(close(s.at(p[1]), [55.0, 0.0]) && close(s.at(p[3]), [0.0, 31.0]));
        // A dimensioned side cannot follow: the drag settles nearest.
        s.constrain(Constraint::Length(l[0], 40.0));
        let r = s.drag(p[2], [60.0, 25.0], &Options::default());
        assert!(r.report.converged);
        assert_eq!(r.outcome, DragOutcome::Nearest, "the dimensioned side holds the corner at its nearest pose");
        assert!(close(s.at(p[1]), [40.0, 0.0]));
        assert!((s.at(p[2])[1] - 25.0).abs() < 1e-6, "{:?}", s.at(p[2]));
    }

    #[test]
    fn drag_session_moves_and_cancels() {
        fn clone_send<T: Clone + Send>() {}
        clone_send::<Drag>();

        let mut s = System::new();
        let (p, _) = rect(&mut s);
        s.constrain(Constraint::Fix(p[0], [0.0, 0.0]));
        let opts = Options::default();
        assert!(s.solve(&opts).converged, "the rectangle must satisfy its constraints at the press");
        let press = s.x.clone();
        let mut drag = s.start_drag(p[2], &opts);
        for target in [[55.0, 31.0], [48.0, 26.0]] {
            let r = drag.to(&mut s, target);
            assert_eq!(r.outcome, DragOutcome::Reached, "the move to {target:?} should report Reached");
            assert!(close(s.at(p[2]), target), "the dragged corner should be at {target:?}");
        }
        drag.cancel(&mut s);
        assert_eq!(s.x, press, "cancel should restore every unknown to its press value");
    }

    /// The chain, its last angle left out when `closed` is false: the end point swings on a circle
    /// about the point before it (one degree of freedom).
    fn chain_with(closed: bool) -> System {
        let mut s = System::new();
        let n = 50;
        let mut pts = Vec::new();
        let (mut at, mut a) = ([0.0f64, 0.0f64], 0.0f64);
        pts.push(s.point(at));
        for i in 0..n {
            at = [at[0] + 4.0 * a.cos() + 0.3 * ((i * 7 % 5) as f64 - 2.0) * 0.1, at[1] + 4.0 * a.sin()];
            pts.push(s.point(at));
            a += 6f64.to_radians();
        }
        let lines: Vec<LineId> = (0..n).map(|i| s.line(pts[i], pts[i + 1])).collect();
        s.constrain(Constraint::Fix(pts[0], [0.0, 0.0]));
        s.constrain(Constraint::Horizontal(lines[0]));
        for (i, l) in lines.iter().enumerate() {
            s.constrain(Constraint::Length(*l, 4.0));
            if i > 0 && (closed || i + 1 < lines.len()) {
                s.constrain(Constraint::Angle(lines[i - 1], *l, 6f64.to_radians()));
            }
        }
        s
    }

    fn two_link(opts: &Options) -> (System, PointId) {
        let mut two = System::new();
        let a = two.point([0.0, 0.0]);
        let b = two.point([8.0, 6.0]);
        let end = two.point([14.0, 2.0]);
        let (ab, be) = (two.line(a, b), two.line(b, end));
        two.constrain(Constraint::Fix(a, [0.0, 0.0]));
        two.constrain(Constraint::Length(ab, 10.0));
        two.constrain(Constraint::Length(be, 10.0));
        assert!(two.solve(opts).converged, "the two-link press must satisfy both link lengths and the fixed pivot");
        (two, end)
    }

    fn three_link() -> (System, PointId) {
        let mut three = System::new();
        let a = three.point([0.0, 0.0]);
        let b = three.point([10.0, 0.0]);
        let c = three.point([16.0, 8.0]);
        let end = three.point([22.0, 0.0]);
        three.constrain(Constraint::Fix(a, [0.0, 0.0]));
        for (first, second) in [(a, b), (b, c), (c, end)] {
            let l = three.line(first, second);
            three.constrain(Constraint::Length(l, 10.0));
        }
        (three, end)
    }

    fn distance(a: P2, b: P2) -> f64 {
        (a[0] - b[0]).hypot(a[1] - b[1])
    }

    fn rocker(degrees: f64) -> P2 {
        [4.0 * degrees.to_radians().cos(), 4.0 * degrees.to_radians().sin()]
    }

    fn inside_toggle(d: f64) -> P2 {
        let want = 61.0_f64.sqrt() - d;
        let theta = ((41.0 - want * want) / 40.0).acos();
        [4.0 * theta.cos(), 4.0 * theta.sin()]
    }

    // Rocker about the origin, radius 4. The two equal links from (5,0) reach
    // the rocker pin while 41 - 40 cos(theta) <= 61, hence |theta| <= 120°.
    fn four_bar(opts: &Options) -> (System, PointId) {
        let press = rocker(10.0);
        let root = [5.0, 0.0];
        let len = 61.0_f64.sqrt() / 2.0;
        let d = distance(root, press);
        let h = (len * len - d * d / 4.0).sqrt();
        let elbow = [(root[0] + press[0]) / 2.0 - h * (press[1] - root[1]) / d,
                     (root[1] + press[1]) / 2.0 + h * (press[0] - root[0]) / d];
        let mut base = System::new();
        let a = base.point([0.0, 0.0]);
        let b = base.point(root);
        let e = base.point(elbow);
        let end = base.point(press);
        base.constrain(Constraint::Fix(a, [0.0, 0.0]));
        base.constrain(Constraint::Fix(b, root));
        for (p, q, l) in [(a, end, 4.0), (b, e, len), (e, end, len)] {
            let bar = base.line(p, q);
            base.constrain(Constraint::Length(bar, l));
        }
        assert!(base.solve(opts).converged, "four-bar press must satisfy the three bar lengths and ground pivots");
        (base, end)
    }

    fn elbow_offset(x: &[f64], tip: usize) -> f64 {
        let (root, elbow) = (tip - 4, tip - 2);
        let dx = x[tip] - x[root];
        let dy = x[tip + 1] - x[root + 1];
        (dx * (x[elbow + 1] - x[root + 1]) - dy * (x[elbow] - x[root])) / dx.hypot(dy)
    }

    /// Puts the arm's last two links exactly in line, with the pin on the root-pin line
    /// at the links' full length: an exact toggle.
    fn set_collinear(s: &mut System, end: PointId) {
        let tip = s.points[end.0];
        let root = tip - 4;
        let elbow = tip - 2;
        let r0 = [s.x[root], s.x[root + 1]];
        let e = [s.x[elbow], s.x[elbow + 1]];
        let pin = [s.x[tip], s.x[tip + 1]];
        let l1 = distance(e, r0);
        let l2 = distance(pin, e);
        let dx = pin[0] - r0[0];
        let dy = pin[1] - r0[1];
        let len = dx.hypot(dy);
        let u = [dx / len, dy / len];
        s.x[elbow] = r0[0] + l1 * u[0];
        s.x[elbow + 1] = r0[1] + l1 * u[1];
        s.x[tip] = r0[0] + (l1 + l2) * u[0];
        s.x[tip + 1] = r0[1] + (l1 + l2) * u[1];
    }

    fn mirror_case(base: &System, end: PointId, opts: &Options) -> System {
        let mut mirror = base.clone();
        let tip = mirror.points[end.0];
        let root = tip - 4;
        let x = &mut mirror.x;
        let side = elbow_offset(x, tip);
        let dx = x[tip] - x[root];
        let dy = x[tip + 1] - x[root + 1];
        let length = dx.hypot(dy);
        x[tip - 2] += 2.0 * side * dy / length;
        x[tip - 1] -= 2.0 * side * dx / length;
        assert!(mirror.solve(opts).converged, "mirrored fixture must solve");
        assert!(side * elbow_offset(&mirror.x, tip) < 0.0, "mirrored fixture must reverse bend");
        mirror
    }

    fn keeps_side(s: &System, end: PointId, press_bend: f64, tol: f64) -> bool {
        let bend = elbow_offset(&s.x, s.points[end.0]);
        bend.abs() <= tol || bend.signum() == press_bend.signum()
    }

    #[test]
    fn drag_past_the_limit_ends_nearest() {
        let opts = Options::default();
        let (base, end) = four_bar(&opts);
        let mirror = mirror_case(&base, end, &opts);
        for mut s in [base, mirror] {
            let press_bend = elbow_offset(&s.x, s.points[end.0]);
            let mut drag = s.start_drag(end, &opts);
            for k in (2..=11).chain(std::iter::once(13)).chain((1..=11).rev()) {
                let degrees = 10.0 * k as f64;
                let before = s.at(end);
                let tol = opts.tolerance * s.scale();
                let r = drag.to(&mut s, rocker(degrees));
                assert!(r.report.converged, "four-bar constraints failed at {degrees}°");
                assert!(keeps_side(&s, end, press_bend, tol), "four-bar changed side at {degrees}°");
                if k == 13 {
                    assert_eq!(r.outcome, DragOutcome::Nearest, "130° is beyond the four-bar limit");
                    assert!(distance(s.at(end), before) > tol, "the pin froze at its 110° pose");
                    assert!(distance(s.at(end), rocker(120.0)) <= tol, "the nearest pin should be at 120°");
                } else {
                    assert_eq!(r.outcome, DragOutcome::Reached, "the four-bar should reach {degrees}°");
                    assert!(distance(s.at(end), rocker(degrees)) <= tol / 2.0, "the pin missed {degrees}°");
                }
            }
        }
    }

    #[test]
    fn toggle_approach_then_one_call_return_keeps_the_side() {
        let opts = Options::default();
        let (base, end) = four_bar(&opts);
        let mirror = mirror_case(&base, end, &opts);
        for press in [base, mirror] {
            let press_bend = elbow_offset(&press.x, press.points[end.0]);
            let mut s = press.clone();
            let mut drag = s.start_drag(end, &opts);
            for target in [rocker(119.999), press.at(end)] {
                let tol = opts.tolerance * s.scale();
                let r = drag.to(&mut s, target);
                assert!(r.report.converged, "the near-toggle move to {target:?} lost a constraint");
                // The pin rule may stop a reachable move up to 2/3 tol short, as the reference does.
                assert!(matches!(r.outcome, DragOutcome::Reached | DragOutcome::Nearest),
                    "the near-toggle move to {target:?} should settle");
                assert!(distance(s.at(end), target) <= tol, "the near-toggle move missed {target:?}");
                assert!(keeps_side(&s, end, press_bend, tol), "the near-toggle move to {target:?} flipped the elbow");
            }
            // From the press the move to the exact toggle stops on the pin rule, within tol of it.
            let mut s = press;
            let mut drag = s.start_drag(end, &opts);
            let tol = opts.tolerance * s.scale();
            let r = drag.to(&mut s, rocker(120.0));
            assert!(r.report.converged, "the exact toggle must satisfy all four-bar constraints");
            assert!(matches!(r.outcome, DragOutcome::Reached | DragOutcome::Nearest),
                "the move to the exact toggle should settle");
            assert!(distance(s.at(end), rocker(120.0)) <= tol, "the pin missed the exact toggle");
            assert!(keeps_side(&s, end, press_bend, tol), "the move to the exact toggle flipped the elbow");
        }
    }

    #[test]
    fn return_beside_the_toggle_takes_rule_b() {
        let opts = Options::default();
        let (mut s, end) = four_bar(&opts);
        let press_bend = elbow_offset(&s.x, s.points[end.0]);
        let mut drag = s.start_drag(end, &opts);
        let tol = opts.tolerance * s.scale();
        let r = drag.to(&mut s, rocker(130.0));
        assert_eq!(r.outcome, DragOutcome::Nearest, "130° should stop at the four-bar limit");
        assert!(r.report.converged, "the limit pose must satisfy the four-bar constraints");
        assert!(distance(s.at(end), rocker(120.0)) <= tol, "the pin must stop at the 120° toggle");

        let target = inside_toggle(1e-9);
        let tol = opts.tolerance * s.scale();
        let r = drag.to(&mut s, target);
        assert_eq!(r.outcome, DragOutcome::Reached, "the pin should reach the target just inside the toggle");
        assert!(r.report.converged, "the return from the toggle must satisfy the constraints");
        assert!(distance(s.at(end), target) <= tol / 2.0, "the returning pin missed its target");
        assert!(keeps_side(&s, end, press_bend, tol), "the elbow flipped on the return from the toggle");
        assert!(r.report.iterations <= 4,
            "rule (b) should answer the return beside the toggle in about 2 rounds (14 without it)");
    }

    #[test]
    fn offset_start_beside_the_toggle_takes_rule_b() {
        let opts = Options::default();
        let (mut s, end) = four_bar(&opts);
        let tip = s.points[end.0];
        let press_bend = elbow_offset(&s.x, tip);
        let mut drag = s.start_drag(end, &opts);
        let r = drag.to(&mut s, rocker(130.0));
        assert_eq!(r.outcome, DragOutcome::Nearest, "the offset fixture must first settle at the limit");
        assert!(r.report.converged, "the limit pose must satisfy the constraints before shifting the elbow");

        let (root, elbow) = (tip - 4, tip - 2);
        let offset = 3e-5;
        let mut got = elbow_offset(&s.x, tip);
        let side = if got >= 0.0 { 1.0 } else { -1.0 };
        for _ in 0..6 {
            let (dx, dy) = (s.x[tip] - s.x[root], s.x[tip + 1] - s.x[root + 1]);
            let len = dx.hypot(dy);
            let shift = side * offset - got;
            s.x[elbow] += shift * (-dy) / len;
            s.x[elbow + 1] += shift * dx / len;
            assert!(s.solve(&opts).converged, "the shifted elbow must project onto the four-bar constraints");
            got = elbow_offset(&s.x, tip);
            if (got - side * offset).abs() <= 1e-3 * offset { break; }
        }
        assert!((got - side * offset).abs() <= 1e-3 * offset, "the projected elbow missed its starting offset");

        let target = inside_toggle(1e-9);
        let tol = opts.tolerance * s.scale();
        let r = drag.to(&mut s, target);
        // The reference ends this start on the pin rule, 0.59 tol short, in 2 rounds.
        assert!(matches!(r.outcome, DragOutcome::Reached | DragOutcome::Nearest),
            "the pin should settle at the target from the offset start");
        assert!(r.report.converged, "the return from the offset start must satisfy the constraints");
        assert!(distance(s.at(end), target) <= tol, "the pin missed its target from the offset start");
        assert!(keeps_side(&s, end, press_bend, tol), "the elbow flipped on the return from the offset start");
        assert!(r.report.iterations <= 4,
            "rule (b) should answer the return from the offset start in about 2 rounds (the probe's start took 16 without it)");
    }

    #[test]
    fn toggle_form_answers_a_return_from_the_exact_toggle() {
        let opts = Options::default();
        let (base, end) = four_bar(&opts);
        let mut s = mirror_case(&base, end, &opts);
        let press_bend = elbow_offset(&s.x, s.points[end.0]);
        let mut drag = s.start_drag(end, &opts);
        let r = drag.to(&mut s, rocker(130.0));
        assert_eq!(r.outcome, DragOutcome::Nearest, "the mirrored four-bar should settle at its limit at 130°");

        set_collinear(&mut s, end);
        let target = inside_toggle(1e-9);
        let tol = opts.tolerance * s.scale();
        let r = drag.to(&mut s, target);
        assert_eq!(r.outcome, DragOutcome::Reached, "the mirrored four-bar should reach the target inside the toggle");
        assert!(r.report.converged, "the return must satisfy the mirrored four-bar constraints");
        assert!(distance(s.at(end), target) <= tol / 2.0, "the mirrored pin missed its return target");
        assert!(keeps_side(&s, end, press_bend, tol), "the mirrored elbow flipped on the return");
        assert!(r.report.iterations <= 2,
            "the toggle form of rule (b) answers the return from the exact toggle in 2 rounds (3 without it)");

        let (mut s, end) = two_link(&opts);
        let press_bend = elbow_offset(&s.x, s.points[end.0]);
        let mut drag = s.start_drag(end, &opts);
        let tol = opts.tolerance * s.scale();
        let r = drag.to(&mut s, [30.0, 0.0]);
        assert_eq!(r.outcome, DragOutcome::Nearest, "the two-link arm should settle at full stretch");
        assert!(distance(s.at(end), [20.0, 0.0]) <= tol, "the two-link pin missed its full-stretch position");

        set_collinear(&mut s, end);
        let target = [19.9, 0.0];
        let tol = opts.tolerance * s.scale();
        let r = drag.to(&mut s, target);
        assert_eq!(r.outcome, DragOutcome::Reached, "the two-link arm should reach its return target");
        assert!(r.report.converged, "the return must satisfy the two-link constraints");
        assert!(distance(s.at(end), target) <= tol / 2.0, "the two-link pin missed its return target");
        assert!(keeps_side(&s, end, press_bend, tol), "the two-link elbow flipped on the return");
        assert!(r.report.iterations <= 4,
            "the toggle form of rule (b) answers the return from the exact toggle in 4 rounds (5 without it)");
    }

    #[test]
    fn k10_full_stretch_one_call_and_sixty_moves() {
        let opts = Options::default();
        let (base, end) = two_link(&opts);
        let mirror = mirror_case(&base, end, &opts);
        for press in [base, mirror] {
            let elbow = PointId(end.0 - 1);
            let root = press.at(PointId(end.0 - 2));
            let p0 = press.at(end);
            let l1 = distance(root, press.at(elbow));
            let l2 = distance(press.at(elbow), p0);
            let limit = l1 + l2;
            let lh = l1 * l2 / limit;
            let press_bend = elbow_offset(&press.x, press.points[end.0]);
            let (dx, dy) = (30.0 - root[0], 0.0 - root[1]);
            let unit = [dx / dx.hypot(dy), dy / dx.hypot(dy)];
            let straight_elbow = [root[0] + l1 * unit[0], root[1] + l1 * unit[1]];
            for d in [0.0, 1e-3] {
                let target = [root[0] + (limit + d) * unit[0], root[1] + (limit + d) * unit[1]];
                let mut final_pins = [[0.0; 2]; 2];
                for (pin, moves) in final_pins.iter_mut().zip([60, 1]) {
                    let mut s = press.clone();
                    let mut drag = s.start_drag(end, &opts);
                    for k in 1..=moves {
                        let t = k as f64 / moves as f64;
                        let pt = [p0[0] + (target[0] - p0[0]) * t, p0[1] + (target[1] - p0[1]) * t];
                        let v = [pt[0] - root[0], pt[1] - root[1]];
                        let r = v[0].hypot(v[1]);
                        let foot = if r > limit { [root[0] + v[0] * limit / r, root[1] + v[1] * limit / r] } else { pt };
                        let tol = opts.tolerance * s.scale();
                        let result = drag.to(&mut s, pt);
                        assert!(result.report.converged, "arm constraints failed at move {k}/{moves}, d={d}");
                        assert!(distance(s.at(end), foot) <= tol, "pin missed its foot at move {k}/{moves}, d={d}");
                        assert!(keeps_side(&s, end, press_bend, tol), "arm flipped at move {k}/{moves}, d={d}");
                        if k == moves {
                            if d > 0.0 {
                                assert_eq!(result.outcome, DragOutcome::Nearest, "the arm should settle at full stretch");
                            } else {
                                assert!(matches!(result.outcome, DragOutcome::Reached | DragOutcome::Nearest),
                                    "the move to the exact limit should settle");
                            }
                            let allowance = tol + 2.0 * (2.0 * tol * lh).sqrt();
                            assert!(distance(s.at(elbow), straight_elbow) <= allowance,
                                "elbow missed its straight pose after {moves} moves, d={d}");
                        }
                    }
                    *pin = s.at(end);
                }
                let tol = opts.tolerance * press.scale();
                assert!(distance(final_pins[0], final_pins[1]) <= 2.0 * tol,
                    "one move and sixty moves disagree at full stretch, d={d}");
            }
        }
    }

    fn coupled_chain(opts: &Options) -> (System, PointId, f64) {
        let mut s = chain_with(false);
        let n = s.lines.len();
        let end = PointId(s.points.len() - 1);
        s.constraints.retain(|c| !matches!(c, Constraint::Angle(_, l, _) if l.0 == n - 2));
        let y = s.at(end)[1];
        s.constrain(Constraint::DistanceY(PointId(0), end, y));
        assert!(s.solve(opts).converged, "the height-constrained chain must solve at the press");
        (s, end, y)
    }

    #[test]
    fn coupled_chain_at_its_limit() {
        let opts = Options::default();
        let (mut s, end, y) = coupled_chain(&opts);
        let at = s.at(end);
        let root = s.at(PointId(end.0 - 2));
        let half_width = (8.0_f64.powi(2) - (y - root[1]).powi(2)).sqrt();
        let press_bend = elbow_offset(&s.x, s.points[end.0]);
        let mut drag = s.start_drag(end, &opts);
        for k in 1..=30 {
            let x = at[0] - 0.2 * k as f64;
            let tol = opts.tolerance * s.scale();
            let r = drag.to(&mut s, [x, at[1] + 3.0]);
            assert!(r.report.converged, "the coupled chain lost a constraint at move {k}");
            if k >= 28 {
                let foot = [x.clamp(root[0] - half_width, root[0] + half_width), y];
                assert_eq!(r.outcome, DragOutcome::Nearest, "the coupled pin should settle at move {k}");
                assert!(distance(s.at(end), foot) <= tol, "the coupled pin missed its foot at move {k}");
                assert!(keeps_side(&s, end, press_bend, tol), "the coupled chain flipped at move {k}");
            }
        }
    }

    #[test]
    fn coupled_loose_point_keeps_its_side() {
        let opts = Options::default();
        let (mut s, end, y) = coupled_chain(&opts);
        let loose = s.point([0.0, 0.0]);
        let at = s.at(end);
        let root = s.at(PointId(end.0 - 2));
        let half_width = (8.0_f64.powi(2) - (y - root[1]).powi(2)).sqrt();
        let tip = s.points[end.0];
        let press_bend = elbow_offset(&s.x, tip);
        let mut drag = s.start_drag(end, &opts);
        // Out to the limit and back: the return to k = 28 starts at the rank-0 limit pose, as move 32
        // of the reference's 60-move series does, and ends at the same pose.
        for k in [1, 30, 28] {
            let x = at[0] - 0.2 * k as f64;
            let target = [x, at[1] + 3.0];
            let foot = [x.clamp(root[0] - half_width, root[0] + half_width), y];
            let tol = opts.tolerance * s.scale();
            let mut copy = s.clone();
            let mut fresh = copy.start_drag(end, &opts);
            let fresh_result = fresh.to(&mut copy, target);
            let r = drag.to(&mut s, target);
            assert!(r.report.converged, "the chain lost a constraint at k={k}");
            assert_eq!(r.outcome, DragOutcome::Nearest, "the target lies above the height constraint at k={k}");
            assert!(distance(s.at(end), foot) <= tol, "the pin missed its closest point at k={k}");
            assert!(keeps_side(&s, end, press_bend, tol), "the elbow changed sides at k={k}");
            // The class is judged once, at the press. A frame built at the limit pose itself (the start
            // of the return to k = 28) judges the spare coupled, so only the regular starts compare.
            assert!(r.side_enforced, "the press's spare class must hold at k={k} (a frame judged at the limit reads it coupled)");
            if k != 28 {
                assert_eq!(r.side_enforced, fresh_result.side_enforced, "the fresh and frozen spare classes differ at k={k}");
            }
            assert_eq!(s.at(loose), [0.0, 0.0], "the loose point moved at k={k}");
            if k == 28 {
                let bend = elbow_offset(&s.x, tip);
                assert!((bend - (-0.45877)).abs() <= 1e-4, "the elbow missed its reference offset on the return to k=28: {bend}");
            }
        }
    }

    #[test]
    fn mirror_neg_answers_a_return_from_the_exact_limit() {
        let opts = Options::default();
        let (mut s, end, y) = coupled_chain(&opts);
        let loose = s.point([0.0, 0.0]);
        let at = s.at(end);
        let root = s.at(PointId(end.0 - 2));
        let half_width = (8.0_f64.powi(2) - (y - root[1]).powi(2)).sqrt();
        let tip = s.points[end.0];
        let press_bend = elbow_offset(&s.x, tip);
        let mut drag = s.start_drag(end, &opts);
        let r = drag.to(&mut s, [at[0] - 0.2 * 30.0, at[1] + 3.0]);
        assert_eq!(r.outcome, DragOutcome::Nearest, "the coupled chain should settle at its limit at k=30");

        set_collinear(&mut s, end);
        let x = at[0] - 0.2 * 28.0;
        let target = [x, at[1] + 3.0];
        let foot = [x.clamp(root[0] - half_width, root[0] + half_width), y];
        let tol = opts.tolerance * s.scale();
        let r = drag.to(&mut s, target);
        assert_eq!(r.outcome, DragOutcome::Nearest, "the return should settle below the target at k=28");
        assert!(r.report.converged, "the return must satisfy the coupled chain constraints");
        assert!(distance(s.at(end), foot) <= tol, "the coupled pin missed its foot on the return");
        assert!(keeps_side(&s, end, press_bend, tol), "the coupled elbow flipped on the return");
        assert!(r.side_enforced, "the return must enforce the side chosen at the press");
        assert_eq!(s.at(loose), [0.0, 0.0], "the loose point moved on the return");
        let bend = elbow_offset(&s.x, tip);
        assert!((bend - (-0.45877)).abs() <= 1e-4,
            "MIRROR_NEG should carry the return from the exact limit to its foot (without it the drag stalls there, NotReached)");
    }

    #[test]
    fn open_centre_stays_where_it_is() {
        let opts = Options::default();
        let mut s = chain_with(false);
        assert!(s.solve(&opts).converged, "the open chain must satisfy its constraints at the press");
        let end = PointId(s.points.len() - 1);
        let centre = s.at(PointId(end.0 - 1));
        let start = s.at(end);
        let tol = opts.tolerance * s.scale();
        let mut drag = s.start_drag(end, &opts);
        let r = drag.to(&mut s, centre);
        assert_eq!(r.outcome, DragOutcome::Nearest, "the centre is equally far from every allowed pin pose");
        assert!(r.report.converged, "the open chain must keep its constraints when dragged to the centre");
        assert!((distance(s.at(end), centre) - 4.0).abs() <= tol, "the pin left its circle around the centre");
        assert!(distance(s.at(end), start) <= tol, "the pin should stay where it started on a continuum of nearest poses");
    }

    #[test]
    fn three_link_reaches_with_a_spare_freedom() {
        let opts = Options::default();
        let (mut s, end) = three_link();
        let spare = s.point([30.0, 30.0]);
        let mut drag = s.start_drag(end, &opts);
        for k in 1..=60 {
            let target = [22.0 - 10.0 * k as f64 / 60.0, 6.0 * k as f64 / 60.0];
            let tol = opts.tolerance * s.scale();
            let r = drag.to(&mut s, target);
            assert!(r.report.converged, "the three-link arm lost a constraint at move {k}");
            assert!(!r.side_enforced, "the three-link arm's spare freedom couples to its fold, so no sheet separates its sides");
            // Some moves stop on the pin rule up to 2/3 tol short of the target, as the reference does.
            assert!(matches!(r.outcome, DragOutcome::Reached | DragOutcome::Nearest),
                "the three-link arm should settle at target {k}");
            assert!(distance(s.at(end), target) <= tol, "the three-link pin missed target {k}");
            assert!(distance(s.at(spare), [30.0, 30.0]) <= 1e-12, "the unrelated point moved at target {k}");
        }
    }

    #[test]
    fn curved_drag_keeps_its_branch() {
        let opts = Options::default();
        let mut s = System::new();
        let center = s.point([0.0, 0.0]);
        let circle = s.circle(center, 10.0);
        let fixed = [-20.0, -10.0];
        let a = s.point(fixed);
        let b = s.point([20.0, -10.0]);
        let line = s.line(a, b);
        s.constrain(Constraint::Fix(center, [0.0, 0.0]));
        s.constrain(Constraint::Radius(circle, 10.0));
        s.constrain(Constraint::Fix(a, fixed));
        s.constrain(Constraint::Tangent(line, circle));
        let side = |s: &System| {
            let (a, b, c) = (s.at(a), s.at(b), s.at(center));
            (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
        };
        let press_side = side(&s);
        let mut drag = s.start_drag(b, &opts);
        // At 90° the nearest point on the press tangent coincides with the fixed end.
        for k in 1..=60 {
            let angle = (k as f64).to_radians();
            let target = [fixed[0] + 40.0 * angle.cos(), fixed[1] + 40.0 * angle.sin()];
            let r = drag.to(&mut s, target);
            assert!(r.report.converged, "the line lost tangency at move {k}");
            assert!(matches!(r.outcome, DragOutcome::Reached | DragOutcome::Nearest),
                "the tangent endpoint failed to settle at move {k}");
            assert!(side(&s) * press_side > 0.0, "the centre must stay on its press side of the tangent at move {k}");
        }
    }

    #[test]
    fn stale_drag_session_rebuilds() {
        let opts = Options::default();
        let (mut s, end) = four_bar(&opts);
        let elbow = PointId(end.0 - 1);
        let press = s.at(end);
        let mut drag = s.start_drag(end, &opts);
        s.constrain(Constraint::Fix(elbow, s.at(elbow)));
        let tol = opts.tolerance * s.scale();
        let r = drag.to(&mut s, rocker(20.0));
        assert!(r.report.converged, "the rebuilt frame must satisfy the added elbow constraint");
        assert_eq!(r.outcome, DragOutcome::Nearest, "the fixed elbow should hold the pin at its nearest pose");
        assert!(distance(s.at(end), press) <= tol, "the newly constrained pin moved away from its press");
        let r = drag.to(&mut s, press);
        assert!(r.report.converged, "the rebuilt frame must keep its constraints on a second move");
        assert_eq!(r.outcome, DragOutcome::Reached, "a second move to the fixed pin should reach it");
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

    fn check_null_basis(qr: &DragQr, jac: &[f64], scale: f64) {
        for (i, zi) in qr.z.iter().enumerate() {
            for (j, zj) in qr.z.iter().enumerate().take(i + 1) {
                let error = (dot(zi, zj) - if i == j { 1.0 } else { 0.0 }).abs();
                assert!(error <= 8.0 * f64::EPSILON, "ZᵀZ differs from I at ({i}, {j}) by {error:e}");
            }
            for (j, row) in jac.chunks_exact(zi.len()).enumerate() {
                let error = dot(row, zi).abs();
                assert!(error <= 8.0 * f64::EPSILON * scale, "JZ differs from zero at ({j}, {i}) by {error:e}");
            }
        }
    }

    #[test]
    fn redundant_rectangle_null_basis_and_report() {
        let mut s = System::new();
        let (p, l) = rect(&mut s);
        s.constrain(Constraint::Fix(p[0], [0.0, 0.0]));
        s.constrain(Constraint::Length(l[0], 40.0));
        s.constrain(Constraint::Parallel(l[0], l[2]));
        s.constrain(Constraint::Length(l[0], 40.0));
        s.x.copy_from_slice(&[0.0, 0.0, 40.0, 0.0, 40.0, 25.0, 0.0, 25.0]);
        let scale = s.scale();
        let branches = s.branches();
        let (r, jac, _) = s.evaluate(&s.x, scale, &branches);
        let qr = DragQr::new(&jac, s.x.len());
        assert_eq!(r.len(), 9, "redundant rectangle has nine scalar rows");
        assert_eq!(qr.rank(), 7, "redundant rectangle has seven independent rows");
        assert_eq!(qr.z.len(), 1, "rectangle has one free height");
        check_null_basis(&qr, &jac, scale);

        let report = s.solve(&Options::default());
        assert_eq!(report.dof, 1, "the rectangle report must leave its height free");
        assert_eq!(report.determined, [true, true, true, true, true, false, true, false],
            "only the two top y coordinates can move");
    }

    #[test]
    fn chain_null_basis_and_report() {
        let mut s = System::new();
        let mut end = s.point([0.0, 0.0]);
        s.constrain(Constraint::Fix(end, [0.0, 0.0]));
        let mut lines = Vec::new();
        let headings = [0.0_f64, 6.0, 12.0, 18.0, 0.0, 0.0, 0.0, 90.0];
        for (i, degrees) in headings.iter().enumerate() {
            let a = degrees.to_radians();
            let [x, y] = s.at(end);
            let next = s.point([x + 4.0 * a.cos(), y + 4.0 * a.sin()]);
            let line = s.line(end, next);
            s.constrain(Constraint::Length(line, 4.0));
            if i == 0 {
                s.constrain(Constraint::Horizontal(line));
            } else if i != 4 && i != headings.len() - 1 {
                s.constrain(Constraint::Angle(lines[i - 1], line, a - headings[i - 1].to_radians()));
            }
            lines.push(line);
            end = next;
        }
        let scale = s.scale();
        let branches = s.branches();
        let (_, jac, _) = s.evaluate(&s.x, scale, &branches);
        let qr = DragQr::new(&jac, s.x.len());
        assert_eq!(qr.z.len(), 2, "the two free joints must give two tangent directions");
        check_null_basis(&qr, &jac, scale);

        let report = s.solve(&Options::default());
        assert_eq!(report.dof, 2, "the chain report must count both free joints");
        assert_eq!(&report.determined[..10], &[true; 10], "the first four links and their base are fixed");
        assert_eq!(&report.determined[10..], &[true, false, true, false, true, false, false, false],
            "the horizontal arm can swing vertically and the tip can also move sideways");
    }
    #[test]
    fn trial_cap_ends_one_move_and_the_drag_goes_on() {
        let opts = Options::default();
        let (base, end) = four_bar(&opts);
        let mirror = mirror_case(&base, end, &opts);
        let (arm, arm_end) = two_link(&opts);
        for (name, s0, end, target) in [("mirror four-bar", mirror, end, rocker(130.0)), ("two-link arm", arm, arm_end, [30.0, 0.0])] {
            let tol = opts.tolerance * s0.scale();
            let press_bend = elbow_offset(&s0.x, s0.points[end.0]);
            let mut free = s0.clone();
            let whole = free.start_drag(end, &opts).to(&mut free, target);
            assert_eq!(whole.outcome, DragOutcome::Nearest, "{name}: the uncapped move should settle at the limit");

            let mut s = s0.clone();
            let mut drag = s.start_drag(end, &opts);
            let cap = drag.frame.trial_cap;
            drag.frame.trial_cap = 1;
            let r = drag.to(&mut s, target);
            assert_eq!(r.outcome, DragOutcome::NotReached, "{name}: a round at the trial cap ends only this move");
            assert!(r.report.iterations < whole.report.iterations, "{name}: the capped move should end before the uncapped one");
            assert!(r.report.converged, "{name}: the capped move must keep its last accepted pose, which holds the constraints");
            assert!(keeps_side(&s, end, press_bend, tol), "{name}: the capped move flipped the elbow");

            drag.frame.trial_cap = cap;
            let r = drag.to(&mut s, target);
            assert_eq!(r.outcome, whole.outcome, "{name}: the next move of the same drag should finish the work");
            assert!(distance(s.at(end), free.at(end)) <= tol, "{name}: the drag after the cap should end where the uncapped move ends");
            assert!(keeps_side(&s, end, press_bend, tol), "{name}: the elbow flipped after the cap");
            assert_eq!(r.side_enforced, whole.side_enforced, "{name}: the cap must not change the press's side class");
        }
    }
}
