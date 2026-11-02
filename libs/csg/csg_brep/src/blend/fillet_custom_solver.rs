//! Custom fillet solvers: the "Eiffel tower" solver (two different offset radii and
//! a swept angle of the larger arc), the secant constant-radius solver (the rolling
//! ball cuts one surface at a secant angle) and the surface generator building the
//! "Eiffel tower" cross-section fillet surface.
//!
//! Upstream: `source/SMLib/src/SmFilletCustomSolver.cpp`,
//! `source/SMLib/inc/SmFilletCustomSolver.h`.
//!
//! | Rust                                              | Upstream                                          |
//! |---------------------------------------------------|---------------------------------------------------|
//! | [`EiffelTowerFs`], [`EiffelTowerFs::new`]         | `SmEiffelTowerFS` (members, constructor; `Drop` is the destructor) |
//! | [`EiffelTowerFs::load_initial_values`]            | `SmEiffelTowerFS::LoadInitialValues`              |
//! | [`EiffelTowerFs::load_jacobian`]                  | `SmEiffelTowerFS::LoadJacobian`                   |
//! | [`EiffelTowerCrossSectionFsg`]                    | `SmEiffelTowerCrossSectionFSG` (constructor; `Drop` is the destructor) |
//! | [`EiffelTowerCrossSectionFsg::create_surface`]    | `SmEiffelTowerCrossSectionFSG::CreateSurface`     |
//! | [`SecantConstantRadiusFs`], [`SecantConstantRadiusFs::new`] | `SmSecantConstantRadiusFS` (members, constructor; `Drop` is the destructor) |
//! | [`SecantConstantRadiusFs::load_initial_values`]   | `SmSecantConstantRadiusFS::LoadInitialValues`     |
//! | [`SecantConstantRadiusFs::load_jacobian`]         | `SmSecantConstantRadiusFS::LoadJacobian`          |
//! | [`ConstantRadiusAssistedFs`] (CO-PORTED)          | `SmConstantRadiusFS` / `SmConstantRadiusAssistedFS` over the `SmFilletSolver` base ([`FilletSolver`]) |
//! | `impl FilletSolverOps for ConstantRadiusAssistedFs` | the same inherited overrides plus `SmConstantRadiusFS::LoadJacobian` (`blend.fillet_standard_solver_p1`'s methods), `SmConstantRadiusAssistedFS::IsKindOf`, `SmConstantRadiusFS::LoadInitialValues` and `SmConstantRadiusAssistedFS::PointOnPlaneSolve` (`_p2`'s) (the class `blend.fillet_intersector` runs `ComputeSurfaceValues` on) |
//! | `impl FilletSolverOps` for both solvers            | their `LoadJacobian` / `LoadInitialValues` overrides; the inherited `SmConstantRadiusFS` / `SmConstantRadiusAssistedFS` overrides `GetJacobianSize`, `SetupOffsetValues`, `OffsetRadiiCanChange`, `GetType` call `blend.fillet_standard_solver_p1`'s, `IsKindOf` and `PointOnPlaneSolve` `_p2`'s |
//! | [`OffsetSurface`], [`SurfaceId`]                  | an `SmOffsetSurface*` rail surface (unit `surface.offset_surface`) and the identity of its base pointer |
//! | [`JacobianLoad`], [`InitialValues`]               | the in/out arguments of `LoadJacobian`, `LoadInitialValues` |
//!
//! Deviations:
//! - **Object model.** The solvers are structs over a co-ported
//!   [`ConstantRadiusAssistedFs`] (the `SmConstantRadiusFS` / `SmConstantRadiusAssistedFS`
//!   members, units `blend.fillet_standard_solver_p1/_p2`) holding the ported
//!   `SmFilletSolver` base ([`FilletSolver`], `blend.fillet_solver_p1`); both solvers
//!   implement [`FilletSolverOps`]. `GetSurface` is the base's (the fillet geometry for
//!   the surfaces is never set here). A
//!   missing surface or an out-of-range rail index (upstream dereferences `NULL` or
//!   indexes past the two-element arrays) is [`SolidError::InvalidInput`]. The
//!   base-class constructor from an edge use ([`ConstantRadiusAssistedFs::from_edgeuse`])
//!   is `blend.fillet_standard_solver_p1`'s `constant_radius_from_edgeuse` (it takes the
//!   B-rep mutably: `SmEdgeuse::EvaluateBinormal` may create UV trim curves, and the
//!   pseudo B-rep that receives the base constructor's fillet geometry, which
//!   `FinishConstruction` fills); [`ConstantRadiusAssistedFs::from_parts`] builds the
//!   members directly with the base-class defaults, without that geometry
//!   ([`ConstantRadiusAssistedFs::from_solver_parts`] on a constructed base).
//! - **Offset surfaces.** A rail surface is an [`OffsetSurface`]: the ported
//!   `SmOffsetSurface` ([`offset_surface::OffsetSurface`], which owns a copy of its
//!   base) with a [`SurfaceId`] standing for the base `SmSurface*`: upstream compares
//!   base pointers (`GetBaseSurface() == ...`, `pF->GetSurface() != pBase2`), which
//!   compare these ids. Its evaluations store the base extension they make, as
//!   upstream's `const` evaluators do, so `crSurface1` of `LoadInitialValues` is
//!   `&mut`. Upstream's `crSurface1` may be one of the solver's own surfaces
//!   (`GetSurface(i)`), which then keeps that extension after the call; here it is the
//!   caller's surface, a separate object (`SmConstantRadiusAssistedFS::PointOnPlaneSolve`,
//!   `blend.fillet_standard_solver_p2`, hands its extension back to the solver's
//!   offset). `GuessPointOnPlaneSolve` runs on the solver itself, whose offsets keep
//!   the extensions it makes.
//!   For the same reason `rpSurface2->LocalPointSolve` runs on a copy of the other
//!   rail's offset and drops an extension it makes (its `MINIMIZE` solve is
//!   `LocalPointSolve_1`).
//! - **Arguments.** Out-params become return values; `SmContext` arguments are dropped.
//!   `LoadJacobian` takes the base-surface identities of `rpSurface1` / `rpSurface2`
//!   (the only use of those pointers) and their offsets by value (never written);
//!   `rbFoundAnswer` is the result. `LoadInitialValues` returns the rail index of
//!   `rpSurface2` (always `GetSurface(1 - lRailIndex)`) and `rlSurface2Index`, or
//!   `None` when `rbDoSurf2Calcs` is `0` (nothing is touched); on an error nothing
//!   is returned (upstream has already set `rpSurface2`, to that same surface).
//!   `CreateSurface` takes
//!   the solver's `GetThisApproxTol3d()` (the only solver value it reads) and drops
//!   its unused centre line, surface and UV curve arguments; the rails are plain
//!   B-spline curves (the fillet intersector builds them with `sm_FlushCrv`) and are
//!   changed in place as upstream (knot insertion and the skinning's rail
//!   synchronisation). The fillet points are the `SmTsectPnt`s themselves.
//! - **Indexing.** `rF`, the Jacobian, `crX`, `rGuessT` and `rIntervals` are indexed
//!   with checks: an index upstream would read or write out of bounds is
//!   [`SolidError::InvalidInput`]. `rIntervals` must have room for the two new
//!   intervals, as upstream's fixed-size `SmExtentNd`.
//! - **Callees.** `SmConstantRadiusAssistedFS::GuessPointOnPlaneSolve` is
//!   `blend.fillet_standard_solver_p2`'s
//!   [`ConstantRadiusAssistedFs::guess_point_on_plane_solve`]. Offset
//!   evaluations use the real offset port; point solvers and default singularity
//!   construction propagate the errors of their surface dispatchers.

use crate::blend::fillet_geom::p1::{FilletBrep, FilletSolverId};
use crate::blend::fillet_intersector::FilletSolverOps as IntersectorOps;
use crate::blend::fillet_solver::p1::{
    FilletSolver, FilletSolverOps, FilletSolverType, OffsetSurfaceSlot,
};
use crate::blend::fillet_standard_solver::p1::CONSTANT_RADIUS_ASSISTED_FS_TYPE;
use crate::curve::curve::p1::GlobalPointSolveOptions;
use crate::curve::curve::p5::is_arc_base;
use crate::curve::curve::Curve;
use crate::error::{ignore_status, Result, SolidError};
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::extent1d::{Extent1d, ExtentNd};
use crate::geom::extent2d::Extent2d;
use crate::geom::geom_utility::p2::segment_segment_intersect;
use crate::geom::global_solver::{SolutionRequested, SolverOperation};
use crate::geom::matrix::Matrix;
use crate::geom::vector2d::{Point2, Vec2};
use crate::intersect::surface_intersector::p1::{SrfIndex, TsectPnt};
use crate::kernel::Kernel;
use crate::math::{Point3, Vec3, EFF_ZERO_SQ, EFF_ZERO_SQRT, PI};
use crate::nurbs::b_spline_curve::p1::NurbCircleParam;
use crate::nurbs::b_spline_curve::BSplineCurve;
use crate::nurbs::b_spline_surface::{BSplineSurface, SurfParamType};
use crate::nurbs::b_spline_surface_n_lib::p2::SkinnedSurfaceOptions;
use crate::nurbs::knots::Side;
use crate::surface::offset_surface;
use crate::surface::surf_of_extrusion::SurfOfExtrusion;
use crate::surface::surface::p1::{evaluate_1st_derivatives, evaluate_2nd_derivatives};
use crate::surface::surface::p4::NoObject;
use crate::surface::surface::p7::{compute_inward_vector, drop_vectors_at};
use crate::surface::surface::{Surface, SurfaceKind};
use crate::topology::edgeuse::p1 as edgeuse;
use crate::topology::{Brep, EdgeuseId, FaceId, ObjectHandle};

const OP: &str = "blend::fillet_custom_solver";
const OP_EIFFEL_INIT: &str = "blend::fillet_custom_solver::EiffelTowerFs::load_initial_values";
const OP_EIFFEL_JAC: &str = "blend::fillet_custom_solver::EiffelTowerFs::load_jacobian";
const OP_SECANT_INIT: &str =
    "blend::fillet_custom_solver::SecantConstantRadiusFs::load_initial_values";
const OP_SECANT_JAC: &str = "blend::fillet_custom_solver::SecantConstantRadiusFs::load_jacobian";
const OP_CREATE: &str = "blend::fillet_custom_solver::EiffelTowerCrossSectionFsg::create_surface";

/// `dAnalyticTol` of `CreateSurface`.
const ANALYTIC_TOL: f64 = 1.0e-8;

/// Sample count of `CreateSurface`'s `IsLine` / `IsArc` tests.
const ANALYTIC_SAMPLES: u64 = 11;

// ---- rail offset surfaces -------------------------------------------------------------

/// The identity of a base surface: upstream compares `SmSurface*` pointers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SurfaceId {
    /// The surface of a face (`SmFace::GetSurface()`).
    Face(FaceId),
    /// A surface not attached to a face; equal numbers are the same object.
    Free(u32),
}

/// A rail's offset surface: the `SmOffsetSurface` (unit `surface.offset_surface`) and
/// the [`SurfaceId`] of the base pointer (`m_pSurface`) upstream compares.
#[derive(Debug, Clone)]
pub struct OffsetSurface {
    /// The offset surface.
    pub surface: offset_surface::OffsetSurface,
    /// The identity of `m_pSurface`.
    pub base_id: SurfaceId,
}

impl OffsetSurface {
    /// `SmOffsetSurface(dOffsetDistance, rSurface, FALSE, lSingularities)`: extension
    /// not allowed. The fillet solvers pass the default `lSingularities`,
    /// [`SS_UNKNOWN`](offset_surface::SS_UNKNOWN).
    ///
    /// # Errors
    ///
    /// Those of [`offset_surface::OffsetSurface::new`] ([`SolidError::Unported`] for
    /// [`SS_UNKNOWN`](offset_surface::SS_UNKNOWN) until `surface.surface_p3` lands).
    pub fn new(
        k: &Kernel,
        offset_distance: f64,
        base: Surface,
        base_id: SurfaceId,
        singularities: u32,
    ) -> Result<Self> {
        Ok(Self {
            surface: offset_surface::OffsetSurface::new(
                k,
                offset_distance,
                base,
                false,
                singularities,
            )?,
            base_id,
        })
    }

    /// `GetBaseSurface`.
    #[must_use]
    pub fn base_surface(&self) -> &Surface {
        self.surface.base_surface()
    }

    /// `GetOffsetDistance`.
    #[must_use]
    pub fn offset_distance(&self) -> f64 {
        self.surface.offset_distance()
    }

    /// `SetOffsetDistance`.
    pub fn set_offset_distance(&mut self, offset_distance: f64) {
        self.surface.set_offset_distance(offset_distance);
    }

    /// `SetAllowExtension`.
    pub fn set_allow_extension(&mut self, allow: bool) {
        self.surface.set_allow_extension(allow);
    }
}

