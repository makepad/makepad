//! Fillet geometry, part 6: the fillet edge calculators of blending rails, edges on a
//! target edge, fillet ends and set-back rails; which face of a split pair a fillet
//! edge lies on; joining two fillet vertices by side curves; copying a fillet
//! geometry; the rail lookups and rail-point setup of a fillet geometry; moving an
//! intersection point in an ordered list; the rail/edge-use intersection on a
//! geometry's surfaces; trimming the original surfaces; and reporting a fillet error.
//!
//! Upstream: `source/SMLib/src/SmFilletGeom.cpp`, `source/SMLib/inc/SmFilletGeom.h`.
//!
//! | Rust                                         | Upstream                                  |
//! |----------------------------------------------|-------------------------------------------|
//! | [`FilletBrep::calc_blending_rail`]           | `SmFilletEdge::CalcBlendingRail`          |
//! | [`FilletBrep::calc_edge_on_edge`]            | `SmFilletEdge::CalcEdgeOnEdge`            |
//! | [`FilletBrep::calc_fillet_end`]              | `SmFilletEdge::CalcFilletEnd`             |
//! | [`FilletBrep::calc_set_back_rail`]           | `SmFilletEdge::CalcSetBackRail`           |
//! | [`FilletBrep::should_swap_faces`]            | `SmFilletEdge::ShouldSwapFaces`           |
//! | [`join_two_fillet_verts_by_curves`]          | `JoinTwoFilletVertsByCurves`              |
//! | [`FilletBrep::copy_geom`]                    | `SmFilletGeom::Copy`                      |
//! | [`FilletBrep::find_edge_tangent_at_point`]   | `SmFilletGeom::FindEdgeTangentAtPoint`    |
//! | [`FilletBrep::rail_index_of_side_eu`]        | `SmFilletGeom::GetRailIndexOfSideEU`      |
//! | [`FilletBrep::rail_index_of_vertexuse`]      | `SmFilletGeom::GetRailIndexOfVertexuse`   |
//! | [`FilletBrep::rail_vertex`]                  | `SmFilletGeom::GetRailVertex`             |
//! | [`FilletBrep::set_up_rail_point`]            | `SmFilletGeom::SetUpRailPoint`            |
//! | [`move_tsp`]                                 | `sm_MoveTSP`                              |
//! | [`FilletBrep::rail_edgeuse_intersect`]       | `SmFilletGeom::RailEdgeuseIntersect`      |
//! | [`FilletBrep::trim_original_surfaces`]       | `SmFilletGeom::TrimOriginalSurfaces`      |
//! | [`FilletBrep::record_fillet_error`], [`FilletBrep::record_fillet_error_in`] | `SmFilletGeom::RecordFilletError` |
//! | [`FilletBrep::new`](super::p1::FilletBrep::new) (in part 1) | `SmFilletBrep::SmFilletBrep` (CO-PORTED in `blend.fillet_geom_p1`) |
//!
//! Deviations:
//! - **Object model.** As in parts 1 and 3, `this` is an id of the pseudo B-rep
//!   [`FilletBrep`] (`self`); the B-rep being filleted is `target` (the fillet edges'
//!   original edges and faces, the vertices' classification objects, the solvers' edge
//!   uses). A geometry's `GetFilletSolver()` is resolved in the caller's `solvers`
//!   table (index [`FilletSolverId`]`.0`), as in part 3. `SmFilletCorner*` arguments
//!   are [`FilletCorner`]s (`m_cpVertex` for `GetFilletedVertex`, `corner_type()`,
//!   `approx_tol_3d` for `GetThisApproxTol3d`); a fillet edge's own corner ([`FilletCornerId`]) is resolved in the caller's `corners`
//!   table the same way. `SmFilletGeom::GetOffsetSurface` falls back to the geometry's
//!   [`FilletSolver`] (`FilletSolver::surface`), as part 3 does.
//! - **Curves.** `SmEdge::SetCurve(pCurve, TRUE)` on a pseudo-B-rep edge is part 1's
//!   notification reset (UV trim curves, drop-curve records and cached gaps of the
//!   edge's uses), then the store; `pNew3DCurve->SetOwner(this)` is not modelled (no
//!   pseudo-B-rep curve owner is ever read). `SM_CAST_PTR(SmBSplineCurve, ...)` and
//!   `SM_CAST_PTR(SmBSplineSurface, ...)` keep what `IsKindOf` the B-spline types
//!   (lines and conics, planes and surfaces of revolution are B-spline subclasses
//!   upstream) and are `NULL` otherwise (here `SM_ERR_NULL_POINTER` at the `NER`).
//!   Copy constructors of
//!   UV trim curves are clones. `JoinTwoFilletVertsByCurves` returns its curve lists
//!   as copies (`Option<Curve>`: a `NULL` curve pointer is `None`).
//! - **Arguments.** Out-params become return values; `SmContext` arguments are the
//!   [`Kernel`]; `iDebugLevel` and debug drawing are dropped. The unused corner of
//!   `CalcEdgeOnEdge` (`SM_REF1`) is dropped. `sm_MoveTSP` reorders any list whose
//!   items carry an intersection point (upstream: pointers to them).
//!   `RailEdgeuseIntersect` takes its geometry's solver (the geometry's
//!   [`FilletGeomId`] is what `SetFilletGeomForSurfaces(this)` stores) and the
//!   intersection point in/out.
//!   `TrimOriginalSurfaces` takes, as [`TrimOriginalSurfacesEnv`], what it reads from
//!   the solver's executive (`GetFilletExecutive()->GetTopologyIntersector()`'s
//!   `GetBrepMate` for the fillet B-rep's edges), the fillet B-rep (`m_pFilletBrep`,
//!   holding the rails' `m_pFilletBrepEdge1`), the marks of the target and fillet
//!   B-reps and the mark-lock index of the target B-rep's context
//!   (`pMatchedOrigFace->GetContext()`); upstream's three possible B-reps reduce to
//!   one target B-rep.
//!   `CalcBlendingRail` takes the target as `&mut Brep`: the original face's surface
//!   is solved, lifted and dropped onto as a copy through the B-rep's
//!   `BrepFaceServices` (handle `CacheObject::Surface(face)`), so its surface cache is
//!   the face's trimmed cache, made on the B-rep as upstream's `smsurf_GetSurfaceCache`
//!   makes it through `GetFace()`.
//! - **Statuses.** `SER`/`NER` failures are errors (`SM_ERR`, `SM_ERR_NULL_POINTER`);
//!   ignored upstream statuses (curve and surface copies, `SetMinMax`, `Unitize`,
//!   `EvaluatePoint` in `ShouldSwapFaces`) are ignored here too, except that
//!   [`SolidError::Unported`] always propagates. A `NULL` dereference upstream (a
//!   missing primary edge use, mate, vertex, rail, curve, surface, solver, corner
//!   vertex or executive) is `SM_ERR_NULL_POINTER` or [`SolidError::InvalidInput`];
//!   an index past an array (a rail index above 1) or a classification object of the
//!   wrong kind where upstream casts blindly is [`SolidError::InvalidInput`].
//! - **Upstream defect kept.** `CalcBlendingRail`'s Hermite branch asks
//!   `CreateCanonical` for degree 2 with the four Bezier points and end multiplicities
//!   4, as upstream; the quadratic it makes drops the start-tangent point
//!   (`plan/upstream-issues.md` WR-190).
//! - **Iteration.** No recursion; `JoinTwoFilletVertsByCurves` removes a side edge use
//!   per step, so it runs at most once per side edge use.

use crate::blend::fillet_corner::p3::FilletCorner;
use crate::blend::fillet_custom_solver::OffsetSurface;
use crate::blend::fillet_executive::p3::ErrorInfo;
use crate::blend::fillet_solver::p1::{FilletSolver, FilletSolverOps, RolloverGuesses};
use crate::curve::curve::p2::BSPLINE_CURVE_TYPE;
use crate::curve::curve::Curve;
use crate::curve::hermite_curve::HermiteCurve;
use crate::error::{ignored, EntityRef, Result, SolidError};
use crate::geom::context::{MarkLock, MarkType};
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::geom_utility::p1::line_line_closest_point;
use crate::geom::global_solver::{SolutionRequested, SolverOperation};
use crate::geom::object::ObjectRef;
use crate::geom::solution_array::{SolutionArray, SolutionObject};
use crate::geom::vector2d::{Point2, Vec2};
use crate::intersect::point_class::{PointClassType, PointClassification};
use crate::intersect::surface_drop_lib::p3::DropSolveContext;
use crate::intersect::surface_intersector::p1::{IntersectionPointType, SrfIndex, TsectPnt};
use crate::kernel::Kernel;
use crate::math::{Point3, Vec3, EFF_ZERO, EFF_ZERO_SQ, EFF_ZERO_SQRT};
use crate::nurbs::b_spline_curve::p1::{BSplineCurveForm, CanonicalData, KnotType};
use crate::nurbs::b_spline_curve::BSplineCurve;
use crate::nurbs::b_spline_curve_n_lib::TrimMode;
use crate::nurbs::b_spline_surface::p1::SurfParamType;
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::nurbs::knots::Side;
use crate::query::brep::BrepFaceServices;
use crate::surface::surface::p2::B_SPLINE_SURFACE_TYPE;
use crate::surface::surface::p4::NoObject;
use crate::surface::surface::p7::drop_vectors_at;
use crate::surface::surface::Surface;
use crate::topology::brep_cache::CacheObject;
use crate::topology::edge;
use crate::topology::edgeuse::p1::{self as eu1, primary_edgeuse};
use crate::topology::edgeuse::p3::{evaluate_binormal, BinormalOptions};
use crate::topology::face::p2 as face2;
use crate::topology::topology_traverser::{collect_faces, Entity, FaceTraversalMarks, Marks};
use crate::topology::{Brep, EdgeId, EdgeuseId, FaceId, VertexId, VertexuseId};

use super::p1::{
    classify_face_point, interpolate_endpts_and_tangents, is_order_correct, set_rail_curve,
    FilStatus, FilletBrep, FilletCornerId, FilletCornerType, FilletEdgeType, FilletErrorType,
    FilletGeomId, FilletGeomType, FilletSolverId, FilletVertexType,
};
use super::p4::solver_of;

const OP_BLENDING: &str = "blend::fillet_geom::p6::FilletBrep::calc_blending_rail";
const OP_ON_EDGE: &str = "blend::fillet_geom::p6::FilletBrep::calc_edge_on_edge";
const OP_END: &str = "blend::fillet_geom::p6::FilletBrep::calc_fillet_end";
const OP_SETBACK: &str = "blend::fillet_geom::p6::FilletBrep::calc_set_back_rail";
const OP_SWAP: &str = "blend::fillet_geom::p6::FilletBrep::should_swap_faces";
const OP_JOIN: &str = "blend::fillet_geom::p6::join_two_fillet_verts_by_curves";
const OP_COPY: &str = "blend::fillet_geom::p6::FilletBrep::copy_geom";
const OP_TANGENT: &str = "blend::fillet_geom::p6::FilletBrep::find_edge_tangent_at_point";
const OP_SIDE_EU: &str = "blend::fillet_geom::p6::FilletBrep::rail_index_of_side_eu";
const OP_RAIL_VU: &str = "blend::fillet_geom::p6::FilletBrep::rail_index_of_vertexuse";
const OP_RAIL_V: &str = "blend::fillet_geom::p6::FilletBrep::rail_vertex";
const OP_SETUP: &str = "blend::fillet_geom::p6::FilletBrep::set_up_rail_point";
const OP_MOVE: &str = "blend::fillet_geom::p6::move_tsp";
const OP_RAIL_EU: &str = "blend::fillet_geom::p6::FilletBrep::rail_edgeuse_intersect";
const OP_TRIM: &str = "blend::fillet_geom::p6::FilletBrep::trim_original_surfaces";
const OP_ERROR: &str = "blend::fillet_geom::p6::FilletBrep::record_fillet_error";

/// Object handle recorded by [`FilletBrep::record_fillet_error_in`] into the
/// executive's [`ErrorInfo`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeomErrorObject {
    /// Target B-rep filleted edge (`pFilletedEdge`).
    FilletedEdge(EdgeId),
    /// Target B-rep side face (`pTargetFace`).
    SideFace(FaceId),
    /// Pseudo-B-rep fillet rail edge (`pRail`).
    FilletEdge(EdgeId),
    /// Fillet surface of geometry `g` (`pFilletSurface`).
    FilletSurface(FilletGeomId),
}

fn invalid(operation: &'static str, reason: &'static str) -> SolidError {
    SolidError::invalid_input(operation, reason)
}

/// The corner `id` of the caller's table.
pub(crate) fn corner_of<'c>(
    corners: &'c [FilletCorner],
    id: FilletCornerId,
    op: &'static str,
) -> Result<&'c FilletCorner> {
    usize::try_from(id.0)
        .ok()
        .and_then(|i| corners.get(i))
        .ok_or_else(|| invalid(op, "fillet corner id not in the corner table"))
}

/// `SM_CAST_PTR(SmBSplineCurve, pCopy)` after an ignored `Copy`: the copy when it is a
/// B-spline curve, else `NULL`.
fn copy_as_b_spline(k: &Kernel, curve: &Curve) -> Result<Option<Curve>> {
    Ok(ignored(curve.copy(k))?.filter(|c| c.is_kind_of(BSPLINE_CURVE_TYPE)))
}

/// `SmCurve::Evaluate(t, 1, TRUE, aPV)`: the position and first derivative.
fn evaluate_tangent(k: &Kernel, curve: &Curve, t: f64, op: &'static str) -> Result<(Point3, Vec3)> {
    let pv = curve.evaluate(k, t, 1, true, true)?;
    match pv.as_slice() {
        [p, d, ..] => Ok((*p, *d)),
        _ => Err(SolidError::sm_err(op)),
    }
}

/// `DropVectors(uv, TRUE, TRUE, 1, &v, &uvv)`.
fn drop_vector(
    k: &Kernel,
    surface: &Surface,
    uv: Point2,
    v: Vec3,
    op: &'static str,
) -> Result<Vec2> {
    drop_vectors_at(k, surface, uv, Side::Left, Side::Left, &[v])?
        .first()
        .copied()
        .ok_or_else(|| SolidError::sm_err(op))
}

/// The UV point of the first solution of a surface point solve.
fn first_uv<O: SolutionObject>(solutions: &SolutionArray<O>, op: &'static str) -> Result<Point2> {
    let s = solutions.get(0).ok_or_else(|| SolidError::sm_err(op))?;
    Ok(Point2::new(s.start.param(0)?, s.start.param(1)?))
}

/// `SmCurve::DropPoint(ivl, pnt, NULL, dTol, NULL, bSuccess, dParam, dDist)` with the
/// default `SM_SO_MINIMIZE`: the parameter when the call succeeds and finds a point,
/// `None` otherwise ([`SolidError::Unported`] propagates).
fn drop_param(
    k: &Kernel,
    curve: &Curve,
    ivl: &Extent1d,
    pnt: Point3,
    tol: f64,
) -> Result<Option<f64>> {
    match curve.drop_point(k, ivl, pnt, None, tol, None, SolverOperation::Minimize) {
        Ok(d) if d.success => Ok(Some(d.parameter)),
        Err(e) if e.is_unported() => Err(e),
        _ => Ok(None),
    }
}

/// `SM_CAST_PTR(SmBSplineSurface, pCopy)` after an ignored `Copy`: the copy when it
/// `IsKindOf(SmBSplineSurface_TYPE)`, else `NULL`.
fn copy_as_b_spline_surface(k: &Kernel, surface: &Surface) -> Result<Option<Surface>> {
    match ignored(surface.copy(k))? {
        Some(s) if s.is_kind_of(B_SPLINE_SURFACE_TYPE)? => Ok(Some(s)),
        _ => Ok(None),
    }
}

/// `sm_MoveTSP(rTsectPnts, lFrom, lTo)`: when the points at `from` and `to` are out of
/// order along the curve (`sm_IsOrderCorrect` of the smaller and larger index), moves
/// the item at `from` to `to` (a swap for a list of two). `tsect` reads an item's
/// intersection point.
///
/// # Errors
///
/// `SM_ERR` for an index past the list; those of `tsect`.
pub fn move_tsp<T>(
    tsect_pnts: &mut Vec<T>,
    from: usize,
    to: usize,
    tsect: impl Fn(&T) -> Result<TsectPnt>,
) -> Result<()> {
    let size = tsect_pnts.len();
    if from >= size || to >= size {
        return Err(SolidError::sm_err(OP_MOVE));
    }

    let lo = from.min(to);
    let hi = from.max(to);
    let pair = [
        tsect(
            tsect_pnts
                .get(lo)
                .ok_or_else(|| SolidError::sm_err(OP_MOVE))?,
        )?,
        tsect(
            tsect_pnts
                .get(hi)
                .ok_or_else(|| SolidError::sm_err(OP_MOVE))?,
        )?,
    ];
    if is_order_correct(&pair, 0, 1)? {
        return Ok(());
    }

    // It's just pointers, so just swap them.
    if size == 2 {
        tsect_pnts.swap(from, to);
    } else {
        let tsp_temp = tsect_pnts.remove(from);
        tsect_pnts.insert(to, tsp_temp);
    }
    Ok(())
}

