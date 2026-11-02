//! `SmCurve`, part 4: chaining a curve list head to tail, the closed / degenerate
//! tests and the sampled polar box of the base class, knot snapping, the base-class
//! validity check, the analytic (line / ellipse) intersection dispatcher, the
//! curve/curve solution filter and projected distance helpers of the solvers, and the
//! evaluator objects of the curve Newton solvers (`SmCurvePropertyEFO`,
//! `SmCCTangentIntENFO`, `SmCPDistAtENFO`, ...).
//!
//! Upstream: `source/SMLib/src/SmCurve.cpp`, `source/SMLib/inc/SmCurve.h`.
//!
//! | Rust                                              | Upstream                                         |
//! |---------------------------------------------------|--------------------------------------------------|
//! | [`fix_chain_curves_list`], [`ChainedCurves`]      | `SmCurve::FixChainCurvesList` (static)           |
//! | [`filter_solution`]                               | `sm_FilterSolution` (static)                     |
//! | [`analytic_curve_intersect`], [`AnalyticOperand`], [`Curve::analytic_curve_intersect`] | `SmCurve::AnalyticCurveIntersect` |
//! | [`proj_dist`]                                     | `sm_GetProjDist` (static)                        |
//! | [`approximate_polar_box`], [`Curve::approximate_polar_box`] | `SmCurve::ApproximatePolarBox`         |
//! | [`is_closed_base`], [`Curve::is_closed`]          | `SmCurve::IsClosed`                              |
//! | [`has_plane_crossings`]                           | `SmCurve::HasPlaneCrossings`                     |
//! | [`is_degenerate_base`], [`Curve::is_degenerate`]  | `SmCurve::IsDegenerate`                          |
//! | [`Curve::snap_to_knots`]                          | `SmCurve::SnapToKnots`                           |
//! | [`assert_valid_base`], [`Curve::assert_valid`]    | `SmCurve::AssertValid`                           |
//! | [`CurvePropertyEfo`]                              | `SmCurvePropertyEFO` (constructor, `Evaluate`)   |
//! | [`CcTangentIntEnfo`], [`CcTangentIntEnfo::static_evaluate`] | `SmCCTangentIntENFO` (constructor, `StaticEvaluate`) |
//! | [`CcIntersectEnfo`]                               | `SmCCIntersectENFO` (constructor, `HasIntervals`, `SetIntervals`) |
//! | [`FindCcNormalEnfo`]                              | `SmFindCCNormalENFO` (constructor)               |
//! | [`FindPcExtremaEfo`]                              | `SmFindPCExtremaEFO` (constructor)               |
//! | [`CpDistAtEnfo`]                                  | `SmCPDistAtENFO` (constructor, `Evaluate`)       |
//! | `EvalFunction for` [`FindCurveDistanceExtrema`]   | `SmFindCurveDistanceExtremaEFO::Evaluate` (co-ported in [`crate::curve::curve_cache`]) |
//!
//! Deviations:
//! - **Base bodies.** The virtual `IsClosed`, `IsDegenerate` and `ApproximatePolarBox`
//!   are generic over [`CurveEvaluate`] (`*_base`), so curve kinds whose overrides
//!   fall back to the base class (the B-spline `IsDegenerate`) can call them on
//!   themselves; [`Curve::is_closed`] and [`Curve::is_degenerate`] dispatch to the
//!   line and B-spline overrides. Evaluation statuses upstream ignores keep upstream's
//!   values (the previous evaluation, or [`Vec3::UNDEF`] for a never-written
//!   `SmVector3d`), except [`SolidError::Unported`], which is always propagated.
//! - **`FixChainCurvesList`** takes the curve list by value (`None` for a `NULL`
//!   entry) and returns the chained curves with the chain starts, the input index of
//!   every output curve and the number of deleted duplicates (not reported upstream).
//!   `FindElement` (pointer identity)
//!   becomes the index of the chosen link. Statuses of `EvaluatePoint` and
//!   `ReverseParameterization` are ignored, as upstream.
//! - **`sm_FilterSolution`** passes a two-entry evaluation to `sm_UnitizeTangent`,
//!   which reads a third entry (the second derivative) when the first derivative has
//!   no length; that read is out of bounds upstream and is [`Vec3::UNDEF`] here.
//! - **`AnalyticCurveIntersect`**: [`SmEllipse`](Ellipse) is not a [`CurveKind`] yet,
//!   so the operands are [`AnalyticOperand`]s (a curve or an ellipse); solutions name
//!   the caller's object ids for this and the other curve.
//! - **`sm_GetProjDist`** returns the failing `SmStatus` of `EvaluatePoint` converted
//!   to a double as the distance; the port returns the error.
//! - **`ApproximatePolarBox`** returns a new box (`ReSet` of the caller's box keeps
//!   only its unused basis and domain).
//! - **`HasPlaneCrossings`** takes the kernel and the planes as [`Plane`]s (the plane's
//!   `GlobalCurveIntersect` is [`Surface::global_curve_intersect`]); its solutions name
//!   no object the result reads.
//! - **`AssertValid`** reports for the object `key`; the owner is an [`ObjectRef`]
//!   whose type code is compared with the allowed owner types (none of them has
//!   subclasses). The test requests of `SM_LEVEL_GIVEN` are not modelled (`SM_LEVEL_0`
//!   tests always run), as in the derived classes' ports.
//! - **Evaluator objects.** `SmCurvePropertyEFO` and `SmCPDistAtENFO` implement
//!   [`EvalFunction`] / [`EvalNFunctions`]; on failure they leave what upstream wrote
//!   before failing in the outputs. Properties or vectors a case dereferences without
//!   a `NULL` check (or arrays shorter than it reads) are
//!   [`SolidError::InvalidInput`]; upstream's explicit `NULL` checks stay `SM_ERR`.
//!   The `Evaluate` members of `SmCCTangentIntENFO`, `SmCCIntersectENFO`,
//!   `SmFindCCNormalENFO` (unit `curve.curve_p5`) and `SmFindPCExtremaEFO`
//!   (`curve.curve_p3`) are not ported here: their structs carry only the
//!   constructors' data, and those units add the trait implementations.

#[cfg(doc)]
use crate::curve::curve::p2::FindCurveDistanceExtrema;
use crate::curve::curve::p2::{
    at, unitize_tangent, Curve, CurveEvaluate, CurveKind, CurvePropertyType,
};
use crate::curve::curve::p3::eval_n;
use crate::curve::ellipse::Ellipse;
use crate::curve::line::Line;
use crate::error::{ignore_status, ignored, Result, SolidError};
use crate::geom::a_object;
use crate::geom::assert_array::{labels, AssertArray, AssertReport, ObjectKey, ReportSpec};
use crate::geom::extent1d::{Extent1d, ExtentNd};
use crate::geom::extent2d::Extent2d;
use crate::geom::geom_utility::p1::line_line_closest_point;
use crate::geom::geom_utility::p2::ContinuityType;
use crate::geom::global_solver::SolverOperation;
use crate::geom::local_solve1d::{EvalFunction, FunctionValue};
use crate::geom::local_solve_nd::EvalNFunctions;
use crate::geom::matrix::Matrix;
use crate::geom::object::ObjectRef;
use crate::geom::polar_box::PolarBox;
use crate::geom::solution_array::{
    ObjectKind, Solution, SolutionEnd, SolutionObject, SolutionType,
};
use crate::geom::tol::{are_same, is_zero, tol3d};
use crate::geom::vector3d::{sqrt, Point3, Vec3};
use crate::kernel::Kernel;
use crate::math::{max2, max4, BIG_DOUBLE, EFF_ZERO, EFF_ZERO_SQ, EFF_ZERO_SQRT, UNDEF_DOUBLE};
use crate::surface::plane::Plane;
use crate::surface::surface::Surface;

const OP_CHAIN: &str = "curve::curve::p4::fix_chain_curves_list";
const OP_EFO: &str = "curve::curve::p4::CurvePropertyEfo::evaluate";
const OP_TAN: &str = "curve::curve::p4::CcTangentIntEnfo::static_evaluate";
const OP_DIST_AT: &str = "curve::curve::p4::CpDistAtEnfo::evaluate";

/// `EvaluatePoint` whose status upstream ignores: [`Vec3::UNDEF`] (a never-written
/// `SmPoint3d`) on failure; [`SolidError::Unported`] is propagated.
pub(crate) fn point_or_undef<C: CurveEvaluate + ?Sized>(
    k: &Kernel,
    curve: &C,
    t: f64,
) -> Result<Point3> {
    Ok(ignored(curve.evaluate_point(k, t))?.unwrap_or(Vec3::UNDEF))
}

/// `Evaluate` whose status upstream ignores into `pv` (entries not written keep
/// their values); [`SolidError::Unported`] is propagated.
fn eval_into<C: CurveEvaluate + ?Sized>(
    k: &Kernel,
    curve: &C,
    t: f64,
    num_derivs: usize,
    non_zero_tangents: bool,
    pv: &mut [Vec3],
) -> Result<()> {
    if let Some(v) = ignored(curve.evaluate(k, t, num_derivs, true, non_zero_tangents))? {
        for (d, s) in pv.iter_mut().zip(v) {
            *d = s;
        }
    }
    Ok(())
}

// ---- chaining ---------------------------------------------------------------------------

/// Result of [`fix_chain_curves_list`].
#[derive(Debug, Clone)]
pub struct ChainedCurves {
    /// The curves in chain order, each oriented head to tail (`r3DCurves` on output).
    pub curves: Vec<Curve>,
    /// Index into [`ChainedCurves::curves`] of each chain's first curve
    /// (`rChainStart`; its length is the number of chains, the return value).
    pub chain_starts: Vec<usize>,
    /// The input index of every output curve.
    pub origin: Vec<usize>,
    /// Duplicates deleted (not reported upstream).
    pub deleted: usize,
}

/// The link chosen so far in one scan of [`fix_chain_curves_list`].
#[derive(Debug, Clone, Copy, PartialEq)]
enum Link {
    /// `pNextCrv == NULL` (no link yet).
    None,
    /// The curve at this input index.
    Index(usize),
}

/// Start, end and mid point of `curve` over its natural interval (statuses ignored).
fn ends_and_mid(k: &Kernel, curve: &Curve) -> Result<(Point3, Point3, Point3)> {
    let domain = curve.natural_interval();
    Ok((
        point_or_undef(k, curve, domain.min())?,
        point_or_undef(k, curve, domain.max())?,
        point_or_undef(k, curve, domain.evaluate(0.5))?,
    ))
}

