//! Production insertion context borrowing the executive's actual retained stores.

use super::super::{
    invalid, AttributeHandle, AttributeStore, Brep, EdgeId, EntityRef, ErrorInfo, FaceId,
    FilletBody, FilletBrep, FilletCorner, FilletObject, FilletSession, Kernel, Result, SecondBase,
    SessionObjects, SolidError, SolverObject,
};
use super::geometry::{InsertionBody, InsertionGeometry, InsertionNotifications, SurfaceBinding};
use super::{
    begin_face_marker, edge_attribute_users, end_face_marker, FaceMarker, TargetTopologyChanges,
};
use crate::blend::fillet_executive::p3::{FilletExecutive, TopologyInsertionAccess};
use crate::blend::fillet_geom::p1::FilletErrorType;
use crate::blend::fillet_geom::p1::{FilletGeomId, FilletSolverId, FilletSurfaceStorage};
use crate::blend::fillet_geom::p4::{IntersectionFaceRole, IntersectionTopologyContext};
use crate::blend::fillet_solver::p1::FilletSolver;
use crate::blend::fillet_solver::p2::SolverErrorObject;
use crate::boolean::topology_intersector::{p1::TopologyIntersector, p3::CurveOnSurfaces};
use crate::geom::{context::Context, object::ObjectRef};
use crate::topology::{brep::p3::topology_ref, edgeuse, BrepId};

/// The fillet surface `SmFilletGeom::GetFilletSurface()` names during insertion: the
/// surface of the fillet B-rep face that took it over, or, when no face took it over,
/// the geometry's own surface. Upstream both are one `SmSurface*`; a surface no face
/// owns is only ever registered (`RegisterExistingSSI`, a pair no face loop queries)
/// and named in a merge error note.
#[derive(Debug, Clone, Copy)]
pub(super) enum InsertionSurface {
    /// The surface in a fillet B-rep face's slot.
    Face(SurfaceBinding),
    /// The surface the geometry still owns (`FilletSurfaceStorage::Owned`).
    Owned(FilletGeomId),
}
impl InsertionSurface {
    /// The object upstream's surface pointer stands for in SSI registrations and
    /// error notes. The geometry's own surface is named as the geometry's fillet
    /// surface, which no face's current surface ever equals.
    fn handle(self) -> FilletObject {
        match self {
            Self::Face(binding) => FilletObject::Surface(binding),
            Self::Owned(g) => FilletObject::SolverError(SolverErrorObject::FilletSurface(g)),
        }
    }
}

/// Captured actual Curve slot and its issued identity in this Session.
/// A replacement or relocation is an explicit capability boundary.
#[derive(Debug)]
pub(super) struct LiveCurveBinding {
    object: FilletObject,
    identity: ObjectRef,
}

