//! Standard fillet solvers (part 1): the constant-radius solver's constructors and
//! equations, the constant-distance solver's equations and initial values, the
//! curve-based solver's base-curve evaluation, the variable-radius solver's radius
//! queries, the surface/surface solver's offset extension and the linear, circular and
//! blend-curve cross section fillet surface generators.
//!
//! Upstream: `source/SMLib/src/SmFilletStandardSolver.cpp`,
//! `source/SMLib/inc/SmFilletStandardSolver.h`.
//!
//! | Rust                                                     | Upstream                                              |
//! |----------------------------------------------------------|-------------------------------------------------------|
//! | [`ConstantRadiusFs`] (alias of the co-ported [`ConstantRadiusAssistedFs`]) | `SmConstantRadiusFS` (members; `Drop` is `~SmConstantRadiusFS`) |
//! | [`ConstantRadiusAssistedFs::from_surfaces`]              | `SmConstantRadiusFS::SmConstantRadiusFS` (surfaces)   |
//! | [`constant_radius_from_edgeuse`]                         | `SmConstantRadiusFS::SmConstantRadiusFS` (edge use)   |
//! | [`ConstantRadiusAssistedFs::setup_offset_values`]        | `SmConstantRadiusFS::SetupOffsetValues`               |
//! | [`ConstantRadiusAssistedFs::load_jacobian`]              | `SmConstantRadiusFS::LoadJacobian`                    |
//! | [`ConstantRadiusAssistedFs::jacobian_size`]              | `SmConstantRadiusFS::GetJacobianSize`                 |
//! | [`ConstantRadiusAssistedFs::offset_radii_can_change`]    | `SmConstantRadiusFS::OffsetRadiiCanChange`            |
//! | [`ConstantRadiusAssistedFs::fillet_radius`]              | `SmConstantRadiusFS::GetFilletRadius`                 |
//! | [`CONSTANT_RADIUS_FS_TYPE`], [`constant_radius_is_kind_of`] | `SmConstantRadiusFS::GetType`, `GetTypeString`, `IsKindOf` |
//! | [`CONSTANT_RADIUS_ASSISTED_FS_TYPE`]                     | `SmConstantRadiusAssistedFS::GetType`, `GetTypeString` (`Drop` is the destructor) |
//! | [`SurfaceSurfaceFs`]                                     | `SmSurfaceSurfaceFS` (members; `Drop` is the destructor) |
//! | [`SurfaceSurfaceFs::set_fillet_trim_type`]               | `SmSurfaceSurfaceFS::SetFilletTrimType`               |
//! | [`SurfaceSurfaceFs::setup_offset_extension`]             | `SmSurfaceSurfaceFS::SetupOffsetExtension`            |
//! | [`SurfaceSurfaceFs::type_code`], [`SurfaceSurfaceFs::type_string`] | `SmSurfaceSurfaceFS::GetType`, `GetTypeString` |
//! | [`ConstantDistanceFs`]                                   | `SmConstantDistanceFS` (members; `Drop` is the destructor) |
//! | [`ConstantDistanceFs::can_make_analytic`], [`ConstantDistanceFs::distance`] | `CanMakeAnalytic`, `GetDistance`  |
//! | [`ConstantDistanceFs::setup_offset_values`]              | `SmConstantDistanceFS::SetupOffsetValues`             |
//! | [`ConstantDistanceFs::load_jacobian`]                    | `SmConstantDistanceFS::LoadJacobian`                  |
//! | [`ConstantDistanceFs::jacobian_size`]                    | `SmConstantDistanceFS::GetJacobianSize`               |
//! | [`ConstantDistanceFs::load_initial_values`]              | `SmConstantDistanceFS::LoadInitialValues`             |
//! | [`InitialSurface`]                                       | the `crSurface1` argument of `LoadInitialValues`      |
//! | [`ConstantDistanceFs::is_kind_of`], [`ConstantDistanceFs::type_code`], [`ConstantDistanceFs::type_string`] | `IsKindOf`, `GetType`, `GetTypeString` |
//! | [`CurveBasedFs`], [`CurveBasedFs::new`], [`CurveBasedFs::new_in`] | `SmCurveBasedFS`, `SmCurveBasedFS::SmCurveBasedFS` (`Drop` is the destructor) |
//! | [`CurveBasedFs::original_curve`], [`CurveBasedFs::set_original_curve`] | `GetOriginalCurve`, `SetOriginalCurve`  |
//! | [`CurveBasedFs::evaluate_base_curve`]                    | `SmCurveBasedFS::EvaluateBaseCurve`                   |
//! | [`CurveBasedFs::is_kind_of`], [`CurveBasedFs::type_code`], [`CurveBasedFs::type_string`] | `IsKindOf`, `GetType`, `GetTypeString` |
//! | [`VariableRadiusFs`]                                     | `SmVariableRadiusFS` (members; `Drop` is the destructor) |
//! | [`VariableRadiusFs::fillet_law`], [`VariableRadiusFs::fillet_law_orientation`], [`VariableRadiusFs::fillet_law_interval`], [`VariableRadiusFs::set_fillet_law_interval`] | `GetFilletLaw`, `GetFilletLawOrientation`, `GetFilletLawInterval`, `SetFilletLawInterval` |
//! | [`VariableRadiusFs::offset_radii_can_change`]            | `SmVariableRadiusFS::OffsetRadiiCanChange`            |
//! | [`VariableRadiusFs::set_extension_flags`]                | `SmVariableRadiusFS::SetExtensionFlags`               |
//! | [`VariableRadiusFs::setup_offset_values`]                | `SmVariableRadiusFS::SetupOffsetValues`               |
//! | [`VariableRadiusFs::jacobian_size`]                      | `SmVariableRadiusFS::GetJacobianSize`                 |
//! | [`VariableRadiusFs::type_code`], [`VariableRadiusFs::type_string`] | `SmVariableRadiusFS::GetType`, `GetTypeString` |
//! | [`LinearCrossSectionFsg::create_surface`]                | `SmLinearCrossSectionFSG::CreateSurface`              |
//! | [`CircularCrossSectionFsg`], [`CircularCrossSectionFsg::create_surface_from_curves`] | `SmCircularCrossSectionFSG`, `CreateSurfaceFromCurves` |
//! | [`BlendCurveCrossSectionFsg`], [`BlendCurveCrossSectionFsg::create_surface_from_curves`] | `SmBlendCurveCrossSectionFSG`, `CreateSurfaceFromCurves` |
//! | [`GuessSphereSolve::new`]                                | `SmGuessSphereSolveENFO::SmGuessSphereSolveENFO` (file-local class) |
//! | [`FilletSolver`] (re-exported from `blend.fillet_solver_p1`) | the `SmFilletSolver` base of the solvers (`FilletSolver::is_kind_of` is `SmFilletSolver::IsKindOf`) |
//! | [`FilletSolverType`], [`FilletSurfaceGeneratorType`] (re-exported from `blend.fillet_solver_p1`), [`BoundaryTrimmingType`] (re-exported from `blend.fillet_geom_p1`) | `SmFilletSolverType`, `SmFilletSurfaceGeneratorType`, `SmBoundaryTrimmingType` |
//! | [`FilletLaw`], [`FilletLawKind`] (CO-PORTED)             | `SmFilletLaw`, `SmLinearFilletLaw`, `SmBSplineFilletLaw` (the data these solvers read; `Evaluate` is in `blend::fillet_executive::p1`, the constructors in `blend::fillet_executive::p2`) |
//! | [`JacobianSurfaces`]                                     | the `rpSurface1`, `rpSurface2` arguments of `LoadJacobian` |
//!
//! Deviations:
//! - **Knife edges.** With `fixes.fillet_knife_edge_rounds` (WR-570, the fork's
//!   source fork `b7ec55b`, not in SMLib at the pin) the edge-use constructors
//!   orient their offsets by [`sector_sides`] (`sm_SectorSides`), which also sides a
//!   cusp's faces; off, they keep upstream's tangency refusal.
//! - **Sections at thirds.** [`BlendCurveCrossSectionFsg::at_thirds`] is the fork's
//!   `m_bAtThirds` (source fork `fd3b6a7`, not in SMLib at the pin; off unless
//!   the executive's `SetBlendCurveAtThirds` is set, so the default is the pinned C++).
//! - **Object model.** `SmConstantRadiusFS` is the co-ported [`ConstantRadiusAssistedFs`]
//!   of `blend.fillet_custom_solver` (the members of `SmFilletSolver`,
//!   `SmConstantRadiusFS` and `SmConstantRadiusAssistedFS` are the same), aliased as
//!   [`ConstantRadiusFs`]. The other solvers hold the [`FilletSolver`] base of unit
//!   `blend.fillet_solver_p1`. `GetSurface` is [`FilletSolver::surface`] (the fillet
//!   geometry for the surfaces, `m_pFGForSurfaces`, is never set by these solvers). A
//!   missing surface, an out-of-range rail index
//!   or a missing original curve (upstream dereferences `NULL` or indexes past its
//!   arrays) is [`SolidError::InvalidInput`]. The solver type and run-time type queries
//!   are plain functions and constants.
//! - **Constructors.** The edge-use constructor returns `Err` where upstream signals
//!   `SM_ERR` and leaves a half-built solver (tangent surfaces). It takes the B-rep
//!   mutably (`SmEdgeuse::EvaluateBinormal` may create a UV trim curve). Its offset
//!   surfaces share the face surfaces (the copies keep their owner); the surface
//!   constructor's take the caller's surfaces. Constructors are fallible: the offset
//!   surface constructor (`SM_SS_UNKNOWN` singularities) evaluates its base. The
//!   constructors take the executive's pseudo B-rep ([`FilletBrep`]) and the solver's
//!   identity ([`FilletSolverId`]): the `SmFilletSolver` constructor's default fillet
//!   geometry ([`FilletSolver::new_in`]) is stored there, and `FinishConstruction`
//!   ([`FilletSolver::finish_construction`]) installs copies of the offsets into it. A
//!   construction that fails after the base leaves that geometry in `pseudo` (the
//!   arena does not delete; upstream's half-built solver holds it).
//! - **Offset surfaces.** The rails are the ported `SmOffsetSurface`
//!   (`surface.offset_surface`) through [`OffsetSurface`]. Evaluations store the base
//!   extension they make, so `crSurface1` of `LoadInitialValues` names the solver's own
//!   rail surface or another one ([`InitialSurface`]): upstream's callers pass
//!   `*GetSurface(lRailIndex)`, whose extension (and grown natural UV domain) then stays
//!   on the solver, and the other rail's surface is `rpSurface2` itself. Its
//!   `rpSurface2->LocalPointSolve` / `GlobalPointSolve` run on a copy of the other
//!   rail's offset (the generic point solvers take the surface shared); an extension
//!   the local solve made is handed back to the solver's offset. The `SmTemporaryChangeValue` on the
//!   other base's `GetOutOfBoundsEnabled()` changes the solver's surface (and
//!   `crSurface1` when it has the same base) in place and restores it on every exit.
//! - **Arguments.** Out-params become return values; `SmContext` arguments are dropped.
//!   `LoadJacobian` takes the base-surface identities of `rpSurface1` / `rpSurface2`
//!   ([`JacobianSurfaces`]) and the in/out offsets `rlSurf1Offset`, `rlSurf2Offset` as
//!   a pair; `rbFoundAnswer` is the result. `LoadInitialValues` takes `rlSurface2Index` (read when
//!   `rbDoSurf2Calcs` is `0`) and returns the rail of `rpSurface2` and the new index. The
//!   surface generators drop the solver (they read only its tolerance and type) and
//!   their unused centre line, surface and UV curve arguments; the rails are plain
//!   B-spline curves changed in place as upstream (knot insertion and synchronisation).
//!   The circular generator's `CreateSurfaceFromCurves` takes the centre line as a
//!   [`Curve`] of an `SmBSplineCurve` kind, so `GetDegree` / `IsLine` / `IsArc` /
//!   `Evaluate` / `GetNaturalInterval` dispatch on its kind as upstream's virtual calls
//!   on `rCenterLine` do (an `SmLine` centre line, `SmOffsetGeometryCreation.cpp:2717-2751`,
//!   answers degree 1 and `SmLine::IsLine` even after `MakeCurvesCompatible` raised its
//!   NURB to degree 3); the knot synchronisation edits its NURB part. The blend
//!   generator still takes the centre line's B-spline data (no caller passes an
//!   analytic one).
//!   `BlendCurveCrossSectionFsg::continuity` is `u32` where `m_lContinuity` is `ULONG`
//!   (64 bits in the reference build); the FFI guard bounds it to 32 bits, so no compared
//!   value differs.
//! - **Indexing.** `rF`, the Jacobian, `crX`, `rGuessT`, `rIntervals` and the cross
//!   section array of `CreateSurfaceFromCurves` are indexed with checks: an index
//!   upstream would read or write out of bounds is [`SolidError::InvalidInput`]
//!   (`SmCircularCrossSectionFSG::CreateSurfaceFromCurves` reads past its cross sections
//!   when a ruled or revolved surface needs a second, knot-equalising pass,
//!   `plan/upstream-issues.md`).
//! - **Unported callees.** `SmFilletSolver::SetExtensionFlags`
//!   ([`FilletSolver::set_extension_flags`]) takes the B-rep and stops at its corner
//!   lookup (`blend.fillet_executive_p1`).
//! - No recursion.

use crate::blend::fillet_custom_solver::{
    offset_compute_inward_vector, offset_evaluate_1st, offset_evaluate_2nd, offset_evaluate_point,
    offset_local_point_solve, offset_natural_uv_domain, surface_global_point_solve,
    ConstantRadiusAssistedFs, OffsetSurface, SurfaceId,
};
use crate::blend::fillet_geom::p1::{FilletBrep, FilletSolverId};
use crate::blend::fillet_solver::p1::{
    ExtensionCorners, FilletSolverOps, FilletSphereSolveEnfo, FilletSurfaceGenerator,
};
use crate::curve::curve::p1::GlobalPointSolveOptions;
use crate::curve::curve::p4::is_closed_base;
use crate::curve::curve::p5::is_arc_base;
use crate::curve::curve::{Curve, CurveKind};
use crate::error::{ignore_status, ignored, Result, SolidError};
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::extent1d::{Extent1d, ExtentNd};
use crate::geom::extent2d::Extent2d;
use crate::geom::geom_utility::p2::ContinuityType;
use crate::geom::global_solver::{SolutionRequested, SolverOperation};
use crate::geom::matrix::Matrix;
use crate::geom::vector2d::{Point2, Vec2};
use crate::intersect::surface_intersector::p1::TsectPnt;
use crate::kernel::Kernel;
use crate::math::{Point3, Vec3, EFF_ZERO, EFF_ZERO_SQ, EFF_ZERO_SQRT, PI, UNDEF_DOUBLE};
use crate::nurbs::b_spline_curve::p1::NurbCircleParam;
use crate::nurbs::b_spline_curve::{BSplineCurve, ControlPointForm};
use crate::nurbs::b_spline_surface::{BSplineSurface, SurfParamType};
use crate::nurbs::b_spline_surface_n_lib::p2::{EvaluateFlags, OrientType, SkinnedSurfaceOptions};
use crate::nurbs::crv_fit::p1::ParamMethod;
use crate::nurbs::knots::Side;
use crate::surface::offset_surface::SS_UNKNOWN;
use crate::surface::surface::p1::zone_tol_3d;
use crate::surface::surface::p3::{out_of_bounds_enabled, set_out_of_bounds_enabled};
use crate::surface::surface::p5::{SS_UMAX, SS_UMIN};
use crate::surface::surface::p7::{compute_higher_order_derivs, drop_vectors_at};
use crate::surface::surface::{Surface, SurfaceKind};
use crate::topology::edgeuse::p1 as edgeuse;
use crate::topology::edgeuse::p3 as edgeuse_p3;
use crate::topology::face::p2 as face;
use crate::topology::{edge, Brep, EdgeuseId};

const OP: &str = "blend::fillet_standard_solver::p1";
const OP_CR_CTOR: &str = "blend::fillet_standard_solver::p1::constant_radius_from_edgeuse";
const OP_CR_JAC: &str = "blend::fillet_standard_solver::p1::ConstantRadiusFs::load_jacobian";
const OP_CD_JAC: &str = "blend::fillet_standard_solver::p1::ConstantDistanceFs::load_jacobian";
const OP_CD_INIT: &str =
    "blend::fillet_standard_solver::p1::ConstantDistanceFs::load_initial_values";
const OP_BASE: &str = "blend::fillet_standard_solver::p1::CurveBasedFs::evaluate_base_curve";
const OP_CIRC: &str =
    "blend::fillet_standard_solver::p1::CircularCrossSectionFsg::create_surface_from_curves";
const OP_BLEND: &str =
    "blend::fillet_standard_solver::p1::BlendCurveCrossSectionFsg::create_surface_from_curves";
const OP_VR: &str = "blend::fillet_standard_solver::p1::VariableRadiusFs";

/// `dAnalyticTol` of the surface generators and `SetExtensionFlags`.
const ANALYTIC_TOL: f64 = 1.0e-8;

/// Sample count of the generators' `IsLine` / `IsArc` tests.
const ANALYTIC_SAMPLES: u64 = 11;

// ---- run-time types ---------------------------------------------------------------------

/// `TOPO_BASE_TYPE`.
const TOPO_BASE_TYPE: i64 = 16000;

/// `SmFilletSolver_TYPE` (`TOPO_BASE_TYPE + 37`, [`FilletSolver::TYPE`]).
pub const FILLET_SOLVER_TYPE: i64 = FilletSolver::TYPE;
/// `SmVariableRadiusFS_TYPE` (`TOPO_BASE_TYPE + 38`).
pub const VARIABLE_RADIUS_FS_TYPE: i64 = TOPO_BASE_TYPE + 38;
/// `SmConstantRadiusFS_TYPE` (`TOPO_BASE_TYPE + 42`).
pub const CONSTANT_RADIUS_FS_TYPE: i64 = TOPO_BASE_TYPE + 42;
/// `SmConstantRadiusAssistedFS_TYPE` (`TOPO_BASE_TYPE + 43`).
pub const CONSTANT_RADIUS_ASSISTED_FS_TYPE: i64 = TOPO_BASE_TYPE + 43;
/// `SmSurfaceSurfaceFS_TYPE` (`TOPO_BASE_TYPE + 44`).
pub const SURFACE_SURFACE_FS_TYPE: i64 = TOPO_BASE_TYPE + 44;
/// `SmConstantDistanceFS_TYPE` (`TOPO_BASE_TYPE + 45`).
pub const CONSTANT_DISTANCE_FS_TYPE: i64 = TOPO_BASE_TYPE + 45;
/// `SmCurveBasedFS_TYPE` (`TOPO_BASE_TYPE + 46`).
pub const CURVE_BASED_FS_TYPE: i64 = TOPO_BASE_TYPE + 46;

/// `SmConstantRadiusFS::GetTypeString`.
pub const CONSTANT_RADIUS_FS_TYPE_STRING: &str = "SmConstantRadiusFS_TYPE";
/// `SmConstantRadiusAssistedFS::GetTypeString`.
pub const CONSTANT_RADIUS_ASSISTED_FS_TYPE_STRING: &str = "SmConstantRadiusAssistedFS_TYPE";

/// `SmConstantRadiusFS::IsKindOf`: `SmConstantRadiusFS_TYPE` or a type a fillet solver
/// is ([`FilletSolver::is_kind_of`]).
#[must_use]
pub const fn constant_radius_is_kind_of(t: i64) -> bool {
    t == CONSTANT_RADIUS_FS_TYPE || FilletSolver::is_kind_of(t)
}

/// The fillet solver base (`SmFilletSolver`), the solver kind (`SmFilletSolverType`)
/// and the surface generator kind (`SmFilletSurfaceGeneratorType`) of unit
/// `blend.fillet_solver_p1`, re-exported here.
pub use crate::blend::fillet_solver::p1::{
    FilletSolver, FilletSolverType, FilletSurfaceGeneratorType,
};

/// An `SmBoolean` result whose failure upstream cannot see (`IsPeriodic`): `Unported`
/// propagates, any other failure reads as `TRUE`.
fn boolean_status(r: Result<bool>) -> Result<bool> {
    match r {
        Ok(v) => Ok(v),
        Err(e) if e.is_unported() => Err(e),
        Err(_) => Ok(true),
    }
}

