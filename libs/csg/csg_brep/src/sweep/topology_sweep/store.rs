//! Borrowed and owned topology access for sweeps, primitive creation and the offset
//! executive's merge.
//!
//! Upstream: `source/SMLib/src/SmTopologySweep.cpp`,
//! `source/SMLib/src/SmPrimitiveCreation.cpp` (caller-owned modeling services),
//! `source/SMLib/src/SmOffsetExecutive.cpp` (`PiecewiseMerge`'s `SmMerge`).
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`SweepStore`] | B-rep/context ownership used by the modeling callers |
//! | [`SweepStore::piecewise_merge_skipping`] | `SmMerge::PiecewiseMerge` with its skip relation |
//!
//! Deviations: checked, never-reused handles replace pointers. Contexts are shared
//! by id, not copied. Notification identities reuse the Boolean adapter's encoding.
//! Face traversal and notifications use the same persistent Topology records.
//! HCR entries own their actual scene; no scene copy is kept. Native dependency
//! errors and partial geometry are preserved. No numerical algorithm or recursion
//! is introduced. Destruction releases the Rust-owned registry/attribute state;
//! the unported native B-rep destructor is not emulated. `RemoveFaces`,
//! `RemoveAllTopology` and the sweep stitch edit through the context the store has
//! already lent to the edit (the trait defaults lend it again and fail).
//! A body's own attributes ([`Brep::add_attribute`]) move into the shared store
//! when it is inserted or adopted, and a taken body gets a copy of its live objects'
//! lists back.

pub(crate) mod classification;
mod merge;

use std::ops::{Deref, DerefMut};
use std::sync::{atomic::AtomicU64, Arc};

use super::access::{EdgeRef, FaceRef, SweepAccess, SweepMergeId};
use super::SweepEntity;
use crate::blend::hcr::{Hcr, HcrOptions};
use crate::boolean::merge::p4::{notification_key, KernelManifoldIntersector};
use crate::curve::curve::Curve;
use crate::error::{EntityRef, Result, SolidError};
use crate::geom::attribute::{
    ids, Attribute, AttributeStore, AttributeValue, Behavior, ObjectKey, TagData,
};
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::context::{Context, MarkLock, MarkType};
use crate::geom::object::ObjectRef;
use crate::geom::tol::Relation;
use crate::kernel::Kernel;
use crate::primitive::primitive_creation::p3::PrimitiveAccess;
use crate::topology::brep::p3 as brep_p3;
use crate::topology::edge::NotifyEnv;
use crate::topology::topology::Topology;
use crate::topology::topology_traverser::{self as traverser, Entity, FaceTraversalMarks};
use crate::topology::{Brep, BrepId, EdgeId, FaceId};
use std::collections::BTreeMap;

const OP: &str = "sweep::topology_sweep::store";

/// A kernel context owned by one [`SweepStore`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ContextId(u32);

#[derive(Debug)]
struct KernelEntry<'a> {
    kernel: Storage<'a, Kernel>,
    locks: Vec<MarkLock>,
}

#[derive(Debug)]
enum Body<'a> {
    Borrowed(&'a mut Brep),
    Owned(Box<Brep>),
    Hcr(Box<Hcr>),
}

impl Body<'_> {
    fn brep(&self) -> &Brep {
        match self {
            Self::Owned(b) => b,
            Self::Borrowed(b) => b,
            Self::Hcr(h) => h.scene_brep(),
        }
    }
    fn brep_mut(&mut self) -> &mut Brep {
        match self {
            Self::Owned(b) => b,
            Self::Borrowed(b) => b,
            Self::Hcr(h) => h.scene_brep_mut(),
        }
    }
}

#[derive(Debug)]
struct BodyEntry<'a> {
    context: ContextId,
    namespace: u32,
    body: Body<'a>,
}

/// Storage supplied by the caller or owned by this adapter. Both modes execute
/// the same access and merge implementations.
#[derive(Debug)]
enum Storage<'a, T> {
    Owned(Box<T>),
    Borrowed(&'a mut T),
}
impl<T> Deref for Storage<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        match self {
            Self::Owned(v) => v,
            Self::Borrowed(v) => v,
        }
    }
}
impl<T> DerefMut for Storage<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        match self {
            Self::Owned(v) => v,
            Self::Borrowed(v) => v,
        }
    }
}

/// Caller identity provider. It must assign stable distinct identities across
/// B-reps and cover future entities. Curve identities must refer to the original
/// geometry object rather than its owning edge.
pub trait SweepIdentities {
    /// Resolve a topology identity in its B-rep namespace.
    fn topology(&self, brep: BrepId, entity: EntityRef) -> ObjectRef;
    /// Enumerate exclusively owned auxiliary keys for temporary deletion. Include
    /// geometry and removed topology retained by the model; exclude shared/borrowed originals.
    /// The caller owns the geometry registry and must apply its actual ownership.
    fn owned_auxiliary_objects(&self, brep: BrepId, model: &Brep) -> Vec<ObjectKey>;
    /// Resolve the original curve object of an edge.
    fn curve(&self, brep: BrepId, edge: EdgeId, curve: &Curve) -> ObjectRef;
    /// Resolve a newly allocated split curve before it is attached to its edge.
    /// Existing borrowed callers may refuse this extra merge continuation.
    fn new_curve(&self, _brep: BrepId, _edge: EdgeId, _type_code: i64) -> Result<ObjectRef> {
        Err(invalid("new split curve needs its caller identity"))
    }
    /// Whether a curve the merge allocates at an edge split takes its actual Curve
    /// identity from the edit's notification context when it is constructed
    /// (`p9::CurveIdentity::Actual`), instead of [`Self::new_curve`].
    fn actual_curve_identities(&self) -> bool {
        false
    }
    /// Reserve two distinct non-object handles for each primary region's merge
    /// source lists. They must not collide with topology, geometry or other lists.
    fn source_lists(
        &self,
        _brep: BrepId,
        _region: crate::topology::RegionId,
    ) -> Result<[ObjectRef; 2]> {
        Err(invalid("merge source lists need caller-reserved handles"))
    }
}

#[derive(Clone, Copy)]
struct Identities<'a>(Option<&'a dyn SweepIdentities>);
impl core::fmt::Debug for Identities<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Identities")
            .field("caller_supplied", &self.0.is_some())
            .finish()
    }
}
impl Identities<'_> {
    fn actual_curve_identities(self) -> bool {
        self.0.is_some_and(SweepIdentities::actual_curve_identities)
    }

    fn source_lists(
        self,
        namespace: u32,
        region: crate::topology::RegionId,
    ) -> Result<[ObjectRef; 2]> {
        if let Some(provider) = self.0 {
            provider.source_lists(BrepId::from_index(namespace - 1), region)
        } else {
            Ok([13_u64, 14].map(|kind| ObjectRef {
                type_code: 0,
                key: ObjectKey(
                    (u64::from(namespace) << 37) | (kind << 32) | u64::from(region.index()),
                ),
            }))
        }
    }

    fn object(self, namespace: u32, entity: EntityRef) -> ObjectRef {
        if let Some(provider) = self.0 {
            provider.topology(BrepId::from_index(namespace - 1), entity)
        } else {
            object(namespace, entity)
        }
    }
}

#[derive(Debug)]
struct BodySlots<'a> {
    base: u32,
    next: Storage<'a, u32>,
    slots: Vec<Option<BodyEntry<'a>>>,
}
impl<'a> BodySlots<'a> {
    fn get(&self, index: usize) -> Option<&Option<BodyEntry<'a>>> {
        self.slots.get(index.checked_sub(self.base as usize)?)
    }
    fn get_mut(&mut self, index: usize) -> Option<&mut Option<BodyEntry<'a>>> {
        self.slots.get_mut(index.checked_sub(self.base as usize)?)
    }
    fn len(&self) -> usize {
        *self.next as usize
    }
    fn push(&mut self, body: Option<BodyEntry<'a>>) {
        // Every insertion reserves a successor before constructing its body.
        *self.next += 1;
        self.slots.push(body);
    }
}

/// Production [`SweepAccess`] and [`PrimitiveAccess`] over one registry.
///
/// [`Self::new`] borrows caller resources and identities;
/// [`Self::owned`] owns the same resources for standalone modeling callers.
/// Borrowed originals are never deleted or re-keyed. Both modes retain merge
/// sessions, canonical mate maps and monotonic handles in this single store.
#[derive(Debug)]
pub struct SweepStore<'a> {
    kernels: Vec<KernelEntry<'a>>,
    bodies: BodySlots<'a>,
    identities: Identities<'a>,
    records: Storage<'a, crate::topology::topology::NotifyRecords>,
    attributes: Storage<'a, AttributeStore>,
    geometry_sequence: Arc<AtomicU64>,
    cleanup_errors: Vec<SolidError>,
    merges: Vec<Option<merge::Session>>,
}

fn invalid(reason: &'static str) -> SolidError {
    SolidError::invalid_input(OP, reason)
}

fn entry<'s, 'a>(bodies: &'s BodySlots<'a>, id: BrepId) -> Result<&'s BodyEntry<'a>> {
    bodies
        .get(id.index() as usize)
        .and_then(Option::as_ref)
        .ok_or_else(|| invalid("unknown or released B-rep"))
}
fn entry_mut<'s, 'a>(bodies: &'s mut BodySlots<'a>, id: BrepId) -> Result<&'s mut BodyEntry<'a>> {
    bodies
        .get_mut(id.index() as usize)
        .and_then(Option::as_mut)
        .ok_or_else(|| invalid("unknown or released B-rep"))
}
fn body_pair_mut<'s, 'a>(
    bodies: &'s mut BodySlots<'a>,
    source: BrepId,
    destination: BrepId,
) -> Result<(&'s mut BodyEntry<'a>, &'s mut BodyEntry<'a>)> {
    if source == destination {
        return Err(invalid("aliased source and destination"));
    }
    entry(bodies, source)?;
    entry(bodies, destination)?;
    let si = source
        .index()
        .checked_sub(bodies.base)
        .ok_or_else(|| invalid("unknown source"))? as usize;
    let di = destination
        .index()
        .checked_sub(bodies.base)
        .ok_or_else(|| invalid("unknown destination"))? as usize;
    let (left, right) = bodies.slots.split_at_mut(si.max(di));
    let (src, dst) = if si < di {
        (left.get_mut(si), right.first_mut())
    } else {
        (right.first_mut(), left.get_mut(di))
    };
    Ok((
        src.and_then(Option::as_mut)
            .ok_or_else(|| invalid("unknown source"))?,
        dst.and_then(Option::as_mut)
            .ok_or_else(|| invalid("unknown destination"))?,
    ))
}

fn object(namespace: u32, entity: EntityRef) -> ObjectRef {
    // The intersector addresses its single B-rep as operand zero. Its namespace
    // supplies the global body identity, including for the B-rep's own key.
    let entity = if matches!(entity, EntityRef::Brep(_)) {
        EntityRef::Brep(BrepId::from_index(0))
    } else {
        entity
    };
    notification_key(namespace, false, entity)
}

fn entity(value: SweepEntity) -> (BrepId, EntityRef) {
    match value {
        SweepEntity::Vertex(b, v) => (b, EntityRef::Vertex(v)),
        SweepEntity::Edge(b, e) => (b, EntityRef::Edge(e)),
        SweepEntity::Face(b, f) => (b, EntityRef::Face(f)),
        SweepEntity::Region(b, r) => (b, EntityRef::Region(r)),
    }
}

/// Every live topology object of `b` that can carry attributes, in arena order.
fn live_entities(b: &Brep) -> Vec<EntityRef> {
    let mut out: Vec<EntityRef> = Vec::new();
    out.extend(b.regions().ids().map(EntityRef::Region));
    out.extend(b.shells().ids().map(EntityRef::Shell));
    out.extend(b.faces().ids().map(EntityRef::Face));
    out.extend(b.faceuses().ids().map(EntityRef::Faceuse));
    out.extend(b.loops().ids().map(EntityRef::Loop));
    out.extend(b.loopuses().ids().map(EntityRef::Loopuse));
    out.extend(b.edges().ids().map(EntityRef::Edge));
    out.extend(b.edgeuses().ids().map(EntityRef::Edgeuse));
    out.extend(b.vertices().ids().map(EntityRef::Vertex));
    out.extend(b.vertexuses().ids().map(EntityRef::Vertexuse));
    out
}

fn validate(b: &Brep, value: EntityRef) -> Result<()> {
    match value {
        EntityRef::Brep(_) => {}
        EntityRef::Region(i) => {
            b.region(i)?;
        }
        EntityRef::Shell(i) => {
            b.shell(i)?;
        }
        EntityRef::Face(i) => {
            b.face(i)?;
        }
        EntityRef::Loop(i) => {
            b.loop_(i)?;
        }
        EntityRef::Edge(i) => {
            b.edge(i)?;
        }
        EntityRef::Vertex(i) => {
            b.vertex(i)?;
        }
        EntityRef::Faceuse(i) => {
            b.faceuse(i)?;
        }
        EntityRef::Loopuse(i) => {
            b.loopuse(i)?;
        }
        EntityRef::Edgeuse(i) => {
            b.edgeuse(i)?;
        }
        EntityRef::Vertexuse(i) => {
            b.vertexuse(i)?;
        }
    }
    Ok(())
}

