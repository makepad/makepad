//! B-rep editing, part 5: coincident-vertex targets and dependency-limited edits.
//!
//! Upstream: `source/SMLib/src/SmBrep.cpp`, `source/SMLib/inc/SmBrep.h`.
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`coincident_vertex_target`], [`combine_coincident_vertices`] | `SmBrep::CombineCoincidentVertices` |
//! | [`create_wire_edge_from_curve`] | `SmBrep::CreateWireEdgeFromCurve` |
//! | [`delete_topological_vertex`] | `SmBrep::DeleteTopologicalVertex` |
//! | [`find_and_set_infinite_region`] | `SmBrep::FindAndSetInfiniteRegion` |
//! | [`glue_vertices`] | `SmBrep::GlueVertices` |
//! | [`make_edge_in_face`], [`make_edge_in_face_with_uv_curve`], [`make_edge_in_face_in`] | `SmBrep::MakeEdgeInFace` |
//! | [`make_manifold`] | `SmBrep::MakeManifold` |
//! | [`remove_topological_edges_and_vertices`] | `SmBrep::RemoveTopologicalEdgesAndVertices` |
//! | [`remove_faces`] | `SmBrep::RemoveFaces` |
//! | [`remove_faces_notified`] | `SmBrep::RemoveFaces` (caller's records) |
//! | [`stitch_and_orient`] | `SmBrep::StitchAndOrient` |
//! | [`squeeze_face`] | `SmBrep::SqueezeFace` |
//! | [`transform`] | `SmBrep::Transform(const SmAxis2Placement&, ...)` (the 4x4 matrix overload is not owned by this unit) |
//! | [`turn_to_nurbs_plain`] | `SmBrep::TurnToNURBS()` as a plain entry point (the edit is `topology.brep_p3`'s) |
//! | [`PendingTopology`] | `SmPendingTopologyDelete` constructor/destructor and `Clear` (co-port) |
//!
//! Deviations: dependency-limited editing. All twelve owned editing
//! control flows are present, including continuations after missing calls.
//! Individual missing callees remain explicit dependency deferrals. None of the
//! deferred mutation paths is certified by numerical-target-only cases.
//! Fresh-mark collection uses ordered local deduplication and still acquires and
//! releases the native context mark. Separate curve/surface slots are distinct
//! owned values; native shared geometry across different owners is not represented
//! and remains uncertified. Equal owner metadata is never used to invent aliasing.
//! Notification environments borrow caller records and keys. A plain entry point (no
//! environment; the `_with_notifications` forms given `None`) runs its whole
//! edit in one environment under the kernel's identities (`plain_edit`,
//! `topology::brep::p3::with_kernel_callbacks`: `b`'s serial in the kernel context's
//! domain, so two B-reps' objects never share a key, notifying through the kernel
//! context's callbacks lent to the edit); one called inside another lent edit is refused
//! before anything changes. `topology.brep_p3`'s edits (`SqueezeEdge`, `DeleteFace`,
//! `MakeWireEdgeVertex`, `OrientTrimmedSurfaces`, `TurnToNURBS`) and `MakeShellVertex`
//! (`topology.brep_p9`) notify through that environment, so [`make_manifold`] and
//! [`stitch_and_orient`] take the kernel. Curves have no identity in the port: `SetCurve`'s new/old curve
//! data are null and a curve's own `SM_NO_CHANGE_OWNER` is not delivered.
//! `SmBrep::Notify` is owned by `topology.brep_p2`, not this unit. Owning-list
//! traversal (`GetRegions`, `GetFaces`, `GetEdges`, `GetVertices`) and the editing
//! flag are `topology.brep_p1`'s list-order collections and `Brep` member. Pointer inputs are optional typed ids;
//! out-parameters are mutable outputs where failure must clear them. The old-tol
//! target calculation is factored out for direct numerical-stage verification;
//! that verification does not certify vertex deletion or gluing. The internal
//! `PendingTopology` owns an allocation until release and borrows the actual
//! attribute store so failure detaches borrowed attributes without deleting them.
//! Its reader integration remains uncertified until `MakeTopologyFromData` lands.
//! Aliased `CombineCoincidentVertices` takes the native path in every case,
//! including an isolated vertex, which upstream deletes along with its survivor
//! before returning success.
//! Shell/region queries and transfers use the landed shell port. Stable shell,
//! vertex and vertex-use record erasure requires part 4's owning-storage lifecycle: the shared
//! arena is append-only, so shifting ids or leaving a visible dummy is invalid.
//! Debug drawing/assertions are omitted. No recursion is introduced.

use crate::curve::curve::Curve;
use crate::error::{Result, SolidError};
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::context::{MarkLock, ALL_MARKS};
use crate::geom::extent1d::Extent1d;
use crate::geom::extent3d::Extent3d;
use crate::geom::object::{NotifyOperation, ObjectRef};
use crate::geom::tol::ZONE_TOL_3D;
use crate::kernel::Kernel;
use crate::math::{max2, Point3, Vec3};
use crate::nurbs::b_spline_curve::BSplineCurve;
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::query::brep::BrepQuery;
use crate::topology::brep::p1::SetRegionVoidType;
use crate::topology::brep::p2 as brep_p2;
use crate::topology::face::p4 as face_p4;
use crate::topology::VertexuseId;
use crate::topology::{vertex, Brep, EdgeId, FaceId, LoopId, RegionId, ShellId, VertexId};

/// A pending topology allocation with attributes borrowed from a database reader.
/// On failure/drop, detaches each attribute with native `bDoNotDelete = TRUE`
/// before dropping the object. [`Self::release`] transfers a successful allocation.
pub struct PendingTopology<'a, T> {
    object: Option<(crate::geom::attribute::ObjectKey, T)>,
    attributes: &'a mut crate::geom::attribute::AttributeStore,
}

impl<'a, T> PendingTopology<'a, T> {
    /// Guards a pending allocation, or the native null-pointer case (`None`).
    pub fn new(
        object: Option<(crate::geom::attribute::ObjectKey, T)>,
        attributes: &'a mut crate::geom::attribute::AttributeStore,
    ) -> Self {
        Self { object, attributes }
    }

    /// Accesses the pending object, when one exists.
    pub fn object_mut(&mut self) -> Option<&mut T> {
        self.object.as_mut().map(|(_, object)| object)
    }

    /// Transfers the allocation and its actual key without detaching attributes
    /// (`SmPendingTopologyDelete::Clear`).
    pub fn release(mut self) -> Option<(crate::geom::attribute::ObjectKey, T)> {
        self.object.take()
    }
}

impl<T> Drop for PendingTopology<'_, T> {
    fn drop(&mut self) {
        if let Some((key, object)) = self.object.take() {
            for attribute in self.attributes.attributes(key) {
                // Native ignores RemoveAttribute's status. The store remains
                // exclusively borrowed by the guard throughout this cleanup.
                let _ = self.attributes.remove_attribute(key, attribute, true);
            }
            drop(object);
        }
    }
}

const OP: &str = "topology::brep::p5";

/// Runs `f` with the caller's notification environment, or, for a plain entry point
/// (`notifications` is `None`: the caller keeps no notification state of its own),
/// with an [`brep_p2::ArenaNotifyState`] of `b` (holding `b`'s own attributes for the
/// edit) notifying through `context`. `f`'s last
/// argument is the context the caller's own environment falls back to (`context`
/// then; `None` for the arena state, which already holds it).
pub(super) fn with_notifications<R>(
    b: &mut Brep,
    notifications: Option<&mut EditNotifyEnv<'_>>,
    context: Option<&mut crate::geom::context::Context>,
    f: impl FnOnce(
        &mut Brep,
        &mut EditNotifyEnv<'_>,
        Option<&mut crate::geom::context::Context>,
    ) -> Result<R>,
) -> Result<R> {
    if let Some(env) = notifications {
        f(b, env, context)
    } else {
        // (not upstream) with `b`'s own attributes, given back afterwards
        let mut state = brep_p2::ArenaNotifyState::with_attributes_of(b);
        let r = f(b, &mut state.env(context), None);
        let restored = state.restore_attributes(b);
        let r = r?;
        restored?;
        Ok(r)
    }
}

