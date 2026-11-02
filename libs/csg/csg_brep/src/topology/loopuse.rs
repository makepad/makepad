//! Loops (`SmLoop`) and loop uses (`SmLoopuse`): a face boundary and its use on each
//! side of the face, their member walks (edge uses, edges, vertex uses, vertices in CCW
//! order), closure and connectivity predicates, orientation flipping, the UV corners
//! of a loop and the missing-seam test of its vertex sectors.
//!
//! Upstream: `source/SMLib/src/SmLoop.cpp`, `source/SMLib/inc/SmLoop.h`,
//! `source/SMLib/src/SmLoopuse.cpp`, `source/SMLib/inc/SmLoopuse.h`.
//!
//! | Rust                                         | Upstream                                         |
//! |----------------------------------------------|--------------------------------------------------|
//! | [`TYPE`], [`TYPE_STRING`], [`CLASS_STRING`], [`is_kind_of`] | `SmLoopuse::GetType`, `GetTypeString`, `GetClassString`, `IsKindOf` |
//! | [`LOOP_TYPE`], [`LOOP_TYPE_STRING`], [`LOOP_CLASS_STRING`], [`loop_class_type`], [`loop_is_kind_of`] | `SmLoop::GetType`, `GetTypeString`, `GetClassString`, `GetClassType`, `IsKindOf` |
//! | [`crate::topology::Loopuse`]                 | `SmLoopuse` (members; `Default` is `SmLoopuse()`, `Drop` `~SmLoopuse`) |
//! | [`crate::topology::Loop`]                    | `SmLoop` (`m_pLU` is [`crate::topology::Loop::loopuse`]; `SmLoop()` / `~SmLoop`) |
//! | [`vertexuse`], [`faceuse`], [`loop_`], [`orientation`], [`other_loopuse`] | `SmLoopuse::GetVertexuse`, `GetFaceuse`, `GetLoop`, `GetOrientation`, `GetOtherLoopuse` |
//! | [`eu_or_vu`], [`loopuse_type`]               | `SmLoopuse::GetEUorVU`, `GetLoopuseType`         |
//! | [`is_vertex_loopuse`], [`is_edge_loopuse`]   | `SmLoopuse::IsVertexLoopuse`, `IsEdgeLoopuse`    |
//! | [`set_orientation`], [`set_loop`], [`set_edgeuse`] | `SmLoopuse::SetOrientation`, `SetLoop`, `SetEdgeuse` |
//! | [`collect_edgeuses`]                         | `SmLoopuse::CollectEdgeuses`                     |
//! | [`edges`], [`edgeuses`], [`vertexuses`], [`vertices`] | `SmLoopuse::GetEdges`, `GetEdgeuses`, `GetVertexuses`, `GetVertices` (`SmTArray` overloads) |
//! | [`shell`]                                    | `SmLoopuse::GetShell`                            |
//! | [`is_closed_3d`], [`Closed3d`]               | `SmLoopuse::IsClosed3d`                          |
//! | [`is_closed_ptrs`]                           | `SmLoopuse::IsClosedPtrs`                        |
//! | [`is_connected_to`]                          | `SmLoopuse::IsConnectedTo`                       |
//! | [`is_missing_seam_at_vertices`]              | `SmLoopuse::IsMissingSeamAtVertices`             |
//! | [`loop_face`], [`loop_loopuse`], [`loop_loopuses`] | `SmLoop::GetFace`, `GetLoopuse`, `GetLoopuses` |
//! | [`loop_vertices`], [`loop_edges`], [`loop_edgeuses`] | `SmLoop::GetVertices`, `GetEdges`, `GetEdgeuses` |
//! | [`flip_loop_orientation`]                    | `SmLoop::FlipLoopOrientation`                    |
//! | [`is_outer_loop`]                            | `SmLoop::IsOuterLoop`                            |
//! | [`loop_is_closed_3d`], [`loop_is_closed_ptrs`] | `SmLoop::IsClosed3d`, `IsClosedPtrs`           |
//! | [`loop_is_connected_to`]                     | `SmLoop::IsConnectedTo`                          |
//! | [`uv_corner_boundaries`], [`UvCorners`]      | `SmLoop::GetUVCornerBoundaries`                  |
//! | [`calc_quad_num`]                            | `sm_CalcQuadNum` (static)                        |
//! | [`is_loopuse_side_in_surface`], [`LoopuseSide`] | `SmLoopuse::IsLoopuseSideInSurface`           |
//!
//! Deviations:
//! - **Object model.** Loops and loop uses are records in the [`Brep`]'s arenas; the
//!   methods are free functions taking the B-rep and the id. Pointer arguments and
//!   results are `Option`s of ids, out-params are return values. `GetEUorVU` returns
//!   the start as an [`EntityRef`]; `GetLoopuseType` the `SmTopoTypes.h` code. The
//!   constructors' and destructors' `SmObject::Notify` calls are not modelled.
//! - **Null pointers.** `SM_ASSERT`s are compiled out of the reference build and are
//!   not checked; where upstream then dereferences a null pointer (a loop without loop
//!   use, a loop use without face use or mate, an edge use without edge or curve, ...)
//!   the port returns [`SolidError::InvalidInput`]. `NER` / `SER` returns are
//!   [`SolidError::Upstream`] (`SM_ERR_NULL_POINTER`, `SM_ERR`). `FlipLoopOrientation`
//!   reads the mate's start as the same kind as the loop use's own (an edge use read
//!   as a vertex use, or the reverse, is undefined behaviour upstream) and swaps the
//!   owner of vertex uses that are loop vertex uses; any other combination is
//!   [`SolidError::InvalidInput`].
//! - **Attribute filters.** `pOptAttributeId` is an [`AttributeFilter`]: a predicate
//!   telling whether an entity holds an attribute with that id (`FindAttribute(id) !=
//!   NULL`); `None` keeps every entity. Entities passed to it are never null
//!   (upstream dereferences a null edge or vertex to test it:
//!   [`SolidError::InvalidInput`]).
//! - **CCW walks.** The walks of an edge loop stop with [`SolidError::InvalidInput`]
//!   after visiting more edge uses than the B-rep holds (upstream loops forever on a
//!   CCW chain that does not return to its start). `CollectEdgeuses` keeps upstream's
//!   own bound (`SM_ERR` after 100000 steps).
//! - **`IsClosed3d`** computes `dOverTol = XSectTol3d - Gap`, which is negative for a
//!   gap over tolerance, and keeps it only when it exceeds `0.0`: no gap ever counts as
//!   over tolerance, the loop is always reported closed (with the largest gap), and
//!   `bCheckAllGaps == FALSE` only stops the scan at the first gap over tolerance.
//!   Ported as is (`plan/upstream-issues.md`). The tolerances keep the
//!   `SmZoneTol3d` / `SmXSectTol3d` value semantics (`fabs`, `SM_USE_DEFAULT`; see
//!   [`tol::zone_tol3d`]), which is why it takes the [`Kernel`]. The edge curve evaluations' statuses
//!   are ignored as upstream (a failed evaluation keeps the previous point, initially
//!   [`Vec3::UNDEF`]); [`SolidError::Unported`] still propagates. `SmLoop::IsClosed3d`
//!   of a loop without loop use returns `None` (closed, outputs not written).
//! - **`IsMissingSeamAtVertices`** ignores `ComputeUVSector`'s status, as upstream, and
//!   reads the missing-seam flags of the sector it returns.
//! - **`SmFace::Notify`** (the `PRE_EDIT` / `POST_EDIT` around `FlipLoopOrientation` of
//!   a loop on a face) runs with an empty attribute store and topology records and no
//!   context or trimmed surface cache: the B-rep keeps none of them.
//! - **Unported callees.** The `POST_EDIT` notification's `SmFace::GetUpwardEdgeuses`
//!   (`topology.face_p4`) returns [`SolidError::Unported`]; `SmEdgeuse::GetOrCreateUVTrimCurve`
//!   is wired (`topology.edgeuse_p3`, whose own deferred callees still return it).
//!   `SmShell::GetRegion` is the co-port
//!   [`eu1::shell_region`].
//! - **Collections.** Ordered membership sets deduplicate edges and vertices
//!   while the result vector preserves first occurrence and filter evaluation
//!   order, avoiding quadratic `AddUnique` scans on long loops (#431).
//! - **Prepared queries.** The shared UV-corner walk can also borrow prepared topology
//!   without mutation. It reports [`SolidError::Unported`] only if a trim curve needs
//!   creating at the point where the mutable walk would create it.
//! - **UV-corner failures (WR-262, #521).** By default `GetUVCornerBoundaries` ignores
//!   failed endpoint evaluations and point drops, as upstream (an edge use can reuse
//!   the preceding one's endpoints). With the opt-in
//!   `Kernel::fixes.topology_failed_evaluations_propagate` they propagate and an
//!   unsuccessful drop returns `SM_ERR`.
//! - The `std::unordered_set` overloads of `GetEdges` / `GetVertices`,
//!   `GetEdgeusesAndVertexuses`, `CalculateBoundingBox`, `GetTurningAngDeg`,
//!   `SmLoop::GetMaxSameDimGap3d`, `IsLamina`, `IsManifold`,
//!   `IsEmbedded`, `ContainsUVPoint`, `RebuildFromEdgeCurves`, `AssertValid`, `Draw`
//!   and `Dump` are not part of this unit.

use std::collections::BTreeSet;
use std::ops::Deref;

use crate::curve::curve::Curve;
use crate::error::{EntityRef, Result, SolidError};
use crate::geom::attribute::{AttributeStore, ObjectKey};
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::{Extent2d, SS_NONE};
use crate::geom::object::{NotifyOperation, ObjectRef};
use crate::geom::tol;
use crate::geom::vector2d::Point2;
use crate::kernel::Kernel;
use crate::math::{Point3, Vec3};
use crate::nurbs::b_spline_surface::p1::SurfParamType;
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::surface::surface::Surface;
use crate::topology::edge::NotifyEnv;
use crate::topology::edgeuse::p1 as eu1;
use crate::topology::face::p3 as face3;
use crate::topology::topology as topo;
use crate::topology::vertexuse as vu1;
use crate::topology::{
    Brep, EdgeId, EdgeuseId, EdgeuseOwner, FaceId, FaceuseId, LoopId, LoopuseId, LoopuseStart,
    ShellId, VertexId, VertexuseId, VertexuseOwner,
};

const OP: &str = "topology::loopuse";

/// Topology access shared by mutable queries and queries over prepared UV trims.
/// Only the mutable view may create a missing trim, at the original lazy call site.
pub(crate) enum QueryTopology<'a> {
    /// The ordinary query path, which may attach missing UV trim curves lazily.
    Mutable(&'a mut Brep),
    /// A read-only query; missing trims are reported only when actually needed.
    Prepared(&'a Brep),
    /// A read-only solve that can ask its caller to create an actually visited trim.
    Requested(&'a Brep, &'a crate::query::brep::RayPreparation),
}

impl Deref for QueryTopology<'_> {
    type Target = Brep;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Mutable(b) => b,
            Self::Prepared(b) | Self::Requested(b, _) => b,
        }
    }
}

