//! The live session's piecewise merge of the intermediate fillet body into the target.
//!
//! Upstream: `source/SMLib/src/SmFilletExecutive.cpp` (`DoPiecewiseMerge` 2676-2907),
//! `source/SMLib/src/SmMerge.cpp` (`PiecewiseMerge` 341-794).
//!
//! | Rust | Upstream |
//! |------|----------|
//! | `FilletSession::do_piecewise_merge` (crate) | `SmFilletExecutive::DoPiecewiseMerge` |
//! | `PiecewiseIdentities` (private) | the addresses of the target, the intermediate body and the merge's stand-alone face B-reps |
//!
//! Deviations: `PiecewiseMerge`'s leading `m_vTI.IntersectInsertRelate()` runs on the
//! executive's own intersector (the one its corners registered their SSI on), as
//! upstream's inherited `m_vTI` is that intersector; its face-at-a-time Booleans
//! ([`crate::boolean::merge::p3::piecewise_merge_related`]) run in a [`SweepStore`]
//! over the session's notification records and attributes, whose merge is
//! `SmMerge(context)` with the executive's tolerances and controls copied
//! (`m_bIgnoreTangency`, `m_bDoingFillets`; `m_bManifoldBoolean`, which that
//! constructor leaves uninitialised, starts `TRUE` as the full constructor sets it;
//! the first face's `PartialMerge` into an empty B-rep has no common vertices, the
//! only place the flag is read, and every later Boolean sets it `FALSE`). The target
//! and intermediate bodies move into the store for the merge and back afterwards,
//! also on an error. The bodies are adopted as they are: no existing Curve or
//! Surface owner is rewritten (native operands keep their owners); every existing
//! curve is named by its actual Curve identity, issued before adoption, and a curve
//! the merge allocates at an edge split takes its actual identity at construction
//! (`SweepIdentities::actual_curve_identities`); only geometry the store's Boolean
//! bound under a target session identity returns to the target's naming afterwards
//! (the same owner entity). The store-allocated stand-alone face B-reps' unissued
//! curve copies keep their namespace identities. A failed `IntersectInsertRelate`
//! ends the merge, as the shared `piecewise_merge` port does. Target surfaces are
//! named by their face's session identity, as the store's merge names them; a
//! fillet geometry whose surface is not installed in the intermediate body has no
//! face there, so relating it or attributing it has no effect and is skipped. The
//! void-flag pass claims a real `SmNewMarkAndLock` on the context and marks the
//! regions' notification records (`Region::Mark`); the lock is held through
//! `MakeManifold`, `RemoveTopologicalEdgesAndVertices` and the deletion of the
//! intermediate body (moved here from the caller, as upstream deletes it inside
//! `DoPiecewiseMerge`), and released on every return. A null region attribute
//! (a native null dereference) is an input error. No recursion.

use super::finish::FinishAccess;
use super::{invalid, FilletObject, FilletSession};
use crate::blend::fillet_executive::p3::FilletFinishAccess;
use crate::blend::fillet_geom::p1::{FilletGeomType, FilletSurfaceStorage};
use crate::boolean::merge::p4::{notification_key, KernelManifoldIntersector};
use crate::curve::curve::Curve;
use crate::error::{EntityRef, Result, SolidError};
use crate::geom::attribute::{ids, Attribute, AttributeValue, Behavior, ObjectKey};
use crate::geom::context::{MarkLock, MarkType, ALL_MARKS};
use crate::geom::object::ObjectRef;
use crate::geom::tol::Relation;
use crate::kernel::Kernel;
use crate::math::{Point2, Vec3, EFF_ZERO};
use crate::nurbs::knots::Side;
use crate::sweep::topology_sweep::access::SweepAccess;
use crate::sweep::topology_sweep::store::{SweepIdentities, SweepStore};
use crate::topology::brep::p3::{seed_records, topology_ref};
use crate::topology::brep::{p1, p2, p5};
use crate::topology::brep_cache::{arena_identity, CacheObject};
use crate::topology::loopuse::notify_ref;
use crate::topology::{
    edgeuse, face, vertex, Brep, BrepId, EdgeId, FaceId, NamingScheme, RegionId,
};

