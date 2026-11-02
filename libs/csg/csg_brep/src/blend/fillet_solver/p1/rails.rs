//! Typed pseudo-B-rep rail split and support-curve operations.
//!
//! Upstream: `source/SMLib/src/SmFilletSolver.cpp` (5999–6064, 6342–6504).
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`split_rail`] | `sm_SplitRail` |
//! | [`compute_support_edge`] | `sm_ComputeSupportEdge` |
//!
//! Deviations: typed arena ids and caller-owned notification routing. Edge
//! construction delegates to the ported fillet geometry builder; surface
//! intersection calls the ported `blend::fillet_geom::p2::srf_srf_intersection`.
//! No recursion.
use crate::blend::fillet_geom::p1::{
    CliffHandle, FilStatus, FilletBrep, FilletEdgeType, FilletGeomId, FilletVertex,
};
use crate::curve::curve::{p2::OwnerChange, Curve};
use crate::error::{ignored, EntityRef, Result, SolidError};
use crate::geom::{global_solver::SolverOperation, object::ObjectRef};
use crate::kernel::Kernel;
use crate::math::{Point3, Vec3};
use crate::surface::surface::Surface;
use crate::topology::{edge, edgeuse::p1 as eu, Brep, EdgeId, Vertex, VertexId};
fn invalid(reason: &'static str) -> SolidError {
    SolidError::invalid_input(OP, reason)
}
const OP: &str = "blend::fillet_solver::p1::rails";