impl QueryTopology<'_> {
    /// `GetOrCreateUVTrimCurve` with the flags used by the containment/corner walks.
    /// The result tests both its status and the presence of the returned curve;
    /// callers that ignore the upstream status may ignore this boolean too.
    pub(crate) fn ensure_trim_curve(&mut self, k: &Kernel, eu: EdgeuseId) -> Result<bool> {
        match self {
            Self::Mutable(b) => {
                let fetch = crate::topology::edgeuse::p3::get_or_create_uv_trim_curve(
                    k, b, eu, false, false, true,
                )?;
                Ok(fetch.status.is_ok() && eu1::uv_trim_curve(b, eu)?.0.is_some())
            }
            Self::Prepared(b) => prepared_trim(b, eu, None),
            Self::Requested(b, preparation) => prepared_trim(b, eu, Some(preparation)),
        }
    }
}

fn prepared_trim(
    b: &Brep,
    eu: EdgeuseId,
    preparation: Option<&crate::query::brep::RayPreparation>,
) -> Result<bool> {
    if preparation.is_some_and(|preparation| preparation.trim_failed(eu)) {
        return Ok(false);
    }
    if eu1::uv_trim_curve(b, eu)?.0.is_none() {
        if let Some(preparation) = preparation {
            preparation.request_trim(b, eu);
            return Err(SolidError::PreparationRequired {
                operation: "topology::loopuse::prepared_trim",
            });
        }
        return Err(SolidError::unported(
            "SmEdgeuse::GetOrCreateUVTrimCurve (unprepared read-only topology)",
            "topology.edgeuse_p3",
        ));
    }
    // The use's own failure record applies even when its mate owns the curve.
    Ok(b.edgeuse(eu)?
        .drop_curve_fail
        .as_deref()
        .is_none_or(|f| f.fail_status == crate::intersect::surface_drop_lib::p1::SM_SUCCESS))
}

/// `SmLoopuse_TYPE` (`TOPO_BASE_TYPE + 4`).
pub const TYPE: i64 = 16004;
/// `SmLoopuse::GetTypeString`.
pub const TYPE_STRING: &str = "SmLoopuse_TYPE";
/// `SmLoopuse::GetClassString`.
pub const CLASS_STRING: &str = "SmLoopuse";

/// `SmLoop_TYPE` (`TOPO_BASE_TYPE + 3`).
pub const LOOP_TYPE: i64 = 16003;
/// `SmLoop::GetTypeString`.
pub const LOOP_TYPE_STRING: &str = "SmLoop_TYPE";
/// `SmLoop::GetClassString`.
pub const LOOP_CLASS_STRING: &str = "SmLoop";

/// `SmEdgeuse_TYPE`, a [`loopuse_type`].
pub const EDGEUSE_TYPE: i64 = 16005;
/// `SmVertexuse_TYPE`, a [`loopuse_type`].
pub const VERTEXUSE_TYPE: i64 = 16006;
/// `SmUnknown_TYPE`, the [`loopuse_type`] of a loop use not yet built.
pub const UNKNOWN_TYPE: i64 = 16999;

/// `CollectEdgeuses`' bound on the CCW walk ("It is highly unlikely that we will
/// ever have more than 100k edges").
const MAX_COLLECT_STEPS: u64 = 100_000;

/// `pOptAttributeId`: whether an entity holds an attribute with the requested id
/// (upstream `FindAttribute(id) != NULL`).
pub type AttributeFilter<'a> = &'a dyn Fn(EntityRef) -> bool;

/// `SmLoopuse::IsKindOf`: [`TYPE`], then the `SmTopology` chain.
#[must_use]
pub const fn is_kind_of(t: i64) -> bool {
    if t == TYPE {
        true
    } else {
        topo::is_kind_of(t)
    }
}

/// `SmLoop::GetClassType` (static): [`LOOP_TYPE`].
#[must_use]
pub const fn loop_class_type() -> i64 {
    LOOP_TYPE
}

/// `SmLoop::IsKindOf`: [`LOOP_TYPE`], then the `SmTopology` chain.
#[must_use]
pub const fn loop_is_kind_of(t: i64) -> bool {
    if t == LOOP_TYPE {
        true
    } else {
        topo::is_kind_of(t)
    }
}

fn null(reason: &'static str) -> SolidError {
    SolidError::invalid_input(OP, reason)
}

// ---- loop use accessors ---------------------------------------------------------------

/// `GetVertexuse`: the vertex use of a vertex loop use, `None` for the other kinds.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn vertexuse(b: &Brep, lu: LoopuseId) -> Result<Option<VertexuseId>> {
    Ok(match b.loopuse(lu)?.start {
        LoopuseStart::Vertexuse(vu) => vu,
        _ => None,
    })
}

/// `GetFaceuse`: the owning face use (`m_pListOwner`).
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn faceuse(b: &Brep, lu: LoopuseId) -> Result<Option<FaceuseId>> {
    Ok(b.loopuse(lu)?.faceuse)
}

/// `GetLoop`: the loop used.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn loop_(b: &Brep, lu: LoopuseId) -> Result<Option<LoopId>> {
    Ok(b.loopuse(lu)?.loop_)
}

/// `GetOrientation`: [`OrientType::Same`] for an outer loop use,
/// [`OrientType::Opposite`] for an inner one.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn orientation(b: &Brep, lu: LoopuseId) -> Result<OrientType> {
    Ok(b.loopuse(lu)?.orientation)
}

/// `GetOtherLoopuse`: the loop use on the face's other side.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn other_loopuse(b: &Brep, lu: LoopuseId) -> Result<Option<LoopuseId>> {
    Ok(b.loopuse(lu)?.mate)
}

/// `GetEUorVU`: the start edge use of an edge loop use or the vertex use of a vertex
/// loop use; `None` when there is none.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn eu_or_vu(b: &Brep, lu: LoopuseId) -> Result<Option<EntityRef>> {
    Ok(match b.loopuse(lu)?.start {
        LoopuseStart::Unknown => None,
        LoopuseStart::Edgeuse(eu) => eu.map(EntityRef::Edgeuse),
        LoopuseStart::Vertexuse(vu) => vu.map(EntityRef::Vertexuse),
    })
}

/// `GetLoopuseType`: [`EDGEUSE_TYPE`], [`VERTEXUSE_TYPE`] or [`UNKNOWN_TYPE`].
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn loopuse_type(b: &Brep, lu: LoopuseId) -> Result<i64> {
    Ok(match b.loopuse(lu)?.start {
        LoopuseStart::Unknown => UNKNOWN_TYPE,
        LoopuseStart::Edgeuse(_) => EDGEUSE_TYPE,
        LoopuseStart::Vertexuse(_) => VERTEXUSE_TYPE,
    })
}

/// `IsVertexLoopuse`: the loop is a single vertex.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn is_vertex_loopuse(b: &Brep, lu: LoopuseId) -> Result<bool> {
    Ok(matches!(b.loopuse(lu)?.start, LoopuseStart::Vertexuse(_)))
}

/// `IsEdgeLoopuse`: the loop is a chain of edge uses.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn is_edge_loopuse(b: &Brep, lu: LoopuseId) -> Result<bool> {
    Ok(matches!(b.loopuse(lu)?.start, LoopuseStart::Edgeuse(_)))
}

/// `SetOrientation`.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn set_orientation(b: &mut Brep, lu: LoopuseId, orientation: OrientType) -> Result<()> {
    b.loopuse_mut(lu)?.orientation = orientation;
    Ok(())
}

/// `SetLoop` (its `SM_ASSERT` that a loop is not replaced is compiled out).
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn set_loop(b: &mut Brep, lu: LoopuseId, l: Option<LoopId>) -> Result<()> {
    b.loopuse_mut(lu)?.loop_ = l;
    Ok(())
}

/// `GetShell`: the shell of the owning face use; `None` without face use.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn shell(b: &Brep, lu: LoopuseId) -> Result<Option<ShellId>> {
    match b.loopuse(lu)?.faceuse {
        Some(fu) => eu1::faceuse_shell(b, fu),
        None => Ok(None),
    }
}

/// `SetEdgeuse`: makes `eu` the start of the edge loop (cycling the loop's
/// definition); `SM_ERR` unless this is an edge loop use and `eu` one of its edge uses.
///
/// # Errors
///
/// [`SolidError::Upstream`] (`SM_ERR`) as above; [`SolidError::NotFound`] for a
/// dangling id.
pub fn set_edgeuse(b: &mut Brep, lu: LoopuseId, eu: Option<EdgeuseId>) -> Result<()> {
    if is_edge_loopuse(b, lu)? {
        if let Some(eu) = eu {
            if eu1::loopuse(b, eu)? == Some(lu) {
                b.loopuse_mut(lu)?.start = LoopuseStart::Edgeuse(Some(eu));
                return Ok(());
            }
        }
    }
    Err(SolidError::sm_err(OP))
}

/// `CollectEdgeuses`: makes the loop use an edge loop use starting at `eu` and walks
/// `eu`'s CCW list, making each edge use a loop edge use of this loop use, until it
/// returns to `eu`.
///
/// # Errors
///
/// [`SolidError::Upstream`] `SM_ERR_NULL_POINTER` for a null `eu` or a CCW list that
/// ends (the edge uses walked so far stay updated) and `SM_ERR` after
/// 100000 steps; [`SolidError::NotFound`] for a dangling id.
pub fn collect_edgeuses(b: &mut Brep, lu: LoopuseId, eu: Option<EdgeuseId>) -> Result<()> {
    // check state - input must be NonNULL
    let start = eu.ok_or_else(|| SolidError::null_pointer(OP))?;

    // local
    let mut count: u64 = 0;

    // Set Loopuse's 1st Edgeuse m_pEUorVU BackPtr and Type to Tgt Edgeuse
    b.loopuse_mut(lu)?.start = LoopuseStart::Edgeuse(Some(start));

    // for every sibling Edgeuse
    let mut cur = Some(start);
    loop {
        // check state - Edgeuse must be NonNULL
        let Some(this) = cur else {
            return Err(SolidError::null_pointer(OP));
        };

        // set pEdgeuse useType to SmLoopuse_TYPE and its BackPtr to this Loopuse
        let r = b.edgeuse_mut(this)?;
        r.owner = EdgeuseOwner::Loopuse(Some(lu));

        // quit when finished walking the circular linked list
        if r.ccw == Some(start) {
            break;
        }

        // increment to the next sibling Edgeuse
        cur = r.ccw;

        // reality check - error past 100k edges
        count += 1;
        if count > MAX_COLLECT_STEPS {
            return Err(SolidError::sm_err(OP));
        }
    }

    // all done
    Ok(())
}

// ---- CCW walks ------------------------------------------------------------------------

