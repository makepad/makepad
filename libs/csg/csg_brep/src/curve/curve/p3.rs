//! `SmCurve`, part 3: the normal of a planar curve loop, closing small gaps in a
//! curve chain, stepping a given arc length along a curve, the local (Newton)
//! solvers of the curve (point, curve/curve, curve intersection) and the local solve
//! of the curve property global solver, with their evaluator objects
//! (`SmFindPCExtremaEFO`, `SmBestLineFitENFO`).
//!
//! Upstream: `source/SMLib/src/SmCurve.cpp`, `source/SMLib/inc/SmCurve.h`.
//!
//! | Rust                                              | Upstream                                         |
//! |---------------------------------------------------|--------------------------------------------------|
//! | [`compute_crvs_normal`]                           | `SmCurve::ComputeCrvsNormal` (static)            |
//! | [`crvs_fix_gaps`], [`FixedGaps`]                  | `SmCurve::CrvsFixGaps` (static)                  |
//! | [`Curve::find_parameter_at_arc_length`], [`ArcLengthParameter`] | `SmCurve::FindParameterAtArcLength` |
//! | `Curve::find_parameter_at_arc_length_with_zone_tol` (crate helper) | `SmCurve::FindParameterAtArcLength` (edge-owned curve) |
//! | [`Curve::local_point_solve`], [`PointSolveOptions`] | `SmCurve::LocalPointSolve`                     |
//! | [`Curve::local_curve_solve`], [`local_curve_solve`] | `SmCurve::LocalCurveSolve`                     |
//! | [`local_curve_intersect`], [`CurveIntersection`]  | `SmCurve::LocalCurveIntersect`                   |
//! | `CurvePropertyGs::local_solve_branch` ([`GlobalSolve::local_solve`] of [`CurvePropertyGs`]) | `SmCurvePropertyGS::LocalSolve` |
//! | `EvalFunction for` [`FindPcExtremaEfo`]           | `SmFindPCExtremaEFO::Evaluate`                   |
//! | [`BestLineFitEnfo`]                               | `SmBestLineFitENFO` (class, constructor, `HasIntervals`, `SetIntervals`, `Evaluate`) |
//! | `EvalNFunctions for` [`CcTangentIntEnfo`]         | `SmCCTangentIntENFO::Evaluate` (co-ported, unit `curve.curve_p5`) |
//!
//! Deviations:
//! - **Curve kinds.** `ComputeCrvsNormal` and `CrvsFixGaps` cast every curve to
//!   `SmBSplineCurve`. `ComputeCrvsNormal` only calls virtual members and the
//!   non-virtual `SmCurve::IsLinear`, so it takes any [`Curve`]. `CrvsFixGaps` calls
//!   the non-virtual `SmBSplineCurve::SetControlPoint`: moving an end point of any
//!   kind but [`CurveKind::BSpline`] is `InvalidInput` (upstream writes through the
//!   bad cast). Its curve list is reordered by [`fix_chain_curves_list`] (with
//!   duplicate deletion, as upstream) and returned with the count.
//! - **Out-params.** `FindParameterAtArcLength` returns [`ArcLengthParameter`]:
//!   `SM_ERR_NOT_CONVERGING` (returned upstream *with* the end parameter) is
//!   [`ArcLengthParameter::converged`] `false`. The local solvers return whether an
//!   answer was found with the [`Solution`] they filled in (a fresh
//!   [`Solution::new`] stands for the caller's; fields upstream does not write keep
//!   its values); the solution objects are the caller's handles. A start parameter
//!   below the first knot when stepping backwards (an out of bounds read upstream) is
//!   `InvalidInput`; a failed `SetMinMax` keeps the previous interval, as upstream.
//! - **Owner tolerance.** The crate-private arc-length entry point carries the
//!   owning edge's resolved zone tolerance through every length cache. This is
//!   independent of Newton's integration accuracy; the public entry point keeps
//!   the ordinary unresolved-owner behavior.
//! - **Generic operands.** [`local_curve_solve`] and [`local_curve_intersect`] are
//!   generic over [`CurveEvaluate`] (so the ellipse and B-spline callers can use
//!   them). The `SM_SO_3D_SIGNED_DIRECTED_MINIMIZE` operation builds
//!   `SmProjectedCurve`s of both curves and needs [`Curve`] operands: it is
//!   [`Curve::local_curve_solve`]; the generic function returns `InvalidInput` for it.
//!   Option vectors and values an operation dereferences without a `NULL` check are
//!   `InvalidInput` when missing; upstream's `NER` checks stay `SM_ERR_NULL_POINTER`.
//! - **Property solver.** `SmCurvePropertyGS::LocalSolve` reads the curve back from
//!   the tree node's owner object; here the solver carries it
//!   ([`CurvePropertyGs::curve`], `InvalidInput` when not set) and the solution
//!   objects are the tree's owner handle. `return TRUE` from the `SmStatus` function
//!   (a projected corner at the rotation origin) is status `1`
//!   ([`SolidError::Upstream`]). A node without data is `SM_ERR_NULL_POINTER`
//!   (`NER`); a node whose data is not a leaf span is `InvalidInput` (upstream casts
//!   whatever `m_pData` holds).
//! - **`SmFindPCExtremaEFO::Evaluate`** is implemented for [`Curve`] (it calls the
//!   virtual `IsDegenerate`). An infinite evaluated tangent returns `SM_ERR`
//!   instead of looping forever while dividing infinity by ten; finite tangents
//!   retain upstream's exact scaling operations and order. The separate
//!   [`MAX_PC_SCALE_STEPS`] cap is retained as a backstop and returns
//!   [`SolidError::InvalidInput`] after 400 divisions (a finite tangent needs at
//!   most 310). Upstream loops forever on the infinite rational B-spline tangent
//!   in fuzz `curve.curve_p9` fuzz01222.
//! - **`SmBestLineFitENFO`** keeps the solver's intervals as copies; its
//!   `m_dSolutionSize` / `m_dScaledZero` are fields read through
//!   [`EvalNFunctions::solution_size`] / [`EvalNFunctions::scaled_zero`]. The status
//!   of the tangent branch's `StaticEvaluate` is ignored, as upstream.
//! - **Ignored statuses.** Where upstream ignores a status (the evaluations of
//!   `ComputeCrvsNormal`, `CrvsFixGaps` and `LocalPointSolve`'s final point, the
//!   first span lengths of `FindParameterAtArcLength`, `GetKnots`, `SetControlPoint`,
//!   `SetCanonical`) the error is ignored and the previous value kept, except
//!   [`SolidError::Unported`], which is always propagated.
//! - **Callees of unit `curve.curve_p5`** (`SmCurve::Length`,
//!   `SmCurve::LocalPropertyAnalysis`, `sm_FindAdditionalCpGuess`, `SmCurve::IsPeriodic`
//!   and the `Evaluate` members of `SmCCIntersectENFO` and `SmFindCCNormalENFO`) are
//!   wired. `FindParameterAtArcLength` takes the kernel for `Length` (its curve cache
//!   reads the kernel's tolerances); `Length` of curved spans reaches `SmIntegrator`
//!   (the ported unit `mass.integrator`).

use crate::curve::curve::knots_ignoring_status;
use crate::curve::curve::p2::{
    proj_signed_angle, property_angle, unitize_tangent, Curve, CurveEvaluate, CurveKind,
    CurvePropertyGs, CurvePropertyType,
};
use crate::curve::curve::p4::{
    fix_chain_curves_list, proj_dist, CcIntersectEnfo, CcTangentIntEnfo, ChainedCurves,
    FindCcNormalEnfo, FindPcExtremaEfo,
};
use crate::curve::curve::p5::{find_additional_cp_guess, is_periodic, local_property_analysis};
use crate::curve::projected_curve::{OwnsCurve, ProjectedCurve};
use crate::error::{ignored, Result, SolidError};
use crate::geom::extent1d::{Extent1d, ExtentNd};
use crate::geom::geom_utility::p1::line_closest_point;
use crate::geom::geom_utility::p2::ContinuityType;
use crate::geom::global_solver::{Branch, GlobalSolve, SolverOperation};
use crate::geom::local_solve1d::{EvalFunction, FunctionValue, LocalSolve1d, TerminationReason};
use crate::geom::local_solve_nd::{BoundaryHandler, EvalNFunctions, LocalSolveNd};
use crate::geom::matrix::Matrix;
use crate::geom::solution_array::{Solution, SolutionEnd, SolutionObject, SolutionType, SortKey};
use crate::geom::tol::{are_same, are_same_to_tol};
use crate::geom::tree::NodeData;
use crate::geom::vector3d::{Point3, Vec3};
use crate::kernel::Kernel;
use crate::math::{max2, BIG_DOUBLE, EFF_ZERO, EFF_ZERO_SQ, EFF_ZERO_SQRT, PI};
use crate::nurbs::b_spline_curve::{ControlPointForm, ProjectionType};

const OP_NORMAL: &str = "curve::curve::p3::compute_crvs_normal";
const OP_GAPS: &str = "curve::curve::p3::crvs_fix_gaps";
const OP_ARC: &str = "curve::curve::p3::Curve::find_parameter_at_arc_length";
const OP_POINT: &str = "curve::curve::p3::Curve::local_point_solve";
const OP_CURVE: &str = "curve::curve::p3::local_curve_solve";
const OP_GS: &str = "curve::curve::p3::CurvePropertyGs::local_solve_branch";
const OP_PC: &str = "curve::curve::p3::FindPcExtremaEfo::evaluate";
const OP_LINE_FIT: &str = "curve::curve::p3::BestLineFitEnfo::evaluate";

/// Most divisions by ten `SmFindPCExtremaEFO::Evaluate` makes to bring `|C'|^2` down
/// to `1e4` (not upstream, which loops forever on an infinite tangent). A finite
/// tangent needs at most 310: its squared length overflows to infinity until each
/// component is below about `1e154`, and then needs at most 152 more divisions.
pub const MAX_PC_SCALE_STEPS: u64 = 400;

/// `SM_ERR_NOT_CONVERGING`.
const SM_ERR_NOT_CONVERGING: i32 = 1022;

/// `EvaluatePoint` whose status upstream ignores: the previous value of the output
/// on failure; [`SolidError::Unported`] is propagated.
fn point_keep<C: CurveEvaluate + ?Sized>(
    k: &Kernel,
    curve: &C,
    t: f64,
    prev: Point3,
) -> Result<Point3> {
    Ok(ignored(curve.evaluate_point(k, t))?.unwrap_or(prev))
}

/// `Evaluate` (from the left) into an `N`-entry array; entries not written stay
/// [`Vec3::UNDEF`].
pub(super) fn eval_n<const N: usize, C: CurveEvaluate + ?Sized>(
    k: &Kernel,
    curve: &C,
    t: f64,
    num_derivs: usize,
    from_left: bool,
    non_zero_tangents: bool,
) -> Result<[Vec3; N]> {
    let v = curve.evaluate(k, t, num_derivs, from_left, non_zero_tangents)?;
    let mut out = [Vec3::UNDEF; N];
    for (d, s) in out.iter_mut().zip(v) {
        *d = s;
    }
    Ok(out)
}

