//! `SmEdgeuse` and `SmFaceuse`, part 1: the edge use and face use queries (owner,
//! mate, radial, loop neighbours, face, shell, B-rep, connectivity), the UV trim curve
//! slot of a mated pair, the cached gap entry points, the face use loop/edge/neighbour
//! collections and the file-static helpers of `SmEdgeuse.cpp`.
//!
//! Upstream: `source/SMLib/src/SmEdgeuse.cpp`, `source/SMLib/inc/SmEdgeuse.h`,
//! `source/SMLib/src/SmFaceuse.cpp`, `source/SMLib/inc/SmFaceuse.h`.
//!
//! | Rust                                   | Upstream                                         |
//! |----------------------------------------|--------------------------------------------------|
//! | [`TYPE`], [`TYPE_STRING`], [`CLASS_STRING`], [`is_kind_of`] | `SmEdgeuse::GetType`, `GetTypeString`, `GetClassString`, `IsKindOf` |
//! | [`FACEUSE_TYPE`], [`FACEUSE_TYPE_STRING`], [`FACEUSE_CLASS_STRING`], [`faceuse_is_kind_of`] | `SmFaceuse::GetType`, `GetTypeString`, `GetClassString`, `IsKindOf` |
//! | [`crate::topology::Edgeuse`]           | `SmEdgeuse` (members; `Default` is the constructor) |
//! | [`crate::topology::Faceuse`]           | `SmFaceuse` (members; `Default` is `SmFaceuse()`, `Drop` `~SmFaceuse`) |
//! | [`edge`]                               | `SmEdgeuse::GetEdge`                             |
//! | [`orientation`], [`set_orientation`]   | `SmEdgeuse::GetOrientation`, `SetOrientation`    |
//! | [`cw_edgeuse`], [`ccw_edgeuse`]        | `SmEdgeuse::GetCWEdgeuse`, `GetCCWEdgeuse`       |
//! | [`has_edge_face_gap`], [`has_edge_ccw_edge_gap`] | `SmEdgeuse::HasEdgeFaceGap`, `HasEdgeCCWEdgeGap` |
//! | [`is_max_edge_face_gap_init`]          | `SmEdgeuse::IsMaxEdgeFaceGapInit`                |
//! | [`is_strut`]                           | `SmEdgeuse::IsStrut`                             |
//! | [`vertexuse`]                          | `SmEdgeuse::GetVertexuse`                        |
//! | [`faceuse`], [`face`], [`loopuse`]     | `SmEdgeuse::GetFaceuse`, `GetFace`, `GetLoopuse` |
//! | [`is_loop_edgeuse`], [`is_shell_edgeuse`] | `SmEdgeuse::IsLoopEdgeuse`, `IsShellEdgeuse`  |
//! | [`corner_mate_edgeuse`]                | `SmEdgeuse::GetCornerMateEdgeuse`                |
//! | [`shell`], [`brep`]                    | `SmEdgeuse::GetShell`, `GetBrep`                 |
//! | [`is_connected_to`]                    | `SmEdgeuse::IsConnectedTo`                       |
//! | [`calc_dist_and_dev`]                  | `sm_CalcDistAndDev` (static)                     |
//! | [`are_seam_curves`]                    | `sm_AreSeamCurves` (static)                      |
//! | [`FindStepOff`]                        | `SmFindStepOffENFO` (constructor, destructor, `Evaluate`) |
//! | [`mate`], [`radial`]                   | `SmEdgeuse::GetMate`, `GetRadial`                |
//! | [`set_uv_trim_curve`], [`UvTrimCurveArg`] | `SmEdgeuse::SetUVTrimCurve`                   |
//! | [`delete_uv_trim_curve`]               | `SmEdgeuse::DeleteUVTrimCurve`                   |
//! | [`set_member_uv_trim_curve`]           | `SmEdgeuse::SetMemberUVTrimCurve`                |
//! | [`max_edge_face_gap`], [`edge_ccw_edge_gap`] | `SmEdgeuse::GetMaxEdgeFaceGap`, `GetEdgeCCWEdgeGap` |
//! | [`calc_max_edge_face_gap`], [`calc_edge_ccw_edge_gap`] | `SmEdgeuse::CalcMaxEdgeFaceGap`, `CalcEdgeCCWEdgeGap` |
//! | [`tolerance`]                          | `SmEdgeuse::GetTolerance` (`SM_USE_OLDTOL`)      |
//! | [`uv_trim_curve`], [`drop_curve_fail`] | `SmEdgeuse::GetUVTrimCurve`, `GetDropCurveFail`  |
//! | [`uv_trim_curve_pointer`]              | `SmEdgeuse::GetUVTrimCurvePointer`               |
//! | [`start_point_uv_loop_gap`]            | `SmEdgeuse::GetStartPointUVLoopGap`              |
//! | [`faceuse_shell`], [`faceuse_region`], [`faceuse_face`], [`faceuse_mate`], [`faceuse_orientation`] | `SmFaceuse::GetShell`, `GetRegion`, `GetFace`, `GetMate`, `GetOrientation` |
//! | [`faceuse_loopuses`], [`faceuse_edgeuses`] | `SmFaceuse::GetLoopuses`, `GetEdgeuses`      |
//! | [`faceuse_neighbors`]                  | `SmFaceuse::GetNeighbors`                        |
//! | [`faceuse_collect_adjacent`]           | `SmFaceuse::CollectAdjacentFaceuses`             |
//! | [`edgeuse_next`], [`edgeuse_last`]     | `SmTopology::GetNext`, `GetLast` of an edge use (CO-PORTED, `topology.topology` list links) |
//! | [`primary_edgeuse`], [`edge_is_lamina`], [`edge_brep`] | `SmEdge::GetPrimaryEdgeuse`, `IsLamina`, `GetBrep` (CO-PORTED, `topology.edge`) |
//! | [`loopuse_edgeuses`], [`loopuse_shell`] | `SmLoopuse::GetEdgeuses`, `GetShell` (unit `topology.loopuse`) |
//! | [`vertexuse_edgeuse`]                  | `SmVertexuse::GetEdgeuse` (CO-PORTED, `topology.vertexuse`) |
//! | [`shell_region`], [`shell_brep`]       | `SmShell::GetRegion`, `GetBrep` (unit `topology.shell`) |
//! | [`owning_topology_is_kind_of`]         | `SmOwningTopology::IsKindOf` (unit `topology.proto_topology_p1`) |
//!
//! Deviations:
//! - **Object model.** An edge or face use is a record in the [`Brep`]'s arenas; the
//!   methods are free functions taking the B-rep and the use's id. A pointer argument
//!   or result is an `Option` of an id. `GetBrep` returns the [`Brep`] when the owner
//!   chain reaches a region on the B-rep's region list. An edge use's `m_pNext` /
//!   `m_pLast` are its neighbours on its edge's edge use list
//!   ([`crate::topology::Edge::edgeuses`]). The constructors' and destructors'
//!   `SmObject::Notify` calls are not modelled; `SmFaceuse()` is `Faceuse::default()`
//!   and `~SmFaceuse` is `Drop`.
//! - **Null pointers.** Where upstream dereferences a null pointer (an edge use
//!   without edge, an edge without primary edge use, a loop edge use without vertex use
//!   or face use, ...) the port returns [`SolidError::InvalidInput`]. `SM_ASSERT`s are
//!   compiled out of the reference build and are not checked.
//! - **UV trim curves.** The curve is owned by the edge use holding it, which is
//!   therefore its owner (`SetOwner`). [`set_uv_trim_curve`] takes the curve as an
//!   [`UvTrimCurveArg`] (upstream's pointer comparison with the attached curve is
//!   [`UvTrimCurveArg::Current`]) and hands a detached old curve that it was not asked
//!   to delete back to the caller instead of leaking it. The copy of the
//!   `SmDropCurveFail` that `SetUVTrimCurve` allocates in its "no work" branch when no
//!   old record exists is never stored upstream (a local leak) and is not made here.
//!   [`set_member_uv_trim_curve`] returns the curve and record it replaces.
//! - **Out-params.** `GetUVTrimCurve` / `GetDropCurveFail` return the optional owner
//!   with the result; `CalcMaxEdgeFaceGap` / `CalcEdgeCCWEdgeGap` return the
//!   calculated gap (the reset gap when there is none) and `CalcEdgeCCWEdgeGap` takes
//!   whether its output is wanted (it decides whether the gap is set at all). The
//!   cached gaps are returned by value. `CollectAdjacentFaceuses` appends to the
//!   caller's list, as upstream (entries already there are not added again).
//! - **`sm_CalcDistAndDev`** reaches the solution's first object through a resolver
//!   (upstream `SM_CAST_PTR(SmSurface, ...)`). It and `GetStartPointUVLoopGap` ignore
//!   the statuses upstream ignores ([`SolidError::Unported`] still propagates),
//!   leaving the outputs of a failed evaluation at their `SM_UNDEF_DOUBLE` defaults.
//!   With the opt-in `Kernel::fixes.topology_failed_evaluations_propagate` (WR-263,
//!   WR-264, #521) they propagate evaluation failures instead of reading unwritten
//!   outputs, and an unsuccessful point drop returns `SM_ERR`. `sm_AreSeamCurves`'
//!   unused edge use argument is dropped.
//! - **Unported callees.** `SmEdgeuse::EvaluateBinormal` is wired
//!   (`topology.edgeuse_p3`); its deferred callees return [`SolidError::Unported`]. The gap computations are [`crate::heal::gap`];
//!   `CalcMaxEdgeFaceGap`'s cross-section tolerance is the context one with no
//!   context (`SmTol::GetXSectTol3d(NULL)`: the B-rep records carry no context).
//! - **Co-ports.** The small accessors of other units this unit reaches are ported
//!   here (table above) and recorded as CO-PORTED in `plan/modules.toml`.
//!   [`loopuse_edgeuses`] (no attribute or connection filter; callers here pass none)
//!   and [`loopuse_shell`] call unit `topology.loopuse`.

use crate::curve::curve::Curve;
use crate::error::{EntityRef, Result, SolidError};
use crate::geom::extent1d::Extent1d;
use crate::geom::global_solver::SolverOperation;
use crate::geom::local_solve_nd::EvalNFunctions;
use crate::geom::matrix::Matrix;
use crate::geom::object::ObjectRef;
use crate::geom::solution_array::{ObjectKind, Solution, SolutionObject};
use crate::geom::tol::{context_xsect_tol_3d, SmallTopoType, ZONE_TOL_3D};
use crate::heal::gap::{EdgeEdgeGap, EdgeFaceGap, GapFunction};
use crate::kernel::Kernel;
use crate::math::{Point2, Point3, Vec3, EFF_ZERO, EFF_ZERO_SQ};
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::nurbs::knots::Side;
use crate::surface::surface::p1::evaluate_1st_derivatives;
use crate::surface::surface::p2::DropCurveFail;
use crate::surface::surface::p4::NoObject;
use crate::surface::surface::p5::DropPointOptions;
use crate::surface::surface::Surface;
use crate::topology::{
    topology as topo, Brep, EdgeId, EdgeuseId, EdgeuseOwner, FaceId, FaceuseId, LoopId, LoopuseId,
    NamingScheme, ObjectHandle, RegionId, ShellId, VertexId, VertexuseId, VertexuseOwner,
};

const OP: &str = "topology::edgeuse::p1";
const OP_STEP: &str = "topology::edgeuse::p1::FindStepOff::evaluate";

/// `SmEdgeuse_TYPE` (`TOPO_BASE_TYPE + 5`).
pub const TYPE: i64 = 16005;
/// `SmEdgeuse::GetTypeString`.
pub const TYPE_STRING: &str = "SmEdgeuse_TYPE";
/// `SmEdgeuse::GetClassString`.
pub const CLASS_STRING: &str = "SmEdgeuse";

