//! Fillet solver part 3: stepping along the fillet (`StepSolve`), the fillet point on
//! a cross-section plane through a guess point, splitting and trimming a fillet surface
//! by its rails' classifications, refining a rail classification near tiny intervals,
//! the rail/side-edge intersection of a rollover, and the full (cliff) rollover.
//!
//! Upstream: `source/SMLib/src/SmFilletSolver.cpp`.
//!
//! | Rust                                                 | Upstream                                       |
//! |------------------------------------------------------|------------------------------------------------|
//! | [`FilletSolver::other_fillet_corner`]                | `SmFilletSolver::GetOtherFilletCorner`         |
//! | [`find_fillet_tsect_pnt`]                            | `sm_FindFilletTsectPnt` (static)               |
//! | [`FilletSolver::split_and_trim_fillet_surface`], [`FilletSolver::split_and_trim_fillet_surface_with`] | `SmFilletSolver::SplitAndTrimFilletSurface`    |
//! | [`FilletSolver::step_solve`], [`StepSolveOutcome`]   | `SmFilletSolver::StepSolve`                    |
//! | [`refine_rail_curve_classification`]                 | `sm_RefineRailCurveClassification`             |
//! | [`calc_rail_x_edge`], [`calc_rail_x_edge_with_ops`] | `sm_CalcRailXEdge` (static)                    |
//! | [`FilletSolver::process_full_roll_over`]             | `SmFilletSolver::ProcessFullRollOver`          |
//! | `impl EvalFunction for FindClippedRadiusEfo`         | `SmFindClippedRadiusEFO` as `SmEvalFunctionObject` |
//! | `impl EvalNFunctions for FilletSphereSolveEnfo`      | `SmFilletSphereSolveENFO` as `SmEvalNFunctionsObject` |
//!
//! Deviations:
//! - **Solver dispatch.** Methods that call virtuals take the solver as
//!   `&mut dyn FilletSolverOps` (as [`FilletSolver::compute_surface_values`]).
//!   `StepSolve` branches on [`FilletSolverOps::solver_type`] and reads a
//!   constant-distance solver's distance through [`FilletSolverOps::constant_distance`]
//!   (both added to the trait for this unit). The variable-radius branch casts to
//!   `SmVariableRadiusFS` (unit `blend.fillet_standard_solver_p1`) and is a stub.
//! - **Offset surface copies.** As in `blend.fillet_solver_p1`, the iso curve and its
//!   `LocalCurveIntersect` run on copies of the solver's offset surfaces;
//!   `LoadInitialValues` receives a copy of the first one (upstream passes
//!   `*GetSurface(0)` as a `const` reference). Upstream evaluates the solver's own
//!   `SmOffsetSurface`s, whose `const` evaluators store the base extension they make
//!   (`m_pExtendedSurface`), so after each use the solver's offsets take the copies'
//!   extensions back (`FilletSolver::keep_extension_of_copy`; a copy starts from the
//!   solver's extension and nothing else evaluates the solver's surface meanwhile, so
//!   the history matches upstream's one object). `EvaluateNormal` runs on the solver's
//!   own surface. Upstream's `SmIsoCurve` holds a pointer to the solver's surface, so
//!   after `SetSurfacesToZeroOffset` it evaluates at zero offset: the copy is rebuilt
//!   from the zeroed surface there.
//! - **Out-params.** `StepSolve`'s three flags are a [`StepSolveOutcome`] the caller
//!   passes in, written where upstream writes them, so on an error they keep what was
//!   set before it (upstream's `SmFilletIntersector` reads `bClipped` / `bBoundaryHit`
//!   after a failed solve). `rdStep` and `rNextPoint` stay in/out arguments (upstream
//!   writes only some members of `rNextPoint`). An uninitialised
//!   `dRadius` (when `SetupOffsetValues` writes nothing) starts at `0.0`.
//! - **Fillet geometries and the pseudo B-rep.** Upstream's `SmFilletGeom*`,
//!   `SmFilletEdge*` and `SmFilletVertex*` are [`FilletGeomId`]s and ids of the
//!   [`FilletBrep`] (`blend.fillet_geom_p1`); the solver's geometries are
//!   [`FilletGeomId`]s in that pseudo B-rep.
//!   `GetFilletExecutive()->GetPseudoBrep()` (unit `blend.fillet_executive_p1`) is the
//!   `fillets` argument; the target B-rep is `target`. Full rollover receives
//!   the caller's [`RailOwnership`] to preserve curve identities and notifications
//!   through the shared p1 rail helpers; cliff trimming resolves its live solver.
//! - **Statuses.** `SE(...)` statuses are ignored, as upstream (debug builds only
//!   print them); [`SolidError::Unported`] always propagates. A `NULL` dereference
//!   upstream is [`SolidError::InvalidInput`]; an index past an array is
//!   [`SolidError::InvalidInput`] (see `refine_rail_curve_classification`).
//! - **Iteration.** No recursion. The loops run over caller arrays (classification
//!   intervals, loop edge uses) or a fixed count (`sm_CalcRailXEdge`'s nine guesses).

use super::p1::{
    compute_support_edge, find_next_good_interval, split_rail, test_manifold_tangent_rollover,
    FilletSolver, FilletSolverOps, FilletSolverType, FilletSphereSolveEnfo, FindClippedRadiusEfo,
    RailOwnership,
};
use crate::blend::fillet_classification::{FilletClassObject, LegacyClassificationObjects};
use crate::blend::fillet_geom::p1::{
    BoundaryTrimmingType, FilStatus, FilletBrep, FilletCornerId, FilletEdgeType, FilletGeomId,
    FilletGeomType, FilletVertex, FilletVertexType,
};
use crate::curve::curve::p1::GlobalPointSolveOptions;
use crate::curve::curve::Curve;
use crate::curve::iso_curve::IsoCurve;
use crate::error::{ignore_status, ignored, EntityRef, Result, SolidError};
use crate::geom::extent1d::{Extent1d, ExtentNd};
use crate::geom::extent2d::Extent2d;
use crate::geom::global_solver::{SolutionRequested, SolverOperation};
use crate::geom::local_solve1d::{EvalFunction, FunctionValue, LocalSolve1d};
use crate::geom::local_solve_nd::{BoundaryHandler, EvalNFunctions, LocalSolveNd};
use crate::geom::matrix::Matrix;
use crate::geom::tol;
use crate::geom::vector2d::Point2;
use crate::intersect::curve_class::p1::CurveClassification;
use crate::intersect::point_class::{ClassGeometry, PointClassType};
use crate::intersect::surface_intersector::p1::{SrfIndex, TsectPnt, MAX_USER_DOUBLES};
use crate::kernel::Kernel;
use crate::math::{Point3, Vec3, BIG_DOUBLE, EFF_ZERO, EFF_ZERO_SQ, EFF_ZERO_SQRT};
use crate::nurbs::b_spline_surface::SurfParamType;
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::nurbs::knots::Side;
use crate::surface::surface::p1::get_surface_cache;
use crate::surface::surface::p4::NoObject;
use crate::surface::surface::Surface;
use crate::topology::edgeuse::{p1 as eu, p2 as edgeuse_p2};
use crate::topology::face::p2 as face_p2;
use crate::topology::{edge, loopuse, vertex, Brep, EdgeId, EdgeuseId, FaceId, VertexId};

const OP: &str = "blend::fillet_solver::p3";

fn invalid(reason: &'static str) -> SolidError {
    SolidError::invalid_input(OP, reason)
}

const fn srf(i: usize) -> SrfIndex {
    if i == 0 {
        SrfIndex::First
    } else {
        SrfIndex::Second
    }
}

// ---- GetOtherFilletCorner ---------------------------------------------------------------

impl FilletSolver {
    /// `GetOtherFilletCorner(pThisCorner)`: the solver's other corner than
    /// `this_corner` (the same one for a closed solver), `None` when `this_corner` is
    /// not one of the solver's corners.
    ///
    /// # Errors
    ///
    /// Those of [`FilletSolver::fillet_corner`] (`GetFilletCorner`, which reads the
    /// filleted edge's vertices in `b`).
    pub fn other_fillet_corner(
        &self,
        b: &Brep,
        this_corner: Option<FilletCornerId>,
        corners: &dyn crate::blend::fillet_geom::p3::SplitCorners,
    ) -> Result<Option<FilletCornerId>> {
        let corner0 = self.fillet_corner(b, 0, corners)?;
        let corner1 = self.fillet_corner(b, 1, corners)?;
        if this_corner == corner0 {
            return Ok(corner1);
        }
        if this_corner == corner1 {
            return Ok(corner0);
        }

        // Error: pThisCorner is not one of ours.
        Ok(None)
    }
}

// ---- sm_FindFilletTsectPnt --------------------------------------------------------------

/// `sm_FindFilletTsectPnt(pFilSolver, cpSurf1, cpSurf2, rPntToDrop, rTraceDir,
/// rNewTsectPnt)`: drops the guess point onto both base surfaces (minimum distance,
/// every solution kept), then solves for the rail points whose offset point lies on
/// the plane through `pnt_to_drop` normal to `trace_dir` (`PointOnPlaneSolve`, given
/// the target `brep` holding the solver's edge uses) and stores `trace_dir` as the
/// curve derivative of `new_tsect`.
///
/// # Errors
///
/// `SM_ERR_NULL_POINTER` when a surface cache cannot be built; `SM_ERR` when a drop
/// finds nothing or the plane solve finds no point; those of the drops and of the
/// solver's `PointOnPlaneSolve`.
pub fn find_fillet_tsect_pnt(
    ops: &mut dyn FilletSolverOps,
    k: &Kernel,
    brep: Option<&mut Brep>,
    surf1: &Surface,
    surf2: &Surface,
    pnt_to_drop: Point3,
    trace_dir: Vec3,
    new_tsect: &mut TsectPnt,
) -> Result<()> {
    const OP_F: &str = "blend::fillet_solver::p3::find_fillet_tsect_pnt";
    // check state: SM_ASSERTs only

    // locals
    let this_approx_tol_3d = ops.solver().this_approx_tol_3d();
    let domain1 = surf1.natural_uv_domain_with_nurb(k)?;
    let domain2 = surf2.natural_uv_domain_with_nurb(k)?;
    for s in [surf1, surf2] {
        // smsurf_GetSurfaceCache + NER
        if let Err(e) = get_surface_cache(k, s, None::<NoObject>) {
            return Err(if e.is_unported() {
                e
            } else {
                SolidError::null_pointer(OP_F)
            });
        }
    }

    // The cache's point test is turned off and its boundary-curve processing on
    // around the drops: for standalone surfaces this changes nothing (see
    // `blend.fillet_geom_p1`, "Surface cache flags").

    // Drop guess point to cpSurf1
    let solutions = surf1.global_point_solve::<NoObject>(
        k,
        None,
        &domain1,
        SolverOperation::Minimize,
        pnt_to_drop,
        this_approx_tol_3d,
        None,
        SolutionRequested::All,
    )?;
    let first = solutions.get(0).ok_or_else(|| SolidError::sm_err(OP_F))?;
    // extract surface1 UVPoint
    let uv1 = Point2::new(first.start.param(0)?, first.start.param(1)?);

    // Drop guess point to cpSurf2
    let solutions = surf2.global_point_solve::<NoObject>(
        k,
        None,
        &domain2,
        SolverOperation::Minimize,
        pnt_to_drop,
        this_approx_tol_3d,
        None,
        SolutionRequested::All,
    )?;
    let first = solutions.get(0).ok_or_else(|| SolidError::sm_err(OP_F))?;
    // extract surface2 UVPoint
    let uv2 = Point2::new(first.start.param(0)?, first.start.param(1)?);

    // Find surface uvPnts on plane = [rPntToDrop, rTraceDir]
    // (virtual: the solver's override, the base's in unit blend.fillet_solver_p2)
    let found_solution = ops.point_on_plane_solve(
        k,
        brep,
        pnt_to_drop,
        trace_dir,
        &domain1,
        &domain2,
        uv1,
        uv2,
        new_tsect,
    )?;
    if !found_solution {
        return Err(SolidError::sm_err(OP_F));
    }

    // store trace direction in data structure
    *new_tsect.crv_deriv_mut() = trace_dir;
    Ok(())
}

