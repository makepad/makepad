//! Original-face selection using the session's actual topology marks.

use super::{invalid, FilletBody, FilletObject, FilletSession, SessionObjects};
use crate::blend::fillet_executive::p3::{
    FilletExecutive, OriginalSurfaceDeletionAccess, OriginalSurfaceTrimAccess,
};
use crate::blend::fillet_geom::p6::{
    TrimOriginalBody, TrimOriginalSurfacesEnv, TrimOriginalSurfacesMarks,
};
use crate::error::{EntityRef, Result};
use crate::geom::context::{MarkLock, MarkType, ALL_MARKS};
use crate::kernel::Kernel;
use crate::topology::{
    brep::{
        p1,
        p3::{seed_records, topology_ref},
    },
    topology_traverser::Entity,
    FaceId,
};

/// Face selections before the subsequent native deletion and merge operations.
#[derive(Debug, Default)]
pub struct OriginalSurfaceSelection {
    /// Intermediate faces selected for retention, in discovery order.
    pub kept: Vec<FaceId>,
    /// Target faces selected for deletion, in discovery order.
    pub deleted: Vec<FaceId>,
}

/// Body-qualified face lists retained across selection and deletion failures.
#[derive(Debug, Default)]
pub struct OriginalSurfaceDeletion {
    /// Selected intermediate faces; actual handles may be invalidated by later edits.
    pub kept: Vec<FilletObject>,
    /// Target faces first, then unkept intermediate faces in owning traversal order.
    /// Deleted handles remain diagnostic identities and must not be dereferenced.
    pub deleted: Vec<FilletObject>,
}

struct DeleteAccess<'a, 'target>(&'a mut SessionObjects<'target>);
impl OriginalSurfaceDeletionAccess<FilletObject> for DeleteAccess<'_, '_> {
    fn original_managed_faces(&self, body: FilletObject) -> Result<Vec<FilletObject>> {
        if body != FilletObject::IntermediateBrep {
            return Err(invalid(
                "deletion fillet body is not the actual intermediate",
            ));
        }
        let body = self
            .0
            .intermediate
            .as_ref()
            .ok_or_else(|| invalid("intermediate body is not allocated"))?;
        Ok(p1::faces(body, None)?
            .into_iter()
            .map(|face| FilletObject::IntermediateTopology(EntityRef::Face(face)))
            .collect())
    }
    fn original_delete_faces(
        &mut self,
        k: &mut Kernel,
        intersector: &mut crate::boolean::topology_intersector::p1::TopologyIntersector<
            FilletObject,
        >,
        faces: &[FilletObject],
    ) -> Result<()> {
        crate::geom::context::Context::with_notification_context(k, |k, context| {
            let intermediate = self
                .0
                .intermediate
                .as_mut()
                .ok_or_else(|| invalid("intermediate body is not allocated"))?;
            let mut geometry = super::insertion::geometry::InsertionGeometry::new(
                self.0.target,
                &mut self.0.pseudo,
                intermediate,
            )?
            .with_notifications(
                &mut self.0.insertion_curves,
                super::insertion::geometry::InsertionNotifications {
                    records: &mut self.0.notification_records,
                    attributes: &mut self.0.attributes,
                    context,
                },
            )?;
            intersector.delete_faces(k, &mut geometry, faces, None)
        })?
    }
}

