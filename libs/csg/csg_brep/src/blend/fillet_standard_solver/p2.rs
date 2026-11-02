//! Standard fillet solvers (part 2): the constant-radius solver's initial values, the
//! assisted solver's plane solves (with the file-local guess sphere/plane equations), the
//! constant-distance and variable-radius constructors, the variable-radius equations,
//! initial values and geometry recalculation, the surface/surface solver's constructor
//! and geometry, the cross-section generators' `CreateSurface` and the solvers'
//! run-time type queries.
//!
//! Upstream: `source/SMLib/src/SmFilletStandardSolver.cpp`,
//! `source/SMLib/inc/SmFilletStandardSolver.h`.
//!
//! | Rust                                                     | Upstream                                              |
//! |----------------------------------------------------------|-------------------------------------------------------|
//! | [`constant_radius_load_initial_values`], [`ConstantRadiusAssistedFs::load_initial_values`], [`SurfaceSurfaceFs::load_initial_values`] | `SmConstantRadiusFS::LoadInitialValues` |
//! | [`ConstantRadiusOps`]                                    | the `SetupOffsetExtension` virtual of the `SmConstantRadiusFS` classes |
//! | [`GuessSphereEnfo`], [`GuessSphereEnfo::evaluate`]       | `SmGuessSphereSolveENFO` (as `GuessPointOnPlaneSolve` evaluates it), `SmGuessSphereSolveENFO::Evaluate` |
//! | [`ConstantRadiusAssistedFs::guess_point_on_plane_solve`] | `SmConstantRadiusAssistedFS::GuessPointOnPlaneSolve`  |
//! | [`point_on_plane_solve`]                                 | `SmConstantRadiusAssistedFS::PointOnPlaneSolve`       |
//! | [`constant_radius_assisted_is_kind_of`]                  | `SmConstantRadiusAssistedFS::IsKindOf`                |
//! | [`ConstantDistanceFs::from_edgeuse`]                     | `SmConstantDistanceFS::SmConstantDistanceFS`          |
//! | [`VariableRadiusFs::from_edgeuse`]                       | `SmVariableRadiusFS::SmVariableRadiusFS` (edge use)   |
//! | [`VariableRadiusFs::from_surfaces`]                      | `SmVariableRadiusFS::SmVariableRadiusFS` (surfaces)   |
//! | [`VariableRadiusFs::re_calc_fillet_geom`]                | `SmVariableRadiusFS::ReCalcFilletGeom`                |
//! | [`VariableRadiusFs::load_jacobian`]                      | `SmVariableRadiusFS::LoadJacobian`                    |
//! | [`VariableRadiusFs::load_initial_values`]                | `SmVariableRadiusFS::LoadInitialValues`               |
//! | [`VariableRadiusFs::is_kind_of`]                         | `SmVariableRadiusFS::IsKindOf`                        |
//! | `impl FilletSolverOps for VariableRadiusFs`              | the `SmVariableRadiusFS` virtuals (`GetJacobianSize`, `SetupOffsetValues`, `OffsetRadiiCanChange`, `LoadJacobian`, `LoadInitialValues`, `GetType`, `IsKindOf`, `GetSolverType`) |
//! | `impl FilletSolverOps for ConstantDistanceFs`            | the `SmConstantDistanceFS` virtuals (`CanMakeAnalytic`, `GetJacobianSize`, `GetSolverType`, `LoadJacobian`, `LoadInitialValues`, `SetupOffsetValues`, `GetType`, `IsKindOf`) and `GetDistance` |
//! | [`CircularCrossSectionFsg::create_surface`]              | `SmCircularCrossSectionFSG::CreateSurface`            |
//! | [`BlendCurveCrossSectionFsg::create_surface`]            | `SmBlendCurveCrossSectionFSG::CreateSurface`          |
//! | [`SurfaceSurfaceFs::new`]                                | `SmSurfaceSurfaceFS::SmSurfaceSurfaceFS`              |
//! | [`SurfaceSurfaceFs::calc_fillet_geom`], [`SurfaceSurfaceFs::calc_fillet_start`], [`SurfaceSurfaceStart`] | `SmSurfaceSurfaceFS::CalcFilletGeom` (up to `GetPseudoBrep`; the rest is `surface_surface`'s) |
//! | [`SurfaceSurfaceFs::is_kind_of`]                         | `SmSurfaceSurfaceFS::IsKindOf`                        |
//!
//! Deviations:
//! - **Object model** as in `blend.fillet_standard_solver_p1` (the solvers are its
//!   structs). The tangent-rollover guess of `SmConstantRadiusFS::LoadInitialValues`
//!   comes from [`ConstantRadiusOps::tangent_rollover_guess`]: every scope that sets
//!   the solver's geometry for the surfaces (`SmFilletGeom::RailEdgeuseIntersect`, the
//!   corners' `ReCalcFilletGeom` scopes) hands the solver that geometry's rail guesses,
//!   taken when the scope starts (`blend::fillet_solver::surfaces`); context-free
//!   callers have none.
//! - **Virtual dispatch.** `SetupOffsetExtension` is dispatched on the solver's class
//!   through [`ConstantRadiusOps`]: `SmSurfaceSurfaceFS` has its own version
//!   (`blend.fillet_standard_solver_p1`); every other constant-radius solver uses the
//!   base `SmFilletSolver::SetupOffsetExtension` (unit `blend.fillet_solver_p1`).
//!   [`point_on_plane_solve`] takes `*this` as a `blend.fillet_solver_p1`
//!   [`FilletSolverOps`] (`dyn`), so `GetJacobianSize`, `LoadInitialValues` and the
//!   `LoadJacobian` its `SmFilletSphereSolveENFO` calls dispatch on the solver's class:
//!   `SmConstantRadiusAssistedFS` (`blend.fillet_custom_solver`'s
//!   `impl FilletSolverOps for ConstantRadiusAssistedFs`) or the custom solvers derived
//!   from it, which inherit the method. `blend.fillet_intersector`'s
//!   `FilletSolverOps::point_on_plane_solve` of all three classes calls it (without a
//!   B-rep).
//! - **Surfaces.** As in `blend.fillet_custom_solver`, `crSurface1` is the caller's
//!   offset surface (a copy when upstream passes `GetSurface(i)`), except for
//!   `SmVariableRadiusFS::LoadInitialValues`, which re-evaluates it after changing the
//!   solver's offset distances: there `None` stands for `GetSurface(lRailIndex)`.
//!   `PointOnPlaneSolve` passes a copy of `*GetSurface(0)` and hands its base extension
//!   back to the solver's offset afterwards, unless the solver's own offset was extended
//!   meanwhile (the custom solvers' `GuessPointOnPlaneSolve`), which upstream does on the
//!   one object. `GuessPointOnPlaneSolve` runs on the solver itself (`&mut`): its
//!   offsets keep the extensions the `SmGuessSphereSolveENFO` evaluations make.
//! - **Constructors.** `SmConstantDistanceFS` and `SmVariableRadiusFS` from an edge use
//!   signal `SM_ERR` (or nothing) for tangent faces and leave a solver without edge
//!   uses or offset surfaces: the same half-built solver is returned (`Ok`). They take
//!   the B-rep mutably (`SmEdgeuse::EvaluateBinormal` may create a UV trim curve).
//!   Every constructor takes the executive's pseudo B-rep ([`FilletBrep`]) and the
//!   solver's identity ([`FilletSolverId`]): the `SmFilletSolver` constructor's default
//!   fillet geometry is stored there (`FilletSolver::new_in`, `CurveBasedFs::new_in`),
//!   and `FinishConstruction` (`FilletSolver::finish_construction`) installs copies of
//!   the offsets into it where upstream calls it: at the end of the
//!   `SmConstantDistanceFS` and edge-use `SmVariableRadiusFS` constructors (not on their
//!   tangent-face early returns), not in the surface `SmVariableRadiusFS` constructor.
//!   A construction that fails after the base leaves that geometry in `pseudo` (the
//!   arena does not delete; upstream's half-built solver holds it).
//! - **`SmGuessSphereSolveENFO`.** [`GuessSphereEnfo`] holds the plane state and the
//!   solver as the `SmConstantRadiusFS` its `Evaluate` casts `m_crFilletSolver` to
//!   (`blend.fillet_standard_solver_p1`'s `GuessSphereSolve` holds it as a
//!   `dyn FilletSolverOps`, which cannot be cast back for the qualified
//!   `SmConstantRadiusFS::LoadJacobian`).
//! - **`ReCalcFilletGeom`** runs `DoLawIntersection` on its `SmFilletIntersector`
//!   (`FilletIntersector::do_law_intersection`), whose overrides step through this
//!   solver's virtuals (`blend.fillet_solver_p4`'s `IntersectorSolver`: the
//!   intersector's surfaces are copies of the solver's offsets, kept at the same offset
//!   distances and base extensions as upstream's one object). The law argument is a copy
//!   of the solver's law (`SmFilletIntersector::EvaluateLawPoint` does not read it).
//!   `SetExtensionFlags` through the `FilletSolverOps` hook is
//!   [`VariableRadiusFs::set_extension_flags`].
//! - **Surface/surface geometry.** `CalcFilletGeom` steps with the ported
//!   `SmFilletSolver::StepSolve` (without a B-rep: the executive of a surface/surface
//!   fillet has none). Its part from `GetPseudoBrep` on needs the live executive's
//!   pseudo-B-rep and original faces (`surface_surface`); without them
//!   [`SurfaceSurfaceFs::calc_fillet_geom`] returns [`SolidError::Unported`] there.
//! - **Arguments.** Out-params become return values (`rbFoundSolution` /
//!   `rTsectPnt`: `Some` point when found). `SmContext` arguments are dropped or become
//!   the [`Kernel`]. The generators' `CreateSurface` take the solver's tolerance and
//!   type (the only solver values they read).
//! - **Indexing.** `rF`, the Jacobian, `crX`, `rGuessT` and `rIntervals` are indexed
//!   with checks: an index upstream would read or write out of bounds is
//!   [`SolidError::InvalidInput`].
//! - No recursion; the loops are bounded by the caller's arrays and the solver's
//!   iteration limits.

use crate::blend::fillet_custom_solver::{
    face_owner, offset_compute_inward_vector, offset_evaluate_1st, offset_evaluate_point,
    offset_local_point_solve, surface_global_point_solve, ConstantRadiusAssistedFs, InitialValues,
    JacobianLoad, OffsetSurface, SurfaceId,
};
use crate::blend::fillet_geom::p1::{
    FilStatus, FilletBrep, FilletGeomId, FilletSolverId, FilletVertexType,
};
use crate::blend::fillet_intersector::FilletIntersector;
use crate::blend::fillet_solver::p1::{
    FilletSolver, FilletSolverOps, FilletSphereSolveEnfo, OffsetSurfaceSlot,
};
use crate::blend::fillet_solver::p3::StepSolveOutcome;
use crate::blend::fillet_solver::p4::IntersectorSolver;
use crate::blend::fillet_standard_solver::p1::SolverRails;
use crate::blend::fillet_standard_solver::p1::{
    constant_radius_is_kind_of, BlendCurveCrossSectionFsg, BoundaryTrimmingType,
    CircularCrossSectionFsg, ConstantDistanceFs, ConstantRadiusFs, CurveBasedFs,
    DistanceInitialValues, FilletLaw, FilletSolverType, JacobianSurfaces, RailSurface,
    SurfaceSurfaceFs, VariableRadiusFs, CONSTANT_RADIUS_ASSISTED_FS_TYPE, CURVE_BASED_FS_TYPE,
};
use crate::curve::curve::p1::GlobalPointSolveOptions;
use crate::curve::curve::{Curve, CurveKind};
use crate::error::{ignored, Result, SolidError};
use crate::geom::extent1d::{Extent1d, ExtentNd};
use crate::geom::extent2d::Extent2d;
use crate::geom::geom_utility::p1::line_closest_point;
use crate::geom::global_solver::{SolutionRequested, SolverOperation};
use crate::geom::local_solve_nd::{BoundaryHandler, EvalNFunctions, LocalSolveNd, NdSolution};
use crate::geom::matrix::Matrix;
use crate::geom::vector2d::Point2;
use crate::intersect::adv_surface_intersector::NoSurfaceHandle;
use crate::intersect::surface_intersector::p1::{SrfIndex, TsectPnt};
use crate::intersect::surface_intersector::p4::LawIntersectionOptions;
use crate::kernel::Kernel;
use crate::math::{Point3, Vec3, EFF_ZERO, EFF_ZERO_SQ, EFF_ZERO_SQRT, PI};
use crate::nurbs::b_spline_curve::BSplineCurve;
use crate::nurbs::b_spline_surface::BSplineSurface;
use crate::nurbs::b_spline_surface_n_lib::p2::{EvaluateFlags, OrientType};
use crate::nurbs::knots::Side;
use crate::surface::offset_surface::SS_UNKNOWN;
use crate::surface::surface::p3::{out_of_bounds_enabled, set_out_of_bounds_enabled};
use crate::surface::surface::Surface;
use crate::topology::edgeuse::p1 as edgeuse;
use crate::topology::edgeuse::p3 as edgeuse_p3;
use crate::topology::face::p2 as face;
use crate::topology::{edge, vertexuse, Brep, EdgeuseId, ObjectHandle, VertexuseId};

const OP: &str = "blend::fillet_standard_solver::p2";
const OP_CR_INIT: &str = "blend::fillet_standard_solver::p2::constant_radius_load_initial_values";
const OP_GUESS: &str = "blend::fillet_standard_solver::p2::GuessSphereSolve::evaluate";
const OP_PLANE: &str =
    "blend::fillet_standard_solver::p2::ConstantRadiusAssistedFs::point_on_plane_solve";
const OP_CD_CTOR: &str = "blend::fillet_standard_solver::p2::ConstantDistanceFs::from_edgeuse";
const OP_VR_CTOR: &str = "blend::fillet_standard_solver::p2::VariableRadiusFs::new";
const OP_VR_JAC: &str = "blend::fillet_standard_solver::p2::VariableRadiusFs::load_jacobian";
const OP_VR_INIT: &str = "blend::fillet_standard_solver::p2::VariableRadiusFs::load_initial_values";
const OP_VR_RECALC: &str =
    "blend::fillet_standard_solver::p2::VariableRadiusFs::re_calc_fillet_geom";
const OP_SS_CALC: &str = "blend::fillet_standard_solver::p2::SurfaceSurfaceFs::calc_fillet_geom";

// ---- shared helpers -----------------------------------------------------------------------

fn at(x: &[f64], i: usize, op: &'static str) -> Result<f64> {
    x.get(i)
        .copied()
        .ok_or_else(|| SolidError::invalid_input(op, "array index out of range"))
}

fn put(f: &mut [f64], i: usize, v: f64, op: &'static str) -> Result<()> {
    let slot = f
        .get_mut(i)
        .ok_or_else(|| SolidError::invalid_input(op, "rF index out of range"))?;
    *slot = v;
    Ok(())
}

fn put_j(j: &mut Matrix, row: usize, col: usize, v: f64, op: &'static str) -> Result<()> {
    j.set_at(row, col, v)
        .map_err(|_| SolidError::invalid_input(op, "Jacobian index out of range"))
}

fn plus(a: usize, b: usize, op: &'static str) -> Result<usize> {
    a.checked_add(b)
        .ok_or_else(|| SolidError::invalid_input(op, "index overflow"))
}

fn minus(a: usize, b: usize, op: &'static str) -> Result<usize> {
    a.checked_sub(b)
        .ok_or_else(|| SolidError::invalid_input(op, "index underflow"))
}

fn interval_at<'a>(ivls: &'a mut ExtentNd, i: usize, op: &'static str) -> Result<&'a mut Extent1d> {
    ivls.get_mut(i)
        .ok_or_else(|| SolidError::invalid_input(op, "rIntervals index out of range"))
}

/// `if (status != SM_SUCCESS) return SM_ERR;`: a failure becomes `SM_ERR`, but
/// [`SolidError::Unported`] and [`SolidError::InvalidInput`] (an input upstream reads
/// out of bounds) are kept.
fn as_sm_err(e: SolidError, op: &'static str) -> SolidError {
    match e {
        SolidError::Unported { .. }
        | SolidError::InvalidInput { .. }
        | SolidError::PortInvariant { .. } => e,
        _ => SolidError::sm_err(op),
    }
}