// ---- StepSolve --------------------------------------------------------------------------

/// The flags [`FilletSolver::step_solve`] sets (`rbFoundGoodPoint`, `rbClipped`,
/// `rbBoundaryHit`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[allow(clippy::struct_excessive_bools)] // upstream's three SmBoolean out-params
pub struct StepSolveOutcome {
    /// A next point was found.
    pub found_good_point: bool,
    /// The step was clipped to stay in the UV domains.
    pub clipped: bool,
    /// The current point is on a boundary: no step is possible.
    pub boundary_hit: bool,
}

/// Element `i` (0 or 1) of a pair; any other index is the second.
fn get2<T: Copy>(a: &[T; 2], i: usize) -> T {
    if i == 0 {
        a[0]
    } else {
        a[1]
    }
}

/// Mutable [`get2`].
fn get2_mut<T>(a: &mut [T; 2], i: usize) -> &mut T {
    if i == 0 {
        &mut a[0]
    } else {
        &mut a[1]
    }
}

/// `|new| < |old| - SM_EFF_ZERO |old|` or a direction change: the clip shortened the
/// step.
fn step_was_clipped(step_size: f64, old_step_size: f64) -> bool {
    step_size.abs() < old_step_size.abs() - EFF_ZERO * old_step_size.abs()
        || step_size * old_step_size < 0.0
}

impl FilletSolver {
    /// `StepSolve(crCurrentPoint, rdStep, crUVDomain1, crUVDomain2, rbFoundGoodPoint,
    /// rbClipped, rbBoundaryHit, rNextPoint)` dispatched on `ops`: steps `step` along
    /// the current point's UV tangents, clipped to the UV domains. A clipped step finds
    /// the next point on the domain boundary (a radius solve for a constant-distance
    /// solver, an iso-curve/offset-surface intersection otherwise, refined onto the
    /// normal plane for a constant-radius-assisted solver) and rescales `step`;
    /// otherwise (or when that fails) the solver's equations plus the plane through
    /// `CrvPos + step CrvDeriv` normal to `CrvDeriv` are solved by Newton iteration,
    /// once more with an adjusting plane when the first solve fails. Writes the UV
    /// points (and the solver's extra unknowns into `m_adUserDoubles`) of `next` and the
    /// flags to `out` (reset first, as upstream initialises them; on an error they keep
    /// what was set before it). `brep` is passed through to `LoadInitialValues`.
    ///
    /// # Errors
    ///
    /// Those of the solver's virtuals (`GetSolverType`, `GetJacobianSize`,
    /// `SetupOffsetValues` (a radius law that cannot be evaluated), `LoadInitialValues`),
    /// the evaluations and the solvers; `SM_ERR` for a failed
    /// variable-radius drop, a failed constant-radius-assisted plane solve, or more
    /// extra unknowns than `SM_MAX_USER_DOUBLES`; [`SolidError::InvalidInput`] for a
    /// missing offset surface, a variable-radius type without its solver or a Jacobian
    /// size below 4 (upstream indexes past `sIntervals`); `SM_ERR_NULL_POINTER` for a
    /// variable-radius solver without original curve; [`SolidError::Unported`] for a
    /// constant-distance solver's distance; those of the
    /// constant-radius-assisted plane solve (the solver's `PointOnPlaneSolve`).
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub fn step_solve(
        ops: &mut dyn FilletSolverOps,
        k: &Kernel,
        mut brep: Option<&mut Brep>,
        current: &TsectPnt,
        step: &mut f64,
        uv_domain1: &Extent2d,
        uv_domain2: &Extent2d,
        next: &mut TsectPnt,
        out: &mut StepSolveOutcome,
    ) -> Result<()> {
        const OP_S: &str = "blend::fillet_solver::p3::step_solve";
        // init output
        *out = StepSolveOutcome::default();

        let uv_domains = [*uv_domain1, *uv_domain2];

        // First pass - just use first derivative of parameter space curves to get
        // start point.
        let mut old_step_size = *step;
        let mut surf_index = 0usize;

        // see if uvLineSegment steps outside of UVDomain1
        let mut step_size = uv_domains[0].clip_line(
            current.uv_pos(SrfIndex::First),
            current.uv_deriv(SrfIndex::First),
            old_step_size,
        );

        // remember when stepSize is truncated by UVDomain1
        if step_was_clipped(step_size, old_step_size) {
            old_step_size = step_size;
            surf_index = 0;
            out.clipped = true;
        }

        // see if (possibly shortened) lineSegment steps outside of UVDomain2
        step_size = uv_domains[1].clip_line(
            current.uv_pos(SrfIndex::Second),
            current.uv_deriv(SrfIndex::Second),
            old_step_size,
        );

        // remember when stepSize is truncated by UVDomain2
        if step_was_clipped(step_size, old_step_size) {
            old_step_size = step_size;
            surf_index = 1;
            out.clipped = true;
        }

        // avoid tolerance sized variations in dStepSize by setting it equal to
        // dOldStepSize
        step_size = old_step_size;

        // quit - when clipped stepsize is zero, or direction change
        if tol::is_zero(step_size) || step_size * *step < 0.0 {
            out.found_good_point = false;
            out.boundary_hit = true;
            return Ok(());
        }

        // compute next sUV guess points for each surface
        let mut uv = [
            current.uv_pos(SrfIndex::First) + step_size * current.uv_deriv(SrfIndex::First),
            current.uv_pos(SrfIndex::Second) + step_size * current.uv_deriv(SrfIndex::Second),
        ];

        // clamp points to UVDomains
        uv[0] = uv_domains[0].clamp_point(uv[0]);
        uv[1] = uv_domains[1].clamp_point(uv[1]);

        // when next step LineSegment was clipped
        if out.clipped {
            let si = surf_index;
            let oi = 1 - si;
            let (this_uv, this_domain) = (get2(&uv, si), get2(&uv_domains, si));
            // set isoParameter values for when uvPoint is on U extent boundary
            let (surf_param, iso_param, guess) = if (this_uv.x - this_domain.min().x).abs()
                < EFF_ZERO
                || (this_uv.x - this_domain.max().x).abs() < EFF_ZERO
            {
                (SurfParamType::U, this_uv.x, this_uv.y)
            } else {
                // set isoParameter values for when uvPoint is on V extent boundary
                (SurfParamType::V, this_uv.y, this_uv.x)
            };

            // locals
            let mut radius = 0.0;
            // Upstream ignores the status (its `SetupOffsetValues` never reports one, but sets
            // the offsets from an unset radius when the law cannot be evaluated: UB-252). The
            // port's reports that failure, and so does this, as the fork's `StepSolve` does
            // (source fork@9a12ad7).
            ops.setup_offset_values(current, Some(&mut radius))?;

            // branch on SolverType
            let solver_type = ops.solver_type()?;
            if solver_type == FilletSolverType::ConstDist {
                // implicit radius function: F(radius) = distance between rail curve
                // points at the intersection - desired distance
                let target_distance = ops.constant_distance()?;
                let mut efo = FindClippedRadiusEfo::new(
                    ops.solver_mut(),
                    si,
                    &uv_domains,
                    surf_param,
                    iso_param,
                    uv,
                    target_distance,
                )?;
                let interval = Extent1d::from_min_max(0.0, efo.target_distance * 10.0);
                let mut ls = LocalSolve1d::with_interval(interval, false);
                let mut radius_found = 0.0;

                // NewtonRaphson iteration for the radius whose boundary crossing has
                // rail points the proper distance apart
                match ls.solve_it(&mut ClippedRadiusFn { k, efo: &mut efo }, radius, 1.0e-12) {
                    Ok(o) => {
                        out.found_good_point = o.found;
                        radius_found = o.t;
                    }
                    Err(e) if e.is_unported() => return Err(e),
                    Err(_) => out.found_good_point = false,
                }
                // the EFO refines sUV[] through references
                *get2_mut(&mut uv, si) = efo.uv;
                *get2_mut(&mut uv, oi) = efo.other_uv;

                // save found point
                if out.found_good_point {
                    *next.uv_pos_mut(SrfIndex::First) = uv[0];
                    *next.uv_pos_mut(SrfIndex::Second) = uv[1];
                    next.user_doubles[0] = radius_found;
                }
            } else {
                // explicit radius functions
                if solver_type == FilletSolverType::VariableRadius {
                    let surface = ops
                        .solver()
                        .surface(si)?
                        .ok_or_else(|| invalid("no offset surface"))?;
                    let srf_pt = surface.surface.surface.evaluate_point(k, get2(&uv, si))?;
                    let approx_tol = ops.solver().approx_tol_3d;
                    // `(SmVariableRadiusFS*) this`: only that class reports the type.
                    let var_rad = ops
                        .variable_radius_mut()
                        .ok_or_else(|| invalid("variable-radius type without its solver"))?;
                    let curve = var_rad
                        .curve_based
                        .original_curve()
                        .ok_or_else(|| SolidError::null_pointer(OP))?;
                    let mut ivl = var_rad.fillet_law_interval();
                    // This will usually be outside of this domain, so:
                    ivl.expand_relative(2.0);
                    let solutions = curve.global_point_solve::<NoObject>(
                        k,
                        &ivl,
                        SolverOperation::Minimize,
                        srf_pt,
                        approx_tol,
                        SolutionRequested::Single,
                        &GlobalPointSolveOptions::default(),
                    )?;
                    if solutions.len() != 1 {
                        return Err(SolidError::sm_err(OP));
                    }
                    // Parameter gets stored in here:
                    next.user_doubles[0] = solutions
                        .get(0)
                        .ok_or_else(|| SolidError::sm_err(OP))?
                        .start
                        .param(0)?;
                    // Status ignored upstream; reported as for the call above (UB-252).
                    ops.setup_offset_values(next, Some(&mut radius))?;
                }

                // Create iso-curve (the iso parameter is not clamped to the natural
                // domain of the surface)
                let iso_curve = iso_curve_of(k, ops.solver(), si, surf_param, iso_param)?;

                // get OtherOffsetSurface/IsoParameterCurve intersection
                let other = Surface::from(
                    ops.solver()
                        .surface(oi)?
                        .ok_or_else(|| invalid("no other offset surface"))?
                        .surface
                        .surface
                        .clone(),
                );
                let found = other.local_curve_intersect(
                    k,
                    &get2(&uv_domains, oi),
                    &iso_curve,
                    &iso_curve.natural_interval(),
                    ops.solver().this_approx_tol_3d(),
                    get2(&uv, oi),
                    guess,
                );
                // upstream evaluates the solver's own offsets: they keep the extensions
                // (also on a failure)
                ops.solver_mut()
                    .keep_extension_of_iso_copy(si, &iso_curve)?;
                ops.solver_mut().keep_extension_of_copy(oi, &other)?;
                let found = found?;
                out.found_good_point = found.is_some();

                // when an intersection point was found
                if let Some(f) = found {
                    let t_found = f.t;
                    // store solution surface uvPoints
                    if surf_param == SurfParamType::U {
                        get2_mut(&mut uv, si).y = t_found;
                    } else {
                        get2_mut(&mut uv, si).x = t_found;
                    }
                    *get2_mut(&mut uv, oi) = f.uv;

                    // set rNextPoint outputs
                    *next.uv_pos_mut(SrfIndex::First) = uv[0];
                    *next.uv_pos_mut(SrfIndex::Second) = uv[1];

                    // constant radius assisted fillets: refine the rail points onto
                    // the plane through the centre point normal to the tangent
                    if solver_type == FilletSolverType::ConstRadiusAssisted {
                        // Move offset surfaces to base surfaces (out-of-bounds
                        // evaluations; also leaves sPlaneOrig not offset).
                        ops.solver_mut().set_surfaces_to_zero_offset()?;

                        // the iso curve points at the (now zero-offset) surface
                        let iso_curve = iso_curve_of(k, ops.solver(), si, surf_param, iso_param)?;
                        let plane_orig = iso_curve.evaluate_point(k, t_found);
                        ops.solver_mut()
                            .keep_extension_of_iso_copy(si, &iso_curve)?;
                        let plane_orig = plane_orig?;

                        // get railcurve point surface normals
                        let normal1 = offset_normal(k, ops.solver_mut(), 0, uv[0])?;
                        let normal2 = offset_normal(k, ops.solver_mut(), 1, uv[1])?;
                        let plane_normal = normal1.cross(normal2).unitize()?;

                        // find two railcurve points on plane at radius distance from
                        // plane's origin
                        // (virtual: the solver's override, the base's in unit
                        // blend.fillet_solver_p2)
                        let found_solution = ops.point_on_plane_solve(
                            k,
                            brep.as_deref_mut(),
                            plane_orig,
                            plane_normal,
                            &uv_domains[0],
                            &uv_domains[1],
                            uv[0],
                            uv[1],
                            next,
                        )?;

                        // restore the surface offsets
                        ops.solver_mut().reload_surface_offsets()?;

                        if !found_solution {
                            return Err(SolidError::sm_err(OP_S));
                        }
                    }
                }
            }

            // when a next point was found - surf uvPoints are set
            if out.found_good_point {
                // set stepSize based on point found and relative distance from the
                // old point.
                let diff = current.uv_pos(SrfIndex::First) - next.uv_pos(SrfIndex::First);
                let leng_diff = diff.length();
                let uv0 =
                    current.uv_pos(SrfIndex::First) + *step * current.uv_deriv(SrfIndex::First);
                let diff_old = current.uv_pos(SrfIndex::First) - uv0;
                let mut leng_tan = diff_old.length();
                if leng_tan < EFF_ZERO {
                    leng_tan = 1.0;
                }
                *step *= leng_diff / leng_tan;
                return Ok(());
            }
        }

        // Not clipped (or no point found on the boundary): the next point solves the
        // offset/offset intersection near the guess sUV[].

        // Set up Solver
        let jacobian_size = ops.jacobian_size()?;
        let mut intervals = ExtentNd::new(jacobian_size);
        for (i, ext) in [
            Extent1d::from_min_max(uv_domain1.min().x, uv_domain1.max().x),
            Extent1d::from_min_max(uv_domain1.min().y, uv_domain1.max().y),
            Extent1d::from_min_max(uv_domain2.min().x, uv_domain2.max().x),
            Extent1d::from_min_max(uv_domain2.min().y, uv_domain2.max().y),
        ]
        .into_iter()
        .enumerate()
        {
            *intervals
                .get_mut(i)
                .ok_or_else(|| invalid("Jacobian size below 4"))? = ext;
        }

        // set all boundaries as nonPeriodic
        let mut periodicities = vec![false; 4];

        // set 1st guess
        let mut guess_t = vec![uv[0].x, uv[0].y, uv[1].x, uv[1].y];

        // set solver initialization values
        let mut surface1 = ops
            .solver()
            .surface(0)?
            .ok_or_else(|| invalid("no first offset surface"))?
            .surface
            .clone();
        let do_surf2_calcs = false; // Don't do surface2 calculations
        let surf1_index = 0; // UV values of Surface1
        let mut surf2_index = 2; // UV values of Surface2
        let rail_index = 0;

        // given current inputs - place a good guess for upcoming solve in sGuessT
        let initial = ops.load_initial_values(
            k,
            brep,
            do_surf2_calcs,
            rail_index,
            &mut surface1,
            surf1_index,
            &mut intervals,
            &mut periodicities,
            &mut guess_t,
        );
        // upstream passes the solver's own surface: it keeps the extensions the
        // evaluations made (also on a failure)
        ops.solver_mut()
            .surface_mut(0)?
            .ok_or_else(|| invalid("no first offset surface"))?
            .surface
            .surface
            .take_extension_of(&surface1.surface);
        if let Some(iv) = initial? {
            surf2_index = iv.surface2_index;
        }

        // the plane through the stepped centre point, normal to the tangent
        let plane_abc = current.crv_deriv();
        let plane_orig = current.crv_pos() + *step * plane_abc;
        let plane_d = -plane_orig.dot(plane_abc);
        let approx_tol_3d = ops.solver().this_approx_tol_3d();
        let mut eval_fun = SphereSolveFn {
            k,
            enfo: FilletSphereSolveEnfo::new(plane_abc, plane_d, ops),
        };

        // NewtonRaphson solver
        let mut ls = LocalSolveNd::new(&mut eval_fun, Some(&intervals), Some(&periodicities));
        ls.set_desired_accuracy(EFF_ZERO_SQ);
        ls.set_boundary_handler(BoundaryHandler::TotalBoundaryHits, 8);
        ls.set_maximum_iterations(30);

        // Use NewtonRaphson to iterate to solution
        let mut solution = ls.solve_it(&guess_t, approx_tol_3d)?;

        // Try it again and allow the plane to adjust as convergence happens
        if !solution.found {
            ls.evaluator().enfo.set_plane_adjusts(true);
            solution = ls.solve_it(&guess_t, approx_tol_3d)?;
        }

        // when a solution was found - set output
        if solution.found {
            out.found_good_point = true;
            let x = &solution.x;
            let at = |i: usize| -> Result<f64> {
                x.get(i)
                    .copied()
                    .ok_or_else(|| invalid("solution vector too short"))
            };
            *next.uv_pos_mut(SrfIndex::First) = Point2::new(at(surf1_index)?, at(surf1_index + 1)?);
            *next.uv_pos_mut(SrfIndex::Second) =
                Point2::new(at(surf2_index)?, at(surf2_index + 1)?);

            // Load the other variables being solved for into the fillet point.
            for (count, &v) in x.iter().skip(surf2_index + 2).enumerate() {
                if count == MAX_USER_DOUBLES {
                    return Err(SolidError::sm_err(OP_S));
                }
                if let Some(slot) = next.user_doubles.get_mut(count) {
                    *slot = v;
                }
            }
        }

        Ok(())
    }
}