impl<'a> SweepStore<'a> {
    /// Bind the caller's persistent identity, notification and attribute stores.
    /// Keep `next_handle` across sessions; registrations never reuse a handle.
    pub fn new(
        identities: &'a dyn SweepIdentities,
        records: &'a mut crate::topology::topology::NotifyRecords,
        attributes: &'a mut AttributeStore,
        next_handle: &'a mut u32,
    ) -> Self {
        Self {
            kernels: Vec::new(),
            bodies: BodySlots {
                base: *next_handle,
                next: Storage::Borrowed(next_handle),
                slots: Vec::new(),
            },
            identities: Identities(Some(identities)),
            records: Storage::Borrowed(records),
            attributes: Storage::Borrowed(attributes),
            geometry_sequence: Arc::new(AtomicU64::new(0)),
            cleanup_errors: Vec::new(),
            merges: Vec::new(),
        }
    }

    /// Create a standalone store owning its kernels, bodies, records and attributes.
    pub fn owned() -> Self {
        Self {
            kernels: Vec::new(),
            bodies: BodySlots {
                base: 0,
                next: Storage::Owned(Box::new(0)),
                slots: Vec::new(),
            },
            identities: Identities(None),
            records: Storage::Owned(Box::default()),
            attributes: Storage::Owned(Box::default()),
            geometry_sequence: Arc::new(AtomicU64::new(0)),
            cleanup_errors: Vec::new(),
            merges: Vec::new(),
        }
    }

    /// Register one actual kernel and share allocation of future geometry keys.
    /// Reuse its returned context for B-reps sharing the same kernel.
    ///
    /// # Errors
    /// A lent context or exhausted context handles; no registration is made.
    pub fn register_kernel(&mut self, kernel: &'a mut Kernel) -> Result<ContextId> {
        let index = u32::try_from(self.kernels.len()).map_err(|_| invalid("too many contexts"))?;
        kernel
            .context
            .share_geometry_sequence(Arc::clone(&self.geometry_sequence))?;
        self.kernels.push(KernelEntry {
            kernel: Storage::Borrowed(kernel),
            locks: Vec::new(),
        });
        Ok(ContextId(index))
    }

    /// Borrow an original body without modifying its geometry or existing key.
    ///
    /// # Errors
    /// An unknown context, exhausted handle, or inconsistent caller identity.
    pub fn register_brep(&mut self, context: ContextId, brep: &'a mut Brep) -> Result<BrepId> {
        self.kernels
            .get(context.0 as usize)
            .ok_or_else(|| invalid("unknown context"))?;
        let index = u32::try_from(self.bodies.len()).map_err(|_| invalid("too many bodies"))?;
        let namespace = index
            .checked_add(1)
            .ok_or_else(|| invalid("body identity exhausted"))?;
        let id = BrepId::from_index(index);
        if brep.key() != self.identities.object(namespace, EntityRef::Brep(id)).key {
            return Err(invalid(
                "borrowed B-rep identity disagrees with caller resolver",
            ));
        }
        self.bodies.push(Some(BodyEntry {
            context,
            namespace,
            body: Body::Borrowed(brep),
        }));
        Ok(id)
    }

    /// Move a caller kernel into the store, preserving its caches, callbacks and RNG.
    pub fn insert_kernel(&mut self, mut kernel: Kernel) -> Result<ContextId> {
        let id = u32::try_from(self.kernels.len()).map_err(|_| invalid("too many contexts"))?;
        kernel
            .context
            .share_geometry_sequence(Arc::clone(&self.geometry_sequence))?;
        self.kernels.push(KernelEntry {
            kernel: Storage::Owned(Box::new(kernel)),
            locks: Vec::new(),
        });
        let context = ContextId(id);
        Ok(context)
    }

    /// The context assigned to a live body.
    pub fn context_of(&self, id: BrepId) -> Result<ContextId> {
        Ok(entry(&self.bodies, id)?.context)
    }

    /// Construct an empty B-rep with its native infinite-region record.
    pub fn create_brep(&mut self, context: ContextId) -> Result<BrepId> {
        let index = u32::try_from(self.bodies.len()).map_err(|_| invalid("too many bodies"))?;
        let namespace = index
            .checked_add(1)
            .filter(|n| *n <= 0x03ff_ffff)
            .ok_or_else(|| invalid("body identity namespace exhausted"))?;
        let key = self
            .identities
            .object(namespace, EntityRef::Brep(BrepId::from_index(index)))
            .key;
        let kernel = &mut self
            .kernels
            .get_mut(context.0 as usize)
            .ok_or_else(|| invalid("unknown context"))?
            .kernel;
        let body = brep_p3::new_brep_for_caller(
            kernel,
            crate::geom::tol::USE_DEFAULT,
            crate::geom::tol::USE_DEFAULT,
            key,
        )?;
        self.insert_body(context, Body::Owned(Box::new(body)))
    }

    /// Adopt an unregistered body. Existing external identities are rejected before
    /// mutation; owned curves/surfaces receive their actual enclosing topology owner.
    /// The body's own attributes ([`Brep::add_attribute`], `SmAObject` attributes)
    /// join the shared store under its identities, where the store's edits carry them.
    pub fn insert_brep(&mut self, context: ContextId, brep: Brep) -> Result<BrepId> {
        if brep.key() != ObjectKey(0)
            || brep.edges().iter().any(|(_, e)| {
                e.curve
                    .as_ref()
                    .is_some_and(|c| c.owner().is_some() || c.notify_key().is_some())
            })
            || brep
                .faces()
                .iter()
                .any(|(_, f)| f.surface.as_ref().is_some_and(|s| s.owner().is_some()))
        {
            return Err(invalid("body already has external object identities"));
        }
        let mut brep = brep;
        let own = brep.take_attributes();
        let id = self.insert_body(context, Body::Owned(Box::new(brep)))?;
        self.seed_attributes(id, own)?;
        Ok(id)
    }

    /// (not upstream) Moves a body's own attributes (`SmAObject` attributes, keyed by
    /// [`crate::topology::loopuse::notify_ref`]) into the shared store under the
    /// body's identities, where the store's edits carry them.
    fn seed_attributes(&mut self, id: BrepId, own: AttributeStore) -> Result<()> {
        let namespace = entry(&self.bodies, id)?.namespace;
        let identities = self.identities;
        self.attributes.absorb(own, &|key| {
            crate::topology::loopuse::entity_of_notify_key(key)
                .map(|e| identities.object(namespace, e).key)
                .ok_or(invalid("attribute list names no object"))
        })
    }

    /// Move in a caller body whose objects already carry the caller's identities for
    /// the handle it receives (its key and records, as the caller notified them
    /// before). Unlike [`Self::insert_brep`] nothing is re-keyed: the store edits the
    /// body under those same identities, and a Boolean may consume it.
    /// The body's own attributes ([`Brep::add_attribute`]) join the shared store under
    /// those identities, as [`Self::insert_brep`]'s do.
    ///
    /// # Errors
    /// An unknown context, exhausted handles, no caller identities, or a B-rep key
    /// that disagrees with the caller resolver; no registration is made.
    pub fn adopt_brep(&mut self, context: ContextId, brep: Brep) -> Result<BrepId> {
        self.kernels
            .get(context.0 as usize)
            .ok_or_else(|| invalid("unknown context"))?;
        if self.identities.0.is_none() {
            return Err(invalid("adopting a body needs caller identities"));
        }
        let index = u32::try_from(self.bodies.len()).map_err(|_| invalid("too many bodies"))?;
        let namespace = index
            .checked_add(1)
            .filter(|n| *n <= 0x03ff_ffff)
            .ok_or_else(|| invalid("body identity namespace exhausted"))?;
        let id = BrepId::from_index(index);
        if brep.key() != self.identities.object(namespace, EntityRef::Brep(id)).key {
            return Err(invalid(
                "adopted B-rep identity disagrees with caller resolver",
            ));
        }
        let mut brep = brep;
        let own = brep.take_attributes();
        self.bodies.push(Some(BodyEntry {
            context,
            namespace,
            body: Body::Owned(Box::new(brep)),
        }));
        self.seed_attributes(id, own)?;
        Ok(id)
    }

    /// Move an owned body out to the caller, with its records and attributes left in
    /// the caller's stores under its identities (the counterpart of
    /// [`Self::adopt_brep`]). The id is not reused. The body also takes a copy of the lists of its
    /// live objects as its own attributes ([`Brep::entity_attributes`]), `SM_AI_TAG`
    /// tags of a tagging sweep among them, as upstream's objects keep theirs.
    ///
    /// # Errors
    /// An unknown or released id, or a borrowed or retained HCR body.
    pub fn take_brep(&mut self, id: BrepId) -> Result<Brep> {
        if !matches!(entry(&self.bodies, id)?.body, Body::Owned(_)) {
            return Err(invalid("only an owned body can be taken"));
        }
        // (not upstream) the body's objects take their attribute lists with them, as
        // `SmAObject` attributes travel with their objects; the shared store keeps its
        // own (the caller's stores, under the body's identities)
        let own = self.body_attributes(id)?;
        let slot = self
            .bodies
            .get_mut(id.index() as usize)
            .ok_or_else(|| invalid("unknown body"))?;
        match slot.take().map(|e| e.body) {
            Some(Body::Owned(mut b)) => {
                b.set_attributes(own);
                Ok(*b)
            }
            _ => Err(invalid("only an owned body can be taken")),
        }
    }

    /// (not upstream) A copy of the shared store's lists for the live objects of body
    /// `id`, keyed by [`crate::topology::loopuse::notify_ref`] (the body's own
    /// attributes once it leaves the store).
    fn body_attributes(&self, id: BrepId) -> Result<AttributeStore> {
        let e = entry(&self.bodies, id)?;
        let names: BTreeMap<ObjectKey, ObjectKey> = live_entities(e.body.brep())
            .into_iter()
            .map(|x| {
                (
                    self.identities.object(e.namespace, x).key,
                    crate::topology::loopuse::notify_ref(x).key,
                )
            })
            .collect();
        let mut own = (*self.attributes).clone();
        own.retain_rekeyed(&|key| names.get(&key).copied())?;
        Ok(own)
    }

    /// `SmMerge::PiecewiseMerge(pOptSkipPairs, bUseExistingTopology,
    /// bCreateNewBrepsForFaces, rpResult)` with the surface pairs whose SSI is skipped
    /// (`SmTArrayRelation` of surfaces, as the driver's surface identities; `None` is
    /// a `NULL` relation, as [`SweepAccess::piecewise_merge`] passes).
    ///
    /// # Errors
    /// Those of [`crate::boolean::merge::p3::piecewise_merge`]; `SM_ERR_NULL_POINTER`
    /// for a null result.
    pub fn piecewise_merge_skipping(
        &mut self,
        id: SweepMergeId,
        skip_pairs: Option<&Relation<ObjectRef, ObjectRef>>,
        use_existing_topology: bool,
        create_new_breps_for_faces: bool,
    ) -> Result<BrepId> {
        self.run_merge(id, |k, driver| {
            crate::boolean::merge::p3::piecewise_merge(
                k,
                driver,
                skip_pairs,
                use_existing_topology,
                create_new_breps_for_faces,
            )?
            .ok_or_else(|| SolidError::null_pointer(OP))
        })
    }

    /// [`Self::piecewise_merge_skipping`] after its leading `IntersectInsertRelate`
    /// ([`crate::boolean::merge::p3::piecewise_merge_related`]): the caller has
    /// already related the operands on its own intersector.
    ///
    /// # Errors
    /// As [`Self::piecewise_merge_skipping`].
    pub fn piecewise_merge_related_skipping(
        &mut self,
        id: SweepMergeId,
        skip_pairs: Option<&Relation<ObjectRef, ObjectRef>>,
        use_existing_topology: bool,
        create_new_breps_for_faces: bool,
    ) -> Result<BrepId> {
        self.run_merge(id, |k, driver| {
            crate::boolean::merge::p3::piecewise_merge_related(
                k,
                driver,
                skip_pairs,
                use_existing_topology,
                create_new_breps_for_faces,
            )?
            .ok_or_else(|| SolidError::null_pointer(OP))
        })
    }

    /// The `m_vTI` of a live merge session, for a caller that configures it as its
    /// own `SmMerge` base does (`SmFilletExecutive` sets `m_bIgnoreTangency` and
    /// `m_bDoingFillets`).
    ///
    /// # Errors
    /// An unknown or released merge.
    pub fn merge_intersector_mut(
        &mut self,
        id: SweepMergeId,
    ) -> Result<&mut KernelManifoldIntersector> {
        Ok(&mut self
            .merges
            .get_mut(id.0 as usize)
            .and_then(Option::as_mut)
            .ok_or_else(|| invalid("unknown or released merge"))?
            .intersector)
    }