/// `v / d` (upstream `operator/`, a plain division: not `operator/=`, which skips
/// tiny divisors).
fn div(v: Vec3, d: f64) -> Vec3 {
    v / d
}

/// Sets `ivl` to `[min, max]`; upstream ignores the status of `SetMinMax`, which
/// leaves the interval unchanged for `min > max`.
fn set_min_max(ivl: &mut Extent1d, min: f64, max: f64) {
    // On failure the previous interval is kept, as upstream.
    let _ = ivl.set_min_max(min, max);
}

/// `m_vStart[i] = v` / `m_vEnd[i] = v`.
pub(crate) fn set_param(end: &mut SolutionEnd, i: usize, v: f64) -> Result<()> {
    *end.param_mut(i)? = v;
    Ok(())
}

impl<C1: CurveEvaluate + ?Sized, C2: CurveEvaluate + ?Sized> EvalNFunctions
    for CcIntersectEnfo<'_, C1, C2>
{
    fn has_intervals(&self) -> bool {
        CcIntersectEnfo::has_intervals(self)
    }

    fn set_intervals(
        &mut self,
        intervals: Option<&ExtentNd>,
        periodicities: Option<&[bool]>,
    ) -> Result<()> {
        CcIntersectEnfo::set_intervals(self, intervals, periodicities);
        Ok(())
    }

    fn solution_size(&self) -> f64 {
        self.solution_size
    }

    fn scaled_zero(&self) -> f64 {
        self.scaled_zero
    }

    /// `SmCCIntersectENFO::Evaluate` ([`CcIntersectEnfo::evaluate_equations`]).
    fn evaluate(
        &mut self,
        x: &[f64],
        f: &mut [f64],
        jacobian: Option<&mut Matrix>,
    ) -> Result<bool> {
        let k = self.kernel;
        self.evaluate_equations(k, x, f, jacobian)
    }
}

impl<C1: CurveEvaluate + ?Sized, C2: CurveEvaluate + ?Sized> EvalNFunctions
    for FindCcNormalEnfo<'_, C1, C2>
{
    /// `SmFindCCNormalENFO::Evaluate` ([`FindCcNormalEnfo::evaluate_equations`]).
    fn evaluate(
        &mut self,
        x: &[f64],
        f: &mut [f64],
        jacobian: Option<&mut Matrix>,
    ) -> Result<bool> {
        let k = self.kernel;
        self.evaluate_equations(k, x, f, jacobian)
    }
}

impl<C1: CurveEvaluate + ?Sized, C2: CurveEvaluate + ?Sized> EvalNFunctions
    for CcTangentIntEnfo<'_, C1, C2>
{
    /// `SmCCTangentIntENFO::Evaluate`: passes the call along to
    /// [`CcTangentIntEnfo::static_evaluate`] (co-ported from unit `curve.curve_p5`).
    fn evaluate(
        &mut self,
        x: &[f64],
        f: &mut [f64],
        jacobian: Option<&mut Matrix>,
    ) -> Result<bool> {
        let k = self.kernel;
        // pass the call along
        CcTangentIntEnfo::static_evaluate(k, self.curve1, self.curve2, x, f, jacobian)
    }
}

// ---- ComputeCrvsNormal ------------------------------------------------------------------

/// The unit normal of a (planar) sequence of curves (`SmCurve::ComputeCrvsNormal`):
/// the sum of the cross products of consecutive chords of 5 samples per curve (1 for
/// a two-point curve), plus the closing corner when the last sample returns to the
/// start point; `(0, 0, 1)` when the sum vanishes.
///
/// # Errors
///
/// `SM_ERR` for a single linear curve; those of the control point counts;
/// [`SolidError::Unported`] from the evaluations.
pub fn compute_crvs_normal(k: &Kernel, curves: &[Curve]) -> Result<Vec3> {
    // Initialize resulting normal
    let mut normal = Vec3::new(0.0, 0.0, 0.0);

    let mut pt2;
    let mut pt3 = Vec3::new(0.0, 0.0, 0.0);
    let mut vec1;
    let mut vec2 = Vec3::new(0.0, 0.0, 0.0);
    let mut start_pt = Vec3::new(0.0, 0.0, 0.0);
    let mut start_vector = Vec3::new(0.0, 0.0, 0.0);
    let mut total_points: u64 = 0;

    let num_curves = curves.len();
    for (ii, crv) in curves.iter().enumerate() {
        let domain = crv.natural_interval();

        if crv.is_linear(k, EFF_ZERO, false)?.linear && num_curves == 1 {
            return Err(SolidError::sm_err(OP_NORMAL));
        }

        let mut t = 0.0;
        pt3 = point_keep(k, crv, domain.evaluate(t), pt3)?;

        // Keep track of start point in case loop is closed
        if ii == 0 {
            start_pt = pt3;
        }

        total_points += 1;
        let mut count: u64 = 5;
        if crv.number_control_points()? == 2 {
            count = 1;
        }
        let t_incr = 1.0 / count as f64;

        for _jj in 0..count {
            pt2 = pt3;
            vec1 = vec2;

            pt3 = point_keep(k, crv, domain.evaluate(t + t_incr), pt3)?;

            total_points += 1;
            vec2 = pt3 - pt2;
            t += t_incr;
            if t > 1.0 {
                t = 1.0;
            }

            if total_points < 3 {
                // Keep track of start vector in case loop is closed
                start_vector = vec2;
                continue;
            }

            normal += vec1.cross(vec2);

            // If loop is closed and this is the last curve, add the last corner
            // Is tolerance required?
            if ii == num_curves - 1 && start_pt.approx_eq(pt3) {
                normal += vec2.cross(start_vector);
            }
        }
    }

    // (status ignored: a zero normal becomes (0, 0, 1))
    Ok(normal.unitize_or_z())
}

// ---- CrvsFixGaps ------------------------------------------------------------------------

/// Result of [`crvs_fix_gaps`].
#[derive(Debug, Clone)]
pub struct FixedGaps {
    /// The return value: the number of end points moved.
    pub fixed: u64,
    /// The curves chained by [`fix_chain_curves_list`], with the moved end points.
    pub chained: ChainedCurves,
}

/// Moves the last control point of the B-spline `curve` to `point`
/// (`SetControlPoint(SM_CP_NON_RATIONAL, n - 1, point, 1.0)`, status ignored).
fn move_end_point(curve: &mut Curve, point: Point3) -> Result<()> {
    let n = curve.number_control_points()?;
    let CurveKind::BSpline(c) = curve.kind_mut() else {
        return Err(SolidError::invalid_input(
            OP_GAPS,
            "end point of a curve that is not a B-spline",
        ));
    };
    ignored(c.set_control_point(ControlPointForm::NonRational, n.wrapping_sub(1), point, 1.0))?;
    Ok(())
}

/// Chains `curves` head to tail ([`fix_chain_curves_list`] with `tol`, deleting
/// duplicates) and closes every gap between consecutive curves of a chain, and
/// between the end and the start of a chain, that is larger than `SM_EFF_ZERO` but
/// smaller than `tol`, by moving the end point of the earlier curve onto the start
/// of the next (`SmCurve::CrvsFixGaps`).
///
/// # Errors
///
/// Those of [`fix_chain_curves_list`] and the control point counts; `InvalidInput`
/// when an end point to move belongs to a curve that is not a B-spline;
/// [`SolidError::Unported`] from the evaluations.
pub fn crvs_fix_gaps(k: &Kernel, curves: Vec<Curve>, tol: f64) -> Result<FixedGaps> {
    let mut fixed: u64 = 0;

    // head-tail them into chains - remove duplicate curves
    let mut chained = fix_chain_curves_list(k, curves.into_iter().map(Some).collect(), tol, true)?;
    let mut chain_start = chained.chain_starts.clone();
    chain_start.push(chained.curves.len());

    let missing = || SolidError::invalid_input(OP_GAPS, "chain index out of range");

    // between each curve within chain
    for j in 1..chain_start.len() {
        let low = *chain_start.get(j - 1).ok_or_else(missing)?;
        let hi = chain_start.get(j).ok_or_else(missing)?.wrapping_sub(1);
        if hi.wrapping_sub(low) < 1 {
            continue;
        }

        // every segment in the current chain
        for join_index in low..hi {
            let crv0 = chained.curves.get(join_index).ok_or_else(missing)?;
            let crv1 = chained.curves.get(join_index + 1).ok_or_else(missing)?;
            let domain0 = crv0.natural_interval();
            let domain1 = crv1.natural_interval();

            // start and end point between curves within loop/chain
            let end_pt = point_keep(k, crv0, domain0.max(), Vec3::UNDEF)?;
            let start_pt = point_keep(k, crv1, domain1.min(), Vec3::UNDEF)?;

            // gap between current segments
            let gap = start_pt.distance(end_pt);

            // when gap is not zero but smaller than dTol
            if gap > EFF_ZERO && gap < tol {
                // set the end point to the start of the next curve
                let crv0 = chained.curves.get_mut(join_index).ok_or_else(missing)?;
                move_end_point(crv0, start_pt)?;
                fixed += 1;
            }
        } // end iter every segment in the current chain
    } // end iter every distinct chain

    // check loop end to start gap and close if gap is small
    for j in 1..chain_start.len() {
        let low = *chain_start.get(j - 1).ok_or_else(missing)?;
        let hi = chain_start.get(j).ok_or_else(missing)?.wrapping_sub(1);
        let crv0 = chained.curves.get(low).ok_or_else(missing)?;
        let crv1 = chained.curves.get(hi).ok_or_else(missing)?;
        let domain0 = crv0.natural_interval();
        let domain1 = crv1.natural_interval();

        // start and end points of 'loop' for each chain
        let start_pt = point_keep(k, crv0, domain0.min(), Vec3::UNDEF)?;
        let end_pt = point_keep(k, crv1, domain1.max(), Vec3::UNDEF)?;

        // end to start gap for current chain
        let gap = start_pt.distance(end_pt);

        // when gap is small yet larger than zero
        if gap > EFF_ZERO && gap < tol {
            // Chain j-1 is closed
            // just as a check, replace end point with start point
            let crv1 = chained.curves.get_mut(hi).ok_or_else(missing)?;
            move_end_point(crv1, start_pt)?;
            fixed += 1;
        }
    } // end iter every distinct chain looking for closed curves

    // all done
    Ok(FixedGaps { fixed, chained })
}

// ---- FindParameterAtArcLength -----------------------------------------------------------

