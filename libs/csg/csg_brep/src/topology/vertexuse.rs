//! Vertex uses (`SmVertexuse`): the use of a vertex at an edge use's start, in a vertex
//! loop or alone in a shell; its owner queries, cached vertex/edge and vertex/face gaps,
//! and the UV point and UV sector of the face corner it marks.
//!
//! Upstream: `source/SMLib/src/SmVertexuse.cpp`, `source/SMLib/inc/SmVertexuse.h`.
//!
//! | Rust                                        | Upstream                                         |
//! |---------------------------------------------|--------------------------------------------------|
//! | [`TYPE`], [`TYPE_STRING`], [`CLASS_STRING`], [`is_kind_of`] | `SmVertexuse::GetType`, `GetTypeString`, `GetClassString`, `IsKindOf` |
//! | [`crate::topology::Vertexuse`]              | `SmVertexuse` (members; `Default` is `SmVertexuse()`, `Drop` `~SmVertexuse`) |
//! | [`vertexuse_type`]                          | `SmVertexuse::GetVertexuseType`                  |
//! | [`vertex()`]                                | `SmVertexuse::GetVertex`                         |
//! | [`is_loop_vertexuse`], [`is_shell_vertexuse`], [`is_edge_vertexuse`] | `SmVertexuse::IsLoopVertexuse`, `IsShellVertexuse`, `IsEdgeVertexuse` |
//! | [`edgeuse`], [`faceuse`], [`loopuse`], [`shell`] | `SmVertexuse::GetEdgeuse`, `GetFaceuse`, `GetLoopuse`, `GetShell` |
//! | [`has_vertex_edge_gap`], [`has_vertex_face_gap`] | `SmVertexuse::HasVertexEdgeGap`, `HasVertexFaceGap` |
//! | [`vertex_edge_gap`], [`vertex_face_gap`]    | `SmVertexuse::GetVertexEdgeGap`, `GetVertexFaceGap` |
//! | [`calc_vertex_edge_gap`], [`calc_vertex_face_gap`] | `SmVertexuse::CalcVertexEdgeGap`, `CalcVertexFaceGap` |
//! | [`init_vertex_face_gap`]                    | `SmVertexuse::InitVertexFaceGap`                 |
//! | [`is_connected_to`]                         | `SmVertexuse::IsConnectedTo`                     |
//! | [`is_strut_vertexuse`], [`is_on_pole`]      | `SmVertexuse::IsStrutVertexuse`, `IsOnPole`      |
//! | [`is_sector_missing_seam`], [`MissingSeam`] | `SmVertexuse::IsSectorMissingSeam`               |
//! | [`compute_uv_point`]                        | `SmVertexuse::ComputeUVPoint`                    |
//! | [`compute_uv_sector`], [`UvSectorIo`], [`UvSector`] | `SmVertexuse::ComputeUVSector`, `SmUVSectorIO` |
//! | [`compute_t_point`]                          | `SmVertexuse::ComputeTPoint`                     |
//! | [`notify`]                                  | `SmVertexuse::Notify`                            |
//! | [`edge_is_wire`]                            | `SmEdge::IsWire` (CO-PORTED, `topology.edge`)    |
//!
//! Deviations:
//! - **Object model.** A vertex use is a record in the [`Brep`]'s arenas; the methods
//!   are free functions taking the B-rep and the use's id. Pointer results are
//!   `Option`s of ids, `GetVertexEdgeGap` / `GetVertexFaceGap` return a copy of the
//!   cached gap. `GetVertexuseType` returns the `SmTopoTypes.h` code of the owner kind.
//!   The constructor's and destructor's `SmObject::Notify` calls are not modelled.
//! - **Null pointers.** Where upstream dereferences a null pointer (a loop vertex use
//!   without loop use, an edge vertex use without edge use, a sector without end edge
//!   use, a vertex use without vertex, ...) the port returns
//!   [`SolidError::InvalidInput`]. `NER` / `SER` returns are
//!   [`SolidError::Upstream`] with the upstream code. `SM_ASSERT`s, `SM_DBG_WARN`s and
//!   the `SM_DEBUG_CODE` blocks are compiled out of the reference build and are not
//!   ported (so `SmTol::IsZeroSpeed` of the sector speeds and `SmEdge::IsSeam` are
//!   never called).
//! - **Vertex use lists.** `SmVertex::GetVertexusesOfEdge` / `GetVertexusesOfFace`
//!   are [`vertex::vertexuses_of_edge`] / [`vertex::vertexuses_of_face`] over the
//!   vertex's use list ([`crate::topology::Vertex::vertexuses`]).
//! - **Statuses ignored upstream.** Where upstream ignores a status (`ComputeUVPoint`
//!   inside `ComputeUVSector`, `FindDegenParamForDirection`, `DropVectors`,
//!   `Unitize`, the angle computations, ...) the port ignores the error as well and
//!   keeps the output upstream keeps (an angle of `0`, an unchanged point);
//!   [`SolidError::Unported`] still propagates. The edge curve and surface
//!   evaluations of `ComputeUVSector`, whose failure leaves uninitialised locals
//!   upstream, propagate their error instead.
//! - **`IsKindOf(SmFilletVertex_TYPE)`** in `HasVertexFaceGap` is always false for a
//!   vertex use (`SmFilletVertex` derives from `SmVertex`) and is dropped.
//! - **Notify.** The vertex use part (clearing the cached gaps) is applied to the
//!   record; the `SmTopology::Notify` step runs on the caller's topology records as
//!   [`topo::notify`] does.
//! - **Unported callees.** `SmEdgeuse::NormalizedEvaluate` is wired
//!   (`topology.edgeuse_p3`; its own deferred callees still return
//!   [`SolidError::Unported`]); `SmLoopuse::GetOrientation` (`topology.loopuse`),
//!   `SmSurface::LocalPointSolve` / `IsSingularity` (`surface.surface_p5`),
//!   `GlobalPointSolve` / `IsOnSeam` (`surface.surface_p4`; a plane's overrides are
//!   ported) and `GetSingularities` (`surface.surface_p3`) are wired. The gap evaluation of
//!   `SmVertexEdgeGap::Set` / `SmVertexFaceGap::Set` is [`crate::heal::gap`].
//!   `GlobalPointSolve` runs the surface-cache sequence around it: the face surface's
//!   kept cache (`smsurf_GetSurfaceCache`, `query::face::p2::face_surface_cache`) is
//!   checked out for the solve, which stands for the temporary `m_bHaveTSurfaceCache`
//!   off (the face classifications inside find no cache); its fast drop tests the
//!   solutions against the face when the cache is trimmed with point testing on (not
//!   switched off by `bOKToMakeUVTrimCurve`). The Rust solver itself builds no surface
//!   cache, so `m_bProcessBoundaryCurves` has nothing to act on. A cache already in use
//!   by an enclosing caller is not fetched again and no point is tested (upstream
//!   reaches it mostly while its tree is built, with point testing off).
//! - `GetTurningAngDeg`, `ComputeTPoint`, `GetBrep`, the fresh-gap checks,
//!   `GetVertexFaceTrimCurveGap`, `RefreshGaps`, `IsOnSeam`,
//!   `IsSectorGapFreeAndOriented`, `AssertValid`, `Draw*` and the `Dump`s are not
//!   part of this unit.

use crate::error::{EntityRef, Result, SolidError, SM_ERR_NULL_POINTER};
use crate::geom::attribute::AttributeStore;
#[cfg(test)]
use crate::geom::attribute::ObjectKey;
use crate::geom::context::Context;
use crate::geom::extent2d::Extent2d;
use crate::geom::global_solver::{SolutionRequested, SolverOperation};
use crate::geom::object::{NotifyOperation, ObjectRef};
use crate::geom::tol;
use crate::geom::tol::ZONE_TOL_3D;
use crate::geom::vector2d::{Point2, Vec2};
use crate::heal::gap::{VertexEdgeGap, VertexFaceGap};
use crate::kernel::Kernel;
use crate::math::{rad_to_deg, Point3, Vec3, EFF_ZERO_DEG, EFF_ZERO_PARAM, PI};
use crate::nurbs::b_spline_surface::p1::SurfParamType;
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::nurbs::knots::Side;
use crate::surface::surface::p7::drop_vectors_at;
use crate::surface::surface::{Surface, SurfaceKind};
use crate::topology::edgeuse::p1 as eu1;
#[cfg(test)]
use crate::topology::topology::Topology;
use crate::topology::topology::{self as topo};
use crate::topology::vertex;
use crate::topology::{
    Brep, EdgeId, EdgeuseId, EdgeuseOwner, FaceId, FaceuseId, LoopuseId, ShellId, VertexId,
    VertexuseId, VertexuseOwner,
};

const OP: &str = "topology::vertexuse";

/// `SmVertexuse_TYPE` (`TOPO_BASE_TYPE + 6`).
pub const TYPE: i64 = 16006;
/// `SmVertexuse::GetTypeString`.
pub const TYPE_STRING: &str = "SmVertexuse_TYPE";
/// `SmVertexuse::GetClassString`.
pub const CLASS_STRING: &str = "SmVertexuse";

/// `SmShell_TYPE`, a [`vertexuse_type`].
pub const SHELL_TYPE: i64 = 16013;
/// `SmLoopuse_TYPE`, a [`vertexuse_type`].
pub const LOOPUSE_TYPE: i64 = 16004;
/// `SmEdgeuse_TYPE`, a [`vertexuse_type`].
pub const EDGEUSE_TYPE: i64 = 16005;
/// `SmUnknown_TYPE`, the [`vertexuse_type`] of an unlinked vertex use.
pub const UNKNOWN_TYPE: i64 = 16999;

/// `SM_APPROX_TOL_3D` (`SM_ZONE_TOL_3D / 2`).
const APPROX_TOL_3D: f64 = ZONE_TOL_3D / 2.0;

/// `SmVertexuse::IsKindOf`: [`TYPE`], then the `SmTopology` chain.
#[must_use]
pub const fn is_kind_of(t: i64) -> bool {
    if t == TYPE {
        true
    } else {
        topo::is_kind_of(t)
    }
}

