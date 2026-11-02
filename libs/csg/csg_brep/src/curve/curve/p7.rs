//! `SmCurve`, part 7: the local solve of the curve/curve intersection global solver
//! (`SmCCIGlobalSolver::LocalSolve`, intersecting two Bezier spans of the curves'
//! decomposition trees, optionally projected to a common plane) and the global
//! curve property search `SmCurve::GlobalPropertyAnalysis` (with its optional check
//! comb).
//!
//! Upstream: `source/SMLib/src/SmCurve.cpp`, `source/SMLib/inc/SmCurve.h`.
//!
//! | Rust                                         | Upstream                                    |
//! |----------------------------------------------|---------------------------------------------|
//! | [`CciGlobalSolver::local_solve_branch`]      | `SmCCIGlobalSolver::LocalSolve`             |
//! | [`Curve::global_property_analysis`], [`PropertyAnalysis`], [`PropertyAnalysisOptions`] | `SmCurve::GlobalPropertyAnalysis` |
//!
//! Deviations:
//! - **Curves and kernel.** The solver's trees only name their curves by the
//!   caller's handle ([`crate::geom::tree::Tree::owner_object`]), so the solver
//!   carries the curves and the kernel ([`CciGlobalSolver::with_curves`]); a solver
//!   without them is [`SolidError::InvalidInput`]. A node without data (upstream
//!   `NER`) is `SM_ERR_NULL_POINTER`; a node that is not a leaf span is
//!   `InvalidInput` (upstream casts whatever `m_pData` holds).
//! - **Projection.** The projected curves are [`ProjectedCurve`]s owning copies of
//!   the curves (upstream borrows them). The pseudo boxes are projected in place as
//!   upstream (`ProjectToPlane` into the box itself, keeping its basis when the
//!   projection fails); a projection type other than parallel, rotation or
//!   perspective is `InvalidInput` (upstream dereferences null projection data).
//! - **Ignored statuses.** The span end evaluations, the closed-curve continuity,
//!   the projected curve set-up, the box projections and intersections and the end
//!   point `LocalPointSolve`s ignore their status upstream; the port ignores the
//!   errors too (a failed evaluation leaves [`Vec3::UNDEF`], a failed continuity
//!   `SM_CT_DISCONTINUOUS`, a failed point solve no answer), except
//!   [`SolidError::Unported`], which is always propagated.
//! - **Upstream quirks kept.** When either curve is degenerate the accumulated end
//!   point solutions are re-added as the last one built (`sSolution2`), once per
//!   accumulated solution, as upstream. The span adjacency tests compare unsigned
//!   leaf indices with wrapping arithmetic.
//! - **`GlobalPropertyAnalysis`.** Out-params become [`PropertyAnalysis`]; the
//!   optional comb arrays are produced when [`PropertyAnalysisOptions::want_comb`]
//!   is set. The curve's cache is built for the call ([`make_curve_cache`], see unit
//!   `curve.curve_p5`). A property value or vector the property reads but the caller
//!   did not give is [`SolidError::InvalidInput`] (upstream reads through a null or
//!   short array). In the comb, failed evaluations (statuses ignored upstream, into
//!   uninitialised locals) leave [`Vec3::UNDEF`]; a failed `EvaluateGeometric` keeps
//!   what `smgu_FrenetFromDerivatives` wrote before failing (see
//!   `evaluate_geometric_partial`).

//!
//! The crate-private `global_property_analysis_into` retains solutions when a later
//! solve fails, for callers that ignore the upstream status and inspect its outputs.

use crate::curve::curve::p2::{
    at, evaluate_continuity, CciGlobalSolver, Curve, CurvePropertyGs, CurvePropertyType,
};
use crate::curve::curve::p3::{eval_n, local_curve_intersect, set_param, PointSolveOptions};
use crate::curve::curve::p4::{filter_solution, point_or_undef};
use crate::curve::curve::p5::{find_additional_cc_tangent_guess, make_curve_cache};
use crate::curve::curve_cache::BezierSpan;
use crate::curve::projected_curve::{OwnsCurve, ProjectedCurve};
use crate::error::{ignored, Result, SolidError};
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::extent1d::Extent1d;
use crate::geom::extent3d::Extent3d;
use crate::geom::geom_utility::p1::{
    line_closest_point, line_line_closest_point, plane_drop_point, plane_evaluate_point,
};
use crate::geom::geom_utility::p2::{frenet_from_derivatives, ContinuityType};
use crate::geom::global_solver::{
    Branch, GlobalSolve, SolutionRequested, SolveRequest, SolverOperation,
};
use crate::geom::pseudo_box::{Projection, PseudoBox};
use crate::geom::solution_array::{
    OperandObject, Solution, SolutionArray, SolutionObject, SolutionType, SortKey,
};
use crate::geom::tol::{are_same, is_zero, ANG_TOL_DEG};
use crate::geom::tree::{NodeData, Tree, TreeNode};
use crate::geom::vector3d::{Point3, Vec3};
use crate::kernel::Kernel;
use crate::math::{max2, min2, rad_to_deg, BIG_DOUBLE, EFF_ZERO, EFF_ZERO_SQ, EFF_ZERO_SQRT, PI};
use crate::nurbs::b_spline_curve::ProjectionType;

const OP_CCI: &str = "curve::curve::p7::CciGlobalSolver::local_solve_branch";
const OP_GPA: &str = "curve::curve::p7::Curve::global_property_analysis";

/// Number of comb samples of `GlobalPropertyAnalysis`.
const COMB_COUNT: usize = 150;

/// `pBox->ProjectToPlane(*pBox, ...)`: the box projected in place (status ignored;
/// a failed projection leaves what it wrote).
fn project_in_place(b: &mut PseudoBox, proj: &Projection) {
    let mut out = *b;
    let _ = b.project_to_plane_into(proj, &mut out);
    *b = out;
}

/// `SmExtent1d::operator==`: both ends the same within the scaled zero.
fn same_interval(a: &Extent1d, b: &Extent1d) -> bool {
    are_same(a.min(), b.min()) && are_same(a.max(), b.max())
}

/// Whether a continuity is at most `limit` (`<=` on the upstream enum values).
fn at_most(c: ContinuityType, limit: ContinuityType) -> bool {
    (c as i32) <= (limit as i32)
}

// ---- SmCCIGlobalSolver::LocalSolve --------------------------------------------------------

/// The leaf span of `node` in `tree` (`(SmBezierSpan*)pNode->m_pData`, `NER`).
fn leaf_span<O: Copy + PartialEq>(tree: &Tree<O>, node: &TreeNode<O>) -> Result<BezierSpan> {
    if matches!(node.data, NodeData::None) {
        return Err(SolidError::null_pointer(OP_CCI));
    }
    Ok(*tree.curve_span(node)?)
}

