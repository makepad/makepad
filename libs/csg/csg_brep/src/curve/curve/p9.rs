//! `SmCurve`, part 9: the local solve of the curve/curve global solver
//! (`SmCCGlobalSolver::LocalSolve`: span culling, discontinuous end point solutions
//! checked for reciprocity, and Newton solves from chord based guesses), the global
//! curve/curve solve `GlobalCurveSolve` (with the signed angle minimiser it
//! composes from other solves), the curve self intersection, the tight bounding
//! and pseudo boxes, and the plane tests `IsOnPlane` and `IsOnPositiveSideOfPlanes`.
//!
//! Upstream: `source/SMLib/src/SmCurve.cpp`, `source/SMLib/inc/SmCurve.h`.
//!
//! | Rust                                              | Upstream                                         |
//! |---------------------------------------------------|--------------------------------------------------|
//! | [`CcGlobalSolver::local_solve_branch`]            | `SmCCGlobalSolver::LocalSolve`                   |
//! | `solve_min_signed_angle` (private)                | `sm_SolveMinSignedAngle` (static)                |
//! | [`Curve::global_curve_solve`], [`CurveSolveOptions`] | `SmCurve::GlobalCurveSolve`                   |
//! | [`Curve::global_curve_self_intersect`]            | `SmCurve::GlobalCurveSelfIntersect`              |
//! | [`Curve::calculate_tight_bounding_box`]           | `CalculateTightBoundingBox` (virtual: `SmCurve`, the `SmLine` override) |
//! | [`Curve::calculate_tight_pseudo_box`], [`Curve::calculate_tight_pseudo_box_into`] | `SmCurve::CalculateTightPseudoBox` |
//! | [`Curve::is_on_plane`], [`is_on_plane_base`]      | `IsOnPlane` (virtual: `SmCurve`, the `SmBSplineCurve` override) |
//! | [`Curve::is_on_positive_side_of_planes`]          | `SmCurve::IsOnPositiveSideOfPlanes`              |
//!
//! Deviations:
//! - **Curves and kernel.** The solver's trees only name their curves by the
//!   caller's handle ([`crate::geom::tree::Tree::owner_object`]), so the solver
//!   carries the curves and the kernel ([`CcGlobalSolver::with_curves`]); a solver
//!   without them is [`SolidError::InvalidInput`]. A node without data (upstream
//!   `NER`) is `SM_ERR_NULL_POINTER`; a node that is not a leaf span is
//!   `InvalidInput` (upstream casts whatever `m_pData` holds). Solutions name the
//!   trees' owner handles (upstream the curves).
//! - **Objects and caches.** `GlobalCurveSolve` builds the curves' caches for the
//!   call with [`make_curve_cache`] (unit `curve.curve_p5`), with the handles of
//!   [`CurveSolveOptions::handles`] as the trees' owners; the same curve (by
//!   identity) shares one cache, as upstream's object cache. A cache that cannot be
//!   built is `SM_ERR_NULL_POINTER` (upstream `NER`). `sm_SolveMinSignedAngle` reads
//!   the continuities of those same caches and its nested `GlobalCurveSolve` builds
//!   its own (upstream finds the cached ones; the result is the same). The internal
//!   solves of the tight boxes and plane tests name a private handle carrying the
//!   curve's natural interval.
//! - **Out-params.** Outputs are returned: a [`SolutionArray`], an [`Extent3d`], a
//!   [`PseudoBox`] (or filled in place, [`Curve::calculate_tight_pseudo_box_into`],
//!   which `IsOnPlane` uses because it keeps the partial box of a failed call) and
//!   an [`OnPlane`] (whose `max_dist` is `None` where upstream leaves
//!   `rdMaxDistToPlaneArg` unset or `SM_UNDEF_DOUBLE`).
//! - **Persistent solutions.** `LocalSolve` reuses one point solution across the
//!   discontinuous end points: [`Curve::local_point_solve`] returns a fresh
//!   solution, merged into the persistent one as upstream writes it (type, value and
//!   first parameter when an answer is found; the whole solution for the ray
//!   operations); `LocalPropertyAnalysis` writes into it in place.
//! - **Ignored statuses.** Where upstream ignores a status (the span end
//!   evaluations, the end point `LocalPropertyAnalysis` / `LocalPointSolve`, the
//!   guess `LocalCurveSolve`s and the pivot guess `GlobalPropertyAnalysis` of
//!   `LocalSolve`; `SolveIt` and `CalculateBoundingBox` of `GlobalCurveSolve`; the
//!   evaluations of the tight boxes; `MakeUnitOrthoVectors` and
//!   `CalculateTightPseudoBox` of `IsOnPlane`; the property analyses of
//!   `IsOnPositiveSideOfPlanes`) the error is ignored and upstream's value kept (no
//!   answer, no solutions, the previous point or [`Vec3::UNDEF`] for a never
//!   written point, the default box, the in-place `Unitize` result), except
//!   [`SolidError::Unported`], which is always propagated. `sm_GetProjDist` returns
//!   a failing status as the distance upstream; that status is the solution value
//!   here too.
//! - **Undefined behaviour.** Option vectors an operation reads without a `NULL`
//!   check, a missing target distance for `SM_SO_AT_DISTANCE` (upstream `NER`) and
//!   plane normal arrays shorter than the point arrays are errors; knot arrays
//!   shorter than the cache's continuities in `sm_SolveMinSignedAngle` are
//!   `InvalidInput`.
//! - **Virtual `IsOnPlane`.** The `SmBSplineCurve` override runs
//!   [`crate::nurbs::b_spline_curve::BSplineCurve::is_on_plane`] for a plain
//!   B-spline; for the derived kinds (ellipse, hyperbola, parabola, line) it is
//!   repeated here with the kind's own `EvaluatePoint` (upstream's virtual call) and
//!   `IsKindOf(SmLine_TYPE)` test, the control polygon coming from the B-spline
//!   part.
//! - **Opt-in fix.** `CalculateTightBoundingBox` adds only the knots strictly inside
//!   the interval with `Kernel::fixes.curve_tight_box_interval_knots` (WR-474, #1054);
//!   off by default, as upstream.
//! - `SM_ASSERT`s and the `SM_DEBUG_CODE` blocks are not ported. `GlobalCurveSolve`
//!   recurses once at most (the signed angle minimiser asks for projected
//!   tangencies), so it needs no depth bound.

use crate::curve::curve::p2::{
    proj_signed_angle, CcGlobalSolver, CciGlobalSolver, Curve, CurveKind, CurvePropertyType,
    BSPLINE_CURVE_TYPE, LINE_TYPE,
};
use crate::curve::curve::p3::local_curve_intersect;
use crate::curve::curve::p3::PointSolveOptions;
use crate::curve::curve::p4::{point_or_undef, proj_dist};
use crate::curve::curve::p5::{local_property_analysis, make_curve_cache};
use crate::curve::curve::p7::PropertyAnalysisOptions;
use crate::curve::curve::p8::{operand_handles, CurveIntersectOptions};
use crate::curve::curve_cache::CurveCache;
use crate::error::{ignore_status, ignored, Result, SolidError};
use crate::geom::extent1d::Extent1d;
use crate::geom::extent3d::Extent3d;
use crate::geom::geom_utility::p1::{
    line_line_closest_point, point_project_to_plane, segment_point_angle_deg_to_perp,
};
use crate::geom::geom_utility::p2::{
    line_line_at_distance, segment_segment_closest_point, segment_segment_perspective_intersect,
    segment_segment_projected_intersect, ContinuityType, PerspectivePlane,
};
use crate::geom::global_solver::{
    Branch, GlobalSolve, SolutionRequested, SolveRequest, SolverOperation,
};
use crate::geom::pseudo_box::{Projection, PseudoBox};
use crate::geom::solution_array::{
    ObjectKind, Solution, SolutionArray, SolutionEnd, SolutionObject, SolutionType, SortKey,
};
use crate::geom::tol::are_same;
use crate::geom::tree::{NodeData, Tree};
use crate::geom::vector3d::{sqrt, Point3, Vec3};
use crate::kernel::Kernel;
use crate::math::{max2, min2, BIG_DOUBLE, EFF_ZERO, EFF_ZERO_SQ, EFF_ZERO_SQRT, PI};
use crate::nurbs::b_spline_curve::{BSplineCurve, BoundingBoxRequest, OnPlane, ProjectionType};

const OP_LOCAL: &str = "curve::curve::p9::CcGlobalSolver::local_solve_branch";
const OP_MSA: &str = "curve::curve::p9::solve_min_signed_angle";
const OP_GCS: &str = "curve::curve::p9::Curve::global_curve_solve";
const OP_POS: &str = "curve::curve::p9::Curve::is_on_positive_side_of_planes";

/// `SM_NUM_PARAM_GUESS`: most guess pairs `LocalSolve` collects.
const NUM_PARAM_GUESS: usize = 40;

// ---- helpers ----------------------------------------------------------------------------

/// The handle of a curve inside this unit's own solves (upstream `this`): a curve with
/// its natural interval.
#[derive(Debug, Clone, Copy, PartialEq)]
struct CurveHandle(Extent1d);

impl SolutionObject for CurveHandle {
    fn kind(&self) -> ObjectKind {
        ObjectKind::Curve
    }

    fn natural_interval(&self) -> Option<Extent1d> {
        Some(self.0)
    }
}