/// `SmFaceuse_TYPE` (`TOPO_BASE_TYPE + 22`).
pub const FACEUSE_TYPE: i64 = 16022;
/// `SmFaceuse::GetTypeString`.
pub const FACEUSE_TYPE_STRING: &str = "SmFaceuse_TYPE";
/// `SmFaceuse::GetClassString`.
pub const FACEUSE_CLASS_STRING: &str = "SmFaceuse";

/// `SmOwningTopology_TYPE` (`TOPO_BASE_TYPE + 10`).
pub const OWNING_TOPOLOGY_TYPE: i64 = crate::topology::proto_topology::p1::owning_topology::TYPE;

/// `SmEdgeuse::IsKindOf`: [`TYPE`], then the `SmTopology` chain.
#[must_use]
pub const fn is_kind_of(t: i64) -> bool {
    if t == TYPE {
        true
    } else {
        topo::is_kind_of(t)
    }
}

/// `SmOwningTopology::IsKindOf` (unit `topology.proto_topology_p1`):
/// [`OWNING_TOPOLOGY_TYPE`], then the `SmTopology` chain.
#[must_use]
pub const fn owning_topology_is_kind_of(t: i64) -> bool {
    crate::topology::proto_topology::p1::owning_topology::is_kind_of(t)
}

/// `SmFaceuse::IsKindOf`: [`FACEUSE_TYPE`], then the `SmOwningTopology` chain.
#[must_use]
pub const fn faceuse_is_kind_of(t: i64) -> bool {
    if t == FACEUSE_TYPE {
        true
    } else {
        owning_topology_is_kind_of(t)
    }
}

fn null(reason: &'static str) -> SolidError {
    SolidError::invalid_input(OP, reason)
}

// ---- co-ported accessors of other units ---------------------------------------------------

/// Position of `eu` on its edge's edge use list with that list, `None` when the edge
/// use is on no list.
fn list_of(brep: &Brep, eu: EdgeuseId) -> Result<Option<(usize, &[EdgeuseId])>> {
    let Some(e) = brep.edgeuse(eu)?.edge else {
        return Ok(None);
    };
    let list = brep.edge(e)?.edgeuses.as_slice();
    Ok(list.iter().position(|&x| x == eu).map(|i| (i, list)))
}

/// `SmTopology::GetNext` of an edge use: the next member of its edge's circular edge
/// use list (CO-PORTED list link of unit `topology.topology`).
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn edgeuse_next(brep: &Brep, eu: EdgeuseId) -> Result<Option<EdgeuseId>> {
    Ok(list_of(brep, eu)?.and_then(|(i, list)| list.get((i + 1) % list.len()).copied()))
}

/// `SmTopology::GetLast` of an edge use: the previous member of its edge's circular
/// edge use list (CO-PORTED list link of unit `topology.topology`).
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn edgeuse_last(brep: &Brep, eu: EdgeuseId) -> Result<Option<EdgeuseId>> {
    Ok(list_of(brep, eu)?
        .and_then(|(i, list)| list.get((i + list.len() - 1) % list.len()).copied()))
}

/// `SmEdge::GetPrimaryEdgeuse` (CO-PORTED from unit `topology.edge`): the head of the
/// edge's edge use list.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn primary_edgeuse(brep: &Brep, edge: EdgeId) -> Result<Option<EdgeuseId>> {
    Ok(brep.edge(edge)?.edgeuses.first().copied())
}

/// `SmEdge::IsLamina` (CO-PORTED from unit `topology.edge`): two edge uses, the
/// first a loop edge use (an edge on the boundary of one face).
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn edge_is_lamina(brep: &Brep, edge: EdgeId) -> Result<bool> {
    let list = &brep.edge(edge)?.edgeuses;
    if list.len() != 2 {
        return Ok(false);
    }
    let Some(&first) = list.first() else {
        return Ok(false);
    };
    is_loop_edgeuse(brep, first)
}

/// `SmEdge::GetBrep` (CO-PORTED from unit `topology.edge`): the B-rep reached
/// through the primary edge use's shell, `None` without primary edge use or shell
/// (upstream reports `SE(SM_ERR)` and returns `NULL`).
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; those of [`shell`].
pub fn edge_brep(brep: &Brep, edge: EdgeId) -> Result<Option<&Brep>> {
    let Some(eu) = primary_edgeuse(brep, edge)? else {
        return Ok(None);
    };
    match shell(brep, eu)? {
        None => Ok(None),
        Some(s) => shell_brep(brep, s),
    }
}

/// `SmLoopuse::GetEdgeuses` without filters (unit `topology.loopuse`): the edge uses
/// of an edge loop use in CCW order from its start edge use; empty for a vertex loop
/// use or a loop use without start.
///
/// # Errors
///
/// Those of [`crate::topology::loopuse::edgeuses`].
pub fn loopuse_edgeuses(brep: &Brep, lu: LoopuseId) -> Result<Vec<EdgeuseId>> {
    crate::topology::loopuse::edgeuses(brep, lu, None, None)
}

/// `SmLoopuse::GetShell` (unit `topology.loopuse`): the shell of the owning face use.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn loopuse_shell(brep: &Brep, lu: LoopuseId) -> Result<Option<ShellId>> {
    crate::topology::loopuse::shell(brep, lu)
}

/// `SmVertexuse::GetEdgeuse` (CO-PORTED from unit `topology.vertexuse`): the edge use
/// of an edge vertex use.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn vertexuse_edgeuse(brep: &Brep, vu: VertexuseId) -> Result<Option<EdgeuseId>> {
    Ok(match brep.vertexuse(vu)?.owner {
        VertexuseOwner::Edgeuse(eu) => eu,
        _ => None,
    })
}

/// `SmShell::GetRegion`: delegates to unit `topology.shell`.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn shell_region(brep: &Brep, s: ShellId) -> Result<Option<RegionId>> {
    crate::topology::shell::shell_region(brep, s)
}

/// `SmShell::GetBrep` (delegates to unit `topology.shell`): the B-rep of the owning
/// region.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn shell_brep(brep: &Brep, s: ShellId) -> Result<Option<&Brep>> {
    crate::topology::shell::shell_brep(brep, s)
}

// ---- simple edge use access ---------------------------------------------------------------

/// `GetEdge`: the owning edge.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn edge(brep: &Brep, eu: EdgeuseId) -> Result<Option<EdgeId>> {
    Ok(brep.edgeuse(eu)?.edge)
}

/// `GetOrientation`.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn orientation(brep: &Brep, eu: EdgeuseId) -> Result<OrientType> {
    Ok(brep.edgeuse(eu)?.orientation)
}

/// `SetOrientation`.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn set_orientation(brep: &mut Brep, eu: EdgeuseId, orientation: OrientType) -> Result<()> {
    brep.edgeuse_mut(eu)?.orientation = orientation;
    Ok(())
}

/// `GetCWEdgeuse`: the clockwise loop neighbour.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn cw_edgeuse(brep: &Brep, eu: EdgeuseId) -> Result<Option<EdgeuseId>> {
    Ok(brep.edgeuse(eu)?.cw)
}

/// `GetCCWEdgeuse`: the counter-clockwise loop neighbour.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn ccw_edgeuse(brep: &Brep, eu: EdgeuseId) -> Result<Option<EdgeuseId>> {
    Ok(brep.edgeuse(eu)?.ccw)
}

/// `IsLoopEdgeuse`: the edge use bounds a face (type `SmLoopuse_TYPE`).
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn is_loop_edgeuse(brep: &Brep, eu: EdgeuseId) -> Result<bool> {
    Ok(matches!(brep.edgeuse(eu)?.owner, EdgeuseOwner::Loopuse(_)))
}

/// `IsShellEdgeuse`: a wire edge use (type `SmShell_TYPE`).
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn is_shell_edgeuse(brep: &Brep, eu: EdgeuseId) -> Result<bool> {
    Ok(matches!(brep.edgeuse(eu)?.owner, EdgeuseOwner::Shell(_)))
}

/// `HasEdgeFaceGap`: [`is_loop_edgeuse`].
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn has_edge_face_gap(brep: &Brep, eu: EdgeuseId) -> Result<bool> {
    is_loop_edgeuse(brep, eu)
}

/// `HasEdgeCCWEdgeGap`: [`is_loop_edgeuse`].
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn has_edge_ccw_edge_gap(brep: &Brep, eu: EdgeuseId) -> Result<bool> {
    is_loop_edgeuse(brep, eu)
}

/// `IsMaxEdgeFaceGapInit`: the cached edge/face gap is set.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn is_max_edge_face_gap_init(brep: &Brep, eu: EdgeuseId) -> Result<bool> {
    Ok(brep.edgeuse(eu)?.max_edge_face_gap.is_init())
}

/// `GetVertexuse`: the vertex use at the edge use's start.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn vertexuse(brep: &Brep, eu: EdgeuseId) -> Result<Option<VertexuseId>> {
    Ok(brep.edgeuse(eu)?.vertexuse)
}

/// `GetLoopuse`: the loop use of a loop edge use.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn loopuse(brep: &Brep, eu: EdgeuseId) -> Result<Option<LoopuseId>> {
    Ok(match brep.edgeuse(eu)?.owner {
        EdgeuseOwner::Loopuse(lu) => lu,
        _ => None,
    })
}

/// `GetFaceuse`: the face use of a loop edge use's loop use.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn faceuse(brep: &Brep, eu: EdgeuseId) -> Result<Option<FaceuseId>> {
    match loopuse(brep, eu)? {
        Some(lu) => Ok(brep.loopuse(lu)?.faceuse),
        None => Ok(None),
    }
}

/// `GetFace`: the face of a loop edge use's face use.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; [`SolidError::InvalidInput`] for a loop
/// use without face use (upstream dereferences it).
pub fn face(brep: &Brep, eu: EdgeuseId) -> Result<Option<FaceId>> {
    match loopuse(brep, eu)? {
        Some(lu) => {
            let fu = brep
                .loopuse(lu)?
                .faceuse
                .ok_or_else(|| null("GetFace: loop use without face use"))?;
            Ok(brep.faceuse(fu)?.face)
        }
        None => Ok(None),
    }
}

/// `GetShell`: the shell of a wire edge use, or of a loop edge use's face use.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn shell(brep: &Brep, eu: EdgeuseId) -> Result<Option<ShellId>> {
    match brep.edgeuse(eu)?.owner {
        EdgeuseOwner::Shell(s) => Ok(s),
        EdgeuseOwner::Loopuse(Some(lu)) => loopuse_shell(brep, lu),
        EdgeuseOwner::Loopuse(None) | EdgeuseOwner::Unknown => Ok(None),
    }
}

/// `GetBrep`: the B-rep of the owning edge ([`edge_brep`]).
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; [`SolidError::InvalidInput`] for an edge
/// use without edge (upstream dereferences it).
pub fn brep(brep: &Brep, eu: EdgeuseId) -> Result<Option<&Brep>> {
    let e = brep
        .edgeuse(eu)?
        .edge
        .ok_or_else(|| null("GetBrep: edge use without edge"))?;
    edge_brep(brep, e)
}

/// `GetTolerance` (`SM_USE_OLDTOL`): the owning edge's tolerance, `SM_ZONE_TOL_3D`
/// without edge.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn tolerance(brep: &Brep, eu: EdgeuseId) -> Result<f64> {
    match brep.edgeuse(eu)?.edge {
        Some(e) => Ok(brep.edge(e)?.tolerance),
        None => Ok(ZONE_TOL_3D),
    }
}

// ---- mate and radial ------------------------------------------------------------------------