/// [`boolean_status`] for `IsSingularity`: a failure reads as singular, direction
/// unknown (`SM_SP_NEITHER`, the `v` branch).
fn singularity_status(
    r: Result<crate::surface::surface::p5::Singularity>,
) -> Result<crate::surface::surface::p5::Singularity> {
    match r {
        Ok(v) => Ok(v),
        Err(e) if e.is_unported() => Err(e),
        Err(_) => Ok(crate::surface::surface::p5::Singularity {
            singular: true,
            direction: SurfParamType::Neither,
        }),
    }
}

/// How a fillet surface is trimmed (`SmBoundaryTrimmingType`): the enum co-ported by
/// `blend.fillet_geom_p1` (PR #147), re-exported here.
pub use crate::blend::fillet_geom::p1::BoundaryTrimmingType;

// ---- the SmFilletSolver base --------------------------------------------------------------

/// `GetSurface(lRailIndex)` of the [`FilletSolver`] base as these solvers dereference
/// it: the rail's offset surface, which must be set.
pub(crate) trait SolverRails {
    /// `*GetSurface(rail)`.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for a rail index above 1 or a missing surface
    /// (upstream dereferences `NULL`); those of [`FilletSolver::surface`].
    fn rail(&self, rail: usize) -> Result<&OffsetSurface>;

    /// `*GetSurface(rail)`, mutable.
    ///
    /// # Errors
    ///
    /// As [`SolverRails::rail`].
    fn rail_mut(&mut self, rail: usize) -> Result<&mut OffsetSurface>;

    /// `GetSurface(0)->SetOffsetDistance(r * m_dOrientations[0])`, then rail 1.
    ///
    /// # Errors
    ///
    /// As [`SolverRails::rail`].
    fn set_offsets(&mut self, radius: f64) -> Result<()>;
}

impl SolverRails for FilletSolver {
    fn rail(&self, rail: usize) -> Result<&OffsetSurface> {
        self.surface(rail)?
            .map(|slot| &slot.surface)
            .ok_or_else(|| SolidError::invalid_input(OP, "no offset surface for the rail"))
    }

    fn rail_mut(&mut self, rail: usize) -> Result<&mut OffsetSurface> {
        self.surface_mut(rail)?
            .map(|slot| &mut slot.surface)
            .ok_or_else(|| SolidError::invalid_input(OP, "no offset surface for the rail"))
    }

    fn set_offsets(&mut self, radius: f64) -> Result<()> {
        for rail in 0..2 {
            let d = radius * self.orientation(rail)?;
            self.rail_mut(rail)?.set_offset_distance(d);
        }
        Ok(())
    }
}

// ---- shared index helpers -----------------------------------------------------------------

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

fn set_j(j: &mut Matrix, row: usize, col: usize, v: f64, op: &'static str) -> Result<()> {
    j.set_at(row, col, v)
        .map_err(|_| SolidError::invalid_input(op, "Jacobian index out of range"))
}

fn add(a: usize, b: usize, op: &'static str) -> Result<usize> {
    a.checked_add(b)
        .ok_or_else(|| SolidError::invalid_input(op, "index overflow"))
}

fn sub(a: usize, b: usize, op: &'static str) -> Result<usize> {
    a.checked_sub(b)
        .ok_or_else(|| SolidError::invalid_input(op, "index underflow"))
}

fn uv_at(x: &[f64], i: usize, op: &'static str) -> Result<Point2> {
    Ok(Point2::new(x_at(x, i, op)?, x_at(x, add(i, 1, op)?, op)?))
}

fn interval_mut<'a>(
    ivls: &'a mut ExtentNd,
    i: usize,
    op: &'static str,
) -> Result<&'a mut Extent1d> {
    ivls.get_mut(i)
        .ok_or_else(|| SolidError::invalid_input(op, "rIntervals index out of range"))
}

/// The `rpSurface1` / `rpSurface2` arguments of `LoadJacobian`: the identity of the
/// base surface of each (`None` for `NULL`), used only to map them to the solver's
/// surfaces.
pub type JacobianSurfaces = [Option<SurfaceId>; 2];

// ---- SmConstantRadiusFS -------------------------------------------------------------------

/// The constant-radius fillet solver (`SmConstantRadiusFS`): the co-ported
/// [`ConstantRadiusAssistedFs`] holds its members.
pub type ConstantRadiusFs = ConstantRadiusAssistedFs;

/// `SE(SmEdgeuse::EvaluateBinormal(dT, FALSE, rPnt, rBinVec, NULL, &rFaceuseNormal))`:
/// the binormal and face use normal of an edge use. The status is only signalled
/// (`SE`): after a failure the binormal keeps its `SM_UNDEF_DOUBLE` initial value and
/// the face use normal is zero (`EvaluateBinormal` zeroes it once the edge use passed its
/// loop check) or `SM_UNDEF_DOUBLE` (an edge use not in a loop). Only
/// [`SolidError::Unported`] propagates.
fn evaluate_binormal(k: &Kernel, brep: &mut Brep, eu: EdgeuseId, t: f64) -> Result<(Vec3, Vec3)> {
    let options = edgeuse_p3::BinormalOptions::default();
    if let Some(r) = ignored(edgeuse_p3::evaluate_binormal(
        k, brep, eu, t, false, &options,
    ))? {
        return Ok((r.binormal, r.face_normal));
    }
    let undef = Vec3::new(UNDEF_DOUBLE, UNDEF_DOUBLE, UNDEF_DOUBLE);
    let normal = if edgeuse::is_loop_edgeuse(brep, eu)? {
        Vec3::new(0.0, 0.0, 0.0)
    } else {
        undef
    };
    Ok((undef, normal))
}

/// The offset orientations of the faces at an edge use and its radial
/// (`m_dOrientations` as the edge-use constructors compute them), with the edge use
/// pair.
fn edgeuse_orientations(
    k: &Kernel,
    brep: &mut Brep,
    eu: EdgeuseId,
    op: &'static str,
) -> Result<([EdgeuseId; 2], [f64; 2])> {
    let eu1 = edgeuse::radial(brep, eu)?
        .ok_or_else(|| SolidError::invalid_input(op, "edge use without radial"))?;
    let e = edgeuse::edge(brep, eu)?
        .ok_or_else(|| SolidError::invalid_input(op, "edge use without edge"))?;
    let t = edge::interval(brep, e)?.evaluate(0.5);
    let (bin_vec, mut faceuse_normal) = evaluate_binormal(k, brep, eu, t)?;
    let (bin_vec2, mut faceuse_normal2) = evaluate_binormal(k, brep, eu1, t)?;

    let faceuse_of = |eu: EdgeuseId| -> Result<_> {
        edgeuse::faceuse(brep, eu)?
            .ok_or_else(|| SolidError::invalid_input(op, "edge use without face use"))
    };
    if edgeuse::faceuse_orientation(brep, faceuse_of(eu)?)? == OrientType::Opposite {
        faceuse_normal = -faceuse_normal;
    }
    if edgeuse::faceuse_orientation(brep, faceuse_of(eu1)?)? == OrientType::Opposite {
        faceuse_normal2 = -faceuse_normal2;
    }

    let mut orientations = [1.0, 1.0];
    let dot1 = bin_vec2.dot(faceuse_normal);
    if dot1.abs() < EFF_ZERO_SQRT {
        // Surfaces are tangent at midpoint: can't fillet this.
        return Err(SolidError::sm_err(op));
    }
    if dot1 < 0.0 {
        orientations[0] = -1.0;
    }

    let dot2 = bin_vec.dot(faceuse_normal2);
    if dot2.abs() < EFF_ZERO_SQRT {
        // Tangent surfaces.
        return Err(SolidError::sm_err(op));
    }
    if dot2 < 0.0 {
        orientations[1] = -1.0;
    }
    Ok(([eu, eu1], orientations))
}

/// `sm_SectorSides(rpEdgeuses, dLength, rdSides)` (the fork's source fork
/// `b7ec55b`, `fixes.fillet_knife_edge_rounds`): the side of each face of the sector
/// between `eus[0]` and its radial `eus[1]` on which the other face lies, at the middle
/// of the edge, `+1.0` when face `1 - i` lies on the side face `i`'s surface normal
/// points to, else `-1.0` (the sign of face `i`'s offset). Where the faces are tangent
/// at the edge and their binormals point the same way (a cusp) they part to second
/// order, face `j` at height `s^2/2 kj Nj` along the binormal, and face `1 - i` lies on
/// the side of face `i` given by `k(1-i) N(1-i).ni - ki Ni.ni`. `None` where upstream
/// returns a failure status: a smooth edge (tangent faces whose binormals point apart),
/// osculating faces (the parting below `SM_EFF_ZERO_SQRT` at the blend's size
/// `length`), a missing radial (`NER`) or a failing binormal or normal-section
/// evaluation (`SER`).
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for an edge use without edge, face use or face surface
/// where upstream dereferences `NULL`; [`SolidError::Unported`] from the evaluations.
pub(super) fn sector_sides(
    k: &Kernel,
    brep: &mut Brep,
    eus: [Option<EdgeuseId>; 2],
    length: f64,
    op: &'static str,
) -> Result<Option<[f64; 2]>> {
    let [Some(eu0), Some(eu1)] = eus else {
        return Ok(None); // NER
    };
    let e = edgeuse::edge(brep, eu0)?
        .ok_or_else(|| SolidError::invalid_input(op, "edge use without edge"))?;
    let param = edge::interval(brep, e)?.evaluate(0.5);

    let Some((b0, n0, uv0, d0)) = sector_binormal(k, brep, eu0, param, op)? else {
        return Ok(None); // SER
    };
    let Some((b1, n1, uv1, d1)) = sector_binormal(k, brep, eu1, param, op)? else {
        return Ok(None); // SER
    };

    let first = [b1.dot(n0), b0.dot(n1)];
    if first[0].abs() >= EFF_ZERO_SQRT && first[1].abs() >= EFF_ZERO_SQRT {
        let side = |d: f64| if d < 0.0 { -1.0 } else { 1.0 };
        return Ok(Some([side(first[0]), side(first[1])]));
    }
    if b0.dot(b1) <= 0.0 {
        return Ok(None); // smooth: the faces continue each other
    }

    // a cusp: each face's normal curvature along its binormal, into the face
    let Some((c0, s0)) = normal_section(k, brep, eu0, uv0, d0, b0, op)? else {
        return Ok(None); // SER
    };
    let Some((c1, s1)) = normal_section(k, brep, eu1, uv1, d1, b1, op)? else {
        return Ok(None); // SER
    };
    let parts = [
        c1 * s1.dot(n0) - c0 * s0.dot(n0),
        c0 * s0.dot(n1) - c1 * s1.dot(n1),
    ];
    let mut sides = [1.0, 1.0];
    for (side, part) in sides.iter_mut().zip(parts) {
        if part.abs() * length < EFF_ZERO_SQRT {
            return Ok(None); // the faces osculate: no sector to fillet
        }
        *side = if part < 0.0 { -1.0 } else { 1.0 };
    }
    Ok(Some(sides))
}

/// [`sector_sides`]' `EvaluateBinormal(dParam, FALSE, ..)` of one edge use: the
/// binormal, the surface's normal (the face use normal, reversed for an
/// [`OrientType::Opposite`] face use), the surface parameters and their binormal
/// direction; `None` for a failing evaluation (`SER`).
fn sector_binormal(
    k: &Kernel,
    brep: &mut Brep,
    eu: EdgeuseId,
    param: f64,
    op: &'static str,
) -> Result<Option<(Vec3, Vec3, Point2, Vec2)>> {
    let options = edgeuse_p3::BinormalOptions {
        want_uv_vector: true,
    };
    let Some(r) = ignored(edgeuse_p3::evaluate_binormal_uv(
        k, brep, eu, param, false, &options,
    ))?
    else {
        return Ok(None);
    };
    let mut normal = r.eval.face_normal;
    let faceuse = edgeuse::faceuse(brep, eu)?
        .ok_or_else(|| SolidError::invalid_input(op, "edge use without face use"))?;
    if edgeuse::faceuse_orientation(brep, faceuse)? == OrientType::Opposite {
        normal = -normal; // the surface's normal
    }
    let uv_dir = r.uv_vector.unwrap_or(Vec2::new(0.0, 0.0));
    Ok(Some((r.eval.binormal, normal, r.eval.uv, uv_dir)))
}

/// [`sector_sides`]' `EvaluateNormalSection(uv, uvDir.x >= 0, uvDir.y >= 0, binormal,
/// ..)` on the surface of `eu`'s face: the normal curvature and the unit surface normal;
/// `None` for a failing evaluation (`SER`).
fn normal_section(
    k: &Kernel,
    brep: &Brep,
    eu: EdgeuseId,
    uv: Point2,
    uv_dir: Vec2,
    binormal: Vec3,
    op: &'static str,
) -> Result<Option<(f64, Vec3)>> {
    let f = edgeuse::face(brep, eu)?
        .ok_or_else(|| SolidError::invalid_input(op, "edge use without face"))?;
    let surface = face::surface(brep, f)?
        .ok_or_else(|| SolidError::invalid_input(op, "face without surface"))?;
    let u_side = if uv_dir.x >= 0.0 {
        Side::Left
    } else {
        Side::Right
    };
    let v_side = if uv_dir.y >= 0.0 {
        Side::Left
    } else {
        Side::Right
    };
    Ok(
        ignored(surface.evaluate_normal_section(k, uv, u_side, v_side, binormal))?
            .map(|section| (section.curvature, section.normal)),
    )
}

/// The face surface at an edge use as the base of an offset surface.
fn face_surface(brep: &Brep, eu: EdgeuseId, op: &'static str) -> Result<(Surface, SurfaceId)> {
    let f = edgeuse::face(brep, eu)?
        .ok_or_else(|| SolidError::invalid_input(op, "edge use without face"))?;
    let s = face::surface(brep, f)?
        .ok_or_else(|| SolidError::invalid_input(op, "face without surface"))?;
    // (the offset shares the face's surface: its owner is kept)
    Ok((s.clone_keeping_owner(), SurfaceId::Face(f)))
}

/// `SmConstantRadiusFS(crContext, dThisApproxTol3d, dAngleTolerance,
/// dTangencyTolerance, dFilletRadius, pEdgeuse, pdOptSecondRadius)`: the offset
/// surfaces (offset by the radius, extension allowed) of the faces at `edgeuse` and its
/// radial, oriented by the binormals at the middle of the edge. The radius of rail 1
/// is `second_radius` when given. The constructor's fillet geometry is stored in
/// `pseudo` for solver `id` and receives copies of the offsets (`FinishConstruction`).
///
/// # Errors
///
/// `SM_ERR` when the surfaces are tangent at the middle of the edge (upstream signals
/// it and leaves the solver without edge uses or surfaces);
/// [`SolidError::InvalidInput`] for an edge use without radial, edge, face use or face
/// surface; those of [`OffsetSurface::new`]; [`SolidError::Unported`] from
/// `SmEdgeuse::EvaluateBinormal` (whose other failures are only signalled upstream and
/// end here as tangent surfaces); those of [`FilletSolver::new_in`] and
/// [`FilletSolver::finish_construction`].
#[allow(clippy::too_many_arguments)]
pub fn constant_radius_from_edgeuse(
    k: &Kernel,
    brep: &mut Brep,
    pseudo: &mut FilletBrep,
    id: FilletSolverId,
    approx_tol_3d: f64,
    ang_tol_rad: f64,
    tangency_tolerance: f64,
    fillet_radius: f64,
    edgeuse: EdgeuseId,
    second_radius: Option<f64>,
) -> Result<ConstantRadiusFs> {
    const OP: &str = OP_CR_CTOR;
    let solver = FilletSolver::new_in(
        k,
        pseudo,
        id,
        approx_tol_3d,
        ang_tol_rad,
        tangency_tolerance,
    )?;
    let mut radii = [fillet_radius, fillet_radius];
    if let Some(r) = second_radius {
        radii[1] = r;
    }

    // Create offset surfaces of appropriate radius in appropriate direction.
    let ([eu0, eu1], orientations) = if k.fixes.fillet_knife_edge_rounds {
        // (fork b7ec55b) offset each surface to the side of it the other face lies on
        // (sm_SectorSides), a cusp's faces too
        let radial = edgeuse::radial(brep, edgeuse)?;
        let [r0, r1] = radii;
        let length = if r0.abs() > r1.abs() {
            r0.abs()
        } else {
            r1.abs()
        }; // smos_Max
        let Some(sides) = sector_sides(k, brep, [Some(edgeuse), radial], length, OP)? else {
            // Smooth, or no sector: can't fillet this.
            return Err(SolidError::sm_err(OP));
        };
        let eu1 = radial.ok_or_else(|| SolidError::invalid_input(OP, "edge use without radial"))?;
        ([edgeuse, eu1], sides)
    } else {
        edgeuse_orientations(k, brep, edgeuse, OP)?
    };
    let (s0, id0) = face_surface(brep, eu0, OP)?;
    let (s1, id1) = face_surface(brep, eu1, OP)?;
    let [r0, r1] = radii;
    let [o0, o1] = orientations;
    let mut off0 = OffsetSurface::new(k, r0 * o0, s0, id0, SS_UNKNOWN)?;
    let mut off1 = OffsetSurface::new(k, r1 * o1, s1, id1, SS_UNKNOWN)?;
    off0.set_allow_extension(true);
    off1.set_allow_extension(true);

    let cr = ConstantRadiusAssistedFs::from_solver_parts(
        solver,
        radii,
        [Some(off0), Some(off1)],
        orientations,
        [Some(eu0), Some(eu1)],
    );
    cr.solver.finish_construction(k, pseudo)?; // Take care of other things in the base class.
    Ok(cr)
}

impl ConstantRadiusAssistedFs {
    /// `SmConstantRadiusFS(crContext, dThisApproxTol3d, dAngleTolerance,
    /// dTangencyTolerance, dOffsetRadiusSurface1, dOffsetRadiusSurface2, crSurface1,
    /// crSurface2, bOrientationSurface1, bOrientationSurface2)`: offset surfaces of the
    /// two surfaces (offset by radius times orientation, extension allowed); an
    /// orientation is `1.0`, or `-1.0` for `false`. No edge uses. The offsets take
    /// upstream's default `lSingularities`, `SM_SS_UNKNOWN`. The constructor's fillet
    /// geometry is stored in `pseudo` for solver `id` and receives copies of the
    /// offsets (`FinishConstruction`).
    ///
    /// # Errors
    ///
    /// Those of [`OffsetSurface::new`], [`FilletSolver::new_in`] and
    /// [`FilletSolver::finish_construction`].
    #[allow(clippy::too_many_arguments)]
    pub fn from_surfaces(
        k: &Kernel,
        pseudo: &mut FilletBrep,
        id: FilletSolverId,
        approx_tol_3d: f64,
        ang_tol_rad: f64,
        tangency_tolerance: f64,
        offset_radii: [f64; 2],
        surfaces: [(Surface, SurfaceId); 2],
        orientations: [bool; 2],
    ) -> Result<Self> {
        let solver = FilletSolver::new_in(
            k,
            pseudo,
            id,
            approx_tol_3d,
            ang_tol_rad,
            tangency_tolerance,
        )?;
        let o = orientations.map(|b| if b { 1.0 } else { -1.0 });
        let [(s0, id0), (s1, id1)] = surfaces;
        let [r0, r1] = offset_radii;
        let [o0, o1] = o;
        let mut off0 = OffsetSurface::new(k, r0 * o0, s0, id0, SS_UNKNOWN)?;
        let mut off1 = OffsetSurface::new(k, r1 * o1, s1, id1, SS_UNKNOWN)?;
        off0.set_allow_extension(true);
        off1.set_allow_extension(true);
        let cr = Self::from_solver_parts(
            solver,
            offset_radii,
            [Some(off0), Some(off1)],
            o,
            [None, None],
        );
        cr.solver.finish_construction(k, pseudo)?; // Take care of other things in the base class.
        Ok(cr)
    }

    /// `GetFilletRadius(lIndex)`.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an index above 1.
    pub fn fillet_radius(&self, index: usize) -> Result<f64> {
        self.fillet_radii
            .get(index)
            .copied()
            .ok_or_else(|| SolidError::invalid_input(OP, "radius index out of range"))
    }

