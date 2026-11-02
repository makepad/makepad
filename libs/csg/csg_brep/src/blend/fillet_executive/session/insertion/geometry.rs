//! Actual target/intermediate topology and pseudo-curve bindings for insertion.

mod edits;
mod intersector;
mod proto;
mod query;
mod topology_objects;

use super::super::{FilletBody, FilletObject};
use super::curves::InsertionCurves;
use crate::blend::fillet_geom::p1::FilletBrep;
use crate::curve::curve::Curve;
use crate::geom::attribute::AttributeStore;
use crate::geom::attribute::ObjectKey;
use crate::geom::context::Context;
use crate::geom::object::ObjectRef;
use crate::intersect::curve_class::p5::ClassGeometryMut;
use crate::intersect::point_class::ClassGeometry;
use crate::math::Point3;
use crate::surface::surface::Surface;
use crate::surface::volume::Volume;
use crate::topology::brep::p3::topology_ref;
use crate::topology::proto_topology::p1::{ProtoGeometry, ProtoTopology};
use crate::topology::{Brep, BrepId, FaceId};
use crate::{EntityRef, Result, SolidError};

const OP: &str = "blend::fillet_executive::insertion_geometry";
fn invalid(reason: &'static str) -> SolidError {
    SolidError::invalid_input(OP, reason)
}

/// One actual body in the insertion operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum InsertionBody {
    /// Original target.
    Target,
    /// Retained pseudo topology and curves.
    Pseudo,
    /// Constructed intermediate fillet body.
    Intermediate,
}

/// An actual Surface instance within this Session, distinct from its owning Face slot.
/// Bindings from different Sessions or identity domains must not be mixed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct SurfaceBinding {
    role: InsertionBody,
    body: ObjectRef,
    face: FaceId,
    geometry: ObjectRef,
}
impl SurfaceBinding {
    /// The face-surface handle the intersector's face loops name this surface by
    /// ([`InsertionGeometry::face_surface`] of its face).
    pub(super) const fn face_surface(&self) -> FilletObject {
        FilletObject::FaceSurface(self.role, self.face)
    }
    pub(super) fn capture_in_body(
        context: &mut Context,
        body: &mut Brep,
        role: InsertionBody,
        face: FaceId,
    ) -> Result<Self> {
        let serial = context
            .issued_brep_serial(body.notify_identity_slot())
            .ok_or_else(|| invalid("Surface body is outside the actual Context domain"))?;
        let body_identity = topology_ref(serial, EntityRef::Brep(BrepId::from_index(0)));
        let surface = body
            .face_mut(face)?
            .surface
            .as_mut()
            .ok_or_else(|| SolidError::null_pointer(OP))?;
        let type_code = surface.type_code();
        let key = context.geometry_key(surface.notify_identity_slot())?;
        Ok(Self {
            role,
            body: body_identity,
            face,
            geometry: ObjectRef { type_code, key },
        })
    }
    pub(super) fn resolve_in_body(self, body: &Brep) -> Result<&Surface> {
        let serial = body
            .notify_serial()
            .ok_or_else(|| invalid("Surface body has no issued identity"))?;
        if self.body != topology_ref(serial, EntityRef::Brep(BrepId::from_index(0))) {
            return Err(invalid("Surface binding belongs to a foreign body"));
        }
        let surface = body
            .face(self.face)?
            .surface
            .as_ref()
            .ok_or_else(|| SolidError::null_pointer(OP))?;
        if surface.notify_key() != Some(self.geometry.key)
            || surface.type_code() != self.geometry.type_code
        {
            return Err(invalid("original insertion Surface was replaced"));
        }
        Ok(surface)
    }
    /// Actual body role.
    #[must_use]
    pub const fn body_role(self) -> InsertionBody {
        self.role
    }
    /// Original actual Face slot.
    #[must_use]
    pub const fn face(self) -> FaceId {
        self.face
    }
    /// Issued identity of the actual geometry, not its Face.
    #[must_use]
    pub const fn identity(self) -> ObjectRef {
        self.geometry
    }
}

/// Mutable borrows of the three actual bodies; no topology or geometry is copied.
pub struct InsertionGeometry<'a> {
    target: &'a mut Brep,
    pseudo: &'a mut FilletBrep,
    intermediate: &'a mut Brep,
    curves: Option<&'a mut InsertionCurves>,
    notifications: Option<InsertionNotifications<'a>>,
    topology_keys: Option<BodyKeys<'a>>,
}

type TopologyKey<'a> = Box<dyn Fn(EntityRef) -> ObjectRef + 'a>;
struct BodyKeys<'a> {
    target: TopologyKey<'a>,
    pseudo: TopologyKey<'a>,
    intermediate: TopologyKey<'a>,
}
impl BodyKeys<'_> {
    fn key_of(&self, role: InsertionBody) -> &dyn Fn(EntityRef) -> ObjectRef {
        match role {
            InsertionBody::Target => &*self.target,
            InsertionBody::Pseudo => &*self.pseudo,
            InsertionBody::Intermediate => &*self.intermediate,
        }
    }
}