/// Where the CCW walks of a loop use start.
enum Walk {
    /// A vertex loop use: no walk.
    Vertex,
    /// An edge loop use (or an unbuilt one, whose `m_pEUorVU` is NULL): the walk from
    /// this start edge use (none: an empty walk).
    Edges(Option<EdgeuseId>),
}

/// The start of the CCW walks of `lu`.
fn walk_start(b: &Brep, lu: LoopuseId) -> Result<Walk> {
    Ok(match b.loopuse(lu)?.start {
        LoopuseStart::Vertexuse(_) => Walk::Vertex,
        LoopuseStart::Edgeuse(eu) => Walk::Edges(eu),
        // `m_pEUorVU` of an unbuilt loop use is NULL (`SM_CAST_PTR` of NULL is NULL)
        LoopuseStart::Unknown => Walk::Edges(None),
    })
}

/// The edge uses of the CCW walk from `start` until it returns to `start` (a null
/// link ends the walk).
fn ccw_walk(b: &Brep, start: Option<EdgeuseId>) -> Result<Vec<EdgeuseId>> {
    let mut out = Vec::new();
    let mut eu = start;
    loop {
        // for NULL terminated lists
        let Some(cur) = eu else {
            return Ok(out);
        };
        if out.len() > b.edgeuses().len() {
            return Err(null("loop use CCW chain does not close"));
        }
        out.push(cur);

        // next iter
        eu = b.edgeuse(cur)?.ccw;

        // quit when we get back to the Starting Edgeuse
        if eu == start {
            return Ok(out);
        }
    }
}

fn keep(filter: Option<AttributeFilter<'_>>, e: EntityRef) -> bool {
    filter.is_none_or(|f| f(e))
}

/// `GetEdges` (`SmTArray` overload): the edges of the loop's edge uses in CCW order,
/// each listed once (`AddUnique`; a null edge is listed once too); with `attribute`
/// only those holding the attribute. Empty for a vertex loop use.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for a null edge tested against `attribute` or a CCW
/// chain that does not close; [`SolidError::NotFound`] for a dangling id.
pub fn edges(
    b: &Brep,
    lu: LoopuseId,
    attribute: Option<AttributeFilter<'_>>,
) -> Result<Vec<Option<EdgeId>>> {
    let Walk::Edges(start) = walk_start(b, lu)? else {
        return Ok(Vec::new());
    };
    let mut out: Vec<Option<EdgeId>> = Vec::new();
    let mut seen = BTreeSet::new();
    for eu in ccw_walk(b, start)? {
        let e = b.edgeuse(eu)?.edge;
        let ok = match (attribute, e) {
            (None, _) => true,
            (Some(f), Some(e)) => f(EntityRef::Edge(e)),
            (Some(_), None) => return Err(null("GetEdges: FindAttribute on a null edge")),
        };
        if ok && seen.insert(e) {
            out.push(e);
        }
    }
    Ok(out)
}

/// `GetEdgeuses`: the loop's edge uses in CCW order (an edge used twice is listed
/// twice); with `attribute` only those holding the attribute, with `connected_to`
/// only those [`eu1::is_connected_to`] it. Empty for a vertex loop use.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for a CCW chain that does not close; those of
/// [`eu1::is_connected_to`]; [`SolidError::NotFound`] for a dangling id.
pub fn edgeuses(
    b: &Brep,
    lu: LoopuseId,
    attribute: Option<AttributeFilter<'_>>,
    connected_to: Option<EntityRef>,
) -> Result<Vec<EdgeuseId>> {
    let Walk::Edges(start) = walk_start(b, lu)? else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for eu in ccw_walk(b, start)? {
        if keep(attribute, EntityRef::Edgeuse(eu))
            && (connected_to.is_none() || eu1::is_connected_to(b, eu, connected_to)?)
        {
            out.push(eu);
        }
    }
    Ok(out)
}

/// `GetVertexuses`: the vertex use of a vertex loop use, or the start vertex uses of
/// the loop's edge uses in CCW order (null ones included); with `attribute` only those
/// holding the attribute.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for a null vertex use tested against `attribute` or a
/// CCW chain that does not close; [`SolidError::NotFound`] for a dangling id.
pub fn vertexuses(
    b: &Brep,
    lu: LoopuseId,
    attribute: Option<AttributeFilter<'_>>,
) -> Result<Vec<Option<VertexuseId>>> {
    let test = |vu: Option<VertexuseId>| -> Result<bool> {
        match (attribute, vu) {
            (None, _) => Ok(true),
            (Some(f), Some(vu)) => Ok(f(EntityRef::Vertexuse(vu))),
            (Some(_), None) => Err(null("GetVertexuses: FindAttribute on a null vertex use")),
        }
    };
    let Walk::Edges(start) = walk_start(b, lu)? else {
        // get the only Vertexuse
        let vu = vertexuse(b, lu)?;
        return Ok(if test(vu)? { vec![vu] } else { Vec::new() });
    };
    let mut out = Vec::new();
    for eu in ccw_walk(b, start)? {
        let vu = b.edgeuse(eu)?.vertexuse;
        if test(vu)? {
            out.push(vu);
        }
    }
    Ok(out)
}

/// `GetVertices`: the vertex of a vertex loop use, or the vertices of the loop's edge
/// uses' start vertex uses in CCW order, each listed once (`AddUnique`); with
/// `attribute` only those holding the attribute.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for a null vertex use (upstream dereferences it), a
/// null vertex tested against `attribute` or a CCW chain that does not close;
/// [`SolidError::NotFound`] for a dangling id.
pub fn vertices(
    b: &Brep,
    lu: LoopuseId,
    attribute: Option<AttributeFilter<'_>>,
) -> Result<Vec<Option<VertexId>>> {
    let vertex_of = |vu: Option<VertexuseId>| -> Result<Option<VertexId>> {
        let vu = vu.ok_or_else(|| null("GetVertices: null vertex use"))?;
        Ok(b.vertexuse(vu)?.vertex)
    };
    let test = |v: Option<VertexId>| -> Result<bool> {
        match (attribute, v) {
            (None, _) => Ok(true),
            (Some(f), Some(v)) => Ok(f(EntityRef::Vertex(v))),
            (Some(_), None) => Err(null("GetVertices: FindAttribute on a null vertex")),
        }
    };
    let Walk::Edges(start) = walk_start(b, lu)? else {
        // get the only Vertex (listed with Add, not AddUnique)
        let v = vertex_of(vertexuse(b, lu)?)?;
        return Ok(if test(v)? { vec![v] } else { Vec::new() });
    };
    let mut out: Vec<Option<VertexId>> = Vec::new();
    let mut seen = BTreeSet::new();
    for eu in ccw_walk(b, start)? {
        let v = vertex_of(b.edgeuse(eu)?.vertexuse)?;
        if test(v)? && seen.insert(v) {
            out.push(v);
        }
    }
    Ok(out)
}

// ---- predicates -----------------------------------------------------------------------

/// The result of [`is_closed_3d`] (`IsClosed3d`'s return value and out-params).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Closed3d {
    /// The return value: no end-to-end gap exceeds its cross-section tolerance.
    pub closed: bool,
    /// `ppOptEdgeuse`: the edge use starting the reported gap (`None` for no gap).
    pub edgeuse: Option<EdgeuseId>,
    /// `pdOptMaxGap`: the reported gap's length.
    pub max_gap: f64,
    /// `psOptXSectTol3d`: the reported gap's cross-section tolerance.
    pub xsect_tol_3d: f64,
}

/// The 3D point of an edge use's end (`start` true: its start) on its edge curve;
/// `prev` when the evaluation fails (its status is ignored upstream).
fn edgeuse_end_point(
    k: &Kernel,
    b: &Brep,
    eu: EdgeuseId,
    start: bool,
    prev: Point3,
) -> Result<Point3> {
    let r = b.edgeuse(eu)?;
    let e = r
        .edge
        .ok_or_else(|| null("IsClosed3d: edge use without edge"))?;
    let edge = b.edge(e)?;
    let curve: &Curve = edge
        .curve
        .as_ref()
        .ok_or_else(|| null("IsClosed3d: edge without curve"))?;
    let ivl = edge.interval;
    let same = r.orientation == OrientType::Same;
    let t = if same == start { ivl.min() } else { ivl.max() };
    match curve.evaluate_point(k, t) {
        Ok(p) => Ok(p),
        Err(e) if e.is_unported() => Err(e),
        Err(_) => Ok(prev),
    }
}

/// `IsClosed3d`: whether every gap between an edge use's end and its CCW neighbour's
/// start is within their cross-section tolerance (`ZoneTol3d + ZoneTol3d`), with the
/// gap reported; `true` for a vertex loop use (outputs zero). See the module docs for
/// upstream's over-tolerance test, which never fires.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for an edge use without edge or an edge without curve
/// (upstream dereferences them) or a CCW chain that does not close;
/// [`SolidError::Unported`] from the curve evaluation; [`SolidError::NotFound`] for a
/// dangling id.
pub fn is_closed_3d(k: &Kernel, b: &Brep, lu: LoopuseId, check_all_gaps: bool) -> Result<Closed3d> {
    // init output / no work - no Edgeuses
    if is_vertex_loopuse(b, lu)? {
        return Ok(Closed3d {
            closed: true,
            edgeuse: None,
            max_gap: 0.0,
            xsect_tol_3d: 0.0,
        });
    }

    // locals
    let mut this_end_point = Vec3::UNDEF;
    let mut next_start_point = Vec3::UNDEF;
    let mut max_gap_edgeuse: Option<EdgeuseId> = None;
    let mut max_gap_sq = 0.0;
    let mut max_gap_xsect_tol_3d = 0.0;

    let mut max_over_tol_edgeuse: Option<EdgeuseId> = None;
    let mut max_over_tol = 0.0;
    let mut max_over_tol_xsect_tol_3d = 0.0;

    let eus = edgeuses(b, lu, None, None)?;
    let num = eus.len();

    // for every Edgeuse - check for EdgeuseEnd-EdgeuseEnd gap sizes
    for (ii, &this_eu) in eus.iter().enumerate() {
        // This Edgeuse End Point
        let this_zone_tol_3d = tol::zone_tol3d(k, eu1::tolerance(b, this_eu)?);
        this_end_point = edgeuse_end_point(k, b, this_eu, false, this_end_point)?;

        // Next Edgeuse Start Point
        let next_eu = eus
            .get((ii + 1) % num)
            .copied()
            .ok_or_else(|| null("IsClosed3d: index"))?;
        let next_zone_tol_3d = tol::zone_tol3d(k, eu1::tolerance(b, next_eu)?);
        next_start_point = edgeuse_end_point(k, b, next_eu, true, next_start_point)?;

        // ThisEndPoint to NextStartPoint gap
        let gap_3d_sq = this_end_point.distance_squared(next_start_point);

        // EdgeEnd/EdgeEnd tolerance
        let xsect_tol_3d = tol::xsect_tol_3d_from_zones(k, this_zone_tol_3d, next_zone_tol_3d);
        let xsect_tol_3d_sq = tol::tol3d(xsect_tol_3d * xsect_tol_3d);

        // When Gap is largest - save maxGap values
        if gap_3d_sq > max_gap_sq {
            max_gap_edgeuse = Some(this_eu);
            max_gap_sq = gap_3d_sq;
            max_gap_xsect_tol_3d = xsect_tol_3d;
        }

        // When gap is bigger than XSectTol3d(ThisEdgeuse, NextEdgeuse)
        if gap_3d_sq > xsect_tol_3d_sq {
            let over_tol = xsect_tol_3d - gap_3d_sq.sqrt();

            // when OverTol is largest - save MaxOvertTol vals
            if over_tol > max_over_tol {
                max_over_tol_edgeuse = Some(this_eu);
                max_over_tol = over_tol;
                max_over_tol_xsect_tol_3d = xsect_tol_3d;
            }

            // when asked to quit early - quit
            if !check_all_gaps {
                break;
            }
        }
    }

    // set output
    let closed = max_over_tol_edgeuse.is_none();
    Ok(if closed {
        Closed3d {
            closed,
            edgeuse: max_gap_edgeuse,
            max_gap: max_gap_sq.sqrt(),
            xsect_tol_3d: max_gap_xsect_tol_3d,
        }
    } else {
        Closed3d {
            closed,
            edgeuse: max_over_tol_edgeuse,
            max_gap: max_over_tol + max_over_tol_xsect_tol_3d,
            xsect_tol_3d: max_over_tol_xsect_tol_3d,
        }
    })
}

