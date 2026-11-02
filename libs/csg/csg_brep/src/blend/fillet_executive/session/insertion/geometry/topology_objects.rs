//! Topology-solver geometry access over the retained insertion objects.
//!
//! Cache trees need a body-qualified storage authority, not raw arena IDs. Their
//! existing unsupported default remains until that authority is supplied.
//!
//! Owners (`SmCurve::GetEdge`, `SmSurface::GetFace`, `SmSurface::GetOwner`) are
//! [`ObjectRef`]s resolved against the insertion objects. `GetEdge` walks
//! [`Curve::edge`]: a generated curve's owner is the retained curve with that issued
//! identity (an edge, edge-use or operation-owned curve), an edge use's owner is its
//! edge (`SmEdgeuse::GetEdge`). An edge use is named by its body-qualified identity,
//! or by its body's naming scheme (`SetUVTrimCurve`'s owner) within the body of the
//! curve being walked. A link no insertion object resolves to one object, and an edge
//! or surface owner no insertion body holds, have no upstream counterpart (the owners
//! are the bodies' own topology and curves) and are `InvalidInput`. The surfaces are
//! face-slot surfaces, owned by their own face, so `GetFace` and `GetOwner` stop at a
//! face as upstream does.

use super::{
    insertion_role, invalid, ClassGeometry, Curve, EntityRef, FilletObject, InsertionBody,
    InsertionGeometry, Result, Surface,
};
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::object::{ObjectRef, ShapeMember};
use crate::intersect::topology_solver::p1::TopologyObjects;
use crate::math::Point3;
use crate::topology::brep::p3::topology_ref;
use crate::topology::{Brep, FaceId};
use crate::SolidError;

const ROLES: [InsertionBody; 3] = [
    InsertionBody::Target,
    InsertionBody::Pseudo,
    InsertionBody::Intermediate,
];

impl InsertionGeometry<'_> {
    /// The body that holds a retained curve handle (`None` for an operation-owned copy).
    fn curve_scope(object: FilletObject) -> Option<InsertionBody> {
        match object {
            FilletObject::Curve(body, _) | FilletObject::UvCurve(body, _) => insertion_role(body),
            FilletObject::IntermediateCurve(_) | FilletObject::IntermediateUvCurve(_) => {
                Some(InsertionBody::Intermediate)
            }
            _ => None,
        }
    }
    /// The owner of one intermediate link of `SmCurve::GetEdge`'s walk: a generated
    /// curve's `GetOwner()` or an edge use's edge. `scope` is the body of the object
    /// last walked and becomes that of the link.
    /// # Errors
    /// `InvalidInput` when no insertion object, or more than one, has the link's
    /// identity.
    fn owner_link(
        &self,
        link: ObjectRef,
        scope: &mut Option<InsertionBody>,
    ) -> Result<Option<ObjectRef>> {
        let unresolved = || invalid("curve owner outside the insertion objects");
        let mut found: Option<(Option<InsertionBody>, Option<ObjectRef>)> = None;
        let mut take = |at: Option<InsertionBody>, owner: Option<ObjectRef>| {
            let ambiguous = found.is_some();
            found = Some((at, owner));
            !ambiguous
        };
        if crate::curve::curve::p2::is_curve_class(link.type_code) {
            let named = |curve: &Curve| {
                curve.notify_key() == Some(link.key) && curve.type_code() == link.type_code
            };
            for role in ROLES {
                let body = self.body(role);
                let edge_curves = body.edges().iter().filter_map(|(_, e)| e.curve.as_ref());
                let uv_curves = body
                    .edgeuses()
                    .iter()
                    .filter_map(|(_, eu)| eu.uv_trim_curve.as_ref());
                for curve in edge_curves.chain(uv_curves).filter(|c| named(c)) {
                    if !take(Some(role), curve.owner()) {
                        return Err(unresolved());
                    }
                }
            }
            for curve in self
                .curves
                .iter()
                .flat_map(|c| c.live())
                .filter(|c| named(c))
            {
                if !take(None, curve.owner()) {
                    return Err(unresolved());
                }
            }
        } else if link.type_code == crate::topology::loopuse::EDGEUSE_TYPE {
            for role in ROLES {
                let body = self.body(role);
                let serial = body.notify_serial().ok_or_else(unresolved)?;
                for (eu, record) in body.edgeuses().iter() {
                    let canonical = topology_ref(serial, EntityRef::Edgeuse(eu)) == link;
                    let named = *scope == Some(role)
                        && crate::topology::edgeuse::p1::owner_ref_in(body, eu) == Some(link);
                    if (canonical || named)
                        && !take(
                            Some(role),
                            record
                                .edge
                                .map(|edge| topology_ref(serial, EntityRef::Edge(edge))),
                        )
                    {
                        return Err(unresolved());
                    }
                }
            }
        }
        let (at, owner) = found.ok_or_else(unresolved)?;
        *scope = at;
        Ok(owner)
    }
}