/// `Evaluate1stDerivatives(uv, TRUE, TRUE, ..)` of an offset surface:
/// `EvaluateNormal(uv, TRUE, TRUE, ..)` (`SmOffsetSurface::EvaluateNormal`), keeping a
/// base extension it makes.
fn offset_evaluate_normal(k: &Kernel, s: &mut OffsetSurface, uv: Point2) -> Result<Vec3> {
    s.surface
        .evaluate_normal_extending(k, uv, Side::Left, Side::Left)
}

/// `SmTemporaryChangeValue<SmBoolean>(pBSpline ? pBSpline->GetOutOfBoundsEnabled() :
/// bTmp, TRUE)` on an offset's base: turns on out-of-bounds evaluation of an
/// `SmBSplineSurface` base (a B-spline surface, a plane or a surface of revolution) and
/// returns the value to restore; `None` for other kinds (upstream changes a local).
fn enable_base_out_of_bounds(s: &mut OffsetSurface) -> Option<bool> {
    let base = s.surface.base_surface_mut();
    let old = out_of_bounds_enabled(base)?;
    set_out_of_bounds_enabled(base, true);
    Some(old)
}

fn restore_base_out_of_bounds(s: &mut OffsetSurface, old: Option<bool>) {
    if let Some(old) = old {
        set_out_of_bounds_enabled(s.surface.base_surface_mut(), old);
    }
}

/// `SE(SmEdgeuse::EvaluateBinormal(dT, FALSE, rPnt, rBinVec, NULL, &rFaceuseNormal))`:
/// the binormal and face use normal of an edge use. The status is only signalled:
/// after a failure the binormal keeps its `SM_UNDEF_DOUBLE` initial value and the face
/// use normal is zero (`EvaluateBinormal` zeroes it once the edge use passed its loop
/// check) or `SM_UNDEF_DOUBLE`. Only [`SolidError::Unported`] propagates.
fn binormal(k: &Kernel, brep: &mut Brep, eu: EdgeuseId, t: f64) -> Result<(Vec3, Vec3)> {
    let options = edgeuse_p3::BinormalOptions::default();
    if let Some(r) = ignored(edgeuse_p3::evaluate_binormal(
        k, brep, eu, t, false, &options,
    ))? {
        return Ok((r.binormal, r.face_normal));
    }
    let undef = Vec3::UNDEF;
    let normal = if edgeuse::is_loop_edgeuse(brep, eu)? {
        Vec3::new(0.0, 0.0, 0.0)
    } else {
        undef
    };
    Ok((undef, normal))
}

/// `pEdgeuse->GetLoopuse()->GetFaceuse()->GetOrientation() == SM_OT_OPPOSITE`.
fn faceuse_opposite(brep: &Brep, eu: EdgeuseId, op: &'static str) -> Result<bool> {
    let fu = edgeuse::faceuse(brep, eu)?
        .ok_or_else(|| SolidError::invalid_input(op, "edge use without face use"))?;
    Ok(edgeuse::faceuse_orientation(brep, fu)? == OrientType::Opposite)
}

/// `pEdgeuse->GetLoopuse()->GetFaceuse()->GetFace()->GetSurface()` as the base of an
/// offset surface (the offset shares the face's surface: its owner is kept).
fn face_base(brep: &Brep, eu: EdgeuseId, op: &'static str) -> Result<(Surface, SurfaceId)> {
    let f = edgeuse::face(brep, eu)?
        .ok_or_else(|| SolidError::invalid_input(op, "edge use without face"))?;
    let s = face::surface(brep, f)?
        .ok_or_else(|| SolidError::invalid_input(op, "face without surface"))?;
    Ok((s.clone_keeping_owner(), SurfaceId::Face(f)))
}

/// The edge-use part of the `SmConstantDistanceFS` and `SmVariableRadiusFS`
/// constructors: the radial, the edge and its interval, the binormals at the middle of
/// the edge and the face use normals (negated for opposite face uses).
struct EdgeuseFrame {
    radial: Option<EdgeuseId>,
    bin_vec: Vec3,
    bin_vec2: Vec3,
    faceuse_normal: Vec3,
    faceuse_normal2: Vec3,
}

fn edgeuse_frame(
    k: &Kernel,
    brep: &mut Brep,
    eu: EdgeuseId,
    op: &'static str,
) -> Result<EdgeuseFrame> {
    let radial = edgeuse::radial(brep, eu)?;
    let e = edgeuse::edge(brep, eu)?
        .ok_or_else(|| SolidError::invalid_input(op, "edge use without edge"))?;
    let interval = edge::interval(brep, e)?;
    let eu1 = radial.ok_or_else(|| SolidError::invalid_input(op, "edge use without radial"))?;
    let t = interval.evaluate(0.5);
    let (bin_vec, mut faceuse_normal) = binormal(k, brep, eu, t)?;
    let (bin_vec2, mut faceuse_normal2) = binormal(k, brep, eu1, t)?;
    if faceuse_opposite(brep, eu, op)? {
        faceuse_normal = -faceuse_normal;
    }
    if faceuse_opposite(brep, eu1, op)? {
        faceuse_normal2 = -faceuse_normal2;
    }
    Ok(EdgeuseFrame {
        radial,
        bin_vec,
        bin_vec2,
        faceuse_normal,
        faceuse_normal2,
    })
}

/// `new SmOffsetSurface(d, *pSurface)` with `SetAllowExtension(TRUE)`.
fn extending_offset(k: &Kernel, d: f64, base: (Surface, SurfaceId)) -> Result<OffsetSurface> {
    let (s, id) = base;
    let mut off = OffsetSurface::new(k, d, s, id, SS_UNKNOWN)?;
    off.set_allow_extension(true);
    Ok(off)
}

/// `m_pSurfaces[0..2]` of a solver as the constructors set them: two distinct
/// `SmOffsetSurface` objects (keys 0 and 1).
fn rail_slots(s0: OffsetSurface, s1: OffsetSurface) -> [Option<OffsetSurfaceSlot>; 2] {
    [
        Some(OffsetSurfaceSlot {
            key: 0,
            surface: s0,
        }),
        Some(OffsetSurfaceSlot {
            key: 1,
            surface: s1,
        }),
    ]
}

// ---- SmConstantRadiusFS::LoadInitialValues -------------------------------------------------

/// A constant-radius solver of some class (`SmConstantRadiusFS` and its subclasses):
/// its `SmConstantRadiusFS` data and the class's `SetupOffsetExtension` virtual.
pub trait ConstantRadiusOps {
    /// The `SmConstantRadiusFS` data.
    fn constant_radius(&self) -> &ConstantRadiusFs;

    /// The `SmConstantRadiusFS` data, mutable.
    fn constant_radius_mut(&mut self) -> &mut ConstantRadiusFs;

    /// The selected offset object for this context.
    fn offset_surface(&self, rail: usize) -> Result<&OffsetSurface> {
        self.constant_radius().surface(rail)
    }

    /// The same selected offset object for an in-place write.
    fn offset_surface_mut(&mut self, rail: usize) -> Result<&mut OffsetSurface> {
        self.constant_radius_mut().surface_mut(rail)
    }

    /// Reset the selected offset using the actual radius and orientation.
    fn set_offset_by_radius(&mut self, rail: usize) -> Result<()> {
        self.constant_radius_mut().set_offset_by_radius(rail)
    }

    /// The tangent-rollover guess of rail `rail`'s other surface: in the
    /// tangent-rollover case (`GetFilletGeomForSurfaces()` of type
    /// `SM_FG_TANGENT_ROLLOVER`) the UV position stored in a rail vertex use's
    /// intersection point (`None`: no such point, or no such geometry). The geometry
    /// for the surfaces is the solver's, set by the scope that lent its surfaces
    /// ([`FilletSolver::tangent_rollover_guess`]); context-free callers have none.
    ///
    /// # Errors
    /// `SM_ERR_NULL_POINTER` without the geometry's rail; [`SolidError::InvalidInput`]
    /// for a geometry for the surfaces set outside a lending scope.
    fn tangent_rollover_guess(&self, rail: usize) -> Result<Option<Point2>> {
        self.constant_radius().solver.tangent_rollover_guess(rail)
    }

    /// `SetupOffsetExtension(pSurf, rDomain)` of the solver's class.
    ///
    /// # Errors
    ///
    /// Those of the class's version.
    fn setup_offset_extension(
        &self,
        k: &Kernel,
        surface: &Surface,
        domain: &mut Extent2d,
    ) -> Result<()>;
}

impl ConstantRadiusOps for ConstantRadiusAssistedFs {
    fn constant_radius(&self) -> &ConstantRadiusFs {
        self
    }

    fn constant_radius_mut(&mut self) -> &mut ConstantRadiusFs {
        self
    }

    /// The base `SmFilletSolver::SetupOffsetExtension`.
    ///
    /// # Errors
    ///
    /// Those of [`FilletSolver::setup_offset_extension`].
    fn setup_offset_extension(
        &self,
        k: &Kernel,
        surface: &Surface,
        domain: &mut Extent2d,
    ) -> Result<()> {
        self.solver.setup_offset_extension(k, surface, domain)
    }
}

impl ConstantRadiusOps for SurfaceSurfaceFs {
    fn constant_radius(&self) -> &ConstantRadiusFs {
        &self.base
    }

    fn constant_radius_mut(&mut self) -> &mut ConstantRadiusFs {
        &mut self.base
    }

    /// `SmSurfaceSurfaceFS::SetupOffsetExtension`.
    fn setup_offset_extension(
        &self,
        k: &Kernel,
        surface: &Surface,
        domain: &mut Extent2d,
    ) -> Result<()> {
        SurfaceSurfaceFs::setup_offset_extension(self, k, surface, domain)
    }
}

/// `SmConstantRadiusFS::LoadInitialValues(rbDoSurf2Calcs, lRailIndex, crSrf1Offset,
/// lSurface1Index, rpSrf2Offset, rlSurface2Index, rIntervals, rPeriodicities, rGuessT)`
/// of a solver of class `S`. Without `do_surf2_calcs` nothing is touched (`None`).
/// Otherwise the point of `surface1` at the guess `rGuessT[surface1_index..+2]` is
/// carried to the other rail: through the other edge use's UV trim curve (when it lies
/// on that rail's base surface) or a global point solve on the base when that UV point
/// is near a parametric degeneracy, then refined by a local point solve on the other
/// offset surface inside its extended base domain. That domain becomes
/// `rIntervals[len(rGuessT)..+2]`, two `false` periodicities and the UV guess are
/// appended. The other base evaluates out of bounds meanwhile (restored on every exit).
///
/// # Errors
///
/// `SM_ERR` when a point solve finds no single solution; those of the evaluations,
/// solves and `SetupOffsetExtension`;
/// [`SolidError::InvalidInput`] for an out-of-range index (upstream reads or writes out
/// of bounds), a rail index above 1, a missing surface, or edge uses without `brep`.
#[allow(clippy::too_many_arguments)]
pub fn constant_radius_load_initial_values<S: ConstantRadiusOps + ?Sized>(
    solver: &mut S,
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
    const OP: &str = OP_CR_INIT;
    // No work - second surface info already calculated.
    if !do_surf2_calcs {
        return Ok(None);
    }

    // get Other OffsetSurface
    if rail_index > 1 {
        return Err(SolidError::invalid_input(OP, "rail index out of range"));
    }
    let other_rail = 1 - rail_index;

    // SmTemporaryChangeValue on the other base's out-of-bounds flag (surface1 is a
    // separate copy: when it has the same base, its copy changes too)
    let base2_id = solver.offset_surface(other_rail)?.base_id;
    let saved2 = enable_base_out_of_bounds(solver.offset_surface_mut(other_rail)?);
    let saved1 = if surface1.base_id == base2_id {
        enable_base_out_of_bounds(surface1)
    } else {
        None
    };
    let r = cr_initial_body(
        solver,
        k,
        brep,
        other_rail,
        surface1,
        surface1_index,
        intervals,
        periodicities,
        guess_t,
    );
    restore_base_out_of_bounds(surface1, saved1);
    restore_base_out_of_bounds(solver.offset_surface_mut(other_rail)?, saved2);
    r
}

