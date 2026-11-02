//! The existing proto-topology algorithms over actual insertion bodies.

use super::{ClassGeometry, Curve, EntityRef, FilletObject, InsertionGeometry, Point3, Result};
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::intersect::curve_class::CurveClassification;
use crate::intersect::point_class::{MergeOutputs, PointClassType, PointClassification};
use crate::math::{Point2, Vec3};
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::topology::proto_topology::p2::ProtoBrep;
use crate::Kernel;

impl ProtoBrep<FilletObject> for InsertionGeometry<'_> {
    fn edge_interval(&self, edge: FilletObject) -> Option<Extent1d> {
        InsertionGeometry::edge_interval(self, edge)
    }
    fn edge_curve(&self, edge: FilletObject) -> Option<Option<FilletObject>> {
        InsertionGeometry::edge_curve(self, edge)
    }
    fn topology_tolerance(&self, object: FilletObject) -> Option<f64> {
        self.topology_zone_tol_3d(object)
    }
    fn face_uv_domain(&self, face: FilletObject) -> Option<Extent2d> {
        InsertionGeometry::face_uv_domain(self, face)
    }
    fn face_trim_uv_domain(&mut self, k: &Kernel, face: FilletObject) -> Result<Extent2d> {
        InsertionGeometry::face_trim_uv_domain(self, k, face)
    }
    fn face_point_classify_uv(
        &mut self,
        k: &Kernel,
        face: FilletObject,
        uv: Point2,
        tolerance: f64,
    ) -> Result<PointClassType> {
        InsertionGeometry::face_point_classify_uv(self, k, face, uv, tolerance)
    }
    fn set_face_uv_domain(&mut self, face: FilletObject, domain: Extent2d) -> Option<()> {
        InsertionGeometry::set_face_uv_domain(self, face, domain)
    }
    fn face_surface(&self, face: FilletObject) -> Option<Option<FilletObject>> {
        InsertionGeometry::face_surface(self, face)
    }
    fn edge_edgeuse_of_face(
        &self,
        edge: FilletObject,
        face: FilletObject,
    ) -> Result<Option<FilletObject>> {
        InsertionGeometry::edge_edgeuse_of_face(self, edge, face)
    }
    fn edge_is_connected_to_face(&self, edge: FilletObject, face: FilletObject) -> Result<bool> {
        InsertionGeometry::edge_is_connected_to_face(self, edge, face)
    }
    fn insert_curve(&mut self, curve: Curve) -> Option<FilletObject> {
        self.insert_curve_copy(curve).ok()
    }
    fn vertex_common_edges(
        &self,
        first: FilletObject,
        second: FilletObject,
    ) -> Result<Vec<FilletObject>> {
        InsertionGeometry::vertex_common_edges(self, first, second)
    }
    fn brep_squeeze_edge(
        &mut self,
        k: &mut Kernel,
        body: FilletObject,
        edge: FilletObject,
        survivor: FilletObject,
    ) -> Result<()> {
        InsertionGeometry::brep_squeeze_edge(self, k, body, edge, survivor)
    }
    fn face_curve_on_classify(
        &self,
        k: &Kernel,
        face: FilletObject,
        classify: bool,
        cc: &mut CurveClassification<FilletObject>,
    ) -> Result<bool> {
        // Legacy const interface; owning algorithm call sites use the mutable successor.
        super::query::classify_readonly(self, k, face, classify, cc)
    }
    fn face_curve_on_classify_mut(
        &mut self,
        k: &Kernel,
        face: FilletObject,
        classify: bool,
        cc: &mut CurveClassification<FilletObject>,
    ) -> Result<bool> {
        InsertionGeometry::face_curve_on_classify_mut(self, k, face, classify, cc)
    }
    fn brep_point_3d_classify(
        &mut self,
        k: &Kernel,
        body: FilletObject,
        point: Point3,
        tolerance: f64,
        boundary: bool,
    ) -> Result<PointClassification<FilletObject>> {
        InsertionGeometry::brep_point_3d_classify(self, k, body, point, tolerance, boundary)
    }
    fn brep_make_edge_in_face(
        &mut self,
        k: &Kernel,
        body: FilletObject,
        face: FilletObject,
        first: FilletObject,
        second: FilletObject,
        mut curve: Curve,
        uv_curve: Option<Curve>,
        interval: Extent1d,
        orientation: OrientType,
        tolerance: f64,
    ) -> Result<(
        Option<FilletObject>,
        Option<FilletObject>,
        Option<FilletObject>,
    )> {
        let role = super::edits::body_role(body)?;
        let (actual, EntityRef::Face(face)) =
            Self::topology(face).ok_or_else(|| super::invalid("unknown Face role"))?
        else {
            return Err(super::invalid("handle is not a Face"));
        };
        if actual != role {
            return Err(super::invalid("Face is not in the actual B-rep"));
        }
        let first = super::edits::vertex(role, first)?;
        let second = super::edits::vertex(role, second)?;
        self.with_edit(role, |body, env| {
            let type_code = curve.type_code();
            let context = env
                .context
                .as_deref_mut()
                .ok_or_else(|| super::invalid("missing actual curve issuer"))?;
            let key = context.geometry_key(curve.notify_identity_slot())?;
            let identity = crate::geom::object::ObjectRef { type_code, key };
            // p9 installs this same owned input Curve; no copy/replacement precedes the sole lookup.
            let curve_ref = |_, _| identity;
            let mut output = crate::topology::brep::p5::EdgeInFace::default();
            crate::topology::brep::p5::make_edge_in_face_in(
                k,
                body,
                Some(face),
                Some(first),
                Some(second),
                curve,
                uv_curve,
                interval,
                orientation,
                tolerance,
                &mut output,
                env,
                &curve_ref,
            )?;
            Ok((
                output
                    .edge
                    .map(|id| Self::handle(role, EntityRef::Edge(id))),
                output
                    .loop_
                    .map(|id| Self::handle(role, EntityRef::Loop(id))),
                output
                    .face
                    .map(|id| Self::handle(role, EntityRef::Face(id))),
            ))
        })
    }
    fn edgeuse_evaluate_binormal(
        &mut self,
        k: &Kernel,
        edgeuse: FilletObject,
        parameter: f64,
    ) -> Result<Vec3> {
        InsertionGeometry::edgeuse_evaluate_binormal(self, k, edgeuse, parameter)
    }
    fn merge_point_class_into_brep(
        &mut self,
        k: &mut Kernel,
        pc: &mut PointClassification<FilletObject>,
        point: Point3,
    ) -> Result<MergeOutputs<FilletObject>> {
        crate::intersect::curve_class::p6::merge_into_object_mut(k, self, pc, point)
    }
}
