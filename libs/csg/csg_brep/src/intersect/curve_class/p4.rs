//! Curve classification, part 4: the full constructor, the loop encounters and loop-crossing
//! count of a classification curve on a face (`GetLoopTouches`, `GetLoopCrossings`), the
//! edge use bounding an interval
//! end (`GetIvlBound_EdgeuseObj`) and the squeeze of the shortest interval
//! (`sm_SqueezeOutSmallestInterval`).
//!
//! Upstream: `source/SMLib/src/SmCurveClass.cpp`, `source/SMLib/inc/SmCurveClass.h`.
//!
//! | Rust                                                    | Upstream                                         |
//! |---------------------------------------------------------|--------------------------------------------------|
//! | [`CurveClassification::with_curve`], [`ClassificationInit`] | `SmCurveClassification(cpCurve, crInterval, cpUVCurve, sSrcZoneTol3d, lOptDataSize, pOptBorrowedData, bIsCurveOwned, bIsCurveCurrent, cpSurface1, cpSurface2, cpUVCurve2, cpTI)` |
//! | [`CurveClassification::get_loop_crossings`], [`LoopCrossings`] | `SmCurveClassification::GetLoopCrossings`  |
//! | [`CurveClassification::get_loop_touches`]               | `SmCurveClassification::GetLoopTouches`          |
//! | `classify_seam_edgeuse` (private)                       | `sm_ClassifySeamEdgeuse` (static)                |
//! | `find_edgeuse_seam_sides` (private)                     | `sm_FindEdgeuseSeamSides` (static)               |
//! | [`CurveClassification::ivl_bound_edgeuse_obj`]          | `SmCurveInterval::GetIvlBound_EdgeuseObj`        |
//! | [`squeeze_out_smallest_interval`]                       | `sm_SqueezeOutSmallestInterval` (static)         |
//!
//! Deviations:
//! - **Object handles.** As in parts 1 and 3, every `SmObject` pointer is a caller-chosen
//!   handle `O` ([`ClassObject`]) whose geometry comes from a [`ClassGeometry`]; the kernel
//!   is passed explicitly. A loop, face, face use, edge, edge use, vertex or vertex use
//!   handle reaches its B-rep through the hooks [`ClassGeometry::topology_entity`] and
//!   [`ClassGeometry::topology_handle`], and the topology members (`SmLoop::GetFace`,
//!   `SmFace::GetUpwardFaceuse` / `GetSurface`, `SmEdge::GetEdgeuses` / `GetCurve` /
//!   `GetInterval`, `SmEdgeuse::GetEdge` / `GetOrientation` / `EvaluateBinormal`,
//!   `SmVertex::GetVertexusesOfFace`, `SmVertexuse::GetVertexuseType` /
//!   `ComputeUVSector`) are the ported ones; a handle the adapter does not resolve is
//!   [`SolidError::InvalidInput`] (a gap in the caller's adapter).
//! - **No back pointer.** `GetIvlBound_EdgeuseObj` is a member of the interval that reaches
//!   its classification through `m_pCurveClassification`; here it is a method of the
//!   classification taking the interval index (as in part 1).
//! - **Touch data.** `SmTouchData::m_pPtClassify` points into this classification's
//!   intervals; it is resolved through `m_lPtClassifyIndx` exactly as
//!   `GetPointClassifiesConnectedTo` set it (index 0: the first start, `i`: the end of
//!   interval `i - 1`).
//! - **Face properties.** `pOptFaceProps` is the real [`FaceProps`] (unit `heal.face_props`);
//!   its `UNSURE` tri-states read as true, as `if(m_bClosedU)` does. `SetProps_Stage2` runs
//!   on the B-rep and face id the face handle of `loops[0]` resolves to.
//! - **`ComputeUVSector`** takes `&mut Brep` in the port; with `bOkToMake` false it creates
//!   nothing, so it runs on a copy of the B-rep. Its status is ignored as upstream, but an
//!   error it returns (a failed evaluation, `Unported`) propagates instead of leaving the
//!   partial outputs.
//! - **Out-params.** `GetLoopCrossings` returns a [`LoopCrossings`] holding the three
//!   output arrays ([`LoopCrossings::sm_err`] is never set); a failing status it returns
//!   after filling them is `Ok` with [`LoopCrossings::status`] set and the arrays as
//!   upstream leaves them. `GetLoopTouches` returns the touch list (its failing status is
//!   the error). The constructor returns the classification.
//! - **Seam face.** `sm_ClassifySeamEdgeuse` reads the face of the face props
//!   (`m_pFace`, a [`FaceId`]); its handle is resolved in the B-rep of the first loop's
//!   face.
//! - **Ignored statuses.** Upstream checks none of the statuses inside these functions
//!   except `ConnectIntervals` and the seam-side classification of the curve's ends
//!   (`SER`); they are ignored here (`Unported` still
//!   propagates), and an output a failed call leaves unwritten keeps its initial value
//!   (`UNDEF` for points and vectors).
//! - **Uninitialised angle.** `Vec2(1, 0).CCWAngleBetween(tangent)` for a zero-length UV
//!   tangent fails upstream and leaves the caller's `double` angle uninitialised; the
//!   port reads it as [`UNDEF_DOUBLE`] (`ccw_angle`).
//! - **Undefined behaviour upstream** is an error here: a classification without a face,
//!   or a face without a surface or classification curve, where the code dereferences them
//!   before its own `NULL` checks (UB-124); an empty classification in
//!   `sm_SqueezeOutSmallestInterval` (`FindBestSqueezeEnd(0)` reads `m_pData[0]`); a touch
//!   whose interval index is past the intervals.
//! - **Not ported (reporting only).** `SM_ASSERT*`, `SM_DBG_WARN`, `ERR_MSG` and `MSG`
//!   messages and the `SM_DEBUG_CODE` blocks (the release build compiles them out).
//! - **Part 2 callees.** `ReSet`, `ConnectIntervals` and `SmCurveInterval::GetMinUV` /
//!   `GetMaxUV` are the part 2 ports; `GetMinUV` / `GetMaxUV` take the kernel, so
//!   [`CurveClassification::ivl_bound_edgeuse_obj`] does too.
//! - **`EvaluateBinormal`** takes `&mut Brep` in the port; with `bUse3DOnly` true it
//!   creates nothing, so it runs on a copy of the B-rep, as `ComputeUVSector` does. Both
//!   upstream call sites ignore its status: a failure keeps the out-vectors as upstream
//!   leaves them (an edge use outside a loop touches neither; any later failure sets the
//!   point to `(0,0,0)` and keeps the binormal), and only [`SolidError::Unported`] and
//!   [`SolidError::InvalidInput`] (a null dereference upstream) propagate.
//! - **Recursion.** None.

use crate::curve::curve::p2::{Curve, CurveKind};
use crate::error::{ignore_status, ignored, EntityRef, Result, SolidError};
use crate::geom::extent1d::{Extent1d, ExtentPointType};
use crate::geom::extent2d::{Extent2d, SS_NONE, SS_UMAX, SS_UMIN, SS_VMAX, SS_VMIN};
use crate::geom::global_solver::SolverOperation;
use crate::geom::periodic_extent1d::PeriodicExtent1d;
use crate::geom::solution_array::{ObjectKind, SolutionObject};
use crate::geom::tol::{
    are_same, eff_zero, is_degenerate_vec3, scaled_zero2, xsect_tol_3d_from_zones, zone_tol_3d,
    SmallTopoType, ANG_TOL_DEG,
};
use crate::heal::face_props::{FaceProps, HealerOpType, LoopObject, PropStage, Tri, UNDEF_ULONG};
use crate::intersect::curve_class::p1::{
    get_connected_touches, remove_non_crossings, CrvDirOnFaceType, CurveClassification, TouchData,
};
use crate::intersect::curve_class::p2::ResetOptions;
use crate::intersect::curve_class::p3::curve_is_degenerate;
use crate::intersect::curve_class::p5::ClassGeometryMut;
use crate::intersect::point_class::{
    object_zone_tol_3d, ClassGeometry, ClassObject, PointClassType, PointClassification,
};
use crate::kernel::Kernel;
use crate::math::{Point2, Point3, Vec2, Vec3, EFF_ZERO, PI, UNDEF_DOUBLE};
use crate::nurbs::b_spline_surface::SurfParamType;
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::nurbs::knots::Side;
use crate::surface::surface::p1::{map_to_2d, map_to_2d_dir};
use crate::surface::surface::p5::DropPointOptions;
use crate::surface::surface::p7::drop_vectors_at;
use crate::surface::surface::Surface;
use crate::topology::edge as topo_edge;
use crate::topology::edgeuse::p1 as eu1;
use crate::topology::edgeuse::p3 as topo_eu3;
use crate::topology::face::p2 as face_p2;
use crate::topology::loopuse;
use crate::topology::vertex as topo_vertex;
use crate::topology::vertexuse as topo_vertexuse;
use crate::topology::{Brep, EdgeId, EdgeuseId, FaceId, VertexuseId};

const OP_CROSS: &str = "intersect::curve_class::p4::CurveClassification::get_loop_crossings";
/// `SM_ERR_ASSERT_FAILURE` (`SmMessages.h`).
const SM_ERR_ASSERT_FAILURE: i32 = 1005;
const OP_BOUND: &str = "intersect::curve_class::p4::CurveClassification::ivl_bound_edgeuse_obj";
const OP_SQUEEZE: &str = "intersect::curve_class::p4::squeeze_out_smallest_interval";

/// `SM_2PI`.
const TWO_PI: f64 = 2.0 * PI;

// ---- inputs and outputs -----------------------------------------------------------------

/// The arguments of the full constructor ([`CurveClassification::with_curve`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClassificationInit<O> {
    /// `cpCurve`: a 2d or 3d curve.
    pub curve: Option<O>,
    /// `crInterval`: the interval of the curve.
    pub interval: Extent1d,
    /// `cpUVCurve`: the associated UV trim curve (on surface 1 for intersections).
    pub uv_curve: Option<O>,
    /// `sSrcZoneTol3d`: the zone tolerance of the curve's source.
    pub src_zone_tol_3d: f64,
    /// `lOptDataSize`: the size of the caller-managed interval array (`pOptBorrowedData`
    /// itself is not modelled, see part 1).
    pub opt_data_size: usize,
    /// `bIsCurveOwned`.
    pub is_curve_owned: bool,
    /// `bIsCurveCurrent`.
    pub is_curve_current: bool,
    /// `cpSurface1`.
    pub surface1: Option<O>,
    /// `cpSurface2`.
    pub surface2: Option<O>,
    /// `cpUVCurve2`.
    pub uv_curve2: Option<O>,
    /// `cpTI`.
    pub ti: Option<O>,
}

/// The outputs of [`CurveClassification::get_loop_crossings`].
#[derive(Debug, Clone, PartialEq)]
pub struct LoopCrossings<O> {
    /// `pCrossingCounts`: one crossing count per target loop.
    pub crossing_counts: Vec<u64>,
    /// `pOptRawTouchDirList`: the complete arrive/depart pairs of `GetLoopTouches`; a
    /// closed curve's two seam ends remain distinct.
    pub raw_touches: Vec<TouchData<O>>,
    /// `pOptDoneTouchDirList`: the processed touch pairs the counts come from.
    pub done_touches: Vec<TouchData<O>>,
    /// Kept for callers: never set. `GetLoopCrossings` no longer has the `SM_ERR`
    /// returns it reported.
    pub sm_err: bool,
    /// The failing status upstream returns after filling the arrays (`SER` of the
    /// seam-side classification of the curve's ends, the `AERN` seam-join check, the
    /// `SER` of `RemoveNonCrossings`): the arrays are as upstream leaves them. `None` on
    /// success.
    pub status: Option<SolidError>,
}

/// Whether `e` is a status upstream returns (kept with the arrays it leaves), rather than
/// an unported callee or upstream undefined behaviour (propagated as an error).
const fn is_status(e: &SolidError) -> bool {
    !matches!(
        e,
        SolidError::Unported { .. }
            | SolidError::InvalidInput { .. }
            | SolidError::PortInvariant { .. }
            | SolidError::NotFound { .. }
    )
}