/// The primary edge use's orientation for [`mate`] / [`radial`] of a loop edge use.
fn primary_orientation(brep: &Brep, eu: EdgeuseId) -> Result<OrientType> {
    let e = brep
        .edgeuse(eu)?
        .edge
        .ok_or_else(|| null("GetMate/GetRadial: edge use without edge"))?;
    let prim = primary_edgeuse(brep, e)?
        .ok_or_else(|| null("GetMate/GetRadial: edge without primary edge use"))?;
    Ok(brep.edgeuse(prim)?.orientation)
}

/// `GetMate`: the oppositely oriented edge use on the face's other side; the next
/// list member of a wire edge use.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; [`SolidError::InvalidInput`] for a
/// non-wire edge use without edge or primary edge use.
pub fn mate(brep: &Brep, eu: EdgeuseId) -> Result<Option<EdgeuseId>> {
    if is_shell_edgeuse(brep, eu)? {
        return edgeuse_next(brep, eu);
    }
    if primary_orientation(brep, eu)? == brep.edgeuse(eu)?.orientation {
        edgeuse_next(brep, eu)
    } else {
        edgeuse_last(brep, eu)
    }
}

/// `GetRadial`: the oppositely oriented edge use on the adjacent face; the next
/// list member of a wire edge use.
///
/// # Errors
///
/// As [`mate`].
pub fn radial(brep: &Brep, eu: EdgeuseId) -> Result<Option<EdgeuseId>> {
    if is_shell_edgeuse(brep, eu)? {
        return edgeuse_next(brep, eu);
    }
    if primary_orientation(brep, eu)? == brep.edgeuse(eu)?.orientation {
        edgeuse_last(brep, eu)
    } else {
        edgeuse_next(brep, eu)
    }
}

/// `IsStrut`: a loop edge use whose radial partner is its clockwise neighbour (the
/// edge does not connect at this end).
///
/// # Errors
///
/// As [`radial`].
pub fn is_strut(brep: &Brep, eu: EdgeuseId) -> Result<bool> {
    let r = brep.edgeuse(eu)?;
    if matches!(r.owner, EdgeuseOwner::Loopuse(_)) {
        let cw = r.cw;
        return Ok(radial(brep, eu)? == cw);
    }
    Ok(false)
}

/// `GetCornerMateEdgeuse`: the loop neighbour sharing `vertex`: the clockwise one when
/// the edge use starts there, the counter-clockwise one when its mate does; `None`
/// otherwise and for a non-loop edge use.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; [`SolidError::InvalidInput`] for a
/// missing vertex use or mate upstream dereferences.
pub fn corner_mate_edgeuse(
    brep: &Brep,
    eu: EdgeuseId,
    vertex: Option<VertexId>,
) -> Result<Option<EdgeuseId>> {
    if !is_loop_edgeuse(brep, eu)? {
        return Ok(None);
    }
    let r = brep.edgeuse(eu)?;
    let vu = r
        .vertexuse
        .ok_or_else(|| null("GetCornerMateEdgeuse: edge use without vertex use"))?;
    if brep.vertexuse(vu)?.vertex == vertex {
        return Ok(r.cw);
    }
    let m = mate(brep, eu)?.ok_or_else(|| null("GetCornerMateEdgeuse: edge use without mate"))?;
    let mvu = brep
        .edgeuse(m)?
        .vertexuse
        .ok_or_else(|| null("GetCornerMateEdgeuse: mate without vertex use"))?;
    if brep.vertexuse(mvu)?.vertex == vertex {
        return Ok(r.ccw);
    }
    Ok(None)
}

/// `IsConnectedTo`: whether the edge use connects to `target` (its vertex use,
/// vertex, itself, edge, face, face use, loop, loop use, shell or region); `true` for
/// `None` and any other kind.
///
/// # Errors
///
/// Those of the queries reaching the target kind ([`face`], ...).
pub fn is_connected_to(brep: &Brep, eu: EdgeuseId, target: Option<EntityRef>) -> Result<bool> {
    // no work - no Topo
    let Some(target) = target else {
        return Ok(true);
    };
    match target {
        EntityRef::Vertexuse(vu) => Ok(vertexuse_edgeuse(brep, vu)? == Some(eu)),
        EntityRef::Vertex(v) => match vertexuse(brep, eu)? {
            Some(vu) => Ok(brep.vertexuse(vu)?.vertex == Some(v)),
            None => Ok(false),
        },
        EntityRef::Edgeuse(other) => Ok(other == eu),
        EntityRef::Edge(e) => Ok(edge(brep, eu)? == Some(e)),
        EntityRef::Face(f) => Ok(face(brep, eu)? == Some(f)),
        EntityRef::Faceuse(fu) => Ok(faceuse(brep, eu)? == Some(fu)),
        EntityRef::Loop(l) => match loopuse(brep, eu)? {
            Some(lu) => Ok(brep.loopuse(lu)?.loop_ == Some(l)),
            None => Ok(false),
        },
        EntityRef::Loopuse(lu) => Ok(loopuse(brep, eu)? == Some(lu)),
        EntityRef::Shell(s) => Ok(shell(brep, eu)? == Some(s)),
        EntityRef::Region(r) => match shell(brep, eu)? {
            Some(s) => Ok(shell_region(brep, s)? == Some(r)),
            None => Ok(false),
        },
        // arrive here when the target is an unsupported type - return TRUE
        EntityRef::Brep(_) => Ok(true),
    }
}

// ---- UV trim curves ---------------------------------------------------------------------

/// `GetUVTrimCurve`: the UV trim curve of the edge use or, when it holds none, of its
/// mate; with the edge use whose slot was read (the mate, possibly `None`, when the
/// edge use holds no curve).
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; those of [`mate`] when the edge use
/// holds no curve.
pub fn uv_trim_curve(brep: &Brep, eu: EdgeuseId) -> Result<(Option<&Curve>, Option<EdgeuseId>)> {
    let r = brep.edgeuse(eu)?;
    if let Some(c) = r.uv_trim_curve.as_ref() {
        return Ok((Some(c), Some(eu)));
    }
    let m = mate(brep, eu)?;
    match m {
        Some(m) => Ok((brep.edgeuse(m)?.uv_trim_curve.as_ref(), Some(m))),
        None => Ok((None, None)),
    }
}

/// `GetDropCurveFail`: the drop-curve record of the edge use or, when it holds no UV
/// trim curve, of its mate; with the edge use whose slot was read (as
/// [`uv_trim_curve`]).
///
/// # Errors
///
/// As [`uv_trim_curve`].
pub fn drop_curve_fail(
    brep: &Brep,
    eu: EdgeuseId,
) -> Result<(Option<&DropCurveFail<ObjectHandle>>, Option<EdgeuseId>)> {
    let r = brep.edgeuse(eu)?;
    if r.uv_trim_curve.is_some() {
        return Ok((r.drop_curve_fail.as_deref(), Some(eu)));
    }
    match mate(brep, eu)? {
        Some(m) => Ok((brep.edgeuse(m)?.drop_curve_fail.as_deref(), Some(m))),
        None => Ok((r.drop_curve_fail.as_deref(), None)),
    }
}

/// `GetUVTrimCurvePointer`: the edge use's own UV trim curve slot.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn uv_trim_curve_pointer(brep: &Brep, eu: EdgeuseId) -> Result<Option<&Curve>> {
    Ok(brep.edgeuse(eu)?.uv_trim_curve.as_ref())
}

/// The UV trim curve [`set_uv_trim_curve`] is given (upstream's `pUVTrimCurve`).
#[derive(Debug, Clone)]
pub enum UvTrimCurveArg {
    /// `NULL`: remove the attached curve.
    Null,
    /// The curve already attached (`GetUVTrimCurve()`); `NULL` when none is.
    Current,
    /// A new curve; ownership passes to the edge use.
    New(Box<Curve>),
}

/// The edge use whose drop-curve record `GetDropCurveFail` reads: the edge use itself
/// when it holds a UV trim curve or has no mate, else its mate.
fn drop_curve_fail_slot(
    brep: &Brep,
    eu: EdgeuseId,
    mate_eu: Option<EdgeuseId>,
) -> Result<EdgeuseId> {
    if brep.edgeuse(eu)?.uv_trim_curve.is_some() {
        return Ok(eu);
    }
    Ok(mate_eu.unwrap_or(eu))
}

/// The handle a drop-curve record keeps for the edge use's B-rep (`GetBrep()`).
pub(crate) fn brep_handle(b: &Brep, eu: EdgeuseId) -> Result<Option<ObjectHandle>> {
    Ok(brep(b, eu)?.map(|x| ObjectHandle {
        kind: ObjectKind::Other,
        key: x.key(),
    }))
}

/// `SetUVTrimCurve`: attaches `curve` to the [`OrientType::Same`] member of the
/// mated pair (to the edge use itself without mate), stores a copy of
/// `drop_curve_fail` beside it, resets both vertex uses' vertex/face gaps, and raises
/// the edge tolerance to `2 * max_dist_to_surface` unless the B-rep only validates.
/// The curve attached before is removed: dropped with `delete_old`, else returned.
/// When `curve` is the attached curve (or both are `NULL`) nothing changes except the
/// attached drop-curve record, which takes `drop_curve_fail` (whose B-rep is set).
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; [`SolidError::InvalidInput`] where
/// upstream dereferences null (no edge, primary edge use, vertex use or B-rep);
/// [`SolidError::Unported`] when growing the edge tolerance must grow a vertex
/// tolerance (`SmVertex::SetTolerance`, unit `topology.vertex`).
pub fn set_uv_trim_curve(
    b: &mut Brep,
    eu: EdgeuseId,
    curve: UvTrimCurveArg,
    max_dist_to_surface: f64,
    delete_old: bool,
    drop_curve_fail: Option<&mut DropCurveFail<ObjectHandle>>,
) -> Result<Option<Curve>> {
    // locals
    let mate_eu = mate(b, eu)?;
    let (old_present, old_slot) = {
        let (c, owner) = uv_trim_curve(b, eu)?;
        (c.is_some(), owner)
    };
    let fail_slot = drop_curve_fail_slot(b, eu, mate_eu)?;

    // no work - pUVTrimCurve already saved
    let same = match &curve {
        UvTrimCurveArg::Null => !old_present,
        UvTrimCurveArg::Current => true,
        UvTrimCurveArg::New(_) => false,
    };
    if same {
        // when given a DropCurveFail - update the current DropCurveFail values
        if let Some(f) = drop_curve_fail {
            f.brep = brep_handle(b, eu)?;
            if let Some(old) = b.edgeuse_mut(fail_slot)?.drop_curve_fail.as_deref_mut() {
                old.assign(f);
            }
            // else: upstream copies it into a local that is never stored
        }
        return Ok(None);
    }
    let new_curve = match curve {
        UvTrimCurveArg::New(c) => Some(*c),
        UvTrimCurveArg::Null | UvTrimCurveArg::Current => None,
    };

    // take the old UVTrimCurve off; delete it when asked
    let mut detached = None;
    if old_present {
        if let Some(slot) = old_slot {
            let old = b.edgeuse_mut(slot)?.uv_trim_curve.take();
            if !delete_old {
                detached = old;
            }
        }
    }

    // delete oldDropCurveFail and clear old UVTrimCurve pointers
    for x in [Some(eu), mate_eu].into_iter().flatten() {
        let r = b.edgeuse_mut(x)?;
        r.uv_trim_curve = None;
        r.drop_curve_fail = None;
    }

    // A new UVTrimCurve can change a Vertexuse UV point. So clear the old VertexFaceGaps.
    for x in [Some(eu), mate_eu].into_iter().flatten() {
        let vu = b
            .edgeuse(x)?
            .vertexuse
            .ok_or_else(|| null("SetUVTrimCurve: edge use without vertex use"))?;
        b.vertexuse_mut(vu)?.vertex_face_gap.reset();
    }

    // if given a New UVTrimCurve - save it
    if let Some(mut c) = new_curve {
        // `new SmDropCurveFail(*pOptDropCurveFail)`: the copy constructor copies the
        // UV curves only with a context, as `operator=`
        let copy = drop_curve_fail.map(|f| {
            let mut c = DropCurveFail::default();
            c.assign(f);
            Box::new(c)
        });
        let holder = match mate_eu {
            Some(m) => {
                if b.edgeuse(eu)?.orientation == OrientType::Same {
                    eu
                } else {
                    m
                }
            }
            // a UVTrimCurve is being temporarily placed onto a standalone Edgeuse
            None => eu,
        };
        // pUVTrimCurve->SetOwner(holder), under the B-rep's naming (a UV trim curve
        // notifies nothing)
        if let Some(owner) = owner_ref_in(b, holder) {
            let _ = c.set_owner(Some(owner), None);
        }
        let r = b.edgeuse_mut(holder)?;
        r.uv_trim_curve = Some(c);
        r.drop_curve_fail = copy;

        // if only validating, then do not allow tolerances to change
        let validate_only = brep(b, eu)?
            .ok_or_else(|| null("SetUVTrimCurve: edge use without B-rep"))?
            .validate_only();
        if !validate_only {
            const TOL_FACTOR: f64 = 2.0;
            let e = edge(b, eu)?.ok_or_else(|| null("SetUVTrimCurve: edge use without edge"))?;
            let curr_tol = b.edge(e)?.tolerance;
            if curr_tol < TOL_FACTOR * max_dist_to_surface {
                crate::topology::edge::set_tolerance(
                    b,
                    e,
                    TOL_FACTOR * max_dist_to_surface,
                    true,
                    true,
                )?;
            }
        }
    }
    Ok(detached)
}