/// Runs the `topology.brep_p3` edit `f` of `b` with the caller's notification
/// environment. Every entry point reaching one passes its environment (a plain entry
/// point's is [`plain_edit`]'s); without one the edit is a plain edit of its own.
fn with_p3_notifications<R>(
    k: &mut Kernel,
    b: &mut Brep,
    notifications: Option<&mut EditNotifyEnv<'_>>,
    f: impl FnOnce(&mut Kernel, &mut Brep, &mut EditNotifyEnv<'_>) -> Result<R>,
) -> Result<R> {
    match notifications {
        Some(env) => f(k, b, env),
        None => plain_edit(k, b, f),
    }
}

/// A plain entry point's edit (the caller keeps no notification state of its own):
/// `edit` runs with the kernel's identities for `b`'s objects
/// ([`crate::topology::brep::p3::with_kernel_callbacks`]: `b`'s serial in the kernel
/// context's identity domain, so two B-reps' objects never share a key), notifying
/// through the kernel context's callbacks, lent to the edit. One environment covers
/// the whole edit, so every object keeps one identity in it. A plain entry point
/// called inside another lent edit is refused before anything changes.
pub(super) fn plain_edit<R>(
    k: &mut Kernel,
    b: &mut Brep,
    edit: impl FnOnce(&mut Kernel, &mut Brep, &mut EditNotifyEnv<'_>) -> Result<R>,
) -> Result<R> {
    crate::topology::brep::p3::with_kernel_callbacks(k, b, edit)
}

fn require_editing(enabled: bool) -> Result<()> {
    if enabled {
        Ok(())
    } else {
        Err(SolidError::sm_err(OP))
    }
}

/// The position, tolerance and temporary-edge length threshold calculated before
/// `CombineCoincidentVertices` modifies topology (`SM_USE_OLDTOL`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CoincidentVertexTarget {
    /// Position of the vertex with more incident edges; ties keep the survivor.
    pub point: Point3,
    /// The tolerance assigned after combining the vertices.
    pub tolerance: f64,
    /// `dMinDist`, used when deciding whether a common edge is short enough to squeeze.
    pub minimum_edge_length: f64,
}

/// Computes the initial old-tolerance stage of `CombineCoincidentVertices`.
/// Does not edit either vertex. Distances and the strict edge-count/tolerance
/// comparisons retain the native operation order.
pub fn coincident_vertex_target(
    b: &Brep,
    survivor: VertexId,
    deleted: VertexId,
) -> Result<CoincidentVertexTarget> {
    let edges = vertex::edges(b, survivor, None)?;
    let edges2 = vertex::edges(b, deleted, None)?;
    let s = b.vertex(survivor)?;
    let d = b.vertex(deleted)?;
    let mut point = s.point;
    let mut min_dist = point.distance(d.point);
    if min_dist > d.tolerance {
        let doubled = 2.0 * min_dist;
        min_dist = doubled;
    } else {
        min_dist += d.tolerance;
    }
    let mut tolerance = max2(s.tolerance, min_dist);
    if edges2.len() > edges.len() {
        min_dist = point.distance(d.point);
        if min_dist > s.tolerance {
            let doubled = 2.0 * min_dist;
            min_dist = doubled;
        } else {
            min_dist += s.tolerance;
        }
        point = d.point;
        tolerance = max2(d.tolerance, min_dist);
    }
    Ok(CoincidentVertexTarget {
        point,
        tolerance,
        minimum_edge_length: min_dist,
    })
}

/// Combines coincident vertices. Clears the deleted-edge output before reading
/// either vertex, including failure paths. The target stage is available, but
/// successful deletion and gluing remain dependency limited.
pub fn combine_coincident_vertices(
    k: &mut Kernel,
    b: &mut Brep,
    survivor: VertexId,
    deleted: VertexId,
    deleted_edge: &mut Option<EdgeId>,
) -> Result<()> {
    combine_with_notifications(k, b, survivor, deleted, deleted_edge, None)
}

/// The existing topology notification environment: real records, attributes,
/// context and entity-to-object mapping, shared with edge editing.
pub use crate::topology::edge::NotifyEnv as EditNotifyEnv;

/// `CombineCoincidentVertices` with the actual notification environment needed
/// by `SmVertex::SetPoint`. The arena-only entry defers when it reaches this
/// environment boundary; supplying it never bypasses an unported native callee.
pub fn combine_with_notifications(
    k: &mut Kernel,
    b: &mut Brep,
    survivor: VertexId,
    deleted: VertexId,
    deleted_edge: &mut Option<EdgeId>,
    notifications: Option<&mut EditNotifyEnv<'_>>,
) -> Result<()> {
    let Some(env) = notifications else {
        return plain_edit(k, b, |k, b, env| {
            combine_with_notifications(k, b, survivor, deleted, deleted_edge, Some(env))
        });
    };
    let mut notifications = Some(env);
    *deleted_edge = None;
    let target = coincident_vertex_target(b, survivor, deleted)?;
    let edges = vertex::edges(b, survivor, None)?;
    let edges2 = vertex::edges(b, deleted, None)?;
    if edges2.is_empty() {
        // Upstream ignores Notify's status here.
        crate::error::ignore_status(notify_merge(
            k,
            b,
            survivor,
            deleted,
            notifications.as_deref_mut(),
        ))?;
        set_vertex_point(k, b, survivor, target.point, notifications.as_deref_mut())?;
        crate::error::ignore_status(vertex::set_tolerance(
            b,
            survivor,
            target.tolerance,
            true,
            true,
        ))?;
        crate::error::ignore_status(delete_vertex(k, b, deleted, notifications.as_deref_mut()))?;
        return Ok(());
    } else if edges.is_empty() {
        let first_uses = vertex::vertexuses(b, survivor)?;
        let last_uses = vertex::vertexuses(b, deleted)?;
        for vu in first_uses {
            crate::error::ignore_status(remove_vertexuse(b, survivor, vu))?;
            crate::error::ignore_status(post_insert_vertexuse(b, deleted, vu))?;
        }
        for vu in last_uses {
            crate::error::ignore_status(remove_vertexuse(b, deleted, vu))?;
            crate::error::ignore_status(post_insert_vertexuse(b, survivor, vu))?;
        }
        // Upstream ignores Notify's status here.
        crate::error::ignore_status(notify_merge(
            k,
            b,
            survivor,
            deleted,
            notifications.as_deref_mut(),
        ))?;
        set_vertex_point(k, b, survivor, target.point, notifications.as_deref_mut())?;
        crate::error::ignore_status(vertex::set_tolerance(
            b,
            survivor,
            target.tolerance,
            true,
            true,
        ))?;
        crate::error::ignore_status(delete_vertex(k, b, deleted, notifications.as_deref_mut()))?;
        return Ok(());
    }
    let mut common = None;
    for e in edges {
        if crate::topology::edge::other_vertex(b, e, Some(survivor))? != Some(deleted) {
            continue;
        }
        let eus = &b.edge(e)?.edgeuses;
        let lamina = if let [head, _] = eus.as_slice() {
            crate::topology::edgeuse::p1::is_loop_edgeuse(b, *head)?
        } else {
            false
        };
        if crate::topology::edge::is_manifold(b, e)? || lamina {
            common = Some(e);
            break;
        }
    }
    if let Some(e) = common {
        let record = b.edge(e)?;
        let curve = record
            .curve
            .as_ref()
            .ok_or_else(|| SolidError::invalid_input(OP, "common edge without curve"))?;
        let length = crate::error::ignored(curve.length_with_zone_tol(
            k,
            &record.interval,
            target.tolerance,
            Some(record.tolerance),
        ))?
        .unwrap_or(0.0);
        if length > target.minimum_edge_length {
            // Retain a long common edge; GlueVertices will close it.
            common = None;
        } else {
            *deleted_edge = Some(e);
        }
        // A long common edge is retained as a closed edge by GlueVertices.
    } else {
        let faces = vertex::faces(b, survivor)?;
        let faces2 = vertex::faces(b, deleted)?;
        if let Some(face) = faces.into_iter().find(|f| faces2.contains(f)) {
            for vu in vertex::vertexuses(b, survivor)? {
                let faceuse = crate::topology::vertexuse::faceuse(b, vu)?;
                let owning_face = match faceuse {
                    Some(fu) => b.faceuse(fu)?.face,
                    None => None,
                };
                if owning_face == Some(face)
                    && matches!(
                        b.vertexuse(vu)?.owner,
                        crate::topology::VertexuseOwner::Loopuse(_)
                    )
                {
                    return Err(SolidError::sm_err(OP));
                }
            }
            let start = b.vertex(survivor)?.point;
            let mut end = b.vertex(deleted)?.point;
            if crate::geom::tol::in_tol(
                start.distance(end),
                crate::geom::tol::scaled_zero2(&start, &end),
            ) {
                end = start + Vec3::new(target.tolerance, target.tolerance, target.tolerance);
            }
            // Preserve failures from the temporary line before the edit metadata is
            // consulted.
            let line = BSplineCurve::create_line_segment(3, start, end)?;
            let interval = line.natural_interval();
            let editing = b.editing_enabled();
            let mut output = EdgeInFace::default();
            make_edge_in_face(
                k,
                b,
                editing,
                Some(face),
                Some(survivor),
                Some(deleted),
                Curve::from(line),
                None,
                interval,
                OrientType::Same,
                crate::math::EFF_ZERO,
                &mut output,
                notifications.as_deref_mut(),
            )?;
            common = output.edge;
        }
    }
    if let Some(edge) = common {
        squeeze_edge(k, b, notifications.as_deref_mut(), edge, survivor)?;
    } else {
        crate::error::ignore_status(glue_with_notifications(
            k,
            b,
            Some(survivor),
            Some(deleted),
            None,
            notifications.as_deref_mut(),
        ))?;
    }
    set_vertex_point(k, b, survivor, target.point, notifications.as_deref_mut())?;
    crate::error::ignore_status(vertex::set_tolerance(
        b,
        survivor,
        target.tolerance,
        true,
        true,
    ))?;
    let env = notifications.ok_or_else(|| SolidError::null_pointer(OP))?;
    crate::topology::brep::p1::update_vertex_edge_intervals(k, b, env, survivor)
}

/// `Notify(SM_NO_MERGE_IN_BREP, pSurvivingVertex, pVertexToDelete, this)`
/// ([`brep_p2::notify`], no B-rep cache held by the caller).
fn notify_merge(
    k: &mut Kernel,
    b: &mut Brep,
    survivor: VertexId,
    deleted: VertexId,
    notifications: Option<&mut EditNotifyEnv<'_>>,
) -> Result<()> {
    notify_merge_with_body(k, b, survivor, deleted, notifications, None)
}

fn notify_merge_with_body(
    k: &mut Kernel,
    b: &mut Brep,
    survivor: VertexId,
    deleted: VertexId,
    notifications: Option<&mut EditNotifyEnv<'_>>,
    body: Option<ObjectRef>,
) -> Result<()> {
    let data = [
        Some(brep_p2::NotifyArg::Entity(crate::error::EntityRef::Vertex(
            survivor,
        ))),
        Some(brep_p2::NotifyArg::Entity(crate::error::EntityRef::Vertex(
            deleted,
        ))),
        Some(brep_p2::NotifyArg::Object(
            body.unwrap_or_else(|| brep_p2::brep_ref(b)),
        )),
    ];
    with_notifications(b, notifications, Some(&mut k.context), |b, env, _| {
        brep_p2::notify_with_body_identity(
            b,
            env,
            None,
            NotifyOperation::MergeInBrep,
            data,
            body.unwrap_or_else(|| brep_p2::brep_ref(b)),
        )
    })
}

/// `SmBrep::DeleteVertex(vertex)` with the caller's notification records, attributes
/// and identities, or (without them) on call-local records
/// ([`crate::topology::brep::p4::with_local_env`]). The context is the kernel's.
fn delete_vertex(
    k: &mut Kernel,
    b: &mut Brep,
    vertex: VertexId,
    notifications: Option<&mut EditNotifyEnv<'_>>,
) -> Result<()> {
    use crate::topology::brep::p4;
    match notifications {
        Some(env) => {
            let mut d = p4::DeleteEnv::from_notify(env);
            p4::delete_vertex(k, b, &mut d, vertex)
        }
        None => p4::with_local_env(|d| p4::delete_vertex(k, b, d, vertex)),
    }
}

/// `SmBrep::DeleteEdge(edge, pOptSurfaceToKeep)`, environment as [`delete_vertex`].
fn delete_edge_with(
    k: &mut Kernel,
    b: &mut Brep,
    edge: EdgeId,
    surface_to_keep: Option<FaceId>,
    notifications: Option<&mut EditNotifyEnv<'_>>,
) -> Result<()> {
    use crate::topology::brep::p4;
    match notifications {
        Some(env) => {
            let mut d = p4::DeleteEnv::from_notify(env);
            p4::delete_edge(k, b, &mut d, edge, surface_to_keep)
        }
        None => p4::with_local_env(|d| p4::delete_edge(k, b, d, edge, surface_to_keep)),
    }
}

/// `SmBrep::SqueezeEdge(edge, survivor)` ([`crate::topology::brep::p3::squeeze_edge`]),
/// notifying as [`with_p3_notifications`].
fn squeeze_edge(
    k: &mut Kernel,
    b: &mut Brep,
    notifications: Option<&mut EditNotifyEnv<'_>>,
    edge: EdgeId,
    survivor: VertexId,
) -> Result<()> {
    with_p3_notifications(k, b, notifications, |k, b, env| {
        crate::topology::brep::p3::squeeze_edge(k, b, env, edge, survivor)
    })
}

fn remove_vertexuse(b: &mut Brep, vertex: VertexId, vu: VertexuseId) -> Result<()> {
    if b.vertexuse(vu)?.vertex != Some(vertex) {
        return Err(SolidError::sm_err(OP));
    }
    let index = b
        .vertex(vertex)?
        .vertexuses
        .iter()
        .position(|&id| id == vu)
        .ok_or_else(|| SolidError::sm_err(OP))?;
    b.vertex_mut(vertex)?.vertexuses.remove(index);
    b.vertexuse_mut(vu)?.vertex = None;
    Ok(())
}

fn post_insert_vertexuse(b: &mut Brep, vertex: VertexId, vu: VertexuseId) -> Result<()> {
    b.vertex(vertex)?;
    if b.vertexuse(vu)?.vertex.is_some() {
        return Err(SolidError::sm_err(OP));
    }
    b.vertex_mut(vertex)?.vertexuses.push(vu);
    b.vertexuse_mut(vu)?.vertex = Some(vertex);
    Ok(())
}

pub(super) fn set_vertex_point(
    k: &mut Kernel,
    b: &mut Brep,
    vertex: VertexId,
    point: Point3,
    notifications: Option<&mut EditNotifyEnv<'_>>,
) -> Result<()> {
    with_notifications(
        b,
        notifications,
        Some(&mut k.context),
        |b, env, fallback| {
            // `SmVertex::SetPoint` returns void: its notification statuses are dropped.
            crate::error::ignore_status(vertex::set_point(
                b,
                vertex,
                point,
                env.records,
                env.attributes,
                env.context.as_deref_mut().or(fallback),
                (env.key_of)(crate::error::EntityRef::Vertex(vertex)),
                &|vu| (env.key_of)(crate::error::EntityRef::Vertexuse(vu)),
            ))
        },
    )
}

/// Makes a wire edge over the caller's curve interval. Evaluates start before end,
/// then needs the owning B-rep's infinite region; the owned curve drops on error.
pub fn create_wire_edge_from_curve(
    k: &mut Kernel,
    b: &mut Brep,
    curve: Curve,
    interval: Extent1d,
) -> Result<EdgeId> {
    plain_edit(k, b, |k, b, env| {
        create_wire_edge_from_curve_in(k, b, curve, interval, env)
    })
}

/// [`create_wire_edge_from_curve`] notifying through the caller's environment `env`
/// (a caller that keeps its own notification state, e.g. a copy into a store body).
///
/// # Errors
///
/// As [`create_wire_edge_from_curve`].
pub(crate) fn create_wire_edge_from_curve_notified(
    k: &mut Kernel,
    b: &mut Brep,
    curve: Curve,
    interval: Extent1d,
    env: &mut EditNotifyEnv<'_>,
) -> Result<EdgeId> {
    create_wire_edge_from_curve_in(k, b, curve, interval, env)
}

/// [`create_wire_edge_from_curve`] under the edit's notification environment ([`plain_edit`]).
fn create_wire_edge_from_curve_in(
    k: &mut Kernel,
    b: &mut Brep,
    curve: Curve,
    interval: Extent1d,
    env: &mut EditNotifyEnv<'_>,
) -> Result<EdgeId> {
    let start = curve.evaluate_point(k, interval.min())?;
    let end = curve.evaluate_point(k, interval.max())?;
    let region = b
        .infinite_region()
        .ok_or_else(|| SolidError::null_pointer(OP))?;
    let (first, edge, last) = with_p3_notifications(k, b, Some(&mut *env), |k, b, env| {
        let (_, first) = make_shell_vertex(b, region, start, env)?;
        let (edge, last) = make_wire_edge_vertex(
            k,
            b,
            env,
            region,
            first,
            curve,
            interval,
            OrientType::Same,
            end,
        )?;
        Ok((first, edge, last))
    })?;
    if start.distance(end) < b.tolerance() {
        crate::error::ignore_status(glue_with_notifications(
            k,
            b,
            Some(first),
            Some(last),
            None,
            Some(&mut *env),
        ))?;
    }
    Ok(edge)
}

/// `SmBrep::MakeShellVertex(region, point)` ([`super::p9::make_shell_vertex`]).
fn make_shell_vertex(
    b: &mut Brep,
    r: RegionId,
    point: Point3,
    env: &mut EditNotifyEnv<'_>,
) -> Result<(ShellId, VertexId)> {
    super::p9::make_shell_vertex(b, Some(r), point, env)
}

/// `SmBrep::MakeWireEdgeVertex(region, first, curve, interval, orientation, end)`
/// ([`crate::topology::brep::p3::make_wire_edge_vertex`]).
#[allow(clippy::too_many_arguments)]
fn make_wire_edge_vertex(
    k: &mut Kernel,
    b: &mut Brep,
    env: &mut EditNotifyEnv<'_>,
    r: RegionId,
    first: VertexId,
    curve: Curve,
    interval: Extent1d,
    orientation: OrientType,
    end: Point3,
) -> Result<(EdgeId, VertexId)> {
    crate::topology::brep::p3::make_wire_edge_vertex(
        k,
        b,
        env,
        Some(r),
        first,
        curve,
        &interval,
        orientation,
        end,
    )
}

/// Deletes a degree-two topological vertex, optionally selecting the shorter edge.
/// The native editing-disabled refusal occurs before Notify and input checks.
pub fn delete_topological_vertex(
    k: &mut Kernel,
    b: &mut Brep,
    editing_enabled: bool,
    vertex: Option<VertexId>,
    edge: Option<EdgeId>,
) -> Result<()> {
    delete_topological_vertex_with_notifications(k, b, editing_enabled, vertex, edge, None)
}

/// Deletes a topological vertex with the caller's real notification environment.
pub fn delete_topological_vertex_with_notifications(
    k: &mut Kernel,
    b: &mut Brep,
    editing_enabled: bool,
    vertex: Option<VertexId>,
    edge: Option<EdgeId>,
    notifications: Option<&mut EditNotifyEnv<'_>>,
) -> Result<()> {
    let Some(env) = notifications else {
        return plain_edit(k, b, |k, b, env| {
            delete_topological_vertex_with_notifications(
                k,
                b,
                editing_enabled,
                vertex,
                edge,
                Some(env),
            )
        });
    };
    let mut notifications = Some(env);
    require_editing(editing_enabled)?;
    crate::error::ignore_status(notify_pre_edit(k, b, notifications.as_deref_mut()))?;
    let vertex = vertex.ok_or_else(|| SolidError::null_pointer(OP))?;
    let edges = vertex::edges(b, vertex, None)?;
    let [first, last] = edges.as_slice() else {
        return Err(SolidError::sm_err(OP));
    };
    let deleted = if let Some(edge) = edge {
        edge
    } else {
        let length1 = edge_curve(b, *first)?
            .approximate_length(k, &b.edge(*first)?.interval, 5, false)?
            .0;
        let length2 = edge_curve(b, *last)?
            .approximate_length(k, &b.edge(*last)?.interval, 5, false)?
            .0;
        if length2 < length1 {
            *last
        } else {
            *first
        }
    };
    if deleted != *first && deleted != *last {
        return Err(SolidError::sm_err(OP));
    }
    let survivor = if *first == deleted { *last } else { *first };
    let join_survivor = crate::topology::edge::start_vertex(b, survivor)? != Some(vertex);
    let join_deleted = crate::topology::edge::start_vertex(b, deleted)? != Some(vertex);
    let max_gap = crate::math::max3(
        b.edge(survivor)?.tolerance,
        b.edge(deleted)?.tolerance,
        b.vertex(vertex)?.tolerance,
    );
    let survivor_interval = b.edge(survivor)?.interval;
    let deleted_interval = b.edge(deleted)?.interval;
    // Curves in separate arena records are distinct owned values. Equal owner
    // metadata does not prove native pointer identity. Shared native geometry
    // cannot currently be represented by these slots; do not guess aliasing from
    // that metadata. Coincidence classification still handles equal geometry.
    let mut same_geometry = false;
    if !same_geometry {
        let delete_curve = edge_curve_with_tolerance(b, deleted)?;
        let survive_curve = edge_curve_with_tolerance(b, survivor)?;
        let deleted_range = delete_curve.natural_interval();
        let survivor_range = survive_curve.natural_interval();
        // The native overload defaults bSnapToEnds to TRUE. Its additional-work
        // output is unused by this caller.
        let coincidence =
            crate::error::ignored(delete_curve.global_coincidence_checker::<JoinCurve>(
                k,
                &deleted_range,
                &survive_curve,
                &survivor_range,
                0.001,
                true,
                [
                    Some(JoinCurve::Deleted(deleted_range)),
                    Some(JoinCurve::Survivor(survivor_range)),
                ],
            ))?;
        if let Some(coincidence) = coincidence {
            if let [solution] = coincidence.solutions.as_slice() {
                if solution.solution_type
                    == crate::geom::solution_array::SolutionType::RangeOfValues
                {
                    let ds0 = *solution
                        .start
                        .parameters
                        .first()
                        .ok_or_else(|| SolidError::sm_err(OP))?;
                    let ds1 = *solution
                        .end
                        .parameters
                        .first()
                        .ok_or_else(|| SolidError::sm_err(OP))?;
                    let ss0 = *solution
                        .start
                        .parameters
                        .get(1)
                        .ok_or_else(|| SolidError::sm_err(OP))?;
                    let ss1 = *solution
                        .end
                        .parameters
                        .get(1)
                        .ok_or_else(|| SolidError::sm_err(OP))?;
                    if (ds1 - ds0) * (ss1 - ss0) > 0.0 {
                        let far = if join_deleted {
                            deleted_interval.min()
                        } else {
                            deleted_interval.max()
                        };
                        if survivor_range.contains_value(far, crate::math::EFF_ZERO) {
                            let far_vertex =
                                crate::topology::edge::other_vertex(b, deleted, Some(vertex))?
                                    .ok_or_else(|| SolidError::null_pointer(OP))?;
                            let point = b.vertex(far_vertex)?.point;
                            // Native ignores EvaluatePoint's status. Its defined
                            // SM_UNDEF_DOUBLE sentinel remains explicit when an ordinary evaluation fails.
                            let evaluated =
                                crate::error::ignored(survive_curve.evaluate_point(k, far))?
                                    .unwrap_or(Vec3::UNDEF);
                            if evaluated.closer_than(max_gap, point) {
                                same_geometry = true;
                            }
                        }
                    }
                }
            }
        }
    }
    if same_geometry {
        let far = if join_survivor {
            deleted_interval.max()
        } else {
            deleted_interval.min()
        };
        b.edge_mut(survivor)?.interval.add_value(far);
    } else {
        let mut deleted_copy = edge_curve(b, deleted)?.copy(k)?;
        let mut interval = deleted_interval;
        deleted_copy.trim(
            k,
            &mut interval,
            crate::nurbs::b_spline_curve_n_lib::TrimMode::Notify,
        )?;
        let mut surviving_copy = edge_curve(b, survivor)?.copy(k)?;
        interval = survivor_interval;
        surviving_copy.trim(
            k,
            &mut interval,
            crate::nurbs::b_spline_curve_n_lib::TrimMode::Notify,
        )?;
        if crate::error::ignored(join_copied_curves(
            k,
            &mut surviving_copy,
            join_survivor,
            &mut deleted_copy,
            join_deleted,
            max_gap,
        ))?
        .is_none()
        {
            return Err(SolidError::sm_err(OP));
        }
        if let Some(analytic) =
            surviving_copy.create_analytic_curve(k, &surviving_copy.natural_interval())?
        {
            surviving_copy = analytic;
        }
        set_joined_edge_curve(k, b, survivor, None, true, notifications.as_deref_mut())?;
        let interval = surviving_copy.natural_interval();
        set_joined_edge_curve(
            k,
            b,
            survivor,
            Some(surviving_copy),
            true,
            notifications.as_deref_mut(),
        )?;
        b.edge_mut(survivor)?.interval = interval;
        set_edge_curve_owner(b, survivor, notifications.as_deref_mut())?;
    }
    for eu in crate::topology::edge::edgeuses(b, survivor, None, None)? {
        let eu = b.edgeuse_mut(eu)?;
        eu.uv_trim_curve = None;
        eu.drop_curve_fail = None;
    }
    crate::error::ignore_status(notify_edge_merge(
        k,
        b,
        survivor,
        deleted,
        notifications.as_deref_mut(),
    ))?;
    let surviving_vertex = crate::topology::edge::other_vertex(b, deleted, Some(vertex))?
        .ok_or_else(|| SolidError::null_pointer(OP))?;
    squeeze_edge(
        k,
        b,
        notifications.as_deref_mut(),
        deleted,
        surviving_vertex,
    )?;
    crate::error::ignore_status(notify_edge_pre_edit(k, b, survivor, notifications))?;
    if !crate::topology::edge::is_wire(b, survivor)? {
        for eu in crate::topology::edge::edgeuses(b, survivor, None, None)? {
            get_or_create_uv_trim_curve(k, b, eu)?;
        }
    }
    Ok(())
}

fn edge_curve(b: &Brep, edge: EdgeId) -> Result<&Curve> {
    b.edge(edge)?
        .curve
        .as_ref()
        .ok_or_else(|| SolidError::null_pointer(OP))
}

/// An evaluation copy of the edge's curve carrying the edge's tolerance
/// ([`Curve::set_owner_zone_tol_3d`]), so the caches its solves build use it as
/// upstream's over the edge-owned curve (`SmTol::GetApproxTol3d(&m_crCurve)`).
fn edge_curve_with_tolerance(b: &Brep, edge: EdgeId) -> Result<Curve> {
    let mut curve = edge_curve(b, edge)?.clone_for_evaluation()?;
    curve.set_owner_zone_tol_3d(Some(b.edge(edge)?.tolerance));
    Ok(curve)
}

/// The two curves of `DeleteTopologicalVertex`'s coincidence check as its solutions
/// name them (upstream's `pDeleteCurve`, `pSurviveCurve`), with the natural interval
/// `IdenticalSolutions` reads through them (`SmCurve::GetNaturalInterval`).
#[derive(Clone, Copy, PartialEq)]
enum JoinCurve {
    Deleted(Extent1d),
    Survivor(Extent1d),
}
impl crate::geom::solution_array::SolutionObject for JoinCurve {
    fn kind(&self) -> crate::geom::solution_array::ObjectKind {
        crate::geom::solution_array::ObjectKind::Curve
    }
    fn natural_interval(&self) -> Option<Extent1d> {
        match self {
            Self::Deleted(range) | Self::Survivor(range) => Some(*range),
        }
    }
}
fn join_copied_curves(
    k: &Kernel,
    survivor: &mut Curve,
    survivor_end: bool,
    deleted: &mut Curve,
    deleted_end: bool,
    gap: f64,
) -> Result<()> {
    use crate::curve::curve::CurveKind;
    let owner = survivor.owner();
    match survivor.kind_mut() {
        CurveKind::Line(line) => line.join_with(
            u64::from(survivor_end),
            deleted,
            u64::from(deleted_end),
            Some(gap),
        ),
        CurveKind::Ellipse(ellipse) => ellipse.join_with(
            k,
            u64::from(survivor_end),
            deleted,
            u64::from(deleted_end),
            Some(gap),
            owner,
        ),
        _ => bspline_curve_base_mut(survivor)?.join_with(
            k,
            survivor_end,
            bspline_curve_base_mut(deleted)?,
            deleted_end,
            Some(gap),
        ),
    }
}
fn bspline_curve_base_mut(curve: &mut Curve) -> Result<&mut BSplineCurve> {
    use crate::curve::curve::CurveKind;
    match curve.kind_mut() {
        CurveKind::BSpline(base) => Ok(base),
        CurveKind::Line(line) => Ok(line.base_mut()),
        CurveKind::Ellipse(ellipse) => Ok(ellipse.base_mut()),
        CurveKind::Hyperbola(hyperbola) => Ok(hyperbola.base_mut()),
        CurveKind::Parabola(parabola) => Ok(parabola.base_mut()),
        _ => Err(SolidError::null_pointer(OP)),
    }
}
/// `pSurvivingEdge->SetCurve(pCurve, TRUE)` (status ignored upstream): notified, the
/// old curve deleted with `delete_old`. Curves carry no identity in the port (the
/// module's Deviations), so the notification's new/old curve data are null.
pub(super) fn set_joined_edge_curve(
    k: &mut Kernel,
    b: &mut Brep,
    edge: EdgeId,
    curve: Option<Curve>,
    delete_old: bool,
    notifications: Option<&mut EditNotifyEnv<'_>>,
) -> Result<()> {
    with_notifications(
        b,
        notifications,
        Some(&mut k.context),
        |b, env, fallback| {
            let mut forwarded = crate::topology::edge::NotifyEnv {
                records: env.records,
                attributes: env.attributes,
                context: env.context.as_deref_mut().or(fallback),
                key_of: env.key_of,
            };
            let options = crate::topology::edge::SetCurveOptions {
                delete_old_curve: delete_old,
                ..crate::topology::edge::SetCurveOptions::default()
            };
            crate::error::ignore_status(crate::topology::edge::set_curve(
                b,
                edge,
                curve,
                options,
                &mut forwarded,
                [None, None],
            ))
        },
    )
}
/// `pCurveSurviveCopy->SetOwner(pSurvivingEdge)`: the owner is the edge's identity in
/// the notification environment. The curve's own `SM_NO_CHANGE_OWNER` notification
/// (`SmObject::Notify` on the curve) is not delivered: the curve has no identity to
/// notify under (the module's Deviations; as `topology.brep_p2`'s `SetCurve` helpers).
pub(super) fn set_edge_curve_owner(
    b: &mut Brep,
    edge: EdgeId,
    notifications: Option<&mut EditNotifyEnv<'_>>,
) -> Result<()> {
    let brep = ObjectRef {
        type_code: crate::geom::a_object::BREP_TYPE,
        key: b.key(),
    };
    with_notifications(b, notifications, None, |b, env, _| {
        let owner = (env.key_of)(crate::error::EntityRef::Edge(edge));
        let curve = b
            .edge_mut(edge)?
            .curve
            .as_mut()
            .ok_or_else(|| SolidError::null_pointer(OP))?;
        let _ = curve.set_owner(Some(owner), Some(brep));
        b.set_geometry_owner_key(crate::error::EntityRef::Edge(edge), Some(owner));
        Ok(())
    })
}

/// `Notify(SM_NO_MERGE_IN_BREP, pSurvivingEdge, pEdgeToDelete, this)`
/// ([`brep_p2::notify`], no B-rep cache held by the caller).
fn notify_edge_merge(
    k: &mut Kernel,
    b: &mut Brep,
    survivor: EdgeId,
    deleted: EdgeId,
    notifications: Option<&mut EditNotifyEnv<'_>>,
) -> Result<()> {
    let data = [
        Some(brep_p2::NotifyArg::Entity(crate::error::EntityRef::Edge(
            survivor,
        ))),
        Some(brep_p2::NotifyArg::Entity(crate::error::EntityRef::Edge(
            deleted,
        ))),
        Some(brep_p2::NotifyArg::Object(brep_p2::brep_ref(b))),
    ];
    with_notifications(b, notifications, Some(&mut k.context), |b, env, _| {
        brep_p2::notify(b, env, None, NotifyOperation::MergeInBrep, data)
    })
}
/// `SER(pEU->GetOrCreateUVTrimCurve(pNewCurve))` with the declaration's defaults:
/// no distance or deviation outputs, `bOKToMakeUVTrimCurve = TRUE`.
fn get_or_create_uv_trim_curve(
    k: &Kernel,
    b: &mut Brep,
    eu: crate::topology::EdgeuseId,
) -> Result<()> {
    crate::topology::edgeuse::p3::get_or_create_uv_trim_curve(k, b, eu, false, false, true)?.status
}

/// Finds the infinite region (part 9's `FindInfiniteRegion`, whose ray fires use
/// `query`: the B-rep's identities and cache), then makes it the infinite region and
/// sets the void flags as `void_setting` asks (part 1's `ResetInfiniteRegion`). A
/// face-use shell to test always swaps the flags (`SM_VS_INFINITEREGION_SWAP`), as
/// upstream.
///
/// # Errors
///
/// Those of both callees; [`SolidError::InvalidInput`] when no region is found
/// (upstream's `NER`).
pub fn find_and_set_infinite_region(
    k: &mut Kernel,
    b: &mut Brep,
    query: &BrepQuery<'_>,
    shell: Option<ShellId>,
    mut void_setting: SetRegionVoidType,
    bbox: Option<&Extent3d>,
) -> Result<()> {
    let region = crate::topology::brep::p9::find_infinite_region(k, b, query, shell, bbox)?
        .ok_or_else(|| SolidError::null_pointer(OP))?;
    if let Some(shell) = shell {
        if shell_is_faceuse_shell(b, shell)? {
            void_setting = SetRegionVoidType::InfiniteRegionSwap;
        }
    }
    crate::topology::brep::p1::reset_infinite_region(k, b, Some(region), void_setting)
}

fn shell_is_faceuse_shell(b: &Brep, shell: ShellId) -> Result<bool> {
    Ok(b.shell(shell)?.is_faceuse_shell())
}

/// Glues vertices, leaving the survivor in place. `wire_edges` is the optional
/// caller-supplied wire list, rather than an empty-list substitute for no list.
pub fn glue_vertices(
    k: &mut Kernel,
    b: &mut Brep,
    survivor: Option<VertexId>,
    deleted: Option<VertexId>,
    wire_edges: Option<&[EdgeId]>,
) -> Result<()> {
    glue_with_notifications(k, b, survivor, deleted, wire_edges, None)
}

/// Glues vertices with the caller's real notification records and attributes.
/// Equal identities, including two null identities, succeed before notification.
pub fn glue_with_notifications(
    k: &mut Kernel,
    b: &mut Brep,
    survivor: Option<VertexId>,
    deleted: Option<VertexId>,
    wire_edges: Option<&[EdgeId]>,
    notifications: Option<&mut EditNotifyEnv<'_>>,
) -> Result<()> {
    glue_with_notification_identity(k, b, survivor, deleted, wire_edges, notifications, None)
}

/// Glue vertices with the caller's issued body identity, without changing `Brep::key`.
/// Both the body caller and body-valued notification payloads use `body`.
///
/// # Errors
/// The same ordered topology and notification errors as [`glue_with_notifications`].
pub fn glue_with_body_identity(
    k: &mut Kernel,
    b: &mut Brep,
    survivor: Option<VertexId>,
    deleted: Option<VertexId>,
    wire_edges: Option<&[EdgeId]>,
    notifications: &mut EditNotifyEnv<'_>,
    body: ObjectRef,
) -> Result<()> {
    glue_with_notification_identity(
        k,
        b,
        survivor,
        deleted,
        wire_edges,
        Some(notifications),
        Some(body),
    )
}

fn glue_with_notification_identity(
    k: &mut Kernel,
    b: &mut Brep,
    survivor: Option<VertexId>,
    deleted: Option<VertexId>,
    wire_edges: Option<&[EdgeId]>,
    notifications: Option<&mut EditNotifyEnv<'_>>,
    body: Option<ObjectRef>,
) -> Result<()> {
    let Some(env) = notifications else {
        return plain_edit(k, b, |k, b, env| {
            glue_with_notification_identity(k, b, survivor, deleted, wire_edges, Some(env), body)
        });
    };
    let mut notifications = Some(env);
    if survivor == deleted {
        return Ok(());
    }
    let survivor = survivor.ok_or_else(|| SolidError::null_pointer(OP))?;
    let deleted = deleted.ok_or_else(|| SolidError::null_pointer(OP))?;
    crate::error::ignore_status(notify_pre_edit_with_body(
        k,
        b,
        notifications.as_deref_mut(),
        body,
    ))?;
    crate::error::ignore_status(notify_vertex_pre_edit_with_body(
        k,
        b,
        survivor,
        notifications.as_deref_mut(),
        body,
    ))?;
    crate::error::ignore_status(notify_merge_with_body(
        k,
        b,
        survivor,
        deleted,
        notifications.as_deref_mut(),
        body,
    ))?;
    for face in vertex::faces(b, deleted)? {
        crate::error::ignore_status(notify_face_pre_edit_with_body(
            k,
            b,
            face,
            notifications.as_deref_mut(),
            body,
        ))?;
    }
    let distance = b.vertex(survivor)?.point.distance(b.vertex(deleted)?.point);
    let mut pair = None;
    for first in vertex::shells(b, survivor)? {
        let region = b.shell(first)?.region;
        for last in vertex::shells(b, deleted)? {
            if region == b.shell(last)?.region {
                pair = Some((first, last));
                break;
            }
        }
    }
    let (first, last) = pair.ok_or_else(|| SolidError::null_pointer(OP))?;
    let mut first = Some(first);
    let radius = distance + b.vertex(deleted)?.tolerance;
    if radius > b.vertex(survivor)?.tolerance {
        crate::error::ignore_status(vertex::set_tolerance(b, survivor, radius, true, true))?;
    }
    if vertex::is_shell_vertex(b, deleted)? {
        delete_vertex(k, b, deleted, notifications)?;
        return Ok(());
    }
    if vertex::is_shell_vertex(b, survivor)? {
        let vu =
            vertex::primary_vertexuse(b, survivor)?.ok_or_else(|| SolidError::null_pointer(OP))?;
        let crate::topology::VertexuseOwner::Shell(shell) = b.vertexuse(vu)?.owner else {
            return Err(SolidError::sm_err(OP));
        };
        let shell = shell.ok_or_else(|| SolidError::null_pointer(OP))?;
        if shell_vertexuse(b, shell)? != Some(vu) {
            return Err(SolidError::sm_err(OP));
        }
        let region = b
            .shell(shell)?
            .region
            .ok_or_else(|| SolidError::null_pointer(OP))?;
        crate::error::ignore_status(remove_region_shell(b, region, shell))?;
        destroy_shell(k, b, notifications.as_deref_mut(), shell)?;
        first = None;
        remove_vertexuse(b, survivor, vu)?;
        destroy_topology_record(b, crate::error::EntityRef::Vertexuse(vu))?;
    }
    let wires = if let Some(given) = wire_edges {
        given.to_vec()
    } else {
        // GetWireEdges always returns SM_SUCCESS upstream. Errors here mark
        // missing callees or undefined pointer accesses, not an ignored native
        // status; do not invent an empty/partial wire list on those errors.
        shell_wire_edges(k, b, last)?
    };
    for vu in vertex::vertexuses(b, deleted)? {
        remove_vertexuse(b, deleted, vu)?;
        post_insert_vertexuse(b, survivor, vu)?;
    }
    remove_owned_vertex(b, deleted);
    destroy_topology_record(b, crate::error::EntityRef::Vertex(deleted))?;
    if let Some(first) = first.filter(|&first| first != last) {
        if let Some(vu) = shell_vertexuse(b, last)? {
            let vertex = b
                .vertexuse(vu)?
                .vertex
                .ok_or_else(|| SolidError::null_pointer(OP))?;
            crate::error::ignore_status(remove_vertexuse(b, vertex, vu))?;
            clear_shell_list(b, last)?;
            destroy_topology_record(b, crate::error::EntityRef::Vertexuse(vu))?;
        } else {
            transfer_shell_children(k, b, last, first, &wires)?;
        }
        let region = b
            .shell(last)?
            .region
            .ok_or_else(|| SolidError::null_pointer(OP))?;
        crate::error::ignore_status(remove_region_shell(b, region, last))?;
        destroy_shell(k, b, notifications, last)?;
    }
    Ok(())
}

/// `Notify(SM_NO_PRE_EDIT, this, NULL, NULL)` ([`brep_p2::notify`], no B-rep cache
/// held by the caller).
pub(super) fn notify_pre_edit(
    k: &mut Kernel,
    b: &mut Brep,
    notifications: Option<&mut EditNotifyEnv<'_>>,
) -> Result<()> {
    notify_pre_edit_with_body(k, b, notifications, None)
}

fn notify_pre_edit_with_body(
    k: &mut Kernel,
    b: &mut Brep,
    notifications: Option<&mut EditNotifyEnv<'_>>,
    body: Option<ObjectRef>,
) -> Result<()> {
    let data = [
        Some(brep_p2::NotifyArg::Object(
            body.unwrap_or_else(|| brep_p2::brep_ref(b)),
        )),
        None,
        None,
    ];
    with_notifications(b, notifications, Some(&mut k.context), |b, env, _| {
        brep_p2::notify_with_body_identity(
            b,
            env,
            None,
            NotifyOperation::PreEdit,
            data,
            body.unwrap_or_else(|| brep_p2::brep_ref(b)),
        )
    })
}
pub(super) fn notify_vertex_pre_edit(
    k: &mut Kernel,
    b: &mut Brep,
    vertex: VertexId,
    notifications: Option<&mut EditNotifyEnv<'_>>,
) -> Result<()> {
    notify_vertex_pre_edit_with_body(k, b, vertex, notifications, None)
}

fn notify_vertex_pre_edit_with_body(
    k: &mut Kernel,
    b: &mut Brep,
    vertex: VertexId,
    notifications: Option<&mut EditNotifyEnv<'_>>,
    body: Option<ObjectRef>,
) -> Result<()> {
    let brep = body.unwrap_or_else(|| brep_p2::brep_ref(b));
    with_notifications(
        b,
        notifications,
        Some(&mut k.context),
        |b, env, fallback| {
            let this = (env.key_of)(crate::error::EntityRef::Vertex(vertex));
            vertex::notify(
                b,
                vertex,
                env.records,
                env.attributes,
                env.context.as_deref_mut().or(fallback),
                NotifyOperation::PreEdit,
                this,
                [Some(this), Some(brep), None],
                &|vu| (env.key_of)(crate::error::EntityRef::Vertexuse(vu)),
            )
        },
    )
}
/// `pFace->Notify(SM_NO_PRE_EDIT, pFace, this, NULL)` ([`crate::topology::face::p3::notify`]);
/// no caller holds the surface's trimmed surface cache.
pub(super) fn notify_face_pre_edit(
    k: &mut Kernel,
    b: &mut Brep,
    face: FaceId,
    notifications: Option<&mut EditNotifyEnv<'_>>,
) -> Result<()> {
    notify_face_pre_edit_with_body(k, b, face, notifications, None)
}

fn notify_face_pre_edit_with_body(
    k: &mut Kernel,
    b: &mut Brep,
    face: FaceId,
    notifications: Option<&mut EditNotifyEnv<'_>>,
    body: Option<ObjectRef>,
) -> Result<()> {
    let brep = body.unwrap_or_else(|| brep_p2::brep_ref(b));
    with_notifications(
        b,
        notifications,
        Some(&mut k.context),
        |b, env, fallback| {
            let this = (env.key_of)(crate::error::EntityRef::Face(face));
            let mut forwarded = crate::topology::edge::NotifyEnv {
                records: env.records,
                attributes: env.attributes,
                context: env.context.as_deref_mut().or(fallback),
                key_of: env.key_of,
            };
            crate::topology::face::p3::notify(
                b,
                face,
                &mut forwarded,
                None,
                NotifyOperation::PreEdit,
                [Some(this), Some(brep), None],
            )
        },
    )
}
fn shell_vertexuse(b: &Brep, shell: ShellId) -> Result<Option<VertexuseId>> {
    let shell = b.shell(shell)?;
    Ok(if shell.is_vertex_shell() {
        match shell.members {
            crate::topology::shell::ShellMembers::Vertexuse(vu) => Some(vu),
            _ => None,
        }
    } else {
        None
    })
}
fn remove_region_shell(b: &mut Brep, region: RegionId, shell: ShellId) -> Result<()> {
    crate::topology::shell::region_remove_shell(b, region, shell)
}
/// `delete pShell` after detachment from its region ([`crate::topology::shell::destroy`]:
/// `~SmShell`'s `SM_NO_DESTRUCTION` and `~SmAObject`'s `ClearAttributes`) under the
/// edit's notification environment.
fn destroy_shell(
    k: &mut Kernel,
    b: &mut Brep,
    env: Option<&mut EditNotifyEnv<'_>>,
    shell: ShellId,
) -> Result<()> {
    let env = env.ok_or_else(|| SolidError::null_pointer(OP))?;
    crate::topology::shell::destroy(k, b, shell, env)
}

/// A plain `delete` after detachment from the topology graph: the record leaves its
/// arena (`topology.brep_p4`'s record removal, which keeps every other typed id
/// valid). It does not call `DeleteVertex` (that would repeat the notifications and
/// list removals) and deletes only the named record. Vertex and vertex use
/// destructors send no notification upstream; their `~SmAObject` attribute cleanup is
/// not modelled.
fn destroy_topology_record(b: &mut Brep, entity: crate::error::EntityRef) -> Result<()> {
    use crate::error::EntityRef as E;
    match entity {
        E::Vertexuse(id) => b.remove_vertexuse_record(id).map(drop),
        E::Vertex(id) => b.remove_vertex_record(id).map(drop),
        _ => Err(SolidError::invalid_input(
            OP,
            "unexpected record to destroy",
        )),
    }
}
fn clear_shell_list(b: &mut Brep, shell: ShellId) -> Result<()> {
    // GlueVertices clears only the head and size, retaining m_tShellType.
    b.shell_mut(shell)?.members = crate::topology::shell::ShellMembers::Empty;
    Ok(())
}
fn shell_wire_edges(k: &mut Kernel, b: &Brep, shell: ShellId) -> Result<Vec<EdgeId>> {
    crate::topology::shell::wire_edges(k, b, shell)
}
fn transfer_shell_children(
    k: &mut Kernel,
    b: &mut Brep,
    source: ShellId,
    target: ShellId,
    wire_edges: &[EdgeId],
) -> Result<()> {
    let wires: Vec<_> = wire_edges.iter().copied().map(Some).collect();
    crate::topology::shell::transfer_children(k, b, source, target, Some(&wires))
}
/// `m_pVertexListHead->Remove(pVertexToDelete)`.
fn remove_owned_vertex(b: &mut Brep, vertex: VertexId) {
    b.remove_from_vertex_list(vertex);
}

/// Outputs of `MakeEdgeInFace`, initialized to null after its editing and face checks.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct EdgeInFace {
    /// The inserted edge.
    pub edge: Option<EdgeId>,
    /// The new loop when insertion splits an existing loop.
    pub loop_: Option<LoopId>,
    /// The new face when insertion splits the face.
    pub face: Option<FaceId>,
}