/// The ordinal of a continuity (`SmContinuityType` compares as an integer).
const fn ord(c: ContinuityType) -> i32 {
    c as i32
}

/// `EvaluatePoint` whose status upstream ignores into a reused point: `pt` keeps its
/// previous value on failure; [`SolidError::Unported`] is propagated.
fn eval_keep(k: &Kernel, curve: &Curve, t: f64, pt: &mut Point3) -> Result<()> {
    if let Some(p) = ignored(curve.evaluate_point(k, t))? {
        *pt = p;
    }
    Ok(())
}

/// Sets parameter `i` of a solution end (`m_vStart[i] = v`).
fn set_param(end: &mut SolutionEnd, i: usize, v: f64) -> Result<()> {
    *end.param_mut(i)? = v;
    Ok(())
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

/// Merges a [`Curve::local_point_solve`] result into the persistent point solution
/// of `LocalSolve` the way upstream writes its `rSolution`.
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

/// Whether a curve tree node has data (`m_pData != NULL`).
const fn has_data<O>(node: &crate::geom::tree::TreeNode<O>) -> bool {
    !matches!(node.data, NodeData::None)
}

// ---- SmCCGlobalSolver::LocalSolve -------------------------------------------------------

/// The end point solve of `LocalSolve` (`LocalPropertyAnalysis` or `LocalPointSolve`
/// of `curve` over `ivl` for `point`): whether an answer was found, the answer
/// written into `solution`.
struct EndSolve<'v> {
    /// `bDoLocalProperty`.
    do_local_property: bool,
    /// `bProjected`.
    projected: bool,
    /// `eCurveProp`.
    curve_prop: CurvePropertyType,
    /// The solver operation.
    op: SolverOperation,
    /// `m_d3dTolerance`.
    tol: f64,
    /// `m_cpOptVectors` (`None` = null).
    opt_vectors: Option<&'v [Vec3]>,
}

impl EndSolve<'_> {
    /// One solve; `vectors` (`saVectors`) is updated as upstream does for the
    /// projected operations. Errors other than [`SolidError::Unported`] are no answer
    /// (statuses ignored upstream, which initialises the answer to `FALSE`).
    fn solve<O: Copy>(
        &self,
        k: &Kernel,
        curve: &Curve,
        ivl: &Extent1d,
        point: Point3,
        vectors: &mut [Vec3; 2],
        solution: &mut Solution<O>,
    ) -> Result<bool> {
        if self.do_local_property {
            let property_value = if self.projected {
                vectors[0] = point;
                0.0
            } else if self.op == SolverOperation::SignedPivotMinimize {
                let v = vectors[1].cross(point - vectors[0]).cross(vectors[1]);
                v.length() // Radius of an infinite cylinder
            } else {
                -vectors[0].dot(point)
            };
            match local_property_analysis(
                k,
                curve,
                ivl,
                self.curve_prop,
                ivl.evaluate(0.5),
                Some(property_value),
                vectors,
                solution,
            ) {
                Ok(f) => Ok(f),
                Err(e) if e.is_unported() => Err(e),
                Err(_) => Ok(false),
            }
        } else {
            let opts = PointSolveOptions {
                distance_tolerance: Some(self.tol),
                target_distance: None,
                vectors: self.opt_vectors,
            };
            match curve.local_point_solve::<O>(k, ivl, self.op, point, &opts, ivl.evaluate(0.5)) {
                Ok((f, s)) => {
                    merge_point_solution(solution, f, &s, self.op);
                    Ok(f)
                }
                Err(e) if e.is_unported() => Err(e),
                Err(_) => Ok(false),
            }
        }
    }
}

