//! The fillet solver base: solver data, offset-surface bookkeeping, surface extension
//! domains, the solver's nonlinear function objects (sphere/plane solve, rail-UV-curve
//! and rail-rail intersections, clipped radius), and the fillet surface generator types.
//!
//! Upstream: `source/SMLib/src/SmFilletSolver.cpp`, `source/SMLib/inc/SmFilletSolver.h`.
//!
//! | Rust                                                   | Upstream                                              |
//! |--------------------------------------------------------|-------------------------------------------------------|
//! | [`FilletSolverOffsetsData`]                            | `SmFilletSolverOffsetsData` (constructor)             |
//! | [`FilletSolver`], [`FilletSolver::new_in`]             | `SmFilletSolver` (members, constructor; `Drop` is the destructor) |
//! | [`FilletSolver::new`], [`FilletSolver::add_constructor_fillet_geom`] | the constructor without, and its `new SmFilletGeom(this)` |
//! | [`FilletSolver::finish_construction`]                  | `SmFilletSolver::FinishConstruction`                  |
//! | getters / setters of [`FilletSolver`]                  | `GetStatus`, `GetReverseTrim`, `GetFilletExecutive`, `GetFilletSurfaceGenerator`, `GetThisApproxTol3d`, `GetThisAngTolRad`, `GetConversionTol`, `GetTangencyTolerance`, `GetEdgeuse`, `GetFilletGeomForSurfaces`, `GetFirstFilletGeom`, `GetLastFilletGeom`, `GetOrientation`, `GetExtensionFactor`, `SetStatus`, `SetReverseTrim`, `SetExtensionFactor`, `SetFilletExecutive`, `SetFilletSurfaceGenerator`, `SetFilletGeomForSurfaces` (`GetCreationContext`: the `Kernel` argument) |
//! | [`FilletSolver::surface`], [`FilletSolver::set_surface`] | `GetSurface`, `SetSurface`                          |
//! | [`FilletSolver::fillet_geoms`]                         | `GetFilletGeoms`                                      |
//! | [`FilletSolver::self_intersection_handler`]            | `GetSelfIntersectionHandler`                          |
//! | [`FilletSolver::vertex`]                               | `GetVertex`                                           |
//! | [`FilletSolver::find_index_of_rail_on_face`]           | `FindIndexOfRailOnFace`                               |
//! | [`FilletSolver::find_index_of_rail_x_side_edgeuse`]    | `FindIndexOfRailXSideEdgeuse`                         |
//! | [`FilletSolver::set_surfaces_to_zero_offset`]          | `SetSurfacesToZeroOffset`                             |
//! | [`FilletSolver::reload_surface_offsets`]               | `ReloadSurfaceOffsets`                                |
//! | [`FilletSolver::set_extension_flags`]                  | `SetExtensionFlags`                                   |
//! | [`FilletSolver::set_offsets_data`], [`FilletSolver::offsets_data`] | `SetOffsetsData`, `GetOffsetsData`        |
//! | [`FilletSolver::setup_offset_extension`]               | `SetupOffsetExtension`                                |
//! | [`FilletSolverOps::setup_offset_extension`]            | `SetupOffsetExtension` (virtual dispatch)             |
//! | [`FilletSolver::compute_surface_values`]               | `ComputeSurfaceValues`                                |
//! | [`compute_uv_vectors`]                                 | `SmFilletSolver::ComputeUVVectors`                    |
//! | [`FilletSolver::split_fillet_geom_at_side_edge`]       | `SplitFilletGeomAtSideEdge`                           |
//! | [`FilletSolver::split_fillet_geom`], [`FilletSolver::split_fillet_geom_with_target`] | `SplitFilletGeom`                                     |
//! | [`FilletSolver::trim_rails_with`]                      | `TrimRails`                                           |
//! | [`FilletSolver::TYPE`], [`FilletSolver::type_string`], [`FilletSolver::is_kind_of`] | `GetType`, `GetTypeString` (`SM_COMMON`), `IsKindOf` |
//! | [`FilletSolverOps`] (trait, base methods)              | the virtuals `GetJacobianSize`, `SetupOffsetValues`, `CanMakeAnalytic`, `OffsetRadiiCanChange`, `LoadJacobian`, `LoadInitialValues`; `GetSolverType`, `PointOnPlaneSolve` and the `SmConstantDistanceFS::GetDistance` read of `SmFindClippedRadiusEFO` (added by `blend.fillet_solver_p3`) |
//! | [`FilStatus`] (re-export of `blend.fillet_geom_p1`'s), [`FilletSolverType`], [`FilletSurfaceGeneratorType`] | `SmFilStatus`, `SmFilletSolverType`, `SmFilletSurfaceGeneratorType` |
//! | [`FilletSurfaceGenerator`]                             | `SmFilletSurfaceGenerator`, `SmLinearCrossSectionFSG`, `SmCircularCrossSectionFSG`, `SmBlendCurveCrossSectionFSG` (constructors, `GetFilletSurfaceGeneratorType`, `GetBlendScale`, `GetContinuity`; `Drop` is each destructor); the circular and blend-curve variants hold `blend.fillet_standard_solver_p1`'s [`CircularCrossSectionFsg`] / [`BlendCurveCrossSectionFsg`] (one type per class) |
//! | [`FilletSurfaceGenerator::create_surface`]             | `SmFilletSurfaceGenerator::CreateSurface` (base; the linear one dispatches to [`LinearCrossSectionFsg::create_surface`]) |
//! | [`FilletSphereSolveEnfo`]                              | `SmFilletSphereSolveENFO` (constructor, `SetPlaneAdjusts`, `Evaluate`) |
//! | [`RailUvCurveIntersectEnfo`]                           | `SmRailUVCurveIntersectENFO` (constructor, `Evaluate`) |
//! | [`RailRailIntersectEnfo`]                              | `SmRailRailIntersectENFO` (constructor, `Evaluate`)   |
//! | [`FindClippedRadiusEfo`]                               | `SmFindClippedRadiusEFO` (constructor, `Evaluate`)    |
//! | [`tol_ang_tol_rad`]                                  | `SmTol::GetAngTolRad(const SmFilletSolver*)` |
//! | [`check_edge_curve_domains`]                           | `sm_CheckEdgeCurveDomains`                            |
//! | [`test_analytic_surface`], [`AnalyticTestSurface`]     | `sm_TestAnalyticSurface`                              |
//! | [`test_manifold_tangent_rollover`]                     | `sm_TestManifoldTangentRollover`                      |
//! | [`test_side_edge_convexity`]                           | `sm_TestSideEdgeConvexity`                            |
//! | [`find_next_good_interval`]                            | `sm_FindNextGoodInterval`                             |
//! | [`trim_rail`]                                          | `sm_TrimRail` (a rail edge of a [`Brep`])             |
//! | [`split_rail`], [`compute_support_edge`]               | `sm_SplitRail`, `sm_ComputeSupportEdge`               |
//! | (`Drop`)                                               | `~SmSelfIntersectionHandler`, `~SmMakeCurveBlendSIH`, `~SmMakeSurfaceBlendSIH` (no data) |
//! | [`MakeSurfaceBlendSih`]                                | `SmMakeSurfaceBlendSIH` (constructor, `GetAlgorithmType`) |
//!
//! Deviations:
//! - **Object model.** Virtual methods of the solver are the [`FilletSolverOps`] trait;
//!   [`FilletSolver`] implements the base versions. Offset surfaces are the
//!   [`OffsetSurface`] rails of `blend::fillet_custom_solver` (the ported
//!   `SmOffsetSurface` with the identity of its base) held in an
//!   [`OffsetSurfaceSlot`] whose `key` stands for the `SmOffsetSurface*` upstream
//!   compares (`SetOffsetsData`); [`FilletSolverOffsetsData`] holds copies of the slots,
//!   so restoring a slot with a different key puts the saved surface back (upstream
//!   deletes the current surface and stores the saved pointer). Evaluations of the
//!   solver's own offsets keep a base extension they make, as upstream's `const`
//!   evaluators do; `SmFindClippedRadiusEFO` evaluates copies (its iso curve and
//!   `LocalCurveIntersect`), whose extension the solver's offsets then take
//!   (`FilletSolver::keep_extension_of_copy`).
//! - **Fillet geometries, executive, corners.** As in `blend.fillet_geom_p1`, the
//!   solver's `SmFilletGeom`s live in the executive's pseudo B-rep ([`FilletBrep`],
//!   `m_pExecutive->m_pPseudoBrep`) with its fillet vertices and edges; the solver
//!   holds their [`FilletGeomId`]s and the functions that reach them take the pseudo
//!   B-rep (`pseudo`); ids of the B-rep being filleted are ids of `b`. A geometry's
//!   `GetOffsetSurface` falls back to the solver it is called from (its own geometries).
//!   The constructor's `new SmFilletGeom(this)` needs the pseudo B-rep:
//!   [`FilletSolver::new_in`] is the full constructor (the derived solvers'
//!   constructors call it, then [`FilletSolver::finish_construction`] where upstream
//!   calls `FinishConstruction`); [`FilletSolver::new`] leaves it out (bare solvers of
//!   tests and harnesses) and [`FilletSolver::add_constructor_fillet_geom`] adds it
//!   later. `GetSurface` /
//!   `SetSurface` with a geometry set for the surfaces (`m_pFGForSurfaces`) read and
//!   write that geometry's offset surfaces, which the scope setting it lends to the
//!   solver (`lend_geometry_surfaces`; the solver cannot reach the pseudo B-rep); a rail
//!   the geometry has none for is refused (upstream recurses without bound, UB-204). `SmFilletExecutive` and the fillet corners belong
//!   to executive/corner units. [`FilletExecutiveRef`] records only whether the
//!   native back pointer is set. Callers pass the operation's executive explicitly.
//!   `SplitFilletGeomAtSideEdge`, `SplitFilletGeom`, `TrimRails`,
//!   `sm_SplitRail`, `sm_ComputeSupportEdge` and `sm_TestManifoldTangentRollover` are
//!   ported in full. The single typed [`split_rail`] / [`compute_support_edge`]
//!   implementations take [`FilletBrep`] and [`RailOwnership`], preserving topology
//!   backlinks, curve owners and notifications. Rail creation delegates to
//!   `FilletBrep::make_fillet_edge`; its missing infinite-region constructor remains
//!   a `topology.brep_p3` deferral. Support construction uses the ported surface
//!   intersection. `SplitFilletGeomAtSideEdge` uses the same pseudo-B-rep records.
//! - **Virtual overrides of unported units.** A derived solver's override owned by a
//!   unit that is not ported returns [`SolidError::Unported`]; the [`FilletSolverOps`]
//!   methods are fallible for that reason. The custom solvers' inherited
//!   `SmConstantRadiusFS` overrides delegate to `ConstantRadiusAssistedFs`.
//! - **Analytic surfaces.** `sm_TestAnalyticSurface` takes an [`AnalyticTestSurface`]:
//!   a [`Surface`], a [`Cone`] (cone or cylinder), a [`Sphere`] or a [`Torus`], since
//!   no [`Surface`] kind holds the last three yet.
//! - **Rail trims.** `sm_TrimRail` trims curves of the rail edge and its edge uses,
//!   whose `SmTol::GetZoneTol3d(this)` is the rail's tolerance; `Curve::trim` reads the
//!   zone tolerance of an owner-less curve from its kernel, so the trims get a kernel
//!   with the rail's tolerance as zone tolerance (`plan/findings.md`, probe notes).
//! - **Arguments.** Out-params become return values; `SmContext` arguments become the
//!   [`Kernel`]. `SmFindClippedRadiusEFO` takes the target distance its constructor
//!   reads from the `SmConstantDistanceFS` (`GetDistance`, unit
//!   `blend.fillet_standard_solver_p1`). `SplitFilletGeom` takes the raw
//!   `SmBoundaryTrimmingType` value.
//! - **Undefined inputs.** A `NULL` dereference upstream (a missing edge use, offset
//!   surface, executive, loop use) and an index past an array (`crX`, `rF`, the
//!   Jacobian, the fillet geometries, the interval arrays) are
//!   [`SolidError::InvalidInput`].
//! - **Iteration.** No loop here depends on more than the caller's array sizes; there
//!   is no recursion.

use crate::blend::fillet_classification::{FilletClassObject, LegacyClassificationObjects};
use crate::blend::fillet_corner::p3::FilletCorner;
use crate::blend::fillet_custom_solver::{InitialValues, JacobianLoad, OffsetSurface, SurfaceId};
use crate::blend::fillet_geom::p1::{
    BoundaryTrimmingType, FilletBrep, FilletGeom, FilletGeomId, FilletGeomType, FilletSolverId,
    FilletVertexType,
};
use crate::blend::fillet_geom::p3::SplitCorners;
use crate::blend::fillet_intersector::HandlingAlgorithmType;
use crate::blend::fillet_standard_solver::p1::{
    BlendCurveCrossSectionFsg, CircularCrossSectionFsg, LinearCrossSectionFsg,
};
use crate::curve::curve::{Curve, CurveKind};
use crate::curve::iso_curve::IsoCurve;
use crate::error::{ignore_status, ignored, EntityRef, Result, SolidError};
use crate::geom::extent1d::{Extent1d, ExtentNd};
use crate::geom::extent2d::Extent2d;
use crate::geom::matrix::Matrix;
use crate::geom::object::OBJECT_TYPE;
use crate::geom::tol::ZONE_TOL_3D;
use crate::geom::vector2d::{Point2, Vec2};
use crate::intersect::curve_class::p1::CurveClassification;
use crate::intersect::point_class::ClassGeometry;
use crate::intersect::point_class::{ClassObject, PointClassType};
use crate::intersect::surface_intersector::p1::TsectPnt;
use crate::kernel::Kernel;
use crate::math::{Point3, Vec3, EFF_ZERO, EFF_ZERO_SQ, PI, UNDEF_DOUBLE};
use crate::nurbs::b_spline_curve::BSplineCurve;
use crate::nurbs::b_spline_curve_n_lib::TrimMode;
use crate::nurbs::b_spline_surface::SurfParamType;
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::nurbs::knots::Side;
use crate::surface::cone::Cone;
use crate::surface::offset_surface::SS_UNKNOWN;
use crate::surface::sphere::Sphere;
use crate::surface::surface::p1::evaluate_1st_derivatives;
use crate::surface::surface::p2::drop_vectors;
use crate::surface::surface::{Surface, SurfaceKind};
use crate::surface::torus::Torus;
use crate::topology::edgeuse::p1 as eu;
use crate::topology::edgeuse::p3::{evaluate_binormal, BinormalOptions};
use crate::topology::face::p2 as face_p2;
use crate::topology::{edge, vertexuse, Brep, EdgeId, EdgeuseId, FaceId, VertexId};
use crate::topology::{vertex, VertexuseId};

const OP: &str = "blend::fillet_solver::p1";

/// `SmFilletSolver_TYPE` (`TOPO_BASE_TYPE + 37`).
const FILLET_SOLVER_TYPE: i64 = 16037;

/// `static int cbiLinear` of `SetupOffsetExtension`: never changed, so the linear
/// extension branch is never taken.
const CBI_LINEAR: i32 = 0;

fn invalid(reason: &'static str) -> SolidError {
    SolidError::invalid_input(OP, reason)
}

/// Upstream `if (call != SM_SUCCESS) return SM_ERR;`: any failure becomes `SM_ERR`;
/// [`SolidError::Unported`] and [`SolidError::InvalidInput`] (a path that is undefined
/// upstream, not a status) pass through.
fn as_sm_err(e: SolidError, op: &'static str) -> SolidError {
    match e {
        SolidError::Unported { .. }
        | SolidError::InvalidInput { .. }
        | SolidError::PortInvariant { .. } => e,
        _ => SolidError::sm_err(op),
    }
}

// ---- small types ----------------------------------------------------------------------

/// `SmFilStatus` (`SmFilletGeom.h`) is unit `blend.fillet_geom_p1`'s
/// [`FilStatus`] (one type per class): the
/// processing status of a solver and of fillet geometries, vertices and edges.
pub use crate::blend::fillet_geom::p1::FilStatus;

/// The kind of a fillet solver (`SmFilletSolverType`, `SmFilletSolver.h`; the
/// `GetSolverType` overrides belong to the derived solvers' units); discriminants are
/// the upstream values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FilletSolverType {
    /// `SM_FS_UNKNOWN`.
    Unknown = 0,
    /// `SM_FS_SURF_SURF`.
    SurfSurf = 1,
    /// `SM_FS_CONST_RADIUS`.
    ConstRadius = 2,
    /// `SM_FS_VARIABLE_RADIUS`.
    VariableRadius = 3,
    /// `SM_FS_CONST_DIST`.
    ConstDist = 4,
    /// `SM_FS_CURVE_BASED`.
    CurveBased = 5,
    /// `SM_FS_CONST_RADIUS_ASSISTED`.
    ConstRadiusAssisted = 6,
}

/// `SmFilletSurfaceGeneratorType`; discriminants are the upstream values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FilletSurfaceGeneratorType {
    /// `SM_FSG_UNKNOWN`.
    Unknown = 0,
    /// `SM_FSG_LINEAR`.
    Linear = 1,
    /// `SM_FSG_CIRCULAR`.
    Circular = 2,
    /// `SM_FSG_BLEND_CURVE`.
    BlendCurve = 3,
}

/// A fillet surface generator (`SmFilletSurfaceGenerator` and its derived classes):
/// the cross-section shape of the fillet surface.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FilletSurfaceGenerator {
    /// `SmFilletSurfaceGenerator` itself.
    Base,
    /// `SmLinearCrossSectionFSG`: a ruled surface (the generator has no members:
    /// [`LinearCrossSectionFsg`] of `blend.fillet_standard_solver_p1`).
    Linear,
    /// `SmCircularCrossSectionFSG` (its members and `CreateSurfaceFromCurves`:
    /// [`CircularCrossSectionFsg`] of `blend.fillet_standard_solver_p1`).
    Circular(CircularCrossSectionFsg),
    /// `SmEiffelTowerCrossSectionFSG` (unit `blend.fillet_custom_solver`).
    EiffelTower,
    /// `SmBlendCurveCrossSectionFSG` (its members and `CreateSurfaceFromCurves`:
    /// [`BlendCurveCrossSectionFsg`] of `blend.fillet_standard_solver_p1`).
    BlendCurve(BlendCurveCrossSectionFsg),
}