/// [`constant_radius_load_initial_values`] inside the temporary change.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn cr_initial_body<S: ConstantRadiusOps + ?Sized>(
    solver: &S,
    k: &Kernel,
    mut brep: Option<&mut Brep>,
    other_rail: usize,
    surface1: &mut OffsetSurface,
    surface1_index: usize,
    intervals: &mut ExtentNd,
    periodicities: &mut Vec<bool>,
    guess_t: &mut Vec<f64>,
) -> Result<Option<InitialValues>> {
    const OP: &str = OP_CR_INIT;
    let fs = solver.constant_radius();
    let surface2 = solver.offset_surface(other_rail)?;
    let mut other_eu = fs.solver.edgeuses.get(other_rail).copied().flatten();
    let base2 = surface2.base_surface();
    let base2_id = surface2.base_id;

    // Get given 3d guess point
    let uv = Point2::new(
        at(guess_t, surface1_index, OP)?,
        at(guess_t, plus(surface1_index, 1, OP)?, OP)?,
    );
    let pnt = offset_evaluate_point(k, surface1, uv)?;

    let mut srf2_guess_uv = Point3::UNDEF;

    // In the tangent-rollover case, the proper uv guess points are set in the TsectPnts
    // of the FilletVertexuses.  They might be on the other side of a seam.
    let rollover_guess = solver.tangent_rollover_guess(1 - other_rail)?;
    if let Some(uv) = rollover_guess {
        srf2_guess_uv.x = uv.x;
        srf2_guess_uv.y = uv.y;
    }

    if rollover_guess.is_none() {
        let mut found_uv = false;

        // If we have edgeuses - use them.
        // If not - drop the GuessPoint to the other surface.

        // But first: if we are rolling on a different face (e.g., doing a rollover),
        // then this is the wrong Edgeuse, and we don't have an appropriate one to use.
        let need_brep = || SolidError::invalid_input(OP, "edge uses without their B-rep");
        if let Some(eu) = other_eu {
            let b = brep.as_deref().ok_or_else(need_brep)?;
            if let Some(f) = edgeuse::face(b, eu)? {
                let same = b.face(f)?.surface.is_some() && base2_id == SurfaceId::Face(f);
                if !same {
                    other_eu = None;
                }
            }
        }

        let trimmed_eu = match other_eu {
            Some(eu) => {
                let b = brep.as_deref().ok_or_else(need_brep)?;
                edgeuse::uv_trim_curve(b, eu)?.0.map(|_| (b, eu))
            }
            None => None,
        };
        if let Some((b, eu)) = trimmed_eu {
            // Drop pt to the filleted edge, in 3D
            let edge_id = edgeuse::edge(b, eu)?
                .ok_or_else(|| SolidError::invalid_input(OP, "edge use without edge"))?;
            let e = b.edge(edge_id)?;
            let curve = e
                .curve
                .as_ref()
                .ok_or_else(|| SolidError::invalid_input(OP, "edge without curve"))?;
            let ivl = e.interval;
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
                return Err(SolidError::sm_err(OP));
            }

            // get OtherFilletEdge Parameter
            let mut fillet_edge_t = sols
                .get(0)
                .ok_or_else(|| SolidError::sm_err(OP))?
                .start
                .param(0)?;

            // If the curve is closed, and the solution is within tol of an
            // end point, snap the solution to whichever end (start or finish)
            // such that the direction that moves towards the interior of the curve
            // also moves generally towards the interior of the surface.
            if curve.is_closed(k, &ivl, 0.0)? {
                let p3 = curve.evaluate_point(k, fillet_edge_t)?;
                let inward = offset_compute_inward_vector(k, surface1, uv)?;
                let snap_tol = 0.001 * curve.approximate_length(k, &ivl, 9, false)?.0;
                let dropped = curve.drop_point(
                    k,
                    &ivl,
                    p3,
                    Some(inward),
                    snap_tol,
                    None,
                    SolverOperation::Minimize,
                )?;
                if dropped.success {
                    fillet_edge_t = dropped.parameter;
                }
            }

            let (uv_curve, _) = edgeuse::uv_trim_curve(b, eu)?;
            let uv_curve = uv_curve
                .ok_or_else(|| SolidError::invalid_input(OP, "edge use without UV curve"))?;
            srf2_guess_uv = uv_curve.evaluate_point(k, fillet_edge_t)?;
            found_uv = true;
        }

        // If we found a uv, check that it's good enough.
        // Check how it looks on the other base surface.
        // If we are near a singularity then use global point solve.
        if found_uv {
            // Check parametric degeneracies:
            // (1) 1st derivs lining up,
            let uv_pnt = Point2::new(srf2_guess_uv.x, srf2_guess_uv.y);
            let flags = EvaluateFlags {
                u_side: Side::Left,
                v_side: Side::Left,
                only_upper_half: true,
                non_zero_tangents: true,
            };
            let mat = base2.evaluate(k, uv_pnt, 1, 1, flags)?;
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

            // (2) one deriv going to zero.
            if du.max_dimension() > 10000.0 * dv.max_dimension()
                || dv.max_dimension() > 10000.0 * du.max_dimension()
            {
                found_uv = false;
            }
        }

        // If we didn't get a good enough uv on surface2, try a global solve
        if !found_uv {
            let sols = surface_global_point_solve(
                k,
                base2,
                face_owner(brep.as_deref_mut(), base2_id),
                &base2.natural_uv_domain_with_nurb(k)?,
                pnt,
                EFF_ZERO_SQ,
            )?;
            if sols.len() != 1 {
                return Err(SolidError::sm_err(OP));
            }
            let s = sols
                .first()
                .copied()
                .ok_or_else(|| SolidError::sm_err(OP))?;
            srf2_guess_uv.x = s.x;
            srf2_guess_uv.y = s.y;
        }

        // Before we add the rGuessT values, drop point to other offset surface
        // to get a better guess
        let uv_guess = Point2::new(srf2_guess_uv.x, srf2_guess_uv.y);
        let mut domain = base2.natural_uv_domain_with_nurb(k)?;
        solver.setup_offset_extension(k, base2, &mut domain)?; // expands sDomain

        if let Some((sol_uv, dist)) = offset_local_point_solve(k, surface2, &domain, pnt, uv_guess)?
        {
            // when local solve worked: save solution
            let [r0, r1] = fs.fillet_radii;
            if dist < (r0.abs() + r1.abs()) {
                srf2_guess_uv.x = sol_uv.x;
                srf2_guess_uv.y = sol_uv.y;
            }
        } else {
            // use global solve
            let sols = surface_global_point_solve(
                k,
                base2,
                face_owner(brep, base2_id),
                &base2.natural_uv_domain_with_nurb(k)?,
                pnt,
                EFF_ZERO_SQ,
            )?;
            if sols.len() == 1 {
                let s = sols
                    .first()
                    .copied()
                    .ok_or_else(|| SolidError::sm_err(OP))?;
                srf2_guess_uv.x = s.x;
                srf2_guess_uv.y = s.y;
            }
        }
    } // end if not bFoundSrf2UVGuess

    // Start setting the return values:
    // the next two entries in rIntervals and rPeriodicities.
    let mut domain = base2.natural_uv_domain_with_nurb(k)?;
    solver.setup_offset_extension(k, base2, &mut domain)?; // expands sDomain

    let surface2_index = guess_t.len();
    *interval_at(intervals, surface2_index, OP)? =
        Extent1d::from_min_max(domain.min().x, domain.max().x);
    *interval_at(intervals, plus(surface2_index, 1, OP)?, OP)? =
        Extent1d::from_min_max(domain.min().y, domain.max().y);

    periodicities.push(false);
    periodicities.push(false);

    // set output
    guess_t.push(srf2_guess_uv.x);
    guess_t.push(srf2_guess_uv.y);

    Ok(Some(InitialValues {
        surface2_rail: other_rail,
        surface2_index,
    }))
}

impl ConstantRadiusAssistedFs {
    /// `SmConstantRadiusFS::LoadInitialValues` of a constant-radius (or assisted)
    /// solver: [`constant_radius_load_initial_values`] with the base
    /// `SetupOffsetExtension`.
    ///
    /// # Errors
    ///
    /// Those of [`constant_radius_load_initial_values`] (with
    /// [`FilletSolver::setup_offset_extension`]).
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
        constant_radius_load_initial_values(
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
}

// ---- SmGuessSphereSolveENFO -----------------------------------------------------------------

/// `SmGuessSphereSolveENFO` (file-local) as `GuessPointOnPlaneSolve` evaluates it: the
/// plane state of its `SmFilletSphereSolveENFO` base, set up as
/// `blend.fillet_standard_solver_p1`'s [`GuessSphereSolve::new`](crate::blend::fillet_standard_solver::p1::GuessSphereSolve::new)
/// does (the plane starts as the "sphere centre and radius", not adjusting), and
/// `m_crFilletSolver` as the `SmConstantRadiusFS` its `Evaluate` casts it to. (That
/// `GuessSphereSolve` holds the solver as a `dyn FilletSolverOps`, which cannot be cast
/// back for the qualified `SmConstantRadiusFS::LoadJacobian`.)
pub struct GuessSphereEnfo<'a> {
    /// `m_vPlaneABC`: plane normal.
    pub plane_abc: Vec3,
    /// `m_dPlaneD`: plane offset.
    pub plane_d: f64,
    /// `m_bPlaneAdjusts`.
    pub plane_adjusts: bool,
    kernel: &'a Kernel,
    solver: &'a mut ConstantRadiusFs,
}

impl<'a> GuessSphereEnfo<'a> {
    /// `SmGuessSphereSolveENFO(crSphereCenter, dSphereRadius, crFilletSolver)`.
    pub fn new(
        k: &'a Kernel,
        sphere_center: Point3,
        sphere_radius: f64,
        solver: &'a mut ConstantRadiusFs,
    ) -> Self {
        Self {
            plane_abc: sphere_center,
            plane_d: sphere_radius,
            plane_adjusts: false,
            kernel: k,
            solver,
        }
    }

    /// `SetPlaneAdjusts`.
    pub fn set_plane_adjusts(&mut self, plane_adjusts: bool) {
        self.plane_adjusts = plane_adjusts;
    }

    /// `SmGuessSphereSolveENFO::Evaluate(crX, rF, pOptJacobian, rbFoundAnswer)`: with a
    /// Jacobian, it and the matching `rF` rows are zeroed; `SmConstantRadiusFS::LoadJacobian`
    /// loads rows 1-3 (from parameter 2, rail 0, `crX[0..2]` on rail 0's surface); row 0
    /// is the plane distance `m_vPlaneABC . F + m_dPlaneD` of the first offset surface's
    /// point at `crX[0..2]` (with plane adjusting, the plane first moves to the line of
    /// the two surfaces' normals through the middle of their points). Returns
    /// `rbFoundAnswer`. Base extensions the evaluations make stay on the solver's
    /// offsets, as upstream's `const` evaluators store them.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when `LoadJacobian` fails; those of the evaluations and unitizations;
    /// [`SolidError::InvalidInput`] for arrays too short (upstream reads or writes past
    /// them) or a missing surface.
    pub fn evaluate(
        &mut self,
        x: &[f64],
        f: &mut [f64],
        mut jacobian: Option<&mut Matrix>,
    ) -> Result<bool> {
        const OP: &str = OP_GUESS;
        let k = self.kernel;
        let solver = &mut *self.solver;
        let mut found = false;

        if let Some(j) = jacobian.as_deref_mut() {
            for ll in 0..j.num_rows() {
                for mm in 0..j.num_columns() {
                    put_j(j, ll, mm, 0.0, OP)?;
                }
                put(f, ll, 0.0, OP)?;
            }
        }

        let mut num_equations = 1;
        let mut num_parameters = 2;
        let surface = Some(solver.surface(0)?.base_id);
        let mut offsets = [0, 0];
        let rail_index = 0;

        let found_answer = solver
            .load_jacobian(
                k,
                &mut num_equations,
                &mut num_parameters,
                rail_index,
                [surface, None],
                &mut offsets,
                x,
                f,
                jacobian.as_deref_mut(),
            )
            .map_err(|e| as_sm_err(e, OP))?;
        let surf2_offset = offsets[1];

        let uv = Point2::new(at(x, 0, OP)?, at(x, 1, OP)?);
        let (sf, sduf, sdvf) = offset_evaluate_1st(k, solver.surface_mut(0)?, uv)?;

        // Let's try using the plane instead of the sphere equation
        // f0 = A*x + B*y + C*z + D
        // || A*du.x + B*du.y + C*du.z    A*dv.x + B*dv.y + C*dv.z
        if self.plane_adjusts {
            let mut norm1 = sduf.cross(sdvf);
            if norm1.length_squared() < EFF_ZERO_SQ {
                norm1 = offset_evaluate_normal(k, solver.surface_mut(0)?, uv)?;
            }
            let norm1 = norm1.unitize()?;

            let uv2 = Point2::new(
                at(x, surf2_offset, OP)?,
                at(x, plus(surf2_offset, 1, OP)?, OP)?,
            );
            let (sg, sdug, sdvg) = offset_evaluate_1st(k, solver.surface_mut(1)?, uv2)?;

            let mut norm2 = sdug.cross(sdvg);
            if norm2.length_squared() < EFF_ZERO_SQ {
                norm2 = offset_evaluate_normal(k, solver.surface_mut(1)?, uv2)?;
            }
            let norm2 = norm2.unitize()?;

            let plane_orig = (sf + sg) / 2.0;
            let mut plane_abc = norm1.cross(norm2);
            if plane_abc.dot(self.plane_abc) < 0.0 {
                plane_abc = -plane_abc;
            }
            self.plane_abc = plane_abc;
            self.plane_d = -self.plane_abc.dot(plane_orig);
        }

        put(f, 0, self.plane_abc.dot(sf) + self.plane_d, OP)?;

        if let Some(j) = jacobian {
            put_j(j, 0, 0, self.plane_abc.dot(sduf), OP)?;
            put_j(j, 0, 1, self.plane_abc.dot(sdvf), OP)?;
        }

        // Jacobian answer looks good now check our answer to see if we return
        // TRUE for rbFoundAnswer
        if found_answer {
            let scaled_tol = solver.solver.conversion_tol * (1.0 + sf.max_dimension());
            if at(f, 0, OP)?.abs() < scaled_tol {
                found = true;
            }
        }

        Ok(found)
    }
}

impl EvalNFunctions for GuessSphereEnfo<'_> {
    fn evaluate(
        &mut self,
        x: &[f64],
        f: &mut [f64],
        jacobian: Option<&mut Matrix>,
    ) -> Result<bool> {
        GuessSphereEnfo::evaluate(self, x, f, jacobian)
    }
}

/// `SmLocalSolveNd(sEvalFun, &sIntervals, &sPeriodicities)` set up as both plane
/// solves do: desired accuracy `SM_EFF_ZERO_SQ`, at most 8 total boundary hits, 30
/// iterations.
fn plane_local_solver<'a, E: EvalNFunctions + ?Sized>(
    eval: &'a mut E,
    intervals: &'a ExtentNd,
    periodicities: &'a [bool],
) -> LocalSolveNd<'a, E> {
    let mut ls = LocalSolveNd::new(eval, Some(intervals), Some(periodicities));
    ls.set_desired_accuracy(EFF_ZERO_SQ);
    ls.set_boundary_handler(BoundaryHandler::TotalBoundaryHits, 8);
    ls.set_maximum_iterations(30);
    ls
}

/// The solver intervals of both plane solves: `GetJacobianSize()` intervals, the first
/// four the two UV domains.
fn plane_intervals(size: usize, d1: &Extent2d, d2: &Extent2d) -> Result<ExtentNd> {
    let mut ivls = ExtentNd::new(size);
    let values = [
        Extent1d::from_min_max(d1.min().x, d1.max().x),
        Extent1d::from_min_max(d1.min().y, d1.max().y),
        Extent1d::from_min_max(d2.min().x, d2.max().x),
        Extent1d::from_min_max(d2.min().y, d2.max().y),
    ];
    for (i, v) in values.into_iter().enumerate() {
        *interval_at(&mut ivls, i, OP)? = v;
    }
    Ok(ivls)
}

/// The UV points (and user doubles) of a found plane solution.
fn store_plane_solution(
    t: &mut TsectPnt,
    sol: &[f64],
    surf1_index: usize,
    surf2_index: usize,
    op: &'static str,
) -> Result<()> {
    *t.uv_pos_mut(SrfIndex::First) = Point2::new(
        at(sol, surf1_index, op)?,
        at(sol, plus(surf1_index, 1, op)?, op)?,
    );
    *t.uv_pos_mut(SrfIndex::Second) = Point2::new(
        at(sol, surf2_index, op)?,
        at(sol, plus(surf2_index, 1, op)?, op)?,
    );
    for i in 4..sol.len() {
        let slot = t
            .user_doubles
            .get_mut(i - 4)
            .ok_or_else(|| SolidError::invalid_input(op, "m_adUserDoubles index out of range"))?;
        *slot = at(sol, i, op)?;
    }
    Ok(())
}

/// `SmConstantRadiusAssistedFS::IsKindOf`: `SmConstantRadiusAssistedFS_TYPE` or a type
/// a constant-radius solver is.
#[must_use]
pub const fn constant_radius_assisted_is_kind_of(t: i64) -> bool {
    t == CONSTANT_RADIUS_ASSISTED_FS_TYPE || constant_radius_is_kind_of(t)
}

impl ConstantRadiusAssistedFs {
    /// `SmConstantRadiusAssistedFS::GuessPointOnPlaneSolve(crPlaneOrig, crPlaneNormal,
    /// crUVDomain1, crUVDomain2, rUV1, rUV2, rbFoundSolution, rTsectPnt)`: solves the
    /// constant-radius equations plus the plane `crPlaneNormal . P = crPlaneNormal .
    /// crPlaneOrig` for the first offset surface's point from `(rUV1, rUV2)` inside the
    /// two domains, then again with the plane adjusting to the solution when that
    /// fails. Returns `rbFoundSolution`; when found, the UV points (and user doubles) of
    /// `point` (`rTsectPnt`) are set, the rest of it is kept. `GetJacobianSize` is
    /// `SmConstantRadiusFS`'s, which every solver derived from this class inherits. The
    /// solver's offsets keep the base extensions the evaluations make, as upstream's
    /// `const` evaluators store them on `*this`.
    ///
    /// # Errors
    ///
    /// Those of the local solves ([`SolidError::Unported`] from an evaluation);
    /// [`SolidError::InvalidInput`] for a missing surface.
    #[allow(clippy::too_many_arguments)]
    pub fn guess_point_on_plane_solve(
        &mut self,
        k: &Kernel,
        plane_orig: Point3,
        plane_normal: Vec3,
        uv_domain1: &Extent2d,
        uv_domain2: &Extent2d,
        uv1: Point2,
        uv2: Point2,
        point: &mut TsectPnt,
    ) -> Result<bool> {
        // Now Set up Solver
        let jacobian_size = ConstantRadiusAssistedFs::jacobian_size(self);
        let intervals = plane_intervals(jacobian_size, uv_domain1, uv_domain2)?;
        let periodicities = vec![false; 4];
        let guess_t = vec![uv1.x, uv1.y, uv2.x, uv2.y];

        let surf1_index = 0; // UV values of Surface1
        let surf2_index = 2; // UV values of Surface2

        let plane_d = -plane_orig.dot(plane_normal);
        let approx_tol_3d = self.solver.approx_tol_3d;
        let mut eval = GuessSphereEnfo::new(k, plane_normal, plane_d, self);
        let mut ls = plane_local_solver(&mut eval, &intervals, &periodicities);

        // Solve it
        let mut sol = ls.solve_it(&guess_t, approx_tol_3d)?;

        // Try it again and allow the plane to adjust as convergance happens
        if !sol.found {
            ls.evaluator().set_plane_adjusts(true);
            sol = ls.solve_it(&guess_t, approx_tol_3d)?;
        }

        if sol.found {
            store_plane_solution(point, &sol.x, surf1_index, surf2_index, OP_GUESS)?;
        }
        Ok(sol.found)
    }
}

