//! Retained merge engines over the store's bodies, kernels and notification state.
//!
//! Upstream: `source/SMLib/src/SmMerge.cpp`, `source/SMLib/src/SmTopologySweep.cpp`.
//!
//! | Rust | Upstream |
//! |------|----------|
//! | `Session` | scoped `SmMerge` objects in `SmTopologySweep::DoSweep` |
//! | `Driver` | operand access for `SmMerge::PiecewiseMerge` / `ManifoldBoolean` |
//!
//! Deviations: never-reused handles replace pointers. The existing merge ports own
//! all algorithms. Operands move into their nullable slots for Boolean calls and
//! surviving bodies return to the same store slots, including after errors; a
//! caller-borrowed operand (`DoSweep` merging into `pBrepToSweepInto`) lends its
//! content and gets the edited body back in the caller's own B-rep. The
//! scope releases its temporary bodies; traversal caches follow body identities.
//! The merge constructor's context may differ from the primary body's context;
//! subsequent modeling services use the actual operand contexts.

use super::super::access::{BTreeEdgeMap, SweepAccess, SweepMergeId};
use super::{
    body_keys, body_pair_mut, entry, entry_mut, invalid, Body, BodyEntry, BodySlots, ContextId,
    Identities, KernelEntry, SweepStore, OP,
};
use crate::boolean::merge::p1::{MergeOptions, MergeState};
use crate::boolean::merge::p2::MergeOperand;
use crate::boolean::merge::p3::{BooleanDriver, MergeEngine, PiecewiseAccess};
use crate::boolean::merge::p4::{BooleanKernels, ManifoldIntersector, ManifoldState};
use crate::boolean::merge::p4::{
    KernelManifoldIntersector, ManifoldKernelAdapter, ManifoldNotificationKeys,
};
use crate::error::{EntityRef, Result, SolidError};
use crate::geom::a_object::BooleanOperation;
use crate::geom::attribute::{ids, Attribute, AttributeStore, Behavior, ObjectKey};
use crate::geom::context::MarkLock;
use crate::geom::object::NotifyOperation;
use crate::geom::object::ObjectRef;
use crate::kernel::Kernel;
use crate::topology::brep::p3 as brep_p3;
use crate::topology::brep::{p1 as bp, p2 as bp2};
use crate::topology::edge::NotifyEnv;
use crate::topology::topology::Topology;
use crate::topology::{Brep, BrepId, FaceId};
use std::collections::BTreeMap;

#[derive(Debug)]
pub(super) struct Session {
    context: ContextId,
    /// The constructed primary (`PiecewiseMerge`'s final result).
    destination: BrepId,
    primary: Option<BrepId>,
    other: Option<BrepId>,
    pub(super) intersector: KernelManifoldIntersector,
    state: ManifoldState,
    pub(super) temporaries: Vec<BrepId>,
}

impl Session {
    pub(super) fn new(
        store: &SweepStore,
        context_of: BrepId,
        destination: BrepId,
        source: BrepId,
        tolerance: f64,
        angle: f64,
    ) -> Result<Self> {
        if destination == source {
            return Err(invalid("aliased merge operands"));
        }
        let context = store.context_of(destination)?;
        let merge = MergeState::with_breps(
            store.kernel(context_of)?,
            (store.brep(destination)?, MergeOperand::Primary),
            Some((store.brep(source)?, MergeOperand::Other)),
            tolerance,
            angle,
            None,
        )?;
        let engine = MergeEngine::from_merge(&merge)?;
        Ok(Self {
            context,
            destination,
            primary: Some(destination),
            other: Some(source),
            intersector: engine.intersector,
            state: engine.state,
            temporaries: Vec::new(),
        })
    }

    pub(super) fn operands(&self) -> Result<(BrepId, BrepId)> {
        Ok((
            self.primary.ok_or_else(|| SolidError::null_pointer(OP))?,
            self.other.ok_or_else(|| SolidError::null_pointer(OP))?,
        ))
    }
}

pub(super) struct Driver<'a, 'caller> {
    identities: Identities<'caller>,
    session: &'a mut Session,
    bodies: &'a mut BodySlots<'caller>,
    records: &'a mut crate::topology::topology::NotifyRecords,
    attributes: &'a mut AttributeStore,
    cleanup_errors: &'a mut Vec<SolidError>,
    other_kernel: Option<(ContextId, &'a mut Kernel)>,
}

struct ActiveSession<'a> {
    slot: &'a mut Option<Session>,
    session: Option<Session>,
}
impl Drop for ActiveSession<'_> {
    fn drop(&mut self) {
        *self.slot = self.session.take();
    }
}

impl SweepStore<'_> {
    pub(super) fn run_merge<T>(
        &mut self,
        id: SweepMergeId,
        action: impl FnOnce(&mut Kernel, &mut Driver<'_, '_>) -> Result<T>,
    ) -> Result<T> {
        let slot = self
            .merges
            .get_mut(id.0 as usize)
            .ok_or_else(|| invalid("unknown or released merge"))?;
        let session = slot
            .take()
            .ok_or_else(|| invalid("unknown or released merge"))?;
        let mut active = ActiveSession {
            slot,
            session: Some(session),
        };
        let session = active
            .session
            .as_mut()
            .ok_or_else(|| invalid("missing active merge"))?;
        let result = (|| {
            let (primary, other) = session.operands()?;
            if entry(&self.bodies, primary)?.context != session.context {
                return Err(invalid("merge primary context changed"));
            }
            let other_context = entry(&self.bodies, other)?.context;
            let (kernel, other_kernel) =
                kernels_mut(&mut self.kernels, session.context, other_context)?;
            let mut driver = Driver {
                identities: self.identities,
                session,
                bodies: &mut self.bodies,
                records: &mut self.records,
                attributes: &mut self.attributes,
                cleanup_errors: &mut self.cleanup_errors,
                other_kernel: other_kernel.map(|k| (other_context, k)),
            };
            action(kernel, &mut driver)
        })();

        result
    }
}