/// Caller-owned object identities and notification routing for new rail curves.
pub trait RailOwnership {
    /// Resolve the actual owner identity and its optional B-rep identity.
    fn edge_owner(&self, edge: EdgeId) -> Result<(ObjectRef, Option<ObjectRef>)>;
    /// Deliver the notification produced by `Curve::set_owner` for `curve`, the
    /// notifying object (`SM_NO_CHANGE_OWNER` on the curve).
    ///
    /// # Errors
    /// Identity resolution or notification transport failures.
    fn owner_changed(&mut self, curve: &mut Curve, change: OwnerChange) -> Result<()>;
    /// Set the owner of the actual installed rail curve. The default preserves the
    /// original adapter contract; retained contexts may notify before assignment.
    ///
    /// # Errors
    /// Identity resolution or notification transport failures.
    fn set_curve_owner(
        &mut self,
        _edge: EdgeId,
        curve: &mut Curve,
        owner: ObjectRef,
        brep: Option<ObjectRef>,
    ) -> Result<()> {
        let (_, notification) = curve.set_owner(Some(owner), brep);
        if let Some(notification) = notification {
            self.owner_changed(curve, notification)?;
        }
        Ok(())
    }
}
/// Retained notification context for the actual installed recovery curve.
pub struct RetainedRailOwnership<'a> {
    serial: u64,
    attributes: &'a mut crate::geom::attribute::AttributeStore,
    context: &'a mut crate::geom::context::Context,
}
impl<'a> RetainedRailOwnership<'a> {
    /// Lend the actual notification context while operating on its retained pseudo
    /// body. A fresh body is issued its actual identity; foreign domains are refused.
    /// The same stores and metadata remain visible to the action and are restored
    /// on returned errors. Completed geometry/topology writes are never rolled back.
    /// # Errors
    /// Identity/domain or notification-lease refusal before the action; otherwise
    /// the action's error with its completed mutation prefix retained.
    pub fn with_context<T>(
        k: &mut Kernel,
        pseudo: &mut FilletBrep,
        attributes: &mut crate::geom::attribute::AttributeStore,
        action: impl FnOnce(&mut Kernel, &mut FilletBrep, &mut RetainedRailOwnership<'_>) -> Result<T>,
    ) -> Result<T> {
        if let Some(serial) = pseudo.brep().notify_serial() {
            if k.context
                .issued_brep_serial(pseudo.brep().notify_identity())
                != Some(serial)
            {
                return Err(invalid("rollover pseudo B-rep belongs to another context"));
            }
        } else {
            k.context
                .brep_serial(pseudo.brep_mut().notify_identity_slot())?;
        }
        crate::geom::context::Context::with_notification_context(k, |k, context| {
            let mut ownership = RetainedRailOwnership::new(pseudo.brep(), attributes, context)?;
            action(k, pseudo, &mut ownership)
        })?
    }

    /// Borrow the actual pseudo body domain and retained notification stores.
    /// # Errors
    /// Missing identity or a body issued by a different notification Context.
    pub fn new(
        pseudo: &Brep,
        attributes: &'a mut crate::geom::attribute::AttributeStore,
        context: &'a mut crate::geom::context::Context,
    ) -> Result<Self> {
        let serial = pseudo
            .notify_serial()
            .ok_or_else(|| invalid("pseudo B-rep has no kernel identity"))?;
        if context.issued_brep_serial(pseudo.notify_identity()) != Some(serial) {
            return Err(invalid("rollover pseudo B-rep belongs to another context"));
        }
        Ok(Self {
            serial,
            attributes,
            context,
        })
    }
}
impl RailOwnership for RetainedRailOwnership<'_> {
    fn edge_owner(&self, edge: EdgeId) -> Result<(ObjectRef, Option<ObjectRef>)> {
        Ok((
            crate::topology::brep::p3::topology_ref(self.serial, EntityRef::Edge(edge)),
            // SM_NO_GET_BREP is NULL in the pinned build.
            None,
        ))
    }
    /// `SM_NO_CHANGE_OWNER` on the actual curve, through the retained context.
    fn owner_changed(&mut self, curve: &mut Curve, change: OwnerChange) -> Result<()> {
        let type_code = curve.type_code();
        let key = self.context.geometry_key(curve.notify_identity_slot())?;
        let actual = ObjectRef { type_code, key };
        crate::geom::object::report_owner_link(Some(self.context), actual.key, change.old_owner);
        crate::error::ignore_status(crate::geom::a_object::notify(
            self.attributes,
            Some(self.context),
            crate::geom::object::NotifyOperation::ChangeOwner,
            actual,
            change.new_owner,
            change.new_owner_brep,
            change.old_owner,
        ))
    }
    fn set_curve_owner(
        &mut self,
        _edge: EdgeId,
        curve: &mut Curve,
        owner: ObjectRef,
        _: Option<ObjectRef>,
    ) -> Result<()> {
        let old_owner = curve.owner();
        if old_owner != Some(owner) && curve.dim() == 3 {
            // SmCurve::SetOwner notifies while GetOwner still returns the old owner
            // (SM_NO_GET_BREP is NULL in the pinned build).
            self.owner_changed(
                curve,
                OwnerChange {
                    new_owner: Some(owner),
                    new_owner_brep: None,
                    old_owner,
                },
            )?;
        }
        curve.set_owner(Some(owner), None);
        if let Some(key) = curve.notify_key() {
            crate::geom::object::report_owner_link(Some(self.context), key, Some(owner));
        }
        Ok(())
    }
}

pub(crate) struct SupportIntersection<'a> {
    pub start: VertexId,
    pub end: Option<VertexId>,
    pub start_direction: Option<Vec3>,
    pub end_direction: Option<Vec3>,
    pub surfaces: [&'a Surface; 2],
    pub approx_tol: f64,
    pub angle_tol: f64,
    pub skip_two_points_intersection: bool,
    pub reference_point: Option<Point3>,
}

pub(crate) trait RailOps {
    fn make_edge(
        &mut self,
        _k: &Kernel,
        fb: &mut FilletBrep,
        start: VertexId,
        end: VertexId,
    ) -> Result<EdgeId> {
        fb.make_fillet_edge(Some(start), Some(end), None)
    }
    fn surface_intersection(
        &mut self,
        k: &Kernel,
        fb: &mut FilletBrep,
        request: SupportIntersection<'_>,
    ) -> Result<Option<Curve>> {
        let SupportIntersection {
            start,
            end,
            mut start_direction,
            end_direction,
            surfaces,
            approx_tol,
            angle_tol,
            skip_two_points_intersection,
            reference_point,
        } = request;
        let [first, second] = surfaces;
        crate::blend::fillet_geom::p2::srf_srf_intersection(
            k,
            Some(fb.vertex_point(start)?),
            end.map(|v| fb.vertex_point(v)).transpose()?,
            start_direction.as_mut(),
            end_direction,
            first,
            second,
            approx_tol,
            angle_tol,
            crate::blend::fillet_geom::p2::SrfSrfOptions {
                skip_two_pnts_intersection: skip_two_points_intersection,
                ref_point: reference_point,
            },
        )
        .map(|curves| curves.curve_3d)
    }
}
pub(crate) struct Missing;
impl RailOps for Missing {}

