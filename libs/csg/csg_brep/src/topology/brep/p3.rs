//! `SmBrep` construction and copying, face and edge removal, wire edges, the
//! orientation of trimmed surfaces and the NURBS conversion of plan unit
//! `topology.brep_p3`.
//!
//! Upstream: `source/SMLib/src/SmBrep.cpp`, `source/SMLib/inc/SmBrep.h`,
//! `source/SM_API/src/SmApiQueries.cpp`.
//!
//! | Rust                            | Upstream                                         |
//! |---------------------------------|--------------------------------------------------|
//! | [`new_brep`]                    | `SmBrep::SmBrep(double, double, SmContext*)`     |
//! | `new_brep_for_caller` (crate)   | the same constructor under a reserved caller identity |
//! | [`copy_brep`]                   | `SmBrep::SmBrep(const SmBrep&, SmCopyBrepMap*, SmCopyBrepMap*, SmBoolean, SmBoolean)` |
//! | [`api_brep_copy`]               | `SmApiBrepCopy`                                  |
//! | [`copy_vertices`], [`copy_vertices_with_context`], [`CopySource`] | `SmBrep::CopyVertices`                           |
//! | [`delete_face`], [`DeleteFaceOptions`] | `SmBrep::DeleteFace`                      |
//! | [`remove_slit_loops_where_solids_touch`] | not upstream: the fork's `SmBrep::RemoveSlitLoopsWhereSolidsTouch` (issue #746) |
//! | [`split_disconnected_shells`]  | not upstream: the fork's `SmBrep::SplitDisconnectedShells` (issue #746) |
//! | [`make_wire_edge`]              | `SmBrep::MakeWireEdge`                           |
//! | [`make_wire_edge_vertex`]       | `SmBrep::MakeWireEdgeVertex`                     |
//! | [`merge_brep`], [`merge_brep_with_context`] | `SmBrep::MergeBrep`                              |
//! | [`orient_trimmed_surfaces`]     | `SmBrep::OrientTrimmedSurfaces`                  |
//! | [`squeeze_edge`]                | `SmBrep::SqueezeEdge`                            |
//! | [`turn_to_nurbs`]               | `SmBrep::TurnToNURBS`                            |
//! | [`topology_ref`], [`topology_entity`], [`notify_serial`] | (an object's address in the notifications, for callers without identities of their own) |
//! | [`edge_curve_identity`]         | (the `m_pCurve` object's address in the notifications) |
//! | [`face_surface_identity`]       | (the `m_pSurface` object's address in the notifications) |
//! | [`with_kernel_callbacks`]       | (for callers without notification state: the B-rep context's callbacks) |
//! | `Brep::remove_from_*_list` (crate) | `SmOwningTopology::Remove` on the B-rep's list heads (and `~SmTopology`) |
//!
//! Deviations: out-params become return values; `rpNewEdge`/`rpNewVertex` are only
//! returned on success. The context of a stack-allocated B-rep
//! (`pOptContextForStack`) is the kernel. Notifications go to the caller's
//! [`NotifyEnv`] (the B-rep spatial cache is not kept: no `BrepCacheSlot`).
//!
//! - **Records.** Upstream `delete` of a topology object unlinks it from its owning
//!   list (`~SmTopology`); records stay in their arenas, unreachable, as in
//!   `topology.brep_p2` (faces, edges and vertices leave the B-rep's lists, regions
//!   leave the region list). The copy constructors build a new [`Brep`] and return it;
//!   `MergeBrep` copies from another [`Brep`] (a self-merge, `MergeBrep(*this)`, cannot
//!   be expressed).
//! - **Ignored statuses.** The copy constructor ignores `MergeBrep`'s status (`SE`):
//!   only [`SolidError::Unported`] propagates. `MergeBrep` ignores the list inserts'
//!   statuses (`SE(PostInsert(..))`) as upstream.
//! - **Sliced curve copy.** `MakeWireEdge`/`MakeWireEdgeVertex` copy a curve that
//!   already belongs to an edge with `new SmCurve(*pCurve)`, a base-class copy without
//!   geometry; the port clones the curve with its geometry (`Curve`'s `Clone`, the
//!   copy constructor of the curve kinds). Recorded as `WR-168` in
//!   `plan/upstream-issues.md`; unreached today (the wire fixtures pass curves no
//!   edge owns).
//! - **Offset surfaces.** `TurnToNURBS` approximates an `SmOffsetSurface` of a B-spline
//!   kind base and extends it; upstream hands `CreateExtendedSurface` a placeholder
//!   `new SmSurface` that the call overwrites (and leaks), so the port has no
//!   placeholder.
//! - **Edge use marks.** `SqueezeEdge` marks the squeezed edge uses (and their radial
//!   partners, which are squeezed too); upstream never reads those marks (the check is
//!   commented out) and deletes the edge uses, so the port does not record them.
//! - **Mark locks.** `SmNewMarkAndLock` is released by its destructor, which cannot
//!   fail; the port releases it explicitly and reports a release failure (a mark
//!   bookkeeping fault) as an error. An `OrientTrimmedSurfaces` that fails returns no
//!   `rbMaybeNotClosedSolid` (upstream may have set it before the failure).
//! - **Use notifications.** As elsewhere in the crate, the destructors and
//!   constructors of uses, loops, shells and regions do not notify
//!   (`SM_NO_DESTRUCTION` / `SM_NO_CONSTRUCTION` at `SmObject` level; `new_brep`'s own
//!   construction is reported); faces, edges and vertices do.
//! - **B-rep-owned attributes.** Upstream's attributes live on the objects; the port's
//!   live in each [`Brep`]'s own store ([`Brep::add_attribute`]). An edit under the
//!   kernel's identities (`kernel_named_edit`, [`with_kernel_callbacks`]) lends `b`'s
//!   store to the edit under those identities and gives back the lists of `b`'s live
//!   objects afterwards; [`api_brep_copy`] lends the source's to the copy and the copy
//!   keeps what the `SM_NO_COPY` notifications give it (a reference attribute becomes
//!   one per B-rep). [`copy_brep`] and [`merge_brep`] use only the caller's `env`.
//! - **Notification identities.** Upstream names every object in a notification by
//!   its address. Topology keeps the [`NotifyEnv`] contract: `b`'s objects are named
//!   by the caller's `env.key_of`, the objects of a B-rep an edit only reads
//!   (`MergeBrep`, `CopyVertices`, the copy constructors) by the caller's `source_key`
//!   (the two B-reps' ids overlap), and each B-rep by its [`Brep::key`]. Keeping these
//!   distinct per object is the caller's part, as an upstream caller's objects are its
//!   own; the records and attributes it keeps under them are used as they are (an
//!   object without a record gets the default one its constructor would make,
//!   `seed_records`). Only objects with no caller key get kernel-issued identities:
//!   a curve is `(domain, counter)`, issued once per context domain and kept on the
//!   [`Curve`] ([`Curve::notify_key`]); a caller without notification state of its own
//!   ([`with_kernel_callbacks`], the healer's tracked edits) names topology by
//!   `(B-rep serial, kind, arena index)` ([`topology_ref`]), the serial issued once per
//!   B-rep and domain and kept on the [`Brep`] ([`notify_serial`]). Both are issued by
//!   the one context the notifications go through (`env.context`, else `k.context`),
//!   have no public setter, and a clone (upstream's copy constructor: another object)
//!   starts without one. Kernel keys have the top bit set (topology `0b10`, geometry
//!   `0b11`); every edit refuses (`InvalidInput`) a caller key in that range before it
//!   changes anything (`check_keys`), unless it is the object's own kernel identity.
//!   The lending adapters are not nestable: one inside another (whose kernel context
//!   then holds no callbacks) is refused before anything changes
//!   (`Context::lend_notify`). Independent contexts can issue equal keys by default.
//!   A shared attribute store must supply disjoint topology keys and coordinate its
//!   geometry sequences. `SweepStore` does this with store-owned allocation state;
//!   the identity domains and callbacks stay distinct. A lease that is not returned
//!   leaves its context a new domain; an object re-entering a domain is renamed.
//! - **Copied shell lists.** `MergeBrep`'s second pass copies `pS->m_pList` through the
//!   copy map and `m_lListSize` verbatim; the arena's size is recomputed from the
//!   members ([`shell::shell_list_size`]), and a face-use list's other members are the
//!   mapped members in order (upstream links them through the copied `m_pNext` /
//!   `m_pLast`), so every member must be mapped (`NER` otherwise). Every face use of
//!   the source is copied, so both agree on every reachable path.
//! - **No recursion.** Every loop is bounded by its input lists; `DeleteFace`'s shell
//!   split loop removes at least one vertex per pass.

use std::borrow::Cow;

use crate::curve::curve::{Curve, CurveKind};
use crate::error::{EntityRef, Result, SolidError, SM_ERR_INVALID_INPUT};
use crate::geom::attribute::ObjectKey;
use crate::geom::context::{
    is_reserved_key, Context, IdentitySlot, MarkLock, MarkType, ALL_MARKS, MAX_BREP_SERIAL,
    TOPOLOGY_KEY_BASE,
};
use crate::geom::extent1d::Extent1d;
use crate::geom::extent3d::Extent3d;
use crate::geom::geom_utility::p2::ContinuityType;
use crate::geom::object::{self, NotifyOperation, ObjectRef};
use crate::geom::tol::{self, SmallTopoType};
use crate::kernel::Kernel;
use crate::math::{Point3, Vec3};
use crate::nurbs::b_spline_curve::BSplineCurve;
use crate::nurbs::b_spline_surface::BSplineSurface;
use crate::nurbs::b_spline_surface_n_lib::p1::OffsetParamType;
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::surface::surface::p2::OFFSET_SURFACE_TYPE;
use crate::surface::surface::{Surface, SurfaceKind};
use crate::topology::brep::p1;
use crate::topology::brep::p2::{self, NotifyArg, OldSurface};
use crate::topology::brep_cache::{arena_identity, CacheObject};
use crate::topology::edge::{self, NotifyEnv, SetCurveOptions};
use crate::topology::edgeuse::p1 as eu1;
use crate::topology::edgeuse::p3 as eu3;
use crate::topology::face::p1 as face1;
use crate::topology::face::p2 as face2;
use crate::topology::loopuse as lu1;
use crate::topology::proto_topology::p1::CopyBrepMap;
use crate::topology::shell::{self, ShellMembers, ShellType};
use crate::topology::topology as topo;
use crate::topology::topology_traverser::{self as tt, Entity, FaceTraversalMarks, Marks};
use crate::topology::trimming_tools;
use crate::topology::{
    vertex, vertexuse, Brep, EdgeId, EdgeuseId, EdgeuseOwner, FaceId, FaceuseId, LoopuseId,
    LoopuseStart, Region, RegionId, ShellId, VertexId, Vertexuse, VertexuseId, VertexuseOwner,
};

const OP: &str = "topology::brep::p3";

/// Upstream `SM_ERR_ASSERT_FAILURE` (`SmMessages.h`).
const SM_ERR_ASSERT_FAILURE: i32 = 1005;

/// Upstream dereferences null here (no `NER`): the FFI guard rejects the input.
const fn null_deref(reason: &'static str) -> SolidError {
    SolidError::invalid_input(OP, reason)
}

/// `NER(ptr)` on a null pointer: upstream returns `SM_ERR_NULL_POINTER`.
const fn ner() -> SolidError {
    SolidError::null_pointer(OP)
}

/// `SER_MSG(SM_ERR, ..)` / `SER(SM_ERR)`.
const fn ser() -> SolidError {
    SolidError::sm_err(OP)
}

/// A call whose status upstream does not check (no `SER`, or a `void` `Notify`): only
/// [`SolidError::Unported`], the [`SolidError::InvalidInput`] of a null pointer
/// upstream dereferences and the [`SolidError::NotFound`] of a dangling id (a freed
/// pointer upstream) propagate.
fn ignore_soft<T>(r: Result<T>) -> Result<()> {
    match r {
        Err(
            e @ (SolidError::Unported { .. }
            | SolidError::InvalidInput { .. }
            | SolidError::PortInvariant { .. }
            | SolidError::NotFound { .. }),
        ) => Err(e),
        Ok(_) | Err(_) => Ok(()),
    }
}

// ---- the B-rep's own lists (SmOwningTopology::Remove on its list heads) ------------------

impl Brep {
    /// Removes `f` from the face list (`m_pFaceListHead->Remove`, also what deleting
    /// the face does through `~SmTopology`).
    pub(crate) fn remove_from_face_list(&mut self, f: FaceId) {
        self.face_list.retain(|&x| x != f);
    }

    /// Removes `e` from the edge list (`~SmTopology` of a deleted edge).
    pub(crate) fn remove_from_edge_list(&mut self, e: EdgeId) {
        self.edge_list.retain(|&x| x != e);
    }

    /// Removes `v` from the vertex list (`~SmTopology` of a deleted vertex).
    pub(crate) fn remove_from_vertex_list(&mut self, v: VertexId) {
        self.vertex_list.retain(|&x| x != v);
    }

    /// `PreInsert(pRegion)` on the B-rep's region list: the region joins the list as
    /// its new head.
    pub(crate) fn pre_insert_region(&mut self, r: RegionId) -> Result<()> {
        self.fix_region_order();
        self.region_order.retain(|&x| x != r);
        self.region_order.insert(0, r);
        self.region_mut(r)?.in_brep = true;
        self.region_list_head = Some(r);
        Ok(())
    }

    /// `PostInsert(pRegion)` on the B-rep's region list: the region joins the list at
    /// its end (it becomes the head of an empty list).
    pub(crate) fn post_insert_region(&mut self, r: RegionId) -> Result<()> {
        let empty = !self.regions.iter().any(|(_, x)| x.in_brep);
        self.fix_region_order();
        self.region_order.retain(|&x| x != r);
        self.region_order.push(r);
        self.region_mut(r)?.in_brep = true;
        if empty {
            self.region_list_head = Some(r);
        }
        Ok(())
    }

    /// `Remove(pRegion)` on the B-rep's region list: the head moves to the next
    /// region when it is removed (none when the list empties).
    pub(crate) fn remove_region_from_list(&mut self, r: RegionId) -> Result<()> {
        let order = p1::regions(self, None)?;
        if self.region_list_head == Some(r) {
            let pos = order.iter().position(|&x| x == r);
            let next = pos
                .and_then(|i| order.get(i + 1).or_else(|| order.first()))
                .copied()
                .filter(|&x| x != r);
            self.region_list_head = next;
        }
        self.fix_region_order();
        self.region_order.retain(|&x| x != r);
        self.region_mut(r)?.in_brep = false;
        Ok(())
    }
}

// ---- unported callees -----------------------------------------------------------------------

// ---- shells and regions (topology.shell) ----------------------------------------------------

/// `SmBoolean` `TRUE`.
const TRUE: i32 = 1;

/// `SmRegion::SetIsVoid`.
fn region_set_is_void(b: &mut Brep, r: RegionId, is_void: i32) -> Result<()> {
    b.region_mut(r)?.set_is_void(is_void);
    Ok(())
}

/// `SmRegion::IsVoid` as a condition (`TRUE` and `UNSURE` are both non-zero).
fn region_is_void(b: &Brep, r: RegionId) -> Result<bool> {
    Ok(b.region(r)?.is_void() != 0)
}

/// `pNewR->m_bIsVoidFlag = pR->m_bIsVoidFlag` (the `SmRegion` member).
fn copy_region_void_flag(dst: &mut Brep, r: RegionId, src: &Brep, from: RegionId) -> Result<()> {
    let flag = src.region(from)?.is_void();
    dst.region_mut(r)?.set_is_void(flag);
    Ok(())
}

/// `SmRegion::GetShells` (no attribute filter).
fn region_shells(b: &Brep, r: RegionId) -> Result<Vec<ShellId>> {
    b.region(r)?.get_shells(None, |_, _| Ok(true))
}

/// `pShell->m_pList = x; pShell->UpdateListSize()` (the size is recomputed from the
/// members, [`shell::shell_list_size`]).
fn set_shell_head(b: &mut Brep, s: ShellId, members: ShellMembers) -> Result<()> {
    b.shell_mut(s)?.members = members;
    Ok(())
}

/// `pShell->m_tShellType = ty`.
fn set_shell_type(b: &mut Brep, s: ShellId, ty: ShellType) -> Result<()> {
    b.shell_mut(s)?.set_shell_type(ty.code());
    Ok(())
}

/// Copies the list links of a face use (`m_pNext`, `m_pLast` through the copy map)
/// into `fu`; links derived from the shell's members (`None`) stay derived, from the
/// copied shell's mapped members.
fn copy_faceuse_links(
    dst: &mut Brep,
    fu: FaceuseId,
    src: &Brep,
    from: FaceuseId,
    map: &p1::ObjectMap,
) -> Result<()> {
    let link =
        |x: Option<FaceuseId>| x.and_then(|x| mapped(EntityRef::Faceuse(x), map, as_faceuse));
    let links = src
        .faceuse(from)?
        .sibling_links
        .map(|(next, last)| (link(next), link(last)));
    dst.faceuse_mut(fu)?.sibling_links = links;
    Ok(())
}

/// `SmFace::Notify(operation, data1, data2, data3)` of face `f`
/// ([`crate::topology::face::p3::notify`], with no trim cache: none is kept for `b`).
fn face_notify(
    b: &mut Brep,
    f: FaceId,
    env: &mut NotifyEnv<'_>,
    operation: NotifyOperation,
    data: [Option<NotifyArg>; 3],
) -> Result<()> {
    let data = data.map(|d| {
        d.map(|d| match d {
            NotifyArg::Entity(e) => (env.key_of)(e),
            NotifyArg::Object(r) => r,
        })
    });
    crate::topology::face::p3::notify(b, f, env, None, operation, data)
}

/// `SmFace::GetVertices` (unit `topology.face_p4`).
fn face_vertices(b: &Brep, f: FaceId) -> Result<Vec<VertexId>> {
    // Upstream defaults (SmFace.h:671): no attribute filter, all loops.
    crate::topology::face::p4::vertices(b, f, None, false, None)
}

/// `SmBrep::MakeShellVertex(pRegion, crPoint, rpNewShell, rpNewVertex)`
/// ([`super::p9::make_shell_vertex`]), notifying through `env`.
fn make_shell_vertex(
    b: &mut Brep,
    region: Option<RegionId>,
    point: Point3,
    env: &mut NotifyEnv<'_>,
) -> Result<(ShellId, VertexId)> {
    super::p9::make_shell_vertex(b, region, point, env)
}

/// `SmBrep::DeleteEdge(pEdge, pOptSurfaceToKeep)` (unit `topology.brep_p4`;
/// `surface_to_keep` is `NULL`, the default, at every call here).
fn delete_edge(
    k: &mut Kernel,
    b: &mut Brep,
    env: &mut NotifyEnv<'_>,
    e: EdgeId,
    surface_to_keep: Option<FaceId>,
) -> Result<()> {
    super::p4::delete_edge(
        k,
        b,
        &mut super::p4::DeleteEnv::from_notify(env),
        e,
        surface_to_keep,
    )
}

/// `SmBrep::DeleteVertex(pVertex)` (unit `topology.brep_p4`).
fn delete_vertex(k: &mut Kernel, b: &mut Brep, env: &mut NotifyEnv<'_>, v: VertexId) -> Result<()> {
    super::p4::delete_vertex(k, b, &mut super::p4::DeleteEnv::from_notify(env), v)
}

/// `SmBrep::CalculateBoundingBox(rBox, bApprox, bTight, bExpandByZoneTol3d)` (unit
/// `query.brep`).
fn brep_bounding_box(
    k: &Kernel,
    b: &Brep,
    approx: bool,
    tight: bool,
    expand_by_zone_tol_3d: bool,
) -> Result<Extent3d> {
    crate::query::brep::calculate_bounding_box(
        k,
        b,
        crate::query::brep::BoundingBoxOptions {
            approx,
            tight,
            expand_by_zone_tol_3d,
        },
    )
}

/// `SmEdgeuse::NormalizedEvaluate(t, bParameterSpaceEval, rPoint, &rFirstDeriv,
/// bOKToMakeUVTrimCurve)` (unit `topology.edgeuse_p3`): the point and first derivative
/// at normalized parameter `t`; a derivative the evaluation does not write keeps the
/// caller's default-constructed `SmVector3d` (`SM_UNDEF_DOUBLE`).
fn normalized_evaluate(
    k: &Kernel,
    b: &mut Brep,
    eu: EdgeuseId,
    t: f64,
    parameter_space: bool,
    ok_to_make_uv_trim_curve: bool,
) -> Result<(Point3, Vec3)> {
    let eval =
        eu3::normalized_evaluate(k, b, eu, t, parameter_space, true, ok_to_make_uv_trim_curve)?;
    Ok((eval.point, eval.first_deriv.unwrap_or(Vec3::UNDEF)))
}

/// The mark authority used by a deletion's shared traversal body.
pub(super) enum DeletionMarks<'a> {
    Indexed(&'a mut Marks),
    Retained,
}

struct RecordMarks<'a> {
    records: &'a mut crate::topology::topology::NotifyRecords,
    key_of: &'a dyn Fn(EntityRef) -> ObjectRef,
}

impl RecordMarks<'_> {
    fn key(&self, entity: Entity) -> ObjectKey {
        let entity = match entity {
            Entity::Face(id) => EntityRef::Face(id),
            Entity::Edge(id) => EntityRef::Edge(id),
            Entity::Vertex(id) => EntityRef::Vertex(id),
            Entity::Faceuse(id) => EntityRef::Faceuse(id),
            Entity::Vertexuse(id) => EntityRef::Vertexuse(id),
            Entity::Shell(id) => EntityRef::Shell(id),
        };
        (self.key_of)(entity).key
    }
}
impl FaceTraversalMarks for RecordMarks<'_> {
    fn is_marked(&self, k: &Kernel, entity: Entity, mark: MarkType) -> bool {
        self.records
            .get(&self.key(entity))
            .is_some_and(|record| record.is_marked(&k.context, mark))
    }
    fn mark(&mut self, k: &Kernel, entity: Entity, mark: MarkType) {
        let key = self.key(entity);
        self.records.or_default(key).mark(Some(&k.context), mark);
    }
}
impl DeletionMarks<'_> {
    pub(super) fn with_marks<T>(
        &mut self,
        records: &mut crate::topology::topology::NotifyRecords,
        key_of: &dyn Fn(EntityRef) -> ObjectRef,
        body: impl FnOnce(&mut dyn FaceTraversalMarks) -> T,
    ) -> T {
        match self {
            Self::Indexed(marks) => body(*marks),
            Self::Retained => body(&mut RecordMarks { records, key_of }),
        }
    }
}

// ---- helpers --------------------------------------------------------------------------------

/// `pRegion` or, for `None`, `GetInfiniteRegion()`.
fn region_or_infinite(b: &Brep, region: Option<RegionId>) -> Option<RegionId> {
    region.or_else(|| b.infinite_region())
}

/// `SmBrep::Notify` of this B-rep without spatial cache.
fn brep_notify(
    b: &Brep,
    env: &mut NotifyEnv<'_>,
    operation: NotifyOperation,
    data: [Option<NotifyArg>; 3],
) -> Result<()> {
    brep_notify_with_identity(b, env, operation, data, None)
}

fn brep_notify_with_identity(
    b: &Brep,
    env: &mut NotifyEnv<'_>,
    operation: NotifyOperation,
    data: [Option<NotifyArg>; 3],
    body: Option<ObjectRef>,
) -> Result<()> {
    // an object made by the edit has its (default) SmTopology record from its
    // constructor upstream; the arena's live in the caller's map, so one is added the
    // first time the edit names the object (seed_records covers the rest)
    for d in data.into_iter().flatten() {
        if let NotifyArg::Entity(e) = d {
            let key = (env.key_of)(e).key;
            env.records.or_default(key);
        }
    }
    let identity = body.unwrap_or_else(|| p2::brep_ref(b));
    env.records.or_default(identity.key);
    match body {
        Some(body) => p2::notify_with_body_identity(b, env, None, operation, data, body),
        None => p2::notify(b, env, None, operation, data),
    }
}

/// The topology entity `e` as a notification argument.
#[allow(clippy::unnecessary_wraps)]
const fn arg(e: EntityRef) -> Option<NotifyArg> {
    Some(NotifyArg::Entity(e))
}

/// The B-rep as a notification argument.
#[allow(clippy::unnecessary_wraps)]
const fn brep_arg(b: &Brep) -> Option<NotifyArg> {
    Some(NotifyArg::Object(p2::brep_ref(b)))
}

/// The edge use's vertex (`pEU->GetVertexuse()->GetVertex()`), `NER` on each.
fn eu_vertex(b: &Brep, eu: EdgeuseId) -> Result<(VertexuseId, VertexId)> {
    let vu = eu1::vertexuse(b, eu)?.ok_or_else(ner)?;
    let v = vertexuse::vertex(b, vu)?.ok_or_else(ner)?;
    Ok((vu, v))
}

/// `pV->Remove(pVU)`: takes the vertex use off the vertex's list.
fn vertex_remove_vertexuse(b: &mut Brep, v: VertexId, vu: VertexuseId) -> Result<()> {
    b.vertex_mut(v)?.vertexuses.retain(|&x| x != vu);
    Ok(())
}

/// `pE->Remove(pEU)`: takes the edge use off the edge's radial list.
fn edge_remove_edgeuse(b: &mut Brep, e: EdgeId, eu: EdgeuseId) -> Result<()> {
    b.edge_mut(e)?.edgeuses.retain(|&x| x != eu);
    Ok(())
}

/// `pFU->Remove(pLU)`: takes the loop use off the face use's list.
fn faceuse_remove_loopuse(b: &mut Brep, fu: FaceuseId, lu: LoopuseId) -> Result<()> {
    b.faceuse_mut(fu)?.loopuses.retain(|&x| x != lu);
    Ok(())
}

/// `m_pFaceListHead->Remove(pFace); delete pFace`: takes the face off the B-rep's face
/// list, notifies its destruction (`~SmFace`: `SmFace::Notify`, unit
/// `topology.face_p3`), drops the surface it owns and removes it from every attribute
/// it uses (`~SmAObject`'s `ClearAttributes`).
fn destroy_face(b: &mut Brep, env: &mut NotifyEnv<'_>, f: FaceId) -> Result<()> {
    b.remove_from_face_list(f);
    ignore_soft(face_notify(
        b,
        f,
        env,
        NotifyOperation::Destruction,
        [arg(EntityRef::Face(f)), None, None],
    ))?;
    let fref = (env.key_of)(EntityRef::Face(f));
    env.attributes.clear_attributes(fref.key)?;
    let expected = p1::default_face_owner(b, f);
    let face = b.face_mut(f)?;
    face.faceuse = None;
    let owned = face
        .surface
        .as_ref()
        .is_some_and(|s| s.owner().is_some_and(|o| o == expected || o == fref));
    if owned {
        face.surface = None;
        b.set_geometry_owner_key(EntityRef::Face(f), None);
    }
    Ok(())
}

// ---- construction -----------------------------------------------------------------------------