const OP: &str = "blend::fillet_executive::session::piecewise";

/// The store handle of the target (the first body the store receives).
const TARGET: BrepId = BrepId::from_index(0);
/// The store handle of the intermediate body.
const INTERMEDIATE: BrepId = BrepId::from_index(1);

/// Identities of the merge's bodies: the target's and the intermediate body's are
/// their session identities (`topology_ref` of their issued serials, the B-rep's own
/// key), every later body's (the stand-alone face B-reps) the merge namespace of its
/// handle.
struct PiecewiseIdentities {
    target: (u64, ObjectKey),
    intermediate: (u64, ObjectKey),
}

impl PiecewiseIdentities {
    fn body(&self, brep: BrepId) -> Option<(u64, ObjectKey)> {
        match brep {
            TARGET => Some(self.target),
            INTERMEDIATE => Some(self.intermediate),
            _ => None,
        }
    }
    /// The namespace identity of an unissued curve of a stand-alone face B-rep the
    /// store allocated (the target's and intermediate body's curves all carry their
    /// actual identities before adoption).
    fn face_brep_curve(brep: BrepId, edge: EdgeId, type_code: i64) -> ObjectRef {
        ObjectRef {
            type_code,
            key: notification_key(brep.index().saturating_add(1), true, EntityRef::Edge(edge)).key,
        }
    }
}

impl SweepIdentities for PiecewiseIdentities {
    fn topology(&self, brep: BrepId, entity: EntityRef) -> ObjectRef {
        match self.body(brep) {
            Some((serial, key)) => {
                let identity = topology_ref(serial, entity);
                if matches!(entity, EntityRef::Brep(_)) {
                    ObjectRef { key, ..identity }
                } else {
                    identity
                }
            }
            None => notification_key(brep.index().saturating_add(1), false, entity),
        }
    }
    fn owned_auxiliary_objects(&self, brep: BrepId, model: &Brep) -> Vec<ObjectKey> {
        if self.body(brep).is_some() {
            // the session keeps the target's and the intermediate body's objects
            return Vec::new();
        }
        let mut keys = Vec::new();
        for (edge, record) in model.edges().iter() {
            if let Some(curve) = &record.curve {
                keys.push(self.curve(brep, edge, curve).key);
            }
        }
        for (face, record) in model.faces().iter() {
            if record.surface.is_some() {
                if let Ok(surface) = KernelManifoldIntersector::surface_identity(model, face) {
                    keys.push(surface.key);
                }
            }
        }
        keys
    }
    fn curve(&self, brep: BrepId, edge: EdgeId, curve: &Curve) -> ObjectRef {
        // the actual Curve identity (issued before adoption, or at construction)
        curve.notify_key().map_or_else(
            || Self::face_brep_curve(brep, edge, curve.type_code()),
            |key| ObjectRef {
                key,
                type_code: curve.type_code(),
            },
        )
    }
    fn new_curve(&self, _brep: BrepId, _edge: EdgeId, _type_code: i64) -> Result<ObjectRef> {
        // A split curve takes its actual identity when it is constructed
        // (`actual_curve_identities`); no edge slot names a curve object.
        Err(invalid(
            "a new piecewise merge curve takes its actual identity at construction",
        ))
    }
    fn actual_curve_identities(&self) -> bool {
        true
    }
    fn source_lists(&self, brep: BrepId, region: RegionId) -> Result<[ObjectRef; 2]> {
        let namespace = u64::from(brep.index().saturating_add(1));
        Ok([13_u64, 14].map(|kind| ObjectRef {
            type_code: 0,
            key: ObjectKey((namespace << 37) | (kind << 32) | u64::from(region.index())),
        }))
    }
}

/// Issues the actual Curve identity of every edge curve of `b` through `context`
/// (the slot each curve object already carries; an issued one is kept), before `b`
/// is adopted by the merge's store. Owners, geometry and naming are untouched: the
/// native operands enter `PiecewiseMerge` as they are, and the store names each
/// existing curve by this identity ([`PiecewiseIdentities::curve`]).
fn issue_actual_curve_identities(
    context: &mut crate::geom::context::Context,
    b: &mut Brep,
) -> Result<()> {
    let edges: Vec<EdgeId> = b.edges().ids().collect();
    for edge in edges {
        crate::topology::brep::p3::edge_curve_identity(context, b, edge)?;
    }
    Ok(())
}

