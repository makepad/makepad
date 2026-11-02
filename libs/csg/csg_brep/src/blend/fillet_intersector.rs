//! The fillet intersector (`SmFilletIntersector`): the advanced surface intersector
//! tracing the intersection of the two offset surfaces of a fillet solver (the fillet
//! centre line), stepping through the solver's own Newton solves and flushing the
//! traced points into the centre line, the two rail curves, their UV curves and the
//! fillet surface.
//!
//! Upstream: `source/SMLib/src/SmFilletIntersector.cpp`,
//! `source/SMLib/inc/SmFilletIntersector.h`.
//!
//! | Rust                                                    | Upstream                                        |
//! |---------------------------------------------------------|-------------------------------------------------|
//! | [`FilletIntersector`], [`FilletIntersector::new`]       | `SmFilletIntersector`, its constructor          |
//! | `Drop` (implicit)                                       | `~SmFilletIntersector`                          |
//! | [`FilletIntersector::compute_point_values`]             | `ComputePointValues`                            |
//! | [`FilletIntersector::compute_point_values_delta`]       | `ComputePointValuesDelta`                       |
//! | [`FilletIntersector::test_span_accuracy`]               | `TestSpanAccuracy`                              |
//! | [`FilletIntersector::compute_next_point`]               | `ComputeNextPoint`                              |
//! | [`flush_crv`]                                           | `sm_FlushCrv` (static)                          |
//! | [`find_guess_mid_tsect_pnt`]                            | `sm_FindGuessMidTsectPnt` (static)              |
//! | [`FilletIntersector::flush_curve`]                      | `FlushCurve`                                    |
//! | [`FilletIntersector::evaluate_law_point`]               | `EvaluateLawPoint`                              |
//! | [`FilletIntersector::do_point_intersection`], [`FilletIntersector::do_law_intersection`], [`FilletDispatch`] | `DoPointIntersection`, `DoLawIntersection` (inherited) run on `*this` |
//! | [`FilletIntersector::curr_fillet_geom`], [`FilletIntersector::curr_fillet_touch_boundary`], [`FilletIntersector::is_curr_fillet_touch_boundary`], [`FilletIntersector::self_intersect`] | `GetCurrFilletGeom`, `GetCurrFilletTouchBoundary`, `IsCurrFilletTouchBoundary`, `GetSelfIntersect` |
//! | [`FilletIntersector::set_self_intersect`]               | `SetSelfIntersect`                              |
//! | [`FilletIntersector::is_kind_of`], [`TYPE`]             | `IsKindOf`, `SmFilletIntersector_TYPE`          |
//! | [`FilletSolverOps`], [`StepSolve`] (CO-PORTED interface) | the `SmFilletSolver` virtuals and members these methods call |
//! | `FilletSolverOps for ConstantRadiusAssistedFs`          | delegates to the `SmFilletSolver` base of unit `blend.fillet_solver_p1`: `ComputeSurfaceValues`, `ComputeUVVectors`, `SetSurfacesToZeroOffset`, `ReloadSurfaceOffsets`, `SetStatus`, `GetLastFilletGeom`, `GetSelfIntersectionHandler`, `GetFilletSurfaceGenerator`, `m_pFSG->CreateSurface` |
//! | [`FilStatus`] (re-export of `blend.fillet_geom_p1`'s)   | `SmFilStatus`                                   |
//! | [`HandlingAlgorithmType`] (CO-PORTED)                   | `SmHandlingAlgorithmType`                       |
//!
//! Deviations:
//! - **Solver.** Upstream holds the solver by reference (`m_rFilletSolver`); here every
//!   method that uses it takes it as an argument (`solver: &mut S`), so a solver can
//!   own the surfaces it hands the intersector. `GetFilletSolver` is dropped. The
//!   solver is any [`FilletSolverOps`]: the trait gathers the `SmFilletSolver`
//!   virtuals and members (and the `SmFilletGeom` and self-intersection handler calls
//!   reached through it) that this file calls. The methods of the base
//!   `SmFilletSolver` (unit `blend.fillet_solver_p1`) are required; the defaults of
//!   the others return [`SolidError::Unported`], tagged with the unit owning each
//!   upstream symbol, so the solver units implement them as they are ported. The
//!   implementation for the co-ported [`ConstantRadiusAssistedFs`] delegates to the
//!   base [`FilletSolver`] (`m_dSavedOffsets` is the base's, uninitialised upstream,
//!   starts at `0`). The fillet geometry pointer (`SmFilletGeom*`, unit
//!   `blend.fillet_geom_p1`) is the solver's handle type
//!   [`FilletSolverOps::FilletGeom`] (a [`FilletGeomId`] of the executive's pseudo
//!   B-rep for that solver); `m_pCurrFilletGeom` is an `Option` of it. The writes
//!   `FlushCurve` makes to the geometry, its rails and their mate edge uses go to
//!   the pseudo B-rep ([`FilletBrep`]) its caller passes (upstream reaches it through
//!   the geometry pointer).
//! - **Virtual overrides.** `ComputeNextPoint`, `FlushCurve`, `ComputePointValues`,
//!   `ComputePointValuesDelta`, `EvaluateLawPoint` and `TestSpanAccuracy` are
//!   inherent methods here that take the solver; [`FilletDispatch`] implements the
//!   base drivers' hook trait (`intersect::surface_intersector::p4::IntersectorVirtuals`)
//!   with them, so [`FilletIntersector::do_point_intersection`] and
//!   [`FilletIntersector::do_law_intersection`] run the base drivers on the overrides.
//!   A driver run directly on a [`FilletIntersector`]'s `base` calls the basic
//!   intersector's versions.
//! - **Surfaces.** Upstream every caller builds `SmFilletIntersector` over the
//!   solver's own `SmOffsetSurface`s, so `SetSurfacesToZeroOffset` changes the
//!   intersector's `m_cpSurface` too (the span tests and `ComputeNextPoint` rely on
//!   it). Here the intersector borrows its two [`Surface`]s; zeroing the solver's
//!   offsets does not change them. A solver must therefore hand the intersector the
//!   surfaces it means to be evaluated (or a solver-backed surface accessor must be
//!   added); the difftest builds both sides over plain base surfaces, so the
//!   offset-surface configuration is not compared here.
//! - **Context.** `m_cpContext = rFilletSolver.GetContext()` is dropped (no contexts).
//! - **Points.** As in the base intersector (`intersect.surface_intersector_p1..p4`),
//!   the points of `ComputeNextPoint` and `TestSpanAccuracy` are arena ids
//!   ([`TsectId`]); `ComputePointValues`, `ComputePointValuesDelta` take the points by
//!   reference. `FlushCurve`'s new mid-span points are owned locally (upstream
//!   allocates and deletes them).
//! - **Outputs.** Out-params become return values ([`NextPoint`], [`SpanAccuracy`],
//!   the curve parameter and UV points of `EvaluateLawPoint`). `FlushCurve` appends
//!   to the `rCurveTypes` and `rDeviations` of a [`TsectCurves`] (its curve arrays
//!   are unused upstream too).
//! - **Statuses.** `StepSolve`'s status is turned into "no answer" as upstream; its
//!   clip and boundary flags (an out-param of [`FilletSolverOps::step_solve`]) keep
//!   whatever it set before failing, and its in/out step size is honoured.
//!   Statuses upstream ignores are ignored, except [`SolidError::Unported`], which
//!   always propagates.
//! - **Curves on surfaces.** Span tests map Hermite UV curves through [`CrvOnSurf`]
//!   on an exact evaluation copy of the concrete [`Surface`]. Plane and offset
//!   overrides are retained; a surface of revolution evaluates as its B-spline
//!   base, as upstream.
//! - **Dead code.** Step 11 of `FlushCurve` (the self-intersection blending) is
//!   guarded by the constant `bTempTrySelfIntHandler = FALSE` and never runs; it is
//!   not ported.
//! - **Loops.** `FlushCurve`'s span equalisation stops once the knot count exceeds
//!   three times its start, as upstream; each pass at most doubles it. No recursion.
//! - **Not ported.** `Dump` and the `SM_DEBUG_CODE` drawing blocks; `SM_ASSERT`s.

use crate::blend::fillet_custom_solver::{offset_evaluate_1st, ConstantRadiusAssistedFs};
use crate::blend::fillet_geom::p1::{set_rail_curve, FilletBrep, FilletGeomId};
use crate::blend::fillet_solver::p1::{compute_uv_vectors, FilletSolver};
use crate::blend::fillet_solver::p3::StepSolveOutcome;
use crate::blend::fillet_standard_solver::p1::FilletLaw;
use crate::curve::crv_on_surf::CrvOnSurf;
use crate::curve::curve::p3::PointSolveOptions;
use crate::curve::curve::Curve;
use crate::curve::hermite_curve::HermiteCurve;
use crate::error::{Result, SolidError};
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::geom_utility::p1::line_closest_point;
use crate::geom::global_solver::SolverOperation;
use crate::geom::solution_array::SolutionObject;
use crate::geom::vector2d::{Point2, Vec2};
use crate::geom::vector3d::{sqrt, Point3, Vec3};
use crate::intersect::adv_surface_intersector::AdvSurfaceIntersector;
use crate::intersect::surface_intersector::p1::{
    IntersectionPointType, SpanAccuracy, SrfIndex, SurfaceIntersector, TsectId, TsectPnt,
};
use crate::intersect::surface_intersector::p2::TsectCurves;
use crate::intersect::surface_intersector::p3::NextPoint;
use crate::intersect::surface_intersector::p4::{
    do_law_intersection_with, do_point_intersection_with, IntersectionCurve, IntersectorVirtuals,
    LawIntersectionOptions, PointIntersectionOptions,
};
use crate::kernel::Kernel;
use crate::math::{max2, max4, min2, EFF_ZERO, EFF_ZERO_SQ};
use crate::nurbs::b_spline_curve::{BSplineCurve, BSplineCurveForm, CanonicalData, KnotType};
use crate::nurbs::b_spline_surface::BSplineSurface;
use crate::surface::surface::p2::{drop_vectors, TsectCurveType};
use crate::surface::surface::Surface;
use crate::topology::edgeuse::p1 as eu1;
use crate::topology::{edge, EdgeId};

const OP_VALUES: &str = "blend::fillet_intersector::FilletIntersector::compute_point_values";
const OP_DELTA: &str = "blend::fillet_intersector::FilletIntersector::compute_point_values_delta";
const OP_SPAN: &str = "blend::fillet_intersector::FilletIntersector::test_span_accuracy";
const OP_NEXT: &str = "blend::fillet_intersector::FilletIntersector::compute_next_point";
const OP_FLUSH: &str = "blend::fillet_intersector::FilletIntersector::flush_curve";

/// `SOLV_BASE_TYPE`.
const SOLV_BASE_TYPE: i64 = 19000;

/// `SmFilletIntersector_TYPE` (`SOLV_BASE_TYPE + 52`).
pub const TYPE: i64 = SOLV_BASE_TYPE + 52;

/// `NUM_TEST_POINTS` of the span tests.
const NUM_TEST_POINTS: usize = 5;

/// `dParamsToTest` of the span tests.
const PARAMS_TO_TEST: [f64; NUM_TEST_POINTS] = [0.5, 0.17, 0.83, 0.33, 0.67];

/// The two surfaces, in upstream index order.
const SRFS: [SrfIndex; 2] = [SrfIndex::First, SrfIndex::Second];

// ---- co-ported enums ------------------------------------------------------------------------

/// The filleting status `SmFilStatus` is unit `blend.fillet_geom_p1`'s
/// [`FilStatus`] (one type per class).
pub use crate::blend::fillet_geom::p1::FilStatus;

/// What a self-intersection handler does with self-intersecting fillets
/// (`SmHandlingAlgorithmType`, CO-PORTED from unit `blend.fillet_solver_p1`);
/// discriminants are the upstream values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HandlingAlgorithmType {
    /// `SM_HA_UNKNOWN`.
    Unknown = 0,
    /// `SM_HA_SMOOTHING`: smooth out the self intersections.
    Smoothing = 1,
    /// `SM_HA_BLENDING`: insert a blending patch.
    Blending = 2,
}

// ---- the solver interface -------------------------------------------------------------------

/// The flags `SmFilletSolver::StepSolve` returns besides the next point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StepSolve {
    /// `rbFoundAnswer`: the Newton solve found the next point.
    pub found_answer: bool,
    /// `rbClipped`: the step was shortened to keep the point in both domains.
    pub clipped: bool,
    /// `rbBoundaryHit`: the step leaves a domain.
    pub boundary_hit: bool,
}

/// The fillet solver as `SmFilletIntersector` uses it (`SmFilletSolver& m_rFilletSolver`
/// and the objects reached through it): an interface CO-PORTED for units
/// `blend.fillet_solver_p1` to `_p4`, `blend.fillet_geom_p1` and the fillet surface
/// generators. Every default method returns [`SolidError::Unported`] tagged with the
/// unit owning the upstream symbol; a solver overrides what is ported.
pub trait FilletSolverOps {
    /// The fillet geometry handle (`SmFilletGeom*`, unit `blend.fillet_geom_p1`).
    type FilletGeom: Clone + core::fmt::Debug;

    /// `ComputePointValues(aUVValues, crUVDomain1, crUVDomain2, dCurveTraceDirection,
    /// rTsectPnt, pOptPreviousPnt)`: the point, derivatives and type of the
    /// intersection point at the rail UV points.
    ///
    /// # Errors
    ///
    /// The solver's failure. A non-virtual `SmFilletSolver` member (unit
    /// `blend.fillet_solver_p4`), so there is no default: a solver over the
    /// `SmFilletSolver` base implements it with [`FilletSolver::compute_point_values`].
    #[allow(clippy::too_many_arguments)]
    fn compute_point_values(
        &mut self,
        k: &Kernel,
        uv_values: [Point2; 2],
        uv_domain1: &Extent2d,
        uv_domain2: &Extent2d,
        curve_trace_direction: f64,
        point: &mut TsectPnt,
        previous: Option<&TsectPnt>,
    ) -> Result<()>;