impl<'t, O: SolutionObject + 't> CciGlobalSolver<'t, O> {
    /// A two-variable single value solution between the branch's nodes naming both
    /// curves.
    fn new_solution(branch: &Branch, handles: [Option<O>; 2]) -> Solution<O> {
        let mut s = Solution::new();
        s.num_variables = 2;
        s.solution_type = SolutionType::SingleValue;
        s.nodes[0] = branch[0]; // Curve1 node
        s.nodes[1] = branch[1]; // Curve2 node
        s.num_objects = 2;
        s.objects[0] = handles[0];
        s.objects[1] = handles[1];
        s
    }

    /// `SmCCIGlobalSolver::LocalSolve` on the leaf spans `branch[0]` and `branch[1]`
    /// (always returns `rbNeedsMoreSubdivision` false). Spans whose (projected,
    /// tolerance expanded) pseudo boxes are disjoint, or that cannot cross in a
    /// self intersection, are skipped; [`SolverOperation::IntersectionTest`] only
    /// accumulates the overlapping boxes and intervals into one solution. Otherwise
    /// the spans are limited to the solver intervals (a trimmed end is
    /// discontinuous, a closed curve's ends take the curve's end continuity) and
    /// the span chords decide whether more than one intersection is possible:
    /// meeting discontinuous chord ends are an answer, coincident ranges and points
    /// come from `SimpleCoincidenceChecker`, curve ends are dropped onto the other
    /// span, and Newton intersections from guess points (more found near tangent
    /// crossings) are filtered and added.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` for a node without data; `InvalidInput` for a missing
    /// node, curve, kernel or projection vector, a node that is not a leaf span or
    /// an unsupported projection type; those of the evaluations, solvers and
    /// filters ([`SolidError::Unported`] from unported callees).
    pub fn local_solve_branch(&mut self, branch: &Branch) -> Result<bool> {
        // init output
        let needs_more_subdivision = false;

        // locals
        let curve_node1 = self.base.branch_node(branch, 0)?;
        let curve_node2 = self.base.branch_node(branch, 1)?;
        let tree1 = self.base.tree(0)?;
        let tree2 = self.base.tree(1)?;
        let handles = [tree1.owner_object()?, tree2.owner_object()?];
        let bez_span1 = leaf_span(tree1, curve_node1)?;
        let bez_span2 = leaf_span(tree2, curve_node2)?;
        let mut pseudo_box1 = *bez_span1.pseudo_box();
        let mut pseudo_box2 = *bez_span2.pseudo_box();
        let mut ivl1 = *bez_span1.interval();
        let mut ivl2 = *bez_span2.interval();
        let [parent_curve1, parent_curve2] = self
            .curves
            .ok_or(SolidError::invalid_input(OP_CCI, "solver without curves"))?;
        let k: &Kernel = self
            .kernel
            .ok_or(SolidError::invalid_input(OP_CCI, "solver without kernel"))?;
        let tol = self.base.tolerance_3d;
        let interval1 = self.interval1;
        let interval2 = self.interval2;

        // expand target interval 3d bounding boxes
        let mut expanded_box = pseudo_box1;
        expanded_box.expand_absolute(tol);

        // allow for projected solutions
        let mut projected: Option<[Curve; 2]> = None;

        // build ProjCrvs and projected PseudoBoxes for various projections
        if self.do_projection {
            let origin = Vec3::new(0.0, 0.0, 0.0);
            let aux_zero = Vec3::new(0.0, 0.0, 0.0);
            let (point, unit, aux, proj) = match self.projection_type {
                ProjectionType::Parallel => {
                    let v0 = self.base.opt_vector(0)?;
                    (
                        origin,
                        v0,
                        aux_zero,
                        Projection::Parallel {
                            point: origin,
                            normal: v0,
                        },
                    )
                }
                ProjectionType::Rotation => {
                    let (v0, v1, v2) = (
                        self.base.opt_vector(0)?,
                        self.base.opt_vector(1)?,
                        self.base.opt_vector(2)?,
                    );
                    (
                        v0,
                        v1,
                        v2,
                        Projection::Rotation {
                            point: v0,
                            axis: v1,
                            x_axis: v2,
                        },
                    )
                }
                ProjectionType::Perspective => {
                    let (v0, v1, v2) = (
                        self.base.opt_vector(0)?,
                        self.base.opt_vector(1)?,
                        self.base.opt_vector(2)?,
                    );
                    (
                        v0,
                        v1,
                        v2,
                        Projection::Perspective {
                            point: v0,
                            normal: v1,
                            eye: v2,
                        },
                    )
                }
                ProjectionType::Unknown => {
                    return Err(SolidError::invalid_input(
                        OP_CCI,
                        "unsupported projection type",
                    ));
                }
            };
            let make = |c: &Curve| {
                ProjectedCurve::new(
                    c.clone(),
                    Some(point),
                    Some(unit),
                    Some(aux),
                    self.projection_type,
                    OwnsCurve::No,
                )
                .map(Curve::from)
            };

            // pay attention - when working with projections, redefine Curve1 and
            // Curve2 to be the projected curves
            projected = Some([make(parent_curve1)?, make(parent_curve2)?]);

            // project the bounding boxes to the common plane
            project_in_place(&mut expanded_box, &proj);
            project_in_place(&mut pseudo_box1, &proj);
            project_in_place(&mut pseudo_box2, &proj);
        } // end looking for a projection solution

        let (curve1, curve2): (&Curve, &Curve) = match &projected {
            Some([a, b]) => (a, b),
            None => (parent_curve1, parent_curve2),
        };

        // do tight bounding check on PseudoBoxes - note: pseudoBoxes already
        // projected for projection problems
        if expanded_box.are_disjoint(&pseudo_box2, None) {
            return Ok(needs_more_subdivision);
        }

        // arrive here when pseudoBoxes intersect

        // low work - Eliminate self-intersection cases that can't intersect
        let mut adjacency: u64 = 0;
        let left1 = bez_span1.aux.left;
        let left2 = bez_span2.aux.left;
        let turn1 = bez_span1.aux.max_turning_angle_deg;
        let turn2 = bez_span2.aux.max_turning_angle_deg;
        if self.self_intersection {
            if curve_node1.data == curve_node2.data {
                return Ok(needs_more_subdivision);
            }
            if left1 == left2.wrapping_sub(1) {
                // 1 span just before 2
                adjacency = 1;
                if !at_most(bez_span1.aux.conts[1], ContinuityType::C0) && turn1 + turn2 < 360.0 {
                    return Ok(needs_more_subdivision);
                }
            }
            if left1 == left2.wrapping_add(1) {
                // 2 span just before 1
                adjacency = 2;
                if !at_most(bez_span1.aux.conts[0], ContinuityType::C0) && turn1 + turn2 < 360.0 {
                    return Ok(needs_more_subdivision);
                }
            }
        } // end m_bSelfIntersection == TRUE check

        // If we are just testing for intersection stop here and do the test.
        if self.base.operation == SolverOperation::IntersectionTest {
            let mut bbox = curve_node1.bbox;
            bbox.expand_absolute(tol);
            // (status ignored: a disjoint result stays Init())
            let mut result = bbox
                .intersect(&curve_node2.bbox)
                .unwrap_or_else(|_| Extent3d::new());

            // Store a union of the intersection 3D bounding box and UV box for
            // the intersections.
            if self.base.solutions.is_empty() {
                let mut sol = Self::new_solution(branch, handles);
                sol.num_variables = 7;
                set_param(&mut sol.start, 6, 1.0)?; // out: possible leafNode/leafNode intersection count
                set_param(&mut sol.start, 0, result.min().x)?;
                set_param(&mut sol.start, 1, result.min().y)?;
                set_param(&mut sol.start, 2, result.min().z)?;
                set_param(&mut sol.start, 3, result.max().x)?;
                set_param(&mut sol.start, 4, result.max().y)?;
                set_param(&mut sol.start, 5, result.max().z)?;
                set_param(&mut sol.end, 0, ivl1.min())?;
                set_param(&mut sol.end, 1, ivl1.max())?;
                set_param(&mut sol.end, 2, ivl2.min())?;
                set_param(&mut sol.end, 3, ivl2.max())?;

                // However, if the is only one subdivision (total) in each tree,
                // then we could well have multiple intersections, or coincidence,
                // and we won't be calling this again. [B162]
                if same_interval(&ivl1, &interval1) && same_interval(&ivl2, &interval2) {
                    set_param(&mut sol.start, 6, 2.0)?;
                }

                self.base.solutions.add(&sol);
            } else {
                // increment the possible leafNode/leafNode intersection total count
                let sol = self
                    .base
                    .solutions
                    .get_mut(0)
                    .ok_or(SolidError::invalid_input(OP_CCI, "missing first solution"))?;
                let count = sol.start.param(6)?;
                set_param(&mut sol.start, 6, count + 1.0)?; // Number of contributors to intersection

                // build previous intersection bounding box
                let old_box = Extent3d::from_points(
                    Vec3::new(
                        sol.start.param(0)?,
                        sol.start.param(1)?,
                        sol.start.param(2)?,
                    ),
                    Vec3::new(
                        sol.start.param(3)?,
                        sol.start.param(4)?,
                        sol.start.param(5)?,
                    ),
                );

                // union new intersection box with previous
                result = result.union(&old_box);

                // update output
                set_param(&mut sol.start, 0, result.min().x)?;
                set_param(&mut sol.start, 1, result.min().y)?;
                set_param(&mut sol.start, 2, result.min().z)?;
                set_param(&mut sol.start, 3, result.max().x)?;
                set_param(&mut sol.start, 4, result.max().y)?;
                set_param(&mut sol.start, 5, result.max().z)?;

                // build previous intersection interval bounds
                let old_ivl1 = Extent1d::from_min_max(sol.end.param(0)?, sol.end.param(1)?);
                let old_ivl2 = Extent1d::from_min_max(sol.end.param(2)?, sol.end.param(3)?);

                // union new intersection interval with previous
                ivl1 = old_ivl1.union(&ivl1);
                ivl2 = old_ivl2.union(&ivl2);

                // update output
                set_param(&mut sol.end, 0, ivl1.min())?;
                set_param(&mut sol.end, 1, ivl1.max())?;
                set_param(&mut sol.end, 2, ivl2.min())?;
                set_param(&mut sol.end, 3, ivl2.max())?;
            }

            // all done when just testing for possible intersections
            return Ok(needs_more_subdivision);
        } // end m_eSolverOperation == SM_SO_INTERSECTION_TEST check

        // Test for leafNode interval containment within given operation interval
        // and trim the Bezier intervals when they are not completely
        // contained by the processing intervals.
        if interval1.are_disjoint(&ivl1, 0.0) {
            return Ok(needs_more_subdivision);
        }
        if interval2.are_disjoint(&ivl2, 0.0) {
            return Ok(needs_more_subdivision);
        }

        let mut conts1 = bez_span1.aux.conts;
        let mut conts2 = bez_span2.aux.conts;

        // trim sIvl1 to m_crInterval1 - set trimmed endContinuities = SM_CT_DISCONTINUOUS
        if ivl1.min() <= interval1.min() {
            conts1[0] = ContinuityType::Discontinuous;
        }
        if ivl1.max() >= interval1.max() {
            conts1[1] = ContinuityType::Discontinuous;
        }

        if !ivl1.is_contained_by(&interval1, EFF_ZERO) {
            ivl1 = ivl1.intersect(&interval1)?;
            // Check for zero length intervals
            if are_same(ivl1.max(), ivl1.min()) {
                return Ok(needs_more_subdivision);
            }
        }

        // get sIvl1 endPoints and the chord between them
        let line_pt1 = point_or_undef(k, curve1, ivl1.min())?;
        let end1 = point_or_undef(k, curve1, ivl1.max())?;
        let line_vec1 = end1 - line_pt1;

        // trim sIvl2 to m_crInterval2 - set trimmed endContinuities = SM_CT_DISCONTINUOUS
        if ivl2.min() <= interval2.min() {
            conts2[0] = ContinuityType::Discontinuous;
        }
        if ivl2.max() >= interval2.max() {
            conts2[1] = ContinuityType::Discontinuous;
        }
        if !ivl2.is_contained_by(&interval2, EFF_ZERO) {
            ivl2 = ivl2.intersect(&interval2)?;
            // Check for zero length intervals
            if are_same(ivl2.max(), ivl2.min()) {
                return Ok(needs_more_subdivision);
            }
        }

        // get sIvl2 EndPoints and the chord between them
        let line_pt2 = point_or_undef(k, curve2, ivl2.min())?;
        let end2 = point_or_undef(k, curve2, ivl2.max())?;
        let line_vec2 = end2 - line_pt2;

        // adjust continuity for boundary endSpans on closed curves
        for (parent, ivl, crv_ivl, conts) in [
            (parent_curve1, ivl1, interval1, &mut conts1),
            (parent_curve2, ivl2, interval2, &mut conts2),
        ] {
            let parent_ivl = parent.natural_interval();

            // if     either end of span is at the curve's natural spanEnd
            //    and we are intersecting the entire curve
            //    and curve is closed
            if (is_zero(ivl.min() - parent_ivl.min()) || is_zero(ivl.max() - parent_ivl.max()))
                && (is_zero(crv_ivl.min() - parent_ivl.min())
                    && is_zero(crv_ivl.max() - parent_ivl.max()))
                && parent.is_closed(k, &parent_ivl, 0.0)?
            {
                // get the curve's end continuity (status ignored)
                let continuity = ignored(evaluate_continuity(
                    k,
                    parent,
                    parent_ivl.min(),
                    false,
                    parent,
                    parent_ivl.max(),
                    true,
                    ANG_TOL_DEG,
                ))?
                .map_or(ContinuityType::Discontinuous, |c| c.continuity);
                // set span endContinuities as appropriate
                if is_zero(ivl.min() - parent_ivl.min()) {
                    conts[0] = continuity;
                }
                if is_zero(ivl.max() - parent_ivl.max()) {
                    conts[1] = continuity;
                }
            }
        }

        // get angle between intervals as angle between interval EndPoint Chords
        // to determine when multiple intersections are possible
        let mut ang_rad = if line_vec1.length_squared() < EFF_ZERO_SQ
            || line_vec2.length_squared() < EFF_ZERO_SQ
        {
            PI / 2.0
        } else {
            line_vec1.angle_between(line_vec2)?
        };

        // mirror angles into the 1st quadrant
        if ang_rad > PI / 2.0 {
            ang_rad = PI - ang_rad;
        }

        let ang_deg = rad_to_deg(ang_rad);

        // get combined span turning angle
        let combined_turn_angle = turn1 + turn2;

        // multiple xsects possible when spanAngle is less than turning angle
        let mut only_one_intersection_possible;

        // Special case for two linear spans.
        if combined_turn_angle < EFF_ZERO * 100.0 {
            // Two 'lines'.  Either a single intersection, or a range solution.
            let len = min2(line_vec1.length(), line_vec2.length());
            let max_change = len * ang_rad;
            only_one_intersection_possible = max_change > tol;
        } else {
            // General case. [Fillet regression test 2:222]
            let scaled_zero = 100.0
                * EFF_ZERO
                * (1.0 + max2(line_vec1.max_dimension(), line_vec2.max_dimension()));
            only_one_intersection_possible = ang_deg * 0.99 > combined_turn_angle + scaled_zero;
        }

        // when selfIntersecting - cull cases with no intersections possible
        if self.self_intersection
            && (left1 == left2.wrapping_sub(1) || left1 == left2.wrapping_add(1))
            && only_one_intersection_possible
            && line_vec1.dot(line_vec2) > 0.0
        {
            // eliminate spans that run in the same direction - they can't intersect
            return Ok(needs_more_subdivision);
        }

        // Do a quick test to see if the the end points of nonZero length intervals
        // correspond in such a way that we don't need to do additional testing.
        let disc = ContinuityType::Discontinuous;
        if line_vec1.length_squared() >= EFF_ZERO_SQ
            && line_vec2.length_squared() >= EFF_ZERO_SQ
            && (conts1[0] == disc || conts1[1] == disc)
            && (conts2[0] == disc || conts2[1] == disc)
        {
            // for all endPoint combinations
            for (j1, c1) in conts1.iter().enumerate() {
                for (j2, c2) in conts2.iter().enumerate() {
                    if *c1 != disc || *c2 != disc {
                        continue;
                    }
                    // get span1 endPoint locals
                    let (pt1, t1, mut dir1) = if j1 == 0 {
                        (line_pt1, ivl1.min(), line_vec1)
                    } else {
                        (end1, ivl1.max(), -line_vec1)
                    };

                    // get span2 endPoint locals
                    let (pt2, t2, mut dir2) = if j2 == 0 {
                        (line_pt2, ivl2.min(), line_vec2)
                    } else {
                        (end2, ivl2.max(), -line_vec2)
                    };

                    // get distance between endPoints
                    let mut sol: Solution<O> = Solution::new();
                    sol.start.solution_value = pt1.distance(pt2);

                    // when endPoints are within tolerance of each other
                    if sol.start.solution_value < tol {
                        // look for cases where we know there are intersections
                        let mut test_one = false;

                        let degree1 = curve1.degree()?;
                        let degree2 = curve2.degree()?;
                        if only_one_intersection_possible
                            || (dir1.dot(dir2) < -EFF_ZERO && combined_turn_angle < 90.0)
                        {
                            test_one = true;
                        } else if degree1 <= 2 && degree2 <= 2 && !only_one_intersection_possible {
                            // We have a low total degree situation see if we can
                            // eliminate the need to go further in those situations
                            // where the curves are tangent to each other at the points.

                            // get span endPoint positions and tangents
                            let pv1: [Vec3; 2] = eval_n(k, curve1, t1, 1, true, true)?; // nonZeroTangent values
                            let pv2: [Vec3; 2] = eval_n(k, curve2, t2, 1, true, true)?; // nonZeroTangent values
                            let tan1 = pv1[1].unitize()?;
                            let tan2 = pv2[1].unitize()?;

                            // get span EndPoint and chord EndPoint tangent crossProducts
                            let cross = tan1.cross(tan2);
                            dir1 = dir1.unitize()?;
                            dir2 = dir2.unitize()?;
                            let cross2 = dir1.cross(dir2);

                            // when both sets of tangents are parallel
                            if cross.length_squared() < EFF_ZERO_SQ
                                && cross2.length_squared() > EFF_ZERO_SQ
                            {
                                test_one = true;
                            }
                        } // end low degree, multiple intersection case check

                        // when these endPoints
                        if test_one {
                            let value = sol.start.solution_value;
                            sol = Self::new_solution(branch, handles);
                            sol.start.solution_value = value;
                            set_param(&mut sol.start, 0, t1)?;
                            set_param(&mut sol.start, 1, t2)?;

                            // get closest point between 2 lines
                            match line_line_closest_point(line_pt1, line_vec1, line_pt2, line_vec2)
                            {
                                Ok((param1, param2)) => {
                                    // If the intersection occurs at or beyond the
                                    // interval ends then the curve ends are to be
                                    // the intersection.  Otherwise we have to
                                    // continue and do iteration.
                                    if (param1 < EFF_ZERO_SQRT || param1 > 1.0 - EFF_ZERO_SQRT)
                                        && (param2 < EFF_ZERO_SQRT || param2 > 1.0 - EFF_ZERO_SQRT)
                                    {
                                        self.add_sorted_solution(
                                            &sol,
                                            SortKey::ByAllParameters,
                                            true,
                                        )?;
                                        return Ok(needs_more_subdivision);
                                    }
                                }
                                Err(e) if e.is_unported() => return Err(e),
                                Err(_) => {
                                    if dir1.dot(dir2) < EFF_ZERO {
                                        // Curves are parallel and point in opposite
                                        // direction. do a line test/point test.
                                        let param1 = line_closest_point(line_pt1, line_vec1, pt2);
                                        if param1 < EFF_ZERO || param1 > 1.0 - EFF_ZERO {
                                            // there is a small gap between these two
                                            // intervals - count as an intersection
                                            self.add_sorted_solution(
                                                &sol,
                                                SortKey::ByAllParameters,
                                                true,
                                            )?;
                                            return Ok(needs_more_subdivision);
                                        }
                                    }
                                }
                            }
                        } // end bTestOne == TRUE check
                    } // end endPoints are closer than tolerance apart check
                } // end iter both span2 endPoints
            } // end iter both span1 EndPoints
        } // end both spans are nonZero length check

        // Do a coincidence test if more than one intersection is possible
        if !only_one_intersection_possible {
            // find coincident point or range(s)
            let mut solutions = curve1.simple_coincidence_checker(
                k,
                &ivl1,
                curve2,
                &ivl2,
                tol,
                Some(&pseudo_box1),
                Some(&pseudo_box2),
                handles,
            )?;

            // when a coincident solution was found (either point or range)
            let mut found_coin = false;
            for sol in &mut solutions {
                sol.num_variables = 2;
                sol.nodes[0] = branch[0]; // Curve1 node
                sol.nodes[1] = branch[1]; // Curve2 node
                sol.num_objects = 2;
                sol.objects[0] = handles[0];
                sol.objects[1] = handles[1];

                // always save range solutions
                if sol.solution_type == SolutionType::RangeOfValues {
                    found_coin = true;
                    self.add_sorted_solution(sol, SortKey::ByAllParameters, true)?;
                } else {
                    // Single valued solutions: find the closest approach. [B606 xhatch]
                    if sol.start.solution_value > tol / 100.0 {
                        // just rule out really tight ones.
                        let found = local_curve_intersect(
                            k,
                            curve1,
                            &ivl1,
                            curve2,
                            &ivl2,
                            tol,
                            sol.start.param(0)?,
                            sol.start.param(1)?,
                        )?;
                        if let Some(isect) = found {
                            if isect.deviation < sol.start.solution_value {
                                set_param(&mut sol.start, 0, isect.my_param)?;
                                set_param(&mut sol.start, 1, isect.other_param)?;
                                sol.start.solution_value = isect.deviation;
                            }
                        }
                    }

                    // Now check the refined solution.
                    let keep = filter_solution(
                        k, curve1, &ivl1, curve2, &ivl2, conts1, conts2, adjacency, sol,
                    )?;

                    // save aSolutions, which should be kept, to m_pSolutions
                    if keep {
                        self.add_sorted_solution(sol, SortKey::ByAllParameters, true)?;
                    }
                }
            }

            // If we found a coincidence we are done
            if found_coin {
                return Ok(needs_more_subdivision);
            }
        } // end multiple intersection possible check

        // Do a little testing to see if we are just barely intersecting
        // each other at a corner point or something
        if !only_one_intersection_possible && !self.do_projection {
            if curve_node1.bbox.are_disjoint(&curve_node2.bbox, 0.0) {
                // Temporarily remove this optimization because it causes problems
                // when curves are disjoint by a plane not a corner.
                only_one_intersection_possible = false;
            } else if let Ok(int_bbox) = curve_node1.bbox.intersect(&curve_node2.bbox) {
                // If all three dimensions are zero then only one possible intersection.
                let mut num_zero_dimensions = 0;
                let dim_tol = EFF_ZERO * (1.0 + int_bbox.max().max_dimension());
                if int_bbox.max().x - int_bbox.min().x < dim_tol {
                    num_zero_dimensions += 1;
                }
                if int_bbox.max().y - int_bbox.min().y < dim_tol {
                    num_zero_dimensions += 1;
                }
                if int_bbox.max().z - int_bbox.min().z < dim_tol {
                    num_zero_dimensions += 1;
                }
                if num_zero_dimensions >= 3 {
                    only_one_intersection_possible = true;
                }
            }
        } // end multiple intersections and not doingProjections check

        // look for curve EndPoint/span intersections when span comes from end of
        // curve by checking nearest distance from each spanEndPoint to the other
        // span whenever those span endPoints are discontinuous.
        let mut found_end_answer = false;
        let mut solution2 = Self::new_solution(branch, handles);
        let mut end_point_solutions: Vec<Solution<O>> = Vec::new();
        let opt_vectors = self.base.opt_vectors.clone();
        let point_opts = PointSolveOptions {
            distance_tolerance: Some(tol),
            target_distance: None,
            vectors: if opt_vectors.is_empty() {
                None
            } else {
                Some(&opt_vectors)
            },
        };

        // note: curve1 and curve2 are curves being intersected after optional
        //       projection; the parent curves before.
        // (end point of span 1 / 2, which end, the other span's curve, interval,
        // box, chord)
        let ends = [
            (
                at_most(conts1[0], ContinuityType::C0),
                line_pt1,
                true,
                false,
            ),
            (
                at_most(conts1[1], ContinuityType::Discontinuous),
                end1,
                true,
                true,
            ),
            (
                at_most(conts2[0], ContinuityType::C0),
                line_pt2,
                false,
                false,
            ),
            (
                at_most(conts2[1], ContinuityType::Discontinuous),
                end2,
                false,
                true,
            ),
        ];
        for (is_curve_end, end_pt, on_span1, at_max) in ends {
            let (other_curve, other_ivl, other_box, line_pt, line_vec) = if on_span1 {
                (curve2, ivl2, &pseudo_box2, line_pt1, line_vec1)
            } else {
                (curve1, ivl1, &pseudo_box1, line_pt2, line_vec2)
            };
            if !(is_curve_end && other_box.distance_to_point(end_pt) < tol) {
                continue;
            }

            // find min distance between the other span and this span end point
            // (status ignored)
            let (found_answer, point_solution): (bool, Solution<O>) =
                ignored(other_curve.local_point_solve(
                    k,
                    &other_ivl,
                    SolverOperation::Minimize,
                    end_pt,
                    &point_opts,
                    other_ivl.evaluate(0.5),
                ))?
                .unwrap_or((false, Solution::new()));

            // when the end point is within tolerance
            if !(found_answer && point_solution.start.solution_value < tol) {
                continue;
            }
            let other_t = point_solution.start.param(0)?;

            // when end point is tightly within tolerance or closest other span point
            // is in the interior
            let tight = point_solution.start.solution_value < tol / 100.0;
            let interior = !other_ivl.is_value_on_boundary(other_t, EFF_ZERO).0;
            let accept = tight || interior;
            if !accept {
                continue;
            }

            // add this end point/span solution to m_pSolutions array
            solution2.start.solution_value = point_solution.start.solution_value;
            let own_t = if on_span1 {
                if at_max {
                    ivl1.max()
                } else {
                    ivl1.min()
                }
            } else if at_max {
                ivl2.max()
            } else {
                ivl2.min()
            };
            if on_span1 {
                set_param(&mut solution2.start, 0, own_t)?;
                set_param(&mut solution2.start, 1, other_t)?;
            } else {
                set_param(&mut solution2.start, 0, other_t)?;
                set_param(&mut solution2.start, 1, own_t)?;
            }
            found_end_answer = true;

            // look for exit conditions
            if only_one_intersection_possible {
                let crv_pnt = other_curve.evaluate_point(k, other_t)?;
                if line_vec.length_squared() < EFF_ZERO_SQ {
                    self.add_sorted_solution(&solution2, SortKey::ByAllParameters, true)?;
                    return Ok(needs_more_subdivision);
                }
                let t = line_closest_point(line_pt, line_vec, crv_pnt);
                if t < EFF_ZERO || t > 1.0 - EFF_ZERO {
                    self.add_sorted_solution(&solution2, SortKey::ByAllParameters, true)?;
                    return Ok(needs_more_subdivision);
                }
            } // end looking for exit conditions

            // accumulate this solution for possible later use
            end_point_solutions.push(solution2);
        }

        // If only one segment possible then choose an intelligent guess point.
        let mut guess1: Vec<f64> = Vec::with_capacity(10);
        let mut guess2: Vec<f64> = Vec::with_capacity(10);
        if only_one_intersection_possible {
            // Just take a midpoint if this fails
            let (t1, t2) = ignored(line_line_closest_point(
                line_pt1, line_vec1, line_pt2, line_vec2,
            ))?
            .unwrap_or((0.5, 0.5));
            let norm_ivl = Extent1d::from_min_max(0.0, 1.0);
            guess1.push(ivl1.evaluate(norm_ivl.clamp_value(t1)));
            guess2.push(ivl2.evaluate(norm_ivl.clamp_value(t2)));
        } else {
            // multiple intersections are possible - try an array of guess points:
            // the mid point, then the end points
            for (a, b) in [(0.5, 0.5), (0.0, 0.0), (0.0, 1.0), (1.0, 0.0), (1.0, 1.0)] {
                guess1.push(ivl1.evaluate(a));
                guess2.push(ivl2.evaluate(b));
            }
        }

        // exit condition: when either curve is degenerate
        if curve1.is_degenerate(k, EFF_ZERO, None)? || curve2.is_degenerate(k, EFF_ZERO, None)? {
            // to preserve previous behavior - add in all accumulated EndPointSolutions
            // (upstream adds the last one built, once per accumulated solution)
            for _ in 0..end_point_solutions.len() {
                self.add_sorted_solution(&solution2, SortKey::ByAllParameters, true)?;
            }
            return Ok(needs_more_subdivision);
        }

        let mut added_solution = false;

        // for every guess point
        let mut i = 0;
        while let (Some(&g1), Some(&g2)) = (guess1.get(i), guess2.get(i)) {
            // execute span/span NewtonRaphson Intersection
            let found = local_curve_intersect(k, curve1, &ivl1, curve2, &ivl2, tol, g1, g2)?;

            // when a solution was found
            if let Some(isect) = found {
                let my_parameter = isect.my_param;
                let other_parameter = isect.other_param;
                let deviation = isect.deviation;

                // place the solution into an SmSolution object.
                let mut solution = Self::new_solution(branch, handles);
                set_param(&mut solution.start, 0, my_parameter)?;
                set_param(&mut solution.start, 1, other_parameter)?;
                solution.start.solution_value = deviation;

                // Filter out intersections which are actually outside of the boundary
                // of the curve and are not of the tangent sort
                let keep = filter_solution(
                    k, curve1, &ivl1, curve2, &ivl2, conts1, conts2, adjacency, &solution,
                )?;

                // Filter out the nasty end case where the intersection converges to
                // the end of the curve but there is a minimization of a point
                // intersection which is closer near the end.
                let nasty_end = ivl1.is_value_on_boundary(my_parameter, EFF_ZERO).0
                    && ivl2.is_value_on_boundary(other_parameter, EFF_ZERO).0
                    && found_end_answer
                    && deviation > tol / 100.0;

                if keep && !nasty_end {
                    self.add_sorted_solution(&solution, SortKey::ByAllParameters, true)?;
                    added_solution = true;

                    // when multiple solutions are possible and more iterations are coming
                    if i < 5 && !only_one_intersection_possible {
                        // see if additional tangent guessPoints can be generated
                        if let Some((new_my, new_other)) = find_additional_cc_tangent_guess(
                            k,
                            curve1,
                            &ivl1,
                            curve2,
                            &ivl2,
                            my_parameter,
                            other_parameter,
                        )? {
                            guess1.push(new_my);
                            guess2.push(new_other);
                        }
                    }
                }
            }
            i += 1;
        } // end iter every guess point

        // add in accumulated endPoint solutions when
        // no actual intersections were found or allowing for multiple solutions
        if !added_solution || !only_one_intersection_possible {
            for sol in &end_point_solutions {
                self.add_sorted_solution(sol, SortKey::ByAllParameters, true)?;
            }
        }

        // all done
        Ok(needs_more_subdivision)
    }
}