    /// `SmConstantRadiusFS::OffsetRadiiCanChange`: never.
    #[must_use]
    pub const fn offset_radii_can_change(&self) -> bool {
        false
    }

    /// `SmConstantRadiusFS::GetJacobianSize`: 4.
    #[must_use]
    pub const fn jacobian_size(&self) -> usize {
        4
    }

    /// `SmConstantRadiusFS::GetSolverType`.
    #[must_use]
    pub const fn constant_radius_solver_type(&self) -> FilletSolverType {
        FilletSolverType::ConstRadius
    }

    /// `SmConstantRadiusFS::SetupOffsetValues`: sets both offset distances to radius
    /// times orientation and returns the rail 0 radius (`*pOffsetDist`).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for a missing offset surface.
    pub fn setup_offset_values(&mut self, _tsect_pnt: &TsectPnt) -> Result<f64> {
        // get offset distance
        let offset_dist = self.fillet_radius(0)?;

        // set offset surface offset distance - Orientation is +/- 1
        self.set_offset_by_radius(0)?;
        self.set_offset_by_radius(1)?;
        Ok(offset_dist)
    }

    /// `GetSurface(i)->SetOffsetDistance(m_dFilletRadii[i]*m_dOrientations[i])`.
    pub(crate) fn set_offset_by_radius(&mut self, rail: usize) -> Result<()> {
        let o = self
            .solver
            .orientations
            .get(rail)
            .copied()
            .ok_or_else(|| SolidError::invalid_input(OP, "rail index out of range"))?;
        let d = self.fillet_radius(rail)? * o;
        let s = self
            .solver
            .surfaces
            .get_mut(rail)
            .and_then(Option::as_mut)
            .map(|slot| &mut slot.surface)
            .ok_or_else(|| SolidError::invalid_input(OP, "no offset surface for the rail"))?;
        s.set_offset_distance(d);
        Ok(())
    }

    /// `SmConstantRadiusFS::LoadJacobian`: with `crX = [.. u1 v1 u2 v2 ..]` and `F`,
    /// `G` the points of the two offset surfaces, sets `rF[ne..ne+3] = F - G` and the
    /// Jacobian rows `ne..ne+3` (`[dF/du dF/dv -dG/du -dG/dv]` in the UV columns),
    /// advancing `num_equations` by 3 and `num_parameters` by 2 per surface whose UV
    /// point is not already in `crX` (one of `surfaces` with the same base, at its
    /// offset in `surf_offsets`); a `NULL` entry of `surfaces` receives the offset
    /// picked for its rail. The offset distances are reset to radius times orientation
    /// first. Returns `rbFoundAnswer`.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a zero first derivative of either offset surface; those of the
    /// evaluations;
    /// [`SolidError::InvalidInput`] for an out-of-range index (upstream reads or writes
    /// out of bounds) or a missing surface.
    pub fn load_jacobian(
        &mut self,
        k: &Kernel,
        num_equations: &mut usize,
        num_parameters: &mut usize,
        rail_index: usize,
        surfaces: JacobianSurfaces,
        surf_offsets: &mut [usize; 2],
        x: &[f64],
        f: &mut [f64],
        jacobian: Option<&mut Matrix>,
    ) -> Result<bool> {
        constant_radius_load_jacobian(
            self,
            k,
            num_equations,
            num_parameters,
            rail_index,
            surfaces,
            surf_offsets,
            x,
            f,
            jacobian,
        )
    }
}

/// Constant-radius Jacobian evaluated against the context's actual offset objects.
#[allow(clippy::too_many_arguments)]
pub(crate) fn constant_radius_load_jacobian<S: super::p2::ConstantRadiusOps + ?Sized>(
    solver: &mut S,
    k: &Kernel,
    num_equations: &mut usize,
    num_parameters: &mut usize,
    rail_index: usize,
    surfaces: JacobianSurfaces,
    surf_offsets: &mut [usize; 2],
    x: &[f64],
    f: &mut [f64],
    jacobian: Option<&mut Matrix>,
) -> Result<bool> {
    const OP: &str = OP_CR_JAC;
    // init output
    let mut found = false;
    let [surface1, surface2] = surfaces;

    // Always need to set the offset surface offset distances prior to solving
    solver.set_offset_by_radius(0)?;
    solver.set_offset_by_radius(1)?;

    let base0 = solver.offset_surface(0)?.base_id;
    let base1 = solver.offset_surface(1)?.base_id;

    // get lSurf1Off index into crX array for lSurf1 parameters
    let mut surf1_off = *num_parameters;
    *num_parameters = add(*num_parameters, 2, OP)?;
    if rail_index == 0 && surface1 == Some(base0) {
        surf1_off = surf_offsets[0];
        *num_parameters = sub(*num_parameters, 2, OP)?;
    } else if rail_index == 1 && surface2 == Some(base0) {
        surf1_off = surf_offsets[1];
        *num_parameters = sub(*num_parameters, 2, OP)?;
    }

    // get offsetSurface1 F Point values for given crX values
    let uv = uv_at(x, surf1_off, OP)?;
    let (p_f, du_f, dv_f) = offset_evaluate_1st(k, solver.offset_surface_mut(0)?, uv)?;

    // get lSurf2Off index into crX array for lSurf2 parameters
    let mut surf2_off = *num_parameters;
    *num_parameters = add(*num_parameters, 2, OP)?;
    if rail_index == 1 && surface1 == Some(base1) {
        surf2_off = surf_offsets[0];
        *num_parameters = sub(*num_parameters, 2, OP)?;
    }
    if rail_index == 0 && surface2 == Some(base1) {
        surf2_off = surf_offsets[1];
        *num_parameters = sub(*num_parameters, 2, OP)?;
    }

    // set rlSurfffset as needed
    if rail_index == 0 && surface1.is_none() {
        surf_offsets[0] = surf1_off;
    }
    if rail_index == 0 && surface2.is_none() {
        surf_offsets[1] = surf2_off;
    }
    if rail_index == 1 && surface1.is_none() {
        surf_offsets[1] = surf1_off;
    }
    if rail_index == 1 && surface2.is_none() {
        surf_offsets[0] = surf2_off;
    }

    // get offsetSurface2 G Point values for given crX values
    let uv2 = uv_at(x, surf2_off, OP)?;
    let (p_g, du_g, dv_g) = offset_evaluate_1st(k, solver.offset_surface_mut(1)?, uv2)?;

    // check surface tangent magnitudes
    // failure: zero length tangents
    if du_f.length_squared() < EFF_ZERO_SQ
        || dv_f.length_squared() < EFF_ZERO_SQ
        || du_g.length_squared() < EFF_ZERO_SQ
        || dv_g.length_squared() < EFF_ZERO_SQ
    {
        return Err(SolidError::sm_err(OP));
    }

    // set sDiff1 = distance between offset points
    let diff1 = p_f - p_g;

    if let Some(j) = jacobian {
        let s1 = add(surf1_off, 1, OP)?;
        let s2 = add(surf2_off, 1, OP)?;
        let mut i = *num_equations;
        for (a, b, c, d) in [
            (du_f.x, dv_f.x, du_g.x, dv_g.x),
            (du_f.y, dv_f.y, du_g.y, dv_g.y),
            (du_f.z, dv_f.z, du_g.z, dv_g.z),
        ] {
            set_j(j, i, surf1_off, a, OP)?;
            set_j(j, i, s1, b, OP)?;
            set_j(j, i, surf2_off, -c, OP)?;
            set_j(j, i, s2, -d, OP)?;
            i = add(i, 1, OP)?; // Move to next equation
        }
    }

    // Compute function values
    for v in [diff1.x, diff1.y, diff1.z] {
        set_f(f, *num_equations, v, OP)?;
        *num_equations = add(*num_equations, 1, OP)?;
    }

    // See if we have converged
    let scaled_tol = solver.constant_radius().solver.conversion_tol * (1.0 + p_f.max_dimension());
    if diff1.x.abs() < scaled_tol && diff1.y.abs() < scaled_tol && diff1.z.abs() < scaled_tol {
        found = true;
    }

    Ok(found)
}

// ---- SmSurfaceSurfaceFS -------------------------------------------------------------------

/// The rolling-ball fillet solver between two surfaces that need not share an edge
/// (`SmSurfaceSurfaceFS`): a constant-radius solver with start guesses on both
/// surfaces and a trimming type.
#[derive(Debug, Clone)]
pub struct SurfaceSurfaceFs {
    /// The `SmConstantRadiusFS` base.
    pub base: ConstantRadiusFs,
    /// `m_vUVGuesses`: where to start rolling the ball on each surface.
    pub uv_guesses: [Point2; 2],
    /// `m_eTrimType`: how to trim the fillet surface.
    pub trim_type: BoundaryTrimmingType,
}

impl SurfaceSurfaceFs {
    /// `SmSurfaceSurfaceFS::GetType`.
    pub const TYPE: i64 = SURFACE_SURFACE_FS_TYPE;

    /// `SetFilletTrimType`.
    pub fn set_fillet_trim_type(&mut self, trim_type: BoundaryTrimmingType) {
        self.trim_type = trim_type;
    }

    /// `SmSurfaceSurfaceFS::GetSolverType`.
    #[must_use]
    pub const fn solver_type(&self) -> FilletSolverType {
        FilletSolverType::SurfSurf
    }

    /// `SmSurfaceSurfaceFS_TYPE` (`GetType`).
    #[must_use]
    pub const fn type_code(&self) -> i64 {
        Self::TYPE
    }

    /// `"SmSurfaceSurfaceFS_TYPE"` (`GetTypeString`).
    #[must_use]
    pub const fn type_string(&self) -> &'static str {
        "SmSurfaceSurfaceFS_TYPE"
    }

    /// `SmSurfaceSurfaceFS::SetupOffsetExtension`: grows `domain` by the extension
    /// factor times its size at each corner (no growth in a periodic direction), only
    /// in `u` (`v`) at a corner that is singular in `u` (otherwise). Upstream's method
    /// is `void` and `IsPeriodic` / `IsSingularity` return an `SmBoolean`: a failing
    /// test reads as `TRUE` (singular in no known direction: the `v` branch), as in the
    /// base `SmFilletSolver::SetupOffsetExtension`.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from `IsPeriodic` or `IsSingularity` (other failures
    /// read as `TRUE`).
    pub fn setup_offset_extension(
        &self,
        k: &Kernel,
        surf: &Surface,
        domain: &mut Extent2d,
    ) -> Result<()> {
        // get length of extensions
        let mut size: Vec2 = self.base.solver.surface_extension_factor * domain.size();

        // Do not extend if periodic.
        let periodic_extension_factor = 0.0;

        if boolean_status(surf.is_periodic(k, domain, SurfParamType::U))? {
            size.x *= periodic_extension_factor;
        }
        if boolean_status(surf.is_periodic(k, domain, SurfParamType::V))? {
            size.y *= periodic_extension_factor;
        }

        // process the min domain boundaries
        let sing = singularity_status(surf.is_singularity(k, domain.min(), EFF_ZERO, false))?;
        if !sing.singular {
            // when surface is not singular - extend the min domain boundary
            domain.add_point(domain.min() - size);
        } else if sing.direction == SurfParamType::U {
            // singular in U direction only extend the u direction
            let mut new_min = domain.min();
            new_min.x -= size.x;
            domain.add_point(new_min);
        } else {
            // eSingDir == SM_SP_V
            // singular in V direction only extend the v direction
            let mut new_min = domain.min();
            new_min.y -= size.y;
            domain.add_point(new_min);
        }

        // process the max domain boundaries
        let sing = singularity_status(surf.is_singularity(k, domain.max(), EFF_ZERO, false))?;
        if !sing.singular {
            // when surface is not singular - extend the max domain boundary
            domain.add_point(domain.max() + size);
        } else if sing.direction == SurfParamType::U {
            // singular in U direction only extend the u direction
            let mut new_max = domain.max();
            new_max.x += size.x;
            domain.add_point(new_max);
        } else {
            // eSingDir == SM_SP_V
            // singular in V direction only extend the v direction
            let mut new_max = domain.max();
            new_max.y += size.y;
            domain.add_point(new_max);
        }
        Ok(())
    }
}

// ---- SmConstantDistanceFS -----------------------------------------------------------------

/// The constant-distance fillet/chamfer solver (`SmConstantDistanceFS`): every cross
/// section has the same chord length between the rails.
#[derive(Debug, Clone)]
pub struct ConstantDistanceFs {
    /// The `SmFilletSolver` base.
    pub solver: FilletSolver,
    /// `m_dDistance`: the distance between corresponding rail points.
    pub distance: f64,
}

/// The outputs of [`ConstantDistanceFs::load_initial_values`] besides the arrays it
/// updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DistanceInitialValues {
    /// `rpSurface2`: the rail whose offset surface is the second surface; `None` when
    /// it is not set (`rbDoSurf2Calcs` is `0`).
    pub surface2_rail: Option<usize>,
    /// `rlSurface2Index` after the call.
    pub surface2_index: usize,
}

/// `crSurface1` of [`ConstantDistanceFs::load_initial_values`]: which offset surface
/// the guess is on. Upstream's callers pass the solver's own rail surface
/// (`*GetSurface(lRailIndex)`), so what the call does to it (a base extension made by
/// its evaluation, which grows its natural UV domain) stays on the solver, and when it
/// is the other rail's surface it is `rpSurface2` itself.
#[derive(Debug)]
pub enum InitialSurface<'a> {
    /// The solver's own offset surface of a rail (`GetSurface(i)`).
    Rail(usize),
    /// Another offset surface.
    Other(&'a mut OffsetSurface),
}

/// The offset surface `surface1` denotes.
fn initial_surface<'b>(
    solver: &'b mut FilletSolver,
    surface1: &'b mut InitialSurface<'_>,
) -> Result<&'b mut OffsetSurface> {
    match surface1 {
        InitialSurface::Rail(i) => solver.rail_mut(*i),
        InitialSurface::Other(s) => Ok(&mut **s),
    }
}

impl ConstantDistanceFs {
    /// `SmConstantDistanceFS::GetType`.
    pub const TYPE: i64 = CONSTANT_DISTANCE_FS_TYPE;

    /// `CanMakeAnalytic`: always (the radius could change, but might not).
    #[must_use]
    pub const fn can_make_analytic(&self) -> bool {
        true
    }

    /// `GetDistance`.
    #[must_use]
    pub const fn distance(&self) -> f64 {
        self.distance
    }

    /// `SmConstantDistanceFS::GetJacobianSize`: 5.
    #[must_use]
    pub const fn jacobian_size(&self) -> usize {
        5
    }

    /// `SmConstantDistanceFS::GetSolverType`.
    #[must_use]
    pub const fn solver_type(&self) -> FilletSolverType {
        FilletSolverType::ConstDist
    }

    /// `SmConstantDistanceFS_TYPE` (`GetType`).
    #[must_use]
    pub const fn type_code(&self) -> i64 {
        Self::TYPE
    }