/// `IsClosedPtrs`: whether the CCW links of the loop's edge uses return to the start
/// (`false` at a null link); `true` for a vertex loop use.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for a CCW chain that neither ends nor returns to its
/// start; [`SolidError::NotFound`] for a dangling id.
pub fn is_closed_ptrs(b: &Brep, lu: LoopuseId) -> Result<bool> {
    // no work - no Edgeuses
    let Walk::Edges(start) = walk_start(b, lu)? else {
        return Ok(true);
    };
    let mut eu = start;
    let mut visited = 0usize;
    loop {
        // for NULL terminated lists
        let Some(cur) = eu else {
            return Ok(false);
        };
        visited += 1;
        if visited > b.edgeuses().len() + 1 {
            return Err(null("IsClosedPtrs: CCW chain does not close"));
        }

        // next iter
        eu = b.edgeuse(cur)?.ccw;

        // quit when we get back to the Starting Edgeuse
        if eu == start {
            return Ok(true);
        }
    }
}

/// `SmVertex::IsConnectedTo` (unit `topology.vertex`).
fn vertex_is_connected_to(b: &Brep, v: VertexId, target: EntityRef) -> Result<bool> {
    crate::topology::vertex::is_connected_to(b, v, Some(target))
}

/// `SmEdge::IsConnectedTo` (unit `topology.edge`).
fn edge_is_connected_to(b: &Brep, e: EdgeId, target: EntityRef) -> Result<bool> {
    crate::topology::edge::is_connected_to(b, e, Some(target))
}

/// `IsConnectedTo`: whether the loop use connects to `target` (its vertex uses,
/// vertices, edge uses, edges, face, face use, loop, itself, shell or region); `true`
/// for `None` and any other kind.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for a face target and a loop use without face use;
/// those of the target's own `IsConnectedTo`; [`SolidError::NotFound`] for a dangling id.
pub fn is_connected_to(b: &Brep, lu: LoopuseId, target: Option<EntityRef>) -> Result<bool> {
    // no work - no Topo
    let Some(target) = target else {
        return Ok(true);
    };
    let this = EntityRef::Loopuse(lu);
    match target {
        EntityRef::Vertexuse(vu) => vu1::is_connected_to(b, vu, Some(this)),
        EntityRef::Vertex(v) => vertex_is_connected_to(b, v, this),
        EntityRef::Edgeuse(eu) => eu1::is_connected_to(b, eu, Some(this)),
        EntityRef::Edge(e) => edge_is_connected_to(b, e, this),
        EntityRef::Face(f) => {
            let fu = faceuse(b, lu)?.ok_or_else(|| null("IsConnectedTo: no face use"))?;
            Ok(eu1::faceuse_face(b, fu)? == Some(f))
        }
        EntityRef::Faceuse(fu) => Ok(faceuse(b, lu)? == Some(fu)),
        EntityRef::Loop(l) => Ok(loop_(b, lu)? == Some(l)),
        EntityRef::Loopuse(other) => Ok(other == lu),
        EntityRef::Shell(s) => Ok(shell(b, lu)? == Some(s)),
        EntityRef::Region(r) => match shell(b, lu)? {
            Some(s) => Ok(eu1::shell_region(b, s)? == Some(r)),
            None => Ok(false),
        },
        // arrive here when the target is an unsupported type - return TRUE
        EntityRef::Brep(_) => Ok(true),
    }
}

/// `IsMissingSeamAtVertices`: whether the vertex sector of any of the loop's vertex
/// uses spans a seam of the face's surface that no seam edge marks; `false` without
/// surface or for a surface closed in neither direction (over its natural domain).
///
/// # Errors
///
/// Those of [`vu1::compute_uv_sector`] (made with `bOKToMakeUVTrimCurve = TRUE`) and
/// of the closure tests; [`SolidError::NotFound`] for a dangling id.
pub fn is_missing_seam_at_vertices(k: &Kernel, b: &mut Brep, lu: LoopuseId) -> Result<bool> {
    // locals
    let surface: Option<&Surface> = match faceuse(b, lu)? {
        Some(fu) => match eu1::faceuse_face(b, fu)? {
            Some(f) => b.face(f)?.surface.as_ref(),
            None => None,
        },
        None => None,
    };

    // low work - no surface or surface not closed
    let Some(surface) = surface else {
        return Ok(false);
    };
    let dom = surface.natural_uv_domain_with_nurb(k)?;
    if !surface.is_closed(k, &dom, SurfParamType::U, None, false)?.0
        && !surface.is_closed(k, &dom, SurfParamType::V, None, false)?.0
    {
        return Ok(false);
    }

    // for every Vertexuse
    for vu in vertexuses(b, lu, None)? {
        // skip NULL Vertexuses - should never happen
        let Some(vu) = vu else {
            continue;
        };

        // sector properties (status ignored)
        let sector = vu1::compute_uv_sector(k, b, vu, true)?;

        // check for missing Seam
        if sector.io.sector_missing_seam_u || sector.io.sector_missing_seam_v {
            return Ok(true);
        }
    }

    // all done
    Ok(false)
}

// ---- loops ----------------------------------------------------------------------------

/// `GetLoopuse`: the primary loop use (`m_pLU`).
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn loop_loopuse(b: &Brep, l: LoopId) -> Result<Option<LoopuseId>> {
    Ok(b.loop_(l)?.loopuse)
}

/// The primary loop use, which upstream dereferences.
fn primary(b: &Brep, l: LoopId) -> Result<LoopuseId> {
    loop_loopuse(b, l)?.ok_or_else(|| null("SmLoop: loop without loop use"))
}

/// `GetFace`: the face of the primary loop use's face use.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for a loop without loop use or a loop use without face
/// use; [`SolidError::NotFound`] for a dangling id.
pub fn loop_face(b: &Brep, l: LoopId) -> Result<Option<FaceId>> {
    let fu = faceuse(b, primary(b, l)?)?.ok_or_else(|| null("SmLoop::GetFace: no face use"))?;
    eu1::faceuse_face(b, fu)
}

/// `GetLoopuses`: the primary loop use and its mate.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for a loop without loop use; [`SolidError::NotFound`]
/// for a dangling id.
pub fn loop_loopuses(b: &Brep, l: LoopId) -> Result<(LoopuseId, Option<LoopuseId>)> {
    let lu = primary(b, l)?;
    Ok((lu, other_loopuse(b, lu)?))
}

/// `GetVertices`: [`vertices`] of the primary loop use (no filter).
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for a loop without loop use; those of [`vertices`].
pub fn loop_vertices(b: &Brep, l: LoopId) -> Result<Vec<Option<VertexId>>> {
    vertices(b, primary(b, l)?, None)
}

/// `GetEdges`: [`edges`] of the primary loop use (no filter).
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for a loop without loop use; those of [`edges`].
pub fn loop_edges(b: &Brep, l: LoopId) -> Result<Vec<Option<EdgeId>>> {
    edges(b, primary(b, l)?, None)
}

/// `GetEdgeuses`: [`edgeuses`] of the primary loop use (no filter).
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for a loop without loop use; those of [`edgeuses`].
pub fn loop_edgeuses(b: &Brep, l: LoopId) -> Result<Vec<EdgeuseId>> {
    edgeuses(b, primary(b, l)?, None, None)
}

/// The identity a topology entity is notified under by [`face_notify_edit`]: its type
/// code and arena index (the B-rep keeps no attributes for it to select).
pub(crate) fn notify_ref(e: EntityRef) -> ObjectRef {
    let (type_code, kind, index) = match e {
        EntityRef::Brep(x) => (16011, 0u64, x.index()),
        EntityRef::Region(x) => (16012, 1, x.index()),
        EntityRef::Shell(x) => (16013, 2, x.index()),
        EntityRef::Faceuse(x) => (16022, 3, x.index()),
        EntityRef::Face(x) => (16002, 4, x.index()),
        EntityRef::Loopuse(x) => (16004, 5, x.index()),
        EntityRef::Loop(x) => (16003, 6, x.index()),
        EntityRef::Edgeuse(x) => (16005, 7, x.index()),
        EntityRef::Edge(x) => (16016, 8, x.index()),
        EntityRef::Vertexuse(x) => (16006, 9, x.index()),
        EntityRef::Vertex(x) => (16021, 10, x.index()),
    };
    ObjectRef {
        type_code,
        key: ObjectKey(1 + (kind << 32) + u64::from(index)),
    }
}

/// The entity [`notify_ref`] gives the key `key`, if any (its inverse on keys). A
/// caller decoding notification keys relies on every event of the edit being keyed by
/// [`notify_ref`], as [`ArenaNotifyState`](crate::topology::brep::p2::ArenaNotifyState)
/// does; a change of that scheme changes this decoder with it. (The heal path's
/// tracker now decodes kernel identities, `topology::brep::p3::topology_entity`, since
/// part 5's plain edits run under them; this inverse also names the B-rep-owned
/// attribute store's keys, [`Brep::attributed_entities`](crate::topology::Brep::attributed_entities).)
pub(crate) fn entity_of_notify_key(key: ObjectKey) -> Option<EntityRef> {
    let raw = key.0.checked_sub(1)?;
    let kind = raw >> 32;
    let index = u32::try_from(raw & u64::from(u32::MAX)).ok()?;
    Some(match kind {
        0 => EntityRef::Brep(crate::topology::BrepId::from_index(index)),
        1 => EntityRef::Region(crate::topology::RegionId::from_index(index)),
        2 => EntityRef::Shell(ShellId::from_index(index)),
        3 => EntityRef::Faceuse(FaceuseId::from_index(index)),
        4 => EntityRef::Face(FaceId::from_index(index)),
        5 => EntityRef::Loopuse(LoopuseId::from_index(index)),
        6 => EntityRef::Loop(LoopId::from_index(index)),
        7 => EntityRef::Edgeuse(EdgeuseId::from_index(index)),
        8 => EntityRef::Edge(EdgeId::from_index(index)),
        9 => EntityRef::Vertexuse(VertexuseId::from_index(index)),
        10 => EntityRef::Vertex(VertexId::from_index(index)),
        _ => return None,
    })
}