/// `SmIsoCurve(*GetSurface(lSurfIndex), eSurfParam, dIsoParam, TRUE, FALSE)` on a copy
/// of the solver's offset surface.
fn iso_curve_of(
    k: &Kernel,
    solver: &FilletSolver,
    surf_index: usize,
    surf_param: SurfParamType,
    iso_param: f64,
) -> Result<Curve> {
    let s = solver
        .surface(surf_index)?
        .ok_or_else(|| invalid("no offset surface"))?
        .surface
        .surface
        .clone();
    Ok(Curve::from(IsoCurve::from_surface(
        k,
        Surface::from(s),
        surf_param,
        iso_param,
        true,
        false,
    )?))
}

/// `GetSurface(rail)->EvaluateNormal(uv, TRUE, TRUE, ..)` on the solver's own offset
/// surface, which keeps an extension the evaluation makes (as upstream's).
fn offset_normal(k: &Kernel, solver: &mut FilletSolver, rail: usize, uv: Point2) -> Result<Vec3> {
    solver
        .surface_mut(rail)?
        .ok_or_else(|| invalid("no offset surface"))?
        .surface
        .surface
        .evaluate_normal_extending(k, uv, Side::Left, Side::Left)
}

/// `SmFindClippedRadiusEFO` as `SmLocalSolve1D` evaluates it, with the kernel its
/// evaluations take (the solver's function interface passes none).
struct ClippedRadiusFn<'e, 'a> {
    k: &'e Kernel,
    efo: &'e mut FindClippedRadiusEfo<'a>,
}

impl EvalFunction for ClippedRadiusFn<'_, '_> {
    /// `SmFindClippedRadiusEFO::Evaluate` ([`FindClippedRadiusEfo::evaluate`]).
    fn evaluate(&mut self, t: f64, out: &mut FunctionValue, _signal_errors: bool) -> Result<()> {
        let (f, f_prime, found) = self.efo.evaluate(self.k, t)?;
        out.f = f;
        out.f_prime = f_prime;
        out.found_answer = found;
        Ok(())
    }
}

/// `SmFilletSphereSolveENFO` as `SmLocalSolveND` evaluates it, with the kernel its
/// evaluations take (the solver's function interface passes none).
struct SphereSolveFn<'e, 'a> {
    k: &'e Kernel,
    enfo: FilletSphereSolveEnfo<'a>,
}

impl EvalNFunctions for SphereSolveFn<'_, '_> {
    /// `SmFilletSphereSolveENFO::Evaluate` ([`FilletSphereSolveEnfo::evaluate`]).
    fn evaluate(
        &mut self,
        x: &[f64],
        f: &mut [f64],
        jacobian: Option<&mut Matrix>,
    ) -> Result<bool> {
        self.enfo.evaluate(self.k, x, f, jacobian)
    }
}

// ---- SplitAndTrimFilletSurface ----------------------------------------------------------

/// The trim interval pair of two overlapping classification intervals: their
/// intersection (`SM_BT_MINIMAL`), their union (`SM_BT_MAXIMAL`), else each interval.
/// Upstream sets both from `rCIvl1.Intersect/Union(rCIvl2)`; an `Intersect` failure
/// (disjoint: excluded by the callers) leaves the "inside out" extent.
fn trim_pair(
    trim_type: BoundaryTrimmingType,
    ivl1: &Extent1d,
    ivl2: &Extent1d,
) -> Option<(Extent1d, Extent1d)> {
    match trim_type {
        BoundaryTrimmingType::Minimal => {
            let i = ivl1.intersect(ivl2).unwrap_or_else(|_| Extent1d::new());
            Some((i, i))
        }
        BoundaryTrimmingType::Maximal => {
            let u = ivl1.union(ivl2);
            Some((u, u))
        }
        _ => None,
    }
}

