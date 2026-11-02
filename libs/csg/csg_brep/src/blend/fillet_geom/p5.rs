//! Fillet geometry, part 5: the fillet/fillet intersection vertex
//! (`CalcFilletIntFillet`), the fillet-edge curve calculators (`CalcCrossSection`,
//! `CalcFilletIntSideFace`, `CalcEdgeOnExtendedEdge`), the pseudo-B-rep edge and rail
//! builders (`MakeFilletEdge`, `MakeRailEdge`, `MakeRailEdgeuse`, the copy
//! constructor of `SmFilletGeom`), the end cross-section curve (`CreateEndCurve`),
//! the rail trims (`TrimRailCurves`, `TrimCliffRails`) and the split-face update of a
//! fillet geometry (`UpdateSplitFace`).
//!
//! Upstream: `source/SMLib/src/SmFilletGeom.cpp`, `source/SMLib/inc/SmFilletGeom.h`.
//!
//! | Rust                                              | Upstream                                         |
//! |---------------------------------------------------|--------------------------------------------------|
//! | [`FilletBrep::calc_fillet_int_fillet`]            | `SmFilletVertex::CalcFilletIntFillet`            |
//! | [`FilletBrep::calc_cross_section`]                | `SmFilletEdge::CalcCrossSection`                 |
//! | [`FilletBrep::calc_fillet_int_side_face`]         | `SmFilletEdge::CalcFilletIntSideFace`            |
//! | [`FilletBrep::calc_edge_on_extended_edge`]        | `SmFilletEdge::CalcEdgeOnExtendedEdge`           |
//! | [`FilletBrep::make_fillet_edge`]                  | `SmFilletEdge::MakeFilletEdge`                   |
//! | [`FilletGeom::new`] (in `blend.fillet_geom_p1`)   | `SmFilletGeom(SmFilletSolver*, SmFilletGeomType)` (CO-PORTED there) |
//! | [`FilletBrep::copy_fillet_geom`]                  | `SmFilletGeom(SmFilletGeom*)`                    |
//! | [`FilletBrep::make_rail_edge`]                    | `SmFilletGeom::MakeRailEdge`                     |
//! | [`FilletBrep::make_rail_edgeuse`]                 | `SmFilletGeom::MakeRailEdgeuse`                  |
//! | [`FilletBrep::create_end_curve`], [`FilletBrep::create_end_curve_with_solver`] | `SmFilletGeom::CreateEndCurve`                   |
//! | [`FilletBrep::trim_rail_curves`], [`FilletBrep::trim_rail_curves_with_solver`] | `SmFilletGeom::TrimRailCurves`                   |
//! | [`FilletBrep::trim_cliff_rails`]                  | `SmFilletGeom::TrimCliffRails`                   |
//! | [`FilletBrep::update_geom_split_face`]            | `SmFilletGeom::UpdateSplitFace`                  |
//!
//! Deviations:
//! - **Object model.** As in `blend.fillet_geom_p1` / `_p4`, a fillet vertex, edge or
//!   geometry is an id of the [`FilletBrep`] (`self`, upstream's pseudo B-rep
//!   `pPseudoBrep`). `GetFilletSolver()` is an opaque [`FilletSolverId`] resolved in
//!   the caller's `solvers` table (index `id.0`). `trim_rail_curves_with_solver`
//!   uses its caller's single live solver at the actual id; both entries share the
//!   normal and cliff trimming bodies. An id not in the table is
//!   [`SolidError::InvalidInput`], a geometry without a solver `SM_ERR_NULL_POINTER`
//!   (upstream dereferences `NULL`). The target B-rep the original faces, edges and
//!   the solvers' edge uses refer to is `target`. The `SmContext` of a corner or
//!   solver is the [`Kernel`].
//! - **Fillet corners.** A vertex's or edge's `SmFilletCorner*` is a [`FilletCornerId`]
//!   that the caller's [`CornerLookup`] resolves to the [`FilletCorner`] (unit
//!   `blend.fillet_corner_p1`) whose corner type, flags, filleted vertex and tolerance
//!   the calculators read; the lookup also holds the executive's extended surfaces
//!   (`GetExtendedSurface`). The corner's error log (`RecordFilletError`, unit
//!   `blend.fillet_corner_p2`) records into the caller's [`ErrorInfo`] when supplied
//!   (a standalone caller without one notes into a scratch record). A `NULL` corner that upstream
//!   dereferences (`CalcFilletIntSideFace`, `MakeRailEdgeuse`) is
//!   `SM_ERR_NULL_POINTER`.
//! - **Topology links.** `SmOwningTopology::PreInsert` / `PostInsert` of a vertex use
//!   into a vertex and of an edge use into an edge are applied to the arena records
//!   directly (the use's back pointer and the owner's ordered list: `PreInsert`
//!   prepends, `PostInsert` appends), as the pseudo-B-rep model of
//!   `blend.fillet_geom_p1` links them; the generic list of
//!   `topology.proto_topology_p1` works on its own record store, not on [`Brep`]
//!   arenas. The edge uses come from [`FilletBrep::new_fillet_edgeuse`], which needs
//!   the pseudo B-rep's infinite region (created by [`FilletBrep::new`]) and its
//!   first shell (`MakeShellVertexTopology`, `topology.brep_p2`). A missing region
//!   returns `SM_ERR_NULL_POINTER`; an empty region receives a shell vertex through
//!   the real topology builder. Upstream's `SmObjDelete`
//!   clean-ups delete the new objects on a failure; the Rust arenas are append-only,
//!   so the records made before a failure stay, unreferenced by the edge list only
//!   when the edge was not made yet.
//! - **Curves.** Curves are values: upstream's pointer juggling (an edge use's UV curve
//!   released with `SetUVCurve(NULL, FALSE)` and set again later, the rail's original
//!   curve kept while its edge holds `NULL`) holds clones here. `SetCurve` on a fillet
//!   edge applies the `SM_NO_CHANGE_GEOMETRY` resets directly (as `blend.fillet_geom_p1`
//!   does). `SM_CAST_PTR(SmBSplineCurve, ..)` succeeds for the kinds derived from
//!   `SmBSplineCurve`; a curve of another kind is `NULL` there. The curve trims run
//!   without the owner notification (as `blend.fillet_geom_p1`'s `Trim`).
//! - **Statuses.** `SER`/`NER` failures are errors; statuses upstream ignores (`SE`,
//!   the `DropPoint` status in `TrimRailCurves`, `ConvertTo2D`, `SetMinMax`) are
//!   ignored, except that [`SolidError::Unported`] always propagates. A failed
//!   `DropPoint` counts as no drop point.
//! - **Surface/surface intersection.** `sm_SrfSrfIntersection` (`blend.fillet_geom_p2`)
//!   runs through `blend.fillet_geom_p4`'s adapter: the fillet vertices go in as their
//!   points and the curves retain their actual B-spline-derived objects through p4.
//! - **Curve cache.** Upstream keeps one curve cache on the rail across the up to nine
//!   `DropPoint` calls and the `GlobalPointSolve` of `TrimRailCurves`
//!   (SmFilletGeom.cpp:10580-10645); [`Curve::drop_point`] and the solve here each build
//!   their own. The results are bit-identical in every difftest case; a future
//!   cache-dependent disagreement should look here first.
//! - **Unported callees.** `SmFilletExecutive::GetPseudoBrep`
//!   (`blend.fillet_executive_p1`); the corner members above. `FindEdgeTangentAtPoint`
//!   and `SmFilletGeom::Copy` are part 6's (`blend.fillet_geom_p6`). Each stub replaces only
//!   its call.
//! - **User pointers.** `MakeRailEdgeuse` stores the two faces' surface pointers in the
//!   new vertex use's `SmTsectPnt::m_apUserPointer`
//!   ([`TsectPnt::user_pointer`](crate::intersect::surface_intersector::p1::TsectPnt::user_pointer),
//!   a shared copy of each face's surface; a face without surface stores `NULL`); a
//!   missing face is `SM_ERR_NULL_POINTER`.
//! - **Arguments.** Out-params become return values; `iDebugLevel` and debug drawing
//!   are dropped. No recursion.

use crate::blend::fillet_corner::p3::{ExecutiveErrorObject, FilletCorner};
use crate::blend::fillet_executive::p3::ErrorInfo;
use crate::blend::fillet_solver::p1::{FilletSolver, FilletSolverOps};
use crate::curve::curve::p1::{ApproximateCurveOptions, GlobalPointSolveOptions};
use crate::curve::curve::{Curve, CurveKind};
use crate::curve::hermite_curve::HermiteCurve;
use crate::error::{Result, SolidError};
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::global_solver::{SolutionRequested, SolverOperation};
use crate::geom::vector2d::{Point2, Vec2};
use crate::kernel::Kernel;
use crate::math::{Point3, Vec3, EFF_ZERO, PI};
use crate::nurbs::b_spline_curve::BSplineCurve;
use crate::nurbs::b_spline_curve_n_lib::TrimMode;
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::nurbs::knots::Side;
use crate::surface::plane::Plane;
use crate::surface::surface::p7::drop_vectors_at;
use crate::surface::surface::{Surface, SurfaceKind};
use crate::topology::edgeuse::p1::{self as eu1, primary_edgeuse};
use crate::topology::{
    edge, vertex, Brep, EdgeId, EdgeuseId, FaceId, Vertex, VertexId, Vertexuse, VertexuseId,
};

use super::p1::{
    clean_up_noise, BoundaryTrimmingType, CornerLookup, CurveObjectHandle, FilStatus, FilletBrep,
    FilletCornerId, FilletCornerType, FilletEdgeType, FilletErrorType, FilletGeom, FilletGeomId,
    FilletGeomType, FilletSolverId, FilletVertex, FilletVertexType, FilletVertexuse,
};
use super::p4::{srf_srf_intersection, SrfSrfCurves};

const OP_INT_FILLET: &str = "blend::fillet_geom::p5::FilletBrep::calc_fillet_int_fillet";
const OP_CROSS: &str = "blend::fillet_geom::p5::FilletBrep::calc_cross_section";
const OP_SIDE: &str = "blend::fillet_geom::p5::FilletBrep::calc_fillet_int_side_face";
const OP_EXT_EDGE: &str = "blend::fillet_geom::p5::FilletBrep::calc_edge_on_extended_edge";
const OP_MAKE_EDGE: &str = "blend::fillet_geom::p5::FilletBrep::make_fillet_edge";
const OP_COPY: &str = "blend::fillet_geom::p5::FilletBrep::copy_fillet_geom";
const OP_RAIL_EDGE: &str = "blend::fillet_geom::p5::FilletBrep::make_rail_edge";
const OP_RAIL_EU: &str = "blend::fillet_geom::p5::FilletBrep::make_rail_edgeuse";
const OP_END: &str = "blend::fillet_geom::p5::FilletBrep::create_end_curve";
const OP_TRIM: &str = "blend::fillet_geom::p5::FilletBrep::trim_rail_curves";
const OP_CLIFF: &str = "blend::fillet_geom::p5::FilletBrep::trim_cliff_rails";
const OP_SPLIT: &str = "blend::fillet_geom::p5::FilletBrep::update_geom_split_face";

// ---- helpers --------------------------------------------------------------------------

/// The global point solve options naming `curve` ([`CurveObjectHandle`]): the curve
/// itself (upstream `this`, id `0`) and the analytic replacement its curve cache may make
/// (id `1`). The replacement's handle carries the rail's own interval, which holds only
/// if the cache's analytic replacement keeps the B-spline's parameterisation (upstream
/// reads the replacement's own `GetNaturalInterval()`); the 300 generated cases agree
/// bit for bit under it.
fn rail_solve_options(curve: &Curve) -> GlobalPointSolveOptions<'static, CurveObjectHandle> {
    let ivl = curve.natural_interval();
    GlobalPointSolveOptions {
        handle: Some(CurveObjectHandle { id: 0, ivl }),
        analytic_handle: Some(CurveObjectHandle { id: 1, ivl }),
        ..GlobalPointSolveOptions::default()
    }
}

/// Ignores a status upstream does not check, but never an [`SolidError::Unported`].
fn ignore_non_unported<T>(r: Result<T>) -> Result<()> {
    match r {
        Err(e) if e.is_unported() => Err(e),
        _ => Ok(()),
    }
}

/// The solver of a fillet geometry in the caller's table (index `id.0`).
fn solver_of<'s>(
    solvers: &'s [FilletSolver],
    id: FilletSolverId,
    op: &'static str,
) -> Result<&'s FilletSolver> {
    usize::try_from(id.0)
        .ok()
        .and_then(|i| solvers.get(i))
        .ok_or_else(|| SolidError::invalid_input(op, "fillet solver id not in the solver table"))
}

/// `smos_Min`.
fn smin(a: f64, b: f64) -> f64 {
    if a < b {
        a
    } else {
        b
    }
}

/// `smos_Max`.
fn smax(a: f64, b: f64) -> f64 {
    if a > b {
        a
    } else {
        b
    }
}

/// `SmPoint3d(SmPoint2d)`: `z = 0`.
const fn p3(uv: Point2) -> Point3 {
    Point3::new(uv.x, uv.y, 0.0)
}

/// `SM_CAST_PTR(SmBSplineCurve, pCurve) != NULL`: the kinds derived from
/// `SmBSplineCurve` (`SmLine`, `SmEllipse`/`SmCircle`, `SmHyperbola`, `SmParabola`).
const fn is_b_spline(c: &Curve) -> bool {
    matches!(
        c.kind(),
        CurveKind::BSpline(_)
            | CurveKind::Line(_)
            | CurveKind::Ellipse(_)
            | CurveKind::Hyperbola(_)
            | CurveKind::Parabola(_)
    )
}

