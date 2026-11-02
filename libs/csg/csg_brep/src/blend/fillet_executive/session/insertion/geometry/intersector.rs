//! Existing topology-intersector operations on the retained insertion bodies.

use super::{invalid, EntityRef, FilletObject, InsertionBody, InsertionGeometry, Result};
use crate::boolean::topology_intersector::{
    p1 as ti, p2::IntersectorBrepP2, p3::IntersectorBrepP3,
};
use crate::geom::{extent2d::Extent2d, extent3d::Extent3d};
use crate::intersect::point_class::PointClassification;
use crate::intersect::topology_solver::p1::TopologyObjects;
use crate::math::{Point2, Point3};
use crate::nurbs::b_spline_curve::p2::BoundingBoxRequest;
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::topology::{
    brep, edge, edgeuse, face, loopuse, vertex, Brep, EdgeId, EdgeuseId, FaceId, FaceuseId,
    LoopuseId, VertexId,
};
use crate::{Kernel, SolidError};

macro_rules! projection {
    ($name:ident, $variant:ident, $id:ty) => {
        fn $name(object: FilletObject) -> Result<(InsertionBody, $id)> {
            match InsertionGeometry::topology(object) {
                Some((role, EntityRef::$variant(id))) => Ok((role, id)),
                _ => Err(invalid(concat!(
                    "not an actual ",
                    stringify!($variant),
                    " handle"
                ))),
            }
        }
    };
}
projection!(vid, Vertex, VertexId);
projection!(eid, Edge, EdgeId);
projection!(fid, Face, FaceId);
projection!(euid, Edgeuse, EdgeuseId);
projection!(fuid, Faceuse, FaceuseId);
projection!(luid, Loopuse, LoopuseId);
fn same(role: InsertionBody, actual: InsertionBody) -> Result<()> {
    if role == actual {
        Ok(())
    } else {
        Err(invalid("operands belong to different actual bodies"))
    }
}

impl InsertionGeometry<'_> {
    fn two_bodies<R>(
        &mut self,
        first: InsertionBody,
        second: InsertionBody,
        call: impl FnOnce(&mut Brep, &mut Brep) -> Result<R>,
    ) -> Result<R> {
        match (first, second) {
            (InsertionBody::Target, InsertionBody::Pseudo) => {
                call(self.target, self.pseudo.brep_mut())
            }
            (InsertionBody::Target, InsertionBody::Intermediate) => {
                call(self.target, self.intermediate)
            }
            (InsertionBody::Pseudo, InsertionBody::Target) => {
                call(self.pseudo.brep_mut(), self.target)
            }
            (InsertionBody::Pseudo, InsertionBody::Intermediate) => {
                call(self.pseudo.brep_mut(), self.intermediate)
            }
            (InsertionBody::Intermediate, InsertionBody::Target) => {
                call(self.intermediate, self.target)
            }
            (InsertionBody::Intermediate, InsertionBody::Pseudo) => {
                call(self.intermediate, self.pseudo.brep_mut())
            }
            _ => Err(invalid("two distinct bodies required")),
        }
    }
    fn register_optional_curves(
        &mut self,
        curves: Vec<Option<crate::curve::curve::Curve>>,
    ) -> Result<Vec<Option<FilletObject>>> {
        curves
            .into_iter()
            .map(|curve| curve.map(|c| self.insert_curve_copy(c)).transpose())
            .collect()
    }
}