/// The parts of `SmUVSectorIO` (`SmVertexuse::ComputeUVSector`) the loop crossings read.
#[derive(Debug, Clone, Copy, PartialEq)]
struct UvSectorIo<O> {
    /// `m_pBegEdgeuse`.
    beg_edgeuse: Option<O>,
    /// `m_pEndEdgeuse`.
    end_edgeuse: Option<O>,
    /// `m_dBegEdgeParam`.
    beg_edge_param: f64,
    /// `m_dEndEdgeParam`.
    end_edge_param: f64,
    /// `m_sBegEdgeUVPoint`.
    beg_edge_uv_point: Point2,
    /// `m_sEndEdgeUVPoint`.
    end_edge_uv_point: Point2,
    /// `m_sBegEdgeUVTan`.
    beg_edge_uv_tan: Vec2,
    /// `m_sEndEdgeUVTan`.
    end_edge_uv_tan: Vec2,
    /// `m_bVertexOnPole`.
    vertex_on_pole: bool,
}

/// The handle of the face surface inside this unit's point drops.
#[derive(Debug, Clone, Copy, PartialEq)]
struct SurfHandle;

impl SolutionObject for SurfHandle {
    fn kind(&self) -> ObjectKind {
        ObjectKind::Surface
    }
}

// ---- part 2 callees -------------------------------------------------------------------

/// `SmCurveClassification::ReSet(...)` with the constructor's arguments (`lOptDataSize`
/// and `pOptBorrowedData` are not modelled, see part 1).
fn reset<O: ClassObject>(
    k: &Kernel,
    cc: &mut CurveClassification<O>,
    init: &ClassificationInit<O>,
) -> Result<()> {
    let options = ResetOptions {
        curve: init.curve,
        interval: Some(init.interval),
        uv_curve: init.uv_curve,
        zone_tol_3d: init.src_zone_tol_3d,
        is_curve_owned: init.is_curve_owned,
        is_curve_current: init.is_curve_current,
        surface1: init.surface1,
        surface2: init.surface2,
        uv_curve2: init.uv_curve2,
        ti: init.ti,
    };
    cc.reset(k, &options)
}

/// `SmCurveInterval::GetMinUV` (`do_min`) or `GetMaxUV` of interval `index`, starting
/// from an undefined point.
fn interval_end_uv<O: ClassObject, G: ClassGeometry<O>>(
    k: &Kernel,
    geometry: &G,
    cc: &CurveClassification<O>,
    index: usize,
    do_min: bool,
) -> Result<Point2> {
    if do_min {
        cc.interval_min_uv(k, geometry, index, Vec2::UNDEF)
    } else {
        cc.interval_max_uv(k, geometry, index, Vec2::UNDEF)
    }
}

// ---- topology members, through the handle hooks ------------------------------------------

/// The B-rep entity behind topology handle `o` ([`ClassGeometry::topology_entity`]).
fn entity_of<'g, O: ClassObject, G: ClassGeometry<O>>(
    geometry: &'g G,
    o: O,
    op: &'static str,
) -> Result<(&'g Brep, EntityRef)> {
    geometry
        .topology_entity(o)
        .ok_or_else(|| SolidError::invalid_input(op, "unresolved topology handle"))
}

/// The handle of entity `e` of B-rep `b` ([`ClassGeometry::topology_handle_in`]).
fn handle_of<O: ClassObject, G: ClassGeometry<O>>(
    geometry: &G,
    b: &Brep,
    e: EntityRef,
    op: &'static str,
) -> Result<O> {
    geometry
        .topology_handle_in(b, e)
        .ok_or_else(|| SolidError::invalid_input(op, "topology entity without handle"))
}

/// A handle of the wrong topology kind (`reason` names the wanted kind).
const fn wrong_kind(op: &'static str, reason: &'static str) -> SolidError {
    SolidError::invalid_input(op, reason)
}

/// The face of face handle `face`.
fn face_id<'g, O: ClassObject, G: ClassGeometry<O>>(
    geometry: &'g G,
    face: O,
    op: &'static str,
) -> Result<(&'g Brep, FaceId)> {
    match entity_of(geometry, face, op)? {
        (b, EntityRef::Face(f)) => Ok((b, f)),
        _ => Err(wrong_kind(op, "handle is not a face")),
    }
}

/// The edge of edge handle `edge`.
fn edge_id<'g, O: ClassObject, G: ClassGeometry<O>>(
    geometry: &'g G,
    edge: O,
    op: &'static str,
) -> Result<(&'g Brep, EdgeId)> {
    match entity_of(geometry, edge, op)? {
        (b, EntityRef::Edge(e)) => Ok((b, e)),
        _ => Err(wrong_kind(op, "handle is not a edge")),
    }
}

/// The edge use of edge use handle `edgeuse`.
fn edgeuse_id<'g, O: ClassObject, G: ClassGeometry<O>>(
    geometry: &'g G,
    edgeuse: O,
    op: &'static str,
) -> Result<(&'g Brep, EdgeuseId)> {
    match entity_of(geometry, edgeuse, op)? {
        (b, EntityRef::Edgeuse(eu)) => Ok((b, eu)),
        _ => Err(wrong_kind(op, "handle is not a edge use")),
    }
}

/// The vertex use of vertex use handle `vertexuse`.
fn vertexuse_id<'g, O: ClassObject, G: ClassGeometry<O>>(
    geometry: &'g G,
    vertexuse: O,
    op: &'static str,
) -> Result<(&'g Brep, VertexuseId)> {
    match entity_of(geometry, vertexuse, op)? {
        (b, EntityRef::Vertexuse(vu)) => Ok((b, vu)),
        _ => Err(wrong_kind(op, "handle is not a vertex use")),
    }
}

/// `SmFaceProps::SetProps_Stage2(face, faceIndex, SM_HO_ALL)` (unit `heal.face_props`) on
/// the face of `face`, its status ignored as upstream; a `NULL` face (dereferenced upstream,
/// UB-131) and a face handle `geometry` does not resolve are errors.
fn set_props_stage_2<O: LoopObject, G: ClassGeometry<O>>(
    k: &Kernel,
    geometry: &G,
    props: &mut FaceProps<O>,
    face: Option<O>,
    face_index: u64,
) -> Result<()> {
    let face =
        face.ok_or_else(|| SolidError::invalid_input(OP_CROSS, "loop without a face (UB-131)"))?;
    let (b, f) = face_id(geometry, face, OP_CROSS)?;
    // an `InvalidInput` of `FaceProps` is an upstream null dereference (UB-131 for the
    // face pointer `SetProps_Gaps` left unset): it propagates instead of being ignored
    match props.set_props_stage2(k, b, f, face_index, HealerOpType::All) {
        Err(e @ (SolidError::InvalidInput { .. } | SolidError::PortInvariant { .. })) => Err(e),
        r => ignore_status(r),
    }
}

/// [`set_props_stage_2`] on the face's B-rep in place
/// ([`FaceProps::set_props_stage2_mut`]).
fn set_props_stage_2_mut<O: LoopObject, G: ClassGeometryMut<O>>(
    k: &Kernel,
    geometry: &mut G,
    props: &mut FaceProps<O>,
    face: Option<O>,
    face_index: u64,
) -> Result<()> {
    let face =
        face.ok_or_else(|| SolidError::invalid_input(OP_CROSS, "loop without a face (UB-131)"))?;
    let Some((b, EntityRef::Face(f))) = geometry.topology_entity_mut(face) else {
        return Err(SolidError::invalid_input(
            OP_CROSS,
            "unresolved mutable face",
        ));
    };
    match props.set_props_stage2_mut(k, b, f, face_index, HealerOpType::All) {
        Err(e @ (SolidError::InvalidInput { .. } | SolidError::PortInvariant { .. })) => Err(e),
        r => ignore_status(r),
    }
}

/// `SmLoop::GetFace` (unit `topology.loopuse`).
fn loop_face<O: ClassObject, G: ClassGeometry<O>>(geometry: &G, lp: O) -> Result<Option<O>> {
    let (b, EntityRef::Loop(l)) = entity_of(geometry, lp, OP_CROSS)? else {
        return Err(wrong_kind(OP_CROSS, "handle is not a loop"));
    };
    loopuse::loop_face(b, l)?
        .map(|f| handle_of(geometry, b, EntityRef::Face(f), OP_CROSS))
        .transpose()
}

/// `SmFace::GetUpwardFaceuse` (unit `topology.face_p2`).
fn face_upward_faceuse<O: ClassObject, G: ClassGeometry<O>>(
    geometry: &G,
    face: O,
    op: &'static str,
) -> Result<Option<O>> {
    let (b, f) = face_id(geometry, face, op)?;
    face_p2::upward_faceuse(b, f)?
        .map(|fu| handle_of(geometry, b, EntityRef::Faceuse(fu), op))
        .transpose()
}

/// `SmFace::GetSurface` (unit `topology.face_p2`).
fn face_surface<'g, O: ClassObject, G: ClassGeometry<O>>(
    geometry: &'g G,
    face: O,
    op: &'static str,
) -> Result<Option<&'g Surface>> {
    let (b, f) = face_id(geometry, face, op)?;
    Ok(b.face(f)?.surface.as_ref())
}

/// The B-rep entity of an optional connected-to handle (`NULL` stays `None`).
fn connected_entity<O: ClassObject, G: ClassGeometry<O>>(
    geometry: &G,
    o: Option<O>,
    op: &'static str,
) -> Result<Option<EntityRef>> {
    o.map(|o| entity_of(geometry, o, op).map(|(_, e)| e))
        .transpose()
}

/// `SmEdge::GetEdgeuses(edgeuses, NULL, faceuse)` (unit `topology.edge`).
fn edge_edgeuses<O: ClassObject, G: ClassGeometry<O>>(
    geometry: &G,
    edge: O,
    faceuse: Option<O>,
    op: &'static str,
) -> Result<Vec<O>> {
    let (b, e) = edge_id(geometry, edge, op)?;
    let connected = connected_entity(geometry, faceuse, op)?;
    topo_edge::edgeuses(b, e, None, connected)?
        .into_iter()
        .map(|eu| handle_of(geometry, b, EntityRef::Edgeuse(eu), op))
        .collect()
}

/// `SmEdge::GetCurve` (unit `topology.edge`), dereferenced upstream.
fn edge_curve<O: ClassObject, G: ClassGeometry<O>>(geometry: &G, edge: O) -> Result<&Curve> {
    let (b, e) = edge_id(geometry, edge, OP_CROSS)?;
    topo_edge::curve(b, e)?.ok_or_else(|| SolidError::null_pointer(OP_CROSS))
}

/// `SmEdge::GetInterval` (unit `topology.edge`).
fn edge_interval<O: ClassObject, G: ClassGeometry<O>>(geometry: &G, edge: O) -> Result<Extent1d> {
    let (b, e) = edge_id(geometry, edge, OP_CROSS)?;
    topo_edge::interval(b, e)
}

/// `SmEdgeuse::GetEdge` (unit `topology.edgeuse_p1`), dereferenced upstream.
fn edgeuse_edge<O: ClassObject, G: ClassGeometry<O>>(geometry: &G, edgeuse: O) -> Result<O> {
    let (b, eu) = edgeuse_id(geometry, edgeuse, OP_CROSS)?;
    let e = eu1::edge(b, eu)?.ok_or_else(|| SolidError::null_pointer(OP_CROSS))?;
    handle_of(geometry, b, EntityRef::Edge(e), OP_CROSS)
}

/// `SmEdgeuse::GetOrientation` (unit `topology.edgeuse_p1`).
fn edgeuse_orientation<O: ClassObject, G: ClassGeometry<O>>(
    geometry: &G,
    edgeuse: O,
    op: &'static str,
) -> Result<OrientType> {
    let (b, eu) = edgeuse_id(geometry, edgeuse, op)?;
    eu1::orientation(b, eu)
}

/// `pEdgeuse->EvaluateBinormal(param, TRUE, point, binormal, &dir, &normal)` (unit
/// `topology.edgeuse_p3`) with the status ignored, as both upstream call sites do. The
/// two out-vectors are the caller's `point` and `binormal`: on success both are written;
/// on a failing status they keep what upstream leaves:
/// - an edge use that is not a loop edge use fails (`SER(SM_ERR_INVALID_INPUT)`) before
///   any output is initialised: both keep their contents;
/// - every later failure (curve or surface evaluation, no usable tangent or binormal,
///   edge point not on the surface) comes after `rBinormalPoint.Set(0,0,0)` and before
///   `rBinormal` is assigned: `point` is `(0,0,0)`, `binormal` keeps its contents.
///
/// Only [`SolidError::Unported`] and [`SolidError::InvalidInput`] propagate: the latter is
/// a null pointer upstream dereferences inside the call (UB, as in `set_props_stage_2`),
/// not a status.
///
/// With `bUse3DOnly` true no UV trim curve is created, so the port runs it on a copy of
/// the B-rep (it takes `&mut Brep`).
fn edgeuse_evaluate_binormal<O: ClassObject, G: ClassGeometry<O>>(
    k: &Kernel,
    geometry: &G,
    edgeuse: O,
    param: f64,
    point: &mut Vec3,
    binormal: &mut Vec3,
) -> Result<()> {
    let (b, eu) = edgeuse_id(geometry, edgeuse, OP_CROSS)?;
    if !eu1::is_loop_edgeuse(b, eu)? {
        return Ok(());
    }
    let mut scratch = b.clone_sharing_surface_caches();
    match topo_eu3::evaluate_binormal(
        k,
        &mut scratch,
        eu,
        param,
        true,
        &topo_eu3::BinormalOptions::default(),
    ) {
        Ok(r) => {
            *point = r.point;
            *binormal = r.binormal;
            Ok(())
        }
        Err(
            e @ (SolidError::Unported { .. }
            | SolidError::InvalidInput { .. }
            | SolidError::PortInvariant { .. }),
        ) => Err(e),
        Err(_) => {
            *point = Vec3::ZERO;
            Ok(())
        }
    }
}