/// After the merge, the geometry the store's Boolean bound to `b`'s edges, edge uses
/// (their UV trim curves) and faces under their session identity (`session`) is rebound to the same entity's
/// identity in `b`'s naming (`to`): the owner is the same edge or face, only its
/// key's namespace changes. Geometry owned under any other identity is untouched.
pub(super) fn normalize_session_owners(
    b: &mut Brep,
    session: impl Fn(EntityRef) -> ObjectRef,
    to: impl Fn(EntityRef) -> ObjectRef,
) -> Result<()> {
    let edges: Vec<EdgeId> = b.edges().ids().collect();
    for edge in edges {
        let e = EntityRef::Edge(edge);
        let from = session(e);
        let new_owner = to(e);
        let rebound = b.edge_mut(edge)?.curve.as_mut().is_some_and(|curve| {
            let matched = curve.owner() == Some(from);
            if matched {
                curve.set_owner(Some(new_owner), None);
            }
            matched
        });
        if rebound {
            b.set_geometry_owner_key(e, Some(new_owner));
        }
    }
    let edgeuses: Vec<crate::topology::EdgeuseId> = b.edgeuses().ids().collect();
    for eu in edgeuses {
        let e = EntityRef::Edgeuse(eu);
        let from = session(e);
        let new_owner = to(e);
        let rebound = b
            .edgeuse_mut(eu)?
            .uv_trim_curve
            .as_mut()
            .is_some_and(|curve| {
                let matched = curve.owner() == Some(from);
                if matched {
                    curve.set_owner(Some(new_owner), None);
                }
                matched
            });
        if rebound {
            b.set_geometry_owner_key(e, Some(new_owner));
        }
    }
    let faces: Vec<FaceId> = b.faces().ids().collect();
    for face in faces {
        let e = EntityRef::Face(face);
        let from = session(e);
        let new_owner = to(e);
        let rebound = b.face_mut(face)?.surface.as_mut().is_some_and(|surface| {
            let matched = surface.owner() == Some(from);
            if matched {
                surface.set_owner(Some(new_owner));
            }
            matched
        });
        if rebound {
            b.set_geometry_owner_key(e, Some(new_owner));
            b.sync_face_surface_tolerance(face)?;
        }
    }
    Ok(())
}

/// The identity a B-rep of naming `naming` owns an edge's or face's geometry under,
/// `None` for a caller's or an unknown scheme.
pub(super) fn naming_owner(naming: NamingScheme, e: EntityRef) -> Option<ObjectRef> {
    match (naming, e) {
        (NamingScheme::Arena, EntityRef::Edge(x)) => Some(arena_identity(CacheObject::Edge(x))),
        (NamingScheme::Arena, EntityRef::Face(x)) => Some(arena_identity(CacheObject::Face(x))),
        (NamingScheme::Notify, e) => Some(notify_ref(e)),
        _ => None,
    }
}

/// `pRegion->IsVoid()` of the region of `brep`'s edge use (`NER` on each null).
fn edgeuse_region_is_void(brep: &Brep, eu: Option<crate::topology::EdgeuseId>) -> Result<i32> {
    let eu = eu.ok_or_else(|| SolidError::null_pointer(OP))?;
    brep.edgeuse(eu)?
        .edge
        .ok_or_else(|| SolidError::null_pointer(OP))?;
    let shell = edgeuse::p1::shell(brep, eu)?.ok_or_else(|| SolidError::null_pointer(OP))?;
    // pShell->GetRegion()->IsVoid() dereferences the region unchecked.
    let region = brep
        .shell(shell)?
        .region
        .ok_or_else(|| invalid("fillet edge use shell has no region"))?;
    Ok(brep.region(region)?.is_void())
}

/// The surface identity of `brep`'s face, `None` without a surface.
fn surface_of(brep: &Brep, face: FaceId) -> Result<Option<ObjectRef>> {
    if brep.face(face)?.surface.is_none() {
        return Ok(None);
    }
    KernelManifoldIntersector::surface_identity(brep, face).map(Some)
}

