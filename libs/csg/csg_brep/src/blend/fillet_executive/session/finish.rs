//! The live session's `DoFilleting` / `DoFillet` drivers and the post-merge tail.
//!
//! Upstream: `source/SMLib/src/SmFilletExecutive.cpp` (`DoFillet` 1568-1593,
//! `DoFilleting` 2917-3648).
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`FilletSession::do_fillet`] | `SmFilletExecutive::DoFillet` |
//! | [`FilletSession::do_filleting`] | `SmFilletExecutive::DoFilleting` |
//! | `FinishAccess` (private) | the tail after `MergeBrep` (3532-3644) on the live bodies |
//!
//! Deviations: the stage order, flags and the tail itself are the shared generic
//! [`FilletExecutive::finish_merged_fillet`]; this file only supplies the live bodies.
//! The original-trim mark lock encloses the tail, as upstream's `sMarkLocks` scope.
//! `delete m_pFilletBrep` clears the session attributes and notification records of
//! every entity of the intermediate body before `~SmBrep`. `RemoveTopoEdges` and
//! `BooleanPostProcess` notify through local environments (their ports take none);
//! afterwards the attributes of target edges they deleted are cleared from the
//! session store. With the opt-in `Kernel::fixes.fillet_refuse_non_manifold_result`
//! `do_fillet` ends with the closed-manifold check of the generic
//! `FilletExecutive::do_fillet` (not upstream, WR-212, source issue #422);
//! the public wrappers that drive it put the caller's body back on that refusal.
//! After the in-place merge (also on error) the geometry bound to the target's
//! edges, edge uses and faces under the session's kernel identities is rebound to the
//! target's naming, as the piecewise merge does (owners are pointers upstream;
//! source issue #423).
//! No public API entry point reaches `DoPiecewiseMerge` or a live-session
//! `IntersectInsertRelate` (global merge): `SetFilletParameters` resets both flags;
//! [`FilletSession::set_do_piecewise_merge`] selects the piecewise merge after it.
//! No recursion.

use super::original_trim::OriginalSurfaceDeletion;
use super::{invalid, FilletBody, FilletObject, FilletSession, SessionObjects};
use crate::blend::fillet_executive::p3::{
    ErrorInfo, FilletExecutive, FilletFinishAccess, StitchAttempt, StitchSettings,
};
use crate::blend::fillet_geom::p1::FilletErrorType;
use crate::error::{EntityRef, Result, SolidError};
use crate::geom::object::NotifyOperation;
use crate::kernel::Kernel;
use crate::topology::brep::{p1, p3};
use crate::topology::{BrepId, EdgeId};

const OP: &str = "blend::fillet_executive::session::finish";

/// Whether `error` is the refusal of `fixes.fillet_refuse_non_manifold_result`
/// (WR-212): a result that is not a closed manifold solid, the only status 1001 of
/// this file that names an edge. The public wrappers recognise it to put the caller's
/// body back.
pub(crate) fn is_non_manifold_refusal(error: &SolidError) -> bool {
    matches!(
        error,
        SolidError::Upstream {
            operation: OP,
            code: 1001,
            entity: Some(EntityRef::Edge(_)),
        }
    )
}

/// `error` without the entity it names (the refused result's edge, gone once the
/// caller's body is put back).
pub(crate) fn without_entity(error: SolidError) -> SolidError {
    match error {
        SolidError::Upstream {
            operation, code, ..
        } => SolidError::Upstream {
            operation,
            code,
            entity: None,
        },
        other => other,
    }
}

/// `FILEXEC_ERR`: the executive's generic failure status.
fn failure() -> SolidError {
    SolidError::Upstream {
        operation: OP,
        code: 1001,
        entity: None,
    }
}

/// Ordinary statuses become `None`; deferrals propagate.
fn ignore<T>(result: Result<T>) -> Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.is_unported() => Err(error),
        Err(_) => Ok(None),
    }
}

fn target_edge(object: FilletObject) -> Result<EdgeId> {
    match object {
        FilletObject::Topology(FilletBody::Target, EntityRef::Edge(edge)) => Ok(edge),
        _ => Err(invalid("object is not an actual target edge")),
    }
}

/// The topology entity of an intersector mate (an intermediate or target object).
const fn mate_entity(object: FilletObject) -> Option<EntityRef> {
    match object {
        FilletObject::Topology(_, entity) | FilletObject::IntermediateTopology(entity) => {
            Some(entity)
        }
        _ => None,
    }
}