/// `SM_CAST_PTR(SmBSplineSurface, pSurf) != NULL`: a B-spline surface, a plane, a
/// surface of revolution, a cone, a sphere or a torus (as `blend.fillet_geom_p4`'s cast).
fn is_b_spline_surface(s: &Surface) -> bool {
    matches!(
        s.kind(),
        SurfaceKind::BSpline(_)
            | SurfaceKind::Plane(_)
            | SurfaceKind::SurfOfRevolution(_)
            | SurfaceKind::SurfOfExtrusion(_)
            | SurfaceKind::Cone(_)
            | SurfaceKind::Sphere(_)
            | SurfaceKind::Torus(_)
    )
}

/// `SmVertex::SetPoint` on a fillet vertex: the `SM_NO_CHANGE_GEOMETRY` notification
/// resets the cached gaps of every vertex use of the vertex, then the point is stored
/// (as `blend.fillet_geom_p1` does).
fn set_vertex_point(brep: &mut Brep, v: VertexId, point: Point3) -> Result<()> {
    for vu in brep.vertex(v)?.vertexuses.clone() {
        let vu = brep.vertexuse_mut(vu)?;
        vu.vertex_edge_gap.reset();
        vu.vertex_face_gap.reset();
    }
    brep.vertex_mut(v)?.point = point;
    Ok(())
}

/// `SmEdge::SetCurve(pCurve, ..)` on a fillet edge: the `SM_NO_CHANGE_GEOMETRY`
/// notification clears each edge use's UV trim curve and drop-curve record and resets
/// its cached edge/face gap (its mate's too) and the cached gaps of the edge's vertex
/// uses; then the curve (possibly `NULL`) replaces the old one (as
/// `blend.fillet_geom_p1` / `_p4` do). The delete and leak-warning flags only decide
/// whether upstream frees the old curve.
fn set_edge_curve(brep: &mut Brep, e: EdgeId, curve: Option<Curve>) -> Result<()> {
    for eu in edge::edgeuses(brep, e, None, None)? {
        let mate_eu = eu1::mate(brep, eu)?;
        let rec = brep.edgeuse_mut(eu)?;
        rec.uv_trim_curve = None;
        rec.drop_curve_fail = None;
        rec.max_edge_face_gap.reset();
        let vu = rec.vertexuse;
        if let Some(m) = mate_eu {
            brep.edgeuse_mut(m)?.max_edge_face_gap.reset();
        }
        if let Some(vu) = vu {
            let vu = brep.vertexuse_mut(vu)?;
            vu.vertex_edge_gap.reset();
            vu.vertex_face_gap.reset();
        }
    }
    brep.edge_mut(e)?.curve = curve;
    Ok(())
}

/// `SmVertex::PostInsert(pVertexuse)`: appends the use to the vertex's list and points
/// it at the vertex.
fn vertex_post_insert(brep: &mut Brep, v: VertexId, vu: VertexuseId) -> Result<()> {
    brep.vertex(v)?;
    brep.vertexuse_mut(vu)?.vertex = Some(v);
    brep.vertex_mut(v)?.vertexuses.push(vu);
    Ok(())
}

/// `SmEdge::PreInsert(pEdgeuse)` (`front`) / `PostInsert(pEdgeuse)`: puts the use at
/// the front / end of the edge's list and points it at the edge.
fn edge_insert(brep: &mut Brep, e: EdgeId, eu: EdgeuseId, front: bool) -> Result<()> {
    brep.edge(e)?;
    brep.edgeuse_mut(eu)?.edge = Some(e);
    let uses = &mut brep.edge_mut(e)?.edgeuses;
    if front {
        uses.insert(0, eu);
    } else {
        uses.push(eu);
    }
    Ok(())
}

/// `new (pPseudoBrep) SmFilletVertexuse()`: a vertex use record with its fillet
/// members.
fn new_fillet_vertexuse(fb: &mut FilletBrep) -> Result<VertexuseId> {
    let vu = fb.brep_mut().add_vertexuse_record(Vertexuse::default())?;
    fb.insert_fillet_vertexuse(vu, FilletVertexuse::new());
    Ok(vu)
}

/// `new (pPseudoBrep) SmFilletVertex()`: `SmVertex()` (point `(0, 0, 0)`, zone
/// tolerance `0`) with no corner.
fn new_fillet_vertex(k: &Kernel, fb: &mut FilletBrep) -> Result<VertexId> {
    let v = fb.brep_mut().add_vertex_record(Vertex {
        point: Point3::ZERO,
        tolerance: 0.0,
        ..Vertex::default()
    })?;
    fb.insert_fillet_vertex(v, FilletVertex::new(k, None));
    Ok(v)
}

/// The edge use's orientation (`SmEdgeuse::SetOrientation`).
fn set_eu_orientation(brep: &mut Brep, eu: EdgeuseId, o: OrientType) -> Result<()> {
    brep.edgeuse_mut(eu)?.orientation = o;
    Ok(())
}

/// The vertex of the vertex use of edge use `eu`.
fn eu_vertex(brep: &Brep, eu: EdgeuseId, op: &'static str) -> Result<VertexId> {
    let vu = brep
        .edgeuse(eu)?
        .vertexuse
        .ok_or_else(|| SolidError::null_pointer(op))?;
    brep.vertexuse(vu)?
        .vertex
        .ok_or_else(|| SolidError::null_pointer(op))
}

// ---- unported callees -------------------------------------------------------------

/// `SmFilletCorner::RecordFilletError(SM_FILERR_BAD_INT, pFilletSurface1, pSurface,
/// pStartFV, pEndFV, ..)` (unit `blend.fillet_corner_p2`) into the executive's
/// [`ErrorInfo`].
fn corner_record_fillet_error(
    corner: &FilletCorner,
    error_info: Option<&mut ErrorInfo<ExecutiveErrorObject>>,
    surface1: ExecutiveErrorObject,
    surface2: Option<ExecutiveErrorObject>,
    fv1: VertexId,
    fv2: VertexId,
) {
    // m_pExecutive->NoteFilletError: a caller without an executive record (a
    // standalone entry point) notes into a scratch record it does not keep.
    let mut scratch = ErrorInfo::default();
    let error_info = match error_info {
        Some(error_info) => error_info,
        None => &mut scratch,
    };
    corner.record_fillet_error_in(
        error_info,
        FilletErrorType::BadInt,
        corner
            .filleted_vertex()
            .map(ExecutiveErrorObject::TargetVertex),
        [Some(surface1), surface2],
        [
            Some(ExecutiveErrorObject::FilletVertex(fv1)),
            Some(ExecutiveErrorObject::FilletVertex(fv2)),
        ],
        Some(
            "SmFilletEdge::CalcFilletIntSideFace(): failure to calculate intersection of fillet surface with side face",
        ),
    );
}

/// `SmFilletEdge::UpdateSplitFace(pOrigFace, pNewFace)` (unit `blend.fillet_geom_p2`,
/// [`FilletBrep::update_edge_split_face`]); `SmFilletGeom::UpdateSplitFace` ignores
/// its status (SmFilletGeom.cpp:11865, 11877).
fn fillet_edge_update_split_face(
    fb: &mut FilletBrep,
    k: &Kernel,
    e: EdgeId,
    orig_face: Option<FaceId>,
    new_face: Option<FaceId>,
    target: &mut Brep,
    corners: &[FilletCorner],
) -> Result<()> {
    ignore_non_unported(fb.update_edge_split_face(k, e, orig_face, new_face, target, corners))
}

/// Actual solver records and extended surfaces used by a corner edge calculation.
pub trait FilletEdgeContext {
    /// Resolve the retained solver for this geometry's actual ID.
    ///
    /// # Errors
    /// Missing or foreign solver identities.
    fn solver(&self, id: FilletSolverId) -> Result<&FilletSolver>;

    /// Resolve the executive's actual extended surface for the original face.
    ///
    /// # Errors
    /// Missing executive context or actual surface-resolution errors.
    fn extended_surface(&self, corner: &FilletCorner, face: FaceId) -> Result<Option<&Surface>>;

    /// The concrete solvers `id1` and `id2` for a solve that changes them
    /// (`sm_CreateCubicRailRailInterpolant`'s `RailRailIntersect`), in that order.
    ///
    /// # Errors
    /// Missing or foreign solver identities; [`SolidError::InvalidInput`] when both
    /// name one solver or the context holds its solvers read-only.
    fn solver_pair_ops_mut(
        &mut self,
        id1: FilletSolverId,
        id2: FilletSolverId,
    ) -> Result<(&mut dyn FilletSolverOps, &mut dyn FilletSolverOps)>;
}

/// A caller's indexed solver table as a [`FilletEdgeContext`]; `op` names the
/// calculator in a missing-solver error.
pub(super) struct IndexedEdgeContext<'s, 'c> {
    pub(super) solvers: &'s [FilletSolver],
    pub(super) corners: CornerLookup<'c>,
    pub(super) op: &'static str,
}

impl FilletEdgeContext for IndexedEdgeContext<'_, '_> {
    fn solver(&self, id: FilletSolverId) -> Result<&FilletSolver> {
        solver_of(self.solvers, id, self.op)
    }

    fn extended_surface(&self, corner: &FilletCorner, face: FaceId) -> Result<Option<&Surface>> {
        self.corners.extended_surface(corner, face)
    }

    fn solver_pair_ops_mut(
        &mut self,
        _id1: FilletSolverId,
        _id2: FilletSolverId,
    ) -> Result<(&mut dyn FilletSolverOps, &mut dyn FilletSolverOps)> {
        Err(SolidError::invalid_input(
            self.op,
            "indexed edge context holds its solvers read-only",
        ))
    }
}

// ---- the routines -----------------------------------------------------------------

impl FilletBrep {
    /// The fillet geometry of edge use `eu` (`GetFilletGeom`).
    fn eu_geom(&self, eu: EdgeuseId) -> Result<Option<FilletGeomId>> {
        Ok(self.fillet_edgeuse(eu)?.fillet_geom())
    }