fn null(reason: &'static str) -> SolidError {
    SolidError::invalid_input(OP, reason)
}

/// `NER(ptr)` on a null pointer: `SM_ERR_NULL_POINTER`.
fn ner(entity: VertexuseId) -> SolidError {
    SolidError::Upstream {
        operation: OP,
        code: SM_ERR_NULL_POINTER,
        entity: Some(EntityRef::Vertexuse(entity)),
    }
}

/// `SER(SM_ERR)` / `return SM_ERR`.
fn sm_err(entity: VertexuseId) -> SolidError {
    SolidError::Upstream {
        operation: OP,
        code: crate::error::SM_ERR,
        entity: Some(EntityRef::Vertexuse(entity)),
    }
}

/// Ignores a status upstream ignores, letting [`SolidError::Unported`] through.
fn ignored<T>(r: Result<T>) -> Result<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(e) if e.is_unported() => Err(e),
        Err(_) => Ok(None),
    }
}

/// Ignores the status of a topology call upstream ignores, letting
/// [`SolidError::Unported`] and [`SolidError::InvalidInput`] (a null pointer upstream
/// dereferences inside the call) through.
fn ignored_status<T>(r: Result<T>) -> Result<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(e @ (SolidError::Unported { .. } | SolidError::InvalidInput { .. })) => Err(e),
        Err(_) => Ok(None),
    }
}

// ---- simple access ---------------------------------------------------------------------

/// `GetVertexuseType`: [`SHELL_TYPE`], [`LOOPUSE_TYPE`], [`EDGEUSE_TYPE`] or
/// [`UNKNOWN_TYPE`].
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn vertexuse_type(b: &Brep, vu: VertexuseId) -> Result<i64> {
    Ok(match b.vertexuse(vu)?.owner {
        VertexuseOwner::Unknown => UNKNOWN_TYPE,
        VertexuseOwner::Edgeuse(_) => EDGEUSE_TYPE,
        VertexuseOwner::Loopuse(_) => LOOPUSE_TYPE,
        VertexuseOwner::Shell(_) => SHELL_TYPE,
    })
}

/// `GetVertex`: the vertex used (the list owner).
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn vertex(b: &Brep, vu: VertexuseId) -> Result<Option<VertexId>> {
    Ok(b.vertexuse(vu)?.vertex)
}

/// `IsLoopVertexuse`: the vertex connects directly to a face (a vertex loop).
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn is_loop_vertexuse(b: &Brep, vu: VertexuseId) -> Result<bool> {
    Ok(matches!(b.vertexuse(vu)?.owner, VertexuseOwner::Loopuse(_)))
}

/// `IsShellVertexuse`: the vertex connects directly to a region.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn is_shell_vertexuse(b: &Brep, vu: VertexuseId) -> Result<bool> {
    Ok(matches!(b.vertexuse(vu)?.owner, VertexuseOwner::Shell(_)))
}

/// `IsEdgeVertexuse`: the vertex starts an edge use.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn is_edge_vertexuse(b: &Brep, vu: VertexuseId) -> Result<bool> {
    Ok(matches!(b.vertexuse(vu)?.owner, VertexuseOwner::Edgeuse(_)))
}

/// `ComputeTPoint`: the parameter of the vertex use's edge end that is expected to
/// connect to its vertex: the edge interval's minimum for a same-oriented edge use, else
/// its maximum.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; `SM_ERR` for a vertex use that is not an
/// edge vertex use; `SM_ERR_NULL_POINTER` for one without edge use;
/// [`SolidError::InvalidInput`] for an edge use without edge (upstream dereferences it).
pub fn compute_t_point(b: &Brep, vu: VertexuseId) -> Result<f64> {
    // for edgeuse type vertexuses
    if !is_edge_vertexuse(b, vu)? {
        // no TPoint to compute
        return Err(sm_err(vu));
    }
    let eu = edgeuse(b, vu)?.ok_or_else(|| ner(vu))?;
    let e = eu1::edge(b, eu)?.ok_or_else(|| null("ComputeTPoint: edge use without edge"))?;
    let ivl = b.edge(e)?.interval;

    // set output
    Ok(if eu1::orientation(b, eu)? == OrientType::Same {
        ivl.min()
    } else {
        ivl.max()
    })
}

/// `GetEdgeuse`: the edge use of an edge vertex use, `None` for the other kinds.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn edgeuse(b: &Brep, vu: VertexuseId) -> Result<Option<EdgeuseId>> {
    Ok(match b.vertexuse(vu)?.owner {
        VertexuseOwner::Edgeuse(eu) => eu,
        _ => None,
    })
}

/// The edge use of an edge vertex use that upstream dereferences.
fn edgeuse_deref(b: &Brep, vu: VertexuseId) -> Result<EdgeuseId> {
    edgeuse(b, vu)?.ok_or_else(|| null("edge vertex use without edge use"))
}

/// `SmLoopuse::GetFaceuse` of a loop use upstream dereferences.
fn loopuse_faceuse(b: &Brep, lu: Option<LoopuseId>) -> Result<Option<FaceuseId>> {
    let lu = lu.ok_or_else(|| null("loop vertex use without loop use"))?;
    Ok(b.loopuse(lu)?.faceuse)
}

/// `GetFaceuse`: the face use of the face sector the vertex use starts (edge vertex
/// use) or of its vertex loop (loop vertex use); `None` for a shell vertex use, an
/// edge vertex use without edge use and an unlinked one.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; [`SolidError::InvalidInput`] for a loop
/// vertex use without loop use (upstream dereferences it).
pub fn faceuse(b: &Brep, vu: VertexuseId) -> Result<Option<FaceuseId>> {
    match b.vertexuse(vu)?.owner {
        // no face for shell vertices; NERN of a null edge use; unknown
        // m_tVertexuseType (SE only)
        VertexuseOwner::Shell(_) | VertexuseOwner::Edgeuse(None) | VertexuseOwner::Unknown => {
            Ok(None)
        }
        // for Edgeuse_type - return vertexuse->edgeuse->faceuse
        VertexuseOwner::Edgeuse(Some(eu)) => eu1::faceuse(b, eu),
        // for Loopuse_type - return vertexuse->loopuse->faceuse
        VertexuseOwner::Loopuse(lu) => loopuse_faceuse(b, lu),
    }
}

/// `GetLoopuse`: the loop use of the vertex use's edge use or vertex loop; `None` for
/// the other kinds.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; [`SolidError::InvalidInput`] for an edge
/// vertex use without edge use (upstream dereferences it).
pub fn loopuse(b: &Brep, vu: VertexuseId) -> Result<Option<LoopuseId>> {
    match b.vertexuse(vu)?.owner {
        VertexuseOwner::Edgeuse(_) => eu1::loopuse(b, edgeuse_deref(b, vu)?),
        VertexuseOwner::Loopuse(lu) => Ok(lu),
        VertexuseOwner::Shell(_) | VertexuseOwner::Unknown => Ok(None),
    }
}

/// `GetShell`: the shell the vertex use connects to (directly, through its edge use or
/// through its loop use's face use); `None` without owner.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; [`SolidError::InvalidInput`] for a loop
/// use without face use (upstream dereferences it).
pub fn shell(b: &Brep, vu: VertexuseId) -> Result<Option<ShellId>> {
    match b.vertexuse(vu)?.owner {
        // no work - no m_pSorLUorEU
        VertexuseOwner::Unknown
        | VertexuseOwner::Shell(None)
        | VertexuseOwner::Edgeuse(None)
        | VertexuseOwner::Loopuse(None) => Ok(None),
        VertexuseOwner::Shell(Some(s)) => Ok(Some(s)),
        VertexuseOwner::Edgeuse(Some(eu)) => eu1::shell(b, eu),
        VertexuseOwner::Loopuse(Some(lu)) => {
            let fu = b
                .loopuse(lu)?
                .faceuse
                .ok_or_else(|| null("GetShell: loop use without face use"))?;
            eu1::faceuse_shell(b, fu)
        }
    }
}

/// `SmEdge::IsWire` (CO-PORTED from unit `topology.edge`): the edge's primary edge use
/// is a shell (wire) edge use.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn edge_is_wire(b: &Brep, e: EdgeId) -> Result<bool> {
    match eu1::primary_edgeuse(b, e)? {
        Some(eu) => Ok(matches!(b.edgeuse(eu)?.owner, EdgeuseOwner::Shell(_))),
        None => Ok(false),
    }
}

// ---- predicates ------------------------------------------------------------------------

/// `IsStrutVertexuse`: an edge vertex use whose edge use's clockwise neighbour uses the
/// same edge (the edge does not connect to another edge at this end).
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; [`SolidError::InvalidInput`] for an edge
/// vertex use without edge use (upstream dereferences it).
pub fn is_strut_vertexuse(b: &Brep, vu: VertexuseId) -> Result<bool> {
    let mut ret = false;
    if is_edge_vertexuse(b, vu)? {
        let eu = edgeuse_deref(b, vu)?;
        if let Some(cw) = b.edgeuse(eu)?.cw {
            if eu != cw && b.edgeuse(eu)?.edge == b.edgeuse(cw)?.edge {
                ret = true;
            }
        }
    }
    Ok(ret)
}

/// `HasVertexEdgeGap`: the vertex use starts an edge use whose edge has a curve.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn has_vertex_edge_gap(b: &Brep, vu: VertexuseId) -> Result<bool> {
    // No VertexEdgeGap when vertexuse does not connect to an edge with a curve
    let Some(eu) = edgeuse(b, vu)? else {
        return Ok(false);
    };
    let Some(e) = b.edgeuse(eu)?.edge else {
        return Ok(false);
    };
    Ok(b.edge(e)?.curve.is_some())
}

