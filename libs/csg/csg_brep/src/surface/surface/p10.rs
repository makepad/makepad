//! Line/surface leaf intersection and the combination of curve/surface intersection
//! solutions into unique points and coincident segments.
//!
//! Upstream: `source/SMLib/src/SmSurface.cpp`.
//!
//! | Rust                                             | Upstream                                    |
//! |--------------------------------------------------|---------------------------------------------|
//! | [`LsiGlobalSolver::local_solve_branch`]          | `SmLSIGlobalSolver::LocalSolve`             |
//! | [`global_curve_intersect_combine_solutions`], [`CombineContext`] | `sm_GlobalCurveIntersectCombineSolutions` (static) |
//!
//! Deviations:
//! - **Out-params.** `LocalSolve`'s `rbNeedsMoreSubdivision` is the return value
//!   (always false, as upstream). The combination edits the caller's
//!   [`SolutionArray`] in place, as upstream.
//! - **Patch data.** The leaf solve reads the node's `SmBezierPatch` through the
//!   solver's borrowed `surface_cache` (upstream casts `m_pData`); the node's tree must
//!   be that cache's tree. The surface evaluated (`sm_GetSurfaceToEvaluate`) and the
//!   parent surface (`sm_GetOriginalSurface`) are the cache's surface, named in the
//!   solutions by its handle. `GetPolarBox` builds a missing polar box without
//!   storing it ([`SurfaceCache::polar_box_unstored`]; the values are the same).
//!   `sm_IsPointValid` calls the virtual `PointTest` of the tree's cache: a face's
//!   trimmed cache answers through the solver's `face_cache`
//!   ([`crate::surface::surface::p8::FaceSolveCache::point_test`], a face
//!   classification), otherwise through [`super::p1::is_point_valid_with`].
//! - **Ignored statuses.** The leaf solve ignores the status of
//!   `SmExtent3d::IntersectLine` and `SmPseudoBox::IntersectLine` (a zero line vector,
//!   an undefined box or a negative volume box leave the hit count at zero: no work)
//!   and of `sm_ClassifyCurveSurfaceXSectPoint`; the combination ignores the status of
//!   `sm_ClassifyCurveSurfaceXSectPoint`, `SmSurface::ClassifyIntersectingRangeSolution`
//!   and of the `EvaluatePoint`s of its point distance check (a failed evaluation
//!   leaves `Vec3::UNDEF`). [`SolidError::Unported`] is always propagated.
//! - **Solution nodes.** The combination resolves `m_apNodes[0]` in
//!   [`CombineContext::curve_tree`] and `m_apNodes[1]` in the tree of
//!   [`CombineContext::surface_cache`]; a node id without its tree is an error (upstream
//!   holds pointers). The objects of the coincident range solutions it builds are
//!   [`CombineContext::curve_handle`] and [`CombineContext::surface_handle`] (upstream
//!   `&crCurve`, `&crSurface`).
//! - **Upstream quirks kept.** The combination classifies the first solution's point
//!   (not the current one) when two point solutions are too far apart; it tests the
//!   surface `u` of the current range start twice in the overlap test; the third
//!   interval span of its point/range culling is never filled (the `v` values go into
//!   the second span), so a point with valid nodes never culls; it resets a cull flag
//!   when the outer loop reaches that solution; the obsolete `while (!bDone2)` loop
//!   runs once (nothing clears the flag) and is a single pass here.
//! - **Debug only code** (`SM_DEBUG_CODE` drawing and dumps) and the assertions are not
//!   ported.
//! - **Bounds.** The leaf solve takes at most six guesses (upstream's fixed arrays). No
//!   recursion.

use super::p1::{is_node_valid, is_surface_cache_owner, surface_to_evaluate};
use super::p2::{original_surface, LsiGlobalSolver, Surface, TsectCurveType};
use super::p4;
use super::p6::{CurveIntersection, RangeTangentPoint};
use super::p9::implicit_iso;
use crate::curve::curve::p3::local_curve_intersect;
use crate::curve::curve::Curve;
use crate::curve::line::Line;
use crate::error::{ignored, Result, SolidError};
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::extent3d::LineHit as BoxHit;
use crate::geom::geom_utility::p2::ContinuityType;
use crate::geom::global_solver::{Branch, GlobalSolve, SolverOperation};
use crate::geom::pseudo_box::LineHit as PseudoHit;
use crate::geom::solution_array::{
    Solution, SolutionArray, SolutionEnd, SolutionObject, SolutionType, SortKey,
};
use crate::geom::tol::{are_same, scaled_zero2};
use crate::geom::tree::{AuxDataType, NodeId, Tree};
use crate::geom::vector2d::{Point2, Vec2};
use crate::geom::vector3d::{Point3, Vec3};
use crate::kernel::Kernel;
use crate::math::{max2, min2, BIG_DOUBLE, EFF_ZERO};
use crate::nurbs::b_spline_surface::SurfParamType;
use crate::nurbs::knots::Side;
use crate::surface::surface_cache::p1::SurfaceCache;
use crate::topology::ObjectHandle;

const OP_LSI: &str = "surface::surface::p10::LsiGlobalSolver::local_solve_branch";
const OP_COMBINE: &str = "surface::surface::p10::global_curve_intersect_combine_solutions";

/// `SM_BIG_ULONG`: no solution index yet.
const NO_INDEX: usize = usize::MAX;

/// Upstream's guess arrays (`SmPoint2d sUVGuesses[6]`).
const MAX_GUESSES: usize = 6;

// ---- calls into other units -------------------------------------------------------------------

/// `SmSurface::LocalCurveIntersect` (`bFoundIntersection`, `rUVFound`, `rdTFound`,
/// `rdDeviation`), the status checked by the caller (`SER`).
#[allow(clippy::too_many_arguments)] // upstream's arguments
fn surface_local_curve_intersect(
    k: &Kernel,
    surface: &Surface,
    domain: &Extent2d,
    curve: &Curve,
    interval: &Extent1d,
    tol: f64,
    uv_guess: Point2,
    t_guess: f64,
) -> Result<Option<CurveIntersection>> {
    surface.local_curve_intersect(k, domain, curve, interval, tol, uv_guess, t_guess)
}

/// `sm_FilterSolution`: whether to keep a curve/surface solution (`rbKeepSolution`).
fn filter_solution<O: SolutionObject>(
    k: &Kernel,
    surface: &Surface,
    domain: &Extent2d,
    curve: &Curve,
    interval: &Extent1d,
    conts: [[ContinuityType; 2]; 3],
    solution: &Solution<O>,
) -> Result<bool> {
    let [u_conts, v_conts, t_conts] = conts;
    crate::surface::surface::p5::filter_solution(
        k, surface, domain, curve, interval, u_conts, v_conts, t_conts, solution,
    )
}

/// The outputs of `sm_ClassifyCurveSurfaceXSectPoint`.
struct XsectPointClass {
    /// `bFoundGuess`, `rsNewUVGuess`, `rdNewTGuess`.
    guess: Option<(Point2, f64)>,
    /// `rePointType`.
    point_type: TsectCurveType,
}

/// `sm_ClassifyCurveSurfaceXSectPoint` (`surface::surface::p4`); `edge_tolerance` is
/// the tolerance of the curve's owning edge, when the caller resolves it.
#[allow(clippy::too_many_arguments)]
fn classify_curve_surface_xsect_point(
    k: &Kernel,
    surface: &Surface,
    domain: &Extent2d,
    curve: &Curve,
    interval: &Extent1d,
    uv: Point2,
    t: f64,
    edge_tolerance: Option<f64>,
) -> Result<XsectPointClass> {
    let c = p4::classify_curve_surface_xsect_point_with(
        k,
        surface,
        domain,
        curve,
        interval,
        uv,
        t,
        edge_tolerance,
    )?;
    Ok(XsectPointClass {
        guess: c.found_guess.then_some((c.new_uv_guess, c.new_t_guess)),
        point_type: c.point_type,
    })
}