pub(super) struct LiveGeometryInsertion<'a> {
    pub(super) k: &'a mut Kernel,
    pub(super) context: &'a mut Context,
    pub(super) ti: &'a mut TopologyIntersector<FilletObject>,
    pub(super) intermediate: &'a mut Brep,
    pub(super) curves: &'a mut super::curves::InsertionCurves,
    pub(super) attributes: &'a mut AttributeStore,
    pub(super) records: &'a mut crate::topology::topology::NotifyRecords,
    pub(super) solvers: &'a [Option<SolverObject>],
    pub(super) corners: &'a [FilletCorner],
    pub(super) solver_order: &'a [FilletObject],
    pub(super) corner_order: &'a [FilletObject],
    pub(super) error: &'a mut ErrorInfo<FilletObject>,
    pub(super) edge_attribute: AttributeHandle,
    pub(super) target_serial: u64,
    /// A surface/surface fillet's second base B-rep, its identity and its own
    /// intersector (lent to the second rails' merges by `with_rail_base`).
    pub(super) second_base: Option<(&'a mut Brep, u64, &'a mut TopologyIntersector<FilletObject>)>,
    /// The body the current merge's base face is recorded as in error notes.
    pub(super) base_label: FilletBody,
}
impl LiveGeometryInsertion<'_> {
    pub(super) fn capture_managed_surface(
        &mut self,
        face: FaceId,
    ) -> Result<Option<SurfaceBinding>> {
        if self.intermediate.face(face)?.surface.is_none() {
            return Ok(None);
        }
        SurfaceBinding::capture_in_body(
            self.context,
            self.intermediate,
            InsertionBody::Intermediate,
            face,
        )
        .map(Some)
    }
    pub(super) fn capture_managed_curve(
        &mut self,
        edge: EdgeId,
    ) -> Result<Option<LiveCurveBinding>> {
        let Some(curve) = self.intermediate.edge_mut(edge)?.curve.as_mut() else {
            return Ok(None);
        };
        let identity = ObjectRef {
            type_code: curve.type_code(),
            key: self.context.geometry_key(curve.notify_identity_slot())?,
        };
        Ok(Some(LiveCurveBinding {
            object: FilletObject::IntermediateCurve(edge),
            identity,
        }))
    }
    pub(super) fn capture_pseudo_curve(
        &mut self,
        pseudo: &mut FilletBrep,
        edge: EdgeId,
    ) -> Result<Option<LiveCurveBinding>> {
        let Some(curve) = pseudo.brep_mut().edge_mut(edge)?.curve.as_mut() else {
            return Ok(None);
        };
        let identity = ObjectRef {
            type_code: curve.type_code(),
            key: self.context.geometry_key(curve.notify_identity_slot())?,
        };
        Ok(Some(LiveCurveBinding {
            object: FilletObject::Curve(FilletBody::Pseudo, edge),
            identity,
        }))
    }
    fn with_geometry<T>(
        &mut self,
        pseudo: &mut FilletBrep,
        target: &mut Brep,
        call: impl FnOnce(
            &mut Kernel,
            &mut TopologyIntersector<FilletObject>,
            &mut InsertionGeometry<'_>,
        ) -> Result<T>,
    ) -> Result<T> {
        let mut geometry = InsertionGeometry::new(target, pseudo, self.intermediate)?
            .with_notifications(
                self.curves,
                InsertionNotifications {
                    records: self.records,
                    attributes: self.attributes,
                    context: self.context,
                },
            )?;
        call(self.k, self.ti, &mut geometry)
    }
    pub(super) fn marker_scope<T>(
        &mut self,
        target: &mut Brep,
        face: FaceId,
        id: u64,
        editing: bool,
        body: impl FnOnce(&mut Self, &mut Brep, &FaceMarker) -> Result<T>,
    ) -> Result<T> {
        let marker = begin_face_marker(self.attributes, target, face, id, editing)?;
        let result = body(self, target, &marker);
        let cleanup = end_face_marker(self.attributes, target, marker);
        match result {
            Err(error) => Err(error),
            Ok(value) => cleanup.map(|()| value),
        }
    }
    pub(super) fn update_changes(
        &mut self,
        pseudo: &mut FilletBrep,
        target: &mut Brep,
        marker: &FaceMarker,
        face: FaceId,
    ) -> Result<()> {
        let base_side = if self.base_label == FilletBody::SecondBase {
            Some(1)
        } else {
            self.second_base.as_ref().map(|_| 0)
        };
        let mut access = TargetTopologyChanges::new(
            self.target_serial,
            target,
            pseudo,
            self.attributes,
            self.solvers,
            self.corners,
        )?
        .with_base_side(base_side);
        crate::blend::fillet_executive::topology_changes::update_topology_changes(
            self.k,
            self.solver_order,
            self.corner_order,
            &mut access,
            FilletObject::Attribute(marker.attribute()),
            FilletObject::Topology(FilletBody::Target, EntityRef::Face(face)),
        )
    }
    pub(super) fn register_bound_intersection(
        &mut self,
        target: &mut Brep,
        face: FaceId,
        surface: Option<&SurfaceBinding>,
    ) -> Result<()> {
        if let Some(binding) = surface {
            binding.resolve_in_body(self.intermediate)?;
        }
        self.register_bound_handle(target, face, surface.map(SurfaceBinding::face_surface))
    }
    /// [`Self::register_bound_intersection`] with the other surface already named by
    /// its face-surface handle (that of a destroyed face for a dangling upstream
    /// pointer, which no face loop queries).
    pub(super) fn register_bound_handle(
        &mut self,
        target: &mut Brep,
        face: FaceId,
        other: Option<FilletObject>,
    ) -> Result<()> {
        let actual = if target.face(face)?.surface.is_some() {
            Some(
                SurfaceBinding::capture_in_body(self.context, target, InsertionBody::Target, face)?
                    .face_surface(),
            )
        } else {
            None
        };
        // The table is keyed by the handles IntersectInsertRelate's face loop
        // queries (`CheckIfHaveExistingSSI(pFace->GetSurface(), ..)`): the faces'
        // current surfaces, so a registered pair is found by a later global or
        // piecewise merge.
        self.ti.register_empty_ssi(actual, other);
        Ok(())
    }
    fn merge_actual_curve(
        &mut self,
        pseudo: &mut FilletBrep,
        target: &mut Brep,
        face: FaceId,
        tolerance: f64,
        curve: FilletObject,
        uv: Option<FilletObject>,
        managed: EdgeId,
        deleted: &mut Vec<EdgeId>,
    ) -> Result<()> {
        self.with_geometry(pseudo, target, |k, ti, geometry| {
            let input = CurveOnSurfaces {
                surface: geometry
                    .face_surface(FilletObject::Topology(
                        FilletBody::Target,
                        EntityRef::Face(face),
                    ))
                    .ok_or_else(|| invalid("original target Face missing"))?,
                other_surface: None,
                curve_tolerance: tolerance,
                curve,
                uv_curve1: uv,
                uv_curve2: None,
                brep_edge: None,
                other_edge: Some(FilletObject::IntermediateTopology(EntityRef::Edge(managed))),
                ti: Some(FilletObject::Intersector),
                classifications: None,
            };
            let attempt = ti.merge_curve_on_surfaces_linked_attempt(k, geometry, &input);
            // MergeCurveOnSurfaces deletes its temporary curves (`SmObjDelete
            // sCleanBSC`) on every return, before its status reaches this caller.
            for temporary in attempt.output.deleted_curves {
                geometry.destroy_curve_copy(temporary)?;
            }
            if let Some(edges) = attempt.output.deleted_edges {
                for edge in edges {
                    match edge {
                        FilletObject::IntermediateTopology(EntityRef::Edge(edge)) => {
                            deleted.push(edge);
                        }
                        FilletObject::Topology(FilletBody::Target, EntityRef::Edge(_)) => {}
                        _ => return Err(invalid("deleted edge outside actual merge operands")),
                    }
                }
            }
            attempt.status
        })
    }
    pub(super) fn merge_corner_curve(
        &mut self,
        pseudo: &mut FilletBrep,
        target: &mut Brep,
        face: FaceId,
        curve: Option<&LiveCurveBinding>,
        managed: EdgeId,
    ) -> Result<()> {
        let binding = curve.ok_or_else(|| {
            SolidError::null_pointer("SmTopologyIntersector::MergeCurveOnSurfaces")
        })?;
        let actual = match binding.object {
            FilletObject::IntermediateCurve(e) => self.intermediate.edge(e)?.curve.as_ref(),
            FilletObject::Curve(FilletBody::Pseudo, e) => pseudo.brep().edge(e)?.curve.as_ref(),
            _ => return Err(invalid("corner curve not in actual retained bodies")),
        }
        .ok_or_else(|| invalid("captured corner Curve removed"))?;
        if actual.notify_key() != Some(binding.identity.key)
            || actual.type_code() != binding.identity.type_code
        {
            return Err(invalid("captured corner Curve replaced"));
        }
        let tolerance = self.intermediate.edge(managed)?.tolerance;
        self.merge_actual_curve(
            pseudo,
            target,
            face,
            tolerance,
            binding.object,
            None,
            managed,
            &mut Vec::new(),
        )
    }
}
impl IntersectionTopologyContext for LiveGeometryInsertion<'_> {
    type FaceMarker = FaceMarker;
    type SurfaceBinding = InsertionSurface;
    fn kernel(&self) -> &Kernel {
        self.k
    }
    fn begin_intersection(&mut self, _solver: &FilletSolver) -> Result<()> {
        Ok(())
    }
    fn surface_binding(
        &mut self,
        pseudo: &FilletBrep,
        g: FilletGeomId,
    ) -> Result<Option<InsertionSurface>> {
        let Some(storage) = pseudo.geom(g)?.fillet_surface.as_ref() else {
            return Ok(None);
        };
        let (body, face) = match storage {
            FilletSurfaceStorage::ManagedFace { body, face } => (*body, *face),
            // `GetFilletSurface()` returns the geometry's surface whoever owns it.
            FilletSurfaceStorage::Owned(_) => return Ok(Some(InsertionSurface::Owned(g))),
        };
        let serial = self
            .intermediate
            .notify_serial()
            .ok_or_else(|| invalid("intermediate identity missing"))?;
        if body != topology_ref(serial, EntityRef::Brep(BrepId::from_index(0))) {
            return Err(invalid("fillet Surface belongs to foreign body"));
        }
        Ok(self
            .capture_managed_surface(face)?
            .map(InsertionSurface::Face))
    }
    fn attribute_edges(&self) -> Result<Vec<EdgeId>> {
        edge_attribute_users(self.attributes, self.edge_attribute, self.intermediate)
    }
    fn with_face_scope<T>(
        &mut self,
        target: &mut Brep,
        face: FaceId,
        role: IntersectionFaceRole,
        body: impl FnOnce(&mut Self, &mut Brep, &FaceMarker) -> Result<T>,
    ) -> Result<T> {
        if role == IntersectionFaceRole::Side && self.second_base.is_some() {
            // Only a surface/surface fillet has a second base B-rep, and its side edges
            // are `SmFilletGeom::CreateEndCurve`'s cross sections, made without an
            // original face (`UpdateSplitFace` only replaces a face equal to the split
            // one): upstream's side loop `continue`s on `!pFace` before this point.
            // The edge fillets' cliff and corner side edges that carry one are never
            // in such a session, so a face here is a broken session, not a B-rep the
            // native loop would merge into.
            return Err(invalid(
                "side edge with an original face in a two-B-rep (surface/surface) session",
            ));
        }
        self.marker_scope(
            target,
            face,
            match role {
                IntersectionFaceRole::Rail => 777,
                IntersectionFaceRole::Side => 778,
            },
            role == IntersectionFaceRole::Rail,
            body,
        )
    }
    fn with_rail_base<T>(
        &mut self,
        target: &mut Brep,
        solver: FilletSolverId,
        side: usize,
        body: impl FnOnce(&mut Self, &mut Brep) -> Result<T>,
    ) -> Result<T> {
        // Only a surface/surface solver's second rail lies on the second base B-rep.
        let second_rail = side == 1
            && matches!(
                self.solvers.get(solver.0 as usize),
                Some(Some(SolverObject::SurfaceSurface(_)))
            );
        if !second_rail {
            return body(self, target);
        }
        let Some((second, serial, intersector)) = self.second_base.take() else {
            return body(self, target);
        };
        // The second base B-rep takes the target's place for this rail: its own
        // identity domain and intersector, its topology recorded as the second base's.
        std::mem::swap(self.ti, intersector);
        let target_serial = std::mem::replace(&mut self.target_serial, serial);
        self.base_label = FilletBody::SecondBase;
        let result = body(self, second);
        self.base_label = FilletBody::Target;
        self.target_serial = target_serial;
        std::mem::swap(self.ti, intersector);
        self.second_base = Some((second, serial, intersector));
        result
    }
    fn managed_edge_tolerance(&self, edge: EdgeId) -> Result<f64> {
        Ok(self.intermediate.edge(edge)?.tolerance)
    }
    fn merge_curve(
        &mut self,
        pseudo: &mut FilletBrep,
        target: &mut Brep,
        face: FaceId,
        tol: f64,
        edge: EdgeId,
        managed: EdgeId,
        deleted: &mut Vec<EdgeId>,
    ) -> Result<()> {
        let primary = edgeuse::p1::primary_edgeuse(pseudo.brep(), edge)?
            .ok_or_else(|| SolidError::null_pointer("SmFilletEdge::GetOriginalUVCurve"))?;
        let mate = edgeuse::p1::mate(pseudo.brep(), primary)?
            .ok_or_else(|| SolidError::null_pointer("SmFilletEdge::GetOriginalUVCurve"))?;
        let uv = pseudo
            .original_uv_curve(edge)?
            .map(|_| FilletObject::UvCurve(FilletBody::Pseudo, mate));
        self.merge_actual_curve(
            pseudo,
            target,
            face,
            tol,
            FilletObject::Curve(FilletBody::Pseudo, edge),
            uv,
            managed,
            deleted,
        )
    }
    fn update_topology(
        &mut self,
        pseudo: &mut FilletBrep,
        target: &mut Brep,
        marker: &FaceMarker,
        face: FaceId,
    ) -> Result<()> {
        self.update_changes(pseudo, target, marker, face)
    }
    fn register_intersection(
        &mut self,
        _pseudo: &FilletBrep,
        target: &mut Brep,
        face: FaceId,
        surface: &Option<InsertionSurface>,
    ) -> Result<()> {
        match surface {
            Some(InsertionSurface::Face(binding)) => {
                self.register_bound_intersection(target, face, Some(binding))
            }
            // `RegisterExistingSSI(pBaseFace->GetSurface(), pFilletSurface, ..)` with a
            // surface no face owns: stored, never matched by a face loop's query.
            Some(owned @ InsertionSurface::Owned(_)) => {
                self.register_bound_handle(target, face, Some(owned.handle()))
            }
            None => self.register_bound_intersection(target, face, None),
        }
    }
    fn record_bad_merge(
        &mut self,
        pseudo: &FilletBrep,
        g: FilletGeomId,
        filleted: Option<EdgeId>,
        _target: &Brep,
        face: FaceId,
        rail: EdgeId,
        surface: &Option<InsertionSurface>,
    ) -> Result<()> {
        if let Some(InsertionSurface::Face(binding)) = surface {
            binding.resolve_in_body(self.intermediate)?;
        }
        let base = self.base_label;
        self.error.note_merge_error(FilletErrorType::BadMerge,pseudo.geom(g)?.status(),filleted.map(|e|FilletObject::Topology(base,EntityRef::Edge(e))),
            Some(FilletObject::Topology(base,EntityRef::Face(face))),Some(FilletObject::Topology(FilletBody::Pseudo,EntityRef::Edge(rail))),surface.map(InsertionSurface::handle),
            Some("SmFilletGeom::InsertIntersectionTopology(): Failure to merge fillet rail curve into side face of Target Brep"));
        Ok(())
    }
}

