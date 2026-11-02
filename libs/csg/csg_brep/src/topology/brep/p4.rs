//! `SmBrep` part 4: deleting edges and vertices from a B-rep (the Euler-style
//! topology surgery behind face merging and wire removal), stitching faces,
//! shrinking geometry to the used domains, and the B-rep destructor.
//!
//! Upstream: `source/SMLib/src/SmBrep.cpp`, `source/SMLib/inc/SmBrep.h`.
//!
//! | Rust                       | Upstream                      |
//! |----------------------------|-------------------------------|
//! | [`delete_edge`]            | `SmBrep::DeleteEdge`          |
//! | [`delete_vertex`]          | `SmBrep::DeleteVertex`        |
//! | [`stitch_faces`]           | `SmBrep::StitchFaces`         |
//! | [`StitchOutcome`]          | its `rlStitchedEdges` .. `rdMaxEdgeFaceTrimCurveGap` outputs |
//! | [`shrink_geometry`]        | `SmBrep::ShrinkGeometry`      |
//! | [`destroy`]                | `SmBrep::~SmBrep`             |
//! | [`DeleteEnv`]              | the objects' addresses and `SmTopology` members the notifications use |
//!
//! Deviations:
//! - **Deletion.** Upstream `delete`s topology objects; the port removes their
//!   records from the B-rep arenas ([`Brep::remove_edge_record`] and siblings), so the
//!   ids of the other records stay valid. `SmOwningTopology::Remove` / `PostInsert` on
//!   an owner's list are the list edits of the arena model: the id leaves / joins the
//!   owner's `Vec` (`PostInsert` appends) and the member's back link is cleared / set.
//!   A `Remove` whose member is not owned by the owner is `SM_ERR` with nothing
//!   changed, as upstream; `DeleteEdge` and `DeleteVertex` ignore that status, as upstream.
//!   `m_p*ListHead->Remove` and the following `delete` are one record removal, which
//!   also takes the id off the B-rep's face, edge or vertex list
//!   ([`Brep::face_list`] and siblings, unit `topology.brep_p1`).
//! - **Shell lists.** A shell's `m_pList` / `m_tShellType` are
//!   [`crate::topology::Shell::members`] / `shell_type` (unit `topology.shell`); the
//!   direct member writes of `DeleteEdge` (`m_pList = ..; UpdateListSize()`, the list
//!   size is not a record) set them as upstream. A face use's `Remove` / `PostInsert`
//!   on a shell edit the face-use list (and the shell's face list with it); face uses
//!   with literal sibling links are relinked to the list order.
//! - **Surface identity.** Upstream compares `SmSurface*` pointers. As in
//!   [`crate::topology::edge`], a surface is named by the face holding it: two faces
//!   have the same surface when they are the same face (both with a surface).
//!   `pOptSurfaceToKeep` is the face whose surface is to be kept.
//! - **Null pointers.** Where upstream dereferences a null pointer or casts a list
//!   owner to the wrong kind (undefined behaviour), the port returns
//!   [`SolidError::InvalidInput`]; `NER` checks return the `SM_ERR_NULL_POINTER`
//!   status, as upstream.
//! - **Surface swap.** `DeleteEdge` swaps the two faces' surfaces through
//!   `SmFace::SetSurface` (ported in `topology.face_p1`, which moves an owned
//!   `Surface`): the swap takes the surface out of the deleted face and hands the
//!   replaced one back.
//! - **`SmLockBrepBox`** is RAII upstream; `StitchFaces` holds `topology.brep_p1`'s
//!   [`LockBrepBox`] guard (unlocked on drop, on every path). Its lock
//!   (`SmBrep::LockBoundingBox`, unit `query.brep`) is not ported yet.
//! - **Collections.** `GetFaces` / `GetEdges` / `GetVertices` / `GetRegions` /
//!   `GetSurfaces` / `GetCurves` are `topology.brep_p1`'s; a surface (curve) is named
//!   by the face (edge) holding it (identity of the returned reference).
//! - **Notifications** go to the ported handlers (`SmEdge::Notify`,
//!   `SmVertex::Notify`, `SmFace::Notify`, `SmBrep::Notify`) with the caller's
//!   [`DeleteEnv`] and the kernel's context, and with upstream's data arguments (the
//!   B-rep is named by [`Brep::key`]); `SmFace::Notify` and `SmBrep::Notify` run without
//!   a cache (the trimmed-surface and B-rep spatial caches are not part of the arena
//!   model). `~SmBrep` has no caller state and notifies through call-local records.
//!   Their statuses are ignored upstream; [`SolidError::Unported`] propagates.
//! - **Destructor notifications.** Upstream's `delete` of a face, edge, edge use or
//!   shell runs its destructor, which notifies `SM_NO_DESTRUCTION` (`~SmFace`,
//!   `SmFace.cpp:94`; `~SmEdge`, `SmEdge.cpp:85`, which also sets and restores the
//!   context's boolean flag around it; `~SmEdgeuse`, `SmEdgeuse.cpp:76`; `~SmShell`,
//!   `SmShell.cpp:55`). `DeleteEdge` and `DeleteVertex` delete their edges, edge uses,
//!   merged-away faces and vertex shells with [`crate::topology::edge::destroy`],
//!   [`crate::topology::edgeuse::p2::destroy`], [`crate::topology::face::p3::destroy`]
//!   and [`crate::topology::shell::destroy`] (the notification, `~SmEdge`'s flag
//!   toggle and `~SmAObject`'s attribute cleanup). Records without a notifying
//!   destructor (face uses, loops, loop uses, vertices, vertex uses, regions) are
//!   removed without `ClearAttributes` (not modelled), and [`destroy`]'s whole-B-rep
//!   teardown removes its records without them.
//! - **`TBB`.** `ShrinkGeometry`'s `CreateUVTrimCurvesParallel` (`SM_USE_TBB`,
//!   `SmBrep.cpp:32253`) is not compiled in the reference build and not ported.
//! - **Loops.** A loop made by splitting a loop gets the split face as
//!   [`crate::topology::Loop::face`]; `Loop::edges` stays empty (the loop uses carry
//!   the topology).
//! - **Destructor.** [`destroy`] runs the destructor's observable work (editing
//!   enabled, the context's boolean flag set for the duration and restored, the
//!   notifications) and empties the arenas, regions included (`delete pR`), and clears
//!   the region list head and the infinite region (`m_pInfiniteRegion = NULL`); Rust
//!   drops the records themselves. A loop without a loop use is skipped where upstream
//!   dereferences `pL->m_pLU` (undefined behaviour only).
//!   The flag is restored on every return, including [`SolidError::Unported`].
//! - **Stitching.** `StitchFaces` builds the `SmStitch` (`heal.stitch_p1`) with its
//!   options and runs `SmStitch::DoStitching` (`heal.stitch_p2`) without the min-gap
//!   outputs; its edge counts (`ULONG`) are `u32` in [`StitchOutcome`] (a count past
//!   `u32::MAX` is [`SolidError::InvalidInput`]).
//! - **Bounds.** No recursion; every loop is bounded by a record list. The edge-use
//!   walks are those of the ported `CollectEdgeuses` (bounded there).

use crate::error::{ignore_status, ignored, EntityRef, Result, SolidError};
use crate::geom::a_object::BREP_TYPE;
use crate::geom::attribute::AttributeStore;
use crate::geom::context::{Context, MarkLock, ALL_MARKS};
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::object::{NotifyOperation, ObjectRef};
use crate::heal::stitch::p1::{DefaultStitchCallback, Stitch, StitchCallback};
use crate::heal::stitch::p2::MinGapRequest;
use crate::kernel::Kernel;
use crate::math::{Point2, Vec3, EFF_ZERO};
use crate::nurbs::b_spline_curve_n_lib::TrimMode;
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::nurbs::knots::Side;
use crate::query::face::p2 as query_face2;
use crate::topology::brep::p1::{self as brep1, LockBrepBox};
use crate::topology::brep::p2::{self as brep2, NotifyArg};
use crate::topology::edge::{self, NotifyEnv};
use crate::topology::edgeuse::p1 as eu1;
use crate::topology::edgeuse::p2 as eu2;
use crate::topology::edgeuse::p3 as eu3;
use crate::topology::face::p1 as face1;
use crate::topology::face::p2 as face2;
use crate::topology::face::p3 as face3;
use crate::topology::shell::{self, ShellMembers, ShellType};
use crate::topology::topology_traverser::{self, Entity, Marks};
use crate::topology::{
    loopuse, vertex, vertexuse, Brep, EdgeId, EdgeuseId, EdgeuseOwner, FaceId, FaceuseId, Loop,
    LoopId, Loopuse, LoopuseId, LoopuseStart, RegionId, Shell, ShellId, VertexId, VertexuseId,
    VertexuseOwner,
};

const OP_DELETE_EDGE: &str = "topology::brep::p4::delete_edge";
const OP_DELETE_VERTEX: &str = "topology::brep::p4::delete_vertex";
const OP_SHRINK: &str = "topology::brep::p4::shrink_geometry";
const OP_STITCH: &str = "topology::brep::p4::stitch_faces";
const OP_DESTROY: &str = "topology::brep::p4::destroy";

/// The caller's state the notifications of [`delete_edge`] and [`delete_vertex`]
/// go through (as [`NotifyEnv`], with the kernel's context).
pub struct DeleteEnv<'a> {
    /// The topology records (`SmTopology` members) of the notified objects.
    pub records: &'a mut crate::topology::topology::NotifyRecords,
    /// The attribute store.
    pub attributes: &'a mut AttributeStore,
    /// The identity each topology entity is notified under.
    pub key_of: &'a dyn Fn(EntityRef) -> ObjectRef,
    /// The context the objects notify (`GetContext`); `None` for the kernel's.
    pub context: Option<&'a mut Context>,
}

impl<'a> DeleteEnv<'a> {
    /// The [`DeleteEnv`] over a [`NotifyEnv`]'s records, attributes and identities.
    pub fn from_notify(env: &'a mut NotifyEnv<'_>) -> Self {
        DeleteEnv {
            records: &mut *env.records,
            attributes: &mut *env.attributes,
            key_of: env.key_of,
            context: env.context.as_deref_mut(),
        }
    }
}

impl core::fmt::Debug for DeleteEnv<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("DeleteEnv")
            .field("records", &self.records.len())
            .finish_non_exhaustive()
    }
}

