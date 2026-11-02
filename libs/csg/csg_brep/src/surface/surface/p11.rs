//! `SmSurface`, part 11: the local surface/surface intersection from a pair of guess
//! points, and the leaf solves of the curve/surface and surface/surface global
//! solvers.
//!
//! Upstream: `source/SMLib/src/SmSurface.cpp`, `source/SMLib/inc/SmSurface.h`.
//!
//! | Rust                                              | Upstream                                         |
//! |---------------------------------------------------|--------------------------------------------------|
//! | [`Surface::local_surface_intersect`], [`SurfaceIntersection`], [`LocalSurfaceIntersectOptions`] | `SmSurface::LocalSurfaceIntersect` (declared virtual; no class overrides it) |
//! | [`CsGlobalSolver::local_solve_branch`] (and its `GlobalSolve` impl) | `SmCSGlobalSolver::LocalSolve`   |
//! | [`SsGlobalSolver::local_solve_branch`]            | `SmSSGlobalSolver::LocalSolve`                   |
//! | [`Surface::intersect_surface_boundaries`], [`BoundaryIntersections`] | `SmSurface::IntersectSurfaceBoundaries` (not virtual) |
//! | [`Surface::lift_curve`], [`LiftedCurve`]          | `SmSurface::LiftCurve` (not virtual)             |
//!
//! Out-params of the last two: `IntersectSurfaceBoundaries` returns the 3D curves and
//! deviations (its UV curves are always `NULL` and its types `SM_TC_TANGENT`; the
//! context only chooses where upstream allocates and is the kernel here); `LiftCurve`
//! returns the curve and its deviation. `IntersectSurfaceBoundaries` sorts the split
//! parameters with `f64::total_cmp` (upstream's shell sort with `<`; equal for finite
//! values).
//!
//! Deviations:
//! - **Patch data.** The leaf solves read the nodes' `SmBezierPatch` through the
//!   solvers' borrowed surface caches (`CsGlobalSolver::surface_cache`,
//!   `SsGlobalSolver::caches`; upstream casts `m_pData`), as `SmLSIGlobalSolver`'s
//!   (unit `surface.surface_p10`). The surface evaluated and the original surface
//!   (`sm_GetSurfaceToEvaluate`, `sm_GetOriginalSurface`) are the cache's surface; the
//!   patch surface (`mBA_pSurface`) is the node's surface held by the cache. Polar
//!   boxes are computed without being stored
//!   ([`SurfaceCache::polar_box_unstored`]; the values are the same). The curve span's
//!   ends (`GetEnds`) are the ones stored when the curve tree was built.
//! - **Out-param.** `LocalSolve`'s `rbNeedsMoreSubdivision` is the return value
//!   (always false, as upstream).
//! - **Out-params** become return values: `LocalSurfaceIntersect` returns `None` for
//!   `rbFoundAnswer == FALSE` and otherwise the solution it writes (`m_vStart[0..4]`
//!   and its value, the distance between the two surface points) as a
//!   [`SurfaceIntersection`]; the solution's objects (this surface and the other one,
//!   upstream pointers) are the caller's to attach.
//! - **Recursion.** `LocalSurfaceIntersect` calls itself once, with the iteration
//!   solution as guess, when that solution reached a domain boundary and no boundary
//!   was tried (the second call then starts on that boundary). The port carries a
//!   depth and returns `SM_ERR` past [`MAX_RECURSION`] calls (upstream reaches depth 1).
//! - **Statuses.** Every status upstream checks (`SER`) is propagated; the two
//!   `Unitize` calls of the plane normal ignore theirs (a zero normal becomes
//!   `(0, 0, 1)`, [`Vec3::unitize_or_z`]), and the leaf solves ignore
//!   `SmExtent2d::Intersect`'s (a failure leaves the patch domain).
//!   `AddSortedSolution` returns `void` upstream; its Rust errors (bad solution data)
//!   are propagated as in the sibling leaf solves. [`SolidError::Unported`] always
//!   propagates.
//! - **Signed directed solves** without a vector (`m_cpOptVectors == NULL`, which
//!   upstream dereferences) are `InvalidInput`.
//! - **Wired callees** (unit `surface.surface_p8`): the file-static
//!   `sm_IntersectSrfBndryCrv` (`do_boundary_testing`) and `sm_ExtractIsoCurve`
//!   (`LiftCurve`) call [`super::p8`]; a UV curve that is not of a B-spline kind is not an
//!   iso curve (upstream's signature takes an `SmBSplineCurve`).
//! - **Upstream issues.** `IntersectSurfaceBoundaries` dereferences a `NULL` curve
//!   array (UB-121; the port always returns the curves); `LocalSurfaceIntersect`'s
//!   boundary retries bind `!bFoundSolution` to the minimum test only (WR-99;
//!   reproduced).
//! - **Surface cache history.** `LiftCurve` reads the surface cache's continuity arrays
//!   (its C0 knot splits, and `GetDomainWithContinuity`'s C1 domain around each
//!   segment's start point) without building the cache's tree, so the arrays are empty
//!   while no earlier call has built it, and a point off the natural domain then gets
//!   the whole domain instead of one the end-knot discontinuities shrink to nothing.
//!   Both reads go through [`continuity_surface_cache`], which follows the surface's
//!   cache slot ([`Surface::cache_tree_state`]; `plan/findings.md`, "surface cache
//!   history").
//! - Debug-only work (`SM_ASSERT`, `SM_DEBUG_CODE` drawing) is not ported.

use crate::curve::composite_curve::CompositeCurve;
use crate::curve::crv_on_surf::{CrvOnSurf, CONTINUITY_ANGLE};
use crate::curve::curve::p8::CurveIntersectOptions;
use crate::curve::curve::{Curve, CurveKind};
use crate::error::{ignored, Result, SolidError};
use crate::geom::extent1d::{Extent1d, ExtentNd};
use crate::geom::extent2d::Extent2d;
use crate::geom::extent3d::Extent3d;
use crate::geom::geom_utility::p2::ContinuityType;
use crate::geom::global_solver::{
    Branch, GlobalSolve, GlobalSolver, PreferredSolution, SolutionPairType, SolverOperation,
};
use crate::geom::local_solve_nd::{BoundaryHandler, LocalSolveNd, TerminationReason};
use crate::geom::pseudo_box::{Projection, PseudoBox};
use crate::geom::solution_array::{
    ObjectKind, Solution, SolutionArray, SolutionObject, SolutionType, SortKey,
};
use crate::geom::tol::are_same;
use crate::geom::tree::{NodeData, NodeId, Tree};
use crate::geom::vector2d::{Point2, Vec2};
use crate::geom::vector3d::{sqrt, Point3, Vec3};
use crate::kernel::Kernel;
use crate::math::{max2, BIG_DOUBLE, EFF_ZERO, EFF_ZERO_SQ, EFF_ZERO_SQRT, PI};
use crate::nurbs::b_spline_curve_n_lib::TrimMode;
use crate::nurbs::b_spline_surface::SurfParamType;
use crate::nurbs::knots::Side;
use crate::surface::plane::Plane;
use crate::surface::surface_cache::p1::{BezierPatch, SurfaceCache};

use super::p1::{
    continuity_surface_cache, is_node_valid, surface_to_evaluate, CsGlobalSolver, SsiNormalizeEnfo,
    SspIntersectEnfo, SurfaceSolveContext,
};
use super::p2::{cs_are_perpen_disjoint_in, intersect_and_split, original_surface, set_up_breaks};
use super::p5::{cs_identical_solutions_in, DropPointOptions};
use super::p6::{boundary_iso_curve, sc_local_solve, ScLocalSolveNodes};
use super::p7::{pass_ss_convergence_test, SsGlobalSolver};
use super::{Surface, SurfaceKind};

const OP_LSI: &str = "surface::surface::p11::Surface::local_surface_intersect";
const OP_CS: &str = "surface::surface::p11::CsGlobalSolver::local_solve_branch";
const OP_SS: &str = "surface::surface::p11::SsGlobalSolver::local_solve_branch";
const OP_LIFT: &str = "surface::surface::p11::Surface::lift_curve";
const OP_ISB: &str = "surface::surface::p11::Surface::intersect_surface_boundaries";

/// The deepest `LocalSurfaceIntersect` self call accepted (upstream reaches 1).
pub const MAX_RECURSION: usize = 16;

/// Three degrees, in radians (`3.0 * SM_PI / 180.0`).
fn three_degrees() -> f64 {
    3.0 * PI / 180.0
}

// ---- calls into units that are not ported yet ------------------------------------------------

/// What `sm_IntersectSrfBndryCrv` finds: the surface point and the iso curve
/// parameter (the deviation and tangency flag are not read by this unit).
#[derive(Debug, Clone, Copy)]
struct BoundaryHit {
    uv: Point2,
    t: f64,
}

/// `sm_IntersectSrfBndryCrv(surface, domain, curve_surface, .., param, iso, tol,
/// guess_uv, guess_t, ..)`: the intersection of `surface` with the iso parameter curve
/// of `curve_surface`; `None` when not found. Upstream's `curve_surface` domain and
/// plane normal arguments are unused there and dropped; the guess is a copy.
#[allow(clippy::too_many_arguments)]
fn intersect_srf_bndry_crv(
    k: &Kernel,
    surface: &Surface,
    uv_domain: &Extent2d,
    curve_surface: &Surface,
    param: SurfParamType,
    iso_parameter: f64,
    tol_3d: f64,
    guess_uv: Point2,
    guess_t: f64,
) -> Result<Option<BoundaryHit>> {
    // (sm_IntersectSrfBndryCrv, surface.surface_p8; the deviation and the tangency flag
    // are not read here)
    let hit = super::p8::intersect_srf_bndry_crv(
        k,
        surface,
        uv_domain,
        curve_surface,
        param,
        iso_parameter,
        tol_3d,
        guess_uv,
        guess_t,
    )?;
    Ok(hit.found.then_some(BoundaryHit {
        uv: hit.uv,
        t: hit.t,
    }))
}