/// `SmBrep(dThisModelSizeEstimate, dThisLargeSmallSizeRatio, pOptContextForStack)`: a
/// new B-rep with the given size estimate and large/small ratio (upstream defaults
/// [`tol::USE_DEFAULT`]), the context's zone tolerance, editing disabled, and an
/// infinite void region as its only region; the construction is reported to the
/// context's callbacks (`SmObject::Notify`) under the new B-rep's own kernel identity
/// (the caller holds no key for it yet: [`topology_ref`] with the serial the kernel
/// context issues it, [`notify_serial`], so two new B-reps never share it, and a later
/// [`with_kernel_callbacks`] edit names it the same). [`Brep::key`] stays the caller's.
///
/// # Errors
///
/// Those of the B-rep's region list (a corrupt list); `InvalidInput` while the kernel
/// context's callbacks are lent (a construction inside a lent edit) or for an
/// exhausted identity domain; the construction notification's status is not checked,
/// as upstream.
pub fn new_brep(
    k: &mut Kernel,
    model_size_estimate: f64,
    large_small_size_ratio: f64,
) -> Result<Brep> {
    new_brep_in(k, model_size_estimate, large_small_size_ratio, None)
}

/// Construct under an already reserved caller identity, including the first
/// Construction notification. The native constructor body is shared with `new_brep`.
pub(crate) fn new_brep_for_caller(
    k: &mut Kernel,
    model_size_estimate: f64,
    large_small_size_ratio: f64,
    key: ObjectKey,
) -> Result<Brep> {
    if is_reserved_key(key) {
        return Err(RESERVED_KEY);
    }
    new_brep_in(k, model_size_estimate, large_small_size_ratio, Some(key))
}

fn new_brep_in(
    k: &mut Kernel,
    model_size_estimate: f64,
    large_small_size_ratio: f64,
    caller_key: Option<ObjectKey>,
) -> Result<Brep> {
    let mut b = Brep::new();
    // (its Euler operators name geometry owners by their notification identities)
    b.set_naming(crate::topology::NamingScheme::Notify);

    // When asked (should be extremly rare) set SystemTolerance sizes
    b.set_this_model_size_estimate(k, model_size_estimate);
    b.set_this_large_small_size_ratio(k, large_small_size_ratio);
    b.tolerance = tol::context_zone_tol_3d(k, Some(&k.context), SmallTopoType::NormalSize);

    // Create infinite region and put into brep's region list
    let r = b.add_region_record(Region::default())?;
    b.set_infinite_region(Some(r));
    region_set_is_void(&mut b, r, TRUE)?;
    b.pre_insert_region(r)?;

    // report construction at SmObject::Notify level - skip other levels
    let this = if let Some(key) = caller_key {
        b.set_key(key);
        p2::brep_ref(&b)
    } else {
        let serial = notify_serial(&mut k.context, &mut b)?;
        ObjectRef {
            key: topology_ref(serial, brep_entity()).key,
            ..p2::brep_ref(&b)
        }
    };
    object::notify(
        Some(&mut k.context),
        NotifyOperation::Construction,
        this,
        Some(this),
        None,
        None,
    );
    Ok(b)
}

/// The copy constructor `SmBrep(crBrepToCopy, pOptCopyBrepMap, pOptReverseMap,
/// bCopyAttributes, bAddAnalytics)`: a new B-rep with `src`'s editing and validation
/// flags, tolerance, size estimate and ratio, and a copy of its topology and geometry
/// ([`merge_brep`]; `add_analytics`, upstream default `TRUE`, recognises analytic
/// surfaces). `copy_map` gets copy to source, `reverse_map` source to copy.
/// `copy_attributes` (default `TRUE`) is unused upstream. `env` notifies the copy's
/// objects under `env.key_of`, the copy itself included: its [`Brep::key`] is
/// `env.key_of(EntityRef::Brep(_))`'s, set before anything is notified (the caller's
/// identity for the new object, as upstream's is its own address); `source_key` is
/// the identity of `src`'s objects (the two B-reps' ids overlap), and `src` keeps its
/// own [`Brep::key`].
///
/// # Errors
///
/// [`SolidError::Unported`] from [`merge_brep`] and [`p2::set_tolerance`] (other
/// `MergeBrep` failures are ignored, as upstream); those of the notification;
/// `InvalidInput` for a caller key in the kernel's reserved range.
#[allow(clippy::too_many_arguments)]
pub fn copy_brep(
    k: &mut Kernel,
    env: &mut NotifyEnv<'_>,
    src: &Brep,
    source_key: &dyn Fn(EntityRef) -> ObjectRef,
    copy_map: Option<&mut CopyBrepMap<EntityRef>>,
    reverse_map: Option<&mut CopyBrepMap<EntityRef>>,
    copy_attributes: bool,
    add_analytics: bool,
) -> Result<Brep> {
    let _ = copy_attributes;
    let mut b = Brep::new();
    b.set_key((env.key_of)(brep_entity()).key);
    // (not upstream) the copy's geometry is owned by `env.key_of`'s identities
    // (`SetOwner(pNewE)` etc. in `MergeBrep`), recorded per entity
    b.set_naming(crate::topology::NamingScheme::Caller);
    b.set_editing_enabled(src.editing_enabled());
    b.set_validate_only(src.validate_only());

    // Create infinite region and put into brep's region list
    let r = b.add_region_record(Region::default())?;
    b.set_infinite_region(Some(r));
    b.pre_insert_region(r)?;

    // copy tolerances
    p2::set_tolerance(k, &mut b, src.tolerance(), true, true)?;
    b.model_size_estimate = src.model_size_estimate;
    b.large_small_size_ratio = src.large_small_size_ratio;
    b.tolerance = src.tolerance;

    with_caller_identities(k, &mut b, env, Some((src, source_key)), |k, b, env| {
        // do the actual copying (SE: the status is not acted on)
        if let Err(e) = merge_brep_in(
            k,
            env,
            b,
            src,
            CopySource {
                key_of: source_key,
                context: None,
            },
            copy_map,
            reverse_map,
            add_analytics,
        ) {
            if e.is_unported() {
                return Err(e);
            }
        }

        // inform the public about the construction and the copy
        ignore_soft(brep_notify(
            b,
            env,
            NotifyOperation::Construction,
            [
                brep_arg(b),
                Some(NotifyArg::Object(p2::brep_ref(src))),
                None,
            ],
        ))
    })?;
    Ok(b)
}

/// A default `SmTopology` record (flags, user value, marks) for every object of `b`
/// that has none yet, under `key_of`, and for the B-rep itself ([`p2::brep_ref`]).
/// Every C++ topology object carries these fields; the arena keeps them in the
/// caller's [`NotifyEnv::records`], which `SmTopology::Notify` reads for every object a
/// notification names (e.g. both vertices of `SqueezeEdge`'s `SM_NO_MERGE_IN_BREP`).
/// Every edit seeds them ([`with_caller_identities`]), as a B-rep built upstream
/// already has them; a record the caller keeps is left as it is.
pub(crate) fn seed_records(
    records: &mut crate::topology::topology::NotifyRecords,
    b: &Brep,
    key_of: &dyn Fn(EntityRef) -> ObjectRef,
) {
    if records.is_empty() {
        // Bulk build (sorted input is O(n)); the same map as entry-by-entry.
        *records = entities(b)
            .map(|e| key_of(e).key)
            .chain([p2::brep_ref(b).key])
            .map(|k| (k, topo::Topology::default()))
            .collect();
        return;
    }
    for e in entities(b) {
        records.or_default(key_of(e).key);
    }
    records.or_default(p2::brep_ref(b).key);
}

/// Leaves `records` exactly as [`seed_records`] does on an empty map (a default record
/// for every object of `b`, and for the B-rep), whatever it held before: records of
/// objects that are still there are reset in place and the rest is added or dropped, so
/// an edit that notifies through the same B-rep again reuses most of the map.
fn reseed_records(
    records: &mut crate::topology::topology::NotifyRecords,
    b: &Brep,
    key_of: &dyn Fn(EntityRef) -> ObjectRef,
) {
    // The B-rep first: kernel keys (kind 0) and `entities` (by kind, then slot) come
    // out ascending, so the sort is skipped, as it gives the same vector either way.
    let mut keys: Vec<ObjectKey> = std::iter::once(p2::brep_ref(b).key)
        .chain(entities(b).map(|e| key_of(e).key))
        .collect();
    if !keys.is_sorted() {
        keys.sort_unstable();
    }
    keys.dedup();
    // (both ascending: `retain` visits the records in key order)
    let mut next = 0;
    records.retain(|key, record| {
        while keys.get(next).is_some_and(|k| k < key) {
            next += 1;
        }
        let keep = keys.get(next) == Some(key);
        *record = topo::Topology::default();
        keep
    });
    if records.len() != keys.len() {
        for key in keys {
            records.or_default(key);
        }
    }
}

/// The kinds of topology keys with their class type codes (`GetType`), by kind number:
/// the B-rep, then region, shell, face use, face, loop use, loop, edge use, edge,
/// vertex use, vertex.
const TOPOLOGY_TYPE_CODES: [i64; 11] = [
    16011, 16012, 16013, 16022, 16002, 16004, 16003, 16005, 16016, 16006, 16021,
];

/// The B-rep itself as an entity of its topology keys (kind 0, index 0).
const fn brep_entity() -> EntityRef {
    EntityRef::Brep(crate::topology::BrepId::from_index(0))
}

/// The kernel-issued topology identity of `e` in the B-rep of serial `serial`
/// ([`notify_serial`]): its class type code and the key `TOPOLOGY_KEY_BASE | serial <<
/// 36 | kind << 32 | index` (kinds as [`topology_entity`] decodes them; the B-rep is
/// kind 0, index 0). Serials are at most `MAX_BREP_SERIAL` (higher bits are dropped).
#[must_use]
pub fn topology_ref(serial: u64, e: EntityRef) -> ObjectRef {
    let (kind, i): (usize, u32) = match e {
        EntityRef::Brep(_) => (0, 0),
        EntityRef::Region(x) => (1, x.index()),
        EntityRef::Shell(x) => (2, x.index()),
        EntityRef::Faceuse(x) => (3, x.index()),
        EntityRef::Face(x) => (4, x.index()),
        EntityRef::Loopuse(x) => (5, x.index()),
        EntityRef::Loop(x) => (6, x.index()),
        EntityRef::Edgeuse(x) => (7, x.index()),
        EntityRef::Edge(x) => (8, x.index()),
        EntityRef::Vertexuse(x) => (9, x.index()),
        EntityRef::Vertex(x) => (10, x.index()),
    };
    ObjectRef {
        type_code: TOPOLOGY_TYPE_CODES.get(kind).copied().unwrap_or(0),
        key: ObjectKey(
            TOPOLOGY_KEY_BASE
                | ((serial & MAX_BREP_SERIAL) << 36)
                | ((kind as u64) << 32)
                | u64::from(i),
        ),
    }
}

/// The entity of the B-rep of serial `serial` that [`topology_ref`] made `key` for;
/// `None` for any other key (another B-rep's, a geometry key, a caller's label).
#[must_use]
pub fn topology_entity(serial: u64, key: ObjectKey) -> Option<EntityRef> {
    use crate::topology::{BrepId, LoopId};
    if key.0 >> 62 != TOPOLOGY_KEY_BASE >> 62 || (key.0 >> 36) & MAX_BREP_SERIAL != serial {
        return None;
    }
    let i = u32::try_from(key.0 & 0xffff_ffff).ok()?;
    Some(match (key.0 >> 32) & 0xf {
        0 if i == 0 => EntityRef::Brep(BrepId::from_index(0)),
        1 => EntityRef::Region(RegionId::from_index(i)),
        2 => EntityRef::Shell(ShellId::from_index(i)),
        3 => EntityRef::Faceuse(FaceuseId::from_index(i)),
        4 => EntityRef::Face(FaceId::from_index(i)),
        5 => EntityRef::Loopuse(LoopuseId::from_index(i)),
        6 => EntityRef::Loop(LoopId::from_index(i)),
        7 => EntityRef::Edgeuse(EdgeuseId::from_index(i)),
        8 => EntityRef::Edge(EdgeId::from_index(i)),
        9 => EntityRef::Vertexuse(VertexuseId::from_index(i)),
        10 => EntityRef::Vertex(VertexId::from_index(i)),
        _ => return None,
    })
}

/// The serial `b`'s objects are notified under through `context` (their topology keys
/// hold it, [`topology_ref`]): the one `context`'s identity domain issued `b`, or a
/// fresh one, kept on `b`. The kernel issues it; a caller can only read it.
///
/// # Errors
///
/// `InvalidInput` once the domain has issued every serial (`MAX_BREP_SERIAL`).
pub fn notify_serial(context: &mut Context, b: &mut Brep) -> Result<u64> {
    context.brep_serial(b.notify_identity_slot())
}

/// The context an edit's geometry identities come from: the one its notifications go
/// through (`env.context`), else the kernel's (nothing is notified then).
fn issuer<'c>(kernel: &'c mut Context, env: Option<&'c mut Context>) -> &'c mut Context {
    match env {
        Some(c) => c,
        None => kernel,
    }
}

/// Every topology object of `b` (not the B-rep itself), without collecting them: the
/// edit entry points walk this per call.
fn entities(b: &Brep) -> impl Iterator<Item = EntityRef> + '_ {
    let regions = b.regions().iter().map(|(x, _)| EntityRef::Region(x));
    let shells = b.shells().iter().map(|(x, _)| EntityRef::Shell(x));
    let faceuses = b.faceuses().iter().map(|(x, _)| EntityRef::Faceuse(x));
    let faces = b.faces().iter().map(|(x, _)| EntityRef::Face(x));
    let loopuses = b.loopuses().iter().map(|(x, _)| EntityRef::Loopuse(x));
    let loops = b.loops().iter().map(|(x, _)| EntityRef::Loop(x));
    let edgeuses = b.edgeuses().iter().map(|(x, _)| EntityRef::Edgeuse(x));
    let edges = b.edges().iter().map(|(x, _)| EntityRef::Edge(x));
    let vertexuses = b.vertexuses().iter().map(|(x, _)| EntityRef::Vertexuse(x));
    let vertices = b.vertices().iter().map(|(x, _)| EntityRef::Vertex(x));
    regions
        .chain(shells)
        .chain(faceuses)
        .chain(faces)
        .chain(loopuses)
        .chain(loops)
        .chain(edgeuses)
        .chain(edges)
        .chain(vertexuses)
        .chain(vertices)
}

/// The number of objects [`entities`] yields.
fn entity_count(b: &Brep) -> usize {
    b.regions().len()
        + b.shells().len()
        + b.faceuses().len()
        + b.faces().len()
        + b.loopuses().len()
        + b.loops().len()
        + b.edgeuses().len()
        + b.edges().len()
        + b.vertexuses().len()
        + b.vertices().len()
}

/// The refusal of a caller key in the kernel's reserved key range.
const RESERVED_KEY: SolidError = SolidError::InvalidInput {
    operation: "topology::brep::p3",
    reason: "a topology key of the caller is in the kernel's reserved key range",
};

/// The objects of each kind an edit of `b` may still allocate: the next `window` ids
/// (arenas only append, so new objects take exactly these).
fn prospective_entities(b: &Brep, window: usize) -> impl Iterator<Item = EntityRef> {
    let next =
        move |len: usize| (len..len.saturating_add(window)).map_while(|i| u32::try_from(i).ok());
    let regions = next(b.regions().len()).map(|i| EntityRef::Region(RegionId::from_index(i)));
    let shells = next(b.shells().len()).map(|i| EntityRef::Shell(ShellId::from_index(i)));
    let faceuses = next(b.faceuses().len()).map(|i| EntityRef::Faceuse(FaceuseId::from_index(i)));
    let faces = next(b.faces().len()).map(|i| EntityRef::Face(FaceId::from_index(i)));
    let loopuses = next(b.loopuses().len()).map(|i| EntityRef::Loopuse(LoopuseId::from_index(i)));
    let loops =
        next(b.loops().len()).map(|i| EntityRef::Loop(crate::topology::LoopId::from_index(i)));
    let edgeuses = next(b.edgeuses().len()).map(|i| EntityRef::Edgeuse(EdgeuseId::from_index(i)));
    let edges = next(b.edges().len()).map(|i| EntityRef::Edge(EdgeId::from_index(i)));
    let vertexuses =
        next(b.vertexuses().len()).map(|i| EntityRef::Vertexuse(VertexuseId::from_index(i)));
    let vertices = next(b.vertices().len()).map(|i| EntityRef::Vertex(VertexId::from_index(i)));
    regions
        .chain(shells)
        .chain(faceuses)
        .chain(faces)
        .chain(loopuses)
        .chain(loops)
        .chain(edgeuses)
        .chain(edges)
        .chain(vertexuses)
        .chain(vertices)
}

/// The number of objects of one kind any edit here allocates at most: the B-rep's own
/// count (`DeleteFace` splits at most that many shells), one per object of a B-rep it
/// reads (copies and merges), three per entry of an input list (`CopyVertices` makes a
/// shell, vertex use and vertex for every entry, repeated ones included, as upstream)
/// and a few for the wire makers.
fn allocation_window(b: &Brep, source: Option<&Brep>, inputs: usize) -> usize {
    let n = entity_count(b);
    let m = source.map_or(0, entity_count);
    n.saturating_add(m)
        .saturating_add(inputs.saturating_mul(3))
        .saturating_add(16)
}

/// Every key `key_of` gives `b`'s objects, those an edit may allocate
/// ([`prospective_entities`], `window` of each kind) and the B-rep's own
/// ([`p2::brep_ref`]) is a caller key (below [`crate::geom::context::KERNEL_KEY_BASE`]) or
/// the object's own kernel identity in `context`'s domain ([`topology_ref`] with `b`'s
/// serial there, as [`with_kernel_callbacks`] names them). So no topology object, old or
/// new, shares a key with a curve, nor with another B-rep's kernel-named objects.
///
/// # Errors
///
/// `InvalidInput` for any other key in the reserved range.
fn check_keys(
    context: &Context,
    b: &Brep,
    key_of: &dyn Fn(EntityRef) -> ObjectRef,
    window: usize,
) -> Result<()> {
    let serial = context.issued_brep_serial(b.notify_identity());
    let own = |e: EntityRef, key: ObjectKey| {
        !is_reserved_key(key) || serial.is_some_and(|s| topology_entity(s, key) == Some(e))
    };
    let all_own = entities(b).into_iter().all(|e| own(e, key_of(e).key))
        && prospective_entities(b, window)
            .into_iter()
            .all(|e| own(e, key_of(e).key))
        && own(brep_entity(), p2::brep_ref(b).key);
    if all_own {
        Ok(())
    } else {
        Err(RESERVED_KEY)
    }
}

