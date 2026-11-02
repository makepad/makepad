//! `SmBrep` queries, tolerance and topology construction, notifications and the
//! geometry edits of plan unit `topology.brep_p2`.
//!
//! Upstream: `source/SMLib/src/SmBrep.cpp`, `source/SMLib/inc/SmBrep.h`.
//!
//! | Rust                         | Upstream                                  |
//! |------------------------------|-------------------------------------------|
//! | [`edgeuses`]                 | `SmBrep::GetEdgeuses`                     |
//! | [`shell_vertices`]           | `SmBrep::GetShellVertices`                |
//! | [`is_manifold_solid`]        | `SmBrep::IsManifoldSolid`                 |
//! | [`is_sheet`]                 | `SmBrep::IsSheet`                         |
//! | [`is_oriented_sheet`]        | `SmBrep::IsOrientedSheet`                 |
//! | [`min_edge_bbox_size`]       | `SmBrep::GetMinEdgeBBoxSize`              |
//! | [`set_tolerance`]            | `SmBrep::SetTolerance` (`SM_USE_OLDTOL`)  |
//! | [`rebuild_uv_trim_curves`]   | `SmBrep::RebuildUVTrimCurves`             |
//! | [`check_and_fix_uv_trim_curves`] | `sm_CheckAndFixUVTrimCurves` (file static) |
//! | [`swap_uv`]                  | `SmBrep::SwapUV`                          |
//! | [`notify`], [`NotifyArg`], [`brep_ref`] | `SmBrep::Notify`               |
//! | [`RegionNotifyState`], [`region_only_key`] | (port helper) the notify state of a caller without attribute state |
//! | [`ArenaNotifyState`] | (port helper) the notify state of an arena-only caller (every object's record) |
//! | `add_to_spatial_tree` (private) | `sm_AddToSpatialTree`                  |
//! | `rm_from_spatial_tree` (private) | `sm_RmFromSpatialTree`                |
//! | [`delete_stale_caches`]      | `SmBrep::DeleteStaleCaches`               |
//! | [`point_from_solution`]      | `sm_GetPointFromSol` (file static)        |
//! | [`local_curve_sector`]       | `SmBrep::LocalCurveSector`                |
//! | [`ray_fire`], [`RayFireResult`] | `SmBrep::RayFire`                      |
//! | [`replace_surface`], [`OldSurface`] | `SmBrep::ReplaceSurface`           |
//! | [`make_shell_vertex_topology`] | `SmBrep::MakeShellVertexTopology`       |
//! | [`make_edge_vertex_topology`] | `SmBrep::MakeEdgeVertexTopology`         |
//! | [`make_edge_topology`]       | `SmBrep::MakeEdgeTopology`                |
//! | [`merge_curves_on_surface`], [`MergeCurvesObjects`], [`MergeCurvesOptions`], [`MergedCurves`] | `SmBrep::MergeCurvesOnSurface` |
//! | [`mate_lamina_edge_to_surface`] | `SmBrep::MateLaminaEdgeToSurface`      |
//!
//! Deviations: out-params become return values. `GetMinEdgeBBoxSize` returns the
//! smallest edge as `Option` (upstream leaves `rpEdge` unassigned when no edge
//! qualifies). `SmZoneTol3d` arguments are taken as the converted value (`fabs`, see
//! [`crate::geom::tol::zone_tol3d`]). Upstream dereferences of null (a manifold edge
//! without radial edge use or face use) return [`SolidError::InvalidInput`]. A loop
//! that keeps going after a failed status upstream (`RebuildUVTrimCurves`) stops at
//! the first [`SolidError::Unported`]. The B-rep cache (`SmCacheMgr` slot
//! `SM_OC_BREP`) is caller-held: [`notify`] edits the one in its `cache` slot (none:
//! no cache). No recursion. Input-driven loops are bounded by their inputs:
//! `MergeCurvesOnSurface`'s face queue only grows by the faces `MergeIntervals` creates
//! (each split adds faces to the finite B-rep), `MateLaminaEdgeToSurface` samples 21
//! points and scans the intersection curves once.
//!
//! Smaller deviations, none reachable as a difference today:
//! - `ReplaceSurface` edits `pTopFace = pOld->GetFace()` (`SmBrep.cpp:26316`), the owner of
//!   the face's surface, which can differ from `pFace` when faces share a surface upstream;
//!   in the arena model every face owns its own surface, so the port edits `f`.
//! - `ReplaceSurface`'s `SmFace::SetSurface` notifies `SM_NO_CHANGE_GEOMETRY` with the
//!   new and the old surface's identities issued by the caller's context
//!   ([`crate::topology::face::p1::set_surface_notified`]); a caller without a context
//!   (whose notifications reach no callback) passes null surfaces.
//! - `MakeShellVertexTopology` / `MakeEdgeVertexTopology` / `MakeEdgeTopology` add their
//!   records to the arenas as they link them, not all up front; `MakeEdgeTopology`'s
//!   `TransferChildrenTo` (the one shell call that can fail) runs before any new record
//!   exists, so upstream's `SmObjDelete` guards have nothing to undo. The merged end
//!   shell is deleted ([`shell::destroy`]); a deleted vertex shell's vertex use stays in
//!   the arena, unlinked.
//! - `MateLaminaEdgeToSurface` checks for the face's surface before the neighbour
//!   (lamina / seam) checks; upstream binds the surface only for `IsSeam`, so a face
//!   without surface there ends in the distance loop instead of
//!   [`SolidError::null_pointer`]. Its intersection UV curve must be a plain
//!   B-spline (`SM_CAST_PTR(SmBSplineCurve, ..)` also passes `SmLine` and the conics,
//!   which the port treats as the null cast); the extended mating surface is always a
//!   B-spline, so the intersector returns NURBS curves. A negative extension distance
//!   (`dSurfaceExtensionFactor < 0`) is [`SolidError::InvalidInput`] before
//!   `CreateExtendedSurface`, where upstream never returns
//!   (`plan/upstream-issues.md` HANG-18).

use std::collections::BTreeMap;

use crate::curve::curve::p2::is_curve_class;
use crate::curve::curve::{p8::CurveIntersectOptions, Curve, CurveKind};
use crate::error::{ignore_status, EntityRef, Result, SolidError, SM_ERR};
use crate::geom::a_object::{self, BREP_TYPE};
use crate::geom::attribute::{AttributeStore, ObjectKey};
use crate::geom::context::{MarkLock, ALL_MARKS};
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::extent3d::Extent3d;
use crate::geom::geom_utility::p2::ContinuityType;
use crate::geom::global_solver::{SolutionRequested, SolverOperation};
use crate::geom::object::{NotifyOperation, ObjectRef};
use crate::geom::solution_array::Solution;
use crate::geom::tol::{zone_tol3d, ZONE_TOL_3D};
use crate::geom::tree::NodeGeomType;
use crate::intersect::curve_class::p1::{CurveClassification, IntervalPosition};
use crate::intersect::curve_class::p6::MergeIntervalBrep;
use crate::intersect::curve_class::p7::CoincidentMaybe;
use crate::intersect::curve_class::ClassificationInit;
use crate::intersect::point_class::{
    ClassGeometry, ClassObject, PointClassType, PointClassification,
};
use crate::intersect::topology_solver::p2::TopologySolveOptions;
use crate::kernel::Kernel;
use crate::math::{Point2, Point3, Vec3, BIG_DOUBLE, EFF_ZERO, EFF_ZERO_SQRT, PI};
use crate::nurbs::b_spline_curve::{BSplineCurve, BoundingBoxRequest};
use crate::nurbs::b_spline_curve_n_lib::TrimMode;
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::query::brep::{self as qbrep, BoundingBoxOptions};
use crate::query::edge as qedge;
use crate::query::face::p1 as qface1;
use crate::query::face::p2::{curve_on_classify_mut, CurveOnClassifyBrep};
use crate::surface::surface::{Surface, SurfaceKind};
use crate::surface::trim_srf_cache::TrimSrfCache;
use crate::topology::brep::p1;
use crate::topology::brep_cache::{self, BrepCache, CacheObject, CacheSource};
use crate::topology::edge::{self, AttributeFilter, NotifyEnv, SetCurveOptions};
use crate::topology::edgeuse::p1::{self as eu1, UvTrimCurveArg};
use crate::topology::edgeuse::p2 as eu2;
use crate::topology::edgeuse::p3 as eu3;
use crate::topology::face::p1 as face1;
use crate::topology::face::p2 as face2;
use crate::topology::face::p3 as face3;
use crate::topology::shell::{self, ShellMembers, ShellType};
use crate::topology::topology::Topology;
use crate::topology::topology_traverser::Marks;
use crate::topology::vertexuse as vu1;
use crate::topology::{
    vertex, Brep, BrepId, Edge, EdgeId, Edgeuse, EdgeuseId, EdgeuseOwner, FaceId, ObjectHandle,
    RegionId, Shell, ShellId, Vertex, VertexId, Vertexuse, VertexuseId, VertexuseOwner,
};

const OP: &str = "topology::brep::p2";

const fn null_deref(reason: &'static str) -> SolidError {
    SolidError::invalid_input(OP, reason)
}

// ---- list queries -------------------------------------------------------------------------

/// `GetEdgeuses`: the edge uses of every B-rep edge (in edge list order, each edge's
/// radial list from its head), filtered by `attribute` (`pOptAttributeId`).
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn edgeuses(b: &Brep, attribute: Option<&AttributeFilter<'_>>) -> Result<Vec<EdgeuseId>> {
    // init output
    let mut out = Vec::new();

    // for every Brep->Edge
    for e in brep_edges(b)? {
        // accumulate all edge->Edgeuses
        out.extend(edge::edgeuses(b, e, attribute, None)?);
    }
    Ok(out)
}

/// `GetShellVertices`: the B-rep vertices (filtered by `attribute`) that are shell
/// vertices, in vertex list order.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn shell_vertices(b: &Brep, attribute: Option<&AttributeFilter<'_>>) -> Result<Vec<VertexId>> {
    let mut out = Vec::new();

    // for every vertex
    for v in brep_vertices(b, attribute)? {
        // collect the Shell vertices
        if vertex::is_shell_vertex(b, v)? {
            out.push(v);
        }
    }
    Ok(out)
}

// ---- model type predicates ----------------------------------------------------------------

