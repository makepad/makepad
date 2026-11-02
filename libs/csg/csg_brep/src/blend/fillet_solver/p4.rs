//! Fillet geometry of a fillet solver: the ordered intersection points along the
//! filleted edge, the analytic attempt, the offset-surface trace of the fillet centre
//! line (with cusp recovery), the split and self-intersection post-processing, and
//! the point evaluation the fillet intersector steps through.
//!
//! Upstream: `source/SMLib/src/SmFilletSolver.cpp` (1495–2673, 3544–3768),
//! `source/SMLib/inc/SmFilletSolver.h`.
//!
//! | Rust                                        | Upstream                                          |
//! |---------------------------------------------|---------------------------------------------------|
//! | [`FilletSolver::calc_fillet_geom`]          | `SmFilletSolver::CalcFilletGeom`                  |
//! | [`FilletSolver::compute_point_values`]      | `SmFilletSolver::ComputePointValues`              |
//! | `TsRef`, `TsectStore` (private)             | `SmTArray<SmTsectPnt*> sTsectPnts` / `sDeletePnts` (pointers into `SmFilletVertexuse::m_vTsectPnt` or to `new SmTsectPnt`) |
//! | `get_or_create_uv_trim_curve`, `evaluate_binormal` (private) | `SmEdgeuse::GetOrCreateUVTrimCurve` and `SmEdgeuse::EvaluateBinormal` with the defaults `CalcFilletGeom` passes (unit `topology.edgeuse_p3`) |
//!
//! Deviations:
//! - **Object model.** The solver's virtuals are the [`FilletSolverOps`] trait object
//!   passed in (`this`); `SmContext` arguments are the [`Kernel`]. The filleted edge,
//!   its edge uses and their UV trim curves belong to the *target* [`Brep`]; rails,
//!   fillet vertex uses and fillet geometries to the pseudo B-rep [`FilletBrep`];
//!   `m_vFilletGeoms` holds their [`FilletGeomId`]s. Where upstream follows a
//!   geometry's `GetFilletSolver()` pointer to another solver
//!   (`SmFilletEdge::CalcCrossSection`), the caller's solver table resolves it.
//! - **Intersection point list.** `sTsectPnts` mixes pointers into fillet vertex uses
//!   (`SmFilletVertexuse::GetTsectPnt()`, the rail-end points) and points allocated by
//!   this function (G1-knot points, the cusp-recovery point). Here the list holds
//!   `TsRef` handles: a vertex use id, or an index into a local append-only store
//!   (`sDeletePnts`, which lives as long as the call). Every in-place write upstream
//!   makes through those pointers (the `smos_CleanUpNoise` of the rail-end UVs, the
//!   `m_dCurveParameter = -SM_BIG_DOUBLE` sort mark, everything `DoPointIntersection`
//!   writes) goes back to the storage the handle names; `DoPointIntersection` works on
//!   a snapshot that is written back point by point.
//! - **Function static.** `ComputePointValues`' `static double sdStaticStep` (set once
//!   per process by the first finite-difference evaluation) is the
//!   [`Kernel::fillet_point_step`] latch: per kernel instead of per process. Difftest
//!   runners seed it from the C++ process's value.
//! - **Intersector.** `SmFilletIntersector sFI(*pOffsetSrf1, ..)` traces copies of the
//!   solver's offset surfaces seen as [`Surface`]s (the intersector borrows its
//!   surfaces while the solver stays mutable). Upstream they are the solver's own
//!   objects, so the zero offsets of `SetSurfacesToZeroOffset` (the rail span tests
//!   and Hermite maps of `TestSpanAccuracy`, `ComputeNextPoint`, `FlushCurve`), the
//!   restores of `ReloadSurfaceOffsets` and the per-point `SetupOffsetValues`
//!   distances reach the intersector: the copies' distances (a `Cell` in
//!   [`OffsetSurface`](crate::surface::offset_surface::OffsetSurface)) are set to the
//!   solver's on entry to `DoPointIntersection` and after every solver call it makes
//!   that may change them. A base extension the solver stores on its own surfaces is
//!   rebuilt by the copies' evaluations instead.
//! - **Callees.** `CalcCylinderFilletGeom`, `CalcTorusFilletGeom`,
//!   `RecordFilletError` (`blend.fillet_solver_p2`; the point list is passed as a
//!   snapshot, they read it only); `sm_FindFilletTsectPnt`, `StepSolve`
//!   (`blend.fillet_solver_p3`; `StepSolve` gets no target B-rep for
//!   `LoadInitialValues`, as the fillet intersector's solvers pass none);
//!   `SmFilletGeom::TopologySplit` (`blend.fillet_geom_p3`, with the geometry's solver,
//!   this one); `SmFilletEdge::CalcCrossSection` (`blend.fillet_geom_p5`, default
//!   `pCorner = NULL`). `SmEdgeuse::EvaluateBinormal` and `GetOrCreateUVTrimCurve` are
//!   those of `topology.edgeuse_p3` (whose own unported paths, e.g.
//!   `SmSurface::DropCurve`, still return [`SolidError::Unported`]);
//!   `SmFilletGeom::CalcBlendingGeom` is `blend.fillet_geom_p4`'s, given this solver
//!   as the geometry's `GetFilletSolver()`.
//! - **`sFI.DoPointIntersection`** is [`FilletIntersector::do_point_intersection`]
//!   (the base driver with the `SmFilletIntersector` overrides) on a copy of the
//!   points in the intersector's arena, copied back afterwards. The intersector sees
//!   this solver through a private adapter implementing the intersector's
//!   `FilletSolverOps`; its writes into the fillet geometry go to the pseudo B-rep (the
//!   rail curve's `SmEdge::SetCurve` with its notification, the mate's UV curve), but
//!   `FlushCurve` stops before them at the self-intersection handler, the executive's
//!   member (`blend.fillet_executive_p1`, not ported). `m_pEndPoint` keeps pointing at the previous
//!   call's point when a call does not set it (upstream: the caller's live point;
//!   here: the arena copy).
//! - **Iteration.** No recursion. Loops are bounded: two surfaces, two rails of two
//!   edge uses, the G1 knots and points, ten trace passes, twenty cusp skips, three
//!   drop tolerances.

use super::p1::{FilletSolver, FilletSolverOps};
use super::p2::SolverErrorObject;
use crate::blend::fillet_custom_solver::OffsetSurface;
use crate::blend::fillet_executive::p3::ErrorInfo;
use crate::blend::fillet_geom::p1::{
    CornerLookup, FilletBrep, FilletErrorType, FilletGeomId, FilletGeomType, FilletSolverId,
};
use crate::blend::fillet_geom::p3::{NoCorners, SplitCorners};
use crate::blend::fillet_intersector::{FilletIntersector, StepSolve as StepSolveFlags};
use crate::blend::fillet_solver::p1::NoExtensionCorners;
use crate::curve::curve::{Curve, CurveKind};
use crate::error::{Result, SolidError};
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::vector2d::Point2;
use crate::intersect::surface_intersector::p1::TsectPnt;
use crate::kernel::Kernel;
use crate::math::{Point3, Vec3};
use crate::nurbs::b_spline_curve::BSplineCurve;
use crate::nurbs::knots::Side;
use crate::surface::surface::p4::NoObject;
use crate::surface::surface::p7::drop_vectors_at;
use crate::surface::surface::{Surface, SurfaceKind};
use crate::topology::edgeuse::p1 as eu;
use crate::topology::edgeuse::p2::BinormalEval;
use crate::topology::edgeuse::p3::{self as edgeuse_p3, BinormalOptions};
use crate::topology::{Brep, EdgeId, EdgeuseId, VertexuseId};

const OP: &str = "blend::fillet_solver::p4";

fn invalid(reason: &'static str) -> SolidError {
    SolidError::invalid_input(OP, reason)
}

// =====================================================================================
// SHARED: data model (sTsectPnts), state passed between the chunks, public resolver
// =====================================================================================

/// An entry of `sTsectPnts` (an `SmTsectPnt*`): which storage the point lives in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TsRef {
    /// `&pVU->GetTsectPnt()`: the point stored in a pseudo-B-rep fillet vertex use.
    Vertexuse(VertexuseId),
    /// A point this call allocated (`new SmTsectPnt`, owned by `sDeletePnts`): an index
    /// into [`TsectStore`].
    Local(usize),
}

/// `sDeletePnts`: the points this call allocates. Append-only, so a [`TsRef::Local`]
/// stays valid for the whole call (as upstream's pointers until `sObjs` deletes them).
#[derive(Debug, Clone, Default)]
struct TsectStore {
    local: Vec<TsectPnt>,
}

impl TsectStore {
    /// `new SmTsectPnt` + `sDeletePnts.Add`: stores `point` and returns its handle.
    fn alloc(&mut self, point: TsectPnt) -> TsRef {
        self.local.push(point);
        TsRef::Local(self.local.len() - 1)
    }

    /// `*pTsectPnt`.
    fn get<'a>(&'a self, fillets: &'a FilletBrep, r: TsRef) -> Result<&'a TsectPnt> {
        match r {
            TsRef::Vertexuse(vu) => Ok(fillets.fillet_vertexuse(vu)?.tsect_pnt()),
            TsRef::Local(i) => self
                .local
                .get(i)
                .ok_or_else(|| invalid("local intersection point out of range")),
        }
    }

    /// `*pTsectPnt`, for an in-place write (it lands in the vertex use for a rail-end
    /// point, as upstream's write through the pointer does).
    fn get_mut<'a>(
        &'a mut self,
        fillets: &'a mut FilletBrep,
        r: TsRef,
    ) -> Result<&'a mut TsectPnt> {
        match r {
            TsRef::Vertexuse(vu) => Ok(fillets.fillet_vertexuse_mut(vu)?.tsect_pnt_mut()),
            TsRef::Local(i) => self
                .local
                .get_mut(i)
                .ok_or_else(|| invalid("local intersection point out of range")),
        }
    }

    /// Copies of the points `refs` names, in order (for a callee that takes the array
    /// of points; write them back with [`TsectStore::write_back`]).
    fn snapshot(&self, fillets: &FilletBrep, refs: &[TsRef]) -> Result<Vec<TsectPnt>> {
        refs.iter()
            .map(|r| self.get(fillets, *r).cloned())
            .collect()
    }

    /// Writes `points` (a [`TsectStore::snapshot`] of `refs`, possibly changed) back
    /// to the storage each handle names, in order (a handle listed twice gets its last
    /// value; upstream's two pointers would alias one point).
    fn write_back(
        &mut self,
        fillets: &mut FilletBrep,
        refs: &[TsRef],
        points: &[TsectPnt],
    ) -> Result<()> {
        if refs.len() != points.len() {
            return Err(invalid("intersection point snapshot length changed"));
        }
        for (r, p) in refs.iter().zip(points) {
            *self.get_mut(fillets, *r)? = p.clone();
        }
        Ok(())
    }
}

/// Actual executive and solver records borrowed by `CalcFilletGeom`.
/// The current solver remains the same object supplied as mutable operations.
pub trait FilletGeometryContext {
    /// Prepare extension flags from the actual endpoint corners.
    ///
    /// # Errors
    /// Actual corner resolution or preparation failures.
    fn set_extension_flags(
        &self,
        solver: &mut dyn FilletSolverOps,
        k: &Kernel,
        target: &Brep,
    ) -> Result<()>;

    /// Resolve an actual solver ID; borrow `current` only for this lookup's use.
    ///
    /// # Errors
    /// An unresolved solver identity or missing actual record.
    fn solver<'s>(
        &'s self,
        id: FilletSolverId,
        current: &'s FilletSolver,
    ) -> Result<&'s FilletSolver>;

    /// The executive's corner table (`m_vFilletCorners`), which the pseudo-B-rep
    /// edge calculations read.
    fn corners(&self) -> CornerLookup<'_>;

    /// The executive's corners as `TopologySplit` reaches and changes them.
    fn split_corners(&mut self) -> &mut dyn SplitCorners;
}

enum GeometrySolvers<'a> {
    Indexed(&'a [FilletSolver]),
    Live(&'a mut dyn FilletGeometryContext),
}

/// The objects `CalcFilletGeom` reaches besides the solver.
struct Cx<'a> {
    /// The kernel (`m_crContext`).
    k: &'a Kernel,
    /// The target B-rep: the filleted edge, its edge uses and UV trim curves.
    target: &'a mut Brep,
    /// The pseudo B-rep: rails, fillet vertex uses (rail-end points), geometries.
    fillets: &'a mut FilletBrep,
    /// The executive's solver table (`GetFilletSolver()` of the pseudo B-rep's
    /// geometries, read by `SmFilletEdge::CalcCrossSection`).
    solvers: GeometrySolvers<'a>,
    /// The executive's error record `RecordFilletError` notes into
    /// (`m_pExecutive->NoteFilletError`); `None` for a standalone entry point, which
    /// notes into a scratch record.
    error_info: Option<&'a mut ErrorInfo<SolverErrorObject>>,
}

/// The locals of `CalcFilletGeom` that live across the chunks.
#[derive(Debug, Clone)]
struct CalcState {
    // ---- C1: setup (:1499-1553) ------------------------------------------------------
    /// `pEdgeCurve`: the edge's curve (a kind derived from `SmBSplineCurve`, checked
    /// with [`bspline_curve_cast`]); virtual calls go through [`Curve`].
    edge_curve: Curve,
    /// `sIvl = pEdge->GetInterval()`.
    ivl: Extent1d,
    /// `sPVSt`: position and first derivative at `sIvl.GetMin()`.
    pv_start: [Vec3; 2],
    /// `sPVEn`: position and first derivative at `sIvl.GetMax()`.
    pv_end: [Vec3; 2],
    /// `pBaseSrf1`, `pBaseSrf2`: copies of `GetSurface(i)->GetBaseSurface()` taken at
    /// the start (upstream holds the pointers; nothing in the call replaces a base).
    base_surfaces: [Surface; 2],
    /// `pFilletGeom0 = m_vFilletGeoms[0]`.
    geom0: FilletGeomId,
    /// `bSplitOriginalFillet = pFilletGeom0->GetSplitFlag()`.
    split_original_fillet: bool,
    /// `pSplitTsectPnt` (`NULL` until a G1-knot point at the split parameter is made).
    split_tsect_pnt: Option<TsRef>,
    // ---- C1/C2: the point list (:1582-1920) ------------------------------------------
    /// `sDeletePnts` (the points this call allocates).
    store: TsectStore,
    /// `sTsectPnts`.
    tsect_pnts: Vec<TsRef>,
    /// `bDoSorting`.
    do_sorting: bool,
    // ---- C3: trace setup (:2028-2090) ------------------------------------------------
    /// The trace locals C3 sets and C4/C5 use.
    trace: TraceLocals,
}

/// The locals of the trace (:2028-2090), set by C3.
#[derive(Debug, Clone)]
struct TraceLocals {
    /// `sDomain1`, `sDomain2` after `SetupOffsetExtension`.
    domains: [Extent2d; 2],
    /// `dThisApproxTol3d = GetThisApproxTol3d()`.
    approx_tol_3d: f64,
    /// `dAngleTol = GetThisAngTolRad()`.
    angle_tol: f64,
    /// `sDir`: the trace start direction.
    dir: Vec3,
    /// `bReverseTracing`.
    reverse_tracing: bool,
    /// `bExtendBefore` (the local; `m_bExtendBefore` is the solver's).
    extend_before: bool,
    /// `bExtendAfter` (the local).
    extend_after: bool,
}

impl CalcState {
    /// The state after setup (:1499-1553): an empty point list, sorting on, and the
    /// trace locals at neutral values until C3 sets them.
    fn new(
        edge_curve: Curve,
        ivl: Extent1d,
        pv_start: [Vec3; 2],
        pv_end: [Vec3; 2],
        base_surfaces: [Surface; 2],
        geom0: FilletGeomId,
        split_original_fillet: bool,
    ) -> Self {
        // (kernel-free: a lazy analytic answers its NURB-less base's empty domain)
        let domain = base_surfaces[0].natural_uv_domain_if_built();
        Self {
            edge_curve,
            ivl,
            pv_start,
            pv_end,
            base_surfaces,
            geom0,
            split_original_fillet,
            split_tsect_pnt: None,
            store: TsectStore::default(),
            tsect_pnts: Vec::new(),
            do_sorting: true,
            trace: TraceLocals {
                domains: [domain, domain],
                approx_tol_3d: 0.0,
                angle_tol: 0.0,
                dir: pv_start[1],
                reverse_tracing: false,
                extend_before: false,
                extend_after: false,
            },
        }
    }

    /// `*sTsectPnts[i]`.
    fn point<'a>(&'a self, fillets: &'a FilletBrep, i: usize) -> Result<&'a TsectPnt> {
        let r = *self
            .tsect_pnts
            .get(i)
            .ok_or_else(|| invalid("intersection point index out of range"))?;
        self.store.get(fillets, r)
    }
}

/// One pass of the `lWhichSurf` loop after collection (:1593-1752): the UV trim curve
/// of `GetEdgeuse(lWhichSurf)` and `sAllTsectPnts`.
#[derive(Debug, Clone)]
struct SurfPass {
    /// `pUVCurveFilletedEdge` (a copy of the edge use's trim curve).
    uv_curve: Curve,
    /// `sAllTsectPnts`.
    all: Vec<TsRef>,
}

/// The fillet intersector of the trace: over copies of the offset surfaces, solution
/// objects unused, the solver's geometry handles.
type TraceIntersector<'s> = FilletIntersector<'s, 's, NoObject, FilletGeomId>;

// =====================================================================================
// PUBLIC API: orchestration (keep the upstream control flow here; chunk bodies below)
// =====================================================================================

impl FilletSolver {
    /// `CalcFilletGeom()`: the fillet geometry of the solver's sector. Collects the
    /// rail-end points of the first fillet geometry, the points at the filleted edge's
    /// G1 knots (and at the split parameter), orders them along a UV trim curve of
    /// the edge, tries an analytic (cylinder / torus) fillet, and otherwise traces the
    /// offset-surface intersection through the points (recovering from cusps), then
    /// splits the geometry when asked, finishes self-intersection patches and undoes a
    /// reversed trace. `ops` is the solver (its virtuals); `target` holds the filleted
    /// edge; `fillets` the pseudo B-rep; `geoms` resolves `m_vFilletGeoms`.
    ///
    /// # Errors
    ///
    /// Upstream's statuses (`SM_ERR`, `SM_ERR_NULL_POINTER`, the callees');
    /// [`SolidError::InvalidInput`] where upstream dereferences `NULL`;
    /// [`SolidError::Unported`] from unported callees.
    pub fn calc_fillet_geom(
        ops: &mut dyn FilletSolverOps,
        k: &Kernel,
        target: &mut Brep,
        fillets: &mut FilletBrep,
        solvers: &[FilletSolver],
    ) -> Result<()> {
        Self::calc_fillet_geom_resolved(
            ops,
            k,
            target,
            fillets,
            GeometrySolvers::Indexed(solvers),
            None,
        )
    }

    /// Calculate geometry using the executive's actual corner and solver context.
    ///
    /// # Errors
    /// As [`Self::calc_fillet_geom`], including actual context resolution failures.
    /// `error_info` is the executive's record its `RecordFilletError` calls note into.
    pub fn calc_fillet_geom_with_context(
        ops: &mut dyn FilletSolverOps,
        k: &Kernel,
        target: &mut Brep,
        fillets: &mut FilletBrep,
        context: &mut impl FilletGeometryContext,
        error_info: &mut ErrorInfo<SolverErrorObject>,
    ) -> Result<()> {
        Self::calc_fillet_geom_resolved(
            ops,
            k,
            target,
            fillets,
            GeometrySolvers::Live(context),
            Some(error_info),
        )
    }

    fn calc_fillet_geom_resolved<'a>(
        ops: &mut dyn FilletSolverOps,
        k: &'a Kernel,
        target: &'a mut Brep,
        fillets: &'a mut FilletBrep,
        solvers: GeometrySolvers<'a>,
        error_info: Option<&'a mut ErrorInfo<SolverErrorObject>>,
    ) -> Result<()> {
        let mut cx = Cx {
            k,
            target,
            fillets,
            solvers,
            error_info,
        };

        // :1495-1553 SetExtensionFlags, edge, curve end points, surfaces, geometry 0
        let mut st = c1_setup(ops, &mut cx)?;

        // :1582-1867 build the ordered sTsectPnts on one of the two edge uses
        for which_surf in 0..2 {
            st.tsect_pnts.clear();
            // :1587-1752 trim curve, closed test, split parameter, G1 points, rail ends
            let Some(pass) = c1_collect_points(ops, &mut cx, &mut st, which_surf)? else {
                // closed UV trim curve on the first edge use: try the other one
                continue;
            };
            if st.do_sorting {
                // :1754-1858 drop to the trim curve and insertion-sort
                c2_sort_points(ops, &mut cx, &mut st, which_surf, &pass)?;
                if st.tsect_pnts.len() > 1 {
                    break;
                }
            } else {
                // :1860-1863 both trim curves closed: sTsectPnts.Append(sAllTsectPnts)
                st.tsect_pnts.extend(pass.all.iter().copied());
            }
        }

        // :1872-1920 no points; drop near-duplicate end points
        c2_clean_ends(ops, &mut cx, &mut st)?;

        // :1955-2011 analytic fillet
        if c3_try_analytic(ops, &mut cx, &mut st)? {
            return Ok(());
        }

        // :2028-2034 domains (and the intersector's surfaces)
        let surfaces = c3_trace_setup(ops, &mut cx, &mut st)?;
        {
            // :2037-2052 SmFilletIntersector sFI(..), extension distance
            let mut fi = c3_make_intersector(ops, cx.k, &st, &surfaces)?;
            // :2054-2090 trace locals, reversal at a tangent last point
            c3_trace_start(ops, &mut cx, &mut st)?;
            // :2093-2467 while (!bFoundAllPoints)
            c4_trace(ops, &mut cx, &mut st, &mut fi)?;
        }

        // :2470-2671 split, self-intersection patches, undo the reversal
        c5_post_process(ops, &mut cx, &mut st)
    }

    /// `ComputePointValues(aUVValues, crUVDomain1, crUVDomain2, dCurveTraceDirection,
    /// rTsectPnt, pOptPreviousPnt)`: the surface values of `point` at the rail UV
    /// points, the centre-line tangent (cross product of the normals, or finite
    /// differences of solver steps for variable offsets and tangent points), the point
    /// type and the UV tangents. `ops` is the solver (its virtuals); the finite
    /// difference step is the kernel's [`Kernel::fillet_point_step`] latch.
    ///
    /// # Errors
    ///
    /// Those of `ComputeSurfaceValues`, `StepSolve` and the evaluations; `SM_ERR` when
    /// neither step succeeds or the stepped normals are parallel; a zero tangent's
    /// `Unitize` status; [`SolidError::Unported`] from unported callees.
    pub fn compute_point_values(
        ops: &mut dyn FilletSolverOps,
        k: &Kernel,
        uv_values: [Point2; 2],
        uv_domain1: &Extent2d,
        uv_domain2: &Extent2d,
        curve_trace_direction: f64,
        point: &mut TsectPnt,
        previous: Option<&TsectPnt>,
    ) -> Result<()> {
        c6_compute_point_values(
            ops,
            k,
            None,
            uv_values,
            uv_domain1,
            uv_domain2,
            curve_trace_direction,
            point,
            previous,
        )
    }
}

// =====================================================================================
// SHARED: helpers used by more than one chunk
// =====================================================================================

/// `GetSurface(rail)` (an `SmOffsetSurface*`; upstream dereferences it unchecked).
fn offset_surface(ops: &dyn FilletSolverOps, rail: usize) -> Result<&OffsetSurface> {
    ops.solver()
        .surface(rail)?
        .map(|slot| &slot.surface)
        .ok_or_else(|| invalid("no offset surface"))
}