/// Orders `curves` (any order and direction, `None` entries skipped) into head to
/// tail chains whose matching end points are less than `sqrt(tol*tol)` apart,
/// reversing curves as needed; with `delete_duplicates`, a curve whose start, end and
/// mid points match the chain end it would join is deleted and removed from the list
/// (`SmCurve::FixChainCurvesList`). An O(n^2) scan: each chain starts with the first
/// unused curve and grows at either end by the closest matching link until none is
/// left.
///
/// # Errors
///
/// [`SolidError::Unported`] from the evaluators.
pub fn fix_chain_curves_list(
    k: &Kernel,
    mut curves: Vec<Option<Curve>>,
    tol: f64,
    delete_duplicates: bool,
) -> Result<ChainedCurves> {
    // prepare output
    let mut chain_starts = Vec::new();

    // locals
    let mut chained: Vec<Curve> = Vec::new();
    let mut origin: Vec<usize> = Vec::new();
    let mut deleted = 0usize;
    let dtol2 = tol * tol; // length squared for point equality, else new chain

    // for every input curve - look for a new chain start
    for kk in 0..curves.len() {
        // skip NULL curves
        let Some(c0) = curves.get_mut(kk).and_then(Option::take) else {
            continue;
        };

        // remember where this next chain begins
        let loop_start = chained.len(); // will be 0 on 1st iteration
        chain_starts.push(loop_start);

        // last link Min(start), Max(end), and Mid(mid_start, mid_end) points
        let (mut start_point, mut end_point, mut mid_start_point) = ends_and_mid(k, &c0)?;
        let mut mid_end_point = mid_start_point;

        // move the curve into the growing chained array
        chained.push(c0);
        origin.push(kk);

        loop {
            // REDOLOOP: persistent locals for the following scan
            let mut dist2 = BIG_DOUBLE;
            let mut case = 0u8;
            let mut next = Link::None;
            let mut tmp_start_pt = Vec3::UNDEF;
            let mut tmp_end_pt = Vec3::UNDEF;
            let mut tmp_mid_start_pt = Vec3::UNDEF;
            let mut tmp_mid_end_pt = Vec3::UNDEF;

            // for every curve - look for next link to chain at start or end
            for j in 0..curves.len() {
                // skip links already used in a chain
                let Some(c1) = curves.get(j).and_then(Option::as_ref) else {
                    continue;
                };

                // next link Min(start1), Max(end1) and Mid(mid) points
                let (start_point1, end_point1, mid_point) = ends_and_mid(k, c1)?;

                let d_s1e = (start_point1 - end_point).length_squared();
                let d_e1e = (end_point1 - end_point).length_squared();
                let d_e1s = (end_point1 - start_point).length_squared();
                let d_s1s = (start_point1 - start_point).length_squared();

                // delete this curve and remove it from the list too
                let mut delete = |curves: &mut [Option<Curve>]| {
                    if curves.get_mut(j).and_then(Option::take).is_some() {
                        deleted += 1;
                    }
                };

                // when links connects 1 of 4, (ThisLinkStart == LastLinkEnd to tol2)
                if d_s1e < dtol2 && d_s1e < dist2 {
                    // when deleting duplicates and next link is a duplicate
                    if delete_duplicates
                        && d_e1s < dtol2
                        && (mid_end_point - mid_point).length_squared() < dtol2
                    {
                        delete(&mut curves);
                        continue;
                    }
                    // add link to end
                    case = 1;
                    dist2 = d_s1e;
                    next = Link::Index(j);
                    tmp_end_pt = end_point1;
                    tmp_mid_end_pt = mid_point;
                }

                // when links connects 2 of 4, (ThisLinkEnd == LastLinkEnd to tol2)
                if d_e1e < dtol2 && d_e1e < dist2 {
                    if delete_duplicates
                        && d_s1s < dtol2
                        && (mid_end_point - mid_point).length_squared() < dtol2
                    {
                        delete(&mut curves);
                        continue;
                    }
                    // Reverse link and add to end
                    case = 2;
                    dist2 = d_e1e;
                    next = Link::Index(j);
                    tmp_end_pt = start_point1;
                    tmp_mid_end_pt = mid_point;
                }

                // when links connects 3 of 4, (ThisLinkEnd == LastLinkStart to tol2)
                if d_e1s < dtol2 && d_e1s < dist2 {
                    if delete_duplicates
                        && d_s1e < dtol2
                        && (mid_start_point - mid_point).length_squared() < dtol2
                    {
                        delete(&mut curves);
                        continue;
                    }
                    // insert link before start
                    case = 3;
                    dist2 = d_e1s;
                    next = Link::Index(j);
                    tmp_start_pt = start_point1;
                    tmp_mid_start_pt = mid_point;
                }

                // when links connects 4 of 4, (ThisLinkStart == LastLinkStart to tol2)
                if d_s1s < dtol2 && d_s1s < dist2 {
                    if delete_duplicates
                        && d_e1e < dtol2
                        && (mid_start_point - mid_point).length_squared() < dtol2
                    {
                        delete(&mut curves);
                    } else {
                        // reverse and insert at start
                        case = 4;
                        dist2 = d_s1s;
                        next = Link::Index(j);
                        tmp_start_pt = end_point1;
                        tmp_mid_start_pt = mid_point;
                    }
                }
            } // end iter j looking for next curve link to add to current chain

            // If we found a link, handle as needed, or start new chain
            if case == 0 {
                break;
            }
            let Link::Index(idx) = next else {
                // (a case is set only with a link)
                return Err(SolidError::InvalidInput {
                    operation: OP_CHAIN,
                    reason: "a link case without a link",
                });
            };
            let mut next_crv =
                curves
                    .get_mut(idx)
                    .and_then(Option::take)
                    .ok_or(SolidError::InvalidInput {
                        operation: OP_CHAIN,
                        reason: "chosen link already used",
                    })?;
            match case {
                1 => {
                    chained.push(next_crv);
                    origin.push(idx);
                    end_point = tmp_end_pt;
                    mid_end_point = tmp_mid_end_pt;
                }
                2 => {
                    let domain1 = next_crv.natural_interval();
                    reverse_ignored(k, &mut next_crv, &domain1)?;
                    chained.push(next_crv);
                    origin.push(idx);
                    end_point = tmp_end_pt;
                    mid_end_point = tmp_mid_end_pt;
                }
                3 => {
                    chained.insert(loop_start, next_crv);
                    origin.insert(loop_start, idx);
                    start_point = tmp_start_pt;
                    mid_start_point = tmp_mid_start_pt;
                }
                _ => {
                    let domain1 = next_crv.natural_interval();
                    reverse_ignored(k, &mut next_crv, &domain1)?;
                    chained.insert(loop_start, next_crv);
                    origin.insert(loop_start, idx);
                    start_point = tmp_start_pt;
                    mid_start_point = tmp_mid_start_pt;
                }
            }
            // if done with link, start j over since we may have stepped over next link.
        }
    } // end iter kk looking for an available start chain curve

    // all done
    Ok(ChainedCurves {
        curves: chained,
        chain_starts,
        origin,
        deleted,
    })
}

/// `ReverseParameterization(domain, domain)` with its status ignored.
fn reverse_ignored(k: &Kernel, curve: &mut Curve, domain: &Extent1d) -> Result<()> {
    ignore_status(curve.reverse_parameterization(k, domain))
}

// ---- solver helpers -----------------------------------------------------------------------

/// `sm_FilterSolution`: whether a curve/curve solution found on the intervals
/// `ivl1` / `ivl2` of the two curves (`start.parameters = [t1, t2]`,
/// `start.solution_value` the gap) is kept. Exact (gap below `SM_EFF_ZERO` or the
/// scaled zero) and tangential (gap perpendicular to both tangents) answers are
/// kept; an answer on an interval end where the curve continues (`conts*` at least
/// `G1`) is dropped when the next interval gets closer; one on a curve end (`conts*`
/// at most `C0`) is kept; others are dropped. `adjacency` 1 / 2 drops the shared end
/// of adjacent spans in self intersection (`[1st, 2nd]` / `[2nd, 1st]`).
///
/// # Errors
///
/// Those of the curve evaluations.
pub fn filter_solution<C1, C2, O>(
    k: &Kernel,
    curve1: &C1,
    ivl1: &Extent1d,
    curve2: &C2,
    ivl2: &Extent1d,
    conts1: [ContinuityType; 2],
    conts2: [ContinuityType; 2],
    adjacency: u64,
    solution: &Solution<O>,
) -> Result<bool>
where
    C1: CurveEvaluate + ?Sized,
    C2: CurveEvaluate + ?Sized,
{
    let [s0, s1, ..] = solution.start.parameters;
    let value = solution.start.solution_value;
    let [c1_lo, c1_hi] = conts1.map(|c| c as i32);
    let [c2_lo, c2_hi] = conts2.map(|c| c as i32);
    let g1 = ContinuityType::G1 as i32;
    let c0 = ContinuityType::C0 as i32;

    // If we have an exact answer keep it
    if value < EFF_ZERO {
        return Ok(true);
    }

    // Eliminate adjacency cases when doing self intersection
    if (adjacency == 1 && are_same(ivl1.min(), s0)) || (adjacency == 2 && are_same(ivl1.max(), s0))
    {
        return Ok(false);
    }

    // Get Solution CurvePoints and CurveTangents (nonZeroTangent values)
    let pv1: [Vec3; 2] = eval_n(k, curve1, s0, 1, true, true)?;
    let pv2: [Vec3; 2] = eval_n(k, curve2, s1, 1, true, true)?;
    let [p1, t1] = pv1;
    let [p2, t2] = pv2;

    // keep answers good to ScaledZero (sensitive to parameter values [B628])
    let scaled_zero = EFF_ZERO
        * (1.0
            + max4(
                max2(p1.max_dimension(), t1.max_dimension()),
                max2(p2.max_dimension(), t2.max_dimension()),
                s0,
                s1,
            ));
    if value < scaled_zero {
        return Ok(true);
    }

    // classify the solution Point against each interval
    let start1 = are_same(ivl1.min(), s0);
    let end1 = are_same(ivl1.max(), s0);
    let mid1 = !start1 && !end1;
    let next1 = mid1 || (start1 && c1_lo >= g1) || (end1 && c1_hi >= g1);

    let start2 = are_same(ivl2.min(), s1);
    let end2 = are_same(ivl2.max(), s1);
    let mid2 = !start2 && !end2;
    let next2 = mid2 || (start2 && c2_lo >= g1) || (end2 && c2_hi >= g1);

    // get gap vector
    let gap_vector = p2 - p1;

    // when either solution is on a boundary with a next segment
    if (!mid1 && next1) || (!mid2 && next2) {
        // get unit tangents pointing into next segment (sm_UnitizeTangent reads a
        // third entry upstream does not have: UNDEF here)
        let (mut u1, _) = unitize_tangent([p1, t1, Vec3::UNDEF], gap_vector);
        if start1 {
            u1 = -u1;
        }
        let (mut u2, _) = unitize_tangent([p2, t2, Vec3::UNDEF], -gap_vector);
        if start2 {
            u2 = -u2;
        }

        // toss sol when next segment is closer to other solution current point
        let t1_gap = u1.dot(gap_vector);
        let t2_gap = u2.dot(gap_vector);

        let perp1 = false; // [B124 and similar]
        let perp2 = false;
        let perp12 = perp1 || perp2;

        let dot_limit = EFF_ZERO_SQRT; // Can't be smaller than this.  [B139 B175 B452]

        if (!mid1 && next1 && t1_gap > dot_limit && !perp1)
            || (!mid2 && next2 && t2_gap < -dot_limit && !perp2)
        {
            return Ok(false);
        }

        // compare Next1 with This2
        if !mid1 && next1 && !perp12 {
            let v21 = -u2 - u1;
            if v21.dot(gap_vector) < -dot_limit {
                return Ok(false);
            }
        }
        // compare Next1 with Next2
        if !mid1 && next1 && next2 && !perp12 {
            let v21 = u2 - u1;
            if v21.dot(gap_vector) < -dot_limit {
                return Ok(false);
            }
        }
        // compare Next2 with This1
        if !mid2 && next2 && !perp12 {
            let v21 = u2 + u1;
            if v21.dot(gap_vector) < -dot_limit {
                return Ok(false);
            }
        }
        // compare Next2 with Next1
        if !mid2 && next2 && next1 && !perp12 {
            let v21 = u2 - u1;
            if v21.dot(gap_vector) < -dot_limit {
                return Ok(false);
            }
        }
    } // end solution on an internal boundary check

    // keep minimization points found at curve/curve tangentIntersections
    // when CurveTangents are perpendicular to gapVector to within 1 degree
    if gap_vector.is_perpendicular_to(t1, 1.0) && gap_vector.is_perpendicular_to(t2, 1.0) {
        return Ok(true);
    }

    // keep intersections on Curve Boundaries (not interval boundaries)
    if (c1_lo <= c0 && start1)
        || (c1_hi <= c0 && end1)
        || (c2_lo <= c0 && start2)
        || (c2_hi <= c0 && end2)
    {
        return Ok(true);
    }

    // else discard the answer
    Ok(false)
}