impl FilletSolver {
    /// `SplitAndTrimFilletSurface(lFilletGeomIndex, eTrimType)`: pairs the good
    /// (face- or edge-interior) intervals of the fillet geometry's two rail
    /// classifications that overlap and whose trimmed lengths (`ApproximateLength`, 5
    /// samples) reach the solver's approximation tolerance, then splits the geometry
    /// along the pairs (`SplitFilletGeom`); with no overlapping pair the fillet surface
    /// is deleted. The solver's geometries live in `fillets` (the pseudo B-rep);
    /// `geometry` resolves the classifications' curves.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` for a rail or rail classification that is missing;
    /// [`SolidError::InvalidInput`] for a geometry index past the solver's geometries
    /// or a classification without a resolvable curve (upstream dereferences it);
    /// those of the lengths and of [`FilletSolver::split_fillet_geom`]. `bases` are the
    /// base B-reps the end curves read for rails 0 and 1 (`SplitFilletGeom`'s
    /// `CreateEndCurve`).
    #[allow(clippy::too_many_lines)]
    pub fn split_and_trim_fillet_surface(
        &mut self,
        k: &Kernel,
        fillets: &mut FilletBrep,
        geometry: &impl ClassGeometry<EntityRef>,
        fillet_geom_index: usize,
        trim_type: BoundaryTrimmingType,
        bases: [&Brep; 2],
    ) -> Result<()> {
        self.split_and_trim_fillet_surface_in(
            k,
            fillets,
            &LegacyClassificationObjects(geometry),
            fillet_geom_index,
            trim_type,
            bases,
        )
    }

    /// [`Self::split_and_trim_fillet_surface`] with the classifications' actual
    /// geometry provider (a live session's rails and target faces) and the base
    /// B-reps the end curves read for rails 0 and 1 (`SplitFilletGeom`'s
    /// `CreateEndCurve`; both the target, or a surface/surface fillet's second base
    /// for rail 1).
    ///
    /// # Errors
    /// As [`Self::split_and_trim_fillet_surface`], with those of the end curves.
    pub fn split_and_trim_fillet_surface_with(
        &mut self,
        k: &Kernel,
        fillets: &mut FilletBrep,
        geometry: &impl ClassGeometry<FilletClassObject>,
        fillet_geom_index: usize,
        trim_type: BoundaryTrimmingType,
        bases: [&Brep; 2],
    ) -> Result<()> {
        self.split_and_trim_fillet_surface_in(
            k,
            fillets,
            geometry,
            fillet_geom_index,
            trim_type,
            bases,
        )
    }

    fn split_and_trim_fillet_surface_in(
        &mut self,
        k: &Kernel,
        fillets: &mut FilletBrep,
        geometry: &impl ClassGeometry<FilletClassObject>,
        fillet_geom_index: usize,
        trim_type: BoundaryTrimmingType,
        bases: [&Brep; 2],
    ) -> Result<()> {
        const OP_T: &str = "blend::fillet_solver::p3::split_and_trim_fillet_surface";
        let fillet_geom = *self
            .fillet_geoms
            .get(fillet_geom_index)
            .ok_or_else(|| invalid("fillet geometry index out of range"))?;
        let rail_class = |fillets: &FilletBrep, which: usize| -> Result<_> {
            let rail = fillets
                .geom(fillet_geom)?
                .rail(which)
                .ok_or_else(|| invalid("fillet geometry without rail"))?;
            fillets
                .fillet_edge(rail)?
                .curve_class
                .clone()
                .ok_or_else(|| SolidError::null_pointer(OP_T))
        };
        let cc1 = rail_class(fillets, 0)?;
        let cc2 = rail_class(fillets, 1)?;
        let interval_of = |cc: &CurveClassification<FilletClassObject>,
                           i: usize|
         -> Result<Extent1d> { Ok(cc.at(i)?.interval) };

        // Find first interval to be used (sm_FindNextGoodInterval leaves the index
        // unchanged when it finds nothing)
        let next_good = |cc: &CurveClassification<FilletClassObject>, start: &mut usize| -> bool {
            match find_next_good_interval(cc, *start) {
                Some(i) => {
                    *start = i;
                    true
                }
                None => false,
            }
        };
        let (mut start1, mut start2) = (0usize, 0usize);
        let mut found1 = next_good(&cc1, &mut start1);
        let mut found2 = next_good(&cc2, &mut start2);

        let mut found_ivls: Vec<(usize, usize)> = Vec::new();

        while found1 && found2 {
            // upstream indexes past the classification when a side ran out of
            // intervals while the other advanced (see below): InvalidInput here
            let civl1 = interval_of(&cc1, start1)?;
            let civl2 = interval_of(&cc2, start2)?;
            // disjoint intervals: just get the next good interval
            if !civl1.are_disjoint(&civl2, 0.0) {
                let (ivl1, ivl2) = trim_pair(trim_type, &civl1, &civl2).unwrap_or((civl1, civl2));
                // Make sure that length of the intervals is sufficient.
                let curve1 = class_curve(geometry, &cc1)?;
                let curve2 = class_curve(geometry, &cc2)?;
                let tol = self.this_approx_tol_3d();
                let mut too_small = false;
                if curve1.approximate_length(k, &ivl1, 5, false)?.0 < tol {
                    too_small = true;
                }
                if curve2.approximate_length(k, &ivl2, 5, false)?.0 < tol {
                    too_small = true;
                }
                if !too_small {
                    found_ivls.push((start1, start2));
                }
            }
            found1 = false;
            found2 = false;
            // Move to next good interval(s); the second branch sets bFound1 to TRUE
            // even when the first one found nothing, as upstream
            if civl1.max() < civl2.max() + EFF_ZERO_SQ {
                start1 += 1;
                found2 = true;
                found1 = next_good(&cc1, &mut start1);
            }
            if civl2.max() < civl1.max() + EFF_ZERO_SQ {
                found1 = true;
                start2 += 1;
                found2 = next_good(&cc2, &mut start2);
            }
        }

        let mut ivls1 = Vec::new();
        let mut ivls2 = Vec::new();
        let mut found_intersection = false;
        for (i, &(i1, i2)) in found_ivls.iter().enumerate() {
            let civl1 = interval_of(&cc1, i1)?;
            let civl2 = interval_of(&cc2, i2)?;
            if !civl1.are_disjoint(&civl2, 0.0) {
                found_intersection = true;
                let (ivl1, ivl2) = match trim_type {
                    BoundaryTrimmingType::Minimal | BoundaryTrimmingType::Maximal => {
                        trim_pair(trim_type, &civl1, &civl2).unwrap_or((civl1, civl2))
                    }
                    BoundaryTrimmingType::None => {
                        if i > 0 {
                            break;
                        }
                        (cc1.interval(), cc2.interval())
                    }
                    _ => (civl1, civl2),
                };
                ivls1.push(ivl1);
                ivls2.push(ivl2);
            }
        }

        if found_intersection {
            self.split_fillet_geom_with_target(
                k,
                fillets,
                geometry,
                fillet_geom_index,
                trim_type as i32,
                &ivls1,
                &ivls2,
                &cc1,
                &cc2,
                bases,
            )?;
        } else {
            // No Good classifications.  Delete the fillet surface.
            fillets.geom_mut(fillet_geom)?.set_fillet_surface(None);
        }
        Ok(())
    }
}

/// `pCC->GetCurve()`, dereferenced.
fn class_curve<'g>(
    geometry: &'g impl ClassGeometry<FilletClassObject>,
    cc: &CurveClassification<FilletClassObject>,
) -> Result<&'g Curve> {
    cc.curve()
        .and_then(|c| geometry.curve(c))
        .ok_or_else(|| invalid("classification without a curve"))
}

// ---- sm_RefineRailCurveClassification ---------------------------------------------------

/// `GetEdgeObject()` of a point classification: the target edge, `None` when the
/// point is not on an edge.
fn edge_object(
    pc: &crate::intersect::point_class::PointClassification<FilletClassObject>,
) -> Result<Option<EdgeId>> {
    match pc.edge_object() {
        None => Ok(None),
        Some(object) => match object.target_entity()? {
            EntityRef::Edge(e) => Ok(Some(e)),
            _ => Err(invalid("edge classification object is not an edge")),
        },
        // upstream casts the object to SmEdge* blindly
    }
}

/// `pCurve->GlobalPointSolve(crIvl, SM_SO_MINIMIZE, crPnt, SM_EFF_ZERO_SQRT, NULL, NULL,
/// SM_SR_SINGLE, rSolutions)`: the parameter of the first solution, `None` when there
/// is none. With `ignore_failure` (`SE(...)`) a failed solve finds nothing.
fn drop_param(
    k: &Kernel,
    curve: &Curve,
    interval: &Extent1d,
    pnt: Point3,
    ignore_failure: bool,
) -> Result<Option<(f64, f64)>> {
    let r = curve.global_point_solve::<NoObject>(
        k,
        interval,
        SolverOperation::Minimize,
        pnt,
        EFF_ZERO_SQRT,
        SolutionRequested::Single,
        &GlobalPointSolveOptions::default(),
    );
    let solutions = if ignore_failure {
        match ignored(r)? {
            Some(s) => s,
            None => return Ok(None),
        }
    } else {
        r?
    };
    match solutions.get(0) {
        Some(s) => Ok(Some((s.start.param(0)?, s.start.solution_value))),
        None => Ok(None),
    }
}

/// `sm_RefineRailCurveClassification(lRailIndex, crSmallIntervals, rRail,
/// rFilletGeom, rRailCurveClassification)`: for each small interval, from the last
/// one, whose ends classify to the same target edge of the rail's original face,
/// computes where the rail of that edge use would cross the edge at both ends
/// (`RailEdgeuseIntersect`); when the two crossings are within the edge's tolerance,
/// the neighbouring intervals meet at their mid parameter and the small interval is
/// removed. `solver` is `rFilletGeom.GetFilletSolver()`; `rail` is the pseudo-B-rep
/// edge of `rRail`.
///
/// As upstream, the loop counts `ii` from the number of small intervals down to 1 and
/// works on classification interval `ii` itself (not on `crSmallIntervals[ii - 1]`),
/// and compares `ii` with the interval count taken before any removal.
///
/// # Errors
///
/// `SM_ERR` (after visiting every small interval) when any interval cannot be
/// refined; [`SolidError::InvalidInput`] for a classification index past the
/// intervals (upstream reads out of bounds), an edge classification object that is not
/// an edge, or a rail without a curve or original face where they are dereferenced;
/// [`SolidError::InvalidInput`] where `RailEdgeuseIntersect` finds no crossing or
/// fails (its status is `SE`-ignored upstream, which then reads the uninitialised
/// `dEUParamMin` / `dEUParamMax`); [`SolidError::Unported`] from the solver.
#[allow(clippy::too_many_arguments)]
pub fn refine_rail_curve_classification(
    solver: &mut FilletSolver,
    k: &Kernel,
    target: &mut Brep,
    fillets: &FilletBrep,
    rail_index: usize,
    small_intervals: &[usize],
    rail: EdgeId,
    classification: &mut CurveClassification<EntityRef>,
) -> Result<()> {
    let mut scoped = classification.map_objects(&mut |o| Ok(FilletClassObject::Legacy(o)))?;
    let result = refine_rail_curve_classification_scoped(
        solver,
        k,
        target,
        fillets,
        rail_index,
        small_intervals,
        rail,
        &mut scoped,
    );
    *classification = scoped.map_objects(&mut |o| {
        o.legacy()
            .ok_or_else(|| invalid("scoped classification reached indexed caller"))
    })?;
    result
}

/// Refine a retained classification without losing target/pseudo provenance.
///
/// # Errors
/// As [`refine_rail_curve_classification`], including wrong-body edge handles.
#[allow(clippy::too_many_arguments)]
pub fn refine_rail_curve_classification_scoped(
    solver: &mut FilletSolver,
    k: &Kernel,
    target: &mut Brep,
    fillets: &FilletBrep,
    rail_index: usize,
    small_intervals: &[usize],
    rail: EdgeId,
    classification: &mut CurveClassification<FilletClassObject>,
) -> Result<()> {
    refine_rail_curve_classification_scoped_with_ops(
        solver,
        k,
        target,
        fillets,
        rail_index,
        small_intervals,
        rail,
        classification,
    )
}