impl FilletSurfaceGenerator {
    /// `SmCircularCrossSectionFSG(bApproximateArcs, dTolPercentRadius, bMirror,
    /// bComplement)`.
    #[must_use]
    pub const fn circular(
        approximate_arcs: bool,
        tol_percent_radius: f64,
        mirror: bool,
        complement: bool,
    ) -> Self {
        Self::Circular(CircularCrossSectionFsg {
            approximate_arcs,
            tol_percent_radius,
            mirror,
            complement,
        })
    }

    /// `SmCircularCrossSectionFSG()` with its default arguments (`TRUE, 0.05, FALSE,
    /// FALSE`).
    #[must_use]
    pub const fn circular_default() -> Self {
        Self::circular(true, 0.05, false, false)
    }

    /// `SmBlendCurveCrossSectionFSG(dBlendScale, lContinuity)`.
    #[must_use]
    pub const fn blend_curve(blend_scale: f64, continuity: u32) -> Self {
        Self::blend_curve_at_thirds(blend_scale, continuity, false)
    }

    /// `SmBlendCurveCrossSectionFSG(dBlendScale, lContinuity, bAtThirds)` (the fork's,
    /// source fork `fd3b6a7`): with `at_thirds`, each section's inner control
    /// points sit a third and two thirds of the way from each contact to where the
    /// contacts' tangent lines meet.
    #[must_use]
    pub const fn blend_curve_at_thirds(blend_scale: f64, continuity: u32, at_thirds: bool) -> Self {
        Self::BlendCurve(BlendCurveCrossSectionFsg {
            continuity,
            blend_scale,
            at_thirds,
        })
    }

    /// `SmBlendCurveCrossSectionFSG()` with its default arguments (`1.5, 1`).
    #[must_use]
    pub const fn blend_curve_default() -> Self {
        Self::blend_curve(1.5, 1)
    }

    /// `MeetsItsFacesTangentially` (virtual; the fork's, source fork `8bffb23`):
    /// whether each cross section is tangent, at its ends on the rails, to the faces the
    /// fillet rolls on. The linear section cuts across both faces, an arc about the ball's
    /// centre is square to the faces at each rail unless mirrored about the rails' chord,
    /// and a blend curve takes its end tangents from that arc; the base generator and any
    /// custom one are not known to.
    #[must_use]
    pub const fn meets_its_faces_tangentially(&self) -> bool {
        match self {
            Self::Circular(g) => !g.mirror,
            Self::BlendCurve(_) => true,
            Self::Base | Self::Linear | Self::EiffelTower => false,
        }
    }

    /// `GetFilletSurfaceGeneratorType` (virtual).
    #[must_use]
    pub const fn fillet_surface_generator_type(&self) -> FilletSurfaceGeneratorType {
        match self {
            Self::Base | Self::EiffelTower => FilletSurfaceGeneratorType::Unknown,
            Self::Linear => FilletSurfaceGeneratorType::Linear,
            Self::Circular(_) => FilletSurfaceGeneratorType::Circular,
            Self::BlendCurve(_) => FilletSurfaceGeneratorType::BlendCurve,
        }
    }

    /// `SmBlendCurveCrossSectionFSG::GetBlendScale` (`None` for the other kinds).
    #[must_use]
    pub const fn blend_scale(&self) -> Option<f64> {
        match self {
            Self::BlendCurve(g) => Some(g.blend_scale),
            _ => None,
        }
    }

    /// `SmBlendCurveCrossSectionFSG::GetContinuity` (`None` for the other kinds).
    #[must_use]
    pub const fn continuity(&self) -> Option<u32> {
        match self {
            Self::BlendCurve(g) => Some(g.continuity),
            _ => None,
        }
    }

    /// `CreateSurface(rFilletSolver, pCenterLine, pRail1Curve, pRail2Curve,
    /// crFilletPoints, pSurf1, pUV1, pSurf2, pUV2, rpFilletSurface)` (virtual): `SM_ERR`
    /// for the base, else the generator's surface. The solver is read only for its
    /// `GetThisApproxTol3d()` (`approx_tol_3d`) and `GetSolverType()`
    /// (`solver_type`); `center_line` is `None` for `NULL`.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for [`FilletSurfaceGenerator::Base`]; those of
    /// `SmEiffelTowerCrossSectionFSG::CreateSurface`,
    /// `SmLinearCrossSectionFSG::CreateSurface` ([`LinearCrossSectionFsg::create_surface`])
    /// and the circular and blend-curve generators' `CreateSurface`;
    /// [`SolidError::InvalidInput`] for a missing centre line of the circular and
    /// blend-curve generators (upstream dereferences `NULL`).
    #[allow(clippy::too_many_arguments)]
    pub fn create_surface(
        &self,
        k: &Kernel,
        approx_tol_3d: f64,
        solver_type: FilletSolverType,
        center_line: Option<&mut BSplineCurve>,
        rail1: &mut BSplineCurve,
        rail2: &mut BSplineCurve,
        points: &[TsectPnt],
        side1: crate::blend::fillet_standard_solver::p1::RailSurface<'_>,
        side2: crate::blend::fillet_standard_solver::p1::RailSurface<'_>,
    ) -> Result<crate::nurbs::b_spline_surface::BSplineSurface> {
        let no_center = || SolidError::invalid_input(OP, "NULL centre line");
        match self {
            Self::Base => Err(SolidError::sm_err(OP)),
            Self::EiffelTower => {
                crate::blend::fillet_custom_solver::EiffelTowerCrossSectionFsg::new()
                    .create_surface(k, approx_tol_3d, rail1, rail2, points)
            }
            // the ruled surface between the rails (SER)
            Self::Linear => LinearCrossSectionFsg.create_surface(rail1, rail2),
            Self::Circular(g) => g.create_surface(
                k,
                approx_tol_3d,
                solver_type,
                center_line.ok_or_else(no_center)?,
                rail1,
                rail2,
            ),
            Self::BlendCurve(g) => g.create_surface(
                k,
                approx_tol_3d,
                solver_type,
                center_line.ok_or_else(no_center)?,
                rail1,
                rail2,
                side1,
                side2,
            ),
        }
    }
}

/// The fillet surface self-intersection handler that steps back and makes a blending
/// surface in the area of a self intersection (`SmMakeSurfaceBlendSIH`). Its
/// `SmoothOutIntersection` is a no-op and `CreateBlends` has no caller upstream.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MakeSurfaceBlendSih {
    /// `m_dStepBackDistFactor`: factor of the fillet radius to step back from an
    /// intersection before making blends between fillet surfaces.
    pub step_back_dist_factor: f64,
}

impl MakeSurfaceBlendSih {
    /// `SmMakeSurfaceBlendSIH(dStepBackDistFactor)`.
    #[must_use]
    pub const fn new(step_back_dist_factor: f64) -> Self {
        Self {
            step_back_dist_factor,
        }
    }

    /// `GetAlgorithmType`: `SM_HA_BLENDING`.
    #[must_use]
    pub const fn algorithm_type(&self) -> HandlingAlgorithmType {
        HandlingAlgorithmType::Blending
    }
}

/// Whether the solver's native executive back pointer is set.
/// The operation passes its one executive explicitly as context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FilletExecutiveRef;

/// An offset surface owned by a solver, with the identity of the `SmOffsetSurface*`
/// upstream compares.
#[derive(Debug, Clone)]
pub struct OffsetSurfaceSlot {
    /// Equal keys are the same `SmOffsetSurface` object.
    pub key: u64,
    /// The surface.
    pub surface: OffsetSurface,
}

/// The tangent-rollover UV guesses `SmConstantRadiusFS::LoadInitialValues` reads from
/// the rails of the geometry for the surfaces (`SM_FG_TANGENT_ROLLOVER`): taken before
/// `SmFilletGeom::RailEdgeuseIntersect`'s solve, which does not change the
/// pseudo-B-rep they come from.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RolloverGuesses {
    /// Whether the geometry is of type `SM_FG_TANGENT_ROLLOVER`.
    pub tangent_rollover: bool,
    /// Per rail index: `None` without the rail (upstream dereferences `NULL`), else the
    /// other surface's UV of the rail's first `SM_FV_RAIL_X_EDGEUSE` vertex use with a
    /// set intersection point (`Some(None)`: no such vertex use).
    pub rails: [Option<Option<Point2>>; 2],
}

/// `SmFilletSolverOffsetsData`: saved offset surfaces and orientations (roll-over).
#[derive(Debug, Clone)]
pub struct FilletSolverOffsetsData {
    /// `m_pSurfaces`.
    pub surfaces: [Option<OffsetSurfaceSlot>; 2],
    /// `m_dOrientations`.
    pub orientations: [f64; 2],
}

impl Default for FilletSolverOffsetsData {
    /// No surfaces, orientations `1.0`.
    fn default() -> Self {
        Self {
            surfaces: [None, None],
            orientations: [1.0, 1.0],
        }
    }
}

// ---- the solver ------------------------------------------------------------------------

/// The fillet solver base (`SmFilletSolver`).
#[derive(Debug, Clone)]
pub struct FilletSolver {
    /// `m_pEdgeuses`: the two radial edge uses of the sector being filleted.
    pub edgeuses: [Option<EdgeuseId>; 2],
    /// `m_pExecutive`.
    pub executive: Option<FilletExecutiveRef>,
    /// `m_pExecutive->m_pSelfIntersectionHandler`, as its `GetAlgorithmType()` (the
    /// only part of the handler a solver reads). The owning executive keeps it equal
    /// to its own handler while this solver is loaded (`None` here).
    pub executive_self_intersection_handler: Option<HandlingAlgorithmType>,
    /// `m_pExecutive->m_pSelfIntersectionHandler` when it is an
    /// `SmMakeCurveBlendSIH` (the handler `SmoothOutIntersection` runs); kept equal
    /// to the executive's handler like `executive_self_intersection_handler`.
    pub executive_curve_blend_sih: Option<super::curve_blend_sih::MakeCurveBlendSih>,
    /// `m_dThisApproxTol3d`.
    pub approx_tol_3d: f64,
    /// `m_dThisAngTolRad`.
    pub ang_tol_rad: f64,
    /// `m_dTangencyTolerance`.
    pub tangency_tolerance: f64,
    /// `m_dEdgeBlendFactor` (`1.0`).
    pub edge_blend_factor: f64,
    /// `m_pSurfaces`.
    pub surfaces: [Option<OffsetSurfaceSlot>; 2],
    /// `m_dOrientations` (not initialised upstream; `1.0` here, see [`FilletSolver::new`]).
    pub orientations: [f64; 2],
    /// `m_pFGForSurfaces`.
    pub fg_for_surfaces: Option<FilletGeomId>,
    /// The geometry whose offset surfaces (`m_pOffSurfs`) are lent to the solver for
    /// the scope that made it the geometry for the surfaces
    /// ([`FilletSolver::lend_geometry_surfaces`]); upstream reads them through
    /// `m_pFGForSurfaces`, which the solver cannot reach in the pseudo B-rep.
    pub(crate) lent_geometry: Option<FilletGeomId>,
    /// The lent offset surfaces of [`FilletSolver::lent_geometry`], each with a transient
    /// identity key (`SetOffsetsData` compares keys).
    pub(crate) lent_surfaces: [Option<OffsetSurfaceSlot>; 2],
    /// The tangent-rollover guesses of the geometry for the surfaces, taken by the
    /// scope that set it ([`FilletSolver::lend_geometry_surfaces`]); upstream's
    /// `SmConstantRadiusFS::LoadInitialValues` reads them through `m_pFGForSurfaces`.
    pub(crate) lent_rollover: Option<(FilletGeomId, RolloverGuesses)>,
    /// `m_dSavedOffsets` (not initialised upstream; `0.0` here).
    pub saved_offsets: [f64; 2],
    /// `m_dSurfaceExtensionFactor` (`0.5`).
    pub surface_extension_factor: f64,
    /// `m_dConversionTol` (`SM_EFF_ZERO / 10`).
    pub conversion_tol: f64,
    /// `m_bReverseTrim`.
    pub reverse_trim: bool,
    /// `m_bExtendBefore` (not initialised upstream; `false` here).
    pub extend_before: bool,
    /// `m_bExtendAfter` (not initialised upstream; `false` here).
    pub extend_after: bool,
    /// `m_pFSG`.
    pub fsg: Option<FilletSurfaceGenerator>,
    /// `m_vFilletGeoms`: geometries stored in the executive's pseudo B-rep
    /// ([`FilletBrep`], `m_pExecutive->m_pPseudoBrep`).
    pub fillet_geoms: Vec<FilletGeomId>,
    /// `m_eStatus`.
    pub status: FilStatus,
    /// `m_vG1Knots`.
    pub g1_knots: Vec<f64>,
    /// Offset surfaces [`FilletSolver::set_surface`] displaced: upstream does not
    /// delete them, so a saved [`FilletSolverOffsetsData`] still points at them (with
    /// any change made while they were the solver's).
    pub detached: Vec<OffsetSurfaceSlot>,
}

impl FilletSolver {
    /// `SmFilletSolver_TYPE` (`GetType`).
    pub const TYPE: i64 = FILLET_SOLVER_TYPE;

    /// `SmFilletSolver(crContext, dThisApproxTol3d, dThisAngTolRad,
    /// dTangencyTolerance)` without its fillet geometry: no edge uses, surfaces or
    /// executive. The constructor's `new SmFilletGeom(this)` is stored in the pseudo
    /// B-rep, which this constructor does not take: [`FilletSolver::new_in`] is the
    /// full constructor, and [`FilletSolver::add_constructor_fillet_geom`] adds that
    /// geometry to a solver made here (the derived solvers' constructors use
    /// [`FilletSolver::new_in`]). Members upstream leaves uninitialised (`m_pEdgeuses`, `m_pExecutive`,
    /// `m_dOrientations`, `m_dSavedOffsets`, the extension flags) start as `None`,
    /// `1.0`, `0.0` and `false`; every derived constructor sets them before use.
    #[must_use]
    pub fn new(approx_tol_3d: f64, ang_tol_rad: f64, tangency_tolerance: f64) -> Self {
        Self {
            edgeuses: [None, None],
            executive: None,
            executive_self_intersection_handler: None,
            executive_curve_blend_sih: None,
            approx_tol_3d,
            ang_tol_rad,
            tangency_tolerance,
            edge_blend_factor: 1.0,
            surfaces: [None, None],
            orientations: [1.0, 1.0],
            fg_for_surfaces: None,
            lent_geometry: None,
            lent_surfaces: [None, None],
            lent_rollover: None,
            saved_offsets: [0.0, 0.0],
            surface_extension_factor: 0.5,
            conversion_tol: EFF_ZERO / 10.0,
            reverse_trim: false,
            extend_before: false,
            extend_after: false,
            fsg: None,
            fillet_geoms: Vec::new(),
            status: FilStatus::Unprocessed,
            g1_knots: Vec::new(),
            detached: Vec::new(),
        }
    }

    /// `SmFilletSolver(crContext, dThisApproxTol3d, dThisAngTolRad,
    /// dTangencyTolerance)`: [`FilletSolver::new`], then the constructor's fillet
    /// geometry ([`FilletSolver::add_constructor_fillet_geom`]) in `pseudo`, with `id`
    /// as the solver's identity.
    ///
    /// # Errors
    ///
    /// Those of [`FilletSolver::add_constructor_fillet_geom`].
    pub fn new_in(
        k: &Kernel,
        pseudo: &mut FilletBrep,
        id: FilletSolverId,
        approx_tol_3d: f64,
        ang_tol_rad: f64,
        tangency_tolerance: f64,
    ) -> Result<Self> {
        let mut solver = Self::new(approx_tol_3d, ang_tol_rad, tangency_tolerance);
        solver.add_constructor_fillet_geom(k, pseudo, id)?;
        Ok(solver)
    }

    /// The constructor's `m_vFilletGeoms.Add(new SmFilletGeom(this))`: a default
    /// geometry of solver `id` (with copies of the solver's offset surfaces, which a
    /// constructor has not set yet), stored in `pseudo`.
    ///
    /// # Errors
    ///
    /// Those of [`FilletGeom::new`].
    pub fn add_constructor_fillet_geom(
        &mut self,
        k: &Kernel,
        pseudo: &mut FilletBrep,
        id: FilletSolverId,
    ) -> Result<FilletGeomId> {
        let geom = FilletGeom::new(k, Some((id, &*self)), FilletGeomType::Default)?;
        let geom = pseudo.add_geom(geom);
        self.fillet_geoms.push(geom);
        Ok(geom)
    }