impl TopologyObjects<FilletObject> for InsertionGeometry<'_> {
    fn curve(&self, object: FilletObject) -> Result<&Curve> {
        ClassGeometry::curve(self, object).ok_or_else(|| invalid("not an actual Curve handle"))
    }
    fn analytic_curve_handle(&self, _curve: FilletObject) -> Option<FilletObject> {
        // No independent replacement Curve is installed in the operation's store.
        None
    }
    fn vertex_point(&self, vertex: FilletObject) -> Result<Point3> {
        ClassGeometry::point(self, vertex).ok_or_else(|| invalid("not an actual Vertex handle"))
    }
    fn vertex_tolerance(&self, vertex: FilletObject) -> Result<f64> {
        let (role, EntityRef::Vertex(vertex)) =
            Self::topology(vertex).ok_or_else(|| invalid("unknown Vertex role"))?
        else {
            return Err(invalid("handle is not a Vertex"));
        };
        crate::topology::vertex::tolerance(self.body(role), vertex)
    }
    fn curve_edge(&self, object: FilletObject) -> Result<Option<FilletObject>> {
        let curve = TopologyObjects::curve(self, object)?;
        let Some(owner) = curve.owner() else {
            return Ok(None);
        };
        if let Some((role, entity @ EntityRef::Edge(_))) = self.owner_binding(owner) {
            return Ok(Some(Self::handle(role, entity)));
        }
        // SmCurve::GetEdge: iter nested owned curves (GenCurves) and edge uses (UV
        // trim curves) until a curve owned by an Edge is found
        let mut scope = Self::curve_scope(object);
        let mut failure = None;
        let edge = curve.edge(|link| match self.owner_link(link, &mut scope) {
            Ok(next) => next,
            Err(error) => {
                failure = Some(error);
                None
            }
        })?;
        if let Some(error) = failure {
            return Err(error);
        }
        let Some(edge) = edge else {
            return Ok(None);
        };
        match self.owner_binding(edge) {
            Some((role, entity @ EntityRef::Edge(_))) => Ok(Some(Self::handle(role, entity))),
            _ => Err(invalid("curve owner Edge outside the insertion bodies")),
        }
    }
    fn edge_curve(&self, edge: FilletObject) -> Result<Option<FilletObject>> {
        InsertionGeometry::edge_curve(self, edge)
            .ok_or_else(|| invalid("not an actual Edge handle"))
    }
    fn edge_interval(&self, edge: FilletObject) -> Result<Extent1d> {
        InsertionGeometry::edge_interval(self, edge)
            .ok_or_else(|| invalid("not an actual Edge interval"))
    }
    fn edge_tolerance(&self, edge: FilletObject) -> Result<f64> {
        let (role, EntityRef::Edge(edge)) =
            Self::topology(edge).ok_or_else(|| invalid("unknown Edge role"))?
        else {
            return Err(invalid("handle is not an Edge"));
        };
        crate::topology::edge::tolerance(self.body(role), edge)
    }
    fn surface_face(&self, surface: FilletObject) -> Result<Option<FilletObject>> {
        if let FilletObject::Surface(binding) = surface {
            return InsertionGeometry::surface_face(self, binding);
        }
        let surface = TopologyObjects::surface(self, surface)?;
        self.face_slot_surface_face(surface)
    }
    fn face_surface(&self, face: FilletObject) -> Result<FilletObject> {
        InsertionGeometry::face_surface(self, face)
            .ok_or_else(|| invalid("not an actual Face handle"))?
            .ok_or_else(|| SolidError::null_pointer(super::OP))
    }
    fn surface(&self, object: FilletObject) -> Result<&Surface> {
        if let FilletObject::Surface(binding) = object {
            return self.bound_surface(binding);
        }
        ClassGeometry::surface(self, object).ok_or_else(|| invalid("not an actual Surface handle"))
    }
    fn face_uv_domain(&self, face: FilletObject) -> Result<Extent2d> {
        InsertionGeometry::face_uv_domain(self, face)
            .ok_or_else(|| invalid("not an actual Face domain"))
    }
    fn face_tolerance(&self, face: FilletObject) -> Result<f64> {
        let (body, face) = TopologyObjects::face_entity(self, face)
            .ok_or_else(|| invalid("not an actual Face handle"))?;
        crate::topology::face::p2::tolerance(body, face)
    }
    fn face_entity(&self, face: FilletObject) -> Option<(&Brep, FaceId)> {
        let (role, EntityRef::Face(face)) = Self::topology(face)? else {
            return None;
        };
        let body = self.body(role);
        body.face(face).ok()?;
        Some((body, face))
    }
    fn surface_owner(&self, object: FilletObject) -> Result<Option<FilletObject>> {
        let surface = TopologyObjects::surface(self, object)?;
        let Some(owner) = surface.owner() else {
            return Ok(None);
        };
        self.owner_binding(owner)
            .map(|(role, entity)| Some(Self::handle(role, entity)))
            .ok_or_else(|| invalid("face-slot Surface owner outside the insertion bodies"))
    }
    fn object_zone_tol_3d(&self, k: &crate::Kernel, object: FilletObject) -> Result<f64> {
        if let Some((role, EntityRef::Brep(_))) = Self::topology(object) {
            return Ok(self.body(role).tolerance());
        }
        crate::intersect::point_class::object_zone_tol_3d(k, self, Some(object))
    }
    fn shape_member(&self, _object: FilletObject) -> ShapeMember {
        // ShapeMember has no body role. No body-qualified tree is installed yet.
        ShapeMember::Other
    }
}