/// Runs the edit `edit` of `b` under the caller's identities: `env.key_of` for `b`'s
/// objects and `source` for those of a B-rep it reads (copies or merges from), the
/// B-reps by their [`Brep::key`]. Refuses a key in the kernel's reserved range first,
/// for the objects the edit may create too ([`check_keys`]), then gives every object of both a default `SmTopology` record
/// unless the caller keeps one ([`seed_records`]), as every upstream object carries
/// one; records the caller keeps are used as they are.
///
/// # Errors
///
/// `InvalidInput` for a reserved key; those of `edit`.
fn with_caller_identities<R>(
    k: &mut Kernel,
    b: &mut Brep,
    env: &mut NotifyEnv<'_>,
    source: Option<(&Brep, &dyn Fn(EntityRef) -> ObjectRef)>,
    edit: impl FnOnce(&mut Kernel, &mut Brep, &mut NotifyEnv<'_>) -> Result<R>,
) -> Result<R> {
    with_caller_identities_for(k, b, env, source, 0, edit)
}

/// [`with_caller_identities`] for an edit that allocates per entry of an input list of
/// `inputs` entries ([`allocation_window`]).
pub(crate) fn with_caller_identities_for<R>(
    k: &mut Kernel,
    b: &mut Brep,
    env: &mut NotifyEnv<'_>,
    source: Option<(&Brep, &dyn Fn(EntityRef) -> ObjectRef)>,
    inputs: usize,
    edit: impl FnOnce(&mut Kernel, &mut Brep, &mut NotifyEnv<'_>) -> Result<R>,
) -> Result<R> {
    let serial = {
        let context = env.context.as_deref().unwrap_or(&k.context);
        context.issued_brep_serial(b.notify_identity())
    };
    let id = progress_id(b, serial, env.key_of);
    let instance = b.progress_tag();
    {
        let window = allocation_window(b, source.map(|(src, _)| src), inputs);
        let progress = (id.clone(), std::sync::Arc::clone(&instance));
        check_keys_once(env.records, progress, serial, b, env.key_of, window)?;
        if let Some((src, key_of)) = source {
            // (a source is read, never added to)
            let context = env.context.as_deref().unwrap_or(&k.context);
            check_keys(context, src, key_of, 0)?;
        }
    }
    seed_records_once(env.records, (id, instance), b, env.key_of)?;
    if let Some((src, key_of)) = source {
        seed_records(env.records, src, key_of);
    }
    edit(k, b, env)
}

/// The kinds of topology object, in [`entities`]' order.
const KINDS: usize = 10;

/// The object of kind `kind` (in [`entities`]' order) at arena index `index`.
fn kind_entity(kind: usize, index: u32) -> EntityRef {
    match kind {
        0 => EntityRef::Region(RegionId::from_index(index)),
        1 => EntityRef::Shell(ShellId::from_index(index)),
        2 => EntityRef::Faceuse(FaceuseId::from_index(index)),
        3 => EntityRef::Face(FaceId::from_index(index)),
        4 => EntityRef::Loopuse(LoopuseId::from_index(index)),
        5 => EntityRef::Loop(crate::topology::LoopId::from_index(index)),
        6 => EntityRef::Edgeuse(EdgeuseId::from_index(index)),
        7 => EntityRef::Edge(EdgeId::from_index(index)),
        8 => EntityRef::Vertexuse(VertexuseId::from_index(index)),
        _ => EntityRef::Vertex(VertexId::from_index(index)),
    }
}

/// Whether the object of kind `kind` (in [`entities`]' order) at arena index `index`
/// is live (not a removed slot).
fn is_live(b: &Brep, kind: usize, index: u32) -> bool {
    match kind {
        0 => b.regions().get(RegionId::from_index(index)).is_some(),
        1 => b.shells().get(ShellId::from_index(index)).is_some(),
        2 => b.faceuses().get(FaceuseId::from_index(index)).is_some(),
        3 => b.faces().get(FaceId::from_index(index)).is_some(),
        4 => b.loopuses().get(LoopuseId::from_index(index)).is_some(),
        5 => b
            .loops()
            .get(crate::topology::LoopId::from_index(index))
            .is_some(),
        6 => b.edgeuses().get(EdgeuseId::from_index(index)).is_some(),
        7 => b.edges().get(EdgeId::from_index(index)).is_some(),
        8 => b.vertexuses().get(VertexuseId::from_index(index)).is_some(),
        _ => b.vertices().get(VertexId::from_index(index)).is_some(),
    }
}

/// The slot count of each kind, removed slots included (the next index of each).
fn slot_counts(b: &Brep) -> [usize; KINDS] {
    [
        b.regions().slot_count(),
        b.shells().slot_count(),
        b.faceuses().slot_count(),
        b.faces().slot_count(),
        b.loopuses().slot_count(),
        b.loops().slot_count(),
        b.edgeuses().slot_count(),
        b.edges().slot_count(),
        b.vertexuses().slot_count(),
        b.vertices().slot_count(),
    ]
}

/// The live-object count of each kind (what [`prospective_entities`] counts from).
fn live_counts(b: &Brep) -> [usize; KINDS] {
    [
        b.regions().len(),
        b.shells().len(),
        b.faceuses().len(),
        b.faces().len(),
        b.loopuses().len(),
        b.loops().len(),
        b.edgeuses().len(),
        b.edges().len(),
        b.vertexuses().len(),
        b.vertices().len(),
    ]
}

/// What a [`topo::NotifyRecords`]' progress for `b` is valid under: the B-rep's key,
/// its serial in the edit's context, and the key `key_of` gives the first index of
/// every kind (a caller's identities are a fixed function of kind and index).
fn progress_id(b: &Brep, serial: Option<u64>, key_of: &dyn Fn(EntityRef) -> ObjectRef) -> Vec<u64> {
    let mut id = vec![p2::brep_ref(b).key.0, serial.unwrap_or(u64::MAX)];
    id.extend((0..KINDS).map(|kind| key_of(kind_entity(kind, 0)).key.0));
    id
}

/// One past the last arena index (ids are `u32`).
const INDEX_END: usize = (u32::MAX as usize).saturating_add(1);

/// [`check_keys`] of `b` (no source) for one edit, deciding each arena index once
/// per store and B-rep value (`progress`: the [`progress_id`] and
/// [`Brep::progress_tag`]): an index's key depends on the index alone (`key_of`,
/// under the same id), so the objects and prospective indices the check covers are
/// the same, and only those not yet passed are asked. Those are the indices from
/// [`topo::SeedProgress::checked_to`] on, and the emptied slots below it that this
/// edit's window reaches (an index below it that was live then has passed, and an
/// emptied slot stays empty), so an edit asks its new objects and the window's
/// growth, not the whole B-rep. A refusal marks nothing.
fn check_keys_once(
    records: &mut topo::NotifyRecords,
    progress: (Vec<u64>, std::sync::Arc<()>),
    serial: Option<u64>,
    b: &Brep,
    key_of: &dyn Fn(EntityRef) -> ObjectRef,
    window: usize,
) -> Result<()> {
    let own = |e: EntityRef, key: ObjectKey| {
        !is_reserved_key(key) || serial.is_some_and(|s| topology_entity(s, key) == Some(e))
    };
    let (_, progress) = records
        .progress_for(progress.0, progress.1)
        .ok_or(RESERVED_KEY)?;
    // (the progress only moves once every index this edit covers has passed)
    let mut checked_to = progress.checked_to;
    let mut passed: Vec<(usize, usize)> = Vec::new();
    let mut skipped: Vec<(usize, usize)> = Vec::new();
    let kinds = live_counts(b)
        .into_iter()
        .zip(slot_counts(b))
        .zip(checked_to.iter_mut())
        .enumerate();
    for (kind, ((count, slots), to)) in kinds {
        let end = count.saturating_add(window).min(INDEX_END);
        let ask = |i: usize| {
            u32::try_from(i).is_ok_and(|index| {
                let e = kind_entity(kind, index);
                own(e, key_of(e).key)
            })
        };
        if let Some(unchecked) = progress.unchecked.get(kind) {
            for &i in unchecked.range(count..end) {
                if !ask(i) {
                    return Err(RESERVED_KEY);
                }
                passed.push((kind, i));
            }
        }
        let from = *to;
        *to = from.max(slots).max(end);
        for i in from..*to {
            let covered = (count..end).contains(&i)
                || u32::try_from(i).is_ok_and(|index| is_live(b, kind, index));
            if !covered {
                // (an emptied slot outside the window)
                skipped.push((kind, i));
            } else if !ask(i) {
                return Err(RESERVED_KEY);
            }
        }
    }
    if !own(brep_entity(), p2::brep_ref(b).key) {
        return Err(RESERVED_KEY);
    }
    progress.checked_to = checked_to;
    for (kind, i) in passed {
        if let Some(unchecked) = progress.unchecked.get_mut(kind) {
            unchecked.remove(&i);
        }
    }
    for (kind, i) in skipped {
        if let Some(unchecked) = progress.unchecked.get_mut(kind) {
            unchecked.insert(i);
        }
    }
    Ok(())
}

/// [`seed_records`] of `b` for one edit, giving each live object its record once per
/// store and B-rep value (`progress`, as [`check_keys_once`]): records are only
/// removed through [`topo::NotifyRecords`]' own methods, which forget the progress,
/// and a live object below [`topo::SeedProgress::seeded_to`] was live, so seeded,
/// when the mark was set (arenas only append and empty slots for good). So only the
/// slots added since are visited, and the store ends exactly as a full seeding leaves
/// it.
fn seed_records_once(
    records: &mut topo::NotifyRecords,
    progress: (Vec<u64>, std::sync::Arc<()>),
    b: &Brep,
    key_of: &dyn Fn(EntityRef) -> ObjectRef,
) -> Result<()> {
    let brep_key = p2::brep_ref(b).key;
    let (map, progress) = records
        .progress_for(progress.0, progress.1)
        .ok_or(RESERVED_KEY)?;
    let kinds = slot_counts(b)
        .into_iter()
        .zip(progress.seeded_to.iter_mut())
        .enumerate();
    for (kind, (slots, seeded_to)) in kinds {
        for index in (*seeded_to..slots).map_while(|i| u32::try_from(i).ok()) {
            if is_live(b, kind, index) {
                map.entry(key_of(kind_entity(kind, index)).key).or_default();
            }
        }
        *seeded_to = slots.max(*seeded_to);
    }
    map.entry(brep_key).or_default();
    Ok(())
}

/// Lends the kernel context's notify callbacks and identity domain to a stand-in
/// context for `run` (`Context::lend_notify`; upstream notifies through the B-rep's
/// context, which a `&mut Kernel` edit cannot also borrow) and gives them back
/// afterwards, whatever `run` returns.
///
/// # Errors
///
/// `InvalidInput`, before anything changes, when they are already lent: a lent edit
/// nested in another would notify nobody, in another domain.
pub(crate) fn with_lent_context<R>(
    k: &mut Kernel,
    run: impl FnOnce(&mut Kernel, &mut Context) -> R,
) -> Result<R> {
    let mut ctx = Context::default();
    ctx.install_lease(k.context.lend_notify()?);
    let r = run(k, &mut ctx);
    k.context.install_lease(ctx.take_lease());
    Ok(r)
}

/// Runs `edit` of `b` for a caller without identities of its own, notified through
/// `context`: `b`'s objects are named by their kernel identities ([`topology_ref`]
/// with `b`'s serial in `context`'s domain, [`notify_serial`]) and the B-rep by its
/// kernel key for the edit (`Brep::key` is restored afterwards, whatever the result),
/// with a default `SmTopology` record for every existing object of `b` from the start
/// (as every upstream object has one, so a notification sent before any p3 edit runs,
/// e.g. part 5's edge merge, finds the records it reads) and `b`'s own attributes
/// ([`Brep::add_attribute`]) under those identities, so the edit's notifications
/// propagate them as upstream's live on the objects; afterwards `b` gets back the
/// attributes left on its live objects, whatever the result (flags are not kept
/// between edits).
///
/// # Errors
///
/// Those of `edit`; `InvalidInput` for an exhausted identity domain.
pub(crate) fn kernel_named_edit<R>(
    k: &mut Kernel,
    context: &mut Context,
    b: &mut Brep,
    edit: impl FnOnce(&mut Kernel, &mut Brep, &mut NotifyEnv<'_>) -> Result<R>,
) -> Result<R> {
    let serial = notify_serial(context, b)?;
    let key_of = move |e: EntityRef| topology_ref(serial, e);
    let label = b.key();
    b.set_key(topology_ref(serial, brep_entity()).key);
    let mut records = std::mem::take(&mut k.notify_records);
    reseed_records(&mut records, b, &key_of);
    let mut attributes = crate::geom::attribute::AttributeStore::default();
    // (not upstream) the B-rep's own attributes, under the edit's identities
    let seeded = p2::seed_brep_attributes(&mut attributes, b.take_attributes(), &key_of);
    let r = seeded.and_then(|()| {
        let mut env = NotifyEnv {
            records: &mut records,
            attributes: &mut attributes,
            context: Some(context),
            key_of: &key_of,
        };
        edit(k, b, &mut env)
    });
    b.set_key(label);
    k.notify_records = records;
    let restored =
        p2::restore_decoded_attributes(b, attributes, &|key| topology_entity(serial, key));
    let r = r?;
    restored?;
    Ok(r)
}

/// Runs `edit` for a caller that keeps no notification state of its own (upstream's
/// objects carry it): through the kernel context's notify callbacks and identity
/// domain, lent for the edit and given back (`with_lent_context`), under the
/// kernel's identities (`kernel_named_edit`).
///
/// Not nestable: inside `edit`, notify through the `env` it is given. A
/// `with_kernel_callbacks` (or the healer's tracked edit) inside `edit` is refused.
///
/// # Errors
///
/// Those of `edit`; `InvalidInput` for a nested call (before anything changes) and for
/// an exhausted identity domain.
pub fn with_kernel_callbacks<R>(
    k: &mut Kernel,
    b: &mut Brep,
    edit: impl FnOnce(&mut Kernel, &mut Brep, &mut NotifyEnv<'_>) -> Result<R>,
) -> Result<R> {
    with_lent_context(k, |k, ctx| kernel_named_edit(k, ctx, b, edit))?
}

/// `SmApiBrepCopy`: a copy of `brep` that preserves its surface types (the copy
/// constructor without maps, `bAddAnalytics = FALSE`). `source_key` is the
/// notification identity of `brep`'s objects (see [`copy_brep`]).
///
/// (not upstream: upstream's attributes live on the objects.) `brep`'s own attributes
/// ([`Brep::add_attribute`]) go to the copy as the copy's `SM_NO_COPY` notifications
/// carry them (SMLib's `Copy` handler: copy and reference attributes are passed on,
/// temporary ones are not); the copy owns the attributes its objects end with, also
/// those the caller seeded in `env`. `env.attributes` keeps the copy's lists, but not
/// `brep`'s, which are lent for the copy only (a standalone attribute of `brep`
/// stays there without users).
///
/// # Errors
///
/// `SM_ERR_INVALID_INPUT` for no B-rep; those of [`copy_brep`].
pub fn api_brep_copy(
    k: &mut Kernel,
    env: &mut NotifyEnv<'_>,
    brep: Option<&Brep>,
    source_key: &dyn Fn(EntityRef) -> ObjectRef,
) -> Result<Brep> {
    let Some(brep) = brep else {
        return Err(SolidError::Upstream {
            operation: OP,
            code: SM_ERR_INVALID_INPUT,
            entity: None,
        });
    };
    // (not upstream) lend `brep`'s own attributes to the copy under `source_key`
    let lent: Vec<(ObjectKey, usize)> = brep
        .attribute_store()
        .attributed_objects()
        .into_iter()
        .filter_map(crate::topology::loopuse::entity_of_notify_key)
        .map(|e| {
            let key = source_key(e).key;
            (key, env.attributes.attributes(key).len())
        })
        .collect();
    p2::seed_brep_attributes(env.attributes, brep.attribute_store().clone(), source_key)?;
    let copy = copy_brep(k, env, brep, source_key, None, None, true, false);
    for (key, before) in lent {
        for h in env.attributes.attributes(key).into_iter().skip(before) {
            env.attributes.remove_attribute(key, h, false)?;
        }
    }
    let mut copy = copy?;
    p2::restore_keyed_attributes(&mut copy, env.attributes, env.key_of)?;
    // (not upstream: the port's cached topology fields, true for the caller)
    copy.refresh_cached_topology();
    Ok(copy)
}

/// Identities and optional separate notification context of a copy's source.
/// `None` means the source shares the destination's notification context.
pub struct CopySource<'a> {
    /// Source topology identities; arena ids alone are not object identities.
    pub key_of: &'a dyn Fn(EntityRef) -> ObjectRef,
    /// Source callbacks when the bodies belong to distinct contexts.
    pub context: Option<&'a mut Context>,
}

/// `CopyVertices`: copies each vertex of `src` into `dest` as a shell vertex of its
/// infinite region at the same point, reporting each copy (`SM_NO_COPY`, which carries
/// the flags, user values and attributes to the copy). Returns the new vertices
/// (`pNewVertices`). `env` notifies `dest`'s objects under `env.key_of`; `source_key`
/// is the identity of `src`'s (the two B-reps' ids overlap).
///
/// # Errors
///
/// Those of `SmBrep::MakeShellVertex` ([`super::p9::make_shell_vertex`]);
/// [`SolidError::NotFound`] for a dangling id; `InvalidInput` for a caller key in the
/// kernel's reserved range.
pub fn copy_vertices(
    k: &mut Kernel,
    src: &Brep,
    vertices: &[VertexId],
    dest: &mut Brep,
    env: &mut NotifyEnv<'_>,
    source_key: &dyn Fn(EntityRef) -> ObjectRef,
) -> Result<Vec<VertexId>> {
    copy_vertices_with_context(
        k,
        src,
        vertices,
        dest,
        env,
        CopySource {
            key_of: source_key,
            context: None,
        },
    )
}

/// [`copy_vertices`] with the source's own callbacks when its context differs.
pub fn copy_vertices_with_context(
    k: &mut Kernel,
    src: &Brep,
    vertices: &[VertexId],
    dest: &mut Brep,
    env: &mut NotifyEnv<'_>,
    source: CopySource<'_>,
) -> Result<Vec<VertexId>> {
    let inputs = vertices.len();
    with_caller_identities_for(
        k,
        dest,
        env,
        Some((src, source.key_of)),
        inputs,
        |_, dest, env| copy_vertices_in(src, source, vertices, dest, env),
    )
}

/// [`copy_vertices`] once the identities are checked and seeded.
fn copy_vertices_in(
    src: &Brep,
    mut source: CopySource<'_>,
    vertices: &[VertexId],
    dest: &mut Brep,
    env: &mut NotifyEnv<'_>,
) -> Result<Vec<VertexId>> {
    let mut out = Vec::new();
    // for every vertex - copy as Shell vertex
    for &v in vertices {
        let coords = vertex::point(src, v)?;
        let inf = dest.infinite_region();

        // make new ShellVertex
        let (_, new_v) = make_shell_vertex(dest, inf, coords, env)?;

        // inform the public - propogate attributes and report copy
        let this = (source.key_of)(EntityRef::Vertex(v));
        let to = (env.key_of)(EntityRef::Vertex(new_v));
        notify_copy_with_context(
            env,
            source.context.as_deref_mut(),
            this,
            Some(to),
            Some(p2::brep_ref(dest)),
            Some(p2::brep_ref(src)),
        )?;
        out.push(new_v);
    }
    Ok(out)
}

// ---- MergeBrep ----------------------------------------------------------------------------------

/// `pFrom->Notify(SM_NO_COPY, pTo, data2, data3)`: every topology `Notify` override
/// (`SmFace::Notify` of `topology.face_p3` included) passes a copy straight to
/// `SmTopology::Notify`, which carries the flags and user values of `this` to `to`
/// (resetting its marks) and then the attributes and callbacks (`SmAObject`).
/// `Notify` is `void`: only the errors of [`ignore_soft`] propagate.
pub(crate) fn notify_copy_with_context(
    env: &mut NotifyEnv<'_>,
    source_context: Option<&mut Context>,
    this: ObjectRef,
    to: Option<ObjectRef>,
    data2: Option<ObjectRef>,
    data3: Option<ObjectRef>,
) -> Result<()> {
    // (a new object's SmTopology constructor makes its record; an object the caller
    // keeps no record for has the constructor's values)
    for key in core::iter::once(this.key).chain(to.map(|t| t.key)) {
        env.records.or_default(key);
    }
    ignore_soft(topo::notify(
        &mut *env.records,
        &mut *env.attributes,
        match source_context {
            Some(context) => Some(context),
            None => env.context.as_deref_mut(),
        },
        NotifyOperation::Copy,
        this,
        to,
        data2,
        data3,
    ))
}

/// The copy recorded for `from` cast to the expected kind.
fn mapped<T>(
    from: EntityRef,
    map: &p1::ObjectMap,
    cast: impl Fn(EntityRef) -> Option<T>,
) -> Option<T> {
    p1::get_object_map(from, map).and_then(cast)
}

const fn as_region(e: EntityRef) -> Option<RegionId> {
    match e {
        EntityRef::Region(r) => Some(r),
        _ => None,
    }
}

const fn as_shell(e: EntityRef) -> Option<ShellId> {
    match e {
        EntityRef::Shell(s) => Some(s),
        _ => None,
    }
}

const fn as_loopuse(e: EntityRef) -> Option<LoopuseId> {
    match e {
        EntityRef::Loopuse(x) => Some(x),
        _ => None,
    }
}

const fn as_edgeuse(e: EntityRef) -> Option<EdgeuseId> {
    match e {
        EntityRef::Edgeuse(x) => Some(x),
        _ => None,
    }
}

const fn as_vertexuse(e: EntityRef) -> Option<VertexuseId> {
    match e {
        EntityRef::Vertexuse(x) => Some(x),
        _ => None,
    }
}

const fn as_faceuse(e: EntityRef) -> Option<FaceuseId> {
    match e {
        EntityRef::Faceuse(x) => Some(x),
        _ => None,
    }
}

/// `MergeBrep`: adds a copy of `other`'s topology graph (regions except the infinite
/// one, which is mapped onto this B-rep's, shells, faces, loops, edges, vertices and
/// their uses, with copies of the surfaces, curves and UV trim curves) to `b`, without
/// intersection or coincidence checks. Surfaces are copied with
/// `CopyAndAddAnalytics` when `add_analytics` (upstream default `TRUE`), else with
/// `Copy`. Every new object is reported (`SM_NO_COPY`) once the graph is complete.
/// `copy_map` gets copy to source, `reverse_map` source to copy (`sm_SetObjectMap`).
/// `env` notifies `b`'s objects under `env.key_of`; `source_key` is the identity of
/// `other`'s (the two B-reps' ids overlap).
///
/// # Errors
///
/// [`SolidError::Unported`] from the geometry copies (`SmCone::IsNurbSurfaceCone`,
/// `surface.cone_p2`, when analytics are added) and those of the copies (`SER`);
/// `SM_ERR_NULL_POINTER` for a UV trim curve that is not a kind of `SmBSplineCurve`
/// (`NER` after `SM_CAST_PTR`); [`SolidError::NotFound`] / [`SolidError::InvalidInput`]
/// for a dangling id or a null pointer upstream dereferences, `InvalidInput` for a
/// caller key in the kernel's reserved range.
#[allow(clippy::too_many_arguments)]
pub fn merge_brep(
    k: &mut Kernel,
    env: &mut NotifyEnv<'_>,
    b: &mut Brep,
    other: &Brep,
    source_key: &dyn Fn(EntityRef) -> ObjectRef,
    copy_map: Option<&mut CopyBrepMap<EntityRef>>,
    reverse_map: Option<&mut CopyBrepMap<EntityRef>>,
    add_analytics: bool,
) -> Result<()> {
    merge_brep_with_context(
        k,
        env,
        b,
        other,
        CopySource {
            key_of: source_key,
            context: None,
        },
        copy_map,
        reverse_map,
        add_analytics,
    )
}

/// [`merge_brep`] with source callbacks resolved independently of the destination.
#[allow(clippy::too_many_arguments)]
pub fn merge_brep_with_context(
    k: &mut Kernel,
    env: &mut NotifyEnv<'_>,
    b: &mut Brep,
    other: &Brep,
    source: CopySource<'_>,
    copy_map: Option<&mut CopyBrepMap<EntityRef>>,
    reverse_map: Option<&mut CopyBrepMap<EntityRef>>,
    add_analytics: bool,
) -> Result<()> {
    with_caller_identities(k, b, env, Some((other, source.key_of)), |k, b, env| {
        merge_brep_in(
            k,
            env,
            b,
            other,
            source,
            copy_map,
            reverse_map,
            add_analytics,
        )
    })
}

/// [`merge_brep`] once the identities are checked and seeded.
#[allow(clippy::too_many_lines, clippy::too_many_arguments)]
fn merge_brep_in(
    k: &Kernel,
    env: &mut NotifyEnv<'_>,
    b: &mut Brep,
    other: &Brep,
    mut source: CopySource<'_>,
    mut copy_map: Option<&mut CopyBrepMap<EntityRef>>,
    mut reverse_map: Option<&mut CopyBrepMap<EntityRef>>,
    add_analytics: bool,
) -> Result<()> {
    let source_key = source.key_of;
    let mut map = p1::ObjectMap::new();
    let set = |orig: EntityRef,
               copy: EntityRef,
               map: &mut p1::ObjectMap,
               cm: &mut Option<&mut CopyBrepMap<EntityRef>>,
               rm: &mut Option<&mut CopyBrepMap<EntityRef>>| {
        p1::set_object_map(orig, Some(copy), map, cm.as_deref_mut(), rm.as_deref_mut());
    };

    // We need to copy everything except the infinite region.
    let my_inf = b.infinite_region();
    let other_inf = other.infinite_region();

    // process Regions and shells
    // Map the infintite regions of the Breps.
    if let Some(o) = other_inf {
        p1::set_object_map(
            EntityRef::Region(o),
            my_inf.map(EntityRef::Region),
            &mut map,
            copy_map.as_deref_mut(),
            reverse_map.as_deref_mut(),
        );
    }
    let regions = p1::regions(other, None)?;
    for &r in &regions {
        let new_r = if let Some(x) = mapped(EntityRef::Region(r), &map, as_region) {
            x
        } else {
            let x = b.add_region_record(Region::default())?;
            set(
                EntityRef::Region(r),
                EntityRef::Region(x),
                &mut map,
                &mut copy_map,
                &mut reverse_map,
            );
            b.post_insert_region(x)?;
            x
        };

        // copy IsVoid state from old to new region
        copy_region_void_flag(b, new_r, other, r)?;

        // for every ThisRegion->shell
        for s in region_shells(other, r)? {
            let new_s = b.add_shell_record(crate::topology::Shell {
                region: Some(new_r),
                ..crate::topology::Shell::default()
            })?;
            set(
                EntityRef::Shell(s),
                EntityRef::Shell(new_s),
                &mut map,
                &mut copy_map,
                &mut reverse_map,
            );
            // set up new Shell (SE upstream: PostInsert cannot fail there; here only on
            // a corrupt list)
            shell::region_post_insert_shell(b, new_r, new_s)?;
            let ty = other.shell(s)?.shell_type;
            set_shell_type(b, new_s, ty)?;
        }
    }

    // for faces and their loops
    let faces = p1::faces(other, None)?;
    for &f in &faces {
        let fu1 = other
            .face(f)?
            .faceuse
            .ok_or(null_deref("face without face use"))?;
        let fu2 = other
            .faceuse(fu1)?
            .mate
            .ok_or(null_deref("face use without mate"))?;

        // Allocate new face and faceuses
        let new_f = b.add_face_record(crate::topology::Face::new(ShellId::from_index(0)))?;
        let new_fu1 = b.add_faceuse_record(crate::topology::Faceuse::default())?;
        let new_fu2 = b.add_faceuse_record(crate::topology::Faceuse::default())?;
        // (add_face_record joined the face list; upstream's PostInsert below)
        b.remove_from_face_list(new_f);

        // Relate the Faces and Faceuses.
        set(
            EntityRef::Face(f),
            EntityRef::Face(new_f),
            &mut map,
            &mut copy_map,
            &mut reverse_map,
        );
        set(
            EntityRef::Faceuse(fu1),
            EntityRef::Faceuse(new_fu1),
            &mut map,
            &mut copy_map,
            &mut reverse_map,
        );
        set(
            EntityRef::Faceuse(fu2),
            EntityRef::Faceuse(new_fu2),
            &mut map,
            &mut copy_map,
            &mut reverse_map,
        );

        // Set Copy Face data
        let src_face = other.face(f)?;
        let face_surf = src_face
            .surface
            .as_ref()
            .ok_or(null_deref("face without surface"))?;
        let mut new_surface = if add_analytics {
            face_surf.copy_and_add_analytics(k)?
        } else {
            face_surf.copy(k)?
        };
        let owner = (env.key_of)(EntityRef::Face(new_f));
        *new_surface.owner_mut() = Some(owner);
        let (rect, dom, ftol) = (
            src_face.rectangular_trim,
            src_face.uv_domain,
            src_face.tolerance,
        );
        {
            let nf = b.face_mut(new_f)?;
            nf.rectangular_trim = rect;
            nf.surface = Some(new_surface);
            nf.uv_domain = dom;
            nf.tolerance = ftol;
        }

        b.geometry_owner_keys.insert(EntityRef::Face(new_f), owner);
        b.sync_face_surface_tolerance(new_f)?;

        // add newFace to target Brep FaceList
        b.face_list.push(new_f);

        // Connect up face and faceuses and set FU fields
        let (o1, o2) = (
            other.faceuse(fu1)?.orientation,
            other.faceuse(fu2)?.orientation,
        );
        {
            let x = b.faceuse_mut(new_fu1)?;
            x.face = Some(new_f);
            x.mate = Some(new_fu2);
            x.orientation = o1;
        }
        {
            let x = b.faceuse_mut(new_fu2)?;
            x.face = Some(new_f);
            x.mate = Some(new_fu1);
            x.orientation = o2;
        }
        b.face_mut(new_f)?.faceuse = Some(if o1 == OrientType::Same {
            new_fu1 // FU1 is primary faceuse
        } else {
            new_fu2 // FU2 is primary faceuse
        });

        // for every face->loop
        for l in face2::loops(other, f, false, None)? {
            let lu1_ = other
                .loop_(l)?
                .loopuse
                .ok_or(null_deref("loop without loop use"))?;
            let lu2_ = other
                .loopuse(lu1_)?
                .mate
                .ok_or(null_deref("loop use without mate"))?;

            // Now allocate new loop and loopuses
            let new_l = b.add_loop_record(crate::topology::Loop {
                face: new_f,
                edges: Vec::new(),
                loopuse: None,
            })?;
            let new_lu1 = b.add_loopuse_record(crate::topology::Loopuse::default())?;
            let new_lu2 = b.add_loopuse_record(crate::topology::Loopuse::default())?;

            // Relate Loops and Loopuses.
            set(
                EntityRef::Loop(l),
                EntityRef::Loop(new_l),
                &mut map,
                &mut copy_map,
                &mut reverse_map,
            );
            set(
                EntityRef::Loopuse(lu1_),
                EntityRef::Loopuse(new_lu1),
                &mut map,
                &mut copy_map,
                &mut reverse_map,
            );
            set(
                EntityRef::Loopuse(lu2_),
                EntityRef::Loopuse(new_lu2),
                &mut map,
                &mut copy_map,
                &mut reverse_map,
            );

            // Connect up faceuses and loopuses and loops
            b.loop_mut(new_l)?.loopuse = Some(if o1 == OrientType::Same {
                new_lu1
            } else {
                new_lu2
            });
            b.face_mut(new_f)?.loops.push(new_l);
            let (s1, s2) = (other.loopuse(lu1_)?, other.loopuse(lu2_)?);
            let start_kind = |s: LoopuseStart| match s {
                LoopuseStart::Unknown => LoopuseStart::Unknown,
                LoopuseStart::Edgeuse(_) => LoopuseStart::Edgeuse(None),
                LoopuseStart::Vertexuse(_) => LoopuseStart::Vertexuse(None),
            };
            let (or1, st1, or2, st2) = (
                s1.orientation,
                start_kind(s1.start),
                s2.orientation,
                start_kind(s2.start),
            );
            {
                let x = b.loopuse_mut(new_lu1)?;
                x.loop_ = Some(new_l);
                x.mate = Some(new_lu2);
                x.faceuse = Some(new_fu1);
                x.orientation = or1;
                x.start = st1;
            }
            {
                let x = b.loopuse_mut(new_lu2)?;
                x.loop_ = Some(new_l);
                x.mate = Some(new_lu1);
                x.faceuse = Some(new_fu2);
                x.orientation = or2;
                x.start = st2;
            }
            b.faceuse_mut(new_fu1)?.loopuses.push(new_lu1);
            b.faceuse_mut(new_fu2)?.loopuses.push(new_lu2);
        }
    }

    // for edges and their edgeuses
    let edges = p1::edges(other, None)?;
    for &e in &edges {
        let src_e = other.edge(e)?;
        let curve = src_e
            .curve
            .as_ref()
            .ok_or(null_deref("edge without curve"))?;
        let mut new_c = curve.copy(k)?;
        let (ivl, etol) = (src_e.interval, src_e.tolerance);
        let new_e = b.add_edge_record(crate::topology::Edge {
            vertices: [VertexId::from_index(0); 2],
            edgeuses: Vec::new(),
            interval: ivl,
            tolerance: etol,
            curve: None,
        })?;
        let owner = (env.key_of)(EntityRef::Edge(new_e));
        let _ = new_c.set_owner(Some(owner), None);
        b.edge_mut(new_e)?.curve = Some(new_c);
        b.geometry_owner_keys.insert(EntityRef::Edge(new_e), owner);

        // Relate the Edges.
        set(
            EntityRef::Edge(e),
            EntityRef::Edge(new_e),
            &mut map,
            &mut copy_map,
            &mut reverse_map,
        );

        // Now do edgeuses
        for eu in edge::edgeuses(other, e, None, None)? {
            let src_eu = other.edgeuse(eu)?;
            let owner = match src_eu.owner {
                EdgeuseOwner::Unknown => EdgeuseOwner::Unknown,
                EdgeuseOwner::Loopuse(x) => EdgeuseOwner::Loopuse(
                    x.and_then(|x| mapped(EntityRef::Loopuse(x), &map, as_loopuse)),
                ),
                EdgeuseOwner::Shell(x) => {
                    EdgeuseOwner::Shell(x.and_then(|x| mapped(EntityRef::Shell(x), &map, as_shell)))
                }
            };
            let uv = match &src_eu.uv_trim_curve {
                Some(c) => {
                    let new_curve = c.copy(k)?;
                    // m_pUVTrimCurve = SM_CAST_PTR(SmBSplineCurve, pNewCurve); NER(..)
                    if curve_bspline_base(&new_curve).is_none() {
                        return Err(ner());
                    }
                    Some(new_curve)
                }
                None => None,
            };
            let orientation = src_eu.orientation;
            let new_eu = b.add_edgeuse_record(crate::topology::Edgeuse {
                edge: Some(new_e),
                owner,
                orientation,
                ..crate::topology::Edgeuse::default()
            })?;
            // pNewEU->m_pUVTrimCurve = pNewCurve; pNewCurve->SetOwner(pNewEU): the copy's
            // edge use is named by the caller's key, recorded so trims made later
            // (`SetUVTrimCurve`) are named alike
            let eu_owner = (env.key_of)(EntityRef::Edgeuse(new_eu));
            b.geometry_owner_keys
                .insert(EntityRef::Edgeuse(new_eu), eu_owner);
            if let Some(mut c) = uv {
                let _ = c.set_owner(Some(eu_owner), None);
                b.edgeuse_mut(new_eu)?.uv_trim_curve = Some(c);
            }

            // add copied edgeuse to pNewE within the Brep
            b.edge_mut(new_e)?.edgeuses.push(new_eu);

            // Relate the Edgeuses.
            set(
                EntityRef::Edgeuse(eu),
                EntityRef::Edgeuse(new_eu),
                &mut map,
                &mut copy_map,
                &mut reverse_map,
            );
        }
    }

    // for vertices and their vertexuses
    let vertices = p1::vertices(other, None)?;
    for &v in &vertices {
        let src_v = other.vertex(v)?;
        let (point, vtol) = (src_v.point, src_v.tolerance);
        let new_v = b.add_vertex_record(crate::topology::Vertex::default())?;

        // set copy vertex values (SetPoint notifies a vertex without uses: nothing)
        b.vertex_mut(new_v)?.point = point;
        b.vertex_mut(new_v)?.tolerance = vtol;

        // Relate the Vertices.
        set(
            EntityRef::Vertex(v),
            EntityRef::Vertex(new_v),
            &mut map,
            &mut copy_map,
            &mut reverse_map,
        );

        // for every vertex->vertexuse
        for vu in vertex::vertexuses(other, v)? {
            let new_vu = b.add_vertexuse_record(Vertexuse {
                vertex: Some(new_v),
                ..Vertexuse::default()
            })?;
            b.vertex_mut(new_v)?.vertexuses.push(new_vu);

            // Relate the Vertexuses.
            set(
                EntityRef::Vertexuse(vu),
                EntityRef::Vertexuse(new_vu),
                &mut map,
                &mut copy_map,
                &mut reverse_map,
            );

            // set copied vertexuse values
            let owner = match other.vertexuse(vu)?.owner {
                VertexuseOwner::Unknown => VertexuseOwner::Unknown,
                VertexuseOwner::Edgeuse(x) => VertexuseOwner::Edgeuse(
                    x.and_then(|x| mapped(EntityRef::Edgeuse(x), &map, as_edgeuse)),
                ),
                VertexuseOwner::Loopuse(x) => VertexuseOwner::Loopuse(
                    x.and_then(|x| mapped(EntityRef::Loopuse(x), &map, as_loopuse)),
                ),
                VertexuseOwner::Shell(x) => VertexuseOwner::Shell(
                    x.and_then(|x| mapped(EntityRef::Shell(x), &map, as_shell)),
                ),
            };
            b.vertexuse_mut(new_vu)?.owner = owner;
        }
    }

    // Second pass of copying: the non-hierarchial relationships.
    for &r in &regions {
        for s in region_shells(other, r)? {
            let new_s = mapped(EntityRef::Shell(s), &map, as_shell);
            let Some(new_s) = new_s else { continue };
            // pNewS->m_pList = map(pS->m_pList), m_lListSize copied (an empty list is
            // skipped upstream; an unmapped head is NULL). A face-use list's other
            // members follow from the copied face-use links, so they are the mapped
            // members in the same order.
            let new_list = match &other.shell(s)?.members {
                ShellMembers::Empty => continue,
                ShellMembers::Edgeuse(x) => mapped(EntityRef::Edgeuse(*x), &map, as_edgeuse)
                    .map_or(ShellMembers::Empty, ShellMembers::Edgeuse),
                ShellMembers::Vertexuse(x) => mapped(EntityRef::Vertexuse(*x), &map, as_vertexuse)
                    .map_or(ShellMembers::Empty, ShellMembers::Vertexuse),
                ShellMembers::Faceuses(list) => {
                    let head = list.first().copied();
                    if head
                        .and_then(|x| mapped(EntityRef::Faceuse(x), &map, as_faceuse))
                        .is_none()
                    {
                        ShellMembers::Empty
                    } else {
                        let mut members = Vec::new();
                        for &x in list {
                            members.push(
                                mapped(EntityRef::Faceuse(x), &map, as_faceuse)
                                    .ok_or(null_deref("unmapped face use"))?,
                            );
                        }
                        ShellMembers::Faceuses(members)
                    }
                }
            };
            set_shell_head(b, new_s, new_list)?;
        }
    }

    // for faces and loops
    for &f in &faces {
        let fu1 = other
            .face(f)?
            .faceuse
            .ok_or(null_deref("face without face use"))?;
        let fu2 = other
            .faceuse(fu1)?
            .mate
            .ok_or(null_deref("face use without mate"))?;
        let new_face = mapped(EntityRef::Face(f), &map, |x| match x {
            EntityRef::Face(x) => Some(x),
            _ => None,
        })
        .ok_or(null_deref("unmapped face"))?;
        for (from, to) in [
            (fu1, mapped(EntityRef::Faceuse(fu1), &map, as_faceuse)),
            (fu2, mapped(EntityRef::Faceuse(fu2), &map, as_faceuse)),
        ] {
            let to = to.ok_or(null_deref("unmapped face use"))?;
            // set newFU ListOwner, Next, and Last Value
            let shell = other
                .faceuse(from)?
                .shell
                .and_then(|s| mapped(EntityRef::Shell(s), &map, as_shell));
            b.faceuse_mut(to)?.shell = shell;
            copy_faceuse_links(b, to, other, from, &map)?;
            // (the face record's shell: GetShell is the primary face use's list owner)
            if b.face(new_face)?.faceuse == Some(to) {
                if let Some(s) = shell {
                    b.face_mut(new_face)?.shell = s;
                }
            }
        }

        // for every loop
        for l in face2::loops(other, f, false, None)? {
            let lu1_ = other
                .loop_(l)?
                .loopuse
                .ok_or(null_deref("loop without loop use"))?;
            let lu2_ = other
                .loopuse(lu1_)?
                .mate
                .ok_or(null_deref("loop use without mate"))?;
            for from in [lu1_, lu2_] {
                let to = mapped(EntityRef::Loopuse(from), &map, as_loopuse)
                    .ok_or(null_deref("unmapped loop use"))?;
                let start = match other.loopuse(from)?.start {
                    LoopuseStart::Unknown => LoopuseStart::Unknown,
                    LoopuseStart::Edgeuse(x) => LoopuseStart::Edgeuse(
                        x.and_then(|x| mapped(EntityRef::Edgeuse(x), &map, as_edgeuse)),
                    ),
                    LoopuseStart::Vertexuse(x) => LoopuseStart::Vertexuse(
                        x.and_then(|x| mapped(EntityRef::Vertexuse(x), &map, as_vertexuse)),
                    ),
                };
                b.loopuse_mut(to)?.start = start;
            }
        }
    }

    // edges and edgeuse
    for &e in &edges {
        for eu in edge::edgeuses(other, e, None, None)? {
            let new_eu = mapped(EntityRef::Edgeuse(eu), &map, as_edgeuse)
                .ok_or(null_deref("unmapped edge use"))?;
            let src = other.edgeuse(eu)?;
            let (ccw, cw) = if src.ccw.is_some() {
                (
                    src.ccw
                        .and_then(|x| mapped(EntityRef::Edgeuse(x), &map, as_edgeuse)),
                    src.cw
                        .and_then(|x| mapped(EntityRef::Edgeuse(x), &map, as_edgeuse)),
                )
            } else {
                // Wire edge
                (None, None)
            };
            let vu = src
                .vertexuse
                .and_then(|x| mapped(EntityRef::Vertexuse(x), &map, as_vertexuse));
            let x = b.edgeuse_mut(new_eu)?;
            x.ccw = ccw;
            x.cw = cw;
            x.vertexuse = vu;
        }
        // the edge record's vertex pair (`GetVertex` reads it through the edge uses)
        let new_e = mapped(EntityRef::Edge(e), &map, |x| match x {
            EntityRef::Edge(x) => Some(x),
            _ => None,
        })
        .ok_or(null_deref("unmapped edge"))?;
        // Upstream has no such pair: it copies the edge and vertex use links above and
        // reads the ends through them. Take the source's ends the same way (its cached
        // pair is not maintained by topology edits, so it can be stale on a Boolean's
        // result). An edge without edge uses, or whose uses do not name both ends, keeps
        // its cached pair, so the copy never fails where upstream's does not.
        let [cached_start, cached_end] = other.edge(e)?.vertices;
        let [a, c] = match edge::start_end_vertices(other, e) {
            Ok((start, end)) => [start.unwrap_or(cached_start), end.unwrap_or(cached_end)],
            Err(_) => [cached_start, cached_end],
        };
        let va = mapped(EntityRef::Vertex(a), &map, |x| match x {
            EntityRef::Vertex(x) => Some(x),
            _ => None,
        });
        let vc = mapped(EntityRef::Vertex(c), &map, |x| match x {
            EntityRef::Vertex(x) => Some(x),
            _ => None,
        });
        if let (Some(va), Some(vc)) = (va, vc) {
            b.edge_mut(new_e)?.vertices = [va, vc];
        }
    }

    // Third pass of copying: Notify on all new topology.
    // (SM_NO_GET_BREP(..) is ((SmObject*)(0)) in the pinned SmTypes.h)
    let mut copy_notify = |env: &mut NotifyEnv<'_>, from: EntityRef, to: Option<EntityRef>| {
        let to = to.map(|t| (env.key_of)(t));
        notify_copy_with_context(
            env,
            source.context.as_deref_mut(),
            source_key(from),
            to,
            None,
            None,
        )
    };
    for &r in &regions {
        for s in region_shells(other, r)? {
            copy_notify(
                env,
                EntityRef::Shell(s),
                p1::get_object_map(EntityRef::Shell(s), &map),
            )?;
        }
        copy_notify(
            env,
            EntityRef::Region(r),
            p1::get_object_map(EntityRef::Region(r), &map),
        )?;
    }
    for &f in &faces {
        let fu1 = other
            .face(f)?
            .faceuse
            .ok_or(null_deref("face without face use"))?;
        let fu2 = other
            .faceuse(fu1)?
            .mate
            .ok_or(null_deref("face use without mate"))?;
        for l in face2::loops(other, f, false, None)? {
            let lu1_ = other
                .loop_(l)?
                .loopuse
                .ok_or(null_deref("loop without loop use"))?;
            let lu2_ = other
                .loopuse(lu1_)?
                .mate
                .ok_or(null_deref("loop use without mate"))?;
            for x in [
                EntityRef::Loop(l),
                EntityRef::Loopuse(lu1_),
                EntityRef::Loopuse(lu2_),
            ] {
                copy_notify(env, x, p1::get_object_map(x, &map))?;
            }
        }
        for x in [
            EntityRef::Face(f),
            EntityRef::Faceuse(fu1),
            EntityRef::Faceuse(fu2),
        ] {
            copy_notify(env, x, p1::get_object_map(x, &map))?;
        }
    }
    for &e in &edges {
        for eu in edge::edgeuses(other, e, None, None)? {
            let x = EntityRef::Edgeuse(eu);
            copy_notify(env, x, p1::get_object_map(x, &map))?;
        }
        let x = EntityRef::Edge(e);
        copy_notify(env, x, p1::get_object_map(x, &map))?;
    }
    for &v in &vertices {
        for vu in vertex::vertexuses(other, v)? {
            let x = EntityRef::Vertexuse(vu);
            copy_notify(env, x, p1::get_object_map(x, &map))?;
        }
        let x = EntityRef::Vertex(v);
        copy_notify(env, x, p1::get_object_map(x, &map))?;
    }
    Ok(())
}

// ---- DeleteFace ---------------------------------------------------------------------------------

/// The optional arguments of [`delete_face`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeleteFaceOptions {
    /// `bDeleteEdgesVertices` (default `FALSE`): also delete the edges and vertices
    /// left hanging (wire edges, shell vertices) once the face is gone.
    pub delete_edges_vertices: bool,
    /// `bDoRegionNesting` (default `TRUE`): combine regions and shells as needed.
    pub do_region_nesting: bool,
    /// `pOptRegionToKeep` (default `NULL`): the region kept when two merge.
    pub region_to_keep: Option<RegionId>,
}

impl Default for DeleteFaceOptions {
    fn default() -> Self {
        Self {
            delete_edges_vertices: false,
            do_region_nesting: true,
            region_to_keep: None,
        }
    }
}

/// `DeleteFace`: removes face `f` and its loops and uses from the B-rep. Its lamina
/// edges become wire edges, its other edges lose this face's edge uses; with region
/// nesting the regions on either side merge (keeping the region to keep, else the
/// infinite or void one) and the remaining topology of the shell is split into
/// connected shells. With `delete_edges_vertices` the edges that became wires and the
/// vertices that became shell vertices are deleted too; they are returned
/// (`pOptDeletedEdgesVertices`, stale upstream).
///
/// # Errors
///
/// `SM_ERR` when editing is disabled, the face is not in the B-rep, or the topology is
/// inconsistent; `SM_ERR_NULL_POINTER` for a missing face use, shell, edge use mate,
/// vertex use or vertex (`NER`); those of `SmFace::Notify`
/// ([`crate::topology::face::p3::notify`]); [`SolidError::Unported`] for every face in
/// the B-rep: `SmFace::GetVertices` (`topology.face_p4`), `DeleteEdge`/`DeleteVertex`
/// (`topology.brep_p4`);
/// `InvalidInput` for a caller key in the kernel's reserved range.
pub fn delete_face(
    k: &mut Kernel,
    b: &mut Brep,
    env: &mut NotifyEnv<'_>,
    marks: &mut Marks,
    f: FaceId,
    options: DeleteFaceOptions,
) -> Result<Vec<EntityRef>> {
    with_caller_identities(k, b, env, None, |k, b, env| {
        delete_face_in(
            k,
            b,
            env,
            &mut DeletionMarks::Indexed(marks),
            f,
            options,
            None,
        )
    })
}

/// Delete a face using the actual notification records for traversal marks and
/// an explicitly issued B-rep notification identity.
///
/// # Errors
/// Those of [`delete_face`]. Earlier edits and retained marks remain on failure.
pub fn delete_face_with_body_identity(
    k: &mut Kernel,
    b: &mut Brep,
    env: &mut NotifyEnv<'_>,
    f: FaceId,
    options: DeleteFaceOptions,
    body: ObjectRef,
) -> Result<Vec<EntityRef>> {
    with_caller_identities(k, b, env, None, |k, b, env| {
        delete_face_in(
            k,
            b,
            env,
            &mut DeletionMarks::Retained,
            f,
            options,
            Some(body),
        )
    })
}

/// [`delete_face`] once the identities are checked and seeded ([`with_caller_identities`]).
#[allow(clippy::too_many_lines)]
fn delete_face_in(
    k: &mut Kernel,
    b: &mut Brep,
    env: &mut NotifyEnv<'_>,
    marks: &mut DeletionMarks<'_>,
    f: FaceId,
    options: DeleteFaceOptions,
    body: Option<ObjectRef>,
) -> Result<Vec<EntityRef>> {
    let DeleteFaceOptions {
        delete_edges_vertices,
        do_region_nesting,
        region_to_keep,
    } = options;
    // init output
    let mut deleted = Vec::new();

    // check state - m_bEditingEnabled must be on
    if !b.editing_enabled() {
        return Err(ser());
    }

    // Check if face is in brep
    if !p1::faces(b, None)?.contains(&f) {
        return Err(ser());
    }

    // tell Brep it's about to change
    ignore_soft(brep_notify_with_identity(
        b,
        env,
        NotifyOperation::PreEdit,
        [
            body.map(NotifyArg::Object).or_else(|| brep_arg(b)),
            None,
            None,
        ],
        body,
    ))?;
    ignore_soft(face_notify(
        b,
        f,
        env,
        NotifyOperation::PreEdit,
        [arg(EntityRef::Face(f)), None, None],
    ))?;
    ignore_soft(brep_notify_with_identity(
        b,
        env,
        NotifyOperation::RmFromBrep,
        [
            arg(EntityRef::Face(f)),
            body.map(NotifyArg::Object).or_else(|| brep_arg(b)),
            None,
        ],
        body,
    ))?;

    // Get all vertices for later new shell detection
    let mut verts: Vec<Option<VertexId>> = face_vertices(b, f)?.into_iter().map(Some).collect();
    let orig_vertices: Vec<VertexId> = verts.iter().flatten().copied().collect();

    let fu = face2::upward_faceuse(b, f)?.ok_or_else(ner)?;
    let other_fu = eu1::faceuse_mate(b, fu)?.ok_or(null_deref("face use without mate"))?;

    let mut shell = eu1::faceuse_shell(b, fu)?.ok_or_else(ner)?;
    let mut other_shell = eu1::faceuse_shell(b, other_fu)?;

    // Take care of the case where we have two Regions being combined as a result of
    // this operation.
    if do_region_nesting && Some(shell) != other_shell {
        let os = other_shell.ok_or(null_deref("face use without shell"))?;
        let other_region = eu1::shell_region(b, os)?;
        if let Some(keep) = region_to_keep {
            if Some(keep) == other_region {
                // Swap shells -- keep the specified Region
                shell = eu1::faceuse_shell(b, other_fu)?.ok_or_else(ner)?;
                other_shell = Some(eu1::faceuse_shell(b, fu)?.ok_or_else(ner)?);
            }
        } else {
            let other_r = other_region.ok_or(null_deref("shell without region"))?;
            if other_region == b.infinite_region()
                || (region_is_void(b, other_r)?
                    && eu1::shell_region(b, shell)? != b.infinite_region())
            {
                // Swap shells -- keep the infinite (or void) region.
                shell = eu1::faceuse_shell(b, other_fu)?.ok_or_else(ner)?;
                other_shell = Some(eu1::faceuse_shell(b, fu)?.ok_or_else(ner)?);
            }
        }
        let os = other_shell.ok_or(null_deref("face use without shell"))?;

        // Transfer all stuff from other shell to shell
        tt::remove_shell(k, b, os, shell)?;

        // Now pOtherShell should be empty - remove it from region and delete it.
        let surviving = eu1::shell_region(b, shell)?.ok_or(null_deref("shell without region"))?;
        let del_region = eu1::shell_region(b, os)?.ok_or(null_deref("shell without region"))?;
        if surviving == del_region {
            return Err(ser()); // Should not happen
        }
        ignore_soft(shell::region_remove_shell(b, del_region, os))?;
        shell::destroy(k, b, os, env)?;
        other_shell = None;
        for s in region_shells(b, del_region)? {
            ignore_soft(shell::region_remove_shell(b, del_region, s))?;
            ignore_soft(shell::region_post_insert_shell(b, surviving, s))?;
        }
        if Some(del_region) == b.infinite_region() {
            b.set_infinite_region(Some(surviving));
            region_set_is_void(b, surviving, TRUE)?;
        }

        // allow attributes to propagate and inform the public of Region merges
        ignore_soft(brep_notify_with_identity(
            b,
            env,
            NotifyOperation::MergeInBrep,
            [
                arg(EntityRef::Region(surviving)),
                arg(EntityRef::Region(del_region)),
                body.map(NotifyArg::Object).or_else(|| brep_arg(b)),
            ],
            body,
        ))?;

        // Remove and Delete the DelRegion
        ignore_soft(b.remove_region_from_list(del_region))?;
    }

    let mut new_wire_edges: Vec<EdgeId> = Vec::new();
    let mut any_edgeuse: Option<EdgeuseId> = None;
    let mut new_shell_vertex_count = 0usize;

    // for every loop in FaceToBeDeleted - remove use connections to vertices and edges
    // then delete the loop and its loopuses
    for lu in eu1::faceuse_loopuses(b, fu, false, None)? {
        let other_lu = lu1::other_loopuse(b, lu)?.ok_or(null_deref("loop use without mate"))?;
        let l = lu1::loop_(b, lu)?;

        if lu1::is_vertex_loopuse(b, lu)? {
            // Single vertex loop - either remove vertexuses belonging to loops or change
            // them to shell vertexuses.
            let vu = lu1::vertexuse(b, lu)?.ok_or_else(ner)?;
            let v = vertexuse::vertex(b, vu)?.ok_or_else(ner)?;
            let other_vu = lu1::vertexuse(b, other_lu)?.ok_or_else(ner)?;
            let n_vus = vertex::vertexuses(b, v)?.len();
            // Remove vertexuses from the vertex.
            vertex_remove_vertexuse(b, v, other_vu)?;
            vertex_remove_vertexuse(b, v, vu)?;
            if n_vus == 2 {
                // If there were only two VU - convert vertex to a shell vertex
                let new_s = b.add_shell_record(crate::topology::Shell::default())?;
                let r = eu1::shell_region(b, shell)?.ok_or(null_deref("shell without region"))?;
                b.shell_mut(new_s)?.region = Some(r);
                shell::region_post_insert_shell(b, r, new_s)?;
                b.vertex_mut(v)?.vertexuses.insert(0, vu);

                // Link up shell and vertexuse
                set_shell_type(b, new_s, ShellType::Vertex)?;
                set_shell_head(b, new_s, ShellMembers::Vertexuse(vu))?;
                b.vertexuse_mut(vu)?.owner = VertexuseOwner::Shell(Some(new_s));

                // count the number of shell vertics made
                new_shell_vertex_count += 1;
            }
        } else {
            for eu in lu1::edgeuses(b, lu, None, None)? {
                if any_edgeuse.is_none() {
                    any_edgeuse = Some(eu);
                }
                let mate = eu1::mate(b, eu)?.ok_or_else(ner)?;
                let e = eu1::edge(b, eu)?.ok_or_else(ner)?;
                if eu1::face(b, mate)? != eu1::face(b, eu)? {
                    return Err(ser());
                }

                if edge::is_lamina(b, e)? {
                    // Don't delete edgeuses convert them to wire edge edgeuses
                    for x in [eu, mate] {
                        let r = b.edgeuse_mut(x)?;
                        r.owner = EdgeuseOwner::Shell(Some(shell));
                        r.ccw = None;
                        r.cw = None;
                    }
                    // TRUE = delete existing UVTrimCurve
                    ignore_soft(eu1::set_uv_trim_curve(
                        b,
                        eu,
                        eu1::UvTrimCurveArg::Null,
                        0.0,
                        true,
                        None,
                    ))?;

                    // remember the chanded edges
                    new_wire_edges.push(e);
                } else {
                    // Easy just remove and delete edgeuses/vertexuses from the list
                    edge_remove_edgeuse(b, e, eu)?;
                    edge_remove_edgeuse(b, e, mate)?;
                    let (vu1, v1) = eu_vertex(b, eu)?;
                    vertex_remove_vertexuse(b, v1, vu1)?;
                    let (vu2, v2) = eu_vertex(b, mate)?;
                    vertex_remove_vertexuse(b, v2, vu2)?;
                    // delete pEU; delete pEUMate (the vertex uses' destructors notify
                    // nothing)
                    crate::topology::edgeuse::p2::destroy(k, b, eu, env)?;
                    crate::topology::edgeuse::p2::destroy(k, b, mate, env)?;
                    if any_edgeuse == Some(eu) {
                        any_edgeuse = None;
                    }
                }
            }
        }

        // The target loop and its loopuses are no longer connected to any edges or
        // vertices: remove the loopuse connections from the faceuses.
        if let Some(l) = l {
            b.face_mut(f)?.loops.retain(|&x| x != l);
        }
        let lfu = lu1::faceuse(b, lu)?.ok_or(null_deref("loop use without face use"))?;
        faceuse_remove_loopuse(b, lfu, lu)?;
        let ofu = lu1::faceuse(b, other_lu)?.ok_or(null_deref("loop use without face use"))?;
        faceuse_remove_loopuse(b, ofu, other_lu)?;
    }

    // Take care of face and faceuses - remove from lists
    ignore_soft(shell::shell_remove_faceuse(b, shell, fu))?;
    if let Some(os) = other_shell {
        ignore_soft(shell::shell_remove_faceuse(b, os, other_fu))?;
    }

    // Don't allow stale pointers to remain in the Faces.
    for x in [fu, other_fu] {
        if let Some(face) = eu1::faceuse_face(b, x)? {
            face2::set_faceuse(b, face, None)?;
        }
    }

    // delete pFU; delete pOtherFU
    delete_faceuses(b, [fu, other_fu])?;

    if do_region_nesting {
        // If there are no vertices and no edges then the entire shell goes away.
        let mut vertex_count = verts.len();
        let mut lock = MarkLock::new(core::slice::from_mut(&mut k.context), 0, ALL_MARKS)?;
        let res = delete_face_nesting(
            k,
            b,
            env,
            marks,
            &mut lock,
            shell,
            any_edgeuse,
            &mut verts,
            &mut vertex_count,
            new_shell_vertex_count,
        );
        lock.release(core::slice::from_mut(&mut k.context))?;
        res?;
    }

    // It's now OK to remove and delete the FaceToDelete
    destroy_face(b, env, f)?;

    if delete_edges_vertices {
        // delete every edge converted into a wire
        for e in new_wire_edges {
            deleted.push(EntityRef::Edge(e));
            ignore_soft(match body {
                Some(body) => super::p4::delete_edge_with_body_identity(
                    k,
                    b,
                    &mut super::p4::DeleteEnv::from_notify(env),
                    e,
                    None,
                    body,
                ),
                None => delete_edge(k, b, env, e, None),
            })?;
        }

        // remove vertices that have become shell vertices
        for v in orig_vertices {
            if vertex::is_shell_vertex(b, v)? {
                deleted.push(EntityRef::Vertex(v));
                ignore_soft(match body {
                    Some(body) => super::p4::delete_vertex_with_body_identity(
                        k,
                        b,
                        &mut super::p4::DeleteEnv::from_notify(env),
                        v,
                        body,
                    ),
                    None => delete_vertex(k, b, env, v),
                })?;
            }
        }
    }
    Ok(deleted)
}

/// The region nesting part of [`delete_face`] (under its mark lock): drops the shell
/// when nothing is left in it, else splits the topology left in it into connected
/// shells (the last new shell's children go back to the original shell).
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn delete_face_nesting(
    k: &mut Kernel,
    b: &mut Brep,
    env: &mut NotifyEnv<'_>,
    marks: &mut DeletionMarks<'_>,
    lock: &mut MarkLock,
    shell: ShellId,
    any_edgeuse: Option<EdgeuseId>,
    verts: &mut [Option<VertexId>],
    vertex_count: &mut usize,
    new_shell_vertex_count: usize,
) -> Result<()> {
    // (unsigned counts upstream)
    let remaining = |c: usize| c.wrapping_sub(new_shell_vertex_count);
    let mut vertices_needing_new_shell = 0usize;
    if any_edgeuse.is_none() && remaining(*vertex_count) == 0 {
        let r = eu1::shell_region(b, shell)?.ok_or(null_deref("shell without region"))?;
        ignore_soft(shell::region_remove_shell(b, r, shell))?;
        // delete pShell
        shell::destroy(k, b, shell, env)?;
    } else {
        // Vertex-only loops supply no boundary edgeuse. An attached wire
        // can nevertheless survive after the shell loses its last faceuse.
        let mut any_edgeuse = any_edgeuse;
        if shell::shell_list_size(b, shell)? == 0 && any_edgeuse.is_none() {
            'vertices: for v in verts.iter().flatten().copied() {
                for vu in vertex::vertexuses(b, v)? {
                    if !vertexuse::is_edge_vertexuse(b, vu)? {
                        continue;
                    }
                    let eu = vertexuse::edgeuse(b, vu)?
                        .ok_or(null_deref("edge vertex use without edge use"))?;
                    if eu1::is_shell_edgeuse(b, eu)? && eu1::shell(b, eu)? == Some(shell) {
                        any_edgeuse = Some(eu);
                        break 'vertices;
                    }
                }
            }
        }

        // See if shell is now a wire shell - if it is point it to an edgeuse
        if shell::shell_list_size(b, shell)? == 0 {
            if let Some(eu) = any_edgeuse {
                set_shell_head(b, shell, ShellMembers::Edgeuse(eu))?;
                set_shell_type(b, shell, ShellType::Wire)?;
            }
        }
        // (an empty list is only an SE(SM_ERR) upstream)

        // single/multiple shell check
        let mut need_traversal = true;
        for v in verts.iter().flatten().copied() {
            // subtract shell vertices - they already have been given their new shells
            if vertex::is_shell_vertex(b, v)? {
                *vertex_count = vertex_count.wrapping_sub(1);
                continue;
            }
            if need_traversal {
                marks.with_marks(env.records, env.key_of, |marks| {
                    tt::shell_traversal_with_marks(k, b, marks, v, shell, None, None, lock)
                })?;
                need_traversal = false;
            }
            // subtract marked vertices - they have a home in the original Shell
            if marks.with_marks(env.records, env.key_of, |marks| {
                marks.is_marked(k, Entity::Vertex(v), lock.mark_type())
            }) {
                *vertex_count = vertex_count.wrapping_sub(1);
            }
        }

        // The shell vertices counted in new_shell_vertex_count are already subtracted
        // above.
        vertices_needing_new_shell = *vertex_count;
    }

    // if any vertices no longer connect to pShell - need 1 or more new shells
    if vertices_needing_new_shell == 0 {
        return Ok(());
    }
    lock.new_mark(core::slice::from_mut(&mut k.context))?;
    let mut last_new_shell: Option<ShellId> = None;
    let mut count = verts.len();
    while count > 0 {
        // Now remove all vertices which have been touched by previous traversal.
        for slot in verts.iter_mut() {
            let Some(v) = *slot else { continue };
            if marks.with_marks(env.records, env.key_of, |marks| {
                marks.is_marked(k, Entity::Vertex(v), lock.mark_type())
            }) || vertex::is_shell_vertex(b, v)?
            {
                *slot = None;
                count -= 1;
            }
        }
        if count == 0 {
            break;
        }

        // Now try to find a vertex to traverse again.
        let Some(v) = verts.iter().flatten().copied().next() else {
            break;
        };
        let r = eu1::shell_region(b, shell)?.ok_or(null_deref("shell without region"))?;
        let new_shell = b.add_shell_record(crate::topology::Shell {
            region: Some(r),
            ..crate::topology::Shell::default()
        })?;
        last_new_shell = Some(new_shell);
        shell::region_post_insert_shell(b, r, new_shell)?;
        let mut faceuses = Vec::new();
        let mut edgeuses = Vec::new();
        marks.with_marks(env.records, env.key_of, |marks| {
            tt::shell_traversal_with_marks(
                k,
                b,
                marks,
                v,
                shell,
                Some(&mut edgeuses),
                Some(&mut faceuses),
                lock,
            )
        })?;

        if faceuses.is_empty() && !edgeuses.is_empty() {
            // Wire shell
            let eu = edgeuses.first().copied().ok_or_else(ser)?;
            set_shell_head(b, new_shell, ShellMembers::Edgeuse(eu))?;
            set_shell_type(b, new_shell, ShellType::Wire)?;
        } else if faceuses.is_empty() && edgeuses.is_empty() {
            // Vertex shell
            set_shell_type(b, new_shell, ShellType::Vertex)?;
            let vu = vertex::primary_vertexuse(b, v)?.ok_or(null_deref("vertex without use"))?;
            set_shell_head(b, new_shell, ShellMembers::Vertexuse(vu))?;
            b.vertexuse_mut(vu)?.owner = VertexuseOwner::Shell(Some(new_shell));
        } else {
            // Faceuse shell
            set_shell_type(b, new_shell, ShellType::Faceuse)?;
        }

        // Link all wire edges back to new shell
        for &eu in &edgeuses {
            if eu1::is_shell_edgeuse(b, eu)? {
                b.edgeuse_mut(eu)?.owner = EdgeuseOwner::Shell(Some(new_shell));
                // remember to remove the wire from pShell when appropriate (not with
                // Remove: m_pList = NULL, m_lListSize = 0)
                if b.shell(shell)?.is_wire_shell()
                    && b.shell(shell)?.members == ShellMembers::Edgeuse(eu)
                {
                    set_shell_head(b, shell, ShellMembers::Empty)?;
                }
            }
        }

        // Transfer faceuses from the old shell to the new shell
        for &x in &faceuses {
            ignore_soft(shell::shell_remove_faceuse(b, shell, x))?;
            shell::shell_post_insert_faceuse(b, new_shell, x)?;
        }
    }

    // If it's still a Faceuse Shell, pShell should be empty and pLastNewShell should
    // contain something.
    if b.shell(shell)?.is_faceuse_shell() {
        let ok = shell::shell_list_size(b, shell)? == 0
            && b.shell(shell)?.members == ShellMembers::Empty
            && match last_new_shell {
                Some(s) => b.shell(s)?.members != ShellMembers::Empty,
                None => false,
            };
        if !ok {
            return Err(ser());
        }
    }

    // rather than delete pShell - move entities in pLastNewShell to pShell and delete
    // pLastNewShell
    let last = last_new_shell.ok_or(null_deref("no new shell"))?;
    let r = eu1::shell_region(b, shell)?.ok_or(null_deref("shell without region"))?;
    ignore_soft(shell::transfer_children(k, b, last, shell, None))?;
    ignore_soft(shell::region_remove_shell(b, r, last))?;
    shell::destroy(k, b, last, env)
}