/// The inputs of [`FilletBrep::trim_original_surfaces`] that upstream reaches through
/// the solver's executive and the B-reps' contexts.
pub struct TrimOriginalSurfacesEnv<'a> {
    /// `GetFilletExecutive()->GetTopologyIntersector().GetBrepMate(pFBEdge)`: the
    /// target B-rep edge mated to a fillet B-rep edge.
    pub brep_mate: &'a dyn Fn(EdgeId) -> Option<EdgeId>,
    /// The identity each fillet B-rep edge owns its curve under (`GetOwner() ==
    /// pFBEdge`).
    pub fillet_edge_key: &'a dyn Fn(EdgeId) -> ObjectRef,
    /// The index in the mark locks of the target B-rep's context
    /// (`pMatchedOrigFace->GetContext()`, found with `sm_FindMarkForContext2`).
    pub target_context: Option<usize>,
}

impl core::fmt::Debug for TrimOriginalSurfacesEnv<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("TrimOriginalSurfacesEnv")
            .field("target_context", &self.target_context)
            .finish_non_exhaustive()
    }
}

/// The B-rep whose topology marks a trim traversal reads or writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrimOriginalBody {
    /// The constructed fillet B-rep.
    Fillet,
    /// The original target B-rep.
    Target,
}

/// Borrowed mark authority for the two B-reps in `TrimOriginalSurfaces`.
/// Implementations retain writes, including those made before an error.
pub trait TrimOriginalSurfacesMarks {
    /// Query one body's entity using the current kernel mark value.
    fn is_marked(&self, k: &Kernel, body: TrimOriginalBody, entity: Entity, mark: MarkType)
        -> bool;
    /// Mark one body's entity using the current kernel mark value.
    fn mark(&mut self, k: &Kernel, body: TrimOriginalBody, entity: Entity, mark: MarkType);
}

struct TrimTraversalMarks<'a, M> {
    marks: &'a mut M,
    body: TrimOriginalBody,
}

impl<M: TrimOriginalSurfacesMarks> FaceTraversalMarks for TrimTraversalMarks<'_, M> {
    fn is_marked(&self, k: &Kernel, entity: Entity, mark: MarkType) -> bool {
        self.marks.is_marked(k, self.body, entity, mark)
    }

    fn mark(&mut self, k: &Kernel, entity: Entity, mark: MarkType) {
        self.marks.mark(k, self.body, entity, mark);
    }
}

struct IndexedTrimMarks<'a> {
    fillet: &'a mut Marks,
    target: &'a mut Marks,
}

impl TrimOriginalSurfacesMarks for IndexedTrimMarks<'_> {
    fn is_marked(
        &self,
        k: &Kernel,
        body: TrimOriginalBody,
        entity: Entity,
        mark: MarkType,
    ) -> bool {
        match body {
            TrimOriginalBody::Fillet => self.fillet.is_marked(k, entity, mark),
            TrimOriginalBody::Target => self.target.is_marked(k, entity, mark),
        }
    }

    fn mark(&mut self, k: &Kernel, body: TrimOriginalBody, entity: Entity, mark: MarkType) {
        match body {
            TrimOriginalBody::Fillet => self.fillet.mark(k, entity, mark),
            TrimOriginalBody::Target => self.target.mark(k, entity, mark),
        }
    }
}