    /// `ComputeSurfaceValues(aUVValues, rTsectPnt)`: the surface and curve point and
    /// derivative values at the rail UV points.
    ///
    /// # Errors
    ///
    /// The solver's failure.
    fn compute_surface_values(
        &mut self,
        k: &Kernel,
        uv_values: [Point2; 2],
        point: &mut TsectPnt,
    ) -> Result<()>;

    /// `ComputeUVVectors(rTsectPnt)` (non-virtual): the UV tangents of the point
    /// ([`crate::blend::fillet_solver::p1::compute_uv_vectors`]).
    ///
    /// # Errors
    ///
    /// The solver's failure.
    fn compute_uv_vectors(&mut self, k: &Kernel, point: &mut TsectPnt) -> Result<()>;

    /// `StepSolve(rTsectPnt, rdStep, crUVDomain1, crUVDomain2, rbFoundAnswer,
    /// rbClipped, rbBoundaryHit, rNextTSP)`: the UV points of the next point, written
    /// to `next`. `step_size` is in/out (`double& rdStep`): a clipped step is
    /// rescaled to the point found. The flags are written to `flags` as upstream
    /// writes them, so they keep what was set before a failure.
    ///
    /// # Errors
    ///
    /// The solver's failure (`FilletSolver::step_solve`, unit `blend.fillet_solver_p3`).
    #[allow(clippy::too_many_arguments)]
    fn step_solve(
        &mut self,
        k: &Kernel,
        point: &TsectPnt,
        step_size: &mut f64,
        uv_domain1: &Extent2d,
        uv_domain2: &Extent2d,
        flags: &mut StepSolve,
        next: &mut TsectPnt,
    ) -> Result<()>;

    /// `PointOnPlaneSolve(crPlaneOrig, crPlaneNormal, crUVDomain1, crUVDomain2, crUV1,
    /// crUV2, rbFoundSolution, rTsectPnt)`: the fillet point on the plane near the
    /// guesses, written to `point`; the result is `rbFoundSolution`.
    ///
    /// # Errors
    ///
    /// The solver's failure. A virtual: each implementer dispatches it on its own
    /// class (the base `SmFilletSolver::PointOnPlaneSolve` is
    /// [`crate::blend::fillet_solver::p1::FilletSolver::point_on_plane_solve`]).
    #[allow(clippy::too_many_arguments)]
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
    ) -> Result<bool>;

    /// `SetSurfacesToZeroOffset()`: saves the offsets of the solver's offset surfaces
    /// and sets them to zero (`void` upstream).
    ///
    /// # Errors
    ///
    /// The solver's failure.
    fn set_surfaces_to_zero_offset(&mut self) -> Result<()>;

    /// `ReloadSurfaceOffsets()`: restores the saved offsets (`void` upstream).
    ///
    /// # Errors
    ///
    /// The solver's failure.
    fn reload_surface_offsets(&mut self) -> Result<()>;

    /// `GetSurface(rail)->Evaluate1stDerivatives(uv, TRUE, TRUE, ..)`: the point and
    /// first partials of the rail's offset surface.
    ///
    /// # Errors
    ///
    /// The evaluation's failure.
    fn rail_surface_evaluate_1st_derivatives(
        &mut self,
        k: &Kernel,
        rail: usize,
        uv: Point2,
    ) -> Result<(Point3, Vec3, Vec3)>;

    /// `SetStatus(eStatus)`.
    ///
    /// # Errors
    ///
    /// The solver's failure.
    fn set_status(&mut self, status: FilStatus) -> Result<()>;

    /// `GetLastFilletGeom()`.
    ///
    /// # Errors
    ///
    /// The solver's failure (no fillet geometry).
    fn last_fillet_geom(&self) -> Result<Self::FilletGeom>;

    /// `GetSelfIntersectionHandler()` and its `GetAlgorithmType()`: `None` without a
    /// handler.
    ///
    /// # Errors
    ///
    /// The solver's failure (no executive to hold a handler);
    /// [`SolidError::Unported`] until the executive is ported.
    fn self_intersection_handler(&self) -> Result<Option<HandlingAlgorithmType>>;

    /// The handler's `SmoothOutIntersection(pFilletGeom, r3DCurves, rUVCurves1,
    /// rUVCurves2)`: joins the new curves to the fillet geometry's previous section
    /// (stored in `pseudo`).
    ///
    /// # Errors
    ///
    /// The handler's failure.
    fn smooth_out_intersection(
        &mut self,
        k: &Kernel,
        pseudo: &mut FilletBrep,
        geom: &Self::FilletGeom,
        curves_3d: &mut Vec<BSplineCurve>,
        uv_curves1: &mut Vec<BSplineCurve>,
        uv_curves2: &mut Vec<BSplineCurve>,
    ) -> Result<()>;

    /// Whether the solver has a fillet surface generator (`m_pFSG != NULL`).
    ///
    /// # Errors
    ///
    /// The solver's failure.
    fn has_surface_generator(&self) -> Result<bool>;

    /// `m_pFSG->CreateSurface(*this, pCenterLine, pCurve1, pCurve2, sPnts, pSur[0],
    /// pUV1, pSur[1], pUV2, pFilletSurface)`: the fillet surface (the generator may
    /// change the curves in place).
    ///
    /// # Errors
    ///
    /// The generator's failure; [`SolidError::Unported`] for a generator of an
    /// unported unit.
    #[allow(clippy::too_many_arguments)]
    fn create_fillet_surface(
        &mut self,
        k: &Kernel,
        center_line: &mut BSplineCurve,
        curve1: &mut BSplineCurve,
        curve2: &mut BSplineCurve,
        points: &[TsectPnt],
        uv1: &mut BSplineCurve,
        uv2: &mut BSplineCurve,
    ) -> Result<Option<BSplineSurface>>;

    /// `pFilletGeom->SetFilletSurface(pFilletSurface)` (after deleting the old one); the
    /// geometry is stored in `pseudo`.
    ///
    /// # Errors
    ///
    /// The geometry lookup's.
    fn set_fillet_surface(
        &mut self,
        pseudo: &mut FilletBrep,
        geom: &Self::FilletGeom,
        surface: Option<BSplineSurface>,
    ) -> Result<()>;

    /// `pFilletGeom->m_pCenterLineCurve = pCenterLine` (after deleting the old one).
    ///
    /// # Errors
    ///
    /// The geometry lookup's.
    fn set_center_line_curve(
        &mut self,
        pseudo: &mut FilletBrep,
        geom: &Self::FilletGeom,
        center_line: BSplineCurve,
    ) -> Result<()>;

    /// `pRail = pFilletGeom->GetRail(rail); pRail->SetCurve(curve, TRUE);
    /// curve->SetOwner(pRail); pRail->SetInterval(curve->GetNaturalInterval())`.
    ///
    /// # Errors
    ///
    /// The geometry lookup's; [`SolidError::InvalidInput`] for a geometry without
    /// that rail (upstream dereferences `NULL`).
    fn set_rail_curve(
        &mut self,
        pseudo: &mut FilletBrep,
        geom: &Self::FilletGeom,
        rail: usize,
        curve: BSplineCurve,
    ) -> Result<()>;

    /// `pRail->GetPrimaryEdgeuse()->GetMate()->SetUVCurve(uv, TRUE);
    /// uv->SetOwner(mate)` for the rail `rail` of the geometry.
    ///
    /// # Errors
    ///
    /// The geometry lookup's; [`SolidError::InvalidInput`] for a missing rail,
    /// primary edge use or mate (upstream dereferences `NULL`).
    fn set_rail_mate_uv_curve(
        &mut self,
        pseudo: &mut FilletBrep,
        geom: &Self::FilletGeom,
        rail: usize,
        uv_curve: BSplineCurve,
    ) -> Result<()>;
}

/// The `SmFilletSolver` members `SmFilletIntersector` reaches through the
/// co-ported constant-radius solver, delegated to the base ([`FilletSolver`], unit
/// `blend.fillet_solver_p1`). The fillet geometry handle is a [`FilletGeomId`] of the
/// pseudo B-rep the geometry writes go to (`blend.fillet_geom_p1`).
impl FilletSolverOps for ConstantRadiusAssistedFs {
    type FilletGeom = FilletGeomId;

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
        // non-virtual in the base (unit blend.fillet_solver_p4); it dispatches on this
        // solver's virtuals (SetupOffsetValues, ..), so the solver itself is passed
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
        // non-virtual in the base; it calls the virtual SetupOffsetValues, which
        // SmConstantRadiusFS overrides (blend.fillet_standard_solver_p1)
        FilletSolver::compute_surface_values(k, self, uv_values, point)
    }

    fn set_surfaces_to_zero_offset(&mut self) -> Result<()> {
        self.solver.set_surfaces_to_zero_offset()
    }

    fn reload_surface_offsets(&mut self) -> Result<()> {
        self.solver.reload_surface_offsets()
    }

    fn compute_uv_vectors(&mut self, _k: &Kernel, point: &mut TsectPnt) -> Result<()> {
        // SmFilletSolver::ComputeUVVectors reads only the intersection point
        compute_uv_vectors(point)
    }

    /// `SmConstantRadiusAssistedFS::PointOnPlaneSolve` (unit
    /// `blend.fillet_standard_solver_p2`). The intersector holds no B-rep: a solver with
    /// edge uses fails in `LoadInitialValues` ([`SolidError::InvalidInput`]).
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

    fn step_solve(
        &mut self,
        k: &Kernel,
        point: &TsectPnt,
        step_size: &mut f64,
        uv_domain1: &Extent2d,
        uv_domain2: &Extent2d,
        flags: &mut StepSolve,
        next: &mut TsectPnt,
    ) -> Result<()> {
        // non-virtual in the base; it dispatches on this solver's virtuals
        // (GetSolverType, LoadInitialValues, ...), so the solver itself is passed. The
        // flags are copied back on an error too: upstream sets them in place.
        let mut out = StepSolveOutcome::default();
        let r = FilletSolver::step_solve(
            self, k, None, point, step_size, uv_domain1, uv_domain2, next, &mut out,
        );
        flags.found_answer = out.found_good_point;
        flags.clipped = out.clipped;
        flags.boundary_hit = out.boundary_hit;
        r
    }

    fn set_status(&mut self, status: FilStatus) -> Result<()> {
        self.solver.set_status(status);
        Ok(())
    }

    fn last_fillet_geom(&self) -> Result<FilletGeomId> {
        self.solver.last_fillet_geom()
    }

    fn self_intersection_handler(&self) -> Result<Option<HandlingAlgorithmType>> {
        // the handler is the executive's member: the base reports the missing
        // executive (a NULL dereference upstream)
        self.solver.self_intersection_handler()
    }

    fn smooth_out_intersection(
        &mut self,
        k: &Kernel,
        pseudo: &mut FilletBrep,
        geom: &FilletGeomId,
        curves_3d: &mut Vec<BSplineCurve>,
        uv_curves1: &mut Vec<BSplineCurve>,
        uv_curves2: &mut Vec<BSplineCurve>,
    ) -> Result<()> {
        // FlushCurve calls this only after `self_intersection_handler` found the
        // executive's smoothing handler (without an executive that read has already
        // failed: a NULL dereference upstream), and the only smoothing handler is
        // SmMakeCurveBlendSIH: run its SmoothOutIntersection on this solver
        let handler = self.solver.executive_curve_blend_sih.ok_or_else(|| {
            SolidError::invalid_input(OP_FLUSH, "no smoothing self-intersection handler")
        })?;
        handler.smooth_out_intersection(self, k, pseudo, *geom, curves_3d, uv_curves1, uv_curves2)
    }

    fn has_surface_generator(&self) -> Result<bool> {
        // GetFilletSurfaceGenerator() != NULL
        Ok(self.solver.fillet_surface_generator().is_some())
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
        use crate::blend::fillet_standard_solver::p1::RailSurface;
        // the virtual m_pFSG->CreateSurface(*this, pCenterLine, pCurve1, pCurve2,
        // sPnts, pSur[0], pUV1, pSur[1], pUV2, ..): the base's generator (SM_ERR for
        // the base class); it reads the solver's tolerance and GetSolverType()
        // (SM_FS_CONST_RADIUS_ASSISTED); pSur[i] = GetSurface(i), passed as copies
        let approx_tol_3d = self.solver.this_approx_tol_3d();
        let generator = self
            .solver
            .fillet_surface_generator()
            .ok_or(SolidError::null_pointer(
                "blend::fillet_intersector::create_fillet_surface",
            ))?;
        let sur0 = Surface::from(self.surface(0)?.surface.clone());
        let sur1 = Surface::from(self.surface(1)?.surface.clone());
        generator
            .create_surface(
                k,
                approx_tol_3d,
                crate::blend::fillet_solver::p1::FilletSolverType::ConstRadiusAssisted,
                Some(center_line),
                curve1,
                curve2,
                points,
                RailSurface {
                    surface: Some(&sur0),
                    uv_curve: Some(uv1),
                },
                RailSurface {
                    surface: Some(&sur1),
                    uv_curve: Some(uv2),
                },
            )
            .map(Some)
    }

    fn rail_surface_evaluate_1st_derivatives(
        &mut self,
        k: &Kernel,
        rail: usize,
        uv: Point2,
    ) -> Result<(Point3, Vec3, Vec3)> {
        offset_evaluate_1st(k, self.surface_mut(rail)?, uv)
    }

    fn set_fillet_surface(
        &mut self,
        pseudo: &mut FilletBrep,
        geom: &FilletGeomId,
        surface: Option<BSplineSurface>,
    ) -> Result<()> {
        pseudo
            .geom_mut(*geom)?
            .set_fillet_surface(surface.map(Surface::from));
        Ok(())
    }

    fn set_center_line_curve(
        &mut self,
        pseudo: &mut FilletBrep,
        geom: &FilletGeomId,
        center_line: BSplineCurve,
    ) -> Result<()> {
        pseudo.geom_mut(*geom)?.center_line_curve = Some(Curve::from(center_line));
        Ok(())
    }

    fn set_rail_curve(
        &mut self,
        pseudo: &mut FilletBrep,
        geom: &FilletGeomId,
        rail: usize,
        curve: BSplineCurve,
    ) -> Result<()> {
        let rail_edge = geom_rail(pseudo, *geom, rail)?;
        let interval = curve.natural_interval();
        // SetCurve(curve, TRUE): the old curve is deleted and the change notification
        // clears the edge uses' UV trim curves and cached gaps; SetOwner is identity
        set_rail_curve(pseudo.brep_mut(), rail_edge, Curve::from(curve))?;
        edge::set_interval(pseudo.brep_mut(), rail_edge, &interval)
    }

    fn set_rail_mate_uv_curve(
        &mut self,
        pseudo: &mut FilletBrep,
        geom: &FilletGeomId,
        rail: usize,
        uv_curve: BSplineCurve,
    ) -> Result<()> {
        let rail_edge = geom_rail(pseudo, *geom, rail)?;
        let prim = eu1::primary_edgeuse(pseudo.brep(), rail_edge)?
            .ok_or_else(|| SolidError::invalid_input(OP_FLUSH, "rail without edge use"))?;
        let mate = eu1::mate(pseudo.brep(), prim)?
            .ok_or_else(|| SolidError::invalid_input(OP_FLUSH, "rail edge use without mate"))?;
        pseudo.set_edgeuse_uv_curve(mate, Some(Curve::from(uv_curve)), true, true)
    }
}