/// Finds both vertex sectors then inserts an edge into a face. The native output
/// lifecycle is preserved: editing-disabled/null-face failures leave outputs alone;
/// a valid face clears all three before sector classification.
pub fn make_edge_in_face(
    k: &mut Kernel,
    b: &mut Brep,
    editing_enabled: bool,
    face: Option<FaceId>,
    start: Option<VertexId>,
    end: Option<VertexId>,
    curve: Curve,
    uv_curve: Option<BSplineCurve>,
    interval: Extent1d,
    orientation: OrientType,
    max_gap_3d: f64,
    output: &mut EdgeInFace,
    notifications: Option<&mut EditNotifyEnv<'_>>,
) -> Result<()> {
    make_edge_in_face_with_uv_curve(
        k,
        b,
        editing_enabled,
        face,
        start,
        end,
        curve,
        uv_curve.map(Curve::from),
        interval,
        orientation,
        max_gap_3d,
        output,
        notifications,
    )
}

/// [`make_edge_in_face`] accepting a polymorphic UV trim, including the native
/// `SmBSplineCurve`-derived line and conic kinds, without replacing their geometry.
/// Editing checks and output lifetime are identical to the original entry point.
pub fn make_edge_in_face_with_uv_curve(
    k: &mut Kernel,
    b: &mut Brep,
    editing_enabled: bool,
    face: Option<FaceId>,
    start: Option<VertexId>,
    end: Option<VertexId>,
    curve: Curve,
    uv_curve: Option<Curve>,
    interval: Extent1d,
    orientation: OrientType,
    max_gap_3d: f64,
    output: &mut EdgeInFace,
    notifications: Option<&mut EditNotifyEnv<'_>>,
) -> Result<()> {
    with_p3_notifications(k, b, notifications, |k, b, env| {
        make_edge_with_backend(
            &mut ArenaFaceEdge {
                k,
                b,
                env,
                curve_ref: &edge_curve_identity,
            },
            editing_enabled,
            face,
            start,
            end,
            curve,
            uv_curve,
            interval,
            orientation,
            max_gap_3d,
            output,
        )
    })
}