/// `delete pFU` for each face use (the arena keeps the record): `SmTopology`'s destructor
/// removes a member still in its owner's list (`m_pListOwner->Remove(this)`, status
/// ignored). After region nesting merged the two shells, the mate face use is in the
/// surviving shell, which upstream's explicit `Remove`s no longer reach.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; the corrupt-list errors of
/// [`shell::shell_remove_faceuse`].
fn delete_faceuses(b: &mut Brep, faceuses: [FaceuseId; 2]) -> Result<()> {
    for x in faceuses {
        if let Some(owner) = eu1::faceuse_shell(b, x)? {
            ignore_soft(shell::shell_remove_faceuse(b, owner, x))?;
        }
    }
    Ok(())
}

// ---- slit loops where solids touch (not upstream; issue #746) --------------------------------

/// The bound on [`split_disconnected_shells`]' splits of one shell (the fork's 1024 passes).
const MAX_SHELL_SPLITS: usize = 1024;

/// Not upstream (source issue #746, the fork's
/// `SmBrep::RemoveSlitLoopsWhereSolidsTouch`): removes the inner loops of no area a
/// volumetric Boolean leaves where solids touch along a curve inside a face (a cylinder
/// resting on the top of a box, united), then splits the shells joined only through
/// them ([`split_disconnected_shells`]). An inner loop is removed only when each of its
/// edges is used exactly twice by the loop and also bounds another face, which keeps
/// it; its uses go as [`delete_face`] removes a face's (edge uses destroyed through
/// `env`, vertex uses taken off their vertices, the loop uses off their face uses).
/// `SmTrimmingTools::CheckFace` rejects such a loop (bad orientation, zero area), and it
/// bounds nothing: the regularized result is the solids touching there, each whole.
/// Returns whether a loop was removed.
///
/// # Errors
///
/// `SM_ERR_NULL_POINTER` for a loop without both loop uses, an edge use without mate,
/// edge or vertex use, a vertex use without vertex (`NER`); those of the edge use
/// destruction, the topology reads and the shell split.
pub fn remove_slit_loops_where_solids_touch(
    k: &mut Kernel,
    b: &mut Brep,
    env: &mut NotifyEnv<'_>,
) -> Result<bool> {
    let mut removed = false;
    for f in p1::faces(b, None)? {
        // inner loops only
        let loops = face2::loops(b, f, false, None)?;
        for &l in loops.iter().skip(1) {
            let lu = b.loop_(l)?.loopuse.ok_or_else(ner)?;
            let uses = lu1::edgeuses(b, lu, None, None)?;
            if uses.len() < 2 {
                continue;
            }
            // every edge used twice by the loop, and bounding another face too
            let mut slit = true;
            for &eu in &uses {
                let e = eu1::edge(b, eu)?.ok_or_else(ner)?;
                let mut in_loop = 0usize;
                for &other in &uses {
                    if eu1::edge(b, other)? == Some(e) {
                        in_loop += 1;
                    }
                }
                let other_face = edge::faces(b, e, None)?.iter().any(|&g| g != f);
                slit = in_loop == 2 && other_face;
                if !slit {
                    break;
                }
            }
            if !slit {
                continue;
            }

            // take the loop off its face, then its edge uses and vertex uses off their
            // edges and vertices
            let other_lu = lu1::other_loopuse(b, lu)?.ok_or_else(ner)?;
            b.face_mut(f)?.loops.retain(|&x| x != l);
            for x in [lu, other_lu] {
                let fu = lu1::faceuse(b, x)?.ok_or_else(ner)?;
                faceuse_remove_loopuse(b, fu, x)?;
            }
            for eu in uses {
                let mate = eu1::mate(b, eu)?.ok_or_else(ner)?;
                let e = eu1::edge(b, eu)?.ok_or_else(ner)?;
                edge_remove_edgeuse(b, e, eu)?;
                edge_remove_edgeuse(b, e, mate)?;
                let (vu1, v1) = eu_vertex(b, eu)?;
                vertex_remove_vertexuse(b, v1, vu1)?;
                let (vu2, v2) = eu_vertex(b, mate)?;
                vertex_remove_vertexuse(b, v2, vu2)?;
                crate::topology::edgeuse::p2::destroy(k, b, eu, env)?;
                crate::topology::edgeuse::p2::destroy(k, b, mate, env)?;
            }
            removed = true;
        }
    }
    if removed {
        split_disconnected_shells(k, b)?;
    }
    Ok(removed)
}