/// `IsManifoldSolid`: at least two regions, no shell vertices and only manifold edges.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn is_manifold_solid(b: &Brep) -> Result<bool> {
    if brep_regions(b)?.len() < 2 {
        return Ok(false);
    }

    if !shell_vertices(b, None)?.is_empty() {
        return Ok(false);
    }

    for e in brep_edges(b)? {
        if !edge::is_manifold(b, e)? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// `IsSheet`: one shell, at least one edge, and every edge lamina or manifold.
///
/// # Errors
///
/// Those of `SmBrep::GetShells` ([`p1::shells`], the mark lock);
/// [`SolidError::NotFound`] for a dangling id.
pub fn is_sheet(k: &mut Kernel, b: &Brep) -> Result<bool> {
    // sheets have just one shell
    if brep_shells(k, b)?.len() != 1 {
        return Ok(false);
    }

    // sheets have edges that are all manifold or lamina
    let edges = brep_edges(b)?;
    if edges.is_empty() {
        return Ok(false);
    }
    for e in edges {
        // all edges are lamina or manifold
        if eu1::edge_is_lamina(b, e)? || edge::is_manifold(b, e)? {
            continue;
        }

        // arrive here when edge is not lamina and not manifold
        return Ok(false);
    }

    // arrive here once all edges have been checked
    Ok(true)
}

/// `IsOrientedSheet`: [`is_sheet`] where, across every manifold edge, the face uses of
/// the primary edge use and its radial share an orientation.
///
/// # Errors
///
/// Those of `SmBrep::GetShells` ([`p1::shells`], the mark lock);
/// [`SolidError::InvalidInput`] where upstream dereferences null (a manifold edge's
/// primary edge use, its radial, or either face use missing);
/// [`SolidError::NotFound`] for a dangling id.
pub fn is_oriented_sheet(k: &mut Kernel, b: &Brep) -> Result<bool> {
    // sheets have just one shell
    if brep_shells(k, b)?.len() != 1 {
        return Ok(false);
    }

    // sheets have edges that are all manifold or lamina
    let edges = brep_edges(b)?;
    if edges.is_empty() {
        return Ok(false);
    }

    // for every edge
    for e in edges {
        // Every edge must be a lamina or manifold edge
        if eu1::edge_is_lamina(b, e)? {
            continue;
        }
        if edge::is_manifold(b, e)? {
            // check the faces to make sure they share a common up/down

            // get radial pair of edgeuses
            let eu1_ = eu1::primary_edgeuse(b, e)?.ok_or(null_deref("edge without edge use"))?;
            let eu2_ = eu1::radial(b, eu1_)?.ok_or(null_deref("edge use without radial"))?;

            // get radial pair of faceuses
            let fu1 = eu1::faceuse(b, eu1_)?.ok_or(null_deref("edge use without face use"))?;
            let fu2 = eu1::faceuse(b, eu2_)?.ok_or(null_deref("edge use without face use"))?;

            // This pair of faceuses will share a common up/down
            //  when both faceuses are either
            //    1. the primary faceuse for their faces, or
            //    2. or are not the primary faceuse for their faces.
            if b.faceuse(fu1)?.orientation == b.faceuse(fu2)?.orientation {
                continue;
            }
        }

        // arrive here when edge is either not lamina and not manifold
        // or is not an oriented manifold edge
        return Ok(false);
    }

    // arrive here once all edges have been checked
    Ok(true)
}

// ---- sizes and tolerances -----------------------------------------------------------------

/// `GetMinEdgeBBoxSize`: the smallest box diagonal over the edges whose
/// [`qedge::calculate_bounding_box`] diagonal squared exceeds `SM_EFF_ZERO`, and that
/// edge. Without edges `(0.001, None)`; when no edge qualifies `(sqrt(1234567), None)`
/// (upstream leaves `rpEdge` unassigned in both cases).
///
/// # Errors
///
/// [`SolidError::Unported`] from the box computation's unported callees (other statuses are ignored, as upstream).
pub fn min_edge_bbox_size(k: &Kernel, b: &Brep) -> Result<(f64, Option<EdgeId>)> {
    let edges = brep_edges(b)?;
    if edges.is_empty() {
        // the no-edge brep returns standard default 0.001
        return Ok((0.001, None));
    }
    let mut size = 1_234_567.0;
    let mut found = None;
    let req = BoundingBoxRequest {
        normal: true,
        pseudo: false,
        polar: false,
    };
    // one box for all edges: a failed call (status ignored) leaves the previous box
    let mut bbox = Extent3d::new();
    for e in edges {
        // this is a slow process depending on tessellation
        match qedge::calculate_bounding_box(k, b, e, req, false) {
            Ok(boxes) => {
                if let Some(n) = boxes.normal {
                    bbox = n;
                }
            }
            Err(err) if err.is_unported() => return Err(err),
            Err(_) => {}
        }
        let diff = bbox.size();
        let size2 = diff.length_squared();
        if size2 < size && size2 > EFF_ZERO {
            size = size2;
            found = Some(e);
        }
    }
    Ok((size.sqrt(), found))
}

/// `SetTolerance` (`SM_USE_OLDTOL`): sets the B-rep tolerance to `new_zone_tol_3d`
/// (a value `<= 0` becomes `SM_ZONE_TOL_3D` times the B-rep box diagonal, `1` when the
/// diagonal is `>= 1e15` or `<= 0`) and raises every smaller face, edge and vertex
/// tolerance to it (passing `update_only_if_larger`, no cascading). Nothing changes when
/// the value equals the current tolerance. `cascade_to_boundaries` (upstream default
/// `TRUE`) is unused upstream.
///
/// Upstream keeps the box of a failed `CalculateBoundingBox` initialised (its status
/// is ignored), so the diagonal of the inside-out box is used then.
///
/// # Errors
///
/// [`SolidError::Unported`] from the B-rep box; [`SolidError::NotFound`] for a dangling
/// id.
pub fn set_tolerance(
    k: &Kernel,
    b: &mut Brep,
    new_zone_tol_3d: f64,
    update_only_if_larger: bool,
    cascade_to_boundaries: bool,
) -> Result<()> {
    // SM_REF1(bCascadeToBndries): not used here
    let _ = cascade_to_boundaries;
    let mut new_tol = new_zone_tol_3d;

    // when sZoneTol3d <= 0.0 compute tol from Bounding box size
    if new_tol <= 0.0 {
        // (status ignored: a failure leaves the initialised box)
        let bbox = crate::error::ignored(qbrep::calculate_bounding_box(
            k,
            &mut *b,
            BoundingBoxOptions::default(),
        ))?
        .unwrap_or_default();
        let mut size = bbox.size().length();
        if size >= 1.0e15 || size <= 0.0 {
            size = 1.0;
        }
        // the SmZoneTol3d assignment takes fabs of a positive value
        new_tol = ZONE_TOL_3D * size;
    }

    // no work - current tol same as input tolerance
    #[allow(clippy::float_cmp)]
    if new_tol == b.tolerance() {
        return Ok(());
    }

    // set Brep Tolerance
    b.set_tolerance(new_tol);

    // for every face
    for f in brep_faces(b)? {
        // increase small tolerances to target tol value
        if b.face(f)?.tolerance < new_tol {
            face_set_tolerance(b, f, new_tol, update_only_if_larger)?;
        }
    }

    // for every edge
    for e in brep_edges(b)? {
        // increase small tolerance to target tol value
        if b.edge(e)?.tolerance < new_tol {
            edge::set_tolerance(b, e, new_tol, update_only_if_larger, false)?;
        }
    }

    // for every vertex
    for v in brep_vertices(b, None)? {
        // increase small tolerances to target tol value
        if b.vertex(v)?.tolerance < new_tol {
            vertex::set_tolerance(b, v, new_tol, update_only_if_larger, false)?;
        }
    }
    Ok(())
}

// ---- UV trim curves -----------------------------------------------------------------------

/// `RebuildUVTrimCurves`: rebuilds the UV trim curve of every edge use that has one
/// ([`eu2::rebuild_uv_trim_curve`]); every one is attempted and the last failure is
/// returned.
///
/// # Errors
///
/// The last failing rebuild's error; [`SolidError::Unported`] (immediately) from an
/// unported callee of a rebuild.
pub fn rebuild_uv_trim_curves(k: &Kernel, b: &mut Brep) -> Result<()> {
    // init return
    let mut ret = Ok(());

    // for every Edgeuse
    for eu in edgeuses(b, None)? {
        // skip those without UVTrimCurves (`GetUVTrimCurve()` reads the mate's slot when
        // the edge use holds none, so both members of a mated pair are rebuilt)
        if eu1::uv_trim_curve(b, eu)?.0.is_none() {
            continue;
        }

        // rebuild UVTrimCurve
        match eu2::rebuild_uv_trim_curve(k, b, eu) {
            Ok(()) => {}
            Err(e) if e.is_unported() => return Err(e),
            Err(e) => ret = Err(e),
        }
    }

    // all done
    ret
}

/// `sm_CheckAndFixUVTrimCurves` (file static, called by `SmBrep::ReadFromFile`):
/// checks every edge use's UV trim curve with `sm_CheckUVTrimCurve` and, as `siRemove`
/// is defined, deletes it when the check fails. Always succeeds upstream.
///
/// # Errors
///
/// Errors of `sm_CheckUVTrimCurve` other than its `SM_ERR` (null dereferences) and of
/// [`eu1::set_uv_trim_curve`].
pub fn check_and_fix_uv_trim_curves(k: &Kernel, b: &mut Brep) -> Result<()> {
    for eu in edgeuses(b, None)? {
        if !check_uv_trim_curve(k, b, eu)? {
            // siRemove: TRUE = delete existing UVTrimCurve
            let _ = eu1::set_uv_trim_curve(b, eu, UvTrimCurveArg::Null, 0.0, true, None)?;
        }
    }
    Ok(())
}

/// `SwapUV`: claims an unlocked mark and swaps the UV parameterisation of every face
/// (`sm_SwapFaceUV`, which skips faces already marked).
///
/// # Errors
///
/// [`SolidError::Unported`] from `sm_SwapFaceUV` (`SmFace::SwapUV`,
/// `topology.face_p1`).
pub fn swap_uv(k: &mut Kernel, b: &mut Brep, marks: &mut Marks) -> Result<()> {
    let faces = brep_faces(b)?;

    // increment and lock an unlocked mark
    let mut lock = MarkLock::new(std::slice::from_mut(&mut k.context), 0, ALL_MARKS)?;
    let mark = lock.mark_type();

    // for every face
    let mut res = Ok(());
    for f in faces {
        // swap the face's orientation - watch for composites (sm_SwapFaceUV is void:
        // only an unported callee stops the loop)
        if let Err(e) = p1::swap_face_uv(k, b, marks, f, mark) {
            if e.is_unported() {
                res = Err(e);
                break;
            }
        }
    }
    lock.release(std::slice::from_mut(&mut k.context))?;
    res
}

// ---- unported callees ---------------------------------------------------------------------

/// `SmBrep::GetEdges`.
fn brep_edges(b: &Brep) -> Result<Vec<EdgeId>> {
    p1::edges(b, None)
}

/// `SmBrep::GetFaces`.
fn brep_faces(b: &Brep) -> Result<Vec<FaceId>> {
    p1::faces(b, None)
}

/// `SmBrep::GetVertices(.., pOptAttributeId)`.
fn brep_vertices(b: &Brep, attribute: Option<&AttributeFilter<'_>>) -> Result<Vec<VertexId>> {
    p1::vertices(b, attribute)
}

/// `SmBrep::GetRegions`.
fn brep_regions(b: &Brep) -> Result<Vec<RegionId>> {
    p1::regions(b, None)
}

/// `SmBrep::GetShells`.
fn brep_shells(k: &mut Kernel, b: &Brep) -> Result<Vec<ShellId>> {
    p1::shells(k, b, None)
}

/// `sm_CheckUVTrimCurve`: `true` for `SM_SUCCESS`, `false` for its `SM_ERR`.
fn check_uv_trim_curve(k: &Kernel, b: &Brep, eu: EdgeuseId) -> Result<bool> {
    match p1::check_uv_trim_curve(k, b, eu) {
        Ok(()) => Ok(true),
        Err(SolidError::Upstream { code: SM_ERR, .. }) => Ok(false),
        Err(e) => Err(e),
    }
}

/// `SmFace::SetTolerance(tol, bUpdateOnlyIfLarger, FALSE)` ([`face3::set_tolerance`]).
fn face_set_tolerance(
    b: &mut Brep,
    f: FaceId,
    tol: f64,
    update_only_if_larger: bool,
) -> Result<()> {
    face3::set_tolerance(b, f, tol, update_only_if_larger, false)
}

// ---- notifications ------------------------------------------------------------------------

/// An object a B-rep notification names (`SmObject*`): a topology entity of this
/// B-rep, or any other object by identity (geometry, another B-rep, ...).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyArg {
    /// A face, edge, vertex or other topology record of this B-rep.
    Entity(EntityRef),
    /// Any other object.
    Object(ObjectRef),
}

impl NotifyArg {
    fn object_ref(self, env: &NotifyEnv<'_>) -> ObjectRef {
        match self {
            Self::Entity(e) => (env.key_of)(e),
            Self::Object(r) => r,
        }
    }
}

/// The spatial cache of a B-rep (`SmCacheMgr::GetObjectCache(SM_OC_BREP, pBrep)`)
/// that [`notify`] keeps up to date, with the kernel its boxes are computed in.
pub struct BrepCacheSlot<'a> {
    /// The kernel.
    pub kernel: &'a Kernel,
    /// The cache; `None` when the B-rep has none.
    pub cache: &'a mut Option<BrepCache>,
}

/// The B-rep's identity (`this` of `SmBrep::Notify`).
#[must_use]
pub const fn brep_ref(b: &Brep) -> ObjectRef {
    ObjectRef {
        type_code: BREP_TYPE,
        key: b.key(),
    }
}

/// `Notify`: keeps the B-rep's spatial cache (`SmCacheMgr::GetObjectCache(SM_OC_BREP,
/// this)`, `cache`) in step with the change, then passes the notification to
/// `SmSAGObject::Notify` with the B-rep as `this`:
///
/// | operation                  | cache effect                                      |
/// |----------------------------|---------------------------------------------------|
/// | `PreEdit`, `Destruction`   | the cache is deleted                              |
/// | `MergeInBrep`              | `data2` leaves its tree                           |
/// | `RmFromBrep`               | `data1` leaves its tree                           |
/// | `SplitInBrep`              | `data2`, `data3` (each unless it is `data1`) join their trees |
/// | `AddToBrep`                | `data1` joins its tree                            |
///
/// Only edges, vertices and faces are in a tree; other objects are skipped.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] where upstream dereferences null (a cache without the
/// tree, a face without surface); [`SolidError::Unported`] from unported callees
/// (rebuilding a cache, `query.brep`); errors of `SmSAGObject::Notify`.
pub fn notify(
    b: &Brep,
    env: &mut NotifyEnv<'_>,
    cache: Option<&mut BrepCacheSlot<'_>>,
    operation: NotifyOperation,
    data: [Option<NotifyArg>; 3],
) -> Result<()> {
    notify_with_body_identity(b, env, cache, operation, data, brep_ref(b))
}

/// Notify using the caller's authoritative identity for this actual body.
/// Cache edits and region propagation share [`notify`]'s implementation.
///
/// # Errors
/// The same cache and notification errors as [`notify`].
pub fn notify_with_body_identity(
    b: &Brep,
    env: &mut NotifyEnv<'_>,
    cache: Option<&mut BrepCacheSlot<'_>>,
    operation: NotifyOperation,
    data: [Option<NotifyArg>; 3],
    body: ObjectRef,
) -> Result<()> {
    use NotifyOperation as N;
    let [d1, d2, d3] = data;
    let mut slot = cache;
    match operation {
        N::PreEdit | N::Destruction => {
            if let Some(s) = slot.as_deref_mut() {
                *s.cache = None;
            }
        }
        // deleted pData2 should be removed from appropriate BrepCache obj SpatialTree
        N::MergeInBrep => rm_from_spatial_tree(b, env, &mut slot, d2)?,
        // deleted pData1 should be removed from appropriate BrepCache obj SpatialTree
        N::RmFromBrep => rm_from_spatial_tree(b, env, &mut slot, d1)?,
        // add new topology object pData2 to appropriate BrepCache obj SpatialTree
        N::SplitInBrep => {
            if d1 != d2 {
                add_to_spatial_tree(b, env, &mut slot, d2)?;
            }
            if d1 != d3 {
                add_to_spatial_tree(b, env, &mut slot, d3)?;
            }
        }
        N::AddToBrep => add_to_spatial_tree(b, env, &mut slot, d1)?,
        _ => {}
    }

    // Propagate notification up hierarchy
    let refs = [d1, d2, d3].map(|d| d.map(|a| a.object_ref(env)));
    let [r1, r2, r3] = refs;
    let regions = BrepRegions {
        b,
        key_of: env.key_of,
    };
    a_object::sag_notify_with_regions(
        &mut *env.records,
        &mut *env.attributes,
        env.context.as_deref_mut(),
        operation,
        body,
        r1,
        r2,
        r3,
        Some(&regions),
    )
}

/// The regions of the notifying B-rep, by the identities its notifications carry
/// (`SmRegion::GetBrep` and the region flags of the attribute handlers reached from
/// [`notify`]): a region attribute names a region by [`NotifyEnv::key_of`]. A key that
/// is no region of this B-rep stays unresolved ([`SolidError::Unported`], as without a
/// lookup).
struct BrepRegions<'a> {
    b: &'a Brep,
    key_of: &'a dyn Fn(EntityRef) -> ObjectRef,
}

impl shell::RegionLookup for BrepRegions<'_> {
    fn resolve_region(&self, object: ObjectKey) -> Result<shell::RegionRef<'_>> {
        for (region_id, _) in self.b.regions().iter() {
            if (self.key_of)(EntityRef::Region(region_id)).key == object {
                return Ok(shell::RegionRef {
                    brep: self.b,
                    brep_id: BrepId::from_index(0),
                    region_id,
                });
            }
        }
        // UNPORTED(topology.shell): SmRegion::GetBrep of a region outside the notifying B-rep.
        // Needs a cross-B-rep lookup (design change). Upstream `SmRegion::GetBrep` follows the region's own pointer into any B-rep, but this
        // lookup borrows only the notifying one. The handlers reach regions through
        // this lookup only for the regions of the merge/split being notified, which
        // all belong to the notifying B-rep; the boolean propagation path resolves
        // regions of both operands through the caller-supplied lookup instead
        // (`boolean::merge::p1`). Covered by `brep_regions_lookup_defers_foreign_key`.
        Err(SolidError::unported("SmRegion::GetBrep", "topology.shell"))
    }
}

/// The identity of a region in a [`RegionNotifyState`]: its arena identity
/// (`loopuse::notify_ref`, `SmRegion_TYPE`), the
/// scheme of [`ArenaNotifyState`], so a caller decoding the events' keys (the heal path's
/// topology tracker) reads every state's keys alike. Any other entity maps to a key
/// without record (`(0, u64::MAX)`, which no arena identity is and the decoder rejects),
/// so notifying its `SmTopology` part through that state is [`SolidError::InvalidInput`],
/// never a silent success.
#[must_use]
pub fn region_only_key(e: EntityRef) -> ObjectRef {
    match e {
        EntityRef::Region(_) => crate::topology::loopuse::notify_ref(e),
        _ => ObjectRef {
            type_code: 0,
            key: ObjectKey(u64::MAX),
        },
    }
}

/// The notification state of a caller that holds no attribute state: the heal path
/// (its API carries no `AttributeStore` at any level, so `SmSAGObject` attribute
/// propagation during healing is not modelled) and difftest adapters. It is an empty
/// attribute store plus a default `SmTopology` record for every region of the B-rep,
/// which such a caller never changes, keyed by [`region_only_key`].
#[derive(Debug, Default)]
pub struct RegionNotifyState {
    /// The regions' `SmTopology` records.
    pub records: crate::topology::topology::NotifyRecords,
    /// The attribute store (empty, or the edited B-rep's own attributes).
    pub attributes: AttributeStore,
}

impl RegionNotifyState {
    /// A default record for every region of `b`.
    #[must_use]
    pub fn new(b: &Brep) -> Self {
        let records = b
            .regions()
            .iter()
            .map(|(r, _)| {
                (
                    region_only_key(EntityRef::Region(r)).key,
                    Topology::default(),
                )
            })
            .collect();
        Self {
            records,
            attributes: AttributeStore::default(),
        }
    }