struct LiveInsertionAccess<'a, 'target> {
    objects: &'a mut SessionObjects<'target>,
    context: &'a mut Context,
}
impl LiveInsertionAccess<'_, '_> {
    fn with_context<T>(
        &mut self,
        k: &mut Kernel,
        executive: &mut FilletExecutive<FilletObject>,
        body: impl FnOnce(&mut LiveGeometryInsertion<'_>, &mut FilletBrep, &mut Brep) -> Result<T>,
    ) -> Result<T> {
        let Some(FilletObject::Attribute(edge_attribute)) = executive.edge_attribute else {
            return Err(invalid("actual edge attribute missing"));
        };
        self.objects
            .attributes
            .get(edge_attribute)
            .ok_or_else(|| invalid("actual edge attribute not live"))?;
        let intermediate = self
            .objects
            .intermediate
            .as_mut()
            .ok_or_else(|| invalid("actual intermediate body missing"))?;
        let mut context = LiveGeometryInsertion {
            k,
            context: self.context,
            ti: &mut executive.intersector,
            intermediate,
            curves: &mut self.objects.insertion_curves,
            attributes: &mut self.objects.attributes,
            records: &mut self.objects.notification_records,
            solvers: &self.objects.solvers,
            corners: &self.objects.corners,
            solver_order: &executive.solvers,
            corner_order: &executive.corners,
            error: &mut executive.error,
            edge_attribute,
            target_serial: self.objects.target_serial,
            second_base: match self.objects.second_base.as_mut() {
                Some(SecondBase {
                    body,
                    serial,
                    intersector: Some(intersector),
                }) => Some((&mut **body, *serial, intersector)),
                _ => None,
            },
            base_label: FilletBody::Target,
        };
        body(&mut context, &mut self.objects.pseudo, self.objects.target)
    }
}
impl TopologyInsertionAccess<FilletObject> for LiveInsertionAccess<'_, '_> {
    fn insertion_solver_geoms(&self, solver: FilletObject) -> Result<Vec<FilletObject>> {
        let FilletObject::Solver(id) = solver else {
            return Err(invalid("insertion solver is not retained"));
        };
        let solver = self
            .objects
            .solvers
            .get(id.0 as usize)
            .and_then(Option::as_ref)
            .ok_or_else(|| invalid("actual insertion solver absent"))?;
        Ok(solver
            .base()
            .fillet_geoms
            .iter()
            .copied()
            .map(FilletObject::Geometry)
            .collect())
    }
    fn insertion_geom_topology(
        &mut self,
        k: &mut Kernel,
        geom: FilletObject,
        executive: &mut FilletExecutive<FilletObject>,
    ) -> Result<()> {
        let FilletObject::Geometry(g) = geom else {
            return Err(invalid("insertion geometry is not retained"));
        };
        self.with_context(k, executive, |context, pseudo, target| {
            let id = pseudo
                .geom(g)?
                .fillet_solver()
                .ok_or_else(|| invalid("actual geometry solver missing"))?;
            let solvers = context.solvers;
            let solver = solvers
                .get(id.0 as usize)
                .and_then(Option::as_ref)
                .ok_or_else(|| invalid("actual geometry solver absent"))?;
            pseudo.insert_intersection_topology_with_context(g, solver.base(), target, context)
        })
    }
    fn insertion_corner_topology(
        &mut self,
        k: &mut Kernel,
        corner: FilletObject,
        executive: &mut FilletExecutive<FilletObject>,
    ) -> Result<()> {
        let FilletObject::Corner(id) = corner else {
            return Err(invalid("insertion corner is not retained"));
        };
        self.with_context(k, executive, |context, pseudo, target| {
            context.insert_corner(pseudo, target, id)
        })
    }
}
impl FilletSession<'_> {
    /// Run component stages under the real native filleting flags. The Boolean
    /// flag is restored on every result; native retains the TI filleting flag.
    /// # Errors
    /// The component's original error, with its mutations retained.
    pub fn with_filleting_context<T>(
        &mut self,
        k: &mut Kernel,
        body: impl FnOnce(&mut Self, &mut Kernel) -> Result<T>,
    ) -> Result<T> {
        let prior = k.context.doing_boolean();
        k.context.set_doing_boolean(true);
        self.executive.intersector.doing_fillets = true;
        let result = body(self, k);
        k.context.set_doing_boolean(prior);
        result
    }
    /// One retained corner's `InsertIntersectionTopology` on the executive's
    /// intersector and the actual bodies, without the fillet B-rep edge map
    /// (`DoPiecewiseMerge` inserts each corner this way before relating its surfaces).
    /// # Errors
    /// Original corner insertion errors or a genuine missing live capability.
    pub(in crate::blend::fillet_executive::session) fn insert_corner_topology(
        &mut self,
        k: &mut Kernel,
        corner: FilletObject,
    ) -> Result<()> {
        Context::with_notification_context(k, |k, context| {
            LiveInsertionAccess {
                objects: &mut self.objects,
                context,
            }
            .insertion_corner_topology(k, corner, &mut self.executive)
        })?
    }
    /// One retained geometry's `InsertIntersectionTopology` on the executive's
    /// intersector and the actual bodies, without the fillet B-rep edge map
    /// (`DoSurfaceFilleting` inserts each geometry with a fillet surface this way).
    /// # Errors
    /// Original geometry insertion errors or a genuine missing live capability.
    pub(in crate::blend::fillet_executive::session) fn insert_geometry_topology(
        &mut self,
        k: &mut Kernel,
        geom: FilletObject,
    ) -> Result<()> {
        Context::with_notification_context(k, |k, context| {
            LiveInsertionAccess {
                objects: &mut self.objects,
                context,
            }
            .insertion_geom_topology(k, geom, &mut self.executive)
        })?
    }
    /// Insert all retained geometry and corner topology through the shared native
    /// order. Target editing is restored on every result.
    /// # Errors
    /// Original mapping/insertion errors or a genuine missing live capability.
    pub fn insert_fillet_intersection_topology(&mut self, k: &mut Kernel) -> Result<()> {
        let editing = self.objects.target.editing_enabled();
        self.objects.target.set_editing_enabled(true);
        let result = (|| {
            self.executive
                .intersector
                .set_primary_brep(Some(FilletObject::Body(FilletBody::Target)));
            self.executive
                .intersector
                .set_other_brep(Some(FilletObject::IntermediateBrep));
            self.set_fillet_brep_edge_map(k)?;
            Context::with_notification_context(k, |k, context| {
                self.executive.insert_intersection_topology(
                    k,
                    &mut LiveInsertionAccess {
                        objects: &mut self.objects,
                        context,
                    },
                )
            })?
        })();
        self.objects.target.set_editing_enabled(editing);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::Point3;
    #[test]
    fn filleting_context_restores_boolean_on_component_error() {
        let mut k = Kernel::default();
        let mut target = crate::primitive::api_primitives::create_box(
            &mut k,
            Point3::new(0.0, 0.0, 0.0),
            10.0,
            20.0,
            30.0,
        )
        .unwrap();
        let mut session = FilletSession::new(&mut k, &mut target).unwrap();
        let result: Result<()> = session.with_filleting_context(&mut k, |session, k| {
            assert!(k.context.doing_boolean());
            assert!(session.executive.intersector.doing_fillets);
            Err(SolidError::sm_err("actual component control"))
        });
        assert!(result.is_err());
        assert!(!k.context.doing_boolean());
        assert!(session.executive.intersector.doing_fillets);
    }
    #[test]
    fn actual_edit_loan_preserves_boolean_metadata_callbacks_and_failure_prefix() {
        use crate::geom::context::{with_mark_lock, MarkType, NotifyCallback, ALL_MARKS};
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
        let mut target = crate::primitive::api_primitives::create_box(
            &mut k,
            Point3::new(0.0, 0.0, 0.0),
            10.0,
            20.0,
            30.0,
        )
        .unwrap();
        let edge = target.edges().ids().next().unwrap();
        let vertex = target.vertices().ids().next().unwrap();
        let mut session = FilletSession::new(&mut k, &mut target).unwrap();
        session
            .prepare_constant_radius_edges(&mut k, &[edge], 0.5, 1, 1, 1.0)
            .unwrap();
        session.create_corners(&k, None).unwrap();
        session.create_fillet_brep(&mut k, None).unwrap();
        k.context.set_this_model_size_estimate(1234.5);
        k.context.set_this_large_small_size_ratio(37.0);
        with_mark_lock(&mut k, ALL_MARKS, |_k, _mark| {}).unwrap();
        let mark = k.context.current_mark(MarkType::Mark);
        let zone = k.context.this_zone_tol_3d();
        let events = Arc::new(Mutex::new(Vec::new()));
        k.context
            .set_user_notify_callback(Some(Box::new(Capture(events.clone()))));
        let caller = session.target_identity(EntityRef::Vertex(vertex));
        let changed = Point3::new(17.0, 18.0, 19.0);
        let result: Result<()> = session.with_filleting_context(&mut k, |session, k| {
            session.with_insertion_geometry(k, |_k, _ti, geometry| {
                geometry.with_edit(InsertionBody::Target, |body, env| {
                    let context = env.context.as_deref().unwrap();
                    assert!(context.doing_boolean());
                    assert_eq!(context.stored_model_size_estimate(), 1234.5);
                    assert_eq!(context.stored_large_small_size_ratio(), 37.0);
                    assert_eq!(context.this_zone_tol_3d(), zone);
                    assert_eq!(context.current_mark(MarkType::Mark), mark);
                    let key_of = env.key_of;
                    crate::topology::vertex::set_point(
                        body,
                        vertex,
                        changed,
                        env.records,
                        env.attributes,
                        env.context.as_deref_mut(),
                        caller,
                        &|vu| key_of(EntityRef::Vertexuse(vu)),
                    )?;
                    Err(SolidError::sm_err("ordinary failure after actual edit"))
                })
            })
        });
        assert!(result.is_err());
        assert_eq!(session.target().vertex(vertex).unwrap().point, changed);
        assert!(!k.context.doing_boolean());
        assert_eq!(k.context.stored_model_size_estimate(), 1234.5);
        assert_eq!(k.context.stored_large_small_size_ratio(), 37.0);
        assert_eq!(k.context.current_mark(MarkType::Mark), mark);
        assert!(events
            .lock()
            .unwrap()
            .iter()
            .any(|&(op, object)| op == NotifyOperation::ChangeGeometry && object == caller));
        assert!(k.context.user_notify_callback().is_some());
        let mut later = crate::curve::curve::Curve::from(crate::curve::line::Line::from_points(
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
            3,
        ));
        assert!(k.context.geometry_key(later.notify_identity_slot()).is_ok());
    }

    #[test]
    fn actual_box_insertion_runs_the_shared_geometry_and_corner_stage() {
        let mut k = Kernel::default();
        let mut target = crate::primitive::api_primitives::create_box(
            &mut k,
            Point3::new(0.0, 0.0, 0.0),
            10.0,
            20.0,
            30.0,
        )
        .unwrap();
        let edge = target.edges().ids().next().unwrap();
        let mut session = FilletSession::new(&mut k, &mut target).unwrap();
        session
            .prepare_constant_radius_edges(&mut k, &[edge], 0.5, 1, 1, 1.0)
            .unwrap();
        session.create_corners(&k, None).unwrap();
        let result = session.with_filleting_context(&mut k, |session, k| {
            session.create_fillet_brep(k, None)?;
            session.shrink_fillet_faces(k)?;
            let editing = session.target().editing_enabled();
            let result = session.insert_fillet_intersection_topology(k);
            assert_eq!(session.target().editing_enabled(), editing);
            result
        });
        assert!(result.is_ok(), "{result:?}");
        assert!(!k.context.doing_boolean());
        assert!(session.executive.intersector.doing_fillets);
        assert_eq!(
            crate::topology::brep::p1::vertices(session.target(), None)
                .unwrap()
                .len(),
            12
        );
        assert_eq!(
            crate::topology::brep::p1::edges(session.target(), None)
                .unwrap()
                .len(),
            20
        );
        assert_eq!(
            crate::topology::brep::p1::faces(session.target(), None)
                .unwrap()
                .len(),
            10
        );
        let owned_edges = crate::topology::brep::p1::edges(session.target(), None).unwrap();
        for edge in owned_edges.iter().skip(18) {
            assert!(
                session
                    .target()
                    .edge(*edge)
                    .unwrap()
                    .edgeuses
                    .iter()
                    .all(|&eu| session
                        .target()
                        .edgeuse(eu)
                        .unwrap()
                        .uv_trim_curve
                        .is_none()),
                "native endrib edge has no owned UV trims: {edge:?}"
            );
        }
    }
    #[test]
    fn geometry_owned_fillet_surface_is_registered_under_its_own_handle() {
        // `GetFilletSurface()` names the geometry's surface whoever owns it; one no
        // face took over is registered with each base face and never matched.
        let mut k = Kernel::default();
        let mut target = crate::primitive::api_primitives::create_box(
            &mut k,
            Point3::new(0.0, 0.0, 0.0),
            10.0,
            20.0,
            30.0,
        )
        .unwrap();
        let edge = target.edges().ids().next().unwrap();
        let mut session = FilletSession::new(&mut k, &mut target).unwrap();
        session
            .prepare_constant_radius_edges(&mut k, &[edge], 0.5, 1, 1, 1.0)
            .unwrap();
        session.create_corners(&k, None).unwrap();
        let result = session.with_filleting_context(&mut k, |session, k| {
            session.create_fillet_brep(k, None)?;
            session.shrink_fillet_faces(k)?;
            let g = FilletGeomId(0);
            let Some(FilletSurfaceStorage::ManagedFace { face, .. }) =
                session.objects.pseudo.geom(g)?.fillet_surface
            else {
                panic!("the fillet face took over the surface");
            };
            let intermediate = session.objects.intermediate.as_ref().unwrap();
            let copy = intermediate.face(face)?.surface.clone().unwrap();
            session.objects.pseudo.geom_mut(g)?.fillet_surface =
                Some(FilletSurfaceStorage::Owned(Box::new(copy)));
            session.insert_fillet_intersection_topology(k)?;
            Ok(g)
        });
        let g = result.unwrap();
        let owned = FilletObject::SolverError(SolverErrorObject::FilletSurface(g));
        let table = session.executive.intersector.intersection_table().unwrap();
        assert!(table.other_surfaces.contains(&Some(owned)));
    }
    #[test]
    fn shrink_resolves_the_fillet_surface_owner_face_in_the_fillet_brep() {
        use crate::blend::fillet_executive::p3::FaceShrinkAccess;
        let mut k = Kernel::default();
        let mut target = crate::primitive::api_primitives::create_box(
            &mut k,
            Point3::new(0.0, 0.0, 0.0),
            10.0,
            20.0,
            30.0,
        )
        .unwrap();
        let edge = target.edges().ids().next().unwrap();
        let mut session = FilletSession::new(&mut k, &mut target).unwrap();
        session
            .prepare_constant_radius_edges(&mut k, &[edge], 0.5, 1, 1, 1.0)
            .unwrap();
        session.create_corners(&k, None).unwrap();
        session
            .with_filleting_context(&mut k, |session, k| session.create_fillet_brep(k, None))
            .unwrap();
        let g = FilletGeomId(0);
        let Some(FilletSurfaceStorage::ManagedFace { face, .. }) =
            session.objects.pseudo.geom(g).unwrap().fillet_surface
        else {
            panic!("the fillet face took over the surface");
        };
        let geom = FilletObject::Geometry(g);
        let own = Some(FilletObject::IntermediateTopology(EntityRef::Face(face)));
        assert_eq!(session.objects.shrink_geometry_face(geom).unwrap(), own);
        // A surface the geometry holds whose owner is a fillet B-rep face (not the
        // geometry's managed one) resolves to that face, as `GetFace()` does.
        let intermediate = session.objects.intermediate.as_ref().unwrap();
        let mut copy = intermediate.face(face).unwrap().surface.clone().unwrap();
        let owner = session
            .objects
            .intermediate_identity(EntityRef::Face(face))
            .unwrap();
        copy.set_owner(Some(owner));
        session.objects.pseudo.geom_mut(g).unwrap().fillet_surface =
            Some(FilletSurfaceStorage::Owned(Box::new(copy)));
        assert_eq!(session.objects.shrink_geometry_face(geom).unwrap(), own);
    }
}
