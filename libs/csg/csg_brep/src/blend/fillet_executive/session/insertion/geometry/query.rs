//! Body-qualified handles for an actual owning-face query.

use super::{
    invalid, Brep, BrepId, ClassGeometry, Curve, EntityRef, FaceId, FilletBody, FilletObject,
    InsertionBody, InsertionGeometry, ObjectRef, Point3, Result, Surface, Volume,
};
use crate::intersect::curve_class::CurveClassification;
use crate::query::face::p2::{
    curve_on_classify_cached, CurveOnClassifyCache, CurveOnClassifyObjects,
};
use crate::topology::{EdgeId, EdgeuseId, LoopId, VertexId, VertexuseId};

struct FaceObjects<'view, 'bodies> {
    geometry: &'view InsertionGeometry<'bodies>,
    role: InsertionBody,
}
impl ClassGeometry<FilletObject> for FaceObjects<'_, '_> {
    fn curve(&self, object: FilletObject) -> Option<&Curve> {
        self.geometry.curve(object)
    }
    fn surface(&self, object: FilletObject) -> Option<&Surface> {
        self.geometry.surface(object)
    }
    fn volume(&self, object: FilletObject) -> Option<&Volume> {
        self.geometry.volume(object)
    }
    fn point(&self, object: FilletObject) -> Option<Point3> {
        self.geometry.point(object)
    }
    fn classification_vertex_point(&self, object: FilletObject) -> Option<Point3> {
        self.geometry.classification_vertex_point(object)
    }
    fn topology_zone_tol_3d(&self, object: FilletObject) -> Option<f64> {
        self.geometry.topology_zone_tol_3d(object)
    }
    fn vertex_face_gap(
        &self,
        vertex: FilletObject,
        surface: &Surface,
        face: Option<FilletObject>,
    ) -> Option<f64> {
        self.geometry.vertex_face_gap(vertex, surface, face)
    }
    fn topology_entity(&self, object: FilletObject) -> Option<(&Brep, EntityRef)> {
        self.geometry.topology_entity(object)
    }
    fn topology_handle(&self, entity: EntityRef) -> Option<FilletObject> {
        Some(InsertionGeometry::handle(self.role, entity))
    }
    fn topology_handle_in(&self, brep: &Brep, entity: EntityRef) -> Option<FilletObject> {
        self.geometry.topology_handle_in(brep, entity)
    }
    fn topology_intersector_mate(
        &self,
        ti: FilletObject,
        entity: FilletObject,
    ) -> Option<Option<FilletObject>> {
        self.geometry.topology_intersector_mate(ti, entity)
    }
    fn brep_id(&self, brep: &Brep) -> Option<BrepId> {
        self.geometry.brep_id(brep)
    }
    fn owner_entity(&self, owner: ObjectRef) -> Option<(&Brep, EntityRef)> {
        self.geometry.owner_entity(owner)
    }
    fn owner_of(&self, owner: ObjectRef) -> Option<ObjectRef> {
        self.geometry.owner_of(owner)
    }
}
impl CurveOnClassifyObjects<FilletObject> for FaceObjects<'_, '_> {
    fn face(&self, id: FaceId) -> FilletObject {
        InsertionGeometry::handle(self.role, EntityRef::Face(id))
    }
    fn vertex(&self, id: VertexId) -> FilletObject {
        InsertionGeometry::handle(self.role, EntityRef::Vertex(id))
    }
    fn edge(&self, id: EdgeId) -> FilletObject {
        InsertionGeometry::handle(self.role, EntityRef::Edge(id))
    }
    fn vertexuse(&self, id: VertexuseId) -> FilletObject {
        InsertionGeometry::handle(self.role, EntityRef::Vertexuse(id))
    }
    fn edgeuse(&self, id: EdgeuseId) -> FilletObject {
        InsertionGeometry::handle(self.role, EntityRef::Edgeuse(id))
    }
    fn edge_curve(&self, id: EdgeId) -> FilletObject {
        match self.role {
            InsertionBody::Target => FilletObject::Curve(FilletBody::Target, id),
            InsertionBody::Pseudo => FilletObject::Curve(FilletBody::Pseudo, id),
            InsertionBody::Intermediate => FilletObject::IntermediateCurve(id),
        }
    }
    fn trim_curve(&self, id: EdgeuseId) -> FilletObject {
        match self.role {
            InsertionBody::Target => FilletObject::UvCurve(FilletBody::Target, id),
            InsertionBody::Pseudo => FilletObject::UvCurve(FilletBody::Pseudo, id),
            InsertionBody::Intermediate => FilletObject::IntermediateUvCurve(id),
        }
    }
    fn face_surface(&self, id: FaceId) -> Option<FilletObject> {
        self.geometry
            .body(self.role)
            .face(id)
            .ok()?
            .surface
            .as_ref()?;
        Some(FilletObject::FaceSurface(self.role, id))
    }
    fn loop_id(&self, object: FilletObject) -> Option<LoopId> {
        let (role, EntityRef::Loop(id)) = InsertionGeometry::topology(object)? else {
            return None;
        };
        (role == self.role).then_some(id)
    }
}

pub(super) fn classify(
    geometry: &mut InsertionGeometry<'_>,
    k: &crate::Kernel,
    face: FilletObject,
    do_point_classify: bool,
    cc: &mut CurveClassification<FilletObject>,
) -> Result<bool> {
    let (role, EntityRef::Face(face)) =
        InsertionGeometry::topology(face).ok_or_else(|| invalid("unknown Face role"))?
    else {
        return Err(invalid("handle is not a Face"));
    };
    geometry.body(role).face(face)?;
    let mut cache = CurveOnClassifyCache::default();
    let result = {
        let objects = FaceObjects { geometry, role };
        curve_on_classify_cached(
            k,
            geometry.body(role),
            face,
            &objects,
            do_point_classify,
            cc,
            false,
            false,
            true,
            &mut cache,
        )
    };
    cache.apply_uv_trim_updates(geometry.body_mut(role))?;
    result
}

/// Legacy const query entry; owning native paths call `classify` instead.
pub(super) fn classify_readonly(
    geometry: &InsertionGeometry<'_>,
    k: &crate::Kernel,
    face: FilletObject,
    do_point_classify: bool,
    cc: &mut CurveClassification<FilletObject>,
) -> Result<bool> {
    let (role, EntityRef::Face(face)) =
        InsertionGeometry::topology(face).ok_or_else(|| invalid("unknown Face role"))?
    else {
        return Err(invalid("handle is not a Face"));
    };
    geometry.body(role).face(face)?;
    let objects = FaceObjects { geometry, role };
    crate::query::face::p2::curve_on_classify(
        k,
        geometry.body(role),
        face,
        &objects,
        do_point_classify,
        cc,
        false,
        false,
        true,
    )
}