/// The solver members `SmMakeCurveBlendSIH::SmoothOutIntersection` reaches when this
/// solver is the intersector's own (no target B-rep: the rail faces' surfaces cannot
/// be read, [`SolidError::InvalidInput`] where upstream reads them).
impl crate::blend::fillet_solver::curve_blend_sih::SmoothingSolver for ConstantRadiusAssistedFs {
    fn original_face_has_surface(&self, _face: crate::topology::FaceId) -> Result<bool> {
        Err(SolidError::invalid_input(
            OP_FLUSH,
            "no target B-rep for the rail faces",
        ))
    }

    fn setup_offset_radius(&mut self, tsect: &TsectPnt) -> Result<f64> {
        let mut radius = 0.0;
        crate::blend::fillet_solver::p1::FilletSolverOps::setup_offset_values(
            self,
            tsect,
            Some(&mut radius),
        )?;
        Ok(radius)
    }

    fn base_natural_uv_domain(&self, rail: usize) -> Result<Extent2d> {
        Ok(own_offset_surface(&self.solver, rail)?
            .base_surface()
            // (kernel-free: a lazy analytic answers its NURB-less base's empty domain)
            .natural_uv_domain_if_built())
    }

    fn rail_surface(&self, rail: usize) -> Result<Surface> {
        Ok(Surface::from(
            own_offset_surface(&self.solver, rail)?.surface.clone(),
        ))
    }

    fn this_approx_tol_3d(&self) -> f64 {
        self.solver.approx_tol_3d
    }
}

/// `GetSurface(rail)` (an `SmOffsetSurface*`; upstream dereferences it unchecked).
fn own_offset_surface(
    solver: &FilletSolver,
    rail: usize,
) -> Result<&crate::blend::fillet_custom_solver::OffsetSurface> {
    solver
        .surface(rail)?
        .map(|slot| &slot.surface)
        .ok_or_else(|| SolidError::invalid_input(OP_FLUSH, "no offset surface"))
}

/// `pFilletGeom->GetRail(rail)`, dereferenced (`NULL` is [`SolidError::InvalidInput`]).
fn geom_rail(pseudo: &FilletBrep, geom: FilletGeomId, rail: usize) -> Result<EdgeId> {
    pseudo
        .geom(geom)?
        .rail(rail)
        .ok_or_else(|| SolidError::invalid_input(OP_FLUSH, "fillet geometry without rail"))
}

// ---- helpers --------------------------------------------------------------------------------

/// A status upstream ignores: `Unported` still propagates.
fn keep_unported<T>(r: Result<T>) -> Result<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(e) if e.is_unported() => Err(e),
        Err(_) => Ok(None),
    }
}

/// `SmCrvOnSurf(uv_curve, *surface, domain, 0, NULL)`.
fn crv_on_surf(
    k: &Kernel,
    uv_curve: HermiteCurve,
    surface: &Surface,
    domain: &Extent2d,
) -> Result<CrvOnSurf> {
    CrvOnSurf::new(
        k,
        Curve::from(uv_curve),
        surface.clone_for_evaluation()?,
        Some(*domain),
        0,
    )
}

/// `sMap.Evaluate(t, 1, TRUE, aPV)`: the position and first derivative.
fn eval1(k: &Kernel, map: &CrvOnSurf, t: f64) -> Result<[Vec3; 2]> {
    let v = map.evaluate(k, t, 1, true, true)?;
    let p = v.first().copied().unwrap_or(Vec3::UNDEF);
    let d = v.get(1).copied().unwrap_or(Vec3::UNDEF);
    Ok([p, d])
}

/// The two Hermite UV span curves `(uv(i), uv'(i) * step) -> (uv(i+1), uv'(i+1) * step)`.
fn uv_span_curves(point: &TsectPnt, next: &TsectPnt, step_size: f64) -> [HermiteCurve; 2] {
    SRFS.map(|s| {
        HermiteCurve::from_2d(
            point.uv_pos(s),
            point.uv_deriv(s) * step_size,
            next.uv_pos(s),
            next.uv_deriv(s) * step_size,
            2,
        )
    })
}

/// A rejected span: a singular point becomes a crossing point.
fn demote_singularity(p: &mut TsectPnt) {
    if p.point_type == IntersectionPointType::Singularity {
        p.point_type = IntersectionPointType::Crossing;
    }
}

/// `sm_FlushCrv`: the piecewise cubic, C0 B-spline of `cntrl_poly` over the unique
/// `knots` (end multiplicity 4, interior 3), appended to `curves`; nothing when there
/// are no knots.
///
/// # Errors
///
/// Those of `SmBSplineCurve::CreateCanonical` (bad dimension, a control point count
/// that does not match the knots).
pub fn flush_crv(
    dimension: u64,
    cntrl_poly: &[Point3],
    knots: &[f64],
    curves: &mut Vec<BSplineCurve>,
) -> Result<()> {
    // check input
    if knots.is_empty() {
        return Ok(());
    }

    // locals for BSplineCurve
    let degree = 3;
    let mut knot_mult = vec![0usize; knots.len()];
    if let Some(first) = knot_mult.first_mut() {
        *first = 4;
    }
    if let Some(last) = knot_mult.last_mut() {
        *last = 4;
    }
    let n = knot_mult.len();
    for m in knot_mult.iter_mut().take(n.saturating_sub(1)).skip(1) {
        *m = 3;
    }

    // build the BSplineCurve
    let new_bsp = BSplineCurve::create_canonical(&CanonicalData {
        dim: dimension,
        degree,
        control_points: cntrl_poly,
        form: BSplineCurveForm::Unspecified,
        knot_multiplicities: &knot_mult,
        knots,
        knot_type: KnotType::Unspecified,
        weights: None,
        trim: None,
    })?;

    // set output
    curves.push(new_bsp);
    Ok(())
}

/// `sm_FindGuessMidTsectPnt`: prorates `curr` and `next` at fraction `f0` into
/// `mid` (curve point, tangent direction with prorated length, UV points and tangents,
/// curve parameter; the deviation is the larger of the two).
pub fn find_guess_mid_tsect_pnt(curr: &TsectPnt, next: &TsectPnt, f0: f64, mid: &mut TsectPnt) {
    let f1 = 1.0 - f0;

    // position:
    *mid.crv_pos_mut() = f1 * curr.crv_pos() + f0 * next.crv_pos();

    // 1st deriv:
    *mid.crv_deriv_mut() = f1 * curr.crv_deriv() + f0 * next.crv_deriv();
    let len = f1 * curr.crv_deriv().length() + f0 * next.crv_deriv().length();
    // status ignored: a zero vector becomes (0, 0, 1)
    *mid.crv_deriv_mut() = mid.crv_deriv().unitize_or_z();
    *mid.crv_deriv_mut() = mid.crv_deriv() * len;

    // Surface uv points:
    for s in SRFS {
        *mid.uv_pos_mut(s) = f1 * curr.uv_pos(s) + f0 * next.uv_pos(s);
        *mid.uv_deriv_mut(s) = f1 * curr.uv_deriv(s) + f0 * next.uv_deriv(s);
    }

    // curve param:
    mid.curve_parameter = f1 * curr.curve_parameter + f0 * next.curve_parameter;

    // deviation: be pessimistic, better safe than sorry.
    mid.deviation = max2(curr.deviation, next.deviation);
}

// ---- the intersector ------------------------------------------------------------------------

/// The fillet intersector (`SmFilletIntersector`): an advanced surface intersector
/// over the (extended) domains of the two offset surfaces, driven by a fillet solver
/// ([`FilletSolverOps`]) passed to each method.
#[derive(Debug, Clone)]
pub struct FilletIntersector<'s, 't, O, G> {
    /// The `SmAdvSurfaceIntersector` base.
    pub base: AdvSurfaceIntersector<'s, 't, O>,
    /// `m_pCurrFilletGeom`: the explicit fillet geometry `FlushCurve` fills (else the
    /// solver's last one).
    curr_fillet_geom: Option<G>,
    /// `m_bCurrFilletTouchBoundary`: the last flushed curve ended on a domain boundary.
    curr_fillet_touch_boundary: bool,
    /// `m_bSelfIntersect` (no longer used upstream).
    self_intersect: bool,
}

