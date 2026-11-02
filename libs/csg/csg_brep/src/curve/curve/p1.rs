//! `SmCurve`, part 1: the global point solver (`GlobalPointSolve`) with the local
//! solve of its curve/point global solver, the Hermite approximation of a curve by a
//! piecewise cubic B-spline (`ApproximateCurve`), tessellation by bisection, the
//! classification of a curve/curve range intersection, and the refinement of range
//! intersections.
//!
//! Upstream: `source/SMLib/src/SmCurve.cpp`, `source/SMLib/inc/SmCurve.h`.
//!
//! | Rust                                              | Upstream                                         |
//! |---------------------------------------------------|--------------------------------------------------|
//! | [`refine_cc_solutions`]                           | `sm_RefineCCSolutions` (static)                  |
//! | [`test_span_accuracy`], [`SpanAccuracy`]          | `sm_TestSpanAccuracy` (static)                   |
//! | [`Curve::approximate_curve`], [`Curve::approximate_curve_base`], [`CurveApproximation`], [`ApproximateCurveOptions`] | `ApproximateCurve` (virtual: the `SmCrvOnSurf` override), `SmCurve::ApproximateCurve` |
//! | [`Curve::classify_intersecting_range_solution`], [`TangentPoint`] | `SmCurve::ClassifyIntersectingRangeSolution` |
//! | `CpGlobalSolver::local_solve_branch` ([`GlobalSolve::local_solve`] of [`CpGlobalSolver`]) | `SmCPGlobalSolver::LocalSolve` |
//! | [`Curve::global_point_solve`], [`GlobalPointSolveOptions`] | `SmCurve::GlobalPointSolve`             |
//! | `Curve::global_point_solve_cached` (crate)        | `SmCurve::GlobalPointSolve` (caller's cache)     |
//! | [`Curve::tessellate_by_bisection`]                | `SmCurve::TessellateByBisection`                 |
//! | [`Curve::tessellate`], [`Curve::tessellate_base`] | `Tessellate` (virtual), `SmCurve::Tessellate`    |
//!
//! Deviations:
//! - **Out-params.** Outputs are returned: [`SpanAccuracy`], [`CurveApproximation`]
//!   (whose `achieved_tol` is `None` where upstream leaves `rdAchievedTol` unwritten,
//!   the degenerate curve), [`TangentPoint`] (written only for a range classified as
//!   degenerate), a [`Tessellation`] (all three arrays; upstream fills the ones it is
//!   given, each independently, and fails with `SM_ERR_INVALID_INPUT` when none is),
//!   and a [`SolutionArray`] for `GlobalPointSolve`.
//! - **Objects.** Solutions name the caller's handles: `GlobalPointSolve` takes the
//!   handle of the curve (named by the line drop and the tree solutions, upstream
//!   `this`) and of its analytic replacement ([`GlobalPointSolveOptions`]; upstream
//!   the cache's `SmLine` / `SmCircle`). `sm_RefineCCSolutions` resolves solution
//!   objects to curves through a caller-supplied lookup (upstream casts the object
//!   pointers; a failing cast is `SM_ERR_NULL_POINTER`).
//! - **Caches.** `GlobalPointSolve` builds the curve's cache with
//!   [`make_curve_cache`] (co-ported from unit `curve.curve_p5`) instead of looking it
//!   up in the cache store (see there); a failed build is `SM_ERR_NULL_POINTER`, as
//!   upstream's `NER` on the missing cache. The cache tree's owner object is the
//!   curve's handle, and [`CpGlobalSolver`] carries the curve
//!   ([`CpGlobalSolver::curve`]) that upstream reads back from the tree's cache.
//! - **Persistent solution.** `SmCPGlobalSolver::LocalSolve` reuses one
//!   `SmSolution` across its guesses: [`Curve::local_point_solve`] returns a fresh
//!   solution, merged into the persistent one exactly as upstream writes it (type,
//!   value and first parameter when an answer is found; the whole solution for the
//!   ray operations).
//! - **Ignored statuses.** Where upstream ignores a status (the span end
//!   evaluations and the closed-curve continuity of `LocalSolve`, `SolveIt` in
//!   `GlobalPointSolve`, `GetKnots` in `TessellateByBisection`, the local intersect
//!   of `sm_RefineCCSolutions`, the local solves and evaluations of
//!   `ClassifyIntersectingRangeSolution`, `Copy` and `EditParameterization` in
//!   `ApproximateCurve`, `AngleBetween`) the error is ignored and upstream's value
//!   kept (the previous one, `0` for a failed angle, [`Vec3::UNDEF`] for a
//!   never-written `SmVector3d`), except [`SolidError::Unported`], which is always
//!   propagated. `sm_GetProjDist` returns a failing status as the distance
//!   upstream; that status is the solution value here too.
//! - **Undefined behaviour.** An `ApproximateCurve` that builds no curve (upstream
//!   dereferences the null curve) and a failing `Copy` of a B-spline in the
//!   just-copy branch (upstream returns a null curve with success) are errors.
//!   Option vectors an operation reads without a `NULL` check are `InvalidInput`
//!   when missing.
//! - **Approximation work.** The Hermite approximation keeps upstream's depth-first
//!   span order, but stops with `SM_ERR` after 8192 subdivisions or when the native
//!   midpoint formula cannot produce a finite, distinct parameter. The subdivision
//!   count is total across break intervals: upstream's natural-interval / 1500
//!   stopping rule permits at most 2047 subdivisions for one in-domain interval.
//!   Across a partition of the natural interval, the total stays below roughly
//!   2 x 1500 plus the break count, rather than 2047 per break, leaving margin under
//!   the 8192 cap. Far-outside breaks can otherwise require trillions of
//!   spans. The bound limits the pending stack and generated polygon growth, not
//!   the supplied breaks (including duplicates), which are still visited in order.
//! - **Output allocation.** The separate [`MAX_APPROX_SPANS`] appended-span bound
//!   is retained and returns `InvalidInput` before unbounded control-polygon growth
//!   (upstream fuzz `curve.curve_p1` fuzz01255 aborted on a 6 GiB allocation).
//! - `SM_ASSERT`, `WARN` and the `SM_DEBUG_CODE` drawing blocks are not ported;
//!   `TessellateByBisection`'s unused `GetDegree` (its only consumer,
//!   `sm_FindTestParams`, ignores it) is not called.
//! - **Callees of unit `curve.curve_p5`** (`SmCurve::LocalPropertyAnalysis`, the
//!   evaluators behind `LocalCurveSolve` / `LocalCurveIntersect`, `SmCurve::IsArc`)
//!   are wired; [`SolidError::Unported`] is propagated from what they reach.

use crate::curve::curve::knots_ignoring_status;
use crate::curve::curve::p2::{
    add_to_curve, evaluate_continuity, find_test_params, flush_crv, output, pop_stack,
    proj_signed_angle, push_point, stack_look, test_step, CpGlobalSolver, Curve, CurveKind,
    CurvePropertyType, TessStepResult, BSPLINE_CURVE_TYPE, CRV_ON_SURF_TYPE,
};
use crate::curve::curve::p3::{eval_n, local_curve_intersect, PointSolveOptions};
use crate::curve::curve::p4::proj_dist;
use crate::curve::curve::p5::{local_property_analysis, make_curve_cache};
use crate::curve::curve_cache::CurveCache;
use crate::curve::hermite_curve::HermiteCurve;
use crate::curve::line::TessellationRequest;
use crate::error::{ignored, Result, SolidError};
use crate::geom::extent1d::Extent1d;
use crate::geom::geom_utility::p1::segment_point_angle_deg_to_perp;
use crate::geom::geom_utility::p2::ContinuityType;
use crate::geom::global_solver::{
    Branch, GlobalSolve, ObjectZoneTol, SolutionRequested, SolveRequest, SolverOperation,
};
use crate::geom::solution_array::{
    OperandObject, Solution, SolutionArray, SolutionObject, SolutionType, SortKey,
};
use crate::geom::tol::{are_same, is_zero, is_zero_to_tol};
use crate::geom::tree::{NodeData, Tree};
use crate::geom::vector3d::{Point3, Vec3};
use crate::kernel::Kernel;
use crate::math::{
    max2, BIG_DOUBLE, EFF_ZERO, EFF_ZERO_RAD, EFF_ZERO_SQ, EFF_ZERO_SQRT, PI, UNDEF_DOUBLE,
};
use crate::nurbs::b_spline_curve::{BSplineCurve, Tessellation};

const OP_REFINE: &str = "curve::curve::p1::refine_cc_solutions";
const OP_APPROX: &str = "curve::curve::p1::Curve::approximate_curve";

/// Most Hermite spans [`Curve::approximate_curve`] appends before it gives up (not
/// upstream). Upstream grows its control polygon without limit, so a curve whose
/// spans pass the accuracy test only when tiny (coordinates near `1e10` and beyond,
/// e.g. a break at `1e300`) runs until memory is exhausted. Upstream fixtures need
/// hundreds of spans at most; 2^20 spans are about 75 MB of control points.
pub const MAX_APPROX_SPANS: usize = 1 << 20;
const OP_LOCAL: &str = "curve::curve::p1::CpGlobalSolver::local_solve_branch";
const OP_GLOBAL: &str = "curve::curve::p1::Curve::global_point_solve";

/// `SM_CONTINUITY_ANGLE` (`SM_ANG_TOL_DEG`, degrees): `EvaluateContinuity`'s default.
const CONTINUITY_ANGLE: f64 = 0.5;

/// Maximum adaptive Hermite subdivisions in one approximation (see deviations).
const MAX_APPROXIMATION_SUBDIVISIONS: usize = 8192;

/// Entry `i` of a three-slot sample array (`d` past its end).
fn at3<T: Copy>(a: &[T; 3], i: usize, d: T) -> T {
    a.get(i).copied().unwrap_or(d)
}

/// Sets entry `i` of a three-slot sample array.
fn put3<T>(a: &mut [T; 3], i: usize, v: T) {
    if let Some(x) = a.get_mut(i) {
        *x = v;
    }
}

/// The first `N` derivatives of an evaluation (missing entries stay `UNDEF`).
fn first_n<const N: usize>(v: &[Vec3]) -> [Vec3; N] {
    let mut out = [Vec3::UNDEF; N];
    for (d, s) in out.iter_mut().zip(v) {
        *d = *s;
    }
    out
}

/// The `SmStatus` value of an error a `double` function returns as its value
/// (`sm_GetProjDist`'s `SER`).
fn status_value(e: &SolidError) -> f64 {
    match e {
        SolidError::Upstream { code, .. } => f64::from(*code),
        SolidError::InvalidInput { .. } | SolidError::PortInvariant { .. } => 1007.0,
        _ => 1001.0,
    }
}

// ---- sm_RefineCCSolutions -----------------------------------------------------------

