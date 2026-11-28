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
//! the press. Each [`DragReport`] gives the move's outcome, side enforcement and round
//! count, plus the whole sketch's residual, convergence and failing constraints.
//! At the press and release, [`Drag::report`] gives the full [`Report`] for the current pose.
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

#[cfg(test)]
pub(crate) mod work {
    use std::cell::Cell;

    #[derive(Clone, Copy, Default, PartialEq, Debug)]
    pub(crate) struct Work {
        pub(crate) evaluate: u64,
        pub(crate) determinant: u64,
        pub(crate) reduced_hessian: u64,
        pub(crate) hessian_tables: u64,
        pub(crate) band_crossings: u64,
        pub(crate) band_floor: u64,
        pub(crate) strip_undone: u64,
        pub(crate) negative_pred: u64,
        pub(crate) report_qr: u64,
        pub(crate) redundant: u64,
        pub(crate) qr: u64,
        pub(crate) start_qr: u64,
        pub(crate) trial_qr: u64,
        pub(crate) projection_qr: u64,
        pub(crate) eigen_ql_ops: u64,
        pub(crate) eigen_jacobi_ops: u64,
    }

    thread_local! {
        static COUNTS: Cell<Work> = Cell::new(Work::default());
    }

    pub(crate) fn add(f: impl FnOnce(&mut Work)) {
        COUNTS.with(|counts| {
            let mut work = counts.get();
            f(&mut work);
            counts.set(work);
        });
    }

    pub(crate) fn take() -> Work {
        COUNTS.with(Cell::take)
    }
}

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
    /// How the move ended relative to the target.
    pub outcome: DragOutcome,
    /// The drag keeps the sketch on the side (assembly mode) it had at the press. False where
    /// nothing separates the two sides, so a flip is not prevented there.
    pub side_enforced: bool,
    /// Drag rounds taken by this move.
    pub rounds: usize,
    /// Largest remaining residual in the whole sketch (sketch units).
    pub residual: f64,
    /// Every constraint in the whole sketch holds within the tolerance.
    pub converged: bool,
    /// Whole-sketch constraints that still fail (only when not converged).
    pub failing: Vec<usize>,
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
/// of one drag agree with each other. It moves the dragged point's constraint component
/// and holds the rest of the sketch at its current pose.
/// A session keeps the last pose's factorisation for a move starting at the same pose and scale.
#[derive(Clone, Debug)]
pub struct Drag {
    point: PointId,
    opts: Options,
    last_rounds: usize,
    last_scale: f64,
    /// The pose at the press, for `cancel`.
    press: Vec<f64>,
    /// The component on which the drag solves.
    sub: System,
    /// Whole-sketch unknown index for each unknown in the component.
    map: Vec<usize>,
    /// Whole-sketch constraint branches at the press, for reporting.
    branches: Vec<f64>,
    /// The press frame: σ_press, the branches and the spare class, held across the moves of one drag.
    frame: drag::PressFrame,
    /// The last move's end pose with its evaluation and QR, for the next move's start.
    kept: Option<drag::Kept>,
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
    /// Moves the dragged point toward `target` and returns the move's outcome, side enforcement,
    /// rounds and whole-sketch constraint readings. Use [`Self::report`] for the full report.
    pub fn to(&mut self, s: &mut System, target: P2) -> DragReport {
        if self.point.0 >= s.points.len() {
            self.last_rounds = 0;
            self.last_scale = s.scale();
            let tol = self.opts.tolerance * self.last_scale;
            let (r, _, rows) = self.evaluate_for_report(s);
            let (residual, converged, failing) = residual_status(&r, &rows, tol);
            return DragReport {
                outcome: DragOutcome::NotReached, side_enforced: self.frame.side_enforced(),
                rounds: self.last_rounds, residual, converged, failing,
            };
        }
        if !self.layout.matches(s) {
            eprintln!("constrain: the sketch changed during a drag; its press frame is rebuilt at the current pose");
            self.kept = None;
            self.branches = s.branches();
            (self.sub, self.map) = s.drag_component(self.point, &self.branches);
            self.frame = drag::PressFrame::new(&self.sub, self.point, s.scale());
            self.layout = Layout::of(s);
        }
        let scale = s.scale();
        let tol = self.opts.tolerance * scale;
        let end = if target.iter().all(|v| v.is_finite()) {
            for (x, &g) in self.sub.x.iter_mut().zip(&self.map) {
                *x = s.x[g];
            }
            let end = drag::drag_move(&mut self.sub, self.point, target, &self.opts, &mut self.frame, scale, &mut self.kept);
            for (&x, &g) in self.sub.x.iter().zip(&self.map) {
                s.x[g] = x;
            }
            end
        } else {
            drag::MoveEnd { rounds: 0, outcome: DragOutcome::NotReached }
        };
        self.last_rounds = end.rounds;
        self.last_scale = scale;
        let (r, _, rows) = s.evaluate(&s.x, scale, &self.branches);
        #[cfg(test)]
        work::add(|w| w.evaluate -= r.len() as u64 * s.x.len() as u64);
        let (residual, converged, failing) = residual_status(&r, &rows, tol);
        DragReport {
            outcome: end.outcome, side_enforced: self.frame.side_enforced(),
            rounds: self.last_rounds, residual, converged, failing,
        }
    }

    /// Full report for the current whole-sketch pose, at the last move's scale (the press
    /// scale before any move), so it agrees with that move's [`DragReport`].
    pub fn report(&self, s: &System) -> Report {
        let tol = self.opts.tolerance * self.last_scale;
        let (r, jac, rows) = self.evaluate_for_report(s);
        s.report_from(r, jac, rows, self.last_rounds, tol)
    }