/// `HasVertexFaceGap`: false for a shell vertex use and for an edge vertex use of a
/// wire edge.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; those of [`is_strut_vertexuse`].
pub fn has_vertex_face_gap(b: &Brep, vu: VertexuseId) -> Result<bool> {
    // when vertexuse does not connect to a face
    if is_shell_vertexuse(b, vu)? {
        // No face: there is no gap.
        return Ok(false);
    }

    // when vertexuse connects to a wire edge
    if is_edge_vertexuse(b, vu)? || is_strut_vertexuse(b, vu)? {
        // See if it connects to a wire edge.
        let edge = match edgeuse(b, vu)? {
            Some(eu) => b.edgeuse(eu)?.edge,
            None => None,
        };
        if let Some(e) = edge {
            if edge_is_wire(b, e)? {
                // No face for wire edge
                return Ok(false);
            }
        }
    }
    Ok(true)
}

/// `IsConnectedTo`: whether the vertex use connects to `target` (itself, its vertex,
/// edge, edge use, face, face use, loop, loop use, shell or region); `true` for `None`
/// and any other kind.
///
/// # Errors
///
/// Those of the queries reaching the target kind ([`loopuse`], [`faceuse`], ...).
pub fn is_connected_to(b: &Brep, vu: VertexuseId, target: Option<EntityRef>) -> Result<bool> {
    // no work - no Topo
    let Some(target) = target else {
        return Ok(true);
    };
    match target {
        EntityRef::Vertexuse(other) => Ok(other == vu),
        EntityRef::Vertex(v) => Ok(vertex(b, vu)? == Some(v)),
        EntityRef::Edge(e) => match edgeuse(b, vu)? {
            Some(eu) => Ok(b.edgeuse(eu)?.edge == Some(e)),
            None => Ok(false),
        },
        EntityRef::Edgeuse(eu) => Ok(b.edgeuse(eu)?.vertexuse == Some(vu)),
        EntityRef::Face(f) => match faceuse(b, vu)? {
            Some(fu) => Ok(b.faceuse(fu)?.face == Some(f)),
            None => Ok(false),
        },
        EntityRef::Faceuse(fu) => Ok(faceuse(b, vu)? == Some(fu)),
        EntityRef::Loop(l) => match loopuse(b, vu)? {
            Some(lu) => Ok(b.loopuse(lu)?.loop_ == Some(l)),
            None => Ok(false),
        },
        EntityRef::Loopuse(lu) => Ok(loopuse(b, vu)? == Some(lu)),
        EntityRef::Shell(s) => Ok(shell(b, vu)? == Some(s)),
        EntityRef::Region(r) => match shell(b, vu)? {
            Some(s) => Ok(eu1::shell_region(b, s)? == Some(r)),
            None => Ok(false),
        },
        // arrive here when the target is an unsupported type - return TRUE
        EntityRef::Brep(_) => Ok(true),
    }
}

// ---- gaps -----------------------------------------------------------------------------

/// `pVertex->GetVertexusesOfEdge(pEdge, list)` ([`vertex::vertexuses_of_edge`], no
/// connect target) of the use's vertex: the uses of the vertex whose edge use's edge is
/// `edge`; empty for no edge.
fn vertexuses_of_edge(
    b: &Brep,
    v: Option<VertexId>,
    edge: Option<EdgeId>,
) -> Result<Vec<VertexuseId>> {
    // no work - no edge
    if edge.is_none() {
        return Ok(Vec::new());
    }
    let v = v.ok_or_else(|| null("GetVertexusesOfEdge: vertex use without vertex"))?;
    vertex::vertexuses_of_edge(b, v, edge, None)
}

/// `pVertex->GetVertexusesOfFace(pFace, list)` ([`vertex::vertexuses_of_face`], no
/// connect target) of the use's vertex: the uses of the vertex whose face use's face is
/// `face`; empty for no face.
fn vertexuses_of_face(
    b: &Brep,
    v: Option<VertexId>,
    face: Option<FaceId>,
) -> Result<Vec<VertexuseId>> {
    // no work - no face
    if face.is_none() {
        return Ok(Vec::new());
    }
    let v = v.ok_or_else(|| null("GetVertexusesOfFace: vertex use without vertex"))?;
    vertex::vertexuses_of_face(b, v, face, None)
}

/// `SmVertexEdgeGap::Set(vu, dEdgeT)`: records the vertex use and parameter and
/// evaluates the gap (`CacheGap`) unless the parameter is `SM_UNDEF_DOUBLE`.
fn set_vertex_edge_gap(
    k: &Kernel,
    b: &mut Brep,
    vu: VertexuseId,
    edge_t: f64,
) -> Result<VertexEdgeGap> {
    VertexEdgeGap::with(k, b, Some(vu), edge_t)
}

/// `SmVertexFaceGap::Set(vu, &uv)`: records the vertex use and surface parameter and
/// evaluates the gap (`CacheGap`).
fn set_vertex_face_gap(
    k: &Kernel,
    b: &mut Brep,
    vu: VertexuseId,
    uv: Point2,
) -> Result<VertexFaceGap> {
    VertexFaceGap::with(k, b, Some(vu), Some(uv))
}

/// `CalcVertexEdgeGap`: computes the gap between the vertex and its edge use's curve
/// end and, when `cache`, stores it in the vertex use and every use of the vertex on
/// the same edge (after clearing their caches). Returns the computed gap when
/// `want_out` (upstream's optional output; the reset gap when there is none), `None`
/// otherwise. Upstream always returns `SM_SUCCESS`.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; [`SolidError::InvalidInput`] for an edge
/// vertex use without vertex; those of [`VertexEdgeGap::set`].
pub fn calc_vertex_edge_gap(
    k: &Kernel,
    b: &mut Brep,
    vu: VertexuseId,
    want_out: bool,
    cache: bool,
) -> Result<Option<VertexEdgeGap>> {
    // init output
    let mut out = want_out.then(VertexEdgeGap::new);

    if !is_edge_vertexuse(b, vu)? {
        return Ok(out);
    }

    // get all vertices connected to the same Edge
    let eu = edgeuse(b, vu)?;
    let edge = match eu {
        Some(eu) => b.edgeuse(eu)?.edge,
        None => None,
    };
    let v = vertex(b, vu)?;
    let siblings = vertexuses_of_edge(b, v, edge)?;

    // when asked - clear cache
    if cache {
        b.vertexuse_mut(vu)?.vertex_edge_gap.reset();
        // keep equivalent vertexuses synchronized
        for &x in &siblings {
            b.vertexuse_mut(x)?.vertex_edge_gap.reset();
        }
    }

    // no work - no VertexEdge Gap
    if !has_vertex_edge_gap(b, vu)? {
        return Ok(out);
    }

    // get edge EndPoint and param value (HasVertexEdgeGap: edge use and edge exist)
    let eu = edgeuse_deref(b, vu)?;
    let r = b.edgeuse(eu)?;
    let e = r
        .edge
        .ok_or_else(|| null("CalcVertexEdgeGap: edge use without edge"))?;
    let ivl = b.edge(e)?.interval;
    let edge_t = if r.orientation == OrientType::Same {
        ivl.min()
    } else {
        ivl.max()
    };

    // set output - SmVertexEdgeGap::Set assigns m_pVertexuse and m_dEdgeT and computes
    // m_dGap3d
    if let Some(o) = out.as_mut() {
        *o = set_vertex_edge_gap(k, b, vu, edge_t)?;
    }
    if cache {
        let g = set_vertex_edge_gap(k, b, vu, edge_t)?;
        b.vertexuse_mut(vu)?.vertex_edge_gap = g;
        // keep equivalent vertexuses synchronized
        for &x in &siblings {
            b.vertexuse_mut(x)?.vertex_edge_gap = g;
        }
    }

    // all done
    Ok(out)
}

/// `CalcVertexFaceGap`: computes the gap between the vertex and its face's surface
/// (through [`compute_uv_point`] without making UV trim curves) and, when `cache`,
/// stores it in the vertex use (after clearing the caches of every use of the vertex
/// on the same face). Returns the computed gap when `want_out` (the reset gap when
/// there is none), `None` otherwise.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; [`SolidError::InvalidInput`] where
/// upstream dereferences null; the status of [`compute_uv_point`];
/// those of [`VertexFaceGap::set`].
pub fn calc_vertex_face_gap(
    k: &Kernel,
    b: &mut Brep,
    vu: VertexuseId,
    want_out: bool,
    cache: bool,
) -> Result<Option<VertexFaceGap>> {
    // init output
    let mut out = want_out.then(VertexFaceGap::new);

    // no work - has no Face/Vertex gap
    if !has_vertex_face_gap(b, vu)? {
        return Ok(out);
    }

    // get all vertices connected to the same Face
    let face = match faceuse(b, vu)? {
        Some(fu) => b.faceuse(fu)?.face,
        None => None,
    };
    let v = vertex(b, vu)?;
    let siblings = vertexuses_of_face(b, v, face)?;

    // when asked - clear cache
    if cache {
        b.vertexuse_mut(vu)?.vertex_face_gap.reset();
        // keep equivalent vertexuses synchronized
        for &x in &siblings {
            b.vertexuse_mut(x)?.vertex_face_gap.reset();
        }
    }

    // project vertex to vertexuse->Face->Surface (FALSE = Use GlobalSolve without
    // touching UVTrimCurve)
    let uv = compute_uv_point(k, b, vu, false)?;

    // set output - SmVertexFaceGap::Set assigns m_pVertexuse and m_sFaceUV and computes
    // m_dGap3d. Across a seam the UV jumps, so the other uses are not synchronized.
    if let Some(o) = out.as_mut() {
        *o = set_vertex_face_gap(k, b, vu, uv)?;
    }
    if cache {
        let g = set_vertex_face_gap(k, b, vu, uv)?;
        b.vertexuse_mut(vu)?.vertex_face_gap = g;
    }

    // all done
    Ok(out)
}