/// `GetSurface(rail)`, mutable (offset distance changes, stored extensions).
fn offset_surface_mut(ops: &mut dyn FilletSolverOps, rail: usize) -> Result<&mut OffsetSurface> {
    ops.solver_mut()
        .surface_mut(rail)?
        .map(|slot| &mut slot.surface)
        .ok_or_else(|| invalid("no offset surface"))
}

/// `GetSurface(rail)->EvaluatePoint(uv, pnt)` on the solver's own surface (an
/// extended base it builds is stored, as upstream's `const` cast does).
fn offset_evaluate_point(
    k: &Kernel,
    ops: &mut dyn FilletSolverOps,
    rail: usize,
    uv: Point2,
) -> Result<Point3> {
    offset_surface_mut(ops, rail)?
        .surface
        .evaluate_point_extending(k, uv)
}

/// `GetSurface(rail)->EvaluateNormal(uv, TRUE, TRUE, n)` on the solver's own surface.
fn offset_evaluate_normal(
    k: &Kernel,
    ops: &mut dyn FilletSolverOps,
    rail: usize,
    uv: Point2,
) -> Result<Vec3> {
    offset_surface_mut(ops, rail)?
        .surface
        .evaluate_normal_extending(k, uv, Side::Left, Side::Left)
}

/// `SM_CAST_PTR(SmBSplineCurve, pCurve)`: the B-spline part of the kinds derived from
/// `SmBSplineCurve` (`SmLine`, `SmEllipse`/`SmCircle`, `SmParabola`, `SmHyperbola`),
/// `None` (upstream `NULL`) for the others.
const fn bspline_curve_cast(curve: &Curve) -> Option<&BSplineCurve> {
    match curve.kind() {
        CurveKind::BSpline(b) => Some(b),
        CurveKind::Line(l) => Some(l.base()),
        CurveKind::Ellipse(e) => Some(e.base()),
        CurveKind::Hyperbola(h) => Some(h.base()),
        CurveKind::Parabola(p) => Some(p.base()),
        CurveKind::Hermite(_)
        | CurveKind::Projected(_)
        | CurveKind::Iso(_)
        | CurveKind::Offset(_)
        | CurveKind::Conic(_)
        | CurveKind::CrvInVolume(_)
        | CurveKind::CrvOnSurf(_)
        | CurveKind::TangentField(_)
        | CurveKind::Composite(_) => None,
    }
}

/// `SM_CAST_PTR(SmBSplineSurface, pSurface) != NULL`: the kinds derived from
/// `SmBSplineSurface` (`SmPlane`, `SmSurfOfRevolution`, `SmSurfOfExtrusion`, and
/// through `SmSurfOfRevolution` `SmCone`/`SmCylinder`, `SmSphere`, `SmTorus`).
fn is_bspline_surface(surface: &Surface) -> bool {
    matches!(
        surface.kind(),
        SurfaceKind::BSpline(_)
            | SurfaceKind::Plane(_)
            | SurfaceKind::SurfOfRevolution(_)
            | SurfaceKind::SurfOfExtrusion(_)
            | SurfaceKind::Cone(_)
            | SurfaceKind::Sphere(_)
            | SurfaceKind::Torus(_)
    )
}

/// `m_vFilletGeoms` (a copy: the loops over it may add geometries).
fn fillet_geom_ids(ops: &dyn FilletSolverOps) -> Vec<FilletGeomId> {
    ops.solver().fillet_geoms.clone()
}

/// `m_vFilletGeoms.Add(pNewFilletGeom)`.
fn add_fillet_geom(ops: &mut dyn FilletSolverOps, id: FilletGeomId) {
    ops.solver_mut().fillet_geoms.push(id);
}

/// The guess point of a centre-line point at parameter `t` of the filleted edge
/// (B2 at :1641-1657, F2 at :2395-2425): `GetEdgeuse(0)->EvaluateBinormal(t, FALSE,
/// ..)`, the binormal scaled to `|GetSurface(1)->GetOffsetDistance()|` (then halved
/// when `half`: `sBinVec*dOffset*0.5`), dropped into the parameter space of
/// `base0` (`pBaseSrf1->DropVectors(uv, TRUE, TRUE, ..)`) from the binormal's UV
/// point, evaluated on `GetSurface(0)`. Returns the guess point and the edge-use
/// tangent (`sTangent`).
fn binormal_guess_point(
    ops: &mut dyn FilletSolverOps,
    cx: &mut Cx<'_>,
    base0: &Surface,
    t: f64,
    half: bool,
) -> Result<(Point3, Vec3)> {
    let eu0 = ops
        .solver()
        .edgeuse(0)?
        .ok_or_else(|| invalid("no edge use 0"))?;
    let b = evaluate_binormal(cx.k, cx.target, eu0, t)?;
    let offset = offset_surface(ops, 1)?.offset_distance().abs();
    let mut bin_vec = b.binormal * offset;
    if half {
        bin_vec *= 0.5;
    }
    let uv_dir = drop_vectors_at(cx.k, base0, b.uv, Side::Left, Side::Left, &[bin_vec])?
        .first()
        .copied()
        .ok_or_else(|| invalid("short drop result"))?;
    let uv = b.uv + uv_dir;
    let pnt = offset_evaluate_point(cx.k, ops, 0, uv)?;
    Ok((pnt, b.tangent))
}

// =====================================================================================
// P4-CHUNK(C1) BEGIN: setup and point collection, SmFilletSolver.cpp:1495-1752
// (chunk-local imports go inside the functions: never edit the shared `use` block)
// =====================================================================================

/// :1495-1553: `SetExtensionFlags()`, the filleted edge, its curve (B-spline kinds
/// only, `NER`), interval and end-point derivatives, the base surfaces, geometry 0 and
/// its split flag.
fn c1_setup(ops: &mut dyn FilletSolverOps, cx: &mut Cx<'_>) -> Result<CalcState> {
    use crate::topology::edge as topo_edge;

    // Set the extension flags for two ends of fillets
    // extend ends when vertices are not blended and not degenerate
    match &cx.solvers {
        GeometrySolvers::Indexed(_) => {
            ops.set_extension_flags(cx.k, cx.target, &NoExtensionCorners)?;
        }
        GeometrySolvers::Live(context) => context.set_extension_flags(ops, cx.k, cx.target)?,
    }

    // local to-be-filleted edge data
    // GetEdgeuse(0) is dereferenced unchecked upstream
    let eu0 = ops
        .solver()
        .edgeuse(0)?
        .ok_or_else(|| invalid("no edge use 0"))?;
    let edge = eu::edge(cx.target, eu0)?.ok_or_else(|| SolidError::null_pointer(OP))?;
    // SM_CAST_PTR(SmBSplineCurve, pEdge->GetCurve()): NULL for a missing curve or a
    // kind not derived from SmBSplineCurve
    let edge_curve = match topo_edge::curve(cx.target, edge)? {
        Some(c) if bspline_curve_cast(c).is_some() => c.clone(),
        _ => return Err(SolidError::null_pointer(OP)),
    };
    let ivl = topo_edge::interval(cx.target, edge)?;

    // evaluate curve end points
    let pv_start = c1_pos_and_tangent(&edge_curve.evaluate(cx.k, ivl.min(), 1, true, false)?)?;
    let pv_end = c1_pos_and_tangent(&edge_curve.evaluate(cx.k, ivl.max(), 1, true, false)?)?;

    // get fillet offset and base surfaces (GetSurface(i) is dereferenced unchecked;
    // an SmOffsetSurface always has its base, so the NER cannot fire)
    let base1 = offset_surface(ops, 0)?.base_surface().clone();
    let base2 = offset_surface(ops, 1)?.base_surface().clone();

    // get fillet geometry pointer and locals (m_vFilletGeoms[0] of an empty array
    // reads out of bounds upstream)
    let geom0 = ops
        .solver()
        .fillet_geoms
        .first()
        .copied()
        .ok_or_else(|| invalid("no fillet geometry"))?;
    let split_original_fillet = cx.fillets.geom(geom0)?.split_flag();

    Ok(CalcState::new(
        edge_curve,
        ivl,
        pv_start,
        pv_end,
        [base1, base2],
        geom0,
        split_original_fillet,
    ))
}

/// The `aPointAndDerivatives[2]` of an `Evaluate(t, 1, ..)` call.
fn c1_pos_and_tangent(pv: &[Vec3]) -> Result<[Vec3; 2]> {
    match pv {
        [p, d, ..] => Ok([*p, *d]),
        _ => Err(invalid("short curve evaluation")),
    }
}

/// :1587-1752, one pass of the `lWhichSurf` loop up to the sort: the UV trim curve
/// of `GetEdgeuse(which_surf)` (`GetOrCreateUVTrimCurve`, `SER`); `None` when it is
/// closed on pass 0 (`continue`), `st.do_sorting = false` when closed on pass 1; the
/// split parameter (temporary `m_vG1Knots` push/pop); a point per G1 knot
/// (`binormal_guess_point(.., half = true)`, `sm_FindFilletTsectPnt`, split capture);
/// the rail-end points (`SM_FV_MATE` / `SM_FV_RAIL_X_EXTENDED_EDGEUSE` skipped,
/// `SM_OT_SAME` in front, `smos_CleanUpNoise` written back into the vertex use).
///
/// As upstream, an error inside the G1-knot loop returns before the temporary split
/// knot is removed from `m_vG1Knots`.
fn c1_collect_points(
    ops: &mut dyn FilletSolverOps,
    cx: &mut Cx<'_>,
    st: &mut CalcState,
    which_surf: usize,
) -> Result<Option<SurfPass>> {
    use crate::blend::fillet_geom::p1::{clean_up_noise, FilletVertexType};
    use crate::intersect::surface_intersector::p1::SrfIndex;
    use crate::math::EFF_ZERO;
    use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
    use crate::topology::{edge as topo_edge, vertexuse as topo_vu};

    // get edgeuse->UVTrimCurve marking a rail curve and bracketing sector to be filleted
    let eu_filleted_edge = ops
        .solver()
        .edgeuse(which_surf)?
        .ok_or_else(|| SolidError::null_pointer(OP))?;
    let uv_curve = get_or_create_uv_trim_curve(cx.k, cx.target, eu_filleted_edge)?;

    // special case: don't sort SmTsectPnts on closed UVTrimCurves
    if uv_curve.is_closed(cx.k, &st.ivl, 0.0)? {
        if which_surf == 0 {
            // try the other one.
            return Ok(None);
        }
        // both closed
        st.do_sorting = false;
    }

    // array of all TsectPnts (including potential duplicates):
    // from rail ends, and possibly G1 discontinuity points and split point.
    let mut all: Vec<TsRef> = Vec::new();

    // when asked to split the edge - find and save a split parameter
    let mut split_param = st.ivl.evaluate(0.5);
    let mut found_split_knot = false;
    if st.split_original_fillet {
        let total_g1_knots = ops.solver().g1_knots.len();
        if total_g1_knots > 0 {
            // Pick a middle G1 knot and split it right there
            split_param = ops
                .solver()
                .g1_knots
                .get(total_g1_knots / 2)
                .copied()
                .ok_or_else(|| invalid("G1 knot index out of range"))?;
            found_split_knot = true;
        } else {
            ops.solver_mut().g1_knots.push(split_param);
        }
    }

    // Now, compute 'through points' from all internal G1 knots
    // (the size is re-read every iteration, as upstream's GetSize())
    // source (source issue #1022, WR-468, opt-in): then from the edge's
    // ends where a rail of geometry 0 ends on an extended edge
    // ([`c1_extended_rail_end_knots`]).
    let extra_knots = if cx.k.fixes.fillet_through_extended_rail_end {
        c1_extended_rail_end_knots(ops, cx, st)?
    } else {
        Vec::new()
    };
    let mut k = 0;
    while k < ops.solver().g1_knots.len() + extra_knots.len() {
        // get position and tangent at G1 knot (status checked, values unused)
        let knot = match ops.solver().g1_knots.get(k) {
            Some(&knot) => knot,
            None => extra_knots
                .get(k - ops.solver().g1_knots.len())
                .copied()
                .ok_or_else(|| invalid("G1 knot index out of range"))?,
        };
        st.edge_curve.evaluate(cx.k, knot, 1, true, false)?;

        // guess point: GetSurface(0) point nearest to the end of the binormal (half
        // the second offset) dropped onto pBaseSrf1; tangent in edgeuse direction
        let (pnt, tangent) = binormal_guess_point(ops, cx, &st.base_surfaces[0], knot, true)?;

        // Build a SmTsectPnt for each to-be-filleted curve g1 points.
        let mut new_pnt = TsectPnt::new();
        match find_fillet_tsect_pnt(
            ops,
            cx.k,
            cx.target,
            &st.base_surfaces[0],
            &st.base_surfaces[1],
            pnt,
            tangent,
            &mut new_pnt,
        ) {
            Ok(()) => {}
            Err(e) if e.is_unported() => return Err(e),
            Err(_) => return Err(SolidError::sm_err(OP)),
        }
        // add new TsectPnt to sAllTsectPnts array (and sDeletePnts)
        let new_ref = st.store.alloc(new_pnt);
        all.push(new_ref);

        // when splitting original fillet - save the appropriate TsectPnt
        if st.split_original_fillet && (knot - split_param).abs() < EFF_ZERO {
            st.split_tsect_pnt = Some(new_ref);
        }
        k += 1;
    }

    // restore G1Knots if a split param was added
    if st.split_original_fillet && !found_split_knot {
        ops.solver_mut().g1_knots.pop();
    }

    // Process rail_ends: rail[i]->edgeuse[jj]->FilletVertexuse->TsectPnt
    let geom0 = st.geom0;
    for which_rail in 0..2 {
        let this_rail = cx
            .fillets
            .geom(geom0)?
            .rail(which_rail)
            .ok_or_else(|| SolidError::null_pointer(OP))?;
        let fil_edgeuses = topo_edge::edgeuses(cx.fillets.brep(), this_rail, None, None)?;
        if fil_edgeuses.len() != 2 {
            return Err(SolidError::sm_err(OP));
        }

        // for both Rail->edgeuses
        for &eu_rail in &fil_edgeuses {
            // from edgeuse get FilletVertexuse and FilletVertex (unchecked casts and
            // dereferences upstream)
            let brep = cx.fillets.brep();
            let vu = eu::vertexuse(brep, eu_rail)?
                .ok_or_else(|| invalid("rail edge use without vertex use"))?;
            let v = topo_vu::vertex(brep, vu)?
                .ok_or_else(|| invalid("rail vertex use without vertex"))?;
            let orientation = eu::orientation(brep, eu_rail)?;

            // skip: mates just tag along with their mates, same param value.
            let vertex_type = cx.fillets.fillet_vertex(v)?.fillet_vertex_type();
            if vertex_type == FilletVertexType::Mate
                || vertex_type == FilletVertexType::RailXExtendedEdgeuse
            {
                continue;
            }

            // fetch TsectPnt from SmFilletVertexuse (already computed)
            let r = TsRef::Vertexuse(vu);
            if orientation == OrientType::Same {
                all.insert(0, r);
            } else {
                // SM_OT_OPPOSITE
                all.push(r);
            }

            // for both components convert numbers like 1.1200000000007 to 1.12
            let tsect = st.store.get_mut(cx.fillets, r)?;
            for srf in [SrfIndex::First, SrfIndex::Second] {
                let uv = tsect.uv_pos_mut(srf);
                uv.x = clean_up_noise(uv.x);
                uv.y = clean_up_noise(uv.y);
            }
        }
    }

    Ok(Some(SurfPass { uv_curve, all }))
}

/// Not upstream: the source fork's addition to `CalcFilletGeom`
/// (source issue #1022, WR-468; opt-in
/// `Kernel::fixes.fillet_through_extended_rail_end`). A rail end on an extended edge
/// (`SM_FV_RAIL_X_EXTENDED_EDGEUSE`) is found on the fillet surface only once that is
/// made, so it gives the trace no point, and the surface can stop short of the
/// filleted edge's end when the other rail ends well inside it. At such an end the
/// corner's cap edges meet at the filleted edge's end, so the fillet runs at least to
/// it: the edge's end parameter (`sIvl.GetMin()` / `GetMax()`, whichever end is the
/// corner's filleted vertex) is a through point too. Returns those parameters not
/// already among the G1 knots, in rail and end order; none for a closed edge or
/// without the executive's corners.
///
/// # Errors
///
/// `SM_ERR_NULL_POINTER` for a missing edge use, edge, rail or corner.
fn c1_extended_rail_end_knots(
    ops: &dyn FilletSolverOps,
    cx: &Cx<'_>,
    st: &CalcState,
) -> Result<Vec<f64>> {
    use crate::blend::fillet_geom::p1::FilletVertexType;
    use crate::topology::edge as topo_edge;

    // a standalone entry point has no executive corners to resolve the rail ends'
    // corners with: nothing to add
    let corners = match &cx.solvers {
        GeometrySolvers::Live(context) => context.corners(),
        GeometrySolvers::Indexed(_) => return Ok(Vec::new()),
    };
    let eu0 = ops
        .solver()
        .edgeuse(0)?
        .ok_or_else(|| SolidError::null_pointer(OP))?;
    let edge = eu::edge(cx.target, eu0)?.ok_or_else(|| SolidError::null_pointer(OP))?;
    let start = topo_edge::start_vertex(cx.target, edge)?;
    let end = topo_edge::end_vertex(cx.target, edge)?;
    let mut knots = Vec::new();
    // a closed edge has no end the fillet stops at
    if start == end {
        return Ok(knots);
    }
    for which_rail in 0..2 {
        let rail = cx
            .fillets
            .geom(st.geom0)?
            .rail(which_rail)
            .ok_or_else(|| SolidError::null_pointer(OP))?;
        let brep = cx.fillets.brep();
        let first = topo_edge::vertex(brep, rail)?;
        let second = match first {
            Some(v) => topo_edge::other_vertex(brep, rail, Some(v))?,
            None => None,
        };
        for fv in [first, second].into_iter().flatten() {
            let fillet_vertex = cx.fillets.fillet_vertex(fv)?;
            if fillet_vertex.fillet_vertex_type() != FilletVertexType::RailXExtendedEdgeuse {
                continue;
            }
            let Some(corner) = fillet_vertex.fillet_corner() else {
                continue;
            };
            let Some(corner_vertex) = corners.corner(corner)?.filleted_vertex() else {
                continue;
            };
            let param = if Some(corner_vertex) == start {
                st.ivl.min()
            } else if Some(corner_vertex) == end {
                st.ivl.max()
            } else {
                continue;
            };
            if !ops.solver().g1_knots.contains(&param) && !knots.contains(&param) {
                knots.push(param);
            }
        }
    }
    Ok(knots)
}

#[cfg(test)]
mod tests_c1 {
    use super::*;
    use crate::blend::fillet_custom_solver::SurfaceId;
    use crate::blend::fillet_geom::p1::{
        FilletGeom, FilletVertex, FilletVertexType, FilletVertexuse,
    };
    use crate::blend::fillet_solver::p1::OffsetSurfaceSlot;
    use crate::curve::line::Line;
    use crate::geom::vector2d::Vec2;
    use crate::intersect::surface_intersector::p1::SrfIndex;
    use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
    use crate::topology::{Edgeuse, Vertexuse};

    /// A solver whose `SetExtensionFlags` is a no-op (the base one needs the
    /// executive).
    struct C1Ops(FilletSolver);

    impl FilletSolverOps for C1Ops {
        fn solver(&self) -> &FilletSolver {
            &self.0
        }
        fn solver_mut(&mut self) -> &mut FilletSolver {
            &mut self.0
        }
        fn set_extension_flags(
            &mut self,
            _k: &Kernel,
            _b: &Brep,
            _corners: &dyn crate::blend::fillet_solver::p1::ExtensionCorners,
        ) -> Result<()> {
            Ok(())
        }
    }