/// Actual retained edit stores and the already lent notification context.
pub struct InsertionNotifications<'a> {
    /// Actual topology notification records.
    pub records: &'a mut crate::topology::topology::NotifyRecords,
    /// Actual attributes, including executive E and active face markers.
    pub attributes: &'a mut AttributeStore,
    /// Actual context lease, supplied by `with_lent_context`.
    pub context: &'a mut Context,
}
impl<'a> InsertionGeometry<'a> {
    /// Borrow the actual Session bodies in their established identity domains.
    /// # Errors
    /// Missing or colliding issued body identities.
    pub fn new(
        target: &'a mut Brep,
        pseudo: &'a mut FilletBrep,
        intermediate: &'a mut Brep,
    ) -> Result<Self> {
        let mut view = Self {
            target,
            pseudo,
            intermediate,
            curves: None,
            notifications: None,
            topology_keys: None,
        };
        let mut identities = Vec::new();
        for role in [
            InsertionBody::Target,
            InsertionBody::Pseudo,
            InsertionBody::Intermediate,
        ] {
            let identity = view.body_identity(role)?;
            if identities.contains(&identity) {
                return Err(invalid("insertion bodies have colliding identity domains"));
            }
            identities.push(identity);
        }
        let key = |role| -> Result<TopologyKey<'a>> {
            let serial = view
                .body(role)
                .notify_serial()
                .ok_or_else(|| invalid("missing actual body identity"))?;
            Ok(Box::new(move |entity| topology_ref(serial, entity)))
        };
        let keys = BodyKeys {
            target: key(InsertionBody::Target)?,
            pseudo: key(InsertionBody::Pseudo)?,
            intermediate: key(InsertionBody::Intermediate)?,
        };
        view.topology_keys = Some(keys);
        Ok(view)
    }
    /// Supply actual operation-owned curves and retained notification stores.
    /// The context must be the real lease used by this Session's edits.
    /// # Errors
    /// A body's existing identity belongs to another context domain. No identity is reissued.
    pub fn with_notifications(
        mut self,
        curves: &'a mut InsertionCurves,
        notifications: InsertionNotifications<'a>,
    ) -> Result<Self> {
        for role in [
            InsertionBody::Target,
            InsertionBody::Pseudo,
            InsertionBody::Intermediate,
        ] {
            if notifications
                .context
                .issued_brep_serial(self.body_mut(role).notify_identity_slot())
                .is_none()
            {
                return Err(invalid(
                    "insertion body is outside the actual notification domain",
                ));
            }
        }
        self.curves = Some(curves);
        self.notifications = Some(notifications);
        Ok(self)
    }
    /// Retain a real curve copy and return its actual issued operation handle.
    /// # Errors
    /// Missing actual edit stores or identity issuance failure.
    pub fn insert_curve_copy(&mut self, curve: Curve) -> Result<FilletObject> {
        let notifications = self
            .notifications
            .as_mut()
            .ok_or_else(|| invalid("missing insertion notification context"))?;
        let curves = self
            .curves
            .as_mut()
            .ok_or_else(|| invalid("missing insertion curve ownership"))?;
        Ok(FilletObject::InsertionCurve(
            curves.insert(notifications.context, curve)?,
        ))
    }
    /// Free a temporary curve copy an intersector call reported for deletion
    /// ([`InsertionCurves::destroy`] in the operation's notification domain).
    /// # Errors
    /// Not an operation-owned curve handle, missing edit stores, or those of
    /// [`InsertionCurves::destroy`].
    pub fn destroy_curve_copy(&mut self, curve: FilletObject) -> Result<()> {
        let FilletObject::InsertionCurve(binding) = curve else {
            return Err(invalid("deleted curve is not an operation-owned copy"));
        };
        let notifications = self
            .notifications
            .as_mut()
            .ok_or_else(|| invalid("missing insertion notification context"))?;
        let curves = self
            .curves
            .as_mut()
            .ok_or_else(|| invalid("missing insertion curve ownership"))?;
        curves.destroy(notifications.context, notifications.attributes, binding)
    }
    /// Run one edit against the actual body and retained notification stores.
    /// # Errors
    /// Missing actual context or the original edit failure, with partial writes retained.
    pub fn with_edit<T>(
        &mut self,
        role: InsertionBody,
        edit: impl FnOnce(&mut Brep, &mut crate::topology::edge::NotifyEnv<'_>) -> Result<T>,
    ) -> Result<T> {
        let serial = self
            .body(role)
            .notify_serial()
            .ok_or_else(|| invalid("missing issued body identity"))?;
        let key_of = |entity| topology_ref(serial, entity);
        let body = match role {
            InsertionBody::Target => &mut *self.target,
            InsertionBody::Pseudo => self.pseudo.brep_mut(),
            InsertionBody::Intermediate => &mut *self.intermediate,
        };
        let notifications = self
            .notifications
            .as_mut()
            .ok_or_else(|| invalid("missing insertion notification context"))?;
        crate::topology::brep::p3::seed_records(notifications.records, body, &key_of);
        let mut env = crate::topology::edge::NotifyEnv {
            records: notifications.records,
            attributes: notifications.attributes,
            context: Some(notifications.context),
            key_of: &key_of,
        };
        edit(body, &mut env)
    }
    /// Native edge interval, including actual curve-domain fallback.
    #[must_use]
    pub fn edge_interval(&self, edge: FilletObject) -> Option<crate::geom::extent1d::Extent1d> {
        let (role, EntityRef::Edge(edge)) = Self::topology(edge)? else {
            return None;
        };
        crate::topology::edge::interval(self.body(role), edge).ok()
    }
    /// Current edge Curve getter; distinct from the Edge topology object.
    #[must_use]
    pub fn edge_curve(&self, edge: FilletObject) -> Option<Option<FilletObject>> {
        let (role, EntityRef::Edge(edge)) = Self::topology(edge)? else {
            return None;
        };
        Some(
            self.body(role)
                .edge(edge)
                .ok()?
                .curve
                .as_ref()
                .map(|_| match role {
                    InsertionBody::Target => FilletObject::Curve(FilletBody::Target, edge),
                    InsertionBody::Pseudo => FilletObject::Curve(FilletBody::Pseudo, edge),
                    InsertionBody::Intermediate => FilletObject::IntermediateCurve(edge),
                }),
        )
    }
    /// Current Face UV domain.
    #[must_use]
    pub fn face_uv_domain(&self, face: FilletObject) -> Option<crate::geom::extent2d::Extent2d> {
        let (role, EntityRef::Face(face)) = Self::topology(face)? else {
            return None;
        };
        crate::topology::face::p2::uv_domain(self.body(role), face).ok()
    }
    /// Write the actual Face UV domain through its existing setter.
    pub fn set_face_uv_domain(
        &mut self,
        face: FilletObject,
        domain: crate::geom::extent2d::Extent2d,
    ) -> Option<()> {
        let (role, EntityRef::Face(face)) = Self::topology(face)? else {
            return None;
        };
        crate::topology::face::p2::set_uv_domain(self.body_mut(role), face, &domain).ok()
    }
    /// Current Face Surface getter, distinct from a captured original binding.
    #[must_use]
    pub fn face_surface(&self, face: FilletObject) -> Option<Option<FilletObject>> {
        let (role, EntityRef::Face(face)) = Self::topology(face)? else {
            return None;
        };
        Some(
            self.body(role)
                .face(face)
                .ok()?
                .surface
                .as_ref()
                .map(|_| FilletObject::FaceSurface(role, face)),
        )
    }
    /// Find the actual edge use belonging to a Face in the same body.
    /// # Errors
    /// Invalid Edge/Face handles or topology walk failures.
    pub fn edge_edgeuse_of_face(
        &self,
        edge: FilletObject,
        face: FilletObject,
    ) -> Result<Option<FilletObject>> {
        let (role, EntityRef::Edge(edge)) =
            Self::topology(edge).ok_or_else(|| invalid("unknown Edge role"))?
        else {
            return Err(invalid("handle is not an Edge"));
        };
        let (face_role, EntityRef::Face(face)) =
            Self::topology(face).ok_or_else(|| invalid("unknown Face role"))?
        else {
            return Err(invalid("handle is not a Face"));
        };
        if role != face_role {
            return Ok(None);
        }
        Ok(
            crate::topology::edge::edgeuse_of_face(self.body(role), edge, Some(face))?
                .map(|eu| Self::handle(role, EntityRef::Edgeuse(eu))),
        )
    }
    /// Test connectivity through the actual Edge topology.
    /// # Errors
    /// Invalid handles or topology walk failures.
    pub fn edge_is_connected_to_face(
        &self,
        edge: FilletObject,
        face: FilletObject,
    ) -> Result<bool> {
        let (role, EntityRef::Edge(edge)) =
            Self::topology(edge).ok_or_else(|| invalid("unknown Edge role"))?
        else {
            return Err(invalid("handle is not an Edge"));
        };
        let (face_role, EntityRef::Face(face)) =
            Self::topology(face).ok_or_else(|| invalid("unknown Face role"))?
        else {
            return Err(invalid("handle is not a Face"));
        };
        if role != face_role {
            return Ok(false);
        }
        crate::topology::edge::is_connected_to_face(self.body(role), edge, Some(face))
    }
    /// Common edges of actual same-body vertices.
    /// # Errors
    /// Invalid/cross-body operands or topology walk failures.
    pub fn vertex_common_edges(
        &self,
        first: FilletObject,
        second: FilletObject,
    ) -> Result<Vec<FilletObject>> {
        let (role, EntityRef::Vertex(first)) =
            Self::topology(first).ok_or_else(|| invalid("unknown Vertex role"))?
        else {
            return Err(invalid("handle is not a Vertex"));
        };
        let (other, EntityRef::Vertex(second)) =
            Self::topology(second).ok_or_else(|| invalid("unknown Vertex role"))?
        else {
            return Err(invalid("handle is not a Vertex"));
        };
        if role != other {
            return Err(invalid("vertices belong to different bodies"));
        }
        Ok(
            crate::topology::vertex::common_edges(self.body(role), first, Some(second), None)?
                .into_iter()
                .map(|edge| Self::handle(role, EntityRef::Edge(edge)))
                .collect(),
        )
    }
    /// Classify against the actual B-rep with its existing proven-owner query provider.
    /// # Errors
    /// Invalid body handle or the original point-query failure.
    pub fn brep_point_3d_classify(
        &mut self,
        k: &crate::Kernel,
        body: FilletObject,
        point: Point3,
        tolerance: f64,
        boundary_intersections: bool,
    ) -> Result<crate::intersect::point_class::PointClassification<FilletObject>> {
        let (role, EntityRef::Brep(_)) =
            Self::topology(body).ok_or_else(|| invalid("unknown B-rep role"))?
        else {
            return Err(invalid("handle is not a B-rep"));
        };
        let body = self.body_mut(role);
        let serial = u32::try_from(
            body.notify_serial()
                .ok_or_else(|| invalid("missing issued B-rep identity"))?,
        )
        .map_err(|_| invalid("B-rep identity is outside query ID range"))?;
        let query =
            crate::query::brep::BrepQuery::of_brep_with_id(BrepId::from_index(serial), body);
        query
            .point_3d_classify(k, body, point, tolerance, boundary_intersections)?
            .map_objects(&mut |entity| Ok(Self::handle(role, entity)))
    }
    /// Evaluate the actual edge-use binormal with native FALSE/default options.
    /// # Errors
    /// Invalid edge-use handle or the original evaluator failure.
    pub fn edgeuse_evaluate_binormal(
        &mut self,
        k: &crate::Kernel,
        edgeuse: FilletObject,
        parameter: f64,
    ) -> Result<crate::math::Vec3> {
        let (role, EntityRef::Edgeuse(edgeuse)) =
            Self::topology(edgeuse).ok_or_else(|| invalid("unknown Edgeuse role"))?
        else {
            return Err(invalid("handle is not an Edgeuse"));
        };
        Ok(crate::topology::edgeuse::p3::evaluate_binormal(
            k,
            self.body_mut(role),
            edgeuse,
            parameter,
            false,
            &crate::topology::edgeuse::p3::BinormalOptions::default(),
        )?
        .binormal)
    }
    /// Classify against an actual Face and retain lazy UV/gap edits on every result.
    /// # Errors
    /// Invalid owning Face, query failure or query-state synchronization failure.
    pub fn face_curve_on_classify_mut(
        &mut self,
        k: &crate::Kernel,
        face: FilletObject,
        do_point_classify: bool,
        cc: &mut crate::intersect::curve_class::CurveClassification<FilletObject>,
    ) -> Result<bool> {
        query::classify(self, k, face, do_point_classify, cc)
    }
    /// Calculate the actual (including partial) trim-derived UV domain.
    /// # Errors
    /// Invalid owning Face or a genuine missing lower capability.
    pub fn face_trim_uv_domain(
        &mut self,
        k: &crate::Kernel,
        face: FilletObject,
    ) -> Result<crate::geom::extent2d::Extent2d> {
        let (role, EntityRef::Face(face)) =
            Self::topology(face).ok_or_else(|| invalid("unknown Face role"))?
        else {
            return Err(invalid("handle is not a Face"));
        };
        let body = self.body_mut(role);
        body.face(face)?;
        let mut output = crate::topology::face::p1::UvDomainFromTrim {
            domain: crate::geom::extent2d::Extent2d::new(),
            max_uv_loop_gap: 0.0,
            second_max_uv_loop_gap: 0.0,
        };
        let result = crate::topology::face::p1::calculate_uv_domain_from_uv_trim_curves_into(
            k,
            body,
            face,
            true,
            false,
            false,
            &mut output,
        );
        if let Err(error) = result {
            if error.is_unported() {
                return Err(error);
            }
        }
        Ok(output.domain)
    }
    /// Classify UV on the actual Face using native ignored ordinary-status semantics.
    /// # Errors
    /// Invalid owning Face or a genuine missing lower capability.
    pub fn face_point_classify_uv(
        &mut self,
        k: &crate::Kernel,
        face: FilletObject,
        uv: crate::math::Point2,
        tolerance: f64,
    ) -> Result<crate::intersect::point_class::PointClassType> {
        let (role, EntityRef::Face(face)) =
            Self::topology(face).ok_or_else(|| invalid("unknown Face role"))?
        else {
            return Err(invalid("handle is not a Face"));
        };
        let body = self.body_mut(role);
        body.face(face)?;
        let mut output = crate::intersect::point_class::PointClassification::new(tolerance);
        let result = crate::query::face::p2::point_classify(
            k,
            body,
            face,
            uv,
            &mut output,
            crate::query::face::p2::PointClassifyOptions {
                src_zone_tol_3d: tolerance,
                do_boundary_intersections: true,
                use_uv_classification: true,
                ..crate::query::face::p2::PointClassifyOptions::default()
            },
        );
        if let Err(error) = result {
            if error.is_unported() {
                return Err(error);
            }
        }
        Ok(output.point_class())
    }
    fn body(&self, role: InsertionBody) -> &Brep {
        match role {
            InsertionBody::Target => self.target,
            InsertionBody::Pseudo => self.pseudo.brep(),
            InsertionBody::Intermediate => self.intermediate,
        }
    }
    fn body_mut(&mut self, role: InsertionBody) -> &mut Brep {
        match role {
            InsertionBody::Target => self.target,
            InsertionBody::Pseudo => self.pseudo.brep_mut(),
            InsertionBody::Intermediate => self.intermediate,
        }
    }
    fn body_identity(&self, role: InsertionBody) -> Result<ObjectRef> {
        let serial = self
            .body(role)
            .notify_serial()
            .ok_or_else(|| invalid("insertion body has no issued identity"))?;
        Ok(topology_ref(serial, EntityRef::Brep(BrepId::from_index(0))))
    }
    fn topology(object: FilletObject) -> Option<(InsertionBody, EntityRef)> {
        match object {
            FilletObject::Body(FilletBody::Target) => Some((
                InsertionBody::Target,
                EntityRef::Brep(BrepId::from_index(0)),
            )),
            FilletObject::Body(FilletBody::Pseudo) => Some((
                InsertionBody::Pseudo,
                EntityRef::Brep(BrepId::from_index(0)),
            )),
            FilletObject::IntermediateBrep => Some((
                InsertionBody::Intermediate,
                EntityRef::Brep(BrepId::from_index(0)),
            )),
            FilletObject::Topology(FilletBody::Target, e) => Some((InsertionBody::Target, e)),
            FilletObject::Topology(FilletBody::Pseudo, e) => Some((InsertionBody::Pseudo, e)),
            FilletObject::IntermediateTopology(e) => Some((InsertionBody::Intermediate, e)),
            _ => None,
        }
    }
    fn handle(role: InsertionBody, entity: EntityRef) -> FilletObject {
        if matches!(entity, EntityRef::Brep(_)) {
            return match role {
                InsertionBody::Target => FilletObject::Body(FilletBody::Target),
                InsertionBody::Pseudo => FilletObject::Body(FilletBody::Pseudo),
                InsertionBody::Intermediate => FilletObject::IntermediateBrep,
            };
        }
        match role {
            InsertionBody::Target => FilletObject::Topology(FilletBody::Target, entity),
            InsertionBody::Pseudo => FilletObject::Topology(FilletBody::Pseudo, entity),
            InsertionBody::Intermediate => FilletObject::IntermediateTopology(entity),
        }
    }
    fn role_of(&self, brep: &Brep) -> Option<InsertionBody> {
        [
            InsertionBody::Target,
            InsertionBody::Pseudo,
            InsertionBody::Intermediate,
        ]
        .into_iter()
        .find(|&role| std::ptr::eq(self.body(role), brep))
    }
    /// Capture the original Surface through its real Context identity slot.
    /// # Errors
    /// Missing actual Surface or unavailable identity issuance.
    pub fn capture_surface(
        &mut self,
        context: &mut Context,
        role: InsertionBody,
        face: FaceId,
    ) -> Result<SurfaceBinding> {
        SurfaceBinding::capture_in_body(context, self.body_mut(role), role, face)
    }
    /// Resolve that same Surface instance; a replacement in the Face slot fails.
    /// # Errors
    /// Foreign body, deleted Face/Surface or replaced geometry identity.
    pub fn bound_surface(&self, binding: SurfaceBinding) -> Result<&Surface> {
        binding.resolve_in_body(self.body(binding.role))
    }
    /// `SmSurface::GetFace()` of a bound face-slot surface: its owner when that is a
    /// Face, `None` for no owner or an owner of another kind.
    /// # Errors
    /// Missing original Surface, or an owner no insertion body resolves to one
    /// entity.
    pub fn surface_face(&self, binding: SurfaceBinding) -> Result<Option<FilletObject>> {
        self.face_slot_surface_face(self.bound_surface(binding)?)
    }
    /// `SmSurface::GetFace()` of a surface held in an insertion body's face slot (a
    /// bound original or a face's current surface).
    /// # Errors
    /// An owner no insertion body resolves to one entity.
    fn face_slot_surface_face(&self, surface: &Surface) -> Result<Option<FilletObject>> {
        let Some(owner) = surface.owner() else {
            return Ok(None);
        };
        match self.owner_binding(owner) {
            Some((role, entity @ EntityRef::Face(_))) => Ok(Some(Self::handle(role, entity))),
            // Not `IsKindOf(SmFace_TYPE)` and not a surface: the owner walk stops.
            Some(_) => Ok(None),
            // A face-slot surface is owned by its own face (`SetSurface` sets the
            // owner, and the face's B-rep is one of the insertion bodies): an owner no
            // insertion body resolves (another B-rep's, a geometry's or an ambiguous
            // key) has no upstream counterpart.
            None => Err(invalid(
                "face-slot Surface owner outside the insertion bodies",
            )),
        }
    }
    fn owner_binding(&self, owner: ObjectRef) -> Option<(InsertionBody, EntityRef)> {
        let mut found = None;
        for role in [
            InsertionBody::Target,
            InsertionBody::Pseudo,
            InsertionBody::Intermediate,
        ] {
            let body = self.body(role);
            let serial = body.notify_serial()?;
            let canonical = body
                .faces()
                .ids()
                .map(EntityRef::Face)
                .chain(body.edges().ids().map(EntityRef::Edge))
                .chain(body.vertices().ids().map(EntityRef::Vertex));
            let mut matches = Vec::new();
            for entity in canonical {
                if topology_ref(serial, entity) == owner {
                    matches.push(entity);
                }
            }
            for (&entity, &producer) in body.geometry_owner_keys() {
                let live = match entity {
                    EntityRef::Face(id) => body.face(id).is_ok(),
                    EntityRef::Edge(id) => body.edge(id).is_ok(),
                    _ => false,
                };
                if live && producer == owner && !matches.contains(&entity) {
                    matches.push(entity);
                }
            }
            for entity in matches {
                if found.is_some_and(|actual| actual != (role, entity)) {
                    return None;
                }
                found = Some((role, entity));
            }
        }
        found
    }
}