/// `SmSurface::LocalSurfaceSolve(domain, other, other_domain, op, tol, NULL, vectors,
/// uv, other_uv, found, solution)` (unit `surface.surface_p3`; not overridden, and
/// its distance tolerance is unused): `[u, v, other u, other v]` and the solution
/// value; `None` when not found. `opt_vectors` empty is upstream's `NULL`.
#[allow(clippy::too_many_arguments)]
fn local_surface_solve(
    k: &Kernel,
    surface: &Surface,
    uv_domain: &Extent2d,
    other: &Surface,
    other_uv_domain: &Extent2d,
    op: SolverOperation,
    opt_vectors: &[Vec3],
    uv: Point2,
    other_uv: Point2,
) -> Result<Option<([f64; 4], f64)>> {
    let found = surface.local_surface_solve(
        k,
        uv_domain,
        other,
        other_uv_domain,
        op,
        opt_vectors,
        uv,
        other_uv,
    )?;
    Ok(found.map(|s| ([s.uv.x, s.uv.y, s.other_uv.x, s.other_uv.y], s.distance)))
}

// ---- LocalSurfaceIntersect -------------------------------------------------------------------

/// A surface/surface intersection point found by [`Surface::local_surface_intersect`]
/// (upstream's `rSolution`: four variables, single valued).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceIntersection {
    /// `m_vStart[0..2]`: the point on this surface.
    pub uv: Point2,
    /// `m_vStart[2..4]`: the point on the other surface.
    pub other_uv: Point2,
    /// `m_vStart.m_dSolutionValue`: the distance between the two surface points.
    pub deviation: f64,
}

/// The optional arguments of [`Surface::local_surface_intersect`].
#[derive(Debug, Clone, Copy, Default)]
pub struct LocalSurfaceIntersectOptions {
    /// `cpOptPlaneNormal`: the solution lies on the plane through the guess points
    /// with this normal (else the cross product of the surface normals, recomputed
    /// while iterating: steepest descent).
    pub plane_normal: Option<Vec3>,
    /// `cpOptPreviousUV`: the previous point on this surface when stepping, so a
    /// boundary solution right next to it is not preferred.
    pub previous_uv: Option<Point2>,
    /// `bDoBoundaryTesting`: look for intersections with the surfaces' boundaries.
    pub do_boundary_testing: bool,
}

/// `sSolutionVector[..]` as `Point2`s.
fn split(sv: &[f64; 4]) -> (Point2, Point2) {
    (Point2::new(sv[0], sv[1]), Point2::new(sv[2], sv[3]))
}

/// The four parameters of a local solve's solution vector.
fn four(x: &[f64]) -> Result<[f64; 4]> {
    match x {
        [a, b, c, d, ..] => Ok([*a, *b, *c, *d]),
        _ => Err(SolidError::sm_err(OP_LSI)),
    }
}

/// The intervals of the two UV domains (`SmExtentNd(4)`).
fn intervals(uv_domain: &Extent2d, other_uv_domain: &Extent2d) -> ExtentNd {
    let mut ivls = ExtentNd::new(4);
    let values = [
        Extent1d::from_min_max(uv_domain.min().x, uv_domain.max().x),
        Extent1d::from_min_max(uv_domain.min().y, uv_domain.max().y),
        Extent1d::from_min_max(other_uv_domain.min().x, other_uv_domain.max().x),
        Extent1d::from_min_max(other_uv_domain.min().y, other_uv_domain.max().y),
    ];
    for (i, ivl) in values.into_iter().enumerate() {
        if let Some(slot) = ivls.get_mut(i) {
            *slot = ivl;
        }
    }
    ivls
}

impl Surface {
    /// The surface/surface intersection point near the guesses `uv_guess` (on this
    /// surface over `uv_domain`) and `other_uv_guess` (on `other` over
    /// `other_uv_domain`) (`LocalSurfaceIntersect`): a Newton solve of the two
    /// surfaces and a plane (through the guess points, normal to the given normal or to
    /// the cross product of the surface normals), with boundary iso curve solves when
    /// testing boundaries, tangent-touching refinements when the normals are nearly
    /// parallel, and a normalized surface/surface/plane solve when the solution leaves
    /// a gap. `None` when no point within `distance_tolerance` was found (also when the
    /// guess normals are parallel and no plane normal is given).
    ///
    /// Dispatch: `EvaluatePoint`/`EvaluateNormal` are the virtuals of each surface's
    /// own kind ([`Surface::evaluate_point`], [`Surface::evaluate_normal`]).
    ///
    /// # Errors
    ///
    /// Those of the evaluations, the angle between zero normals and the solves (all
    /// checked upstream); `SM_ERR` past [`MAX_RECURSION`] self calls.
    pub fn local_surface_intersect(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        other: &Surface,
        other_uv_domain: &Extent2d,
        distance_tolerance: f64,
        uv_guess: Point2,
        other_uv_guess: Point2,
        opts: &LocalSurfaceIntersectOptions,
    ) -> Result<Option<SurfaceIntersection>> {
        self.local_surface_intersect_at(
            k,
            uv_domain,
            other,
            other_uv_domain,
            distance_tolerance,
            uv_guess,
            other_uv_guess,
            opts,
            0,
        )
    }