/// `SmOffsetSurface::EvaluatePoint`.
pub(crate) fn offset_evaluate_point(
    k: &Kernel,
    s: &mut OffsetSurface,
    uv: Point2,
) -> Result<Point3> {
    s.surface.evaluate_point_extending(k, uv)
}

/// `SmOffsetSurface::GetNaturalUVDomain`.
pub(crate) fn offset_natural_uv_domain(s: &OffsetSurface) -> Extent2d {
    s.surface.natural_uv_domain()
}

/// `Evaluate1stDerivatives(uv, TRUE, TRUE, ..)` of an offset surface:
/// `(P, S_u, S_v)`.
pub(crate) fn offset_evaluate_1st(
    k: &Kernel,
    s: &mut OffsetSurface,
    uv: Point2,
) -> Result<(Point3, Vec3, Vec3)> {
    s.surface
        .with_evaluator(|e| evaluate_1st_derivatives(k, e, uv, Side::Left, Side::Left))
}

/// `Evaluate2ndDerivatives(uv, TRUE, TRUE, ..)` of an offset surface:
/// `(P, S_u, S_v, S_uv, S_uu, S_vv)`.
pub(crate) fn offset_evaluate_2nd(
    k: &Kernel,
    s: &mut OffsetSurface,
    uv: Point2,
) -> Result<[Vec3; 6]> {
    let d = s
        .surface
        .with_evaluator(|e| evaluate_2nd_derivatives(k, e, uv, Side::Left, Side::Left))?;
    Ok([d.point, d.du, d.dv, d.duv, d.duu, d.dvv])
}

/// `SmSurface::ComputeInwardVector` of an offset surface (it evaluates the offset
/// surface).
pub(crate) fn offset_compute_inward_vector(
    k: &Kernel,
    s: &mut OffsetSurface,
    uv: Point2,
) -> Result<Vec3> {
    s.surface
        .with_evaluator(|e| compute_inward_vector(k, e, uv))
}

/// `LocalPointSolve(domain, SM_SO_MINIMIZE, point, guess, bFound, sSol)` of an offset
/// surface: the found UV point and its distance.
/// `None` when no answer was found. The solve runs on a copy of the offset (the solver
/// is borrowed shared); a base extension it makes is kept by the offset (pending, as
/// its `&self` evaluators keep one), as upstream's `const` evaluators store it.
pub(crate) fn offset_local_point_solve(
    k: &Kernel,
    s: &OffsetSurface,
    domain: &Extent2d,
    point: Point3,
    guess: Point2,
) -> Result<Option<(Point2, f64)>> {
    // SmSurface::LocalPointSolve on the offset: its evaluations dispatch to
    // SmOffsetSurface (a MINIMIZE solve runs LocalPointSolve_1)
    let surface = Surface::from(s.surface.clone());
    let r = surface.local_point_solve::<NoObject>(
        k,
        domain,
        SolverOperation::Minimize,
        point,
        guess,
    )?;
    // (upstream solves on the offset itself: it keeps an extension the solve made)
    if let SurfaceKind::OffsetSurface(o) = surface.kind() {
        s.surface.keep_extension_made_by(o);
    }
    if !r.found {
        return Ok(None);
    }
    let start = &r.solution.start;
    Ok(Some((
        Point2::new(start.param(0)?, start.param(1)?),
        start.solution_value,
    )))
}

/// `GlobalPointSolve(domain, SM_SO_MINIMIZE, point, SM_EFF_ZERO_SQ, NULL, SM_SR_SINGLE,
/// ..)` of a base surface: the UV points of the solutions. `owner` is the B-rep and
/// face owning the surface (a face's surface, `SurfaceId::Face`), whose trim test the
/// fast drop runs (`pFace->PointClassify`, making the face's trimmed cache when it has
/// none).
pub(crate) fn surface_global_point_solve(
    k: &Kernel,
    s: &Surface,
    owner: Option<(&mut Brep, FaceId)>,
    domain: &Extent2d,
    point: Point3,
    tol: f64,
) -> Result<Vec<Point2>> {
    let starts = match owner {
        Some((b, f)) => crate::intersect::point_class::live_owned_surface_global_point_solve(
            k,
            s,
            b,
            f,
            domain,
            SolverOperation::Minimize,
            point,
            tol,
            SolutionRequested::Single,
        )?,
        None => crate::surface::surface::p4::global_point_solve_starts(
            k,
            s,
            domain,
            SolverOperation::Minimize,
            point,
            tol,
            SolutionRequested::Single,
        )?,
    };
    starts
        .iter()
        .map(|e| Ok(Point2::new(e.param(0)?, e.param(1)?)))
        .collect()
}

/// The B-rep and face owning a base surface: `GetOwner()` of a face's surface.
pub(crate) fn face_owner(brep: Option<&mut Brep>, base: SurfaceId) -> Option<(&mut Brep, FaceId)> {
    match (brep, base) {
        (Some(b), SurfaceId::Face(f)) => Some((b, f)),
        _ => None,
    }
}

// ---- co-ported solver data ------------------------------------------------------------

/// The `SmConstantRadiusFS` / `SmConstantRadiusAssistedFS` part the custom solvers read
/// (CO-PORTED from `blend.fillet_standard_solver_p1`): the `SmFilletSolver` base
/// ([`FilletSolver`], unit `blend.fillet_solver_p1`) and the offset radii.
#[derive(Debug, Clone)]
pub struct ConstantRadiusAssistedFs {
    /// The `SmFilletSolver` base.
    pub solver: FilletSolver,
    /// `m_dFilletRadii`: the offset radius of each surface.
    pub fillet_radii: [f64; 2],
    /// The object is an `SmConstantRadiusFS` itself, not an
    /// `SmConstantRadiusAssistedFS`: `GetType`, `IsKindOf`, `GetSolverType` and
    /// `PointOnPlaneSolve` are then `SmConstantRadiusFS`'s (the last the base
    /// `SmFilletSolver::PointOnPlaneSolve`). Set by the constructors that build a plain
    /// `SmConstantRadiusFS` (`SetFilletParameters`); `false` from [`Self::from_parts`].
    pub plain: bool,
}

impl ConstantRadiusAssistedFs {
    /// The base as the `SmFilletSolver` constructor initialises it
    /// (`m_dSurfaceExtensionFactor = 0.5`, `m_dConversionTol = SM_EFF_ZERO / 10`), with
    /// the edge uses, offset surfaces (keys 0 and 1), orientations and radii given (what
    /// the `SmConstantRadiusFS` constructor computes from the edge use). The base is
    /// [`FilletSolver::new`], without the constructor's fillet geometry.
    #[must_use]
    pub fn from_parts(
        approx_tol_3d: f64,
        ang_tol_rad: f64,
        tangency_tolerance: f64,
        fillet_radii: [f64; 2],
        surfaces: [Option<OffsetSurface>; 2],
        orientations: [f64; 2],
        edgeuses: [Option<EdgeuseId>; 2],
    ) -> Self {
        Self::from_solver_parts(
            FilletSolver::new(approx_tol_3d, ang_tol_rad, tangency_tolerance),
            fillet_radii,
            surfaces,
            orientations,
            edgeuses,
        )
    }

    /// [`ConstantRadiusAssistedFs::from_parts`] on a constructed base (the
    /// `SmConstantRadiusFS` constructors pass [`FilletSolver::new_in`]'s, which holds
    /// the constructor's fillet geometry).
    #[must_use]
    pub fn from_solver_parts(
        mut solver: FilletSolver,
        fillet_radii: [f64; 2],
        surfaces: [Option<OffsetSurface>; 2],
        orientations: [f64; 2],
        edgeuses: [Option<EdgeuseId>; 2],
    ) -> Self {
        solver.edgeuses = edgeuses;
        let [s0, s1] = surfaces;
        solver.surfaces = [
            s0.map(|surface| OffsetSurfaceSlot { key: 0, surface }),
            s1.map(|surface| OffsetSurfaceSlot { key: 1, surface }),
        ];
        solver.orientations = orientations;
        Self {
            solver,
            fillet_radii,
            plain: false,
        }
    }

    /// `SmConstantRadiusAssistedFS(crContext, dThisApproxTol3d, dAngleTolerance,
    /// dTangencyTolerance, dFilletRadius1, dFilletRadius2, pEdgeuse)`: the offset
    /// surfaces and orientations of the faces at `edgeuse` and its radial, with the
    /// constructor's fillet geometry in `pseudo` for solver `id`.
    ///
    /// # Errors
    ///
    /// Those of `SmConstantRadiusFS::SmConstantRadiusFS`
    /// ([`crate::blend::fillet_standard_solver::p1::constant_radius_from_edgeuse`]).
    #[allow(clippy::too_many_arguments)]
    pub fn from_edgeuse(
        k: &Kernel,
        brep: &mut Brep,
        pseudo: &mut FilletBrep,
        id: FilletSolverId,
        approx_tol_3d: f64,
        ang_tol_rad: f64,
        tangency_tolerance: f64,
        fillet_radius1: f64,
        fillet_radius2: f64,
        edgeuse: EdgeuseId,
    ) -> Result<Self> {
        crate::blend::fillet_standard_solver::p1::constant_radius_from_edgeuse(
            k,
            brep,
            pseudo,
            id,
            approx_tol_3d,
            ang_tol_rad,
            tangency_tolerance,
            fillet_radius1,
            edgeuse,
            Some(fillet_radius2),
        )
    }

    /// `GetSurface(lRailIndex)`.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for a rail index above 1 or a missing surface.
    pub fn surface(&self, rail: usize) -> Result<&OffsetSurface> {
        self.solver
            .surface(rail)?
            .map(|slot| &slot.surface)
            .ok_or_else(|| SolidError::invalid_input(OP, "no offset surface for the rail"))
    }

    /// Takes over the base extension of `copy`, a copy of `GetSurface(rail)` that a
    /// caller evaluated in its place (`LoadInitialValues`' `crSurface1`): upstream
    /// evaluates the solver's own offset surface, whose `m_pExtendedSurface` then holds
    /// it. A caller that passes a copy of `GetSurface(i)` must hand it back this way
    /// (as `point_on_plane_solve` does).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for a rail index above 1 or a missing surface.
    pub fn take_surface_extension(&mut self, rail: usize, copy: &OffsetSurface) -> Result<()> {
        self.surface_mut(rail)?
            .surface
            .take_extension_of(&copy.surface);
        Ok(())
    }

    pub(crate) fn surface_mut(&mut self, rail: usize) -> Result<&mut OffsetSurface> {
        self.solver
            .surface_mut(rail)?
            .map(|slot| &mut slot.surface)
            .ok_or_else(|| SolidError::invalid_input(OP, "no offset surface for the rail"))
    }

    fn orientation(&self, rail: usize) -> Result<f64> {
        self.solver.orientation(rail)
    }

    fn radius(&self, rail: usize) -> Result<f64> {
        self.fillet_radii
            .get(rail)
            .copied()
            .ok_or_else(|| SolidError::invalid_input(OP, "rail index out of range"))
    }

    /// `GetSurface(i)->SetOffsetDistance(m_dFilletRadii[i]*m_dOrientations[i])`.
    fn reset_offset(&mut self, rail: usize) -> Result<()> {
        let d = self.radius(rail)? * self.orientation(rail)?;
        self.surface_mut(rail)?.set_offset_distance(d);
        Ok(())
    }
}

// ---- shared argument records ----------------------------------------------------------

/// The arguments `LoadJacobian` reads besides the solver (`crX`, the identities of
/// `rpSurface1` / `rpSurface2` and their offsets into `crX`).
#[derive(Debug, Clone, Copy)]
pub struct JacobianLoad<'a> {
    /// `lRailIndex`: the rail of the first surface (0 or 1).
    pub rail_index: usize,
    /// `rpSurface1->GetBaseSurface()`: `None` for a `NULL` `rpSurface1`.
    pub surface1: Option<SurfaceId>,
    /// `rlSurf1Offset`: index of `rpSurface1`'s UV point in `crX`.
    pub surf1_offset: usize,
    /// `rpSurface2->GetBaseSurface()`: `None` for a `NULL` `rpSurface2`.
    pub surface2: Option<SurfaceId>,
    /// `rlSurf2Offset`: index of `rpSurface2`'s UV point in `crX`.
    pub surf2_offset: usize,
    /// `crX`: the current solution.
    pub x: &'a [f64],
}

/// The outputs of `LoadInitialValues` besides the arrays it updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InitialValues {
    /// `rpSurface2`: the rail whose offset surface is the second surface
    /// (`GetSurface(1 - lRailIndex)`).
    pub surface2_rail: usize,
    /// `rlSurface2Index`: index of the second surface's UV point in `rGuessT`.
    pub surface2_index: usize,
}

fn x_at(x: &[f64], i: usize, op: &'static str) -> Result<f64> {
    x.get(i)
        .copied()
        .ok_or_else(|| SolidError::invalid_input(op, "crX index out of range"))
}