    fn plane(z: f64) -> Surface {
        Surface::from(crate::surface::plane::Plane::new(
            Point3::new(0.0, 0.0, z),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec2::new(1.0, 1.0),
            &Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0),
        ))
    }

    fn line_curve(a: [f64; 3], b: [f64; 3], dim: u64) -> Curve {
        Curve::from(Line::from_points(
            Point3::new(a[0], a[1], a[2]),
            Point3::new(b[0], b[1], b[2]),
            dim,
        ))
    }

    /// Target B-rep: one edge (curve `curve`, interval [0,1]) with two edge uses,
    /// each with an open line as UV trim curve.
    fn target(curve: Option<Curve>) -> (Brep, [EdgeuseId; 2]) {
        let mut b = Brep::new();
        let v0 = b.add_vertex(Point3::new(0.0, 0.0, 0.0)).unwrap();
        let v1 = b.add_vertex(Point3::new(1.0, 0.0, 0.0)).unwrap();
        let e = b.add_edge(v0, v1).unwrap();
        b.edge_mut(e).unwrap().curve = curve;
        b.edge_mut(e).unwrap().interval = Extent1d::from_min_max(0.0, 1.0);
        let mut eus = Vec::new();
        for o in [OrientType::Same, OrientType::Opposite] {
            let eu = b
                .add_edgeuse_record(Edgeuse {
                    edge: Some(e),
                    orientation: o,
                    uv_trim_curve: Some(line_curve([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], 2)),
                    ..Edgeuse::default()
                })
                .unwrap();
            eus.push(eu);
        }
        b.edge_mut(e).unwrap().edgeuses.clone_from(&eus);
        (b, [eus[0], eus[1]])
    }

    /// Pseudo B-rep with geometry 0 whose two rails each have two edge uses, the
    /// vertex types and orientations given per rail edge use, and rail-end points with
    /// noisy UVs (`base + 1e-13`).
    fn fillets(
        k: &Kernel,
        spec: [[(FilletVertexType, OrientType, f64); 2]; 2],
    ) -> (FilletBrep, FilletGeomId, Vec<VertexuseId>) {
        let mut f = FilletBrep::new();
        let mut geom = FilletGeom::new(k, None, FilletGeomType::Default).unwrap();
        let mut vus = Vec::new();
        for (rail_index, rail_spec) in spec.iter().enumerate() {
            let va = f.brep_mut().add_vertex(Point3::new(0.0, 0.0, 0.0)).unwrap();
            let vb = f.brep_mut().add_vertex(Point3::new(1.0, 0.0, 0.0)).unwrap();
            let rail = f.new_fillet_edge(va, vb, None).unwrap();
            let mut eus = Vec::new();
            for (j, (vtype, orient, base)) in rail_spec.iter().enumerate() {
                let v = if j == 0 { va } else { vb };
                let vu = f
                    .brep_mut()
                    .add_vertexuse_record(Vertexuse {
                        vertex: Some(v),
                        ..Vertexuse::default()
                    })
                    .unwrap();
                let eu = f
                    .brep_mut()
                    .add_edgeuse_record(Edgeuse {
                        edge: Some(rail),
                        orientation: *orient,
                        vertexuse: Some(vu),
                        ..Edgeuse::default()
                    })
                    .unwrap();
                eus.push(eu);
                let mut fv = FilletVertex::new(k, None);
                fv.set_fillet_vertex_type(*vtype);
                f.insert_fillet_vertex(v, fv);
                let mut fvu = FilletVertexuse::new();
                *fvu.tsect_pnt_mut().uv_pos_mut(SrfIndex::First) =
                    Point2::new(base + 1.0e-13, base + 0.25);
                *fvu.tsect_pnt_mut().uv_pos_mut(SrfIndex::Second) =
                    Point2::new(base + 0.5 - 1.0e-13, 0.123_456_789);
                f.insert_fillet_vertexuse(vu, fvu);
                vus.push(vu);
            }
            f.brep_mut().edge_mut(rail).unwrap().edgeuses = eus;
            geom.set_rail(rail_index, Some(rail));
        }
        let g = f.add_geom(geom);
        (f, g, vus)
    }

    fn solver(k: &Kernel, eus: [EdgeuseId; 2]) -> C1Ops {
        let mut s = FilletSolver::new(1.0e-5, 0.01, 0.01);
        s.edgeuses = [Some(eus[0]), Some(eus[1])];
        for (i, z) in [(0usize, 0.0), (1, 1.0)] {
            let surface =
                OffsetSurface::new(k, 0.5, plane(z), SurfaceId::Free(i as u32), 0).unwrap();
            s.set_surface(
                i,
                Some(OffsetSurfaceSlot {
                    key: i as u64,
                    surface,
                }),
            )
            .unwrap();
        }
        s.fillet_geoms.push(FilletGeomId(0));
        C1Ops(s)
    }

    const ROWS: [[(FilletVertexType, OrientType, f64); 2]; 2] = [
        [
            (FilletVertexType::Unknown, OrientType::Same, 1.0),
            (FilletVertexType::Mate, OrientType::Opposite, 2.0),
        ],
        [
            (
                FilletVertexType::RailXExtendedEdgeuse,
                OrientType::Same,
                3.0,
            ),
            (FilletVertexType::Unknown, OrientType::Opposite, 4.0),
        ],
    ];

    #[test]
    fn setup_reads_edge_surfaces_and_geometry() {
        let k = Kernel::default();
        let (mut tb, eus) = target(Some(line_curve([0.0, 0.0, 0.0], [2.0, 0.0, 0.0], 3)));
        let (mut fb, g, _) = fillets(&k, ROWS);
        let mut ops = solver(&k, eus);
        let mut cx = Cx {
            k: &k,
            target: &mut tb,
            fillets: &mut fb,
            solvers: GeometrySolvers::Indexed(&[]),
            error_info: None,
        };
        let st = c1_setup(&mut ops, &mut cx).unwrap();
        assert_eq!(st.ivl.min(), 0.0);
        assert_eq!(st.ivl.max(), 1.0);
        assert_eq!(st.pv_start[0], Point3::new(0.0, 0.0, 0.0));
        assert_eq!(st.pv_end[0], Point3::new(2.0, 0.0, 0.0));
        assert_eq!(st.geom0, g);
        assert!(!st.split_original_fillet);
        assert!(st.do_sorting);
        assert!(st.tsect_pnts.is_empty());
    }

    #[test]
    fn setup_statuses() {
        let k = Kernel::default();
        let (mut fb, _g, _) = fillets(&k, ROWS);

        // no edge curve: SM_ERR_NULL_POINTER
        let (mut tb, eus) = target(None);
        let mut ops = solver(&k, eus);
        let mut cx = Cx {
            k: &k,
            target: &mut tb,
            fillets: &mut fb,
            solvers: GeometrySolvers::Indexed(&[]),
            error_info: None,
        };
        let e = c1_setup(&mut ops, &mut cx).unwrap_err();
        assert_eq!(e, SolidError::null_pointer(OP));

        // no edge use 0: an upstream NULL dereference
        ops.0.edgeuses = [None, None];
        let e = c1_setup(&mut ops, &mut cx).unwrap_err();
        assert!(matches!(e, SolidError::InvalidInput { .. }));

        // the base SetExtensionFlags fails first (no vertex uses, no executive)
        let mut base = solver(&k, eus).0;
        assert!(c1_setup(&mut base, &mut cx).is_err());
    }

    #[test]
    fn rail_ends_skip_mates_order_by_orientation_and_clean_noise() {
        let k = Kernel::default();
        let (mut tb, eus) = target(Some(line_curve([0.0, 0.0, 0.0], [2.0, 0.0, 0.0], 3)));
        let (mut fb, _g, vus) = fillets(&k, ROWS);
        let mut ops = solver(&k, eus);
        let mut cx = Cx {
            k: &k,
            target: &mut tb,
            fillets: &mut fb,
            solvers: GeometrySolvers::Indexed(&[]),
            error_info: None,
        };
        let mut st = c1_setup(&mut ops, &mut cx).unwrap();
        let pass = c1_collect_points(&mut ops, &mut cx, &mut st, 0)
            .unwrap()
            .unwrap();
        // rail 0: VU0 (SAME, front); VU1 mate skipped; rail 1: VU2 extended skipped,
        // VU3 (OPPOSITE, back)
        let order: Vec<TsRef> = vec![TsRef::Vertexuse(vus[0]), TsRef::Vertexuse(vus[3])];
        assert_eq!(pass.all, order);
        assert!(st.store.local.is_empty());
        assert!(st.do_sorting);
        // CleanUpNoise written into the vertex uses that were collected
        let p0 = cx.fillets.fillet_vertexuse(vus[0]).unwrap().tsect_pnt();
        assert_eq!(p0.uv_pos(SrfIndex::First), Point2::new(1.0, 1.25));
        assert_eq!(p0.uv_pos(SrfIndex::Second), Point2::new(1.5, 0.123_456_789));
        let p3 = cx.fillets.fillet_vertexuse(vus[3]).unwrap().tsect_pnt();
        assert_eq!(p3.uv_pos(SrfIndex::First), Point2::new(4.0, 4.25));
        // skipped ones untouched
        let p1 = cx.fillets.fillet_vertexuse(vus[1]).unwrap().tsect_pnt();
        assert_eq!(p1.uv_pos(SrfIndex::First).x, 2.0 + 1.0e-13);
    }

    #[test]
    fn same_orientation_inserts_in_front() {
        let k = Kernel::default();
        let rows = [
            [
                (FilletVertexType::Unknown, OrientType::Opposite, 1.0),
                (FilletVertexType::Unknown, OrientType::Same, 2.0),
            ],
            [
                (FilletVertexType::Unknown, OrientType::Same, 3.0),
                (FilletVertexType::Unknown, OrientType::Unknown, 4.0),
            ],
        ];
        let (mut tb, eus) = target(Some(line_curve([0.0, 0.0, 0.0], [2.0, 0.0, 0.0], 3)));
        let (mut fb, _g, vus) = fillets(&k, rows);
        let mut ops = solver(&k, eus);
        let mut cx = Cx {
            k: &k,
            target: &mut tb,
            fillets: &mut fb,
            solvers: GeometrySolvers::Indexed(&[]),
            error_info: None,
        };
        let mut st = c1_setup(&mut ops, &mut cx).unwrap();
        let pass = c1_collect_points(&mut ops, &mut cx, &mut st, 1)
            .unwrap()
            .unwrap();
        let v = |i: usize| TsRef::Vertexuse(vus[i]);
        assert_eq!(pass.all, vec![v(2), v(1), v(0), v(3)]);
    }

    #[test]
    fn rail_with_wrong_edgeuse_count_is_sm_err() {
        let k = Kernel::default();
        let (mut tb, eus) = target(Some(line_curve([0.0, 0.0, 0.0], [2.0, 0.0, 0.0], 3)));
        let (mut fb, g, _) = fillets(&k, ROWS);
        let rail = fb.geom(g).unwrap().rail(1).unwrap();
        fb.brep_mut().edge_mut(rail).unwrap().edgeuses.pop();
        let mut ops = solver(&k, eus);
        let mut cx = Cx {
            k: &k,
            target: &mut tb,
            fillets: &mut fb,
            solvers: GeometrySolvers::Indexed(&[]),
            error_info: None,
        };
        let mut st = c1_setup(&mut ops, &mut cx).unwrap();
        let e = c1_collect_points(&mut ops, &mut cx, &mut st, 0).unwrap_err();
        assert_eq!(e, SolidError::sm_err(OP));
    }

    #[test]
    fn g1_knot_point_error_leaves_the_split_knot() {
        let k = Kernel::default();
        let (mut tb, eus) = target(Some(line_curve([0.0, 0.0, 0.0], [2.0, 0.0, 0.0], 3)));
        let (mut fb, g, _) = fillets(&k, ROWS);
        fb.geom_mut(g).unwrap().need_split = true;
        let mut ops = solver(&k, eus);
        let mut cx = Cx {
            k: &k,
            target: &mut tb,
            fillets: &mut fb,
            solvers: GeometrySolvers::Indexed(&[]),
            error_info: None,
        };
        let mut st = c1_setup(&mut ops, &mut cx).unwrap();
        assert!(st.split_original_fillet);
        // split without G1 knots: sIvl.Evaluate(0.5) is pushed, then the knot loop's
        // EvaluateBinormal fails (the edge use is in no loop: SM_ERR_INVALID_INPUT) and
        // returns before RemoveLast
        let e = c1_collect_points(&mut ops, &mut cx, &mut st, 0).unwrap_err();
        assert_eq!(
            e,
            SolidError::Upstream {
                operation: "topology::edgeuse::p3",
                code: crate::error::SM_ERR_INVALID_INPUT,
                entity: None,
            }
        );
        assert_eq!(ops.0.g1_knots, vec![0.5]);
        assert_eq!(st.split_tsect_pnt, None);
    }

    /// A closed degree-1 B-spline triangle over [0,1] in the UV plane.
    fn closed_uv_curve() -> Curve {
        use crate::nurbs::math::CPoint;
        let pw = [
            CPoint::new(0.0, 0.0, 0.0, 1.0),
            CPoint::new(1.0, 0.0, 0.0, 1.0),
            CPoint::new(1.0, 1.0, 0.0, 1.0),
            CPoint::new(0.0, 0.0, 0.0, 1.0),
        ];
        let u = [0.0, 0.0, 1.0 / 3.0, 2.0 / 3.0, 1.0, 1.0];
        Curve::from(BSplineCurve::from_data(2, 3, 5, 1, &pw, &u).unwrap())
    }

    #[test]
    fn closed_trim_curves() {
        let k = Kernel::default();
        let (mut tb, eus) = target(Some(line_curve([0.0, 0.0, 0.0], [2.0, 0.0, 0.0], 3)));
        for eu in eus {
            tb.edgeuse_mut(eu).unwrap().uv_trim_curve = Some(closed_uv_curve());
        }
        let (mut fb, _g, vus) = fillets(&k, ROWS);
        let mut ops = solver(&k, eus);
        let mut cx = Cx {
            k: &k,
            target: &mut tb,
            fillets: &mut fb,
            solvers: GeometrySolvers::Indexed(&[]),
            error_info: None,
        };
        let mut st = c1_setup(&mut ops, &mut cx).unwrap();
        // pass 0 closed: continue (nothing collected, nothing cleaned)
        assert!(c1_collect_points(&mut ops, &mut cx, &mut st, 0)
            .unwrap()
            .is_none());
        assert!(st.do_sorting);
        let p0 = cx.fillets.fillet_vertexuse(vus[0]).unwrap().tsect_pnt();
        assert_eq!(p0.uv_pos(SrfIndex::First).x, 1.0 + 1.0e-13);
        // pass 1 closed: no sorting, collection goes on
        let pass = c1_collect_points(&mut ops, &mut cx, &mut st, 1)
            .unwrap()
            .unwrap();
        assert!(!st.do_sorting);
        assert_eq!(pass.all.len(), 2);
    }

    #[test]
    fn missing_trim_curve_creation_status_is_returned() {
        let k = Kernel::default();
        let (mut tb, eus) = target(Some(line_curve([0.0, 0.0, 0.0], [2.0, 0.0, 0.0], 3)));
        for eu in eus {
            tb.edgeuse_mut(eu).unwrap().uv_trim_curve = None;
        }
        let (mut fb, _g, _) = fillets(&k, ROWS);
        let mut ops = solver(&k, eus);
        let mut cx = Cx {
            k: &k,
            target: &mut tb,
            fillets: &mut fb,
            solvers: GeometrySolvers::Indexed(&[]),
            error_info: None,
        };
        let mut st = c1_setup(&mut ops, &mut cx).unwrap();
        // GetOrCreateUVTrimCurve builds the missing curve; CreateUVTrimCurve's failing
        // status (here NER) is SER'd
        let e = c1_collect_points(&mut ops, &mut cx, &mut st, 0).unwrap_err();
        assert_eq!(
            e,
            SolidError::Upstream {
                operation: "topology::edgeuse::p3",
                code: crate::error::SM_ERR_NULL_POINTER,
                entity: None,
            }
        );
    }

    /// The executive's corner table, for [`c1_extended_rail_end_knots`]; the other
    /// context calls are not reached.
    struct CornersOnly(
        Vec<crate::blend::fillet_corner::p3::FilletCorner>,
        NoCorners,
    );

    impl FilletGeometryContext for CornersOnly {
        fn set_extension_flags(
            &self,
            _solver: &mut dyn FilletSolverOps,
            _k: &Kernel,
            _target: &Brep,
        ) -> Result<()> {
            Ok(())
        }
        fn solver<'s>(
            &'s self,
            _id: FilletSolverId,
            current: &'s FilletSolver,
        ) -> Result<&'s FilletSolver> {
            Ok(current)
        }
        fn corners(&self) -> CornerLookup<'_> {
            CornerLookup::table(&self.0)
        }
        fn split_corners(&mut self) -> &mut dyn SplitCorners {
            &mut self.1
        }
    }

    /// source issue #1022: a rail of geometry 0 that ends on an extended edge
    /// (`SM_FV_RAIL_X_EXTENDED_EDGEUSE`) at the corner of the filleted edge's start
    /// (or end) makes that end parameter a through knot, once, and only for a corner at
    /// one of the edge's ends.
    #[test]
    fn issue_1022_extended_rail_end_adds_the_edge_end_as_a_through_knot() {
        use crate::blend::fillet_corner::p3::{FilletCorner, FilletCornerKind};
        use crate::blend::fillet_geom::p1::FilletCornerId;

        let k = Kernel::default();
        // the target edge v0 -> v1 with a vertex use on each edge use (start, end)
        let (mut tb, eus) = target(Some(line_curve([0.0, 0.0, 0.0], [2.0, 0.0, 0.0], 3)));
        let edge = eu::edge(&tb, eus[0]).unwrap().unwrap();
        let [v0, v1] = tb.edge(edge).unwrap().vertices;
        for (e, v) in [(eus[0], v0), (eus[1], v1)] {
            let vu = tb
                .add_vertexuse_record(Vertexuse {
                    vertex: Some(v),
                    ..Vertexuse::default()
                })
                .unwrap();
            tb.edgeuse_mut(e).unwrap().vertexuse = Some(vu);
        }
        // ROWS: rail 1's first end is the only SM_FV_RAIL_X_EXTENDED_EDGEUSE vertex
        let (mut fb, _g, _) = fillets(&k, ROWS);
        let extended: Vec<_> = crate::topology::brep::p1::vertices(fb.brep(), None)
            .unwrap()
            .into_iter()
            .filter(|&v| {
                fb.fillet_vertex(v)
                    .is_ok_and(|f| f.fillet_vertex_type() == FilletVertexType::RailXExtendedEdgeuse)
            })
            .collect();
        assert_eq!(extended.len(), 1);
        fb.fillet_vertex_mut(extended[0])
            .unwrap()
            .set_fillet_corner(Some(FilletCornerId(0)));

        for (corner_vertex, want) in [(v0, vec![0.0]), (v1, vec![1.0])] {
            let corner = FilletCorner::new(
                FilletCornerKind::Open,
                Some(corner_vertex),
                None,
                None,
                1e-6,
                1e-3,
            );
            let mut ctx = CornersOnly(vec![corner], NoCorners);
            let mut ops = solver(&k, eus);
            let mut cx = Cx {
                k: &k,
                target: &mut tb,
                fillets: &mut fb,
                solvers: GeometrySolvers::Live(&mut ctx),
                error_info: None,
            };
            let st = c1_setup(&mut ops, &mut cx).unwrap();
            assert_eq!(c1_extended_rail_end_knots(&ops, &cx, &st).unwrap(), want);
            // a knot already among the G1 knots is not added again
            ops.0.g1_knots.push(want[0]);
            assert!(c1_extended_rail_end_knots(&ops, &cx, &st)
                .unwrap()
                .is_empty());
        }

        // without the executive's corners (a standalone entry point): none
        let mut ops = solver(&k, eus);
        let mut cx = Cx {
            k: &k,
            target: &mut tb,
            fillets: &mut fb,
            solvers: GeometrySolvers::Indexed(&[]),
            error_info: None,
        };
        let st = c1_setup(&mut ops, &mut cx).unwrap();
        assert!(c1_extended_rail_end_knots(&ops, &cx, &st)
            .unwrap()
            .is_empty());
    }
}

// P4-CHUNK(C1) END

// =====================================================================================
// P4-CHUNK(C2) BEGIN: sorting and end cleanup, SmFilletSolver.cpp:1754-1920
// (chunk-local imports go inside the functions: never edit the shared `use` block)
// =====================================================================================

/// :1754-1858: `dScale`; for each of `pass.all` (the `-SM_BIG_DOUBLE` UV skip kept),
/// `GlobalPointSolve` on `pass.uv_curve` (exactly one solution, else `SM_ERR`), the
/// boundary adjustment along the tangent line, and the deduplicating insertion sort
/// into `st.tsect_pnts` that marks each inserted point's `curve_parameter` with
/// `-SM_BIG_DOUBLE` (written back through its handle).
fn c2_sort_points(
    ops: &mut dyn FilletSolverOps,
    cx: &mut Cx<'_>,
    st: &mut CalcState,
    which_surf: usize,
    pass: &SurfPass,
) -> Result<()> {
    use crate::curve::curve::p1::GlobalPointSolveOptions;
    use crate::geom::geom_utility::p1::line_closest_point;
    use crate::geom::global_solver::{SolutionRequested, SolverOperation};
    use crate::math::{BIG_DOUBLE, EFF_ZERO};

    // UVPos(lWhichSurf) (upstream indexes past the pair for other values)
    let srf = c2_srf_index(which_surf)?;
    let approx_tol_3d = ops.solver().approx_tol_3d;

    let d_scale = st.ivl.min().abs() + st.ivl.max().abs() + 1.0;
    // sProjParams
    let mut proj_params: Vec<f64> = Vec::new();
    for &r in &pass.all {
        let uv = st.store.get(cx.fillets, r)?.uv_pos(srf);
        // SmPoint3d sUVPnt(sUV): z = 0
        let uv_pnt = Point3::new(uv.x, uv.y, 0.0);

        // skip points set to -SM_BIG_DOUBLE (upstream's comment says the sort marks
        // them so, but the mark goes to m_dCurveParameter: kept as written)
        if uv.x == -BIG_DOUBLE || uv.y == -BIG_DOUBLE {
            continue;
        }

        // drop the UV point to pUVCurveFilletedEdge: dT = the drop parameter
        let solutions = pass.uv_curve.global_point_solve::<NoObject>(
            cx.k,
            &st.ivl,
            SolverOperation::Minimize,
            uv_pnt,
            approx_tol_3d,
            SolutionRequested::Single,
            &GlobalPointSolveOptions::default(),
        )?;
        if solutions.len() != 1 {
            return Err(SolidError::sm_err(OP));
        }
        let mut t = solutions
            .get(0)
            .ok_or_else(|| SolidError::sm_err(OP))?
            .start
            .param(0)?;

        // a drop to the edge's end point may lie beyond it (the edge needs extending):
        // drop to the tangent extension line instead
        if st.ivl.is_value_on_boundary(t, EFF_ZERO).0 {
            let pv = pass.uv_curve.evaluate(cx.k, t, 1, true, false)?;
            let p0 = *pv
                .first()
                .ok_or_else(|| invalid("short curve evaluation"))?;
            let p1 = *pv.get(1).ok_or_else(|| invalid("short curve evaluation"))?;
            let adjustment = line_closest_point(p0, p1, uv_pnt);
            t += adjustment;
        }

        // sort by dT values - don't insert points already present
        if let Some(index) = c2_insert_index(&proj_params, t, d_scale) {
            // sTsectPnts and sProjParams grow together from empty
            if index > st.tsect_pnts.len() {
                return Err(invalid("sorted intersection point index out of range"));
            }
            proj_params.insert(index, t);
            st.store.get_mut(cx.fillets, r)?.curve_parameter = -BIG_DOUBLE;
            st.tsect_pnts.insert(index, r);
        }
    }
    Ok(())
}

/// `lWhichSurf` as a `UVPos` index.
fn c2_srf_index(which_surf: usize) -> Result<crate::intersect::surface_intersector::p1::SrfIndex> {
    use crate::intersect::surface_intersector::p1::SrfIndex;
    match which_surf {
        0 => Ok(SrfIndex::First),
        1 => Ok(SrfIndex::Second),
        _ => Err(invalid("surface index past the pair")),
    }
}

/// The `kk` loop of :1830-1843: `None` when `t` is within `SM_EFF_ZERO_SQRT*dScale`
/// of a parameter already present (the scan stops at the first entry that is either
/// that close or greater than `t`), else the insert index.
fn c2_insert_index(proj_params: &[f64], t: f64, d_scale: f64) -> Option<usize> {
    use crate::math::EFF_ZERO_SQRT;
    let mut index_of_insert = proj_params.len();
    for (kk, &p) in proj_params.iter().enumerate() {
        if (t - p).abs() < EFF_ZERO_SQRT * d_scale {
            return None;
        } else if t < p {
            index_of_insert = kk;
            break;
        }
    }
    Some(index_of_insert)
}

/// `pOffsetSrf1->EvaluatePoint(uv, sPnt)` with its status ignored: a failed
/// evaluation leaves the fresh `SmPoint3d` at `SM_UNDEF_DOUBLE` ([`Vec3::UNDEF`]);
/// `Unported` still propagates. A missing `GetSurface(0)` (a `NULL` dereference
/// upstream, ruled out by setup) is `invalid`, not ignored.
fn c2_offset_point_ignored(
    k: &Kernel,
    ops: &mut dyn FilletSolverOps,
    uv: Point2,
) -> Result<Point3> {
    let surface = offset_surface_mut(ops, 0)?;
    Ok(
        crate::error::ignored(surface.surface.evaluate_point_extending(k, uv))?
            .unwrap_or(Vec3::UNDEF),
    )
}