/// [`make_edge_in_face_with_uv_curve`] inside a caller's edit: notifying through the
/// caller's `env`, with the new edge's curve identified by the caller's `curve_ref`
/// (as [`super::p9::make_edge_in_face_with_vu`] takes it). The editing check reads
/// `b`'s own `m_bEditingEnabled` ([`Brep::editing_enabled`]), as upstream's member does.
///
/// # Errors
///
/// Those of [`make_edge_in_face_with_uv_curve`].
pub fn make_edge_in_face_in(
    k: &Kernel,
    b: &mut Brep,
    face: Option<FaceId>,
    start: Option<VertexId>,
    end: Option<VertexId>,
    curve: Curve,
    uv_curve: Option<Curve>,
    interval: Extent1d,
    orientation: OrientType,
    max_gap_3d: f64,
    output: &mut EdgeInFace,
    env: &mut EditNotifyEnv<'_>,
    curve_ref: &dyn Fn(EdgeId, i64) -> ObjectRef,
) -> Result<()> {
    let editing_enabled = b.editing_enabled();
    make_edge_with_backend(
        &mut ArenaFaceEdge {
            k,
            b,
            env,
            curve_ref,
        },
        editing_enabled,
        face,
        start,
        end,
        curve,
        uv_curve,
        interval,
        orientation,
        max_gap_3d,
        output,
    )
}