fn set_f(f: &mut [f64], i: usize, v: f64, op: &'static str) -> Result<()> {
    let slot = f
        .get_mut(i)
        .ok_or_else(|| SolidError::invalid_input(op, "rF index out of range"))?;
    *slot = v;
    Ok(())
}

fn get_f(f: &[f64], i: usize, op: &'static str) -> Result<f64> {
    f.get(i)
        .copied()
        .ok_or_else(|| SolidError::invalid_input(op, "rF index out of range"))
}

fn set_j(j: &mut Matrix, row: usize, col: usize, v: f64, op: &'static str) -> Result<()> {
    j.set_at(row, col, v)
        .map_err(|_| SolidError::invalid_input(op, "Jacobian index out of range"))
}

fn checked_add(a: usize, b: usize, op: &'static str) -> Result<usize> {
    a.checked_add(b)
        .ok_or_else(|| SolidError::invalid_input(op, "index overflow"))
}

/// The `crX` offsets of the two surfaces' UV points (the index logic shared by both
/// `LoadJacobian`s): `lSurf1Off`, then `lSurf2Off`, advancing `rlNumParameters` as
/// upstream. `base` are `GetSurface(0)` and `GetSurface(1)`'s base identities.
fn surface_offset(
    num_parameters: &mut usize,
    load: &JacobianLoad<'_>,
    rail: usize,
    base: SurfaceId,
    op: &'static str,
) -> Result<usize> {
    let mut off = *num_parameters;
    *num_parameters = checked_add(*num_parameters, 2, op)?;
    // rail 0's surface is surface 1 for lRailIndex 0, surface 2 for lRailIndex 1;
    // rail 1's the other way round
    let (first, second) = if rail == 0 {
        (
            (0, load.surface1, load.surf1_offset),
            (1, load.surface2, load.surf2_offset),
        )
    } else {
        (
            (1, load.surface1, load.surf1_offset),
            (0, load.surface2, load.surf2_offset),
        )
    };
    for (rail_index, surface, offset) in [first, second] {
        if load.rail_index == rail_index && surface == Some(base) {
            off = offset;
            *num_parameters -= 2;
        }
    }
    Ok(off)
}

/// The shared body of both `LoadInitialValues` up to the rail-specific guess: the
/// guessed UV points, the normals of both base surfaces there, the plane of the
/// cross section and the solver domains (the second one extended).
struct InitialSetup {
    surface2_rail: usize,
    uv1: Point2,
    uv2: Point2,
    normal1: Vec3,
    normal2: Vec3,
    plane_orig: Point3,
    plane_normal: Vec3,
    domain1: Extent2d,
    domain2: Extent2d,
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn initial_setup(
    fs: &ConstantRadiusAssistedFs,
    k: &Kernel,
    brep: Option<&mut Brep>,
    rail_index: usize,
    surface1: &mut OffsetSurface,
    surface1_index: usize,
    intervals: &ExtentNd,
    guess_t: &[f64],
    op: &'static str,
) -> Result<InitialSetup> {
    if rail_index > 1 {
        return Err(SolidError::invalid_input(op, "rail index out of range"));
    }
    let other_rail = 1 - rail_index;
    let surface2 = fs.surface(other_rail)?;

    let uv1 = Point2::new(
        x_at(guess_t, surface1_index, op)?,
        x_at(guess_t, checked_add(surface1_index, 1, op)?, op)?,
    );
    let pnt = offset_evaluate_point(k, surface1, uv1)?;
    let base2 = surface2.base_surface();
    let mut domain1 = offset_natural_uv_domain(surface1);
    let mut domain2 = offset_natural_uv_domain(surface2);

    // Now find the UV guess on the other Rail's surface
    let mut fillet_edge: Option<(&Curve, f64)> = None;
    let uv2: Point2;

    // If we have edgeuses then use them to compute a point on the other surfaces
    // If not then drop the point to the other surface.
    let mut other_eu = fs.solver.edgeuses.get(other_rail).copied().flatten();
    let need_brep = || SolidError::invalid_input(op, "edge uses without their B-rep");
    if let Some(eu) = other_eu {
        let b = brep.as_deref().ok_or_else(need_brep)?;
        if edgeuse::uv_trim_curve(b, eu)?.0.is_none() {
            other_eu = None;
        }
    }

    // But: if we are rolling on a different face (e.g., doing a rollover),
    // then this is the wrong Edgeuse, and we don't have an appropriate one to use.
    if let Some(eu) = other_eu {
        let b = brep.as_deref().ok_or_else(need_brep)?;
        if let Some(f) = edgeuse::face(b, eu)? {
            let same = b.face(f)?.surface.is_some() && surface2.base_id == SurfaceId::Face(f);
            if !same {
                other_eu = None;
            }
        }
    }

    if let Some(eu) = other_eu {
        let b = brep.as_deref().ok_or_else(need_brep)?;
        let edge_id = edgeuse::edge(b, eu)?
            .ok_or_else(|| SolidError::invalid_input(op, "edge use without edge"))?;
        let edge = b.edge(edge_id)?;
        let curve = edge
            .curve
            .as_ref()
            .ok_or_else(|| SolidError::invalid_input(op, "edge without curve"))?;
        let ivl = edge.interval;
        let sols = curve.global_point_solve(
            k,
            &ivl,
            SolverOperation::Minimize,
            pnt,
            fs.solver.approx_tol_3d,
            SolutionRequested::Single,
            &GlobalPointSolveOptions::<ObjectHandle>::default(),
        )?;
        if sols.len() != 1 {
            return Err(SolidError::sm_err(op));
        }
        let mut t = sols
            .get(0)
            .ok_or_else(|| SolidError::sm_err(op))?
            .start
            .param(0)?;

        // If the curve is closed, and the solution is within tol of an
        // end point, snap the solution to whichever end (start or finish)
        // such that the direction that moves towards the interior of the curve
        // also moves generally towards the interior of the surface.
        if curve.is_closed(k, &ivl, 0.0)? {
            let p3 = curve.evaluate_point(k, t)?;
            let inward = offset_compute_inward_vector(k, surface1, uv1)?;
            let dropped = curve.drop_point(
                k,
                &ivl,
                p3,
                Some(inward),
                fs.solver.approx_tol_3d,
                None,
                SolverOperation::Minimize,
            )?;
            if dropped.success {
                t = dropped.parameter;
            }
        }

        // Now determine edgeuses, surfaces and orientations for current
        // iteration of the solver.
        let (uv_curve, _) = edgeuse::uv_trim_curve(b, eu)?;
        let uv_curve =
            uv_curve.ok_or_else(|| SolidError::invalid_input(op, "edge use without UV curve"))?;
        let p2d = uv_curve.evaluate_point(k, t)?;
        let mut guess2 = Point2::new(p2d.x, p2d.y);

        // Before we add it drop point to other surface to get a better guess
        if let Some((sol_uv, dist)) = offset_local_point_solve(k, surface2, &domain2, pnt, guess2)?
        {
            if dist < (fs.radius(0)?.abs() + fs.radius(1)?.abs()) {
                guess2 = sol_uv;
            }
        }
        uv2 = guess2;
        fillet_edge = Some((curve, t));
    } else {
        let sols = surface_global_point_solve(
            k,
            base2,
            face_owner(brep, surface2.base_id),
            &base2.natural_uv_domain_with_nurb(k)?,
            pnt,
            EFF_ZERO_SQ,
        )?;
        if sols.len() != 1 {
            return Err(SolidError::sm_err(op));
        }
        uv2 = sols
            .first()
            .copied()
            .ok_or_else(|| SolidError::sm_err(op))?;
    }

    let base1 = surface1.base_surface();
    let normal1 = base1
        .evaluate_normal(k, uv1, Side::Left, Side::Left)?
        .unitize()?;
    let normal2 = base2
        .evaluate_normal(k, uv2, Side::Left, Side::Left)?
        .unitize()?;

    let (plane_orig, plane_normal) = if let Some((curve, t)) = fillet_edge {
        let pv = curve.evaluate(k, t, 1, true, true)?;
        let p = pv.first().copied().ok_or_else(|| SolidError::sm_err(op))?;
        let d = pv.get(1).copied().ok_or_else(|| SolidError::sm_err(op))?;
        (p, d)
    } else {
        (pnt, normal1.cross(normal2))
    };

    let ivl_u = intervals
        .get(surface1_index)
        .ok_or_else(|| SolidError::invalid_input(op, "rIntervals index out of range"))?;
    let ivl_v = intervals
        .get(checked_add(surface1_index, 1, op)?)
        .ok_or_else(|| SolidError::invalid_input(op, "rIntervals index out of range"))?;
    let min = Point2::new(ivl_u.min(), ivl_v.min());
    let max = Point2::new(ivl_u.max(), ivl_v.max());
    // (status ignored upstream)
    ignore_status(domain1.set_min_max(min, max))?;
    let size2: Vec2 = fs.solver.surface_extension_factor * domain2.size();
    let min = Point2::new(domain2.min().x - size2.x, domain2.min().y - size2.y);
    let max = Point2::new(domain2.max().x + size2.x, domain2.max().y + size2.y);
    ignore_status(domain2.set_min_max(min, max))?;

    Ok(InitialSetup {
        surface2_rail: other_rail,
        uv1,
        uv2,
        normal1,
        normal2,
        plane_orig,
        plane_normal,
        domain1,
        domain2,
    })
}

/// The shared tail of both `LoadInitialValues`: the intervals and periodicities of
/// the second surface's UV point, and the final guesses.
#[allow(clippy::too_many_arguments)]
fn store_initial_values(
    surface1_index: usize,
    surface2_rail: usize,
    domain2: &Extent2d,
    uv1: Point2,
    uv2: Point2,
    intervals: &mut ExtentNd,
    periodicities: &mut Vec<bool>,
    guess_t: &mut Vec<f64>,
    op: &'static str,
) -> Result<InitialValues> {
    let n = guess_t.len();
    let range = || SolidError::invalid_input(op, "rIntervals index out of range");
    *intervals.get_mut(n).ok_or_else(range)? =
        Extent1d::from_min_max(domain2.min().x, domain2.max().x);
    *intervals
        .get_mut(checked_add(n, 1, op)?)
        .ok_or_else(range)? = Extent1d::from_min_max(domain2.min().y, domain2.max().y);

    periodicities.push(false);
    periodicities.push(false);

    let surface2_index = guess_t.len();
    set_f(guess_t, surface1_index, uv1.x, op)?;
    set_f(guess_t, checked_add(surface1_index, 1, op)?, uv1.y, op)?;

    guess_t.push(uv2.x);
    guess_t.push(uv2.y);

    Ok(InitialValues {
        surface2_rail,
        surface2_index,
    })
}

/// `pBase->DropVectors(uv, TRUE, TRUE, 1, &v, &delta)`: the UV step of `v`.
fn drop_vector(k: &Kernel, base: &Surface, uv: Point2, v: Vec3, op: &'static str) -> Result<Vec2> {
    drop_vectors_at(k, base, uv, Side::Left, Side::Left, &[v])?
        .first()
        .copied()
        .ok_or_else(|| SolidError::sm_err(op))
}

// ---- SmEiffelTowerFS ------------------------------------------------------------------

/// The "Eiffel tower" fillet solver (`SmEiffelTowerFS`): constant-radius filleting
/// between two surfaces with different offset radii, where the arc of the larger
/// radius sweeps `tower_angle`.
#[derive(Debug, Clone)]
pub struct EiffelTowerFs {
    /// The `SmConstantRadiusAssistedFS` base (radius 0 is `dFilletRadius1`, the
    /// offset of the input edge use's surface; radius 1 is `dFilletRadius2`).
    pub base: ConstantRadiusAssistedFs,
    /// `m_dTowerAngle`: secant angle for surface 2, in radians.
    pub tower_angle: f64,
}

impl EiffelTowerFs {
    /// `SmEiffelTowerFS(..., dFilletRadius1, dFilletRadius2, dTowerAngle, pEdgeuse)`
    /// over its already built base (see [`ConstantRadiusAssistedFs`]).
    #[must_use]
    pub const fn new(base: ConstantRadiusAssistedFs, tower_angle: f64) -> Self {
        Self { base, tower_angle }
    }