/// Not upstream (source issue #746, the fork's
/// `SmBrep::SplitDisconnectedShells`): splits each shell into its connected parts, as
/// [`delete_face`] splits a shell a removed face leaves in pieces. The face uses
/// reached through the shell from the first face use's face's first vertex stay; the
/// rest move to a new face-use shell, in the infinite region or in a region of its
/// own (with the old region's void flag), which is split in its turn. At most
/// [`MAX_SHELL_SPLITS`] splits of each shell.
///
/// # Errors
///
/// Those of the shell traversal, the mark lock and the list edits.
pub fn split_disconnected_shells(k: &mut Kernel, b: &mut Brep) -> Result<()> {
    for r in p1::regions(b, None)? {
        let mut shells = region_shells(b, r)?;
        let mut jj = 0;
        while let Some(&s) = shells.get(jj) {
            jj += 1;
            for _ in 0..MAX_SHELL_SPLITS {
                let all = match &b.shell(s)?.members {
                    ShellMembers::Faceuses(list) => list.clone(),
                    _ => Vec::new(),
                };
                let Some(&first) = all.first() else {
                    break;
                };
                if all.len() < 2 {
                    break;
                }
                let face = eu1::faceuse_face(b, first)?.ok_or_else(ner)?;
                let Some(&start) = face_vertices(b, face)?.first() else {
                    break;
                };
                let mut reached = Vec::new();
                let mut marks = Marks::default();
                let mut lock = MarkLock::new(core::slice::from_mut(&mut k.context), 0, ALL_MARKS)?;
                let traversed = tt::shell_traversal(
                    k,
                    b,
                    &mut marks,
                    start,
                    s,
                    None,
                    Some(&mut reached),
                    &lock,
                );
                lock.release(core::slice::from_mut(&mut k.context))?;
                traversed?;
                if reached.len() >= all.len() {
                    break;
                }

                // the face uses not reached go to a new shell, in the infinite region or
                // a region of its own
                let new_s = b.add_shell_record(crate::topology::Shell::default())?;
                set_shell_type(b, new_s, ShellType::Faceuse)?;
                if Some(r) == b.infinite_region() {
                    shell::region_post_insert_shell(b, r, new_s)?;
                } else {
                    let new_r = b.add_region_record(Region::new())?;
                    let is_void = b.region(r)?.is_void();
                    region_set_is_void(b, new_r, is_void)?;
                    shell::region_post_insert_shell(b, new_r, new_s)?;
                    b.post_insert_region(new_r)?;
                }
                for fu in all {
                    if reached.contains(&fu) {
                        continue;
                    }
                    shell::shell_remove_faceuse(b, s, fu)?;
                    shell::shell_post_insert_faceuse(b, new_s, fu)?;
                }
                // the new shell may have parts of its own: it is split in its turn
                shells.push(new_s);
            }
        }
    }
    Ok(())
}

// ---- wire edges -------------------------------------------------------------------------------

/// The curve the new wire edge gets: `curve` itself when no edge owns it, else a copy
/// (see the module Deviations: upstream's copy is a sliced `SmCurve`).
fn wire_curve(curve: &Curve) -> Curve {
    // (Curve's Clone resets the owner, as the copy constructor)
    curve.clone()
}

/// The identity `curve` is notified under through `context`: the curve object's own
/// kernel-issued key ([`Curve::notify_key`]) when `context`'s identity domain issued
/// it, else a fresh key of that domain, kept on the curve (see the module's
/// Deviations). Upstream passes the `SmCurve` object itself.
///
/// # Errors
///
/// `InvalidInput` once the domain has issued every geometry key.
fn curve_identity(context: &mut Context, curve: &mut Curve) -> Result<ObjectRef> {
    let slot: &mut IdentitySlot = curve.notify_identity_slot();
    let key = context.geometry_key(slot)?;
    Ok(ObjectRef {
        type_code: curve.type_code(),
        key,
    })
}

/// The identity edge `e`'s curve is notified under through `context`
/// ([`Curve::notify_key`]; issued by `context` when it has none of its domain), `None`
/// without a curve.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; `InvalidInput` for an exhausted identity
/// domain.
pub fn edge_curve_identity(
    context: &mut Context,
    b: &mut Brep,
    e: EdgeId,
) -> Result<Option<ObjectRef>> {
    b.edge_mut(e)?
        .curve
        .as_mut()
        .map(|c| curve_identity(context, c))
        .transpose()
}

/// The identity face `f`'s surface is notified under through `context`
/// ([`Surface::notify_key`]; issued by `context` when it has none of its domain),
/// `None` without a surface.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; `InvalidInput` for an exhausted identity
/// domain.
pub fn face_surface_identity(
    context: &mut Context,
    b: &mut Brep,
    f: FaceId,
) -> Result<Option<ObjectRef>> {
    b.face_mut(f)?
        .surface
        .as_mut()
        .map(|s| {
            let type_code = s.type_code();
            let key = context.geometry_key(s.notify_identity_slot())?;
            Ok(ObjectRef { type_code, key })
        })
        .transpose()
}

/// [`edge_curve_identity`] through the edit's issuing context ([`issuer`]).
fn edge_curve_ref(
    k: &mut Kernel,
    b: &mut Brep,
    env: &mut NotifyEnv<'_>,
    e: EdgeId,
) -> Result<Option<ObjectRef>> {
    edge_curve_identity(issuer(&mut k.context, env.context.as_deref_mut()), b, e)
}

/// Binds `curve` (owned by nobody, or copied) to the new edge `e` (`SetCurve(.., FALSE)`
/// then `SetOwner(edge)`).
fn bind_wire_curve(
    k: &mut Kernel,
    b: &mut Brep,
    env: &mut NotifyEnv<'_>,
    e: EdgeId,
    curve: Curve,
) -> Result<()> {
    let mut new_curve = if curve
        .owner()
        .is_some_and(|o| o.type_code == edge_type_code())
    {
        // (a copy: another object, of an identity of its own)
        wire_curve(&curve)
    } else {
        // upstream binds the caller's curve object itself: it is moved in, with its
        // identity, and no second binding can reuse it
        curve
    };
    let _ = new_curve.set_owner(None, None);
    let new_ref = curve_identity(
        issuer(&mut k.context, env.context.as_deref_mut()),
        &mut new_curve,
    )?;
    edge::set_curve(
        b,
        e,
        Some(new_curve),
        SetCurveOptions {
            delete_old_curve: false,
            ..SetCurveOptions::default()
        },
        env,
        [Some(new_ref), None],
    )?;
    // connect Curve to Edge
    let edge_ref = (env.key_of)(EntityRef::Edge(e));
    if let Some(c) = b.edge_mut(e)?.curve.as_mut() {
        let _ = c.set_owner(Some(edge_ref), None);
    }
    b.set_geometry_owner_key(EntityRef::Edge(e), Some(edge_ref));
    Ok(())
}

/// `SmEdge_TYPE`.
const fn edge_type_code() -> i64 {
    edge::TYPE
}

/// `MakeWireEdge`: a new wire edge in `region` (the infinite region for `None`) from
/// `start` to `end` along `curve` over `interval` (`OrientType::Same`: the curve runs
/// from `start` to `end`), with the B-rep tolerance; a vertex further than a quarter
/// of the tolerance from its curve end refreshes its tolerance. Reported as
/// `SM_NO_ADD_TO_BREP`.
///
/// # Errors
///
/// Those of [`p2::make_edge_topology`], of the curve evaluation and of the tolerance
/// refresh; `InvalidInput` for a caller key in the kernel's reserved range or an
/// exhausted identity domain.
#[allow(clippy::too_many_arguments)]
pub fn make_wire_edge(
    k: &mut Kernel,
    b: &mut Brep,
    env: &mut NotifyEnv<'_>,
    region: Option<RegionId>,
    start: VertexId,
    end: VertexId,
    curve: Curve,
    interval: &Extent1d,
    orientation: OrientType,
) -> Result<EdgeId> {
    with_caller_identities(k, b, env, None, |k, b, env| {
        make_wire_edge_in(k, b, env, region, start, end, curve, interval, orientation)
    })
}

/// [`make_wire_edge`] once the identities are checked and seeded ([`with_caller_identities`]).
#[allow(clippy::too_many_arguments)]
fn make_wire_edge_in(
    k: &mut Kernel,
    b: &mut Brep,
    env: &mut NotifyEnv<'_>,
    region: Option<RegionId>,
    start: VertexId,
    end: VertexId,
    curve: Curve,
    interval: &Extent1d,
    orientation: OrientType,
) -> Result<EdgeId> {
    // Check for region default.
    let region = region_or_infinite(b, region);

    // make new edge connected to vertices and region
    let e = p2::make_edge_topology(k, b, region, start, end, env)?;

    // Set edge parameters
    let t = b.tolerance();
    edge::set_tolerance(b, e, t, true, true)?;
    edge::set_interval(b, e, interval)?;
    edge::set_orientation(b, e, orientation)?;

    // bind the curve (or a copy of it) to the new edge
    bind_wire_curve(k, b, env, e, curve)?;

    // set vertex tolerances to include edge end points (pCurve: the bound curve, the
    // caller's object moved in, or its identical copy)
    let bound = b
        .edge(e)?
        .curve
        .as_ref()
        .ok_or(null_deref("wire edge without curve"))?;
    let (p_start, p_end) = (
        bound.evaluate_point(k, interval.min())?,
        bound.evaluate_point(k, interval.max())?,
    );
    let mut tol_sq = b.tolerance();
    tol_sq = (tol_sq / 4.0) * (tol_sq / 4.0);
    if p_start.distance_squared(vertex::point(b, start)?) > tol_sq {
        vertex::refresh_tolerance(b, start, false)?;
    }
    if p_end.distance_squared(vertex::point(b, end)?) > tol_sq {
        vertex::refresh_tolerance(b, end, false)?;
    }

    // Notify of construction of new edge
    ignore_soft(brep_notify(
        b,
        env,
        NotifyOperation::AddToBrep,
        [arg(EntityRef::Edge(e)), brep_arg(b), None],
    ))?;
    Ok(e)
}

/// `MakeWireEdgeVertex`: a new wire edge in `region` (the infinite region for `None`)
/// from `start` to a new vertex at `end_point` along `curve` over `interval`, both with
/// the B-rep tolerance; both vertices refresh their tolerances. Reported as
/// `SM_NO_ADD_TO_BREP` (vertex, then edge). Returns `(edge, vertex)`.
///
/// # Errors
///
/// Those of [`p2::make_edge_vertex_topology`] and of the tolerance refresh;
/// `InvalidInput` for a caller key in the kernel's reserved range or an exhausted
/// identity domain.
#[allow(clippy::too_many_arguments)]
pub fn make_wire_edge_vertex(
    k: &mut Kernel,
    b: &mut Brep,
    env: &mut NotifyEnv<'_>,
    region: Option<RegionId>,
    start: VertexId,
    curve: Curve,
    interval: &Extent1d,
    orientation: OrientType,
    end_point: Point3,
) -> Result<(EdgeId, VertexId)> {
    with_caller_identities(k, b, env, None, |k, b, env| {
        make_wire_edge_vertex_in(
            k,
            b,
            env,
            region,
            start,
            curve,
            interval,
            orientation,
            end_point,
        )
    })
}

/// [`make_wire_edge_vertex`] once the identities are checked and seeded ([`with_caller_identities`]).
#[allow(clippy::too_many_arguments)]
fn make_wire_edge_vertex_in(
    k: &mut Kernel,
    b: &mut Brep,
    env: &mut NotifyEnv<'_>,
    region: Option<RegionId>,
    start: VertexId,
    curve: Curve,
    interval: &Extent1d,
    orientation: OrientType,
    end_point: Point3,
) -> Result<(EdgeId, VertexId)> {
    // Check for region default.
    let region = region_or_infinite(b, region);

    let (e, v) = p2::make_edge_vertex_topology(b, region, start)?;
    let vref = (env.key_of)(EntityRef::Vertex(v));
    let uref = |vu: VertexuseId| (env.key_of)(EntityRef::Vertexuse(vu));
    vertex::set_point(
        b,
        v,
        end_point,
        &mut *env.records,
        &mut *env.attributes,
        env.context.as_deref_mut(),
        vref,
        &uref,
    )?;

    // Set edge and vertex tolerance vector
    let t = b.tolerance();
    edge::set_tolerance(b, e, t, true, true)?;
    vertex::set_tolerance(b, v, t, true, true)?;

    // Set up interval and orientation
    edge::set_interval(b, e, interval)?;
    edge::set_orientation(b, e, orientation)?;

    // bind the curve (or a copy of it) to the new edge
    bind_wire_curve(k, b, env, e, curve)?;

    vertex::refresh_tolerance(b, start, false)?;
    vertex::refresh_tolerance(b, v, false)?;

    // Notify of construction of new edge and vertex
    ignore_soft(brep_notify(
        b,
        env,
        NotifyOperation::AddToBrep,
        [arg(EntityRef::Vertex(v)), brep_arg(b), None],
    ))?;
    ignore_soft(brep_notify(
        b,
        env,
        NotifyOperation::AddToBrep,
        [arg(EntityRef::Edge(e)), brep_arg(b), None],
    ))?;
    Ok((e, v))
}

// ---- SqueezeEdge --------------------------------------------------------------------------------

/// `SqueezeEdge`: squeezes the [vertex-edge-vertex] of edge `e` into the vertex
/// `survivor`: the edge's uses and their vertex uses are removed (loops of just this
/// edge become vertex loops, neighbours skip it, its UV trim curves go), the other
/// vertex's uses move to `survivor`, and the edge and other vertex are deleted. The
/// edge must not be closed.
///
/// # Errors
///
/// `SM_ERR` when `survivor` is not a vertex of `e`, `e` is closed, or a wire edge's
/// other vertex has fewer than two edges (or that edge fewer than two uses);
/// those of `SmFace::Notify` ([`crate::topology::face::p3::notify`]) for an edge in a
/// face; `InvalidInput` for a corrupt shell list, a caller key in the kernel's reserved
/// range or an exhausted identity domain.
pub fn squeeze_edge(
    k: &mut Kernel,
    b: &mut Brep,
    env: &mut NotifyEnv<'_>,
    e: EdgeId,
    survivor: VertexId,
) -> Result<()> {
    with_caller_identities(k, b, env, None, |k, b, env| {
        squeeze_edge_in(k, b, env, e, survivor)
    })
}

/// [`squeeze_edge`] once the identities are checked and seeded ([`with_caller_identities`]).
#[allow(clippy::too_many_lines)]
fn squeeze_edge_in(
    k: &mut Kernel,
    b: &mut Brep,
    env: &mut NotifyEnv<'_>,
    e: EdgeId,
    survivor: VertexId,
) -> Result<()> {
    // get edge vertices
    let v1 = edge::vertex(b, e)?;
    let v2 = edge::other_vertex(b, e, v1)?;

    // check intput - pSurvivingVertex must be one of the edge's vertices
    if v1 != Some(survivor) && v2 != Some(survivor) {
        return Err(ser());
    }

    // check input - edge cannot be a closed loop
    if v1 == v2 {
        return Err(ser());
    }

    // notify Brep and edge they are about to be modified
    ignore_soft(brep_notify(
        b,
        env,
        NotifyOperation::PreEdit,
        [brep_arg(b), None, None],
    ))?;
    let eref = (env.key_of)(EntityRef::Edge(e));
    let bref = p2::brep_ref(b);
    ignore_soft(edge::notify(
        b,
        e,
        env,
        NotifyOperation::PreEdit,
        [Some(eref), Some(bref), None],
    ))?;
    let cref = edge_curve_ref(k, b, env, e)?;
    ignore_soft(brep_notify(
        b,
        env,
        NotifyOperation::RmFromBrep,
        [
            arg(EntityRef::Edge(e)),
            brep_arg(b),
            cref.map(NotifyArg::Object),
        ],
    ))?;

    // locals - vertex to delete, edge->edgeuses
    let del_vertex =
        edge::other_vertex(b, e, Some(survivor))?.ok_or(null_deref("edge without other vertex"))?;
    let edgeuses = edge::edgeuses(b, e, None, None)?;

    // Special case if pEdgeToSqueeze is a wire.
    if vertexuse::edge_is_wire(b, e)? {
        for &eu in &edgeuses {
            let Some(shell) = eu1::shell(b, eu)? else {
                continue;
            };
            // SM_CAST_PTR(SmEdgeuse, pShell->GetList()) == pEU
            if b.shell(shell)?.members != ShellMembers::Edgeuse(eu) {
                continue;
            }
            // We have to change the Shell's pointer to an edge use on the edge on the
            // other side of the vertex that's being deleted.
            let edges = vertex::edges(b, del_vertex, None)?;
            if edges.len() < 2 {
                return Err(ser());
            }
            let (e0, e1) = (
                edges.first().copied().ok_or_else(ser)?,
                edges.get(1).copied().ok_or_else(ser)?,
            );
            let surviving_edge = if e0 == e { e1 } else { e0 };
            let surviving_eus = edge::edgeuses(b, surviving_edge, None, None)?;
            if surviving_eus.len() < 2 {
                return Err(ser());
            }
            let (s0, s1) = (
                surviving_eus.first().copied().ok_or_else(ser)?,
                surviving_eus.get(1).copied().ok_or_else(ser)?,
            );
            let surviving_eu = if eu1::orientation(b, eu)? == eu1::orientation(b, s0)? {
                s0
            } else {
                s1
            };
            set_shell_head(b, shell, ShellMembers::Edgeuse(surviving_eu))?;
        }
    } else {
        // Not a wire edge: so it's in a Loop, on a Face.
        let mut lock = MarkLock::new(core::slice::from_mut(&mut k.context), 0, ALL_MARKS)?;
        let res = squeeze_loops(b, env, &edgeuses, e, survivor);
        lock.release(core::slice::from_mut(&mut k.context))?;
        res?;
    }

    // Remove and destroy all squeezeEdge->edgeuses and corresponding vertexuses.
    for &eu in &edgeuses {
        // (dereferenced unchecked upstream)
        let vu = eu1::vertexuse(b, eu)?.ok_or(null_deref("edge use without vertex use"))?;
        let v = vertexuse::vertex(b, vu)?.ok_or(null_deref("vertex use without vertex"))?;
        edge_remove_edgeuse(b, e, eu)?;
        vertex_remove_vertexuse(b, v, vu)?;
    }

    // inform public two vertices are being merged into one
    ignore_soft(brep_notify(
        b,
        env,
        NotifyOperation::MergeInBrep,
        [
            arg(EntityRef::Vertex(survivor)),
            arg(EntityRef::Vertex(del_vertex)),
            brep_arg(b),
        ],
    ))?;

    // Move all deleteVertex->vertexuses to survivingVertex.
    for vu in vertex::vertexuses(b, del_vertex)? {
        vertex_remove_vertexuse(b, del_vertex, vu)?;
        b.vertex_mut(survivor)?.vertexuses.push(vu);
        b.vertexuse_mut(vu)?.vertex = Some(survivor);
    }

    // Delete squeezeEdge and deleteVertex now that they are removed from the topology
    // graph.
    let vref = (env.key_of)(EntityRef::Vertex(del_vertex));
    let key_of = env.key_of;
    ignore_soft(vertex::notify(
        b,
        del_vertex,
        &mut *env.records,
        &mut *env.attributes,
        env.context.as_deref_mut(),
        NotifyOperation::Destruction,
        vref,
        [Some(vref), None, None],
        &|vu| key_of(EntityRef::Vertexuse(vu)),
    ))?;
    b.remove_from_vertex_list(del_vertex);
    ignore_soft(edge::notify(
        b,
        e,
        env,
        NotifyOperation::Destruction,
        [Some(eref), None, None],
    ))?;
    let expected = p1::default_edge_owner(b, e);
    let owned = b
        .edge(e)?
        .curve
        .as_ref()
        .is_some_and(|c| c.owner().is_some_and(|o| o == expected || o == eref));
    if owned {
        b.edge_mut(e)?.curve = None;
        b.set_geometry_owner_key(EntityRef::Edge(e), None);
    }
    b.remove_from_edge_list(e);
    Ok(())
}