/// Refine a scoped classification with the actual solver's virtual operations.
///
/// # Errors
/// As [`refine_rail_curve_classification_scoped`].
#[allow(clippy::too_many_arguments)]
pub fn refine_rail_curve_classification_scoped_with_ops(
    solver: &mut dyn FilletSolverOps,
    k: &Kernel,
    target: &mut Brep,
    fillets: &FilletBrep,
    rail_index: usize,
    small_intervals: &[usize],
    rail: EdgeId,
    classification: &mut CurveClassification<FilletClassObject>,
) -> Result<()> {
    const OP_R: &str = "blend::fillet_solver::p3::refine_rail_curve_classification";
    // Locals
    let rail_face = fillets.fillet_edge(rail)?.orig_face;
    let num_small_ivls = small_intervals.len();
    let num_ivls = classification.size();
    // s3DPntMin / s3DPntMax live across iterations: a failed (ignored) evaluation
    // keeps the previous value
    let mut pnt_min = Point3::UNDEF;
    let mut pnt_max = Point3::UNDEF;
    // sTsectPntMin / sTsectPntMax too (RailEdgeuseIntersect writes them in place)
    let mut tsect_min = TsectPnt::default();
    let mut tsect_max = TsectPnt::default();
    let mut failed = false;

    for ii in (1..=num_small_ivls).rev() {
        // The first and last intervals should have been squeezed by TestRollOver
        if ii == num_ivls {
            failed = true;
            continue;
        }

        let ci = classification.at(ii)?;
        let ivl = ci.interval;
        let start_edge = edge_object(&ci.start)?;
        let end_edge = edge_object(&ci.end)?;

        // Must have the interval classify to the same edge on both ends.
        let Some(edge) = start_edge.filter(|_| start_edge == end_edge) else {
            failed = true;
            continue;
        };

        // the Edgeuse connecting the RailFace and the Edge that crosses the Rail
        let Some(side_eu) = edge::edgeuse_of_face(target, edge, rail_face)? else {
            failed = true;
            continue;
        };

        // the RailCurve local end points (statuses ignored)
        let rail_curve =
            edge::curve(fillets.brep(), rail)?.ok_or_else(|| invalid("rail without a curve"))?;
        if let Some(p) = ignored(rail_curve.evaluate_point(k, ivl.min()))? {
            pnt_min = p;
        }
        if let Some(p) = ignored(rail_curve.evaluate_point(k, ivl.max()))? {
            pnt_max = p;
        }

        let edge_ivl = edge::interval(target, edge)?;
        // a copy: the solver below may give the side edge use a UV trim curve
        let edge_curve = edge::curve(target, edge)?
            .ok_or_else(|| invalid("edge without a curve"))?
            .clone();
        let edge_curve = &edge_curve;
        let zone_tol_3d = edge::tolerance(target, edge)?;

        // a guess for the Edge parameter where it crosses the rail at the Ivl min
        let Some((guess_min, _)) = drop_param(k, edge_curve, &edge_ivl, pnt_min, true)? else {
            failed = true;
            continue;
        };

        // the Edge parameter where it will cross the rail-to-be-created for Ivl min
        let eu_param_min = rail_edgeuse_param(
            solver,
            k,
            target,
            rail_index,
            side_eu,
            guess_min,
            &mut tsect_min,
        )?;

        // a guess for the Edge parameter at the Ivl max
        let Some((guess_max, _)) = drop_param(k, edge_curve, &edge_ivl, pnt_max, true)? else {
            failed = true;
            continue;
        };

        let eu_param_max = rail_edgeuse_param(
            solver,
            k,
            target,
            rail_index,
            side_eu,
            guess_max,
            &mut tsect_max,
        )?;

        // Evaluate the params so we can check if the points are within tolerance
        if let Some(p) = ignored(edge_curve.evaluate_point(k, eu_param_min))? {
            pnt_min = p;
        }
        if let Some(p) = ignored(edge_curve.evaluate_point(k, eu_param_max))? {
            pnt_max = p;
        }

        // Can't squeeze the interval if it's not smaller than tolerance
        if pnt_min.distance(pnt_max) > zone_tol_3d {
            failed = true;
            continue;
        }

        // Set the intervals of the CurveIntervals on each side of the short interval
        let new_param = (eu_param_max + eu_param_min) / 2.;
        let mut max_side_ivl = classification.at(ii + 1)?.interval;
        let mut min_side_ivl = classification.at(ii - 1)?.interval;
        ignore_status(max_side_ivl.set_min(new_param))?;
        ignore_status(min_side_ivl.set_max(new_param))?;
        classification.at_mut(ii + 1)?.set_interval(&max_side_ivl);
        classification.at_mut(ii - 1)?.set_interval(&min_side_ivl);

        // Remove the small interval
        classification.remove_at(ii)?;
    }

    if failed {
        return Err(SolidError::sm_err(OP_R));
    }
    Ok(())
}

/// `SE(pFilletSolver->RailEdgeuseIntersect(lRailIndex, pSideEU, dGuessParam,
/// bFoundIntersection, rTsectPnt, rdEUParam))` of `sm_RefineRailCurveClassification`:
/// the edge use parameter. The status is signalled only; without a crossing (or on a
/// failure) upstream goes on with the uninitialised parameter, which is
/// [`SolidError::InvalidInput`] here. `Unported` propagates.
#[allow(clippy::too_many_arguments)]
fn rail_edgeuse_param(
    solver: &mut dyn FilletSolverOps,
    k: &Kernel,
    target: &mut Brep,
    rail_index: usize,
    side_eu: EdgeuseId,
    guess: f64,
    tsect: &mut TsectPnt,
) -> Result<f64> {
    match FilletSolver::rail_edgeuse_intersect(solver, k, target, rail_index, side_eu, guess, tsect)
    {
        Ok(Some(param)) => Ok(param),
        Err(e) if e.is_unported() => Err(e),
        // the uninitialised dEUParamMin / dEUParamMax would be read next
        Ok(None) | Err(_) => Err(invalid(
            "RailEdgeuseIntersect found no crossing (uninitialised edge use parameter upstream)",
        )),
    }
}

// ---- sm_CalcRailXEdge -------------------------------------------------------------------

/// `sm_CalcRailXEdge(pFV, pFG, lRailIndex, pRail, pSideEdge, pGuess3DPnt, iDebugLevel)`:
/// intersects rail `rail_index` of the fillet geometry with the side edge's edge use on
/// the rail's original face (`RailEdgeuseIntersect`, from the drop of `guess` onto the
/// edge, else from up to nine guesses along the edge), storing the result in the
/// tsect point of the vertex use of `vertex` at the rail end; the vertex (and its mate,
/// on the other rail) take the surface points and are marked processed. `solver` is
/// `pFG->GetFilletSolver()`.
///
/// # Errors
///
/// `SM_ERR_NULL_POINTER` when the side edge has no edge use on the rail's face or a
/// rail has no original face; `SM_ERR` when the guess drop or the intersection finds
/// nothing; [`SolidError::InvalidInput`] where upstream dereferences `NULL`; those of
/// the drop, the evaluations and `SmFilletSolver::RailEdgeuseIntersect`.
#[allow(clippy::too_many_arguments)]
pub fn calc_rail_x_edge(
    solver: &mut FilletSolver,
    k: &Kernel,
    target: &mut Brep,
    fillets: &mut FilletBrep,
    vertex: VertexId,
    fillet_geom: FilletGeomId,
    rail_index: usize,
    rail: EdgeId,
    side_edge: EdgeId,
    guess: Option<Point3>,
) -> Result<()> {
    calc_rail_x_edge_with_ops(
        solver,
        k,
        target,
        fillets,
        vertex,
        fillet_geom,
        rail_index,
        rail,
        side_edge,
        guess,
    )
}

/// [`calc_rail_x_edge`] with the actual solver's virtual operations retained.
///
/// # Errors
/// As [`calc_rail_x_edge`], including failures from the actual solver's virtuals.
#[allow(clippy::too_many_arguments)]
pub fn calc_rail_x_edge_with_ops(
    solver: &mut dyn FilletSolverOps,
    k: &Kernel,
    target: &mut Brep,
    fillets: &mut FilletBrep,
    vertex: VertexId,
    fillet_geom: FilletGeomId,
    rail_index: usize,
    rail: EdgeId,
    side_edge: EdgeId,
    guess: Option<Point3>,
) -> Result<()> {
    const OP_X: &str = "blend::fillet_solver::p3::calc_rail_x_edge";
    // Find the right edgeuse for solver to work with (for a seam, all are on the face).
    let rail_face = fillets.fillet_edge(rail)?.orig_face;
    let mut side_eu = None;
    for eu in edge::edgeuses(target, side_edge, None, None)? {
        if eu::face(target, eu)? == rail_face {
            side_eu = Some(eu);
            break;
        }
    }
    let side_eu = side_eu.ok_or_else(|| SolidError::null_pointer(OP_X))?;
    let ivl = edge::interval(target, side_edge)?;
    let mut have_guess_t = false;
    let mut guess_param = 0.0;
    if let Some(g) = guess {
        let curve =
            edge::curve(target, side_edge)?.ok_or_else(|| invalid("edge without a curve"))?;
        let (t, _) =
            drop_param(k, curve, &ivl, g, false)?.ok_or_else(|| SolidError::sm_err(OP_X))?;
        have_guess_t = true;
        guess_param = t;
    }

    // Call solver
    let vu = fillets
        .vu_at_rail_end(vertex, Some(fillet_geom), None, None)?
        .ok_or_else(|| invalid("no vertex use at the rail end"))?;
    // rTsectPnt is the vertex use's own point: written in place, also on a failure
    let mut tsect = fillets.fillet_vertexuse(vu)?.tsect_pnt().clone();
    let mut found_intersection = false;
    for j in 1..=9u32 {
        if !have_guess_t {
            guess_param = ivl.evaluate(0.1 * f64::from(j));
        }
        let r = FilletSolver::rail_edgeuse_intersect(
            solver,
            k,
            target,
            rail_index,
            side_eu,
            guess_param,
            &mut tsect,
        );
        fillets.fillet_vertexuse_mut(vu)?.set_tsect_pnt(&tsect);
        found_intersection = r?.is_some();
        if have_guess_t || found_intersection {
            break;
        }
    }
    if !found_intersection {
        return Err(SolidError::sm_err(OP_X));
    }

    let face = fillets
        .fillet_edge(rail)?
        .orig_face
        .ok_or_else(|| SolidError::null_pointer(OP_X))?;
    let uv = fillets
        .fillet_vertexuse(vu)?
        .tsect_pnt()
        .uv_pos(srf(rail_index));
    let pnt = face_surface(target, face)?.evaluate_point(k, uv)?;
    fillets.fillet_vertex_mut(vertex)?.set_original_uv(uv);
    fillets.set_vertex_point(vertex, pnt)?;
    fillets
        .fillet_vertex_mut(vertex)?
        .set_status(FilStatus::Processed);

    if let Some(mate) = fillets.fillet_vertex(vertex)?.mate(0) {
        let rail1 = fillets
            .geom(fillet_geom)?
            .rail(1 - rail_index)
            .ok_or_else(|| invalid("fillet geometry without rail"))?;
        let face1 = fillets
            .fillet_edge(rail1)?
            .orig_face
            .ok_or_else(|| SolidError::null_pointer(OP_X))?;
        let uv1 = fillets
            .fillet_vertexuse(vu)?
            .tsect_pnt()
            .uv_pos(srf(1 - rail_index));
        let pnt1 = face_surface(target, face1)?.evaluate_point(k, uv1)?;
        fillets.fillet_vertex_mut(mate)?.set_original_uv(uv1);
        fillets.set_vertex_point(mate, pnt1)?;
        fillets
            .fillet_vertex_mut(mate)?
            .set_status(FilStatus::Processed);
        let mate_vu = fillets
            .vu_at_rail_end(mate, Some(fillet_geom), None, None)?
            .ok_or_else(|| invalid("no mate vertex use at the rail end"))?;
        *fillets
            .fillet_vertexuse_mut(mate_vu)?
            .tsect_pnt_mut()
            .uv_pos_mut(srf(1 - rail_index)) = uv1;
    }
    Ok(())
}

