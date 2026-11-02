//! The `SmMerge` objects `SmPrimitiveCreation` builds over the model's B-reps:
//! construction, `PiecewiseMerge`, `ManifoldBoolean(SM_BO_UNION)` and `CopyFaces`
//! through a retained merge.
//!
//! Upstream: `source/SMLib/src/SmMerge.cpp`, `source/SMLib/src/SmPrimitiveCreation.cpp`
//! (`CreateTaperExtrude`'s `SmMerge sMergeExec(*pContext, pSurfXSectBrep, pSurfBrep,
//! dBrepTol)`).
//!
//! | Rust        | Upstream                                                   |
//! |-------------|------------------------------------------------------------|
//! | `Session`   | a scoped `SmMerge` object (`SmMerge::SmMerge`)              |
//! | `Driver`    | its operand access for `PiecewiseMerge` / `ManifoldBoolean` |
//!
//! Deviations: the session is the engine `SmMerge::SmMerge` builds
//! ([`MergeEngine::from_merge`], namespace 0), as [`DependencyMerge`] adopts it; the
//! operands stay in the model's slots and move into the Boolean only for its call,
//! returning to the same slots (a consumed operand's slot is left empty). Offset-map
//! attributes (`SM_AI_OFFSETMAP`, `SM_AB_COPY`) live in the model, keyed by face; for
//! each merge stage they are placed on the operands' face identities in the engine's
//! attribute store as copy-behaviour pointer attributes to a stage table, so the
//! stage's copy and split notifications carry them to new faces as upstream's surface
//! copies do, and are read back from the surviving operands afterwards. The model's
//! geometry is owned under the model's identities (`loopuse::notify_ref`); for each
//! stage the operands' own geometry is relabelled with the engine's import label
//! (`brep_cache::arena_identity`, which the engine binds to its operand keys, so its
//! temporary origin shift moves and restores that geometry) and labelled back on the
//! survivors afterwards; `PiecewiseMerge`'s surface subset uses the relabelled
//! identities. No recursion.
//!
//! [`DependencyMerge`]: crate::boolean::merge::p3::DependencyMerge

use crate::boolean::merge::p1::{MergeOptions, MergeState};
use crate::boolean::merge::p2::MergeOperand;
use crate::boolean::merge::p3::{BooleanDriver, MergeEngine, PiecewiseAccess};
use crate::boolean::merge::p4::{
    notification_key, BooleanKernels, KernelManifoldIntersector, ManifoldIntersector, ManifoldState,
};
use crate::curve::composite_curve::OffsetMapAttribute;
use crate::error::{EntityRef, Result, SolidError};
use crate::geom::a_object::BooleanOperation;
use crate::geom::attribute::{ids, Attribute, AttributeStore, AttributeValue, Behavior};
use crate::geom::context::MarkLock;
use crate::geom::object::{NotifyOperation, ObjectRef};
use crate::kernel::Kernel;
use crate::sweep::topology_sweep::access::FaceRef;
use crate::topology::brep::p2::ArenaNotifyState;
use crate::topology::brep_cache::{arena_identity, CacheObject};
use crate::topology::loopuse::notify_ref;
use crate::topology::topology_traverser::Marks;
use crate::topology::{Brep, BrepId, FaceId};
use std::collections::BTreeMap;

use super::OP;

/// The notification namespace of the engine `SmMerge::SmMerge` builds.
const NAMESPACE: u32 = 0;

fn invalid(reason: &'static str) -> SolidError {
    SolidError::invalid_input(OP, reason)
}

/// A constructed `SmMerge` and the operands it holds (`m_vTI.m_pBrep`, `m_pOther`).
#[derive(Debug)]
pub(super) struct Session {
    primary: Option<BrepId>,
    other: Option<BrepId>,
    engine: MergeEngine,
    /// Face B-reps `PiecewiseMerge` made; released with the merge.
    pub(super) temporaries: Vec<BrepId>,
}

impl Session {
    /// `SmMerge(context, pBrep, pOtherBrep, dApproxTol3d, dAngleTolRadians)`.
    pub(super) fn new(
        k: &Kernel,
        breps: &[Option<Brep>],
        destination: BrepId,
        source: BrepId,
        tolerance: f64,
        angle: f64,
    ) -> Result<Self> {
        if destination == source {
            return Err(invalid("aliased merge operands"));
        }
        let merge = MergeState::with_breps(
            k,
            (slot(breps, destination)?, MergeOperand::Primary),
            Some((slot(breps, source)?, MergeOperand::Other)),
            tolerance,
            angle,
            None,
        )?;
        Ok(Self {
            primary: Some(destination),
            other: Some(source),
            engine: MergeEngine::from_merge(&merge)?,
            temporaries: Vec::new(),
        })
    }