#[cfg(test)]
mod tests {
    use super::super::InsertionCurves;
    use super::*;
    use crate::blend::fillet_classification::FilletBody;
    use crate::blend::fillet_geom::p1::FilletBrep;
    use crate::curve::line::Line;
    use crate::geom::attribute::ObjectKey;
    use crate::primitive::api_primitives::create_box;
    use crate::topology::NamingScheme;
    use crate::Kernel;

    fn box_at(k: &mut Kernel, x: f64) -> Brep {
        let mut b = create_box(k, Point3::new(x, 0.0, 0.0), 10.0, 20.0, 30.0).unwrap();
        crate::topology::brep::p3::notify_serial(&mut k.context, &mut b).unwrap();
        b
    }
    fn line_owned_by(owner: Option<ObjectRef>) -> Curve {
        let mut curve = Curve::from(Line::from_points(
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
            3,
        ));
        let _ = curve.set_owner(owner, None);
        curve
    }

    #[test]
    fn curve_edge_walks_generated_curve_and_edgeuse_owners() {
        let mut k = Kernel::default();
        let mut target = box_at(&mut k, 0.0);
        let mut intermediate = box_at(&mut k, 100.0);
        let mut pseudo = FilletBrep::new();
        *pseudo.brep_mut() = box_at(&mut k, 200.0);
        target.set_naming(NamingScheme::Notify);
        intermediate.set_naming(NamingScheme::Notify);
        let serial = target.notify_serial().unwrap();
        let edge = target
            .edges()
            .ids()
            .find(|&e| target.edge(e).unwrap().curve.is_some())
            .unwrap();
        let (eu, eu_edge) = target
            .edgeuses()
            .iter()
            .find_map(|(eu, r)| r.edge.map(|e| (eu, e)))
            .unwrap();
        let edge_ref = topology_ref(serial, EntityRef::Edge(edge));
        let eu_ref = topology_ref(serial, EntityRef::Edgeuse(eu));
        let face = target.faces().ids().next().unwrap();
        let face_ref = topology_ref(serial, EntityRef::Face(face));
        let edge_handle = FilletObject::Topology(FilletBody::Target, EntityRef::Edge(edge));
        let eu_edge_handle = FilletObject::Topology(FilletBody::Target, EntityRef::Edge(eu_edge));

        // Operation-owned copies: A is owned by the edge use, B by A (a GenCurve
        // chain), C by the edge.
        let mut curves = InsertionCurves::default();
        let a = curves
            .insert(&mut k.context, line_owned_by(Some(eu_ref)))
            .unwrap();
        let a_curve = curves.get(a).unwrap();
        let a_ref = ObjectRef {
            type_code: a_curve.type_code(),
            key: a_curve.notify_key().unwrap(),
        };
        let b = curves
            .insert(&mut k.context, line_owned_by(Some(a_ref)))
            .unwrap();
        let c = curves
            .insert(&mut k.context, line_owned_by(Some(edge_ref)))
            .unwrap();
        let dangling = ObjectRef {
            type_code: a_ref.type_code,
            key: ObjectKey(u64::MAX),
        };
        let lost = curves
            .insert(&mut k.context, line_owned_by(Some(dangling)))
            .unwrap();

        let mut view = InsertionGeometry::new(&mut target, &mut pseudo, &mut intermediate).unwrap();
        view.curves = Some(&mut curves);
        let curve_edge =
            |view: &InsertionGeometry<'_>, object| TopologyObjects::curve_edge(view, object);
        // Owned by an edge.
        assert_eq!(
            curve_edge(&view, FilletObject::InsertionCurve(c)).unwrap(),
            Some(edge_handle)
        );
        // Owned by an edge use, whose owner is its edge.
        assert_eq!(
            curve_edge(&view, FilletObject::InsertionCurve(a)).unwrap(),
            Some(eu_edge_handle)
        );
        // Owned by a curve owned by an edge use.
        assert_eq!(
            curve_edge(&view, FilletObject::InsertionCurve(b)).unwrap(),
            Some(eu_edge_handle)
        );
        // A UV trim curve named by its B-rep's naming scheme (`SetUVTrimCurve`): the
        // intermediate box has an edge use of the same index and notify identity, so
        // the name resolves only in the walked curve's own body.
        let named = crate::topology::edgeuse::p1::owner_ref(NamingScheme::Notify, eu);
        assert!(named.is_some());
        assert_ne!(named, Some(eu_ref));
        view.target.edgeuse_mut(eu).unwrap().uv_trim_curve = Some(line_owned_by(named));
        assert_eq!(
            curve_edge(&view, FilletObject::UvCurve(FilletBody::Target, eu)).unwrap(),
            Some(eu_edge_handle)
        );
        view.intermediate.edgeuse_mut(eu).unwrap().uv_trim_curve = Some(line_owned_by(named));
        assert_eq!(
            curve_edge(&view, FilletObject::IntermediateUvCurve(eu)).unwrap(),
            Some(FilletObject::IntermediateTopology(EntityRef::Edge(
                view.intermediate.edgeuse(eu).unwrap().edge.unwrap()
            )))
        );
        // With the canonical identity the UV trim curve resolves from any body.
        view.target.edgeuse_mut(eu).unwrap().uv_trim_curve = Some(line_owned_by(Some(eu_ref)));
        assert_eq!(
            curve_edge(&view, FilletObject::UvCurve(FilletBody::Target, eu)).unwrap(),
            Some(eu_edge_handle)
        );
        // Not owned, or owned by something that is not an edge, curve or edge use
        // (the walk stops): NULL.
        view.target.edge_mut(edge).unwrap().curve = Some(line_owned_by(Some(face_ref)));
        let edge_curve = FilletObject::Curve(FilletBody::Target, edge);
        assert_eq!(curve_edge(&view, edge_curve).unwrap(), None);
        view.target.edge_mut(edge).unwrap().curve = Some(line_owned_by(None));
        assert_eq!(curve_edge(&view, edge_curve).unwrap(), None);
        // A curve link no insertion object holds, and an edge no insertion body
        // holds, are broken handles, not missing ports.
        let error = curve_edge(&view, FilletObject::InsertionCurve(lost)).unwrap_err();
        assert!(!error.is_unported());
        let foreign_edge = ObjectRef {
            type_code: edge_ref.type_code,
            key: ObjectKey(u64::MAX),
        };
        view.target.edge_mut(edge).unwrap().curve = Some(line_owned_by(Some(foreign_edge)));
        let error = curve_edge(&view, edge_curve).unwrap_err();
        assert!(!error.is_unported());
    }