    fn insert_body(&mut self, context: ContextId, mut body: Body<'a>) -> Result<BrepId> {
        self.kernels
            .get(context.0 as usize)
            .ok_or_else(|| invalid("unknown context"))?;
        let index = u32::try_from(self.bodies.len()).map_err(|_| invalid("too many bodies"))?;
        let namespace = index
            .checked_add(1)
            .filter(|n| *n <= 0x03ff_ffff)
            .ok_or_else(|| invalid("body identity namespace exhausted"))?;
        let id = BrepId::from_index(index);
        let b = body.brep_mut();
        let bk = self.identities.object(namespace, EntityRef::Brep(id));
        b.set_key(bk.key);
        let edges: Vec<_> = b.edges().ids().collect();
        for e in edges {
            let owner = self.identities.object(namespace, EntityRef::Edge(e));
            let has_curve = if let Some(c) = &mut b.edge_mut(e)?.curve {
                let _ = c.set_owner(Some(owner), Some(bk));
                let kernel = &mut self
                    .kernels
                    .get_mut(context.0 as usize)
                    .ok_or_else(|| invalid("unknown context"))?
                    .kernel;
                brep_p3::edge_curve_identity(&mut kernel.context, b, e)?;
                true
            } else {
                false
            };
            if has_curve {
                b.set_geometry_owner_key(EntityRef::Edge(e), Some(owner));
            }
        }
        let faces: Vec<_> = b.faces().ids().collect();
        for f in faces {
            let owner = self.identities.object(namespace, EntityRef::Face(f));
            let has_surface = if let Some(s) = &mut b.face_mut(f)?.surface {
                s.set_owner(Some(owner));
                true
            } else {
                false
            };
            if has_surface {
                b.set_geometry_owner_key(EntityRef::Face(f), Some(owner));
            }
            // the owned surface carries its face's tolerance, as wherever a face gets its
            // surface (`SmFace::GetTolerance` through `SmSurface::GetFace`)
            b.sync_face_surface_tolerance(f)?;
        }
        let entities = std::iter::once(EntityRef::Brep(id))
            .chain(b.regions().ids().map(EntityRef::Region))
            .chain(b.shells().ids().map(EntityRef::Shell))
            .chain(b.faces().ids().map(EntityRef::Face))
            .chain(b.loops().ids().map(EntityRef::Loop))
            .chain(b.edges().ids().map(EntityRef::Edge))
            .chain(b.vertices().ids().map(EntityRef::Vertex))
            .chain(b.faceuses().ids().map(EntityRef::Faceuse))
            .chain(b.loopuses().ids().map(EntityRef::Loopuse))
            .chain(b.edgeuses().ids().map(EntityRef::Edgeuse))
            .chain(b.vertexuses().ids().map(EntityRef::Vertexuse));
        for e in entities {
            self.records
                .insert(self.identities.object(namespace, e).key, Topology::new());
        }
        self.bodies.push(Some(BodyEntry {
            context,
            namespace,
            body,
        }));
        Ok(id)
    }

    /// Resolve a validated topology identity in the body's persistent namespace.
    pub fn object_ref(&self, id: BrepId, value: EntityRef) -> Result<ObjectRef> {
        let e = entry(&self.bodies, id)?;
        if let EntityRef::Brep(b) = value {
            if b != id {
                return Err(invalid("foreign B-rep identity"));
            }
        }
        validate(e.body.brep(), value)?;
        Ok(self.identities.object(e.namespace, value))
    }

    /// Attributes shared by the store's bodies. Topology keys and newly issued
    /// geometry keys remain unique across all owned contexts.
    pub fn attributes(&self, body: BrepId) -> Result<&AttributeStore> {
        self.context_of(body)?;
        Ok(&self.attributes)
    }

    /// Mutable attributes, retaining reference-attribute sharing between contexts.
    pub fn attributes_mut(&mut self, body: BrepId) -> Result<&mut AttributeStore> {
        self.context_of(body)?;
        Ok(&mut self.attributes)
    }

    /// The curve object's actual native notification identity, issued by its
    /// owning context. The returned key addresses the store's shared attributes.
    pub fn curve_object(&self, edge: EdgeRef) -> Result<ObjectRef> {
        let curve = self
            .brep(edge.0)?
            .edge(edge.1)?
            .curve
            .as_ref()
            .ok_or_else(|| SolidError::null_pointer(OP))?;
        if let Some(provider) = self.identities.0 {
            return Ok(provider.curve(edge.0, edge.1, curve));
        }
        Ok(ObjectRef {
            key: curve
                .notify_key()
                .ok_or_else(|| invalid("curve has no issued notification identity"))?,
            type_code: curve.type_code(),
        })
    }

    /// Whether a body handle still resolves, including retained HCR scene handles.
    pub fn contains_brep(&self, id: BrepId) -> bool {
        entry(&self.bodies, id).is_ok()
    }

    /// Release an owned registry entry and its attributes. The id is not reused.
    /// A failed attribute cleanup leaves the body registered and reports the error.
    pub fn remove_brep(&mut self, id: BrepId) -> Result<()> {
        let e = entry(&self.bodies, id)?;
        if matches!(e.body, Body::Borrowed(_)) {
            return Ok(());
        }
        let keys = body_keys(self.identities, id, e.body.brep(), &self.records);
        for key in &keys {
            self.attributes.clear_attributes(*key)?;
        }
        for key in keys {
            self.records.remove(&key);
        }
        let slot = self
            .bodies
            .get_mut(id.index() as usize)
            .ok_or_else(|| invalid("unknown body"))?;
        *slot = None;
        Ok(())
    }

    /// Retrieve errors from the trait's infallible cleanup hook. No error is silently
    /// converted to successful deletion; failed entries remain live.
    pub fn take_cleanup_errors(&mut self) -> Vec<SolidError> {
        std::mem::take(&mut self.cleanup_errors)
    }

    fn tag_at(&self, key: ObjectKey) -> Option<TagData> {
        let attributes = &self.attributes;
        attributes
            .find_attribute(key, ids::TAG)
            .and_then(|h| attributes.get(h))
            .and_then(|a| match a.value() {
                AttributeValue::Tag(t) => Some(*t),
                _ => None,
            })
    }
}

fn body_keys(
    identities: Identities<'_>,
    id: BrepId,
    body: &Brep,
    records: &crate::topology::topology::NotifyRecords,
) -> Vec<ObjectKey> {
    let mut keys = if let Some(provider) = identities.0 {
        provider.owned_auxiliary_objects(id, body)
    } else {
        records
            .keys()
            .copied()
            .filter(|key| key.0 >> 37 == u64::from(id.index()) + 1)
            .collect()
    };
    let namespace = id.index() + 1;
    keys.push(identities.object(namespace, EntityRef::Brep(id)).key);
    macro_rules! collect_keys {
        ($arena:ident, $kind:ident) => {
            keys.extend(
                body.$arena()
                    .ids()
                    .map(|i| identities.object(namespace, EntityRef::$kind(i)).key),
            );
        };
    }
    collect_keys!(regions, Region);
    collect_keys!(shells, Shell);
    collect_keys!(faces, Face);
    collect_keys!(loops, Loop);
    collect_keys!(edges, Edge);
    collect_keys!(vertices, Vertex);
    collect_keys!(faceuses, Faceuse);
    collect_keys!(loopuses, Loopuse);
    collect_keys!(edgeuses, Edgeuse);
    collect_keys!(vertexuses, Vertexuse);
    keys.extend(
        body.edges()
            .iter()
            .filter_map(|(_, e)| e.curve.as_ref().and_then(Curve::notify_key)),
    );
    keys.sort_unstable();
    keys.dedup();
    keys
}

struct RecordMarks<'a> {
    records: &'a mut crate::topology::topology::NotifyRecords,
    namespace: u32,
    identities: Identities<'a>,
}

fn traversal_entity(e: Entity) -> EntityRef {
    match e {
        Entity::Face(i) => EntityRef::Face(i),
        Entity::Edge(i) => EntityRef::Edge(i),
        Entity::Vertex(i) => EntityRef::Vertex(i),
        Entity::Shell(i) => EntityRef::Shell(i),
        Entity::Faceuse(i) => EntityRef::Faceuse(i),
        Entity::Vertexuse(i) => EntityRef::Vertexuse(i),
    }
}

impl FaceTraversalMarks for RecordMarks<'_> {
    fn is_marked(&self, k: &Kernel, e: Entity, mark: MarkType) -> bool {
        self.records
            .get(
                &self
                    .identities
                    .object(self.namespace, traversal_entity(e))
                    .key,
            )
            .is_some_and(|t| t.is_marked(&k.context, mark))
    }
    fn mark(&mut self, k: &Kernel, e: Entity, mark: MarkType) {
        self.records
            .or_default(
                self.identities
                    .object(self.namespace, traversal_entity(e))
                    .key,
            )
            .mark(Some(&k.context), mark);
    }
}