// ---- SmCurve::GlobalPropertyAnalysis ------------------------------------------------------

/// `EvaluateGeometric(t, 3, TRUE, sGeom)` with its status ignored: the outputs
/// `smgu_FrenetFromDerivatives` wrote before failing (position and tangent, then the
/// curvature vector, else zero, and a zero torsion vector); [`Vec3::UNDEF`] (the
/// uninitialised array) when the evaluation itself fails.
fn evaluate_geometric_partial(k: &Kernel, curve: &Curve, t: f64) -> Result<[Vec3; 4]> {
    let mut geom = [Vec3::UNDEF; 4];
    let Some(vals) = ignored(curve.evaluate(k, t, 3, true, false))? else {
        return Ok(geom);
    };
    let fill = |geom: &mut [Vec3; 4], v: Vec<Vec3>| {
        for (d, s) in geom.iter_mut().zip(v) {
            *d = s;
        }
    };
    if let Some(v) = ignored(frenet_from_derivatives(&vals, 3))? {
        fill(&mut geom, v);
        return Ok(geom);
    }
    // failed past the tangent: [2] and [3] were zeroed, [2] set when the tangent
    // is not zero (the two-vector prefix of the same computation)
    geom = [at(&vals, 0), at(&vals, 1), Vec3::ZERO, Vec3::ZERO];
    if let Some(v) = ignored(frenet_from_derivatives(&vals, 2))? {
        fill(&mut geom, v);
    }
    Ok(geom)
}