/// `sm_GetProjDist`: the length of `test_point - C(param)` projected onto the plane
/// of normal `plane_norm` (`N x V x N`); for the projected and directed
/// minimise/maximise operations as is, otherwise signed by its direction against
/// `dir_vector` (negated with `reverse_order`).
///
/// # Errors
///
/// Those of [`CurveEvaluate::evaluate_point`] (upstream returns the failing status
/// converted to a double).
pub fn proj_dist<C: CurveEvaluate + ?Sized>(
    k: &Kernel,
    curve: &C,
    param: f64,
    test_point: Point3,
    plane_norm: Vec3,
    reverse_order: bool,
    dir_vector: Vec3,
    op: SolverOperation,
) -> Result<f64> {
    // evaluate curve at given parameter
    let pnt = curve.evaluate_point(k, param)?;

    // get vector from testPoint to curvePoint projected to given plane
    let vec = test_point - pnt;
    let mut vec_proj = plane_norm.cross(vec).cross(plane_norm);

    // get projected vector length
    let dist = vec_proj.length();

    // return appropriate answer
    match op {
        SolverOperation::ProjectedMinimize | SolverOperation::ProjectedMaximize => Ok(dist),
        SolverOperation::DirectedMinimize | SolverOperation::DirectedMaximize => Ok(dist.abs()),
        _ => {
            if reverse_order {
                vec_proj = -vec_proj;
            }
            Ok(if dir_vector.dot(vec_proj) >= 0.0 {
                dist
            } else {
                -dist
            })
        }
    }
}

// ---- analytic intersections ---------------------------------------------------------------

/// An operand of [`analytic_curve_intersect`]: a curve or an ellipse (`SmEllipse`
/// is not a [`CurveKind`] yet).
#[derive(Debug, Clone, Copy)]
pub enum AnalyticOperand<'a> {
    /// A curve (only its [`Line`] kind is analytic here).
    Curve(&'a Curve),
    /// An ellipse or circle.
    Ellipse(&'a Ellipse),
}

impl<'a> AnalyticOperand<'a> {
    /// `SM_CAST_NONNULL_PTR(SmLine, p)`.
    fn line(self) -> Option<&'a Line> {
        match self {
            Self::Curve(c) => match c.kind() {
                CurveKind::Line(l) => Some(l),
                _ => None,
            },
            Self::Ellipse(_) => None,
        }
    }

    /// `SM_CAST_NONNULL_PTR(SmEllipse, p)`.
    const fn ellipse(self) -> Option<&'a Ellipse> {
        match self {
            Self::Ellipse(e) => Some(e),
            Self::Curve(c) => match c.kind() {
                CurveKind::Ellipse(e) => Some(e),
                _ => None,
            },
        }
    }
}

/// Result of [`analytic_curve_intersect`].
#[derive(Debug, Clone, PartialEq)]
pub struct AnalyticIntersection<O> {
    /// Intersections found (`start.parameters`: `[this param, other param]`).
    pub solutions: Vec<Solution<O>>,
    /// Whether the general curve/curve intersector must be called.
    pub needs_more_intersections: bool,
}

/// Intersections of `this` over `interval` with `other` over `other_interval`
/// within `tol` by the analytic intersectors (`SmCurve::AnalyticCurveIntersect`):
/// line/line, line/ellipse, ellipse/line and ellipse/ellipse; any other pair finds
/// nothing and needs the general intersector. Solutions name `this_obj` and
/// `other_obj`.
///
/// # Errors
///
/// Those of the line and ellipse intersectors.
pub fn analytic_curve_intersect<O: Copy>(
    k: &Kernel,
    this: AnalyticOperand<'_>,
    interval: &Extent1d,
    other: AnalyticOperand<'_>,
    other_interval: &Extent1d,
    tol: f64,
    this_obj: Option<O>,
    other_obj: Option<O>,
) -> Result<AnalyticIntersection<O>> {
    // init output
    let none = AnalyticIntersection {
        solutions: Vec::new(),
        needs_more_intersections: true,
    };

    // when thisCurve is a line
    if let Some(this_line) = this.line() {
        // when otherCurve is a line try line/line intersection
        if let Some(line) = other.line() {
            let r = this_line.intersect_with_line(
                interval,
                line,
                other_interval,
                tol,
                this_obj,
                other_obj,
            )?;
            return Ok(AnalyticIntersection {
                solutions: r.solutions,
                needs_more_intersections: r.needs_more_intersections,
            });
        }

        // when otherCurve is an ellipse try line/ellipse intersection
        if let Some(ellipse) = other.ellipse() {
            let r = this_line.intersect_with_ellipse(
                k,
                interval,
                ellipse,
                other_interval,
                tol,
                this_obj,
                other_obj,
            )?;
            return Ok(AnalyticIntersection {
                solutions: r.solutions,
                needs_more_intersections: r.needs_more_intersections,
            });
        }
        return Ok(none);
    }

    // when thisCurve is an ellipse
    if let Some(this_ellipse) = this.ellipse() {
        // when OtherCurve is a line - try ellipse/line intersection
        if let Some(line) = other.line() {
            let r = this_ellipse.intersect_with_line(
                k,
                interval,
                line,
                other_interval,
                tol,
                this_obj,
                other_obj,
            )?;
            return Ok(AnalyticIntersection {
                solutions: r.solutions,
                needs_more_intersections: r.needs_more_intersections,
            });
        }

        // when OtherCurve is an ellipse - try ellipse/ellipse intersection
        if let Some(ellipse) = other.ellipse() {
            let r = this_ellipse.intersect_with_ellipse(
                k,
                interval,
                ellipse,
                other_interval,
                tol,
                this_obj,
                other_obj,
            )?;
            return Ok(AnalyticIntersection {
                solutions: r.solutions,
                needs_more_intersections: r.needs_more_intersections,
            });
        }
        return Ok(none);
    }

    // all done
    Ok(none)
}

// ---- base-class queries -------------------------------------------------------------------

/// `SmCurve::ApproximatePolarBox`: the polar box of the curve's tangents
/// (nonzero-tangent evaluations) at the mid parameter, then at `point_count` (made
/// odd) equally spaced parameters of `interval` clamped to the natural interval.
/// Evaluation statuses are ignored, as upstream.
///
/// # Errors
///
/// [`SolidError::Unported`] from the evaluator.
pub fn approximate_polar_box<C: CurveEvaluate + ?Sized>(
    k: &Kernel,
    curve: &C,
    interval: &Extent1d,
    point_count: u64,
) -> Result<PolarBox> {
    // make point_count odd
    let point_count = if point_count % 2 == 0 {
        point_count + 1
    } else {
        point_count
    };

    // bound interval to curve's natural interval
    let nat_int = curve.natural_interval();
    let ivl = Extent1d::from_min_max(
        nat_int.clamp_value(interval.min()),
        nat_int.clamp_value(interval.max()),
    );

    // init polarBox - set polar axis by adding 1st vector taken as curve's mid tangent
    let mut pv = [Vec3::UNDEF; 2];
    let mut polar_box = PolarBox::new();
    polar_box.reset();
    eval_into(k, curve, ivl.evaluate(0.5), 1, true, &mut pv)?;
    polar_box.add_vector(at(&pv, 1), None);

    // iteration locals
    let mut param = ivl.min();
    let step = ivl.length() / (point_count - 1) as f64;

    // for every sample point
    for _ in 0..point_count {
        // evaluate this param point
        eval_into(k, curve, param, 1, true, &mut pv)?;

        // add the tangent to the PolarBox
        polar_box.add_vector(at(&pv, 1), None);
        param += step;
    }

    // all done
    Ok(polar_box)
}