impl SweepAccess for SweepStore<'_> {
    fn brep(&self, id: BrepId) -> Result<&Brep> {
        Ok(entry(&self.bodies, id)?.body.brep())
    }
    fn brep_mut(&mut self, id: BrepId) -> Result<&mut Brep> {
        Ok(entry_mut(&mut self.bodies, id)?.body.brep_mut())
    }
    fn kernel(&self, id: BrepId) -> Result<&Kernel> {
        let context = self.context_of(id)?;
        self.kernels
            .get(context.0 as usize)
            .map(|e| &*e.kernel)
            .ok_or_else(|| invalid("unknown context"))
    }
    fn kernel_mut(&mut self, id: BrepId) -> Result<&mut Kernel> {
        let context = self.context_of(id)?;
        self.kernels
            .get_mut(context.0 as usize)
            .map(|e| &mut *e.kernel)
            .ok_or_else(|| invalid("unknown context"))
    }
    fn with_brep_mut<T>(
        &mut self,
        id: BrepId,
        action: impl FnOnce(
            &mut Kernel,
            &mut Brep,
            &dyn Fn(FaceId) -> ObjectRef,
            &mut NotifyEnv<'_>,
        ) -> Result<T>,
    ) -> Result<T> {
        let e = entry_mut(&mut self.bodies, id)?;
        let k = &mut self
            .kernels
            .get_mut(e.context.0 as usize)
            .ok_or_else(|| invalid("unknown context"))?
            .kernel;
        let ns = e.namespace;
        let identities = self.identities;
        let face_objects = |f| identities.object(ns, EntityRef::Face(f));
        let key_of = |v| identities.object(ns, v);
        let attributes = &mut self.attributes;
        Context::with_notification_context(k, |k, context| {
            let mut env = NotifyEnv {
                records: &mut self.records,
                attributes,
                context: Some(context),
                key_of: &key_of,
            };
            action(k, e.body.brep_mut(), &face_objects, &mut env)
        })?
    }
    fn with_breps_mut<T>(
        &mut self,
        source: BrepId,
        destination: BrepId,
        action: impl FnOnce(
            &mut Kernel,
            &mut Brep,
            &mut Brep,
            &dyn Fn(EntityRef) -> ObjectRef,
            &dyn Fn(FaceId) -> ObjectRef,
            Option<&mut Context>,
            &mut NotifyEnv<'_>,
        ) -> Result<T>,
    ) -> Result<T> {
        let (src, dst) = body_pair_mut(&mut self.bodies, source, destination)?;
        let (k, source_kernel) = merge::kernels_mut(&mut self.kernels, dst.context, src.context)?;
        let identities = self.identities;
        let source_key = |e| identities.object(src.namespace, e);
        let ns = dst.namespace;
        let face_objects = |f| identities.object(ns, EntityRef::Face(f));
        let key_of = |v| identities.object(ns, v);
        let attributes = &mut self.attributes;
        Context::with_notification_context(k, |k, context| {
            let mut env = NotifyEnv {
                records: &mut self.records,
                attributes,
                context: Some(context),
                key_of: &key_of,
            };
            action(
                k,
                src.body.brep_mut(),
                dst.body.brep_mut(),
                &source_key,
                &face_objects,
                source_kernel.map(|k| &mut k.context),
                &mut env,
            )
        })?
    }
    fn with_brep_notifications<T>(
        &mut self,
        id: BrepId,
        action: impl FnOnce(&mut Brep, &mut NotifyEnv<'_>) -> Result<T>,
    ) -> Result<T> {
        let e = entry_mut(&mut self.bodies, id)?;
        let k = &mut self
            .kernels
            .get_mut(e.context.0 as usize)
            .ok_or_else(|| invalid("unknown context"))?
            .kernel;
        let identities = self.identities;
        let key_of = |v| identities.object(e.namespace, v);
        let attributes = &mut self.attributes;
        let mut env = NotifyEnv {
            records: &mut self.records,
            attributes,
            context: Some(&mut k.context),
            key_of: &key_of,
        };
        action(e.body.brep_mut(), &mut env)
    }
    fn retain_hcr(&mut self, context_of: BrepId, hcr: Hcr) -> Result<BrepId> {
        let context = self.context_of(context_of)?;
        // Register the session's actual owned scene. No second B-rep is cloned
        // or detached from the session's silhouette/edge-id tables.
        self.insert_body(context, Body::Hcr(Box::new(hcr)))
    }
    fn hcr(
        &mut self,
        original: BrepId,
        inverse: Axis2Placement,
        tolerance: f64,
        angle: f64,
        seams: bool,
        smooth: bool,
        silhouettes: bool,
    ) -> Result<super::access::SweepScene> {
        let source_namespace = entry(&self.bodies, original)?.namespace;
        let scene = self.new_brep(original)?;
        let identities = self.identities;
        let source_key = |e| identities.object(source_namespace, e);
        let result = (|| {
            let mut hcr = self.with_breps_mut(original, scene, |k, source, _, _, _, _, env| {
                Hcr::with_transform(
                    k,
                    env,
                    source,
                    &source_key,
                    &inverse,
                    HcrOptions {
                        approx_tol_3d: tolerance,
                        ang_tol_rad: angle,
                        display_seams: seams,
                        display_smooth_edges: smooth,
                    },
                )
            })?;
            hcr.set_hcr_for_silhouettes(silhouettes);
            entry_mut(&mut self.bodies, scene)?.body = Body::Hcr(Box::new(hcr));
            let e = entry_mut(&mut self.bodies, scene)?;
            let k = &mut self
                .kernels
                .get_mut(e.context.0 as usize)
                .ok_or_else(|| invalid("unknown context"))?
                .kernel;
            if let Body::Hcr(hcr) = &mut e.body {
                crate::error::ignore_status(hcr.compute_global_visibility(k))?;
            }
            Ok(super::access::SweepScene { brep: scene })
        })();
        if result.is_err() {
            self.delete_brep(scene);
        }
        result
    }
    fn hcr_session(&self, scene: BrepId) -> Result<&Hcr> {
        match &entry(&self.bodies, scene)?.body {
            Body::Hcr(h) => Ok(h),
            Body::Owned(_) | Body::Borrowed(_) => Err(invalid("body is not an HCR session")),
        }
    }
    fn new_brep(&mut self, context_of: BrepId) -> Result<BrepId> {
        self.create_brep(self.context_of(context_of)?)
    }
    fn delete_brep(&mut self, id: BrepId) {
        if self.contains_brep(id) {
            if let Err(e) = self.remove_brep(id) {
                self.cleanup_errors.push(e);
            }
        }
    }
    fn tag(&self, value: SweepEntity) -> Result<Option<TagData>> {
        let (b, e) = entity(value);
        Ok(self.tag_at(self.object_ref(b, e)?.key))
    }
    fn add_tag(&mut self, value: SweepEntity, tag: TagData) -> Result<()> {
        let (b, e) = entity(value);
        let key = self.object_ref(b, e)?.key;
        let attributes = self.attributes_mut(b)?;
        let h = attributes.insert(Attribute::tag(ids::TAG, tag, Behavior::Copy))?;
        attributes.add_attribute(key, h)
    }
    fn curve_tag(&self, edge: EdgeRef) -> Result<Option<TagData>> {
        Ok(self.tag_at(self.curve_object(edge)?.key))
    }
    fn set_editing(&mut self, id: BrepId, editing: bool) -> Result<bool> {
        let b = self.brep_mut(id)?;
        let old = b.editing_enabled();
        b.set_editing_enabled(editing);
        Ok(old)
    }
    fn delete_merge(&mut self, id: SweepMergeId) {
        if let Some(session) = self.merges.get_mut(id.0 as usize).and_then(Option::take) {
            for body in session.temporaries {
                if self.contains_brep(body) {
                    if let Err(error) = self.remove_brep(body) {
                        self.cleanup_errors.push(error);
                    }
                }
            }
        }
    }
    fn new_merge(
        &mut self,
        context_of: BrepId,
        destination: BrepId,
        source: BrepId,
        tolerance: f64,
        angle: f64,
    ) -> Result<SweepMergeId> {
        let session = merge::Session::new(self, context_of, destination, source, tolerance, angle)?;
        let id =
            SweepMergeId(u32::try_from(self.merges.len()).map_err(|_| invalid("too many merges"))?);
        self.merges.push(Some(session));
        Ok(id)
    }
    fn copy_faces(
        &mut self,
        source: BrepId,
        faces: &[FaceId],
        destination: BrepId,
        merge: SweepMergeId,
        edge_mates: &mut super::access::BTreeEdgeMap,
    ) -> Result<()> {
        self.run_merge(merge, |k, driver| {
            driver.copy_faces(k, source, faces, destination, edge_mates)
        })
    }
    fn merge_operands(&self, id: SweepMergeId) -> Result<(BrepId, BrepId)> {
        self.merges
            .get(id.0 as usize)
            .and_then(Option::as_ref)
            .ok_or_else(|| invalid("unknown or released merge"))?
            .operands()
    }
    fn merge_relations(
        &mut self,
        id: SweepMergeId,
    ) -> Result<&mut crate::boolean::topology_intersector::p1::Relations> {
        Ok(&mut self
            .merges
            .get_mut(id.0 as usize)
            .and_then(Option::as_mut)
            .ok_or_else(|| invalid("unknown or released merge"))?
            .intersector
            .merge
            .intersector)
    }
    fn piecewise_merge(
        &mut self,
        id: SweepMergeId,
        use_existing_topology: bool,
        create_new_breps_for_faces: bool,
    ) -> Result<BrepId> {
        self.piecewise_merge_skipping(id, None, use_existing_topology, create_new_breps_for_faces)
    }
    fn union(&mut self, id: SweepMergeId) -> Result<BrepId> {
        self.run_merge(id, |k, driver| driver.union(k))
    }
    fn lock_mark(&mut self, id: BrepId, mask: i32) -> Result<MarkType> {
        let c = self.context_of(id)?;
        let e = self
            .kernels
            .get_mut(c.0 as usize)
            .ok_or_else(|| invalid("unknown context"))?;
        if ![
            MarkType::Mark,
            MarkType::Mark2,
            MarkType::Mark3,
            MarkType::MarkIo,
            MarkType::MarkAssert,
        ]
        .into_iter()
        .any(|m| mask & m.code() != 0 && !e.kernel.context.is_mark_locked(m))
        {
            return Err(invalid("no permitted unlocked mark"));
        }
        let lock = MarkLock::new(std::slice::from_mut(&mut e.kernel.context), 0, mask)?;
        let mark = lock.mark_type();
        e.locks.push(lock);
        Ok(mark)
    }
    fn unlock_mark(&mut self, id: BrepId, mark: MarkType) -> Result<()> {
        let c = self.context_of(id)?;
        let e = self
            .kernels
            .get_mut(c.0 as usize)
            .ok_or_else(|| invalid("unknown context"))?;
        let index = e
            .locks
            .iter()
            .rposition(|l| l.mark_type() == mark)
            .ok_or_else(|| invalid("mark lock not owned by context"))?;
        let mut lock = e.locks.remove(index);
        lock.release(std::slice::from_mut(&mut e.kernel.context))
    }
    fn is_marked(&self, value: SweepEntity, mark: MarkType) -> Result<bool> {
        let (b, e) = entity(value);
        let key = self.object_ref(b, e)?.key;
        let context = &self.kernel(b)?.context;
        Ok(self
            .records
            .get(&key)
            .is_some_and(|t| t.is_marked(context, mark)))
    }
    fn mark(&mut self, value: SweepEntity, mark: MarkType) -> Result<()> {
        let (b, e) = entity(value);
        let key = self.object_ref(b, e)?.key;
        let c = self.context_of(b)?;
        let k = &self
            .kernels
            .get(c.0 as usize)
            .ok_or_else(|| invalid("unknown context"))?
            .kernel;
        self.records.or_default(key).mark(Some(&k.context), mark);
        Ok(())
    }
    fn collect_faces(
        &mut self,
        face: FaceRef,
        mark: MarkType,
        faces: &mut Vec<FaceId>,
    ) -> Result<()> {
        let e = entry(&self.bodies, face.0)?;
        let k = &self
            .kernels
            .get(e.context.0 as usize)
            .ok_or_else(|| invalid("unknown context"))?
            .kernel;
        traverser::collect_faces(
            k,
            e.body.brep(),
            &mut RecordMarks {
                records: &mut self.records,
                namespace: e.namespace,
                identities: self.identities,
            },
            Some(face.1),
            mark,
            faces,
            None,
            None,
        )
    }
    /// `SmBrep::RemoveFaces`, notifying the store's lent context. The default's
    /// kernel-callback variant would lend that context a second time.
    fn remove_faces(&mut self, brep: BrepId, faces: &[FaceId]) -> Result<()> {
        self.with_brep_mut(brep, |kernel, brep, _, env| {
            let editing = brep.editing_enabled();
            crate::topology::brep::p5::remove_faces_notified(kernel, brep, editing, faces, env)
        })
    }
    /// `SmBrep::RemoveAllTopology`, notifying the store's lent context (as
    /// [`Self::remove_faces`]).
    fn remove_all_topology(&mut self, brep: BrepId) -> Result<()> {
        self.with_brep_mut(brep, |kernel, brep, _, env| {
            crate::topology::brep::p10::remove_all_topology_notified(kernel, brep, env)
        })
    }
    /// `SmStitch(SmSweepStitchCallback, tol, TRUE, TRUE).DoStitching()`, editing
    /// through the store's lent context (as [`Self::remove_faces`]). Split curves
    /// take the caller's identities.
    fn stitch(
        &mut self,
        brep: BrepId,
        sweep: &mut super::TopologySweep,
        tolerance: f64,
        making_manifold_solid: bool,
    ) -> Result<()> {
        let identities = self.identities;
        let new_curve = |edge: EdgeId, type_code: i64| {
            identities
                .0
                .ok_or_else(|| invalid("new split curve needs its caller identity"))?
                .new_curve(brep, edge, type_code)
        };
        self.with_brep_mut(brep, |kernel, model, _, env| {
            let curve_identity = if identities.actual_curve_identities() {
                crate::topology::brep::p9::CurveIdentity::Actual(
                    &crate::topology::brep::p9::issue_curve_identity,
                )
            } else {
                crate::topology::brep::p9::CurveIdentity::Checked(&new_curve)
            };
            let mut edits = crate::heal::stitch::p1::BorrowedStitchEdits {
                env,
                curve_identity,
            };
            let mut callback = super::access::SweepStitchCallback { brep, sweep };
            let mut stitch =
                crate::heal::stitch::p1::Stitch::new(&mut callback, tolerance, true, true);
            stitch.making_manifold_solid = making_manifold_solid;
            stitch.edits = Some(&mut edits);
            stitch
                .do_stitching(
                    kernel,
                    model,
                    None,
                    None,
                    crate::heal::stitch::p2::MinGapRequest::default(),
                )
                .map(|_| ())
        })
    }
}

impl PrimitiveAccess for SweepStore<'_> {
    fn with_primitive_brep<T>(
        &mut self,
        b: BrepId,
        action: impl FnOnce(&mut Kernel, &mut Brep) -> Result<T>,
    ) -> Result<T> {
        // This callback receives no NotifyEnv. Keep callbacks on the live Kernel
        // rather than moving them into an environment the caller cannot access.
        let e = entry_mut(&mut self.bodies, b)?;
        let k = &mut self
            .kernels
            .get_mut(e.context.0 as usize)
            .ok_or_else(|| invalid("unknown context"))?
            .kernel;
        action(k, e.body.brep_mut())
    }
    fn delete_primitive_faces(&mut self, id: BrepId, faces: &[FaceId]) -> Result<()> {
        let e = entry_mut(&mut self.bodies, id)?;
        let k = &mut self
            .kernels
            .get_mut(e.context.0 as usize)
            .ok_or_else(|| invalid("unknown context"))?
            .kernel;
        let identities = self.identities;
        let key_of = |entity| identities.object(e.namespace, entity);
        KernelManifoldIntersector::delete_faces_with_keys(
            k,
            e.body.brep_mut(),
            faces,
            &mut self.records,
            &mut self.attributes,
            &key_of,
        )
    }
}

