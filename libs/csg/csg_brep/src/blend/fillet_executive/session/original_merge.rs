//! Real surviving-edge tags and body copying within the original trim mark scope.
//!
//! Upstream: `source/SMLib/src/SmFilletExecutive.cpp`, tagging and `MergeBrep` (3480–3530).
//! Session-owned marker handles preserve the native stack scope across this component stop.

use super::original_trim::OriginalSurfaceDeletion;
use super::{
    invalid, Attribute, Behavior, FilletBody, FilletObject, FilletSession, SessionObjects,
};
use crate::blend::fillet_executive::p3::{FilletTagMergeAccess, TopoEdgeView};
use crate::blend::fillet_geom::p1::FilletEdgeType;
use crate::error::{EntityRef, Result};
use crate::geom::{attribute::ids, context::Context};
use crate::kernel::Kernel;
use crate::topology::brep::{p1, p3};

struct MergeAccess<'a, 'target>(&'a mut SessionObjects<'target>);
impl FilletTagMergeAccess<FilletObject> for MergeAccess<'_, '_> {
    fn tag_merge_edges(&self, body: FilletObject) -> Result<Vec<FilletObject>> {
        if body != FilletObject::IntermediateBrep {
            return Err(invalid("merge source is not the actual intermediate body"));
        }
        let managed = self
            .0
            .intermediate
            .as_ref()
            .ok_or_else(|| invalid("intermediate body is not allocated"))?;
        Ok(p1::edges(managed, None)?
            .into_iter()
            .map(|edge| FilletObject::IntermediateTopology(EntityRef::Edge(edge)))
            .collect())
    }
    fn tag_merge_edge(&self, object: FilletObject) -> Result<TopoEdgeView<FilletObject>> {
        let edge = self.0.mapping_edge(object)?;
        let FilletObject::Topology(FilletBody::Pseudo, EntityRef::Edge(id)) = object else {
            return Err(invalid("tag source is not actual pseudo edge"));
        };
        Ok(TopoEdgeView {
            on_edge: edge.fillet_edge_type() == FilletEdgeType::OnEdge,
            has_curve: self.0.pseudo.brep().edge(id)?.curve.is_some(),
            brep_edge: edge
                .fillet_brep_edge()
                .map(|edge| FilletObject::IntermediateTopology(EntityRef::Edge(edge))),
        })
    }
    fn tag_merge_attribute(&mut self, _k: &Kernel) -> Result<FilletObject> {
        Ok(FilletObject::Attribute(self.0.attributes.insert(
            Attribute::new(ids::FILLET_EDGES, Behavior::StandaloneReference),
        )?))
    }
    fn tag_merge_attach(&mut self, edge: FilletObject, attribute: FilletObject) -> Result<()> {
        let FilletObject::IntermediateTopology(EntityRef::Edge(edge)) = edge else {
            return Err(invalid("tag user is not actual intermediate edge"));
        };
        let FilletObject::Attribute(attribute) = attribute else {
            return Err(invalid("merge tag is not an actual attribute"));
        };
        let body = self
            .0
            .intermediate
            .as_ref()
            .ok_or_else(|| invalid("intermediate body is not allocated"))?;
        body.edge(edge)?;
        let object = self.0.intermediate_identity(EntityRef::Edge(edge))?;
        self.0.attributes.add_attribute(object.key, attribute)
    }
    fn tag_merge_bodies(
        &mut self,
        k: &mut Kernel,
        target: FilletObject,
        source: FilletObject,
    ) -> Result<()> {
        if target != FilletObject::Body(FilletBody::Target)
            || source != FilletObject::IntermediateBrep
        {
            return Err(invalid(
                "merge operands are not actual target and intermediate bodies",
            ));
        }
        Context::with_notification_context(k, |k, context| {
            let managed = self
                .0
                .intermediate
                .as_mut()
                .ok_or_else(|| invalid("intermediate body is not allocated"))?;
            let source_serial = context
                .issued_brep_serial(managed.notify_identity_slot())
                .ok_or_else(|| invalid("merge source belongs to a different Context"))?;
            let target_serial = context
                .issued_brep_serial(self.0.target.notify_identity_slot())
                .ok_or_else(|| invalid("merge target belongs to a different Context"))?;
            if target_serial != self.0.target_serial || source_serial == target_serial {
                return Err(invalid("merge operand identity is not authoritative"));
            }
            let destination_key = |entity| p3::topology_ref(target_serial, entity);
            let source_key = |entity| p3::topology_ref(source_serial, entity);
            let mut env = crate::topology::edge::NotifyEnv {
                records: &mut self.0.notification_records,
                attributes: &mut self.0.attributes,
                context: Some(context),
                key_of: &destination_key,
            };
            p3::merge_brep_with_context(
                k,
                &mut env,
                self.0.target,
                managed,
                p3::CopySource {
                    key_of: &source_key,
                    context: None,
                },
                None,
                None,
                true,
            )
        })?
    }
}