/// The loop part of [`squeeze_edge`]: every edge use of the squeezed edge is taken out
/// of its loop (its UV trim curve deleted; a loop of only this edge becomes a vertex
/// loop at `survivor`).
fn squeeze_loops(
    b: &mut Brep,
    env: &mut NotifyEnv<'_>,
    edgeuses: &[EdgeuseId],
    e: EdgeId,
    survivor: VertexId,
) -> Result<()> {
    for &eu in edgeuses {
        if let Some(face) = eu1::face(b, eu)? {
            ignore_soft(face_notify(
                b,
                face,
                env,
                NotifyOperation::PreEdit,
                [arg(EntityRef::Face(face)), brep_arg(b), None],
            ))?;
        }

        // remove UVTrimCurves
        b.edgeuse_mut(eu)?.uv_trim_curve = None;

        // See if squeeze edge is a strut edge in CW / CCW direction.
        let eu_cw = eu1::cw_edgeuse(b, eu)?.ok_or(null_deref("edge use without CW"))?;
        let strut_start = eu1::edge(b, eu_cw)? == Some(e);
        let eu_ccw = eu1::ccw_edgeuse(b, eu)?.ok_or(null_deref("edge use without CCW"))?;
        let strut_end = eu1::edge(b, eu_ccw)? == Some(e);

        // Make sure loop's primary edgeuse is not pEU.
        let lu = eu1::loopuse(b, eu)?.ok_or(null_deref("edge use without loop use"))?;
        if b.loopuse(lu)?.start == LoopuseStart::Edgeuse(Some(eu)) {
            b.loopuse_mut(lu)?.start = LoopuseStart::Edgeuse(Some(eu_cw));
        }

        // When edgeuse is a strut on both ends ... convert loop into a vertex loop.
        if strut_start && strut_end {
            let vu = b.add_vertexuse_record(Vertexuse {
                vertex: Some(survivor),
                owner: VertexuseOwner::Loopuse(Some(lu)),
                ..Vertexuse::default()
            })?;
            b.vertex_mut(survivor)?.vertexuses.insert(0, vu);
            b.loopuse_mut(lu)?.start = LoopuseStart::Vertexuse(Some(vu));
        }

        // Change neighbor edgeuse pointers to skip this squeeze edge.
        b.edgeuse_mut(eu_cw)?.ccw = Some(eu_ccw);
        b.edgeuse_mut(eu_ccw)?.cw = Some(eu_cw);
    }
    Ok(())
}

// ---- TurnToNURBS --------------------------------------------------------------------------------

/// Whether `s` is a kind of `SmBSplineSurface` (`SM_CAST_PTR(SmBSplineSurface, ..)`).
/// The B-spline part of a B-spline kind surface with its NURB
/// (`GetOrCreateGwNurbPointer`, which calls the kind's virtual `MakeNurb` when the
/// NURB is missing): borrowed when present; else a cone's, sphere's, torus's or surface
/// of revolution's is made and kept on the surface, as upstream keeps it, and an
/// extrusion's is made on a copy (nothing observable differs). A `MakeNurb` that fails leaves the
/// NULL NURB, as upstream's ignored status does; `None` for another kind.
///
/// # Errors
///
/// [`SolidError::Unported`] from a surface of revolution's `MakeNurb`.
pub(crate) fn with_nurb<'a>(k: &Kernel, s: &'a Surface) -> Result<Option<Cow<'a, BSplineSurface>>> {
    // a cone, sphere, torus or surface of revolution makes its NURB on the surface and
    // keeps it (`Surface::kind_with_nurb`, status ignored)
    if let Some(base) = s.revolution_base_with_nurb(k)? {
        return Ok(Some(Cow::Borrowed(base)));
    }
    Ok(match s.kind() {
        SurfaceKind::BSpline(x) => Some(Cow::Borrowed(x)),
        SurfaceKind::Plane(x) => Some(x.base_for_query()),
        SurfaceKind::SurfOfExtrusion(x) => {
            if x.base().gw_nurb().is_some() {
                Some(Cow::Borrowed(x.base()))
            } else {
                let mut tmp = x.as_ref().clone();
                match tmp.make_nurb(k) {
                    Err(e) if e.is_unported() => return Err(e),
                    Ok(()) | Err(_) => {}
                }
                Some(Cow::Owned(tmp.base().clone()))
            }
        }
        _ => None,
    })
}

fn as_bspline_surface(s: &Surface) -> Option<&BSplineSurface> {
    match s.kind() {
        SurfaceKind::BSpline(x) => Some(x),
        SurfaceKind::Plane(x) => Some(x.base()),
        SurfaceKind::SurfOfRevolution(x) => Some(x.base()),
        SurfaceKind::SurfOfExtrusion(x) => Some(x.base()),
        SurfaceKind::Cone(x) => Some(x.rev().base()),
        SurfaceKind::Torus(x) => Some(x.base().base()),
        SurfaceKind::Sphere(x) => Some(x.base().base()),
        _ => None,
    }
}

/// `TurnToNURBS`: makes every face surface and edge curve of the B-rep a plain
/// B-spline: a B-spline kind (`SmPlane`, `SmSurfOfRevolution`, `SmSphere`, `SmCone`, `SmTorus`) gets its B-spline
/// copy (`ReplaceSurface`, which rebuilds the UV trim curves); an `SmOffsetSurface` of a
/// B-spline kind is approximated (`ApproximateOffsetSurface` at ten times the B-rep
/// tolerance, chord length; a failed approximation skips the face), extended by 2% of
/// its box diagonal (`CreateExtendedSurface`, G1) and replaces the face's surface; a
/// curve of another type is copied (B-spline kinds), lifted (`SmCrvOnSurf`) or
/// approximated. Surfaces of other kinds are kept.
///
/// # Errors
///
/// those of [`p2::replace_surface`] (its `SmFace::Notify` and `CreateUVTrimCurves`,
/// `topology::face::p3`) for any face with an analytic surface; those of the curve lift / approximation;
/// `SM_ERR_NULL_POINTER` when a curve could not be made a B-spline or an offset's base
/// is not a B-spline kind (`NER`); those of the offset's bounding box and extension;
/// `InvalidInput` for a caller key in the kernel's reserved range or an exhausted
/// identity domain.
pub fn turn_to_nurbs(k: &mut Kernel, b: &mut Brep, env: &mut NotifyEnv<'_>) -> Result<()> {
    with_caller_identities(k, b, env, None, |k, b, env| {
        turn_to_nurbs_in(
            k,
            b,
            env,
            &|f| arena_identity(CacheObject::Face(f)),
            &|e| arena_identity(CacheObject::Edge(e)),
            &|e| arena_identity(CacheObject::Edge(e)),
        )
    })
}

/// The same conversion with the actual geometry owners of a caller-key edit.
/// The arena entrypoint retains its legacy owner convention; native construction
/// and production stores supply the identities held by their geometry objects.
pub(crate) fn turn_to_nurbs_with_geometry_owners(
    k: &mut Kernel,
    b: &mut Brep,
    env: &mut NotifyEnv<'_>,
    geometry_owner: &dyn Fn(EntityRef) -> ObjectRef,
) -> Result<()> {
    let key_of = env.key_of;
    with_caller_identities(k, b, env, None, |k, b, env| {
        turn_to_nurbs_in(
            k,
            b,
            env,
            &|f| geometry_owner(EntityRef::Face(f)),
            &|e| geometry_owner(EntityRef::Edge(e)),
            &|e| key_of(EntityRef::Edge(e)),
        )
    })
}

/// [`turn_to_nurbs`] once the identities are checked and seeded ([`with_caller_identities`]).
fn turn_to_nurbs_in(
    k: &mut Kernel,
    b: &mut Brep,
    env: &mut NotifyEnv<'_>,
    face_owner: &dyn Fn(FaceId) -> ObjectRef,
    edge_owner: &dyn Fn(EdgeId) -> ObjectRef,
    new_edge_owner: &dyn Fn(EdgeId) -> ObjectRef,
) -> Result<()> {
    ignore_soft(brep_notify(
        b,
        env,
        NotifyOperation::PreEdit,
        [brep_arg(b), None, None],
    ))?;

    // for every Brep->Surface (GetSurfaces, then the surface's face)
    let faces = p1::surface_owners(b, None, face_owner)?;
    for f in faces {
        let surface = b
            .face(f)?
            .surface
            .as_ref()
            .ok_or(null_deref("face without surface"))?;
        let Some(_) = as_bspline_surface(surface) else {
            if surface.type_code() != OFFSET_SURFACE_TYPE {
                continue;
            }
            let SurfaceKind::OffsetSurface(offset) = surface.kind() else {
                return Err(ner());
            };

            // it is an offset surface (ApproximateOffsetSurface's
            // GetOrCreateGwNurbPointer makes a missing NURB; NER there without one)
            let base = with_nurb(k, offset.base_surface())?.ok_or_else(ner)?;

            // offset tolerance should be quite large compared to brep tol
            let approx_tol_3d = b.tolerance() * 10.0;

            // convert (approximate) offset surface to NURB (1 = chord length)
            let new_bss = match base.approximate_offset_surface(
                offset.offset_distance(),
                approx_tol_3d,
                OffsetParamType::ChordLength,
            ) {
                Ok(x) => x,
                Err(e) if e.is_unported() => return Err(e),
                // SE(SM_ERR_INVALID_INPUT); continue
                Err(_) => continue,
            };

            // delete current UV trimming curves (status not checked upstream)
            ignore_soft(trimming_tools::clear_uv_trim_curves(b, f))?;

            // first extend the new surface in all directions by a small amount
            let new_surface = Surface::from(new_bss);
            let domain = new_surface.natural_uv_domain_with_nurb(k)?;
            // (status not checked upstream: a failed call leaves the Init() box, whose
            // size clamps to 1.0 below)
            let srf_bbox =
                match new_surface.calculate_bounding_box(k, &domain, [true, false, false], true) {
                    Ok(boxes) => boxes.normal_box.unwrap_or_else(Extent3d::new),
                    Err(e) if e.is_unported() => return Err(e),
                    Err(_) => Extent3d::new(),
                };
            let mut size = srf_bbox.size().length();
            if size >= 1.0e15 || size <= 0.0 {
                size = 1.0;
            }
            let extend_distance = 0.02 * size;
            let SurfaceKind::BSpline(new_bss) = new_surface.kind() else {
                return Err(ner());
            };
            let ext = new_bss.create_extended_surface(k, extend_distance, ContinuityType::G1)?;
            let face_ref = (env.key_of)(EntityRef::Face(f));
            p2::replace_surface(
                k,
                b,
                env,
                f,
                Surface::from(ext),
                face_ref,
                OldSurface::Delete,
                true,
            )?;
            continue;
        };
        if surface.type_code() == BSplineSurface::TYPE {
            continue;
        }
        // new SmBSplineSurface(*pBSS): the copy constructor's GetOrCreateGwNurbPointer
        // makes a missing NURB first
        let new_bss = Surface::from(with_nurb(k, surface)?.ok_or_else(ner)?.into_owned());
        let face_ref = (env.key_of)(EntityRef::Face(f));
        p2::replace_surface(k, b, env, f, new_bss, face_ref, OldSurface::Delete, true)?;
    }

    // for every Brep->Curve
    let edges = p1::curve_owners(b, None, edge_owner)?;
    for e in edges {
        let curve = b.edge(e)?.curve.as_ref().ok_or_else(ner)?;
        if curve.type_code() == BSplineCurve::TYPE {
            // SetCurve(pCurve, FALSE) with the edge's own curve: nothing happens
            continue;
        }
        let edge_tol = b.edge(e)?.tolerance;
        let approx_tol = 0.5 * edge_tol;
        let mut bsc: Option<Curve> = None;

        // Copy pCurve if it's a kind of SmBSplineCurve
        if let Some(base) = curve_bspline_base(curve) {
            bsc = Some(Curve::from(base.clone()));
        }

        // LiftCurve is pCurve is an SmCrvOnSurf (status ignored)
        if let CurveKind::CrvOnSurf(cos) = curve.kind() {
            // (`*SM_CAST_PTR(SmBSplineCurve, GetUVCurve())`: dereferenced unchecked)
            if curve_bspline_base(cos.uv_curve()).is_none() {
                return Err(null_deref("curve on surface without a B-spline UV curve"));
            }
            // (`pCrvOnSurf->GetBaseSurface()`: the curve's own surface, not a copy)
            let base = cos.surface();
            let ivl = b.edge(e)?.interval;
            match base.lift_curve(
                k,
                &base.natural_uv_domain_with_nurb(k)?,
                cos.uv_curve(),
                &ivl,
                approx_tol,
                false,
            ) {
                Ok(l) => bsc = Some(l.curve),
                Err(err) if err.is_unported() => return Err(err),
                Err(_) => {}
            }
        }

        // Approximate pCurve if we don't have a SmBSplineCurve yet (status ignored)
        if bsc.is_none() {
            let (knots, _) = match curve.knots(k, None) {
                Ok(x) => x,
                Err(err) if err.is_unported() => return Err(err),
                Err(_) => (Vec::new(), Vec::new()),
            };
            let opts = crate::curve::curve::p1::ApproximateCurveOptions::default();
            match curve.approximate_curve(k, &knots, approx_tol, &opts) {
                Ok(a) => bsc = Some(a.curve),
                Err(err) if err.is_unported() => return Err(err),
                Err(_) => {}
            }
        }

        // Must have a BSplineCurve here
        let bsc = bsc.ok_or_else(ner)?;

        // Update the edge: drop the old curve (TRUE = delete preExisting Curve)
        // (the new curve exists while the old one does: distinct identities)
        let old_ref = edge_curve_ref(k, b, env, e)?.ok_or_else(ner)?;
        let mut bsc = bsc;
        let r = curve_identity(issuer(&mut k.context, env.context.as_deref_mut()), &mut bsc)?;
        edge::set_curve(
            b,
            e,
            None,
            SetCurveOptions {
                delete_old_curve: true,
                ..SetCurveOptions::default()
            },
            env,
            [None, Some(old_ref)],
        )?;
        let mut new_curve = bsc;
        let _ = new_curve.set_owner(None, None);
        edge::set_curve(
            b,
            e,
            Some(new_curve),
            SetCurveOptions::default(),
            env,
            [Some(r), None],
        )?;
        let edge_ref = new_edge_owner(e);
        if let Some(c) = b.edge_mut(e)?.curve.as_mut() {
            let _ = c.set_owner(Some(edge_ref), None);
        }
        b.set_geometry_owner_key(EntityRef::Edge(e), Some(edge_ref));
    }
    Ok(())
}

/// The `SmBSplineCurve` part of a curve that is a kind of it (`SmLine`, the conics).
fn curve_bspline_base(c: &Curve) -> Option<&BSplineCurve> {
    match c.kind() {
        CurveKind::BSpline(x) => Some(x),
        CurveKind::Line(x) => Some(x.base()),
        CurveKind::Ellipse(x) => Some(x.base()),
        CurveKind::Parabola(x) => Some(x.base()),
        CurveKind::Hyperbola(x) => Some(x.base()),
        _ => None,
    }
}

// ---- OrientTrimmedSurfaces ----------------------------------------------------------------------

/// What [`orient_trimmed_surfaces`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OrientResult {
    /// `rbMaybeNotClosedSolid`: inconsistent ray votes, the B-rep may not be a closed
    /// solid.
    pub maybe_not_closed_solid: bool,
}

/// `OrientTrimmedSurfaces`: orients the faces so that their normals point out of
/// (`normals_outward`) or into the solid. Unless `just_unify_normals`, each face's
/// orientation is voted by firing rays from points inside it; then the faces reached
/// from it across manifold edges are collected and the smaller set of differently
/// oriented faces is reversed (`SmFace::SwapUV`). `faces_to_swap_uv`
/// (`pOptFacesToSwapUV`) are swapped first and count reversed in the vote.
///
/// # Errors
///
/// `SM_ERR` for a face without edges; `SM_ERR_ASSERT_FAILURE` for a non-orientable
/// B-rep; [`SolidError::Unported`]: the ray votes need the B-rep box (`query.brep`) and
/// edge use evaluation (`topology.edgeuse_p3`), the angle-limited collection
/// `SmEdgeuse::EvaluateBinormal` (`topology.edgeuse_p3`), `SmFace::SwapUV` its
/// `face_p3`/`face_p4` callees; `InvalidInput` for a caller key in the kernel's
/// reserved range.
pub fn orient_trimmed_surfaces(
    k: &mut Kernel,
    b: &mut Brep,
    env: &mut NotifyEnv<'_>,
    marks: &mut Marks,
    normals_outward: bool,
    just_unify_normals: bool,
    faces_to_swap_uv: Option<&[FaceId]>,
) -> Result<OrientResult> {
    with_caller_identities(k, b, env, None, |k, b, env| {
        orient_trimmed_surfaces_in(
            k,
            b,
            env,
            marks,
            normals_outward,
            just_unify_normals,
            faces_to_swap_uv,
        )
    })
}

/// [`orient_trimmed_surfaces`] once the identities are checked and seeded ([`with_caller_identities`]).
#[allow(clippy::too_many_lines)]
fn orient_trimmed_surfaces_in(
    k: &mut Kernel,
    b: &mut Brep,
    env: &mut NotifyEnv<'_>,
    marks: &mut Marks,
    normals_outward: bool,
    just_unify_normals: bool,
    faces_to_swap_uv: Option<&[FaceId]>,
) -> Result<OrientResult> {
    let mut lock = MarkLock::new(core::slice::from_mut(&mut k.context), 0, ALL_MARKS)?;
    let mark = lock.mark_type();
    let res = orient_body(
        k,
        b,
        env,
        marks,
        mark,
        normals_outward,
        just_unify_normals,
        faces_to_swap_uv,
    );
    lock.release(core::slice::from_mut(&mut k.context))?;
    res
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn orient_body(
    k: &Kernel,
    b: &mut Brep,
    env: &mut NotifyEnv<'_>,
    marks: &mut Marks,
    mark: MarkType,
    normals_outward: bool,
    just_unify_normals: bool,
    faces_to_swap_uv: Option<&[FaceId]>,
) -> Result<OrientResult> {
    // init output
    let mut maybe_not_closed = false;

    let tolerance = b.tolerance();
    let faces = p1::faces(b, None)?;
    let mut faceuses_processed = 0usize;
    let mut swap_faces: Vec<FaceId> = Vec::new();
    let in_swap = |f: FaceId| faces_to_swap_uv.is_some_and(|l| l.contains(&f));

    // Check orientation of each face
    for &face in &faces {
        // if we have processed a faceuse from each face, we're done
        if faceuses_processed >= faces.len() {
            break;
        }

        // skipped processed faces
        if marks.is_marked(k, Entity::Face(face), mark) {
            continue;
        }

        // processing this face uses a faceuse
        faceuses_processed += 1;

        // init voting for rayfire
        let mut same_faces: Vec<FaceId> = Vec::new();
        let mut reverse_faces: Vec<FaceId> = Vec::new();

        // shouldn't be necessary, but avoid ever touching this face again
        marks.mark(k, Entity::Face(face), mark);

        // If this face needs swapping, do it. Aids in bookkeeping later.
        if in_swap(face) {
            // (no SER upstream)
            ignore_soft(face1::swap_uv_with_notifications(k, b, face, env))?;
        }

        let (reverse_votes, same_votes) = if just_unify_normals {
            (0u32, 10u32)
        } else {
            match orient_ray_votes(k, b, env, marks, mark, face, normals_outward, tolerance)? {
                RayVotes::Skip => continue,
                RayVotes::Votes {
                    reverse,
                    same,
                    maybe_not_closed: m,
                } => {
                    maybe_not_closed |= m;
                    (reverse, same)
                }
            }
        };

        if reverse_votes == same_votes {
            maybe_not_closed = true;
            continue;
        }

        // votes for deciding which set of faces to flip
        let mut n_reverse = 0u32;
        let mut n_same = 0u32;

        let mut faceuses = Vec::new();
        let up = face2::upward_faceuse(b, face)?.ok_or(null_deref("face without face use"))?;
        let start = if reverse_votes > same_votes {
            // collect all connected downward faceuses
            eu1::faceuse_mate(b, up)?.ok_or(null_deref("face use without mate"))?
        } else {
            // collect all connected upward faceuses
            up
        };
        eu1::faceuse_collect_adjacent(k, b, start, &mut faceuses, None)?;

        // if we get too many faceuses, use a geometric contraint to pick off fewer
        let remaining = faces.len().wrapping_sub(faceuses_processed);
        if faceuses.len() > remaining {
            let angle = crate::math::PI * (11.0 / 12.0);
            faceuses.clear();
            eu1::faceuse_collect_adjacent(k, b, start, &mut faceuses, Some(angle))?;
            if faceuses.len() > remaining {
                return Err(SolidError::Upstream {
                    operation: OP,
                    code: SM_ERR_ASSERT_FAILURE,
                    entity: None,
                });
            }
        }
        faceuses_processed += faceuses.len();

        if reverse_votes > same_votes {
            if same_votes > 0 {
                maybe_not_closed = true;
            }
            reverse_faces.push(face);
            n_reverse += 1;
        } else {
            if reverse_votes > 0 {
                maybe_not_closed = true;
            }
            same_faces.push(face);
            n_same += 1;
        }

        // Faceuses collected are on the positive side and should have same orientation.
        for &fu in &faceuses {
            let swapping = eu1::faceuse_face(b, fu)?.ok_or(null_deref("face use without face"))?;
            marks.mark(k, Entity::Face(swapping), mark);
            if eu1::faceuse_orientation(b, fu)? == OrientType::Opposite {
                if in_swap(swapping) {
                    n_same += 1;
                } else {
                    n_reverse += 1;
                }
                reverse_faces.push(swapping);
            } else {
                if in_swap(swapping) {
                    n_reverse += 1;
                } else {
                    n_same += 1;
                }
                same_faces.push(swapping);
            }
        }

        // If more sReverse votes than sSame votes, we flip the sSameFaces
        if n_reverse > n_same {
            swap_faces.extend(same_faces);
        } else {
            swap_faces.extend(reverse_faces);
        }
    }

    // SwapUV on faces to be reversed
    ignore_soft(brep_notify(
        b,
        env,
        NotifyOperation::PreEdit,
        [brep_arg(b), None, None],
    ))?;
    for f in swap_faces {
        face1::swap_uv_with_notifications(k, b, f, env)?;
    }
    Ok(OrientResult {
        maybe_not_closed_solid: maybe_not_closed,
    })
}

/// The ray vote of one face in [`orient_trimmed_surfaces`].
enum RayVotes {
    /// The face separates two regions: it is done (swapped unless its upward face use
    /// faces the infinite region).
    Skip,
    /// The reverse / same votes and whether the rays were inconsistent.
    Votes {
        reverse: u32,
        same: u32,
        maybe_not_closed: bool,
    },
}

/// The ray firing of [`orient_trimmed_surfaces`] for `face`: up to two consistent
/// ray pairs from points inside the face (on trimmed iso curves through test points
/// of its edge uses), each voting whether the face normal points into the solid.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn orient_ray_votes(
    k: &Kernel,
    b: &mut Brep,
    env: &mut NotifyEnv<'_>,
    marks: &mut Marks,
    mark: MarkType,
    face: FaceId,
    normals_outward: bool,
    tolerance: f64,
) -> Result<RayVotes> {
    // five more or less random points
    let test_param = [0.234_567, 0.567_891, 0.789_123, 0.156_789, 0.856_789];

    // Random vector
    let ray_vec = Vec3::new(1.23456, 2.34567, 3.45678).unitize()?;

    // Brep BoundingBox and BoundingSphere
    let bbox = brep_bounding_box(k, b, true, false, true)?;
    let radius = bbox.sphere_bound().radius;

    let fu = face2::upward_faceuse(b, face)?.ok_or(null_deref("face without face use"))?;
    let other_fu = eu1::faceuse_mate(b, fu)?.ok_or(null_deref("face use without mate"))?;

    // When Faceuse is a boundary between two different Regions
    let r1 = eu1::faceuse_region(b, fu)?;
    let r2 = eu1::faceuse_region(b, other_fu)?;
    if r1 != r2 {
        // when upwardFaceuse is connected to infiniteRegion: no work needs to be done
        if r1 == b.infinite_region() {
            return Ok(RayVotes::Skip);
        }
        // point into different Regions and the first is NOT the infinite region
        face1::swap_uv_with_notifications(k, b, face, env)?;
        marks.mark(k, Entity::Face(face), mark);
        return Ok(RayVotes::Skip);
    }

    // Get upward facuse and its edgeuses
    let fu = face2::upward_faceuse(b, face)?.ok_or(null_deref("face without face use"))?;
    let edgeuses = eu1::faceuse_edgeuses(b, fu, false, None)?;
    if edgeuses.is_empty() {
        return Err(ser()); // Face needs to have at least one edge
    }

    let mut reverse = 0u32;
    let mut same = 0u32;
    let mut bad_count = 0u32;
    let mut maybe_not_closed = false;
    let mut rays_fired = 0u32;
    'edgeuses: for &eu in &edgeuses {
        if rays_fired >= 2 {
            break;
        }
        for &t in &test_param {
            if rays_fired >= 2 {
                break;
            }
            // get edgeuse testPoint on the face.
            let (pnt, tan) = normalized_evaluate(k, b, eu, t, true, true)?;

            // build temporary isoParam Curve through the testPoint
            let surface = b
                .face(face)?
                .surface
                .as_ref()
                .ok_or(null_deref("face without surface"))?;
            let (dir, at) = if tan.x.abs() > tan.y.abs() {
                (crate::nurbs::b_spline_surface::SurfParamType::U, pnt.x)
            } else {
                (crate::nurbs::b_spline_surface::SurfParamType::V, pnt.y)
            };
            let iso = surface
                .create_iso_parametric_curve(k, dir, at, b.tolerance(), None, false)?
                .curve;

            // trim isoParamCurve to Face
            let trimmed = face2::create_curves_by_trimming(
                k,
                b,
                face,
                Some(tolerance),
                Some(&[Some(iso)]),
                None,
                true,
                false,
            )?;
            let trimmed = trimmed.curves_3d.unwrap_or_default();

            // skip isoParamCurves outside of the face
            let Some(curve) = trimmed.first() else {
                continue;
            };

            // get a point on the face far from its trim edges
            let ivl = curve.natural_interval();
            let point_in_face = curve.evaluate_point(k, ivl.evaluate(test_param[1]))?;

            // Get Surface Normal for this point.
            let dom = b.face(face)?.uv_domain;
            // the face's surface reaches its face's cache (smsurf_GetSurfaceCache),
            // made on the B-rep: an exact evaluation copy of the surface meanwhile (the
            // face's surface, read again after the trimming's in-place edits)
            let surface = b
                .face(face)?
                .surface
                .as_ref()
                .ok_or(null_deref("face without surface"))?
                .clone_for_evaluation()?;
            let tolerance_3d = b.tolerance();
            let mut sols = crate::geom::solution_array::SolutionArray::new();
            surface.global_point_solve_into_with_context::<CacheObject>(
                k,
                Some(CacheObject::Surface(face)),
                None,
                &mut crate::query::brep::BrepFaceServices::new(b),
                &dom,
                crate::geom::global_solver::SolverOperation::Minimize,
                point_in_face,
                tolerance_3d,
                None,
                crate::geom::global_solver::SolutionRequested::Single,
                &mut sols,
            )?;
            let sol = *sols.get(0).ok_or_else(ser)?;
            let uv = crate::math::Point2::new(sol.start.param(0)?, sol.start.param(1)?);
            let normal = surface.evaluate_normal(
                k,
                uv,
                crate::nurbs::knots::Side::Left,
                crate::nurbs::knots::Side::Left,
            )?;

            // for two rays leaving the surface in either direction
            let mut inside = [false; 2];
            for (ll, slot) in inside.iter_mut().enumerate() {
                let mut direction = if ll == 0 { 1.0 } else { -1.0 };
                let new_ray_vec = if normal.dot(ray_vec) < 0.0 {
                    direction = -direction;
                    (ray_vec - normal).unitize()?
                } else {
                    (ray_vec + normal).unitize()?
                };

                // build a ray line segment
                if !normals_outward {
                    direction = -direction;
                }
                let end = point_in_face + direction * radius * 3.0 * new_ray_vec;
                let line = Curve::from(BSplineCurve::create_line_segment(3, point_in_face, end)?);

                // get all ray/brep intersections (editing off while solving)
                let solutions = brep_curve_solve(k, b, &line, tolerance)?;

                // Count the solutions to determine if we are inside or outside
                let mut points: Vec<Point3> = Vec::new();
                for (num_variables, param) in solutions {
                    // when the solution was a face/curve intersection
                    if num_variables == 3 {
                        let p = line.evaluate_point(k, param)?;
                        if !points.iter().any(|q| p.distance(*q) < tolerance) {
                            points.push(p);
                        }
                    }
                }

                // count number of face/curve intersections (ignoring the point on this
                // face; an unsigned count upstream)
                let n = points.len().wrapping_sub(1);

                // point is inside if there were an odd number of intersections
                *slot = n % 2 == 1;
            }

            // Make sure rays are consistent
            if inside[0] == inside[1] {
                bad_count += 1;
                if bad_count == 5 {
                    break;
                }
                maybe_not_closed = true;
                continue;
            }

            // Now let's see if the normal is in the right direction
            if inside[0] {
                reverse += 1;
            } else {
                same += 1;
            }

            // increment the successfully cast ray count
            rays_fired += 1;
        }
        if bad_count > 5 {
            break 'edgeuses;
        }
    }
    Ok(RayVotes::Votes {
        reverse,
        same,
        maybe_not_closed,
    })
}