/// Split a rail in the pseudo-B-rep, preserving ownership and partial mutations.
///
/// # Errors
/// Missing topology or geometry, curve-copy failures, or unavailable edge construction.
pub fn split_rail(
    k: &Kernel,
    fillets: &mut FilletBrep,
    ownership: &mut dyn RailOwnership,
    geom: FilletGeomId,
    rail: EdgeId,
) -> Result<(VertexId, EdgeId)> {
    split_rail_with_ops(k, fillets, ownership, geom, rail, &mut Missing)
}
pub(crate) fn split_rail_with_ops(
    k: &Kernel,
    fillets: &mut FilletBrep,
    ownership: &mut dyn RailOwnership,
    geom: FilletGeomId,
    rail: EdgeId,
    ops: &mut impl RailOps,
) -> Result<(VertexId, EdgeId)> {
    Rails {
        k,
        fillets,
        ownership,
    }
    .split_rail(geom, rail, ops)
}
/// Compute, trim and install a support curve between its adjacent fillet surfaces.
///
/// # Errors
/// Missing topology/surfaces, failed intersection/drop/trim, or unavailable surface intersection.
pub fn compute_support_edge(
    k: &Kernel,
    fillets: &mut FilletBrep,
    ownership: &mut dyn RailOwnership,
    support: EdgeId,
    geom: FilletGeomId,
    approx: f64,
    angle: f64,
) -> Result<()> {
    compute_support_edge_with_ops(
        k,
        fillets,
        ownership,
        support,
        geom,
        approx,
        angle,
        &mut Missing,
    )
}
pub(crate) fn compute_support_edge_with_ops(
    k: &Kernel,
    fillets: &mut FilletBrep,
    ownership: &mut dyn RailOwnership,
    support: EdgeId,
    geom: FilletGeomId,
    approx: f64,
    angle: f64,
    ops: &mut impl RailOps,
) -> Result<()> {
    Rails {
        k,
        fillets,
        ownership,
    }
    .support_edge(support, geom, approx, angle, ops)
}
fn required<T>(value: Option<T>) -> Result<T> {
    value.ok_or_else(|| SolidError::null_pointer(OP))
}
struct Rails<'a> {
    k: &'a Kernel,
    fillets: &'a mut FilletBrep,
    ownership: &'a mut dyn RailOwnership,
}
impl Rails<'_> {
    fn start(&self, rail: EdgeId) -> Result<VertexId> {
        required(edge::vertex(self.fillets.brep(), rail)?)
    }
    fn end(&self, rail: EdgeId) -> Result<VertexId> {
        let start = self.start(rail)?;
        required(edge::other_vertex(self.fillets.brep(), rail, Some(start))?)
    }
    fn install_curve(&mut self, edge: EdgeId, curve: Curve) -> Result<()> {
        crate::blend::fillet_geom::p1::set_rail_curve(self.fillets.brep_mut(), edge, curve)?;
        let (owner, brep) = self.ownership.edge_owner(edge)?;
        let curve = required(self.fillets.brep_mut().edge_mut(edge)?.curve.as_mut())?;
        self.ownership.set_curve_owner(edge, curve, owner, brep)
    }

    fn split_rail(
        &mut self,
        geom: FilletGeomId,
        rail: EdgeId,
        ops: &mut impl RailOps,
    ) -> Result<(VertexId, EdgeId)> {
        let start = self.start(rail)?;
        let end = self.end(rail)?;
        let mut edgeuse = required(eu::primary_edgeuse(self.fillets.brep(), rail)?)?;
        let mut vu = required(self.fillets.brep().edgeuse(edgeuse)?.vertexuse)?;
        if self.fillets.brep().vertexuse(vu)?.vertex != Some(end) {
            edgeuse = required(eu::mate(self.fillets.brep(), edgeuse)?)?;
            vu = required(self.fillets.brep().edgeuse(edgeuse)?.vertexuse)?;
        }
        if start == end
            && self.fillets.brep().edgeuse(edgeuse)?.orientation
                != crate::nurbs::b_spline_surface_n_lib::p2::OrientType::Opposite
        {
            edgeuse = required(eu::mate(self.fillets.brep(), edgeuse)?)?;
            vu = required(self.fillets.brep().edgeuse(edgeuse)?.vertexuse)?;
        }
        self.fillets
            .set_edgeuse_uv_curve(edgeuse, None, true, true)?;
        self.fillets
            .brep_mut()
            .vertex_mut(end)?
            .vertexuses
            .retain(|v| *v != vu);
        // SmVertex() starts at the origin with tolerance 0, independent of
        // the system zone tolerance used by Brep::add_vertex's convenience API.
        let vertex = self
            .fillets
            .brep_mut()
            .add_vertex_record(Vertex::default())?;
        self.fillets
            .insert_fillet_vertex(vertex, FilletVertex::new(self.k, None));
        self.fillets
            .brep_mut()
            .vertex_mut(vertex)?
            .vertexuses
            .push(vu);
        self.fillets.brep_mut().vertexuse_mut(vu)?.vertex = Some(vertex);
        let (new_start, new_end) = edge::start_end_vertices(self.fillets.brep(), rail)?;
        self.fillets.brep_mut().edge_mut(rail)?.vertices =
            [required(new_start)?, required(new_end)?];
        let new_rail = ops.make_edge(self.k, self.fillets, vertex, end)?;
        let original_face = self.fillets.fillet_edge(rail)?.original_face();
        let record = self.fillets.fillet_edge_mut(new_rail)?;
        record.set_fillet_edge_type(FilletEdgeType::Rail);
        record.set_original_face(original_face);
        let mut new_eu = required(eu::primary_edgeuse(self.fillets.brep(), new_rail)?)?;
        let new_vu = required(self.fillets.brep().edgeuse(new_eu)?.vertexuse)?;
        if self.fillets.brep().vertexuse(new_vu)?.vertex != Some(end) {
            new_eu = required(eu::mate(self.fillets.brep(), new_eu)?)?;
        }
        self.fillets.set_edgeuse_fillet_geom(new_eu, Some(geom))?;
        let curve = required(edge::curve(self.fillets.brep(), rail)?)?.copy(self.k)?;
        self.install_curve(new_rail, curve)?;
        let tolerance = edge::tolerance(self.fillets.brep(), rail)?;
        edge::set_tolerance(self.fillets.brep_mut(), new_rail, tolerance, true, true)?;
        Ok((vertex, new_rail))
    }

    fn support_edge(
        &mut self,
        support: EdgeId,
        geom: FilletGeomId,
        approx: f64,
        angle: f64,
        ops: &mut impl RailOps,
    ) -> Result<()> {
        let start = self.start(support)?;
        if !self.fillets.fillet_vertex(start)?.is_processed() {
            return Err(SolidError::sm_err(OP));
        }
        let start_point = self.fillets.vertex_point(start)?;
        let primary = required(eu::primary_edgeuse(self.fillets.brep(), support)?)?;
        let mate = required(eu::mate(self.fillets.brep(), primary)?)?;
        let first = required(self.fillets.fillet_edgeuse(primary)?.fillet_geom())?;
        let second = required(self.fillets.fillet_edgeuse(mate)?.fillet_geom())?;
        let surf1 = required(self.fillets.geom(first)?.fillet_surface(None)?)?.clone();
        let surf2 = required(self.fillets.geom(second)?.fillet_surface(None)?)?.clone();
        let outcome = ops.surface_intersection(
            self.k,
            self.fillets,
            SupportIntersection {
                start,
                end: None,
                start_direction: None,
                end_direction: None,
                surfaces: [&surf1, &surf2],
                approx_tol: approx,
                angle_tol: angle,
                skip_two_points_intersection: false,
                reference_point: None,
            },
        );
        let mut curve = match outcome {
            Err(e) if e.is_unported() => return Err(e),
            Ok(Some(curve)) => curve,
            _ => {
                self.fillets
                    .fillet_edge_mut(support)?
                    .set_status(FilStatus::SurfIntFailure);
                return Err(SolidError::sm_err(OP));
            }
        };
        let mut interval = curve.natural_interval();
        let surface = required(self.fillets.geom(geom)?.fillet_surface(None)?)?;
        let solutions = surface.global_curve_intersect(
            self.k,
            [CliffHandle::Curve(&curve), CliffHandle::Surface(surface)],
            &surface.natural_uv_domain_with_nurb(self.k)?,
            &curve,
            &interval,
            approx,
        )?;
        let sol = solutions.get(0).ok_or_else(|| SolidError::sm_err(OP))?;
        let t = sol
            .start
            .parameters
            .first()
            .copied()
            .ok_or_else(|| SolidError::null_pointer(OP))?;
        let end_point = curve.evaluate_point(self.k, t)?;
        let end = self.end(support)?;
        self.fillets.brep_mut().vertex_mut(end)?.point = end_point;
        self.fillets
            .fillet_vertex_mut(end)?
            .set_status(FilStatus::Processed);
        let drop1 = curve.drop_point(
            self.k,
            &interval,
            start_point,
            None,
            approx,
            None,
            SolverOperation::Minimize,
        )?;
        if !drop1.success {
            return Err(SolidError::sm_err(OP));
        }
        let drop2 = curve.drop_point(
            self.k,
            &interval,
            end_point,
            None,
            approx,
            None,
            SolverOperation::Minimize,
        )?;
        if !drop2.success {
            return Err(SolidError::sm_err(OP));
        }
        let reverse = drop1.parameter > drop2.parameter;
        if reverse {
            ignored(interval.set_min_max(drop2.parameter, drop1.parameter))?;
        } else {
            ignored(interval.set_min_max(drop1.parameter, drop2.parameter))?;
        }
        curve.trim(
            self.k,
            &mut interval,
            crate::nurbs::b_spline_curve_n_lib::TrimMode::Notify,
        )?;
        if reverse {
            curve.reverse_parameterization(self.k, &interval)?;
        }
        self.install_curve(support, curve)
    }
}