/// `SmConstantRadiusAssistedFS::PointOnPlaneSolve(crPlaneOrig, crPlaneNormal,
/// crUVDomain1, crUVDomain2, rUV1, rUV2, rbFoundSolution, rTsectPnt)` on `*this` =
/// `solver`, an `SmConstantRadiusAssistedFS` or a class derived from it that inherits
/// the method (`SmEiffelTowerFS`, `SmSecantConstantRadiusFS`): its virtual
/// `GetJacobianSize`, `LoadInitialValues` and (through the `SmFilletSphereSolveENFO`)
/// `LoadJacobian` dispatch on the solver's class. Loads the initial values from `rUV1`
/// on rail 0 (`rUV2` is not used), solves the solver's equations on the plane through
/// `crPlaneOrig` normal to `crPlaneNormal` (again with the plane adjusting when that
/// fails) and, when found, stores both UV points, the surface points, derivatives and
/// unit normals and the first surface point as the curve point in `point`
/// (`rTsectPnt`; the rest of it is kept). Returns `rbFoundSolution`.
///
/// `crSurface1` of `LoadInitialValues` is `*GetSurface(0)`: it runs on a copy (the
/// solver is borrowed mutably meanwhile) whose base extension is handed back to the
/// solver's rail 0 offset afterwards, as upstream evaluates the solver's own offset.
/// `brep` is the B-rep of the solver's edge uses (`None` without).
///
/// # Errors
///
/// Those of the solver's `LoadInitialValues` and of the plane solves
/// (`SmFilletSphereSolveENFO::Evaluate`, unit `blend.fillet_solver_p1`); `SM_ERR` for a
/// zero normal at the solution; [`SolidError::InvalidInput`] for a missing surface.
#[allow(clippy::too_many_arguments)]
pub fn point_on_plane_solve(
    solver: &mut dyn FilletSolverOps,
    k: &Kernel,
    brep: Option<&mut Brep>,
    plane_orig: Point3,
    plane_normal: Vec3,
    uv_domain1: &Extent2d,
    uv_domain2: &Extent2d,
    uv1: Point2,
    _uv2: Point2,
    point: &mut TsectPnt,
) -> Result<bool> {
    const OP: &str = OP_PLANE;
    // Now Set up Solver
    let jacobian_size = solver.jacobian_size()?;
    let mut intervals = plane_intervals(jacobian_size, uv_domain1, uv_domain2)?;
    let mut periodicities = vec![false; 2];
    let mut guess_t = vec![uv1.x, uv1.y];

    let mut surface1 = rail_offset(solver.solver(), 0)?.clone();
    let domain_before = surface1.surface.natural_uv_domain_with_nurb(k)?;
    let surf1_index = 0; // UV values of Surface1
    let rail_index = 0;

    // Load the initial values
    // given current inputs - place a good guess for upcoming solve in sGuessT
    let init = solver.load_initial_values(
        k,
        brep,
        true,
        rail_index,
        &mut surface1,
        surf1_index,
        &mut intervals,
        &mut periodicities,
        &mut guess_t,
    );
    // (upstream's crSurface1 is the solver's own offset: it keeps the extension the
    // evaluations of crSurface1 made, unless the solver's own offset was extended
    // later in the call, as by the custom solvers' GuessPointOnPlaneSolve)
    let own = &mut rail_offset_mut(solver.solver_mut(), 0)?.surface;
    if own.natural_uv_domain_with_nurb(k)? == domain_before {
        own.take_extension_of(&surface1.surface);
    }
    let init = init?;
    let surf2_index = init.map_or(0, |v| v.surface2_index);

    // get plane equation for given PlanePt and PlaneNormal
    let plane_d = -plane_orig.dot(plane_normal);

    // Solve equations with NewtonRaphson iteration (and again with the plane
    // adjusting)
    let sol = fillet_sphere_solve(
        k,
        solver,
        plane_normal,
        plane_d,
        &intervals,
        &periodicities,
        &guess_t,
    )?;

    // when a solution point was found
    if !sol.found {
        return Ok(false);
    }
    store_plane_solution(point, &sol.x, surf1_index, surf2_index, OP)?;

    // for both surfaces - get and store solution point surface values
    for (rail, srf) in [(0, SrfIndex::First), (1, SrfIndex::Second)] {
        // get and store the surface point values
        let uv = point.uv_pos(srf);
        let s = rail_offset_mut(solver.solver_mut(), rail)?;
        let (p, du, dv) = offset_evaluate_1st(k, s, uv)?;
        *point.srf_pos_mut(srf) = p;
        *point.srf_du_mut(srf) = du;
        *point.srf_dv_mut(srf) = dv;

        // Store a unitized surface normal.
        let mut norm = du.cross(dv);
        if norm.length_squared() < EFF_ZERO_SQ {
            norm = offset_evaluate_normal(k, rail_offset_mut(solver.solver_mut(), rail)?, uv)?;
            if norm.length_squared() < EFF_ZERO_SQ {
                return Err(SolidError::sm_err(OP));
            }
        }
        // (the Unitize status is ignored: the normal is not zero here)
        let norm = norm.unitize_or_z();
        // SrfNorm(lSrf) is the [2][2] entry of the evaluation matrix
        // (SmSurfaceIntersector.h:224)
        point.srf_eval_matrix_mut(srf)[2][2] = norm;
    }

    // Now compute the point and vectors for the 3D curve
    // Point is average of surface points
    *point.crv_pos_mut() = point.srf_pos(SrfIndex::First);
    Ok(true)
}

/// `GetSurface(rail)` of the solver base.
fn rail_offset(s: &FilletSolver, rail: usize) -> Result<&OffsetSurface> {
    s.surface(rail)?
        .map(|slot| &slot.surface)
        .ok_or_else(|| SolidError::invalid_input(OP_PLANE, "no offset surface"))
}

/// `GetSurface(rail)` of the solver base, for evaluation.
fn rail_offset_mut(s: &mut FilletSolver, rail: usize) -> Result<&mut OffsetSurface> {
    s.surface_mut(rail)?
        .map(|slot| &mut slot.surface)
        .ok_or_else(|| SolidError::invalid_input(OP_PLANE, "no offset surface"))
}

/// The `SmFilletSphereSolveENFO` as the equation set of an `SmLocalSolveNd`.
struct SphereEquations<'a>(&'a Kernel, FilletSphereSolveEnfo<'a>);

impl EvalNFunctions for SphereEquations<'_> {
    fn evaluate(
        &mut self,
        x: &[f64],
        f: &mut [f64],
        jacobian: Option<&mut Matrix>,
    ) -> Result<bool> {
        self.1.evaluate(self.0, x, f, jacobian)
    }
}

/// `SmFilletSphereSolveENFO sEvalFun(crPlaneNormal, dPlaneD, *this)` solved by the
/// plane local solver at `m_dThisApproxTol3d`, again with `SetPlaneAdjusts(TRUE)` when
/// not found.
///
/// # Errors
///
/// Those of the local solves (`SmFilletSphereSolveENFO::Evaluate`, unit
/// `blend.fillet_solver_p1`).
fn fillet_sphere_solve(
    k: &Kernel,
    solver: &mut dyn FilletSolverOps,
    plane_normal: Vec3,
    plane_d: f64,
    intervals: &ExtentNd,
    periodicities: &[bool],
    guess_t: &[f64],
) -> Result<NdSolution> {
    let approx_tol_3d = solver.solver().approx_tol_3d;
    // (the constructor's "sphere centre and radius" are the plane normal and D)
    let mut eval = SphereEquations(k, FilletSphereSolveEnfo::new(plane_normal, plane_d, solver));
    let mut ls = plane_local_solver(&mut eval, intervals, periodicities);

    // Solve equations with NewtonRaphson iteration
    let mut sol = ls.solve_it(guess_t, approx_tol_3d)?;

    // Try it again and allow the plane to adjust as convergance happens
    if !sol.found {
        ls.evaluator().1.set_plane_adjusts(true);
        sol = ls.solve_it(guess_t, approx_tol_3d)?;
    }
    Ok(sol)
}

// ---- SmConstantDistanceFS constructor -------------------------------------------------------

impl ConstantDistanceFs {
    /// `SmConstantDistanceFS(crContext, dThisApproxTol3d, dAngleTolerance,
    /// dTangencyTolerance, dDistance, pEdgeuse)`: the edge use and its radial, offset
    /// orientations from the binormals at the middle of the edge, and offset surfaces of
    /// both faces by `dDistance * sqrt(2) / 2` (extension allowed). For faces tangent at
    /// the middle of the edge upstream signals `SM_ERR` and returns a solver without edge
    /// uses or surfaces (orientations as far as computed): so does this. The
    /// constructor's fillet geometry is stored in `pseudo` for solver `id` and, unless
    /// the faces are tangent, receives copies of the offsets (`FinishConstruction`).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an edge use without radial, edge, face use or
    /// face surface (upstream dereferences `NULL`); those of [`OffsetSurface::new`],
    /// [`FilletSolver::new_in`] and [`FilletSolver::finish_construction`];
    /// [`SolidError::Unported`] from `SmEdgeuse::EvaluateBinormal`.
    #[allow(clippy::too_many_arguments)]
    pub fn from_edgeuse(
        k: &Kernel,
        brep: &mut Brep,
        pseudo: &mut FilletBrep,
        id: FilletSolverId,
        approx_tol_3d: f64,
        ang_tol_rad: f64,
        tangency_tolerance: f64,
        distance: f64,
        edgeuse: EdgeuseId,
    ) -> Result<Self> {
        const OP: &str = OP_CD_CTOR;
        let mut solver = FilletSolver::new_in(
            k,
            pseudo,
            id,
            approx_tol_3d,
            ang_tol_rad,
            tangency_tolerance,
        )?;
        solver.edgeuses = [Some(edgeuse), edgeuse::radial(brep, edgeuse)?];

        // Create offset surfaces of appropriate radius in appropriate
        // direction.  For now assume that the orientations at the center of the
        // edge work for the rest of the edge.
        if k.fixes.fillet_knife_edge_rounds {
            // (fork b7ec55b) offset each surface to the side of it the other face lies
            // on (sm_SectorSides), a cusp's faces too
            let mut cd = Self { solver, distance };
            let radial = cd.solver.edgeuses[1];
            let Some(sides) =
                super::p1::sector_sides(k, brep, [Some(edgeuse), radial], cd.distance, OP)?
            else {
                // Smooth, or no sector: can't fillet this.
                cd.solver.edgeuses = [None, None];
                // (SE(SM_ERR): only signalled)
                return Ok(cd);
            };
            cd.solver.orientations = sides;
            let eu1 =
                radial.ok_or_else(|| SolidError::invalid_input(OP, "edge use without radial"))?;
            return cd.finish_offsets(k, brep, pseudo, edgeuse, eu1, OP);
        }
        let fr = edgeuse_frame(k, brep, edgeuse, OP)?;
        let mut cd = Self { solver, distance };

        cd.solver.orientations = [1.0, 1.0];
        let dot1 = fr.bin_vec2.dot(fr.faceuse_normal);
        if dot1.abs() < EFF_ZERO_SQRT {
            cd.solver.edgeuses = [None, None];
            // (SE(SM_ERR): only signalled)
            return Ok(cd);
        }
        if dot1 < 0.0 {
            cd.solver.orientations[0] = -1.0;
        }
        let dot2 = fr.bin_vec.dot(fr.faceuse_normal2);
        if dot2.abs() < EFF_ZERO_SQRT {
            cd.solver.edgeuses = [None, None];
            // (SE(SM_ERR): only signalled)
            return Ok(cd);
        }
        if dot2 < 0.0 {
            cd.solver.orientations[1] = -1.0;
        }

        let eu1 = fr
            .radial
            .ok_or_else(|| SolidError::invalid_input(OP, "edge use without radial"))?;
        cd.finish_offsets(k, brep, pseudo, edgeuse, eu1, OP)
    }

    /// The tail of [`ConstantDistanceFs::from_edgeuse`] once the orientations are set:
    /// offset surfaces of both faces by `dDistance * sqrt(2) / 2` (extension allowed),
    /// then `FinishConstruction`.
    fn finish_offsets(
        mut self,
        k: &Kernel,
        brep: &Brep,
        pseudo: &mut FilletBrep,
        edgeuse: EdgeuseId,
        eu1: EdgeuseId,
        op: &'static str,
    ) -> Result<Self> {
        let s0 = face_base(brep, edgeuse, op)?;
        let s1 = face_base(brep, eu1, op)?;
        let approx_radius = self.distance * 2.0_f64.sqrt() / 2.0;
        let [o0, o1] = self.solver.orientations;
        self.solver.surfaces = rail_slots(
            extending_offset(k, approx_radius * o0, s0)?,
            extending_offset(k, approx_radius * o1, s1)?,
        );

        self.solver.finish_construction(k, pseudo)?; // Take care of other things in the base class.
        Ok(self)
    }
}

// ---- SmVariableRadiusFS -----------------------------------------------------------------------

/// `if (m_pOriginal->GetDegree() > 1)`: `SE(m_pOriginal->Length(m_vLawInterval,
/// dThisApproxTol3d * 10, dEdgeArcLength))` (a failure leaves `0.0`) and
/// `rFilletLaw.SetEdgeArcLength(dEdgeArcLength)`.
fn set_law_arc_length(
    k: &Kernel,
    curve: &Curve,
    interval: &Extent1d,
    approx_tol_3d: f64,
    law: &mut FilletLaw,
) -> Result<()> {
    if curve.degree()? > 1 {
        let accuracy = approx_tol_3d * 10.0;
        let edge_arc_length = ignored(curve.length(k, interval, accuracy))?.unwrap_or(0.0);
        law.edge_arc_length = edge_arc_length;
    }
    Ok(())
}