    fn evaluate_for_report(&self, s: &System) -> (Vec<f64>, Jac, Vec<usize>) {
        // If the constraints changed, their current branches replace the stale row mapping.
        let current_branches;
        let branches = if self.layout.constraints == s.constraints {
            &self.branches
        } else {
            current_branches = s.branches();
            &current_branches
        };
        let (r, jac, rows) = s.evaluate(&s.x, self.last_scale, branches);
        #[cfg(test)]
        work::add(|w| w.evaluate -= r.len() as u64 * s.x.len() as u64);
        (r, jac, rows)
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
        let branches = self.branches();
        let (sub, map) = self.drag_component(p, &branches);
        let scale = self.scale();
        let frame = drag::PressFrame::new(&sub, p, scale);
        Drag {
            point: p, opts: *opts, last_rounds: 0, last_scale: scale,
            press: self.x.clone(), sub, map, branches, frame, kept: None, layout: Layout::of(self),
        }
    }

    #[cfg(test)]
    fn start_drag_whole(&self, p: PointId, opts: &Options) -> Drag {
        let branches = self.branches();
        let sub = self.clone();
        let map = (0..self.x.len()).collect();
        let scale = self.scale();
        let frame = drag::PressFrame::new(&sub, p, scale);
        Drag {
            point: p, opts: *opts, last_rounds: 0, last_scale: scale,
            press: self.x.clone(), sub, map, branches, frame, kept: None, layout: Layout::of(self),
        }
    }

    /// Stop at fixed points other than the dragged point, keeping those endpoints and their Fix rows.
    /// The null space has zero rows at a fixed point, so mechanisms joined only there move independently.
    /// A sketch that no fixed point splits gives `map = 0..n` and a sub-system equal to the sketch field by field.
    fn drag_component(&self, p: PointId, branches: &[f64]) -> (System, Vec<usize>) {
        fn root(parents: &mut [usize], mut node: usize) -> usize {
            while parents[node] != node {
                parents[node] = parents[parents[node]];
                node = parents[node];
            }
            node
        }

        // A point's x and y share a node. Each radius or other unknown starts on its own.
        let mut parents: Vec<_> = (0..self.x.len()).collect();
        for &x in &self.points {
            parents[x + 1] = x;
        }
        let mut fixed = vec![false; self.x.len()];
        for c in &self.constraints {
            if let Constraint::Fix(q, _) = *c {
                let x = self.points[q.0];
                fixed[x..x + 2].fill(q != p);
            }
        }
        let scale = self.scale();
        let mut locals = Vec::with_capacity(self.constraints.len());
        for (ci, c) in self.constraints.iter().enumerate() {
            let mut ctx = Ctx { x: &self.x, sys: self, locals: Vec::new() };
            let _ = ctx.residuals(c, scale, branches[ci]);
            let mut free = ctx.locals.iter().copied().filter(|&g| !fixed[g]);
            if let Some(first) = free.next() {
                let first_root = root(&mut parents, first);
                for g in free {
                    let other_root = root(&mut parents, g);
                    parents[other_root] = first_root;
                }
            }
            locals.push(ctx.locals);
        }

        let component = root(&mut parents, self.points[p.0]);
        let mut included = vec![false; self.x.len()];
        included[component] = true;
        for vars in &locals {
            if vars.iter().any(|&g| root(&mut parents, g) == component) {
                for &g in vars {
                    if fixed[g] {
                        included[root(&mut parents, g)] = true;
                    }
                }
            }
        }
        let mut map = Vec::new();
        let mut inverse = vec![usize::MAX; self.x.len()];
        for (g, index) in inverse.iter_mut().enumerate() {
            if included[root(&mut parents, g)] {
                *index = map.len();
                map.push(g);
            }
        }
        let sub = System {
            x: map.iter().map(|&g| self.x[g]).collect(),
            points: self.points.iter().map(|&g| inverse[g]).collect(),
            lines: self.lines.clone(),
            circles: self.circles.iter().map(|c| Circle { r: inverse[c.r], ..*c }).collect(),
            arcs: self.arcs.clone(),
            constraints: self.constraints.iter().zip(locals).filter_map(|(&c, vars)| {
                vars.iter().all(|&g| inverse[g] != usize::MAX).then_some(c)
            }).collect(),
        };
        (sub, map)
    }

    /// Drags `p` in one move and returns its [`DragReport`]. Use [`Self::start_drag`]
    /// and [`Drag::to`] for a session with a full report available from [`Drag::report`].
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

    fn report_from(&self, r: Vec<f64>, jac: Jac, rows: Vec<usize>, iterations: usize, tol: f64) -> Report {
        let n = self.x.len();
        let (residual, converged, failing) = residual_status(&r, &rows, tol);
        #[cfg(test)]
        work::add(|w| w.report_qr += 1);
        let qr = DragQr::new(&jac, n);
        let dof = n - qr.rank();
        let determined = (0..n).map(|j| qr.determined(j)).collect();
        let mut redundant = qr.redundant_rows(&jac);
        for row in &mut redundant {
            *row = rows[*row];
        }
        redundant.dedup();
        #[cfg(test)]
        assert_eq!(redundant, redundant_constraints(&jac.dense(), &rows, n), "the QR's redundant constraints must match the row-order sweep");
        Report { converged, iterations, residual, dof, determined, redundant, failing }
    }