    /// `"SmFilletSolver_TYPE"` (`GetTypeString`).
    #[must_use]
    pub const fn type_string(&self) -> &'static str {
        "SmFilletSolver_TYPE"
    }

    /// True for `SmFilletSolver_TYPE` and `SmObject_TYPE` (`IsKindOf`).
    #[must_use]
    pub const fn is_kind_of(t: i64) -> bool {
        t == FILLET_SOLVER_TYPE || t == OBJECT_TYPE
    }

    /// `FinishConstruction`: installs copies of the offset surfaces into every fillet
    /// geometry (stored in `pseudo`).
    ///
    /// # Errors
    ///
    /// Those of [`FilletSolver::surface`] and the copy; [`SolidError::NotFound`] for a
    /// geometry id not in `pseudo`. A failed copy is skipped, as upstream (its status is
    /// ignored and a `NULL` copy is not installed).
    pub fn finish_construction(&self, k: &Kernel, pseudo: &mut FilletBrep) -> Result<()> {
        for fg in &self.fillet_geoms {
            for which in 0..2 {
                let Some(ours) = self.surface(which)? else {
                    continue;
                };
                // SmOffsetSurface::Copy() returns an SmSurface*, not SmOffsetSurface*
                if let Some(copy) = copy_offset_surface(k, Some(ours))? {
                    pseudo
                        .geom_mut(*fg)?
                        .set_offset_surface(which, Some(copy))?;
                }
            }
        }
        Ok(())
    }

    /// `GetStatus`.
    #[must_use]
    pub const fn status(&self) -> FilStatus {
        self.status
    }

    /// `SetStatus`.
    pub fn set_status(&mut self, status: FilStatus) {
        self.status = status;
    }

    /// `GetReverseTrim`.
    #[must_use]
    pub const fn reverse_trim(&self) -> bool {
        self.reverse_trim
    }

    /// `SetReverseTrim`.
    pub fn set_reverse_trim(&mut self, reverse_trim: bool) {
        self.reverse_trim = reverse_trim;
    }

    /// `GetFilletExecutive`.
    #[must_use]
    pub const fn fillet_executive(&self) -> Option<FilletExecutiveRef> {
        self.executive
    }

    /// `SetFilletExecutive`.
    pub fn set_fillet_executive(&mut self, executive: Option<FilletExecutiveRef>) {
        self.executive = executive;
    }

    /// `GetFilletSurfaceGenerator`.
    #[must_use]
    pub const fn fillet_surface_generator(&self) -> Option<&FilletSurfaceGenerator> {
        self.fsg.as_ref()
    }

    /// `SetFilletSurfaceGenerator`.
    pub fn set_fillet_surface_generator(&mut self, fsg: Option<FilletSurfaceGenerator>) {
        self.fsg = fsg;
    }

    /// `GetThisApproxTol3d`.
    #[must_use]
    pub const fn this_approx_tol_3d(&self) -> f64 {
        self.approx_tol_3d
    }

    /// `GetThisAngTolRad`.
    #[must_use]
    pub const fn this_ang_tol_rad(&self) -> f64 {
        self.ang_tol_rad
    }

    /// `GetConversionTol`.
    #[must_use]
    pub const fn conversion_tol(&self) -> f64 {
        self.conversion_tol
    }

    /// `GetTangencyTolerance`.
    #[must_use]
    pub const fn tangency_tolerance(&self) -> f64 {
        self.tangency_tolerance
    }

    /// `GetEdgeuse(lIndex)`.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an index above 1.
    pub fn edgeuse(&self, index: usize) -> Result<Option<EdgeuseId>> {
        self.edgeuses
            .get(index)
            .copied()
            .ok_or_else(|| invalid("edge use index out of range"))
    }

    /// `GetFilletGeomForSurfaces`.
    #[must_use]
    pub const fn fillet_geom_for_surfaces(&self) -> Option<FilletGeomId> {
        self.fg_for_surfaces
    }

    /// `SetFilletGeomForSurfaces`.
    pub fn set_fillet_geom_for_surfaces(&mut self, fg: Option<FilletGeomId>) {
        self.fg_for_surfaces = fg;
    }

    /// `GetFirstFilletGeom`.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] without fillet geometries.
    pub fn first_fillet_geom(&self) -> Result<FilletGeomId> {
        self.fillet_geoms
            .first()
            .copied()
            .ok_or_else(|| invalid("no fillet geometry"))
    }

    /// `GetLastFilletGeom`.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] without fillet geometries.
    pub fn last_fillet_geom(&self) -> Result<FilletGeomId> {
        self.fillet_geoms
            .last()
            .copied()
            .ok_or_else(|| invalid("no fillet geometry"))
    }

    /// `GetFilletGeoms`.
    #[must_use]
    pub fn fillet_geoms(&self) -> Vec<FilletGeomId> {
        self.fillet_geoms.clone()
    }

    /// `GetOrientation(lRailIndex)`.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for a rail index above 1.
    pub fn orientation(&self, rail: usize) -> Result<f64> {
        self.orientations
            .get(rail)
            .copied()
            .ok_or_else(|| invalid("rail index out of range"))
    }

    /// `GetExtensionFactor`.
    #[must_use]
    pub const fn extension_factor(&self) -> f64 {
        self.surface_extension_factor
    }

    /// `SetExtensionFactor` (virtual; no override).
    pub fn set_extension_factor(&mut self, factor: f64) {
        self.surface_extension_factor = factor;
    }

    /// `GetSelfIntersectionHandler`: the executive's handler, as its algorithm type
    /// (`None` for a `NULL` handler).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] without an executive (upstream dereferences
    /// `NULL`).
    pub fn self_intersection_handler(&self) -> Result<Option<HandlingAlgorithmType>> {
        if self.executive.is_none() {
            return Err(invalid("no fillet executive"));
        }
        Ok(self.executive_self_intersection_handler)
    }

    /// The slot `GetSurface` / `SetSurface` address for rail `rail`: with a geometry for
    /// the surfaces, that geometry's offset surface (`m_pFGForSurfaces->GetOffsetSurface`
    /// / `SetOffsetSurface`), lent to the solver by the scope that set the geometry;
    /// else the solver's own (`m_pSurfaces`).
    fn addressed_slot(&self, rail: usize) -> Result<&Option<OffsetSurfaceSlot>> {
        let slots = if self.fg_for_surfaces.is_none() {
            &self.surfaces
        } else if self.lent_geometry == self.fg_for_surfaces {
            &self.lent_surfaces
        } else {
            return Err(invalid(
                "the geometry for the surfaces has not lent its offset surfaces",
            ));
        };
        slots
            .get(rail)
            .ok_or_else(|| invalid("rail index out of range"))
    }

    /// [`FilletSolver::addressed_slot`], mutable.
    fn addressed_slot_mut(&mut self, rail: usize) -> Result<&mut Option<OffsetSurfaceSlot>> {
        let slots = if self.fg_for_surfaces.is_none() {
            &mut self.surfaces
        } else if self.lent_geometry == self.fg_for_surfaces {
            &mut self.lent_surfaces
        } else {
            return Err(invalid(
                "the geometry for the surfaces has not lent its offset surfaces",
            ));
        };
        slots
            .get_mut(rail)
            .ok_or_else(|| invalid("rail index out of range"))
    }

    /// `GetSurface(lRailIndex)`: with a geometry for the surfaces, that geometry's
    /// offset surface (`SmFilletGeom::GetOffsetSurface`, lent to the solver by
    /// [`FilletSolver::lend_geometry_surfaces`]), else the solver's own.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for a rail index above 1; with a geometry for the
    /// surfaces, when the geometry has no offset surface for the rail
    /// (`GetOffsetSurface` falls back to `m_pFilletSolver->GetSurface`, which asks the
    /// same geometry again without bound: UB-204) or has not lent its surfaces.
    pub fn surface(&self, rail: usize) -> Result<Option<&OffsetSurfaceSlot>> {
        let slot = self.addressed_slot(rail)?;
        if self.fg_for_surfaces.is_some() && slot.is_none() {
            return Err(invalid(
                "geometry set for surfaces has no offset surface for rail (UB-204)",
            ));
        }
        Ok(slot.as_ref())
    }

    pub(crate) fn surface_mut(&mut self, rail: usize) -> Result<Option<&mut OffsetSurfaceSlot>> {
        let with_geometry = self.fg_for_surfaces.is_some();
        let slot = self.addressed_slot_mut(rail)?;
        if with_geometry && slot.is_none() {
            return Err(invalid(
                "geometry set for surfaces has no offset surface for rail (UB-204)",
            ));
        }
        Ok(slot.as_mut())
    }

    /// After an evaluation of `copy`, a copy of rail `rail`'s offset surface made for
    /// it (wrapped in a [`Surface`] or an iso curve), the rail's offset surface takes the
    /// copy's current base extension (`m_pExtendedSurface`): upstream evaluates the
    /// solver's own `SmOffsetSurface`, whose `const` evaluators store the extension they
    /// make. The copy starts from the rail's extension and nothing else evaluates the
    /// rail in between, so this leaves the rail as upstream's one object. A `copy` that
    /// is not an offset surface (or an iso curve of one) has nothing to hand back.
    ///
    /// # Errors
    ///
    /// Those of [`FilletSolver::surface_mut`]; [`SolidError::InvalidInput`] for a missing
    /// offset surface.
    pub(crate) fn keep_extension_of_copy(&mut self, rail: usize, copy: &Surface) -> Result<()> {
        if let SurfaceKind::OffsetSurface(c) = copy.kind() {
            self.surface_mut(rail)?
                .ok_or_else(|| invalid("no offset surface"))?
                .surface
                .surface
                .take_extension_of(c);
        }
        Ok(())
    }

    /// [`FilletSolver::keep_extension_of_copy`] for the surface of an iso curve built
    /// over a copy of rail `rail`'s offset surface (`SmIsoCurve(*GetSurface(rail), ...)`
    /// holds a pointer to the solver's surface upstream).
    ///
    /// # Errors
    ///
    /// Those of [`FilletSolver::keep_extension_of_copy`].
    pub(crate) fn keep_extension_of_iso_copy(&mut self, rail: usize, iso: &Curve) -> Result<()> {
        match iso.kind() {
            CurveKind::Iso(c) => self.keep_extension_of_copy(rail, c.surface()),
            _ => Ok(()),
        }
    }

    /// `SetSurface(lRailIndex, pNewOffsetSurface)`: with a geometry for the surfaces,
    /// `SmFilletGeom::SetOffsetSurface` on its lent offset surfaces (deleting the
    /// replaced one), else the solver's own slot.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for a rail index above 1 (`SetOffsetSurface`'s
    /// `SM_ERR_INVALID_INPUT`), or a geometry for the surfaces that has not lent its
    /// surfaces.
    pub fn set_surface(&mut self, rail: usize, surface: Option<OffsetSurfaceSlot>) -> Result<()> {
        if self.fg_for_surfaces.is_some() {
            // m_pFGForSurfaces->SetOffsetSurface(lRailIndex, pNewOffsetSurface)
            *self.addressed_slot_mut(rail)? = surface;
            return Ok(());
        }
        let slot = self
            .surfaces
            .get_mut(rail)
            .ok_or_else(|| invalid("rail index out of range"))?;
        if let Some(old) = std::mem::replace(slot, surface) {
            self.detached.push(old);
        }
        Ok(())
    }

    /// `GetVertex(lIndex)`: the start (0) or end (other) vertex of the filleted edge.
    /// A missing edge signals `SM_ERR` upstream and returns `NULL`.
    ///
    /// # Errors
    ///
    /// Those of the topology queries.
    pub fn vertex(&self, b: &Brep, index: usize) -> Result<Option<VertexId>> {
        let Some(e0) = self.edgeuse(0)? else {
            return Ok(None);
        };
        let Some(e) = eu::edge(b, e0)? else {
            // SE(SM_ERR): signalled only
            return Ok(None);
        };
        let start = edge::start_vertex(b, e)?;
        if index == 0 {
            return Ok(start);
        }
        edge::other_vertex(b, e, start)
    }

    /// `FindIndexOfRailOnFace(pOrigFace)`: 0 when the first edge use's face is
    /// `face`, else 1 (0 without a first edge use, after signalling `SM_ERR`).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an edge use without loop use or face use
    /// (upstream dereferences `NULL`).
    pub fn find_index_of_rail_on_face(&self, b: &Brep, face: Option<FaceId>) -> Result<usize> {
        let Some(e0) = self.edgeuse(0)? else {
            return Ok(0);
        };
        if eu::loopuse(b, e0)?.is_none() {
            return Err(invalid("edge use without loop use"));
        }
        let fu = eu::faceuse(b, e0)?.ok_or_else(|| invalid("loop use without face use"))?;
        if eu::faceuse_face(b, fu)? == face {
            return Ok(0);
        }
        Ok(1)
    }

    /// `FindIndexOfRailXSideEdgeuse(pSideEU)`: the rail that the side edge use
    /// crosses.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] where upstream dereferences `NULL` (no first edge
    /// use, an edge use without edge, vertex use or counter-clockwise neighbour).
    pub fn find_index_of_rail_x_side_edgeuse(&self, b: &Brep, side_eu: EdgeuseId) -> Result<usize> {
        let mut rail = 0;
        let fil_eu = self
            .edgeuse(0)?
            .ok_or_else(|| invalid("no first edge use"))?;
        let side_edge =
            eu::edge(b, side_eu)?.ok_or_else(|| invalid("side edge use without edge"))?;
        if edge::is_closed(b, side_edge)? {
            // Such as when in 2x1 cases
            let v = vertex_of_edgeuse(b, fil_eu)?;
            let side_v = vertex_of_edgeuse(b, side_eu)?;
            if side_v == v {
                if eu::cw_edgeuse(b, fil_eu)? == Some(side_eu) {
                    rail = 1;
                }
            } else {
                let ccw = eu::ccw_edgeuse(b, fil_eu)?
                    .ok_or_else(|| invalid("edge use without counter-clockwise neighbour"))?;
                if eu::mate(b, ccw)? == Some(side_eu) {
                    rail = 1;
                }
            }
        } else {
            // Work from the faces of our two Edgeuses.
            rail = 0;
            if eu::face(b, fil_eu)? != eu::face(b, side_eu)? {
                rail = 1;
            }
        }
        Ok(rail)
    }

    /// `SetSurfacesToZeroOffset`: saves each surface's offset and sets it to zero.
    ///
    /// # Errors
    ///
    /// Those of [`FilletSolver::surface`].
    pub fn set_surfaces_to_zero_offset(&mut self) -> Result<()> {
        for i in 0..2 {
            let saved = match self.surface_mut(i)? {
                Some(s) => {
                    let d = s.surface.offset_distance();
                    s.surface.set_offset_distance(0.0);
                    Some(d)
                }
                None => None,
            };
            if let (Some(d), Some(slot)) = (saved, self.saved_offsets.get_mut(i)) {
                *slot = d;
            }
        }
        Ok(())
    }

    /// `ReloadSurfaceOffsets`: restores the offsets saved by
    /// [`FilletSolver::set_surfaces_to_zero_offset`].
    ///
    /// # Errors
    ///
    /// Those of [`FilletSolver::surface`].
    pub fn reload_surface_offsets(&mut self) -> Result<()> {
        for i in 0..2 {
            let d = self.saved_offsets.get(i).copied().unwrap_or(0.0);
            if let Some(s) = self.surface_mut(i)? {
                s.surface.set_offset_distance(d);
            }
        }
        Ok(())
    }

    /// `SetExtensionFlags`: extend each end whose corner is neither blending nor
    /// degenerate.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] where upstream dereferences `NULL` (missing edge
    /// uses, vertex uses or executive); those of `corners` (the executive's
    /// `GetFilletCornerOfVertex`, dereferenced).
    pub fn set_extension_flags(&mut self, b: &Brep, corners: &dyn ExtensionCorners) -> Result<()> {
        self.set_extension_flags_with_lookup(b, |vertex| corners.corner_of(vertex))
    }

    /// Set extension flags using the actual corners resolved by the executive.
    /// Both endpoint lookups complete before either flag changes.
    ///
    /// # Errors
    /// Missing topology/executive or the caller's actual corner-resolution errors.
    pub fn set_extension_flags_with_lookup<'corner>(
        &mut self,
        b: &Brep,
        mut corner_of: impl FnMut(Option<VertexId>) -> Result<&'corner FilletCorner>,
    ) -> Result<()> {
        let eu0 = self
            .edgeuse(0)?
            .ok_or_else(|| invalid("no first edge use"))?;
        let eu1 = self
            .edgeuse(1)?
            .ok_or_else(|| invalid("no second edge use"))?;
        let mut vert0 = vertex_of_edgeuse(b, eu0)?;
        let mut vert1 = vertex_of_edgeuse(b, eu1)?;

        // swap vertices for opposite oriented edgeuses
        if eu::orientation(b, eu0)? == OrientType::Opposite {
            std::mem::swap(&mut vert0, &mut vert1);
        }
        if self.executive.is_none() {
            return Err(invalid("no fillet executive"));
        }
        let before = corner_of(vert0)?;
        let after = corner_of(vert1)?;
        self.extend_before = !before.is_blending_corner() && !before.is_degenerate_corner();
        self.extend_after = !after.is_blending_corner() && !after.is_degenerate_corner();
        Ok(())
    }

    /// `SetOffsetsData`: restores saved orientations and offset surfaces (a surface
    /// whose key differs from the saved one is replaced by the saved one).
    ///
    /// # Errors
    ///
    /// Those of [`FilletSolver::surface`] / [`FilletSolver::set_surface`].
    pub fn set_offsets_data(&mut self, data: &FilletSolverOffsetsData) -> Result<()> {
        for i in 0..2 {
            let orientation = data
                .orientations
                .get(i)
                .copied()
                .ok_or_else(|| invalid("rail index out of range"))?;
            if let Some(o) = self.orientations.get_mut(i) {
                *o = orientation;
            }
            let orig = data.surfaces.get(i).and_then(Option::as_ref);
            let current_key = self.surface(i)?.map(|s| s.key);
            if current_key != orig.map(|s| s.key) {
                // the current surface is deleted; the saved one is the live object
                // (detached since the save) when there is one
                let restored = orig.map(|o| {
                    self.detached
                        .iter()
                        .position(|d| d.key == o.key)
                        .map_or_else(|| o.clone(), |pos| self.detached.remove(pos))
                });
                // SetSurface(i, pOrigOffSurf)
                *self.addressed_slot_mut(i)? = restored;
            }
        }
        Ok(())
    }

    /// `GetOffsetsData`: the current orientations and offset surfaces.
    ///
    /// # Errors
    ///
    /// Those of [`FilletSolver::surface`].
    pub fn offsets_data(&self) -> Result<FilletSolverOffsetsData> {
        let mut data = FilletSolverOffsetsData::default();
        for i in 0..2 {
            let o = self.orientation(i)?;
            if let Some(d) = data.orientations.get_mut(i) {
                *d = o;
            }
            let s = self.surface(i)?.cloned();
            if let Some(d) = data.surfaces.get_mut(i) {
                *d = s;
            }
        }
        Ok(data)
    }

    /// `SetupOffsetExtension(pSurf, rDomain)`: grows `domain` by the extension factor
    /// times its size at both ends (a fifth of that across a periodic direction), not
    /// across a singular side.
    ///
    /// # Errors
    ///
    /// Those of `IsPeriodic` and `IsSingularity`.
    pub fn setup_offset_extension(
        &self,
        k: &Kernel,
        surface: &Surface,
        domain: &mut Extent2d,
    ) -> Result<()> {
        // get length of extensions
        let domain_size = domain.size();
        let mut size = Vec2::new(
            self.surface_extension_factor * domain_size.x,
            self.surface_extension_factor * domain_size.y,
        );

        // Extend by a smaller amount if periodic.
        let periodic_extension_factor = 0.2;
        let linear_extension_factor = 50.0;
        if bool_status(surface.is_periodic(k, domain, SurfParamType::U))? {
            size.x *= periodic_extension_factor;
        } else if CBI_LINEAR > 0 && surface.degree_with_nurb(k, SurfParamType::U)? == 1 {
            size.x *= linear_extension_factor;
        }
        if bool_status(surface.is_periodic(k, domain, SurfParamType::V))? {
            size.y *= periodic_extension_factor;
        } else if CBI_LINEAR > 0 && surface.degree_with_nurb(k, SurfParamType::V)? == 1 {
            size.y *= linear_extension_factor;
        }

        // process the min domain boundaries
        let min = domain.min;
        let sing = singularity_status(surface.is_singularity(k, min, EFF_ZERO, false))?;
        if !sing.singular {
            // when surface is not singular - extend the min domain boundary
            domain.add_point(Point2::new(min.x - size.x, min.y - size.y));
        } else if sing.direction == SurfParamType::U {
            // singular in U direction only extend the u direction
            let mut new_min = domain.min;
            new_min.x -= size.x;
            domain.add_point(new_min);
        } else {
            // singular in V direction only extend the v direction
            let mut new_min = domain.min;
            new_min.y -= size.y;
            domain.add_point(new_min);
        }

        // process the max domain boundaries
        let max = domain.max;
        let sing = singularity_status(surface.is_singularity(k, max, EFF_ZERO, false))?;
        if !sing.singular {
            domain.add_point(Point2::new(max.x + size.x, max.y + size.y));
        } else if sing.direction == SurfParamType::U {
            let mut new_max = domain.max;
            new_max.x += size.x;
            domain.add_point(new_max);
        } else {
            let mut new_max = domain.max;
            new_max.y += size.y;
            domain.add_point(new_max);
        }
        Ok(())
    }

    /// `SplitFilletGeomAtSideEdge(pFilletGeom, pSplitFilVU, lWhichEnd,
    /// ppRetFilletGeom)`: splits a fillet geometry where one of its rails meets a side
    /// edge. The part on the face across the side edge (the original geometry for
    /// `which_end` 0, the new one otherwise) becomes a tangent roll-over geometry on an
    /// offset of that face. Returns the new geometry (`*ppRetFilletGeom`).
    ///
    /// `b` is the B-rep being filleted (the side edge, its faces and the solver's edge
    /// uses; the side edge's binormal evaluation may build UV trim curves on it);
    /// `pseudo` is the executive's pseudo-B-rep holding the fillet geometries, the
    /// fillet vertex use, the rails and the fillet vertices. A geometry's
    /// `GetOffsetSurface` falls back to this solver (the geometries are its own).
    ///
    /// # Errors
    ///
    /// Those of [`FilletBrep::rail_index_of_vertexuse`] (`SM_ERR` when the vertex use
    /// ends neither rail), [`FilletBrep::rail_vertex`] and
    /// [`FilletBrep::topology_split`]; `SM_ERR_NULL_POINTER` (`NER`) for a geometry
    /// without solver, a missing fillet vertex, side edge use, original face or
    /// adjacent face;
    /// [`SolidError::InvalidInput`] when the faces across the side edge are not tangent
    /// (`SM_ERR_INVALID_INPUT`), for a classification object that is not an edge use
    /// (upstream casts it) and where upstream dereferences `NULL`;
    /// [`SolidError::NotFound`] for a geometry, vertex or edge without its fillet
    /// record in `pseudo`; those of the offset surface constructor and copies.
    pub fn split_fillet_geom_at_side_edge(
        &mut self,
        k: &Kernel,
        b: &mut Brep,
        pseudo: &mut FilletBrep,
        fillet_geom: FilletGeomId,
        split_vertexuse: VertexuseId,
        which_end: usize,
        corners: &mut dyn SplitCorners,
    ) -> Result<FilletGeomId> {
        const OP_S: &str = "blend::fillet_solver::p1::split_fillet_geom_at_side_edge";
        let nul = || SolidError::null_pointer(OP_S);

        // Get the rail being split.
        let rail_idx = pseudo.rail_index_of_vertexuse(fillet_geom, Some(split_vertexuse))?;
        let other_idx = 1usize
            .checked_sub(rail_idx)
            .ok_or_else(|| invalid("rail index out of range"))?;
        let split_rail = pseudo.geom(fillet_geom)?.rail(rail_idx);

        // The Fillet Vertex contains a PointClass object whose object is the side
        // Edgeuse that we intersect.
        let fil_vtx = vertexuse::vertex(pseudo.brep(), split_vertexuse)?.ok_or_else(nul)?;
        // SM_CAST_PTR(SmEdgeuse, GetPointClassObject())
        let mut side_eu = match pseudo.fillet_vertex(fil_vtx)?.point_class_object() {
            Some(EntityRef::Edgeuse(e)) => e,
            None => return Err(nul()),
            Some(_) => return Err(invalid("point class object is not an edge use")),
        };
        let side_edge =
            eu::edge(b, side_eu)?.ok_or_else(|| invalid("side edge use without edge"))?;

        // Should be the same Face on both sides of the side Edge.
        let split_rail = split_rail.ok_or_else(|| invalid("fillet geometry without rail"))?;
        let orig_face = pseudo
            .fillet_edge(split_rail)?
            .original_face()
            .ok_or_else(nul)?;
        let faces = edge::faces(b, side_edge, None)?;

        // Find the adjacent face -- the one on the other side of the side edge that
        // we hit. If there is only one Face for the side edge, it must be closed.
        let mut adj_face = None;
        let mut closed_side_face = false;
        if faces.len() == 1 {
            // Closed Edge to start with.
            adj_face = faces.first().copied();
            closed_side_face = true;
        } else {
            // Find the other Face.
            for f in &faces {
                if *f != orig_face {
                    adj_face = Some(*f);
                    break;
                }
            }
        }
        let adj_face = adj_face.ok_or_else(nul)?;
        let adj_surface = face_p2::surface(b, adj_face)?
            .ok_or_else(|| invalid("adjacent face without surface"))?
            .clone();

        // Create new offset surface for the adjacent face.
        // First get offset distance and relative direction.
        let mut offset_dist = self
            .surface(rail_idx)?
            .ok_or_else(|| invalid("no offset surface"))?
            .surface
            .offset_distance();

        // Same normals?  This routine will tell us that:
        let mut opposite_normals = false;
        if adj_face != orig_face {
            // Mild assumption: closed surface is not Moebius...
            let (tangent, opposite) =
                test_manifold_tangent_rollover(k, b, side_edge, self.tangency_tolerance)?;
            opposite_normals = opposite;
            if !tangent {
                return Err(SolidError::invalid_input(
                    OP_S,
                    "the faces across the side edge are not tangent",
                ));
            }
        }
        if opposite_normals {
            offset_dist *= -1.0;
        }
        // new SmOffsetSurface(offsetDist, *adjSurface): default lSingularities
        let mut adj_off_surf = OffsetSurface::new(
            k,
            offset_dist,
            adj_surface,
            SurfaceId::Face(adj_face),
            SS_UNKNOWN,
        )?;
        adj_off_surf.set_allow_extension(true);

        // If the base part is the rollover part, set its type before call to split:
        // makes a difference inside that routine.
        if which_end == 0 {
            // Original FG becomes rollover, on adjacent face.
            pseudo
                .geom_mut(fillet_geom)?
                .set_fillet_geom_type(FilletGeomType::TangentRollover);
        }

        // Do the split (default eNewGeomType = SM_FG_DEFAULT; the geometry's solver is
        // this one).
        let solver_id = pseudo.geom(fillet_geom)?.fillet_solver().ok_or_else(nul)?;
        let new_fillet_geom = pseudo.topology_split(
            k,
            fillet_geom,
            FilletGeomType::Default,
            solver_id,
            self,
            b,
            corners,
        )?;

        // The new fillet geom comes out as the 'high end'. Update offset surfaces,
        // orig face, and type in the two FG's. Copy offset surface from non-split side
        // of old to new.
        // FG::SetOffsetSurface() deletes what's there; its status is not checked
        let off_surf = geom_offset_surface_copy(k, self, pseudo, fillet_geom, other_idx)?;
        ignore_status(
            pseudo
                .geom_mut(new_fillet_geom)?
                .set_offset_surface(other_idx, off_surf),
        )?;

        if which_end == 0 {
            let rail = pseudo
                .geom(fillet_geom)?
                .rail(rail_idx)
                .ok_or_else(|| invalid("fillet geometry without rail"))?;
            pseudo
                .fillet_edge_mut(rail)?
                .set_original_face(Some(adj_face));

            let off_surf = geom_offset_surface_copy(k, self, pseudo, fillet_geom, rail_idx)?;
            ignore_status(
                pseudo
                    .geom_mut(new_fillet_geom)?
                    .set_offset_surface(rail_idx, off_surf),
            )?;
            ignore_status(
                pseudo
                    .geom_mut(fillet_geom)?
                    .set_offset_surface(rail_idx, Some(adj_off_surf)),
            )?;
        } else {
            // New FG becomes rollover, on adjacent face.
            pseudo
                .geom_mut(new_fillet_geom)?
                .set_fillet_geom_type(FilletGeomType::TangentRollover);
            let rail = pseudo
                .geom(new_fillet_geom)?
                .rail(rail_idx)
                .ok_or_else(|| invalid("fillet geometry without rail"))?;
            pseudo
                .fillet_edge_mut(rail)?
                .set_original_face(Some(adj_face));

            ignore_status(
                pseudo
                    .geom_mut(new_fillet_geom)?
                    .set_offset_surface(rail_idx, Some(adj_off_surf)),
            )?;
        }
        self.fillet_geoms.push(new_fillet_geom);

        // Grab the two new Vertices. TopologySplit() leaves the new ones in the
        // middle, which is the high end of the original FG.
        let mut new_v: [Option<VertexId>; 2] = [None, None];
        for (kk, slot) in new_v.iter_mut().enumerate() {
            let this_rail = pseudo
                .geom(fillet_geom)?
                .rail(kk)
                .ok_or_else(|| invalid("fillet geometry without rail"))?;
            let v = edge::vertex(pseudo.brep(), this_rail)?;
            *slot = edge::other_vertex(pseudo.brep(), this_rail, v)?;
        }
        let missing = || invalid("rail without other vertex");
        let [v0, v1] = new_v;
        let (v0, v1) = (v0.ok_or_else(missing)?, v1.ok_or_else(missing)?);
        let (split_v, opp_v) = if rail_idx == 0 { (v0, v1) } else { (v1, v0) };

        // Set them as each others' mates.
        pseudo.fillet_vertex_mut(v0)?.set_mate(0, Some(v1));
        pseudo.fillet_vertex_mut(v1)?.set_mate(0, Some(v0));

        // The new vertex on the split rail is now the vertex on the side edge, where
        // the input vertex was. Move the geometry from the original split vertex into
        // the new one.
        let point = pseudo.vertex_point(fil_vtx)?;
        pseudo.set_vertex_point(split_v, point)?;
        let tol = vertex::tolerance(pseudo.brep(), fil_vtx)?;
        vertex::set_tolerance(pseudo.brep_mut(), split_v, tol, false, true)?;
        let vertex_type = pseudo.fillet_vertex(fil_vtx)?.fillet_vertex_type();
        let split_fv = pseudo.fillet_vertex_mut(split_v)?;
        split_fv.set_fillet_vertex_type(vertex_type);
        split_fv.set_status(FilStatus::Unprocessed);

        // Find the Filleted Edgeuse on the same face as the rail curve.
        if closed_side_face {
            side_eu =
                eu::radial(b, side_eu)?.ok_or_else(|| invalid("side edge use without radial"))?;
        } else {
            let filleted_eu = self
                .edgeuse(rail_idx)?
                .ok_or_else(|| invalid("no filleted edge use"))?;
            let rail_face = eu::face(b, filleted_eu)?;
            if eu::face(b, side_eu)? != rail_face {
                side_eu = eu::radial(b, side_eu)?
                    .ok_or_else(|| invalid("side edge use without radial"))?;
            }
        }
        let point_class = pseudo.fillet_vertex(fil_vtx)?.point_class();
        pseudo
            .fillet_vertex_mut(split_v)?
            .set_point_class(point_class, Some(EntityRef::Edgeuse(side_eu)));

        // The opposite new vertex is now this new vertex's mate.
        pseudo
            .fillet_vertex_mut(opp_v)?
            .set_fillet_vertex_type(FilletVertexType::Mate);

        // The original split-vertex, and its mate, are no longer valid.
        pseudo
            .fillet_vertex_mut(fil_vtx)?
            .set_status(FilStatus::Unprocessed);
        let opp_fv = if which_end == 0 {
            pseudo.rail_vertex(fillet_geom, 0, other_idx)?
        } else {
            pseudo.rail_vertex(new_fillet_geom, 1, other_idx)?
        };
        pseudo
            .fillet_vertex_mut(opp_fv)?
            .set_status(FilStatus::Unprocessed);
        Ok(new_fillet_geom)
    }

    /// `SplitFilletGeom(lFilletGeomIndex, eTrimType, crIvls1, crIvls2, crCC1, crCC2)`:
    /// one fillet geometry per interval pair (copies of the geometry for all but the
    /// last), each with rail points at its interval ends, end curves and trimmed rails.
    /// Empty interval arrays do nothing. `geometry` resolves the classifications'
    /// objects; `pseudo` is the executive's pseudo-B-rep (`m_pExecutive->m_pPseudoBrep`)
    /// holding the geometries, their rails and vertices.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for a geometry index past the solver's geometries or
    /// fewer second intervals than first ones (upstream only asserts equal sizes, then
    /// reads `crIvls2[i]` for every `i` of `crIvls1`); [`SolidError::NotFound`] for a
    /// vertex without its fillet record; those of [`FilletBrep::rail_vertex`],
    /// `FindPointClass`, [`FilletBrep::set_up_rail_point`], the end curves
    /// ([`FilletBrep::create_end_curve_with_solver`], reading rails 0 and 1's base
    /// B-reps `bases`) and [`trim_rail`].
    #[allow(clippy::too_many_arguments)]
    pub fn split_fillet_geom(
        &mut self,
        k: &Kernel,
        pseudo: &mut FilletBrep,
        geometry: &impl ClassGeometry<EntityRef>,
        fillet_geom_index: usize,
        trim_type: i32,
        ivls1: &[Extent1d],
        ivls2: &[Extent1d],
        cc1: &CurveClassification<EntityRef>,
        cc2: &CurveClassification<EntityRef>,
        bases: [&Brep; 2],
    ) -> Result<()> {
        let cc1 = cc1.map_objects(&mut |o| Ok(FilletClassObject::Legacy(o)))?;
        let cc2 = cc2.map_objects(&mut |o| Ok(FilletClassObject::Legacy(o)))?;
        self.split_fillet_geom_with_target(
            k,
            pseudo,
            &LegacyClassificationObjects(geometry),
            fillet_geom_index,
            trim_type,
            ivls1,
            ivls2,
            &cc1,
            &cc2,
            bases,
        )
    }

    /// [`Self::split_fillet_geom`] with retained body-qualified rail classifications.
    /// `bases` are the base B-reps of rails 0 and 1 the end curves read
    /// (`CreateEndCurve`, which for `SM_BT_BLEND` reads each rail end vertex's
    /// classification edge in its rail's base B-rep; upstream reaches them through the
    /// vertices' pointers).
    ///
    /// # Errors
    /// As [`Self::split_fillet_geom`], or a non-target point classification.
    #[allow(clippy::too_many_arguments)]
    pub fn split_fillet_geom_with_target(
        &mut self,
        k: &Kernel,
        pseudo: &mut FilletBrep,
        geometry: &impl ClassGeometry<FilletClassObject>,
        fillet_geom_index: usize,
        trim_type: i32,
        ivls1: &[Extent1d],
        ivls2: &[Extent1d],
        cc1: &CurveClassification<FilletClassObject>,
        cc2: &CurveClassification<FilletClassObject>,
        bases: [&Brep; 2],
    ) -> Result<()> {
        self.split_fillet_geom_in(
            k,
            pseudo,
            geometry,
            fillet_geom_index,
            trim_type,
            ivls1,
            ivls2,
            cc1,
            cc2,
            bases,
        )
    }

    /// `new SmFilletGeom(pFilletGeom)` (the copy constructor) for this solver's
    /// geometry `other` of `pseudo`: a geometry of the same solver without offset
    /// surfaces, two new rail edges (`MakeRailEdge`), `Copy(pFilletGeom)` (both
    /// statuses ignored, `SE`), then `other`'s type. The new geometry's id.
    ///
    /// # Errors
    /// [`SolidError::NotFound`] for `other` not in `pseudo`; [`SolidError::Unported`]
    /// from the rail edges or the copy.
    fn copy_constructed_geom(
        &self,
        k: &Kernel,
        pseudo: &mut FilletBrep,
        other: FilletGeomId,
    ) -> Result<FilletGeomId> {
        let (solver, geom_type) = {
            let o = pseudo.geom(other)?;
            (o.fillet_solver(), o.fillet_geom_type())
        };
        let mut geom = FilletGeom::new(k, None, geom_type)?;
        geom.solver = solver;
        let copy = pseudo.add_geom(geom);
        for i in 0..2 {
            ignore_status(pseudo.make_rail_edge(k, copy, i).map(|_| ()))?;
        }
        ignore_status(pseudo.copy_geom_with_solver(k, copy, other, self))?;
        pseudo.geom_mut(copy)?.set_fillet_geom_type(geom_type);
        Ok(copy)
    }

    #[allow(clippy::too_many_arguments)]
    fn split_fillet_geom_in(
        &mut self,
        k: &Kernel,
        pseudo: &mut FilletBrep,
        geometry: &impl ClassGeometry<FilletClassObject>,
        fillet_geom_index: usize,
        trim_type: i32,
        ivls1: &[Extent1d],
        ivls2: &[Extent1d],
        cc1: &CurveClassification<FilletClassObject>,
        cc2: &CurveClassification<FilletClassObject>,
        bases: [&Brep; 2],
    ) -> Result<()> {
        // SM_ASSERT(sizes equal) is a no-op; a shorter crIvls2 is read past its end.
        if ivls2.len() < ivls1.len() {
            return Err(invalid("fewer second intervals than first ones"));
        }
        let fillet_geom = *self
            .fillet_geoms
            .get(fillet_geom_index)
            .ok_or_else(|| invalid("fillet geometry index out of range"))?;

        let n = ivls1.len();
        let mut fillet_geoms = Vec::new();
        for ii in 0..n {
            // Create a new Fillet Geometry object by copying old one.
            let new_fillet_geom = if ii + 1 < n {
                let copy = self.copy_constructed_geom(k, pseudo, fillet_geom)?;
                self.fillet_geoms.push(copy);
                copy
            } else {
                fillet_geom
            };
            fillet_geoms.push(new_fillet_geom);
        }

        for (i, (new_fillet_geom, (ivl1, ivl2))) in fillet_geoms
            .iter()
            .copied()
            .zip(ivls1.iter().zip(ivls2.iter()))
            .enumerate()
        {
            // If we split things then set the processed flag for interior vertexuses
            // to FALSE.
            let v00 = pseudo.rail_vertex(fillet_geom, 0, 0)?;
            let v01 = pseudo.rail_vertex(fillet_geom, 0, 1)?;
            let v10 = pseudo.rail_vertex(fillet_geom, 1, 0)?;
            let v11 = pseudo.rail_vertex(fillet_geom, 1, 1)?;
            if i > 0 {
                for v in [v00, v10] {
                    pseudo
                        .fillet_vertex_mut(v)?
                        .set_status(FilStatus::Unprocessed);
                }
            }
            if i + 1 < n {
                for v in [v01, v11] {
                    pseudo
                        .fillet_vertex_mut(v)?
                        .set_status(FilStatus::Unprocessed);
                }
            }

            // Found classifications of each interval
            let pc00 = cc1.find_point_class(k, geometry, ivl1.min(), 0.0)?;
            let pc01 = cc1.find_point_class(k, geometry, ivl1.max(), 0.0)?;
            let pc10 = cc2.find_point_class(k, geometry, ivl2.min(), 0.0)?;
            let pc11 = cc2.find_point_class(k, geometry, ivl2.max(), 0.0)?;

            // Create the fillet points using the rail and parameter value
            for (rail, end, t, pc) in [
                (0usize, 0usize, ivl1.min(), pc00),
                (0, 1, ivl1.max(), pc01),
                (1, 0, ivl2.min(), pc10),
                (1, 1, ivl2.max(), pc11),
            ] {
                let pc = pc.map_objects(&mut FilletClassObject::target_entity)?;
                pseudo.set_up_rail_point(k, new_fillet_geom, rail, end, t, Some(&pc))?;
            }

            // Create the ending curve on the fillet
            for end in 0..2usize {
                // (SmBoundaryTrimmingType)lTrimType: only SM_BT_BLEND is tested
                let end_trim = match trim_type {
                    1 => BoundaryTrimmingType::Minimal,
                    2 => BoundaryTrimmingType::Maximal,
                    3 => BoundaryTrimmingType::Bevel,
                    4 => BoundaryTrimmingType::Blend,
                    _ => BoundaryTrimmingType::None,
                };
                let solver_id = pseudo
                    .geom(new_fillet_geom)?
                    .fillet_solver()
                    .ok_or_else(|| SolidError::null_pointer(OP))?;
                pseudo.create_end_curve_with_solver(
                    k,
                    new_fillet_geom,
                    end,
                    end_trim,
                    solver_id,
                    self,
                    bases,
                )?;
            }

            // Trim the rail and that should give us a nice set of boundary for the
            // creation of topology.
            for (rail, ivl) in [(0usize, ivl1), (1, ivl2)] {
                let rail_edge = pseudo
                    .geom(new_fillet_geom)?
                    .rail(rail)
                    .ok_or_else(|| invalid("fillet geometry without rail"))?;
                trim_rail(k, pseudo.brep_mut(), rail_edge, ivl)?;
            }
        }
        Ok(())
    }

    /// `TrimRails`: trims every fillet geometry's rail curves to its processed fillet
    /// vertices, in native order, using the actual borrowed solver.
    ///
    /// # Errors
    /// The first trimming error, retaining mutations of earlier geometries.
    pub fn trim_rails_with(
        &self,
        mut trim: impl FnMut(FilletGeomId, &Self) -> Result<()>,
    ) -> Result<()> {
        for &fillet_geom in &self.fillet_geoms {
            trim(fillet_geom, self)?;
        }
        Ok(())
    }
}