    #[allow(clippy::too_many_lines)]
    fn local_surface_intersect_at(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        other: &Surface,
        other_uv_domain: &Extent2d,
        distance_tolerance: f64,
        uv_guess: Point2,
        other_uv_guess: Point2,
        opts: &LocalSurfaceIntersectOptions,
        depth: usize,
    ) -> Result<Option<SurfaceIntersection>> {
        if depth > MAX_RECURSION {
            return Err(SolidError::sm_err(OP_LSI));
        }
        let do_boundary = opts.do_boundary_testing;
        let three_deg = three_degrees();

        // locals for NewtonRaphson Solutions
        let mut found_solution = false;

        // locals for boundary solutions
        let mut tried_boundary = false;
        let mut this_on_boundary = false;
        let mut other_on_boundary = false;
        let mut found_boundary = false;
        let mut uv_boundary = Point2::UNDEF;
        let mut uv_other_boundary = Point2::UNDEF;

        // 1st - When testing boundaries: when guess point is on a natural boundary look
        //       for 1st boundaryCurve/Surface intersection
        if do_boundary {
            // when guessPoint is on Surface1 U Boundary
            if are_same(uv_guess.x, uv_domain.min().x) || are_same(uv_guess.x, uv_domain.max().x) {
                tried_boundary = true;
                this_on_boundary = true;

                // intersect OtherSurface with const u Surface boundary isoParameterCurve
                // (SER: checked; sm_IntersectSrfBndryCrv over the other surface's kind)
                let hit = intersect_srf_bndry_crv(
                    k,
                    other,
                    other_uv_domain,
                    self,
                    SurfParamType::U,
                    uv_guess.x,
                    distance_tolerance,
                    other_uv_guess,
                    uv_guess.y,
                )?;
                found_solution = hit.is_some();

                // save found solutions
                if let Some(h) = hit {
                    found_boundary = true;
                    uv_other_boundary = h.uv;
                    uv_boundary = Point2::new(uv_guess.x, h.t);
                }
            }

            // when guessPoint is on Surface1 V Boundary
            if !found_solution
                && (are_same(uv_guess.y, uv_domain.min().y)
                    || are_same(uv_guess.y, uv_domain.max().y))
            {
                tried_boundary = true;
                this_on_boundary = true;

                // intersect OtherSurface with const v Surface boundary isoParameterCurve
                // (SER: checked)
                let hit = intersect_srf_bndry_crv(
                    k,
                    other,
                    other_uv_domain,
                    self,
                    SurfParamType::V,
                    uv_guess.y,
                    distance_tolerance,
                    other_uv_guess,
                    uv_guess.x,
                )?;
                found_solution = hit.is_some();
                if let Some(h) = hit {
                    found_boundary = true;
                    uv_other_boundary = h.uv;
                    uv_boundary = Point2::new(h.t, uv_guess.y);
                }
            }

            // when guessPoint is on Surface2 U Boundary
            if !found_solution
                && (are_same(other_uv_guess.x, other_uv_domain.min().x)
                    || are_same(other_uv_guess.x, other_uv_domain.max().x))
            {
                other_on_boundary = true;
                tried_boundary = true;

                // intersect Surface with const u OtherSurface boundary isoParameterCurve
                // (SER: checked)
                let hit = intersect_srf_bndry_crv(
                    k,
                    self,
                    uv_domain,
                    other,
                    SurfParamType::U,
                    other_uv_guess.x,
                    distance_tolerance,
                    uv_guess,
                    other_uv_guess.y,
                )?;
                found_solution = hit.is_some();
                if let Some(h) = hit {
                    found_boundary = true;
                    uv_boundary = h.uv;
                    uv_other_boundary = Point2::new(other_uv_guess.x, h.t);
                }
            }

            // when guessPoint is on Surface2 V Boundary
            if !found_solution
                && (are_same(other_uv_guess.y, other_uv_domain.min().y)
                    || are_same(other_uv_guess.y, other_uv_domain.max().y))
            {
                other_on_boundary = true;
                tried_boundary = true;

                // intersect Surface with const v OtherSurface boundary isoParameterCurve
                // (SER: checked)
                let hit = intersect_srf_bndry_crv(
                    k,
                    self,
                    uv_domain,
                    other,
                    SurfParamType::V,
                    other_uv_guess.y,
                    distance_tolerance,
                    uv_guess,
                    other_uv_guess.x,
                )?;
                // (its found flag is overwritten by the solve below before any read)
                if let Some(h) = hit {
                    found_boundary = true;
                    uv_boundary = h.uv;
                    uv_other_boundary = Point2::new(h.t, other_uv_guess.y);
                }
            }
        }

        // 2nd - iterate to Surf/Surf intersection Point from GuessPoint
        let ivls = intervals(uv_domain, other_uv_domain);
        // Do not do periodicity for this function.
        let periodicities = [false; 4];
        let guess = [uv_guess.x, uv_guess.y, other_uv_guess.x, other_uv_guess.y];

        // get Surface GuessPoints (SER: checked; each surface's own EvaluatePoint)
        let pnt1 = self.evaluate_point(k, uv_guess)?;
        let pnt2 = other.evaluate_point(k, other_uv_guess)?;

        // Select through point for plane containing answer
        let plane_orig: Point3 = if this_on_boundary {
            pnt1
        } else if other_on_boundary {
            pnt2
        } else {
            (pnt1 + pnt2) / 2.0
        };

        // Select normal for plane containing answer
        let steepest_descent;
        let mut plane_norm = if let Some(n) = opts.plane_normal {
            // In some cases like tracing tangency curves we need to get a normal from
            // outside because of the problems with the surface normals being the same.
            steepest_descent = false;
            n
        } else {
            // (SER: checked; EvaluateNormal(uv, TRUE, TRUE) of each surface's kind)
            let norm1 = self.evaluate_normal(k, uv_guess, Side::Left, Side::Left)?;
            let norm2 = other.evaluate_normal(k, other_uv_guess, Side::Left, Side::Left)?;
            steepest_descent = true;
            let n = norm1.cross(norm2);

            // check for bad starting points
            if n.length_squared() < EFF_ZERO_SQ {
                // Bad starting point - just quit and try a different one
                return Ok(None);
            }
            n
        };

        // build plane containing answers (SE: Unitize status ignored)
        plane_norm = plane_norm.unitize_or_z();
        let plane_surface = Plane::from_normal(plane_orig, plane_norm);

        // instance SmSSPIntersectENFO surf/surf/plane NewtonRaphson Solver
        let ssi_node_tol = distance_tolerance / 100.0;
        let mut eval_fun = SspIntersectEnfo::new(
            k,
            self,
            other,
            steepest_descent,
            Some(&plane_surface),
            ssi_node_tol * ssi_node_tol,
        );
        let (ls_found, ls_x, termination) = {
            let mut ls = LocalSolveNd::new(&mut eval_fun, Some(&ivls), Some(&periodicities));
            ls.set_desired_accuracy(EFF_ZERO_SQ);
            ls.set_boundary_handler(BoundaryHandler::TotalBoundaryHits, 3);
            ls.set_maximum_iterations(20);

            // Solve at 4 times tolerance for now and cull solutions to tolerance later
            // (SER: checked)
            let r = ls.solve_it(&guess, distance_tolerance * 4.0)?;
            (r.found, r.x, ls.termination_reason())
        };
        found_solution = ls_found;
        let mut sv = four(&ls_x)?;

        // set solution points
        let (mut uv, mut uv_other) = split(&sv);

        // When Solution iterated to a boundary from guessPoints not on the boundary and
        // we are doing BoundaryTesting
        if do_boundary
            && found_solution
            && !tried_boundary
            && (uv_domain.is_point_on_boundary(uv, EFF_ZERO).0
                || other_uv_domain.is_point_on_boundary(uv_other, EFF_ZERO).0)
        {
            // do intersection over with guessPoint = iteration Solution so that
            // BoundarySolutions can be found and selected.
            return self.local_surface_intersect_at(
                k,
                uv_domain,
                other,
                other_uv_domain,
                distance_tolerance,
                uv,
                uv_other,
                opts,
                depth + 1,
            );
        }

        // 3rd - Select Good Boundary Solutions over Iteration Solutions when we have them
        if found_boundary {
            let mut use_boundary = true;

            // skip boundary solutions that are very close to the last solution in favor
            // of iterationSolutions that aren't.
            if let (true, Some(prev)) = (found_solution, opts.previous_uv) {
                let uv_pt = Point2::new(sv[0], sv[1]);
                let dist_interior = prev.distance(uv_pt);
                let dist_boundary = prev.distance(uv_boundary);
                if dist_interior > 100.0 * dist_boundary {
                    use_boundary = false;
                }
            }

            // save the boundarySolution
            if use_boundary {
                found_solution = true;
                sv = [
                    uv_boundary.x,
                    uv_boundary.y,
                    uv_other_boundary.x,
                    uv_other_boundary.y,
                ];
                (uv, uv_other) = split(&sv);
            }
        }

        // 4th - When no Iteration solution was found because boundaries were hit - look
        //       for a surf/BoundaryCurve solution at the iteration stopping point
        if !found_solution && termination == TerminationReason::OutOfBounds && do_boundary {
            // when solution search stopped on Surface1 U Boundary
            if are_same(sv[0], uv_domain.min().x) || are_same(sv[0], uv_domain.max().x) {
                // (SER: checked)
                let hit = intersect_srf_bndry_crv(
                    k,
                    other,
                    other_uv_domain,
                    self,
                    SurfParamType::U,
                    sv[0],
                    distance_tolerance,
                    Point2::new(sv[2], sv[3]),
                    sv[1],
                )?;
                found_solution = hit.is_some();
                if let Some(h) = hit {
                    sv = [sv[0], h.t, h.uv.x, h.uv.y];
                    (uv, uv_other) = split(&sv);
                }
            }

            // when solution search stopped on Surface1 V Boundary (upstream's
            // precedence: `(!found && min) || max`)
            if (!found_solution && are_same(sv[1], uv_domain.min().y))
                || are_same(sv[1], uv_domain.max().y)
            {
                // (SER: checked)
                let hit = intersect_srf_bndry_crv(
                    k,
                    other,
                    other_uv_domain,
                    self,
                    SurfParamType::V,
                    sv[1],
                    distance_tolerance,
                    Point2::new(sv[2], sv[3]),
                    sv[0],
                )?;
                found_solution = hit.is_some();
                if let Some(h) = hit {
                    sv = [h.t, sv[1], h.uv.x, h.uv.y];
                    (uv, uv_other) = split(&sv);
                }
            }

            // when solution search stopped on Surface2 U Boundary
            if (!found_solution && are_same(sv[2], other_uv_domain.min().x))
                || are_same(sv[2], other_uv_domain.max().x)
            {
                // (SER: checked)
                let hit = intersect_srf_bndry_crv(
                    k,
                    self,
                    uv_domain,
                    other,
                    SurfParamType::U,
                    sv[2],
                    distance_tolerance,
                    Point2::new(sv[0], sv[1]),
                    sv[3],
                )?;
                found_solution = hit.is_some();
                if let Some(h) = hit {
                    sv = [h.uv.x, h.uv.y, sv[2], h.t];
                    (uv, uv_other) = split(&sv);
                }
            }

            // when solution search stopped on Surface2 V Boundary
            if (!found_solution && are_same(sv[3], other_uv_domain.min().y))
                || are_same(sv[3], other_uv_domain.max().y)
            {
                // (SER: checked)
                let hit = intersect_srf_bndry_crv(
                    k,
                    self,
                    uv_domain,
                    other,
                    SurfParamType::V,
                    sv[3],
                    distance_tolerance,
                    Point2::new(sv[0], sv[1]),
                    sv[2],
                )?;
                found_solution = hit.is_some();
                if let Some(h) = hit {
                    sv = [h.uv.x, h.uv.y, h.t, sv[3]];
                    (uv, uv_other) = split(&sv);
                }
            }
        }

        // 5th - when a solution was found and bSteepestDescent is TRUE and the surface
        //       normals are parallel to one another - look for a solution where the
        //       surfaces are a minDist from one another. A TangentTouching case
        if found_solution && steepest_descent {
            // get angle between surface normals at the solution (SER: checked)
            let norm1 = self.evaluate_normal(k, uv, Side::Left, Side::Left)?;
            let norm2 = other.evaluate_normal(k, uv_other, Side::Left, Side::Left)?;
            let angle = norm1.angle_between(norm2)?;

            // If the angle is within 3 degrees of being parallel then try refining
            // solution with a normalized convergence.
            if angle < three_deg || angle > PI - three_deg {
                // find point of closest approach between the two surfaces in current
                // solution neighborhood (SER: checked)
                let sol_norm = local_surface_solve(
                    k,
                    self,
                    uv_domain,
                    other,
                    other_uv_domain,
                    SolverOperation::Minimize,
                    &[],
                    uv,
                    uv_other,
                )?;
                // (SER: checked)
                let p1 = self.evaluate_point(k, uv)?;
                let p2 = other.evaluate_point(k, uv_other)?;
                let distance_between_points = p1.distance(p2);

                // use new solution when deviation is better
                if let Some((x, value)) = sol_norm {
                    if value < distance_between_points {
                        sv = x;
                        (uv, uv_other) = split(&sv);
                    }
                }
            }
        }

        // 6th - when a Non-Zero Deviation solution has been found
        if found_solution {
            // (SER: checked)
            let p1 = self.evaluate_point(k, uv)?;
            let p2 = other.evaluate_point(k, uv_other)?;
            let mut dist = p1.distance(p2);

            // If we don't converge to an exact answer then try converging to
            // surf/surf/plane intersection with a variation on the set of equations
            if dist > EFF_ZERO {
                // (SER: checked)
                let norm1 = self.evaluate_normal(k, uv, Side::Left, Side::Left)?;
                let norm2 = other.evaluate_normal(k, uv_other, Side::Left, Side::Left)?;
                let angle = norm1.angle_between(norm2)?;

                // If the angle is within 3 degrees of being perpendicular than we should
                // try to do a normalized convergence for minimization.
                if angle < three_deg || angle > PI - three_deg {
                    // Recompute plane normal if needed
                    if opts.plane_normal.is_none() {
                        let new_plane_norm = norm1.cross(norm2);
                        if new_plane_norm.length_squared() > EFF_ZERO_SQ {
                            plane_norm = new_plane_norm;
                        }
                    }
                    // (SE: Unitize status ignored)
                    plane_norm = plane_norm.unitize_or_z();
                    let plane_d = -p1.dot(plane_norm);

                    let ivls2 = intervals(uv_domain, other_uv_domain);
                    let periodicities2 = [false; 4];
                    let guess2 = [uv.x, uv.y, uv_other.x, uv_other.y];

                    // set up surf/surf/plane intersection NewtonRaphson solver using
                    // different eqns
                    let mut eval_fun2 = SsiNormalizeEnfo::new(
                        k,
                        self,
                        other,
                        EFF_ZERO * EFF_ZERO,
                        plane_norm,
                        plane_d,
                    );
                    let r2 = {
                        let mut ls2 =
                            LocalSolveNd::new(&mut eval_fun2, Some(&ivls2), Some(&periodicities2));
                        ls2.set_boundary_handler(BoundaryHandler::TotalBoundaryHits, 8);

                        // solve it (SER: checked)
                        ls2.solve_it(&guess2, EFF_ZERO_SQRT)?
                    };

                    // when an answer was found
                    if r2.found {
                        let sv2 = four(&r2.x)?;
                        let (uv2, uv_other2) = split(&sv2);
                        // (SER: checked)
                        let pt1 = self.evaluate_point(k, uv2)?;
                        let pt2 = other.evaluate_point(k, uv_other2)?;
                        let dist2 = pt1.distance(pt2);

                        // save solution when deviation is better than previous solution
                        // deviation
                        if dist2 < dist {
                            dist = dist2;
                            sv = sv2;
                            (uv, uv_other) = split(&sv);
                        }
                    }

                    // when distance is not exactly zero
                    if dist != 0.0 {
                        // find point of closest approach between the two surfaces in
                        // current solution neighborhood (SER: checked; the found flag
                        // overwrites bFoundSolution upstream)
                        let sol = local_surface_solve(
                            k,
                            self,
                            uv_domain,
                            other,
                            other_uv_domain,
                            SolverOperation::Normalize,
                            &[],
                            uv,
                            uv_other,
                        )?;
                        if let Some((x, _)) = sol {
                            // get the Surf1Point/Surf2Point gap size (SER: checked)
                            let (uv2, uv_other2) = split(&x);
                            let pt1 = self.evaluate_point(k, uv2)?;
                            let pt2 = other.evaluate_point(k, uv_other2)?;
                            let dist2 = pt1.distance(pt2);

                            // save solution when this is a better answer
                            if dist2 < dist {
                                dist = dist2;
                                sv = x;
                            }
                        }
                    }
                }
            }

            // when deviation is within tolerance - save output
            if dist < distance_tolerance {
                let (uv, other_uv) = split(&sv);
                return Ok(Some(SurfaceIntersection {
                    uv,
                    other_uv,
                    deviation: dist,
                }));
            }
        }

        // all done
        Ok(None)
    }
}