    /// `LoadInitialValues`: with `do_surf2_calcs`, the UV guess on the other rail's
    /// surface (from the other edge use's curve, or by dropping the point onto the
    /// other base surface), both guesses solved onto the cross-section plane and
    /// moved apart by the tower geometry, the second surface's intervals and
    /// periodicities appended, and the guesses stored. Without it nothing changes and
    /// the result is `None`.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when a point solve does not give exactly one solution or the plane
    /// solve finds nothing; [`SolidError::InvalidInput`] for out-of-range indices or
    /// a missing surface; those of the evaluations; [`SolidError::Unported`] (module
    /// notes).
    #[allow(clippy::too_many_arguments)]
    pub fn load_initial_values(
        &mut self,
        k: &Kernel,
        brep: Option<&mut Brep>,
        do_surf2_calcs: bool,
        rail_index: usize,
        surface1: &mut OffsetSurface,
        surface1_index: usize,
        intervals: &mut ExtentNd,
        periodicities: &mut Vec<bool>,
        guess_t: &mut Vec<f64>,
    ) -> Result<Option<InitialValues>> {
        const OP: &str = OP_EIFFEL_INIT;
        if !do_surf2_calcs {
            // Don't really need to do anything because both surfaces have already
            // been loaded.
            return Ok(None);
        }
        let s = initial_setup(
            &self.base,
            k,
            brep,
            rail_index,
            surface1,
            surface1_index,
            intervals,
            guess_t,
            OP,
        )?;
        let (mut uv1, mut uv2) = (s.uv1, s.uv2);

        let rad_diff = self.base.radius(0)? - self.base.radius(1)?;
        // Will first solve for the regular tangent case on the cross-section plane
        let scale_factor = 1.2;
        let mut tsect = TsectPnt::new();
        // (GuessPointOnPlaneSolve runs on *this: its evaluations extend the solver's
        // own offsets)
        let found = if rail_index == 0 {
            self.base.guess_point_on_plane_solve(
                k,
                s.plane_orig,
                s.plane_normal,
                &s.domain1,
                &s.domain2,
                uv1,
                uv2,
                &mut tsect,
            )?
        } else {
            self.base.guess_point_on_plane_solve(
                k,
                s.plane_orig,
                s.plane_normal,
                &s.domain2,
                &s.domain1,
                uv2,
                uv1,
                &mut tsect,
            )?
        };
        if !found {
            return Err(SolidError::sm_err(OP));
        }
        let fs = &self.base;
        let base1 = surface1.base_surface();
        let base2 = fs.surface(s.surface2_rail)?.base_surface();
        if rail_index == 0 {
            uv1 = tsect.uv_pos(SrfIndex::First);
            uv2 = tsect.uv_pos(SrfIndex::Second);
            // Then we want to move sUV1 along sNormal2 for a (3D-)distance
            // of (R1-R2)*sin(m_dTowerAngle) as our final guess sUV1
            // Similarily, move sUV2 along -sNormal1 for a distance of
            // (R1-R2)*cos(m_dTowerAngle) as our final guess sUV2
            let len1 = fs.orientation(1)? * scale_factor * rad_diff * self.tower_angle.sin();
            let v1 = len1 * s.normal2;
            let delta1 = drop_vector(k, base1, uv1, v1, OP)?;
            uv1 += delta1;
            let len2 = -fs.orientation(0)? * rad_diff * self.tower_angle.cos();
            let v2 = len2 * s.normal1;
            let delta2 = drop_vector(k, base2, uv2, v2, OP)?;
            uv2 += delta2;
        } else {
            uv1 = tsect.uv_pos(SrfIndex::Second);
            uv2 = tsect.uv_pos(SrfIndex::First);
            // Then we want to move sUV2 along sNormal1 for a (3D-)distance
            // of (R1-R2)*sin(m_dTowerAngle) as our final guess sUV2
            // Similarily, move sUV2 along -sNormal1 for a distance of
            // (R1-R2)*cos(m_dTowerAngle) as our final guess sUV2
            let len2 = fs.orientation(1)? * scale_factor * rad_diff * self.tower_angle.sin();
            let v2 = len2 * s.normal1;
            let delta2 = drop_vector(k, base2, uv2, v2, OP)?;
            uv2 += delta2;
            let len1 = -fs.orientation(0)? * rad_diff * self.tower_angle.cos();
            let v1 = len1 * s.normal2;
            let delta1 = drop_vector(k, base1, uv1, v1, OP)?;
            uv1 += delta1;
        }

        store_initial_values(
            surface1_index,
            s.surface2_rail,
            &s.domain2,
            uv1,
            uv2,
            intervals,
            periodicities,
            guess_t,
            OP,
        )
        .map(Some)
    }