/// An `SmBoolean` query whose failed evaluation upstream returns the status, read as
/// `TRUE` (`SER` inside a boolean function); [`SolidError::Unported`] propagates.
fn bool_status(r: Result<bool>) -> Result<bool> {
    match r {
        Ok(v) => Ok(v),
        Err(e @ SolidError::Unported { .. }) => Err(e),
        Err(_) => Ok(true),
    }
}

/// [`bool_status`] for `IsSingularity`: a failure reads as singular, direction
/// unknown (the V branch).
fn singularity_status(
    r: Result<crate::surface::surface::p5::Singularity>,
) -> Result<crate::surface::surface::p5::Singularity> {
    match r {
        Ok(v) => Ok(v),
        Err(e @ SolidError::Unported { .. }) => Err(e),
        Err(_) => Ok(crate::surface::surface::p5::Singularity {
            singular: true,
            direction: SurfParamType::Neither,
        }),
    }
}

fn vertex_of_edgeuse(b: &Brep, e: EdgeuseId) -> Result<Option<VertexId>> {
    let vu = eu::vertexuse(b, e)?.ok_or_else(|| invalid("edge use without vertex use"))?;
    vertexuse::vertex(b, vu)
}

// ---- virtual methods ---------------------------------------------------------------------

/// The solver as a `dyn` [`FilletSolverOps`]: lets a default method hand `self` to the
/// base functions that take the virtuals as a trait object (implemented for every
/// solver).
pub trait AsFilletSolverOps {
    /// `self` as a trait object.
    fn as_solver_ops(&mut self) -> &mut dyn FilletSolverOps;
}