impl VariableRadiusFs {
    /// `SmVariableRadiusFS(crContext, dThisApproxTol3d, dAngleTolerance,
    /// dTangencyTolerance, dDistance, pEdgeuse, rFilletLaw, bLawOrientation)`: the edge
    /// use and its radial, the edge curve as original curve and its interval as law
    /// interval (with the law's edge arc length for a curved edge), the offset
    /// orientations from the binormals at the middle of the edge and offset surfaces of
    /// both faces by the law's radius at the middle of the interval (extension
    /// allowed). For faces tangent at the middle of the edge upstream returns a solver
    /// without edge uses or surfaces (signalling `SM_ERR` only for the second test): so
    /// does this. `dDistance` is not used. The constructor's fillet geometry is stored
    /// in `pseudo` for solver `id` and, unless the faces are tangent, receives copies of
    /// the offsets (`FinishConstruction`).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an edge use without radial, edge, curve, face
    /// use or face surface (upstream dereferences `NULL`); [`SolidError::Unported`] from
    /// `SmEdgeuse::EvaluateBinormal`; those of the law evaluation, [`OffsetSurface::new`],
    /// [`CurveBasedFs::new_in`] and [`FilletSolver::finish_construction`].
    #[allow(clippy::too_many_arguments)]
    pub fn from_edgeuse(
        k: &Kernel,
        brep: &mut Brep,
        pseudo: &mut FilletBrep,
        id: FilletSolverId,
        approx_tol_3d: f64,
        ang_tol_rad: f64,
        tangency_tolerance: f64,
        _distance: f64,
        edgeuse: EdgeuseId,
        mut law: FilletLaw,
        law_orientation: bool,
    ) -> Result<Self> {
        const OP: &str = OP_VR_CTOR;
        let mut curve_based = CurveBasedFs::new_in(
            k,
            pseudo,
            id,
            approx_tol_3d,
            ang_tol_rad,
            tangency_tolerance,
        )?;
        curve_based.solver.edgeuses = [Some(edgeuse), edgeuse::radial(brep, edgeuse)?];
        let e = edgeuse::edge(brep, edgeuse)?
            .ok_or_else(|| SolidError::invalid_input(OP, "edge use without edge"))?;
        let law_interval = edge::interval(brep, e)?;
        let original = brep
            .edge(e)?
            .curve
            .clone()
            .ok_or_else(|| SolidError::invalid_input(OP, "edge without curve"))?;
        set_law_arc_length(k, &original, &law_interval, approx_tol_3d, &mut law)?;
        curve_based.original = Some(original);

        if k.fixes.fillet_knife_edge_rounds {
            // (fork b7ec55b) the law's radius at the middle first, then each surface
            // offset to the side of it the other face lies on (sm_SectorSides), a cusp's
            // faces too
            let mut vr = Self {
                curve_based,
                law,
                law_orientation,
                law_interval,
            };
            // (SE: a failed law evaluation leaves the radius undefined; only Unported
            // propagates)
            let values = ignored(vr.law.evaluate(
                vr.law_interval.evaluate(0.5),
                &vr.law_interval,
                vr.law_orientation,
            ))?
            .ok_or_else(|| SolidError::invalid_input(OP, "undefined fillet law radius"))?;
            let [approx_radius, _, _] = values;
            let radial = vr.curve_based.solver.edgeuses[1];
            let Some(sides) =
                super::p1::sector_sides(k, brep, [Some(edgeuse), radial], approx_radius.abs(), OP)?
            else {
                // Smooth, or no sector: can't fillet this.
                vr.curve_based.solver.edgeuses = [None, None];
                // (SE(SM_ERR): only signalled)
                return Ok(vr);
            };
            vr.curve_based.solver.orientations = sides;
            let eu1 =
                radial.ok_or_else(|| SolidError::invalid_input(OP, "edge use without radial"))?;
            return vr.finish_offsets(k, brep, pseudo, edgeuse, eu1, approx_radius, OP);
        }
        let fr = edgeuse_frame(k, brep, edgeuse, OP)?;
        let mut vr = Self {
            curve_based,
            law,
            law_orientation,
            law_interval,
        };

        vr.curve_based.solver.orientations = [1.0, 1.0];
        let dot1 = fr.bin_vec2.dot(fr.faceuse_normal);
        if dot1.abs() < EFF_ZERO_SQRT {
            vr.curve_based.solver.edgeuses = [None, None];
            return Ok(vr);
        }
        if dot1 < 0.0 {
            vr.curve_based.solver.orientations[0] = -1.0;
        }
        let dot2 = fr.bin_vec.dot(fr.faceuse_normal2);
        if dot2.abs() < EFF_ZERO_SQRT {
            vr.curve_based.solver.edgeuses = [None, None];
            // (SE(SM_ERR): only signalled)
            return Ok(vr);
        }
        if dot2 < 0.0 {
            vr.curve_based.solver.orientations[1] = -1.0;
        }

        // (SE: a failed law evaluation leaves the radius undefined; only Unported
        // propagates)
        let values = ignored(vr.law.evaluate(
            vr.law_interval.evaluate(0.5),
            &vr.law_interval,
            vr.law_orientation,
        ))?
        .ok_or_else(|| SolidError::invalid_input(OP, "undefined fillet law radius"))?;
        let [approx_radius, _, _] = values;
        let eu1 = fr
            .radial
            .ok_or_else(|| SolidError::invalid_input(OP, "edge use without radial"))?;
        vr.finish_offsets(k, brep, pseudo, edgeuse, eu1, approx_radius, OP)
    }

    /// The tail of [`VariableRadiusFs::from_edgeuse`] once the orientations and the
    /// law's radius at the middle are known: offset surfaces of both faces by that radius
    /// (extension allowed), then `FinishConstruction`.
    #[allow(clippy::too_many_arguments)]
    fn finish_offsets(
        mut self,
        k: &Kernel,
        brep: &Brep,
        pseudo: &mut FilletBrep,
        edgeuse: EdgeuseId,
        eu1: EdgeuseId,
        approx_radius: f64,
        op: &'static str,
    ) -> Result<Self> {
        let s0 = face_base(brep, edgeuse, op)?;
        let s1 = face_base(brep, eu1, op)?;
        let [o0, o1] = self.curve_based.solver.orientations;
        self.curve_based.solver.surfaces = rail_slots(
            extending_offset(k, approx_radius * o0, s0)?,
            extending_offset(k, approx_radius * o1, s1)?,
        );

        self.curve_based.solver.finish_construction(k, pseudo)?; // Take care of other things in the base class.
        Ok(self)
    }

    /// `SmVariableRadiusFS(crContext, dThisApproxTol3d, dAngleTolerance,
    /// dTangencyTolerance, pSurface, pSurface2, dOffsetOrientation, dOffsetOrientation2,
    /// pCenterCurve, rFilletLaw, bLawOrientation)`: no edge uses, the centre curve as
    /// original curve and its natural interval as law interval (with the law's edge arc
    /// length for a curved centre), the given orientations and offset surfaces of both
    /// surfaces by the law's radius at the middle of the interval (extension allowed).
    /// The constructor's fillet geometry is stored in `pseudo` for solver `id`; this
    /// constructor does not call `FinishConstruction` upstream, so that geometry keeps
    /// no offsets.
    ///
    /// # Errors
    ///
    /// Those of the law evaluation, [`OffsetSurface::new`] and [`CurveBasedFs::new_in`].
    #[allow(clippy::too_many_arguments)]
    pub fn from_surfaces(
        k: &Kernel,
        pseudo: &mut FilletBrep,
        id: FilletSolverId,
        approx_tol_3d: f64,
        ang_tol_rad: f64,
        tangency_tolerance: f64,
        surfaces: [(Surface, SurfaceId); 2],
        orientations: [f64; 2],
        center_curve: Curve,
        mut law: FilletLaw,
        law_orientation: bool,
    ) -> Result<Self> {
        const OP: &str = OP_VR_CTOR;
        let mut curve_based = CurveBasedFs::new_in(
            k,
            pseudo,
            id,
            approx_tol_3d,
            ang_tol_rad,
            tangency_tolerance,
        )?;
        curve_based.solver.edgeuses = [None, None];
        let law_interval = center_curve.natural_interval();
        set_law_arc_length(k, &center_curve, &law_interval, approx_tol_3d, &mut law)?;
        curve_based.original = Some(center_curve);
        curve_based.solver.orientations = orientations;

        // (SE: a failed law evaluation leaves the radius undefined; only Unported
        // propagates)
        let values =
            ignored(law.evaluate(law_interval.evaluate(0.5), &law_interval, law_orientation))?
                .ok_or_else(|| SolidError::invalid_input(OP, "undefined fillet law radius"))?;
        let [approx_radius, _, _] = values;
        let [o0, o1] = orientations;
        let [s0, s1] = surfaces;
        curve_based.solver.surfaces = rail_slots(
            extending_offset(k, approx_radius * o0, s0)?,
            extending_offset(k, approx_radius * o1, s1)?,
        );
        // (no FinishConstruction() here upstream: the fillet geometry keeps no offsets)
        Ok(Self {
            curve_based,
            law,
            law_orientation,
            law_interval,
        })
    }

    /// `SmVariableRadiusFS::IsKindOf`: its own type or a type a curve-based solver is.
    #[must_use]
    pub const fn is_kind_of(&self, t: i64) -> bool {
        t == Self::TYPE || t == CURVE_BASED_FS_TYPE || FilletSolver::is_kind_of(t)
    }

    /// `SmVariableRadiusFS::ReCalcFilletGeom(bIsAnalyticFillet, pOptMarchDir,
    /// pOptRefCurve)`: clears the extension flags; collects the intersection points of
    /// the rail ends of the solver's first fillet geometry (in `pseudo`, the pseudo
    /// B-rep the geometry lives in), skipping unprocessed, mate and
    /// rail-x-extended-edge-use vertices, sorted by their projection on the reference
    /// curve (points within `SM_EFF_ZERO_SQRT` of the curve's length of one already
    /// taken are dropped); then traces the law intersection of the two offsets over
    /// their extended domains through those points (`SmFilletIntersector::
    /// DoLawIntersection`), which updates the points in place, and fails when a point
    /// after the first has a negative curve parameter. `bIsAnalyticFillet` and
    /// `pOptMarchDir` are unused upstream.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` without reference curve, rail or original curve; `SM_ERR`
    /// for a rail without exactly two edge uses, a projection without exactly one
    /// solution or a point with a negative curve parameter; those of the projection,
    /// `SetupOffsetExtension` and the law intersection (its overrides through this
    /// solver);
    /// [`SolidError::InvalidInput`] for a solver without fillet geometry or surfaces, or
    /// a rail edge use without vertex use or fillet vertex (upstream reads out of
    /// bounds or dereferences `NULL`).
    #[allow(clippy::too_many_lines)]
    pub fn re_calc_fillet_geom(
        &mut self,
        k: &Kernel,
        pseudo: &mut FilletBrep,
        _is_analytic_fillet: bool,
        _march_dir: Option<Vec3>,
        ref_curve: Option<&Curve>,
    ) -> Result<()> {
        const OP: &str = OP_VR_RECALC;
        self.curve_based.solver.extend_before = false;
        self.curve_based.solver.extend_after = false;

        // pCurve = pOptRefCurve; NER(pCurve); (m_vFilletGeoms[0] is read before, but
        // first dereferenced in the rail loop)
        let curve = ref_curve.ok_or_else(|| SolidError::null_pointer(OP))?;
        let fillet_geom = *self
            .curve_based
            .solver
            .fillet_geoms
            .first()
            .ok_or_else(|| SolidError::invalid_input(OP, "solver without fillet geometry"))?;
        // (b3DTest is TRUE: the 2D test on the edge uses' UV trim curves is never taken)
        let b3d_test = true;

        let ivl = curve.natural_interval();
        // (the closed solver edge test is disabled upstream: `#if 0`)
        let is_closed_solver_edge = false;

        let tol = self.curve_based.solver.approx_tol_3d;
        let scale = ivl.length();

        // Sort the SmTsectPnt's of each rail-end, the rails should
        // go in the same 'direction' as the solver edge
        // (sTsectPnts holds pointers to the vertex uses' points: their ids here)
        let mut tsect_pnts: Vec<VertexuseId> = Vec::new();
        let mut proj_params: Vec<f64> = Vec::new();
        for i in 0..2 {
            // (m_vOtherRails1 / m_vOtherRails2 are not appended upstream)
            let all_rails = [pseudo.geom(fillet_geom)?.rail(i)];
            for rail in all_rails {
                let rail = rail.ok_or_else(|| SolidError::null_pointer(OP))?;
                let fil_eus = edge::edgeuses(pseudo.brep(), rail, None, None)?;
                if fil_eus.len() != 2 {
                    return Err(SolidError::sm_err(OP));
                }
                for &eu in &fil_eus {
                    let vu = pseudo.brep().edgeuse(eu)?.vertexuse.ok_or_else(|| {
                        SolidError::invalid_input(OP, "rail edge use without vertex use")
                    })?;
                    let v = vertexuse::vertex(pseudo.brep(), vu)?.ok_or_else(|| {
                        SolidError::invalid_input(OP, "vertex use without vertex")
                    })?;
                    let fv = pseudo.fillet_vertex(v)?;
                    if fv.status == FilStatus::Unprocessed
                        || fv.vertex_type == FilletVertexType::Mate
                        || fv.vertex_type == FilletVertexType::RailXExtendedEdgeuse
                    {
                        continue;
                    }
                    let tsect_pnt = pseudo.fillet_vertexuse(vu)?.tsect_pnt();
                    // Do a 3D test (an SM_IP_UNKNOWN point is not set up: commented out
                    // upstream)
                    let pnt = tsect_pnt.crv_pos();

                    // Drop 3D point on surface
                    let solutions = curve.global_point_solve(
                        k,
                        &ivl,
                        SolverOperation::Minimize,
                        pnt,
                        tol,
                        SolutionRequested::Single,
                        &GlobalPointSolveOptions::<ObjectHandle>::default(),
                    )?;
                    if solutions.len() != 1 {
                        return Err(SolidError::sm_err(OP));
                    }
                    let mut t = solutions
                        .get(0)
                        .ok_or_else(|| SolidError::sm_err(OP))?
                        .start
                        .param(0)?;
                    if b3d_test && ivl.is_value_on_boundary(t, EFF_ZERO).0 && is_closed_solver_edge
                    {
                        // Do this logical test if dT is on boundary of a
                        // closed edge. (Please noted that the rail is already
                        // in the same direction as the solver)
                        if pseudo.brep().edgeuse(eu)?.orientation == OrientType::Opposite {
                            t = ivl.max();
                        }
                    }

                    let mut index_of_insert = proj_params.len();
                    let mut do_insertion = true;
                    for (kk, &p) in proj_params.iter().enumerate() {
                        if (t - p).abs() < EFF_ZERO_SQRT * scale {
                            do_insertion = false;
                            break;
                        } else if t < p {
                            index_of_insert = kk;
                            break;
                        }
                    }
                    if do_insertion {
                        proj_params.insert(index_of_insert, t);
                        tsect_pnts.insert(index_of_insert, vu);
                    }
                }
            }
        }

        let solver = &self.curve_based.solver;
        let surf1 = Surface::from(rail_offset(solver, 0)?.surface.clone());
        let surf2 = Surface::from(rail_offset(solver, 1)?.surface.clone());
        let mut domain1 = rail_offset(solver, 0)?
            .surface
            .natural_uv_domain_with_nurb(k)?;
        let mut domain2 = rail_offset(solver, 1)?
            .surface
            .natural_uv_domain_with_nurb(k)?;
        solver.setup_offset_extension(k, &surf1, &mut domain1)?;
        solver.setup_offset_extension(k, &surf2, &mut domain2)?;
        // Setup FilletIntersector (no current fillet geometry: the default NULL)
        let mut fi: FilletIntersector<'_, '_, NoSurfaceHandle, FilletGeomId> =
            FilletIntersector::new(k, &surf1, &domain1, &surf2, &domain2, None)?;
        let mut ids = Vec::new();
        for &vu in &tsect_pnts {
            let p = pseudo.fillet_vertexuse(vu)?.tsect_pnt().clone();
            ids.push(fi.base.base.alloc_point(p));
        }

        // (bUseNormalAveraging and bUniformSteps are TRUE; the uniform-steps flag and
        // the angle tolerance are unused by DoLawIntersection)
        let orientations = solver.orientations;
        let original = self
            .curve_based
            .original
            .clone()
            .ok_or_else(|| SolidError::null_pointer(OP))?;
        let opts = LawIntersectionOptions {
            law_orient: self.law_orientation,
            point_curve: &original,
            point_curve_interval: self.law_interval,
            surface_orientations: orientations,
            use_normal_plane_averaging: true,
            approx_tol_3d: Some(tol),
        };
        // `*m_rLaw` is also the solver's own law (SetupOffsetValues evaluates it while
        // the solver is borrowed as the intersector's); SmFilletIntersector's
        // EvaluateLawPoint does not read its law argument, so a copy stands in for it.
        let mut law = self.law.clone();
        // DoLawIntersection runs on the SmFilletIntersector: its EvaluateLawPoint,
        // ComputePointValuesDelta, TestSpanAccuracy and FlushCurve overrides step
        // through this solver's virtuals (the intersector's surfaces are copies of the
        // solver's offsets, kept at its offset distances)
        let trace = [
            fi.base.base.surface(SrfIndex::First),
            fi.base.base.surface(SrfIndex::Second),
        ];
        let traced = match IntersectorSolver::new(self, trace) {
            Ok(mut fs) => fi.do_law_intersection(&mut fs, pseudo, k, &ids, &opts, &mut law),
            Err(e) => Err(e),
        };
        // the points are SmTsectPnt references upstream: copy them back
        for (&vu, &id) in tsect_pnts.iter().zip(&ids) {
            let p = fi.base.base.point(id)?.clone();
            pseudo.fillet_vertexuse_mut(vu)?.set_tsect_pnt(&p);
        }
        traced?;
        for &vu in tsect_pnts.iter().skip(1) {
            if pseudo.fillet_vertexuse(vu)?.tsect_pnt().curve_parameter < 0.0 {
                return Err(SolidError::sm_err(OP));
            }
        }

        Ok(())
    }