    /// `"SmConstantDistanceFS_TYPE"` (`GetTypeString`).
    #[must_use]
    pub const fn type_string(&self) -> &'static str {
        "SmConstantDistanceFS_TYPE"
    }

    /// `SmConstantDistanceFS::IsKindOf`: its own type or a type a fillet solver is.
    #[must_use]
    pub const fn is_kind_of(&self, t: i64) -> bool {
        t == Self::TYPE || FilletSolver::is_kind_of(t)
    }

    /// `SmConstantDistanceFS::SetupOffsetValues`: the fillet radius stored in the
    /// intersection point (`m_adUserDoubles[0]`) becomes both offset distances (times
    /// the orientations); returns it (`*pOffsetDist`).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for a missing offset surface.
    pub fn setup_offset_values(&mut self, tsect_pnt: &TsectPnt) -> Result<f64> {
        // get fillet radius at this point
        let radius = tsect_pnt
            .user_doubles
            .first()
            .copied()
            .ok_or_else(|| SolidError::invalid_input(OP, "no user doubles"))?;

        // save fillet radius and set OffsetSurface Offset distances
        self.solver.set_offsets(radius)?;
        Ok(radius)
    }

    /// `SmConstantDistanceFS::LoadJacobian`: with `crX = [.. u1 v1 u2 v2 R ..]`, `F`,
    /// `G` the base surface points, `nF`, `nG` their oriented unit normals and `L` the
    /// distance, sets `rF[ne..ne+4] = [F - G + R (nF - nG), (F-G).(F-G) - L^2]` and the
    /// Jacobian rows `ne..ne+4` (UV columns and the radius column
    /// `max(uv offsets) + 2`), advancing `num_equations` by 4 and `num_parameters` by 2
    /// per surface whose UV point is not already in `crX` (one of `surfaces` with the
    /// same base, at its offset in `surf_offsets`, which are only read) and by 1 for
    /// the radius. Both offset distances become `R` (times the orientations) unless the
    /// rail points are more than `10 L` apart. Returns `rbFoundAnswer`.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a zero first derivative or normal of either base surface or rail
    /// points more than `10 L` apart; `SM_ERR_INVALID_INPUT` from `Unitize`; those of
    /// the evaluations; [`SolidError::InvalidInput`] for an out-of-range index
    /// (upstream reads or writes out of bounds) or a missing surface.
    pub fn load_jacobian(
        &mut self,
        k: &Kernel,
        num_equations: &mut usize,
        num_parameters: &mut usize,
        rail_index: usize,
        surfaces: JacobianSurfaces,
        surf_offsets: [usize; 2],
        x: &[f64],
        f: &mut [f64],
        jacobian: Option<&mut Matrix>,
    ) -> Result<bool> {
        const OP: &str = OP_CD_JAC;
        // init output
        let mut found = false;
        let [surface1, surface2] = surfaces;
        let base0 = self.solver.rail(0)?.base_id;
        let base1 = self.solver.rail(1)?.base_id;

        // get lSurf1Off index into crX array for lSurf1 parameters
        let mut surf1_off = *num_parameters;
        if rail_index == 0 && surface1 == Some(base0) {
            surf1_off = surf_offsets[0];
        } else if rail_index == 1 && surface2 == Some(base0) {
            surf1_off = surf_offsets[1];
        } else {
            *num_parameters = add(*num_parameters, 2, OP)?;
        }

        // get lSurf2Off index into crX array for lSurf2 parameters
        let mut surf2_off = *num_parameters;
        if rail_index == 1 && surface1 == Some(base1) {
            surf2_off = surf_offsets[0];
        } else if rail_index == 0 && surface2 == Some(base1) {
            surf2_off = surf_offsets[1];
        } else {
            *num_parameters = add(*num_parameters, 2, OP)?;
        }

        // get baseSurface1 F point values for given crX values
        let uv = uv_at(x, surf1_off, OP)?;
        self.solver.rail_mut(0)?.set_offset_distance(0.0);
        let [p_f, du_f, dv_f, duv_f, duu_f, dvv_f] =
            offset_evaluate_2nd(k, self.solver.rail_mut(0)?, uv)?;

        // get baseSurface2 G point values for given crX values
        let uv2 = uv_at(x, surf2_off, OP)?;
        self.solver.rail_mut(1)?.set_offset_distance(0.0);
        let [p_g, du_g, dv_g, duv_g, duu_g, dvv_g] =
            offset_evaluate_2nd(k, self.solver.rail_mut(1)?, uv2)?;

        // get current radius value
        let radius = x_at(x, *num_parameters, OP)?;
        *num_parameters = add(*num_parameters, 1, OP)?;

        // failure: zero length tangents
        if du_f.length_squared() < EFF_ZERO_SQ
            || dv_f.length_squared() < EFF_ZERO_SQ
            || du_g.length_squared() < EFF_ZERO_SQ
            || dv_g.length_squared() < EFF_ZERO_SQ
        {
            return Err(SolidError::sm_err(OP));
        }

        // get baseSurface SurfaceNormals at current solution points
        let mut n_f = du_f.cross(dv_f); // Normal on base surface #1
        let mut n_g = du_g.cross(dv_g); // Normal on base surface #2

        // failure: zero length SurfaceNormal
        if n_f.length_squared() < EFF_ZERO_SQ || n_g.length_squared() < EFF_ZERO_SQ {
            return Err(SolidError::sm_err(OP));
        }

        // get derivatives of SurfaceNormals
        let n_f_du = du_f.derivative_of_cross_product(dv_f, duu_f, duv_f);
        let n_f_dv = du_f.derivative_of_cross_product(dv_f, duv_f, dvv_f);
        let n_g_du = du_g.derivative_of_cross_product(dv_g, duu_g, duv_g);
        let n_g_dv = du_g.derivative_of_cross_product(dv_g, duv_g, dvv_g);

        // normalize SurfaceNormal derivatives
        let o0 = self.solver.orientation(0)?;
        let o1 = self.solver.orientation(1)?;
        let unit_n_f_du = o0 * n_f.unitized_derivative(n_f_du);
        let unit_n_f_dv = o0 * n_f.unitized_derivative(n_f_dv);
        let unit_n_g_du = o1 * n_g.unitized_derivative(n_g_du);
        let unit_n_g_dv = o1 * n_g.unitized_derivative(n_g_dv);

        // orient and normalize SurfaceNormal vectors
        n_f = (n_f * o0).unitize()?;
        n_g = (n_g * o1).unitize()?;

        // set sDiff1 = vector between current rail points (desire sDiff1.Length == m_dDistance
        let diff1 = p_f - p_g;

        // set sDiff2 = vector between points on the fillet center curve (desire sDiff2.Length == 0.0)
        let diff2 = diff1 + radius * (n_f - n_g);

        if let Some(j) = jacobian {
            let mut i = *num_equations;
            let last_column = add(surf2_off.max(surf1_off), 2, OP)?;
            let s1 = add(surf1_off, 1, OP)?;
            let s2 = add(surf2_off, 1, OP)?;

            for (a, b, c, d, e) in [
                (
                    du_f.x + radius * unit_n_f_du.x,
                    dv_f.x + radius * unit_n_f_dv.x,
                    -du_g.x - radius * unit_n_g_du.x,
                    -dv_g.x - radius * unit_n_g_dv.x,
                    n_f.x - n_g.x,
                ),
                (
                    du_f.y + radius * unit_n_f_du.y,
                    dv_f.y + radius * unit_n_f_dv.y,
                    -du_g.y - radius * unit_n_g_du.y,
                    -dv_g.y - radius * unit_n_g_dv.y,
                    n_f.y - n_g.y,
                ),
                (
                    du_f.z + radius * unit_n_f_du.z,
                    dv_f.z + radius * unit_n_f_dv.z,
                    -du_g.z - radius * unit_n_g_du.z,
                    -dv_g.z - radius * unit_n_g_dv.z,
                    n_f.z - n_g.z,
                ),
                (
                    2.0 * diff1.dot(du_f),
                    2.0 * diff1.dot(dv_f),
                    -2.0 * diff1.dot(du_g),
                    -2.0 * diff1.dot(dv_g),
                    0.0,
                ),
            ] {
                set_j(j, i, surf1_off, a, OP)?;
                set_j(j, i, s1, b, OP)?;
                set_j(j, i, surf2_off, c, OP)?;
                set_j(j, i, s2, d, OP)?;
                set_j(j, i, last_column, e, OP)?;
                i = add(i, 1, OP)?; // Move to next equation
            }
        }

        // Compute function values - distance vector between offset points on fillet center curve
        //                           distance between points on rail curves
        let value = diff1.dot(diff1) - self.distance * self.distance;
        for v in [diff2.x, diff2.y, diff2.z, value] {
            set_f(f, *num_equations, v, OP)?;
            *num_equations = add(*num_equations, 1, OP)?;
        }

        // failure: railPoints more than 10*m_Distance apart
        if value.abs().sqrt() > 10.0 * self.distance {
            return Err(SolidError::sm_err(OP)); // No convergance just skip out of here
        }

        // set offset surface distances = current radius solution
        self.solver.set_offsets(radius)?;

        // See if we have converged
        let scaled_tol = self.solver.conversion_tol * (1.0 + p_f.max_dimension());
        if diff2.x.abs() < scaled_tol
            && diff2.y.abs() < scaled_tol
            && diff2.z.abs() < scaled_tol
            && value.abs() < scaled_tol
        {
            found = true;
        }

        Ok(found)
    }

    /// `SmConstantDistanceFS::LoadInitialValues`. With `do_surf2_calcs`: the point of
    /// `surface1` at the guess `rGuessT[surface1_index..+2]` is carried to the other
    /// rail's offset surface (through the other edge use's UV trim curve when it has
    /// one on that surface, else by a global point solve); that surface's base domain
    /// extended by the extension factor becomes two new intervals at
    /// `rIntervals[len(rGuessT)..]`, two `false` periodicities and the new UV guess are
    /// appended, and `surface2_index` becomes the old guess count. Then always:
    /// `rIntervals[surface2_index + 2] = [0, 10 L]`, a `false` periodicity, both
    /// offset distances `L sqrt(2) / 2` (times the orientations), and that radius
    /// appended to the guesses.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when a point solve finds no single solution; those of the evaluations
    /// and solves;
    /// [`SolidError::InvalidInput`] for an out-of-range index (upstream reads or writes
    /// out of bounds), a rail index above 1, a missing surface, or edge uses without
    /// `brep`.
    pub fn load_initial_values(
        &mut self,
        k: &Kernel,
        brep: Option<&Brep>,
        do_surf2_calcs: bool,
        rail_index: usize,
        mut surface1: InitialSurface<'_>,
        surface1_index: usize,
        surface2_index: usize,
        intervals: &mut ExtentNd,
        periodicities: &mut Vec<bool>,
        guess_t: &mut Vec<f64>,
    ) -> Result<DistanceInitialValues> {
        const OP: &str = OP_CD_INIT;
        let mut surface2_rail = None;
        let mut surface2_index = surface2_index;
        if do_surf2_calcs {
            // Now find the UV guess on the other Rail's surface
            let uv = uv_at(guess_t, surface1_index, OP)?;
            let pnt =
                offset_evaluate_point(k, initial_surface(&mut self.solver, &mut surface1)?, uv)?;

            if rail_index > 1 {
                return Err(SolidError::invalid_input(OP, "rail index out of range"));
            }
            let other_rail = 1 - rail_index;
            surface2_rail = Some(other_rail);
            let uv2 = self.surface2_guess(k, brep, other_rail, &mut surface1, uv, pnt)?;

            let domain = self
                .solver
                .rail(other_rail)?
                .base_surface()
                .natural_uv_domain_with_nurb(k)?;
            let size: Vec2 = self.solver.surface_extension_factor * domain.size();

            let n = guess_t.len();
            *interval_mut(intervals, n, OP)? =
                Extent1d::from_min_max(domain.min().x - size.x, domain.max().x + size.x);
            *interval_mut(intervals, add(n, 1, OP)?, OP)? =
                Extent1d::from_min_max(domain.min().y - size.y, domain.max().y + size.y);

            periodicities.push(false);
            periodicities.push(false);

            surface2_index = n;

            guess_t.push(uv2.x);
            guess_t.push(uv2.y);
        }

        // Last of all load the distance
        *interval_mut(intervals, add(surface2_index, 2, OP)?, OP)? =
            Extent1d::from_min_max(0.0, self.distance * 10.0);
        periodicities.push(false);
        let approx_radius = self.distance * 2.0_f64.sqrt() / 2.0;
        self.solver.set_offsets(approx_radius)?;

        guess_t.push(approx_radius);

        Ok(DistanceInitialValues {
            surface2_rail,
            surface2_index,
        })
    }

    /// The UV guess on the other rail's surface for the point `pnt` of `surface1` at
    /// `uv` (the `rbDoSurf2Calcs` body of `LoadInitialValues` up to `s2DPnt`). The
    /// other rail's base (and `surface1`'s, when it is the same surface) evaluates out
    /// of bounds meanwhile (`SmTemporaryChangeValue`, restored on every exit).
    fn surface2_guess(
        &mut self,
        k: &Kernel,
        brep: Option<&Brep>,
        other_rail: usize,
        surface1: &mut InitialSurface<'_>,
        uv: Point2,
        pnt: Point3,
    ) -> Result<Point2> {
        let base2_id = self.solver.rail(other_rail)?.base_id;
        // (`rpSurface2` itself: its base is the one changed)
        let aliased = matches!(surface1, InitialSurface::Rail(i) if *i == other_rail);
        let same_base =
            !aliased && initial_surface(&mut self.solver, surface1)?.base_id == base2_id;
        let saved2 = enable_out_of_bounds(self.solver.rail_mut(other_rail)?);
        let saved1 = if same_base {
            enable_out_of_bounds(initial_surface(&mut self.solver, surface1)?)
        } else {
            None
        };
        let r = self.surface2_guess_body(k, brep, other_rail, surface1, uv, pnt);
        if let Some(old) = saved1 {
            set_out_of_bounds_enabled(
                initial_surface(&mut self.solver, surface1)?
                    .surface
                    .base_surface_mut(),
                old,
            );
        }
        if let Some(old) = saved2 {
            set_out_of_bounds_enabled(
                self.solver.rail_mut(other_rail)?.surface.base_surface_mut(),
                old,
            );
        }
        r
    }

    /// [`Self::surface2_guess`] inside the temporary change.
    fn surface2_guess_body(
        &mut self,
        k: &Kernel,
        brep: Option<&Brep>,
        other_rail: usize,
        surface1: &mut InitialSurface<'_>,
        uv: Point2,
        pnt: Point3,
    ) -> Result<Point2> {
        const OP: &str = OP_CD_INIT;
        let base2_id = self.solver.rail(other_rail)?.base_id;
        let approx_tol_3d = self.solver.approx_tol_3d;

        // If we have edgeuses then use them to compute a point on the other surfaces
        // If not then drop the point to the other surface.
        let mut found_uv = false;
        let mut uv2 = Point2::new(0.0, 0.0);
        let mut other_eu = self.solver.edgeuses.get(other_rail).copied().flatten();
        let need_brep = || SolidError::invalid_input(OP, "edge uses without their B-rep");
        if let Some(eu) = other_eu {
            let b = brep.ok_or_else(need_brep)?;
            if edgeuse::uv_trim_curve(b, eu)?.0.is_none() {
                other_eu = None;
            }
        }

        // But: if we are rolling on a different face (e.g., doing a rollover),
        // then this is the wrong Edgeuse, and we don't have an appropriate one to use.
        if let Some(eu) = other_eu {
            let b = brep.ok_or_else(need_brep)?;
            if let Some(f) = edgeuse::face(b, eu)? {
                let same = b.face(f)?.surface.is_some() && base2_id == SurfaceId::Face(f);
                if !same {
                    other_eu = None;
                }
            }
        }

        if let Some(eu) = other_eu {
            let b = brep.ok_or_else(need_brep)?;
            let edge_id = edgeuse::edge(b, eu)?
                .ok_or_else(|| SolidError::invalid_input(OP, "edge use without edge"))?;
            let edge = b.edge(edge_id)?;
            let curve = edge
                .curve
                .as_ref()
                .ok_or_else(|| SolidError::invalid_input(OP, "edge without curve"))?;
            let ivl = edge.interval;
            let sols = curve.global_point_solve(
                k,
                &ivl,
                SolverOperation::Minimize,
                pnt,
                approx_tol_3d,
                SolutionRequested::Single,
                &GlobalPointSolveOptions::<crate::topology::ObjectHandle>::default(),
            )?;
            if sols.len() != 1 {
                return Err(SolidError::sm_err(OP));
            }
            let mut t = sols
                .get(0)
                .ok_or_else(|| SolidError::sm_err(OP))?
                .start
                .param(0)?;

            // If the curve is closed, and the solution is within tol of an
            // end point, snap the solution to whichever end (start or finish)
            // such that the direction that moves towards the interior of the
            // curve also moves generally towards the interior of the surface.
            if curve.is_closed(k, &ivl, 0.0)? {
                let p3 = curve.evaluate_point(k, t)?;
                let inward = offset_compute_inward_vector(
                    k,
                    initial_surface(&mut self.solver, surface1)?,
                    uv,
                )?;
                let dropped = curve.drop_point(
                    k,
                    &ivl,
                    p3,
                    Some(inward),
                    approx_tol_3d,
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
            let uv_curve = uv_curve
                .ok_or_else(|| SolidError::invalid_input(OP, "edge use without UV curve"))?;
            let p2d = uv_curve.evaluate_point(k, t)?;
            uv2 = Point2::new(p2d.x, p2d.y);
            found_uv = true;
            let surface2 = self.solver.rail(other_rail)?;
            let domain2 = offset_natural_uv_domain(surface2);
            if let Some((sol_uv, dist)) = offset_local_point_solve(k, surface2, &domain2, pnt, uv2)?
            {
                if dist < 2.0 * self.distance {
                    uv2 = sol_uv;
                }
            }
        }

        if found_uv {
            let flags = EvaluateFlags {
                u_side: Side::Left,
                v_side: Side::Left,
                only_upper_half: true,
                non_zero_tangents: true,
            };
            let base2 = self.solver.rail(other_rail)?.base_surface();
            let mat = base2.evaluate(k, uv2, 1, 1, flags)?;
            let du = mat
                .get(1)
                .and_then(|r| r.first())
                .copied()
                .ok_or_else(|| SolidError::sm_err(OP))?;
            let dv = mat
                .first()
                .and_then(|r| r.get(1))
                .copied()
                .ok_or_else(|| SolidError::sm_err(OP))?;
            let mut deg = du.angle_between(dv)?;
            deg *= 180.0 / PI;
            // (not a range test: a NaN angle keeps the guess, as upstream)
            #[allow(clippy::manual_range_contains)]
            if deg < 30.0 || deg > 150.0 {
                found_uv = false;
            }
            // If we are near a singularity then use global point solve
            if du.max_dimension() > 10000.0 * dv.max_dimension()
                || dv.max_dimension() > 10000.0 * du.max_dimension()
            {
                found_uv = false;
            }
        }

        if !found_uv {
            let surface2 = self.solver.rail(other_rail)?;
            let domain2 = offset_natural_uv_domain(surface2);
            let sols = surface_global_point_solve(
                k,
                &Surface::from(surface2.surface.clone()),
                None,
                &domain2,
                pnt,
                EFF_ZERO_SQ,
            )?;
            if sols.len() != 1 {
                return Err(SolidError::sm_err(OP));
            }
            uv2 = sols
                .first()
                .copied()
                .ok_or_else(|| SolidError::sm_err(OP))?;
        }
        Ok(uv2)
    }
}

/// Turns on out-of-bounds evaluation of an offset's base when it is an
/// `SmBSplineSurface` (a B-spline surface, a plane or a surface of revolution): the
/// value to restore, `None` for other kinds (`SmTemporaryChangeValue` then changes a
/// local).
fn enable_out_of_bounds(s: &mut OffsetSurface) -> Option<bool> {
    let base = s.surface.base_surface_mut();
    let old = out_of_bounds_enabled(base)?;
    set_out_of_bounds_enabled(base, true);
    Some(old)
}

// ---- SmCurveBasedFS -----------------------------------------------------------------------

/// The fillet solvers that follow an edge or curve (`SmCurveBasedFS`): the original
/// curve (`m_pOriginal`, here owned) and its extension (`m_pExtended`).
#[derive(Debug, Clone)]
pub struct CurveBasedFs {
    /// The `SmFilletSolver` base.
    pub solver: FilletSolver,
    /// `m_pOriginal`: the curve of the edge being filleted.
    pub original: Option<Curve>,
    /// `m_pExtended`: the extended edge curve, when one was made.
    pub extended: Option<BSplineCurve>,
}

/// The `SmBSplineCurve` part of a curve that is one (`IsKindOf(SmBSplineCurve_TYPE)`:
/// a B-spline curve, line, ellipse or circle, parabola or hyperbola), with its NURB
/// (`GetOrCreateGwNurbPointer`: a line makes its NURB on a copy).
fn bspline_part(c: &Curve) -> Result<Option<std::borrow::Cow<'_, BSplineCurve>>> {
    Ok(match c.kind() {
        CurveKind::BSpline(b) => Some(std::borrow::Cow::Borrowed(b)),
        CurveKind::Line(l) => Some(l.base_with_nurb()?),
        CurveKind::Ellipse(e) => Some(std::borrow::Cow::Borrowed(e.base())),
        CurveKind::Parabola(p) => Some(std::borrow::Cow::Borrowed(p.base())),
        CurveKind::Hyperbola(h) => Some(std::borrow::Cow::Borrowed(h.base())),
        _ => None,
    })
}

impl CurveBasedFs {
    /// `SmCurveBasedFS::GetType`.
    pub const TYPE: i64 = CURVE_BASED_FS_TYPE;

    /// `SmCurveBasedFS(crContext, dThisApproxTol3d, dAngleTolerance,
    /// dTangencyTolerance)`: no original or extended curve. The base is
    /// [`FilletSolver::new`], without the constructor's fillet geometry
    /// ([`CurveBasedFs::new_in`] adds it).
    #[must_use]
    pub fn new(approx_tol_3d: f64, ang_tol_rad: f64, tangency_tolerance: f64) -> Self {
        Self {
            solver: FilletSolver::new(approx_tol_3d, ang_tol_rad, tangency_tolerance),
            original: None,
            extended: None,
        }
    }

    /// `SmCurveBasedFS(crContext, dThisApproxTol3d, dAngleTolerance,
    /// dTangencyTolerance)` with the base's fillet geometry: [`FilletSolver::new_in`]
    /// in `pseudo`, with `id` as the solver's identity.
    ///
    /// # Errors
    ///
    /// Those of [`FilletSolver::new_in`].
    pub fn new_in(
        k: &Kernel,
        pseudo: &mut FilletBrep,
        id: FilletSolverId,
        approx_tol_3d: f64,
        ang_tol_rad: f64,
        tangency_tolerance: f64,
    ) -> Result<Self> {
        Ok(Self {
            solver: FilletSolver::new_in(
                k,
                pseudo,
                id,
                approx_tol_3d,
                ang_tol_rad,
                tangency_tolerance,
            )?,
            original: None,
            extended: None,
        })
    }

    /// `GetOriginalCurve`.
    #[must_use]
    pub const fn original_curve(&self) -> Option<&Curve> {
        self.original.as_ref()
    }

    /// `SetOriginalCurve`.
    pub fn set_original_curve(&mut self, curve: Option<Curve>) {
        self.original = curve;
    }

    /// `SmCurveBasedFS::GetSolverType`.
    #[must_use]
    pub const fn solver_type(&self) -> FilletSolverType {
        FilletSolverType::CurveBased
    }

    /// `SmCurveBasedFS_TYPE` (`GetType`).
    #[must_use]
    pub const fn type_code(&self) -> i64 {
        Self::TYPE
    }