    /// The weighted, damped min-norm Newton iteration of [`Self::solve_weighted`] without its
    /// report: the last residuals, Jacobian, each row's constraint and the iteration count.
    fn iterate_weighted(
        &mut self, w: &[f64], scale: f64, branches: &[f64], tol: f64, max_iterations: usize,
    ) -> (Vec<f64>, Jac, Vec<usize>, usize) {
        let n = self.x.len();
        let winv: Vec<f64> = w.iter().map(|w| 1.0 / w).collect();
        let (mut r, mut jac, mut rows) = self.evaluate(&self.x, scale, branches);
        let mut lambda = 1e-9;
        let mut iterations = 0;
        while iterations < max_iterations && max_abs(&r) > tol {
            iterations += 1;
            let m = r.len();
            let dense = jac.dense();
            // A = J W⁻¹ Jᵀ
            let mut a = vec![0.0; m * m];
            let mut max_diag: f64 = 0.0;
            for i in 0..m {
                for j in 0..=i {
                    let mut s = 0.0;
                    for k in 0..n {
                        s += dense[i * n + k] * winv[k] * dense[j * n + k];
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
                            s += dense[i * n + k] * y[i];
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

    /// Residuals, their Jacobian in compressed rows, and each row's constraint index.
    fn evaluate(&self, x: &[f64], scale: f64, branches: &[f64]) -> (Vec<f64>, Jac, Vec<usize>) {
        let n = x.len();
        let row_capacity = 2 * self.constraints.len();
        let mut r = Vec::with_capacity(row_capacity);
        let mut rows = Vec::with_capacity(row_capacity);
        let mut jac = Jac {
            n,
            start: Vec::with_capacity(row_capacity + 1),
            pairs: Vec::with_capacity(4 * row_capacity),
        };
        jac.start.push(0);
        let mut locals = Vec::with_capacity(8);
        for (ci, c) in self.constraints.iter().enumerate() {
            locals.clear();
            let mut ctx = Ctx { x, sys: self, locals };
            let out = ctx.residuals(c, scale, branches[ci]);
            for d in out.iter() {
                r.push(d.v);
                let base = jac.pairs.len();
                for (k, &var) in ctx.locals.iter().enumerate() {
                    jac.pairs.push((var, 0.0 + d.g[k]));
                }
                if !jac.pairs[base..].windows(2).all(|w| w[0].0 < w[1].0) {
                    jac.pairs[base..].sort_unstable_by_key(|&(var, _)| var);
                }
                jac.start.push(jac.pairs.len());
                rows.push(ci);
            }
            locals = ctx.locals;
        }
        #[cfg(test)]
        work::add(|w| w.evaluate += r.len() as u64 * n as u64);
        (r, jac, rows)
    }

    #[cfg(test)]
    fn evaluate_dense(&self, x: &[f64], scale: f64, branches: &[f64]) -> (Vec<f64>, Vec<f64>, Vec<usize>) {
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

/// Each row stores its constraint's locals sorted by unknown, including pairs with an exact zero.
/// Values accumulate as `0.0 + g` as in the dense matrix, so a −0 derivative is stored as +0.
/// Expanding with `dense()` reproduces the row-major matrix bit for bit.
#[derive(Clone, Debug)]
struct Jac {
    n: usize,
    start: Vec<usize>,
    pairs: Vec<(usize, f64)>,
}

impl Jac {
    fn rows(&self) -> usize {
        self.start.len() - 1
    }

    fn row(&self, i: usize) -> &[(usize, f64)] {
        &self.pairs[self.start[i]..self.start[i + 1]]
    }

    fn dense(&self) -> Vec<f64> {
        let mut dense = vec![0.0; self.rows() * self.n];
        for i in 0..self.rows() {
            let base = i * self.n;
            for &(var, value) in self.row(i) {
                dense[base + var] = value;
            }
        }
        dense
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
                // The radius does not enter. Reading it would make a ninth unknown when p is its own point.
                let ctr = self.pt(self.sys.circles[c.0].center);
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

fn residual_status(r: &[f64], rows: &[usize], tol: f64) -> (f64, bool, Vec<usize>) {
    let residual = max_abs(r);
    let converged = residual <= tol;
    let failing = if converged {
        Vec::new()
    } else {
        let mut f: Vec<usize> = rows.iter().zip(r).filter(|(_, v)| v.abs() > tol).map(|(c, _)| *c).collect();
        f.dedup();
        f
    };
    (residual, converged, failing)
}

/// Pivot cut of the drag's QR, relative to the first pivot (Q7).
const RANK_CUT: f64 = 1e-9;

/// Column-pivoted Householder QR of Jᵀ. The independent columns give the drag
/// its constraint normals Y. Each stores R above the tail of its reflector.
#[derive(Clone, Debug)]
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
    fn new(jac: &Jac, n: usize) -> Self {
        #[cfg(test)]
        work::add(|w| w.qr += 1);
        let m = if n == 0 { 0 } else { jac.rows() };
        let mut columns = Vec::with_capacity(m);
        for i in 0..m {
            let mut column = vec![0.0; n];
            for &(var, value) in jac.row(i) {
                column[var] = value;
            }
            columns.push(column);
        }
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

    /// Returns the redundant Jacobian rows in ascending order. The last significant row in a
    /// dependency is implied by earlier rows. Eliminating it from the other dependencies
    /// recovers those earlier-row relations regardless of the QR's pivot order.
    fn redundant_rows(&self, jac: &Jac) -> Vec<usize> {
        #[cfg(test)]
        work::add(|w| w.redundant += 1);
        let m = jac.rows();
        let rank = self.rank();
        if rank == m {
            return Vec::new();
        }
        if jac.n == 0 {
            return (0..m).collect();
        }
        let norms: Vec<_> = (0..m).map(|i| {
            jac.row(i).iter().map(|&(_, value)| value * value).sum::<f64>().sqrt()
        }).collect();
        let mut redundant = Vec::with_capacity(m - rank);
        let mut dependencies = Vec::new();
        for j in rank..m {
            let d = self.order[j];
            if norms[d] == 0.0 {
                redundant.push(d);
                continue;
            }
            // Back-substitute the top of column j through R11 into y in the original row order.
            let mut y = vec![0.0; m];
            for k in (0..rank).rev() {
                let sum = (k + 1..rank).map(|l| self.columns[l][k] * y[self.order[l]]).sum::<f64>();
                y[self.order[k]] = (self.columns[j][k] - sum) / self.columns[k][k];
            }
            y[d] = -1.0;
            dependencies.push((d, y));
        }

        let highest = |d: usize, y: &[f64]| {
            let cut = 1e-8 * norms[d];
            // Each remaining vector has -1 at its own d and zero at the other remaining d's.
            (d + 1..m).rev().find(|&i| y[i].abs() * norms[i] >= cut).unwrap_or(d)
        };
        while let Some((slot, h, _)) = dependencies.iter().enumerate()
            .map(|(slot, (d, y))| (slot, highest(*d, y), *d))
            .max_by_key(|&(_, h, d)| (h, std::cmp::Reverse(d)))
        {
            let (_, pivot) = dependencies.swap_remove(slot);
            redundant.push(h);
            for (_, y) in &mut dependencies {
                if y[h] != 0.0 {
                    let factor = y[h] / pivot[h];
                    for (entry, &p) in y.iter_mut().zip(&pivot) {
                        *entry -= factor * p;
                    }
                    y[h] = 0.0;
                }
            }
        }
        redundant.sort_unstable();
        redundant
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

/// Row-order Gram-Schmidt reference for `DragQr::redundant_rows`, with rows mapped to constraints.
#[cfg(test)]
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

        let opts = Options::default();
        let mut base = System::new();
        let (p, l) = quad(&mut base);
        let width = Constraint::Length(l[0], 40.0);
        let height = Constraint::Length(l[1], 20.0);
        for c in [Constraint::Fix(p[0], [0.0, 0.0]), Constraint::Horizontal(l[0]),
                  Constraint::Vertical(l[1]), Constraint::Horizontal(l[2]), Constraint::Vertical(l[3]),
                  width, height] {
            base.constrain(c);
        }
        let mut s = base.clone();
        let r = s.solve(&opts);
        assert!(r.converged, "q=0 rectangle must converge");
        assert!(r.redundant.is_empty(), "q=0 rectangle must have no redundant constraints");
        assert_eq!(r.dof, 0, "q=0 rectangle must have no free direction");

        for (case, position) in [("first", 0), ("middle", 3), ("last", base.constraints.len())] {
            let mut copy = base.clone();
            copy.constraints.insert(position, width);
            let later = copy.constraints.iter().rposition(|&c| c == width).unwrap();
            assert_eq!(copy.solve(&opts).redundant, [later],
                "with the duplicate placed {case}, only the later width constraint must be redundant");
        }

        let mut extras = Vec::new();
        for c in [width, height, Constraint::Parallel(l[0], l[2])] {
            extras.push(s.constrain(c));
            assert_eq!(s.solve(&opts).redundant, extras,
                "q={} rectangle must report all extras in insertion order", extras.len());
        }

        let zero = base.constrain(Constraint::Coincident(p[1], p[1]));
        assert_eq!(base.solve(&opts).redundant, [zero], "the zero rows of a self-coincidence must report its id once");
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
        // Two half circles joined by tangent lines: a stadium of width 2r. With `shared` false the
        // arcs have their own end points, held on the lines' ends by Coincident, as a sketch draws them.
        for shared in [true, false] {
            let mut s = System::new();
            let c1 = s.point([0.0, 0.0]);
            let c2 = s.point([28.0, 1.0]);
            let (p1, p2, p3, p4) = (s.point([0.0, -5.5]), s.point([29.0, -4.0]), s.point([27.0, 6.0]), s.point([1.0, 5.0]));
            let [e1, e2, e3, e4] = if shared { [p1, p2, p3, p4] } else {
                [p1, p2, p3, p4].map(|p| {
                    let at = s.at(p);
                    let e = s.point(at);
                    s.constrain(Constraint::Coincident(e, p));
                    e
                })
            };
            let a1 = s.arc(c2, e2, e3);
            let a2 = s.arc(c1, e4, e1);
            let (bottom, top) = (s.line(p1, p2), s.line(p3, p4));
            let (k1, k2) = (s.arcs[a1.0].circle, s.arcs[a2.0].circle);
            s.constrain(Constraint::Fix(c1, [0.0, 0.0]));
            s.constrain(Constraint::HorizontalPoints(c1, c2));
            s.constrain(Constraint::Distance(c1, c2, 30.0));
            s.constrain(Constraint::EqualRadius(k1, k2));
            s.constrain(Constraint::Radius(k1, 5.0));
            s.constrain(Constraint::TangentAt(bottom, k2, e1));
            s.constrain(Constraint::TangentAt(bottom, k1, e2));
            s.constrain(Constraint::TangentAt(top, k1, e3));
            s.constrain(Constraint::TangentAt(top, k2, e4));
            let r = s.solve(&Options::default());
            assert!(r.well_constrained(), "shared={shared}: {r:?}");
            assert!((s.at(p1)[1] + 5.0).abs() < 1e-7 && (s.at(p2)[1] + 5.0).abs() < 1e-7, "shared={shared}: the bottom line at y = -5");
            assert!((s.at(p3)[1] - 5.0).abs() < 1e-7 && (s.at(p4)[1] - 5.0).abs() < 1e-7, "shared={shared}: the top line at y = 5");
            assert!(close(s.at(c2), [30.0, 0.0]), "shared={shared}: the second centre at 30");
        }
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
        assert!(r.converged, "the free-corner move must satisfy the rectangle constraints");
        assert_eq!(r.outcome, DragOutcome::Reached, "the free corner should reach the pointer");
        assert!(close(s.at(p[2]), [55.0, 31.0]), "the free corner missed the pointer at {:?}", s.at(p[2]));
        assert!(close(s.at(p[1]), [55.0, 0.0]) && close(s.at(p[3]), [0.0, 31.0]), "the adjacent corners must follow the free corner");
        // A dimensioned side cannot follow: the drag settles nearest.
        s.constrain(Constraint::Length(l[0], 40.0));
        let mut drag = s.start_drag(p[2], &Options::default());
        work::take();
        let r = drag.to(&mut s, [60.0, 25.0]);
        let move_work = work::take();
        assert_eq!(move_work.report_qr, 0, "a move must skip the full report's QR");
        assert_eq!(move_work.redundant, 0, "a move must skip the redundant-row sweep");
        assert!(r.converged, "the dimensioned-side move must satisfy the rectangle constraints");
        assert_eq!(r.outcome, DragOutcome::Nearest, "the dimensioned side holds the corner at its nearest pose");
        assert!(close(s.at(p[1]), [40.0, 0.0]), "the dimensioned side must end at its specified length");
        assert!((s.at(p[2])[1] - 25.0).abs() < 1e-6, "the dragged corner missed the pointer's height at {:?}", s.at(p[2]));

        let report = drag.report(&s);
        let report_work = work::take();
        assert_eq!(report.converged, r.converged, "the full report and move must agree on convergence");
        assert_eq!(report.failing, r.failing, "the full report and move must name the same failing constraints");
        assert_eq!(report.iterations, r.rounds, "the full report must carry the last move's round count");
        assert_eq!(report.residual.to_bits(), r.residual.to_bits(), "the full report and move residuals must match bit for bit");
        assert_eq!(report_work.report_qr, 1, "requesting the full report must build its QR once");
        assert_eq!(report_work.redundant, 1, "requesting the full report must find the redundant rows once");
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
    fn spectators_leave_the_drag_bit_for_bit() {
        fn mechanism_bits(s: &System) -> [u64; 8] {
            std::array::from_fn(|i| s.x[i].to_bits())
        }

        let opts = Options::default();
        let mut baseline = None;
        for size in [0, 1, 200] {
            let (mut s, end) = four_bar(&opts);
            let mut free = Vec::new();
            for i in 0..size {
                let start = [0.5 + 0.2 * (i % 20) as f64, 0.05 * (i / 20) as f64];
                free.push((s.point(start), start));
            }
            let mut drag = s.start_drag(end, &opts);
            work::take();
            let mut records = Vec::new();
            for degrees in [20.0, 40.0, 60.0, 80.0, 100.0, 120.0, 130.0] {
                let r = drag.to(&mut s, rocker(degrees));
                records.push((mechanism_bits(&s), r.outcome, r.rounds, r.side_enforced));
                for &(p, start) in &free {
                    assert_eq!(s.at(p), start, "with {size} spectators {p:?} moved at {degrees}°");
                }
            }
            let counts = work::take();
            if let Some((base_records, base_counts)) = &baseline {
                assert_eq!(&records, base_records,
                    "with {size} spectators the pose bits, outcome, iterations and side enforcement must match the bare four-bar");
                assert_eq!(&counts, base_counts, "with {size} spectators the solver work must match the bare four-bar");
            } else {
                baseline = Some((records, counts));
            }

            if size > 0 {
                let before = mechanism_bits(&s);
                let mut free_drag = s.start_drag(free[0].0, &opts);
                let r = free_drag.to(&mut s, [2.0, 0.25]);
                assert_eq!(r.outcome, DragOutcome::Reached, "with {size} spectators the free point must reach its target");
                assert_eq!(mechanism_bits(&s), before, "with {size} spectators dragging the free point moved the four-bar");
            }
        }
    }

    fn attach_arm(s: &mut System, joint: PointId, j: usize, links: usize) -> Vec<(PointId, P2)> {
        let mut points = Vec::with_capacity(links);
        let mut previous = joint;
        for l in 0..links {
            // Integer tenths preserve the original two heights, 0.1 and 0.3, exactly.
            let start = [0.5 + 0.5 * l as f64 + 0.4 * j as f64, (1 + 2 * l) as f64 / 10.0];
            let point = s.point(start);
            let line = s.line(previous, point);
            let length = distance(s.at(previous), s.at(point));
            s.constrain(Constraint::Length(line, length));
            points.push((point, start));
            previous = point;
        }
        points
    }

    #[test]
    fn a_fixed_point_separates_the_mechanisms_on_it() {
        let opts = Options::default();
        let mut baseline = None;
        for arms in [0, 1, 8] {
            let (mut s, end) = four_bar(&opts);
            let ground = PointId(0);
            let mut arm_points = Vec::new();
            for j in 0..arms {
                arm_points.extend(attach_arm(&mut s, ground, j, 2));
            }
            let mut drag = s.start_drag(end, &opts);
            assert_eq!(drag.sub.x.len(), 8, "with {arms} extra arms the drag should contain only the four-bar");
            work::take();
            let mut records = Vec::new();
            for degrees in [20.0, 40.0, 60.0, 80.0, 100.0, 120.0, 130.0] {
                let r = drag.to(&mut s, rocker(degrees));
                let bits: [u64; 8] = std::array::from_fn(|i| s.x[i].to_bits());
                records.push((bits, r.outcome, r.rounds, r.side_enforced));
                for &(p, start) in &arm_points {
                    assert_eq!(s.at(p), start, "with {arms} extra arms {p:?} moved at {degrees}°");
                }
            }
            let counts = work::take();
            if let Some((base_records, base_counts)) = &baseline {
                assert_eq!(&records, base_records,
                    "with {arms} extra arms the pose bits, outcome, iterations and side enforcement must match the bare four-bar");
                assert_eq!(&counts, base_counts, "with {arms} extra arms solver work must match the bare four-bar");
            } else {
                baseline = Some((records, counts));
            }
        }
    }

    #[test]
    fn a_free_shared_point_keeps_both_sides() {
        let opts = Options::default();
        let (base, end) = four_bar(&opts);
        let elbow = PointId(2);
        let sweep = [20.0, 40.0, 60.0, 80.0, 100.0, 120.0, 130.0];
        let pose_bits = |s: &System| s.x.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
        for (fixture, joint, links, unknowns) in [
            ("two links on the elbow", elbow, 2, 12),
            ("three links on the elbow", elbow, 3, 14),
            ("two links on the dragged end", end, 2, 12),
        ] {
            work::take();
            let mut s = base.clone();
            let arm = attach_arm(&mut s, joint, 0, links);
            let press = s.clone();
            let pin = press.at(end);
            let mut plain = Vec::new();
            let mut drag = s.start_drag(end, &opts);
            assert_eq!(drag.sub.x.len(), unknowns, "the component must include the four-bar and {fixture}");
            let mut negative_pred = 0;
            for degrees in sweep {
                let target = rocker(degrees);
                let tol = opts.tolerance * s.scale();
                let r = drag.to(&mut s, target);
                let w = work::take();
                negative_pred += w.negative_pred;
                assert!(w.strip_undone <= 2 * r.rounds as u64,
                    "with {fixture} the move to {degrees}° undid the strip on {} trials in {} rounds: a radius far beyond the sketch", w.strip_undone, r.rounds);
                assert!(matches!(r.outcome, DragOutcome::Reached | DragOutcome::Nearest),
                    "with {fixture} the move to {degrees}° should settle, got {:?}", r.outcome);
                assert!(r.converged, "with {fixture} the constraints must hold at {degrees}°");
                if degrees < 130.0 {
                    assert!(distance(s.at(end), target) <= tol, "with {fixture} the dragged point missed {degrees}°");
                }
                plain.push((pose_bits(&s), r.outcome, r.side_enforced));
            }
            for (p, start) in arm {
                assert_ne!(s.at(p), start, "with {fixture} arm point {p:?} must follow the joint");
            }
            assert_eq!(negative_pred, 0, "with {fixture} a trial the judge sees must predict a gain");

            let mut s = press.clone();
            let mut drag = s.start_drag(end, &opts);
            let r = drag.to(&mut s, pin);
            assert_eq!(r.outcome, DragOutcome::Reached, "with {fixture} a zero first pull must reach the press position");
            assert!(!r.side_enforced, "with {fixture} a zero first pull must classify the spare as coupled");
            for (degrees, (pose, outcome, side)) in sweep.into_iter().zip(plain) {
                let r = drag.to(&mut s, rocker(degrees));
                assert_eq!(pose_bits(&s), pose, "with {fixture} a zero first pull changed the pose bits at {degrees}°");
                assert_eq!(r.outcome, outcome, "with {fixture} a zero first pull changed the outcome at {degrees}°");
                assert_eq!(r.side_enforced, side, "with {fixture} a zero first pull changed side enforcement at {degrees}°");
            }

            let mut s = press;
            let mut drag = s.start_drag(end, &opts);
            let radius = pin[0].hypot(pin[1]);
            let target = [pin[0] + 1e-3 * pin[0] / radius, pin[1] + 1e-3 * pin[1] / radius];
            let r = drag.to(&mut s, target);
            assert!(!r.side_enforced, "with {fixture} a normal first pull must classify the spare as coupled");
            for degrees in sweep {
                let r = drag.to(&mut s, rocker(degrees));
                assert!(!r.side_enforced, "with {fixture} the spare must stay coupled at {degrees}° after a normal first pull");
            }
        }
    }

    #[test]
    fn component_drag_agrees_with_the_whole_sketch_drag() {
        let opts = Options::default();
        let compare = |fixture: &str, size: usize, mut s: System, end: PointId, targets: &[P2]| {
            let mechanism_len = s.x.len() - 2 * size;
            let mut whole = s.clone();
            let mut drag = s.start_drag(end, &opts);
            let mut whole_drag = whole.start_drag_whole(end, &opts);
            for (i, &target) in targets.iter().enumerate() {
                let step = i + 1;
                let tol = opts.tolerance * s.scale();
                let r = drag.to(&mut s, target);
                let reference = whole_drag.to(&mut whole, target);
                assert_eq!(r.outcome, reference.outcome, "{fixture} with {size} spectators: outcome at move {step}");
                assert_eq!(r.side_enforced, reference.side_enforced,
                    "{fixture} with {size} spectators: side enforcement at move {step}");
                for g in 0..mechanism_len {
                    assert!((s.x[g] - whole.x[g]).abs() <= 10.0 * tol,
                        "{fixture} with {size} spectators: unknown {g} at move {step} differs ({} vs {}, tol {tol})", s.x[g], whole.x[g]);
                }
            }
        };

        let sweep = [20.0, 40.0, 60.0, 80.0, 100.0, 120.0, 130.0].map(rocker);
        for size in [1, 4, 10] {
            let (mut s, end) = four_bar(&opts);
            for i in 0..size {
                s.point([10.0 + i as f64, 10.0]);
            }
            compare("four-bar", size, s, end, &sweep);
        }
        let (mut s, end) = three_link();
        s.point([30.0, 30.0]);
        let targets: Vec<_> = (1..=60).map(|k| [22.0 - 10.0 * k as f64 / 60.0, 6.0 * k as f64 / 60.0]).collect();
        compare("three-link arm", 1, s, end, &targets);
    }

    #[test]
    fn one_component_sketch_is_its_own_sub_system() {
        let opts = Options::default();
        let (chain, end, _) = coupled_chain(&opts);
        for (name, (s, p)) in [("four-bar", four_bar(&opts)), ("two-link arm", two_link(&opts)), ("coupled chain", (chain, end))] {
            let drag = s.start_drag(p, &opts);
            assert_eq!(drag.sub.x, s.x, "{name} unknowns");
            assert_eq!(drag.sub.points, s.points, "{name} points");
            assert_eq!(drag.sub.lines, s.lines, "{name} lines");
            assert_eq!(drag.sub.circles, s.circles, "{name} circles");
            assert_eq!(drag.sub.arcs, s.arcs, "{name} arcs");
            assert_eq!(drag.sub.constraints, s.constraints, "{name} constraints");
            assert_eq!(drag.map, (0..s.x.len()).collect::<Vec<_>>(), "{name} identity map");
        }
    }

    #[test]
    fn free_points_add_no_band_floor_work() {
        let opts = Options::default();
        for size in [0, 10, 40] {
            let (mut s, end) = four_bar(&opts);
            let mut free = Vec::new();
            for i in 0..size {
                let start = [(i % 10) as f64 * 0.5, (i / 10) as f64];
                free.push((s.point(start), start));
            }
            work::take();
            let mut drag = s.start_drag(end, &opts);
            let mut rounds = 0;
            let mut reached = 0;
            for degrees in [20.0, 40.0, 60.0, 80.0, 100.0, 120.0] {
                let r = drag.to(&mut s, rocker(degrees));
                assert!(matches!(r.outcome, DragOutcome::Reached | DragOutcome::Nearest),
                    "with {size} free points the move to {degrees}° should settle, got {:?}", r.outcome);
                assert!(r.converged, "with {size} free points the constraints must hold at {degrees}°");
                rounds += r.rounds as u64;
                if r.outcome == DragOutcome::Reached { reached += 1; }
                for &(p, start) in &free {
                    assert_eq!(s.at(p), start, "with {size} free points {p:?} moved at {degrees}°");
                }
            }
            let counts = work::take();
            assert_eq!(counts.band_floor, 0, "with {size} free points no move should need the cofactor sum");
            assert_eq!(counts.hessian_tables, rounds - reached,
                "with {size} free points each round must build one curvature table except the last round of a Reached move");
        }
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
                assert!(r.converged, "four-bar constraints failed at {degrees}°");
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
                assert!(r.converged, "the near-toggle move to {target:?} lost a constraint");
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
            assert!(r.converged, "the exact toggle must satisfy all four-bar constraints");
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
        assert!(r.converged, "the limit pose must satisfy the four-bar constraints");
        assert!(distance(s.at(end), rocker(120.0)) <= tol, "the pin must stop at the 120° toggle");

        let target = inside_toggle(1e-9);
        let tol = opts.tolerance * s.scale();
        let r = drag.to(&mut s, target);
        assert_eq!(r.outcome, DragOutcome::Reached, "the pin should reach the target just inside the toggle");
        assert!(r.converged, "the return from the toggle must satisfy the constraints");
        assert!(distance(s.at(end), target) <= tol / 2.0, "the returning pin missed its target");
        assert!(keeps_side(&s, end, press_bend, tol), "the elbow flipped on the return from the toggle");
        assert!(r.rounds <= 4,
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
        assert!(r.converged, "the limit pose must satisfy the constraints before shifting the elbow");

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
        assert!(r.converged, "the return from the offset start must satisfy the constraints");
        assert!(distance(s.at(end), target) <= tol, "the pin missed its target from the offset start");
        assert!(keeps_side(&s, end, press_bend, tol), "the elbow flipped on the return from the offset start");
        assert!(r.rounds <= 4,
            "rule (b) should answer the return from the offset start in about 2 rounds (the probe's start took 16 without it)");
    }

    #[test]
    fn toggle_form_answers_a_return_from_the_exact_toggle() {
        work::take();
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
        assert!(r.converged, "the return must satisfy the mirrored four-bar constraints");
        assert!(distance(s.at(end), target) <= tol / 2.0, "the mirrored pin missed its return target");
        assert!(keeps_side(&s, end, press_bend, tol), "the mirrored elbow flipped on the return");
        assert!(r.rounds <= 2,
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
        assert!(r.converged, "the return must satisfy the two-link constraints");
        assert!(distance(s.at(end), target) <= tol / 2.0, "the two-link pin missed its return target");
        assert!(keeps_side(&s, end, press_bend, tol), "the two-link elbow flipped on the return");
        assert!(r.rounds <= 4,
            "the toggle form of rule (b) answers the return from the exact toggle in 4 rounds (5 without it)");
        let counts = work::take();
        assert!(counts.band_crossings > 0 && counts.band_floor == 0,
            "the returns cross the sheet in refused trials, and the screen keeps the cofactor sum off them");
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
                        assert!(result.converged, "arm constraints failed at move {k}/{moves}, d={d}");
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
            assert!(r.converged, "the coupled chain lost a constraint at move {k}");
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
            assert!(r.converged, "the chain lost a constraint at k={k}");
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
        assert!(r.converged, "the return must satisfy the coupled chain constraints");
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
        assert!(r.converged, "the open chain must keep its constraints when dragged to the centre");
        assert!((distance(s.at(end), centre) - 4.0).abs() <= tol, "the pin left its circle around the centre");
        assert!(distance(s.at(end), start) <= tol, "the pin should stay where it started on a continuum of nearest poses");
    }

    #[test]
    fn three_link_reaches_with_a_spare_freedom() {
        let opts = Options::default();
        let (mut s, end) = three_link();
        let spare = s.point([30.0, 30.0]);
        {
            let mut copy = s.clone();
            let mut drag = copy.start_drag(end, &opts);
            let target = copy.at(end);
            for (state, tables) in [("unjudged", 1), ("judged", 0)] {
                work::take();
                let r = drag.to(&mut copy, target);
                let counts = work::take();
                assert_eq!(r.outcome, DragOutcome::Reached, "the stationary pin must be Reached when the spare is {state}");
                assert_eq!(r.rounds, 1, "the stationary move must finish in one round when the spare is {state}");
                assert_eq!(counts.hessian_tables, tables, "the stationary move has the wrong curvature table count when the spare is {state}");
            }
        }
        let mut drag = s.start_drag(end, &opts);
        for k in 1..=60 {
            let target = [22.0 - 10.0 * k as f64 / 60.0, 6.0 * k as f64 / 60.0];
            let tol = opts.tolerance * s.scale();
            work::take();
            let r = drag.to(&mut s, target);
            let counts = work::take();
            if r.outcome == DragOutcome::Reached {
                assert_eq!(counts.hessian_tables, r.rounds as u64 - 1,
                    "Reached move {k} must build a curvature table in every round except its last");
            }
            assert!(r.converged, "the three-link arm lost a constraint at move {k}");
            assert!(!r.side_enforced, "the three-link arm's spare freedom couples to its fold, so no sheet separates its sides");
            // Some moves stop on the pin rule up to 2/3 tol short of the target, as the reference does.
            assert!(matches!(r.outcome, DragOutcome::Reached | DragOutcome::Nearest),
                "the three-link arm should settle at target {k}");
            assert!(distance(s.at(end), target) <= tol, "the three-link pin missed target {k}");
            assert_eq!(s.at(spare), [30.0, 30.0], "the unrelated point moved at target {k}");
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
            assert!(r.converged, "the line lost tangency at move {k}");
            assert!(matches!(r.outcome, DragOutcome::Reached | DragOutcome::Nearest),
                "the tangent endpoint failed to settle at move {k}");
            assert!(side(&s) * press_side > 0.0, "the centre must stay on its press side of the tangent at move {k}");
        }
    }

    #[test]
    fn stale_drag_session_rebuilds() {
        fn compare_move(drag: &mut Drag, s: &mut System, k: usize) -> (DragOutcome, work::Work) {
            let target = rocker(20.0 + k as f64);
            let mut cold = drag.clone();
            cold.kept = None;
            let mut copy = s.clone();
            let expected = cold.to(&mut copy, target);

            work::take();
            let r = drag.to(s, target);
            let counts = work::take();
            assert_eq!(s.x.len(), copy.x.len(), "move {k} must preserve the cold pose's length");
            for (i, (x, y)) in s.x.iter().zip(&copy.x).enumerate() {
                assert_eq!(x.to_bits(), y.to_bits(), "move {k} must match the cold pose bits at unknown {i}");
            }
            assert_eq!(r.outcome, expected.outcome, "move {k} must match the cold outcome");
            assert_eq!(r.rounds, expected.rounds, "move {k} must match the cold round count");
            (r.outcome, counts)
        }

        let opts = Options::default();
        let (mut s, end) = four_bar(&opts);
        let elbow = PointId(end.0 - 1);
        let press = s.at(end);
        let mut drag = s.start_drag(end, &opts);
        for k in 0..60 {
            let (outcome, counts) = compare_move(&mut drag, &mut s, k);
            // Reached asks for half the tolerance; a move that stops between half and the whole
            // tolerance reads Nearest (move 42 here), so the series checks the miss itself.
            let miss = distance(s.at(end), rocker(20.0 + k as f64));
            assert!(outcome != DragOutcome::NotReached && miss <= opts.tolerance * s.scale(),
                "move {k} should end within the tolerance of its rocker target ({outcome:?}, miss {miss:e})");
            if k == 0 {
                assert_eq!(counts.start_qr, 1, "the first move must build one round-start QR");
            } else {
                assert_eq!(counts.start_qr, 0, "move {k} should reuse the preceding move's QR");
                assert_eq!(counts.qr, counts.trial_qr + counts.projection_qr, "move {k} should factor only trials and projections");
            }
        }
        let pin_x = s.points[end.0];
        s.x[pin_x] = f64::from_bits(s.x[pin_x].to_bits() + 1);
        let (_, counts) = compare_move(&mut drag, &mut s, 60);
        assert_eq!(counts.start_qr, 1, "a one-ulp pin edit must cause a fresh round-start QR");
        drag.cancel(&mut s);

        s.constrain(Constraint::Fix(elbow, s.at(elbow)));
        let tol = opts.tolerance * s.scale();
        work::take();
        let r = drag.to(&mut s, rocker(20.0));
        assert_eq!(work::take().start_qr, 1, "the first move on the changed layout must build a fresh round-start QR");
        assert!(r.converged, "the rebuilt frame must satisfy the added elbow constraint");
        assert_eq!(r.outcome, DragOutcome::Nearest, "the fixed elbow should hold the pin at its nearest pose");
        assert!(distance(s.at(end), press) <= tol, "the newly constrained pin moved away from its press");
        let r = drag.to(&mut s, press);
        assert!(r.converged, "the rebuilt frame must keep its constraints on a second move");
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

    fn bits(values: &[f64]) -> Vec<u64> {
        values.iter().map(|value| value.to_bits()).collect()
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
        let (r, jac, rows) = s.evaluate(&s.x, scale, &branches);
        let (reference_r, reference_jac, reference_rows) = s.evaluate_dense(&s.x, scale, &branches);
        let dense = jac.dense();
        assert_eq!(bits(&r), bits(&reference_r), "rectangle residuals must match the dense evaluation bit for bit");
        assert_eq!(rows, reference_rows, "rectangle rows must have the same constraint indices as the dense evaluation");
        assert_eq!(bits(&dense), bits(&reference_jac), "rectangle Jacobian entries must match the dense evaluation bit for bit");
        assert!(jac.pairs.iter().any(|&(_, value)| value.to_bits() == 0), "the rectangle must store a positive-zero derivative among its row pairs");
        let qr = DragQr::new(&jac, s.x.len());
        assert_eq!(r.len(), 9, "redundant rectangle has nine scalar rows");
        assert_eq!(qr.rank(), 7, "redundant rectangle has seven independent rows");
        assert_eq!(qr.z.len(), 1, "rectangle has one free height");
        check_null_basis(&qr, &dense, scale);

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
        let (r, jac, rows) = s.evaluate(&s.x, scale, &branches);
        let (reference_r, reference_jac, reference_rows) = s.evaluate_dense(&s.x, scale, &branches);
        let dense = jac.dense();
        assert_eq!(bits(&r), bits(&reference_r), "chain residuals must match the dense evaluation bit for bit");
        assert_eq!(rows, reference_rows, "chain rows must have the same constraint indices as the dense evaluation");
        assert_eq!(bits(&dense), bits(&reference_jac), "chain Jacobian entries must match the dense evaluation bit for bit");
        let qr = DragQr::new(&jac, s.x.len());
        assert_eq!(qr.z.len(), 2, "the two free joints must give two tangent directions");
        check_null_basis(&qr, &dense, scale);

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
            assert!(r.rounds < whole.rounds, "{name}: the capped move should end before the uncapped one");
            assert!(r.converged, "{name}: the capped move must keep its last accepted pose, which holds the constraints");
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