/// `SmVertex::GetVertexusesOfFace(face, vertexuses, faceuse)` (unit `topology.vertex`).
fn vertex_vertexuses_of_face<O: ClassObject, G: ClassGeometry<O>>(
    geometry: &G,
    vertex: O,
    face: Option<O>,
    faceuse: Option<O>,
) -> Result<Vec<O>> {
    let (b, EntityRef::Vertex(v)) = entity_of(geometry, vertex, OP_CROSS)? else {
        return Err(wrong_kind(OP_CROSS, "handle is not a vertex"));
    };
    let face = face
        .map(|f| face_id(geometry, f, OP_CROSS).map(|(_, f)| f))
        .transpose()?;
    let connected = connected_entity(geometry, faceuse, OP_CROSS)?;
    topo_vertex::vertexuses_of_face(b, v, face, connected)?
        .into_iter()
        .map(|vu| handle_of(geometry, b, EntityRef::Vertexuse(vu), OP_CROSS))
        .collect()
}

/// `SmVertexuse::GetVertexuseType() == SmLoopuse_TYPE` (unit `topology.vertexuse`).
fn vertexuse_is_loop_vertexuse<O: ClassObject, G: ClassGeometry<O>>(
    geometry: &G,
    vertexuse: O,
) -> Result<bool> {
    let (b, vu) = vertexuse_id(geometry, vertexuse, OP_CROSS)?;
    Ok(topo_vertexuse::vertexuse_type(b, vu)? == topo_vertexuse::LOOPUSE_TYPE)
}

/// `SmVertexuse::ComputeUVSector(sectorIO, FALSE)` (unit `topology.vertexuse`) with the
/// status ignored: a failed computation keeps the outputs it wrote. With `bOkToMake` false
/// nothing is created, so the port runs it on a copy of the B-rep (it takes `&mut Brep`).
fn vertexuse_compute_uv_sector<O: ClassObject, G: ClassGeometry<O>>(
    k: &Kernel,
    geometry: &G,
    vertexuse: O,
) -> Result<UvSectorIo<O>> {
    let (b, vu) = vertexuse_id(geometry, vertexuse, OP_CROSS)?;
    let mut scratch = b.clone_sharing_surface_caches();
    let io = topo_vertexuse::compute_uv_sector(k, &mut scratch, vu, false)?.io;
    let eu_handle = |eu: Option<EdgeuseId>| -> Result<Option<O>> {
        eu.map(|eu| handle_of(geometry, b, EntityRef::Edgeuse(eu), OP_CROSS))
            .transpose()
    };
    Ok(UvSectorIo {
        beg_edgeuse: eu_handle(io.beg_edgeuse)?,
        end_edgeuse: eu_handle(io.end_edgeuse)?,
        beg_edge_param: io.beg_edge_param,
        end_edge_param: io.end_edge_param,
        beg_edge_uv_point: io.beg_edge_uv_point,
        end_edge_uv_point: io.end_edge_uv_point,
        beg_edge_uv_tan: io.beg_edge_uv_tan,
        end_edge_uv_tan: io.end_edge_uv_tan,
        vertex_on_pole: io.vertex_on_pole,
    })
}

// ---- seam sides (v1.0.1) -------------------------------------------------------------------

/// `sm_FindEdgeuseSeamSides` (static): which periodic sides of `direction` edge use
/// `edgeuse` occupies, from its connected curves: bit 1 the minimum side, bit 2 the
/// maximum, `0` unanchored. Both ways from the encounter at `parameter`, each curve is
/// sampled at growing fractions towards its far end and dropped onto the surface (each
/// drop guessed by the last); the first sample farther than `tolerance` in 3D from the
/// seam decides that way's side; segmented seam edges are followed through the loop
/// until a curve leaves the seam. Independent of loop winding and of UV trims.
///
/// # Errors
///
/// `SM_ERR` for a drop that fails; those of the evaluations and drops (`SER`); a loop
/// neighbour that is `NULL` (`NER`).
fn find_edgeuse_seam_sides(
    k: &Kernel,
    b: &Brep,
    edgeuse: EdgeuseId,
    parameter: f64,
    surface: &Surface,
    direction: SurfParamType,
    tolerance: f64,
) -> Result<u32> {
    const FRACTIONS: [f64; 6] = [1e-4, 1e-3, 1e-2, 0.1, 0.5, 0.9];
    let mut sides = 0u32;
    let domain = surface.natural_uv_domain_with_nurb(k)?;
    // Start close to the encounter, then look farther along a coincident run.
    // Use a 3D seam neighborhood rather than a UV tolerance near a pole.
    for way in 0..2 {
        let mut current = edgeuse;
        let mut found = false;
        let mut uv_guess: Option<Point2> = None;
        loop {
            let edge = eu1::edge(b, current)?.ok_or_else(|| SolidError::null_pointer(OP_CROSS))?;
            let curve =
                topo_edge::curve(b, edge)?.ok_or_else(|| SolidError::null_pointer(OP_CROSS))?;
            let interval = topo_edge::interval(b, edge)?;
            let increasing = (eu1::orientation(b, current)? == OrientType::Same) == (way == 0);
            let start = if current == edgeuse {
                parameter
            } else if increasing {
                interval.min()
            } else {
                interval.max()
            };
            let end = if increasing {
                interval.max()
            } else {
                interval.min()
            };
            for fraction in FRACTIONS {
                if start == end {
                    break;
                }
                let point = curve.evaluate_point(k, start + fraction * (end - start))?;
                let opts = DropPointOptions {
                    guess_uv: uv_guess,
                    operation: SolverOperation::Minimize,
                    tgt_seam_side: None,
                    want_solutions: false,
                };
                let drop = surface.drop_point::<SurfHandle>(k, point, &domain, &opts)?;
                if !drop.success {
                    return Err(SolidError::sm_err(OP_CROSS));
                }
                let uv = drop.result_uv;
                uv_guess = Some(uv);
                let mut seam_uv = uv;
                if direction == SurfParamType::U {
                    seam_uv.x = domain.u_min();
                } else {
                    seam_uv.y = domain.v_min();
                }
                let seam_point = surface.evaluate_point(k, seam_uv)?;
                let surface_point = surface.evaluate_point(k, uv)?;
                if surface_point.distance(seam_point) <= tolerance {
                    continue;
                }
                let (position, middle) = if direction == SurfParamType::U {
                    (uv.x, 0.5 * (domain.u_min() + domain.u_max()))
                } else {
                    (uv.y, 0.5 * (domain.v_min() + domain.v_max()))
                };
                sides |= if position < middle { 1 } else { 2 };
                found = true;
                break;
            }
            if found {
                break;
            }
            current = if way == 0 {
                eu1::ccw_edgeuse(b, current)?
            } else {
                eu1::cw_edgeuse(b, current)?
            }
            .ok_or_else(|| SolidError::null_pointer(OP_CROSS))?;
            if current == edgeuse {
                break;
            }
        }
    }
    Ok(sides)
}

// ---- helpers ------------------------------------------------------------------------------

/// `pCurve->Evaluate(t, 1, TRUE, pn)` with the status ignored (an unwritten output stays
/// `UNDEF`).
fn eval_pn(k: &Kernel, curve: &Curve, t: f64) -> Result<[Vec3; 2]> {
    let mut pn = [Vec3::UNDEF; 2];
    eval_pn_into(k, curve, t, &mut pn)?;
    Ok(pn)
}

/// [`eval_pn`] into the caller's array: a failing status leaves the array as it was (the
/// previous sample's values when the array outlives the loop that fills it).
fn eval_pn_into(k: &Kernel, curve: &Curve, t: f64, pn: &mut [Vec3; 2]) -> Result<()> {
    if let Some(d) = ignored(curve.evaluate(k, t, 1, true, true))? {
        for (o, v) in pn.iter_mut().zip(d) {
            *o = v;
        }
    }
    Ok(())
}

/// `pSurface->DropPoint(pt, domain, guess, ..., op, seamSide)` with the status ignored:
/// the dropped UV point (`initial` when the call fails).
fn drop_uv(
    k: &Kernel,
    surface: &Surface,
    pt: Point3,
    domain: &Extent2d,
    guess: Option<Point2>,
    tgt_seam_side: Option<Vec3>,
    initial: Point2,
) -> Result<Point2> {
    let opts = DropPointOptions {
        guess_uv: guess,
        operation: SolverOperation::Minimize,
        tgt_seam_side,
        want_solutions: false,
    };
    Ok(
        ignored(surface.drop_point::<SurfHandle>(k, pt, domain, &opts))?
            .map_or(initial, |d| d.result_uv),
    )
}

/// `pSurface->DropVectors(uv, TRUE, TRUE, 1, &v, &out)` with the status ignored.
fn drop_vector(k: &Kernel, surface: &Surface, uv: Point2, v: Vec3) -> Result<Vec2> {
    Ok(ignored(drop_vectors_at(
        k,
        surface,
        uv,
        Side::Left,
        Side::Left,
        &[v],
    ))?
    .and_then(|d| d.first().copied())
    .unwrap_or(Vec2::UNDEF))
}

/// `sU.CCWAngleBetween(v, angle)` with the status ignored.
fn ccw_angle(v: Vec2) -> Result<f64> {
    Ok(ignored(Vec2::new(1.0, 0.0).ccw_angle_between(v))?.unwrap_or(UNDEF_DOUBLE))
}

/// `SM_IS_BETWEEN(c, a, b)` (`SmMath.h`).
fn is_between(c: f64, a: f64, b: f64) -> bool {
    let z = scaled_zero2(&a, &b);
    ((a + z) < c && c < (b - z)) || ((b + z) < c && c < (a - z))
}

/// `uv[eParam]`: the coordinate of `uv` in direction `param` (`U` = 0, else 1).
fn uv_coord(uv: Point2, param: SurfParamType) -> f64 {
    if param == SurfParamType::U {
        uv.x
    } else {
        uv.y
    }
}

/// The `SM_SS_*` mask of the poles of `props` within its zone tolerance of `pt`.
fn on_pole_mask<O: LoopObject>(props: &FaceProps<O>, pt: Point3) -> u32 {
    let near = |i: usize, bit: u32| -> u32 {
        match props.pole_points.get(i) {
            Some(p) if p.is_initialized() && pt.distance(*p) < props.zone_tol_3d => bit,
            _ => 0,
        }
    };
    near(0, SS_UMIN) + near(1, SS_VMIN) + near(2, SS_UMAX) + near(3, SS_VMAX)
}

/// The touch end direction on a natural boundary of the surface.
fn boundary_dir(no_vertex_pole: u64, on_pole: u32, on_seam: bool) -> CrvDirOnFaceType {
    if no_vertex_pole != u64::from(SS_NONE) {
        CrvDirOnFaceType::PoleNoVertex
    } else if on_pole != SS_NONE {
        CrvDirOnFaceType::Pole
    } else if on_seam {
        CrvDirOnFaceType::Seam
    } else {
        CrvDirOnFaceType::Lamina
    }
}

/// A natural-boundary direction (`LAMINA`, `SEAM`, `POLE`, `POLE_NO_VERTEX`).
fn is_boundary_dir(d: CrvDirOnFaceType) -> bool {
    matches!(
        d,
        CrvDirOnFaceType::Lamina
            | CrvDirOnFaceType::Seam
            | CrvDirOnFaceType::Pole
            | CrvDirOnFaceType::PoleNoVertex
    )
}

/// `rTouch = Get(index)` with a range check.
fn touch_at<O: Copy>(list: &[TouchData<O>], index: usize) -> Result<TouchData<O>> {
    list.get(index)
        .copied()
        .ok_or_else(|| SolidError::invalid_input(OP_CROSS, "touch index past the list"))
}