impl<'t, O: SolutionObject + 't> CcGlobalSolver<'t, O> {
    /// Option vector `i` (`m_cpOptVectors[i]`, read without a `NULL` check).
    fn opt(&self, i: usize) -> Result<Vec3> {
        self.base.opt_vector(i)
    }

    /// `SmCCGlobalSolver::LocalSolve`: adds the curve/curve solutions of the leaf
    /// span pair `branch` (spans clipped to the solver intervals) to the solver's
    /// solutions. Pairs are culled by their pseudo boxes (the minimum and the 3D
    /// signed directed minimum, which also intersects the spans and tightens the
    /// best answer from the span end distances; the maximum only tightens it) and,
    /// for spans turning less than 90 degrees, by chord line tests ruling out common
    /// normals. For the minimising and maximising operations the discontinuous span
    /// ends are solved against the other span and kept when the reverse solve finds
    /// the end again. Finally `LocalCurveSolve` runs from guesses at the span
    /// midpoints (plus the chord closest points for the minimum, the chord points at
    /// the target distance, the cylinder intersections for the signed pivot
    /// minimum). Never asks for more subdivision.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` for a node without data; `InvalidInput` for a solver
    /// without curves, a node that is not a leaf span or a missing option vector;
    /// `SM_ERR` past `NUM_PARAM_GUESS` guesses; those of the interval clipping,
    /// angles, `LocalCurveIntersect`, the end point evaluations and the solution
    /// accumulation.
    pub fn local_solve_branch(&mut self, branch: &Branch) -> Result<bool> {
        type Op = SolverOperation;

        // init output
        let needs_more_subdivision = false;

        // node and bezier curve locals
        let curve_node1 = self.base.branch_node(branch, 0)?;
        let curve_node2 = self.base.branch_node(branch, 1)?;
        if !has_data(curve_node1) || !has_data(curve_node2) {
            return Err(SolidError::null_pointer(OP_LOCAL));
        }
        let tree1: &Tree<O> = self.base.tree(0)?;
        let tree2: &Tree<O> = self.base.tree(1)?;
        let bez_span1 = *tree1.curve_span(curve_node1)?;
        let bez_span2 = *tree2.curve_span(curve_node2)?;

        // get original curves and their handles
        let [curve1, curve2] = self
            .curves
            .ok_or(SolidError::invalid_input(OP_LOCAL, "solver without curves"))?;
        let k: &Kernel = self
            .kernel
            .ok_or(SolidError::invalid_input(OP_LOCAL, "solver without kernel"))?;
        let handles = [tree1.owner_object()?, tree2.owner_object()?];
        let op = self.base.operation;
        let tol = self.base.tolerance_3d;
        let interval1 = self.interval1;
        let interval2 = self.interval2;
        let opt_vectors = self.base.opt_vectors.clone();
        let opt_slice: Option<&[Vec3]> = if opt_vectors.is_empty() {
            None
        } else {
            Some(&opt_vectors)
        };

        // get node curve intervals
        let mut ivl1 = *bez_span1.interval();
        let mut ivl2 = *bez_span2.interval();

        // skip disjoint intervals
        if interval1.are_disjoint(&ivl1, 0.0) {
            return Ok(needs_more_subdivision);
        }
        if interval2.are_disjoint(&ivl2, 0.0) {
            return Ok(needs_more_subdivision);
        }

        // get node span endPoint continuities
        let mut conts1 = bez_span1.aux.conts;
        let mut conts2 = bez_span2.aux.conts;

        // set node1 Interval ends that extend beyond the processing intervals to discontinuous
        if ivl1.min() <= interval1.min() {
            conts1[0] = ContinuityType::Discontinuous;
        }
        if ivl1.max() >= interval1.max() {
            conts1[1] = ContinuityType::Discontinuous;
        }

        // clip node1 interval to curve1 process interval - quit for zero length intervals
        if !ivl1.is_contained_by(&interval1, EFF_ZERO) {
            ivl1 = ivl1.intersect(&interval1)?;
            if are_same(ivl1.max(), ivl1.min()) {
                return Ok(needs_more_subdivision);
            }
        }

        // set node2 Interval ends that extend beyond the processing intervals to discontinuous
        if ivl2.min() <= interval2.min() {
            conts2[0] = ContinuityType::Discontinuous;
        }
        if ivl2.max() >= interval2.max() {
            conts2[1] = ContinuityType::Discontinuous;
        }

        // clip node2 interval to curve2 process interval - quit for zero length intervals
        if !ivl2.is_contained_by(&interval2, EFF_ZERO) {
            ivl2 = ivl2.intersect(&interval2)?;
            if are_same(ivl2.max(), ivl2.min()) {
                return Ok(needs_more_subdivision);
            }
        }

        // get node1 span endPoints and endPoint chords (statuses ignored)
        let start1 = point_or_undef(k, curve1, ivl1.min())?;
        let mut end1 = point_or_undef(k, curve1, ivl1.max())?;
        let line_vec1 = end1 - start1;

        // get node2 span endPoints and endPoint chords (statuses ignored)
        let mut start2 = point_or_undef(k, curve2, ivl2.min())?;
        let mut end2 = point_or_undef(k, curve2, ivl2.max())?;
        let line_vec2 = end2 - start2;

        let turn1 = bez_span1.aux.max_turning_angle_deg;
        let turn2 = bez_span2.aux.max_turning_angle_deg;

        // for SM_SO_3D_SIGNED_DIRECTED_MINIMIZE operations
        if op == Op::Signed3dDirectedMinimize {
            // Do a tight bounding box test to see if they are disjoint
            // in projected space.
            let mut tmp_pbox = *bez_span1.pseudo_box();
            tmp_pbox.expand_absolute(tol);
            let proj_point = tmp_pbox.evaluate(0.5, 0.5, 0.5);
            let proj = Projection::Parallel {
                point: proj_point,
                normal: self.opt(0)?,
            };
            if tmp_pbox.are_project_disjoint(bez_span2.pseudo_box(), &proj) {
                return Ok(needs_more_subdivision);
            }
        }

        // for SM_SO_MAXIMIZE operations - find max dist between span endPoints
        if op == Op::Maximize {
            // If we are maximizing then update the best answer based on endPoint distances
            let dist_ss = (start1 - start2).length_squared();
            let dist_se = (start1 - end2).length_squared();
            let dist_es = (end1 - start2).length_squared();
            let dist_ee = (end1 - end2).length_squared();

            let mut max_dist = max2(dist_ss, dist_se);
            max_dist = max2(max_dist, dist_es);
            max_dist = max2(max_dist, dist_ee);

            if max_dist > self.base.best_answer_so_far_sq {
                self.base.best_answer_so_far_sq = max_dist;
            }
        }

        if op == Op::Minimize {
            // If we are minimizing we should try an intersection if they intersect

            // get span pseudoBoxes increased by tolerance intersect
            let mut tmp_pbox0 = *bez_span1.pseudo_box();
            tmp_pbox0.expand_absolute(tol);
            if !tmp_pbox0.are_disjoint(bez_span2.pseudo_box(), None) {
                // Have possibility of an intersection - find a guess point
                // (Just take a midpoint if this fails due to parallel chords)
                let (t1, t2) = line_line_closest_point(start1, line_vec1, start2, line_vec2)
                    .unwrap_or((0.5, 0.5));
                let norm_ivl = Extent1d::from_min_max(0.0, 1.0);
                let guess1 = ivl1.evaluate(norm_ivl.clamp_value(t1));
                let guess2 = ivl2.evaluate(norm_ivl.clamp_value(t2));

                // look for a nodeSpan/nodeSpan intersection
                let hit =
                    local_curve_intersect(k, curve1, &ivl1, curve2, &ivl2, tol, guess1, guess2)?;

                // save solutions
                if let Some(h) = hit {
                    let mut solution: Solution<O> = Solution::new();
                    solution.solution_type = SolutionType::SingleValue;
                    solution.nodes[0] = branch[0];
                    solution.nodes[1] = branch[1];
                    solution.num_variables = 2;
                    solution.start.solution_value = h.deviation;
                    solution.start.parameters[0] = h.my_param;
                    solution.start.parameters[1] = h.other_param;
                    solution.num_objects = 2;
                    solution.objects[0] = handles[0];
                    solution.objects[1] = handles[1];

                    // add solution to the SmGlobalSolver::m_pSolutions list
                    self.add_sorted_solution(&solution, SortKey::ByValueWithParameter, true)?;
                }
            }

            // If we are minimizing then update the best answer based on endPoint distances
            let dist_ss = (start1 - start2).length_squared();
            let dist_se = (start1 - end2).length_squared();
            let dist_es = (end1 - start2).length_squared();
            let dist_ee = (end1 - end2).length_squared();

            let mut min_dist = min2(dist_ss, dist_se);
            min_dist = min2(min_dist, dist_es);
            min_dist = min2(min_dist, dist_ee);

            if min_dist < self.base.best_answer_so_far_sq {
                self.base.best_answer_so_far_sq = min_dist;
            }

            // Cull disjoint pseudo boxes after expanding each pseudo box by
            // 1/2 the tolerance plus the current best minimum distance.
            let mut tmp_pbox1 = *bez_span1.pseudo_box();
            let mut tmp_pbox2 = *bez_span2.pseudo_box();
            tmp_pbox1.expand_absolute((sqrt(self.base.best_answer_so_far_sq) + tol) / 2.0);
            tmp_pbox2.expand_absolute((sqrt(self.base.best_answer_so_far_sq) + tol) / 2.0);
            if tmp_pbox1.are_disjoint(&tmp_pbox2, None) {
                return Ok(needs_more_subdivision);
            }
        }

        // decide when to check solutions to see if they are normal to curve
        let mut do_line_test = false;

        // when turning angles are less than 90 and we are looking for min/max solutions
        if turn1 < 90.0 || turn2 < 90.0 {
            // always do LineTest for Normalize;
            // do LineTest for min/max operations on continuous boundaries
            if op == Op::Normalize
                || matches!(
                    op,
                    Op::Minimize
                        | Op::Maximize
                        | Op::SignedDirectedMinimize
                        | Op::DirectedMinimize
                        | Op::DirectedMaximize
                        | Op::ProjectedMinimize
                        | Op::ProjectedMaximize
                ) && ord(conts1[0]) > ord(ContinuityType::C0)
                    && ord(conts1[1]) > ord(ContinuityType::C0)
                    && ord(conts2[0]) > ord(ContinuityType::C0)
                    && ord(conts2[1]) > ord(ContinuityType::C0)
            {
                do_line_test = true;
            }
        }

        // when asked reject cases when solution with gapVector perp to both curves is not possible
        if do_line_test {
            // when span chords crossing-XSect to SM_EFF_ZERO
            if let Ok((t1, t2)) = line_line_closest_point(start1, line_vec1, start2, line_vec2) {
                // get intersection points and gap
                let p1 = start1 + t1 * line_vec1;
                let p2 = start2 + t2 * line_vec2;
                let vec = p1 - p2;

                // when the gap is zero
                if vec.length_squared() < EFF_ZERO_SQ {
                    // get angle between chordLines
                    let ang = line_vec1.angle_between(line_vec2)?;
                    let mut ang_deg = ang * 180.0 / PI;
                    if ang_deg > 90.0 {
                        ang_deg = 180.0 - ang_deg;
                    }

                    // when angle is larger than combined turningAngle - no solution possible
                    let combined_turning_angle = turn1 + turn2;
                    if ang_deg * 0.99 - EFF_ZERO > combined_turning_angle {
                        return Ok(needs_more_subdivision); // No answer found
                    }
                }
            }
        }

        let directed = matches!(
            op,
            Op::SignedDirectedMinimize | Op::DirectedMinimize | Op::DirectedMaximize
        );

        // For directed min/max operations
        // do line test on vectors projected to plane.
        if do_line_test && directed {
            // project all endPoints to plane [sStart1,m_cpOptVectors[0]]
            let normal = self.opt(0)?;
            end1 = point_project_to_plane(end1, start1, normal);
            start2 = point_project_to_plane(start2, start1, normal);
            end2 = point_project_to_plane(end2, start1, normal);

            let v1 = end1 - start1;
            let v2 = end2 - start2;
            if v1.length_squared() > EFF_ZERO_SQ && v2.length_squared() > EFF_ZERO_SQ {
                let ang = v1.angle_between(v2)?;
                let mut ang_deg = ang * 180.0 / PI;
                if ang_deg > 90.0 {
                    ang_deg = 180.0 - ang_deg;
                }
                if ang_deg * 0.99 - EFF_ZERO > turn1 + turn2 {
                    return Ok(needs_more_subdivision); // No answer found
                }
            }
        }

        // Note - Here we could do the line test for the projected
        // minimization if we projected all of the points into a plane.
        if do_line_test && (op == Op::ProjectedMinimize || op == Op::ProjectedMaximize) {
            // All we need to do is to project all points to plane defined by sStart1
            // and let the normal line test take care of it.
            let normal = self.opt(0)?;
            end1 = point_project_to_plane(end1, start1, normal);
            start2 = point_project_to_plane(start2, start1, normal);
            end2 = point_project_to_plane(end2, start1, normal);
        }

        // for nonDirected Minimizes - cull candidates falling outside of the segment angles
        if do_line_test && !directed {
            // First Test to see if segment 2 is outside of valid angle of segment1
            if turn1 < 90.0 {
                let (ang1, t1) = segment_point_angle_deg_to_perp(start1, end1, start2);
                let (ang2, t2) = segment_point_angle_deg_to_perp(start1, end1, end2);

                if min2(ang1, ang2) > turn1 && (t1 - t2).abs() < EFF_ZERO {
                    return Ok(needs_more_subdivision);
                }
            }

            // Now test to see if segment 1 is outside of valid angle of segment 2
            if turn2 < 90.0 {
                let (ang1, t1) = segment_point_angle_deg_to_perp(start2, end2, start1);
                let (ang2, t2) = segment_point_angle_deg_to_perp(start2, end2, end1);

                if min2(ang1, ang2) > turn2 && (t1 - t2).abs() < EFF_ZERO {
                    return Ok(needs_more_subdivision);
                }
            }
        }

        // for min/max operations - check solutions at discontinuous boundaries
        if matches!(
            op,
            Op::Minimize
                | Op::Maximize
                | Op::Rayfire
                | Op::Signed3dDirectedMinimize
                | Op::SignedDirectedMinimize
                | Op::DirectedMinimize
                | Op::DirectedMaximize
                | Op::ProjectedMinimize
                | Op::ProjectedMaximize
                | Op::SignedPivotMinimize
        ) {
            // locals
            let mut end_solve = EndSolve {
                do_local_property: false,
                projected: false,
                curve_prop: CurvePropertyType::PerpendicularToVector,
                op,
                tol,
                opt_vectors: opt_slice,
            };
            let mut vectors = [Vec3::UNDEF; 2];
            let mut direction = Vec3::new(0.0, 0.0, 0.0);

            // set special case switch values
            match op {
                Op::SignedDirectedMinimize | Op::DirectedMinimize | Op::DirectedMaximize => {
                    end_solve.do_local_property = true;
                    end_solve.curve_prop = CurvePropertyType::PlaneIntersection;
                    vectors[0] = self.opt(1)?.cross(self.opt(0)?);
                    vectors[1] = self.opt(0)?;
                    direction = self.opt(1)?;
                    vectors[0] = vectors[0].unitize()?;
                }
                Op::SignedPivotMinimize => {
                    end_solve.do_local_property = true;
                    end_solve.curve_prop = CurvePropertyType::CylinderIntersection;
                    vectors[0] = self.opt(1)?; // Pivot
                    vectors[1] = self.opt(0)?; // Normal of projection plane
                    direction = self.opt(2)?; // 'Side' of Pivot pt where solutions can be taken
                }
                Op::ProjectedMinimize | Op::ProjectedMaximize => {
                    end_solve.projected = true;
                    end_solve.do_local_property = true;
                    end_solve.curve_prop = if op == Op::ProjectedMaximize {
                        CurvePropertyType::ProjectedPointMaximize
                    } else {
                        CurvePropertyType::ProjectedPointMinimize
                    };
                    vectors[1] = self.opt(0)?;
                }
                _ => {}
            }

            // discontinuous endPoint solution locals
            let mut point_solution: Solution<O> = Solution::new();
            let mut solution: Solution<O> = Solution::new();
            solution.num_variables = 2;
            solution.solution_type = SolutionType::SingleValue;
            solution.nodes[0] = branch[0];
            solution.nodes[1] = branch[1];

            // If we have a total discontinuity (i.e. end of curve)
            // then we always test.  If we have a derivative
            // discontinuity then we just test the ones on the left
            // side - otherwise we would do each point two times.

            // for both NodeSegments
            for ii in 0..2_usize {
                let (this_curve, this_ivl, other_curve, other_ivl) = if ii == 0 {
                    (curve1, ivl1, curve2, ivl2)
                } else {
                    (curve2, ivl2, curve1, ivl1)
                };

                // for both Segment EndPoints
                for jj in 0..2_usize {
                    let (end_discontinuity, end_point, ivl_bound) = match ii * 2 + jj {
                        // Node1 Segment Start
                        0 => (
                            ord(conts1[0]) <= ord(ContinuityType::C0),
                            start1,
                            ivl1.min(),
                        ),
                        // Node1 Segment End
                        1 => (
                            ord(conts1[1]) <= ord(ContinuityType::Discontinuous),
                            end1,
                            ivl1.max(),
                        ),
                        // Node2 Segment Start
                        2 => (
                            ord(conts2[0]) <= ord(ContinuityType::C0),
                            start2,
                            ivl2.min(),
                        ),
                        // Node2 Segment End
                        _ => (
                            ord(conts2[1]) <= ord(ContinuityType::Discontinuous),
                            end2,
                            ivl2.max(),
                        ),
                    };

                    // when NodeSegment end is discontinuous - check EndPoint/OtherSegment distance
                    if !end_discontinuity {
                        continue;
                    }
                    let mut found_answer = end_solve.solve(
                        k,
                        other_curve,
                        &other_ivl,
                        end_point,
                        &mut vectors,
                        &mut point_solution,
                    )?;

                    // to avoid dangling solutions - only add solutions where
                    // solving for the OtherSolutionPoint back to thisSegment finds the
                    // Segment endPoint
                    if found_answer {
                        // get the OtherSolvePoint
                        let mut point_reciprocal_solution: Solution<O> = Solution::new();
                        let sol_pt =
                            other_curve.evaluate_point(k, point_solution.start.parameters[0])?;

                        let found_reciprocal_answer = end_solve.solve(
                            k,
                            this_curve,
                            &this_ivl,
                            sol_pt,
                            &mut vectors,
                            &mut point_reciprocal_solution,
                        )?;

                        // Use 1stSolution only when when a reciprocal solution
                        // was found and ThisReciprocalSolution finds the same
                        // point as the discontinuity boundary.
                        // If the curve is degenerate, though, we can't trust
                        // the return parameter, it's just the same as the input.
                        found_answer = found_reciprocal_answer;
                        if !this_curve.is_degenerate(k, EFF_ZERO, None)? {
                            let reciprocal_param = point_reciprocal_solution.start.parameters[0];
                            found_answer &= are_same(reciprocal_param, ivl_bound);
                        }
                    }

                    // when a found solution passed the reciprocity test - save it
                    if found_answer {
                        // save correctly signed solution
                        solution.start.solution_value =
                            if ii == 1 && op == Op::Signed3dDirectedMinimize {
                                -point_solution.start.solution_value
                            } else {
                                point_solution.start.solution_value
                            };
                        if op == Op::SignedPivotMinimize {
                            let sol_pt = other_curve
                                .evaluate_point(k, point_solution.start.parameters[0])?;
                            solution.start.solution_value = proj_signed_angle(
                                if ii == 0 { end_point } else { sol_pt },
                                if ii == 0 { sol_pt } else { end_point },
                                vectors[0], // PIVOT
                                vectors[1], // NORMAL
                                direction,  // SIDE
                            )?;
                        } else if self.base.projected_operation {
                            solution.start.solution_value = match proj_dist(
                                k,
                                other_curve,
                                point_solution.start.parameters[0],
                                end_point,
                                vectors[1],
                                true,
                                direction,
                                op,
                            ) {
                                Ok(d) => d,
                                Err(e) if e.is_unported() => return Err(e),
                                Err(e) => status_value(&e),
                            };
                        }
                        set_param(&mut solution.start, ii, ivl_bound)?;
                        set_param(
                            &mut solution.start,
                            1 - ii,
                            point_solution.start.parameters[0],
                        )?;
                        solution.nodes[0] = branch[0];
                        solution.nodes[1] = branch[1];
                        solution.num_objects = 2;
                        solution.objects[0] = handles[0];
                        solution.objects[1] = handles[1];

                        // add solution to the SmGlobalSolver::m_pSolutions list
                        self.add_sorted_solution(&solution, SortKey::ByValueWithParameter, true)?;
                    }
                }
            }
        }

        let mut guesses1: Vec<f64> = Vec::new();
        let mut guesses2: Vec<f64> = Vec::new();

        // If we are doing a distance-at function then do a real smart
        // job of getting a very good guess point because it needs to be very
        // close to converge properly.  Without this, things which are linear
        // are just too big and don't converge well.
        if op == Op::AtDistance {
            if let Ok((params1, params2)) =
                line_line_at_distance(start1, line_vec1, start2, line_vec2, self.base.at_distance)
            {
                let norm = Extent1d::from_min_max(0.0, 1.0);
                for (&p1, &p2) in params1.iter().zip(&params2) {
                    let t1 = norm.clamp_value(p1);
                    let t2 = norm.clamp_value(p2);
                    let found = guesses1
                        .iter()
                        .zip(&guesses2)
                        .any(|(&g1, &g2)| are_same(g1, t1) && are_same(g2, t2));
                    if !found {
                        guesses1.push(t1);
                        guesses2.push(t2);
                    }
                }
                // Convert to Bezier parameterization
                for g in &mut guesses1 {
                    *g = ivl1.evaluate(*g);
                }
                for g in &mut guesses2 {
                    *g = ivl2.evaluate(*g);
                }
            }
        }

        // For minimization try taking the closest point between the two lines
        // to get a better initial guess.
        if guesses1.is_empty() && op == Op::Minimize {
            let hits = segment_segment_closest_point(start1, end1, start2, end2, tol)?;
            for (&t1, &t2) in hits.params1.iter().zip(&hits.params2).take(hits.count) {
                guesses1.push(ivl1.evaluate(t1));
                guesses2.push(ivl2.evaluate(t2));
            }
            guesses1.push(ivl1.evaluate(0.5));
            guesses2.push(ivl2.evaluate(0.5));
        }

        // default guess = curve midPoints
        if guesses1.is_empty() {
            // Take midpoint of interval for guess.
            guesses1.push(ivl1.evaluate(0.5));
            guesses2.push(ivl2.evaluate(0.5));
        }

        // for operation SM_SO_SIGNED_PIVOT_MINIMIZE
        if op == Op::SignedPivotMinimize {
            // Try to get a better guess point for this operation.
            let vectors = [self.opt(1)?, self.opt(0)?]; // Pivot, Normal of projection plane
            for (this_is_1, start, line_vec) in
                [(false, start1, line_vec1), (true, start2, line_vec2)]
            {
                let test_p1 = start + 0.5 * line_vec;
                let v = vectors[1].cross(test_p1 - vectors[0]).cross(vectors[1]);
                let property_value = v.length(); // Radius of an infinite cylinder
                let (target_curve, target_ivl, handle) = if this_is_1 {
                    (curve1, &ivl1, handles[0])
                } else {
                    (curve2, &ivl2, handles[1])
                };
                let opts = PropertyAnalysisOptions {
                    property_value: Some(property_value),
                    opt_vectors: &vectors,
                    want_comb: false,
                    handle,
                };
                let sols = ignored(target_curve.global_property_analysis(
                    k,
                    target_ivl,
                    CurvePropertyType::CylinderIntersection,
                    tol,
                    &opts,
                ))?
                .map(|r| r.solutions)
                .unwrap_or_default();
                for s in sols.as_slice() {
                    if this_is_1 {
                        guesses1.push(s.start.parameters[0]);
                        guesses2.push(ivl2.evaluate(0.5));
                    } else {
                        guesses1.push(ivl1.evaluate(0.5));
                        guesses2.push(s.start.parameters[0]);
                    }
                    if guesses1.len() == NUM_PARAM_GUESS {
                        return Err(SolidError::sm_err(OP_LOCAL));
                    }
                }
            }
        }

        // for every guess point
        let local_solver_op = op;
        for (&g1, &g2) in guesses1.iter().zip(&guesses2) {
            // call LocalCurveSolve (status ignored)
            let found = match curve1.local_curve_solve::<O>(
                k,
                &ivl1,
                curve2,
                &ivl2,
                local_solver_op,
                tol,
                Some(self.base.at_distance),
                opt_slice,
                [g1, g2],
                handles,
            ) {
                Ok((true, s)) => Some(s),
                Err(e) if e.is_unported() => return Err(e),
                Ok((false, _)) | Err(_) => None,
            };

            // when a solution was found
            if let Some(mut solution) = found {
                solution.nodes[0] = branch[0];
                solution.nodes[1] = branch[1];
                solution.num_variables = 2;
                solution.num_objects = 2;
                solution.objects[0] = handles[0];
                solution.objects[1] = handles[1];

                // Add a solution to the SmGlobalSolver::m_pSolutions list
                self.add_sorted_solution(&solution, SortKey::ByValueWithParameter, true)?;
            }
        }

        Ok(needs_more_subdivision)
    }
}