impl<T: FilletSolverOps> AsFilletSolverOps for T {
    fn as_solver_ops(&mut self) -> &mut dyn FilletSolverOps {
        self
    }
}

/// The executive's `GetFilletCornerOfVertex`, dereferenced, as
/// `SmFilletSolver::SetExtensionFlags` reads it.
pub trait ExtensionCorners {
    /// The corner of target vertex `vertex`.
    ///
    /// # Errors
    /// `SM_ERR_NULL_POINTER` for a vertex without corner (upstream dereferences the
    /// `NULL` corner).
    fn corner_of(&self, vertex: Option<VertexId>) -> Result<&FilletCorner>;
}

/// A caller without an executive's corners (a standalone entry point): every vertex's
/// corner is `NULL`.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoExtensionCorners;

impl ExtensionCorners for NoExtensionCorners {
    fn corner_of(&self, _vertex: Option<VertexId>) -> Result<&FilletCorner> {
        Err(SolidError::null_pointer(
            "blend::fillet_solver::p1::set_extension_flags",
        ))
    }
}

/// The virtual methods of `SmFilletSolver` (the base versions are the default methods)
/// with access to the base data.
pub trait FilletSolverOps: AsFilletSolverOps {
    /// The `SmFilletSolver` base.
    fn solver(&self) -> &FilletSolver;

    /// The `SmFilletSolver` base, mutable.
    fn solver_mut(&mut self) -> &mut FilletSolver;

    /// The selected offset surface; contexts can resolve an actual fillet geometry.
    fn solver_surface(&self, rail: usize) -> Result<Option<&OffsetSurface>> {
        Ok(self.solver().surface(rail)?.map(|slot| &slot.surface))
    }

    /// The same selected offset object for an in-place mutation.
    fn solver_surface_mut(&mut self, rail: usize) -> Result<Option<&mut OffsetSurface>> {
        Ok(self
            .solver_mut()
            .surface_mut(rail)?
            .map(|slot| &mut slot.surface))
    }

    /// `SmFilletSolver::RailEdgeuseIntersect` while `geometry` is the geometry for the
    /// surfaces (`SmFilletGeom::RailEdgeuseIntersect`): the caller has lent the
    /// geometry's offset surfaces to the solver, so the default reads them through
    /// `GetSurface`; solvers that also need the geometry itself (the tangent-rollover
    /// guesses) override it.
    #[allow(clippy::too_many_arguments)]
    fn rail_edgeuse_intersect_on_geometry(
        &mut self,
        k: &Kernel,
        _geometry: &mut FilletGeom,
        b: &mut Brep,
        rail_index: usize,
        edgeuse: EdgeuseId,
        guess: f64,
        tsect: &mut TsectPnt,
    ) -> Result<Option<f64>> {
        FilletSolver::rail_edgeuse_intersect(
            self.as_solver_ops(),
            k,
            b,
            rail_index,
            edgeuse,
            guess,
            tsect,
        )
    }

    /// `GetJacobianSize`: the base signals `SM_ERR` ("Called pure virtual method") and
    /// returns 0.
    ///
    /// # Errors
    ///
    /// None for the base; [`SolidError::Unported`] for an override of an unported unit.
    fn jacobian_size(&self) -> Result<usize> {
        Ok(0)
    }

    /// `SetupOffsetValues(rTsectPnt, pOffsetDist)`: the base returns `SM_ERR`.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for the base.
    fn setup_offset_values(
        &mut self,
        _tsect: &TsectPnt,
        _offset_dist: Option<&mut f64>,
    ) -> Result<()> {
        Err(SolidError::sm_err(OP))
    }

    /// `OffsetRadiiCanChange`: `TRUE` for the base.
    ///
    /// # Errors
    ///
    /// None for the base; [`SolidError::Unported`] for an override of an unported unit.
    fn offset_radii_can_change(&self) -> Result<bool> {
        Ok(true)
    }

    /// `SetupOffsetExtension(pSurf, rDomain)` (virtual): the base is
    /// [`FilletSolver::setup_offset_extension`]. `SmSurfaceSurfaceFS` overrides it (unit
    /// `blend.fillet_standard_solver_p1`); a solver of that class implements this hook.
    ///
    /// # Errors
    ///
    /// Those of [`FilletSolver::setup_offset_extension`]; [`SolidError::Unported`] for
    /// an override of an unported unit.
    fn setup_offset_extension(
        &self,
        k: &Kernel,
        surface: &Surface,
        domain: &mut Extent2d,
    ) -> Result<()> {
        self.solver().setup_offset_extension(k, surface, domain)
    }

    /// `CanMakeAnalytic`: `!OffsetRadiiCanChange()` for the base.
    ///
    /// # Errors
    ///
    /// Those of [`FilletSolverOps::offset_radii_can_change`].
    fn can_make_analytic(&self) -> Result<bool> {
        Ok(!self.offset_radii_can_change()?)
    }

    /// `LoadJacobian`: the base returns `SM_ERR` and touches nothing.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for the base.
    fn load_jacobian(
        &mut self,
        _k: &Kernel,
        _num_equations: &mut usize,
        _num_parameters: &mut usize,
        _load: &mut JacobianLoad<'_>,
        _f: &mut [f64],
        _jacobian: Option<&mut Matrix>,
    ) -> Result<bool> {
        Err(SolidError::sm_err(OP))
    }

    /// `LoadInitialValues(rbDoSurf2Calcs, lRailIndex, crSurface1, lSurface1Index,
    /// rpSurface2, rlSurface2Index, rExtents, rPeriodicities, rGuessT)`: the base
    /// returns `SM_ERR` and touches nothing.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for the base.
    fn load_initial_values(
        &mut self,
        _k: &Kernel,
        _brep: Option<&mut Brep>,
        _do_surf2_calcs: bool,
        _rail_index: usize,
        _surface1: &mut OffsetSurface,
        _surface1_index: usize,
        _intervals: &mut ExtentNd,
        _periodicities: &mut Vec<bool>,
        _guess_t: &mut Vec<f64>,
    ) -> Result<Option<InitialValues>> {
        Err(SolidError::sm_err(OP))
    }

    /// `SetExtensionFlags` (virtual): the base version
    /// [`FilletSolver::set_extension_flags`] on `b` with the executive's corners
    /// `corners` (`SmVariableRadiusFS` also extends its edge curve, with `k`).
    ///
    /// # Errors
    ///
    /// Those of [`FilletSolver::set_extension_flags`].
    fn set_extension_flags(
        &mut self,
        _k: &Kernel,
        b: &Brep,
        corners: &dyn ExtensionCorners,
    ) -> Result<()> {
        self.solver_mut().set_extension_flags(b, corners)
    }

    /// `GetType` (virtual): `SmFilletSolver_TYPE` for the base (each derived solver's
    /// `SM_COMMON` type belongs to its unit).
    ///
    /// # Errors
    ///
    /// None for the base; [`SolidError::Unported`] for an override of an unported unit.
    fn type_code(&self) -> Result<i64> {
        Ok(FilletSolver::TYPE)
    }

    /// `IsKindOf` (virtual): the base's `SmFilletSolver_TYPE` / `SmObject_TYPE`.
    ///
    /// # Errors
    ///
    /// None for the base; [`SolidError::Unported`] for an override of an unported unit.
    fn is_kind_of(&self, t: i64) -> Result<bool> {
        Ok(FilletSolver::is_kind_of(t))
    }

    /// `GetSolverType` (pure virtual upstream; each derived solver's override belongs
    /// to its unit). The default stands for a derived solver that names no kind
    /// (`SM_FS_UNKNOWN`). Added by `blend.fillet_solver_p3` (`StepSolve` branches on it).
    ///
    /// # Errors
    ///
    /// None for the default; [`SolidError::Unported`] for an override of an unported
    /// unit.
    fn solver_type(&self) -> Result<FilletSolverType> {
        Ok(FilletSolverType::Unknown)
    }

    /// `SM_CAST_PTR(SmVariableRadiusFS, this)`: mutable borrow when the concrete solver
    /// is a [`VariableRadiusFs`](crate::blend::fillet_standard_solver::p1::VariableRadiusFs).
    fn variable_radius_mut(
        &mut self,
    ) -> Option<&mut crate::blend::fillet_standard_solver::p1::VariableRadiusFs> {
        None
    }

    /// `((SmConstantDistanceFS*)this)->GetDistance()`: the rail distance of a
    /// constant-distance solver, read by `SmFindClippedRadiusEFO` (a cast upstream
    /// makes only for a [`FilletSolverType::ConstDist`] solver). Added by
    /// `blend.fillet_solver_p3`.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] by default (a solver of another class, which
    /// upstream would cast to `SmConstantDistanceFS`).
    fn constant_distance(&self) -> Result<f64> {
        // the cast is only made for a solver whose GetSolverType() is
        // SM_FST_CONST_DIST, which only SmConstantDistanceFS (its override) reports
        Err(invalid(
            "(SmConstantDistanceFS*) cast of a solver of another class (undefined upstream)",
        ))
    }