    /// The [`NotifyEnv`] over this state (no context).
    pub fn env(&mut self) -> NotifyEnv<'_> {
        NotifyEnv {
            records: &mut self.records,
            attributes: &mut self.attributes,
            context: None,
            key_of: &region_only_key,
        }
    }
}

/// The notification state of an arena-only caller: a plain entry point that keeps no
/// `SmTopology` records or attribute store of its own (the `topology.brep_p5` entry points
/// without a notification environment, the heal path, `CopyEdges`). Upstream every object
/// carries its `SmTopology` part and attributes; such a caller models neither, so the
/// notification runs on a default record for every object of the B-rep, keyed by its
/// arena identity (`loopuse::notify_ref`), and an attribute store that is empty, or
/// holds the B-rep's own attributes for the edit ([`Self::with_attributes_of`]). The records are never read back: they exist so that
/// `SmTopology::Notify`'s flag propagation finds both objects, as upstream it always does.
/// The identities are unique within one B-rep only.
#[derive(Debug, Default)]
pub struct ArenaNotifyState {
    /// Each object's default `SmTopology` record.
    pub records: crate::topology::topology::NotifyRecords,
    /// The (empty) attribute store.
    pub attributes: AttributeStore,
}

impl ArenaNotifyState {
    /// A default record for every object of `b` (and the B-rep itself).
    #[must_use]
    pub fn new(b: &Brep) -> Self {
        let mut state = Self::default();
        state.seed(b);
        state
    }

    /// Adds a default record for every object of `b` that has none yet (a second B-rep
    /// of the same edit, or objects the edit created).
    pub fn seed(&mut self, b: &Brep) {
        use crate::topology::loopuse::notify_ref;
        let mut all = vec![EntityRef::Brep(BrepId::from_index(0))];
        all.extend(b.regions().iter().map(|(x, _)| EntityRef::Region(x)));
        all.extend(b.shells().iter().map(|(x, _)| EntityRef::Shell(x)));
        all.extend(b.faceuses().iter().map(|(x, _)| EntityRef::Faceuse(x)));
        all.extend(b.faces().iter().map(|(x, _)| EntityRef::Face(x)));
        all.extend(b.loopuses().iter().map(|(x, _)| EntityRef::Loopuse(x)));
        all.extend(b.loops().iter().map(|(x, _)| EntityRef::Loop(x)));
        all.extend(b.edgeuses().iter().map(|(x, _)| EntityRef::Edgeuse(x)));
        all.extend(b.edges().iter().map(|(x, _)| EntityRef::Edge(x)));
        all.extend(b.vertexuses().iter().map(|(x, _)| EntityRef::Vertexuse(x)));
        all.extend(b.vertices().iter().map(|(x, _)| EntityRef::Vertex(x)));
        if self.records.is_empty() {
            // Bulk build (sorted input is O(n)); the same map as entry-by-entry.
            self.records = all
                .into_iter()
                .map(|e| (notify_ref(e).key, Topology::default()))
                .collect();
            return;
        }
        for e in all {
            self.records.or_default(notify_ref(e).key);
        }
    }

    /// [`Self::new`] holding `b`'s own attributes ([`Brep::add_attribute`], keyed by the
    /// same arena identities), so the edit's notifications propagate them with SMLib's
    /// handlers, as upstream's live on the objects the edit changes. `b` holds none
    /// until [`Self::restore_attributes`] gives them back. Only for an edit of `b`
    /// alone: a second B-rep [`Self::seed`]ed in shares its identities.
    pub(crate) fn with_attributes_of(b: &mut Brep) -> Self {
        let mut state = Self::new(b);
        state.attributes = b.take_attributes();
        state
    }

    /// Gives `b` the attributes this state's notifications left on its live objects
    /// (pair of [`Self::with_attributes_of`]); the lists of objects the edit deleted
    /// are cleared, as upstream's destructors do.
    ///
    /// # Errors
    /// None in practice.
    pub(crate) fn restore_attributes(&mut self, b: &mut Brep) -> Result<()> {
        restore_decoded_attributes(
            b,
            core::mem::take(&mut self.attributes),
            &crate::topology::loopuse::entity_of_notify_key,
        )
    }

    /// The [`NotifyEnv`] over this state, notifying through `context` (the kernel's,
    /// when the caller holds it mutably: its notify callbacks see the events, as upstream
    /// the B-rep's context's do).
    pub fn env<'a>(
        &'a mut self,
        context: Option<&'a mut crate::geom::context::Context>,
    ) -> NotifyEnv<'a> {
        NotifyEnv {
            records: &mut self.records,
            attributes: &mut self.attributes,
            context,
            key_of: &crate::topology::loopuse::notify_ref,
        }
    }
}

/// (not upstream: upstream attributes live on the objects.) Moves `source`, a B-rep's
/// own attribute store (keyed by [`crate::topology::loopuse::notify_ref`]), into an
/// operation's `store` under that operation's identities `key_of` of the same objects.
///
/// # Errors
/// [`SolidError::InvalidInput`] for a list whose key names no object; a store that
/// cannot grow.
pub(crate) fn seed_brep_attributes(
    store: &mut AttributeStore,
    source: AttributeStore,
    key_of: &dyn Fn(EntityRef) -> ObjectRef,
) -> Result<()> {
    store.absorb(source, &|key| {
        crate::topology::loopuse::entity_of_notify_key(key)
            .map(|e| key_of(e).key)
            .ok_or(SolidError::invalid_input(
                "topology::brep::p2::seed_brep_attributes",
                "attribute list names no object",
            ))
    })
}

/// (not upstream) Ends an operation's naming of `b`: `b`'s own attributes become the
/// lists `store` holds for its live objects, named by `decode` (an operation identity
/// to the entity it names); every other list is cleared, as upstream's object
/// destructor does ([`AttributeStore::retain_rekeyed`]).
///
/// # Errors
/// None in practice.
pub(crate) fn restore_decoded_attributes(
    b: &mut Brep,
    mut store: AttributeStore,
    decode: &dyn Fn(ObjectKey) -> Option<EntityRef>,
) -> Result<()> {
    if !store.attributed_objects().is_empty() {
        let live = &*b;
        let listed = live.listed_entities();
        store.retain_rekeyed(&|key| {
            decode(key)
                .filter(|&e| live.still_has(&listed, e))
                .map(|e| crate::topology::loopuse::notify_ref(e).key)
        })?;
    }
    b.set_attributes(store);
    Ok(())
}

/// [`restore_decoded_attributes`] for identities `key_of` without a decoder: `b`'s
/// live objects are named by `key_of` and given the lists `store` holds for them;
/// `store` itself is left as it is.
///
/// # Errors
/// None in practice.
pub(crate) fn restore_keyed_attributes(
    b: &mut Brep,
    store: &AttributeStore,
    key_of: &dyn Fn(EntityRef) -> ObjectRef,
) -> Result<()> {
    if store.attributed_objects().is_empty() {
        b.set_attributes(AttributeStore::default());
        return Ok(());
    }
    let names: BTreeMap<ObjectKey, EntityRef> = live_entities(b)
        .into_iter()
        .map(|e| (key_of(e).key, e))
        .collect();
    restore_decoded_attributes(b, store.clone(), &|key| names.get(&key).copied())
}

/// Every live object of `b` that can carry attributes (all but the B-rep itself).
fn live_entities(b: &Brep) -> Vec<EntityRef> {
    let mut all = Vec::new();
    all.extend(b.regions().iter().map(|(x, _)| EntityRef::Region(x)));
    all.extend(b.shells().iter().map(|(x, _)| EntityRef::Shell(x)));
    all.extend(b.faceuses().iter().map(|(x, _)| EntityRef::Faceuse(x)));
    all.extend(b.faces().iter().map(|(x, _)| EntityRef::Face(x)));
    all.extend(b.loopuses().iter().map(|(x, _)| EntityRef::Loopuse(x)));
    all.extend(b.loops().iter().map(|(x, _)| EntityRef::Loop(x)));
    all.extend(b.edgeuses().iter().map(|(x, _)| EntityRef::Edgeuse(x)));
    all.extend(b.edges().iter().map(|(x, _)| EntityRef::Edge(x)));
    all.extend(b.vertexuses().iter().map(|(x, _)| EntityRef::Vertexuse(x)));
    all.extend(b.vertices().iter().map(|(x, _)| EntityRef::Vertex(x)));
    all
}

/// The identities a rebuilt cache keys its edge/face owners by.
fn cache_keys(env: &NotifyEnv<'_>, object: CacheObject) -> ObjectRef {
    caller_cache_key(env.key_of, object)
}

/// The identity of a cache object under the caller's topology naming `key_of` (the
/// names its edges, faces and vertices own their geometry under); other objects keep
/// their [`brep_cache::arena_identity`].
pub(crate) fn caller_cache_key(
    key_of: &dyn Fn(EntityRef) -> ObjectRef,
    object: CacheObject,
) -> ObjectRef {
    match object {
        CacheObject::Edge(e) | CacheObject::Curve(e) => key_of(EntityRef::Edge(e)),
        CacheObject::Face(f) | CacheObject::Surface(f) => key_of(EntityRef::Face(f)),
        CacheObject::Vertex(v) => key_of(EntityRef::Vertex(v)),
        other => brep_cache::arena_identity(other),
    }
}

/// The spatial tree an added object goes to.
#[derive(Clone, Copy)]
enum TreeKind {
    Vertex,
    Curve,
    Surface,
}

/// `pTree->AddToSpatialTree(box, object)`; on failure the cache is deleted and
/// rebuilt (`SmCacheMgr::GetOrCreateObjectCache`, which leaves no cache when the
/// build fails softly).
fn add_to_tree(
    b: &Brep,
    env: &NotifyEnv<'_>,
    slot: &mut Option<&mut BrepCacheSlot<'_>>,
    kind: TreeKind,
    bbox: &Extent3d,
    object: Option<CacheObject>,
) -> Result<()> {
    let Some(s) = slot.as_deref_mut() else {
        return Ok(());
    };
    let Some(cache) = s.cache.as_mut() else {
        return Ok(());
    };
    let tree = match kind {
        TreeKind::Vertex => cache.vertex_tree_mut(),
        TreeKind::Curve => cache.curve_tree_mut(),
        TreeKind::Surface => cache.surface_tree_mut(),
    }
    .ok_or(null_deref("B-rep cache without spatial tree"))?;
    match tree.add_to_spatial_tree(bbox, object, NodeGeomType::Default) {
        Ok(()) => return Ok(()),
        Err(e) if e.is_unported() => return Err(e),
        Err(_) => {}
    }
    let kernel = s.kernel;
    *s.cache = None;
    let rebuilt = {
        let keys = |o: CacheObject| cache_keys(env, o);
        brep_cache::make_brep_cache(kernel, CacheSource::Brep(b), &keys)
    };
    let cache = match rebuilt {
        Ok(c) => Some(c),
        Err(e) if e.is_unported() => return Err(e),
        Err(_) => None,
    };
    if let Some(s) = slot.as_deref_mut() {
        *s.cache = cache;
    }
    Ok(())
}

/// `pTree->RmFromSpatialTree(box, object)`; a miss only asserts upstream.
fn rm_from_tree(
    slot: &mut Option<&mut BrepCacheSlot<'_>>,
    kind: TreeKind,
    bbox: &Extent3d,
    object: CacheObject,
) -> Result<()> {
    let Some(s) = slot.as_deref_mut() else {
        return Ok(());
    };
    let Some(cache) = s.cache.as_mut() else {
        return Ok(());
    };
    let tree = match kind {
        TreeKind::Vertex => cache.vertex_tree_mut(),
        TreeKind::Curve => cache.curve_tree_mut(),
        TreeKind::Surface => cache.surface_tree_mut(),
    }
    .ok_or(null_deref("B-rep cache without spatial tree"))?;
    // SM_ASSERT_MSG when not found
    let _found = tree.rm_from_spatial_tree(bbox, Some(object))?;
    Ok(())
}

/// The kernel of the B-rep's cache slot, when it has a cache.
fn cache_kernel<'k>(slot: &Option<&mut BrepCacheSlot<'k>>) -> Option<&'k Kernel> {
    slot.as_ref()
        .and_then(|s| s.cache.is_some().then_some(s.kernel))
}

/// `sm_AddToSpatialTree`: adds a new edge's curve (box of the edge, grown by its
/// tolerance), vertex (its point grown by its tolerance) or face's surface (box of the
/// face, grown by its tolerance) to the B-rep cache's tree; nothing without cache.
fn add_to_spatial_tree(
    b: &Brep,
    env: &NotifyEnv<'_>,
    slot: &mut Option<&mut BrepCacheSlot<'_>>,
    new: Option<NotifyArg>,
) -> Result<()> {
    let Some(NotifyArg::Entity(entity)) = new else {
        return Ok(());
    };
    // when brep has a cache
    let Some(k) = cache_kernel(slot) else {
        return Ok(());
    };
    match entity {
        // add new edge->Curve to BrepCache->CurveTree
        EntityRef::Edge(e) => {
            let data = b.edge(e)?;
            // Get edge bounding box (status ignored: the box stays initialised)
            let mut bbox = edge_box(k, b, e)?;
            bbox.expand_absolute(data.tolerance);
            let curve = data.curve.as_ref().map(|_| CacheObject::Curve(e));
            add_to_tree(b, env, slot, TreeKind::Curve, &bbox, curve)
        }
        // add new vertex->Point to BrepCache->VertexTree
        EntityRef::Vertex(v) => {
            let data = b.vertex(v)?;
            let mut bbox = Extent3d::from_point(data.point);
            bbox.expand_absolute(data.tolerance);
            add_to_tree(
                b,
                env,
                slot,
                TreeKind::Vertex,
                &bbox,
                Some(CacheObject::Vertex(v)),
            )
        }
        // add new face->surface to BrepCache->SurfaceTree
        EntityRef::Face(f) => {
            let data = b.face(f)?;
            let mut bbox = face_box(k, b, f)?;
            bbox.expand_absolute(data.tolerance);
            let surface = data.surface.as_ref().map(|_| CacheObject::Surface(f));
            add_to_tree(b, env, slot, TreeKind::Surface, &bbox, surface)
        }
        _ => Ok(()),
    }
}