/// :1872-1920: `SM_ERR` without points; with more than two, drop the penultimate
/// point when the last two are within `m_dThisApproxTol3d` on `GetSurface(0)` (UV of
/// surface 0, statuses ignored), then the second when the first two are.
fn c2_clean_ends(ops: &mut dyn FilletSolverOps, cx: &mut Cx<'_>, st: &mut CalcState) -> Result<()> {
    use crate::intersect::surface_intersector::p1::SrfIndex;

    // check state
    let mut total_tsects = st.tsect_pnts.len();
    if total_tsects == 0 {
        return Err(SolidError::sm_err(OP));
    }
    let approx_tol_3d = ops.solver().approx_tol_3d;

    // the last two points very close to each other: remove the next to last one
    if total_tsects > 2 {
        let uv1 = st
            .point(cx.fillets, total_tsects - 1)?
            .uv_pos(SrfIndex::First);
        let pnt1 = c2_offset_point_ignored(cx.k, ops, uv1)?;
        let uv2 = st
            .point(cx.fillets, total_tsects - 2)?
            .uv_pos(SrfIndex::First);
        let pnt2 = c2_offset_point_ignored(cx.k, ops, uv2)?;
        let v = pnt1 - pnt2;
        let len = v.length();

        if len < approx_tol_3d {
            // sTsectPnts[n-2] = sTsectPnts[n-1]; sTsectPnts.RemoveLast()
            let last = st
                .tsect_pnts
                .pop()
                .ok_or_else(|| invalid("intersection point index out of range"))?;
            let penultimate = st
                .tsect_pnts
                .last_mut()
                .ok_or_else(|| invalid("intersection point index out of range"))?;
            *penultimate = last;
            total_tsects -= 1;
        }
    }

    // the first two points very close to each other: remove the second one
    if total_tsects > 2 {
        let uv1 = st.point(cx.fillets, 0)?.uv_pos(SrfIndex::First);
        let pnt1 = c2_offset_point_ignored(cx.k, ops, uv1)?;
        let uv2 = st.point(cx.fillets, 1)?.uv_pos(SrfIndex::First);
        let pnt2 = c2_offset_point_ignored(cx.k, ops, uv2)?;
        let v = pnt1 - pnt2;
        let len = v.length();

        if len < approx_tol_3d {
            // sTsectPnts.RemoveAt(1) (more than two entries here)
            st.tsect_pnts.remove(1);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests_c2 {
    use super::*;
    use crate::blend::fillet_custom_solver::{ConstantRadiusAssistedFs, SurfaceId};
    use crate::geom::vector2d::Vec2;
    use crate::intersect::surface_intersector::p1::SrfIndex;
    use crate::math::BIG_DOUBLE;

    /// The plane z = 0 over `[0,1]^2` (u along x, v along y).
    fn plane() -> Surface {
        Surface::from(crate::surface::plane::Plane::new(
            Point3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec2::new(1.0, 1.0),
            &Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0),
        ))
    }

    fn solver(k: &Kernel, tol: f64) -> ConstantRadiusAssistedFs {
        let off =
            |d: f64, id: u32| OffsetSurface::new(k, d, plane(), SurfaceId::Free(id), 0).unwrap();
        ConstantRadiusAssistedFs::from_parts(
            tol,
            1e-3,
            1e-6,
            [0.25, 0.25],
            [Some(off(0.25, 0)), Some(off(0.25, 1))],
            [1.0, 1.0],
            [None, None],
        )
    }

    /// Setup state over the edge interval `[0,1]` with local points at `uvs` (UV of
    /// both surfaces) in that order.
    fn state(uvs: &[(f64, f64)]) -> (CalcState, Vec<TsRef>) {
        let line = Curve::from(
            BSplineCurve::create_line_segment(
                3,
                Point3::new(0.0, 0.0, 0.0),
                Point3::new(1.0, 0.0, 0.0),
            )
            .unwrap(),
        );
        let mut st = CalcState::new(
            line,
            Extent1d::from_min_max(0.0, 1.0),
            [Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0)],
            [Vec3::new(1.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0)],
            [plane(), plane()],
            FilletGeomId(0),
            false,
        );
        let refs = uvs
            .iter()
            .map(|&(u, v)| {
                let mut p = TsectPnt::new();
                *p.uv_pos_mut(SrfIndex::First) = Point2::new(u, v);
                *p.uv_pos_mut(SrfIndex::Second) = Point2::new(u, v);
                p.curve_parameter = 0.5;
                st.store.alloc(p)
            })
            .collect();
        (st, refs)
    }

    /// The UV trim curve of the edge: u from 0 to 1 at v = 0.5 (z = 0).
    fn uv_line() -> Curve {
        Curve::from(
            BSplineCurve::create_line_segment(
                3,
                Point3::new(0.0, 0.5, 0.0),
                Point3::new(1.0, 0.5, 0.0),
            )
            .unwrap(),
        )
    }

    #[test]
    fn insert_index_dedups_and_orders() {
        let p = [0.1, 0.5, 0.9];
        assert_eq!(c2_insert_index(&[], 0.3, 1.0), Some(0));
        assert_eq!(c2_insert_index(&p, 0.0, 1.0), Some(0));
        assert_eq!(c2_insert_index(&p, 0.3, 1.0), Some(1));
        assert_eq!(c2_insert_index(&p, 0.95, 1.0), Some(3));
        assert_eq!(c2_insert_index(&p, 0.5 + 0.5e-6, 1.0), None);
        // the threshold scales with dScale
        assert_eq!(c2_insert_index(&p, 0.5 + 2.0e-6, 1.0), Some(2));
        assert_eq!(c2_insert_index(&p, 0.5 + 2.0e-6, 3.0), None);
        // the scan stops at the first greater entry: no dedup against later ones
        assert_eq!(c2_insert_index(&[0.2, 0.2], 0.1, 1.0), Some(0));
    }

    #[test]
    fn sort_orders_dedups_marks_and_extends_past_the_end() {
        let k = Kernel::default();
        let mut fs = solver(&k, 1e-6);
        let mut target = Brep::new();
        let mut fillets = FilletBrep::new();
        let mut cx = Cx {
            k: &k,
            target: &mut target,
            fillets: &mut fillets,
            solvers: GeometrySolvers::Indexed(&[]),
            error_info: None,
        };
        let (mut st, refs) = state(&[
            (0.7, 0.4),
            (-BIG_DOUBLE, 0.5), // skipped
            (0.2, 0.6),
            (0.2 + 1.0e-8, 0.5), // duplicate of the previous drop
            (1.25, 0.5),         // beyond the edge end: tangent-line drop
        ]);
        let pass = SurfPass {
            uv_curve: uv_line(),
            all: refs.clone(),
        };
        c2_sort_points(&mut fs, &mut cx, &mut st, 1, &pass).unwrap();
        assert_eq!(st.tsect_pnts, vec![refs[2], refs[0], refs[4]]);
        for (i, r) in refs.iter().enumerate() {
            let cp = st.store.get(cx.fillets, *r).unwrap().curve_parameter;
            let marked = matches!(i, 0 | 2 | 4);
            assert_eq!(cp, if marked { -BIG_DOUBLE } else { 0.5 }, "point {i}");
        }
        assert!(c2_sort_points(&mut fs, &mut cx, &mut st, 2, &pass).is_err());
    }

    #[test]
    fn clean_ends_drops_close_end_points() {
        let k = Kernel::default();
        let mut fs = solver(&k, 1e-3);
        let mut target = Brep::new();
        let mut fillets = FilletBrep::new();
        let mut cx = Cx {
            k: &k,
            target: &mut target,
            fillets: &mut fillets,
            solvers: GeometrySolvers::Indexed(&[]),
            error_info: None,
        };

        // no points
        let (mut st, _) = state(&[]);
        let e = c2_clean_ends(&mut fs, &mut cx, &mut st).unwrap_err();
        assert!(matches!(e, SolidError::Upstream { .. }));

        // two points are never pruned, however close
        let (mut st, refs) = state(&[(0.5, 0.5), (0.5, 0.5)]);
        st.tsect_pnts = refs.clone();
        c2_clean_ends(&mut fs, &mut cx, &mut st).unwrap();
        assert_eq!(st.tsect_pnts, refs);

        // close at both ends: the penultimate goes (the last stays), then the second
        let (mut st, refs) = state(&[
            (0.1, 0.5),
            (0.1, 0.5001),
            (0.5, 0.5),
            (0.9, 0.5001),
            (0.9, 0.5),
        ]);
        st.tsect_pnts = refs.clone();
        c2_clean_ends(&mut fs, &mut cx, &mut st).unwrap();
        assert_eq!(st.tsect_pnts, vec![refs[0], refs[2], refs[4]]);

        // three points, last two close: only the end test runs before n drops to 2
        let (mut st, refs) = state(&[(0.1, 0.5), (0.1, 0.5001), (0.1, 0.5002)]);
        st.tsect_pnts = refs.clone();
        c2_clean_ends(&mut fs, &mut cx, &mut st).unwrap();
        assert_eq!(st.tsect_pnts, vec![refs[0], refs[2]]);

        // far apart: unchanged
        let (mut st, refs) = state(&[(0.1, 0.5), (0.5, 0.5), (0.9, 0.5)]);
        st.tsect_pnts = refs.clone();
        c2_clean_ends(&mut fs, &mut cx, &mut st).unwrap();
        assert_eq!(st.tsect_pnts, refs);
    }
}

// P4-CHUNK(C2) END

// =====================================================================================
// P4-CHUNK(C3) BEGIN: analytic attempt and trace setup, SmFilletSolver.cpp:1955-2090
// (chunk-local imports go inside the functions: never edit the shared `use` block)
// =====================================================================================

/// :1955-2011: under `CanMakeAnalytic()`, the base surfaces as B-splines (one offset
/// level unwrapped, `NER`), `sm_TestAnalyticSurface` on both, then
/// `CalcCylinderFilletGeom` and `CalcTorusFilletGeom`. `true` when one succeeded
/// (`return SM_SUCCESS`); their failures fall through to the trace.
fn c3_try_analytic(
    ops: &mut dyn FilletSolverOps,
    cx: &mut Cx<'_>,
    st: &mut CalcState,
) -> Result<bool> {
    use super::p1::{test_analytic_surface, AnalyticTestSurface};

    // when building a fillet that can be analytic
    if !ops.can_make_analytic()? {
        return Ok(false);
    }

    // get filletEdge->Surface1, then filletEdge->Surface2 (each NER)
    let base1 = c3_bspline_base(&st.base_surfaces[0])?;
    let base2 = c3_bspline_base(&st.base_surfaces[1])?;

    // when both surfaces are analytic (planes or cones); `&&` short-circuits as
    // upstream's does (the second test only runs when the first passes)
    let both_analytic = test_analytic_surface(cx.k, AnalyticTestSurface::Surface(base1))?
        && test_analytic_surface(cx.k, AnalyticTestSurface::Surface(base2))?;
    if !both_analytic {
        return Ok(false);
    }

    // try making a cylindrical fillet to store in m_vFilletGeoms[0]; quit when this
    // succeeds (a failure falls through, an unported callee does not)
    match calc_cylinder_fillet_geom(ops, cx, &mut st.store, &st.tsect_pnts) {
        Ok(()) => return Ok(true),
        Err(e) if e.is_unported() => return Err(e),
        Err(_) => {}
    }

    // try making a torus fillet to store in m_vFilletGeoms[0]; quit when this succeeds
    match calc_torus_fillet_geom(ops, cx, &mut st.store, &st.tsect_pnts) {
        Ok(()) => return Ok(true),
        Err(e) if e.is_unported() => return Err(e),
        Err(_) => {}
    }

    // WR-420 (fork addition, opt-in): where the centre line collapses to a point, the
    // corner ball's patch
    if !cx.k.fixes.fillet_corner_ball_patch {
        return Ok(false);
    }
    let tsects = st.store.snapshot(cx.fillets, &st.tsect_pnts)?;
    match FilletSolver::calc_sphere_fillet_geom(ops, cx.k, cx.target, cx.fillets, &tsects, None) {
        Ok(()) => Ok(true),
        Err(e) if e.is_unported() => Err(e),
        Err(_) => Ok(false),
    }
}

/// :1960-1977: `SM_CAST_PTR(SmBSplineSurface, pBaseSrf)`, else (another level of
/// offset surface) `SM_CAST_PTR(SmBSplineSurface, pOffsetSurf->GetBaseSurface())`;
/// `NER` when the base is not an offset surface or its base not a B-spline.
fn c3_bspline_base(base: &Surface) -> Result<&Surface> {
    if is_bspline_surface(base) {
        return Ok(base);
    }
    // If the base surface is not an SmBSplineSurface, maybe it's another level of
    // offset surface; check one more level.
    let SurfaceKind::OffsetSurface(offset) = base.kind() else {
        return Err(SolidError::null_pointer(OP));
    };
    let inner = offset.base_surface();
    if is_bspline_surface(inner) {
        Ok(inner)
    } else {
        Err(SolidError::null_pointer(OP))
    }
}

/// :2028-2034: `st.trace.domains` = the base surfaces' natural domains after
/// `SetupOffsetExtension`; returns the solver's offset surfaces as [`Surface`]s for
/// the intersector.
fn c3_trace_setup(
    ops: &mut dyn FilletSolverOps,
    cx: &mut Cx<'_>,
    st: &mut CalcState,
) -> Result<[Surface; 2]> {
    // get fillet baseSurface domains
    let mut domain1 = st.base_surfaces[0].natural_uv_domain_if_built();
    let mut domain2 = st.base_surfaces[1].natural_uv_domain_if_built();

    // extend baseSurface domains by m_dSurfaceExtensionFactor * rDomain.GetSize();
    // don't extend in periodic directions nor through singular boundaries
    ops.solver()
        .setup_offset_extension(cx.k, &st.base_surfaces[0], &mut domain1)?;
    ops.solver()
        .setup_offset_extension(cx.k, &st.base_surfaces[1], &mut domain2)?;
    st.trace.domains = [domain1, domain2];

    // *pOffsetSrf1, *pOffsetSrf2 (copies the intersector borrows)
    let surface1 = Surface::from(offset_surface(ops, 0)?.surface.clone());
    let surface2 = Surface::from(offset_surface(ops, 1)?.surface.clone());
    Ok([surface1, surface2])
}

/// :2037-2052: `SmFilletIntersector sFI(*pOffsetSrf1, sDomain1, *pOffsetSrf2,
/// sDomain2, *this)`; with an extension flag set and an arc edge curve
/// (`IsArc(5, m_dThisApproxTol3d, ..)`), `SetExtensionDistance(|5 * offset 0|)`.
fn c3_make_intersector<'s>(
    ops: &mut dyn FilletSolverOps,
    k: &'s Kernel,
    st: &CalcState,
    surfaces: &'s [Surface; 2],
) -> Result<TraceIntersector<'s>> {
    let [surface1, surface2] = surfaces;
    let [domain1, domain2] = &st.trace.domains;
    // the solver (`*this`) is passed to each intersector call instead
    let mut fi = TraceIntersector::new(k, surface1, domain1, surface2, domain2, None)?;

    // If we need to trace beyond the end and we are tracing along an arc, put a stop
    // tracing criterion to avoid possible infinite looping (`IsArc` only runs when an
    // extension flag is set, as upstream's `&&`).
    let solver = ops.solver();
    if (solver.extend_after || solver.extend_before)
        && st.edge_curve.is_arc(k, 5, solver.approx_tol_3d)?.is_some()
    {
        // set max extension distance
        let offset = offset_surface(ops, 0)?.offset_distance();
        fi.base.base.set_extension_distance((5.0 * offset).abs());
    }
    Ok(fi)
}

/// :2054-2090: the trace locals (tolerances, `sDir = sPVSt[1]`); a TANGENT last point
/// reverses `st.tsect_pnts`, sets `sDir = -sPVEn[1]` and `bReverseTracing`; the local
/// extension flags from the solver's.
fn c3_trace_start(
    ops: &mut dyn FilletSolverOps,
    cx: &mut Cx<'_>,
    st: &mut CalcState,
) -> Result<()> {
    use crate::intersect::surface_intersector::p1::IntersectionPointType;

    let solver = ops.solver();
    st.trace.approx_tol_3d = solver.this_approx_tol_3d();
    st.trace.angle_tol = solver.this_ang_tol_rad();
    // intersection trace start direction
    st.trace.dir = st.pv_start[1];

    // Check the last TsectPnt to see if it is degenerate (at a surf/surf tangency
    // point). If so, reverse the tracing to avoid its early termination.
    st.trace.reverse_tracing = false;
    if let Some(&last) = st.tsect_pnts.last() {
        if st.store.get(cx.fillets, last)?.point_type == IntersectionPointType::TangentPoint {
            // sTsectPnts.ReverseArray(0, lTotalTsects)
            st.tsect_pnts.reverse();
            st.trace.dir = -st.pv_end[1];
            st.trace.reverse_tracing = true;
        }
    }

    st.trace.extend_before = solver.extend_before;
    st.trace.extend_after = solver.extend_after;
    Ok(())
}

#[cfg(test)]
mod tests_c3 {
    use super::*;
    use crate::blend::fillet_custom_solver::{ConstantRadiusAssistedFs, SurfaceId};
    use crate::curve::ellipse::{Ellipse, EllipseOptions};
    use crate::curve::line::Line;
    use crate::intersect::surface_intersector::p1::IntersectionPointType;
    use crate::nurbs::b_spline_surface::BSplineSurface;
    use crate::surface::offset_surface;

    fn bilinear(p: [[f64; 3]; 4]) -> Surface {
        let [a, b, c, d] = p.map(Vec3::from);
        Surface::from(BSplineSurface::create_bilinear_surface(a, b, c, d).unwrap())
    }