/// Runs `f` with a [`DeleteEnv`] on topology records and an attribute store local to
/// the call, and entities notified under their type code and arena index (as
/// [`loopuse::face_notify_edit`] does): the environment of a caller that holds no
/// notification state (the B-rep keeps no attributes for its records; upstream on
/// objects without attributes).
pub(crate) fn with_local_env<T>(f: impl FnOnce(&mut DeleteEnv<'_>) -> Result<T>) -> Result<T> {
    let mut records = crate::topology::topology::NotifyRecords::new();
    let mut attributes = AttributeStore::default();
    let mut env = DeleteEnv {
        records: &mut records,
        attributes: &mut attributes,
        key_of: &loopuse::notify_ref,
        context: None,
    };
    f(&mut env)
}

// ---- notifications and face queries ---------------------------------------------------------

/// The B-rep's own identity (`this`) as a notification argument ([`Brep::key`]).
fn brep_ref(b: &Brep) -> ObjectRef {
    ObjectRef {
        type_code: BREP_TYPE,
        key: b.key(),
    }
}

/// `SmBrep::Notify(op, data1, data2, data3)` (unit `topology.brep_p2`,
/// [`brep2::notify`]) with upstream's data arguments, the caller's records, attributes
/// and identities and the kernel's context. The B-rep's spatial cache is not part of
/// the arena model (`None`: the B-rep has none, as for [`loopuse`]'s notifications).
fn brep_notify(
    k: &mut Kernel,
    b: &Brep,
    env: &mut DeleteEnv<'_>,
    operation: NotifyOperation,
    data: [Option<NotifyArg>; 3],
) -> Result<()> {
    brep_notify_with_identity(k, b, env, operation, data, None)
}

fn brep_notify_with_identity(
    k: &mut Kernel,
    b: &Brep,
    env: &mut DeleteEnv<'_>,
    operation: NotifyOperation,
    data: [Option<NotifyArg>; 3],
    body: Option<ObjectRef>,
) -> Result<()> {
    let key_of = env.key_of;
    let mut n = NotifyEnv {
        records: env.records,
        attributes: env.attributes,
        context: Some(env.context.as_deref_mut().unwrap_or(&mut k.context)),
        key_of,
    };
    match body {
        Some(body) => brep2::notify_with_body_identity(b, &mut n, None, operation, data, body),
        None => brep2::notify(b, &mut n, None, operation, data),
    }
}

/// The B-rep as a notification argument (`this`).
fn brep_arg(b: &Brep) -> NotifyArg {
    NotifyArg::Object(brep_ref(b))
}

fn brep_arg_with_identity(b: &Brep, body: Option<ObjectRef>) -> NotifyArg {
    NotifyArg::Object(body.unwrap_or_else(|| brep_ref(b)))
}

/// `pFace->Notify(op, pFace, this, NULL)`: [`face3::notify`] with the caller's records,
/// attributes and identities and the kernel's context. The trimmed-surface cache is
/// not part of the arena model (`None`, as in [`crate::topology::loopuse`]).
fn face_notify(
    k: &mut Kernel,
    b: &mut Brep,
    env: &mut DeleteEnv<'_>,
    f: FaceId,
    operation: NotifyOperation,
    body: Option<ObjectRef>,
) -> Result<()> {
    let key_of = env.key_of;
    let data = [
        Some(key_of(EntityRef::Face(f))),
        Some(body.unwrap_or_else(|| brep_ref(b))),
        None,
    ];
    let mut n = NotifyEnv {
        records: env.records,
        attributes: env.attributes,
        context: Some(env.context.as_deref_mut().unwrap_or(&mut k.context)),
        key_of,
    };
    face3::notify(b, f, &mut n, None, operation, data)
}

fn remove_uv_trim_curves(
    k: &mut Kernel,
    b: &mut Brep,
    env: &mut DeleteEnv<'_>,
    face: FaceId,
    body: Option<ObjectRef>,
) -> Result<()> {
    if body.is_none() {
        return face1::remove_uv_trim_curves(b, face);
    }
    let mut notify = NotifyEnv {
        records: env.records,
        attributes: env.attributes,
        key_of: env.key_of,
        context: Some(env.context.as_deref_mut().unwrap_or(&mut k.context)),
    };
    face1::remove_uv_trim_curves_with_notifications(b, face, &mut notify)
}

/// `SmFace::ComputeLoopOrientation(pLoop, eOrient)` (default `pUVBounds = NULL`,
/// `bDo3dLoopComputation = FALSE`). A null loop is dereferenced upstream.
fn face_compute_loop_orientation(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
    l: Option<LoopId>,
) -> Result<OrientType> {
    let l = deref(l, OP_DELETE_EDGE)?;
    face3::compute_loop_orientation(k, b, f, l, None, false)
}

/// `SmFace::GetVertices(vertices)` (default `bTgtLoopsOnly = FALSE`, no target loops).
fn face_vertices(b: &Brep, f: FaceId) -> Result<Vec<VertexId>> {
    query_face2::face_vertices(b, f, false, None)
}

/// `SmFace::GetEdges(edges)` (default arguments).
fn face_edges(b: &Brep, f: FaceId) -> Result<Vec<EdgeId>> {
    topology_traverser::face_edges(b, f)
}

// ---- shell and region lists (topology.shell) -----------------------------------------------

/// `SmShell::GetRegion`.
fn shell_region(b: &Brep, s: ShellId) -> Result<Option<RegionId>> {
    shell::shell_region(b, s)
}

/// `SmShell::IsWireShell`.
fn shell_is_wire_shell(b: &Brep, s: ShellId) -> Result<bool> {
    Ok(b.shell(s)?.is_wire_shell())
}

/// A shell's list head (`m_pList`): its first face use, its edge use or its vertex
/// use; `None` for a `NULL` head.
fn shell_head(b: &Brep, s: ShellId) -> Result<Option<EntityRef>> {
    Ok(match &b.shell(s)?.members {
        ShellMembers::Empty => None,
        ShellMembers::Faceuses(list) => list.first().copied().map(EntityRef::Faceuse),
        ShellMembers::Edgeuse(eu) => Some(EntityRef::Edgeuse(*eu)),
        ShellMembers::Vertexuse(vu) => Some(EntityRef::Vertexuse(*vu)),
    })
}

/// `pShell->m_pList = head; pShell->UpdateListSize()` (the list size is not a record
/// of the arena model), then `pShell->m_tShellType = ty` when given.
fn set_shell_head(
    b: &mut Brep,
    s: ShellId,
    head: ShellMembers,
    ty: Option<ShellType>,
) -> Result<()> {
    let shell = b.shell_mut(s)?;
    shell.members = head;
    if let Some(ty) = ty {
        shell.shell_type = ty;
    }
    Ok(())
}

/// A vertex-use list head (`NULL` for `None`).
fn vu_head(vu: Option<VertexuseId>) -> ShellMembers {
    vu.map_or(ShellMembers::Empty, ShellMembers::Vertexuse)
}

/// An edge-use list head (`NULL` for `None`).
fn eu_head(eu: Option<EdgeuseId>) -> ShellMembers {
    eu.map_or(ShellMembers::Empty, ShellMembers::Edgeuse)
}

/// `pRegion->PostInsert(pShell)` with `pRegion = pOldShell->GetRegion()` (dereferenced
/// unchecked upstream): the shell joins the end of the region's shell list.
fn region_post_insert(b: &mut Brep, r: Option<RegionId>, s: ShellId) -> Result<()> {
    let r = deref(r, OP_DELETE_EDGE)?;
    b.region_mut(r)?.shells.push(s);
    b.shell_mut(s)?.region = Some(r);
    Ok(())
}

/// `SmRegion::GetShells(shells)` (no filter).
fn region_shells(b: &Brep, r: RegionId) -> Result<Vec<ShellId>> {
    b.region(r)?.get_shells(None, |_, _| Ok(true))
}

/// Face uses with literal sibling links (`Faceuse::sibling_links`) keep them in step
/// with the shell's face-use list after a `Remove` / `PostInsert` (upstream relinks
/// the neighbours); lists whose members derive their links from the list are left so.
fn sync_faceuse_links(b: &mut Brep, s: ShellId) -> Result<()> {
    let members = b.shell(s)?.get_faceuses()?;
    let mut literal = false;
    for &fu in &members {
        literal |= b.faceuse(fu)?.sibling_links.is_some();
    }
    if !literal {
        return Ok(());
    }
    let mut previous = members.last().copied();
    for (&member, &next) in members.iter().zip(members.iter().cycle().skip(1)) {
        b.faceuse_mut(member)?.sibling_links = Some((Some(next), previous));
        previous = Some(member);
    }
    Ok(())
}

/// `SmOwningTopology::PostInsert` of a face use at the end of a shell's list (the
/// arena model's face list of the shell follows).
fn shell_post_insert_faceuse(b: &mut Brep, s: ShellId, fu: FaceuseId) -> Result<()> {
    let members = &mut b.shell_mut(s)?.members;
    match members {
        ShellMembers::Empty => *members = ShellMembers::Faceuses(vec![fu]),
        ShellMembers::Faceuses(list) => list.push(fu),
        // upstream would splice the face use into the edge-use / vertex-use ring
        _ => {
            return Err(SolidError::invalid_input(
                OP_DELETE_EDGE,
                "face use inserted into a wire or vertex shell list",
            ))
        }
    }
    let face = b.faceuse(fu)?.face;
    b.faceuse_mut(fu)?.shell = Some(s);
    if let Some(f) = face {
        b.shell_mut(s)?.faces.push(f);
    }
    sync_faceuse_links(b, s)
}

// ---- B-rep collections (topology.brep_p1) --------------------------------------------------

/// `SmBrep::GetSurfaces(surfaces)` (no filter): each surface named by the face holding
/// it (the faces whose surface is owned by the face, in face order).
fn brep_get_surfaces(b: &Brep) -> Result<Vec<FaceId>> {
    let surfaces = brep1::surfaces(b, None)?;
    let faces = brep1::faces(b, None)?;
    let mut out = Vec::new();
    for s in surfaces {
        for &f in &faces {
            if b.face(f)?
                .surface
                .as_ref()
                .is_some_and(|x| core::ptr::eq(x, s))
            {
                out.push(f);
                break;
            }
        }
    }
    Ok(out)
}

/// `SmBrep::GetCurves(curves)` (no filter): each curve named by the edge holding it
/// (the edges whose curve is owned by the edge, in edge order).
fn brep_get_curves(b: &Brep) -> Result<Vec<EdgeId>> {
    let curves = brep1::curves(b, None)?;
    let edges = brep1::edges(b, None)?;
    let mut out = Vec::new();
    for c in curves {
        for &e in &edges {
            if b.edge(e)?
                .curve
                .as_ref()
                .is_some_and(|x| core::ptr::eq(x, c))
            {
                out.push(e);
                break;
            }
        }
    }
    Ok(out)
}

/// `SmBrep::UnLockBoundingBox` (unit `query.brep`).
fn unlock_bounding_box(b: &mut Brep) {
    crate::query::brep::unlock_bounding_box(b);
}

// ---- helpers -------------------------------------------------------------------------------

/// `SmFace::CalculateUVDomainFromUVTrimCurves(domain)` with upstream's defaults
/// (`bLimitDomain = TRUE`, no loop gap outputs): the domain it returns.
fn face_calculate_uv_domain(k: &Kernel, b: &mut Brep, f: FaceId) -> Result<Extent2d> {
    face1::calculate_uv_domain_from_uv_trim_curves(k, b, f, true, false, false).map(|o| o.domain)
}

/// `SmFace::RemoveLoop(pLoop)`. Upstream notifies the face (`SmFace`'s own
/// `Notify(SM_NO_PRE_EDIT, this, SM_NO_GET_BREP(this), NULL)`, as in
/// [`face1::remove_loop`]) and then dereferences the loop, so a null loop is the
/// null-dereference error after the notification.
fn face_remove_loop(b: &mut Brep, f: FaceId, l: Option<LoopId>) -> Result<()> {
    if let Some(l) = l {
        return face1::remove_loop(b, f, l);
    }
    ignore_status(loopuse::face_notify_edit(b, f, NotifyOperation::PreEdit))?;
    Err(SolidError::invalid_input(
        OP_DELETE_EDGE,
        "null pointer dereference",
    ))
}

/// The surface swap of `DeleteEdge`:
/// `pSurvOwner = pSurvF->GetSurface()->GetFace(); ..; pSurfTmp = pSurvF->GetSurface();
/// pSurvF->SetSurface(pDelF->GetSurface()); pDelF->SetSurface(pSurfTmp);`.
///
/// Upstream dereferences both faces' surfaces (`SmBrep.cpp:6050-6051`) before either
/// `SetSurface`, so a survivor without a surface is the null-dereference error with
/// nothing changed. (The deleted face holds the surface to keep.) Surfaces are owned
/// values here, so the swap takes the surface out of `del_f`, gives it to `surv_f`
/// (`SetSurface` hands back the one it replaces, without deleting it) and gives that
/// one to `del_f`. `SetSurface` is `void` upstream; the only failure the port can
/// raise is [`SolidError::Unported`] from its notification.
fn swap_face_surfaces(
    k: &Kernel,
    b: &mut Brep,
    key_of: &dyn Fn(EntityRef) -> ObjectRef,
    surv_f: FaceId,
    del_f: FaceId,
) -> Result<()> {
    // pSurvF->GetSurface()->GetFace(), pDelF->GetSurface()->GetFace()
    deref(b.face(surv_f)?.surface.as_ref(), OP_DELETE_EDGE)?;
    let del_surface = deref(b.face_mut(del_f)?.surface.take(), OP_DELETE_EDGE)?;
    let surv_this = key_of(EntityRef::Face(surv_f));
    let surv_old = face1::set_surface(k, b, surv_f, surv_this, del_surface, false)?;
    // the survivor's surface was checked above
    let surv_old = deref(surv_old, OP_DELETE_EDGE)?;
    let del_this = key_of(EntityRef::Face(del_f));
    face1::set_surface(k, b, del_f, del_this, surv_old, false)?;
    Ok(())
}

/// A pointer upstream dereferences without a check.
fn deref<T>(o: Option<T>, op: &'static str) -> Result<T> {
    o.ok_or_else(|| SolidError::invalid_input(op, "null pointer dereference"))
}

/// `NER(p)`: the null-pointer status.
fn ner<T>(o: Option<T>, op: &'static str) -> Result<T> {
    o.ok_or_else(|| SolidError::null_pointer(op))
}

/// The surface of face `f`, named by the face (see the module docs).
fn surface_of(b: &Brep, f: FaceId) -> Result<Option<FaceId>> {
    Ok(b.face(f)?.surface.is_some().then_some(f))
}

fn set_ccw(b: &mut Brep, eu: EdgeuseId, to: Option<EdgeuseId>) -> Result<()> {
    b.edgeuse_mut(eu)?.ccw = to;
    Ok(())
}

fn set_cw(b: &mut Brep, eu: EdgeuseId, to: Option<EdgeuseId>) -> Result<()> {
    b.edgeuse_mut(eu)?.cw = to;
    Ok(())
}

fn ccw(b: &Brep, eu: EdgeuseId) -> Result<Option<EdgeuseId>> {
    eu1::ccw_edgeuse(b, eu)
}

/// `x->m_pCCW = y; y->m_pCW = x` with `y` read now (`y` dereferenced).
fn link(b: &mut Brep, x: Option<EdgeuseId>, y: Option<EdgeuseId>, op: &'static str) -> Result<()> {
    let x = deref(x, op)?;
    set_ccw(b, x, y)?;
    let y = deref(y, op)?;
    set_cw(b, y, Some(x))
}

/// `SmOwningTopology::Remove` of an edge use from its edge's list.
fn edge_remove(b: &mut Brep, e: EdgeId, eu: EdgeuseId, op: &'static str) -> Result<()> {
    if b.edgeuse(eu)?.edge != Some(e) {
        return Err(SolidError::sm_err(op));
    }
    let list = &mut b.edge_mut(e)?.edgeuses;
    let Some(at) = list.iter().position(|&x| x == eu) else {
        return Err(SolidError::sm_err(op));
    };
    list.remove(at);
    b.edgeuse_mut(eu)?.edge = None;
    Ok(())
}

/// `SmOwningTopology::Remove` of a vertex use from its vertex's list.
fn vertex_remove(b: &mut Brep, v: VertexId, vu: VertexuseId, op: &'static str) -> Result<()> {
    if b.vertexuse(vu)?.vertex != Some(v) {
        return Err(SolidError::sm_err(op));
    }
    let list = &mut b.vertex_mut(v)?.vertexuses;
    let Some(at) = list.iter().position(|&x| x == vu) else {
        return Err(SolidError::sm_err(op));
    };
    list.remove(at);
    b.vertexuse_mut(vu)?.vertex = None;
    Ok(())
}

/// `SmOwningTopology::PostInsert` of a vertex use at the end of a vertex's list.
fn vertex_post_insert(b: &mut Brep, v: VertexId, vu: VertexuseId) -> Result<()> {
    b.vertex_mut(v)?.vertexuses.push(vu);
    b.vertexuse_mut(vu)?.vertex = Some(v);
    Ok(())
}

/// `SmOwningTopology::Remove` of a loop use from its face use's list.
fn faceuse_remove(b: &mut Brep, fu: FaceuseId, lu: LoopuseId, op: &'static str) -> Result<()> {
    if b.loopuse(lu)?.faceuse != Some(fu) {
        return Err(SolidError::sm_err(op));
    }
    let list = &mut b.faceuse_mut(fu)?.loopuses;
    let Some(at) = list.iter().position(|&x| x == lu) else {
        return Err(SolidError::sm_err(op));
    };
    list.remove(at);
    b.loopuse_mut(lu)?.faceuse = None;
    Ok(())
}

/// `SmOwningTopology::PostInsert` of a loop use at the end of a face use's list.
fn faceuse_post_insert(b: &mut Brep, fu: FaceuseId, lu: LoopuseId) -> Result<()> {
    b.faceuse_mut(fu)?.loopuses.push(lu);
    b.loopuse_mut(lu)?.faceuse = Some(fu);
    Ok(())
}

/// `SmOwningTopology::Remove` of a face use from its shell's list (`m_pList`; the head
/// moves to the next member when it is removed, the list empties to `NULL`). The arena
/// model's face list of the shell follows. `SM_ERR` with nothing changed when the face
/// use is not on the shell's list, as upstream.
fn shell_remove_faceuse(b: &mut Brep, s: ShellId, fu: FaceuseId, op: &'static str) -> Result<()> {
    if b.faceuse(fu)?.shell != Some(s) {
        return Err(SolidError::sm_err(op));
    }
    let members = &mut b.shell_mut(s)?.members;
    let ShellMembers::Faceuses(list) = members else {
        // no sibling links on a list of another kind (`m_pNext == NULL`)
        return Err(SolidError::sm_err(op));
    };
    let Some(at) = list.iter().position(|&x| x == fu) else {
        return Err(SolidError::sm_err(op));
    };
    list.remove(at);
    if list.is_empty() {
        *members = ShellMembers::Empty;
    }
    let face = b.faceuse(fu)?.face;
    {
        let r = b.faceuse_mut(fu)?;
        r.shell = None;
        r.sibling_links = None;
    }
    if let Some(f) = face {
        b.shell_mut(s)?.faces.retain(|&x| x != f);
    }
    sync_faceuse_links(b, s)
}

/// A vertex use becomes the vertex use of loop use `lu` (`m_tVertexuseType`,
/// `m_pSorLUorEU`) and `lu` a vertex loop use of it (`m_tLoopuseType`, `m_pEUorVU`).
fn make_vertex_loop(b: &mut Brep, vu: VertexuseId, lu: LoopuseId) -> Result<()> {
    b.vertexuse_mut(vu)?.owner = VertexuseOwner::Loopuse(Some(lu));
    b.loopuse_mut(lu)?.start = LoopuseStart::Vertexuse(Some(vu));
    Ok(())
}

/// `SmVertex::Notify(SM_NO_PRE_EDIT, vertex, NULL, NULL)`.
fn notify_vertex(
    k: &mut Kernel,
    b: &mut Brep,
    env: &mut DeleteEnv<'_>,
    v: VertexId,
    body: Option<ObjectRef>,
) -> Result<()> {
    let key_of = env.key_of;
    let this = key_of(EntityRef::Vertex(v));
    vertex::notify(
        b,
        v,
        env.records,
        env.attributes,
        Some(if body.is_some() {
            env.context.as_deref_mut().unwrap_or(&mut k.context)
        } else {
            &mut k.context
        }),
        NotifyOperation::PreEdit,
        this,
        [Some(this), None, None],
        &|vu| key_of(EntityRef::Vertexuse(vu)),
    )
}

/// `SmEdge::Notify(SM_NO_PRE_EDIT, edge, NULL, NULL)`.
fn notify_edge(k: &mut Kernel, b: &mut Brep, env: &mut DeleteEnv<'_>, e: EdgeId) -> Result<()> {
    let key_of = env.key_of;
    let this = key_of(EntityRef::Edge(e));
    let mut n = NotifyEnv {
        records: env.records,
        attributes: env.attributes,
        context: Some(env.context.as_deref_mut().unwrap_or(&mut k.context)),
        key_of,
    };
    edge::notify(
        b,
        e,
        &mut n,
        NotifyOperation::PreEdit,
        [Some(this), None, None],
    )
}

/// `delete pEdge` ([`edge::destroy`]) under the deletion's notification records,
/// attributes and identities.
fn destroy_edge(k: &mut Kernel, b: &mut Brep, env: &mut DeleteEnv<'_>, e: EdgeId) -> Result<()> {
    let mut n = NotifyEnv {
        records: &mut *env.records,
        attributes: &mut *env.attributes,
        context: env.context.as_deref_mut(),
        key_of: env.key_of,
    };
    edge::destroy(k, b, e, &mut n)
}

/// `delete pFace` ([`face3::destroy`]) under the deletion's notification records,
/// attributes and identities.
fn destroy_face(k: &mut Kernel, b: &mut Brep, env: &mut DeleteEnv<'_>, f: FaceId) -> Result<()> {
    let mut n = NotifyEnv {
        records: &mut *env.records,
        attributes: &mut *env.attributes,
        context: env.context.as_deref_mut(),
        key_of: env.key_of,
    };
    face3::destroy(k, b, f, &mut n)
}

/// `delete pShell` ([`shell::destroy`]) under the deletion's notification records,
/// attributes and identities.
fn destroy_shell(k: &mut Kernel, b: &mut Brep, env: &mut DeleteEnv<'_>, s: ShellId) -> Result<()> {
    let mut n = NotifyEnv {
        records: &mut *env.records,
        attributes: &mut *env.attributes,
        context: env.context.as_deref_mut(),
        key_of: env.key_of,
    };
    shell::destroy(k, b, s, &mut n)
}

/// `delete pEU` ([`eu2::destroy`]) under the deletion's notification records,
/// attributes and identities.
fn delete_edgeuse(
    k: &mut Kernel,
    b: &mut Brep,
    env: &mut DeleteEnv<'_>,
    eu: EdgeuseId,
) -> Result<()> {
    let mut n = NotifyEnv {
        records: &mut *env.records,
        attributes: &mut *env.attributes,
        context: env.context.as_deref_mut(),
        key_of: env.key_of,
    };
    eu2::destroy(k, b, eu, &mut n)
}

// ---- DeleteEdge ----------------------------------------------------------------------------

/// `SmBrep::DeleteEdge`: deletes a wire, lamina or manifold edge. Deleting a manifold
/// edge between two faces merges them (the face of the loop use chosen below
/// survives; `surface_to_keep`, the face holding the surface to keep, can force the
/// survivor's surface), between two sides of one loop splits the loop; deleting a
/// lamina edge shortens its loop; deleting a wire edge may split its shell.
///
/// # Errors
///
/// `SM_ERR` ([`SolidError::Upstream`]) when editing is not enabled, the edge is not
/// wire, lamina or manifold, a lamina edge is a strut, or two inner loops would be
/// merged; the null-pointer status where upstream checks a pointer (`NER`); the
/// statuses of `CollectEdgeuses` and the other checked calls;
/// [`SolidError::InvalidInput`] where upstream dereferences null;
/// [`SolidError::Unported`] from the callees not ported yet (see the module docs).
pub fn delete_edge(
    k: &mut Kernel,
    b: &mut Brep,
    env: &mut DeleteEnv<'_>,
    e: EdgeId,
    surface_to_keep: Option<FaceId>,
) -> Result<()> {
    delete_edge_in(k, b, env, e, surface_to_keep, None)
}

/// [`delete_edge`] with the caller's issued body identity and retained traversal records.
///
/// # Errors
/// Those of [`delete_edge`]; completed edits and marks remain on failure.
pub fn delete_edge_with_body_identity(
    k: &mut Kernel,
    b: &mut Brep,
    env: &mut DeleteEnv<'_>,
    e: EdgeId,
    surface_to_keep: Option<FaceId>,
    body: ObjectRef,
) -> Result<()> {
    delete_edge_in(k, b, env, e, surface_to_keep, Some(body))
}

fn delete_edge_in(
    k: &mut Kernel,
    b: &mut Brep,
    env: &mut DeleteEnv<'_>,
    e: EdgeId,
    surface_to_keep: Option<FaceId>,
    body: Option<ObjectRef>,
) -> Result<()> {
    const OP: &str = OP_DELETE_EDGE;
    // check state - m_bEditingEnabled must be on
    if !b.editing_enabled() {
        return Err(SolidError::sm_err(OP));
    }

    // Only delete wire, lamina, or manifold edges in this method
    let is_manifold = edge::is_manifold(b, e)?;
    if !(edge::is_wire(b, e)? || is_manifold || edge::is_lamina(b, e)?) {
        return Err(SolidError::sm_err(OP));
    }

    // prepare the public - notify the Brep it's about to be edited
    let this = brep_arg_with_identity(b, body);
    let edge_arg = NotifyArg::Entity(EntityRef::Edge(e));
    ignore_status(brep_notify_with_identity(
        k,
        b,
        env,
        NotifyOperation::PreEdit,
        [Some(this), None, None],
        body,
    ))?;
    ignore_status(notify_edge(k, b, env, e))?;
    ignore_status(brep_notify_with_identity(
        k,
        b,
        env,
        NotifyOperation::RmFromBrep,
        [Some(edge_arg), Some(this), None],
        body,
    ))?;

    // get radial/mate list starters
    let eu = deref(eu1::primary_edgeuse(b, e)?, OP)?;
    let eu_mate = deref(eu1::mate(b, eu)?, OP)?;

    if is_manifold {
        delete_manifold_edge(k, b, env, e, eu, eu_mate, surface_to_keep, body)
    } else if edge::is_lamina(b, e)? {
        delete_lamina_edge(k, b, env, e, eu, eu_mate, body)
    } else {
        delete_wire_edge(k, b, env, e, eu, eu_mate, body)
    }
}

/// The manifold branch of [`delete_edge`].
fn delete_manifold_edge(
    k: &mut Kernel,
    b: &mut Brep,
    env: &mut DeleteEnv<'_>,
    e: EdgeId,
    eu: EdgeuseId,
    eu_mate: EdgeuseId,
    surface_to_keep: Option<FaceId>,
    body: Option<ObjectRef>,
) -> Result<()> {
    const OP: &str = OP_DELETE_EDGE;
    // get Radial/Mate neighbor Edgeuses
    let eu_rad = deref(eu1::radial(b, eu)?, OP)?;
    let eu_rad_mate = deref(eu1::mate(b, eu_rad)?, OP)?;

    let mut flip_face = false;

    // Don't do the face flip check if same surface on both sides.
    let srf_face = deref(eu1::face(b, eu)?, OP)?;
    let srf_rad_face = deref(eu1::face(b, eu_rad)?, OP)?;
    let srf = surface_of(b, srf_face)?;
    let srf_rad = surface_of(b, srf_rad_face)?;
    if srf != srf_rad {
        let mut uv3 = Vec3::UNDEF; // sUV3d
        let norm1 = edgeuse_normal(k, b, eu, srf, &mut uv3, OP)?;
        let norm2 = edgeuse_normal(k, b, eu_rad, srf_rad, &mut uv3, OP)?;
        let dot = norm1.dot(norm2);
        if dot < 0.0 {
            flip_face = true;
        }
    }

    // get Primary, Mate, Radial, and RadialMate Loopuses
    let mut lu = ner(eu1::loopuse(b, eu)?, OP)?;
    let lu_mate = ner(eu1::loopuse(b, eu_mate)?, OP)?;
    let lu_rad = ner(eu1::loopuse(b, eu_rad)?, OP)?;
    let mut lu_rad_mate = ner(eu1::loopuse(b, eu_rad_mate)?, OP)?;

    // get primary faceuse and face
    let faceuse = ner(loopuse::faceuse(b, lu)?, OP)?;
    let face = ner(eu1::faceuse_face(b, faceuse)?, OP)?;

    // get Radial face
    let fu_rad = deref(loopuse::faceuse(b, lu_rad)?, OP)?;
    let face_rad = ner(eu1::faceuse_face(b, fu_rad)?, OP)?;

    // mark which side of face primaryEdgeuse is on
    let eu_on_primary_side = face2::upward_faceuse(b, face)? == Some(faceuse);

    // note when removed edge splits a loop
    let split_loop = lu == lu_rad;

    // mark when face and radial face have same orientation
    let same_orientation_faces =
        eu1::faceuse_orientation(b, faceuse)? == eu1::faceuse_orientation(b, fu_rad)?;

    // Determine if either end of edge is a strut end
    let v_is_strut = eu1::is_strut(b, eu)?;
    let v_rad_is_strut = eu1::is_strut(b, eu_rad)?;

    let both_inner = loopuse::orientation(b, lu)? == OrientType::Opposite
        && loopuse::orientation(b, lu_rad)? == OrientType::Opposite;

    // determine when faces are to be merged
    let merge_face = !split_loop && !v_is_strut && !v_rad_is_strut && !both_inner;

    // When merging faces - pick the surviving and delete faces.
    let mut surv_lu = None;
    let mut del_lu = None;
    let mut surv_f = None;
    let mut del_f = None;
    if merge_face {
        let swap_del_face = if loopuse::orientation(b, lu)? != loopuse::orientation(b, lu_rad)? {
            // one outer, one inner: choose inner loop to survive
            loopuse::orientation(b, lu)? == OrientType::Same
        } else if let Some(keep) = surface_to_keep {
            surface_of(b, face)? != Some(keep)
        } else {
            false
        };
        let (s, d) = if swap_del_face {
            (lu_rad, lu)
        } else {
            (lu, lu_rad)
        };
        surv_lu = Some(s);
        del_lu = Some(d);
        let s_fu = deref(loopuse::faceuse(b, s)?, OP)?;
        surv_f = Some(ner(eu1::faceuse_face(b, s_fu)?, OP)?);
        let d_fu = deref(loopuse::faceuse(b, d)?, OP)?;
        del_f = eu1::faceuse_face(b, d_fu)?;
    }

    // get edges connected to pEdge through loops
    let mut eu_cw = eu1::cw_edgeuse(b, eu)?;
    let mut eu_rad_cw = eu1::cw_edgeuse(b, eu_rad)?;
    let mut eu_mate_cw = eu1::cw_edgeuse(b, eu_mate)?;
    let mut eu_rad_mate_cw = eu1::cw_edgeuse(b, eu_rad_mate)?;

    let eu_single = ccw(b, eu)? == Some(eu);
    let eu_rad_single = ccw(b, eu_rad)? == Some(eu_rad);
    let single_closed_loop = lu != lu_rad && (eu_single || eu_rad_single);
    let single_closed_loop_isolated = lu != lu_rad && eu_single && eu_rad_single;

    // notify the face and the radial face of the upcoming edit
    ignore_status(face_notify(k, b, env, face, NotifyOperation::PreEdit, body))?;
    ignore_status(face_notify(
        k,
        b,
        env,
        face_rad,
        NotifyOperation::PreEdit,
        body,
    ))?;

    if merge_face {
        // inform the public about this upcoming merge in Brep
        let data = [
            surv_f.map(|f| NotifyArg::Entity(EntityRef::Face(f))),
            del_f.map(|f| NotifyArg::Entity(EntityRef::Face(f))),
            Some(brep_arg_with_identity(b, body)),
        ];
        ignore_status(brep_notify_with_identity(
            k,
            b,
            env,
            NotifyOperation::MergeInBrep,
            data,
            body,
        ))?;
    }

    // Remove parameter space curves from both faces if they do not have the same
    // underlying surface
    if !split_loop && surface_of(b, face)? != surface_of(b, face_rad)? {
        remove_uv_trim_curves(k, b, env, face, body)?;
        remove_uv_trim_curves(k, b, env, face_rad, body)?;
    }

    // When merging two faces - see if Face Surfaces need to be swapped
    if !split_loop && !both_inner {
        if let Some(keep) = surface_to_keep {
            // UB-210: upstream reads pSurvF->GetSurface() here although pSurvF is set
            // only when merging faces; unreachable on well-formed topology (a strut end
            // implies a split loop), kept as a defensive null-dereference error.
            let sf = deref(surv_f, OP)?;
            if Some(keep) != surface_of(b, sf)? {
                let df = deref(del_f, OP)?;
                // when DelF owns the surface to keep
                if surface_of(b, df)? == Some(keep) {
                    // swap the surfaces between the faces
                    swap_face_surfaces(k, b, env.key_of, sf, df)?;
                    if flip_face {
                        ignore_status(face1::flip_face_orientation(b, sf))?;
                    }
                }
                // else: WARN (asked to keep a surface not used by the faces)
            }
        }
    }

    if eu_rad_cw == Some(eu_rad) {
        let c = ccw(b, eu)?;
        link(b, eu_cw, c, OP)?;
        let c = ccw(b, eu_mate)?;
        link(b, eu_mate_cw, c, OP)?;
    } else if eu_cw == Some(eu) {
        let c = ccw(b, eu_rad)?;
        link(b, eu_rad_cw, c, OP)?;
        let c = ccw(b, eu_rad_mate)?;
        link(b, eu_rad_mate_cw, c, OP)?;
    } else {
        let c = ccw(b, eu_rad)?;
        link(b, eu_cw, c, OP)?;
        let c = ccw(b, eu)?;
        link(b, eu_rad_cw, c, OP)?;
        let c = ccw(b, eu_rad_mate)?;
        link(b, eu_mate_cw, c, OP)?;
        let c = ccw(b, eu_mate)?;
        link(b, eu_rad_mate_cw, c, OP)?;
    }

    if split_loop && !single_closed_loop {
        let new_l = b.add_loop_record(Loop {
            face,
            edges: Vec::new(),
            loopuse: None,
        })?;
        lu = b.add_loopuse_record(Loopuse::default())?;
        lu_rad_mate = b.add_loopuse_record(Loopuse::default())?;
        {
            let r = b.loopuse_mut(lu)?;
            r.loop_ = Some(new_l);
            r.mate = Some(lu_rad_mate);
        }
        {
            let r = b.loopuse_mut(lu_rad_mate)?;
            r.loop_ = Some(new_l);
            r.mate = Some(lu);
        }
        let (fu, fu_mate) = face2::faceuses(b, face)?;
        let fu = deref(fu, OP)?;
        let fu_mate = deref(fu_mate, OP)?;
        if eu_on_primary_side {
            b.loop_mut(new_l)?.loopuse = Some(lu);
            faceuse_post_insert(b, fu, lu)?;
            faceuse_post_insert(b, fu_mate, lu_rad_mate)?;
        } else {
            b.loop_mut(new_l)?.loopuse = Some(lu_rad_mate);
            faceuse_post_insert(b, fu_mate, lu)?;
            faceuse_post_insert(b, fu, lu_rad_mate)?;
        }
        b.loopuse_mut(lu)?.orientation = OrientType::Opposite;
        b.loopuse_mut(lu_rad_mate)?.orientation = OrientType::Opposite;
    }

    if v_is_strut || single_closed_loop_isolated {
        let vu = ner(eu1::vertexuse(b, eu)?, OP)?;
        if single_closed_loop_isolated && loopuse::orientation(b, lu)? == OrientType::Same {
            make_vertex_loop(b, vu, lu_rad)?;
        } else {
            make_vertex_loop(b, vu, lu)?;
        }
        let vu_rad_mate = ner(eu1::vertexuse(b, eu_rad_mate)?, OP)?;
        if single_closed_loop_isolated && loopuse::orientation(b, lu_rad_mate)? == OrientType::Same
        {
            make_vertex_loop(b, vu_rad_mate, lu_mate)?;
        } else {
            make_vertex_loop(b, vu_rad_mate, lu_rad_mate)?;
        }
    } else {
        // Normal case: need to remove vertexuses.
        let vu = ner(eu1::vertexuse(b, eu)?, OP)?;
        let v = ner(vertexuse::vertex(b, vu)?, OP)?;
        ignore_status(vertex_remove(b, v, vu, OP))?;
        b.remove_vertexuse_record(vu)?;
        let vu_rad_mate = ner(eu1::vertexuse(b, eu_rad_mate)?, OP)?;
        ignore_status(vertex_remove(b, v, vu_rad_mate, OP))?;
        b.remove_vertexuse_record(vu_rad_mate)?;
    }

    if v_rad_is_strut && !single_closed_loop_isolated {
        let vu_mate = ner(eu1::vertexuse(b, eu_mate)?, OP)?;
        if v_is_strut {
            // Both are strut edges
            make_vertex_loop(b, vu_mate, lu_mate)?;
            let vu_rad = ner(eu1::vertexuse(b, eu_rad)?, OP)?;
            make_vertex_loop(b, vu_rad, lu_rad)?;
        } else {
            make_vertex_loop(b, vu_mate, lu_rad_mate)?;
            let vu_rad = ner(eu1::vertexuse(b, eu_rad)?, OP)?;
            make_vertex_loop(b, vu_rad, lu)?;
        }
    } else {
        // remove pMate Vertex vertex uses
        let vu_mate = ner(eu1::vertexuse(b, eu_mate)?, OP)?;
        let v_mate = ner(vertexuse::vertex(b, vu_mate)?, OP)?;
        ignore_status(vertex_remove(b, v_mate, vu_mate, OP))?;
        b.remove_vertexuse_record(vu_mate)?;
        let vu_rad = ner(eu1::vertexuse(b, eu_rad)?, OP)?;
        ignore_status(vertex_remove(b, v_mate, vu_rad, OP))?;
        b.remove_vertexuse_record(vu_rad)?;
    }

    for x in [eu, eu_mate, eu_rad, eu_rad_mate] {
        ignore_status(edge_remove(b, e, x, OP))?;
    }

    for (l, x) in [
        (lu, eu),
        (lu_mate, eu_mate),
        (lu_rad, eu_rad),
        (lu_rad_mate, eu_rad_mate),
    ] {
        if loopuse::is_edge_loopuse(b, l)?
            && loopuse::eu_or_vu(b, l)? == Some(EntityRef::Edgeuse(x))
        {
            let c = ccw(b, x)?;
            ignore_status(loopuse::set_edgeuse(b, l, c))?;
        }
    }

    if eu_cw == Some(eu) {
        eu_cw = None;
    }
    if eu_mate_cw == Some(eu_mate) {
        eu_mate_cw = None;
    }
    if eu_rad_cw == Some(eu_rad) {
        eu_rad_cw = None;
    }
    if eu_rad_mate_cw == Some(eu_rad_mate) {
        eu_rad_mate_cw = None;
    }

    for x in [eu, eu_mate, eu_rad, eu_rad_mate] {
        delete_edgeuse(k, b, env, x)?;
    }
    destroy_edge(k, b, env, e)?;

    if v_is_strut || v_rad_is_strut {
        if !v_is_strut {
            loopuse::collect_edgeuses(b, lu_mate, eu_rad_mate_cw)?;
            loopuse::collect_edgeuses(b, lu_rad, eu_cw)?;
        }
        if !v_rad_is_strut {
            loopuse::collect_edgeuses(b, lu_mate, eu_mate_cw)?;
            loopuse::collect_edgeuses(b, lu_rad, eu_rad_cw)?;
        }
        return Ok(());
    }

    if split_loop {
        // split a loop branch - not merging two faces
        loopuse::collect_edgeuses(b, lu, eu_cw)?;
        loopuse::collect_edgeuses(b, lu_mate, eu_mate_cw)?;
        loopuse::collect_edgeuses(b, lu_rad, eu_rad_cw)?;
        loopuse::collect_edgeuses(b, lu_rad_mate, eu_rad_mate_cw)?;

        if loopuse::orientation(b, lu_mate)? == OrientType::Same {
            let l = loopuse::loop_(b, lu_mate)?;
            let loop_orient = face_compute_loop_orientation(k, b, face, l)?;
            if loop_orient != OrientType::Same {
                loopuse::collect_edgeuses(b, lu, eu_rad_cw)?;
                loopuse::collect_edgeuses(b, lu_mate, eu_rad_mate_cw)?;
                loopuse::collect_edgeuses(b, lu_rad, eu_cw)?;
                loopuse::collect_edgeuses(b, lu_rad_mate, eu_mate_cw)?;
            }
        }
        return Ok(());
    }

    // Different loops on the two sides (radially) - merging two faces
    if both_inner {
        return Err(SolidError::sm_err(OP));
    }
    let surv_lu = deref(surv_lu, OP)?;
    let del_lu = deref(del_lu, OP)?;
    let surv_f = deref(surv_f, OP)?;
    let del_f = deref(del_f, OP)?;

    if !single_closed_loop_isolated {
        if eu_cw.is_some() {
            loopuse::collect_edgeuses(b, surv_lu, eu_cw)?;
        } else if eu_rad_cw.is_some() {
            loopuse::collect_edgeuses(b, surv_lu, eu_rad_cw)?;
        }
        let surv_lu_other = deref(loopuse::other_loopuse(b, surv_lu)?, OP)?;
        if eu_mate_cw.is_some() {
            loopuse::collect_edgeuses(b, surv_lu_other, eu_mate_cw)?;
        } else if eu_rad_mate_cw.is_some() {
            loopuse::collect_edgeuses(b, surv_lu_other, eu_rad_mate_cw)?;
        }
    }

    let del_loop = loopuse::loop_(b, del_lu)?;
    let del_lu_mate = loopuse::other_loopuse(b, del_lu)?;

    face_remove_loop(b, del_f, del_loop)?;

    if surface_of(b, del_f)? == surface_of(b, surv_f)? {
        let uv_union = face2::uv_domain(b, del_f)?.union(&face2::uv_domain(b, surv_f)?);
        face2::set_uv_domain(b, surv_f, &uv_union)?;
    }

    if let Some(l) = del_loop {
        b.remove_loop_record(l)?;
    }
    if let Some(l) = del_lu_mate {
        b.remove_loopuse_record(l)?;
    }
    b.remove_loopuse_record(del_lu)?;

    // move the remaining loops of the deleted face to the surviving face
    for move_loop in face2::loops(b, del_f, false, None)? {
        face1::remove_loop(b, del_f, move_loop)?;
        if !same_orientation_faces {
            let l_lu = deref(b.loop_(move_loop)?.loopuse, OP)?;
            b.loop_mut(move_loop)?.loopuse = loopuse::other_loopuse(b, l_lu)?;
        }
        face1::insert_loop(b, surv_f, move_loop)?;
    }

    let (fu_del, fu_del_mate) = face2::faceuses(b, del_f)?;
    let fu_del = deref(fu_del, OP)?;
    let fu_del_mate = deref(fu_del_mate, OP)?;
    let s = deref(eu1::faceuse_shell(b, fu_del)?, OP)?;
    ignore_status(shell_remove_faceuse(b, s, fu_del, OP))?;
    let s = deref(eu1::faceuse_shell(b, fu_del_mate)?, OP)?;
    ignore_status(shell_remove_faceuse(b, s, fu_del_mate, OP))?;
    b.remove_faceuse_record(fu_del)?;
    b.remove_faceuse_record(fu_del_mate)?;
    destroy_face(k, b, env, del_f)?;
    Ok(())
}

/// The normal of surface `srf` (named by its face) at the UV point of edge use `eu`
/// at its middle (`NormalizedEvaluate(0.5, TRUE, sUV3d)` with the header defaults, then
/// `EvaluateNormal(uv, FALSE, FALSE, ..)`). Both statuses are ignored upstream:
/// `sUV3d` is shared by the two calls of `DeleteEdge`, so a failed evaluation leaves
/// the previous point (undefined before the first); a failed normal is the undefined
/// vector.
fn edgeuse_normal(
    k: &Kernel,
    b: &mut Brep,
    eu: EdgeuseId,
    srf: Option<FaceId>,
    uv3: &mut Vec3,
    op: &'static str,
) -> Result<Vec3> {
    if let Some(e) = ignored(eu3::normalized_evaluate(k, b, eu, 0.5, true, false, true))? {
        *uv3 = e.point;
    }
    let uv = Point2::new(uv3.x, uv3.y);
    let f = deref(srf, op)?;
    let s = deref(b.face(f)?.surface.as_ref(), op)?;
    Ok(ignored(s.evaluate_normal(k, uv, Side::Right, Side::Right))?.unwrap_or(Vec3::UNDEF))
}

/// The lamina branch of [`delete_edge`].
fn delete_lamina_edge(
    k: &mut Kernel,
    b: &mut Brep,
    env: &mut DeleteEnv<'_>,
    e: EdgeId,
    eu: EdgeuseId,
    eu_mate: EdgeuseId,
    body: Option<ObjectRef>,
) -> Result<()> {
    const OP: &str = OP_DELETE_EDGE;
    let lu = ner(eu1::loopuse(b, eu)?, OP)?;
    let lu_mate = ner(eu1::loopuse(b, eu_mate)?, OP)?;
    let faceuse = ner(loopuse::faceuse(b, lu)?, OP)?;
    let face = ner(eu1::faceuse_face(b, faceuse)?, OP)?;

    ignore_status(face_notify(k, b, env, face, NotifyOperation::PreEdit, body))?;

    let v_is_strut = eu1::is_strut(b, eu)?;
    let v_other_is_strut = eu1::is_strut(b, eu_mate)?;
    if v_is_strut || v_other_is_strut {
        // a nonmanifold edge is a strut edge
        return Err(SolidError::sm_err(OP));
    }

    let eu_cw = eu1::cw_edgeuse(b, eu)?;
    let eu_mate_cw = eu1::cw_edgeuse(b, eu_mate)?;

    let single_closed_loop_isolated = ccw(b, eu)? == Some(eu);

    let c = ccw(b, eu)?;
    link(b, eu_cw, c, OP)?;
    let c = ccw(b, eu_mate)?;
    link(b, eu_mate_cw, c, OP)?;

    if single_closed_loop_isolated {
        let vu = ner(eu1::vertexuse(b, eu)?, OP)?;
        make_vertex_loop(b, vu, lu)?;
        let vu_mate = ner(eu1::vertexuse(b, eu_mate)?, OP)?;
        make_vertex_loop(b, vu_mate, lu_mate)?;
    } else {
        // Normal case: need to remove two vertexuses.
        let vu = ner(eu1::vertexuse(b, eu)?, OP)?;
        let v = ner(vertexuse::vertex(b, vu)?, OP)?;
        ignore_status(vertex_remove(b, v, vu, OP))?;
        b.remove_vertexuse_record(vu)?;
        let vu_mate = ner(eu1::vertexuse(b, eu_mate)?, OP)?;
        let v_mate = ner(vertexuse::vertex(b, vu_mate)?, OP)?;
        ignore_status(vertex_remove(b, v_mate, vu_mate, OP))?;
        b.remove_vertexuse_record(vu_mate)?;
    }

    ignore_status(edge_remove(b, e, eu, OP))?;
    ignore_status(edge_remove(b, e, eu_mate, OP))?;

    if loopuse::eu_or_vu(b, lu)? == Some(EntityRef::Edgeuse(eu)) {
        let c = ccw(b, eu)?;
        ignore_status(loopuse::set_edgeuse(b, lu, c))?;
    }
    if loopuse::eu_or_vu(b, lu_mate)? == Some(EntityRef::Edgeuse(eu_mate)) {
        let c = ccw(b, eu_mate)?;
        ignore_status(loopuse::set_edgeuse(b, lu_mate, c))?;
    }

    delete_edgeuse(k, b, env, eu)?;
    delete_edgeuse(k, b, env, eu_mate)?;
    destroy_edge(k, b, env, e)?;
    Ok(())
}

/// A new vertex-use shell holding `vu` (`None`: upstream warns that the vertex use
/// has been deleted), posted at the end of the region of `shell`.
fn new_vertex_shell(b: &mut Brep, shell: ShellId, vu: Option<VertexuseId>) -> Result<()> {
    let s = b.add_shell_record(Shell::default())?;
    set_shell_head(b, s, vu_head(vu), None)?;
    if let Some(vu) = vu {
        b.vertexuse_mut(vu)?.owner = VertexuseOwner::Shell(Some(s));
    }
    // else WARN: the vertex use has been deleted
    b.shell_mut(s)?.shell_type = ShellType::Vertex;
    let region = shell_region(b, shell)?;
    region_post_insert(b, region, s)
}

/// The wire branch of [`delete_edge`].
fn delete_wire_edge(
    k: &mut Kernel,
    b: &mut Brep,
    env: &mut DeleteEnv<'_>,
    e: EdgeId,
    eu: EdgeuseId,
    eu_mate: EdgeuseId,
    body: Option<ObjectRef>,
) -> Result<()> {
    const OP: &str = OP_DELETE_EDGE;
    let shell = ner(eu1::shell(b, eu)?, OP)?;

    ignore_status(edge_remove(b, e, eu, OP))?;
    ignore_status(edge_remove(b, e, eu_mate, OP))?;

    let vu1 = ner(eu1::vertexuse(b, eu)?, OP)?;
    let v1 = ner(vertexuse::vertex(b, vu1)?, OP)?;
    ignore_status(vertex_remove(b, v1, vu1, OP))?;

    let vu2 = ner(eu1::vertexuse(b, eu_mate)?, OP)?;
    let v2 = ner(vertexuse::vertex(b, vu2)?, OP)?;
    ignore_status(vertex_remove(b, v2, vu2, OP))?;

    let mut vu1 = Some(vu1);
    let mut vu2 = Some(vu2);
    for (v, slot) in [(v1, &mut vu1), (v2, &mut vu2)] {
        let Some(vu) = *slot else { continue };
        if vertex::vertexuses(b, v)?.is_empty() {
            ignore_status(vertex_post_insert(b, v, vu))?;
            b.vertexuse_mut(vu)?.owner = VertexuseOwner::Shell(Some(shell));
        } else {
            // the vertex is connected to some other edges: delete the vertex use
            b.remove_vertexuse_record(vu)?;
            *slot = None;
        }
    }

    destroy_edge(k, b, env, e)?;

    if v1 == v2 {
        if vertex::is_shell_vertex(b, v1)? {
            // reset the shell parameters to be a vertexShell
            let head = vertex::primary_vertexuse(b, v1)?;
            set_shell_head(b, shell, vu_head(head), Some(ShellType::Vertex))?;
        } else if shell_is_wire_shell(b, shell)? {
            // give the shell a new primary Edgeuse
            let vu = deref(vertex::primary_vertexuse(b, v1)?, OP)?;
            let head = vertexuse::edgeuse(b, vu)?;
            set_shell_head(b, shell, eu_head(head), None)?;
        }
        delete_edgeuse(k, b, env, eu)?;
        delete_edgeuse(k, b, env, eu_mate)?;
        return Ok(());
    }

    let v1_shell = vertex::is_shell_vertex(b, v1)?;
    let v2_shell = vertex::is_shell_vertex(b, v2)?;
    if v1_shell && v2_shell {
        // give V1 to the existing shell and create a new shell for V2
        set_shell_head(b, shell, vu_head(vu1), Some(ShellType::Vertex))?;
        new_vertex_shell(b, shell, vu2)?;
        delete_edgeuse(k, b, env, eu)?;
        delete_edgeuse(k, b, env, eu_mate)?;
        return Ok(());
    }
    if v1_shell != v2_shell {
        let (vu_new, v_other) = if v1_shell { (vu1, v2) } else { (vu2, v1) };
        new_vertex_shell(b, shell, vu_new)?;
        if shell_is_wire_shell(b, shell)? {
            let edges = vertex::edges(b, v_other, None)?;
            if let Some(&start_wire) = edges.first() {
                // the shell remains a wire shell and needs a new primary edge use
                let head = eu1::primary_edgeuse(b, start_wire)?;
                set_shell_head(b, shell, eu_head(head), None)?;
            } else {
                let head = vertex::primary_vertexuse(b, v_other)?;
                set_shell_head(b, shell, vu_head(head), Some(ShellType::Vertex))?;
            }
        }
        delete_edgeuse(k, b, env, eu)?;
        delete_edgeuse(k, b, env, eu_mate)?;
        return Ok(());
    }

    let mut lock = MarkLock::new(core::slice::from_mut(&mut k.context), 0, ALL_MARKS)?;
    let r = wire_split_shell(k, b, env, &lock, shell, v1, v2, eu, eu_mate, body);
    lock.release(core::slice::from_mut(&mut k.context))?;
    r
}

/// The shell traversals of the wire branch of [`delete_edge`], under `lock`.
fn wire_split_shell(
    k: &mut Kernel,
    b: &mut Brep,
    env: &mut DeleteEnv<'_>,
    lock: &MarkLock,
    shell: ShellId,
    v1: VertexId,
    v2: VertexId,
    eu: EdgeuseId,
    eu_mate: EdgeuseId,
    body: Option<ObjectRef>,
) -> Result<()> {
    const OP: &str = OP_DELETE_EDGE;
    let mut indexed = body.is_none().then(Marks::default);
    let mut marks = match indexed.as_mut() {
        Some(marks) => super::p3::DeletionMarks::Indexed(marks),
        None => super::p3::DeletionMarks::Retained,
    };
    let mut edgeuses1 = Vec::new();
    let mut faceuses1 = Vec::new();
    marks.with_marks(env.records, env.key_of, |marks| {
        topology_traverser::shell_traversal_with_marks(
            k,
            b,
            marks,
            v1,
            shell,
            Some(&mut edgeuses1),
            Some(&mut faceuses1),
            lock,
        )
    })?;
    let split_shell = !marks.with_marks(env.records, env.key_of, |marks| {
        marks.is_marked(k, Entity::Vertex(v2), lock.mark_type())
    });
    if split_shell {
        let new_shell = b.add_shell_record(Shell::default())?;
        let region = shell_region(b, shell)?;
        region_post_insert(b, region, new_shell)?;
        let mut edgeuses2 = Vec::new();
        let mut faceuses2 = Vec::new();
        marks.with_marks(env.records, env.key_of, |marks| {
            topology_traverser::shell_traversal_with_marks(
                k,
                b,
                marks,
                v2,
                shell,
                Some(&mut edgeuses2),
                Some(&mut faceuses2),
                lock,
            )
        })?;
        if faceuses2.is_empty() {
            // NewShell is a wireShell - set first edge and ShellType (an empty
            // edge-use list keeps the NULL head; upstream only asserts)
            if let Some(&first) = edgeuses2.first() {
                set_shell_head(b, new_shell, ShellMembers::Edgeuse(first), None)?;
            }
            b.shell_mut(new_shell)?.shell_type = ShellType::Wire;
        } else {
            // the first face is set later by PostInsert(pFU)
            b.shell_mut(new_shell)?.shell_type = ShellType::Faceuse;
        }
        for &x in &edgeuses2 {
            if eu1::is_shell_edgeuse(b, x)? {
                b.edgeuse_mut(x)?.owner = EdgeuseOwner::Shell(Some(new_shell));
            }
        }
        // transfer faceuses from the old shell to the new shell
        for &fu in &faceuses2 {
            ignore_status(shell_remove_faceuse(b, shell, fu, OP))?;
            shell_post_insert_faceuse(b, new_shell, fu)?;
        }
    }
    // when pShell points to a deleted edgeuse (wireshell case) or is a wireshell
    // after a shell split, ensure it has a valid first edge
    let head = shell_head(b, shell)?;
    if head == Some(EntityRef::Edgeuse(eu))
        || head == Some(EntityRef::Edgeuse(eu_mate))
        || (faceuses1.is_empty() && split_shell)
    {
        if let Some(&first) = edgeuses1.first() {
            set_shell_head(b, shell, ShellMembers::Edgeuse(first), None)?;
        }
        b.shell_mut(shell)?.shell_type = ShellType::Wire;
    }
    delete_edgeuse(k, b, env, eu)?;
    delete_edgeuse(k, b, env, eu_mate)?;
    Ok(())
}

// ---- DeleteVertex --------------------------------------------------------------------------

/// `SmBrep::DeleteVertex`: deletes a vertex with no edges: a shell vertex (with its
/// shell when the vertex is the shell's) or the vertex of vertex loops (with the
/// loops and their loop uses).
///
/// # Errors
///
/// `SM_ERR` ([`SolidError::Upstream`]) when editing is not enabled or the vertex has
/// edges; the null-pointer status where upstream checks a face;
/// [`SolidError::InvalidInput`] where upstream dereferences null, casts a vertex use's
/// owner to a loop use it is not, or reads past its vertex use pairs;
/// [`SolidError::Unported`] from the callees not ported yet.
pub fn delete_vertex(
    k: &mut Kernel,
    b: &mut Brep,
    env: &mut DeleteEnv<'_>,
    v: VertexId,
) -> Result<()> {
    delete_vertex_in(k, b, env, v, None)
}

/// [`delete_vertex`] with the caller's issued body identity and retained traversal records.
///
/// # Errors
/// Those of [`delete_vertex`]; completed edits and marks remain on failure.
pub fn delete_vertex_with_body_identity(
    k: &mut Kernel,
    b: &mut Brep,
    env: &mut DeleteEnv<'_>,
    v: VertexId,
    body: ObjectRef,
) -> Result<()> {
    delete_vertex_in(k, b, env, v, Some(body))
}

fn delete_vertex_in(
    k: &mut Kernel,
    b: &mut Brep,
    env: &mut DeleteEnv<'_>,
    v: VertexId,
    body: Option<ObjectRef>,
) -> Result<()> {
    const OP: &str = OP_DELETE_VERTEX;
    // check state - m_bEditingEnabled must be on
    if !b.editing_enabled() {
        return Err(SolidError::sm_err(OP));
    }

    // inform the public of upcoming change
    let this = brep_arg_with_identity(b, body);
    let vertex_arg = NotifyArg::Entity(EntityRef::Vertex(v));
    ignore_status(brep_notify_with_identity(
        k,
        b,
        env,
        NotifyOperation::PreEdit,
        [Some(this), None, None],
        body,
    ))?;
    ignore_status(notify_vertex(k, b, env, v, body))?;
    ignore_status(brep_notify_with_identity(
        k,
        b,
        env,
        NotifyOperation::RmFromBrep,
        [Some(vertex_arg), Some(this), None],
        body,
    ))?;

    // vertices should be disconnected from edges prior to delete
    if !vertex::edges(b, v, None)?.is_empty() {
        return Err(SolidError::sm_err(OP));
    }

    let vertexuses = vertex::vertexuses(b, v)?;
    if vertexuses.is_empty() {
        // WARN: deleting a vertex with no vertexuses
        b.remove_vertex_record(v)?;
        return Ok(());
    }

    if vertex::is_shell_vertex(b, v)? {
        let vu = deref(vertexuses.first().copied(), OP)?;
        let VertexuseOwner::Shell(shell) = b.vertexuse(vu)?.owner else {
            return Err(SolidError::invalid_input(
                OP,
                "shell vertex use without a shell",
            ));
        };
        let shell = deref(shell, OP)?;
        // The vertex to delete may not actually own a shell.
        if b.shell(shell)?.is_vertex_shell()
            && shell_head(b, shell)? == Some(EntityRef::Vertexuse(vu))
        {
            let region = deref(shell_region(b, shell)?, OP)?;
            ignore_status(shell::region_remove_shell(b, region, shell))?;
            destroy_shell(k, b, env, shell)?;
        }
        ignore_status(vertex_remove(b, v, vu, OP))?;
        b.remove_vertex_record(v)?;
        b.remove_vertexuse_record(vu)?;
        return Ok(());
    }

    // Must be loop vertex (can be connected to 1 or more faces)
    for pair in vertexuses.chunks(2) {
        let [vu1, vu2] = *pair else {
            return Err(SolidError::invalid_input(OP, "odd number of vertex uses"));
        };
        let lu_of = |b: &Brep, vu: VertexuseId| -> Result<LoopuseId> {
            match b.vertexuse(vu)?.owner {
                VertexuseOwner::Loopuse(lu) => deref(lu, OP),
                _ => Err(SolidError::invalid_input(
                    OP,
                    "vertex use owner is not a loop use",
                )),
            }
        };
        let lu1 = lu_of(b, vu1)?;
        let lu2 = lu_of(b, vu2)?;
        let fu1 = deref(loopuse::faceuse(b, lu1)?, OP)?;
        let f = ner(eu1::faceuse_face(b, fu1)?, OP)?;

        ignore_status(face_notify(k, b, env, f, NotifyOperation::PreEdit, body))?;

        // remove and delete vertex to loop connections
        ignore_status(faceuse_remove(b, fu1, lu1, OP))?;
        let fu2 = deref(loopuse::faceuse(b, lu2)?, OP)?;
        ignore_status(faceuse_remove(b, fu2, lu2, OP))?;
        if let Some(l) = loopuse::loop_(b, lu1)? {
            b.remove_loop_record(l)?;
        }
        loopuse::set_loop(b, lu1, None)?;
        b.remove_loopuse_record(lu1)?;
        b.remove_loopuse_record(lu2)?;
        ignore_status(vertex_remove(b, v, vu1, OP))?;
        ignore_status(vertex_remove(b, v, vu2, OP))?;
        b.remove_vertexuse_record(vu1)?;
        b.remove_vertexuse_record(vu2)?;
    }

    // delete unconnected vertex
    b.remove_vertex_record(v)?;
    Ok(())
}

// ---- StitchFaces ---------------------------------------------------------------------------

/// The outputs of [`stitch_faces`].
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct StitchOutcome {
    /// `rlStitchedEdges`: the number of edges stitched.
    pub stitched_edges: u32,
    /// `rlLaminaEdges`: the number of lamina edges remaining.
    pub lamina_edges: u32,
    /// `rdMaxVertexEdgeGap`: the largest gap between coincident vertices considered.
    pub max_vertex_edge_gap: f64,
    /// `rdMaxEdgeFaceTrimCurveGap`: the largest gap between coincident edges considered.
    pub max_edge_face_trim_curve_gap: f64,
}

/// `sStitch.DoStitching(this, pVertices, pEdges, ..)`.
fn do_stitching(
    k: &mut Kernel,
    b: &mut Brep,
    stitch: &mut Stitch<'_>,
    vertices: Option<&[VertexId]>,
    edges: Option<&[EdgeId]>,
) -> Result<StitchOutcome> {
    // the min-gap outputs are not requested (NULL)
    let r = stitch.do_stitching(k, b, vertices, edges, MinGapRequest::default())?;
    let count = |n: u64| {
        u32::try_from(n).map_err(|_| SolidError::invalid_input(OP_STITCH, "edge count overflow"))
    };
    Ok(StitchOutcome {
        stitched_edges: count(r.stitched_edges)?,
        lamina_edges: count(r.lamina_edges)?,
        max_vertex_edge_gap: r.max_vertex_gap,
        max_edge_face_trim_curve_gap: r.max_edge_gap,
    })
}

/// `SmBrep::StitchFaces`: glues coincident vertex and edge pairs of the B-rep (of the
/// topology of `faces` when given; `None` entries are skipped) within
/// `xsect_tol_3d`, with spine edges allowed when `allow_spine_edges`.
///
/// # Errors
///
/// Those of the face queries and the stitcher; [`SolidError::Unported`] while the
/// bounding-box lock (`SmBrep::LockBoundingBox`, unit `query.brep`) and the stitcher's
/// callees are not ported. The lock ([`LockBrepBox`]) is released when
/// it goes out of scope, on every path.
pub fn stitch_faces(
    k: &mut Kernel,
    b: &mut Brep,
    xsect_tol_3d: f64,
    faces: Option<&[Option<FaceId>]>,
    allow_spine_edges: bool,
) -> Result<StitchOutcome> {
    stitch_faces_with_identity(k, b, xsect_tol_3d, faces, allow_spine_edges, None)
}

pub(crate) fn stitch_faces_with_identity(
    k: &mut Kernel,
    b: &mut Brep,
    xsect_tol_3d: f64,
    faces: Option<&[Option<FaceId>]>,
    allow_spine_edges: bool,
    identity: Option<crate::query::brep::BrepIdentity<'_>>,
) -> Result<StitchOutcome> {
    stitch_faces_with_context(k, b, xsect_tol_3d, faces, allow_spine_edges, identity, None)
}

/// Stitch with the actual caller's notification state; no call-local edit stores.
pub(crate) fn stitch_faces_with_context<'a>(
    k: &mut Kernel,
    b: &mut Brep,
    xsect_tol_3d: f64,
    faces: Option<&[Option<FaceId>]>,
    allow_spine_edges: bool,
    identity: Option<crate::query::brep::BrepIdentity<'a>>,
    edits: Option<&'a mut dyn crate::heal::stitch::p1::StitchEditContext>,
) -> Result<StitchOutcome> {
    // StitchCallback has SelectTopologyToKeep(): pick KeepItem and DeleteItem
    let mut callback = DefaultStitchCallback::new();
    stitch_faces_with_callback(
        k,
        b,
        xsect_tol_3d,
        faces,
        allow_spine_edges,
        identity,
        edits,
        &mut callback,
    )
}