pub(super) fn kernels_mut<'s>(
    kernels: &'s mut [KernelEntry<'_>],
    primary: ContextId,
    other: ContextId,
) -> Result<(&'s mut Kernel, Option<&'s mut Kernel>)> {
    let a = primary.0 as usize;
    let b = other.0 as usize;
    if a == b {
        return Ok((
            &mut kernels
                .get_mut(a)
                .ok_or_else(|| invalid("unknown context"))?
                .kernel,
            None,
        ));
    }
    let (left, right) = kernels
        .split_at_mut_checked(a.max(b))
        .ok_or_else(|| invalid("unknown context"))?;
    let (a, b) = if a < b {
        (left.get_mut(a), right.first_mut())
    } else {
        (right.first_mut(), left.get_mut(b))
    };
    Ok((
        &mut a.ok_or_else(|| invalid("unknown context"))?.kernel,
        Some(&mut b.ok_or_else(|| invalid("unknown context"))?.kernel),
    ))
}

impl Driver<'_, '_> {
    fn body(&self, id: BrepId) -> Result<&Brep> {
        Ok(entry(self.bodies, id)?.body.brep())
    }

    fn with_pair<T>(
        &mut self,
        k: &mut Kernel,
        action: impl FnOnce(
            &mut BooleanKernels<'_>,
            &mut Brep,
            &mut Brep,
            &mut ManifoldKernelAdapter<'_>,
            &mut ManifoldState,
            &mut AttributeStore,
        ) -> Result<T>,
    ) -> Result<T> {
        let (primary, other) = self.session.operands()?;
        let (other, primary) = body_pair_mut(self.bodies, other, primary)?;

        let secondary = if other.context == self.session.context {
            None
        } else {
            let (context, kernel) = self
                .other_kernel
                .as_mut()
                .ok_or_else(|| invalid("missing other context"))?;
            if *context != other.context {
                return Err(invalid("other context changed"));
            }
            Some(&mut **kernel)
        };
        let mut kernels = BooleanKernels {
            primary: k,
            other: secondary,
        };
        let ids = self.session.operands()?;
        let identities = self.identities;
        let primary_key = |e| identities.object(primary.namespace, e);
        let other_key = |e| identities.object(other.namespace, e);
        // Curves are resolved before any mutable geometry borrow. Newly created
        // edge identities are resolved by topology, while existing curves keep
        // their original owner-issued keys.
        let primary_curves = curve_keys(identities, ids.0, primary.body.brep());
        let other_curves = curve_keys(identities, ids.1, other.body.brep());
        let primary_curve = |e, kind| curve_key(&primary_curves, identities, ids.0, e, kind);
        let other_curve = |e, kind| curve_key(&other_curves, identities, ids.1, e, kind);
        let source_lists = |r| identities.source_lists(primary.namespace, r);
        self.session.intersector.with_caller_notifications(
            ManifoldNotificationKeys {
                primary: &primary_key,
                other: &other_key,
                primary_curve: &primary_curve,
                other_curve: &other_curve,
                source_lists: &source_lists,
                actual_curve_identities: identities.actual_curve_identities(),
            },
            ids,
            self.records,
            |intersector| {
                action(
                    &mut kernels,
                    primary.body.brep_mut(),
                    other.body.brep_mut(),
                    intersector,
                    &mut self.session.state,
                    self.attributes,
                )
            },
        )
    }

    pub(super) fn copy_faces(
        &mut self,
        k: &mut Kernel,
        source: BrepId,
        faces: &[FaceId],
        destination: BrepId,
        edge_mates: &mut BTreeEdgeMap,
    ) -> Result<()> {
        if self.session.operands()? != (destination, source) {
            return Err(invalid("copy operands do not match merge"));
        }
        let result = self.with_pair(k, |kernels, a, b, i, _, attributes| {
            i.copy_faces(kernels, a, b, faces, attributes)
        });
        edge_mates.extend(
            self.session
                .intersector
                .merge
                .intersector
                .edges
                .other_to_primary(),
        );
        result
    }

    pub(super) fn union(&mut self, k: &mut Kernel) -> Result<BrepId> {
        let (mut primary, mut other) = (self.session.primary, self.session.other);
        let mut manifold = self.manifold_flag();
        let mut lamina = self.session.intersector.intersect_lamina_edges();
        let result = self.manifold_boolean(
            k,
            &mut primary,
            &mut other,
            &mut manifold,
            &mut lamina,
            BooleanOperation::Union,
            None,
        );
        self.set_operands(primary, other);
        result?.ok_or_else(|| SolidError::null_pointer(OP))
    }

    fn surface_face(&self, surface: ObjectRef) -> Result<(BrepId, FaceId)> {
        for (offset, entry) in self.bodies.slots.iter().enumerate() {
            let Some(entry) = entry else {
                continue;
            };
            for (face, record) in entry.body.brep().faces().iter() {
                if record.surface.is_some()
                    && self
                        .identities
                        .object(entry.namespace, EntityRef::Face(face))
                        == surface
                {
                    let index = self
                        .bodies
                        .base
                        .checked_add(u32::try_from(offset).map_err(|_| invalid("body index"))?)
                        .ok_or_else(|| invalid("body index"))?;
                    return Ok((BrepId::from_index(index), face));
                }
            }
        }
        Err(invalid("not a surface owner"))
    }
}

// Restore each surviving operand to its original slot; deletion must invalidate
// the handle even when the Boolean returns an error after consuming an operand.
/// Returns a Boolean operand to its slot: an owned body as owned, a borrowed caller
/// body into the caller's own B-rep (still borrowed). A borrowed body the Boolean
/// deleted leaves its slot empty and the caller's B-rep empty, and is reported:
/// upstream would leave the caller holding a deleted B-rep.
fn restore_body<'caller>(
    bodies: &mut BodySlots<'caller>,
    id: BrepId,
    context: ContextId,
    namespace: u32,
    body: Option<Brep>,
    borrowed: Option<&'caller mut Brep>,
) -> Result<()> {
    let slot = bodies
        .get_mut(id.index() as usize)
        .ok_or_else(|| invalid("body slot lost"))?;
    let consumed_borrow = body.is_none() && borrowed.is_some();
    *slot = match (body, borrowed) {
        (Some(b), Some(original)) => {
            *original = b;
            Some(Body::Borrowed(original))
        }
        (Some(b), None) => Some(Body::Owned(Box::new(b))),
        (None, _) => None,
    }
    .map(|body| BodyEntry {
        context,
        namespace,
        body,
    });
    if consumed_borrow {
        return Err(invalid("a Boolean deleted a borrowed caller body"));
    }
    Ok(())
}