#[cfg(test)]
mod rollover_notification_tests {
    use super::*;
    use crate::blend::fillet_geom::p1::{FilletBrep, FilletGeom, FilletGeomType, FilletVertex};
    use crate::blend::fillet_solver::p1::split_rail;
    use crate::geom::attribute::{AttributeStore, ObjectKey};
    use crate::geom::context::{Context as NotifyContext, NotifyCallback};
    use crate::geom::object::NotifyOperation;
    use crate::kernel::Kernel;
    use crate::math::Vec3;
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    #[derive(Debug, Clone, PartialEq)]
    struct Event {
        system: bool,
        caller: ObjectRef,
        data: [Option<ObjectRef>; 3],
        observed_owner: Option<ObjectRef>,
    }
    #[derive(Debug)]
    struct Callback {
        system: bool,
        events: Arc<Mutex<Vec<Event>>>,
        owners: BTreeMap<ObjectKey, Option<ObjectRef>>,
    }
    impl NotifyCallback for Callback {
        fn execute(
            &mut self,
            operation: NotifyOperation,
            caller: ObjectRef,
            data1: Option<ObjectRef>,
            data2: Option<ObjectRef>,
            data3: Option<ObjectRef>,
        ) -> Result<()> {
            assert_eq!(operation, NotifyOperation::ChangeOwner);
            self.events.lock().unwrap().push(Event {
                system: self.system,
                caller,
                data: [data1, data2, data3],
                observed_owner: *self.owners.get(&caller.key).unwrap(),
            });
            // Native ignores callback failures, including dependency statuses.
            Err(SolidError::unported("test callback", "test"))
        }
        fn owner_link(&mut self, object: ObjectKey, owner: Option<ObjectRef>) {
            self.owners.insert(object, owner);
        }
    }