// ---- sm_SolveMinSignedAngle -----------------------------------------------------------

/// The curves, intervals and caches of a signed angle solve.
struct AngleOperands<'a, O> {
    curve: &'a Curve,
    interval: &'a Extent1d,
    other: &'a Curve,
    other_interval: &'a Extent1d,
    /// `pCC1->GetContinuitiesArray()`.
    conts1: &'a [ContinuityType],
    /// `pCC2->GetContinuitiesArray()`.
    conts2: &'a [ContinuityType],
    handles: [Option<O>; 2],
}

/// `sm_SolveMinSignedAngle`: the minimum signed angle solutions of `SM_SO_SIGNED_ANGLE_MINIMIZE`
/// (first option vector the projection plane normal, second the reference
/// direction), composed from the directed angle minima at the curves' C0
/// discontinuities and the projected tangencies between the curves, accumulated by
/// `gs` set up as a signed directed minimum.
fn solve_min_signed_angle<O: SolutionObject>(
    gs: &mut CcGlobalSolver<'_, O>,
    k: &Kernel,
    ops: &AngleOperands<'_, O>,
    dist_tol: f64,
    opt_vectors: &[Vec3],
    requested: SolutionRequested,
) -> Result<SolutionArray<O>> {
    let opt = |i: usize| -> Result<Vec3> {
        opt_vectors
            .get(i)
            .copied()
            .ok_or(SolidError::invalid_input(OP_MSA, "missing option vector"))
    };

    // Set up global solver so things work properly for it
    gs.base
        .set_solver_operation(SolverOperation::SignedDirectedMinimize); // Use same signed business as with this
    gs.base.set_solution_requested(requested);
    gs.base.set_3d_tolerance(dist_tol);
    gs.base.set_best_answer_so_far_sq(BIG_DOUBLE);
    gs.base.set_num_trees(0);
    gs.base.set_num_variables(2);
    let saved = gs.base.set_solutions(SolutionArray::new());

    let result = min_signed_angle_into(gs, k, ops, dist_tol, &opt, opt_vectors, requested);

    // take the accumulated solutions back
    let solutions = gs.base.set_solutions(saved);
    result.map(|()| solutions)
}