    /// `"SmCurveBasedFS_TYPE"` (`GetTypeString`).
    #[must_use]
    pub const fn type_string(&self) -> &'static str {
        "SmCurveBasedFS_TYPE"
    }

    /// `SmCurveBasedFS::IsKindOf`: its own type or a type a fillet solver is.
    #[must_use]
    pub const fn is_kind_of(&self, t: i64) -> bool {
        t == Self::TYPE || FilletSolver::is_kind_of(t)
    }

    /// `SmCurveBasedFS::EvaluateBaseCurve`: the point and derivatives
    /// `0..=num_derivs` of the extended curve (when there is one) or the original
    /// curve at `parameter`. A parameter within `SM_EFF_ZERO * (1 + |min| + |max|)`
    /// outside that curve's interval snaps to its end; one further out extends the
    /// original (a B-spline based curve) with `G1` continuity to twice the overshoot,
    /// at least a tenth of the interval length past the end, and keeps the extension.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when a parameter outside the interval needs a curve that is not
    /// B-spline based extended; those of the extension and the evaluation;
    /// [`SolidError::InvalidInput`] without original curve (upstream dereferences
    /// `NULL`).
    pub fn evaluate_base_curve(
        &mut self,
        k: &Kernel,
        parameter: f64,
        num_derivs: usize,
    ) -> Result<Vec<Vec3>> {
        const OP: &str = OP_BASE;
        let original = self
            .original
            .as_ref()
            .ok_or_else(|| SolidError::invalid_input(OP, "no original curve"))?;

        // pick curve to use
        let mut use_extended = self.extended.is_some();

        // get curve interval
        let ivl = match &self.extended {
            Some(e) => e.natural_interval(),
            None => original.natural_interval(),
        };
        let mut parameter = parameter;

        // when parameter is not within interval
        if !ivl.contains_value(parameter, 0.0) {
            // pick parameter space tolerance
            let scaled_tol = EFF_ZERO * (1.0 + ivl.min().abs() + ivl.max().abs());

            // snap to begin point
            if parameter < ivl.min() && parameter + scaled_tol > ivl.min() {
                parameter = ivl.min();
            }

            // snap to end point
            if parameter > ivl.max() && parameter - scaled_tol < ivl.max() {
                parameter = ivl.max();
            }

            // when parameter is still outside interval
            if !ivl.contains_value(parameter, 0.0) {
                // Works better to extend the original: extending the extension
                // results in progressively smaller extensions.  [B551]
                // when curve is a BSpline
                let Some(bsc) = bspline_part(original)? else {
                    // Unable to extend non BSpline curves
                    return Err(SolidError::sm_err(OP));
                };
                // But extend farther than requested, so that we won't have to do this too often.
                // This doubles the extension distance:
                let mut extend_param = if parameter < ivl.min() {
                    parameter - (ivl.min() - parameter)
                } else {
                    parameter + (parameter - ivl.max())
                };
                // But never do a really tiny extension: it might get snapped to the end,
                // and therefore not extended at all.  [Fillet 308]
                let len = ivl.length();
                if extend_param < ivl.min() {
                    if extend_param > ivl.min() - len / 10.0 {
                        extend_param = ivl.min() - len / 10.0;
                    }
                } else if extend_param < ivl.max() + len / 10.0 {
                    extend_param = ivl.max() + len / 10.0;
                }
                let extended = bsc.create_extended_curve(extend_param, ContinuityType::G1, true)?;
                // (upstream evaluates a NULL extension: it is always made here, the
                // parameter lies outside the original's interval)
                let extended = extended.ok_or_else(|| {
                    SolidError::invalid_input(OP, "extension parameter inside the interval")
                })?;
                self.extended = Some(extended);
                use_extended = true;
            }
        }

        // evaluate the curve (`SmCurve::Evaluate`'s default `bNonZeroTangents = TRUE`)
        match (&self.extended, use_extended) {
            (Some(e), true) => e.evaluate(parameter, num_derivs, true, true),
            _ => self
                .original
                .as_ref()
                .ok_or_else(|| SolidError::invalid_input(OP, "no original curve"))?
                .evaluate(k, parameter, num_derivs, true, true),
        }
    }
}

// ---- co-ported fillet laws ----------------------------------------------------------------

/// The kind of a fillet law (CO-PORTED from unit `blend.fillet_executive_p1`): the
/// data of `SmFilletLaw` and its subclasses the variable-radius solver reads.
#[derive(Debug, Clone)]
pub enum FilletLawKind {
    /// `SmFilletLaw` itself.
    Base,
    /// `SmLinearFilletLaw`: the radius runs linearly from `start_radius` to
    /// `end_radius` (`m_dStartValue`, `m_dEndValue`).
    Linear {
        /// `GetStartRadius`.
        start_radius: f64,
        /// `GetEndRadius`.
        end_radius: f64,
    },
    /// `SmBSplineFilletLaw`: the radius is the x coordinate of `law_curve`
    /// (`m_pLawCurve`).
    BSpline {
        /// `GetLawCurve`.
        law_curve: BSplineCurve,
        /// `m_pExtendedLaw` (`GetExtendedCurve`): the extension of `law_curve` the
        /// evaluation creates once a parameter leaves the curve's domain
        /// (`blend::fillet_executive::p1`).
        extended_law: Option<BSplineCurve>,
    },
}

/// A fillet law (`SmFilletLaw`, CO-PORTED from unit `blend.fillet_executive_p1`,
/// whose [`FilletLaw::evaluate`](crate::blend::fillet_executive::p1) lives in that
/// unit): the radius along the filleted edge and the extended edge curve it keeps.
#[derive(Debug, Clone)]
pub struct FilletLaw {
    /// The law's class and its data.
    pub kind: FilletLawKind,
    /// `m_vEdgeMap`: the subset of the radius function that maps to the edge
    /// (`[0, 1]` by default).
    pub edge_map: Extent1d,
    /// `m_dEdgeArcLength` (`SetEdgeArcLength`).
    pub edge_arc_length: f64,
    /// `m_pExtended`: the extended edge curve.
    pub extended: Option<BSplineCurve>,
}

impl FilletLaw {
    /// A law of `kind` with the default edge map `[0, 1]`, no arc length and no
    /// extended edge curve (`SmFilletLaw::SmFilletLaw`).
    #[must_use]
    pub const fn new(kind: FilletLawKind) -> Self {
        Self {
            kind,
            edge_map: Extent1d::from_raw(0.0, 1.0),
            edge_arc_length: 0.0,
            extended: None,
        }
    }

    /// `IsLinearLaw`: a linear law, or a B-spline law of degree 1.
    ///
    /// # Errors
    ///
    /// Those of the law curve's `GetDegree`.
    pub fn is_linear_law(&self) -> Result<bool> {
        Ok(match &self.kind {
            FilletLawKind::Base => false,
            FilletLawKind::Linear { .. } => true,
            FilletLawKind::BSpline { law_curve, .. } => law_curve.degree()? == 1,
        })
    }

    /// `GetExtendedEdgeCurve`.
    #[must_use]
    pub const fn extended_edge_curve(&self) -> Option<&BSplineCurve> {
        self.extended.as_ref()
    }

    /// `SetExtendedEdgeCurve`.
    pub fn set_extended_edge_curve(&mut self, curve: Option<BSplineCurve>) {
        self.extended = curve;
    }
}

// ---- SmVariableRadiusFS -------------------------------------------------------------------

/// The variable-radius fillet solver (`SmVariableRadiusFS`): the radius follows a law
/// over the parameter of the filleted edge.
#[derive(Debug, Clone)]
pub struct VariableRadiusFs {
    /// The `SmCurveBasedFS` base.
    pub curve_based: CurveBasedFs,
    /// `m_rLaw`: the radius law (owned, as upstream deletes it with the solver).
    pub law: FilletLaw,
    /// `m_bLawOrientation`: the law runs against the edge.
    pub law_orientation: bool,
    /// `m_vLawInterval`: the edge interval the law maps.
    pub law_interval: Extent1d,
}

impl VariableRadiusFs {
    /// `SmVariableRadiusFS::GetType`.
    pub const TYPE: i64 = VARIABLE_RADIUS_FS_TYPE;

    /// `GetFilletLaw`.
    #[must_use]
    pub const fn fillet_law(&self) -> &FilletLaw {
        &self.law
    }

    /// `GetFilletLawOrientation`.
    #[must_use]
    pub const fn fillet_law_orientation(&self) -> bool {
        self.law_orientation
    }

    /// `GetFilletLawInterval`.
    #[must_use]
    pub const fn fillet_law_interval(&self) -> Extent1d {
        self.law_interval
    }

    /// `SetFilletLawInterval`.
    pub fn set_fillet_law_interval(&mut self, interval: Extent1d) {
        self.law_interval = interval;
    }

    /// `SmVariableRadiusFS::GetJacobianSize`: 5.
    #[must_use]
    pub const fn jacobian_size(&self) -> usize {
        5
    }

    /// `SmVariableRadiusFS::GetSolverType`.
    #[must_use]
    pub const fn solver_type(&self) -> FilletSolverType {
        FilletSolverType::VariableRadius
    }

    /// `SmVariableRadiusFS_TYPE` (`GetType`).
    #[must_use]
    pub const fn type_code(&self) -> i64 {
        Self::TYPE
    }

    /// `"SmVariableRadiusFS_TYPE"` (`GetTypeString`).
    #[must_use]
    pub const fn type_string(&self) -> &'static str {
        "SmVariableRadiusFS_TYPE"
    }

    /// `SmVariableRadiusFS::OffsetRadiiCanChange`: false only for a circular cross
    /// section with a linear law whose end radii agree within `SM_EFF_ZERO`.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] without surface generator (upstream dereferences
    /// `NULL`); those of `IsLinearLaw`.
    pub fn offset_radii_can_change(&self) -> Result<bool> {
        let mut ret = true;
        let fsg = self
            .curve_based
            .solver
            .fillet_surface_generator()
            .map(FilletSurfaceGenerator::fillet_surface_generator_type)
            .ok_or_else(|| SolidError::invalid_input(OP_VR, "no fillet surface generator"))?;
        if fsg == FilletSurfaceGeneratorType::Circular && self.law.is_linear_law()? {
            if let FilletLawKind::Linear {
                start_radius,
                end_radius,
            } = self.law.kind
            {
                if (start_radius - end_radius).abs() < EFF_ZERO {
                    ret = false;
                }
            }
        }
        Ok(ret)
    }

    /// `SmVariableRadiusFS::SetExtensionFlags`: the base-class flags, then, for a
    /// B-spline based edge curve of degree above 1 that is not a line and has no
    /// extended curve yet, the law's extended edge curve: the curve extended
    /// (`C-infinity`) by its interval length at both ends.
    ///
    /// # Errors
    ///
    /// Those of `SmFilletSolver::SetExtensionFlags` ([`FilletSolver::set_extension_flags`]
    /// on `b`, the B-rep of the edge uses); [`SolidError::InvalidInput`] without original
    /// curve or when the first extension fails (upstream then dereferences `NULL`).
    pub fn set_extension_flags(
        &mut self,
        k: &Kernel,
        b: &Brep,
        corners: &dyn ExtensionCorners,
    ) -> Result<()> {
        self.curve_based.solver.set_extension_flags(b, corners)?;
        self.extend_edge_curve(k)
    }

    /// The `SmVariableRadiusFS` part of `SetExtensionFlags`.
    fn extend_edge_curve(&mut self, k: &Kernel) -> Result<()> {
        // In order to help out computing fillet radius outside the curve boundary
        // when the filleted edge is not linear. Let's extend the edge.
        let original = self
            .curve_based
            .original
            .as_ref()
            .ok_or_else(|| SolidError::invalid_input(OP_VR, "no original curve"))?;
        let Some(bsc) = bspline_part(original)? else {
            return Ok(());
        };
        if self.law.extended_edge_curve().is_none()
            && original.degree()? > 1
            && original.is_line(k, 5, ANALYTIC_TOL)?.is_none()
        {
            // Should come in here only once.
            let ivl = original.natural_interval();
            let precise_extension = true; // Otherwise it adds another length of sIvl.
            let new_t = ivl.min() - ivl.length();
            // (status only signalled upstream; a failure leaves a NULL curve it then
            // dereferences)
            let first = ignored(bsc.create_extended_curve(
                new_t,
                ContinuityType::CInfinity,
                precise_extension,
            ))?
            .flatten()
            .ok_or_else(|| SolidError::invalid_input(OP_VR, "edge curve extension failed"))?;
            let new_t = ivl.max() + ivl.length();
            let second = ignored(first.create_extended_curve(
                new_t,
                ContinuityType::CInfinity,
                precise_extension,
            ))?
            .flatten();
            self.law.set_extended_edge_curve(second);
        }
        Ok(())
    }

    /// `SmVariableRadiusFS::SetupOffsetValues`: the law's radius at the parameter
    /// stored in the intersection point (`m_adUserDoubles[0]`) becomes both offset
    /// distances (times the orientations); returns it (`*pdOffsetDist`).
    ///
    /// # Errors
    ///
    /// Those of the law evaluation ([`FilletLaw::evaluate`]);
    /// [`SolidError::InvalidInput`] for a missing offset surface.
    pub fn setup_offset_values(&mut self, tsect_pnt: &TsectPnt) -> Result<f64> {
        // compute the radius value at this intersection point
        let t = tsect_pnt
            .user_doubles
            .first()
            .copied()
            .ok_or_else(|| SolidError::invalid_input(OP_VR, "no user doubles"))?;
        // (status ignored upstream: the values are then undefined)
        let values = self
            .law
            .evaluate(t, &self.law_interval, self.law_orientation)?;
        let [radius, _, _] = values;

        // save fillet radius and set OffsetSurface offset distances
        self.curve_based.solver.set_offsets(radius)?;
        Ok(radius)
    }
}

// ---- SmGuessSphereSolveENFO ---------------------------------------------------------------

/// The sphere/plane solve of a fillet point at a given distance from a known point
/// (`SmGuessSphereSolveENFO`, file-local): its `SmFilletSphereSolveENFO` base is
/// [`FilletSphereSolveEnfo`] of unit `blend.fillet_solver_p1`, which holds the solver.
/// Its `Evaluate` belongs to unit `blend.fillet_standard_solver_p2`.
pub struct GuessSphereSolve<'a> {
    /// The `SmFilletSphereSolveENFO` base.
    pub base: FilletSphereSolveEnfo<'a>,
}

impl<'a> GuessSphereSolve<'a> {
    /// `SmGuessSphereSolveENFO(crSphereCenter, dSphereRadius, crFilletSolver)`: the
    /// base constructor (the plane starts as the sphere, not adjusting).
    pub fn new(
        sphere_center: Point3,
        sphere_radius: f64,
        solver: &'a mut dyn FilletSolverOps,
    ) -> Self {
        Self {
            base: FilletSphereSolveEnfo::new(sphere_center, sphere_radius, solver),
        }
    }
}

// ---- surface generators -------------------------------------------------------------------

/// The fillet surface generator with a linear cross section (`SmLinearCrossSectionFSG`):
/// a ruled surface between the rails.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LinearCrossSectionFsg;

impl LinearCrossSectionFsg {
    /// `GetFilletSurfaceGeneratorType`.
    #[must_use]
    pub const fn generator_type(&self) -> FilletSurfaceGeneratorType {
        FilletSurfaceGeneratorType::Linear
    }

    /// `SmLinearCrossSectionFSG::CreateSurface`: the ruled surface between the two
    /// rails, linear in `v`.
    ///
    /// # Errors
    ///
    /// Those of `CreateRuledSurface`.
    pub fn create_surface(
        &self,
        rail1: &BSplineCurve,
        rail2: &BSplineCurve,
    ) -> Result<BSplineSurface> {
        BSplineSurface::create_ruled_surface(rail1, rail2, SurfParamType::V)
    }
}

/// The fillet surface generator with a circular cross section
/// (`SmCircularCrossSectionFSG`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CircularCrossSectionFsg {
    /// `m_bApproximateArcs`: approximate the arcs (with equal knot vectors) instead of
    /// using exact rational arcs.
    pub approximate_arcs: bool,
    /// `m_dTolPercentRadius`: the allowed deviation of approximated arcs, as a fraction
    /// of the radius.
    pub tol_percent_radius: f64,
    /// `m_bMirror`: mirror each cross section about its rail chord.
    pub mirror: bool,
    /// `m_bComplement`: use the larger part of each circle.
    pub complement: bool,
}

impl Default for CircularCrossSectionFsg {
    /// `SmCircularCrossSectionFSG()`: approximate arcs, 5 percent of the radius, no
    /// mirror, no complement.
    fn default() -> Self {
        Self {
            approximate_arcs: true,
            tol_percent_radius: 0.05,
            mirror: false,
            complement: false,
        }
    }
}

/// Whether `c` is an `SmBSplineCurve` kind (B-spline, line, ellipse, hyperbola or
/// parabola), the kinds an `SmBSplineCurve &` binds.
fn is_b_spline_kind(c: &Curve) -> bool {
    matches!(
        c.kind(),
        CurveKind::BSpline(_)
            | CurveKind::Line(_)
            | CurveKind::Ellipse(_)
            | CurveKind::Hyperbola(_)
            | CurveKind::Parabola(_)
    )
}

/// The NURB part of an `SmBSplineCurve` kind, in place, for the static B-spline
/// routines that edit it whatever the kind (a line's NURB is made first when it has
/// none); `None` for the other kinds.
fn nurb_part_mut(c: &mut Curve) -> Result<Option<&mut BSplineCurve>> {
    Ok(match c.kind_mut() {
        CurveKind::BSpline(b) => Some(b),
        CurveKind::Line(l) => {
            let made = match l.base_with_nurb()? {
                std::borrow::Cow::Owned(b) => Some(b),
                std::borrow::Cow::Borrowed(_) => None,
            };
            if let Some(b) = made {
                *l.base_mut() = b;
            }
            Some(l.base_mut())
        }
        CurveKind::Ellipse(e) => Some(e.base_mut()),
        CurveKind::Hyperbola(h) => Some(h.base_mut()),
        CurveKind::Parabola(p) => Some(p.base_mut()),
        _ => None,
    })
}

/// The arc a curve is (`IsArc(11, 1e-8, ..)`, the `SmCurve` base: B-spline curves do
/// not override it).
fn bspline_is_arc(k: &Kernel, c: &BSplineCurve) -> Result<Option<(Axis2Placement, f64, f64, f64)>> {
    is_arc_base(
        k,
        c,
        |c, tol| c.is_degenerate(k, tol, None),
        ANALYTIC_SAMPLES,
        ANALYTIC_TOL,
    )
}

/// `GetKnots` with the status ignored: the unique knots, or `old` on failure.
fn knots_or(c: &BSplineCurve, old: Vec<f64>) -> Result<Vec<f64>> {
    Ok(ignored(c.knots(None))?.map_or(old, |k| k.0))
}

/// The knot at `i` of a knot array upstream indexes unchecked.
fn knot(knots: &[f64], i: usize, op: &'static str) -> Result<f64> {
    knots
        .get(i)
        .copied()
        .ok_or_else(|| SolidError::invalid_input(op, "knot index out of range"))
}

/// The shared start of both `CreateSurfaceFromCurves`: rails with two unique knots
/// get two more at the thirds of the interval, then mid-span knots go into the
/// largest span until there are four (`bail_if_stuck`: an insertion that adds no knot
/// is `SM_ERR`; otherwise upstream loops forever, here
/// [`SolidError::InvalidInput`]). Returns the knots of `rail1`.
fn add_section_knots(
    rail1: &mut BSplineCurve,
    rail2: &mut BSplineCurve,
    mut knots: Vec<f64>,
    bail_if_stuck: bool,
    op: &'static str,
) -> Result<Vec<f64>> {
    // insert a mid knot for 2 knot curves
    if knots.len() == 2 {
        let step = (knot(&knots, 1, op)? - knot(&knots, 0, op)?) / 3.0;
        let new1 = knot(&knots, 0, op)? + step;
        let new2 = new1 + step;
        // (statuses ignored upstream)
        ignore_status(rail1.insert_one_knot(new1, 1))?;
        ignore_status(rail2.insert_one_knot(new1, 1))?;
        ignore_status(rail1.insert_one_knot(new2, 1))?;
        ignore_status(rail2.insert_one_knot(new2, 1))?;
        knots = knots_or(rail1, knots)?;
    }

    let mut n_knots_old = knots.len();

    // insert a mid interval knot into largest span
    while knots.len() < 4 {
        let mut max_span = 0;
        let mut max_span_value = 0.0;
        for k in 1..knots.len() {
            let span = knot(&knots, k, op)? - knot(&knots, k - 1, op)?;
            if span > max_span_value {
                max_span = k;
                max_span_value = span;
            }
        }

        let mid = (knot(&knots, max_span, op)? + knot(&knots, sub(max_span, 1, op)?, op)?) / 2.0;
        ignore_status(rail1.insert_one_knot(mid, 1))?;
        ignore_status(rail2.insert_one_knot(mid, 1))?;
        knots = knots_or(rail1, knots)?;

        let n_knots_new = knots.len();

        // if no knots were added then bail to avoid endless loop
        if n_knots_new == n_knots_old {
            if bail_if_stuck {
                return Err(SolidError::sm_err(op));
            }
            return Err(SolidError::invalid_input(
                op,
                "knot insertion adds no knot (upstream loops forever)",
            ));
        }
        n_knots_old = n_knots_new;
    }
    Ok(knots)
}

/// Whether two lines' directions differ by more than `SM_EFF_ZERO_SQRT` (the sweep
/// test of the generators).
fn line_is_off(line_vector: Vec3, rail: &BSplineCurve) -> Result<bool> {
    if let Some((_, line_vector2)) = rail.is_line(ANALYTIC_SAMPLES, ANALYTIC_TOL)? {
        let angle = line_vector2.angle_between(line_vector)?;
        if angle > EFF_ZERO_SQRT {
            return Ok(true);
        }
    }
    Ok(false)
}