/// `sm_RefineCCSolutions`: polishes the ends of the curve/curve range solutions by a
/// local intersection from each end (when the end deviates by more than `1e-6`),
/// keeping a result that deviates less and moves both parameters by less than
/// `0.002` of the range (as upstream, the end's move is measured from the range's start,
/// so a forward range's end is never refined, unless
/// `Kernel::fixes.refine_cc_range_ends` is set: WR-383). `curve_of` resolves a solution
/// object to its curve.
///
/// # Errors
///
/// `SM_ERR` for a range solution without two variables; `SM_ERR_NULL_POINTER` for
/// an object that is not a curve; those of the local intersection.
pub fn refine_cc_solutions<'c, O, F>(
    k: &Kernel,
    solutions: &mut SolutionArray<O>,
    curve_of: F,
) -> Result<()>
where
    O: SolutionObject,
    F: Fn(Option<O>) -> Option<&'c Curve>,
{
    let desired_precision = 1.0e-10; // for curve/curve intersection
    let shift_limit = 0.002; // Maximum relative parameter shift allowed.

    for ii in 0..solutions.len() {
        let Some(sol) = solutions.get_mut(ii) else {
            break;
        };

        // Check only range solutions.
        if sol.solution_type != SolutionType::RangeOfValues {
            continue;
        }

        if sol.num_variables != 2 {
            return Err(SolidError::sm_err(OP_REFINE));
        }

        let crv1 = curve_of(sol.objects[0]).ok_or(SolidError::null_pointer(OP_REFINE))?;
        let crv2 = curve_of(sol.objects[1]).ok_or(SolidError::null_pointer(OP_REFINE))?;

        let natural_dom1 = crv1.natural_interval();
        let natural_dom2 = crv2.natural_interval();

        // Set up the intervals.
        let mut sol_ivl1 = Extent1d::new();
        sol_ivl1.add_value(sol.start.parameters[0]);
        sol_ivl1.add_value(sol.end.parameters[0]);
        let reverse1 = sol.start.parameters[0] > sol.end.parameters[0];

        let mut sol_ivl2 = Extent1d::new();
        sol_ivl2.add_value(sol.start.parameters[1]);
        sol_ivl2.add_value(sol.end.parameters[1]);
        let reverse2 = sol.start.parameters[1] > sol.end.parameters[1];

        for at_end in [false, true] {
            let end = if at_end { sol.end } else { sol.start };
            if end.solution_value <= 0.000001 {
                continue;
            }
            let guess1 = end.parameters[0];
            let guess2 = end.parameters[1];

            // (status ignored)
            let found = ignored(local_curve_intersect(
                k,
                crv1,
                &natural_dom1,
                crv2,
                &natural_dom2,
                desired_precision,
                guess1,
                guess2,
            ))?
            .flatten();

            if let Some(x) = found {
                if x.deviation < end.solution_value {
                    // Do not allow big shifts.  [140915b]
                    let mut frac1 = sol_ivl1
                        .inversion(x.my_param)
                        .parameter
                        .unwrap_or(UNDEF_DOUBLE);
                    let mut frac2 = sol_ivl2
                        .inversion(x.other_param)
                        .parameter
                        .unwrap_or(UNDEF_DOUBLE);
                    if reverse1 {
                        frac1 = 1.0 - frac1;
                    }
                    if reverse2 {
                        frac2 = 1.0 - frac2;
                    }

                    // Not upstream (WR-383, source issue #527): with the
                    // opt-in, an end is judged by its distance from 1 (the fraction is
                    // measured from the solution's start); upstream tests |frac| at both
                    if at_end && k.fixes.refine_cc_range_ends {
                        frac1 = 1.0 - frac1;
                        frac2 = 1.0 - frac2;
                    }
                    if frac1.abs() < shift_limit && frac2.abs() < shift_limit {
                        let dst = if at_end { &mut sol.end } else { &mut sol.start };
                        dst.solution_value = x.deviation;
                        dst.parameters[0] = x.my_param;
                        dst.parameters[1] = x.other_param;
                    }
                }
            }
        }
    }

    Ok(())
}

// ---- sm_TestSpanAccuracy ------------------------------------------------------------

/// Result of [`test_span_accuracy`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpanAccuracy {
    /// `rbSpanPassesTest`: the Hermite span is within the tolerance (or very short).
    pub passes: bool,
    /// `rdMaxDistToCrv`: the largest sampled distance (up to the first failing
    /// sample when the span fails).
    pub max_dist: f64,
}

/// `sm_TestSpanAccuracy`: whether the Hermite span between `last` and `curr`
/// (position, first derivative at the parameters `last_t`, `curr_t`) lies within
/// `approx_tol` of `curve` at 3 samples (7 when the span is forced to pass: shorter
/// than `SM_EFF_ZERO` scaled or than 1/1500 of the curve's interval). With
/// `match_parameterization` the distance is taken at equal relative parameters,
/// else to the closest Hermite point (a failed drop counts as `2 approx_tol`).
///
/// # Errors
///
/// Those of the curve evaluations; [`SolidError::Unported`] from the drop.
pub fn test_span_accuracy(
    k: &Kernel,
    curve: &Curve,
    last_t: f64,
    last_pv: [Point3; 2],
    curr_t: f64,
    curr_pv: [Point3; 2],
    approx_tol: f64,
    match_parameterization: bool,
) -> Result<SpanAccuracy> {
    // init output
    let mut out = SpanAccuracy {
        passes: false,
        max_dist: 0.0,
    };

    // locals
    let mut force_pass = false;
    let this_span = curr_t - last_t;
    let t_scale = 1.0 + curr_t.abs();
    let ivl = curve.natural_interval();
    let min_step = 1500.0;
    let mut num_test_points: u32 = 3;

    // when span is getting very small - just pass it
    if this_span < EFF_ZERO * t_scale {
        force_pass = true;
    }

    // Don't let the interval get too small
    if this_span < (ivl.length()) / min_step {
        force_pass = true;
    }

    // hermite cuve between interval end points
    let hermite = HermiteCurve::new(
        last_pv[0],
        last_pv[1] * this_span,
        curr_pv[0],
        curr_pv[1] * this_span,
        3,
    );
    let hermite_curve = Curve::from(hermite);

    if force_pass {
        // get a better error value if we're going to keep this step.
        num_test_points = 7;
    }

    for ii in 0..num_test_points {
        let normalized_param = (f64::from(ii) + 1.0) / (f64::from(num_test_points) + 1.0);
        let crv_param = last_t + normalized_param * this_span;

        // Evaluate the curve.  Then, if we're matching parameterization,
        // evaluate the Hermite curve at the same (relative) parameter,
        // else find the closest point on the Hermite curve.
        let crv_pnt = curve.evaluate_point(k, crv_param)?;

        let approx_dist = if match_parameterization {
            // Approx dist = DistanceBetween(Curve(param), Hermite(param))
            let herm_pnt = hermite.evaluate_point(normalized_param);
            herm_pnt.distance(crv_pnt)
        } else {
            // Approx dist = DistanceBetween(Curve(param), NearestPointOnHermite)
            let hermite_domain = Extent1d::from_min_max(0.0, 1.0);
            let r = hermite_curve.local_point_solve::<()>(
                k,
                &hermite_domain,
                SolverOperation::Minimize,
                crv_pnt,
                &PointSolveOptions::default(),
                normalized_param,
            );
            // save nearest distance (when drop point failed - load a big distance to
            // quit testing)
            match ignored(r)? {
                Some((true, sol)) => sol.start.solution_value,
                _ => 2.0 * approx_tol,
            }
        };

        // When ApproxDist is too big
        if approx_dist > approx_tol && !force_pass {
            // set output and quit testing
            out.passes = false;
            return Ok(out);
        }
        out.max_dist = max2(out.max_dist, approx_dist);
    }

    // arrive here when HermiteCurve is within ApproxTol of Curve at sample points or
    // when bForcePass == TRUE
    out.passes = true;
    Ok(out)
}

// ---- ApproximateCurve ---------------------------------------------------------------

/// The flags of [`Curve::approximate_curve`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApproximateCurveOptions {
    /// `bOptCreateAnalytics` (default `true`): make an exact arc when the
    /// approximation lies on a circle.
    pub create_analytics: bool,
    /// `bOptMatchParameterization` (default `false`): measure the error at equal
    /// parameters instead of to the closest point.
    pub match_parameterization: bool,
    /// `bJustCopyBSplines` (default `false`): copy B-spline curves instead of
    /// approximating them.
    pub just_copy_bsplines: bool,
}

impl Default for ApproximateCurveOptions {
    fn default() -> Self {
        Self {
            create_analytics: true,
            match_parameterization: false,
            just_copy_bsplines: false,
        }
    }
}

/// Result of [`Curve::approximate_curve`].
#[derive(Debug, Clone)]
pub struct CurveApproximation {
    /// `rpNewBSplineCurve`: the approximation (a B-spline or a copy of this curve).
    pub curve: Curve,
    /// `rdAchievedTol`: the largest sampled distance; `None` where upstream does not
    /// write it (a degenerate curve).
    pub achieved_tol: Option<f64>,
}

impl Curve {
    /// `ApproximateCurve`: a piecewise cubic B-spline through the curve at the
    /// parameters `breaks` (at least 2), each break interval bisected until its
    /// Hermite span passes [`test_span_accuracy`] with `approx_tol_3d`, flushed by
    /// [`flush_crv`] (an exact arc when possible with `create_analytics`). A
    /// degenerate curve becomes a degenerate B-spline over `[breaks[0], last]`;
    /// B-splines are copied with `just_copy_bsplines` (tolerance `0`) and also when
    /// the approximation would have more control points.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for fewer than 2 breaks, exhausted subdivision work or a midpoint
    /// that cannot progress; those of the evaluations, the degenerate curve,
    /// [`flush_crv`] and `Copy`; `InvalidInput` when nothing is built.
    pub fn approximate_curve(
        &self,
        k: &Kernel,
        breaks: &[f64],
        approx_tol_3d: f64,
        opts: &ApproximateCurveOptions,
    ) -> Result<CurveApproximation> {
        match self.kind() {
            // the SmCrvOnSurf override (its cache update is lost on this copy)
            CurveKind::CrvOnSurf(c) => {
                let mut c = c.clone_exact_shared();
                let (curve, gap) =
                    c.approximate_curve(k, breaks, approx_tol_3d, opts.match_parameterization)?;
                Ok(CurveApproximation {
                    curve: Curve::from(curve),
                    achieved_tol: Some(gap),
                })
            }
            // the SmCrvOnSurf override on the derived object (its Evaluate approximates)
            CurveKind::TangentField(c) => {
                let members = c.members();
                let mut c = c.base().clone();
                let (curve, gap) = c.approximate_curve_as(
                    k,
                    breaks,
                    approx_tol_3d,
                    opts.match_parameterization,
                    Some(members),
                )?;
                Ok(CurveApproximation {
                    curve: Curve::from(curve),
                    achieved_tol: Some(gap),
                })
            }
            // the SmCompositeCurve override (breaks and flags unused)
            CurveKind::Composite(c) => {
                let a = c.approximate_curve(k, approx_tol_3d)?;
                let curve = a.curve.ok_or_else(|| {
                    SolidError::invalid_input(OP_APPROX, "composite approximation is not valid")
                })?;
                Ok(CurveApproximation {
                    curve: Curve::from(curve),
                    achieved_tol: Some(a.max_gap),
                })
            }
            _ => self.approximate_curve_base(k, breaks, approx_tol_3d, opts),
        }
    }