/// Result of [`Curve::find_parameter_at_arc_length`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArcLengthParameter {
    /// `rdEndParam`: the parameter the arc length away from the start.
    pub end_param: f64,
    /// `*pbResultPastEnd`: the length runs past the end of the curve (the end
    /// parameter is then the curve end, or where the iteration left the curve).
    pub past_end: bool,
    /// False when the Newton iteration stopped moving (upstream returns
    /// `SM_ERR_NOT_CONVERGING` with the last parameter).
    pub converged: bool,
}

impl ArcLengthParameter {
    /// The upstream status: `0` (`SM_SUCCESS`) or `SM_ERR_NOT_CONVERGING`.
    #[must_use]
    pub const fn status(&self) -> i32 {
        if self.converged {
            0
        } else {
            SM_ERR_NOT_CONVERGING
        }
    }
}

impl Curve {
    /// The parameter `arc_length` (negative: backwards) along the curve from
    /// `start_param` (`SmCurve::FindParameterAtArcLength`): whole knot spans are
    /// skipped by their lengths, then Newton's method on the arc length within the
    /// last span (damped when oscillating, at most 100 iterations) finds the
    /// parameter to about `0.01 * (|arc_length| + 1) * SM_EFF_ZERO_SQRT`.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for a start parameter more than `SM_EFF_ZERO` (relative) outside
    /// the knots, or below the first knot when stepping backwards (an out of bounds
    /// read upstream), or a curve without knots; those of the lengths and
    /// evaluations of the Newton iteration.
    pub fn find_parameter_at_arc_length(
        &self,
        k: &Kernel,
        start_param: f64,
        arc_length: f64,
    ) -> Result<ArcLengthParameter> {
        self.find_parameter_at_arc_length_with_zone_tol(k, start_param, arc_length, None)
    }

    /// Finds an arc-length parameter using the owning edge's resolved zone
    /// tolerance for each temporary length cache.
    pub(crate) fn find_parameter_at_arc_length_with_zone_tol(
        &self,
        k: &Kernel,
        start_param: f64,
        arc_length: f64,
        owner_zone_tol_3d: Option<f64>,
    ) -> Result<ArcLengthParameter> {
        let mut out = ArcLengthParameter {
            end_param: 0.0,
            past_end: false,
            converged: true,
        };
        let mut start_param = start_param;
        let mut arc_length = arc_length;

        let orig_ivl = self.natural_interval();
        let param_tol = (orig_ivl.length() + 1.0) * EFF_ZERO;

        // Set up the tolerance to produce about 6 decimal places of accuracy.
        let mut tol = (arc_length.abs() + 1.0) * EFF_ZERO_SQRT;

        // Why only 6?  Partly because Length() complains.  So make it tighter, but
        // not enough to get complaints from Length().
        tol *= 0.01;

        // This will be the absolute value of the required arc length.
        // It will decrease as we cover whole spans.
        let mut abs_len_needed = arc_length;
        let mut increasing = true;
        if arc_length < 0.0 {
            increasing = false;
            abs_len_needed = -arc_length;
        }

        // First, find the span containing dStartParam.
        let (knots, _) = knots_ignoring_status(self.knots_with_status(k, None))?;
        let num_knots = knots.len() as i64;
        let knot = |i: i64| -> Result<f64> {
            usize::try_from(i)
                .ok()
                .and_then(|i| knots.get(i))
                .copied()
                .ok_or(SolidError::invalid_input(OP_ARC, "knot index out of range"))
        };

        // Ruling this out will make later checks easier:
        if start_param < knot(0)? - param_tol || start_param > knot(num_knots - 1)? + param_tol {
            return Err(SolidError::invalid_input(
                OP_ARC,
                "start parameter outside the curve",
            ));
        }

        let mut ik: i64;

        // Callers usually start at one end or the other:
        if increasing && start_param <= knot(0)? {
            ik = 0;
        } else if !increasing && start_param >= knot(num_knots - 1)? {
            ik = num_knots - 2;
        } else {
            ik = 0;
            while knot(ik)? <= start_param && ik < num_knots - 1 {
                ik += 1;
            }
            ik -= 1;
        }

        // Move down spans until the length to dStartParam is within the span
        let span_step: i64 = if increasing { 1 } else { -1 };
        let mut span_len = 0.0;
        let mut interval = Extent1d::new();
        if increasing {
            set_min_max(&mut interval, start_param, knot(ik + 1)?);
        } else {
            set_min_max(&mut interval, knot(ik)?, start_param);
        }

        if let Some(l) = ignored(self.length_with_zone_tol(k, &interval, tol, owner_zone_tol_3d))? {
            span_len = l;
        }

        while span_len <= abs_len_needed {
            abs_len_needed -= span_len;

            // Check for serendipity
            if abs_len_needed < tol {
                out.end_param = if increasing { knot(ik + 1)? } else { knot(ik)? };
                return Ok(out);
            }

            ik += span_step; // move up or down to the next span

            if ik >= num_knots - 1 || ik < 0 {
                // We've fallen off the end of the curve without reaching len.
                // Return a good status, just a warning if asked for.
                out.end_param = if increasing {
                    knot(num_knots - 1)?
                } else {
                    knot(0)?
                };
                out.past_end = true;
                return Ok(out);
            }

            set_min_max(&mut interval, knot(ik)?, knot(ik + 1)?);
            if let Some(l) =
                ignored(self.length_with_zone_tol(k, &interval, tol, owner_zone_tol_3d))?
            {
                span_len = l;
            }

            // Reset dStartParam to the 'beginning' of the span:
            // bottom if increasing, top otherwise.
            start_param = if increasing { knot(ik)? } else { knot(ik + 1)? };
        }

        // How much remaining, within this span:
        arc_length = if increasing {
            abs_len_needed
        } else {
            -abs_len_needed
        };

        // Prorate the current interval to get a guess for dCurrT.
        let frac = interval.length() * (arc_length / span_len);
        let mut curr_t = start_param + frac;

        // and set dCurrLen according to dCurrT
        if increasing {
            set_min_max(&mut interval, start_param, curr_t);
        } else {
            set_min_max(&mut interval, curr_t, start_param);
        }
        let mut curr_len = self.length_with_zone_tol(k, &interval, tol, owner_zone_tol_3d)?;
        if !increasing {
            curr_len = -curr_len;
        }

        let mut prev_delta = BIG_DOUBLE;
        let mut off_end_count: u64 = 0;
        let max_iter: u64 = 100; // (really high, for a Newton iteration)

        for iter in 0..max_iter {
            // Newton step: function value to be zeroed is the difference between
            // the arc length to the current parameter value dCurrT and the
            // desired arc length.  Its deriv is just the magnitude of the
            // curve deriv at the current point.
            let fcn_val = curr_len - arc_length;

            if fcn_val.abs() < tol {
                break; // success.
            }

            // exact Tangent values
            let deriv: [Vec3; 2] = eval_n(k, self, curr_t, 1, true, false)?;
            let fcn_deriv = deriv[1].length();
            let mut delta_t = -fcn_val / fcn_deriv;

            // Check for no movement
            if delta_t.abs() < param_tol {
                out.converged = false;
                break;
            }

            // Check for no change in movement
            if (delta_t - prev_delta).abs() < param_tol && iter > 30 {
                out.converged = false;
                break;
            }

            // Apply the step
            let old_t = curr_t;
            curr_t += delta_t;

            // Check for falling off the end (an unsigned count upstream)
            if curr_t < orig_ivl.min() && delta_t < 0.0 {
                off_end_count = off_end_count.wrapping_sub(1);
            } else if curr_t > orig_ivl.max() && delta_t > 0.0 {
                off_end_count = off_end_count.wrapping_add(1);
            }

            // clamp before checking off-end
            curr_t = orig_ivl.clamp_value(curr_t);
            delta_t = curr_t - old_t;

            if off_end_count >= 3 {
                out.past_end = true;
                break; // but still return good status; just a warning
            }

            // Apply damping if oscillating.
            if delta_t.abs() > (0.9999 * prev_delta).abs() && delta_t * prev_delta < 0.0 {
                delta_t /= 2.0;
                curr_t = old_t + delta_t;
                curr_t = orig_ivl.clamp_value(curr_t);
            }
            prev_delta = delta_t;

            // Find the function value at current param: get arc length.
            if increasing {
                set_min_max(&mut interval, start_param, curr_t);
            } else {
                set_min_max(&mut interval, curr_t, start_param);
            }

            curr_len = self.length_with_zone_tol(k, &interval, tol, owner_zone_tol_3d)?;

            if !increasing {
                curr_len = -curr_len;
            }
        }

        out.end_param = curr_t;
        Ok(out)
    }
}

// ---- SmFindPCExtremaEFO -----------------------------------------------------------------