pub(super) struct FinishAccess<'a, 'target>(pub(super) &'a mut SessionObjects<'target>);

impl FinishAccess<'_, '_> {
    fn require_target(brep: FilletObject) -> Result<()> {
        if brep == FilletObject::Body(FilletBody::Target) {
            Ok(())
        } else {
            Err(invalid("finish body is not the actual target"))
        }
    }

    /// `ClearAttributes` of target edges a local-environment edit deleted.
    fn clear_deleted_edges(&mut self, before: &[EdgeId]) -> Result<()> {
        let serial = self.0.target_serial;
        for &edge in before {
            if self.0.target.edge(edge).is_err() {
                let key = p3::topology_ref(serial, EntityRef::Edge(edge)).key;
                self.0.attributes.clear_attributes(key)?;
            }
        }
        Ok(())
    }
}

impl FilletFinishAccess<FilletObject> for FinishAccess<'_, '_> {
    fn finish_delete_brep(&mut self, k: &mut Kernel, brep: FilletObject) -> Result<()> {
        if brep != FilletObject::IntermediateBrep {
            return Err(invalid("deleted body is not the actual intermediate body"));
        }
        let mut body = self
            .0
            .intermediate
            .take()
            .ok_or_else(|| invalid("intermediate body is not allocated"))?;
        let serial = body
            .notify_serial()
            .ok_or_else(|| invalid("intermediate body has no kernel identity"))?;
        // ~SmAObject: every remaining object leaves its attributes' user lists.
        let mut entities = Vec::new();
        entities.extend(body.faces().iter().map(|(x, _)| EntityRef::Face(x)));
        entities.extend(body.loops().iter().map(|(x, _)| EntityRef::Loop(x)));
        entities.extend(body.edges().iter().map(|(x, _)| EntityRef::Edge(x)));
        entities.extend(body.edgeuses().iter().map(|(x, _)| EntityRef::Edgeuse(x)));
        entities.extend(body.vertices().iter().map(|(x, _)| EntityRef::Vertex(x)));
        entities.extend(
            body.vertexuses()
                .iter()
                .map(|(x, _)| EntityRef::Vertexuse(x)),
        );
        for entity in entities {
            let key = p3::topology_ref(serial, entity).key;
            self.0.attributes.clear_attributes(key)?;
            self.0.notification_records.remove(&key);
        }
        crate::topology::brep::p4::destroy(k, &mut body)
    }
    fn finish_tolerance(&self, _k: &Kernel, brep: FilletObject) -> Result<f64> {
        Self::require_target(brep)?;
        Ok(self.0.target.tolerance())
    }
    fn finish_do_stitching(
        &mut self,
        k: &mut Kernel,
        brep: FilletObject,
        tolerance: f64,
        settings: StitchSettings,
    ) -> Result<StitchAttempt> {
        Self::require_target(brep)?;
        let serial = self.0.target_serial;
        let key_of = |entity| p3::topology_ref(serial, entity);
        let keys = |object| match object {
            crate::topology::brep_cache::CacheObject::Brep => {
                key_of(EntityRef::Brep(BrepId::from_index(0)))
            }
            crate::topology::brep_cache::CacheObject::Other(owner) => owner,
            object => crate::topology::brep::p2::caller_cache_key(&key_of, object),
        };
        let owner_of = |_| None;
        let identity = crate::query::brep::BrepIdentity {
            brep_id: BrepId::from_index(
                u32::try_from(serial)
                    .map_err(|_| invalid("target body identity exceeds query domain"))?,
            ),
            keys: &keys,
            owner_of: &owner_of,
        };
        let SessionObjects {
            target,
            notification_records,
            attributes,
            ..
        } = &mut *self.0;
        if k.context.issued_brep_serial(target.notify_identity_slot()) != Some(serial) {
            return Err(invalid(
                "stitch body belongs to another notification context",
            ));
        }
        let result = crate::geom::context::Context::with_notification_context(k, |k, context| {
            p3::seed_records(notification_records, target, &key_of);
            let mut env = crate::topology::edge::NotifyEnv {
                records: notification_records,
                attributes,
                context: Some(context),
                key_of: &key_of,
            };
            let mut edits = crate::heal::stitch::p1::BorrowedStitchEdits {
                env: &mut env,
                curve_identity: crate::topology::brep::p9::CurveIdentity::Actual(
                    &crate::topology::brep::p9::issue_curve_identity,
                ),
            };
            // SmStitch sStitch(sCallBack, dStitchTol3d): base callback, default flags.
            let mut callback = crate::heal::stitch::p1::DefaultStitchCallback::new();
            let mut stitch = crate::heal::stitch::p1::Stitch::new(
                &mut callback,
                tolerance,
                settings.do_region_nesting,
                settings.validate_result,
            );
            stitch.query_identity = Some(identity);
            stitch.edits = Some(&mut edits);
            stitch.ignore_problems = settings.ignore_problems;
            stitch.fast_edge_compare = settings.fast_edge_compare;
            stitch.squeeze_small_edges = settings.squeeze_small_edges;
            stitch.split_edges_with_vertices = settings.split_edges_with_vertices;
            stitch.making_manifold_solid = settings.making_manifold_solid;
            stitch.remove_laminar_slivers = settings.remove_laminar_slivers;
            stitch.do_stitching(
                k,
                target,
                None,
                None,
                crate::heal::stitch::p2::MinGapRequest::default(),
            )
        })?;
        match result {
            Ok(done) => Ok(StitchAttempt {
                status: None,
                stitched: done.stitched_edges,
                lamina: done.lamina_edges,
                max_vertex_gap: done.max_vertex_gap,
                max_edge_gap: done.max_edge_gap,
            }),
            Err(error) if error.is_unported() => Err(error),
            Err(error) => Ok(StitchAttempt {
                status: Some(error),
                stitched: 0,
                lamina: 0,
                max_vertex_gap: 0.0,
                max_edge_gap: 0.0,
            }),
        }
    }
    fn finish_attribute_users(&self, attribute: FilletObject) -> Result<Vec<FilletObject>> {
        let FilletObject::Attribute(handle) = attribute else {
            return Err(invalid("merge tag is not an actual attribute"));
        };
        let serial = self.0.target_serial;
        let edges = p1::edges(self.0.target, None)?;
        self.0
            .attributes
            .users(handle)?
            .into_iter()
            .map(|user| {
                edges
                    .iter()
                    .find(|edge| p3::topology_ref(serial, EntityRef::Edge(**edge)).key == user)
                    .map(|edge| FilletObject::Topology(FilletBody::Target, EntityRef::Edge(*edge)))
                    .ok_or_else(|| invalid("merge tag user is not an actual target edge"))
            })
            .collect()
    }
    fn finish_remove_topo_edges(
        &mut self,
        k: &mut Kernel,
        executive: &mut FilletExecutive<FilletObject>,
    ) -> Result<()> {
        let edges = executive
            .topo_edges
            .iter()
            .map(|edge| edge.map(target_edge).transpose())
            .collect::<Result<Vec<_>>>()?;
        let before = p1::edges(self.0.target, None)?;
        let result =
            crate::blend::fillet_executive::p1::remove_topo_edges(k, self.0.target, &edges);
        let cleared = self.clear_deleted_edges(&before);
        result.and(cleared)
    }
    fn finish_boolean_post_process(
        &mut self,
        k: &mut Kernel,
        executive: &mut FilletExecutive<FilletObject>,
        check_all_edges: bool,
    ) -> Result<()> {
        let mut relations = crate::boolean::merge::p1::CleanupRelations::default();
        for edge in executive.intersection_edges.iter().flatten() {
            relations.edges.push(target_edge(*edge)?);
        }
        for vertex in executive.intersection_vertices.iter().flatten() {
            let FilletObject::Topology(FilletBody::Target, EntityRef::Vertex(vertex)) = *vertex
            else {
                return Err(invalid(
                    "intersection vertex is not an actual target vertex",
                ));
            };
            relations.vertices.push(vertex);
        }
        // `m_vTI.GetOtherMate` of the target's edges and vertices (the FALSE edge and
        // vertex filters and `GetCommonVertices`): only whether a mate exists is read, so
        // each mate is recorded under its own operand-local id.
        for edge in p1::edges(self.0.target, None)? {
            let key = FilletObject::Topology(FilletBody::Target, EntityRef::Edge(edge));
            if let Some(EntityRef::Edge(other)) = executive
                .intersector
                .other_mate(Some(key))
                .and_then(mate_entity)
            {
                relations.mates.edges.record(edge, other)?;
            }
        }
        for vertex in p1::vertices(self.0.target, None)? {
            let key = FilletObject::Topology(FilletBody::Target, EntityRef::Vertex(vertex));
            if let Some(EntityRef::Vertex(other)) = executive
                .intersector
                .other_mate(Some(key))
                .and_then(mate_entity)
            {
                relations.mates.vertices.record(vertex, other)?;
            }
        }
        if let Some((edges, faces)) = &self.0.cleanup_kept {
            relations.kept_edges.clone_from(edges);
            relations.kept_faces.clone_from(faces);
        }
        let before = p1::edges(self.0.target, None)?;
        let result = crate::boolean::merge::p1::boolean_post_process(
            k,
            self.0.target,
            &relations,
            check_all_edges,
        );
        let cleared = self.clear_deleted_edges(&before);
        result.and(cleared)
    }
    fn finish_faces(&self, brep: FilletObject) -> Result<Vec<FilletObject>> {
        Self::require_target(brep)?;
        Ok(p1::faces(self.0.target, None)?
            .into_iter()
            .map(|face| FilletObject::Topology(FilletBody::Target, EntityRef::Face(face)))
            .collect())
    }
    fn finish_notify_face(
        &mut self,
        k: &mut Kernel,
        face: FilletObject,
        operation: NotifyOperation,
        data1: Option<FilletObject>,
        data2: Option<FilletObject>,
        data3: Option<FilletObject>,
    ) -> Result<()> {
        let FilletObject::Topology(FilletBody::Target, EntityRef::Face(id)) = face else {
            return Err(invalid("notified object is not an actual target face"));
        };
        let serial = self.0.target_serial;
        let key_of = |entity| p3::topology_ref(serial, entity);
        let data =
            |object: Option<FilletObject>| -> Result<Option<crate::geom::object::ObjectRef>> {
                match object {
                    None => Ok(None),
                    Some(FilletObject::Topology(FilletBody::Target, entity)) => {
                        Ok(Some(key_of(entity)))
                    }
                    Some(_) => Err(invalid("notification datum is not actual target topology")),
                }
            };
        let data = [data(data1)?, data(data2)?, data(data3)?];
        let SessionObjects {
            target,
            notification_records,
            attributes,
            ..
        } = &mut *self.0;
        p3::seed_records(notification_records, target, &key_of);
        crate::geom::context::Context::with_notification_context(k, |_, context| {
            let mut env = crate::topology::edge::NotifyEnv {
                records: notification_records,
                attributes,
                context: Some(context),
                key_of: &key_of,
            };
            crate::topology::face::p3::notify(target, id, &mut env, None, operation, data)
        })?
    }
    fn finish_orient_trimmed_surfaces(
        &mut self,
        k: &mut Kernel,
        brep: FilletObject,
        outward: bool,
        maybe_not_solid: &mut bool,
        just_unify: bool,
    ) -> Result<()> {
        Self::require_target(brep)?;
        let serial = self.0.target_serial;
        let key_of = |entity| p3::topology_ref(serial, entity);
        let SessionObjects {
            target,
            notification_records,
            attributes,
            ..
        } = &mut *self.0;
        p3::seed_records(notification_records, target, &key_of);
        let oriented =
            crate::geom::context::Context::with_notification_context(k, |k, context| {
                let mut env = crate::topology::edge::NotifyEnv {
                    records: notification_records,
                    attributes,
                    context: Some(context),
                    key_of: &key_of,
                };
                let mut marks = crate::topology::topology_traverser::Marks::default();
                p3::orient_trimmed_surfaces(
                    k, target, &mut env, &mut marks, outward, just_unify, None,
                )
            })??;
        *maybe_not_solid = oriented.maybe_not_closed_solid;
        Ok(())
    }
}