/// The optional inputs and outputs of [`Curve::global_property_analysis`].
#[derive(Debug, Clone, Copy)]
pub struct PropertyAnalysisOptions<'a, O> {
    /// `cpdPropertyValue`: the value some properties need (`None` = null).
    pub property_value: Option<f64>,
    /// `cpOptVectors`: the points and vectors some properties need.
    pub opt_vectors: &'a [Vec3],
    /// Whether to build the check comb (`pOptCombPoints` and `pOptCombVecs` not
    /// null).
    pub want_comb: bool,
    /// The curve's handle in the caller's object space, named by the solutions.
    /// With `None` the solutions name no object, but are merged as upstream's
    /// (which name the curve).
    pub handle: Option<O>,
}

impl<O> Default for PropertyAnalysisOptions<'_, O> {
    fn default() -> Self {
        Self {
            property_value: None,
            opt_vectors: &[],
            want_comb: false,
            handle: None,
        }
    }
}

/// Result of [`Curve::global_property_analysis`].
#[derive(Debug, Clone)]
pub struct PropertyAnalysis<O> {
    /// `rSolutions`: the solutions found.
    pub solutions: SolutionArray<O>,
    /// `*pOptCombPoints`: the comb's curve sample points (empty without a comb).
    pub comb_points: Vec<Point3>,
    /// `*pOptCombVecs`: the comb's tines, proportional to the property being zeroed
    /// (empty without a comb).
    pub comb_vecs: Vec<Vec3>,
}