/// The body of [`solve_min_signed_angle`], adding to `gs`'s solutions.
fn min_signed_angle_into<O: SolutionObject>(
    gs: &mut CcGlobalSolver<'_, O>,
    k: &Kernel,
    ops: &AngleOperands<'_, O>,
    dist_tol: f64,
    opt: &dyn Fn(usize) -> Result<Vec3>,
    opt_vectors: &[Vec3],
    requested: SolutionRequested,
) -> Result<()> {
    let new_solution = || {
        let mut s: Solution<O> = Solution::new();
        s.solution_type = SolutionType::SingleValue;
        s.num_variables = 2;
        s.num_objects = 2;
        s.objects[0] = ops.handles[0];
        s.objects[1] = ops.handles[1];
        s.nodes[0] = None;
        s.nodes[1] = None;
        s
    };

    // Process discontinuites of crCurve, then of crOtherCurve
    for first in [true, false] {
        let (disc_curve, disc_ivl, conts, solve_curve, solve_ivl, handle) = if first {
            (
                ops.curve,
                ops.interval,
                ops.conts1,
                ops.other,
                ops.other_interval,
                ops.handles[1],
            )
        } else {
            (
                ops.other,
                ops.other_interval,
                ops.conts2,
                ops.curve,
                ops.interval,
                ops.handles[0],
            )
        };
        let (knots, _) = disc_curve.knots(k, None)?;

        // for every continuity boundary
        for (jj, &cont) in conts.iter().enumerate() {
            // when the continuity is low
            if ord(cont) > ord(ContinuityType::C0) {
                continue;
            }
            let knot = knots.get(jj).copied().ok_or(SolidError::invalid_input(
                OP_MSA,
                "fewer knots than continuities",
            ))?;

            // and this discontinuity is in the interval
            if !disc_ivl.contains_value(knot, 0.0) {
                continue;
            }
            let pnt = disc_curve.evaluate_point(k, knot)?;
            let dir_norm = [pnt, opt(0)?, opt(1)?];

            // look for solutions
            let popts = PropertyAnalysisOptions {
                property_value: None,
                opt_vectors: &dir_norm,
                want_comb: false,
                handle,
            };
            let sols = solve_curve
                .global_property_analysis(
                    k,
                    solve_ivl,
                    CurvePropertyType::MinimizeDirectedAngle,
                    dist_tol,
                    &popts,
                )?
                .solutions;

            // save solutions
            for r in sols.as_slice() {
                let mut s = new_solution();
                if first {
                    s.start.solution_value = r.start.solution_value;
                    s.start.parameters[0] = knot;
                    s.start.parameters[1] = r.start.parameters[0];
                } else {
                    s.start.parameters[0] = r.start.parameters[0];
                    s.start.parameters[1] = knot;

                    // Solution is negative the complementary angle
                    let mut complement = PI - r.start.solution_value.abs();
                    if complement * r.start.solution_value > 0.0 {
                        complement = -complement;
                    }
                    s.start.solution_value = complement;
                }
                gs.add_sorted_solution(&s, SortKey::ByValueWithParameter, true)?;
            }
        }
    }

    // Now process tangencies between curves.
    let tangencies = ops.curve.global_curve_solve(
        k,
        ops.interval,
        ops.other,
        ops.other_interval,
        SolverOperation::ProjectedTangency,
        dist_tol,
        &CurveSolveOptions {
            target_distance: None,
            opt_vectors: Some(opt_vectors),
            handles: ops.handles,
        },
        requested,
    )?;

    // save every solution
    for r in tangencies.as_slice() {
        let mut s = new_solution();
        s.start.parameters[0] = r.start.parameters[0];
        s.start.parameters[1] = r.start.parameters[1];

        let pnt1 = ops.curve.evaluate_point(k, s.start.parameters[0])?;
        let pnt2 = ops.other.evaluate_point(k, s.start.parameters[1])?;
        let mut vec = pnt2 - pnt1;
        vec = opt(0)?.cross(vec).cross(opt(0)?);
        // If the point is at the origin of rotation just ignore it
        if vec.length_squared() > EFF_ZERO_SQ {
            vec = vec.unitize()?;
            let mut cos_theta = opt(0)?.ccw_angle_between(opt(1)?, vec)?;
            if cos_theta > PI {
                cos_theta = 2.0 * PI - cos_theta; // make range 180 to -180
            }
            s.start.solution_value = cos_theta;

            gs.add_sorted_solution(&s, SortKey::ByValueWithParameter, true)?;
        }
    }

    Ok(())
}

// ---- GlobalCurveSolve -----------------------------------------------------------------

/// The optional inputs of [`Curve::global_curve_solve`].
#[derive(Debug, Clone, Copy)]
pub struct CurveSolveOptions<'a, O> {
    /// `cpdOptTargetDistance`: the target distance of [`SolverOperation::AtDistance`]
    /// (required there), else the limit a minimum must be below (a maximum above).
    pub target_distance: Option<f64>,
    /// `cpOptVectors` (`None` = null): see [`SolverOperation`] for their meaning per
    /// operation.
    pub opt_vectors: Option<&'a [Vec3]>,
    /// The handles of this curve and the other curve: the owners of their cache
    /// trees, named by the solutions (upstream `this`, `&crOtherCurve`). With both
    /// `None` the solutions name no objects, but are merged as upstream's (which
    /// always name the curves).
    pub handles: [Option<O>; 2],
}

impl<O> Default for CurveSolveOptions<'_, O> {
    fn default() -> Self {
        Self {
            target_distance: None,
            opt_vectors: None,
            handles: [None, None],
        }
    }
}

/// `GetOrCreateObjectCache` for a curve (`NER` on failure).
fn build_cache<O: SolutionObject>(
    k: &Kernel,
    curve: &Curve,
    handle: Option<O>,
) -> Result<CurveCache<O>> {
    match make_curve_cache::<O>(k, curve, handle) {
        Ok(c) => Ok(c),
        Err(e) if e.is_unported() => Err(e),
        Err(_) => Err(SolidError::null_pointer(OP_GCS)),
    }
}