/// `GetVertexEdgeGap`: the cached vertex/edge gap, refreshed through
/// [`calc_vertex_edge_gap`] when `force_calc` or not yet initialised; `None` without a
/// vertex/edge gap.
///
/// # Errors
///
/// Those of [`calc_vertex_edge_gap`].
pub fn vertex_edge_gap(
    k: &Kernel,
    b: &mut Brep,
    vu: VertexuseId,
    force_calc: bool,
) -> Result<Option<VertexEdgeGap>> {
    // no work - no gap
    if !has_vertex_edge_gap(b, vu)? {
        return Ok(None);
    }

    // when asked - refresh the cache (status ignored)
    if force_calc || !b.vertexuse(vu)?.vertex_edge_gap.is_init() {
        ignored_status(calc_vertex_edge_gap(k, b, vu, false, true))?;
    }

    // all done
    Ok(Some(b.vertexuse(vu)?.vertex_edge_gap))
}

/// `GetVertexFaceGap`: the cached vertex/face gap, refreshed through
/// [`calc_vertex_face_gap`] when `force_calc` or not yet initialised; `None` without a
/// vertex/face gap.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; [`SolidError::InvalidInput`] and
/// [`SolidError::Unported`] from [`calc_vertex_face_gap`] (its status is ignored).
pub fn vertex_face_gap(
    k: &Kernel,
    b: &mut Brep,
    vu: VertexuseId,
    force_calc: bool,
) -> Result<Option<VertexFaceGap>> {
    // no work - no Gap
    if !has_vertex_face_gap(b, vu)? {
        return Ok(None);
    }

    // when asked - refresh the cache (status ignored)
    if force_calc || !b.vertexuse(vu)?.vertex_face_gap.is_init() {
        ignored_status(calc_vertex_face_gap(k, b, vu, false, true))?;
    }

    // all done
    Ok(Some(b.vertexuse(vu)?.vertex_face_gap))
}

/// `InitVertexFaceGap`: resets the cached vertex/face gap.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn init_vertex_face_gap(b: &mut Brep, vu: VertexuseId) -> Result<()> {
    b.vertexuse_mut(vu)?.vertex_face_gap.reset();
    Ok(())
}

// ---- unported callees -------------------------------------------------------------------

/// `pFace->GetUVDomain()` (unit `topology.face_p2`).
fn face_uv_domain(b: &Brep, f: FaceId) -> Result<Extent2d> {
    crate::topology::face::p2::uv_domain(b, f)
}

/// `pFace->GetTolerance()` (unit `topology.face_p2`).
fn face_tolerance(b: &Brep, f: FaceId) -> Result<f64> {
    crate::topology::face::p2::tolerance(b, f)
}

/// `pLoopuse->GetOrientation()` (unit `topology.loopuse`).
fn loopuse_orientation(b: &Brep, lu: LoopuseId) -> Result<OrientType> {
    crate::topology::loopuse::orientation(b, lu)
}

/// `pEdgeuse->NormalizedEvaluate(t, TRUE, rUVPnt, &rUVTan, bOKToMakeUVTrimCurve)`: the
/// UV point and UV tangent of the edge use at normalised parameter `t` (making a UV
/// trim curve when allowed and missing).
fn normalized_evaluate(
    k: &Kernel,
    b: &mut Brep,
    eu: EdgeuseId,
    t: f64,
    ok_to_make: bool,
) -> Result<(Point3, Vec3)> {
    let r = crate::topology::edgeuse::p3::normalized_evaluate(k, b, eu, t, true, true, ok_to_make)?;
    Ok((r.point, r.first_deriv.unwrap_or(Vec3::UNDEF)))
}

/// `pEdgeuse->NormalizedEvaluate(t, TRUE, rUVPnt, NULL, bOKToMakeUVTrimCurve)`: the UV
/// point only.
fn normalized_evaluate_point(
    k: &Kernel,
    b: &mut Brep,
    eu: EdgeuseId,
    t: f64,
    ok_to_make: bool,
) -> Result<Point3> {
    Ok(
        crate::topology::edgeuse::p3::normalized_evaluate(k, b, eu, t, true, false, ok_to_make)?
            .point,
    )
}

/// A point solve's start end (`m_vStart`): the surface parameters and the solution
/// value (the distance for `SM_SO_MINIMIZE`).
#[derive(Debug, Clone, Copy, PartialEq)]
struct PointDrop {
    uv: Point2,
    value: f64,
}

/// `pSurface->LocalPointSolve(domain, op, point, guess, bFoundAnswer, sSol)`: whether
/// an answer was found and the solution.
fn local_point_solve(
    k: &Kernel,
    surface: &Surface,
    domain: &Extent2d,
    op: SolverOperation,
    point: Point3,
    guess: Point2,
) -> Result<(bool, PointDrop)> {
    let r = surface
        .local_point_solve::<crate::surface::surface::p4::NoObject>(k, domain, op, point, guess)?;
    Ok((
        r.found,
        PointDrop {
            uv: Point2::new(r.solution.start.param(0)?, r.solution.start.param(1)?),
            value: r.solution.start.solution_value,
        },
    ))
}

/// `pSurface->GlobalPointSolve(domain, op, point, tol, NULL, request, solutions)` of
/// face `f`'s surface, on the surface's cache (`smsurf_GetSurfaceCache`, `NER`) checked
/// out and made to look standalone (`m_bHaveTSurfaceCache = FALSE`) with boundary
/// curve processing on and, when `point_test` is false, point testing off (all restored
/// afterwards). The cache stays out of the surface's slot during the solve, so the
/// face classifications in it walk the loops as upstream's do with
/// `m_bHaveTSurfaceCache` off. The fast drop's trim test classifies on the B-rep when
/// the cache is a trimmed one with point testing on and its face not modified. A cache
/// already in use by an enclosing caller (upstream the same object, e.g. while its
/// tree is built with point testing off) is not fetched again, and the solve tests no
/// point.
#[allow(clippy::too_many_arguments)]
fn global_point_solve(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
    domain: &Extent2d,
    op: SolverOperation,
    point: Point3,
    tol: f64,
    request: SolutionRequested,
    point_test: bool,
) -> Result<Vec<PointDrop>> {
    use crate::intersect::point_class::{PointClassType, PointClassification};
    use crate::query::face::p2::{face_surface_cache, point_classify, PointClassifyOptions};

    let surface = face_surface(b, f)?.clone_for_evaluation()?;
    let lease = if surface.trim_cache_in_use() {
        None
    } else {
        Some(face_surface_cache(k, b, f)?)
    };
    let test_on = point_test
        && lease.as_ref().is_some_and(|l| {
            l.trimmed && l.cache.base().point_test_enabled && !l.cache.base().face_was_modified
        });
    let mut classify = |k: &Kernel, uv: Point2, src_tol: f64| -> Result<bool> {
        // pFace->PointClassify(uv, tol, TRUE, FALSE, sPtClass): good on SM_SUCCESS and
        // a face, edge or vertex class
        let mut pc = PointClassification::new(src_tol);
        let status = point_classify(
            k,
            b,
            f,
            uv,
            &mut pc,
            PointClassifyOptions {
                src_zone_tol_3d: src_tol,
                do_boundary_intersections: true,
                use_uv_classification: false,
                ..PointClassifyOptions::default()
            },
        );
        match status {
            Err(e) if e.is_unported() => Err(e),
            Err(_) => Ok(false),
            Ok(()) => Ok(matches!(
                pc.point_class(),
                PointClassType::Face | PointClassType::Edge | PointClassType::Vertex
            )),
        }
    };
    let solutions = surface
        .global_point_solve_face_tested::<crate::surface::surface::p4::NoObject>(
            k,
            Some(crate::surface::surface::p4::NoObject),
            domain,
            op,
            point,
            tol,
            None,
            request,
            if test_on { Some(&mut classify) } else { None },
        );
    if let Some(lease) = lease {
        lease.release();
    }
    solutions?
        .as_slice()
        .iter()
        .map(|s| {
            Ok(PointDrop {
                uv: Point2::new(s.start.param(0)?, s.start.param(1)?),
                value: s.start.solution_value,
            })
        })
        .collect()
}

/// `pSurface->IsSingularity(uv, eDir, dTol3d)` (virtual: the plane override, the base
/// body otherwise).
fn is_singularity(
    k: &Kernel,
    surface: &Surface,
    uv: Point2,
    tol: f64,
) -> Result<(bool, SurfParamType)> {
    match surface.kind() {
        SurfaceKind::Plane(p) => Ok(p.is_singularity()),

        SurfaceKind::Cone(_)
        | SurfaceKind::Sphere(_)
        | SurfaceKind::Torus(_)
        | SurfaceKind::BSpline(_)
        | SurfaceKind::CurveBounded(_)
        | SurfaceKind::SurfOfRevolution(_)
        | SurfaceKind::SurfOfExtrusion(_)
        | SurfaceKind::Step(_)
        | SurfaceKind::SrfInVolume(_)
        | SurfaceKind::OffsetSurface(_) => {
            let r = surface.is_singularity(k, uv, tol, false)?;
            Ok((r.singular, r.direction))
        }
    }
}

/// `pSurface->IsOnSeam(uv, NULL, &eSeamDir, ...)` (virtual: the plane override, the
/// base body otherwise).
fn is_on_seam(k: &Kernel, surface: &Surface, uv: Point2) -> Result<(bool, SurfParamType)> {
    match surface.kind() {
        SurfaceKind::Plane(p) => Ok(p.is_on_seam()),

        SurfaceKind::Cone(_)
        | SurfaceKind::Sphere(_)
        | SurfaceKind::Torus(_)
        | SurfaceKind::BSpline(_)
        | SurfaceKind::CurveBounded(_)
        | SurfaceKind::SurfOfRevolution(_)
        | SurfaceKind::SurfOfExtrusion(_)
        | SurfaceKind::Step(_)
        | SurfaceKind::SrfInVolume(_)
        | SurfaceKind::OffsetSurface(_) => {
            let info = surface.is_on_seam(k, uv, None)?;
            Ok((info.on_seam, info.seam_dir))
        }
    }
}

/// `pSurface->GetSingularities() != 0`.
fn has_singularities(k: &Kernel, surface: &Surface) -> Result<bool> {
    Ok(surface.get_singularities(k, false, false)?.singular != 0)
}

// ---- UV point and sector --------------------------------------------------------------

