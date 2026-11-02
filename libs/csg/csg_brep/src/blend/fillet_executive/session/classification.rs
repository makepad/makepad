//! Borrowed target/pseudo geometry for the executive's attached rail classifications.

use crate::blend::fillet_classification::{FilletBody, FilletClassObject};
use crate::curve::curve::Curve;
use crate::error::EntityRef;
use crate::geom::object::ObjectRef;
use crate::intersect::point_class::ClassGeometry;
use crate::math::Point3;
use crate::query::face::p2::CurveOnClassifyObjects;
use crate::surface::surface::Surface;
use crate::surface::volume::Volume;
use crate::topology::{Brep, BrepId, EdgeId, EdgeuseId, FaceId, LoopId, VertexId, VertexuseId};

/// Each body is borrowed from the same live session during the classification loan.
/// No indexed/legacy handles are accepted by this production provider.
/// `second` is a surface/surface fillet's second base B-rep when it is not the
/// target; `classified` names the body whose face is being classified (the target
/// outside a second rail's classification).
pub(super) struct Objects<'a> {
    pub target: &'a Brep,
    pub pseudo: &'a Brep,
    pub second: Option<&'a Brep>,
    pub classified: FilletBody,
}

impl<'a> Objects<'a> {
    /// The target and pseudo bodies, classifying a target face.
    pub(super) fn new(target: &'a Brep, pseudo: &'a Brep) -> Self {
        Self {
            target,
            pseudo,
            second: None,
            classified: FilletBody::Target,
        }
    }

    fn body(&self, body: FilletBody) -> &Brep {
        match body {
            FilletBody::Target => self.target,
            FilletBody::Pseudo => self.pseudo,
            // Second-base handles are only issued with a second base present.
            FilletBody::SecondBase => self.second.unwrap_or(self.target),
        }
    }

    fn body_of(&self, brep: &Brep) -> Option<FilletBody> {
        if std::ptr::eq(brep, self.target) {
            Some(FilletBody::Target)
        } else if std::ptr::eq(brep, self.pseudo) {
            Some(FilletBody::Pseudo)
        } else if self.second.is_some_and(|second| std::ptr::eq(brep, second)) {
            Some(FilletBody::SecondBase)
        } else {
            None
        }
    }
}

impl ClassGeometry<FilletClassObject> for Objects<'_> {
    fn curve(&self, object: FilletClassObject) -> Option<&Curve> {
        match object {
            FilletClassObject::Curve(body, edge) => self.body(body).edge(edge).ok()?.curve.as_ref(),
            FilletClassObject::TrimCurve(body, eu) => {
                self.body(body).edgeuse(eu).ok()?.uv_trim_curve.as_ref()
            }
            _ => None,
        }
    }
    fn surface(&self, object: FilletClassObject) -> Option<&Surface> {
        let FilletClassObject::Surface(body, face) = object else {
            return None;
        };
        self.body(body).face(face).ok()?.surface.as_ref()
    }
    fn volume(&self, _: FilletClassObject) -> Option<&Volume> {
        None
    }
    fn point(&self, object: FilletClassObject) -> Option<Point3> {
        let FilletClassObject::Topology(body, EntityRef::Vertex(vertex)) = object else {
            return None;
        };
        Some(self.body(body).vertex(vertex).ok()?.point)
    }
    fn classification_vertex_point(&self, object: FilletClassObject) -> Option<Point3> {
        self.point(object)
    }
    fn topology_zone_tol_3d(&self, object: FilletClassObject) -> Option<f64> {
        let FilletClassObject::Topology(body, entity) = object else {
            return None;
        };
        let brep = self.body(body);
        match entity {
            EntityRef::Vertex(id) => Some(brep.vertex(id).ok()?.tolerance),
            EntityRef::Edge(id) => Some(brep.edge(id).ok()?.tolerance),
            EntityRef::Face(id) => Some(brep.face(id).ok()?.tolerance),
            EntityRef::Edgeuse(id) => Some(brep.edge(brep.edgeuse(id).ok()?.edge?).ok()?.tolerance),
            _ => None,
        }
    }
    fn topology_entity(&self, object: FilletClassObject) -> Option<(&Brep, EntityRef)> {
        let FilletClassObject::Topology(body, entity) = object else {
            return None;
        };
        Some((self.body(body), entity))
    }
    fn topology_handle(&self, entity: EntityRef) -> Option<FilletClassObject> {
        Some(FilletClassObject::Topology(self.classified, entity))
    }
    fn topology_handle_in(&self, brep: &Brep, entity: EntityRef) -> Option<FilletClassObject> {
        Some(FilletClassObject::Topology(self.body_of(brep)?, entity))
    }
    fn brep_id(&self, brep: &Brep) -> Option<BrepId> {
        self.body_of(brep)?;
        // These are the actual distinct serials issued by the kernel constructor,
        // not local topology indices or synthesized offsets between body arenas.
        Some(BrepId::from_index(
            u32::try_from(brep.notify_serial()?).ok()?,
        ))
    }
    fn owner_entity(&self, owner: ObjectRef) -> Option<(&Brep, EntityRef)> {
        for brep in [Some(self.target), Some(self.pseudo), self.second]
            .into_iter()
            .flatten()
        {
            let Some(serial) = brep.notify_serial() else {
                continue;
            };
            let Some(entity) = crate::topology::brep::p3::topology_entity(serial, owner.key) else {
                continue;
            };
            if crate::topology::brep::p3::topology_ref(serial, entity) == owner {
                return Some((brep, entity));
            }
        }
        None
    }
}