impl ti::IntersectorBrep<FilletObject> for InsertionGeometry<'_> {
    fn face_point_3d_classify(
        &mut self,
        k: &Kernel,
        object: FilletObject,
        point: Point3,
        tolerance: f64,
        boundaries: bool,
        pc: &mut PointClassification<FilletObject>,
        guess: Option<Point2>,
    ) -> Result<()> {
        let (role, face) = fid(object)?;
        let mut actual = pc.map_objects(&mut |o| {
            let (body, entity) = Self::topology(o)
                .ok_or_else(|| invalid("classification without actual topology"))?;
            same(role, body)?;
            Ok(entity)
        })?;
        let result = crate::query::face::p1::point_3d_classify(
            k,
            self.body_mut(role),
            face,
            point,
            tolerance,
            boundaries,
            &mut actual,
            crate::query::face::p1::Point3dClassifyOptions {
                guess_uv: guess,
                ..Default::default()
            },
        );
        *pc = actual.map_objects(&mut |entity| Ok(Self::handle(role, entity)))?;
        result
    }
    fn make_vertex_loop(
        &mut self,
        body: FilletObject,
        face: FilletObject,
        point: Point3,
    ) -> Result<(FilletObject, FilletObject)> {
        let role = super::edits::body_role(body)?;
        let (actual, face) = fid(face)?;
        same(role, actual)?;
        self.with_edit(role, |b, env| {
            let (loop_, vertex) =
                brep::p9::make_vertex_loop(b, face, point, OrientType::Opposite, env)?;
            Ok((
                Self::handle(role, EntityRef::Loop(loop_)),
                Self::handle(role, EntityRef::Vertex(vertex)),
            ))
        })
    }
    fn make_vertex_split_edge(
        &mut self,
        k: &mut Kernel,
        body: FilletObject,
        edge: FilletObject,
        parameter: f64,
    ) -> Result<(FilletObject, FilletObject, FilletObject)> {
        let role = super::edits::body_role(body)?;
        let (actual, edge) = eid(edge)?;
        same(role, actual)?;
        self.with_edit(role, |b, env| {
            let output = brep::p9::make_vertex_split_edge_with_curve_identity(
                k,
                b,
                edge,
                parameter,
                env,
                brep::p9::CurveIdentity::Actual(&brep::p9::issue_curve_identity),
            )?;
            if !output.split {
                return Err(SolidError::sm_err(super::OP));
            }
            let [a, z] = output
                .edges
                .ok_or_else(|| invalid("successful split without actual edges"))?;
            let vertex = output
                .vertex
                .ok_or_else(|| invalid("successful split without actual vertex"))?;
            Ok((
                Self::handle(role, EntityRef::Edge(a)),
                Self::handle(role, EntityRef::Edge(z)),
                Self::handle(role, EntityRef::Vertex(vertex)),
            ))
        })
    }
    fn vertex_brep(&self, object: FilletObject) -> Result<Option<FilletObject>> {
        let (role, id) = vid(object)?;
        Ok(vertex::brep(self.body(role), id)?.map(|_| {
            Self::handle(
                role,
                EntityRef::Brep(crate::topology::BrepId::from_index(0)),
            )
        }))
    }
    fn brep_arena(&self, body: FilletObject) -> Result<&Brep> {
        Ok(self.body(super::edits::body_role(body)?))
    }
    fn edge_brep_handle(&self, object: FilletObject) -> Result<Option<FilletObject>> {
        let (role, id) = eid(object)?;
        Ok(edge::brep(self.body(role), id)?.map(|_| {
            Self::handle(
                role,
                EntityRef::Brep(crate::topology::BrepId::from_index(0)),
            )
        }))
    }
    fn set_vertex_point(&mut self, _k: &Kernel, object: FilletObject, point: Point3) -> Result<()> {
        let (role, id) = vid(object)?;
        self.with_edit(role, |b, env| {
            vertex::set_point(
                b,
                id,
                point,
                env.records,
                env.attributes,
                env.context.as_deref_mut(),
                (env.key_of)(EntityRef::Vertex(id)),
                &|vu| (env.key_of)(EntityRef::Vertexuse(vu)),
            )
        })
    }
    fn set_topology_tolerance(
        &mut self,
        _k: &Kernel,
        object: FilletObject,
        tol: f64,
        larger: bool,
        cascade: bool,
    ) -> Result<()> {
        let (role, entity) =
            Self::topology(object).ok_or_else(|| invalid("unknown topology role"))?;
        match entity {
            EntityRef::Vertex(v) => {
                vertex::set_tolerance(self.body_mut(role), v, tol, larger, cascade)
            }
            EntityRef::Edge(e) => edge::set_tolerance(self.body_mut(role), e, tol, larger, cascade),
            _ => Err(invalid("SetTolerance requires actual vertex or edge")),
        }
    }
    fn update_object_tolerance(
        &mut self,
        k: &Kernel,
        object: FilletObject,
        gap: f64,
        tol: f64,
        check: bool,
    ) -> Result<()> {
        use crate::geom::tol::{update_object_tolerance, TopologyToUpdate};
        let (role, entity) =
            Self::topology(object).ok_or_else(|| invalid("unknown topology role"))?;
        let target = match entity {
            EntityRef::Vertex(v) => TopologyToUpdate::Vertex(v),
            EntityRef::Edge(e) => TopologyToUpdate::Edge(e),
            EntityRef::Face(f) => TopologyToUpdate::Face(f),
            _ => return Err(invalid("not actual tolerance topology")),
        };
        update_object_tolerance(k, self.body_mut(role), Some(target), gap, tol, check)
    }
    fn vertex_is_shell_vertex(&self, object: FilletObject) -> Result<bool> {
        let (r, v) = vid(object)?;
        vertex::is_shell_vertex(self.body(r), v)
    }
    fn edge_is_wire(&self, object: FilletObject) -> Result<bool> {
        let (r, e) = eid(object)?;
        edge::is_wire(self.body(r), e)
    }
    fn edge_is_lamina(&self, object: FilletObject) -> Result<bool> {
        let (r, e) = eid(object)?;
        edge::is_lamina(self.body(r), e)
    }
    fn edge_vertex(&self, object: FilletObject) -> Result<Option<FilletObject>> {
        let (r, e) = eid(object)?;
        Ok(edge::vertex(self.body(r), e)?.map(|v| Self::handle(r, EntityRef::Vertex(v))))
    }
    fn face_brep(&self, object: FilletObject) -> Result<Option<FilletObject>> {
        let (r, f) = fid(object)?;
        Ok(face::p4::brep(self.body(r), f)?
            .map(|_| Self::handle(r, EntityRef::Brep(crate::topology::BrepId::from_index(0)))))
    }
    fn face_vertices(&self, object: FilletObject) -> Result<Vec<FilletObject>> {
        let (r, f) = fid(object)?;
        Ok(face::p4::vertices(self.body(r), f, None, false, None)?
            .into_iter()
            .map(|v| Self::handle(r, EntityRef::Vertex(v)))
            .collect())
    }
    fn face_edges(&self, object: FilletObject) -> Result<Vec<FilletObject>> {
        let (r, f) = fid(object)?;
        Ok(face::p4::edges(self.body(r), f, None, false, None)?
            .into_iter()
            .map(|e| Self::handle(r, EntityRef::Edge(e)))
            .collect())
    }
    fn edge_primary_edgeuse_orientation(&self, object: FilletObject) -> Result<Option<OrientType>> {
        let (r, e) = eid(object)?;
        let b = self.body(r);
        edgeuse::p1::primary_edgeuse(b, e)?
            .map(|eu| edgeuse::p1::orientation(b, eu))
            .transpose()
    }
    fn edge_bounding_box(&self, k: &Kernel, object: FilletObject) -> Result<Extent3d> {
        let (r, e) = eid(object)?;
        crate::query::edge::calculate_bounding_box(
            k,
            self.body(r),
            e,
            BoundingBoxRequest {
                normal: true,
                ..Default::default()
            },
            false,
        )?
        .normal
        .ok_or_else(|| invalid("normal bounding box absent"))
    }
    fn face_uv_domain_from_uv_trim_curves(
        &mut self,
        k: &Kernel,
        object: FilletObject,
    ) -> Result<Extent2d> {
        let (r, f) = fid(object)?;
        Ok(face::p1::calculate_uv_domain_from_uv_trim_curves(
            k,
            self.body_mut(r),
            f,
            true,
            false,
            false,
        )?
        .domain)
    }
    fn face_point_classify(
        &mut self,
        k: &Kernel,
        object: FilletObject,
        uv: Point2,
        pc: &mut PointClassification<FilletObject>,
    ) -> Result<()> {
        let (r, f) = fid(object)?;
        let mut actual = pc.map_objects(&mut |object| {
            let (role, entity) = Self::topology(object)
                .ok_or_else(|| invalid("classification without actual topology"))?;
            same(r, role)?;
            Ok(entity)
        })?;
        let result = crate::query::face::p2::point_classify(
            k,
            self.body_mut(r),
            f,
            uv,
            &mut actual,
            crate::query::face::p2::PointClassifyOptions::default(),
        );
        *pc = actual.map_objects(&mut |entity| Ok(Self::handle(r, entity)))?;
        result
    }
    fn face_intersect(
        &mut self,
        k: &Kernel,
        first: FilletObject,
        second: FilletObject,
        tol: f64,
        angle: f64,
        wants: ti::FaceIntersectWants,
        coincident: Option<bool>,
    ) -> Result<ti::FaceIntersection<FilletObject>> {
        use face::p3::{FaceBreps, FaceIntersectOptions, IntersectObject};
        let (a, fa) = fid(first)?;
        let (b, fb) = fid(second)?;
        let options = FaceIntersectOptions {
            want_this_classes: true,
            want_other_classes: true,
            want_cc_curves: true,
            want_this_uv_curves: wants.this_uv_curves,
            want_other_uv_curves: wants.other_uv_curves,
            want_points: wants.points_3d,
            coincidence: coincident,
        };
        let out = if a == b {
            face::p3::face_intersect_breps(
                k,
                FaceBreps::One(self.body_mut(a)),
                fa,
                fb,
                tol,
                angle,
                &options,
            )?
        } else {
            self.two_bodies(a, b, |x, y| {
                face::p3::face_intersect_breps(
                    k,
                    FaceBreps::Two(x, y),
                    fa,
                    fb,
                    tol,
                    angle,
                    &options,
                )
            })?
        };
        let curves_3d = out
            .curves_3d
            .into_iter()
            .map(|c| self.insert_curve_copy(c))
            .collect::<Result<Vec<_>>>()?;
        let cc3d = self.register_optional_curves(
            out.cc_curves_3d
                .unwrap_or_default()
                .into_iter()
                .map(Some)
                .collect(),
        )?;
        let uv1 = self.register_optional_curves(out.cc_this_uv_curves.unwrap_or_default())?;
        let uv2 = self.register_optional_curves(out.cc_other_uv_curves.unwrap_or_default())?;
        let this_uv_curves =
            self.register_optional_curves(out.this_uv_curves.unwrap_or_default())?;
        let other_uv_curves =
            self.register_optional_curves(out.other_uv_curves.unwrap_or_default())?;
        let mut map = |object| -> Result<FilletObject> {
            let bad = || invalid("intersection output without actual geometry");
            Ok(match object {
                IntersectObject::Curve(i) => cc3d.get(i).copied().flatten().ok_or_else(bad)?,
                IntersectObject::UvCurve(i, false) => {
                    uv1.get(i).copied().flatten().ok_or_else(bad)?
                }
                IntersectObject::UvCurve(i, true) => {
                    uv2.get(i).copied().flatten().ok_or_else(bad)?
                }
                IntersectObject::Surface(f, side) => {
                    FilletObject::FaceSurface(if side { b } else { a }, f)
                }
                IntersectObject::EdgeCurve(e, side) => InsertionGeometry::edge_curve(
                    self,
                    Self::handle(if side { b } else { a }, EntityRef::Edge(e)),
                )
                .flatten()
                .ok_or_else(bad)?,
                IntersectObject::Trim(eu, side) => match if side { b } else { a } {
                    InsertionBody::Target => FilletObject::UvCurve(super::FilletBody::Target, eu),
                    InsertionBody::Pseudo => FilletObject::UvCurve(super::FilletBody::Pseudo, eu),
                    InsertionBody::Intermediate => FilletObject::IntermediateUvCurve(eu),
                },
                IntersectObject::Topology(entity, side) => {
                    Self::handle(if side { b } else { a }, entity)
                }
                // face intersections name no free surface
                IntersectObject::FreeSurface(_) => return Err(bad()),
            })
        };
        let cc1 = out
            .this_classes
            .unwrap_or_default()
            .into_iter()
            .map(|c| c.map_objects(&mut map))
            .collect::<Result<Vec<_>>>()?;
        let cc2 = out
            .other_classes
            .unwrap_or_default()
            .into_iter()
            .map(|c| c.map_objects(&mut map))
            .collect::<Result<Vec<_>>>()?;
        Ok(ti::FaceIntersection {
            curves_3d,
            cc1,
            cc2,
            cc_curves: [
                cc3d.into_iter().flatten().collect(),
                uv1.into_iter().flatten().collect(),
                uv2.into_iter().flatten().collect(),
            ],
            this_uv_curves: this_uv_curves.into_iter().flatten().collect(),
            other_uv_curves: other_uv_curves.into_iter().flatten().collect(),
            points_3d: out.points_3d.unwrap_or_default(),
        })
    }
    fn set_editing_enabled(&mut self, body: FilletObject, enabled: bool) -> Result<bool> {
        let r = super::edits::body_role(body)?;
        let b = self.body_mut(r);
        let old = b.editing_enabled();
        b.set_editing_enabled(enabled);
        Ok(old)
    }
    fn editing_enabled(&self, body: FilletObject) -> Result<bool> {
        Ok(self.body(super::edits::body_role(body)?).editing_enabled())
    }
    fn brep_tolerance(&self, body: FilletObject) -> Result<f64> {
        Ok(self.body(super::edits::body_role(body)?).tolerance())
    }
    fn topology_objects(&self) -> &dyn TopologyObjects<FilletObject> {
        self
    }
    /// The body of `face` lent for its surface's trimmed cache, with the handles of
    /// that body's entities and their inverse.
    fn face_cache_target(
        &mut self,
        face: FilletObject,
    ) -> Result<ti::FaceCacheTarget<'_, FilletObject>> {
        let (role, id) = fid(face)?;
        let surface = self
            .face_surface(face)
            .flatten()
            .ok_or_else(|| invalid("face without surface"))?;
        Ok(ti::FaceCacheTarget {
            brep: self.body_mut(role),
            face: id,
            surface,
            handles: Box::new(move |e| Ok(InsertionGeometry::handle(role, e))),
            entities: Box::new(move |h| match InsertionGeometry::topology(h) {
                Some((r, e)) if r == role => Ok(e),
                _ => Err(invalid("handle outside the face's body")),
            }),
        })
    }
    fn face_entity_handles(&self, face: FilletObject) -> Result<ti::EntityHandles<FilletObject>> {
        let (role, _) = fid(face)?;
        Ok(Box::new(move |e| Ok(InsertionGeometry::handle(role, e))))
    }
    fn brep_delete_face(
        &mut self,
        k: &mut Kernel,
        body: FilletObject,
        object: FilletObject,
    ) -> Result<()> {
        let r = super::edits::body_role(body)?;
        let (a, f) = fid(object)?;
        same(r, a)?;
        self.with_edit(r, |b, env| {
            let body = (env.key_of)(EntityRef::Brep(crate::topology::BrepId::from_index(0)));
            brep::p3::delete_face_with_body_identity(
                k,
                b,
                env,
                f,
                brep::p3::DeleteFaceOptions::default(),
                body,
            )
            .map(drop)
        })
    }
    fn brep_delete_edge(
        &mut self,
        k: &mut Kernel,
        body: FilletObject,
        object: FilletObject,
    ) -> Result<()> {
        crate::topology::proto_topology::p3::BrepGlue::delete_edge(self, k, body, object)
    }
    fn brep_delete_vertex(
        &mut self,
        k: &mut Kernel,
        body: FilletObject,
        object: FilletObject,
    ) -> Result<()> {
        let r = super::edits::body_role(body)?;
        let (a, v) = vid(object)?;
        same(r, a)?;
        self.with_edit(r, |b, env| {
            let body = (env.key_of)(EntityRef::Brep(crate::topology::BrepId::from_index(0)));
            brep::p4::delete_vertex_with_body_identity(
                k,
                b,
                &mut brep::p4::DeleteEnv::from_notify(env),
                v,
                body,
            )
        })
    }
    fn brep_glue_edges_general(
        &mut self,
        k: &mut Kernel,
        body: FilletObject,
        surviving: Option<FilletObject>,
        orientation: OrientType,
        gap: f64,
        nesting: bool,
        to_delete: Option<FilletObject>,
        inside: Option<&[FilletObject]>,
    ) -> Result<()> {
        let r = super::edits::body_role(body)?;
        let edge = |o| -> Result<EdgeId> {
            let (a, e) = eid(o)?;
            same(r, a)?;
            Ok(e)
        };
        let surviving = surviving.map(edge).transpose()?;
        let to_delete = to_delete.map(edge).transpose()?;
        let inside = inside
            .map(|values| {
                values
                    .iter()
                    .map(|&o| {
                        let (a, f) = fuid(o)?;
                        same(r, a)?;
                        Ok(f)
                    })
                    .collect::<Result<Vec<_>>>()
            })
            .transpose()?;
        self.with_edit(r, |b, env| {
            brep::p7::glue_edges_general(
                k,
                b,
                surviving,
                orientation,
                gap,
                nesting,
                to_delete,
                inside.as_deref(),
                env,
            )
        })
    }
    fn brep_edges(&self, body: FilletObject) -> Result<Vec<FilletObject>> {
        let r = super::edits::body_role(body)?;
        Ok(brep::p1::edges(self.body(r), None)?
            .into_iter()
            .map(|e| Self::handle(r, EntityRef::Edge(e)))
            .collect())
    }
    fn brep_vertices(&self, body: FilletObject) -> Result<Vec<FilletObject>> {
        let r = super::edits::body_role(body)?;
        Ok(brep::p1::vertices(self.body(r), None)?
            .into_iter()
            .map(|v| Self::handle(r, EntityRef::Vertex(v)))
            .collect())
    }
    fn face_calculate_bounding_box(
        &self,
        k: &Kernel,
        object: FilletObject,
        tight: bool,
    ) -> Result<Extent3d> {
        let (r, f) = fid(object)?;
        crate::query::face::p1::calculate_bounding_box(k, self.body(r), f, tight)
    }
}