    /// The plane z = 0 and the plane x = 0 over the unit square.
    fn planes() -> (Surface, Surface) {
        (
            bilinear([
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [1.0, 1.0, 0.0],
            ]),
            bilinear([
                [0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
                [0.0, 1.0, 1.0],
            ]),
        )
    }

    /// The planes of [`planes`] as `SmPlane`s (`sm_TestAnalyticSurface` accepts a
    /// plane without the planarity test).
    fn analytic_planes() -> (Surface, Surface) {
        use crate::geom::vector2d::Vec2;
        use crate::surface::plane::Plane;
        let domain = planes().0.natural_uv_domain_if_built();
        let plane = |x: [f64; 3], y: [f64; 3]| {
            Surface::from(Plane::new(
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::from(x),
                Vec3::from(y),
                Vec2::new(1.0, 1.0),
                &domain,
            ))
        };
        (
            plane([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
            plane([0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
        )
    }

    fn offset(k: &Kernel, d: f64, base: &Surface) -> Surface {
        Surface::from(offset_surface::OffsetSurface::new(k, d, base.clone(), false, 0).unwrap())
    }

    fn solver(k: &Kernel, a: &Surface, b: &Surface) -> ConstantRadiusAssistedFs {
        ConstantRadiusAssistedFs::from_parts(
            0.001,
            0.5,
            0.03,
            [0.25, 0.25],
            [
                Some(OffsetSurface::new(k, 0.25, a.clone(), SurfaceId::Free(0), 0).unwrap()),
                Some(OffsetSurface::new(k, -0.25, b.clone(), SurfaceId::Free(1), 0).unwrap()),
            ],
            [1.0, -1.0],
            [None, None],
        )
    }

    fn state(curve: Curve, bases: [Surface; 2]) -> CalcState {
        CalcState::new(
            curve,
            Extent1d::from_min_max(0.0, 1.0),
            [Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0)],
            [Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, 2.0, 0.0)],
            bases,
            FilletGeomId(0),
            false,
        )
    }

    fn line() -> Curve {
        Curve::from(Line::from_points(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            3,
        ))
    }

    fn arc(k: &Kernel) -> Curve {
        Curve::from(
            Ellipse::new_circle(
                k,
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(1.0, 0.0, 0.0),
                Vec3::new(0.0, 1.0, 0.0),
                Extent1d::from_min_max(0.0, 90.0),
                2.0,
                &EllipseOptions::default(),
            )
            .unwrap(),
        )
    }

    fn with_cx<R>(k: &Kernel, f: impl FnOnce(&mut Cx<'_>) -> R) -> R {
        let mut target = Brep::new();
        let mut fillets = FilletBrep::new();
        let mut cx = Cx {
            k,
            target: &mut target,
            fillets: &mut fillets,
            solvers: GeometrySolvers::Indexed(&[]),
            error_info: None,
        };
        f(&mut cx)
    }

    #[test]
    fn analytic_planes_reach_the_cylinder_attempt() {
        let k = Kernel::default();
        let (a, b) = planes();
        let mut fs = solver(&k, &a, &b);
        let (pa, pb) = analytic_planes();
        let mut st = state(line(), [pa, pb]);
        // CalcCylinderFilletGeom and CalcTorusFilletGeom (blend.fillet_solver_p2) run
        // and fail on this bare solver (no fillet geometry): no analytic fillet
        let r = with_cx(&k, |cx| c3_try_analytic(&mut fs, cx, &mut st));
        assert!(matches!(r, Ok(false)), "{r:?}");
    }

    #[test]
    fn analytic_needs_can_make_analytic() {
        let k = Kernel::default();
        let (a, b) = planes();
        // the base solver: OffsetRadiiCanChange TRUE, so CanMakeAnalytic FALSE
        let mut fs = solver(&k, &a, &b).solver;
        let mut st = state(line(), [a, b]);
        let r = with_cx(&k, |cx| c3_try_analytic(&mut fs, cx, &mut st));
        assert!(matches!(r, Ok(false)));
    }

    #[test]
    fn bspline_base_unwraps_one_offset_level() {
        let k = Kernel::default();
        let (a, _) = planes();
        assert!(std::ptr::eq(
            c3_bspline_base(&a).unwrap(),
            std::ptr::from_ref(&a)
        ));

        let once = offset(&k, 0.5, &a);
        let inner = c3_bspline_base(&once).unwrap();
        assert!(matches!(inner.kind(), SurfaceKind::BSpline(_)));

        let twice = offset(&k, 0.5, &once);
        let e = c3_bspline_base(&twice).unwrap_err();
        assert!(matches!(
            e,
            SolidError::Upstream {
                code: crate::error::SM_ERR_NULL_POINTER,
                ..
            }
        ));
    }

    #[test]
    fn analytic_through_an_offset_base() {
        let k = Kernel::default();
        let (a, b) = planes();
        let mut fs = solver(&k, &a, &b);
        let (pa, pb) = analytic_planes();
        let mut st = state(line(), [offset(&k, 0.5, &pa), pb]);
        let r = with_cx(&k, |cx| c3_try_analytic(&mut fs, cx, &mut st));
        assert!(matches!(r, Ok(false)), "{r:?}");

        let (pa, pb) = analytic_planes();
        let mut st = state(line(), [pa, offset(&k, 0.5, &offset(&k, 0.5, &pb))]);
        let r = with_cx(&k, |cx| c3_try_analytic(&mut fs, cx, &mut st));
        assert!(matches!(
            r,
            Err(SolidError::Upstream {
                code: crate::error::SM_ERR_NULL_POINTER,
                ..
            })
        ));
    }

    #[test]
    fn trace_setup_extends_the_natural_domains() {
        let k = Kernel::default();
        let (a, b) = planes();
        let mut fs = solver(&k, &a, &b);
        let mut st = state(line(), [a.clone(), b.clone()]);
        let surfaces = with_cx(&k, |cx| c3_trace_setup(&mut fs, cx, &mut st)).unwrap();

        let mut d1 = a.natural_uv_domain_if_built();
        let mut d2 = b.natural_uv_domain_if_built();
        fs.solver.setup_offset_extension(&k, &a, &mut d1).unwrap();
        fs.solver.setup_offset_extension(&k, &b, &mut d2).unwrap();
        assert_eq!(st.trace.domains[0].min, d1.min);
        assert_eq!(st.trace.domains[0].max, d1.max);
        assert_eq!(st.trace.domains[1].min, d2.min);
        assert_eq!(st.trace.domains[1].max, d2.max);
        // the unit square grown by half its size at both ends
        assert_eq!(d1.min, Point2::new(-0.5, -0.5));
        assert_eq!(d1.max, Point2::new(1.5, 1.5));

        for (s, d) in surfaces.iter().zip([0.25, -0.25]) {
            let SurfaceKind::OffsetSurface(o) = s.kind() else {
                panic!("not an offset surface");
            };
            assert_eq!(o.offset_distance(), d);
        }
    }

    /// Upstream's intersector evaluates the solver's own offset surfaces: the trace
    /// copies follow every offset change the solver makes through the adapter.
    #[test]
    fn trace_copies_follow_the_solver_offsets() {
        use crate::blend::fillet_intersector::FilletSolverOps as _;

        let k = Kernel::default();
        let (a, b) = planes();
        let mut fs = solver(&k, &a, &b);
        let mut st = state(line(), [a, b]);
        let dist = |s: &Surface| match s.kind() {
            SurfaceKind::OffsetSurface(o) => o.offset_distance(),
            _ => panic!("not an offset surface"),
        };
        with_cx(&k, |cx| {
            let surfaces = c3_trace_setup(&mut fs, cx, &mut st).unwrap();
            let mut solver = IntersectorSolver {
                ops: &mut fs,
                trace: [&surfaces[0], &surfaces[1]],
                brep: None,
            };
            solver.set_surfaces_to_zero_offset().unwrap();
            assert_eq!(dist(&surfaces[0]), 0.0);
            assert_eq!(dist(&surfaces[1]), 0.0);
            solver.reload_surface_offsets().unwrap();
            assert_eq!(dist(&surfaces[0]), 0.25);
            assert_eq!(dist(&surfaces[1]), -0.25);
        });
    }

    fn extension_distance(k: &Kernel, curve: Curve, before: bool, after: bool) -> f64 {
        let (a, b) = planes();
        let mut fs = solver(k, &a, &b);
        fs.solver.extend_before = before;
        fs.solver.extend_after = after;
        let mut st = state(curve, [a, b]);
        with_cx(k, |cx| {
            let surfaces = c3_trace_setup(&mut fs, cx, &mut st).unwrap();
            let fi = c3_make_intersector(&mut fs, cx.k, &st, &surfaces).unwrap();
            fi.base.base.extension_distance
        })
    }

    #[test]
    fn extension_distance_only_for_an_extended_arc() {
        let k = Kernel::default();
        let default = extension_distance(&k, line(), false, false);
        assert_eq!(extension_distance(&k, line(), true, true), default);
        assert_eq!(extension_distance(&k, arc(&k), false, false), default);
        assert_eq!(extension_distance(&k, arc(&k), true, false), 1.25);
        assert_eq!(extension_distance(&k, arc(&k), false, true), 1.25);
    }

    fn traced(last: IntersectionPointType, before: bool, after: bool) -> (CalcState, Vec<TsRef>) {
        let k = Kernel::default();
        let (a, b) = planes();
        let mut fs = solver(&k, &a, &b);
        fs.solver.extend_before = before;
        fs.solver.extend_after = after;
        let mut st = state(line(), [a, b]);
        let mut p = TsectPnt::new();
        p.point_type = IntersectionPointType::Crossing;
        let r0 = st.store.alloc(p.clone());
        let r1 = st.store.alloc(p.clone());
        p.point_type = last;
        let r2 = st.store.alloc(p);
        st.tsect_pnts = vec![r0, r1, r2];
        with_cx(&k, |cx| c3_trace_start(&mut fs, cx, &mut st)).unwrap();
        (st, vec![r0, r1, r2])
    }

    #[test]
    fn trace_start_keeps_the_order_without_a_tangent_end() {
        let (st, refs) = traced(IntersectionPointType::Crossing, true, false);
        assert_eq!(st.tsect_pnts, refs);
        assert!(!st.trace.reverse_tracing);
        assert_eq!(st.trace.dir, st.pv_start[1]);
        assert_eq!(st.trace.approx_tol_3d, 0.001);
        assert_eq!(st.trace.angle_tol, 0.5);
        assert!(st.trace.extend_before);
        assert!(!st.trace.extend_after);
    }

    #[test]
    fn trace_start_reverses_at_a_tangent_end() {
        let (st, mut refs) = traced(IntersectionPointType::TangentPoint, false, true);
        refs.reverse();
        assert_eq!(st.tsect_pnts, refs);
        assert!(st.trace.reverse_tracing);
        assert_eq!(st.trace.dir, Vec3::new(0.0, -2.0, 0.0));
        // the locals copy the solver's flags (C4 swaps them per pass)
        assert!(!st.trace.extend_before);
        assert!(st.trace.extend_after);
    }

    #[test]
    fn trace_start_with_no_points() {
        let k = Kernel::default();
        let (a, b) = planes();
        let mut fs = solver(&k, &a, &b);
        let mut st = state(line(), [a, b]);
        with_cx(&k, |cx| c3_trace_start(&mut fs, cx, &mut st)).unwrap();
        assert!(st.tsect_pnts.is_empty());
        assert!(!st.trace.reverse_tracing);
        assert_eq!(st.trace.dir, st.pv_start[1]);
    }
}

// P4-CHUNK(C3) END

// =====================================================================================
// P4-CHUNK(C4) BEGIN: the trace loop, SmFilletSolver.cpp:2093-2467
// (chunk-local imports go inside the functions: never edit the shared `use` block)
// =====================================================================================

/// :2093-2467, `while (!bFoundAllPoints)`: the ten-pass cap (`SetStatus`,
/// `RecordFilletError`, `SM_ERR`); the reversed extension flags (every pass);
/// `DoPointIntersection` on a snapshot of the points (written back; a failure only
/// sets the status); missed points (`curve_parameter < 0`); the boundary branch; the
/// cusp branch (centre-line end, `GlobalPointSolve`, twenty skips with
/// `binormal_guess_point(.., half = false)`, `sm_FindFilletTsectPnt`,
/// `IsPointOnCurve`, insert in front).
fn c4_trace(
    ops: &mut dyn FilletSolverOps,
    cx: &mut Cx<'_>,
    st: &mut CalcState,
    fi: &mut TraceIntersector<'_>,
) -> Result<()> {
    use super::p1::FilStatus;

    // ULONG lMaxLoopCount = 10; ULONG lLoopCount = 0; (:2081-2082)
    let max_loop_count: u32 = 10;
    let mut loop_count: u32 = 0;

    // while (!bFoundAllPoints): the flag starts FALSE, and the only assignment that
    // can leave it TRUE is followed by the `break` at :2278.
    loop {
        loop_count += 1;
        if loop_count > max_loop_count {
            // Either too many self-intersections or other
            // suspicious situations might have happened
            ops.solver_mut().set_status(FilStatus::SurfTracingFailure);
            crate::error::ignore_status(record_fillet_error(
                ops,
                cx,
                &st.store,
                &st.tsect_pnts,
                FilletErrorType::BadInt,
                C4_RADIUS_TOO_LARGE,
            ))?;
            // SER_MSG(SM_ERR, "CalcFilletGeom(): Ran into too many self-intersections ..")
            return Err(SolidError::sm_err(OP));
        }

        // Determine the tracing direction. Every pass: this overwrites the recovery
        // value of bExtendBefore set below, as upstream does.
        if st.trace.reverse_tracing {
            st.trace.extend_before = ops.solver().extend_after;
            st.trace.extend_after = ops.solver().extend_before;
        }

        // Do the offset-surface intersection (FlushCurve fills the solver's last
        // fillet geometry). Upstream writes through the point pointers in place:
        // run on a snapshot and write it back whatever the outcome.
        let args = PointIntersectionArgs {
            extend_before: st.trace.extend_before,
            extend_after: st.trace.extend_after,
            start_dir: Some(st.trace.dir),
            approx_tol_3d: st.trace.approx_tol_3d,
            ang_tol_rad: st.trace.angle_tol,
        };
        let mut points = st.store.snapshot(cx.fillets, &st.tsect_pnts)?;
        let stat = do_point_intersection(fi, ops, cx, &mut points, args);
        if let Err(e) = stat.as_ref() {
            if e.is_unported() {
                return stat;
            }
        }
        st.store.write_back(cx.fillets, &st.tsect_pnts, &points)?;

        if stat.is_err() && ops.solver().status == FilStatus::Unprocessed {
            ops.solver_mut().set_status(FilStatus::SurfTracingFailure);
        }

        // Check all points, after the first one, to make sure
        // that they were properly processed by DoPointIntersection().
        let Some(ii) = c4_first_missed_point(&st.store, cx.fillets, &st.tsect_pnts)? else {
            // quit with success when all FoundPoints have been processed
            break;
        };
        // bFoundAllPoints = FALSE: another iteration. On it, trace from that start
        // point in both directions (RanIntoACusp fix).
        st.trace.extend_before = true;
        // remove the current 1st start point - we've tried walking from it
        // (sTsectPnts.RemoveAt(0, ii)).
        st.tsect_pnts.drain(..ii);

        if fi.is_curr_fillet_touch_boundary() {
            // traced the fillet to a surface natural boundary: assume there were no
            // self intersections
            fi.set_self_intersect(false);

            // The newly created fillet touches the boundary edge of a base surface
            // (typically a G1-closed base surface). Trace again to create another
            // fillet.
            if ops.solver().extend_after {
                // Something strange happening, return ERROR
                return Err(SolidError::sm_err(OP));
            }

            // no need to trace next start point in both directions
            st.trace.extend_before = false;
        } else {
            // The newly created fillet did not trace to surface end - probably a
            // surface cusp: step past where the solver stopped, insert a new TsectPnt
            // and re-trace.
            c4_cusp_recovery(ops, cx, st, fi)?;
        }
    }

    Ok(())
}

/// The `RecordFilletError` message at :2103 and :2332.
const C4_RADIUS_TOO_LARGE: &str = "Offset surface intersection failed, probably radius too large";

/// :2242-2275: the index `ii >= 1` of the first point `DoPointIntersection` missed
/// (`m_dCurveParameter < 0.0`), if any. The first point is never checked.
fn c4_first_missed_point(
    store: &TsectStore,
    fillets: &FilletBrep,
    refs: &[TsRef],
) -> Result<Option<usize>> {
    for (ii, r) in refs.iter().enumerate().skip(1) {
        if store.get(fillets, *r)?.curve_parameter < 0.0 {
            return Ok(Some(ii));
        }
    }
    Ok(None)
}

/// :2384, `dNextT = dT + dSkipSize*(1.0+0.1*ii)`.
fn c4_next_t(t: f64, skip_size: f64, ii: u32) -> f64 {
    t + skip_size * (1.0 + 0.1 * f64::from(ii))
}

/// :2306-2466, the self-intersecting (cusp) branch of the trace loop: the end of the
/// last centre line dropped onto the filleted edge, then up to twenty growing skips
/// past it, each a guess point (`binormal_guess_point(.., half = false)`), `sDir`
/// set to the edge-use tangent, and `sm_FindFilletTsectPnt` into one point allocated
/// for the branch; the first success is put in front of `sTsectPnts` unless it lies
/// on a centre line already traced.
fn c4_cusp_recovery(
    ops: &mut dyn FilletSolverOps,
    cx: &mut Cx<'_>,
    st: &mut CalcState,
    fi: &mut TraceIntersector<'_>,
) -> Result<()> {
    use super::p1::FilStatus;
    use crate::curve::curve::p1::GlobalPointSolveOptions;
    use crate::geom::global_solver::{SolutionRequested, SolverOperation};

    fi.set_self_intersect(true);

    // pCenterLine = GetLastFilletGeom()->GetCenterLineCurve()
    let last = ops.solver().last_fillet_geom()?;
    let end_pnt = match cx.fillets.geom(last)?.center_line_geometry() {
        // get centerline's endPoint - it's located very near a surface cusp
        Some(center_line) => {
            Some(center_line.evaluate_point(cx.k, center_line.natural_interval().max())?)
        }
        None => None,
    };
    let Some(end_pnt) = end_pnt else {
        // remember when none of fillet centerline curve has been stepped
        ops.solver_mut().set_status(FilStatus::SurfTracingFailure);
        crate::error::ignore_status(record_fillet_error(
            ops,
            cx,
            &st.store,
            &st.tsect_pnts,
            FilletErrorType::BadInt,
            C4_RADIUS_TOO_LARGE,
        ))?;
        return Err(SolidError::null_pointer(OP));
    };

    // find closest filletEdge point to centerline end
    let solutions = st.edge_curve.global_point_solve::<NoObject>(
        cx.k,
        &st.ivl,
        SolverOperation::Minimize,
        end_pnt,
        st.trace.approx_tol_3d,
        SolutionRequested::Single,
        &GlobalPointSolveOptions::default(),
    )?;

    // expect 1 solution (it might be before or after the cusp section)
    if solutions.len() != 1 {
        return Err(SolidError::sm_err(OP));
    }

    // dT = param at which last centerline step ran into trouble (probable cusp)
    let t = solutions
        .get(0)
        .ok_or_else(|| SolidError::sm_err(OP))?
        .start
        .param(0)?;

    // pick initial skip size
    let mut skip_size = 0.1 * st.ivl.length();
    if st.trace.reverse_tracing {
        skip_size *= -1.0;
    }

    // an additional TsectPnt which may or may not be added to sTsectPnts (always
    // kept in sDeletePnts)
    let new_pnt = st.store.alloc(TsectPnt::new());

    // for a sequence of bigger steps - try tracing the curve after the cusp region
    for ii in 0..20_u32 {
        let next_t = c4_next_t(t, skip_size, ii);
        if !st.ivl.contains_value(next_t, 0.0) {
            continue;
        }

        // Cross-sectional guess point from the surface offset (no 0.5 here, unlike
        // the G1-knot points)
        let [base1, base2] = &st.base_surfaces;
        let (pt, tangent) = binormal_guess_point(ops, cx, base1, next_t, false)?;

        // Adjust tracing direction
        st.trace.dir = tangent;

        // Solve the point on the cross-section plane
        let found = find_fillet_tsect_pnt(
            ops,
            cx.k,
            cx.target,
            base1,
            base2,
            pt,
            tangent,
            st.store.get_mut(cx.fillets, new_pnt)?,
        );
        match found {
            Ok(()) => {}
            Err(e) if e.is_unported() => return Err(e),
            Err(_) => continue,
        }

        // gather all fillet centerlines made so far
        let mut curves: Vec<Curve> = Vec::new();
        for id in fillet_geom_ids(ops) {
            if let Some(center_line) = cx.fillets.geom(id)?.center_line_geometry() {
                curves.push(center_line.clone());
            }
        }

        // When new point is not on an existing centerline
        if !fi.base.base.is_point_on_curve(cx.k, pt, &curves, None)? {
            // use it to start the next segment of the fillet trace
            st.tsect_pnts.insert(0, new_pnt);
        }

        // break from sequence of bigger steps until we are past the cusp
        break;
    }

    Ok(())
}

#[cfg(test)]
mod tests_c4 {
    use super::*;
    use crate::curve::line::Line;
    use crate::nurbs::b_spline_surface::BSplineSurface;

    fn bilinear(p: [[f64; 3]; 4]) -> Surface {
        let [a, b, c, d] = p.map(Vec3::from);
        Surface::from(BSplineSurface::create_bilinear_surface(a, b, c, d).unwrap())
    }

    fn surfaces() -> [Surface; 2] {
        [
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
        ]
    }

    fn state(params: &[f64]) -> CalcState {
        let line = Line::from_points(Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0), 3);
        let pv = [Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0)];
        let mut st = CalcState::new(
            Curve::from(line),
            Extent1d::from_min_max(0.0, 1.0),
            pv,
            pv,
            surfaces(),
            FilletGeomId(0),
            false,
        );
        for &t in params {
            let mut p = TsectPnt::new();
            p.curve_parameter = t;
            let r = st.store.alloc(p);
            st.tsect_pnts.push(r);
        }
        st
    }

    #[test]
    fn missed_point_skips_the_first_and_finds_the_first_negative() {
        let fillets = FilletBrep::new();
        let st = state(&[-1.0, 0.5, -BIG, 2.0, -3.0]);
        assert_eq!(
            c4_first_missed_point(&st.store, &fillets, &st.tsect_pnts).unwrap(),
            Some(2)
        );
        let st = state(&[-1.0, 0.0, 2.0]);
        assert_eq!(
            c4_first_missed_point(&st.store, &fillets, &st.tsect_pnts).unwrap(),
            None
        );
        let st = state(&[]);
        assert_eq!(
            c4_first_missed_point(&st.store, &fillets, &st.tsect_pnts).unwrap(),
            None
        );
        // a dangling handle is reported, not skipped
        assert!(
            c4_first_missed_point(&st.store, &fillets, &[TsRef::Local(0), TsRef::Local(9)])
                .is_err()
        );
    }

    const BIG: f64 = crate::math::BIG_DOUBLE;

    #[test]
    fn next_t_keeps_the_upstream_operation_order() {
        let (t, skip) = (0.3, -0.07);
        for ii in 0..20_u32 {
            let expected = t + skip * (1.0 + 0.1 * f64::from(ii));
            assert_eq!(c4_next_t(t, skip, ii).to_bits(), expected.to_bits());
        }
        assert_eq!(c4_next_t(1.0, 0.5, 0), 1.5);
    }

    #[test]
    fn trace_swaps_reversed_flags_then_defers_in_cusp_recovery() {
        use crate::blend::fillet_geom::p1::FilletGeom;

        let k = Kernel::default();
        let mut solver = FilletSolver::new(1.0e-5, 0.01, 0.01);
        solver.extend_before = true;
        solver.extend_after = false;
        let mut target = Brep::new();
        let mut fillets = FilletBrep::new();
        let g = fillets.add_geom(FilletGeom::new(&k, None, FilletGeomType::Default).unwrap());
        solver.fillet_geoms.push(g);
        let mut cx = Cx {
            k: &k,
            target: &mut target,
            fillets: &mut fillets,
            solvers: GeometrySolvers::Indexed(&[]),
            error_info: None,
        };
        let mut st = state(&[0.0, -1.0, 1.0]);
        st.trace.reverse_tracing = true;
        st.trace.extend_before = true;
        st.trace.extend_after = false;
        let s = surfaces();
        let d = Extent2d::from_bounds(-1.0, -1.0, 2.0, 2.0);
        let mut fi: TraceIntersector<'_> =
            FilletIntersector::new(&k, &s[0], &d, &s[1], &d, None).unwrap();

        // DoPointIntersection fails on these unprepared points (a tracing failure, not
        // a return), point 1 stays missed, and the cusp recovery finds no centre line
        // on the last geometry: RecordFilletError's status is ignored, then
        // SM_ERR_NULL_POINTER.
        let e = c4_trace(&mut solver, &mut cx, &mut st, &mut fi).unwrap_err();
        assert!(
            matches!(
                e,
                SolidError::Upstream {
                    code: crate::error::SM_ERR_NULL_POINTER,
                    ..
                }
            ),
            "{e:?}"
        );
        // bExtendBefore = m_bExtendAfter, then TRUE for the missed point;
        // bExtendAfter = m_bExtendBefore
        assert!(st.trace.extend_before);
        assert!(st.trace.extend_after);
        assert_eq!(
            solver.status,
            super::super::p1::FilStatus::SurfTracingFailure
        );
        // RemoveAt(0, 1)
        assert_eq!(st.tsect_pnts.len(), 2);
        assert!(fi.self_intersect());
    }
}

// P4-CHUNK(C4) END

// =====================================================================================
// P4-CHUNK(C5) BEGIN: post-processing, SmFilletSolver.cpp:2470-2671
// (chunk-local imports go inside the functions: never edit the shared `use` block)
// =====================================================================================

/// :2470-2671: with one geometry and the split flag, `TopologySplit`, the rail and UV
/// curve copies, the `DropPoint` ladder, trims, the new start vertex and its rail
/// vertex uses, and the fillet surface split at the rail parameter; with several
/// geometries, `CalcCrossSection` of their cross-section edges and
/// `CalcBlendingGeom` of the blends; after a reversed trace, the rails' 3D and UV
/// curves, the centre line and the fillet surface reversed.
fn c5_post_process(
    ops: &mut dyn FilletSolverOps,
    cx: &mut Cx<'_>,
    st: &mut CalcState,
) -> Result<()> {
    // when just one filletGeom was created (no selfIntersections), see if we are
    // required to split FG
    if ops.solver().fillet_geoms.len() == 1 && st.split_original_fillet {
        c5_split_original_fillet(ops, cx, st)?;
    }

    // when more than one filletGeom was created (self intersections; also right
    // after the split above, as upstream)
    if ops.solver().fillet_geoms.len() > 1 {
        c5_self_intersection_patches(ops, cx)?;
    }

    // when trace was reversed (due to endPoint degeneracies) - restore output trace
    // directions
    if st.trace.reverse_tracing {
        c5_undo_reversal(ops, cx)?;
    }

    // all done
    Ok(())
}

/// :2473-2587: `TopologySplit` of geometry 0, the new geometry's rails (copies of
/// the old rails and their mate UV trim curves, trimmed at the split parameter), its
/// start vertex, and the fillet surface split in two at the rail-0 parameter.
fn c5_split_original_fillet(
    ops: &mut dyn FilletSolverOps,
    cx: &mut Cx<'_>,
    st: &CalcState,
) -> Result<()> {
    use crate::nurbs::b_spline_surface::SurfParamType;

    let geom0 = st.geom0;
    let new_geom = topology_split(&*ops, cx, geom0, FilletGeomType::Default)?;
    add_fillet_geom(ops, new_geom);
    let mut split_at_param = 0.0;

    // Setup rail curves for new FilletGeom
    for mm in 0..2usize {
        split_at_param = c5_split_rail(cx, st, new_geom, mm, split_at_param)?;
    }

    // Split fillet into two
    let (sur_l, sur_r) = {
        let fillet_surface = cx
            .fillets
            .geom(geom0)?
            .fillet_surface(None)?
            .ok_or_else(|| SolidError::null_pointer(OP))?;
        fillet_surface.split_at(cx.k, split_at_param, SurfParamType::U)?
    };
    // delete pFilletSurface; m_pFilletSurface = SM_CAST_PTR(SmBSplineSurface, ..)
    cx.fillets
        .geom_mut(geom0)?
        .set_fillet_surface(c5_bspline_surface_cast(sur_l));
    cx.fillets
        .geom_mut(new_geom)?
        .set_fillet_surface(c5_bspline_surface_cast(sur_r));
    Ok(())
}

/// `SM_CAST_PTR(SmBSplineSurface, pSur)` of an owned surface: the surface when its kind
/// derives from `SmBSplineSurface`, else `None` (the half is then lost, as upstream).
fn c5_bspline_surface_cast(surface: Surface) -> Option<Surface> {
    is_bspline_surface(&surface).then_some(surface)
}

/// One pass `mm` of :2481-2571: copies rail `mm` of geometry 0 and its mate UV trim
/// curve onto the new geometry's rail, finds the split parameter on rail 0 (the
/// `DropPoint` ladder; rail 1 reuses rail 0's parameter), trims both rails, and moves
/// the new rail's start vertex to the split point. Returns `dSplitAtParam`.
fn c5_split_rail(
    cx: &mut Cx<'_>,
    st: &CalcState,
    new_geom: FilletGeomId,
    mm: usize,
    split_at_param: f64,
) -> Result<f64> {
    use crate::blend::fillet_geom::p1::{FilStatus, FilletEdgeType};
    use crate::error::ignore_status;
    use crate::intersect::surface_intersector::p1::SrfIndex;
    use crate::nurbs::b_spline_curve_n_lib::TrimMode;
    use crate::topology::{edge as edge_unit, vertex as vertex_unit, vertexuse as vu_unit};

    let mut split_at_param = split_at_param;
    let brep = cx.fillets.brep();
    let rail = cx
        .fillets
        .geom(st.geom0)?
        .rail(mm)
        .ok_or_else(|| invalid("split: geometry 0 has no rail"))?;
    let rail_curve = edge_unit::curve(brep, rail)?.ok_or_else(|| SolidError::null_pointer(OP))?;
    let mut trim_ivl = rail_curve.natural_interval();
    let mut new_trim_ivl = trim_ivl;
    let prim_eu = eu::primary_edgeuse(brep, rail)?
        .ok_or_else(|| invalid("split: rail without primary edge use"))?;
    let mate_eu =
        eu::mate(brep, prim_eu)?.ok_or_else(|| invalid("split: rail edge use without mate"))?;
    let uv_curve =
        eu::uv_trim_curve_pointer(brep, mate_eu)?.ok_or_else(|| SolidError::null_pointer(OP))?;
    let new_rail = cx
        .fillets
        .geom(new_geom)?
        .rail(mm)
        .ok_or_else(|| invalid("split: new geometry has no rail"))?;
    let new_prim_eu = eu::primary_edgeuse(brep, new_rail)?
        .ok_or_else(|| invalid("split: new rail without primary edge use"))?;
    let new_mate_eu = eu::mate(brep, new_prim_eu)?;

    // Copy the original rail curve and UVTrimCurve
    let new_rail_curve = rail_curve.copy(cx.k)?;
    let new_rail_uv_curve = uv_curve.copy(cx.k)?;

    // Set NewRail->Curve = RailCurveCopy (TRUE = delete preExisting Edge->Curve;
    // side effect: the new rail's edge uses lose their UV trim curves)
    crate::blend::fillet_geom::p1::set_rail_curve(cx.fillets.brep_mut(), new_rail, new_rail_curve)?;
    // pNewRailCurve->SetOwner(pNewRail): owner identities are not modelled here

    // Set NewRail->UVTrimCurve = Rail->UVTrimCurveCopy
    let new_mate_eu =
        new_mate_eu.ok_or_else(|| invalid("split: new rail edge use without mate"))?;
    cx.fillets
        .set_edgeuse_uv_curve(new_mate_eu, Some(new_rail_uv_curve), true, true)?;

    // SM_USE_OLDTOL
    let rail_tol = edge_unit::tolerance(cx.fillets.brep(), rail)?;
    edge_unit::set_tolerance(cx.fillets.brep_mut(), new_rail, rail_tol, true, true)?;

    let split_ref = st
        .split_tsect_pnt
        .ok_or_else(|| invalid("split: pSplitTsectPnt is NULL"))?;
    let split_pnt = st.store.get(cx.fillets, split_ref)?.clone();
    let uv = split_pnt.uv_pos(if mm == 0 {
        SrfIndex::First
    } else {
        SrfIndex::Second
    });
    let pnt = if mm == 0 {
        let pnt = st.base_surfaces[0].evaluate_point(cx.k, uv)?;
        // Find the split-at parameter (i.e. -u) of the fillet
        let rail_curve = edge_unit::curve(cx.fillets.brep(), rail)?
            .ok_or_else(|| SolidError::null_pointer(OP))?;
        split_at_param = c5_drop_split_param(
            cx.k,
            rail_curve,
            &trim_ivl,
            pnt,
            st.trace.approx_tol_3d,
            split_at_param,
        )?;
        pnt
    } else {
        st.base_surfaces[1].evaluate_point(cx.k, uv)?
    };

    ignore_status(trim_ivl.set_min_max(trim_ivl.min(), split_at_param))?;
    cx.fillets
        .brep_mut()
        .edge_mut(rail)?
        .curve
        .as_mut()
        .ok_or_else(|| SolidError::null_pointer(OP))?
        .trim(cx.k, &mut trim_ivl, TrimMode::Notify)?; // may snap sIvl by tol to existing knots
    ignore_status(new_trim_ivl.set_min_max(split_at_param, new_trim_ivl.max()))?;
    cx.fillets
        .brep_mut()
        .edge_mut(new_rail)?
        .curve
        .as_mut()
        .ok_or_else(|| SolidError::null_pointer(OP))?
        .trim(cx.k, &mut new_trim_ivl, TrimMode::Notify)?; // may snap sIvl by tol to existing knots

    let new_vert = vertex_unit::edge_start_vertex(cx.fillets.brep(), new_rail)?
        .ok_or_else(|| invalid("split: new rail without start vertex"))?;
    cx.fillets.set_vertex_point(new_vert, pnt)?;
    cx.fillets
        .fillet_vertex_mut(new_vert)?
        .set_status(FilStatus::Processed);
    let vus = vertex_unit::vertexuses(cx.fillets.brep(), new_vert)?;
    for vu in vus {
        let b = cx.fillets.brep();
        let vu_eu = vu_unit::edgeuse(b, vu)?
            .ok_or_else(|| invalid("split: vertex use without edge use"))?;
        let e = eu::edge(b, vu_eu)?.ok_or_else(|| invalid("split: edge use without edge"))?;
        if cx.fillets.fillet_edge(e)?.fillet_edge_type() != FilletEdgeType::Rail {
            continue;
        }
        cx.fillets
            .fillet_vertexuse_mut(vu)?
            .set_tsect_pnt(&split_pnt);
    }
    Ok(split_at_param)
}

/// :2520-2551: `pRailCurve->DropPoint(sTrimIvl, sPnt, NULL, dDroppingTol, NULL, ..)`
/// (default `SM_SO_MINIMIZE`) with the dropping tolerance `tol`, then `10*tol`, then
/// `20*tol` (the double loop counter runs 10, 20, 30; the tolerance is updated after
/// each drop, so `30*tol` is never used). Returns the last dropped parameter
/// (`dSplitAtParam`, which every drop writes); `SM_ERR` when no drop succeeds.
fn c5_drop_split_param(
    k: &Kernel,
    rail_curve: &Curve,
    trim_ivl: &Extent1d,
    pnt: Point3,
    approx_tol_3d: f64,
    split_at_param: f64,
) -> Result<f64> {
    use crate::geom::global_solver::SolverOperation;

    let mut split_at_param = split_at_param;
    let mut dropping_tol = approx_tol_3d;
    let mut success = false;
    let mut scale = 10.0;
    while scale < 39.0 {
        let dropped = rail_curve.drop_point(
            k,
            trim_ivl,
            pnt,
            None,
            dropping_tol,
            None,
            SolverOperation::Minimize,
        )?;
        success = dropped.success;
        split_at_param = dropped.parameter;
        dropping_tol = approx_tol_3d * scale;
        if success {
            break;
        }
        scale += 10.0;
    }
    if !success {
        return Err(SolidError::sm_err(OP));
    }
    Ok(split_at_param)
}

/// :2590-2627: `CalcCrossSection` of every `SM_FE_CROSS_SECTION` edge of the
/// `SM_FG_DEFAULT` geometries, then `CalcBlendingGeom` of the `SM_FG_BLENDS` ones.
fn c5_self_intersection_patches(ops: &mut dyn FilletSolverOps, cx: &mut Cx<'_>) -> Result<()> {
    use crate::blend::fillet_geom::p1::FilletEdgeType;

    // Self-intersection cases
    for g in fillet_geom_ids(ops) {
        if cx.fillets.geom(g)?.fillet_geom_type() == FilletGeomType::Default {
            // Calculate fillet edges of type SM_FE_CROSS_SECTION
            let mut fg_edges = Vec::new();
            cx.fillets.geom(g)?.fillet_edges(&mut fg_edges);
            for e in fg_edges {
                if cx.fillets.fillet_edge(e)?.fillet_edge_type() == FilletEdgeType::CrossSection {
                    calc_cross_section(ops, cx, e)?;
                }
            }
        }
    }

    for g in fillet_geom_ids(ops) {
        if cx.fillets.geom(g)?.fillet_geom_type() == FilletGeomType::Blends {
            // Calculate the surface for blending patch
            calc_blending_geom(&*ops, cx, g)?;
        }
    }
    Ok(())
}

/// :2630-2662: reverses every geometry's rails (3D curve, and the mate UV trim curve
/// over the 3D curve's new interval), centre line and fillet surface (`Reverse(U)`).
fn c5_undo_reversal(ops: &mut dyn FilletSolverOps, cx: &mut Cx<'_>) -> Result<()> {
    use crate::nurbs::b_spline_surface::SurfParamType;
    use crate::topology::edge as edge_unit;

    // Reverse the fillet surface & rails
    for g in fillet_geom_ids(ops) {
        for kk in 0..2usize {
            let rail = cx
                .fillets
                .geom(g)?
                .rail(kk)
                .ok_or_else(|| invalid("reverse: geometry has no rail"))?;
            let b = cx.fillets.brep();
            if edge_unit::curve(b, rail)?
                .and_then(bspline_curve_cast)
                .is_none()
            {
                return Err(SolidError::null_pointer(OP));
            }
            let prim_eu = eu::primary_edgeuse(b, rail)?
                .ok_or_else(|| invalid("reverse: rail without primary edge use"))?;
            let mate_eu = eu::mate(b, prim_eu)?
                .ok_or_else(|| invalid("reverse: rail edge use without mate"))?;
            // pMateEU->GetOrCreateUVTrimCurve(pUVCurve) (defaults): status ignored; the
            // curve attached to the edge use or its mate is reversed in place
            let _ = edgeuse_p3::get_or_create_uv_trim_curve(
                cx.k,
                cx.fillets.brep_mut(),
                mate_eu,
                false,
                false,
                true,
            )?
            .status;
            let (uv_curve, uv_owner) = eu::uv_trim_curve(cx.fillets.brep(), mate_eu)?;
            let (Some(_), Some(uv_owner)) = (uv_curve, uv_owner) else {
                return Err(SolidError::null_pointer(OP));
            };
            let k = cx.k;
            let b = cx.fillets.brep_mut();
            let curve_3d = b
                .edge_mut(rail)?
                .curve
                .as_mut()
                .ok_or_else(|| SolidError::null_pointer(OP))?;
            let mut ivl1 = curve_3d.natural_interval();
            ivl1 = curve_3d.reverse_parameterization(k, &ivl1)?;
            let uv_curve = b
                .edgeuse_mut(uv_owner)?
                .uv_trim_curve
                .as_mut()
                .ok_or_else(|| SolidError::null_pointer(OP))?;
            uv_curve.reverse_parameterization(k, &ivl1)?;
        }

        let k = cx.k;
        let geom = cx.fillets.geom_mut(g)?;
        if geom.center_line_curve.is_none() || !geom.has_fillet_surface() {
            return Err(SolidError::null_pointer(OP));
        }
        if let Some(center_line) = geom.center_line_curve.as_mut() {
            let ivl2 = center_line.natural_interval();
            center_line.reverse_parameterization(k, &ivl2)?;
        }
        if let Some(fillet_surf) = geom.fillet_surface_mut(None)? {
            fillet_surf.reverse(k, SurfParamType::U)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests_c5 {
    use super::*;
    use crate::blend::fillet_geom::p1::{
        FilletEdgeType, FilletEdgeuse, FilletGeom, FilletVertex, FilletVertexuse,
    };
    use crate::error::SM_ERR_NULL_POINTER;
    use crate::nurbs::b_spline_surface::BSplineSurface;
    use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
    use crate::nurbs::crv_basic::Curve as NurbCurve;
    use crate::nurbs::math::{CPoint, NOW};
    use crate::nurbs::srf_basic::Surface as NurbSurface;
    use crate::topology::{Edgeuse, Vertexuse, VertexuseOwner};

    fn line_bspline(a: Point3, b: Point3) -> BSplineCurve {
        let nurb = NurbCurve::new(
            vec![
                CPoint::new(a.x, a.y, a.z, NOW),
                CPoint::new(b.x, b.y, b.z, NOW),
            ],
            1,
            vec![0.0, 0.0, 1.0, 1.0],
        )
        .unwrap();
        BSplineCurve::from_nurb(nurb, 3, false)
    }

    fn line(a: Point3, b: Point3) -> Curve {
        Curve::from(line_bspline(a, b))
    }

    /// A cubic-by-linear strip over `[0,1] x [0,1]`, x from 0 to 3, y from 0 to 2.
    fn strip() -> Surface {
        let net = (0..4)
            .map(|i| {
                let x = f64::from(i);
                vec![CPoint::new(x, 0.0, 0.0, NOW), CPoint::new(x, 2.0, 0.0, NOW)]
            })
            .collect();
        let nurb = NurbSurface::new(
            net,
            3,
            1,
            vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0],
            vec![0.0, 0.0, 1.0, 1.0],
        )
        .unwrap();
        Surface::from(BSplineSurface::from_nurb(nurb, false))
    }

    /// A rail from `(0,y,0)` to `(1,y,0)` with two edge uses (same, opposite) and
    /// their vertex uses; the mate of the primary edge use holds a UV trim curve.
    /// Returns the rail and that mate.
    fn add_rail(k: &Kernel, fb: &mut FilletBrep, y: f64) -> (EdgeId, EdgeuseId) {
        let p0 = Point3::new(0.0, y, 0.0);
        let p1 = Point3::new(1.0, y, 0.0);
        let v0 = fb.brep_mut().add_vertex(p0).unwrap();
        let v1 = fb.brep_mut().add_vertex(p1).unwrap();
        fb.insert_fillet_vertex(v0, FilletVertex::new(k, None));
        fb.insert_fillet_vertex(v1, FilletVertex::new(k, None));
        let e = fb.new_fillet_edge(v0, v1, None).unwrap();
        fb.fillet_edge_mut(e)
            .unwrap()
            .set_fillet_edge_type(FilletEdgeType::Rail);
        let mut uses = Vec::new();
        for (v, o) in [(v0, OrientType::Same), (v1, OrientType::Opposite)] {
            let b = fb.brep_mut();
            let e_u = b.add_edgeuse_record(Edgeuse::default()).unwrap();
            let vu = b.add_vertexuse_record(Vertexuse::default()).unwrap();
            b.edgeuse_mut(e_u).unwrap().edge = Some(e);
            b.edgeuse_mut(e_u).unwrap().orientation = o;
            b.edgeuse_mut(e_u).unwrap().vertexuse = Some(vu);
            b.vertexuse_mut(vu).unwrap().vertex = Some(v);
            b.vertexuse_mut(vu).unwrap().owner = VertexuseOwner::Edgeuse(Some(e_u));
            b.vertex_mut(v).unwrap().vertexuses.push(vu);
            b.edge_mut(e).unwrap().edgeuses.push(e_u);
            fb.insert_fillet_edgeuse(e_u, FilletEdgeuse::default());
            fb.insert_fillet_vertexuse(vu, FilletVertexuse::new());
            uses.push(e_u);
        }
        fb.brep_mut().edge_mut(e).unwrap().curve = Some(line(p0, p1));
        let mate_eu = eu::mate(fb.brep(), uses[0]).unwrap().unwrap();
        assert_eq!(mate_eu, uses[1]);
        fb.brep_mut().edgeuse_mut(mate_eu).unwrap().uv_trim_curve =
            Some(line(Point3::new(0.0, 0.0, 0.0), Point3::new(1.0, 0.0, 0.0)));
        (e, mate_eu)
    }

    struct Fixture {
        k: Kernel,
        fb: FilletBrep,
        g: FilletGeomId,
        rails: [EdgeId; 2],
        mates: [EdgeuseId; 2],
        solver: FilletSolver,
        target: Brep,
        st: CalcState,
    }

    impl Fixture {
        fn new() -> Self {
            let k = Kernel::default();
            let mut fb = FilletBrep::new();
            let (r0, m0) = add_rail(&k, &mut fb, 0.0);
            let (r1, m1) = add_rail(&k, &mut fb, 1.0);
            let mut geom = FilletGeom::new(&k, None, FilletGeomType::Default).unwrap();
            geom.rails = [Some(r0), Some(r1)];
            geom.center_line_curve =
                Some(line_bspline(Point3::new(0.0, 0.5, 0.5), Point3::new(1.0, 0.5, 0.5)).into());
            geom.set_fillet_surface(Some(strip()));
            let g = fb.add_geom(geom);

            let mut target = Brep::new();
            let tv0 = target.add_vertex(Point3::new(0.0, 0.0, 0.0)).unwrap();
            let tv1 = target.add_vertex(Point3::new(1.0, 0.0, 0.0)).unwrap();
            target.add_edge(tv0, tv1).unwrap();
            let st = CalcState::new(
                line(Point3::new(0.0, 0.0, 0.0), Point3::new(1.0, 0.0, 0.0)),
                Extent1d::from_raw(0.0, 1.0),
                [Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0)],
                [Vec3::new(1.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0)],
                [strip(), strip()],
                g,
                false,
            );
            let mut solver = FilletSolver::new(1.0e-5, 0.01, 0.01);
            solver.fillet_geoms = vec![g];
            Self {
                k,
                fb,
                g,
                rails: [r0, r1],
                mates: [m0, m1],
                solver,
                target,
                st,
            }
        }

        fn run(&mut self) -> Result<()> {
            let mut cx = Cx {
                k: &self.k,
                target: &mut self.target,
                fillets: &mut self.fb,
                solvers: GeometrySolvers::Indexed(&[]),
                error_info: None,
            };
            c5_post_process(&mut self.solver, &mut cx, &mut self.st)
        }

        fn rail_start(&self, i: usize) -> Point3 {
            let c = self
                .fb
                .brep()
                .edge(self.rails[i])
                .unwrap()
                .curve
                .as_ref()
                .unwrap();
            c.evaluate_point(&Kernel::default(), c.natural_interval().min())
                .unwrap()
        }

        fn uv_start(&self, i: usize) -> Point3 {
            let c = self
                .fb
                .brep()
                .edgeuse(self.mates[i])
                .unwrap()
                .uv_trim_curve
                .as_ref()
                .unwrap();
            c.evaluate_point(&Kernel::default(), c.natural_interval().min())
                .unwrap()
        }

        fn centre_start(&self) -> Point3 {
            let c = self.fb.geom(self.g).unwrap().center_line_curve().unwrap();
            c.evaluate_point(c.natural_interval().min()).unwrap()
        }

        fn surface_at(&self, uv: Point2) -> Point3 {
            self.fb
                .geom(self.g)
                .unwrap()
                .fillet_surface(None)
                .unwrap()
                .unwrap()
                .evaluate_point(&Kernel::default(), uv)
                .unwrap()
        }

        /// A second geometry of `geom_type` in `m_vFilletGeoms`.
        fn add_geom(&mut self, geom_type: FilletGeomType) -> FilletGeomId {
            let g = self
                .fb
                .add_geom(FilletGeom::new(&self.k, None, geom_type).unwrap());
            self.solver.fillet_geoms.push(g);
            g
        }
    }

    fn assert_close(a: Point3, b: Point3) {
        assert!((a - b).length() < 1.0e-12, "{a:?} != {b:?}");
    }

    #[test]
    fn single_geometry_without_split_or_reversal_changes_nothing() {
        let mut f = Fixture::new();
        f.run().unwrap();
        assert_close(f.rail_start(0), Point3::new(0.0, 0.0, 0.0));
        assert_close(f.rail_start(1), Point3::new(0.0, 1.0, 0.0));
        assert_close(f.uv_start(0), Point3::new(0.0, 0.0, 0.0));
        assert_close(f.centre_start(), Point3::new(0.0, 0.5, 0.5));
        assert_close(
            f.surface_at(Point2::new(0.0, 0.25)),
            Point3::new(0.0, 0.5, 0.0),
        );
        assert_eq!(f.solver.fillet_geoms.len(), 1);
    }

    #[test]
    fn reversed_trace_reverses_rails_uv_curves_centre_line_and_surface() {
        let mut f = Fixture::new();
        f.st.trace.reverse_tracing = true;
        f.run().unwrap();
        for (i, y) in [(0, 0.0), (1, 1.0)] {
            assert_close(f.rail_start(i), Point3::new(1.0, y, 0.0));
            assert_close(f.uv_start(i), Point3::new(1.0, 0.0, 0.0));
        }
        assert_close(f.centre_start(), Point3::new(1.0, 0.5, 0.5));
        assert_close(
            f.surface_at(Point2::new(0.0, 0.25)),
            Point3::new(3.0, 0.5, 0.0),
        );
    }

    #[test]
    fn reversed_trace_reverses_rails_before_the_centre_line_check() {
        let mut f = Fixture::new();
        f.st.trace.reverse_tracing = true;
        f.fb.geom_mut(f.g).unwrap().center_line_curve = None;
        let e = f.run().unwrap_err();
        assert!(matches!(
            e,
            SolidError::Upstream {
                code: SM_ERR_NULL_POINTER,
                ..
            }
        ));
        assert_close(f.rail_start(0), Point3::new(1.0, 0.0, 0.0));
        assert_close(
            f.surface_at(Point2::new(0.0, 0.25)),
            Point3::new(0.0, 0.5, 0.0),
        );
    }

    #[test]
    fn reversed_trace_ignores_a_failed_uv_trim_curve_creation() {
        let mut f = Fixture::new();
        f.st.trace.reverse_tracing = true;
        f.fb.brep_mut()
            .edgeuse_mut(f.mates[0])
            .unwrap()
            .uv_trim_curve = None;
        // the creation's status is ignored; NER(pUVCurve) then fails
        let e = f.run().unwrap_err();
        assert_eq!(e, SolidError::null_pointer(OP));
    }

    #[test]
    fn reversed_trace_rejects_a_rail_curve_that_is_not_a_bspline() {
        let mut f = Fixture::new();
        f.st.trace.reverse_tracing = true;
        f.fb.brep_mut().edge_mut(f.rails[1]).unwrap().curve = None;
        let e = f.run().unwrap_err();
        assert!(matches!(
            e,
            SolidError::Upstream {
                code: SM_ERR_NULL_POINTER,
                ..
            }
        ));
    }

    #[test]
    fn split_flag_with_one_geometry_reaches_topology_split() {
        let mut f = Fixture::new();
        f.st.split_original_fillet = true;
        // TopologySplit (blend.fillet_geom_p3) needs the geometry's solver, which the
        // fixture's geometry does not name: SM_ERR_NULL_POINTER
        let e = f.run().unwrap_err();
        assert!(
            matches!(
                e,
                SolidError::Upstream {
                    code: SM_ERR_NULL_POINTER,
                    ..
                }
            ),
            "{e:?}"
        );
    }

    #[test]
    fn split_flag_with_two_geometries_is_skipped() {
        let mut f = Fixture::new();
        f.st.split_original_fillet = true;
        f.add_geom(FilletGeomType::Default);
        f.run().unwrap();
    }

    #[test]
    fn self_intersection_patches_cross_sections_then_blends() {
        // default geometries without cross-section edges: nothing to do
        let mut f = Fixture::new();
        f.add_geom(FilletGeomType::Default);
        f.run().unwrap();

        // a blends geometry: CalcBlendingGeom (the fixture's geometry has no
        // GetFilletSolver(): NER)
        let mut f = Fixture::new();
        f.add_geom(FilletGeomType::Blends);
        let e = f.run().unwrap_err();
        assert_eq!(
            e,
            SolidError::null_pointer("blend::fillet_geom::p4::FilletBrep::calc_blending_geom")
        );

        // a cross-section edge of a default geometry comes first: CalcCrossSection
        let mut f = Fixture::new();
        f.add_geom(FilletGeomType::Blends);
        let rail = f.rails[0];
        f.fb.fillet_edge_mut(rail)
            .unwrap()
            .set_fillet_edge_type(FilletEdgeType::CrossSection);
        f.fb.geom_mut(f.g).unwrap().edges.push(rail);
        // CalcCrossSection (blend.fillet_geom_p5) runs: no fillet surface on the
        // edge's geometry, SM_ERR_NULL_POINTER
        let e = f.run().unwrap_err();
        assert!(
            matches!(
                e,
                SolidError::Upstream {
                    code: SM_ERR_NULL_POINTER,
                    ..
                }
            ),
            "{e:?}"
        );

        // a cross-section edge of a blends geometry is not calculated
        let mut f = Fixture::new();
        let g2 = f.add_geom(FilletGeomType::CliffRollover);
        let rail = f.rails[0];
        f.fb.fillet_edge_mut(rail)
            .unwrap()
            .set_fillet_edge_type(FilletEdgeType::CrossSection);
        f.fb.geom_mut(g2).unwrap().edges.push(rail);
        f.run().unwrap();
    }

    #[test]
    fn drop_ladder_returns_the_rail_parameter() {
        let k = Kernel::default();
        let rail = line(Point3::new(0.0, 0.0, 0.0), Point3::new(4.0, 0.0, 0.0));
        let ivl = rail.natural_interval();
        let t =
            c5_drop_split_param(&k, &rail, &ivl, Point3::new(1.0, 0.5, 0.0), 1.0e-5, 0.0).unwrap();
        assert!((t - 0.25).abs() < 1.0e-12, "{t}");
    }

    #[test]
    fn bspline_surface_cast_keeps_bspline_kinds() {
        assert!(c5_bspline_surface_cast(strip()).is_some());
    }
}

// P4-CHUNK(C5) END

// =====================================================================================
// P4-CHUNK(C6) BEGIN: ComputePointValues, SmFilletSolver.cpp:3544-3768
// (chunk-local imports go inside the functions: never edit the shared `use` block)
// =====================================================================================

/// :3544-3768, the whole body of `ComputePointValues` (see
/// [`FilletSolver::compute_point_values`]). The step is
/// `k.latch_fillet_point_step(if tangent { 1.0e-3 } else { 1.0e-5 })`.
fn c6_compute_point_values(
    ops: &mut dyn FilletSolverOps,
    k: &Kernel,
    mut brep: Option<&mut Brep>,
    uv_values: [Point2; 2],
    uv_domain1: &Extent2d,
    uv_domain2: &Extent2d,
    curve_trace_direction: f64,
    point: &mut TsectPnt,
    previous: Option<&TsectPnt>,
) -> Result<()> {
    use super::p1::compute_uv_vectors;
    use crate::error::ignore_status;
    use crate::intersect::surface_intersector::p1::{IntersectionPointType, SrfIndex};
    use crate::math::EFF_ZERO_SQ;

    // :3557 set the surface positions, tangents, normals and the tangent plane angle
    FilletSolver::compute_surface_values(k, ops, uv_values, point)?;

    // :3568 intersection curve tangent: (SrfNorm(0) * SrfNorm(1)) * dCurveTraceDirection
    *point.crv_deriv_mut() = point
        .srf_norm(SrfIndex::First)
        .cross(point.srf_norm(SrfIndex::Second))
        * curve_trace_direction;

    // :3571 surfaces tangent to within tolerance
    if point.crv_deriv().length_squared() < 10000.0 * EFF_ZERO_SQ {
        point.point_type = IntersectionPointType::TangentPoint;
        match previous {
            // use the previous point's curve tangent
            Some(prev) => *point.crv_deriv_mut() = prev.crv_deriv(),
            None => return Ok(()),
        }
    }

    // :3588 SER(CrvDeriv().Unitize())
    c6_unitize(point.crv_deriv_mut())?;

    // :3596 finite differences for variable radii or at a surface/surface tangency
    if ops.offset_radii_can_change()? || point.point_type == IntersectionPointType::TangentPoint {
        // :3600
        compute_uv_vectors(point)?;

        // :3604 static double sdStaticStep = TANGENT ? 1e-3 : 1e-5 (latched once; with
        // `fixes.fillet_point_step_per_point` chosen for this point, ND-3)
        let first = if point.point_type == IntersectionPointType::TangentPoint {
            1.0e-3
        } else {
            1.0e-5
        };
        let mut step = if k.fixes.fillet_point_step_per_point {
            first
        } else {
            k.latch_fillet_point_step(first)
        };

        // :3615 forward step (StepSolve may shorten `dStep`, passed by reference)
        let mut step_forw = TsectPnt::new();
        // bFoundForw, bClipped, bBoundaryHit
        let mut flags = StepSolveFlags::default();
        step_solve(
            ops,
            k,
            brep.as_deref_mut(),
            point,
            &mut step,
            uv_domain1,
            uv_domain2,
            &mut flags,
            &mut step_forw,
        )?;
        let mut found_forw = flags.found_answer;

        // :3619 bigger step in the small-step case; upstream passes `dStep` again, not
        // `dBigStep` (ported as written, WR-456), into the same `sStepForw`. With
        // `fixes.fillet_point_bigger_step_retry` the retry solves with the bigger step.
        if !found_forw && step < 1.0e-4 {
            let big_step = 1.0e-3;
            let mut big = big_step;
            let retry_step = if k.fixes.fillet_point_bigger_step_retry {
                &mut big
            } else {
                &mut step
            };
            step_solve(
                ops,
                k,
                brep.as_deref_mut(),
                point,
                retry_step,
                uv_domain1,
                uv_domain2,
                &mut flags,
                &mut step_forw,
            )?;
            found_forw = flags.found_answer;
            if found_forw {
                step = big_step;
            }
        }

        // :3629 surface points before the next StepSolve changes the offsets
        let forw_pts = if found_forw {
            let p0 = offset_evaluate_point(k, ops, 0, step_forw.uv_pos(SrfIndex::First))?;
            let p1 = offset_evaluate_point(k, ops, 1, step_forw.uv_pos(SrfIndex::Second))?;
            Some((p0, p1))
        } else {
            None
        };

        // :3640 backward step
        step = -step;
        let mut step_back = TsectPnt::new();
        // bFoundBack (a new local), and the same bClipped, bBoundaryHit
        let mut back_flags = StepSolveFlags {
            found_answer: false,
            ..flags
        };
        step_solve(
            ops,
            k,
            brep,
            point,
            &mut step,
            uv_domain1,
            uv_domain2,
            &mut back_flags,
            &mut step_back,
        )?;
        let found_back = back_flags.found_answer;

        let back_pts = if found_back {
            let p0 = offset_evaluate_point(k, ops, 0, step_back.uv_pos(SrfIndex::First))?;
            let p1 = offset_evaluate_point(k, ops, 1, step_back.uv_pos(SrfIndex::Second))?;
            Some((p0, p1))
        } else {
            None
        };

        // :3664 central difference, or one-sided at a domain end
        let (step_pt_forw, step_pt_back) = match (forw_pts, back_pts) {
            (Some((f0, f1)), Some((b0, b1))) => {
                let fw = (f0 + f1) / 2.0;
                let bk = (b0 + b1) / 2.0;
                step *= 2.0; // 2.0 * dStep (multiplication commutes exactly)
                (fw, bk)
            }
            (Some((f0, f1)), None) => ((f0 + f1) / 2.0, point.crv_pos()),
            (None, Some((b0, b1))) => (point.crv_pos(), (b0 + b1) / 2.0),
            (None, None) => return Err(SolidError::sm_err(OP)),
        };

        // :3685
        step = step.abs();
        let diff = step_pt_forw - step_pt_back;
        *point.crv_deriv_mut() = diff / step;
        c6_unitize(point.crv_deriv_mut())?;

        // :3694 tangent case: orient by the cross product of the stepped-off normals
        if point.point_type == IntersectionPointType::TangentPoint {
            let at = if found_forw { &step_forw } else { &step_back };
            let (uv0, uv1) = (at.uv_pos(SrfIndex::First), at.uv_pos(SrfIndex::Second));
            let norm1 = offset_evaluate_normal(k, ops, 0, uv0)?;
            let norm2 = offset_evaluate_normal(k, ops, 1, uv1)?;
            let mut curve_direction = norm1.cross(norm2);
            if curve_direction.length_squared() < EFF_ZERO_SQ {
                return Err(SolidError::sm_err(OP));
            }
            curve_direction *= curve_trace_direction;
            if curve_direction.dot(point.crv_deriv()) < 0.0 {
                *point.crv_deriv_mut() = -point.crv_deriv();
            }
        }
    }

    // :3761
    point.point_type = IntersectionPointType::Crossing;

    // :3764 ComputeUVVectors, status ignored
    ignore_status(compute_uv_vectors(point))?;

    Ok(())
}

/// `SmVector3d::Unitize()` in place (`bSignalBadInput = TRUE`): a vector of squared
/// length below `SM_EFF_ZERO_SQ` becomes `(0, 0, 1)` and the status is
/// `SM_ERR_INVALID_INPUT`; otherwise the unit vector (unchanged when already within
/// `SM_EFF_ZERO_SQ` of unit squared length).
fn c6_unitize(v: &mut Vec3) -> Result<()> {
    match v.unitize() {
        Ok(u) => {
            *v = u;
            Ok(())
        }
        Err(e) => {
            *v = Vec3::Z;
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests_c6 {
    use super::*;
    use crate::blend::fillet_custom_solver::{ConstantRadiusAssistedFs, SurfaceId};
    use crate::intersect::surface_intersector::p1::IntersectionPointType;
    use crate::nurbs::b_spline_surface::BSplineSurface;

    fn bilinear(p: [[f64; 3]; 4]) -> Surface {
        let [a, b, c, d] = p.map(Vec3::from);
        Surface::from(BSplineSurface::create_bilinear_surface(a, b, c, d).unwrap())
    }

    fn xy(z: f64) -> Surface {
        bilinear([[0.0, 0.0, z], [1.0, 0.0, z], [0.0, 1.0, z], [1.0, 1.0, z]])
    }

    /// The XZ plane, `u` along z and `v` along x (normal +y).
    fn xz() -> Surface {
        bilinear([
            [0.0, 0.0, 0.0],
            [0.0, 0.0, 1.0],
            [1.0, 0.0, 0.0],
            [1.0, 0.0, 1.0],
        ])
    }

    fn solver(k: &Kernel, a: &Surface, b: &Surface) -> ConstantRadiusAssistedFs {
        ConstantRadiusAssistedFs::from_parts(
            0.001,
            0.5,
            0.03,
            [0.25, 0.5],
            [
                Some(OffsetSurface::new(k, 0.25, a.clone(), SurfaceId::Free(0), 0).unwrap()),
                Some(OffsetSurface::new(k, 0.5, b.clone(), SurfaceId::Free(1), 0).unwrap()),
            ],
            [1.0, 1.0],
            [None, None],
        )
    }

    /// Constant radius solver with `OffsetRadiiCanChange() == TRUE` (finite differences).
    struct Variable(ConstantRadiusAssistedFs);

    impl FilletSolverOps for Variable {
        fn solver(&self) -> &FilletSolver {
            &self.0.solver
        }
        fn solver_mut(&mut self) -> &mut FilletSolver {
            &mut self.0.solver
        }
        fn setup_offset_values(
            &mut self,
            tsect: &TsectPnt,
            offset_dist: Option<&mut f64>,
        ) -> Result<()> {
            FilletSolverOps::setup_offset_values(&mut self.0, tsect, offset_dist)
        }
    }

    fn dom() -> Extent2d {
        Extent2d::from_bounds(-1.0, -1.0, 2.0, 2.0)
    }

    fn run(
        ops: &mut dyn FilletSolverOps,
        k: &Kernel,
        dir: f64,
        point: &mut TsectPnt,
        previous: Option<&TsectPnt>,
    ) -> Result<()> {
        let uv = [Point2::new(0.5, 0.25), Point2::new(0.25, 0.5)];
        FilletSolver::compute_point_values(ops, k, uv, &dom(), &dom(), dir, point, previous)
    }

    #[test]
    fn crossing_point_constant_radius() {
        let k = Kernel::default();
        let (a, b) = (xy(0.0), xz());
        let mut fs = solver(&k, &a, &b);
        for dir in [1.0, -1.0] {
            let mut p = TsectPnt::new();
            run(&mut fs, &k, dir, &mut p, None).unwrap();
            assert_eq!(p.point_type, IntersectionPointType::Crossing);
            // n0 = +z, n1 = +y: (z x y) * dir = -x * dir
            assert_eq!(p.curve_pv[1], Vec3::new(-dir, 0.0, 0.0));
            assert_eq!(p.curve_pv[0], p.surface_pv[0][0][0]);
            // uv tangent on surface 0 (du = x) is (-dir, 0)
            assert!((p.uv_curve_pv[0][1].x + dir).abs() < 1e-12);
            assert!(p.uv_curve_pv[0][1].y.abs() < 1e-12);
        }
        // no finite differences: the step stays unlatched
        assert_eq!(k.fillet_point_step.get(), None);
    }

    #[test]
    fn tangent_without_previous_returns_early() {
        let k = Kernel::default();
        let (a, b) = (xy(0.0), xy(1.0));
        let mut fs = solver(&k, &a, &b);
        let mut p = TsectPnt::new();
        run(&mut fs, &k, 1.0, &mut p, None).unwrap();
        assert_eq!(p.point_type, IntersectionPointType::TangentPoint);
        assert_eq!(p.curve_pv[1], Vec3::new(0.0, 0.0, 0.0));
        assert_eq!(k.fillet_point_step.get(), None);
    }

    #[test]
    fn tangent_with_zero_previous_derivative_fails_unitize() {
        let k = Kernel::default();
        let (a, b) = (xy(0.0), xy(1.0));
        let mut fs = solver(&k, &a, &b);
        let prev = TsectPnt::new();
        let mut prev = prev;
        *prev.crv_deriv_mut() = Vec3::new(0.0, 0.0, 0.0);
        let mut p = TsectPnt::new();
        let err = run(&mut fs, &k, 1.0, &mut p, Some(&prev)).unwrap_err();
        assert!(matches!(err, SolidError::InvalidInput { .. }));
        // upstream's Unitize leaves (0, 0, 1)
        assert_eq!(p.curve_pv[1], Vec3::Z);
        assert_eq!(p.point_type, IntersectionPointType::TangentPoint);
    }

    #[test]
    fn tangent_with_previous_reaches_step_solve() {
        let k = Kernel::default();
        let (a, b) = (xy(0.0), xy(1.0));
        let mut fs = solver(&k, &a, &b);
        let mut prev = TsectPnt::new();
        *prev.crv_deriv_mut() = Vec3::new(0.0, 3.0, 0.0);
        let mut p = TsectPnt::new();
        // StepSolve (blend.fillet_solver_p3) finds no step on these planes: SM_ERR
        let err = run(&mut fs, &k, 1.0, &mut p, Some(&prev)).unwrap_err();
        assert!(
            matches!(err, SolidError::Upstream { code: 1001, .. }),
            "{err:?}"
        );
        assert_eq!(p.curve_pv[1], Vec3::new(0.0, 1.0, 0.0));
        // the tangent point latches the larger step
        assert_eq!(k.fillet_point_step.get(), Some(1.0e-3));
    }

    #[test]
    fn variable_radius_crossing_latches_small_step() {
        let k = Kernel::default();
        let (a, b) = (xy(0.0), xz());
        let mut fs = Variable(solver(&k, &a, &b));
        let mut p = TsectPnt::new();
        // StepSolve rejects the test solver's Jacobian size
        let err = run(&mut fs, &k, 1.0, &mut p, None).unwrap_err();
        assert!(matches!(err, SolidError::InvalidInput { .. }), "{err:?}");
        assert_eq!(k.fillet_point_step.get(), Some(1.0e-5));
        // an already latched step is kept
        let k2 = Kernel::default();
        k2.fillet_point_step.set(Some(7.0e-4));
        let mut p = TsectPnt::new();
        let _ = run(&mut fs, &k2, 1.0, &mut p, None);
        assert_eq!(k2.fillet_point_step.get(), Some(7.0e-4));
    }

    /// `Variable` that records the finite-difference solves' first guesses
    /// (`load_initial_values`) and makes the solve after the first guess find nothing.
    struct Probe {
        inner: Variable,
        guesses: Vec<Vec<f64>>,
    }

    impl FilletSolverOps for Probe {
        fn solver(&self) -> &FilletSolver {
            self.inner.solver()
        }
        fn solver_mut(&mut self) -> &mut FilletSolver {
            self.inner.solver_mut()
        }
        fn setup_offset_values(
            &mut self,
            tsect: &TsectPnt,
            offset_dist: Option<&mut f64>,
        ) -> Result<()> {
            self.inner.setup_offset_values(tsect, offset_dist)
        }
        fn jacobian_size(&self) -> Result<usize> {
            Ok(4)
        }
        fn load_jacobian(
            &mut self,
            k: &Kernel,
            num_equations: &mut usize,
            num_parameters: &mut usize,
            load: &mut crate::blend::fillet_custom_solver::JacobianLoad<'_>,
            f: &mut [f64],
            jacobian: Option<&mut crate::geom::matrix::Matrix>,
        ) -> Result<bool> {
            let found = FilletSolverOps::load_jacobian(
                &mut self.inner.0,
                k,
                num_equations,
                num_parameters,
                load,
                f,
                jacobian,
            )?;
            if self.guesses.len() == 1 {
                // no root: the first solve ends without a point
                for v in f.iter_mut() {
                    *v += 1.0;
                }
            }
            Ok(found)
        }
        fn load_initial_values(
            &mut self,
            _k: &Kernel,
            _brep: Option<&mut Brep>,
            _do_surf2_calcs: bool,
            _rail_index: usize,
            _surface1: &mut OffsetSurface,
            _surface1_index: usize,
            _intervals: &mut crate::geom::extent1d::ExtentNd,
            _periodicities: &mut Vec<bool>,
            guess_t: &mut Vec<f64>,
        ) -> Result<Option<crate::blend::fillet_custom_solver::InitialValues>> {
            self.guesses.push(guess_t.clone());
            Ok(None)
        }
    }

    /// WR-456 (#1181): after a failed small forward step the retry steps by the bigger
    /// step with the fix, and by the small one again as upstream does without it.
    #[test]
    fn bigger_step_retry_uses_the_bigger_step_with_the_fix() {
        let (a, b) = (xy(0.0), xz());
        let mut moves = Vec::new();
        for fixed in [false, true] {
            let mut k = Kernel::default();
            k.fixes.fillet_point_bigger_step_retry = fixed;
            let mut ops = Probe {
                inner: Variable(solver(&k, &a, &b)),
                guesses: Vec::new(),
            };
            let mut p = TsectPnt::new();
            let r = run(&mut ops, &k, 1.0, &mut p, None);
            assert!(ops.guesses.len() >= 2, "{r:?} {:?}", ops.guesses);
            // how far each of the first two guesses lies from the point's uv
            let from = |g: &[f64]| (g[0] - 0.5).hypot(g[1] - 0.25);
            let (first, retry) = (from(&ops.guesses[0]), from(&ops.guesses[1]));
            assert!(first > 0.0 && first < 1.0e-4, "{first}");
            moves.push(retry / first);
        }
        assert!((moves[0] - 1.0).abs() < 1.0e-9, "{moves:?}");
        assert!((moves[1] - 100.0).abs() < 1.0e-6, "{moves:?}");
    }

    /// #1173 (ND-3): the first finite-difference guess lies one step from the point. With the
    /// fix that step is the point's own whatever an earlier blend latched; without it the
    /// latched step is used. The latch itself is neither read nor written with the fix.
    #[test]
    fn per_point_step_ignores_the_latch_with_the_fix() {
        let (a, b) = (xy(0.0), xy(1.0));
        let mut prev = TsectPnt::new();
        *prev.crv_deriv_mut() = Vec3::new(0.0, 3.0, 0.0);
        // distance of the first guess from the point's uv, for the fix on/off and a latch
        let first_guess = |fixed: bool, latched: Option<f64>| {
            let mut k = Kernel::default();
            k.fixes.fillet_point_step_per_point = fixed;
            k.fillet_point_step.set(latched);
            let mut ops = Probe {
                inner: Variable(solver(&k, &a, &b)),
                guesses: Vec::new(),
            };
            let _ = run(&mut ops, &k, 1.0, &mut TsectPnt::new(), Some(&prev));
            assert_eq!(
                k.fillet_point_step.get(),
                if fixed {
                    latched
                } else {
                    latched.or(Some(1.0e-3))
                }
            );
            let g = ops.guesses.first().expect("a finite-difference solve ran");
            (g[0] - 0.5).hypot(g[1] - 0.25)
        };
        let (fresh, preset) = (first_guess(true, None), first_guess(true, Some(1.0e-5)));
        assert!(fresh > 0.0);
        assert_eq!(fresh, preset);
        // without the fix the preset latch changes the step
        assert!(first_guess(false, Some(1.0e-5)) < fresh / 10.0);
        assert_eq!(first_guess(false, None), fresh);
    }

    #[test]
    fn base_solver_setup_offset_values_fails() {
        // SmFilletSolver::SetupOffsetValues (base) returns SM_ERR through
        // ComputeSurfaceValues' SER
        let k = Kernel::default();
        let mut solver = FilletSolver::new(1.0e-5, 0.01, 0.01);
        let mut p = TsectPnt::new();
        let e = run(&mut solver, &k, 1.0, &mut p, None).unwrap_err();
        assert!(!e.is_unported());
        assert_eq!(e, SolidError::sm_err("blend::fillet_solver::p1"));
    }

    #[test]
    fn unitize_matches_upstream_in_place() {
        let mut v = Vec3::new(0.0, 3.0, 4.0);
        c6_unitize(&mut v).unwrap();
        assert_eq!(v, Vec3::new(0.0, 0.6, 0.8));
        let mut z = Vec3::new(1e-13, 0.0, 0.0);
        assert!(matches!(
            c6_unitize(&mut z),
            Err(SolidError::InvalidInput { .. })
        ));
        assert_eq!(z, Vec3::Z);
    }
}

// P4-CHUNK(C6) END

// =====================================================================================
// STUBS: calls into units that are not ported (one helper each; chunks call these)
// =====================================================================================

/// `CalcCylinderFilletGeom(sTsectPnts)` (default `pFG = NULL`).
fn calc_cylinder_fillet_geom(
    ops: &mut dyn FilletSolverOps,
    cx: &mut Cx<'_>,
    store: &mut TsectStore,
    points: &[TsRef],
) -> Result<()> {
    // the points are read only (a snapshot of sTsectPnts)
    let tsects = store.snapshot(cx.fillets, points)?;
    FilletSolver::calc_cylinder_fillet_geom(ops, cx.k, cx.target, cx.fillets, &tsects, None)
}

/// `CalcTorusFilletGeom(sTsectPnts)` (default `pFG = NULL`).
fn calc_torus_fillet_geom(
    ops: &mut dyn FilletSolverOps,
    cx: &mut Cx<'_>,
    store: &mut TsectStore,
    points: &[TsRef],
) -> Result<()> {
    let tsects = store.snapshot(cx.fillets, points)?;
    FilletSolver::calc_torus_fillet_geom(ops, cx.k, cx.target, cx.fillets, &tsects, None)
}

/// `RecordFilletError(eErrorType, sTsectPnts, pMessage)` (upstream ignores its
/// status: call through `ignore_status`, which still propagates `Unported`).
fn record_fillet_error(
    ops: &mut dyn FilletSolverOps,
    cx: &mut Cx<'_>,
    store: &TsectStore,
    points: &[TsRef],
    error_type: FilletErrorType,
    message: &'static str,
) -> Result<()> {
    let tsects = store.snapshot(cx.fillets, points)?;
    ops.solver().record_fillet_error_in(
        cx.target,
        cx.fillets,
        error_type,
        &tsects,
        Some(message),
        cx.error_info.as_deref_mut(),
    )
}

/// `sm_FindFilletTsectPnt(this, pBaseSrf1, pBaseSrf2, rPnt, rTangent, rTsectPnt)`:
/// the fillet point on the plane through `point` normal to `tangent`, written to
/// `out`.
fn find_fillet_tsect_pnt(
    ops: &mut dyn FilletSolverOps,
    k: &Kernel,
    brep: &mut Brep,
    base1: &Surface,
    base2: &Surface,
    point: Point3,
    tangent: Vec3,
    out: &mut TsectPnt,
) -> Result<()> {
    super::p3::find_fillet_tsect_pnt(ops, k, Some(brep), base1, base2, point, tangent, out)
}

/// `StepSolve(rTsectPnt, rdStep, crUVDomain1, crUVDomain2, rbFoundAnswer,
/// rbClipped, rbBoundaryHit, rNextPoint)`: writes the flags into `flags` (as upstream
/// writes its out-params, so they keep what was set before a failure) and the next
/// point into `next` (its UV positions, `m_adUserDoubles`, or all of it through
/// `PointOnPlaneSolve`), and may shorten `step` (`double & rdStep`, in/out). Changes
/// the offset surfaces' offsets.
fn step_solve(
    ops: &mut dyn FilletSolverOps,
    k: &Kernel,
    brep: Option<&mut Brep>,
    point: &TsectPnt,
    step: &mut f64,
    uv_domain1: &Extent2d,
    uv_domain2: &Extent2d,
    flags: &mut StepSolveFlags,
    next: &mut TsectPnt,
) -> Result<()> {
    // `brep` is the target B-rep holding the solver's edge uses (`GetEdgeuse`, read by
    // `LoadInitialValues`) when the caller has it; the flags are copied back on an
    // error too
    let mut out = super::p3::StepSolveOutcome::default();
    let r = FilletSolver::step_solve(
        ops, k, brep, point, step, uv_domain1, uv_domain2, next, &mut out,
    );
    flags.found_answer = out.found_good_point;
    flags.clipped = out.clipped;
    flags.boundary_hit = out.boundary_hit;
    r
}

/// `pFilletGeom->TopologySplit(rpNewFilletGeom, eType)`: the new geometry.
fn topology_split(
    ops: &dyn FilletSolverOps,
    cx: &mut Cx<'_>,
    geom: FilletGeomId,
    geom_type: FilletGeomType,
) -> Result<FilletGeomId> {
    // the geometry's GetFilletSolver() is this solver
    let solver_id = cx
        .fillets
        .geom(geom)?
        .fillet_solver()
        .ok_or_else(|| SolidError::null_pointer(OP))?;
    // a standalone entry point holds no executive: no corner of any vertex
    let corners: &mut dyn SplitCorners = match &mut cx.solvers {
        GeometrySolvers::Indexed(_) => &mut NoCorners,
        GeometrySolvers::Live(context) => context.split_corners(),
    };
    cx.fillets.topology_split(
        cx.k,
        geom,
        geom_type,
        solver_id,
        ops.solver(),
        cx.target,
        corners,
    )
}

/// `pFilletGeom->CalcBlendingGeom()` ([`FilletBrep::calc_blending_geom_of_solver`],
/// unit `blend.fillet_geom_p4`): the geometry's `GetFilletSolver()` is this solver
/// (every geometry of `m_vFilletGeoms` is made by it or split from one of its own).
fn calc_blending_geom(
    ops: &dyn FilletSolverOps,
    cx: &mut Cx<'_>,
    geom: FilletGeomId,
) -> Result<()> {
    cx.fillets
        .calc_blending_geom_of_solver(cx.k, geom, ops.solver(), cx.target)
}

/// `pFilletEdge->CalcCrossSection(m_crContext)` (a pseudo-B-rep edge).
fn calc_cross_section(ops: &dyn FilletSolverOps, cx: &mut Cx<'_>, edge: EdgeId) -> Result<()> {
    match &cx.solvers {
        GeometrySolvers::Indexed(solvers) => cx.fillets.calc_cross_section(
            cx.k,
            edge,
            None,
            CornerLookup::default(),
            solvers,
            cx.target,
        ),
        GeometrySolvers::Live(context) => cx.fillets.calc_cross_section_with_lookup(
            cx.k,
            edge,
            None,
            context.corners(),
            &|id| context.solver(id, ops.solver()),
            cx.target,
        ),
    }
}

// =====================================================================================
// CALLEES of topology.edgeuse_p3 (ported): thin adapters with upstream's defaults
// =====================================================================================

/// `SER(pEU->GetOrCreateUVTrimCurve(rpUVTrimCurve))` with the defaults
/// (`pdOptDistToSurf = pdOptDeviation = NULL`, `bOKToMakeUVTrimCurve = TRUE`,
/// `SmEdgeuse.h:127-136`): [`edgeuse_p3::get_or_create_uv_trim_curve`] (which builds
/// and attaches a missing curve to `brep`), its failing status returned, then a copy
/// of the curve attached to the edge use or its mate (`GetUVTrimCurve`).
fn get_or_create_uv_trim_curve(k: &Kernel, brep: &mut Brep, edgeuse: EdgeuseId) -> Result<Curve> {
    edgeuse_p3::get_or_create_uv_trim_curve(k, brep, edgeuse, false, false, true)?.status?;
    // a successful fetch with bOKToMakeUVTrimCurve = TRUE always leaves a curve
    eu::uv_trim_curve(brep, edgeuse)?
        .0
        .cloned()
        .ok_or_else(|| SolidError::null_pointer(OP))
}

/// `SmEdgeuse::EvaluateBinormal(t, FALSE, rPnt, rBinVec, &sTangent, NULL, &sUVParameter)`
/// ([`edgeuse_p3::evaluate_binormal`]; `bUse3DOnly = FALSE`: get or create the UV trim
/// curve, so the target B-rep may change; no `pOptUVDir`).
fn evaluate_binormal(
    k: &Kernel,
    brep: &mut Brep,
    edgeuse: EdgeuseId,
    t: f64,
) -> Result<BinormalEval> {
    edgeuse_p3::evaluate_binormal(k, brep, edgeuse, t, false, &BinormalOptions::default())
}

/// The inputs of `sFI.DoPointIntersection(m_crContext, sTsectPnts, bExtendBefore,
/// bExtendAfter, &sDir, NULL, &dThisApproxTol3d, &dAngleTol, ..)`.
#[derive(Debug, Clone, Copy)]
struct PointIntersectionArgs {
    /// `bExtendBefore`.
    extend_before: bool,
    /// `bExtendAfter`.
    extend_after: bool,
    /// `&sDir` (the end direction is `NULL`).
    start_dir: Option<Vec3>,
    /// `&dThisApproxTol3d`.
    approx_tol_3d: f64,
    /// `&dAngleTol`.
    ang_tol_rad: f64,
}

/// `sFI.DoPointIntersection(..)` over `points` (a snapshot of `sTsectPnts`, written
/// in place as upstream writes through the pointers; the curve outputs are unused by
/// `CalcFilletGeom`; `FlushCurve` fills the solver's last fillet geometry).
///
/// The points are copied into the intersector's point arena, the inherited driver
/// runs with the `SmFilletIntersector` overrides
/// ([`FilletIntersector::do_point_intersection`], stepping through this solver as
/// [`IntersectorSolver`]), and every point is copied back whatever the result (the
/// error is returned after the copy). `points` is a slice (a `&mut Vec` snapshot
/// coerces to it).
fn do_point_intersection(
    fi: &mut TraceIntersector<'_>,
    ops: &mut dyn FilletSolverOps,
    cx: &mut Cx<'_>,
    points: &mut [TsectPnt],
    args: PointIntersectionArgs,
) -> Result<()> {
    use crate::intersect::surface_intersector::p1::SrfIndex;
    use crate::intersect::surface_intersector::p4::PointIntersectionOptions;

    let ids: Vec<_> = points
        .iter()
        .map(|p| fi.base.base.alloc_point(p.clone()))
        .collect();
    let opts = PointIntersectionOptions {
        extend_before_start: args.extend_before,
        extend_after_end: args.extend_after,
        start_direction: args.start_dir,
        end_direction: None,
        approx_tol_3d: Some(args.approx_tol_3d),
        ang_tol_rad: Some(args.ang_tol_rad),
    };
    let k = cx.k;
    let trace = [
        fi.base.base.surface(SrfIndex::First),
        fi.base.base.surface(SrfIndex::Second),
    ];
    // offsets the solver set since the last call (`DoPointIntersection` evaluates the
    // surfaces before its first solver call)
    sync_solver_extensions(&*ops, trace)?;
    sync_trace_offsets(&*ops, trace)?;
    let mut solver = IntersectorSolver {
        ops,
        trace,
        brep: Some(&mut *cx.target),
    };
    let r = fi.do_point_intersection(&mut solver, &mut *cx.fillets, k, &ids, &opts);
    for (p, &id) in points.iter_mut().zip(&ids) {
        p.clone_from(fi.base.base.point(id)?);
    }
    r.map(|_| ())
}

/// `SmFilletIntersector sFI(crSurface1, crDomain1, crSurface2, crDomain2, *this)` and
/// `sFI.DoPointIntersection(.., rTsectPnts, bExtendBefore, bExtendAfter, NULL, NULL,
/// &dApproxTol3d, &dAngTol, ..)` for a solver that traces outside
/// [`FilletSolver::calc_fillet_geom_with_context`] (`SmSurfaceSurfaceFS::CalcFilletGeom`).
/// `surfaces` are copies of the solver's offset surfaces; `points` are written back in
/// place as upstream writes through the pointers; `FlushCurve` fills the solver's last
/// fillet geometry in `fillets`.
///
/// # Errors
/// Those of the intersector construction and of the point intersection.
#[allow(clippy::too_many_arguments)]
pub(crate) fn point_intersection_on_surfaces(
    ops: &mut dyn FilletSolverOps,
    k: &Kernel,
    target: &mut Brep,
    fillets: &mut FilletBrep,
    surfaces: &[Surface; 2],
    domains: [Extent2d; 2],
    points: &mut [TsectPnt],
    extend: [bool; 2],
    approx_tol_3d: f64,
    ang_tol_rad: f64,
) -> Result<()> {
    let [surface1, surface2] = surfaces;
    let [domain1, domain2] = &domains;
    let mut fi = TraceIntersector::new(k, surface1, domain1, surface2, domain2, None)?;
    let mut cx = Cx {
        k,
        target,
        fillets,
        solvers: GeometrySolvers::Indexed(&[]),
        error_info: None,
    };
    let [extend_before, extend_after] = extend;
    let args = PointIntersectionArgs {
        extend_before,
        extend_after,
        start_dir: None,
        approx_tol_3d,
        ang_tol_rad,
    };
    do_point_intersection(&mut fi, ops, &mut cx, points, args)
}

/// The solver `*this` as `SmFilletIntersector` uses it (`m_rFilletSolver`): the
/// fillet intersector's [`FilletSolverOps`](crate::blend::fillet_intersector::FilletSolverOps)
/// over this solver's virtuals. `FlushCurve`'s writes into the fillet geometry go to
/// the pseudo B-rep the intersector's dispatch passes in.
pub(crate) struct IntersectorSolver<'b> {
    /// The solver (`this`).
    ops: &'b mut dyn FilletSolverOps,
    /// The intersector's surfaces (copies of the solver's offset surfaces), kept at
    /// the solver's offset distances by [`sync_trace_offsets`] after every call that
    /// may set them.
    trace: [&'b Surface; 2],
    /// The target B-rep holding the solver's edge uses (upstream's `GetEdgeuse`
    /// pointers reach it directly), passed to `LoadInitialValues` under
    /// `ComputePointValues` and `StepSolve`, and read for the rails' original faces
    /// by `SmoothOutIntersection`; `None` where the caller has none.
    brep: Option<&'b mut Brep>,
}

/// Upstream's intersector evaluates the solver's own `SmOffsetSurface`s, so every
/// `SetOffsetDistance` the solver makes (`SetSurfacesToZeroOffset`,
/// `ReloadSurfaceOffsets`, `SetupOffsetValues` under `ComputePointValues`,
/// `ComputeSurfaceValues`, `StepSolve`) changes the surfaces it traces; here the trace
/// surfaces are copies, which take the solver's distances, and the base extension the
/// solver's evaluations made (`m_pExtendedSurface` of the one object upstream). A trace
/// surface that is not an offset surface (only tests build one) has nothing to follow.
fn sync_trace_offsets(ops: &dyn FilletSolverOps, trace: [&Surface; 2]) -> Result<()> {
    for (rail, surface) in trace.into_iter().enumerate() {
        if let SurfaceKind::OffsetSurface(copy) = surface.kind() {
            let own = &offset_surface(ops, rail)?.surface;
            copy.set_offset_distance(own.offset_distance());
            copy.follow_extension_of(own);
        }
    }
    Ok(())
}

/// The other direction of [`sync_trace_offsets`], before a solver call: the solver's
/// offset surfaces take the base extension the intersector's evaluations of the trace
/// surfaces made since the last call (the intersector's constructor, its span tests and
/// flush evaluate them; upstream they are the solver's own objects).
fn sync_solver_extensions(ops: &dyn FilletSolverOps, trace: [&Surface; 2]) -> Result<()> {
    for (rail, surface) in trace.into_iter().enumerate() {
        if let SurfaceKind::OffsetSurface(copy) = surface.kind() {
            offset_surface(ops, rail)?.surface.follow_extension_of(copy);
        }
    }
    Ok(())
}

impl<'b> IntersectorSolver<'b> {
    /// The solver `ops` as an `SmFilletIntersector` over the trace surfaces `trace`
    /// (copies of the solver's offset surfaces) uses it; the trace surfaces take the
    /// solver's current offset distances first. Used by the other solvers that run a
    /// fillet intersector on themselves (`SmVariableRadiusFS::ReCalcFilletGeom`).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for a solver without its offset surfaces.
    pub(crate) fn new(ops: &'b mut dyn FilletSolverOps, trace: [&'b Surface; 2]) -> Result<Self> {
        sync_solver_extensions(&*ops, trace)?;
        sync_trace_offsets(&*ops, trace)?;
        Ok(Self {
            ops,
            trace,
            brep: None,
        })
    }

    /// [`sync_solver_extensions`] before a solver call.
    fn pre(&self) -> Result<()> {
        sync_solver_extensions(&*self.ops, self.trace)
    }

    /// [`sync_trace_offsets`] after a solver call, whatever its result (a failing call
    /// may have set distances before failing, and the intersector ignores some
    /// failures).
    fn synced<T>(&self, r: Result<T>) -> Result<T> {
        sync_trace_offsets(&*self.ops, self.trace)?;
        r
    }
}

/// The rail edge `rail` of geometry `geom` of the pseudo B-rep.
fn geom_rail(pseudo: &FilletBrep, geom: FilletGeomId, rail: usize) -> Result<EdgeId> {
    pseudo
        .geom(geom)?
        .rail(rail)
        .ok_or_else(|| invalid("fillet geometry without the rail"))
}

impl crate::blend::fillet_intersector::FilletSolverOps for IntersectorSolver<'_> {
    type FilletGeom = FilletGeomId;

    /// [`FilletSolver::compute_point_values`].
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
        self.pre()?;
        let r = c6_compute_point_values(
            &mut *self.ops,
            k,
            self.brep.as_deref_mut(),
            uv_values,
            uv_domain1,
            uv_domain2,
            curve_trace_direction,
            point,
            previous,
        );
        self.synced(r)
    }

    /// [`FilletSolver::compute_surface_values`].
    fn compute_surface_values(
        &mut self,
        k: &Kernel,
        uv_values: [Point2; 2],
        point: &mut TsectPnt,
    ) -> Result<()> {
        self.pre()?;
        let r = FilletSolver::compute_surface_values(k, &mut *self.ops, uv_values, point);
        self.synced(r)
    }

    /// `ComputeUVVectors` (`super::p1::compute_uv_vectors`).
    fn compute_uv_vectors(&mut self, _k: &Kernel, point: &mut TsectPnt) -> Result<()> {
        super::p1::compute_uv_vectors(point)
    }

    /// `StepSolve` (the stub of this file), writing into `flags`, `next` and
    /// `step_size` as upstream writes its flags, `rNextTSP` and the caller's
    /// `dStepSize`.
    fn step_solve(
        &mut self,
        k: &Kernel,
        point: &TsectPnt,
        step_size: &mut f64,
        uv_domain1: &Extent2d,
        uv_domain2: &Extent2d,
        flags: &mut StepSolveFlags,
        next: &mut TsectPnt,
    ) -> Result<()> {
        self.pre()?;
        let r = step_solve(
            &mut *self.ops,
            k,
            self.brep.as_deref_mut(),
            point,
            step_size,
            uv_domain1,
            uv_domain2,
            flags,
            next,
        );
        self.synced(r)
    }

    fn set_surfaces_to_zero_offset(&mut self) -> Result<()> {
        self.pre()?;
        let r = self.ops.solver_mut().set_surfaces_to_zero_offset();
        self.synced(r)
    }

    fn reload_surface_offsets(&mut self) -> Result<()> {
        self.pre()?;
        let r = self.ops.solver_mut().reload_surface_offsets();
        self.synced(r)
    }

    /// `GetSurface(rail)->Evaluate1stDerivatives(uv, TRUE, TRUE, ..)` on the solver's
    /// own offset surface (a base extension the evaluation makes is kept, as
    /// upstream's `const` evaluators store it), as `ComputeSurfaceValues` evaluates it.
    fn rail_surface_evaluate_1st_derivatives(
        &mut self,
        k: &Kernel,
        rail: usize,
        uv: Point2,
    ) -> Result<(Point3, Vec3, Vec3)> {
        self.pre()?;
        let r = crate::blend::fillet_custom_solver::offset_evaluate_1st(
            k,
            offset_surface_mut(&mut *self.ops, rail)?,
            uv,
        );
        self.synced(r)
    }

    /// `SetStatus` (the intersector's co-ported `SmFilStatus` mapped onto the
    /// solver's, value for value).
    /// The solver's `PointOnPlaneSolve` (virtual).
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
        self.pre()?;
        let r = self.ops.point_on_plane_solve(
            k,
            self.brep.as_deref_mut(),
            plane_origin,
            plane_normal,
            uv_domain1,
            uv_domain2,
            uv1,
            uv2,
            point,
        );
        self.synced(r)
    }

    fn set_status(&mut self, status: crate::blend::fillet_intersector::FilStatus) -> Result<()> {
        self.ops.solver_mut().set_status(status);
        Ok(())
    }

    fn last_fillet_geom(&self) -> Result<FilletGeomId> {
        self.ops.solver().last_fillet_geom()
    }

    fn self_intersection_handler(
        &self,
    ) -> Result<Option<crate::blend::fillet_intersector::HandlingAlgorithmType>> {
        // the handler is the executive's member: the base reports the missing
        // executive (a NULL dereference upstream)
        self.ops.solver().self_intersection_handler()
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
        // the executive's handler (FlushCurve calls this for a smoothing one, which
        // only SmMakeCurveBlendSIH is)
        let handler = self
            .ops
            .solver()
            .executive_curve_blend_sih
            .ok_or_else(|| invalid("no smoothing self-intersection handler"))?;
        handler.smooth_out_intersection(self, k, pseudo, *geom, curves_3d, uv_curves1, uv_curves2)
    }

    fn has_surface_generator(&self) -> Result<bool> {
        Ok(self.ops.solver().fillet_surface_generator().is_some())
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
    ) -> Result<Option<crate::nurbs::b_spline_surface::BSplineSurface>> {
        use crate::blend::fillet_standard_solver::p1::RailSurface;
        // the virtual m_pFSG->CreateSurface(*this, pCenterLine, pCurve1, pCurve2,
        // sPnts, pSur[0], pUV1, pSur[1], pUV2, ..) with pSur[i] = GetSurface(i)
        let solver_type = self.ops.solver_type()?;
        self.pre()?;
        let sur0 = Surface::from(offset_surface(&*self.ops, 0)?.surface.clone());
        let sur1 = Surface::from(offset_surface(&*self.ops, 1)?.surface.clone());
        let solver = self.ops.solver();
        let generator = solver
            .fillet_surface_generator()
            .ok_or_else(|| SolidError::null_pointer(OP))?;
        generator
            .create_surface(
                k,
                solver.approx_tol_3d,
                solver_type,
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

    fn set_fillet_surface(
        &mut self,
        pseudo: &mut FilletBrep,
        geom: &FilletGeomId,
        surface: Option<crate::nurbs::b_spline_surface::BSplineSurface>,
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
        use crate::topology::edge as edge_unit;

        let rail_edge = geom_rail(pseudo, *geom, rail)?;
        let interval = curve.natural_interval();
        // SetCurve(curve, TRUE): the old curve is deleted and the change notification
        // clears the edge uses' UV trim curves and cached gaps
        crate::blend::fillet_geom::p1::set_rail_curve(
            pseudo.brep_mut(),
            rail_edge,
            Curve::from(curve),
        )?;
        edge_unit::set_interval(pseudo.brep_mut(), rail_edge, &interval)
    }

    fn set_rail_mate_uv_curve(
        &mut self,
        pseudo: &mut FilletBrep,
        geom: &FilletGeomId,
        rail: usize,
        uv_curve: BSplineCurve,
    ) -> Result<()> {
        let rail_edge = geom_rail(pseudo, *geom, rail)?;
        let primary = eu::primary_edgeuse(pseudo.brep(), rail_edge)?
            .ok_or_else(|| invalid("rail without an edge use"))?;
        let mate = eu::mate(pseudo.brep(), primary)?
            .ok_or_else(|| invalid("rail edge use without a mate"))?;
        pseudo.set_edgeuse_uv_curve(mate, Some(Curve::from(uv_curve)), true, true)
    }
}

/// The solver members `SmMakeCurveBlendSIH::SmoothOutIntersection` reaches.
impl crate::blend::fillet_solver::curve_blend_sih::SmoothingSolver for IntersectorSolver<'_> {
    fn original_face_has_surface(&self, face: crate::topology::FaceId) -> Result<bool> {
        let target = self
            .brep
            .as_deref()
            .ok_or_else(|| invalid("no target B-rep for the rail faces"))?;
        Ok(target.face(face)?.surface.is_some())
    }

    fn setup_offset_radius(&mut self, tsect: &TsectPnt) -> Result<f64> {
        self.pre()?;
        let mut radius = 0.0;
        let r = self.ops.setup_offset_values(tsect, Some(&mut radius));
        self.synced(r).map(|()| radius)
    }

    fn base_natural_uv_domain(&self, rail: usize) -> Result<Extent2d> {
        Ok(offset_surface(&*self.ops, rail)?
            .base_surface()
            .natural_uv_domain_if_built())
    }

    fn rail_surface(&self, rail: usize) -> Result<Surface> {
        Ok(Surface::from(
            offset_surface(&*self.ops, rail)?.surface.clone(),
        ))
    }

    fn this_approx_tol_3d(&self) -> f64 {
        self.ops.solver().approx_tol_3d
    }
}

#[cfg(test)]
mod c7_tests {
    use super::*;
    use crate::intersect::surface_intersector::p1::IntersectionPointType;
    use crate::nurbs::b_spline_surface::BSplineSurface;

    fn bilinear(p: [[f64; 3]; 4]) -> Surface {
        let [a, b, c, d] = p.map(Vec3::from);
        Surface::from(BSplineSurface::create_bilinear_surface(a, b, c, d).unwrap())
    }

    #[test]
    fn point_intersection_runs_the_fillet_driver_and_copies_back() {
        let k = Kernel::default();
        let surfaces = [
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
        ];
        let d = Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0);
        let mut fi: TraceIntersector<'_> =
            FilletIntersector::new(&k, &surfaces[0], &d, &surfaces[1], &d, None).unwrap();
        let mut solver = FilletSolver::new(1.0e-5, 0.01, 0.01);
        let mut target = Brep::new();
        let mut fillets = FilletBrep::new();
        let mut cx = Cx {
            k: &k,
            target: &mut target,
            fillets: &mut fillets,
            solvers: GeometrySolvers::Indexed(&[]),
            error_info: None,
        };
        let mut points = vec![TsectPnt::new(), TsectPnt::new()];
        points[0].curve_parameter = 5.0;
        points[0].point_type = IntersectionPointType::Crossing;
        let args = PointIntersectionArgs {
            extend_before: false,
            extend_after: false,
            start_dir: Some(Vec3::new(1.0, 0.0, 0.0)),
            approx_tol_3d: 1.0e-3,
            ang_tol_rad: 0.1,
        };
        let e =
            do_point_intersection(&mut fi, &mut solver, &mut cx, &mut points, args).unwrap_err();
        // the base solver cannot evaluate the first point (no surfaces / chunk C6),
        // which the fillet ComputePointValues reports; no dispatch gap any more
        assert!(!matches!(
            e,
            SolidError::Unported {
                unit: "intersect.surface_intersector_p4",
                ..
            }
        ));
        // DoPointIntersection's writes to the first point came back
        assert_eq!(points[0].curve_parameter, -1.0);
        assert_eq!(points[0].point_type, IntersectionPointType::Unknown);
        assert_eq!(points[1].curve_parameter, 0.0);
        assert_eq!(fi.base.base.this_approx_tol_3d, 1.0e-3);
        assert_eq!(fi.base.base.this_ang_tol_rad, 0.1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_handles_write_back_to_their_storage() {
        let mut fillets = FilletBrep::new();
        let mut store = TsectStore::default();
        let a = store.alloc(TsectPnt::new());
        let b = store.alloc(TsectPnt::new());
        assert_eq!((a, b), (TsRef::Local(0), TsRef::Local(1)));
        let mut snap = store.snapshot(&fillets, &[b, a]).unwrap();
        snap[0].curve_parameter = 2.0;
        snap[1].curve_parameter = 1.0;
        store.write_back(&mut fillets, &[b, a], &snap).unwrap();
        assert_eq!(store.get(&fillets, a).unwrap().curve_parameter, 1.0);
        assert_eq!(store.get(&fillets, b).unwrap().curve_parameter, 2.0);
        assert!(store.get(&fillets, TsRef::Local(2)).is_err());
        assert!(store.write_back(&mut fillets, &[a], &snap).is_err());
    }
}