    /// `SmCurve::ApproximateCurve` (the base-class body of
    /// [`Curve::approximate_curve`]).
    ///
    /// # Errors
    ///
    /// As [`Curve::approximate_curve`].
    pub fn approximate_curve_base(
        &self,
        k: &Kernel,
        breaks: &[f64],
        approx_tol_3d: f64,
        opts: &ApproximateCurveOptions,
    ) -> Result<CurveApproximation> {
        // low work - when asked, just copy BSplines
        if opts.just_copy_bsplines && self.is_kind_of(BSPLINE_CURVE_TYPE) {
            // (upstream ignores the status of Copy)
            let curve = self.copy(k)?;
            return Ok(CurveApproximation {
                curve,
                achieved_tol: Some(0.0),
            });
        }

        // need 2 breakpoints to define the curve interval
        let (Some(&first), Some(&last)) = (breaks.first(), breaks.last()) else {
            return Err(SolidError::sm_err(OP_APPROX));
        };
        if breaks.len() < 2 {
            return Err(SolidError::sm_err(OP_APPROX));
        }

        // low work - degenerate curves
        if self.is_degenerate(k, EFF_ZERO, None)? {
            let pnt = self.evaluate_point(k, first)?;
            let degenerate = BSplineCurve::create_degenerate_curve(self.dim(), pnt)?;
            let mut curve = Curve::from(degenerate);
            let ivl = Extent1d::from_min_max(first, last);
            // (status ignored)
            ignored(curve.edit_parameterization(k, &ivl))?;
            return Ok(CurveApproximation {
                curve,
                achieved_tol: None,
            });
        }

        // locals
        let mut stack: Vec<f64> = Vec::with_capacity(256 * 4 * 2);
        let mut ctrl_poly: Vec<Point3> = Vec::with_capacity(256);
        let mut knots: Vec<f64> = Vec::with_capacity(256);
        let mut max_deviation = 0.0;
        let mut subdivisions = 0;

        // for every interval defined by input BreakParams array
        for w in breaks.windows(2) {
            let (Some(&a), Some(&b)) = (w.first(), w.get(1)) else {
                continue;
            };
            let mut last_t = a;
            let mut curr_t = b;

            // interval start position and tangent (Last)
            let mut last_pv: [Vec3; 2] = eval_n(k, self, last_t, 1, true, true)?; // NonZeroTangent values
            {
                // walk to a nonZero Tangent evaluation
                let mut step = 0.0001;
                while last_pv[1].length_squared() < EFF_ZERO_SQ * 100000.0 {
                    let tmp = last_pv[0];
                    last_pv = eval_n(k, self, last_t + step * (curr_t - last_t), 1, true, true)?;
                    last_pv[0] = tmp;
                    step *= 2.0;
                    if step > 0.5 {
                        break;
                    }
                }
            }

            // interval end position and tangent (Curr)
            let mut curr_pv: [Vec3; 2] = eval_n(k, self, curr_t, 1, false, true)?; // NonZeroTangent values
            {
                let mut step = 0.0001;
                while curr_pv[1].length_squared() < EFF_ZERO_SQ * 100000.0 {
                    let tmp = curr_pv[0];
                    curr_pv = eval_n(k, self, curr_t - step * (curr_t - last_t), 1, true, true)?;
                    curr_pv[0] = tmp;
                    step *= 2.0;
                    if step > 0.5 {
                        break;
                    }
                }
            }

            // Place Curr Point [param, posXYZ, tanXYZ] on sStack
            push_point(curr_t, curr_pv, &mut stack);

            // For each span subdivide until our approximating curve is accurate enough.
            while !stack.is_empty() {
                // get last point's param, pos, and tan from stack - leave stack unmodified
                (curr_t, curr_pv) = stack_look(&stack)?;

                // Test if HermiteSpan is within ApproxTol of thisCurve interval
                let acc = test_span_accuracy(
                    k,
                    self,
                    last_t,
                    last_pv,
                    curr_t,
                    curr_pv,
                    approx_tol_3d,
                    opts.match_parameterization,
                )?;

                // When Span passes test (or If step size gets real small just accept it)
                if acc.passes || (last_t - curr_t).abs() < EFF_ZERO {
                    // save max deviation
                    max_deviation = max2(max_deviation, acc.max_dist);

                    // (not upstream) bound the growing curve: see MAX_APPROX_SPANS
                    if knots.len() > MAX_APPROX_SPANS {
                        return Err(SolidError::invalid_input(
                            OP_APPROX,
                            "approximation needs more than MAX_APPROX_SPANS spans",
                        ));
                    }

                    // Append Hermite interval to end of growing piecewise cubic bezier
                    add_to_curve(last_pv, curr_pv, last_t, curr_t, &mut ctrl_poly, &mut knots)?;

                    // for next iteration - move Curr param, pos, and tan to Last
                    last_t = curr_t;
                    last_pv = curr_pv;

                    // for next iteration - pop stack
                    pop_stack(&mut stack)?;
                } else {
                    if subdivisions >= MAX_APPROXIMATION_SUBDIVISIONS {
                        return Err(SolidError::sm_err(OP_APPROX));
                    }
                    subdivisions += 1;

                    // get param mid point and associated curve pos and tan
                    let mid_t = (last_t + curr_t) / 2.0;
                    if !mid_t.is_finite() || mid_t == last_t || mid_t == curr_t {
                        return Err(SolidError::sm_err(OP_APPROX));
                    }
                    let mid_pv: [Vec3; 2] = eval_n(k, self, mid_t, 1, true, true)?; // NonZeroTangent values
                    curr_t = mid_t;
                    curr_pv = mid_pv;

                    // push midPoint onto stach
                    push_point(curr_t, curr_pv, &mut stack);
                }
            }
        }

        // create pBSC curve from control polygon
        let bsc = flush_crv(
            approx_tol_3d,
            &mut ctrl_poly,
            &mut knots,
            opts.create_analytics,
        )?
        .ok_or(SolidError::invalid_input(
            OP_APPROX,
            "no approximating curve built",
        ))?;
        let new_count = bsc.number_control_points()?;
        let mut out = CurveApproximation {
            curve: Curve::from(bsc),
            achieved_tol: Some(max_deviation),
        };

        // return a warning when input curve is better than the approximation
        if self.is_kind_of(BSPLINE_CURVE_TYPE)
            && !self.is_kind_of(CRV_ON_SURF_TYPE)
            && new_count > self.number_control_points()?
        {
            out.curve = self.copy(k)?;
        }

        Ok(out)
    }
}

// ---- ClassifyIntersectingRangeSolution ------------------------------------------------

/// The tangent intersection point of
/// [`Curve::classify_intersecting_range_solution`] (the fields of `rSolPoint` it
/// writes).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TangentPoint {
    /// `m_dSolutionValue`: the deviation at the better range end.
    pub solution_value: f64,
    /// `m_adParameters[0..2]`: the parameters of that end on this and the other curve.
    pub parameters: [f64; 2],
}

impl Curve {
    /// `ClassifyIntersectingRangeSolution`: whether the range intersection `sol` with
    /// `other` is degenerate (a tangent point, not a coincident stretch), and then the
    /// point (`None`: `rSolPoint` left unmodified). A point solution is degenerate.
    /// Otherwise, if a local minimum of the gap (well below the better end's
    /// deviation) lies inside the range, the gap is sampled from the better end at
    /// geometrically growing steps: a non-zero, monotonic, faster than linearly
    /// growing gap (or a sample that does not drop onto the other curve) is a
    /// tangent point; a gap that stays zero over a short range also is.
    ///
    /// # Errors
    ///
    /// Those of the local solves.
    pub fn classify_intersecting_range_solution<O: SolutionObject>(
        &self,
        k: &Kernel,
        other: &Self,
        sol: &Solution<O>,
    ) -> Result<(bool, Option<TangentPoint>)> {
        // low work - Solution already a point solution
        if sol.solution_type == SolutionType::SingleValue {
            return Ok((true, None));
        }

        // locals to run from best solution end to other end
        let this_curve = self;
        let ivl = self.natural_interval();
        let o_ivl = other.natural_interval();
        let (start_dev, end_dev, start_par, delta_par) =
            if sol.start.solution_value < sol.end.solution_value {
                (
                    sol.start.solution_value,
                    sol.end.solution_value,
                    [sol.start.parameters[0], sol.start.parameters[1]],
                    [
                        sol.end.parameters[0] - sol.start.parameters[0],
                        sol.end.parameters[1] - sol.start.parameters[1],
                    ],
                )
            } else {
                (
                    sol.end.solution_value,
                    sol.start.solution_value,
                    [sol.end.parameters[0], sol.end.parameters[1]],
                    [
                        sol.start.parameters[0] - sol.end.parameters[0],
                        sol.start.parameters[1] - sol.end.parameters[1],
                    ],
                )
            };

        // Do this only if it looks like a relative minimum on the interior.  [140915b]
        let mut this_dom = Extent1d::from_value(sol.start.parameters[0]);
        this_dom.add_value(sol.end.parameters[0]);
        let mut othr_dom = Extent1d::from_value(sol.start.parameters[1]);
        othr_dom.add_value(sol.end.parameters[1]);
        // (status ignored)
        let solved = ignored(self.local_curve_solve::<O>(
            k,
            &this_dom,
            other,
            &othr_dom,
            SolverOperation::Minimize,
            1.0,
            Some(end_dev),
            None,
            [this_dom.evaluate(0.5), othr_dom.evaluate(0.5)],
            [None, None],
        ))?;
        let Some((true, min_sol)) = solved else {
            return Ok((false, None)); // Did not find a minimum.
        };

        if min_sol.start.solution_value > start_dev / 10.0 {
            return Ok((false, None)); // Not much of a minimum.
        }

        // get Solution segment start points and gapVector
        let pnt0 = ignored(this_curve.evaluate_point(k, start_par[0]))?.unwrap_or(Vec3::UNDEF);
        let pnt1 = ignored(other.evaluate_point(k, start_par[1]))?.unwrap_or(Vec3::UNDEF);
        let xsect_gap = pnt1 - pnt0;

        // scale zero to current intersection point location
        let scaled_zero = EFF_ZERO * (1.0 + pnt0.max_dimension());

        // locals - gap function properties
        let mut curve_point = [Vec3::UNDEF; 3];
        let mut o_curve_point = [Vec3::UNDEF; 3];
        let mut gap = [Vec3::UNDEF; 3];
        let mut gap_length = [0.0; 3];

        // init classification booleans
        let mut b_zero = true;
        let mut b_mono = true;
        let mut b_gdiv = true;

        // init first SamplePoint array entries with the xSect StartPoint
        curve_point[0] = pnt0;
        o_curve_point[0] = pnt1;
        gap[0] = Vec3::ZERO; // set gap to exactly zero - see trick later on.
        gap_length[0] = 0.0;

        // init walk parameter (a little smaller than a fraction of a power of two)
        let mut delta = 1.0 / 16.0001;

        // init solution classification
        let mut is_degenerate = false;

        // until solution segment is classified
        let mut done = false;
        let mut ii: usize = 1;
        while !done && delta < 1.0 {
            // get the gapFunction sample array index
            let tgt: usize = if ii > 1 { 2 } else { 1 };

            // get the samplePoint Parameter Values
            let t1 = start_par[0] + delta * delta_par[0];
            let t2 = start_par[1] + delta * delta_par[1];

            // get ThisCurve sample point
            if let Some(p) = ignored(this_curve.evaluate_point(k, t1))? {
                put3(&mut curve_point, tgt, p);
            }

            // Find OtherCurve nearest point to sCurvePoint
            let dropped = ignored(other.local_point_solve::<O>(
                k,
                &o_ivl,
                SolverOperation::Normalize,
                at3(&curve_point, tgt, Vec3::UNDEF),
                &PointSolveOptions::default(),
                t2,
            ))?;

            if let Some((true, s)) = dropped {
                // evaluate the nearest OtherCurve point
                let o_drop = s.start.parameters[0];
                if let Some(p) = ignored(other.evaluate_point(k, o_drop))? {
                    put3(&mut o_curve_point, tgt, p);
                }

                // get the Curve/Curve gapVector (minus the start gap)
                let g = (at3(&o_curve_point, tgt, Vec3::UNDEF)
                    - at3(&curve_point, tgt, Vec3::UNDEF))
                    - xsect_gap;
                put3(&mut gap, tgt, g);
                put3(&mut gap_length, tgt, g.length());
            } else {
                // Found a point in the intersection range on this curve which does
                // not drop along a normal to the other curve: a point solution.
                is_degenerate = true;

                // get out of this classification loop
                done = true;
                ii += 1;
                delta *= 2.0;
                continue;
            }

            // once we have 3 sample points
            if ii >= 2 {
                let tgt1 = (tgt + 3 - 1) % 3;
                let tgt2 = (tgt + 3 - 2) % 3;
                let len = at3(&gap_length, tgt, 0.0);
                let len1 = at3(&gap_length, tgt1, 0.0);
                let len2 = at3(&gap_length, tgt2, 0.0);

                // 1st: is xSectCurve gapSize zero (coincident curves)
                if len > scaled_zero {
                    b_zero = false;
                }

                // 2nd: is xSectCurve gap function monotonic
                if at3(&gap, tgt, Vec3::UNDEF).dot(at3(&gap, tgt1, Vec3::UNDEF)) <= 0.0 {
                    b_mono = false;
                }

                // 3rd: is xSectCurve gap function geometrically diverging
                if len == 0.0 || len < len1 + 1.0001 * (len1 - len2) {
                    b_gdiv = false;
                }

                // (4th, the linear divergence test, sets nothing upstream)

                // now classify this XSectCurve GapFunction.
                if !b_zero && b_mono && b_gdiv {
                    is_degenerate = true;
                    done = true;
                } else if b_zero && b_mono && b_gdiv {
                    // try another point
                    done = false;

                    // move current guess to last guess
                    curve_point[1] = curve_point[2];
                    o_curve_point[1] = o_curve_point[2];
                    gap[1] = gap[2];
                    gap_length[1] = gap_length[2];
                } else {
                    // this side of the curve can have a coincident region
                    is_degenerate = false;
                    done = true;
                }
            }
            ii += 1;
            delta *= 2.0;
        }

        // check to see if interval was classified
        if !done && b_gdiv {
            // range diverged but never got larger than Scaled Zero:
            // let's try throwing away short intervals
            is_degenerate = delta_par[0].abs() < ivl.length() / 10.0
                && delta_par[1].abs() < o_ivl.length() / 10.0;
        }

        // when soluton is classified as degenerate
        if is_degenerate {
            return Ok((
                true,
                Some(TangentPoint {
                    solution_value: start_dev,
                    parameters: start_par,
                }),
            ));
        }
        Ok((false, None))
    }
}