/// [`stitch_faces_with_context`] with the caller's keep/delete choice for the glued
/// pairs: the fork's `pOptCallback` argument of `SmBrep::StitchFaces`
/// (source fork@1072d1b), `SmStitchCallback`'s own choice when it is
/// [`DefaultStitchCallback`].
///
/// # Errors
///
/// Those of [`stitch_faces`] and of `callback`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn stitch_faces_with_callback<'a, C: StitchCallback>(
    k: &mut Kernel,
    b: &mut Brep,
    xsect_tol_3d: f64,
    faces: Option<&[Option<FaceId>]>,
    allow_spine_edges: bool,
    identity: Option<crate::query::brep::BrepIdentity<'a>>,
    mut edits: Option<&'a mut dyn crate::heal::stitch::p1::StitchEditContext>,
    callback: &mut C,
) -> Result<StitchOutcome> {
    // lock brep bounding box, until goes out of scope
    let mut lockit = LockBrepBox::new(k, b)?;
    let b = lockit.brep();

    // construct Stitch manager using the caller's keep/delete logic for glue pairs
    let mut stitch = Stitch::new(callback, xsect_tol_3d, true, true);
    stitch.query_identity = identity;
    stitch.edits = edits
        .as_mut()
        .map(|edits| &mut **edits as &mut dyn crate::heal::stitch::p1::StitchEditContext);
    stitch.fast_edge_compare = true;
    stitch.ignore_problems = true;
    stitch.squeeze_small_edges = true;
    stitch.making_manifold_solid = !allow_spine_edges;
    stitch.remove_laminar_slivers = true;

    let mut lists = None;
    if let Some(faces) = faces {
        let mut vertices = Vec::new();
        let mut edges = Vec::new();
        for f in faces.iter().flatten() {
            vertices.extend(face_vertices(b, *f)?);
            edges.extend(face_edges(b, *f)?);
        }
        lists = Some((vertices, edges));
    }
    // pass the stitch call along (SER: its status is returned as is)
    do_stitching(
        k,
        b,
        &mut stitch,
        lists.as_ref().map(|(v, _)| v.as_slice()),
        lists.as_ref().map(|(_, e)| e.as_slice()),
    )
}