/// The identity edge use `eu` of `b` owns geometry under: for a
/// [`NamingScheme::Caller`] B-rep the caller's key recorded for it where the B-rep was
/// built (`Brep::geometry_owner_key`; `None` when none is recorded), else
/// [`owner_ref`] of `b`'s naming.
pub(crate) fn owner_ref_in(b: &Brep, eu: EdgeuseId) -> Option<ObjectRef> {
    match b.naming() {
        NamingScheme::Caller => b.geometry_owner_key(EntityRef::Edgeuse(eu)),
        naming => owner_ref(naming, eu),
    }
}

/// The identity edge use `eu` owns geometry under in a B-rep with naming `naming`
/// ([`NamingScheme`]); `None` when the B-rep names its owners by a caller's keys (see
/// [`owner_ref_in`]). Both the notify and the arena scheme name an edge use by its
/// notification identity: [`Brep::use_arena_naming`] rebinds edge uses' UV trim curves
/// to it, and the arena keys of `check_brep` and the merge's `adopt_namespace_owners`
/// fall back to it for edge uses. A B-rep that records no scheme names its edge uses as the
/// arena scheme does: upstream `SetUVTrimCurve` always owns the new curve by its holder
/// (`pUVTrimCurve->SetOwner(eu)`), so leaving it without owner would differ from
/// upstream for every B-rep made by `Brep::new` / `Brep::new_with_infinite_region`
/// (`plan/findings/topology.edgeuse_p1--uv-trim-owner-under-unknown-naming.md`).
pub(crate) fn owner_ref(naming: NamingScheme, eu: EdgeuseId) -> Option<ObjectRef> {
    match naming {
        NamingScheme::Caller => None,
        NamingScheme::Unknown | NamingScheme::Notify | NamingScheme::Arena => {
            Some(crate::topology::loopuse::notify_ref(EntityRef::Edgeuse(eu)))
        }
    }
}

/// `DeleteUVTrimCurve`: `SetUVTrimCurve(NULL, 0.0, TRUE, NULL)`.
///
/// # Errors
///
/// As [`set_uv_trim_curve`].
pub fn delete_uv_trim_curve(b: &mut Brep, eu: EdgeuseId) -> Result<()> {
    set_uv_trim_curve(b, eu, UvTrimCurveArg::Null, 0.0, true, None).map(|_| ())
}

/// The curve and drop-curve record [`set_member_uv_trim_curve`] replaced.
pub type ReplacedUvTrimCurve = (Option<Curve>, Option<Box<DropCurveFail<ObjectHandle>>>);

/// `SetMemberUVTrimCurve`: places `curve` (and `drop_curve_fail` when given) on the
/// [`OrientType::Same`] member of the mated pair (this edge use when it is the `SAME`
/// one, else its mate) and makes that member the curve's owner, without any other
/// bookkeeping; returns what the slots held.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; [`SolidError::InvalidInput`] when the
/// owner is a missing mate (upstream dereferences it); those of [`mate`].
pub fn set_member_uv_trim_curve(
    b: &mut Brep,
    eu: EdgeuseId,
    mut curve: Curve,
    drop_curve_fail: Option<DropCurveFail<ObjectHandle>>,
) -> Result<ReplacedUvTrimCurve> {
    let owner = if b.edgeuse(eu)?.orientation == OrientType::Same {
        eu
    } else {
        mate(b, eu)?.ok_or_else(|| null("SetMemberUVTrimCurve: edge use without mate"))?
    };
    // pUVTrimCurve->SetOwner(pOwner), under the B-rep's naming (a UV trim curve notifies
    // nothing), as `set_uv_trim_curve`
    if let Some(key) = owner_ref_in(b, owner) {
        let _ = curve.set_owner(Some(key), None);
    }
    let r = b.edgeuse_mut(owner)?;
    let old_curve = r.uv_trim_curve.replace(curve);
    let old_fail = match drop_curve_fail {
        Some(f) => r.drop_curve_fail.replace(Box::new(f)),
        None => None,
    };
    Ok((old_curve, old_fail))
}

// ---- gaps -------------------------------------------------------------------------------

/// The status of a gap calculation the `Get*Gap` callers ignore: an upstream status is
/// dropped; [`SolidError::Unported`], [`SolidError::InvalidInput`] (a null pointer
/// upstream dereferences) and [`SolidError::NotFound`] propagate.
fn ignore_status<T>(r: Result<T>) -> Result<()> {
    match r {
        Err(e @ (SolidError::Upstream { .. } | SolidError::Nlib { .. })) => {
            let _ = e;
            Ok(())
        }
        Err(e) => Err(e),
        Ok(_) => Ok(()),
    }
}

/// `GetMaxEdgeFaceGap`: the cached largest edge/face gap of a loop edge use
/// (calculated first when `force_calc` or not cached); `None` for other edge uses.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; [`SolidError::Unported`] and
/// [`SolidError::InvalidInput`] from the calculation (its status is ignored, as
/// upstream).
pub fn max_edge_face_gap(
    k: &Kernel,
    b: &mut Brep,
    eu: EdgeuseId,
    force_calc: bool,
) -> Result<Option<EdgeFaceGap>> {
    // no work - no Gap
    if !has_edge_face_gap(b, eu)? {
        return Ok(None);
    }
    if force_calc || !b.edgeuse(eu)?.max_edge_face_gap.is_init() {
        ignore_status(calc_max_edge_face_gap(k, b, eu, true))?;
    }
    Ok(Some(b.edgeuse(eu)?.max_edge_face_gap))
}

/// `GetEdgeCCWEdgeGap`: the cached gap between a loop edge use's end and its
/// counter-clockwise neighbour's start (calculated first when `force_calc` or not
/// cached); `None` for other edge uses.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; [`SolidError::Unported`] and
/// [`SolidError::InvalidInput`] from the calculation (its status is ignored, as
/// upstream).
pub fn edge_ccw_edge_gap(
    k: &Kernel,
    b: &mut Brep,
    eu: EdgeuseId,
    force_calc: bool,
) -> Result<Option<EdgeEdgeGap>> {
    // no work - no gap
    if !has_edge_ccw_edge_gap(b, eu)? {
        return Ok(None);
    }
    if force_calc || !b.edgeuse(eu)?.edge_ccw_edge_gap.is_init() {
        ignore_status(calc_edge_ccw_edge_gap(k, b, eu, false, true))?;
    }
    Ok(Some(b.edgeuse(eu)?.edge_ccw_edge_gap))
}

/// `CalcMaxEdgeFaceGap`: the largest gap between the edge's curve and the face's
/// surface (the longest sample of an `SmCrvSrfGapFunction` with 20 samples over the
/// edge interval), cached on the edge use and its mate when `cache` (the caches are
/// reset first). Non-loop edge uses have no such gap: the reset gap is returned.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; [`SolidError::InvalidInput`] where
/// upstream dereferences null (no face, edge, curve or surface); `SM_ERR` for a
/// degenerate edge interval; those of [`GapFunction::max_gap_sample`] and
/// [`EdgeFaceGap::set`] ([`SolidError::Unported`]: the surface drop).
pub fn calc_max_edge_face_gap(
    k: &Kernel,
    b: &mut Brep,
    eu: EdgeuseId,
    cache: bool,
) -> Result<EdgeFaceGap> {
    // init output
    let out = EdgeFaceGap::new();

    // Get Mate (Mates share the same gaps)
    let mate_eu = mate(b, eu)?;

    // when asked - clear cache
    if cache {
        b.edgeuse_mut(eu)?.max_edge_face_gap.reset();
        if let Some(m) = mate_eu {
            b.edgeuse_mut(m)?.max_edge_face_gap.reset();
        }
    }

    // no work - no VertexEdge Gap
    if !has_edge_face_gap(b, eu)? {
        return Ok(out);
    }

    // Locals for Edge/UVTrimCurve gap calc
    let e = edge(b, eu)?.ok_or_else(|| null("CalcMaxEdgeFaceGap: edge use without edge"))?;
    let f = face(b, eu)?.ok_or_else(|| null("CalcMaxEdgeFaceGap: edge use without face"))?;

    // avoid infinite loop - don't call SmTol::GetXSectTol3d(pEdge,pFace); the context
    // tolerance is only used for the gap function
    let xsect_tol = context_xsect_tol_3d(k, None, SmallTopoType::NormalSize);

    // construct gap function
    let interval = b.edge(e)?.interval;
    if interval.min() == interval.max() {
        return Err(SolidError::sm_err(OP));
    }
    let sample = {
        let curve = b
            .edge(e)?
            .curve
            .as_ref()
            .ok_or_else(|| null("CalcMaxEdgeFaceGap: edge without curve"))?;
        let surface = b
            .face(f)?
            .surface
            .as_ref()
            .ok_or_else(|| null("CalcMaxEdgeFaceGap: face without surface"))?;
        let mut gf = GapFunction::crv_srf(xsect_tol, curve, &interval, surface, 20);
        // get max gap sample - forces lazy evaluation
        *gf.max_gap_sample(k, false)?
    };

    // set Edge/Face gap - assigns m_pEdgeuse, m_dEdgeT and m_sFaceUV and computes the
    // gap
    let gap = EdgeFaceGap::with(
        k,
        b,
        Some(eu),
        sample.this_param().x,
        Some(sample.other_param()),
    )?;
    if cache {
        b.edgeuse_mut(eu)?.max_edge_face_gap = gap;
        // keep Mate gaps current
        if let Some(m) = mate_eu {
            let r = b.edgeuse_mut(m)?;
            r.max_edge_face_gap = gap;
            r.max_edge_face_gap.set_edgeuse(Some(m));
        }
    }

    // all done
    Ok(gap)
}