impl FilletSession<'_> {
    /// `DoFillet`: refuse uninitialized parameters, create the corners, then
    /// [`Self::do_filleting`] and the closed-manifold check of the generic
    /// [`FilletExecutive::do_fillet`].
    ///
    /// # Errors
    /// The first stage error, with the executive's error info recorded as upstream.
    /// With `fixes.fillet_vertex_error_diagnostics`, a missing-corner note from the
    /// corner stage is kept in [`FilletSession::error_info`] across the later failure;
    /// the returned error is still the later stage's own status.
    pub fn do_fillet(&mut self, k: &mut Kernel) -> Result<()> {
        if !self.executive.initialized {
            // The generic NoteError overload changes exactly these two members.
            self.executive.error.code = 1; // SM_FILERR_EXEC_UNINITIALIZED
            self.executive.error.comment = "ERROR: fillet parameters not set up.".into();
            return Err(invalid("fillet parameters not set up"));
        }
        self.create_corners(k, None)?;
        // The opt-in missing-corner note is the reason the later stage fails; keep it
        // through that later error so `error_info()` reports the unsupported vertex,
        // while the returned failure stays the later stage's own status.
        let missing_corner_note = (k.fixes.fillet_vertex_error_diagnostics
            && self.executive.error.code == FilletErrorType::CornerUnsupported as i32)
            .then(|| self.executive.error.clone());
        let result = self.do_filleting(k, None);
        if result.is_err() {
            if let Some(note) = missing_corner_note {
                self.executive.error = note;
            }
        }
        result?;
        if !k.fixes.fillet_refuse_non_manifold_result {
            return Ok(());
        }
        self.check_closed_manifold()
    }

    /// The closed-manifold result check of `FilletExecutive::do_fillet` (not
    /// upstream, opt-in `fixes.fillet_refuse_non_manifold_result`, WR-212,
    /// source issue #422), on the live target.
    fn check_closed_manifold(&mut self) -> Result<()> {
        for edge in p1::edges(self.objects.target, None)? {
            if crate::topology::edge::is_manifold(self.objects.target, edge)? {
                continue;
            }
            self.executive.error.code = 7; // SM_FILERR_BAD_MERGE
            self.executive.error.status = 2; // SM_FIL_FAILURE
            self.executive.error.edge = Some(FilletObject::Topology(
                FilletBody::Target,
                EntityRef::Edge(edge),
            ));
            self.executive.error.comment =
                "Fillet Merge Failure: Result Is Not A Closed Manifold Solid".into();
            return Err(SolidError::Upstream {
                operation: OP,
                code: 1001,
                entity: Some(EntityRef::Edge(edge)),
            });
        }
        Ok(())
    }

    /// `DoFilleting`: build the intermediate fillet body, shrink its main faces,
    /// insert its intersection topology, then (with trimming) select and delete the
    /// original faces, tag and merge the fillet faces and run the post-merge tail
    /// under one original-trim mark scope. The Boolean flag and target editing are
    /// restored on every result.
    ///
    /// # Errors
    /// The first stage error, with all earlier mutations retained.
    pub fn do_filleting(
        &mut self,
        k: &mut Kernel,
        mut error_out: Option<&mut ErrorInfo<FilletObject>>,
    ) -> Result<()> {
        self.sync_self_intersection_handler();
        // WR-459: the clean-up after filleting leaves the target's own edges and faces.
        self.objects.cleanup_kept = if k.fixes.fillet_cleanup_keeps_target_faces {
            Some((
                p1::edges(self.objects.target, None)?.into_iter().collect(),
                p1::faces(self.objects.target, None)?.into_iter().collect(),
            ))
        } else {
            None
        };
        self.with_filleting_context(k, |session, k| {
            if ignore(session.create_fillet_brep(k, error_out.as_deref_mut()))?.is_none() {
                return Err(failure());
            }
            let faces = session
                .objects
                .intermediate
                .as_ref()
                .map(|body| p1::faces(body, None))
                .transpose()?
                .map_or(0, |faces| faces.len());
            if faces < 1 {
                return Err(failure());
            }
            if k.fixes.fillet_refuse_crossing_rails {
                session.refuse_crossing_rails(k)?;
            }
            session.shrink_fillet_faces(k)?;
            // The fork's SetMarkCornerPatches (60af574): mark the set-back corners'
            // patch faces, which the merge copies into the target with their marks.
            session.mark_corner_patches()?;
            let editing = session.objects.target.editing_enabled();
            session.objects.target.set_editing_enabled(true);
            let result = session.merge_fillet(k);
            session.objects.target.set_editing_enabled(editing);
            result?;
            if let Some(output) = error_out {
                output.assign(&session.executive.error);
            }
            Ok(())
        })
    }

    fn merge_fillet(&mut self, k: &mut Kernel) -> Result<()> {
        self.executive
            .intersector
            .set_primary_brep(Some(FilletObject::Body(FilletBody::Target)));
        self.executive
            .intersector
            .set_other_brep(Some(FilletObject::IntermediateBrep));
        if self.executive.options.piecewise_merge {
            // DoPiecewiseMerge deletes m_pFilletBrep inside its mark scope
            return self.do_piecewise_merge(k);
        }
        let result = self.merge_fillet_in_place(k);
        // (not upstream: owners are pointers there) the insertion names the geometry
        // it binds to the target's edges and faces by the session's kernel identity
        // of the entity; rebind it to the same entity's identity in the target's
        // naming, as the piecewise merge does, so the curve owner test of
        // `SmEdge::ValidateGeometry` (`SmBrep` test #61) finds its edge
        // (source issue #423). Also after an error.
        let naming = self.objects.target.naming();
        let serial = self.objects.target_serial;
        let session_owner = |e| p3::topology_ref(serial, e);
        let restored =
            super::piecewise::normalize_session_owners(self.objects.target, session_owner, |e| {
                super::piecewise::naming_owner(naming, e).unwrap_or_else(|| session_owner(e))
            });
        result.and(restored)
    }

    /// The in-place (non-piecewise) body of [`Self::merge_fillet`].
    fn merge_fillet_in_place(&mut self, k: &mut Kernel) -> Result<()> {
        // SetFilletBrepEdgeMap, then the geometry and corner insertion.
        self.insert_fillet_intersection_topology(k)?;
        // Upstream cbi_MAP_TOPO_EDGES=0: do not collect mapped intersections.
        if self.executive.options.global_merge {
            // SmTol::GetApproxTol3d(m_pTargetBrep), then m_vTI.IntersectInsertRelate()
            // between the target and the intermediate body.
            let tolerance =
                crate::geom::tol::approx_tol3d(k, 0.5 * self.objects.target.tolerance());
            self.executive.intersector.set_this_approx_tol_3d(tolerance);
            self.with_insertion_geometry(k, |k, ti, geometry| {
                ti.intersect_insert_relate(k, geometry, FilletObject::Intersector)
                    .map(|_| ())
            })?;
        }
        if self.executive.options.trimming {
            self.with_original_trim_marks(k, |session, k, locks, serial| {
                let mut deleted = OriginalSurfaceDeletion::default();
                session.select_and_delete_with_marks(k, locks, serial, &mut deleted)?;
                session.tag_and_merge_retained(k)?;
                session.finish_merge(k)
            })?;
        }
        // The final spine-edge scan only emits SM_DBG_WARN in this release build.
        Ok(())
    }

    /// The post-merge tail on the live bodies, then the destruction of the temporary
    /// tags (upstream `SmObjsDelete`, on every result).
    fn finish_merge(&mut self, k: &mut Kernel) -> Result<()> {
        let handles = self
            .objects
            .merge_attributes
            .take()
            .ok_or_else(|| invalid("fillet faces have not been merged"))?;
        let tags: Vec<_> = handles
            .iter()
            .copied()
            .map(FilletObject::Attribute)
            .collect();
        let result = self.executive.finish_merged_fillet(
            k,
            &mut FinishAccess(&mut self.objects),
            FilletObject::Body(FilletBody::Target),
            FilletObject::IntermediateBrep,
            &tags,
        );
        let cleanup = super::original_merge::destroy_tags(&mut self.objects, &tags);
        result.and(cleanup)
    }
}