/// The cache tree (`GetTree`, `NER`).
fn cache_tree<O: SolutionObject>(cache: &CurveCache<O>) -> Result<&Tree<O>> {
    cache.tree().ok_or(SolidError::null_pointer(OP_GCS))
}

/// The axis aligned box of `CalculateBoundingBox` of `curve` over its natural
/// interval (status ignored: the default box on failure).
fn natural_bounding_box(k: &Kernel, curve: &Curve) -> Result<Extent3d> {
    let req = BoundingBoxRequest {
        normal: true,
        pseudo: false,
        polar: false,
    };
    Ok(
        ignored(curve.calculate_bounding_box(k, &curve.natural_interval(), req, true))?
            .and_then(|b| b.normal)
            .unwrap_or_else(Extent3d::new),
    )
}

impl Curve {
    /// `GlobalCurveSolve`: the extrema between this curve over `interval` and `other`
    /// over `other_interval` (`start.parameters`: this curve's, then the other curve's
    /// parameter): the closest or farthest points ([`SolverOperation::Minimize`],
    /// `Maximize`, and their directed, projected and signed variants), the common
    /// normals (`Normalize`), the points at a target distance (`AtDistance`), the
    /// projected tangencies and the minimum signed angle; the projected, rotated
    /// projected and perspective intersections go to the curve/curve intersection
    /// solver, with line/line pairs (also curves whose caches replace them by lines)
    /// intersected directly. A minimum (or at-distance) target whose curve boxes are
    /// farther apart than the target plus `dist_tol` has no solutions.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` when a cache cannot be built or has no tree, or for
    /// [`SolverOperation::AtDistance`] without target distance; `InvalidInput` for a
    /// missing option vector; those of the line/line intersection and of the signed
    /// angle minimum. Errors of the global solve itself are ignored (upstream ignores
    /// `SolveIt`'s status), except [`SolidError::Unported`].
    pub fn global_curve_solve<O: SolutionObject>(
        &self,
        k: &Kernel,
        interval: &Extent1d,
        other: &Self,
        other_interval: &Extent1d,
        op: SolverOperation,
        dist_tol: f64,
        opts: &CurveSolveOptions<'_, O>,
        requested: SolutionRequested,
    ) -> Result<SolutionArray<O>> {
        type Op = SolverOperation;
        // no handles: upstream's solutions still name this curve and the other one
        // (whose natural intervals IdenticalSolutions reads)
        if opts.handles == [None, None] {
            let own = operand_handles(self, other);
            let inner = CurveSolveOptions {
                target_distance: opts.target_distance,
                opt_vectors: opts.opt_vectors,
                handles: [Some(own[0]), Some(own[1])],
            };
            return Ok(self
                .global_curve_solve(
                    k,
                    interval,
                    other,
                    other_interval,
                    op,
                    dist_tol,
                    &inner,
                    requested,
                )?
                .without_objects());
        }
        let [h_this, h_other] = opts.handles;
        let same_curve = core::ptr::eq(self, other);
        let opt = |i: usize| -> Result<Vec3> {
            opts.opt_vectors
                .and_then(|v| v.get(i))
                .copied()
                .ok_or(SolidError::invalid_input(OP_GCS, "missing option vector"))
        };

        // init outputs
        let mut sols: SolutionArray<O> = SolutionArray::new();

        if let Some(target) = opts.target_distance {
            if op == Op::Minimize || op == Op::Signed3dDirectedMinimize || op == Op::AtDistance {
                let bbox1 = natural_bounding_box(k, self)?;
                let bbox2 = natural_bounding_box(k, other)?;
                if bbox1.are_disjoint(&bbox2, target + dist_tol) {
                    return Ok(sols);
                }
            }
        }

        // get target curves' caches
        let mut cache1 = build_cache(k, self, h_this)?;
        let mut cache2_own = if same_curve {
            None
        } else {
            Some(build_cache(k, other, h_other)?)
        };

        // get each curve's spatial decomposition tree
        cache_tree(&cache1)?;
        cache_tree(cache2_own.as_ref().unwrap_or(&cache1))?;

        // Set up global solver for projected intersect.
        if op == Op::ProjectedIntersect
            || op == Op::PerspectiveIntersect
            || op == Op::RotatedProjectedIntersect
        {
            // special case analytic/analytic line intersections for parallel and
            // perspective projections, because lines parallel and perspecitve
            // projected map to lines.
            if op == Op::ProjectedIntersect || op == Op::PerspectiveIntersect {
                let analytic1 = cache1.analytic_curve(k)?.cloned();
                let analytic2 = match cache2_own.as_mut() {
                    Some(c2) => c2.analytic_curve(k)?.cloned(),
                    None => analytic1.clone(),
                };

                if let (Some(a1), Some(a2)) = (analytic1, analytic2) {
                    // special case Line/Line intersection
                    if matches!(a1.kind(), CurveKind::Line(_))
                        && matches!(a2.kind(), CurveKind::Line(_))
                    {
                        // get Line start and stop endPoints
                        let start1 = a1.evaluate_point(k, interval.min())?;
                        let end1 = a1.evaluate_point(k, interval.max())?;
                        let start2 = a2.evaluate_point(k, other_interval.min())?;
                        let end2 = a2.evaluate_point(k, other_interval.max())?;

                        let hits = if op == Op::ProjectedIntersect {
                            // Intersect two projected segments
                            segment_segment_projected_intersect(
                                start1,
                                end1,
                                start2,
                                end2,
                                opt(0)?,
                                dist_tol,
                            )?
                        } else {
                            let view = PerspectivePlane {
                                point: opt(0)?,
                                unit_normal: opt(1)?,
                                eye: opt(2)?,
                            };
                            // Intersect two projected segments
                            segment_segment_perspective_intersect(
                                start1, end1, start2, end2, &view, dist_tol,
                            )?
                        };

                        // no solution case
                        if hits.count == 0 {
                            return Ok(sols);
                        }

                        // single solution case - set output
                        let mut sol: Solution<O> = Solution::new();
                        sol.num_objects = 0;
                        sol.num_variables = 2;
                        sol.solution_type = SolutionType::SingleValue;
                        sol.start.solution_value = hits.distances[0];
                        sol.start.parameters[0] = interval.evaluate(hits.params1[0]);
                        sol.start.parameters[1] = other_interval.evaluate(hits.params2[0]);
                        if hits.count > 1 {
                            sol.solution_type = SolutionType::RangeOfValues;
                            sol.end.solution_value = hits.distances[1];
                            sol.end.parameters[0] = interval.evaluate(hits.params1[1]);
                            sol.end.parameters[1] = other_interval.evaluate(hits.params2[1]);
                        }
                        sols.add(&sol);

                        // all done
                        return Ok(sols);
                    }
                }
            }

            // arrive here when not a Line/Line intersection for parallel and
            // perspective projections case: get and return general solver solutions
            let projection_type = match op {
                Op::ProjectedIntersect => ProjectionType::Parallel,
                Op::PerspectiveIntersect => ProjectionType::Perspective,
                _ => ProjectionType::Rotation,
            };
            let tree1 = cache_tree(&cache1)?;
            let tree2 = cache_tree(cache2_own.as_ref().unwrap_or(&cache1))?;
            let mut gs = CciGlobalSolver::new(
                tree1,
                tree2,
                *interval,
                *other_interval,
                true,
                projection_type,
            )
            .with_curves(self, other, k);
            ignore_status(gs.solve_it(
                &SolveRequest {
                    operation: op,
                    solution_requested: requested,
                    tolerance_3d: dist_tol,
                    best_answer_so_far_sq: BIG_DOUBLE,
                    opt_vectors: opts.opt_vectors.unwrap_or(&[]),
                },
                &mut sols,
            ))?;
            // all done
            return Ok(sols);
        }

        // Set up global solver to solve single variable.
        let cache2 = cache2_own.as_ref().unwrap_or(&cache1);
        let tree1 = cache_tree(&cache1)?;
        let tree2 = cache_tree(cache2)?;
        let mut gs = CcGlobalSolver::new(tree1, tree2, *interval, *other_interval)
            .with_curves(self, other, k);

        // get and return min_signed_angle solutions
        if op == Op::SignedAngleMinimize {
            let operands = AngleOperands {
                curve: self,
                interval,
                other,
                other_interval,
                conts1: cache1.continuities(),
                conts2: cache2.continuities(),
                handles: opts.handles,
            };
            return solve_min_signed_angle(
                &mut gs,
                k,
                &operands,
                dist_tol,
                opts.opt_vectors.unwrap_or(&[]),
                requested,
            );
        }

        // set dBestAnswerSoFarSq and sGS member values for different eSolverOperations
        let mut best_answer_so_far_sq = BIG_DOUBLE;
        match op {
            Op::AtDistance => {
                let target = opts
                    .target_distance
                    .ok_or(SolidError::null_pointer(OP_GCS))?;
                gs.base.set_at_distance(target);
            }
            Op::SignedDirectedMinimize | Op::Signed3dDirectedMinimize => {
                if let Some(t) = opts.target_distance {
                    best_answer_so_far_sq = t * t.abs();
                }
            }
            Op::Minimize | Op::DirectedMinimize | Op::ProjectedMinimize => {
                if let Some(t) = opts.target_distance {
                    best_answer_so_far_sq = t * t;
                }
            }
            Op::Maximize | Op::DirectedMaximize | Op::ProjectedMaximize => {
                best_answer_so_far_sq = 0.0;
                if let Some(t) = opts.target_distance {
                    best_answer_so_far_sq = t * t;
                }
            }
            _ => {}
        }

        // get global solver solutions (status ignored)
        ignore_status(gs.solve_it(
            &SolveRequest {
                operation: op,
                solution_requested: requested,
                tolerance_3d: dist_tol,
                best_answer_so_far_sq,
                opt_vectors: opts.opt_vectors.unwrap_or(&[]),
            },
            &mut sols,
        ))?;

        Ok(sols)
    }