/// The insertion role of a body-qualified geometry handle. A second base B-rep is
/// never one of an insertion view's bodies (its rails merge with it as the target).
fn insertion_role(body: FilletBody) -> Option<InsertionBody> {
    match body {
        FilletBody::Target => Some(InsertionBody::Target),
        FilletBody::Pseudo => Some(InsertionBody::Pseudo),
        FilletBody::SecondBase => None,
    }
}

impl ClassGeometry<FilletObject> for InsertionGeometry<'_> {
    fn curve(&self, object: FilletObject) -> Option<&Curve> {
        match object {
            FilletObject::InsertionCurve(binding) => self.curves.as_ref()?.get(binding),
            FilletObject::Curve(body, edge) => self
                .body(insertion_role(body)?)
                .edge(edge)
                .ok()?
                .curve
                .as_ref(),
            FilletObject::IntermediateCurve(edge) => {
                self.intermediate.edge(edge).ok()?.curve.as_ref()
            }
            FilletObject::UvCurve(body, eu) => self
                .body(insertion_role(body)?)
                .edgeuse(eu)
                .ok()?
                .uv_trim_curve
                .as_ref(),
            FilletObject::IntermediateUvCurve(eu) => {
                self.intermediate.edgeuse(eu).ok()?.uv_trim_curve.as_ref()
            }
            _ => None,
        }
    }
    fn surface(&self, object: FilletObject) -> Option<&Surface> {
        match object {
            FilletObject::Surface(binding) => self.bound_surface(binding).ok(),
            FilletObject::FaceSurface(role, face) => {
                self.body(role).face(face).ok()?.surface.as_ref()
            }
            _ => None,
        }
    }
    fn volume(&self, _: FilletObject) -> Option<&Volume> {
        None
    }
    fn point(&self, object: FilletObject) -> Option<Point3> {
        let (role, EntityRef::Vertex(vertex)) = Self::topology(object)? else {
            return None;
        };
        Some(self.body(role).vertex(vertex).ok()?.point)
    }
    fn classification_vertex_point(&self, object: FilletObject) -> Option<Point3> {
        self.point(object)
    }
    fn topology_zone_tol_3d(&self, object: FilletObject) -> Option<f64> {
        let (role, entity) = Self::topology(object)?;
        let body = self.body(role);
        match entity {
            EntityRef::Vertex(id) => Some(body.vertex(id).ok()?.tolerance),
            EntityRef::Edge(id) => Some(body.edge(id).ok()?.tolerance),
            EntityRef::Face(id) => Some(body.face(id).ok()?.tolerance),
            EntityRef::Edgeuse(id) => Some(body.edge(body.edgeuse(id).ok()?.edge?).ok()?.tolerance),
            _ => None,
        }
    }
    fn topology_entity(&self, object: FilletObject) -> Option<(&Brep, EntityRef)> {
        let (role, entity) = Self::topology(object)?;
        Some((self.body(role), entity))
    }
    fn topology_handle_in(&self, brep: &Brep, entity: EntityRef) -> Option<FilletObject> {
        Some(Self::handle(self.role_of(brep)?, entity))
    }
    fn owner_entity(&self, owner: ObjectRef) -> Option<(&Brep, EntityRef)> {
        let (role, entity) = self.owner_binding(owner)?;
        Some((self.body(role), entity))
    }
    fn brep_id(&self, brep: &Brep) -> Option<BrepId> {
        self.role_of(brep)?;
        Some(BrepId::from_index(
            u32::try_from(brep.notify_serial()?).ok()?,
        ))
    }
}
impl ClassGeometryMut<FilletObject> for InsertionGeometry<'_> {
    fn set_filleting_edge_curve(&mut self, object: FilletObject, curve: Curve) -> Result<()> {
        self.replace_filleting_curve(object, curve)
    }
    fn curve_mut(&mut self, object: FilletObject) -> Option<&mut Curve> {
        match object {
            FilletObject::InsertionCurve(binding) => self.curves.as_mut()?.get_mut(binding),
            FilletObject::Curve(body, edge) => self
                .body_mut(insertion_role(body)?)
                .edge_mut(edge)
                .ok()?
                .curve
                .as_mut(),
            FilletObject::IntermediateCurve(edge) => {
                self.intermediate.edge_mut(edge).ok()?.curve.as_mut()
            }
            FilletObject::UvCurve(body, eu) => self
                .body_mut(insertion_role(body)?)
                .edgeuse_mut(eu)
                .ok()?
                .uv_trim_curve
                .as_mut(),
            FilletObject::IntermediateUvCurve(eu) => self
                .intermediate
                .edgeuse_mut(eu)
                .ok()?
                .uv_trim_curve
                .as_mut(),
            _ => None,
        }
    }
    fn topology_entity_mut(&mut self, object: FilletObject) -> Option<(&mut Brep, EntityRef)> {
        let (role, entity) = Self::topology(object)?;
        Some((self.body_mut(role), entity))
    }
}