/// `sm_RmFromSpatialTree`: removes an edge's curve (box of the curve over the edge
/// interval, its natural interval for a wire; only when the edge owns the curve), a
/// vertex (box of its point) or a face's surface (box of the face; only when the face
/// owns the surface) from the B-rep cache's tree; nothing without cache.
fn rm_from_spatial_tree(
    b: &Brep,
    env: &NotifyEnv<'_>,
    slot: &mut Option<&mut BrepCacheSlot<'_>>,
    old: Option<NotifyArg>,
) -> Result<()> {
    let Some(NotifyArg::Entity(entity)) = old else {
        return Ok(());
    };
    // when brep has a cache
    let Some(k) = cache_kernel(slot) else {
        return Ok(());
    };
    match entity {
        EntityRef::Edge(e) => {
            let data = b.edge(e)?;
            let this = (env.key_of)(EntityRef::Edge(e));
            // skip edges that are part of a composite edge
            let Some(curve) = data.curve.as_ref() else {
                return Ok(());
            };
            if curve.owner() != Some(this) {
                return Ok(());
            }
            // the full curve box for wires (the curve may be shared)
            let domain = if edge::is_wire(b, e)? {
                curve.natural_interval()
            } else {
                data.interval
            };
            // get obj bounding box (status ignored: the box stays initialised)
            let req = BoundingBoxRequest {
                normal: true,
                pseudo: false,
                polar: false,
            };
            let bbox = match curve.calculate_bounding_box(k, &domain, req, true) {
                Ok(boxes) => boxes.normal.unwrap_or_else(Extent3d::new),
                Err(err) if err.is_unported() => return Err(err),
                Err(_) => Extent3d::new(),
            };
            rm_from_tree(slot, TreeKind::Curve, &bbox, CacheObject::Curve(e))
        }
        EntityRef::Vertex(v) => {
            let bbox = Extent3d::from_point(b.vertex(v)?.point);
            rm_from_tree(slot, TreeKind::Vertex, &bbox, CacheObject::Vertex(v))
        }
        EntityRef::Face(f) => {
            let data = b.face(f)?;
            let this = (env.key_of)(EntityRef::Face(f));
            let surface = data
                .surface
                .as_ref()
                .ok_or(null_deref("face without surface"))?;
            // when face is not a part of a composite face
            if surface.owner() != Some(this) {
                return Ok(());
            }
            let bbox = face_box(k, b, f)?;
            rm_from_tree(slot, TreeKind::Surface, &bbox, CacheObject::Surface(f))
        }
        _ => Ok(()),
    }
}

/// `SmEdge::CalculateBoundingBox(&box)` with its status ignored (the box stays
/// initialised on failure).
fn edge_box(k: &Kernel, b: &Brep, e: EdgeId) -> Result<Extent3d> {
    let req = BoundingBoxRequest {
        normal: true,
        pseudo: false,
        polar: false,
    };
    match qedge::calculate_bounding_box(k, b, e, req, false) {
        Ok(boxes) => Ok(boxes.normal.unwrap_or_else(Extent3d::new)),
        Err(err) if err.is_unported() => Err(err),
        Err(_) => Ok(Extent3d::new()),
    }
}

/// `SmFace::CalculateBoundingBox(box)` (default `bTight = FALSE`) with its status ignored
/// (`SE`; the box stays initialised on failure), as [`edge_box`].
fn face_box(k: &Kernel, b: &Brep, f: FaceId) -> Result<Extent3d> {
    match qface1::calculate_bounding_box(k, b, f, false) {
        Ok(bbox) => Ok(bbox),
        Err(err) if err.is_unported() => Err(err),
        Err(_) => Ok(Extent3d::new()),
    }
}

// ---- caches -------------------------------------------------------------------------------

/// `DeleteStaleCaches`: deletes the trimmed-surface cache (`SM_OC_TRIMSRF`, here the
/// caller-held `caches` keyed by face) of every face with a surface whose cache is
/// marked `m_bFaceWasModified`.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id.
pub fn delete_stale_caches<O: Copy + PartialEq>(
    b: &Brep,
    caches: &mut BTreeMap<FaceId, TrimSrfCache<O>>,
) -> Result<()> {
    // For every Face - delete stale Surface caches
    for f in brep_faces(b)? {
        // skip faces without surfaces or without caches
        if b.face(f)?.surface.is_none() {
            continue;
        }
        let Some(cache) = caches.get(&f) else {
            continue;
        };

        // if the cache is marked as modified - delete the cache
        if cache.base().face_was_modified {
            caches.remove(&f);
        }
    }
    Ok(())
}

// ---- solutions ----------------------------------------------------------------------------

/// `sm_GetPointFromSol` (file static, used by `SmBrep::ProjectAndTrim`): the 3D point of
/// object `index` of a topology-solver solution: a vertex's point, an edge's (or edge
/// curve's) curve at its parameter, or a face's surface at its two parameters. Earlier
/// objects skip one parameter per curve or edge and two per face (by kind, also for
/// objects the B-rep does not hold). An object of any other kind gives the origin
/// (upstream signals `SM_ERR` and returns `(0,0,0)`).
///
/// # Errors
///
/// [`SolidError::InvalidInput`] where upstream reads a null object, an edge without
/// curve, a face without surface or a parameter past the solution's, and for a curve,
/// edge or face object the B-rep does not hold (the port cannot evaluate it); the evaluation's
/// error (upstream ignores it and returns an uninitialised point).
pub fn point_from_solution(
    k: &Kernel,
    b: &Brep,
    solution: &Solution<CacheObject>,
    index: usize,
) -> Result<Point3> {
    let param = |i: usize| -> Result<f64> {
        solution
            .start
            .parameters
            .get(i)
            .copied()
            .ok_or(null_deref("solution parameter out of range"))
    };
    let mut start = 0usize;
    for ii in 0..=index {
        // Get the object from the solution.
        let object = solution
            .objects
            .get(ii)
            .copied()
            .flatten()
            .ok_or(null_deref("null solution object"))?;
        if ii == index {
            match object {
                CacheObject::Vertex(v) => return Ok(b.vertex(v)?.point),
                CacheObject::Edge(e) | CacheObject::Curve(e) => {
                    let curve = b
                        .edge(e)?
                        .curve
                        .as_ref()
                        .ok_or(null_deref("edge without curve"))?;
                    // Evaluate the curve at the current parameter
                    return curve.evaluate_point(k, param(start)?);
                }
                CacheObject::Face(f) => {
                    let surface = b
                        .face(f)?
                        .surface
                        .as_ref()
                        .ok_or(null_deref("face without surface"))?;
                    // Initialize a 2D point using two parameters from the solution
                    let uv = Point2::new(param(start)?, param(start + 1)?);
                    return surface.evaluate_point(k, uv);
                }
                // a curve, edge or face the B-rep does not hold: upstream evaluates it
                CacheObject::Other(r)
                    if is_curve_class(r.type_code)
                        || edge::TYPE == r.type_code
                        || face2::TYPE == r.type_code =>
                {
                    return Err(null_deref("solution object outside the B-rep"));
                }
                _ => {}
            }
        } else {
            // This is not the index we want, move down to the next start
            // parameter based on the type of the object.
            match object {
                CacheObject::Curve(_) | CacheObject::Edge(_) => start += 1,
                CacheObject::Face(_) => start += 2,
                CacheObject::Other(r) => {
                    if is_curve_class(r.type_code) || edge::TYPE == r.type_code {
                        start += 1;
                    }
                    if face2::TYPE == r.type_code {
                        start += 2;
                    }
                }
                _ => {}
            }
        }
    }
    // SE(SM_ERR)
    Ok(Point3::new(0.0, 0.0, 0.0))
}

// ---- classification -----------------------------------------------------------------------

/// The region of an edge use's shell (`pEU->GetShell()->GetRegion()`).
fn edgeuse_region(b: &Brep, eu: EdgeuseId) -> Result<Option<RegionId>> {
    let s = eu1::shell(b, eu)?.ok_or(null_deref("edge use without shell"))?;
    shell_region(b, s)
}

/// `LocalCurveSector`: the region containing the curve interval between the curve's
/// start ([`OrientType::Same`]) or end ([`OrientType::Opposite`]) and its known
/// intersection `hit` with the B-rep, found by stepping `10 * GetTolerance()` along
/// the curve and sectoring the face, the edge's radial sectors, or the faces and edges
/// around the vertex hit (voting for the most frequent region). `None` when the vote
/// finds no region.
///
/// # Errors
///
/// `SM_ERR` for a hit that is not a face, edge or vertex, for an edge whose sectoring
/// finds no edge use, and for a vertex use that is neither an edge nor a loop vertex
/// use; errors of the curve evaluations and sectoring; [`SolidError::InvalidInput`]
/// where upstream dereferences null; [`SolidError::Unported`] from
/// the sectoring callees.
pub fn local_curve_sector(
    k: &Kernel,
    b: &mut Brep,
    curve: &Curve,
    interval: &Extent1d,
    orientation: OrientType,
    hit: &PointClassification<EntityRef>,
) -> Result<Option<RegionId>> {
    // Do this thing by stepping off the curve end a little and finding
    // the closest face point.  Compute the normals of the found face
    // and see which one is toward the point.

    // evaluate Curve EndPoint tangent at intersection
    let mut test_dir = if orientation == OrientType::Same {
        let pv = curve.evaluate(k, interval.min(), 1, true, true)?;
        *pv.get(1).ok_or(null_deref("missing tangent"))?
    } else {
        let pv = curve.evaluate(k, interval.max(), 1, false, true)?;
        -*pv.get(1).ok_or(null_deref("missing tangent"))?
    };

    // evaluate Curve NearEndPoint - this is for classifying vertex corners where this
    // point is dropped to all vertex->faces
    let d3_step = b.tolerance() * 10.0;
    let (length, _) = curve.approximate_length(k, interval, 10, false)?;
    let t_step = d3_step / length * (interval.max() - interval.min());
    let t_test = if orientation == OrientType::Same {
        interval.min() + t_step
    } else {
        interval.max() - t_step
    };
    let test_pnt = curve.evaluate_point(k, t_test)?;
    test_dir *= t_step / test_dir.length();

    // switch on the intersection type
    match (hit.point_class(), hit.object()) {
        (PointClassType::Face, Some(EntityRef::Face(f))) => {
            // Just find the local sectoring of this one face.
            let uv_guess = hit.uv_param();
            let (_dist, region) =
                p1::find_sector_of_face(k, b, f, Some(uv_guess), test_pnt, test_dir)?;
            Ok(region)
        }
        (PointClassType::Edge, Some(EntityRef::Edge(e))) => {
            // when intersecting a wire - the region is just the wire's region
            if edge::is_wire(b, e)? {
                let eu = eu1::primary_edgeuse(b, e)?.ok_or(null_deref("edge without edge use"))?;
                return edgeuse_region(b, eu);
            }

            // classify the curve against the radial sectors of the intersected edge
            let sector = edge::find_radial_sector_of_curve(
                k,
                b,
                e,
                hit.t_param(),
                curve,
                interval,
                orientation,
                true,
            )?;
            let found = sector.radial_edgeuse.ok_or(SolidError::sm_err(OP))?;

            // region of point is region of classification sector
            edgeuse_region(b, found)
        }
        (PointClassType::Vertex, Some(EntityRef::Vertex(v))) => {
            // when intersecting a shellVertex - the region is just the vertex's region
            if vertex::is_shell_vertex(b, v)? {
                let vu =
                    vertex::primary_vertexuse(b, v)?.ok_or(null_deref("vertex without use"))?;
                let s = vu1::shell(b, vu)?.ok_or(null_deref("vertex use without shell"))?;
                return shell_region(b, s);
            }

            // Find the local sectoring of all of the faces around
            // this vertex and use the one which is closest to the point.
            let mut regions: Vec<Option<RegionId>> = Vec::new();
            let mut votes: Vec<u64> = Vec::new();

            // for every vertexuse
            for vu in vertex::vertexuses(b, v)? {
                if vu1::is_edge_vertexuse(b, vu)? {
                    // vertex connects to an edge
                    let Some(eu) = vu1::edgeuse(b, vu)? else {
                        continue;
                    };

                    // get region by classifying Curve against vertex->Edge
                    let e = b
                        .edgeuse(eu)?
                        .edge
                        .ok_or(null_deref("edge use without edge"))?;
                    let ivl = b.edge(e)?.interval;
                    let edge_t = if b.edgeuse(eu)?.orientation == OrientType::Same {
                        ivl.min()
                    } else {
                        ivl.max()
                    };
                    let sector = edge::find_radial_sector_of_curve(
                        k,
                        b,
                        e,
                        edge_t,
                        curve,
                        interval,
                        orientation,
                        true,
                    )?;
                    let Some(found) = sector.radial_edgeuse else {
                        continue;
                    };
                    let region = edgeuse_region(b, found)?;
                    p1::vote_for_region(&mut regions, &mut votes, region);
                } else {
                    // when vertex DOES NOT connect to an edge
                    if vu1::is_loop_vertexuse(b, vu)? {
                        // TestPnt can be classified against vertexuse->Face
                        let fu = vu1::faceuse(b, vu)?
                            .ok_or(null_deref("vertex use without face use"))?;
                        let f = b
                            .faceuse(fu)?
                            .face
                            .ok_or(null_deref("face use without face"))?;
                        let (_dist, found) =
                            p1::find_sector_of_face(k, b, f, None, test_pnt, test_dir)?;
                        p1::vote_for_region(&mut regions, &mut votes, found);
                    } else {
                        return Err(SolidError::sm_err(OP));
                    }
                }
            }

            p1::find_highest_vote(&regions, &votes)
        }
        _ => Err(SolidError::sm_err(OP)),
    }
}

/// What [`ray_fire`] found.
#[derive(Debug, Clone, PartialEq)]
pub struct RayFireResult {
    /// `rpRegionOfPoint`: the region containing the ray's start point.
    pub region: Option<RegionId>,
    /// `rRayIntersection`: the best ray/B-rep intersection (unknown when the ray hits
    /// nothing).
    pub intersection: PointClassification<EntityRef>,
}

/// The topology entity a B-rep cache object stands for (also the solution objects of
/// `SmBrep::Point3DClassify`'s boundary solve, unit `query.brep`).
pub(crate) fn cache_entity(object: CacheObject) -> Result<EntityRef> {
    match object {
        CacheObject::Vertex(v) => Ok(EntityRef::Vertex(v)),
        CacheObject::Edge(e) | CacheObject::Curve(e) => Ok(EntityRef::Edge(e)),
        CacheObject::Face(f) | CacheObject::Surface(f) => Ok(EntityRef::Face(f)),
        _ => Err(null_deref("ray hit an object outside the B-rep")),
    }
}

/// `RayFire`: fires a ray from `start` along `direction` through the B-rep (solver
/// operation `SM_SO_RAYFIRE` on the B-rep with its `cache`; `brep_id`, `keys` and
/// `owner_of` as in [`BrepObjects`](crate::topology::brep_cache::BrepObjects)) and classifies
/// the start point: the infinite region (or the first region when there is none) when
/// nothing is hit, else the region [`local_curve_sector`] finds stepping back from the
/// closest, lowest dimensional hit along the ray. `answer_limit` bounds the distance
/// searched (`pOptAnswerLimit`).
///
/// # Errors
///
/// `SM_ERR` when the best hit is closer than `SM_EFF_ZERO * (1 + max |start|)`; errors
/// of the B-rep box, the solve, the classification load and [`local_curve_sector`];
/// [`SolidError::Unported`] from the callees.
#[allow(clippy::too_many_arguments)] // one upstream routine
pub fn ray_fire(
    k: &Kernel,
    b: &mut Brep,
    brep_id: BrepId,
    cache: &BrepCache,
    keys: &dyn Fn(CacheObject) -> ObjectRef,
    owner_of: &dyn Fn(ObjectRef) -> Option<ObjectRef>,
    start: Point3,
    direction: Vec3,
    ray_zone_tol_3d: f64,
    answer_limit: Option<f64>,
) -> Result<RayFireResult> {
    ray_fire_with_solver(
        k,
        b,
        start,
        direction,
        ray_zone_tol_3d,
        answer_limit,
        |b, point, options| {
            let identity = crate::query::brep::BrepIdentity {
                brep_id,
                keys,
                owner_of,
            };
            crate::query::brep::solve_mutable_brep_point(k, b, &identity, cache, point, options)
        },
    )
}