    /// `SmVariableRadiusFS::LoadJacobian`: with `crX = [.. u1 v1 u2 v2 t ..]` (the UV
    /// points located as `SmConstantRadiusFS::LoadJacobian` does, `t` the next
    /// parameter), the law's radius `r(t)` and slope `r'(t)` set both offset distances;
    /// `rF[ne..ne+3] = F - G` of the offset points, `rF[ne+3] = C'(t) . (C(t) - F)` with
    /// `C` the (extended) edge curve, and the Jacobian rows `ne..ne+4`. Advances
    /// `num_equations` by 4 and `num_parameters` by 1 plus 2 per surface whose UV point
    /// is not already in `crX`. Returns `rbFoundAnswer`.
    ///
    /// # Errors
    ///
    /// Those of the law evaluation; `SM_ERR` for a zero first derivative; those of the evaluations and
    /// unitizations; [`SolidError::InvalidInput`] for an out-of-range index or a
    /// missing surface or curve.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub fn load_jacobian(
        &mut self,
        k: &Kernel,
        num_equations: &mut usize,
        num_parameters: &mut usize,
        rail_index: usize,
        surfaces: JacobianSurfaces,
        surf_offsets: &[usize; 2],
        x: &[f64],
        f: &mut [f64],
        jacobian: Option<&mut Matrix>,
    ) -> Result<bool> {
        const OP: &str = OP_VR_JAC;
        let mut found = false;
        let [surface1, surface2] = surfaces;
        let base0 = self.curve_based.solver.rail(0)?.base_id;
        let base1 = self.curve_based.solver.rail(1)?.base_id;

        let mut surf1_off = *num_parameters;
        *num_parameters = plus(*num_parameters, 2, OP)?;
        if rail_index == 0 && surface1 == Some(base0) {
            surf1_off = surf_offsets[0];
            *num_parameters = minus(*num_parameters, 2, OP)?;
        }
        if rail_index == 1 && surface2 == Some(base0) {
            surf1_off = surf_offsets[1];
            *num_parameters = minus(*num_parameters, 2, OP)?;
        }

        let mut surf2_off = *num_parameters;
        *num_parameters = plus(*num_parameters, 2, OP)?;
        if rail_index == 1 && surface1 == Some(base1) {
            surf2_off = surf_offsets[0];
            *num_parameters = minus(*num_parameters, 2, OP)?;
        }
        if rail_index == 0 && surface2 == Some(base1) {
            surf2_off = surf_offsets[1];
            *num_parameters = minus(*num_parameters, 2, OP)?;
        }

        let t = at(x, *num_parameters, OP)?;
        *num_parameters = plus(*num_parameters, 1, OP)?;
        let [offset_distance, lp, _] =
            self.law
                .evaluate(t, &self.law_interval, self.law_orientation)?;

        let [o0, o1] = self.curve_based.solver.orientations;
        let uv = Point2::new(at(x, surf1_off, OP)?, at(x, plus(surf1_off, 1, OP)?, OP)?);
        let s0 = self.surface_mut(0)?;
        s0.set_offset_distance(offset_distance * o0);
        let (sf, sduf, sdvf) = offset_evaluate_1st(k, s0, uv)?;
        let mut sn = sduf.cross(sdvf).unitize()?;
        sn *= o0;

        let uv2 = Point2::new(at(x, surf2_off, OP)?, at(x, plus(surf2_off, 1, OP)?, OP)?);
        let s1 = self.surface_mut(1)?;
        s1.set_offset_distance(offset_distance * o1);
        let (sg, sdug, sdvg) = offset_evaluate_1st(k, s1, uv2)?;
        let mut sn2 = sdug.cross(sdvg).unitize()?;
        sn2 *= o1;

        if sduf.length_squared() < EFF_ZERO_SQ
            || sdvf.length_squared() < EFF_ZERO_SQ
            || sdug.length_squared() < EFF_ZERO_SQ
            || sdvg.length_squared() < EFF_ZERO_SQ
        {
            return Err(SolidError::sm_err(OP));
        }

        let diff1 = sf - sg;

        let last_column = minus(*num_parameters, 1, OP)?;
        let pv = self.curve_based.evaluate_base_curve(k, t, 2)?;
        let pv0 = at3(&pv, 0, OP)?;
        let pv1 = at3(&pv, 1, OP)?;
        let pv2 = at3(&pv, 2, OP)?;

        if let Some(j) = jacobian {
            let s1c = plus(surf1_off, 1, OP)?;
            let s2c = plus(surf2_off, 1, OP)?;
            let mut i = *num_equations;
            for (a, b, c, d, n, n2) in [
                (sduf.x, sdvf.x, sdug.x, sdvg.x, sn.x, sn2.x),
                (sduf.y, sdvf.y, sdug.y, sdvg.y, sn.y, sn2.y),
                (sduf.z, sdvf.z, sdug.z, sdvg.z, sn.z, sn2.z),
            ] {
                put_j(j, i, surf1_off, a, OP)?;
                put_j(j, i, s1c, b, OP)?;
                put_j(j, i, surf2_off, -c, OP)?;
                put_j(j, i, s2c, -d, OP)?;
                put_j(j, i, last_column, lp * (n - n2), OP)?;
                i = plus(i, 1, OP)?; // Move to next equation
            }
            put_j(j, i, surf1_off, -pv1.dot(sduf), OP)?;
            put_j(j, i, s1c, -pv1.dot(sdvf), OP)?;
            put_j(j, i, surf2_off, 0.0, OP)?;
            put_j(j, i, s2c, 0.0, OP)?;
            put_j(
                j,
                i,
                last_column,
                pv2.dot(pv0) + pv1.dot(pv1) - pv2.dot(sf) - pv1.dot(sn) * lp,
                OP,
            )?;
        }

        for v in [diff1.x, diff1.y, diff1.z] {
            put(f, *num_equations, v, OP)?;
            *num_equations = plus(*num_equations, 1, OP)?;
        }
        let diff2 = pv0 - sf;
        put(f, *num_equations, pv1.dot(diff2), OP)?;
        *num_equations = plus(*num_equations, 1, OP)?;

        let scaled_tol = self.curve_based.solver.conversion_tol * (1.0 + sf.max_dimension());
        if diff1.x.abs() < scaled_tol
            && diff1.y.abs() < scaled_tol
            && diff1.z.abs() < scaled_tol
            && at(f, minus(*num_equations, 1, OP)?, OP)?.abs() < scaled_tol
        {
            found = true;
        }
        Ok(found)
    }

    /// `GetSurface(rail)`, mutable.
    fn surface_mut(&mut self, rail: usize) -> Result<&mut OffsetSurface> {
        self.curve_based.solver.rail_mut(rail)
    }

    /// `SmVariableRadiusFS::LoadInitialValues(rbDoSurf2Calcs, lRailIndex, crSurface1,
    /// lSurface1Index, pSurface2, rlSurface2Index, rIntervals, rPeriodicities, rGuessT)`:
    /// the point of `surface1` (`None`: the solver's `GetSurface(lRailIndex)`) at the
    /// guess `rGuessT[surface1_index..+2]` is dropped on the (extended) edge curve; that
    /// parameter (moved along the tangent line when outside the curve's interval) sets
    /// the offset distances (`SetupOffsetValues`). With `do_surf2_calcs` the
    /// re-evaluated point is carried to the other rail's offset surface (through the
    /// other edge use's UV trim curve or a global point solve) and that rail's
    /// extended base domain, two `false` periodicities and the UV guess are appended.
    /// Then always: `rIntervals[surface2_index + 2]` is the curve interval extended by
    /// `5 m_dSurfaceExtensionFactor` lengths, a `false` periodicity and the parameter
    /// are appended.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when the curve drop finds no single solution; those of
    /// `SetupOffsetValues` (the fillet law), the evaluations and solves; [`SolidError::InvalidInput`] for an out-of-range
    /// index, a rail index above 1, a missing surface or curve, or edge uses without
    /// `brep`.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub fn load_initial_values(
        &mut self,
        k: &Kernel,
        brep: Option<&Brep>,
        do_surf2_calcs: bool,
        rail_index: usize,
        mut surface1: Option<&mut OffsetSurface>,
        surface1_index: usize,
        surface2_index: usize,
        intervals: &mut ExtentNd,
        periodicities: &mut Vec<bool>,
        guess_t: &mut Vec<f64>,
    ) -> Result<DistanceInitialValues> {
        const OP: &str = OP_VR_INIT;
        if rail_index > 1 {
            return Err(SolidError::invalid_input(OP, "rail index out of range"));
        }
        let uv = Point2::new(
            at(guess_t, surface1_index, OP)?,
            at(guess_t, plus(surface1_index, 1, OP)?, OP)?,
        );
        let mut pnt = match surface1.as_deref_mut() {
            Some(s) => offset_evaluate_point(k, s, uv)?,
            None => offset_evaluate_point(k, self.surface_mut(rail_index)?, uv)?,
        };

        let curve: Curve = match &self.curve_based.extended {
            Some(e) => Curve::from(e.clone()),
            None => self
                .curve_based
                .original
                .clone()
                .ok_or_else(|| SolidError::invalid_input(OP, "no original curve"))?,
        };
        let ivl = curve.natural_interval();
        let mut sols = curve.global_point_solve(
            k,
            &ivl,
            SolverOperation::Minimize,
            pnt,
            self.curve_based.solver.approx_tol_3d,
            SolutionRequested::All,
            &GlobalPointSolveOptions::<ObjectHandle>::default(),
        )?;
        if sols.len() == 2 {
            // [B391]
            let sol = |i: usize| sols.get(i).ok_or_else(|| SolidError::sm_err(OP));
            let t0 = sol(0)?.start.param(0)?;
            let t1 = sol(1)?.start.param(0)?;
            let dist0 = self.law_interval.distance_from(t0);
            let dist1 = self.law_interval.distance_from(t1);
            if dist0 < dist1 / 2.0 {
                sols.remove_at(1, 1);
            } else if dist1 < dist0 / 2.0 {
                sols.remove_at(0, 1);
            } else {
                let val0 = sol(0)?.start.solution_value;
                let val1 = sol(1)?.start.solution_value;
                if val0 < val1 * 0.9 {
                    sols.remove_at(1, 1);
                } else if val1 < val0 * 0.9 {
                    sols.remove_at(0, 1);
                } else {
                    // (SE_MSG(SM_ERR, ..): only signalled) This preserves previous
                    // behavior.
                    sols.remove_at(1, 1);
                }
            }
        }
        if sols.len() != 1 {
            return Err(SolidError::sm_err(OP));
        }
        let drop_t = sols
            .get(0)
            .ok_or_else(|| SolidError::sm_err(OP))?
            .start
            .param(0)?;
        let mut fillet_edge_t = drop_t;

        if !ivl.contains_value(fillet_edge_t, -EFF_ZERO) {
            // Neg tol: return False for on boundary
            let pv = curve.evaluate(k, fillet_edge_t, 1, true, true)?;
            let param_extension = line_closest_point(at3(&pv, 0, OP)?, at3(&pv, 1, OP)?, pnt);
            fillet_edge_t += param_extension;
        }

        let mut tsect = TsectPnt::new();
        if let Some(slot) = tsect.user_doubles.first_mut() {
            *slot = fillet_edge_t; // SetupOffsetValues() looks for this here.
        }
        let radius = self.setup_offset_values(&tsect)?;
        pnt = match surface1 {
            Some(s) => offset_evaluate_point(k, s, uv)?,
            None => offset_evaluate_point(k, self.surface_mut(rail_index)?, uv)?,
        };

        let mut surface2_rail = None;
        let mut surface2_index = surface2_index;
        if do_surf2_calcs {
            let other_rail = 1 - rail_index;
            let solver = &self.curve_based.solver;
            let surface2 = solver.rail(other_rail)?;
            let domain = surface2.base_surface().natural_uv_domain_with_nurb(k)?;
            let size = solver.surface_extension_factor * domain.size();
            let extended = Extent2d::from_points(
                Point2::new(domain.min().x - size.x, domain.min().y - size.y),
                Point2::new(domain.max().x + size.x, domain.max().y + size.y),
            );
            surface2_rail = Some(other_rail);
            let mut s2d = Point3::UNDEF;

            let need_brep = || SolidError::invalid_input(OP, "edge uses without their B-rep");
            let mut other_eu = solver.edgeuses.get(other_rail).copied().flatten();
            if let Some(eu) = other_eu {
                let b = brep.ok_or_else(need_brep)?;
                if edgeuse::uv_trim_curve(b, eu)?.0.is_none() {
                    other_eu = None;
                }
            }
            if let Some(eu) = other_eu {
                let b = brep.ok_or_else(need_brep)?;
                if let Some(f) = edgeuse::face(b, eu)? {
                    let same =
                        b.face(f)?.surface.is_some() && surface2.base_id == SurfaceId::Face(f);
                    if !same {
                        other_eu = None;
                    }
                }
            }
            if let Some(eu) = other_eu {
                let b = brep.ok_or_else(need_brep)?;
                let (uv_curve, _) = edgeuse::uv_trim_curve(b, eu)?;
                let uv_curve = uv_curve
                    .ok_or_else(|| SolidError::invalid_input(OP, "edge use without UV curve"))?;
                s2d = uv_curve.evaluate_point(k, drop_t)?;
                let guess = Point2::new(s2d.x, s2d.y);
                if let Some((sol_uv, dist)) =
                    offset_local_point_solve(k, surface2, &extended, pnt, guess)?
                {
                    if dist < 1.1 * radius {
                        s2d.x = sol_uv.x;
                        s2d.y = sol_uv.y;
                    }
                }
            } else {
                let sols = surface_global_point_solve(
                    k,
                    &Surface::from(surface2.surface.clone()),
                    None,
                    &extended,
                    pnt,
                    EFF_ZERO_SQ,
                )?;
                if sols.len() == 1 {
                    let s = sols
                        .first()
                        .copied()
                        .ok_or_else(|| SolidError::sm_err(OP))?;
                    s2d.x = s.x;
                    s2d.y = s.y;
                }
            }

            let n = guess_t.len();
            *interval_at(intervals, n, OP)? =
                Extent1d::from_min_max(domain.min().x - size.x, domain.max().x + size.x);
            *interval_at(intervals, plus(n, 1, OP)?, OP)? =
                Extent1d::from_min_max(domain.min().y - size.y, domain.max().y + size.y);
            periodicities.push(false);
            periodicities.push(false);
            surface2_index = n;
            guess_t.push(s2d.x);
            guess_t.push(s2d.y);
        }

        let edge_extension =
            5.0 * self.curve_based.solver.surface_extension_factor * (ivl.max() - ivl.min());
        *interval_at(intervals, plus(surface2_index, 2, OP)?, OP)? =
            Extent1d::from_min_max(ivl.min() - edge_extension, ivl.max() + edge_extension);
        periodicities.push(false);
        guess_t.push(fillet_edge_t);

        Ok(DistanceInitialValues {
            surface2_rail,
            surface2_index,
        })
    }
}

fn at3(v: &[Vec3], i: usize, op: &'static str) -> Result<Vec3> {
    v.get(i)
        .copied()
        .ok_or_else(|| SolidError::invalid_input(op, "evaluation shorter than requested"))
}

// ---- SmVariableRadiusFS virtuals ----------------------------------------------------------------

/// The virtuals of `SmVariableRadiusFS` (the class `ReCalcFilletGeom` runs its
/// `SmFilletIntersector` on): its overrides of `GetJacobianSize`, `SetupOffsetValues`,
/// `OffsetRadiiCanChange`, `LoadJacobian`, `LoadInitialValues`, `SetExtensionFlags`,
/// `GetType`, `IsKindOf` and `GetSolverType`; the rest is the `SmFilletSolver` base
/// (`SmCurveBasedFS` overrides none of these but `GetSolverType`, which
/// `SmVariableRadiusFS` overrides again).
impl FilletSolverOps for VariableRadiusFs {
    fn solver(&self) -> &FilletSolver {
        &self.curve_based.solver
    }

    fn solver_mut(&mut self) -> &mut FilletSolver {
        &mut self.curve_based.solver
    }

    /// `SmVariableRadiusFS::GetJacobianSize`: 5.
    fn jacobian_size(&self) -> Result<usize> {
        Ok(VariableRadiusFs::jacobian_size(self))
    }