impl<'s, 't, O: SolutionObject + 't, G: Clone> FilletIntersector<'s, 't, O, G> {
    /// The intersector of `surface1` over `uv_domain1` and `surface2` over
    /// `uv_domain2` with the fillet geometry `curr_fillet_geom` (upstream default
    /// `NULL`): the advanced intersector built with `bFromFilletIntersector = TRUE`
    /// (the domains may exceed the natural ones), without the boundary point on curve
    /// test (`m_bDoBoundaryPointOnCurveTest = FALSE`).
    ///
    /// # Errors
    ///
    /// Those of [`AdvSurfaceIntersector::new`].
    pub fn new(
        k: &'s Kernel,
        surface1: &'s Surface,
        uv_domain1: &Extent2d,
        surface2: &'s Surface,
        uv_domain2: &Extent2d,
        curr_fillet_geom: Option<G>,
    ) -> Result<Self> {
        let mut base =
            AdvSurfaceIntersector::new(k, surface1, uv_domain1, surface2, uv_domain2, true)?;
        // Will not try to extend boundary intersection point to catch
        // an end point which is also on the boundary. i.e. deactivate this feature
        base.base.do_boundary_point_on_curve_test = false;
        Ok(Self {
            base,
            curr_fillet_geom,
            curr_fillet_touch_boundary: false,
            self_intersect: false,
        })
    }

    /// `GetCurrFilletGeom`.
    #[must_use]
    pub const fn curr_fillet_geom(&self) -> Option<&G> {
        self.curr_fillet_geom.as_ref()
    }

    /// `GetCurrFilletTouchBoundary`.
    #[must_use]
    pub const fn curr_fillet_touch_boundary(&self) -> bool {
        self.curr_fillet_touch_boundary
    }

    /// `IsCurrFilletTouchBoundary`.
    #[must_use]
    pub const fn is_curr_fillet_touch_boundary(&self) -> bool {
        self.curr_fillet_touch_boundary
    }

    /// `GetSelfIntersect`.
    #[must_use]
    pub const fn self_intersect(&self) -> bool {
        self.self_intersect
    }

    /// `SetSelfIntersect`.
    pub fn set_self_intersect(&mut self, is_self_intersect: bool) {
        self.self_intersect = is_self_intersect;
    }

    /// `IsKindOf`: `SmFilletIntersector_TYPE` or a type the advanced intersector is.
    #[must_use]
    pub const fn is_kind_of(&self, t: i64) -> bool {
        t == TYPE || self.base.is_kind_of(t)
    }

    /// `ComputePointValues`: the solver's point values at `uv_values` over the two
    /// domains and the trace direction (`step_size` is unused upstream).
    ///
    /// # Errors
    ///
    /// `SM_ERR` when the solver fails; [`SolidError::Unported`] propagates.
    pub fn compute_point_values<S: FilletSolverOps<FilletGeom = G>>(
        &self,
        solver: &mut S,
        k: &Kernel,
        uv_values: [Point2; 2],
        point: &mut TsectPnt,
        previous: Option<&TsectPnt>,
        _step_size: Option<f64>,
    ) -> Result<()> {
        let si = &self.base.base;
        // pass the call along to the solver
        match solver.compute_point_values(
            k,
            uv_values,
            &si.uv_domains[0],
            &si.uv_domains[1],
            si.curve_trace_direction,
            point,
            previous,
        ) {
            Ok(()) => Ok(()),
            Err(e) if e.is_unported() => Err(e),
            Err(_) => Err(SolidError::sm_err(OP_VALUES)),
        }
    }

    /// `ComputePointValuesDelta`: the surface and curve values of `point` at
    /// `uv_values` and of `delta` at `uv_values_delta`; `point`'s curve tangent becomes
    /// the unit chord to `delta` (oriented with the curve parameters), its type a
    /// crossing, and its UV tangents are computed.
    ///
    /// # Errors
    ///
    /// The solver's; `SM_ERR` for a chord shorter than `SM_EFF_ZERO`.
    pub fn compute_point_values_delta<S: FilletSolverOps<FilletGeom = G>>(
        &self,
        solver: &mut S,
        k: &Kernel,
        uv_values: [Point2; 2],
        uv_values_delta: [Point2; 2],
        point: &mut TsectPnt,
        delta: &mut TsectPnt,
    ) -> Result<()> {
        // given m_pSurface[0,1] UV params - compute Surface pt, Crv pos, and deriv values
        solver.compute_surface_values(k, uv_values, point)?;
        solver.compute_surface_values(k, uv_values_delta, delta)?;

        // get curve[param deltaParam] chord (tangent approximation)
        let mut diff = delta.crv_pos() - point.crv_pos();

        // when needed - negate the chord to match the parameter progression
        if delta.curve_parameter < point.curve_parameter {
            diff = -diff;
        }

        // skip zero length chords
        if diff.length() < EFF_ZERO {
            return Err(SolidError::sm_err(OP_DELTA));
        }

        // unitize and save the chord
        *point.crv_deriv_mut() = diff.unitize()?;

        // set output
        point.point_type = IntersectionPointType::Crossing;

        solver.compute_uv_vectors(k, point)?;

        // all done
        Ok(())
    }

    /// `TestSpanAccuracy`: rejects the span from `point` to `next` when the tangent
    /// turns more than `this_ang_tol_rad` or (on the base surfaces, offsets set to
    /// zero) the Hermite UV spans mapped onto the surfaces stray from the Hermite 3D
    /// spans by more than the angle-adjusted tolerance (both tests only for a step
    /// longer than `100 * SM_EFF_ZERO`). With `mid`, a mid point farther than 100
    /// times the tolerance from the spans also rejects. A rejected span turns a
    /// singular `next` into a crossing point. The deviation is never set (`None`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a zero `next` tangent; `SM_ERR_INVALID_INPUT` for a zero `point`
    /// tangent; those of the evaluations.
    pub fn test_span_accuracy<S: FilletSolverOps<FilletGeom = G>>(
        &mut self,
        solver: &mut S,
        k: &Kernel,
        point: TsectId,
        next: TsectId,
        mid: Option<TsectId>,
    ) -> Result<SpanAccuracy> {
        let si = &self.base.base;
        let tsect = si.point(point)?.clone();
        let mut next_tsp = si.point(next)?.clone();
        let mid_pnt = match mid {
            Some(m) => Some(si.point(m)?.clone()),
            None => None,
        };

        let step_size = next_tsp.curve_parameter - tsect.curve_parameter;

        if next_tsp.crv_deriv().length_squared() < EFF_ZERO_SQ {
            return Err(SolidError::sm_err(OP_SPAN));
        }

        let angle_found_rad = tsect.crv_deriv().angle_between(next_tsp.crv_deriv())?;
        // First do a quick angular test to see if we can reject this span
        if angle_found_rad > si.this_ang_tol_rad && step_size > EFF_ZERO * 100.0 {
            demote_singularity(&mut next_tsp);
            *self.base.base.point_mut(next)? = next_tsp;
            return Ok(SpanAccuracy {
                found_good_point: false,
                deviation_found: None,
                angle_found_rad,
            });
        }

        let [uv_crv0, uv_crv1] = uv_span_curves(&tsect, &next_tsp, step_size);

        // First set surfaces back to the base surfaces if needed - this
        // makes the offsets zero.
        solver.set_surfaces_to_zero_offset()?;

        let s0 = si.surface(SrfIndex::First);
        let s1 = si.surface(SrfIndex::Second);
        let map0 = crv_on_surf(k, uv_crv0, s0, &si.uv_domains[0])?;
        let map1 = crv_on_surf(k, uv_crv1, s1, &si.uv_domains[1])?;

        let ivl0 = map0.natural_interval();
        let [p00, d00] = eval1(k, &map0, ivl0.min())?;
        let [p01, d01] = eval1(k, &map0, ivl0.max())?;
        let crv_3d0 = HermiteCurve::new(p00, d00, p01, d01, 3);
        let ivl1 = map1.natural_interval();
        let [p10, d10] = eval1(k, &map1, ivl1.min())?;
        let [p11, d11] = eval1(k, &map1, ivl1.max())?;
        let crv_3d1 = HermiteCurve::new(p10, d10, p11, d11, 3);

        // Utilize the angle between the normals to adjust the tolerance.
        let min_tan_angle = min2(
            tsect.tangent_plane_angle_rad,
            next_tsp.tangent_plane_angle_rad,
        );
        let mut adjusted_tolerance = 2.0 * si.this_approx_tol_3d * (min_tan_angle / 2.0).sin();
        // Don't let the adjusted tolerance get to be less then 1/10th of the actual
        // tolerance.
        adjusted_tolerance = max2(adjusted_tolerance, si.this_approx_tol_3d / 1.0);
        // Don't let points get farther away than the tolerance either
        adjusted_tolerance = min2(si.this_approx_tol_3d, adjusted_tolerance);

        let tol_sq = adjusted_tolerance * adjusted_tolerance;
        if let Some(mid_pnt) = &mid_pnt {
            let test_mid1 = s0.evaluate_point(k, mid_pnt.uv_pos(SrfIndex::First))?;
            let test_mid2 = s1.evaluate_point(k, mid_pnt.uv_pos(SrfIndex::Second))?;
            let c0 = Curve::from(crv_3d0);
            let c1 = Curve::from(crv_3d1);

            let (found, sol) = c0.local_point_solve::<()>(
                k,
                &c0.natural_interval(),
                SolverOperation::Minimize,
                test_mid1,
                &PointSolveOptions::default(),
                0.5,
            )?;
            let t0 = if found { sol.start.param(0)? } else { 0.5 };
            let crv_pnt0 = crv_3d0.evaluate_point(t0);

            let (found, sol) = c1.local_point_solve::<()>(
                k,
                &c1.natural_interval(),
                SolverOperation::Minimize,
                test_mid2,
                &PointSolveOptions::default(),
                0.5,
            )?;
            let t1 = if found { sol.start.param(0)? } else { 0.5 };
            let crv_pnt1 = crv_3d1.evaluate_point(t1);

            let mut big_tol_sq = adjusted_tolerance * 100.0;
            big_tol_sq = big_tol_sq * big_tol_sq;
            let dist1 = crv_pnt0.distance_squared(test_mid1);
            let dist2 = crv_pnt1.distance_squared(test_mid2);
            if dist1 > big_tol_sq || dist2 > big_tol_sq {
                solver.reload_surface_offsets()?;
                return Ok(SpanAccuracy {
                    found_good_point: false,
                    deviation_found: None,
                    angle_found_rad,
                });
            }
        }

        for &param in &PARAMS_TO_TEST {
            let uv3d0 = uv_crv0.evaluate_point(param);
            let uv3d1 = uv_crv1.evaluate_point(param);
            let uv0 = si.uv_domains[0].clamp_point(Point2::new(uv3d0.x, uv3d0.y));
            let uv1 = si.uv_domains[1].clamp_point(Point2::new(uv3d1.x, uv3d1.y));
            let pnt0 = s0.evaluate_point(k, uv0)?;
            let pnt1 = s1.evaluate_point(k, uv1)?;
            let crv_pnt0 = crv_3d0.evaluate_point(param);
            let crv_pnt1 = crv_3d1.evaluate_point(param);
            let dist_sq = pnt0.distance_squared(crv_pnt0);
            let dist_sq2 = pnt1.distance_squared(crv_pnt1);
            let dist_sq = max2(dist_sq, dist_sq2);
            if dist_sq > tol_sq && step_size > EFF_ZERO * 100.0 {
                demote_singularity(&mut next_tsp);
                *self.base.base.point_mut(next)? = next_tsp;
                solver.reload_surface_offsets()?;
                return Ok(SpanAccuracy {
                    found_good_point: false,
                    deviation_found: None,
                    angle_found_rad,
                });
            }
        }

        solver.reload_surface_offsets()?;
        Ok(SpanAccuracy {
            found_good_point: true,
            deviation_found: None,
            angle_found_rad,
        })
    }

    /// `ComputeNextPoint`: steps from the crossing point `prev` by `step_size` with
    /// the solver's `StepSolve`, evaluates the new point into `next`, and accepts it
    /// unless it is not a crossing, its tangent is zero or turns too much (on a step
    /// over `1000 * SM_EFF_ZERO` with distinct points), it lies behind `prev`, or the
    /// Hermite UV spans mapped onto the base surfaces stray from the Hermite 3D spans
    /// by more than `this_approx_tol_3d` (ten times that when the derivatives were
    /// recomputed after a through-point clip). A point that is not a crossing has no
    /// next point.
    ///
    /// # Errors
    ///
    /// Those of the angle and derivative computations and the evaluations; a `next`
    /// that is `prev` is [`SolidError::InvalidInput`] (two aliases of one point)
    /// when that point is a crossing (any other type returns no point, as upstream).
    pub fn compute_next_point<S: FilletSolverOps<FilletGeom = G>>(
        &mut self,
        solver: &mut S,
        k: &Kernel,
        prev: TsectId,
        step_size: f64,
        next: TsectId,
    ) -> Result<NextPoint> {
        if prev == next {
            // Upstream may alias `rTsectPnt` and `rNextTSP`: a point that is not a
            // crossing returns at once with every flag false, before either is
            // touched. A crossing would be written while it is read; refuse it.
            if self.base.base.point(prev)?.point_type != IntersectionPointType::Crossing {
                return Ok(NextPoint {
                    found_good_point: false,
                    deviation_found: None,
                    angle_found_rad: None,
                    clipped: false,
                    boundary_hit: false,
                });
            }
            return Err(SolidError::invalid_input(
                OP_NEXT,
                "the next point is the previous point",
            ));
        }
        let mut tsect = self.base.base.point(prev)?.clone();
        let mut next_tsp = self.base.base.point(next)?.clone();
        let out = self.compute_next_point_on(solver, k, &mut tsect, step_size, &mut next_tsp);
        *self.base.base.point_mut(prev)? = tsect;
        *self.base.base.point_mut(next)? = next_tsp;
        out
    }

    /// The body of [`Self::compute_next_point`] on detached points.
    #[allow(clippy::too_many_lines)]
    fn compute_next_point_on<S: FilletSolverOps<FilletGeom = G>>(
        &self,
        solver: &mut S,
        k: &Kernel,
        tsect: &mut TsectPnt,
        step_size: f64,
        next_tsp: &mut TsectPnt,
    ) -> Result<NextPoint> {
        let si = &self.base.base;
        // init output
        let mut out = NextPoint {
            found_good_point: false,
            deviation_found: None,
            angle_found_rad: None,
            clipped: false,
            boundary_hit: false,
        };

        // Bottom line, SM_IP_CROSSING is the only case that can have a next point.
        if tsect.point_type != IntersectionPointType::Crossing {
            return Ok(out);
        }

        // `dStepSize` is passed to StepSolve by reference: a clipped step comes back
        // rescaled and every later use below sees the new value.
        let mut step_size = step_size;

        // solve for the next SmTsectPt - save step output values
        let mut step = StepSolve::default();
        if keep_unported(solver.step_solve(
            k,
            tsect,
            &mut step_size,
            &si.uv_domains[0],
            &si.uv_domains[1],
            &mut step,
            next_tsp,
        ))?
        .is_none()
        {
            step.found_answer = false;
        }

        // Set return values
        out.clipped = step.clipped;
        out.boundary_hit = step.boundary_hit;

        // quit when NR iteration can't find a next step value
        if !step.found_answer {
            return Ok(out);
        }

        // compute rNextTSP point values -
        let uvs = [
            next_tsp.uv_pos(SrfIndex::First),
            next_tsp.uv_pos(SrfIndex::Second),
        ];
        let stat =
            self.compute_point_values(solver, k, uvs, next_tsp, Some(tsect), Some(step_size));

        // stop criteria: fail to compute derivs when exiting a surface boundary - trace is done
        if keep_unported(stat)?.is_none() {
            out.boundary_hit = true; // causes termination of current tracing
            return Ok(out);
        }

        // stop criteria: next point is a tangent or coincident point - trace is done
        if next_tsp.point_type == IntersectionPointType::TangentPoint
            || next_tsp.point_type == IntersectionPointType::Coincidence
        {
            return Ok(out);
        }

        // stop criteria: next point stepsize is zero
        if next_tsp.crv_deriv().length_squared() < EFF_ZERO_SQ {
            return Ok(out);
        }

        // (not upstream, `fixes.fillet_trace_rejects_short_step`, WR-503) a point far short of the
        // step is another root of the equations, not the next point of this trace
        if k.fixes.fillet_trace_rejects_short_step
            && !step.clipped
            && tsect.crv_pos().distance(next_tsp.crv_pos()) < 0.5 * step_size.abs()
        {
            demote_singularity(next_tsp);
            return Ok(out);
        }

        // get change in 3dCurve angle for this step
        let angle_found_rad = tsect.crv_deriv().angle_between(next_tsp.crv_deriv())?;
        out.angle_found_rad = Some(angle_found_rad);

        // when the change in tangent angle is too large -
        //   reject span over a large enough stepSize,
        //   accept span to on small steps to stop endless recursion.
        if angle_found_rad > si.this_ang_tol_rad {
            // (upstream: an empty check of nearly 180 degree turns)

            // if step size is still large enough to split
            if step_size > EFF_ZERO * 1000.0 {
                // failing point when sample pts are distinct - stop
                let tol = EFF_ZERO * 1000.0 * (1.0 + next_tsp.crv_pos().max_dimension());
                if tsect.crv_pos().distance(next_tsp.crv_pos()) > tol {
                    demote_singularity(next_tsp);
                    return Ok(out);
                }
            }
        }

        // Also make sure that the point is going in the same direction
        let line_param = line_closest_point(
            Vec3::from(tsect.uv_pos(SrfIndex::First)),
            Vec3::from(tsect.uv_deriv(SrfIndex::First)),
            Vec3::from(next_tsp.uv_pos(SrfIndex::First)),
        );
        if line_param < 0.0 {
            demote_singularity(next_tsp);
            return Ok(out);
        }

        // set next SmTsectPt parameter values = lastParameter + dStepSize
        next_tsp.curve_parameter = tsect.curve_parameter + step_size;

        // But first - set deriviatves with finite differences
        //   when m_bClippedByThroughPoint == TRUE
        //   and the magnitude of either uv derivative has changed substantially
        let mut adjust_derivatives = false;
        if si.clipped_by_through_point {
            for which_surf in SRFS {
                let start_uv_len = tsect.uv_deriv(which_surf).length();
                let end_uv_len = next_tsp.uv_deriv(which_surf).length();

                if start_uv_len > 3.0 * end_uv_len || end_uv_len > 3.0 * start_uv_len {
                    // Compute derivatives by making a small step
                    let mut small_step = TsectPnt::new();
                    let mut d_step = 1.0e-5;
                    let mut small_flags = StepSolve::default();
                    solver.step_solve(
                        k,
                        tsect,
                        &mut d_step,
                        &si.uv_domains[0],
                        &si.uv_domains[1],
                        &mut small_flags,
                        &mut small_step,
                    )?;
                    let found_good = small_flags.found_answer;

                    if found_good {
                        // Got uv's, fill in the rest.
                        let uv_array = [
                            next_tsp.uv_pos(SrfIndex::First),
                            next_tsp.uv_pos(SrfIndex::Second),
                        ];
                        if keep_unported(self.compute_point_values(
                            solver,
                            k,
                            uv_array,
                            &mut small_step,
                            None,
                            None,
                        ))?
                        .is_none()
                        {
                            break; // If either doesn't work, forget this idea.
                        }

                        // Set the deriv of the 3d curve using finite difference.
                        let diff = small_step.crv_pos() - tsect.crv_pos();
                        *tsect.crv_deriv_mut() = diff / d_step;
                        // SER(Unitize()): a zero vector is left as (0, 0, 1)
                        let unit = tsect.crv_deriv().unitize();
                        *tsect.crv_deriv_mut() = tsect.crv_deriv().unitize_or_z();
                        unit?;

                        // Set the derivs of the uv curves on both surfaces.
                        for j_srf in SRFS {
                            let dropped = drop_vectors(
                                small_step.srf_du(j_srf),
                                small_step.srf_dv(j_srf),
                                &[tsect.crv_deriv()],
                            )?;
                            let new_deriv = dropped.first().copied().unwrap_or(Vec2::UNDEF);

                            // Set it only if shorter.  (?)
                            let orig_len = tsect.uv_deriv(j_srf).length();
                            if new_deriv.length() < orig_len {
                                *tsect.uv_deriv_mut(j_srf) = new_deriv;
                            }
                        }
                        adjust_derivatives = true; // Remember for later.
                    }

                    // If we did this for the first surf,
                    // no need to do it again for the second.
                    break;
                }
            }
        }

        // create the Hermite UVTrimCurves
        let [uv_crv0, uv_crv1] = uv_span_curves(tsect, next_tsp, step_size);

        // Set OffsetSurface offset distance to zero to get baseSurface evaluations.
        solver.set_surfaces_to_zero_offset()?;

        // use UVTrimCurves on baseSurfaces to define 3d railCurves for this step
        let s0 = si.surface(SrfIndex::First);
        let s1 = si.surface(SrfIndex::Second);
        let map0 = crv_on_surf(k, uv_crv0, s0, &si.uv_domains[0])?;
        let map1 = crv_on_surf(k, uv_crv1, s1, &si.uv_domains[1])?;

        let crv_3d0 = rail_hermite(k, &map0, adjust_derivatives)?;
        let crv_3d1 = rail_hermite(k, &map1, adjust_derivatives)?;

        let mut adjusted_tolerance = si.this_approx_tol_3d;
        if adjust_derivatives {
            adjusted_tolerance *= 10.0;
        }
        let tol_sq = adjusted_tolerance * adjusted_tolerance;

        // for every test point
        let mut max_dist_sq = 0.0;
        for &param in &PARAMS_TO_TEST {
            // get UVPnts from UVTrimCurves clamped to surf domain
            let uv3d0 = uv_crv0.evaluate_point(param);
            let uv3d1 = uv_crv1.evaluate_point(param);
            let uv0 = si.uv_domains[0].clamp_point(Point2::new(uv3d0.x, uv3d0.y));
            let uv1 = si.uv_domains[1].clamp_point(Point2::new(uv3d1.x, uv3d1.y));

            // evaluate the associated surface points
            let pnt0 = s0.evaluate_point(k, uv0)?;
            let pnt1 = s1.evaluate_point(k, uv1)?;

            // evaluate the associated 3d RailCurve points
            let crv_pnt0 = crv_3d0.evaluate_point(param);
            let crv_pnt1 = crv_3d1.evaluate_point(param);

            // get distance between 3dCurve(param) points and Surf(UVTrimCurve(param)) points
            let dist_sq = pnt0.distance_squared(crv_pnt0);
            let dist_sq2 = pnt1.distance_squared(crv_pnt1);
            let dist_sq = max2(dist_sq, dist_sq2);

            // when max dDistSq value is too large on a long enough step - reject this span
            if dist_sq > tol_sq && step_size > EFF_ZERO * 100.0 {
                demote_singularity(next_tsp);
                solver.reload_surface_offsets()?;
                return Ok(out);
            }

            // save the max deviation
            if dist_sq > max_dist_sq {
                max_dist_sq = dist_sq;
            }
        }

        // set output and return FoundGoodPoint == TRUE
        out.deviation_found = Some(sqrt(max_dist_sq));
        out.found_good_point = true; // passed test
        solver.reload_surface_offsets()?;
        Ok(out)
    }

    /// `FlushCurve`: takes the curve points (clearing the list), notes whether the
    /// last one is on a domain boundary ([`Self::curr_fillet_touch_boundary`]), and
    /// with at least two points builds the centre line, the rail UV curves and the
    /// rail 3D curves (one cubic Bézier span per point span, spans first equalised to
    /// within a factor 2.8 by solver mid points, a tiny last span merged), appends the
    /// curve type and deviation to `out`, lets the self-intersection handler smooth
    /// them, builds the fillet surface and stores everything in the current (else the
    /// solver's last) fillet geometry.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` without curve points; `SM_ERR` when a mid point has no
    /// solution or the handler or surface generator fails (after setting the solver
    /// status); those of the solver and the curve construction.
    #[allow(clippy::too_many_lines)]
    pub fn flush_curve<S: FilletSolverOps<FilletGeom = G>>(
        &mut self,
        solver: &mut S,
        k: &Kernel,
        pseudo: &mut FilletBrep,
        out: &mut TsectCurves,
    ) -> Result<()> {
        // 1. offset surface sample points
        let ids = self.base.base.curve_points().to_vec();
        let mut pnts = ids
            .iter()
            .map(|&id| self.base.base.point(id).cloned())
            .collect::<Result<Vec<_>>>()?;

        // 2. Remember if the end of current fillet touched a boundary on
        //    either surface UVDomain
        let last_tsect_pnt = pnts.last().ok_or(SolidError::null_pointer(OP_FLUSH))?;
        self.curr_fillet_touch_boundary = false;

        // for both surfaces
        for (s, dom) in SRFS.into_iter().zip(self.base.base.uv_domains.iter()) {
            // get tolerance proportional to max UV coordinate value
            let size = max4(
                dom.min().x.abs(),
                dom.min().y.abs(),
                dom.max().x.abs(),
                dom.max().y.abs(),
            );
            let tol = EFF_ZERO * (1.0 + size) * 100.0;

            // remember if lastTsectPnt is on this surface's (extended) uv domain boundary
            if dom.is_point_on_boundary(last_tsect_pnt.uv_pos(s), tol).0 {
                self.curr_fillet_touch_boundary = true;
                break;
            }
        }

        // Clear out curve point list that's been copied into sPnts
        self.base.base.curve_points.clear();

        // no work - not enough xsect points to flush a curve
        if pnts.len() < 2 {
            return Ok(());
        }

        let uv_domains = self.base.base.uv_domains;
        let curve_trace_direction = self.base.base.curve_trace_direction;

        // new curve locals
        let mut knots: Vec<f64> = Vec::new();
        let mut ctrl_pts_3d: Vec<Point3> = Vec::new();
        let mut uv_ctrl_pts: [Vec<Point3>; 2] = [Vec::new(), Vec::new()];
        let mut rail_ctrl_pts_3d: [Vec<Point3>; 2] = [Vec::new(), Vec::new()];

        let mut curve_type = TsectCurveType::Crossing;
        let mut max_deviation = 0.0;
        let mut max_angle = 0.0;

        // 3. If the final span is tiny, remove it by consolidating it with
        // the previous span.
        if pnts.len() > 2 {
            let n = pnts.len();
            let lt = pnts.get(n - 1).map_or(0.0, |p| p.curve_parameter);
            let nlt = pnts.get(n - 2).map_or(0.0, |p| p.curve_parameter);
            let nnlt = pnts.get(n - 3).map_or(0.0, |p| p.curve_parameter);
            let ratio = (nlt - nnlt) / (lt - nlt);
            if ratio > 10.0 {
                // If the ratio is too big remove the next to last point
                if let Some(last) = pnts.pop() {
                    if let Some(slot) = pnts.last_mut() {
                        *slot = last;
                    }
                }
            }
        }

        // 4. Build array of knots
        for p in &pnts {
            knots.push(p.curve_parameter);
        }

        // 5. add knots so that each span is similiarly sized
        let mut done = false;
        let maximum_delta_factor = 2.8;
        let orig_num_knots = knots.len();

        // continue to iter until spans are uniformly sized
        while !done {
            done = true;

            // end condition: if knot count is more than tripled
            if knots.len() as f64 > 3.0 * orig_num_knots as f64 {
                break;
            }

            // iter every span
            let mut j = 0usize;
            while j + 1 < knots.len() {
                let kj = knots.get(j).copied().unwrap_or(0.0);
                let kj1 = knots.get(j + 1).copied().unwrap_or(0.0);

                // see if this span should be split
                let mut split = false;
                let span_size = kj1 - kj;

                if j > 0 {
                    // Look at size of previous span
                    let last_span_size = kj - knots.get(j - 1).copied().unwrap_or(0.0);
                    if span_size / last_span_size > maximum_delta_factor {
                        split = true;
                    }
                }

                if j < knots.len() - 2 {
                    // Look at size of next span
                    let next_span_size = knots.get(j + 2).copied().unwrap_or(0.0) - kj1;
                    if span_size / next_span_size > maximum_delta_factor {
                        split = true;
                    }
                }

                if !split {
                    j += 1;
                    continue;
                }

                // Split the span at the midpoint. [Fillet reg 2:229]
                let frac = 0.5;

                // add a mid-span knot value to knot array
                done = false;
                let split_knot = (1.0 - frac) * kj + frac * kj1;
                knots.insert(j + 1, split_knot);

                // 5a. Find 3D intersection point for new knot value
                let (pj, pj1) = match (pnts.get(j), pnts.get(j + 1)) {
                    (Some(a), Some(b)) => (a.clone(), b.clone()),
                    _ => {
                        return Err(SolidError::invalid_input(
                            OP_FLUSH,
                            "more knots than curve points",
                        ))
                    }
                };
                let mut mid_pnt = TsectPnt::new();
                find_guess_mid_tsect_pnt(&pj, &pj1, frac, &mut mid_pnt);

                let plane_orig = mid_pnt.crv_pos();
                let uv1 = mid_pnt.uv_pos(SrfIndex::First);
                let uv2 = mid_pnt.uv_pos(SrfIndex::Second);
                let plane_normal = mid_pnt.crv_deriv().unitize()?;

                // find filletPoint on given plane satisfying geometry
                //   requirements implemented in derived SmFilletSolver class
                let found_solution = solver.point_on_plane_solve(
                    k,
                    plane_orig,
                    plane_normal,
                    &uv_domains[0],
                    &uv_domains[1],
                    uv1,
                    uv2,
                    &mut mid_pnt,
                )?;
                if !found_solution {
                    return Err(SolidError::sm_err(OP_FLUSH));
                }

                // save the rail UVCurve points for the new mid-span point
                let uvs = [
                    mid_pnt.uv_pos(SrfIndex::First),
                    mid_pnt.uv_pos(SrfIndex::Second),
                ];

                // get the rail 3DCurve sample values
                solver.compute_point_values(
                    k,
                    uvs,
                    &uv_domains[0],
                    &uv_domains[1],
                    curve_trace_direction,
                    &mut mid_pnt,
                    Some(&pj),
                )?;

                // add the completed mid-span point to the sPnts array
                pnts.insert(j + 1, mid_pnt);
                j += 2;
            }
        }

        // set the stored fillet-surface OffsetDistances to 0.0
        solver.set_surfaces_to_zero_offset()?;

        // 6. for every offset-surface xsect curve sample point span
        // build center-line, rail UVCurves, and rail 3DCurves control-point arrays
        // all with the same parameterization
        for (i, pair) in pnts.windows(2).enumerate() {
            let [tsp_last, tsp_curr] = pair else {
                continue;
            };

            // save max angle between offset surfaces along xsect curve sample points
            max_angle = max2(max_angle, tsp_curr.tangent_plane_angle_rad);

            // If we have a tangent curve set that curve type
            if i == 0 && tsp_curr.point_type == IntersectionPointType::TangentCurve {
                curve_type = TsectCurveType::Tangent;
            }

            // Collect maximum deviation
            if tsp_last.deviation > max_deviation {
                max_deviation = tsp_last.deviation;
            }
            if tsp_curr.deviation > max_deviation {
                max_deviation = tsp_curr.deviation;
            }

            // let dScale = span parameter length
            let scale = tsp_curr.curve_parameter - tsp_last.curve_parameter;

            // for the CenterLine
            let p1 = tsp_last.crv_pos();
            let p4 = tsp_curr.crv_pos();
            let d1 = tsp_last.crv_deriv() * scale;
            let d2 = tsp_curr.crv_deriv() * scale;

            // Hermite to Bezier
            let p2 = p1 + d1 / 3.0;
            let p3 = p4 - d2 / 3.0;

            // for the first span - add the start control point to the s3DCtrlPts array
            if ctrl_pts_3d.is_empty() {
                ctrl_pts_3d.push(p1);
            }

            // now add the span's last 3 control points to the s3DCtrlPts array
            ctrl_pts_3d.push(p2);
            ctrl_pts_3d.push(p3);
            ctrl_pts_3d.push(p4);

            // for both surface rail curves - build the rail curves
            for (rail, s) in SRFS.into_iter().enumerate() {
                let uv1 = tsp_last.uv_pos(s);
                let uv4 = tsp_curr.uv_pos(s);
                let der1 = tsp_last.uv_deriv(s) * scale;
                let der2 = tsp_curr.uv_deriv(s) * scale;

                // Convert from Hermite to Bezier.
                let uv2 = uv1 + der1 / 3.0;
                let uv3 = uv4 - der2 / 3.0;

                let (Some(uv_pts), Some(pts_3d)) =
                    (uv_ctrl_pts.get_mut(rail), rail_ctrl_pts_3d.get_mut(rail))
                else {
                    continue;
                };

                // for the first span - add the start control point
                if uv_pts.is_empty() {
                    uv_pts.push(Vec3::from(uv1));
                }

                // now add the span's last 3 control points
                uv_pts.push(Vec3::from(uv2));
                uv_pts.push(Vec3::from(uv3));
                uv_pts.push(Vec3::from(uv4));

                // Now lift the point and derivatives of UV curves into 3D on the zero offset surface.
                let (pnt, du, dv) =
                    solver.rail_surface_evaluate_1st_derivatives(k, rail, tsp_last.uv_pos(s))?;
                let p13d = pnt;
                let d13d = du * der1.x + dv * der1.y;

                let (pnt, du, dv) =
                    solver.rail_surface_evaluate_1st_derivatives(k, rail, tsp_curr.uv_pos(s))?;
                let p43d = pnt;
                let d23d = du * der2.x + dv * der2.y;

                // Convert from Hermite to Bezier.
                let p23d = p13d + d13d / 3.0;
                let p33d = p43d - d23d / 3.0;
                if pts_3d.is_empty() {
                    pts_3d.push(p13d);
                }

                pts_3d.push(p23d);
                pts_3d.push(p33d);
                pts_3d.push(p43d);
            }
        }

        // 7. convert control-point/knot Arrays into SmBsplineCurves
        let mut curves_3d: Vec<BSplineCurve> = Vec::new();
        let mut surface1_uv_curves: Vec<BSplineCurve> = Vec::new();
        let mut surface2_uv_curves: Vec<BSplineCurve> = Vec::new();
        let [uv_ctrl_pts0, uv_ctrl_pts1] = &uv_ctrl_pts;
        let [rail_pts0, rail_pts1] = &rail_ctrl_pts_3d;
        flush_crv(3, &ctrl_pts_3d, &knots, &mut curves_3d)?;
        flush_crv(2, uv_ctrl_pts0, &knots, &mut surface1_uv_curves)?;
        flush_crv(2, uv_ctrl_pts1, &knots, &mut surface2_uv_curves)?;
        flush_crv(3, rail_pts0, &knots, &mut curves_3d)?;
        flush_crv(3, rail_pts1, &knots, &mut curves_3d)?;
        out.deviations.push(max_deviation);
        out.curve_types.push(curve_type);

        // 8. Select target pFilletGeom to Modify
        let fillet_geom = match &self.curr_fillet_geom {
            Some(g) => g.clone(),
            None => solver.last_fillet_geom()?,
        };

        // 9. when self-intersection checking/correcting is on
        if let Some(algorithm) = solver.self_intersection_handler()? {
            if algorithm == HandlingAlgorithmType::Smoothing {
                let smoothed = solver.smooth_out_intersection(
                    k,
                    pseudo,
                    &fillet_geom,
                    &mut curves_3d,
                    &mut surface1_uv_curves,
                    &mut surface2_uv_curves,
                );
                if keep_unported(smoothed)?.is_none() {
                    solver.set_status(FilStatus::SelfIntHandlingFailure)?;
                    return Err(SolidError::sm_err(OP_FLUSH));
                }
            }
        }

        // get (the potentially modified) rail curves, their UVTrimCurves, and the Centerline
        let mut uv1 = surface1_uv_curves
            .pop()
            .ok_or(SolidError::null_pointer(OP_FLUSH))?;
        let mut uv2 = surface2_uv_curves
            .pop()
            .ok_or(SolidError::null_pointer(OP_FLUSH))?;
        let mut curve2 = curves_3d.pop().ok_or(SolidError::null_pointer(OP_FLUSH))?;
        let mut curve1 = curves_3d.pop().ok_or(SolidError::null_pointer(OP_FLUSH))?;
        let mut center_line = curves_3d.pop().ok_or(SolidError::null_pointer(OP_FLUSH))?;

        // 10. Generate the fillet surface from the rail curves and centerline
        let mut fillet_surface = None;
        if solver.has_surface_generator()? {
            let created = solver.create_fillet_surface(
                k,
                &mut center_line,
                &mut curve1,
                &mut curve2,
                &pnts,
                &mut uv1,
                &mut uv2,
            );
            let Some(s) = keep_unported(created)? else {
                solver.set_status(FilStatus::SurfSkinningFailure)?;
                return Err(SolidError::sm_err(OP_FLUSH));
            };
            fillet_surface = s;
        }

        // 11. Correct self-intersecting fillet-surface: never runs upstream
        //     (bTempTrySelfIntHandler is a FALSE constant).

        // 12. Save Fillet Surface and rail curve data
        solver.set_fillet_surface(pseudo, &fillet_geom, fillet_surface)?;
        solver.set_center_line_curve(pseudo, &fillet_geom, center_line)?;
        solver.set_rail_curve(pseudo, &fillet_geom, 0, curve1)?;
        solver.set_rail_mate_uv_curve(pseudo, &fillet_geom, 0, uv1)?;
        solver.set_rail_curve(pseudo, &fillet_geom, 1, curve2)?;
        solver.set_rail_mate_uv_curve(pseudo, &fillet_geom, 1, uv2)?;

        // restore FilletSolver offset distances in offset Surfaces stored in the FilletSolver
        solver.reload_surface_offsets()?;

        // all done
        Ok(())
    }

    /// `EvaluateLawPoint`: the solver's point on the plane through `plane_origin`
    /// with normal `plane_normal` near `uv0`, `uv1`: its curve parameter (user double
    /// 0) and UV points. The law arguments are unused upstream; whether a solution was
    /// found is not checked (a default point's values are returned).
    ///
    /// # Errors
    ///
    /// The solver's.
    #[allow(clippy::too_many_arguments)]
    pub fn evaluate_law_point<S: FilletSolverOps<FilletGeom = G>, L: ?Sized>(
        &self,
        solver: &mut S,
        k: &Kernel,
        _parameter: f64,
        uv0: Point2,
        uv1: Point2,
        _law: &L,
        _law_orient: bool,
        _point_curve_interval: &Extent1d,
        _surface_orientations: [f64; 2],
        plane_origin: Point3,
        plane_normal: Vec3,
    ) -> Result<(f64, [Point2; 2])> {
        let si = &self.base.base;
        let mut tmp_pnt = TsectPnt::new();

        // find filletPoint on given plane
        let _found_solution = solver.point_on_plane_solve(
            k,
            plane_origin,
            plane_normal,
            &si.uv_domains[0],
            &si.uv_domains[1],
            uv0,
            uv1,
            &mut tmp_pnt,
        )?;

        // set output
        let uvs = [
            tmp_pnt.uv_pos(SrfIndex::First),
            tmp_pnt.uv_pos(SrfIndex::Second),
        ];
        let curve_param = tmp_pnt.user_doubles.first().copied().unwrap_or(0.0);
        Ok((curve_param, uvs))
    }

    /// `DoPointIntersection` (the `SmSurfaceIntersector` method, not overridden) run
    /// on this intersector: the base driver of `intersect::surface_intersector::p4`
    /// with its virtual calls dispatched to this class's overrides ([`FilletDispatch`]):
    /// `ComputePointValues` and `ComputeNextPoint` step through `solver`, and
    /// `FlushCurve` stores the traced curves in the current (else the solver's last)
    /// fillet geometry of `pseudo`.
    ///
    /// # Errors
    ///
    /// As the base `DoPointIntersection`, with the errors of the overrides.
    pub fn do_point_intersection<S: FilletSolverOps<FilletGeom = G>>(
        &mut self,
        solver: &mut S,
        pseudo: &mut FilletBrep,
        k: &Kernel,
        points: &[TsectId],
        opts: &PointIntersectionOptions,
    ) -> Result<IntersectionCurve> {
        do_point_intersection_with(
            &mut FilletDispatch {
                intersector: self,
                solver,
                pseudo,
            },
            k,
            points,
            opts,
        )
    }

    /// `DoLawIntersection` (the `SmSurfaceIntersector` method, not overridden) run on
    /// this intersector: the base driver of `intersect::surface_intersector::p4` with
    /// its virtual calls dispatched to this class's overrides ([`FilletDispatch`]):
    /// `EvaluateLawPoint` solves the solver's point on each section plane (the law is
    /// not evaluated), `ComputePointValuesDelta` and `TestSpanAccuracy` evaluate
    /// through `solver`, and `FlushCurve` stores the traced curve in the current (else
    /// the solver's last) fillet geometry of `pseudo`.
    ///
    /// # Errors
    ///
    /// As the base `DoLawIntersection`, with the errors of the overrides.
    pub fn do_law_intersection<S: FilletSolverOps<FilletGeom = G>>(
        &mut self,
        solver: &mut S,
        pseudo: &mut FilletBrep,
        k: &Kernel,
        points: &[TsectId],
        opts: &LawIntersectionOptions<'_>,
        law: &mut FilletLaw,
    ) -> Result<IntersectionCurve> {
        do_law_intersection_with(
            &mut FilletDispatch {
                intersector: self,
                solver,
                pseudo,
            },
            k,
            points,
            opts,
            law,
        )
    }
}