/// Shared numeric ray selection and sector classification; the caller supplies the
/// solve's geometry access, including mutable lazy preparation when available.
pub(crate) fn ray_fire_with_solver(
    k: &Kernel,
    b: &mut Brep,
    start: Point3,
    direction: Vec3,
    ray_zone_tol_3d: f64,
    answer_limit: Option<f64>,
    solve: impl FnOnce(
        &mut Brep,
        Point3,
        TopologySolveOptions<'_>,
    ) -> Result<crate::geom::solution_array::SolutionArray<CacheObject>>,
) -> Result<RayFireResult> {
    // get Brep BoundingBox and BoundingSphere
    let bbox = qbrep::calculate_bounding_box(k, &mut *b, BoundingBoxOptions::default())?;
    let sphere = bbox.sphere_bound();
    let radius = sphere.radius;

    // let sVecs[0] = unit ray direction
    let ray_end = start + (2.0 * radius) * direction;
    let ray = (ray_end - start).unitize()?;

    // cast ray through Brep
    let best_sq = answer_limit.map_or(BIG_DOUBLE, |l| l * l);
    let vectors = [ray];
    let solutions = solve(
        b,
        start,
        TopologySolveOptions {
            operation: SolverOperation::Rayfire,
            requested: SolutionRequested::All,
            tolerance_3d: ray_zone_tol_3d,
            best_answer_so_far_sq: best_sq,
            opt_vectors: &vectors,
        },
    )?;

    let mut intersection = PointClassification::new(ray_zone_tol_3d);

    // when no intersections are found - set region = infiniteRegion
    let Some(first) = solutions.get(0) else {
        // But sometimes callers will set the infinite region to Null... [B111]
        let region = match b.infinite_region() {
            Some(r) => Some(r),
            None => b.region_list_head(),
        };
        intersection.set_class_object(PointClassType::Unknown, None);
        return Ok(RayFireResult {
            region,
            intersection,
        });
    };

    // check all solutions to find closest one with lowest dimensional object
    let mut best = *first;
    for ii in 1..solutions.len() {
        let this = *solutions.get(ii).ok_or(null_deref("solution index"))?;
        let scaled_zero = EFF_ZERO_SQRT * 100.0 * (1.0 + radius);
        if this.start.solution_value - best.start.solution_value < scaled_zero {
            // Hit the same thing take the lowest dimensional element
            // -- it is the one with the lowest number of variables
            // If there is a tie just use the start value below.
            if this.num_variables < best.num_variables {
                best = this;
                continue;
            }
            if this.num_variables > best.num_variables {
                continue;
            }
        }
        if this.start.param(0)? < best.start.param(0)? {
            best = this;
        }
    }

    // when ray solution distance is negative (the region is overwritten below)
    let mut region = None;
    if best.start.solution_value < -EFF_ZERO {
        region = b.infinite_region();
        intersection.set_class_object(PointClassType::Unknown, None);
    }

    // check solution tolerance for an error
    let tol = EFF_ZERO * (1.0 + start.max_dimension());
    if best.start.solution_value < tol {
        return Err(SolidError::sm_err(OP));
    }
    let _ = region;

    // load best Ray Solution into output PointClassification
    let hit = cache_entity(best.objects[0].ok_or(null_deref("null solution object"))?)?;
    intersection.load_from_intersection(
        ray_zone_tol_3d,
        hit,
        &best.start,
        0,
        best.start.solution_value,
    )?;

    // make temporary [StartPoint,IntersectionPoint] line segement
    let end = start + best.start.solution_value * ray;
    let line = Curve::from(BSplineCurve::create_line_segment(3, start, end)?);
    let ivl = line.natural_interval();

    // Classify from the end of the ray -- the point on the Edge -- back towards
    // the ray point: Orientation is Opposite.
    let region = local_curve_sector(k, b, &line, &ivl, OrientType::Opposite, &intersection)?;
    Ok(RayFireResult {
        region,
        intersection,
    })
}

// ---- geometry replacement -----------------------------------------------------------------

/// What [`replace_surface`] does with the face's old surface (`bAddToDeletedList`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OldSurface {
    /// `TRUE`: add it to the B-rep's deleted-surface list.
    AddToDeletedList,
    /// `FALSE`: delete it.
    Delete,
    /// `UNSURE`: neither; it is handed back.
    Keep,
}

/// `ReplaceSurface`: gives face `f` the surface `new_surface` (owned by the face) and
/// its natural UV domain, removes the face's UV trim curves, then disposes of the old
/// surface as `old` says and, with `create_uv_trim_curves` (upstream default `TRUE`),
/// rebuilds the UV trim curves. Returns the old surface for [`OldSurface::Keep`] and
/// the modified faces (`pOptFacesModified`: this face).
///
/// Composites were removed upstream: the face owning the old surface
/// (`pOld->GetFace()`) is the face itself.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for a face without surface (upstream dereferences it);
/// [`SolidError::Unported`] from the callees of `SmFace::Notify` ([`face3::notify`]:
/// `GetUpwardEdgeuses`, `topology.face_p4`, for `POST_EDIT`), also reached through
/// [`face1::set_surface_notified`] and [`face1::remove_uv_trim_curves`], and of
/// `SmFace::CreateUVTrimCurves` ([`face3::create_uv_trim_curves`]).
#[allow(clippy::too_many_arguments)] // one upstream routine
pub fn replace_surface(
    k: &Kernel,
    b: &mut Brep,
    env: &mut NotifyEnv<'_>,
    f: FaceId,
    mut new_surface: Surface,
    face_ref: ObjectRef,
    old: OldSurface,
    create_uv_trim_curves: bool,
) -> Result<(Option<Surface>, Vec<FaceId>)> {
    // locals
    let new_domain = new_surface.natural_uv_domain_with_nurb(k)?;
    if b.face(f)?.surface.is_none() {
        return Err(null_deref("face without surface"));
    }
    // pTopFace = pOld->GetFace(): the surface's owner, `f` itself in the arena model
    let top_face = f;

    // inform the face
    face_notify(b, env, f, face_ref, NotifyOperation::PreEdit)?;

    // change surface<->owner pointers
    let _ = new_surface.set_owner(Some(face_ref));
    // the surfaces as the notification names them (issued by the caller's context)
    let surface_refs = match env.context.as_deref_mut() {
        Some(ctx) => {
            let new_ref = surface_identity(ctx, &mut new_surface)?;
            let old_ref = match b.face_mut(top_face)?.surface.as_mut() {
                Some(s) => Some(surface_identity(ctx, s)?),
                None => None,
            };
            [Some(new_ref), old_ref]
        }
        None => [None, None],
    };
    // calls SmFace::Notify(SM_NO_CHANGE_GEOMETRY, ..) which tells all Face SmEdgeuses
    // and SmVertexuses to delete all their cached data
    let old_surface = face1::set_surface_notified(
        k,
        b,
        env,
        top_face,
        face_ref,
        new_surface,
        false,
        surface_refs,
    )?;

    // default faceDomain to NewSurface Domain
    face2::set_uv_domain(b, top_face, &new_domain)?;

    // remove the isoparameter curves
    face1::remove_uv_trim_curves(b, top_face)?;

    // when asked - remember the modified faces
    let modified = vec![f];

    // inform the face
    face_notify(b, env, f, face_ref, NotifyOperation::PostEdit)?;

    // either remember, delete, or ignore the old surface
    let kept = match old {
        OldSurface::AddToDeletedList => {
            if let Some(s) = old_surface {
                b.add_deleted_surface(s);
            }
            None
        }
        OldSurface::Delete => None,
        OldSurface::Keep => old_surface,
    };

    // when asked - build new UVTrimCurves
    if create_uv_trim_curves {
        face_create_uv_trim_curves(k, b, env, top_face)?;
    }

    // all done
    Ok((kept, modified))
}

/// The identity `surface` is notified under through `context` ([`Surface::notify_key`];
/// issued by `context` when it has none of its domain).
fn surface_identity(
    context: &mut crate::geom::context::Context,
    surface: &mut Surface,
) -> Result<ObjectRef> {
    let key = context.geometry_key(surface.notify_identity_slot())?;
    Ok(ObjectRef {
        type_code: surface.type_code(),
        key,
    })
}

// ---- topology construction ----------------------------------------------------------------

/// `pRegion` or, for `None`, `GetInfiniteRegion()` (`NULL` is dereferenced upstream).
fn region_or_infinite(b: &Brep, region: Option<RegionId>) -> Result<RegionId> {
    match region {
        Some(r) => {
            b.region(r)?;
            Ok(r)
        }
        None => b
            .infinite_region()
            .ok_or(null_deref("B-rep without infinite region")),
    }
}

/// `MakeShellVertexTopology`: a new vertex shell in `region` (the infinite region for
/// `None`) holding a new vertex (at the origin, with the B-rep's tolerance) through its
/// one vertex use. Returns `(shell, vertex)`.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] without infinite region; [`SolidError::NotFound`] for a
/// dangling region.
pub fn make_shell_vertex_topology(
    b: &mut Brep,
    region: Option<RegionId>,
) -> Result<(ShellId, VertexId)> {
    // Check for region default.
    let region = region_or_infinite(b, region)?;

    // First do allocation; put shell into region's shell list
    let s = b.add_shell_record(Shell {
        region: Some(region),
        ..Shell::default()
    })?;
    region_post_insert_shell(b, region, s)?;

    // Put vertex into brep's vertex list
    let v = b.add_vertex_record(Vertex::default())?;

    // Put vertexuse into vertex's list
    let vu = b.add_vertexuse_record(Vertexuse {
        vertex: Some(v),
        ..Vertexuse::default()
    })?;
    b.vertex_mut(v)?.vertexuses.insert(0, vu);

    // Link up shell and vertexuse
    shell_set_vertexuse(b, s, vu)?;
    b.vertexuse_mut(vu)?.owner = VertexuseOwner::Shell(Some(s));

    // Set tolerance
    let tol = b.tolerance();
    vertex::set_tolerance(b, v, tol, true, true)?;
    Ok((s, v))
}

/// The new edge's records: an edge with two edge uses, each with a vertex use at
/// `start` / `end`, the first the primary edge use (start to end), both wire edge uses
/// of `shell`.
fn link_new_wire_edge(b: &mut Brep, start: VertexId, end: VertexId) -> Result<(EdgeId, EdgeuseId)> {
    // Put edge into brep's edge list
    let e = b.add_edge_record(Edge {
        vertices: [start, end],
        edgeuses: Vec::new(),
        interval: Extent1d::new(),
        tolerance: 0.0,
        curve: None,
    })?;

    // Put VU1 into existing vertex list and VU2 into new vertex
    let vu1_ = b.add_vertexuse_record(Vertexuse {
        vertex: Some(start),
        ..Vertexuse::default()
    })?;
    b.vertex_mut(start)?.vertexuses.push(vu1_);
    let vu2_ = b.add_vertexuse_record(Vertexuse {
        vertex: Some(end),
        ..Vertexuse::default()
    })?;
    b.vertex_mut(end)?.vertexuses.push(vu2_);

    // Put EU1 and EU2 into edge - Make EU1 the Primary Edgeuse
    let eu_1 = b.add_edgeuse_record(Edgeuse {
        edge: Some(e),
        ..Edgeuse::default()
    })?;
    let eu_2 = b.add_edgeuse_record(Edgeuse {
        edge: Some(e),
        ..Edgeuse::default()
    })?;
    b.edge_mut(e)?.edgeuses = vec![eu_1, eu_2];

    // Attach vertexuses to edgeuses
    for (eu, vu) in [(eu_1, vu1_), (eu_2, vu2_)] {
        b.edgeuse_mut(eu)?.vertexuse = Some(vu);
        b.vertexuse_mut(vu)?.owner = VertexuseOwner::Edgeuse(Some(eu));
    }
    Ok((e, eu_1))
}

/// Makes vertex shell `s` the wire shell of `eu` (deleting its vertex use) when it is
/// a vertex shell, and gives both edge uses of `e` the shell.
fn attach_wire_edge(b: &mut Brep, s: ShellId, e: EdgeId, eu: EdgeuseId) -> Result<()> {
    // If start shell was shell vertex make it a wire edge with pointer to newE.
    if shell_is_vertex_shell(b, s)? {
        let vu = shell_vertexuse(b, s)?;
        remove_vertexuse_from_vertex(b, vu)?;
        shell_set_wire_edgeuse(b, s, eu)?;
    }

    // Assign shells to edgeuses
    for eu in b.edge(e)?.edgeuses.clone() {
        b.edgeuse_mut(eu)?.owner = EdgeuseOwner::Shell(Some(s));
    }
    Ok(())
}

/// Removes vertex use `vu` from its vertex's list (`pVU->GetVertex()->Remove(pVU)`;
/// the record stays in the arena, unreachable, as upstream deletes it).
fn remove_vertexuse_from_vertex(b: &mut Brep, vu: VertexuseId) -> Result<()> {
    let v = b
        .vertexuse(vu)?
        .vertex
        .ok_or(null_deref("vertex use without vertex"))?;
    b.vertex_mut(v)?.vertexuses.retain(|&x| x != vu);
    Ok(())
}

/// `MakeEdgeVertexTopology`: a new wire edge in `region` (the infinite region for
/// `None`) from `start` to a new vertex, in the shell of `start` in the region (a
/// vertex shell becomes a wire shell). Returns `(edge, new vertex)`.
///
/// # Errors
///
/// `SM_ERR` when `start` has no shell in the region; [`SolidError::InvalidInput`]
/// without infinite region or where upstream dereferences null (a vertex shell without
/// vertex use); [`SolidError::NotFound`] for a dangling id.
pub fn make_edge_vertex_topology(
    b: &mut Brep,
    region: Option<RegionId>,
    start: VertexId,
) -> Result<(EdgeId, VertexId)> {
    b.vertex(start)?;

    // Check for region default.
    let region = region_or_infinite(b, region)?;

    // Find the shell corresponding to the region from the Start vertex.
    let s = vertex::shell_in_region(b, start, Some(region))?.ok_or(SolidError::sm_err(OP))?;

    // Put Vertex into Brep's vertex list and edge into edge list
    let new_v = b.add_vertex_record(Vertex::default())?;
    let (e, eu) = link_new_wire_edge(b, start, new_v)?;
    attach_wire_edge(b, s, e, eu)?;
    Ok((e, new_v))
}