    /// `SmVariableRadiusFS::SetupOffsetValues`.
    fn setup_offset_values(
        &mut self,
        tsect: &TsectPnt,
        offset_dist: Option<&mut f64>,
    ) -> Result<()> {
        let d = VariableRadiusFs::setup_offset_values(self, tsect)?;
        if let Some(out) = offset_dist {
            *out = d;
        }
        Ok(())
    }

    /// `SmVariableRadiusFS::OffsetRadiiCanChange`.
    fn offset_radii_can_change(&self) -> Result<bool> {
        VariableRadiusFs::offset_radii_can_change(self)
    }

    /// `SmVariableRadiusFS::LoadJacobian` ([`VariableRadiusFs::load_jacobian`], which
    /// leaves the in/out offsets as they are).
    fn load_jacobian(
        &mut self,
        k: &Kernel,
        num_equations: &mut usize,
        num_parameters: &mut usize,
        load: &mut JacobianLoad<'_>,
        f: &mut [f64],
        jacobian: Option<&mut Matrix>,
    ) -> Result<bool> {
        let offsets = [load.surf1_offset, load.surf2_offset];
        VariableRadiusFs::load_jacobian(
            self,
            k,
            num_equations,
            num_parameters,
            load.rail_index,
            [load.surface1, load.surface2],
            &offsets,
            load.x,
            f,
            jacobian,
        )
    }

    /// `SmVariableRadiusFS::LoadInitialValues` ([`VariableRadiusFs::load_initial_values`]).
    ///
    /// Every upstream caller passes `*GetSurface(lRailIndex)` as `crSurface1`, so the
    /// offset distances `SetupOffsetValues` sets in the call change it: the call runs
    /// on the solver's own rail surface, which is then copied into `surface1` (the
    /// callers' copy of it). `rlSurface2Index` is `2` on entry (the callers that pass
    /// `rbDoSurf2Calcs == 0` set it so; the others leave it to this call).
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
        self.curve_based
            .solver
            .rail_mut(rail_index)?
            .clone_from(surface1);
        let r = VariableRadiusFs::load_initial_values(
            self,
            k,
            brep.as_deref(),
            do_surf2_calcs,
            rail_index,
            None,
            surface1_index,
            2,
            intervals,
            periodicities,
            guess_t,
        );
        surface1.clone_from(self.curve_based.solver.rail(rail_index)?);
        let v = r?;
        Ok(v.surface2_rail.map(|surface2_rail| InitialValues {
            surface2_rail,
            surface2_index: v.surface2_index,
        }))
    }

    /// `SmVariableRadiusFS::SetExtensionFlags` ([`VariableRadiusFs::set_extension_flags`]).
    ///
    /// # Errors
    ///
    /// Those of [`VariableRadiusFs::set_extension_flags`].
    fn set_extension_flags(
        &mut self,
        k: &Kernel,
        b: &Brep,
        corners: &dyn crate::blend::fillet_solver::p1::ExtensionCorners,
    ) -> Result<()> {
        VariableRadiusFs::set_extension_flags(self, k, b, corners)
    }

    /// `SmVariableRadiusFS::GetType`.
    fn type_code(&self) -> Result<i64> {
        Ok(VariableRadiusFs::TYPE)
    }

    /// `SmVariableRadiusFS::IsKindOf`.
    fn is_kind_of(&self, t: i64) -> Result<bool> {
        Ok(VariableRadiusFs::is_kind_of(self, t))
    }

    /// `SmVariableRadiusFS::GetSolverType`.
    fn solver_type(&self) -> Result<FilletSolverType> {
        Ok(VariableRadiusFs::solver_type(self))
    }

    /// `SM_CAST_PTR(SmVariableRadiusFS, this)`.
    fn variable_radius_mut(&mut self) -> Option<&mut VariableRadiusFs> {
        Some(self)
    }
}

// ---- SmConstantDistanceFS virtuals --------------------------------------------------------------

/// The virtuals of `SmConstantDistanceFS`: its overrides of `CanMakeAnalytic`,
/// `GetJacobianSize`, `GetSolverType`, `LoadJacobian`, `LoadInitialValues`,
/// `SetupOffsetValues`, `GetType` and `IsKindOf` (unit
/// `blend.fillet_standard_solver_p1`), plus the `GetDistance` cast
/// `SmFindClippedRadiusEFO` makes; the rest is the `SmFilletSolver` base.
impl FilletSolverOps for ConstantDistanceFs {
    fn solver(&self) -> &FilletSolver {
        &self.solver
    }

    fn solver_mut(&mut self) -> &mut FilletSolver {
        &mut self.solver
    }

    /// `SmConstantDistanceFS::GetJacobianSize`: 5.
    fn jacobian_size(&self) -> Result<usize> {
        Ok(ConstantDistanceFs::jacobian_size(self))
    }

    /// `SmConstantDistanceFS::SetupOffsetValues`.
    fn setup_offset_values(
        &mut self,
        tsect: &TsectPnt,
        offset_dist: Option<&mut f64>,
    ) -> Result<()> {
        let d = ConstantDistanceFs::setup_offset_values(self, tsect)?;
        if let Some(out) = offset_dist {
            *out = d;
        }
        Ok(())
    }

    /// `SmConstantDistanceFS::CanMakeAnalytic`: `TRUE`.
    fn can_make_analytic(&self) -> Result<bool> {
        Ok(ConstantDistanceFs::can_make_analytic(self))
    }

    /// `SmConstantDistanceFS::LoadJacobian` ([`ConstantDistanceFs::load_jacobian`], which
    /// only reads the in/out offsets).
    fn load_jacobian(
        &mut self,
        k: &Kernel,
        num_equations: &mut usize,
        num_parameters: &mut usize,
        load: &mut JacobianLoad<'_>,
        f: &mut [f64],
        jacobian: Option<&mut Matrix>,
    ) -> Result<bool> {
        ConstantDistanceFs::load_jacobian(
            self,
            k,
            num_equations,
            num_parameters,
            load.rail_index,
            [load.surface1, load.surface2],
            [load.surf1_offset, load.surf2_offset],
            load.x,
            f,
            jacobian,
        )
    }

    /// `SmConstantDistanceFS::LoadInitialValues` ([`ConstantDistanceFs::load_initial_values`]).
    ///
    /// Every upstream caller passes `*GetSurface(lRailIndex)` as `crSurface1`, so the
    /// offset distances the call sets change it: the call runs on the solver's own rail
    /// surface, which is then copied into `surface1` (the callers' copy of it), as for
    /// `SmVariableRadiusFS`. `rlSurface2Index` is `2` on entry (the callers that pass
    /// `rbDoSurf2Calcs == 0` set it so; the others leave it to this call).
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
        self.solver.rail_mut(rail_index)?.clone_from(surface1);
        let r = ConstantDistanceFs::load_initial_values(
            self,
            k,
            brep.as_deref(),
            do_surf2_calcs,
            rail_index,
            crate::blend::fillet_standard_solver::p1::InitialSurface::Rail(rail_index),
            surface1_index,
            2,
            intervals,
            periodicities,
            guess_t,
        );
        surface1.clone_from(self.solver.rail(rail_index)?);
        let v = r?;
        Ok(v.surface2_rail.map(|surface2_rail| InitialValues {
            surface2_rail,
            surface2_index: v.surface2_index,
        }))
    }

    /// `SmConstantDistanceFS::GetType`.
    fn type_code(&self) -> Result<i64> {
        Ok(ConstantDistanceFs::TYPE)
    }

    /// `SmConstantDistanceFS::IsKindOf`.
    fn is_kind_of(&self, t: i64) -> Result<bool> {
        Ok(ConstantDistanceFs::is_kind_of(self, t))
    }

    /// `SmConstantDistanceFS::GetSolverType`.
    fn solver_type(&self) -> Result<FilletSolverType> {
        Ok(ConstantDistanceFs::solver_type(self))
    }

    /// `((SmConstantDistanceFS*)this)->GetDistance()`.
    fn constant_distance(&self) -> Result<f64> {
        Ok(self.distance())
    }
}

// ---- surface generators -------------------------------------------------------------------------

impl CircularCrossSectionFsg {
    /// `SmCircularCrossSectionFSG::CreateSurface(rFilletSolver, pCenterLine,
    /// pRail1Curve, pRail2Curve, crFilletPoints, pSurf1, pUV1, pSurf2, pUV2,
    /// rpFilletSurface)`: [`CircularCrossSectionFsg::create_surface_from_curves`] at the
    /// solver's tolerance, without `G1` continuity (upstream keeps it off for every
    /// solver type). The fillet points and surfaces are not used.
    ///
    /// # Errors
    ///
    /// Those of [`CircularCrossSectionFsg::create_surface_from_curves`].
    pub fn create_surface(
        &self,
        k: &Kernel,
        approx_tol_3d: f64,
        _solver_type: FilletSolverType,
        center_line: &mut BSplineCurve,
        rail1: &mut BSplineCurve,
        rail2: &mut BSplineCurve,
    ) -> Result<BSplineSurface> {
        let need_g1_continuity = false;
        // (SM_FS_CONST_RADIUS: "bNeedG1Continuity = TRUE; // fix it later" is commented
        // out upstream)
        // the centre line is a plain SmBSplineCurve here, so its virtuals are the
        // B-spline ones
        let mut cl = Curve::from(core::mem::take(center_line));
        let r = self.create_surface_from_curves(
            k,
            &mut cl,
            rail1,
            rail2,
            approx_tol_3d,
            need_g1_continuity,
        );
        let CurveKind::BSpline(b) = cl.into_kind() else {
            return Err(SolidError::invalid_input(
                "blend::fillet_standard_solver::p2::CircularCrossSectionFsg::create_surface",
                "the centre line changed kind",
            ));
        };
        *center_line = b;
        r
    }
}

impl BlendCurveCrossSectionFsg {
    /// `SmBlendCurveCrossSectionFSG::CreateSurface(rFilletSolver, pCenterLine,
    /// pRail1Curve, pRail2Curve, crFilletPoints, pSurf1, pUV1, pSurf2, pUV2,
    /// rpFilletSurface)`:
    /// [`BlendCurveCrossSectionFsg::create_surface_from_curves`] at the solver's
    /// tolerance, with `G1` continuity for a constant-radius solver
    /// (`SM_FS_CONST_RADIUS`). The fillet points are not used.
    ///
    /// # Errors
    ///
    /// Those of [`BlendCurveCrossSectionFsg::create_surface_from_curves`].
    #[allow(clippy::too_many_arguments)]
    pub fn create_surface(
        &self,
        k: &Kernel,
        approx_tol_3d: f64,
        solver_type: FilletSolverType,
        center_line: &mut BSplineCurve,
        rail1: &mut BSplineCurve,
        rail2: &mut BSplineCurve,
        side1: RailSurface<'_>,
        side2: RailSurface<'_>,
    ) -> Result<BSplineSurface> {
        let mut need_g1_continuity = false;
        if solver_type == FilletSolverType::ConstRadius {
            need_g1_continuity = true;
        }
        self.create_surface_from_curves(
            k,
            center_line,
            rail1,
            rail2,
            approx_tol_3d,
            need_g1_continuity,
            side1,
            side2,
        )
    }
}

// ---- SmSurfaceSurfaceFS ---------------------------------------------------------------------------

impl SurfaceSurfaceFs {
    /// `SmSurfaceSurfaceFS(crContext, dThisApproxTol3d, dAngleTolerance,
    /// dTangencyTolerance, dOffsetRadiusSurface1, dOffsetRadiusSurface2, crSurface1,
    /// crSurface2, bOrientationSurface1, bOrientationSurface2, crUVGuessSurface1,
    /// crUVGuessSurface2)`: the constant-radius solver over the two surfaces, the UV
    /// guesses and `SM_BT_MINIMAL` trimming. The constructor's fillet geometry is
    /// stored in `pseudo` for solver `id` and receives copies of the offsets
    /// (`FinishConstruction`, in the `SmConstantRadiusFS` constructor).
    ///
    /// # Errors
    ///
    /// Those of [`ConstantRadiusAssistedFs::from_surfaces`].
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        k: &Kernel,
        pseudo: &mut FilletBrep,
        id: FilletSolverId,
        approx_tol_3d: f64,
        ang_tol_rad: f64,
        tangency_tolerance: f64,
        offset_radii: [f64; 2],
        surfaces: [(Surface, SurfaceId); 2],
        orientations: [bool; 2],
        uv_guesses: [Point2; 2],
    ) -> Result<Self> {
        let base = ConstantRadiusAssistedFs::from_surfaces(
            k,
            pseudo,
            id,
            approx_tol_3d,
            ang_tol_rad,
            tangency_tolerance,
            offset_radii,
            surfaces,
            orientations,
        )?;
        Ok(Self {
            base,
            uv_guesses,
            trim_type: BoundaryTrimmingType::Minimal,
        })
    }

    /// `SmSurfaceSurfaceFS::IsKindOf`: its own type or a type a constant-radius solver
    /// is.
    #[must_use]
    pub const fn is_kind_of(&self, t: i64) -> bool {
        t == Self::TYPE || constant_radius_is_kind_of(t)
    }

    /// `SmConstantRadiusFS::LoadInitialValues` of a surface/surface solver (its
    /// `SetupOffsetExtension`): [`constant_radius_load_initial_values`].
    ///
    /// # Errors
    ///
    /// Those of [`constant_radius_load_initial_values`].
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
        constant_radius_load_initial_values(
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

    /// `SmSurfaceSurfaceFS::CalcFilletGeom`: the start point of the ball from the UV
    /// guesses (curve point the middle of the two offset points, curve tangent the
    /// cross product of their normals), then a step along the offset intersection.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for parallel normals at the guesses; those of the evaluations and of
    /// `StepSolve`; `SM_ERR_NULL_POINTER` once the geometry is to be built: this entry
    /// point is a solver outside an executive, whose `m_pExecutive->GetPseudoBrep()`
    /// upstream dereferences `NULL` (a solver in a live fillet session runs
    /// [`SurfaceSurfaceFs::calc_fillet_geom_with_context`]).
    pub fn calc_fillet_geom(&mut self, k: &Kernel) -> Result<()> {
        if self.calc_fillet_start(k)?.is_none() {
            return Ok(());
        }
        Err(SolidError::null_pointer(OP_SS_CALC))
    }

    /// `SmSurfaceSurfaceFS::CalcFilletGeom` up to `m_pExecutive->GetPseudoBrep()`: the
    /// start point from the UV guesses, the step along the offset intersection
    /// (`StepSolve`, retried after three alternating point solves), the extended
    /// domains. `None` where upstream returns `SM_SUCCESS` before that (no good step,
    /// or no fillet geometry).
    ///
    /// # Errors
    ///
    /// As [`SurfaceSurfaceFs::calc_fillet_geom`] before the pseudo-B-rep.
    pub fn calc_fillet_start(&mut self, k: &Kernel) -> Result<Option<SurfaceSurfaceStart>> {
        const OP: &str = OP_SS_CALC;
        let domain1 = self
            .base
            .surface(0)?
            .surface
            .natural_uv_domain_with_nurb(k)?;
        let domain2 = self
            .base
            .surface(1)?
            .surface
            .natural_uv_domain_with_nurb(k)?;

        // Need to solve for start point using guess point
        let [g0, g1] = self.uv_guesses;
        let mut curr = TsectPnt::new();
        *curr.uv_pos_mut(SrfIndex::First) = g0;
        *curr.uv_deriv_mut(SrfIndex::First) = Point2::new(0.0, 0.0);
        *curr.uv_pos_mut(SrfIndex::Second) = g1;
        *curr.uv_deriv_mut(SrfIndex::Second) = Point2::new(0.0, 0.0);
        let p1 = offset_evaluate_point(k, self.base.surface_mut(0)?, g0)?;
        let p2 = offset_evaluate_point(k, self.base.surface_mut(1)?, g1)?;
        *curr.crv_pos_mut() = (p1 + p2) / 2.0;
        let n1 = offset_evaluate_normal(k, self.base.surface_mut(0)?, g0)?;
        let n2 = offset_evaluate_normal(k, self.base.surface_mut(1)?, g1)?;
        *curr.crv_deriv_mut() = n1.cross(n2);
        if curr.crv_deriv().length_squared() < EFF_ZERO_SQ {
            return Err(SolidError::sm_err(OP));
        }

        let mut step = 0.0001;
        let mut stepped = TsectPnt::new();
        let mut outcome = StepSolveOutcome::default();
        FilletSolver::step_solve(
            self,
            k,
            None,
            &curr,
            &mut step,
            &domain1,
            &domain2,
            &mut stepped,
            &mut outcome,
        )?;
        if !outcome.found_good_point {
            for _ii in 0..3 {
                let p4 = offset_evaluate_point(
                    k,
                    self.base.surface_mut(1)?,
                    curr.uv_pos(SrfIndex::Second),
                )?;
                if let Some((uv, _)) = offset_local_point_solve(
                    k,
                    self.base.surface(0)?,
                    &domain1,
                    p4,
                    curr.uv_pos(SrfIndex::First),
                )? {
                    *curr.uv_pos_mut(SrfIndex::First) = uv;
                }
                let p3 = offset_evaluate_point(
                    k,
                    self.base.surface_mut(0)?,
                    curr.uv_pos(SrfIndex::First),
                )?;
                if let Some((uv, _)) = offset_local_point_solve(
                    k,
                    self.base.surface(1)?,
                    &domain2,
                    p3,
                    curr.uv_pos(SrfIndex::Second),
                )? {
                    *curr.uv_pos_mut(SrfIndex::Second) = uv;
                }
            }
            FilletSolver::step_solve(
                self,
                k,
                None,
                &curr,
                &mut step,
                &domain1,
                &domain2,
                &mut stepped,
                &mut outcome,
            )?;
            if !outcome.found_good_point {
                return Ok(None);
            }
        }

        let tsect_pnts = vec![stepped];
        let mut domain1 = domain1;
        let mut domain2 = domain2;
        let base1 = Surface::from(self.base.surface(0)?.surface.clone());
        let base2 = Surface::from(self.base.surface(1)?.surface.clone());
        ConstantRadiusOps::setup_offset_extension(self, k, &base1, &mut domain1)?;
        ConstantRadiusOps::setup_offset_extension(self, k, &base2, &mut domain2)?;

        if self.base.solver.fillet_geoms.is_empty() {
            return Ok(None);
        }
        Ok(Some(SurfaceSurfaceStart {
            tsect_pnts,
            domains: [domain1, domain2],
        }))
    }
}