/// `(sKnots, lMaxNumUniqueKnots - 2)` knots inserted evenly into a cross section
/// (the knot-equalising pass).
fn equalise_knots(arc: &mut BSplineCurve, max_unique: usize, op: &'static str) -> Result<()> {
    let num_new = max_unique
        .checked_sub(2)
        .ok_or_else(|| SolidError::invalid_input(op, "fewer than 2 unique knots"))?;
    if num_new == 0 {
        return Ok(());
    }
    let ivl = arc.natural_interval();
    let new_knots: Vec<f64> = (0..num_new)
        .map(|mm| ivl.evaluate((mm as f64 + 1.0) / (num_new as f64 + 1.0)))
        .collect();
    arc.insert_knots(&new_knots)
}

/// The cross section at `index` of the array upstream indexes unchecked.
fn section_mut<'a>(
    sections: &'a mut [Option<BSplineCurve>],
    index: usize,
    op: &'static str,
) -> Result<&'a mut Option<BSplineCurve>> {
    sections
        .get_mut(index)
        .ok_or_else(|| SolidError::invalid_input(op, "cross section index out of range"))
}

/// The cross boundary derivative surface at one end of a skin: the ruled surface from
/// the end cross section to its copy scaled, translated and rotated onto the rail
/// tangents `start` / `end` (`CreateByScaleTransRot`), over `[curve interval] x [0, 1]`.
fn deriv_surface(gen_curve: &BSplineCurve, start: Vec3, end: Vec3) -> Result<BSplineSurface> {
    let new_curve = gen_curve.create_by_scale_trans_rot(start, end, 0.0)?;
    let mut surf = BSplineSurface::create_ruled_surface(gen_curve, &new_curve, SurfParamType::U)?;
    let crv_ivl = gen_curve.natural_interval();
    let uv_min = Point2::new(crv_ivl.min(), 0.0);
    let uv_max = Point2::new(crv_ivl.max(), 1.0);
    // (status ignored upstream)
    ignore_status(surf.adjust_step_uv_domain(&Extent2d::from_points(uv_min, uv_max)))?;
    Ok(surf)
}

/// The first derivative at `t` of a B-spline curve (`Evaluate(t, 1, TRUE, sPV)`,
/// `SmBSplineCurve.h` default `bNonZeroTangents = FALSE`).
fn tangent_at(c: &BSplineCurve, t: f64, op: &'static str) -> Result<Vec3> {
    c.evaluate(t, 1, true, false)?
        .get(1)
        .copied()
        .ok_or_else(|| SolidError::sm_err(op))
}

/// Inserts the mid knots of the first and last spans of `rail1` into both rails (the
/// G1 continuity set-up of the generators).
fn insert_end_span_knots(
    rail1: &mut BSplineCurve,
    rail2: &mut BSplineCurve,
    op: &'static str,
) -> Result<()> {
    // (statuses ignored upstream)
    let knts = knots_or(rail1, Vec::new())?;
    let n = knts.len();
    let start_knot_mid = (knot(&knts, 0, op)? + knot(&knts, 1, op)?) / 2.0;
    ignore_status(rail1.insert_one_knot(start_knot_mid, 1))?;
    ignore_status(rail2.insert_one_knot(start_knot_mid, 1))?;
    let end_knot_mid = (knot(&knts, sub(n, 2, op)?, op)? + knot(&knts, sub(n, 1, op)?, op)?) / 2.0;
    ignore_status(rail1.insert_one_knot(end_knot_mid, 1))?;
    ignore_status(rail2.insert_one_knot(end_knot_mid, 1))?;
    Ok(())
}

/// The ruled or revolved fillet surface reparameterised to the rail's interval by
/// `[0, 1]`.
fn reparameterize_to_rail(surface: &mut BSplineSurface, rail1: &BSplineCurve) -> Result<()> {
    let ivl = rail1.natural_interval();
    let domain = Extent2d::from_points(Point2::new(ivl.min(), 0.0), Point2::new(ivl.max(), 1.0));
    surface.reparameterize(&domain)
}

/// Makes the first interior control point row next to a `u` side that is
/// (approximately) singular exactly equal when its end points are within the zone
/// tolerance (`[B597]` of `SmCircularCrossSectionFSG::CreateSurfaceFromCurves`).
fn equalise_singular_rows(
    k: &Kernel,
    surface: BSplineSurface,
    op: &'static str,
) -> Result<BSplineSurface> {
    let surface = Surface::from(surface);
    let sing = u64::from(surface.get_singularities(k, false, false)?.approx_singular);
    let tol = zone_tol_3d(k, &surface)?;
    let SurfaceKind::BSpline(mut surface) = surface.into_kind() else {
        return Err(SolidError::sm_err(op));
    };
    let (num_u, num_v) = match surface.gw_nurb() {
        Some(s) => {
            let ix = s.indices();
            (add(ix.n, 1, op)?, add(ix.m, 1, op)?)
        }
        None => return Err(SolidError::sm_err(op)),
    };
    for (side, idx_u) in [(SS_UMIN, Some(1)), (SS_UMAX, num_u.checked_sub(2))] {
        if sing & side == 0 {
            continue;
        }
        // First interior row.
        let idx_u =
            idx_u.ok_or_else(|| SolidError::invalid_input(op, "fewer than 2 control rows"))?;
        let last_v = sub(num_v, 1, op)?;
        let form = ControlPointForm::EuclideanRational;
        let (mut pt1, mut w1) = surface.control_point(form, idx_u, 0)?;
        let (pt2, w2) = surface.control_point(form, idx_u, last_v)?;
        if pt2.closer_than(tol, pt1) {
            pt1 = (pt1 + pt2) / 2.0; // Make sure they're exactly the same.
            w1 = (w1 + w2) / 2.0;
            for ii in 0..num_v {
                // (status ignored upstream)
                ignore_status(surface.set_control_point(form, idx_u, ii, pt1, w1))?;
            }
        }
    }
    Ok(surface)
}

impl CircularCrossSectionFsg {
    /// `GetFilletSurfaceGeneratorType`.
    #[must_use]
    pub const fn generator_type(&self) -> FilletSurfaceGeneratorType {
        FilletSurfaceGeneratorType::Circular
    }

    /// `GetTolPercentRadius`.
    #[must_use]
    pub const fn tol_percent_radius(&self) -> f64 {
        self.tol_percent_radius
    }

    /// `GetApproximateArcs`.
    #[must_use]
    pub const fn approximate_arcs(&self) -> bool {
        self.approximate_arcs
    }

    /// `SmCircularCrossSectionFSG::CreateSurfaceFromCurves`: a circular arc about the
    /// centre line from `rail1` to `rail2` at every unique knot of `rail1` (the rails
    /// first get knots until they have four; all three curves are synchronised when
    /// the rails' knot counts differ): a point curve where the rails meet within
    /// `approx_tol_3d`, none where the two radii differ by more than a tenth, an arc
    /// patched onto the rails (end points moved, or an interpolating blend curve) where
    /// they differ slightly; approximated arcs get equal knot vectors in a second pass.
    /// The surface is the ruled surface between the end sections when the centre line
    /// is a degree 1 line and no rail is a line in another direction, the surface of
    /// revolution of the first section when all three curves are coaxial arcs with the
    /// same sweep, else a skin through all sections along both rails (with cross
    /// boundary derivative surfaces from the rail tangents for `need_g1_continuity`),
    /// whose first interior control rows at singular `u` sides are made equal.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a zero radius, a zero or straight arc angle, too few sections, or
    /// a knot insertion that adds no knot; `SM_ERR_INVALID_INPUT` from `Unitize`; those
    /// of the curve and surface constructions; [`SolidError::InvalidInput`] where
    /// upstream indexes past its cross sections (a second pass for a ruled or revolved
    /// surface).
    ///
    /// `center_line` is upstream's `SmBSplineCurve & rCenterLine`: an `SmBSplineCurve`
    /// kind (B-spline, line, ellipse, hyperbola or parabola) whose `GetDegree`, `IsLine`,
    /// `IsArc`, `Evaluate` and `GetNaturalInterval` dispatch on that kind (an `SmLine`
    /// answers degree 1 and its own line whatever its NURB); the static knot
    /// synchronisation edits its NURB part. Another kind is
    /// [`SolidError::InvalidInput`] (upstream cannot bind it).
    pub fn create_surface_from_curves(
        &self,
        k: &Kernel,
        center_line: &mut Curve,
        rail1: &mut BSplineCurve,
        rail2: &mut BSplineCurve,
        approx_tol_3d: f64,
        need_g1_continuity: bool,
    ) -> Result<BSplineSurface> {
        const OP: &str = OP_CIRC;
        if !is_b_spline_kind(center_line) {
            return Err(SolidError::invalid_input(
                OP,
                "the centre line is not an SmBSplineCurve kind",
            ));
        }
        let knots = rail1.knots(None)?.0;
        let knots2 = rail2.knots(None)?.0;

        // make sure all 3 curves have compatible knot vectors
        if knots2.len() != knots.len() {
            let cl = nurb_part_mut(center_line)?.ok_or(SolidError::invalid_input(
                OP,
                "the centre line is not an SmBSplineCurve kind",
            ))?;
            let mut rails = [
                core::mem::take(rail1),
                core::mem::take(rail2),
                core::mem::take(cl),
            ];
            let sync = BSplineCurve::synchronize_knots_of_curves(&mut rails, EFF_ZERO_SQRT);
            let [a, b, c] = rails;
            *rail1 = a;
            *rail2 = b;
            *cl = c;
            sync?;
        }

        // local array for cross section curves and parameters
        let mut cross_sections: Vec<Option<BSplineCurve>> = Vec::new();
        let mut parameters_of_cross_sections: Vec<f64> = Vec::new();

        // Surface creation requires 4 or more cross sections so add
        // some extra knots if needed.
        let knots = add_section_knots(rail1, rail2, knots, true, OP)?;

        // when center, rail1, and rail2 curves are parallel lines - NewSurf = RuledSurface
        // when center, rail1, and rail2 curves are arcs           - NewSurf = SurfaceOfRevolution
        // else                                                    - NewSurf = Skinned Surface
        let mut sweep = false;
        let mut revolve = false;

        // see if newSurf == RuledSurface (`GetDegree` and `IsLine` of the centre line's
        // own kind)
        let deg1_line = if center_line.degree()? == 1 {
            center_line.is_line(k, ANALYTIC_SAMPLES, ANALYTIC_TOL)?
        } else {
            None
        };
        if let Some((_, line_vector)) = deg1_line {
            sweep = true;
            if line_is_off(line_vector, rail1)? {
                sweep = false;
            }
            if line_is_off(line_vector, rail2)? {
                sweep = false;
            }
        }

        // see if NewSurf == SurfaceOfRevolution
        let arc = center_line.is_arc(k, ANALYTIC_SAMPLES, ANALYTIC_TOL)?;
        if let Some((ref_frame, _, _, end_ang)) = &arc {
            revolve = true;
            match bspline_is_arc(k, rail1)? {
                None => revolve = false,
                Some((ref_frame2, radius2, _, end_ang2)) => {
                    if (end_ang2 - end_ang).abs() * radius2 > approx_tol_3d
                        || !ref_frame2.are_coaxial(ref_frame, approx_tol_3d, radius2)
                    {
                        revolve = false;
                    }
                    if revolve {
                        match bspline_is_arc(k, rail2)? {
                            None => revolve = false,
                            Some((ref_frame3, radius3, _, end_ang3)) => {
                                // Also check against 1s Rail curve.
                                if (end_ang3 - end_ang).abs() * radius3 > approx_tol_3d
                                    || !ref_frame3.are_coaxial(ref_frame, approx_tol_3d, radius3)
                                    || (end_ang3 - end_ang2).abs() * radius3 > approx_tol_3d
                                    || !ref_frame3.are_coaxial(&ref_frame2, approx_tol_3d, radius3)
                                {
                                    revolve = false;
                                }
                            }
                        }
                    }
                }
            }
        }

        // track max/min number of knots in cross-section curves
        let mut max_num_unique_knots: usize = 0;
        let mut min_num_unique_knots: usize = 1_000_000;

        // for two passes - 1st pass allocate curves
        //                  2nd pass make all curve knot vectors compatible
        for kkk in 0..2 {
            // for every pRail1Curve->knot
            let mut ii = 1;
            while ii < knots.len() {
                // skip to end cross section curve for sweeps and revolves
                if ii > 1 && (sweep || revolve) {
                    ii = knots.len() - 1;
                }
                self.cross_section_at(
                    k,
                    kkk,
                    ii,
                    &knots,
                    center_line,
                    rail1,
                    rail2,
                    approx_tol_3d,
                    &mut cross_sections,
                    &mut parameters_of_cross_sections,
                    &mut max_num_unique_knots,
                    &mut min_num_unique_knots,
                )?;
                ii += 1;
            }

            if !self.approximate_arcs || max_num_unique_knots == min_num_unique_knots {
                // only need 1 pass
                break;
            }
        }

        // In case we put blanks in:
        let mut sections: Vec<BSplineCurve> = Vec::new();
        let mut params: Vec<f64> = Vec::new();
        for (c, t) in cross_sections.into_iter().zip(parameters_of_cross_sections) {
            if let Some(c) = c {
                sections.push(c);
                params.push(t);
            }
        }

        // when surface can be constructed as a ruled surface (linear center and rail curves)
        if sweep {
            if sections.len() < 2 {
                // [B448]
                return Err(SolidError::sm_err(OP));
            }
            let (Some(start), Some(end)) = (sections.first(), sections.last()) else {
                return Err(SolidError::sm_err(OP));
            };
            let mut surface = BSplineSurface::create_ruled_surface(start, end, SurfParamType::U)?;
            reparameterize_to_rail(&mut surface, rail1)?;
            return Ok(surface);
        }
        if revolve {
            // when surface can be constructed as a surfaceOfRevolution
            let (Some(gen_curve), Some((ref_frame, _, _, end_ang))) = (sections.first(), &arc)
            else {
                // [B448]
                return Err(SolidError::sm_err(OP));
            };
            let mut surface = BSplineSurface::create_surf_of_revolution(
                k,
                gen_curve,
                ref_frame.origin(),
                ref_frame.z_axis(),
                *end_ang,
                false,
                None,
            )?;
            reparameterize_to_rail(&mut surface, rail1)?;
            return Ok(surface);
        }

        // build a skin surface to interpolate all crossSection curves and option end
        // crossTangent values
        if sections.len() < 2 {
            // [B448]
            return Err(SolidError::sm_err(OP));
        }
        let mut deriv_surfs: [Option<BSplineSurface>; 2] = [None, None];

        // when asked for G1 continuity
        if need_g1_continuity {
            let ivl = center_line.natural_interval();
            // for NewSurface start and end boundaries
            for (kkk, slot) in deriv_surfs.iter_mut().enumerate() {
                // We need to provide cross-boundary derivative surface
                // for skinned-surface creation.
                let (gen_curve, t) = if kkk == 1 {
                    (sections.last(), ivl.max())
                } else {
                    (sections.first(), ivl.min())
                };
                let gen_curve = gen_curve.ok_or_else(|| SolidError::sm_err(OP))?;

                // get railCurve endTangents
                let st = tangent_at(rail1, t, OP)?;
                // gwc:replaced one line to interpolate rail2Curve rather than CenterLine
                let end = tangent_at(rail2, t, OP)?;

                // save the crossTangent surface
                *slot = Some(deriv_surface(gen_curve, st, end)?);
            }

            // add a new knot in the NewSurface start/end intervals
            // to allow the newSurface to interpolate the computed crossTangent values
            insert_end_span_knots(rail1, rail2, OP)?;
        }

        // Build the Skinned Surface to interpolate crossSections and optional end Cross Tangents
        let [d0, d1] = &deriv_surfs;
        let opts = SkinnedSurfaceOptions {
            synchronized: false,
            cross_section_surf_param: SurfParamType::U,
            approx_tol: approx_tol_3d,
            rail1: Some(rail1),
            rail2: Some(rail2),
            rail1_used_as_spine_curve: false,
            intersections_with_rails: Some(&params),
            deriv_surfs: Some([d0.as_ref(), d1.as_ref()]),
            degree: 3,
        };
        let fillet_surface = BSplineSurface::create_skinned_surface(k, &sections, opts)?;

        // We have a case where the rail curves start off in the same direction,
        // because the surfaces are not only tangent at one end but also both locally
        // flat.  This means that the end control points of the first interior row of
        // control points will be identical.  However, the algorithm above creates
        // internal points on that first internal row that are not the same.  We fix this
        // by making them all the same.  This means that all of the u-derivatives at the
        // singularity will be parallel: that's legal and doesn't cause problems. [B597]
        equalise_singular_rows(k, fillet_surface, OP)
    }