    /// `GlobalCurveSelfIntersect`: [`Curve::global_curve_intersect`] of this curve
    /// over `interval` with itself (points where it meets itself at different
    /// parameters; a closed curve always has one). `handle` names the curve in the
    /// solutions, `analytic_handle` the analytic replacement its cache may make.
    ///
    /// # Errors
    ///
    /// Those of [`Curve::global_curve_intersect`].
    pub fn global_curve_self_intersect<O: SolutionObject>(
        &self,
        k: &Kernel,
        interval: &Extent1d,
        dist_tol: f64,
        handle: Option<O>,
        analytic_handle: Option<O>,
    ) -> Result<SolutionArray<O>> {
        let opts = CurveIntersectOptions {
            skip_global_coincidence_check: false,
            handles: [handle, handle],
            analytic_handles: [analytic_handle, analytic_handle],
        };
        self.global_curve_intersect(k, interval, self, interval, dist_tol, &opts)
    }

    // ---- tight boxes ----------------------------------------------------------------

    /// The points of this curve over `interval` where its tangent is perpendicular
    /// to `dir` (`GlobalPropertyAnalysis(SM_CP_PERPENDICULAR_TO_VECTOR)` with
    /// tolerance `SM_EFF_ZERO_SQRT`), added to `add` (each evaluation's status
    /// ignored into the reused point `pt`).
    fn add_perpendicular_points(
        &self,
        k: &Kernel,
        interval: &Extent1d,
        dir: Vec3,
        pt: &mut Point3,
        add: &mut dyn FnMut(Point3),
    ) -> Result<()> {
        let dirs = [dir];
        let opts = PropertyAnalysisOptions {
            property_value: None,
            opt_vectors: &dirs,
            want_comb: false,
            handle: Some(CurveHandle(self.natural_interval())),
        };
        let sols = self
            .global_property_analysis(
                k,
                interval,
                CurvePropertyType::PerpendicularToVector,
                EFF_ZERO_SQRT,
                &opts,
            )?
            .solutions;
        for s in sols.as_slice() {
            eval_keep(k, self, s.start.parameters[0], pt)?;
            add(*pt);
        }
        Ok(())
    }

    /// `CalculateTightBoundingBox`: the axis aligned box of the curve over `interval`
    /// from its end points, (for B-splines) its tangent discontinuous knots and last
    /// knot, and the points where its tangent is perpendicular to x, y and z. The
    /// line override is [`crate::curve::line::Line::calculate_tight_bounding_box`].
    /// With [`crate::kernel::UpstreamFixes::curve_tight_box_interval_knots`] only the
    /// tangent discontinuous knots strictly inside `interval` are added (WR-474, #1054).
    ///
    /// # Errors
    ///
    /// Those of the knot and continuity queries and of the property analyses.
    pub fn calculate_tight_bounding_box(
        &self,
        k: &Kernel,
        interval: &Extent1d,
    ) -> Result<Extent3d> {
        if let CurveKind::Line(l) = self.kind() {
            return Ok(l
                .calculate_tight_bounding_box(interval)
                .unwrap_or_else(Extent3d::new));
        }

        let mut tight_box = Extent3d::new();

        // First put in the end points.
        let mut pt = Vec3::UNDEF;
        eval_keep(k, self, interval.min(), &mut pt)?;
        tight_box.add_point(pt);
        eval_keep(k, self, interval.max(), &mut pt)?;
        tight_box.add_point(pt);

        // Now add all tangent discontinuous points
        if self.is_kind_of(BSPLINE_CURVE_TYPE) {
            let (knots, _) = self.knots(k, None)?;
            let (_, continuities) = self.calculate_continuities(k, 2.0)?;

            let n = knots.len();
            for (i, &knot) in knots.iter().enumerate().skip(1) {
                let low = continuities
                    .get(i)
                    .is_some_and(|&c| ord(c) <= ord(ContinuityType::C1));
                let add = if k.fixes.curve_tight_box_interval_knots {
                    // WR-474 (#1054): only the tangent discontinuous knots strictly inside
                    // the interval; its ends are in the box already, and a knot outside it
                    // is a point the curve never reaches over `interval`.
                    low && knot > interval.min() && knot < interval.max()
                } else {
                    i == n - 1 || low
                };
                if add {
                    eval_keep(k, self, knot, &mut pt)?;
                    tight_box.add_point(pt);
                }
            }
        }

        // Now solve along curve.
        for dir in [
            Vec3::new(1.0, 0.0, 0.0), // +/- x
            Vec3::new(0.0, 1.0, 0.0), // +/- y
            Vec3::new(0.0, 0.0, 1.0), // +/- z
        ] {
            self.add_perpendicular_points(k, interval, dir, &mut pt, &mut |p| {
                tight_box.add_point(p);
            })?;
        }

        Ok(tight_box)
    }

    /// `CalculateTightPseudoBox` into `pseudo_box`: its intervals reset, its basis set
    /// to the three unit vectors, then the curve's end points over `interval` and the
    /// points where its tangent is perpendicular to each basis vector added. On error
    /// the box keeps what was added so far.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for linearly dependent basis vectors (the box is then only
    /// reset); those of the property analyses.
    pub fn calculate_tight_pseudo_box_into(
        &self,
        k: &Kernel,
        interval: &Extent1d,
        basis: [Vec3; 3],
        pseudo_box: &mut PseudoBox,
    ) -> Result<()> {
        // init output and check input
        pseudo_box.init(); // init intervals - leave basis vectors alone
        let [b0, b1, b2] = basis;
        pseudo_box.set_basis(b0, b1, b2)?;

        // First put in the end points.
        let mut pt = Vec3::UNDEF;
        eval_keep(k, self, interval.min(), &mut pt)?;
        pseudo_box.add_point(pt);
        eval_keep(k, self, interval.max(), &mut pt)?;
        pseudo_box.add_point(pt);

        // Now solve along curve.
        for dir in basis {
            self.add_perpendicular_points(k, interval, dir, &mut pt, &mut |p| {
                pseudo_box.add_point(p);
            })?;
        }

        // all done
        Ok(())
    }

    /// `CalculateTightPseudoBox`: the pseudo box with the unit `basis` of the curve
    /// over `interval` (see [`Curve::calculate_tight_pseudo_box_into`]).
    ///
    /// # Errors
    ///
    /// Those of [`Curve::calculate_tight_pseudo_box_into`].
    pub fn calculate_tight_pseudo_box(
        &self,
        k: &Kernel,
        interval: &Extent1d,
        basis: [Vec3; 3],
    ) -> Result<PseudoBox> {
        let mut pseudo_box = PseudoBox::new();
        self.calculate_tight_pseudo_box_into(k, interval, basis, &mut pseudo_box)?;
        Ok(pseudo_box)
    }

    // ---- plane tests ----------------------------------------------------------------

    /// `IsOnPlane`: whether the curve over `interval` (the natural interval for
    /// `None`) is within `tol` of the plane through `plane_point` with normal
    /// `plane_normal`, and the largest distance. The B-spline kinds run the
    /// `SmBSplineCurve` override (end, middle and control points; see the module
    /// deviations), the others [`is_on_plane_base`].
    ///
    /// # Errors
    ///
    /// Those of the override; [`SolidError::Unported`] from the base.
    pub fn is_on_plane(
        &self,
        k: &Kernel,
        plane_point: Point3,
        plane_normal: Vec3,
        tol: f64,
        interval: Option<&Extent1d>,
    ) -> Result<OnPlane> {
        match self.kind() {
            CurveKind::BSpline(c) => c.is_on_plane(plane_point, plane_normal, tol, interval),
            CurveKind::Hyperbola(c) => {
                bspline_is_on_plane(k, self, c.base(), plane_point, plane_normal, tol, interval)
            }
            CurveKind::Ellipse(c) => {
                bspline_is_on_plane(k, self, c.base(), plane_point, plane_normal, tol, interval)
            }
            CurveKind::Parabola(c) => {
                bspline_is_on_plane(k, self, c.base(), plane_point, plane_normal, tol, interval)
            }
            CurveKind::Line(c) => {
                let base = c.base_with_nurb()?;
                bspline_is_on_plane(k, self, &base, plane_point, plane_normal, tol, interval)
            }
            _ => is_on_plane_base(self, k, plane_point, plane_normal, tol, interval),
        }
    }