/// The surface of face `f` (present: checked by the caller).
fn face_surface(b: &Brep, f: FaceId) -> Result<&Surface> {
    b.face(f)?
        .surface
        .as_ref()
        .ok_or_else(|| null("face without surface"))
}

/// The 2D part of a 3D point (`SmPoint2d = SmPoint3d`).
fn xy(p: Vec3) -> Point2 {
    Point2::new(p.x, p.y)
}

/// `ComputeUVPoint`: the projection of the vertex onto its face's surface. For an edge
/// vertex use the guess comes from the UV trim curve ends of its edge use and the
/// previous (clockwise) one, picking the side of a seam both curves step into; a
/// local solve that fails, lands far from the guess or out of tolerance falls back to
/// a global solve, whose solution nearest the edge use's UV start wins. A loop vertex
/// use takes the first solution of a global intersect (then minimise) solve.
/// `ok_to_make` allows making missing UV trim curves (and point testing in the global
/// solve).
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; [`SolidError::InvalidInput`] for a vertex
/// use without vertex (upstream dereferences it); `SM_ERR_NULL_POINTER` without face
/// use, face, surface (a shell vertex use, a wire edge's vertex use), edge use or
/// previous edge use; `SM_ERR` for a shell vertex use and when no solution is found;
/// the statuses of the evaluations and solves.
pub fn compute_uv_point(
    k: &Kernel,
    b: &mut Brep,
    vu: VertexuseId,
    ok_to_make: bool,
) -> Result<Point2> {
    // Vertex and Face locals
    let v = vertex(b, vu)?.ok_or_else(|| null("ComputeUVPoint: vertex use without vertex"))?;
    let (vertex_pnt, vertex_tol) = {
        let r = b.vertex(v)?;
        (r.point, r.tolerance)
    };
    let fu = faceuse(b, vu)?.ok_or_else(|| ner(vu))?;
    let f = b.faceuse(fu)?.face.ok_or_else(|| ner(vu))?;
    if b.face(f)?.surface.is_none() {
        return Err(ner(vu));
    }
    let uv_domain = face_uv_domain(b, f)?;
    let nat_domain = face_surface(b, f)?.natural_uv_domain_with_nurb(k)?;

    let sol = if is_edge_vertexuse(b, vu)? {
        // Edgeuse locals - thisEdgeuse->StartPt == CWEdgeuse->EndPt (with tolerances)
        let eu = edgeuse(b, vu)?.ok_or_else(|| ner(vu))?;
        let prev = b.edgeuse(eu)?.cw.ok_or_else(|| ner(vu))?;

        // try quick solution based on this (and prev) Vertexuse->Edgeuse->UVTrimCurve data
        // get this SurfaceUV for ThisEdgeuse start point from UVTrimCurve
        let (this_uv_pnt, mut this_uv_tan) = normalized_evaluate(k, b, eu, 0.0, ok_to_make)?;
        let mut uv_guess = xy(this_uv_pnt);

        // get SurfaceUV for PrevEdgeuse end point from UVTrimCurve
        let (prev_uv_pnt, mut prev_uv_tan) = normalized_evaluate(k, b, prev, 1.0, ok_to_make)?;

        // when PrevUV and ThisUV UVTrimCurve EndPts are far apart - check for
        // singularities and seams
        let surface = face_surface(b, f)?;
        let surf_uv_domain = surface.natural_uv_domain_with_nurb(k)?;
        let dist_u = (this_uv_pnt.x - prev_uv_pnt.x).abs();
        let dist_v = (this_uv_pnt.y - prev_uv_pnt.y).abs();
        let domain_length_u = surf_uv_domain.x_length();
        let domain_length_v = surf_uv_domain.y_length();
        if dist_u > domain_length_u / 4.0 || dist_v > domain_length_v / 4.0 {
            let closed_u = surface
                .is_closed(k, &surf_uv_domain, SurfParamType::U, None, false)?
                .0;
            let closed_v = surface
                .is_closed(k, &surf_uv_domain, SurfParamType::V, None, false)?
                .0;
            let tol_1000 = EFF_ZERO_PARAM * 1000.0;

            // check for seam with UVTrimCurves on conflicting sides problem
            if (closed_u && tol::are_same_to_tol(dist_u, domain_length_u, tol_1000))
                || (closed_v && tol::are_same_to_tol(dist_v, domain_length_v, tol_1000))
            {
                // Express derivatives per unit normalized curve parameter
                let this_len = trim_or_edge_length(b, eu)?;
                let prev_len = trim_or_edge_length(b, prev)?;
                this_uv_tan *= this_len;
                prev_uv_tan *= prev_len;

                // At a doubly periodic corner, probe into each adjacent curve
                let umin = surf_uv_domain.min().x;
                let umax = surf_uv_domain.max().x;
                let vmin = surf_uv_domain.min().y;
                let vmax = surf_uv_domain.max().y;
                let doubly_periodic_corner = closed_u
                    && closed_v
                    && (tol::are_same_to_tol(this_uv_pnt.x, umin, tol_1000)
                        || tol::are_same_to_tol(this_uv_pnt.x, umax, tol_1000))
                    && (tol::are_same_to_tol(this_uv_pnt.y, vmin, tol_1000)
                        || tol::are_same_to_tol(this_uv_pnt.y, vmax, tol_1000));
                if doubly_periodic_corner {
                    prev_uv_tan = -prev_uv_tan;
                }

                // Away from a doubly periodic corner, only motion across the crossed seam
                // can choose its side. A torus seam partitioned at a vertex can have both
                // adjacent tangents along the seam.
                if !doubly_periodic_corner {
                    if closed_u && tol::are_same_to_tol(dist_u, domain_length_u, tol_1000) {
                        this_uv_tan.y = 0.0;
                        prev_uv_tan.y = 0.0;
                    } else {
                        this_uv_tan.x = 0.0;
                        prev_uv_tan.x = 0.0;
                    }
                }

                // skip cases where the EndTans are parallel to Seam
                if !tol::is_degenerate_vec3(k, this_uv_tan, Some(tol_1000))
                    && !tol::is_degenerate_vec3(k, prev_uv_tan, Some(tol_1000))
                {
                    let step = |p: Vec3, t: Vec3| xy(p + 0.001 * t);
                    let this_step_from_this = step(this_uv_pnt, this_uv_tan);
                    let prev_step_from_this = step(this_uv_pnt, prev_uv_tan);
                    let this_step_from_prev = step(prev_uv_pnt, this_uv_tan);
                    let prev_step_from_prev = step(prev_uv_pnt, prev_uv_tan);
                    let inside = |p: Point2| surf_uv_domain.contains_point(p, EFF_ZERO_PARAM);

                    if inside(this_step_from_this) && inside(prev_step_from_this) {
                        // The Vertexuse is probably on the sThisUVPnt side of the seam
                        uv_guess = xy(this_uv_pnt);
                    } else if inside(this_step_from_prev) && inside(prev_step_from_prev) {
                        uv_guess = xy(prev_uv_pnt);
                    } else if doubly_periodic_corner {
                        // select the corner entered by both inward probes
                        let mut found_corner = false;
                        for corner in 0..4u32 {
                            let c = Point2::new(
                                if corner & 1 != 0 { umax } else { umin },
                                if corner & 2 != 0 { vmax } else { vmin },
                            );
                            let this_step = Point2::new(
                                c.x + 0.001 * this_uv_tan.x,
                                c.y + 0.001 * this_uv_tan.y,
                            );
                            let prev_step = Point2::new(
                                c.x + 0.001 * prev_uv_tan.x,
                                c.y + 0.001 * prev_uv_tan.y,
                            );
                            if inside(this_step) && inside(prev_step) {
                                uv_guess = c;
                                found_corner = true;
                                break;
                            }
                        }
                        // Before a missing seam is inserted, the two sector arms can enter
                        // opposite periodic sides without a common corner. Preserve this
                        // edgeuse's UV; ComputeUVSector keeps both ends.
                        if !found_corner {
                            let across_u = tol::are_same_to_tol(dist_u, domain_length_u, tol_1000);
                            let across_v = tol::are_same_to_tol(dist_v, domain_length_v, tol_1000);
                            let missing_seam = (across_u || tol::is_zero_to_tol(dist_u, tol_1000))
                                && (across_v || tol::is_zero_to_tol(dist_v, tol_1000))
                                && ((across_u && this_uv_tan.x * prev_uv_tan.x < 0.0)
                                    || (across_v && this_uv_tan.y * prev_uv_tan.y < 0.0))
                                && inside(this_step_from_this)
                                && inside(prev_step_from_prev);
                            if missing_seam {
                                uv_guess = xy(this_uv_pnt);
                            }
                        }
                    }
                }
            }
        }

        // limit UV points surface domain
        uv_guess = uv_domain.clamp_point(uv_guess);

        // try to project vertex point down onto surface
        let (mut found, mut s) = local_point_solve(
            k,
            face_surface(b, f)?,
            &uv_domain,
            SolverOperation::Minimize,
            vertex_pnt,
            uv_guess,
        )?;

        // If the found solution is more than half the surface domain from the guess
        // assume we jumped a seam and try GlobalPointSolve [B678]
        if found
            && ((s.uv.x - uv_guess.x).abs() > nat_domain.x_length() / 2.0
                || (s.uv.y - uv_guess.y).abs() > nat_domain.y_length() / 2.0)
        {
            found = false;
        }

        // If local solve fails then go back and do a global closest point
        if !found || s.value > vertex_tol + face_tolerance(b, f)? {
            let face_tol = face_tolerance(b, f)?;
            let sols = global_point_solve(
                k,
                b,
                f,
                &uv_domain,
                SolverOperation::Minimize,
                vertex_pnt,
                vertex_tol + face_tol,
                SolutionRequested::Single,
                ok_to_make,
            )?;
            let Some(&first) = sols.first() else {
                return Err(sm_err(vu));
            };

            // Use UVTrimCurves to choose the right drop point [B678]
            if sols.len() > 1 {
                let this_uv = normalized_evaluate_point(k, b, eu, 0.0, ok_to_make)?;
                let guess = xy(this_uv);
                let mut min_dist = crate::math::BIG_DOUBLE;
                for x in &sols {
                    let d = x.uv.distance_squared(guess);
                    if d < min_dist {
                        s = *x;
                        min_dist = d;
                    }
                }
            } else {
                s = first;
            }
        }
        // a projection distance larger than the tolerance is only reported (SE)
        s.uv
    } else if is_loop_vertexuse(b, vu)? {
        // project point down to face's surface
        let face_tol = face_tolerance(b, f)?;

        let mut sols = global_point_solve(
            k,
            b,
            f,
            &uv_domain,
            SolverOperation::Intersect,
            vertex_pnt,
            vertex_tol + face_tol,
            SolutionRequested::All,
            true,
        )?;
        // If that failed, try again with Minimize. [090827]
        if sols.is_empty() {
            sols = global_point_solve(
                k,
                b,
                f,
                &uv_domain,
                SolverOperation::Minimize,
                vertex_pnt,
                vertex_tol + face_tol,
                SolutionRequested::All,
                true,
            )?;
        }
        // return only solution when just 1, or any solution when many (on a pole or seam)
        sols.first().ok_or_else(|| sm_err(vu))?.uv
    } else {
        // default case: must be a ShellVertexuse
        return Err(sm_err(vu));
    };

    Ok(sol)
}