/// `SmFilletIntersector` seen through its `SmSurfaceIntersector` virtuals: the fillet
/// intersector, the solver its overrides take as an argument and the pseudo B-rep
/// `FlushCurve` writes the fillet geometry into. The base drivers
/// (`DoPointIntersection`, `TraceCurve`, `TestPoint`) run on it as upstream runs them
/// on `*this`.
#[derive(Debug)]
pub struct FilletDispatch<'a, 's, 't, O, G, S> {
    /// The fillet intersector.
    pub intersector: &'a mut FilletIntersector<'s, 't, O, G>,
    /// `m_rFilletSolver`.
    pub solver: &'a mut S,
    /// The pseudo B-rep holding the solver's fillet geometries.
    pub pseudo: &'a mut FilletBrep,
}

impl<'s, 't, O, G, S> IntersectorVirtuals<'s> for FilletDispatch<'_, 's, 't, O, G, S>
where
    O: SolutionObject + 't,
    G: Clone,
    S: FilletSolverOps<FilletGeom = G>,
{
    fn intersector(&self) -> &SurfaceIntersector<'s> {
        &self.intersector.base.base
    }

    fn intersector_mut(&mut self) -> &mut SurfaceIntersector<'s> {
        &mut self.intersector.base.base
    }

    /// [`FilletIntersector::compute_point_values`].
    fn compute_point_values(
        &mut self,
        k: &Kernel,
        uv_values: [Point2; 2],
        point: &mut TsectPnt,
        previous: Option<&TsectPnt>,
        step_size: Option<f64>,
    ) -> Result<()> {
        self.intersector
            .compute_point_values(self.solver, k, uv_values, point, previous, step_size)
    }

    /// [`FilletIntersector::compute_next_point`].
    fn compute_next_point(
        &mut self,
        k: &Kernel,
        prev: TsectId,
        step_size: f64,
        next: TsectId,
    ) -> Result<NextPoint> {
        self.intersector
            .compute_next_point(self.solver, k, prev, step_size, next)
    }

    /// [`FilletIntersector::flush_curve`].
    fn flush_curve(&mut self, k: &Kernel, out: &mut TsectCurves) -> Result<()> {
        self.intersector
            .flush_curve(self.solver, k, self.pseudo, out)
    }

    /// [`FilletIntersector::evaluate_law_point`] (the law arguments are unused).
    fn evaluate_law_point(
        &mut self,
        k: &Kernel,
        parameter: f64,
        uv0: Point2,
        uv1: Point2,
        law: &mut FilletLaw,
        law_orient: bool,
        point_curve_interval: &Extent1d,
        surface_orientations: [f64; 2],
        plane_origin: Point3,
        plane_normal: Vec3,
    ) -> Result<(Option<f64>, [Point2; 2])> {
        let (curve_param, uvs) = self.intersector.evaluate_law_point(
            self.solver,
            k,
            parameter,
            uv0,
            uv1,
            &*law,
            law_orient,
            point_curve_interval,
            surface_orientations,
            plane_origin,
            plane_normal,
        )?;
        Ok((Some(curve_param), uvs))
    }

    /// [`FilletIntersector::compute_point_values_delta`].
    fn compute_point_values_delta(
        &mut self,
        k: &Kernel,
        uv_values: [Point2; 2],
        uv_values_delta: [Point2; 2],
        point: &mut TsectPnt,
        delta: &mut TsectPnt,
    ) -> Result<()> {
        self.intersector.compute_point_values_delta(
            self.solver,
            k,
            uv_values,
            uv_values_delta,
            point,
            delta,
        )
    }

    /// [`FilletIntersector::test_span_accuracy`].
    fn test_span_accuracy(
        &mut self,
        k: &Kernel,
        point: TsectId,
        next: TsectId,
        mid: Option<TsectId>,
    ) -> Result<SpanAccuracy> {
        self.intersector
            .test_span_accuracy(self.solver, k, point, next, mid)
    }
}