/// Handle adapter for the shared owned control flow. Missing sector/insertion
/// primitives remain calls to their actual owning units for either handle model.
pub(crate) trait FaceEdgeBackend {
    type Face: Copy;
    type Vertex: Copy;
    type Curve;
    type UvCurve;
    type Vertexuse: Copy;
    type Output: Default;
    fn validate_face(&self, face: Self::Face) -> Result<()>;
    fn tolerance(&self) -> Result<f64>;
    fn sector(
        &mut self,
        face: Self::Face,
        vertex: Option<Self::Vertex>,
        curve: &Self::Curve,
        uv: Option<&Self::UvCurve>,
        start: f64,
        end: f64,
        orientation: OrientType,
        tolerance: f64,
        all_sectors: bool,
    ) -> Result<Option<Self::Vertexuse>>;
    fn insert(
        &mut self,
        face: Self::Face,
        first: Self::Vertexuse,
        last: Self::Vertexuse,
        curve: Self::Curve,
        uv: Option<Self::UvCurve>,
        interval: Extent1d,
        orientation: OrientType,
        max_gap: f64,
        output: &mut Self::Output,
    ) -> Result<()>;
}

pub(crate) fn make_edge_with_backend<B: FaceEdgeBackend>(
    b: &mut B,
    editing_enabled: bool,
    face: Option<B::Face>,
    start: Option<B::Vertex>,
    end: Option<B::Vertex>,
    curve: B::Curve,
    uv_curve: Option<B::UvCurve>,
    interval: Extent1d,
    orientation: OrientType,
    max_gap_3d: f64,
    output: &mut B::Output,
) -> Result<()> {
    require_editing(editing_enabled)?;
    let f = face.ok_or_else(|| SolidError::null_pointer(OP))?;
    b.validate_face(f)?;
    *output = B::Output::default();
    let (t_start, t_end, end_orientation) = if orientation == OrientType::Opposite {
        (interval.max(), interval.min(), OrientType::Same)
    } else {
        (interval.min(), interval.max(), OrientType::Opposite)
    };
    let first = b
        .sector(
            f,
            start,
            &curve,
            uv_curve.as_ref(),
            t_start,
            t_end,
            orientation,
            b.tolerance()?,
            false,
        )?
        .ok_or_else(|| SolidError::null_pointer(OP))?;
    let last = b
        .sector(
            f,
            end,
            &curve,
            uv_curve.as_ref(),
            t_end,
            t_start,
            end_orientation,
            b.tolerance()?,
            false,
        )?
        .ok_or_else(|| SolidError::null_pointer(OP))?;
    b.insert(
        f,
        first,
        last,
        curve,
        uv_curve,
        interval,
        orientation,
        max_gap_3d,
        output,
    )
}

struct ArenaFaceEdge<'a, 'e> {
    k: &'a Kernel,
    b: &'a mut Brep,
    env: &'a mut EditNotifyEnv<'e>,
    curve_ref: &'a dyn Fn(EdgeId, i64) -> ObjectRef,
}
impl FaceEdgeBackend for ArenaFaceEdge<'_, '_> {
    type Face = FaceId;
    type Vertex = VertexId;
    type Curve = Curve;
    type UvCurve = Curve;
    type Vertexuse = VertexuseId;
    type Output = EdgeInFace;
    fn validate_face(&self, f: FaceId) -> Result<()> {
        self.b.face(f).map(|_| ())
    }
    fn tolerance(&self) -> Result<f64> {
        Ok(self.b.tolerance())
    }
    fn sector(
        &mut self,
        f: FaceId,
        v: Option<VertexId>,
        c: &Curve,
        uv: Option<&Curve>,
        start: f64,
        end: f64,
        orientation: OrientType,
        tolerance: f64,
        all: bool,
    ) -> Result<Option<VertexuseId>> {
        find_vertex_sector(
            self.k,
            self.b,
            f,
            v,
            c,
            uv,
            start,
            end,
            orientation,
            tolerance,
            all,
        )
    }
    fn insert(
        &mut self,
        f: FaceId,
        first: VertexuseId,
        last: VertexuseId,
        c: Curve,
        uv: Option<Curve>,
        interval: Extent1d,
        orientation: OrientType,
        gap: f64,
        out: &mut EdgeInFace,
    ) -> Result<()> {
        make_edge_in_face_with_vu(
            self.k,
            self.b,
            self.env,
            f,
            first,
            last,
            c,
            uv,
            interval,
            orientation,
            gap,
            out,
            self.curve_ref,
        )
    }
}

/// `SmFace::FindVertexSector(vertex, curve, uv, start, end, orientation, tolerance,
/// all_sectors, ...)` with upstream's default `bUVSpaceOkay = TRUE`
/// ([`face_p4::find_vertex_sector`]); only the sector's vertex use is read.
#[allow(clippy::too_many_arguments)]
pub(crate) fn find_vertex_sector(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
    v: Option<VertexId>,
    curve: &Curve,
    uv: Option<&Curve>,
    start: f64,
    end: f64,
    orientation: OrientType,
    tolerance: f64,
    all_sectors: bool,
) -> Result<Option<VertexuseId>> {
    // NER(cpVertex)
    let vertex = v.ok_or_else(|| SolidError::null_pointer(OP))?;
    let sector = face_p4::find_vertex_sector(
        k,
        b,
        f,
        face_p4::FindVertexSectorArgs {
            vertex,
            curve,
            uv_curve: uv,
            param_at_vertex: start,
            param_ivl_end: end,
            orientation,
            zone_tol_3d: tolerance,
            validate_sector: all_sectors,
            uv_space_okay: true,
        },
    )?;
    Ok(sector.vertexuse)
}

/// `SmBrep::MakeEdgeInFaceWithVU(face, first, last, curve, uv, interval, orientation,
/// max_gap_3d, ...)` without drop-curve state ([`super::p9::make_edge_in_face_with_vu`]).
/// The double gap becomes the `SmZoneTol3d` parameter. The new edge's curve takes its
/// identity from `curve_ref` (a call-local one for the plain entry points, module docs).
#[allow(clippy::too_many_arguments)]
fn make_edge_in_face_with_vu(
    k: &Kernel,
    b: &mut Brep,
    env: &mut EditNotifyEnv<'_>,
    f: FaceId,
    first: VertexuseId,
    last: VertexuseId,
    curve: Curve,
    uv: Option<Curve>,
    interval: Extent1d,
    orientation: OrientType,
    max_gap_3d: f64,
    output: &mut EdgeInFace,
    curve_ref: &dyn Fn(EdgeId, i64) -> ObjectRef,
) -> Result<()> {
    let made = super::p9::make_edge_in_face_with_vu(
        k,
        b,
        super::p9::EdgeInFaceInput {
            face: Some(f),
            vertices: [Some(first), Some(last)],
            curve: Some(curve),
            uv_trim: uv,
            drop_fail: None,
            interval,
            orientation,
            curve_zone_tol_3d: crate::geom::tol::zone_tol3d(k, max_gap_3d),
        },
        env,
        curve_ref,
    )?;
    *output = EdgeInFace {
        edge: Some(made.edge),
        loop_: made.loop_,
        face: made.face,
    };
    Ok(())
}

/// A call-local identity for an edge's curve: the type code above the edge index,
/// outside the arena identities' key space (bare indices), as `heal::stitch` does.
fn edge_curve_identity(edge: EdgeId, type_code: i64) -> ObjectRef {
    ObjectRef {
        type_code,
        key: crate::geom::attribute::ObjectKey(
            (type_code.unsigned_abs() << 32) | u64::from(edge.index()),
        ),
    }
}

/// Makes the selected regions manifold. `None` means all non-infinite regions;
/// `Some(&[])` means no regions and returns the native `SM_ERR`.
pub fn make_manifold(
    k: &mut Kernel,
    b: &mut Brep,
    regions_to_keep: Option<&[RegionId]>,
    keep_interior_faces: bool,
) -> Result<()> {
    plain_edit(k, b, |k, b, env| {
        make_manifold_in(k, b, regions_to_keep, keep_interior_faces, env)
    })
}

/// [`make_manifold`] under the edit's notification environment ([`plain_edit`]).
fn make_manifold_in(
    k: &mut Kernel,
    b: &mut Brep,
    regions_to_keep: Option<&[RegionId]>,
    keep_interior_faces: bool,
    env: &mut EditNotifyEnv<'_>,
) -> Result<()> {
    // The temporary editing change has no lasting effect on this early return.
    if regions_to_keep.is_some_and(<[RegionId]>::is_empty) {
        return Err(SolidError::sm_err(OP));
    }
    let previous = set_editing_enabled(b, true);
    let result = (|| {
        let mut regions = if let Some(given) = regions_to_keep {
            given.to_vec()
        } else {
            let mut all = brep_regions(b)?;
            let infinite = b.infinite_region();
            if let Some(index) = all.iter().position(|r| Some(*r) == infinite) {
                all.remove(index);
            }
            all
        };
        if regions.is_empty() {
            return Err(SolidError::sm_err(OP));
        }
        for face in brep_faces(b)? {
            let (first, last) = face_shells(b, face)?;
            let r1 = b
                .shell(first)?
                .region
                .ok_or_else(|| SolidError::null_pointer(OP))?;
            let r2 = b
                .shell(last)?
                .region
                .ok_or_else(|| SolidError::null_pointer(OP))?;
            let keep1 = regions.contains(&r1);
            let keep2 = regions.contains(&r2);
            if r1 != r2 && (keep1 != keep2 || (keep_interior_faces && keep1)) {
                continue;
            }
            delete_face(k, b, Some(&mut *env), face, false, true, None)?;
        }
        for edge in brep_edges(b)? {
            if crate::topology::edge::is_wire(b, edge)? {
                delete_edge_with(k, b, edge, None, None)?;
            }
        }
        for vertex in brep_vertices(b)? {
            if vertex::is_shell_vertex(b, vertex)? {
                delete_vertex(k, b, vertex, None)?;
            }
        }
        regions = brep_regions(b)?;
        for region in regions {
            let is_infinite = Some(region) == b.infinite_region();
            set_region_is_void(b, region, is_infinite)?;
        }
        Ok(())
    })();
    // Upstream restores in a destructor.
    set_editing_enabled(b, previous);
    result
}

/// Sets `m_bEditingEnabled`, returning the previous value.
fn set_editing_enabled(b: &mut Brep, enabled: bool) -> bool {
    let previous = b.editing_enabled();
    b.set_editing_enabled(enabled);
    previous
}

pub(super) fn brep_regions(b: &Brep) -> Result<Vec<RegionId>> {
    crate::topology::brep::p1::regions(b, None)
}
pub(super) fn brep_faces(b: &Brep) -> Result<Vec<FaceId>> {
    crate::topology::brep::p1::faces(b, None)
}
pub(super) fn brep_edges(b: &Brep) -> Result<Vec<EdgeId>> {
    crate::topology::brep::p1::edges(b, None)
}
pub(super) fn brep_vertices(b: &Brep) -> Result<Vec<VertexId>> {
    crate::topology::brep::p1::vertices(b, None)
}
fn face_shells(b: &Brep, face: FaceId) -> Result<(ShellId, ShellId)> {
    let (a, z) = crate::topology::face::p2::faceuses(b, face)?;
    let a = a.ok_or_else(|| SolidError::null_pointer(OP))?;
    let z = z.ok_or_else(|| SolidError::null_pointer(OP))?;
    Ok((
        b.faceuse(a)?
            .shell
            .ok_or_else(|| SolidError::null_pointer(OP))?,
        b.faceuse(z)?
            .shell
            .ok_or_else(|| SolidError::null_pointer(OP))?,
    ))
}
/// `SmBrep::DeleteFace(face, bDeleteEdgesVertices, bDoRegionNesting, NULL,
/// pOptRegionToKeep)` ([`crate::topology::brep::p3::delete_face`]), notifying as
/// [`with_p3_notifications`].
fn delete_face(
    k: &mut Kernel,
    b: &mut Brep,
    notifications: Option<&mut EditNotifyEnv<'_>>,
    face: FaceId,
    delete_edges_vertices: bool,
    nest: bool,
    keep_region: Option<RegionId>,
) -> Result<()> {
    use crate::topology::brep::p3 as brep_p3;
    let options = brep_p3::DeleteFaceOptions {
        delete_edges_vertices,
        do_region_nesting: nest,
        region_to_keep: keep_region,
    };
    with_p3_notifications(k, b, notifications, |k, b, env| {
        let mut marks = crate::topology::topology_traverser::Marks::default();
        brep_p3::delete_face(k, b, env, &mut marks, face, options).map(drop)
    })
}
fn set_region_is_void(b: &mut Brep, region: RegionId, is_void: bool) -> Result<()> {
    b.region_mut(region)?.set_is_void(i32::from(is_void));
    Ok(())
}

/// Removes redundant edges/vertices, preserving optional keep lists. The native
/// owning-list traversal is required even for apparently empty arena contents.
pub fn remove_topological_edges_and_vertices(
    k: &mut Kernel,
    b: &mut Brep,
    keep_edges: Option<&[EdgeId]>,
    keep_vertices: Option<&[VertexId]>,
) -> Result<()> {
    remove_topological_with_notifications(k, b, keep_edges, keep_vertices, None)
}