impl Drop for SweepStore<'_> {
    fn drop(&mut self) {
        for entry in &mut self.kernels {
            for mut lock in entry.locks.drain(..) {
                let _ = lock.release(std::slice::from_mut(&mut entry.kernel.context));
            }
        }
        for index in self.bodies.base as usize..self.bodies.len() {
            if let Ok(index) = u32::try_from(index) {
                if let Err(error) = self.remove_brep(BrepId::from_index(index)) {
                    if self.contains_brep(BrepId::from_index(index)) {
                        self.cleanup_errors.push(error);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::axis2_placement::Axis2Placement;
    use crate::geom::context::{NotifyCallback, ALL_MARKS};
    use crate::geom::extent3d::Extent3d;
    use crate::geom::object::NotifyOperation;
    use crate::math::Vec3;
    use crate::sweep::topology_sweep::{
        engine::SweepTargets, geometry::SweepGeometry, TopologySweep, TopologySweepOptions,
    };
    use crate::sweep::trans_sweep_geometry::TranslationalSweepGeometry;
    use crate::topology::{ShellId, VertexId, VertexuseOwner};
    use std::sync::{Arc, Mutex};

    type Notices = Arc<Mutex<Vec<(NotifyOperation, ObjectRef)>>>;

    #[derive(Debug)]
    struct Log(Notices);
    impl NotifyCallback for Log {
        fn execute(
            &mut self,
            op: NotifyOperation,
            caller: ObjectRef,
            _: Option<ObjectRef>,
            _: Option<ObjectRef>,
            _: Option<ObjectRef>,
        ) -> Result<()> {
            self.0.lock().unwrap().push((op, caller));
            Ok(())
        }
    }

    fn point(store: &mut SweepStore, b: BrepId, at: Vec3) -> VertexId {
        let r = store.brep(b).unwrap().infinite_region().unwrap();
        store.make_shell_vertex((b, r), at).unwrap().1 .1
    }

    fn translation() -> SweepGeometry {
        SweepGeometry::Translation(TranslationalSweepGeometry::new(Vec3::new(10.0, -4.0, 6.0)))
    }

    #[test]
    fn transform_uses_the_existing_notification_lease_and_real_curve_owner() {
        use crate::curve::line::Line;
        use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
        let mut store = SweepStore::owned();
        let context = store.insert_kernel(Kernel::default()).unwrap();
        let body = store.create_brep(context).unwrap();
        let region = store.infinite_region(body).unwrap();
        let start = store
            .make_shell_vertex((body, region), Vec3::ZERO)
            .unwrap()
            .1
             .1;
        let curve = Curve::from(Line::from_points(Vec3::ZERO, Vec3::X, 3));
        let interval = curve.natural_interval();
        let (edge, end) = store
            .make_wire_edge_vertex(
                (body, region),
                (body, start),
                curve,
                interval,
                OrientType::Same,
                Vec3::X,
            )
            .unwrap();
        let mut placement = Axis2Placement::default();
        placement.translate(Vec3::new(2.0, 3.0, 4.0));
        store.transform(body, placement).unwrap();
        let model = store.brep(body).unwrap();
        assert_eq!(model.vertex(start).unwrap().point, Vec3::new(2.0, 3.0, 4.0));
        assert_eq!(model.vertex(end.1).unwrap().point, Vec3::new(3.0, 3.0, 4.0));
        let curve = model.edge(edge.1).unwrap().curve.as_ref().unwrap();
        assert_eq!(
            curve.owner(),
            Some(store.object_ref(body, EntityRef::Edge(edge.1)).unwrap())
        );
        assert_eq!(
            curve
                .evaluate_point(store.kernel(body).unwrap(), interval.min())
                .unwrap(),
            Vec3::new(2.0, 3.0, 4.0)
        );
        assert_eq!(
            curve
                .evaluate_point(store.kernel(body).unwrap(), interval.max())
                .unwrap(),
            Vec3::new(3.0, 3.0, 4.0)
        );
    }

    #[test]
    fn real_vertex_sweep_creates_native_shell_vertex_and_notifications() {
        // Repeat with shared/distinct kernels and every initial Boolean-flag pair.
        for shared in [false, true] {
            for source_flag in [false, true] {
                for dest_flag in [false, true] {
                    if shared && source_flag != dest_flag {
                        continue;
                    }
                    let mut store = SweepStore::owned();
                    let notices = Arc::new(Mutex::new(Vec::new()));
                    let mut source_kernel = Kernel::default();
                    source_kernel.tolerances.zone_tol_3d = 0.02;
                    source_kernel.context.set_doing_boolean(source_flag);
                    source_kernel
                        .context
                        .set_user_notify_callback(Some(Box::new(Log(notices.clone()))));
                    let source_context = store.insert_kernel(source_kernel).unwrap();
                    let dest_context = if shared {
                        source_context
                    } else {
                        let mut kernel = Kernel::default();
                        kernel.tolerances.zone_tol_3d = 0.125;
                        kernel.context.set_doing_boolean(dest_flag);
                        kernel
                            .context
                            .set_user_notify_callback(Some(Box::new(Log(notices.clone()))));
                        store.insert_kernel(kernel).unwrap()
                    };
                    let source = store.create_brep(source_context).unwrap();
                    let dest = store.create_brep(dest_context).unwrap();
                    let original = point(&mut store, source, Vec3::new(1.0, 2.0, 3.0));
                    notices.lock().unwrap().clear();
                    let region = store.brep(dest).unwrap().infinite_region().unwrap();
                    let mut targets = SweepTargets {
                        vertices_same: Some(vec![Some((source, original))]),
                        ..Default::default()
                    };
                    let mut sweep = TopologySweep::new(TopologySweepOptions {
                        stitching: false,
                        ..Default::default()
                    });
                    sweep
                        .do_sweep(
                            &mut store,
                            &translation(),
                            Some(source),
                            Some((dest, region)),
                            &mut targets,
                        )
                        .unwrap();
                    let result = sweep
                        .get_to_same(Some(SweepEntity::Vertex(source, original)))
                        .unwrap();
                    let SweepEntity::Vertex(owner, v) = result else {
                        panic!("not a vertex")
                    };
                    assert_eq!(owner, dest);
                    let b = store.brep(dest).unwrap();
                    assert_eq!(b.vertices().len(), 1);
                    assert_eq!(b.shells().len(), 1);
                    let vertex = b.vertex(v).unwrap();
                    assert_eq!(vertex.point, Vec3::new(11.0, -2.0, 9.0));
                    assert_eq!(
                        vertex.tolerance,
                        store
                            .brep(source)
                            .unwrap()
                            .vertex(original)
                            .unwrap()
                            .tolerance
                    );
                    assert_eq!(
                        store.brep(source).unwrap().vertex(original).unwrap().point,
                        Vec3::new(1.0, 2.0, 3.0)
                    );
                    let shell = ShellId::from_index(0);
                    assert_eq!(b.shell(shell).unwrap().region, Some(region));
                    assert_eq!(vertex.vertexuses.len(), 1);
                    assert_eq!(
                        b.vertexuse(vertex.vertexuses[0]).unwrap().owner,
                        VertexuseOwner::Shell(Some(shell))
                    );
                    let vertex_key = store.object_ref(dest, EntityRef::Vertex(v)).unwrap();
                    assert!(notices
                        .lock()
                        .unwrap()
                        .iter()
                        .any(|(op, _)| *op == NotifyOperation::AddToBrep));
                    assert!(store.records.contains_key(&vertex_key.key));
                    assert_ne!(
                        vertex_key,
                        store
                            .object_ref(source, EntityRef::Vertex(original))
                            .unwrap()
                    );
                    assert_eq!(
                        store.kernel(source).unwrap().context.doing_boolean(),
                        source_flag
                    );
                    assert_eq!(
                        store.kernel(dest).unwrap().context.doing_boolean(),
                        dest_flag
                    );
                    assert!(store.take_cleanup_errors().is_empty());
                }
            }
        }
    }

    #[test]
    fn higher_vertex_sweep_constructs_the_real_wire_and_preserves_contexts() {
        let mut store = SweepStore::owned();
        let c = store.insert_kernel(Kernel::default()).unwrap();
        let source = store.create_brep(c).unwrap();
        let dest = store.create_brep(c).unwrap();
        let original = point(&mut store, source, Vec3::new(1., 2., 3.));
        let region = store.brep(dest).unwrap().infinite_region().unwrap();
        let mut targets = SweepTargets {
            vertices_higher: Some(vec![Some((source, original))]),
            ..Default::default()
        };
        let mut sweep = TopologySweep::new(TopologySweepOptions {
            stitching: false,
            ..Default::default()
        });
        sweep
            .do_sweep(
                &mut store,
                &translation(),
                Some(source),
                Some((dest, region)),
                &mut targets,
            )
            .unwrap();
        let Some(SweepEntity::Edge(owner, edge)) =
            sweep.get_to_higher(Some(SweepEntity::Vertex(source, original)))
        else {
            panic!("missing edge");
        };
        assert_eq!(owner, dest);
        let b = store.brep(dest).unwrap();
        assert_eq!(b.edges().len(), 1);
        assert_eq!(b.vertices().len(), 2);
        assert_eq!(b.shells().len(), 1);
        let e = b.edge(edge).unwrap();
        assert_eq!(
            b.vertex(e.vertices[0]).unwrap().point,
            Vec3::new(1., 2., 3.)
        );
        assert_eq!(
            b.vertex(e.vertices[1]).unwrap().point,
            Vec3::new(11., -2., 9.)
        );
        assert_eq!(e.edgeuses.len(), 2);
        // The append-only arena retains the initial shell's detached vertexuse.
        // Native GetVertexuses walks the live per-vertex owning lists instead.
        assert_eq!(
            b.vertices()
                .iter()
                .map(|(_, v)| v.vertexuses.len())
                .sum::<usize>(),
            2
        );
        let curve = e.curve.as_ref().unwrap();
        assert_eq!(
            curve
                .evaluate_point(store.kernel(dest).unwrap(), e.interval.mid())
                .unwrap(),
            Vec3::new(6., 0., 6.)
        );
        assert_eq!(
            curve.notify_key(),
            Some(store.curve_object((dest, edge)).unwrap().key)
        );
        assert!(!store.kernel(source).unwrap().context.doing_boolean());
    }

    #[test]
    fn body_borrows_preserve_actual_context_and_reject_aliases_and_stale_handles() {
        let mut store = SweepStore::owned();
        let c = store.insert_kernel(Kernel::default()).unwrap();
        let a = store.create_brep(c).unwrap();
        let b = store.new_brep(a).unwrap();
        assert!(std::ptr::eq(
            store.kernel(a).unwrap(),
            store.kernel(b).unwrap()
        ));
        for (source, dest) in [(a, b), (b, a)] {
            store
                .with_breps_mut(source, dest, |k, s, d, _, _, _, env| {
                    k.tolerances.zone_tol_3d = 0.25;
                    assert!(env.context.is_some());
                    assert_ne!(s.key(), d.key());
                    d.set_editing_enabled(true);
                    Ok(())
                })
                .unwrap();
        }
        assert_eq!(store.kernel(a).unwrap().tolerances.zone_tol_3d, 0.25);
        assert!(store
            .with_breps_mut(a, a, |_, _, _, _, _, _, _| Ok(()))
            .is_err());
        store.delete_brep(a);
        store.delete_brep(a);
        assert!(store.brep(a).is_err());
        let next = store.new_brep(b).unwrap();
        assert_ne!(next, a);
        assert!(store.take_cleanup_errors().is_empty());
    }

    #[test]
    fn topology_marks_tags_and_native_cleanup_share_one_record_store() {
        let mut store = SweepStore::owned();
        let c = store.insert_kernel(Kernel::default()).unwrap();
        let a = store.create_brep(c).unwrap();
        let v = point(&mut store, a, Vec3::ZERO);
        let value = SweepEntity::Vertex(a, v);
        let mark = store.lock_mark(a, ALL_MARKS).unwrap();
        store.mark(value, mark).unwrap();
        let key = store.object_ref(a, EntityRef::Vertex(v)).unwrap().key;
        assert!(store.records[&key].is_marked(&store.kernel(a).unwrap().context, mark));
        assert!(store.is_marked(value, mark).unwrap());
        let tag = TagData {
            primary_id: 42,
            ..Default::default()
        };
        store.add_tag(value, tag).unwrap();
        assert_eq!(store.tag(value).unwrap(), Some(tag));
        let attribute = store
            .attributes(a)
            .unwrap()
            .find_attribute(key, ids::TAG)
            .unwrap();
        store.unlock_mark(a, mark).unwrap();
        assert!(!store.kernel(a).unwrap().context.is_mark_locked(mark));
        let next = store.lock_mark(a, ALL_MARKS).unwrap();
        assert!(!store.is_marked(value, next).unwrap());
        store.unlock_mark(a, next).unwrap();
        store.remove_brep(a).unwrap();
        assert!(!store.attributes.is_live(attribute));
        assert!(!store.records.contains_key(&key));
    }

    #[test]
    fn primitive_access_uses_the_same_brep_and_runs_the_planar_face_constructor() {
        let mut store = SweepStore::owned();
        let c = store.insert_kernel(Kernel::default()).unwrap();
        let b = store.create_brep(c).unwrap();
        store
            .with_primitive_brep(b, |k, b| {
                k.context.set_doing_boolean(true);
                b.set_editing_enabled(true);
                Ok(())
            })
            .unwrap();
        assert!(store.kernel(b).unwrap().context.doing_boolean());
        assert!(store.brep(b).unwrap().editing_enabled());
        store.delete_primitive_faces(b, &[]).unwrap();
        let region = store.brep(b).unwrap().infinite_region().unwrap();
        // `SmBrep::CreatePlanarFacesWith3DCurves` runs on this B-rep: without curves
        // there is no plane, so it fails with `SM_ERR` and leaves the B-rep empty.
        let mut curves = vec![None];
        let mut output = Vec::new();
        let error = store
            .planar_faces((b, region), &mut curves, 0.01, &mut output)
            .unwrap_err();
        assert!(!error.is_unported(), "{error:?}");
        assert_eq!(curves.len(), 1);
        assert!(output.is_empty());
        assert!(store.brep(b).unwrap().face_list().is_empty());
    }

    #[test]
    fn retained_hcr_scene_has_one_owner_and_is_released_with_its_handle() {
        let mut store = SweepStore::owned();
        let c = store.insert_kernel(Kernel::default()).unwrap();
        let original = store.create_brep(c).unwrap();
        let scene = Brep::new_with_infinite_region().unwrap();
        let hcr = Hcr::from_scene(
            store.kernel(original).unwrap(),
            scene,
            &Axis2Placement::IDENTITY,
            Extent3d::new(),
            HcrOptions {
                approx_tol_3d: 0.01,
                ang_tol_rad: 0.1,
                display_seams: false,
                display_smooth_edges: false,
            },
        );
        let id = store.retain_hcr(original, hcr).unwrap();
        assert!(std::ptr::eq(
            store.hcr_session(id).unwrap().scene_brep(),
            store.brep(id).unwrap()
        ));
        assert_eq!(store.context_of(id).unwrap(), c);
        store.delete_brep(id);
        assert!(store.hcr_session(id).is_err());
        assert!(store.contains_brep(original));
    }

    #[test]
    fn inserted_bodies_bring_their_attributes_and_taken_bodies_take_them_back() {
        // #1462: a B-rep's own attributes join the shared store under the body's
        // identities, and a taken body carries the lists of its live objects again
        let mut store = SweepStore::owned();
        let c = store.insert_kernel(Kernel::default()).unwrap();
        let mut raw = Brep::new_with_infinite_region().unwrap();
        let shell = raw.add_shell().unwrap();
        let f = raw.add_face(shell).unwrap();
        raw.add_attribute(
            EntityRef::Face(f),
            Attribute::long(10007, 7, Behavior::Copy),
        )
        .unwrap();
        let b = store.insert_brep(c, raw).unwrap();
        let key = store.object_ref(b, EntityRef::Face(f)).unwrap().key;
        let h = store
            .attributes(b)
            .unwrap()
            .find_attribute(key, 10007)
            .unwrap();
        assert_eq!(
            store.attributes(b).unwrap().get(h).unwrap().long_value(),
            Some(7)
        );
        let back = store.take_brep(b).unwrap();
        let a = back.find_attribute(EntityRef::Face(f), 10007).unwrap();
        assert_eq!(a.long_value(), Some(7));
        assert_eq!(back.entity_attributes(EntityRef::Face(f)).len(), 1);
    }

    #[test]
    fn face_collection_observes_the_same_persistent_marks_as_native_notifications() {
        let mut store = SweepStore::owned();
        let c = store.insert_kernel(Kernel::default()).unwrap();
        let mut raw = Brep::new_with_infinite_region().unwrap();
        let shell = raw.add_shell().unwrap();
        let f = raw.add_face(shell).unwrap();
        let b = store.insert_brep(c, raw).unwrap();
        let mark = store.lock_mark(b, ALL_MARKS).unwrap();
        let mut faces = Vec::new();
        store.collect_faces((b, f), mark, &mut faces).unwrap();
        assert_eq!(faces, vec![f]);
        assert!(store.is_marked(SweepEntity::Face(b, f), mark).unwrap());
        let key = store.object_ref(b, EntityRef::Face(f)).unwrap().key;
        assert!(store.records[&key].is_marked(&store.kernel(b).unwrap().context, mark));
        store.collect_faces((b, f), mark, &mut faces).unwrap();
        assert!(faces.is_empty());
        store.unlock_mark(b, mark).unwrap();
    }

    #[test]
    fn curve_tags_are_separate_from_topology_and_foreign_owners_are_rejected() {
        use crate::curve::line::Line;
        use crate::topology::Edge;
        let mut store = SweepStore::owned();
        let c = store.insert_kernel(Kernel::default()).unwrap();
        let mut raw = Brep::new_with_infinite_region().unwrap();
        let v1 = raw.add_vertex(Vec3::ZERO).unwrap();
        let v2 = raw.add_vertex(Vec3::X).unwrap();
        let edge = raw
            .add_edge_record(Edge {
                curve: Some(Curve::from(Line::from_points(Vec3::ZERO, Vec3::X, 3))),
                ..Edge::new([v1, v2])
            })
            .unwrap();
        let b = store.insert_brep(c, raw).unwrap();
        let curve_key = store.curve_object((b, edge)).unwrap().key;
        assert_ne!(
            curve_key,
            store.object_ref(b, EntityRef::Edge(edge)).unwrap().key
        );
        let curve_tag = TagData {
            primary_id: 11,
            ..Default::default()
        };
        let h = store
            .attributes_mut(b)
            .unwrap()
            .insert(Attribute::tag(ids::TAG, curve_tag, Behavior::Copy))
            .unwrap();
        store
            .attributes_mut(b)
            .unwrap()
            .add_attribute(curve_key, h)
            .unwrap();
        assert_eq!(store.curve_tag((b, edge)).unwrap(), Some(curve_tag));
        assert_eq!(store.tag(SweepEntity::Edge(b, edge)).unwrap(), None);
        assert!(store
            .insert_brep(c, store.brep(b).unwrap().clone())
            .is_err());
        store.remove_brep(b).unwrap();
        assert!(!store.attributes.is_live(h));
    }

    #[test]
    fn callback_scope_preserves_live_kernel_changes_and_restores_callbacks_on_error() {
        let mut store = SweepStore::owned();
        let notices = Arc::new(Mutex::new(Vec::new()));
        let mut k = Kernel::default();
        k.context
            .set_user_notify_callback(Some(Box::new(Log(notices.clone()))));
        let c = store.insert_kernel(k).unwrap();
        let b = store.create_brep(c).unwrap();
        let constructor_identity = store.object_ref(b, EntityRef::Brep(b)).unwrap();
        assert!(notices
            .lock()
            .unwrap()
            .iter()
            .any(|(op, caller)| *op == NotifyOperation::Construction
                && *caller == constructor_identity));
        let error: Result<()> = store.with_brep_mut(b, |k, b, _, env| {
            k.tolerances.zone_tol_3d = 0.3;
            k.context.set_doing_boolean(true);
            let r = b.infinite_region().unwrap();
            crate::topology::brep::p9::make_shell_vertex(b, Some(r), Vec3::ZERO, env)?;
            Err(SolidError::sm_err("injected after native allocation"))
        });
        assert!(error.is_err());
        assert_eq!(store.kernel(b).unwrap().tolerances.zone_tol_3d, 0.3);
        assert!(store.kernel(b).unwrap().context.doing_boolean());
        assert!(store
            .kernel(b)
            .unwrap()
            .context
            .user_notify_callback()
            .is_some());
        assert!(!notices.lock().unwrap().is_empty());
        assert_eq!(store.brep(b).unwrap().vertices().len(), 1);
        store
            .with_primitive_brep(b, |kernel, _| {
                assert!(kernel.context.user_notify_callback().is_some());
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn inserted_breps_record_face_tolerances_on_their_surfaces() {
        use crate::surface::plane::Plane;
        use crate::surface::surface::Surface;
        let mut raw = Brep::new_with_infinite_region().unwrap();
        let shell = raw.add_shell().unwrap();
        let face = raw.add_face(shell).unwrap();
        let record = raw.face_mut(face).unwrap();
        record.surface = Some(Surface::from(Plane::from_normal(Vec3::ZERO, Vec3::Z)));
        record.tolerance = 2.5e-4;
        let mut store = SweepStore::owned();
        let c = store.insert_kernel(Kernel::default()).unwrap();
        let b = store.insert_brep(c, raw).unwrap();
        let brep = store.brep(b).unwrap();
        let surface = brep.face(face).unwrap().surface.as_ref().unwrap();
        // the store owns the surface under the face: SmSurface::GetFace()->GetTolerance()
        assert_eq!(surface.face_zone_tol_3d().unwrap(), Some(2.5e-4));
    }

    #[test]
    fn axis_classification_reaches_the_native_face_boundary() {
        use crate::curve::line::Line;
        use crate::surface::plane::Plane;
        use crate::surface::surface::Surface;
        let mut raw = Brep::new_with_infinite_region().unwrap();
        let shell = raw.add_shell().unwrap();
        let face = raw.add_face(shell).unwrap();
        raw.face_mut(face).unwrap().surface =
            Some(Surface::from(Plane::from_normal(Vec3::ZERO, Vec3::Z)));
        let mut store = SweepStore::owned();
        let c = store.insert_kernel(Kernel::default()).unwrap();
        let b = store.insert_brep(c, raw).unwrap();
        let axis = Curve::from(Line::from_points(Vec3::ZERO, Vec3::X, 3));
        let result = store.axis_inside_face((b, face), &axis);
        // the face has no loops: the interval's point classification
        // (SmFace::Point3DClassify) finds it outside
        assert!(matches!(result, Ok(false)), "{result:?}");
    }

    #[test]
    fn native_curve_identity_domains_do_not_mix_attributes_across_contexts() {
        use crate::curve::line::Line;
        use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
        let mut store = SweepStore::owned();
        let mut handles = Vec::new();
        for tag_id in [41, 73] {
            let context = store.insert_kernel(Kernel::default()).unwrap();
            let body = store.create_brep(context).unwrap();
            let start = point(&mut store, body, Vec3::ZERO);
            let region = store.infinite_region(body).unwrap();
            let curve =
                Curve::from(Line::create_line_segment(3, Vec3::ZERO, Vec3::X, None).unwrap());
            let interval = curve.natural_interval();
            let (edge, _) = store
                .make_wire_edge_vertex(
                    (body, region),
                    (body, start),
                    curve,
                    interval,
                    OrientType::Same,
                    Vec3::X,
                )
                .unwrap();
            let identity = store.curve_object(edge).unwrap();
            let attributes = store.attributes_mut(body).unwrap();
            let attribute = attributes
                .insert(Attribute::tag(
                    ids::TAG,
                    TagData {
                        primary_id: tag_id,
                        ..Default::default()
                    },
                    Behavior::Copy,
                ))
                .unwrap();
            attributes.add_attribute(identity.key, attribute).unwrap();
            handles.push((edge, identity, tag_id));
        }
        assert_ne!(
            handles[0].1.key, handles[1].1.key,
            "owned contexts share a geometry sequence, not their identity domains"
        );
        for (edge, identity, tag_id) in handles {
            assert_eq!(store.curve_object(edge).unwrap(), identity);
            assert_eq!(store.curve_tag(edge).unwrap().unwrap().primary_id, tag_id);
        }
    }
}

#[cfg(test)]
mod borrowed_tests {
    use super::*;
    use crate::geom::context::ALL_MARKS;
    use crate::topology::{Face, FaceuseId, LoopId, ShellId, Vertex, VertexId};
    pub(super) fn register<'a>(
        store: &mut SweepStore<'a>,
        context: ContextId,
        brep: &'a mut Brep,
    ) -> Result<BrepId> {
        let id = BrepId::from_index(*store.bodies.next);
        brep.set_key(
            store
                .identities
                .0
                .unwrap()
                .topology(id, EntityRef::Brep(id))
                .key,
        );
        store.register_brep(context, brep)
    }
    pub(super) struct Identities;
    impl SweepIdentities for Identities {
        fn source_lists(&self, b: BrepId, r: crate::topology::RegionId) -> Result<[ObjectRef; 2]> {
            Ok([30_u64, 31].map(|kind| ObjectRef {
                type_code: 0,
                key: ObjectKey((u64::from(b.index()) << 40) | (kind << 32) | u64::from(r.index())),
            }))
        }
        fn topology(&self, b: BrepId, e: EntityRef) -> ObjectRef {
            let (kind, index) = match e {
                EntityRef::Brep(i) => (0, i.index()),
                EntityRef::Shell(i) => (1, i.index()),
                EntityRef::Face(i) => (2, i.index()),
                EntityRef::Loop(i) => (3, i.index()),
                EntityRef::Edge(i) => (4, i.index()),
                EntityRef::Vertex(i) => (5, i.index()),
                EntityRef::Region(i) => (6, i.index()),
                EntityRef::Faceuse(i) => (7, i.index()),
                EntityRef::Loopuse(i) => (8, i.index()),
                EntityRef::Edgeuse(i) => (9, i.index()),
                EntityRef::Vertexuse(i) => (10, i.index()),
            };
            let type_code = match e {
                EntityRef::Brep(_) => 16011,
                EntityRef::Shell(_) => 16013,
                EntityRef::Face(_) => 16002,
                EntityRef::Loop(_) => 16003,
                EntityRef::Edge(_) => 16016,
                EntityRef::Vertex(_) => 16021,
                EntityRef::Region(_) => 16012,
                EntityRef::Faceuse(_) => 16022,
                EntityRef::Loopuse(_) => 16004,
                EntityRef::Edgeuse(_) => 16005,
                EntityRef::Vertexuse(_) => 16006,
            };
            ObjectRef {
                type_code,
                key: ObjectKey((u64::from(b.index()) << 40) | (kind << 32) | u64::from(index)),
            }
        }
        fn curve(&self, b: BrepId, e: EdgeId, _: &Curve) -> ObjectRef {
            ObjectRef {
                type_code: 11001,
                key: ObjectKey((u64::from(b.index()) << 40) | (20 << 32) | u64::from(e.index())),
            }
        }
        fn owned_auxiliary_objects(&self, _: BrepId, _: &Brep) -> Vec<ObjectKey> {
            Vec::new()
        }
    }
    #[test]
    fn borrowed_key_mismatch_rejected_without_consuming_handle() {
        let mut k = Kernel::default();
        let mut b = Brep::new();
        b.set_key(ObjectKey(991));
        let mut records = crate::topology::topology::NotifyRecords::new();
        let mut attributes = AttributeStore::new();
        let mut next = 15;
        let mut store = SweepStore::new(&Identities, &mut records, &mut attributes, &mut next);
        let c = store.register_kernel(&mut k).unwrap();
        assert!(store.register_brep(c, &mut b).is_err());
        assert_eq!(*store.bodies.next, 15);
        assert!(store.bodies.slots.is_empty());
    }
    #[test]
    fn adopted_bodies_keep_their_identities_and_can_be_taken_back() {
        let mut k = Kernel::default();
        let mut records = crate::topology::topology::NotifyRecords::new();
        let mut attributes = AttributeStore::new();
        let mut next = 4;
        let mut store = SweepStore::new(&Identities, &mut records, &mut attributes, &mut next);
        let c = store.register_kernel(&mut k).unwrap();
        let id = BrepId::from_index(4);
        let mut wrong = Brep::new();
        wrong.set_key(ObjectKey(991));
        assert!(store.adopt_brep(c, wrong).is_err());
        let mut b = Brep::new_with_infinite_region().unwrap();
        b.set_key(Identities.topology(id, EntityRef::Brep(id)).key);
        b.set_tolerance(0.25);
        assert_eq!(store.adopt_brep(c, b).unwrap(), id);
        // nothing is re-keyed or registered for the adopted body
        assert_eq!(
            store.brep(id).unwrap().key(),
            Identities.topology(id, EntityRef::Brep(id)).key
        );
        assert!(store.records.is_empty());
        let b = store.take_brep(id).unwrap();
        assert_eq!(b.tolerance(), 0.25);
        assert!(!store.contains_brep(id));
        assert!(store.take_brep(id).is_err());
        // a store without caller identities cannot adopt
        let mut owned = SweepStore::owned();
        let c = owned.insert_kernel(Kernel::default()).unwrap();
        assert!(owned.adopt_brep(c, Brep::new()).is_err());
    }
    #[test]
    fn actual_shared_and_distinct_contexts_and_borrow_orders() {
        let mut k1 = Kernel::default();
        let mut k2 = Kernel::default();
        k1.context.set_doing_boolean(true);
        k1.tolerances.zone_tol_3d = 0.002;
        k2.tolerances.zone_tol_3d = 0.007;
        let mut a = Brep::new();
        let mut b = Brep::new();
        let mut c = Brep::new();
        a.set_editing_enabled(true);
        let mut records = crate::topology::topology::NotifyRecords::new();
        let mut attributes = AttributeStore::new();
        let mut next_handle = 0;
        let mut store =
            SweepStore::new(&Identities, &mut records, &mut attributes, &mut next_handle);
        let shared = store.register_kernel(&mut k1).unwrap();
        let distinct = store.register_kernel(&mut k2).unwrap();
        let a = register(&mut store, shared, &mut a).unwrap();
        let b = register(&mut store, shared, &mut b).unwrap();
        let c = register(&mut store, distinct, &mut c).unwrap();
        assert!(std::ptr::eq(
            store.kernel(a).unwrap(),
            store.kernel(b).unwrap()
        ));
        assert!(!std::ptr::eq(
            store.kernel(a).unwrap(),
            store.kernel(c).unwrap()
        ));
        for (source, destination) in [(a, c), (c, a), (a, b), (b, a)] {
            let expected = store.kernel(destination).unwrap().context.doing_boolean();
            let tolerance = store.kernel(destination).unwrap().tolerances.zone_tol_3d;
            store
                .with_breps_mut(source, destination, |k, src, dst, _, face, _, env| {
                    assert_eq!(k.context.doing_boolean(), expected);
                    assert_eq!(k.tolerances.zone_tol_3d, tolerance);
                    assert!(env.context.is_some());
                    assert_eq!(
                        face(FaceId::from_index(12)),
                        (env.key_of)(EntityRef::Face(FaceId::from_index(12)))
                    );
                    src.set_editing_enabled(true);
                    dst.set_editing_enabled(false);
                    Ok(())
                })
                .unwrap();
        }
        assert!(store
            .with_breps_mut(a, a, |_, _, _, _, _, _, _| Ok(()))
            .is_err());
        assert!(store
            .with_breps_mut(
                a,
                BrepId::from_index(u32::MAX),
                |_, _, _, _, _, _, _| Ok(())
            )
            .is_err());
        let error: Result<()> = store.with_brep_mut(a, |_, _, _, env| {
            assert!(env.context.is_some());
            Err(invalid("injected borrowed-adapter error"))
        });
        assert!(error.is_err());
        assert!(store.kernel(a).unwrap().context.doing_boolean());
    }
    #[test]
    fn retained_marks_partial_traversal_and_owned_cleanup() {
        let mut k = Kernel::default();
        let mut b = Brep::new();
        let mut face = Face::new(ShellId::from_index(0));
        face.loops.push(LoopId::from_index(99));
        face.faceuse = Some(FaceuseId::from_index(99));
        let f = b.add_face_record(face).unwrap();
        let mut records = crate::topology::topology::NotifyRecords::new();
        let mut attributes = AttributeStore::new();
        let mut next_handle = 0;
        let mut store =
            SweepStore::new(&Identities, &mut records, &mut attributes, &mut next_handle);
        let ctx = store.register_kernel(&mut k).unwrap();
        let id = register(&mut store, ctx, &mut b).unwrap();
        let m = store.lock_mark(id, ALL_MARKS).unwrap();
        assert!(store.kernel(id).unwrap().context.is_mark_locked(m));
        let mut faces = vec![FaceId::from_index(77)];
        assert!(store.collect_faces((id, f), m, &mut faces).is_err());
        assert_eq!(faces, vec![f]);
        assert!(store.is_marked(SweepEntity::Face(id, f), m).unwrap());
        store.collect_faces((id, f), m, &mut faces).unwrap();
        assert!(faces.is_empty());
        // A notification-side record update is the next traversal's authority.
        store
            .with_brep_notifications(id, |_, env| {
                let key = (env.key_of)(EntityRef::Face(f)).key;
                env.records.insert(key, Topology::default());
                Ok(())
            })
            .unwrap();
        assert!(!store.is_marked(SweepEntity::Face(id, f), m).unwrap());
        assert!(store.collect_faces((id, f), m, &mut faces).is_err());
        assert_eq!(faces, vec![f]);
        let m2 = store.lock_mark(id, ALL_MARKS).unwrap();
        assert_ne!(m, m2);
        store.unlock_mark(id, m2).unwrap();
        assert!(store.kernel(id).unwrap().context.is_mark_locked(m));
        store.unlock_mark(id, m).unwrap();
        assert!(store
            .mark(SweepEntity::Vertex(id, VertexId::from_index(90)), m)
            .is_err());
        let owned = store.new_brep(id).unwrap();
        let v = store
            .brep_mut(owned)
            .unwrap()
            .add_vertex_record(Vertex::default())
            .unwrap();
        let value = SweepEntity::Vertex(owned, v);
        store
            .add_tag(
                value,
                TagData {
                    primary_id: 41,
                    ..TagData::default()
                },
            )
            .unwrap();
        store
            .add_tag(
                value,
                TagData {
                    primary_id: 42,
                    ..TagData::default()
                },
            )
            .unwrap();
        assert_eq!(store.tag(value).unwrap().unwrap().primary_id, 41);
        let (owner, topology) = entity(value);
        let key = store.object_ref(owner, topology).unwrap().key;
        store.records.insert(key, Topology::default());
        store.delete_brep(owned);
        store.delete_brep(owned);
        assert!(store.brep(owned).is_err());
        assert!(store.attributes.attributes(key).is_empty());
        assert!(!store.records.contains_key(&key));
        let later = store.new_brep(id).unwrap();
        assert!(later.index() > owned.index());
        store.delete_brep(id);
        assert!(store.brep(id).is_ok());
    }
}

#[cfg(test)]
mod bounding_box_tests {
    use super::borrowed_tests::{register, Identities};
    use super::*;
    use crate::curve::ellipse::{Ellipse, EllipseOptions};
    use crate::geom::{extent1d::Extent1d, extent2d::Extent2d, extent3d::Extent3d};
    use crate::math::{Point3, Vec3};
    use crate::nurbs::{b_spline_surface::BSplineSurface, math::CPoint};
    use crate::surface::surface::Surface;
    use crate::topology::brep::p5 as brep_p5;
    use crate::topology::brep::p8::{create_face_from_surface, FaceBuildEnv};
    use crate::topology::topology_traverser::Marks;
    use crate::topology::ShellId;

    #[test]
    fn borrowed_bounding_box_includes_curved_edge_and_honours_lock() {
        let mut k = Kernel::default();
        let mut b = Brep::new_with_infinite_region().unwrap();
        let circle = Ellipse::new_circle(
            &k,
            Point3::ZERO,
            Vec3::X,
            Vec3::Y,
            Extent1d::from_raw(0.0, 180.0),
            2.0,
            &EllipseOptions::default(),
        )
        .unwrap();
        let curve = Curve::from(circle);
        let interval = curve.natural_interval();
        brep_p5::create_wire_edge_from_curve(&mut k, &mut b, curve, interval).unwrap();
        let vertex = b.vertices().ids().next().unwrap();
        let mut records = crate::topology::topology::NotifyRecords::new();
        let mut attrs = AttributeStore::new();
        let mut next = 0;
        {
            let mut store = SweepStore::new(&Identities, &mut records, &mut attrs, &mut next);
            let context = store.register_kernel(&mut k).unwrap();
            let id = register(&mut store, context, &mut b).unwrap();
            let mut bbox = Extent3d::new();
            store.bounding_box(id, &mut bbox).unwrap();
            // Both end vertices are on y=0; the arc reaches y=2.
            assert!(bbox.max().y >= 2.0);
            store
                .with_brep_mut(id, |k, b, _, _| {
                    crate::query::brep::lock_bounding_box(k, b, false)
                })
                .unwrap();
            store
                .brep_mut(id)
                .unwrap()
                .vertex_mut(vertex)
                .unwrap()
                .point
                .x = 100.0;
            let locked = bbox;
            store.bounding_box(id, &mut bbox).unwrap();
            assert_eq!(bbox, locked);
        }
        assert_eq!(b.vertex(vertex).unwrap().point.x, 100.0);
    }

    #[test]
    fn borrowed_bounding_box_includes_trimmed_surface_interior() {
        let mut k = Kernel::default();
        let mut b = Brep::new_with_infinite_region().unwrap();
        let id = BrepId::from_index(0);
        b.set_key(Identities.topology(id, EntityRef::Brep(id)).key);
        let mut net = vec![vec![CPoint::new(0.0, 0.0, 0.0, 1.0); 3]; 3];
        for (i, row) in net.iter_mut().enumerate() {
            for (j, cp) in row.iter_mut().enumerate() {
                *cp = CPoint::new(
                    i as f64 * 3.0,
                    j as f64 * 2.0,
                    if i == 1 && j == 1 { 5.0 } else { 0.0 },
                    1.0,
                );
            }
        }
        let knots = [0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
        let surface = Surface::from(
            BSplineSurface::from_data(2, 2, 2, 2, 5, 5, &net, &knots, &knots).unwrap(),
        );
        let domain = Extent2d::from_bounds(0.2, 0.25, 0.8, 0.75);
        let mut records = crate::topology::topology::NotifyRecords::new();
        let mut attrs = AttributeStore::new();
        let face = {
            let key_of = |entity| Identities.topology(id, entity);
            let face_key = |face| key_of(EntityRef::Face(face));
            let mut env = NotifyEnv {
                records: &mut records,
                attributes: &mut attrs,
                context: None,
                key_of: &key_of,
            };
            let mut marks = Marks::default();
            create_face_from_surface(
                &mut k,
                &mut b,
                Some(surface),
                &domain,
                &mut FaceBuildEnv {
                    env: &mut env,
                    marks: &mut marks,
                    this_face: &face_key,
                },
            )
            .unwrap()
        };
        let before = format!("{b:?}");
        let mut next = 0;
        {
            let mut store = SweepStore::new(&Identities, &mut records, &mut attrs, &mut next);
            let context = store.register_kernel(&mut k).unwrap();
            let id = register(&mut store, context, &mut b).unwrap();
            let mut bbox = Extent3d::new();
            store.bounding_box(id, &mut bbox).unwrap();
            // The default loose surface box includes its elevated control point;
            // a vertex/edge-only approximation cannot reach this height.
            assert!(bbox.max().z >= 5.0);
        }
        assert_eq!(b.face(face).unwrap().uv_domain, domain);
        assert_eq!(b.face(face).unwrap().loops.len(), 1);
        assert_eq!(format!("{b:?}"), before);
    }

    #[test]
    fn bounding_box_replaces_empty_output_and_preserves_it_on_error() {
        let mut store = SweepStore::owned();
        let context = store.insert_kernel(Kernel::default()).unwrap();
        let id = store.create_brep(context).unwrap();
        let sentinel = Extent3d::from_point(Point3::new(11.0, 12.0, 13.0));
        let mut output = sentinel;
        store.bounding_box(id, &mut output).unwrap();
        assert_eq!(output, Extent3d::new());
        output = sentinel;
        assert!(store
            .bounding_box(BrepId::from_index(1234), &mut output)
            .is_err());
        assert_eq!(output, sentinel);
        store
            .brep_mut(id)
            .unwrap()
            .add_face_record(crate::topology::Face::new(ShellId::from_index(0)))
            .unwrap();
        let error = store.bounding_box(id, &mut output).unwrap_err();
        assert!(!error.is_unported());
        assert_eq!(output, sentinel);
        assert!(store.with_brep_mut(id, |_, _, _, _| Ok(())).is_ok());
    }
}

#[cfg(test)]
mod production_path_tests {
    use super::borrowed_tests::{register, Identities};
    use super::*;
    use crate::blend::hcr::HcrOptions;
    use crate::geom::axis2_placement::Axis2Placement;
    use crate::geom::extent3d::Extent3d;
    use crate::sweep::topology_sweep::engine::SweepTargets;
    use crate::sweep::topology_sweep::geometry::SweepGeometry;
    use crate::sweep::topology_sweep::{SweepGeometryCreation, TopologySweep};
    #[test]
    fn public_sweep_early_failure_restores_shared_and_distinct_flags() {
        for shared in [false, true] {
            let mut k1 = Kernel::default();
            let mut k2 = Kernel::default();
            k2.context.set_doing_boolean(true);
            let mut original = Brep::new_with_infinite_region().unwrap();
            let mut destination = Brep::new_with_infinite_region().unwrap();
            let mut records = crate::topology::topology::NotifyRecords::new();
            let mut attrs = AttributeStore::new();
            let mut next_handle = 0;
            let mut store =
                SweepStore::new(&Identities, &mut records, &mut attrs, &mut next_handle);
            let c1 = store.register_kernel(&mut k1).unwrap();
            let c2 = if shared {
                c1
            } else {
                store.register_kernel(&mut k2).unwrap()
            };
            let source = register(&mut store, c1, &mut original).unwrap();
            let dest = register(&mut store, c2, &mut destination).unwrap();
            let region = store.infinite_region(dest).unwrap();
            let before_source = store.kernel(source).unwrap().context.doing_boolean();
            let before_dest = store.kernel(dest).unwrap().context.doing_boolean();
            let mut targets = SweepTargets {
                faces_higher: Some(vec![Some((source, FaceId::from_index(123)))]),
                ..SweepTargets::default()
            };
            let error = TopologySweep::default()
                .do_sweep(
                    &mut store,
                    &SweepGeometry::Base(SweepGeometryCreation::default()),
                    Some(source),
                    Some((dest, region)),
                    &mut targets,
                )
                .unwrap_err();
            assert!(matches!(error, SolidError::NotFound { .. }));
            assert_eq!(
                store.kernel(source).unwrap().context.doing_boolean(),
                before_source
            );
            assert_eq!(
                store.kernel(dest).unwrap().context.doing_boolean(),
                before_dest
            );
            assert_eq!(
                store
                    .bodies
                    .slots
                    .iter()
                    .filter(|e| e
                        .as_ref()
                        .is_some_and(|e| !matches!(e.body, Body::Borrowed(_))))
                    .count(),
                0
            );
        }
    }
    #[test]
    fn hcr_scene_access_and_idempotent_release() {
        let mut k = Kernel::default();
        let mut original = Brep::new_with_infinite_region().unwrap();
        let mut records = crate::topology::topology::NotifyRecords::new();
        let mut attrs = AttributeStore::new();
        let mut next_handle = 0;
        let mut store = SweepStore::new(&Identities, &mut records, &mut attrs, &mut next_handle);
        let c = store.register_kernel(&mut k).unwrap();
        let original = register(&mut store, c, &mut original).unwrap();
        let hcr = Hcr::from_scene(
            store.kernel(original).unwrap(),
            Brep::new_with_infinite_region().unwrap(),
            &Axis2Placement::default(),
            Extent3d::new(),
            HcrOptions {
                approx_tol_3d: 0.01,
                ang_tol_rad: 0.02,
                display_seams: true,
                display_smooth_edges: true,
            },
        );
        let scene = store.retain_hcr(original, hcr).unwrap();
        assert!(std::ptr::eq(
            store.brep(scene).unwrap(),
            store.hcr_session(scene).unwrap().scene_brep()
        ));
        store.brep_mut(scene).unwrap().set_editing_enabled(true);
        assert!(store
            .hcr_session(scene)
            .unwrap()
            .scene_brep()
            .editing_enabled());
        store
            .with_breps_mut(scene, original, |_, s, d, _, _, _, _| {
                assert!(s.editing_enabled());
                assert!(!d.editing_enabled());
                Ok(())
            })
            .unwrap();
        store.delete_brep(scene);
        store.delete_brep(scene);
        assert!(store.hcr_session(scene).is_err());
        assert!(store.brep(scene).is_err());
        assert!(store.brep(original).is_ok());
    }
}

#[cfg(test)]
mod persistence_tests {
    use super::borrowed_tests::{register, Identities};
    use super::*;
    use crate::curve::line::Line;
    use crate::geom::context::{NotifyCallback, ALL_MARKS};
    use crate::geom::object::{self, NotifyOperation};
    use crate::math::Vec3;
    use crate::topology::{Edge, Vertex, VertexId};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    #[derive(Debug)]
    struct Counter(Arc<AtomicUsize>);
    impl NotifyCallback for Counter {
        fn execute(
            &mut self,
            _: NotifyOperation,
            _: ObjectRef,
            _: Option<ObjectRef>,
            _: Option<ObjectRef>,
            _: Option<ObjectRef>,
        ) -> Result<()> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }
    #[test]
    fn actual_callbacks_survive_early_error_and_mark_exhaustion_preserves_owner() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut k = Kernel::default();
        k.context
            .set_user_notify_callback(Some(Box::new(Counter(calls.clone()))));
        let mut foreign = MarkLock::new(
            std::slice::from_mut(&mut k.context),
            0,
            MarkType::Mark.code(),
        )
        .unwrap();
        let mut b = Brep::new();
        let mut records = crate::topology::topology::NotifyRecords::new();
        let mut attrs = AttributeStore::new();
        let mut next = 0;
        {
            let mut store = SweepStore::new(&Identities, &mut records, &mut attrs, &mut next);
            let c = store.register_kernel(&mut k).unwrap();
            let id = register(&mut store, c, &mut b).unwrap();
            let error: Result<()> = store.with_brep_mut(id, |_, _, _, env| {
                object::notify(
                    env.context.as_deref_mut(),
                    NotifyOperation::PreEdit,
                    (env.key_of)(EntityRef::Brep(id)),
                    None,
                    None,
                    None,
                );
                Err(invalid("injected borrowed-adapter error"))
            });
            assert!(error.is_err());
            store
                .with_brep_notifications(id, |_, env| {
                    object::notify(
                        env.context.as_deref_mut(),
                        NotifyOperation::PostEdit,
                        (env.key_of)(EntityRef::Brep(id)),
                        None,
                        None,
                        None,
                    );
                    Ok(())
                })
                .unwrap();
            assert_eq!(calls.load(Ordering::SeqCst), 2);
            assert!(store.lock_mark(id, MarkType::Mark.code()).is_err());
            assert!(store
                .kernel(id)
                .unwrap()
                .context
                .is_mark_locked(MarkType::Mark));
            let _owned = store.lock_mark(id, ALL_MARKS).unwrap();
        }
        assert!(k.context.is_mark_locked(MarkType::Mark));
        assert!(!k.context.is_mark_locked(MarkType::Mark2));
        assert!(k.context.user_notify_callback().is_some());
        foreign
            .release(std::slice::from_mut(&mut k.context))
            .unwrap();
    }
    struct StableOriginal {
        local: BrepId,
    }
    impl SweepIdentities for StableOriginal {
        fn source_lists(&self, b: BrepId, r: crate::topology::RegionId) -> Result<[ObjectRef; 2]> {
            Identities.source_lists(b, r)
        }
        fn topology(&self, b: BrepId, e: EntityRef) -> ObjectRef {
            Identities.topology(
                if b == self.local {
                    BrepId::from_index(0)
                } else {
                    b
                },
                e,
            )
        }
        fn curve(&self, _: BrepId, _: EdgeId, c: &Curve) -> ObjectRef {
            ObjectRef {
                type_code: c.type_code(),
                key: ObjectKey(900_000),
            }
        }
        fn owned_auxiliary_objects(&self, _: BrepId, _: &Brep) -> Vec<ObjectKey> {
            Vec::new()
        }
    }
    #[test]
    fn persistent_original_attributes_monotonic_handles_and_curve_identity() {
        let mut k = Kernel::default();
        let mut original = Brep::new();
        let v = original.add_vertex_record(Vertex::default()).unwrap();
        let mut edge = Edge::new([v, v]);
        edge.curve =
            Some(Line::from_points(Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0), 3).into());
        let edge = original.add_edge_record(edge).unwrap();
        let mut records = crate::topology::topology::NotifyRecords::new();
        let mut attrs = AttributeStore::new();
        let mut next = 0;
        let old_temporary;
        let curve_tag = attrs
            .insert(Attribute::tag(
                ids::TAG,
                TagData {
                    primary_id: 73,
                    ..TagData::default()
                },
                Behavior::Copy,
            ))
            .unwrap();
        attrs.add_attribute(ObjectKey(900_000), curve_tag).unwrap();
        {
            let identities = StableOriginal {
                local: BrepId::from_index(next),
            };
            let mut store = SweepStore::new(&identities, &mut records, &mut attrs, &mut next);
            let c = store.register_kernel(&mut k).unwrap();
            let id = register(&mut store, c, &mut original).unwrap();
            store
                .add_tag(
                    SweepEntity::Vertex(id, v),
                    TagData {
                        primary_id: 81,
                        ..TagData::default()
                    },
                )
                .unwrap();
            assert_eq!(store.curve_tag((id, edge)).unwrap().unwrap().primary_id, 73);
            old_temporary = store.new_brep(id).unwrap();
            let temporary_v = store
                .brep_mut(old_temporary)
                .unwrap()
                .add_vertex_record(Vertex::default())
                .unwrap();
            store
                .add_tag(
                    SweepEntity::Vertex(old_temporary, temporary_v),
                    TagData::default(),
                )
                .unwrap();
        }
        let temporary_key = Identities
            .topology(old_temporary, EntityRef::Vertex(VertexId::from_index(0)))
            .key;
        assert!(attrs.attributes(temporary_key).is_empty());
        {
            let identities = StableOriginal {
                local: BrepId::from_index(next),
            };
            let mut store = SweepStore::new(&identities, &mut records, &mut attrs, &mut next);
            let c = store.register_kernel(&mut k).unwrap();
            let id = register(&mut store, c, &mut original).unwrap();
            assert_eq!(
                store
                    .tag(SweepEntity::Vertex(id, v))
                    .unwrap()
                    .unwrap()
                    .primary_id,
                81
            );
            assert_eq!(store.curve_tag((id, edge)).unwrap().unwrap().primary_id, 73);
            assert!(store.brep(old_temporary).is_err());
            let new = store.new_brep(id).unwrap();
            assert!(new.index() > old_temporary.index());
        }
        assert!(attrs.is_live(curve_tag));
    }
}

#[cfg(test)]
mod borrowed_wire_tests {
    use super::borrowed_tests::{register, Identities};
    use super::*;
    use crate::math::Vec3;
    use crate::sweep::topology_sweep::engine::SweepTargets;
    use crate::sweep::topology_sweep::geometry::SweepGeometry;
    use crate::sweep::topology_sweep::TopologySweep;
    use crate::sweep::trans_sweep_geometry::TranslationalSweepGeometry;
    #[test]
    fn public_translation_builds_a_wire_in_borrowed_shared_and_distinct_contexts() {
        for shared in [false, true] {
            let mut k1 = Kernel::default();
            let mut k2 = Kernel::default();
            let mut original = Brep::new_with_infinite_region().unwrap();
            let mut destination = Brep::new_with_infinite_region().unwrap();
            let v = original
                .add_vertex_record(crate::topology::Vertex::default())
                .unwrap();
            let mut records = crate::topology::topology::NotifyRecords::new();
            let mut attributes = AttributeStore::new();
            let mut next = 0;
            let mut store = SweepStore::new(&Identities, &mut records, &mut attributes, &mut next);
            let c1 = store.register_kernel(&mut k1).unwrap();
            let c2 = if shared {
                c1
            } else {
                store.register_kernel(&mut k2).unwrap()
            };
            let source = register(&mut store, c1, &mut original).unwrap();
            let dest = register(&mut store, c2, &mut destination).unwrap();
            let region = store.infinite_region(dest).unwrap();
            let mut sweep = TopologySweep {
                do_stitching: false,
                ..TopologySweep::default()
            };
            let mut targets = SweepTargets {
                vertices_higher: Some(vec![Some((source, v))]),
                ..SweepTargets::default()
            };
            let geometry = SweepGeometry::Translation(TranslationalSweepGeometry::new(Vec3::new(
                0.0, 0.0, 2.0,
            )));
            sweep
                .do_sweep(
                    &mut store,
                    &geometry,
                    Some(source),
                    Some((dest, region)),
                    &mut targets,
                )
                .unwrap();
            let result = store.brep(dest).unwrap();
            assert_eq!(result.vertices().len(), 2);
            assert_eq!(result.edges().len(), 1);
            assert_eq!(result.shells().len(), 1);
            let Some(SweepEntity::Vertex(owner, vertex)) =
                sweep.get_to_same(Some(SweepEntity::Vertex(source, v)))
            else {
                panic!("missing translated endpoint");
            };
            assert_eq!(owner, dest);
            assert_eq!(
                result.vertex(vertex).unwrap().point,
                Vec3::new(0.0, 0.0, 2.0)
            );
            let Some(SweepEntity::Edge(owner, edge)) =
                sweep.get_to_higher(Some(SweepEntity::Vertex(source, v)))
            else {
                panic!("missing swept edge");
            };
            assert_eq!(owner, dest);
            assert!(result.edge(edge).unwrap().vertices.contains(&vertex));
            assert_eq!(store.brep(source).unwrap().vertices().len(), 1);
            assert_eq!(store.brep(source).unwrap().shells().len(), 0);
            assert!(!store.kernel(source).unwrap().context.doing_boolean());
            assert!(!store.kernel(dest).unwrap().context.doing_boolean());
        }
    }
    #[test]
    fn borrowed_vertex_copies_keep_real_contexts_attributes_and_merge_mates() {
        use crate::geom::attribute::{Attribute, AttributeValue, Behavior};
        for shared in [false, true] {
            let (mut source_kernel, mut destination_kernel) =
                (Kernel::default(), Kernel::default());
            let mut source_body = Brep::new_with_infinite_region().unwrap();
            let mut destination_body = Brep::new_with_infinite_region().unwrap();
            let (mut records, mut attributes, mut next) = (
                crate::topology::topology::NotifyRecords::new(),
                AttributeStore::new(),
                29,
            );
            let mut store = SweepStore::new(&Identities, &mut records, &mut attributes, &mut next);
            let source_context = store.register_kernel(&mut source_kernel).unwrap();
            let destination_context = if shared {
                source_context
            } else {
                store.register_kernel(&mut destination_kernel).unwrap()
            };
            let source = register(&mut store, source_context, &mut source_body).unwrap();
            let destination =
                register(&mut store, destination_context, &mut destination_body).unwrap();
            let region = store.infinite_region(source).unwrap();
            let vertex = store
                .make_shell_vertex((source, region), Vec3::new(1., 2., 3.))
                .unwrap()
                .1
                 .1;
            let source_key = store
                .object_ref(source, EntityRef::Vertex(vertex))
                .unwrap()
                .key;
            let attribute = store
                .attributes_mut(source)
                .unwrap()
                .insert(Attribute::long(8765, 42, Behavior::Reference))
                .unwrap();
            store
                .attributes_mut(source)
                .unwrap()
                .add_attribute(source_key, attribute)
                .unwrap();
            let copied = store.copy_vertices(source, &[vertex], destination).unwrap()[0];
            let destination_key = store
                .object_ref(destination, EntityRef::Vertex(copied))
                .unwrap()
                .key;
            assert_ne!(source_key, destination_key);
            assert_eq!(
                store
                    .brep(destination)
                    .unwrap()
                    .vertex(copied)
                    .unwrap()
                    .point,
                Vec3::new(1., 2., 3.)
            );
            assert_eq!(
                store
                    .attributes(destination)
                    .unwrap()
                    .find_attribute(destination_key, 8765),
                Some(attribute)
            );
            assert_eq!(
                store
                    .attributes(source)
                    .unwrap()
                    .get(attribute)
                    .unwrap()
                    .value(),
                &AttributeValue::Long(42)
            );
            let merge = store
                .new_merge(source, destination, source, 0.01, 0.1)
                .unwrap();
            store
                .merge_relations(merge)
                .unwrap()
                .vertices
                .record(copied, vertex)
                .unwrap();
            store
                .copy_faces(source, &[], destination, merge, &mut BTreeMap::new())
                .unwrap();
            assert_eq!(
                store.merge_relations(merge).unwrap().vertices.other(copied),
                Some(vertex)
            );
            store.delete_merge(merge);
            store.delete_brep(source);
            store.delete_brep(destination);
            assert!(store.contains_brep(source));
            assert!(store.contains_brep(destination));
            drop(store);
            assert_eq!(attributes.find_attribute(source_key, 8765), Some(attribute));
            assert_eq!(
                attributes.find_attribute(destination_key, 8765),
                Some(attribute)
            );
            assert_eq!(next, 31);
        }
    }
}