pub(super) fn destroy_tags(objects: &mut SessionObjects<'_>, tags: &[FilletObject]) -> Result<()> {
    let mut cleanup = Ok(());
    for tag in tags {
        let removed = match tag {
            FilletObject::Attribute(handle) => objects.attributes.destroy(*handle),
            _ => Err(invalid("merge tag is not an actual attribute")),
        };
        cleanup = cleanup.and(removed);
    }
    cleanup
}

impl FilletSession<'_> {
    /// Select/delete original faces, tag eligible surviving edges and copy the
    /// actual intermediate body into the target under the original mark scope.
    /// Temporary attributes remain owned by this session after success for later
    /// stitching/remapping; errors destroy them before releasing the mark lock.
    /// Stops before TI clearing, intermediate destruction and stitching.
    ///
    /// # Errors
    /// Actual selection, deletion, tagging or merge errors, retaining partial bodies
    /// and face lists. Repeating a completed merge requires a new session.
    pub fn trim_delete_and_merge_fillet_faces(
        &mut self,
        k: &mut Kernel,
        output: &mut OriginalSurfaceDeletion,
    ) -> Result<()> {
        if self.objects.merge_attributes.is_some() {
            return Err(invalid("fillet faces have already been merged"));
        }
        *output = OriginalSurfaceDeletion::default();
        let editing = self.objects.target.editing_enabled();
        self.objects.target.set_editing_enabled(true);
        let result = self.with_original_trim_marks(k, |session, k, locks, serial| {
            session.select_and_delete_with_marks(k, locks, serial, output)?;
            session.tag_and_merge_retained(k)
        });
        self.objects.target.set_editing_enabled(editing);
        result
    }

    /// Tag the eligible surviving edges and copy the intermediate body into the
    /// target, retaining the temporary tags for the post-merge tail. Errors destroy
    /// the tags. The caller holds the original trim mark scope.
    pub(super) fn tag_and_merge_retained(&mut self, k: &mut Kernel) -> Result<()> {
        if self.objects.merge_attributes.is_some() {
            return Err(invalid("fillet faces have already been merged"));
        }
        let mut tags = Vec::new();
        let merged = self.executive.tag_and_merge_fillet_faces(
            k,
            &mut MergeAccess(&mut self.objects),
            FilletObject::Body(FilletBody::Target),
            FilletObject::IntermediateBrep,
            &mut tags,
        );
        let retained = merged.and_then(|()| {
            tags.iter()
                .map(|object| match object {
                    FilletObject::Attribute(handle) => Ok(*handle),
                    _ => Err(invalid("merge tag is not an actual attribute")),
                })
                .collect::<Result<Vec<_>>>()
        });
        match retained {
            Ok(handles) => {
                self.objects.merge_attributes = Some(handles);
                Ok(())
            }
            Err(error) => {
                let _ = destroy_tags(&mut self.objects, &tags);
                Err(error)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::context::MarkType;
    use crate::math::Point3;

    #[test]
    fn actual_box_merge_copies_real_e_users_and_keeps_managed_body_alive() {
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
        let mut output = OriginalSurfaceDeletion::default();
        session
            .with_filleting_context(&mut k, |session, k| {
                session.create_fillet_brep(k, None)?;
                session.shrink_fillet_faces(k)?;
                session.insert_fillet_intersection_topology(k)?;
                assert!(session.executive.topo_edges.is_empty());
                let Some(FilletObject::Attribute(e)) = session.executive.edge_attribute else {
                    panic!("missing actual E");
                };
                let source_edges = p1::edges(session.intermediate_brep().unwrap(), None)?;
                let source_users: Vec<_> = source_edges
                    .iter()
                    .map(|edge| {
                        session
                            .intermediate_identity(EntityRef::Edge(*edge))
                            .unwrap()
                            .key
                    })
                    .collect();
                assert_eq!(session.attributes().users(e)?, source_users);
                let editing = session.target().editing_enabled();
                session.trim_delete_and_merge_fillet_faces(k, &mut output)?;
                assert_eq!(session.target().editing_enabled(), editing);
                assert_eq!(session.target().key(), key);
                assert_eq!(p1::vertices(session.target(), None)?.len(), 14);
                let target_edges = p1::edges(session.target(), None)?;
                assert_eq!(target_edges.len(), 19);
                let target_faces = p1::faces(session.target(), None)?;
                assert_eq!(target_faces.len(), 7);
                assert_eq!(
                    p1::vertices(session.intermediate_brep().unwrap(), None)?.len(),
                    4
                );
                assert_eq!(
                    p1::edges(session.intermediate_brep().unwrap(), None)?.len(),
                    4
                );
                let source_face = p1::faces(session.intermediate_brep().unwrap(), None)?[0];
                let target_face = *target_faces.last().unwrap();
                let source = session
                    .intermediate_brep()
                    .unwrap()
                    .face(source_face)?
                    .surface
                    .as_ref()
                    .unwrap();
                let copied = session
                    .target()
                    .face(target_face)?
                    .surface
                    .as_ref()
                    .unwrap();
                assert_eq!(source.type_code(), 15012);
                assert_eq!(copied.type_code(), 15012);
                assert!(!std::ptr::eq(source, copied));
                assert_eq!(
                    source.owner(),
                    Some(session.intermediate_identity(EntityRef::Face(source_face))?)
                );
                assert_eq!(
                    copied.owner(),
                    Some(session.target_identity(EntityRef::Face(target_face)))
                );
                let mut expected = source_users;
                expected.extend(
                    target_edges
                        .iter()
                        .skip(15)
                        .map(|edge| session.target_identity(EntityRef::Edge(*edge)).key),
                );
                assert_eq!(session.attributes().users(e)?, expected);
                assert_eq!(
                    session.objects.merge_attributes.as_deref(),
                    Some([].as_slice())
                );
                assert_eq!(
                    session.executive.intersector.primary_brep(),
                    Some(FilletObject::Body(FilletBody::Target))
                );
                assert_eq!(
                    session.executive.intersector.other_brep(),
                    Some(FilletObject::IntermediateBrep)
                );
                for mark in [MarkType::Mark, MarkType::Mark2, MarkType::Mark3] {
                    assert!(!k.context.is_mark_locked(mark));
                }
                let epoch = k.context.current_mark(MarkType::Mark);
                assert!(session
                    .trim_delete_and_merge_fillet_faces(k, &mut output)
                    .is_err());
                assert_eq!(k.context.current_mark(MarkType::Mark), epoch);
                assert_eq!(session.attributes().users(e)?, expected);
                Ok(())
            })
            .unwrap();
        assert!(!k.context.doing_boolean());
    }
}