/// The length of the edge use's UV trim curve's natural interval, or of its edge's
/// interval without trim curve.
fn trim_or_edge_length(b: &Brep, eu: EdgeuseId) -> Result<f64> {
    if let Some(c) = eu1::uv_trim_curve(b, eu)?.0 {
        return Ok(c.natural_interval().length());
    }
    let e = b
        .edgeuse(eu)?
        .edge
        .ok_or_else(|| null("ComputeUVPoint: edge use without edge"))?;
    Ok(b.edge(e)?.interval.length())
}

/// The optional outputs of [`compute_uv_sector`] (`SmUVSectorIO`).
#[allow(clippy::struct_excessive_bools)] // the flags of SmUVSectorIO
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UvSectorIo {
    /// `m_pVertexuse`: the vertex use defining the sector.
    pub vertexuse: Option<VertexuseId>,
    /// `m_pBegEdgeuse`: the sector's begin edge use (edge vertex uses only).
    pub beg_edgeuse: Option<EdgeuseId>,
    /// `m_pEndEdgeuse`: the sector's end edge use (edge vertex uses only).
    pub end_edgeuse: Option<EdgeuseId>,
    /// `m_dBegEdgeParam`: the begin edge's corner parameter.
    pub beg_edge_param: f64,
    /// `m_dEndEdgeParam`: the end edge's corner parameter.
    pub end_edge_param: f64,
    /// `m_sBegEdgeUVPoint`: the vertex's UV point at the sector's begin.
    pub beg_edge_uv_point: Point2,
    /// `m_sEndEdgeUVPoint`: the vertex's UV point at the sector's end (differs on seams
    /// and poles).
    pub end_edge_uv_point: Point2,
    /// `m_sBegEdgeUVTan`: the unit UV tangent along the begin arm.
    pub beg_edge_uv_tan: Vec2,
    /// `m_sEndEdgeUVTan`: the unit UV tangent along the end arm.
    pub end_edge_uv_tan: Vec2,
    /// `m_dSectorAngDeg`: the UV rotation from the begin to the end tangent, `[0, 360]`.
    pub sector_ang_deg: f64,
    /// `m_bVertexOnClosedU`.
    pub vertex_on_closed_u: bool,
    /// `m_bVertexOnClosedV`.
    pub vertex_on_closed_v: bool,
    /// `m_bVertexOnPole`.
    pub vertex_on_pole: bool,
    /// `m_bVertexOnFlatCorner` (never set upstream).
    pub vertex_on_flat_corner: bool,
    /// `m_bSectorMissingSeamU`.
    pub sector_missing_seam_u: bool,
    /// `m_bSectorMissingSeamV`.
    pub sector_missing_seam_v: bool,
}

impl UvSectorIo {
    /// The state `ComputeUVSector` initialises its output to for `vu`.
    #[must_use]
    pub fn new(vu: Option<VertexuseId>) -> Self {
        Self {
            vertexuse: vu,
            beg_edgeuse: None,
            end_edgeuse: None,
            beg_edge_param: crate::math::UNDEF_DOUBLE,
            end_edge_param: crate::math::UNDEF_DOUBLE,
            beg_edge_uv_point: Point2::UNDEF,
            end_edge_uv_point: Point2::UNDEF,
            beg_edge_uv_tan: Vec2::UNDEF,
            end_edge_uv_tan: Vec2::UNDEF,
            sector_ang_deg: crate::math::UNDEF_DOUBLE,
            vertex_on_closed_u: false,
            vertex_on_closed_v: false,
            vertex_on_pole: false,
            vertex_on_flat_corner: false,
            sector_missing_seam_u: false,
            sector_missing_seam_v: false,
        }
    }
}

/// The result of [`compute_uv_sector`]: upstream's status (`SM_SUCCESS` or `SM_ERR`)
/// and the outputs as far as they were set.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UvSector {
    /// `SM_SUCCESS`: the vertex use is an edge vertex use of a face corner with
    /// geometry.
    pub success: bool,
    /// The outputs.
    pub io: UvSectorIo,
}