/// `SmFace::Notify(SM_NO_PRE_EDIT / SM_NO_POST_EDIT, pFace, SM_NO_GET_BREP(pFace) (0), NULL)`: the
/// face itself is the first argument. The B-rep
/// carries no attribute store, topology records, context callbacks or trimmed surface
/// caches, so the notification runs with empty ones (upstream on a face and uses
/// without attributes or cache).
pub(crate) fn face_notify_edit(b: &mut Brep, f: FaceId, operation: NotifyOperation) -> Result<()> {
    face_notify_as(b, f, notify_ref(EntityRef::Face(f)), operation)
}

/// `SmFace::Notify(op, data1, data2, data3)` on face `f` with the entities as data
/// (`None` for a null datum), under the same empty environment as [`face_notify_edit`].
pub(crate) fn face_notify_data(
    b: &mut Brep,
    f: FaceId,
    operation: NotifyOperation,
    data: [Option<EntityRef>; 3],
) -> Result<()> {
    let mut records = crate::topology::topology::NotifyRecords::new();
    let mut attributes = AttributeStore::default();
    let key_of = |e: EntityRef| notify_ref(e);
    let mut env = NotifyEnv {
        records: &mut records,
        attributes: &mut attributes,
        context: None,
        key_of: &key_of,
    };
    let data = data.map(|d| d.map(notify_ref));
    face3::notify(b, f, &mut env, None, operation, data)
}

/// [`face_notify_edit`] for a face notified under the identity `this` (the caller's
/// `this` pointer, which is the notification's first data argument).
pub(crate) fn face_notify_as(
    b: &mut Brep,
    f: FaceId,
    this: ObjectRef,
    operation: NotifyOperation,
) -> Result<()> {
    let mut records = crate::topology::topology::NotifyRecords::new();
    let mut attributes = AttributeStore::default();
    let key_of = |e: EntityRef| {
        if e == EntityRef::Face(f) {
            this
        } else {
            notify_ref(e)
        }
    };
    let mut env = NotifyEnv {
        records: &mut records,
        attributes: &mut attributes,
        context: None,
        key_of: &key_of,
    };
    face3::notify(b, f, &mut env, None, operation, [Some(this), None, None])
}

/// `FlipLoopOrientation`: swaps the starts of the loop's two loop uses (the vertex
/// uses of a vertex loop, with their owners; the edge use lists of an edge loop,
/// re-collected), notifying the face before and after.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for a loop without loop use or mates of different
/// kinds (see the module docs); [`SolidError::Upstream`] `SM_ERR_NULL_POINTER` for a
/// missing mate or start; those of [`collect_edgeuses`]; [`SolidError::Unported`] for
/// a loop on a face (the post-edit notification, `topology.face_p4`);
/// [`SolidError::NotFound`] for a dangling id.
pub fn flip_loop_orientation(b: &mut Brep, l: LoopId) -> Result<()> {
    let (lu1, lu2) = loop_loopuses(b, l)?;
    let lu2 = lu2.ok_or_else(|| SolidError::null_pointer(OP))?;
    let face = match faceuse(b, lu1)? {
        Some(fu) => eu1::faceuse_face(b, fu)?,
        None => None,
    };
    if let Some(f) = face {
        face_notify_edit(b, f, NotifyOperation::PreEdit)?;
    }

    let start1 = b.loopuse(lu1)?.start;
    let start2 = b.loopuse(lu2)?.start;
    if let LoopuseStart::Vertexuse(vu1) = start1 {
        // get Vertexuses for both mated Loopuses
        let vu1 = vu1.ok_or_else(|| SolidError::null_pointer(OP))?;
        let vu2 = match start2 {
            LoopuseStart::Vertexuse(Some(vu)) => vu,
            LoopuseStart::Vertexuse(None) | LoopuseStart::Edgeuse(None) | LoopuseStart::Unknown => {
                return Err(SolidError::null_pointer(OP));
            }
            LoopuseStart::Edgeuse(Some(_)) => {
                return Err(null("FlipLoopOrientation: mate is not a vertex loop use"));
            }
        };
        for vu in [vu1, vu2] {
            if !matches!(b.vertexuse(vu)?.owner, VertexuseOwner::Loopuse(_)) {
                return Err(null(
                    "FlipLoopOrientation: vertex use is not a loop vertex use",
                ));
            }
        }

        // swap which vertexuse goes to which Loopuse - remember to set Vertexuse back
        // pointers
        b.loopuse_mut(lu1)?.start = LoopuseStart::Vertexuse(Some(vu2));
        b.loopuse_mut(lu2)?.start = LoopuseStart::Vertexuse(Some(vu1));
        b.vertexuse_mut(vu2)?.owner = VertexuseOwner::Loopuse(Some(lu1));
        b.vertexuse_mut(vu1)?.owner = VertexuseOwner::Loopuse(Some(lu2));
    } else {
        // get first Edgeuses for both mated Loopuses
        let LoopuseStart::Edgeuse(Some(eu1_)) = start1 else {
            return Err(SolidError::null_pointer(OP));
        };
        let eu2_ = match start2 {
            LoopuseStart::Edgeuse(Some(eu)) => eu,
            LoopuseStart::Edgeuse(None) | LoopuseStart::Vertexuse(None) | LoopuseStart::Unknown => {
                return Err(SolidError::null_pointer(OP));
            }
            LoopuseStart::Vertexuse(Some(_)) => {
                return Err(null("FlipLoopOrientation: mate is not an edge loop use"));
            }
        };

        // swap which Edgeuse goes to which Loopuse and rebuild the Edgeuse->CW/CCW
        // LinkedLists and back pointers
        collect_edgeuses(b, lu2, Some(eu1_))?;
        collect_edgeuses(b, lu1, Some(eu2_))?;
    }

    if let Some(f) = face {
        face_notify_edit(b, f, NotifyOperation::PostEdit)?;
    }
    Ok(())
}

/// `IsOuterLoop`: whether the primary loop use is the first loop use of its face use.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for a loop without loop use, a loop use without face
/// use or a face use without loop uses (upstream reads index 0 of an empty array);
/// [`SolidError::NotFound`] for a dangling id.
pub fn is_outer_loop(b: &Brep, l: LoopId) -> Result<bool> {
    let lu = primary(b, l)?;
    let fu = faceuse(b, lu)?.ok_or_else(|| null("IsOuterLoop: no face use"))?;
    let lus = eu1::faceuse_loopuses(b, fu, false, None)?;
    let first = lus
        .first()
        .copied()
        .ok_or_else(|| null("IsOuterLoop: face use without loop uses"))?;
    Ok(first == lu)
}

/// `SmLoop::IsClosed3d`: [`is_closed_3d`] of the primary loop use; `None` (closed,
/// outputs not written) without loop use.
///
/// # Errors
///
/// Those of [`is_closed_3d`].
pub fn loop_is_closed_3d(
    k: &Kernel,
    b: &Brep,
    l: LoopId,
    check_all_gaps: bool,
) -> Result<Option<Closed3d>> {
    // no work - no Loopuse, report as ok
    match loop_loopuse(b, l)? {
        None => Ok(None),
        Some(lu) => is_closed_3d(k, b, lu, check_all_gaps).map(Some),
    }
}

/// `SmLoop::IsClosedPtrs`: [`is_closed_ptrs`] of the primary loop use; `true` without
/// loop use.
///
/// # Errors
///
/// Those of [`is_closed_ptrs`].
pub fn loop_is_closed_ptrs(b: &Brep, l: LoopId) -> Result<bool> {
    match loop_loopuse(b, l)? {
        None => Ok(true),
        Some(lu) => is_closed_ptrs(b, lu),
    }
}

/// `SmLoop::IsConnectedTo`: whether the loop connects to `target` (its vertex uses,
/// vertices, edge uses, edges, face, face uses, itself, its loop uses, the shells or
/// regions of its face's face uses); `true` for `None` and any other kind.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] where upstream dereferences null (no loop use, face
/// use, face, or a face use without shell for shell and region targets);
/// those of the target's own `IsConnectedTo`; [`SolidError::NotFound`] for a dangling id.
pub fn loop_is_connected_to(b: &Brep, l: LoopId, target: Option<EntityRef>) -> Result<bool> {
    // no work - no Topo
    let Some(target) = target else {
        return Ok(true);
    };
    let this = EntityRef::Loop(l);
    let face_uses = |b: &Brep| -> Result<[Option<FaceuseId>; 2]> {
        let f = loop_face(b, l)?.ok_or_else(|| null("SmLoop::IsConnectedTo: no face"))?;
        let (a, c) = crate::topology::face::p2::faceuses(b, f)?;
        Ok([a, c])
    };
    let shell_of = |b: &Brep, fu: Option<FaceuseId>| -> Result<Option<ShellId>> {
        let fu = fu.ok_or_else(|| null("SmLoop::IsConnectedTo: no face use"))?;
        eu1::faceuse_shell(b, fu)
    };
    match target {
        EntityRef::Vertexuse(vu) => vu1::is_connected_to(b, vu, Some(this)),
        EntityRef::Vertex(v) => vertex_is_connected_to(b, v, this),
        EntityRef::Edgeuse(eu) => eu1::is_connected_to(b, eu, Some(this)),
        EntityRef::Edge(e) => edge_is_connected_to(b, e, this),
        EntityRef::Face(f) => Ok(loop_face(b, l)? == Some(f)),
        EntityRef::Faceuse(fu) => Ok(loop_face(b, l)? == eu1::faceuse_face(b, fu)?),
        EntityRef::Loop(other) => Ok(other == l),
        EntityRef::Loopuse(lu) => Ok(loop_(b, lu)? == Some(l)),
        EntityRef::Shell(s) => {
            let [fu1, fu2] = face_uses(b)?;
            Ok(shell_of(b, fu1)? == Some(s) || shell_of(b, fu2)? == Some(s))
        }
        EntityRef::Region(r) => {
            let [fu1, fu2] = face_uses(b)?;
            let region_of = |fu| -> Result<Option<crate::topology::RegionId>> {
                let s = shell_of(b, fu)?.ok_or_else(|| null("SmLoop::IsConnectedTo: no shell"))?;
                eu1::shell_region(b, s)
            };
            Ok(region_of(fu1)? == Some(r) || region_of(fu2)? == Some(r))
        }
        // arrive here when the target is an unsupported type - return TRUE
        EntityRef::Brep(_) => Ok(true),
    }
}