// ---- SmCPGlobalSolver::LocalSolve -------------------------------------------------------

/// Merges a [`Curve::local_point_solve`] result into the persistent solution of
/// `LocalSolve` the way upstream writes its `rSolution`.
fn merge_point_solution<O: Copy>(
    dst: &mut Solution<O>,
    found: bool,
    src: &Solution<O>,
    op: SolverOperation,
) {
    if !found {
        return;
    }
    if op == SolverOperation::Rayfire || op == SolverOperation::Signed3dDirectedMinimize {
        *dst = *src;
        return;
    }
    dst.solution_type = src.solution_type;
    dst.start.solution_value = src.start.solution_value;
    dst.start.parameters[0] = src.start.parameters[0];
}

impl<'t, O: SolutionObject + 't> CpGlobalSolver<'t, O> {
    /// `AddSortedSolution` by value (with parameter) or by first parameter.
    fn add_solution(&mut self, solution: &Solution<O>, by_value: bool) -> Result<()> {
        let key = if by_value {
            SortKey::ByValueWithParameter
        } else {
            SortKey::ByFirstParameter
        };
        self.add_sorted_solution(solution, key, true)
    }

    /// Option vector `i` (`m_cpOptVectors[i]`, read without a `NULL` check).
    fn opt(&self, i: usize) -> Result<Vec3> {
        self.base
            .opt_vectors
            .get(i)
            .copied()
            .ok_or(SolidError::invalid_input(OP_LOCAL, "missing option vector"))
    }

    /// `SmCPGlobalSolver::LocalSolve` on the point node `branch[0]` and curve leaf
    /// `branch[1]` (always returns `rbNeedsMoreSubdivision` false): the span interval
    /// is limited to the solver's (span ends at a closed curve's seam take the seam
    /// continuity), spans out of reach of the best answer or tolerance are culled,
    /// a degenerate span is a point solution, then each guess (the closest point of
    /// the span chord, its middle and ends) is solved locally (by
    /// `LocalPropertyAnalysis` for the directed, projected and pivot operations, else
    /// [`Curve::local_point_solve`]); boundary solutions that continue into the
    /// neighbouring span are skipped, solutions equal to their guess are collected
    /// and three of them make the span a range solution.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` for a node without data; `InvalidInput` for a missing
    /// branch node, curve or option vector or a node that is not a curve leaf; those
    /// of `Unitize`, the signed angle, `AddSortedSolution` and `LocalPropertyAnalysis`.
    pub fn local_solve_branch(&mut self, k: &Kernel, branch: &Branch) -> Result<bool> {
        // init outputs
        let needs_more_subdivision = false;

        // locals
        let point_node = self.base.branch_node(branch, 0)?;
        let curve_node = self.base.branch_node(branch, 1)?;
        if matches!(curve_node.data, NodeData::None) {
            return Err(SolidError::null_pointer(OP_LOCAL));
        }
        let curve_tree = self.base.tree(1)?;
        let bez_span = *curve_tree.curve_span(curve_node)?;
        let mut bez_ivl = *bez_span.interval();
        let test_pnt = point_node.bbox.min();
        let curve = self
            .curve
            .ok_or(SolidError::invalid_input(OP_LOCAL, "solver without curve"))?;
        let curve_obj = curve_tree.owner_object()?;
        let op = self.base.operation;
        let requested = self.base.solution_requested;
        let tol_3d = self.base.tolerance_3d;
        let opt_vectors = self.base.opt_vectors.clone();
        let interval = self.interval;

        // Do interval tests
        let mut conts = bez_span.aux.conts;

        // no work - disjoint leafNode and problem intervals
        if bez_ivl.are_disjoint(&interval, 0.0) {
            return Ok(needs_more_subdivision);
        }

        // when BezierInterval extends beyond TotalProblemInterval - mark this range
        // ends as discontinuous
        if bez_ivl.min() <= interval.min() {
            conts[0] = ContinuityType::Discontinuous;
        }
        if bez_ivl.max() >= interval.max() {
            conts[1] = ContinuityType::Discontinuous;
        }

        // trim sBezIvl to m_crInterval
        if !bez_ivl.is_contained_by(&interval, EFF_ZERO) {
            bez_ivl = bez_ivl.intersect(&interval)?;
            // no work - zero length trimmed interval
            if are_same(bez_ivl.max(), bez_ivl.min()) {
                return Ok(needs_more_subdivision);
            }
        }

        // adjust continuity for boundary endSpans on closed curves
        let parent_ivl = curve.natural_interval();
        if (is_zero(bez_ivl.min() - parent_ivl.min()) || is_zero(bez_ivl.max() - parent_ivl.max()))
            && (is_zero(interval.min() - parent_ivl.min())
                && is_zero(interval.max() - parent_ivl.max()))
            && curve.is_closed(k, &parent_ivl, 0.0)?
        {
            // get pCurve1's end continuity (status ignored)
            let mut continuity = ContinuityType::Discontinuous;
            if let Some(c) = ignored(evaluate_continuity(
                k,
                curve,
                parent_ivl.min(),
                false,
                curve,
                parent_ivl.max(),
                true,
                CONTINUITY_ANGLE,
            ))? {
                continuity = c.continuity;
            }
            // set span endContinuities as appropriate
            if is_zero(bez_ivl.min() - parent_ivl.min()) {
                conts[0] = continuity;
            }
            if is_zero(bez_ivl.max() - parent_ivl.max()) {
                conts[1] = continuity;
            }
        }

        // cull no hope minimization and SingleSolution normal situations
        if op == SolverOperation::Minimize
            || (op == SolverOperation::Normalize && requested == SolutionRequested::Single)
        {
            // quit when point/pseudoBox distance is greater than tolerance to current
            // best answer
            let mut dist = bez_span.pseudo_box().distance_to_point(test_pnt);
            dist -= tol_3d;
            if dist < 0.0 {
                dist = 0.0;
            }
            if dist * dist > self.base.best_answer_so_far_sq {
                return Ok(needs_more_subdivision);
            }
        }

        // cull no hope intersection cases
        if op == SolverOperation::Intersect {
            let dist = bez_span.pseudo_box().distance_to_point(test_pnt);
            if dist > tol_3d {
                return Ok(needs_more_subdivision);
            }
        }

        let max_turn = bez_span.aux.max_turning_angle_deg;
        let do_line_test = max_turn < 90.0
            && (op == SolverOperation::Normalize
                || ((op == SolverOperation::Minimize || op == SolverOperation::Maximize)
                    && ((conts[0] as i32) > (ContinuityType::C0 as i32)
                        && (conts[1] as i32) > (ContinuityType::C0 as i32))));

        // locals
        let mut guesses: Vec<f64> = Vec::with_capacity(6);

        // get BezierCurve EndPoints and the vector between them (statuses ignored)
        let mut start_pt = [Vec3::UNDEF; 2];
        let mut end_pt = [Vec3::UNDEF; 2];
        if let Some(v) = ignored(curve.evaluate(k, bez_ivl.min(), 1, true, true))? {
            start_pt = first_n(&v);
        }
        if let Some(v) = ignored(curve.evaluate(k, bez_ivl.max(), 1, true, true))? {
            end_pt = first_n(&v);
        }

        let vec = end_pt[0] - start_pt[0];

        // negate sStartPt tangent so that both end Point tangent vectors point outside
        start_pt[1] = -start_pt[1];

        let by_value_degenerate = op == SolverOperation::Minimize
            || op == SolverOperation::Maximize
            || (op == SolverOperation::Normalize && requested == SolutionRequested::Single);

        // when bezierCurve segment is degenerate (zeroLength)
        if vec.length_squared() < EFF_ZERO_SQ {
            // when looking for min/max/intersect solutions
            if op == SolverOperation::Minimize
                || op == SolverOperation::Maximize
                || op == SolverOperation::Intersect
                || (op == SolverOperation::Normalize && requested == SolutionRequested::Single)
            {
                // get point/point distance
                let dist = end_pt[0].distance(test_pnt);

                // cull out of tolerance intersect cases
                if op == SolverOperation::Intersect && dist > tol_3d {
                    return Ok(needs_more_subdivision);
                }

                // save this as a solution
                let mut solution: Solution<O> = Solution::new();
                solution.solution_type = SolutionType::SingleValue;
                solution.nodes[0] = branch[0];
                solution.nodes[1] = branch[1];
                solution.num_variables = 1;
                solution.start.solution_value = dist;
                solution.start.parameters[0] = bez_ivl.min();
                solution.num_objects = 1;
                solution.objects[0] = curve_obj;

                // add solution to sSolution array in proper order
                self.add_solution(&solution, by_value_degenerate)?;
            }

            return Ok(needs_more_subdivision);
        }

        // arrive here when bezier segment is not zeroLength
        // get closest point on endPoint lineSegment to testPoint
        let (ang_deg_from_perp, t) =
            segment_point_angle_deg_to_perp(start_pt[0], end_pt[0], test_pnt);

        // when doing the line test
        if do_line_test
            && ang_deg_from_perp * 0.99 - EFF_ZERO > max_turn
            && !test_pnt.closer_than(tol_3d, start_pt[0])
            && !test_pnt.closer_than(tol_3d, end_pt[0])
        {
            return Ok(needs_more_subdivision); // no Normal point possible on this segment
        }

        // for all cases but SM_SO_AT_DISTANCE
        if op == SolverOperation::AtDistance {
            // add end and mid Point locations as guesses
            guesses.push(bez_ivl.evaluate(0.5));
            guesses.push(bez_ivl.evaluate(0.0));
            guesses.push(bez_ivl.evaluate(1.0));
        } else {
            // add closest linePoint to guess array and unique end and mid point positions
            guesses.push(bez_ivl.evaluate(t));
            if !is_zero_to_tol(t - 0.5, EFF_ZERO_SQRT) {
                guesses.push(bez_ivl.evaluate(0.5));
            }
            if !is_zero_to_tol(t - 0.0, EFF_ZERO_SQRT) {
                guesses.push(bez_ivl.evaluate(0.0));
            }
            if !is_zero_to_tol(t - 1.0, EFF_ZERO_SQRT) {
                guesses.push(bez_ivl.evaluate(1.0));
            }
        }

        // use minimization to find point/curve intersection points
        let local_op = if op == SolverOperation::Intersect {
            SolverOperation::Minimize
        } else {
            op
        };

        // init solution search locals
        let mut solution: Solution<O> = Solution::new();
        let mut coin_solutions: Vec<Solution<O>> = Vec::with_capacity(6);
        let mut found_solution: bool;
        let mut last_found_solution = false;
        let mut solution_value = BIG_DOUBLE;
        let mut last_deviation = 0.0;
        let mut last_parameter = 0.0;

        // when intersecting - only try the 1st guess
        let num_guesses = if op == SolverOperation::Intersect {
            1
        } else {
            guesses.len()
        };

        let by_value_main = matches!(
            op,
            SolverOperation::Minimize
                | SolverOperation::Maximize
                | SolverOperation::Rayfire
                | SolverOperation::SignedDirectedMinimize
                | SolverOperation::Signed3dDirectedMinimize
                | SolverOperation::DirectedMinimize
                | SolverOperation::DirectedMaximize
                | SolverOperation::ProjectedMinimize
                | SolverOperation::ProjectedMaximize
                | SolverOperation::ProjectedTangentThroughPoint
        ) || (op == SolverOperation::Normalize
            && requested == SolutionRequested::Single);

        let opt_slice: Option<&[Vec3]> = if opt_vectors.is_empty() {
            None
        } else {
            Some(&opt_vectors)
        };

        // for every guess point
        for i in 0..num_guesses {
            let guess = guesses.get(i).copied().unwrap_or(0.0);

            // for projected, directed, and signed pivot cases
            if matches!(
                op,
                SolverOperation::SignedDirectedMinimize
                    | SolverOperation::DirectedMinimize
                    | SolverOperation::DirectedMaximize
                    | SolverOperation::ProjectedMinimize
                    | SolverOperation::ProjectedMaximize
                    | SolverOperation::SignedPivotMinimize
                    | SolverOperation::ProjectedTangentThroughPoint
            ) {
                // locals for LocalPropertyAnalysis call
                let mut curve_prop = CurvePropertyType::PerpendicularToVector;
                let mut vectors = [Vec3::UNDEF; 2];
                let mut direction = Vec3::ZERO;
                let mut property_value = 0.0;

                match op {
                    SolverOperation::DirectedMinimize
                    | SolverOperation::DirectedMaximize
                    | SolverOperation::SignedDirectedMinimize => {
                        curve_prop = CurvePropertyType::PlaneIntersection;
                        vectors[0] = self.opt(1)?.cross(self.opt(0)?);
                        vectors[0] = vectors[0].unitize()?;
                        direction = self.opt(1)?;
                        property_value = -vectors[0].dot(test_pnt);
                    }
                    SolverOperation::SignedPivotMinimize => {
                        curve_prop = CurvePropertyType::CylinderIntersection;
                        vectors[0] = self.opt(1)?; // Pivot
                        vectors[1] = self.opt(0)?; // Normal of projection plane
                        direction = self.opt(2)?; // 'Side' of Pivot pt
                        let v = vectors[1].cross(test_pnt - vectors[0]).cross(vectors[1]);
                        property_value = v.length(); // Radius of an infinite cylinder
                    }
                    SolverOperation::ProjectedMinimize | SolverOperation::ProjectedMaximize => {
                        curve_prop = if op == SolverOperation::ProjectedMaximize {
                            CurvePropertyType::ProjectedPointMaximize
                        } else {
                            CurvePropertyType::ProjectedPointMinimize
                        };
                        vectors[0] = test_pnt;
                        vectors[1] = self.opt(0)?;
                    }
                    SolverOperation::ProjectedTangentThroughPoint => {
                        curve_prop = CurvePropertyType::ProjectedTangentThroughPoint;
                        vectors[0] = test_pnt;
                        vectors[1] = self.opt(0)?;
                    }
                    _ => {}
                }

                // execute NewtonRaphson serarch to find curve param value
                // that sets requested CurveProperty to zero
                match local_property_analysis(
                    k,
                    curve,
                    &bez_ivl,
                    curve_prop,
                    guess,
                    Some(property_value),
                    &vectors,
                    &mut solution,
                ) {
                    Ok(f) => found_solution = f,
                    Err(e) if e.is_unported() => return Err(e),
                    Err(_) => continue, // If error try another guess
                }

                // when no solution are found
                // see if endPoints should be solutions for projected_min/max cases
                if !found_solution
                    && (op == SolverOperation::ProjectedMinimize
                        || op == SolverOperation::ProjectedMaximize)
                {
                    if i + 2 == num_guesses && (conts[0] as i32) < (ContinuityType::G1 as i32) {
                        // save the start point
                        solution.start.parameters[0] = bez_ivl.evaluate(0.0);
                    } else if i + 1 == num_guesses
                        && (conts[1] as i32) < (ContinuityType::G1 as i32)
                    {
                        // save the end point
                        solution.start.parameters[0] = bez_ivl.evaluate(1.0);
                    } else {
                        continue;
                    }
                }

                if op == SolverOperation::SignedPivotMinimize {
                    // set SolutionValue = CCW angle from sTestPnt to sSolPt
                    let sol_pt = curve.evaluate_point(k, solution.start.parameters[0])?;
                    solution.start.solution_value =
                        proj_signed_angle(test_pnt, sol_pt, vectors[0], vectors[1], direction)?;
                } else if op == SolverOperation::ProjectedTangentThroughPoint {
                    // Ok: The return function value is not the distance between the points.
                } else {
                    // set SolutionValue = appropriate signed testPoint/CurvePoint distance
                    solution.start.solution_value = match proj_dist(
                        k,
                        curve,
                        solution.start.parameters[0],
                        test_pnt,
                        self.opt(0)?,
                        true,
                        direction,
                        op,
                    ) {
                        Ok(d) => d,
                        Err(e) if e.is_unported() => return Err(e),
                        Err(e) => status_value(&e),
                    };
                }
            } else {
                // else try SmCurve::LocalPointSolve for current guess
                let opts = PointSolveOptions {
                    distance_tolerance: Some(tol_3d),
                    target_distance: Some(self.base.at_distance_sq),
                    vectors: opt_slice,
                };
                match curve.local_point_solve::<O>(k, &bez_ivl, local_op, test_pnt, &opts, guess) {
                    Ok((f, s)) => {
                        found_solution = f;
                        merge_point_solution(&mut solution, f, &s, local_op);
                    }
                    Err(e) if e.is_unported() => return Err(e),
                    Err(_) => continue, // If have an error move on to next guess
                }
            }

            // quit on 1st guesses that fail (except for SM_SO_NORMALIZE)
            if i == 0 && !found_solution {
                if op == SolverOperation::Normalize {
                    continue;
                }
                return Ok(needs_more_subdivision);
            }

            // skip same answers from different guesses
            if last_found_solution
                && found_solution
                && is_zero(last_deviation - solution.start.solution_value)
                && is_zero(last_parameter - solution.start.parameters[0])
            {
                continue;
            }

            // remember last unique solution
            if found_solution {
                last_found_solution = true;
                last_deviation = solution.start.solution_value;
                last_parameter = solution.start.parameters[0];
            }

            // set outputs
            if i == 0 {
                solution_value = solution.start.solution_value;
            }

            // quit for out-of-tolerance intersection solutions
            if op == SolverOperation::Intersect && solution.start.solution_value > tol_3d {
                return Ok(needs_more_subdivision);
            }

            // move on to next guess points for nonZero RayFires
            if op == SolverOperation::Rayfire {
                let scaled_zero = EFF_ZERO * (1.0 + test_pnt.max_dimension());
                if solution.start.solution_value < -scaled_zero {
                    continue;
                }
            }

            // now Cull Boundary Pt Solutions when real solution will be in next segment
            let mut bndry: Option<(Point3, Vec3)> = None;
            if is_zero(solution.start.parameters[0] - bez_ivl.min())
                && (conts[0] as i32) >= (ContinuityType::G1 as i32)
            {
                bndry = Some((start_pt[0], start_pt[1])); // been negated: points outside
            }
            if is_zero(solution.start.parameters[0] - bez_ivl.max())
                && (conts[1] as i32) >= (ContinuityType::G1 as i32)
            {
                bndry = Some((end_pt[0], end_pt[1])); // points outside interval
            }

            // when solution is a tolerant boundary solution to a non SM_SO_NORMALIZE problem
            if let Some((bndry_pt, bndry_vec)) = bndry {
                if local_op != SolverOperation::Normalize
                    && !is_zero_to_tol(solution.start.solution_value, EFF_ZERO * 100.0)
                {
                    // skip tolerant answers for problems that should solve exactly
                    if local_op == SolverOperation::AtDistance {
                        continue;
                    }

                    // Solution/BoundaryPt vector
                    let sol_vec = test_pnt - bndry_pt;
                    let mut ang_from_perp_rad = 0.0;
                    let mut outside_dir = false;

                    if matches!(
                        local_op,
                        SolverOperation::Minimize
                            | SolverOperation::Maximize
                            | SolverOperation::Intersect
                            | SolverOperation::Rayfire
                    ) {
                        if sol_vec.length_squared() > EFF_ZERO_SQ {
                            // (status ignored; 0 on failure)
                            ang_from_perp_rad = bndry_vec.angle_between(sol_vec).unwrap_or(0.0);
                        }
                        ang_from_perp_rad = (ang_from_perp_rad - PI / 2.0).abs();
                        let dot = bndry_vec.dot(sol_vec);
                        outside_dir = dot > 0.0; // TRUE = sSolVec points outside interval
                    } else if local_op == SolverOperation::Signed3dDirectedMinimize {
                        ang_from_perp_rad = self.opt(0)?.angle_between(sol_vec).unwrap_or(0.0);
                    }

                    // skip solutions with tolerant angles that will get better in the
                    // next interval
                    if !is_zero_to_tol(ang_from_perp_rad, EFF_ZERO_RAD) && outside_dir {
                        continue;
                    }
                }
            }

            // If we get an answer back which is the same as the guess point
            // then we might have an interval which matches not just a single point
            if is_zero(solution.start.parameters[0] - guess)
                && is_zero(solution.start.solution_value - solution_value)
            {
                coin_solutions.push(solution);
                continue;
            }

            if !found_solution {
                continue;
            }

            // save the solution
            solution.nodes[0] = branch[0];
            solution.nodes[1] = branch[1];
            solution.num_variables = 1;
            solution.num_objects = 1;
            solution.objects[0] = curve_obj;

            // add solution to sorted sSolution array
            self.add_solution(&solution, by_value_main)?;
        }

        let by_value_coin = matches!(
            op,
            SolverOperation::Minimize
                | SolverOperation::Maximize
                | SolverOperation::SignedDirectedMinimize
                | SolverOperation::Signed3dDirectedMinimize
                | SolverOperation::DirectedMinimize
                | SolverOperation::DirectedMaximize
                | SolverOperation::ProjectedMinimize
                | SolverOperation::ProjectedMaximize
        ) || (op == SolverOperation::Normalize
            && requested == SolutionRequested::Single);

        // If there is less than 3 which are coincident then just put them on to the list.
        // If there are 3 then assume that it is a coincident segment.
        if coin_solutions.len() < 3 {
            for coin in &mut coin_solutions {
                coin.solution_type = SolutionType::SingleValue;
                coin.nodes[0] = branch[0];
                coin.nodes[1] = branch[1];
                coin.num_variables = 1;
                coin.num_objects = 1;
                coin.objects[0] = curve_obj;
            }
            for coin in &coin_solutions {
                self.add_solution(coin, by_value_coin)?;
            }
        } else {
            // Coincident segment found
            let mut num_done = 0;
            let mut done_first = false;
            let mut done_last = false;
            let mut ivl_solution: Solution<O> = Solution::new();
            ivl_solution.solution_type = SolutionType::RangeOfValues;

            for coin in &coin_solutions {
                if is_zero(coin.start.parameters[0] - bez_ivl.min()) {
                    num_done += 1;
                    done_first = true;
                    ivl_solution.start = coin.start;
                }
                if is_zero(coin.start.parameters[0] - bez_ivl.max()) {
                    num_done += 1;
                    done_last = true;
                    ivl_solution.end = coin.start;
                }
            }

            if num_done == 2 && done_first && done_last {
                ivl_solution.nodes[0] = branch[0];
                ivl_solution.nodes[1] = branch[1];
                ivl_solution.num_variables = 1;
                ivl_solution.solution_type = SolutionType::RangeOfValues;
                ivl_solution.num_objects = 1;
                ivl_solution.objects[0] = curve_obj;
                self.add_solution(&ivl_solution, by_value_coin)?;
            }
        }

        Ok(needs_more_subdivision)
    }
}