    /// `PointOnPlaneSolve(crPlaneOrig, crPlaneNormal, crUVDomain1, crUVDomain2, crUV1,
    /// crUV2, rbFoundSolution, rTsectPnt)`: the rail points on the plane at the fillet
    /// radius from its origin, written to `point`; the result is `rbFoundSolution`.
    /// Virtual upstream (`SmFilletSolver.h:258`) and overridden by
    /// `SmConstantRadiusAssistedFS` (unit `blend.fillet_standard_solver_p2`), so
    /// `StepSolve` and `sm_FindFilletTsectPnt` dispatch through it. Added by
    /// `blend.fillet_solver_p3`.
    ///
    /// # Errors
    ///
    /// Those of the base `SmFilletSolver::PointOnPlaneSolve`
    /// ([`FilletSolver::point_on_plane_solve`], unit `blend.fillet_solver_p2`). `brep`
    /// is the target B-rep holding the solver's edge uses (upstream reaches them through
    /// its `GetEdgeuse` pointers), for `LoadInitialValues`; `None` when the caller has
    /// none.
    #[allow(clippy::too_many_arguments)]
    fn point_on_plane_solve(
        &mut self,
        k: &Kernel,
        brep: Option<&mut Brep>,
        plane_origin: Point3,
        plane_normal: Vec3,
        uv_domain1: &Extent2d,
        uv_domain2: &Extent2d,
        uv1: Point2,
        uv2: Point2,
        point: &mut TsectPnt,
    ) -> Result<bool> {
        FilletSolver::point_on_plane_solve(
            self.as_solver_ops(),
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

impl FilletSolverOps for FilletSolver {
    fn solver(&self) -> &FilletSolver {
        self
    }

    fn solver_mut(&mut self) -> &mut FilletSolver {
        self
    }
}

/// `GetSurface(rail)->GetBaseSurface()` as the identity `LoadJacobian` compares.
fn base_id(s: &FilletSolver, rail: usize) -> Result<Option<SurfaceId>> {
    Ok(s.surface(rail)?.map(|slot| slot.surface.base_id))
}

/// `pOffSurf->Copy(ctx, pSurf)` with its status ignored, as upstream: `None` for no
/// surface or a failed copy (`pSurf` stays `NULL`). The copy shares the base surface
/// (the solvers' offsets do not own theirs), so it keeps the base identity.
fn copy_offset_surface(k: &Kernel, s: Option<&OffsetSurfaceSlot>) -> Result<Option<OffsetSurface>> {
    let Some(slot) = s else {
        return Ok(None);
    };
    Ok(
        ignored(slot.surface.surface.copy(k))?.map(|surface| OffsetSurface {
            surface,
            base_id: slot.surface.base_id,
        }),
    )
}

/// `pFG->GetOffsetSurface(rail)` copied with `Copy(m_crContext, pSurf)` (status
/// ignored): `None` for no surface or a failed copy. The geometry's own surface, else
/// the solver's (`solver`, the geometry's `GetFilletSolver()`).
fn geom_offset_surface_copy(
    k: &Kernel,
    solver: &FilletSolver,
    pseudo: &FilletBrep,
    fillet_geom: FilletGeomId,
    rail: usize,
) -> Result<Option<OffsetSurface>> {
    let Some(orig) = pseudo
        .geom(fillet_geom)?
        .offset_surface(rail, Some(solver))?
    else {
        return Ok(None);
    };
    Ok(ignored(orig.surface.copy(k))?.map(|surface| OffsetSurface {
        surface,
        base_id: orig.base_id,
    }))
}

/// `Evaluate1stDerivatives(uv, TRUE, TRUE, ..)` of an offset surface (the inherited
/// `SmSurface` body over `SmOffsetSurface::Evaluate`); an extension of the base it
/// makes is kept, as upstream's `const` evaluators do.
fn offset_evaluate_1st(
    k: &Kernel,
    s: &mut OffsetSurface,
    uv: Point2,
) -> Result<(Point3, Vec3, Vec3)> {
    s.surface
        .with_evaluator(|e| evaluate_1st_derivatives(k, e, uv, Side::Left, Side::Left))
}

/// `EvaluateNormal(uv, TRUE, TRUE, ..)` of an offset surface
/// (`SmOffsetSurface::EvaluateNormal`), keeping an extension it makes.
fn offset_evaluate_normal(k: &Kernel, s: &mut OffsetSurface, uv: Point2) -> Result<Vec3> {
    s.surface
        .evaluate_normal_extending(k, uv, Side::Left, Side::Left)
}

/// The solver's offset surface of `rail` (`GetSurface(rail)`), for evaluation.
fn solver_surface_mut(s: &mut FilletSolver, rail: usize) -> Result<&mut OffsetSurface> {
    s.surface_mut(rail)?
        .map(|slot| &mut slot.surface)
        .ok_or_else(|| invalid("no offset surface"))
}

fn x_at(x: &[f64], i: usize) -> Result<f64> {
    x.get(i)
        .copied()
        .ok_or_else(|| invalid("crX index out of range"))
}

fn set_f(f: &mut [f64], i: usize, v: f64) -> Result<()> {
    let slot = f
        .get_mut(i)
        .ok_or_else(|| invalid("rF index out of range"))?;
    *slot = v;
    Ok(())
}

fn f_at(f: &[f64], i: usize) -> Result<f64> {
    f.get(i)
        .copied()
        .ok_or_else(|| invalid("rF index out of range"))
}

fn set_j(j: &mut Matrix, r: usize, c: usize, v: f64) -> Result<()> {
    j.set_at(r, c, v)
        .map_err(|_| invalid("Jacobian index out of range"))
}

/// Zeroes the Jacobian and the matching `rF` rows, or all of `rF` without a Jacobian.
fn zero_outputs(f: &mut [f64], jacobian: Option<&mut Matrix>) -> Result<()> {
    if let Some(j) = jacobian {
        for ll in 0..j.num_rows() {
            for mm in 0..j.num_columns() {
                set_j(j, ll, mm, 0.0)?;
            }
            set_f(f, ll, 0.0)?;
        }
    } else {
        for v in f.iter_mut() {
            *v = 0.0;
        }
    }
    Ok(())
}

// ---- SmFilletSphereSolveENFO -----------------------------------------------------------

/// `SmFilletSphereSolveENFO`: the solver's equations plus the distance of the first
/// surface point to a plane (the obsolete sphere centre and radius seed the plane).
pub struct FilletSphereSolveEnfo<'a> {
    /// `m_vSphereCenter`.
    pub sphere_center: Point3,
    /// `m_dSphereRadius`.
    pub sphere_radius: f64,
    /// `m_vPlaneABC`: plane normal.
    pub plane_abc: Vec3,
    /// `m_dPlaneD`: plane offset.
    pub plane_d: f64,
    /// `m_bPlaneAdjusts`: recompute the plane from the current solution.
    pub plane_adjusts: bool,
    solver: &'a mut dyn FilletSolverOps,
}

impl<'a> FilletSphereSolveEnfo<'a> {
    /// `m_crFilletSolver`.
    #[must_use]
    pub fn solver(&self) -> &dyn FilletSolverOps {
        &*self.solver
    }

    /// `m_crFilletSolver`, for the virtuals that write to the solver.
    pub fn solver_mut(&mut self) -> &mut dyn FilletSolverOps {
        &mut *self.solver
    }

    /// `SmFilletSphereSolveENFO(crSphereCenter, dSphereRadius, crFilletSolver)`: the
    /// plane starts as `(centre, radius)`, not adjusting.
    pub fn new(
        sphere_center: Point3,
        sphere_radius: f64,
        solver: &'a mut dyn FilletSolverOps,
    ) -> Self {
        Self {
            sphere_center,
            sphere_radius,
            plane_abc: sphere_center,
            plane_d: sphere_radius,
            plane_adjusts: false,
            solver,
        }
    }

    /// `SetPlaneAdjusts`.
    pub fn set_plane_adjusts(&mut self, plane_adjusts: bool) {
        self.plane_adjusts = plane_adjusts;
    }

    /// `Evaluate(crX, rF, pOptJacobian, rbFoundAnswer)`: zeroes the outputs, lets the
    /// solver load every row but the first (`LoadJacobian` from row 1, parameter 2),
    /// then sets row 0 to the plane distance of the first offset surface's point at
    /// `crX[0..2]`. Returns `rbFoundAnswer`.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when `LoadJacobian` fails; those of the evaluations;
    /// [`SolidError::InvalidInput`] for arrays too short (upstream reads or writes past
    /// them) or a missing first surface.
    pub fn evaluate(
        &mut self,
        k: &Kernel,
        x: &[f64],
        f: &mut [f64],
        mut jacobian: Option<&mut Matrix>,
    ) -> Result<bool> {
        const OP_E: &str = "blend::fillet_solver::p1::FilletSphereSolveEnfo::evaluate";
        if let Some(j) = jacobian.as_deref() {
            if f.len() < j.num_rows() {
                return Err(invalid("rF shorter than the Jacobian"));
            }
        }
        zero_outputs(f, jacobian.as_deref_mut())?;

        let mut num_equations = 1;
        let mut num_parameters = 2;
        let surface1 = base_id(self.solver.solver(), 0)?;
        let mut load = JacobianLoad {
            rail_index: 0,
            surface1,
            surf1_offset: 0,
            surface2: None,
            surf2_offset: 0,
            x,
        };
        let found_answer = self
            .solver
            .load_jacobian(
                k,
                &mut num_equations,
                &mut num_parameters,
                &mut load,
                f,
                jacobian.as_deref_mut(),
            )
            .map_err(|e| as_sm_err(e, OP_E))?;
        // rlSurf2Offset as LoadJacobian left it
        let surf2_offset = load.surf2_offset;

        // calc OffsetSurface0 current solution surface values
        let uv = Point2::new(x_at(x, 0)?, x_at(x, 1)?);
        let (sf, sduf, sdvf) =
            offset_evaluate_1st(k, solver_surface_mut(self.solver.solver_mut(), 0)?, uv)?;

        // update the plane to fit the current solution
        if self.plane_adjusts {
            let mut norm1 = sduf.cross(sdvf);
            if norm1.length_squared() < EFF_ZERO_SQ {
                norm1 = offset_evaluate_normal(
                    k,
                    solver_surface_mut(self.solver.solver_mut(), 0)?,
                    uv,
                )?;
            }
            let norm1 = norm1.unitize()?;

            let uv2 = Point2::new(x_at(x, surf2_offset)?, x_at(x, surf2_offset + 1)?);
            let (sg, sdug, sdvg) =
                offset_evaluate_1st(k, solver_surface_mut(self.solver.solver_mut(), 1)?, uv2)?;
            let mut norm2 = sdug.cross(sdvg);
            if norm2.length_squared() < EFF_ZERO_SQ {
                norm2 = offset_evaluate_normal(
                    k,
                    solver_surface_mut(self.solver.solver_mut(), 1)?,
                    uv2,
                )?;
            }
            let norm2 = norm2.unitize()?;

            let plane_orig = (sf + sg) / 2.0;
            let mut plane_abc = norm1.cross(norm2);

            // preserve plane normal orientation
            if plane_abc.dot(self.plane_abc) < 0.0 {
                plane_abc = -plane_abc;
            }
            self.plane_abc = plane_abc;
            self.plane_d = -self.plane_abc.dot(plane_orig);
        }

        // set rF 1st row
        set_f(f, 0, self.plane_abc.dot(sf) + self.plane_d)?;
        if let Some(j) = jacobian {
            set_j(j, 0, 0, self.plane_abc.dot(sduf))?;
            set_j(j, 0, 1, self.plane_abc.dot(sdvf))?;
        }

        let mut found = false;
        if found_answer {
            let scaled_tol = self.solver.solver().conversion_tol() * (1.0 + sf.max_dimension());
            if f_at(f, 0)?.abs() < scaled_tol {
                found = true;
            }
        }
        Ok(found)
    }
}

// ---- SmRailUVCurveIntersectENFO -------------------------------------------------------

/// `SmRailUVCurveIntersectENFO`: a fillet rail meeting a UV curve on one of the
/// surfaces; `crX = [t u v ...]` with `t` on the UV curve and `(u, v)` the rail's UV
/// point.
pub struct RailUvCurveIntersectEnfo<'a> {
    /// `m_crUVCurve` (an `SmBSplineCurve&`; `Evaluate` is virtual, so any
    /// `SmBSplineCurve`-derived kind such as an `SmLine` dispatches on its own kind).
    pub uv_curve: &'a Curve,
    /// `m_vInterval`.
    pub interval: Extent1d,
    /// `m_lRailIndex`.
    pub rail_index: usize,
    solver: &'a mut dyn FilletSolverOps,
}

impl<'a> RailUvCurveIntersectEnfo<'a> {
    /// `SmRailUVCurveIntersectENFO(crUVCurve, crInterval, crSurfaceOfCurve, lRailIndex,
    /// crFilletSolver)` (the surface of the curve is only drawn upstream).
    pub fn new(
        uv_curve: &'a Curve,
        interval: Extent1d,
        rail_index: usize,
        solver: &'a mut dyn FilletSolverOps,
    ) -> Self {
        Self {
            uv_curve,
            interval,
            rail_index,
            solver,
        }
    }

    /// `Evaluate(crX, rF, pOptJacobian, rbFoundAnswer)`: rows 0 and 1 are the UV curve
    /// point at `crX[0]` minus `crX[1..3]`, the other rows (and Jacobian rows) are
    /// zeroed when a Jacobian is given, then the solver loads its rows from row 2,
    /// parameter 3. Returns `rbFoundAnswer`.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when `LoadJacobian` fails; those of the curve evaluation;
    /// [`SolidError::InvalidInput`] for arrays too short.
    pub fn evaluate(
        &mut self,
        k: &Kernel,
        x: &[f64],
        f: &mut [f64],
        mut jacobian: Option<&mut Matrix>,
    ) -> Result<bool> {
        const OP_E: &str = "blend::fillet_solver::p1::RailUvCurveIntersectEnfo::evaluate";
        let pv = self.uv_curve.evaluate(k, x_at(x, 0)?, 1, true, false)?;
        let p0 = pv
            .first()
            .copied()
            .ok_or_else(|| invalid("short curve evaluation"))?;
        let p1 = pv
            .get(1)
            .copied()
            .ok_or_else(|| invalid("short curve evaluation"))?;

        set_f(f, 0, p0.x - x_at(x, 1)?)?;
        set_f(f, 1, p0.y - x_at(x, 2)?)?;
        if let Some(j) = jacobian.as_deref_mut() {
            set_j(j, 0, 0, p1.x)?;
            set_j(j, 0, 1, -1.0)?;
            set_j(j, 0, 2, 0.0)?;
            set_j(j, 1, 0, p1.y)?;
            set_j(j, 1, 1, 0.0)?;
            set_j(j, 1, 2, -1.0)?;
            for kk in 3..j.num_columns() {
                set_j(j, 0, kk, 0.0)?;
                set_j(j, 1, kk, 0.0)?;
            }
            for ll in 2..j.num_rows() {
                for mm in 0..j.num_columns() {
                    set_j(j, ll, mm, 0.0)?;
                }
                set_f(f, ll, 0.0)?;
            }
        }

        let mut num_equations = 2;
        let mut num_parameters = 3;
        let surface1 = self
            .solver
            .solver_surface(self.rail_index)?
            .map(|surface| surface.base_id);
        let mut load = JacobianLoad {
            rail_index: self.rail_index,
            surface1,
            surf1_offset: 1,
            surface2: None,
            surf2_offset: 0,
            x,
        };
        let found_answer = self
            .solver
            .load_jacobian(
                k,
                &mut num_equations,
                &mut num_parameters,
                &mut load,
                f,
                jacobian,
            )
            .map_err(|e| as_sm_err(e, OP_E))?;

        let mut found = false;
        if found_answer {
            let scaled_tol = self.solver.solver().conversion_tol() * (1.0 + p0.max_dimension());
            if f_at(f, 0)?.abs() < scaled_tol && f_at(f, 1)?.abs() < scaled_tol {
                found = true;
            }
        }
        Ok(found)
    }
}

// ---- SmRailRailIntersectENFO ----------------------------------------------------------

/// `SmRailRailIntersectENFO`: the rails of two solvers meeting on a shared surface.
pub struct RailRailIntersectEnfo<'a> {
    /// `m_lRailIndex`.
    pub rail_index: usize,
    /// `m_lOtherRailIndex`.
    pub other_rail_index: usize,
    solver: &'a mut dyn FilletSolverOps,
    other_solver: &'a mut dyn FilletSolverOps,
}

impl<'a> RailRailIntersectEnfo<'a> {
    /// `SmRailRailIntersectENFO(lRailIndex, crFilletSolver, lOtherRailIndex,
    /// crOtherFilletSolver)`.
    pub fn new(
        rail_index: usize,
        solver: &'a mut dyn FilletSolverOps,
        other_rail_index: usize,
        other_solver: &'a mut dyn FilletSolverOps,
    ) -> Self {
        Self {
            rail_index,
            other_rail_index,
            solver,
            other_solver,
        }
    }

    /// `Evaluate(crX, rF, pOptJacobian, rbFoundAnswer)`: zeroes the Jacobian rows and
    /// matching `rF` rows, then both solvers load their rows (the second continuing
    /// from the counts the first left). Returns `rbFoundAnswer`.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when either `LoadJacobian` fails; [`SolidError::InvalidInput`] for
    /// arrays too short.
    pub fn evaluate(
        &mut self,
        k: &Kernel,
        x: &[f64],
        f: &mut [f64],
        mut jacobian: Option<&mut Matrix>,
    ) -> Result<bool> {
        const OP_E: &str = "blend::fillet_solver::p1::RailRailIntersectEnfo::evaluate";
        if let Some(j) = jacobian.as_deref_mut() {
            for ll in 0..j.num_rows() {
                for mm in 0..j.num_columns() {
                    set_j(j, ll, mm, 0.0)?;
                }
                set_f(f, ll, 0.0)?;
            }
        }

        let mut num_equations = 0;
        let mut num_parameters = 2;
        let surface1 = base_id(self.solver.solver(), self.rail_index)?;
        let mut load = JacobianLoad {
            rail_index: self.rail_index,
            surface1,
            surf1_offset: 0,
            surface2: None,
            surf2_offset: 0,
            x,
        };
        let found1 = self
            .solver
            .load_jacobian(
                k,
                &mut num_equations,
                &mut num_parameters,
                &mut load,
                f,
                jacobian.as_deref_mut(),
            )
            .map_err(|e| as_sm_err(e, OP_E))?;

        // pSurface, lSurf1Offset and lSurf2Offset as the first call left them;
        // pSurface2 is reset to NULL
        let mut load2 = JacobianLoad {
            rail_index: self.other_rail_index,
            surface1: load.surface1,
            surf1_offset: load.surf1_offset,
            surface2: None,
            surf2_offset: load.surf2_offset,
            x,
        };
        let found2 = self
            .other_solver
            .load_jacobian(
                k,
                &mut num_equations,
                &mut num_parameters,
                &mut load2,
                f,
                jacobian,
            )
            .map_err(|e| as_sm_err(e, OP_E))?;
        Ok(found1 && found2)
    }
}

// ---- SmFindClippedRadiusEFO -----------------------------------------------------------

/// `SmFindClippedRadiusEFO`: the constant-distance fillet radius at which the
/// offset/offset intersection crosses an iso boundary of one offset surface.
pub struct FindClippedRadiusEfo<'a> {
    solver: &'a mut FilletSolver,
    surf_index: usize,
    /// `m_eSurfParam`.
    pub surf_param: SurfParamType,
    /// `m_dIsoParam`.
    pub iso_param: f64,
    /// `m_rUV`: the surface's UV point (updated).
    pub uv: Vec2,
    /// `m_rOtherUV`: the other surface's UV point (updated).
    pub other_uv: Vec2,
    /// `m_dOrientation`.
    pub orientation: f64,
    /// `m_dOtherOrientation`.
    pub other_orientation: f64,
    /// `m_OtherDomain`.
    pub other_domain: Extent2d,
    /// `m_dTargetDistance`.
    pub target_distance: f64,
    /// `m_dThisApproxTol3d`.
    pub approx_tol_3d: f64,
    /// `m_dGuessT`.
    pub guess_t: f64,
    /// `rdFOfT` as the last [`Self::evaluate`] left it, also on its failure paths
    /// (`0` before the first call).
    pub last_f_of_t: f64,
    /// `rdFPrimeOfT` as the last [`Self::evaluate`] left it (`0` on its failure paths).
    pub last_f_prime: f64,
    /// `rbFoundAnswer` as the last [`Self::evaluate`] left it, also on its failure paths.
    pub last_found: bool,
}

impl<'a> FindClippedRadiusEfo<'a> {
    /// `SmFindClippedRadiusEFO(pFS, lSurfIndex, sUVDomains, eSurfParam, dIsoParam,
    /// sUV)`, with the solver's `GetDistance()` as `target_distance`.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for a surface index above 1.
    pub fn new(
        solver: &'a mut FilletSolver,
        surf_index: usize,
        uv_domains: &[Extent2d; 2],
        surf_param: SurfParamType,
        iso_param: f64,
        uv: [Vec2; 2],
        target_distance: f64,
    ) -> Result<Self> {
        if surf_index > 1 {
            return Err(invalid("surface index out of range"));
        }
        let other = 1 - surf_index;
        let orientation = solver.orientation(surf_index)?;
        let other_orientation = solver.orientation(other)?;
        let this_uv = *uv
            .get(surf_index)
            .ok_or_else(|| invalid("surface index out of range"))?;
        let other_uv = *uv
            .get(other)
            .ok_or_else(|| invalid("surface index out of range"))?;
        let other_domain = *uv_domains
            .get(other)
            .ok_or_else(|| invalid("surface index out of range"))?;
        let approx_tol_3d = solver.this_approx_tol_3d();
        Ok(Self {
            solver,
            surf_index,
            surf_param,
            iso_param,
            uv: this_uv,
            other_uv,
            orientation,
            other_orientation,
            other_domain,
            target_distance,
            approx_tol_3d,
            guess_t: this_uv.x,
            last_f_of_t: 0.0,
            last_f_prime: 0.0,
            last_found: false,
        })
    }

    fn set_offsets(&mut self, radius: f64) -> Result<()> {
        let (i, o) = (self.surf_index, 1 - self.surf_index);
        let d = radius * self.orientation;
        let od = radius * self.other_orientation;
        self.solver
            .surface_mut(i)?
            .ok_or_else(|| invalid("no offset surface"))?
            .surface
            .set_offset_distance(d);
        self.solver
            .surface_mut(o)?
            .ok_or_else(|| invalid("no other offset surface"))?
            .surface
            .set_offset_distance(od);
        Ok(())
    }