/// `CalcEdgeCCWEdgeGap`: the gap between the edge use's end and its counter-clockwise
/// neighbour's start, cached on the edge use when `cache` (the cache is reset first)
/// and returned when `want_output`. Non-loop edge uses have no such gap.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; [`SolidError::InvalidInput`] where
/// upstream dereferences null (no counter-clockwise neighbour or edge);
/// those of [`EdgeEdgeGap::set_edge_edge_gap`] when the gap is cached or wanted.
pub fn calc_edge_ccw_edge_gap(
    k: &Kernel,
    b: &mut Brep,
    eu: EdgeuseId,
    want_output: bool,
    cache: bool,
) -> Result<Option<EdgeEdgeGap>> {
    // init output
    let out = want_output.then(EdgeEdgeGap::new);

    // locals
    let ccw = ccw_edgeuse(b, eu)?;

    // when asked - clear cache
    if cache {
        b.edgeuse_mut(eu)?.edge_ccw_edge_gap.reset();
    }

    // no work - no VertexEdge Gap
    if !has_edge_ccw_edge_gap(b, eu)? {
        return Ok(out);
    }

    // Locals for Edge/UVTrimCurve gap calc
    let e = edge(b, eu)?.ok_or_else(|| null("CalcEdgeCCWEdgeGap: edge use without edge"))?;
    let ivl = b.edge(e)?.interval;
    let edge_t = if orientation(b, eu)? == OrientType::Same {
        ivl.max()
    } else {
        ivl.min()
    };
    let ccw = ccw.ok_or_else(|| null("CalcEdgeCCWEdgeGap: edge use without CCW neighbour"))?;
    let ce = edge(b, ccw)?.ok_or_else(|| null("CalcEdgeCCWEdgeGap: CCW edge use without edge"))?;
    let civl = b.edge(ce)?.interval;
    let ccw_edge_t = if orientation(b, ccw)? == OrientType::Same {
        civl.min()
    } else {
        civl.max()
    };

    // set m_sEdgeCCWEdgeGap3d
    if cache {
        let g = EdgeEdgeGap::with(k, b, Some(eu), Some(ccw), edge_t, ccw_edge_t)?;
        b.edgeuse_mut(eu)?.edge_ccw_edge_gap = g;
    }

    // set output
    match out {
        Some(_) => Ok(Some(EdgeEdgeGap::with(
            k,
            b,
            Some(eu),
            Some(ccw),
            edge_t,
            ccw_edge_t,
        )?)),
        None => Ok(None),
    }
}

// ---- UV loop gap ------------------------------------------------------------------------

/// A curve point at `t`, `SM_UNDEF_DOUBLE` where upstream ignores a failed
/// evaluation (only [`SolidError::Unported`] propagates; with the opt-in
/// [`crate::kernel::UpstreamFixes::topology_failed_evaluations_propagate`] every error).
fn point_or_undef(k: &Kernel, r: Result<Point3>) -> Result<Point3> {
    if k.fixes.topology_failed_evaluations_propagate {
        return r;
    }
    Ok(crate::error::ignored(r)?.unwrap_or(Vec3::UNDEF))
}

/// The edge interval of `eu`'s edge.
fn edge_interval(b: &Brep, eu: EdgeuseId, what: &'static str) -> Result<Extent1d> {
    let e = edge(b, eu)?.ok_or_else(|| null(what))?;
    Ok(b.edge(e)?.interval)
}

/// `GetStartPointUVLoopGap`: the UV distance between this edge use's start and its
/// clockwise neighbour's end, from the UV trim curves when both have one, else by
/// dropping the 3D end points onto the face's surface; `0` for a non-loop edge use,
/// one without clockwise neighbour, or a face without surface.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; [`SolidError::InvalidInput`] where
/// upstream dereferences null; [`SolidError::Unported`] for the point drop
/// (`SmSurface::DropPoint`, unit `surface.surface_p5`). With the opt-in
/// `Kernel::fixes.topology_failed_evaluations_propagate`, also curve and point-drop
/// errors, including `SM_ERR` when a drop does not converge.
pub fn start_point_uv_loop_gap(k: &Kernel, b: &Brep, eu: EdgeuseId) -> Result<f64> {
    // check state - only Loopuse Edgeuses have End_to_End gaps
    let r = b.edgeuse(eu)?;
    let Some(cw) = r.cw.filter(|_| matches!(r.owner, EdgeuseOwner::Loopuse(_))) else {
        return Ok(0.0);
    };

    // locals
    let this_ivl = edge_interval(b, eu, "GetStartPointUVLoopGap: edge use without edge")?;
    let start_param = if r.orientation == OrientType::Same {
        this_ivl.min()
    } else {
        this_ivl.max()
    };
    let cw_ivl = edge_interval(b, cw, "GetStartPointUVLoopGap: CW edge use without edge")?;
    let cw_end_param = if b.edgeuse(cw)?.orientation == OrientType::Same {
        cw_ivl.max()
    } else {
        cw_ivl.min()
    };
    let this_uv = uv_trim_curve(b, eu)?.0;
    let cw_uv = uv_trim_curve(b, cw)?.0;

    // When UVTrimCurves are available - use them to get EndPt UV Values
    let (Some(tc), Some(cc)) = (this_uv, cw_uv) else {
        // Drop 3d Points to Surface
        let surface = match face(b, eu)? {
            Some(f) => b.face(f)?.surface.as_ref(),
            None => None,
        };
        // check state - must have a surface
        let Some(surface) = surface else {
            return Ok(0.0);
        };
        // evaluate Edgeuse End 3dPoints
        let curve_of = |x: EdgeuseId| -> Result<_> {
            let e = edge(b, x)?.ok_or_else(|| null("GetStartPointUVLoopGap: no edge"))?;
            b.edge(e)?
                .curve
                .as_ref()
                .ok_or_else(|| null("GetStartPointUVLoopGap: edge without curve"))
        };
        let start_3d = point_or_undef(k, curve_of(eu)?.evaluate_point(k, start_param))?;
        let cw_end_3d = point_or_undef(k, curve_of(cw)?.evaluate_point(k, cw_end_param))?;

        // drop point to Surface (statuses ignored, as upstream; with the opt-in fix a
        // failed drop supplies neither a valid point nor the next guess)
        let fix = k.fixes.topology_failed_evaluations_propagate;
        let domain = surface.natural_uv_domain_with_nurb(k)?;
        let start_opts = DropPointOptions {
            operation: SolverOperation::Minimize,
            ..Default::default()
        };
        let start_drop = surface.drop_point::<NoObject>(k, start_3d, &domain, &start_opts);
        let start_2d = if fix {
            let d = start_drop?;
            if !d.success {
                return Err(SolidError::sm_err(
                    "topology::edgeuse::p1::start_point_uv_loop_gap",
                ));
            }
            d.result_uv
        } else {
            // `sStart2d` stays default-constructed when the call fails
            crate::error::ignored(start_drop)?.map_or(Point2::UNDEF, |d| d.result_uv)
        };
        let cw_opts = DropPointOptions {
            guess_uv: Some(start_2d),
            operation: SolverOperation::Minimize,
            ..Default::default()
        };
        let cw_drop = surface.drop_point::<NoObject>(k, cw_end_3d, &domain, &cw_opts);
        let cw_end_2d = if fix {
            let d = cw_drop?;
            if !d.success {
                return Err(SolidError::sm_err(
                    "topology::edgeuse::p1::start_point_uv_loop_gap",
                ));
            }
            d.result_uv
        } else {
            // upstream's DropPoint sets rResultUV to the guess before anything can fail
            crate::error::ignored(cw_drop)?.map_or(start_2d, |d| d.result_uv)
        };
        return Ok(start_2d.distance(cw_end_2d));
    };
    let start_uv = point_or_undef(k, tc.evaluate_point(k, start_param))?;
    let cw_end_uv = point_or_undef(k, cc.evaluate_point(k, cw_end_param))?;

    // compute and return the EdgeEnd to EdgeEnd gap
    Ok(start_uv.distance(cw_end_uv))
}

// ---- face uses --------------------------------------------------------------------------

/// `SmFaceuse::GetShell`: the owning shell.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn faceuse_shell(b: &Brep, fu: FaceuseId) -> Result<Option<ShellId>> {
    Ok(b.faceuse(fu)?.shell)
}

/// `SmFaceuse::GetRegion`: the owning shell's region.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn faceuse_region(b: &Brep, fu: FaceuseId) -> Result<Option<RegionId>> {
    match b.faceuse(fu)?.shell {
        Some(s) => shell_region(b, s),
        None => Ok(None),
    }
}

/// `SmFaceuse::GetFace`.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn faceuse_face(b: &Brep, fu: FaceuseId) -> Result<Option<FaceId>> {
    Ok(b.faceuse(fu)?.face)
}

/// `SmFaceuse::GetMate`: the face's other face use.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn faceuse_mate(b: &Brep, fu: FaceuseId) -> Result<Option<FaceuseId>> {
    Ok(b.faceuse(fu)?.mate)
}

/// `SmFaceuse::GetOrientation`.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn faceuse_orientation(b: &Brep, fu: FaceuseId) -> Result<OrientType> {
    Ok(b.faceuse(fu)?.orientation)
}

/// `SmFaceuse::GetLoopuses`: all loop uses; with `target_loops_only`, the outer loop
/// use (`target_loops` `None` or empty) or the uses of `target_loops` on this face use.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; [`SolidError::InvalidInput`] for a
/// target loop without loop use or other loop use (upstream dereferences them).
pub fn faceuse_loopuses(
    b: &Brep,
    fu: FaceuseId,
    target_loops_only: bool,
    target_loops: Option<&[LoopId]>,
) -> Result<Vec<LoopuseId>> {
    let list = &b.faceuse(fu)?.loopuses;
    if !target_loops_only {
        // include all loopuses branch
        return Ok(list.clone());
    }
    let loops = match target_loops {
        Some(l) if !l.is_empty() => l,
        // Only use OuterLoop branch
        _ => return Ok(list.first().copied().into_iter().collect()),
    };
    // Use given Loops branch
    let mut out = Vec::new();
    for &l in loops {
        let lu = b
            .loop_(l)?
            .loopuse
            .ok_or_else(|| null("GetLoopuses: loop without loop use"))?;
        if b.loopuse(lu)?.faceuse == Some(fu) {
            out.push(lu);
            continue;
        }
        let other = b
            .loopuse(lu)?
            .mate
            .ok_or_else(|| null("GetLoopuses: loop use without other loop use"))?;
        if b.loopuse(other)?.faceuse == Some(fu) {
            out.push(other);
        }
    }
    Ok(out)
}

/// `SmFaceuse::GetEdgeuses`: the distinct edge uses of the loop uses
/// [`faceuse_loopuses`] selects, in loop and CCW order.
///
/// # Errors
///
/// Those of [`faceuse_loopuses`] and [`loopuse_edgeuses`].
pub fn faceuse_edgeuses(
    b: &Brep,
    fu: FaceuseId,
    target_loops_only: bool,
    target_loops: Option<&[LoopId]>,
) -> Result<Vec<EdgeuseId>> {
    let mut out: Vec<EdgeuseId> = Vec::new();
    for lu in faceuse_loopuses(b, fu, target_loops_only, target_loops)? {
        for eu in loopuse_edgeuses(b, lu)? {
            if !out.contains(&eu) {
                out.push(eu);
            }
        }
    }
    Ok(out)
}

/// `SmFaceuse::GetNeighbors`: the distinct face uses across the radial partners of
/// every edge use (the face use itself when it is its own neighbour); with
/// `skip_lamina_edges`, neighbours on the same face are skipped.
///
/// # Errors
///
/// Those of [`loopuse_edgeuses`] and [`radial`]; [`SolidError::InvalidInput`] for an
/// edge use without face use when lamina edges are skipped.
pub fn faceuse_neighbors(
    b: &Brep,
    fu: FaceuseId,
    skip_lamina_edges: bool,
) -> Result<Vec<FaceuseId>> {
    let mut out: Vec<FaceuseId> = Vec::new();
    for lu in faceuse_loopuses(b, fu, false, None)? {
        for eu in loopuse_edgeuses(b, lu)? {
            let Some(rad) = radial(b, eu)? else {
                continue;
            };
            let Some(neighbor) = faceuse(b, rad)? else {
                continue;
            };
            // skip lamina edges when asked
            if skip_lamina_edges {
                let own = faceuse(b, eu)?
                    .ok_or_else(|| null("GetNeighbors: edge use without face use"))?;
                if b.faceuse(neighbor)?.face == b.faceuse(own)?.face {
                    continue;
                }
            }
            if !out.contains(&neighbor) {
                out.push(neighbor);
            }
        }
    }
    Ok(out)
}