    fn rail(k: &mut Kernel) -> (FilletBrep, FilletGeomId, EdgeId, ObjectRef) {
        let mut fb = FilletBrep::new();
        k.context
            .brep_serial(fb.brep_mut().notify_identity_slot())
            .unwrap();
        let vertices = [Vec3::ZERO, Vec3::X].map(|point| {
            let v = fb.brep_mut().add_vertex(point).unwrap();
            fb.insert_fillet_vertex(v, FilletVertex::new(k, None));
            v
        });
        let edge = fb
            .make_fillet_edge(Some(vertices[0]), Some(vertices[1]), None)
            .unwrap();
        let serial = fb.brep().notify_serial().unwrap();
        let owner = crate::topology::brep::p3::topology_ref(serial, EntityRef::Edge(edge));
        let mut curve = Curve::from(crate::curve::line::Line::from_points(
            Vec3::ZERO,
            Vec3::X,
            3,
        ));
        curve.set_owner(Some(owner), None);
        let original = ObjectRef {
            type_code: curve.type_code(),
            key: k
                .context
                .geometry_key(curve.notify_identity_slot())
                .unwrap(),
        };
        fb.brep_mut().edge_mut(edge).unwrap().curve = Some(curve);
        fb.brep_mut().edge_mut(edge).unwrap().tolerance = 0.125;
        let geom = fb.add_geom(FilletGeom::new(k, None, FilletGeomType::Default).unwrap());
        (fb, geom, edge, original)
    }