    /// `LoadJacobian`: the three equations of the tower cross section at the current
    /// solution `crX = [u1 v1 u2 v2]` (`F`, `G` the offset surface points, `nF`, `nG`
    /// the base surface normals, `diffR` the radius difference): `(F-G).(F-G) -
    /// diffR^2`, `(nF x nG).(F-G)` and `nF/|nF| . (F-G)/|F-G| - cos(tower angle)`,
    /// written to `f[num_equations..]` and, when given, their partial derivatives to
    /// the Jacobian rows; `num_equations` advances by 3 and `num_parameters` by the
    /// UV points not shared with `load`'s surfaces. Leaves both offset surfaces at
    /// their fillet offsets. Returns whether all three values are below the scaled
    /// conversion tolerance.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a zero first derivative or base surface normal;
    /// [`SolidError::InvalidInput`] for out-of-range indices or a missing surface;
    /// those of the evaluations and [`Vec3::unitize`]; [`SolidError::Unported`]
    /// (module notes).
    #[allow(clippy::too_many_lines)]
    pub fn load_jacobian(
        &mut self,
        k: &Kernel,
        num_equations: &mut usize,
        num_parameters: &mut usize,
        load: &mut JacobianLoad<'_>,
        f: &mut [f64],
        jacobian: Option<&mut Matrix>,
    ) -> Result<bool> {
        const OP: &str = OP_EIFFEL_JAC;
        let fs = &mut self.base;
        let x = load.x;

        // Always need to reset offset surface offset distance prior to solving
        fs.reset_offset(0)?;
        fs.reset_offset(1)?;

        // get lSurf1Off index into crX array for lSurf1 parameters
        let base0 = fs.surface(0)?.base_id;
        let surf1_off = surface_offset(num_parameters, load, 0, base0, OP)?;

        // get BaseSurface1 F point values for given crX values
        let uv = Point2::new(
            x_at(x, surf1_off, OP)?,
            x_at(x, checked_add(surf1_off, 1, OP)?, OP)?,
        );
        fs.surface_mut(0)?.set_offset_distance(0.0);
        let [_, duf, dvf, duvf, duuf, dvvf] = offset_evaluate_2nd(k, fs.surface_mut(0)?, uv)?;

        // get lSurf2Off index into crX array for lSurf2 parameters
        let base1 = fs.surface(1)?.base_id;
        let surf2_off = surface_offset(num_parameters, load, 1, base1, OP)?;

        // get BaseSurface2 G point values for given crX values
        let uv2 = Point2::new(
            x_at(x, surf2_off, OP)?,
            x_at(x, checked_add(surf2_off, 1, OP)?, OP)?,
        );
        fs.surface_mut(1)?.set_offset_distance(0.0);
        let [_, dug, dvg, duvg, duug, dvvg] = offset_evaluate_2nd(k, fs.surface_mut(1)?, uv2)?;

        // check surface tangent magnitudes
        let duf_len_sq = duf.length_squared();
        let dvf_len_sq = dvf.length_squared();

        let dug_len_sq = dug.length_squared();
        let dvg_len_sq = dvg.length_squared();

        // failure: zero length tangent
        if duf_len_sq < EFF_ZERO_SQ
            || dvf_len_sq < EFF_ZERO_SQ
            || dug_len_sq < EFF_ZERO_SQ
            || dvg_len_sq < EFF_ZERO_SQ
        {
            return Err(SolidError::sm_err(OP));
        }

        // get baseSurface SurfaceNormals at current solution points
        let nf = duf.cross(dvf); // Normal on base surface #1
        let ng = dug.cross(dvg); // Normal on base surface #2

        // failure: zero length SurfaceNormal
        if nf.length_squared() < EFF_ZERO_SQ || ng.length_squared() < EFF_ZERO_SQ {
            return Err(SolidError::sm_err(OP));
        }

        // get derivatives of SurfaceNormals
        let nfdu = duf.derivative_of_cross_product(dvf, duuf, duvf);
        let nfdv = duf.derivative_of_cross_product(dvf, duvf, dvvf);
        let ngdu = dug.derivative_of_cross_product(dvg, duug, duvg);
        let ngdv = dug.derivative_of_cross_product(dvg, duvg, dvvg);

        // get fillet-curve tangent direction and its derivatives
        // from base surface surface normal values
        let n3 = nf.cross(ng);
        let n3duf = nfdu.cross(ng);
        let n3dvf = nfdv.cross(ng);
        let n3dug = nf.cross(ngdu);
        let n3dvg = nf.cross(ngdv);

        // normalize SurfaceNormal derivatives
        let orient0 = fs.orientation(0)?;
        let unit_nfdu = orient0 * nf.unitized_derivative(nfdu);
        let unit_nfdv = orient0 * nf.unitized_derivative(nfdv);
        let unit_nf = (orient0 * nf).unitize()?;

        // evaluate offsetSurface F and G point positions
        let d0 = fs.radius(0)? * orient0;
        fs.surface_mut(0)?.set_offset_distance(d0);
        let (pf, duf, dvf) = offset_evaluate_1st(k, fs.surface_mut(0)?, uv)?;
        let d1 = fs.radius(1)? * fs.orientation(1)?;
        fs.surface_mut(1)?.set_offset_distance(d1);
        let (pg, dug, dvg) = offset_evaluate_1st(k, fs.surface_mut(1)?, uv2)?;

        // set sV = distance between offsetSurface points
        let v = pf - pg;
        let unit_vduf = v.unitized_derivative(duf);
        let unit_vdvf = v.unitized_derivative(dvf);
        let unit_vdug = v.unitized_derivative(-dug);
        let unit_vdvg = v.unitized_derivative(-dvg);
        let unit_v = v.unitize()?;

        let diff_r = fs.radius(0)? - fs.radius(1)?;

        if let Some(j) = jacobian {
            let i = *num_equations;
            let c1 = checked_add(surf1_off, 1, OP)?;
            let c2 = checked_add(surf2_off, 1, OP)?;
            set_j(j, i, surf1_off, 2.0 * v.dot(duf), OP)?;
            set_j(j, i, c1, 2.0 * v.dot(dvf), OP)?;
            set_j(j, i, surf2_off, -2.0 * v.dot(dug), OP)?;
            set_j(j, i, c2, -2.0 * v.dot(dvg), OP)?;

            let i = checked_add(i, 1, OP)?; // Move to next equation;
            set_j(j, i, surf1_off, n3duf.dot(v) + n3.dot(duf), OP)?;
            set_j(j, i, c1, n3dvf.dot(v) + n3.dot(dvf), OP)?;
            set_j(j, i, surf2_off, n3dug.dot(v) - n3.dot(dug), OP)?;
            set_j(j, i, c2, n3dvg.dot(v) - n3.dot(dvg), OP)?;

            let i = checked_add(i, 1, OP)?;
            set_j(
                j,
                i,
                surf1_off,
                unit_nfdu.dot(unit_v) + unit_nf.dot(unit_vduf),
                OP,
            )?;
            set_j(j, i, c1, unit_nfdv.dot(unit_v) + unit_nf.dot(unit_vdvf), OP)?;
            set_j(j, i, surf2_off, unit_nf.dot(unit_vdug), OP)?;
            set_j(j, i, c2, unit_nf.dot(unit_vdvg), OP)?;
        }

        // Compute function values
        let initial = *num_equations;
        set_f(f, *num_equations, v.dot(v) - diff_r * diff_r, OP)?;
        *num_equations += 1;
        set_f(f, *num_equations, n3.dot(v), OP)?;
        *num_equations += 1;
        set_f(
            f,
            *num_equations,
            unit_nf.dot(unit_v) - self.tower_angle.cos(),
            OP,
        )?;
        *num_equations += 1;

        // See if we have converged
        let scaled_tol = self.base.solver.conversion_tol * (1.0 + pf.max_dimension());
        Ok(get_f(f, initial, OP)?.abs() < scaled_tol
            && get_f(f, initial + 1, OP)?.abs() < scaled_tol
            && get_f(f, initial + 2, OP)?.abs() < scaled_tol)
    }
}

// ---- SmEiffelTowerCrossSectionFSG -----------------------------------------------------

/// The surface generator of fillets with an "Eiffel tower" cross section
/// (`SmEiffelTowerCrossSectionFSG`): two circular arcs per cross section.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EiffelTowerCrossSectionFsg;

/// The cross section of `CreateSurface` at one rail parameter: `p1`, `p2` the fillet
/// point's surface positions, `s1`, `s2` the rail points.
fn cross_section(
    k: &Kernel,
    p1: Point3,
    p2: Point3,
    s1: Point3,
    s2: Point3,
    approx_tol_3d: f64,
) -> Result<BSplineCurve> {
    const OP: &str = OP_CREATE;
    if s1.distance(s2) < approx_tol_3d {
        // Create Point curve
        return BSplineCurve::create_point_curve(p1); /* parameterized from 0 to 1 */
    }

    let v = s2 - s1;
    let v1 = s1 - p1;
    let z_vec = v1.cross(v);
    let radius = v1.length();
    // (status ignored upstream)
    let z_vec = z_vec.unitize_or_z();

    // Project sP2 onto plane with normal sZVec
    let p2 = p2.project_point_to_plane(p1, z_vec);
    let v3 = p2 - p1;
    let angle = z_vec.ccw_angle_between(v1, v3)?;

    let v1 = v1.unitize()?;
    let y_vec = z_vec.cross(v1);
    let mut placement = Axis2Placement::default();
    placement.set_canonical(p1, v1, y_vec)?;
    let mut cross_section = BSplineCurve::create_circle_segment(
        3,
        &placement,
        radius,
        0.0,
        angle * 180.0 / PI,
        NurbCircleParam::Quadratic,
    )?;

    // Will try to make the second arc begin with
    // the end point of the first arc
    let ivl = cross_section.natural_interval();
    let p3 = cross_section.evaluate_point(ivl.max())?;
    let vec_p3s2 = s2 - p3;
    let mid_p3s2 = (s2 + p3) / 2.0;
    let bisector = z_vec.cross(vec_p3s2).unitize()?;

    let points =
        segment_segment_intersect(p3, p1, mid_p3s2, mid_p3s2 + bisector * radius, ANALYTIC_TOL)?;
    if points.len() != 1 {
        return Err(SolidError::sm_err(OP));
    }
    let p2 = points
        .first()
        .copied()
        .ok_or_else(|| SolidError::sm_err(OP))?;
    let v2 = s2 - p2;
    let v3 = p3 - p2;
    // Compute center of the arc #2
    let radius2 = v3.length();

    let angle2 = z_vec.ccw_angle_between(v3, v2)?;
    if angle2 > EFF_ZERO_SQRT {
        let y_vec = z_vec.cross(v3).unitize()?;
        let v3 = v3.unitize()?;
        let mut placement2 = Axis2Placement::default();
        placement2.set_canonical(p2, v3, y_vec)?;
        let mut arc2 = BSplineCurve::create_circle_segment(
            3,
            &placement2,
            radius2,
            0.0,
            angle2 * 180.0 / PI,
            NurbCircleParam::Quadratic,
        )?;
        // Join pCrossSection with pArc2
        cross_section.join_with(k, true, &mut arc2, false, None)?;
    }
    Ok(cross_section)
}

impl EiffelTowerCrossSectionFsg {
    /// `SmEiffelTowerCrossSectionFSG()`.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// `CreateSurface`: the fillet surface through one "Eiffel tower" cross section
    /// per unique knot of `rail1` (an arc about the first fillet point from the
    /// `rail1` point towards the second fillet point, joined with an arc ending at the
    /// `rail2` point; a point curve where the rails meet within `approx_tol_3d`): a
    /// ruled surface between the end cross sections when both rails are parallel
    /// lines, a surface of revolution of the first one when both rails are arcs,
    /// else a skin through all of them along both rails with extrusion
    /// cross-boundary derivative surfaces at its ends (the rails get a knot inserted
    /// in their first and last spans). The ruled and revolved surfaces are
    /// reparameterised to the rail's interval by `[0, 1]`.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when the rails' unique knot counts differ from each other or from the
    /// fillet point count, or the second arc's centre is not found; those of the
    /// curve and surface constructions; [`SolidError::Unported`] from the skinning.
    pub fn create_surface(
        &self,
        k: &Kernel,
        approx_tol_3d: f64,
        rail1: &mut BSplineCurve,
        rail2: &mut BSplineCurve,
        fillet_points: &[TsectPnt],
    ) -> Result<BSplineSurface> {
        const OP: &str = OP_CREATE;
        let need_g1_continuity = true;

        let knots = rail1.knots(None)?.0;
        let knots2 = rail2.knots(None)?.0;
        let totals_cross_sections = knots.len();
        if totals_cross_sections != knots2.len() || totals_cross_sections != fillet_points.len() {
            return Err(SolidError::sm_err(OP)); // Curves not compatible
        }

        let mut cross_sections: Vec<BSplineCurve> = Vec::new();
        let mut parameters_of_cross_sections: Vec<f64> = Vec::new();

        for (&t, tsp) in knots.iter().zip(fillet_points) {
            parameters_of_cross_sections.push(t);
            let p1 = tsp.srf_pos(SrfIndex::First);
            let p2 = tsp.srf_pos(SrfIndex::Second);
            let s1 = rail1.evaluate_point(t)?;
            let s2 = rail2.evaluate_point(t)?;
            cross_sections.push(cross_section(k, p1, p2, s1, s2, approx_tol_3d)?);
        }

        let mut sweep = false;
        if let Some((_, line_vector)) = rail1.is_line(ANALYTIC_SAMPLES, ANALYTIC_TOL)? {
            if let Some((_, line_vector2)) = rail2.is_line(ANALYTIC_SAMPLES, ANALYTIC_TOL)? {
                let angle = line_vector2.angle_between(line_vector)?;
                if angle < EFF_ZERO_SQRT {
                    sweep = true;
                }
            }
        }

        let first = cross_sections
            .first()
            .ok_or_else(|| SolidError::sm_err(OP))?;
        let last = cross_sections
            .last()
            .ok_or_else(|| SolidError::sm_err(OP))?;

        if sweep {
            // Create linear sweep surface
            let mut fillet_surface =
                BSplineSurface::create_ruled_surface(first, last, SurfParamType::U)?;
            let ivl = rail1.natural_interval();
            let domain =
                Extent2d::from_points(Point2::new(ivl.min(), 0.0), Point2::new(ivl.max(), 1.0));
            fillet_surface.reparameterize(&domain)?;
            return Ok(fillet_surface);
        }

        // See if we can create a surface of revolution
        let is_arc = |c: &BSplineCurve| {
            is_arc_base(
                k,
                c,
                |c, tol| c.is_degenerate(k, tol, None),
                ANALYTIC_SAMPLES,
                ANALYTIC_TOL,
            )
        };
        if let Some((ref_frame, _radius, _start_ang, end_ang)) = is_arc(rail1)? {
            if is_arc(rail2)?.is_some() {
                // Create surface of revolution
                let mut fillet_surface = BSplineSurface::create_surf_of_revolution(
                    k,
                    first,
                    ref_frame.origin(),
                    ref_frame.z_axis(),
                    end_ang,
                    false,
                    None,
                )?;
                let ivl = rail1.natural_interval();
                let domain =
                    Extent2d::from_points(Point2::new(ivl.min(), 0.0), Point2::new(ivl.max(), 1.0));
                fillet_surface.reparameterize(&domain)?;
                return Ok(fillet_surface);
            }
        }

        // Create skinned surface
        let mut deriv_surfs: [Option<SurfOfExtrusion>; 2] = [None, None];
        if need_g1_continuity {
            let ivl = rail1.natural_interval();
            for (i, slot) in deriv_surfs.iter_mut().enumerate() {
                // We need to provide cross-boundary derivative surface
                // for skinned-surface creation.
                // Create extrusion surface using end cross-section
                // curve and the tangent vector of centerline.
                // NOTE: Will work ONLY with constant-radius filleting.
                let (gen_curve, t) = if i == 1 {
                    (last, ivl.max())
                } else {
                    (first, ivl.min())
                };
                let pv = rail1.evaluate(t, 1, true, false)?;
                let tangent = pv.get(1).copied().ok_or_else(|| SolidError::sm_err(OP))?;
                let gen_curve = gen_curve.copy();
                let crv_ivl = gen_curve.natural_interval();
                let mut surf =
                    SurfOfExtrusion::create_canonical(k, Curve::from(gen_curve), tangent)?;
                let uv_min = Point2::new(crv_ivl.min(), 0.0);
                let uv_max = Point2::new(crv_ivl.max(), 1.0);
                // (status ignored upstream)
                ignore_status(
                    surf.adjust_step_uv_domain(k, &Extent2d::from_points(uv_min, uv_max)),
                )?;
                *slot = Some(surf);
            }
            // (statuses ignored upstream)
            let knots_local = rail1.knots(None).map(|r| r.0).unwrap_or_default();
            let knot = |i: usize| {
                knots_local
                    .get(i)
                    .copied()
                    .ok_or_else(|| SolidError::invalid_input(OP, "rail has fewer than 2 knots"))
            };
            let n = knots_local.len();
            if n < 2 {
                return Err(SolidError::invalid_input(OP, "rail has fewer than 2 knots"));
            }
            let start_knot_mid = (knot(0)? + knot(1)?) / 2.0;
            ignore_status(rail1.insert_one_knot(start_knot_mid, 1))?;
            ignore_status(rail2.insert_one_knot(start_knot_mid, 1))?;
            let end_knot_mid = (knot(n - 2)? + knot(n - 1)?) / 2.0;
            ignore_status(rail1.insert_one_knot(end_knot_mid, 1))?;
            ignore_status(rail2.insert_one_knot(end_knot_mid, 1))?;
        }
        let [d0, d1] = &deriv_surfs;
        let opts = SkinnedSurfaceOptions {
            synchronized: false,
            cross_section_surf_param: SurfParamType::U,
            approx_tol: approx_tol_3d,
            rail1: Some(rail1),
            rail2: Some(rail2),
            rail1_used_as_spine_curve: false,
            intersections_with_rails: Some(&parameters_of_cross_sections),
            deriv_surfs: Some([
                d0.as_ref().map(SurfOfExtrusion::base),
                d1.as_ref().map(SurfOfExtrusion::base),
            ]),
            degree: 3,
        };
        BSplineSurface::create_skinned_surface(k, &cross_sections, opts)
    }
}

// ---- SmSecantConstantRadiusFS ---------------------------------------------------------

/// The secant constant-radius fillet solver (`SmSecantConstantRadiusFS`): the rolling
/// ball touches one surface tangentially and cuts the input edge use's surface at
/// `secant_angle`.
#[derive(Debug, Clone)]
pub struct SecantConstantRadiusFs {
    /// The `SmConstantRadiusAssistedFS` base (both radii are `dFilletRadius`).
    pub base: ConstantRadiusAssistedFs,
    /// `m_dSecantAngle`: secant angle for surface 2, in radians.
    pub secant_angle: f64,
}

impl SecantConstantRadiusFs {
    /// `SmSecantConstantRadiusFS(..., dFilletRadius, dSecantAngle, pEdgeuse)` over its
    /// already built base (see [`ConstantRadiusAssistedFs`]).
    #[must_use]
    pub const fn new(base: ConstantRadiusAssistedFs, secant_angle: f64) -> Self {
        Self { base, secant_angle }
    }

    /// `LoadInitialValues`: as [`EiffelTowerFs::load_initial_values`], but after the
    /// plane solve the guess on the secant surface moves by twice the drop of
    /// `-R sin(secant angle)` times the other surface's normal.
    ///
    /// # Errors
    ///
    /// As [`EiffelTowerFs::load_initial_values`].
    #[allow(clippy::too_many_arguments)]
    pub fn load_initial_values(
        &mut self,
        k: &Kernel,
        brep: Option<&mut Brep>,
        do_surf2_calcs: bool,
        rail_index: usize,
        surface1: &mut OffsetSurface,
        surface1_index: usize,
        intervals: &mut ExtentNd,
        periodicities: &mut Vec<bool>,
        guess_t: &mut Vec<f64>,
    ) -> Result<Option<InitialValues>> {
        const OP: &str = OP_SECANT_INIT;
        if !do_surf2_calcs {
            // Don't really need to do anything because both surfaces have already
            // been loaded.
            return Ok(None);
        }
        let s = initial_setup(
            &self.base,
            k,
            brep,
            rail_index,
            surface1,
            surface1_index,
            intervals,
            guess_t,
            OP,
        )?;
        let (mut uv1, mut uv2) = (s.uv1, s.uv2);

        // Will first solve for the regular tangent case on the cross-section plane
        let mut tsect = TsectPnt::new();
        // (GuessPointOnPlaneSolve runs on *this: its evaluations extend the solver's
        // own offsets)
        let found = if rail_index == 0 {
            self.base.guess_point_on_plane_solve(
                k,
                s.plane_orig,
                s.plane_normal,
                &s.domain1,
                &s.domain2,
                uv1,
                uv2,
                &mut tsect,
            )?
        } else {
            self.base.guess_point_on_plane_solve(
                k,
                s.plane_orig,
                s.plane_normal,
                &s.domain2,
                &s.domain1,
                uv2,
                uv1,
                &mut tsect,
            )?
        };
        if !found {
            return Err(SolidError::sm_err(OP));
        }
        let fs = &self.base;
        let base1 = surface1.base_surface();
        let base2 = fs.surface(s.surface2_rail)?.base_surface();
        if rail_index == 0 {
            uv1 = tsect.uv_pos(SrfIndex::First);
            uv2 = tsect.uv_pos(SrfIndex::Second);
            // Now we have a point corresponding to sUV2 where the ball touches
            // surface2 tangentially. Then we want to move sUV2 along -sNormal1 for
            // a (3-d)distance of 2*Radius*sin(theta) as our final guess point
            let len = -fs.orientation(1)? * fs.radius(1)? * self.secant_angle.sin();
            let v = len * s.normal1;
            let delta = drop_vector(k, base2, uv2, v, OP)?;
            uv2 += delta * 2.0;
        } else {
            uv1 = tsect.uv_pos(SrfIndex::Second);
            uv2 = tsect.uv_pos(SrfIndex::First);
            // Now we have a point corresponding to sUV1 where the ball touches
            // surface1 tangentially. Then we want to move sUV1 along -sNormal2 for
            // a (3-d)distance of 2*Radius*sin(theta) as our final guess point
            let len = -fs.orientation(0)? * fs.radius(0)? * self.secant_angle.sin();
            let v = len * s.normal2;
            let delta = drop_vector(k, base1, uv1, v, OP)?;
            uv1 += delta * 2.0;
        }

        store_initial_values(
            surface1_index,
            s.surface2_rail,
            &s.domain2,
            uv1,
            uv2,
            intervals,
            periodicities,
            guess_t,
            OP,
        )
        .map(Some)
    }

