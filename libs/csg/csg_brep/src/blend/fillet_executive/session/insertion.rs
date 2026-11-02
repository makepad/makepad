//! Actual executive stores lent to intersection insertion. No second intersector,
//! solver table or attribute store is constructed here.

mod corner;
pub mod curves;
pub mod geometry;
mod live;

use super::{
    invalid, Attribute, AttributeHandle, AttributeStore, Behavior, Brep, EdgeId, EntityRef,
    ErrorInfo, FaceId, FilletBody, FilletBrep, FilletCorner, FilletObject, FilletSession, Kernel,
    Result, SolverObject,
};
use crate::blend::fillet_executive::topology_changes::TopologyChangesAccess;
use crate::boolean::topology_intersector::p1::TopologyIntersector;
use crate::geom::attribute::ObjectKey;
use crate::geom::object::ObjectRef;

/// Disjoint borrows of one actual executive and its retained objects.
pub struct InsertionState<'a> {
    /// Actual inherited intersector.
    pub intersector: &'a mut TopologyIntersector<FilletObject>,
    /// Actual operation-owned curve copies retained across merge failures.
    pub curves: &'a mut curves::InsertionCurves,
    /// Actual standalone-reference edge attribute.
    pub edge_attribute: AttributeHandle,
    /// Actual attribute users and values.
    pub attributes: &'a mut AttributeStore,
    /// Actual topology records used by notification callbacks.
    pub notification_records: &'a mut crate::topology::topology::NotifyRecords,
    /// Actual source pseudo topology.
    pub pseudo: &'a mut FilletBrep,
    /// Actual target receiving insertion.
    pub target: &'a mut Brep,
    /// Actual constructed intermediate body.
    pub intermediate: &'a mut Brep,
    /// Actual concrete solver slots.
    pub solvers: &'a [Option<SolverObject>],
    /// Actual retained corner records.
    pub corners: &'a [FilletCorner],
    /// Executive solver traversal order.
    pub solver_order: &'a [FilletObject],
    /// Executive corner traversal order.
    pub corner_order: &'a [FilletObject],
    /// Actual executive error record.
    pub error: &'a mut ErrorInfo<FilletObject>,
    /// Target's kernel-issued identity domain.
    pub target_serial: u64,
}

impl FilletSession<'_> {
    /// Run an insertion component with the retained intersector and actual edit stores.
    /// The real Context lease is restored after success or failure.
    /// # Errors
    /// Missing actual insertion state or the original component error.
    pub fn with_insertion_geometry<T>(
        &mut self,
        k: &mut Kernel,
        operation: impl FnOnce(
            &mut Kernel,
            &mut TopologyIntersector<FilletObject>,
            &mut geometry::InsertionGeometry<'_>,
        ) -> Result<T>,
    ) -> Result<T> {
        let state = self.insertion_state()?;
        crate::geom::context::Context::with_notification_context(k, |k, context| {
            let mut geometry =
                geometry::InsertionGeometry::new(state.target, state.pseudo, state.intermediate)?
                    .with_notifications(
                    state.curves,
                    geometry::InsertionNotifications {
                        records: state.notification_records,
                        attributes: state.attributes,
                        context,
                    },
                )?;
            operation(k, state.intersector, &mut geometry)
        })?
    }

    /// Lend the actual stores after intermediate construction for insertion.
    /// No topology is changed and no intersector is copied or reinitialized.
    /// # Errors
    /// Missing intermediate body or actual edge-attribute lifecycle state.
    pub fn insertion_state(&mut self) -> Result<InsertionState<'_>> {
        let Some(FilletObject::Attribute(edge_attribute)) = self.executive.edge_attribute else {
            return Err(invalid("insertion edge attribute is not retained"));
        };
        self.objects
            .attributes
            .get(edge_attribute)
            .ok_or_else(|| invalid("insertion edge attribute is not live"))?;
        let intermediate = self
            .objects
            .intermediate
            .as_mut()
            .ok_or_else(|| invalid("intermediate body is not allocated"))?;
        Ok(InsertionState {
            intersector: &mut self.executive.intersector,
            curves: &mut self.objects.insertion_curves,
            edge_attribute,
            attributes: &mut self.objects.attributes,
            notification_records: &mut self.objects.notification_records,
            pseudo: &mut self.objects.pseudo,
            target: self.objects.target,
            intermediate,
            solvers: &self.objects.solvers,
            corners: &self.objects.corners,
            solver_order: &self.executive.solvers,
            corner_order: &self.executive.corners,
            error: &mut self.executive.error,
            target_serial: self.objects.target_serial,
        })
    }
}