/// `SmTopologySolver::BrepCurveSolve(this, line, its natural interval, SM_SO_INTERSECT,
/// SM_SR_ALL, tolerance, SM_BIG_DOUBLE, NULL)` with `m_bEditingEnabled` off during the
/// solve: the `(number of variables, curve parameter)` of every solution. The B-rep's
/// spatial cache is built for the call (upstream's was dropped by the pre-edit
/// notification).
fn brep_curve_solve(
    k: &Kernel,
    b: &mut Brep,
    line: &Curve,
    tolerance: f64,
) -> Result<Vec<(usize, f64)>> {
    use crate::intersect::topology_solver::p1::{CurveRef, TopologySolver};
    use crate::intersect::topology_solver::p2::TopologySolveOptions;
    let editing = b.editing_enabled();
    b.set_editing_enabled(false);
    let res = (|| -> Result<Vec<(usize, f64)>> {
        let query = crate::query::brep::BrepQuery::of_brep(b);
        // (CacheMakeOrValidate: the cache with its vertex, curve and surface trees)
        let cache = query.cache(k, b)?;
        // the solve makes the face caches it reaches on the B-rep
        // (smsurf_GetSurfaceCache) and classifies on it
        let identity = query.effective_identity();
        let objects = crate::query::brep::LiveBrepObjects::new(b, &identity, Some(cache))?;
        let sols = TopologySolver::brep_curve_solve(
            k,
            &objects,
            Some(CacheObject::Brep),
            CurveRef {
                curve: line,
                handle: CacheObject::Other(ObjectRef {
                    type_code: BSplineCurve::TYPE,
                    key: ObjectKey(0),
                }),
            },
            &line.natural_interval(),
            TopologySolveOptions {
                operation: crate::geom::global_solver::SolverOperation::Intersect,
                requested: crate::geom::global_solver::SolutionRequested::All,
                tolerance_3d: tolerance,
                best_answer_so_far_sq: crate::math::BIG_DOUBLE,
                opt_vectors: &[],
            },
        )?;
        let mut out = Vec::new();
        for i in 0..sols.len() {
            let s = sols.get(i).ok_or_else(ser)?;
            out.push((s.num_variables, s.start.param(0)?));
        }
        Ok(out)
    })();
    b.set_editing_enabled(editing);
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `TurnToNURBS`' copy of a B-spline kind surface (`new SmBSplineSurface(*pBSS)`)
    /// and its offset arm's `ApproximateOffsetSurface` read the NURB through
    /// `GetOrCreateGwNurbPointer`: a bounded plane built without `MakeNurb` gets its
    /// NURB and domain, not the empty B-spline part.
    #[test]
    fn retained_face_deletion_uses_real_marks_body_callbacks_and_failure_prefix() {
        use crate::geom::attribute::{ids, Attribute, AttributeStore, Behavior};
        use crate::topology::edge::notification_test_support as callbacks;

        for fail_after_notifications in [false, true] {
            let mut k = Kernel::default();
            let mut b = crate::primitive::api_primitives::create_box(
                &mut k,
                Point3::ZERO,
                10.0,
                20.0,
                30.0,
            )
            .unwrap();
            b.set_editing_enabled(true);
            let legacy_key = b.key();
            let serial = b.notify_serial().unwrap();
            let key_of = |entity| topology_ref(serial, entity);
            let body = key_of(EntityRef::Brep(crate::topology::BrepId::from_index(0)));
            assert_ne!(legacy_key, body.key);
            let face = b.face_list()[0];
            let face_ref = key_of(EntityRef::Face(face));
            if fail_after_notifications {
                // Defined NER after the actual public-edit notification prefix.
                b.face_mut(face).unwrap().faceuse = None;
            }
            let mut records = crate::topology::topology::NotifyRecords::new();
            seed_records(&mut records, &b, &key_of);
            records.get_mut(&face_ref.key).unwrap().set_user_index1(73);
            let mut attributes = AttributeStore::default();
            let marker = attributes
                .insert(Attribute::long(ids::USER_1, 917, Behavior::Reference))
                .unwrap();
            attributes.add_attribute(body.key, marker).unwrap();
            let events = callbacks::install(&mut k.context);
            k.context.set_doing_boolean(true);
            let mut outer =
                MarkLock::new(core::slice::from_mut(&mut k.context), 0, ALL_MARKS).unwrap();
            let outer_type = outer.mark_type();
            let outer_value = k.context.current_mark(outer_type);
            let nested_before = k.context.current_mark(MarkType::Mark2);
            let result = Context::with_notification_context(&mut k, |k, context| {
                let mut env = NotifyEnv {
                    records: &mut records,
                    attributes: &mut attributes,
                    context: Some(context),
                    key_of: &key_of,
                };
                delete_face_with_body_identity(
                    k,
                    &mut b,
                    &mut env,
                    face,
                    DeleteFaceOptions::default(),
                    body,
                )
            })
            .unwrap();
            assert!(k.context.is_mark_locked(outer_type));
            assert_eq!(k.context.current_mark(outer_type), outer_value);
            assert!(!k.context.is_mark_locked(MarkType::Mark2));
            assert!(!k.context.is_mark_locked(MarkType::Mark3));
            if fail_after_notifications {
                assert!(
                    matches!(result, Err(SolidError::Upstream { code, .. }) if code == crate::error::SM_ERR_NULL_POINTER)
                );
                assert!(b.face(face).is_ok());
                assert_eq!(records[&face_ref.key].user_index1(), 73);
                assert_eq!(k.context.current_mark(MarkType::Mark2), nested_before);
            } else {
                result.unwrap();
                assert!(!b.face_list().contains(&face));
                assert!(k.context.current_mark(MarkType::Mark2) > nested_before);
                assert!(records
                    .values()
                    .any(|record| record.get_mark(MarkType::Mark2) > 0));
            }
            outer
                .release(core::slice::from_mut(&mut k.context))
                .unwrap();
            assert_eq!(b.key(), legacy_key);
            assert_eq!(attributes.users(marker).unwrap(), &[body.key]);
            assert!(k.context.doing_boolean());
            callbacks::assert_installed(&k.context, &events);
            let events = events.lock().unwrap();
            assert!(events.len() >= 6);
            for pair in events.chunks_exact(2) {
                assert_eq!(pair[0].callback, "user");
                assert_eq!(pair[1].callback, "system");
                assert_eq!(pair[0].caller, pair[1].caller);
                assert_eq!(pair[0].operation, pair[1].operation);
                assert_eq!(pair[0].data, pair[1].data);
            }
            assert_eq!(events[0].caller, body);
            assert_eq!(events[0].operation, NotifyOperation::PreEdit);
            assert_eq!(events[0].data, [Some(body), None, None]);
            assert_eq!(events[2].caller, face_ref);
            assert_eq!(events[2].operation, NotifyOperation::PreEdit);
            assert_eq!(events[4].caller, body);
            assert_eq!(events[4].operation, NotifyOperation::RmFromBrep);
            assert_eq!(events[4].data, [Some(face_ref), Some(body), None]);
            for event in events
                .iter()
                .filter(|event| event.caller.type_code == body.type_code)
            {
                assert_eq!(event.caller, body);
                match event.operation {
                    NotifyOperation::PreEdit => assert_eq!(event.data[0], Some(body)),
                    NotifyOperation::RmFromBrep => assert_eq!(event.data[1], Some(body)),
                    NotifyOperation::MergeInBrep => assert_eq!(event.data[2], Some(body)),
                    _ => (),
                }
            }
            assert!(!events.iter().any(|event| event.caller.key == legacy_key));
        }
    }

    #[test]
    fn lazy_plane_gets_its_nurb_before_the_copy() {
        use crate::geom::extent2d::Extent2d;
        use crate::geom::vector2d::Vec2;
        use crate::surface::plane::Plane;
        let k = Kernel::default();
        let dom = Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0);
        let plane = Plane::new(Vec3::ZERO, Vec3::X, Vec3::Y, Vec2::new(1.0, 1.0), &dom);
        let s = Surface::from(plane);
        let SurfaceKind::Plane(p) = s.kind() else {
            panic!("not a plane")
        };
        assert!(p.base().gw_nurb().is_none());
        let with = with_nurb(&k, &s).unwrap().unwrap();
        assert!(with.gw_nurb().is_some());
        let d = with.natural_uv_domain();
        assert!(d.max().x > d.min().x && d.max().y > d.min().y);
        // another kind has no B-spline part
        assert!(with_nurb(&k, &Surface::from(BSplineSurface::new()))
            .unwrap()
            .is_some());
    }

    /// An object an edit makes has no record in the caller's map yet; the first
    /// notification that names it gives it its default `SmTopology` record (upstream's
    /// constructor makes one), so `SM_NO_MERGE_IN_BREP` of two such vertices works.
    #[test]
    fn new_objects_get_their_records_when_notified() {
        let mut b = Brep::new();
        let v0 = b.add_vertex(Vec3::ZERO).unwrap();
        let v1 = b.add_vertex(Vec3::X).unwrap();
        let r = with_env(|env| {
            assert!(env.records.is_empty());
            let r = brep_notify(
                &b,
                env,
                NotifyOperation::MergeInBrep,
                [
                    arg(EntityRef::Vertex(v0)),
                    arg(EntityRef::Vertex(v1)),
                    brep_arg(&b),
                ],
            );
            for v in [v0, v1] {
                assert!(env
                    .records
                    .contains_key(&(env.key_of)(EntityRef::Vertex(v)).key));
            }
            r
        });
        r.unwrap();
    }

    /// Every curve object gets an identity of its own from the issuing context: a curve
    /// keeps it; a copy (a new object, as a replacement curve) gets another; the keys
    /// are in the geometry range.
    #[test]
    fn curve_identities_are_distinct() {
        use crate::curve::line::Line;
        let mut ctx = Context::default();
        let mut c0 = Curve::from(Line::from_points(Vec3::ZERO, Vec3::X, 3));
        let mut c1 = Curve::from(Line::from_points(Vec3::ZERO, Vec3::Y, 3));
        let (r0, r1) = (
            curve_identity(&mut ctx, &mut c0).unwrap(),
            curve_identity(&mut ctx, &mut c1).unwrap(),
        );
        assert_ne!(r0.key, r1.key);
        assert_eq!(curve_identity(&mut ctx, &mut c0).unwrap(), r0);
        let mut copy = c0.clone();
        assert_eq!(copy.notify_key(), None);
        let rc = curve_identity(&mut ctx, &mut copy).unwrap();
        assert!(rc.key != r0.key && rc.key != r1.key);
        assert!(r0.key.0 >= crate::geom::context::GEOMETRY_KEY_BASE);
    }

    /// Binding moves the caller's unowned curve in with its identity (upstream binds
    /// that object), so a second wire edge needs a curve value of its own: a copy, a
    /// new object with a key of its own.
    #[test]
    fn bound_curves_never_share_a_key() {
        use crate::curve::line::Line;
        use crate::topology::Edge;
        let mut k = Kernel::default();
        let mut b = Brep::new();
        let edge = |b: &mut Brep| {
            b.add_edge_record(Edge {
                vertices: [VertexId::from_index(0); 2],
                edgeuses: Vec::new(),
                interval: Extent1d::from_min_max(0.0, 1.0),
                tolerance: 1e-6,
                curve: None,
            })
            .unwrap()
        };
        let (e1, e2) = (edge(&mut b), edge(&mut b));
        let mut c = Curve::from(Line::from_points(Vec3::ZERO, Vec3::X, 3));
        let original = curve_identity(&mut k.context, &mut c).unwrap();
        let second_use = c.clone();
        with_env(|env| bind_wire_curve(&mut k, &mut b, env, e1, c)).unwrap();
        with_env(|env| bind_wire_curve(&mut k, &mut b, env, e2, second_use)).unwrap();
        let key = |b: &Brep, e| {
            b.edge(e)
                .unwrap()
                .curve
                .as_ref()
                .and_then(Curve::notify_key)
        };
        assert_eq!(key(&b, e1), Some(original.key));
        assert!(key(&b, e2).is_some() && key(&b, e2) != key(&b, e1));
    }

    /// A key is only valid in the domain that issued it: a curve keyed by C1 that then
    /// meets C2 gets a key of C2's domain, so it never shares one with a curve C2 keyed
    /// itself, even though both domains count from the start.
    #[test]
    fn curve_keys_belong_to_their_context() {
        use crate::curve::line::Line;
        let (mut c1, mut c2) = (Context::default(), Context::default());
        let mut a = Curve::from(Line::from_points(Vec3::ZERO, Vec3::X, 3));
        let mut b = Curve::from(Line::from_points(Vec3::ZERO, Vec3::Y, 3));
        let a1 = curve_identity(&mut c1, &mut a).unwrap();
        let a2 = curve_identity(&mut c2, &mut a).unwrap();
        let b2 = curve_identity(&mut c2, &mut b).unwrap();
        assert_ne!(a2.key, b2.key);
        // (C2 gave A its own first key, which has C1's first number but C2's domain)
        assert_eq!(a1.key, a2.key);
        assert_eq!(curve_identity(&mut c2, &mut a).unwrap(), a2);
    }

    /// Every key a notify callback got, in order.
    type Seen = std::sync::Arc<std::sync::Mutex<Vec<ObjectRef>>>;

    #[derive(Debug)]
    struct Rec(Seen);

    impl crate::geom::context::NotifyCallback for Rec {
        fn execute(
            &mut self,
            _operation: NotifyOperation,
            caller: ObjectRef,
            data1: Option<ObjectRef>,
            data2: Option<ObjectRef>,
            data3: Option<ObjectRef>,
        ) -> Result<()> {
            let mut seen = self.0.lock().unwrap();
            seen.extend([Some(caller), data1, data2, data3].into_iter().flatten());
            Ok(())
        }
    }

    /// A context whose user callback records every key it is given.
    fn recording_context() -> (Context, Seen) {
        let seen = Seen::default();
        let mut ctx = Context::default();
        ctx.set_user_notify_callback(Some(Box::new(Rec(std::sync::Arc::clone(&seen)))));
        (ctx, seen)
    }

    /// A caller's identities: kind and index, `bit` set to tell a second B-rep apart
    /// (as a caller names its own objects; every key is below the kernel's range).
    fn caller_key(e: EntityRef, bit: u64) -> ObjectRef {
        let r = default_ref(e);
        ObjectRef {
            type_code: r.type_code,
            key: ObjectKey(r.key.0 | bit),
        }
    }

    /// Kind and index (`(kind << 32) | index`), with the class type code.
    fn default_ref(e: EntityRef) -> ObjectRef {
        let r = topology_ref(0, e);
        ObjectRef {
            type_code: r.type_code,
            key: ObjectKey(r.key.0 & !TOPOLOGY_KEY_BASE),
        }
    }

    fn dest_key(e: EntityRef) -> ObjectRef {
        caller_key(e, 0)
    }

    fn source_key(e: EntityRef) -> ObjectRef {
        caller_key(e, 1 << 40)
    }

    /// `SqueezeEdge` of the chain's first edge into its first vertex, notified through
    /// `ctx` (`env.context`) under the caller's identities; returns the keys `ctx`'s
    /// callback got.
    fn squeeze_through(k: &mut Kernel, ctx: &mut Context, seen: &Seen) -> Vec<ObjectKey> {
        let (mut b, [a, _, _], [e0, _]) = test_support::wire_chain(true);
        let first = seen.lock().unwrap().len();
        let mut records = crate::topology::topology::NotifyRecords::new();
        let mut attributes = crate::geom::attribute::AttributeStore::default();
        let mut env = NotifyEnv {
            records: &mut records,
            attributes: &mut attributes,
            context: Some(ctx),
            key_of: &dest_key,
        };
        squeeze_edge(k, &mut b, &mut env, e0, a).unwrap();
        let all = seen.lock().unwrap();
        all.get(first..).unwrap().iter().map(|r| r.key).collect()
    }

    fn geometry_keys(v: &[ObjectKey]) -> Vec<ObjectKey> {
        v.iter()
            .copied()
            .filter(|x| x.0 >= crate::geom::context::GEOMETRY_KEY_BASE)
            .collect()
    }

    /// R1: geometry identities come from the one context the notifications go through,
    /// not the kernel's: one callback, edits in two kernels (both counting from the
    /// start), and no curve key of the first edit comes back for the second's.
    #[test]
    fn one_context_issues_the_identities_of_edits_in_two_kernels() {
        let (mut ctx, seen) = recording_context();
        let (mut k1, mut k2) = (Kernel::default(), Kernel::default());
        let one = geometry_keys(&squeeze_through(&mut k1, &mut ctx, &seen));
        let two = geometry_keys(&squeeze_through(&mut k2, &mut ctx, &seen));
        assert!(!one.is_empty() && !two.is_empty(), "{one:?}");
        assert!(two.iter().all(|x| !one.contains(x)), "{one:?} {two:?}");
    }

    /// Setter: a caller key in the kernel's range is refused before anything changes,
    /// so `Brep::set_key` (the one setter) cannot take a kernel-named B-rep's identity;
    /// a clone (the only other way to duplicate an object) starts with no identity.
    #[test]
    fn caller_keys_cannot_take_a_kernel_identity() {
        let (mut ctx, _seen) = recording_context();
        let mut k = Kernel::default();
        let (mut b1, [a, _, _], [e0, _]) = test_support::wire_chain(true);
        let mut b2 = b1.clone();
        let serial = notify_serial(&mut ctx, &mut b1).unwrap();
        // b2 claims b1's kernel identities, for itself and for its objects
        b2.set_key(topology_ref(serial, brep_entity()).key);
        let forged = move |e: EntityRef| topology_ref(serial, e);
        let edges = b2.edges().len();
        let mut records = crate::topology::topology::NotifyRecords::new();
        let mut attributes = crate::geom::attribute::AttributeStore::default();
        for key_of in [&forged as &dyn Fn(EntityRef) -> ObjectRef, &dest_key] {
            let mut env = NotifyEnv {
                records: &mut records,
                attributes: &mut attributes,
                context: Some(&mut ctx),
                key_of,
            };
            let r = squeeze_edge(&mut k, &mut b2, &mut env, e0, a);
            assert_eq!(r, Err(RESERVED_KEY));
        }
        assert_eq!(b2.edges().len(), edges);
        assert!(records.is_empty());
        // a key in the geometry range is refused the same way
        b2.set_key(ObjectKey(0));
        let curve_key = |_: EntityRef| ObjectRef {
            type_code: 0,
            key: ObjectKey(crate::geom::context::GEOMETRY_KEY_BASE | 1),
        };
        let mut env = NotifyEnv {
            records: &mut records,
            attributes: &mut attributes,
            context: Some(&mut ctx),
            key_of: &curve_key,
        };
        assert_eq!(
            squeeze_edge(&mut k, &mut b2, &mut env, e0, a),
            Err(RESERVED_KEY)
        );
        // clones are other objects: no serial, no curve key
        assert_eq!(b1.clone().notify_serial(), None);
        let curve = b1.edge(e0).unwrap().curve.clone().unwrap();
        assert_eq!(curve.notify_key(), None);
    }

    /// C-M1 / C-M2: the records and attributes a caller keeps under its own keys are
    /// what a copy or a merge carries (`SM_NO_COPY`), every time: the source's
    /// identities are the caller's, not re-issued per call. The B-rep's attribute
    /// under its key reaches the copy (the construction's `SM_NO_COPY`).
    #[test]
    fn copies_and_merges_carry_the_callers_records_and_attributes() {
        use crate::geom::attribute::{Attribute, Behavior};
        let mut k = Kernel::default();
        let (mut src, _, [e0, _]) = test_support::wire_chain(true);
        src.set_key(ObjectKey(100));
        let mut records = crate::topology::topology::NotifyRecords::new();
        let mut attributes = crate::geom::attribute::AttributeStore::default();
        let edge = source_key(EntityRef::Edge(e0)).key;
        let mut t = topo::Topology::new();
        t.set_all_flags(256);
        t.set_user_index1(73);
        records.insert(edge, t);
        for (obj, id) in [(edge, 5), (src.key(), 6)] {
            let h = attributes
                .insert(Attribute::long(id, 1, Behavior::Copy))
                .unwrap();
            attributes.add_attribute(obj, h).unwrap();
        }
        let mut env = NotifyEnv {
            records: &mut records,
            attributes: &mut attributes,
            context: None,
            key_of: &dest_key,
        };
        let copy = copy_brep(&mut k, &mut env, &src, &source_key, None, None, true, false).unwrap();
        let mut dest = Brep::new();
        let inf = dest.add_region_record(Region::default()).unwrap();
        dest.set_infinite_region(Some(inf));
        dest.pre_insert_region(inf).unwrap();
        // (dest's objects are told apart from the copy's: another caller key space)
        let dest_key2 = |e: EntityRef| caller_key(e, 2 << 40);
        env.key_of = &dest_key2;
        for _ in 0..2 {
            merge_brep(
                &mut k,
                &mut env,
                &mut dest,
                &src,
                &source_key,
                None,
                None,
                false,
            )
            .unwrap();
        }
        let carried = |records: &crate::topology::topology::NotifyRecords,
                       b: &Brep,
                       key_of: &dyn Fn(EntityRef) -> ObjectRef| {
            b.edges()
                .ids()
                .filter(|&x| {
                    records
                        .get(&key_of(EntityRef::Edge(x)).key)
                        .is_some_and(|r| r.user_index1() == 73 && r.all_flags() == 256)
                })
                .count()
        };
        assert_eq!(carried(env.records, &copy, &dest_key), 1);
        assert_eq!(carried(env.records, &dest, &dest_key2), 2);
        let attributed = dest
            .edges()
            .ids()
            .filter(|&x| {
                env.attributes
                    .find_attribute(dest_key2(EntityRef::Edge(x)).key, 5)
                    .is_some()
            })
            .count();
        assert_eq!(attributed, 2);
        assert!(env.attributes.find_attribute(copy.key(), 6).is_some());
        // the source's own state is untouched
        assert_eq!(env.records[&edge].user_index1(), 73);
    }

    /// C-M3 / P4: a lent edit nested in another is refused before anything changes:
    /// the B-rep, its serial and the callbacks stay as they were, and the outer edit
    /// still notifies the kernel's callback.
    #[test]
    fn nested_lent_edits_are_refused() {
        let mut k = Kernel::default();
        let seen = Seen::default();
        k.context
            .set_user_notify_callback(Some(Box::new(Rec(std::sync::Arc::clone(&seen)))));
        let (mut b, [a, _, _], [e0, _]) = test_support::wire_chain(true);
        let r = with_kernel_callbacks(&mut k, &mut b, |k, b, env| {
            let serial = b.notify_serial();
            let key = b.key();
            let inner = with_kernel_callbacks(k, b, |k, b, env| squeeze_edge(k, b, env, e0, a));
            assert!(
                matches!(inner, Err(SolidError::InvalidInput { .. })),
                "{inner:?}"
            );
            // nor can the public lease calls swap the lent context's placeholder
            assert!(k.context.take_notify_lease().is_err());
            let mut other = Context::default();
            let lease = other.take_notify_lease().unwrap();
            assert!(k.context.restore_notify_lease(lease).is_err());
            // (the edge is still there: its uses are kept)
            assert!(!b.edge(e0).unwrap().edgeuses.is_empty());
            assert_eq!((b.notify_serial(), b.key()), (serial, key));
            squeeze_edge(k, b, env, e0, a)
        });
        r.unwrap();
        assert!(!seen.lock().unwrap().is_empty());
        assert!(k.context.user_notify_callback().is_some());
        // and the kernel context issues again once the lend is over
        let (mut b2, [a2, _, _], [e2, _]) = test_support::wire_chain(true);
        with_kernel_callbacks(&mut k, &mut b2, |k, b, env| squeeze_edge(k, b, env, e2, a2))
            .unwrap();
    }

    /// P2b: the documented boundary (stores belong to one identity domain). Two
    /// contexts, and one context across a lease that is not given back, are separate
    /// domains: each issues its first curve the same key, so a store shared across them
    /// would alias two curves.
    #[test]
    fn identities_are_unique_only_within_one_domain() {
        use crate::curve::line::Line;
        let line = || Curve::from(Line::from_points(Vec3::ZERO, Vec3::X, 3));
        let (mut a, mut b, mut c) = (line(), line(), line());
        let (mut c1, mut c2) = (Context::default(), Context::default());
        let ka = curve_identity(&mut c1, &mut a).unwrap();
        let kb = curve_identity(&mut c2, &mut b).unwrap();
        assert_eq!(ka.key, kb.key);
        // an unreturned lease leaves c1 a new domain
        drop(c1.take_notify_lease().unwrap());
        let kc = curve_identity(&mut c1, &mut c).unwrap();
        assert_eq!(kc.key, ka.key);
        // a returning object is renamed, never aliased: a now has a key c1's new
        // domain issued after c's
        let ka2 = curve_identity(&mut c1, &mut a).unwrap();
        assert_ne!(ka2.key, kc.key);
    }

    /// The kernel's callbacks are lent with their identity domain
    /// ([`with_kernel_callbacks`]): two lent edits see disjoint keys, as one context's
    /// callbacks would.
    #[test]
    fn kernel_callbacks_keep_their_identity_domain() {
        let mut k = Kernel::default();
        let seen = Seen::default();
        k.context
            .set_user_notify_callback(Some(Box::new(Rec(std::sync::Arc::clone(&seen)))));
        let edit = |k: &mut Kernel| {
            let (mut b, [a, _, _], [e0, _]) = test_support::wire_chain(true);
            let first = seen.lock().unwrap().len();
            with_kernel_callbacks(k, &mut b, |k, b, env| squeeze_edge(k, b, env, e0, a)).unwrap();
            let all = seen.lock().unwrap();
            all.get(first..)
                .unwrap()
                .iter()
                .map(|r| r.key)
                .collect::<Vec<_>>()
        };
        let one = edit(&mut k);
        let two = edit(&mut k);
        assert!(!one.is_empty());
        assert!(two.iter().all(|x| !one.contains(x)), "{one:?} {two:?}");
        assert!(k.context.user_notify_callback().is_some());
    }

    /// `DeleteFace` after region nesting merged the face's two shells: upstream's
    /// explicit `Remove` takes off only `pFU` (`pOtherShell` is gone), and `delete
    /// pOtherFU`'s destructor removes the mate from the surviving shell. A closed
    /// six-face block's two merged six-member lists keep 10 members, not 11.
    #[test]
    fn deleted_faceuses_leave_the_merged_shell() {
        use crate::topology::Faceuse;
        let mut b = Brep::new();
        let s = b
            .add_shell_record(crate::topology::Shell::default())
            .unwrap();
        let fus: Vec<FaceuseId> = (0..12)
            .map(|_| {
                b.add_faceuse_record(Faceuse {
                    shell: Some(s),
                    ..Faceuse::default()
                })
                .unwrap()
            })
            .collect();
        {
            let sh = b.shell_mut(s).unwrap();
            sh.shell_type = ShellType::Faceuse;
            sh.members = ShellMembers::Faceuses(fus.clone());
        }
        // the deleted face's use (from the first list) and its mate (from the second)
        let (fu, other_fu) = (fus[0], fus[6]);
        ignore_soft(shell::shell_remove_faceuse(&mut b, s, fu)).unwrap();
        assert_eq!(b.shell(s).unwrap().get_faceuses().unwrap().len(), 11);
        delete_faceuses(&mut b, [fu, other_fu]).unwrap();
        let left = b.shell(s).unwrap().get_faceuses().unwrap();
        assert_eq!(left.len(), 10);
        assert!(!left.contains(&fu) && !left.contains(&other_fu));
        assert_eq!(b.faceuse(other_fu).unwrap().shell, None);
        // nothing left to unlink: a second pass is a no-op
        delete_faceuses(&mut b, [fu, other_fu]).unwrap();
        assert_eq!(b.shell(s).unwrap().get_faceuses().unwrap().len(), 10);
    }

    fn with_env<R>(f: impl FnOnce(&mut NotifyEnv<'_>) -> R) -> R {
        let mut records = crate::topology::topology::NotifyRecords::new();
        let mut attributes = crate::geom::attribute::AttributeStore::default();
        let key_of = |e: EntityRef| {
            arena_identity(match e {
                EntityRef::Face(f) => CacheObject::Face(f),
                EntityRef::Edge(x) => CacheObject::Edge(x),
                EntityRef::Vertex(v) => CacheObject::Vertex(v),
                _ => CacheObject::Brep,
            })
        };
        let mut env = NotifyEnv {
            records: &mut records,
            attributes: &mut attributes,
            context: None,
            key_of: &key_of,
        };
        f(&mut env)
    }

    #[test]
    fn copy_notification_carries_flags_to_the_copy() {
        use crate::geom::attribute::ObjectKey;
        let from = ObjectRef {
            type_code: vertex::TYPE,
            key: ObjectKey(1),
        };
        let to = ObjectRef {
            type_code: vertex::TYPE,
            key: ObjectKey(2),
        };
        let mut records = crate::topology::topology::NotifyRecords::new();
        let mut src = topo::Topology::new();
        src.set_all_flags(5);
        records.insert(from.key, src);
        let mut attributes = crate::geom::attribute::AttributeStore::default();
        let key_of = |_: EntityRef| to;
        let mut env = NotifyEnv {
            records: &mut records,
            attributes: &mut attributes,
            context: None,
            key_of: &key_of,
        };
        notify_copy_with_context(&mut env, None, from, Some(to), None, None).unwrap();
        let copy = records[&to.key];
        assert_eq!(copy.all_flags(), 5);
    }

    #[test]
    fn api_copy_of_null_is_invalid_input() {
        let mut k = Kernel::default();
        let r = with_env(|env| api_brep_copy(&mut k, env, None, &source_key));
        assert!(matches!(
            r,
            Err(SolidError::Upstream {
                code: SM_ERR_INVALID_INPUT,
                ..
            })
        ));
    }

    #[test]
    fn constructor_makes_a_void_infinite_region() {
        let mut k = Kernel::default();
        let b = new_brep(&mut k, tol::USE_DEFAULT, tol::USE_DEFAULT).unwrap();
        let r = b.infinite_region().unwrap();
        assert_eq!(b.region(r).unwrap().is_void(), TRUE);
        assert_eq!(p1::regions(&b, None).unwrap(), vec![r]);
        assert!(b.region(r).unwrap().shells.is_empty());
    }

    #[test]
    fn delete_face_needs_editing_and_a_face_of_the_brep() {
        let mut k = Kernel::default();
        let mut b = Brep::new();
        let s = b.add_shell().unwrap();
        let f = b.add_face(s).unwrap();
        let mut marks = Marks::default();
        let r = with_env(|env| {
            delete_face(
                &mut k,
                &mut b,
                env,
                &mut marks,
                f,
                DeleteFaceOptions::default(),
            )
        });
        assert!(matches!(r, Err(SolidError::Upstream { code: 1001, .. })));
        b.set_editing_enabled(true);
        b.remove_from_face_list(f);
        let r = with_env(|env| {
            delete_face(
                &mut k,
                &mut b,
                env,
                &mut marks,
                f,
                DeleteFaceOptions::default(),
            )
        });
        assert!(matches!(r, Err(SolidError::Upstream { code: 1001, .. })));
        assert!(DeleteFaceOptions::default().do_region_nesting);
    }

    #[test]
    fn turn_to_nurbs_of_an_empty_brep_is_a_no_op() {
        let mut k = Kernel::default();
        let mut b = Brep::new();
        with_env(|env| turn_to_nurbs(&mut k, &mut b, env)).unwrap();
        assert!(b.faces().is_empty());
    }

    #[test]
    fn region_list_head_moves_on_removal() {
        let mut b = Brep::new();
        let r0 = b.add_region_record(Region::default()).unwrap();
        let r1 = b.add_region_record(Region::default()).unwrap();
        b.post_insert_region(r0).unwrap();
        b.post_insert_region(r1).unwrap();
        assert_eq!(b.region_list_head(), Some(r0));
        b.remove_region_from_list(r0).unwrap();
        assert_eq!(b.region_list_head(), Some(r1));
        b.remove_region_from_list(r1).unwrap();
        assert_eq!(b.region_list_head(), None);
        assert!(p1::regions(&b, None).unwrap().is_empty());
    }

    /// `PostInsert` appends at the circular list's tail (before the head), also after
    /// the head moved to a later region; `PreInsert` makes the region the head.
    #[test]
    fn region_list_appends_before_a_moved_head() {
        let mut b = Brep::new();
        let r0 = b.add_region_record(Region::default()).unwrap();
        let r1 = b.add_region_record(Region::default()).unwrap();
        b.post_insert_region(r0).unwrap();
        b.post_insert_region(r1).unwrap();
        b.set_region_list_head(Some(r1));
        assert_eq!(p1::regions(&b, None).unwrap(), vec![r1, r0]);
        // a record created on the list is appended as PostInsert
        let r2 = b
            .add_region_record(Region {
                in_brep: true,
                ..Region::default()
            })
            .unwrap();
        let r3 = b.add_region_record(Region::default()).unwrap();
        b.post_insert_region(r3).unwrap();
        assert_eq!(p1::regions(&b, None).unwrap(), vec![r1, r0, r2, r3]);
        b.remove_region_from_list(r0).unwrap();
        b.post_insert_region(r0).unwrap();
        assert_eq!(p1::regions(&b, None).unwrap(), vec![r1, r2, r3, r0]);
        b.remove_region_from_list(r1).unwrap();
        assert_eq!(b.region_list_head(), Some(r2));
        b.pre_insert_region(r1).unwrap();
        assert_eq!(p1::regions(&b, None).unwrap(), vec![r1, r2, r3, r0]);
        b.remove_region_from_list(r3).unwrap();
        assert_eq!(p1::regions(&b, None).unwrap(), vec![r1, r2, r0]);
    }

    /// Review of 255d52b7, 1: an object an edit creates cannot take a reserved key
    /// either. The caller names every existing object ordinarily, but would give the new
    /// wire edge a curve's key: the edit is refused before anything changes.
    #[test]
    fn new_objects_cannot_take_a_reserved_key() {
        use crate::curve::line::Line;
        let (mut ctx, seen) = recording_context();
        let mut k = Kernel::default();
        let (mut b, [a, _, c], _) = test_support::wire_chain(true);
        b.set_editing_enabled(true);
        let next = EntityRef::Edge(EdgeId::from_index(u32::try_from(b.edges().len()).unwrap()));
        let key_of = move |e: EntityRef| {
            if e == next {
                ObjectRef {
                    type_code: edge::TYPE,
                    key: ObjectKey(crate::geom::context::GEOMETRY_KEY_BASE | 1),
                }
            } else {
                dest_key(e)
            }
        };
        let (pa, pc) = (b.vertex(a).unwrap().point, b.vertex(c).unwrap().point);
        let curve = Curve::from(Line::from_points(pa, pc, 3));
        let ivl = curve.natural_interval();
        let edges = b.edges().len();
        let mut records = crate::topology::topology::NotifyRecords::new();
        let mut attributes = crate::geom::attribute::AttributeStore::default();
        let mut env = NotifyEnv {
            records: &mut records,
            attributes: &mut attributes,
            context: Some(&mut ctx),
            key_of: &key_of,
        };
        let r = make_wire_edge(
            &mut k,
            &mut b,
            &mut env,
            None,
            a,
            c,
            curve,
            &ivl,
            OrientType::Same,
        );
        assert_eq!(r, Err(RESERVED_KEY));
        assert_eq!(b.edges().len(), edges);
        assert!(records.is_empty() && seen.lock().unwrap().is_empty());
    }

    /// Review of 255d52b7, 2: each copy is its own object, named by the caller's
    /// destination mapper (`env.key_of` of the B-rep itself) before anything is
    /// notified, so two live copies in one store keep their own attributes.
    #[test]
    fn copies_take_their_own_caller_identity() {
        use crate::geom::attribute::{Attribute, Behavior};
        let mut k = Kernel::default();
        let (mut src1, _, _) = test_support::wire_chain(true);
        let (mut src2, _, _) = test_support::wire_chain(true);
        src1.set_key(ObjectKey(100));
        src2.set_key(ObjectKey(200));
        let mut records = crate::topology::topology::NotifyRecords::new();
        let mut attributes = crate::geom::attribute::AttributeStore::default();
        for (key, value) in [(src1.key(), 73), (src2.key(), 91)] {
            let h = attributes
                .insert(Attribute::long(42, value, Behavior::Copy))
                .unwrap();
            attributes.add_attribute(key, h).unwrap();
        }
        let (dk1, dk2) = (
            |e: EntityRef| caller_key(e, 3 << 40),
            |e: EntityRef| caller_key(e, 4 << 40),
        );
        let (sk1, sk2) = (
            |e: EntityRef| caller_key(e, 5 << 40),
            |e: EntityRef| caller_key(e, 6 << 40),
        );
        let mut env = NotifyEnv {
            records: &mut records,
            attributes: &mut attributes,
            context: None,
            key_of: &dk1,
        };
        let copy1 = copy_brep(&mut k, &mut env, &src1, &sk1, None, None, true, false).unwrap();
        env.key_of = &dk2;
        let copy2 = copy_brep(&mut k, &mut env, &src2, &sk2, None, None, true, false).unwrap();
        assert_eq!(copy1.key(), dk1(brep_entity()).key);
        assert_eq!(copy2.key(), dk2(brep_entity()).key);
        let value = |key: ObjectKey| {
            let h = env.attributes.find_attribute(key, 42).unwrap();
            env.attributes.get(h).unwrap().long_value()
        };
        assert_eq!(
            (value(copy1.key()), value(copy2.key())),
            (Some(73), Some(91))
        );
        assert_eq!((src1.key(), src2.key()), (ObjectKey(100), ObjectKey(200)));
    }

    /// Review of 255d52b7, 2: two new B-reps report their construction under two
    /// identities (the caller holds no key for them yet), and a later kernel-named edit
    /// names the B-rep as its construction did.
    #[test]
    fn new_breps_construct_under_their_own_identities() {
        let mut k = Kernel::default();
        let seen = Seen::default();
        k.context
            .set_user_notify_callback(Some(Box::new(Rec(std::sync::Arc::clone(&seen)))));
        let mut b1 = new_brep(&mut k, tol::USE_DEFAULT, tol::USE_DEFAULT).unwrap();
        let b2 = new_brep(&mut k, tol::USE_DEFAULT, tol::USE_DEFAULT).unwrap();
        let keys: Vec<ObjectKey> = seen.lock().unwrap().iter().map(|r| r.key).collect();
        // (each construction names its B-rep as caller and as data1)
        assert_eq!(keys.len(), 4);
        assert_ne!(keys[0], keys[2]);
        assert_eq!((b1.key(), b2.key()), (ObjectKey(0), ObjectKey(0)));
        let named = with_kernel_callbacks(&mut k, &mut b1, |_, b, _| Ok(p2::brep_ref(b).key));
        assert_eq!(named.unwrap(), keys[0]);
    }

    /// Review of 255d52b7, 3: a plain `topology.brep_p5` edit runs under the kernel's
    /// identities, so the same edit of two live B-reps, through one callback, never
    /// names two objects with one key; and one nested in a lent edit is refused before
    /// anything changes.
    #[test]
    fn plain_p5_edits_of_two_breps_never_share_a_key() {
        use crate::topology::brep::p5;
        let mut k = Kernel::default();
        let seen = Seen::default();
        k.context
            .set_user_notify_callback(Some(Box::new(Rec(std::sync::Arc::clone(&seen)))));
        let mut keys = Vec::new();
        for label in [100, 200] {
            let (mut b, [_, v, _], [_, e1]) = test_support::wire_chain(true);
            b.set_key(ObjectKey(label));
            b.set_editing_enabled(true);
            let first = seen.lock().unwrap().len();
            p5::delete_topological_vertex(&mut k, &mut b, true, Some(v), Some(e1)).unwrap();
            let all = seen.lock().unwrap();
            let topology: Vec<ObjectKey> = all
                .get(first..)
                .unwrap()
                .iter()
                .map(|r| r.key)
                .filter(|x| x.0 < crate::geom::context::GEOMETRY_KEY_BASE)
                .collect();
            assert!(!topology.is_empty());
            keys.push(topology);
        }
        assert!(keys[1].iter().all(|x| !keys[0].contains(x)), "{keys:?}");
        // nested: refused, the B-rep unchanged
        let (mut b, _, _) = test_support::wire_chain(true);
        b.set_editing_enabled(true);
        let before: Vec<Point3> = b.vertices().iter().map(|(_, v)| v.point).collect();
        let r = with_kernel_callbacks(&mut k, &mut b, |k, b, _| {
            let placement = crate::geom::axis2_placement::Axis2Placement::default();
            p5::transform(k, b, &placement, Some(Vec3::new(2.0, 1.0, 1.0)), false)
        });
        assert!(matches!(r, Err(SolidError::InvalidInput { .. })), "{r:?}");
        let after: Vec<Point3> = b.vertices().iter().map(|(_, v)| v.point).collect();
        assert_eq!(before, after);
    }

    /// Codex re-check of 6b04141d, M1: `CopyVertices` allocates for every entry of its
    /// list, repeated entries included, so the keys checked before the copy cover the
    /// list's length, not the arenas' (Codex's repro: one shell vertex copied 30 times,
    /// the 30th new vertex keyed like a curve, is refused before anything changes).
    #[test]
    fn copy_vertices_checks_a_key_for_every_entry() {
        let mut k = Kernel::default();
        let mut src = Brep::new_with_infinite_region().unwrap();
        let mut records = crate::topology::topology::NotifyRecords::new();
        let mut attributes = crate::geom::attribute::AttributeStore::default();
        let inf = src.infinite_region();
        let (_, v) = {
            let mut env = NotifyEnv {
                records: &mut records,
                attributes: &mut attributes,
                context: None,
                key_of: &source_key,
            };
            super::super::p9::make_shell_vertex(&mut src, inf, Vec3::ZERO, &mut env).unwrap()
        };
        let mut dest = Brep::new_with_infinite_region().unwrap();
        let reserved = EntityRef::Vertex(VertexId::from_index(29));
        let key_of = move |e: EntityRef| {
            if e == reserved {
                ObjectRef {
                    type_code: vertex::TYPE,
                    key: ObjectKey(crate::geom::context::GEOMETRY_KEY_BASE | 1),
                }
            } else {
                dest_key(e)
            }
        };
        let (mut ctx, seen) = recording_context();
        let mut env = NotifyEnv {
            records: &mut records,
            attributes: &mut attributes,
            context: Some(&mut ctx),
            key_of: &key_of,
        };
        let r = copy_vertices(&mut k, &src, &[v; 30], &mut dest, &mut env, &source_key);
        assert_eq!(r, Err(RESERVED_KEY));
        assert!(dest.vertices().is_empty());
        assert!(seen.lock().unwrap().is_empty());
    }

    /// Codex re-check of 6b04141d, M3: a plain part-5 edit's environment has a record
    /// for every existing object from the start, so on the A--B--C wire
    /// `DeleteTopologicalVertex(B)` delivers its edge merge (`SM_NO_MERGE_IN_BREP` of the
    /// two edges, sent before any p3 edit runs) to the kernel's callback.
    #[test]
    fn plain_p5_edits_deliver_their_first_notifications() {
        use crate::topology::brep::p5;
        type Events = std::sync::Arc<std::sync::Mutex<Vec<(NotifyOperation, Option<ObjectRef>)>>>;
        #[derive(Debug)]
        struct Ops(Events);
        impl crate::geom::context::NotifyCallback for Ops {
            fn execute(
                &mut self,
                operation: NotifyOperation,
                _caller: ObjectRef,
                data1: Option<ObjectRef>,
                _data2: Option<ObjectRef>,
                _data3: Option<ObjectRef>,
            ) -> Result<()> {
                self.0.lock().unwrap().push((operation, data1));
                Ok(())
            }
        }
        let events = Events::default();
        let mut k = Kernel::default();
        k.context
            .set_user_notify_callback(Some(Box::new(Ops(std::sync::Arc::clone(&events)))));
        let (mut b, [_, v, _], [_, e1]) = test_support::wire_chain(true);
        b.set_editing_enabled(true);
        p5::delete_topological_vertex(&mut k, &mut b, true, Some(v), Some(e1)).unwrap();
        let edge_merges = events
            .lock()
            .unwrap()
            .iter()
            .filter(|(op, d1)| {
                *op == NotifyOperation::MergeInBrep && d1.is_some_and(|d| d.type_code == edge::TYPE)
            })
            .count();
        assert!(edge_merges >= 1);
    }

    /// One edit's [`check_keys_once`] and [`seed_records_once`] of `b` into `records`,
    /// asserting they decide and seed as the full walks ([`check_keys`],
    /// [`seed_records`] into `full`).
    fn edit_once(
        b: &mut Brep,
        key_of: &dyn Fn(EntityRef) -> ObjectRef,
        records: &mut topo::NotifyRecords,
        full: &mut topo::NotifyRecords,
    ) -> Result<()> {
        let window = allocation_window(b, None, 0);
        let expected = check_keys(&Context::default(), b, key_of, window);
        let id = progress_id(b, None, key_of);
        let instance = b.progress_tag();
        let progress = (id.clone(), std::sync::Arc::clone(&instance));
        let checked = check_keys_once(records, progress, None, b, key_of, window);
        assert_eq!(checked, expected);
        checked?;
        seed_records_once(records, (id, instance), b, key_of)?;
        seed_records(full, b, key_of);
        assert_eq!(*records, *full);
        Ok(())
    }

    /// The per-edit check and seeding only visit the slots added since the last edit
    /// of the same B-rep value (and the emptied slots a window reaches again), yet
    /// decide and seed exactly as the full walks: through removals and additions,
    /// after a record leaves the store, and on a clone (another value under the same
    /// identity, whose objects the first value's progress says nothing about).
    #[test]
    fn incremental_check_and_seeding_match_the_full_walks() {
        let mut b = Brep::default();
        let vs: Vec<VertexId> = (0..4)
            .map(|i| b.add_vertex(Point3::new(f64::from(i), 0.0, 0.0)).unwrap())
            .collect();
        // (vertex 0 takes a reserved key: refused while it is live or in a window)
        let reserved = EntityRef::Vertex(vs[0]);
        let key_of = move |e: EntityRef| ObjectRef {
            key: if e == reserved {
                ObjectKey(crate::geom::context::GEOMETRY_KEY_BASE | 1)
            } else {
                dest_key(e).key
            },
            ..dest_key(e)
        };
        let mut records = topo::NotifyRecords::new();
        let mut full = topo::NotifyRecords::new();
        let mut copy = b.clone();
        b.remove_vertex_record(vs[0]).unwrap();
        b.remove_vertex_record(vs[1]).unwrap();
        edit_once(&mut b, &key_of, &mut records, &mut full).unwrap();
        b.add_vertex(Point3::ZERO).unwrap();
        edit_once(&mut b, &key_of, &mut records, &mut full).unwrap();
        let key = dest_key(EntityRef::Vertex(vs[2])).key;
        records.remove(&key);
        full.remove(&key);
        edit_once(&mut b, &key_of, &mut records, &mut full).unwrap();
        // the clone still has vertex 0, and vertex 1, which `b` lost before its edits
        assert_eq!(
            edit_once(&mut copy, &key_of, &mut records, &mut full),
            Err(RESERVED_KEY)
        );
        copy.remove_vertex_record(vs[0]).unwrap();
        edit_once(&mut copy, &key_of, &mut records, &mut full).unwrap();
        assert!(records.contains_key(&dest_key(EntityRef::Vertex(vs[1])).key));
        // a window that reaches vertex 0's emptied slot again refuses it
        for v in b.vertices().ids().collect::<Vec<_>>() {
            b.remove_vertex_record(v).unwrap();
        }
        assert_eq!(
            edit_once(&mut b, &key_of, &mut records, &mut full),
            Err(RESERVED_KEY)
        );
    }
}