/// The state `SmSurfaceSurfaceFS::CalcFilletGeom` reaches at
/// `pBrep = m_pExecutive->GetPseudoBrep()`: the stepped start point and the extended
/// domains of the two offset surfaces.
#[derive(Debug, Clone)]
pub struct SurfaceSurfaceStart {
    /// `sTsectPnts` (the one point `StepSolve` found).
    pub tsect_pnts: Vec<TsectPnt>,
    /// `sDomain1`, `sDomain2` after `SetupOffsetExtension`.
    pub domains: [Extent2d; 2],
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blend::fillet_standard_solver::p1::{
        FilletLawKind, CONSTANT_RADIUS_FS_TYPE, SURFACE_SURFACE_FS_TYPE, VARIABLE_RADIUS_FS_TYPE,
    };
    use crate::curve::line::Line;
    use crate::geom::vector2d::Vec2;
    use crate::nurbs::crv_basic::Curve as NurbCurve;
    use crate::nurbs::math::{CPoint, NOW};
    use crate::surface::plane::Plane;

    fn p(x: f64, y: f64, z: f64) -> Point3 {
        Point3::new(x, y, z)
    }

    /// The plane through the origin spanned by `x` (1, 0, 0) and `y` over `[0, 1]^2`.
    fn plane(y: Vec3) -> Surface {
        Surface::from(Plane::new(
            p(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            y,
            Vec2::new(1.0, 1.0),
            &Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0),
        ))
    }

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

    /// A rolling-ball corner: z = 0 (normal +z, offset up) and y = 0 (normal -y,
    /// offset towards +y) with radius `r`; the offsets meet on `y = z = r`.
    fn corner(k: &Kernel, r: f64) -> ConstantRadiusAssistedFs {
        let mut s0 = OffsetSurface::new(
            k,
            r,
            plane(Vec3::new(0.0, 1.0, 0.0)),
            SurfaceId::Free(0),
            SS_UNKNOWN,
        )
        .expect("offset");
        let mut s1 = OffsetSurface::new(
            k,
            -r,
            plane(Vec3::new(0.0, 0.0, 1.0)),
            SurfaceId::Free(1),
            SS_UNKNOWN,
        )
        .expect("offset");
        s0.set_allow_extension(true);
        s1.set_allow_extension(true);
        ConstantRadiusAssistedFs::from_parts(
            1e-6,
            30.0 * PI / 180.0,
            2.0 * PI / 180.0,
            [r, r],
            [Some(s0), Some(s1)],
            [1.0, -1.0],
            [None, None],
        )
    }

    #[test]
    fn guess_plane_solve_finds_the_ball_on_a_corner() {
        let k = Kernel::default();
        let mut fs = corner(&k, 0.25);
        let d = Extent2d::from_bounds(-1.0, -1.0, 2.0, 2.0);
        let mut t = TsectPnt::new();
        let found = fs
            .guess_point_on_plane_solve(
                &k,
                p(0.4, 0.0, 0.0),
                Vec3::new(1.0, 0.0, 0.0),
                &d,
                &d,
                Point2::new(0.45, 0.3),
                Point2::new(0.35, 0.2),
                &mut t,
            )
            .expect("solve");
        assert!(found);
        let (a, b) = (t.uv_pos(SrfIndex::First), t.uv_pos(SrfIndex::Second));
        assert!(
            (a.x - 0.4).abs() < 1e-6 && (a.y - 0.25).abs() < 1e-6,
            "{a:?}"
        );
        assert!(
            (b.x - 0.4).abs() < 1e-6 && (b.y - 0.25).abs() < 1e-6,
            "{b:?}"
        );
        // LoadJacobian set the offsets to radius times orientation (as they were)
        assert_eq!(fs.surface(1).expect("s1").offset_distance(), -0.25);
    }

    #[test]
    fn point_on_plane_solve_finds_the_ball_on_a_corner() {
        use crate::blend::fillet_intersector::FilletSolverOps as IntersectorOps;
        let k = Kernel::default();
        let mut fs = corner(&k, 0.25);
        let d = Extent2d::from_bounds(-1.0, -1.0, 2.0, 2.0);
        let mut t = TsectPnt::new();
        let found = point_on_plane_solve(
            &mut fs,
            &k,
            None,
            p(0.4, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            &d,
            &d,
            Point2::new(0.45, 0.3),
            Point2::new(0.35, 0.2),
            &mut t,
        )
        .expect("solve");
        assert!(found);
        // the first offset surface's point lies on the plane x = 0.4, on the ball's
        // centre line y = z = 0.25
        let c = t.srf_pos(SrfIndex::First);
        assert!(
            (c.x - 0.4).abs() < 1e-6 && (c.y - 0.25).abs() < 1e-6 && (c.z - 0.25).abs() < 1e-6,
            "{c:?}"
        );
        // the intersector's virtual dispatches to the same override
        let mut fs2 = corner(&k, 0.25);
        let mut t2 = TsectPnt::new();
        let found2 = IntersectorOps::point_on_plane_solve(
            &mut fs2,
            &k,
            p(0.4, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            &d,
            &d,
            Point2::new(0.45, 0.3),
            Point2::new(0.35, 0.2),
            &mut t2,
        )
        .expect("solve");
        assert!(found2);
        assert_eq!(t2.srf_pos(SrfIndex::First), c);
        // and so does the solver's own virtual (StepSolve, sm_FindFilletTsectPnt), not
        // the base SmFilletSolver::PointOnPlaneSolve
        let mut fs3 = corner(&k, 0.25);
        let mut t3 = TsectPnt::new();
        let ops: &mut dyn FilletSolverOps = &mut fs3;
        let found3 = ops
            .point_on_plane_solve(
                &k,
                None,
                p(0.4, 0.0, 0.0),
                Vec3::new(1.0, 0.0, 0.0),
                &d,
                &d,
                Point2::new(0.45, 0.3),
                Point2::new(0.35, 0.2),
                &mut t3,
            )
            .expect("solve");
        assert!(found3);
        assert_eq!(t3.srf_pos(SrfIndex::First), c);
    }

    #[test]
    fn surface_surface_initial_values_use_its_extension() {
        let k = Kernel::default();
        let mut fs = SurfaceSurfaceFs::new(
            &k,
            &mut FilletBrep::new(),
            FilletSolverId(0),
            1e-6,
            0.5,
            0.03,
            [0.25, 0.25],
            [
                (plane(Vec3::new(0.0, 1.0, 0.0)), SurfaceId::Free(0)),
                (plane(Vec3::new(0.0, 0.0, 1.0)), SurfaceId::Free(1)),
            ],
            [true, false],
            [Point2::new(0.5, 0.25), Point2::new(0.5, 0.25)],
        )
        .expect("solver");
        assert_eq!(fs.trim_type, BoundaryTrimmingType::Minimal);
        assert!(fs.is_kind_of(SURFACE_SURFACE_FS_TYPE) && fs.is_kind_of(CONSTANT_RADIUS_FS_TYPE));
        let mut s1 = fs.base.surface(0).expect("s0").clone();
        let mut ivls = ExtentNd::new(4);
        let mut per = vec![false, false];
        let mut guess = vec![0.5, 0.25];
        let v = fs
            .load_initial_values(
                &k, None, true, 0, &mut s1, 0, &mut ivls, &mut per, &mut guess,
            )
            .expect("initial values")
            .expect("computed");
        assert_eq!((v.surface2_rail, v.surface2_index), (1, 2));
        assert_eq!(guess.len(), 4);
        assert!((guess[2] - 0.5).abs() < 1e-9 && (guess[3] - 0.25).abs() < 1e-9);
        // the domain extended by half its size on both sides
        let iv = ivls.get(2).expect("u interval");
        assert_eq!((iv.min(), iv.max()), (-0.5, 1.5));
        assert_eq!(per.len(), 4);

        // a plain constant-radius solver runs the base SetupOffsetExtension (wired)
        let mut cr = fs.base.clone();
        let (mut ivls2, mut per2, mut guess2) =
            (ExtentNd::new(4), vec![false, false], vec![0.5, 0.25]);
        let r = cr
            .load_initial_values(
                &k,
                None,
                true,
                0,
                &mut s1,
                0,
                &mut ivls2,
                &mut per2,
                &mut guess2,
            )
            .expect("base initial values");
        assert!(r.is_some());
        // nothing to do without the second surface
        assert!(cr
            .load_initial_values(&k, None, false, 0, &mut s1, 0, &mut ivls, &mut per, &mut guess)
            .expect("no-op")
            .is_none());
    }

    #[test]
    fn calc_fillet_geom_rejects_parallel_planes_then_steps() {
        let k = Kernel::default();
        let parallel = |z: f64| {
            Surface::from(Plane::new(
                p(0.0, 0.0, z),
                Vec3::new(1.0, 0.0, 0.0),
                Vec3::new(0.0, 1.0, 0.0),
                Vec2::new(1.0, 1.0),
                &Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0),
            ))
        };
        let g = [Point2::new(0.5, 0.5), Point2::new(0.5, 0.5)];
        let mut fs = SurfaceSurfaceFs::new(
            &k,
            &mut FilletBrep::new(),
            FilletSolverId(0),
            1e-6,
            0.5,
            0.03,
            [0.1, 0.1],
            [
                (parallel(0.0), SurfaceId::Free(0)),
                (parallel(1.0), SurfaceId::Free(1)),
            ],
            [true, false],
            g,
        )
        .expect("solver");
        assert!(matches!(
            fs.calc_fillet_geom(&k),
            Err(SolidError::Upstream { .. })
        ));
        let mut fs = SurfaceSurfaceFs::new(
            &k,
            &mut FilletBrep::new(),
            FilletSolverId(0),
            1e-6,
            0.5,
            0.03,
            [0.1, 0.1],
            [
                (plane(Vec3::new(0.0, 1.0, 0.0)), SurfaceId::Free(0)),
                (plane(Vec3::new(0.0, 0.0, 1.0)), SurfaceId::Free(1)),
            ],
            [true, false],
            g,
        )
        .expect("solver");
        assert!(matches!(
            fs.calc_fillet_geom(&k),
            Err(SolidError::Upstream {
                code: crate::error::SM_ERR_NULL_POINTER,
                ..
            })
        ));
    }

    #[test]
    fn type_queries_and_generators() {
        assert!(constant_radius_assisted_is_kind_of(
            CONSTANT_RADIUS_ASSISTED_FS_TYPE
        ));
        assert!(constant_radius_assisted_is_kind_of(CONSTANT_RADIUS_FS_TYPE));
        assert!(!constant_radius_assisted_is_kind_of(
            VARIABLE_RADIUS_FS_TYPE
        ));

        let k = Kernel::default();
        let law = FilletLaw::new(FilletLawKind::Linear {
            start_radius: 0.1,
            end_radius: 0.2,
        });
        let centre = Curve::from(Line::from_points(p(0.0, 0.0, 0.0), p(1.0, 0.0, 0.0), 3));
        let vr = VariableRadiusFs::from_surfaces(
            &k,
            &mut FilletBrep::new(),
            FilletSolverId(0),
            1e-6,
            0.5,
            0.03,
            [
                (plane(Vec3::new(0.0, 1.0, 0.0)), SurfaceId::Free(0)),
                (plane(Vec3::new(0.0, 0.0, 1.0)), SurfaceId::Free(1)),
            ],
            [1.0, -1.0],
            centre.clone(),
            law.clone(),
            false,
        );
        // the fillet law evaluates
        assert!(vr.is_ok());
        let mut vr = VariableRadiusFs {
            curve_based: CurveBasedFs::new(1e-6, 0.5, 0.03),
            law,
            law_orientation: false,
            law_interval: Extent1d::from_min_max(0.0, 1.0),
        };
        assert!(vr.is_kind_of(VARIABLE_RADIUS_FS_TYPE) && vr.is_kind_of(CURVE_BASED_FS_TYPE));
        assert!(!vr.is_kind_of(CONSTANT_RADIUS_FS_TYPE));
        let mut pseudo = FilletBrep::new();
        assert!(matches!(
            vr.re_calc_fillet_geom(&k, &mut pseudo, false, None, None),
            Err(SolidError::Upstream { .. })
        ));
        // (no fillet geometry: upstream reads m_vFilletGeoms[0] out of bounds)
        assert!(matches!(
            vr.re_calc_fillet_geom(&k, &mut pseudo, false, None, Some(&centre)),
            Err(SolidError::InvalidInput { .. })
        ));

        // the generators' CreateSurface: G1 only for the blend curves of a
        // constant-radius solver
        let r1 = line(p(0.0, 0.0, 1.0), p(5.0, 0.0, 1.0));
        let r2 = line(p(0.0, 1.0, 0.0), p(5.0, 1.0, 0.0));
        let mut c = line(p(0.0, 1.0, 1.0), p(5.0, 1.0, 1.0));
        let (mut a, mut b) = (r1.copy(), r2.copy());
        let s = CircularCrossSectionFsg::default()
            .create_surface(
                &k,
                1e-6,
                FilletSolverType::ConstRadius,
                &mut c,
                &mut a,
                &mut b,
            )
            .expect("circular");
        let corner = s.evaluate_point(Point2::new(0.0, 0.0)).expect("point");
        assert!(corner.distance(p(0.0, 0.0, 1.0)) < 1e-6);
        let none = RailSurface {
            surface: None,
            uv_curve: None,
        };
        let mut c = line(p(0.0, 1.0, 1.0), p(5.0, 1.0, 1.0));
        let (mut a, mut b) = (r1.copy(), r2.copy());
        b.insert_one_knot(0.5, 1).expect("knot");
        let r = BlendCurveCrossSectionFsg::default().create_surface(
            &k,
            1e-6,
            FilletSolverType::VariableRadius,
            &mut c,
            &mut a,
            &mut b,
            none,
            none,
        );
        assert!(matches!(r, Err(SolidError::Upstream { .. })));
    }
}