// ---- GlobalPointSolve -----------------------------------------------------------------

/// The optional inputs of [`Curve::global_point_solve`].
#[derive(Debug, Clone, Copy)]
pub struct GlobalPointSolveOptions<'a, O> {
    /// `cpdOptTargetDistance`: the target distance of
    /// [`SolverOperation::AtDistance`] (required there), or the distance limit of the
    /// minimising and maximising operations.
    pub target_distance: Option<f64>,
    /// `cpOptVectors`: the vectors of the ray, directed and projected operations.
    pub opt_vectors: Option<&'a [Vec3]>,
    /// The handle of this curve, named by its solutions. With this and
    /// [`GlobalPointSolveOptions::analytic_handle`] both `None` the solutions name no
    /// object, but are merged as upstream's (which name the curve).
    pub handle: Option<O>,
    /// The handle of the curve cache's analytic replacement (an `SmLine` the cache
    /// makes of a straight B-spline), named by its fast drop's solutions.
    pub analytic_handle: Option<O>,
    /// `SmTol::GetZoneTol3d(const SmObject*)` of the handles, which a
    /// [`SolverOperation::Rayfire`] solve reads (`GetObjectTolerance`); `None` when
    /// the caller cannot resolve them (that solve then returns
    /// [`SolidError::Unported`]).
    pub zone_tol_3d: Option<ObjectZoneTol<'a, O>>,
}