    /// One cross section of `CreateSurfaceFromCurves` (the body of its loop over
    /// `jj` for the knot span ending at `ii`, pass `kkk`).
    fn cross_section_at(
        &self,
        k: &Kernel,
        kkk: usize,
        ii: usize,
        knots: &[f64],
        center_line: &Curve,
        rail1: &BSplineCurve,
        rail2: &BSplineCurve,
        approx_tol_3d: f64,
        cross_sections: &mut Vec<Option<BSplineCurve>>,
        parameters_of_cross_sections: &mut Vec<f64>,
        max_num_unique_knots: &mut usize,
        min_num_unique_knots: &mut usize,
    ) -> Result<()> {
        const OP: &str = OP_CIRC;
        // for lNumBetween count locations between knot points
        // ok for lNumBetween == 0
        let num_between: usize = 0;
        for jj in 0..=num_between + 1 {
            let k0 = knot(knots, ii - 1, OP)?;
            let k1 = knot(knots, ii, OP)?;
            let t = if jj == 0 {
                k0
            } else if jj == num_between + 1 {
                k1
            } else {
                k0 + jj as f64 * (k1 - k0) / (num_between as f64 + 1.0)
            };

            if jj == 0 && ii != 1 {
                continue; // Only do first point first time through
            }
            // Compute the knot index
            let knot_index = if jj == 0 && ii == 1 { 0 } else { ii };

            // get center, rail1, rail2 points for dT
            let cent = center_line
                .evaluate(k, t, 1, true, false)?
                .first()
                .copied()
                .ok_or_else(|| SolidError::sm_err(OP))?;
            let p1 = rail1.evaluate_point(t)?;
            let p2 = rail2.evaluate_point(t)?;

            // get sample Point gaps
            let v1 = p1 - cent;
            let v2 = p2 - cent;

            // when railCurve samplePoints are close together - make a degenerate cross section curve
            if p1.distance(p2) < approx_tol_3d {
                if kkk == 0 {
                    // Create Degenerate Point curve for each dT value on Rail1
                    let arc = BSplineCurve::create_point_curve(p1)?; /* parameterized from 0 to 1 */

                    // track max/min cross section unique knot count
                    let num_ctrl_pts = arc.number_of_unique_knots()?;
                    *max_num_unique_knots = (*max_num_unique_knots).max(num_ctrl_pts);
                    *min_num_unique_knots = (*min_num_unique_knots).min(num_ctrl_pts);

                    // accumulate cross section curve and parameter value
                    parameters_of_cross_sections.push(t);
                    cross_sections.push(Some(arc));
                } else {
                    // Insert knots into degenerate Curve until UniqueKnotCount = MaxNumUniqueKnots
                    let arc = section_mut(cross_sections, knot_index, OP)?
                        .as_mut()
                        .ok_or_else(|| SolidError::invalid_input(OP, "NULL cross section"))?;
                    equalise_knots(arc, *max_num_unique_knots, OP)?;
                }
                continue;
            }

            // else the the railCurve points are far appart.
            // build and add a circular arc to the cross section array.
            let radius1 = v1.length();
            let radius2 = v2.length();

            // Check for case of zero radius on interior of surface
            if radius1 < EFF_ZERO || radius2 < EFF_ZERO {
                // Zero radius cross-section curve
                return Err(SolidError::sm_err(OP));
            } else if (radius1 - radius2).abs() > radius1 / 10.0 {
                // skip adding a cross section for this parameter value
                // But put something into the arrays (on 1st pass), to keep the indexing correct. [B663]
                if kkk == 0 {
                    parameters_of_cross_sections.push(t);
                    cross_sections.push(None);
                }
                continue;
            }

            // get the CCW angle from sV1 to sV2
            let v1 = v1.unitize()?;
            let v2 = v2.unitize()?;
            let mut z_vec = v1.cross(v2).unitize()?;
            let angle = z_vec.ccw_angle_between(v1, v2)?;

            // quit for zero and 180 angles
            if angle.abs() < EFF_ZERO_SQRT || angle.abs() > PI - EFF_ZERO_SQRT {
                // Tangent or Degenerate Angle for cross-section curve
                return Err(SolidError::sm_err(OP));
            }

            // negate normal for neg angles and recompute CCW angle
            if angle < 0.0 {
                z_vec = -z_vec;
            }
            let mut angle = z_vec.ccw_angle_between(v1, v2)?;
            if angle < EFF_ZERO {
                return Err(SolidError::sm_err(OP)); // Can't happen ??
            }

            // make unitNormal coordinate system
            let mut y_vec = z_vec.cross(v1);
            let x_vec = y_vec.cross(z_vec);

            // when asked - make complementary circle
            if self.complement {
                y_vec = -y_vec;
                angle = 2.0 * PI - angle;
            }

            // make coordinate system object
            let x_vec = x_vec.unitize()?;
            let y_vec = y_vec.unitize()?;
            let mut placement = Axis2Placement::default();
            placement.set_canonical(cent, x_vec, y_vec)?;

            let mut arc: Option<BSplineCurve> = None;
            // when approximating arcs
            if self.approximate_arcs {
                if kkk == 0 {
                    // on the 1st pass - make an arc, track the min/max number of Unique knots
                    let a = BSplineCurve::approximate_arc(
                        3,
                        None,
                        &placement,
                        radius1,
                        0.0,
                        angle * 180.0 / PI,
                        self.tol_percent_radius,
                    )?;

                    // track max/min number of unique knots
                    let num_knots = a.number_of_unique_knots()?;
                    *max_num_unique_knots = (*max_num_unique_knots).max(num_knots);
                    *min_num_unique_knots = (*min_num_unique_knots).min(num_knots);
                    arc = Some(a);
                } else if (radius1 - radius2).abs() < approx_tol_3d {
                    // on 2nd pass with equal gapVector radii
                    // get current arc and uniqueKnot count
                    let slot = section_mut(cross_sections, knot_index, OP)?;
                    let Some(current) = slot.as_ref() else {
                        continue;
                    };
                    let num_unique_knots = current.number_of_unique_knots()?;

                    // when needed - replace pArc with a new Arc with appropriate number of knots
                    if num_unique_knots != *max_num_unique_knots {
                        let max_ctrl_pts = add(*max_num_unique_knots, 2, OP)? as u64;
                        *slot = Some(BSplineCurve::approximate_arc(
                            3,
                            Some(max_ctrl_pts),
                            &placement,
                            radius1,
                            0.0,
                            angle * 180.0 / PI,
                            self.tol_percent_radius,
                        )?);
                    }
                } else {
                    // on 2nd pass with unequal gapVector radii
                    // Do need to fix knot vector of blend
                    let Some(current) = section_mut(cross_sections, knot_index, OP)?.as_mut()
                    else {
                        continue;
                    };
                    equalise_knots(current, *max_num_unique_knots, OP)?;
                    continue;
                }
            } else if kkk == 0 {
                // not approximating arcs: 1st pass build a circular segment
                arc = Some(BSplineCurve::create_circle_segment(
                    3,
                    &placement,
                    radius1,
                    0.0,
                    angle * 180.0 / PI,
                    NurbCircleParam::Quadratic,
                )?);
            }

            // on 1st pass - See if we have a non-circle case
            if kkk == 0 && (radius1 - radius2).abs() > 1.0e-8 {
                let a = arc.as_mut().ok_or_else(|| SolidError::sm_err(OP))?;
                // Tweak arc or create a blend curve using railPoints and current pArc tangents
                // to interpolate the railCurve sample points
                let ivl = a.natural_interval();
                let start_tan = tangent_at(a, ivl.min(), OP)?;
                let end_tan = tangent_at(a, ivl.max(), OP)?;
                let points = [p1, p2];
                let mut vectors = [start_tan, end_tan];

                // Here either tweak the arc a little by moving the ends or
                // create an interpolating curve.
                if (radius1 - radius2).abs() < approx_tol_3d * 4.0 {
                    // move the arc endPoints to lie directly on the rail curves
                    a.edit_end_point(p1, true, None)?;
                    a.edit_end_point(p2, false, None)?;
                } else {
                    // replace the circular arc with an interpolating curve
                    BSplineCurve::fair_blend_curve_derivatives(&points, &mut vectors, None, false)?;
                    arc = Some(BSplineCurve::create_interpolating_curve(
                        ParamMethod::Uniform,
                        3,
                        3,
                        &points,
                        &vectors,
                        None,
                        true,
                    )?);
                }
            }

            // When asked - mirror the cross section
            if self.mirror {
                let mirror_x_axis = (p2 - p1).unitize()?;
                let z_vec = z_vec.unitize()?;
                let mut mirror = Axis2Placement::default();
                mirror.set_canonical(p1, mirror_x_axis, z_vec)?;
                if kkk == 1 {
                    let slot = section_mut(cross_sections, knot_index, OP)?;
                    let current = slot
                        .as_ref()
                        .ok_or_else(|| SolidError::invalid_input(OP, "NULL cross section"))?;
                    *slot = Some(current.create_mirror_curve(&mirror)?);
                } else {
                    let a = arc.as_ref().ok_or_else(|| SolidError::sm_err(OP))?;
                    arc = Some(a.create_mirror_curve(&mirror)?);
                }
            }

            // accumulate the cross section curves
            if kkk == 0 {
                parameters_of_cross_sections.push(t);
                cross_sections.push(arc);
            }
        }
        Ok(())
    }
}

/// The fillet surface generator with a blend curve cross section
/// (`SmBlendCurveCrossSectionFSG`): `G1`, `G2` or `G3` blend curves between the rails.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlendCurveCrossSectionFsg {
    /// `m_lContinuity`: 1, 2 or 3 for `G1`, `G2` or `G3`.
    pub continuity: u32,
    /// `m_dBlendScale`: the scale of the end derivatives.
    pub blend_scale: f64,
    /// `m_bAtThirds` (the fork's, source fork `fd3b6a7`): the sections' inner
    /// control points sit a third and two thirds of the way from each contact to
    /// where the contacts' tangent lines meet, instead of the faired Hermite scaled
    /// by the blend scale.
    pub at_thirds: bool,
}

impl Default for BlendCurveCrossSectionFsg {
    /// `SmBlendCurveCrossSectionFSG()`: blend scale `1.5`, `G1`, not at thirds.
    fn default() -> Self {
        Self {
            continuity: 1,
            blend_scale: 1.5,
            at_thirds: false,
        }
    }
}

/// A surface and its UV curve along a rail (`pSurf1`, `pUV1` of
/// `SmBlendCurveCrossSectionFSG::CreateSurfaceFromCurves`).
#[derive(Debug, Clone, Copy)]
pub struct RailSurface<'a> {
    /// The surface the rail lies on.
    pub surface: Option<&'a Surface>,
    /// The rail's UV curve on it (its `x`, `y` are the UV point).
    pub uv_curve: Option<&'a BSplineCurve>,
}

impl BlendCurveCrossSectionFsg {
    /// `GetFilletSurfaceGeneratorType`.
    #[must_use]
    pub const fn generator_type(&self) -> FilletSurfaceGeneratorType {
        FilletSurfaceGeneratorType::BlendCurve
    }

    /// `GetBlendScale`.
    #[must_use]
    pub const fn blend_scale(&self) -> f64 {
        self.blend_scale
    }

    /// `GetContinuity`.
    #[must_use]
    pub const fn continuity(&self) -> u32 {
        self.continuity
    }

    /// `SmBlendCurveCrossSectionFSG::CreateSurfaceFromCurves`: a blend curve from
    /// `rail1` to `rail2` at every unique knot of `rail1` (the rails first get knots
    /// until they have four; their knot counts must agree), with the end tangents of
    /// the approximated arc about the centre line, faired and scaled by the blend scale
    /// (for continuity 2 or 3 and both UV curves, also the second (and third)
    /// directional derivatives of the surfaces along the dropped tangents, raising the
    /// degree to 5 (7)); a point curve where the rails meet within `approx_tol_3d`.
    /// The surface is the ruled surface between the end sections when the centre line
    /// is a degree 1 line and no rail is a line in another direction, the surface of
    /// revolution of the first section when all three curves are arcs, else a skin
    /// through all sections along both rails (with cross boundary derivative surfaces
    /// from the rail 1 and centre line tangents for `need_g1_continuity`; closed rails
    /// close the skin with a copy of the first section).
    ///
    /// # Errors
    ///
    /// `SM_ERR` when the rails' knot counts differ, for a zero radius or a zero or
    /// straight arc angle; `SM_ERR_INVALID_INPUT` from `Unitize`; those of the curve and
    /// surface constructions and the surface queries; [`SolidError::InvalidInput`]
    /// for continuity above 1 with both UV curves but a missing surface (upstream
    /// dereferences `NULL`), or a knot insertion that adds no knot (upstream loops
    /// forever).
    pub fn create_surface_from_curves(
        &self,
        k: &Kernel,
        center_line: &mut BSplineCurve,
        rail1: &mut BSplineCurve,
        rail2: &mut BSplineCurve,
        approx_tol_3d: f64,
        need_g1_continuity: bool,
        side1: RailSurface<'_>,
        side2: RailSurface<'_>,
    ) -> Result<BSplineSurface> {
        const OP: &str = OP_BLEND;
        let knots = rail1.knots(None)?.0;
        let knots2 = rail2.knots(None)?.0;
        if knots2.len() != knots.len() {
            return Err(SolidError::sm_err(OP)); // Curves not compatible
        }

        let mut cross_sections: Vec<BSplineCurve> = Vec::new();
        let mut parameters_of_cross_sections: Vec<f64> = Vec::new();

        // Surface creation requires 4 or more cross sections so add
        // some extra knots if needed.
        let knots = add_section_knots(rail1, rail2, knots, false, OP)?;

        for i in 1..knots.len() {
            let num_between: usize = 0;
            for j in 0..=num_between + 1 {
                let k0 = knot(&knots, i - 1, OP)?;
                let k1 = knot(&knots, i, OP)?;
                let mut t = k0 + j as f64 * (k1 - k0) / (num_between as f64 + 1.0);
                if j == 0 {
                    t = k0;
                }
                if j == num_between + 1 {
                    t = k1;
                }
                if j == 0 && i != 1 {
                    continue; // Only do first point first time through
                }
                let section = self.cross_section_at(
                    k,
                    t,
                    center_line,
                    rail1,
                    rail2,
                    approx_tol_3d,
                    side1,
                    side2,
                )?;
                parameters_of_cross_sections.push(t);
                cross_sections.push(section);
            }
        }

        let mut is_line = center_line.is_line(ANALYTIC_SAMPLES, ANALYTIC_TOL)?;
        if center_line.degree()? != 1 && is_line.is_some() {
            is_line = None;
        }

        let mut sweep = false;
        if let Some((_, line_vector)) = is_line {
            sweep = true;
            if line_is_off(line_vector, rail1)? {
                sweep = false;
            }
            if line_is_off(line_vector, rail2)? {
                sweep = false;
            }
        }

        if sweep {
            let (Some(start), Some(end)) = (cross_sections.first(), cross_sections.last()) else {
                return Err(SolidError::invalid_input(OP, "no cross sections"));
            };
            let mut surface = BSplineSurface::create_ruled_surface(start, end, SurfParamType::U)?;
            reparameterize_to_rail(&mut surface, rail1)?;
            return Ok(surface);
        }

        let arc = bspline_is_arc(k, center_line)?;
        let mut revolve = false;
        if arc.is_some() {
            revolve = true;
            if bspline_is_arc(k, rail1)?.is_none() {
                revolve = false;
            }
            if bspline_is_arc(k, rail2)?.is_none() {
                revolve = false;
            }
        }
        if let (true, Some((ref_frame, _, _, end_ang))) = (revolve, &arc) {
            let gen_curve = cross_sections
                .first()
                .ok_or_else(|| SolidError::invalid_input(OP, "no cross sections"))?;
            let mut surface = BSplineSurface::create_surf_of_revolution(
                k,
                gen_curve,
                ref_frame.origin(),
                ref_frame.z_axis(),
                *end_ang,
                false,
                None,
            )?;
            reparameterize_to_rail(&mut surface, rail1)?;
            return Ok(surface);
        }

        let mut deriv_surfs: [Option<BSplineSurface>; 2] = [None, None];
        if need_g1_continuity {
            let ivl = center_line.natural_interval();
            for (i, slot) in deriv_surfs.iter_mut().enumerate() {
                // We need to provide cross-boundary derivative surface
                // for skinned-surface creation.
                // Create rulled surface using end cross-section
                // curve and the tangent vectors of rails
                let (gen_curve, t) = if i == 1 {
                    (cross_sections.last(), ivl.max())
                } else {
                    (cross_sections.first(), ivl.min())
                };
                let gen_curve =
                    gen_curve.ok_or_else(|| SolidError::invalid_input(OP, "no cross sections"))?;
                let st = tangent_at(rail1, t, OP)?;
                let end = tangent_at(center_line, t, OP)?;
                *slot = Some(deriv_surface(gen_curve, st, end)?);
            }
            insert_end_span_knots(rail1, rail2, OP)?;
        }
        if is_closed_base(k, &*rail1, &rail1.natural_interval(), 0.0)?
            && is_closed_base(k, &*rail2, &rail2.natural_interval(), 0.0)?
        {
            let copy_first = cross_sections
                .first()
                .ok_or_else(|| SolidError::invalid_input(OP, "no cross sections"))?
                .copy();
            cross_sections.pop();
            cross_sections.push(copy_first);
        }
        let [d0, d1] = &deriv_surfs;
        let opts = SkinnedSurfaceOptions {
            synchronized: true,
            cross_section_surf_param: SurfParamType::U,
            approx_tol: approx_tol_3d,
            rail1: Some(rail1),
            rail2: Some(rail2),
            rail1_used_as_spine_curve: false,
            intersections_with_rails: Some(&parameters_of_cross_sections),
            deriv_surfs: Some([d0.as_ref(), d1.as_ref()]),
            degree: 3,
        };
        BSplineSurface::create_skinned_surface(k, &cross_sections, opts)
    }