/// `GetAt(index) = value` with a range check.
fn set_touch<O>(list: &mut [TouchData<O>], index: usize, value: TouchData<O>) -> Result<()> {
    let slot = list
        .get_mut(index)
        .ok_or_else(|| SolidError::invalid_input(OP_CROSS, "touch index past the list"))?;
    *slot = value;
    Ok(())
}

/// Where a `ClassifyCrv` end lies relative to the seams of a closed surface: the
/// `ebTouch_*OnSeamU` / `ebTouch_*OnSeamV` pair for one end of the classification curve.
#[allow(clippy::too_many_arguments)]
fn end_on_seams<O: LoopObject>(
    k: &Kernel,
    surface: &Surface,
    props: &FaceProps<O>,
    on_pole: u32,
    on_seam: SurfParamType,
    end_pn: &[Vec3; 2],
    srf_min_uv: Point2,
    srf_max_uv: Point2,
    is_start: bool,
) -> Result<(bool, bool)> {
    // when Closed3d ClassifyCrv starts/stops on a pole - also on the Seams when not
    // coincident to the Seam
    if on_pole != SS_NONE {
        let u = props.closed_u != Tri::False && on_seam != SurfParamType::U;
        let v = props.closed_v != Tri::False && on_seam != SurfParamType::V;
        return Ok((u, v));
    }

    // drop End to Surface
    let mut end_uv = drop_uv(
        k,
        surface,
        end_pn[0],
        &props.natural_uv_domain,
        None,
        None,
        Vec2::UNDEF,
    )?;

    // Use CrvTangent to pick proper SeamSide for the end's DegenParam
    let (du, dv) = match ignored(surface.evaluate_1st_derivatives(k, end_uv, true, true))? {
        Some((_, du, dv)) => (du, dv),
        None => (Vec3::UNDEF, Vec3::UNDEF),
    };
    // start: positive dot -> min side; stop: positive dot -> max side
    let (pos, neg) = if is_start {
        (srf_min_uv, srf_max_uv)
    } else {
        (srf_max_uv, srf_min_uv)
    };
    if props.closed_u != Tri::False && on_seam != SurfParamType::U {
        end_uv.x = if end_pn[1].dot(du) > 0.0 {
            pos.x
        } else {
            neg.x
        };
    }
    if props.closed_v != Tri::False && on_seam != SurfParamType::V {
        end_uv.y = if end_pn[1].dot(dv) > 0.0 {
            pos.y
        } else {
            neg.y
        };
    }

    // classify end against seam
    let seam = surface.is_on_seam(k, end_uv, None)?;
    let dir = seam.seam_dir;
    let u = seam.on_seam
        && on_seam != SurfParamType::U
        && (dir == SurfParamType::Both || dir == SurfParamType::U);
    let v = seam.on_seam
        && on_seam != SurfParamType::V
        && (dir == SurfParamType::Both || dir == SurfParamType::V);
    Ok((u, v))
}

// ---- sm_SqueezeOutSmallestInterval ----------------------------------------------------------