    /// `m_pOtherSurface->LocalCurveIntersect(m_OtherDomain, sIsoCurve, ...)` with the
    /// iso curve of the offset surface (`SmIsoCurve(*m_pSurface, m_eSurfParam,
    /// m_dIsoParam, TRUE, FALSE)`, unclamped): the other surface's UV point and the
    /// curve parameter, `None` when no good point was found. Both surfaces are
    /// evaluated through copies; the solver's offsets then take the base extension the
    /// evaluations made ([`FilletSolver::keep_extension_of_copy`]), as upstream's
    /// `const` evaluators store it in the solver's own offsets.
    fn intersect_iso(&mut self, k: &Kernel) -> Result<Option<(Point2, f64)>> {
        let (i, o) = (self.surf_index, 1 - self.surf_index);
        let surface = Surface::from(
            self.solver
                .surface(i)?
                .ok_or_else(|| invalid("no offset surface"))?
                .surface
                .surface
                .clone(),
        );
        let other = Surface::from(
            self.solver
                .surface(o)?
                .ok_or_else(|| invalid("no other offset surface"))?
                .surface
                .surface
                .clone(),
        );
        let iso = IsoCurve::from_surface(k, surface, self.surf_param, self.iso_param, true, false)?;
        let curve = Curve::from(iso);
        let interval = curve.natural_interval();
        let found = other.local_curve_intersect(
            k,
            &self.other_domain,
            &curve,
            &interval,
            self.approx_tol_3d,
            self.other_uv,
            self.guess_t,
        );
        // (also on a failure: upstream's evaluations stored their extensions already)
        self.solver.keep_extension_of_iso_copy(i, &curve)?;
        self.solver.keep_extension_of_copy(o, &other)?;
        Ok(found?.map(|c| (c.uv, c.t)))
    }

    fn base_points(&self, k: &Kernel, uv: Point2, other_uv: Point2) -> Result<f64> {
        let (i, o) = (self.surf_index, 1 - self.surf_index);
        let base = self
            .solver
            .surface(i)?
            .ok_or_else(|| invalid("no offset surface"))?
            .surface
            .base_surface();
        let other_base = self
            .solver
            .surface(o)?
            .ok_or_else(|| invalid("no other offset surface"))?
            .surface
            .base_surface();
        let p = evaluate_point_out_of_bounds(k, base, uv)?;
        let op = evaluate_point_out_of_bounds(k, other_base, other_uv)?;
        Ok(p.distance(op))
    }

    /// `Evaluate(dRadius, rdFOfT, rdFPrimeOfT, rbFoundAnswer)`: `F` is the rail point
    /// distance minus the target distance at the boundary crossing for `radius`, `F'`
    /// its forward difference over `SM_ZONE_TOL_3D`. Returns `(F, F', found)`; the
    /// out-parameters upstream leaves on a failure path are in
    /// [`Self::last_f_of_t`], [`Self::last_f_prime`] and [`Self::last_found`].
    ///
    /// # Errors
    ///
    /// `SM_ERR` when the iso curve misses the other surface; those of the evaluations;
    /// [`SolidError::Unported`] (module notes).
    pub fn evaluate(&mut self, k: &Kernel, radius: f64) -> Result<(f64, f64, bool)> {
        const OP_E: &str = "blend::fillet_solver::p1::FindClippedRadiusEfo::evaluate";
        let mut radius = radius;
        // init output
        self.last_f_of_t = 0.0;
        self.last_f_prime = 0.0;
        self.last_found = false;
        let mut found = false;
        self.set_offsets(radius)?;
        let Some((other_uv_found, t_found)) = self.intersect_iso(k)? else {
            return Err(SolidError::sm_err(OP_E));
        };
        self.other_uv = other_uv_found;
        self.guess_t = t_found;
        if self.surf_param == SurfParamType::U {
            self.uv.x = self.iso_param;
            self.uv.y = t_found;
        } else {
            self.uv.x = t_found;
            self.uv.y = self.iso_param;
        }
        let distance = self.base_points(k, self.uv, self.other_uv)?;
        let f_of_t = distance - self.target_distance;
        self.last_f_of_t = f_of_t;
        if f_of_t.abs() < EFF_ZERO {
            found = true;
            self.last_found = true;
        }

        let step_size = ZONE_TOL_3D;
        radius += step_size;
        self.set_offsets(radius)?;
        let Some((other_uv_found, t_found)) = self.intersect_iso(k)? else {
            return Err(SolidError::sm_err(OP_E));
        };
        let uv_found = if self.surf_param == SurfParamType::U {
            Point2::new(self.iso_param, t_found)
        } else {
            Point2::new(t_found, self.iso_param)
        };
        let distance1 = self.base_points(k, uv_found, other_uv_found)?;
        let f_prime = (distance1 - self.target_distance - f_of_t) / step_size;
        self.last_f_prime = f_prime;
        Ok((f_of_t, f_prime, found))
    }
}

/// `EvaluatePoint` of a base surface with `SmBSplineSurface` out-of-bounds evaluation
/// temporarily enabled (`SmTemporaryChangeValue` on `GetOutOfBoundsEnabled` of
/// `SM_CAST_PTR(SmBSplineSurface, ..)`: B-splines, planes and surfaces of revolution;
/// only the `SmBSplineSurface` member changes, not a revolution's generator).
fn evaluate_point_out_of_bounds(k: &Kernel, base: &Surface, uv: Point2) -> Result<Point3> {
    // the flag is set on the surface itself upstream: evaluate an exact copy
    let mut s = base.clone_for_evaluation()?;
    match s.kind_mut() {
        SurfaceKind::BSpline(b) => *b.out_of_bounds_enabled_mut() = true,
        SurfaceKind::Plane(p) => *p.out_of_bounds_enabled_mut() = true,
        SurfaceKind::SurfOfRevolution(r) => *r.base_mut().out_of_bounds_enabled_mut() = true,
        SurfaceKind::SurfOfExtrusion(r) => *r.base_mut().out_of_bounds_enabled_mut() = true,
        SurfaceKind::Cone(c) => *c.base_mut().out_of_bounds_enabled_mut() = true,
        SurfaceKind::Sphere(c) => *c.base.base_mut().out_of_bounds_enabled_mut() = true,
        SurfaceKind::Torus(c) => *c.base.base_mut().out_of_bounds_enabled_mut() = true,
        // an SmOffsetSurface is not an SmBSplineSurface either
        SurfaceKind::CurveBounded(_)
        | SurfaceKind::Step(_)
        | SurfaceKind::SrfInVolume(_)
        | SurfaceKind::OffsetSurface(_) => {}
    }
    s.evaluate_point(k, uv)
}

// ---- ComputeSurfaceValues / ComputeUVVectors --------------------------------------------

impl FilletSolver {
    /// `ComputeSurfaceValues(aUVValues, rTsectPnt)` dispatched on `ops` (its
    /// `SetupOffsetValues`): positions, first derivatives and unit normals of both
    /// offset surfaces, the curve position and the tangent plane angle (folded to at
    /// most `pi / 2`).
    ///
    /// # Errors
    ///
    /// Those of `SetupOffsetValues` (`SM_ERR` for the base solver) and of the
    /// evaluations; `SM_ERR` for a zero normal; [`SolidError::InvalidInput`] for a
    /// missing surface.
    pub fn compute_surface_values(
        k: &Kernel,
        ops: &mut dyn FilletSolverOps,
        uv_values: [Point2; 2],
        tsect: &mut TsectPnt,
    ) -> Result<()> {
        let snapshot = tsect.clone();
        ops.setup_offset_values(&snapshot, None)?;
        for (srf, uv) in uv_values.iter().enumerate() {
            let s = solver_surface_mut(ops.solver_mut(), srf)?;
            let (p, du, dv) = offset_evaluate_1st(k, s, *uv)?;
            let pv = tsect
                .surface_pv
                .get_mut(srf)
                .ok_or_else(|| invalid("surface index out of range"))?;
            pv[0][0] = p;
            pv[1][0] = du;
            pv[0][1] = dv;
            pv[2][0] = Vec3::new(0.0, 0.0, 0.0);
            pv[1][1] = Vec3::new(0.0, 0.0, 0.0);
            pv[0][2] = Vec3::new(0.0, 0.0, 0.0);
            let mut norm = du.cross(dv);
            if norm.length_squared() < EFF_ZERO_SQ {
                norm = offset_evaluate_normal(k, s, *uv)?;
                if norm.length_squared() < EFF_ZERO_SQ {
                    return Err(SolidError::sm_err(OP));
                }
            }
            // sNorm.Unitize() unchecked: a failure leaves (0, 0, 1)
            let norm = norm.unitize().unwrap_or(Vec3::new(0.0, 0.0, 1.0));
            pv[2][2] = norm;
            if let Some(uvp) = tsect.uv_curve_pv.get_mut(srf) {
                uvp[0] = *uv;
            }
        }
        tsect.curve_pv[0] = tsect.surface_pv[0][0][0];
        let n0 = tsect.surface_pv[0][2][2];
        let n1 = tsect.surface_pv[1][2][2];
        tsect.tangent_plane_angle_rad = n0.angle_between(n1)?;
        if tsect.tangent_plane_angle_rad > PI / 2.0 {
            tsect.tangent_plane_angle_rad = PI - tsect.tangent_plane_angle_rad;
        }
        Ok(())
    }
}

/// `SmFilletSolver::ComputeUVVectors(rTsectPnt)`: each surface's UV tangent of the
/// curve tangent (`smsurf_DropVectors` on the surface's first derivatives).
///
/// # Errors
///
/// Those of `smsurf_DropVectors` (the first surface's result is kept when the second
/// fails, as upstream).
pub fn compute_uv_vectors(tsect: &mut TsectPnt) -> Result<()> {
    for j in 0..2 {
        let pv = tsect
            .surface_pv
            .get(j)
            .ok_or_else(|| invalid("surface index out of range"))?;
        let du = pv[1][0];
        let dv = pv[0][1];
        let dropped = drop_vectors(du, dv, &[tsect.curve_pv[1]])?;
        let v = dropped
            .first()
            .copied()
            .ok_or_else(|| invalid("short drop result"))?;
        if let Some(uvp) = tsect.uv_curve_pv.get_mut(j) {
            uvp[1] = v;
        }
    }
    Ok(())
}

/// `SmTol::GetAngTolRad(const SmFilletSolver*)`: the solver's angle tolerance, the
/// system one without a solver.
#[must_use]
pub fn tol_ang_tol_rad(solver: Option<&FilletSolver>) -> f64 {
    solver.map_or_else(
        crate::geom::tol::ang_tol_rad,
        FilletSolver::this_ang_tol_rad,
    )
}

// ---- static helpers --------------------------------------------------------------------