/// `SmSurface::LocalPointSolve` (`rbFoundAnswer`, `rSolution`).
fn surface_local_point_solve<O: Copy>(
    k: &Kernel,
    surface: &Surface,
    domain: &Extent2d,
    operation: SolverOperation,
    point: Point3,
    uv_guess: Point2,
) -> Result<(bool, Solution<O>)> {
    let r = surface.local_point_solve::<O>(k, domain, operation, point, uv_guess)?;
    Ok((r.found, r.solution))
}

/// The outputs of `SmSurface::DropPoint` used here: `rbSuccess` and `rdGap`.
struct DroppedPoint {
    success: bool,
    gap: f64,
}

/// `SmSurface::DropPoint` with a guess.
fn surface_drop_point(
    k: &Kernel,
    surface: &Surface,
    point: Point3,
    domain: &Extent2d,
    guess: Point2,
) -> Result<DroppedPoint> {
    let opts = crate::surface::surface::p5::DropPointOptions {
        guess_uv: Some(guess),
        ..Default::default()
    };
    let d = surface.drop_point::<p4::NoObject>(k, point, domain, &opts)?;
    Ok(DroppedPoint {
        success: d.success,
        gap: d.gap,
    })
}

/// `SmSurface::ClassifyIntersectingRangeSolution`: `bIsDegenerate` and, when it is,
/// the tangent point solution written to `rSolPoint`.
fn classify_intersecting_range_solution<O: SolutionObject>(
    k: &Kernel,
    surface: &Surface,
    curve: &Curve,
    solution: &Solution<O>,
) -> Result<(bool, Option<RangeTangentPoint>)> {
    surface.classify_intersecting_range_solution(k, curve, solution)
}

/// The outputs of `SmSurface::DropAndTrimCurve` used here.
struct DropAndTrim {
    /// `rdDistToSurface`.
    dist_to_surface: f64,
    /// `rUVCurves`.
    uv_curves: Vec<Curve>,
}

/// `SmSurface::DropAndTrimCurve(context, domain, curve, interval, tol, ..., TRUE,
/// TRUE)` (upstream requires an `SmBSplineCurve`).
fn drop_and_trim_curve<O: SolutionObject>(
    k: &Kernel,
    surface: &Surface,
    cache: Option<&SurfaceCache<O>>,
    domain: &Extent2d,
    curve: &Curve,
    interval: &Extent1d,
    tol: f64,
) -> Result<DropAndTrim> {
    // the trace gets the surface's cache (`smsurf_GetSurfaceCache`): for a face's
    // surface, the face's cache the solve ran on
    let mut held = match cache {
        Some(c) if surface.face()?.is_some() => Some(c.map_handles(Ok)?),
        _ => None,
    };
    let r = surface.drop_and_trim_curve_in::<ObjectHandle, O>(
        k,
        held.as_mut(),
        domain,
        curve,
        interval,
        tol,
        true,
        true,
        None,
    )?;
    Ok(DropAndTrim {
        dist_to_surface: r.max_drop_to_surf,
        uv_curves: r.uv_curves,
    })
}

// ---- helpers -----------------------------------------------------------------------------------

/// `SM_IS_BETWEEN(c, a, b)`: `c` strictly inside the open interval between `a` and
/// `b` shrunk by their scaled zero.
fn is_between(c: f64, a: f64, b: f64) -> bool {
    let z = scaled_zero2(&a, &b);
    ((a + z) < c && c < (b - z)) || ((b + z) < c && c < (a - z))
}

/// `continuity <= SM_CT_C0`.
fn at_most_c0(c: ContinuityType) -> bool {
    (c as i32) <= ContinuityType::C0 as i32
}

/// Parameter `i` of a solution end (`m_adParameters[i]`).
fn p(end: &SolutionEnd, i: usize) -> Result<f64> {
    end.param(i)
}

/// Sets parameter `i` of a solution end.
fn set_p(end: &mut SolutionEnd, i: usize, v: f64) -> Result<()> {
    *end.param_mut(i)? = v;
    Ok(())
}

/// `EvaluatePoint` whose status upstream ignores: `Vec3::UNDEF` on failure.
fn point_or_undef(k: &Kernel, curve: &Curve, t: f64) -> Result<Point3> {
    Ok(ignored(curve.evaluate_point(k, t))?.unwrap_or(Vec3::UNDEF))
}

/// The count and parameters of an `IntersectLine` whose status upstream ignores (an
/// invalid input leaves the count at zero).
fn box_hit(hit: &Result<BoxHit>) -> Option<(f64, f64)> {
    match *hit {
        Ok(BoxHit::Touch { t }) => Some((t, t)),
        Ok(BoxHit::Through { enter, exit }) => Some((enter, exit)),
        Ok(BoxHit::Miss) | Err(_) => None,
    }
}

/// [`box_hit`] for a pseudo box.
fn pseudo_hit(hit: &Result<PseudoHit>) -> Option<(u32, f64, f64)> {
    match *hit {
        Ok(PseudoHit::Graze(t)) => Some((1, t, t)),
        Ok(PseudoHit::Segment(enter, exit)) => Some((2, enter, exit)),
        Ok(PseudoHit::Miss) | Err(_) => None,
    }
}

// ---- SmLSIGlobalSolver::LocalSolve -------------------------------------------------------------

/// Read-only state of one leaf solve.
struct LsiLeaf<'s, O> {
    cache: &'s SurfaceCache<O>,
    owner: Option<crate::geom::tree::TreeOwner>,
    parent: O,
    eval: &'s Surface,
    domain: Extent2d,
    line: Curve,
    interval: Extent1d,
    conts: [[ContinuityType; 2]; 3],
    nodes: [Option<NodeId>; 2],
}

impl<O: SolutionObject> LsiLeaf<'_, O> {
    /// A line/surface solution `[t, u, v]` of this leaf (upstream's field-by-field
    /// set up of `sSolution`).
    fn solution(&self, t: f64, uv: Point2, deviation: f64) -> Result<Solution<O>> {
        let mut solution = Solution::new();
        self.adopt(&mut solution);
        solution.num_variables = 3;
        set_p(&mut solution.start, 0, t)?;
        set_p(&mut solution.start, 1, uv.x)?;
        set_p(&mut solution.start, 2, uv.y)?;
        solution.start.solution_value = deviation;
        solution.solution_type = SolutionType::SingleValue;
        Ok(solution)
    }

    /// Names this leaf's objects (`NULL`, the parent surface) and nodes in `solution`.
    fn adopt(&self, solution: &mut Solution<O>) {
        solution.num_objects = 2;
        solution.objects[0] = None;
        solution.objects[1] = Some(self.parent);
        solution.nodes[0] = self.nodes[0];
        solution.nodes[1] = self.nodes[1];
    }
}