/// `MakeEdgeTopology`: a new wire edge in `region` (the infinite region for `None`)
/// from `start` to `end`. When the vertices are in different shells of the region the
/// end shell is merged into the start shell (a vertex shell is always the one
/// removed: its vertex use is deleted, else its children are transferred), removed
/// and deleted (`delete pEndShell`: [`shell::destroy`] under `env`). The edge has no
/// curve.
///
/// # Errors
///
/// `SM_ERR` when a vertex has no shell in the region; [`SolidError::InvalidInput`]
/// without infinite region or where upstream dereferences null (a vertex shell without
/// vertex use, a shell without region); those of `TransferChildrenTo`
/// ([`shell::transfer_children`]) and of the shell's destruction
/// ([`shell::destroy`]); [`SolidError::NotFound`] for a dangling id.
pub fn make_edge_topology(
    k: &mut Kernel,
    b: &mut Brep,
    region: Option<RegionId>,
    start: VertexId,
    end: VertexId,
    env: &mut NotifyEnv<'_>,
) -> Result<EdgeId> {
    b.vertex(start)?;
    b.vertex(end)?;

    // Check for region default.
    let region = region_or_infinite(b, region)?;

    // Find the shells corresponding to the region from the Start vertex and end vertex.
    let mut start_shell =
        vertex::shell_in_region(b, start, Some(region))?.ok_or(SolidError::sm_err(OP))?;
    let mut end_shell =
        vertex::shell_in_region(b, end, Some(region))?.ok_or(SolidError::sm_err(OP))?;

    // Transfer stuff from end shell to start shell if they are different.
    if shell_is_vertex_shell(b, start_shell)? && !shell_is_vertex_shell(b, end_shell)? {
        // If we have a shell vertex then make sure that is the one which gets removed.
        core::mem::swap(&mut start_shell, &mut end_shell);
    }

    if start_shell != end_shell {
        if shell_is_vertex_shell(b, end_shell)? {
            let vu = shell_vertexuse(b, end_shell)?;
            remove_vertexuse_from_vertex(b, vu)?;
            shell_clear_members(b, end_shell)?;
        } else {
            shell_transfer_children(k, b, end_shell, start_shell)?;
        }
        region_remove_shell(b, end_shell)?;
        // delete pEndShell (SmBrep.cpp:13724): ~SmShell, ported as shell::destroy
        shell::destroy(k, b, end_shell, env)?;
    }

    let (e, eu) = link_new_wire_edge(b, start, end)?;
    attach_wire_edge(b, start_shell, e, eu)?;
    Ok(e)
}

// ---- more unported callees ----------------------------------------------------------------

/// `SmShell::GetRegion` (`m_pListOwner`, kept on the shell record).
fn shell_region(b: &Brep, s: ShellId) -> Result<Option<RegionId>> {
    Ok(b.shell(s)?.region)
}

/// `pFace->Notify(operation, pFace, this, NULL)` ([`face3::notify`]; upstream ignores
/// the status). `face_ref` is the face's identity. No trimmed surface cache is passed:
/// B-rep surfaces carry no object cache (the caller-held caches of
/// [`delete_stale_caches`] are not threaded here), so `PRE_EDIT`'s cache flag has
/// nothing to mark.
fn face_notify(
    b: &mut Brep,
    env: &mut NotifyEnv<'_>,
    f: FaceId,
    face_ref: ObjectRef,
    operation: NotifyOperation,
) -> Result<()> {
    let data = [Some(face_ref), Some(brep_ref(b)), None];
    ignore_status(face3::notify(b, f, env, None, operation, data))
}

/// `SER(pTopFace->CreateUVTrimCurves(TRUE, NULL, NULL, ..))`
/// ([`face3::create_uv_trim_curves_notified`]: its UV trim curve edits notify through
/// `env`; the gap statistics are dropped).
fn face_create_uv_trim_curves(
    k: &Kernel,
    b: &mut Brep,
    env: &mut NotifyEnv<'_>,
    f: FaceId,
) -> Result<()> {
    face3::create_uv_trim_curves_notified(k, b, f, true, None, Some(env)).map(|_| ())
}

/// `pRegion->PostInsert(pShell)`: the new shell goes to the tail of the region's shell
/// list (`SmOwningTopology::PostInsert`, inline in `SmOwningTopology.h`; the list of
/// [`crate::topology::Region::shells`], as `topology::brep::p6` inserts a face shell).
/// The shell's owner (`m_pListOwner`) is already the region.
fn region_post_insert_shell(b: &mut Brep, r: RegionId, s: ShellId) -> Result<()> {
    b.region_mut(r)?.shells.push(s);
    b.shell_mut(s)?.region = Some(r);
    Ok(())
}

/// `pShell->GetRegion()->Remove(pShell)` ([`shell::region_remove_shell`]); the status
/// is ignored upstream, so a shell missing from its region's list (`SM_ERR`, the
/// port's [`SolidError::InvalidInput`]) goes on. A shell without region is a null
/// dereference.
fn region_remove_shell(b: &mut Brep, s: ShellId) -> Result<()> {
    let r = shell_region(b, s)?.ok_or(null_deref("shell without region"))?;
    match shell::region_remove_shell(b, r, s) {
        Err(SolidError::InvalidInput { .. } | SolidError::PortInvariant { .. }) | Ok(()) => Ok(()),
        Err(e) => Err(e),
    }
}

/// `SmShell::IsVertexShell` (`m_tShellType == SmVertexuse_TYPE`).
fn shell_is_vertex_shell(b: &Brep, s: ShellId) -> Result<bool> {
    Ok(b.shell(s)?.is_vertex_shell())
}

/// The vertex use of vertex shell `s` (`(SmVertexuse*)pShell->m_pList`); a NULL list
/// (or a list of another class) is dereferenced upstream.
fn shell_vertexuse(b: &Brep, s: ShellId) -> Result<VertexuseId> {
    match b.shell(s)?.members {
        ShellMembers::Vertexuse(vu) => Ok(vu),
        _ => Err(null_deref("vertex shell without vertex use")),
    }
}

/// Makes `s` a vertex shell of `vu` (`m_tShellType = SmVertexuse_TYPE`,
/// `m_pList = pNewVU`; `UpdateListSize` has no Rust counterpart).
fn shell_set_vertexuse(b: &mut Brep, s: ShellId, vu: VertexuseId) -> Result<()> {
    let shell = b.shell_mut(s)?;
    shell.shell_type = ShellType::Vertex;
    shell.members = ShellMembers::Vertexuse(vu);
    Ok(())
}

/// Makes `s` the wire shell of `eu` (`m_pList = NULL`, `m_tShellType =
/// SmEdgeuse_TYPE`, `m_pList = pNewEU1`).
fn shell_set_wire_edgeuse(b: &mut Brep, s: ShellId, eu: EdgeuseId) -> Result<()> {
    let shell = b.shell_mut(s)?;
    shell.shell_type = ShellType::Wire;
    shell.members = ShellMembers::Edgeuse(eu);
    Ok(())
}

/// Empties shell `s` (`m_pList = NULL`; the type is left, as upstream).
fn shell_clear_members(b: &mut Brep, s: ShellId) -> Result<()> {
    b.shell_mut(s)?.members = ShellMembers::Empty;
    Ok(())
}

/// `SER(pFrom->TransferChildrenTo(pTo, NULL))` ([`shell::transfer_children`] without
/// wire-edge candidates).
fn shell_transfer_children(k: &mut Kernel, b: &mut Brep, from: ShellId, to: ShellId) -> Result<()> {
    shell::transfer_children(k, b, from, to, None)
}

// ---- merging curves into faces ------------------------------------------------------------

/// What [`merge_curves_on_surface`] needs of the caller's classification handles, on
/// top of [`CurveOnClassifyObjects`] (which includes [`ClassGeometry`]) and
/// [`MergeIntervalBrep`]: the adapter holds the B-rep the curves merge into, which
/// upstream's `MergeIntervals` edits through the classification's topology pointers.
pub trait MergeCurvesObjects<O: ClassObject>:
    CurveOnClassifyBrep<O> + MergeIntervalBrep<O>
{
    /// The edge behind an edge handle (an interval's `m_vMid.GetObject()`).
    fn edge_of(&self, object: O) -> Option<EdgeId>;
    /// The face behind a face handle (a new face of `MergeIntervals`).
    fn face_of(&self, object: O) -> Option<FaceId>;
}

/// The options of [`merge_curves_on_surface`].
#[derive(Debug, Clone, Copy)]
pub struct MergeCurvesOptions<O> {
    /// `bUseProjection` (default `FALSE`): project for intersections to allow for
    /// tolerances.
    pub use_projection: bool,
    /// `pOptIntersectionSurface` (default `NULL`): the other surface when the curves
    /// come from surface/surface intersections.
    pub intersection_surface: Option<O>,
    /// `bUVSpaceOkay` (default `TRUE`): try UV space classification in dodgy tolerant
    /// 3D cases.
    pub uv_space_okay: bool,
}

impl<O> Default for MergeCurvesOptions<O> {
    fn default() -> Self {
        Self {
            use_projection: false,
            intersection_surface: None,
            uv_space_okay: true,
        }
    }
}

/// What [`merge_curves_on_surface`] made.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MergedCurves {
    /// `rNewFaces`: the faces split off (not the original face).
    pub new_faces: Vec<FaceId>,
    /// `rEdges`: the edges the curves classify to, new or existing, in curve order.
    pub edges: Vec<EdgeId>,
}

/// `MergeCurvesOnSurface`: merges the 3D `curves` (with optional UV curves
/// `uv_curves`, one entry per curve, `None` for upstream's null entry; used on the first
/// face only) lying on the surface of `face` into the
/// face, splitting it (and the faces split off it) into new faces. Curves shorter than
/// `tolerance` are skipped. Each curve is classified against each face
/// in place ([`curve_on_classify_mut`]), coincident-maybe intervals are squeezed, the
/// intervals are merged into the B-rep `objects` holds ([`crate::query::face::p2::FaceBrep::brep`], edited through
/// [`MergeIntervalBrep`]; a failed merge is ignored, as upstream \[B674\]) and the edges
/// the curve classifies to are collected. The context's `DoingBoolean` flag is set for
/// the duration.
///
/// # Errors
///
/// `SM_ERR` when editing is not enabled or a coincident-maybe test is `UNSURE`;
/// errors of the classification; [`SolidError::InvalidInput`] for a curve handle that
/// does not resolve or a missing UV curve; those of `ConnectIntervals` and
/// `MergeIntervals`; [`SolidError::Unported`] from the curve classification constructor
/// (its `ReSet`, `intersect.curve_class_p2`), from `AreCoincidentMaybe`'s edge branch,
/// and from the graph edits `MergeIntervals` defers.
#[allow(clippy::too_many_arguments)]
pub fn merge_curves_on_surface<O: ClassObject + Copy>(
    k: &mut Kernel,
    objects: &mut impl MergeCurvesObjects<O>,
    face: FaceId,
    tolerance: f64,
    curves: &[O],
    uv_curves: Option<&[Option<O>]>,
    options: MergeCurvesOptions<O>,
) -> Result<MergedCurves> {
    // check state - m_bEditingEnabled must be on
    if !objects.brep().editing_enabled() {
        return Err(SolidError::sm_err(OP));
    }

    let saved = k.context.doing_boolean();
    k.context.set_doing_boolean(true);
    let res = merge_curves_body(k, objects, face, tolerance, curves, uv_curves, options);
    k.context.set_doing_boolean(saved);
    res
}

#[allow(clippy::too_many_arguments)]
fn merge_curves_body<O: ClassObject + Copy>(
    k: &mut Kernel,
    objects: &mut impl MergeCurvesObjects<O>,
    face: FaceId,
    tolerance: f64,
    curves: &[O],
    uv_curves: Option<&[Option<O>]>,
    options: MergeCurvesOptions<O>,
) -> Result<MergedCurves> {
    let mut out = MergedCurves::default();
    let surface = objects.face_surface(face);

    // init queue of Faces to be merged with curves - later, gets children faces when
    // pFace is split
    let mut faces_to_merge = vec![face];

    // iter every input curve - check to see if the curves need to be split
    for (ii, &curve_h) in curves.iter().enumerate() {
        let curve = ClassGeometry::curve(&*objects, curve_h)
            .ok_or(null_deref("unresolved curve handle"))?;

        // skip very short curves (gwc:this is tolerance bug waiting to happen)
        let natural = curve.natural_interval();
        if curve.approximate_length(k, &natural, 10, false)?.0 < tolerance {
            continue;
        }

        // for every face that could be split by curves
        let mut jj = 0;
        while jj < faces_to_merge.len() {
            let f = *faces_to_merge.get(jj).ok_or(null_deref("face queue"))?;

            // on first face iter - get UVCurve if given
            let uv_curve = match (uv_curves, jj) {
                (Some(uv), 0) => *uv.get(ii).ok_or(null_deref("missing UV curve"))?,
                _ => None,
            };

            // classify the curve against the surface->Face
            let mut cc = new_curve_classification(k, curve_h, &natural, uv_curve, tolerance)?;

            // for SSS_XSect refinement
            cc.set_intersect_data(surface, options.intersection_surface, None, true, None);

            // classify the curve against the surface->Face
            let _modified = curve_on_classify_mut(
                k,
                f,
                objects,
                true,
                &mut cc,
                options.use_projection,
                false,
                options.uv_space_okay,
            )?;

            // First remove coincident maybe segments from every classification segment
            let mut kk = 0;
            while kk < cc.size() {
                // squeeze coincident maybe intervals
                let (coin, pos) = cc.are_coincident_maybe(k, &*objects, &cc, kk, false)?;
                if coin == CoincidentMaybe::Unsure {
                    // This signals an unrecoverable error.
                    return Err(SolidError::sm_err(OP));
                }
                if coin == CoincidentMaybe::Fixable {
                    connect_intervals(&mut cc, kk, pos)?;
                }
                kk += 1;
            }

            // merge classified intersection curve segments into this Brep
            // Don't SER on failure for one curve. Merge the rest. [B674]
            // (the new faces made before a failure are kept, as upstream's list)
            let (new_faces, r) = merge_intervals(k, objects, &mut cc)?;
            if let Err(e) = r {
                if e.is_unported() {
                    return Err(e);
                }
            }

            // save new faces and intersection segments that classify to edges in output
            // arrays
            out.new_faces.extend_from_slice(&new_faces);
            let at = jj + 1;
            faces_to_merge.splice(at..at, new_faces.iter().copied());
            jj += new_faces.len();

            // place all edges classifying to input curve into rEdges output array
            for ivl in cc.intervals() {
                if ivl.mid.point_class() == PointClassType::Edge {
                    let e = ivl
                        .mid
                        .object()
                        .and_then(|o| objects.edge_of(o))
                        .ok_or(null_deref("edge interval without edge"))?;
                    out.edges.push(e);
                }
            }
            jj += 1;
        }
    }

    // all done
    Ok(out)
}

/// `SmCurveClassification(pCurve, pCurve->GetNaturalInterval(), pUVCurve, d3DTolerance,
/// 20, aData, FALSE, TRUE)` ([`CurveClassification::with_curve`]; `aData` is not
/// modelled, see `intersect.curve_class_p1`).
fn new_curve_classification<O: ClassObject + Copy>(
    k: &Kernel,
    curve: O,
    interval: &Extent1d,
    uv_curve: Option<O>,
    tolerance: f64,
) -> Result<CurveClassification<O>> {
    CurveClassification::with_curve(
        k,
        &ClassificationInit {
            curve: Some(curve),
            interval: *interval,
            uv_curve,
            // the SmZoneTol3d argument from the plain double
            src_zone_tol_3d: zone_tol3d(k, tolerance),
            opt_data_size: 20,
            is_curve_owned: false,
            is_curve_current: true,
            surface1: None,
            surface2: None,
            uv_curve2: None,
            ti: None,
        },
    )
}

/// `SER(ConnectIntervals(index, ePos, FALSE))` (`SmBrep.cpp:22072`;
/// [`CurveClassification::connect_intervals`]).
fn connect_intervals<O: ClassObject>(
    cc: &mut CurveClassification<O>,
    index: usize,
    pos: IntervalPosition,
) -> Result<()> {
    cc.connect_intervals(index, pos, false)
}