/// Moves a Boolean operand out of its slot entry: an owned body itself, or a
/// borrowed caller body's content together with the borrow it returns to
/// ([`restore_body`]). A retained HCR scene is never an operand: its slot is left
/// as it is.
fn take_operand<'caller>(
    slot: Option<&mut Option<BodyEntry<'caller>>>,
    what: &'static str,
) -> Result<(Brep, Option<&'caller mut Brep>)> {
    let entry = slot
        .and_then(|slot| slot.take_if(|e| !matches!(e.body, Body::Hcr(_))))
        .ok_or_else(|| invalid(what))?;
    match entry.body {
        Body::Owned(b) => Ok((*b, None)),
        Body::Borrowed(original) => Ok((core::mem::take(original), Some(original))),
        Body::Hcr(_) => Err(invalid(what)),
    }
}

fn curve_keys(
    identities: Identities<'_>,
    id: BrepId,
    body: &Brep,
) -> BTreeMap<crate::topology::EdgeId, ObjectRef> {
    body.edges()
        .iter()
        .filter_map(|(edge, record)| {
            record.curve.as_ref().and_then(|curve| {
                if let Some(provider) = identities.0 {
                    Some((edge, provider.curve(id, edge, curve)))
                } else {
                    curve.notify_key().map(|key| {
                        (
                            edge,
                            ObjectRef {
                                key,
                                type_code: curve.type_code(),
                            },
                        )
                    })
                }
            })
        })
        .collect()
}
fn curve_key(
    curves: &BTreeMap<crate::topology::EdgeId, ObjectRef>,
    identities: Identities<'_>,
    body: BrepId,
    edge: crate::topology::EdgeId,
    type_code: i64,
) -> Result<ObjectRef> {
    curves.get(&edge).copied().map_or_else(
        || {
            identities
                .0
                .ok_or_else(|| invalid("new split curve needs its caller identity"))?
                .new_curve(body, edge, type_code)
        },
        Ok,
    )
}

fn clear_deleted(
    records: &mut crate::topology::topology::NotifyRecords,
    attributes: &mut AttributeStore,
    keys: &[ObjectKey],
) -> Result<()> {
    for key in keys {
        attributes.clear_attributes(*key)?;
    }
    for key in keys {
        records.remove(key);
    }
    Ok(())
}

// The native Boolean may consume either operand, including on an error. Return
// surviving bodies to their original slots even if a caller callback unwinds.
struct BooleanBodies<'a, 'caller> {
    slots: &'a mut BodySlots<'caller>,
    primary: Option<Brep>,
    other: Option<Brep>,
    /// The caller B-reps that borrowed operands return to ([`restore_body`]).
    primary_borrow: Option<&'caller mut Brep>,
    other_borrow: Option<&'caller mut Brep>,
    primary_meta: (BrepId, ContextId, u32),
    other_meta: (BrepId, ContextId, u32),
    primary_keys: Vec<ObjectKey>,
    other_keys: Vec<ObjectKey>,
    records: &'a mut crate::topology::topology::NotifyRecords,
    attributes: &'a mut AttributeStore,
    cleanup_errors: &'a mut Vec<SolidError>,
}
impl Drop for BooleanBodies<'_, '_> {
    fn drop(&mut self) {
        for (body, borrowed, (id, context, namespace), keys) in [
            (
                &mut self.primary,
                self.primary_borrow.take(),
                self.primary_meta,
                &self.primary_keys,
            ),
            (
                &mut self.other,
                self.other_borrow.take(),
                self.other_meta,
                &self.other_keys,
            ),
        ] {
            if body.is_none() {
                if let Err(error) = clear_deleted(self.records, self.attributes, keys) {
                    self.cleanup_errors.push(error);
                }
            }
            if let Err(error) =
                restore_body(self.slots, id, context, namespace, body.take(), borrowed)
            {
                self.cleanup_errors.push(error);
            }
        }
    }
}

impl BooleanDriver for Driver<'_, '_> {
    type Operand = BrepId;
    fn same_operand(&self, a: &BrepId, b: &BrepId) -> bool {
        a == b
    }

    fn construct(
        &mut self,
        k: &mut Kernel,
        primary: &mut Option<BrepId>,
        other: &mut Option<BrepId>,
        tolerance: f64,
        angle: f64,
        options: Option<&MergeOptions>,
    ) -> Result<()> {
        let id = primary.ok_or_else(|| SolidError::null_pointer(OP))?;
        let source = other
            .map(|id| self.body(id).map(|body| (body, MergeOperand::Other)))
            .transpose()?;
        let merge = MergeState::with_breps(
            k,
            (self.body(id)?, MergeOperand::Primary),
            source,
            tolerance,
            angle,
            options,
        )?;
        let engine = MergeEngine::from_merge(&merge)?;
        self.session.intersector = engine.intersector;
        self.session.state = engine.state;
        self.set_operands(*primary, *other);
        Ok(())
    }