    /// The solver of geometry `g` (`GetFilletSolver()`, dereferenced).
    fn geom_solver<'s>(
        &self,
        g: FilletGeomId,
        solvers: &'s [FilletSolver],
        op: &'static str,
    ) -> Result<&'s FilletSolver> {
        let sid = self
            .geom(g)?
            .fillet_solver()
            .ok_or_else(|| SolidError::null_pointer(op))?;
        solver_of(solvers, sid, op)
    }

    /// The primary edge use of `e` and its mate.
    fn p5_prim_mate(&self, e: EdgeId, op: &'static str) -> Result<(EdgeuseId, EdgeuseId)> {
        let prim = primary_edgeuse(self.brep(), e)?.ok_or_else(|| SolidError::null_pointer(op))?;
        let mate = eu1::mate(self.brep(), prim)?.ok_or_else(|| SolidError::null_pointer(op))?;
        Ok((prim, mate))
    }

    /// `GetVertex()` and `GetOtherVertex(GetVertex())` of fillet edge `e`.
    fn edge_ends(&self, e: EdgeId, op: &'static str) -> Result<(VertexId, VertexId)> {
        let start = edge::vertex(self.brep(), e)?.ok_or_else(|| SolidError::null_pointer(op))?;
        let end = edge::other_vertex(self.brep(), e, Some(start))?
            .ok_or_else(|| SolidError::null_pointer(op))?;
        Ok((start, end))
    }

    /// `pEdgeuse->SetUVCurve(pCurve)` with its defaults.
    fn set_uv(&mut self, eu: EdgeuseId, curve: Option<Curve>) -> Result<()> {
        self.set_edgeuse_uv_curve(eu, curve, false, true)
    }

    /// `SmFilletVertex::CalcFilletIntFillet`: the vertex at the end of the
    /// fillet/fillet intersection curve leaving `v`, whose other end is already
    /// processed. An Nx2 corner that is not a bevel corner goes to
    /// [`FilletBrep::calc_nx2_vert_fillet_x_fillet`]. Without the intersection curve on
    /// the fillet/fillet edge, the two fillet surfaces are intersected first (nothing
    /// happens while either surface is missing); the curve and its UV curves are
    /// reversed when needed so that the edge runs from its start vertex, and the vertex
    /// gets the curve end away from the other vertex.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` without corner, fillet/fillet edge, edge uses, fillet
    /// geometries or solvers; `SM_ERR` when the other vertex is not processed or the
    /// intersection fails (vertex and edge get `SM_FIL_SURF_INT_FAILURE`); those of the
    /// evaluations and reversals; those of [`CornerLookup::corner`] (`corners` resolves the corner);
    /// [`SolidError::Unported`] for the intersection (`sm_SrfSrfIntersection`).
    pub fn calc_fillet_int_fillet(
        &mut self,
        k: &Kernel,
        v: VertexId,
        corners: CornerLookup<'_>,
        solvers: &[FilletSolver],
        target: &Brep,
    ) -> Result<()> {
        self.calc_fillet_int_fillet_with_context(
            k,
            v,
            corners,
            &IndexedEdgeContext {
                solvers,
                corners,
                op: OP_INT_FILLET,
            },
            target,
        )
    }

    /// [`Self::calc_fillet_int_fillet`] resolving the geometries' solvers through
    /// `context` (a live executive's retained solvers).
    ///
    /// # Errors
    /// As [`Self::calc_fillet_int_fillet`], with `context`'s resolution failures.
    #[allow(clippy::too_many_lines)] // one upstream routine
    pub fn calc_fillet_int_fillet_with_context(
        &mut self,
        k: &Kernel,
        v: VertexId,
        corners: CornerLookup<'_>,
        context: &impl FilletEdgeContext,
        target: &Brep,
    ) -> Result<()> {
        // check state
        let corner = self
            .fillet_vertex(v)?
            .fillet_corner()
            .ok_or_else(|| SolidError::null_pointer(OP_INT_FILLET))?;

        // First, check if we have an adjusted Nx2 Concave corner
        let the_corner = corners.corner(corner)?;
        if the_corner.corner_type() == FilletCornerType::NByTwo && !the_corner.is_bevel_corner() {
            self.calc_nx2_vert_fillet_x_fillet_with_context(k, v, context)?;
            return Ok(());
        }

        // get CornerFillet->OrigVertex
        let filleted_vertex = the_corner.filleted_vertex();

        // find the filletEdge defined by the filletSurface/filletSurface intersection
        let mut curr_edge = None;
        for e in vertex::edges(self.brep(), v, None)? {
            if self.fillet_edge(e)?.fillet_edge_type() == FilletEdgeType::FilletXFillet {
                curr_edge = Some(e);
                break;
            }
        }
        let curr_edge = curr_edge.ok_or_else(|| SolidError::null_pointer(OP_INT_FILLET))?;

        // get filletFVertex->Point at other end of intersection curve
        let other_fv = edge::other_vertex(self.brep(), curr_edge, Some(v))?
            .ok_or_else(|| SolidError::null_pointer(OP_INT_FILLET))?;
        if !self.fillet_vertex(other_fv)?.is_processed() {
            return Err(SolidError::sm_err(OP_INT_FILLET));
        }
        let other_pnt = self.vertex_point(other_fv)?;

        // locals from intersection FilletEdge
        let (prim, mate) = self.p5_prim_mate(curr_edge, OP_INT_FILLET)?;
        let has_curve = edge::curve(self.brep(), curr_edge)?.is_some_and(is_b_spline);

        // when intersection FilletEdge->Curve has not yet been computed
        if !has_curve {
            let g1 = self
                .eu_geom(prim)?
                .ok_or_else(|| SolidError::null_pointer(OP_INT_FILLET))?;
            let g2 = self
                .eu_geom(mate)?
                .ok_or_else(|| SolidError::null_pointer(OP_INT_FILLET))?;
            let sid1 = self.geom(g1)?.fillet_solver();
            let sid2 = self.geom(g2)?.fillet_solver();

            // check state - filletSurface must have been computed by now
            let (Some(surface1), Some(surface2)) = (
                self.geom(g1)?.fillet_surface(None)?.cloned(),
                self.geom(g2)?.fillet_surface(None)?.cloned(),
            ) else {
                return Ok(()); // Fillet surfaces have not yet been computed
            };
            let fs1 =
                context.solver(sid1.ok_or_else(|| SolidError::null_pointer(OP_INT_FILLET))?)?;
            let fs2 =
                context.solver(sid2.ok_or_else(|| SolidError::null_pointer(OP_INT_FILLET))?)?;

            // pick appoxTol and angTol as tightest filletSolver tolerances
            let approx_tol = smin(fs1.approx_tol_3d, fs2.approx_tol_3d);
            let ang_tol = smin(fs1.ang_tol_rad, fs2.ang_tol_rad);

            // set sDir = fillet chord (vector connecting intersection endPoints)
            let fv = filleted_vertex.ok_or_else(|| SolidError::null_pointer(OP_INT_FILLET))?;
            let mut dir = target.vertex(fv)?.point - other_pnt;

            // find the surface/surface intersection curve knowing only 1 end-point
            let result = srf_srf_intersection(
                k,
                self,
                Some(other_fv),
                None,
                Some(&mut dir),
                None,
                &surface1,
                &surface2,
                approx_tol,
                ang_tol,
                true,
                None,
            );
            let curves = match result {
                Err(e) if e.is_unported() => return Err(e),
                Ok(SrfSrfCurves {
                    curve_3d: Some(c),
                    uv_curve1,
                    uv_curve2,
                }) => (c, uv_curve1, uv_curve2),
                _ => {
                    // error: sm_SrfSrfIntersection() failed or failed to produce a p3DCurve
                    self.fillet_vertex_mut(v)?
                        .set_status(FilStatus::SurfIntFailure);
                    self.fillet_edge_mut(curr_edge)?
                        .set_status(FilStatus::SurfIntFailure);
                    return Err(SolidError::sm_err(OP_INT_FILLET));
                }
            };

            // save outputs: Curve, Edgeuses->UVCurves, Status = processed
            let (curve, uv1, uv2) = curves;
            let ivl = curve.natural_interval();
            set_edge_curve(self.brep_mut(), curr_edge, Some(curve))?;
            edge::set_interval(self.brep_mut(), curr_edge, &ivl)?;
            self.set_uv(prim, uv1)?;
            self.set_uv(mate, uv2)?;
            self.fillet_edge_mut(curr_edge)?
                .set_status(FilStatus::Processed);
        }

        // get intersection curve endPoints
        let curve = edge::curve(self.brep(), curr_edge)?
            .ok_or_else(|| SolidError::null_pointer(OP_INT_FILLET))?;
        let ivl = curve.natural_interval();
        let pnt1 = curve.evaluate_point(k, ivl.min())?;
        let pnt2 = curve.evaluate_point(k, ivl.max())?;

        // use xSect point closest to this vertex to set vertex position and 3dCurve
        // direction
        let other_closer_to_pnt1 = other_pnt.distance(pnt1) < other_pnt.distance(pnt2);
        let vert_geom = if other_closer_to_pnt1 { pnt2 } else { pnt1 };
        let edge_start = edge::vertex(self.brep(), curr_edge)?;
        let reverse = (edge_start == Some(v) && other_closer_to_pnt1)
            || (edge_start != Some(v) && !other_closer_to_pnt1);

        // when 3dCurve needs to be reversed
        if reverse {
            // reverse it and its uvTrimCurves
            let k2 = k;
            let brep = self.brep_mut();
            brep.edge_mut(curr_edge)?
                .curve
                .as_mut()
                .ok_or_else(|| SolidError::null_pointer(OP_INT_FILLET))?
                .reverse_parameterization(k2, &ivl)?;
            for eu in [prim, mate] {
                brep.edgeuse_mut(eu)?
                    .uv_trim_curve
                    .as_mut()
                    .ok_or_else(|| SolidError::null_pointer(OP_INT_FILLET))?
                    .reverse_parameterization(k2, &ivl)?;
            }
        }

        // set this filletVertex point and status values
        set_vertex_point(self.brep_mut(), v, vert_geom)?;
        self.fillet_vertex_mut(v)?.set_status(FilStatus::Processed);
        Ok(())
    }

    /// `SmFilletEdge::CalcCrossSection(crContext, pCorner)`: the cross-section curve of
    /// fillet edge `e` on the fillet surface of its primary edge use's geometry (or on
    /// its original face's surface without one), joined between its two vertices by
    /// [`create_uv_curve`](super::p4::create_uv_curve) and lifted when only a UV curve
    /// results; the side curve on the mate's other fillet geometry is made the same
    /// way. Adjacent fillet geometries of an Nx2 or closed blending corner are first
    /// matched along their common edge (`sm_ExamineNFixFilletGeoms`).
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` for missing uses, surfaces, solvers, original face or
    /// curves; those of the UV curve creation and the lift; those of [`CornerLookup::corner`] (`corners` resolves the corner);
    /// [`SolidError::Unported`] for the callees' gaps.
    #[allow(clippy::too_many_lines)] // one upstream routine
    pub fn calc_cross_section(
        &mut self,
        k: &Kernel,
        e: EdgeId,
        corner: Option<FilletCornerId>,
        corners: CornerLookup<'_>,
        solvers: &[FilletSolver],
        target: &Brep,
    ) -> Result<()> {
        self.calc_cross_section_with_lookup(
            k,
            e,
            corner,
            corners,
            &|id| solver_of(solvers, id, OP_CROSS),
            target,
        )
    }

    /// Calculate a cross-section using borrowed records from the actual solver store.
    ///
    /// # Errors
    /// As [`Self::calc_cross_section`], including the caller's solver lookup errors.
    pub fn calc_cross_section_with_lookup<'solver>(
        &mut self,
        k: &Kernel,
        e: EdgeId,
        corner: Option<FilletCornerId>,
        corners: CornerLookup<'_>,
        resolve: &impl Fn(FilletSolverId) -> Result<&'solver FilletSolver>,
        target: &Brep,
    ) -> Result<()> {
        let (start_fv, end_fv) = self.edge_ends(e, OP_CROSS)?;
        let (prim, mate) = self.p5_prim_mate(e, OP_CROSS)?;
        let g1 = self.eu_geom(prim)?;
        let g2 = self.eu_geom(mate)?;
        let mut fs2_tol: Option<f64> = None;

        let mut surface1: Surface;
        let mut surface2: Option<Surface> = None;
        let tol: f64;
        if let Some(g1) = g1 {
            let sid1 = self.geom(g1)?.fillet_solver();
            surface1 = self
                .geom(g1)?
                .fillet_surface(None)?
                .cloned()
                .ok_or_else(|| SolidError::null_pointer(OP_CROSS))?;

            if let Some(g2) = g2.filter(|&g2| g2 != g1) {
                let fs2 = resolve(
                    self.geom(g2)?
                        .fillet_solver()
                        .ok_or_else(|| SolidError::null_pointer(OP_CROSS))?,
                )?;
                fs2_tol = Some(fs2.approx_tol_3d);
                surface2 = Some(
                    self.geom(g2)?
                        .fillet_surface(None)?
                        .cloned()
                        .ok_or_else(|| SolidError::null_pointer(OP_CROSS))?,
                );
            }
            let fs1 = resolve(sid1.ok_or_else(|| SolidError::null_pointer(OP_CROSS))?)?;
            tol = fs1.approx_tol_3d;

            // Examine the connectivity between two adjacent fillet geoms.
            if let (Some(corner), Some(g2)) = (corner, g2) {
                if self.geom(g1)?.fillet_geom_type() != FilletGeomType::CliffRollover
                    && self.geom(g2)?.fillet_geom_type() != FilletGeomType::CliffRollover
                {
                    let mut at_start_of_geom1 = true;
                    let mut at_start_of_geom2 = false;
                    let mut check_connectivity = false;
                    let the_corner = corners.corner(corner)?;
                    let filleted_vertex = the_corner.filleted_vertex();
                    match the_corner.corner_type() {
                        FilletCornerType::NByTwo => {
                            at_start_of_geom1 = fs1.vertex(target, 0)? == filleted_vertex;
                            // pFilSolver2 stays NULL when both uses share the geometry
                            if g2 == g1 {
                                return Err(SolidError::null_pointer(OP_CROSS));
                            }
                            let fs2 = resolve(
                                self.geom(g2)?
                                    .fillet_solver()
                                    .ok_or_else(|| SolidError::null_pointer(OP_CROSS))?,
                            )?;
                            at_start_of_geom2 = fs2.vertex(target, 0)? == filleted_vertex;
                            check_connectivity = true;
                        }
                        FilletCornerType::NByOneClosed if the_corner.is_blending_corner() => {
                            check_connectivity = true;
                        }
                        _ => {}
                    }

                    if check_connectivity {
                        // The status is ignored upstream (no `SER`). Each geometry's
                        // `GetFilletSolver()` is resolved here for the adjusting path.
                        let solver_of = |g: FilletGeomId| -> Result<Option<&FilletSolver>> {
                            self.geom(g)?.fillet_solver().map(resolve).transpose()
                        };
                        let solvers = [solver_of(g1)?, solver_of(g2)?];
                        ignore_non_unported(super::p1::examine_n_fix_fillet_geoms_with_solvers(
                            k,
                            self,
                            g1,
                            at_start_of_geom1,
                            g2,
                            at_start_of_geom2,
                            start_fv,
                            end_fv,
                            tol,
                            solvers,
                        ))?;

                        // Upstream's `pSurface1` / `pSurface2` point at the fillet
                        // geometries' own surfaces, which the fix edits in place (a
                        // closed corner's end row becomes its start row); re-read them.
                        surface1 = self
                            .geom(g1)?
                            .fillet_surface(None)?
                            .cloned()
                            .ok_or_else(|| SolidError::null_pointer(OP_CROSS))?;
                        if surface2.is_some() {
                            surface2 = Some(
                                self.geom(g2)?
                                    .fillet_surface(None)?
                                    .cloned()
                                    .ok_or_else(|| SolidError::null_pointer(OP_CROSS))?,
                            );
                        }
                    }
                }
            }
        } else {
            let orig_face = self
                .fillet_edge(e)?
                .original_face()
                .ok_or_else(|| SolidError::null_pointer(OP_CROSS))?;
            let face = target.face(orig_face)?;
            surface1 = face
                .surface
                .clone()
                .ok_or_else(|| SolidError::null_pointer(OP_CROSS))?;
            tol = crate::topology::face::p2::tolerance(target, orig_face)?;
        }

        // Drop start & end points onto pFilletSurface1, then join them with a uv-curve
        let mut create_two_boundary_curves = false;
        let closed_blending = match corner {
            Some(c) => {
                let c = corners.corner(c)?;
                c.corner_type() == FilletCornerType::NByOneClosed
                    && g1 == g2
                    && c.is_blending_corner()
            }
            None => false,
        };
        if closed_blending {
            create_two_boundary_curves = true;
        }

        let uv = super::p4::create_uv_curve(
            k,
            self,
            start_fv,
            end_fv,
            create_two_boundary_curves,
            &surface1,
            50.0 * tol,
        )?;
        let mut curve_3d: Option<Curve> = uv.curve_3d;
        let uv_curve1 = uv.uv_curve1;
        let uv_curve2 = uv.uv_curve2;

        if curve_3d.is_none() {
            // Lift uv curve. `sm_CreateUVCurve` can succeed with neither curve, and
            // upstream then dereferences `*pUVCurve1` (UB-213).
            let uv1 = uv_curve1
                .clone()
                .ok_or_else(|| SolidError::null_pointer(OP_CROSS))?;
            let lifted = surface1.lift_curve(
                k,
                &surface1.natural_uv_domain_with_nurb(k)?,
                &uv1,
                &uv1.natural_interval(),
                tol / 10.0,
                false,
            )?;
            curve_3d = Some(lifted.curve);
        }
        let curve_3d = curve_3d.ok_or_else(|| SolidError::null_pointer(OP_CROSS))?;

        set_edge_curve(self.brep_mut(), e, Some(curve_3d))?; // side effect: delete current this->UVTrimCurves
        self.set_uv(prim, uv_curve1)?;

        // Need second UV curve when we had a closed fillet surface
        let closed = match corner {
            Some(c) => {
                corners.corner(c)?.corner_type() == FilletCornerType::NByOneClosed && g1 == g2
            }
            None => false,
        };
        if closed {
            let uv2 = uv_curve2.ok_or_else(|| SolidError::null_pointer(OP_CROSS))?;
            self.set_uv(mate, Some(Curve::from(uv2)))?;
            let g1 = g1.ok_or_else(|| SolidError::null_pointer(OP_CROSS))?;
            self.geom_mut(g1)?.add_side_fillet_edgeuse(mate);
        }

        if let Some(g1) = g1 {
            self.geom_mut(g1)?.add_side_fillet_edgeuse(prim);
        }

        // See if we need to add side curve for the other solver
        if let Some(surface2) = surface2 {
            let tol = fs2_tol.ok_or_else(|| SolidError::null_pointer(OP_CROSS))?;
            let uv = super::p4::create_uv_curve(
                k,
                self,
                start_fv,
                end_fv,
                false,
                &surface2,
                50.0 * tol,
            )?;
            // the duplicate 3D curve and an unexpected second UV curve are deleted
            self.set_uv(mate, uv.uv_curve1)?;
            let g2 = g2.ok_or_else(|| SolidError::null_pointer(OP_CROSS))?;
            self.geom_mut(g2)?.add_side_fillet_edgeuse(mate);
        }

        Ok(())
    }

    /// `SmFilletEdge::CalcFilletIntSideFace(crContext, pCorner)`: the side curve of
    /// fillet edge `e` between its two processed vertices, the intersection of the
    /// fillet surface with the end face (the corner's extended side surface, or the
    /// plane through the corner's vertex and the edge's end points) when the edge has
    /// no curve yet; the primary edge use joins the fillet geometry's side uses.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when a vertex is not processed or the intersection fails (the edge gets
    /// `SM_FIL_SURF_INT_FAILURE`); `SM_ERR_NULL_POINTER` without uses, fillet
    /// geometry, solver, surface or corner; those of [`CornerLookup::corner`] (`corners` resolves the corner) and of
    /// [`CornerLookup::extended_surface`]; [`SolidError::Unported`] from the
    /// intersection (`sm_SrfSrfIntersection`).
    pub fn calc_fillet_int_side_face(
        &mut self,
        k: &Kernel,
        e: EdgeId,
        corner: Option<FilletCornerId>,
        corners: CornerLookup<'_>,
        solvers: &[FilletSolver],
        target: &Brep,
    ) -> Result<()> {
        self.calc_fillet_int_side_face_with_error_info(k, e, corner, corners, solvers, target, None)
    }

    /// [`Self::calc_fillet_int_side_face`] with an optional [`ErrorInfo`] sink for
    /// `SmFilletCorner::RecordFilletError`.
    ///
    /// # Errors
    /// As [`Self::calc_fillet_int_side_face`].
    #[allow(clippy::too_many_arguments)]
    pub fn calc_fillet_int_side_face_with_error_info(
        &mut self,
        k: &Kernel,
        e: EdgeId,
        corner: Option<FilletCornerId>,
        corners: CornerLookup<'_>,
        solvers: &[FilletSolver],
        target: &Brep,
        error_info: Option<&mut ErrorInfo<ExecutiveErrorObject>>,
    ) -> Result<()> {
        self.calc_fillet_int_side_face_with_context_and_error_info(
            k,
            e,
            corner,
            corners,
            &IndexedEdgeContext {
                solvers,
                corners,
                op: OP_SIDE,
            },
            target,
            error_info,
        )
    }

    /// Calculate a side-face end rib using actual borrowed solver/surface context.
    ///
    /// # Errors
    /// As [`Self::calc_fillet_int_side_face`], including actual context failures.
    pub fn calc_fillet_int_side_face_with_context(
        &mut self,
        k: &Kernel,
        e: EdgeId,
        corner: Option<FilletCornerId>,
        corners: CornerLookup<'_>,
        context: &impl FilletEdgeContext,
        target: &Brep,
    ) -> Result<()> {
        self.calc_fillet_int_side_face_with_context_and_error_info(
            k, e, corner, corners, context, target, None,
        )
    }

    /// [`Self::calc_fillet_int_side_face_with_context`] with an optional [`ErrorInfo`]
    /// sink for `SmFilletCorner::RecordFilletError`.
    ///
    /// # Errors
    /// As [`Self::calc_fillet_int_side_face_with_context`].
    #[allow(clippy::too_many_arguments)]
    pub fn calc_fillet_int_side_face_with_context_and_error_info(
        &mut self,
        k: &Kernel,
        e: EdgeId,
        corner: Option<FilletCornerId>,
        corners: CornerLookup<'_>,
        context: &impl FilletEdgeContext,
        target: &Brep,
        error_info: Option<&mut ErrorInfo<ExecutiveErrorObject>>,
    ) -> Result<()> {
        // get filletEdge->vertices
        let (start_fv, end_fv) = self.edge_ends(e, OP_SIDE)?;

        // check state - both vertices must be processed
        if !self.fillet_vertex(start_fv)?.is_processed()
            || !self.fillet_vertex(end_fv)?.is_processed()
        {
            return Err(SolidError::sm_err(OP_SIDE));
        }

        // get vertex endPoints
        let start_pnt = self.vertex_point(start_fv)?;
        let end_pnt = self.vertex_point(end_fv)?;

        // locals: PrimEU connected to FilletSurface, MateEU connected to endFace
        let (prim, mate) = self.p5_prim_mate(e, OP_SIDE)?;
        let g1 = self
            .eu_geom(prim)?
            .ok_or_else(|| SolidError::null_pointer(OP_SIDE))?;
        let fs1 = context.solver(
            self.geom(g1)?
                .fillet_solver()
                .ok_or_else(|| SolidError::null_pointer(OP_SIDE))?,
        )?;
        let has_curve = edge::curve(self.brep(), e)?.is_some_and(is_b_spline);

        // get tolerance
        let approx_tol = fs1.approx_tol_3d;
        let mut new_curves: Option<SrfSrfCurves> = None;

        // when filletEdge does not yet have a 3DCurve
        if !has_curve {
            let fillet_surface1 = self
                .geom(g1)?
                .fillet_surface(None)?
                .cloned()
                .ok_or_else(|| SolidError::null_pointer(OP_SIDE))?;
            // upstream calls `pCorner->IsExtendedSurfacePatch()` on the missing corner
            // (UB-214: a null dereference, tested only four lines later)
            let corner = corner.ok_or_else(|| SolidError::null_pointer(OP_SIDE))?;

            // when endFace->Surface is defined by sideFace->ExtendedSurface; the face's
            // surface may be missing (`pFace->GetSurface()` has no `NER`): upstream hands
            // the `NULL` on to `sm_SrfSrfIntersection`, whose `NER(pSurf2)` takes the
            // failure branch below
            let the_corner = corners.corner(corner)?;
            let (surface, surface2_id): (Option<Surface>, Option<ExecutiveErrorObject>) =
                if the_corner.is_extended_surface_patch() {
                    let face = self
                        .fillet_edge(e)?
                        .original_face()
                        .ok_or_else(|| SolidError::null_pointer(OP_SIDE))?;
                    if let Some(s) = context.extended_surface(the_corner, face)? {
                        (
                            Some(s.clone()),
                            Some(ExecutiveErrorObject::TargetFaceSurface {
                                face,
                                extended: true,
                            }),
                        )
                    } else {
                        let orig = target.face(face)?.surface.clone();
                        let id = orig
                            .as_ref()
                            .map(|_| ExecutiveErrorObject::TargetFaceSurface {
                                face,
                                extended: false,
                            });
                        (orig, id)
                    }
                } else {
                    // endFace->Surface = plane through corner->OriginalVertex,
                    // edge->EndPoints
                    let fv = the_corner
                        .filleted_vertex()
                        .ok_or_else(|| SolidError::null_pointer(OP_SIDE))?;
                    let origin = target.vertex(fv)?.point;
                    let mut x_vector = self.vertex_point(start_fv)? - origin;
                    let mut y_vector = self.vertex_point(end_fv)? - origin;
                    let x_len = x_vector.length();
                    let y_len = y_vector.length();
                    let uv_scale = Vec2::new(1.0, 1.0);
                    x_vector /= x_len;
                    y_vector /= y_len;
                    y_vector = x_vector.cross(y_vector).cross(x_vector);
                    y_vector = y_vector.unitize_or_z();
                    let uv_domain = Extent2d::from_bounds(-x_len, -y_len, 2.0 * x_len, 2.0 * y_len);
                    let mut plane = Plane::new(origin, x_vector, y_vector, uv_scale, &uv_domain);
                    ignore_non_unported(plane.make_nurb())?;
                    (
                        Some(Surface::from(plane)),
                        Some(ExecutiveErrorObject::TempEndPlane(e)),
                    )
                };

            // do Surface/surface xSect
            let ang_tol = fs1.ang_tol_rad;
            let mut dir = end_pnt - start_pnt;
            let intersection = match &surface {
                Some(surface) => srf_srf_intersection(
                    k,
                    self,
                    Some(start_fv),
                    Some(end_fv),
                    Some(&mut dir),
                    None,
                    &fillet_surface1,
                    surface,
                    approx_tol,
                    ang_tol,
                    false,
                    None,
                ),
                // NER(pSurf2)
                None => Err(SolidError::null_pointer(OP_SIDE)),
            };
            match intersection {
                Err(err) if err.is_unported() => return Err(err),
                Ok(c) if c.curve_3d.is_some() => new_curves = Some(c),
                _ => {
                    self.fillet_edge_mut(e)?
                        .set_status(FilStatus::SurfIntFailure);
                    corner_record_fillet_error(
                        the_corner,
                        error_info,
                        ExecutiveErrorObject::FilletGeomSurface(g1),
                        surface2_id,
                        start_fv,
                        end_fv,
                    );
                    return Err(SolidError::sm_err(OP_SIDE));
                }
            }
        }

        // when a new 3DCurve was constructed by surf/surf intersection
        if let Some(c) = new_curves {
            // save 3DCurve and UVTrimCurves, and set the owner
            set_edge_curve(self.brep_mut(), e, c.curve_3d)?;
            self.set_uv(prim, c.uv_curve1)?;
            self.set_uv(mate, c.uv_curve2)?;
        }

        // connect the sideFilletEdge to the FilletSurfaceFace
        self.geom_mut(g1)?.add_side_fillet_edgeuse(prim);
        Ok(())
    }

    /// `SmFilletEdge::CalcEdgeOnExtendedEdge(crContext, pOptCorner)`: the curve of
    /// fillet edge `e` along its extended original edge, the intersection of the
    /// (extended) surfaces of the original edge's first two faces between the edge's
    /// processed vertices.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` without corner or original edge, or for a surface that is
    /// not an `SmBSplineSurface`; `SM_ERR` when a vertex is not processed or the
    /// original edge has fewer than two faces; those of [`CornerLookup::corner`] (`corners` resolves the corner) and of
    /// [`CornerLookup::extended_surface`]; [`SolidError::Unported`] for the
    /// intersection (`sm_SrfSrfIntersection`).
    pub fn calc_edge_on_extended_edge(
        &mut self,
        k: &Kernel,
        e: EdgeId,
        corner: Option<FilletCornerId>,
        corners: CornerLookup<'_>,
        target: &Brep,
    ) -> Result<()> {
        self.calc_edge_on_extended_edge_with_surfaces(k, e, corner, corners, target, |c, f| {
            Ok(corners.extended_surface(c, f)?.cloned())
        })
    }

    /// [`Self::calc_edge_on_extended_edge`] resolving the extended surfaces through the
    /// caller's live executive objects.
    ///
    /// # Errors
    /// As [`Self::calc_edge_on_extended_edge`]; those of `context`.
    pub fn calc_edge_on_extended_edge_with_context(
        &mut self,
        k: &Kernel,
        e: EdgeId,
        corner: Option<FilletCornerId>,
        corners: CornerLookup<'_>,
        context: &impl FilletEdgeContext,
        target: &Brep,
    ) -> Result<()> {
        self.calc_edge_on_extended_edge_with_surfaces(k, e, corner, corners, target, |c, f| {
            Ok(context.extended_surface(c, f)?.cloned())
        })
    }

    fn calc_edge_on_extended_edge_with_surfaces(
        &mut self,
        k: &Kernel,
        e: EdgeId,
        corner: Option<FilletCornerId>,
        corners: CornerLookup<'_>,
        target: &Brep,
        extended_surface: impl Fn(&FilletCorner, FaceId) -> Result<Option<Surface>>,
    ) -> Result<()> {
        let corner = corner.ok_or_else(|| SolidError::null_pointer(OP_EXT_EDGE))?;
        let (start_fv, end_fv) = self.edge_ends(e, OP_EXT_EDGE)?;
        if !self.fillet_vertex(start_fv)?.is_processed()
            || !self.fillet_vertex(end_fv)?.is_processed()
        {
            return Err(SolidError::sm_err(OP_EXT_EDGE));
        }

        // Edge need to be extended by recomputing the SSI
        let orig_edge = self
            .fillet_edge(e)?
            .original_edge()
            .ok_or_else(|| SolidError::null_pointer(OP_EXT_EDGE))?;
        let mut faces = edge::faces(target, orig_edge, None)?;
        if faces.len() < 2 {
            return Err(SolidError::sm_err(OP_EXT_EDGE));
        }
        let the_corner = corners.corner(corner)?;
        // Past a tangent side edge (`make_nx1_corner_topology`), the extended edge
        // bounds the end face and a face that is not the side face this edge runs in:
        // its extension is where that side face meets the end face's extension
        // (source fork@d5ebcb3, WR-457).
        if k.fixes.fillet_nx1_tangent_side_edge {
            if let Some(side) = self.fillet_edge(e)?.original_face() {
                if !faces.contains(&side) {
                    let mut end_face = None;
                    for &f in &faces {
                        if extended_surface(the_corner, f)?.is_some() {
                            end_face = Some(f);
                            break;
                        }
                    }
                    let end_face = end_face.ok_or_else(|| SolidError::null_pointer(OP_EXT_EDGE))?;
                    faces = vec![side, end_face];
                }
            }
        }
        let mut surfaces: Vec<Option<Surface>> = Vec::with_capacity(2);
        for &f in faces.iter().take(2) {
            let surf = match extended_surface(the_corner, f)? {
                Some(s) => Some(s),
                None => target.face(f)?.surface.clone(),
            };
            // SM_CAST_PTR(SmBSplineSurface, pSurf)
            surfaces.push(surf.filter(is_b_spline_surface));
        }

        let mut tol = edge::tolerance(target, orig_edge)?;
        tol = smax(tol, the_corner.this_approx_tol_3d());
        let mut dir = self.vertex_point(end_fv)? - self.vertex_point(start_fv)?;
        let angle_tol = 2.0 * PI / 180.0;
        let surface1 = surfaces
            .first()
            .cloned()
            .flatten()
            .ok_or_else(|| SolidError::null_pointer(OP_EXT_EDGE))?;
        let surface2 = surfaces
            .get(1)
            .cloned()
            .flatten()
            .ok_or_else(|| SolidError::null_pointer(OP_EXT_EDGE))?;
        let curves = srf_srf_intersection(
            k,
            self,
            Some(start_fv),
            Some(end_fv),
            Some(&mut dir),
            None,
            &surface1,
            &surface2,
            tol,
            angle_tol,
            false,
            None,
        )?;
        let curve = curves
            .curve_3d
            .ok_or_else(|| SolidError::null_pointer(OP_EXT_EDGE))?;
        // both UV curves are deleted
        set_edge_curve(self.brep_mut(), e, Some(curve))?;
        Ok(())
    }

    /// `SmFilletEdge::MakeFilletEdge(pPseudoBrep, pStartVertex, pEndVertex,
    /// rpNewFilletEdge, cpCorner)`: a new fillet edge (no curve) from `start` to `end`
    /// with two edge uses (primary `SM_OT_SAME` at the start, mate `SM_OT_OPPOSITE` at
    /// the end) and their fillet vertex uses.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` for a missing vertex; those of the edge use constructor
    /// ([`FilletBrep::new_fillet_edgeuse`]); [`SolidError::NotFound`] for dangling ids.
    pub fn make_fillet_edge(
        &mut self,
        start: Option<VertexId>,
        end: Option<VertexId>,
        corner: Option<FilletCornerId>,
    ) -> Result<EdgeId> {
        // Check input
        let start = start.ok_or_else(|| SolidError::null_pointer(OP_MAKE_EDGE))?;
        let end = end.ok_or_else(|| SolidError::null_pointer(OP_MAKE_EDGE))?;
        self.fillet_vertex(start)?;
        self.fillet_vertex(end)?;

        // allocate the FilletEdge
        let new_e = self.new_fillet_edge(start, end, corner)?;
        // allocate 1st and 2nd FilletEdgeuse
        let eu1_id = self.new_fillet_edgeuse()?;
        let eu2_id = self.new_fillet_edgeuse()?;
        // allocate the FilletVertexuses - connect them to the FilletEdgeuses
        let vu1 = new_fillet_vertexuse(self)?;
        self.set_vertexuse_property(vu1, eu1_id)?;
        let vu2 = new_fillet_vertexuse(self)?;
        self.set_vertexuse_property(vu2, eu2_id)?;

        // Put VU1 into start vertex list and VU2 into end vertex
        vertex_post_insert(self.brep_mut(), start, vu1)?;
        vertex_post_insert(self.brep_mut(), end, vu2)?;

        // Put EU1 and EU2 into edge - Make EU1 the Primary Edgeuse
        set_eu_orientation(self.brep_mut(), eu1_id, OrientType::Same)?;
        set_eu_orientation(self.brep_mut(), eu2_id, OrientType::Opposite)?;
        edge_insert(self.brep_mut(), new_e, eu1_id, true)?;
        edge_insert(self.brep_mut(), new_e, eu2_id, false)?;

        // Attach edgeuses to vertexuses (2nd half of backpointer set)
        self.set_edgeuse_fillet_vertexuse(eu1_id, Some(vu1))?;
        self.set_edgeuse_fillet_vertexuse(eu2_id, Some(vu2))?;
        Ok(new_e)
    }

    /// `SmFilletGeom(SmFilletGeom* pFilletGeomToCopy)`: a new fillet geometry with the
    /// source's solver and type, two new rails (`MakeRailEdge` in the executive's
    /// pseudo B-rep, statuses ignored) and the source's data (`Copy`, status ignored).
    /// The copy is added to this B-rep's geometries.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` for a source without solver or a solver without executive
    /// (upstream dereferences them).
    pub fn copy_fillet_geom(
        &mut self,
        k: &Kernel,
        src: FilletGeomId,
        solvers: &[FilletSolver],
    ) -> Result<FilletGeomId> {
        let solver_id = self.geom(src)?.fillet_solver();
        let solver = solver_of(
            solvers,
            solver_id.ok_or_else(|| SolidError::null_pointer(OP_COPY))?,
            OP_COPY,
        )?;
        if solver.executive.is_none() {
            return Err(SolidError::null_pointer(OP_COPY));
        }
        // m_eType is assigned last; the members are as SmFilletGeom(NULL, ..) leaves
        // them, with the source's solver (its offset surfaces are not copied here)
        let mut geom = FilletGeom::new(k, None, FilletGeomType::Default)?;
        geom.solver = solver_id;
        let g = self.add_geom(geom);

        // Create Rails: `m_pExecutive->GetPseudoBrep()` is the executive's pseudo B-rep,
        // the `FilletBrep` the geometries live in (`self`).
        for i in 0..2 {
            ignore_non_unported(self.make_rail_edge(k, g, i))?;
        }
        ignore_non_unported(self.copy_geom(k, g, src, solvers))?;
        let gtype = self.geom(src)?.fillet_geom_type();
        self.geom_mut(g)?.set_fillet_geom_type(gtype);
        Ok(g)
    }

    /// `SmFilletGeom::MakeRailEdge(pPseudoBrep, lRailIndex, rpNewRail)`: a new rail
    /// edge (`SM_FE_RAIL`, no curve) of geometry `g` between two new fillet vertices,
    /// with two edge uses and their vertex uses; it becomes the geometry's rail
    /// `rail_index` and its vertices join the geometry's vertices.
    ///
    /// # Errors
    ///
    /// Those of the edge use constructor ([`FilletBrep::new_fillet_edgeuse`]);
    /// [`SolidError::InvalidInput`] for a rail index above 1.
    pub fn make_rail_edge(
        &mut self,
        k: &Kernel,
        g: FilletGeomId,
        rail_index: usize,
    ) -> Result<EdgeId> {
        self.geom(g)?;
        if rail_index > 1 {
            return Err(SolidError::invalid_input(
                OP_RAIL_EDGE,
                "rail index above 1",
            ));
        }
        // the edge's vertices are its uses' (SmFilletEdge(pPseudoBrep) has none); the
        // two fillet vertices are made first so that the edge record can name them
        let start_vert = new_fillet_vertex(k, self)?;
        let end_vert = new_fillet_vertex(k, self)?;
        let rail = self.new_fillet_edge(start_vert, end_vert, None)?;
        self.fillet_edge_mut(rail)?
            .set_fillet_edge_type(FilletEdgeType::Rail);

        let new_eu1 = self.new_fillet_edgeuse()?;
        let new_eu2 = self.new_fillet_edgeuse()?;
        let vu1 = new_fillet_vertexuse(self)?;
        self.set_vertexuse_property(vu1, new_eu1)?;
        let vu2 = new_fillet_vertexuse(self)?;

        // Put VU1 into start vertex list and VU2 into end vertex
        vertex_post_insert(self.brep_mut(), start_vert, vu1)?;
        vertex_post_insert(self.brep_mut(), end_vert, vu2)?;
        self.set_vertexuse_property(vu2, new_eu2)?;
        edge_insert(self.brep_mut(), rail, new_eu1, true)?;
        edge_insert(self.brep_mut(), rail, new_eu2, false)?;
        self.set_edgeuse_fillet_vertexuse(new_eu1, Some(vu1))?;
        self.set_edgeuse_fillet_vertexuse(new_eu2, Some(vu2))?;

        let geom = self.geom_mut(g)?;
        geom.set_rail(rail_index, Some(rail));
        geom.vertices.push(start_vert);
        geom.vertices.push(end_vert);
        Ok(rail)
    }

    /// `SmFilletGeom::MakeRailEdgeuse(pPseudoBrep, lRailIndex, pFilletVertex,
    /// rpNewEdgeuse, eOrient, pOptBelongingRail)`: a new edge use of geometry `g` on its
    /// rail `rail_index` (or `belonging_rail`; a new rail edge when it has none) at
    /// fillet vertex `fv`. Its orientation is `orient`, or, for `SM_OT_UNKNOWN`,
    /// `SM_OT_SAME` unless the solver's (non-closed) edge use runs the other way at the
    /// corner's vertex; a `SM_OT_SAME` use goes first in the rail's list.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` without solver, vertex, solver edge uses, faces, corner;
    /// those of the edge use constructor; those of [`CornerLookup::corner`] (`corners` resolves the corner) of a
    /// non-closed rail.
    #[allow(clippy::too_many_arguments)]
    pub fn make_rail_edgeuse(
        &mut self,
        g: FilletGeomId,
        rail_index: usize,
        fv: Option<VertexId>,
        orient: OrientType,
        belonging_rail: Option<EdgeId>,
        corners: CornerLookup<'_>,
        solvers: &[FilletSolver],
        target: &Brep,
    ) -> Result<EdgeuseId> {
        // Check input
        let solver = self.geom_solver(g, solvers, OP_RAIL_EU)?;
        self.make_rail_edgeuse_on(
            g,
            rail_index,
            fv,
            orient,
            belonging_rail,
            corners,
            solver,
            target,
        )
    }

    /// [`FilletBrep::make_rail_edgeuse`] with geometry `g`'s solver already resolved
    /// (`GetFilletSolver()`, checked non-`NULL` by the caller).
    ///
    /// # Errors
    ///
    /// Those of [`FilletBrep::make_rail_edgeuse`] after its solver lookup.
    #[allow(clippy::too_many_arguments)]
    pub fn make_rail_edgeuse_on(
        &mut self,
        g: FilletGeomId,
        rail_index: usize,
        fv: Option<VertexId>,
        orient: OrientType,
        belonging_rail: Option<EdgeId>,
        corners: CornerLookup<'_>,
        solver: &FilletSolver,
        target: &Brep,
    ) -> Result<EdgeuseId> {
        let fv = fv.ok_or_else(|| SolidError::null_pointer(OP_RAIL_EU))?;
        self.fillet_vertex(fv)?;
        if rail_index > 1 {
            return Err(SolidError::invalid_input(OP_RAIL_EU, "rail index above 1"));
        }
        let base_eu = solver.edgeuse(rail_index)?;
        let base_eu1 = solver.edgeuse(1 - rail_index)?;

        // 1. allocate new FilletEdgeuse->FilletVertexuse pair
        let new_eu = self.new_fillet_edgeuse()?;
        let new_vu = new_fillet_vertexuse(self)?;
        self.set_vertexuse_property(new_vu, new_eu)?;

        // 2. connect filletVertexuse to filletEdgeuse
        self.set_edgeuse_fillet_vertexuse(new_eu, Some(new_vu))?;
        vertex_post_insert(self.brep_mut(), fv, new_vu)?;
        self.set_edgeuse_fillet_geom(new_eu, Some(g))?;

        // 3. Get or make rail FilletEdge
        let mut rail_e = belonging_rail;
        if rail_e.is_none() {
            rail_e = self.geom(g)?.rail(rail_index);
        }
        let rail_e = if let Some(r) = rail_e {
            r
        } else {
            // Create a new one; SmFilletEdge(pPseudoBrep) has no vertices, the
            // record names the fillet vertex at both ends until uses are added. The
            // placeholder is only the arena record's `vertices` field: every topology
            // accessor (`edge::vertex`, `start_end_vertices`, `vertices`) derives the
            // vertices from the edge uses' vertex uses, as upstream's `GetVertex(es)`
            // do, so it is never returned as the rail's vertex; the difftest export
            // reads that field; its native counterpart preserves creation order
            // rather than using orientation-dependent GetVertices ordering.
            let r = self.new_fillet_edge(fv, fv, None)?;
            self.fillet_edge_mut(r)?
                .set_fillet_edge_type(FilletEdgeType::Rail);
            self.geom_mut(g)?.set_rail(rail_index, Some(r));
            r
        };

        // 4. set topology pointers
        let base_eu = base_eu.ok_or_else(|| SolidError::null_pointer(OP_RAIL_EU))?;
        let base_face = eu1::face(target, base_eu)?;
        self.fillet_edge_mut(rail_e)?.set_original_face(base_face);

        // Set srf ptrs in FilletVertexuse's TsectPnt.
        let face = base_face.ok_or_else(|| SolidError::null_pointer(OP_RAIL_EU))?;
        target.face(face)?;
        if let Some(slot) = self
            .fillet_vertexuse_mut(new_vu)?
            .tsect_pnt_mut()
            .user_pointer
            .get_mut(rail_index)
        {
            *slot = user_surface(target, face)?;
        }
        let base_eu1 = base_eu1.ok_or_else(|| SolidError::null_pointer(OP_RAIL_EU))?;
        let face1 =
            eu1::face(target, base_eu1)?.ok_or_else(|| SolidError::null_pointer(OP_RAIL_EU))?;
        target.face(face1)?;
        if let Some(slot) = self
            .fillet_vertexuse_mut(new_vu)?
            .tsect_pnt_mut()
            .user_pointer
            .get_mut(1 - rail_index)
        {
            *slot = user_surface(target, face1)?;
        }

        // set new Edgeuse->m_eOrientation
        if orient == OrientType::Unknown {
            set_eu_orientation(self.brep_mut(), new_eu, OrientType::Same)?;
            // when edge is not closed
            let base_v = eu_vertex(target, base_eu, OP_RAIL_EU)?;
            let base_mate =
                eu1::mate(target, base_eu)?.ok_or_else(|| SolidError::null_pointer(OP_RAIL_EU))?;
            if Some(base_v) != Some(eu_vertex(target, base_mate, OP_RAIL_EU)?) {
                // Non-closed rails: the original vertex of the pFilletVertex
                let corner = self
                    .fillet_vertex(fv)?
                    .fillet_corner()
                    .ok_or_else(|| SolidError::null_pointer(OP_RAIL_EU))?;
                let orig_v = corners.corner(corner)?.filleted_vertex();
                let base_orient = target.edgeuse(base_eu)?.orientation;
                if (base_orient == OrientType::Same && orig_v != Some(base_v))
                    || (base_orient == OrientType::Opposite && orig_v == Some(base_v))
                {
                    set_eu_orientation(self.brep_mut(), new_eu, OrientType::Opposite)?;
                }
            }
        } else {
            set_eu_orientation(self.brep_mut(), new_eu, orient)?;
        }

        // 5. Put FEU into rail edge as the mate of the pRailE
        let same = self.brep().edgeuse(new_eu)?.orientation == OrientType::Same;
        edge_insert(self.brep_mut(), rail_e, new_eu, same)?;
        Ok(new_eu)
    }

    /// `SmFilletGeom::CreateEndCurve(lEndIndex, eTrimType, pPseudoBrep)`: the end
    /// cross-section of geometry `g` at end `end_index` between its two rails' end
    /// vertices, a UV line (or, for `SM_BT_BLEND` with an edge tangent at either
    /// end, a UV Hermite curve along the dropped tangents) lifted onto the fillet
    /// surface, as a new `SM_FE_CROSS_SECTION` fillet edge whose primary use joins the
    /// geometry's side uses.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` without surface, rails, uses, solver; those of the
    /// curve creations, the lift, [`FilletBrep::make_fillet_edge`] and (for
    /// `SM_BT_BLEND`) `FindEdgeTangentAtPoint`, which reads the rail end vertices'
    /// classification edges in `target`.
    pub fn create_end_curve(
        &mut self,
        k: &Kernel,
        g: FilletGeomId,
        end_index: usize,
        trim_type: BoundaryTrimmingType,
        solvers: &[FilletSolver],
        target: &Brep,
    ) -> Result<EdgeId> {
        self.create_end_curve_resolved(
            k,
            g,
            end_index,
            trim_type,
            &|id, op| solver_of(solvers, id, op),
            [target, target],
        )
    }

    /// `CreateEndCurve` using the actual live solver of geometry `g` (its
    /// `SplitFilletGeom` caller is that solver).
    ///
    /// # Errors
    /// The errors of [`Self::create_end_curve`]; [`SolidError::InvalidInput`] if the
    /// geometry names a solver other than `solver_id`.
    #[allow(clippy::too_many_arguments)]
    pub fn create_end_curve_with_solver(
        &mut self,
        k: &Kernel,
        g: FilletGeomId,
        end_index: usize,
        trim_type: BoundaryTrimmingType,
        solver_id: FilletSolverId,
        solver: &FilletSolver,
        bases: [&Brep; 2],
    ) -> Result<EdgeId> {
        self.create_end_curve_resolved(
            k,
            g,
            end_index,
            trim_type,
            &|id, op| {
                if id == solver_id {
                    Ok(solver)
                } else {
                    Err(SolidError::invalid_input(op, "unresolved geometry solver"))
                }
            },
            bases,
        )
    }

    #[allow(clippy::too_many_lines)] // one upstream routine
    fn create_end_curve_resolved<'s>(
        &mut self,
        k: &Kernel,
        g: FilletGeomId,
        end_index: usize,
        trim_type: BoundaryTrimmingType,
        resolve: &impl Fn(FilletSolverId, &'static str) -> Result<&'s FilletSolver>,
        bases: [&Brep; 2],
    ) -> Result<EdgeId> {
        // rail ii's end vertices are classified on base B-rep ii: the target for
        // both rails, or a surface/surface fillet's second base for its second rail
        let [base0, base1] = bases;
        // Create a line between the two UV points and lift the curve.
        let fillet_surface = self
            .geom(g)?
            .fillet_surface(None)?
            .cloned()
            .ok_or_else(|| SolidError::null_pointer(OP_END))?;
        let mut eus = Vec::with_capacity(2);
        for i in 0..2 {
            let rail = self
                .geom(g)?
                .rail(i)
                .ok_or_else(|| SolidError::null_pointer(OP_END))?;
            let prim = primary_edgeuse(self.brep(), rail)?
                .ok_or_else(|| SolidError::null_pointer(OP_END))?;
            eus.push(if end_index == 0 {
                prim
            } else {
                eu1::mate(self.brep(), prim)?.ok_or_else(|| SolidError::null_pointer(OP_END))?
            });
        }
        let (Some(&eu_0), Some(&eu_1)) = (eus.first(), eus.get(1)) else {
            return Err(SolidError::null_pointer(OP_END));
        };
        let eus = [eu_0, eu_1];
        let vu_of = |fb: &Self, eu: EdgeuseId| -> Result<VertexuseId> {
            fb.brep()
                .edgeuse(eu)?
                .vertexuse
                .ok_or_else(|| SolidError::null_pointer(OP_END))
        };
        let vu0 = vu_of(self, eus[0])?;
        let vu1 = vu_of(self, eus[1])?;
        let fv_start = self
            .brep()
            .vertexuse(vu0)?
            .vertex
            .ok_or_else(|| SolidError::null_pointer(OP_END))?;
        let fv_end = self
            .brep()
            .vertexuse(vu1)?
            .vertex
            .ok_or_else(|| SolidError::null_pointer(OP_END))?;
        let p_start = self.fillet_vertexuse(vu0)?.uv_point();
        let p_end = self.fillet_vertexuse(vu1)?.uv_point();
        let sid = self
            .geom(g)?
            .fillet_solver()
            .ok_or_else(|| SolidError::null_pointer(OP_END))?;
        let solver = resolve(sid, OP_END)?;

        let uv_curve: Curve = if trim_type == BoundaryTrimmingType::Blend {
            let mut uv_tan1 = Vec2::new(0.0, 1.0);
            let mut uv_tan2 = Vec2::new(0.0, 1.0);
            let dist = self
                .vertex_point(fv_start)?
                .distance(self.vertex_point(fv_end)?);

            // Find Edge Tangents
            // (rbFound, rTangent): a tangent not found leaves SmVector3d() undefined
            let found = |t: Option<Vec3>| (t.is_some(), t.unwrap_or(Vec3::UNDEF));
            let (found1, mut tangent1) =
                found(self.find_edge_tangent_at_point(k, g, 0, end_index, base0)?);
            let (found2, mut tangent2) =
                found(self.find_edge_tangent_at_point(k, g, 1, end_index, base1)?);

            // when 1st tangent was found
            if found1 {
                tangent1 = tangent1.unitize_or_z();
                tangent1 *= dist;
                uv_tan1 = drop_vector(k, &fillet_surface, p_start, tangent1)?;
                if uv_tan1.y < 0.0 {
                    uv_tan1 = -uv_tan1;
                }
                if uv_tan1.y < 0.1 {
                    // If angle is too small then difficult to blend just skip it
                    uv_tan1.x = 0.0;
                    uv_tan1.y = 1.0;
                }
            }
            // when second tangent was found
            if found2 {
                tangent2 = tangent2.unitize_or_z();
                tangent2 *= dist;
                uv_tan2 = drop_vector(k, &fillet_surface, p_end, tangent2)?;
                if uv_tan2.y < 0.0 {
                    uv_tan2 = -uv_tan2;
                }
                if uv_tan2.y < 0.1 {
                    uv_tan2.x = 0.0;
                    uv_tan2.y = 1.0;
                }
            }

            if !found1 && !found2 {
                // just make a line segment
                Curve::from(BSplineCurve::create_line_segment(
                    2,
                    p3(p_start),
                    p3(p_end),
                )?)
            } else {
                // some tangent was found - make a hermite curve between the end points
                uv_tan1 = (uv_tan1 * solver.edge_blend_factor) * 1.5;
                uv_tan2 = (uv_tan2 * solver.edge_blend_factor) * 1.5;
                let herm = Curve::from(HermiteCurve::new(
                    p3(p_start),
                    Vec3::new(uv_tan1.x, uv_tan1.y, 0.0),
                    p3(p_end),
                    Vec3::new(uv_tan2.x, uv_tan2.y, 0.0),
                    2,
                ));
                let breaks = [0.0, 1.0];
                let approx = herm.approximate_curve(
                    k,
                    &breaks,
                    solver.approx_tol_3d / 10.0,
                    &ApproximateCurveOptions {
                        create_analytics: true,
                        match_parameterization: false,
                        just_copy_bsplines: false,
                    },
                )?;
                let mut c = approx.curve;
                ignore_non_unported(c.convert_to_2d(k))?;
                c
            }
        } else {
            // just make a line segment
            Curve::from(BSplineCurve::create_line_segment(
                2,
                p3(p_start),
                p3(p_end),
            )?)
        };

        // next - Lift pUVCurve to p3DCurve
        let lifted = fillet_surface.lift_curve(
            k,
            &fillet_surface.natural_uv_domain_with_nurb(k)?,
            &uv_curve,
            &uv_curve.natural_interval(),
            solver.approx_tol_3d / 4.0,
            false,
        )?;
        let curve_3d = lifted.curve;

        // make pFilletEdge in pPseudoBrep
        let fillet_edge = self.make_fillet_edge(Some(fv_start), Some(fv_end), None)?;
        self.fillet_edge_mut(fillet_edge)?
            .set_fillet_edge_type(FilletEdgeType::CrossSection);
        let ivl = curve_3d.natural_interval();
        set_edge_curve(self.brep_mut(), fillet_edge, Some(curve_3d))?;
        edge::set_interval(self.brep_mut(), fillet_edge, &ivl)?;
        let eu = primary_edgeuse(self.brep(), fillet_edge)?
            .ok_or_else(|| SolidError::null_pointer(OP_END))?;
        self.set_uv(eu, Some(uv_curve))?;

        // save results in SmFilletGeom arrays
        let geom = self.geom_mut(g)?;
        geom.side_eus.push(eu);
        geom.edges.push(fillet_edge);
        Ok(fillet_edge)
    }

    /// `SmFilletGeom::TrimRailCurves`: trims each rail of geometry `g` (and its UV
    /// curves) to its end vertices: the rail's UV curve on the fillet surface becomes
    /// the iso-v line of the matching surface side, the start vertex is dropped onto
    /// the rail and the end vertex solved for; a rail whose end points coincide is
    /// trimmed at its single self-intersection (closed and degenerate rails are left
    /// alone). A cliff-rollover geometry goes to [`FilletBrep::trim_cliff_rails`].
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` without fillet surface, rails, uses or rail curve;
    /// `SM_ERR` when the rail does not follow either v side of the surface, the start
    /// vertex does not drop, the start use is not `SM_OT_SAME` or the end vertex has
    /// no solution; those of the evaluations, solves and trims.
    pub fn trim_rail_curves(
        &mut self,
        k: &Kernel,
        g: FilletGeomId,
        solvers: &[FilletSolver],
        target: &Brep,
    ) -> Result<()> {
        self.trim_rail_curves_resolved(k, g, &|id, op| solver_of(solvers, id, op), target)
    }

    /// `TrimRailCurves` using the actual live solver of geometry `g`, without
    /// copying it or assuming its id is zero. The indexed-table entry is unchanged.
    ///
    /// # Errors
    /// The errors of [`Self::trim_rail_curves`]; [`SolidError::InvalidInput`] if
    /// the geometry names a solver other than `solver_id`.
    pub fn trim_rail_curves_with_solver(
        &mut self,
        k: &Kernel,
        g: FilletGeomId,
        solver_id: FilletSolverId,
        solver: &FilletSolver,
        target: &Brep,
    ) -> Result<()> {
        self.trim_rail_curves_resolved(
            k,
            g,
            &|id, op| {
                if id == solver_id {
                    Ok(solver)
                } else {
                    Err(SolidError::invalid_input(op, "unresolved geometry solver"))
                }
            },
            target,
        )
    }

    #[allow(clippy::too_many_lines)] // one upstream routine
    fn trim_rail_curves_resolved<'s>(
        &mut self,
        k: &Kernel,
        g: FilletGeomId,
        resolve: &impl Fn(FilletSolverId, &'static str) -> Result<&'s FilletSolver>,
        target: &Brep,
    ) -> Result<()> {
        // handle CliffRollover separately
        if self.geom(g)?.fillet_geom_type() == FilletGeomType::CliffRollover {
            self.trim_cliff_rails_resolved(k, g, resolve, target)?;
            return Ok(());
        }

        // check state
        let fillet_surface = self
            .geom(g)?
            .fillet_surface(None)?
            .cloned()
            .ok_or_else(|| SolidError::null_pointer(OP_TRIM))?;

        // locals: FilletSurface Natural domain and current tolerance
        let uv_domain = fillet_surface.natural_uv_domain_with_nurb(k)?;
        let sid = self
            .geom(g)?
            .fillet_solver()
            .ok_or_else(|| SolidError::null_pointer(OP_TRIM))?;
        let app_tol = resolve(sid, OP_TRIM)?.approx_tol_3d;

        // for both rail curves
        for ii in 0..2 {
            let rail = self
                .geom(g)?
                .rail(ii)
                .ok_or_else(|| SolidError::null_pointer(OP_TRIM))?;
            let (prim, mate) = self.p5_prim_mate(rail, OP_TRIM)?;
            let vu0 = self
                .brep()
                .edgeuse(prim)?
                .vertexuse
                .ok_or_else(|| SolidError::null_pointer(OP_TRIM))?;
            let vu1 = self
                .brep()
                .edgeuse(mate)?
                .vertexuse
                .ok_or_else(|| SolidError::null_pointer(OP_TRIM))?;
            let v0 = eu_vertex(self.brep(), prim, OP_TRIM)?;
            let v1 = eu_vertex(self.brep(), mate, OP_TRIM)?;

            // get rail->vertex->points
            let v0_processed = self.fillet_vertex(v0)?.is_processed();
            let v1_processed = self.fillet_vertex(v1)?.is_processed();
            let mut pnt0 = Point3::new(-1000.0, 9999.0, 2000.0);
            if v0_processed {
                pnt0 = self.vertex_point(v0)?;
            }
            let mut pnt1 = Point3::new(3456.0, 8901.0, 4521.0);
            if v1_processed {
                pnt1 = self.vertex_point(v1)?;
            }

            // get rail->Curve
            let rail_curve = match edge::curve(self.brep(), rail)? {
                Some(c) if is_b_spline(c) => c.clone(),
                _ => return Err(SolidError::null_pointer(OP_TRIM)),
            };

            // init Trim interval = 3DCurve->NaturalInterval
            let mut trim_ivl = rail_curve.natural_interval();

            // Pick a point out in the middle.
            let param = trim_ivl.evaluate(0.34567);
            let curve_pnt = rail_curve.evaluate_point(k, param)?;
            let mut iso_v = uv_domain.min().y;
            let mut test_uv = Point2::new(param, iso_v);
            let mut surf_pnt = fillet_surface.evaluate_point(k, test_uv)?;
            if curve_pnt.distance(surf_pnt) > app_tol {
                // check the other edge of the fillet surface
                iso_v = uv_domain.max().y;
                test_uv.y = iso_v;
                surf_pnt = fillet_surface.evaluate_point(k, test_uv)?;
                if curve_pnt.distance(surf_pnt) > app_tol {
                    // Rail is not syncronizing with the parametrization of the fillet
                    // surface
                    return Err(SolidError::sm_err(OP_TRIM));
                }
            }

            // set rail->PrimaryEdgeuse->UVTrimCurve endPoint values
            let start_uv = Point2::new(trim_ivl.min(), test_uv.y);
            let end_uv = Point2::new(trim_ivl.max(), test_uv.y);

            // clean out existing primaryEdgeuse->UVTrimCurve; create the filletSurface
            // UV-Min/Max Line for this rail->PrimaryEdgeuse->UVTrimCurve
            self.set_edgeuse_uv_curve(prim, None, true, true)?;
            let mut uv_curve = BSplineCurve::create_line_segment(2, p3(start_uv), p3(end_uv))?;
            uv_curve.edit_parameterization(&trim_ivl)?;

            // Save UV curve in rail->PrimaryEdgeuse->UVTrimCurve
            self.set_uv(prim, Some(Curve::from(uv_curve)))?;

            // when the two trim points differ by more than tolerance
            let mut param1 = trim_ivl.min();
            let mut param2 = trim_ivl.max();
            if pnt0.distance(pnt1) > app_tol {
                if v0_processed {
                    // drop V0->point to rail->Curve over sequence of ranges until we
                    // get a hit
                    let mut t = 0.0;
                    let mut success = false;
                    let mut dropping_tol = app_tol;
                    let mut scale = 10.0;
                    while scale < 100.0 {
                        match rail_curve.drop_point(
                            k,
                            &trim_ivl,
                            pnt0,
                            None,
                            dropping_tol,
                            None,
                            SolverOperation::Minimize,
                        ) {
                            Ok(d) => {
                                success = d.success;
                                if d.success {
                                    t = d.parameter;
                                }
                            }
                            Err(e) if e.is_unported() => return Err(e),
                            // TrimRailCurve(): Suffered a SmCurve::DropPoint() failure
                            Err(_) => success = false,
                        }
                        if success {
                            break;
                        }
                        dropping_tol = app_tol * scale;
                        scale += 10.0;
                    }
                    if !success {
                        return Err(SolidError::sm_err(OP_TRIM));
                    }

                    // clean up dT value from 3.000000001 to 3.0
                    t = clean_up_noise(t);

                    // set Vertexuse0->UVPoint on filletSurface
                    self.fillet_vertexuse_mut(vu0)?
                        .set_uv_point(Point2::new(t, iso_v));

                    // save V0 to d3Curve dropPoint parameter
                    if self.brep().edgeuse(prim)?.orientation == OrientType::Same {
                        param1 = t;
                    } else {
                        return Err(SolidError::sm_err(OP_TRIM));
                    }
                }

                // when V1 vertex is processed
                if v1_processed {
                    // find 3DCurve points close to V1
                    let opts = rail_solve_options(&rail_curve);
                    let sols = rail_curve.global_point_solve(
                        k,
                        &trim_ivl,
                        SolverOperation::Minimize,
                        pnt1,
                        app_tol,
                        SolutionRequested::All,
                        &opts,
                    )?;
                    let n = sols.len();
                    let first = sols.get(0).ok_or_else(|| SolidError::sm_err(OP_TRIM))?;
                    param2 = first.start.parameters[0];
                    if n > 1 {
                        let mut best = first.start.solution_value;
                        for s in sols.as_slice().iter().skip(1) {
                            // Check only those beyond dParam1
                            if s.start.parameters[0] > param1 && s.start.solution_value <= best {
                                param2 = s.start.parameters[0];
                                best = s.start.solution_value;
                            }
                        }
                    }

                    // set Vertexuse1->UVPoint on filletSurface
                    self.fillet_vertexuse_mut(vu1)?
                        .set_uv_point(Point2::new(param2, iso_v));
                }

                // clip the interval to drop points
                ignore_non_unported(trim_ivl.set_min_max(param1, param2))?;

                // Trim 3D curve and both UVTrimCurves (each may snap sIvl by tol to
                // existing knots)
                self.trim_rail_and_uv_curves(k, rail, prim, mate, &mut trim_ivl)?;
            } else {
                // Rail could be either closed, degenerate or self-intersected
                // No further processing if closed or degenerate
                if rail_curve.is_closed(k, &trim_ivl, app_tol)?
                    || rail_curve.is_degenerate(k, EFF_ZERO, None)?
                {
                    continue;
                }

                // Check if rail self-intersect
                let opts = rail_solve_options(&rail_curve);
                let sols = rail_curve.global_curve_self_intersect(
                    k,
                    &trim_ivl,
                    app_tol * 10.0,
                    opts.handle,
                    opts.analytic_handle,
                )?;

                // when there is just 1 selfIntersection solution
                if sols.len() == 1 {
                    // get nonSelfIntersecting interval
                    let sol = sols
                        .get(0)
                        .ok_or_else(|| SolidError::null_pointer(OP_TRIM))?;
                    param1 = sol.start.parameters[0];
                    param2 = sol.start.parameters[1];
                    ignore_non_unported(trim_ivl.set_min_max(param1, param1))?;
                    trim_ivl.add_value(param2);

                    // trim curve and UVTrimCurves to interval
                    self.trim_rail_and_uv_curves(k, rail, prim, mate, &mut trim_ivl)?;
                }
                // else: more than 1 self intersection found on rail curve - case not
                // handled yet
            }
        }
        Ok(())
    }

    /// The trims at the end of `TrimRailCurves`: the rail's curve, then its mate's UV
    /// curve (the original face's, when it has one), then its primary use's UV curve,
    /// each with the (possibly snapped) interval. Each trim reads
    /// `SmTol::GetZoneTol3d(curve)` of the curve's holder ([`held_curve_zone_tol_3d`]).
    fn trim_rail_and_uv_curves(
        &mut self,
        k: &Kernel,
        rail: EdgeId,
        prim: EdgeuseId,
        mate: EdgeuseId,
        trim_ivl: &mut Extent1d,
    ) -> Result<()> {
        let rail_tol = held_curve_zone_tol_3d(k, self.brep(), Some(rail))?;
        let mate_edge = self.brep().edgeuse(mate)?.edge;
        let mate_tol = held_curve_zone_tol_3d(k, self.brep(), mate_edge)?;
        let prim_edge = self.brep().edgeuse(prim)?.edge;
        let prim_tol = held_curve_zone_tol_3d(k, self.brep(), prim_edge)?;
        self.brep_mut()
            .edge_mut(rail)?
            .curve
            .as_mut()
            .ok_or_else(|| SolidError::null_pointer(OP_TRIM))?
            .trim_with_zone_tol(k, trim_ivl, TrimMode::Notify, rail_tol)?;
        if let Some(c) = self
            .brep_mut()
            .edgeuse_mut(mate)?
            .uv_trim_curve
            .as_mut()
            .filter(|c| is_b_spline(c))
        {
            c.trim_with_zone_tol(k, trim_ivl, TrimMode::Notify, mate_tol)?;
        }
        self.brep_mut()
            .edgeuse_mut(prim)?
            .uv_trim_curve
            .as_mut()
            .ok_or_else(|| SolidError::null_pointer(OP_TRIM))?
            .trim_with_zone_tol(k, trim_ivl, TrimMode::Notify, prim_tol)?;
        Ok(())
    }

    /// `SmFilletGeom::TrimCliffRails`: the rails of a cliff-rollover geometry `g`: for
    /// each rail and its other rails, unprocessed cliff vertices are computed
    /// (`CalcCliffRailIntEdge`), a cliff rail becomes the intersection of the fillet
    /// surface with its side surface, and a plain rail a copy of the original rail
    /// curve (with the iso-v UV line and the base UV curve) trimmed to its vertices.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a geometry of another type, a rail that follows neither v side of
    /// the surface, a vertex that does not drop or solve, or reversed trim parameters;
    /// `SM_ERR_NULL_POINTER` for missing solver, surface, rails, uses, curves, faces;
    /// those of the evaluations, solves, copies and trims; [`SolidError::Unported`] for
    /// the intersection (`sm_SrfSrfIntersection`) and the cliff vertices' callees.
    pub fn trim_cliff_rails(
        &mut self,
        k: &Kernel,
        g: FilletGeomId,
        solvers: &[FilletSolver],
        target: &Brep,
    ) -> Result<()> {
        self.trim_cliff_rails_resolved(k, g, &|id, op| solver_of(solvers, id, op), target)
    }

    #[allow(clippy::too_many_lines)] // one upstream routine
    fn trim_cliff_rails_resolved<'s>(
        &mut self,
        k: &Kernel,
        g: FilletGeomId,
        resolve: &impl Fn(FilletSolverId, &'static str) -> Result<&'s FilletSolver>,
        target: &Brep,
    ) -> Result<()> {
        if self.geom(g)?.fillet_geom_type() != FilletGeomType::CliffRollover {
            return Err(SolidError::sm_err(OP_CLIFF));
        }
        let sid = self
            .geom(g)?
            .fillet_solver()
            .ok_or_else(|| SolidError::null_pointer(OP_CLIFF))?;
        let solver = resolve(sid, OP_CLIFF)?;
        let tolerance = solver.approx_tol_3d;
        let angle_tol = solver.ang_tol_rad;
        let fillet_surface = self
            .geom(g)?
            .fillet_surface(None)?
            .cloned()
            .ok_or_else(|| SolidError::null_pointer(OP_CLIFF))?;
        let fil_srf_domain = fillet_surface.natural_uv_domain_with_nurb(k)?;

        for rail_idx in 0..2 {
            let main_rail = self
                .geom(g)?
                .rail(rail_idx)
                .ok_or_else(|| SolidError::null_pointer(OP_CLIFF))?;
            let (rail_eu, rail_mate_eu) = self.p5_prim_mate(main_rail, OP_CLIFF)?;
            let orig_rail_curve = edge::curve(self.brep(), main_rail)?
                .cloned()
                .ok_or_else(|| SolidError::null_pointer(OP_CLIFF))?;
            let orig_rail_ivl = orig_rail_curve.natural_interval();
            let base_uv_trim_curve = self
                .brep()
                .edgeuse(rail_mate_eu)?
                .uv_trim_curve
                .clone()
                .filter(is_b_spline);

            self.set_edgeuse_uv_curve(rail_eu, None, true, true)?;
            self.set_edgeuse_uv_curve(rail_mate_eu, None, false, false)?;
            set_edge_curve(self.brep_mut(), main_rail, None)?;

            // build pFilletUVCurve = IsoLineTrimCurve for pRailEU
            let fillet_uv_curve = {
                let t = orig_rail_ivl.evaluate(0.34567);
                let mut iso_v = fil_srf_domain.min().y;
                let mut test_uv = Point2::new(t, iso_v);
                let curve_pnt = orig_rail_curve.evaluate_point(k, t)?;
                let mut surf_pnt = fillet_surface.evaluate_point(k, test_uv)?;
                if curve_pnt.distance(surf_pnt) > tolerance {
                    iso_v = fil_srf_domain.max().y;
                    test_uv.y = iso_v;
                    surf_pnt = fillet_surface.evaluate_point(k, test_uv)?;
                    if curve_pnt.distance(surf_pnt) > tolerance {
                        return Err(SolidError::sm_err(OP_CLIFF));
                    }
                }
                let start_uv = Point2::new(orig_rail_ivl.min(), iso_v);
                let end_uv = Point2::new(orig_rail_ivl.max(), iso_v);
                let mut c = BSplineCurve::create_line_segment(2, p3(start_uv), p3(end_uv))?;
                c.edit_parameterization(&orig_rail_ivl)?;
                Curve::from(c)
            };

            let mut all_curves_this_rail = vec![main_rail];
            if rail_idx == 0 {
                all_curves_this_rail.extend_from_slice(&self.geom(g)?.other_rails1);
            } else {
                all_curves_this_rail.extend_from_slice(&self.geom(g)?.other_rails2);
            }

            for rail in all_curves_this_rail {
                let rail_type = self.fillet_edge(rail)?.fillet_edge_type();
                let prim = primary_edgeuse(self.brep(), rail)?
                    .ok_or_else(|| SolidError::null_pointer(OP_CLIFF))?;
                let (start_v, end_v) = self.edge_ends(rail, OP_CLIFF)?;
                let mate = eu1::mate(self.brep(), prim)?
                    .ok_or_else(|| SolidError::null_pointer(OP_CLIFF))?;

                for v in [start_v, end_v] {
                    let fv = self.fillet_vertex(v)?;
                    if !fv.is_processed()
                        && fv.fillet_vertex_type() == FilletVertexType::CliffRailXEdge
                    {
                        self.calc_cliff_rail_int_edge(k, v, target, &fillet_surface, tolerance)?;
                    }
                }

                let start_pnt = self.vertex_point(start_v)?;
                let end_pnt = self.vertex_point(end_v)?;

                if rail_type == FilletEdgeType::CliffRail {
                    let side_fg = if Some(start_v) == Some(eu_vertex(self.brep(), prim, OP_CLIFF)?)
                    {
                        self.eu_geom(prim)?
                    } else {
                        self.eu_geom(mate)?
                    };
                    let side_surface = if let Some(sg) = side_fg.filter(|&s| s != g) {
                        self.geom(sg)?.fillet_surface(None)?.cloned()
                    } else {
                        let face = self
                            .fillet_edge(rail)?
                            .original_face()
                            .ok_or_else(|| SolidError::null_pointer(OP_CLIFF))?;
                        target.face(face)?.surface.clone()
                    }
                    .ok_or_else(|| SolidError::null_pointer(OP_CLIFF))?;

                    let mut dir = end_pnt - start_pnt;
                    let curves = srf_srf_intersection(
                        k,
                        self,
                        Some(start_v),
                        Some(end_v),
                        Some(&mut dir),
                        None,
                        &fillet_surface,
                        &side_surface,
                        tolerance,
                        angle_tol,
                        false,
                        None,
                    )?;
                    let curve = curves
                        .curve_3d
                        .ok_or_else(|| SolidError::null_pointer(OP_CLIFF))?;
                    let ivl = curve.natural_interval();
                    set_edge_curve(self.brep_mut(), rail, Some(curve))?;
                    edge::set_interval(self.brep_mut(), rail, &ivl)?;
                    self.set_uv(prim, curves.uv_curve1)?;
                    self.set_uv(mate, None)?;
                } else if rail_type == FilletEdgeType::Rail {
                    if start_v == end_v {
                        set_edge_curve(self.brep_mut(), rail, Some(orig_rail_curve.clone()))?;
                        self.set_uv(prim, Some(fillet_uv_curve.clone()))?;
                        self.set_uv(mate, base_uv_trim_curve.clone())?;
                        continue;
                    }

                    let mut trim_ivl = orig_rail_ivl;
                    let mut param1 = trim_ivl.min();
                    let mut param2 = trim_ivl.max();
                    let tol = solver.approx_tol_3d;

                    if self.fillet_vertex(start_v)?.is_processed() {
                        let drop = |dist_tol: f64| -> Result<Option<f64>> {
                            match orig_rail_curve.drop_point(
                                k,
                                &trim_ivl,
                                start_pnt,
                                None,
                                dist_tol,
                                None,
                                SolverOperation::Minimize,
                            ) {
                                Ok(d) if d.success => Ok(Some(d.parameter)),
                                Err(e) if e.is_unported() => Err(e),
                                _ => Ok(None),
                            }
                        };
                        param1 = match drop(tol)? {
                            Some(t) => t,
                            None => {
                                drop(10.0 * tol)?.ok_or_else(|| SolidError::sm_err(OP_CLIFF))?
                            }
                        };
                    }

                    if self.fillet_vertex(end_v)?.is_processed() {
                        let opts = rail_solve_options(&orig_rail_curve);
                        let sols = orig_rail_curve.global_point_solve(
                            k,
                            &trim_ivl,
                            SolverOperation::Minimize,
                            end_pnt,
                            tol,
                            SolutionRequested::All,
                            &opts,
                        )?;
                        let first = sols.get(0).ok_or_else(|| SolidError::sm_err(OP_CLIFF))?;
                        param2 = first.start.parameters[0];
                        let mut best = first.start.solution_value;
                        for s in sols.as_slice().iter().skip(1) {
                            if s.start.parameters[0] > param1
                                && s.start.solution_value < best + EFF_ZERO
                            {
                                param2 = s.start.parameters[0];
                                best = s.start.solution_value;
                            }
                        }
                    }

                    if param1 > param2 {
                        return Err(SolidError::sm_err(OP_CLIFF));
                    }
                    ignore_non_unported(trim_ivl.set_min_max(param1, param2))?;
                    let mut copy_curve = orig_rail_curve.copy(k)?;
                    copy_curve.trim(k, &mut trim_ivl, TrimMode::Notify)?;
                    set_edge_curve(self.brep_mut(), rail, Some(copy_curve))?;
                    edge::set_interval(self.brep_mut(), rail, &trim_ivl)?;

                    let mut copy_fillet_uv = fillet_uv_curve.copy(k)?;
                    copy_fillet_uv.trim(k, &mut trim_ivl, TrimMode::Notify)?;
                    self.set_uv(prim, Some(copy_fillet_uv))?;
                    if let Some(base) = &base_uv_trim_curve {
                        let mut copy_base = base.copy(k)?;
                        copy_base.trim(k, &mut trim_ivl, TrimMode::Notify)?;
                        self.set_uv(mate, Some(copy_base))?;
                    }
                }
            }
        }
        Ok(())
    }

    /// `SmFilletGeom::UpdateSplitFace(pOrigFace, pNewFace)`: updates the split faces
    /// of every rail, other rail and side edge of geometry `g`
    /// (`SmFilletEdge::UpdateSplitFace`).
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` for a missing rail (upstream calls through `NULL`); those of
    /// `SmFilletEdge::UpdateSplitFace` (`blend.fillet_geom_p2`, wired; its status is
    /// ignored upstream, so only [`SolidError::Unported`] propagates). `target` and
    /// `corners` are those of [`FilletBrep::update_edge_split_face`].
    pub fn update_geom_split_face(
        &mut self,
        k: &Kernel,
        g: FilletGeomId,
        orig_face: Option<FaceId>,
        new_face: Option<FaceId>,
        target: &mut Brep,
        corners: &[FilletCorner],
    ) -> Result<()> {
        // For both rails:
        for ii in 0..2 {
            // Collect all extra rails for this rail.
            let geom = self.geom(g)?;
            let mut all_rails = vec![geom
                .rail(ii)
                .ok_or_else(|| SolidError::null_pointer(OP_SPLIT))?];
            all_rails.extend_from_slice(if ii == 0 {
                &geom.other_rails1
            } else {
                &geom.other_rails2
            });
            for rail in all_rails {
                fillet_edge_update_split_face(self, k, rail, orig_face, new_face, target, corners)?;
            }
        }

        // Also SideEUs.
        for feu in self.geom(g)?.side_eus.clone() {
            if let Some(fe) = self.brep().edgeuse(feu)?.edge {
                if self.fillet_edge(fe).is_ok() {
                    fillet_edge_update_split_face(
                        self, k, fe, orig_face, new_face, target, corners,
                    )?;
                }
            }
        }
        Ok(())
    }
}