/// `SmCurve::IsClosed`: whether the curve's points at the ends of `interval` are
/// closer than `tol` (`0.0`: `SM_EFF_ZERO * (1 + |start|max)`) while the mid point is
/// farther. Evaluation statuses are ignored, as upstream.
///
/// # Errors
///
/// [`SolidError::Unported`] from the evaluator.
pub fn is_closed_base<C: CurveEvaluate + ?Sized>(
    k: &Kernel,
    curve: &C,
    interval: &Extent1d,
    tol: f64,
) -> Result<bool> {
    // get curve start, end, and mid 3d Points
    let p1 = point_or_undef(k, curve, interval.min())?;
    let p2 = point_or_undef(k, curve, interval.max())?;
    let p_mid = point_or_undef(k, curve, interval.evaluate(0.5))?;

    // let dTol = dTolerance (when given) or dScaledZero
    let tol = if tol == 0.0 {
        EFF_ZERO * (1.0 + p1.max_dimension())
    } else {
        tol
    };

    // curve is closed when startPt is within tolerance of endPoint
    // and out of tolerance of midPoint
    Ok(p1.distance(p2) < tol && p_mid.distance(p1) > tol)
}

/// `SmCurve::IsDegenerate`: whether the curve over `interval` (`None`: the natural
/// interval) collapses to a point: an empty interval, or ends closer than
/// `max(tol3d, scaled zero)` with first derivatives (scaled by the interval length)
/// below that at both ends and at the fraction `0.4567`, whose point is also at the
/// start. Evaluation statuses are ignored, as upstream.
///
/// # Errors
///
/// [`SolidError::Unported`] from the evaluator.
pub fn is_degenerate_base<C: CurveEvaluate + ?Sized>(
    k: &Kernel,
    curve: &C,
    tol3d: f64,
    interval: Option<&Extent1d>,
) -> Result<bool> {
    // evaluate interval start and end points
    let ivl = interval
        .copied()
        .unwrap_or_else(|| curve.natural_interval());

    // degenerate if no effective domain
    if ivl.min() == ivl.max() {
        return Ok(true);
    }
    let p1 = point_or_undef(k, curve, ivl.min())?;
    let p2 = point_or_undef(k, curve, ivl.max())?;

    // select a tolerance - scale it to p1 location
    let scaled_zero = EFF_ZERO * (1.0 + p1.max_dimension());
    let tol2 = max2(tol3d * tol3d, scaled_zero * scaled_zero);

    // when start and end points are close
    if p1.distance_squared(p2) < tol2 {
        let mut pv = [Vec3::UNDEF; 2];
        let lsq = ivl.length() * ivl.length();

        // if start, mid, or end tangents are nonZero - the curve is not degenerate
        eval_into(k, curve, ivl.min(), 1, false, &mut pv)?; // exact tangent lengths
        if at(&pv, 1).length_squared() * lsq > tol2 {
            return Ok(false);
        }

        eval_into(k, curve, ivl.max(), 1, false, &mut pv)?;
        if at(&pv, 1).length_squared() * lsq > tol2 {
            return Ok(false);
        }

        // not the exact midpoint (a projected circle seen edge-on) [091023]
        eval_into(k, curve, ivl.evaluate(0.4567), 1, false, &mut pv)?;
        if at(&pv, 1).length_squared() * lsq > tol2 {
            return Ok(false);
        }

        // p1 close to p2 and start, mid, end tangents are all zero
        if p1.distance_squared(at(&pv, 0)) > tol2 {
            return Ok(false);
        }

        return Ok(true);
    }

    // p1 not close to p2 - not degenerate
    Ok(false)
}

/// `SmCurve::HasPlaneCrossings`: whether the curve crosses one of `planes` (a point
/// intersection off the curve ends, or a range that is not the whole curve).
///
/// The status of `GlobalCurveIntersect` is ignored upstream (no `SER`): a failed
/// intersection leaves no solutions, so the plane is not crossed.
///
/// # Errors
///
/// [`SolidError::Unported`] from [`Surface::global_curve_intersect`].
pub fn has_plane_crossings(k: &Kernel, curve: &Curve, planes: &[Plane]) -> Result<bool> {
    // locals
    let ivl = curve.natural_interval();

    // for every plane
    for plane in planes {
        // find all Curve/Plane intersections
        // (status ignored: an error leaves the solution array empty)
        let solutions = ignored(plane_global_curve_intersect(
            k, plane, curve, &ivl, EFF_ZERO,
        ))?
        .unwrap_or_default();

        // for every solution - classify the intersection
        for (solution_type, start, end) in &solutions {
            let s0 = start.parameters.first().copied().unwrap_or(0.0);
            let e0 = end.parameters.first().copied().unwrap_or(0.0);
            // when curve intersections mark Curve/DiscontinuityPlane crossings
            if (*solution_type == SolutionType::SingleValue
                && !ivl.is_value_on_boundary(s0, EFF_ZERO).0)
                || (*solution_type == SolutionType::RangeOfValues
                    && (!ivl.is_value_on_boundary(s0, EFF_ZERO).0
                        || !ivl.is_value_on_boundary(e0, EFF_ZERO).0))
            {
                return Ok(true);
            }
        }
    }

    // arrive here when the curve does NOT cross any of the input planes
    Ok(false)
}

/// The objects of the plane intersection's solutions: the curve and the plane with
/// their natural interval and UV domain (upstream's objects are the curve and the
/// plane, whose parameter intervals `IdenticalSolutions` reads; nothing else reads
/// them).
#[derive(Debug, Clone, Copy, PartialEq)]
enum PlaneHandle {
    Curve(Extent1d),
    Plane(Extent2d),
}

impl SolutionObject for PlaneHandle {
    fn kind(&self) -> ObjectKind {
        match self {
            Self::Curve(_) => ObjectKind::Curve,
            Self::Plane(_) => ObjectKind::Surface,
        }
    }

    fn natural_interval(&self) -> Option<Extent1d> {
        match self {
            Self::Curve(ivl) => Some(*ivl),
            Self::Plane(_) => None,
        }
    }

    fn natural_uv_domain(&self) -> Option<Extent2d> {
        match self {
            Self::Curve(_) => None,
            Self::Plane(domain) => Some(*domain),
        }
    }
}

/// `pPlane->GlobalCurveIntersect(pPlane->GetNaturalUVDomain(), curve, ivl, tol)`:
/// the solutions' types, starts and ends.
fn plane_global_curve_intersect(
    k: &Kernel,
    plane: &Plane,
    curve: &Curve,
    ivl: &Extent1d,
    tol: f64,
) -> Result<Vec<(SolutionType, SolutionEnd, SolutionEnd)>> {
    let surface = Surface::from(plane.clone());
    let sols = surface.global_curve_intersect::<PlaneHandle>(
        k,
        [
            PlaneHandle::Curve(curve.natural_interval()),
            PlaneHandle::Plane(surface.natural_uv_domain_with_nurb(k)?),
        ],
        &surface.natural_uv_domain_with_nurb(k)?,
        curve,
        ivl,
        tol,
    )?;
    Ok(sols
        .as_slice()
        .iter()
        .map(|s| (s.solution_type, s.start, s.end))
        .collect())
}

// ---- validity -----------------------------------------------------------------------------

/// `SmCurve::AssertValid` of a curve of dimension `dim` and class `class_string`
/// (`GetClassString`) owned by `owner`, reporting for the object `key` into `list`:
/// the `SmAObject` checks (unless `level_given`), then test 0, the owner is `NULL`,
/// an edge, edge use, surface of extrusion or revolution, curve in volume, offset
/// curve or curve on surface, or (2D curves) a face. `true` when every check passes.
///
/// # Errors
///
/// Those of [`AssertReport::new`].
pub fn assert_valid_base(
    owner: Option<ObjectRef>,
    dim: u64,
    class_string: &'static str,
    list: Option<&mut AssertArray>,
    level_given: bool,
    key: ObjectKey,
) -> Result<bool> {
    // check base class validity
    let mut rtn = true;

    // call the base class AssertValid
    rtn &= if level_given {
        true
    } else {
        a_object::assert_valid(false)
    };

    // check type of SmCurve::m_pOwner
    let owner_ok = owner.is_none_or(|o| {
        matches!(
            o.type_code,
            labels::EDGE_TYPE
                | labels::EDGEUSE_TYPE
                | labels::SURF_OF_EXTRUSION_TYPE
                | labels::SURF_OF_REVOLUTION_TYPE
                | super::p2::CRV_IN_VOLUME_TYPE
                | super::p2::OFFSET_CURVE_TYPE
                | super::p2::CRV_ON_SURF_TYPE
        ) || (dim == 2 && o.type_code == labels::FACE_TYPE)
    });
    rtn &= if owner_ok {
        if let Some(l) = list {
            l.set_last_add(false);
        }
        true
    } else {
        if let Some(l) = list {
            let r = AssertReport::new(ReportSpec {
                reporter: Some(key),
                test_index: 0,
                test_class: labels::CURVE_TYPE,
                owner: Some(key),
                owner_type: labels::CURVE_TYPE,
                owner_type_string: Some(class_string),
                other: None,
                other_type: 0,
                other_type_string: Some("NULL"),
                tol3d: tol3d(UNDEF_DOUBLE),
                value: UNDEF_DOUBLE,
                list_index: 0,
                message: None,
            })?;
            l.add_unique_delete_duplicate(r);
        }
        false
    };

    // all done
    Ok(rtn)
}

impl Curve {
    /// `IsClosed` (base: [`is_closed_base`]; a line is never closed).
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the evaluator.
    pub fn is_closed(&self, k: &Kernel, interval: &Extent1d, tol: f64) -> Result<bool> {
        match self.kind() {
            CurveKind::Line(l) => Ok(l.is_closed(interval, tol)),
            CurveKind::Composite(c) => Ok(c.is_closed()),
            _ => is_closed_base(k, self, interval, tol),
        }
    }

    /// `IsDegenerate` (base: [`is_degenerate_base`]; B-spline, ellipse and line overrides).
    ///
    /// # Errors
    ///
    /// Those of [`crate::nurbs::b_spline_curve::BSplineCurve::is_degenerate`];
    /// [`SolidError::Unported`] from the evaluator.
    pub fn is_degenerate(
        &self,
        k: &Kernel,
        tol3d: f64,
        interval: Option<&Extent1d>,
    ) -> Result<bool> {
        match self.kind() {
            CurveKind::BSpline(c) => c.is_degenerate(k, tol3d, interval),
            CurveKind::Hyperbola(c) => c.base().is_degenerate(k, tol3d, interval),
            CurveKind::Ellipse(e) => Ok(e.is_degenerate(tol3d)),
            CurveKind::Parabola(c) => c.base().is_degenerate(k, tol3d, interval),
            CurveKind::Line(l) => Ok(l.is_degenerate(tol3d, interval)),
            _ => is_degenerate_base(k, self, tol3d, interval),
        }
    }