#[cfg(test)]
mod stitch_identity_tests {
    use super::*;
    use crate::blend::fillet_executive::p3::{FaceConstructionAccess, FaceStitchAccess};
    use crate::math::Point3;

    #[test]
    fn both_stitch_sites_reject_a_foreign_notification_domain() {
        let mut owner = Kernel::default();
        let mut target = crate::primitive::api_primitives::create_box(
            &mut owner,
            Point3::ZERO,
            10.0,
            20.0,
            30.0,
        )
        .unwrap();
        let mut session = FilletSession::new(&mut owner, &mut target).unwrap();
        session.objects.faces_new_brep(&mut owner).unwrap();
        let before_records = session.objects.notification_records.len();
        let before_key = session.objects.target.key();
        let mut foreign = Kernel::default();
        let other =
            crate::primitive::api_primitives::create_box(&mut foreign, Point3::ZERO, 1.0, 2.0, 3.0)
                .unwrap();
        assert_eq!(
            other.notify_serial(),
            session.objects.target.notify_serial()
        );
        let tolerance = session.objects.target.tolerance();
        let target = FilletObject::Body(FilletBody::Target);
        assert!(FinishAccess(&mut session.objects)
            .finish_do_stitching(&mut foreign, target, tolerance, StitchSettings::default(),)
            .is_err());
        assert!(session
            .objects
            .stitch_body(&mut foreign, FilletObject::IntermediateBrep, tolerance,)
            .is_err());
        assert_eq!(session.objects.notification_records.len(), before_records);
        assert_eq!(session.objects.target.key(), before_key);
        assert!(session.objects.intermediate.is_some());
        assert_eq!(
            owner
                .context
                .issued_brep_serial(session.objects.target.notify_identity_slot()),
            Some(session.objects.target_serial)
        );
    }
}