    fn manifold_boolean(
        &mut self,
        k: &mut Kernel,
        primary: &mut Option<BrepId>,
        other: &mut Option<BrepId>,
        manifold: &mut bool,
        lamina: &mut u64,
        operation: BooleanOperation,
        lock: Option<&mut MarkLock>,
    ) -> Result<Option<BrepId>> {
        let a = primary.ok_or_else(|| SolidError::null_pointer(OP))?;
        let b = other.ok_or_else(|| SolidError::null_pointer(OP))?;
        let (source, destination) = body_pair_mut(self.bodies, b, a)?;
        // `SmMerge::ManifoldBoolean` edits its operands in place: an owned body or a
        // caller-borrowed one (`DoSweep`'s merge into `pBrepToSweepInto`), never a
        // retained HCR scene.
        if matches!(destination.body, Body::Hcr(_)) || matches!(source.body, Body::Hcr(_)) {
            return Err(invalid(
                "a retained HCR scene cannot be consumed by a Boolean",
            ));
        }
        let (ac, an, ag) = (
            destination.context,
            destination.namespace,
            body_keys(self.identities, a, destination.body.brep(), self.records),
        );
        let (bc, bn, bg) = (
            source.context,
            source.namespace,
            body_keys(self.identities, b, source.body.brep(), self.records),
        );

        self.session.intersector.set_intersect_lamina_edges(*lamina);
        let secondary = if bc == self.session.context {
            None
        } else {
            let (context, kernel) = self
                .other_kernel
                .as_mut()
                .ok_or_else(|| invalid("missing other context"))?;
            if *context != bc {
                return Err(invalid("other context changed"));
            }
            Some(&mut **kernel)
        };
        let (ab, primary_borrow) =
            take_operand(self.bodies.get_mut(a.index() as usize), "unknown primary")?;
        let (bb, other_borrow) =
            take_operand(self.bodies.get_mut(b.index() as usize), "unknown other")?;
        let mut operands = BooleanBodies {
            slots: self.bodies,
            primary: Some(ab),
            other: Some(bb),
            primary_borrow,
            other_borrow,
            primary_meta: (a, ac, an),
            other_meta: (b, bc, bn),
            primary_keys: ag,
            other_keys: bg,
            records: self.records,
            attributes: self.attributes,
            cleanup_errors: self.cleanup_errors,
        };
        let mut kernels = BooleanKernels {
            primary: k,
            other: secondary,
        };
        let identities = self.identities;
        let primary_key = |e| identities.object(an, e);
        let other_key = |e| identities.object(bn, e);
        let primary_curves = operands
            .primary
            .as_ref()
            .map(|body| curve_keys(identities, a, body))
            .unwrap_or_default();
        let other_curves = operands
            .other
            .as_ref()
            .map(|body| curve_keys(identities, b, body))
            .unwrap_or_default();
        let primary_curve = |e, kind| curve_key(&primary_curves, identities, a, e, kind);
        let other_curve = |e, kind| curve_key(&other_curves, identities, b, e, kind);
        let source_lists = |r| identities.source_lists(an, r);
        let result = self.session.intersector.with_caller_notifications(
            ManifoldNotificationKeys {
                primary: &primary_key,
                other: &other_key,
                primary_curve: &primary_curve,
                other_curve: &other_curve,
                source_lists: &source_lists,
                actual_curve_identities: identities.actual_curve_identities(),
            },
            (a, b),
            &mut *operands.records,
            |intersector| {
                intersector.manifold_boolean(
                    &mut kernels,
                    &mut operands.primary,
                    &mut operands.other,
                    &mut self.session.state,
                    &mut *operands.attributes,
                    *manifold,
                    operation,
                    lock,
                )
            },
        );
        if operands.primary.is_none() {
            *primary = None;
        }
        if operands.other.is_none() {
            *other = None;
        }

        result.map(|assigned| if assigned { *primary } else { None })
    }
}

impl PiecewiseAccess for Driver<'_, '_> {
    type Surface = ObjectRef;
    fn operands(&self) -> (Option<BrepId>, Option<BrepId>) {
        (self.session.primary, self.session.other)
    }
    fn set_operands(&mut self, primary: Option<BrepId>, other: Option<BrepId>) {
        // Arena ids can repeat in another body; traversal caches cannot follow them.
        if self.session.primary != primary {
            self.session.state.primary_marks =
                crate::topology::topology_traverser::Marks::default();
        }
        if self.session.other != other {
            self.session.state.other_marks = crate::topology::topology_traverser::Marks::default();
        }
        self.session.primary = primary;
        self.session.other = other;
    }
    fn surfaces(&self, id: BrepId) -> Result<Vec<ObjectRef>> {
        let b = self.body(id)?;
        bp::faces(b, None)?
            .into_iter()
            .filter_map(|f| match b.face(f) {
                Ok(face) if face.surface.is_none() => None,
                _ => Some(KernelManifoldIntersector::surface_identity(b, f)),
            })
            .collect()
    }
    fn intersect_insert_relate(&mut self, k: &mut Kernel) -> Result<()> {
        self.with_pair(k, |kernels, a, b, i, _, attributes| {
            i.intersect_insert_relate(kernels, a, b, attributes)
        })
    }
    fn clear_relationships(&mut self) -> Result<()> {
        self.session.intersector.clear_relationships()
    }
    fn set_subset(&mut self, surfaces: &[ObjectRef]) {
        self.session.intersector.set_surface_subset(surfaces);
    }
    fn surface_map(&mut self, surface: ObjectRef, only_if_missing: bool) -> Result<()> {
        self.surface_face(surface)?;
        if !only_if_missing
            || self
                .attributes
                .find_attribute(surface.key, ids::SURFACEMAP)
                .is_none()
        {
            let h = self.attributes.insert(Attribute::pointer(
                ids::SURFACEMAP,
                0,
                Behavior::Reference,
            ))?;
            self.attributes.add_attribute(surface.key, h)?;
        }
        Ok(())
    }
    fn surface_map_users(&self, surface: ObjectRef) -> Result<Option<Vec<ObjectRef>>> {
        self.attributes
            .find_attribute(surface.key, ids::SURFACEMAP)
            .map(|h| {
                self.attributes.users(h).map(|users| {
                    users
                        .into_iter()
                        .map(|key| ObjectRef {
                            key,
                            type_code: surface.type_code,
                        })
                        .collect()
                })
            })
            .transpose()
    }
    fn new_face_brep(&mut self, k: &mut Kernel, source: BrepId) -> Result<BrepId> {
        let e = entry(self.bodies, source)?;
        if e.context != self.session.context {
            return Err(invalid("new face body context differs from merge"));
        }
        let tolerance = e.body.brep().tolerance();
        let index = u32::try_from(self.bodies.len()).map_err(|_| invalid("too many bodies"))?;
        let namespace = index
            .checked_add(1)
            .filter(|n| *n <= 0x03ff_ffff)
            .ok_or_else(|| invalid("body identity namespace exhausted"))?;
        let id = BrepId::from_index(index);
        let mut body = brep_p3::new_brep_for_caller(
            k,
            crate::geom::tol::USE_DEFAULT,
            crate::geom::tol::USE_DEFAULT,
            self.identities.object(namespace, EntityRef::Brep(id)).key,
        )?;
        bp2::set_tolerance(k, &mut body, tolerance, true, true)?;
        body.set_editing_enabled(true);
        self.records.insert(
            self.identities.object(namespace, EntityRef::Brep(id)).key,
            Topology::new(),
        );
        if let Some(region) = body.infinite_region() {
            self.records.insert(
                self.identities
                    .object(namespace, EntityRef::Region(region))
                    .key,
                Topology::new(),
            );
        }
        self.bodies.push(Some(BodyEntry {
            context: self.session.context,
            namespace,
            body: Body::Owned(Box::new(body)),
        }));
        self.session.temporaries.push(id);
        Ok(id)
    }
    fn manifold_flag(&self) -> bool {
        self.session
            .intersector
            .merge
            .manifold_boolean
            .unwrap_or(false)
    }
    fn set_controls(&mut self, manifold: bool, wires: bool, lamina: u64) {
        self.session.intersector.merge.manifold_boolean = Some(manifold);
        self.session.intersector.set_intersect_wire_edges(wires);
        self.session.intersector.set_intersect_lamina_edges(lamina);
    }
    fn process_wires(&mut self, k: &mut Kernel, lock: &MarkLock) -> Result<()> {
        self.with_pair(k, |kernels, a, b, i, state, attributes| {
            i.process_wires(
                kernels,
                a,
                b,
                state,
                attributes,
                BooleanOperation::Merge,
                lock.mark_type1(),
                lock.mark_type2(),
                &mut Vec::new(),
            )
        })
    }
    fn release_marks(&mut self, k: &mut Kernel, lock: &mut MarkLock) -> Result<()> {
        let secondary = self.other_kernel.as_mut().map(|(_, k)| &mut **k);
        BooleanKernels {
            primary: k,
            other: secondary,
        }
        .mark_action(|contexts| lock.release(contexts))
    }
    fn register_empty_ssi(&mut self, skip: ObjectRef, user: ObjectRef) -> Result<()> {
        // PiecewiseMerge registers the pair for its next Merge (the final result with
        // the new face B-rep) while the operands are still the PartialMerge's (the
        // new face B-rep with the original other). The users also name the surfaces
        // of other bodies (the original other's own); upstream registers those pairs
        // too, but no surface of the next Merge's operands ever matches them, so they
        // are skipped here (face ids are per body). See [`next_merge_pair`].
        // A user whose face no longer exists in any body (deleted by the
        // PartialMerge) is no operand surface either: upstream registers its
        // pointer, which never matches, so it is skipped too.
        let (Ok((a, af)), Ok((b, bf))) = (self.surface_face(skip), self.surface_face(user)) else {
            return Ok(());
        };
        if next_merge_pair(self.session.destination, self.session.primary, a, b) {
            self.session.intersector.register_empty_ssi(af, bf);
        }
        Ok(())
    }
    fn clear_existing_ssi(&mut self) -> Result<()> {
        self.session.intersector.clear_existing_ssi();
        Ok(())
    }
    fn notify_pre_edit(&mut self, k: &mut Kernel, primary: BrepId) -> Result<()> {
        let b = entry_mut(self.bodies, primary)?;
        let key_of = |e| self.identities.object(b.namespace, e);
        let mut env = NotifyEnv {
            records: self.records,
            attributes: self.attributes,
            context: Some(&mut k.context),
            key_of: &key_of,
        };
        bp2::notify(
            b.body.brep(),
            &mut env,
            None,
            NotifyOperation::PreEdit,
            [None, None, None],
        )
    }
    fn delete_other(&mut self, _k: &mut Kernel) -> Result<()> {
        if let Some(id) = self.session.other {
            let e = entry(self.bodies, id)?;
            if matches!(e.body, Body::Borrowed(_)) {
                return Err(invalid("cannot consume a borrowed original"));
            }
            let keys = body_keys(self.identities, id, e.body.brep(), self.records);
            clear_deleted(self.records, self.attributes, &keys)?;
            *self
                .bodies
                .get_mut(id.index() as usize)
                .ok_or_else(|| invalid("body slot lost"))? = None;
            self.session.other = None;
        }
        Ok(())
    }
}