impl Curve {
    /// `GlobalPropertyAnalysis`: the points of the curve over `interval` with the
    /// property `property` (see [`CurvePropertyType`] for the values and vectors
    /// each needs), within `dist_tol`. A line, or a planar curve whose plane is
    /// normal to the vector, has no perpendicular-to-vector solution;
    /// [`CurvePropertyType::PlaneClassify`] gives the ranges between the plane
    /// intersections whose middle lies above the plane; every other property is
    /// solved over the spans of the curve's cache tree by the curve property global
    /// solver ([`CurvePropertyGs`]). With [`PropertyAnalysisOptions::want_comb`] the
    /// 150-sample check comb of the property is built too.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` when the cache cannot be built; `InvalidInput` for a
    /// missing property value or vector; those of the global solve and of the
    /// evaluations ([`SolidError::Unported`] from unported callees).
    pub fn global_property_analysis<O: SolutionObject>(
        &self,
        k: &Kernel,
        interval: &Extent1d,
        property: CurvePropertyType,
        dist_tol: f64,
        opts: &PropertyAnalysisOptions<'_, O>,
    ) -> Result<PropertyAnalysis<O>> {
        let mut out = PropertyAnalysis {
            solutions: SolutionArray::new(),
            comb_points: Vec::new(),
            comb_vecs: Vec::new(),
        };
        self.global_property_analysis_into(k, interval, property, dist_tol, opts, &mut out)?;
        Ok(out)
    }