/// `ComputeUVSector`: the UV point and the begin and end UV tangents of the face
/// corner the vertex use marks, with its UV angle and whether the vertex sits on a
/// seam or pole and whether the sector contains a seam not marked by an edge. A loop
/// vertex use gets a full 360 degree sector; a shell vertex use, a vertex use without
/// vertex or surface, an edge vertex use without sector edge uses or curves fail
/// (`success` false).
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; [`SolidError::InvalidInput`] where
/// upstream dereferences null (an edge vertex use of a face without end edge use, ...);
/// the evaluation statuses; [`SolidError::Unported`].
pub fn compute_uv_sector(
    k: &Kernel,
    b: &mut Brep,
    vu: VertexuseId,
    ok_to_make: bool,
) -> Result<UvSector> {
    // init output
    let mut io = UvSectorIo::new(Some(vu));
    let fail = |io: UvSectorIo| Ok(UvSector { success: false, io });

    // no work - no sector
    if is_shell_vertexuse(b, vu)? {
        return fail(io);
    }

    // locals
    let v = vertex(b, vu)?;
    let lu = loopuse(b, vu)?;
    let fu = faceuse(b, vu)?;
    let f = match fu {
        Some(fu) => b.faceuse(fu)?.face,
        None => None,
    };
    let f = match f {
        Some(f) if b.face(f)?.surface.is_some() => Some(f),
        _ => None,
    };

    // no work - no geometry
    let (Some(v), Some(f), Some(fu)) = (v, f, fu) else {
        return fail(io);
    };

    // locals
    let uv_domain = face_uv_domain(b, f)?;

    // compute the UV point - pass the call along (status ignored)
    if let Some(p) = ignored_status(compute_uv_point(k, b, vu, ok_to_make))? {
        io.beg_edge_uv_point = p;
    }

    match b.vertexuse(vu)?.owner {
        VertexuseOwner::Loopuse(_) => {
            // return a full 360 deg face sector
            io.beg_edge_uv_tan = Vec2::new(1.0, 0.0);
            io.end_edge_uv_tan = Vec2::new(1.0, 0.0);
            io.sector_ang_deg = 360.0;
            fail(io)
        }
        VertexuseOwner::Edgeuse(_) => {
            // Sector BegEU = pVU->EU and EndEU = its corner mate at the vertex
            io.beg_edgeuse = edgeuse(b, vu)?;
            io.end_edgeuse = match io.beg_edgeuse {
                Some(beg) => eu1::corner_mate_edgeuse(b, beg, Some(v))?,
                None => None,
            };
            // no work - no face sector - attached to wires
            let (Some(beg), Some(end)) = (io.beg_edgeuse, io.end_edgeuse) else {
                return fail(io);
            };

            // The preceding edgeuse ends at this corner. Comparing vertex objects cannot
            // distinguish its starting and ending uses when the edge is closed.
            let m = eu1::mate(b, end)?
                .ok_or_else(|| null("ComputeUVSector: end edge use without mate"))?;
            let end_vu = b
                .edgeuse(m)?
                .vertexuse
                .ok_or_else(|| null("ComputeUVSector: edge use without vertex use"))?;

            // locals
            let beg_edge = b.edgeuse(beg)?.edge;
            let end_edge = b.edgeuse(end)?.edge;

            // compute EndSector UVPoint (status ignored)
            if let Some(p) = ignored_status(compute_uv_point(k, b, end_vu, ok_to_make))? {
                io.end_edge_uv_point = p;
            }

            // no work - no geometry
            let (Some(beg_edge), Some(end_edge)) = (beg_edge, end_edge) else {
                return fail(io);
            };
            if b.edge(beg_edge)?.curve.is_none() || b.edge(end_edge)?.curve.is_none() {
                return fail(io);
            }
            let beg_orient = b.edgeuse(beg)?.orientation;
            let end_orient = b.edgeuse(end)?.orientation;
            let beg_ivl = b.edge(beg_edge)?.interval;
            let end_ivl = b.edge(end_edge)?.interval;

            // Begin and End EdgeParams
            io.beg_edge_param = if beg_orient == OrientType::Same {
                beg_ivl.min()
            } else {
                beg_ivl.max()
            };
            io.end_edge_param = if end_orient == OrientType::Opposite {
                end_ivl.min()
            } else {
                end_ivl.max()
            };

            // Begin and End Tangent Vecs, Oriented From Vertex into Face
            let eval = |b: &Brep, e: EdgeId, t: f64| -> Result<Vec3> {
                let c = b
                    .edge(e)?
                    .curve
                    .as_ref()
                    .ok_or_else(|| null("ComputeUVSector: edge without curve"))?;
                let pv = c.evaluate(k, t, 1, true, true)?;
                pv.get(1)
                    .copied()
                    .ok_or_else(|| null("ComputeUVSector: short evaluation"))
            };
            let mut beg_tan = eval(b, beg_edge, io.beg_edge_param)?;
            let mut end_tan = eval(b, end_edge, io.end_edge_param)?;
            if beg_orient == OrientType::Opposite {
                beg_tan = -beg_tan;
            }
            if end_orient == OrientType::Same {
                end_tan = -end_tan;
            }

            let surface = face_surface(b, f)?;

            // IsBegVertexUV on a Surface Singularity
            let (on_pole, singular_dir) =
                is_singularity(k, surface, io.beg_edge_uv_point, APPROX_TOL_3D)?;
            io.vertex_on_pole = on_pole;
            if on_pole {
                // Find DegenParam for BegEdge and EndEdge (statuses ignored)
                if let Some(d) = ignored(surface.find_degen_param_for_direction(
                    k,
                    io.beg_edge_uv_point,
                    beg_tan,
                    true,
                    singular_dir,
                    APPROX_TOL_3D,
                ))? {
                    io.beg_edge_uv_point = d.uv;
                }
                if let Some(d) = ignored(surface.find_degen_param_for_direction(
                    k,
                    io.end_edge_uv_point,
                    end_tan,
                    true,
                    singular_dir,
                    APPROX_TOL_3D,
                ))? {
                    io.end_edge_uv_point = d.uv;
                }
            }

            // Is BegVertexUV on a Surface FlatCorner
            let on_flat_corner = surface.is_flat_corner(k, io.beg_edge_uv_point, EFF_ZERO_DEG)?;

            if on_flat_corner {
                // on a FlatCorner - look for special case of Edges running along
                // SurfaceBoundaries
                let (_, mut du, mut dv) =
                    surface.evaluate_1st_derivatives(k, io.beg_edge_uv_point, true, true)?;

                // as needed negate sDU and sDV to point into the surface
                let mid_uv = uv_domain.mid();
                let mut u_dir = Vec2::new(1.0, 0.0);
                let mut v_dir = Vec2::new(0.0, 1.0);
                if mid_uv.x < io.beg_edge_uv_point.x {
                    du = -du;
                    u_dir = Vec2::new(-1.0, 0.0);
                }
                if mid_uv.y < io.beg_edge_uv_point.y {
                    dv = -dv;
                    v_dir = Vec2::new(0.0, -1.0);
                }

                // Expect Beg and End Edge tangents to align one each with sDU and sDV
                let par = |a: Vec3, d: Vec3| {
                    let (p, anti) = a.parallel_sense(d, EFF_ZERO_DEG);
                    p && anti == Some(false)
                };
                let beg_du = par(beg_tan, du);
                let beg_dv = par(beg_tan, dv);
                let end_du = par(end_tan, du);
                let end_dv = par(end_tan, dv);
                if beg_du {
                    io.beg_edge_uv_tan = u_dir;
                }
                if beg_dv {
                    io.beg_edge_uv_tan = v_dir;
                }
                if end_du {
                    io.end_edge_uv_tan = u_dir;
                }
                if end_dv {
                    io.end_edge_uv_tan = v_dir;
                }
            } else {
                // project Beg and End Edge Tangent Vecs down to UVPlane Corner and unitize
                for (uv, tan, out) in [
                    (io.beg_edge_uv_point, beg_tan, &mut io.beg_edge_uv_tan),
                    (io.end_edge_uv_point, end_tan, &mut io.end_edge_uv_tan),
                ] {
                    if let Some(d) = ignored(drop_vectors_at(
                        k,
                        surface,
                        uv,
                        Side::Left,
                        Side::Left,
                        &[tan],
                    ))? {
                        if let Some(&t) = d.first() {
                            *out = t;
                        }
                    }
                    if let Ok(u) = out.unitize() {
                        *out = u;
                    }
                }
            }

            // count Opposite orientations
            let lu = lu.ok_or_else(|| null("ComputeUVSector: edge use without loop use"))?;
            let opposite_count = u32::from(b.faceuse(fu)?.orientation == OrientType::Opposite)
                + u32::from(loopuse_orientation(b, lu)? == OrientType::Opposite);

            // UVSector AngDeg in SurfNormal coordinates (a failed angle is 0)
            let ang = if opposite_count == 0 || opposite_count == 2 {
                io.beg_edge_uv_tan.ccw_angle_between(io.end_edge_uv_tan)
            } else {
                io.end_edge_uv_tan.ccw_angle_between(io.beg_edge_uv_tan)
            };
            io.sector_ang_deg = rad_to_deg(ignored(ang)?.unwrap_or(0.0));

            // seam-ending-at-a-pole and crack special cases, not a single-edge loop
            if beg != end && b.edgeuse(beg)?.edge == b.edgeuse(end)?.edge {
                io.sector_ang_deg = 360.0;
            }

            // Is BegVertexUV on a Surface->Seam
            let surface = face_surface(b, f)?;
            let (on_seam, seam_dir) = is_on_seam(k, surface, io.beg_edge_uv_point)?;
            let seam_u = seam_dir == SurfParamType::U || seam_dir == SurfParamType::Both;
            let seam_v = seam_dir == SurfParamType::V || seam_dir == SurfParamType::Both;
            io.vertex_on_closed_u = on_seam && seam_u;
            io.vertex_on_closed_v = on_seam && seam_v;

            // When Vertex is on a SurfaceSeam - check for Missing SeamEdge
            if on_seam {
                let (_, beg_du, beg_dv) =
                    surface.evaluate_1st_derivatives(k, io.beg_edge_uv_point, true, true)?;
                let (_, end_du, end_dv) =
                    surface.evaluate_1st_derivatives(k, io.end_edge_uv_point, true, true)?;
                let rad_limit = 0.5 / 180.0 * PI;
                let in_range = |a: f64| a > rad_limit && a < PI - rad_limit;
                let angle = |a: Vec3, d: Vec3| -> Result<f64> {
                    Ok(ignored(a.angle_between(d))?.unwrap_or(0.0))
                };
                let mut has_iso_u = false;
                let mut has_iso_v = false;
                if seam_u {
                    let a1 = angle(beg_tan, beg_dv)?;
                    let a2 = angle(end_tan, end_dv)?;
                    has_iso_u = (io.beg_edge_uv_tan.x * io.end_edge_uv_tan.x) < 0.0
                        && in_range(a1)
                        && in_range(a2);
                }
                if seam_v {
                    let a1 = angle(beg_tan, beg_du)?;
                    let a2 = angle(end_tan, end_du)?;
                    has_iso_v = (io.beg_edge_uv_tan.y * io.end_edge_uv_tan.y) < 0.0
                        && in_range(a1)
                        && in_range(a2);
                }
                io.sector_missing_seam_u = on_seam && seam_u && has_iso_u;
                io.sector_missing_seam_v = on_seam && seam_v && has_iso_v;
            }

            // all done
            Ok(UvSector { success: true, io })
        }
        // vertexuse is in an uninitialized state
        VertexuseOwner::Shell(_) | VertexuseOwner::Unknown => fail(io),
    }
}

/// The result of [`is_sector_missing_seam`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MissingSeam {
    /// The sector contains a seam not marked by a seam edge (either direction).
    pub missing: bool,
    /// `*pOptSectorMissingSeamU`.
    pub u: bool,
    /// `*pOptSectorMissingSeamV`.
    pub v: bool,
}

/// `IsSectorMissingSeam`: whether the vertex use's sector ([`compute_uv_sector`],
/// status ignored) contains a surface seam not marked by a seam edge; `uv_space_okay`
/// is passed on as `ComputeUVSector`'s `bOKToMakeUVTrimCurve`.
///
/// # Errors
///
/// As [`compute_uv_sector`].
pub fn is_sector_missing_seam(
    k: &Kernel,
    b: &mut Brep,
    vu: VertexuseId,
    uv_space_okay: bool,
) -> Result<MissingSeam> {
    let s = compute_uv_sector(k, b, vu, uv_space_okay)?;
    Ok(MissingSeam {
        missing: s.io.sector_missing_seam_u || s.io.sector_missing_seam_v,
        u: s.io.sector_missing_seam_u,
        v: s.io.sector_missing_seam_v,
    })
}

/// `IsOnPole`: whether the vertex maps to a pole of its face's surface (through the
/// cached vertex/face gap's UV, [`vertex_face_gap`]).
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; those of [`vertex_face_gap`];
/// [`SolidError::Unported`].
pub fn is_on_pole(k: &Kernel, b: &mut Brep, vu: VertexuseId) -> Result<bool> {
    let mut on_pole = false;

    // walk topology from vertexuse to surface
    let f = match faceuse(b, vu)? {
        Some(fu) => b.faceuse(fu)?.face,
        None => None,
    };
    let f = match f {
        Some(f) if b.face(f)?.surface.is_some() => Some(f),
        _ => None,
    };

    // when vertexuse connects to a face->Surface with poles - see if vertex is on pole
    if let Some(f) = f {
        if has_singularities(k, face_surface(b, f)?)? && has_vertex_face_gap(b, vu)? {
            // find the gap to the surface
            let gap = vertex_face_gap(k, b, vu, false)?
                .ok_or_else(|| null("IsOnPole: no vertex/face gap"))?;
            let face_uv = gap.face_uv;
            if face_uv.is_initialized()
                && is_singularity(k, face_surface(b, f)?, face_uv, crate::math::EFF_ZERO)?.0
            {
                on_pole = true;
            }
        }
    }

    // all done
    Ok(on_pole)
}

// ---- maintenance ----------------------------------------------------------------------