/// Removes topological entities using the actual edit-notification environment.
pub fn remove_topological_with_notifications(
    k: &mut Kernel,
    b: &mut Brep,
    keep_edges: Option<&[EdgeId]>,
    keep_vertices: Option<&[VertexId]>,
    notifications: Option<&mut EditNotifyEnv<'_>>,
) -> Result<()> {
    let Some(env) = notifications else {
        return plain_edit(k, b, |k, b, env| {
            remove_topological_with_notifications(k, b, keep_edges, keep_vertices, Some(env))
        });
    };
    let mut notifications = Some(env);
    let previous = set_editing_enabled(b, true);
    let result = (|| {
        for edge in brep_edges(b)? {
            // IsTopological can expand a surface even when this edge is on the
            // keep list. Do not move the keep-list test before this call.
            let Some(topological) =
                crate::error::ignored(crate::topology::edge::is_topological(k, b, edge, 1.0))?
            else {
                continue;
            };
            if !topological.is_topological {
                continue;
            }
            if keep_edges.is_some_and(|keep| keep.contains(&edge)) {
                continue;
            }
            let keep = topological
                .surf_to_keep
                .ok_or_else(|| SolidError::null_pointer(OP))?;
            let edgeuses = crate::topology::edge::edgeuses(b, edge, None, None)?;
            let first = *edgeuses.first().ok_or_else(|| SolidError::sm_err(OP))?;
            let third = *edgeuses.get(2).ok_or_else(|| SolidError::sm_err(OP))?;
            let face0 = crate::topology::edgeuse::p1::face(b, first)?
                .ok_or_else(|| SolidError::null_pointer(OP))?;
            let face1 = crate::topology::edgeuse::p1::face(b, third)?
                .ok_or_else(|| SolidError::null_pointer(OP))?;
            // A face owns one surface value. Equal owner metadata on two
            // values cannot establish pointer identity.
            let same_surface = face0 == face1;
            if !same_surface {
                let domain = b
                    .face(keep)?
                    .surface
                    .as_ref()
                    .ok_or_else(|| SolidError::null_pointer(OP))?
                    .natural_uv_domain_with_nurb(k)?;
                crate::topology::face::p2::set_uv_domain(b, keep, &domain)?;
                let tolerance = b.face(keep)?.tolerance;
                if topological.dist_between > tolerance / 10.0 {
                    set_face_tolerance(b, keep, tolerance + topological.dist_between, true, true)?;
                }
            }
            // Return the kept surface's current owner after the deletion. Its
            // former face id can become stale when DeleteEdge merges the faces.
            let current_face =
                delete_edge_keep_surface(k, b, edge, keep, notifications.as_deref_mut())?;
            let domain = topological.merged_face_domain.unwrap_or_default();
            crate::topology::face::p2::set_uv_domain(b, current_face, &domain)?;
        }
        for vertex in brep_vertices(b)? {
            if vertex::is_loop_vertex(b, vertex)? {
                delete_vertex(k, b, vertex, notifications.as_deref_mut())?;
                continue;
            }
            if crate::error::ignored(vertex::is_topological_vertex(k, b, vertex))?.unwrap_or(false)
            {
                if keep_vertices.is_some_and(|keep| keep.contains(&vertex)) {
                    continue;
                }
                crate::error::ignore_status(delete_topological_vertex_with_notifications(
                    k,
                    b,
                    true,
                    Some(vertex),
                    None,
                    notifications.as_deref_mut(),
                ))?;
            }
        }
        Ok(())
    })();
    // Upstream restores in a destructor.
    set_editing_enabled(b, previous);
    result
}

/// `DeleteEdge(pEdge, pSurfToKeep)` followed by `pSurfToKeep->GetFace()`: the face
/// holding the kept surface afterwards. A surface is named by its face here; when
/// `DeleteEdge` merges the faces and swaps the kept surface into the survivor, the
/// kept face's record is deleted and the survivor (the other face of the edge) holds
/// it; otherwise the kept face still does.
fn delete_edge_keep_surface(
    k: &mut Kernel,
    b: &mut Brep,
    edge: EdgeId,
    surface_face: FaceId,
    notifications: Option<&mut EditNotifyEnv<'_>>,
) -> Result<FaceId> {
    let mut faces = Vec::new();
    for eu in crate::topology::edge::edgeuses(b, edge, None, None)? {
        if let Some(f) = crate::topology::edgeuse::p1::face(b, eu)? {
            if f != surface_face && !faces.contains(&f) {
                faces.push(f);
            }
        }
    }
    delete_edge_with(k, b, edge, Some(surface_face), notifications)?;
    if b.face(surface_face).is_ok() {
        return Ok(surface_face);
    }
    faces
        .into_iter()
        .find(|&f| b.face(f).is_ok_and(|r| r.surface.is_some()))
        .ok_or_else(|| SolidError::null_pointer(OP))
}

/// Removes the requested faces then their wire edges and shell vertices.
/// Even an empty list advances and releases one unlocked context traversal mark.
/// Editing-disabled refusal occurs before the mark is acquired.
pub fn remove_faces(
    k: &mut Kernel,
    b: &mut Brep,
    editing_enabled: bool,
    faces: &[FaceId],
) -> Result<()> {
    plain_edit(k, b, |k, b, env| {
        remove_faces_in(k, b, editing_enabled, faces, env)
    })
}

/// [`remove_faces`] under the caller's notification environment `env` (the objects'
/// `SmTopology` records and attributes), instead of the kernel's (`plain_edit`).
///
/// # Errors
///
/// Those of [`remove_faces`].
pub fn remove_faces_notified(
    k: &mut Kernel,
    b: &mut Brep,
    editing_enabled: bool,
    faces: &[FaceId],
    env: &mut EditNotifyEnv<'_>,
) -> Result<()> {
    remove_faces_in(k, b, editing_enabled, faces, env)
}

/// [`remove_faces`] under the edit's notification environment ([`plain_edit`]).
fn remove_faces_in(
    k: &mut Kernel,
    b: &mut Brep,
    editing_enabled: bool,
    faces: &[FaceId],
    env: &mut EditNotifyEnv<'_>,
) -> Result<()> {
    require_editing(editing_enabled)?;
    let mut mark = MarkLock::new(core::slice::from_mut(&mut k.context), 0, ALL_MARKS)?;
    let result = (|| {
        let mut all_edges = Vec::new();
        let mut all_vertices = Vec::new();
        for &face in faces {
            b.face(face)?;
            for edge in face_edges(b, face)? {
                if !all_edges.contains(&edge) {
                    all_edges.push(edge);
                }
            }
            for vertex in face_vertices(b, face)? {
                if !all_vertices.contains(&vertex) {
                    all_vertices.push(vertex);
                }
            }
            delete_face(k, b, Some(&mut *env), face, false, true, None)?;
        }
        for edge in all_edges {
            if crate::topology::edge::is_wire(b, edge)? {
                delete_edge_with(k, b, edge, None, Some(&mut *env))?;
            }
        }
        for vertex in all_vertices {
            if vertex::is_shell_vertex(b, vertex)? {
                delete_vertex(k, b, vertex, Some(&mut *env))?;
            }
        }
        Ok(())
    })();
    // Upstream releases in a destructor; a release failure never hides `result`.
    let released = mark.release(core::slice::from_mut(&mut k.context));
    result.and(released)
}

/// `SmFace::GetEdges(rEdges)` (unit `topology.face_p4`; default arguments).
fn face_edges(b: &Brep, face: FaceId) -> Result<Vec<EdgeId>> {
    crate::topology::face::p4::edges(b, face, None, false, None)
}
/// `SmFace::GetVertices(rVertices)` (unit `topology.face_p4`; default arguments).
fn face_vertices(b: &Brep, face: FaceId) -> Result<Vec<VertexId>> {
    crate::topology::face::p4::vertices(b, face, None, false, None)
}

/// Stitching and orientation options with the native defaults.
#[derive(Debug, Clone, PartialEq)]
pub struct StitchAndOrientOptions {
    /// The raw stitching tolerance; nonpositive values select twice the B-rep tolerance.
    pub stitch_tolerance: f64,
    /// Allows formation of spine edges.
    pub allow_spine_edges: bool,
    /// Optional faces to swap UV after import.
    pub faces_to_swap: Option<Vec<FaceId>>,
    /// Overrides the lamina-count-based decision when present, including `Some(false)`.
    pub just_unify: Option<bool>,
}

impl Default for StitchAndOrientOptions {
    fn default() -> Self {
        Self {
            stitch_tolerance: -0.1,
            allow_spine_edges: false,
            faces_to_swap: None,
            just_unify: None,
        }
    }
}

/// Computes the first stitching tolerance exactly as `StitchAndOrient` does.
/// Only a nonpositive requested tolerance is capped at `100 * SM_ZONE_TOL_3D`.
pub fn initial_stitch_tolerance(brep_tolerance: f64, requested: f64) -> f64 {
    let local = 2.0 * brep_tolerance;
    if requested <= 0.0 {
        if local > 100.0 * ZONE_TOL_3D {
            100.0 * ZONE_TOL_3D
        } else {
            local
        }
    } else {
        requested
    }
}

/// Stitches twice if needed, then orients surfaces. No success is manufactured
/// from the statuses ignored upstream when a required callee is missing.
///
/// The stitch passes run outside an edit of their own: the stitch's glue and squeeze
/// helpers notify through their own kernel-context edits
/// ([`crate::topology::brep::p3::with_kernel_callbacks`]), which the guard refuses
/// inside another edit, and upstream's `SmBrep::StitchFaces` has no edit scope to
/// nest. Only the orientation pass needs the caller's notification environment.
pub fn stitch_and_orient(
    k: &mut Kernel,
    b: &mut Brep,
    options: &StitchAndOrientOptions,
) -> Result<()> {
    stitch_and_orient_with_identity(k, b, options, None)
}

pub(crate) fn stitch_and_orient_with_identity(
    k: &mut Kernel,
    b: &mut Brep,
    options: &StitchAndOrientOptions,
    identity: Option<crate::query::brep::BrepIdentity<'_>>,
) -> Result<()> {
    let mut lamina = 0;
    let mut tolerance = initial_stitch_tolerance(b.tolerance(), options.stitch_tolerance);
    crate::error::ignore_status(stitch_faces(
        k,
        b,
        tolerance,
        options.allow_spine_edges,
        &mut lamina,
        identity,
    ))?;
    if lamina > 1 {
        tolerance = if options.stitch_tolerance <= 0.0 {
            tolerance * 20.0
        } else {
            tolerance * 10.0
        };
        crate::error::ignore_status(stitch_faces(
            k,
            b,
            tolerance,
            options.allow_spine_edges,
            &mut lamina,
            identity,
        ))?;
    }
    plain_edit(k, b, |k, b, env| {
        crate::error::ignore_status(orient_trimmed_surfaces(
            k,
            b,
            Some(env),
            true,
            options.just_unify.unwrap_or(lamina > 0),
            options.faces_to_swap.as_deref(),
        ))?;
        Ok(())
    })
}

/// `StitchFaces(tol, lNumStitched, lNumLamina, dMaxVGap, dMaxEGap, NULL, bAllowSpine)`:
/// `lamina` receives the remaining lamina edge count; on failure it keeps its value.
fn stitch_faces(
    k: &mut Kernel,
    b: &mut Brep,
    tolerance: f64,
    allow_spine: bool,
    lamina: &mut usize,
    identity: Option<crate::query::brep::BrepIdentity<'_>>,
) -> Result<()> {
    let out = crate::topology::brep::p4::stitch_faces_with_identity(
        k,
        b,
        tolerance,
        None,
        allow_spine,
        identity,
    )?;
    *lamina = usize::try_from(out.lamina_edges)
        .map_err(|_| SolidError::invalid_input(OP, "lamina edge count"))?;
    Ok(())
}

/// `SmBrep::OrientTrimmedSurfaces(bNormalsOutward, bJustUnifyNormals, pFacesToSwapUV)`
/// ([`crate::topology::brep::p3::orient_trimmed_surfaces`]), notifying as
/// [`with_p3_notifications`].
fn orient_trimmed_surfaces(
    k: &mut Kernel,
    b: &mut Brep,
    notifications: Option<&mut EditNotifyEnv<'_>>,
    normals_outward: bool,
    just_unify: bool,
    faces_to_swap: Option<&[FaceId]>,
) -> Result<()> {
    with_p3_notifications(k, b, notifications, |k, b, env| {
        let mut marks = crate::topology::topology_traverser::Marks::default();
        crate::topology::brep::p3::orient_trimmed_surfaces(
            k,
            b,
            env,
            &mut marks,
            normals_outward,
            just_unify,
            faces_to_swap,
        )
        .map(drop)
    })
}

/// Output lists of `SqueezeFace`. All are reset even when editing is disabled.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SqueezedFace {
    /// Deleted vertices, as stale identities.
    pub deleted_vertices: Vec<VertexId>,
    /// New edges.
    pub new_edges: Vec<EdgeId>,
    /// Deleted edges, as stale identities.
    pub deleted_edges: Vec<EdgeId>,
}

/// Deletes a degenerate face and stitches its former boundary topology.
pub fn squeeze_face(
    k: &mut Kernel,
    b: &mut Brep,
    editing_enabled: bool,
    face: FaceId,
    stitch_tolerance: Option<f64>,
    output: &mut SqueezedFace,
) -> Result<()> {
    plain_edit(k, b, |k, b, env| {
        squeeze_face_in(k, b, editing_enabled, face, stitch_tolerance, output, env)
    })
}