    /// `IsOnPositiveSideOfPlanes`: whether the curve over its natural interval lies
    /// on the positive side of every plane (`plane_points[i]`, `plane_normals[i]`)
    /// within `SM_EFF_ZERO` (`GlobalPropertyAnalysis(SM_CP_PLANE_CLASSIFY)` finds a
    /// single range covering the curve). Touching or lying in a plane counts as
    /// positive.
    ///
    /// # Errors
    ///
    /// `InvalidInput` when there are fewer normals than points;
    /// [`SolidError::Unported`] from the analyses (whose other failures count as no
    /// solutions, statuses ignored upstream).
    pub fn is_on_positive_side_of_planes(
        &self,
        k: &Kernel,
        plane_points: &[Point3],
        plane_normals: &[Vec3],
    ) -> Result<bool> {
        // locals
        let ivl = self.natural_interval();

        // for every plane
        for (ii, &plane_point) in plane_points.iter().enumerate() {
            let normal = plane_normals
                .get(ii)
                .copied()
                .ok_or(SolidError::invalid_input(
                    OP_POS,
                    "fewer normals than points",
                ))?;

            // locals: d = -dot(PlaneNormal, PointOnPlane),
            let d = -normal.dot(plane_point);

            // find all Curve Intervals on positive side of the plane
            let normals = [normal];
            let opts = PropertyAnalysisOptions {
                property_value: Some(d),
                opt_vectors: &normals,
                want_comb: false,
                handle: Some(CurveHandle(ivl)),
            };
            let sols = ignored(self.global_property_analysis(
                k,
                &ivl,
                CurvePropertyType::PlaneClassify,
                EFF_ZERO,
                &opts,
            ))?
            .map(|r| r.solutions)
            .unwrap_or_default();

            // when curve is NOT (totally on the positive side of this plane)
            let whole = sols.len() == 1
                && sols.get(0).is_some_and(|s| {
                    s.solution_type == SolutionType::RangeOfValues
                        && ivl.are_equal_bounds(
                            s.start.parameters[0],
                            s.end.parameters[0],
                            EFF_ZERO,
                        )
                });
            if !whole {
                return Ok(false);
            }
        }

        // arrive here when curve is on the inside of all discontinuity planes
        Ok(true)
    }
}

/// The `SmBSplineCurve::IsOnPlane` override for a curve derived from `SmBSplineCurve`
/// (`base` its B-spline part): the start, end and middle points (the curve's own
/// evaluation) within `tol` of the plane, then (unless the curve is a line) the
/// control points of the spans covering the interval.
fn bspline_is_on_plane(
    k: &Kernel,
    curve: &Curve,
    base: &BSplineCurve,
    plane_point: Point3,
    plane_normal: Vec3,
    tol: f64,
    interval: Option<&Extent1d>,
) -> Result<OnPlane> {
    let iv = match interval {
        Some(i) => *i,
        None => curve.natural_interval(),
    };

    // get Curve start, mid, end points
    let start_pt = curve.evaluate_point(k, iv.min())?;
    let end_pt = curve.evaluate_point(k, iv.max())?;
    let mid_pt = curve.evaluate_point(k, iv.evaluate(0.5))?;

    // project points to plane
    let proj_start_pt = point_project_to_plane(start_pt, plane_point, plane_normal);
    let proj_end_pt = point_project_to_plane(end_pt, plane_point, plane_normal);
    let proj_mid_pt = point_project_to_plane(mid_pt, plane_point, plane_normal);

    // get start, mid, end point distances to plane
    let st_discr = (start_pt - proj_start_pt).length();
    let end_discr = (end_pt - proj_end_pt).length();
    let mid_discr = (mid_pt - proj_mid_pt).length();

    // low work - points not on plane
    if st_discr > tol || end_discr > tol || mid_discr > tol {
        return Ok(OnPlane {
            on_plane: false,
            max_dist: None,
        });
    }

    // get max of 3 sample point distances - good enough for lines
    let mut max_dist = crate::math::max3(st_discr, mid_discr, end_discr);

    // for nonLine curves - get max distance to controlPoint
    if !curve.is_kind_of(LINE_TYPE) {
        for pnt in base.polygon(&iv)? {
            let dist = crate::geom::geom_utility::p1::plane_point_distance(
                plane_point,
                plane_normal,
                pnt,
            )?;
            if dist > max_dist {
                max_dist = dist;
            }
        }
    }

    Ok(OnPlane {
        on_plane: max_dist < tol,
        max_dist: Some(max_dist),
    })
}

/// The base-class `SmCurve::IsOnPlane`: the tight pseudo box of the curve over
/// `interval` (the natural interval for `None`) aligned with the plane contains
/// `plane_point` within `tol` and is point sized, linear or planar within `tol`; the
/// largest distance is half the box's extent along the normal (set whenever the box
/// contains the point).
///
/// # Errors
///
/// [`SolidError::Unported`] from the tight pseudo box (whose other failures leave
/// the partial box, statuses ignored upstream).
pub fn is_on_plane_base(
    curve: &Curve,
    k: &Kernel,
    plane_point: Point3,
    plane_normal: Vec3,
    tol: f64,
    interval: Option<&Extent1d>,
) -> Result<OnPlane> {
    // build an unitorthonormal basis from crPlaneNormal (status ignored: a failed
    // Unitize of the normal leaves (0, 0, 1) in the x axis, the others unwritten)
    let basis = match plane_normal.make_unit_ortho_vectors(None) {
        Ok([x, y, z]) => [y, z, x],
        Err(_) => [Vec3::UNDEF, Vec3::UNDEF, plane_normal.unitize_or_z()],
    };

    // calculate tight pseudo box
    let iv = match interval {
        Some(i) => *i,
        None => curve.natural_interval(),
    };
    let mut pseudo_box = PseudoBox::new();
    ignore_status(curve.calculate_tight_pseudo_box_into(k, &iv, basis, &mut pseudo_box))?;

    // classify the PlanePoint
    let mut on_plane = pseudo_box.contains_point(plane_point, tol);
    let mut max_dist = None;

    // set output
    if on_plane {
        on_plane = pseudo_box.is_point_sized(tol).is_point_sized
            || pseudo_box.is_linear(tol).is_linear
            || pseudo_box.is_planar(tol).is_planar;
        max_dist = pseudo_box.interval(2).map(|i| i.length() / 2.0);
    }

    Ok(OnPlane { on_plane, max_dist })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segment(a: [f64; 3], b: [f64; 3]) -> Curve {
        Curve::from(
            BSplineCurve::create_line_segment(3, Vec3::from_array(a), Vec3::from_array(b)).unwrap(),
        )
    }

    fn handles(c1: &Curve, c2: &Curve) -> [Option<CurveHandle>; 2] {
        [
            Some(CurveHandle(c1.natural_interval())),
            Some(CurveHandle(c2.natural_interval())),
        ]
    }

    #[test]
    fn minimum_between_parallel_segments() {
        let k = Kernel::default();
        let a = segment([0.0, 0.0, 0.0], [4.0, 0.0, 0.0]);
        let b = segment([0.0, 3.0, 0.0], [4.0, 3.0, 0.0]);
        let opts = CurveSolveOptions {
            handles: handles(&a, &b),
            ..CurveSolveOptions::default()
        };
        let sols = a
            .global_curve_solve(
                &k,
                &a.natural_interval(),
                &b,
                &b.natural_interval(),
                SolverOperation::Minimize,
                1e-5,
                &opts,
                SolutionRequested::All,
            )
            .unwrap();
        assert!(!sols.is_empty());
        for s in sols.as_slice() {
            assert!((s.start.solution_value - 3.0).abs() < 1e-9);
        }
    }

    #[test]
    fn at_distance_needs_a_target() {
        let k = Kernel::default();
        let a = segment([0.0, 0.0, 0.0], [4.0, 0.0, 0.0]);
        let b = segment([0.0, 3.0, 0.0], [4.0, 3.0, 0.0]);
        let opts = CurveSolveOptions {
            handles: handles(&a, &b),
            ..CurveSolveOptions::default()
        };
        let ivl = a.natural_interval();
        let r = a.global_curve_solve(
            &k,
            &ivl,
            &b,
            &ivl,
            SolverOperation::AtDistance,
            1e-5,
            &opts,
            SolutionRequested::All,
        );
        assert!(r.is_err());
    }

    #[test]
    fn tight_box_of_a_segment() {
        let k = Kernel::default();
        let a = segment([1.0, -2.0, 0.5], [4.0, 3.0, 0.5]);
        let bx = a
            .calculate_tight_bounding_box(&k, &a.natural_interval())
            .unwrap();
        assert_eq!(
            [
                bx.u_min(),
                bx.v_min(),
                bx.w_min(),
                bx.u_max(),
                bx.v_max(),
                bx.w_max()
            ],
            [1.0, -2.0, 0.5, 4.0, 3.0, 0.5]
        );
    }

    #[test]
    fn planar_segment_is_on_its_plane() {
        let k = Kernel::default();
        let a = segment([1.0, -2.0, 0.5], [4.0, 3.0, 0.5]);
        let z = Vec3::new(0.0, 0.0, 1.0);
        let on = a
            .is_on_plane(&k, Vec3::new(0.0, 0.0, 0.5), z, 1e-6, None)
            .unwrap();
        assert!(on.on_plane);
        let off = a
            .is_on_plane(&k, Vec3::new(0.0, 0.0, 2.0), z, 1e-6, None)
            .unwrap();
        assert!(!off.on_plane);
    }

    #[test]
    fn segment_on_positive_side() {
        let k = Kernel::default();
        let a = segment([1.0, -2.0, 0.5], [4.0, 3.0, 0.5]);
        let below = [Vec3::new(0.0, 0.0, -1.0)];
        let up = [Vec3::new(0.0, 0.0, 1.0)];
        assert!(a.is_on_positive_side_of_planes(&k, &below, &up).unwrap());
        let down = [Vec3::new(0.0, 0.0, -1.0)];
        assert!(!a.is_on_positive_side_of_planes(&k, &below, &down).unwrap());
    }
}