    /// The operands, destination first.
    pub(super) fn operands(&self) -> Result<(BrepId, BrepId)> {
        Ok((
            self.primary.ok_or_else(|| SolidError::null_pointer(OP))?,
            self.other.ok_or_else(|| SolidError::null_pointer(OP))?,
        ))
    }

    /// The canonical mate store.
    pub(super) fn relations(&mut self) -> &mut crate::boolean::topology_intersector::p1::Relations {
        &mut self.engine.intersector.merge.intersector
    }
}

fn slot(breps: &[Option<Brep>], id: BrepId) -> Result<&Brep> {
    breps
        .get(id.index() as usize)
        .and_then(Option::as_ref)
        .ok_or_else(|| SolidError::null_pointer(OP))
}

/// Two distinct B-reps of the model, mutably, in argument order.
fn pair_mut(breps: &mut [Option<Brep>], a: BrepId, b: BrepId) -> Result<(&mut Brep, &mut Brep)> {
    let ai = a.index() as usize;
    let bi = b.index() as usize;
    if ai == bi {
        return Err(invalid("aliased merge operands"));
    }
    let (left, right) = breps
        .split_at_mut_checked(ai.max(bi))
        .ok_or_else(|| SolidError::null_pointer(OP))?;
    let (x, y) = if ai < bi {
        (left.get_mut(ai), right.first_mut())
    } else {
        (right.first_mut(), left.get_mut(bi))
    };
    Ok((
        x.and_then(Option::as_mut)
            .ok_or_else(|| SolidError::null_pointer(OP))?,
        y.and_then(Option::as_mut)
            .ok_or_else(|| SolidError::null_pointer(OP))?,
    ))
}

fn face_key(other: bool, face: FaceId) -> crate::geom::attribute::ObjectKey {
    notification_key(NAMESPACE, other, EntityRef::Face(face)).key
}