impl ProtoGeometry<FilletObject> for InsertionGeometry<'_> {
    fn curve_mut(&mut self, object: FilletObject) -> Option<&mut Curve> {
        ClassGeometryMut::curve_mut(self, object)
    }
}

impl ProtoTopology<FilletObject> for InsertionGeometry<'_> {
    fn vertex_use_faces(&self, vertex: FilletObject) -> Result<Vec<Option<FilletObject>>> {
        let (role, entity) = Self::topology(vertex).ok_or(invalid("unknown topology role"))?;
        Ok(ProtoTopology::vertex_use_faces(self.body(role), entity)?
            .into_iter()
            .map(|value| value.map(|entity| Self::handle(role, entity)))
            .collect())
    }
    fn edge_use_faces(&self, edge: FilletObject) -> Result<Vec<Option<FilletObject>>> {
        let (role, entity) = Self::topology(edge).ok_or(invalid("unknown topology role"))?;
        Ok(ProtoTopology::edge_use_faces(self.body(role), entity)?
            .into_iter()
            .map(|value| value.map(|entity| Self::handle(role, entity)))
            .collect())
    }
    fn edge_start_vertex(&self, edge: FilletObject) -> Result<Option<FilletObject>> {
        let (role, entity) = Self::topology(edge).ok_or(invalid("unknown topology role"))?;
        Ok(ProtoTopology::edge_start_vertex(self.body(role), entity)?
            .map(|entity| Self::handle(role, entity)))
    }
    fn edge_other_vertex(
        &self,
        edge: FilletObject,
        vertex: Option<FilletObject>,
    ) -> Result<Option<FilletObject>> {
        let (role, entity) = Self::topology(edge).ok_or(invalid("unknown topology role"))?;
        let vertex = vertex
            .map(|vertex| {
                let (other_role, vertex) =
                    Self::topology(vertex).ok_or(invalid("unknown vertex role"))?;
                if role != other_role {
                    return Err(invalid("edge and vertex belong to different bodies"));
                }
                Ok(vertex)
            })
            .transpose()?;
        Ok(
            ProtoTopology::edge_other_vertex(self.body(role), entity, vertex)?
                .map(|entity| Self::handle(role, entity)),
        )
    }
    fn vertex_point(&self, vertex: FilletObject) -> Result<Point3> {
        let (role, entity) = Self::topology(vertex).ok_or(invalid("unknown topology role"))?;
        ProtoTopology::vertex_point(self.body(role), entity)
    }
    fn edge_brep(&self, edge: FilletObject) -> Result<Option<ObjectKey>> {
        let (role, entity) = Self::topology(edge).ok_or(invalid("unknown topology role"))?;
        let actual_body = ProtoTopology::edge_brep(self.body(role), entity)?;
        actual_body
            .map(|_| self.body_identity(role).map(|identity| identity.key))
            .transpose()
    }
    fn edge_faces(&self, edge: FilletObject) -> Result<Vec<FilletObject>> {
        let (role, entity) = Self::topology(edge).ok_or(invalid("unknown topology role"))?;
        Ok(ProtoTopology::edge_faces(self.body(role), entity)?
            .into_iter()
            .map(|entity| Self::handle(role, entity))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blend::fillet_geom::p1::FilletGeomId;
    use crate::primitive::api_primitives::create_box;
    use crate::Kernel;

    fn box_at(k: &mut Kernel, x: f64) -> Brep {
        let mut b = create_box(k, Point3::new(x, 0.0, 0.0), 10.0, 20.0, 30.0).unwrap();
        crate::topology::brep::p3::notify_serial(&mut k.context, &mut b).unwrap();
        b
    }

    #[test]
    fn actual_equal_local_ids_keep_body_and_geometry_identity() {
        let mut k = Kernel::default();
        let mut target = box_at(&mut k, 0.0);
        let mut intermediate = box_at(&mut k, 100.0);
        let mut pseudo = FilletBrep::new();
        *pseudo.brep_mut() = box_at(&mut k, 200.0);
        let mut view = InsertionGeometry::new(&mut target, &mut pseudo, &mut intermediate).unwrap();
        let face = FaceId::from_index(0);
        let a = view
            .capture_surface(&mut k.context, InsertionBody::Target, face)
            .unwrap();
        let b = view
            .capture_surface(&mut k.context, InsertionBody::Intermediate, face)
            .unwrap();
        assert_ne!(a, b);
        assert_ne!(a.identity(), b.identity());
        assert!(std::ptr::eq(
            view.bound_surface(a).unwrap(),
            view.target.face(face).unwrap().surface.as_ref().unwrap()
        ));
        let edge = view
            .target
            .edges()
            .ids()
            .find(|&id| view.target.edge(id).unwrap().curve.is_some())
            .unwrap();
        assert!(!std::ptr::eq(
            view.curve(FilletObject::Curve(FilletBody::Target, edge))
                .unwrap(),
            view.curve(FilletObject::IntermediateCurve(edge)).unwrap()
        ));
        let vertex = view.target.vertices().ids().next().unwrap();
        let target_edge = FilletObject::Topology(FilletBody::Target, EntityRef::Edge(edge));
        let start = view.edge_start_vertex(target_edge).unwrap().unwrap();
        assert!(matches!(
            start,
            FilletObject::Topology(FilletBody::Target, EntityRef::Vertex(_))
        ));
        assert!(view
            .edge_faces(target_edge)
            .unwrap()
            .iter()
            .all(|face| matches!(
                face,
                FilletObject::Topology(FilletBody::Target, EntityRef::Face(_))
            )));
        let faces = view.edge_faces(target_edge).unwrap();
        let actual_face = faces.first().copied().unwrap();
        assert!(view
            .edge_is_connected_to_face(target_edge, actual_face)
            .unwrap());
        assert!(view
            .edge_edgeuse_of_face(target_edge, actual_face)
            .unwrap()
            .is_some());
        let FilletObject::Topology(_, EntityRef::Face(face_id)) = actual_face else {
            panic!("actual Face expected")
        };
        let foreign_face = FilletObject::IntermediateTopology(EntityRef::Face(face_id));
        assert!(!view
            .edge_is_connected_to_face(target_edge, foreign_face)
            .unwrap());
        assert!(view
            .edge_edgeuse_of_face(target_edge, foreign_face)
            .unwrap()
            .is_none());
        let current = view.face_surface(actual_face).unwrap().unwrap();
        assert!(std::ptr::eq(
            view.surface(current).unwrap(),
            view.target.face(face_id).unwrap().surface.as_ref().unwrap()
        ));
        let foreign = FilletObject::Topology(FilletBody::Pseudo, EntityRef::Vertex(vertex));
        assert!(view.edge_other_vertex(target_edge, Some(foreign)).is_err());
        let inside_target = view
            .brep_point_3d_classify(
                &k,
                FilletObject::Body(FilletBody::Target),
                Point3::new(5.0, 10.0, 15.0),
                1e-5,
                false,
            )
            .unwrap();
        let inside_intermediate = view
            .brep_point_3d_classify(
                &k,
                FilletObject::IntermediateBrep,
                Point3::new(105.0, 10.0, 15.0),
                1e-5,
                false,
            )
            .unwrap();
        assert_eq!(
            inside_target.point_class(),
            crate::intersect::point_class::PointClassType::Region
        );
        assert_eq!(
            inside_intermediate.point_class(),
            crate::intersect::point_class::PointClassType::Region
        );
        assert!(matches!(
            inside_target.object(),
            Some(FilletObject::Topology(
                FilletBody::Target,
                EntityRef::Region(_)
            ))
        ));
        assert!(matches!(
            inside_intermediate.object(),
            Some(FilletObject::IntermediateTopology(EntityRef::Region(_)))
        ));
        let before = view.target.vertex(vertex).unwrap().point;
        let object = FilletObject::Topology(FilletBody::Pseudo, EntityRef::Vertex(vertex));
        let (body, _) = view.topology_entity_mut(object).unwrap();
        body.vertex_mut(vertex).unwrap().point.x += 7.0;
        assert_eq!(view.target.vertex(vertex).unwrap().point, before);
        assert_ne!(view.pseudo.brep().vertex(vertex).unwrap().point, before);
    }

    #[test]
    fn topology_solver_preserves_body_roles_and_does_not_infer_curve_ownership() {
        use crate::intersect::topology_solver::p1::TopologyObjects;
        let mut k = Kernel::default();
        let mut target = box_at(&mut k, 0.0);
        let mut intermediate = box_at(&mut k, 100.0);
        let mut pseudo = FilletBrep::new();
        *pseudo.brep_mut() = box_at(&mut k, 200.0);
        let edge = target
            .edges()
            .ids()
            .find(|&e| target.edge(e).unwrap().curve.is_some())
            .unwrap();
        let vertex = target.vertices().ids().next().unwrap();
        let view = InsertionGeometry::new(&mut target, &mut pseudo, &mut intermediate).unwrap();
        let target_vertex = FilletObject::Topology(FilletBody::Target, EntityRef::Vertex(vertex));
        let other_vertex = FilletObject::IntermediateTopology(EntityRef::Vertex(vertex));
        let first = TopologyObjects::vertex_point(&view, target_vertex).unwrap();
        let second = TopologyObjects::vertex_point(&view, other_vertex).unwrap();
        assert_eq!(second.x - first.x, 100.0);
        let curve = FilletObject::Curve(FilletBody::Target, edge);
        view.target
            .edge_mut(edge)
            .unwrap()
            .curve
            .as_mut()
            .unwrap()
            .set_owner(None, None);
        assert_eq!(TopologyObjects::curve_edge(&view, curve).unwrap(), None);
        let foreign = ObjectRef {
            type_code: -123,
            key: ObjectKey(u64::MAX),
        };
        view.target
            .edge_mut(edge)
            .unwrap()
            .curve
            .as_mut()
            .unwrap()
            .set_owner(Some(foreign), None);
        // Not an edge, curve or edge use: SmCurve::GetEdge's walk stops (NULL).
        assert_eq!(TopologyObjects::curve_edge(&view, curve).unwrap(), None);
        assert_eq!(
            ClassGeometry::curve(&view, curve).unwrap().owner(),
            Some(foreign)
        );
        assert!(
            TopologyObjects::brep_cache(&view, FilletObject::Body(FilletBody::Target)).is_err()
        );
    }

    #[test]
    fn bound_surface_face_follows_upstream_get_face() {
        let mut k = Kernel::default();
        let mut target = box_at(&mut k, 0.0);
        let mut intermediate = box_at(&mut k, 100.0);
        let mut pseudo = FilletBrep::new();
        *pseudo.brep_mut() = box_at(&mut k, 200.0);
        let mut view = InsertionGeometry::new(&mut target, &mut pseudo, &mut intermediate).unwrap();
        let face = FaceId::from_index(0);
        let binding = view
            .capture_surface(&mut k.context, InsertionBody::Target, face)
            .unwrap();
        let serial = view.target.notify_serial().unwrap();
        let set_owner = |view: &mut InsertionGeometry<'_>, owner| {
            view.target
                .face_mut(face)
                .unwrap()
                .surface
                .as_mut()
                .unwrap()
                .set_owner(owner);
        };
        // Its own face.
        set_owner(&mut view, Some(topology_ref(serial, EntityRef::Face(face))));
        assert_eq!(
            view.surface_face(binding).unwrap(),
            Some(FilletObject::Topology(
                FilletBody::Target,
                EntityRef::Face(face)
            ))
        );
        // No owner, or an owner that is not a face: `GetFace()` is NULL.
        set_owner(&mut view, None);
        assert_eq!(view.surface_face(binding).unwrap(), None);
        let vertex = view.target.vertices().ids().next().unwrap();
        set_owner(
            &mut view,
            Some(topology_ref(serial, EntityRef::Vertex(vertex))),
        );
        assert_eq!(view.surface_face(binding).unwrap(), None);
        // An owner no insertion body holds is a broken binding, not a missing port.
        set_owner(
            &mut view,
            Some(ObjectRef {
                type_code: -123,
                key: ObjectKey(u64::MAX),
            }),
        );
        let error = view.surface_face(binding).unwrap_err();
        assert!(!error.is_unported());
    }

    #[test]
    fn actual_surface_replacement_cannot_reuse_captured_binding() {
        let mut k = Kernel::default();
        let mut target = box_at(&mut k, 0.0);
        let mut intermediate = box_at(&mut k, 100.0);
        let mut replacement = box_at(&mut k, 300.0);
        let mut pseudo = FilletBrep::new();
        *pseudo.brep_mut() = box_at(&mut k, 200.0);
        let mut view = InsertionGeometry::new(&mut target, &mut pseudo, &mut intermediate).unwrap();
        let face = FaceId::from_index(0);
        let binding = view
            .capture_surface(&mut k.context, InsertionBody::Intermediate, face)
            .unwrap();
        let actual = replacement.face_mut(face).unwrap().surface.take();
        view.intermediate.face_mut(face).unwrap().surface = actual;
        assert!(view.bound_surface(binding).is_err());
        let current = view
            .face_surface(FilletObject::IntermediateTopology(EntityRef::Face(face)))
            .unwrap()
            .unwrap();
        assert!(std::ptr::eq(
            view.surface(current).unwrap(),
            view.intermediate
                .face(face)
                .unwrap()
                .surface
                .as_ref()
                .unwrap()
        ));
    }
    #[test]
    fn retained_curves_and_lent_notifications_survive_partial_edit_failure() {
        use crate::geom::context::NotifyCallback;
        use crate::geom::object::NotifyOperation;
        use std::sync::{Arc, Mutex};
        #[derive(Debug)]
        struct Capture(Arc<Mutex<Vec<(NotifyOperation, ObjectRef)>>>);
        impl NotifyCallback for Capture {
            fn execute(
                &mut self,
                op: NotifyOperation,
                caller: ObjectRef,
                _one: Option<ObjectRef>,
                _two: Option<ObjectRef>,
                _three: Option<ObjectRef>,
            ) -> Result<()> {
                self.0.lock().unwrap().push((op, caller));
                Ok(())
            }
        }
        let mut k = Kernel::default();
        let mut target = box_at(&mut k, 0.0);
        let mut intermediate = box_at(&mut k, 100.0);
        let mut source = box_at(&mut k, 300.0);
        let edge = source
            .edges()
            .ids()
            .find(|&id| source.edge(id).unwrap().curve.is_some())
            .unwrap();
        let curve = source.edge_mut(edge).unwrap().curve.take().unwrap();
        let owner = curve.owner();
        let mut pseudo = FilletBrep::new();
        *pseudo.brep_mut() = box_at(&mut k, 200.0);
        let vertex = target.vertices().ids().next().unwrap();
        let serial = target.notify_serial().unwrap();
        let caller = topology_ref(serial, EntityRef::Vertex(vertex));
        let changed = Point3::new(17.0, 18.0, 19.0);
        let mut curves = InsertionCurves::default();
        let mut records = crate::topology::topology::NotifyRecords::new();
        let mut attributes = AttributeStore::default();
        let events = Arc::new(Mutex::new(Vec::new()));
        k.context
            .set_user_notify_callback(Some(Box::new(Capture(events.clone()))));
        let binding = crate::topology::brep::p3::with_lent_context(
            &mut k,
            |k, context| -> Result<FilletObject> {
                let mut view = InsertionGeometry::new(&mut target, &mut pseudo, &mut intermediate)?
                    .with_notifications(
                        &mut curves,
                        InsertionNotifications {
                            records: &mut records,
                            attributes: &mut attributes,
                            context,
                        },
                    )?;
                let foreign = FilletObject::Topology(FilletBody::Pseudo, EntityRef::Vertex(vertex));
                let own = FilletObject::Topology(FilletBody::Target, EntityRef::Vertex(vertex));
                let rejected = crate::topology::proto_topology::p3::BrepGlue::glue_vertices(
                    &mut view,
                    k,
                    FilletObject::Body(FilletBody::Target),
                    own,
                    foreign,
                    None,
                );
                assert!(rejected.is_err());
                let binding = view.insert_curve_copy(curve)?;
                assert_eq!(view.curve(binding).unwrap().owner(), owner);
                assert!(ClassGeometryMut::curve_mut(&mut view, binding).is_some());
                let result: Result<()> = view.with_edit(InsertionBody::Target, |body, env| {
                    let vu_ref = |id| topology_ref(serial, EntityRef::Vertexuse(id));
                    crate::topology::vertex::set_point(
                        body,
                        vertex,
                        changed,
                        env.records,
                        env.attributes,
                        env.context.as_deref_mut(),
                        caller,
                        &vu_ref,
                    )?;
                    Err(invalid("deliberate failure after actual vertex edit"))
                });
                assert!(result.is_err());
                assert_eq!(view.target.vertex(vertex)?.point, changed);
                Ok(binding)
            },
        )
        .unwrap()
        .unwrap();
        let FilletObject::InsertionCurve(binding) = binding else {
            panic!("actual curve binding required")
        };
        assert_eq!(curves.checked(binding).unwrap().owner(), owner);
        assert!(InsertionCurves::default().get(binding).is_none());
        assert!(records.contains_key(&caller.key));
        assert!(events
            .lock()
            .unwrap()
            .iter()
            .any(|&(op, object)| op == NotifyOperation::ChangeGeometry && object == caller));
    }
    #[test]
    fn foreign_notification_domain_is_rejected_without_reissuing_body_identity() {
        let mut k = Kernel::default();
        let mut foreign = Kernel::default();
        let mut target = box_at(&mut k, 0.0);
        let mut intermediate = box_at(&mut k, 100.0);
        let mut pseudo = FilletBrep::new();
        *pseudo.brep_mut() = box_at(&mut k, 200.0);
        let serials = [
            target.notify_serial(),
            pseudo.brep().notify_serial(),
            intermediate.notify_serial(),
        ];
        let mut curves = InsertionCurves::default();
        let mut records = crate::topology::topology::NotifyRecords::new();
        let mut attributes = AttributeStore::default();
        let result = InsertionGeometry::new(&mut target, &mut pseudo, &mut intermediate)
            .unwrap()
            .with_notifications(
                &mut curves,
                InsertionNotifications {
                    records: &mut records,
                    attributes: &mut attributes,
                    context: &mut foreign.context,
                },
            );
        assert!(result.is_err());
        drop(result);
        assert_eq!(
            [
                target.notify_serial(),
                pseudo.brep().notify_serial(),
                intermediate.notify_serial()
            ],
            serials
        );
        assert!(records.is_empty());
    }
    #[test]
    fn actual_constructed_fillet_rail_classifies_on_its_original_face() {
        use crate::blend::fillet_executive::session::FilletSession;
        use crate::intersect::curve_class::CurveClassification;
        let mut k = Kernel::default();
        let mut target = box_at(&mut k, 0.0);
        let edge = target.edges().ids().next().unwrap();
        let mut session = FilletSession::new(&mut k, &mut target).unwrap();
        session
            .prepare_constant_radius_edges(&mut k, &[edge], 0.5, 1, 1, 1.0)
            .unwrap();
        session.create_corners(&k, None).unwrap();
        session.create_fillet_brep(&mut k, None).unwrap();
        session.shrink_fillet_faces(&mut k).unwrap();
        session.set_fillet_brep_edge_map(&mut k).unwrap();
        session
            .with_insertion_geometry(&mut k, |k, _, geometry| {
                let rail = geometry.pseudo.geom(FilletGeomId(0))?.rail(0).unwrap();
                let record = geometry.pseudo.fillet_edge(rail)?;
                let face = record.original_face().unwrap();
                let managed = record.fillet_brep_edge().unwrap();
                let tolerance = geometry.intermediate.edge(managed)?.tolerance;
                let curve = FilletObject::Curve(FilletBody::Pseudo, rail);
                let interval = geometry.curve(curve).unwrap().natural_interval();
                let mut cc =
                    CurveClassification::with_curve_interval(k, curve, interval, tolerance);
                let face = FilletObject::Topology(FilletBody::Target, EntityRef::Face(face));
                cc.set_surface1(geometry.face_surface(face).unwrap());
                geometry.face_curve_on_classify_mut(k, face, true, &mut cc)?;
                assert!(cc.size() > 0);
                Ok(())
            })
            .unwrap();
    }
    #[test]
    fn actual_constructed_first_rail_merges_with_the_retained_intersector() {
        use crate::blend::fillet_executive::session::FilletSession;
        use crate::boolean::topology_intersector::p3::CurveOnSurfaces;
        let mut k = Kernel::default();
        let mut target = box_at(&mut k, 0.0);
        let edge = target.edges().ids().next().unwrap();
        let mut session = FilletSession::new(&mut k, &mut target).unwrap();
        session
            .prepare_constant_radius_edges(&mut k, &[edge], 0.5, 1, 1, 1.0)
            .unwrap();
        session.create_corners(&k, None).unwrap();
        k.context.set_doing_boolean(true);
        session.executive.intersector.doing_fillets = true;
        session.create_fillet_brep(&mut k, None).unwrap();
        session.shrink_fillet_faces(&mut k).unwrap();
        session.set_fillet_brep_edge_map(&mut k).unwrap();
        let result = session.with_insertion_geometry(&mut k, |k, ti, geometry| {
            let rail = geometry.pseudo.geom(FilletGeomId(0))?.rail(0).unwrap();
            let record = geometry.pseudo.fillet_edge(rail)?;
            let face = record.original_face().unwrap();
            let managed = record.fillet_brep_edge().unwrap();
            let tolerance = geometry.intermediate.edge(managed)?.tolerance;
            let primary =
                crate::topology::edgeuse::p1::primary_edgeuse(geometry.pseudo.brep(), rail)?
                    .unwrap();
            let mate =
                crate::topology::edgeuse::p1::mate(geometry.pseudo.brep(), primary)?.unwrap();
            let uv = geometry
                .pseudo
                .original_uv_curve(rail)?
                .map(|_| FilletObject::UvCurve(FilletBody::Pseudo, mate));
            let face = FilletObject::Topology(FilletBody::Target, EntityRef::Face(face));
            ti.set_primary_brep(Some(FilletObject::Body(FilletBody::Target)));
            ti.set_other_brep(Some(FilletObject::IntermediateBrep));
            geometry.target.set_editing_enabled(true);
            let input = CurveOnSurfaces {
                surface: geometry.face_surface(face).unwrap(),
                other_surface: None,
                curve_tolerance: tolerance,
                curve: FilletObject::Curve(FilletBody::Pseudo, rail),
                uv_curve1: uv,
                uv_curve2: None,
                brep_edge: None,
                other_edge: Some(FilletObject::IntermediateTopology(EntityRef::Edge(managed))),
                ti: Some(FilletObject::Intersector),
                classifications: None,
            };
            let attempt = ti.merge_curve_on_surfaces_linked_attempt(k, geometry, &input);
            assert!(crate::topology::brep::p1::vertices(geometry.target, None)?.len() > 8);
            assert!(crate::topology::brep::p1::edges(geometry.target, None)?.len() > 12);
            assert!(crate::topology::brep::p1::faces(geometry.target, None)?.len() > 6);
            attempt.status?;
            assert!(!attempt.output.deleted_topology);
            assert_eq!(attempt.output.deleted_edges, Some(Vec::new()));
            assert_eq!(attempt.output.deleted_vertices, Some(Vec::new()));
            assert!(attempt.output.deleted_curves.is_empty());
            assert_eq!(
                ti.primary_brep(),
                Some(FilletObject::Body(FilletBody::Target))
            );
            assert_eq!(ti.other_brep(), Some(FilletObject::IntermediateBrep));
            Ok(())
        });
        assert!(result.is_ok(), "{result:?}");
    }

    #[test]
    fn actual_face_uv_classification_preserves_complete_outputs() {
        use crate::boolean::topology_intersector::p1::IntersectorBrep;
        use crate::intersect::point_class::{PointClassType, PointClassification};
        let mut k = Kernel::default();
        let mut target = box_at(&mut k, 0.0);
        let mut intermediate = box_at(&mut k, 100.0);
        let mut pseudo = FilletBrep::new();
        *pseudo.brep_mut() = box_at(&mut k, 200.0);
        let face = target.faces().ids().next().unwrap();
        for vertex in crate::topology::face::p4::vertices(&target, face, None, false, None).unwrap()
        {
            target.vertex_mut(vertex).unwrap().tolerance = 0.003;
        }
        let uv = target
            .face(face)
            .unwrap()
            .surface
            .as_ref()
            .unwrap()
            .natural_uv_domain_if_built()
            .min();
        let face_handle = FilletObject::Topology(FilletBody::Target, EntityRef::Face(face));
        let mut geometry =
            InsertionGeometry::new(&mut target, &mut pseudo, &mut intermediate).unwrap();
        let mut pc = PointClassification::new(1e-7);
        let uvw = Point3::new(3.0, 4.0, 5.0);
        pc.set_uvw_param(uvw);
        IntersectorBrep::face_point_classify(&mut geometry, &k, face_handle, uv, &mut pc).unwrap();
        assert_eq!(pc.point_class(), PointClassType::Vertex);
        assert_eq!(pc.src_zone_tol_3d(), 0.003);
        assert_eq!(pc.uvw_param(), uvw);
        geometry.target.face_mut(face).unwrap().surface = None;
        let prior_source = pc.src_zone_tol_3d();
        assert!(
            IntersectorBrep::face_point_classify(&mut geometry, &k, face_handle, uv, &mut pc)
                .is_err()
        );
        assert_eq!(pc.src_zone_tol_3d(), prior_source);
        assert_eq!(pc.uvw_param(), uvw);
    }

    #[test]
    fn actual_point_imprint_split_issues_identity_on_the_retained_curve_objects() {
        use crate::intersect::curve_class::p6::merge_into_object_mut;
        use crate::intersect::point_class::{PointClassType, PointClassification};
        let mut k = Kernel::default();
        let mut target = box_at(&mut k, 0.0);
        target.set_editing_enabled(true);
        let edge = target
            .edges()
            .ids()
            .find(|&id| target.edge(id).unwrap().curve.is_some())
            .unwrap();
        let interval = crate::topology::edge::interval(&target, edge).unwrap();
        let parameter = interval.evaluate(0.5);
        let point = target
            .edge(edge)
            .unwrap()
            .curve
            .as_ref()
            .unwrap()
            .evaluate_point(&k, parameter)
            .unwrap();
        let mut intermediate = box_at(&mut k, 100.0);
        let mut pseudo = FilletBrep::new();
        *pseudo.brep_mut() = box_at(&mut k, 200.0);
        let mut curves = InsertionCurves::default();
        let mut records = crate::topology::topology::NotifyRecords::new();
        let mut attributes = AttributeStore::default();
        crate::topology::brep::p3::with_lent_context(&mut k, |k, context| -> Result<()> {
            let mut view = InsertionGeometry::new(&mut target, &mut pseudo, &mut intermediate)?
                .with_notifications(
                    &mut curves,
                    InsertionNotifications {
                        records: &mut records,
                        attributes: &mut attributes,
                        context,
                    },
                )?;
            let mut pc = PointClassification::new(1e-5);
            pc.set_class_object(
                PointClassType::Edge,
                Some(FilletObject::Topology(
                    FilletBody::Target,
                    EntityRef::Edge(edge),
                )),
            );
            pc.set_t_param(parameter, false);
            let output = merge_into_object_mut(k, &mut view, &mut pc, point)?;
            assert_eq!(output.new_edges.len(), 2);
            assert_eq!(output.new_vertices.len(), 1);
            let mut identities = Vec::new();
            for handle in output.new_edges {
                let FilletObject::Topology(FilletBody::Target, EntityRef::Edge(edge)) = handle
                else {
                    panic!("actual target Edge required")
                };
                let curve = view.target.edge(edge)?.curve.as_ref().unwrap();
                identities.push(curve.notify_key().unwrap());
            }
            assert_ne!(identities.first(), identities.get(1));
            let vertex = output.new_vertices.first().copied().unwrap();
            assert_eq!(view.point(vertex), Some(point));
            Ok(())
        })
        .unwrap()
        .unwrap();
    }
}