/// `fixes.fillet_refuse_ball_not_fitting` (WR-462, source issue #1172): a
/// 60 x 40 x 20 box, its top edge at y = 40 rounded r5, then its top edge at x = 60
/// rounded r3, then its vertical edge at x = 60, y = 40 with the two edges up into the
/// corner (the side's arc on the r5 round, the seam between the rounds).
#[cfg(test)]
mod ball_fit_tests {
    use super::*;
    use crate::blend::fillet_geom::p1::FilletErrorType;
    use crate::geom::global_solver::SolverOperation;
    use crate::math::Point3;
    use crate::topology::{edge, Brep};

    const NO_ROOM: i32 = FilletErrorType::NoRoom as i32;

    fn edge_near(k: &Kernel, b: &Brep, point: [f64; 3]) -> EdgeId {
        let point = Point3::new(point[0], point[1], point[2]);
        let mut nearest = (f64::MAX, None);
        for id in p1::edges(b, None).unwrap() {
            let Some(curve) = edge::curve(b, id).unwrap() else {
                continue;
            };
            let interval = edge::interval(b, id).unwrap();
            let d = curve
                .drop_point(
                    k,
                    &interval,
                    point,
                    None,
                    1e-6,
                    None,
                    SolverOperation::Minimize,
                )
                .unwrap();
            if d.success && d.distance < nearest.0 {
                nearest = (d.distance, Some(id));
            }
        }
        assert!(nearest.0 < 2e-3, "no edge near {point:?}");
        nearest.1.unwrap()
    }