impl CurveOnClassifyObjects<FilletClassObject> for Objects<'_> {
    fn face(&self, id: FaceId) -> FilletClassObject {
        FilletClassObject::Topology(self.classified, EntityRef::Face(id))
    }
    fn vertex(&self, id: VertexId) -> FilletClassObject {
        FilletClassObject::Topology(self.classified, EntityRef::Vertex(id))
    }
    fn edge(&self, id: EdgeId) -> FilletClassObject {
        FilletClassObject::Topology(self.classified, EntityRef::Edge(id))
    }
    fn vertexuse(&self, id: VertexuseId) -> FilletClassObject {
        FilletClassObject::Topology(self.classified, EntityRef::Vertexuse(id))
    }
    fn edgeuse(&self, id: EdgeuseId) -> FilletClassObject {
        FilletClassObject::Topology(self.classified, EntityRef::Edgeuse(id))
    }
    fn edge_curve(&self, id: EdgeId) -> FilletClassObject {
        FilletClassObject::Curve(self.classified, id)
    }
    fn trim_curve(&self, id: EdgeuseId) -> FilletClassObject {
        FilletClassObject::TrimCurve(self.classified, id)
    }
    fn face_surface(&self, id: FaceId) -> Option<FilletClassObject> {
        self.body(self.classified).face(id).ok()?.surface.as_ref()?;
        Some(FilletClassObject::Surface(self.classified, id))
    }
    fn loop_id(&self, object: FilletClassObject) -> Option<LoopId> {
        match object {
            FilletClassObject::Topology(body, EntityRef::Loop(id)) if body == self.classified => {
                Some(id)
            }
            _ => None,
        }
    }
}

/// Live retained corner resolution; geometry borrows exist only inside `classify`.
pub(super) struct Context<'a> {
    pub corners: &'a [crate::blend::fillet_corner::p3::FilletCorner],
}