// ---- the global solvers' local solves -------------------------------------------------------

/// `SmContinuityType` order (`<=` compares the upstream values).
const fn ct(c: ContinuityType) -> i32 {
    c as i32
}

/// Copies the node ids of `branch` into `solution.nodes[0..2]`.
fn set_nodes<O>(solution: &mut Solution<O>, branch: &Branch) {
    for (slot, id) in solution
        .nodes
        .iter_mut()
        .zip(branch.iter().copied())
        .take(2)
    {
        *slot = id;
    }
}

/// The patch of a surface tree node and the surfaces it refers to: the cache's
/// surface for `sm_GetOriginalSurface` / `sm_GetSurfaceToEvaluate` and the patch's
/// `mBA_pSurface`.
struct PatchView<'c, O> {
    patch: BezierPatch<O>,
    eval: &'c Surface,
    patch_surface: &'c Surface,
}

/// Resolves the patch of surface node `id` in `cache` (upstream casts `m_pData` and
/// reads `mBA_pSurface`; the tree's original surface must be the cache's).
fn patch_view<'c, O: SolutionObject>(
    op: &'static str,
    tree: &Tree<O>,
    cache: &'c SurfaceCache<O>,
    id: NodeId,
) -> Result<PatchView<'c, O>> {
    let node = tree.node(id)?;
    let patch = cache.patch(id)?.clone();
    if Some(surface_to_evaluate(tree, node)?) != cache.surface_handle() {
        return Err(SolidError::invalid_input(
            op,
            "surface cache does not own the tree's surface",
        ));
    }
    let handle = match node.data {
        NodeData::Surface(aux) => aux.surface,
        _ => None,
    };
    let patch_surface = cache.node_surface(handle)?;
    Ok(PatchView {
        patch,
        eval: cache.surface(),
        patch_surface,
    })
}

/// The tight box test of the signed directed solves: the first box, grown by
/// `tol`, and the second projected along `vector` are disjoint
/// (`AreProjectDisjoint(.., SM_PT_PARALLEL, &center, &m_cpOptVectors[0], NULL)`).
fn project_disjoint(first: &PseudoBox, second: &PseudoBox, tol: f64, vector: Vec3) -> bool {
    let mut tmp = *first;
    tmp.expand_absolute(tol);
    let proj_point = tmp.evaluate(0.5, 0.5, 0.5);
    let proj = Projection::Parallel {
        point: proj_point,
        normal: vector,
    };
    tmp.are_project_disjoint(second, &proj)
}

/// `m_cpOptVectors[0]` (upstream dereferences `NULL` without vectors).
fn first_vector(op: &'static str, vectors: &[Vec3]) -> Result<Vec3> {
    vectors
        .first()
        .copied()
        .ok_or_else(|| SolidError::invalid_input(op, "signed directed solve without a vector"))
}

/// The minimize cull of both local solves: the boxes, each grown by half of the best
/// distance so far plus the tolerance, are disjoint.
fn best_answer_disjoint(first: &PseudoBox, second: &PseudoBox, best_sq: f64, tol: f64) -> bool {
    let mut b1 = *first;
    let mut b2 = *second;
    b1.expand_absolute((sqrt(best_sq) + tol) / 2.0);
    b2.expand_absolute((sqrt(best_sq) + tol) / 2.0);
    b1.are_disjoint(&b2, None)
}

/// A patch domain limited to a solve domain, with its `u` and `v` side continuities.
type LimitedDomain = (Extent2d, [ContinuityType; 2], [ContinuityType; 2]);

/// The patch domain limited to the solve domain with the patch side continuities
/// (sides on the solve domain's boundary become discontinuous); `None` when the
/// limited domain has a zero length side (no work).
fn limit_domain(
    patch_domain: Extent2d,
    solve_domain: &Extent2d,
    mut u_conts: [ContinuityType; 2],
    mut v_conts: [ContinuityType; 2],
) -> Result<Option<LimitedDomain>> {
    let mut domain = patch_domain;
    if solve_domain.min().x >= domain.min().x {
        u_conts[0] = ContinuityType::Discontinuous;
    }
    if solve_domain.max().x <= domain.max().x {
        u_conts[1] = ContinuityType::Discontinuous;
    }
    if solve_domain.min().y >= domain.min().y {
        v_conts[0] = ContinuityType::Discontinuous;
    }
    if solve_domain.max().y <= domain.max().y {
        v_conts[1] = ContinuityType::Discontinuous;
    }
    if !domain.is_contained_by(solve_domain, EFF_ZERO) {
        // (SE: SmExtent2d::Intersect's status is ignored; a failure leaves the domain)
        if let Some(d) = ignored(domain.intersect(solve_domain))? {
            domain = d;
        }
        // Check for zero in either domain bound
        if are_same(domain.max().x, domain.min().x) || are_same(domain.max().y, domain.min().y) {
            return Ok(None);
        }
    }
    Ok(Some((domain, u_conts, v_conts)))
}

impl<'t, O: SolutionObject + 't> CsGlobalSolver<'t, '_, O> {
    /// `SmCSGlobalSolver::LocalSolve`: solves the curve span of `branch[0]` against
    /// the Bezier patch of `branch[1]` (culling by node class, domains, boxes and, for
    /// signed directed minimizes, the projected boxes and the patch's normal cone; for
    /// minimizes a curve/surface intersection first), then `sm_SCLocalSolve`, adding the
    /// solutions `[t, u, v]` inside the face to `base.solutions`. Returns the request
    /// for more subdivision (always false).
    ///
    /// Dispatch: the evaluations run on the cache's surface (`sm_GetSurfaceToEvaluate`,
    /// its own kind) and `self.curve`.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` without `curve` or `surface_cache`; `InvalidInput` when the
    /// nodes are not a span and a patch of those caches, or for a signed directed
    /// solve without a vector; those of the checked (`SER`) calls:
    /// `SmExtent1d::Intersect`, `LocalCurveIntersect` and `sm_SCLocalSolve`.
    #[allow(clippy::too_many_lines)]
    pub fn local_solve_branch(&mut self, branch: &Branch) -> Result<bool> {
        let k = self.kernel;
        let curve_node = *self.base.branch_node(branch, 0)?;
        let surface_node = *self.base.branch_node(branch, 1)?;
        let surface_id = branch
            .get(1)
            .copied()
            .flatten()
            .ok_or(SolidError::null_pointer(OP_CS))?;
        let cache = self.surface_cache.ok_or(SolidError::null_pointer(OP_CS))?;
        let curve = self.curve.ok_or(SolidError::null_pointer(OP_CS))?;
        let curve_tree = self.base.tree(0)?;
        let surface_tree = self.base.tree(1)?;
        let span = *curve_tree.curve_span(&curve_node)?;
        let view = patch_view(OP_CS, surface_tree, cache, surface_id)?;
        let patch = &view.patch;

        // If the classification of the node is outside then we can quit here
        if !is_node_valid(surface_tree.owner(), cache, &surface_node)? {
            return Ok(false);
        }

        let surface_handle = original_surface(surface_tree, &surface_node)?;
        let curve_handle = curve_tree.owner_object()?;
        let mut ivl = *span.aux.interval();
        let op = self.base.operation;
        let tol = self.base.tolerance_3d;

        // Do interval tests and reject if disjoint
        if patch.aux.uv_domain.are_disjoint(&self.uv_domain, 0.0) {
            return Ok(false);
        }
        if self.interval.are_disjoint(&ivl, 0.0) {
            return Ok(false);
        }

        if op == SolverOperation::Signed3dDirectedMinimize {
            // Do a tight bounding box test in projected space to see if they are
            // disjoint
            let vector = first_vector(OP_CS, &self.base.opt_vectors)?;
            if project_disjoint(&span.pseudo_box, &patch.pseudo_box, tol, vector) {
                return Ok(false);
            }
            // Do a test to see if the range of vectors possible with the curve do not
            // cross the perpendicular of the normal field.
            let polar = cache.polar_box_unstored(k, surface_id)?;
            let [start, end] = span.aux.ends;
            let line_vec = end - start;
            if line_vec.length_squared() > EFF_ZERO_SQ {
                let mut curve_angle = span.aux.max_turning_angle_deg * PI / 180.0;
                curve_angle += 3.0 * PI / 180.0;
                if polar.is_perpendicular_disjoint_with_cone(line_vec, curve_angle) {
                    return Ok(false);
                }
            }
        }

        let mut t_conts = span.aux.conts;
        let Some((uv_domain, u_conts, v_conts)) = limit_domain(
            patch.aux.uv_domain,
            &self.uv_domain,
            patch.u_curve_conts,
            patch.v_curve_conts,
        )?
        else {
            return Ok(false);
        };

        if self.interval.min() >= ivl.min() {
            t_conts[0] = ContinuityType::Discontinuous;
        }
        if self.interval.max() <= ivl.max() {
            t_conts[1] = ContinuityType::Discontinuous;
        }
        if !ivl.is_contained_by(&self.interval, EFF_ZERO) {
            // (SER: checked)
            ivl = ivl.intersect(&self.interval)?;
            // Check for zero length intervals
            if are_same(ivl.max(), ivl.min()) {
                return Ok(false);
            }
        }

        if op == SolverOperation::Minimize {
            // If we are minimizing and the bounding boxes intersect then we should try
            // to intersect the curve and surface to see if we have a zero length
            // minimization.
            let mut box0 = patch.pseudo_box;
            box0.expand_absolute(tol);
            if !box0.are_disjoint(&span.pseudo_box, None) {
                let uv_guess = uv_domain.evaluate(0.5, 0.5);
                let t_guess = ivl.evaluate(0.5);
                // (SER: checked; LocalCurveIntersect is not overridden)
                let hit = view
                    .eval
                    .local_curve_intersect(k, &uv_domain, curve, &ivl, tol, uv_guess, t_guess)?;
                if let Some(h) = hit {
                    // for points within the surface's trim boundary
                    if super::p1::is_point_valid_with(
                        k,
                        surface_tree.owner(),
                        cache,
                        h.uv,
                        None,
                        self.face_queries,
                    )? {
                        let mut solution = Solution::new();
                        solution.num_variables = 3;
                        *solution.start.param_mut(0)? = h.t;
                        *solution.start.param_mut(1)? = h.uv.x;
                        *solution.start.param_mut(2)? = h.uv.y;
                        solution.start.solution_value = h.deviation;
                        solution.solution_type = SolutionType::SingleValue;
                        solution.num_objects = 2;
                        solution.objects[0] = curve_handle;
                        solution.objects[1] = Some(surface_handle);
                        set_nodes(&mut solution, branch);
                        self.add_sorted_solution(&solution, SortKey::ByValueWithParameter, true)?;
                    }
                }
            }

            if self.base.best_answer_so_far_sq < BIG_DOUBLE / 10.0
                && best_answer_disjoint(
                    &patch.pseudo_box,
                    &span.pseudo_box,
                    self.base.best_answer_so_far_sq,
                    tol,
                )
            {
                return Ok(false);
            }
        }

        // The following loop will do all of the extra work required if there are
        // discontinuities at the ends of the curves or boundaries of the patches.
        let mut skip_normalize = false;
        if (op == SolverOperation::Minimize || op == SolverOperation::Maximize)
            && cs_are_perpen_disjoint_in(
                k,
                curve_tree,
                &curve_node,
                &surface_node,
                cache,
                surface_id,
            )?
        {
            skip_normalize = true;
        }

        let nodes = ScLocalSolveNodes {
            eval_surface: view.eval,
            patch_surface: view.patch_surface,
            surface_box: surface_node.bbox,
            curve_box: Some(curve_node.bbox),
        };

        // (SER: checked)
        let solutions: SolutionArray<O> = sc_local_solve(
            k,
            &nodes,
            &uv_domain,
            u_conts,
            v_conts,
            curve,
            &ivl,
            t_conts,
            tol,
            &self.base.opt_vectors,
            op,
            skip_normalize,
            cache.process_boundary_curves,
        )?;

        for solution in solutions.as_slice() {
            let mut solution = *solution;
            solution.num_variables = 3;
            solution.num_objects = 2;
            solution.objects[0] = curve_handle;
            solution.objects[1] = Some(surface_handle);
            set_nodes(&mut solution, branch);

            // for points within the surface's trim boundary
            let uv_found = Point2::new(solution.start.param(1)?, solution.start.param(2)?);
            if super::p1::is_point_valid_with(
                k,
                surface_tree.owner(),
                cache,
                uv_found,
                None,
                self.face_queries,
            )? {
                let key = if op == SolverOperation::Minimize
                    || op == SolverOperation::Maximize
                    || op == SolverOperation::Signed3dDirectedMinimize
                {
                    SortKey::ByValueWithParameter
                } else {
                    SortKey::ByFirstParameter
                };
                self.add_sorted_solution(&solution, key, true)?;
            }
        }

        Ok(false)
    }
}