impl EvalFunction for FindPcExtremaEfo<'_, Curve> {
    /// `SmFindPCExtremaEFO::Evaluate`: `F = (C(t) - P) . C'(t)` and
    /// `F' = C'.C' + (C - P).C''` (the second order term limited to `-0.95 C'.C'`,
    /// negated for [`SolverOperation::Maximize`]), all scaled by the power of ten
    /// picked at the first call to keep `|C'|^2 <= 1e4`. Found when the gap or its
    /// squared angle from the normal plane is below the squared tolerance. A zero
    /// tangent is a solution on a degenerate curve and `SM_ERR` otherwise; a zero gap
    /// is `SM_ERR` when maximising.
    fn evaluate(&mut self, t: f64, out: &mut FunctionValue, _signal_errors: bool) -> Result<()> {
        let k = self.kernel;
        // init output
        out.f = 0.0;
        out.f_prime = 0.0;
        out.found_answer = false;

        // curve point
        let side = t < self.domain.mid();
        let mut pv: [Vec3; 3] = eval_n(k, self.curve, t, 2, side, true)?; // nonZeroTangent values

        // gap: sV = CurvePoint - TestPoint
        let mut sv = pv[0] - self.test_point;

        // Scale the problem down to a reasonable size (0-10000)
        if self.have_scale {
            let mut scale = 1.0;
            for _kk in 0..self.num_scale {
                scale *= 10.0;
            }
            if scale > 1.0 {
                sv = div(sv, scale);
                pv[0] = div(pv[0], scale);
                pv[1] = div(pv[1], scale);
                pv[2] = div(pv[2], scale);
            }
        } else {
            while pv[1].length_squared() > 1.0e4 {
                if pv[1].x.is_infinite() || pv[1].y.is_infinite() || pv[1].z.is_infinite() {
                    return Err(SolidError::sm_err(OP_PC));
                }
                // (not upstream) an infinite tangent never scales down: see
                // MAX_PC_SCALE_STEPS
                if self.num_scale >= MAX_PC_SCALE_STEPS {
                    return Err(SolidError::invalid_input(
                        OP_PC,
                        "tangent too large to scale (upstream loops forever)",
                    ));
                }
                self.num_scale += 1;
                sv = div(sv, 10.0);
                pv[0] = div(pv[0], 10.0);
                pv[1] = div(pv[1], 10.0);
                pv[2] = div(pv[2], 10.0);
            }
            self.have_scale = true;
        }

        // Check for a zero length tangent.
        // If it's because of a degenerate curve, then we're already at a solution.
        // If not degenerate, we're at a singularity and can't do much.
        let tan_dot_tan = pv[1].dot(pv[1]);
        if tan_dot_tan < EFF_ZERO_SQ {
            // degenerate curve?  Just return that we are perp, where we are.
            if self.curve.is_degenerate(k, EFF_ZERO, None)? {
                out.f = 0.0;
                out.found_answer = true;
                return Ok(());
            }
            return Err(SolidError::sm_err(OP_PC));
        } // end zero tangent check

        let v_dot_v = sv.dot(sv);
        let v_dot_tan = sv.dot(pv[1]);
        let mut v_dot_2nd = sv.dot(pv[2]);

        // Evaluate Func = (C(t) - P) * C'(t)
        out.f = v_dot_tan;

        // 1st Deriv = C'(t) * C'(t) + (C(t) - P) * C"(t), the second order term
        // limited so that it cannot change the sign of the step. [B52]
        let lim_2d_deriv = 0.95; // Smaller values can slow down convergence.

        if v_dot_2nd < -tan_dot_tan * lim_2d_deriv {
            v_dot_2nd = -tan_dot_tan * lim_2d_deriv;
        }

        out.f_prime = tan_dot_tan + v_dot_2nd;

        // Set sign of 1st Derive for maximize
        if self.solver_operation == SolverOperation::Maximize {
            // Let - sign of FofT rule
            out.f_prime = -out.f_prime;
        }

        // Done when GapSize goes to zero
        if v_dot_v < self.squared_tolerance {
            // for SM_SO_MINIMIZE = SUCCESS
            //     SM_SO_MAXIMIZE = FAILURE
            if self.solver_operation == SolverOperation::Maximize {
                return Err(SolidError::sm_err(OP_PC));
            }
            out.found_answer = true;
            return Ok(());
        }

        // Find gap/CurveTangent angle from perpendicular squared
        // Test this to see if we satisfy our tolerance
        let gap_tan_angle_from_perp_squared = (v_dot_tan * v_dot_tan) / (v_dot_v * tan_dot_tan);
        if gap_tan_angle_from_perp_squared < self.squared_tolerance {
            out.found_answer = true;
            return Ok(());
        }

        // all done
        Ok(())
    }
}

// ---- SmBestLineFitENFO ------------------------------------------------------------------

/// The (unfinished upstream) best line fit equations (`SmBestLineFitENFO`): at
/// `x = [s1, s2]`, the gap `C1(s1) - C2(s2)` dotted with the unit tangents of both
/// curves, switching to the tangent intersection equations
/// ([`CcTangentIntEnfo::static_evaluate`]) near a tangent intersection.
#[derive(Debug, Clone)]
pub struct BestLineFitEnfo<'a, C1: ?Sized, C2: ?Sized> {
    /// Caller context for virtual evaluations.
    pub kernel: &'a Kernel,

    /// `m_crCurve1`.
    pub curve1: &'a C1,
    /// `m_crCurve2`.
    pub curve2: &'a C2,
    /// `m_cpIntervals` (`None` for `NULL`).
    pub intervals: Option<ExtentNd>,
    /// `m_cpPeriodicities` (`None` for `NULL`).
    pub periodicities: Option<Vec<bool>>,
    /// `m_dSquaredTolerance` (not used).
    pub squared_tolerance: f64,
    /// `m_dSolutionSize`: the gap length of the last evaluation.
    pub solution_size: f64,
    /// `m_dScaledZero`: `SM_EFF_ZERO` scaled by the size of the last points.
    pub scaled_zero: f64,
}

impl<'a, C1: ?Sized, C2: ?Sized> BestLineFitEnfo<'a, C1, C2> {
    /// `SmBestLineFitENFO(crCurve1, crCurve2, dSquaredTolerance)`: no intervals or
    /// periodicities; the base class's solution size `0` and scaled zero `1`.
    #[must_use]
    pub const fn new(
        k: &'a Kernel,
        curve1: &'a C1,
        curve2: &'a C2,
        squared_tolerance: f64,
    ) -> Self {
        Self {
            kernel: k,
            curve1,
            curve2,
            intervals: None,
            periodicities: None,
            squared_tolerance,
            solution_size: 0.0,
            scaled_zero: 1.0,
        }
    }
}

impl<C1: CurveEvaluate + ?Sized, C2: CurveEvaluate + ?Sized> EvalNFunctions
    for BestLineFitEnfo<'_, C1, C2>
{
    fn solution_size(&self) -> f64 {
        self.solution_size
    }

    fn scaled_zero(&self) -> f64 {
        self.scaled_zero
    }

    /// `HasIntervals`: always true.
    fn has_intervals(&self) -> bool {
        true
    }

    /// `SetIntervals`: keeps (copies of) the solver's intervals and periodicities.
    fn set_intervals(
        &mut self,
        intervals: Option<&ExtentNd>,
        periodicities: Option<&[bool]>,
    ) -> Result<()> {
        self.intervals = intervals.cloned();
        self.periodicities = periodicities.map(<[bool]>::to_vec);
        Ok(())
    }

    /// `Evaluate`: `F = [gap . u1, gap . u2]` with the unit tangents `u` (limited
    /// second order terms in the Jacobian), or the tangent intersection equations
    /// when the gap is below `0.1` and the tangents are parallel within 0.01 degree;
    /// `rbFoundAnswer` is always false. `InvalidInput` for `x` or `f` shorter than 2.
    fn evaluate(
        &mut self,
        x: &[f64],
        f: &mut [f64],
        jacobian: Option<&mut Matrix>,
    ) -> Result<bool> {
        let k = self.kernel;
        let (Some(&x0), Some(&x1)) = (x.first(), x.get(1)) else {
            return Err(SolidError::invalid_input(
                OP_LINE_FIT,
                "need two parameters",
            ));
        };
        if f.len() < 2 {
            return Err(SolidError::invalid_input(
                OP_LINE_FIT,
                "need two function values",
            ));
        }

        // init output
        let mut found = false;

        // Evaluate both curves; need two derivatives.
        let pvv1: [Vec3; 3] = eval_n(k, self.curve1, x0, 2, true, true)?; // nonZeroTangent values
        let pvv2: [Vec3; 3] = eval_n(k, self.curve2, x1, 2, true, true)?; // nonZeroTangent values

        // get the gap - save the SolutionSize
        let gap_vec = pvv1[0] - pvv2[0];
        self.solution_size = gap_vec.length();
        self.scaled_zero =
            EFF_ZERO * (1.0 + max2(pvv1[0].max_dimension(), pvv2[0].max_dimension()));

        // Check for nearby tangent intersection cases - this allows
        // large near maxima conditions to 'fall off' the hill. [B98 B115]
        let switch_to_tan_fcns = true;

        let ang_lim_deg = 0.01; // Note: .01 works better than 2.0.
        let dist_lim_sq = 0.01;
        let is_tangent = switch_to_tan_fcns
            && gap_vec.length_squared() < dist_lim_sq
            && pvv1[1].is_parallel_to(pvv2[1], ang_lim_deg);

        // when near a tangent curve/curve intersection
        if is_tangent {
            // switch to the TangentIntersection Solver Objective Functions
            // (status ignored)
            if let Some(r) = ignored(CcTangentIntEnfo::static_evaluate(
                k,
                self.curve1,
                self.curve2,
                x,
                f,
                jacobian,
            ))? {
                found = r;
            }
        } else {
            // the usual case, not tangent.
            // Get the unitized derivatives.
            let (c1_t_unit, _) = unitize_tangent(pvv1, -gap_vec);
            let (c2_t_unit, _) = unitize_tangent(pvv2, gap_vec);

            // Function values:
            if let [f0, f1, ..] = f {
                *f0 = gap_vec.dot(c1_t_unit);
                *f1 = gap_vec.dot(c2_t_unit);
            }

            // Derivatives:
            if let Some(j) = jacobian {
                // Deriv of the unitized C_t  ( C_t / || C_t || )
                let c1_t_unit_t = pvv1[1].unitized_derivative(pvv1[2]);
                let c2_t_unit_t = pvv2[1].unitized_derivative(pvv2[2]);

                let term11 = pvv1[1].dot(c1_t_unit);
                let mut term12 = gap_vec.dot(c1_t_unit_t);
                let term21 = -pvv2[1].dot(c2_t_unit);
                let mut term22 = gap_vec.dot(c2_t_unit_t);

                if term12 < -term11 / 2.0 {
                    // note, dTerm11 > 0 always
                    term12 = -term11 / 2.0;
                }
                if term22 > -term21 / 2.0 {
                    // note, dTerm21 < 0 always
                    term22 = -term21 / 2.0;
                }
                j.set_at(0, 0, term11 + term12)?;
                j.set_at(0, 1, -pvv2[1].dot(c1_t_unit))?;
                j.set_at(1, 0, pvv1[1].dot(c2_t_unit))?;
                j.set_at(1, 1, term21 + term22)?;
            }
        } // end not near tangent branch

        Ok(found)
    }
}

// ---- LocalPointSolve --------------------------------------------------------------------

/// The optional inputs of [`Curve::local_point_solve`].
#[derive(Debug, Clone, Copy, Default)]
pub struct PointSolveOptions<'a> {
    /// `cpdOptDistanceTolerance`: largest ray / curve gap for
    /// [`SolverOperation::Rayfire`] and [`SolverOperation::Signed3dDirectedMinimize`]
    /// (required there).
    pub distance_tolerance: Option<f64>,
    /// `cpdOptTargetDistance`: the distance of [`SolverOperation::AtDistance`]
    /// (required there).
    pub target_distance: Option<f64>,
    /// `cpOptVectors`: the ray direction (first entry) of
    /// [`SolverOperation::Rayfire`] and [`SolverOperation::Signed3dDirectedMinimize`]
    /// (required there).
    pub vectors: Option<&'a [Vec3]>,
}