    /// `ApproximatePolarBox` ([`approximate_polar_box`]).
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the evaluator.
    pub fn approximate_polar_box(
        &self,
        k: &Kernel,
        interval: &Extent1d,
        point_count: u64,
    ) -> Result<PolarBox> {
        approximate_polar_box(k, self, interval, point_count)
    }

    /// `SnapToKnots`: `param` moved to the nearest knot closer than `snap_tol`, or
    /// unchanged.
    ///
    /// # Errors
    ///
    /// Those of [`Curve::knots`].
    pub fn snap_to_knots(&self, k: &Kernel, param: f64, snap_tol: f64) -> Result<f64> {
        // init output
        let mut snapped = param;

        // locals
        let mut jump = BIG_DOUBLE;
        let (knots, _) = self.knots(k, None)?;

        // for every knot
        for &knot in &knots {
            let test_jump = (knot - param).abs();

            // when TestJump is less than SnapDistance and the smallest jump yet seen
            if test_jump < snap_tol && test_jump < jump {
                // snap parameter to knot value
                snapped = knot;

                // remember smallest snap dist value
                jump = test_jump;
            }
        }

        // all done
        Ok(snapped)
    }

    /// `SmCurve::AssertValid` ([`assert_valid_base`] with this curve's owner,
    /// dimension and class).
    ///
    /// # Errors
    ///
    /// Those of [`assert_valid_base`].
    pub fn assert_valid(
        &self,
        list: Option<&mut AssertArray>,
        level_given: bool,
        key: ObjectKey,
    ) -> Result<bool> {
        assert_valid_base(
            self.owner(),
            self.dim(),
            self.class_string(),
            list,
            level_given,
            key,
        )
    }

    /// `AnalyticCurveIntersect` of this curve ([`analytic_curve_intersect`]).
    ///
    /// # Errors
    ///
    /// Those of [`analytic_curve_intersect`].
    pub fn analytic_curve_intersect<O: Copy>(
        &self,
        k: &Kernel,
        interval: &Extent1d,
        other: AnalyticOperand<'_>,
        other_interval: &Extent1d,
        tol: f64,
        this_obj: Option<O>,
        other_obj: Option<O>,
    ) -> Result<AnalyticIntersection<O>> {
        analytic_curve_intersect(
            k,
            AnalyticOperand::Curve(self),
            interval,
            other,
            other_interval,
            tol,
            this_obj,
            other_obj,
        )
    }
}

// ---- evaluator objects --------------------------------------------------------------------

/// The Newton function of a curve property (`SmCurvePropertyEFO`): `F(t)` and
/// `F'(t)` of [`CurvePropertyType`] at a curve parameter (see the upstream table in
/// `SmCurve.cpp`), with the property value, the points / vectors and the squared
/// tolerance some properties use.
#[derive(Debug, Clone, Copy)]
pub struct CurvePropertyEfo<'a, C: ?Sized> {
    /// Caller context for virtual evaluations.
    pub kernel: &'a Kernel,

    /// `m_crCurve`.
    pub curve: &'a C,
    /// `m_eCurveProperty`.
    pub property: CurvePropertyType,
    /// `*m_cpdPropertyValue` (`None` for `NULL`).
    pub property_value: Option<f64>,
    /// `m_cpOptVectors` (`None` for `NULL`).
    pub opt_vectors: Option<&'a [Vec3]>,
    /// `m_dSquaredTolerance`.
    pub squared_tolerance: f64,
}

impl<'a, C: CurveEvaluate + ?Sized> CurvePropertyEfo<'a, C> {
    /// `SmCurvePropertyEFO(crCurve, eCurveProperty, cpdPropertyValue, cpOptVectors,
    /// dSquaredTolerance)`.
    #[must_use]
    pub const fn new(
        k: &'a Kernel,
        curve: &'a C,
        property: CurvePropertyType,
        property_value: Option<f64>,
        opt_vectors: Option<&'a [Vec3]>,
        squared_tolerance: f64,
    ) -> Self {
        Self {
            kernel: k,
            curve,
            property,
            property_value,
            opt_vectors,
            squared_tolerance,
        }
    }

    /// Vector `i` of the option vectors upstream reads without a check.
    fn vec(&self, i: usize) -> Result<Vec3> {
        self.opt_vectors
            .and_then(|v| v.get(i).copied())
            .ok_or(SolidError::InvalidInput {
                operation: OP_EFO,
                reason: "property needs more option vectors",
            })
    }

    /// The option vectors after upstream's `NULL` check (`SER(SM_ERR)`).
    fn checked_vec(&self, i: usize) -> Result<Vec3> {
        if self.opt_vectors.is_none() {
            return Err(SolidError::sm_err(OP_EFO));
        }
        self.vec(i)
    }

    /// The property value upstream reads without a check.
    fn value(&self) -> Result<f64> {
        self.property_value.ok_or(SolidError::InvalidInput {
            operation: OP_EFO,
            reason: "property needs a property value",
        })
    }

    /// The property value after upstream's `NULL` check (`SER(SM_ERR)`).
    fn checked_value(&self) -> Result<f64> {
        self.property_value
            .ok_or_else(|| SolidError::sm_err(OP_EFO))
    }

    /// `m_crCurve.Evaluate(dT, n, TRUE, sC, non_zero)` into four entries.
    fn eval(&self, k: &Kernel, t: f64, n: usize, non_zero: bool) -> Result<[Vec3; 4]> {
        eval_n(k, self.curve, t, n, true, non_zero)
    }
}