impl<'t, O: SolutionObject + 't> GlobalSolve<'t, O> for CsGlobalSolver<'t, '_, O> {
    fn solver(&self) -> &GlobalSolver<'t, O> {
        &self.base
    }

    fn solver_mut(&mut self) -> &mut GlobalSolver<'t, O> {
        &mut self.base
    }

    fn branch_may_contain_answers(&mut self, branch: &Branch) -> Result<bool> {
        // (surface::surface::p1)
        CsGlobalSolver::branch_may_contain_answers(self, branch)
    }

    fn local_solve(&mut self, branch: &Branch) -> Result<bool> {
        self.local_solve_branch(branch)
    }

    fn identical_solutions(
        &self,
        sol1: &Solution<O>,
        sol2: &Solution<O>,
    ) -> Result<(SolutionPairType, Option<PreferredSolution>)> {
        // (surface::surface::p5)
        // the solutions name the solve's curve and surface (`&crCurve`, `this`)
        if let Some(operands) = &self.operands {
            cs_identical_solutions_in(self.kernel, &self.base, self.surface, sol1, sol2, operands)
        } else {
            let domains = super::p1::SolveOperands {
                operands: [
                    super::p5::curve_operand(&self.base, 0, self.curve),
                    super::p5::surface_operand(&self.base, 1, self.surface_cache, self.surface),
                ],
            };
            cs_identical_solutions_in(self.kernel, &self.base, self.surface, sol1, sol2, &domains)
        }
    }
}

/// One boundary iso curve solve of `SmSSGlobalSolver::LocalSolve`: which surface's
/// patch side (`own`), in which direction.
#[derive(Debug, Clone, Copy)]
enum Side4 {
    /// Surface 1, constant `u`.
    U1,
    /// Surface 1, constant `v`.
    V1,
    /// Surface 2, constant `u`.
    U2,
    /// Surface 2, constant `v`.
    V2,
}

impl<'t, O: SolutionObject + 't> SsGlobalSolver<'t, '_, O> {
    /// The boundary iso curve solves of `SmSSGlobalSolver::LocalSolve` (minimize and
    /// maximize with boundary processing): the patch sides of one surface with
    /// position discontinuities against the other patch (`sm_SCLocalSolve`), the
    /// solutions remapped to `[u1, v1, u2, v2]`. Upstream's `continue` for an interior
    /// side at `j == 1` skips the rest of that pass.
    #[allow(clippy::too_many_arguments)]
    fn boundary_solves(
        &mut self,
        branch: &Branch,
        views: [&PatchView<'_, O>; 2],
        boxes: [Extent3d; 2],
        domains: [Extent2d; 2],
        conts: [[ContinuityType; 2]; 4],
        objects: [Option<O>; 2],
        process_boundary_curves: bool,
    ) -> Result<()> {
        let k = self.kernel;
        let op = self.base.operation;
        let tol = self.base.tolerance_3d;
        let c0 = ct(ContinuityType::C0);
        let [u_conts, v_conts, u_conts2, v_conts2] = conts;
        let [dom1, dom2] = domains;
        // To prevent from solving same low level solutions twice
        let tmp_conts = [ContinuityType::C1C2; 2];
        let vectors = self.base.opt_vectors.clone();
        let [view1, view2] = views;
        let [box1, box2] = boxes;

        'pass: for j in 0..2_usize {
            // Only do this discontinuity checking if performing min/max
            if op != SolverOperation::Minimize && op != SolverOperation::Maximize {
                continue;
            }
            if !process_boundary_curves {
                continue;
            }
            let from_left = j == 0;
            for side in [Side4::U1, Side4::V1, Side4::U2, Side4::V2] {
                let side_conts = match side {
                    Side4::U1 => u_conts,
                    Side4::V1 => v_conts,
                    Side4::U2 => u_conts2,
                    Side4::V2 => v_conts2,
                };
                let c = side_conts
                    .get(j)
                    .copied()
                    .ok_or(SolidError::sm_err(OP_SS))?;
                if ct(c) > c0 {
                    continue;
                }
                // Only solve for internal curves once
                if j == 1 && c != ContinuityType::Discontinuous {
                    continue 'pass;
                }
                // the iso curve's surface and domain, and the patch it is solved against
                let (own_view, own_dom, other_view, other_dom, other_box) = match side {
                    Side4::U1 | Side4::V1 => (view1, dom1, view2, dom2, box2),
                    Side4::U2 | Side4::V2 => (view2, dom2, view1, dom1, box1),
                };
                let (dir, value, ic_ivl) = match side {
                    Side4::U1 | Side4::U2 => (
                        SurfParamType::U,
                        if from_left {
                            own_dom.min().x
                        } else {
                            own_dom.max().x
                        },
                        Extent1d::from_min_max(own_dom.min().y, own_dom.max().y),
                    ),
                    Side4::V1 | Side4::V2 => (
                        SurfParamType::V,
                        if from_left {
                            own_dom.min().y
                        } else {
                            own_dom.max().y
                        },
                        Extent1d::from_min_max(own_dom.min().x, own_dom.max().x),
                    ),
                };
                // SmIsoCurve(*pSrfEval, dir, value, bFlag) over the evaluated surface
                let ic = boundary_iso_curve(k, own_view.eval, dir, value, from_left)?;
                // (IsDegenerate's default tolerance SM_EFF_ZERO; the iso curve's kind)
                if ic.is_degenerate(k, EFF_ZERO, None)? {
                    continue;
                }
                // the patch continuities and the curve's (the other direction's)
                let (pu, pv, t_conts) = match side {
                    Side4::U1 => (u_conts2, v_conts2, v_conts),
                    Side4::V1 => (u_conts2, v_conts2, u_conts),
                    Side4::U2 => (tmp_conts, tmp_conts, v_conts2),
                    Side4::V2 => (tmp_conts, tmp_conts, u_conts2),
                };
                let nodes = ScLocalSolveNodes {
                    eval_surface: other_view.eval,
                    patch_surface: other_view.patch_surface,
                    surface_box: other_box,
                    curve_box: None,
                };
                // (SER: checked)
                let sols: SolutionArray<O> = sc_local_solve(
                    self.kernel,
                    &nodes,
                    &other_dom,
                    pu,
                    pv,
                    &ic,
                    &ic_ivl,
                    t_conts,
                    tol,
                    &vectors,
                    op,
                    false,
                    process_boundary_curves,
                )?;
                for sol in sols.as_slice() {
                    let mut r = *sol;
                    r.num_variables = 4;
                    set_nodes(&mut r, branch);
                    r.num_objects = 2;
                    r.objects[0] = objects[0];
                    r.objects[1] = objects[1];
                    // the solution [t, u, v] of the iso curve and the other patch
                    let s0 = r.start.param(0)?;
                    let s1 = r.start.param(1)?;
                    let s2 = r.start.param(2)?;
                    let new = match side {
                        Side4::U1 => [value, s0, s1, s2],
                        // (upstream leaves m_vStart[0], the curve parameter: u1)
                        Side4::V1 => [s0, value, s1, s2],
                        Side4::U2 => [s1, s2, value, s0],
                        Side4::V2 => [s1, s2, s0, value],
                    };
                    for (i, v) in new.into_iter().enumerate() {
                        *r.start.param_mut(i)? = v;
                    }
                    r.solution_type = SolutionType::SingleValue;
                    self.add_sorted_solution(&r, SortKey::ByValueWithParameter, true)?;
                }
            }
        }
        Ok(())
    }