/// `DropVectors(uv, TRUE, TRUE, 1, &direction, &uv_dir)`.
fn drop_vector(k: &Kernel, surface: &Surface, uv: Point2, direction: Vec3) -> Result<Vec2> {
    let dropped = drop_vectors_at(k, surface, uv, Side::Left, Side::Left, &[direction])?;
    dropped
        .first()
        .copied()
        .ok_or_else(|| SolidError::null_pointer(OP_END))
}

/// `(void*)pFace->GetSurface()` as an intersection point's user pointer (`None` for a
/// face without surface).
fn user_surface(
    target: &Brep,
    face: FaceId,
) -> Result<Option<crate::intersect::surface_intersector::p1::UserSurface>> {
    Ok(target
        .face(face)?
        .surface
        .clone()
        .map(|s| crate::intersect::surface_intersector::p1::UserSurface(std::sync::Arc::new(s))))
}

/// `SmTol::GetZoneTol3d(pCurve)` for a curve held by an edge, or by an edge use (a UV
/// trim curve, whose `SmCurve::GetEdge` is the edge use's owning edge): that edge's
/// tolerance (`SmEdge::GetTolerance`, zero on a fillet B-rep's rails); the system zone
/// tolerance when there is no edge. `SmBSplineCurve::Trim` maps it to parameter space
/// to find degenerate end spans to remove.
pub(super) fn held_curve_zone_tol_3d(k: &Kernel, b: &Brep, holder: Option<EdgeId>) -> Result<f64> {
    Ok(match holder {
        Some(e) => edge::tolerance(b, e)?,
        None => crate::geom::tol::zone_tol_3d(k, crate::geom::tol::SmallTopoType::NormalSize),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kernel() -> Kernel {
        Kernel::default()
    }

    fn fb_with_points(k: &Kernel, pts: &[Point3]) -> (FilletBrep, Vec<VertexId>) {
        let mut fb = FilletBrep::new();
        let mut ids = Vec::new();
        for &p in pts {
            let v = fb.brep_mut().add_vertex(p).unwrap();
            fb.insert_fillet_vertex(v, FilletVertex::new(k, None));
            ids.push(v);
        }
        (fb, ids)
    }

    #[test]
    fn int_fillet_without_corner_is_null() {
        let k = kernel();
        let (mut fb, v) = fb_with_points(&k, &[Point3::ZERO]);
        let e = fb
            .calc_fillet_int_fillet(&k, v[0], CornerLookup::default(), &[], &Brep::new())
            .unwrap_err();
        assert!(matches!(e, SolidError::Upstream { .. }));
    }

    #[test]
    fn int_fillet_with_corner_reads_the_corner() {
        let k = kernel();
        let (mut fb, v) = fb_with_points(&k, &[Point3::ZERO]);
        fb.fillet_vertex_mut(v[0])
            .unwrap()
            .set_fillet_corner(Some(FilletCornerId(0)));
        // without a corner store: an empty table
        let e = fb
            .calc_fillet_int_fillet(&k, v[0], CornerLookup::default(), &[], &Brep::new())
            .unwrap_err();
        assert!(matches!(
            e,
            SolidError::Upstream {
                code: crate::error::SM_ERR_NULL_POINTER,
                ..
            }
        ));
        // a corner in the table: no fillet/fillet edge at the vertex
        let corners = [FilletCorner::new(
            crate::blend::fillet_corner::p3::FilletCornerKind::TwoByTwo,
            None,
            None,
            None,
            1e-6,
            1e-3,
        )];
        let e = fb
            .calc_fillet_int_fillet(&k, v[0], CornerLookup::table(&corners), &[], &Brep::new())
            .unwrap_err();
        assert!(matches!(e, SolidError::Upstream { .. }), "{e:?}");
    }

    #[test]
    fn make_fillet_edge_needs_vertices_then_region() {
        let k = kernel();
        let (mut fb, v) = fb_with_points(&k, &[Point3::ZERO, Point3::new(1.0, 0.0, 0.0)]);
        assert!(matches!(
            fb.make_fillet_edge(None, Some(v[1]), None).unwrap_err(),
            SolidError::Upstream { .. }
        ));
        // FilletBrep::new has its infinite region (SmBrep(), topology.brep_p3)
        fb.make_fillet_edge(Some(v[0]), Some(v[1]), None).unwrap();
    }

    #[test]
    fn make_rail_edgeuse_checks_solver_and_vertex_then_allocates() {
        let k = kernel();
        let (mut fb, v) = fb_with_points(&k, &[Point3::ZERO]);
        let g = fb.add_geom(FilletGeom::new(&k, None, FilletGeomType::Default).unwrap());
        let target = Brep::new();
        // NER(pFilletSolver)
        assert!(matches!(
            fb.make_rail_edgeuse(
                g,
                0,
                Some(v[0]),
                OrientType::Same,
                None,
                CornerLookup::default(),
                &[],
                &target
            )
            .unwrap_err(),
            SolidError::Upstream { .. }
        ));
        let solvers = [FilletSolver::new(1e-6, 1e-3, 1e-3)];
        fb.geom_mut(g).unwrap().solver = Some(FilletSolverId(0));
        // NER(pFilletVertex)
        assert!(matches!(
            fb.make_rail_edgeuse(
                g,
                0,
                None,
                OrientType::Same,
                None,
                CornerLookup::default(),
                &solvers,
                &target
            )
            .unwrap_err(),
            SolidError::Upstream { .. }
        ));
        // the edge use is allocated in the infinite region (SmBrep(), topology.brep_p3);
        // the bare fillet geometry then fails upstream, no longer as Unported
        assert!(matches!(
            fb.make_rail_edgeuse(
                g,
                0,
                Some(v[0]),
                OrientType::Same,
                None,
                CornerLookup::default(),
                &solvers,
                &target
            )
            .unwrap_err(),
            SolidError::Upstream { .. }
        ));
    }

    #[test]
    fn trim_cliff_rails_rejects_other_types() {
        let k = kernel();
        let mut fb = FilletBrep::new();
        let g = fb.add_geom(FilletGeom::new(&k, None, FilletGeomType::Default).unwrap());
        assert!(matches!(
            fb.trim_cliff_rails(&k, g, &[], &Brep::new()).unwrap_err(),
            SolidError::Upstream { .. }
        ));
    }

    #[test]
    fn update_split_face_without_rail_is_null() {
        let k = kernel();
        let mut fb = FilletBrep::new();
        let g = fb.add_geom(FilletGeom::new(&k, None, FilletGeomType::Default).unwrap());
        assert!(matches!(
            fb.update_geom_split_face(&k, g, None, None, &mut Brep::new(), &[])
                .unwrap_err(),
            SolidError::Upstream { .. }
        ));
    }
}