    /// `LoadJacobian`: the three equations of the secant ball at the current solution
    /// `crX = [u1 v1 u2 v2]` (`V2 = F - G2` from the first offset surface to the second
    /// base surface, `N2` its unit normal, `N3 = N1 x N2`): `V2.V2 - R1 R2`, `V2.N2 -
    /// R2 cos(secant angle)` and `V2.N3`, written to `f[num_equations..]` and, when
    /// given, their partial derivatives to the Jacobian rows; `num_equations` advances
    /// by 3 and `num_parameters` as in [`EiffelTowerFs::load_jacobian`]. Leaves both
    /// offset surfaces at their fillet offsets. Returns whether every coordinate of
    /// `V2` is below the scaled conversion tolerance.
    ///
    /// # Errors
    ///
    /// As [`EiffelTowerFs::load_jacobian`].
    #[allow(clippy::too_many_lines)]
    pub fn load_jacobian(
        &mut self,
        k: &Kernel,
        num_equations: &mut usize,
        num_parameters: &mut usize,
        load: &mut JacobianLoad<'_>,
        f: &mut [f64],
        jacobian: Option<&mut Matrix>,
    ) -> Result<bool> {
        const OP: &str = OP_SECANT_JAC;
        let fs = &mut self.base;
        let x = load.x;

        // Always need to reset offsetSurface offset distances prior to solving
        fs.reset_offset(0)?;
        fs.reset_offset(1)?;

        // get lSurf1Off index into crX array for lSurf1 parameters
        let base0 = fs.surface(0)?.base_id;
        let surf1_off = surface_offset(num_parameters, load, 0, base0, OP)?;

        // get baseSurface1 F2 point values for given crX values
        let uv = Point2::new(
            x_at(x, surf1_off, OP)?,
            x_at(x, checked_add(surf1_off, 1, OP)?, OP)?,
        );
        fs.surface_mut(0)?.set_offset_distance(0.0);
        let (_, duf2, dvf2) = offset_evaluate_1st(k, fs.surface_mut(0)?, uv)?;

        // get baseSurface1 surface normal and check its size
        let n = duf2.cross(dvf2);
        if n.length_squared() < EFF_ZERO_SQ {
            return Err(SolidError::sm_err(OP));
        }
        let orient0 = fs.orientation(0)?;
        let n = n.unitize()? * orient0;

        // get offsetSurface1 F point values for given crX
        let d0 = fs.radius(0)? * orient0;
        fs.surface_mut(0)?.set_offset_distance(d0);
        let (pf, duf, dvf) = offset_evaluate_1st(k, fs.surface_mut(0)?, uv)?;

        // get lSurf2Off index into crX array for lSurf2 parameters
        let base1 = fs.surface(1)?.base_id;
        let surf2_off = surface_offset(num_parameters, load, 1, base1, OP)?;

        // get baseSurface2 G2 point values for given crX values
        let uv2 = Point2::new(
            x_at(x, surf2_off, OP)?,
            x_at(x, checked_add(surf2_off, 1, OP)?, OP)?,
        );
        fs.surface_mut(1)?.set_offset_distance(0.0);
        let (pg2, dug2, dvg2) = offset_evaluate_1st(k, fs.surface_mut(1)?, uv2)?;

        // get baseSurface surface normal value
        let n2 = dug2.cross(dvg2);
        if n2.length_squared() < EFF_ZERO_SQ {
            return Err(SolidError::sm_err(OP));
        }
        let orient1 = fs.orientation(1)?;
        let n2 = n2.unitize()? * orient1;

        // restore offsetSurface2 offset distance
        let d1 = fs.radius(1)? * orient1;
        fs.surface_mut(1)?.set_offset_distance(d1);

        // check surface tangent magnitudes
        let duf_len_sq = duf.length_squared();
        let dvf_len_sq = dvf.length_squared();

        let dug2_len_sq = dug2.length_squared();
        let dvg2_len_sq = dvg2.length_squared();

        // failure: zero length tangents
        if duf_len_sq < EFF_ZERO_SQ
            || dvf_len_sq < EFF_ZERO_SQ
            || dug2_len_sq < EFF_ZERO_SQ
            || dvg2_len_sq < EFF_ZERO_SQ
        {
            return Err(SolidError::sm_err(OP));
        }

        // set sDiff = distance between OffsetSurface1 and baseSurface2
        let diff = pf - pg2;

        // set sN3 = crossProduct(BaseSurface1Normal, BaseSurface2Normal)
        let n3 = n.cross(n2);

        if let Some(j) = jacobian {
            let i = *num_equations;
            let c1 = checked_add(surf1_off, 1, OP)?;
            let c2 = checked_add(surf2_off, 1, OP)?;
            set_j(j, i, surf1_off, 2.0 * diff.dot(duf), OP)?;
            set_j(j, i, c1, 2.0 * diff.dot(dvf), OP)?;
            set_j(j, i, surf2_off, -2.0 * diff.dot(dug2), OP)?;
            set_j(j, i, c2, -2.0 * diff.dot(dvg2), OP)?;

            let i = checked_add(i, 1, OP)?; // Move to next equation;
            set_j(j, i, surf1_off, duf.dot(n2), OP)?;
            set_j(j, i, c1, dvf.dot(n2), OP)?;
            set_j(j, i, surf2_off, -dug2.dot(n2), OP)?;
            set_j(j, i, c2, -dvg2.dot(n2), OP)?;

            let i = checked_add(i, 1, OP)?;
            set_j(j, i, surf1_off, duf.dot(n3), OP)?;
            set_j(j, i, c1, dvf.dot(n3), OP)?;
            set_j(j, i, surf2_off, -dug2.dot(n3), OP)?;
            set_j(j, i, c2, -dvg2.dot(n3), OP)?;
        }

        // Compute function values
        let r0 = fs.radius(0)?;
        let r1 = fs.radius(1)?;
        set_f(f, *num_equations, diff.dot(diff) - r0 * r1, OP)?;
        *num_equations += 1;
        set_f(
            f,
            *num_equations,
            diff.dot(n2) - r1 * self.secant_angle.cos(),
            OP,
        )?;
        *num_equations += 1;
        set_f(f, *num_equations, diff.dot(n3), OP)?;
        *num_equations += 1;

        // See if we have converged
        let scaled_tol = self.base.solver.conversion_tol * (1.0 + pf.max_dimension());
        Ok(diff.x.abs() < scaled_tol && diff.y.abs() < scaled_tol && diff.z.abs() < scaled_tol)
    }
}

// ---- virtual dispatch -----------------------------------------------------------------

macro_rules! fillet_solver_ops {
    ($t:ty) => {
        impl FilletSolverOps for $t {
            fn solver(&self) -> &FilletSolver {
                &self.base.solver
            }

            fn solver_mut(&mut self) -> &mut FilletSolver {
                &mut self.base.solver
            }

            /// `SmConstantRadiusFS::GetJacobianSize` (inherited).
            fn jacobian_size(&self) -> Result<usize> {
                Ok(ConstantRadiusAssistedFs::jacobian_size(&self.base))
            }

            /// `SmConstantRadiusFS::SetupOffsetValues` (inherited).
            fn setup_offset_values(
                &mut self,
                tsect: &TsectPnt,
                offset_dist: Option<&mut f64>,
            ) -> Result<()> {
                constant_radius_setup_offset_values(&mut self.base, tsect, offset_dist)
            }

            /// `SmConstantRadiusFS::OffsetRadiiCanChange` (inherited).
            fn offset_radii_can_change(&self) -> Result<bool> {
                Ok(ConstantRadiusAssistedFs::offset_radii_can_change(
                    &self.base,
                ))
            }

            /// `SmConstantRadiusAssistedFS::GetType` (inherited, `SM_COMMON`).
            fn type_code(&self) -> Result<i64> {
                Ok(CONSTANT_RADIUS_ASSISTED_FS_TYPE)
            }

            /// `SmConstantRadiusAssistedFS::IsKindOf` (inherited).
            fn is_kind_of(&self, t: i64) -> Result<bool> {
                Ok(
                    crate::blend::fillet_standard_solver::p2::constant_radius_assisted_is_kind_of(
                        t,
                    ),
                )
            }

            /// `SmConstantRadiusAssistedFS::GetSolverType` (inherited):
            /// `SM_FS_CONST_RADIUS_ASSISTED` (`SmFilletStandardSolver.h:112`).
            fn solver_type(&self) -> Result<FilletSolverType> {
                Ok(FilletSolverType::ConstRadiusAssisted)
            }

            fn load_jacobian(
                &mut self,
                k: &Kernel,
                num_equations: &mut usize,
                num_parameters: &mut usize,
                load: &mut JacobianLoad<'_>,
                f: &mut [f64],
                jacobian: Option<&mut Matrix>,
            ) -> Result<bool> {
                <$t>::load_jacobian(self, k, num_equations, num_parameters, load, f, jacobian)
            }

            fn load_initial_values(
                &mut self,
                k: &Kernel,
                mut brep: Option<&mut Brep>,
                do_surf2_calcs: bool,
                rail_index: usize,
                surface1: &mut OffsetSurface,
                surface1_index: usize,
                intervals: &mut ExtentNd,
                periodicities: &mut Vec<bool>,
                guess_t: &mut Vec<f64>,
            ) -> Result<Option<InitialValues>> {
                <$t>::load_initial_values(
                    self,
                    k,
                    brep.as_deref_mut(),
                    do_surf2_calcs,
                    rail_index,
                    surface1,
                    surface1_index,
                    intervals,
                    periodicities,
                    guess_t,
                )
            }

            /// `SmConstantRadiusAssistedFS::PointOnPlaneSolve` (inherited, unit
            /// `blend.fillet_standard_solver_p2`): the override `StepSolve` and the
            /// other callers of the virtual dispatch to.
            fn point_on_plane_solve(
                &mut self,
                k: &Kernel,
                mut brep: Option<&mut Brep>,
                plane_origin: Point3,
                plane_normal: Vec3,
                uv_domain1: &Extent2d,
                uv_domain2: &Extent2d,
                uv1: Point2,
                uv2: Point2,
                point: &mut TsectPnt,
            ) -> Result<bool> {
                crate::blend::fillet_standard_solver::p2::point_on_plane_solve(
                    self,
                    k,
                    brep.as_deref_mut(),
                    plane_origin,
                    plane_normal,
                    uv_domain1,
                    uv_domain2,
                    uv1,
                    uv2,
                    point,
                )
            }
        }
    };
}

fillet_solver_ops!(EiffelTowerFs);
fillet_solver_ops!(SecantConstantRadiusFs);

/// The `SmFilletSolver` interface `SmFilletIntersector` uses
/// ([`crate::blend::fillet_intersector::FilletSolverOps`]) for a solver derived from
/// `SmConstantRadiusAssistedFS`: everything is inherited (the base's implementation),
/// except that `ComputeSurfaceValues` and `PointOnPlaneSolve` dispatch their virtual
/// calls (`SetupOffsetValues`; `GetJacobianSize`, `LoadInitialValues`, `LoadJacobian`)
/// on the derived class.
macro_rules! fillet_intersector_ops {
    ($t:ty) => {
        impl crate::blend::fillet_intersector::FilletSolverOps for $t {
            type FilletGeom = crate::blend::fillet_geom::p1::FilletGeomId;

            /// `SmFilletSolver::ComputePointValues` (non-virtual, unit
            /// `blend.fillet_solver_p4`) on this class's virtuals.
            fn compute_point_values(
                &mut self,
                k: &Kernel,
                uv_values: [Point2; 2],
                uv_domain1: &Extent2d,
                uv_domain2: &Extent2d,
                curve_trace_direction: f64,
                point: &mut TsectPnt,
                previous: Option<&TsectPnt>,
            ) -> Result<()> {
                FilletSolver::compute_point_values(
                    self,
                    k,
                    uv_values,
                    uv_domain1,
                    uv_domain2,
                    curve_trace_direction,
                    point,
                    previous,
                )
            }

            fn compute_surface_values(
                &mut self,
                k: &Kernel,
                uv_values: [Point2; 2],
                point: &mut TsectPnt,
            ) -> Result<()> {
                FilletSolver::compute_surface_values(k, self, uv_values, point)
            }

            fn compute_uv_vectors(&mut self, k: &Kernel, point: &mut TsectPnt) -> Result<()> {
                IntersectorOps::compute_uv_vectors(&mut self.base, k, point)
            }

            /// `SmConstantRadiusAssistedFS::PointOnPlaneSolve` (inherited, unit
            /// `blend.fillet_standard_solver_p2`) on this class.
            fn point_on_plane_solve(
                &mut self,
                k: &Kernel,
                plane_origin: Point3,
                plane_normal: Vec3,
                uv_domain1: &Extent2d,
                uv_domain2: &Extent2d,
                uv1: Point2,
                uv2: Point2,
                point: &mut TsectPnt,
            ) -> Result<bool> {
                crate::blend::fillet_standard_solver::p2::point_on_plane_solve(
                    self,
                    k,
                    None,
                    plane_origin,
                    plane_normal,
                    uv_domain1,
                    uv_domain2,
                    uv1,
                    uv2,
                    point,
                )
            }

            /// `SmFilletSolver::StepSolve` (non-virtual, unit `blend.fillet_solver_p3`):
            /// it dispatches on this class's virtuals, so the derived solver is passed.
            fn step_solve(
                &mut self,
                k: &Kernel,
                point: &TsectPnt,
                step_size: &mut f64,
                uv_domain1: &Extent2d,
                uv_domain2: &Extent2d,
                flags: &mut crate::blend::fillet_intersector::StepSolve,
                next: &mut TsectPnt,
            ) -> Result<()> {
                let mut out = crate::blend::fillet_solver::p3::StepSolveOutcome::default();
                let r = FilletSolver::step_solve(
                    self, k, None, point, step_size, uv_domain1, uv_domain2, next, &mut out,
                );
                flags.found_answer = out.found_good_point;
                flags.clipped = out.clipped;
                flags.boundary_hit = out.boundary_hit;
                r
            }

            fn set_surfaces_to_zero_offset(&mut self) -> Result<()> {
                IntersectorOps::set_surfaces_to_zero_offset(&mut self.base)
            }

            fn reload_surface_offsets(&mut self) -> Result<()> {
                IntersectorOps::reload_surface_offsets(&mut self.base)
            }

            fn rail_surface_evaluate_1st_derivatives(
                &mut self,
                k: &Kernel,
                rail: usize,
                uv: Point2,
            ) -> Result<(Point3, Vec3, Vec3)> {
                IntersectorOps::rail_surface_evaluate_1st_derivatives(&mut self.base, k, rail, uv)
            }

            fn set_status(
                &mut self,
                status: crate::blend::fillet_intersector::FilStatus,
            ) -> Result<()> {
                IntersectorOps::set_status(&mut self.base, status)
            }

            fn last_fillet_geom(&self) -> Result<Self::FilletGeom> {
                IntersectorOps::last_fillet_geom(&self.base)
            }

            fn self_intersection_handler(
                &self,
            ) -> Result<Option<crate::blend::fillet_intersector::HandlingAlgorithmType>> {
                IntersectorOps::self_intersection_handler(&self.base)
            }

            fn smooth_out_intersection(
                &mut self,
                k: &Kernel,
                pseudo: &mut FilletBrep,
                geom: &Self::FilletGeom,
                curves_3d: &mut Vec<BSplineCurve>,
                uv_curves1: &mut Vec<BSplineCurve>,
                uv_curves2: &mut Vec<BSplineCurve>,
            ) -> Result<()> {
                IntersectorOps::smooth_out_intersection(
                    &mut self.base,
                    k,
                    pseudo,
                    geom,
                    curves_3d,
                    uv_curves1,
                    uv_curves2,
                )
            }

            fn has_surface_generator(&self) -> Result<bool> {
                IntersectorOps::has_surface_generator(&self.base)
            }

            fn create_fillet_surface(
                &mut self,
                k: &Kernel,
                center_line: &mut BSplineCurve,
                curve1: &mut BSplineCurve,
                curve2: &mut BSplineCurve,
                points: &[TsectPnt],
                uv1: &mut BSplineCurve,
                uv2: &mut BSplineCurve,
            ) -> Result<Option<BSplineSurface>> {
                IntersectorOps::create_fillet_surface(
                    &mut self.base,
                    k,
                    center_line,
                    curve1,
                    curve2,
                    points,
                    uv1,
                    uv2,
                )
            }

            fn set_fillet_surface(
                &mut self,
                pseudo: &mut crate::blend::fillet_geom::p1::FilletBrep,
                geom: &Self::FilletGeom,
                surface: Option<BSplineSurface>,
            ) -> Result<()> {
                IntersectorOps::set_fillet_surface(&mut self.base, pseudo, geom, surface)
            }

            fn set_center_line_curve(
                &mut self,
                pseudo: &mut crate::blend::fillet_geom::p1::FilletBrep,
                geom: &Self::FilletGeom,
                center_line: BSplineCurve,
            ) -> Result<()> {
                IntersectorOps::set_center_line_curve(&mut self.base, pseudo, geom, center_line)
            }

            fn set_rail_curve(
                &mut self,
                pseudo: &mut crate::blend::fillet_geom::p1::FilletBrep,
                geom: &Self::FilletGeom,
                rail: usize,
                curve: BSplineCurve,
            ) -> Result<()> {
                IntersectorOps::set_rail_curve(&mut self.base, pseudo, geom, rail, curve)
            }

            fn set_rail_mate_uv_curve(
                &mut self,
                pseudo: &mut crate::blend::fillet_geom::p1::FilletBrep,
                geom: &Self::FilletGeom,
                rail: usize,
                uv_curve: BSplineCurve,
            ) -> Result<()> {
                IntersectorOps::set_rail_mate_uv_curve(&mut self.base, pseudo, geom, rail, uv_curve)
            }
        }
    };
}

fillet_intersector_ops!(EiffelTowerFs);
fillet_intersector_ops!(SecantConstantRadiusFs);

/// `SmConstantRadiusFS::SetupOffsetValues` with the optional `pOffsetDist` out-param
/// (`blend.fillet_standard_solver_p1`'s
/// [`ConstantRadiusAssistedFs::setup_offset_values`] returns it).
fn constant_radius_setup_offset_values(
    fs: &mut ConstantRadiusAssistedFs,
    tsect: &TsectPnt,
    offset_dist: Option<&mut f64>,
) -> Result<()> {
    let d = ConstantRadiusAssistedFs::setup_offset_values(fs, tsect)?;
    if let Some(out) = offset_dist {
        *out = d;
    }
    Ok(())
}

/// The virtuals of `SmConstantRadiusAssistedFS` itself (the class `blend.fillet_intersector`
/// runs `SmFilletSolver::ComputeSurfaceValues` on): the `SmConstantRadiusFS` overrides
/// are `blend.fillet_standard_solver_p1`'s, `IsKindOf` and `LoadInitialValues`
/// `_p2`'s, so none falls through to the base.
impl FilletSolverOps for ConstantRadiusAssistedFs {
    fn solver(&self) -> &FilletSolver {
        &self.solver
    }