// ---- ShrinkGeometry ------------------------------------------------------------------------

/// `SmBrep::ShrinkGeometry`: shrinks each face's UV domain to its trim curves (snapped
/// to the surface domain within a thousandth of its size), trims each surface to its
/// face's domain and each edge curve to its edge's interval.
///
/// # Errors
///
/// Those of the curve trims; [`SolidError::Unported`] while the B-rep and face
/// queries it uses are not ported.
pub fn shrink_geometry(k: &Kernel, b: &mut Brep) -> Result<()> {
    for f in brep1::faces(b, None)? {
        let Some(face_domain) = ignored(face_calculate_uv_domain(k, b, f))? else {
            continue;
        };
        let surf_domain =
            deref(face2::surface(b, f)?, OP_SHRINK)?.natural_uv_domain_with_nurb(k)?;
        let tol = face_domain.size().length() / 1000.0;
        let mut min = face_domain.min();
        let mut max = face_domain.max();
        if min.x - surf_domain.min().x < tol {
            min.x = surf_domain.min().x;
        }
        if min.y - surf_domain.min().y < tol {
            min.y = surf_domain.min().y;
        }
        if surf_domain.max().x - max.x < tol {
            max.x = surf_domain.max().x;
        }
        if surf_domain.max().y - max.y < tol {
            max.y = surf_domain.max().y;
        }
        let new_domain = Extent2d::from_points(min, max);
        face2::set_uv_domain(b, f, &new_domain)?;
    }

    for face in brep_get_surfaces(b)? {
        let mut face_domain = face2::uv_domain(b, face)?;
        let surf_domain =
            deref(face2::surface(b, face)?, OP_SHRINK)?.natural_uv_domain_with_nurb(k)?;
        // The status of `Intersect` is dropped upstream, but on disjoint domains it has
        // `Init()`ed the result (SmExtent2d.cpp:414-418), and that undefined extent is
        // the face domain from here on (`AreEqual`, `TrimWithDomain`, `SetUVDomain`).
        face_domain = surf_domain
            .intersect(&face_domain)
            .unwrap_or_else(|_| Extent2d::new());
        if !surf_domain.are_equal(&face_domain, EFF_ZERO) {
            // trim the surface: the domain is in/out (TrimWithDomain may snap it to
            // the trimmed surface's natural domain), and the snapped value is the one
            // `SetUVDomain` gets below
            if let Some(s) = b.face_mut(face)?.surface.as_mut() {
                ignore_status(s.trim_with_domain(k, &mut face_domain))?;
            }
            ignore_status(face1::remove_uv_trim_curves(b, face))?;
        }
        face2::set_uv_domain(b, face, &face_domain)?;
    }

    for e in brep_get_curves(b)? {
        let mut ivl: Extent1d = edge::interval(b, e)?;
        if let Some(c) = b.edge_mut(e)?.curve.as_mut() {
            // may snap ivl by tol to existing knots
            c.trim(k, &mut ivl, TrimMode::Notify)?;
        }
    }
    Ok(())
}