impl<O> Default for GlobalPointSolveOptions<'_, O> {
    fn default() -> Self {
        Self {
            target_distance: None,
            opt_vectors: None,
            handle: None,
            analytic_handle: None,
            zone_tol_3d: None,
        }
    }
}

/// `DropPointFast(interval, op, point, NULL, tol, target, requested, ..)` of `curve`
/// naming `handle` (base: `SM_ERR`; a line drops the point onto itself, an ellipse
/// handles circles).
fn drop_point_fast<O: Copy>(
    k: &Kernel,
    curve: &Curve,
    interval: &Extent1d,
    op: SolverOperation,
    point: Point3,
    tol: f64,
    target_distance: Option<f64>,
    requested: SolutionRequested,
    handle: Option<O>,
) -> Result<Vec<Solution<O>>> {
    match curve.kind() {
        CurveKind::Line(l) => l.drop_point_fast(interval, op, point, tol, target_distance, handle),
        CurveKind::Ellipse(e) => e.drop_point_fast(
            k,
            interval,
            op,
            point,
            None,
            tol,
            target_distance,
            requested,
            handle,
        ),
        _ => Err(SolidError::sm_err("curve::curve::p1::drop_point_fast")),
    }
}

impl Curve {
    /// `GlobalPointSolve`: the points of the curve over `interval` solving `op` for
    /// `test_point` (closest / farthest, normal feet, intersections within
    /// `dist_tol`, points at a distance, rays, directed and projected extrema). For
    /// [`SolverOperation::Minimize`] and [`SolverOperation::Intersect`] the line drop
    /// is tried first, then the drop onto the cache's analytic replacement (polished
    /// by [`Curve::local_point_solve`]); otherwise the point is solved against every
    /// span of the curve's cache tree by the curve/point global solver.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` when the cache cannot be built or
    /// [`SolverOperation::AtDistance`] has no target distance; those of the polishing
    /// local solve; [`SolidError::Unported`] from unported callees. Failures of the tree solve itself are ignored, as upstream.
    pub fn global_point_solve<O: SolutionObject>(
        &self,
        k: &Kernel,
        interval: &Extent1d,
        op: SolverOperation,
        test_point: Point3,
        dist_tol: f64,
        requested: SolutionRequested,
        opts: &GlobalPointSolveOptions<'_, O>,
    ) -> Result<SolutionArray<O>> {
        // no handles: upstream's solutions still name this curve (whose natural
        // interval IdenticalSolutions reads); the caller gets them without objects
        if opts.handle.is_none() && opts.analytic_handle.is_none() {
            let ivl = self.natural_interval();
            let inner = GlobalPointSolveOptions {
                target_distance: opts.target_distance,
                opt_vectors: opts.opt_vectors,
                handle: Some(OperandObject::Curve(0, ivl)),
                analytic_handle: Some(OperandObject::Curve(2, ivl)),
                zone_tol_3d: None,
            };
            return Ok(self
                .global_point_solve_cached(
                    k, interval, op, test_point, dist_tol, requested, &inner, &mut None,
                )?
                .without_objects());
        }
        self.global_point_solve_cached(
            k, interval, op, test_point, dist_tol, requested, opts, &mut None,
        )
    }

    /// [`Curve::global_point_solve`] for a caller that names no handles, keeping the
    /// curve's cache in `cache` across calls as [`Curve::global_point_solve_cached`]
    /// does. The cache names the stand-in handle of the unnamed solve (the curve with its
    /// natural interval), so the slot must belong to this curve, unchanged since the
    /// cache was built.
    ///
    /// # Errors
    ///
    /// As [`Curve::global_point_solve`].
    pub(crate) fn global_point_solve_unnamed_cached<O: SolutionObject>(
        &self,
        k: &Kernel,
        interval: &Extent1d,
        op: SolverOperation,
        test_point: Point3,
        dist_tol: f64,
        requested: SolutionRequested,
        target_distance: Option<f64>,
        cache: &mut Option<CurveCache<OperandObject>>,
    ) -> Result<SolutionArray<O>> {
        let ivl = self.natural_interval();
        let inner = GlobalPointSolveOptions {
            target_distance,
            opt_vectors: None,
            handle: Some(OperandObject::Curve(0, ivl)),
            analytic_handle: Some(OperandObject::Curve(2, ivl)),
            zone_tol_3d: None,
        };
        Ok(self
            .global_point_solve_cached(
                k, interval, op, test_point, dist_tol, requested, &inner, cache,
            )?
            .without_objects())
    }