    fn solver_mut(&mut self) -> &mut FilletSolver {
        &mut self.solver
    }

    fn rail_edgeuse_intersect_on_geometry(
        &mut self,
        k: &Kernel,
        geometry: &mut crate::blend::fillet_geom::p1::FilletGeom,
        target: &mut Brep,
        rail_index: usize,
        edgeuse: EdgeuseId,
        guess: f64,
        tsect: &mut TsectPnt,
    ) -> Result<Option<f64>> {
        crate::blend::fillet_solver::surfaces::rail_edgeuse_intersect(
            self, k, geometry, target, rail_index, edgeuse, guess, tsect,
        )
    }

    /// `SmConstantRadiusFS::GetJacobianSize`.
    fn jacobian_size(&self) -> Result<usize> {
        Ok(ConstantRadiusAssistedFs::jacobian_size(self))
    }

    /// `SmConstantRadiusFS::SetupOffsetValues`.
    fn setup_offset_values(
        &mut self,
        tsect: &TsectPnt,
        offset_dist: Option<&mut f64>,
    ) -> Result<()> {
        constant_radius_setup_offset_values(self, tsect, offset_dist)
    }

    /// `SmConstantRadiusFS::OffsetRadiiCanChange`.
    fn offset_radii_can_change(&self) -> Result<bool> {
        Ok(ConstantRadiusAssistedFs::offset_radii_can_change(self))
    }

    /// `SmConstantRadiusAssistedFS::GetType` (`SM_COMMON`); `SmConstantRadiusFS`'s for a
    /// [`plain`](Self::plain) solver.
    fn type_code(&self) -> Result<i64> {
        if self.plain {
            return Ok(crate::blend::fillet_standard_solver::p1::CONSTANT_RADIUS_FS_TYPE);
        }
        Ok(CONSTANT_RADIUS_ASSISTED_FS_TYPE)
    }

    /// `SmConstantRadiusAssistedFS::GetSolverType`: `SM_FS_CONST_RADIUS_ASSISTED`
    /// (`SmFilletStandardSolver.h:112`; `StepSolve` branches on it). The fillet
    /// intersector's shim builds a real `SmConstantRadiusAssistedFS`; a harness that
    /// models a plain `SmConstantRadiusFS` with this type overrides it (as
    /// `blend.fillet_solver_p4`'s `ShimSolver` does). It returned `SM_FS_CONST_RADIUS`
    /// until `blend.fillet_solver_p4` wired `ComputePointValues`, which made the
    /// intersector's `ComputeNextPoint` cases reach `StepSolve` and showed the gap.
    /// A [`plain`](Self::plain) solver is `SmConstantRadiusFS`: `SM_FS_CONST_RADIUS`.
    fn solver_type(&self) -> Result<FilletSolverType> {
        if self.plain {
            return Ok(FilletSolverType::ConstRadius);
        }
        Ok(FilletSolverType::ConstRadiusAssisted)
    }

    /// `SmConstantRadiusAssistedFS::IsKindOf` (unit `blend.fillet_standard_solver_p2`);
    /// `SmConstantRadiusFS::IsKindOf` for a [`plain`](Self::plain) solver.
    fn is_kind_of(&self, t: i64) -> Result<bool> {
        if self.plain {
            return Ok(crate::blend::fillet_standard_solver::p1::constant_radius_is_kind_of(t));
        }
        Ok(crate::blend::fillet_standard_solver::p2::constant_radius_assisted_is_kind_of(t))
    }

    /// `SmConstantRadiusFS::LoadJacobian` ([`ConstantRadiusAssistedFs::load_jacobian`]);
    /// the in/out `rlSurf1Offset`, `rlSurf2Offset` are written back into `load`.
    fn load_jacobian(
        &mut self,
        k: &Kernel,
        num_equations: &mut usize,
        num_parameters: &mut usize,
        load: &mut JacobianLoad<'_>,
        f: &mut [f64],
        jacobian: Option<&mut Matrix>,
    ) -> Result<bool> {
        let mut offsets = [load.surf1_offset, load.surf2_offset];
        let found = ConstantRadiusAssistedFs::load_jacobian(
            self,
            k,
            num_equations,
            num_parameters,
            load.rail_index,
            [load.surface1, load.surface2],
            &mut offsets,
            load.x,
            f,
            jacobian,
        );
        [load.surf1_offset, load.surf2_offset] = offsets;
        found
    }

    /// `SmConstantRadiusFS::LoadInitialValues` (unit `blend.fillet_standard_solver_p2`:
    /// [`ConstantRadiusAssistedFs::load_initial_values`]).
    fn load_initial_values(
        &mut self,
        k: &Kernel,
        brep: Option<&mut Brep>,
        do_surf2_calcs: bool,
        rail_index: usize,
        surface1: &mut OffsetSurface,
        surface1_index: usize,
        intervals: &mut ExtentNd,
        periodicities: &mut Vec<bool>,
        guess_t: &mut Vec<f64>,
    ) -> Result<Option<InitialValues>> {
        ConstantRadiusAssistedFs::load_initial_values(
            self,
            k,
            brep,
            do_surf2_calcs,
            rail_index,
            surface1,
            surface1_index,
            intervals,
            periodicities,
            guess_t,
        )
    }