/// The identities a B-rep's own edge or face geometry is owned under: the one of
/// its naming scheme ([`naming_owner`]) and the recorded one. Another scheme's
/// identity with an equal raw key names a different owner and is not accepted.
fn own_identities(b: &Brep, e: EntityRef) -> Vec<ObjectRef> {
    let mut ids = Vec::new();
    ids.extend(naming_owner(b.naming(), e));
    ids.extend(b.geometry_owner_key(e));
    ids
}

/// The surface identity of a target face as the merge names it: its session face
/// identity when the face owns its surface, else the surface's owner.
fn target_surface(b: &Brep, serial: u64, face: FaceId) -> Result<Option<ObjectRef>> {
    let Some(surface) = surface_of(b, face)? else {
        return Ok(None);
    };
    let e = EntityRef::Face(face);
    let session = topology_ref(serial, e);
    Ok(Some(
        if surface == session || own_identities(b, e).contains(&surface) {
            session
        } else {
            surface
        },
    ))
}

impl FilletSession<'_> {
    /// `SmFilletExecutive::DoPiecewiseMerge`: relate each fillet surface
    /// with the original surfaces it meets, insert the corners' intersection
    /// topology, merge the intermediate body into the target one face at a time,
    /// then set the void flags of the regions the fillet faces bound and make the
    /// target manifold on its solid regions and delete the intermediate body, under
    /// one claimed mark.
    ///
    /// # Errors
    /// Missing executive objects, the corners' insertion, the merge, the mark lock,
    /// and the manifold, topology-removal and deletion edits.
    pub(super) fn do_piecewise_merge(&mut self, k: &mut Kernel) -> Result<()> {
        let manifold_solid = p2::is_manifold_solid(self.objects.target)?;
        // the current topological edges and vertices remain after the fillets
        let original_edges = p1::topological_edges(k, self.objects.target)?;
        let original_vertices = p1::topological_vertices(k, self.objects.target)?;

        let target_serial = self.objects.target_serial;
        let region_key = |region| topology_ref(target_serial, EntityRef::Region(region)).key;
        for region in p1::regions(self.objects.target, None)? {
            let is_void = self.objects.target.region(region)?.is_void();
            let h = self.objects.attributes.insert(Attribute::long(
                ids::FILLET_REGIONMAP,
                i64::from(is_void),
                Behavior::Reference,
            ))?;
            self.objects
                .attributes
                .add_attribute(region_key(region), h)?;
        }

        // Non-intersecting surface pairs (fillet surface, original surface).
        let mut no_ssi: Relation<ObjectRef, ObjectRef> = Relation::default();
        let solver_order = self.executive.solvers.clone();
        for solver in solver_order {
            let FilletObject::Solver(id) = solver else {
                return Err(invalid("piecewise solver is not retained"));
            };
            let solver = self
                .objects
                .solvers
                .get(id.0 as usize)
                .and_then(Option::as_ref)
                .ok_or_else(|| invalid("piecewise solver absent"))?
                .base();
            let geoms = solver.fillet_geoms.clone();
            let first_edgeuse = solver.edgeuses.first().copied().flatten();
            for geom in geoms {
                let record = self.objects.pseudo.geom(geom)?;
                // Don't use cliff side patch for classification purposes
                if record.geom_type == FilletGeomType::CliffSidePatch {
                    continue;
                }
                let inside_fillet = edgeuse_region_is_void(self.objects.target, first_edgeuse)?;
                let rails = record.rails;
                let fillet_face = match record.fillet_surface {
                    Some(FilletSurfaceStorage::ManagedFace { face, .. }) => Some(face),
                    _ => None,
                };
                for rail in rails {
                    let rail = rail.ok_or_else(|| SolidError::null_pointer(OP))?;
                    let original = self.objects.pseudo.fillet_edge(rail)?.original_face();
                    let original_surface = original
                        .map(|face| target_surface(self.objects.target, target_serial, face))
                        .transpose()?
                        .flatten();
                    let fillet_surface = fillet_face
                        .map(|face| self.intermediate_surface(face))
                        .transpose()?
                        .flatten();
                    // skip rails without original faces or surfaces
                    let (Some(original_surface), Some(fillet_surface)) =
                        (original_surface, fillet_surface)
                    else {
                        continue;
                    };
                    let h = self.objects.attributes.insert(Attribute::long(
                        ids::FILLET_EDGEMAP,
                        i64::from(inside_fillet),
                        Behavior::Reference,
                    ))?;
                    self.objects
                        .attributes
                        .add_attribute(fillet_surface.key, h)?;
                    no_ssi.relate_pair(fillet_surface, original_surface)?;
                }
            }
        }

        let corner_order = self.executive.corners.clone();
        for corner in corner_order {
            let FilletObject::Corner(id) = corner else {
                return Err(invalid("piecewise corner is not retained"));
            };
            self.insert_corner_topology(k, corner)?;
            let record = self
                .objects
                .corners
                .get(id.0 as usize)
                .ok_or_else(|| invalid("piecewise corner absent"))?;
            let mut corner_surfaces = Vec::new();
            for index in 0..record.surfaces.len() {
                corner_surfaces.push(
                    record
                        .surface_faces
                        .get(index)
                        .map(|&face| self.intermediate_surface(face))
                        .transpose()?
                        .flatten(),
                );
            }
            if record.blending {
                let vertex = record
                    .vertex
                    .ok_or_else(|| invalid("blending corner has no vertex"))?;
                for face in vertex::faces(self.objects.target, vertex)? {
                    // skip fillet vertices not on an original face
                    let Some(original_surface) =
                        target_surface(self.objects.target, target_serial, face)?
                    else {
                        continue;
                    };
                    for surface in corner_surfaces.iter().copied().flatten() {
                        no_ssi.relate_pair(surface, original_surface)?;
                        let h = self.objects.attributes.insert(Attribute::long(
                            ids::FILLET_VERTMAP,
                            0,
                            Behavior::Reference,
                        ))?;
                        self.objects.attributes.add_attribute(surface.key, h)?;
                    }
                }
            }
            // Merge each edge of corner into originating face
            for &edge in &record.edges {
                let Some(face) = self.objects.pseudo.fillet_edge(edge)?.original_face() else {
                    continue;
                };
                let Some(original_surface) =
                    target_surface(self.objects.target, target_serial, face)?
                else {
                    continue;
                };
                for surface in corner_surfaces.iter().copied().flatten() {
                    no_ssi.relate_pair(surface, original_surface)?;
                }
            }
        }

        let other_tolerance = self
            .objects
            .intermediate
            .as_ref()
            .ok_or_else(|| invalid("intermediate body is not allocated"))?
            .tolerance();
        self.executive
            .intersector
            .set_this_approx_tol_3d(other_tolerance);
        self.piecewise_merge(k, &no_ssi)?;

        // increment and lock an unlocked mark, held until the function returns
        let mut lock = MarkLock::new(std::slice::from_mut(&mut k.context), 0, ALL_MARKS)?;
        let mark = lock.mark_type();
        let result = (|| {
            self.set_fillet_region_void_flags(k, mark, region_key)?;

            // Find solid regions and make manifold using them
            let target = &mut *self.objects.target;
            let mut keep = Vec::new();
            for region in p1::regions(target, None)? {
                if target.region(region)?.is_void() == 0 && target.infinite_region() != Some(region)
                {
                    keep.push(region);
                }
            }
            if !keep.is_empty() || !manifold_solid {
                p5::make_manifold(k, target, Some(&keep), false)?;
                // remove any new topological edges and vertices
                p5::remove_topological_edges_and_vertices(
                    k,
                    target,
                    Some(&original_edges),
                    Some(&original_vertices),
                )?;
            }
            // clean up: delete m_pFilletBrep
            FinishAccess(&mut self.objects).finish_delete_brep(k, FilletObject::IntermediateBrep)
        })();
        // ~SmNewMarkAndLock, on every return
        let released = lock.release(std::slice::from_mut(&mut k.context));
        result.and(released)
    }

    /// The surface identity of an intermediate face.
    fn intermediate_surface(&self, face: FaceId) -> Result<Option<ObjectRef>> {
        let body = self
            .objects
            .intermediate
            .as_ref()
            .ok_or_else(|| invalid("intermediate body is not allocated"))?;
        surface_of(body, face)
    }

    /// `PiecewiseMerge(&no_ssi, TRUE, TRUE, pResult)` on the executive's merge, with
    /// the target as `m_vTI.m_pBrep` and the intermediate body as `m_vTI.m_pOther`.
    fn piecewise_merge(
        &mut self,
        k: &mut Kernel,
        no_ssi: &Relation<ObjectRef, ObjectRef>,
    ) -> Result<()> {
        // The first surface query of m_pOther has no effect (it is overwritten).
        self.with_insertion_geometry(k, |k, ti, geometry| {
            ti.intersect_insert_relate(k, geometry, FilletObject::Intersector)
                .map(|_| ())
        })?;
        let tolerance = self.executive.intersector.this_approx_tol_3d();
        let angle = self.executive.intersector.this_ang_tol_rad();
        let ignore_tangency = self.executive.intersector.ignore_tangency;
        let doing_fillets = self.executive.intersector.doing_fillets;
        let intermediate_serial = self
            .objects
            .intermediate
            .as_ref()
            .ok_or_else(|| invalid("intermediate body is not allocated"))?
            .notify_serial()
            .ok_or_else(|| invalid("intermediate body has no kernel identity"))?;
        let mut intermediate = self
            .objects
            .intermediate
            .take()
            .ok_or_else(|| invalid("intermediate body is not allocated"))?;
        let mut target = std::mem::take(&mut *self.objects.target);
        // Both operands enter the store as they are, with their actual curve
        // identities issued; the store's Boolean binds the geometry it creates to
        // the target's session identities, which return to its naming afterwards.
        let target_serial = self.objects.target_serial;
        let naming = target.naming();
        let session_owner = |e| topology_ref(target_serial, e);
        let issued = issue_actual_curve_identities(&mut k.context, &mut target)
            .and_then(|()| issue_actual_curve_identities(&mut k.context, &mut intermediate));
        let identities = PiecewiseIdentities {
            target: (self.objects.target_serial, target.key()),
            intermediate: (intermediate_serial, intermediate.key()),
        };
        let mut next_handle = 0;
        let mut store = SweepStore::new(
            &identities,
            &mut self.objects.notification_records,
            &mut self.objects.attributes,
            &mut next_handle,
        );
        let mut bodies = (Some(target), Some(std::mem::take(&mut intermediate)));
        let context = store.register_kernel(k);
        let result = (|| {
            issued?;
            let context = context?;
            let target = bodies
                .0
                .take()
                .ok_or_else(|| SolidError::null_pointer(OP))?;
            let id = store.adopt_brep(context, target)?;
            if id != TARGET {
                return Err(invalid("target store handle"));
            }
            let other = bodies
                .1
                .take()
                .ok_or_else(|| SolidError::null_pointer(OP))?;
            let id = store.adopt_brep(context, other)?;
            if id != INTERMEDIATE {
                return Err(invalid("intermediate store handle"));
            }
            store.with_merge(
                TARGET,
                TARGET,
                INTERMEDIATE,
                tolerance,
                angle,
                |s, merge| {
                    let intersector = s.merge_intersector_mut(merge)?;
                    intersector.set_ignore_tangency(ignore_tangency);
                    intersector.set_doing_fillets(doing_fillets);
                    s.piecewise_merge_related_skipping(merge, Some(no_ssi), true, true)
                },
            )
        })();
        // Both bodies return to the session, also after an error.
        let target = store.take_brep(TARGET).ok().or(bodies.0);
        let other = store.take_brep(INTERMEDIATE).ok().or(bodies.1);
        drop(store);
        let mut target = target.ok_or_else(|| invalid("target lost in the merge"))?;
        let restored = normalize_session_owners(&mut target, session_owner, |e| {
            naming_owner(naming, e).unwrap_or_else(|| session_owner(e))
        });
        target.set_naming(naming);
        *self.objects.target = target;
        restored?;
        self.objects.intermediate = other;
        match result {
            Ok(TARGET) => Ok(()),
            Ok(_) => Err(invalid("piecewise merge result is not the target")),
            Err(error) => Err(error),
        }
    }

    /// The void flags of the regions the merged fillet faces bound (2802-2856), then
    /// the removal of the fillet map attributes from the target's surfaces.
    fn set_fillet_region_void_flags(
        &mut self,
        k: &Kernel,
        mark: MarkType,
        region_key: impl Fn(RegionId) -> ObjectKey,
    ) -> Result<()> {
        let serial = self.objects.target_serial;
        let target = &mut *self.objects.target;
        let attributes = &mut self.objects.attributes;
        // the regions' notification records carry their marks
        let records = &mut self.objects.notification_records;
        seed_records(records, target, &|entity| topology_ref(serial, entity));
        let faces = p1::faces(target, None)?;
        for &f in &faces {
            let Some(surface_key) = target_surface(target, serial, f)?.map(|s| s.key) else {
                return Err(SolidError::null_pointer(OP));
            };
            if attributes
                .find_attribute(surface_key, ids::FILLET_EDGEMAP)
                .is_none()
            {
                continue;
            }
            let domain = face::p2::uv_domain(target, f)?;
            let uv: Point2 = domain.evaluate(0.5, 0.5);
            let surface = target
                .face(f)?
                .surface
                .as_ref()
                .ok_or_else(|| SolidError::null_pointer(OP))?;
            let normal: Vec3 = surface.evaluate_normal(k, uv, Side::Left, Side::Left)?;
            let [_, _, _, _, _, dvv] = surface.evaluate_2nd_derivatives(k, uv, true, true)?;
            let mut faceuse =
                face::p2::upward_faceuse(target, f)?.ok_or_else(|| SolidError::null_pointer(OP))?;
            // See if the top Faceuse on the inside of the fillet
            if dvv.length() < EFF_ZERO {
                continue;
            }
            if normal.dot(dvv) > 0.0 {
                faceuse = target
                    .faceuse(faceuse)?
                    .mate
                    .ok_or_else(|| SolidError::null_pointer(OP))?;
            }
            let region_of = |fu| -> Result<Option<RegionId>> {
                let shell = target
                    .faceuse(fu)?
                    .shell
                    .ok_or_else(|| SolidError::null_pointer(OP))?;
                Ok(target.shell(shell)?.region)
            };
            let region = region_of(faceuse)?;
            let mate = target
                .faceuse(faceuse)?
                .mate
                .ok_or_else(|| SolidError::null_pointer(OP))?;
            if region_of(mate)? == region {
                continue;
            }
            let region = region.ok_or_else(|| invalid("fillet faceuse shell has no region"))?;
            if records
                .get(&region_key(region))
                .is_some_and(|record| record.is_marked(&k.context, mark))
            {
                continue;
            }
            let h = attributes
                .find_attribute(region_key(region), ids::FILLET_REGIONMAP)
                .ok_or_else(|| invalid("fillet region has no region-map attribute"))?;
            let is_void = match attributes.get(h).map(Attribute::value) {
                Some(AttributeValue::Long(value)) => *value,
                _ => return Err(invalid("region-map attribute is not a long")),
            };
            let inside_fillet = is_void == 0;
            records
                .or_default(region_key(region))
                .mark(Some(&k.context), mark);
            target
                .region_mut(region)?
                .set_is_void(i32::from(inside_fillet));
        }
        // Now clean up attributes
        for &f in &faces {
            let Some(surface_key) = target_surface(target, serial, f)?.map(|s| s.key) else {
                return Err(SolidError::null_pointer(OP));
            };
            for id in [ids::FILLET_EDGEMAP, ids::FILLET_VERTMAP] {
                if let Some(h) = attributes.find_attribute(surface_key, id) {
                    attributes.remove_attribute(surface_key, h, false)?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::Point3;

    #[test]
    fn piecewise_merge_fillets_one_box_edge() {
        let mut k = Kernel::default();
        let mut target = crate::primitive::api_primitives::create_box(
            &mut k,
            Point3::new(0.0, 0.0, 0.0),
            10.0,
            20.0,
            30.0,
        )
        .unwrap();
        let edges = p1::edges(&target, None).unwrap();
        {
            let mut session = FilletSession::new(&mut k, &mut target).unwrap();
            session
                .prepare_constant_radius_edges(&mut k, &edges[..1], 0.5, 1, 1, 1.0)
                .unwrap();
            session.set_do_piecewise_merge(true);
            session.do_fillet(&mut k).unwrap();
        }
        // the six box faces and the fillet face
        assert_eq!(p1::faces(&target, None).unwrap().len(), 7);
    }

    /// The void-flag pass holds a real `SmNewMarkAndLock` (2809-2856): one unlocked
    /// mark is advanced and stays locked through manifold construction, topology
    /// removal and the intermediate body's deletion, then is released.
    #[test]
    fn piecewise_void_flags_claim_and_release_a_real_mark() {
        let mut k = Kernel::default();
        let mut target =
            crate::primitive::api_primitives::create_box(&mut k, Point3::ZERO, 10.0, 20.0, 30.0)
                .unwrap();
        let edges = p1::edges(&target, None).unwrap();
        let before = k.context.current_mark(MarkType::Mark);
        {
            let mut session = FilletSession::new(&mut k, &mut target).unwrap();
            session
                .prepare_constant_radius_edges(&mut k, &edges[..1], 0.5, 1, 1, 1.0)
                .unwrap();
            session.set_do_piecewise_merge(true);
            session.do_fillet(&mut k).unwrap();
            assert!(session.objects.intermediate.is_none());
        }
        assert!(k.context.current_mark(MarkType::Mark) > before);
        for mark in [MarkType::Mark, MarkType::Mark2, MarkType::Mark3] {
            assert!(!k.context.is_mark_locked(mark));
        }
    }

    /// Private regression for the adapter's registration step, before its Boolean.
    /// Native `DoPiecewiseMerge` never changes an existing geometry's owner merely
    /// to name it for the merge (SmFilletExecutive.cpp2676-2800).
    #[test]
    fn piecewise_registration_preserves_original_issued_curve_owner() {
        let mut k = Kernel::default();
        let mut target =
            crate::primitive::api_primitives::create_box(&mut k, Point3::ZERO, 10.0, 20.0, 30.0)
                .unwrap();
        let session = FilletSession::new(&mut k, &mut target).unwrap();
        let serial = session.objects.target_serial;
        let edge = p1::edges(session.objects.target, None)
            .unwrap()
            .into_iter()
            .find(|&edge| {
                let owner = session
                    .objects
                    .target
                    .edge(edge)
                    .unwrap()
                    .curve
                    .as_ref()
                    .unwrap()
                    .owner();
                owner.is_some() && owner != Some(topology_ref(serial, EntityRef::Edge(edge)))
            })
            .expect("the real primitive retains its producer owner before merge registration");
        let original_identity = crate::topology::brep::p3::edge_curve_identity(
            &mut k.context,
            session.objects.target,
            edge,
        )
        .unwrap()
        .unwrap();
        let original_curve = session
            .objects
            .target
            .edge(edge)
            .unwrap()
            .curve
            .as_ref()
            .unwrap();
        let original_owner = original_curve.owner();
        let parameter = original_curve.natural_interval().evaluate(0.5);
        let original_point = original_curve.evaluate_point(&k, parameter).unwrap();
        let original_body_key = session.objects.target.key();
        let original_naming = session.objects.target.naming();

        // Exact production call at piecewise_merge's pre-adoption boundary.
        issue_actual_curve_identities(&mut k.context, session.objects.target).unwrap();

        let current_curve = session
            .objects
            .target
            .edge(edge)
            .unwrap()
            .curve
            .as_ref()
            .unwrap();
        assert_eq!(current_curve.notify_key(), Some(original_identity.key));
        assert_eq!(
            current_curve.evaluate_point(&k, parameter).unwrap(),
            original_point
        );
        assert_eq!(session.objects.target.key(), original_body_key);
        assert_eq!(session.objects.target.naming(), original_naming);
        eprintln!("actual_curve={original_identity:?} original_owner={original_owner:?} registration_owner={:?}", current_curve.owner());
        assert_eq!(current_curve.owner(), original_owner,
            "registering the existing target must not rewrite the same actual curve's producer owner");
    }
}