    /// [`Curve::global_point_solve`] keeping the curve's cache in `cache` across calls,
    /// as upstream keeps it on the curve (`GetOrCreateObjectCache`): the cache is built
    /// where the solve first needs it (with the same errors) and reused by later calls
    /// given the same slot. The slot must belong to this curve, unchanged since the
    /// cache was built, and to the same `opts.handle` (the cache names it in its
    /// solutions); the build is deterministic, so a reused cache holds the same spans,
    /// tree and analytic replacement as a fresh one.
    ///
    /// # Errors
    ///
    /// As [`Curve::global_point_solve`].
    pub(crate) fn global_point_solve_cached<O: SolutionObject>(
        &self,
        k: &Kernel,
        interval: &Extent1d,
        op: SolverOperation,
        test_point: Point3,
        dist_tol: f64,
        requested: SolutionRequested,
        opts: &GlobalPointSolveOptions<'_, O>,
        cache: &mut Option<CurveCache<O>>,
    ) -> Result<SolutionArray<O>> {
        let target = opts.target_distance;

        // Try quick solves when MINIMIZING and INTERSECTING (without using curve cache)
        if op == SolverOperation::Minimize || op == SolverOperation::Intersect {
            if let Some(sols) = ignored(drop_point_fast(
                k,
                self,
                interval,
                op,
                test_point,
                dist_tol,
                target,
                requested,
                opts.handle,
            ))? {
                let mut out = SolutionArray::new();
                for s in &sols {
                    out.add(s);
                }
                return Ok(out);
            }
        }

        // get target curve's cache
        let mut solutions: SolutionArray<O> = SolutionArray::new();
        if cache.is_none() {
            *cache = Some(match make_curve_cache::<O>(k, self, opts.handle) {
                Ok(c) => c,
                Err(e) if e.is_unported() => return Err(e),
                Err(_) => return Err(SolidError::null_pointer(OP_GLOBAL)),
            });
        }
        let cache = cache.as_mut().ok_or(SolidError::null_pointer(OP_GLOBAL))?;

        // Analytical Curve Optimization
        if op == SolverOperation::Minimize || op == SolverOperation::Intersect {
            if let Some(analytic) = cache.analytic_curve(k)? {
                // (the cache's analytic curve is this curve when it is analytic)
                let analytic_handle = if self.is_analytic() {
                    opts.handle
                } else {
                    opts.analytic_handle
                };
                if let Some(sols) = ignored(drop_point_fast(
                    k,
                    analytic,
                    interval,
                    op,
                    test_point,
                    dist_tol,
                    target,
                    requested,
                    analytic_handle,
                ))? {
                    for s in &sols {
                        solutions.add(s);
                    }

                    // when there are solutions that need tightening
                    if let Some(first) = solutions.get(0).copied() {
                        let anal_value = first.start.parameters[0];
                        let anal_solution_value = first.start.solution_value;

                        // when solution could be tighter
                        if anal_solution_value > EFF_ZERO {
                            let (found, sol) = self.local_point_solve::<O>(
                                k,
                                interval,
                                op,
                                test_point,
                                &PointSolveOptions::default(),
                                anal_value,
                            )?;

                            // when LocalPointSolve converged to a better answer
                            if found && sol.start.solution_value < anal_solution_value {
                                if let Some(slot) = solutions.get_mut(0) {
                                    *slot = sol;
                                }
                            }
                        }
                    }

                    // all done
                    return Ok(solutions);
                }
            }
        }

        // Execute a Point/CurveLeafNode solution for every LeafNode in the target
        // curve's cache.
        let point_tree: Tree<O> = Tree::from_point(test_point, 0.0, false);
        let curve_tree = cache.tree().ok_or(SolidError::null_pointer(OP_GLOBAL))?;

        // Construct and init global solver
        let mut gs = CpGlobalSolver::new(k, &point_tree, curve_tree, *interval).with_curve(self);
        gs.zone_tol_3d = opts.zone_tol_3d;

        // set atDistance value - when appropriate
        if op == SolverOperation::AtDistance {
            let d = target.ok_or(SolidError::null_pointer(OP_GLOBAL))?;
            gs.base.set_at_distance(d);
        }

        // Set dBestAnswerSoFarSq for various cases
        let best_answer_so_far_sq = match op {
            SolverOperation::Rayfire
            | SolverOperation::Minimize
            | SolverOperation::DirectedMinimize
            | SolverOperation::ProjectedMinimize
            | SolverOperation::SignedDirectedMinimize
            | SolverOperation::Signed3dDirectedMinimize => match target {
                Some(d) => {
                    if d >= 0.0 {
                        d * d
                    } else {
                        -d * d
                    }
                }
                None => BIG_DOUBLE,
            },
            SolverOperation::Maximize
            | SolverOperation::DirectedMaximize
            | SolverOperation::ProjectedMaximize => match target {
                Some(d) => d * d,
                None => 0.0,
            },
            _ => BIG_DOUBLE,
        };

        // execute the spatial decomposition tree leafNode/Point solutions (status
        // ignored)
        let request = SolveRequest {
            operation: op,
            solution_requested: requested,
            tolerance_3d: dist_tol,
            best_answer_so_far_sq,
            opt_vectors: opts.opt_vectors.unwrap_or(&[]),
        };
        ignored(gs.solve_it(&request, &mut solutions))?;

        Ok(solutions)
    }
}

// ---- TessellateByBisection / Tessellate ---------------------------------------------

impl Curve {
    /// `TessellateByBisection`: sample points of the curve over `interval` whose
    /// segments pass the chord height ratio, tangent angle (degrees) and maximum 3D
    /// distance tests ([`test_step`]; a zero tolerance is ignored). A degenerate curve
    /// gives its start point (zero tangent); a line, when no maximum distance is
    /// asked for, its two ends (with the start tangent). Otherwise steps grow by
    /// doubling and shrink by halving from the whole interval, each accepted or
    /// growing step being re-tested at three points per knot span it covers.
    ///
    /// # Errors
    ///
    /// Those of the evaluations and of the degeneracy and line tests.
    pub fn tessellate_by_bisection(
        &self,
        k: &Kernel,
        interval: &Extent1d,
        chord_height_tol: f64,
        angle_tol_deg: f64,
        max_3d_distance: f64,
    ) -> Result<Tessellation> {
        let mut out = Tessellation::default();

        // locals
        let max_3d_distance_squared = max_3d_distance * max_3d_distance;

        // SmCurve locals - knots (status ignored)
        let (knots, _) = knots_ignoring_status(self.knots_with_status(k, None))?;

        // get this SmCurve's start position and derivative
        let mut cur_pv: [Vec3; 2] = eval_n(k, self, interval.min(), 1, true, true)?; // nonZeroTangent values

        // when SmCurve's start derivative is zero
        if cur_pv[1].length_squared() < EFF_ZERO_SQ {
            // get SmCurve's pos and der values near beginning
            let tmp: [Vec3; 2] = eval_n(k, self, interval.evaluate(0.001), 1, true, true)?;

            // if der near end is zero - make one up to continue processing
            if tmp[1].length_squared() < EFF_ZERO_SQ {
                cur_pv[1] = Vec3::new(1.0, 1.0, 1.0); // Recover by adding arbitrary vector here
            } else {
                cur_pv[1] = tmp[1];
            }
        }

        // check for degenerate or linear curves
        if self.is_degenerate(k, EFF_ZERO, None)? {
            // degenerate curve is a point - load one tessellation point and exit
            out.parameters.push(interval.min());
            out.points.push(cur_pv[0]);
            out.tangents.push(Vec3::ZERO);
            return Ok(out);
        }

        // If linear, and we're not concerned with segment length, return the whole line.
        let tol = if chord_height_tol > EFF_ZERO {
            chord_height_tol / 100.0
        } else {
            EFF_ZERO * 100.0
        };
        if max_3d_distance < EFF_ZERO && self.is_line(k, 5, tol)?.is_some() {
            // load two tessellation points and exit
            out.parameters.push(interval.min());
            out.parameters.push(interval.max());
            out.points.push(cur_pv[0]);
            cur_pv[0] = self.evaluate_point(k, interval.max())?;
            out.points.push(cur_pv[0]);
            out.tangents.push(cur_pv[1]);
            out.tangents.push(cur_pv[1]);
            return Ok(out);
        }

        // Get step size estimates
        let min_step_size = interval.length() / 100000.0;

        // load start param and value into output arrays
        output(
            cur_pv,
            interval.min(),
            Some(&mut out.parameters),
            Some(&mut out.points),
            Some(&mut out.tangents),
        );

        // begin at the curve start with step size set to interval size
        let mut cur_t = interval.min();
        let mut param_step_size = interval.length();
        let scaled_zero = EFF_ZERO * (1.0 + interval.max_dimension());

        // locals
        let mut done = false;
        let mut last_step_size = 0.0;
        let mut num_segments: u64 = 0;

        // search for next tessellation point
        while !done {
            num_segments += 1;

            let mut next_pv: [Vec3; 2];
            let mut step_direction: i64 = 0;
            let mut tests_made: u64 = 0;
            loop {
                // count the number of iterations to find the next tessellation point
                tests_made += 1;

                // limit the next step size to the interval's max value
                if cur_t + param_step_size > interval.max() {
                    param_step_size = interval.max() - cur_t;
                }

                // make sure that our last step is not too close to the end of the curve
                if cur_t + param_step_size < interval.max() - scaled_zero
                    && cur_t + 2.0 * param_step_size > interval.max() + scaled_zero
                {
                    param_step_size = interval.max() - cur_t;
                }

                next_pv = eval_n(k, self, cur_t + param_step_size, 1, true, true)?; // nonZeroTangent values
                if next_pv[1].length_squared() < EFF_ZERO_SQ {
                    let tmp: [Vec3; 2] =
                        eval_n(k, self, cur_t + param_step_size * 0.99, 1, true, true)?;
                    if tmp[1].length_squared() < EFF_ZERO_SQ {
                        next_pv[1] = cur_pv[1];
                    } else {
                        next_pv[1] = tmp[1];
                    }
                }

                if param_step_size > min_step_size - EFF_ZERO {
                    let mut result = test_step(
                        param_step_size,
                        cur_pv,
                        next_pv,
                        chord_height_tol,
                        angle_tol_deg,
                        max_3d_distance_squared,
                    );

                    // If we don't have a lot of segments force some additional testing.
                    if result == TessStepResult::Accept || result == TessStepResult::Increase {
                        // generate 3 sample points per knot interval
                        let params = find_test_params(cur_t, cur_t + param_step_size, &knots);
                        for &tmp_t in &params {
                            let next_pv2: [Vec3; 2] = eval_n(k, self, tmp_t, 1, true, true)?; // nonZeroTangent values
                            if next_pv2[1].length_squared() < EFF_ZERO_SQ {
                                continue;
                            }
                            let result2 = test_step(
                                tmp_t - cur_t,
                                cur_pv,
                                next_pv2,
                                chord_height_tol,
                                angle_tol_deg,
                                max_3d_distance_squared,
                            );
                            if result2 == TessStepResult::Cut {
                                result = result2;
                                break;
                            }
                        }
                    }

                    if tests_made > 100 {
                        break; // Just accept what we have so far
                    }

                    if result == TessStepResult::Increase
                        && num_segments == 1
                        && are_same(param_step_size, interval.length())
                    {
                        let test_tol = EFF_ZERO * (1.0 + cur_pv[0].max_dimension());
                        if next_pv[0].distance(cur_pv[0]) < test_tol {
                            result = TessStepResult::Cut;
                        }
                    }
                    if result == TessStepResult::Accept {
                        break;
                    }
                    if result == TessStepResult::Cut {
                        if step_direction == 1 {
                            param_step_size = last_step_size;
                            next_pv = eval_n(k, self, cur_t + param_step_size, 1, true, true)?;
                            break;
                        }
                        param_step_size /= 2.0;

                        step_direction = -1;
                        if param_step_size < min_step_size {
                            param_step_size = min_step_size;
                            next_pv = eval_n(k, self, cur_t + param_step_size, 1, true, true)?;
                            break;
                        }
                        continue;
                    } else if result == TessStepResult::Increase {
                        step_direction = 1;
                        last_step_size = param_step_size;

                        // If the current step goes effectively all the way to the end,
                        // accept it: evaluate there and break the while loop.
                        if cur_t + param_step_size > interval.max() - scaled_zero {
                            param_step_size = interval.max() - cur_t;
                            next_pv = eval_n(k, self, interval.max(), 1, true, true)?;
                            break;
                        }

                        param_step_size *= 2.0; // Double the step.

                        // If the doubled step goes past end, set it to the end
                        if cur_t + param_step_size > interval.max() + EFF_ZERO {
                            param_step_size = interval.max() - cur_t;
                        }

                        continue;
                    }
                } else if param_step_size < min_step_size
                    && param_step_size < interval.max() - cur_t
                {
                    param_step_size = min_step_size;
                    next_pv = eval_n(k, self, cur_t + param_step_size, 1, true, true)?;
                    break;
                }

                // If we made it to here no adjustment was required
                break;
            }

            // If made it to here then the point was accepted.
            cur_t += param_step_size;
            output(
                next_pv,
                cur_t,
                Some(&mut out.parameters),
                Some(&mut out.points),
                Some(&mut out.tangents),
            );
            cur_pv = next_pv;
            if are_same(cur_t, interval.max()) {
                done = true;
            }
            let est_step = param_step_size;
            let max_step = interval.max() - cur_t; // Max step possible
            param_step_size = max_step.min(est_step);
        }

        Ok(out)
    }