/// Whether a `RegisterExistingSSI(pSkip2, pSurfUser)` pair of `SmMerge::PiecewiseMerge`
/// (`SmMerge.cpp:638`) can ever be found by the next Merge, whose operands are the
/// final result `destination` (`m_vTI.m_pBrep = pFinalResult`, cpp:647) and the new
/// face B-rep `primary` (`m_vTI.m_pOther = m_vTI.m_pBrep`, cpp:646), before the table
/// is cleared after it (`ClearExistingSSI`, cpp:682). Upstream keys the table by
/// surface pointer and `SmTopologyIntersector::CheckIfHaveExistingSSI`
/// (`SmTopologyIntersector.cpp:184-248`) matches a pair only when both pointers are
/// the queried operand surfaces, so a pair naming a surface of any other body is
/// registered but never matches. The port's table keys surfaces by per-body face id,
/// where such a pair could alias an operand face, so it is not registered instead.
fn next_merge_pair(
    destination: BrepId,
    primary: Option<BrepId>,
    skip_body: BrepId,
    user_body: BrepId,
) -> bool {
    skip_body == destination && primary == Some(user_body) && skip_body != user_body
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::curve::Curve;
    use crate::geom::attribute::TagData;
    use crate::geom::context::NotifyCallback;
    use crate::math::Vec3;
    use crate::sweep::topology_sweep::SweepEntity;
    use std::sync::{Arc, Mutex};

    type Events = Arc<Mutex<Vec<(NotifyOperation, ObjectRef, Option<ObjectRef>)>>>;
    #[derive(Debug)]
    struct Log(Events);
    impl NotifyCallback for Log {
        fn execute(
            &mut self,
            operation: NotifyOperation,
            caller: ObjectRef,
            first: Option<ObjectRef>,
            _: Option<ObjectRef>,
            _: Option<ObjectRef>,
        ) -> Result<()> {
            self.0.lock().unwrap().push((operation, caller, first));
            Ok(())
        }
    }

    fn models(shared: bool) -> (SweepStore<'static>, BrepId, BrepId, Events, Events) {
        let mut store = SweepStore::owned();
        let source_events = Arc::new(Mutex::new(Vec::new()));
        let dest_events = if shared {
            source_events.clone()
        } else {
            Arc::new(Mutex::new(Vec::new()))
        };
        let mut source_kernel = Kernel::default();
        source_kernel
            .context
            .set_user_notify_callback(Some(Box::new(Log(source_events.clone()))));
        let source_context = store.insert_kernel(source_kernel).unwrap();
        let dest_context = if shared {
            source_context
        } else {
            let mut k = Kernel::default();
            k.context
                .set_user_notify_callback(Some(Box::new(Log(dest_events.clone()))));
            store.insert_kernel(k).unwrap()
        };
        let source = store.create_brep(source_context).unwrap();
        let dest = store.create_brep(dest_context).unwrap();
        (store, source, dest, source_events, dest_events)
    }

    fn point(store: &mut SweepStore, body: BrepId, at: Vec3) -> crate::topology::VertexId {
        let region = store.infinite_region(body).unwrap();
        store.make_shell_vertex((body, region), at).unwrap().1 .1
    }

    #[test]
    fn copying_vertices_keeps_reference_attributes_and_notifies_the_source_context() {
        for shared in [false, true] {
            let (mut store, source, dest, source_events, dest_events) = models(shared);
            let vertex = point(&mut store, source, Vec3::new(1., 2., 3.));
            point(&mut store, dest, Vec3::new(9., 9., 9.));
            let source_key = store.object_ref(source, EntityRef::Vertex(vertex)).unwrap();
            let attribute = store
                .attributes_mut(source)
                .unwrap()
                .insert(Attribute::tag(
                    ids::TAG,
                    TagData {
                        primary_id: 19,
                        ..Default::default()
                    },
                    Behavior::Reference,
                ))
                .unwrap();
            store
                .attributes_mut(source)
                .unwrap()
                .add_attribute(source_key.key, attribute)
                .unwrap();
            source_events.lock().unwrap().clear();
            dest_events.lock().unwrap().clear();
            let copies = store.copy_vertices(source, &[vertex], dest).unwrap();
            assert_eq!(copies.len(), 1);
            let copy = copies[0];
            assert_eq!(
                store.brep(dest).unwrap().vertex(copy).unwrap().point,
                Vec3::new(1., 2., 3.)
            );
            let dest_key = store.object_ref(dest, EntityRef::Vertex(copy)).unwrap();
            assert_ne!(source_key, dest_key);
            assert_eq!(
                store
                    .attributes(dest)
                    .unwrap()
                    .find_attribute(dest_key.key, ids::TAG),
                Some(attribute)
            );
            assert_eq!(
                store.attributes(source).unwrap().users(attribute).unwrap(),
                vec![source_key.key, dest_key.key]
            );
            assert!(source_events.lock().unwrap().contains(&(
                NotifyOperation::Copy,
                source_key,
                Some(dest_key)
            )));
            if !shared {
                assert!(!dest_events
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|(op, _, _)| *op == NotifyOperation::Copy));
            }
            assert!(store
                .kernel(source)
                .unwrap()
                .context
                .user_notify_callback()
                .is_some());
            assert!(store
                .kernel(dest)
                .unwrap()
                .context
                .user_notify_callback()
                .is_some());
            store.remove_brep(source).unwrap();
            assert_eq!(
                store.attributes(dest).unwrap().users(attribute).unwrap(),
                vec![dest_key.key]
            );
        }
    }

    #[test]
    fn merging_a_body_copies_topology_without_consuming_the_source() {
        for shared in [false, true] {
            let (mut store, source, dest, source_events, _) = models(shared);
            let vertex = point(&mut store, source, Vec3::new(1., 2., 3.));
            store
                .add_tag(
                    SweepEntity::Vertex(source, vertex),
                    TagData {
                        primary_id: 31,
                        ..Default::default()
                    },
                )
                .unwrap();
            store.merge_brep(dest, source).unwrap();
            assert_eq!(store.brep(source).unwrap().vertices().len(), 1);
            let dest_vertices = bp::vertices(store.brep(dest).unwrap(), None).unwrap();
            assert_eq!(dest_vertices.len(), 1);
            assert_eq!(
                store
                    .tag(SweepEntity::Vertex(dest, dest_vertices[0]))
                    .unwrap()
                    .unwrap()
                    .primary_id,
                31
            );
            assert!(source_events
                .lock()
                .unwrap()
                .iter()
                .any(|(op, caller, _)| *op == NotifyOperation::Copy
                    && *caller == store.object_ref(source, EntityRef::Vertex(vertex)).unwrap()));
            assert!(store.contains_brep(source));
        }
    }

    #[test]
    fn merge_constructor_context_can_differ_from_the_primary_body_context() {
        let (mut store, source, dest, _, _) = models(false);
        let id = store.new_merge(source, dest, source, 0.1, 0.2).unwrap();
        assert_eq!(store.merge_operands(id).unwrap(), (dest, source));
        assert_eq!(
            store.merges[id.0 as usize].as_ref().unwrap().context,
            store.context_of(dest).unwrap()
        );
        store
            .copy_faces(source, &[], dest, id, &mut BTreeMap::new())
            .unwrap();
        store.delete_merge(id);
        assert!(store.contains_brep(source));
        assert!(store.contains_brep(dest));
        assert!(store.take_cleanup_errors().is_empty());
    }

    #[test]
    fn merge_lifetime_checks_aliases_and_released_handles() {
        let (mut store, source, dest, _, _) = models(false);
        assert!(store.new_merge(dest, dest, dest, 0.1, 0.2).is_err());
        let id = store.new_merge(dest, dest, source, 0.1, 0.2).unwrap();
        assert_eq!(store.merge_operands(id).unwrap(), (dest, source));
        assert!(store
            .copy_faces(dest, &[], source, id, &mut BTreeMap::new())
            .is_err());
        store
            .copy_faces(source, &[], dest, id, &mut BTreeMap::new())
            .unwrap();
        store.delete_merge(id);
        assert!(store.merge_operands(id).is_err());
        let next = store.new_merge(dest, dest, source, 0.1, 0.2).unwrap();
        assert_ne!(next, id);
        store.delete_brep(source);
        assert!(store.piecewise_merge(next, true, true).is_err());
        assert!(store.contains_brep(dest));
    }

    #[test]
    fn empty_piecewise_merge_keeps_both_operands_and_releases_marks() {
        for shared in [false, true] {
            let (mut store, source, dest, _, _) = models(shared);
            let result = store
                .with_merge(dest, dest, source, 0.1, 0.2, |store, id| {
                    store.piecewise_merge(id, true, true)
                })
                .unwrap();
            assert_eq!(result, dest);
            assert!(store.contains_brep(source));
            assert!(store.contains_brep(dest));
            assert!(store.merges.iter().all(Option::is_none));
        }
    }

    #[test]
    fn empty_union_runs_the_native_engine() {
        let (mut store, source, dest, _, _) = models(false);
        let id = store.new_merge(dest, dest, source, 0.1, 0.2).unwrap();
        assert_eq!(store.union(id).unwrap(), dest);
        assert!(store.contains_brep(dest));
        assert_eq!(store.brep(dest).unwrap().vertices().len(), 0);
        store.delete_merge(id);
        assert!(store.merges.iter().all(Option::is_none));
    }
    #[test]
    fn face_copy_keeps_the_canonical_edge_mates_and_constructs_native_topology() {
        use crate::curve::line::Line;
        use crate::geom::extent2d::Extent2d;
        use crate::surface::{plane::Plane, surface::Surface};
        let (mut store, source, dest, _, _) = models(false);
        let points = [Vec3::ZERO, Vec3::X, Vec3::new(1., 1., 0.), Vec3::Y];
        let region = store.infinite_region(source).unwrap();
        let vertices: Vec<_> = points
            .iter()
            .map(|p| point(&mut store, source, *p))
            .collect();
        let mut edgeuses = Vec::new();
        for i in 0..4 {
            let curve = Curve::from(
                Line::create_line_segment(3, points[i], points[(i + 1) % 4], None).unwrap(),
            );
            let interval = curve.natural_interval();
            let edge = store
                .make_wire_edge(
                    (source, region),
                    (source, vertices[i]),
                    (source, vertices[(i + 1) % 4]),
                    curve,
                    interval,
                    crate::nurbs::b_spline_surface_n_lib::p2::OrientType::Same,
                )
                .unwrap();
            edgeuses.push(
                crate::topology::edgeuse::p1::primary_edgeuse(store.brep(source).unwrap(), edge.1)
                    .unwrap()
                    .unwrap(),
            );
        }
        // Existing face input: assemble its arena links because MakeFaceTopology's
        // constructor tail is not ported. The copy must use the real MergeEdge port.
        let face = store
            .with_brep_mut(source, |_, b, face_keys, _| {
                use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
                use crate::topology::shell::{ShellMembers, ShellType};
                use crate::topology::{EdgeuseOwner, Faceuse, LoopEdge, Loopuse, LoopuseStart};
                let shell = crate::topology::edgeuse::p1::shell(b, edgeuses[0])?.unwrap();
                let face = b.add_face(shell)?;
                let mut surface = Surface::from(Plane::from_normal(Vec3::ZERO, Vec3::Z));
                surface.set_owner(Some(face_keys(face)));
                b.face_mut(face)?.surface = Some(surface);
                b.face_mut(face)?.uv_domain = Extent2d::from_bounds(0., 0., 1., 1.);
                let first = b.add_faceuse_record(Faceuse {
                    face: Some(face),
                    shell: Some(shell),
                    orientation: OrientType::Same,
                    ..Default::default()
                })?;
                let second = b.add_faceuse_record(Faceuse {
                    face: Some(face),
                    shell: Some(shell),
                    orientation: OrientType::Opposite,
                    mate: Some(first),
                    ..Default::default()
                })?;
                b.faceuse_mut(first)?.mate = Some(second);
                b.face_mut(face)?.faceuse = Some(first);
                let loop_edges = edgeuses
                    .iter()
                    .map(|eu| LoopEdge {
                        edge: b.edgeuse(*eu).unwrap().edge.unwrap(),
                        forward: true,
                    })
                    .collect();
                let loop_id = b.add_loop(face, loop_edges)?;
                let lu1 = b.add_loopuse_record(Loopuse {
                    faceuse: Some(first),
                    loop_: Some(loop_id),
                    orientation: OrientType::Same,
                    ..Default::default()
                })?;
                let lu2 = b.add_loopuse_record(Loopuse {
                    faceuse: Some(second),
                    loop_: Some(loop_id),
                    orientation: OrientType::Same,
                    mate: Some(lu1),
                    ..Default::default()
                })?;
                b.loopuse_mut(lu1)?.mate = Some(lu2);
                b.loop_mut(loop_id)?.loopuse = Some(lu1);
                b.faceuse_mut(first)?.loopuses.push(lu1);
                b.faceuse_mut(second)?.loopuses.push(lu2);
                let opposite: Vec<_> = edgeuses
                    .iter()
                    .rev()
                    .map(|eu| crate::topology::edgeuse::p1::mate(b, *eu).unwrap().unwrap())
                    .collect();
                for (lu, uses) in [(lu1, &edgeuses), (lu2, &opposite)] {
                    b.loopuse_mut(lu)?.start = LoopuseStart::Edgeuse(uses.first().copied());
                    for (i, eu) in uses.iter().enumerate() {
                        let record = b.edgeuse_mut(*eu)?;
                        record.owner = EdgeuseOwner::Loopuse(Some(lu));
                        record.ccw = Some(uses[(i + 1) % uses.len()]);
                        record.cw = Some(uses[(i + uses.len() - 1) % uses.len()]);
                    }
                }
                b.shell_mut(shell)?.shell_type = ShellType::Faceuse;
                b.shell_mut(shell)?.members = ShellMembers::Faceuses(vec![first, second]);
                Ok(face)
            })
            .unwrap();
        let merge = store.new_merge(dest, dest, source, 0.01, 0.1).unwrap();
        let mut mates = BTreeMap::new();
        let result = store.copy_faces(source, &[face], dest, merge, &mut mates);
        result.unwrap();
        assert_eq!(store.brep(dest).unwrap().faces().len(), 1);
        assert_eq!(store.brep(dest).unwrap().vertices().len(), 4);
        assert_eq!(mates.len(), 4);
        assert_eq!(store.brep(dest).unwrap().edges().len(), 4);
        for (from, to) in &mates {
            assert_eq!(
                store.brep(source).unwrap().edge(*from).unwrap().interval,
                store.brep(dest).unwrap().edge(*to).unwrap().interval
            );
        }
        assert!(store.contains_brep(source));
        store.delete_merge(merge);
    }
    #[test]
    fn scoped_merge_releases_native_temporary_bodies_on_error() {
        let (mut store, source, dest, _, _) = models(false);
        let mut temporary = None;
        let result: Result<()> = store.with_merge(dest, dest, source, 0.1, 0.2, |store, id| {
            store.run_merge(id, |k, driver| {
                temporary = Some(driver.new_face_brep(k, dest)?);
                Err(SolidError::sm_err("injected after native body allocation"))
            })
        });
        assert!(result.is_err());
        let temporary = temporary.unwrap();
        assert!(!store.contains_brep(temporary));
        assert!(store.contains_brep(source));
        assert!(store.contains_brep(dest));
        assert!(store.merges.iter().all(Option::is_none));
        assert!(store.take_cleanup_errors().is_empty());
        assert_ne!(store.new_brep(dest).unwrap(), temporary);
    }

    #[test]
    fn changing_operands_does_not_reuse_marks_from_another_body() {
        use crate::geom::context::ALL_MARKS;
        use crate::topology::topology_traverser::Entity;
        let (mut store, source, dest, _, _) = models(true);
        let original = point(&mut store, source, Vec3::ZERO);
        let replacement = store.new_brep(dest).unwrap();
        let replacement_vertex = point(&mut store, replacement, Vec3::X);
        assert_eq!(original, replacement_vertex, "arena indices can coincide");
        let mark = store.lock_mark(dest, ALL_MARKS).unwrap();
        store
            .with_merge(dest, dest, source, 0.1, 0.2, |store, id| {
                store.run_merge(id, |k, driver| {
                    driver
                        .session
                        .state
                        .other_marks
                        .mark(k, Entity::Vertex(original), mark);
                    assert!(driver.session.state.other_marks.is_marked(
                        k,
                        Entity::Vertex(original),
                        mark
                    ));
                    driver.set_operands(Some(dest), Some(replacement));
                    assert!(!driver.session.state.other_marks.is_marked(
                        k,
                        Entity::Vertex(replacement_vertex),
                        mark
                    ));
                    Ok(())
                })
            })
            .unwrap();
        store.unlock_mark(dest, mark).unwrap();
    }
    #[test]
    fn unwinding_a_merge_scope_releases_its_retained_temporary() {
        use std::panic::{catch_unwind, AssertUnwindSafe};
        let (mut store, source, dest, _, _) = models(false);
        let mut temporary = None;
        let panic = catch_unwind(AssertUnwindSafe(|| {
            let _: Result<()> = store.with_merge(dest, dest, source, 0.1, 0.2, |store, id| {
                store.run_merge(id, |k, driver| {
                    temporary = Some(driver.new_face_brep(k, dest)?);
                    panic!("injected caller panic after native allocation");
                })
            });
        }));
        assert!(panic.is_err());
        assert!(!store.contains_brep(temporary.unwrap()));
        assert!(store.contains_brep(source));
        assert!(store.contains_brep(dest));
        assert!(store.merges.iter().all(Option::is_none));
        assert!(store.take_cleanup_errors().is_empty());
    }
    #[derive(Debug)]
    struct PanicOnPreEdit;
    impl NotifyCallback for PanicOnPreEdit {
        fn execute(
            &mut self,
            operation: NotifyOperation,
            _: ObjectRef,
            _: Option<ObjectRef>,
            _: Option<ObjectRef>,
            _: Option<ObjectRef>,
        ) -> Result<()> {
            assert_ne!(
                operation,
                NotifyOperation::PreEdit,
                "injected caller callback panic"
            );
            Ok(())
        }
    }
    #[test]
    fn unwinding_a_boolean_returns_bodies_records_and_callbacks_to_the_store() {
        use std::panic::{catch_unwind, AssertUnwindSafe};
        let (mut store, source, dest, _, _) = models(false);
        let sv = point(&mut store, source, Vec3::new(10_000., 0., 0.));
        let dv = point(&mut store, dest, Vec3::new(10_001., 0., 0.));
        let merge = store.new_merge(dest, dest, source, 0.01, 0.1).unwrap();
        store
            .kernel_mut(dest)
            .unwrap()
            .context
            .set_user_notify_callback(Some(Box::new(PanicOnPreEdit)));
        let panic = catch_unwind(AssertUnwindSafe(|| {
            let _ = store.union(merge);
        }));
        assert!(panic.is_err());
        assert_eq!(
            store.brep(source).unwrap().vertex(sv).unwrap().point,
            Vec3::new(10_000., 0., 0.)
        );
        assert_eq!(
            store.brep(dest).unwrap().vertex(dv).unwrap().point,
            Vec3::new(10_001., 0., 0.)
        );
        assert!(store
            .kernel(dest)
            .unwrap()
            .context
            .user_notify_callback()
            .is_some());
        assert_eq!(store.merge_operands(merge).unwrap(), (dest, source));
        assert!(store.take_cleanup_errors().is_empty());
        store.delete_merge(merge);
    }

    #[test]
    fn only_pairs_of_the_next_merge_operands_are_registered() {
        let [dest, face_brep, other] = [0, 1, 2].map(BrepId::from_index);
        // (final-result surface, new face B-rep surface): the next Merge can find it
        assert!(next_merge_pair(dest, Some(face_brep), dest, face_brep));
        // a user naming the original other's surface, a reversed pair, a pair on one
        // body, or no primary: upstream registers these but never matches them
        assert!(!next_merge_pair(dest, Some(face_brep), dest, other));
        assert!(!next_merge_pair(dest, Some(face_brep), other, face_brep));
        assert!(!next_merge_pair(dest, Some(face_brep), face_brep, dest));
        assert!(!next_merge_pair(dest, Some(dest), dest, dest));
        assert!(!next_merge_pair(dest, None, dest, face_brep));
    }

    #[test]
    fn registering_a_pair_outside_the_operands_succeeds_without_an_entry() {
        use crate::boolean::merge::p3::PiecewiseAccess;
        use crate::geom::extent2d::Extent2d;
        use crate::surface::plane::Plane;
        use crate::topology::{Face, ShellId};
        let (mut store, source, dest, _, _) = models(true);
        let third = store.new_brep(dest).unwrap();
        let face_of = |store: &mut SweepStore, body| {
            let face = store
                .brep_mut(body)
                .unwrap()
                .add_face_record(Face {
                    surface: Some(Plane::from_normal(Vec3::ZERO, Vec3::Z).into()),
                    uv_domain: Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0),
                    ..Face::new(ShellId::from_index(0))
                })
                .unwrap();
            store.object_ref(body, EntityRef::Face(face)).unwrap()
        };
        let in_dest = face_of(&mut store, dest);
        let in_third = face_of(&mut store, third);
        store
            .with_merge(dest, dest, source, 0.1, 0.2, |store, id| {
                store.run_merge(id, |_, driver| {
                    // the session's primary is the destination itself: no pair of the
                    // next merge: both calls succeed (upstream registers them too)
                    driver.register_empty_ssi(in_dest, in_third)?;
                    driver.register_empty_ssi(in_third, in_dest)?;
                    Ok(())
                })
            })
            .unwrap();
    }
}