/// `pEU->EvaluateBinormal(param, TRUE, point, binormal, NULL, &normal)` with its status
/// ignored: the face use normal, a zero vector where the evaluation failed (upstream
/// zeroes the output before it can fail). [`SolidError::Unported`] propagates.
fn binormal_face_normal(k: &Kernel, b: &mut Brep, eu: EdgeuseId, param: f64) -> Result<Vec3> {
    let options = crate::topology::edgeuse::p3::BinormalOptions::default();
    Ok(
        crate::error::ignored(crate::topology::edgeuse::p3::evaluate_binormal(
            k, b, eu, param, true, &options,
        ))?
        .map_or(Vec3::new(0.0, 0.0, 0.0), |r| r.face_normal),
    )
}

/// `SmFaceuse::CollectAdjacentFaceuses`: appends to `faceuses` the face uses reached
/// from this one across radial partners, not crossing lamina edges (and, with an
/// angle threshold, not across edges whose face normals differ by more).
///
/// # Errors
///
/// Those of [`faceuse_edgeuses`] and [`radial`]; [`SolidError::InvalidInput`] where
/// upstream dereferences null (no edge, radial partner or its face use);
/// [`SolidError::Unported`] and the binormal errors with a threshold
/// (`SmEdgeuse::EvaluateBinormal`, [`crate::topology::edgeuse::p3`]).
pub fn faceuse_collect_adjacent(
    k: &Kernel,
    b: &mut Brep,
    fu: FaceuseId,
    faceuses: &mut Vec<FaceuseId>,
    angle_threshold: Option<f64>,
) -> Result<()> {
    // init faceuse stack
    let mut stack = vec![fu];

    // while there are faceuses on the stack
    while let Some(top) = stack.pop() {
        for eu in faceuse_edgeuses(b, top, false, None)? {
            // skip lamina edges
            let e = edge(b, eu)?
                .ok_or_else(|| null("CollectAdjacentFaceuses: edge use without edge"))?;
            if edge_is_lamina(b, e)? {
                continue;
            }

            // add the radial partner's face to output list and faceuse stack
            let rad = radial(b, eu)?
                .ok_or_else(|| null("CollectAdjacentFaceuses: edge use without radial partner"))?;
            let fu_rad = faceuse(b, rad)?;

            if let Some(threshold) = angle_threshold {
                // the face use normals at the start, middle and end of the edge must
                // differ by no more than the threshold. The binormal statuses are
                // ignored upstream: `EvaluateBinormal` zeroes `*pOptFaceuseNormal`
                // before it can fail, so a failing evaluation leaves a zero normal, and
                // `AngleBetween` of a zero vector fails with `rdAngRad == 0` (so the
                // angle is 0 and the face use is kept). Only `Unported` propagates.
                let ivl = b.edge(e)?.interval;
                let mut too_far = false;
                for param in [ivl.min(), ivl.mid(), ivl.max()] {
                    let n_eu = binormal_face_normal(k, b, eu, param)?;
                    let n_rad = binormal_face_normal(k, b, rad, param)?;
                    // (status ignored; the angle is 0 on failure)
                    let angle = crate::error::ignored(n_eu.angle_between(n_rad))?.unwrap_or(0.0);
                    if angle > threshold {
                        too_far = true;
                        break;
                    }
                }
                if too_far {
                    continue;
                }
            }

            if fu_rad != Some(fu) && !fu_rad.is_some_and(|x| faceuses.contains(&x)) {
                // a NULL face use would be dereferenced when popped
                let x = fu_rad.ok_or_else(|| {
                    null("CollectAdjacentFaceuses: radial partner without face use")
                })?;
                stack.push(x);
                faceuses.push(x);
            }
        }
    }
    Ok(())
}

// ---- file-static helpers ------------------------------------------------------------------

/// `sm_CalcDistAndDev`: for a point dropped onto a surface, its distance along the
/// surface normal and its deviation in the normal plane (`want_dev`), or the
/// solution's value as the distance (without `want_dev`). Both are `-1` when the
/// solution's first object is not a surface or the normal is undefined. `surface_of`
/// resolves a surface handle.
///
/// # Errors
///
/// [`SolidError::Unported`] from the surface evaluations (their other failures are
/// ignored, as upstream). With the opt-in
/// `Kernel::fixes.topology_failed_evaluations_propagate` (WR-264), every error of the
/// surface evaluations: failed derivatives cannot supply a distance.
pub fn calc_dist_and_dev<'s, O: SolutionObject>(
    k: &Kernel,
    pt: Point3,
    sol: &Solution<O>,
    surface_of: impl FnOnce(O) -> Option<&'s Surface>,
    want_dev: bool,
) -> Result<(f64, Option<f64>)> {
    if !want_dev {
        return Ok((sol.start.solution_value, None));
    }

    // pDev is not Null.
    let mut dist = -1.0;
    let mut dev = -1.0;

    let srf = sol
        .objects
        .first()
        .copied()
        .flatten()
        .filter(|o| o.kind().is_kind_of(ObjectKind::Surface))
        .and_then(surface_of);
    let Some(srf) = srf else {
        return Ok((dist, Some(dev)));
    };

    // Find distance (along surface normal) and deviation (in normal plane).
    let [u, v, ..] = sol.start.parameters;
    let uv = Point2::new(u, v);
    let derivs = evaluate_1st_derivatives(k, srf, uv, Side::Left, Side::Left);
    let (s_pt, s_su, s_sv) = if k.fixes.topology_failed_evaluations_propagate {
        derivs?
    } else {
        crate::error::ignored(derivs)?.unwrap_or((Vec3::UNDEF, Vec3::UNDEF, Vec3::UNDEF))
    };

    let mut norm = s_su.cross(s_sv);
    let mag = norm.length();
    if mag > EFF_ZERO {
        norm /= mag;
    } else {
        norm = srf.evaluate_normal(k, uv, Side::Left, Side::Left)?;
        if norm.length() < 0.5 {
            return Ok((dist, Some(dev)));
        }
    }

    let diff = pt - s_pt;
    let dot = diff.dot(norm);
    dist = dot.abs();

    let d3d_dist = diff.length();

    // Pythagorean:
    dev = if dist >= d3d_dist {
        0.0 // (Shouldn't happen, just a check for Sqrt.)
    } else {
        crate::geom::vector3d::sqrt(d3d_dist * d3d_dist - dist * dist)
    };

    Ok((dist, Some(dev)))
}

/// `sm_AreSeamCurves`: whether two UV curves represent a surface seam: always when
/// `DropCurve` made them, else when their natural intervals agree with each other
/// and, to 1% of its length, with the edge domain.
#[must_use]
pub fn are_seam_curves(
    uv_curves: &[&Curve],
    edge_domain: &Extent1d,
    drop_curve_called: bool,
) -> bool {
    let [c1, c2] = uv_curves else {
        return false;
    };

    if drop_curve_called {
        return true; // DropCurve() will never return two curves except for seams.
    }

    // Check curve domains.
    let dom1 = c1.natural_interval();
    let dom2 = c2.natural_interval();
    if dom1.are_same(&dom2) {
        // The check vs. the Edge domain can have a little slop?  Say 1%.
        let tol = edge_domain.length() / 100.0;
        if dom1.are_equal(edge_domain, tol) {
            return true;
        }
    }
    false
}

/// The equations of a step-off point (`SmFindStepOffENFO`): the point on `surface`
/// at distance `step_off_dist` from `point` in the plane through `point` with normal
/// `plane_normal`, in the surface parameters `x = [u v]`:
/// `F0 = |S - P|^2 - d^2`, `F1 = S . N - P . N`.
#[derive(Debug, Clone, Copy)]
pub struct FindStepOff<'a> {
    /// Caller context retained by evaluation callbacks.
    pub kernel: &'a Kernel,

    /// `m_dStepOffDist`.
    step_off_dist: f64,
    /// `m_crPoint`.
    point: Point3,
    /// `m_crPlaneNormal`.
    plane_normal: Vec3,
    /// `m_crSurface`.
    surface: &'a Surface,
}

impl<'a> FindStepOff<'a> {
    /// The equations for `point` on `surface` (the constructor).
    #[must_use]
    pub const fn new(
        k: &'a Kernel,
        step_off_dist: f64,
        point: Point3,
        plane_normal: Vec3,
        surface: &'a Surface,
    ) -> Self {
        Self {
            kernel: k,
            step_off_dist,
            point,
            plane_normal,
            surface,
        }
    }
}