impl IntersectorBrepP2<FilletObject> for InsertionGeometry<'_> {
    fn vertex_edges(&self, object: FilletObject) -> Result<Vec<FilletObject>> {
        let (r, v) = vid(object)?;
        Ok(vertex::edges(self.body(r), v, None)?
            .into_iter()
            .map(|e| Self::handle(r, EntityRef::Edge(e)))
            .collect())
    }
    fn vertex_faces(&self, object: FilletObject) -> Result<Vec<FilletObject>> {
        let (r, v) = vid(object)?;
        Ok(vertex::faces(self.body(r), v)?
            .into_iter()
            .map(|f| Self::handle(r, EntityRef::Face(f)))
            .collect())
    }
    fn brep_faces(&self, body: FilletObject) -> Result<Vec<FilletObject>> {
        let r = super::edits::body_role(body)?;
        Ok(brep::p1::faces(self.body(r), None)?
            .into_iter()
            .map(|f| Self::handle(r, EntityRef::Face(f)))
            .collect())
    }
    fn brep_combine_coincident_vertices(
        &mut self,
        k: &mut Kernel,
        body: FilletObject,
        surviving: FilletObject,
        deleted: FilletObject,
    ) -> Result<Option<FilletObject>> {
        let r = super::edits::body_role(body)?;
        let (a, s) = vid(surviving)?;
        same(r, a)?;
        let (a, d) = vid(deleted)?;
        same(r, a)?;
        self.with_edit(r, |b, env| {
            let mut edge = None;
            brep::p5::combine_with_notifications(k, b, s, d, &mut edge, Some(env))?;
            Ok(edge.map(|e| Self::handle(r, EntityRef::Edge(e))))
        })
    }
    fn edge_normalized_evaluate(
        &mut self,
        k: &Kernel,
        object: FilletObject,
        t: f64,
    ) -> Result<Point3> {
        let (r, e) = eid(object)?;
        let b = self.body_mut(r);
        let eu = edgeuse::p1::primary_edgeuse(b, e)?
            .ok_or_else(|| invalid("actual edge has no primary edgeuse"))?;
        Ok(edgeuse::p3::normalized_evaluate(k, b, eu, t, false, false, true)?.point)
    }
}