/// The 3D Hermite rail span of a UV span on its surface: the end points and
/// derivatives of the mapped curve; with `adjust`, the derivatives taken at the
/// interior parameters `0.001` and `0.999` instead.
fn rail_hermite(k: &Kernel, map: &CrvOnSurf, adjust: bool) -> Result<HermiteCurve> {
    let ivl = map.natural_interval();
    let [p0, mut d0] = eval1(k, map, ivl.min())?;
    let [p1, mut d1] = eval1(k, map, ivl.max())?;

    // If we had to find the uv curve derivs by divided differences (above),
    // set 3d curve derivs from nearby internal points.
    if adjust {
        let [_, da] = eval1(k, map, ivl.evaluate(0.001))?;
        let [_, db] = eval1(k, map, ivl.evaluate(0.999))?;
        d0 = da;
        d1 = db;
    }

    Ok(HermiteCurve::new(p0, d0, p1, d1, 3))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blend::fillet_custom_solver::{OffsetSurface, SurfaceId};
    use crate::intersect::adv_surface_intersector::NoSurfaceHandle;
    use crate::nurbs::b_spline_surface::BSplineSurface;
    use crate::surface::offset_surface::SS_UNKNOWN;

    fn bilinear(p: [[f64; 3]; 4]) -> Surface {
        let [a, b, c, d] = p.map(Vec3::from);
        Surface::from(BSplineSurface::create_bilinear_surface(a, b, c, d).unwrap())
    }

    fn solver(a: &Surface, b: &Surface) -> ConstantRadiusAssistedFs {
        let k = Kernel::default();
        ConstantRadiusAssistedFs::from_parts(
            0.001,
            0.5,
            0.03,
            [1.0, 2.0],
            [
                Some(
                    OffsetSurface::new(&k, 1.0, a.clone(), SurfaceId::Free(0), SS_UNKNOWN).unwrap(),
                ),
                Some(
                    OffsetSurface::new(&k, -2.0, b.clone(), SurfaceId::Free(1), SS_UNKNOWN)
                        .unwrap(),
                ),
            ],
            [1.0, -1.0],
            [None, None],
        )
    }

    fn pnt(uv0: [f64; 2], uv1: [f64; 2], d: [f64; 3], t: f64) -> TsectPnt {
        let mut p = TsectPnt::new();
        p.point_type = IntersectionPointType::Crossing;
        *p.uv_pos_mut(SrfIndex::First) = Point2::new(uv0[0], uv0[1]);
        *p.uv_pos_mut(SrfIndex::Second) = Point2::new(uv1[0], uv1[1]);
        *p.uv_deriv_mut(SrfIndex::First) = Vec2::new(1.0, 0.0);
        *p.uv_deriv_mut(SrfIndex::Second) = Vec2::new(1.0, 0.0);
        *p.crv_pos_mut() = Vec3::new(uv0[0], 0.0, 0.0);
        *p.crv_deriv_mut() = Vec3::from(d);
        p.curve_parameter = t;
        p
    }

    fn surfaces() -> (Surface, Surface) {
        (
            bilinear([
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [1.0, 1.0, 0.0],
            ]),
            bilinear([
                [0.0, 0.0, 0.0],
                [0.0, 0.0, 1.0],
                [1.0, 0.0, 0.0],
                [1.0, 0.0, 1.0],
            ]),
        )
    }

    #[test]
    fn construction_and_flags() {
        let k = &Kernel::default();
        let (a, b) = surfaces();
        let d = Extent2d::from_bounds(-1.0, -1.0, 2.0, 2.0);
        let mut fi: FilletIntersector<'_, '_, NoSurfaceHandle, ()> =
            FilletIntersector::new(k, &a, &d, &b, &d, None).unwrap();
        assert!(!fi.base.base.do_boundary_point_on_curve_test);
        // the fillet flag keeps domains beyond the natural ones
        assert_eq!(fi.base.base.uv_domains[0], d);
        assert!(fi.is_kind_of(TYPE));
        assert!(!fi.is_kind_of(7));
        assert!(!fi.self_intersect());
        fi.set_self_intersect(true);
        assert!(fi.self_intersect());
        assert!(!fi.is_curr_fillet_touch_boundary());
        assert!(fi.curr_fillet_geom().is_none());
    }

    #[test]
    fn span_rejects_large_turn_and_keeps_offsets() {
        let k = &Kernel::default();
        let (a, b) = surfaces();
        let d = Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0);
        let mut s = solver(&a, &b);
        let mut fi: FilletIntersector<'_, '_, NoSurfaceHandle, FilletGeomId> =
            FilletIntersector::new(k, &a, &d, &b, &d, None).unwrap();
        let p = fi
            .base
            .base
            .alloc_point(pnt([0.1, 0.0], [0.1, 0.0], [1.0, 0.0, 0.0], 0.0));
        let mut n = pnt([0.2, 0.0], [0.2, 0.0], [0.0, 1.0, 0.0], 0.1);
        n.point_type = IntersectionPointType::Singularity;
        let n = fi.base.base.alloc_point(n);
        let r = fi
            .test_span_accuracy(&mut s, &Kernel::default(), p, n, None)
            .unwrap();
        assert!(!r.found_good_point);
        assert!((r.angle_found_rad - core::f64::consts::FRAC_PI_2).abs() < 1e-12);
        assert_eq!(
            fi.base.base.point(n).unwrap().point_type,
            IntersectionPointType::Crossing
        );
        // a zero next tangent is SM_ERR
        let z = fi
            .base
            .base
            .alloc_point(pnt([0.2, 0.0], [0.2, 0.0], [0.0, 0.0, 0.0], 0.1));
        assert!(fi
            .test_span_accuracy(&mut s, &Kernel::default(), p, z, None)
            .is_err());
        assert_eq!(s.surface(0).unwrap().offset_distance(), 1.0);
    }

    #[test]
    fn span_accepts_straight_line_on_planes() {
        let k = &Kernel::default();
        let (a, b) = surfaces();
        let d = Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0);
        let mut s = solver(&a, &b);
        let mut fi: FilletIntersector<'_, '_, NoSurfaceHandle, FilletGeomId> =
            FilletIntersector::new(k, &a, &d, &b, &d, None).unwrap();
        let p = fi
            .base
            .base
            .alloc_point(pnt([0.1, 0.0], [0.1, 0.0], [1.0, 0.0, 0.0], 0.0));
        let n = fi
            .base
            .base
            .alloc_point(pnt([0.2, 0.0], [0.2, 0.0], [1.0, 0.0, 0.0], 0.1));
        let m = fi
            .base
            .base
            .alloc_point(pnt([0.15, 0.0], [0.15, 0.0], [1.0, 0.0, 0.0], 0.05));
        let r = fi
            .test_span_accuracy(&mut s, &Kernel::default(), p, n, Some(m))
            .unwrap();
        assert!(r.found_good_point);
        assert!(r.deviation_found.is_none());
        // offsets restored
        assert_eq!(s.surface(0).unwrap().offset_distance(), 1.0);
        assert_eq!(s.surface(1).unwrap().offset_distance(), -2.0);
        assert_eq!(s.solver.saved_offsets, [1.0, -2.0]);
    }

    #[test]
    fn next_point_needs_a_crossing() {
        let k = &Kernel::default();
        let (a, b) = surfaces();
        let d = Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0);
        let mut s = solver(&a, &b);
        let mut fi: FilletIntersector<'_, '_, NoSurfaceHandle, FilletGeomId> =
            FilletIntersector::new(k, &a, &d, &b, &d, None).unwrap();
        let mut t = pnt([0.1, 0.0], [0.1, 0.0], [1.0, 0.0, 0.0], 0.0);
        t.point_type = IntersectionPointType::TangentCurve;
        let p = fi.base.base.alloc_point(t);
        let n = fi.base.base.alloc_point(TsectPnt::new());
        let r = fi
            .compute_next_point(&mut s, &Kernel::default(), p, 0.1, n)
            .unwrap();
        assert!(!r.found_good_point && !r.clipped && !r.boundary_hit);
        // aliased non-crossing: no point, as upstream
        let r = fi
            .compute_next_point(&mut s, &Kernel::default(), p, 0.1, p)
            .unwrap();
        assert!(!r.found_good_point && !r.clipped && !r.boundary_hit);
        // a crossing reaches StepSolve, whose Newton solve (through the
        // SmConstantRadiusFS::LoadInitialValues override, blend.fillet_standard_solver_p2)
        // finds no point on these surfaces
        let c = fi
            .base
            .base
            .alloc_point(pnt([0.1, 0.0], [0.1, 0.0], [1.0, 0.0, 0.0], 0.0));
        let r = fi
            .compute_next_point(&mut s, &Kernel::default(), c, 0.1, n)
            .unwrap();
        assert!(
            !r.found_good_point && !r.clipped && !r.boundary_hit,
            "{r:?}"
        );
    }

    /// A solver whose `StepSolve` clips the step to `clip` (rescaling it, as
    /// upstream's i/o `rdStep`) and optionally fails after setting `rbClipped`.
    struct ClipSolver {
        clip: f64,
        fail: bool,
    }

    impl FilletSolverOps for ClipSolver {
        type FilletGeom = ();

        fn point_on_plane_solve(
            &mut self,
            _k: &Kernel,
            _plane_origin: Point3,
            _plane_normal: Vec3,
            _uv_domain1: &Extent2d,
            _uv_domain2: &Extent2d,
            _uv1: Point2,
            _uv2: Point2,
            _point: &mut TsectPnt,
        ) -> Result<bool> {
            Err(SolidError::sm_err("test"))
        }

        fn compute_point_values(
            &mut self,
            _k: &Kernel,
            uv_values: [Point2; 2],
            _uv_domain1: &Extent2d,
            _uv_domain2: &Extent2d,
            _curve_trace_direction: f64,
            point: &mut TsectPnt,
            _previous: Option<&TsectPnt>,
        ) -> Result<()> {
            point.point_type = IntersectionPointType::Crossing;
            *point.uv_pos_mut(SrfIndex::First) = uv_values[0];
            *point.uv_pos_mut(SrfIndex::Second) = uv_values[1];
            *point.uv_deriv_mut(SrfIndex::First) = Vec2::new(1.0, 0.0);
            *point.uv_deriv_mut(SrfIndex::Second) = Vec2::new(1.0, 0.0);
            *point.crv_pos_mut() = Vec3::new(uv_values[0].x, 0.0, 0.0);
            *point.crv_deriv_mut() = Vec3::new(1.0, 0.0, 0.0);
            Ok(())
        }

        fn step_solve(
            &mut self,
            _k: &Kernel,
            point: &TsectPnt,
            step_size: &mut f64,
            _uv_domain1: &Extent2d,
            _uv_domain2: &Extent2d,
            flags: &mut StepSolve,
            next: &mut TsectPnt,
        ) -> Result<()> {
            *flags = StepSolve::default();
            flags.clipped = true;
            if self.fail {
                return Err(SolidError::sm_err("test"));
            }
            *step_size = self.clip;
            for s in SRFS {
                *next.uv_pos_mut(s) = point.uv_pos(s) + point.uv_deriv(s) * self.clip;
            }
            flags.found_answer = true;
            Ok(())
        }

        fn compute_surface_values(
            &mut self,
            _k: &Kernel,
            _uv_values: [Point2; 2],
            _point: &mut TsectPnt,
        ) -> Result<()> {
            Ok(())
        }

        fn compute_uv_vectors(&mut self, _k: &Kernel, _point: &mut TsectPnt) -> Result<()> {
            Ok(())
        }

        fn set_surfaces_to_zero_offset(&mut self) -> Result<()> {
            Ok(())
        }

        fn reload_surface_offsets(&mut self) -> Result<()> {
            Ok(())
        }

        fn rail_surface_evaluate_1st_derivatives(
            &mut self,
            _k: &Kernel,
            _rail: usize,
            _uv: Point2,
        ) -> Result<(Point3, Vec3, Vec3)> {
            Ok((Vec3::ZERO, Vec3::ZERO, Vec3::ZERO))
        }

        fn set_status(&mut self, _status: FilStatus) -> Result<()> {
            Ok(())
        }

        fn last_fillet_geom(&self) -> Result<()> {
            Ok(())
        }

        fn self_intersection_handler(&self) -> Result<Option<HandlingAlgorithmType>> {
            Ok(None)
        }

        fn smooth_out_intersection(
            &mut self,
            _k: &Kernel,
            _pseudo: &mut FilletBrep,
            _geom: &(),
            _curves_3d: &mut Vec<BSplineCurve>,
            _uv_curves1: &mut Vec<BSplineCurve>,
            _uv_curves2: &mut Vec<BSplineCurve>,
        ) -> Result<()> {
            Ok(())
        }

        fn has_surface_generator(&self) -> Result<bool> {
            Ok(false)
        }

        fn create_fillet_surface(
            &mut self,
            _k: &Kernel,
            _center_line: &mut BSplineCurve,
            _curve1: &mut BSplineCurve,
            _curve2: &mut BSplineCurve,
            _points: &[TsectPnt],
            _uv1: &mut BSplineCurve,
            _uv2: &mut BSplineCurve,
        ) -> Result<Option<BSplineSurface>> {
            Ok(None)
        }

        fn set_fillet_surface(
            &mut self,
            _pseudo: &mut FilletBrep,
            _geom: &(),
            _surface: Option<BSplineSurface>,
        ) -> Result<()> {
            Ok(())
        }

        fn set_center_line_curve(
            &mut self,
            _pseudo: &mut FilletBrep,
            _geom: &(),
            _center_line: BSplineCurve,
        ) -> Result<()> {
            Ok(())
        }

        fn set_rail_curve(
            &mut self,
            _pseudo: &mut FilletBrep,
            _geom: &(),
            _rail: usize,
            _curve: BSplineCurve,
        ) -> Result<()> {
            Ok(())
        }

        fn set_rail_mate_uv_curve(
            &mut self,
            _pseudo: &mut FilletBrep,
            _geom: &(),
            _rail: usize,
            _uv_curve: BSplineCurve,
        ) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn next_point_uses_the_step_size_step_solve_returns() {
        let k = &Kernel::default();
        let (a, b) = surfaces();
        let d = Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0);
        let mut fi: FilletIntersector<'_, '_, NoSurfaceHandle, ()> =
            FilletIntersector::new(k, &a, &d, &b, &d, None).unwrap();
        let p = fi
            .base
            .base
            .alloc_point(pnt([0.1, 0.0], [0.1, 0.0], [1.0, 0.0, 0.0], 0.0));
        let n = fi.base.base.alloc_point(TsectPnt::new());
        let mut s = ClipSolver {
            clip: 0.2,
            fail: false,
        };
        let r = fi
            .compute_next_point(&mut s, &Kernel::default(), p, 0.5, n)
            .unwrap();
        assert!(r.found_good_point && r.clipped && !r.boundary_hit);
        // rNextTSP.m_dCurveParameter = rTsectPnt.m_dCurveParameter + (clipped) dStepSize
        assert_eq!(fi.base.base.point(n).unwrap().curve_parameter, 0.2);

        // a failing StepSolve keeps the flags it set
        let mut s = ClipSolver {
            clip: 0.2,
            fail: true,
        };
        let r = fi
            .compute_next_point(&mut s, &Kernel::default(), p, 0.5, n)
            .unwrap();
        assert!(!r.found_good_point && r.clipped);
    }

    #[test]
    fn next_point_aliased_non_crossing_is_no_point() {
        let k = &Kernel::default();
        let (a, b) = surfaces();
        let d = Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0);
        let mut fi: FilletIntersector<'_, '_, NoSurfaceHandle, ()> =
            FilletIntersector::new(k, &a, &d, &b, &d, None).unwrap();
        let mut q = pnt([0.1, 0.0], [0.1, 0.0], [1.0, 0.0, 0.0], 0.0);
        q.point_type = IntersectionPointType::TangentPoint;
        let p = fi.base.base.alloc_point(q);
        let mut s = ClipSolver {
            clip: 0.2,
            fail: false,
        };
        let r = fi
            .compute_next_point(&mut s, &Kernel::default(), p, 0.5, p)
            .unwrap();
        assert!(!r.found_good_point && !r.clipped && !r.boundary_hit);
        // an aliased crossing is still refused
        let c = fi
            .base
            .base
            .alloc_point(pnt([0.1, 0.0], [0.1, 0.0], [1.0, 0.0, 0.0], 0.0));
        assert!(fi
            .compute_next_point(&mut s, &Kernel::default(), c, 0.5, c)
            .is_err());
    }

    #[test]
    fn flush_curve_short_lists() {
        let k = &Kernel::default();
        let (a, b) = surfaces();
        let d = Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0);
        let mut s = solver(&a, &b);
        let mut fi: FilletIntersector<'_, '_, NoSurfaceHandle, FilletGeomId> =
            FilletIntersector::new(k, &a, &d, &b, &d, None).unwrap();
        let mut out = TsectCurves::default();
        // no points: SM_ERR_NULL_POINTER
        assert!(fi
            .flush_curve(&mut s, &Kernel::default(), &mut FilletBrep::new(), &mut out)
            .is_err());
        // one point on the boundary of the first domain
        let p = fi
            .base
            .base
            .alloc_point(pnt([1.0, 0.5], [0.5, 0.5], [1.0, 0.0, 0.0], 0.0));
        fi.base.base.add_point_to_curve(p).unwrap();
        fi.flush_curve(&mut s, &Kernel::default(), &mut FilletBrep::new(), &mut out)
            .unwrap();
        assert!(fi.curr_fillet_touch_boundary());
        assert!(fi.base.base.curve_points().is_empty());
        assert!(out.curve_types.is_empty());
        // two points evaluate the offset rails, then reach the solver's
        // self-intersection handler, which is the missing executive's member
        // (a NULL dereference upstream)
        let p = fi
            .base
            .base
            .alloc_point(pnt([0.1, 0.5], [0.5, 0.5], [1.0, 0.0, 0.0], 0.0));
        let q = fi
            .base
            .base
            .alloc_point(pnt([0.2, 0.5], [0.5, 0.5], [1.0, 0.0, 0.0], 0.1));
        fi.base.base.add_point_to_curve(p).unwrap();
        fi.base.base.add_point_to_curve(q).unwrap();
        let e = fi
            .flush_curve(&mut s, &Kernel::default(), &mut FilletBrep::new(), &mut out)
            .unwrap_err();
        assert!(matches!(e, SolidError::InvalidInput { .. }), "{e:?}");
        assert!(!fi.curr_fillet_touch_boundary());
    }

    #[test]
    fn base_members_delegate_to_the_solver_base() {
        use crate::blend::fillet_solver::p1::FilletExecutiveRef;
        let (a, b) = surfaces();
        let mut s = solver(&a, &b);
        let k = Kernel::default();

        s.set_status(FilStatus::SurfSkinningFailure).unwrap();
        assert_eq!(s.solver.status(), FilStatus::SurfSkinningFailure);
        s.set_status(FilStatus::Processed).unwrap();
        assert_eq!(s.solver.status(), FilStatus::Processed);

        assert!(!s.has_surface_generator().unwrap());
        // the base constructor's geometry lives in the pseudo B-rep
        assert!(matches!(
            s.last_fillet_geom().unwrap_err(),
            SolidError::InvalidInput { .. }
        ));
        let mut pseudo = FilletBrep::new();
        let g = s
            .solver
            .add_constructor_fillet_geom(
                &k,
                &mut pseudo,
                crate::blend::fillet_geom::p1::FilletSolverId(0),
            )
            .unwrap();
        assert_eq!(s.last_fillet_geom().unwrap(), g);
        // its offset surfaces are copies of the solver's (set before the geometry here)
        assert!(pseudo
            .geom(g)
            .unwrap()
            .off_surfs
            .iter()
            .all(Option::is_some));

        // FlushCurve's writes go to the geometry in the pseudo B-rep
        let mut curves = Vec::new();
        flush_crv(
            3,
            &[
                Point3::new(0.0, 0.0, 0.0),
                Point3::new(1.0, 0.0, 0.0),
                Point3::new(2.0, 0.0, 0.0),
                Point3::new(3.0, 0.0, 0.0),
            ],
            &[0.0, 1.0],
            &mut curves,
        )
        .unwrap();
        let line = curves.pop().unwrap();
        s.set_center_line_curve(&mut pseudo, &g, line.clone())
            .unwrap();
        assert!(pseudo.geom(g).unwrap().center_line_curve().is_some());
        s.set_fillet_surface(&mut pseudo, &g, None).unwrap();
        assert!(pseudo
            .geom(g)
            .unwrap()
            .fillet_surface(None)
            .unwrap()
            .is_none());
        // a geometry without rails: upstream dereferences NULL
        assert!(matches!(
            s.set_rail_curve(&mut pseudo, &g, 0, line.clone())
                .unwrap_err(),
            SolidError::InvalidInput { .. }
        ));
        assert!(matches!(
            s.set_rail_mate_uv_curve(&mut pseudo, &g, 1, line)
                .unwrap_err(),
            SolidError::InvalidInput { .. }
        ));

        // the handler is the executive's member
        assert!(matches!(
            s.self_intersection_handler().unwrap_err(),
            SolidError::InvalidInput { .. }
        ));
        s.solver.set_fillet_executive(Some(FilletExecutiveRef));
        assert_eq!(s.self_intersection_handler().unwrap(), None);
        s.solver.executive_self_intersection_handler =
            Some(crate::blend::fillet_solver::p1::MakeSurfaceBlendSih::new(1.0).algorithm_type());
        assert_eq!(
            s.self_intersection_handler().unwrap(),
            Some(HandlingAlgorithmType::Blending)
        );

        // ComputeSurfaceValues runs the SmConstantRadiusFS::SetupOffsetValues override
        // (blend.fillet_standard_solver_p1): radius times orientation on both rails
        s.solver.surfaces[1]
            .as_mut()
            .unwrap()
            .surface
            .set_offset_distance(0.0);
        let mut p = TsectPnt::new();
        s.compute_surface_values(&k, [Point2::new(0.5, 0.5); 2], &mut p)
            .expect("ported constant-radius surface values");
        assert_eq!(s.surface(0).unwrap().offset_distance(), 1.0);
        assert_eq!(s.surface(1).unwrap().offset_distance(), -2.0);
        assert_eq!(p.srf_pos(SrfIndex::First), Vec3::new(0.5, 0.5, 1.0));
        assert_eq!(p.srf_pos(SrfIndex::Second), Vec3::new(0.5, -2.0, 0.5));
        assert_eq!(p.srf_norm(SrfIndex::First), Vec3::new(0.0, 0.0, 1.0));
        assert_eq!(p.srf_norm(SrfIndex::Second), Vec3::new(0.0, 1.0, 0.0));
        assert_eq!(p.crv_pos(), p.srf_pos(SrfIndex::First));
        assert_eq!(
            crate::blend::fillet_solver::p1::FilletSolverOps::solver_type(&s).unwrap(),
            crate::blend::fillet_solver::p1::FilletSolverType::ConstRadiusAssisted
        );

        // StepSolve is the base's (unit blend.fillet_solver_p3) dispatched on this
        // solver: a clipped step finds no boundary point here and goes on to the Newton
        // solve (through the SmConstantRadiusFS::LoadInitialValues override,
        // blend.fillet_standard_solver_p2), which finds no point; the clip it recorded
        // first is kept
        let d = Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0);
        let cur = pnt([0.9, 0.5], [0.9, 0.5], [1.0, 0.0, 0.0], 0.0);
        let mut step = 0.5;
        let mut flags = StepSolve::default();
        let mut next = TsectPnt::new();
        s.step_solve(&k, &cur, &mut step, &d, &d, &mut flags, &mut next)
            .unwrap();
        assert!(
            flags.clipped && !flags.found_answer && !flags.boundary_hit,
            "{flags:?}"
        );
    }

    #[test]
    fn mid_point_and_flush_crv() {
        let a = pnt([0.0, 0.0], [1.0, 1.0], [2.0, 0.0, 0.0], 0.0);
        let mut b = pnt([1.0, 2.0], [3.0, 1.0], [0.0, 4.0, 0.0], 1.0);
        b.deviation = 0.25;
        let mut m = TsectPnt::new();
        find_guess_mid_tsect_pnt(&a, &b, 0.25, &mut m);
        assert_eq!(m.curve_parameter, 0.25);
        assert_eq!(m.deviation, 0.25);
        assert!((m.crv_deriv().length() - 2.5).abs() < 1e-12);
        let mut z = TsectPnt::new();
        let a0 = pnt([0.0, 0.0], [1.0, 1.0], [0.0, 0.0, 0.0], 0.0);
        find_guess_mid_tsect_pnt(&a0, &a0, 0.5, &mut z);
        assert_eq!(z.crv_deriv(), Vec3::new(0.0, 0.0, 0.0));

        let mut out = Vec::new();
        flush_crv(3, &[], &[], &mut out).unwrap();
        assert!(out.is_empty());
        let poly: Vec<Point3> = (0..7).map(|i| Vec3::new(f64::from(i), 0.0, 0.0)).collect();
        flush_crv(3, &poly, &[0.0, 1.0, 2.0], &mut out).unwrap();
        assert_eq!(out.len(), 1);
        assert!(flush_crv(3, &poly, &[0.0, 1.0], &mut out).is_err());
    }
}