impl<'t, 's, O: SolutionObject + 't> LsiGlobalSolver<'t, 's, O> {
    /// `sm_IsPointValid` of a leaf of `cache` (tree owner kind `owner`): the face's
    /// trimmed cache answers with its point test (a face classification) when the
    /// solve runs on one, else the prepared query service. A failed native classification keeps the
    /// point (`SE(SM_ERR)`, return TRUE); [`SolidError::Unported`] propagates.
    fn is_point_valid(
        &mut self,
        k: &Kernel,
        owner: Option<crate::geom::tree::TreeOwner>,
        cache: &SurfaceCache<O>,
        uv: Point2,
    ) -> Result<bool> {
        match self.face_cache.as_deref_mut() {
            // keep all points when SurfaceNode has no trim boundaries
            Some(face_cache) if is_surface_cache_owner(owner) => Ok(
                super::p1::ignore_native_classification(face_cache.point_test(k, uv))?
                    .unwrap_or(true),
            ),
            _ => super::p1::is_point_valid_with(k, owner, cache, uv, None, self.face_queries),
        }
    }

    /// Filters a single valued solution of `surface` over `domain` and adds it when
    /// kept and inside the trim boundary (upstream's repeated `sm_FilterSolution` /
    /// `sm_IsPointValid` / `AddSortedSolution` block).
    fn keep_point(
        &mut self,
        k: &Kernel,
        leaf: &LsiLeaf<'s, O>,
        surface: &Surface,
        domain: &Extent2d,
        solution: &Solution<O>,
    ) -> Result<bool> {
        let keep = filter_solution(
            k,
            surface,
            domain,
            &leaf.line,
            &leaf.interval,
            leaf.conts,
            solution,
        )?;
        if keep {
            let uv = Point2::new(p(&solution.start, 1)?, p(&solution.start, 2)?);
            // for points within the surface's trim boundary
            if self.is_point_valid(k, leaf.owner, leaf.cache, uv)? {
                self.add_sorted_solution(solution, SortKey::ByAllParameters, true)?;
            }
        }
        Ok(keep)
    }

    /// `SmLSIGlobalSolver::LocalSolve`: intersects the line with the Bezier patch of
    /// the surface leaf node `branch[0]` (guess points refined by the surface's local
    /// curve intersection, then the line ends and the patch boundaries at
    /// discontinuities), adding the solutions `[line t, surface u, surface v]` kept to
    /// `base.solutions`. Returns the request for more subdivision (always false).
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` without `surface_cache` (or `kernel` for the boundary
    /// coincidence checks); `InvalidInput` when the node is not a patch node of that
    /// cache or the cache does not hold the tree's surface; those of the evaluations
    /// and solves; [`SolidError::Unported`] from the calls into units not ported yet.
    #[allow(clippy::too_many_lines)]
    pub fn local_solve_branch(&mut self, k: &Kernel, branch: &Branch) -> Result<bool> {
        // locals - apBranch[0] is SurfaceNode
        let surface_node = *self.base.branch_node(branch, 0)?;
        let surface_id = branch
            .first()
            .copied()
            .flatten()
            .ok_or(SolidError::null_pointer(OP_LSI))?;
        let cache = self.surface_cache.ok_or(SolidError::null_pointer(OP_LSI))?;
        let patch = cache.patch(surface_id)?.clone();
        let tree = self.base.tree(0)?;
        let parent = original_surface(tree, &surface_node)?;
        if Some(parent) != cache.surface_handle() {
            return Err(SolidError::invalid_input(
                OP_LSI,
                "surface cache does not own the tree's surface",
            ));
        }
        let mut uv_domain = patch.aux.uv_domain;

        // Use the original surface (sm_GetSurfaceToEvaluate)
        let eval_handle = surface_to_evaluate(tree, &surface_node)?;
        if Some(eval_handle) != cache.surface_handle() {
            return Err(SolidError::invalid_input(
                OP_LSI,
                "surface cache does not own the surface to evaluate",
            ));
        }
        let srf_eval = cache.surface();

        // no work - This SurfaceNode is classified outside its face.
        if !is_node_valid(tree.owner(), cache, &surface_node)? {
            return Ok(false);
        }

        // no work - surfaceLeafDomain disjoint from intersectionDomain
        if uv_domain.are_disjoint(&self.uv_domain, 0.0) {
            return Ok(false);
        }

        // Set discontinuities: Mark Leaf boundaries discontinuous for line and surface
        let t_conts = [ContinuityType::Discontinuous; 2];
        let mut u_conts = patch.u_curve_conts;
        let mut v_conts = patch.v_curve_conts;

        // mark intersection boundary limits as discontinuous
        if self.uv_domain.min().x >= uv_domain.min().x {
            u_conts[0] = ContinuityType::Discontinuous;
        }
        if self.uv_domain.max().x <= uv_domain.max().x {
            u_conts[1] = ContinuityType::Discontinuous;
        }
        if self.uv_domain.min().y >= uv_domain.min().y {
            v_conts[0] = ContinuityType::Discontinuous;
        }
        if self.uv_domain.max().y <= uv_domain.max().y {
            v_conts[1] = ContinuityType::Discontinuous;
        }

        // when SurfaceLeafDomain is partially intersected by the IntersectionDomain
        if !uv_domain.is_contained_by(&self.uv_domain, EFF_ZERO) {
            // trim leafNode domain down to intersection domain
            uv_domain = uv_domain.intersect(&self.uv_domain)?;

            // no work - remaining domain has a zero length dimension
            if are_same(uv_domain.max().x, uv_domain.min().x) {
                return Ok(false);
            }
            if are_same(uv_domain.max().y, uv_domain.min().y) {
                return Ok(false);
            }
        }

        // cull obviously nonIntersecting cases with BBox and pseudo box line intersections
        let mut pseudo_box = patch.pseudo_box;
        pseudo_box.expand_absolute(self.distance_tolerance);

        // BBox/line intersect
        let mut bbox = surface_node.bbox;
        bbox.expand_absolute(self.base.tolerance_3d);
        let Some((bbox_enter, bbox_exit)) =
            box_hit(&bbox.intersect_line(self.line_point, self.line_vector))
        else {
            // no work - no BBox intersection
            return Ok(false);
        };

        // PseudoBox/line intersect
        let Some((_, pseudo_enter, pseudo_exit)) =
            pseudo_hit(&pseudo_box.intersect_line(self.line_point, self.line_vector))
        else {
            // no work - no Pseudo intersection
            return Ok(false);
        };

        // no work - BBox/Pseudo intersection segments don't overlap
        if max2(bbox_enter, pseudo_enter) > min2(bbox_exit, pseudo_exit) {
            return Ok(false);
        }

        // target line interval that must contain any intersection
        let mut bound_ivl =
            Extent1d::from_min_max(max2(bbox_enter, pseudo_enter), min2(bbox_exit, pseudo_exit));

        // expand by more than tolerance [B242]
        bound_ivl.expand_absolute(2.0 * self.base.tolerance_3d);

        // trim sBoundIvl to m_cpLineInterval (the interval of interest)
        if let Some(line_ivl) = &self.line_interval {
            bound_ivl = bound_ivl
                .intersect(line_ivl)
                .unwrap_or_else(|_| Extent1d::new());
        }

        // when firing rays only look at positive line parameters
        if self.ray_fire && bound_ivl.max() <= 0.0 {
            // no work - ray casting and no positive intersection segment
            return Ok(false);
        }

        // no work - if given, m_cpLineInterval doesn't overlap sBoxIvl
        if let Some(line_ivl) = &self.line_interval {
            if line_ivl.are_disjoint(&bound_ivl, 0.0) {
                return Ok(false);
            }
        }

        // init a line interval - for a ray or for a given line interval
        let ivl = match &self.line_interval {
            Some(line_ivl) => line_ivl
                .intersect(&bound_ivl)
                .unwrap_or_else(|_| Extent1d::new()),
            None => bound_ivl,
        };

        // let guess = LineInterval/SurfaceDomain CenterPoints
        let mut first_t = ivl.evaluate(0.5);
        let first_uv_guess = uv_domain.evaluate(0.5, 0.5);
        let mut uv_guesses = Vec::new();
        let mut t_guesses = Vec::new();

        // Let SurfaceGuessPoint = Drop LinePoint down to SurfacePoint Tangent Plane
        let drop = |uv: Point2, t: f64| -> Result<Point2> {
            let point = self.line_point + t * self.line_vector;
            super::p7::drop_points_to_tangent_plane(
                k,
                srf_eval,
                uv,
                Side::Left,
                Side::Left,
                &[point],
            )?
            .first()
            .copied()
            .ok_or(SolidError::sm_err(OP_LSI))
        };
        let mut first_uv = drop(first_uv_guess, first_t)?;

        // When the updated SurfaceGuessPoint is outside the leafNodeIntersectionDomain
        let uv_eps = EFF_ZERO * (1.0 + uv_domain.size().max_dimension());
        if !uv_domain.contains_point(first_uv, uv_eps) {
            // move guess to SurfaceMidPoint and near LineInterval beginPoint.
            first_t = ivl.evaluate(0.1);
            first_uv = drop(uv_domain.evaluate(0.5, 0.5), first_t)?;

            // When SurfaceMid/LineStart Drop is also outside the leafNodeIntersectionDomain
            if !uv_domain.contains_point(first_uv, 0.0) {
                // move guess to SurfaceMidPoint and near LineInterval endPoint.
                first_t = ivl.evaluate(0.9);
                first_uv = drop(uv_domain.evaluate(0.5, 0.5), first_t)?;

                // when this 3rd point is also outside the leafNodeIntersectionDomain
                if !uv_domain.contains_point(first_uv, 0.0) {
                    // clamp the guess to the leafNodeIntersectionDomain
                    first_uv = uv_domain.clamp_point(first_uv);

                    // add in 3 more guesses: SurfLeafCenterPoint/(Interval Start, Mid, End)
                    for fraction in [0.5, 0.1, 0.9] {
                        uv_guesses.push(uv_domain.evaluate(0.5, 0.5));
                        t_guesses.push(ivl.evaluate(fraction));
                    }
                }
            }
        }
        uv_guesses.insert(0, first_uv);
        t_guesses.insert(0, first_t);

        // Make the line object (sTmpIvl = the bounded interval of interest).
        let line = Curve::from(Line::infinite(self.line_point, self.line_vector, 3, false));
        let leaf = LsiLeaf {
            cache,
            owner: tree.owner(),
            parent,
            eval: srf_eval,
            domain: uv_domain,
            line,
            interval: ivl,
            conts: [u_conts, v_conts, t_conts],
            nodes: [
                branch.first().copied().flatten(),
                branch.get(1).copied().flatten(),
            ],
        };

        // for every Guess (more than 1 when LineGuess does not drop to SurfaceDomain)
        let mut i = 0;
        while let (Some(&uv_guess), Some(&t_guess)) = (uv_guesses.get(i), t_guesses.get(i)) {
            let index = i;
            i += 1;

            // pass the call along to local curve intersection
            let Some(hit) = surface_local_curve_intersect(
                k,
                leaf.eval,
                &leaf.domain,
                &leaf.line,
                &leaf.interval,
                self.base.tolerance_3d,
                uv_guess,
                t_guess,
            )?
            else {
                continue;
            };

            // process intersections
            let solution = leaf.solution(hit.t, hit.uv, hit.deviation)?;

            // skip negative parameter ray/surface intersections
            if self.ray_fire {
                let scaled_zero = EFF_ZERO * (1.0 + self.line_point.max_dimension());
                if hit.t < -scaled_zero {
                    continue;
                }
            }

            // keep exact solutions - check near solutions
            if !filter_solution(
                k,
                leaf.eval,
                &leaf.domain,
                &leaf.line,
                &leaf.interval,
                leaf.conts,
                &solution,
            )? {
                continue;
            }

            // for points within the surface's trim boundary
            if self.is_point_valid(k, leaf.owner, cache, hit.uv)? {
                self.add_sorted_solution(&solution, SortKey::ByAllParameters, true)?;
            }

            // Try to eliminate additional intersections with the patch's normal map.
            let line_vec = self.line_vector.unitize()?;
            if !cache
                .polar_box_unstored(k, surface_id)?
                .has_perpendicular_to_vector(line_vec)
            {
                return Ok(false);
            }

            // look for another intersection very close to this one (near tangent)
            if index < 1 {
                let class = classify_curve_surface_xsect_point(
                    k,
                    leaf.eval,
                    &leaf.domain,
                    &leaf.line,
                    &leaf.interval,
                    hit.uv,
                    hit.t,
                    None,
                );
                // status ignored upstream (SE)
                if let Some(XsectPointClass {
                    guess: Some((new_uv, new_t)),
                    ..
                }) = ignored(class)?
                {
                    if uv_guesses.len() >= MAX_GUESSES {
                        return Err(SolidError::invalid_input(
                            OP_LSI,
                            "intersection guess limit exceeded",
                        ));
                    }
                    uv_guesses.push(new_uv);
                    t_guesses.push(new_t);
                }
            }
        }

        // the extra work required at discontinuities of the line ends and patch boundaries
        let natural = cache.surface().natural_uv_domain_with_nurb(k)?;
        for j in 0..2 {
            // Handle curve discontinuities
            if t_conts.get(j).copied().is_some_and(at_most_c0) {
                let t = if j == 1 {
                    leaf.interval.max()
                } else {
                    leaf.interval.min()
                };
                let pnt = leaf.line.evaluate_point(k, t)?;
                if pseudo_box.distance_to_point(pnt) < EFF_ZERO {
                    // Get the big surface from the tree and cache
                    let surface = cache.surface();
                    let (found, mut solution) = surface_local_point_solve::<O>(
                        self.kernel,
                        surface,
                        &natural,
                        SolverOperation::Minimize,
                        pnt,
                        leaf.domain.evaluate(0.5, 0.5),
                    )?;
                    if found && solution.start.solution_value < self.base.tolerance_3d {
                        // turn Surf/Point solution into a line:[pt,vec]/Surf solution
                        leaf.adopt(&mut solution);
                        solution.num_variables = 3;
                        let (u, v) = (p(&solution.start, 0)?, p(&solution.start, 1)?);
                        set_p(&mut solution.start, 2, v)?;
                        set_p(&mut solution.start, 1, u)?;
                        set_p(&mut solution.start, 0, t)?;
                        solution.solution_type = SolutionType::SingleValue;
                        self.keep_point(k, &leaf, surface, &natural, &solution)?;
                    }
                }
            }

            // Handle surface boundary discontinuities
            if !cache.process_boundary_curves {
                continue;
            }
            for dir in [SurfParamType::U, SurfParamType::V] {
                if self.boundary_solve(k, &leaf, &patch.pseudo_box, dir, j)? {
                    return Ok(false);
                }
            }
        }

        // all done
        Ok(false)
    }

    /// The patch boundary of `leaf` at its `dir` minimum (`side == 0`) or maximum
    /// when its continuity there is at most C0: coincident ranges of the line along
    /// the boundary iso curve, else its crossings. True when upstream returns (a
    /// coincidence was found, or a point on a degenerate interval).
    fn boundary_solve(
        &mut self,
        k: &Kernel,
        leaf: &LsiLeaf<'s, O>,
        patch_box: &crate::geom::pseudo_box::PseudoBox,
        dir: SurfParamType,
        side: usize,
    ) -> Result<bool> {
        let [u_conts, v_conts, _] = leaf.conts;
        let is_u = dir == SurfParamType::U;
        let cont = if is_u { u_conts } else { v_conts };
        if !cont.get(side).copied().is_some_and(at_most_c0) {
            return Ok(false);
        }
        let domain = leaf.domain;
        let constant = match (is_u, side) {
            (true, 0) => domain.min().x,
            (true, _) => domain.max().x,
            (false, 0) => domain.min().y,
            (false, _) => domain.max().y,
        };
        let iso = implicit_iso(k, leaf.eval, dir, constant, side == 0)?;
        if iso.is_degenerate(k, EFF_ZERO, None)? {
            return Ok(false);
        }
        let iso_ivl = if is_u {
            Extent1d::from_min_max(domain.min().y, domain.max().y)
        } else {
            Extent1d::from_min_max(domain.min().x, domain.max().x)
        };
        let tol = self.base.tolerance_3d;

        // Check for coincidence with curves of low continuity or boundary curves
        let k = self.kernel;
        let found = leaf.line.simple_coincidence_checker::<O>(
            k,
            &leaf.interval,
            &iso,
            &iso_ivl,
            tol,
            None,
            Some(patch_box),
            [None, None],
        )?;
        let num_found = found.len();
        let mut found_coin = false;
        for mut solution in found {
            // turn TmpLine/isoCurve solution into a line:[pt,tan]/Surface solution
            solution.num_variables = 3;
            leaf.adopt(&mut solution);
            if is_u {
                let other = p(&solution.start, 1)?;
                set_p(&mut solution.start, 2, other)?;
                set_p(&mut solution.start, 1, constant)?;
            } else {
                set_p(&mut solution.start, 2, constant)?;
            }
            if solution.solution_type == SolutionType::RangeOfValues {
                found_coin = true;
                if is_u {
                    let other = p(&solution.end, 1)?;
                    set_p(&mut solution.end, 2, other)?;
                    set_p(&mut solution.end, 1, constant)?;
                } else {
                    set_p(&mut solution.end, 2, constant)?;
                }
                self.add_sorted_solution(&solution, SortKey::ByAllParameters, true)?;
            } else {
                // Single valued solutions
                let keep = self.keep_point(k, leaf, leaf.eval, &domain, &solution)?;
                // when working with degenerate intervals we're done (v boundary only)
                if keep
                    && !is_u
                    && (leaf.interval.length() < EFF_ZERO || iso_ivl.length() < EFF_ZERO)
                {
                    found_coin = true;
                }
            }
        }

        // If we found a coincidence we are done
        if found_coin {
            return Ok(true);
        }

        // when the point tester found nothing look for straight intersections
        if num_found == 0 {
            if let Some(hit) = local_curve_intersect(
                k,
                &iso,
                &iso_ivl,
                &leaf.line,
                &leaf.interval,
                tol,
                iso_ivl.evaluate(0.5),
                leaf.interval.evaluate(0.5),
            )? {
                let uv = if is_u {
                    Point2::new(constant, hit.my_param)
                } else {
                    Point2::new(hit.my_param, constant)
                };
                let solution = leaf.solution(hit.other_param, uv, hit.deviation)?;
                self.keep_point(k, leaf, leaf.eval, &domain, &solution)?;
            }
        }
        Ok(false)
    }
}

// ---- sm_GlobalCurveIntersectCombineSolutions ---------------------------------------------------

/// Where the combination finds the cache nodes and objects of the solutions (see the
/// module deviations).
#[derive(Debug, Clone, Copy)]
pub struct CombineContext<'a, O> {
    /// The curve cache tree `m_apNodes[0]` indexes.
    pub curve_tree: Option<&'a Tree<O>>,
    /// The surface cache whose tree `m_apNodes[1]` indexes.
    pub surface_cache: Option<&'a SurfaceCache<O>>,
    /// The curve's handle (upstream `&crCurve`).
    pub curve_handle: Option<O>,
    /// The surface's handle (upstream `&crSurface`).
    pub surface_handle: Option<O>,
    /// The tolerance of the curve's owning edge (`pCurveOwner->GetTolerance()` of
    /// `sm_ClassifyCurveSurfaceXSectPoint`), when the caller resolves it.
    pub edge_tolerance: Option<f64>,
}

impl<O> Default for CombineContext<'_, O> {
    fn default() -> Self {
        Self {
            curve_tree: None,
            surface_cache: None,
            curve_handle: None,
            surface_handle: None,
            edge_tolerance: None,
        }
    }
}

impl<O: SolutionObject> CombineContext<'_, O> {
    /// The curve tree node of a solution (`m_apNodes[0]`).
    fn curve_node(&self, id: NodeId) -> Result<&crate::geom::tree::TreeNode<O>> {
        self.curve_tree
            .ok_or(SolidError::invalid_input(
                OP_COMBINE,
                "solution curve node without a curve tree",
            ))?
            .node(id)
    }

    /// The surface cache tree node of a solution (`m_apNodes[1]`).
    fn surface_node(&self, id: NodeId) -> Result<&crate::geom::tree::TreeNode<O>> {
        self.surface_cache
            .ok_or(SolidError::invalid_input(
                OP_COMBINE,
                "solution surface node without a surface cache",
            ))?
            .tree()
            .ok_or(SolidError::invalid_input(
                OP_COMBINE,
                "surface cache has no tree",
            ))?
            .node(id)
    }

    /// `bNode1Valid && bNode2Valid`: the curve node is a span or curve data node and
    /// the surface node a patch, patch head or aux data node; then the curve node's
    /// interval and the surface node's domain.
    fn node_spans(&self, nodes: &[Option<NodeId>]) -> Result<Option<(Extent1d, Extent2d)>> {
        let mut valid1 = None;
        if let Some(id) = nodes.first().copied().flatten() {
            let n = self.curve_node(id)?;
            if matches!(
                n.aux_type,
                AuxDataType::BezierCurve | AuxDataType::CurveData
            ) {
                valid1 = Some(n);
            }
        }
        let mut valid2 = None;
        if let Some(id) = nodes.get(1).copied().flatten() {
            let n = self.surface_node(id)?;
            if matches!(
                n.aux_type,
                AuxDataType::BezierSurface | AuxDataType::BezierSurfaceHead | AuxDataType::AuxData
            ) {
                valid2 = Some(id);
            }
        }
        let (Some(n1), Some(id2)) = (valid1, valid2) else {
            return Ok(None);
        };
        let tree = self
            .curve_tree
            .ok_or(SolidError::null_pointer(OP_COMBINE))?;
        let cache = self
            .surface_cache
            .ok_or(SolidError::null_pointer(OP_COMBINE))?;
        Ok(Some((
            tree.curve_aux(n1)?.interval,
            cache.aux(id2)?.uv_domain,
        )))
    }
}

/// The point classification of the combination: whether `solution`'s start point is
/// classified `SM_TC_COINCIDENT` (status ignored upstream).
#[allow(clippy::too_many_arguments)]
fn is_coincident_point<O>(
    k: &Kernel,
    surface: &Surface,
    uv_domain: &Extent2d,
    curve: &Curve,
    interval: &Extent1d,
    edge_tolerance: Option<f64>,
    solution: &Solution<O>,
) -> Result<bool> {
    let uv = Point2::new(p(&solution.start, 1)?, p(&solution.start, 2)?);
    let t = p(&solution.start, 0)?;
    let class = ignored(classify_curve_surface_xsect_point(
        k,
        surface,
        uv_domain,
        curve,
        interval,
        uv,
        t,
        edge_tolerance,
    ))?;
    Ok(class.is_some_and(|c| c.point_type == TsectCurveType::Coincident))
}

/// Drops the curve points at `fractions` of the opening between two range solutions
/// to the surface: false when one does not drop or lands farther than `tol`.
#[allow(clippy::too_many_arguments)] // the opening and its surface
fn opening_drops(
    k: &Kernel,
    surface: &Surface,
    uv_domain: &Extent2d,
    curve: &Curve,
    ivls: [&Extent1d; 3],
    tol: f64,
    count: u32,
    step: f64,
) -> Result<bool> {
    let [ivl1, ivl2, ivl3] = ivls;
    for jj in 0..count {
        let param = step * f64::from(jj + 1);
        let pt_on1 = curve.evaluate_point(k, ivl1.evaluate(param))?;
        let guess = Point2::new(ivl2.evaluate(param), ivl3.evaluate(param));
        let dropped = surface_drop_point(k, surface, pt_on1, uv_domain, guess)?;

        // don't combine solutions when SolutionGap MidPoint does not drop or is too far
        if !dropped.success || dropped.gap > tol {
            return Ok(false);
        }
    }
    Ok(true)
}

/// `sm_GlobalCurveIntersectCombineSolutions`: combines the curve/surface solutions
/// `[curve t, surface u, surface v]` (sorted by the curve parameter) into unique point
/// solutions and coincident segments, in place: merges touching, overlapping and
/// coincidently joined ranges and coincident points, drops short ranges next to
/// points, turns small or degenerate ranges into points, and when a coincidence is
/// found replaces the solutions by the curve's drop onto the surface.
///
/// # Errors
///
/// Those of the evaluations and approximate lengths, a solution node without its tree
/// (see [`CombineContext`]), and [`SolidError::Unported`] from the calls into units not
/// ported yet.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)] // upstream's arguments
pub fn global_curve_intersect_combine_solutions<O: SolutionObject>(
    k: &Kernel,
    surface: &Surface,
    uv_domain: &Extent2d,
    curve: &Curve,
    interval: &Extent1d,
    distance_tolerance: f64,
    ctx: &CombineContext<'_, O>,
    solutions: &mut SolutionArray<O>,
) -> Result<()> {
    let tol = distance_tolerance;
    let mut found_coincidence = false;

    // Phase 1: combine pairs of solutions of the same type.
    let uv_size = surface.natural_uv_domain_with_nurb(k)?.size();
    let t_size = curve.natural_interval().length();

    // set up rSolutions array indexing for Point and interval solutions
    let mut last_interval = 0;
    let mut last_point = 0;
    if !solutions.is_empty() {
        let first = solutions.get_at(0)?;
        if first.solution_type == SolutionType::RangeOfValues {
            last_interval = 0;
            last_point = NO_INDEX;
        } else {
            last_interval = NO_INDEX;
            last_point = 0;
            if is_coincident_point(
                k,
                surface,
                uv_domain,
                curve,
                interval,
                ctx.edge_tolerance,
                &first,
            )? {
                found_coincidence = true;
            }
        }
    }

    // for every rSolution after the 1st
    let num = solutions.len();
    for ii in 1..num {
        // index of the last solution of the current solution's type
        let curr_was_range = solutions.get_at(ii)?.solution_type == SolutionType::RangeOfValues;
        let last_index = if curr_was_range {
            last_interval
        } else {
            last_point
        };

        // last solution and its end (rLastSol, pLastEnd)
        let last_slot = if last_index == NO_INDEX {
            0
        } else {
            last_index
        };
        let mut last_sol = solutions.get_at(last_slot)?;
        let last_end = if last_sol.solution_type == SolutionType::RangeOfValues {
            last_sol.end
        } else {
            last_sol.start
        };

        let mut do_combine = false;
        'decide: {
            // no previous solution of same type - keep solution
            if last_index == NO_INDEX {
                break 'decide;
            }

            // Look for other solutions starting where this one starts on the curve and
            // closer to the last end on the surface.
            let mut closest = ii;
            let scaled_zero = EFF_ZERO * (1.0 + p(&solutions.get_at(ii)?.start, 0)?.abs());
            for jj in (ii + 1)..num {
                let c = solutions.get_at(closest)?.start;
                let o = solutions.get_at(jj)?.start;
                if (p(&c, 0)? - p(&o, 0)?).abs() < scaled_zero {
                    let u_diff1 = (p(&c, 1)? - p(&last_end, 1)?).abs();
                    let v_diff1 = (p(&c, 2)? - p(&last_end, 2)?).abs();
                    let u_diff2 = (p(&o, 1)? - p(&last_end, 1)?).abs();
                    let v_diff2 = (p(&o, 2)? - p(&last_end, 2)?).abs();
                    if u_diff2 + v_diff2 < u_diff1 + v_diff1 {
                        closest = jj;
                    }
                } else {
                    // quit at the first worst solution
                    break;
                }
            }

            // when a closer OtherSolution was found - swap its position
            if closest != ii {
                solutions.swap(closest, ii)?;
            }
            let curr = solutions.get_at(ii)?;
            let curr_start = curr.start;

            // Don't allow jumping over seam
            let uv_diff = Vec2::new(
                (p(&curr_start, 1)? - p(&last_end, 1)?).abs(),
                (p(&curr_start, 2)? - p(&last_end, 2)?).abs(),
            );
            let t_diff = (p(&curr_start, 0)? - p(&last_end, 0)?).abs();
            if uv_diff.x > uv_size.x * 0.90 || uv_diff.y > uv_size.y * 0.90 {
                // don't connect solutions across Surface seams
                break 'decide;
            }

            // don't connect solutions across closed curve ends
            if t_diff > t_size * 0.90 {
                break 'decide;
            }

            // When Solution endPoints touch on either curve or surface - combine
            if are_same(p(&last_end, 0)?, p(&curr_start, 0)?)
                || (are_same(p(&last_end, 1)?, p(&curr_start, 1)?)
                    && are_same(p(&last_end, 2)?, p(&curr_start, 2)?))
            {
                do_combine = true;

                // for interval solutions - make some exceptions
                if curr.solution_type == SolutionType::RangeOfValues {
                    let start_dev1 = last_sol.start.solution_value;
                    let end_dev1 = last_sol.end.solution_value;

                    // don't combine when end of rLastSol is much better than start
                    if start_dev1 > EFF_ZERO && start_dev1 > end_dev1 * 100_000.0 {
                        do_combine = false;
                    }
                    let start_dev2 = curr.start.solution_value;
                    let end_dev2 = curr.end.solution_value;

                    // don't combine when start of CurrSol is much better than end
                    if end_dev2 > EFF_ZERO && end_dev2 > start_dev2 * 100_000.0 {
                        do_combine = false;
                    }

                    // If the smaller segment is less than 1/1000 of the other then
                    // just combine them anyhow.
                    let last_leng = max2(EFF_ZERO, p(&last_sol.end, 0)? - p(&last_sol.start, 0)?);
                    let curr_leng = max2(EFF_ZERO, p(&curr.end, 0)? - p(&curr.start, 0)?);
                    if last_leng > curr_leng * 1000.00 || curr_leng > last_leng * 1000.00 {
                        do_combine = true;
                    }
                }
                break 'decide;
            }

            // for point solutions only
            if curr.solution_type == SolutionType::SingleValue {
                // evaluate solution points (statuses ignored)
                let pt1 = point_or_undef(k, curve, p(&curr_start, 0)?)?;
                let pt2 = point_or_undef(k, curve, p(&last_end, 0)?)?;
                let dist2 = pt1.distance_squared(pt2);

                // combine solutions when points are close enough together
                do_combine = dist2 < tol * tol;
                if do_combine {
                    break 'decide;
                }

                // see if points bound a coincident segment region (the first solution)
                let first = solutions.get_at(0)?;
                if is_coincident_point(
                    k,
                    surface,
                    uv_domain,
                    curve,
                    interval,
                    ctx.edge_tolerance,
                    &first,
                )? {
                    found_coincidence = true;
                }
            }

            // for range solutions only
            if curr.solution_type == SolutionType::RangeOfValues {
                // When SolutionIntervals overlap on both curve and surface - combine
                if p(&last_end, 0)? >= p(&curr_start, 0)? {
                    let u = p(&curr_start, 1)?;
                    let (u0, u1) = (p(&last_sol.start, 1)?, p(&last_sol.end, 1)?);
                    do_combine = is_between(u, u0, u1) && is_between(u, u0, u1);
                    break 'decide;
                }

                // curve parameter intervals from lastSol->End to nextSol->Start
                let mut ivl1 = Extent1d::new();
                let mut ivl2 = Extent1d::new();
                let mut ivl3 = Extent1d::new();
                ivl1.add_value(p(&last_end, 0)?);
                ivl1.add_value(p(&curr_start, 0)?);
                ivl2.add_value(p(&last_end, 1)?);
                ivl2.add_value(p(&curr_start, 1)?);
                ivl3.add_value(p(&last_end, 2)?);
                ivl3.add_value(p(&curr_start, 2)?);

                // drop two opening points [.33 .66] to the surface
                if !opening_drops(
                    k,
                    surface,
                    uv_domain,
                    curve,
                    [&ivl1, &ivl2, &ivl3],
                    tol,
                    2,
                    0.33,
                )? {
                    break 'decide;
                }

                // a small opening: assume the curves are coincident between solutions
                if ivl1.length() < t_size / 1000.0
                    && (ivl2.length() < uv_size.x / 1000.0 || ivl3.length() < uv_size.y / 1000.0)
                {
                    do_combine = true;
                    break 'decide;
                }

                // sample three more points [.25 .50 .75]
                if !opening_drops(
                    k,
                    surface,
                    uv_domain,
                    curve,
                    [&ivl1, &ivl2, &ivl3],
                    tol,
                    3,
                    0.25,
                )? {
                    break 'decide;
                }

                // the gap between two interval solutions is coincident
                do_combine = true;
            }
        }

        // continue here after deciding whether to combine current and last solutions
        let curr = solutions.get_at(ii)?;
        if do_combine {
            if curr.solution_type == SolutionType::RangeOfValues {
                // Expand rLastSol interval to cover rCurrSol interval
                if p(&last_sol.end, 0)? < p(&curr.end, 0)? {
                    last_sol.end = curr.end;
                    solutions.set_at(last_slot, &last_sol)?;
                }
            } else if curr.start.solution_value < last_end.solution_value {
                // just use the better solution
                solutions.set_at(last_slot, &curr)?;
            }
        } else {
            // Keep the current solution, packed into the array
            let next = if last_interval == NO_INDEX {
                last_point.saturating_add(1)
            } else if last_point == NO_INDEX {
                last_interval.saturating_add(1)
            } else {
                last_interval.max(last_point).saturating_add(1)
            };
            if curr_was_range {
                last_interval = next;
            } else {
                last_point = next;
            }
            if next != ii {
                solutions.set_at(next, &curr)?;
            }
        }
    }

    // Set rSolutions array Size after all combining has been done.
    if !solutions.is_empty() {
        let last = if last_interval == NO_INDEX {
            last_point
        } else if last_point == NO_INDEX {
            last_interval
        } else {
            last_interval.max(last_point)
        };
        // the packed indices never pass the array (shrinking only)
        if last >= solutions.len() {
            return Err(SolidError::sm_err(OP_COMBINE));
        }
        solutions.set_size(last + 1);
    }

    // Phase 2: remove coincident segments which include or are adjacent to point
    // intersections.
    let num = solutions.len();
    let mut cull = vec![false; num];
    let mut compress = false;
    for ii in 0..num {
        let inv_sol = solutions.get_at(ii)?;
        if let Some(c) = cull.get_mut(ii) {
            *c = false;
        }

        // skip point-solutions solutions
        if inv_sol.solution_type != SolutionType::RangeOfValues {
            continue;
        }
        let inv_start = inv_sol.start;
        let inv_end = inv_sol.end;
        let mut inv_span1 = Extent1d::new();
        inv_span1.add_value(p(&inv_start, 0)?);
        inv_span1.add_value(p(&inv_end, 0)?);
        let mut inv_span2 = Extent1d::new();
        inv_span2.add_value(p(&inv_start, 1)?);
        inv_span2.add_value(p(&inv_end, 1)?);
        // upstream adds the v values to the second span; the third stays Init()
        let inv_span3 = Extent1d::new();
        inv_span2.add_value(p(&inv_start, 2)?);
        inv_span2.add_value(p(&inv_end, 2)?);

        // search for point solutions within or adjacent to this interval solution
        for jj in 0..num {
            if ii == jj {
                continue;
            }
            let pt_sol = solutions.get_at(jj)?;

            // skip interval solutions
            if pt_sol.solution_type != SolutionType::SingleValue {
                continue;
            }
            let pt_start = pt_sol.start;

            // Get the cache subdivision intervals containing this point
            if let Some((pt_ivl1, dom)) = ctx.node_spans(&pt_sol.nodes)? {
                let pt_ivl2 = dom.u_interval();
                let pt_ivl3 = dom.v_interval();

                // overlap between IntervalSolution Span and PointSubDivision Span
                // (a disjoint intersection is Init())
                let xsect1 = pt_ivl1
                    .intersect(&inv_span1)
                    .unwrap_or_else(|_| Extent1d::new());
                let xsect2 = pt_ivl2
                    .intersect(&inv_span2)
                    .unwrap_or_else(|_| Extent1d::new());
                let xsect3 = pt_ivl3
                    .intersect(&inv_span3)
                    .unwrap_or_else(|_| Extent1d::new());
                if xsect1.is_init() || xsect2.is_init() || xsect3.is_init() {
                    continue;
                }
            }

            // Delete the range interval only if it's 'short': 1/1000 of the whole domain.
            let close_param = interval.length() / 1000.0;
            if inv_span1.length() < close_param {
                if let Some(c) = cull.get_mut(ii) {
                    *c = true;
                }
                compress = true;
            } else {
                // Range is long enough.  See about deleting the point solution.
                let pt_param1 = p(&pt_sol.start, 0)?;
                let delete_point = if pt_param1 < inv_span1.min() {
                    let dist = inv_span1.min() - pt_param1;
                    dist < close_param
                        && pt_sol.start.solution_value > 2.0 * inv_start.solution_value
                } else if pt_param1 > inv_span1.max() {
                    let dist = pt_param1 - inv_span1.max();
                    dist < close_param && pt_sol.start.solution_value > 2.0 * inv_end.solution_value
                } else {
                    // Point sol is inside the range sol: delete it.
                    true
                };
                if delete_point {
                    if let Some(c) = cull.get_mut(jj) {
                        *c = true;
                    }
                    compress = true;
                }
            }
            if cull.get(ii).copied().unwrap_or(false) {
                break;
            }

            // quit searching when point solutions get past interval solutions
            if p(&pt_start, 0)? > inv_span1.max()
                || p(&pt_start, 1)? > inv_span2.max()
                || p(&pt_start, 2)? > inv_span3.max()
            {
                break;
            }
        }
    }

    // compress the solutions array to remove all unwanted interval solutions
    if compress {
        let mut next = 0;
        for ii in 0..num {
            if cull.get(ii).copied().unwrap_or(false) {
                continue;
            }
            if ii != next {
                let s = solutions.get_at(ii)?;
                solutions.set_at(next, &s)?;
            }
            next += 1;
        }
        if !solutions.is_empty() {
            solutions.set_size(next);
        }
    }

    // Phase 3: turn small and degenerate ranges into point solutions (one pass).
    let mut length = BIG_DOUBLE;
    let mut is_degenerate = false;
    for ii in 0..solutions.len() {
        let mut curr = solutions.get_at(ii)?;
        if curr.solution_type != SolutionType::RangeOfValues {
            continue;
        }
        let dev_start = curr.start.solution_value;
        let dev_end = curr.end.solution_value;
        let mut ivl = Extent1d::from_value(p(&curr.start, 0)?);
        let mut reg = Extent2d::from_point(Point2::new(p(&curr.start, 1)?, p(&curr.start, 2)?));
        ivl.add_value(p(&curr.end, 0)?);
        reg.add_point(Point2::new(p(&curr.end, 1)?, p(&curr.end, 2)?));
        let ivl_length = ivl.length();
        let reg_length = reg.size().length();
        let pnt = curve.evaluate_point(k, p(&curr.start, 0)?)?;
        if length == BIG_DOUBLE {
            length = curve.approximate_length(k, interval, 5, false)?.0;
        }
        let seg_leng = curve.approximate_length(k, &ivl, 5, false)?.0;
        let scaled_zero = EFF_ZERO * (1.0 + pnt.max_dimension());
        let scaled_ps_tol = 100.0 * EFF_ZERO * (1.0 + interval.max_dimension());
        let scaled_ps_tol2 = 100.0 * EFF_ZERO * (1.0 + uv_domain.max_dimension());

        // keep nonZero intervals whose ends are exact intersections
        if dev_start < scaled_zero
            && dev_end < scaled_zero
            && ivl_length > scaled_ps_tol
            && reg_length > scaled_ps_tol2
        {
            continue;
        }

        let large_interval = seg_leng > length / 10.0;
        let small_interval = ivl_length < scaled_ps_tol || reg_length < scaled_ps_tol2;

        // for nonDegenerate intervals (status ignored)
        if !small_interval {
            // (`ClassifyIntersectingRangeSolution` writes only the deviation and the
            // first three parameters of `rCurrSol.m_vStart`)
            if let Some((degenerate, point)) = ignored(classify_intersecting_range_solution(
                k, surface, curve, &curr,
            ))? {
                is_degenerate = degenerate;
                if let Some(pt) = point {
                    curr.start.solution_value = pt.solution_value;
                    for (slot, v) in curr.start.parameters.iter_mut().zip(pt.parameters) {
                        *slot = v;
                    }
                    solutions.set_at(ii, &curr)?;
                }
            }
        }
        if large_interval && !is_degenerate {
            continue;
        }

        // Change very small and diverging intervals into a single pt solutions.
        if small_interval || is_degenerate {
            if curr.start.solution_value > curr.end.solution_value {
                curr.start = curr.end;
            }
            curr.solution_type = SolutionType::SingleValue;
            solutions.set_at(ii, &curr)?;
        }
    }

    // a NonDegenerate RangeOfValues solution means a coincidence
    for sol in solutions.as_slice() {
        if found_coincidence {
            break;
        }
        if sol.solution_type == SolutionType::RangeOfValues
            && p(&sol.end, 0)? - p(&sol.start, 0)? > 0.02 * interval.length()
        {
            found_coincidence = true;
        }
    }
    if found_coincidence {
        // keep every point solution not tangent with the surface
        let mut tmp = SolutionArray::new();
        for sol in solutions.as_slice() {
            if sol.solution_type == SolutionType::SingleValue {
                let pv = curve.evaluate(k, p(&sol.start, 0)?, 1, true, true)?;
                let uv = Point2::new(p(&sol.start, 1)?, p(&sol.start, 2)?);
                let normal = surface.evaluate_normal(k, uv, Side::Left, Side::Left)?;
                let tangent = pv.get(1).copied().ok_or(SolidError::sm_err(OP_COMBINE))?;
                if !tangent.is_perpendicular_to(normal, 2.0) {
                    tmp.add(sol);
                }
            }
        }

        // replace the coincident parts by the curve's drop onto the surface
        let dropped = drop_and_trim_curve(
            k,
            surface,
            ctx.surface_cache,
            uv_domain,
            curve,
            interval,
            tol,
        )?;
        if dropped.dist_to_surface > tol {
            return Ok(());
        }
        for uv_curve in &dropped.uv_curves {
            let ivl = uv_curve.natural_interval();
            let mut sol = Solution::new();
            sol.num_variables = 3;
            sol.num_objects = 2;
            sol.objects[0] = ctx.curve_handle;
            sol.objects[1] = ctx.surface_handle;
            sol.solution_type = SolutionType::RangeOfValues;
            let start = uv_curve.evaluate_point(k, ivl.min())?;
            sol.start.solution_value = dropped.dist_to_surface;
            set_p(&mut sol.start, 0, ivl.min())?;
            set_p(&mut sol.start, 1, start.x)?;
            set_p(&mut sol.start, 2, start.y)?;
            let end = uv_curve.evaluate_point(k, ivl.max())?;
            sol.end.solution_value = dropped.dist_to_surface;
            set_p(&mut sol.end, 0, ivl.max())?;
            set_p(&mut sol.end, 1, end.x)?;
            set_p(&mut sol.end, 2, end.y)?;
            tmp.add(&sol);
        }
        if !dropped.uv_curves.is_empty() {
            solutions.reset();
            solutions.append(&tmp);
        }
        return Ok(());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::solution_array::ObjectKind;
    use crate::nurbs::b_spline_curve::BSplineCurve;
    use crate::nurbs::b_spline_surface::BSplineSurface;
    use crate::nurbs::crv_basic::Curve as NurbCurve;
    use crate::nurbs::math::{CPoint, NOW};

    #[derive(Debug, Clone, Copy, PartialEq)]
    struct H(u8);
    impl SolutionObject for H {
        fn kind(&self) -> ObjectKind {
            ObjectKind::Surface
        }
    }

    fn plane() -> Surface {
        let net = (0..2)
            .map(|i| {
                (0..2)
                    .map(|j| CPoint::new(f64::from(i), f64::from(j), 0.0, NOW))
                    .collect()
            })
            .collect();
        let knots = vec![0.0, 0.0, 1.0, 1.0];
        let n = crate::nurbs::srf_basic::Surface::new(net, 1, 1, knots.clone(), knots).unwrap();
        Surface::from(BSplineSurface::from_nurb(n, false))
    }

    fn line() -> Curve {
        let n = NurbCurve::new(
            vec![
                CPoint::new(0.0, 0.5, 0.0, NOW),
                CPoint::new(1.0, 0.5, 0.0, NOW),
            ],
            1,
            vec![0.0, 0.0, 1.0, 1.0],
        )
        .unwrap();
        Curve::from(BSplineCurve::from_nurb(n, 3, false))
    }

    fn range(t0: f64, t1: f64) -> Solution<H> {
        let mut s = Solution::new();
        s.solution_type = SolutionType::RangeOfValues;
        s.num_variables = 3;
        s.start.parameters[0] = t0;
        s.start.parameters[1] = t0;
        s.start.parameters[2] = 0.5;
        s.end.parameters[0] = t1;
        s.end.parameters[1] = t1;
        s.end.parameters[2] = 0.5;
        s
    }

    #[test]
    fn touching_short_ranges_combine() {
        let k = Kernel::default();
        let mut sols = SolutionArray::new();
        sols.add(&range(0.100, 0.105));
        sols.add(&range(0.105, 0.110));
        // across the surface seam (v jumps by the domain size): kept apart
        let mut seam = range(0.500, 0.505);
        seam.start.parameters[2] = 1.5;
        seam.end.parameters[2] = 1.5;
        sols.add(&seam);
        global_curve_intersect_combine_solutions(
            &k,
            &plane(),
            &Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0),
            &line(),
            &Extent1d::from_min_max(0.0, 1.0),
            1e-6,
            &CombineContext::default(),
            &mut sols,
        )
        .unwrap();
        assert_eq!(sols.len(), 2);
        let first = sols.get_at(0).unwrap();
        assert_eq!(first.end.parameters[0], 0.110);
        assert_eq!(sols.get_at(1).unwrap().start.parameters[0], 0.500);
    }

    #[test]
    fn leading_point_on_coincident_line_becomes_range() {
        let k = Kernel::default();
        let mut sols = SolutionArray::new();
        let mut s = range(0.2, 0.2);
        s.solution_type = SolutionType::SingleValue;
        sols.add(&s);
        global_curve_intersect_combine_solutions(
            &k,
            &plane(),
            &Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0),
            &line(),
            &Extent1d::from_min_max(0.0, 1.0),
            1e-6,
            &CombineContext::default(),
            &mut sols,
        )
        .unwrap();
        // the line lies on the surface: its drop (an iso line) replaces the point
        // by the whole coincident range
        assert_eq!(sols.len(), 1);
        let r = sols.get_at(0).unwrap();
        assert_eq!(r.solution_type, SolutionType::RangeOfValues);
        assert_eq!(r.start.parameters[..3], [0.0, 0.0, 0.5]);
        assert_eq!(r.end.parameters[..3], [1.0, 1.0, 0.5]);
    }

    #[test]
    fn empty_solutions_stay_empty() {
        let k = Kernel::default();
        let mut sols: SolutionArray<H> = SolutionArray::new();
        global_curve_intersect_combine_solutions(
            &k,
            &plane(),
            &Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0),
            &line(),
            &Extent1d::from_min_max(0.0, 1.0),
            1e-6,
            &CombineContext::default(),
            &mut sols,
        )
        .unwrap();
        assert!(sols.is_empty());
    }
}