    /// The executive's error code after the corner round of `radius` with the fix on or
    /// off, and its result.
    fn corner_round(radius: f64, fix: bool) -> (i32, Result<()>) {
        let mut k = Kernel::default();
        let mut body =
            crate::primitive::api_primitives::create_box(&mut k, Point3::ZERO, 60.0, 40.0, 20.0)
                .unwrap();
        for (point, r) in [([30.0, 40.0, 20.0], 5.0), ([60.0, 20.0, 20.0], 3.0)] {
            let e = edge_near(&k, &body, point);
            crate::blend::fillet_executive::api::fillet_edges(
                &mut k,
                &mut body,
                &[e],
                r,
                1,
                1,
                1.0,
            )
            .unwrap();
        }
        let edges: Vec<_> = [
            [60.0, 40.0, 7.5],
            [60.0, 35.0 + 24.0_f64.sqrt(), 16.0],
            [58.93224999673208, 37.56010200025561, 19.29486599217306],
        ]
        .into_iter()
        .map(|p| edge_near(&k, &body, p))
        .collect();
        k.fixes.fillet_refuse_ball_not_fitting = fix;
        let mut session = FilletSession::new(&mut k, &mut body).unwrap();
        session
            .prepare_constant_radius_edges(&mut k, &edges, radius, 1, 1, 1.0)
            .unwrap();
        let result = session.do_fillet(&mut k);
        (session.executive.error.code, result)
    }

    #[test]
    fn a_ball_that_does_not_fit_is_no_room_before_the_corners() {
        for radius in [3.1, 7.0] {
            let (code, result) = corner_round(radius, false);
            assert!(matches!(
                result,
                Err(SolidError::Upstream { code: 1001, .. })
            ));
            assert_ne!(code, NO_ROOM, "r{radius}: the corners fail on their own");
            let (code, result) = corner_round(radius, true);
            assert!(matches!(
                result,
                Err(SolidError::Upstream { code: 1001, .. })
            ));
            assert_eq!(code, NO_ROOM, "r{radius}: NoRoom");
        }
    }

    #[test]
    fn a_ball_that_fits_is_not_refused() {
        let (code, result) = corner_round(2.0, true);
        // the round past the gate completes (main wired SmFace::PointClassify, which this
        // test used to stop at)
        result.unwrap();
        assert_ne!(code, NO_ROOM);
    }
}