impl EvalNFunctions for FindStepOff<'_> {
    /// `Evaluate`: `F` and, when asked, the Jacobian
    /// `[[2 Su.(S-P), 2 Sv.(S-P)], [Su.N, Sv.N]]` at `x`; never reports an answer.
    ///
    /// Errors: the surface evaluation's; `SM_ERR` for a (nearly) zero first partial;
    /// `InvalidInput` for `x` or `f` shorter than 2 or a Jacobian smaller than 2x2.
    fn evaluate(
        &mut self,
        x: &[f64],
        f: &mut [f64],
        jacobian: Option<&mut Matrix>,
    ) -> Result<bool> {
        let k = self.kernel;
        let (Some(&u), Some(&v)) = (x.first(), x.get(1)) else {
            return Err(SolidError::invalid_input(
                OP_STEP,
                "parameter array shorter than 2",
            ));
        };
        if f.len() < 2 {
            return Err(SolidError::invalid_input(
                OP_STEP,
                "function array shorter than 2",
            ));
        }

        let (s_point, s_du, s_dv) =
            evaluate_1st_derivatives(k, self.surface, Point2::new(u, v), Side::Left, Side::Left)?;

        let du_len_sq = s_du.length_squared();
        let dv_len_sq = s_dv.length_squared();
        // if we have a zero length tangent then we must fail
        if du_len_sq < EFF_ZERO_SQ || dv_len_sq < EFF_ZERO_SQ {
            return Err(SolidError::sm_err(OP_STEP));
        }

        let s_minus_p = s_point - self.point;

        // Solve the following two equations.  What they do is find the
        // intersection of a plane and a surface at a given distance
        // from the original point.
        let plane_d = -self.plane_normal.dot(self.point);

        if let Some(jac) = jacobian {
            jac.set_at(0, 0, 2.0 * s_du.dot(s_minus_p))?;
            jac.set_at(0, 1, 2.0 * s_dv.dot(s_minus_p))?;
            jac.set_at(1, 0, s_du.dot(self.plane_normal))?;
            jac.set_at(1, 1, s_dv.dot(self.plane_normal))?;
        }

        // Compute function values
        let f0 = s_minus_p.dot(s_minus_p) - self.step_off_dist * self.step_off_dist;
        let f1 = s_point.dot(self.plane_normal) + plane_d;
        for (slot, value) in f.iter_mut().zip([f0, f1]) {
            *slot = value;
        }
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::solution_array::SolutionObject;
    use crate::nurbs::crv_basic::Curve as NlCurve;
    use crate::nurbs::math::{CPoint, NOW, NOZ};
    use crate::surface::plane::Plane;
    use crate::topology::{
        Edge, Edgeuse, Face, Faceuse, Loop, Loopuse, LoopuseStart, Region, Shell, Vertex, Vertexuse,
    };

    /// A lamina square face: two face uses, one loop with two loop uses, four edges
    /// with a `SAME` use in loop use 0 (`a_i`, id `2 i`) and an `OPPOSITE` one in loop
    /// use 1 (`b_i`, id `2 i + 1`).
    fn square() -> Brep {
        let mut b = Brep::new();
        let id = |i: u32| EdgeuseId::from_index(i);
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
            ..Face::new(ShellId::from_index(0))
        })
        .unwrap();
        b.add_loop_record(Loop {
            face: FaceId::from_index(0),
            edges: Vec::new(),
            loopuse: Some(LoopuseId::from_index(0)),
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
                loopuses: vec![LoopuseId::from_index(k)],
                sibling_links: None,
            })
            .unwrap();
            b.add_loopuse_record(Loopuse {
                faceuse: Some(FaceuseId::from_index(k)),
                loop_: Some(LoopId::from_index(0)),
                mate: Some(LoopuseId::from_index(1 - k)),
                start: LoopuseStart::Edgeuse(Some(id(if k == 0 { 0 } else { 7 }))),
                ..Loopuse::default()
            })
            .unwrap();
        }
        let corners = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        for (i, c) in corners.iter().enumerate() {
            b.add_vertex_record(Vertex {
                point: Vec3::new(c[0], c[1], 0.0),
                tolerance: ZONE_TOL_3D,
                ..Vertex::default()
            })
            .unwrap();
            let i = i as u32;
            let (next, prev) = ((i + 1) % 4, (i + 3) % 4);
            b.add_edge_record(Edge {
                vertices: [VertexId::from_index(i), VertexId::from_index(next)],
                edgeuses: vec![id(2 * i), id(2 * i + 1)],
                interval: Extent1d::from_min_max(0.0, 1.0),
                tolerance: 1e-4,
                curve: None,
            })
            .unwrap();
            // a_i runs from vertex i, b_i from vertex i + 1
            b.add_edgeuse_record(Edgeuse {
                edge: Some(EdgeId::from_index(i)),
                owner: EdgeuseOwner::Loopuse(Some(LoopuseId::from_index(0))),
                ccw: Some(id(2 * next)),
                cw: Some(id(2 * prev)),
                vertexuse: Some(VertexuseId::from_index(2 * i)),
                orientation: OrientType::Same,
                ..Edgeuse::default()
            })
            .unwrap();
            b.add_edgeuse_record(Edgeuse {
                edge: Some(EdgeId::from_index(i)),
                owner: EdgeuseOwner::Loopuse(Some(LoopuseId::from_index(1))),
                ccw: Some(id(2 * prev + 1)),
                cw: Some(id(2 * next + 1)),
                vertexuse: Some(VertexuseId::from_index(2 * i + 1)),
                orientation: OrientType::Opposite,
                ..Edgeuse::default()
            })
            .unwrap();
            for (v, e) in [(i, 2 * i), (next, 2 * i + 1)] {
                b.add_vertexuse_record(Vertexuse {
                    vertex: Some(VertexId::from_index(v)),
                    owner: VertexuseOwner::Edgeuse(Some(id(e))),
                    ..Vertexuse::default()
                })
                .unwrap();
            }
        }
        b
    }

    fn uv_line(a: [f64; 2], c: [f64; 2]) -> Curve {
        let pw = vec![
            CPoint::new(a[0], a[1], NOZ, NOW),
            CPoint::new(c[0], c[1], NOZ, NOW),
        ];
        let nl = NlCurve::new(pw, 1, vec![0.0, 0.0, 1.0, 1.0]).unwrap();
        Curve::from(crate::nurbs::b_spline_curve::BSplineCurve::from_nurb(
            nl, 2, false,
        ))
    }

    #[test]
    fn type_system() {
        assert!(is_kind_of(TYPE));
        assert!(is_kind_of(topo::TYPE));
        assert!(!is_kind_of(FACEUSE_TYPE));
        assert!(faceuse_is_kind_of(FACEUSE_TYPE));
        assert!(faceuse_is_kind_of(OWNING_TOPOLOGY_TYPE));
        assert!(!faceuse_is_kind_of(TYPE));
        assert_eq!((TYPE_STRING, CLASS_STRING), ("SmEdgeuse_TYPE", "SmEdgeuse"));
    }

    #[test]
    fn mates_radials_and_loop_neighbours() {
        let b = square();
        let a0 = EdgeuseId::from_index(0);
        let b0 = EdgeuseId::from_index(1);
        assert_eq!(mate(&b, a0).unwrap(), Some(b0));
        assert_eq!(mate(&b, b0).unwrap(), Some(a0));
        assert_eq!(radial(&b, a0).unwrap(), Some(b0));
        assert!(!is_strut(&b, a0).unwrap());
        assert_eq!(edgeuse_next(&b, a0).unwrap(), Some(b0));
        assert_eq!(edgeuse_last(&b, a0).unwrap(), Some(b0));
        assert_eq!(faceuse(&b, b0).unwrap(), Some(FaceuseId::from_index(1)));
        assert_eq!(face(&b, a0).unwrap(), Some(FaceId::from_index(0)));
        assert_eq!(shell(&b, a0).unwrap(), Some(ShellId::from_index(0)));
        assert!(brep(&b, a0).unwrap().is_some());
        assert!((tolerance(&b, a0).unwrap() - 1e-4).abs() < 1e-18);
        // a_0 starts at vertex 0: CW neighbour; its mate starts at vertex 1: CCW one
        let v = |i| Some(VertexId::from_index(i));
        assert_eq!(
            corner_mate_edgeuse(&b, a0, v(0)).unwrap(),
            Some(EdgeuseId::from_index(6))
        );
        assert_eq!(
            corner_mate_edgeuse(&b, a0, v(1)).unwrap(),
            Some(EdgeuseId::from_index(2))
        );
        assert_eq!(corner_mate_edgeuse(&b, a0, v(2)).unwrap(), None);
        assert!(edge_is_lamina(&b, EdgeId::from_index(0)).unwrap());
        let lu0: Vec<u32> = loopuse_edgeuses(&b, LoopuseId::from_index(0))
            .unwrap()
            .iter()
            .map(|x| x.index())
            .collect();
        assert_eq!(lu0, vec![0, 2, 4, 6]);
    }

    #[test]
    fn connectivity() {
        let b = square();
        let a1 = EdgeuseId::from_index(2);
        let yes = |t| is_connected_to(&b, a1, Some(t)).unwrap();
        assert!(is_connected_to(&b, a1, None).unwrap());
        assert!(yes(EntityRef::Edge(EdgeId::from_index(1))));
        assert!(!yes(EntityRef::Edge(EdgeId::from_index(0))));
        assert!(yes(EntityRef::Vertex(VertexId::from_index(1))));
        assert!(yes(EntityRef::Vertexuse(VertexuseId::from_index(2))));
        assert!(!yes(EntityRef::Vertexuse(VertexuseId::from_index(3))));
        assert!(yes(EntityRef::Loop(LoopId::from_index(0))));
        assert!(yes(EntityRef::Loopuse(LoopuseId::from_index(0))));
        assert!(!yes(EntityRef::Loopuse(LoopuseId::from_index(1))));
        assert!(yes(EntityRef::Faceuse(FaceuseId::from_index(0))));
        assert!(yes(EntityRef::Region(RegionId::from_index(0))));
        assert!(yes(EntityRef::Brep(crate::topology::BrepId::from_index(0))));
    }

    #[test]
    fn faceuse_collections() {
        let mut b = square();
        let f0 = FaceuseId::from_index(0);
        assert_eq!(
            faceuse_region(&b, f0).unwrap(),
            Some(RegionId::from_index(0))
        );
        assert_eq!(faceuse_loopuses(&b, f0, true, None).unwrap().len(), 1);
        let l = [LoopId::from_index(0)];
        assert_eq!(
            faceuse_loopuses(&b, FaceuseId::from_index(1), true, Some(&l)).unwrap(),
            vec![LoopuseId::from_index(1)]
        );
        assert_eq!(faceuse_edgeuses(&b, f0, false, None).unwrap().len(), 4);
        assert_eq!(
            faceuse_neighbors(&b, f0, false).unwrap(),
            vec![FaceuseId::from_index(1)]
        );
        assert!(faceuse_neighbors(&b, f0, true).unwrap().is_empty());
        let mut out = vec![FaceuseId::from_index(1)];
        faceuse_collect_adjacent(&Kernel::default(), &mut b, f0, &mut out, None).unwrap();
        assert_eq!(out, vec![FaceuseId::from_index(1)]);
    }

    #[test]
    fn uv_trim_curves_live_on_the_same_member() {
        let mut b = square();
        let a0 = EdgeuseId::from_index(0);
        let b0 = EdgeuseId::from_index(1);
        // through the OPPOSITE member: lands on its SAME mate
        let mut fail = DropCurveFail::<ObjectHandle> {
            fail_status: 7,
            ..DropCurveFail::default()
        };
        b.vertexuse_mut(VertexuseId::from_index(0))
            .unwrap()
            .vertex_face_gap
            .vertexuse = Some(VertexuseId::from_index(0));
        let old = set_uv_trim_curve(
            &mut b,
            b0,
            UvTrimCurveArg::New(Box::new(uv_line([0.0, 0.0], [1.0, 0.0]))),
            1e-5,
            false,
            Some(&mut fail),
        )
        .unwrap();
        assert!(old.is_none());
        assert!(uv_trim_curve_pointer(&b, a0).unwrap().is_some());
        assert!(uv_trim_curve_pointer(&b, b0).unwrap().is_none());
        let (c, owner) = uv_trim_curve(&b, b0).unwrap();
        assert!(c.is_some());
        assert_eq!(owner, Some(a0));
        assert_eq!(
            drop_curve_fail(&b, b0).unwrap().0.map(|f| f.fail_status),
            Some(7)
        );
        assert!(!b
            .vertexuse(VertexuseId::from_index(0))
            .unwrap()
            .vertex_face_gap
            .is_init());

        // the attached curve again: only the drop record is updated (and its B-rep set)
        fail.fail_status = 9;
        set_uv_trim_curve(
            &mut b,
            a0,
            UvTrimCurveArg::Current,
            0.0,
            false,
            Some(&mut fail),
        )
        .unwrap();
        assert!(fail.brep.is_some());
        assert_eq!(
            drop_curve_fail(&b, a0).unwrap().0.map(|f| f.fail_status),
            Some(9)
        );

        // a replaced curve is handed back unless deleted
        let old = set_uv_trim_curve(
            &mut b,
            a0,
            UvTrimCurveArg::New(Box::new(uv_line([0.0, 1.0], [1.0, 1.0]))),
            0.0,
            false,
            None,
        )
        .unwrap();
        assert!(old.is_some());
        assert!(drop_curve_fail(&b, a0).unwrap().0.is_none());

        // growing the edge tolerance grows the vertex tolerances: SmVertex::SetTolerance
        set_uv_trim_curve(
            &mut b,
            a0,
            UvTrimCurveArg::New(Box::new(uv_line([0.0, 1.0], [1.0, 1.0]))),
            1.0,
            true,
            None,
        )
        .unwrap();
        let e = edge(&b, a0).unwrap().unwrap();
        assert_eq!(b.edge(e).unwrap().tolerance, 2.0);
        b.set_validate_only(true);
        set_uv_trim_curve(
            &mut b,
            a0,
            UvTrimCurveArg::New(Box::new(uv_line([0.0, 1.0], [1.0, 1.0]))),
            1.0,
            true,
            None,
        )
        .unwrap();

        delete_uv_trim_curve(&mut b, b0).unwrap();
        assert!(uv_trim_curve(&b, a0).unwrap().0.is_none());

        let (old, _) =
            set_member_uv_trim_curve(&mut b, b0, uv_line([0.0, 0.0], [1.0, 0.0]), None).unwrap();
        assert!(old.is_none());
        assert!(uv_trim_curve_pointer(&b, a0).unwrap().is_some());

        // the SAME member owns the curve it is given, whichever use of the pair placed
        // it (SmEdgeuse::SetMemberUVTrimCurve's pUVTrimCurve->SetOwner(pOwner))
        b.set_naming(NamingScheme::Notify);
        let mut stale = uv_line([0.0, 1.0], [1.0, 1.0]);
        let _ = stale.set_owner(owner_ref_in(&b, b0), None);
        set_member_uv_trim_curve(&mut b, b0, stale, None).unwrap();
        let owner = uv_trim_curve_pointer(&b, a0)
            .unwrap()
            .and_then(Curve::owner);
        assert_eq!(owner, owner_ref_in(&b, a0));
        assert!(owner.is_some());
    }

    #[test]
    fn issue_521_loop_gap_refuses_failed_trim_and_edge_evaluations() {
        use crate::nurbs::b_spline_curve::BSplineCurve;
        let mut k = Kernel::default();
        k.fixes.topology_failed_evaluations_propagate = true;
        let a0 = EdgeuseId::from_index(0);
        for bad in [0, 6] {
            let mut b = square();
            b.edgeuse_mut(a0).unwrap().uv_trim_curve = Some(uv_line([0.0, 0.0], [1.0, 0.0]));
            b.edgeuse_mut(EdgeuseId::from_index(6))
                .unwrap()
                .uv_trim_curve = Some(uv_line([0.0, 1.0], [0.0, 0.0]));
            let empty = Curve::from(BSplineCurve::new(2));
            let expected = empty.evaluate_point(&k, 0.0).unwrap_err();
            b.edgeuse_mut(EdgeuseId::from_index(bad))
                .unwrap()
                .uv_trim_curve = Some(empty);
            assert_eq!(start_point_uv_loop_gap(&k, &b, a0), Err(expected));
        }
        for bad in [0, 3] {
            let mut b = square();
            for (edge, a, z) in [(0, Vec3::ZERO, Vec3::X), (3, Vec3::Y, Vec3::ZERO)] {
                b.edge_mut(EdgeId::from_index(edge)).unwrap().curve =
                    Some(Curve::from(crate::curve::line::Line::from_points(a, z, 3)));
            }
            assert_eq!(start_point_uv_loop_gap(&k, &b, a0).unwrap(), 0.0);
            let empty = Curve::from(BSplineCurve::new(3));
            let expected = empty.evaluate_point(&k, 0.0).unwrap_err();
            b.edge_mut(EdgeId::from_index(bad)).unwrap().curve = Some(empty);
            assert_eq!(start_point_uv_loop_gap(&k, &b, a0), Err(expected));
        }
    }

    #[test]
    fn start_point_uv_loop_gap_uses_trim_curves() {
        let mut b = square();
        for (i, (s, e)) in [
            ([0.0, 0.0], [1.0, 0.0]),
            ([1.0, 0.0], [1.0, 1.0]),
            ([1.0, 1.0], [0.0, 1.0]),
            ([0.0, 1.0], [0.0, 0.25]),
        ]
        .into_iter()
        .enumerate()
        {
            b.edgeuse_mut(EdgeuseId::from_index(2 * i as u32))
                .unwrap()
                .uv_trim_curve = Some(uv_line(s, e));
        }
        let a0 = EdgeuseId::from_index(0);
        assert!(
            (start_point_uv_loop_gap(&Kernel::default(), &b, a0).unwrap() - 0.25).abs() < 1e-15
        );
        assert_eq!(
            start_point_uv_loop_gap(&Kernel::default(), &b, EdgeuseId::from_index(2)).unwrap(),
            0.0
        );
        // without trim curves the end points are dropped onto the surface
        b.edgeuse_mut(a0).unwrap().uv_trim_curve = None;
        b.edge_mut(EdgeId::from_index(0)).unwrap().curve = None;
        assert!(start_point_uv_loop_gap(&Kernel::default(), &b, a0).is_err());
    }

    #[test]
    fn gaps_of_loop_edge_uses() {
        let k = Kernel::default();
        let mut b = square();
        let a0 = EdgeuseId::from_index(0);
        b.edgeuse_mut(a0)
            .unwrap()
            .max_edge_face_gap
            .set_edgeuse(Some(a0));
        // the square's edges have no curves
        assert!(matches!(
            max_edge_face_gap(&k, &mut b, a0, true),
            Err(SolidError::InvalidInput { .. })
        ));
        // the caches were reset before
        assert!(!is_max_edge_face_gap_init(&b, a0).unwrap());
        assert!(
            calc_edge_ccw_edge_gap(&Kernel::default(), &mut b, a0, false, false)
                .unwrap()
                .is_none()
        );
        assert!(matches!(
            calc_edge_ccw_edge_gap(&Kernel::default(), &mut b, a0, true, false),
            Err(SolidError::InvalidInput { .. })
        ));
        // With curves, both edge/edge and edge/face gaps are evaluated.
        for i in 0..4u32 {
            let e = EdgeId::from_index(i);
            let [v0, v1] = b.edge(e).unwrap().vertices;
            let (p0, p1) = (b.vertex(v0).unwrap().point, b.vertex(v1).unwrap().point);
            let mut line = Curve::from(crate::curve::line::Line::from_points(p0, p1, 3));
            line.set_owner(None, None);
            b.edge_mut(e).unwrap().curve = Some(line);
        }
        let g = calc_edge_ccw_edge_gap(&Kernel::default(), &mut b, a0, true, true)
            .unwrap()
            .unwrap();
        assert!(g.is_init() && g.gap_3d.abs() < 1e-12);
        assert_eq!(b.edgeuse(a0).unwrap().edge_ccw_edge_gap, g);
        let face_gap = max_edge_face_gap(&k, &mut b, a0, true).unwrap().unwrap();
        assert_eq!(face_gap.gap_3d, 0.0);
        assert_eq!(face_gap.edgeuse, Some(a0));
        assert!(is_max_edge_face_gap_init(&b, a0).unwrap());
        // a degenerate edge interval fails before the gap function
        b.edge_mut(EdgeId::from_index(0)).unwrap().interval = Extent1d::from_value(0.5);
        assert!(matches!(
            calc_max_edge_face_gap(&k, &mut b, a0, true),
            Err(SolidError::Upstream { .. })
        ));
    }

    #[test]
    fn missing_links_are_errors() {
        let mut b = square();
        let a0 = EdgeuseId::from_index(0);
        b.edge_mut(EdgeId::from_index(0)).unwrap().edgeuses.clear();
        assert!(matches!(mate(&b, a0), Err(SolidError::InvalidInput { .. })));
        assert!(matches!(
            edgeuse_next(&b, EdgeuseId::from_index(99)),
            Err(SolidError::NotFound { .. })
        ));
        b.edgeuse_mut(a0).unwrap().edge = None;
        assert!(matches!(brep(&b, a0), Err(SolidError::InvalidInput { .. })));
        assert_eq!(tolerance(&b, a0).unwrap(), ZONE_TOL_3D);
    }

    #[derive(Debug, Clone, Copy, PartialEq)]
    struct H(ObjectKind);
    impl SolutionObject for H {
        fn kind(&self) -> ObjectKind {
            self.0
        }
    }

    #[test]
    fn issue_521_dist_and_dev_propagates_surface_evaluation_failure() {
        let mut k = Kernel::default();
        k.fixes.topology_failed_evaluations_propagate = true;
        let s = Surface::from(crate::nurbs::b_spline_surface::BSplineSurface::default());
        let mut sol = Solution::<H>::new();
        sol.objects[0] = Some(H(ObjectKind::Surface));
        sol.start.solution_value = 0.5;
        let expected =
            evaluate_1st_derivatives(&k, &s, Point2::new(0.0, 0.0), Side::Left, Side::Left)
                .unwrap_err();
        assert_eq!(
            calc_dist_and_dev(&k, Vec3::Z, &sol, |_| Some(&s), true),
            Err(expected)
        );
        assert_eq!(
            calc_dist_and_dev(&k, Vec3::Z, &sol, |_| Some(&s), false).unwrap(),
            (0.5, None)
        );
    }

    #[test]
    fn failed_evaluations_are_ignored_by_default() {
        // WR-263 / WR-264: upstream ignores the failed evaluations' statuses.
        use crate::nurbs::b_spline_curve::BSplineCurve;
        let k = Kernel::default();
        let a0 = EdgeuseId::from_index(0);
        let mut b = square();
        b.edgeuse_mut(a0).unwrap().uv_trim_curve = Some(Curve::from(BSplineCurve::new(2)));
        b.edgeuse_mut(EdgeuseId::from_index(6))
            .unwrap()
            .uv_trim_curve = Some(uv_line([0.0, 1.0], [0.0, 0.0]));
        assert!(start_point_uv_loop_gap(&k, &b, a0).is_ok());
        let s = Surface::from(crate::nurbs::b_spline_surface::BSplineSurface::default());
        let mut sol = Solution::<H>::new();
        sol.objects[0] = Some(H(ObjectKind::Surface));
        sol.start.solution_value = 0.5;
        assert!(calc_dist_and_dev(&k, Vec3::Z, &sol, |_| Some(&s), true).is_ok());
    }

    #[test]
    fn dist_and_dev_on_a_plane() {
        let k = &Kernel::default();
        let s = Surface::from(Plane::from_normal(Vec3::ZERO, Vec3::Z));
        let mut sol = Solution::<H>::new();
        sol.start.solution_value = 0.5;
        let pt = Vec3::new(3.0, 4.0, 2.0);
        assert_eq!(
            calc_dist_and_dev(k, pt, &sol, |_| Some(&s), false).unwrap(),
            (0.5, None)
        );
        assert_eq!(
            calc_dist_and_dev(k, pt, &sol, |_| Some(&s), true).unwrap(),
            (-1.0, Some(-1.0))
        );
        sol.objects[0] = Some(H(ObjectKind::Surface));
        let (d, dev) = calc_dist_and_dev(k, pt, &sol, |_| Some(&s), true).unwrap();
        assert!((d - 2.0).abs() < 1e-12);
        assert!(dev.is_some_and(|x| x > 0.0));
    }

    #[test]
    fn seam_curves() {
        let a = uv_line([0.0, 0.0], [0.0, 1.0]);
        let c = uv_line([1.0, 0.0], [1.0, 1.0]);
        let d = Extent1d::from_min_max(0.0, 1.0);
        assert!(!are_seam_curves(&[&a], &d, true));
        assert!(are_seam_curves(&[&a, &c], &d, true));
        assert!(are_seam_curves(
            &[&a, &c],
            &Extent1d::from_min_max(0.005, 1.005),
            false
        ));
        assert!(!are_seam_curves(
            &[&a, &c],
            &Extent1d::from_min_max(0.0, 0.5),
            false
        ));
    }

    #[test]
    fn step_off_equations() {
        let k = &Kernel::default();
        let s = Surface::from(Plane::from_normal(Vec3::ZERO, Vec3::Z));
        let mut e = FindStepOff::new(k, 1.0, Vec3::ZERO, Vec3::Y, &s);
        let mut f = [0.0; 2];
        let mut m = Matrix::new(2, 2);
        let found = e.evaluate(&[0.0, 0.0], &mut f, Some(&mut m)).unwrap();
        assert!(!found);
        let p = s.evaluate_point(k, Point2::new(0.0, 0.0)).unwrap();
        assert!((f[0] - (p.dot(p) - 1.0)).abs() < 1e-12);
        assert!((f[1] - p.y).abs() < 1e-12);
        assert!(e.evaluate(&[0.0], &mut f, None).is_err());
        let mut small = Matrix::new(1, 1);
        assert!(e.evaluate(&[0.0, 0.0], &mut f, Some(&mut small)).is_err());
    }
}