/// [`squeeze_face`] under the edit's notification environment ([`plain_edit`]).
fn squeeze_face_in(
    k: &mut Kernel,
    b: &mut Brep,
    editing_enabled: bool,
    face: FaceId,
    stitch_tolerance: Option<f64>,
    output: &mut SqueezedFace,
    env: &mut EditNotifyEnv<'_>,
) -> Result<()> {
    *output = SqueezedFace::default();
    require_editing(editing_enabled)?;
    b.face(face)?;
    // `SmLockBrepBox`: unlocks when it goes out of scope, whatever the result
    let mut lock = crate::topology::brep::p1::LockBrepBox::new(k, b)?;
    let b = lock.brep();
    let result = (|| {
        let tolerance = stitch_tolerance.unwrap_or_else(|| {
            crate::geom::tol::xsect_tol_3d_from_zones(k, b.tolerance(), b.tolerance())
        });
        // DoRegionNesting, FastEdgeCompare, IgnoreProblems, ValidateResult,
        // SqueezeSmallEdges, MakingManifoldSolid, RemoveLaminarSlivers.
        // SplitEdgesWithVertices retains the manager's native default TRUE.
        let mut callback = crate::heal::stitch::p1::DefaultStitchCallback::new();
        let mut stitch =
            crate::heal::stitch::p1::Stitch::new(&mut callback, tolerance, false, false);
        stitch.fast_edge_compare = true;
        stitch.ignore_problems = true;
        stitch.squeeze_small_edges = true;
        stitch.making_manifold_solid = false;
        stitch.remove_laminar_slivers = false;
        let mut vertices: Vec<_> = face_vertices(b, face)?.into_iter().map(Some).collect();
        let mut edges: Vec<_> = face_edges(b, face)?.into_iter().map(Some).collect();
        let (shell1, shell2) = face_shells(b, face)?;
        let nest = (shell_size(b, shell1)? == 1 || shell_size(b, shell2)? == 1) && shell1 != shell2;
        let mut keep_region = None;
        if nest {
            let r1 = b
                .shell(shell1)?
                .region
                .ok_or_else(|| SolidError::null_pointer(OP))?;
            let r2 = b
                .shell(shell2)?
                .region
                .ok_or_else(|| SolidError::null_pointer(OP))?;
            let n1 = region_size(b, r1)?;
            let n2 = region_size(b, r2)?;
            let keep_first = if region_is_infinite(b, r1)? {
                true
            } else if region_is_infinite(b, r2)? {
                false
            } else if n1 > n2 {
                true
            } else if n2 > n1 {
                false
            } else {
                shell_size(b, shell1)? > shell_size(b, shell2)?
            };
            keep_region = Some(if keep_first { r1 } else { r2 });
        }
        delete_face(k, b, Some(&mut *env), face, false, true, keep_region)?;
        // After face deletion, ordinary stitch failures record SM_ERR but do
        // not suppress the remaining cleanup calls or their partially filled outputs.
        let mut failed = false;
        let vlist_opt = crate::error::ignored(stitch.stitch_vertex_list(k, b, &mut vertices))?;
        let (surviving, deleted_vertices, deleted_edges) = if let Some(vlist) = vlist_opt {
            (
                vlist.surviving.into_iter().map(Some).collect::<Vec<_>>(),
                vlist.deleted,
                vlist.deleted_edges,
            )
        } else {
            failed = true;
            (Vec::new(), Vec::new(), Vec::new())
        };
        edges.retain(|edge| !edge.is_some_and(|e| deleted_edges.contains(&e)));
        output.deleted_edges.extend_from_slice(&deleted_edges);
        vertices.clone_from(&surviving);

        let split_opt = crate::error::ignored(stitch.split_edges_with_vertices(
            k,
            b,
            &mut vertices,
            &mut edges,
        ))?;
        let mut new_edges = if let Some(res) = split_opt {
            res.new_edges
        } else {
            failed = true;
            Vec::new()
        };

        let edge_opt = crate::error::ignored(stitch.stitch_edges_of_vertices(k, b, &surviving))?;
        let mut deleted_edges = if let Some(res) = edge_opt {
            res.deleted
        } else {
            failed = true;
            Vec::new()
        };

        let temporary: Vec<_> = new_edges
            .iter()
            .copied()
            .filter(|edge| deleted_edges.contains(edge))
            .collect();
        new_edges.retain(|edge| !temporary.contains(edge));
        deleted_edges.retain(|edge| !temporary.contains(edge));
        output.deleted_vertices.extend(deleted_vertices);
        output.deleted_edges.extend(deleted_edges);
        output.new_edges.extend(new_edges);
        if failed {
            Err(SolidError::sm_err(OP))
        } else {
            Ok(())
        }
    })();
    result
}

fn shell_size(b: &Brep, shell: ShellId) -> Result<usize> {
    crate::topology::shell::shell_list_size(b, shell)
}
fn region_size(b: &Brep, region: RegionId) -> Result<usize> {
    Ok(b.region(region)?.shells.len())
}
fn region_is_infinite(b: &Brep, region: RegionId) -> Result<bool> {
    Ok(b.region(region)?.is_infinite_region(b, region))
}

#[cfg(test)]
fn cleanup_status(result: Result<()>, failed: &mut bool) -> Result<()> {
    // After deleting the face, every native stitch failure sets SM_ERR but
    // continues cleanup and output appends. Only missing implementation defers.
    if crate::error::ignored(result)?.is_none() {
        *failed = true;
    }
    Ok(())
}

/// Scales about the current origin then applies the placement. The identity exit
/// precedes notifications, geometry conversion and tolerance scaling, even with
/// `scale_tolerances = true`. Nonidentity transformation requires part 2's Notify
/// before any point or geometry is changed; those continuations are uncertified.
pub fn transform(
    k: &mut Kernel,
    b: &mut Brep,
    placement: &Axis2Placement,
    scale: Option<Vec3>,
    scale_tolerances: bool,
) -> Result<()> {
    transform_with_notifications(k, b, placement, scale, scale_tolerances, None)
}

/// Transforms a B-rep using the actual topology notification environment.
pub fn transform_with_notifications(
    k: &mut Kernel,
    b: &mut Brep,
    placement: &Axis2Placement,
    scale: Option<Vec3>,
    scale_tolerances: bool,
    notifications: Option<&mut EditNotifyEnv<'_>>,
) -> Result<()> {
    if let Some(env) = notifications {
        transform_in(k, b, placement, scale, scale_tolerances, env)
    } else {
        plain_edit(k, b, |k, b, env| {
            transform_in(k, b, placement, scale, scale_tolerances, env)
        })
    }
}

fn transform_in(
    k: &mut Kernel,
    b: &mut Brep,
    placement: &Axis2Placement,
    scale: Option<Vec3>,
    scale_tolerances: bool,
    env: &mut EditNotifyEnv<'_>,
) -> Result<()> {
    let owner_keys = env.key_of;
    let mut notifications = Some(env);
    if placement.is_identity(crate::math::EFF_ZERO)
        && scale.is_none_or(|s| s.approx_eq(Vec3::new(1.0, 1.0, 1.0)))
    {
        return Ok(());
    }
    crate::error::ignore_status(notify_pre_edit(k, b, notifications.as_deref_mut()))?;
    if scale.is_some_and(|s| {
        !crate::geom::tol::are_same(s.x, s.y)
            || !crate::geom::tol::are_same(s.x, s.z)
            || s.x < crate::math::EFF_ZERO
    }) {
        crate::error::ignore_status(turn_to_nurbs(k, b, notifications.as_deref_mut()))?;
    }
    let mut bbox = Extent3d::new();
    let vertices = brep_vertices(b)?;
    for &vertex in &vertices {
        let mut point = b.vertex(vertex)?.point;
        crate::error::ignore_status(notify_vertex_pre_edit(
            k,
            b,
            vertex,
            notifications.as_deref_mut(),
        ))?;
        if let Some(scale) = scale {
            point.x *= scale.x;
            point.y *= scale.y;
            point.z *= scale.z;
        }
        point = placement.transform_point(point);
        set_vertex_point(k, b, vertex, point, notifications.as_deref_mut())?;
        bbox.add_point(point);
    }
    for edge in crate::topology::brep::p1::curve_owners(b, None, &|e| {
        transform_owner_key(
            b,
            crate::EntityRef::Edge(e),
            owner_keys(crate::EntityRef::Edge(e)),
        )
    })? {
        crate::error::ignore_status(notify_edge_pre_edit(
            k,
            b,
            edge,
            notifications.as_deref_mut(),
        ))?;
        let curve = b
            .edge_mut(edge)?
            .curve
            .as_mut()
            .ok_or_else(|| SolidError::null_pointer(OP))?;
        curve.transform(k, placement, scale)?;
    }
    let mut rectangular: Vec<_> = brep_faces(b)?.into_iter().map(Some).collect();
    for face in &mut rectangular {
        if let Some(id) = *face {
            if crate::topology::face::p2::rectangular_trim(b, id)? != 1 {
                *face = None;
            }
        }
    }
    for face in crate::topology::brep::p1::surface_owners(b, None, &|f| {
        transform_owner_key(
            b,
            crate::EntityRef::Face(f),
            owner_keys(crate::EntityRef::Face(f)),
        )
    })? {
        crate::error::ignore_status(notify_face_pre_edit(
            k,
            b,
            face,
            notifications.as_deref_mut(),
        ))?;
        let transformed = {
            let surface = b
                .face_mut(face)?
                .surface
                .as_mut()
                .ok_or_else(|| SolidError::null_pointer(OP))?;
            crate::error::ignored(surface.transform(k, placement, scale))?.is_some()
        };
        if !transformed {
            let surface = b
                .face_mut(face)?
                .surface
                .as_mut()
                .ok_or_else(|| SolidError::null_pointer(OP))?;
            let base =
                transformed_surface_base(k, surface)?.ok_or_else(|| SolidError::sm_err(OP))?;
            replace_surface(
                k,
                b,
                face,
                crate::surface::surface::Surface::from(base),
                false,
                None,
                true,
                notifications.as_deref_mut(),
            )?;
        }
    }
    for face in rectangular.into_iter().flatten() {
        b.face_mut(face)?.rectangular_trim = 1;
    }
    if scale.is_some_and(|s| (s.x < 0.0) ^ (s.y < 0.0) ^ (s.z < 0.0)) {
        for edge in brep_edges(b)? {
            reverse_reflection_edgeuses(b, edge)?;
        }
        for face in brep_faces(b)? {
            swap_reflection_shells(b, face)?;
        }
    }
    if scale.is_some() && scale_tolerances {
        let size = bbox.size().length();
        let zone = b.set_this_model_size_estimate(k, size);
        crate::geom::tol::set_context_model_size_estimate(&mut k.context, size);
        for vertex in vertices {
            crate::error::ignore_status(vertex::set_tolerance(b, vertex, zone, false, false))?;
        }
        for edge in brep_edges(b)? {
            crate::error::ignore_status(crate::topology::edge::set_tolerance(
                b, edge, zone, false, false,
            ))?;
        }
        for face in brep_faces(b)? {
            set_face_tolerance(b, face, zone, false, false)?;
        }
    }
    Ok(())
}

/// `SmBrep::TurnToNURBS()` as a plain entry point (the caller keeps no notification
/// state of its own: `plain_edit`), with the geometry owners [`transform`]'s
/// conversion uses.
///
/// # Errors
///
/// Those of [`crate::topology::brep::p3::turn_to_nurbs`]; `InvalidInput` for a call
/// inside another lent edit (before anything changes).
pub fn turn_to_nurbs_plain(k: &mut Kernel, b: &mut Brep) -> Result<()> {
    turn_to_nurbs(k, b, None)
}

/// `SmBrep::TurnToNURBS()` ([`crate::topology::brep::p3::turn_to_nurbs`]), notifying as
/// [`with_p3_notifications`].
fn turn_to_nurbs(
    k: &mut Kernel,
    b: &mut Brep,
    notifications: Option<&mut EditNotifyEnv<'_>>,
) -> Result<()> {
    with_p3_notifications(k, b, notifications, |k, b, env| {
        let key_of = env.key_of;
        let mut selection = std::collections::BTreeMap::new();
        for entity in brep_faces(b)?
            .into_iter()
            .map(crate::EntityRef::Face)
            .chain(brep_edges(b)?.into_iter().map(crate::EntityRef::Edge))
        {
            selection.insert(entity, transform_owner_key(b, entity, key_of(entity)));
        }
        crate::topology::brep::p3::turn_to_nurbs_with_geometry_owners(k, b, env, &|entity| {
            selection
                .get(&entity)
                .copied()
                .unwrap_or_else(|| key_of(entity))
        })
    })
}
pub(super) fn notify_edge_pre_edit(
    k: &mut Kernel,
    b: &mut Brep,
    edge: EdgeId,
    notifications: Option<&mut EditNotifyEnv<'_>>,
) -> Result<()> {
    let brep = ObjectRef {
        type_code: crate::geom::a_object::BREP_TYPE,
        key: b.key(),
    };
    with_notifications(
        b,
        notifications,
        Some(&mut k.context),
        |b, env, fallback| {
            let this = (env.key_of)(crate::error::EntityRef::Edge(edge));
            let mut forwarded = crate::topology::edge::NotifyEnv {
                records: env.records,
                attributes: env.attributes,
                context: env.context.as_deref_mut().or(fallback),
                key_of: env.key_of,
            };
            crate::topology::edge::notify(
                b,
                edge,
                &mut forwarded,
                NotifyOperation::PreEdit,
                [Some(this), Some(brep), None],
            )
        },
    )
}

// Geometry may retain a constructor's caller keys while plain-edit or temporary
// transform notifications use distinct identities. Match only a proven producer
// key or the issued notification identity; foreign and unowned geometry keep the
// native filter.
pub(super) fn transform_owner_key(
    b: &Brep,
    entity: crate::EntityRef,
    kernel_key: ObjectRef,
) -> ObjectRef {
    let owner = match entity {
        crate::EntityRef::Edge(edge) => b
            .edges()
            .get(edge)
            .and_then(|e| e.curve.as_ref())
            .and_then(Curve::owner),
        crate::EntityRef::Face(face) => b
            .faces()
            .get(face)
            .and_then(|f| f.surface.as_ref())
            .and_then(crate::surface::surface::Surface::owner),
        _ => None,
    };
    if owner == Some(kernel_key) {
        kernel_key
    } else {
        b.geometry_owner_key(entity).unwrap_or(kernel_key)
    }
}