    #[test]
    fn retained_scope_registers_fresh_body_and_preserves_error_prefix_without_foreign_rebinding() {
        let mut k = Kernel::default();
        k.context.set_doing_boolean(true);
        let mut pseudo = FilletBrep::new();
        assert_eq!(pseudo.brep().notify_serial(), None);
        let mut attributes = AttributeStore::default();
        let result: Result<()> = RetainedRailOwnership::with_context(
            &mut k,
            &mut pseudo,
            &mut attributes,
            |k, pseudo, ownership| {
                assert!(k.context.doing_boolean());
                assert!(ownership.context.doing_boolean());
                pseudo.brep_mut().add_vertex(Vec3::X)?;
                Err(SolidError::invalid_input(
                    "scope control",
                    "after actual write",
                ))
            },
        );
        assert!(result.is_err());
        assert_eq!(pseudo.brep().vertices().len(), 1);
        let serial = pseudo.brep().notify_serial().unwrap();
        assert_eq!(
            k.context
                .issued_brep_serial(pseudo.brep().notify_identity()),
            Some(serial)
        );
        assert!(k.context.doing_boolean());
        let mut foreign = Kernel::default();
        let mut foreign_body = FilletBrep::new();
        RetainedRailOwnership::with_context(
            &mut foreign,
            &mut foreign_body,
            &mut attributes,
            |_, _, _| Ok(()),
        )
        .unwrap();
        assert_eq!(foreign_body.brep().notify_serial(), Some(serial));
        let mut entered = false;
        assert!(RetainedRailOwnership::with_context(
            &mut foreign,
            &mut pseudo,
            &mut attributes,
            |_, _, _| {
                entered = true;
                Ok(())
            }
        )
        .is_err());
        assert!(!entered);
        assert_eq!(pseudo.brep().notify_serial(), Some(serial));
        assert_eq!(pseudo.brep().vertices().len(), 1);
        assert_eq!(
            k.context
                .issued_brep_serial(pseudo.brep().notify_identity()),
            Some(serial)
        );
    }