/// Actual users of E which are live intermediate edges, in attribute-user order.
/// Membership is checked against actual issued keys, never decoded arena indices.
/// # Errors
/// Missing actual attribute/body identity.
pub fn edge_attribute_users(
    attributes: &AttributeStore,
    attribute: AttributeHandle,
    intermediate: &Brep,
) -> Result<Vec<EdgeId>> {
    let serial = intermediate
        .notify_serial()
        .ok_or_else(|| invalid("intermediate body has no kernel identity"))?;
    let mut edges = Vec::new();
    for user in attributes.users(attribute)? {
        if let Some((edge, _)) = intermediate.edges().iter().find(|(edge, _)| {
            crate::topology::brep::p3::topology_ref(serial, EntityRef::Edge(*edge)).key == user
        }) {
            edges.push(edge);
        }
    }
    Ok(edges)
}

/// One actual face marker, owned by `with_face_marker` until its scope ends.
#[derive(Debug)]
pub struct FaceMarker {
    handle: AttributeHandle,
    face: ObjectRef,
    restore_editing: Option<bool>,
}
impl FaceMarker {
    /// The actual attribute, including users propagated by topology operations.
    #[must_use]
    pub const fn attribute(&self) -> AttributeHandle {
        self.handle
    }
    /// The original actual face in its issued body domain.
    #[must_use]
    pub const fn original_face(&self) -> ObjectRef {
        self.face
    }
    fn finish(self, attributes: &mut AttributeStore, target: &mut Brep) -> Result<()> {
        let cleanup = attributes.destroy(self.handle);
        if let Some(editing) = self.restore_editing {
            target.set_editing_enabled(editing);
        }
        cleanup
    }
}

/// Native stack-marker scope. Rail callers use id777 with editing enabled;
/// side callers use id778 without changing editing. Cleanup runs on every result.
/// # Errors
/// Actual identity/attribute errors or the body's original error.
pub fn with_face_marker<T>(
    attributes: &mut AttributeStore,
    target: &mut Brep,
    face: FaceId,
    id: u64,
    enable_editing: bool,
    body: impl FnOnce(&mut AttributeStore, &mut Brep, &FaceMarker) -> Result<T>,
) -> Result<T> {
    let marker = begin_face_marker(attributes, target, face, id, enable_editing)?;
    let result = body(attributes, target, &marker);
    let cleanup = end_face_marker(attributes, target, marker);
    match result {
        Err(error) => Err(error),
        Ok(value) => cleanup.map(|()| value),
    }
}

pub(crate) fn begin_face_marker(
    attributes: &mut AttributeStore,
    target: &mut Brep,
    face: FaceId,
    id: u64,
    enable_editing: bool,
) -> Result<FaceMarker> {
    target.face(face)?;
    let serial = target
        .notify_serial()
        .ok_or_else(|| invalid("target body has no kernel identity"))?;
    let actual = crate::topology::brep::p3::topology_ref(serial, EntityRef::Face(face));
    let restore_editing = enable_editing.then(|| target.editing_enabled());
    if enable_editing {
        target.set_editing_enabled(true);
    }
    let result = (|| {
        let handle = attributes.insert(Attribute::new(id, Behavior::StandaloneReference))?;
        if let Err(error) = attributes.add_attribute(actual.key, handle) {
            let _ = attributes.destroy(handle);
            return Err(error);
        }
        Ok(FaceMarker {
            handle,
            face: actual,
            restore_editing,
        })
    })();
    if result.is_err() {
        if let Some(editing) = restore_editing {
            target.set_editing_enabled(editing);
        }
    }
    result
}

pub(crate) fn end_face_marker(
    attributes: &mut AttributeStore,
    target: &mut Brep,
    marker: FaceMarker,
) -> Result<()> {
    marker.finish(attributes, target)
}