fn transformed_surface_base(
    k: &Kernel,
    surface: &mut crate::surface::surface::Surface,
) -> Result<Option<crate::nurbs::b_spline_surface::BSplineSurface>> {
    use crate::surface::surface::SurfaceKind;
    Ok(match surface.kind_mut() {
        SurfaceKind::BSpline(base) => Some(base.clone()),
        SurfaceKind::Plane(plane) => {
            if plane.base().gw_nurb().is_none() {
                crate::error::ignore_status(plane.make_nurb())?;
            }
            Some(plane.base().clone())
        }
        SurfaceKind::SurfOfRevolution(revolution) => {
            if revolution.base().gw_nurb().is_none() {
                crate::error::ignore_status(revolution.make_nurb(k))?;
            }
            Some(revolution.base().clone())
        }
        SurfaceKind::Cone(cone) => {
            if cone.rev().base().gw_nurb().is_none() {
                crate::error::ignore_status(cone.make_nurb(k))?;
            }
            Some(cone.rev().base().clone())
        }
        SurfaceKind::Sphere(sphere) => {
            if sphere.base().base().gw_nurb().is_none() {
                crate::error::ignore_status(sphere.make_nurb(k))?;
            }
            Some(sphere.base().base().clone())
        }
        SurfaceKind::Torus(torus) => {
            if torus.base().base().gw_nurb().is_none() {
                crate::error::ignore_status(torus.make_nurb(k))?;
            }
            Some(torus.base().base().clone())
        }
        SurfaceKind::SurfOfExtrusion(extrusion) => {
            if extrusion.base().gw_nurb().is_none() {
                crate::error::ignore_status(extrusion.make_nurb(k))?;
            }
            Some(extrusion.base().clone())
        }
        SurfaceKind::CurveBounded(_)
        | SurfaceKind::Step(_)
        | SurfaceKind::OffsetSurface(_)
        | SurfaceKind::SrfInVolume(_) => None,
    })
}
/// `SER(ReplaceSurface(pFace, pNewSurf, bAddToDeletedList, pOptFacesModified,
/// bCreateUVTrimCurves))` ([`brep_p2::replace_surface`]); the face is notified under
/// the caller's identity for it.
#[allow(clippy::too_many_arguments)] // one upstream call
pub(super) fn replace_surface(
    k: &Kernel,
    b: &mut Brep,
    face: FaceId,
    surface: crate::surface::surface::Surface,
    add_to_deleted: bool,
    mut modified: Option<&mut Vec<FaceId>>,
    create_uv_trims: bool,
    notifications: Option<&mut EditNotifyEnv<'_>>,
) -> Result<()> {
    // init output
    if let Some(modified) = modified.as_deref_mut() {
        modified.clear();
    }
    let old = if add_to_deleted {
        brep_p2::OldSurface::AddToDeletedList
    } else {
        brep_p2::OldSurface::Delete
    };
    // The kernel is held immutably here (`SmSurface` evaluation), so an arena-only
    // caller's notifications reach no context callbacks.
    let (_, faces) = with_notifications(b, notifications, None, |b, env, _| {
        let face_ref = (env.key_of)(crate::error::EntityRef::Face(face));
        brep_p2::replace_surface(k, b, env, face, surface, face_ref, old, create_uv_trims)
    })?;
    if let Some(modified) = modified {
        modified.extend(faces);
    }
    Ok(())
}
fn reverse_reflection_edgeuses(b: &mut Brep, edge: EdgeId) -> Result<()> {
    crate::topology::brep::p1::reverse_reflection_edgeuses(b, edge)
}
fn swap_reflection_shells(b: &mut Brep, face: FaceId) -> Result<()> {
    crate::topology::brep::p1::swap_reflection_shells(b, face)
}
fn set_face_tolerance(
    b: &mut Brep,
    face: FaceId,
    tolerance: f64,
    only_larger: bool,
    cascade: bool,
) -> Result<()> {
    crate::topology::face::p3::set_tolerance(b, face, tolerance, only_larger, cascade)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topology::Vertex;

    #[test]
    fn supplied_uv_reaches_both_sectors_and_insertion() {
        struct Backend {
            sectors: std::cell::RefCell<Vec<(f64, f64, OrientType)>>,
        }
        impl FaceEdgeBackend for Backend {
            type Face = ();
            type Vertex = ();
            type Curve = ();
            type UvCurve = String;
            type Vertexuse = ();
            type Output = Option<String>;
            fn validate_face(&self, (): ()) -> Result<()> {
                Ok(())
            }
            fn tolerance(&self) -> Result<f64> {
                Ok(1e-6)
            }
            fn sector(
                &mut self,
                (): (),
                _vertex: Option<()>,
                (): &(),
                uv: Option<&String>,
                start: f64,
                end: f64,
                orientation: OrientType,
                tolerance: f64,
                all: bool,
            ) -> Result<Option<()>> {
                assert_eq!(uv.map(String::as_str), Some("trace-489"));
                assert_eq!(tolerance, 1e-6);
                assert!(!all);
                self.sectors.borrow_mut().push((start, end, orientation));
                Ok(Some(()))
            }
            fn insert(
                &mut self,
                (): (),
                (): (),
                (): (),
                (): (),
                uv: Option<String>,
                interval: Extent1d,
                _orientation: OrientType,
                max_gap: f64,
                output: &mut Self::Output,
            ) -> Result<()> {
                assert_eq!(interval, Extent1d::from_min_max(-3.0, 7.0));
                assert_eq!(max_gap, 2e-7);
                *output = uv;
                Ok(())
            }
        }
        for orientation in [OrientType::Same, OrientType::Opposite] {
            let mut backend = Backend {
                sectors: std::cell::RefCell::new(Vec::new()),
            };
            let mut output = None;
            make_edge_with_backend(
                &mut backend,
                true,
                Some(()),
                Some(()),
                Some(()),
                (),
                Some(String::from("trace-489")),
                Extent1d::from_min_max(-3.0, 7.0),
                orientation,
                2e-7,
                &mut output,
            )
            .unwrap();
            assert_eq!(output.as_deref(), Some("trace-489"));
            let expected = if orientation == OrientType::Same {
                vec![
                    (-3.0, 7.0, OrientType::Same),
                    (7.0, -3.0, OrientType::Opposite),
                ]
            } else {
                vec![
                    (7.0, -3.0, OrientType::Opposite),
                    (-3.0, 7.0, OrientType::Same),
                ]
            };
            assert_eq!(*backend.sectors.borrow(), expected);
        }
    }

    #[test]
    fn squeeze_cleanup_continues_after_typed_native_failures() {
        let errors = [
            SolidError::sm_err("SmStitch::StitchVertexPairs"),
            SolidError::NotFound {
                operation: "SmStitch::SplitEdgesWithVertices",
                entity: crate::error::EntityRef::Edge(EdgeId::from_index(2)),
            },
            SolidError::Degenerate {
                operation: "SmStitch::StitchEdgesOfVertices",
                entity: None,
            },
        ];
        let mut failed = false;
        for error in errors {
            cleanup_status(Err(error), &mut failed).unwrap();
            assert!(failed);
        }
        cleanup_status(Ok(()), &mut failed).unwrap();
        assert!(failed);

        let mut failed = false;
        let missing = SolidError::unported("SmStitch::StitchVertexPairs", "heal.stitch_p2");
        assert_eq!(
            cleanup_status(Err(missing.clone()), &mut failed),
            Err(missing)
        );
        assert!(!failed);
    }

    #[test]
    fn failure_outputs_follow_native_lifecycle() {
        let mut b = Brep::new();
        let v = b.add_vertex_record(Vertex::default()).unwrap();
        let mut edge = Some(EdgeId::from_index(3));
        // With `SmBrep::DeleteVertex` (topology.brep_p4) and `Notify` ported, the
        // isolated-vertex branch completes (upstream ignores both statuses); the
        // deleted-edge output is still reset first.
        assert!(
            combine_coincident_vertices(&mut Kernel::default(), &mut b, v, v, &mut edge).is_ok()
        );
        assert!(edge.is_none());
        let mut out = SqueezedFace {
            deleted_vertices: vec![v],
            new_edges: vec![EdgeId::from_index(1)],
            deleted_edges: vec![EdgeId::from_index(2)],
        };
        assert!(squeeze_face(
            &mut Kernel::default(),
            &mut b,
            false,
            FaceId::from_index(0),
            None,
            &mut out
        )
        .is_err());
        assert_eq!(out, SqueezedFace::default());
    }

    #[test]
    fn identity_precedes_tolerance_update_and_notifications() {
        let mut b = Brep::new();
        let mut k = Kernel::default();
        let v = b
            .add_vertex_record(Vertex {
                point: Vec3::new(1., 2., 3.),
                tolerance: 42.,
                ..Vertex::default()
            })
            .unwrap();
        let original = b.vertex(v).unwrap().clone();
        transform(&mut k, &mut b, &Axis2Placement::IDENTITY, None, true).unwrap();
        assert_eq!(b.vertex(v).unwrap(), &original);
        assert_eq!(b.vertices().len(), 1);
        assert!(remove_faces(&mut k, &mut b, true, &[]).is_ok());
        assert!(remove_faces(&mut k, &mut b, false, &[]).is_err());
    }

    #[test]
    fn empty_face_removal_preserves_an_existing_mark_lock() {
        use crate::geom::context::MarkType;
        let mut k = Kernel::default();
        let mut b = Brep::new();
        let mut outer = MarkLock::new(core::slice::from_mut(&mut k.context), 0, ALL_MARKS).unwrap();
        let held = outer.mark_type();
        let marks = [
            MarkType::Mark,
            MarkType::Mark2,
            MarkType::Mark3,
            MarkType::MarkIo,
            MarkType::MarkAssert,
        ];
        let before = marks.map(|m| k.context.current_mark(m));
        remove_faces(&mut k, &mut b, true, &[]).unwrap();
        assert!(k.context.is_mark_locked(held));
        let advanced: Vec<_> = marks
            .into_iter()
            .zip(before)
            .filter(|(m, n)| k.context.current_mark(*m) != *n)
            .collect();
        assert_eq!(advanced.len(), 1);
        assert_ne!(advanced[0].0, held);
        assert_eq!(k.context.current_mark(advanced[0].0), advanced[0].1 + 2);
        assert!(!k.context.is_mark_locked(advanced[0].0));
        let before_refusal = marks.map(|m| k.context.current_mark(m));
        assert!(remove_faces(&mut k, &mut b, false, &[]).is_err());
        assert_eq!(marks.map(|m| k.context.current_mark(m)), before_refusal);
        outer
            .release(core::slice::from_mut(&mut k.context))
            .unwrap();
        assert!(!k.context.is_mark_locked(held));
    }
    #[test]
    fn pending_failure_detaches_borrowed_dependent_attributes() {
        use crate::geom::attribute::{Attribute, AttributeStore, Behavior, ObjectKey};
        let mut store = AttributeStore::new();
        let key = ObjectKey(91);
        let attribute = store
            .insert(Attribute::long(10000, 12, Behavior::Copy))
            .unwrap();
        store.add_attribute(key, attribute).unwrap();
        {
            let _pending = PendingTopology::new(Some((key, 7_u32)), &mut store);
        }
        assert!(store.attributes(key).is_empty());
        // A normal clear would delete this dependent attribute; pending cleanup
        // must retain the caller's allocation so a reader can retry it elsewhere.
        assert!(store.get(attribute).is_some());
        store.add_attribute(ObjectKey(92), attribute).unwrap();
    }

    #[test]
    fn pending_release_preserves_attributes_and_transfers_object() {
        use crate::geom::attribute::{Attribute, AttributeStore, Behavior, ObjectKey};
        let mut store = AttributeStore::new();
        let key = ObjectKey(91);
        let attribute = store
            .insert(Attribute::long(10000, 12, Behavior::Copy))
            .unwrap();
        store.add_attribute(key, attribute).unwrap();
        let mut pending = PendingTopology::new(Some((key, 7_u32)), &mut store);
        *pending.object_mut().unwrap() = 8;
        assert_eq!(pending.release(), Some((key, 8)));
        assert_eq!(store.attributes(key), vec![attribute]);
    }

    #[test]
    fn issued_body_glue_notifies_real_caller_and_keeps_failure_prefix() {
        use crate::geom::attribute::{ids, Attribute, AttributeStore, Behavior};
        use crate::topology::edge::notification_test_support as callbacks;

        for separate_region in [false, true] {
            let mut k = Kernel::default();
            let mut b = crate::primitive::api_primitives::create_box(
                &mut k,
                Point3::ZERO,
                10.0,
                20.0,
                30.0,
            )
            .unwrap();
            let original_key = b.key();
            let serial = b.notify_serial().unwrap();
            let key_of = |entity| crate::topology::brep::p3::topology_ref(serial, entity);
            let body = key_of(crate::EntityRef::Brep(crate::topology::BrepId::from_index(
                0,
            )));
            assert_ne!(body.key, original_key);
            let survivor = b.vertex_list()[0];
            let deleted = if separate_region {
                let region = b
                    .add_region_record(crate::topology::Region::default())
                    .unwrap();
                let mut setup_records = crate::topology::topology::NotifyRecords::new();
                crate::topology::brep::p3::seed_records(&mut setup_records, &b, &key_of);
                let mut setup_attributes = AttributeStore::default();
                crate::topology::brep::p9::make_shell_vertex_with_kernel_context(
                    &mut k,
                    &mut b,
                    Some(region),
                    Point3::new(40.0, 0.0, 0.0),
                    &mut crate::topology::edge::NotificationStores {
                        records: &mut setup_records,
                        attributes: &mut setup_attributes,
                        key_of: &key_of,
                    },
                )
                .unwrap()
                .1
            } else {
                b.vertex_list()[1]
            };
            let survivor_ref = key_of(crate::EntityRef::Vertex(survivor));
            let deleted_ref = key_of(crate::EntityRef::Vertex(deleted));
            let mut records = crate::topology::topology::NotifyRecords::new();
            crate::topology::brep::p3::seed_records(&mut records, &b, &key_of);
            let mut attributes = AttributeStore::default();
            let marker = attributes
                .insert(Attribute::long(ids::USER_1, 917, Behavior::Reference))
                .unwrap();
            attributes.add_attribute(body.key, marker).unwrap();
            let events = callbacks::install(&mut k.context);
            let result =
                crate::geom::context::Context::with_notification_context(&mut k, |k, context| {
                    let mut env = EditNotifyEnv {
                        records: &mut records,
                        attributes: &mut attributes,
                        context: Some(context),
                        key_of: &key_of,
                    };
                    glue_with_body_identity(
                        k,
                        &mut b,
                        Some(survivor),
                        Some(deleted),
                        None,
                        &mut env,
                        body,
                    )
                })
                .unwrap();
            callbacks::assert_installed(&k.context, &events);
            assert_eq!(b.key(), original_key);
            assert_eq!(attributes.users(marker).unwrap(), &[body.key]);
            assert_eq!(attributes.get(marker).unwrap().long_value(), Some(917));
            let events = events.lock().unwrap();
            assert_eq!(events[0].caller, body);
            assert_eq!(events[0].operation, NotifyOperation::PreEdit);
            assert_eq!(events[0].data, [Some(body), None, None]);
            assert_eq!((events[0].callback, events[1].callback), ("user", "system"));
            if separate_region {
                assert!(result.is_err());
                assert_eq!(events.len(), 6);
                assert!(b.vertex_list().contains(&deleted));
            } else {
                result.unwrap();
                assert!(!b.vertex_list().contains(&deleted));
                assert!(events.iter().any(|event| event.caller == survivor_ref
                    && event.operation == NotifyOperation::PreEdit
                    && event.data == [Some(survivor_ref), Some(body), None]));
                assert!(events.iter().any(|event| event.caller == body
                    && event.operation == NotifyOperation::MergeInBrep
                    && event.data == [Some(survivor_ref), Some(deleted_ref), Some(body)]));
            }
            assert!(!events.iter().any(|event| event.caller.key == original_key));
        }
    }

    #[test]
    fn glue_alias_precedes_arena_access_and_notifications() {
        let mut k = Kernel::default();
        let mut b = Brep::new();
        // Even a stale equal handle is only compared, never dereferenced natively.
        let stale = Some(VertexId::from_index(999));
        glue_vertices(&mut k, &mut b, stale, stale, None).unwrap();
        glue_vertices(&mut k, &mut b, None, None, None).unwrap();
        assert!(b.vertices().is_empty());
    }
}