/// Rebinds the geometry `b`'s edges and faces own under one of their own identities
/// (the model's [`notify_ref`], the merge engine's import label [`arena_identity`]
/// or a merge stage's operand key) to `to`'s identity for that edge or face. Shared
/// geometry (any other owner) is left alone.
fn rebind_owners(b: &mut Brep, to: impl Fn(EntityRef) -> ObjectRef) -> Result<()> {
    let own = own_identities;
    let edges: Vec<_> = b.edges().ids().collect();
    for edge in edges {
        let e = EntityRef::Edge(edge);
        let new_owner = to(e);
        let rebound = b.edge_mut(edge)?.curve.as_mut().is_some_and(|curve| {
            let matched = curve
                .owner()
                .is_some_and(|o| own(e, CacheObject::Edge(edge)).contains(&o));
            if matched {
                curve.set_owner(Some(new_owner), None);
            }
            matched
        });
        if rebound {
            b.set_geometry_owner_key(e, Some(new_owner));
        }
    }
    let faces: Vec<_> = b.faces().ids().collect();
    for face in faces {
        let e = EntityRef::Face(face);
        let new_owner = to(e);
        let rebound = b.face_mut(face)?.surface.as_mut().is_some_and(|surface| {
            let matched = surface
                .owner()
                .is_some_and(|o| own(e, CacheObject::Face(face)).contains(&o));
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

/// The surface identity the merge engine sees for `face`'s surface once its owner is
/// rebound ([`rebind_owners`] to [`engine_owner`]): `PiecewiseMerge`'s subset is taken
/// before the stages rebind the operands.
fn engine_surface_identity(b: &Brep, face: FaceId) -> Result<ObjectRef> {
    let identity = KernelManifoldIntersector::surface_identity(b, face)?;
    let e = EntityRef::Face(face);
    Ok(
        if own_identities(e, CacheObject::Face(face)).contains(&identity) {
            engine_owner(e)
        } else {
            identity
        },
    )
}

/// The identities an edge or face of the model owns its geometry under.
fn own_identities(e: EntityRef, cache: CacheObject) -> [ObjectRef; 4] {
    [
        notify_ref(e),
        arena_identity(cache),
        notification_key(NAMESPACE, false, e),
        notification_key(NAMESPACE, true, e),
    ]
}

/// The engine's import label of an edge's or face's geometry.
fn engine_owner(e: EntityRef) -> ObjectRef {
    match e {
        EntityRef::Edge(x) => arena_identity(CacheObject::Edge(x)),
        EntityRef::Face(x) => arena_identity(CacheObject::Face(x)),
        other => notify_ref(other),
    }
}

/// Removes every `SM_AI_OFFSETMAP` attribute from `b`'s faces under `other`'s keys.
fn clear_offset_maps(attributes: &mut AttributeStore, b: &Brep, other: bool) -> Result<()> {
    for (face, _) in b.faces().iter() {
        let key = face_key(other, face);
        while let Some(h) = attributes.find_attribute(key, ids::OFFSETMAP) {
            if !attributes.is_live(h) {
                break;
            }
            attributes.remove_attribute(key, h, false)?;
        }
    }
    Ok(())
}

/// The offset maps of one merge stage, placed on the operands' face identities.
struct StageMaps {
    table: Vec<OffsetMapAttribute>,
}

impl StageMaps {
    /// Moves the model's offset maps of `id` (B-rep `b`, operand `other`) onto its
    /// face identities in `attributes`.
    fn place(
        &mut self,
        attributes: &mut AttributeStore,
        maps: &mut BTreeMap<FaceRef, OffsetMapAttribute>,
        id: BrepId,
        b: &Brep,
        other: bool,
    ) -> Result<()> {
        clear_offset_maps(attributes, b, other)?;
        let faces: Vec<FaceRef> = maps
            .range((id, FaceId::from_index(0))..)
            .map(|(f, _)| *f)
            .take_while(|f| f.0 == id)
            .collect();
        for face in faces {
            let Some(map) = maps.remove(&face) else {
                continue;
            };
            let index =
                u64::try_from(self.table.len()).map_err(|_| invalid("too many offset maps"))?;
            self.table.push(map);
            let h = attributes.insert(Attribute::pointer(ids::OFFSETMAP, index, Behavior::Copy))?;
            attributes.add_attribute(face_key(other, face.1), h)?;
        }
        Ok(())
    }

    /// Reads the offset maps of the surviving `id` (B-rep `b`, operand `other`) back
    /// into the model and removes them from `attributes`.
    fn take_back(
        &self,
        attributes: &mut AttributeStore,
        maps: &mut BTreeMap<FaceRef, OffsetMapAttribute>,
        id: BrepId,
        b: &Brep,
        other: bool,
    ) -> Result<()> {
        for (face, _) in b.faces().iter() {
            let key = face_key(other, face);
            if let Some(h) = attributes.find_attribute(key, ids::OFFSETMAP) {
                if let Some(AttributeValue::Pointer(index)) =
                    attributes.get(h).map(Attribute::value)
                {
                    let map = usize::try_from(*index)
                        .ok()
                        .and_then(|i| self.table.get(i))
                        .ok_or_else(|| invalid("unknown offset map"))?;
                    maps.insert((id, face), *map);
                }
            }
        }
        clear_offset_maps(attributes, b, other)
    }
}

/// (not upstream) Starts a merge stage on the model's attributes: the operands'
/// lists are renamed to the engine's operand identities and the engine's own lists
/// (surface maps) join them, and the whole store becomes the engine's for the stage,
/// so the stage's notifications reach every attribute, sharing included.
fn begin_stage(
    model: &mut AttributeStore,
    engine: &mut MergeEngine,
    operands: [(BrepId, &mut Brep, bool); 2],
) -> Result<()> {
    for (id, body, other) in operands {
        super::attributes::adopt(model, id, body)?;
        super::attributes::enter(model, id, &|e| notification_key(NAMESPACE, other, e))?;
    }
    let internal = core::mem::take(&mut engine.attributes);
    model.absorb(internal, &Ok)?;
    core::mem::swap(model, &mut engine.attributes);
    Ok(())
}

/// (not upstream) Ends [`begin_stage`]: the engine keeps its own lists, the model gets
/// the objects' lists back under its names (those of objects the stage deleted, or of
/// a consumed operand, `None`, are cleared).
fn end_stage(
    model: &mut AttributeStore,
    engine: &mut MergeEngine,
    operands: [(BrepId, Option<&Brep>, bool); 2],
) -> Result<()> {
    core::mem::swap(model, &mut engine.attributes);
    let mut internal = model.clone();
    internal.retain_rekeyed(&|key| (!super::attributes::is_object_key(key)).then_some(key))?;
    engine.attributes = internal;
    model.retain_rekeyed(&|key| super::attributes::is_object_key(key).then_some(key))?;
    for (id, body, other) in operands {
        super::attributes::leave(model, id, body, &|key| {
            super::attributes::engine_entity(other, key)
        })?;
    }
    Ok(())
}

/// The operand access of one merge call over the model's slots.
pub(super) struct Driver<'a> {
    pub(super) breps: &'a mut Vec<Option<Brep>>,
    pub(super) marks: &'a mut Vec<Marks>,
    pub(super) offset_maps: &'a mut BTreeMap<FaceRef, OffsetMapAttribute>,
    /// The model's attributes (`model/attributes.rs`).
    pub(super) attributes: &'a mut AttributeStore,
    pub(super) session: &'a mut Session,
}

impl Driver<'_> {
    fn brep(&self, id: BrepId) -> Result<&Brep> {
        slot(self.breps, id)
    }

    /// Runs `action` on the two operands with their offset maps placed on the
    /// engine's attribute store and their notification records seeded.
    fn with_pair<T>(
        &mut self,
        k: &mut Kernel,
        action: impl FnOnce(
            &mut BooleanKernels<'_>,
            &mut Brep,
            &mut Brep,
            &mut KernelManifoldIntersector,
            &mut ManifoldState,
            &mut AttributeStore,
        ) -> Result<T>,
    ) -> Result<T> {
        let (pa, pb) = self.session.operands()?;
        let (a, b) = pair_mut(self.breps, pa, pb)?;
        let engine = &mut self.session.engine;
        // (not upstream) the model's attributes ride the stage's notifications
        begin_stage(
            self.attributes,
            engine,
            [(pa, &mut *a, false), (pb, &mut *b, true)],
        )?;
        let mut stage = StageMaps { table: Vec::new() };
        stage.place(&mut engine.attributes, self.offset_maps, pa, a, false)?;
        stage.place(&mut engine.attributes, self.offset_maps, pb, b, true)?;
        rebind_owners(a, engine_owner)?;
        rebind_owners(b, engine_owner)?;
        crate::topology::brep::p3::seed_records(&mut engine.intersector.records, a, &|e| {
            notification_key(NAMESPACE, false, e)
        });
        crate::topology::brep::p3::seed_records(&mut engine.intersector.records, b, &|e| {
            notification_key(NAMESPACE, true, e)
        });
        let mut kernels = BooleanKernels {
            primary: k,
            other: None,
        };
        let result = action(
            &mut kernels,
            a,
            b,
            &mut engine.intersector,
            &mut engine.state,
            &mut engine.attributes,
        );
        rebind_owners(a, notify_ref)?;
        rebind_owners(b, notify_ref)?;
        stage.take_back(&mut engine.attributes, self.offset_maps, pa, a, false)?;
        stage.take_back(&mut engine.attributes, self.offset_maps, pb, b, true)?;
        end_stage(
            self.attributes,
            engine,
            [(pa, Some(&*a), false), (pb, Some(&*b), true)],
        )?;
        result
    }

    /// `CopyFaces(faces, destination, NULL, TRUE, this merge)` from the other operand
    /// into the destination; the merge's other-to-primary edge mates after it.
    pub(super) fn copy_faces(
        &mut self,
        k: &mut Kernel,
        source: BrepId,
        faces: &[FaceId],
        destination: BrepId,
    ) -> Result<()> {
        if self.session.operands()? != (destination, source) {
            return Err(invalid("copy operands do not match merge"));
        }
        self.with_pair(k, |kernels, a, b, i, _, attributes| {
            i.copy_faces(kernels, a, b, faces, attributes)
        })
    }

    /// `ManifoldBoolean(SM_BO_UNION, rpResult)` with the merge's own controls.
    pub(super) fn union(&mut self, k: &mut Kernel) -> Result<BrepId> {
        let (mut primary, mut other) = (self.session.primary, self.session.other);
        let mut manifold = self.manifold_flag();
        let mut lamina = self.session.engine.intersector.intersect_lamina_edges();
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

    /// The operand (`primary`, `other`) owning `surface` (by surface identity).
    fn operand_face(&self, operand: Option<BrepId>, surface: ObjectRef) -> Result<FaceId> {
        let b = self.brep(operand.ok_or_else(|| SolidError::null_pointer(OP))?)?;
        for (face, record) in b.faces().iter() {
            if record.surface.is_some() && engine_surface_identity(b, face)? == surface {
                return Ok(face);
            }
        }
        Err(invalid("not a surface of the merge operands"))
    }
}

impl BooleanDriver for Driver<'_> {
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
            .map(|id| self.brep(id).map(|b| (b, MergeOperand::Other)))
            .transpose()?;
        let merge = MergeState::with_breps(
            k,
            (self.brep(id)?, MergeOperand::Primary),
            source,
            tolerance,
            angle,
            options,
        )?;
        let engine = MergeEngine::from_merge(&merge)?;
        self.session.engine.intersector = engine.intersector;
        self.session.engine.state = engine.state;
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
        if a == b {
            return Err(invalid("aliased merge operands"));
        }
        let take = |breps: &mut Vec<Option<Brep>>, id: BrepId| {
            breps
                .get_mut(id.index() as usize)
                .and_then(Option::take)
                .ok_or_else(|| SolidError::null_pointer(OP))
        };
        let mut primary_body = take(self.breps, a)?;
        let mut other_body = match take(self.breps, b) {
            Ok(body) => body,
            Err(e) => {
                if let Some(s) = self.breps.get_mut(a.index() as usize) {
                    *s = Some(primary_body);
                }
                return Err(e);
            }
        };
        let engine = &mut self.session.engine;
        let mut stage = StageMaps { table: Vec::new() };
        let placed = rebind_owners(&mut primary_body, engine_owner)
            .and_then(|()| rebind_owners(&mut other_body, engine_owner));
        let placed = placed.and_then(|()| {
            stage
                .place(
                    &mut engine.attributes,
                    self.offset_maps,
                    a,
                    &primary_body,
                    false,
                )
                .and_then(|()| {
                    stage.place(
                        &mut engine.attributes,
                        self.offset_maps,
                        b,
                        &other_body,
                        true,
                    )
                })
        });
        // (not upstream) the model's attributes ride the Boolean's notifications
        let placed = placed.and_then(|()| {
            begin_stage(
                self.attributes,
                engine,
                [(a, &mut primary_body, false), (b, &mut other_body, true)],
            )
        });
        let begun = placed.is_ok();
        let mut operands = (Some(primary_body), Some(other_body));
        let result = placed.and_then(|()| {
            engine.intersector.set_intersect_lamina_edges(*lamina);
            let mut kernels = BooleanKernels {
                primary: k,
                other: None,
            };
            engine.intersector.manifold_boolean(
                &mut kernels,
                &mut operands.0,
                &mut operands.1,
                &mut engine.state,
                &mut engine.attributes,
                *manifold,
                operation,
                lock,
            )
        });
        // Survivors return to their slots, including after an error; a consumed
        // operand's handle no longer resolves.
        let mut restored = Ok(());
        let mut survivors = (operands.0.take(), operands.1.take());
        for (id, body, is_other) in [(a, &mut survivors.0, false), (b, &mut survivors.1, true)] {
            if let Some(body) = body {
                let rebound = rebind_owners(body, notify_ref);
                if restored.is_ok() {
                    restored = rebound;
                }
                let back =
                    stage.take_back(&mut engine.attributes, self.offset_maps, id, body, is_other);
                if restored.is_ok() {
                    restored = back;
                }
            }
        }
        let returned = if begun {
            end_stage(
                self.attributes,
                engine,
                [
                    (a, survivors.0.as_ref(), false),
                    (b, survivors.1.as_ref(), true),
                ],
            )
        } else {
            Ok(())
        };
        if restored.is_ok() {
            restored = returned;
        }
        for (id, body) in [(a, survivors.0), (b, survivors.1)] {
            if let Some(s) = self.breps.get_mut(id.index() as usize) {
                *s = body;
            }
        }
        if self.brep(a).is_err() {
            *primary = None;
        }
        if self.brep(b).is_err() {
            *other = None;
        }
        let assigned = result?;
        restored?;
        Ok(if assigned { *primary } else { None })
    }
}

impl PiecewiseAccess for Driver<'_> {
    type Surface = ObjectRef;

    fn operands(&self) -> (Option<BrepId>, Option<BrepId>) {
        (self.session.primary, self.session.other)
    }
    fn set_operands(&mut self, primary: Option<BrepId>, other: Option<BrepId>) {
        // Arena ids repeat in another B-rep; the traversal caches cannot follow them.
        if self.session.primary != primary {
            self.session.engine.state.primary_marks = Marks::default();
        }
        if self.session.other != other {
            self.session.engine.state.other_marks = Marks::default();
        }
        self.session.primary = primary;
        self.session.other = other;
    }
    fn surfaces(&self, id: BrepId) -> Result<Vec<ObjectRef>> {
        let b = self.brep(id)?;
        crate::topology::brep::p1::faces(b, None)?
            .into_iter()
            .filter_map(|f| match b.face(f) {
                Ok(face) if face.surface.is_none() => None,
                _ => Some(engine_surface_identity(b, f)),
            })
            .collect()
    }
    fn intersect_insert_relate(&mut self, k: &mut Kernel) -> Result<()> {
        self.with_pair(k, |kernels, a, b, i, _, attributes| {
            i.intersect_insert_relate(kernels, a, b, attributes)
        })
    }
    fn clear_relationships(&mut self) -> Result<()> {
        self.session.engine.intersector.clear_relationships()
    }
    fn set_subset(&mut self, surfaces: &[ObjectRef]) {
        self.session.engine.intersector.set_surface_subset(surfaces);
    }
    fn surface_map(&mut self, surface: ObjectRef, only_if_missing: bool) -> Result<()> {
        let attributes = &mut self.session.engine.attributes;
        if !only_if_missing
            || attributes
                .find_attribute(surface.key, ids::SURFACEMAP)
                .is_none()
        {
            let h =
                attributes.insert(Attribute::pointer(ids::SURFACEMAP, 0, Behavior::Reference))?;
            attributes.add_attribute(surface.key, h)?;
        }
        Ok(())
    }
    fn surface_map_users(&self, surface: ObjectRef) -> Result<Option<Vec<ObjectRef>>> {
        let attributes = &self.session.engine.attributes;
        attributes
            .find_attribute(surface.key, ids::SURFACEMAP)
            .map(|h| {
                attributes.users(h).map(|users| {
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
        let tolerance = self.brep(source)?.tolerance();
        let index = u32::try_from(self.breps.len()).map_err(|_| invalid("too many B-reps"))?;
        let mut body = crate::topology::brep::p3::new_brep(
            k,
            crate::geom::tol::USE_DEFAULT,
            crate::geom::tol::USE_DEFAULT,
        )?;
        crate::topology::brep::p2::set_tolerance(k, &mut body, tolerance, true, true)?;
        body.set_editing_enabled(true);
        self.breps.push(Some(body));
        self.marks.push(Marks::default());
        let id = BrepId::from_index(index);
        self.session.temporaries.push(id);
        Ok(id)
    }
    fn manifold_flag(&self) -> bool {
        self.session
            .engine
            .intersector
            .merge
            .manifold_boolean
            .unwrap_or(false)
    }
    fn set_controls(&mut self, manifold: bool, wires: bool, lamina: u64) {
        let i = &mut self.session.engine.intersector;
        i.merge.manifold_boolean = Some(manifold);
        i.set_intersect_wire_edges(wires);
        i.set_intersect_lamina_edges(lamina);
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
        BooleanKernels {
            primary: k,
            other: None,
        }
        .mark_action(|contexts| lock.release(contexts))
    }
    fn register_empty_ssi(&mut self, skip: ObjectRef, user: ObjectRef) -> Result<()> {
        let af = self.operand_face(self.session.primary, skip)?;
        let bf = self.operand_face(self.session.other, user)?;
        self.session.engine.intersector.register_empty_ssi(af, bf);
        Ok(())
    }
    fn clear_existing_ssi(&mut self) -> Result<()> {
        self.session.engine.intersector.clear_existing_ssi();
        Ok(())
    }
    fn notify_pre_edit(&mut self, k: &mut Kernel, primary: BrepId) -> Result<()> {
        let b = self
            .breps
            .get_mut(primary.index() as usize)
            .and_then(Option::as_mut)
            .ok_or_else(|| SolidError::null_pointer(OP))?;
        let mut state = ArenaNotifyState::new(b);
        let mut env = state.env(Some(&mut k.context));
        crate::topology::brep::p2::notify(
            b,
            &mut env,
            None,
            NotifyOperation::PreEdit,
            [None, None, None],
        )
    }
    fn delete_other(&mut self, _k: &mut Kernel) -> Result<()> {
        if let Some(id) = self.session.other.take() {
            if let Some(s) = self.breps.get_mut(id.index() as usize) {
                *s = None;
            }
            // its objects' destructors clear their lists
            super::attributes::release(self.attributes, id)?;
        }
        Ok(())
    }
}