/// Test fixtures shared with the callers' tests.
#[cfg(test)]
pub(crate) mod test_support {
    use crate::curve::curve::Curve;
    use crate::curve::line::Line;
    use crate::geom::extent1d::Extent1d;
    use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
    use crate::topology::shell::{ShellMembers, ShellType};
    use crate::topology::{
        Brep, Edge, EdgeId, Edgeuse, EdgeuseId, EdgeuseOwner, Region, Shell, Vertex, VertexId,
        Vertexuse, VertexuseId, VertexuseOwner,
    };

    /// A wire chain A--B--C (edge uses `2i` / `2i+1` of edge `i` at `v_i` / `v_{i+1}`) in
    /// one wire shell of the infinite region, with line curves when `curves`.
    pub(crate) fn wire_chain(curves: bool) -> (Brep, [VertexId; 3], [EdgeId; 2]) {
        let mut b = Brep::new();
        let r = b
            .add_region_record(Region {
                in_brep: true,
                ..Region::default()
            })
            .unwrap();
        b.set_infinite_region_record(Some(r));
        let s = b
            .add_shell_record(Shell {
                region: Some(r),
                shell_type: ShellType::Wire,
                members: ShellMembers::Edgeuse(EdgeuseId::from_index(0)),
                ..Shell::default()
            })
            .unwrap();
        b.region_mut(r).unwrap().shells.push(s);
        let v: Vec<VertexId> = (0..3)
            .map(|i| {
                b.add_vertex_record(Vertex {
                    point: crate::math::Vec3::new(f64::from(i), 0.0, 0.0),
                    tolerance: 1e-6,
                    ..Vertex::default()
                })
                .unwrap()
            })
            .collect();
        let mut es = Vec::new();
        for i in 0..2u32 {
            let (p, q) = (v[i as usize], v[i as usize + 1]);
            let curve = curves.then(|| {
                let (pp, qq) = (b.vertex(p).unwrap().point, b.vertex(q).unwrap().point);
                Curve::from(Line::from_points(pp, qq, 3))
            });
            es.push(
                b.add_edge_record(Edge {
                    vertices: [p, q],
                    edgeuses: vec![
                        EdgeuseId::from_index(2 * i),
                        EdgeuseId::from_index(2 * i + 1),
                    ],
                    interval: Extent1d::from_min_max(0.0, 1.0),
                    tolerance: 1e-6,
                    curve,
                })
                .unwrap(),
            );
        }
        for i in 0..4u32 {
            b.add_edgeuse_record(Edgeuse {
                edge: Some(EdgeId::from_index(i / 2)),
                owner: EdgeuseOwner::Shell(Some(s)),
                vertexuse: Some(VertexuseId::from_index(i)),
                orientation: if i % 2 == 0 {
                    OrientType::Same
                } else {
                    OrientType::Opposite
                },
                ..Edgeuse::default()
            })
            .unwrap();
        }
        for (i, &vx) in [v[0], v[1], v[1], v[2]].iter().enumerate() {
            let vu = b
                .add_vertexuse_record(Vertexuse {
                    vertex: Some(vx),
                    owner: VertexuseOwner::Edgeuse(Some(EdgeuseId::from_index(i as u32))),
                    ..Vertexuse::default()
                })
                .unwrap();
            b.vertex_mut(vx).unwrap().vertexuses.push(vu);
        }
        (b, [v[0], v[1], v[2]], [es[0], es[1]])
    }
}