// ---- UV corners -----------------------------------------------------------------------

/// The result of [`uv_corner_boundaries`] (`GetUVCornerBoundaries`' return value and
/// out-params).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UvCorners {
    /// `SM_SS_NONE` or an OR of `SM_SS_UMIN`, `SM_SS_VMIN`, `SM_SS_UMAX`, `SM_SS_VMAX`.
    pub boundaries: u32,
    /// `rOnEdges`: the edges with an end on a side, one entry per end.
    pub on_edges: Vec<Option<EdgeId>>,
    /// `rOnBndries`: the sides each of those ends is on.
    pub on_boundaries: Vec<u32>,
    /// `rOnParams`: the parameter of each of those ends (of the UV trim curve, or of
    /// the edge when the trim curve could not be built).
    pub on_params: Vec<f64>,
}

/// `pEdgeuse->GetOrCreateUVTrimCurve(pUVTrimCurve)`: the curve after the call (its status
/// is ignored upstream; `None` when it could not be made).
fn get_or_create_uv_trim_curve(
    k: &Kernel,
    b: &mut QueryTopology<'_>,
    eu: EdgeuseId,
) -> Result<Option<Curve>> {
    let _ = b.ensure_trim_curve(k, eu)?;
    Ok(crate::topology::edgeuse::p1::uv_trim_curve(b, eu)?
        .0
        .cloned())
}

/// `pS->DropPoint(point, domain, NULL, bSuccess, uv, dGap, bIsMulti)` (unit
/// `surface.surface_p5`): the UV point it writes to its out-param, or `None` only when
/// it leaves the out-param unwritten. Upstream ignores both the status and `bSuccess`
/// here, so a point written by a failed drop must still be returned.
///
/// With no guess and no solution array, `DropPoint` writes `rResultUV` only when it
/// ends with `rbSuccess` set (its failed `GlobalPointSolve` returns before any write).
///
/// With the opt-in [`crate::kernel::UpstreamFixes::topology_failed_evaluations_propagate`]
/// (WR-262) only a successful drop supplies a corner: a failed one is an error, and an
/// unsuccessful one `SM_ERR`.
fn drop_point(k: &Kernel, s: &Surface, dom: &Extent2d, p: Point3) -> Result<Option<Point2>> {
    let opts = crate::surface::surface::p5::DropPointOptions::default();
    let r = s.drop_point::<crate::surface::surface::p4::NoObject>(k, p, dom, &opts);
    if k.fixes.topology_failed_evaluations_propagate {
        let r = r?;
        return if r.success {
            Ok(Some(r.result_uv))
        } else {
            Err(SolidError::sm_err(
                "topology::loopuse::uv_corner_boundaries",
            ))
        };
    }
    let r = ignored(r)?;
    Ok(r.filter(|d| d.success).map(|d| d.result_uv))
}

/// Ignores a status upstream ignores ([`SolidError::Unported`] still propagates).
fn ignored<T>(r: Result<T>) -> Result<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(e) if e.is_unported() => Err(e),
        Err(_) => Ok(None),
    }
}

/// [`ignored`], or with the opt-in
/// [`crate::kernel::UpstreamFixes::topology_failed_evaluations_propagate`] every error.
fn evaluated<T>(k: &Kernel, r: Result<T>) -> Result<Option<T>> {
    if k.fixes.topology_failed_evaluations_propagate {
        r.map(Some)
    } else {
        ignored(r)
    }
}

/// `GetUVCornerBoundaries`: the sides of `uv_domain` (within `tol`) touched by the UV
/// ends of the loop's edge uses (their UV trim curves, made when missing, or the
/// dropped edge end points when one cannot be made), with each end's edge, sides and
/// parameter; [`SS_NONE`] and no ends without loop use.
///
/// # Errors
///
/// [`SolidError::Unported`] for a loop with edge uses (`GetOrCreateUVTrimCurve`);
/// those of [`edgeuses`]; [`SolidError::NotFound`] for a dangling id. With the opt-in
/// `Kernel::fixes.topology_failed_evaluations_propagate`, also those of the curve
/// evaluations and point drops, and `SM_ERR` when a point drop does not converge.
pub fn uv_corner_boundaries(
    k: &Kernel,
    b: &mut Brep,
    l: LoopId,
    uv_domain: &Extent2d,
    tol: f64,
) -> Result<UvCorners> {
    uv_corner_boundaries_with_topology(k, &mut QueryTopology::Mutable(b), l, uv_domain, tol)
}

/// Shared corner walk; prepared topology rejects missing trims only when visited.
pub(crate) fn uv_corner_boundaries_with_topology(
    k: &Kernel,
    b: &mut QueryTopology<'_>,
    l: LoopId,
    uv_domain: &Extent2d,
    tol: f64,
) -> Result<UvCorners> {
    // init return values
    let mut out = UvCorners {
        boundaries: SS_NONE,
        ..UvCorners::default()
    };

    // check state - no loopuse - no classification
    let Some(lu) = loop_loopuse(b, l)? else {
        return Ok(out);
    };

    // locals (carried from one edge use to the next where upstream ignores a failed
    // evaluation; with the opt-in fix a failure returns instead)
    let mut uv_pnt_beg = Point2::UNDEF;
    let mut uv_pnt_end = Point2::UNDEF;
    let mut uv_beg = Vec3::UNDEF;
    let mut uv_end = Vec3::UNDEF;

    // for every edgeuse - classify the end points
    for eu in edgeuses(b, lu, None, None)? {
        // get UVTrimCurve
        let trim = get_or_create_uv_trim_curve(k, b, eu)?;

        let ivl: Extent1d;
        if let Some(trim) = trim {
            // end points
            ivl = trim.natural_interval();
            if let Some(p) = evaluated(k, trim.evaluate_point(k, ivl.evaluate(0.0)))? {
                uv_beg = p;
            }
            if let Some(p) = evaluated(k, trim.evaluate_point(k, ivl.evaluate(1.0)))? {
                uv_end = p;
            }
            uv_pnt_beg = Point2::new(uv_beg.x, uv_beg.y);
            uv_pnt_end = Point2::new(uv_end.x, uv_end.y);
        } else {
            // Use point drop for problem edgeuses
            let e = eu1::edge(b, eu)?.ok_or_else(|| null("GetUVCornerBoundaries: no edge"))?;
            let f = eu1::face(b, eu)?.ok_or_else(|| null("GetUVCornerBoundaries: no face"))?;
            let face = b.face(f)?;
            let s = face
                .surface
                .as_ref()
                .ok_or_else(|| null("GetUVCornerBoundaries: no surface"))?;
            let dom = face.uv_domain;
            let edge = b.edge(e)?;
            let c = edge
                .curve
                .as_ref()
                .ok_or_else(|| null("GetUVCornerBoundaries: no curve"))?;
            ivl = edge.interval;
            let beg = evaluated(k, c.evaluate_point(k, ivl.min()))?.unwrap_or(Vec3::UNDEF);
            let end = evaluated(k, c.evaluate_point(k, ivl.max()))?.unwrap_or(Vec3::UNDEF);
            if let Some(p) = drop_point(k, s, &dom, beg)? {
                uv_pnt_beg = p;
            }
            if let Some(p) = drop_point(k, s, &dom, end)? {
                uv_pnt_end = p;
            }
        }
        let edge = eu1::edge(b, eu)?;

        // classify the BegPoint
        let these = uv_domain.point_boundaries(uv_pnt_beg, tol);
        out.boundaries |= these;
        if these != 0 {
            out.on_edges.push(edge);
            out.on_boundaries.push(these);
            out.on_params.push(ivl.evaluate(0.0));
        }

        // classify the endPoint - in a perfect world we can skip this, but allow for
        // tolerance errors
        let these = uv_domain.point_boundaries(uv_pnt_end, tol);
        out.boundaries |= these;
        if these != 0 {
            out.on_edges.push(edge);
            out.on_boundaries.push(these);
            out.on_params.push(ivl.evaluate(1.0));
        }
    }

    // all done
    Ok(out)
}

/// `sm_CalcQuadNum`: the quadrant of `test` in a 2D frame centred on `origin`
/// (`1 | 0` above, `2 | 3` below; the `y` axis belongs to the right half, the `x`
/// axis to the upper half).
#[must_use]
pub fn calc_quad_num(origin: Point3, test: Point3) -> u32 {
    if test.x >= origin.x {
        if test.y < origin.y {
            3
        } else {
            0
        }
    } else if test.y >= origin.y {
        1
    } else {
        2
    }
}

/// The result of [`is_loopuse_side_in_surface`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoopuseSide {
    /// The return value: `true` when every edge use's inside side lies in the surface.
    pub in_surface: bool,
    /// `*pOptOnNaturalBoundary`: some edge use tested runs on a natural boundary of the
    /// surface.
    pub on_natural_boundary: bool,
    /// `*pOptEdgeuseIndx`: the index of the first edge use that fails (`0` otherwise).
    pub edgeuse_index: usize,
}