    /// The blend curve cross section at `t` (the body of the loops of
    /// `CreateSurfaceFromCurves`).
    fn cross_section_at(
        &self,
        k: &Kernel,
        t: f64,
        center_line: &BSplineCurve,
        rail1: &BSplineCurve,
        rail2: &BSplineCurve,
        approx_tol_3d: f64,
        side1: RailSurface<'_>,
        side2: RailSurface<'_>,
    ) -> Result<BSplineCurve> {
        const OP: &str = OP_BLEND;
        let cent = center_line
            .evaluate(t, 1, true, false)?
            .first()
            .copied()
            .ok_or_else(|| SolidError::sm_err(OP))?;
        let p1 = rail1.evaluate_point(t)?;
        let p2 = rail2.evaluate_point(t)?;
        let v1 = p1 - cent;
        let v2 = p2 - cent;
        if p1.distance(p2) < approx_tol_3d {
            // Create a point curve.
            return BSplineCurve::create_point_curve(p1); /* parameterized from 0 to 1 */
        }

        let radius = v1.length();
        let radius2 = v2.length();

        // Check for case of zero radius on interior of surface
        // We do allow it on the ends of the surface - however it
        // may not be allowed in the creation - we'll see.
        if radius < EFF_ZERO || radius2 < EFF_ZERO {
            // (an error at interior and end knots alike: "Someday we will handle this
            // at the ends")
            return Err(SolidError::sm_err(OP));
        }

        let v1 = v1.unitize()?;
        let v2 = v2.unitize()?;
        let mut z_vec = v1.cross(v2).unitize()?;
        let angle = z_vec.ccw_angle_between(v1, v2)?;
        if angle.abs() < EFF_ZERO_SQRT {
            return Err(SolidError::sm_err(OP)); // Degenerate case here
        }
        if angle.abs() > PI - EFF_ZERO_SQRT {
            return Err(SolidError::sm_err(OP)); // Nearly 180 degree case here
        }

        if angle < 0.0 {
            z_vec = -z_vec;
        }
        let angle = z_vec.ccw_angle_between(v1, v2)?;
        if angle < EFF_ZERO {
            return Err(SolidError::sm_err(OP)); // Can't happen ??
        }

        let y_vec = z_vec.cross(v1);
        let x_vec = y_vec.cross(z_vec);

        let x_vec = x_vec.unitize()?;
        let y_vec = y_vec.unitize()?;
        let mut placement = Axis2Placement::default();
        placement.set_canonical(cent, x_vec, y_vec)?;

        let arc = BSplineCurve::approximate_arc(
            3,
            None,
            &placement,
            radius,
            0.0,
            angle * 180.0 / PI,
            approx_tol_3d * 100.0,
        )?;

        let ivl = arc.natural_interval();
        let tan_1 = tangent_at(&arc, ivl.min(), OP)?.unitize()?;
        let points = [p1, p2];
        let tan_2 = tangent_at(&arc, ivl.max(), OP)?.unitize()?;
        let mut vectors = [tan_1, tan_2];
        let tan_3d_1 = tan_1;
        let tan_3d_2 = tan_2.unitize()? * tan_3d_1.length();

        // Create a blend curve instead using tangent vectors of the curve
        let mut higher_order_vecs: Option<Vec<Vec3>> = None;
        if let (true, Some(uv1_curve), Some(uv2_curve)) =
            (self.continuity > 1, side1.uv_curve, side2.uv_curve)
        {
            let null = || SolidError::invalid_input(OP, "UV curves without their surfaces");
            let surf1 = side1.surface.ok_or_else(null)?;
            let surf2 = side2.surface.ok_or_else(null)?;
            let uv_pnt1 = uv1_curve.evaluate_point(t)?;
            let uv_pnt2 = uv2_curve.evaluate_point(t)?;
            let uv1 = Point2::new(uv_pnt1.x, uv_pnt1.y);
            let uv2 = Point2::new(uv_pnt2.x, uv_pnt2.y);
            let drop1 = |s: &Surface, uv: Point2, v: Vec3| -> Result<Vec2> {
                drop_vectors_at(k, s, uv, Side::Left, Side::Left, &[v])?
                    .first()
                    .copied()
                    .ok_or_else(|| SolidError::sm_err(OP))
            };
            let uv_tan1 = drop1(surf1, uv1, tan_3d_1)?;
            let uv_tan2 = drop1(surf2, uv2, tan_3d_2)?;
            let mut vecs = Vec::new();
            if self.continuity == 2 {
                let g2 = compute_higher_order_derivs(k, surf1, uv1, uv_tan1, false, false)?.g2;
                surf1.evaluate_point(k, uv1)?;
                vecs.push(g2);

                let g2b = compute_higher_order_derivs(k, surf2, uv2, uv_tan2, false, false)?.g2;
                vecs.push(g2b);

                surf2.evaluate_point(k, uv2)?;
            }
            if self.continuity == 3 {
                let d1 = compute_higher_order_derivs(k, surf1, uv1, uv_tan1, true, false)?;
                surf1.evaluate_point(k, uv1)?;
                vecs.push(d1.g2);

                let d2 = compute_higher_order_derivs(k, surf2, uv2, uv_tan2, true, false)?;
                vecs.push(d2.g2);

                surf2.evaluate_point(k, uv2)?;

                vecs.push(d1.g3.ok_or_else(|| SolidError::sm_err(OP))?);
                vecs.push(d2.g3.ok_or_else(|| SolidError::sm_err(OP))?);
            }
            higher_order_vecs = Some(vecs);
        }

        BSplineCurve::fair_blend_curve_derivatives(
            &points,
            &mut vectors,
            higher_order_vecs.as_deref_mut(),
            false,
        )?;
        let s = self.blend_scale;
        for v in &mut vectors {
            *v *= s;
        }
        let mut degree = 3;
        if let Some(h) = higher_order_vecs.as_mut() {
            if h.len() == 2 {
                for v in h.iter_mut() {
                    *v = *v * s * s;
                }
                degree = 5;
            }
            if h.len() == 4 {
                for (n, v) in h.iter_mut().enumerate() {
                    *v = if n < 2 { *v * s * s } else { *v * s * s * s };
                }
                degree = 7;
            }
        }
        // At thirds (fork fd3b6a7): the Hermite's inner control points (P0 + D0/n,
        // P1 - D1/n for degree n) a third of the way from each contact to where the two
        // contact tangent lines meet, so D_i is n/3 of that distance; the higher
        // derivatives keep their curvature with it.
        if self.at_thirds {
            let [d0, d1] = vectors;
            let l0 = d0.length();
            let l1 = d1.length();
            let w = p1 - p2;
            if l0 > EFF_ZERO && l1 > EFF_ZERO {
                let d0 = d0 / l0;
                let d1 = d1 / l1;
                // Lines P0 + u d0 and P1 - v d1 (u, v > 0 run towards the corner).
                let b = -d0.dot(d1);
                let den = 1.0 - b * b;
                if den > 1.0e-10 {
                    let dd = d0.dot(w);
                    let e = -d1.dot(w);
                    let u = (b * e - dd) / den;
                    let v = (e - b * dd) / den;
                    if u > EFF_ZERO && v > EFF_ZERO {
                        let n = degree as f64;
                        let k0 = (n / 3.0) * u / l0;
                        let k1 = (n / 3.0) * v / l1;
                        let [v0, v1] = &mut vectors;
                        *v0 *= k0;
                        *v1 *= k1;
                        if let Some(h) = higher_order_vecs.as_mut() {
                            let len = h.len();
                            for (index, vec) in h.iter_mut().enumerate() {
                                *vec = match index {
                                    0 => *vec * (k0 * k0),
                                    1 => *vec * (k1 * k1),
                                    2 if len == 4 => *vec * (k0 * k0 * k0),
                                    3 if len == 4 => *vec * (k1 * k1 * k1),
                                    _ => *vec,
                                };
                            }
                        }
                    }
                }
            }
        }
        BSplineCurve::create_interpolating_curve(
            ParamMethod::Uniform,
            3,
            degree,
            &points,
            &vectors,
            higher_order_vecs.as_deref(),
            false,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blend::fillet_solver::p1::OffsetSurfaceSlot;
    use crate::nurbs::crv_basic::Curve as NurbCurve;
    use crate::nurbs::math::{CPoint, NOW};

    fn line(a: Point3, b: Point3) -> BSplineCurve {
        let pw = vec![
            CPoint::new(a.x, a.y, a.z, NOW),
            CPoint::new(b.x, b.y, b.z, NOW),
        ];
        BSplineCurve::from_nurb(
            NurbCurve::new(pw, 1, vec![0.0, 0.0, 1.0, 1.0]).expect("curve"),
            3,
            false,
        )
    }

    fn p(x: f64, y: f64, z: f64) -> Point3 {
        Point3::new(x, y, z)
    }

    fn plane_surface(z: f64) -> Surface {
        Surface::from(
            BSplineSurface::create_bilinear_surface(
                p(0.0, 0.0, z),
                p(1.0, 0.0, z),
                p(0.0, 1.0, z),
                p(1.0, 1.0, z),
            )
            .expect("bilinear"),
        )
    }

    fn cr() -> ConstantRadiusFs {
        cr_in(&mut FilletBrep::new(), FilletSolverId(0))
    }

    fn cr_in(pseudo: &mut FilletBrep, id: FilletSolverId) -> ConstantRadiusFs {
        ConstantRadiusFs::from_surfaces(
            &Kernel::default(),
            pseudo,
            id,
            1e-6,
            0.5,
            0.03,
            [1.0, 2.0],
            [
                (plane_surface(0.0), SurfaceId::Free(0)),
                (plane_surface(1.0), SurfaceId::Free(1)),
            ],
            [true, false],
        )
        .expect("constant radius solver")
    }

    #[test]
    fn constant_radius_from_surfaces_finishes_construction() {
        let mut pseudo = FilletBrep::new();
        let fs = cr_in(&mut pseudo, FilletSolverId(3));
        // the SmFilletSolver constructor's geometry, with copies of both offsets
        assert_eq!(fs.solver.fillet_geoms.len(), 1);
        let fg = pseudo.geom(fs.solver.fillet_geoms[0]).expect("geometry");
        assert_eq!(fg.fillet_solver(), Some(FilletSolverId(3)));
        for (rail, d) in [(0, 1.0), (1, -2.0)] {
            let off = fg
                .offset_surface(rail, None)
                .expect("own offset")
                .expect("offset copy");
            assert_eq!(off.offset_distance(), d);
            assert!(off.surface.allow_extension());
        }
    }

    #[test]
    fn constant_radius_from_surfaces_offsets_by_oriented_radius() {
        let mut fs = cr();
        assert_eq!(fs.solver.orientations, [1.0, -1.0]);
        assert_eq!(fs.surface(0).expect("s0").offset_distance(), 1.0);
        assert_eq!(fs.surface(1).expect("s1").offset_distance(), -2.0);
        assert!(fs.surface(1).expect("s1").surface.allow_extension());
        assert_eq!(fs.jacobian_size(), 4);
        assert!(!fs.offset_radii_can_change());
        fs.solver.surfaces[0]
            .as_mut()
            .expect("s0")
            .surface
            .set_offset_distance(7.0);
        let r = fs.setup_offset_values(&TsectPnt::new()).expect("setup");
        assert_eq!(r, 1.0);
        assert_eq!(fs.surface(0).expect("s0").offset_distance(), 1.0);
        assert!(constant_radius_is_kind_of(CONSTANT_RADIUS_FS_TYPE));
        assert!(constant_radius_is_kind_of(FILLET_SOLVER_TYPE));
        assert!(!constant_radius_is_kind_of(CONSTANT_DISTANCE_FS_TYPE));
    }

    #[test]
    fn constant_radius_jacobian_of_parallel_planes() {
        let k = &Kernel::default();
        let mut fs = cr();
        let (mut ne, mut np) = (0, 0);
        let mut offs = [0, 0];
        let mut f = vec![0.0; 3];
        let r = fs.load_jacobian(
            k,
            &mut ne,
            &mut np,
            0,
            [None, None],
            &mut offs,
            &[0.5; 4],
            &mut f,
            None,
        );
        // z = 0 offset by 1, z = 1 offset by -2: F - G = (0, 0, 2)
        assert!(!r.expect("jacobian"));
        assert_eq!((ne, np), (3, 4));
        assert_eq!(offs, [0, 2]);
        assert_eq!(f, vec![0.0, 0.0, 2.0]);
    }

    fn cd() -> ConstantDistanceFs {
        let k = Kernel::default();
        let mut solver = FilletSolver::new(1e-6, 0.5, 0.03);
        solver.surfaces = [
            Some(OffsetSurfaceSlot {
                key: 0,
                surface: OffsetSurface::new(
                    &k,
                    0.0,
                    plane_surface(0.0),
                    SurfaceId::Free(0),
                    SS_UNKNOWN,
                )
                .expect("offset"),
            }),
            Some(OffsetSurfaceSlot {
                key: 1,
                surface: OffsetSurface::new(
                    &k,
                    0.0,
                    plane_surface(1.0),
                    SurfaceId::Free(1),
                    SS_UNKNOWN,
                )
                .expect("offset"),
            }),
        ];
        solver.orientations = [1.0, -1.0];
        ConstantDistanceFs {
            solver,
            distance: 2.0,
        }
    }

    #[test]
    fn constant_distance_setup_and_initial_values() {
        let mut fs = cd();
        let mut t = TsectPnt::new();
        t.user_doubles[0] = 0.75;
        assert_eq!(fs.setup_offset_values(&t).expect("setup"), 0.75);
        assert_eq!(fs.solver.rail(1).expect("s1").offset_distance(), -0.75);

        let k = Kernel::default();
        let mut s1 = fs.solver.rail(0).expect("s0").clone();
        let mut ivls = ExtentNd::new(4);
        let mut per = vec![true];
        let mut guess = vec![0.1, 0.2];
        let v = fs
            .load_initial_values(
                &k,
                None,
                false,
                0,
                InitialSurface::Other(&mut s1),
                0,
                1,
                &mut ivls,
                &mut per,
                &mut guess,
            )
            .expect("init");
        assert_eq!(v.surface2_rail, None);
        assert_eq!(v.surface2_index, 1);
        let ivl = ivls.get(3).expect("ivl");
        assert_eq!((ivl.min(), ivl.max()), (0.0, 20.0));
        assert_eq!(per, vec![true, false]);
        let r = 2.0 * 2.0_f64.sqrt() / 2.0;
        assert_eq!(guess, vec![0.1, 0.2, r]);
        assert_eq!(fs.solver.rail(0).expect("s0").offset_distance(), r);

        // out of range interval index: upstream writes past rIntervals
        let r = fs.load_initial_values(
            &k,
            None,
            false,
            0,
            InitialSurface::Other(&mut s1),
            0,
            3,
            &mut ivls,
            &mut per,
            &mut guess,
        );
        assert!(matches!(r, Err(SolidError::InvalidInput { .. })));

        // the guess on the other plane (a global point solve on its offset) is below
        // the first guess
        let mut ivls = ExtentNd::new(8);
        let mut per = vec![];
        let mut guess = vec![0.1, 0.2];
        let v = fs
            .load_initial_values(
                &k,
                None,
                true,
                0,
                InitialSurface::Rail(0),
                0,
                0,
                &mut ivls,
                &mut per,
                &mut guess,
            )
            .expect("surface 2 guess");
        assert_eq!(v.surface2_rail, Some(1));
        assert_eq!(v.surface2_index, 2);
        assert_eq!(guess.len(), 5);
        assert!((guess[2] - 0.1).abs() < 1e-9 && (guess[3] - 0.2).abs() < 1e-9);
        assert_eq!(per, vec![false, false, false]);
        // the temporary out-of-bounds change is undone
        let base2 = fs.solver.rail(1).expect("s1").base_surface();
        assert_eq!(out_of_bounds_enabled(base2), Some(false));
        assert!(
            fs.is_kind_of(CONSTANT_DISTANCE_FS_TYPE)
                && fs.is_kind_of(crate::geom::object::OBJECT_TYPE)
        );
        assert!(fs.can_make_analytic());
        assert_eq!(fs.jacobian_size(), 5);
    }

    #[test]
    fn base_curve_snaps_extends_and_keeps_the_extension() {
        let k = &Kernel::default();
        let mut fs = CurveBasedFs::new(1e-6, 0.5, 0.03);
        assert!(matches!(
            fs.evaluate_base_curve(k, 0.5, 1),
            Err(SolidError::InvalidInput { .. })
        ));
        fs.set_original_curve(Some(Curve::from(line(p(0.0, 0.0, 0.0), p(2.0, 0.0, 0.0)))));
        let v = fs.evaluate_base_curve(k, 1.0 + 1e-12, 0).expect("snap");
        assert_eq!(v.first().copied(), Some(p(2.0, 0.0, 0.0)));
        assert!(fs.extended.is_none());
        let v = fs.evaluate_base_curve(k, 1.5, 1).expect("extend");
        assert!((v[0].x - 3.0).abs() < 1e-12);
        let ext = fs.extended.as_ref().expect("extended").natural_interval();
        assert!(ext.max() >= 2.0 - 1e-12);
        // a non B-spline curve cannot be extended
        let mut h = CurveBasedFs::new(1e-6, 0.5, 0.03);
        h.set_original_curve(Some(Curve::from(
            crate::curve::hermite_curve::HermiteCurve::new(
                p(0.0, 0.0, 0.0),
                p(1.0, 0.0, 0.0),
                p(1.0, 1.0, 0.0),
                p(0.0, 1.0, 0.0),
                3,
            ),
        )));
        assert!(matches!(
            h.evaluate_base_curve(k, 2.0, 0),
            Err(SolidError::Upstream { .. })
        ));
        assert_eq!(fs.type_string(), "SmCurveBasedFS_TYPE");
    }

    #[test]
    fn surface_surface_extension_of_a_plane() {
        let k = Kernel::default();
        let fs = SurfaceSurfaceFs {
            base: cr(),
            uv_guesses: [Point2::new(0.5, 0.5); 2],
            trim_type: BoundaryTrimmingType::Minimal,
        };
        let mut d = Extent2d::from_points(Point2::new(0.0, 0.0), Point2::new(1.0, 2.0));
        fs.setup_offset_extension(&k, &plane_surface(0.0), &mut d)
            .expect("extension");
        assert_eq!(
            (d.min().x, d.min().y, d.max().x, d.max().y),
            (-0.5, -1.0, 1.5, 3.0)
        );
        let mut fs = fs;
        fs.set_fillet_trim_type(BoundaryTrimmingType::Bevel);
        assert_eq!(fs.trim_type, BoundaryTrimmingType::Bevel);
    }

    #[test]
    fn variable_radius_queries() {
        let k = &Kernel::default();
        let mut cb = CurveBasedFs::new(1e-6, 0.5, 0.03);
        cb.solver
            .set_fillet_surface_generator(Some(FilletSurfaceGenerator::Circular(
                CircularCrossSectionFsg::default(),
            )));
        let mut fs = VariableRadiusFs {
            curve_based: cb,
            law: FilletLaw::new(FilletLawKind::Linear {
                start_radius: 1.0,
                end_radius: 1.0 + 1e-13,
            }),
            law_orientation: false,
            law_interval: Extent1d::from_min_max(0.0, 1.0),
        };
        assert!(!fs.offset_radii_can_change().expect("can change"));
        fs.law.kind = FilletLawKind::Linear {
            start_radius: 1.0,
            end_radius: 2.0,
        };
        assert!(fs.offset_radii_can_change().expect("can change"));
        fs.curve_based.solver.set_fillet_surface_generator(None);
        assert!(fs.offset_radii_can_change().is_err());
        // the base SetExtensionFlags dereferences the missing edge uses
        assert!(matches!(
            fs.set_extension_flags(
                k,
                &Brep::new(),
                &crate::blend::fillet_solver::p1::NoExtensionCorners
            ),
            Err(SolidError::InvalidInput { .. })
        ));
        // the law evaluates (1.25); the solver has no offset surfaces to set
        let mut t = TsectPnt::new();
        t.user_doubles[0] = 0.25;
        assert!(matches!(
            fs.setup_offset_values(&t),
            Err(SolidError::InvalidInput { .. })
        ));
        fs.set_fillet_law_interval(Extent1d::from_min_max(2.0, 3.0));
        assert_eq!(fs.fillet_law_interval().max(), 3.0);
        assert_eq!(fs.jacobian_size(), 5);
    }

    #[test]
    fn linear_and_circular_sections_of_a_box_edge() {
        let k = Kernel::default();
        let r1 = line(p(0.0, 0.0, 1.0), p(5.0, 0.0, 1.0));
        let r2 = line(p(0.0, 1.0, 0.0), p(5.0, 1.0, 0.0));
        let ruled = LinearCrossSectionFsg
            .create_surface(&r1, &r2)
            .expect("ruled");
        assert!(ruled.gw_nurb().is_some());

        let mut c = Curve::from(line(p(0.0, 1.0, 1.0), p(5.0, 1.0, 1.0)));
        let (mut a, mut b) = (r1.copy(), r2.copy());
        let s = CircularCrossSectionFsg::default()
            .create_surface_from_curves(&k, &mut c, &mut a, &mut b, 1e-6, false)
            .expect("circular");
        // knots were added to reach four cross sections
        assert_eq!(a.knots(None).expect("knots").0.len(), 4);
        let corner = s.evaluate_point(Point2::new(0.0, 0.0)).expect("point");
        assert!(corner.distance(p(0.0, 0.0, 1.0)) < 1e-6);

        // a zero radius section
        let mut c = Curve::from(line(p(0.0, 0.0, 1.0), p(5.0, 0.0, 1.0)));
        let (mut a, mut b) = (r1.copy(), r2.copy());
        assert!(matches!(
            CircularCrossSectionFsg::default()
                .create_surface_from_curves(&k, &mut c, &mut a, &mut b, 1e-6, false),
            Err(SolidError::Upstream { .. })
        ));
    }

    #[test]
    fn blend_needs_matching_knot_counts() {
        let k = Kernel::default();
        let mut c = line(p(0.0, 1.0, 1.0), p(5.0, 1.0, 1.0));
        let mut a = line(p(0.0, 0.0, 1.0), p(5.0, 0.0, 1.0));
        let mut b = line(p(0.0, 1.0, 0.0), p(5.0, 1.0, 0.0));
        b.insert_one_knot(0.5, 1).expect("knot");
        let none = RailSurface {
            surface: None,
            uv_curve: None,
        };
        let r = BlendCurveCrossSectionFsg::default()
            .create_surface_from_curves(&k, &mut c, &mut a, &mut b, 1e-6, false, none, none);
        assert!(matches!(r, Err(SolidError::Upstream { .. })));
        let fsg = BlendCurveCrossSectionFsg::default();
        assert_eq!((fsg.continuity(), fsg.blend_scale()), (1, 1.5));
        assert_eq!(fsg.generator_type(), FilletSurfaceGeneratorType::BlendCurve);
    }

    #[test]
    fn guess_sphere_solve_starts_from_the_sphere() {
        let mut fs = cr();
        let g = GuessSphereSolve::new(p(1.0, 2.0, 3.0), 0.5, &mut fs);
        assert_eq!(g.base.plane_abc, p(1.0, 2.0, 3.0));
        assert_eq!(g.base.plane_d, 0.5);
        assert!(!g.base.plane_adjusts);
    }

    #[test]
    fn circular_centre_line_dispatches_on_its_line_kind() {
        let k = Kernel::default();
        let r1 = line(p(0.0, 0.0, 1.0), p(5.0, 0.0, 1.0));
        let r2 = line(p(0.0, 1.0, 0.0), p(5.0, 1.0, 0.0));
        // an SmLine centre line whose NURB was elevated to degree 3 (as
        // MakeCurvesCompatible does in SmOffsetGeometryCreation::EdgeuseOffset):
        // SmLine::GetDegree still answers 1, so the ruled (sweep) branch runs
        let mut lc = Curve::from(crate::curve::line::Line::from_points(
            p(0.0, 1.0, 1.0),
            p(5.0, 1.0, 1.0),
            3,
        ));
        nurb_part_mut(&mut lc)
            .expect("nurb")
            .expect("line kind")
            .degree_elevate(3)
            .expect("elevate");
        assert_eq!(lc.degree().expect("degree"), 1);
        let (mut a, mut b) = (r1.copy(), r2.copy());
        let swept = CircularCrossSectionFsg::default()
            .create_surface_from_curves(&k, &mut lc, &mut a, &mut b, 1e-6, false)
            .expect("swept");
        // the same NURB as a plain B-spline (degree 3) takes the skinned branch
        let mut bc = Curve::from(nurb_part_mut(&mut lc).expect("nurb").expect("kind").clone());
        assert_eq!(bc.degree().expect("degree"), 3);
        let (mut a, mut b) = (r1.copy(), r2.copy());
        let skinned = CircularCrossSectionFsg::default()
            .create_surface_from_curves(&k, &mut bc, &mut a, &mut b, 1e-6, false)
            .expect("skinned");
        let rows = |s: &BSplineSurface| {
            (
                s.number_control_points(SurfParamType::U),
                s.number_control_points(SurfParamType::V),
            )
        };
        // the ruled surface spans the two end sections; the skin all four
        assert_eq!(rows(&swept), (2, 7));
        assert_eq!(rows(&skinned), (4, 7));

        // a centre line that is no SmBSplineCurve kind cannot bind rCenterLine
        let mut hc = Curve::from(crate::curve::hermite_curve::HermiteCurve::default());
        let (mut a, mut b) = (r1.copy(), r2.copy());
        assert!(matches!(
            CircularCrossSectionFsg::default()
                .create_surface_from_curves(&k, &mut hc, &mut a, &mut b, 1e-6, false),
            Err(SolidError::InvalidInput { .. })
        ));
    }
}