/// Actual stores used by the shared split-face reconciliation loop.
pub struct TargetTopologyChanges<'a> {
    target_serial: u64,
    target: &'a mut Brep,
    pseudo: &'a mut FilletBrep,
    attributes: &'a AttributeStore,
    solvers: &'a [Option<SolverObject>],
    corners: &'a [FilletCorner],
    base_side: Option<usize>,
}
impl<'a> TargetTopologyChanges<'a> {
    /// In a surface/surface fillet with a second base B-rep, the rail side whose base
    /// B-rep is this target (`target` is the second base B-rep for side 1). The other
    /// side's rails lie on faces of the other B-rep, which a split of this target's
    /// face never reaches (upstream compares face pointers; typed face ids of the two
    /// arenas may coincide).
    #[must_use]
    pub fn with_base_side(mut self, side: Option<usize>) -> Self {
        self.base_side = side;
        self
    }
    /// The rails of a surface/surface geometry's other side (see
    /// [`Self::with_base_side`]).
    fn foreign_rails(
        &self,
        geometry: crate::blend::fillet_geom::p1::FilletGeomId,
    ) -> Result<Vec<EdgeId>> {
        let Some(side) = self.base_side else {
            return Ok(Vec::new());
        };
        let geom = self.pseudo.geom(geometry)?;
        let surface_solver = geom.fillet_solver().is_some_and(|id| {
            matches!(
                self.solvers.get(id.0 as usize),
                Some(Some(SolverObject::SurfaceSurface(_)))
            )
        });
        if !surface_solver {
            return Ok(Vec::new());
        }
        let other = 1 - side.min(1);
        let mut rails: Vec<EdgeId> = geom.rail(other).into_iter().collect();
        rails.extend_from_slice(if other == 0 {
            &geom.other_rails1
        } else {
            &geom.other_rails2
        });
        Ok(rails)
    }
    /// Borrow the actual records; no geometry or solver state is copied.
    /// # Errors
    /// The supplied identity domain does not belong to the actual target.
    pub fn new(
        target_serial: u64,
        target: &'a mut Brep,
        pseudo: &'a mut FilletBrep,
        attributes: &'a AttributeStore,
        solvers: &'a [Option<SolverObject>],
        corners: &'a [FilletCorner],
    ) -> Result<Self> {
        if target.notify_serial() != Some(target_serial) {
            return Err(invalid("split-face target identity domain does not match"));
        }
        Ok(Self {
            target_serial,
            target,
            pseudo,
            attributes,
            solvers,
            corners,
            base_side: None,
        })
    }
    fn target_face(&self, face: FilletObject) -> Result<FaceId> {
        let FilletObject::Topology(FilletBody::Target, EntityRef::Face(face)) = face else {
            return Err(invalid("split-face update needs actual target faces"));
        };
        self.target.face(face)?;
        Ok(face)
    }
}
impl TopologyChangesAccess<FilletObject> for TargetTopologyChanges<'_> {
    type User = ObjectKey;
    fn change_users(&self, attribute: FilletObject) -> Result<Vec<ObjectKey>> {
        let FilletObject::Attribute(attribute) = attribute else {
            return Err(invalid("split-face marker is not an actual attribute"));
        };
        self.attributes.users(attribute)
    }
    fn change_face(&self, user: ObjectKey) -> Result<Option<FilletObject>> {
        for (face, _) in self.target.faces().iter() {
            let actual =
                crate::topology::brep::p3::topology_ref(self.target_serial, EntityRef::Face(face));
            if actual.key == user {
                return Ok(Some(FilletObject::Topology(
                    FilletBody::Target,
                    EntityRef::Face(face),
                )));
            }
        }
        for (edge, _) in self.target.edges().iter() {
            if crate::topology::brep::p3::topology_ref(self.target_serial, EntityRef::Edge(edge))
                .key
                == user
            {
                return Ok(None);
            }
        }
        // `SM_CAST_PTR(SmFace, sAllObjects[ii])` is NULL for every other kind of
        // object. The marker is a standalone reference added to one face of the base
        // B-rep, and face splits copy it to faces of that B-rep: a user of the base
        // B-rep that is not one of its faces (other topology, or the key of an object
        // since destroyed, which upstream's destructor removes from the users) is
        // skipped like a non-face user.
        if crate::topology::brep::p3::topology_entity(self.target_serial, user).is_some() {
            return Ok(None);
        }
        // A face of another B-rep cannot carry the marker; any other object (a
        // geometry or a non-topology user) is not a face.
        if let Some(serial) = self.pseudo.brep().notify_serial() {
            if let Some(EntityRef::Face(_)) =
                crate::topology::brep::p3::topology_entity(serial, user)
            {
                return Err(invalid("split-face marker user is a pseudo-B-rep face"));
            }
        }
        Ok(None)
    }
    fn change_geometries(&self, solver: FilletObject) -> Result<Vec<FilletObject>> {
        let FilletObject::Solver(id) = solver else {
            return Err(invalid("split-face solver is not retained"));
        };
        let solver = self
            .solvers
            .get(id.0 as usize)
            .and_then(Option::as_ref)
            .ok_or_else(|| invalid("split-face solver is not live"))?;
        Ok(solver
            .base()
            .fillet_geoms
            .iter()
            .copied()
            .map(FilletObject::Geometry)
            .collect())
    }
    fn change_geometry(
        &mut self,
        k: &Kernel,
        geometry: FilletObject,
        old: FilletObject,
        new: FilletObject,
    ) -> Result<()> {
        let FilletObject::Geometry(geometry) = geometry else {
            return Err(invalid("split-face geometry is not retained"));
        };
        let old = self.target_face(old)?;
        let new = self.target_face(new)?;
        // The other base B-rep's rails are not on the split face: withhold their
        // original faces from the update.
        let mut withheld = Vec::new();
        for rail in self.foreign_rails(geometry)? {
            let edge = self.pseudo.fillet_edge_mut(rail)?;
            withheld.push((rail, edge.original_face()));
            edge.set_original_face(None);
        }
        let result = self.pseudo.update_geom_split_face(
            k,
            geometry,
            Some(old),
            Some(new),
            self.target,
            self.corners,
        );
        for (rail, face) in withheld {
            self.pseudo.fillet_edge_mut(rail)?.set_original_face(face);
        }
        result
    }
    fn change_corner(
        &mut self,
        k: &Kernel,
        corner: FilletObject,
        old: FilletObject,
        new: FilletObject,
    ) -> Result<()> {
        let FilletObject::Corner(id) = corner else {
            return Err(invalid("split-face corner is not retained"));
        };
        let corner = self
            .corners
            .get(id.0 as usize)
            .ok_or_else(|| invalid("split-face corner is not live"))?;
        let old = self.target_face(old)?;
        let new = self.target_face(new)?;
        corner.update_split_face_with_breps(k, self.pseudo, self.target, old, new, self.corners)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::tol::USE_DEFAULT;
    use crate::math::Vec3;
    use crate::topology::brep::p3::{new_brep, topology_ref};
    use crate::SolidError;

    #[test]
    fn real_marker_copy_and_editing_are_scoped_on_all_results() {
        for enable_editing in [false, true] {
            for status in 0..3 {
                let mut k = Kernel::default();
                let mut target = new_brep(&mut k, USE_DEFAULT, USE_DEFAULT).unwrap();
                let shell = target.add_shell().unwrap();
                let first = target.add_face(shell).unwrap();
                let split = target.add_face(shell).unwrap();
                let serial = target.notify_serial().unwrap();
                target.set_editing_enabled(false);
                let mut attributes = AttributeStore::new();
                let unrelated = attributes
                    .insert(Attribute::new(55, Behavior::StandaloneReference))
                    .unwrap();
                attributes
                    .add_attribute(topology_ref(serial, EntityRef::Face(first)).key, unrelated)
                    .unwrap();
                let mut retained = None;
                let result = with_face_marker(
                    &mut attributes,
                    &mut target,
                    first,
                    if enable_editing { 777 } else { 778 },
                    enable_editing,
                    |attributes, target, marker| {
                        assert_eq!(target.editing_enabled(), enable_editing);
                        retained = Some(marker.attribute());
                        let copied = topology_ref(serial, EntityRef::Face(split));
                        attributes.copy(
                            marker.attribute(),
                            marker.original_face().key,
                            copied.key,
                        )?;
                        assert_eq!(
                            attributes.users(marker.attribute())?,
                            vec![marker.original_face().key, copied.key]
                        );
                        target.face_mut(first)?.tolerance = 0.125;
                        match status {
                            0 => Ok(()),
                            1 => Err(SolidError::sm_err("actual scope body")),
                            _ => Err(SolidError::unported(
                                "actual scope dependency",
                                "boolean.topology_intersector_p3",
                            )),
                        }
                    },
                );
                assert_eq!(result.is_ok(), status == 0);
                assert!(!target.editing_enabled());
                assert_eq!(target.face(first).unwrap().tolerance, 0.125);
                assert!(attributes.get(retained.unwrap()).is_none());
                assert_eq!(
                    attributes.attributes(topology_ref(serial, EntityRef::Face(first)).key),
                    vec![unrelated]
                );
                assert!(attributes
                    .attributes(topology_ref(serial, EntityRef::Face(split)).key)
                    .is_empty());
            }
        }
    }

    #[test]
    fn marker_users_that_are_not_base_faces_are_skipped() {
        let mut k = Kernel::default();
        let mut target = new_brep(&mut k, USE_DEFAULT, USE_DEFAULT).unwrap();
        let shell = target.add_shell().unwrap();
        let face = target.add_face(shell).unwrap();
        let vertex = target.add_vertex(Vec3::new(0., 0., 0.)).unwrap();
        let serial = target.notify_serial().unwrap();
        let mut pseudo = FilletBrep::new();
        *pseudo.brep_mut() = new_brep(&mut k, USE_DEFAULT, USE_DEFAULT).unwrap();
        let pseudo_shell = pseudo.brep_mut().add_shell().unwrap();
        let pseudo_face = pseudo.brep_mut().add_face(pseudo_shell).unwrap();
        let pseudo_serial = pseudo.brep().notify_serial().unwrap();
        let attributes = AttributeStore::new();
        let access =
            TargetTopologyChanges::new(serial, &mut target, &mut pseudo, &attributes, &[], &[])
                .unwrap();
        let key = |serial, entity| topology_ref(serial, entity).key;
        assert_eq!(
            access
                .change_face(key(serial, EntityRef::Face(face)))
                .unwrap(),
            Some(FilletObject::Topology(
                FilletBody::Target,
                EntityRef::Face(face)
            ))
        );
        // `SM_CAST_PTR(SmFace, ..)` is NULL for other topology of the base B-rep, and
        // a destroyed face is no user upstream.
        for user in [
            key(serial, EntityRef::Vertex(vertex)),
            key(serial, EntityRef::Shell(shell)),
            key(serial, EntityRef::Face(FaceId::from_index(99))),
            ObjectKey(7),
        ] {
            assert_eq!(access.change_face(user).unwrap(), None);
        }
        // A face of another B-rep cannot carry the base face's marker.
        let error = access
            .change_face(key(pseudo_serial, EntityRef::Face(pseudo_face)))
            .unwrap_err();
        assert!(!error.is_unported());
    }

    #[test]
    fn actual_intersector_and_edge_users_keep_body_identity() {
        let mut k = Kernel::default();
        let mut target = new_brep(&mut k, USE_DEFAULT, USE_DEFAULT).unwrap();
        let a = target.add_vertex(Vec3::new(0., 0., 0.)).unwrap();
        let b = target.add_vertex(Vec3::new(1., 0., 0.)).unwrap();
        let target_edge = target.add_edge(a, b).unwrap();
        let mut session = FilletSession::new(&mut k, &mut target).unwrap();
        let mut intermediate = new_brep(&mut k, USE_DEFAULT, USE_DEFAULT).unwrap();
        let a = intermediate.add_vertex(Vec3::new(0., 1., 0.)).unwrap();
        let b = intermediate.add_vertex(Vec3::new(1., 1., 0.)).unwrap();
        let edge = intermediate.add_edge(a, b).unwrap();
        assert_eq!(target_edge, edge);
        session.objects.intermediate = Some(intermediate);
        {
            let state = session.insertion_state().unwrap();
            state.intersector.topology_deleted = true;
            let target_key = topology_ref(state.target_serial, EntityRef::Edge(target_edge)).key;
            let actual = topology_ref(
                state.intermediate.notify_serial().unwrap(),
                EntityRef::Edge(edge),
            )
            .key;
            assert_ne!(target_key, actual);
            state
                .attributes
                .add_attribute(target_key, state.edge_attribute)
                .unwrap();
            state
                .attributes
                .add_attribute(actual, state.edge_attribute)
                .unwrap();
            assert_eq!(
                edge_attribute_users(state.attributes, state.edge_attribute, state.intermediate)
                    .unwrap(),
                vec![edge]
            );
        }
        assert!(session.executive.intersector.topology_deleted);
        let result: Result<()> = session.with_insertion_geometry(&mut k, |_, ti, _| {
            ti.ignore_tangency = true;
            Err(SolidError::sm_err("actual component failure"))
        });
        assert!(result.is_err());
        assert!(session.executive.intersector.ignore_tangency);
        // Returning the real lease leaves the original Context usable.
        let mut slot = crate::geom::context::IdentitySlot::new();
        assert!(k.context.geometry_key(&mut slot).is_ok());
    }
}