    #[test]
    fn face_surface_owner_queries_follow_get_face_and_get_owner() {
        let mut k = Kernel::default();
        let mut target = box_at(&mut k, 0.0);
        let mut intermediate = box_at(&mut k, 100.0);
        let mut pseudo = FilletBrep::new();
        *pseudo.brep_mut() = box_at(&mut k, 200.0);
        let serial = intermediate.notify_serial().unwrap();
        let face = intermediate.faces().ids().next().unwrap();
        let vertex = intermediate.vertices().ids().next().unwrap();
        let mut view = InsertionGeometry::new(&mut target, &mut pseudo, &mut intermediate).unwrap();
        let surface = FilletObject::FaceSurface(InsertionBody::Intermediate, face);
        let face_handle = FilletObject::IntermediateTopology(EntityRef::Face(face));
        let vertex_handle = FilletObject::IntermediateTopology(EntityRef::Vertex(vertex));
        let set_owner = |view: &mut InsertionGeometry<'_>, owner| {
            view.intermediate
                .face_mut(face)
                .unwrap()
                .surface
                .as_mut()
                .unwrap()
                .set_owner(owner);
        };
        set_owner(&mut view, Some(topology_ref(serial, EntityRef::Face(face))));
        assert_eq!(
            TopologyObjects::surface_face(&view, surface).unwrap(),
            Some(face_handle)
        );
        assert_eq!(
            TopologyObjects::surface_owner(&view, surface).unwrap(),
            Some(face_handle)
        );
        // An owner of another kind: GetFace stops, GetOwner returns it.
        set_owner(
            &mut view,
            Some(topology_ref(serial, EntityRef::Vertex(vertex))),
        );
        assert_eq!(TopologyObjects::surface_face(&view, surface).unwrap(), None);
        assert_eq!(
            TopologyObjects::surface_owner(&view, surface).unwrap(),
            Some(vertex_handle)
        );
        set_owner(&mut view, None);
        assert_eq!(TopologyObjects::surface_face(&view, surface).unwrap(), None);
        assert_eq!(
            TopologyObjects::surface_owner(&view, surface).unwrap(),
            None
        );
        // An owner no insertion body holds is a broken face slot, not a missing port.
        set_owner(
            &mut view,
            Some(ObjectRef {
                type_code: -123,
                key: ObjectKey(u64::MAX),
            }),
        );
        assert!(!TopologyObjects::surface_face(&view, surface)
            .unwrap_err()
            .is_unported());
        assert!(!TopologyObjects::surface_owner(&view, surface)
            .unwrap_err()
            .is_unported());
    }
}