/// Removes the shortest interval of `cc` by squeezing it to zero length
/// (`sm_SqueezeOutSmallestInterval`): the first shortest interval loses the end
/// [`CurveClassification::find_best_squeeze_end`] picks (`ConnectIntervals(.., FALSE)`).
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for a classification without intervals (upstream reads
/// `m_pData[0]`); those of [`CurveClassification::connect_intervals`].
pub fn squeeze_out_smallest_interval<O: ClassObject>(
    cc: &mut CurveClassification<O>,
) -> Result<()> {
    // find the shortest classification interval
    let mut smallest = 0;
    for j in 1..cc.size() {
        let len1 = cc.at(smallest)?.interval.length();
        let len2 = cc.at(j)?.interval.length();
        if len2 < len1 {
            smallest = j;
        }
    }

    // pick end of interval to remove from classification
    if cc.size() == 0 {
        return Err(SolidError::invalid_input(
            OP_SQUEEZE,
            "classification without intervals",
        ));
    }
    let squeeze_pos = cc.find_best_squeeze_end(smallest)?;

    // remove smallest interval target endBoundary (SER)
    cc.connect_intervals(smallest, squeeze_pos, false)?;

    // all done
    Ok(())
}

impl<O: ClassObject> CurveClassification<O> {
    // ---- constructor ------------------------------------------------------------------

    /// A classification of `init.curve` over `init.interval` (the full
    /// `SmCurveClassification` constructor): the members as [`CurveClassification::new`],
    /// then `ReSet` with the arguments (its status is not checked, as upstream).
    ///
    /// # Errors
    ///
    /// None in practice: `ReSet`'s status is ignored and only [`SolidError::Unported`]
    /// would propagate.
    pub fn with_curve(k: &Kernel, init: &ClassificationInit<O>) -> Result<Self> {
        let mut cc = Self::new(k);

        // pass the call along
        ignore_status(reset(k, &mut cc, init))?;
        Ok(cc)
    }

    // ---- GetIvlBound_EdgeuseObj -------------------------------------------------------

    /// The edge use of the face that bounds interval `index` at its start (`do_min`) or
    /// end (`SmCurveInterval::GetIvlBound_EdgeuseObj`): of the one or two upward edge uses
    /// of the end point's edge on the classification's face, the one whose orientation
    /// matches the sign of the triple product (surface normal, edge tangent, curve
    /// tangent); `None` when the end is not on an edge or no edge use matches.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for `index >= size()`, a classification without a
    /// face (upstream calls `GetUpwardFaceuse` on `NULL`, UB-124) and a face, edge or edge
    /// use handle `geometry` does not resolve; [`SolidError::Unported`] from `GetMinUV` /
    /// `GetMaxUV` (whose status is otherwise ignored) for a face the adapter does not
    /// resolve to a B-rep.
    pub fn ivl_bound_edgeuse_obj<G: ClassGeometry<O>>(
        &self,
        k: &Kernel,
        geometry: &G,
        index: usize,
        do_min: bool,
    ) -> Result<Option<O>> {
        // locals
        let ivl = self.at(index)?;
        let tgt_pt_classify = if do_min { &ivl.start } else { &ivl.end };
        let classify_crv = self.curve();
        let tgt_edge_obj = tgt_pt_classify.edge_object();
        let classify_face = self.face();
        // pClassifyFace->GetUpwardFaceuse() / GetSurface() before the NULL check: UB-124
        let face = classify_face.ok_or_else(|| {
            SolidError::invalid_input(OP_BOUND, "classification without a face (UB-124)")
        })?;
        let upper_faceuse = face_upward_faceuse(geometry, face, OP_BOUND)?;
        let classify_surface = face_surface(geometry, face, OP_BOUND)?;

        // no work - not bounded by an Edge on a Face with a Surface
        let (Some(tgt_edge_obj), Some(classify_surface)) = (tgt_edge_obj, classify_surface) else {
            return Ok(None);
        };

        // Get TgtEdgeObj Edgeuses connected to CrvClassification->Face->UpperFaceuse
        let edgeuses = edge_edgeuses(geometry, tgt_edge_obj, upper_faceuse, OP_BOUND)?;

        // ObjCrv Pt/1stDer at ObjCrv/ClassifyCrv XSect
        let tgt_object_crv = tgt_pt_classify
            .curve_object(geometry)?
            .ok_or_else(|| SolidError::null_pointer(OP_BOUND))?;
        let tgt_edge_pn = eval_pn(k, tgt_object_crv, tgt_pt_classify.t_param())?;

        // ClassifyCrv Pt/1stDer at ObjCrv/ClassifyCrv XSect
        let tgt_classify_crv_param = if do_min {
            self.interval().min()
        } else {
            self.interval().max()
        };
        let classify_crv = classify_crv
            .and_then(|h| geometry.curve(h))
            .ok_or_else(|| SolidError::null_pointer(OP_BOUND))?;
        let classify_crv_pn = eval_pn(k, classify_crv, tgt_classify_crv_param)?;

        // CrvClassification Face locals (statuses ignored)
        let tgt_classify_surf_uv =
            ignored(interval_end_uv(k, geometry, self, index, do_min))?.unwrap_or(Vec2::UNDEF);
        let tgt_classify_surf_norm = ignored(classify_surface.evaluate_normal(
            k,
            tgt_classify_surf_uv,
            Side::Left,
            Side::Left,
        ))?
        .unwrap_or(Vec3::UNDEF);

        // Find the Edgeuse direction that bounds the ClassifyCrv
        let triple = tgt_classify_surf_norm.triple_product(tgt_edge_pn[1], classify_crv_pn[1]);
        let tgt_orient = if triple > 0.0 {
            if do_min {
                OrientType::Same
            } else {
                OrientType::Opposite
            }
        } else if do_min {
            OrientType::Opposite
        } else {
            OrientType::Same
        };

        // get IvlBounding Edgeuse from desired Edgeuse orientation
        if let Some(&e0) = edgeuses.first() {
            if edgeuse_orientation(geometry, e0, OP_BOUND)? == tgt_orient {
                return Ok(Some(e0));
            }
        }
        if let Some(&e1) = edgeuses.get(1) {
            if edgeuse_orientation(geometry, e1, OP_BOUND)? == tgt_orient {
                return Ok(Some(e1));
            }
        }
        Ok(None)
    }

    // ---- GetLoopCrossings -------------------------------------------------------------

    /// The point classification `m_pPtClassify` of `touch` refers to.
    fn touch_point_class(&self, touch: &TouchData<O>) -> Result<&PointClassification<O>> {
        match usize::try_from(touch.pt_classify_index).ok() {
            Some(0) => Ok(&self.at(0)?.start),
            Some(i) => Ok(&self.at(i - 1)?.end),
            None => Err(SolidError::invalid_input(
                OP_CROSS,
                "touch index past the intervals",
            )),
        }
    }

    /// The ordered encounters of the classification curve with `loops`
    /// (`GetLoopTouches`): one arrive/depart pair per point classification on a loop,
    /// tangent, coincident, vertex-loop, pole and seam encounters included. A closed
    /// curve's two seam ends stay distinct records at their own parameters; an end on a
    /// seam side no edge use bounds is [`CrvDirOnFaceType::SeamNoEdgeuse`]. Which
    /// encounters are crossings is the consumer's decision
    /// ([`CurveClassification::get_loop_crossings`]).
    ///
    /// `face_props` are the face's properties when already at stage 2
    /// ([`PropStage::Stage2A`] or later); otherwise `SetProps_Stage2` computes them (into
    /// `face_props` when given).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for a face without surface or a classification without
    /// curve (upstream dereferences them before its `NULL` checks, UB-124), a loop without
    /// face when stage 2 is computed (UB-131) and a topology handle `geometry` does not
    /// resolve, a null pointer `SmEdgeuse::EvaluateBinormal` dereferences, and those of
    /// `SetProps_Stage2` and [`CurveClassification::point_classifies_connected_to`]; the
    /// status of `EvaluateBinormal` itself is ignored as upstream; `SM_ERR` and the
    /// evaluation and projection errors of the seam-side classification of the ends.
    pub fn get_loop_touches<G: ClassGeometry<O>>(
        &self,
        k: &Kernel,
        geometry: &G,
        loops: &[O],
        face_props: Option<&mut FaceProps<O>>,
    ) -> Result<Vec<TouchData<O>>>
    where
        O: LoopObject,
    {
        let mut local_props = FaceProps::<O>::new();
        let props: &mut FaceProps<O> = match face_props {
            Some(p) => p,
            None => &mut local_props,
        };
        match self.loop_touches_with_props(k, geometry, loops, props)? {
            (_, Some(e)) => Err(e),
            (raw, None) => Ok(raw),
        }
    }

    /// [`CurveClassification::get_loop_touches`] over mutable classification objects:
    /// the `SetProps_Stage2` it may run edits the face's B-rep in place
    /// ([`FaceProps::set_props_stage2_mut`] on the B-rep of the first loop's face).
    ///
    /// # Errors
    ///
    /// As [`CurveClassification::get_loop_touches`]; [`SolidError::InvalidInput`] for a
    /// face handle `geometry` cannot resolve mutably when stage 2 is computed.
    pub fn get_loop_touches_mut<G: ClassGeometryMut<O>>(
        &self,
        k: &Kernel,
        geometry: &mut G,
        loops: &[O],
        face_props: Option<&mut FaceProps<O>>,
    ) -> Result<Vec<TouchData<O>>>
    where
        O: LoopObject,
    {
        let mut local_props = FaceProps::<O>::new();
        let props: &mut FaceProps<O> = match face_props {
            Some(p) => p,
            None => &mut local_props,
        };
        match self.loop_touches_with_props_mut(k, geometry, loops, props)? {
            (_, Some(e)) => Err(e),
            (raw, None) => Ok(raw),
        }
    }

    /// `GetLoopTouches` on the caller's face props.
    fn loop_touches_with_props<G: ClassGeometry<O>>(
        &self,
        k: &Kernel,
        geometry: &G,
        loops: &[O],
        props: &mut FaceProps<O>,
    ) -> Result<(Vec<TouchData<O>>, Option<SolidError>)>
    where
        O: LoopObject,
    {
        // no work - no TgtLoops
        let Some(&loop0) = loops.first() else {
            return Ok((Vec::new(), None));
        };

        // must have Face SM_PROPSTAGE_2 Props - when pOptFaceProps is not given
        if props.prop_stage < PropStage::Stage2A {
            let face = loop_face(geometry, loop0)?;
            let face_index = if props.face_index == UNDEF_ULONG {
                0
            } else {
                props.face_index
            };
            set_props_stage_2(k, geometry, props, face, face_index)?;
        }
        self.loop_touches_at_stage_2(k, geometry, loops, props, loop0)
    }

    /// [`Self::loop_touches_with_props`] with `SetProps_Stage2` editing the B-rep in place.
    fn loop_touches_with_props_mut<G: ClassGeometryMut<O>>(
        &self,
        k: &Kernel,
        geometry: &mut G,
        loops: &[O],
        props: &mut FaceProps<O>,
    ) -> Result<(Vec<TouchData<O>>, Option<SolidError>)>
    where
        O: LoopObject,
    {
        // no work - no TgtLoops
        let Some(&loop0) = loops.first() else {
            return Ok((Vec::new(), None));
        };

        // must have Face SM_PROPSTAGE_2 Props - when pOptFaceProps is not given
        if props.prop_stage < PropStage::Stage2A {
            let face = loop_face(&*geometry, loop0)?;
            let face_index = if props.face_index == UNDEF_ULONG {
                0
            } else {
                props.face_index
            };
            set_props_stage_2_mut(k, geometry, props, face, face_index)?;
        }
        self.loop_touches_at_stage_2(k, &*geometry, loops, props, loop0)
    }

    /// The number of times the classification curve crosses each of `loops` in UV space
    /// (`GetLoopCrossings`), from the encounters of
    /// [`CurveClassification::get_loop_touches`] (returned whole as the raw touches): a
    /// closed curve's last arrival and first departure are joined into one encounter
    /// (with a coincident run continuing through that join), then a copy is filtered to
    /// transverse crossings ([`remove_non_crossings`]). An open curve ending on a seam is
    /// not joined (upstream only warns that its count may be unreliable).
    ///
    /// `face_props` are the face's properties when already at stage 2
    /// ([`PropStage::Stage2A`] or later); otherwise `SetProps_Stage2` computes them (into
    /// `face_props` when given).
    ///
    /// # Errors
    ///
    /// Those of [`CurveClassification::get_loop_touches`]; `SM_ERR_ASSERT_FAILURE` for a
    /// closed curve whose seam end pair is incomplete (`AERN`); those of
    /// [`remove_non_crossings`].
    pub fn get_loop_crossings<G: ClassGeometry<O>>(
        &self,
        k: &Kernel,
        geometry: &G,
        loops: &[O],
        face_props: Option<&mut FaceProps<O>>,
    ) -> Result<LoopCrossings<O>>
    where
        O: LoopObject,
    {
        let mut local_props = FaceProps::<O>::new();
        let props: &mut FaceProps<O> = match face_props {
            Some(p) => p,
            None => &mut local_props,
        };
        let (raw, status) = self.loop_touches_with_props(k, geometry, loops, props)?;
        self.crossings_from_touches(k, geometry, loops, props, raw, status)
    }

    /// [`CurveClassification::get_loop_crossings`] over mutable classification objects:
    /// the `SetProps_Stage2` it may run edits the face's B-rep in place
    /// ([`FaceProps::set_props_stage2_mut`] on the B-rep of the first loop's face), so
    /// the UV trim curves, the tolerance growth and the gap caches of its seam check stay
    /// there, as upstream's `GetLoopCrossings` leaves them.
    ///
    /// # Errors
    ///
    /// As [`CurveClassification::get_loop_crossings`]; [`SolidError::InvalidInput`] for
    /// a face handle `geometry` cannot resolve mutably when stage 2 is computed.
    pub fn get_loop_crossings_mut<G: ClassGeometryMut<O>>(
        &self,
        k: &Kernel,
        geometry: &mut G,
        loops: &[O],
        face_props: Option<&mut FaceProps<O>>,
    ) -> Result<LoopCrossings<O>>
    where
        O: LoopObject,
    {
        let mut local_props = FaceProps::<O>::new();
        let props: &mut FaceProps<O> = match face_props {
            Some(p) => p,
            None => &mut local_props,
        };
        let (raw, status) = self.loop_touches_with_props_mut(k, geometry, loops, props)?;
        self.crossings_from_touches(k, &*geometry, loops, props, raw, status)
    }

    /// The crossing part of `GetLoopCrossings` on the encounters `raw` of
    /// `GetLoopTouches` (and the failing status it returned with them).
    fn crossings_from_touches<G: ClassGeometry<O>>(
        &self,
        k: &Kernel,
        geometry: &G,
        loops: &[O],
        props: &FaceProps<O>,
        raw: Vec<TouchData<O>>,
        touches_status: Option<SolidError>,
    ) -> Result<LoopCrossings<O>>
    where
        O: LoopObject,
    {
        let mut out = LoopCrossings {
            crossing_counts: vec![0; loops.len()],
            raw_touches: raw,
            done_touches: Vec::new(),
            sm_err: false,
            status: touches_status,
        };
        let mut count = out.raw_touches.len();
        if out.status.is_some() || count == 0 {
            return Ok(out);
        }
        let mut crossings = out.raw_touches.clone();

        let is_seam = |d: CrvDirOnFaceType| {
            d == CrvDirOnFaceType::Seam || d == CrvDirOnFaceType::SeamNoEdgeuse
        };
        if is_seam(touch_at(&out.raw_touches, 0)?.loop_dir)
            || is_seam(touch_at(&out.raw_touches, count - 1)?.loop_dir)
        {
            let xsect_tol = xsect_tol_3d_from_zones(k, props.zone_tol_3d, props.zone_tol_3d);
            // SmTol::IsClosed(GetCurve(), &tol): a NULL curve is not closed
            let closed = match self.curve() {
                Some(h) => {
                    let curve = geometry
                        .curve(h)
                        .ok_or_else(|| SolidError::null_pointer(OP_CROSS))?;
                    curve.is_closed(k, &curve.natural_interval(), xsect_tol)?
                }
                None => false,
            };
            if closed {
                if !(count >= 4 && is_seam(touch_at(&out.raw_touches, count - 1)?.loop_dir)) {
                    out.done_touches = crossings;
                    out.status = Some(SolidError::Upstream {
                        operation: OP_CROSS,
                        code: SM_ERR_ASSERT_FAILURE,
                        entity: None,
                    });
                    return Ok(out);
                }
                // Join the last arrival and first departure; the pair may not be a
                // crossing. Their original records remain in the raw touches.
                set_touch(&mut crossings, 0, touch_at(&out.raw_touches, count - 2)?)?;
                crossings.truncate(count - 2);
                count = crossings.len();
                // A coincident interval can continue through the periodic join.
                if touch_at(&crossings, 0)?.loop_dir == CrvDirOnFaceType::On
                    && touch_at(&crossings, count - 1)?.loop_dir == CrvDirOnFaceType::On
                {
                    let t = touch_at(&crossings, count - 2)?;
                    set_touch(&mut crossings, 0, t)?;
                    crossings.truncate(count - 2);
                }
            }
            // else: an open sample ending on a seam; its crossing count may be
            // unreliable (upstream only warns)
        }
        match remove_non_crossings(&mut crossings) {
            Ok(()) => {}
            Err(e) if is_status(&e) => {
                out.done_touches = crossings;
                out.status = Some(e);
                return Ok(out);
            }
            Err(e) => return Err(e),
        }
        out.crossing_counts = loops
            .iter()
            .map(|&lp| get_connected_touches(Some(lp), &crossings).len() as u64 / 2)
            .collect();
        out.done_touches = crossings;
        Ok(out)
    }

    /// The rest of `GetLoopTouches` once the face props are at stage 2.
    #[allow(clippy::too_many_lines)] // one upstream routine
    fn loop_touches_at_stage_2<G: ClassGeometry<O>>(
        &self,
        k: &Kernel,
        geometry: &G,
        loops: &[O],
        props: &mut FaceProps<O>,
        loop0: O,
    ) -> Result<(Vec<TouchData<O>>, Option<SolidError>)>
    where
        O: LoopObject,
    {
        // Tgt Face and Surface Props
        let face = loop_face(geometry, loop0)?;
        let face_xsect_tol_3d = xsect_tol_3d_from_zones(k, props.zone_tol_3d, props.zone_tol_3d);
        let upper_faceuse = match face {
            Some(f) => face_upward_faceuse(geometry, f, OP_CROSS)?,
            None => None,
        };
        let surface = match face {
            Some(f) => face_surface(geometry, f, OP_CROSS)?,
            None => None,
        };
        // pSurface->GetNaturalUVDomain() before the NULL check: UB-124
        let surface = surface.ok_or_else(|| {
            SolidError::invalid_input(OP_CROSS, "face without a surface (UB-124)")
        })?;
        let srf_domain = surface.natural_uv_domain_with_nurb(k)?;
        let srf_min_uv = srf_domain.min();
        let srf_max_uv = srf_domain.max();
        let crv_classification_interval = self.interval();
        let crv_classification_ivl_cnt = self.size();

        // ClassifyCrv Props: pClassifyCrv->GetNaturalInterval() before the NULL check: UB-124
        let classify_crv_handle = self.curve();
        let classify_crv = classify_crv_handle
            .and_then(|h| geometry.curve(h))
            .ok_or_else(|| {
                SolidError::invalid_input(OP_CROSS, "classification without a curve (UB-124)")
            })?;
        let classify_crv_ivl = classify_crv.natural_interval();
        // SmTol::IsClosed(pClassifyCrv, &tol): GetZoneTol3d(pObj) is evaluated first
        let _ = object_zone_tol_3d(k, geometry, classify_crv_handle)?;
        let classify_crv_closed_3d =
            classify_crv.is_closed(k, &classify_crv_ivl, face_xsect_tol_3d)?;
        let classify_crv_degen =
            curve_is_degenerate(k, Some(classify_crv), &classify_crv_ivl, face_xsect_tol_3d)?;
        let mut classify_crv_on_seam = SurfParamType::Neither;

        // check state - degen geometry - return no touches
        if classify_crv_degen {
            return Ok((Vec::new(), None));
        }

        // ClassifyCrv 3d Start/Stop
        let classify_crv_start_pn = eval_pn(k, classify_crv, crv_classification_interval.min())?;
        let classify_crv_stop_pn = eval_pn(k, classify_crv, crv_classification_interval.max())?;

        // When working with a periodic surface - set eClassifyCrv_OnSeam
        if props.closed_u != Tri::False || props.closed_v != Tri::False {
            let smp_cnt: u32 = 3;
            let mut on_seam = true;
            // sSmpPt / sSmpPtUV live across the samples: a failed call leaves the last value
            let mut smp_pt = Vec3::UNDEF;
            let mut smp_uv = Vec2::UNDEF;
            for ii in 1..=smp_cnt {
                // From SmpParam => SmpPtUV
                let smp_param = classify_crv_ivl.evaluate(f64::from(ii) / f64::from(smp_cnt + 1));
                smp_pt = ignored(classify_crv.evaluate_point(k, smp_param))?.unwrap_or(smp_pt);
                smp_uv = drop_uv(
                    k,
                    surface,
                    smp_pt,
                    &surface.natural_uv_domain_with_nurb(k)?,
                    None,
                    None,
                    smp_uv,
                )?;

                // Classify SmpPtUV against Seams
                let seam = surface.is_on_seam(k, smp_uv, None)?;
                on_seam &= seam.on_seam;
                if !on_seam {
                    classify_crv_on_seam = SurfParamType::Neither;
                    break;
                } else if ii == 1 {
                    classify_crv_on_seam = seam.seam_dir;
                }
            }
        }

        // ---- Build Raw Touch List
        let mut raw = self.point_classifies_connected_to(geometry, loops)?;

        // no work - ClassifyCrv does not touch any TgtLoop
        if raw.is_empty() {
            return Ok((raw, None));
        }

        // Double RawTouchDirList Size (two entries per Touch:[ArrivalDir DepartureDir])
        raw = raw.iter().flat_map(|t| [*t, *t]).collect();
        let touch_cnt = raw.len();
        let first_touch = touch_at(&raw, 0)?;
        let last_touch = touch_at(&raw, touch_cnt - 1)?;

        // 1st/Last RawTouchDir 3dPoints
        let touch_1st_pn = eval_pn(k, classify_crv, first_touch.param)?;
        let touch_last_pn = eval_pn(k, classify_crv, last_touch.param)?;

        // when Surface has poles - remember if TouchList first/last is on Pole
        let mut touch_1st_on_pole = SS_NONE;
        let mut touch_last_on_pole = SS_NONE;
        if props.poles != u64::from(SS_NONE) {
            touch_1st_on_pole = on_pole_mask(props, touch_1st_pn[0]);
            touch_last_on_pole = on_pole_mask(props, touch_last_pn[0]);
        }

        // get 1st/Last UV Pos and Dir for upcoming MapTo2d() calls
        let pole_guess = |on_pole: u32| -> Point2 {
            if on_pole & SS_UMIN != 0 || on_pole & SS_VMIN != 0 {
                srf_min_uv
            } else {
                srf_max_uv
            }
        };
        let first_guess = if touch_1st_on_pole == SS_NONE {
            srf_domain.mid()
        } else {
            pole_guess(touch_1st_on_pole)
        };
        let last_guess = if touch_last_on_pole == SS_NONE {
            srf_domain.mid()
        } else {
            pole_guess(touch_last_on_pole)
        };
        let touch_1st_uv0 = drop_uv(
            k,
            surface,
            touch_1st_pn[0],
            &srf_domain,
            Some(first_guess),
            Some(touch_1st_pn[1]),
            first_guess,
        )?;
        let touch_1st_uv1 = drop_vector(k, surface, touch_1st_uv0, touch_1st_pn[1])?;
        let neg_last_pn = -touch_last_pn[1];
        let touch_last_uv0 = drop_uv(
            k,
            surface,
            touch_last_pn[0],
            &srf_domain,
            Some(last_guess),
            Some(neg_last_pn),
            last_guess,
        )?;
        let touch_last_uv1 = drop_vector(k, surface, touch_last_uv0, touch_last_pn[1])?;

        // 2d Tols at RawTouchDirList Start/Stop
        // (on a pole: Min(TolU, TolV); off a pole: the directional tolerance)
        let first_zone_tol_2d = if touch_1st_on_pole == SS_NONE {
            map_to_2d_dir(k, props.zone_tol_3d, touch_1st_uv0, touch_1st_uv1, surface)?
        } else {
            map_to_2d(k, props.zone_tol_3d, touch_1st_uv0, surface)?
        };
        let last_zone_tol_2d = if touch_last_on_pole == SS_NONE {
            map_to_2d_dir(
                k,
                props.zone_tol_3d,
                touch_last_uv0,
                touch_last_uv1,
                surface,
            )?
        } else {
            map_to_2d(k, props.zone_tol_3d, touch_last_uv0, surface)?
        };

        // remember when first/last touch is on ClassifyCrv Start/Stop
        let mut touch_1st_type =
            crv_classification_interval.classify_point(first_touch.param, first_zone_tol_2d);
        let mut touch_last_type =
            crv_classification_interval.classify_point(last_touch.param, last_zone_tol_2d);

        // check state - stored params must be on or inside the interval
        if touch_1st_type == ExtentPointType::Outside || touch_last_type == ExtentPointType::Outside
        {
            // Snap offending val to a boundary value
            let mut last = touch_at(&raw, touch_cnt - 1)?;
            last.param = crv_classification_interval.clamp_value(last.param);
            set_touch(&mut raw, touch_cnt - 1, last)?;

            if touch_1st_type == ExtentPointType::Outside {
                let mut first = touch_at(&raw, 0)?;
                first.param = crv_classification_interval.clamp_value(first.param);
                set_touch(&mut raw, 0, first)?;
                touch_1st_type =
                    crv_classification_interval.classify_point(first.param, eff_zero());
            }
            if touch_last_type == ExtentPointType::Outside {
                let mut last = touch_at(&raw, touch_cnt - 1)?;
                last.param = crv_classification_interval.clamp_value(last.param);
                set_touch(&mut raw, touch_cnt - 1, last)?;
                touch_last_type =
                    crv_classification_interval.classify_point(last.param, eff_zero());
            }
        }

        // UVSpace degenerate ClassifyCrvs do not cross loops
        if touch_1st_type == ExtentPointType::Both || touch_last_type == ExtentPointType::Both {
            // pRawTouchDirList->ReSet()
            return Ok((Vec::new(), None));
        }

        // when there is a touch on the ClassifyCrv Ends at Surface NaturalBoundaries
        let is_end = |t: ExtentPointType| t == ExtentPointType::Start || t == ExtentPointType::End;
        if is_end(touch_1st_type) || is_end(touch_last_type) {
            let touch_1st_on_no_vertex_pole = u64::from(touch_1st_on_pole) & props.no_vertex_poles;
            let touch_last_on_no_vertex_pole =
                u64::from(touch_last_on_pole) & props.no_vertex_poles;
            let mut touch_1st_on_seam = (false, false);
            let mut touch_last_on_seam = (false, false);

            // when Surface has Seams and ClassifyCrv Closed3d (unset tri-states end FALSE)
            if classify_crv_closed_3d
                && (props.closed_u != Tri::False || props.closed_v != Tri::False)
            {
                touch_1st_on_seam = end_on_seams(
                    k,
                    surface,
                    props,
                    touch_1st_on_pole,
                    classify_crv_on_seam,
                    &classify_crv_start_pn,
                    srf_min_uv,
                    srf_max_uv,
                    true,
                )?;
                touch_last_on_seam = end_on_seams(
                    k,
                    surface,
                    props,
                    touch_last_on_pole,
                    classify_crv_on_seam,
                    &classify_crv_stop_pn,
                    srf_min_uv,
                    srf_max_uv,
                    false,
                )?;
            }

            // just one raw touch with 2 TouchDirs per Touch
            let one_touch_tgt_type = if touch_cnt == 2 {
                touch_1st_type
            } else {
                ExtentPointType::Both
            };

            // When needed Set the Start RawTouchDirList DirVal
            if is_end(touch_1st_type)
                && (one_touch_tgt_type == ExtentPointType::Start
                    || one_touch_tgt_type == ExtentPointType::Both)
            {
                let mut t = touch_at(&raw, 0)?;
                t.loop_dir = boundary_dir(
                    touch_1st_on_no_vertex_pole,
                    touch_1st_on_pole,
                    touch_1st_on_seam.0 || touch_1st_on_seam.1,
                );
                set_touch(&mut raw, 0, t)?;
            }

            // When needed Set the Stop RawTouchDirList DirVal
            if is_end(touch_last_type)
                && (one_touch_tgt_type == ExtentPointType::End
                    || one_touch_tgt_type == ExtentPointType::Both)
            {
                let mut t = touch_at(&raw, touch_cnt - 1)?;
                t.loop_dir = boundary_dir(
                    touch_last_on_no_vertex_pole,
                    touch_last_on_pole,
                    touch_last_on_seam.0 || touch_last_on_seam.1,
                );
                set_touch(&mut raw, touch_cnt - 1, t)?;
            }
        }

        // for every ClassifyCrv/rLoop Touch - calculate eArriveTypes and eDepartTypes
        let ctx = TouchContext {
            k,
            geometry,
            surface,
            face,
            upper_faceuse,
            classify_crv,
            crv_classification_interval,
            ivl_cnt: crv_classification_ivl_cnt,
        };
        let mut i1 = 1;
        while i1 < raw.len() {
            let i0 = i1 - 1;
            let arrive_touch = touch_at(&raw, i0)?;
            let depart_touch = touch_at(&raw, i1)?;
            let (arrive, depart) = self.classify_touch(
                k,
                &ctx,
                &arrive_touch,
                arrive_touch.loop_dir,
                depart_touch.loop_dir,
            )?;
            let mut a = arrive_touch;
            a.loop_dir = arrive;
            set_touch(&mut raw, i0, a)?;
            let mut d = depart_touch;
            d.loop_dir = depart;
            set_touch(&mut raw, i1, d)?;
            i1 += 2;
        }

        // If the first or last touch is on a seam, classify whether an edge use bounds
        // that periodic side
        if !raw.is_empty() {
            let last = raw.len() - 1;
            for (index, first) in [(0, true), (last, false)] {
                let mut touch = touch_at(&raw, index)?;
                match self.classify_seam_edgeuse(k, geometry, &mut touch, first, face, props) {
                    Ok(()) => set_touch(&mut raw, index, touch)?,
                    // SER: the touches stay as filled so far
                    Err(e) if is_status(&e) => return Ok((raw, Some(e))),
                    Err(e) => return Err(e),
                }
            }
        }
        Ok((raw, None))
    }

    /// `sm_ClassifySeamEdgeuse` (static): an end touch of the classification curve that
    /// leaves the surface over a seam ([`CrvDirOnFaceType::Seam`]) becomes
    /// [`CrvDirOnFaceType::SeamNoEdgeuse`] when no edge use connected to its loop bounds
    /// the periodic side the curve's end is on (an unused periodic image; the point
    /// classification and the loop stay). The end's UV comes from the curve's own UV
    /// branch for a curve on the surface, else from a drop steered by the inward tangent;
    /// the edge uses are those of the touch's edge, or those around its vertex, on the
    /// face's upward face use; the side each occupies comes from
    /// [`find_edgeuse_seam_sides`], or from the directed edge use for a loop wholly on the
    /// seam. No face containment or UV trim is used.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a drop that fails or an end UV not on a seam; those of the
    /// evaluations, drops and topology queries (`SER`); a `NULL` the upstream code
    /// dereferences.
    ///
    /// The face is that of `props` (`m_pFace`), resolved in the B-rep of
    /// `first_loop_face`.
    #[allow(clippy::too_many_lines)] // one upstream routine
    fn classify_seam_edgeuse<G: ClassGeometry<O>>(
        &self,
        k: &Kernel,
        geometry: &G,
        touch: &mut TouchData<O>,
        first: bool,
        first_loop_face: Option<O>,
        props: &FaceProps<O>,
    ) -> Result<()>
    where
        O: LoopObject,
    {
        if touch.loop_dir != CrvDirOnFaceType::Seam {
            return Ok(());
        }
        let first_loop_face = first_loop_face.ok_or_else(|| SolidError::null_pointer(OP_CROSS))?;
        let (b, f) = face_id(geometry, first_loop_face, OP_CROSS)?;
        let pf = props
            .face
            .ok_or_else(|| SolidError::null_pointer(OP_CROSS))?;
        let face = if pf == f {
            first_loop_face
        } else {
            handle_of(geometry, b, EntityRef::Face(pf), OP_CROSS)?
        };
        let surface = face_surface(geometry, face, OP_CROSS)?
            .ok_or_else(|| SolidError::null_pointer(OP_CROSS))?;
        let sample = self
            .curve()
            .and_then(|h| geometry.curve(h))
            .ok_or_else(|| SolidError::null_pointer(OP_CROSS))?;
        let domain = surface.natural_uv_domain_with_nurb(k)?;
        let pn = sample.evaluate(k, touch.param, 1, true, true)?;
        let (Some(&sample_pt), Some(&sample_tan)) = (pn.first(), pn.get(1)) else {
            return Err(SolidError::sm_err(OP_CROSS));
        };
        let uv = if matches!(sample.kind(), CurveKind::CrvOnSurf(_)) {
            if first {
                interval_end_uv(k, geometry, self, 0, true)?
            } else {
                let last = self.size().checked_sub(1).ok_or_else(|| {
                    SolidError::invalid_input(OP_CROSS, "classification without intervals")
                })?;
                interval_end_uv(k, geometry, self, last, false)?
            }
        } else {
            // A model-space sample has no defining UV branch. Its inward tangent
            // selects the image at this end; an unconstrained drop may choose either.
            let inward = if first { sample_tan } else { -sample_tan };
            let opts = DropPointOptions {
                guess_uv: None,
                operation: SolverOperation::Minimize,
                tgt_seam_side: Some(inward),
                want_solutions: false,
            };
            let drop = surface.drop_point::<SurfHandle>(k, sample_pt, &domain, &opts)?;
            if !drop.success {
                return Err(SolidError::sm_err(OP_CROSS));
            }
            drop.result_uv
        };
        let direction = drop_vectors_at(k, surface, uv, Side::Left, Side::Left, &[sample_tan])?
            .first()
            .copied()
            .ok_or_else(|| SolidError::sm_err(OP_CROSS))?;
        let seam_info = surface.is_on_seam(k, uv, None)?;
        if !seam_info.on_seam {
            return Err(SolidError::sm_err(OP_CROSS));
        }
        let mut seam = seam_info.seam_dir;
        if seam == SurfParamType::Both {
            let du = direction.x.abs() / domain.u_interval().length();
            let dv = direction.y.abs() / domain.v_interval().length();
            // A sample along one seam leaves only the other. A diagonal corner
            // encounter must match both periodic sides on the same edgeuse.
            if du > 0.0 && dv <= EFF_ZERO * du {
                seam = SurfParamType::U;
            } else if dv > 0.0 && du <= EFF_ZERO * dv {
                seam = SurfParamType::V;
            }
        }

        // the edge uses of the touch's edge, or around its vertex, with the encounter's
        // parameter on each
        let pc = self.touch_point_class(touch)?;
        let upward = face_upward_faceuse(geometry, face, OP_CROSS)?;
        let mut uses: Vec<(EdgeuseId, f64)> = Vec::new();
        if let Some(edge) = pc.edge_object() {
            for h in edge_edgeuses(geometry, edge, upward, OP_CROSS)? {
                let (_, eu) = edgeuse_id(geometry, h, OP_CROSS)?;
                uses.push((eu, pc.t_param()));
            }
        } else if let Some(vertex) = pc.vertex_object() {
            for h in vertex_vertexuses_of_face(geometry, vertex, Some(face), upward)? {
                let (_, vu) = vertexuse_id(geometry, h, OP_CROSS)?;
                if topo_vertexuse::vertexuse_type(b, vu)? != topo_vertexuse::EDGEUSE_TYPE {
                    continue;
                }
                let eu = topo_vertexuse::edgeuse(b, vu)?;
                let param = topo_vertexuse::compute_t_point(b, vu)?;
                let eu = eu.ok_or_else(|| SolidError::null_pointer(OP_CROSS))?;
                uses.push((eu, param));
                let cw =
                    eu1::cw_edgeuse(b, eu)?.ok_or_else(|| SolidError::null_pointer(OP_CROSS))?;
                let mate = eu1::mate(b, cw)?.ok_or_else(|| SolidError::null_pointer(OP_CROSS))?;
                let mate_vu =
                    eu1::vertexuse(b, mate)?.ok_or_else(|| SolidError::null_pointer(OP_CROSS))?;
                uses.push((cw, topo_vertexuse::compute_t_point(b, mate_vu)?));
            }
        }
        let tolerance = xsect_tol_3d_from_zones(k, props.zone_tol_3d, props.zone_tol_3d);
        let connected = connected_entity(geometry, touch.connected_to, OP_CROSS)?;
        for (eu, param) in uses {
            if !eu1::is_connected_to(b, eu, connected)? {
                continue;
            }
            let mut bounds = true;
            for direction_index in 0..2 {
                if !bounds {
                    break;
                }
                let dir = if direction_index == 0 {
                    SurfParamType::U
                } else {
                    SurfParamType::V
                };
                if seam != SurfParamType::Both && seam != dir {
                    continue;
                }
                let coordinate = if direction_index == 0 { uv.x } else { uv.y };
                let middle = if direction_index == 0 {
                    0.5 * (domain.u_min() + domain.u_max())
                } else {
                    0.5 * (domain.v_min() + domain.v_max())
                };
                let wanted_side = if coordinate < middle { 1u32 } else { 2u32 };
                let sides = find_edgeuse_seam_sides(k, b, eu, param, surface, dir, tolerance)?;
                if sides == 0 {
                    // An entire seam loop has no off-seam anchor; its left side is
                    // defined by the directed edgeuse. Retain tangential encounters.
                    let edge =
                        eu1::edge(b, eu)?.ok_or_else(|| SolidError::null_pointer(OP_CROSS))?;
                    let curve = topo_edge::curve(b, edge)?
                        .ok_or_else(|| SolidError::null_pointer(OP_CROSS))?;
                    let edge_pn = curve.evaluate(k, param, 1, true, true)?;
                    let mut edge_tan = edge_pn
                        .get(1)
                        .copied()
                        .ok_or_else(|| SolidError::sm_err(OP_CROSS))?;
                    let normal = surface.evaluate_normal(k, uv, Side::Left, Side::Left)?;
                    if eu1::orientation(b, eu)? == OrientType::Opposite {
                        edge_tan = -edge_tan;
                    }
                    let side = normal.triple_product(edge_tan, sample_tan)
                        * if first { 1.0 } else { -1.0 };
                    bounds = side >= 0.0;
                } else {
                    bounds = (sides & wanted_side) != 0;
                }
            }
            if bounds {
                return Ok(());
            }
        }
        touch.loop_dir = CrvDirOnFaceType::SeamNoEdgeuse;
        Ok(())
    }

    /// The arrive and depart directions of one raw touch pair (the body of
    /// `GetLoopCrossings`' touch loop; a `continue` returns the directions so far).
    #[allow(clippy::too_many_lines)] // one upstream loop body
    fn classify_touch<G: ClassGeometry<O>>(
        &self,
        k: &Kernel,
        c: &TouchContext<'_, O, G>,
        arrive_touch: &TouchData<O>,
        mut arrive_type: CrvDirOnFaceType,
        mut depart_type: CrvDirOnFaceType,
    ) -> Result<(CrvDirOnFaceType, CrvDirOnFaceType)> {
        let pt_classify = self.touch_point_class(arrive_touch)?;
        let this_edge = pt_classify.edge_object();
        let this_vertex = pt_classify.vertex_object();
        let this_param = arrive_touch.param;

        // ThisTouch->ClassifyCrv(dThisTouch_Param) geometry properties
        let classify_crv_pn = eval_pn(k, c.classify_crv, this_param)?;
        let classify_crv_uv = drop_uv(
            c.k,
            c.surface,
            classify_crv_pn[0],
            &c.surface.natural_uv_domain_with_nurb(k)?,
            None,
            None,
            Vec2::UNDEF,
        )?;
        let classify_crv_uv_1st_dir =
            drop_vector(k, c.surface, classify_crv_uv, classify_crv_pn[1])?;
        // a failed Unitize leaves the vector unchanged
        let classify_crv_uv_tan =
            ignored(classify_crv_uv_1st_dir.unitize())?.unwrap_or(classify_crv_uv_1st_dir);
        let depart_uv_ang_rad = ccw_angle(classify_crv_uv_tan)?;
        let arrive_uv_ang_rad = if depart_uv_ang_rad < PI {
            depart_uv_ang_rad + PI
        } else {
            depart_uv_ang_rad - PI
        };

        // the touch's neighbouring interval interiors
        let next_prev_mid = |s: &Self| -> Result<(PointClassType, PointClassType)> {
            let idx = usize::try_from(arrive_touch.pt_classify_index).map_err(|_| {
                SolidError::invalid_input(OP_CROSS, "touch index past the intervals")
            })?;
            let next = idx.checked_rem(c.ivl_cnt).ok_or_else(|| {
                SolidError::invalid_input(OP_CROSS, "classification without intervals")
            })?;
            let prev = (next + c.ivl_cnt - 1) % c.ivl_cnt;
            Ok((s.at(next)?.mid.point_class(), s.at(prev)?.mid.point_class()))
        };

        if let Some(vertex) = this_vertex {
            // ---- VertexTouch branch
            let mut vertexuses =
                vertex_vertexuses_of_face(c.geometry, vertex, c.face, c.upper_faceuse)?;

            // no work - no Vertexuses connected to Face->TgtLoop
            if vertexuses.is_empty() {
                return Ok((arrive_type, depart_type));
            }

            // Vertex-loop vertexuses are not two-arm face sectors: zero-length cracks
            let mut kept = Vec::with_capacity(vertexuses.len());
            for &vu in &vertexuses {
                if vertexuse_is_loop_vertexuse(c.geometry, vu)? {
                    continue;
                }
                kept.push(vu);
            }
            if kept.is_empty() {
                if arrive_type == CrvDirOnFaceType::Uninit {
                    arrive_type = CrvDirOnFaceType::In;
                }
                if depart_type == CrvDirOnFaceType::Uninit {
                    depart_type = CrvDirOnFaceType::In;
                }
                return Ok((arrive_type, depart_type));
            }
            vertexuses = kept;

            // for every Vertex->Vertexuse - Compute UVSector Props
            let mut sectors = Vec::with_capacity(vertexuses.len());
            let mut sector_ivl_rad = Vec::with_capacity(vertexuses.len());
            for &vu in &vertexuses {
                let sector = vertexuse_compute_uv_sector(c.k, c.geometry, vu)?;
                let beg_rad = ccw_angle(sector.beg_edge_uv_tan)?;
                let end_rad = ccw_angle(sector.end_edge_uv_tan)?;
                let mut ivl = PeriodicExtent1d::full(TWO_PI);
                ignore_status(ivl.set_min_max(beg_rad, end_rad, Some(TWO_PI), false))?;
                sectors.push(sector);
                sector_ivl_rad.push(ivl);
            }

            // for every VertexSector - check arrival/departure SectorIvl-AngRad containment
            for (sector, ivl_rad) in sectors.iter().zip(&sector_ivl_rad) {
                let vertex_uv = sector.beg_edge_uv_point;
                let beg_pn = Self::directional_pn(k, c, vertex_uv, sector.beg_edge_uv_tan)?;
                let end_pn = Self::directional_pn(k, c, vertex_uv, sector.end_edge_uv_tan)?;

                // tangent ClassifyCrv Arrivals/Departures in 3d with SM_ANG_TOL_DEG
                let t1 = classify_crv_pn[1];
                let depart_beg_tan =
                    t1.is_parallel_to(beg_pn[1], ANG_TOL_DEG) && t1.dot(beg_pn[1]) > 0.0;
                let depart_end_tan =
                    t1.is_parallel_to(end_pn[1], ANG_TOL_DEG) && t1.dot(end_pn[1]) > 0.0;
                // -v.IsParallelTo(..) negates the flag, not the vector
                let arrive_beg_tan =
                    t1.is_parallel_to(beg_pn[1], ANG_TOL_DEG) && -t1.dot(beg_pn[1]) > 0.0;
                let arrive_end_tan =
                    t1.is_parallel_to(end_pn[1], ANG_TOL_DEG) && -t1.dot(end_pn[1]) > 0.0;
                let depart_tangent = depart_beg_tan || depart_end_tan;
                let arrive_tangent = arrive_beg_tan || arrive_end_tan;

                // containment within a sector on a singularity
                let mut pole_contains_depart = CrvDirOnFaceType::Uninit;
                let mut pole_contains_arrive = CrvDirOnFaceType::Uninit;
                if sector.vertex_on_pole {
                    let free_param =
                        if are_same(sector.beg_edge_uv_point.x, sector.end_edge_uv_point.x) {
                            SurfParamType::V
                        } else {
                            SurfParamType::U
                        };
                    let system_zone = zone_tol_3d(c.k, SmallTopoType::NormalSize);
                    let depart_uv = ignored(c.surface.find_degen_param_for_direction(
                        k,
                        classify_crv_uv,
                        t1,
                        true,
                        free_param,
                        system_zone,
                    ))?
                    .map_or(classify_crv_uv, |d| d.uv);
                    let arrive_uv = ignored(c.surface.find_degen_param_for_direction(
                        k,
                        classify_crv_uv,
                        -t1,
                        true,
                        free_param,
                        system_zone,
                    ))?
                    .map_or(classify_crv_uv, |d| d.uv);
                    let beg = uv_coord(sector.beg_edge_uv_point, free_param);
                    let end = uv_coord(sector.end_edge_uv_point, free_param);
                    pole_contains_depart = if is_between(uv_coord(depart_uv, free_param), beg, end)
                    {
                        CrvDirOnFaceType::In
                    } else {
                        CrvDirOnFaceType::Out
                    };
                    pole_contains_arrive = if is_between(uv_coord(arrive_uv, free_param), beg, end)
                    {
                        CrvDirOnFaceType::In
                    } else {
                        CrvDirOnFaceType::Out
                    };
                }

                // sector classification of the departure and arrival
                let mut sector_depart = if depart_tangent {
                    CrvDirOnFaceType::On
                } else if pole_contains_depart == CrvDirOnFaceType::In
                    || (!sector.vertex_on_pole && ivl_rad.contains_value(depart_uv_ang_rad))
                {
                    CrvDirOnFaceType::In
                } else {
                    CrvDirOnFaceType::Out
                };
                let mut sector_arrive = if arrive_tangent {
                    CrvDirOnFaceType::On
                } else if pole_contains_arrive == CrvDirOnFaceType::In
                    || (!sector.vertex_on_pole && ivl_rad.contains_value(arrive_uv_ang_rad))
                {
                    CrvDirOnFaceType::In
                } else {
                    CrvDirOnFaceType::Out
                };

                let (next_mid, prev_mid) = next_prev_mid(self)?;

                // ON disambiguation by sampling along the sector edge
                if (sector_depart == CrvDirOnFaceType::On || sector_arrive == CrvDirOnFaceType::On)
                    && depart_type != CrvDirOnFaceType::On
                    && arrive_type != CrvDirOnFaceType::On
                {
                    let depart_coin =
                        sector_depart == CrvDirOnFaceType::On && next_mid == PointClassType::Edge;
                    let arrive_coin =
                        sector_arrive == CrvDirOnFaceType::On && prev_mid == PointClassType::Edge;
                    for kk in 0..2 {
                        if !((kk == 0 && sector_depart == CrvDirOnFaceType::On && !depart_coin)
                            || (kk == 1 && sector_arrive == CrvDirOnFaceType::On && !arrive_coin))
                        {
                            continue;
                        }
                        let use_beg = (kk == 0 && depart_beg_tan) || (kk == 1 && arrive_beg_tan);
                        let (tgt_edgeuse, end_param, end_speed) = if use_beg {
                            (
                                sector.beg_edgeuse,
                                sector.beg_edge_param,
                                sector.beg_edge_uv_tan.length(),
                            )
                        } else {
                            (
                                sector.end_edgeuse,
                                sector.end_edge_param,
                                sector.end_edge_uv_tan.length(),
                            )
                        };
                        let tgt_edgeuse =
                            tgt_edgeuse.ok_or_else(|| SolidError::null_pointer(OP_CROSS))?;
                        let speed_ratio = classify_crv_uv_1st_dir.length() / end_speed;
                        let tgt_ivl =
                            edge_interval(c.geometry, edgeuse_edge(c.geometry, tgt_edgeuse)?)?;
                        let ivl_beg = are_same(end_param, tgt_ivl.min());
                        let mut smp_edgeuse_param = if ivl_beg {
                            tgt_ivl.min()
                        } else {
                            tgt_ivl.max()
                        };
                        let smp_edgeuse_step = if ivl_beg {
                            tgt_ivl.length() / 100.0
                        } else {
                            -tgt_ivl.length() / 100.0
                        };
                        let mut smp_classify_crv_param = this_param;
                        let smp_classify_crv_step = (if kk == 0 { 1.0 } else { -1.0 })
                            * speed_ratio
                            * smp_edgeuse_step.abs();
                        // Upstream declares `sClassifyCrvPN`, `sTgtEdgeusePoint` and
                        // `sTgtEdgeuseBiNorm` (default `UNDEF`) once per `kk` pass, outside
                        // the sampling loop, and ignores the status of both samples. A
                        // failing `Evaluate` / `EvaluateBinormal` therefore leaves the
                        // previous iteration's values (`UNDEF` on the first), except that a
                        // failure after `EvaluateBinormal` initialised its outputs leaves
                        // the point at `(0,0,0)` and the binormal as it was
                        // (`edgeuse_evaluate_binormal`).
                        let mut smp_pn = [Vec3::UNDEF; 2];
                        let mut bn_point = Vec3::UNDEF;
                        let mut bn_binormal = Vec3::UNDEF;
                        for _ in 0..10 {
                            eval_pn_into(k, c.classify_crv, smp_classify_crv_param, &mut smp_pn)?;
                            edgeuse_evaluate_binormal(
                                c.k,
                                c.geometry,
                                tgt_edgeuse,
                                smp_edgeuse_param,
                                &mut bn_point,
                                &mut bn_binormal,
                            )?;
                            let gap = smp_pn[0] - bn_point;
                            if !is_degenerate_vec3(c.k, gap, None) {
                                let dir = if bn_binormal.dot(gap) > 0.0 {
                                    CrvDirOnFaceType::In
                                } else {
                                    CrvDirOnFaceType::Out
                                };
                                if kk == 0 {
                                    sector_depart = dir;
                                } else {
                                    sector_arrive = dir;
                                }
                                break;
                            }
                            smp_edgeuse_param += smp_edgeuse_step;
                            smp_classify_crv_param += smp_classify_crv_step;
                        }
                    }
                }

                // merge this sector's classification
                if arrive_type == CrvDirOnFaceType::Uninit || arrive_type == CrvDirOnFaceType::Out {
                    arrive_type = sector_arrive;
                }
                if depart_type == CrvDirOnFaceType::Uninit || depart_type == CrvDirOnFaceType::Out {
                    depart_type = sector_depart;
                }
            }

            if arrive_type == CrvDirOnFaceType::Uninit {
                arrive_type = CrvDirOnFaceType::Out;
            }
            if depart_type == CrvDirOnFaceType::Uninit {
                depart_type = CrvDirOnFaceType::Out;
            }
        } else if let Some(edge) = this_edge {
            // ---- EdgeTouch branch
            let edge_crv = edge_curve(c.geometry, edge)?;
            let edge_ivl = edge_interval(c.geometry, edge)?;
            let edge_param = pt_classify.t_param();

            // Edge->Edgeuses connected to the face's upper faceuse, Edge Pt/1stDer
            let edgeuses = edge_edgeuses(c.geometry, edge, c.upper_faceuse, OP_CROSS)?;
            let edge_pn = eval_pn(k, edge_crv, edge_param)?;
            let _edge_uv = drop_uv(
                c.k,
                c.surface,
                edge_pn[0],
                &c.surface.natural_uv_domain_with_nurb(k)?,
                None,
                None,
                Vec2::UNDEF,
            )?;

            // tangent ClassifyCrv/Edge
            let are_tangent = edge_pn[1].is_parallel_to(classify_crv_pn[1], ANG_TOL_DEG);

            for &edgeuse in &edgeuses {
                let orient = edgeuse_orientation(c.geometry, edgeuse, OP_CROSS)?;
                let edgeuse_tan = if orient == OrientType::Same {
                    edge_pn[1]
                } else {
                    -edge_pn[1]
                };
                let surf_norm =
                    ignored(
                        c.surface
                            .evaluate_normal(k, classify_crv_uv, Side::Left, Side::Left),
                    )?
                    .unwrap_or(Vec3::UNDEF);
                let cross_mag_3d = classify_crv_pn[1].cross(edgeuse_tan).dot(surf_norm);

                let edgeuse_depart = if is_boundary_dir(depart_type) {
                    depart_type
                } else if are_tangent {
                    CrvDirOnFaceType::On
                } else if cross_mag_3d > 0.0 {
                    CrvDirOnFaceType::Out
                } else {
                    CrvDirOnFaceType::In
                };
                let edgeuse_arrive = if is_boundary_dir(arrive_type) {
                    arrive_type
                } else if are_tangent {
                    CrvDirOnFaceType::On
                } else if -cross_mag_3d > 0.0 {
                    CrvDirOnFaceType::Out
                } else {
                    CrvDirOnFaceType::In
                };

                // merge
                if arrive_type == CrvDirOnFaceType::Uninit || arrive_type == CrvDirOnFaceType::Out {
                    arrive_type = edgeuse_arrive;
                }
                if depart_type == CrvDirOnFaceType::Uninit || depart_type == CrvDirOnFaceType::Out {
                    depart_type = edgeuse_depart;
                }

                // ON disambiguation by divergence sampling
                if edgeuse_depart == CrvDirOnFaceType::On || edgeuse_arrive == CrvDirOnFaceType::On
                {
                    let (next_mid, prev_mid) = next_prev_mid(self)?;
                    let depart_coin =
                        edgeuse_depart == CrvDirOnFaceType::On && next_mid == PointClassType::Edge;
                    let arrive_coin =
                        edgeuse_arrive == CrvDirOnFaceType::On && prev_mid == PointClassType::Edge;
                    let smp_cnt: u32 = 10;
                    let smp_div_cnt = 100.0;
                    for kk in 0..2 {
                        if !((kk == 0 && edgeuse_depart == CrvDirOnFaceType::On && !depart_coin)
                            || (kk == 1 && edgeuse_arrive == CrvDirOnFaceType::On && !arrive_coin))
                        {
                            continue;
                        }
                        let speed_ratio = classify_crv_pn[1].length() / edge_pn[1].length();
                        let mut smp_edgeuse_param = edge_param;
                        let mut smp_edgeuse_step = (if classify_crv_pn[1].dot(edge_pn[1]) > 0.0 {
                            1.0
                        } else {
                            -1.0
                        }) * (if orient == OrientType::Same {
                            1.0
                        } else {
                            -1.0
                        }) * (if kk == 0 {
                            (edge_ivl.max() - this_param) / smp_div_cnt
                        } else {
                            (edge_ivl.min() - this_param) / smp_div_cnt
                        });
                        let mut smp_classify_crv_param = this_param;
                        let mut smp_classify_crv_step = (if kk == 0 { 1.0 } else { -1.0 })
                            * speed_ratio
                            * smp_edgeuse_step.abs();

                        // limit the ClassifyCrv step to stay in its interval
                        let max_step = ((if kk == 0 {
                            c.crv_classification_interval.max()
                        } else {
                            c.crv_classification_interval.min()
                        }) - smp_classify_crv_param)
                            / f64::from(smp_cnt + 1);
                        if max_step.abs() < smp_classify_crv_step.abs() {
                            smp_edgeuse_step *= max_step / smp_classify_crv_step;
                            smp_classify_crv_step = max_step;
                        }

                        for _ in 0..smp_cnt {
                            // Upstream declares the sample outputs inside the loop body
                            // (default `UNDEF` each iteration) and ignores the status of
                            // both samples: a failing `Evaluate` leaves `UNDEF`, a failing
                            // `EvaluateBinormal` leaves `UNDEF` for the binormal and for the
                            // point too, unless it failed after initialising its outputs, when
                            // the point is `(0,0,0)` (`edgeuse_evaluate_binormal`).
                            let smp_pn = eval_pn(k, c.classify_crv, smp_classify_crv_param)?;
                            let mut bn_point = Vec3::UNDEF;
                            let mut bn_binormal = Vec3::UNDEF;
                            edgeuse_evaluate_binormal(
                                c.k,
                                c.geometry,
                                edgeuse,
                                smp_edgeuse_param,
                                &mut bn_point,
                                &mut bn_binormal,
                            )?;
                            let gap = smp_pn[0] - bn_point;
                            if !is_degenerate_vec3(c.k, gap, None) {
                                let dir = if bn_binormal.dot(gap) > 0.0 {
                                    CrvDirOnFaceType::In
                                } else {
                                    CrvDirOnFaceType::Out
                                };
                                if kk == 0 {
                                    depart_type = dir;
                                } else {
                                    arrive_type = dir;
                                }
                                break;
                            }
                            smp_edgeuse_param += smp_edgeuse_step;
                            smp_classify_crv_param += smp_classify_crv_step;
                        }
                    }
                }
            }

            if arrive_type == CrvDirOnFaceType::Uninit {
                arrive_type = CrvDirOnFaceType::Out;
            }
            if depart_type == CrvDirOnFaceType::Uninit {
                depart_type = CrvDirOnFaceType::Out;
            }
        }
        // else: a TgtLoop-connected PtClassify->Object not a Vertex or an Edge (warning only)

        Ok((arrive_type, depart_type))
    }

    /// `pSurface->EvaluateDirectionalDerivs(uv, dir, 1, pn)` with the status ignored.
    fn directional_pn<G: ClassGeometry<O>>(
        k: &Kernel,
        c: &TouchContext<'_, O, G>,
        uv: Point2,
        dir: Vec2,
    ) -> Result<[Vec3; 2]> {
        let mut pn = [Vec3::UNDEF; 2];
        if let Some(d) = ignored(
            c.surface
                .evaluate_directional_derivs(k, uv, dir, 1, true, true),
        )? {
            for (o, v) in pn.iter_mut().zip(d) {
                *o = v;
            }
        }
        Ok(pn)
    }
}

/// The face and curve state `GetLoopCrossings` shares with its touch loop body.
struct TouchContext<'a, O, G> {
    /// The kernel.
    k: &'a Kernel,
    /// The geometry behind the handles.
    geometry: &'a G,
    /// `pSurface`: the face's surface.
    surface: &'a Surface,
    /// `pFace`.
    face: Option<O>,
    /// `pUpperFaceuse`.
    upper_faceuse: Option<O>,
    /// `pClassifyCrv`.
    classify_crv: &'a Curve,
    /// `sCrvClassification_Interval`.
    crv_classification_interval: Extent1d,
    /// `lCrvClassification_IvlCnt`.
    ivl_cnt: usize,
}