impl crate::blend::fillet_solver::p5::RailClassificationContext for Context<'_> {
    fn rail_curve(&self, edge: EdgeId) -> FilletClassObject {
        FilletClassObject::Curve(FilletBody::Pseudo, edge)
    }
    fn rail_mate_uv_curve(&self, pseudo: &Brep, edgeuse: EdgeuseId) -> Option<FilletClassObject> {
        pseudo.edgeuse(edgeuse).ok()?.uv_trim_curve.as_ref()?;
        Some(FilletClassObject::TrimCurve(FilletBody::Pseudo, edgeuse))
    }
    fn classify(
        &mut self,
        k: &crate::kernel::Kernel,
        target: &mut Brep,
        pseudo: &Brep,
        face: FaceId,
        cc: &mut crate::intersect::curve_class::p1::CurveClassification<FilletClassObject>,
    ) -> crate::error::Result<()> {
        let mut cache = crate::query::face::p2::CurveOnClassifyCache::default();
        let result = {
            let objects = Objects::new(target, pseudo);
            crate::query::face::p2::curve_on_classify_cached(
                k, target, face, &objects, true, cc, true, false, true, &mut cache,
            )
        };
        // Native const queries retain their lazy gap/UV edits, including mutations
        // made before a failed classification. Use the shared query synchronization.
        cache.apply_uv_trim_updates(target)?;
        result.map(|_| ())
    }
    fn corner_type(
        &self,
        id: crate::blend::fillet_geom::p1::FilletCornerId,
    ) -> crate::error::Result<crate::blend::fillet_geom::p1::FilletCornerType> {
        self.corners
            .get(id.0 as usize)
            .map(crate::blend::fillet_corner::p3::FilletCorner::corner_type)
            .ok_or_else(|| super::invalid("rollover corner is not live in this session"))
    }
    fn corners(&self) -> crate::blend::fillet_geom::p1::CornerLookup<'_> {
        crate::blend::fillet_geom::p1::CornerLookup::table(self.corners)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::Kernel;
    use crate::primitive::api_primitives::create_box;

    #[test]
    fn equal_local_ids_keep_actual_body_and_geometry_identity() {
        let mut k = Kernel::default();
        let target = create_box(&mut k, Point3::new(0.0, 0.0, 0.0), 10.0, 20.0, 30.0).unwrap();
        let pseudo = create_box(&mut k, Point3::new(100.0, 200.0, 300.0), 4.0, 5.0, 6.0).unwrap();
        let target_edge = target.edges().ids().next().unwrap();
        let pseudo_edge = pseudo.edges().ids().next().unwrap();
        assert_eq!(target_edge, pseudo_edge);
        let objects = Objects::new(&target, &pseudo);
        let source = FilletClassObject::Curve(FilletBody::Pseudo, pseudo_edge);
        let boundary = objects.edge_curve(target_edge);
        assert_ne!(source, boundary);
        assert!(std::ptr::eq(
            objects.curve(source).unwrap(),
            pseudo.edge(pseudo_edge).unwrap().curve.as_ref().unwrap()
        ));
        assert!(std::ptr::eq(
            objects.curve(boundary).unwrap(),
            target.edge(target_edge).unwrap().curve.as_ref().unwrap()
        ));
        assert!(!std::ptr::eq(
            objects.curve(source).unwrap(),
            objects.curve(boundary).unwrap()
        ));
        assert!(objects
            .curve(FilletClassObject::Legacy(EntityRef::Edge(target_edge)))
            .is_none());
        let target_entity = EntityRef::Edge(target_edge);
        let scoped = objects.topology_handle_in(&pseudo, target_entity).unwrap();
        assert_eq!(
            scoped,
            FilletClassObject::Topology(FilletBody::Pseudo, target_entity)
        );
        assert!(std::ptr::eq(
            objects.topology_entity(scoped).unwrap().0,
            &raw const pseudo
        ));
        assert_ne!(objects.brep_id(&target), objects.brep_id(&pseudo));
        let actual_owner = target
            .edge(target_edge)
            .unwrap()
            .curve
            .as_ref()
            .unwrap()
            .owner()
            .unwrap();
        // Primitive construction retains its supplied producer key. This provider
        // does not infer a canonical body identity from that opaque key.
        assert!(objects.owner_entity(actual_owner).is_none());
        let canonical =
            crate::topology::brep::p3::topology_ref(target.notify_serial().unwrap(), target_entity);
        let (body, entity) = objects.owner_entity(canonical).unwrap();
        assert!(std::ptr::eq(body, &raw const target));
        assert_eq!(entity, target_entity);
        let foreign = create_box(&mut k, Point3::new(-10.0, 0.0, 0.0), 1.0, 1.0, 1.0).unwrap();
        let foreign_edge = foreign.edges().ids().next().unwrap();
        let foreign_owner = foreign
            .edge(foreign_edge)
            .unwrap()
            .curve
            .as_ref()
            .unwrap()
            .owner()
            .unwrap();
        assert!(objects.owner_entity(foreign_owner).is_none());
        let foreign_canonical = crate::topology::brep::p3::topology_ref(
            foreign.notify_serial().unwrap(),
            EntityRef::Edge(foreign_edge),
        );
        assert!(objects.owner_entity(foreign_canonical).is_none());
        assert!(objects
            .topology_handle_in(&foreign, EntityRef::Edge(foreign_edge))
            .is_none());
    }
    #[test]
    fn actual_box_rail_crossing_retains_concrete_solver_virtuals() {
        use super::super::{live_solver_ops, FilletObject, FilletSession};
        use crate::blend::fillet_geom::p1::{FilStatus, FilletVertexType};
        use crate::blend::fillet_solver::p3::{calc_rail_x_edge, calc_rail_x_edge_with_ops};
        use crate::topology::edgeuse::p1 as eu;

        let mut k = Kernel::default();
        let mut target = create_box(&mut k, Point3::ZERO, 10., 20., 30.).unwrap();
        let edge = target.edges().ids().next().unwrap();
        let mut session = FilletSession::new(&mut k, &mut target).unwrap();
        session
            .prepare_constant_radius_edges(&mut k, &[edge], 0.5, 1, 1, 1.)
            .unwrap();
        session.create_corners(&k, None).unwrap();
        session.process_smooth_edge_sequences(&mut k).unwrap();
        session.create_corner_vertices(&mut k).unwrap();
        session.adjust_corner_topology(&mut k).unwrap();
        session.calculate_solver_geometry(&mut k).unwrap();
        let FilletObject::Solver(id) = session.executive.solvers[0] else {
            panic!("actual solver")
        };
        let objects = &mut session.objects;
        let vertex = objects.corners[0].vertices[0];
        assert_eq!(
            objects
                .pseudo
                .fillet_vertex(vertex)
                .unwrap()
                .fillet_vertex_type(),
            FilletVertexType::RailXEdgeuse
        );
        let Some(EntityRef::Edgeuse(side_use)) = objects
            .pseudo
            .fillet_vertex(vertex)
            .unwrap()
            .point_class_object()
        else {
            panic!("actual side use")
        };
        let side = eu::edge(objects.target, side_use).unwrap().unwrap();
        let solver = objects.solvers[id.0 as usize].as_mut().unwrap();
        let geom = solver.base().first_fillet_geom().unwrap();
        let which = solver
            .base()
            .find_index_of_rail_x_side_edgeuse(objects.target, side_use)
            .unwrap();
        let rail = objects.pseudo.geom(geom).unwrap().rail(which).unwrap();
        let expected = objects.pseudo.vertex_point(vertex).unwrap();
        let base = calc_rail_x_edge(
            solver.base_mut(),
            &k,
            objects.target,
            &mut objects.pseudo,
            vertex,
            geom,
            which,
            rail,
            side,
            None,
        )
        .unwrap_err();
        assert!(
            matches!(
                base,
                crate::error::SolidError::Upstream {
                    code: 1001,
                    operation: "blend::fillet_solver::p1",
                    ..
                }
            ),
            "{base:?}"
        );
        objects.pseudo.brep_mut().vertex_mut(vertex).unwrap().point = Point3::UNDEF;
        objects
            .pseudo
            .fillet_vertex_mut(vertex)
            .unwrap()
            .set_status(FilStatus::Unprocessed);
        calc_rail_x_edge_with_ops(
            live_solver_ops(solver),
            &k,
            objects.target,
            &mut objects.pseudo,
            vertex,
            geom,
            which,
            rail,
            side,
            None,
        )
        .unwrap();
        assert_eq!(
            objects.pseudo.fillet_vertex(vertex).unwrap().status(),
            FilStatus::Processed
        );
        let actual = objects.pseudo.vertex_point(vertex).unwrap();
        assert!(actual.distance(expected) < 1e-9, "{actual:?} {expected:?}");
    }
}