/// `Notify`: clears the cached gaps on `PostEdit`, `SplitInBrep`, `ChangeGeometry` and
/// `Split`, then passes the notification to [`topo::notify`] (`SmTopology::Notify`) on
/// the caller's topology `records` (`this` is the vertex use's identity).
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; those of [`topo::notify`].
pub fn notify(
    b: &mut Brep,
    vu: VertexuseId,
    records: &mut crate::topology::topology::NotifyRecords,
    attributes: &mut AttributeStore,
    context: Option<&mut Context>,
    operation: NotifyOperation,
    this: ObjectRef,
    data: [Option<ObjectRef>; 3],
) -> Result<()> {
    use NotifyOperation as N;
    match operation {
        N::PostEdit | N::SplitInBrep | N::ChangeGeometry | N::Split => {
            // Clear Cached Gap data
            let r = b.vertexuse_mut(vu)?;
            r.vertex_edge_gap.reset();
            r.vertex_face_gap.reset();
        }
        // SM_NO_UNKNOWN: SE_MSG only (compiled out of the release build)
        _ => {}
    }

    // Propagate notification up hierarchy
    let [d1, d2, d3] = data;
    topo::notify(records, attributes, context, operation, this, d1, d2, d3)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::extent1d::Extent1d;
    use crate::topology::{Edge, Edgeuse, Loopuse, Region, Shell, Vertex};

    /// A wire edge from v0 to v1: two shell edge uses and their vertex uses, plus a
    /// shell vertex use of a third vertex.
    fn wire() -> (Brep, [VertexuseId; 3], [EdgeuseId; 2]) {
        let mut b = Brep::new();
        let r = b
            .add_region_record(Region {
                in_brep: true,
                ..Region::default()
            })
            .unwrap();
        let s = b
            .add_shell_record(Shell {
                faces: Vec::new(),
                region: Some(r),
                ..Shell::default()
            })
            .unwrap();
        let v: Vec<VertexId> = (0..3)
            .map(|i| {
                b.add_vertex_record(Vertex {
                    point: Vec3::new(f64::from(i), 0.0, 0.0),
                    tolerance: ZONE_TOL_3D,
                    ..Vertex::default()
                })
                .unwrap()
            })
            .collect();
        let e = b
            .add_edge_record(Edge {
                vertices: [v[0], v[1]],
                edgeuses: vec![EdgeuseId::from_index(0), EdgeuseId::from_index(1)],
                interval: Extent1d::from_min_max(0.0, 1.0),
                tolerance: ZONE_TOL_3D,
                curve: None,
            })
            .unwrap();
        let eus: Vec<EdgeuseId> = (0..2)
            .map(|i| {
                b.add_edgeuse_record(Edgeuse {
                    edge: Some(e),
                    owner: EdgeuseOwner::Shell(Some(s)),
                    vertexuse: Some(VertexuseId::from_index(i)),
                    orientation: if i == 0 {
                        OrientType::Same
                    } else {
                        OrientType::Opposite
                    },
                    ..Edgeuse::default()
                })
                .unwrap()
            })
            .collect();
        let vus: Vec<VertexuseId> = (0..2)
            .map(|i| {
                b.add_vertexuse_record(crate::topology::Vertexuse {
                    vertex: Some(v[i]),
                    owner: VertexuseOwner::Edgeuse(Some(eus[i])),
                    ..crate::topology::Vertexuse::default()
                })
                .unwrap()
            })
            .collect();
        let svu = b
            .add_vertexuse_record(crate::topology::Vertexuse {
                vertex: Some(v[2]),
                owner: VertexuseOwner::Shell(Some(s)),
                ..crate::topology::Vertexuse::default()
            })
            .unwrap();
        b.region_mut(r).unwrap().shells.push(s);
        b.shell_mut(s).unwrap().shell_type = crate::topology::shell::ShellType::Wire;
        b.shell_mut(s).unwrap().members = crate::topology::shell::ShellMembers::Edgeuse(eus[0]);
        for (vertex, vertexuse) in v.iter().zip([vus[0], vus[1], svu]) {
            b.vertex_mut(*vertex).unwrap().vertexuses.push(vertexuse);
        }
        (b, [vus[0], vus[1], svu], [eus[0], eus[1]])
    }

    #[test]
    fn kinds_and_owners() {
        let (b, vu, eu) = wire();
        assert_eq!(vertexuse_type(&b, vu[0]).unwrap(), EDGEUSE_TYPE);
        assert_eq!(vertexuse_type(&b, vu[2]).unwrap(), SHELL_TYPE);
        assert!(is_edge_vertexuse(&b, vu[0]).unwrap());
        assert!(is_shell_vertexuse(&b, vu[2]).unwrap());
        assert_eq!(edgeuse(&b, vu[0]).unwrap(), Some(eu[0]));
        assert_eq!(edgeuse(&b, vu[2]).unwrap(), None);
        assert_eq!(faceuse(&b, vu[0]).unwrap(), None);
        assert_eq!(loopuse(&b, vu[0]).unwrap(), None);
        assert_eq!(shell(&b, vu[0]).unwrap(), Some(ShellId::from_index(0)));
        assert_eq!(shell(&b, vu[2]).unwrap(), Some(ShellId::from_index(0)));
        assert!(!is_strut_vertexuse(&b, vu[0]).unwrap());
        assert!(!has_vertex_edge_gap(&b, vu[0]).unwrap());
        assert!(!has_vertex_face_gap(&b, vu[0]).unwrap());
        assert!(!has_vertex_face_gap(&b, vu[2]).unwrap());
        assert!(edge_is_wire(&b, EdgeId::from_index(0)).unwrap());
        assert!(is_kind_of(TYPE) && is_kind_of(topo::TYPE) && !is_kind_of(16005));
        let d = crate::topology::Vertexuse::default();
        assert_eq!(d.uv_point, Point2::new(1.0e20, 1.0e20));
        assert!(!d.vertex_edge_gap.is_init() && !d.vertex_face_gap.is_init());
    }

    #[test]
    fn connections() {
        let (b, vu, eu) = wire();
        let t = |e| is_connected_to(&b, vu[0], Some(e)).unwrap();
        assert!(is_connected_to(&b, vu[0], None).unwrap());
        assert!(t(EntityRef::Vertexuse(vu[0])));
        assert!(!t(EntityRef::Vertexuse(vu[1])));
        assert!(t(EntityRef::Vertex(VertexId::from_index(0))));
        assert!(t(EntityRef::Edge(EdgeId::from_index(0))));
        assert!(t(EntityRef::Edgeuse(eu[0])));
        assert!(!t(EntityRef::Edgeuse(eu[1])));
        assert!(!t(EntityRef::Face(FaceId::from_index(0))));
        assert!(t(EntityRef::Shell(ShellId::from_index(0))));
        assert!(t(EntityRef::Region(crate::topology::RegionId::from_index(
            0
        ))));
        assert!(t(EntityRef::Brep(crate::topology::BrepId::from_index(0))));
    }

    #[test]
    fn gaps_and_null_paths() {
        let k = Kernel::default();
        let (mut b, vu, _) = wire();
        b.vertexuse_mut(vu[0]).unwrap().vertex_edge_gap.vertexuse = Some(vu[0]);
        // no curve: the cache is cleared and nothing is evaluated
        let g = calc_vertex_edge_gap(&Kernel::default(), &mut b, vu[0], true, true).unwrap();
        assert_eq!(g, Some(VertexEdgeGap::new()));
        assert!(!b.vertexuse(vu[0]).unwrap().vertex_edge_gap.is_init());
        assert_eq!(
            vertex_edge_gap(&Kernel::default(), &mut b, vu[0], true).unwrap(),
            None
        );
        assert_eq!(vertex_face_gap(&k, &mut b, vu[0], true).unwrap(), None);
        assert_eq!(
            calc_vertex_face_gap(&k, &mut b, vu[2], true, true).unwrap(),
            Some(VertexFaceGap::new())
        );
        // shell and wire vertex uses have no face: NER
        for x in [vu[0], vu[2]] {
            assert!(matches!(
                compute_uv_point(&k, &mut b, x, true),
                Err(SolidError::Upstream {
                    code: SM_ERR_NULL_POINTER,
                    ..
                })
            ));
            assert!(!compute_uv_sector(&k, &mut b, x, true).unwrap().success);
            assert!(!is_on_pole(&k, &mut b, x).unwrap());
            assert!(!is_sector_missing_seam(&k, &mut b, x, true).unwrap().missing);
        }
        // a loop vertex use without loop use: upstream dereferences null
        let lvu = b
            .add_vertexuse_record(crate::topology::Vertexuse {
                vertex: Some(VertexId::from_index(2)),
                owner: VertexuseOwner::Loopuse(None),
                ..crate::topology::Vertexuse::default()
            })
            .unwrap();
        assert!(matches!(
            faceuse(&b, lvu),
            Err(SolidError::InvalidInput { .. })
        ));
        assert_eq!(shell(&b, lvu).unwrap(), None);
        let lu = b.add_loopuse_record(Loopuse::default()).unwrap();
        b.vertexuse_mut(lvu).unwrap().owner = VertexuseOwner::Loopuse(Some(lu));
        assert_eq!(faceuse(&b, lvu).unwrap(), None);
        assert_eq!(loopuse(&b, lvu).unwrap(), Some(lu));
        assert!(matches!(
            shell(&b, lvu),
            Err(SolidError::InvalidInput { .. })
        ));
    }

    #[test]
    fn notify_clears_gaps() {
        let (mut b, vu, _) = wire();
        let mut records = crate::topology::topology::NotifyRecords::new();
        let this = ObjectRef {
            type_code: TYPE,
            key: ObjectKey(7),
        };
        records.insert(this.key, Topology::new());
        let mut attrs = AttributeStore::default();
        for (op, cleared) in [
            (NotifyOperation::PreEdit, false),
            (NotifyOperation::Split, true),
        ] {
            let r = b.vertexuse_mut(vu[0]).unwrap();
            r.vertex_edge_gap.vertexuse = Some(vu[0]);
            r.vertex_face_gap.vertexuse = Some(vu[0]);
            notify(
                &mut b,
                vu[0],
                &mut records,
                &mut attrs,
                None,
                op,
                this,
                [None; 3],
            )
            .unwrap();
            let r = b.vertexuse(vu[0]).unwrap();
            assert_eq!(r.vertex_edge_gap.is_init(), !cleared);
            assert_eq!(r.vertex_face_gap.is_init(), !cleared);
        }
    }
}