    /// `Tessellate` (virtual): the line's even parameter steps (its tolerances
    /// ignored), the B-spline override (the same body as the base), or the base body
    /// [`Curve::tessellate_base`].
    ///
    /// # Errors
    ///
    /// Those of [`Curve::tessellate_base`] and of the B-spline override.
    pub fn tessellate(
        &self,
        k: &Kernel,
        interval: &Extent1d,
        chord_height_tol: f64,
        angle_tol_deg: f64,
        min_segments: u64,
    ) -> Result<Tessellation> {
        match self.kind() {
            CurveKind::Line(l) => l.tessellate(
                interval,
                min_segments,
                TessellationRequest {
                    parameters: true,
                    points: true,
                    tangents: true,
                },
            ),
            CurveKind::BSpline(c) => c.tessellate(
                k,
                interval,
                chord_height_tol,
                angle_tol_deg,
                min_segments as usize,
            ),
            CurveKind::Hyperbola(c) => c.base().tessellate(
                k,
                interval,
                chord_height_tol,
                angle_tol_deg,
                min_segments as usize,
            ),
            CurveKind::Ellipse(c) => c.base().tessellate(
                k,
                interval,
                chord_height_tol,
                angle_tol_deg,
                min_segments as usize,
            ),
            CurveKind::Parabola(c) => c.base().tessellate(
                k,
                interval,
                chord_height_tol,
                angle_tol_deg,
                min_segments as usize,
            ),
            CurveKind::Hermite(_)
            | CurveKind::Conic(_)
            | CurveKind::Projected(_)
            | CurveKind::Iso(_)
            | CurveKind::Offset(_)
            | CurveKind::CrvInVolume(_)
            | CurveKind::CrvOnSurf(_)
            | CurveKind::TangentField(_)
            | CurveKind::Composite(_) => {
                self.tessellate_base(k, interval, chord_height_tol, angle_tol_deg, min_segments)
            }
        }
    }

    /// `SmCurve::Tessellate` (the base-class body): [`Curve::tessellate_by_bisection`]
    /// with the maximum segment length `ApproximateLength(5) / min_segments` (none
    /// for at most one segment).
    ///
    /// # Errors
    ///
    /// Those of [`Curve::tessellate_by_bisection`] and the length.
    pub fn tessellate_base(
        &self,
        k: &Kernel,
        interval: &Extent1d,
        chord_height_tol: f64,
        angle_tol_deg: f64,
        min_segments: u64,
    ) -> Result<Tessellation> {
        // init MaxLength control parameter
        let max_length = if min_segments > 1 {
            self.approximate_length(k, interval, 5, false)?.0 / (min_segments as f64 * 1.0)
        } else {
            0.0
        };

        // pass the call along
        self.tessellate_by_bisection(k, interval, chord_height_tol, angle_tol_deg, max_length)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// source issue #527 (WR-383): a forward range whose end sits 0.001 off
    /// the crossing; the local intersection moves the end by 0.001 of the range, which
    /// upstream's test (|frac| near 0) refuses and the opt-in (|1 - frac|) accepts.
    #[test]
    fn issue_527_refine_cc_range_end() {
        use crate::geom::attribute::ObjectKey;
        use crate::geom::solution_array::{ObjectKind, Solution, SolutionType};
        use crate::topology::ObjectHandle;
        let line = |a: [f64; 3], b: [f64; 3]| {
            let p = |c: [f64; 3]| Vec3::new(c[0], c[1], c[2]);
            Curve::from(BSplineCurve::create_line_segment(3, p(a), p(b)).unwrap())
        };
        // crv1 along x; crv2 crosses it at x = 0.999, at t2 = 1 / 1.001
        let crv1 = line([0.0, 0.0, 0.0], [1.0, 0.0, 0.0]);
        let crv2 = line([0.999, 1.0, 0.0], [0.999, -0.001, 0.0]);
        let h = |key| ObjectHandle {
            kind: ObjectKind::Curve,
            key: ObjectKey(key),
        };
        let run = |k: &Kernel| {
            let mut sol = Solution::<ObjectHandle>::new();
            sol.solution_type = SolutionType::RangeOfValues;
            sol.num_variables = 2;
            sol.num_objects = 2;
            sol.objects[0] = Some(h(1));
            sol.objects[1] = Some(h(2));
            sol.start.parameters[0] = 0.0;
            sol.start.parameters[1] = 0.0;
            sol.start.solution_value = 0.0;
            sol.end.parameters[0] = 1.0;
            sol.end.parameters[1] = 1.0;
            sol.end.solution_value = 0.001;
            let mut sols = SolutionArray::with_capacity(1);
            sols.try_set_size(1).unwrap();
            sols.set_at(0, &sol).unwrap();
            refine_cc_solutions(k, &mut sols, |o| match o.map(|o| o.key) {
                Some(ObjectKey(1)) => Some(&crv1),
                Some(ObjectKey(2)) => Some(&crv2),
                _ => None,
            })
            .unwrap();
            sols.get(0).unwrap().end
        };
        let mut k = Kernel::default();
        let end = run(&k);
        assert_eq!((end.parameters[0], end.solution_value), (1.0, 0.001));
        k.fixes.refine_cc_range_ends = true;
        let end = run(&k);
        assert!((end.parameters[0] - 0.999).abs() < 1e-9, "{end:?}");
        assert!(end.solution_value < 1e-9, "{end:?}");
    }

    fn cubic() -> Curve {
        Curve::from(HermiteCurve::new(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(3.0, 3.0, 0.0),
            Vec3::new(4.0, 0.0, 0.0),
            Vec3::new(3.0, -3.0, 0.0),
            3,
        ))
    }

    #[test]
    fn tessellation_covers_the_interval() {
        let k = &Kernel::default();
        let c = cubic();
        let ivl = Extent1d::from_min_max(0.0, 1.0);
        let t = c.tessellate_by_bisection(k, &ivl, 0.01, 5.0, 0.0).unwrap();
        assert!(t.parameters.len() > 2);
        assert_eq!(t.parameters.first().copied(), Some(0.0));
        assert!((t.parameters.last().copied().unwrap() - 1.0).abs() < 1e-12);
        assert!(t.parameters.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(t.points.len(), t.parameters.len());
    }

    #[test]
    fn approximation_is_within_tolerance() {
        let k = &Kernel::default();
        let c = cubic();
        let r = c
            .approximate_curve(
                &Kernel::default(),
                &[0.0, 1.0],
                1e-3,
                &ApproximateCurveOptions::default(),
            )
            .unwrap();
        assert!(r.achieved_tol.unwrap() <= 1e-3);
        let p = r.curve.evaluate_point(k, 0.5).unwrap();
        assert!(p.distance(c.evaluate_point(k, 0.5).unwrap()) < 1e-2);
    }

    #[test]
    fn approximation_bounds_outside_domain_refinement_without_rejecting_tolerance() {
        let k = Kernel::default();
        let c = Curve::from(BSplineCurve::create_line_segment(3, Vec3::ZERO, Vec3::X).unwrap());
        let opts = ApproximateCurveOptions {
            create_analytics: false,
            match_parameterization: true,
            just_copy_bsplines: false,
        };
        // A negative tolerance still follows native subdivision and force-pass
        // semantics over the natural interval; it is not replaced or rejected.
        let natural = c
            .approximate_curve_base(&k, &[0.0, 1.0], -1e300, &opts)
            .unwrap();
        assert_eq!(natural.curve.natural_interval(), c.natural_interval());
        assert_eq!(
            natural.curve.evaluate_point(&k, 0.5).unwrap(),
            Vec3::X * 0.5
        );

        // The same stopping rule over an enormous clamped interval would need
        // trillions of spans, despite this curve having only two control points.
        assert!(matches!(
            c.approximate_curve_base(&k, &[0.0, 1e100], -1e300, &opts),
            Err(SolidError::Upstream { code: 1001, .. })
        ));
    }

    #[test]
    fn approximation_fails_when_native_midpoint_overflows() {
        let c = Curve::from(BSplineCurve::create_line_segment(3, Vec3::ZERO, Vec3::X).unwrap());
        let opts = ApproximateCurveOptions {
            create_analytics: false,
            match_parameterization: true,
            just_copy_bsplines: false,
        };
        assert!(matches!(
            c.approximate_curve_base(&Kernel::default(), &[f64::MAX * 0.75, f64::MAX], 0.0, &opts,),
            Err(SolidError::Upstream { code: 1001, .. })
        ));
    }

    #[test]
    fn global_point_solve_finds_closest_point() {
        let k = Kernel::default();
        let c = cubic();
        let ivl = Extent1d::from_min_max(0.0, 1.0);
        let sols = c
            .global_point_solve::<Obj>(
                &k,
                &ivl,
                SolverOperation::Maximize,
                Vec3::new(2.0, -5.0, 0.0),
                1e-6,
                SolutionRequested::Single,
                &GlobalPointSolveOptions::default(),
            )
            .unwrap();
        assert_eq!(sols.len(), 1);
    }

    #[test]
    fn point_solution_range_is_degenerate() {
        let k = &Kernel::default();
        let c = cubic();
        let mut s: Solution<Obj> = Solution::new();
        s.solution_type = SolutionType::SingleValue;
        assert_eq!(
            c.classify_intersecting_range_solution(k, &c, &s).unwrap(),
            (true, None)
        );
    }

    #[derive(Debug, Clone, Copy, PartialEq)]
    struct Obj;
    impl SolutionObject for Obj {
        fn kind(&self) -> crate::geom::solution_array::ObjectKind {
            crate::geom::solution_array::ObjectKind::Other
        }
    }

    #[test]
    fn span_accuracy_of_exact_hermite_passes() {
        let k = &Kernel::default();
        let c = cubic();
        let a = c.evaluate(k, 0.0, 1, true, false).unwrap();
        let b = c.evaluate(k, 1.0, 1, false, false).unwrap();
        let r =
            test_span_accuracy(k, &c, 0.0, [a[0], a[1]], 1.0, [b[0], b[1]], 1e-9, true).unwrap();
        assert!(r.passes);
        assert!(r.max_dist < 1e-12);
    }
}