/// `MergeIntervals(&newFaces, NULL, NULL, pFace)` (unit `intersect.curve_class_p5`,
/// `bVerticesOnly` default `FALSE`; the debug-only face argument is not passed): the new
/// faces, also those made before a failure (upstream's list is filled as faces are
/// made), with its status.
fn merge_intervals<O: ClassObject + Copy>(
    k: &mut Kernel,
    objects: &mut impl MergeCurvesObjects<O>,
    cc: &mut CurveClassification<O>,
) -> Result<(Vec<FaceId>, Result<()>)> {
    let (merged, r) = match cc.merge_intervals_mut_with_partial(k, objects, false) {
        Ok(m) => (m, Ok(())),
        Err(e) => (*e.merged, Err(e.error)),
    };
    let faces = merged
        .faces
        .into_iter()
        .map(|o| {
            objects
                .face_of(o)
                .ok_or(null_deref("new face handle without face"))
        })
        .collect::<Result<_>>()?;
    Ok((faces, r))
}

// ---- mating a lamina edge -----------------------------------------------------------------

/// The B-spline of a curve (`SM_CAST_PTR(SmBSplineCurve, ..)`, `NER` on failure).
fn into_bspline(c: Curve) -> Result<BSplineCurve> {
    match c.into_kind() {
        CurveKind::BSpline(b) => Ok(b),
        _ => Err(SolidError::null_pointer(OP)),
    }
}

/// `CreateExtendedCurve(.., SM_CT_G1, .., TRUE)` then `RemoveExtraKnots(SM_EFF_ZERO_SQRT
/// / 100)`; upstream dereferences the null curve of an extension inside the interval.
fn extended_uv_curve(uv: &BSplineCurve, parameter: f64) -> Result<BSplineCurve> {
    let mut ext = uv
        .create_extended_curve(parameter, ContinuityType::G1, true)?
        .ok_or(null_deref("extension parameter inside the curve"))?;
    ext.remove_extra_knots(EFF_ZERO_SQRT / 100.0)?;
    Ok(ext)
}

/// The first (`first`) or last solution of `extended` intersected with `other`.
fn uv_curve_hit(
    k: &Kernel,
    extended: &BSplineCurve,
    other: &Curve,
    tol: f64,
    first: bool,
) -> Result<Option<Solution<ObjectHandle>>> {
    let ext = Curve::from(extended.clone());
    let sols = ext.global_curve_intersect::<ObjectHandle>(
        k,
        &extended.natural_interval(),
        other,
        &other.natural_interval(),
        tol,
        &CurveIntersectOptions::default(),
    )?;
    let n = sols.len();
    if n == 0 {
        return Ok(None);
    }
    Ok(sols.get(if first { 0 } else { n - 1 }).copied())
}

/// `SetCurve(curve, TRUE)` and `curve->SetOwner(edge)`.
fn give_edge_curve(
    b: &mut Brep,
    env: &mut NotifyEnv<'_>,
    e: EdgeId,
    mut curve: Curve,
) -> Result<()> {
    let this = (env.key_of)(EntityRef::Edge(e));
    // the owner change is delivered by SmObject::Notify when the curve has an
    // identity; a new curve has none here
    let _ = curve.set_owner(Some(this), Some(brep_ref(b)));
    let opts = SetCurveOptions {
        delete_old_curve: true,
        ..SetCurveOptions::default()
    };
    let _ = edge::set_curve(b, e, Some(curve), opts, env, [None, None])?;
    Ok(())
}

/// `MateLaminaEdgeToSurface`: moves lamina edge `lamina` (whose neighbours in its loop
/// are lamina or seam edges) onto `mating`: the face's surface is extended by
/// `surface_extension_factor` (upstream default 1.5) times the largest distance of 21
/// edge points from the (extended, for a plane) mating surface, the two neighbours'
/// UV trim curves are extended, and the intersection curve meeting both becomes the
/// edge's curve, with the neighbours trimmed or lifted from their extended UV curves.
/// The face takes the extended surface when the new UV curves leave its surface's
/// domain. Nothing happens when the edge is already on the surface (distance below
/// `SM_EFF_ZERO`) or a trim of an extended neighbour curve fails.
/// `uv_curve_extension_factor` is unused upstream.
///
/// # Errors
///
/// `SM_ERR` when editing is disabled, the edge or a neighbour is not lamina (nor seam),
/// or no intersection curve meets both extended curves; errors of the geometry calls;
/// [`SolidError::InvalidInput`] where upstream dereferences null;
/// [`SolidError::Unported`] from the callees of `SmFace::Notify` ([`face3::notify`]);
/// those of `SmEdgeuse::GetOrCreateUVTrimCurve` / `NormalizedEvaluate`
/// ([`crate::topology::edgeuse::p3`]) and of [`replace_surface`].
#[allow(clippy::too_many_lines)]
pub fn mate_lamina_edge_to_surface(
    k: &Kernel,
    b: &mut Brep,
    env: &mut NotifyEnv<'_>,
    lamina: EdgeId,
    mating: &Surface,
    surface_extension_factor: f64,
    uv_curve_extension_factor: f64,
) -> Result<()> {
    let _ = uv_curve_extension_factor;
    // check state - m_bEditingEnabled must be on
    if !b.editing_enabled() {
        return Err(SolidError::sm_err(OP));
    }

    // no work - not lamina edge
    if !eu1::edge_is_lamina(b, lamina)? {
        return Err(SolidError::sm_err(OP));
    }

    let eu = eu1::primary_edgeuse(b, lamina)?.ok_or(null_deref("edge without edge use"))?;
    let face = eu1::face(b, eu)?.ok_or(SolidError::null_pointer(OP))?;
    let face_ref = (env.key_of)(EntityRef::Face(face));
    face_notify(b, env, face, face_ref, NotifyOperation::PreEdit)?;

    // no work - CW and CCW neighbors are not lamina or Seam edges
    if b.face(face)?.surface.is_none() {
        return Err(null_deref("face without surface"));
    }
    let eu_ccw = b
        .edgeuse(eu)?
        .ccw
        .ok_or(null_deref("edge use without CCW"))?;
    let e_ccw = b
        .edgeuse(eu_ccw)?
        .edge
        .ok_or(null_deref("edge use without edge"))?;
    if !eu1::edge_is_lamina(b, e_ccw)? && !edge::is_seam_of_surface(k, b, e_ccw, face)? {
        return Err(SolidError::sm_err(OP));
    }
    let eu_cw = b.edgeuse(eu)?.cw.ok_or(null_deref("edge use without CW"))?;
    let e_cw = b
        .edgeuse(eu_cw)?
        .edge
        .ok_or(null_deref("edge use without edge"))?;
    if !eu1::edge_is_lamina(b, e_cw)? && !edge::is_seam_of_surface(k, b, e_cw, face)? {
        return Err(SolidError::sm_err(OP));
    }

    // Create an extended mating surface if it is a plane
    let mut ext_mating = mating.clone();
    if let SurfaceKind::Plane(p) = ext_mating.kind_mut() {
        let mut domain = p.step_uv_domain();
        domain.expand_absolute(domain.size().length() * surface_extension_factor);
        // status ignored
        if let Err(e) = p.adjust_step_uv_domain(&domain) {
            if e.is_unported() {
                return Err(e);
            }
        }
    }
    let mating_domain = ext_mating.natural_uv_domain_with_nurb(k)?;

    // The first thing to do is to create an extended surface based on the distance the
    // edge is from the surface. Take 20 points to get an approximate distance
    let ivl = b.edge(lamina)?.interval;
    let mut max_dist = 0.0;
    {
        let e_curve = b
            .edge(lamina)?
            .curve
            .as_ref()
            .ok_or(null_deref("edge without curve"))?;
        for j in 0..=20u32 {
            let edge_pnt = e_curve.evaluate_point(k, ivl.evaluate(f64::from(j) / 20.0))?;
            // (only the distance is read: no handle for the surface)
            let sols = ext_mating.global_point_solve::<ObjectHandle>(
                k,
                None,
                &mating_domain,
                SolverOperation::Minimize,
                edge_pnt,
                EFF_ZERO_SQRT,
                None,
                SolutionRequested::All,
            )?;
            let s0 = sols
                .get(0)
                .ok_or(null_deref("point solve without solution"))?;
            if s0.start.solution_value > max_dist {
                max_dist = s0.start.solution_value;
            }
        }
    }

    // If the dMaxDist is 0 then we don't have to do any work.
    if max_dist < EFF_ZERO {
        return Ok(());
    }

    // Extend surface by dMaxDist*dSurfaceExtensionFactor
    let orig_domain;
    let mut ext_surface = {
        let surf = b
            .face(face)?
            .surface
            .as_ref()
            .ok_or(null_deref("face without surface"))?;
        orig_domain = surf.natural_uv_domain_with_nurb(k)?;
        let ext_dist = max_dist * surface_extension_factor;
        if ext_dist < 0.0 {
            // upstream: N_SrfExtendByDist never returns for a negative distance (HANG-18)
            return Err(SolidError::invalid_input(
                OP,
                "negative surface extension distance",
            ));
        }
        surf.create_extended_surface(k, ext_dist, ContinuityType::G1R)?
    };
    let ext_domain = ext_surface.natural_uv_domain_with_nurb(k)?;

    // Create extended UV curves (temp copies of the UV trim curves)
    let uv_ccw = uv_trim_curve_copy(k, b, eu_ccw)?;
    let uv_cw = uv_trim_curve_copy(k, b, eu_cw)?;

    let curve_extension_factor = 10.0; // Curve Extension Factor
    let ccw_ivl = uv_ccw.natural_interval();
    let ccw_orient = b.edgeuse(eu_ccw)?.orientation;
    let mut ext_param = ccw_ivl.min() - curve_extension_factor * ccw_ivl.length();
    if ccw_orient == OrientType::Opposite {
        ext_param = ccw_ivl.max() + curve_extension_factor * ccw_ivl.length();
    }
    let mut ccw_ext = extended_uv_curve(&uv_ccw, ext_param)?;

    let cw_ivl = uv_cw.natural_interval();
    let cw_orient = b.edgeuse(eu_cw)?.orientation;
    ext_param = cw_ivl.max() + curve_extension_factor * cw_ivl.length();
    if cw_orient == OrientType::Opposite {
        ext_param = cw_ivl.min() - curve_extension_factor * cw_ivl.length();
    }
    let mut cw_ext = extended_uv_curve(&uv_cw, ext_param)?;

    // Now we have two extended curves - intersect the surfaces
    let deg20_rad = 20.0 * PI / 360.0;
    let ssi_tol = (b.tolerance() / 4.0).abs();
    let mut inter = ext_surface.global_surface_intersect(
        k,
        &ext_domain,
        &ext_mating,
        &mating_domain,
        [true, true],
        Some(ssi_tol),
        Some(deg20_rad),
    )?;

    // Now see if we have an intersection curve that has two intersections with the
    // extended parameter space curves.
    let mut found = None;
    for i in 0..inter.surface1_uv_curves.len() {
        let int_crv = inter
            .surface1_uv_curves
            .get(i)
            .and_then(Option::as_ref)
            .ok_or(null_deref("null intersection UV curve"))?;
        let Some(sol_ccw) = uv_curve_hit(
            k,
            &ccw_ext,
            int_crv,
            ssi_tol,
            ccw_orient == OrientType::Same,
        )?
        else {
            continue;
        };
        let Some(sol_cw) = uv_curve_hit(
            k,
            &cw_ext,
            int_crv,
            ssi_tol,
            cw_orient == OrientType::Opposite,
        )?
        else {
            continue;
        };
        if i >= inter.curves_3d.len() {
            return Err(null_deref("intersection without 3D curve"));
        }
        let c3d = inter.curves_3d.remove(i);
        let uv = inter
            .surface1_uv_curves
            .remove(i)
            .ok_or(null_deref("null intersection UV curve"))?;
        found = Some((sol_ccw, sol_cw, c3d, into_bspline(uv)?));
        break;
    }

    // If we did not find the solutions we are done with an error
    let Some((sol_ccw, sol_cw, mut found_3d, mut found_uv)) = found else {
        return Err(SolidError::sm_err(OP));
    };

    // Here we found solutions to things - do some trimming and to get the proper
    // length curves.
    let ccw_trim = sol_ccw.start.param(0)?;
    let mut ccw_trim_ivl = ccw_ext.natural_interval();
    let _ = if ccw_orient == OrientType::Same {
        ccw_trim_ivl.set_min_max(ccw_trim, ccw_trim_ivl.max())
    } else {
        ccw_trim_ivl.set_min_max(ccw_trim_ivl.min(), ccw_trim)
    };
    // may snap sIvl by tol to existing knots
    match ccw_ext.trim(k, &mut ccw_trim_ivl, TrimMode::Notify) {
        Ok(()) => {}
        Err(e) if e.is_unported() => return Err(e),
        Err(_) => return Ok(()),
    }

    let cw_trim = sol_cw.start.param(0)?;
    let mut cw_trim_ivl = cw_ext.natural_interval();
    let _ = if cw_orient == OrientType::Same {
        cw_trim_ivl.set_min_max(cw_trim_ivl.min(), cw_trim)
    } else {
        cw_trim_ivl.set_min_max(cw_trim, cw_trim_ivl.max())
    };
    match cw_ext.trim(k, &mut cw_trim_ivl, TrimMode::Notify) {
        Ok(()) => {}
        Err(e) if e.is_unported() => return Err(e),
        Err(_) => return Ok(()),
    }

    let mut crv_trim_ivl = Extent1d::from_value(sol_ccw.start.param(1)?);
    crv_trim_ivl.add_value(sol_cw.start.param(1)?);

    let new_orient = if sol_ccw.start.param(1)? < sol_cw.start.param(1)? {
        OrientType::Opposite
    } else {
        OrientType::Same
    };

    // may snap sIvl by tol to existing knots
    found_3d.trim(k, &mut crv_trim_ivl, TrimMode::Notify)?;
    found_uv.trim(k, &mut crv_trim_ivl, TrimMode::Notify)?;

    // Compute a UV bounding box for the new UV curves.
    let req = BoundingBoxRequest {
        normal: true,
        pseudo: false,
        polar: false,
    };
    let bbox_of = |c: &BSplineCurve, ivl: &Extent1d| -> Result<Extent3d> {
        c.calculate_bounding_box(ivl, req)?
            .normal
            .ok_or(null_deref("missing box"))
    };
    let mut uv_bbox = Extent3d::new();
    uv_bbox = uv_bbox.union(&bbox_of(&cw_ext, &cw_trim_ivl)?);
    uv_bbox = uv_bbox.union(&bbox_of(&ccw_ext, &ccw_trim_ivl)?);
    uv_bbox = uv_bbox.union(&bbox_of(&found_uv, &crv_trim_ivl)?);

    // Determine if we can use original surface or need to replace it with the
    // extended one.
    let mut uv_domain = Extent2d::from_extent3d(&uv_bbox);
    let replaced = if uv_domain.is_contained_by(&orig_domain, EFF_ZERO) {
        if !uv_domain.is_contained_by(&b.face(face)?.uv_domain, EFF_ZERO) {
            face2::set_uv_domain(b, face, &orig_domain)?;
        }
        Some(ext_surface)
    } else {
        // Need to replace it.
        uv_domain = uv_domain.union(&orig_domain);
        ext_surface.trim_with_domain(k, &mut uv_domain)?;

        // replace pFace's surface (deletes all pFace UVTrimCurves)
        let _ = replace_surface(
            k,
            b,
            env,
            face,
            ext_surface,
            face_ref,
            OldSurface::Delete,
            true,
        )?;
        None
    };

    // Replace Edge->Curve and set its new domain
    // side effect: delete current pLaminaEdgeToModify->UVTrimCurves
    give_edge_curve(b, env, lamina, found_3d)?;
    edge::set_interval(b, lamina, &crv_trim_ivl)?;
    let prim = eu1::primary_edgeuse(b, lamina)?.ok_or(null_deref("edge without edge use"))?;
    let tol = b.edge(lamina)?.tolerance;
    let _ = eu1::set_uv_trim_curve(
        b,
        prim,
        UvTrimCurveArg::New(Box::new(Curve::from(found_uv))),
        tol,
        true,
        None,
    )?;
    edge::set_orientation(b, lamina, new_orient)?;

    // Update the geometry of the Vertices.
    let vu = b
        .edgeuse(prim)?
        .vertexuse
        .ok_or(null_deref("edge use without vertex use"))?;
    let v = b
        .vertexuse(vu)?
        .vertex
        .ok_or(null_deref("vertex use without vertex"))?;
    let pnt = normalized_evaluate_3d(k, b, prim, 0.0)?;
    set_vertex_point(b, env, v, pnt)?;
    let mate = eu1::mate(b, prim)?.ok_or(null_deref("edge use without mate"))?;
    let vu = b
        .edgeuse(mate)?
        .vertexuse
        .ok_or(null_deref("edge use without vertex use"))?;
    let v = b
        .vertexuse(vu)?
        .vertex
        .ok_or(null_deref("vertex use without vertex"))?;
    let pnt = normalized_evaluate_3d(k, b, prim, 1.0)?;
    set_vertex_point(b, env, v, pnt)?;

    // Now see what we need to do with the side curves.
    for (eu_side, side_ivl, side_trim_ivl, side_ext) in [
        (eu_ccw, ccw_ivl, ccw_trim_ivl, ccw_ext),
        (eu_cw, cw_ivl, cw_trim_ivl, cw_ext),
    ] {
        let e_side = b
            .edgeuse(eu_side)?
            .edge
            .ok_or(null_deref("edge use without edge"))?;
        if side_ivl.contains_value(side_trim_ivl.min(), 0.0)
            && side_ivl.contains_value(side_trim_ivl.max(), 0.0)
        {
            // Use existing curves but trim them back.
            edge::set_interval(b, e_side, &side_trim_ivl)?;
            trim_uv_trim_curve(k, b, eu_side, &side_trim_ivl)?;
        } else {
            let tol = b.edge(e_side)?.tolerance / 4.0;
            let face_domain = b.face(face)?.uv_domain;
            let side_curve = Curve::from(side_ext);
            let lifted = if let Some(surface) = replaced.as_ref() {
                surface.lift_curve(
                    k,
                    &face_domain,
                    &side_curve,
                    &side_curve.natural_interval(),
                    tol,
                    true,
                )?
            } else {
                // the face's surface reaches its face's cache (smsurf_GetSurfaceCache),
                // made on the B-rep: an exact evaluation copy of the surface meanwhile
                let surface = b
                    .face(face)?
                    .surface
                    .as_ref()
                    .ok_or(null_deref("face without surface"))?
                    .clone_for_evaluation()?;
                surface.lift_curve_with_context(
                    k,
                    &face_domain,
                    &side_curve,
                    &side_curve.natural_interval(),
                    tol,
                    true,
                    &mut crate::query::brep::BrepFaceServices::new(b),
                    Some(CacheObject::Surface(face)),
                )?
            };
            let new_3d = lifted.curve;
            let new_ivl = new_3d.natural_interval();
            // side effect: delete current UVTrimCurves
            give_edge_curve(b, env, e_side, new_3d)?;
            edge::set_interval(b, e_side, &new_ivl)?;
            let _ = eu1::set_uv_trim_curve(
                b,
                eu_side,
                UvTrimCurveArg::New(Box::new(side_curve)),
                lifted.max_approx_dev,
                true,
                None,
            )?;
        }
    }
    Ok(())
}