/// `SmLoopuse::IsLoopuseSideInSurface(pOptPeriodicU, pOptPeriodicV,
/// pOptOnNaturalBoundary, pOptEdgeuseIndx)`: runs
/// [`is_edgeuse_side_in_surface`](crate::topology::edgeuse::p2::is_edgeuse_side_in_surface)
/// with 6 samples on the loop use's edge uses in order, stopping at the first that
/// fails. A vertex loop use is `true`.
///
/// # Errors
///
/// Those of [`edgeuses`] and of the edge use test.
pub fn is_loopuse_side_in_surface(
    k: &Kernel,
    b: &Brep,
    lu: LoopuseId,
    periodic_u: Option<bool>,
    periodic_v: Option<bool>,
) -> Result<LoopuseSide> {
    // init return value
    let mut out = LoopuseSide {
        in_surface: true,
        on_natural_boundary: false,
        edgeuse_index: 0,
    };

    // locals
    let smp_count = 6;
    let edgeuses = edgeuses(b, lu, None, None)?;

    // for every edgeuse - check Bounding Edge Side status
    for (ii, &eu) in edgeuses.iter().enumerate() {
        // check and remember Edgeuse Side status
        let side = crate::topology::edgeuse::p2::is_edgeuse_side_in_surface(
            k, b, eu, smp_count, periodic_u, periodic_v,
        )?;
        if side.on_natural_boundary {
            out.on_natural_boundary = true;
        }
        out.in_surface &= side.in_surface;

        // inform the public
        if !side.in_surface {
            out.edgeuse_index = ii;
            break;
        }
    }

    // all done
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nurbs::b_spline_curve::BSplineCurve;
    use crate::nurbs::crv_basic::Curve as NlCurve;
    use crate::nurbs::math::{CPoint, NOW};
    use crate::surface::plane::Plane;
    use crate::topology::{
        Edge, Edgeuse, Face, Faceuse, Loop, Loopuse, Region, RegionId, Shell, Vertex, Vertexuse,
    };

    fn line(a: Point3, c: Point3) -> Curve {
        let pw = vec![
            CPoint::new(a.x, a.y, a.z, NOW),
            CPoint::new(c.x, c.y, c.z, NOW),
        ];
        let nl = NlCurve::new(pw, 1, vec![0.0, 0.0, 1.0, 1.0]).unwrap();
        Curve::from(BSplineCurve::from_nurb(nl, 3, false))
    }

    fn eu(i: u32) -> EdgeuseId {
        EdgeuseId::from_index(i)
    }

    fn lu(i: u32) -> LoopuseId {
        LoopuseId::from_index(i)
    }

    /// A lamina unit square on a plane: face uses 0 / 1, loop 0 with loop uses 0
    /// (`SAME` edge uses `2 i`, CCW `0 -> 2 -> 4 -> 6`) and 1 (`OPPOSITE` edge uses
    /// `2 i + 1`), edge `i` a line from corner `i` to corner `i + 1` (corner 2 moved by
    /// `gap` along x on edge 1's end).
    fn square(gap: f64) -> Brep {
        let mut b = Brep::new();
        b.add_region_record(Region {
            in_brep: true,
            ..Region::default()
        })
        .unwrap();
        b.add_shell_record(Shell {
            faces: vec![FaceId::from_index(0)],
            region: Some(RegionId::from_index(0)),
            ..Shell::default()
        })
        .unwrap();
        b.add_face_record(Face {
            loops: vec![LoopId::from_index(0)],
            surface: Some(Surface::from(Plane::from_normal(Vec3::ZERO, Vec3::Z))),
            faceuse: Some(FaceuseId::from_index(0)),
            ..Face::new(ShellId::from_index(0))
        })
        .unwrap();
        b.add_loop_record(Loop {
            face: FaceId::from_index(0),
            edges: Vec::new(),
            loopuse: Some(lu(0)),
        })
        .unwrap();
        for (k, o) in [OrientType::Same, OrientType::Opposite]
            .into_iter()
            .enumerate()
        {
            let k = k as u32;
            b.add_faceuse_record(Faceuse {
                shell: Some(ShellId::from_index(0)),
                face: Some(FaceId::from_index(0)),
                mate: Some(FaceuseId::from_index(1 - k)),
                orientation: o,
                loopuses: vec![lu(k)],
                sibling_links: None,
            })
            .unwrap();
            b.add_loopuse_record(Loopuse {
                faceuse: Some(FaceuseId::from_index(k)),
                loop_: Some(LoopId::from_index(0)),
                mate: Some(lu(1 - k)),
                start: LoopuseStart::Edgeuse(Some(eu(if k == 0 { 0 } else { 7 }))),
                orientation: OrientType::Same,
            })
            .unwrap();
        }
        let corners = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        let p = |i: usize| {
            let c = corners[i % 4];
            Vec3::new(c[0], c[1], 0.0)
        };
        for i in 0..4u32 {
            b.add_vertex_record(Vertex {
                point: p(i as usize),
                tolerance: 1e-4,
                ..Vertex::default()
            })
            .unwrap();
            let (next, prev) = ((i + 1) % 4, (i + 3) % 4);
            let mut end = p(next as usize);
            if i == 1 {
                end.x += gap;
            }
            b.add_edge_record(Edge {
                vertices: [VertexId::from_index(i), VertexId::from_index(next)],
                edgeuses: vec![eu(2 * i), eu(2 * i + 1)],
                interval: Extent1d::from_min_max(0.0, 1.0),
                tolerance: 1e-4,
                curve: Some(line(p(i as usize), end)),
            })
            .unwrap();
            b.add_edgeuse_record(Edgeuse {
                edge: Some(EdgeId::from_index(i)),
                owner: EdgeuseOwner::Loopuse(Some(lu(0))),
                ccw: Some(eu(2 * next)),
                cw: Some(eu(2 * prev)),
                vertexuse: Some(VertexuseId::from_index(2 * i)),
                orientation: OrientType::Same,
                ..Edgeuse::default()
            })
            .unwrap();
            b.add_edgeuse_record(Edgeuse {
                edge: Some(EdgeId::from_index(i)),
                owner: EdgeuseOwner::Loopuse(Some(lu(1))),
                ccw: Some(eu(2 * prev + 1)),
                cw: Some(eu(2 * next + 1)),
                vertexuse: Some(VertexuseId::from_index(2 * i + 1)),
                orientation: OrientType::Opposite,
                ..Edgeuse::default()
            })
            .unwrap();
            for (v, e) in [(i, 2 * i), (next, 2 * i + 1)] {
                b.add_vertexuse_record(Vertexuse {
                    vertex: Some(VertexId::from_index(v)),
                    owner: VertexuseOwner::Edgeuse(Some(eu(e))),
                    ..Vertexuse::default()
                })
                .unwrap();
            }
        }
        b
    }

    #[test]
    fn types() {
        assert!(is_kind_of(TYPE));
        assert!(is_kind_of(topo::TYPE));
        assert!(!is_kind_of(LOOP_TYPE));
        assert!(loop_is_kind_of(LOOP_TYPE));
        assert!(!loop_is_kind_of(TYPE));
        assert_eq!(loop_class_type(), 16003);
        assert_eq!(TYPE_STRING, "SmLoopuse_TYPE");
        assert_eq!(LOOP_CLASS_STRING, "SmLoop");
    }

    #[test]
    fn accessors_and_walks() {
        let b = square(0.0);
        assert_eq!(loopuse_type(&b, lu(0)).unwrap(), EDGEUSE_TYPE);
        assert!(is_edge_loopuse(&b, lu(0)).unwrap());
        assert!(!is_vertex_loopuse(&b, lu(0)).unwrap());
        assert_eq!(vertexuse(&b, lu(0)).unwrap(), None);
        assert_eq!(
            eu_or_vu(&b, lu(1)).unwrap(),
            Some(EntityRef::Edgeuse(eu(7)))
        );
        assert_eq!(other_loopuse(&b, lu(0)).unwrap(), Some(lu(1)));
        assert_eq!(shell(&b, lu(1)).unwrap(), Some(ShellId::from_index(0)));
        let ids = |v: Vec<EdgeuseId>| v.into_iter().map(EdgeuseId::index).collect::<Vec<_>>();
        assert_eq!(ids(edgeuses(&b, lu(0), None, None).unwrap()), [0, 2, 4, 6]);
        assert_eq!(ids(edgeuses(&b, lu(1), None, None).unwrap()), [7, 5, 3, 1]);
        let e = edges(&b, lu(1), None).unwrap();
        assert_eq!(e.len(), 4);
        assert_eq!(e.first(), Some(&Some(EdgeId::from_index(3))));
        let v = vertices(&b, lu(0), None).unwrap();
        assert_eq!(
            v,
            (0..4)
                .map(|i| Some(VertexId::from_index(i)))
                .collect::<Vec<_>>()
        );
        assert_eq!(vertexuses(&b, lu(1), None).unwrap().len(), 4);
        // the attribute filter sees the entity of each list
        let only = |e: EntityRef| e == EntityRef::Edge(EdgeId::from_index(2));
        assert_eq!(
            edges(&b, lu(0), Some(&only)).unwrap(),
            [Some(EdgeId::from_index(2))]
        );
        assert!(vertices(&b, lu(0), Some(&only)).unwrap().is_empty());
        // the connection filter
        let c = edgeuses(
            &b,
            lu(0),
            None,
            Some(EntityRef::Edge(EdgeId::from_index(1))),
        )
        .unwrap();
        assert_eq!(ids(c), [2]);
        // loop-level views
        assert_eq!(
            loop_face(&b, LoopId::from_index(0)).unwrap(),
            Some(FaceId::from_index(0))
        );
        assert_eq!(
            loop_loopuses(&b, LoopId::from_index(0)).unwrap(),
            (lu(0), Some(lu(1)))
        );
        assert!(is_outer_loop(&b, LoopId::from_index(0)).unwrap());
        assert_eq!(loop_edgeuses(&b, LoopId::from_index(0)).unwrap().len(), 4);
    }

    #[test]
    fn closure() {
        let k = Kernel::default();
        let b = square(0.0);
        assert!(is_closed_ptrs(&b, lu(0)).unwrap());
        let c = is_closed_3d(&k, &b, lu(0), true).unwrap();
        assert!(c.closed);
        assert_eq!(c.max_gap, 0.0);
        assert_eq!(c.edgeuse, None);
        // a gap over tolerance still reports closed (module docs), with the gap
        let b = square(0.5);
        let c = is_closed_3d(&k, &b, lu(0), false).unwrap();
        assert!(c.closed);
        assert_eq!(c.edgeuse, Some(eu(2)));
        assert_eq!(c.max_gap, 0.5);
        assert_eq!(c.xsect_tol_3d, 2e-4);
        // a broken CCW chain
        let mut b = square(0.0);
        b.edgeuse_mut(eu(4)).unwrap().ccw = None;
        assert!(!is_closed_ptrs(&b, lu(0)).unwrap());
        assert_eq!(edgeuses(&b, lu(0), None, None).unwrap().len(), 3);
        // a chain cycling without its start
        b.edgeuse_mut(eu(4)).unwrap().ccw = Some(eu(2));
        assert!(matches!(
            is_closed_ptrs(&b, lu(0)),
            Err(SolidError::InvalidInput { .. })
        ));
        assert!(matches!(
            edgeuses(&b, lu(0), None, None),
            Err(SolidError::InvalidInput { .. })
        ));
        assert!(loop_is_closed_3d(&k, &b, LoopId::from_index(0), true).is_err());
    }

    #[test]
    fn connectivity() {
        let b = square(0.0);
        let l = LoopId::from_index(0);
        assert!(is_connected_to(&b, lu(0), None).unwrap());
        assert!(is_connected_to(&b, lu(0), Some(EntityRef::Loop(l))).unwrap());
        assert!(is_connected_to(&b, lu(0), Some(EntityRef::Face(FaceId::from_index(0)))).unwrap());
        assert!(!is_connected_to(
            &b,
            lu(0),
            Some(EntityRef::Faceuse(FaceuseId::from_index(1)))
        )
        .unwrap());
        assert!(is_connected_to(&b, lu(0), Some(EntityRef::Edgeuse(eu(2)))).unwrap());
        assert!(!is_connected_to(&b, lu(0), Some(EntityRef::Edgeuse(eu(3)))).unwrap());
        assert!(
            is_connected_to(&b, lu(1), Some(EntityRef::Region(RegionId::from_index(0)))).unwrap()
        );
        assert!(is_connected_to(&b, lu(0), Some(EntityRef::Edge(EdgeId::from_index(0)))).unwrap());
        assert!(loop_is_connected_to(&b, l, Some(EntityRef::Loopuse(lu(1)))).unwrap());
        assert!(
            loop_is_connected_to(&b, l, Some(EntityRef::Shell(ShellId::from_index(0)))).unwrap()
        );
        assert!(
            loop_is_connected_to(&b, l, Some(EntityRef::Faceuse(FaceuseId::from_index(1))))
                .unwrap()
        );
        assert!(
            loop_is_connected_to(&b, l, Some(EntityRef::Vertex(VertexId::from_index(0)))).unwrap()
        );
    }

    #[test]
    fn mutation() {
        let mut b = square(0.0);
        // SetEdgeuse only to an edge use of this loop use
        set_edgeuse(&mut b, lu(0), Some(eu(4))).unwrap();
        assert_eq!(
            eu_or_vu(&b, lu(0)).unwrap(),
            Some(EntityRef::Edgeuse(eu(4)))
        );
        assert!(matches!(
            set_edgeuse(&mut b, lu(0), Some(eu(1))),
            Err(SolidError::Upstream { code: 1001, .. })
        ));
        assert!(set_edgeuse(&mut b, lu(0), None).is_err());
        // CollectEdgeuses re-owns the CCW list
        collect_edgeuses(&mut b, lu(1), Some(eu(0))).unwrap();
        assert_eq!(
            b.edgeuse(eu(6)).unwrap().owner,
            EdgeuseOwner::Loopuse(Some(lu(1)))
        );
        assert!(matches!(
            collect_edgeuses(&mut b, lu(1), None),
            Err(SolidError::Upstream { code: 1006, .. })
        ));
        set_orientation(&mut b, lu(0), OrientType::Opposite).unwrap();
        assert_eq!(orientation(&b, lu(0)).unwrap(), OrientType::Opposite);
        set_loop(&mut b, lu(0), None).unwrap();
        assert_eq!(loop_(&b, lu(0)).unwrap(), None);
        // flipping a loop on a face runs its post-edit notification through
        // SmFace::GetUpwardEdgeuses (topology.face_p4)
        let mut b = square(0.0);
        flip_loop_orientation(&mut b, LoopId::from_index(0)).unwrap();
        // without the face, the edge use lists swap loop uses
        let mut b = square(0.0);
        b.faceuse_mut(FaceuseId::from_index(0)).unwrap().face = None;
        flip_loop_orientation(&mut b, LoopId::from_index(0)).unwrap();
        assert_eq!(
            eu_or_vu(&b, lu(0)).unwrap(),
            Some(EntityRef::Edgeuse(eu(7)))
        );
        assert_eq!(
            b.edgeuse(eu(0)).unwrap().owner,
            EdgeuseOwner::Loopuse(Some(lu(1)))
        );
        assert_eq!(
            b.edgeuse(eu(1)).unwrap().owner,
            EdgeuseOwner::Loopuse(Some(lu(0)))
        );
    }

    #[test]
    fn issue_521_uv_corners_refuse_failed_trim_evaluation_after_valid_edge() {
        let mut k = Kernel::default();
        k.fixes.topology_failed_evaluations_propagate = true;
        let dom = Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0);
        for bad in [0, 2] {
            let mut b = square(0.0);
            for i in 0..4 {
                b.edgeuse_mut(eu(2 * i)).unwrap().uv_trim_curve =
                    b.edge(EdgeId::from_index(i)).unwrap().curve.clone();
            }
            let empty = Curve::from(BSplineCurve::new(2));
            let expected = empty.evaluate_point(&k, 0.0).unwrap_err();
            b.edgeuse_mut(eu(bad)).unwrap().uv_trim_curve = Some(empty);
            assert_eq!(
                uv_corner_boundaries(&k, &mut b, LoopId::from_index(0), &dom, 1e-6),
                Err(expected),
            );
        }
    }

    #[test]
    fn uv_corners_ignore_failed_trim_evaluation_by_default() {
        // WR-262: upstream ignores the failed evaluation and keeps the previous ends.
        let k = Kernel::default();
        let dom = Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0);
        let mut b = square(0.0);
        for i in 0..4 {
            b.edgeuse_mut(eu(2 * i)).unwrap().uv_trim_curve =
                b.edge(EdgeId::from_index(i)).unwrap().curve.clone();
        }
        b.edgeuse_mut(eu(2)).unwrap().uv_trim_curve = Some(Curve::from(BSplineCurve::new(2)));
        assert!(uv_corner_boundaries(&k, &mut b, LoopId::from_index(0), &dom, 1e-6).is_ok());
    }

    #[test]
    fn vertex_loop_and_corners() {
        let k = Kernel::default();
        let mut b = square(0.0);
        // make loop use 0 a vertex loop of vertex use 0, loop use 1 of vertex use 1
        for (l, v) in [(0, 0), (1, 1)] {
            b.loopuse_mut(lu(l)).unwrap().start =
                LoopuseStart::Vertexuse(Some(VertexuseId::from_index(v)));
            b.vertexuse_mut(VertexuseId::from_index(v)).unwrap().owner =
                VertexuseOwner::Loopuse(Some(lu(l)));
        }
        assert_eq!(
            vertexuses(&b, lu(0), None).unwrap(),
            [Some(VertexuseId::from_index(0))]
        );
        assert_eq!(
            vertices(&b, lu(1), None).unwrap(),
            [Some(VertexId::from_index(1))]
        );
        assert!(edgeuses(&b, lu(0), None, None).unwrap().is_empty());
        assert!(is_closed_ptrs(&b, lu(0)).unwrap());
        assert!(is_closed_3d(&k, &b, lu(0), true).unwrap().closed);
        let dom = Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0);
        let c = uv_corner_boundaries(&k, &mut b, LoopId::from_index(0), &dom, 1e-6).unwrap();
        assert_eq!(c.boundaries, SS_NONE);
        // the plane is closed in neither direction
        assert!(!is_missing_seam_at_vertices(&k, &mut b, lu(0)).unwrap());
        b.faceuse_mut(FaceuseId::from_index(0)).unwrap().face = None;
        flip_loop_orientation(&mut b, LoopId::from_index(0)).unwrap();
        assert_eq!(
            vertexuse(&b, lu(0)).unwrap(),
            Some(VertexuseId::from_index(1))
        );
        assert_eq!(
            b.vertexuse(VertexuseId::from_index(0)).unwrap().owner,
            VertexuseOwner::Loopuse(Some(lu(1)))
        );
    }

    #[test]
    fn quadrants() {
        let o = Vec3::new(1.0, 1.0, 0.0);
        assert_eq!(calc_quad_num(o, Vec3::new(2.0, 2.0, 0.0)), 0);
        assert_eq!(calc_quad_num(o, Vec3::new(0.0, 2.0, 0.0)), 1);
        assert_eq!(calc_quad_num(o, Vec3::new(0.0, 0.0, 0.0)), 2);
        assert_eq!(calc_quad_num(o, Vec3::new(2.0, 0.0, 0.0)), 3);
        assert_eq!(calc_quad_num(o, o), 0);
        assert_eq!(calc_quad_num(o, Vec3::new(1.0, 0.0, 0.0)), 3);
        assert_eq!(calc_quad_num(o, Vec3::new(0.0, 1.0, 0.0)), 1);
    }

    #[test]
    fn requested_trim_prepares_only_the_visited_use_and_bounds_retries() {
        let mut k = Kernel::default();
        let mut b = crate::primitive::api_primitives::create_pyramid(&mut k, Vec3::ZERO, 5.0, 20.0)
            .unwrap();
        let edgeuse = crate::topology::brep::p1::edges(&b, None)
            .unwrap()
            .into_iter()
            .flat_map(|e| b.edge(e).unwrap().edgeuses.iter().copied())
            .find(|&eu| eu1::uv_trim_curve(&b, eu).unwrap().0.is_none())
            .unwrap();
        let count = |b: &Brep| {
            b.edgeuses()
                .iter()
                .filter(|(_, eu)| eu.uv_trim_curve.is_some())
                .count()
        };
        let before = count(&b);
        let mut preparation = crate::query::brep::RayPreparation::default();
        assert!(QueryTopology::Requested(&b, &preparation)
            .ensure_trim_curve(&k, edgeuse)
            .is_err_and(|e| matches!(e, SolidError::PreparationRequired { .. })));
        assert_eq!(
            preparation.take_request(),
            Some(crate::query::brep::PreparationRequest::UvTrim(edgeuse))
        );
        assert_eq!(count(&b), before);
        preparation
            .prepare(
                &k,
                &mut b,
                crate::query::brep::PreparationRequest::UvTrim(edgeuse),
            )
            .unwrap();
        assert_eq!(count(&b), before + 1);
        assert!(QueryTopology::Requested(&b, &preparation)
            .ensure_trim_curve(&k, edgeuse)
            .unwrap());
        assert_eq!(preparation.take_request(), None);
        assert!(matches!(
            preparation.prepare(
                &k,
                &mut b,
                crate::query::brep::PreparationRequest::UvTrim(edgeuse)
            ),
            Err(SolidError::InvalidInput { .. })
        ));
    }

    #[test]
    fn prepared_trims_preserve_own_failure_status_and_corner_behavior() {
        let k = Kernel::default();
        let mut b = square(0.0);
        for i in 0..4 {
            let trim = b.edge(EdgeId::from_index(i)).unwrap().curve.clone();
            b.edgeuse_mut(eu(2 * i)).unwrap().uv_trim_curve = trim;
        }
        b.edgeuse_mut(eu(0)).unwrap().drop_curve_fail =
            Some(Box::new(crate::surface::surface::p2::DropCurveFail {
                fail_status: crate::error::SM_ERR,
                ..crate::surface::surface::p2::DropCurveFail::default()
            }));
        // This use's recorded failure is observed; its mate uses the same trim
        // but must not inherit that failure (GetOrCreateUVTrimCurve semantics).
        for (edgeuse, expected) in [(eu(0), false), (eu(1), true)] {
            let preparation = crate::query::brep::RayPreparation::default();
            assert_eq!(
                QueryTopology::Requested(&b, &preparation)
                    .ensure_trim_curve(&k, edgeuse)
                    .unwrap(),
                expected
            );
            assert_eq!(preparation.take_request(), None);
            assert_eq!(
                QueryTopology::Prepared(&b)
                    .ensure_trim_curve(&k, edgeuse)
                    .unwrap(),
                expected,
            );
            assert_eq!(
                QueryTopology::Mutable(&mut b)
                    .ensure_trim_curve(&k, edgeuse)
                    .unwrap(),
                expected,
            );
        }
        // Corner classification ignores the recorded status, unlike containment.
        let domain = Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0);
        let prepared = uv_corner_boundaries_with_topology(
            &k,
            &mut QueryTopology::Prepared(&b),
            LoopId::from_index(0),
            &domain,
            1e-6,
        )
        .unwrap();
        assert_ne!(prepared.boundaries, SS_NONE);
        assert_eq!(
            prepared,
            uv_corner_boundaries(&k, &mut b, LoopId::from_index(0), &domain, 1e-6).unwrap(),
        );
    }
}