impl<C: CurveEvaluate + ?Sized> EvalFunction for CurvePropertyEfo<'_, C> {
    // `signal_errors` only chooses whether upstream prints the error.
    fn evaluate(&mut self, t: f64, out: &mut FunctionValue, _signal_errors: bool) -> Result<()> {
        let err = || SolidError::sm_err(OP_EFO);

        // init output
        out.f = 0.0;
        out.f_prime = 0.0;
        out.found_answer = false;

        // Note: always evaluate from the Right span (low end of span).
        match self.property {
            CurvePropertyType::PointInversion => {
                // F = (C - P) . (C - P), F' = C' . (C-P)
                let p = self.checked_vec(0)?;
                let c = self.eval(self.kernel, t, 1, true)?;
                if c[1].length_squared() < EFF_ZERO {
                    return Err(err());
                }
                let cp = c[0] - p;
                out.f = cp.dot(cp);
                out.f_prime = cp.dot(c[1]);
            }
            CurvePropertyType::DistanceToPoint => {
                // F = (C - P) . (C - P) - dist*dist, F' = 2 * C' . (C-P)
                let p = self.checked_vec(0)?;
                let c = self.eval(self.kernel, t, 1, true)?;
                if c[1].length_squared() < EFF_ZERO {
                    return Err(err());
                }
                let cp = c[0] - p;
                let d = self.value()?;
                out.f = cp.dot(cp) - d * d;
                out.f_prime = 2.0 * cp.dot(c[1]);
            }
            CurvePropertyType::PerpendicularToVector => {
                // F = C' . V, F' = C'' . V
                let v = self.checked_vec(0)?;
                let c = self.eval(self.kernel, t, 2, true)?;
                if c[1].length_squared() < EFF_ZERO {
                    return Err(err());
                }
                out.f = v.dot(c[1]);
                out.f_prime = v.dot(c[2]);
            }
            CurvePropertyType::InflectionPoints => {
                // F = (C'' x C')**2, F' = 2(C'' x C')'.(C'' x C')
                let c = self.eval(self.kernel, t, 3, true)?;
                let a = c[2].cross(c[1]);
                let a_prime = c[2].derivative_of_cross_product(c[1], c[3], c[2]);
                out.f = a.dot(a);
                if out.f < self.squared_tolerance {
                    out.found_answer = true;
                }
                out.f_prime = 2.0 * a_prime.dot(a);
            }
            CurvePropertyType::RadiusOfCurvature => {
                let k = self.checked_value()?;
                // F = |C'|^3 / |C' x C''| - K
                let c = self.eval(self.kernel, t, 3, false)?;
                let b = c[1].length();
                if is_zero(b) {
                    return Err(err()); // Zero first derivative error
                }
                let numer = b * b * b;

                let a = c[1].cross(c[2]);
                let denom = a.length();
                if is_zero(denom) {
                    return Err(err()); // No curvature: infinite radius
                }
                out.f = numer / denom - k;

                let b_prime = c[1].dot(c[2]) / b;
                let numer_prime = 3.0 * b * b * b_prime;
                let a_prime = c[1].cross(c[3]);
                let denom_prime = a.dot(a_prime) / denom;
                out.f_prime = (numer_prime - (k + out.f) * denom_prime) / denom;

                if (out.f * out.f).abs() < self.squared_tolerance {
                    out.found_answer = true;
                }
            }
            CurvePropertyType::FirstDerivativeLength => {
                let k = self.checked_value()?;
                let c = self.eval(self.kernel, t, 2, false)?;
                // F = |C'| - K, F' = (C'.C'') / |C'|
                let len = sqrt(c[1].dot(c[1]));
                out.f = len - k;
                if out.f.abs() < self.squared_tolerance {
                    out.found_answer = true;
                }

                if len < EFF_ZERO_SQ {
                    if out.f < EFF_ZERO {
                        out.f_prime = 0.0;
                        return Ok(());
                    }
                    return Err(err());
                }
                out.f_prime = c[1].dot(c[2]) / len;
            }
            CurvePropertyType::ParallelToVector => {
                let v = self.checked_vec(0)?;
                let c = self.eval(self.kernel, t, 3, false)?;
                // F = (C'/|C'| x V)^2, F' = (same) . (same)'
                let length = c[1].length();
                if is_zero(length) {
                    return Err(err());
                }
                let cu = c[1] / length;
                let cup = c[1].unitized_derivative(c[2]);
                let same_thing = cu.cross(v);
                out.f = same_thing.dot(same_thing);

                let vp = Vec3::new(0.0, 0.0, 0.0);
                let same_thing_prime = cu.derivative_of_cross_product(v, cup, vp);
                out.f_prime = same_thing.dot(same_thing_prime);
            }
            CurvePropertyType::SilhouetteVector => {
                let v = self.checked_vec(0)?;
                // F = C' x C'' x C' . V (unitized)
                let c = self.eval(self.kernel, t, 3, true)?;
                let mut cpu = c[1];
                if c[1].length_squared() < EFF_ZERO {
                    return Err(err());
                }

                // no silhouette when tangent is perp to the viewing vector.
                if c[1].dot(v).abs() < EFF_ZERO_SQRT {
                    return Err(err());
                }

                cpu = cpu.unitize_or_z();
                let cpup = c[1].unitized_derivative(c[2]);
                if c[2].length_squared() < EFF_ZERO {
                    return Err(err());
                }
                let cppu = c[2].unitize_or_z();
                let cppup = c[2].unitized_derivative(c[3]);
                let a = cpu.cross(cppu);
                if a.length_squared() < EFF_ZERO {
                    return Err(err());
                }
                let osc_plane_vec = a.cross(cpu);
                out.f = v.dot(osc_plane_vec);
                let a_prime = cpu.derivative_of_cross_product(cppu, cpup, cppup);
                let tmp = a.derivative_of_cross_product(cpu, a_prime, cpup);
                out.f_prime = tmp.dot(v);
            }
            CurvePropertyType::SilhouettePoint => {
                let p = self.checked_vec(0)?;
                let c = self.eval(self.kernel, t, 3, true)?;
                let c_minus_p = c[0] - p;
                // F = C' x C'' x C' . (C-P) (unitized)
                let mut cpu = c[1];
                if c[1].length_squared() < EFF_ZERO {
                    return Err(err());
                }

                // tangent in the silhouette plane: not really a silhouette
                if c[1].dot(c_minus_p).abs() < EFF_ZERO_SQRT {
                    return Err(err());
                }

                cpu = cpu.unitize_or_z();
                let cpup = c[1].unitized_derivative(c[2]);
                if c[2].length_squared() < EFF_ZERO {
                    return Err(err());
                }
                let cppu = c[2].unitize_or_z();
                let cppup = c[2].unitized_derivative(c[3]);
                let a = cpu.cross(cppu);
                if a.length_squared() < EFF_ZERO {
                    return Err(err());
                }
                let osc_plane_vec = a.cross(cpu);
                out.f = c_minus_p.dot(osc_plane_vec);
                let a_prime = cpu.derivative_of_cross_product(cppu, cpup, cppup);
                let tmp = a.derivative_of_cross_product(cpu, a_prime, cpup);
                out.f_prime = tmp.dot(c_minus_p) + osc_plane_vec.dot(c[1]);
            }
            CurvePropertyType::XNormal => {
                let c = self.eval(self.kernel, t, 2, true)?;
                // F = C'.x, F' = C''.x
                out.f = c[1].x;
                out.f_prime = c[2].x;
                if is_zero(out.f) && is_zero(out.f_prime) {
                    return Err(err());
                }
            }
            CurvePropertyType::YNormal => {
                let c = self.eval(self.kernel, t, 2, true)?;
                out.f = c[1].y;
                out.f_prime = c[2].y;
                if is_zero(out.f) && is_zero(out.f_prime) {
                    return Err(err());
                }
            }
            CurvePropertyType::ZNormal => {
                let c = self.eval(self.kernel, t, 2, true)?;
                out.f = c[1].z;
                out.f_prime = c[2].z;
                if is_zero(out.f) && is_zero(out.f_prime) {
                    return Err(err());
                }
            }
            CurvePropertyType::XyzNormal => {
                let c = self.eval(self.kernel, t, 2, true)?;
                // F = C'.x * C'.y * C'.z
                out.f = c[1].x * c[1].y * c[1].z;
                out.f_prime =
                    c[2].x * (c[1].y * c[1].z) + c[1].x * (c[2].y * c[1].z + c[1].y * c[2].z);
                if is_zero(c[1].x) && is_zero(c[2].x) {
                    out.f = c[1].y * c[1].z;
                    out.f_prime = c[2].y * c[1].z + c[1].y * c[2].z;
                    if is_zero(out.f) && is_zero(out.f_prime) {
                        return Err(err());
                    }
                }
                if is_zero(c[1].y) && is_zero(c[2].y) {
                    out.f = c[1].x * c[1].z;
                    out.f_prime = c[2].x * c[1].z + c[1].x * c[2].z;
                    if is_zero(out.f) && is_zero(out.f_prime) {
                        return Err(err());
                    }
                }
                if is_zero(c[1].z) && is_zero(c[2].z) {
                    out.f = c[1].x * c[1].y;
                    out.f_prime = c[2].x * c[1].y + c[1].x * c[2].y;
                    if is_zero(out.f) && is_zero(out.f_prime) {
                        return Err(err());
                    }
                }
            }
            CurvePropertyType::MinimizeFirstDerivative => {
                let c = self.eval(self.kernel, t, 2, false)?;
                // F = C'.C', F' = 2 C' . C''
                out.f = c[1].dot(c[1]);
                out.f_prime = 2.0 * c[1].dot(c[2]);
            }
            CurvePropertyType::MaximizeFirstDerivative => {
                let c = self.eval(self.kernel, t, 2, false)?;
                // F = 1 / ( C'.C' ), F' = - 2 C'.C'' / (C'.C')^2
                out.f = 1.0 / c[1].dot(c[1]);
                let denom = c[1].dot(c[1]) * c[1].dot(c[1]);
                if denom < EFF_ZERO_SQ {
                    return Err(err());
                }
                out.f_prime = (-2.0 * c[1].dot(c[2])) / denom;
            }
            CurvePropertyType::MinimizeRadiusOfCurvature => {
                let c = self.eval(self.kernel, t, 3, false)?;
                // F = b^3 / |A|, A = C' x C'', b = |C'|
                let b = c[1].length();
                if is_zero(b) {
                    return Err(err()); // Zero 1stDeriv point in search for MinRadOfCurvature
                }
                let numer = b * b * b;

                let a = c[1].cross(c[2]);
                let denom = a.length();
                if is_zero(denom) {
                    return Err(err()); // No curvature: infinite radius
                }
                out.f = numer / denom;

                let b_prime = c[1].dot(c[2]) / b;
                let numer_prime = 3.0 * b * b * b_prime;
                let a_prime = c[1].cross(c[3]);
                let denom_prime = a.dot(a_prime) / denom;
                out.f_prime = (numer_prime - out.f * denom_prime) / denom;
            }
            CurvePropertyType::MaximizeRadiusOfCurvature => {
                let c = self.eval(self.kernel, t, 3, false)?;
                // F = |A| / b^3
                let a = c[1].cross(c[2]);
                let numer = a.length();
                if numer < EFF_ZERO {
                    // (falls through upstream)
                    out.f = 0.0;
                    out.f_prime = 0.0;
                    out.found_answer = true;
                }

                let b = c[1].length();
                if is_zero(b) {
                    return Err(err()); // Zero first derivative error
                }
                let denom = b * b * b;

                out.f = numer / denom;

                let a_prime = c[1].cross(c[3]);
                let numer_prime = a.dot(a_prime) / numer;
                let b_prime = c[1].dot(c[2]) / b;
                let denom_prime = 3.0 * b * b * b_prime;
                out.f_prime = (numer_prime - out.f * denom_prime) / denom;
            }
            CurvePropertyType::PlaneIntersection => {
                let abc = self.checked_vec(0)?;
                let d = self.checked_value()?;
                let c = self.eval(self.kernel, t, 1, true)?;

                // Switch over to normalization if get really close
                out.f = c[0].dot(abc) + d;
                if out.f.abs() < EFF_ZERO_SQRT && c[1].dot(abc).abs() < EFF_ZERO_SQRT {
                    // F = C'.sABC, F' = C''.sABC
                    out.f = c[1].dot(abc);
                    let c2 = self.eval(self.kernel, t, 2, true)?;
                    out.f_prime = c2[2].dot(abc);
                } else {
                    // F = C.sABC + D, F' = C'.sABC
                    out.f_prime = c[1].dot(abc);
                }
                if out.f.abs() < EFF_ZERO {
                    out.found_answer = true;
                }
            }
            CurvePropertyType::CylinderIntersection => {
                if self.opt_vectors.is_none() {
                    return Err(err());
                }
                let cyl_radius = self.value()?;
                let center = self.vec(0)?;
                let n = self.vec(1)?;
                let c = self.eval(self.kernel, t, 1, true)?;
                let fp = n.cross(c[1]).cross(n);
                let cf = n.cross(c[0] - center).cross(n);
                out.f = cf.dot(cf) - cyl_radius * cyl_radius;
                out.f_prime = 2.0 * cf.dot(fp);
            }
            CurvePropertyType::MinimizeCcwAngle
            | CurvePropertyType::MinimizeAngleToPlane
            | CurvePropertyType::MinimizeDirectedAngle => {
                if self.opt_vectors.is_none() {
                    return Err(err());
                }
                let c = self.eval(self.kernel, t, 2, true)?;
                // F = (C' x N) . (C - Origin)
                let origin = self.vec(0)?;
                let n = self.vec(1)?;
                let cpxn = c[1].cross(n);
                let c_orig = c[0] - origin;
                out.f = cpxn.dot(c_orig);
                let cpxnp = c[1].derivative_of_cross_product(n, c[2], Vec3::new(0.0, 0.0, 0.0));
                out.f_prime = cpxnp.dot(c_orig) + cpxn.dot(c[1]);
            }
            CurvePropertyType::ProjectedPointMinimize
            | CurvePropertyType::ProjectedPointMaximize => {
                // evaluate current guess
                let c = self.eval(self.kernel, t, 2, true)?;
                let p = self.vec(0)?;
                let n = self.vec(1)?;

                let v = c[0] - p;
                let v_proj = n.cross(v).cross(n);
                let tan_proj = n.cross(c[1]).cross(n);
                let sec_proj = n.cross(c[2]).cross(n);

                out.f = tan_proj.dot(v_proj);
                out.f_prime = sec_proj.dot(v_proj) + tan_proj.dot(tan_proj);

                // F = unit(TanProj) . VProj (only gap or perpendicular zeros)
                let v_dot_v = v_proj.dot(v_proj);
                let v_dot_tan = tan_proj.dot(v_proj);
                let v_dot_sec = sec_proj.dot(v_proj);
                let tan_dot_tan = tan_proj.dot(tan_proj);
                let tan_dot_sec = tan_proj.dot(sec_proj);
                let tan_len = sqrt(tan_dot_tan);
                let v_len = sqrt(v_dot_v);

                if tan_len > EFF_ZERO {
                    out.f = v_dot_tan / tan_len;
                    out.f_prime = ((v_dot_sec + tan_dot_tan) * tan_len
                        - (tan_dot_sec * v_dot_tan) / tan_len)
                        / tan_dot_tan;
                } else {
                    out.f = 0.0;
                    out.f_prime = 0.0;
                }

                if self.property == CurvePropertyType::ProjectedPointMaximize {
                    out.f = -out.f; // [B543]
                }

                out.found_answer = (v_dot_v < (EFF_ZERO_SQ * 100.0))
                    || ((v_dot_tan / tan_len / v_len).abs() < EFF_ZERO);
            }
            CurvePropertyType::TangentToCircle => {
                if self.opt_vectors.is_none() {
                    return Err(err());
                }

                let c = self.eval(self.kernel, t, 2, true)?;
                let p = self.vec(0)?;
                let tan0 = self.vec(1)?;
                let diff = c[0] - p;
                let mag = c[1].length();
                if mag < EFF_ZERO {
                    return Err(err());
                }

                // Determine which case we have (ahead / behind on both tangents).
                let flip = match line_line_closest_point(p, tan0, c[0], c[1]) {
                    Ok((t0, t1)) => t0 * t1 < 0.0, // If opposite signs, flip.
                    Err(e) if e.is_unported() => return Err(e),
                    Err(_) => {
                        // parallel (or anti-parallel) tangents
                        let dot = c[1].dot(tan0);
                        dot < 0.0
                    }
                };

                // Now the possibly-flipped vector:
                let tan = if flip { -tan0 } else { tan0 };

                let tan_sum = c[1] + mag * tan;
                out.f = diff.dot(tan_sum);

                let mag_prime = c[1].dot(c[2]) / mag;
                out.f_prime = c[1].dot(tan_sum) + diff.dot(c[2] + mag_prime * tan);
            }
            CurvePropertyType::MaximaToLine => {
                // F = Dot(C',Cross(Cross(V,C-P),V))
                if self.opt_vectors.is_none() {
                    return Err(err());
                }
                let p = self.vec(0)?;
                let v = self.vec(1)?;
                let c = self.eval(self.kernel, t, 2, true)?;
                if c[1].length_squared() < EFF_ZERO {
                    return Err(err());
                }

                let norm_at_c = v.cross(v.cross(c[0] - p));
                let norm_at_c_prime = v.cross(v.cross(c[1] - p));

                out.f = norm_at_c.dot(c[1]);
                out.f_prime = norm_at_c.dot(c[2]) + norm_at_c_prime.dot(c[1]);
            }
            CurvePropertyType::MaximaToPlane => {
                // F = C' . V, F' = C'' . V
                if self.opt_vectors.is_none() {
                    return Err(err());
                }
                let v = self.vec(1)?;
                let c = self.eval(self.kernel, t, 2, true)?;
                if c[1].length_squared() < EFF_ZERO {
                    return Err(err());
                }

                out.f = v.dot(c[1]);
                out.f_prime = v.dot(c[2]);
            }
            CurvePropertyType::ProjectedTangentThroughPoint => {
                // F = ( (C-P) x C' ) . N, F' = ( (C-P) x C'' ) . N
                if self.opt_vectors.is_none() {
                    return Err(err());
                }
                let c = self.eval(self.kernel, t, 2, true)?;
                if c[1].length_squared() < EFF_ZERO {
                    return Err(err());
                }

                let gap = c[0] - self.vec(0)?;
                // Use non-unitized vectors (sbUseUnit == FALSE).
                let n = self.vec(1)?;
                let cross1 = gap.cross(c[1]);
                out.f = cross1.dot(n);
                let cross2 = gap.cross(c[2]);
                out.f_prime = cross2.dot(n);
            }
            CurvePropertyType::PlaneClassify => return Err(err()),
        }
        Ok(())
    }
}