struct TrimMarks<'a> {
    records: &'a mut crate::topology::topology::NotifyRecords,
    fillet_serial: u64,
    target_serial: u64,
}
impl TrimMarks<'_> {
    fn key(&self, body: TrimOriginalBody, entity: Entity) -> crate::geom::attribute::ObjectKey {
        let entity = match entity {
            Entity::Face(id) => EntityRef::Face(id),
            Entity::Edge(id) => EntityRef::Edge(id),
            Entity::Vertex(id) => EntityRef::Vertex(id),
            Entity::Faceuse(id) => EntityRef::Faceuse(id),
            Entity::Vertexuse(id) => EntityRef::Vertexuse(id),
            Entity::Shell(id) => EntityRef::Shell(id),
        };
        topology_ref(
            match body {
                TrimOriginalBody::Fillet => self.fillet_serial,
                TrimOriginalBody::Target => self.target_serial,
            },
            entity,
        )
        .key
    }
}
impl TrimOriginalSurfacesMarks for TrimMarks<'_> {
    fn is_marked(
        &self,
        k: &Kernel,
        body: TrimOriginalBody,
        entity: Entity,
        mark: MarkType,
    ) -> bool {
        self.records
            .get(&self.key(body, entity))
            .is_some_and(|topology| topology.is_marked(&k.context, mark))
    }
    fn mark(&mut self, k: &Kernel, body: TrimOriginalBody, entity: Entity, mark: MarkType) {
        let key = self.key(body, entity);
        self.records.or_default(key).mark(Some(&k.context), mark);
    }
}

struct TrimAccess<'a, 'target> {
    objects: &'a mut SessionObjects<'target>,
    locks: &'a [MarkLock],
    fillet_serial: u64,
}
impl OriginalSurfaceTrimAccess<FilletObject, FaceId> for TrimAccess<'_, '_> {
    fn original_trim_geometries(&self, solver: FilletObject) -> Result<Vec<FilletObject>> {
        let FilletObject::Solver(id) = solver else {
            return Err(invalid("trim solver is not an actual solver"));
        };
        Ok(self
            .objects
            .solver(id)?
            .base()
            .fillet_geoms()
            .into_iter()
            .map(FilletObject::Geometry)
            .collect())
    }
    fn original_trim_geometry(
        &mut self,
        k: &mut Kernel,
        geom: FilletObject,
        kept: &mut Vec<FaceId>,
        deleted: &mut Vec<FaceId>,
        executive: &mut FilletExecutive<FilletObject>,
    ) -> Result<()> {
        let FilletObject::Geometry(geom) = geom else {
            return Err(invalid("trim geometry is not actual geometry"));
        };
        let id = self
            .objects
            .pseudo
            .geom(geom)?
            .fillet_solver()
            .ok_or_else(|| invalid("trim geometry has no actual solver"))?;
        let solver = self
            .objects
            .solvers
            .get(id.0 as usize)
            .and_then(Option::as_ref)
            .ok_or_else(|| invalid("trim solver is not live"))?
            .base();
        let fillet = self
            .objects
            .intermediate
            .as_mut()
            .ok_or_else(|| invalid("intermediate body is not allocated"))?;
        let mate =
            |edge| match executive
                .intersector
                .brep_mate(Some(FilletObject::IntermediateTopology(EntityRef::Edge(
                    edge,
                )))) {
                Some(FilletObject::Topology(FilletBody::Target, EntityRef::Edge(edge))) => {
                    Some(edge)
                }
                _ => None,
            };
        let key = |edge| topology_ref(self.fillet_serial, EntityRef::Edge(edge));
        let env = TrimOriginalSurfacesEnv {
            brep_mate: &mate,
            fillet_edge_key: &key,
            target_context: Some(0),
        };
        let mut marks = TrimMarks {
            records: &mut self.objects.notification_records,
            fillet_serial: self.fillet_serial,
            target_serial: self.objects.target_serial,
        };
        self.objects.pseudo.trim_original_surfaces_with_solver(
            k,
            geom,
            fillet,
            self.objects.target,
            &mut marks,
            kept,
            deleted,
            self.locks,
            &env,
            id,
            solver,
        )
    }
}