impl IntersectorBrepP3<FilletObject> for InsertionGeometry<'_> {
    fn surface_face(&self, object: FilletObject) -> Result<Option<FilletObject>> {
        TopologyObjects::surface_face(self, object)
    }
    fn edge_is_manifold(&self, object: FilletObject) -> Result<bool> {
        let (r, e) = eid(object)?;
        edge::is_manifold(self.body(r), e)
    }
    fn loopuse_is_vertex_loopuse(&self, object: FilletObject) -> Result<bool> {
        let (r, l) = luid(object)?;
        loopuse::is_vertex_loopuse(self.body(r), l)
    }
    fn loopuse_edgeuses(&self, object: FilletObject) -> Result<Vec<FilletObject>> {
        let (r, l) = luid(object)?;
        Ok(loopuse::edgeuses(self.body(r), l, None, None)?
            .into_iter()
            .map(|eu| Self::handle(r, EntityRef::Edgeuse(eu)))
            .collect())
    }
    fn edgeuse_edge(&self, object: FilletObject) -> Result<Option<FilletObject>> {
        let (r, eu) = euid(object)?;
        Ok(self
            .body(r)
            .edgeuse(eu)?
            .edge
            .map(|e| Self::handle(r, EntityRef::Edge(e))))
    }
    fn edgeuse_face(&self, object: FilletObject) -> Result<Option<FilletObject>> {
        let (r, eu) = euid(object)?;
        Ok(edgeuse::p1::face(self.body(r), eu)?.map(|f| Self::handle(r, EntityRef::Face(f))))
    }
    fn faceuse_face(&self, object: FilletObject) -> Result<Option<FilletObject>> {
        let (r, fu) = fuid(object)?;
        Ok(self
            .body(r)
            .faceuse(fu)?
            .face
            .map(|f| Self::handle(r, EntityRef::Face(f))))
    }
    fn edge_find_radial_sector(
        &mut self,
        k: &Kernel,
        edge: FilletObject,
        classify: FilletObject,
        orientation: OrientType,
    ) -> Result<(Option<FilletObject>, Option<FilletObject>)> {
        let (a, e) = eid(edge)?;
        let (b, eu) = euid(classify)?;
        let sector = if a == b {
            edge::find_radial_sector_between(
                k,
                self.body_mut(a),
                e,
                None,
                eu,
                None,
                orientation,
                None,
            )?
        } else {
            self.two_bodies(a, b, |x, y| {
                edge::find_radial_sector_between(k, x, e, Some(y), eu, None, orientation, None)
            })?
        };
        Ok((
            sector
                .radial_edgeuse
                .map(|eu| Self::handle(a, EntityRef::Edgeuse(eu))),
            sector
                .coincident_faceuse
                .map(|fu| Self::handle(a, EntityRef::Faceuse(fu))),
        ))
    }
    fn face_bounding_box(&self, k: &Kernel, object: FilletObject) -> Result<Extent3d> {
        let (r, f) = fid(object)?;
        crate::query::face::p1::calculate_bounding_box(k, self.body(r), f, false)
    }
    fn face_upward_loopuses(&self, object: FilletObject) -> Result<Vec<FilletObject>> {
        let (r, f) = fid(object)?;
        Ok(face::p4::upward_loopuses(self.body(r), f, false, None)?
            .into_iter()
            .map(|lu| Self::handle(r, EntityRef::Loopuse(lu)))
            .collect())
    }
}