/// The tangent curve/curve intersection equations (`SmCCTangentIntENFO`): parallel
/// unit tangents and a zero gap. Its `Evaluate` (a pass-through to
/// [`CcTangentIntEnfo::static_evaluate`]) is unit `curve.curve_p5`.
#[derive(Debug, Clone, Copy)]
pub struct CcTangentIntEnfo<'a, C1: ?Sized, C2: ?Sized> {
    /// Caller context for virtual evaluations.
    pub kernel: &'a Kernel,

    /// `m_crCurve1`.
    pub curve1: &'a C1,
    /// `m_crCurve2`.
    pub curve2: &'a C2,
    /// `m_dSquaredTolerance` (not used).
    pub squared_tolerance: f64,
}

impl<'a, C1: CurveEvaluate + ?Sized, C2: CurveEvaluate + ?Sized> CcTangentIntEnfo<'a, C1, C2> {
    /// `SmCCTangentIntENFO(crCurve1, crCurve2, dSquaredTolerance)`.
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
            squared_tolerance,
        }
    }

    /// `StaticEvaluate`: at `x = [t1, t2]`, `f = [u1.u2 - sign(u1.u2), gap.gap]` with
    /// `u` the unit tangents and `gap = C1 - C2`, and the Jacobian
    /// `[[u1'.u2, u1.u2'], [2 C1'.gap, -2 C2'.gap]]` when asked for. Returns
    /// `rbFoundAnswer` (always false).
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a (nearly) zero tangent; `InvalidInput` for `x` or `f` shorter
    /// than 2 (upstream asserts their size); those of the evaluations and of the
    /// Jacobian writes (a matrix smaller than 2x2).
    pub fn static_evaluate(
        k: &Kernel,
        curve1: &C1,
        curve2: &C2,
        x: &[f64],
        f: &mut [f64],
        jacobian: Option<&mut Matrix>,
    ) -> Result<bool> {
        let (Some(&x0), Some(&x1)) = (x.first(), x.get(1)) else {
            return Err(SolidError::InvalidInput {
                operation: OP_TAN,
                reason: "need two parameters",
            });
        };
        if f.len() < 2 {
            return Err(SolidError::InvalidInput {
                operation: OP_TAN,
                reason: "need two function values",
            });
        }

        // init output
        let found = false;

        // Curve1Point, Curve2Point positions and tangents (nonZeroTangent values)
        let pvv1: [Vec3; 3] = eval_n(k, curve1, x0, 2, true, true)?;
        let pvv2: [Vec3; 3] = eval_n(k, curve2, x1, 2, true, true)?;
        let tan_len_sq1 = pvv1[1].length_squared();
        let tan_len_sq2 = pvv2[1].length_squared();

        // if we have a zero length tangent then we must fail
        if tan_len_sq2 < EFF_ZERO || tan_len_sq1 < EFF_ZERO {
            return Err(SolidError::sm_err(OP_TAN));
        }

        let ufp = pvv1[1].unitize().map_err(|_| SolidError::sm_err(OP_TAN))?;
        let ugp = pvv2[1].unitize().map_err(|_| SolidError::sm_err(OP_TAN))?;
        let ufpp = pvv1[1].unitized_derivative(pvv1[2]);
        let ugpp = pvv2[1].unitized_derivative(pvv2[2]);
        let gap_vec = pvv1[0] - pvv2[0];

        if let Some(j) = jacobian {
            j.set_at(0, 0, ufpp.dot(ugp))?;
            j.set_at(0, 1, ufp.dot(ugpp))?;
            j.set_at(1, 0, 2.0 * pvv1[1].dot(gap_vec))?;
            j.set_at(1, 1, -2.0 * pvv2[1].dot(gap_vec))?;
        }

        // Compute function values
        let sign = if ufp.dot(ugp) > 0.0 { 1.0 } else { -1.0 };

        if let [f0, f1, ..] = f {
            *f0 = ufp.dot(ugp) - sign;
            *f1 = gap_vec.dot(gap_vec);
        }

        // all done
        Ok(found)
    }
}

/// The curve/curve intersection equations (`SmCCIntersectENFO`): the gap dotted
/// with both tangents, with optional bounds on the parameters. Its `Evaluate` is
/// unit `curve.curve_p5`.
#[derive(Debug, Clone)]
pub struct CcIntersectEnfo<'a, C1: ?Sized, C2: ?Sized> {
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

impl<'a, C1: ?Sized, C2: ?Sized> CcIntersectEnfo<'a, C1, C2> {
    /// `SmCCIntersectENFO(crCurve1, crCurve2, dSquaredTolerance)`: no intervals or
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

    /// `HasIntervals`: always true.
    #[must_use]
    pub const fn has_intervals(&self) -> bool {
        true
    }

    /// `SetIntervals`: keeps (copies of) the solver's intervals and periodicities.
    pub fn set_intervals(&mut self, intervals: Option<&ExtentNd>, periodicities: Option<&[bool]>) {
        self.intervals = intervals.cloned();
        self.periodicities = periodicities.map(<[bool]>::to_vec);
    }
}

/// The curve/curve normal (extrema) equations (`SmFindCCNormalENFO`). Its
/// `Evaluate` is unit `curve.curve_p5`.
#[derive(Debug, Clone, Copy)]
pub struct FindCcNormalEnfo<'a, C1: ?Sized, C2: ?Sized> {
    /// Caller context for virtual evaluations.
    pub kernel: &'a Kernel,

    /// `m_crCurve1`: this curve.
    pub curve1: &'a C1,
    /// `m_crCurve2`: the other curve.
    pub curve2: &'a C2,
    /// `m_dSquaredTolerance`.
    pub squared_tolerance: f64,
    /// `m_dSquaredAtDistance`.
    pub squared_at_distance: f64,
    /// `m_eSolverOperation`.
    pub solver_operation: SolverOperation,
    /// `m_cpOptVectors` (`None` for `NULL`).
    pub opt_vectors: Option<&'a [Vec3]>,
    /// `m_lCount`.
    pub count: u64,
    /// `m_bStepBack`.
    pub step_back: bool,
}

impl<'a, C1: ?Sized, C2: ?Sized> FindCcNormalEnfo<'a, C1, C2> {
    /// `SmFindCCNormalENFO(crCurve1, crCurve2, dSquaredTolerance, dSquaredAtDistance,
    /// eSolverOperation, cpOptVectors)`: count `0`, no step back.
    #[must_use]
    pub const fn new(
        k: &'a Kernel,
        curve1: &'a C1,
        curve2: &'a C2,
        squared_tolerance: f64,
        squared_at_distance: f64,
        solver_operation: SolverOperation,
        opt_vectors: Option<&'a [Vec3]>,
    ) -> Self {
        Self {
            kernel: k,
            curve1,
            curve2,
            squared_tolerance,
            squared_at_distance,
            solver_operation,
            opt_vectors,
            count: 0,
            step_back: false,
        }
    }
}

/// The point/curve extrema function (`SmFindPCExtremaEFO`): `(C(t) - P) . C'(t)`.
/// Its `Evaluate` is unit `curve.curve_p3`.
#[derive(Debug, Clone, Copy)]
pub struct FindPcExtremaEfo<'a, C: ?Sized> {
    /// Caller context for virtual evaluations.
    pub kernel: &'a Kernel,

    /// `m_crCurve`.
    pub curve: &'a C,
    /// `m_crTestPoint`.
    pub test_point: Point3,
    /// `m_sDomain`: domain of interest on the curve.
    pub domain: Extent1d,
    /// `m_dSquaredTolerance`.
    pub squared_tolerance: f64,
    /// `m_eSolverOperation` (minimise or maximise).
    pub solver_operation: SolverOperation,
    /// `m_bHaveScale`.
    pub have_scale: bool,
    /// `m_lNumScale`.
    pub num_scale: u64,
}