    #[test]
    fn rollover_split_notifies_actual_copy_before_owner_and_keeps_callback_failures_ignored() {
        let mut k = Kernel::default();
        let (mut fb, geom, old_edge, original) = rail(&mut k);
        let events = Arc::new(Mutex::new(Vec::new()));
        for system in [false, true] {
            let callback = Some(Box::new(Callback {
                system,
                events: events.clone(),
                owners: BTreeMap::new(),
            }) as Box<dyn NotifyCallback>);
            if system {
                k.context.set_sys_notify_callback(callback);
            } else {
                k.context.set_user_notify_callback(callback);
            }
        }
        let mut attributes = AttributeStore::default();
        let (_, new_edge) = NotifyContext::with_notification_context(&mut k, |k, context| {
            let mut ownership =
                RetainedRailOwnership::new(fb.brep(), &mut attributes, context).unwrap();
            split_rail(k, &mut fb, &mut ownership, geom, old_edge)
        })
        .unwrap()
        .unwrap();
        let installed = fb.brep().edge(new_edge).unwrap().curve.as_ref().unwrap();
        let actual = ObjectRef {
            type_code: installed.type_code(),
            key: installed.notify_key().unwrap(),
        };
        assert_ne!(actual, original);
        let owner = crate::topology::brep::p3::topology_ref(
            fb.brep().notify_serial().unwrap(),
            EntityRef::Edge(new_edge),
        );
        // Virtual Curve::copy constructs a fresh, unowned wrapper.
        let old_owner = None;
        assert_eq!(
            *events.lock().unwrap(),
            vec![
                Event {
                    system: false,
                    caller: actual,
                    data: [Some(owner), None, old_owner],
                    observed_owner: old_owner
                },
                Event {
                    system: true,
                    caller: actual,
                    data: [Some(owner), None, old_owner],
                    observed_owner: old_owner
                },
            ]
        );
        assert_eq!(installed.owner(), Some(owner));
        assert_eq!(fb.brep().edge(new_edge).unwrap().tolerance, 0.125);
        assert!(k.context.user_notify_callback().is_some());
        assert!(k.context.sys_notify_callback().is_some());
    }

    #[test]
    fn rollover_identity_refusal_retains_installed_copy_and_prior_topology_without_late_tolerance()
    {
        let mut k = Kernel::default();
        let (mut fb, geom, old_edge, _) = rail(&mut k);
        let old_vertices = fb.brep().vertices().len();
        let old_edges = fb.edge_list().len();
        let mut attributes = AttributeStore::default();
        let error = NotifyContext::with_notification_context(&mut k, |k, context| {
            let mut ownership =
                RetainedRailOwnership::new(fb.brep(), &mut attributes, context).unwrap();
            // A real active identity lease refuses issuance at the actual install.
            let lease = ownership.context.lend_notify().unwrap();
            let result = split_rail(k, &mut fb, &mut ownership, geom, old_edge);
            ownership.context.install_lease(lease);
            result
        })
        .unwrap()
        .unwrap_err();
        assert!(matches!(error, SolidError::InvalidInput { .. }));
        assert_eq!(fb.brep().vertices().len(), old_vertices + 1);
        assert_eq!(fb.edge_list().len(), old_edges + 1);
        let new_edge = *fb.edge_list().last().unwrap();
        let installed = fb.brep().edge(new_edge).unwrap();
        assert!(installed.curve.is_some());
        assert_eq!(installed.tolerance, 0.0);
        assert_eq!(installed.curve.as_ref().unwrap().owner(), None);
    }

    #[test]
    fn rollover_refuses_foreign_domain_even_with_equal_numeric_serial() {
        let mut k = Kernel::default();
        let (fb, _, _, _) = rail(&mut k);
        let mut foreign = Kernel::default();
        let mut other = Brep::new();
        foreign
            .context
            .brep_serial(other.notify_identity_slot())
            .unwrap();
        assert_eq!(fb.brep().notify_serial(), other.notify_serial());
        let mut attributes = AttributeStore::default();
        assert!(
            RetainedRailOwnership::new(fb.brep(), &mut attributes, &mut foreign.context).is_err()
        );
        assert!(attributes.attributes(ObjectKey(0)).is_empty());
    }
}