    /// `SmSSGlobalSolver::LocalSolve`: solves the Bezier patches of `branch[0]` and
    /// `branch[1]` (culling by node class, domains, boxes and the patches' normal
    /// cones; for intersections only the node pair is recorded; for minimizes a local
    /// surface/surface intersection first and the boundary iso curves with position
    /// discontinuities), then the local surface/surface solve from the patch middles,
    /// adding the solutions `[u1, v1, u2, v2]` inside both faces to `base.solutions`.
    /// Returns the request for more subdivision (always false).
    ///
    /// Dispatch: the evaluations run on the caches' surfaces (`sm_GetOriginalSurface`
    /// / `sm_GetSurfaceToEvaluate`, each its own kind).
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` without both `caches`; `InvalidInput` when a node is not a
    /// patch of its cache or for a signed directed solve without a vector; those of
    /// the checked (`SER`) calls: `ComputeConeBoundTo`, `LocalSurfaceIntersect`,
    /// `sm_SCLocalSolve`, `LocalSurfaceSolve` and `sm_PassSSConvergenceTest`.
    #[allow(clippy::too_many_lines)]
    pub fn local_solve_branch(&mut self, branch: &Branch) -> Result<bool> {
        let k = self.kernel;
        let node1 = *self.base.branch_node(branch, 0)?;
        let node2 = *self.base.branch_node(branch, 1)?;
        let id1 = branch
            .first()
            .copied()
            .flatten()
            .ok_or(SolidError::null_pointer(OP_SS))?;
        let id2 = branch
            .get(1)
            .copied()
            .flatten()
            .ok_or(SolidError::null_pointer(OP_SS))?;
        let [Some(cache1), Some(cache2)] = self.caches else {
            return Err(SolidError::null_pointer(OP_SS));
        };
        let tree1 = self.base.tree(0)?;
        let tree2 = self.base.tree(1)?;
        let view1 = patch_view(OP_SS, tree1, cache1, id1)?;
        let view2 = patch_view(OP_SS, tree2, cache2, id2)?;

        // If the classification of the node is outside then we can quit here
        if !is_node_valid(tree1.owner(), cache1, &node1)? {
            return Ok(false);
        }
        if !is_node_valid(tree2.owner(), cache2, &node2)? {
            return Ok(false);
        }

        let objects = [tree1.owner_object()?, tree2.owner_object()?];
        let op = self.base.operation;
        let tol = self.base.tolerance_3d;
        let patch1 = &view1.patch;
        let patch2 = &view2.patch;

        // Do interval tests and reject if disjoint
        if patch1.aux.uv_domain.are_disjoint(&self.uv_domain1, 0.0) {
            return Ok(false);
        }
        if patch2.aux.uv_domain.are_disjoint(&self.uv_domain2, 0.0) {
            return Ok(false);
        }

        if op == SolverOperation::Intersect {
            let mut box0 = patch1.pseudo_box;
            box0.expand_absolute(tol);
            if !box0.are_disjoint(&patch2.pseudo_box, None) {
                let mut solution = Solution::new();
                set_nodes(&mut solution, branch);
                solution.start.solution_value = 0.0;
                solution.solution_type = SolutionType::SingleValue;
                solution.num_objects = 2;
                solution.objects[0] = objects[0];
                solution.objects[1] = objects[1];
                solution.num_variables = 0;
                self.add_sorted_solution(&solution, SortKey::BySolutionValue, true)?;
            }
            return Ok(false);
        }

        let Some((dom1, u_conts, v_conts)) = limit_domain(
            patch1.aux.uv_domain,
            &self.uv_domain1,
            patch1.u_curve_conts,
            patch1.v_curve_conts,
        )?
        else {
            return Ok(false);
        };
        let Some((dom2, u_conts2, v_conts2)) = limit_domain(
            patch2.aux.uv_domain,
            &self.uv_domain2,
            patch2.u_curve_conts,
            patch2.v_curve_conts,
        )?
        else {
            return Ok(false);
        };

        if op == SolverOperation::Minimize {
            // If we are minimizing and the bounding boxes intersect then we should try
            // to intersect the curve and surface to see if we have a zero length
            // minimization.
            let mut box0 = patch1.pseudo_box;
            box0.expand_absolute(tol);
            if !box0.are_disjoint(&patch2.pseudo_box, None) {
                let uv_guess1 = dom1.evaluate(0.5, 0.5);
                let uv_guess2 = dom2.evaluate(0.5, 0.5);
                // (SER: checked; cpSurf1/cpSurf2 are the original surfaces, over their
                // natural domains, with boundary testing)
                let found = view1.eval.local_surface_intersect(
                    k,
                    &view1.eval.natural_uv_domain_with_nurb(k)?,
                    view2.eval,
                    &view2.eval.natural_uv_domain_with_nurb(k)?,
                    tol,
                    uv_guess1,
                    uv_guess2,
                    &LocalSurfaceIntersectOptions {
                        plane_normal: None,
                        previous_uv: None,
                        do_boundary_testing: true,
                    },
                )?;
                if let Some(x) = found {
                    if super::p1::is_point_valid_with(
                        k,
                        tree1.owner(),
                        cache1,
                        x.uv,
                        None,
                        self.face_queries,
                    )? && super::p1::is_point_valid_with(
                        k,
                        tree2.owner(),
                        cache2,
                        x.other_uv,
                        None,
                        self.face_queries,
                    )? {
                        let mut solution = Solution::new();
                        solution.num_variables = 4;
                        solution.solution_type = SolutionType::SingleValue;
                        solution.start.solution_value = x.deviation;
                        for (i, v) in [x.uv.x, x.uv.y, x.other_uv.x, x.other_uv.y]
                            .into_iter()
                            .enumerate()
                        {
                            *solution.start.param_mut(i)? = v;
                        }
                        solution.num_objects = 2;
                        solution.objects[0] = objects[0];
                        solution.objects[1] = objects[1];
                        set_nodes(&mut solution, branch);
                        self.add_sorted_solution(&solution, SortKey::ByValueWithParameter, true)?;
                    }
                }
            }

            if self.base.best_answer_so_far_sq < BIG_DOUBLE / 10.0
                && best_answer_disjoint(
                    &patch1.pseudo_box,
                    &patch2.pseudo_box,
                    self.base.best_answer_so_far_sq,
                    tol,
                )
            {
                return Ok(false);
            }
        }

        // The following loop will do all of the extra work required if there are
        // discontinuities at the ends of the curves or boundaries of the patches.
        // (GetProcessBoundaryCurves of the first node's cache)
        let process_boundary_curves = cache1.process_boundary_curves;
        self.boundary_solves(
            branch,
            [&view1, &view2],
            [node1.bbox, node2.bbox],
            [dom1, dom2],
            [u_conts, v_conts, u_conts2, v_conts2],
            objects,
            process_boundary_curves,
        )?;

        let polar1 = cache1.polar_box_unstored(k, id1)?;
        let polar2 = cache2.polar_box_unstored(k, id2)?;
        if op == SolverOperation::Signed3dDirectedMinimize {
            // Do a tight bounding box test in projected space to see if they are
            // disjoint
            let vector = first_vector(OP_SS, &self.base.opt_vectors)?;
            if project_disjoint(&patch1.pseudo_box, &patch2.pseudo_box, tol, vector) {
                return Ok(false); // No answer
            }
            // Check to see if gauss maps are disjoint.
            if polar1.are_disjoint(&polar2) {
                return Ok(false); // No answer
            }
        } else {
            // Minimize, normalize, maximize (SER: checked)
            let (cone_vec, cone_angle) = node1.bbox.cone_bound_to(&node2.bbox)?;
            if polar1.is_disjoint_with_cone(cone_vec, cone_angle) {
                return Ok(false);
            }
            if polar2.is_disjoint_with_cone(cone_vec, cone_angle) {
                return Ok(false);
            }
            if polar1.are_disjoint(&polar2) {
                return Ok(false); // No answer
            }
        }

        // one guess: the patch middles
        let uv_guess1 = dom1.evaluate(0.5, 0.5);
        let uv_guess2 = dom2.evaluate(0.5, 0.5);
        // (SER: checked; pSrfEval1->LocalSurfaceSolve: virtual, never overridden)
        let mut found = local_surface_solve(
            k,
            view1.eval,
            &dom1,
            view2.eval,
            &dom2,
            op,
            &self.base.opt_vectors,
            uv_guess1,
            uv_guess2,
        )?;
        // Have to check for normalized answers here
        if let Some((x, _)) = found {
            if op != SolverOperation::Signed3dDirectedMinimize {
                let (uv, uv2) = split(&x);
                // (SER: checked)
                if !pass_ss_convergence_test(k, view1.eval, uv, view2.eval, uv2)? {
                    found = None;
                }
            }
        }
        if let Some((x, value)) = found {
            let mut solution = Solution::new();
            // (the solution LocalSurfaceSolve writes: single valued, four variables)
            solution.solution_type = SolutionType::SingleValue;
            solution.start.solution_value = value;
            for (i, v) in x.into_iter().enumerate() {
                *solution.start.param_mut(i)? = v;
            }
            solution.num_variables = 4;
            set_nodes(&mut solution, branch);
            solution.num_objects = 2;
            solution.objects[0] = objects[0];
            solution.objects[1] = objects[1];
            let (uv1, uv2) = split(&x);
            if super::p1::is_point_valid_with(
                k,
                tree1.owner(),
                cache1,
                uv1,
                None,
                self.face_queries,
            )? && super::p1::is_point_valid_with(
                k,
                tree2.owner(),
                cache2,
                uv2,
                None,
                self.face_queries,
            )? {
                let key = if op == SolverOperation::Minimize || op == SolverOperation::Maximize {
                    SortKey::ByValueWithParameter
                } else {
                    SortKey::ByFirstTwoParameters
                };
                self.add_sorted_solution(&solution, key, true)?;
            }
        }

        Ok(false)
    }
}

// ---- IntersectSurfaceBoundaries --------------------------------------------------------------

/// The boundary curves of the other surface lying on this one, found by
/// [`Surface::intersect_surface_boundaries`] (upstream's output arrays; the UV
/// curves are always `NULL` and the types always `SM_TC_TANGENT`, so they are not
/// returned).
#[derive(Debug, Clone, Default)]
pub struct BoundaryIntersections {
    /// `pOpt3DCurves`: the trimmed copies of the other surface's boundary curves.
    pub curves_3d: Vec<Curve>,
    /// `pOptDeviations`: the drop distance of each curve's middle point.
    pub deviations: Vec<f64>,
}

/// The handles of the boundary curves in their curve/curve intersection.
#[derive(Debug, Clone, Copy, PartialEq)]
struct BoundaryCurveHandle(u8, Extent1d);

impl SolutionObject for BoundaryCurveHandle {
    fn kind(&self) -> ObjectKind {
        ObjectKind::Curve
    }

    fn natural_interval(&self) -> Option<Extent1d> {
        Some(self.1)
    }
}

/// `CreateIsoBoundaries` in `U` and `V`: the boundary curves `[u min, v min, u max,
/// v max]` (`pCrv0, pCrv1, pCrv2, pCrv3`) and the list of those used, skipping the
/// maximum boundary in a closed direction (`IsClosed` over `uv_domain`, default
/// tolerance).
fn boundary_curves(
    k: &Kernel,
    surface: &Surface,
    uv_domain: &Extent2d,
    tol: f64,
) -> Result<Vec<Curve>> {
    // (SER: checked)
    let u = surface.create_iso_boundaries(k, SurfParamType::U, tol)?;
    let v = surface.create_iso_boundaries(k, SurfParamType::V, tol)?;
    let (Some((c0, c2)), Some((c1, c3))) = (u, v) else {
        return Err(SolidError::sm_err(OP_ISB));
    };
    let mut out = vec![c0, c1];
    // (IsClosed of the surface's own kind, default tolerance, no continuity)
    if !surface
        .is_closed(k, uv_domain, SurfParamType::U, None, false)?
        .0
    {
        out.push(c2);
    }
    if !surface
        .is_closed(k, uv_domain, SurfParamType::V, None, false)?
        .0
    {
        out.push(c3);
    }
    Ok(out)
}

impl Surface {
    /// The boundary curves of `other` (over `other_uv_domain`) that lie on this
    /// surface over `uv_domain` (`IntersectSurfaceBoundaries`, for coincident
    /// surfaces): each boundary curve of `other` is split at its intersections with
    /// this surface's boundary curves, and every piece whose middle point drops into
    /// `uv_domain` (and is longer than `approx_tol_3d`) is kept as a trimmed copy with
    /// that drop distance.
    ///
    /// Dispatch: `CreateIsoBoundaries`, `IsClosed` and `DropPoint` of each surface's
    /// kind; the boundary curves keep the kind of the surface's iso curves (lines,
    /// circles or B-splines).
    ///
    /// # Errors
    ///
    /// Those of the checked (`SER`) calls `CreateIsoBoundaries` and `IsClosed`
    /// (upstream ignores the statuses of `GlobalCurveIntersect`, `EvaluatePoint`,
    /// `DropPoint`, `Copy` and `Trim`); [`SolidError::Unported`] always propagates.
    pub fn intersect_surface_boundaries(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        other: &Surface,
        other_uv_domain: &Extent2d,
        approx_tol_3d: f64,
    ) -> Result<BoundaryIntersections> {
        let mut out = BoundaryIntersections::default();
        let this_bdry = boundary_curves(k, self, uv_domain, approx_tol_3d)?;
        let that_bdry = boundary_curves(k, other, other_uv_domain, approx_tol_3d)?;

        // sCrvPt keeps its last value when an evaluation fails
        let mut crv_pt = Point3::UNDEF;

        // For every other-surface boundary, intersect it with all of our boundaries
        // and classify each segment as In or Out. Save the Ins.
        for other_crv in &that_bdry {
            let domain0 = other_crv.natural_interval();
            let mut params = vec![domain0.min(), domain0.max()];

            for this_crv in &this_bdry {
                let domain1 = this_crv.natural_interval();
                let opts = CurveIntersectOptions {
                    skip_global_coincidence_check: true,
                    handles: [
                        Some(BoundaryCurveHandle(0, domain0)),
                        Some(BoundaryCurveHandle(1, domain1)),
                    ],
                    analytic_handles: [
                        Some(BoundaryCurveHandle(2, domain0)),
                        Some(BoundaryCurveHandle(3, domain1)),
                    ],
                };
                // (SE: status ignored; a failed intersection adds no parameters)
                let sols = ignored(other_crv.global_curve_intersect(
                    k,
                    &domain0,
                    this_crv,
                    &domain1,
                    approx_tol_3d,
                    &opts,
                ))?;
                for sol in sols.iter().flat_map(SolutionArray::as_slice) {
                    params.push(sol.start.param(0)?);
                    if sol.solution_type == SolutionType::RangeOfValues {
                        params.push(sol.end.param(0)?);
                    }
                }
            }

            // Classify each segment as in or out of the this surface.
            // (ShellSort with smgu_DoubleLessThan: ascending)
            params.sort_by(f64::total_cmp);

            for pair in params.windows(2) {
                let [p0, p1] = pair else {
                    continue;
                };
                let mut ivl = Extent1d::from_min_max(*p0, *p1);
                if ivl.length() < domain0.length() * EFF_ZERO {
                    continue;
                }

                let crv_t = ivl.mid();
                // (SE: status ignored)
                if let Some(p) = ignored(other_crv.evaluate_point(k, crv_t))? {
                    crv_pt = p;
                }

                // (SE: status ignored: a failed drop is no success; DropPoint is not
                // virtual, its solves run with the caller's kernel)
                let drop = ignored(self.drop_point::<BoundaryCurveHandle>(
                    k,
                    crv_pt,
                    uv_domain,
                    &DropPointOptions {
                        guess_uv: None,
                        operation: SolverOperation::Intersect,
                        ..DropPointOptions::default()
                    },
                ))?;
                let Some(drop) = drop else {
                    continue;
                };

                if drop.success && uv_domain.contains_point(drop.result_uv, 0.0) {
                    // (Copy's status: a failure skips the piece)
                    let Some(mut copy) = ignored(other_crv.copy(k))? else {
                        continue;
                    };

                    if copy.approximate_length(k, &ivl, 4, false)?.0 < approx_tol_3d {
                        continue;
                    }

                    // may snap sIvl by tol to existing knots (SE: status ignored)
                    ignored(copy.trim(k, &mut ivl, TrimMode::Notify))?;

                    out.curves_3d.push(copy);
                    out.deviations.push(drop.gap);
                }
            }
        }

        Ok(out)
    }
}

// ---- LiftCurve -------------------------------------------------------------------------------

/// `sm_ExtractIsoCurve(ctx, surface, domain, uv_curve, interval, tol, ..)`: the 3D
/// curve and its deviation when the UV curve is an iso parameter line; `None`
/// otherwise.
fn extract_iso_curve(
    k: &Kernel,
    surface: &Surface,
    uv_domain: &Extent2d,
    uv_curve: &Curve,
    interval: &Extent1d,
    approx_tol_3d: f64,
) -> Result<Option<(Curve, f64)>> {
    // `sm_ExtractIsoCurve` takes an `SmBSplineCurve`: every `Curve` kind derived from it
    // has one as base. Other kinds cannot reach upstream's signature, so they are not
    // iso curves here.
    let uv_bspline = match uv_curve.kind() {
        CurveKind::BSpline(b) => b,
        CurveKind::Line(l) => l.base(),
        CurveKind::Ellipse(e) => e.base(),
        CurveKind::Hyperbola(h) => h.base(),
        CurveKind::Parabola(p) => p.base(),
        _ => return Ok(None),
    };
    // (sm_ExtractIsoCurve, surface.surface_p8; rdMaxApproxDev is always 0.0 there)
    let Some(iso) =
        super::p8::extract_iso_curve(k, surface, uv_domain, uv_bspline, interval, approx_tol_3d)?
    else {
        return Ok(None);
    };
    // rp3dCurve is an SmBSplineCurve: the iso curve keeps its own kind (an SmLine or
    // conic is one; the caller's virtual calls dispatch on it)
    match iso.kind() {
        CurveKind::BSpline(_)
        | CurveKind::Line(_)
        | CurveKind::Ellipse(_)
        | CurveKind::Hyperbola(_)
        | CurveKind::Parabola(_) => Ok(Some((iso, 0.0))),
        _ => Err(SolidError::sm_err(OP_LIFT)),
    }
}

/// The lifted curve of [`Surface::lift_curve`].
#[derive(Debug, Clone)]
pub struct LiftedCurve {
    /// `rp3dCurve`: the 3D curve `surface(uv_curve(interval))`, an `SmBSplineCurve`:
    /// a B-spline, or the analytic iso curve (line or conic) `sm_ExtractIsoCurve` made.
    pub curve: Curve,
    /// `rdMaxApproxDev`: the largest distance from the curve to the surface.
    pub max_approx_dev: f64,
}

impl Surface {
    /// The 3D curve of the UV curve `uv_curve` over `interval` on this surface over
    /// `uv_domain`, approximated to `approx_tol_3d` (`LiftCurve`): an iso parameter
    /// curve when the UV curve is an iso line, else the curve on surface approximated
    /// per segment between the surface's C0 knot lines (unless
    /// `skip_continuity_checks`) and joined into one curve.
    ///
    /// Dispatch: `GetKnots` of the surface's own kind; the curve on surface preserves
    /// that concrete surface dispatch.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when no segment could be lifted or a segment's continuous domain is
    /// empty; `SM_ERR_NULL_POINTER` without a surface cache; those of the checked
    /// (`SER`) calls.
    #[allow(clippy::too_many_lines)]
    pub fn lift_curve(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        uv_curve: &Curve,
        interval: &Extent1d,
        approx_tol_3d: f64,
        skip_continuity_checks: bool,
    ) -> Result<LiftedCurve> {
        self.lift_curve_in::<crate::surface::surface::p4::NoObject>(
            k,
            uv_domain,
            uv_curve,
            interval,
            approx_tol_3d,
            skip_continuity_checks,
            None,
            None,
        )
    }

    /// [`Surface::lift_curve`] through the caller's face-aware cache context:
    /// `handle` names this surface in the cache, so a surface owned by a face can
    /// reach that face when the continuity checks build its cache.
    ///
    /// # Errors
    ///
    /// Those of [`Surface::lift_curve`] and of the context's cache preparation.
    #[allow(clippy::too_many_arguments)]
    pub fn lift_curve_with_context<O: Copy + PartialEq>(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        uv_curve: &Curve,
        interval: &Extent1d,
        approx_tol_3d: f64,
        skip_continuity_checks: bool,
        context: &mut dyn SurfaceSolveContext<O>,
        handle: Option<O>,
    ) -> Result<LiftedCurve> {
        self.lift_curve_in(
            k,
            uv_domain,
            uv_curve,
            interval,
            approx_tol_3d,
            skip_continuity_checks,
            Some(context),
            handle,
        )
    }

    #[allow(clippy::too_many_lines, clippy::too_many_arguments)]
    fn lift_curve_in<O: Copy + PartialEq>(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        uv_curve: &Curve,
        interval: &Extent1d,
        approx_tol_3d: f64,
        skip_continuity_checks: bool,
        mut context: Option<&mut dyn SurfaceSolveContext<O>>,
        handle: Option<O>,
    ) -> Result<LiftedCurve> {
        let mut max_approx_dev = 0.0;

        // Test to see if the UV curve corresponds to an ISO curve
        if let Some((curve, dev)) =
            extract_iso_curve(k, self, uv_domain, uv_curve, interval, approx_tol_3d)?
        {
            return Ok(LiftedCurve {
                curve,
                max_approx_dev: dev,
            });
        }

        // sSegments: parameter values where the projected 3D curve is only C0
        let mut segments = vec![interval.min(), interval.max()];
        let mut break_params: Vec<f64> = Vec::new();

        // when asked to do continuity checks load the break parameters with all param
        // values where the lifted curve is only C0 continuous
        if skip_continuity_checks {
            // load sBreakParameters with lift curve begin/end points
            break_params.extend_from_slice(&segments);
        } else {
            // Right now step surfaces will not be created that have discontinuities.
            if !matches!(self.kind(), SurfaceKind::Step(_)) {
                // NER: no surface cache (no GetTree: the continuity arrays are empty
                // unless the cache's tree was built)
                let cache = continuity_surface_cache(
                    k,
                    self,
                    handle,
                    context
                        .as_mut()
                        .map(|c| &mut **c as &mut dyn SurfaceSolveContext<O>),
                );
                let cache = match cache {
                    Ok(c) => c,
                    Err(e) if e.is_unported() => return Err(e),
                    Err(_) => return Err(SolidError::null_pointer(OP_LIFT)),
                };

                // the solutions of sm_IntersectAndSplit name crUVCurveToLift (the
                // curve tree's owner), whose natural interval IdenticalSolutions reads
                let lift_handle = BoundaryCurveHandle(0, uv_curve.natural_interval());

                // for every U knot (SER: checked)
                let (knots, _) = self.knots_with_nurb(k, SurfParamType::U, None)?;
                let u_conts = cache.u_continuities();
                let size = u_conts.len().saturating_sub(1);
                for iu in 1..size {
                    // when surface U knot is C0 or discontinuous
                    if ct(*u_conts.get(iu).ok_or(SolidError::sm_err(OP_LIFT))?)
                        < ct(ContinuityType::C1)
                    {
                        // split the UVcurve at the Surface discontinuity (SER: checked)
                        let knot = *knots.get(iu).ok_or(SolidError::sm_err(OP_LIFT))?;
                        intersect_and_split::<BoundaryCurveHandle>(
                            k,
                            uv_curve,
                            interval,
                            Point3::new(knot, 0.0, 0.0),
                            Vec3::new(1.0, 0.0, 0.0),
                            Some(lift_handle),
                            &mut segments,
                        )?;
                    }
                }

                // for every V knot (SER: checked)
                let (knots, _) = self.knots_with_nurb(k, SurfParamType::V, None)?;
                let v_conts = cache.v_continuities();
                let size = v_conts.len().saturating_sub(1);
                for iv in 1..size {
                    // when surface V knot is C0 or discontinuous
                    if ct(*v_conts.get(iv).ok_or(SolidError::sm_err(OP_LIFT))?)
                        < ct(ContinuityType::C1)
                    {
                        // (SER: checked)
                        let knot = *knots.get(iv).ok_or(SolidError::sm_err(OP_LIFT))?;
                        intersect_and_split::<BoundaryCurveHandle>(
                            k,
                            uv_curve,
                            interval,
                            Point3::new(0.0, knot, 0.0),
                            Vec3::new(0.0, 1.0, 0.0),
                            Some(lift_handle),
                            &mut segments,
                        )?;
                    }
                }
            }

            // get UVCurve knots and associated continuities (SER: checked; default
            // angle SM_CONTINUITY_ANGLE)
            let (knots, _) = uv_curve.knots(k, None)?;
            let (_, curve_conts) = uv_curve.calculate_continuities(k, CONTINUITY_ANGLE)?;

            // for every curve knot: the projected 3D curve's C0 break points
            for (j, c) in curve_conts.iter().enumerate() {
                if ct(*c) < ct(ContinuityType::C1) {
                    break_params.push(*knots.get(j).ok_or(SolidError::sm_err(OP_LIFT))?);
                }
            }
        }

        let surface = self.clone_for_evaluation()?;

        let mut curves_3d: Vec<Curve> = Vec::new();

        // For each segment in list trace the curve
        for pair in segments.windows(2) {
            let [s0, s1] = pair else {
                continue;
            };
            // get this segment's interval
            let ivl = Extent1d::from_min_max(*s0, *s1);

            // Remove any break parameters which are outside of domain (SER: checked)
            let mut ivl_breaks = break_params.clone();
            set_up_breaks(&mut ivl_breaks, &ivl)?;

            // skip very small segments
            if ivl_breaks.len() < 2 {
                continue;
            }

            // the UV curve point and (non-zero) tangent just inside the segment
            // (SER: checked)
            let pv = uv_curve.evaluate(k, ivl.min() + EFF_ZERO_SQRT, 1, true, true)?;
            let (Some(p0), Some(p1)) = (pv.first(), pv.get(1)) else {
                return Err(SolidError::sm_err(OP_LIFT));
            };
            let pnt = Point2::new(p0.x, p0.y);
            let vec = Vec2::new(p1.x, p1.y);

            // get surface domain enclosing pnt that is at least SM_CT_C1 continuous
            let seg_domain = match context.as_mut() {
                Some(ctx) => self.domain_with_continuity_with_context(
                    k,
                    ContinuityType::C1,
                    pnt,
                    vec,
                    &mut **ctx,
                    handle,
                )?,
                None => self.domain_with_continuity(k, ContinuityType::C1, pnt, vec)?,
            };

            // If we don't get some sort of a domain then error out.
            if seg_domain.size().length_squared() < EFF_ZERO_SQ {
                return Err(SolidError::sm_err(OP_LIFT));
            }

            // define 3DCurve = SmCrvOnSurf(segment, thisSurface)
            let mut trace = CrvOnSurf::new(
                k,
                uv_curve.clone_for_evaluation()?,
                surface.clone_for_evaluation()?,
                Some(seg_domain),
                0,
            )?;

            // Check for degenerate curve segment (SER: checked)
            let pnt1 = trace.evaluate_point(k, ivl.min())?;
            let mut degenerate = true;

            // check 10 sample points until 1 is found that is more than tol dist from
            // begin point
            for ii in 1..=10_u32 {
                // (SER: checked)
                let pnt2 = trace.evaluate_point(k, ivl.evaluate(f64::from(ii) / 10.0))?;
                if pnt2.distance(pnt1) > approx_tol_3d / 100.0 {
                    degenerate = false;
                    break;
                }
            }

            // skip degenerate curves
            if degenerate {
                continue;
            }

            // approximate the trace with a 3D BSplineCurve to approx_tol_3d (SER:
            // checked; bOptMatchParameterization FALSE)
            let (curve, max_dist) =
                trace.approximate_curve(k, &ivl_breaks, approx_tol_3d, false)?;
            max_approx_dev = max2(max_approx_dev, max_dist);

            // save lifted curve approximations
            curves_3d.push(Curve::from(curve));
        }

        // set output based on the number of lifted segments
        let curve = match curves_3d.len() {
            // signal error
            0 => return Err(SolidError::sm_err(OP_LIFT)),
            // output = 1 lifted BSpline approximation curve
            1 => match curves_3d.pop().map(Curve::into_kind) {
                Some(CurveKind::BSpline(b)) => b,
                _ => return Err(SolidError::sm_err(OP_LIFT)),
            },
            // output = 1 composite curve (IsClosed default tolerance 0.0)
            _ => {
                let closed = uv_curve.is_closed(k, interval, 0.0)?;
                let mut composite = CompositeCurve::with_curves(3, curves_3d, closed, None, None)?;
                // (SER: checked)
                composite.make_composite_nurb()?
            }
        };

        Ok(LiftedCurve {
            curve: Curve::from(curve),
            max_approx_dev,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plane(origin: Point3, normal: Vec3) -> Surface {
        Surface::from(Plane::from_normal(origin, normal))
    }

    #[test]
    fn two_perpendicular_planes_meet_on_their_line() {
        let k = &Kernel::default();
        let a = plane(Point3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 1.0));
        let b = plane(Point3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0));
        let dom = Extent2d::from_points(Point2::new(-5.0, -5.0), Point2::new(5.0, 5.0));
        let r = a
            .local_surface_intersect(
                k,
                &dom,
                &b,
                &dom,
                1e-6,
                Point2::new(0.3, 0.2),
                Point2::new(0.1, 0.4),
                &LocalSurfaceIntersectOptions::default(),
            )
            .unwrap()
            .unwrap();
        let p = a.evaluate_point(k, r.uv).unwrap();
        let q = b.evaluate_point(k, r.other_uv).unwrap();
        assert!(p.distance(q) < 1e-6);
        assert!(p.x.abs() < 1e-9 && p.z.abs() < 1e-9);
    }

    #[test]
    fn parallel_guess_normals_give_no_answer() {
        let k = &Kernel::default();
        let a = plane(Point3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 1.0));
        let b = plane(Point3::new(0.0, 0.0, 1.0), Vec3::new(0.0, 0.0, 1.0));
        let dom = Extent2d::from_points(Point2::new(-5.0, -5.0), Point2::new(5.0, 5.0));
        let r = a
            .local_surface_intersect(
                k,
                &dom,
                &b,
                &dom,
                1e-6,
                Point2::new(0.3, 0.2),
                Point2::new(0.1, 0.4),
                &LocalSurfaceIntersectOptions::default(),
            )
            .unwrap();
        assert!(r.is_none());
    }
}