impl<'a, C: ?Sized> FindPcExtremaEfo<'a, C> {
    /// `SmFindPCExtremaEFO(crCurve, crTestPoint, crDomain, dSquaredTolerance,
    /// eSolverOperation)`: no scale picked yet.
    #[must_use]
    pub const fn new(
        k: &'a Kernel,
        curve: &'a C,
        test_point: Point3,
        domain: Extent1d,
        squared_tolerance: f64,
        solver_operation: SolverOperation,
    ) -> Self {
        Self {
            kernel: k,
            curve,
            test_point,
            domain,
            squared_tolerance,
            solver_operation,
            have_scale: false,
            num_scale: 0,
        }
    }
}

/// The point-at-distance equations (`SmCPDistAtENFO`): at `x = [s, cx, cy, cz]`,
/// `(C(s) - P)_i^2 - c_i` for each axis and `cx + cy + cz - d^2`.
#[derive(Debug, Clone, Copy)]
pub struct CpDistAtEnfo<'a, C: ?Sized> {
    /// Caller context for virtual evaluations.
    pub kernel: &'a Kernel,

    /// `m_crCurve`.
    pub curve: &'a C,
    /// `m_crPoint`: the test point.
    pub point: Point3,
    /// `m_dTargetDistanceSq`.
    pub target_distance_sq: f64,
    /// `m_dSquaredTolerance` (not used).
    pub squared_tolerance: f64,
}

impl<'a, C: CurveEvaluate + ?Sized> CpDistAtEnfo<'a, C> {
    /// `SmCPDistAtENFO(crCurve, crPoint, dTargetDistanceSq, dSquaredTolerance)`.
    #[must_use]
    pub const fn new(
        k: &'a Kernel,
        curve: &'a C,
        point: Point3,
        target_distance_sq: f64,
        squared_tolerance: f64,
    ) -> Self {
        Self {
            kernel: k,
            curve,
            point,
            target_distance_sq,
            squared_tolerance,
        }
    }
}

impl<C: CurveEvaluate + ?Sized> EvalNFunctions for CpDistAtEnfo<'_, C> {
    /// `Evaluate`: the four equations and, when asked for, their Jacobian
    /// `[[2X'(X-Xp), -1, 0, 0], [2Y'(Y-Yp), 0, -1, 0], [2Z'(Z-Zp), 0, 0, -1],
    /// [0, 1, 1, 1]]`; `rbFoundAnswer` is always false. `SM_ERR` for a (nearly) zero
    /// tangent; `InvalidInput` for `x` or `f` shorter than 4.
    fn evaluate(
        &mut self,
        x: &[f64],
        f: &mut [f64],
        jacobian: Option<&mut Matrix>,
    ) -> Result<bool> {
        let k = self.kernel;
        let [x0, x1, x2, x3] = match x {
            [a, b, c, d, ..] => [*a, *b, *c, *d],
            _ => {
                return Err(SolidError::InvalidInput {
                    operation: OP_DIST_AT,
                    reason: "need four unknowns",
                })
            }
        };
        let [f0, f1, f2, f3, ..] = f else {
            return Err(SolidError::InvalidInput {
                operation: OP_DIST_AT,
                reason: "need four function values",
            });
        };

        // init output
        let found = false;

        // CurvePoint position and tangent (nonZeroTangent values)
        let pv: [Vec3; 2] = eval_n(k, self.curve, x0, 1, true, true)?;
        let tan_len_sq1 = pv[1].length_squared();

        // if we have a zero length tangent then we must fail
        if tan_len_sq1 < EFF_ZERO {
            return Err(SolidError::sm_err(OP_DIST_AT));
        }

        // gap = C(s) - TestPoint
        let gap = pv[0] - self.point;

        if let Some(j) = jacobian {
            let p = self.point;
            let rows = [
                [2.0 * pv[1].x * (pv[0].x - p.x), -1.0, 0.0, 0.0],
                [2.0 * pv[1].y * (pv[0].y - p.y), 0.0, -1.0, 0.0],
                [2.0 * pv[1].z * (pv[0].z - p.z), 0.0, 0.0, -1.0],
                [0.0, 1.0, 1.0, 1.0],
            ];
            for (r, row) in rows.iter().enumerate() {
                for (c, &v) in row.iter().enumerate() {
                    j.set_at(r, c, v)?;
                }
            }
        }

        // Compute function values
        let fun_vec = gap.multiply(gap);
        *f0 = fun_vec.x - x1;
        *f1 = fun_vec.y - x2;
        *f2 = fun_vec.z - x3;
        *f3 = x1 + x2 + x3 - self.target_distance_sq;

        // all done
        Ok(found)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::hermite_curve::HermiteCurve;

    fn hermite(p1: [f64; 3], d1: [f64; 3], p2: [f64; 3], d2: [f64; 3]) -> Curve {
        Curve::from(HermiteCurve::new(
            Vec3::from_array(p1),
            Vec3::from_array(d1),
            Vec3::from_array(p2),
            Vec3::from_array(d2),
            3,
        ))
    }

    fn seg(a: [f64; 3], b: [f64; 3]) -> Curve {
        let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        hermite(a, d, b, d)
    }

    #[test]
    fn chain_orders_and_reverses_hermite_links() {
        // square edges in shuffled order (reversal of a Hermite curve fails silently)
        let curves = vec![
            Some(seg([0.0, 0.0, 0.0], [1.0, 0.0, 0.0])),
            None,
            Some(seg([1.0, 1.0, 0.0], [0.0, 1.0, 0.0])),
            Some(seg([1.0, 0.0, 0.0], [1.0, 1.0, 0.0])),
            Some(seg([5.0, 5.0, 5.0], [6.0, 5.0, 5.0])),
        ];
        let r = fix_chain_curves_list(&Kernel::default(), curves, 1e-6, true).unwrap();
        assert_eq!(r.chain_starts, vec![0, 3]);
        assert_eq!(r.origin, vec![0, 3, 2, 4]);
        assert_eq!(r.deleted, 0);
    }

    #[test]
    fn chain_deletes_duplicates() {
        let curves = vec![
            Some(seg([0.0, 0.0, 0.0], [1.0, 0.0, 0.0])),
            Some(seg([1.0, 0.0, 0.0], [0.0, 0.0, 0.0])),
        ];
        let r = fix_chain_curves_list(&Kernel::default(), curves, 1e-6, true).unwrap();
        assert_eq!(r.deleted, 1);
        assert_eq!(r.origin, vec![0]);
    }

    #[test]
    fn closed_and_degenerate() {
        let k = &Kernel::default();
        let open = seg([0.0, 0.0, 0.0], [1.0, 0.0, 0.0]);
        let ivl = open.natural_interval();
        assert!(!open.is_closed(k, &ivl, 0.0).unwrap());
        assert!(!open.is_degenerate(k, EFF_ZERO, None).unwrap());
        let loop_ = hermite([0.0; 3], [1.0, 1.0, 0.0], [0.0; 3], [1.0, -1.0, 0.0]);
        assert!(loop_.is_closed(k, &ivl, 0.0).unwrap());
        let point = hermite([1.0; 3], [0.0; 3], [1.0; 3], [0.0; 3]);
        assert!(point.is_degenerate(k, EFF_ZERO, None).unwrap());
        assert!(is_degenerate_base(k, &open, 0.0, Some(&Extent1d::from_value(0.5))).unwrap());
    }

    #[test]
    fn snap_and_property() {
        let k = &Kernel::default();
        let c = seg([0.0, 0.0, 0.0], [2.0, 0.0, 0.0]);
        assert_eq!(c.snap_to_knots(k, 0.999, 0.01).unwrap(), 1.0);
        assert_eq!(c.snap_to_knots(k, 0.5, 0.01).unwrap(), 0.5);
        let p = [Vec3::new(1.0, 1.0, 0.0)];
        let mut efo = CurvePropertyEfo::new(
            k,
            &c,
            CurvePropertyType::PointInversion,
            None,
            Some(&p),
            0.0,
        );
        let mut out = FunctionValue::default();
        efo.evaluate(0.5, &mut out, true).unwrap();
        assert!((out.f - 1.0).abs() < 1e-12);
        assert!(out.f_prime.abs() < 1e-12);
        let mut none =
            CurvePropertyEfo::new(k, &c, CurvePropertyType::PlaneClassify, None, None, 0.0);
        assert!(none.evaluate(0.5, &mut out, true).is_err());
    }

    #[test]
    fn assert_valid_owner_types() {
        let c = seg([0.0, 0.0, 0.0], [1.0, 0.0, 0.0]);
        let mut list = AssertArray::new();
        assert!(c
            .assert_valid(Some(&mut list), false, ObjectKey(1))
            .unwrap());
        assert!(list.is_empty());
        let bad = ObjectRef {
            type_code: super::super::p2::LINE_TYPE,
            key: crate::geom::attribute::ObjectKey(2),
        };
        assert!(!assert_valid_base(
            Some(bad),
            3,
            "SmHermiteCurve",
            Some(&mut list),
            false,
            ObjectKey(1)
        )
        .unwrap());
        assert_eq!(list.len(), 1);
    }

    #[test]
    fn dist_at_and_tangent_equations() {
        let k = &Kernel::default();
        let c = seg([0.0, 0.0, 0.0], [2.0, 0.0, 0.0]);
        let mut e = CpDistAtEnfo::new(k, &c, Vec3::new(1.0, 1.0, 0.0), 1.0, 0.0);
        let mut f = [0.0; 4];
        let mut j = Matrix::new(4, 4);
        e.evaluate(&[0.5, 0.0, 1.0, 0.0], &mut f, Some(&mut j))
            .unwrap();
        assert!(f.iter().all(|v| v.abs() < 1e-12));
        let d = seg([1.0, -1.0, 0.0], [1.0, 1.0, 0.0]);
        let mut f2 = [0.0; 2];
        CcTangentIntEnfo::static_evaluate(k, &c, &d, &[0.5, 0.5], &mut f2, None).unwrap();
        // perpendicular unit tangents: 0 - (-1)
        assert!((f2[0] - 1.0).abs() < 1e-12);
        assert!(f2[1].abs() < 1e-12);
    }
}