/// `sm_CheckEdgeCurveDomains(pContext, pEdge, pCurve, bExtendBefore, bExtendAfter,
/// sDelCurve)`: when the edge's curve reaches beyond the edge's interval (by more than
/// `SM_EFF_ZERO`) at an end that is not extended, a copy trimmed to the edge interval
/// at those ends. Returns the copy (`TRUE`, `pCurve` the copy) or `None` (`FALSE`,
/// `pCurve` the edge's curve).
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for an edge without curve (upstream dereferences
/// `NULL`); the trim's status is ignored as upstream ([`SolidError::Unported`] still
/// propagates).
pub fn check_edge_curve_domains(
    k: &Kernel,
    b: &Brep,
    e: EdgeId,
    extend_before: bool,
    extend_after: bool,
) -> Result<Option<Curve>> {
    let curve = edge::curve(b, e)?.ok_or_else(|| invalid("edge without curve"))?;
    let edge_ivl = edge::interval(b, e)?;
    let curv_ivl = curve.natural_interval();
    let mut t0 = curv_ivl.min();
    let mut t1 = curv_ivl.max();
    let mut trim = false;
    if !extend_before && curv_ivl.min() < edge_ivl.min() - EFF_ZERO {
        trim = true;
        t0 = edge_ivl.min();
    }
    if !extend_after && curv_ivl.max() > edge_ivl.max() + EFF_ZERO {
        trim = true;
        t1 = edge_ivl.max();
    }
    if trim {
        if let Some(mut copy) = ignored(curve.copy(k))? {
            let mut trim_ivl = Extent1d::from_min_max(t0, t1);
            ignore_status(copy.trim(k, &mut trim_ivl, TrimMode::Notify))?;
            return Ok(Some(copy));
        }
    }
    Ok(None)
}

/// The `SmBSplineSurface*` [`test_analytic_surface`] tests: a [`Surface`] (which may
/// hold an `SmCone` / `SmCylinder`), a bare cone, or an `SmSphere` or `SmTorus`, which
/// no [`Surface`] kind holds yet.
#[derive(Debug, Clone, Copy)]
pub enum AnalyticTestSurface<'a> {
    /// A surface of one of the [`Surface`] kinds.
    Surface(&'a Surface),
    /// A cone or cylinder (`SmCone_TYPE` / `SmCylinder_TYPE`).
    Cone(&'a Cone),
    /// A sphere.
    Sphere(&'a Sphere),
    /// A torus.
    Torus(&'a Torus),
}

/// `sm_TestAnalyticSurface(pSurface)`: a plane by type (`GetType()`), a cone or
/// cylinder (`IsKindOf(SmCone_TYPE)`), or a surface that is planar (`IsPlanar()`,
/// tolerance `SM_EFF_ZERO`) or a canonical cone (`SmCone::IsNurbSurfaceCone`).
///
/// # Errors
///
/// Those of `IsPlanar` and of the cone test (`SmCone::IsNurbSurfaceCone`);
/// `InvalidInput` for a surface kind that is not an
/// `SmBSplineSurface` (a plane is accepted by type before).
pub fn test_analytic_surface(k: &Kernel, surface: AnalyticTestSurface<'_>) -> Result<bool> {
    use crate::surface::surface::p2::{CONE_TYPE, PLANE_TYPE};
    let type_code = match surface {
        AnalyticTestSurface::Surface(s) => s.type_code(),
        AnalyticTestSurface::Cone(c) => c.type_code(),
        AnalyticTestSurface::Sphere(s) => s.type_code(),
        AnalyticTestSurface::Torus(t) => t.type_code(),
    };
    // return TRUE when surface type is Plane or Cone
    if type_code == PLANE_TYPE {
        return Ok(true);
    }
    let kind_of_cone = match surface {
        AnalyticTestSurface::Surface(s) => s.is_kind_of(CONE_TYPE)?,
        AnalyticTestSurface::Cone(c) => c.is_kind_of(CONE_TYPE)?,
        AnalyticTestSurface::Sphere(s) => s.is_kind_of(CONE_TYPE)?,
        AnalyticTestSurface::Torus(t) => t.is_kind_of(CONE_TYPE)?,
    };
    if kind_of_cone {
        return Ok(true);
    }

    // test NurbSurface to see if its a canonical Plane (SmSurface::IsPlanar: cones,
    // cylinders, spheres and tori inherit it and the SmBSplineSurface bounding box,
    // which is that of their SmSurfOfRevolution base)
    let planar = match surface {
        AnalyticTestSurface::Surface(s) => s.is_planar(k, EFF_ZERO)?,
        AnalyticTestSurface::Cone(c) => Surface::from(c.rev().clone()).is_planar(k, EFF_ZERO)?,
        AnalyticTestSurface::Sphere(s) => Surface::from(s.base().clone()).is_planar(k, EFF_ZERO)?,
        AnalyticTestSurface::Torus(t) => Surface::from(t.base.clone()).is_planar(k, EFF_ZERO)?,
    };
    if planar.is_planar {
        return Ok(true);
    }
    // test NurbSurface to see if its a canonical cone (the SmBSplineSurface part of the
    // surface, default dToleranceScale = 1)
    let bss = match surface {
        AnalyticTestSurface::Surface(s) => match s.kind_with_nurb(k)? {
            SurfaceKind::BSpline(b) => b,
            SurfaceKind::Plane(p) => p.base(),
            SurfaceKind::SurfOfRevolution(r) => r.base(),
            SurfaceKind::SurfOfExtrusion(r) => r.base(),
            SurfaceKind::Cone(c) => c.rev().base(),
            SurfaceKind::Sphere(s) => s.base().base(),
            SurfaceKind::Torus(s) => s.base().base(),
            // not an SmBSplineSurface: upstream's parameter type excludes these
            _ => {
                return Err(SolidError::InvalidInput {
                    operation: "blend::fillet_solver::test_analytic_surface",
                    reason: "surface is not an SmBSplineSurface",
                })
            }
        },
        AnalyticTestSurface::Cone(c) => c.rev().base(),
        AnalyticTestSurface::Sphere(s) => s.base().base(),
        AnalyticTestSurface::Torus(t) => t.base().base(),
    };
    Ok(Cone::is_nurb_surface_cone(k, bss, 1.0)?.is_some())
}

/// `SE(pEU->EvaluateBinormal(dParameter, FALSE, rPnt, rBinVec, NULL, &rFaceuseNormal))`
/// into outputs the caller default-constructed (`SM_UNDEF_DOUBLE` components): the
/// point, binormal and face use normal. A failing status is ignored, and the outputs
/// are then what upstream leaves in them: all undefined when the edge use is not in a
/// loop (that check fails before the outputs are initialised); otherwise the point and
/// the normal zeroed and the binormal undefined (they are assigned only on success).
///
/// # Errors
///
/// [`SolidError::Unported`]; `NotFound` and `InvalidInput` (a dangling id or a `NULL`
/// upstream dereferences) also propagate.
fn binormal_status_ignored(
    k: &Kernel,
    b: &mut Brep,
    edge_use: EdgeuseId,
    parameter: f64,
) -> Result<(Point3, Vec3, Vec3)> {
    let undefined = Vec3::new(UNDEF_DOUBLE, UNDEF_DOUBLE, UNDEF_DOUBLE);
    let zero = Vec3::new(0.0, 0.0, 0.0);
    let in_loop = eu::is_loop_edgeuse(b, edge_use)?;
    let options = BinormalOptions::default();
    match evaluate_binormal(k, b, edge_use, parameter, false, &options) {
        Ok(r) => Ok((r.point, r.binormal, r.face_normal)),
        Err(e)
            if e.is_unported()
                || matches!(
                    e,
                    SolidError::NotFound { .. }
                        | SolidError::InvalidInput { .. }
                        | SolidError::PortInvariant { .. }
                ) =>
        {
            Err(e)
        }
        Err(_) if !in_loop => Ok((undefined, undefined, undefined)),
        Err(_) => Ok((zero, undefined, zero)),
    }
}

/// `sm_TestManifoldTangentRollover(pE, rbOppositeNormals, dTangencyTolRad,
/// iDebugLevel)`: whether the faces at the edge are tangent (the second edge use's
/// binormal is within `tangency_tol_rad` of perpendicular to the first face use
/// normal), and whether their face use normals oppose. Returns `(tangent,
/// opposite_normals)`.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] where upstream dereferences `NULL` (an edge without
/// primary edge use, an edge use without face use, radial or mate); the binormal
/// evaluation's [`SolidError::Unported`], `NotFound` and `InvalidInput` (its status
/// is otherwise ignored, see `binormal_status_ignored`).
pub fn test_manifold_tangent_rollover(
    k: &Kernel,
    b: &mut Brep,
    e: EdgeId,
    tangency_tol_rad: f64,
) -> Result<(bool, bool)> {
    let edge_use = eu::primary_edgeuse(b, e)?.ok_or_else(|| invalid("edge without edge use"))?;
    let test_param = edge::interval(b, e)?.evaluate(0.34);
    // SE(EvaluateBinormal(dTestParam, FALSE, sPnt, sBinVec, NULL, &sFaceuseNormal))
    let (_pnt, _bin_vec, mut faceuse_normal) = binormal_status_ignored(k, b, edge_use, test_param)?;
    let fu = eu::faceuse(b, edge_use)?.ok_or_else(|| invalid("edge use without face use"))?;
    if eu::faceuse_orientation(b, fu)? == OrientType::Opposite {
        faceuse_normal = -faceuse_normal;
    }
    let radial = eu::radial(b, edge_use)?.ok_or_else(|| invalid("edge use without radial"))?;
    let other_eu = eu::mate(b, radial)?.ok_or_else(|| invalid("radial edge use without mate"))?;
    // SE(EvaluateBinormal(dTestParam, FALSE, sPnt2, sBinVec2, NULL, &sFaceuseNormal2))
    let (_pnt2, bin_vec2, mut faceuse_normal2) =
        binormal_status_ignored(k, b, other_eu, test_param)?;
    let fu2 = eu::faceuse(b, other_eu)?.ok_or_else(|| invalid("edge use without face use"))?;
    if eu::faceuse_orientation(b, fu2)? == OrientType::Opposite {
        faceuse_normal2 = -faceuse_normal2;
    }
    let opposite_normals = faceuse_normal.dot(faceuse_normal2) < 0.0;

    // Test tangency. Note, we're comparing vectors that would be perpendicular when
    // the Faces are tangent.
    let dot_limit = (PI / 2.0 - tangency_tol_rad).cos(); // [B683]
    if bin_vec2.dot(faceuse_normal).abs() > dot_limit {
        return Ok((false, opposite_normals));
    }
    Ok((true, opposite_normals))
}

/// `sm_TestSideEdgeConvexity(pSideEdge, bIsTangentialSideEdge)`: whether a radial
/// sector of the side edge is convex: for each edge use (the mate of one whose shell
/// lies in the B-rep's infinite region), `IsConvexRadialSector(10, step_off)`, stepping
/// off for a tangential side edge; true at the first convex one.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for an edge use without shell or mate (upstream
/// dereferences `NULL`); those of `IsConvexRadialSector`.
pub fn test_side_edge_convexity(
    k: &Kernel,
    b: &mut Brep,
    side_edge: EdgeId,
    is_tangential: bool,
) -> Result<bool> {
    let eus = edge::edgeuses(b, side_edge, None, None)?;
    for eu0 in eus {
        let mut eu_id = eu0;
        let shell = eu::shell(b, eu_id)?.ok_or_else(|| invalid("edge use without shell"))?;
        // pShell->GetBrep()->GetInfiniteRegion(): the shell's B-rep is `b`
        let infinite_reg = b.infinite_region();
        if b.shell(shell)?.region == infinite_reg {
            eu_id = eu::mate(b, eu_id)?.ok_or_else(|| invalid("edge use without mate"))?;
        }

        let step_off = is_tangential;
        if crate::topology::edgeuse::p2::is_convex_radial_sector(k, b, eu_id, 10, step_off, 0.0)? {
            return Ok(true);
        }
    } // end for each Edgeuse

    Ok(false)
}

/// `sm_FindNextGoodInterval(pCC, lStartSearch, rbFound, rlFoundIndex)`: the first
/// interval from `start` whose interior is on a face or an edge.
#[must_use]
pub fn find_next_good_interval<O: ClassObject>(
    cc: &CurveClassification<O>,
    start: usize,
) -> Option<usize> {
    cc.intervals()
        .iter()
        .enumerate()
        .skip(start)
        .find(|(_, ivl)| {
            let pc = ivl.mid.point_class();
            pc == PointClassType::Face || pc == PointClassType::Edge
        })
        .map(|(i, _)| i)
}

/// `sm_TrimRail(pRail, crTrimInterval)`: trims the rail's 3D curve and the UV trim
/// curves of its primary edge use and of the mate (`SmEdgeuse::GetUVTrimCurve`, which
/// reads the mate's curve when the edge use holds none), each to the interval the
/// previous trim may have snapped.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for a rail without primary edge use or mate (upstream
/// dereferences `NULL`); `SM_ERR_NULL_POINTER` for a curve that is not an
/// `SmBSplineCurve` or a missing UV curve (`NER`); those of the trims.
pub fn trim_rail(k: &Kernel, b: &mut Brep, rail: EdgeId, trim_interval: &Extent1d) -> Result<()> {
    const OP_T: &str = "blend::fillet_solver::p1::trim_rail";
    let prim = eu::primary_edgeuse(b, rail)?.ok_or_else(|| invalid("rail without edge use"))?;
    let mate = eu::mate(b, prim)?.ok_or_else(|| invalid("rail edge use without mate"))?;
    let is_bspline = edge::curve(b, rail)?.is_some_and(|c| c.is_kind_of(BSplineCurve::TYPE));
    if !is_bspline {
        return Err(SolidError::null_pointer(OP_T));
    }
    let (uv, uv_owner) = eu::uv_trim_curve(b, prim)?;
    let uv_owner = uv_owner
        .filter(|_| uv.is_some())
        .ok_or(SolidError::null_pointer(OP_T))?;
    let (orig, orig_owner) = eu::uv_trim_curve(b, mate)?;
    let orig_owner = orig_owner
        .filter(|_| orig.is_some())
        .ok_or(SolidError::null_pointer(OP_T))?;
    let mut trim = *trim_interval;
    let missing = || SolidError::null_pointer(OP_T);
    // `SmBSplineCurve::Trim` reads `SmTol::GetZoneTol3d(this)`: for a curve owned by an
    // edge, or by an edge use of an edge (the UV trim curves), that is the edge's
    // tolerance (`SM_USE_OLDTOL`), not the system zone tolerance the owner-less
    // `Curve::trim` reads from its kernel. All three curves belong to the rail, so
    // they trim with a kernel whose zone tolerance is the rail's tolerance.
    let mut tk = Kernel::new(&crate::kernel::KernelOptions {
        tolerances: crate::kernel::Tolerances {
            zone_tol_3d: edge::tolerance(b, rail)?,
            ..k.tolerances
        },
        ..crate::kernel::KernelOptions::default()
    });
    // the caller's random state carries over; the caller's `context` (marks, notify
    // callbacks owned by boxed trait objects) cannot be cloned and nothing on the trim
    // path reads it
    tk.rng = k.rng.clone();
    tk.fixes.bspline_trim_preserves_uv_spans = k.fixes.bspline_trim_preserves_uv_spans;
    b.edge_mut(rail)?
        .curve
        .as_mut()
        .ok_or_else(missing)?
        .trim(&tk, &mut trim, TrimMode::Notify)?;
    b.edgeuse_mut(uv_owner)?
        .uv_trim_curve
        .as_mut()
        .ok_or_else(missing)?
        .trim(&tk, &mut trim, TrimMode::Notify)?;
    b.edgeuse_mut(orig_owner)?
        .uv_trim_curve
        .as_mut()
        .ok_or_else(missing)?
        .trim(&tk, &mut trim, TrimMode::Notify)?;
    Ok(())
}

#[path = "p1/rails.rs"]
pub(crate) mod rails;
pub use rails::{compute_support_edge, split_rail, RailOwnership, RetainedRailOwnership};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surface::plane::Plane;

    #[test]
    fn trim_callback_borrows_actual_solver_and_stops_after_failure() {
        use crate::blend::fillet_geom::p1::{FilletGeom, FilletGeomType};

        let k = Kernel::default();
        let mut pseudo = FilletBrep::new();
        let mut solver = FilletSolver::new(1e-5, 1e-3, 0.01);
        let ids: Vec<_> = (0..3)
            .map(|_| pseudo.add_geom(FilletGeom::new(&k, None, FilletGeomType::Default).unwrap()))
            .collect();
        solver.fillet_geoms.clone_from(&ids);
        let mut visited = Vec::new();
        let error = solver
            .trim_rails_with(|id, actual| {
                assert!(core::ptr::eq(actual, &raw const solver));
                visited.push(id);
                if Some(&id) == ids.get(1) {
                    return Err(SolidError::sm_err("second geometry trim"));
                }
                pseudo.geom_mut(id)?.deviations[0] = actual.approx_tol_3d;
                Ok(())
            })
            .unwrap_err();
        assert!(matches!(error, SolidError::Upstream { code: 1001, .. }));
        assert_eq!(visited, ids[..2]);
        assert_eq!(
            pseudo.geom(ids[0]).unwrap().deviations[0],
            solver.approx_tol_3d
        );
        assert_eq!(pseudo.geom(ids[1]).unwrap().deviations[0], 0.0);
        assert_eq!(pseudo.geom(ids[2]).unwrap().deviations[0], 0.0);
    }

    #[test]
    fn extension_lookup_uses_actual_oriented_endpoints_and_live_corners() {
        use crate::blend::fillet_corner::p3::FilletCornerKind;
        let mut k = Kernel::default();
        let target =
            crate::primitive::api_primitives::create_box(&mut k, Point3::ZERO, 10.0, 20.0, 30.0)
                .unwrap();
        let edge = target.edges().ids().next().unwrap();
        let primary = eu::primary_edgeuse(&target, edge).unwrap().unwrap();
        let mate = eu::mate(&target, primary).unwrap().unwrap();
        let start = edge::start_vertex(&target, edge).unwrap();
        let end = edge::end_vertex(&target, edge).unwrap();
        let mut before = FilletCorner::new(FilletCornerKind::NByOne, start, None, None, 1e-5, 0.01);
        let mut after =
            FilletCorner::new(FilletCornerKind::Degenerate, end, None, None, 1e-5, 0.01);
        for edgeuses in [[primary, mate], [mate, primary]] {
            let mut solver = FilletSolver::new(1e-5, 1e-3, 0.01);
            solver.executive = Some(FilletExecutiveRef);
            solver.edgeuses = edgeuses.map(Some);
            let mut looked_up = Vec::new();
            solver
                .set_extension_flags_with_lookup(&target, |vertex| {
                    looked_up.push(vertex);
                    if vertex == start {
                        Ok(&before)
                    } else {
                        assert_eq!(vertex, end);
                        Ok(&after)
                    }
                })
                .unwrap();
            assert_eq!(looked_up, [start, end]);
            assert!(solver.extend_before);
            assert!(!solver.extend_after);
            before.blending = true;
            after.degenerate = false;
            solver
                .set_extension_flags_with_lookup(&target, |vertex| {
                    if vertex == start {
                        Ok(&before)
                    } else {
                        Ok(&after)
                    }
                })
                .unwrap();
            assert!(!solver.extend_before);
            assert!(solver.extend_after);
            before.blending = false;
            after.degenerate = true;
        }
    }

    #[test]
    fn extension_second_lookup_failure_preserves_both_flags() {
        use crate::blend::fillet_corner::p3::FilletCornerKind;
        let mut k = Kernel::default();
        let target =
            crate::primitive::api_primitives::create_box(&mut k, Point3::ZERO, 10.0, 20.0, 30.0)
                .unwrap();
        let edge = target.edges().ids().next().unwrap();
        let primary = eu::primary_edgeuse(&target, edge).unwrap().unwrap();
        let mate = eu::mate(&target, primary).unwrap().unwrap();
        let corner = FilletCorner::new(FilletCornerKind::NByOne, None, None, None, 1e-5, 0.01);
        let mut solver = FilletSolver::new(1e-5, 1e-3, 0.01);
        solver.executive = Some(FilletExecutiveRef);
        solver.edgeuses = [Some(primary), Some(mate)];
        solver.extend_before = false;
        solver.extend_after = true;
        let mut calls = 0;
        let error = solver
            .set_extension_flags_with_lookup(&target, |_| {
                calls += 1;
                if calls == 1 {
                    Ok(&corner)
                } else {
                    Err(SolidError::sm_err("second actual corner lookup"))
                }
            })
            .unwrap_err();
        assert_eq!(calls, 2);
        assert!(matches!(error, SolidError::Upstream { code: 1001, .. }));
        assert!(!solver.extend_before);
        assert!(solver.extend_after);
    }

    /// `Evaluate`'s result and the EFO's `(F, F', found, guess_t)` after it.
    type Evaluated = (Result<(f64, f64, bool)>, (f64, f64, bool, f64));

    /// A solver with offsets of `bases` (orientations `orient`) and an EFO on surface 0,
    /// `SM_SP_U`, `iso = 0`, evaluated at `radius`; returns the result and the EFO's
    /// `(F, F', found, guess_t)` after the call.
    fn clipped(
        bases: [Surface; 2],
        orient: [f64; 2],
        other_dom: Extent2d,
        radius: f64,
    ) -> Evaluated {
        let k = &Kernel::default();
        let kern = Kernel::default();
        let mut fs = FilletSolver::new(1e-6, 0.1, 0.01);
        for (k, base) in bases.into_iter().enumerate() {
            let surface =
                OffsetSurface::new(&kern, 0.0, base, SurfaceId::Free(k as u32), SS_UNKNOWN)
                    .unwrap();
            fs.set_surface(
                k,
                Some(OffsetSurfaceSlot {
                    key: k as u64,
                    surface,
                }),
            )
            .unwrap();
        }
        fs.orientations = orient;
        let wide = Extent2d::from_points(Point2::new(-10.0, -10.0), Point2::new(10.0, 10.0));
        let uv = [Vec2::new(0.1, 0.2), Vec2::new(0.3, -0.2)];
        let mut e = FindClippedRadiusEfo::new(
            &mut fs,
            0,
            &[wide, other_dom],
            SurfParamType::U,
            0.0,
            uv,
            0.5,
        )
        .unwrap();
        let r = e.evaluate(k, radius);
        let after = (e.last_f_of_t, e.last_f_prime, e.last_found, e.guess_t);
        (r, after)
    }

    fn z_plane() -> Surface {
        Surface::from(Plane::from_normal(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 1.0),
        ))
    }

    /// The plane of normal `(1, 2, 3)` whose unit offset passes through `(0, 0, 1)`.
    fn tilted() -> Surface {
        let n = Vec3::new(1.0, 2.0, 3.0);
        let u = n.unitize().unwrap();
        Surface::from(Plane::from_normal(Vec3::new(0.0, 0.0, 1.0) - u, n))
    }

    fn is_sm_err(r: &Result<(f64, f64, bool)>) -> bool {
        matches!(r, Err(SolidError::Upstream { code, .. }) if *code == crate::error::SM_ERR)
    }

    #[test]
    fn clipped_parallel_offsets_fail_at_the_first_intersection() {
        let wide = Extent2d::from_points(Point2::new(-10.0, -10.0), Point2::new(10.0, 10.0));
        let (r, (f, fp, found, guess)) = clipped([z_plane(), z_plane()], [1.0, -1.0], wide, 1.0);
        assert!(is_sm_err(&r), "{r:?}");
        assert_eq!((f, fp, found, guess), (0.0, 0.0, false, 0.1));
    }

    #[test]
    fn clipped_tilted_plane_evaluates() {
        let wide = Extent2d::from_points(Point2::new(-10.0, -10.0), Point2::new(10.0, 10.0));
        let (r, _) = clipped([z_plane(), tilted()], [1.0, 1.0], wide, 1.0);
        let (f, fp, found) = r.unwrap();
        assert!(f.is_finite() && fp.is_finite() && fp != 0.0 && !found);
    }

    #[test]
    fn clipped_point_domain_fails_after_the_radius_step() {
        let point = Extent2d::from_points(Point2::new(0.0, 0.0), Point2::new(0.0, 0.0));
        let (r, (f, fp, _, guess)) = clipped([z_plane(), tilted()], [1.0, 1.0], point, 1.0);
        assert!(is_sm_err(&r), "{r:?}");
        // the first intersection set F and the guess, the second failed
        assert!(f != 0.0 && fp == 0.0 && guess != 0.1, "{f} {fp} {guess}");
    }

    #[test]
    fn clipped_far_domain_fails_at_the_first_intersection() {
        let far = Extent2d::from_points(Point2::new(50.0, 50.0), Point2::new(60.0, 60.0));
        let (r, (f, _, _, guess)) = clipped([z_plane(), tilted()], [1.0, 1.0], far, 1.0);
        assert!(is_sm_err(&r), "{r:?}");
        assert_eq!((f, guess), (0.0, 0.1));
    }

    #[test]
    fn constructor_geometry_gets_the_offset_copies_at_finish() {
        let kern = Kernel::default();
        let mut pseudo = FilletBrep::new();
        let mut fs =
            FilletSolver::new_in(&kern, &mut pseudo, FilletSolverId(7), 1e-6, 0.1, 0.01).unwrap();
        assert_eq!(fs.fillet_geoms().len(), 1);
        let g = fs.first_fillet_geom().unwrap();
        assert_eq!(fs.last_fillet_geom().unwrap(), g);
        {
            let geom = pseudo.geom(g).unwrap();
            // made before the solver had surfaces
            assert!(geom.off_surfs.iter().all(Option::is_none));
            assert_eq!(geom.fillet_solver(), Some(FilletSolverId(7)));
            assert_eq!(geom.fillet_geom_type(), FilletGeomType::Default);
        }
        let surface =
            OffsetSurface::new(&kern, 0.25, z_plane(), SurfaceId::Free(0), SS_UNKNOWN).unwrap();
        fs.set_surface(0, Some(OffsetSurfaceSlot { key: 0, surface }))
            .unwrap();
        fs.finish_construction(&kern, &mut pseudo).unwrap();
        let geom = pseudo.geom(g).unwrap();
        let own = geom.off_surfs[0].as_ref().unwrap();
        assert_eq!(own.surface.offset_distance(), 0.25);
        assert!(geom.off_surfs[1].is_none());
        // GetOffsetSurface: the geometry's own, else the solver's
        assert_eq!(
            geom.offset_surface(0, Some(&fs))
                .unwrap()
                .unwrap()
                .surface
                .offset_distance(),
            0.25
        );
        assert!(geom.offset_surface(1, Some(&fs)).unwrap().is_none());
        // the plain constructor has no geometry until it is added
        let mut bare = FilletSolver::new(1e-6, 0.1, 0.01);
        assert!(bare.fillet_geoms().is_empty());
        bare.finish_construction(&kern, &mut pseudo).unwrap();
        let g2 = bare
            .add_constructor_fillet_geom(&kern, &mut pseudo, FilletSolverId(8))
            .unwrap();
        assert_ne!(g, g2);
    }
}