    /// `GlobalPropertyAnalysis` with outputs retained even when a later solve fails.
    /// For upstream callers that ignore its status but inspect partial solutions.
    pub(crate) fn global_property_analysis_into<O: SolutionObject>(
        &self,
        k: &Kernel,
        interval: &Extent1d,
        property: CurvePropertyType,
        dist_tol: f64,
        opts: &PropertyAnalysisOptions<'_, O>,
        out: &mut PropertyAnalysis<O>,
    ) -> Result<()> {
        type P = CurvePropertyType;

        // no handle: upstream's solutions still name this curve (whose natural
        // interval IdenticalSolutions reads); the caller gets them without objects
        if opts.handle.is_none() {
            let inner = PropertyAnalysisOptions {
                property_value: opts.property_value,
                opt_vectors: opts.opt_vectors,
                want_comb: opts.want_comb,
                handle: Some(OperandObject::Curve(0, self.natural_interval())),
            };
            let mut own = PropertyAnalysis {
                solutions: SolutionArray::new(),
                comb_points: Vec::new(),
                comb_vecs: Vec::new(),
            };
            let r = self
                .global_property_analysis_into(k, interval, property, dist_tol, &inner, &mut own);
            *out = PropertyAnalysis {
                solutions: own.solutions.without_objects(),
                comb_points: own.comb_points,
                comb_vecs: own.comb_vecs,
            };
            return r;
        }

        // init output
        *out = PropertyAnalysis {
            solutions: SolutionArray::new(),
            comb_points: Vec::new(),
            comb_vecs: Vec::new(),
        };
        let vecs = opts.opt_vectors;
        let opt = |i: usize| -> Result<Vec3> {
            vecs.get(i)
                .copied()
                .ok_or(SolidError::invalid_input(OP_GPA, "missing optional vector"))
        };
        let value = || -> Result<f64> {
            opts.property_value
                .ok_or(SolidError::invalid_input(OP_GPA, "missing property value"))
        };

        // make sure curve has a cache and get its decomposition tree
        let cache = match make_curve_cache::<O>(k, self, opts.handle) {
            Ok(c) => c,
            Err(e) if e.is_unported() => return Err(e),
            Err(_) => return Err(SolidError::null_pointer(OP_GPA)),
        };
        let curve_tree = cache.tree().ok_or(SolidError::null_pointer(OP_GPA))?;

        // special case SM_CP_PERPENDICULAR_TO_VECTOR
        // quit when curve is a line or when curve is in plane perp to target vector
        if property == P::PerpendicularToVector {
            // For a line, either every point or no point is a solution.
            if self.is_linear(k, EFF_ZERO, false)?.linear {
                return Ok(());
            }

            let planar = self.is_planar(k, EFF_ZERO)?;
            if planar.is_planar {
                // (status ignored: a failed angle is 0)
                let angle = planar.normal.angle_between(opt(0)?).unwrap_or(0.0);
                if angle < EFF_ZERO {
                    return Ok(());
                }
            }
        } // end perp-to-vector check

        // special case SM_CP_PLANE_CLASSIFY
        if property == P::PlaneClassify {
            // To classify a curve relative to a plane first intersect it with the
            // plane and then classify each segment between the intersection points.
            let plane_d = value()?;
            let plane_abc = opt(0)?;
            let inner = PropertyAnalysisOptions {
                want_comb: false,
                ..*opts
            };
            let sols = self
                .global_property_analysis(k, interval, P::PlaneIntersection, dist_tol, &inner)?
                .solutions;

            // Now Classify each segment relative to the side of the plane
            // and keep segments which are on + side of plane.
            let n = sols.len();
            for i in 0..=n {
                let t1 = match i.checked_sub(1).and_then(|j| sols.get(j)) {
                    None => interval.min(),
                    Some(prev) if prev.solution_type == SolutionType::RangeOfValues => {
                        prev.end.param(0)?
                    }
                    Some(prev) => prev.start.param(0)?,
                };
                let t2 = match sols.get(i) {
                    None => interval.max(),
                    Some(s) => s.start.param(0)?,
                };

                let t_mid = (t1 + t2) / 2.0;
                let mid_pnt = self.evaluate_point(k, t_mid)?;
                let val = mid_pnt.dot(plane_abc) + plane_d;
                if val > dist_tol {
                    let mut sol: Solution<O> = Solution::new();
                    sol.solution_type = SolutionType::RangeOfValues;
                    set_param(&mut sol.start, 0, t1)?;
                    set_param(&mut sol.end, 0, t2)?;
                    out.solutions.add(&sol);
                }
            }
        } else {
            // Set up global solver to solve single variable.
            let mut gs = CurvePropertyGs::new(
                k,
                curve_tree,
                *interval,
                property,
                opts.property_value,
                vecs.to_vec(),
            )
            .with_curve(self);

            // assign solver op based on eCurveProperty
            let op = match property {
                P::MinimizeFirstDerivative
                | P::MaximizeFirstDerivative
                | P::MinimizeRadiusOfCurvature
                | P::MaximizeRadiusOfCurvature
                | P::MinimizeCcwAngle
                | P::MinimizeAngleToPlane
                | P::ProjectedPointMinimize => SolverOperation::Minimize, // Maximization actually minimizes 1/val.
                P::MinimizeDirectedAngle => SolverOperation::SignedDirectedMinimize,
                P::ProjectedPointMaximize => SolverOperation::Maximize,
                _ => SolverOperation::Find,
            };

            // get the solution
            let request = SolveRequest {
                operation: op,
                solution_requested: SolutionRequested::All,
                tolerance_3d: dist_tol,
                best_answer_so_far_sq: BIG_DOUBLE,
                opt_vectors: &[],
            };
            gs.solve_it(&request, &mut out.solutions)?;
        }

        // when asked - build comb of property being minimized
        if opts.want_comb {
            let mut max = 0.0;
            let mut do_scaling = false;

            for ii in 0..COMB_COUNT {
                let t = interval.evaluate(ii as f64 / COMB_COUNT as f64);

                // for most properties - evaluate with no zero length tangents
                // for some - work with exact tangent values (statuses ignored)
                let exact = matches!(
                    property,
                    P::FirstDerivativeLength
                        | P::MinimizeFirstDerivative
                        | P::MaximizeFirstDerivative
                        | P::MinimizeRadiusOfCurvature
                        | P::MaximizeRadiusOfCurvature
                );
                let mut pv: [Vec3; 3] =
                    ignored(eval_n(k, self, t, 2, true, !exact))?.unwrap_or([Vec3::UNDEF; 3]);
                let geom = evaluate_geometric_partial(k, self, t)?;
                out.comb_points.push(pv[0]);

                let speed = pv[1].length();
                let kappa = geom[2].length();

                let degenerate = speed < EFF_ZERO;
                let linear = kappa < EFF_ZERO;

                let dir = if degenerate {
                    Vec3::new(0.0, 0.0, 1.0)
                } else if linear {
                    // (status ignored: the Y axis output stays uninitialised)
                    geom[1]
                        .make_unit_ortho_vectors(None)
                        .map_or(Vec3::UNDEF, |[_, y, _]| y)
                } else {
                    geom[2] / kappa
                };

                // sPV : position, 1st and 2nd derivative
                // sGeom: position, 1st derivative, curvature vector, torsion vector
                let mut scaled = |v: Vec3, out: &mut Vec<Vec3>| {
                    out.push(v);
                    do_scaling = true;
                    if max < v.length() {
                        max = v.length();
                    }
                };
                match property {
                    P::PerpendicularToVector => {
                        let _tgt = opt(0)?.unitize_or_z();
                        pv[1] = pv[1].unitize_or_z();
                        let v = opt(0)?.dot(pv[1]) * dir;
                        out.comb_vecs.push(v);
                    }
                    P::InflectionPoints => {
                        let mut v = pv[2].length() * dir;
                        let size = v.length();
                        if size > 1.0 {
                            v /= size;
                        }
                        out.comb_vecs.push(v);
                    }
                    P::RadiusOfCurvature => {
                        let v = dir * (kappa - value()?);
                        scaled(v, &mut out.comb_vecs);
                    }
                    P::FirstDerivativeLength => {
                        let v = dir * (speed - value()?);
                        scaled(v, &mut out.comb_vecs);
                    }
                    P::ParallelToVector => {
                        let _tgt = opt(0)?.unitize_or_z();
                        pv[1] = pv[1].unitize_or_z();
                        let v = opt(0)?.cross(pv[1]).length() * dir;
                        out.comb_vecs.push(v);
                    }
                    P::SilhouetteVector => {
                        let mut v = pv[1].cross(pv[2]).cross(pv[1]);
                        let length = v.length();
                        if length > EFF_ZERO {
                            v /= length;
                        }
                        let size = v.dot(opt(0)?);
                        out.comb_vecs.push(dir * size);
                    }
                    P::SilhouettePoint => {
                        let mut v = pv[1].cross(pv[2]).cross(pv[1]);
                        let length = v.length();
                        if length > EFF_ZERO {
                            v /= length;
                        }
                        let mut size = v.dot(pv[0] - opt(0)?);
                        // trim vectors for better image
                        size = size.clamp(-1.0, 1.0);
                        out.comb_vecs.push(dir * size);
                    }
                    P::XNormal => scaled(dir * geom[1].x, &mut out.comb_vecs),
                    P::YNormal => scaled(dir * geom[1].y, &mut out.comb_vecs),
                    P::ZNormal => scaled(dir * geom[1].z, &mut out.comb_vecs),
                    P::XyzNormal => scaled(dir * geom[1].min_dimension(), &mut out.comb_vecs),
                    P::MinimizeFirstDerivative | P::MaximizeFirstDerivative => {
                        scaled(dir * geom[1].length(), &mut out.comb_vecs);
                    }
                    P::MinimizeRadiusOfCurvature | P::MaximizeRadiusOfCurvature => {
                        scaled(geom[2], &mut out.comb_vecs);
                    }
                    P::PlaneClassify | P::PlaneIntersection => {
                        let plane = Axis2Placement::from_plane(opt(0)?, value()?);
                        let uv = plane_drop_point(&plane, pv[0]);
                        let point = plane_evaluate_point(&plane, uv);
                        out.comb_vecs.push(point - pv[0]);
                    }
                    P::MinimizeAngleToPlane | P::MinimizeDirectedAngle | P::MinimizeCcwAngle => {
                        let cp_x_n = pv[1].cross(opt(1)?);
                        let c_orig = pv[0] - opt(0)?;
                        let size = cp_x_n.dot(c_orig);
                        scaled(dir * size, &mut out.comb_vecs);
                    }
                    P::ProjectedPointMinimize | P::ProjectedPointMaximize => {
                        let (o0, o1) = (opt(0)?, opt(1)?);
                        let v = pv[0] - o0;
                        let v_proj = o1.cross(v).cross(o1);
                        let tan_proj = o1.cross(pv[1]).cross(o1);
                        let size = tan_proj.dot(v_proj);
                        scaled(geom[2] * size, &mut out.comb_vecs);
                    }
                    P::CylinderIntersection => {
                        let cyl_radius = value()?;
                        let center = opt(0)?;
                        let n = opt(1)?;
                        let cf = n.cross(pv[0] - center).cross(n);
                        let size = cf.dot(cf) - cyl_radius * cyl_radius;
                        out.comb_vecs.push(geom[2] * size);
                    }
                    P::DistanceToPoint => {
                        let cp = pv[0] - opt(0)?;
                        let d = value()?;
                        let size = cp.dot(cp) - d * d;
                        out.comb_vecs.push(geom[2] * size);
                    }
                    P::TangentToCircle => {
                        let (o0, o1) = (opt(0)?, opt(1)?);
                        let diff = pv[0] - o0;
                        let mag = pv[1].length();
                        let flip = match line_line_closest_point(o0, o1, pv[0], pv[1]) {
                            Ok((t0, t1)) => t0 * t1 < 0.0, // If opposite signs, flip.
                            Err(e) if e.is_unported() => return Err(e),
                            Err(_) => {
                                // Curve tangent is either parallel to given vector,
                                // or zero.
                                let dot = pv[1].dot(o1);
                                dot < 0.0
                            }
                        };

                        // Now the possibly-flipped vector:
                        let tan = if flip { -o1 } else { o1 };

                        // Ok, we can finally calculate the function and derivative.
                        let tan_sum = pv[1] + mag * tan;
                        let size = diff.dot(tan_sum);
                        out.comb_vecs.push(geom[2] * size);
                    }
                    P::PointInversion => {
                        let cp = pv[0] - opt(0)?;
                        let size = cp.dot(cp);
                        out.comb_vecs.push(geom[2] * size);
                    }
                    P::MaximaToLine => {
                        let p = opt(0)?;
                        let v = opt(1)?.unitize_or_z();
                        pv[1] = pv[1].unitize_or_z();
                        let size = v.dot(v.cross(v.cross(pv[1] - p)));
                        out.comb_vecs.push(size * dir);
                    }
                    P::MaximaToPlane => {
                        let _tgt = opt(1)?.unitize_or_z();
                        pv[1] = pv[1].unitize_or_z();
                        let v = opt(0)?.dot(pv[1]) * dir;
                        out.comb_vecs.push(v);
                    }
                    P::ProjectedTangentThroughPoint => {}
                } // end switch on eCurveProperty
            } // end iter every comb point

            if do_scaling && max > EFF_ZERO {
                for v in &mut out.comb_vecs {
                    *v /= max;
                }
            }
        } // end pOptCombPoints && pOptCombVecs existence check

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::curve::p2::CciGlobalSolver;
    use crate::curve::hermite_curve::HermiteCurve;
    use crate::geom::solution_array::ObjectKind;

    #[derive(Debug, Clone, Copy, PartialEq)]
    struct Obj(i64);

    impl SolutionObject for Obj {
        fn kind(&self) -> ObjectKind {
            ObjectKind::Other
        }
    }

    fn hermite(p1: [f64; 3], d1: [f64; 3], p2: [f64; 3], d2: [f64; 3]) -> Curve {
        Curve::from(HermiteCurve::new(
            Vec3::from_array(p1),
            Vec3::from_array(d1),
            Vec3::from_array(p2),
            Vec3::from_array(d2),
            3,
        ))
    }

    #[test]
    fn crossing_curves_intersect_once() {
        let k = Kernel::default();
        let a = hermite(
            [0.0, 0.0, 0.0],
            [2.0, 0.0, 0.0],
            [2.0, 0.0, 0.0],
            [2.0, 0.0, 0.0],
        );
        let b = hermite(
            [1.0, -1.0, 0.0],
            [0.0, 2.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 2.0, 0.0],
        );
        let ca = make_curve_cache::<Obj>(&k, &a, Some(Obj(0))).unwrap();
        let cb = make_curve_cache::<Obj>(&k, &b, Some(Obj(1))).unwrap();
        let (ta, tb) = (ca.tree().unwrap(), cb.tree().unwrap());
        let mut gs = CciGlobalSolver::new(
            ta,
            tb,
            a.natural_interval(),
            b.natural_interval(),
            false,
            ProjectionType::Parallel,
        )
        .with_curves(&a, &b, &k);
        let mut sols = SolutionArray::new();
        let req = SolveRequest {
            operation: SolverOperation::Intersect,
            tolerance_3d: 1e-6,
            ..SolveRequest::default()
        };
        // (the Newton intersection reaches SmCCIntersectENFO, unit curve.curve_p5)
        if let Err(e) = gs.solve_it(&req, &mut sols) {
            assert!(e.is_unported());
            return;
        }
        assert_eq!(sols.len(), 1);
        let s = sols.get(0).unwrap();
        assert!((s.start.parameters[0] - 0.5).abs() < 1e-8);
        assert!((s.start.parameters[1] - 0.5).abs() < 1e-8);
        assert_eq!(s.objects[0], Some(Obj(0)));
        assert_eq!(s.objects[1], Some(Obj(1)));
    }

    #[test]
    fn intersection_test_counts_overlapping_spans() {
        let k = Kernel::default();
        let a = hermite(
            [0.0, 0.0, 0.0],
            [2.0, 0.0, 0.0],
            [2.0, 0.0, 0.0],
            [2.0, 0.0, 0.0],
        );
        let b = hermite(
            [1.0, -1.0, 0.0],
            [0.0, 2.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 2.0, 0.0],
        );
        let ca = make_curve_cache::<Obj>(&k, &a, None).unwrap();
        let cb = make_curve_cache::<Obj>(&k, &b, None).unwrap();
        let mut gs = CciGlobalSolver::new(
            ca.tree().unwrap(),
            cb.tree().unwrap(),
            a.natural_interval(),
            b.natural_interval(),
            false,
            ProjectionType::Parallel,
        )
        .with_curves(&a, &b, &k);
        let mut sols = SolutionArray::new();
        let req = SolveRequest {
            operation: SolverOperation::IntersectionTest,
            tolerance_3d: 1e-6,
            ..SolveRequest::default()
        };
        gs.solve_it(&req, &mut sols).unwrap();
        assert_eq!(sols.len(), 1);
        assert!(sols.get(0).unwrap().start.parameters[6] >= 1.0);
    }

    #[test]
    fn far_apart_curves_do_not_intersect() {
        let k = Kernel::default();
        let a = hermite(
            [0.0, 0.0, 0.0],
            [2.0, 0.0, 0.0],
            [2.0, 0.0, 0.0],
            [2.0, 0.0, 0.0],
        );
        let b = hermite(
            [0.0, 5.0, 0.0],
            [2.0, 1.0, 0.0],
            [2.0, 6.0, 0.0],
            [2.0, 0.0, 0.0],
        );
        let ca = make_curve_cache::<Obj>(&k, &a, None).unwrap();
        let cb = make_curve_cache::<Obj>(&k, &b, None).unwrap();
        let mut gs = CciGlobalSolver::new(
            ca.tree().unwrap(),
            cb.tree().unwrap(),
            a.natural_interval(),
            b.natural_interval(),
            false,
            ProjectionType::Parallel,
        )
        .with_curves(&a, &b, &k);
        let mut sols = SolutionArray::new();
        let req = SolveRequest {
            operation: SolverOperation::Intersect,
            tolerance_3d: 1e-6,
            ..SolveRequest::default()
        };
        gs.solve_it(&req, &mut sols).unwrap();
        assert!(sols.is_empty());
    }

    #[test]
    fn solver_without_curves_is_invalid() {
        let k = Kernel::default();
        let a = hermite(
            [0.0, 0.0, 0.0],
            [2.0, 0.0, 0.0],
            [2.0, 0.0, 0.0],
            [2.0, 0.0, 0.0],
        );
        let ca = make_curve_cache::<Obj>(&k, &a, None).unwrap();
        let t = ca.tree().unwrap();
        let mut gs = CciGlobalSolver::new(
            t,
            t,
            a.natural_interval(),
            a.natural_interval(),
            false,
            ProjectionType::Parallel,
        );
        let mut sols = SolutionArray::new();
        let req = SolveRequest {
            operation: SolverOperation::Intersect,
            tolerance_3d: 1e-6,
            ..SolveRequest::default()
        };
        assert!(gs.solve_it(&req, &mut sols).is_err());
    }

    #[test]
    fn plane_intersection_and_classification() {
        let k = Kernel::default();
        // a parabola-like arc crossing y = 0.25 twice
        let c = hermite(
            [0.0, 0.0, 0.0],
            [1.0, 2.0, 0.0],
            [2.0, 0.0, 0.0],
            [1.0, -2.0, 0.0],
        );
        let vecs = [Vec3::new(0.0, 1.0, 0.0)];
        let opts = PropertyAnalysisOptions::<Obj> {
            property_value: Some(-0.25),
            opt_vectors: &vecs,
            want_comb: true,
            handle: Some(Obj(0)),
        };
        let ivl = c.natural_interval();
        // (the span solves reach SmCurve::LocalPropertyAnalysis, unit curve.curve_p5)
        match c.global_property_analysis(
            &k,
            &ivl,
            CurvePropertyType::PlaneIntersection,
            1e-6,
            &opts,
        ) {
            Err(e) => assert!(e.is_unported()),
            Ok(r) => {
                assert_eq!(r.solutions.len(), 2);
                for s in r.solutions.as_slice() {
                    let p = c.evaluate_point(&k, s.start.parameters[0]).unwrap();
                    assert!((p.y - 0.25).abs() < 1e-6);
                }
                assert_eq!(r.comb_points.len(), COMB_COUNT);
                assert_eq!(r.comb_vecs.len(), COMB_COUNT);
            }
        }
        match c.global_property_analysis(&k, &ivl, CurvePropertyType::PlaneClassify, 1e-6, &opts) {
            Err(e) => assert!(e.is_unported()),
            Ok(r) => {
                assert_eq!(r.solutions.len(), 1);
                assert_eq!(
                    r.solutions.get(0).unwrap().solution_type,
                    SolutionType::RangeOfValues
                );
            }
        }
    }

    #[test]
    fn perpendicular_to_vector_of_a_line_has_no_solution() {
        let k = Kernel::default();
        let c = hermite(
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
        );
        let vecs = [Vec3::new(0.0, 1.0, 0.0)];
        let opts = PropertyAnalysisOptions::<Obj> {
            opt_vectors: &vecs,
            ..PropertyAnalysisOptions::default()
        };
        let r = c
            .global_property_analysis(
                &k,
                &c.natural_interval(),
                CurvePropertyType::PerpendicularToVector,
                1e-6,
                &opts,
            )
            .unwrap();
        assert!(r.solutions.is_empty());
    }
}