// ---- ~SmBrep -------------------------------------------------------------------------------

/// `SmBrep::~SmBrep`: enables editing, sets the context's boolean flag for the
/// duration (restored after), notifies the destruction and deletes every face (with
/// its loops and uses), edge (with its edge uses), vertex (with its vertex uses) and
/// region (with its shells).
///
/// # Errors
///
/// [`SolidError::Unported`] from the callees not ported yet.
pub fn destroy(k: &mut Kernel, b: &mut Brep) -> Result<()> {
    // Do these before Notify (in case of user callback).
    b.set_editing_enabled(true);
    let prev_doing_boolean = k.context.doing_boolean();
    k.context.set_doing_boolean(true);
    let r = destroy_records(k, b);
    k.context.set_doing_boolean(prev_doing_boolean);
    r
}

fn destroy_records(k: &mut Kernel, b: &mut Brep) -> Result<()> {
    const OP: &str = OP_DESTROY;
    let this = brep_arg(b);
    // no caller state: the notification goes through call-local records
    with_local_env(|env| {
        ignore_status(brep_notify(
            k,
            b,
            env,
            NotifyOperation::Destruction,
            [Some(this), None, None],
        ))
    })?;

    // faces
    for f in brep1::faces(b, None)? {
        let fu1 = deref(b.face(f)?.faceuse, OP)?;
        let fu2 = b.faceuse(fu1)?.mate;
        for l in face2::loops(b, f, false, None)? {
            let lu1 = b.loop_(l)?.loopuse;
            if let Some(lu1) = lu1 {
                if let Some(lu2) = b.loopuse(lu1)?.mate {
                    b.remove_loopuse_record(lu2)?;
                }
                b.remove_loopuse_record(lu1)?;
            }
            b.remove_loop_record(l)?;
        }
        if let Some(fu2) = fu2 {
            b.remove_faceuse_record(fu2)?;
        }
        b.remove_faceuse_record(fu1)?;
        b.remove_face_record(f)?;
    }

    // edges
    for e in brep1::edges(b, None)? {
        for eu in b.edge(e)?.edgeuses.clone() {
            b.remove_edgeuse_record(eu)?;
        }
        b.remove_edge_record(e)?;
    }

    // vertices
    for v in brep1::vertices(b, None)? {
        for vu in b.vertex(v)?.vertexuses.clone() {
            b.remove_vertexuse_record(vu)?;
        }
        b.remove_vertex_record(v)?;
    }

    // regions
    for r in brep1::regions(b, None)? {
        for s in region_shells(b, r)? {
            b.remove_shell_record(s)?;
        }
        b.remove_region_record(r)?;
    }
    b.set_region_list_head(None);
    b.set_infinite_region_record(None);

    unlock_bounding_box(b);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `delete pEU` / `delete pShell` / `delete pFace` (#556): each destructor notifies
    /// `SM_NO_DESTRUCTION` once per installed callback, the record leaves its arena and
    /// the object leaves its attributes while the other users stay.
    #[test]
    fn retained_edge_and_vertex_deletion_notify_the_actual_body() {
        use crate::geom::attribute::{ids, Attribute, Behavior};
        use crate::math::Point3;
        use crate::topology::edge::notification_test_support as callbacks;

        for delete_connected_vertex in [true, false] {
            let mut k = Kernel::default();
            let mut b = crate::primitive::api_primitives::create_box(
                &mut k,
                Point3::ZERO,
                10.0,
                20.0,
                30.0,
            )
            .unwrap();
            let serial = b.notify_serial().unwrap();
            b.set_editing_enabled(true);
            let legacy_key = b.key();
            let key_of = |entity| super::super::p3::topology_ref(serial, entity);
            let body = key_of(EntityRef::Brep(crate::topology::BrepId::from_index(0)));
            assert_ne!(legacy_key, body.key);
            let edge = b.edge_list()[0];
            let vertex = b.vertex_list()[0];
            let mut records = crate::topology::topology::NotifyRecords::new();
            super::super::p3::seed_records(&mut records, &b, &key_of);
            let mut attributes = AttributeStore::default();
            let marker = attributes
                .insert(Attribute::long(ids::USER_1, 917, Behavior::Reference))
                .unwrap();
            attributes.add_attribute(body.key, marker).unwrap();
            let events = callbacks::install(&mut k.context);
            k.context.set_doing_boolean(true);
            let result = Context::with_notification_context(&mut k, |k, context| {
                let mut env = DeleteEnv {
                    records: &mut records,
                    attributes: &mut attributes,
                    key_of: &key_of,
                    context: Some(context),
                };
                if delete_connected_vertex {
                    delete_vertex_with_body_identity(k, &mut b, &mut env, vertex, body)
                } else {
                    delete_edge_with_body_identity(k, &mut b, &mut env, edge, None, body)
                }
            })
            .unwrap();
            let entity = if delete_connected_vertex {
                EntityRef::Vertex(vertex)
            } else {
                EntityRef::Edge(edge)
            };
            if delete_connected_vertex {
                assert!(
                    matches!(result, Err(SolidError::Upstream { code, .. }) if code == crate::error::SM_ERR)
                );
                b.vertex(vertex).unwrap();
            } else {
                result.unwrap();
                assert!(!b.edge_list().contains(&edge));
            }
            assert_eq!(b.key(), legacy_key);
            assert!(k.context.doing_boolean());
            assert_eq!(attributes.users(marker).unwrap(), &[body.key]);
            callbacks::assert_installed(&k.context, &events);
            let events = events.lock().unwrap();
            assert_eq!(events[0].caller, body);
            assert_eq!(events[0].operation, NotifyOperation::PreEdit);
            assert_eq!(events[0].data, [Some(body), None, None]);
            assert!(events.iter().any(|event| event.caller == key_of(entity)
                && event.operation == NotifyOperation::PreEdit));
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
    fn edgeuse_shell_and_face_destructors_notify_and_clear_attributes() {
        use crate::curve::curve::Curve;
        use crate::geom::attribute::{Attribute, Behavior};
        use crate::topology::edge::notification_test_support as callbacks;
        use crate::topology::loopuse::notify_ref as key_of;

        let mut k = Kernel::default();
        let events = callbacks::install(&mut k.context);
        let mut b = Brep::new_with_infinite_region().unwrap();
        b.set_editing_enabled(true);
        let line = crate::curve::line::Line::from_points(Vec3::ZERO, Vec3::X, 3);
        let interval = line.natural_interval();
        let e = crate::topology::brep::p5::create_wire_edge_from_curve(
            &mut k,
            &mut b,
            Curve::from(line),
            interval,
        )
        .unwrap();
        let eu = b.edge(e).unwrap().edgeuses[0];
        let shell = eu1::shell(&b, eu).unwrap().unwrap();
        let domain = Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0);
        let plane = crate::surface::plane::Plane::create_canonical(
            &crate::geom::axis2_placement::Axis2Placement::IDENTITY,
        )
        .unwrap();
        let mut records = crate::topology::topology::NotifyRecords::new();
        let mut attributes = AttributeStore::default();
        let face = {
            let mut marks = Marks::default();
            let this_face = |f: FaceId| key_of(EntityRef::Face(f));
            let mut env = NotifyEnv {
                records: &mut records,
                attributes: &mut attributes,
                context: None,
                key_of: &key_of,
            };
            let mut fenv = crate::topology::brep::p8::FaceBuildEnv {
                env: &mut env,
                marks: &mut marks,
                this_face: &this_face,
            };
            crate::topology::brep::p8::create_face_from_surface(
                &mut k,
                &mut b,
                Some(crate::surface::surface::Surface::from(plane)),
                &domain,
                &mut fenv,
            )
            .unwrap()
        };
        let targets = [
            key_of(EntityRef::Edgeuse(eu)),
            key_of(EntityRef::Shell(shell)),
            key_of(EntityRef::Face(face)),
        ];
        let survivor = key_of(EntityRef::Edge(e));
        let shared = attributes
            .insert(Attribute::long(81, 912, Behavior::Reference))
            .unwrap();
        attributes.add_attribute(survivor.key, shared).unwrap();
        for t in targets {
            attributes.add_attribute(t.key, shared).unwrap();
        }
        events.lock().unwrap().clear();
        let mut env = NotifyEnv {
            records: &mut records,
            attributes: &mut attributes,
            context: None,
            key_of: &key_of,
        };
        eu2::destroy(&mut k, &mut b, eu, &mut env).unwrap();
        shell::destroy(&mut k, &mut b, shell, &mut env).unwrap();
        face3::destroy(&mut k, &mut b, face, &mut env).unwrap();
        assert!(b.edgeuse(eu).is_err() && b.shell(shell).is_err() && b.face(face).is_err());
        assert_eq!(attributes.users(shared).unwrap(), vec![survivor.key]);
        let destroyed: Vec<_> = events
            .lock()
            .unwrap()
            .iter()
            .filter(|ev| ev.operation == NotifyOperation::Destruction)
            .map(|ev| ev.caller)
            .collect();
        // (the user and the system callback, in that order, for each object)
        let expected: Vec<_> = targets.iter().flat_map(|&t| [t, t]).collect();
        assert_eq!(destroyed, expected);
        assert!(!k.context.doing_boolean());
    }

    #[test]
    fn stitch_of_an_empty_brep_stitches_nothing() {
        let mut k = Kernel::default();
        let mut b = Brep::new();
        let r = stitch_faces(&mut k, &mut b, 1e-3, None, false).unwrap();
        assert_eq!(r.stitched_edges, 0);
        assert_eq!(r.lamina_edges, 0);
        assert_eq!(r.max_vertex_edge_gap, 0.0);
        assert_eq!(r.max_edge_face_trim_curve_gap, 0.0);
    }

    #[test]
    fn removing_a_faceuse_moves_the_shell_head_and_keeps_the_faces_in_step() {
        let mut b = Brep::new();
        let s = b.add_shell_record(Shell::default()).unwrap();
        let mut fus = Vec::new();
        for _ in 0..3 {
            let f = b.add_face_record(crate::topology::Face::new(s)).unwrap();
            let fu = b
                .add_faceuse_record(crate::topology::Faceuse::default())
                .unwrap();
            b.faceuse_mut(fu).unwrap().face = Some(f);
            b.shell_mut(s).unwrap().shell_type = ShellType::Faceuse;
            shell_post_insert_faceuse(&mut b, s, fu).unwrap();
            fus.push(fu);
        }
        assert_eq!(shell_head(&b, s).unwrap(), Some(EntityRef::Faceuse(fus[0])));
        shell_remove_faceuse(&mut b, s, fus[0], OP_DELETE_EDGE).unwrap();
        assert_eq!(shell_head(&b, s).unwrap(), Some(EntityRef::Faceuse(fus[1])));
        assert_eq!(b.shell(s).unwrap().faces.len(), 2);
        assert_eq!(b.faceuse(fus[0]).unwrap().shell, None);
        // not on the list any more: SM_ERR, nothing changed
        assert!(shell_remove_faceuse(&mut b, s, fus[0], OP_DELETE_EDGE).is_err());
        assert_eq!(
            b.shell(s).unwrap().get_faceuses().unwrap(),
            vec![fus[1], fus[2]]
        );
    }

    #[test]
    fn removed_faces_leave_the_brep_face_list() {
        let mut b = Brep::new();
        let s = b.add_shell_record(Shell::default()).unwrap();
        let f0 = b.add_face_record(crate::topology::Face::new(s)).unwrap();
        let f1 = b.add_face_record(crate::topology::Face::new(s)).unwrap();
        b.remove_face_record(f0).unwrap();
        assert_eq!(brep1::faces(&b, None).unwrap(), vec![f1]);
        b.set_face_list(vec![f1]).unwrap();
    }

    #[test]
    fn remove_loop_without_loop_notifies_first() {
        // the notification comes before the null dereference upstream: it resets the
        // face's rectangular-trim flag, then the null loop is the dereference error
        let mut b = Brep::new();
        let s = b.add_shell_record(Shell::default()).unwrap();
        let f = b.add_face_record(crate::topology::Face::new(s)).unwrap();
        b.face_mut(f).unwrap().rectangular_trim = face2::UNSURE + 1;
        let r = face_remove_loop(&mut b, f, None);
        assert!(matches!(r, Err(SolidError::InvalidInput { .. })));
        assert_eq!(b.face(f).unwrap().rectangular_trim, face2::UNSURE);
    }
}