impl FilletSession<'_> {
    /// Select original faces using actual common-mate marks and retained solvers.
    /// This component ends its mark scope before returning; it does not delete faces
    /// or complete native trimming and merging.
    ///
    /// # Errors
    /// Missing actual body/context authority or the first selection failure.
    /// Both bodies must have identities issued by this same kernel context;
    /// a distinct target context needs the native second-lock capability.
    /// Previously written topology marks remain; locks and editing are restored.
    pub fn select_original_fillet_surfaces(
        &mut self,
        k: &mut Kernel,
        selection: &mut OriginalSurfaceSelection,
    ) -> Result<()> {
        let editing = self.objects.target.editing_enabled();
        self.objects.target.set_editing_enabled(true);
        let result = self.with_original_trim_marks(k, |session, k, locks, serial| {
            session.executive.select_original_surfaces(
                k,
                &mut TrimAccess {
                    objects: &mut session.objects,
                    locks,
                    fillet_serial: serial,
                },
                &mut selection.kept,
                &mut selection.deleted,
            )
        });
        self.objects.target.set_editing_enabled(editing);
        result
    }

    /// Select and delete original faces under the same actual mark lock.
    /// The output is replaced by this attempt's body-qualified partial lists.
    /// Stops before native attribute tagging and merging.
    ///
    /// # Errors
    /// Original selection/deletion errors, retaining completed list and store mutations.
    /// Locks and target editing are restored on every returned result.
    pub fn trim_and_delete_original_fillet_faces(
        &mut self,
        k: &mut Kernel,
        output: &mut OriginalSurfaceDeletion,
    ) -> Result<()> {
        *output = OriginalSurfaceDeletion::default();
        let editing = self.objects.target.editing_enabled();
        self.objects.target.set_editing_enabled(true);
        let result = self.with_original_trim_marks(k, |session, k, locks, serial| {
            session.select_and_delete_with_marks(k, locks, serial, output)
        });
        self.objects.target.set_editing_enabled(editing);
        result
    }

    pub(super) fn select_and_delete_with_marks(
        &mut self,
        k: &mut Kernel,
        locks: &[MarkLock],
        serial: u64,
        output: &mut OriginalSurfaceDeletion,
    ) -> Result<()> {
        let mut selection = OriginalSurfaceSelection::default();
        let selected = self.executive.select_original_surfaces(
            k,
            &mut TrimAccess {
                objects: &mut self.objects,
                locks,
                fillet_serial: serial,
            },
            &mut selection.kept,
            &mut selection.deleted,
        );
        output.kept = selection
            .kept
            .into_iter()
            .map(|face| FilletObject::IntermediateTopology(EntityRef::Face(face)))
            .collect();
        output.deleted = selection
            .deleted
            .into_iter()
            .map(|face| FilletObject::Topology(FilletBody::Target, EntityRef::Face(face)))
            .collect();
        selected?;
        self.executive.delete_original_surface_faces(
            k,
            &mut DeleteAccess(&mut self.objects),
            FilletObject::IntermediateBrep,
            &output.kept,
            &mut output.deleted,
        )
    }

    /// `DoSurfaceFilleting`'s trimming (`SmFilletExecutive.cpp` 3875-3960): under the
    /// same common-mate marks, each geometry's `TrimOriginalSurfaces`, then
    /// `GetTopologyIntersector().DeleteFaces(sDelFaces)` of the selected original faces
    /// only (unlike `DoFilleting`, unkept fillet faces are not added).
    ///
    /// # Errors
    /// The first selection or deletion error; locks are released on every result.
    pub(super) fn trim_original_surfaces_for_surface_filleting(
        &mut self,
        k: &mut Kernel,
    ) -> Result<()> {
        self.with_original_trim_marks(k, |session, k, locks, serial| {
            let mut kept = Vec::new();
            let mut deleted = Vec::new();
            session.executive.select_original_surfaces(
                k,
                &mut TrimAccess {
                    objects: &mut session.objects,
                    locks,
                    fillet_serial: serial,
                },
                &mut kept,
                &mut deleted,
            )?;
            let deleted: Vec<_> = deleted
                .into_iter()
                .map(|face| FilletObject::Topology(FilletBody::Target, EntityRef::Face(face)))
                .collect();
            DeleteAccess(&mut session.objects).original_delete_faces(
                k,
                &mut session.executive.intersector,
                &deleted,
            )
        })
    }

    // The lock scope can also enclose later native trim continuation without
    // advancing marks or copying the authoritative record map between components.
    pub(super) fn with_original_trim_marks<T>(
        &mut self,
        k: &mut Kernel,
        body: impl FnOnce(&mut Self, &mut Kernel, &[MarkLock], u64) -> Result<T>,
    ) -> Result<T> {
        let fillet = self
            .objects
            .intermediate
            .as_mut()
            .ok_or_else(|| invalid("intermediate body is not allocated"))?;
        let serial = k
            .context
            .issued_brep_serial(fillet.notify_identity_slot())
            .ok_or_else(|| invalid("intermediate body belongs to another context"))?;
        let target_serial = k
            .context
            .issued_brep_serial(self.objects.target.notify_identity_slot())
            .ok_or_else(|| invalid("target body belongs to another context"))?;
        if target_serial != self.objects.target_serial || serial == target_serial {
            return Err(invalid("trim body identity is not authoritative"));
        }
        seed_records(&mut self.objects.notification_records, fillet, &|entity| {
            topology_ref(serial, entity)
        });
        seed_records(
            &mut self.objects.notification_records,
            self.objects.target,
            &|entity| topology_ref(target_serial, entity),
        );
        let mut lock = MarkLock::new(std::slice::from_mut(&mut k.context), 0, ALL_MARKS)?;
        let result = (|| {
            self.mark_common_trim_topology(k, serial, lock.mark_type())?;
            body(self, k, std::slice::from_ref(&lock), serial)
        })();
        let released = lock.release(std::slice::from_mut(&mut k.context));
        match result {
            Err(error) => Err(error),
            Ok(value) => released.map(|()| value),
        }
    }

    fn mark_common_trim_topology(&mut self, k: &Kernel, serial: u64, mark: MarkType) -> Result<()> {
        let fillet = self
            .objects
            .intermediate
            .as_ref()
            .ok_or_else(|| invalid("intermediate body is not allocated"))?;
        let mut marks = TrimMarks {
            records: &mut self.objects.notification_records,
            fillet_serial: serial,
            target_serial: self.objects.target_serial,
        };
        for vertex in p1::vertices(self.objects.target, None)? {
            if let Some(mate) = self
                .executive
                .intersector
                .other_mate(Some(FilletObject::Topology(
                    FilletBody::Target,
                    EntityRef::Vertex(vertex),
                )))
            {
                let FilletObject::IntermediateTopology(EntityRef::Vertex(other)) = mate else {
                    return Err(invalid(
                        "common vertex mate is not actual intermediate topology",
                    ));
                };
                fillet.vertex(other)?;
                marks.mark(k, TrimOriginalBody::Fillet, Entity::Vertex(other), mark);
                marks.mark(k, TrimOriginalBody::Target, Entity::Vertex(vertex), mark);
            }
        }
        for edge in p1::edges(self.objects.target, None)? {
            if let Some(mate) = self
                .executive
                .intersector
                .other_mate(Some(FilletObject::Topology(
                    FilletBody::Target,
                    EntityRef::Edge(edge),
                )))
            {
                let FilletObject::IntermediateTopology(EntityRef::Edge(other)) = mate else {
                    return Err(invalid(
                        "common edge mate is not actual intermediate topology",
                    ));
                };
                fillet.edge(other)?;
                marks.mark(k, TrimOriginalBody::Target, Entity::Edge(edge), mark);
                marks.mark(k, TrimOriginalBody::Fillet, Entity::Edge(other), mark);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::Point3;

    #[test]
    fn actual_box_selection_uses_retained_mates_and_releases_marks() {
        let mut k = Kernel::default();
        let mut target = crate::primitive::api_primitives::create_box(
            &mut k,
            Point3::new(0., 0., 0.),
            10.,
            20.,
            30.,
        )
        .unwrap();
        let edge = target.edges().ids().next().unwrap();
        let mut session = FilletSession::new(&mut k, &mut target).unwrap();
        session
            .prepare_constant_radius_edges(&mut k, &[edge], 0.5, 1, 1, 1.0)
            .unwrap();
        session.create_corners(&k, None).unwrap();
        session
            .with_filleting_context(&mut k, |session, k| {
                session.create_fillet_brep(k, None)?;
                session.shrink_fillet_faces(k)?;
                session.insert_fillet_intersection_topology(k)?;
                let editing = session.target().editing_enabled();
                let mut selected = OriginalSurfaceSelection::default();
                session.select_original_fillet_surfaces(k, &mut selected)?;
                assert_eq!(session.target().editing_enabled(), editing);
                let fillet_faces = p1::faces(session.intermediate_brep().unwrap(), None)?;
                let target_faces = p1::faces(session.target(), None)?;
                let kept_labels: Vec<_> = selected
                    .kept
                    .iter()
                    .map(|face| {
                        fillet_faces
                            .iter()
                            .position(|candidate| candidate == face)
                            .unwrap()
                    })
                    .collect();
                let deleted_labels: Vec<_> = selected
                    .deleted
                    .iter()
                    .map(|face| {
                        target_faces
                            .iter()
                            .position(|candidate| candidate == face)
                            .unwrap()
                    })
                    .collect();
                // Native same-input owning traversal, not raw arena indices.
                assert_eq!(kept_labels, [0]);
                assert_eq!(deleted_labels, [3, 4, 7, 5]);
                for face in &selected.kept {
                    session.intermediate_brep().unwrap().face(*face)?;
                }
                for face in &selected.deleted {
                    session.target().face(*face)?;
                }
                for mark in [MarkType::Mark, MarkType::Mark2, MarkType::Mark3] {
                    assert!(!k.context.is_mark_locked(mark));
                }
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn scoped_error_retains_body_qualified_marks_and_unlocks() {
        let mut k = Kernel::default();
        let mut target = crate::primitive::api_primitives::create_box(
            &mut k,
            Point3::new(0., 0., 0.),
            1.,
            1.,
            1.,
        )
        .unwrap();
        let mut fillet = crate::primitive::api_primitives::create_box(
            &mut k,
            Point3::new(2., 0., 0.),
            1.,
            1.,
            1.,
        )
        .unwrap();
        crate::topology::brep::p3::notify_serial(&mut k.context, &mut fillet).unwrap();
        let mut session = FilletSession::new(&mut k, &mut target).unwrap();
        session.objects.intermediate = Some(fillet);
        let target_face = session.objects.target.faces().ids().next().unwrap();
        let existing_key =
            topology_ref(session.objects.target_serial, EntityRef::Face(target_face)).key;
        let existing = session
            .objects
            .notification_records
            .or_default(existing_key);
        existing.set_user_index1(73);
        existing.mark(Some(&k.context), MarkType::Mark2);
        let prior_mark = existing.get_mark(MarkType::Mark2);
        let mut captured = None;
        let result: Result<()> =
            session.with_original_trim_marks(&mut k, |session, k, locks, serial| {
                let mark = locks[0].mark_type();
                assert!(k.context.is_mark_locked(mark));
                let face = session.objects.target.faces().ids().next().unwrap();
                session.objects.intermediate.as_ref().unwrap().face(face)?;
                let mut marks = TrimMarks {
                    records: &mut session.objects.notification_records,
                    fillet_serial: serial,
                    target_serial: session.objects.target_serial,
                };
                marks.mark(k, TrimOriginalBody::Fillet, Entity::Face(face), mark);
                assert!(!marks.is_marked(k, TrimOriginalBody::Target, Entity::Face(face), mark));
                captured = Some((
                    marks.key(TrimOriginalBody::Fillet, Entity::Face(face)),
                    mark,
                ));
                Err(invalid("defined continuation failure"))
            });
        assert!(result.is_err());
        let (key, mark) = captured.unwrap();
        assert!(!k.context.is_mark_locked(mark));
        assert!(session.objects.notification_records[&key].is_marked(&k.context, mark));
        let retained = &session.objects.notification_records[&existing_key];
        assert_eq!(retained.user_index1(), 73);
        assert_eq!(retained.get_mark(MarkType::Mark2), prior_mark);
    }
    #[test]
    fn foreign_context_rejects_before_lock_or_mark_mutation() {
        let mut k = Kernel::default();
        let mut foreign = Kernel::default();
        let mut target = crate::primitive::api_primitives::create_box(
            &mut k,
            Point3::new(0., 0., 0.),
            1.,
            1.,
            1.,
        )
        .unwrap();
        let mut fillet = crate::primitive::api_primitives::create_box(
            &mut foreign,
            Point3::new(2., 0., 0.),
            1.,
            1.,
            1.,
        )
        .unwrap();
        crate::topology::brep::p3::notify_serial(&mut foreign.context, &mut fillet).unwrap();
        let mut session = FilletSession::new(&mut k, &mut target).unwrap();
        session.objects.intermediate = Some(fillet);
        let count = session.objects.notification_records.len();
        let editing = session.target().editing_enabled();
        let mut selected = OriginalSurfaceSelection::default();
        assert!(session
            .select_original_fillet_surfaces(&mut k, &mut selected)
            .is_err());
        assert_eq!(session.objects.notification_records.len(), count);
        assert_eq!(session.target().editing_enabled(), editing);
        assert!(selected.kept.is_empty() && selected.deleted.is_empty());
        for mark in [MarkType::Mark, MarkType::Mark2, MarkType::Mark3] {
            assert!(!k.context.is_mark_locked(mark));
        }
    }
    #[test]
    fn qualified_deletion_list_keeps_equal_face_ids_and_error_prefix() {
        use crate::topology::Brep;
        struct RefuseDeletion<'a> {
            managed: &'a Brep,
            mark: MarkType,
        }
        impl OriginalSurfaceDeletionAccess<FilletObject> for RefuseDeletion<'_> {
            fn original_managed_faces(&self, body: FilletObject) -> Result<Vec<FilletObject>> {
                assert_eq!(body, FilletObject::IntermediateBrep);
                Ok(p1::faces(self.managed, None)?
                    .into_iter()
                    .map(|face| FilletObject::IntermediateTopology(EntityRef::Face(face)))
                    .collect())
            }
            fn original_delete_faces(
                &mut self,
                k: &mut Kernel,
                _: &mut crate::boolean::topology_intersector::p1::TopologyIntersector<FilletObject>,
                _: &[FilletObject],
            ) -> Result<()> {
                assert!(k.context.is_mark_locked(self.mark));
                Err(invalid("defined deletion callback refusal"))
            }
        }
        let mut k = Kernel::default();
        let mut target = crate::primitive::api_primitives::create_box(
            &mut k,
            Point3::new(0., 0., 0.),
            1.,
            1.,
            1.,
        )
        .unwrap();
        let mut managed = crate::primitive::api_primitives::create_box(
            &mut k,
            Point3::new(2., 0., 0.),
            1.,
            1.,
            1.,
        )
        .unwrap();
        crate::topology::brep::p3::notify_serial(&mut k.context, &mut managed).unwrap();
        let target_face = p1::faces(&target, None).unwrap()[0];
        let managed_faces = p1::faces(&managed, None).unwrap();
        assert_eq!(target_face, managed_faces[0]);
        let mut session = FilletSession::new(&mut k, &mut target).unwrap();
        session.objects.intermediate = Some(managed);
        let kept: Vec<_> = managed_faces
            .iter()
            .skip(1)
            .map(|face| FilletObject::IntermediateTopology(EntityRef::Face(*face)))
            .collect();
        let original = FilletObject::Topology(FilletBody::Target, EntityRef::Face(target_face));
        let other = FilletObject::IntermediateTopology(EntityRef::Face(managed_faces[0]));
        let mut deleted = vec![original];
        let mut mark = None;
        let result = session.with_original_trim_marks(&mut k, |session, k, locks, _| {
            let actual_mark = locks[0].mark_type();
            mark = Some(actual_mark);
            session.executive.delete_original_surface_faces(
                k,
                &mut RefuseDeletion {
                    managed: session.objects.intermediate.as_ref().unwrap(),
                    mark: actual_mark,
                },
                FilletObject::IntermediateBrep,
                &kept,
                &mut deleted,
            )
        });
        assert!(result.is_err());
        assert_eq!(deleted, [original, other]);
        assert!(!k.context.is_mark_locked(mark.unwrap()));
        assert_eq!(p1::faces(session.target(), None).unwrap().len(), 6);
        assert_eq!(
            p1::faces(session.intermediate_brep().unwrap(), None)
                .unwrap()
                .len(),
            6
        );
    }
    #[test]
    fn actual_box_deletion_retains_issued_callbacks_and_releases_mark_scope() {
        use crate::geom::{
            context::NotifyCallback,
            object::{NotifyOperation, ObjectRef},
        };
        use std::sync::{Arc, Mutex};
        #[derive(Debug)]
        struct Capture(Arc<Mutex<Vec<ObjectRef>>>);
        impl NotifyCallback for Capture {
            fn execute(
                &mut self,
                _: NotifyOperation,
                caller: ObjectRef,
                one: Option<ObjectRef>,
                two: Option<ObjectRef>,
                three: Option<ObjectRef>,
            ) -> Result<()> {
                let mut calls = self.0.lock().unwrap();
                calls.push(caller);
                calls.extend([one, two, three].into_iter().flatten());
                Ok(())
            }
        }
        let mut k = Kernel::default();
        let mut target = crate::primitive::api_primitives::create_box(
            &mut k,
            Point3::new(0., 0., 0.),
            10.,
            20.,
            30.,
        )
        .unwrap();
        let edge = target.edges().ids().next().unwrap();
        let key = target.key();
        let mut session = FilletSession::new(&mut k, &mut target).unwrap();
        session
            .prepare_constant_radius_edges(&mut k, &[edge], 0.5, 1, 1, 1.)
            .unwrap();
        session.create_corners(&k, None).unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut output = OriginalSurfaceDeletion::default();
        let expected =
            session.target_identity(EntityRef::Brep(crate::topology::BrepId::from_index(0)));
        session
            .with_filleting_context(&mut k, |session, k| {
                session.create_fillet_brep(k, None)?;
                session.shrink_fillet_faces(k)?;
                session.insert_fillet_intersection_topology(k)?;
                let faces_before = p1::faces(session.target(), None)?;
                let editing = session.target().editing_enabled();
                let outer_epoch = k.context.current_mark(MarkType::Mark);
                let nested_epoch = k.context.current_mark(MarkType::Mark2);
                k.context
                    .set_user_notify_callback(Some(Box::new(Capture(Arc::clone(&calls)))));
                let result = session.trim_and_delete_original_fillet_faces(k, &mut output);
                assert_eq!(session.target().editing_enabled(), editing);
                result?;
                assert_eq!(output.kept.len(), 1);
                assert_eq!(output.deleted.len(), 4);
                for object in &output.deleted {
                    let FilletObject::Topology(FilletBody::Target, EntityRef::Face(face)) = object
                    else {
                        panic!("unexpected deletion role");
                    };
                    assert!(faces_before.contains(face));
                    // The existing safe arena retains unlinked records; native
                    // body membership is the owning face list, not arena lookup.
                    assert!(!p1::faces(session.target(), None)?.contains(face));
                }
                assert_eq!(p1::vertices(session.target(), None)?.len(), 10);
                assert_eq!(p1::edges(session.target(), None)?.len(), 15);
                assert_eq!(p1::faces(session.target(), None)?.len(), 6);
                assert_eq!(k.context.current_mark(MarkType::Mark) - outer_epoch, 2);
                assert_eq!(k.context.current_mark(MarkType::Mark2) - nested_epoch, 28);
                assert!(!session.executive.intersector.topology_deleted);
                assert_eq!(
                    p1::faces(session.intermediate_brep().unwrap(), None)?.len(),
                    1
                );
                assert_eq!(session.target().key(), key);
                for mark in [MarkType::Mark, MarkType::Mark2, MarkType::Mark3] {
                    assert!(!k.context.is_mark_locked(mark));
                }
                Ok(())
            })
            .unwrap();
        let calls = calls.lock().unwrap();
        let bodies: Vec<_> = calls
            .iter()
            .filter(|object| object.type_code == expected.type_code)
            .collect();
        assert!(!bodies.is_empty());
        assert!(bodies.iter().all(|object| **object == expected));
        assert!(!k.context.doing_boolean());
    }
}