/// `JoinTwoFilletVertsByCurves`: appends to the ordered curve lists the curves of a
/// chain of `side_eus` edges that leads from `start_fv` to `end_fv`, each with its UV
/// trim curve (`GetUVTrimCurve`: the use's or its mate's) and its orientation along
/// the chain. When every UV curve of the lists is present the chain is tested in UV
/// space: a side edge whose UV start is more than `uv_tol` from the chain's end is
/// skipped beyond `20 * uv_tol`, or when the fillet surface puts the two points more
/// than ten times the solver's tolerance apart. Returns `rbMadeConnection`: whether
/// the chain reached `end_fv`. `g` is the fillet geometry (`fb`'s), `solvers` resolves
/// its solver.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for empty curve lists or a geometry without solver
/// (upstream reads `GetLast()` of an empty array, dereferences `NULL`);
/// `SM_ERR_NULL_POINTER` for a missing last curve, side edge or fillet surface; those
/// of the curve and surface evaluations.
#[allow(clippy::too_many_arguments)]
pub fn join_two_fillet_verts_by_curves(
    k: &Kernel,
    fb: &FilletBrep,
    g: FilletGeomId,
    start_fv: Option<VertexId>,
    end_fv: Option<VertexId>,
    side_eus: &[EdgeuseId],
    uv_tol: f64,
    curves_3d: &mut Vec<Option<Curve>>,
    uv_curves: &mut Vec<Option<Curve>>,
    curve_orients: &mut Vec<OrientType>,
    solvers: &[FilletSolver],
) -> Result<bool> {
    join_two_fillet_verts_by_curves_resolved(
        k,
        fb,
        g,
        start_fv,
        end_fv,
        side_eus,
        uv_tol,
        curves_3d,
        uv_curves,
        curve_orients,
        &|id, op| solver_of(solvers, id, op),
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn join_two_fillet_verts_by_curves_resolved<'s>(
    k: &Kernel,
    fb: &FilletBrep,
    g: FilletGeomId,
    start_fv: Option<VertexId>,
    end_fv: Option<VertexId>,
    side_eus: &[EdgeuseId],
    uv_tol: f64,
    curves_3d: &mut Vec<Option<Curve>>,
    uv_curves: &mut Vec<Option<Curve>>,
    curve_orients: &mut Vec<OrientType>,
    resolve: &impl Fn(FilletSolverId, &'static str) -> Result<&'s FilletSolver>,
) -> Result<bool> {
    let nul = || SolidError::null_pointer(OP_JOIN);

    // locals
    let geom = fb.geom(g)?;
    let fillet_surface = geom.fillet_surface(None)?;
    let solver_id = geom
        .fillet_solver()
        .ok_or_else(|| invalid(OP_JOIN, "fillet geometry without solver"))?;
    let d3d_tolerance = resolve(solver_id, OP_JOIN)?.approx_tol_3d;

    // when all UVTrimCurves are present - do 2D testing else do more expensive 3D testing
    let do_2d_test = uv_curves.iter().all(Option::is_some);

    // Find the 3d or UV point at the end of the input OrderedEdge List
    let last_curve_orient = *curve_orients
        .last()
        .ok_or_else(|| invalid(OP_JOIN, "empty orientation list"))?;
    let last_curve = if do_2d_test {
        uv_curves.last()
    } else {
        curves_3d.last()
    }
    .ok_or_else(|| invalid(OP_JOIN, "empty curve list"))?
    .as_ref()
    .ok_or_else(nul)?;
    let ivl = last_curve.natural_interval();
    let param = if last_curve_orient == OrientType::Same {
        ivl.max()
    } else {
        ivl.min()
    };
    let mut end_pnt = last_curve.evaluate_point(k, param)?;

    // Make a copy of side-EUs array
    let mut copy_side_eus = side_eus.to_vec();

    // until we connect to target EndFilletVertex
    let mut curr_fv = start_fv;
    let mut made_connection = true;
    while curr_fv != end_fv && made_connection {
        made_connection = false;

        // for every SideEdgeuse
        let mut ii = 0;
        while ii < copy_side_eus.len() {
            let side_eu = *copy_side_eus.get(ii).ok_or_else(nul)?;
            let curr_edge = eu1::edge(fb.brep(), side_eu)?.ok_or_else(nul)?;
            let curve = fb.brep().edge(curr_edge)?.curve.as_ref();
            let edge_start = edge::vertex(fb.brep(), curr_edge)?;
            let edge_end = edge::other_vertex(fb.brep(), curr_edge, edge_start)?;
            let side_uv_curve = eu1::uv_trim_curve(fb.brep(), side_eu)?.0;

            // skip edges not connected to the current FilletVertex
            if edge_start != curr_fv && edge_end != curr_fv {
                ii += 1;
                continue;
            }

            // Found 'NEXT' curve
            let side_ivl = side_uv_curve.map(Curve::natural_interval);
            let (orient, next_fv) = if edge_start == curr_fv {
                (OrientType::Same, edge_end)
            } else {
                (OrientType::Opposite, edge_start)
            };

            // when testing in 2D with UVTrimCurves - skip Curves with bad gaps - no
            // skips in 3D
            if let (true, Some(side_uv), Some(side_ivl)) = (do_2d_test, side_uv_curve, side_ivl) {
                // get ordered endParameters
                let mut start_param = side_ivl.min();
                let mut end_param = side_ivl.max();
                if orient == OrientType::Opposite {
                    core::mem::swap(&mut end_param, &mut start_param);
                }

                // when distance between last and current curve endPoints is out of
                // tolerance
                let start_pnt = side_uv.evaluate_point(k, start_param)?;
                let uv_dist = start_pnt.distance(end_pnt);

                if uv_dist > uv_tol {
                    // skip curves with large gaps
                    if uv_dist > 20.0 * uv_tol {
                        ii += 1;
                        continue; // Too much of a gap
                    }

                    // for slightly out of tolerance gaps - test 3D distance between
                    // endPoints
                    let surface = fillet_surface.ok_or_else(nul)?;
                    let uv_start = Point2::new(start_pnt.x, start_pnt.y);
                    let uv_end = Point2::new(end_pnt.x, end_pnt.y);
                    let s3d_start_pnt = surface.evaluate_point(k, uv_start)?;
                    let s3d_end_pnt = surface.evaluate_point(k, uv_end)?;
                    let gap = s3d_start_pnt.distance(s3d_end_pnt);

                    // reject gaps larger than 10*tolerance
                    if gap > 10.0 * d3d_tolerance {
                        ii += 1;
                        continue;
                    }
                } // end 2D gap larger than tolerance check

                // found a keeper edge -increment EndPoint
                end_pnt = side_uv.evaluate_point(k, end_param)?;
            } // end 2DTest check

            // Add Edge to Arrays - prepare for next iteration
            curves_3d.push(curve.cloned());
            uv_curves.push(side_uv_curve.cloned());
            curve_orients.push(orient);
            copy_side_eus.remove(ii);
            curr_fv = next_fv;
            made_connection = true;
            break;
        } // end iter every sideEdgeuse
    } // end while trying to connect to target endFilletVertex

    Ok(curr_fv == end_fv)
}

/// `pSurf->GlobalPointSolve(sUVDomain, SM_SO_INTERSECT, crPoint, dTol, NULL, sSolutions,
/// SM_SR_SINGLE)` on a face's surface (`handle`) under the face's services, so the
/// solve's surface cache is the face's trimmed one.
fn global_point_solve_on_face(
    k: &Kernel,
    surf: &Surface,
    handle: Option<CacheObject>,
    services: &mut BrepFaceServices<'_>,
    uv_domain: &Extent2d,
    p: Point3,
    tol: f64,
) -> Result<SolutionArray<CacheObject>> {
    let mut solutions = SolutionArray::new();
    surf.global_point_solve_into_with_context(
        k,
        handle,
        None,
        services,
        uv_domain,
        SolverOperation::Intersect,
        p,
        tol,
        None,
        SolutionRequested::Single,
        &mut solutions,
    )?;
    Ok(solutions)
}

impl FilletBrep {
    // ---- SmFilletEdge -------------------------------------------------------------------

    /// The start vertex of fillet edge `e` and its other vertex (`GetVertex()`,
    /// `GetOtherVertex(pStartFV)`).
    fn edge_end_vertices(&self, e: EdgeId, op: &'static str) -> Result<(VertexId, VertexId)> {
        let start = edge::vertex(self.brep(), e)?.ok_or_else(|| SolidError::null_pointer(op))?;
        let end = edge::other_vertex(self.brep(), e, Some(start))?
            .ok_or_else(|| SolidError::null_pointer(op))?;
        Ok((start, end))
    }

    /// `SmFilletEdge::CalcBlendingRail`: the edge between two rail ends on the original
    /// face of edge `e`: a line, circular arc, parabola (or the failing Hermite) between
    /// the end points along the rails' end tangents, in UV space of the face's surface
    /// (lifted to 3D) or, for a bilinear surface, in 3D (dropped to UV). The rails'
    /// UV trim curves give the UV tangents unless an end vertex is not a mate, which is
    /// then dropped onto the surface. `target` holds the original face.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` without original face, surface, rails at the ends or
    /// primary edge use / mate / vertices / curves; `SM_ERR` (status
    /// `SM_FE_END_PNT_DROP_FAILURE`) when an end point does not drop onto the
    /// surface, or for an empty drop; those of the evaluations, the curve
    /// construction, `LiftCurve` and the 3D branch's `DropCurve`.
    #[allow(clippy::too_many_lines)]
    pub fn calc_blending_rail(&mut self, k: &Kernel, e: EdgeId, target: &mut Brep) -> Result<()> {
        let nul = || SolidError::null_pointer(OP_BLENDING);
        let face = self.fillet_edge(e)?.original_face().ok_or_else(nul)?;
        // the face's own surface (evaluated as a copy: its trimmed surface cache is made
        // on `target` through the face, as `smsurf_GetSurfaceCache` reaches `GetFace()`)
        let surf = &face2::surface(target, face)?
            .ok_or_else(nul)?
            .clone_for_evaluation()?;
        let tol = face2::tolerance(target, face)?;
        let uv_domain = surf.natural_uv_domain_with_nurb(k)?;
        let handle = Some(CacheObject::Surface(face));
        let mut services = BrepFaceServices::new(target);
        let (start_fv, end_fv) = self.edge_end_vertices(e, OP_BLENDING)?;

        let prim_eu = primary_edgeuse(self.brep(), e)?.ok_or_else(nul)?;
        let mate_eu = eu1::mate(self.brep(), prim_eu)?.ok_or_else(nul)?;

        // Calc tangent from the rail at start vertex
        let rail1 = self.first_rail_at(start_fv)?.ok_or_else(nul)?;

        let mut drop_start_point = false;
        let uv1 = if self.fillet_vertex(start_fv)?.fillet_vertex_type() == FilletVertexType::Mate {
            // In normal cases, pStartFV & pEndFV should all be SM_FV_MATE
            self.fillet_vertex(start_fv)?.original_uv()
        } else {
            // Special cases, such as in 4x2 concave case
            // Drop pStartFV onto pSurf
            let p = self.vertex_point(start_fv)?;
            let mut solutions =
                global_point_solve_on_face(k, surf, handle, &mut services, &uv_domain, p, tol)?;
            if solutions.is_empty() {
                solutions = global_point_solve_on_face(
                    k,
                    surf,
                    handle,
                    &mut services,
                    &uv_domain,
                    p,
                    1.0e2 * EFF_ZERO,
                )?;
            }
            if solutions.is_empty() {
                self.fillet_edge_mut(e)?
                    .set_status(FilStatus::EndPntDropFailure);
                return Err(SolidError::sm_err(OP_BLENDING));
            }
            drop_start_point = true;
            first_uv(&solutions, OP_BLENDING)?
        };

        let (tangent1, uv_tangent1) =
            self.rail_end_tangents(k, rail1, start_fv, surf, uv1, drop_start_point)?;

        // Calc tangent from the other rail
        let rail2 = self.first_rail_at(end_fv)?.ok_or_else(nul)?;

        let mut drop_end_point = false;
        let uv2 = if self.fillet_vertex(end_fv)?.fillet_vertex_type() == FilletVertexType::Mate {
            self.fillet_vertex(end_fv)?.original_uv()
        } else {
            let p = self.vertex_point(end_fv)?;
            let mut solutions = global_point_solve_on_face(
                k,
                surf,
                handle,
                &mut services,
                &uv_domain,
                p,
                EFF_ZERO,
            )?;
            if solutions.is_empty() {
                solutions = global_point_solve_on_face(
                    k,
                    surf,
                    handle,
                    &mut services,
                    &uv_domain,
                    p,
                    1.0e2 * EFF_ZERO,
                )?;
            }
            if solutions.is_empty() {
                self.fillet_edge_mut(e)?
                    .set_status(FilStatus::EndPntDropFailure);
                return Err(SolidError::sm_err(OP_BLENDING));
            }
            drop_end_point = true;
            first_uv(&solutions, OP_BLENDING)?
        };

        let (tangent2, uv_tangent2) =
            self.rail_end_tangents(k, rail2, end_fv, surf, uv2, drop_end_point)?;

        let len1 = tangent1.length();
        let len2 = tangent2.length();
        // SM_ASSERT(dLen1 > dTol && dLen2 > dTol) only reports; SmVector3d's operator/
        // (Vec3's `/=` has a different near-zero guard)
        let tangent1 = tangent1 / len1;
        let uv_tangent1 = uv_tangent1 / len1;
        let tangent2 = tangent2 / len2;
        let uv_tangent2 = uv_tangent2 / len2;

        let (start_pnt, mut start_vec, end_pnt, mut end_vec, curve_dim) =
            if surf.degree_with_nurb(k, SurfParamType::U)? == 1
                && surf.degree_with_nurb(k, SurfParamType::V)? == 1
            {
                (
                    self.vertex_point(start_fv)?,
                    tangent1,
                    self.vertex_point(end_fv)?,
                    tangent2,
                    3u64,
                )
            } else {
                (
                    Point3::new(uv1.x, uv1.y, 0.0),
                    Vec3::new(uv_tangent1.x, uv_tangent1.y, 0.0),
                    Point3::new(uv2.x, uv2.y, 0.0),
                    Vec3::new(uv_tangent2.x, uv_tangent2.y, 0.0),
                    2u64,
                )
            };

        // Will create a conic first
        let curve = if start_vec.is_colinear_with(end_vec, start_pnt, end_pnt) {
            BSplineCurve::create_line_segment(curve_dim, start_pnt, end_pnt)?
        } else {
            let (mut t1, mut t2) = line_line_closest_point(start_pnt, start_vec, end_pnt, end_vec)?;
            let knots = [0.0, 1.0];
            if t1 * t2 < 0.0 {
                // Can not make quadratic UV curve, create Hermite instead
                end_vec = -end_vec;
                // First, Adjust the lengths of direction vectors
                let len = start_pnt.distance(end_pnt);
                t1 = t1.abs();
                t2 = t2.abs();
                let sum = t1 + t2;
                start_vec *= len * t1 / sum;
                end_vec *= len * t2 / sum;
                let herm = HermiteCurve::new(start_pnt, start_vec, end_pnt, end_vec, curve_dim);
                let cntrl_poly = herm.bezier_points();
                let knot_mult = [4usize, 4];
                // degree 2 with four control points, as upstream
                BSplineCurve::create_canonical(&CanonicalData {
                    dim: curve_dim,
                    degree: 2,
                    control_points: &cntrl_poly,
                    form: BSplineCurveForm::Unspecified,
                    knot_multiplicities: &knot_mult,
                    knots: &knots,
                    knot_type: KnotType::Unspecified,
                    weights: None,
                    trim: None,
                })?
            } else {
                let mid_pnt = start_pnt + start_vec * t1;
                let cntrl_poly = [start_pnt, mid_pnt, end_pnt];
                let knot_mult = [3usize, 3];
                let mut weights = [1.0, 1.0, 1.0];
                if (t1 - t2).abs() < EFF_ZERO_SQRT {
                    // Make circular arc
                    let vec = end_pnt - start_pnt;
                    let angle = vec.angle_between(start_vec)?;
                    if let Some(w) = weights.get_mut(1) {
                        *w = angle.cos();
                    }
                    BSplineCurve::create_canonical(&CanonicalData {
                        dim: curve_dim,
                        degree: 2,
                        control_points: &cntrl_poly,
                        form: BSplineCurveForm::Unspecified,
                        knot_multiplicities: &knot_mult,
                        knots: &knots,
                        knot_type: KnotType::Unspecified,
                        weights: Some(&weights),
                        trim: None,
                    })?
                } else {
                    // Make parabola
                    BSplineCurve::create_canonical(&CanonicalData {
                        dim: curve_dim,
                        degree: 2,
                        control_points: &cntrl_poly,
                        form: BSplineCurveForm::Unspecified,
                        knot_multiplicities: &knot_mult,
                        knots: &knots,
                        knot_type: KnotType::Unspecified,
                        weights: None,
                        trim: None,
                    })?
                }
            }
        };

        let curve = Curve::from(curve);
        let ivl = curve.natural_interval();
        let (new_3d_curve, uv_curve) = if curve_dim == 2 {
            let lifted = surf.lift_curve_with_context(
                k,
                &uv_domain,
                &curve,
                &ivl,
                tol,
                false,
                &mut services,
                handle,
            )?;
            (lifted.curve, curve)
        } else {
            // Project 3D curve onto the plane
            let uv_curves = super::p2::surface_drop_curve_with_context(
                k,
                surf,
                &uv_domain,
                &curve,
                &ivl,
                tol,
                DropSolveContext {
                    solver: &mut services,
                    handle,
                },
            )?;
            let uv_curve = uv_curves
                .into_iter()
                .next()
                .ok_or_else(|| SolidError::sm_err(OP_BLENDING))?;
            (curve, uv_curve)
        };

        // side effect: delete current this->UVTrimCurves
        set_rail_curve(self.brep_mut(), e, new_3d_curve)?;
        self.set_edgeuse_uv_curve(mate_eu, Some(uv_curve), false, true)?;
        Ok(())
    }

    /// The 3D and UV tangents of `rail` at its end `fv` for `CalcBlendingRail`: the
    /// rail curve's derivative at the end its mate edge use leaves `fv` from, the UV
    /// derivative of that use's UV trim curve (unless `dropped` or without one: the
    /// 3D tangent dropped into the surface at `uv`), both negated at the curve's start.
    fn rail_end_tangents(
        &self,
        k: &Kernel,
        rail: EdgeId,
        fv: VertexId,
        surf: &Surface,
        uv: Point2,
        dropped: bool,
    ) -> Result<(Vec3, Vec2)> {
        let nul = || SolidError::null_pointer(OP_BLENDING);
        let rail_prim = primary_edgeuse(self.brep(), rail)?.ok_or_else(nul)?;
        let eu = eu1::mate(self.brep(), rail_prim)?.ok_or_else(nul)?;
        let orient = eu1::orientation(self.brep(), eu)?;
        let curve = self.brep().edge(rail)?.curve.as_ref().ok_or_else(nul)?;
        let ivl = curve.natural_interval();
        let rail_start = edge::vertex(self.brep(), rail)?;
        let mut get_start_tan = false;
        let mut param = ivl.max();
        if (rail_start != Some(fv) && orient == OrientType::Same)
            || (rail_start == Some(fv) && orient == OrientType::Opposite)
        {
            get_start_tan = true;
            param = ivl.min();
        }
        let (_, mut tangent) = evaluate_tangent(k, curve, param, OP_BLENDING)?;
        let uv_curve = eu1::uv_trim_curve(self.brep(), eu)?.0;
        let mut uv_tangent = match uv_curve {
            Some(c) if !dropped => {
                let (_, d) = evaluate_tangent(k, c, param, OP_BLENDING)?;
                Vec2::new(d.x, d.y)
            }
            _ => drop_vector(k, surf, uv, tangent, OP_BLENDING)?,
        };
        if get_start_tan {
            tangent = -tangent;
            uv_tangent = -uv_tangent;
        }
        Ok((tangent, uv_tangent))
    }

    /// `SmFilletEdge::CalcEdgeOnEdge`: edge `e` (of type `SM_FE_ON_EDGE`) gets a copy
    /// of its original (target) edge's B-spline curve, trimmed between its end points
    /// dropped onto it (within the target edge's interval and tolerance) and reversed
    /// when they come in descending order. The edge's uses keep no UV trim curves.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` without original edge, its curve, a B-spline copy of it,
    /// or the edge's vertices; `SM_ERR` (status `SM_FE_END_PNT_DROP_FAILURE`) when an
    /// end point does not drop; those of `Trim` and `ReverseParameterization`.
    pub fn calc_edge_on_edge(&mut self, k: &Kernel, e: EdgeId, target: &Brep) -> Result<()> {
        let nul = || SolidError::null_pointer(OP_ON_EDGE);
        // get filletEdge vertices
        let (start_fv, end_fv) = self.edge_end_vertices(e, OP_ON_EDGE)?;
        let orig_edge = self.fillet_edge(e)?.original_edge().ok_or_else(nul)?;

        // GWC: Stop Fillets from switching SmCurve objects to SmBSplineCurve objs
        let orig_curve = target.edge(orig_edge)?.curve.as_ref().ok_or_else(nul)?;
        let mut new_3d_curve = copy_as_b_spline(k, orig_curve)?.ok_or_else(nul)?;

        // Trim ThisFilletEdge->OrigCurveCopy by Start and End FilletPoints
        let mut trim_ivl = edge::interval(target, orig_edge)?;
        let tol = edge::tolerance(target, orig_edge)?;

        // drop startVertex to thisFilletEdge->OrigCurveCopy
        let start_pnt = self.vertex_point(start_fv)?;
        let Some(param1) = drop_param(k, &new_3d_curve, &trim_ivl, start_pnt, tol)? else {
            self.fillet_edge_mut(e)?
                .set_status(FilStatus::EndPntDropFailure);
            return Err(SolidError::sm_err(OP_ON_EDGE));
        };

        // drop endVertex to thisFilletEdge->OrigCurveCopy
        let end_pnt = self.vertex_point(end_fv)?;
        let Some(param2) = drop_param(k, &new_3d_curve, &trim_ivl, end_pnt, tol)? else {
            self.fillet_edge_mut(e)?
                .set_status(FilStatus::EndPntDropFailure);
            return Err(SolidError::sm_err(OP_ON_EDGE));
        };

        // set 3DCurve->interval = ordered[startDropparameter, endDropParameter]
        if param1 > param2 {
            let _ = trim_ivl.set_min_max(param2, param1);
        } else {
            let _ = trim_ivl.set_min_max(param1, param2);
        }
        new_3d_curve.trim(k, &mut trim_ivl, TrimMode::Notify)?; // may snap sIvl by tol to existing knots

        // reverse curve if needed
        if param1 > param2 {
            new_3d_curve.reverse_parameterization(k, &trim_ivl)?;
        }

        // set Curve (side effect: delete current this->UVTrimCurves)
        set_rail_curve(self.brep_mut(), e, new_3d_curve)?;
        Ok(())
    }

    /// `SmFilletEdge::CalcFilletEnd(crContext, pCorner)`: the edge across the end of a
    /// fillet surface between its two processed end vertices on the corner's side
    /// edges: both end points are dropped onto the fillet surface of the primary edge
    /// use's geometry, the side edges' tangents there (oriented away from the corner's
    /// vertex) are dropped into UV space, the UV curve interpolating ends and tangents
    /// is lifted to 3D with a tenth of the solver's tolerance, and the primary edge
    /// use (the UV curve's owner) becomes a side edge use of the geometry.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` without corner, geometry, fillet surface, side edge use
    /// curve or primary edge use / vertices; `SM_ERR` for an unprocessed end vertex,
    /// an end vertex not classified to an edge use, a zero side tangent, or (status
    /// `SM_FE_END_PNT_DROP_FAILURE`) a point that does not drop;
    /// [`SolidError::InvalidInput`] for a classification object that is not an edge
    /// use, an edge use without edge or a solver missing from `solvers`; those of the
    /// solves, evaluations, the interpolation and `LiftCurve`.
    #[allow(clippy::too_many_lines)]
    pub fn calc_fillet_end(
        &mut self,
        k: &Kernel,
        e: EdgeId,
        target: &Brep,
        corner: Option<&FilletCorner>,
        solvers: &[FilletSolver],
    ) -> Result<()> {
        self.calc_fillet_end_with_lookup(k, e, target, corner, &|id| solver_of(solvers, id, OP_END))
    }

    /// [`Self::calc_fillet_end`] resolving the geometry's solver through `resolve`
    /// (the live session's solver store) instead of an indexed table.
    ///
    /// # Errors
    ///
    /// As [`Self::calc_fillet_end`]; those of `resolve`.
    #[allow(clippy::too_many_lines)]
    pub fn calc_fillet_end_with_lookup<'solver>(
        &mut self,
        k: &Kernel,
        e: EdgeId,
        target: &Brep,
        corner: Option<&FilletCorner>,
        resolve: &impl Fn(FilletSolverId) -> Result<&'solver FilletSolver>,
    ) -> Result<()> {
        let nul = || SolidError::null_pointer(OP_END);
        let corner = corner.ok_or_else(nul)?;

        // Gather info.
        let cp_vertex = corner.vertex;

        let (start_fv, end_fv) = self.edge_end_vertices(e, OP_END)?;
        if !self.fillet_vertex(start_fv)?.is_processed()
            || !self.fillet_vertex(end_fv)?.is_processed()
        {
            return Err(SolidError::sm_err(OP_END));
        }

        let prim_eu = primary_edgeuse(self.brep(), e)?.ok_or_else(nul)?;
        let fillet_geom1 = self
            .fillet_edgeuse(prim_eu)?
            .fillet_geom()
            .ok_or_else(nul)?;
        let solver_id = self.geom(fillet_geom1)?.fillet_solver();

        let fillet_surface = self
            .geom(fillet_geom1)?
            .fillet_surface(None)?
            .ok_or_else(nul)?
            .clone();

        // Drop two end points to fillet
        let uv_domain = fillet_surface.natural_uv_domain_with_nurb(k)?;

        let p1 = self.vertex_point(start_fv)?;
        let solutions1 = fillet_surface.global_point_solve::<NoObject>(
            k,
            None,
            &uv_domain,
            SolverOperation::Minimize,
            p1,
            EFF_ZERO,
            None,
            SolutionRequested::All,
        )?;
        if solutions1.is_empty() {
            self.fillet_edge_mut(e)?
                .set_status(FilStatus::EndPntDropFailure);
            return Err(SolidError::sm_err(OP_END));
        }
        let start_uv = first_uv(&solutions1, OP_END)?;

        let p2 = self.vertex_point(end_fv)?;
        let solutions2 = fillet_surface.global_point_solve::<NoObject>(
            k,
            None,
            &uv_domain,
            SolverOperation::Minimize,
            p2,
            EFF_ZERO,
            None,
            SolutionRequested::All,
        )?;
        if solutions2.is_empty() {
            self.fillet_edge_mut(e)?
                .set_status(FilStatus::EndPntDropFailure);
            return Err(SolidError::sm_err(OP_END));
        }
        let end_uv = first_uv(&solutions2, OP_END)?;

        // Calc tangents along two side edges
        if self.fillet_vertex(start_fv)?.point_class() != PointClassType::Edgeuse {
            return Err(SolidError::sm_err(OP_END));
        }
        if self.fillet_vertex(end_fv)?.point_class() != PointClassType::Edgeuse {
            return Err(SolidError::sm_err(OP_END));
        }

        // Start:
        let tan1 = self.side_edge_tangent(k, start_fv, target, cp_vertex, true)?;
        // End:
        let tan2 = self.side_edge_tangent(k, end_fv, target, cp_vertex, false)?;

        let uv_tan1 = drop_vector(k, &fillet_surface, start_uv, tan1, OP_END)?;
        let uv_tan2 = drop_vector(k, &fillet_surface, end_uv, tan2, OP_END)?;

        // Create a UV-space curve.
        let start_pnt = Point3::new(start_uv.x, start_uv.y, 0.0);
        let mut start_vec = Vec3::new(uv_tan1.x, uv_tan1.y, 0.0);
        let end_pnt = Point3::new(end_uv.x, end_uv.y, 0.0);
        let mut end_vec = Vec3::new(uv_tan2.x, uv_tan2.y, 0.0);
        let dimension = 2;
        let uv_curve = Curve::from(interpolate_endpts_and_tangents(
            start_pnt,
            end_pnt,
            &mut start_vec,
            &mut end_vec,
            dimension,
        )?);

        let solver_id =
            solver_id.ok_or_else(|| invalid(OP_END, "fillet geometry without solver"))?;
        let approx_tol = resolve(solver_id)?.approx_tol_3d / 10.0;
        let lifted = fillet_surface.lift_curve(
            k,
            &uv_domain,
            &uv_curve,
            &uv_curve.natural_interval(),
            approx_tol,
            false,
        )?;

        // side effect: delete current this->UVTrimCurves
        set_rail_curve(self.brep_mut(), e, lifted.curve)?;
        self.set_edgeuse_uv_curve(prim_eu, Some(uv_curve), false, true)?;
        self.geom_mut(fillet_geom1)?
            .add_side_fillet_edgeuse(prim_eu);
        Ok(())
    }

    /// The tangent of the target side edge fillet vertex `fv` is classified to (at its
    /// original parameter), oriented away from the corner's vertex `cp_vertex` at the
    /// start (`is_start`) or towards it at the end, for `CalcFilletEnd`.
    fn side_edge_tangent(
        &self,
        k: &Kernel,
        fv: VertexId,
        target: &Brep,
        cp_vertex: Option<VertexId>,
        is_start: bool,
    ) -> Result<Vec3> {
        let nul = || SolidError::null_pointer(OP_END);
        let record = self.fillet_vertex(fv)?;
        let t = record.original_t_param();
        let eu = match record.point_class_object() {
            Some(EntityRef::Edgeuse(eu)) => eu,
            None => return Err(nul()),
            Some(_) => return Err(invalid(OP_END, "classification object is not an edge use")),
        };
        let e = eu1::edge(target, eu)?.ok_or_else(|| invalid(OP_END, "edge use without edge"))?;
        let curve = target.edge(e)?.curve.as_ref().ok_or_else(nul)?;

        let (_, d) = evaluate_tangent(k, curve, t, OP_END)?;
        if d.length_squared() < EFF_ZERO_SQ {
            return Err(SolidError::sm_err(OP_END));
        }
        let mut tan = d;

        let e_prim = primary_edgeuse(target, e)?
            .ok_or_else(|| invalid(OP_END, "edge without primary edge use"))?;
        let orient = eu1::orientation(target, e_prim)?;
        let v = edge::vertex(target, e)?;
        let at_corner = v == cp_vertex;
        let flip = if is_start {
            (at_corner && orient == OrientType::Same)
                || (!at_corner && orient == OrientType::Opposite)
        } else {
            (!at_corner && orient == OrientType::Same)
                || (at_corner && orient == OrientType::Opposite)
        };
        if flip {
            tan = -tan;
        }
        Ok(tan)
    }

    /// `SmFilletEdge::CalcSetBackRail(crContext, pOptCorner)`: edge `e` (a set-back rail
    /// or rail extension, without geometry) gets a trimmed copy of its geometry's rail
    /// on the edge's original face: the rail curve between the edge's end points
    /// dropped onto it, with a copy of the rail's primary UV trim curve (or the
    /// fillet surface iso line the rail runs along) for its primary use and of the
    /// rail mate's UV curve for its mate, all reversed when the end points come in
    /// descending order. For an `SM_FCR_N_x_2` corner an edge shorter than the
    /// corner's tolerance is marked to be squeezed (its rail-by-edge-use end vertex to
    /// be deleted); otherwise the primary use becomes a side edge use of the geometry.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` without original face, geometry, rail, B-spline rail
    /// curve or copy, fillet surface (when the rail has no UV curve), primary edge use /
    /// mate / vertices; `SM_ERR` when the rail runs along neither iso line, for an
    /// unprocessed end vertex, a squeezed edge whose end is not a rail/edge-use vertex,
    /// or (status `SM_FE_END_PNT_DROP_FAILURE`) when an end point does not drop;
    /// [`SolidError::InvalidInput`] for a solver missing from `solvers`; those of the
    /// evaluations, `Trim`, `ReverseParameterization` and `EditParameterization`.
    #[allow(clippy::too_many_lines)]
    pub fn calc_set_back_rail(
        &mut self,
        k: &Kernel,
        e: EdgeId,
        target: &Brep,
        opt_corner: Option<&FilletCorner>,
        solvers: &[FilletSolver],
    ) -> Result<()> {
        self.calc_set_back_rail_with_lookup(k, e, target, opt_corner, &|id| {
            solver_of(solvers, id, OP_SETBACK)
        })
    }

    /// [`Self::calc_set_back_rail`] resolving the geometry's solver through `resolve`
    /// (the live session's solver store) instead of an indexed table.
    ///
    /// # Errors
    ///
    /// As [`Self::calc_set_back_rail`]; those of `resolve`.
    #[allow(clippy::too_many_lines)]
    pub fn calc_set_back_rail_with_lookup<'solver>(
        &mut self,
        k: &Kernel,
        e: EdgeId,
        target: &Brep,
        opt_corner: Option<&FilletCorner>,
        resolve: &impl Fn(FilletSolverId) -> Result<&'solver FilletSolver>,
    ) -> Result<()> {
        let nul = || SolidError::null_pointer(OP_SETBACK);
        // locals
        let face = self.fillet_edge(e)?.original_face().ok_or_else(nul)?;
        let prim_eu = primary_edgeuse(self.brep(), e)?.ok_or_else(nul)?;
        let mate_eu = eu1::mate(self.brep(), prim_eu)?.ok_or_else(nul)?;
        let fillet_geom = self
            .fillet_edgeuse(prim_eu)?
            .fillet_geom()
            .ok_or_else(nul)?;

        let solver_id = self
            .geom(fillet_geom)?
            .fillet_solver()
            .ok_or_else(|| invalid(OP_SETBACK, "fillet geometry without solver"))?;
        let fil_solver1 = resolve(solver_id)?;
        let approx_tol = fil_solver1.approx_tol_3d;
        let rail_index = fil_solver1.find_index_of_rail_on_face(target, Some(face))?;
        let rail = self
            .geom(fillet_geom)?
            .rail(rail_index)
            .ok_or_else(|| invalid(OP_SETBACK, "fillet geometry without rail"))?;

        let rail_curve = self
            .brep()
            .edge(rail)?
            .curve
            .as_ref()
            .filter(|c| c.is_kind_of(BSPLINE_CURVE_TYPE))
            .ok_or_else(nul)?;

        let mut trim_ivl = rail_curve.natural_interval();

        let rail_prim_eu = primary_edgeuse(self.brep(), rail)?.ok_or_else(nul)?;
        let rail_mate_eu = eu1::mate(self.brep(), rail_prim_eu)?.ok_or_else(nul)?;
        let rail_uv_curve = eu1::uv_trim_curve_pointer(self.brep(), rail_prim_eu)?;
        let orig_uv_curve = eu1::uv_trim_curve_pointer(self.brep(), rail_mate_eu)?;

        // Copy original curves
        // GWC: Stop Fillets from switching SmCurve objects to SmBSplineCurve objs
        let mut new_rail_curve = copy_as_b_spline(k, rail_curve)?.ok_or_else(nul)?;

        let mut new_orig_uv_curve = orig_uv_curve.cloned();
        let mut new_rail_uv_curve = if let Some(c) = rail_uv_curve {
            c.clone()
        } else {
            // RailUVCurve as the appropriate FilletSurface IsoParameterCurve
            let surface = self
                .geom(fillet_geom)?
                .fillet_surface(None)?
                .ok_or_else(nul)?;
            let uv_domain = surface.natural_uv_domain_with_nurb(k)?;

            // compare RailPoint to a FilletSurface Point to see if rail runs along
            // sUVDomain.MinY IsoParamCurve
            let param = trim_ivl.evaluate(0.34567);
            let iso_v = uv_domain.min().y;
            let mut test_uv = Point2::new(param, iso_v);

            // evaluate a RailCurve point and a FilletSurface MinY point
            let curve_pnt = new_rail_curve.evaluate_point(k, param)?;
            let surf_pnt = surface.evaluate_point(k, test_uv)?;

            // Compare RailCurve point to FilletSurface MinY point
            if curve_pnt.distance(surf_pnt) > approx_tol {
                // evaluate FilletSurface MaxY point
                let iso_v = uv_domain.max().y;
                test_uv.y = iso_v;
                let surf_pnt = surface.evaluate_point(k, test_uv)?;

                // Compare RailCurve point to FilletSurface MaxY point
                if curve_pnt.distance(surf_pnt) > approx_tol {
                    // Rail is not syncronizing with the parametrization of the fillet
                    // surface
                    return Err(SolidError::sm_err(OP_SETBACK));
                }
            }

            // FilletSurface IsoParamCurve end points
            let start_uv = Point2::new(trim_ivl.min(), test_uv.y);
            let end_uv = Point2::new(trim_ivl.max(), test_uv.y);

            // Create UV-curve on fillet for this rail
            let mut c = Curve::from(BSplineCurve::create_line_segment(
                2,
                Point3::new(start_uv.x, start_uv.y, 0.0),
                Point3::new(end_uv.x, end_uv.y, 0.0),
            )?);
            c.edit_parameterization(k, &trim_ivl)?;
            c
        };

        // End FilletVertices for the FilletEdge
        let (v0, v1) = self.edge_end_vertices(e, OP_SETBACK)?;

        // check state - FilletVertices should be processed so that their Vertex->Points
        // are set
        if !self.fillet_vertex(v0)?.is_processed() {
            return Err(SolidError::sm_err(OP_SETBACK));
        }
        if !self.fillet_vertex(v1)?.is_processed() {
            return Err(SolidError::sm_err(OP_SETBACK));
        }

        // End FilletVertex Points
        let pnt0 = self.vertex_point(v0)?;
        let pnt1 = self.vertex_point(v1)?;

        // Drop Vertex0 Point to RailCurve
        let Some(param1) = drop_param(k, &new_rail_curve, &trim_ivl, pnt0, approx_tol * 10.0)?
        else {
            self.fillet_edge_mut(e)?
                .set_status(FilStatus::EndPntDropFailure);
            return Err(SolidError::sm_err(OP_SETBACK));
        };

        // Drop Vertex1 Point to RailCurve
        let Some(param2) = drop_param(k, &new_rail_curve, &trim_ivl, pnt1, approx_tol * 10.0)?
        else {
            self.fillet_edge_mut(e)?
                .set_status(FilStatus::EndPntDropFailure);
            return Err(SolidError::sm_err(OP_SETBACK));
        };

        // set Rail NewCurve and UVTrimCurve and NewOrigUVCurve TrimIvls to found
        // DropParam values
        let reverse_curves = param1 > param2;
        if reverse_curves {
            let _ = trim_ivl.set_min_max(param2, param1);
        } else {
            let _ = trim_ivl.set_min_max(param1, param2);
        }
        new_rail_curve.trim(k, &mut trim_ivl, TrimMode::Notify)?;
        new_rail_uv_curve.trim(k, &mut trim_ivl, TrimMode::Notify)?;
        if let Some(c) = new_orig_uv_curve.as_mut() {
            c.trim(k, &mut trim_ivl, TrimMode::Notify)?;
        }

        // when needed - reverse curve parameterizations
        if reverse_curves {
            let ivl = new_rail_curve.natural_interval();
            new_rail_curve.reverse_parameterization(k, &ivl)?;
            new_rail_uv_curve.reverse_parameterization(k, &ivl)?;
            if let Some(c) = new_orig_uv_curve.as_mut() {
                c.reverse_parameterization(k, &ivl)?;
            }
        }

        // Set this FilletEdge->Curve and UVCurve values (side effect: delete current
        // this->UVTrimCurves)
        set_rail_curve(self.brep_mut(), e, new_rail_curve)?;
        self.set_edgeuse_uv_curve(prim_eu, Some(new_rail_uv_curve), false, true)?;
        self.set_edgeuse_uv_curve(mate_eu, new_orig_uv_curve, false, true)?;

        // for Corner of type = SM_FCR_N_x_2,
        if let Some(corner) = opt_corner.filter(|c| c.corner_type() == FilletCornerType::NByTwo) {
            let len = self
                .brep()
                .edge(e)?
                .curve
                .as_ref()
                .ok_or_else(nul)?
                .approximate_length(k, &trim_ivl, 5, false)?
                .0;
            if len < corner.approx_tol_3d {
                // The edge is too small, try to squeeze it (later)
                let mut delete_fv = v0;
                if self.fillet_vertex(delete_fv)?.fillet_vertex_type()
                    == FilletVertexType::FilletXFillet
                {
                    delete_fv = v1;
                }
                if self.fillet_vertex(delete_fv)?.fillet_vertex_type()
                    != FilletVertexType::RailXEdgeuse
                {
                    return Err(SolidError::sm_err(OP_SETBACK));
                }
                self.fillet_vertex_mut(delete_fv)?
                    .set_status(FilStatus::ToBeDeleted);
                self.fillet_edge_mut(e)?.set_status(FilStatus::ToBeSqueezed);

                // all done
                return Ok(());
            }
        }

        // add the PrimEU to the m_vSideEUs array
        self.geom_mut(fillet_geom)?.add_side_fillet_edgeuse(prim_eu);
        Ok(())
    }

    /// `SmFilletEdge::ShouldSwapFaces(pOrigFace, pNewFace)`: whether fillet edge `e`
    /// lies on `new_face` and not `orig_face` (both of `target`), by classifying a
    /// point of it (`sm_ClassifyFacePoint`): its curve at the middle of its interval
    /// (or of its curve classification), then at the start and end while the point is
    /// on neither face; without either, the vertex of its fillet corner (resolved in
    /// `corners`). `false` when the point is on the original face, on both, or
    /// unclassified, and for an edge with neither interval, classification nor corner.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` without a curve to evaluate or a corner vertex;
    /// [`SolidError::InvalidInput`] for a corner missing from `corners`; those of
    /// `sm_ClassifyFacePoint` ([`SolidError::Unported`] propagates; the curve
    /// evaluation's status is ignored as upstream).
    pub fn should_swap_faces(
        &self,
        k: &Kernel,
        e: EdgeId,
        orig_face: FaceId,
        new_face: FaceId,
        target: &mut Brep,
        corners: &[FilletCorner],
    ) -> Result<bool> {
        let nul = || SolidError::null_pointer(OP_SWAP);
        let fe = self.fillet_edge(e)?;
        let interval = self.brep().edge(e)?.interval;

        // Find a point to drop (SmPoint3d(): SM_UNDEF_DOUBLE coordinates).
        let mut test_pt = Point3::UNDEF;
        let mut which_try = 1;
        loop {
            let pick = |ivl: &Extent1d, which: i32| match which {
                1 => ivl.mid(),
                2 => ivl.min(),
                _ => ivl.max(),
            };
            if !(interval.is_init() || interval.has_negative_length()) {
                let param = pick(&interval, which_try);
                let curve = self.brep().edge(e)?.curve.as_ref().ok_or_else(nul)?;
                if let Some(p) = ignored(curve.evaluate_point(k, param))? {
                    test_pt = p;
                }
            } else if let Some(cc) = fe.curve_class() {
                let edge_domain = cc.interval();
                let param = pick(&edge_domain, which_try);
                let curve = self.brep().edge(e)?.curve.as_ref().ok_or_else(nul)?;
                if let Some(p) = ignored(curve.evaluate_point(k, param))? {
                    test_pt = p;
                }
            } else if let Some(corner) = fe.fillet_corner() {
                let base_vtx = corner_of(corners, corner, OP_SWAP)?
                    .vertex
                    .ok_or_else(nul)?;
                test_pt = target.vertex(base_vtx)?.point;

                // We have no curve, only one point, so only one try:
                which_try = 3;
            } else {
                // Shouldn't happen
                return Ok(false);
            }

            let which_face = classify_face_point(k, target, test_pt, orig_face, new_face)?;

            if which_face == 1 {
                return Ok(false);
            }
            if which_face == 2 {
                return Ok(true);
            }
            if which_face == 3 {
                return Ok(false); // If in both, we don't have to switch m_pOrigFace.
            }

            // the point does not classify to its own Face: try different points on the
            // curve.
            if which_face == 0 {
                which_try += 1;
                if which_try <= 3 {
                    continue;
                }
            }
            return Ok(false);
        }
    }

    // ---- SmFilletGeom ---------------------------------------------------------------

    /// `SmFilletGeom::Copy(pFilletGeomToCopy)`: geometry `g` takes copies of `other`'s
    /// fillet surface and centre line, offset surfaces (`GetOffsetSurface`, falling
    /// back to the solver's) and deviations; each of its rails takes the type,
    /// original face and edge of `other`'s rail, a copy of its B-spline curve (with
    /// that interval) and copies of the UV trim curves of its primary edge use and of
    /// that use's mate (`GetUVTrimCurve`: each use's or its mate's). `solvers`
    /// resolves the geometries' (common) solver.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when the geometries have different solvers; `SM_ERR_NULL_POINTER`
    /// (leaving the copies made so far) when `other` has no B-spline fillet surface or
    /// centre line, or a rail curve copy is not a B-spline;
    /// [`SolidError::InvalidInput`] without solver, rails or a solver missing from
    /// `solvers` (upstream dereferences `NULL`); those of the rail edits.
    pub fn copy_geom(
        &mut self,
        k: &Kernel,
        g: FilletGeomId,
        other: FilletGeomId,
        solvers: &[FilletSolver],
    ) -> Result<()> {
        // check state
        if self.geom(g)?.fillet_solver() != self.geom(other)?.fillet_solver() {
            return Err(SolidError::sm_err(OP_COPY));
        }
        let solver_id = self
            .geom(g)?
            .fillet_solver()
            .ok_or_else(|| invalid(OP_COPY, "fillet geometry without solver"))?;
        let solver = solver_of(solvers, solver_id, OP_COPY)?;
        self.copy_geom_with_solver(k, g, other, solver)
    }

    /// [`FilletBrep::copy_geom`] with the geometries' (common) solver `solver`.
    ///
    /// # Errors
    /// As [`FilletBrep::copy_geom`].
    pub fn copy_geom_with_solver(
        &mut self,
        k: &Kernel,
        g: FilletGeomId,
        other: FilletGeomId,
        solver: &FilletSolver,
    ) -> Result<()> {
        let nul = || SolidError::null_pointer(OP_COPY);

        // check state
        if self.geom(g)?.fillet_solver() != self.geom(other)?.fillet_solver() {
            return Err(SolidError::sm_err(OP_COPY));
        }
        // locals: m_pFilletSolver->m_crContext
        if self.geom(g)?.fillet_solver().is_none() {
            return Err(invalid(OP_COPY, "fillet geometry without solver"));
        }

        // copy FilletSurface and CenterLineCurve
        let to_copy = self.geom(other)?;
        let copy_surface = match to_copy.fillet_surface(None)? {
            Some(s) => copy_as_b_spline_surface(k, s)?,
            None => None,
        };
        let copy_curve = match to_copy.center_line_geometry() {
            Some(curve) => copy_as_b_spline(k, curve)?,
            None => None,
        };
        let has_surface = copy_surface.is_some();
        self.geom_mut(g)?.set_fillet_surface(copy_surface);
        if !has_surface {
            return Err(nul());
        }
        let has_curve = copy_curve.is_some();
        self.geom_mut(g)?.center_line_curve = copy_curve;
        if !has_curve {
            return Err(nul());
        }

        // for both offset surfaces
        for i in 0..2usize {
            // Offset surfaces.  Base surfaces are not owned.
            let to_copy = self.geom(other)?;
            let other_off_surf = match to_copy.off_surfs.get(i).and_then(Option::as_ref) {
                Some(s) => Some(s),
                None => solver.surface(i)?.map(|slot| &slot.surface),
            };

            // when given an OffsetSurface to copy - copy it
            if let Some(off) = other_off_surf {
                // SmOffsetSurface::Copy(); a failed copy leaves the slot unchanged
                if let Some(surface) = ignored(off.surface.copy(k))? {
                    let new_off = OffsetSurface {
                        surface,
                        base_id: off.base_id,
                    };
                    if let Some(slot) = self.geom_mut(g)?.off_surfs.get_mut(i) {
                        *slot = Some(new_off);
                    }
                }
            }

            let deviation = self
                .geom(other)?
                .deviations
                .get(i)
                .copied()
                .ok_or_else(|| invalid(OP_COPY, "rail index past 1"))?;
            if let Some(d) = self.geom_mut(g)?.deviations.get_mut(i) {
                *d = deviation;
            }

            // rails
            let rail_to_copy = self
                .geom(other)?
                .rail(i)
                .ok_or_else(|| invalid(OP_COPY, "fillet geometry without rail"))?;
            let rail = self
                .geom(g)?
                .rail(i)
                .ok_or_else(|| invalid(OP_COPY, "fillet geometry without rail"))?;

            // Copy Rail parameters
            let src = self.fillet_edge(rail_to_copy)?;
            let (edge_type, orig_face, orig_edge) = (
                src.fillet_edge_type(),
                src.original_face(),
                src.original_edge(),
            );
            let dst = self.fillet_edge_mut(rail)?;
            dst.set_fillet_edge_type(edge_type);
            dst.set_original_face(orig_face);
            dst.set_original_edge(orig_edge);

            // Copy 3D curve for new rail
            let orig_3d_curve = self
                .brep()
                .edge(rail_to_copy)?
                .curve
                .as_ref()
                .filter(|c| c.is_kind_of(BSPLINE_CURVE_TYPE));
            if let Some(orig) = orig_3d_curve {
                let curve_3d = copy_as_b_spline(k, orig)?.ok_or_else(nul)?;
                let crv_ivl = curve_3d.natural_interval();
                // side effect: delete current pEdge->UVTrimCurves
                set_rail_curve(self.brep_mut(), rail, curve_3d)?;
                edge::set_interval(self.brep_mut(), rail, &crv_ivl)?;
            }

            // Copy UV curves for new rail
            let eu_orig = primary_edgeuse(self.brep(), rail_to_copy)?
                .ok_or_else(|| invalid(OP_COPY, "rail without primary edge use"))?;
            let orig_uv_curve = eu1::uv_trim_curve(self.brep(), eu_orig)?.0.cloned();
            if let Some(uv) = orig_uv_curve {
                if let Some(eu) = primary_edgeuse(self.brep(), rail)? {
                    self.set_edgeuse_uv_curve(eu, Some(uv), true, true)?;
                }
            }

            let eu_orig_mate = eu1::mate(self.brep(), eu_orig)?
                .ok_or_else(|| invalid(OP_COPY, "rail edge use without mate"))?;
            let orig_uv_curve = eu1::uv_trim_curve(self.brep(), eu_orig_mate)?.0.cloned();
            if let Some(uv) = orig_uv_curve {
                let prim = primary_edgeuse(self.brep(), rail)?
                    .ok_or_else(|| invalid(OP_COPY, "rail without primary edge use"))?;
                if let Some(eu) = eu1::mate(self.brep(), prim)? {
                    self.set_edgeuse_uv_curve(eu, Some(uv), true, true)?;
                }
            }
        }
        Ok(())
    }

    /// `SmFilletGeom::FindEdgeTangentAtPoint(lRailIndex, lEndIndex)`: the unit tangent,
    /// at the original parameter, of the target edge the given end of rail
    /// `rail_index` of geometry `g` is classified to (`SM_PC_EDGE`); `None` for other
    /// classifications or a zero tangent (`rbFoundTangent = FALSE`).
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` without the classification edge or its curve;
    /// [`SolidError::InvalidInput`] for a rail index above 1, a missing rail or vertex,
    /// or an edge classification object that is not an edge; those of the evaluation.
    pub fn find_edge_tangent_at_point(
        &self,
        k: &Kernel,
        g: FilletGeomId,
        rail_index: usize,
        end_index: usize,
        target: &Brep,
    ) -> Result<Option<Vec3>> {
        let nul = || SolidError::null_pointer(OP_TANGENT);
        let rail = self
            .geom(g)?
            .rails
            .get(rail_index)
            .copied()
            .ok_or_else(|| invalid(OP_TANGENT, "rail index past 1"))?
            .ok_or_else(|| invalid(OP_TANGENT, "fillet geometry without rail"))?;
        let mut fv = edge::vertex(self.brep(), rail)?;
        if end_index != 0 {
            fv = edge::other_vertex(self.brep(), rail, fv)?;
        }
        let fv = fv.ok_or_else(|| invalid(OP_TANGENT, "rail without vertex"))?;
        let record = self.fillet_vertex(fv)?;

        if record.point_class() == PointClassType::Edge {
            let t = record.original_t_param();
            let e = match record.point_class_object() {
                Some(EntityRef::Edge(e)) => e,
                None => return Err(nul()),
                Some(_) => return Err(invalid(OP_TANGENT, "classification object is not an edge")),
            };
            let curve = target.edge(e)?.curve.as_ref().ok_or_else(nul)?;
            let (_, d) = evaluate_tangent(k, curve, t, OP_TANGENT)?;
            if d.length_squared() > EFF_ZERO_SQ {
                return Ok(Some(d.unitize_or_z()));
            }
        }
        // SM_PC_VERTEX: someday we will look at all edges at this vertex
        Ok(None)
    }

    /// `SmFilletGeom::GetRailIndexOfSideEU(pEU)`: the rail of geometry `g` on the face
    /// of target edge use `eu` (or of its radial use); without both rails, the
    /// solver's `FindIndexOfRailXSideEdgeuse`.
    ///
    /// # Errors
    ///
    /// `SM_ERR` (upstream: `lIdx = 0`) when neither face is a rail's original face;
    /// `SM_ERR_NULL_POINTER` without radial use; [`SolidError::InvalidInput`] without
    /// a solver (upstream dereferences `NULL`) or one missing from `solvers`; those of
    /// `FindIndexOfRailXSideEdgeuse`.
    pub fn rail_index_of_side_eu(
        &self,
        g: FilletGeomId,
        eu: EdgeuseId,
        target: &Brep,
        solvers: &[FilletSolver],
    ) -> Result<usize> {
        self.rail_index_of_side_eu_resolved(g, eu, target, &|id| solver_of(solvers, id, OP_SIDE_EU))
    }

    /// Shared indexed-table and live-solver implementation; resolution stays at
    /// the upstream lookup, after the complete-rail shortcut.
    pub(crate) fn rail_index_of_side_eu_resolved<'s>(
        &self,
        g: FilletGeomId,
        eu: EdgeuseId,
        target: &Brep,
        resolve: &impl Fn(FilletSolverId) -> Result<&'s FilletSolver>,
    ) -> Result<usize> {
        let geom = self.geom(g)?;
        let (Some(rail0), Some(rail1)) = (geom.rail(0), geom.rail(1)) else {
            let solver_id = geom
                .fillet_solver()
                .ok_or_else(|| invalid(OP_SIDE_EU, "fillet geometry without solver"))?;
            return resolve(solver_id)?.find_index_of_rail_x_side_edgeuse(target, eu);
        };

        let face0 = self.fillet_edge(rail0)?.original_face();
        let face1 = self.fillet_edge(rail1)?.original_face();

        let mut idx = 2; // error condition, locally

        let side_face = eu1::face(target, eu)?;
        if face0 == side_face {
            idx = 0;
        } else if face1 == side_face {
            idx = 1;
        }

        if idx < 2 {
            return Ok(idx);
        }

        // Be nice: allow the other side of the given edgeuse
        let radial_eu =
            eu1::radial(target, eu)?.ok_or_else(|| SolidError::null_pointer(OP_SIDE_EU))?;

        let side_face = eu1::face(target, radial_eu)?;
        if face0 == side_face {
            idx = 0;
        } else if face1 == side_face {
            idx = 1;
        }

        if idx > 1 {
            return Err(SolidError::sm_err(OP_SIDE_EU));
        }
        Ok(idx)
    }

    /// `SmFilletGeom::GetRailIndexOfVertexuse(pFVU)`: the rail of geometry `g` that has
    /// `vu` at one of its ends (`GetRailVertexuse`).
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` for a `None` vertex use; `SM_ERR` (upstream: `lIdx = 0`)
    /// when `vu` is at neither rail's ends; those of [`FilletBrep::rail_vertexuse`] (a
    /// missing rail).
    pub fn rail_index_of_vertexuse(
        &self,
        g: FilletGeomId,
        vu: Option<VertexuseId>,
    ) -> Result<usize> {
        // NER( pFVU )
        let vu = Some(vu.ok_or_else(|| SolidError::null_pointer(OP_RAIL_VU))?);
        if self.rail_vertexuse(g, 0, 0)? == vu || self.rail_vertexuse(g, 0, 1)? == vu {
            return Ok(0);
        }
        if self.rail_vertexuse(g, 1, 0)? == vu || self.rail_vertexuse(g, 1, 1)? == vu {
            return Ok(1);
        }
        Err(SolidError::sm_err(OP_RAIL_VU))
    }

    /// `SmFilletGeom::GetRailVertex(lRailIndex, lEndIndex)`: the fillet vertex of the
    /// rail-end vertex use (`GetRailVertexuse`).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] without the vertex use or its vertex (upstream
    /// dereferences `NULL`); those of [`FilletBrep::rail_vertexuse`].
    pub fn rail_vertex(
        &self,
        g: FilletGeomId,
        rail_index: usize,
        end_index: usize,
    ) -> Result<VertexId> {
        let vu = self
            .rail_vertexuse(g, rail_index, end_index)?
            .ok_or_else(|| invalid(OP_RAIL_V, "rail end without vertex use"))?;
        self.brep()
            .vertexuse(vu)?
            .vertex
            .ok_or_else(|| invalid(OP_RAIL_V, "vertex use without vertex"))
    }

    /// `SmFilletGeom::SetUpRailPoint(lRailIndex, lEndIndex, dParameter,
    /// pOptPointClass)`: unless processed, the fillet vertex at that rail end takes
    /// the rail curve's point at `parameter`, an unknown type (a rail/edge-use or
    /// rail/vertex type with a copy of `opt_point_class` classified to an edge or a
    /// vertex) and is processed; its vertex use takes the fillet surface parameters
    /// `(parameter, v)` of that corner of the surface's domain.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` without fillet surface; [`SolidError::InvalidInput`]
    /// without the vertex use, vertex, rail or rail curve (upstream dereferences
    /// `NULL`); those of [`FilletBrep::rail_vertexuse`] and the evaluation.
    pub fn set_up_rail_point(
        &mut self,
        k: &Kernel,
        g: FilletGeomId,
        rail_index: usize,
        end_index: usize,
        parameter: f64,
        opt_point_class: Option<&PointClassification<EntityRef>>,
    ) -> Result<()> {
        let vu = self
            .rail_vertexuse(g, rail_index, end_index)?
            .ok_or_else(|| invalid(OP_SETUP, "rail end without vertex use"))?;
        let fillet_vert = self
            .brep()
            .vertexuse(vu)?
            .vertex
            .ok_or_else(|| invalid(OP_SETUP, "vertex use without vertex"))?;

        if self.fillet_vertex(fillet_vert)?.is_processed() {
            return Ok(());
        }

        let rail = self
            .geom(g)?
            .rail(rail_index)
            .ok_or_else(|| invalid(OP_SETUP, "fillet geometry without rail"))?;
        let rail_curve = self
            .brep()
            .edge(rail)?
            .curve
            .as_ref()
            .ok_or_else(|| invalid(OP_SETUP, "rail without curve"))?;
        let s3d_pnt = rail_curve.evaluate_point(k, parameter)?;

        let fv = self.fillet_vertex_mut(fillet_vert)?;
        fv.set_fillet_vertex_type(FilletVertexType::Unknown);
        if let Some(pc) = opt_point_class {
            fv.copy_point_class(pc);

            if pc.point_class() == PointClassType::Edge {
                fv.set_fillet_vertex_type(FilletVertexType::RailXEdgeuse);
            }

            if pc.point_class() == PointClassType::Vertex {
                fv.set_fillet_vertex_type(FilletVertexType::RailXVertex);
            }
        }

        self.set_vertex_point(fillet_vert, s3d_pnt)?; //cbi: untrimmed pt: incorrect.

        // Get UV value (in the fillet surface) for this corner vertexuse
        let surface = self
            .geom(g)?
            .fillet_surface(None)?
            .ok_or_else(|| SolidError::null_pointer(OP_SETUP))?;
        let uv_domain = surface.natural_uv_domain_with_nurb(k)?;
        let mut uv = uv_domain.evaluate(end_index as f64, rail_index as f64);
        uv.x = parameter;

        self.fillet_vertexuse_mut(vu)?.set_uv_point(uv);

        self.fillet_vertex_mut(fillet_vert)?
            .set_status(FilStatus::Processed);
        Ok(())
    }

    /// `SmFilletGeom::RailEdgeuseIntersect(lRailIndex, pEdgeuse, dGuessEdgeParameter,
    /// ...)`: the solver's rail/edge-use intersection evaluated on this geometry's
    /// surfaces: the solver's geometry for surfaces is `g` during the call and restored
    /// after it (also on a failure). `tsect` is `rTsectPnt` (written in place, as
    /// upstream, also on a failure); returns `rdEdgeuseParameter` when an intersection
    /// was found (`rbFoundIntersection`), else `None`. `target` is mutable because the
    /// solver may build the edge use's UV trim curve.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` for a geometry without solver (upstream's `NER(pFS)`);
    /// those of the solver's call ([`FilletSolver::rail_edgeuse_intersect`]).
    #[allow(clippy::too_many_arguments)]
    pub fn rail_edgeuse_intersect(
        &mut self,
        k: &Kernel,
        g: FilletGeomId,
        solver: &mut FilletSolver,
        target: &mut Brep,
        rail_index: usize,
        edgeuse: EdgeuseId,
        guess_edge_parameter: f64,
        tsect: &mut TsectPnt,
    ) -> Result<Option<f64>> {
        self.rail_edgeuse_intersect_with_solver_ops(
            k,
            g,
            solver,
            target,
            rail_index,
            edgeuse,
            guess_edge_parameter,
            tsect,
        )
    }

    /// Intersect using the actual derived solver, restoring its surface geometry
    /// after either success or failure.
    ///
    /// # Errors
    /// As [`Self::rail_edgeuse_intersect`].
    #[allow(clippy::too_many_arguments)]
    pub fn rail_edgeuse_intersect_with_solver_ops(
        &mut self,
        k: &Kernel,
        g: FilletGeomId,
        solver: &mut dyn FilletSolverOps,
        target: &mut Brep,
        rail_index: usize,
        edgeuse: EdgeuseId,
        guess_edge_parameter: f64,
        tsect: &mut TsectPnt,
    ) -> Result<Option<f64>> {
        let rollover = self.rollover_guesses(g)?;
        let geometry = self.geom_mut(g)?;
        // NER( pFS )
        if geometry.fillet_solver().is_none() {
            return Err(SolidError::null_pointer(OP_RAIL_EU));
        }

        // Tell our FilletSolver to use our surfaces.
        // If there's anything there already, restore it when we're done.
        let saved_fg =
            solver
                .solver_mut()
                .lend_geometry_surfaces(g, rollover, &mut geometry.off_surfs);

        let stat = FilletSolver::rail_edgeuse_intersect(
            solver,
            k,
            target,
            rail_index,
            edgeuse,
            guess_edge_parameter,
            tsect,
        );

        solver
            .solver_mut()
            .return_geometry_surfaces(saved_fg, &mut geometry.off_surfs);

        stat
    }

    /// The tangent-rollover guesses of geometry `g`'s rails
    /// (`SmConstantRadiusFS::LoadInitialValues` with `g` as the geometry for the
    /// surfaces): per rail, the other surface's UV position of the first rail edge use
    /// whose vertex is `SM_FV_RAIL_X_EDGEUSE` and whose intersection point is set.
    ///
    /// # Errors
    /// [`SolidError::NotFound`] for dangling ids.
    pub fn rollover_guesses(&self, g: FilletGeomId) -> Result<RolloverGuesses> {
        let geometry = self.geom(g)?;
        let mut out = RolloverGuesses {
            tangent_rollover: geometry.fillet_geom_type() == FilletGeomType::TangentRollover,
            ..RolloverGuesses::default()
        };
        if !out.tangent_rollover {
            return Ok(out);
        }
        for (rail_index, slot) in out.rails.iter_mut().enumerate() {
            let Some(rail) = geometry.rail(rail_index) else {
                continue;
            };
            let mut found = None;
            for eu in edge::edgeuses(self.brep(), rail, None, None)? {
                let Some(vu) = eu1::vertexuse(self.brep(), eu)? else {
                    continue;
                };
                let Ok(fvu) = self.fillet_vertexuse(vu) else {
                    continue;
                };
                let Some(fv) = self.brep().vertexuse(vu)?.vertex else {
                    continue;
                };
                let Ok(fvtx) = self.fillet_vertex(fv) else {
                    continue;
                };
                if fvtx.fillet_vertex_type() != FilletVertexType::RailXEdgeuse {
                    continue;
                }
                // Its uv should be set, and we should use it for srf2 guess.
                let tsect = fvu.tsect_pnt();
                if matches!(
                    tsect.point_type,
                    IntersectionPointType::Undefined | IntersectionPointType::Unknown
                ) {
                    continue;
                }
                found = Some(tsect.uv_pos(if rail_index == 0 {
                    SrfIndex::Second
                } else {
                    SrfIndex::First
                }));
                break;
            }
            *slot = Some(found);
        }
        Ok(out)
    }

    /// Intersect with access to the actual geometry's mutable offset objects.
    ///
    /// # Errors
    /// As [`Self::rail_edgeuse_intersect_with_solver_ops`].
    #[allow(clippy::too_many_arguments)]
    pub fn rail_edgeuse_intersect_with_geometry(
        &mut self,
        k: &Kernel,
        g: FilletGeomId,
        solver: &mut dyn FilletSolverOps,
        target: &mut Brep,
        rail_index: usize,
        edgeuse: EdgeuseId,
        guess: f64,
        tsect: &mut TsectPnt,
    ) -> Result<Option<f64>> {
        let rollover = self.rollover_guesses(g)?;
        let geometry = self.geom_mut(g)?;
        if geometry.fillet_solver().is_none() {
            return Err(SolidError::null_pointer(OP_RAIL_EU));
        }
        // pFS->SetFilletGeomForSurfaces(this), its surfaces lent to the solver
        let saved =
            solver
                .solver_mut()
                .lend_geometry_surfaces(g, rollover, &mut geometry.off_surfs);
        let result = solver.rail_edgeuse_intersect_on_geometry(
            k, geometry, target, rail_index, edgeuse, guess, tsect,
        );
        solver
            .solver_mut()
            .return_geometry_surfaces(saved, &mut geometry.off_surfs);
        result
    }

    /// `SmFilletGeom::TrimOriginalSurfaces(rFilletBrepFacesKept, rOriginalFacesDelete,
    /// rMarkLocks)`: for each rail (and other rail) of geometry `g` of type
    /// `SM_FE_RAIL` whose fillet B-rep edge is mated to a target edge with two faces:
    /// collects the fillet B-rep faces connected to that edge's face (marked with the
    /// first lock's mark) into `kept`; then for every second edge use of the target
    /// edge on the rail's original face (or one split from it) whose binormal points
    /// along the fillet edge's binormal (reversed for a reverse-trim solver), collects
    /// the target faces reachable from its face without crossing marked edges (marked
    /// with the target context's mark). Unless one of those faces is the use's
    /// radial face (the trim leaked), they are added to `deleted`. Faces are added
    /// once each.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when the fillet edge's midpoint does not drop onto the target edge;
    /// `SM_ERR_NULL_POINTER` without the target edge's curve;
    /// [`SolidError::InvalidInput`] without solver, rails, mark locks, the target
    /// context's mark lock, or a fillet edge without primary edge use or curve
    /// (upstream dereferences `NULL`); those of the traversals, binormal evaluations
    /// and `DropPoint`.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub fn trim_original_surfaces(
        &self,
        k: &Kernel,
        g: FilletGeomId,
        fillet_brep: &mut Brep,
        fillet_marks: &mut Marks,
        target: &mut Brep,
        target_marks: &mut Marks,
        kept: &mut Vec<FaceId>,
        deleted: &mut Vec<FaceId>,
        mark_locks: &[MarkLock],
        env: &TrimOriginalSurfacesEnv<'_>,
        solvers: &[FilletSolver],
    ) -> Result<()> {
        let mut marks = IndexedTrimMarks {
            fillet: fillet_marks,
            target: target_marks,
        };
        self.trim_original_surfaces_resolved(
            k,
            g,
            fillet_brep,
            target,
            &mut marks,
            kept,
            deleted,
            mark_locks,
            env,
            |id| solver_of(solvers, id, OP_TRIM),
        )
    }

    /// Trim original surfaces using one retained solver and body-qualified marks.
    /// The solver is checked at the same point as the indexed entrypoint's lookup.
    ///
    /// # Errors
    ///
    /// Those of [`Self::trim_original_surfaces`], including a mismatched solver id.
    #[allow(clippy::too_many_arguments)]
    pub fn trim_original_surfaces_with_solver(
        &self,
        k: &Kernel,
        g: FilletGeomId,
        fillet_brep: &mut Brep,
        target: &mut Brep,
        marks: &mut impl TrimOriginalSurfacesMarks,
        kept: &mut Vec<FaceId>,
        deleted: &mut Vec<FaceId>,
        mark_locks: &[MarkLock],
        env: &TrimOriginalSurfacesEnv<'_>,
        solver_id: FilletSolverId,
        solver: &FilletSolver,
    ) -> Result<()> {
        self.trim_original_surfaces_resolved(
            k,
            g,
            fillet_brep,
            target,
            marks,
            kept,
            deleted,
            mark_locks,
            env,
            |id| {
                if id == solver_id {
                    Ok(solver)
                } else {
                    Err(invalid(
                        OP_TRIM,
                        "fillet geometry solver does not match retained solver",
                    ))
                }
            },
        )
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    fn trim_original_surfaces_resolved<'s>(
        &self,
        k: &Kernel,
        g: FilletGeomId,
        fillet_brep: &mut Brep,
        target: &mut Brep,
        marks: &mut impl TrimOriginalSurfacesMarks,
        kept: &mut Vec<FaceId>,
        deleted: &mut Vec<FaceId>,
        mark_locks: &[MarkLock],
        env: &TrimOriginalSurfacesEnv<'_>,
        solver: impl FnOnce(FilletSolverId) -> Result<&'s FilletSolver>,
    ) -> Result<()> {
        // locals
        let geom = self.geom(g)?;
        let solver_id = geom
            .fillet_solver()
            .ok_or_else(|| invalid(OP_TRIM, "fillet geometry without solver"))?;
        let fs = solver(solver_id)?;
        let fillet_mark_type = mark_locks
            .first()
            .ok_or_else(|| invalid(OP_TRIM, "no mark locks"))?
            .mark_type();
        let mut bad_trim = false;

        let mut del_faces: Vec<FaceId> = Vec::new();
        let mut collected_faces: Vec<FaceId> = Vec::new();

        // For both Fillet sides (both rails):
        for ii in 0..2usize {
            // get sAllRails on this side. Usually just m_vRails[ii], but also
            // m_vOtherRails if original rail was split into multiple segments by
            // rollover
            let main_rail = geom
                .rail(ii)
                .ok_or_else(|| invalid(OP_TRIM, "fillet geometry without rail"))?;
            let mut all_rails = vec![main_rail];
            all_rails.extend_from_slice(if ii == 0 {
                &geom.other_rails1
            } else {
                &geom.other_rails2
            });

            // Get the surface which the rail curve is on.
            let orig_face = self.fillet_edge(main_rail)?.original_face();
            let splt_faces = self.fillet_edge(main_rail)?.split_faces().to_vec();

            // for every rail on this fillet side - usually 1 sometimes more
            for rail in all_rails {
                let rail_rec = self.fillet_edge(rail)?;

                // skip rails not attached to OrigBrep faces
                if rail_rec.fillet_edge_type() != FilletEdgeType::Rail {
                    continue;
                }

                // get the m_pFilletBrep->Edge mapped to this m_pPseudoBrep->pRail
                let Some(fb_edge) = rail_rec.fillet_brep_edge1 else {
                    continue;
                };

                // Get mated m_pTargetBrep->Edge for this m_pFilletBrep->Edge
                let mut brep_edge = (env.brep_mate)(fb_edge);

                // when FilletBrep->Edge is not mated to an OrigBrep->Edge
                if brep_edge.is_none() {
                    let brep_edge_curve = fillet_brep
                        .edge(fb_edge)?
                        .curve
                        .as_ref()
                        .ok_or_else(|| invalid(OP_TRIM, "fillet B-rep edge without curve"))?;

                    // skip FilletBrep->Edge->Curves owned by the FilletBrepEdge
                    if brep_edge_curve.owner() == Some((env.fillet_edge_key)(fb_edge)) {
                        continue;
                    }
                    // (composite edges were removed upstream)
                    brep_edge = None;
                }

                // skip m_pFilletBrep->Edges not mated to an Orig m_pTargetBrep->Edge
                let Some(brep_edge) = brep_edge else {
                    continue;
                };

                let prim_eu = primary_edgeuse(fillet_brep, fb_edge)?
                    .ok_or_else(|| invalid(OP_TRIM, "fillet B-rep edge without edge use"))?;
                let fb_face = eu1::face(fillet_brep, prim_eu)?;

                // Collect all faces reachable from pFBFace in m_pFilletBrep -- all new
                // Fillet faces.
                collect_faces(
                    k,
                    fillet_brep,
                    &mut TrimTraversalMarks {
                        marks,
                        body: TrimOriginalBody::Fillet,
                    },
                    fb_face,
                    fillet_mark_type,
                    &mut collected_faces,
                    None,
                    None,
                )?;

                // accumulate output rFilletBrepFacesKept array += unique
                // m_pFilletBrep->sCollectedFaces
                for &f in &collected_faces {
                    if !kept.contains(&f) {
                        kept.push(f);
                    }
                }

                // get Orig m_pTargetBrep->Faces connected to the mated Orig
                // m_pTargetBrep->Edge
                let faces1 = edge::faces(target, brep_edge, None)?;

                // skip nonManifold orig m_pTargetBrep >Edges
                if faces1.len() != 2 {
                    continue;
                }

                // Get the m_pFilletBrep->Edge binormal pointing into the fillet
                let ivl = edge::interval(fillet_brep, fb_edge)?;
                let fillet_eval = evaluate_binormal(
                    k,
                    fillet_brep,
                    prim_eu,
                    ivl.evaluate(0.5),
                    false,
                    &BinormalOptions::default(),
                )?;
                let mid_pnt = fillet_eval.point;
                let fillet_bin = fillet_eval.binormal;

                // Drop the m_pFilletBrep->Edge->MidPoint onto the Orig
                // m_pTargetBrep->Edge->Curve
                let ivl2 = edge::interval(target, brep_edge)?;
                let dist_tol =
                    edge::tolerance(fillet_brep, fb_edge)? + edge::tolerance(target, brep_edge)?;
                let brep_edge_curve = target
                    .edge(brep_edge)?
                    .curve
                    .as_ref()
                    .ok_or_else(|| SolidError::null_pointer(OP_TRIM))?;
                let dropped = brep_edge_curve.drop_point(
                    k,
                    &ivl2,
                    mid_pnt,
                    None,
                    dist_tol,
                    None,
                    SolverOperation::Minimize,
                )?;
                if !dropped.success {
                    return Err(SolidError::sm_err(OP_TRIM));
                }
                let dropped_parameter = dropped.parameter;

                // for every Orig m_pTargetBrep->pBrepEdge->Edgeuse - find the Face
                // connected to the edgeuse no longer needed
                let edgeuses = edge::edgeuses(target, brep_edge, None, None)?;
                for &eu in edgeuses.iter().step_by(2) {
                    // skip Orig m_pTargetBrep->pBrepEdge->Edgeuses not attached to the
                    // original Face(s).
                    let eu_face = eu1::face(target, eu)?;
                    if !(eu_face == orig_face || eu_face.is_some_and(|f| splt_faces.contains(&f))) {
                        continue;
                    }

                    // Get OrigBrep->Edge->Binormal evaluation
                    let mut bin2 = evaluate_binormal(
                        k,
                        target,
                        eu,
                        dropped_parameter,
                        false,
                        &BinormalOptions::default(),
                    )?
                    .binormal;

                    // when asked - reverse the binormal direction
                    if fs.reverse_trim {
                        bin2 = -bin2;
                    }

                    // If this orig m_pTargetBrep->Edge->Edgeuse binormal points in the
                    // same dir as m_pFilletBrep Edge's binormal, then the
                    // m_pTargetBrep->Edgeuse->Face is 'inside' the closed loop of common
                    // edges and it (and all Faces connected to it) get marked for
                    // deletion.
                    if fillet_bin.dot(bin2) > 0.2 {
                        // 0.2: just to avoid noise around 0.0.
                        let matched_orig_face = eu1::face(target, eu)?;
                        let radial = eu1::radial(target, eu)?
                            .ok_or_else(|| invalid(OP_TRIM, "edge use without radial"))?;
                        let adj_orig_face = eu1::face(target, radial)?;

                        let faces2 = edge::faces(target, brep_edge, None)?;
                        if faces2.len() == 1 {
                            continue;
                        }

                        // get MarkType for MatchedOrigFace->Context
                        let lock = mark_locks
                            .iter()
                            .find(|l| l.context() == env.target_context)
                            .ok_or_else(|| invalid(OP_TRIM, "no mark lock for the context"))?;
                        let orig_mark_type = lock.mark_type();

                        // collect all faces connected to pMatchedOrigFace without stepping
                        // over marked vertices and edges
                        collect_faces(
                            k,
                            target,
                            &mut TrimTraversalMarks {
                                marks,
                                body: TrimOriginalBody::Target,
                            },
                            matched_orig_face,
                            orig_mark_type,
                            &mut collected_faces,
                            None,
                            None,
                        )?;

                        // for every sCollectedFaces, mark and add unique faces to
                        // sDelFaces list
                        for &f in &collected_faces {
                            // check for faces that 'leaked out' of the closed loop
                            if Some(f) == adj_orig_face {
                                bad_trim = true;
                            }

                            if !del_faces.contains(&f) {
                                del_faces.push(f);
                            }
                            marks.mark(
                                k,
                                TrimOriginalBody::Target,
                                Entity::Face(f),
                                orig_mark_type,
                            );
                        }
                    }
                } // end iter kk, every RailEdge->Edgeuse
            } // end iter jj, every RailEdge on this fillet side
        } // end iter ii, both fillet sides (both rails)

        // when no connected OrigFaces 'leaked' outside the fillet boundaries
        if !bad_trim {
            // accumulate output, rOriginalFacesDelete += unique sDelFaces
            for f in del_faces {
                if !deleted.contains(&f) {
                    deleted.push(f);
                }
            }
        }
        Ok(())
    }

    /// `SmFilletGeom::RecordFilletError(eType, pFilletedEdge, pTargetFace, pRail,
    /// pFilletSurface, cComment)`: passes the error, with geometry `g`'s status, to its
    /// solver's fillet executive (`NoteFilletError`).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] without solver or executive (upstream dereferences
    /// `NULL`) or a solver missing from `solvers`.
    #[allow(clippy::too_many_arguments)]
    pub fn record_fillet_error(
        &self,
        g: FilletGeomId,
        error_type: FilletErrorType,
        filleted_edge: Option<EdgeId>,
        target_face: Option<FaceId>,
        rail: Option<EdgeId>,
        fillet_surface: Option<&Surface>,
        comment: Option<&str>,
        solvers: &[FilletSolver],
    ) -> Result<()> {
        self.record_fillet_error_in(
            g,
            error_type,
            filleted_edge,
            target_face,
            rail,
            fillet_surface.map(|_| GeomErrorObject::FilletSurface(g)),
            comment,
            solvers,
            None,
        )
    }

    /// [`Self::record_fillet_error`] writing into the executive's [`ErrorInfo`]
    /// (`SmFilletExecutive::NoteFilletError` -> `SmFilletErrorInfo::NoteError`).
    ///
    /// # Errors
    ///
    /// As [`Self::record_fillet_error`].
    #[allow(clippy::too_many_arguments)]
    pub fn record_fillet_error_in(
        &self,
        g: FilletGeomId,
        error_type: FilletErrorType,
        filleted_edge: Option<EdgeId>,
        target_face: Option<FaceId>,
        rail: Option<EdgeId>,
        fillet_surface: Option<GeomErrorObject>,
        comment: Option<&str>,
        solvers: &[FilletSolver],
        error_info: Option<&mut ErrorInfo<GeomErrorObject>>,
    ) -> Result<()> {
        let geom = self.geom(g)?;
        let solver_id = geom
            .fillet_solver()
            .ok_or_else(|| invalid(OP_ERROR, "fillet geometry without solver"))?;
        let _executive = solver_of(solvers, solver_id, OP_ERROR)?
            .executive
            .ok_or_else(|| invalid(OP_ERROR, "solver without fillet executive"))?;

        // pass the call along to the FilletExecutive
        if let Some(info) = error_info {
            info.note_merge_error(
                error_type,
                geom.status(),
                filleted_edge.map(GeomErrorObject::FilletedEdge),
                target_face.map(GeomErrorObject::SideFace),
                rail.map(GeomErrorObject::FilletEdge),
                fillet_surface,
                comment,
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blend::fillet_solver::p1::FilletExecutiveRef;
    use crate::error::{SM_ERR, SM_ERR_NULL_POINTER};
    use crate::geom::attribute::ObjectKey;
    use crate::geom::context::ALL_MARKS;
    use crate::nurbs::math::{CPoint, NOW};
    use crate::topology::{Edgeuse, Vertexuse, VertexuseOwner};

    use super::super::p1::{
        FilletEdgeuse, FilletGeom, FilletGeomType, FilletSolverId, FilletVertex, FilletVertexuse,
    };

    fn code(e: &SolidError) -> Option<i32> {
        match e {
            SolidError::Upstream { code, .. } => Some(*code),
            _ => None,
        }
    }

    fn vertex(fb: &mut FilletBrep, k: &Kernel, p: Point3) -> VertexId {
        let v = fb.brep_mut().add_vertex(p).unwrap();
        fb.insert_fillet_vertex(v, FilletVertex::new(k, None));
        v
    }

    /// A pseudo-B-rep edge from `vs` to `ve` with two edge uses (`SAME` at `vs`,
    /// `OPPOSITE` at `ve`) and their vertex uses.
    fn edge_with_uses(
        fb: &mut FilletBrep,
        vs: VertexId,
        ve: VertexId,
        edge_type: FilletEdgeType,
    ) -> (EdgeId, [EdgeuseId; 2]) {
        let e = fb.new_fillet_edge(vs, ve, None).unwrap();
        fb.fillet_edge_mut(e)
            .unwrap()
            .set_fillet_edge_type(edge_type);
        let mut eus = Vec::new();
        for (v, o) in [(vs, OrientType::Same), (ve, OrientType::Opposite)] {
            let b = fb.brep_mut();
            let eu = b.add_edgeuse_record(Edgeuse::default()).unwrap();
            let vu = b.add_vertexuse_record(Vertexuse::default()).unwrap();
            let r = b.edgeuse_mut(eu).unwrap();
            r.edge = Some(e);
            r.orientation = o;
            r.vertexuse = Some(vu);
            let u = b.vertexuse_mut(vu).unwrap();
            u.vertex = Some(v);
            u.owner = VertexuseOwner::Edgeuse(Some(eu));
            b.vertex_mut(v).unwrap().vertexuses.push(vu);
            b.edge_mut(e).unwrap().edgeuses.push(eu);
            fb.insert_fillet_edgeuse(eu, FilletEdgeuse::default());
            fb.insert_fillet_vertexuse(vu, FilletVertexuse::new());
            eus.push(eu);
        }
        (e, [eus[0], eus[1]])
    }

    /// A line from `a` to `b` on `[t0, t1]` (dimension 3, or 2 dropping `z`).
    fn line(a: Point3, b: Point3, t0: f64, t1: f64, dim: u64) -> Curve {
        let mut c = BSplineCurve::create_line_segment(dim, a, b).unwrap();
        c.edit_parameterization(&Extent1d::from_min_max(t0, t1))
            .unwrap();
        Curve::from(c)
    }

    /// The bilinear patch `(u, v) -> (u, v, 0)` over the unit square.
    fn flat_patch() -> Surface {
        let net = vec![
            vec![
                CPoint::new(0.0, 0.0, 0.0, NOW),
                CPoint::new(0.0, 1.0, 0.0, NOW),
            ],
            vec![
                CPoint::new(1.0, 0.0, 0.0, NOW),
                CPoint::new(1.0, 1.0, 0.0, NOW),
            ],
        ];
        let nl = crate::nurbs::srf_basic::Surface::new(
            net,
            1,
            1,
            vec![0.0, 0.0, 1.0, 1.0],
            vec![0.0, 0.0, 1.0, 1.0],
        )
        .unwrap();
        Surface::from(crate::nurbs::b_spline_surface::BSplineSurface::from_nurb(
            nl, false,
        ))
    }

    /// A geometry on solver 0 with two rails from `(0, y, 0)` to `(1, y, 0)`, `y` 0 and
    /// 1, each with its line curve; returns the geometry and the rails.
    fn two_rail_geom(fb: &mut FilletBrep, k: &Kernel) -> (FilletGeomId, [EdgeId; 2]) {
        let mut geom = FilletGeom::new(k, None, FilletGeomType::Blends).unwrap();
        geom.solver = Some(FilletSolverId(0));
        let g = fb.add_geom(geom);
        let mut rails = [EdgeId::from_index(0); 2];
        for (i, y) in [0.0, 1.0].into_iter().enumerate() {
            let a = Point3::new(0.0, y, 0.0);
            let b = Point3::new(1.0, y, 0.0);
            let vs = vertex(fb, k, a);
            let ve = vertex(fb, k, b);
            let (rail, eus) = edge_with_uses(fb, vs, ve, FilletEdgeType::Rail);
            fb.brep_mut().edge_mut(rail).unwrap().curve = Some(line(a, b, 0.0, 1.0, 3));
            for eu in eus {
                fb.set_edgeuse_fillet_geom(eu, Some(g)).unwrap();
            }
            fb.geom_mut(g).unwrap().set_rail(i, Some(rail));
            rails[i] = rail;
        }
        (g, rails)
    }

    fn solver(executive: bool) -> FilletSolver {
        let mut s = FilletSolver::new(1e-6, 1e-3, 1e-6);
        s.executive = executive.then_some(FilletExecutiveRef);
        s
    }

    fn tsect(x: f64, dx: f64) -> TsectPnt {
        let mut t = TsectPnt::new();
        *t.crv_pos_mut() = Point3::new(x, 0.0, 0.0);
        *t.crv_deriv_mut() = Vec3::new(dx, 0.0, 0.0);
        t
    }

    #[test]
    fn blending_rail_hermite_canonical_drops_a_control_point() {
        // CalcBlendingRail's Hermite branch: degree 2, four control points, end
        // multiplicities 4 (upstream issue WR-190); CreateCanonical makes a quadratic of
        // three of the points, without the start tangent's
        let herm = HermiteCurve::new(
            Point3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Point3::new(1.0, 1.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            2,
        );
        let cntrl_poly = herm.bezier_points();
        let r = BSplineCurve::create_canonical(&CanonicalData {
            dim: 2,
            degree: 2,
            control_points: &cntrl_poly,
            form: BSplineCurveForm::Unspecified,
            knot_multiplicities: &[4, 4],
            knots: &[0.0, 1.0],
            knot_type: KnotType::Unspecified,
            weights: None,
            trim: None,
        });
        let c = r.unwrap();
        assert_eq!(c.degree().unwrap(), 2);
        let pts: Vec<_> = c
            .gw_nurb()
            .unwrap()
            .cpts()
            .iter()
            .map(|p| (p.x, p.y))
            .collect();
        assert_eq!(pts.len(), 3);
        assert_eq!((pts[0], pts[2]), ((0.0, 0.0), (1.0, 1.0)));
        assert!((pts[1].1 - 2.0 / 3.0).abs() < 1e-15 && pts[1].0 == 1.0);
    }

    #[test]
    fn move_tsp_moves_only_out_of_order_points() {
        let pts = [tsect(0.0, 1.0), tsect(1.0, 1.0), tsect(2.0, 1.0)];
        let get = |&i: &usize| Ok(pts[i].clone());

        // in order along the curve: unchanged
        let mut order = vec![0usize, 1, 2];
        move_tsp(&mut order, 0, 2, get).unwrap();
        assert_eq!(order, [0, 1, 2]);

        // out of order: the first moves to the end
        let mut order = vec![2usize, 0, 1];
        move_tsp(&mut order, 0, 2, get).unwrap();
        assert_eq!(order, [0, 1, 2]);

        // two points: swapped
        let mut order = vec![1usize, 0];
        move_tsp(&mut order, 1, 0, get).unwrap();
        assert_eq!(order, [0, 1]);

        // an index past the list: SM_ERR, unchanged
        let mut order = vec![1usize, 0];
        let err = move_tsp(&mut order, 0, 2, get).unwrap_err();
        assert_eq!(code(&err), Some(SM_ERR));
        assert_eq!(order, [1, 0]);
    }

    #[test]
    fn rail_lookups() {
        let k = Kernel::default();
        let mut fb = FilletBrep::new();
        let (g, rails) = two_rail_geom(&mut fb, &k);

        // the start and end vertices of each rail
        let v00 = fb.rail_vertex(g, 0, 0).unwrap();
        let v01 = fb.rail_vertex(g, 0, 1).unwrap();
        assert_eq!(Some(v00), edge::vertex(fb.brep(), rails[0]).unwrap());
        assert_eq!(fb.vertex_point(v01).unwrap(), Point3::new(1.0, 0.0, 0.0));

        // the rail of each rail-end vertex use
        for rail in 0..2 {
            for end in 0..2 {
                let vu = fb.rail_vertexuse(g, rail, end).unwrap().unwrap();
                assert_eq!(fb.rail_index_of_vertexuse(g, Some(vu)).unwrap(), rail);
            }
        }

        // a vertex use on another edge: SM_ERR
        let a = vertex(&mut fb, &k, Point3::new(5.0, 5.0, 5.0));
        let b = vertex(&mut fb, &k, Point3::new(6.0, 5.0, 5.0));
        let (_, eus) = edge_with_uses(&mut fb, a, b, FilletEdgeType::CrossSection);
        let other = fb.brep().edgeuse(eus[0]).unwrap().vertexuse.unwrap();
        let err = fb.rail_index_of_vertexuse(g, Some(other)).unwrap_err();
        assert_eq!(code(&err), Some(SM_ERR));
    }

    #[test]
    fn set_up_rail_point_places_an_unprocessed_corner() {
        let k = Kernel::default();
        let mut fb = FilletBrep::new();
        let (g, _) = two_rail_geom(&mut fb, &k);

        // without a fillet surface: the point is set, then SM_ERR_NULL_POINTER
        let err = fb.set_up_rail_point(&k, g, 1, 1, 0.25, None).unwrap_err();
        assert_eq!(code(&err), Some(SM_ERR_NULL_POINTER));
        let v = fb.rail_vertex(g, 1, 1).unwrap();
        assert_eq!(fb.vertex_point(v).unwrap(), Point3::new(0.25, 1.0, 0.0));

        fb.geom_mut(g)
            .unwrap()
            .set_fillet_surface(Some(flat_patch()));
        let mut pc = PointClassification::new(1e-6);
        pc.set_class_object(
            PointClassType::Edge,
            Some(EntityRef::Edge(EdgeId::from_index(0))),
        );
        fb.set_up_rail_point(&k, g, 1, 1, 0.5, Some(&pc)).unwrap();
        let fv = fb.fillet_vertex(v).unwrap();
        assert!(fv.is_processed());
        assert_eq!(fv.fillet_vertex_type(), FilletVertexType::RailXEdgeuse);
        assert_eq!(fb.vertex_point(v).unwrap(), Point3::new(0.5, 1.0, 0.0));
        let vu = fb.rail_vertexuse(g, 1, 1).unwrap().unwrap();
        // the u parameter, at the v of the rail's corner of the domain
        assert_eq!(
            fb.fillet_vertexuse(vu).unwrap().uv_point(),
            Point2::new(0.5, 1.0)
        );

        // processed: nothing changes
        fb.set_up_rail_point(&k, g, 1, 1, 0.75, None).unwrap();
        assert_eq!(fb.vertex_point(v).unwrap(), Point3::new(0.5, 1.0, 0.0));
    }

    #[test]
    fn copy_geom_needs_the_same_solver_and_both_curves() {
        let k = Kernel::default();
        let mut fb = FilletBrep::new();
        let (a, _) = two_rail_geom(&mut fb, &k);
        let (b, rails_b) = two_rail_geom(&mut fb, &k);
        let solvers = [solver(false)];

        // different solvers
        fb.geom_mut(b).unwrap().solver = Some(FilletSolverId(1));
        let err = fb.copy_geom(&k, b, a, &solvers).unwrap_err();
        assert_eq!(code(&err), Some(SM_ERR));
        fb.geom_mut(b).unwrap().solver = Some(FilletSolverId(0));

        // no fillet surface to copy: NULL, SM_ERR_NULL_POINTER
        let err = fb.copy_geom(&k, b, a, &solvers).unwrap_err();
        assert_eq!(code(&err), Some(SM_ERR_NULL_POINTER));

        // both: the rails take the other rails' types, faces and curves
        fb.geom_mut(a)
            .unwrap()
            .set_fillet_surface(Some(flat_patch()));
        fb.geom_mut(a).unwrap().center_line_curve = Some(
            BSplineCurve::create_line_segment(
                3,
                Point3::new(0.0, 0.5, 1.0),
                Point3::new(1.0, 0.5, 1.0),
            )
            .unwrap()
            .into(),
        );
        fb.geom_mut(a).unwrap().deviations = [1e-3, 2e-3];
        let rail_a = fb.geom(a).unwrap().rail(0).unwrap();
        fb.fillet_edge_mut(rail_a)
            .unwrap()
            .set_original_face(Some(FaceId::from_index(7)));
        fb.brep_mut().edge_mut(rails_b[0]).unwrap().curve = None;
        fb.copy_geom(&k, b, a, &solvers).unwrap();
        let gb = fb.geom(b).unwrap();
        assert!(gb.fillet_surface(None).unwrap().is_some() && gb.center_line_curve().is_some());
        assert_eq!(gb.deviations, [1e-3, 2e-3]);
        assert_eq!(
            fb.fillet_edge(rails_b[0]).unwrap().original_face(),
            Some(FaceId::from_index(7))
        );
        assert!(fb.brep().edge(rails_b[0]).unwrap().curve.is_some());
    }

    #[test]
    fn join_follows_side_edges_in_3d() {
        let k = Kernel::default();
        let mut fb = FilletBrep::new();
        let (g, rails) = two_rail_geom(&mut fb, &k);
        let solvers = [solver(false)];
        let start = fb.rail_vertex(g, 0, 1).unwrap();
        let end = fb.rail_vertex(g, 1, 1).unwrap();
        // a side edge from the end of rail 1 back to the end of rail 0 (backwards)
        let (side, eus) = edge_with_uses(&mut fb, end, start, FilletEdgeType::CrossSection);
        fb.brep_mut().edge_mut(side).unwrap().curve = Some(line(
            Point3::new(1.0, 1.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
            0.0,
            1.0,
            3,
        ));
        fb.geom_mut(g).unwrap().add_side_fillet_edgeuse(eus[0]);

        let mut c3 = vec![fb.brep().edge(rails[0]).unwrap().curve.clone()];
        let mut cuv = vec![None];
        let mut orients = vec![OrientType::Same];
        let sides = fb.geom(g).unwrap().side_eus.clone();
        let made = join_two_fillet_verts_by_curves(
            &k,
            &fb,
            g,
            Some(start),
            Some(end),
            &sides,
            1e-6,
            &mut c3,
            &mut cuv,
            &mut orients,
            &solvers,
        )
        .unwrap();
        assert!(made);
        assert_eq!(orients, [OrientType::Same, OrientType::Opposite]);
        assert_eq!(c3.len(), 2);
        assert!(cuv.iter().all(Option::is_none));

        // empty lists: upstream reads GetLast() of an empty array
        let err = join_two_fillet_verts_by_curves(
            &k,
            &fb,
            g,
            Some(start),
            Some(end),
            &sides,
            1e-6,
            &mut Vec::new(),
            &mut Vec::new(),
            &mut Vec::new(),
            &solvers,
        )
        .unwrap_err();
        assert!(matches!(err, SolidError::InvalidInput { .. }), "{err:?}");

        // The same actual geometry can resolve a nonzero retained solver slot.
        let retained = [solver(false), solver(false)];
        let actual_id = FilletSolverId(1);
        fb.geom_mut(g).unwrap().solver = Some(actual_id);
        let mut live3 = vec![fb.brep().edge(rails[0]).unwrap().curve.clone()];
        let mut live_uv = vec![None];
        let mut live_orients = vec![OrientType::Same];
        let resolver = |id, _| {
            assert_eq!(id, actual_id);
            Ok(&retained[1])
        };
        assert!(join_two_fillet_verts_by_curves_resolved(
            &k,
            &fb,
            g,
            Some(start),
            Some(end),
            &sides,
            1e-6,
            &mut live3,
            &mut live_uv,
            &mut live_orients,
            &resolver,
        )
        .unwrap());
        assert_eq!(live_orients, orients);
        assert_eq!(live3.len(), c3.len());
        assert!(live_uv.iter().all(Option::is_none));

        // A real lookup failure occurs before any curve list is appended.
        let mut failed3 = vec![fb.brep().edge(rails[0]).unwrap().curve.clone()];
        let mut failed_uv = vec![None];
        let mut failed_orients = vec![OrientType::Same];
        let failed = join_two_fillet_verts_by_curves_resolved(
            &k,
            &fb,
            g,
            Some(start),
            Some(end),
            &sides,
            1e-6,
            &mut failed3,
            &mut failed_uv,
            &mut failed_orients,
            &|id, _| {
                assert_eq!(id, actual_id);
                Err(SolidError::sm_err("retained solver lookup"))
            },
        )
        .unwrap_err();
        assert_eq!(code(&failed), Some(SM_ERR));
        assert_eq!(failed3.len(), 1);
        assert_eq!(failed_uv.len(), 1);
        assert_eq!(failed_orients, [OrientType::Same]);
    }

    #[test]
    fn rail_edgeuse_intersect_restores_the_solver_geometry() {
        let k = Kernel::default();
        let mut fb = FilletBrep::new();
        let (g, _) = two_rail_geom(&mut fb, &k);
        let mut s = solver(false);
        let saved = Some(FilletGeomId(3));
        s.set_fillet_geom_for_surfaces(saved);
        let err = fb
            .rail_edgeuse_intersect(
                &k,
                g,
                &mut s,
                &mut Brep::new(),
                0,
                EdgeuseId::from_index(0),
                0.5,
                &mut TsectPnt::default(),
            )
            .unwrap_err();
        // the solver reads its surfaces through the geometry, which has none: upstream's
        // GetOffsetSurface recurses through GetSurface without bound (UB-204), refused
        assert!(!err.is_unported(), "{err:?}");
        assert!(matches!(err, SolidError::InvalidInput { .. }), "{err:?}");
        assert_eq!(s.fillet_geom_for_surfaces(), saved);
        assert_eq!(s.lent_geometry, None);

        // a geometry without solver: NER(pFS), the solver untouched
        fb.geom_mut(g).unwrap().solver = None;
        let err = fb
            .rail_edgeuse_intersect(
                &k,
                g,
                &mut s,
                &mut Brep::new(),
                0,
                EdgeuseId::from_index(0),
                0.5,
                &mut TsectPnt::default(),
            )
            .unwrap_err();
        assert_eq!(code(&err), Some(SM_ERR_NULL_POINTER));
        assert_eq!(s.fillet_geom_for_surfaces(), saved);
    }

    #[test]
    fn record_fillet_error_needs_an_executive() {
        let k = Kernel::default();
        let mut fb = FilletBrep::new();
        let (g, rails) = two_rail_geom(&mut fb, &k);
        fb.geom_mut(g).unwrap().status = FilStatus::Processed;
        let none = fb
            .record_fillet_error(
                g,
                FilletErrorType::BadTrim,
                None,
                None,
                None,
                None,
                None,
                &[solver(false)],
            )
            .unwrap_err();
        assert!(matches!(none, SolidError::InvalidInput { .. }), "{none:?}");

        let fe = EdgeId::from_index(4);
        let tf = FaceId::from_index(7);
        let mut info = ErrorInfo::default();
        fb.record_fillet_error_in(
            g,
            FilletErrorType::BadMerge,
            Some(fe),
            Some(tf),
            Some(rails[0]),
            Some(GeomErrorObject::FilletSurface(g)),
            Some("merge failed"),
            &[solver(true)],
            Some(&mut info),
        )
        .unwrap();
        assert_eq!(info.code, FilletErrorType::BadMerge as i32);
        assert_eq!(info.status, FilStatus::Processed as i32);
        assert_eq!(info.edge, Some(GeomErrorObject::FilletedEdge(fe)));
        assert_eq!(info.side_face, Some(GeomErrorObject::SideFace(tf)));
        assert_eq!(
            info.edges,
            vec![Some(GeomErrorObject::FilletEdge(rails[0]))]
        );
        assert_eq!(info.surfaces, vec![Some(GeomErrorObject::FilletSurface(g))]);
        assert_eq!(info.comment, "merge failed");
    }

    #[test]
    fn should_swap_without_a_point_is_false() {
        let k = Kernel::default();
        let mut fb = FilletBrep::new();
        let a = vertex(&mut fb, &k, Point3::new(0.0, 0.0, 0.0));
        let b = vertex(&mut fb, &k, Point3::new(1.0, 0.0, 0.0));
        let (e, _) = edge_with_uses(&mut fb, a, b, FilletEdgeType::Rail);
        let mut target = Brep::new();
        let f = FaceId::from_index(0);
        // no interval, classification or corner: "unhandled case", FALSE
        assert!(!fb.should_swap_faces(&k, e, f, f, &mut target, &[]).unwrap());
        // an interval but no curve: upstream dereferences NULL
        fb.brep_mut().edge_mut(e).unwrap().interval = Extent1d::from_min_max(0.0, 1.0);
        let err = fb
            .should_swap_faces(&k, e, f, f, &mut target, &[])
            .unwrap_err();
        assert_eq!(code(&err), Some(SM_ERR_NULL_POINTER));
    }

    #[test]
    fn find_edge_tangent_reads_edge_classifications_only() {
        let k = Kernel::default();
        let mut fb = FilletBrep::new();
        let (g, _) = two_rail_geom(&mut fb, &k);
        let mut target = Brep::new();
        let ta = target.add_vertex(Point3::new(0.0, 0.0, 0.0)).unwrap();
        let tb = target.add_vertex(Point3::new(0.0, 0.0, 2.0)).unwrap();
        let te = target.add_edge(ta, tb).unwrap();
        target.edge_mut(te).unwrap().curve = Some(line(
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(0.0, 0.0, 2.0),
            0.0,
            1.0,
            3,
        ));

        // unclassified: not found
        assert_eq!(
            fb.find_edge_tangent_at_point(&k, g, 0, 1, &target).unwrap(),
            None
        );

        let v = fb.rail_vertex(g, 0, 1).unwrap();
        let fv = fb.fillet_vertex_mut(v).unwrap();
        fv.set_point_class(PointClassType::Edge, Some(EntityRef::Edge(te)));
        fv.set_original_t_param(0.5);
        let t = fb
            .find_edge_tangent_at_point(&k, g, 0, 1, &target)
            .unwrap()
            .unwrap();
        assert!((t - Vec3::new(0.0, 0.0, 1.0)).length() < 1e-12);

        // a rail index past the two rails
        let err = fb
            .find_edge_tangent_at_point(&k, g, 2, 0, &target)
            .unwrap_err();
        assert!(matches!(err, SolidError::InvalidInput { .. }), "{err:?}");
    }

    #[test]
    fn retained_trim_keeps_body_marks_and_successful_prefix_on_error() {
        use crate::topology::topology::Topology;
        use std::collections::BTreeMap;

        #[derive(Default)]
        struct RetainedMarks(BTreeMap<(bool, Entity), Topology>);
        impl TrimOriginalSurfacesMarks for RetainedMarks {
            fn is_marked(
                &self,
                k: &Kernel,
                body: TrimOriginalBody,
                entity: Entity,
                mark: MarkType,
            ) -> bool {
                self.0
                    .get(&(body == TrimOriginalBody::Target, entity))
                    .is_some_and(|record| record.is_marked(&k.context, mark))
            }
            fn mark(&mut self, k: &Kernel, body: TrimOriginalBody, entity: Entity, mark: MarkType) {
                self.0
                    .entry((body == TrimOriginalBody::Target, entity))
                    .or_default()
                    .mark(Some(&k.context), mark);
            }
        }

        let mut k = Kernel::default();
        let mut fillet =
            crate::primitive::api_primitives::create_box(&mut k, Point3::ZERO, 10.0, 20.0, 30.0)
                .unwrap();
        let mut target =
            crate::primitive::api_primitives::create_box(&mut k, Point3::ZERO, 10.0, 20.0, 30.0)
                .unwrap();
        let mut pseudo = FilletBrep::new();
        let (g, rails) = two_rail_geom(&mut pseudo, &k);
        let id = FilletSolverId(23);
        pseudo.geom_mut(g).unwrap().solver = Some(id);
        let fs = solver(false);
        let edge = fillet.edge_list()[0];
        pseudo.fillet_edge_mut(rails[0]).unwrap().fillet_brep_edge1 = Some(edge);
        let lock = MarkLock::new(core::slice::from_mut(&mut k.context), 0, ALL_MARKS).unwrap();
        let mark = lock.mark_type();
        let locks = [lock];
        let mut records = RetainedMarks::default();
        let face = target.face_list()[0];
        assert_eq!(face, fillet.face_list()[0]);
        records.mark(&k, TrimOriginalBody::Target, Entity::Face(face), mark);
        let original_target_record = *records.0.get(&(true, Entity::Face(face))).unwrap();
        let key = |_: EdgeId| ObjectRef {
            type_code: 16001,
            key: ObjectKey(0),
        };
        // A dangling mate fails AFTER collecting the real fillet faces. This
        // defined Rust error tests retained prefix state, not native parity.
        let dangling_mate = |_: EdgeId| Some(EdgeId::from_index(u32::MAX));
        let env = TrimOriginalSurfacesEnv {
            brep_mate: &dangling_mate,
            fillet_edge_key: &key,
            target_context: Some(0),
        };
        let mut kept = Vec::new();
        let mut deleted = Vec::new();

        // Wrong actual solver is rejected before touching marks or outputs.
        let before = records.0.clone();
        let error = pseudo
            .trim_original_surfaces_with_solver(
                &k,
                g,
                &mut fillet,
                &mut target,
                &mut records,
                &mut kept,
                &mut deleted,
                &locks,
                &env,
                FilletSolverId(22),
                &fs,
            )
            .unwrap_err();
        assert!(matches!(error, SolidError::InvalidInput { .. }));
        assert_eq!(records.0, before);
        assert!(kept.is_empty() && deleted.is_empty());

        pseudo
            .trim_original_surfaces_with_solver(
                &k,
                g,
                &mut fillet,
                &mut target,
                &mut records,
                &mut kept,
                &mut deleted,
                &locks,
                &env,
                id,
                &fs,
            )
            .unwrap_err();
        assert_eq!(kept.len(), fillet.face_list().len());
        assert!(deleted.is_empty());
        assert_eq!(
            *records.0.get(&(true, Entity::Face(face))).unwrap(),
            original_target_record
        );
        for &f in fillet.face_list() {
            assert!(records.is_marked(&k, TrimOriginalBody::Fillet, Entity::Face(f), mark));
        }
        // One body-qualified store distinguishes equal local ids. The target
        // seed does not suppress the actual fillet traversal.
        assert!(records.is_marked(&k, TrimOriginalBody::Target, Entity::Face(face), mark));
    }

    #[test]
    fn trim_original_surfaces_skips_rails_without_mated_edges() {
        let mut k = Kernel::default();
        let mut fb = FilletBrep::new();
        let (g, rails) = two_rail_geom(&mut fb, &k);
        let solvers = [solver(false)];
        let lock = MarkLock::new(core::slice::from_mut(&mut k.context), 0, ALL_MARKS).unwrap();
        let locks = [lock];

        // a fillet B-rep edge (a wire: no face) for rail 0, its curve owned by it
        let mut fillet = Brep::new();
        let a = fillet.add_vertex(Point3::new(0.0, 0.0, 0.0)).unwrap();
        let b = fillet.add_vertex(Point3::new(1.0, 0.0, 0.0)).unwrap();
        let fe = fillet.add_edge(a, b).unwrap();
        let key = |e: EdgeId| ObjectRef {
            type_code: 16001,
            key: ObjectKey(u64::from(e.index())),
        };
        let mut c = line(
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
            0.0,
            1.0,
            3,
        );
        let _ = c.set_owner(Some(key(fe)), None);
        fillet.edge_mut(fe).unwrap().curve = Some(c);
        fb.fillet_edge_mut(rails[0]).unwrap().fillet_brep_edge1 = Some(fe);

        let mut target = Brep::new();
        let (mut fm, mut tm) = (Marks::default(), Marks::default());
        let (mut kept, mut deleted) = (Vec::new(), Vec::new());
        let unmated = |_: EdgeId| None;
        let env = TrimOriginalSurfacesEnv {
            brep_mate: &unmated,
            fillet_edge_key: &key,
            target_context: Some(0),
        };
        fb.trim_original_surfaces(
            &k,
            g,
            &mut fillet,
            &mut fm,
            &mut target,
            &mut tm,
            &mut kept,
            &mut deleted,
            &locks,
            &env,
            &solvers,
        )
        .unwrap();
        assert!(kept.is_empty() && deleted.is_empty());

        // mated to a target edge without faces: the (empty) fillet faces are collected,
        // the non-manifold target edge is skipped
        let ta = target.add_vertex(Point3::new(0.0, 0.0, 0.0)).unwrap();
        let tb = target.add_vertex(Point3::new(1.0, 0.0, 0.0)).unwrap();
        let te = target.add_edge(ta, tb).unwrap();
        let mated = move |_: EdgeId| Some(te);
        let env = TrimOriginalSurfacesEnv {
            brep_mate: &mated,
            fillet_edge_key: &key,
            target_context: Some(0),
        };
        let err = fb
            .trim_original_surfaces(
                &k,
                g,
                &mut fillet,
                &mut fm,
                &mut target,
                &mut tm,
                &mut kept,
                &mut deleted,
                &locks,
                &env,
                &solvers,
            )
            .unwrap_err();
        // the fillet B-rep edge has no edge use (upstream dereferences NULL)
        assert!(matches!(err, SolidError::InvalidInput { .. }), "{err:?}");

        // no solver, no mark locks
        let err = fb
            .trim_original_surfaces(
                &k,
                g,
                &mut fillet,
                &mut fm,
                &mut target,
                &mut tm,
                &mut kept,
                &mut deleted,
                &[],
                &env,
                &solvers,
            )
            .unwrap_err();
        assert!(matches!(err, SolidError::InvalidInput { .. }), "{err:?}");
        fb.geom_mut(g).unwrap().solver = None;
        let err = fb
            .trim_original_surfaces(
                &k,
                g,
                &mut fillet,
                &mut fm,
                &mut target,
                &mut tm,
                &mut kept,
                &mut deleted,
                &locks,
                &env,
                &solvers,
            )
            .unwrap_err();
        assert!(matches!(err, SolidError::InvalidInput { .. }), "{err:?}");
    }
}