/// `SmVertex::SetPoint` through the notification environment.
fn set_vertex_point(b: &mut Brep, env: &mut NotifyEnv<'_>, v: VertexId, p: Point3) -> Result<()> {
    let this = (env.key_of)(EntityRef::Vertex(v));
    let key_of = env.key_of;
    let vu_ref = |vu: VertexuseId| key_of(EntityRef::Vertexuse(vu));
    vertex::set_point(
        b,
        v,
        p,
        &mut *env.records,
        &mut *env.attributes,
        env.context.as_deref_mut(),
        this,
        &vu_ref,
    )
}

/// `pEU->GetOrCreateUVTrimCurve(pUV)` (status ignored; header defaults `NULL`, `NULL`,
/// `TRUE`), then `SER(pUV->Copy(..))`: a copy of the edge use's UV trim curve. A
/// missing curve is dereferenced upstream (after its `SM_ASSERT`). UV trim curves are
/// made as B-splines (the `SmBSplineCurve*&` overload casts). An `SmLine` is an
/// `SmBSplineCurve`: its copy serves `CreateExtendedCurve` through its B-spline part,
/// whose NURB `GetOrCreateGwNurbPointer` makes (`MakeNurb`, the `SmLine` override; status
/// ignored) when missing. A curve of another kind is [`SolidError::InvalidInput`].
fn uv_trim_curve_copy(k: &Kernel, b: &mut Brep, eu: EdgeuseId) -> Result<BSplineCurve> {
    let _ = eu3::get_or_create_uv_trim_curve(k, b, eu, false, false, true)?;
    let curve = eu1::uv_trim_curve(b, eu)?
        .0
        .ok_or(null_deref("edge use without UV trim curve"))?;
    match curve.kind() {
        CurveKind::BSpline(c) => Ok(c.clone()),
        CurveKind::Line(l) => {
            let mut l = l.clone();
            if l.base.gw_nurb().is_none() {
                let _ = l.make_nurb();
            }
            Ok(l.base)
        }
        _ => Err(SolidError::InvalidInput {
            operation: OP,
            reason: "UV trim curve is not a B-spline",
        }),
    }
}

/// `SER(pEU->GetOrCreateUVTrimCurve(pUV))` then `pUV->Trim(ivl)` (`bNotify` default
/// `TRUE`; status ignored, only [`SolidError::Unported`] propagates). The curve is the
/// one of the edge use or its mate (the owner of the pair's curve); a missing curve is
/// dereferenced upstream.
fn trim_uv_trim_curve(k: &Kernel, b: &mut Brep, eu: EdgeuseId, ivl: &Extent1d) -> Result<()> {
    eu3::get_or_create_uv_trim_curve(k, b, eu, false, false, true)?.status?;
    let (Some(_), Some(owner)) = eu1::uv_trim_curve(b, eu)? else {
        return Err(null_deref("edge use without UV trim curve"));
    };
    let curve = b
        .edgeuse_mut(owner)?
        .uv_trim_curve
        .as_mut()
        .ok_or(null_deref("edge use without UV trim curve"))?;
    // may snap sIvl by tol to existing knots
    let mut trim_ivl = *ivl;
    match curve.trim(k, &mut trim_ivl, TrimMode::Notify) {
        Err(e) if e.is_unported() => Err(e),
        _ => Ok(()),
    }
}

/// `SER(pEU->NormalizedEvaluate(t, FALSE, pnt))` (3D; header defaults: no derivative,
/// `bOKToMakeUVTrimCurve = TRUE`).
fn normalized_evaluate_3d(k: &Kernel, b: &mut Brep, eu: EdgeuseId, t: f64) -> Result<Point3> {
    Ok(eu3::normalized_evaluate(k, b, eu, t, false, false, true)?.point)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surface::plane::Plane;
    use crate::topology::Face;

    fn line(a: Point3, b: Point3) -> Curve {
        Curve::from(BSplineCurve::create_line_segment(3, a, b).unwrap())
    }

    /// A B-rep with one vertex, one edge (a line from the origin to (2, 0, 0)) and one
    /// face on the plane z = 1.
    fn brep() -> (Brep, VertexId, EdgeId, FaceId) {
        let mut b = Brep::new();
        let v = b.add_vertex(Point3::new(1.0, 2.0, 3.0)).unwrap();
        let e = b.add_edge(v, v).unwrap();
        b.edge_mut(e).unwrap().curve =
            Some(line(Point3::new(0.0, 0.0, 0.0), Point3::new(2.0, 0.0, 0.0)));
        let s = b.add_shell().unwrap();
        let f = b.add_face_record(Face::new(s)).unwrap();
        b.face_mut(f).unwrap().surface = Some(Surface::from(Plane::from_normal(
            Point3::new(0.0, 0.0, 1.0),
            Vec3::new(0.0, 0.0, 1.0),
        )));
        (b, v, e, f)
    }

    /// A region key that is no region of the notifying B-rep stays unresolved
    /// (the documented gap of [`BrepRegions`]); a region of the B-rep resolves.
    #[test]
    fn brep_regions_lookup_defers_foreign_key() {
        use crate::topology::loopuse::notify_ref;
        use shell::RegionLookup;
        let (b, _, _, _) = brep();
        let key_of = |e: EntityRef| notify_ref(e);
        let lookup = BrepRegions {
            b: &b,
            key_of: &key_of,
        };
        for (id, _) in b.regions().iter() {
            let k = notify_ref(EntityRef::Region(id)).key;
            assert!(lookup.resolve_region(k).is_ok());
        }
        let foreign = notify_ref(EntityRef::Region(RegionId::from_index(99))).key;
        assert!(matches!(
            lookup.resolve_region(foreign),
            Err(SolidError::Unported {
                unit: "topology.shell",
                ..
            })
        ));
    }

    /// The region keys of a [`RegionNotifyState`] are arena identities: the heal path's
    /// tracker (`entity_of_notify_key`, the inverse of `notify_ref`) decodes a split's
    /// regions back to themselves for every index (a bare index decoded region `r >= 1`
    /// as `Brep(r - 1)`), and rejects the record-less key of any other entity.
    #[test]
    fn region_only_keys_decode_as_the_tracker_reads_them() {
        use crate::topology::loopuse::{entity_of_notify_key, notify_ref};
        use crate::topology::RegionId;
        for i in [0, 1, 2, 7, u32::MAX] {
            let r = EntityRef::Region(RegionId::from_index(i));
            let key = region_only_key(r);
            assert_eq!(key, notify_ref(r));
            assert_eq!(entity_of_notify_key(key.key), Some(r));
        }
        let other = region_only_key(EntityRef::Face(FaceId::from_index(1)));
        assert_eq!(entity_of_notify_key(other.key), None);
        let mut b = Brep::new();
        let r0 = b
            .add_region_record(crate::topology::Region::default())
            .unwrap();
        let r1 = b
            .add_region_record(crate::topology::Region::default())
            .unwrap();
        let state = RegionNotifyState::new(&b);
        for r in [r0, r1] {
            let key = region_only_key(EntityRef::Region(r)).key;
            assert!(state.records.contains_key(&key));
            assert_eq!(entity_of_notify_key(key), Some(EntityRef::Region(r)));
        }
    }

    /// `MakeEdgeTopology` between two shell vertices deletes the end vertex shell
    /// (`delete pEndShell`): the region keeps one wire shell and the arena no orphan.
    #[test]
    fn edge_between_shell_vertices_deletes_the_merged_shell() {
        let mut k = Kernel::default();
        let mut b = Brep::new_with_infinite_region().unwrap();
        let (_, v1) = make_shell_vertex_topology(&mut b, None).unwrap();
        let (end_shell, v2) = make_shell_vertex_topology(&mut b, None).unwrap();
        assert_eq!(b.shells().len(), 2);
        let mut state = ArenaNotifyState::new(&b);
        let mut env = state.env(None);
        make_edge_topology(&mut k, &mut b, None, v1, v2, &mut env).unwrap();
        assert_eq!(b.shells().len(), 1);
        assert!(b.shell(end_shell).is_err());
        let region = b.infinite_region().unwrap();
        assert_eq!(b.region(region).unwrap().shells.len(), 1);
    }

    fn solution(objects: &[CacheObject], params: &[f64]) -> Solution<CacheObject> {
        let mut s = Solution::new();
        for (i, o) in objects.iter().enumerate() {
            s.objects[i] = Some(*o);
        }
        s.num_objects = objects.len();
        for (i, p) in params.iter().enumerate() {
            s.start.parameters[i] = *p;
        }
        s
    }

    #[test]
    fn point_from_solution_walks_the_parameters() {
        let (b, v, e, f) = brep();
        // edge (1 parameter), face (2), vertex (0), curve (1)
        let objs = [
            CacheObject::Edge(e),
            CacheObject::Face(f),
            CacheObject::Vertex(v),
            CacheObject::Curve(e),
        ];
        let s = solution(&objs, &[0.25, 0.5, 0.75, 1.0]);
        let edge = b.edge(e).unwrap().curve.as_ref().unwrap();
        assert_eq!(
            point_from_solution(&Kernel::default(), &b, &s, 0).unwrap(),
            edge.evaluate_point(&Kernel::default(), 0.25).unwrap()
        );
        let plane = b.face(f).unwrap().surface.as_ref().unwrap();
        assert_eq!(
            point_from_solution(&Kernel::default(), &b, &s, 1).unwrap(),
            plane
                .evaluate_point(&Kernel::default(), Point2::new(0.5, 0.75))
                .unwrap()
        );
        assert_eq!(
            point_from_solution(&Kernel::default(), &b, &s, 2).unwrap(),
            Point3::new(1.0, 2.0, 3.0)
        );
        assert_eq!(
            point_from_solution(&Kernel::default(), &b, &s, 3).unwrap(),
            edge.evaluate_point(&Kernel::default(), 1.0).unwrap()
        );
    }

    #[test]
    fn point_from_solution_odd_objects() {
        let (b, _, _, f) = brep();
        // a surface object is no curve, edge, vertex or face: SE(SM_ERR), the origin
        let s = solution(&[CacheObject::Surface(f)], &[]);
        assert_eq!(
            point_from_solution(&Kernel::default(), &b, &s, 0).unwrap(),
            Point3::new(0.0, 0.0, 0.0)
        );
        // a null object is dereferenced upstream
        let s = solution(&[], &[]);
        assert!(matches!(
            point_from_solution(&Kernel::default(), &b, &s, 0),
            Err(SolidError::InvalidInput { .. })
        ));
    }

    #[test]
    fn check_and_fix_without_edge_uses() {
        let (mut b, ..) = brep();
        let k = Kernel::default();
        check_and_fix_uv_trim_curves(&k, &mut b).unwrap();
    }

    #[test]
    fn notify_without_cache_only_propagates() {
        let (b, _, e, _) = brep();
        let mut records = crate::topology::topology::NotifyRecords::new();
        let mut attrs = AttributeStore::default();
        let key_of = |x: EntityRef| ObjectRef {
            type_code: 0,
            key: ObjectKey(match x {
                EntityRef::Edge(e) => u64::from(e.index()) + 1,
                _ => 0,
            }),
        };
        let mut env = NotifyEnv {
            records: &mut records,
            attributes: &mut attrs,
            context: None,
            key_of: &key_of,
        };
        for op in [
            NotifyOperation::AddToBrep,
            NotifyOperation::RmFromBrep,
            NotifyOperation::PreEdit,
            NotifyOperation::Destruction,
        ] {
            notify(
                &b,
                &mut env,
                None,
                op,
                [Some(NotifyArg::Entity(EntityRef::Edge(e))), None, None],
            )
            .unwrap();
        }
    }
}