/// `pFace->GetSurface()`, dereferenced.
fn face_surface(target: &Brep, face: FaceId) -> Result<&Surface> {
    target
        .face(face)?
        .surface
        .as_ref()
        .ok_or_else(|| invalid("face without a surface"))
}

// ---- ProcessFullRollOver ----------------------------------------------------------------

impl FilletSolver {
    /// `ProcessFullRollOver(pFilletGeom, lRollOverRailIndex)`: the fillet rolls over
    /// every edge (cliff) of the rail's original face. The geometry becomes a cliff
    /// rollover; the rail is split at each edge of the face's first loop, starting at
    /// the edge nearest the rail start and walking the loop in the rail's direction,
    /// each piece getting a side fillet geometry on the extended side face
    /// (`CreateCliffRollOverSideFG`); the new vertices are then computed (rail/edge
    /// intersection when trimming side faces, else the support edges) and the cliff
    /// rails trimmed. Side faces are trimmed unless the solver's edge use is a convex
    /// radial sector. `fillet_geom` is the geometry in `fillets` (the executive's pseudo
    /// B-rep). Tangent
    /// rollover is not handled (upstream: "Unimplemented Tangent Rollover Case").
    /// `ownership` resolves the caller's curve owner identities and notifications.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` for a missing fillet surface, rail curve, original face,
    /// side face, mate edge use or support edge; `SM_ERR` for a tangent side edge;
    /// [`SolidError::InvalidInput`] where upstream dereferences `NULL` (no solver edge
    /// use, no face use or loop use, no edge nearest the rail start); those of the
    /// convexity test, the drops and evaluations; [`SolidError::Unported`] from
    /// `sm_SplitRail` (`blend.fillet_solver_p1`, its fillet geometry calls),
    /// `SmFilletEdge::MakeFilletEdge` ([`FilletBrep::make_fillet_edge`], while the
    /// pseudo B-rep has no infinite region or shell), those of
    /// [`FilletSolver::create_cliff_roll_over_side_fg`] and the remaining callees of
    /// cliff rail trimming.
    /// The caller's ownership routing and actual geometry solver id are retained.
    #[allow(clippy::too_many_lines, clippy::too_many_arguments)]
    pub fn process_full_roll_over(
        &mut self,
        k: &Kernel,
        target: &mut Brep,
        fillets: &mut FilletBrep,
        fillet_geom: FilletGeomId,
        roll_over_rail_index: usize,
        ownership: &mut dyn RailOwnership,
    ) -> Result<()> {
        const OP_P: &str = "blend::fillet_solver::p3::process_full_roll_over";
        let nul = || SolidError::null_pointer(OP_P);
        let geom = fillet_geom;

        let solver_eu = self
            .edgeuse(0)?
            .ok_or_else(|| invalid("solver without edge use"))?;

        // Determine if we need to 'add' or 'trim' side faces
        let trim_side_faces =
            !edgeuse_p2::is_convex_radial_sector(k, target, solver_eu, 10, false, 0.0)?;

        // the pseudo B-rep is `fillets`
        fillets
            .geom_mut(geom)?
            .set_fillet_geom_type(FilletGeomType::CliffRollover);

        let fillet_surface = fillets
            .geom(geom)?
            .fillet_surface(None)?
            .cloned()
            .ok_or_else(nul)?;
        let mut rail = fillets
            .geom(geom)?
            .rail(roll_over_rail_index)
            .ok_or_else(|| invalid("fillet geometry without rail"))?;
        let orig_curve = edge::curve(fillets.brep(), rail)?
            .cloned()
            .ok_or_else(nul)?;
        let orig_face = fillets.fillet_edge(rail)?.orig_face.ok_or_else(nul)?;
        let fu = face_p2::upward_faceuse(target, orig_face)?
            .ok_or_else(|| invalid("face without upward face use"))?;

        // Collect the edgeuses of the outer loop
        let loopuses = eu::faceuse_loopuses(target, fu, false, None)?;
        let lu = *loopuses
            .first()
            .ok_or_else(|| invalid("face use without loop use"))?;
        let edgeuses = loopuse::edgeuses(target, lu, None, None)?;

        // Determine which edge the start-of-the-rail is 'faced' to by finding the
        // minimum distance from the rail start to each edge
        let start_v =
            edge::vertex(fillets.brep(), rail)?.ok_or_else(|| invalid("rail without vertex"))?;
        let rail_start_pnt = fillets.vertex_point(start_v)?;
        let mut dir = Vec3::UNDEF;

        let mut min_dist = BIG_DOUBLE;
        let total_edges = edgeuses.len();
        let mut start_eu = None;

        for &e_use in &edgeuses {
            let side_edge =
                eu::edge(target, e_use)?.ok_or_else(|| invalid("edge use without edge"))?;
            let (tangent, _opposite_normals) =
                test_manifold_tangent_rollover(k, target, side_edge, self.tangency_tolerance)?;
            if tangent {
                // Unimplemented Tangent Rollover Case
                return Err(SolidError::sm_err(OP_P));
            }

            let side_curve =
                edge::curve(target, side_edge)?.ok_or_else(|| invalid("edge without a curve"))?;
            let ivl = edge::interval(target, side_edge)?;
            if let Some((t, dist)) = drop_param(k, side_curve, &ivl, rail_start_pnt, false)? {
                if dist < min_dist {
                    min_dist = dist;
                    start_eu = Some(e_use);
                    let pnts = side_curve.evaluate(k, t, 1, true, true)?;
                    dir = *pnts.get(1).ok_or_else(|| invalid("missing derivative"))?;
                }
            }
        }

        // Is the rail running the same direction as the loop
        let orig_ivl = edge::interval(fillets.brep(), rail)?;
        let pnts = orig_curve.evaluate(k, orig_ivl.min(), 1, true, true)?;
        let tangent0 = *pnts.get(1).ok_or_else(|| invalid("missing derivative"))?;
        let orient = if tangent0.dot(dir) > 0.0 {
            OrientType::Same
        } else {
            OrientType::Opposite
        };

        // Get the edgeuse that goes along the rail's direction
        let mut start_eu = start_eu.ok_or_else(|| invalid("no edge nearest the rail start"))?;
        if orient != eu::orientation(target, start_eu)? {
            start_eu = eu::mate(target, start_eu)?.ok_or_else(nul)?;
        }

        fillets
            .fillet_edge_mut(rail)?
            .set_fillet_edge_type(FilletEdgeType::CliffRail);
        let mut all_new_verts: Vec<VertexId> = Vec::new();

        // Traverse the loop
        let mut curr_eu = start_eu;
        for j in 0..=total_edges {
            let next_eu = eu::ccw_edgeuse(target, curr_eu)?
                .ok_or_else(|| invalid("edge use without ccw edge use"))?;
            let side_edge =
                eu::edge(target, curr_eu)?.ok_or_else(|| invalid("edge use without edge"))?;
            let radial_eu =
                eu::radial(target, curr_eu)?.ok_or_else(|| invalid("edge use without radial"))?;
            let side_face = eu::face(target, radial_eu)?.ok_or_else(nul)?;
            let next_vu = eu::vertexuse(target, next_eu)?
                .ok_or_else(|| invalid("edge use without vertex use"))?;
            let curr_v = target
                .vertexuse(next_vu)?
                .vertex
                .ok_or_else(|| invalid("vertex use without vertex"))?;

            let mut new_rail = None;
            if j != total_edges {
                // Split the rail now
                let (new_v, split) = split_rail(k, fillets, ownership, fillet_geom, rail)?;
                new_rail = Some(split);

                let cw = eu::cw_edgeuse(target, radial_eu)?;
                let fv = fillets.fillet_vertex_mut(new_v)?;
                fv.set_fillet_vertex_type(FilletVertexType::CliffRailXEdge);
                fv.set_point_class(PointClassType::Edgeuse, cw.map(EntityRef::Edgeuse));
                all_new_verts.push(new_v);

                let g = fillets.geom_mut(geom)?;
                if roll_over_rail_index == 0 {
                    g.other_rails1.push(split);
                } else {
                    g.other_rails2.push(split);
                }
                fillets
                    .fillet_edge_mut(split)?
                    .set_fillet_edge_type(FilletEdgeType::CliffRail);

                if !trim_side_faces {
                    // Create an edge connecting pNewV & pCurrV as the 'support' edge
                    let end_point = target.vertex(curr_v)?.point;
                    let end_vert = fillets.brep_mut().add_vertex(end_point)?;
                    let mut record = FilletVertex::new(k, None);
                    record.set_status(FilStatus::Processed);
                    fillets.insert_fillet_vertex(end_vert, record);
                    let new_edge = fillets.make_fillet_edge(Some(end_vert), Some(new_v), None)?;
                    fillets
                        .fillet_edge_mut(new_edge)?
                        .set_fillet_edge_type(FilletEdgeType::OnExtendEdge);
                }
            }

            // Now this rail becomes a rail along the cliff: a side fillet geometry
            // coinciding with the extended side face
            fillets
                .fillet_edge_mut(rail)?
                .set_fillet_edge_type(FilletEdgeType::CliffRail);
            let extension_dist = self
                .surface(0)?
                .ok_or_else(|| invalid("no first offset surface"))?
                .surface
                .offset_distance()
                .abs();
            // `this` is the solver of the geometry being rolled over
            let solver_id = fillets.geom(geom)?.fillet_solver().ok_or_else(nul)?;
            let _new_side_fg: Option<FilletGeomId> = self.create_cliff_roll_over_side_fg(
                k,
                target,
                fillets,
                solver_id,
                rail,
                side_edge,
                side_face,
                trim_side_faces,
                extension_dist,
            )?;

            if let Some(r) = new_rail {
                rail = r;
            }
            curr_eu = next_eu;
        }

        // Calculate the newly created vertices
        for kk in 0..total_edges {
            let new_v = *all_new_verts
                .get(kk)
                .ok_or_else(|| invalid("missing new vertex"))?;
            if trim_side_faces {
                // Intersect the curve with the fillet for the pNewV's geometry
                let tol = self.this_approx_tol_3d();
                fillets.calc_cliff_rail_int_edge(k, new_v, target, &fillet_surface, tol)?;
            } else {
                // Find the 'support' edge and compute its geometry
                let mut edge_on_edge = None;
                for e in vertex::edges(fillets.brep(), new_v, None)?
                    .into_iter()
                    .take(3)
                {
                    if fillets.fillet_edge(e)?.edge_type == FilletEdgeType::OnExtendEdge {
                        edge_on_edge = Some(e);
                        break;
                    }
                }
                let edge_on_edge = edge_on_edge.ok_or_else(nul)?;
                compute_support_edge(
                    k,
                    fillets,
                    ownership,
                    edge_on_edge,
                    geom,
                    self.this_approx_tol_3d(),
                    self.this_ang_tol_rad(),
                )?;
            }
        }

        // Finally, trim all rails of the fillet
        let solver_id = fillets.geom(geom)?.fillet_solver().ok_or_else(nul)?;
        fillets.trim_rail_curves_with_solver(k, geom, solver_id, self, target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blend::fillet_custom_solver::{
        InitialValues, JacobianLoad, OffsetSurface, SurfaceId,
    };
    use crate::blend::fillet_geom::p1::FilletGeom;
    use crate::blend::fillet_solver::p1::OffsetSurfaceSlot;
    use crate::nurbs::b_spline_curve::BSplineCurve;
    use crate::nurbs::b_spline_surface::BSplineSurface;
    use crate::nurbs::math::{CPoint, NOW};
    use crate::nurbs::srf_basic::Surface as NlSurface;
    use crate::surface::offset_surface::SS_UNKNOWN;
    use crate::surface::surface::p1::evaluate_1st_derivatives;
    use crate::surface::volume::Volume;

    /// `origin + u du + v dv` over `[0, 1]^2`.
    fn patch(origin: [f64; 3], du: [f64; 3], dv: [f64; 3]) -> Surface {
        let pt = |s: f64, t: f64| {
            CPoint::new(
                origin[0] + s * du[0] + t * dv[0],
                origin[1] + s * du[1] + t * dv[1],
                origin[2] + s * du[2] + t * dv[2],
                NOW,
            )
        };
        let net = vec![
            vec![pt(0.0, 0.0), pt(0.0, 1.0)],
            vec![pt(1.0, 0.0), pt(1.0, 1.0)],
        ];
        let nl = NlSurface::new(
            net,
            1,
            1,
            vec![0.0, 0.0, 1.0, 1.0],
            vec![0.0, 0.0, 1.0, 1.0],
        )
        .expect("patch");
        Surface::from(BSplineSurface::from_nurb(nl, false))
    }

    const L: f64 = 2.0;
    const R: f64 = 0.2;

    /// The floor `(u L, v L, 0)` and the wall `(0, u L, v L)` (normals +z, +x).
    fn floor_and_wall() -> (Surface, Surface) {
        (
            patch([0.0; 3], [L, 0.0, 0.0], [0.0, L, 0.0]),
            patch([0.0; 3], [0.0, L, 0.0], [0.0, 0.0, L]),
        )
    }

    /// The runner's harness solver: offsets `R`, the offset/offset intersection.
    struct TestSolver {
        base: FilletSolver,
        ty: FilletSolverType,
        /// `SetupOffsetValues` fails (a law that cannot be evaluated), before it sets anything.
        setup_fails: bool,
    }

    impl TestSolver {
        fn new(ty: FilletSolverType) -> Self {
            let k = Kernel::default();
            let (s0, s1) = floor_and_wall();
            let mut base = FilletSolver::new(1e-6, 0.1, 0.01);
            for (i, s) in [s0, s1].into_iter().enumerate() {
                let off = OffsetSurface::new(&k, R, s, SurfaceId::Free(i as u32), SS_UNKNOWN)
                    .expect("offset");
                base.set_surface(
                    i,
                    Some(OffsetSurfaceSlot {
                        key: i as u64,
                        surface: off,
                    }),
                )
                .expect("set");
            }
            Self {
                base,
                ty,
                setup_fails: false,
            }
        }
    }

    impl FilletSolverOps for TestSolver {
        fn solver(&self) -> &FilletSolver {
            &self.base
        }
        fn solver_mut(&mut self) -> &mut FilletSolver {
            &mut self.base
        }
        fn solver_type(&self) -> Result<FilletSolverType> {
            Ok(self.ty)
        }
        fn constant_distance(&self) -> Result<f64> {
            Ok(R * 2.0f64.sqrt())
        }
        fn jacobian_size(&self) -> Result<usize> {
            Ok(4)
        }
        fn setup_offset_values(&mut self, _t: &TsectPnt, d: Option<&mut f64>) -> Result<()> {
            if self.setup_fails {
                return Err(SolidError::sm_err(
                    "test: the radius law cannot be evaluated",
                ));
            }
            for s in self.base.surfaces.iter_mut().flatten() {
                s.surface.set_offset_distance(R);
            }
            if let Some(d) = d {
                *d = R;
            }
            Ok(())
        }
        fn load_initial_values(
            &mut self,
            _k: &Kernel,
            _brep: Option<&mut Brep>,
            _do2: bool,
            _rail: usize,
            _s1: &mut OffsetSurface,
            _i1: usize,
            _ext: &mut ExtentNd,
            _per: &mut Vec<bool>,
            _guess: &mut Vec<f64>,
        ) -> Result<Option<InitialValues>> {
            Ok(None)
        }
        fn load_jacobian(
            &mut self,
            k: &Kernel,
            ne: &mut usize,
            np: &mut usize,
            load: &mut JacobianLoad<'_>,
            f: &mut [f64],
            jacobian: Option<&mut Matrix>,
        ) -> Result<bool> {
            let x = load.x;
            let eval = |i: usize, uv: Point2| {
                let mut s = self.base.surfaces[i].as_ref().unwrap().surface.clone();
                s.surface
                    .with_evaluator(|e| evaluate_1st_derivatives(k, e, uv, Side::Left, Side::Left))
            };
            let o1 = load.surf1_offset;
            let (p, pu, pv) = eval(0, Point2::new(x[o1], x[o1 + 1]))?;
            let (q, qu, qv) = eval(1, Point2::new(x[*np], x[*np + 1]))?;
            let d = p - q;
            f[*ne] = d.x;
            f[*ne + 1] = d.y;
            f[*ne + 2] = d.z;
            if let Some(j) = jacobian {
                for (i, (a, b, c, e)) in [
                    (pu.x, pv.x, qu.x, qv.x),
                    (pu.y, pv.y, qu.y, qv.y),
                    (pu.z, pv.z, qu.z, qv.z),
                ]
                .into_iter()
                .enumerate()
                {
                    j.set_at(*ne + i, o1, a)?;
                    j.set_at(*ne + i, o1 + 1, b)?;
                    j.set_at(*ne + i, *np, -c)?;
                    j.set_at(*ne + i, *np + 1, -e)?;
                }
            }
            load.surf2_offset = *np;
            *ne += 3;
            *np += 2;
            Ok(d.length() < 1e-12)
        }
    }

    /// The centre-line point at `y` (tangent +y).
    fn centre_point(y: f64) -> TsectPnt {
        let mut t = TsectPnt::new();
        *t.uv_pos_mut(SrfIndex::First) = Point2::new(R / L, y / L);
        *t.uv_pos_mut(SrfIndex::Second) = Point2::new(y / L, R / L);
        *t.uv_deriv_mut(SrfIndex::First) = Point2::new(0.0, 1.0 / L);
        *t.uv_deriv_mut(SrfIndex::Second) = Point2::new(1.0 / L, 0.0);
        *t.crv_pos_mut() = Vec3::new(R, y, R);
        *t.crv_deriv_mut() = Vec3::new(0.0, 1.0, 0.0);
        t
    }

    fn unit_domain() -> Extent2d {
        Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0)
    }

    fn step(ty: FilletSolverType, y: f64, step: f64) -> (Result<StepSolveOutcome>, f64, TsectPnt) {
        let (r, out, step, next) = step_flags(ty, y, step);
        (r.map(|()| out), step, next)
    }

    /// [`step`] with the flags kept on an error.
    fn step_flags(
        ty: FilletSolverType,
        y: f64,
        step: f64,
    ) -> (Result<()>, StepSolveOutcome, f64, TsectPnt) {
        step_with(TestSolver::new(ty), y, step)
    }

    fn step_with(
        mut fs: TestSolver,
        y: f64,
        mut step: f64,
    ) -> (Result<()>, StepSolveOutcome, f64, TsectPnt) {
        let k = Kernel::default();
        let mut next = TsectPnt::new();
        let mut out = StepSolveOutcome {
            found_good_point: true,
            clipped: true,
            boundary_hit: true,
        };
        let r = FilletSolver::step_solve(
            &mut fs,
            &k,
            None,
            &centre_point(y),
            &mut step,
            &unit_domain(),
            &unit_domain(),
            &mut next,
            &mut out,
        );
        (r, out, step, next)
    }

    #[test]
    fn step_solve_steps_along_the_centre_line() {
        let (r, s, next) = step(FilletSolverType::ConstRadius, 0.5, 0.3);
        let out = r.unwrap();
        assert!(
            out.found_good_point && !out.clipped && !out.boundary_hit,
            "{out:?}"
        );
        assert_eq!(s, 0.3);
        let uv0 = next.uv_pos(SrfIndex::First);
        let uv1 = next.uv_pos(SrfIndex::Second);
        assert!(
            (uv0.x - R / L).abs() < 1e-9 && (uv0.y - 0.8 / L).abs() < 1e-9,
            "{uv0:?}"
        );
        assert!(
            (uv1.x - 0.8 / L).abs() < 1e-9 && (uv1.y - R / L).abs() < 1e-9,
            "{uv1:?}"
        );
    }

    #[test]
    fn step_solve_zero_step_hits_the_boundary() {
        let (r, _, _) = step(FilletSolverType::ConstRadius, 0.5, 0.0);
        let out = r.unwrap();
        assert!(out.boundary_hit && !out.found_good_point);
    }

    #[test]
    fn step_solve_clipped_step_ends_on_the_domain_boundary() {
        for ty in [
            FilletSolverType::Unknown,
            FilletSolverType::ConstRadius,
            FilletSolverType::ConstDist,
        ] {
            let (r, s, next) = step(ty, 1.8, 0.5);
            let out = r.unwrap();
            assert!(out.found_good_point && out.clipped, "{ty:?} {out:?}");
            assert!(
                (next.uv_pos(SrfIndex::First).y - 1.0).abs() < 1e-9,
                "{ty:?}"
            );
            assert!((s - 0.2).abs() < 1e-6, "{ty:?} step {s}");
        }
    }

    #[test]
    fn step_solve_casts_to_the_variable_radius_solver_and_runs_the_plane_solve() {
        // a solver reporting SM_FS_VARIABLE_RADIUS without being one: upstream's
        // `(SmVariableRadiusFS*) this` cast is undefined, an input error here
        let (r, _, _) = step(FilletSolverType::VariableRadius, 1.8, 0.5);
        assert!(matches!(r, Err(SolidError::InvalidInput { .. })), "{r:?}");
        // the assisted kind's step ends in the (base) PointOnPlaneSolve, which the test
        // solver's equations satisfy on the clipped step
        let (r, _, _) = step(FilletSolverType::ConstRadiusAssisted, 1.8, 0.5);
        let out = r.unwrap();
        assert!(
            out.found_good_point && out.clipped && !out.boundary_hit,
            "{out:?}"
        );
    }

    /// UB-252: a radius that `SetupOffsetValues` could not set is not stepped on. Upstream ignores
    /// the status (`SmFilletSolver.cpp:5254`) and goes on with the radius unset; the fork returns
    /// the failure (source fork@9a12ad7, source issue #458), and so does the
    /// port. Run in the clipped branch, where the call sits.
    #[test]
    fn step_solve_returns_a_failed_setup_of_the_offsets() {
        for ty in [
            FilletSolverType::Unknown,
            FilletSolverType::ConstRadius,
            FilletSolverType::ConstDist,
            FilletSolverType::VariableRadius,
        ] {
            let mut fs = TestSolver::new(ty);
            fs.setup_fails = true;
            let (r, out, _, next) = step_with(fs, 1.8, 0.5);
            let e = r.expect_err(&format!("{ty:?}: a failed setup is a failed step"));
            assert!(
                !e.is_unported(),
                "{ty:?}: the setup's own failure, not a deferral: {e:?}"
            );
            assert!(
                out.clipped && !out.found_good_point,
                "{ty:?}: nothing found after the failure: {out:?}"
            );
            assert_eq!(
                next.uv_pos(SrfIndex::First),
                TsectPnt::new().uv_pos(SrfIndex::First),
                "{ty:?}: no point stored"
            );
        }
    }

    #[test]
    fn step_solve_keeps_the_flags_set_before_a_failure() {
        // the flags start reset; the clip is recorded and the plane solve succeeds
        let (r, out, _, _) = step_flags(FilletSolverType::ConstRadiusAssisted, 1.8, 0.5);
        assert!(r.is_ok(), "{r:?}");
        assert!(
            out.clipped && out.found_good_point && !out.boundary_hit,
            "{out:?}"
        );
        // the variable-radius cast fails after the clip, before any point is found
        let (r, out, _, _) = step_flags(FilletSolverType::VariableRadius, 1.8, 0.5);
        assert!(r.is_err(), "{r:?}");
        assert!(
            out.clipped && !out.found_good_point && !out.boundary_hit,
            "{out:?}"
        );
    }

    #[test]
    fn find_fillet_tsect_pnt_solves_on_the_plane() {
        let k = Kernel::default();
        let mut fs = TestSolver::new(FilletSolverType::ConstRadius);
        let (s0, s1) = floor_and_wall();
        let mut t = TsectPnt::new();
        let r = find_fillet_tsect_pnt(
            &mut fs,
            &k,
            None,
            &s0,
            &s1,
            Vec3::new(R, 1.0, R),
            Vec3::new(0.0, 1.0, 0.0),
            &mut t,
        );
        // the base PointOnPlaneSolve finds the ball on the plane; the trace direction is
        // stored after it
        assert!(r.is_ok(), "{r:?}");
        assert_eq!(t.crv_deriv(), Vec3::new(0.0, 1.0, 0.0));
    }

    #[test]
    fn other_fillet_corner_needs_the_executive() {
        // no executive: GetFilletExecutive() is dereferenced upstream
        let fs = FilletSolver::new(1e-6, 0.1, 0.01);
        let r = fs.other_fillet_corner(
            &Brep::new(),
            None,
            &crate::blend::fillet_geom::p3::NoCorners,
        );
        assert!(matches!(r, Err(SolidError::InvalidInput { .. })), "{r:?}");
        // with one, the executive's corner of a missing vertex is NULL
        let mut fs = fs;
        fs.executive = Some(crate::blend::fillet_solver::p1::FilletExecutiveRef);
        let r = fs.other_fillet_corner(
            &Brep::new(),
            None,
            &crate::blend::fillet_geom::p3::NoCorners,
        );
        assert_eq!(r, Ok(None));
    }

    /// Resolves every handle to the one rail curve.
    struct OneCurve(Curve);

    impl ClassGeometry<EntityRef> for OneCurve {
        fn curve(&self, _o: EntityRef) -> Option<&Curve> {
            Some(&self.0)
        }
        fn surface(&self, _o: EntityRef) -> Option<&Surface> {
            None
        }
        fn volume(&self, _o: EntityRef) -> Option<&Volume> {
            None
        }
        fn point(&self, _o: EntityRef) -> Option<Point3> {
            None
        }
    }

    fn line(a: [f64; 3], b: [f64; 3]) -> Curve {
        Curve::from(
            BSplineCurve::create_line_segment(3, Vec3::from_array(a), Vec3::from_array(b))
                .expect("line"),
        )
    }

    /// A geometry with two rails classified by `classes` (`(t0, t1, mid class)` each).
    fn geometry_with_rails(
        k: &Kernel,
        classes: [&[(f64, f64, PointClassType)]; 2],
    ) -> (FilletBrep, FilletGeomId, Curve) {
        let mut fb = FilletBrep::new();
        let curve = line([0.0; 3], [1.0, 0.0, 0.0]);
        let mut geom = FilletGeom::new(k, None, FilletGeomType::Default).expect("geom");
        geom.set_fillet_surface(Some(patch([0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0])));
        for (which, cls) in classes.into_iter().enumerate() {
            let v0 = fb.brep_mut().add_vertex(Vec3::new(0.0, 0.0, 0.0)).unwrap();
            let v1 = fb.brep_mut().add_vertex(Vec3::new(1.0, 0.0, 0.0)).unwrap();
            let rail = fb.new_fillet_edge(v0, v1, None).unwrap();
            fb.brep_mut().edge_mut(rail).unwrap().curve = Some(curve.clone());
            let mut cc = CurveClassification::with_curve_interval(
                k,
                EntityRef::Edge(rail),
                Extent1d::from_min_max(0.0, 1.0),
                1e-6,
            );
            cc.set_size(k, cls.len()).unwrap();
            for (i, &(t0, t1, mid)) in cls.iter().enumerate() {
                let ci = cc.at_mut(i).unwrap();
                ci.set_interval(&Extent1d::from_min_max(t0, t1));
                ci.mid.set_class_object(mid, Some(EntityRef::Edge(rail)));
            }
            fb.fillet_edge_mut(rail)
                .unwrap()
                .set_indexed_curve_class(Some(&cc))
                .unwrap();
            geom.set_rail(which, Some(rail));
        }
        let g = fb.add_geom(geom);
        (fb, g, curve)
    }

    #[test]
    fn split_and_trim_deletes_a_surface_without_overlapping_good_intervals() {
        let k = Kernel::default();
        let face = PointClassType::Face;
        let out = PointClassType::Nothing;
        let (mut fb, g, curve) = geometry_with_rails(
            &k,
            [
                &[(0.0, 0.4, face), (0.4, 1.0, out)],
                &[(0.0, 0.5, out), (0.5, 1.0, face)],
            ],
        );
        let mut fs = FilletSolver::new(1e-6, 0.1, 0.01);
        fs.fillet_geoms.push(g);
        fs.split_and_trim_fillet_surface(
            &k,
            &mut fb,
            &OneCurve(curve),
            0,
            BoundaryTrimmingType::Minimal,
            [&Brep::new(), &Brep::new()],
        )
        .unwrap();
        assert!(fb.geom(g).unwrap().fillet_surface.is_none());
    }

    #[test]
    fn split_and_trim_splits_overlapping_good_intervals() {
        let k = Kernel::default();
        let face = PointClassType::Face;
        let (mut fb, g, curve) = geometry_with_rails(
            &k,
            [&[(0.0, 0.6, face)], &[(0.2, 1.0, PointClassType::Edge)]],
        );
        let mut fs = FilletSolver::new(1e-6, 0.1, 0.01);
        fs.fillet_geoms.push(g);
        let r = fs.split_and_trim_fillet_surface(
            &k,
            &mut fb,
            &OneCurve(curve),
            0,
            BoundaryTrimmingType::Maximal,
            [&Brep::new(), &Brep::new()],
        );
        // SplitFilletGeom (wired to blend.fillet_geom_p1 on main) reaches the geometry's
        // rail vertex uses, which this fixture's rails do not have: an upstream error
        assert!(
            matches!(
                r,
                Err(SolidError::Upstream {
                    operation: "blend::fillet_geom::p1::FilletBrep::rail_vertexuse",
                    ..
                })
            ),
            "{r:?}"
        );
        assert!(fb.geom(g).unwrap().fillet_surface.is_some());
    }

    #[test]
    fn split_and_trim_needs_rail_classifications() {
        let k = Kernel::default();
        let (mut fb, g, curve) = geometry_with_rails(&k, [&[], &[]]);
        let rail = fb.geom(g).unwrap().rail(1).unwrap();
        fb.fillet_edge_mut(rail).unwrap().curve_class = None;
        let mut fs = FilletSolver::new(1e-6, 0.1, 0.01);
        fs.fillet_geoms.push(g);
        let r = fs.split_and_trim_fillet_surface(
            &k,
            &mut fb,
            &OneCurve(curve),
            0,
            BoundaryTrimmingType::None,
            [&Brep::new(), &Brep::new()],
        );
        assert!(matches!(r, Err(SolidError::Upstream { .. })), "{r:?}");
    }

    #[test]
    fn refine_rail_curve_classification_reports_unrefinable_intervals() {
        let k = Kernel::default();
        let face = PointClassType::Face;
        let (fb, g, _) = geometry_with_rails(
            &k,
            [
                &[(0.0, 0.3, face), (0.3, 0.31, face), (0.31, 1.0, face)],
                &[],
            ],
        );
        let rail = fb.geom(g).unwrap().rail(0).unwrap();
        let mut cc = fb.fillet_edge(rail).unwrap().curve_class.clone().unwrap();
        let mut fs = FilletSolver::new(1e-6, 0.1, 0.01);
        let mut target = Brep::new();
        // interval 1 does not classify to an edge at either end; interval 3 is the
        // interval count
        let r = refine_rail_curve_classification_scoped(
            &mut fs,
            &k,
            &mut target,
            &fb,
            0,
            &[1, 2, 2],
            rail,
            &mut cc,
        );
        assert!(matches!(r, Err(SolidError::Upstream { .. })), "{r:?}");
        assert_eq!(cc.size(), 3);
    }

    #[test]
    fn calc_rail_x_edge_needs_an_edge_use_on_the_rail_face() {
        let k = Kernel::default();
        let (mut fb, g, _) = geometry_with_rails(&k, [&[], &[]]);
        let rail = fb.geom(g).unwrap().rail(0).unwrap();
        let v = fb.brep_mut().add_vertex(Vec3::new(0.0, 0.0, 0.0)).unwrap();
        let mut target = Brep::new();
        let a = target.add_vertex(Vec3::new(0.0, 0.0, 0.0)).unwrap();
        let b = target.add_vertex(Vec3::new(1.0, 0.0, 0.0)).unwrap();
        let side = target.add_edge(a, b).unwrap();
        let mut fs = FilletSolver::new(1e-6, 0.1, 0.01);
        let r = calc_rail_x_edge(&mut fs, &k, &mut target, &mut fb, v, g, 0, rail, side, None);
        assert!(matches!(r, Err(SolidError::Upstream { .. })), "{r:?}");
    }

    #[test]
    fn process_full_roll_over_needs_the_solver_edge_use() {
        struct NoOwnership;
        impl RailOwnership for NoOwnership {
            fn edge_owner(
                &self,
                _: EdgeId,
            ) -> Result<(
                crate::geom::object::ObjectRef,
                Option<crate::geom::object::ObjectRef>,
            )> {
                Err(invalid("unexpected owner lookup"))
            }
            fn owner_changed(
                &mut self,
                _: &mut Curve,
                _: crate::curve::curve::p2::OwnerChange,
            ) -> Result<()> {
                Err(invalid("unexpected owner notification"))
            }
        }
        let k = Kernel::default();
        let (mut fb, g, _) = geometry_with_rails(&k, [&[], &[]]);
        let mut fs = FilletSolver::new(1e-6, 0.1, 0.01);
        fs.fillet_geoms.push(g);
        let mut target = Brep::new();
        let r = fs.process_full_roll_over(&k, &mut target, &mut fb, g, 0, &mut NoOwnership);
        assert!(matches!(r, Err(SolidError::InvalidInput { .. })), "{r:?}");
    }
}