impl Curve {
    /// The parameter in `interval` near `guess` solving `op` for `test_point`
    /// (`SmCurve::LocalPointSolve`): the closest / farthest point or a normal foot
    /// ([`SolverOperation::Minimize`], `Maximize`, `Normalize`, `Intersect`) by
    /// Newton's method on [`FindPcExtremaEfo`] (a solution clamped to the interval
    /// counts, except when normalising), a point at the target distance
    /// ([`SolverOperation::AtDistance`], by [`crate::curve::curve::p4::CpDistAtEnfo`]),
    /// or the curve point closest to a ray ([`SolverOperation::Rayfire`],
    /// `Signed3dDirectedMinimize`, through `LocalPropertyAnalysis`). Returns whether
    /// an answer was found and the solution (type, value and first parameter; the
    /// whole `LocalPropertyAnalysis` solution for a ray).
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` for a ray without distance tolerance or vectors;
    /// `InvalidInput` for an empty vector array or a missing target distance;
    /// `SM_ERR` when the at-distance solver fails; those of the evaluations and of
    /// `LocalPropertyAnalysis`. A failing Newton iteration of the other operations is no
    /// answer, as upstream.
    pub fn local_point_solve<O: Copy>(
        &self,
        k: &Kernel,
        interval: &Extent1d,
        op: SolverOperation,
        test_point: Point3,
        opts: &PointSolveOptions<'_>,
        guess: f64,
    ) -> Result<(bool, Solution<O>)> {
        let mut solution = Solution::new();

        // Final solution will be within the interval.
        let guess = interval.clamp_value(guess);

        // for RAYFIRE and SIGNED_MINIMIZE
        if op == SolverOperation::Rayfire || op == SolverOperation::Signed3dDirectedMinimize {
            // check inputs
            let dist_tol = opts
                .distance_tolerance
                .ok_or(SolidError::null_pointer(OP_POINT))?;
            let vectors = opts.vectors.ok_or(SolidError::null_pointer(OP_POINT))?;
            let vec0 = *vectors
                .first()
                .ok_or(SolidError::invalid_input(OP_POINT, "empty option vectors"))?;

            // locals
            let mut sol = Solution::new();
            let vecs = [test_point, vec0];

            // Find point on curve with closest projection to [testPoint, OptVec[0]] plane
            let found = local_property_analysis(
                k,
                self,
                interval,
                CurvePropertyType::ProjectedPointMinimize,
                guess,
                Some(dist_tol),
                &vecs,
                &mut sol,
            )?;

            // quit when no answer was found
            if !found {
                return Ok((false, solution));
            }

            // get Curve solution point
            let pnt = self.evaluate_point(k, sol.start.param(0)?)?;

            // Project CurvePoint to [testPoint, OptVec[0]] line
            let param = line_closest_point(vecs[0], vecs[1], pnt);
            let pnt_on_vec = vecs[0] + param * vecs[1];

            // when Ray/Curve gap is less than optDistTol
            if pnt_on_vec.distance(pnt) < dist_tol {
                // output the solution
                sol.start.solution_value = param;
                return Ok((true, sol));
            }

            return Ok((false, solution));
        } // end SolverOperation == RAYFIRE or SIGNED_DIRECTED_MINIMIZE check

        if op == SolverOperation::AtDistance {
            let target = opts.target_distance.ok_or(SolidError::invalid_input(
                OP_POINT,
                "missing target distance",
            ))?;
            let mut intervals = ExtentNd::new(4);
            let unbounded = Extent1d::from_raw(-BIG_DOUBLE, BIG_DOUBLE);
            for (i, ivl) in [*interval, unbounded, unbounded, unbounded]
                .into_iter()
                .enumerate()
            {
                if let Some(slot) = intervals.get_mut(i) {
                    *slot = ivl;
                }
            }
            // Otherwise it will stop at ends.
            let periodicities = [false; 4];

            let mut eval = crate::curve::curve::p4::CpDistAtEnfo::new(
                k,
                self,
                test_point,
                target,      // Desired GapSizeSq
                EFF_ZERO_SQ, // currently not used
            );
            let mut ls = LocalSolveNd::new(&mut eval, Some(&intervals), Some(&periodicities));

            let guess_t = [guess, 0.1, 0.1, 0.1];
            let r = match ls.solve_it(&guess_t, EFF_ZERO) {
                Ok(r) => r,
                Err(e) if e.is_unported() => return Err(e),
                // some sort of error in solver
                Err(_) => return Err(SolidError::sm_err(OP_POINT)),
            };
            if r.found {
                let t0 = r.x.first().copied().unwrap_or(0.0);
                solution.solution_type = SolutionType::SingleValue;
                set_param(&mut solution.start, 0, t0)?;
                let eval_pnt = self.evaluate_point(k, t0)?;
                solution.start.solution_value = eval_pnt.distance(test_point);
                return Ok((true, solution));
            }
            return Ok((false, solution));
        } // end eSolverOperation == SM_SO_AT_DISTANCE check

        // For all other operation cases - Use Newton-Raphson to solve
        let mut efo = FindPcExtremaEfo::new(
            k,
            self,
            test_point,
            *interval,
            EFF_ZERO_SQ, // NR iter stops when gapSize or Angle from Perp gets smaller than SM_EFF_ZERO
            op,
        );
        let mut ls = LocalSolve1d::with_interval(*interval, false);
        // acceptable accuracy when Func(t) < tolerance
        let r = match ls.solve_it(&mut efo, guess, 1.0e-8) {
            Ok(r) => r,
            Err(e) if e.is_unported() => return Err(e),
            // some solver error - return without answer
            Err(_) => return Ok((false, solution)),
        };

        // when SolveIt failed to converge
        if !r.found {
            // Don't report failures to converge due to hitting bounds
            // (except for operation = SM_SO_NORMALIZE) because these
            // failures are really successful bounded value solutions
            // reported in dFoundT.
            if ls.termination_reason() != TerminationReason::OutOfBounds
                || op == SolverOperation::Normalize
            {
                // when normalizing must be within bounds
                return Ok((false, solution));
            }
        } // end no solution branch

        // else when SolveIt did converge or found a bounded value
        //   let solutionValue = dist(TestPoint,SolutionPoint)
        let found_pnt = point_keep(k, self, r.t, Vec3::UNDEF)?;

        solution.solution_type = SolutionType::SingleValue;
        solution.start.solution_value = found_pnt.distance(test_point);
        set_param(&mut solution.start, 0, r.t)?;

        // all done
        Ok((true, solution))
    }
}

// ---- LocalCurveSolve --------------------------------------------------------------------

/// The local curve/curve solve of every operation but
/// [`SolverOperation::Signed3dDirectedMinimize`] (see [`local_curve_solve`]).
fn curve_solve_newton<O: Copy, C1, C2>(
    k: &Kernel,
    this: &C1,
    interval: &Extent1d,
    other: &C2,
    other_interval: &Extent1d,
    op: SolverOperation,
    dist_tol_3d: f64,
    target_distance: Option<f64>,
    opt_vectors: Option<&[Vec3]>,
    guesses: [f64; 2],
    objects: [Option<O>; 2],
) -> Result<(bool, Solution<O>)>
where
    C1: CurveEvaluate + ?Sized,
    C2: CurveEvaluate + ?Sized,
{
    let mut solution = Solution::new();

    let mut intervals = ExtentNd::new(2);
    for (i, ivl) in [*interval, *other_interval].into_iter().enumerate() {
        if let Some(slot) = intervals.get_mut(i) {
            *slot = ivl;
        }
    }

    // Local solvers should not jump across seams:
    let periodicities = [false, false];

    let mut at_dist_sq = 0.0;
    if let Some(d) = target_distance {
        at_dist_sq = d * d;
    }

    let mut eval = FindCcNormalEnfo::new(
        k,
        this,
        other,
        EFF_ZERO * EFF_ZERO,
        at_dist_sq,
        op,
        opt_vectors,
    );
    let mut ls = LocalSolveNd::new(&mut eval, Some(&intervals), Some(&periodicities));

    ls.set_boundary_handler(BoundaryHandler::TotalBoundaryHits, 8);

    if op == SolverOperation::AtDistance {
        ls.set_maximum_iterations(300); // This one tends to have slower convergance
    }
    let r = ls.solve_it(&guesses, dist_tol_3d)?;
    let sol0 = r.x.first().copied().unwrap_or(0.0);
    let sol1 = r.x.get(1).copied().unwrap_or(0.0);

    // Fill in rSolution in any event.  It might be a better solution than
    // the input guesses, and the caller can check for that if desired.
    solution.solution_type = SolutionType::SingleValue;
    solution.num_objects = 2;
    solution.objects[0] = objects[0];
    solution.objects[1] = objects[1];
    let pnt1 = this.evaluate_point(k, sol0)?;
    let pnt2 = other.evaluate_point(k, sol1)?;
    solution.start.solution_value = pnt1.distance(pnt2);

    // Note that for projected operations we only have one vector and that
    // is the normal.  For directed operations the direction vector is also
    // available.
    if matches!(
        op,
        SolverOperation::ProjectedMinimize
            | SolverOperation::ProjectedMaximize
            | SolverOperation::SignedDirectedMinimize
            | SolverOperation::DirectedMinimize
            | SolverOperation::DirectedMaximize
            | SolverOperation::SignedPivotMinimize
    ) {
        let vec = |i: usize| -> Result<Vec3> {
            opt_vectors
                .and_then(|v| v.get(i))
                .copied()
                .ok_or(SolidError::invalid_input(OP_CURVE, "missing option vector"))
        };
        let normal = vec(0)?;
        let mut direction = vec(0)?;
        if matches!(
            op,
            SolverOperation::SignedDirectedMinimize
                | SolverOperation::DirectedMinimize
                | SolverOperation::DirectedMaximize
        ) {
            direction = vec(1)?;
        }

        if op == SolverOperation::SignedPivotMinimize {
            direction = vec(2)?;
            let sol_pt = other.evaluate_point(k, sol1)?;
            solution.start.solution_value =
                proj_signed_angle(pnt1, sol_pt, vec(1)?, normal, direction)?;
        } else {
            solution.start.solution_value =
                proj_dist(k, other, sol1, pnt1, normal, true, direction, op)?;
        }
    }

    let mut found = true;

    // Solution value in this case is not the distance.
    if op == SolverOperation::SurfaceProjectedIntersect {
        let mut obj_fcns = [0.0; 2];
        // (status ignored; Not used by Evaluate)
        ignored(ls.evaluator().evaluate(&r.x, &mut obj_fcns, None))?;
        solution.start.solution_value = obj_fcns[0].abs() + obj_fcns[1].abs();

        // For a legitimate crossing, this should be extremely precise.
        // Check for spurious solutions caused by hitting cache boundaries.
        if solution.start.solution_value > 1000.0 * EFF_ZERO {
            if let Some(ivls) = ls.intervals() {
                let on = |i: usize, v: f64| {
                    ivls.get(i)
                        .is_some_and(|ivl| ivl.is_value_on_boundary(v, EFF_ZERO).0)
                };
                if on(0, sol0) {
                    found = false;
                }
                if on(1, sol1) {
                    found = false;
                }
            }

            if !found {
                return Ok((false, solution));
            }
        }
    }

    set_param(&mut solution.start, 0, sol0)?;
    set_param(&mut solution.start, 1, sol1)?;

    if !r.found {
        // If we terminate because of hitting bounds that
        // is ok in the closest point problem.  We have
        // returned the bound hit in dFoundT.
        // When normalizing must be within bounds.
        // Also for tangency. [Unit tests]
        if ls.termination_reason() != TerminationReason::OutOfBounds
            || matches!(
                op,
                SolverOperation::Normalize
                    | SolverOperation::AtDistance
                    | SolverOperation::ProjectedTangency
                    | SolverOperation::SurfaceProjectedIntersect
            )
        {
            return Ok((false, solution));
        }
    }

    Ok((found, solution))
}

/// The local solution of `op` between `this` over `interval` and `other` over
/// `other_interval` from the parameters `guesses` (`SmCurve::LocalCurveSolve`):
/// Newton's method on the curve/curve normal equations (`SmFindCCNormalENFO`). The
/// solution (filled in even when none is accepted) has both parameters, the objects
/// `objects` and the distance (the projected distance or signed pivot angle for the
/// projected, directed and pivot operations; `|F0| + |F1|` for
/// [`SolverOperation::SurfaceProjectedIntersect`], rejected when large on an
/// interval bound). A solution stopped out of bounds counts except when normalising,
/// at distance, for tangency and the surface projected intersection.
///
/// # Errors
///
/// `InvalidInput` for [`SolverOperation::Signed3dDirectedMinimize`] (see
/// [`Curve::local_curve_solve`]) or a missing option vector; those of the solver
/// and evaluations.
pub fn local_curve_solve<O: Copy, C1, C2>(
    k: &Kernel,
    this: &C1,
    interval: &Extent1d,
    other: &C2,
    other_interval: &Extent1d,
    op: SolverOperation,
    dist_tol_3d: f64,
    target_distance: Option<f64>,
    opt_vectors: Option<&[Vec3]>,
    guesses: [f64; 2],
    objects: [Option<O>; 2],
) -> Result<(bool, Solution<O>)>
where
    C1: CurveEvaluate + ?Sized,
    C2: CurveEvaluate + ?Sized,
{
    if op == SolverOperation::Signed3dDirectedMinimize {
        return Err(SolidError::invalid_input(
            OP_CURVE,
            "the 3D signed directed minimum needs Curve operands",
        ));
    }
    curve_solve_newton(
        k,
        this,
        interval,
        other,
        other_interval,
        op,
        dist_tol_3d,
        target_distance,
        opt_vectors,
        guesses,
        objects,
    )
}

impl Curve {
    /// [`local_curve_solve`] of this curve with `other`, including
    /// [`SolverOperation::Signed3dDirectedMinimize`]: the curves projected along the
    /// first option vector are intersected ([`local_curve_intersect`]) and the
    /// signed 3D distance between the intersection points is the solution value
    /// (signed by the first option vector); when they do not intersect, nearly
    /// coplanar curves are solved as [`SolverOperation::SignedDirectedMinimize`] and
    /// accepted when the gap is within 1 degree of the vector.
    ///
    /// # Errors
    ///
    /// As [`local_curve_solve`]; `InvalidInput` for the 3D signed operation without
    /// option vectors.
    pub fn local_curve_solve<O: Copy>(
        &self,
        k: &Kernel,
        interval: &Extent1d,
        other: &Self,
        other_interval: &Extent1d,
        op: SolverOperation,
        dist_tol_3d: f64,
        target_distance: Option<f64>,
        opt_vectors: Option<&[Vec3]>,
        guesses: [f64; 2],
        objects: [Option<O>; 2],
    ) -> Result<(bool, Solution<O>)> {
        if op != SolverOperation::Signed3dDirectedMinimize {
            return curve_solve_newton(
                k,
                self,
                interval,
                other,
                other_interval,
                op,
                dist_tol_3d,
                target_distance,
                opt_vectors,
                guesses,
                objects,
            );
        }

        // Do intersection and then compute 3D distance
        let mut solution = Solution::new();
        let vectors = opt_vectors.ok_or(SolidError::invalid_input(
            OP_CURVE,
            "missing option vectors",
        ))?;
        let vec0 = *vectors
            .first()
            .ok_or(SolidError::invalid_input(OP_CURVE, "missing option vector"))?;
        let origin = Vec3::new(0.0, 0.0, 0.0);
        let aux_data = Vec3::new(0.0, 0.0, 0.0);
        let proj1 = Self::from(ProjectedCurve::new(
            self.clone(),
            Some(origin),
            Some(vec0),
            Some(aux_data),
            ProjectionType::Parallel,
            OwnsCurve::No,
        )?);
        let proj2 = Self::from(ProjectedCurve::new(
            other.clone(),
            Some(origin),
            Some(vec0),
            Some(aux_data),
            ProjectionType::Parallel,
            OwnsCurve::No,
        )?);

        // find projected curve intersections
        let isect = local_curve_intersect(
            k,
            &proj1,
            interval,
            &proj2,
            other_interval,
            dist_tol_3d,
            guesses[0],
            guesses[1],
        )?;
        let Some(isect) = isect else {
            // Check for coplanarity case and use separate solver

            // curve values at guess parameters
            let pv1: [Vec3; 2] = eval_n(k, self, guesses[0], 1, true, true)?; // nonZeroTangent values
            let pv2: [Vec3; 2] = eval_n(k, other, guesses[1], 1, true, true)?; // nonZeroTangent values

            let norm = pv1[1].cross(vec0);
            if norm.length_squared() < EFF_ZERO_SQ {
                return Ok((false, solution));
            }
            let norm = norm.unitize()?;
            let d = -norm.dot(pv1[0]);
            let dist = norm.dot(pv2[0]) + d;
            let tol = EFF_ZERO_SQRT * (1.0 + pv2[0].max_dimension());
            if dist.abs() > tol {
                return Ok((false, solution));
            }
            if norm.dot(pv2[1]) > EFF_ZERO_SQRT {
                return Ok((false, solution));
            }
            // If make it here then we are likely to have a coplanar curves case
            // use a different solver to fix the problem.
            let vecs = [norm, vec0];
            let (found, sol) = self.local_curve_solve(
                k,
                interval,
                other,
                other_interval,
                SolverOperation::SignedDirectedMinimize,
                dist_tol_3d,
                target_distance,
                Some(&vecs),
                guesses,
                objects,
            )?;
            if !found {
                return Ok((false, solution));
            }

            // Make sure resulting points lie parallel to the vector
            let p1 = self.evaluate_point(k, sol.start.param(0)?)?;
            let p2 = other.evaluate_point(k, sol.start.param(1)?)?;
            let vec = p2 - p1;
            if vec.length_squared() < EFF_ZERO_SQ {
                return Ok((false, solution));
            }
            let mut angle = vec.angle_between(vec0)?;
            if angle > PI / 2.0 {
                angle = PI - angle;
            }
            if angle > 1.0 * PI / 180.0 {
                return Ok((false, solution));
            }

            solution = sol;
            solution.num_objects = 2;
            solution.objects[0] = objects[0];
            solution.objects[1] = objects[1];
            return Ok((true, solution));
        };

        let p1 = self.evaluate_point(k, isect.my_param)?;
        let p2 = other.evaluate_point(k, isect.other_param)?;
        let vec = p2 - p1;
        let mut dist = vec.length();
        if vec.dot(vec0) < 0.0 {
            dist = -dist;
        }
        solution.solution_type = SolutionType::SingleValue;
        solution.num_variables = 2;
        solution.start.solution_value = dist;
        set_param(&mut solution.start, 0, isect.my_param)?;
        set_param(&mut solution.start, 1, isect.other_param)?;
        solution.num_objects = 2;
        solution.objects[0] = objects[0];
        solution.objects[1] = objects[1];
        Ok((true, solution))
    }
}

// ---- LocalCurveIntersect ----------------------------------------------------------------

/// An intersection found by [`local_curve_intersect`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CurveIntersection {
    /// `rdMyParameter`.
    pub my_param: f64,
    /// `rdOtherParameter`.
    pub other_param: f64,
    /// `rdDeviation`: the distance between the two points.
    pub deviation: f64,
}

/// Distance between `this` at `t1` and `other` at `t2` (positions of nonZeroTangent
/// evaluations) and both evaluations.
fn deviation_at<C1, C2>(
    k: &Kernel,
    this: &C1,
    other: &C2,
    t1: f64,
    t2: f64,
) -> Result<(f64, [Vec3; 2], [Vec3; 2])>
where
    C1: CurveEvaluate + ?Sized,
    C2: CurveEvaluate + ?Sized,
{
    let pv1: [Vec3; 2] = eval_n(k, this, t1, 1, true, true)?; // nonZeroTangent values
    let pv2: [Vec3; 2] = eval_n(k, other, t2, 1, true, true)?; // nonZeroTangent values
    Ok((pv1[0].distance(pv2[0]), pv1, pv2))
}

/// The intersection of `this` over `interval` and `other` over `other_interval`
/// near the guesses (`SmCurve::LocalCurveIntersect`): Newton's method on the
/// curve/curve intersection equations (`SmCCIntersectENFO`, accuracy
/// `SM_EFF_ZERO / 10`, acceptable `2 dist_tol`); a result within `dist_tol` is
/// polished by a second solve from it, then by the closest approach of the curves
/// ([`local_curve_solve`], [`SolverOperation::Minimize`]) and, for nearly parallel
/// tangents, by the tangent intersection equations. `None` when no intersection is
/// found near the guesses.
///
/// # Errors
///
/// Those of the solvers and evaluations.
pub fn local_curve_intersect<C1, C2>(
    k: &Kernel,
    this: &C1,
    interval: &Extent1d,
    other: &C2,
    other_interval: &Extent1d,
    dist_tol: f64,
    my_guess: f64,
    other_guess: f64,
) -> Result<Option<CurveIntersection>>
where
    C1: CurveEvaluate + ?Sized,
    C2: CurveEvaluate + ?Sized,
{
    // locals
    let mut intervals = ExtentNd::new(2);
    for (i, ivl) in [*interval, *other_interval].into_iter().enumerate() {
        if let Some(slot) = intervals.get_mut(i) {
            *slot = ivl;
        }
    }
    // Do not utilize periodicities in local solver -- local solvers
    // should not jump across seams.
    let periodicities = [false, false];

    let guess_t = [my_guess, other_guess];

    // create a newton raphson solver object containing a curve/curve evaluator.
    let mut eval = CcIntersectEnfo::new(k, this, other, EFF_ZERO * EFF_ZERO / 10000.0);
    let mut ls = LocalSolveNd::new(&mut eval, Some(&intervals), Some(&periodicities));

    // Pick up close solutions and then test them
    ls.set_desired_accuracy(EFF_ZERO / 10.0);
    ls.set_boundary_handler(BoundaryHandler::HitsAllBoundaries, 3);

    // look for a newton raphson solution
    let r = ls.solve_it(&guess_t, dist_tol * 2.0)?;
    let mut sol = [
        r.x.first().copied().unwrap_or(0.0),
        r.x.get(1).copied().unwrap_or(0.0),
    ];

    // when a solution was found
    if !r.found {
        // arrive here when no answer is found
        return Ok(None);
    }

    // get solution deviation
    let (mut deviation, mut pv1, mut pv2) = deviation_at(k, this, other, sol[0], sol[1])?;

    // Be happy with a result within the really tight tolerance:
    if deviation < EFF_ZERO_SQ {
        return Ok(Some(CurveIntersection {
            my_param: sol[0],
            other_param: sol[1],
            deviation,
        }));
    }

    // when deviation is within the loose tolerance
    if deviation >= dist_tol {
        return Ok(None);
    }

    // If it's not real tight,
    // see if we can improve it by starting again at the solution.
    // [my_shell_demo iter: 2]
    if deviation > EFF_ZERO {
        // save current solutions
        let old_sol = sol;
        let old_guess = guess_t;

        // However, if the solution didn't move,
        // no sense trying again.
        if !are_same_to_tol(old_sol[0], old_guess[0], EFF_ZERO_SQRT)
            || !are_same_to_tol(old_sol[1], old_guess[1], EFF_ZERO_SQRT)
        {
            // create a second NewtonRaphson Solver
            let mut eval2 = CcIntersectEnfo::new(k, this, other, EFF_ZERO * EFF_ZERO / 10000.0);
            let mut ls2 = LocalSolveNd::new(&mut eval2, Some(&intervals), Some(&periodicities));

            // Pick up close solutions and then test them
            ls2.set_desired_accuracy(EFF_ZERO / 10.0);
            ls2.set_boundary_handler(BoundaryHandler::HitsAllBoundaries, 3);

            // use current solutions as next guesses (the old guess is restored
            // afterwards upstream)
            let guess2 = sol;

            // solve starting at previous solution
            let r2 = ls2.solve_it(&guess2, dist_tol * 2.0)?;
            sol = [
                r2.x.first().copied().unwrap_or(0.0),
                r2.x.get(1).copied().unwrap_or(0.0),
            ];

            // when a better solution was found - store it
            if r2.found {
                // get new deviation
                let (new_deviation, npv1, npv2) = deviation_at(k, this, other, sol[0], sol[1])?;
                pv1 = npv1;
                pv2 = npv2;

                // when new deviation is better than last, store it
                if new_deviation < deviation {
                    deviation = new_deviation;
                } else {
                    // restore old solution
                    sol = old_sol;
                }
            } else {
                // didn't find a solution, restore old one
                sol = old_sol;
            }
        } // end first solution uv not the same as guess uv branch
    } // end deviation larger than zero check

    // If we did not converge to an exact intersection (with nonZero length intervals)
    //  then try finding minimum distance between spans using intersection as start point.
    if deviation > EFF_ZERO
        && !are_same(interval.min(), interval.max())
        && !are_same(other_interval.min(), other_interval.max())
    {
        // Find minimum distance between spans as best possible answer
        let (found, s) = local_curve_solve::<(), C1, C2>(
            k,
            this,
            interval,
            other,
            other_interval,
            SolverOperation::Minimize,
            dist_tol,
            None,
            None,
            sol,
            [None, None],
        )?;

        // when a better solution was found
        if found && s.start.solution_value < deviation {
            deviation = s.start.solution_value;
            sol = [s.start.param(0)?, s.start.param(1)?];
        } // end found a better solution check
    } // end did not converge to an exact solution check

    // If the curves are nearly parallel,
    // an SmCCTangentIntENFO can often do better.
    if deviation > EFF_ZERO && pv1[1].is_parallel_to(pv2[1], 2.0) {
        let mut eval2 = CcTangentIntEnfo::new(k, this, other, EFF_ZERO * EFF_ZERO);
        let periodicities2 = [
            is_periodic(k, this, interval)?,
            is_periodic(k, other, other_interval)?,
        ];

        let guess_t2 = sol;

        let mut ls2 = LocalSolveNd::new(&mut eval2, Some(&intervals), Some(&periodicities2));

        // Pick up close solutions and then test them
        ls2.set_desired_accuracy(EFF_ZERO_SQ); // very tight
                                               // See if answer is better than one we found
        let r2 = ls2.solve_it(&guess_t2, dist_tol * 2.0)?;
        let sol2 = [
            r2.x.first().copied().unwrap_or(0.0),
            r2.x.get(1).copied().unwrap_or(0.0),
        ];
        let (new_deviation, _, _) = deviation_at(k, this, other, sol2[0], sol2[1])?;
        if new_deviation < deviation {
            return Ok(Some(CurveIntersection {
                my_param: sol2[0],
                other_param: sol2[1],
                deviation: new_deviation,
            }));
        }
    } // end tangent processor check

    // store the output and quit
    Ok(Some(CurveIntersection {
        my_param: sol[0],
        other_param: sol[1],
        deviation,
    }))
}

// ---- SmCurvePropertyGS::LocalSolve ------------------------------------------------------

impl<'t, O: SolutionObject + 't> CurvePropertyGs<'t, O> {
    /// `SmCurvePropertyGS::LocalSolve` on the curve leaf `branch[0]` (always returns
    /// `rbNeedsMoreSubdivision` false): for the angle properties the span is skipped
    /// when no pseudo box corner beats the best answer; the span interval is limited
    /// to the solver's; for the angle and plane intersection properties a span end
    /// at a less than G1 continuity is a candidate solution (a plane intersection
    /// span lying in the plane is a range solution); then `LocalPropertyAnalysis`
    /// from the span middle (and its ends and quarters for the other properties) adds
    /// the solutions found, a span with three exact guesses becoming a range.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` for a node without data, `InvalidInput` for a missing
    /// branch node, curve, option vector or value or a node that is not a curve
    /// leaf; status `1` for a projected pseudo box corner at the origin of the angle
    /// properties; those of the evaluations, `LocalPropertyAnalysis` and
    /// `sm_FindAdditionalCpGuess`.
    pub fn local_solve_branch(&mut self, k: &Kernel, branch: &Branch) -> Result<bool> {
        // init output
        let needs_more_subdivision = false;

        // locals
        let curve_node_id = branch[0].ok_or(SolidError::invalid_input(OP_GS, "null curve node"))?;
        let tree = self.base.tree(0)?;
        let curve_node = tree.node(curve_node_id)?;
        if matches!(curve_node.data, NodeData::None) {
            return Err(SolidError::null_pointer(OP_GS));
        }
        let bez_span = *tree.curve_span(curve_node)?;
        let owner = tree.owner_object()?;

        let curve = self
            .curve
            .ok_or(SolidError::invalid_input(OP_GS, "solver without curve"))?;
        let mut ivl = *bez_span.interval();
        let prop = self.property;

        if matches!(
            prop,
            CurvePropertyType::MinimizeAngleToPlane
                | CurvePropertyType::MinimizeDirectedAngle
                | CurvePropertyType::MinimizeCcwAngle
        ) {
            // Check the minimum angle made by the pseudo box to see if
            // we can eliminate having to process this node.
            let corners = bez_span.pseudo_box().corners();
            let (o0, o1, o2) = (self.opt(0)?, self.opt(1)?, self.opt(2)?);
            let mut min_cos_theta = BIG_DOUBLE;
            for c in corners {
                let mut vec = c - o0;
                vec = o1.cross(vec).cross(o1); // Project to view plane
                if vec.length_squared() < EFF_ZERO {
                    // (`return TRUE` from an SmStatus function)
                    return Err(SolidError::Upstream {
                        operation: OP_GS,
                        code: 1,
                        entity: None,
                    });
                }
                let vec = vec.unitize()?;
                let cos_theta = property_angle(prop, o1, o2, vec)?;

                if cos_theta < min_cos_theta {
                    min_cos_theta = cos_theta;
                }
            }

            // If we have no angles which are small enough than just return
            if (min_cos_theta * min_cos_theta.abs()) + EFF_ZERO_SQRT
                > self.base.best_answer_so_far_sq
            {
                return Ok(needs_more_subdivision);
            }
        } // end angle cases

        // Do interval tests and reject if disjoint or adjust
        // if sIvl is not completely inside of m_crInterval
        let mut conts = bez_span.aux.conts;
        if ivl.are_disjoint(&self.interval, 0.0) {
            return Ok(needs_more_subdivision);
        }

        if ivl.min() <= self.interval.min() {
            conts[0] = ContinuityType::Discontinuous;
        }

        if ivl.max() >= self.interval.max() {
            conts[1] = ContinuityType::Discontinuous;
        }

        if !ivl.is_contained_by(&self.interval, EFF_ZERO) {
            ivl = ivl.intersect(&self.interval)?;
            // Check for zero length intervals
            if are_same(ivl.max(), ivl.min()) {
                return Ok(needs_more_subdivision);
            }
        }

        // locals
        let mut sol: Solution<O> = Solution::new();

        // Process discontinuity points when appropriate
        if matches!(
            prop,
            CurvePropertyType::MinimizeAngleToPlane
                | CurvePropertyType::MinimizeDirectedAngle
                | CurvePropertyType::MinimizeCcwAngle
                | CurvePropertyType::PlaneIntersection
        ) {
            for (jj, cont) in conts.iter().enumerate() {
                let mut param = if jj == 0 { ivl.min() } else { ivl.max() };
                if (*cont as i32) < (ContinuityType::G1 as i32) {
                    let pnt = curve.evaluate_point(k, param)?;

                    // for minimize angle cases
                    if prop != CurvePropertyType::PlaneIntersection {
                        let (o0, o1, o2) = (self.opt(0)?, self.opt(1)?, self.opt(2)?);
                        let mut vec = pnt - o0;
                        vec = o1.cross(vec).cross(o1);

                        // If the point is at the origin of rotation just ignore it
                        if vec.length_squared() > EFF_ZERO_SQ {
                            let vec = vec.unitize()?;
                            let cos_theta = property_angle(prop, o1, o2, vec)?;

                            // save solution
                            sol.solution_type = SolutionType::SingleValue;
                            sol.nodes[0] = branch[0]; // Point node
                            sol.nodes[1] = branch[1]; // Curve node
                            sol.num_objects = 1;
                            sol.objects[0] = owner;
                            sol.num_variables = 1;
                            set_param(&mut sol.start, 0, param)?;
                            sol.start.solution_value = cos_theta;
                            self.add_sorted_solution(&sol, SortKey::ByValueWithParameter, true)?;
                        }
                    } // end minimize angle cases

                    if prop == CurvePropertyType::PlaneIntersection {
                        let abc = self.opt(0)?;
                        let value = self.value()?;
                        let val = pnt.dot(abc) + value;

                        // points within tolerance
                        if val.abs() < self.base.tolerance_3d {
                            sol.solution_type = SolutionType::SingleValue;
                            sol.nodes[0] = branch[0]; // Point node
                            sol.nodes[1] = branch[1]; // Curve node
                            sol.num_objects = 1;
                            sol.objects[0] = owner;
                            sol.num_variables = 1;
                            set_param(&mut sol.start, 0, param)?;
                            sol.start.solution_value = val.abs();

                            if jj == 0 {
                                param = ivl.max();
                                let pnt2 = curve.evaluate_point(k, param)?;
                                let val2 = pnt2.dot(abc) + value;
                                let pnt3 = curve.evaluate_point(k, ivl.evaluate(0.4567))?;
                                let val3 = pnt3.dot(abc) + value;
                                if val2.abs() < self.base.tolerance_3d
                                    && val3.abs() < self.base.tolerance_3d
                                {
                                    sol.solution_type = SolutionType::RangeOfValues;
                                    set_param(&mut sol.end, 0, param)?;
                                    sol.end.solution_value = val.abs();
                                    self.add_sorted_solution(
                                        &sol,
                                        SortKey::ByFirstParameter,
                                        true,
                                    )?;
                                    return Ok(needs_more_subdivision);
                                }
                            } // end max interval value value check

                            // inform the public
                            self.add_sorted_solution(&sol, SortKey::ByFirstParameter, true)?;
                        } // end points within tolerance check
                    } // end plane intersection branch
                } // end aeConts[jj] < SM_CT_G1 check
            } // end iter over interval min and max
        } // end discontinuity cases

        let mut guess_t: Vec<f64> = if matches!(
            prop,
            CurvePropertyType::MinimizeAngleToPlane
                | CurvePropertyType::MinimizeDirectedAngle
                | CurvePropertyType::MinimizeCcwAngle
                | CurvePropertyType::ParallelToVector
        ) {
            // There are possible optimizations here that we could use to
            // check the angle of the curve relative to the point.
            vec![ivl.evaluate(0.5)]
        } else {
            // Just take 5 guesses
            vec![
                ivl.evaluate(0.5),
                ivl.evaluate(0.0),
                ivl.evaluate(1.0),
                ivl.evaluate(0.25),
                ivl.evaluate(0.75),
            ]
        };

        let num_original_guess = guess_t.len();

        // If we have three zeroes in this little span, assume it's a zero function
        // and create a Range solution.  [B500. before other fix]
        let mut num_serendipities: u64 = 0;

        // each guess
        let mut ii = 0;
        while ii < guess_t.len() {
            let guess = *guess_t
                .get(ii)
                .ok_or(SolidError::invalid_input(OP_GS, "guess index"))?;
            let this_ii = ii;
            ii += 1;
            let mut found = match local_property_analysis(
                k,
                curve,
                &ivl,
                prop,
                guess,
                self.property_value,
                &self.opt_vectors,
                &mut sol,
            ) {
                Ok(f) => f,
                Err(e) if e.is_unported() => return Err(e),
                // Had an error just try a different starting point - perhaps some
                // higher derivative was zero.
                Err(_) => continue,
            };

            if found && prop == CurvePropertyType::ParallelToVector {
                // nonZeroTangent values
                let pv: [Vec3; 2] = eval_n(k, curve, sol.start.param(0)?, 1, true, true)?;
                let mut angle = self.opt(0)?.angle_between(pv[1])?;
                if angle > PI / 2.0 {
                    angle = PI - angle;
                }
                if angle > self.base.tolerance_3d {
                    found = false;
                }
            }

            // If we did not find a solution the first time then stop
            if this_ii == 0 && !found {
                return Ok(needs_more_subdivision);
            }
            if !found {
                continue; // If fail don't save answer
            }

            // Have a solution.
            // Was the guess point a solution?
            if are_same_to_tol(guess, sol.start.param(0)?, EFF_ZERO) {
                num_serendipities += 1;
            }

            if prop == CurvePropertyType::PlaneIntersection {
                // This returns signed solution values.  AddSortedSolution() requires
                // absolute values.
                sol.start.solution_value = sol.start.solution_value.abs();
                if sol.solution_type == SolutionType::RangeOfValues {
                    sol.end.solution_value = sol.end.solution_value.abs();
                }

                // Also, look for a nearby guess: if nearly tangent,
                // make a circle at the solution and intersect with plane.
                if this_ii < num_original_guess {
                    if let Some(new_guess) =
                        find_additional_cp_guess(k, curve, &ivl, self.opt(0)?, sol.start.param(0)?)?
                    {
                        guess_t.push(new_guess);
                    }
                }
            }

            sol.nodes[0] = branch[0]; // Curve node
            sol.num_variables = 1;
            sol.num_objects = 1;
            sol.objects[0] = owner;

            // Check for a Range solution.
            if num_serendipities >= 3 {
                // Make a Range solution and we're done.
                sol.solution_type = SolutionType::RangeOfValues;
                set_param(&mut sol.start, 0, ivl.min())?;
                set_param(&mut sol.end, 0, ivl.max())?;
                sol.end.solution_value = sol.start.solution_value;
                self.add_sorted_solution(&sol, SortKey::ByFirstParameter, true)?;
                return Ok(needs_more_subdivision);
            }

            if matches!(
                prop,
                CurvePropertyType::MinimizeFirstDerivative
                    | CurvePropertyType::MaximizeFirstDerivative
                    | CurvePropertyType::MinimizeRadiusOfCurvature
                    | CurvePropertyType::MaximizeRadiusOfCurvature
                    | CurvePropertyType::MinimizeCcwAngle
                    | CurvePropertyType::MinimizeAngleToPlane
                    | CurvePropertyType::MinimizeDirectedAngle
                    | CurvePropertyType::ProjectedPointMaximize
                    | CurvePropertyType::ProjectedPointMinimize
            ) {
                self.add_sorted_solution(&sol, SortKey::ByValueWithParameter, true)?;
            } else {
                self.add_sorted_solution(&sol, SortKey::ByFirstParameter, true)?;
            }
        } // end iter every guess point

        // all done
        Ok(needs_more_subdivision)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::hermite_curve::HermiteCurve;
    use crate::curve::line::{BoundedLineOptions, Line};

    fn hermite(p1: [f64; 3], d1: [f64; 3], p2: [f64; 3], d2: [f64; 3]) -> Curve {
        Curve::from(HermiteCurve::new(
            Vec3::from_array(p1),
            Vec3::from_array(d1),
            Vec3::from_array(p2),
            Vec3::from_array(d2),
            3,
        ))
    }

    fn line(p: [f64; 3], v: [f64; 3], a: f64, b: f64) -> Curve {
        Curve::from(Line::bounded(
            Vec3::from_array(p),
            Vec3::from_array(v),
            &Extent1d::from_min_max(a, b),
            &BoundedLineOptions::default(),
        ))
    }

    #[test]
    fn normal_of_a_planar_loop() {
        let k = &Kernel::default();
        // a counter-clockwise loop in the XY plane made of curved Hermite pieces
        let curves = vec![
            hermite(
                [0.0, 0.0, 0.0],
                [1.0, -0.5, 0.0],
                [1.0, 0.0, 0.0],
                [1.0, 0.5, 0.0],
            ),
            hermite(
                [1.0, 0.0, 0.0],
                [0.5, 1.0, 0.0],
                [1.0, 1.0, 0.0],
                [-0.5, 1.0, 0.0],
            ),
            hermite(
                [1.0, 1.0, 0.0],
                [-1.0, 0.5, 0.0],
                [0.0, 1.0, 0.0],
                [-1.0, -0.5, 0.0],
            ),
            hermite(
                [0.0, 1.0, 0.0],
                [-0.5, -1.0, 0.0],
                [0.0, 0.0, 0.0],
                [0.5, -1.0, 0.0],
            ),
        ];
        let n = compute_crvs_normal(k, &curves).unwrap();
        assert!((n.z - 1.0).abs() < 1e-12, "{n:?}");
    }

    #[test]
    fn single_line_has_no_normal() {
        let k = &Kernel::default();
        let l = line([0.0; 3], [1.0, 0.0, 0.0], 0.0, 1.0);
        assert!(compute_crvs_normal(k, &[l]).is_err());
    }

    #[test]
    fn arc_length_on_a_line() {
        let k = Kernel::default();
        let l = line([0.0; 3], [2.0, 0.0, 0.0], 0.0, 5.0);
        let ivl = l.natural_interval();
        let r = l
            .find_parameter_at_arc_length(&k, ivl.min(), 0.25 * ivl.length() * 2.0)
            .unwrap();
        assert!(r.converged);
        assert!(!r.past_end);
        let back = l.find_parameter_at_arc_length(&k, ivl.max(), -1e6).unwrap();
        assert!(back.past_end);
    }

    #[test]
    fn closest_point_on_a_hermite_curve() {
        let k = &Kernel::default();
        let c = hermite(
            [0.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [2.0, 0.0, 0.0],
            [1.0, -1.0, 0.0],
        );
        let ivl = c.natural_interval();
        let (found, sol): (bool, Solution<()>) = c
            .local_point_solve(
                k,
                &ivl,
                SolverOperation::Minimize,
                Vec3::new(1.0, 2.0, 0.0),
                &PointSolveOptions::default(),
                0.4,
            )
            .unwrap();
        assert!(found);
        assert!((sol.start.parameters[0] - 0.5).abs() < 1e-8);
    }

    #[test]
    fn point_extrema_rejects_overflowed_hermite_tangent_without_scaling_forever() {
        let k = &Kernel::default();
        let c = hermite(
            [0.0; 3],
            [f64::MAX / 2.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0; 3],
        );
        let domain = c.natural_interval();
        let mut evaluator = FindPcExtremaEfo::new(
            k,
            &c,
            Vec3::ZERO,
            domain,
            EFF_ZERO_SQ,
            SolverOperation::Minimize,
        );
        let mut value = FunctionValue::default();
        assert!(matches!(
            evaluator.evaluate(0.25, &mut value, true),
            Err(SolidError::Upstream { code: 1001, .. })
        ));
        let (found, _): (bool, Solution<()>) = c
            .local_point_solve(
                k,
                &domain,
                SolverOperation::Minimize,
                Vec3::ZERO,
                &PointSolveOptions::default(),
                0.25,
            )
            .unwrap();
        assert!(!found);
    }

    #[test]
    fn crossing_lines_intersect_locally() {
        let k = &Kernel::default();
        let a = hermite([0.0; 3], [1.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 0.0, 0.0]);
        let b = hermite(
            [0.5, -1.0, 0.0],
            [0.0, 2.0, 0.0],
            [0.5, 1.0, 0.0],
            [0.0, 2.0, 0.0],
        );
        let ivl = a.natural_interval();
        let r = local_curve_intersect(k, &a, &ivl, &b, &ivl, 1e-6, 0.4, 0.6)
            .unwrap()
            .unwrap();
        assert!((r.my_param - 0.5).abs() < 1e-9, "{r:?}");
        assert!((r.other_param - 0.5).abs() < 1e-9, "{r:?}");
        assert!(r.deviation < 1e-9);
    }
}