    /// `SmConstantRadiusAssistedFS::PointOnPlaneSolve` (unit
    /// `blend.fillet_standard_solver_p2`): the override `StepSolve` and the other
    /// callers of the virtual dispatch to (the base `SmFilletSolver` version solves
    /// a different system, and is the one a [`plain`](Self::plain) solver inherits).
    fn point_on_plane_solve(
        &mut self,
        k: &Kernel,
        mut brep: Option<&mut Brep>,
        plane_origin: Point3,
        plane_normal: Vec3,
        uv_domain1: &Extent2d,
        uv_domain2: &Extent2d,
        uv1: Point2,
        uv2: Point2,
        point: &mut TsectPnt,
    ) -> Result<bool> {
        if self.plain {
            return FilletSolver::point_on_plane_solve(
                self,
                k,
                brep.as_deref_mut(),
                plane_origin,
                plane_normal,
                uv_domain1,
                uv_domain2,
                uv1,
                uv2,
                point,
            );
        }
        crate::blend::fillet_standard_solver::p2::point_on_plane_solve(
            self,
            k,
            brep,
            plane_origin,
            plane_normal,
            uv_domain1,
            uv_domain2,
            uv1,
            uv2,
            point,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nurbs::b_spline_curve::p1::NurbCircleParam;

    fn line(a: Point3, b: Point3) -> BSplineCurve {
        BSplineCurve::create_line_segment(3, a, b).expect("line")
    }

    /// A plane through `origin` spanned by `x` and `y`, domain `[0,1]^2`.
    fn plane(origin: Point3, x: Vec3, y: Vec3) -> Surface {
        Surface::from(crate::surface::plane::Plane::new(
            origin,
            x,
            y,
            Vec2::new(1.0, 1.0),
            &Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0),
        ))
    }

    fn rail(k: &Kernel, d: f64, base: Surface, id: u32) -> OffsetSurface {
        OffsetSurface::new(k, d, base, SurfaceId::Free(id), 0).expect("offset")
    }

    fn tsect(p1: Point3, p2: Point3) -> TsectPnt {
        let mut t = TsectPnt::new();
        *t.srf_pos_mut(SrfIndex::First) = p1;
        *t.srf_pos_mut(SrfIndex::Second) = p2;
        t
    }

    fn assisted_parallel_planes() -> ConstantRadiusAssistedFs {
        let k = Kernel::default();
        let x = Vec3::new(1.0, 0.0, 0.0);
        let y = Vec3::new(0.0, 1.0, 0.0);
        ConstantRadiusAssistedFs::from_parts(
            1e-6,
            0.5,
            0.03,
            [1.0, 2.0],
            [
                Some(rail(&k, 7.0, plane(Vec3::new(0.0, 0.0, 0.0), x, y), 11)),
                Some(rail(&k, 9.0, plane(Vec3::new(0.0, 0.0, 1.0), x, y), 29)),
            ],
            [1.0, -1.0],
            [None, None],
        )
    }

    #[test]
    fn assisted_virtuals_use_ported_constant_radius_methods() {
        let mut fs = assisted_parallel_planes();
        let mut distance = -17.0;
        let ops: &mut dyn FilletSolverOps = &mut fs;
        assert_eq!(ops.jacobian_size().expect("size"), 4);
        assert!(!ops.offset_radii_can_change().expect("fixed radii"));
        assert_eq!(
            ops.type_code().expect("type"),
            CONSTANT_RADIUS_ASSISTED_FS_TYPE
        );
        ops.setup_offset_values(&TsectPnt::new(), Some(&mut distance))
            .expect("offsets");
        assert_eq!(distance, 1.0);
        ops.setup_offset_values(&TsectPnt::new(), None)
            .expect("optional distance");
        assert!(!ops.is_kind_of(0).expect("unrelated type"));
        assert!(ops
            .is_kind_of(CONSTANT_RADIUS_ASSISTED_FS_TYPE)
            .expect("assisted type"));
        assert_eq!(fs.surface(0).expect("first rail").offset_distance(), 1.0);
        assert_eq!(fs.surface(1).expect("second rail").offset_distance(), -2.0);
    }

    #[test]
    fn assisted_virtual_jacobian_writes_rail_offsets_values_and_matrix() {
        let k = Kernel::default();
        for rail_index in [0, 1] {
            let mut fs = assisted_parallel_planes();
            let x = [99.0, 99.0, 0.25, 0.75, 0.5, 0.125];
            let mut load = JacobianLoad {
                rail_index,
                surface1: None,
                surf1_offset: 71,
                surface2: None,
                surf2_offset: 83,
                x: &x,
            };
            let (mut ne, mut np) = (1, 2);
            let mut f = [17.0; 5];
            let mut matrix = Matrix::new(4, 6);
            let ops: &mut dyn FilletSolverOps = &mut fs;
            assert!(!ops
                .load_jacobian(&k, &mut ne, &mut np, &mut load, &mut f, Some(&mut matrix))
                .expect("jacobian"));
            assert_eq!((ne, np), (4, 6));
            assert_eq!(
                [load.surf1_offset, load.surf2_offset],
                if rail_index == 0 { [2, 4] } else { [4, 2] }
            );
            assert_eq!(f, [17.0, -0.25, 0.625, 2.0, 17.0]);
            for (row, expected) in [
                [1.0, 0.0, -1.0, 0.0],
                [0.0, 1.0, 0.0, -1.0],
                [0.0, 0.0, 0.0, 0.0],
            ]
            .iter()
            .enumerate()
            {
                for (column, value) in expected.iter().enumerate() {
                    assert_eq!(matrix.at(row + 1, column + 2).expect("entry"), *value);
                }
            }
        }
    }

    #[test]
    fn assisted_virtual_jacobian_preserves_offsets_after_later_error() {
        let k = Kernel::default();
        let mut fs = assisted_parallel_planes();
        let mut load = JacobianLoad {
            rail_index: 1,
            surface1: None,
            surf1_offset: 71,
            surface2: None,
            surf2_offset: 83,
            x: &[0.25, 0.75],
        };
        let (mut ne, mut np) = (0, 0);
        let mut f = [17.0; 3];
        let ops: &mut dyn FilletSolverOps = &mut fs;
        assert!(matches!(
            ops.load_jacobian(&k, &mut ne, &mut np, &mut load, &mut f, None),
            Err(SolidError::InvalidInput { .. })
        ));
        assert_eq!((ne, np), (0, 4));
        assert_eq!([load.surf1_offset, load.surf2_offset], [2, 0]);
        assert_eq!(f, [17.0; 3]);
    }

    #[test]
    fn cross_section_point_curve_when_rails_meet() {
        let k = &Kernel::default();
        let p = Point3::new(1.0, 2.0, 3.0);
        let c = cross_section(k, p, p, p, p, 1e-6).expect("point curve");
        let q = c.evaluate_point(0.5).expect("eval");
        assert!(q.distance(p) < 1e-12);
    }

    #[test]
    fn cross_section_tower_arcs_end_on_rails() {
        let k = &Kernel::default();
        // R1 = 2 swept 60 degrees, then R2 = 1 swept 30 more degrees
        let h = 3.0f64.sqrt() / 2.0;
        let s1 = Point3::new(0.0, 0.0, 0.0);
        let p1 = Point3::new(0.0, 0.0, 2.0);
        let p2 = Point3::new(0.0, h, 1.5);
        let s2 = Point3::new(0.0, h + 1.0, 1.5);
        let c = cross_section(k, p1, p2, s1, s2, 1e-6).expect("tower");
        let ivl = c.natural_interval();
        let a = c.evaluate_point(ivl.min()).expect("eval");
        let b = c.evaluate_point(ivl.max()).expect("eval");
        assert!(a.distance(s1) < 1e-9, "{a:?}");
        assert!(b.distance(s2) < 1e-6, "{b:?}");
    }

    #[test]
    fn create_surface_rejects_mismatched_points() {
        let k = Kernel::default();
        let mut r1 = line(Point3::new(0.0, 1.0, 0.0), Point3::new(5.0, 1.0, 0.0));
        let mut r2 = line(Point3::new(0.0, 0.0, 1.0), Point3::new(5.0, 0.0, 1.0));
        let pts = [tsect(
            Point3::new(0.0, 1.0, 1.0),
            Point3::new(0.0, 1.0, 1.0),
        )];
        let r = EiffelTowerCrossSectionFsg::new().create_surface(&k, 1e-6, &mut r1, &mut r2, &pts);
        assert!(r.is_err());
    }

    #[test]
    fn create_surface_ruled_for_parallel_lines() {
        let k = Kernel::default();
        let mut r1 = line(Point3::new(0.0, 1.0, 0.0), Point3::new(5.0, 1.0, 0.0));
        let mut r2 = line(Point3::new(0.0, 0.0, 1.0), Point3::new(5.0, 0.0, 1.0));
        let pts = [
            tsect(Point3::new(0.0, 1.0, 1.0), Point3::new(0.0, 1.0, 1.0)),
            tsect(Point3::new(5.0, 1.0, 1.0), Point3::new(5.0, 1.0, 1.0)),
        ];
        let s = EiffelTowerCrossSectionFsg::new()
            .create_surface(&k, 1e-6, &mut r1, &mut r2, &pts)
            .expect("surface");
        let d = s.natural_uv_domain();
        assert_eq!((d.min().x, d.max().x), (0.0, 1.0));
        assert_eq!((d.min().y, d.max().y), (0.0, 1.0));
    }

    #[test]
    fn rail_surfaces_take_base_singularities_then_evaluate() {
        let k = Kernel::default();
        let o = Vec3::new(0.0, 0.0, 0.0);
        let (x, y, z) = (
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(0.0, 0.0, 1.0),
        );
        // upstream's default lSingularities asks the base: a plane has none
        let rail0 = OffsetSurface::new(
            &k,
            2.0,
            plane(o, x, y),
            SurfaceId::Free(0),
            offset_surface::SS_UNKNOWN,
        )
        .unwrap();
        assert_eq!(
            rail0.surface.singularities(),
            crate::geom::extent2d::SS_NONE
        );
        let base = ConstantRadiusAssistedFs::from_parts(
            1e-6,
            0.5,
            0.03,
            [2.0, 1.0],
            [
                Some(rail(&k, 2.0, plane(o, x, y), 0)),
                Some(rail(&k, 1.0, plane(o, y, z), 1)),
            ],
            [1.0, 1.0],
            [None, None],
        );
        let mut fs = EiffelTowerFs::new(base, 0.3);
        let (mut ne, mut np) = (0, 0);
        let xs = [0.5, 0.5, 0.5, 0.5];
        let mut f = [0.0; 3];
        let mut load = JacobianLoad {
            rail_index: 0,
            surface1: None,
            surf1_offset: 0,
            surface2: None,
            surf2_offset: 0,
            x: &xs,
        };
        let r = fs.load_jacobian(&k, &mut ne, &mut np, &mut load, &mut f, None);
        assert!(r.is_ok(), "{r:?}");
        assert_eq!(np, 4);
        assert!(f.iter().all(|v| v.is_finite()));
        // left at the fillet offsets
        assert_eq!(fs.base.surface(0).expect("rail 0").offset_distance(), 2.0);
        assert_eq!(fs.base.surface(1).expect("rail 1").offset_distance(), 1.0);
        let _ = NurbCircleParam::Quadratic;
    }

    #[test]
    fn load_initial_values_without_surf2_calcs_is_a_no_op() {
        let k = Kernel::default();
        let o = Vec3::new(0.0, 0.0, 0.0);
        let off = rail(
            &k,
            1.0,
            plane(o, Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0)),
            0,
        );
        let base = ConstantRadiusAssistedFs::from_parts(
            1e-6,
            0.5,
            0.03,
            [1.0, 1.0],
            [Some(off.clone()), None],
            [1.0, -1.0],
            [None, None],
        );
        let mut fs = SecantConstantRadiusFs::new(base, 0.2);
        let mut ivl = ExtentNd::new(4);
        let mut per = vec![false, true];
        let mut g = vec![0.25, 0.75];
        let mut surface1 = off;
        let r = fs
            .load_initial_values(
                &k,
                None,
                false,
                0,
                &mut surface1,
                0,
                &mut ivl,
                &mut per,
                &mut g,
            )
            .expect("no-op");
        assert_eq!(r, None);
        assert_eq!(per, vec![false, true]);
        assert_eq!(g, vec![0.25, 0.75]);
    }

    #[test]
    fn surface_offset_shares_matching_surfaces() {
        let x = [0.0; 8];
        let load = JacobianLoad {
            rail_index: 1,
            surface1: Some(SurfaceId::Free(7)),
            surf1_offset: 4,
            surface2: Some(SurfaceId::Free(3)),
            surf2_offset: 6,
            x: &x,
        };
        let mut np = 2;
        // rail 0 matches rpSurface2 for lRailIndex 1
        let off = surface_offset(&mut np, &load, 0, SurfaceId::Free(3), OP).expect("off");
        assert_eq!((off, np), (6, 2));
        // rail 1 matches rpSurface1 for lRailIndex 1
        let off = surface_offset(&mut np, &load, 1, SurfaceId::Free(7), OP).expect("off");
        assert_eq!((off, np), (4, 2));
        // no match: next two parameters
        let off = surface_offset(&mut np, &load, 1, SurfaceId::Free(9), OP).expect("off");
        assert_eq!((off, np), (2, 4));
    }
}
