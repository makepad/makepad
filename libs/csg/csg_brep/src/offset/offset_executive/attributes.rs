//! (not upstream) The B-rep-owned attributes of an offset or shell, and the
//! notification identities of the wrappers that need no caller-built
//! [`OffsetNotifications`].
//!
//! Upstream: none of its own. Upstream's attributes live on the objects
//! (`SmAObject`), so `SmOffsetExecutive::OffsetBrep` / `ShellBrep` and
//! `SmApiShellBrepFull` / `SmApiOffsetBrepFull` carry them through the Copy, Split,
//! Merge and Delete notifications they send (e.g. the COPY of each offset face's
//! source at `SmOffsetExecutive.cpp:1237`).
//!
//! | Rust | Upstream |
//! |------|----------|
//! | `seed_attributes` (crate) | the source objects' own attribute lists |
//! | `write_back_attributes` (crate) | the result objects' own attribute lists |
//! | `SimpleIdentities` (crate) | the addresses of the objects of one offset call |
//!
//! Deviations: here each [`Brep`] owns its attributes ([`Brep::add_attribute`]), keyed
//! by B-rep-local identities, while an offset notifies its caller's stores under the
//! caller's identities. The entry points copy the source's attributes into
//! [`OffsetNotifications::attributes`] under [`OffsetNotifications::original_key_of`]
//! before they run, and hand the lists that store holds for the returned B-rep's live
//! objects back to it after (the source keeps its own). A reference attribute on
//! objects of the source and of the result becomes one attribute per B-rep, and a
//! standalone attribute without users survives in the result's store too. No
//! recursion.

use std::collections::BTreeMap;

use super::p2::OffsetNotifications;
use crate::curve::curve::Curve;
use crate::error::{EntityRef, Result, SolidError};
use crate::geom::a_object::BREP_TYPE;
use crate::geom::attribute::{ids, AttributeStore, ObjectKey};
use crate::geom::object::ObjectRef;
use crate::sweep::topology_sweep::store::SweepIdentities;
use crate::topology::loopuse;
use crate::topology::topology::{NotifyRecords, Topology};
use crate::topology::{Brep, BrepId, EdgeId, Region, RegionId, Shell};

const OP_SEED: &str = "offset::offset_executive::attributes::seed_attributes";

/// Every live object of `b` that can carry attributes, in arena order.
pub(crate) fn live_entities(b: &Brep) -> Vec<EntityRef> {
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

/// Copies `source`'s own attributes into `store` under `key_of`'s names of its
/// objects (sharing and user order kept); `source` keeps its own.
///
/// # Errors
/// [`SolidError::InvalidInput`] for an attribute list that names no object of the
/// B-rep (a corrupt store), or a store that cannot grow.
pub(crate) fn seed_attributes(
    source: &Brep,
    key_of: &dyn Fn(EntityRef) -> ObjectRef,
    store: &mut AttributeStore,
) -> Result<()> {
    store.absorb(source.attribute_store().clone(), &|key| {
        loopuse::entity_of_notify_key(key)
            .map(|e| key_of(e).key)
            .ok_or(SolidError::invalid_input(
                OP_SEED,
                "attribute list names no object",
            ))
    })
}

/// Gives `result` the attribute lists `store` holds for its live objects under
/// `key_of`'s names, replacing its own store (every other list is not the result's;
/// `store` itself is unchanged), less the merge's surface maps.
///
/// # Errors
/// None in practice.
pub(crate) fn write_back_attributes(
    result: &mut Brep,
    key_of: &dyn Fn(EntityRef) -> ObjectRef,
    store: &AttributeStore,
) -> Result<()> {
    let names: BTreeMap<ObjectKey, ObjectKey> = live_entities(result)
        .into_iter()
        .map(|e| (key_of(e).key, loopuse::notify_ref(e).key))
        .collect();
    let mut own = store.clone();
    own.retain_rekeyed(&|key| names.get(&key).copied())?;
    // a merge's `SM_AI_SURFACEMAP` belongs to the face's surface upstream
    // (`SmMerge.cpp:417`), which shares the face's identity here: no topology object
    // of the result carries it
    for &local in names.values() {
        while let Some(h) = own.find_attribute(local, ids::SURFACEMAP) {
            own.remove_attribute(local, h, false)?;
        }
    }
    result.set_attributes(own);
    Ok(())
}

/// Hands the attributes of an offset's returned B-rep back to it: an inset shell's
/// result is the original's arena (it carries `original_key`), under
/// [`OffsetNotifications::original_key_of`]; every other result is the result arena,
/// under [`OffsetNotifications::result_key_of`] (a merging `PiecewiseMerge`'s B-rep
/// takes the result identities when it replaces the offset temporary).
pub(crate) fn finish_attributes(
    result: &mut Brep,
    original_key: ObjectKey,
    notifications: &OffsetNotifications<'_>,
) -> Result<()> {
    let key_of = if result.key() == original_key {
        notifications.original_key_of
    } else {
        notifications.result_key_of
    };
    write_back_attributes(result, key_of, notifications.attributes)
}

// ---- the identities of the simple wrappers ---------------------------------------

/// The type code of `entity` and its arena index.
fn type_and_index(entity: EntityRef) -> (i64, u32) {
    match entity {
        EntityRef::Brep(id) => (BREP_TYPE, id.index()),
        EntityRef::Region(id) => (Region::type_code(), id.index()),
        EntityRef::Shell(id) => (Shell::type_code(), id.index()),
        EntityRef::Face(id) => (crate::topology::face::p2::TYPE, id.index()),
        // `SmFaceuse_TYPE`
        EntityRef::Faceuse(id) => (16022, id.index()),
        EntityRef::Loopuse(id) => (loopuse::TYPE, id.index()),
        EntityRef::Edge(id) => (crate::topology::edge::TYPE, id.index()),
        EntityRef::Edgeuse(id) => (crate::topology::edgeuse::p1::TYPE, id.index()),
        EntityRef::Vertex(id) => (crate::topology::vertex::TYPE, id.index()),
        EntityRef::Vertexuse(id) => (crate::topology::vertexuse::TYPE, id.index()),
        EntityRef::Loop(id) => (crate::topology::topology::TYPE, id.index()),
    }
}

/// A key in `space` (bits 56-62, below the kernel's reserved bit 63), with `handle`
/// (bits 46-55: a merge-store body or a curve / source-list class), `kind` (bits
/// 32-45: a type code or list index) and `index` (the low 32 bits).
const fn key(space: u64, handle: u64, kind: u64, index: u32) -> ObjectKey {
    ObjectKey(
        ((space & 0x7f) << 56) | ((handle & 0x3ff) << 46) | ((kind & 0x3fff) << 32) | index as u64,
    )
}

/// The original arena's objects.
const SPACE_ORIGINAL: u64 = 1;
/// The result arena's objects.
const SPACE_RESULT: u64 = 2;
/// `SmApiShellBrepFull`'s working copy.
const SPACE_COPY: u64 = 3;
/// The bodies of a merging `PiecewiseMerge` (the handle is the store's).
const SPACE_MERGE: u64 = 4;
/// Edge curves (handle: 0 original, 1 result, 2 copy; merge bodies use their own
/// space plus one).
const SPACE_CURVE: u64 = 5;
/// Region source lists (handle: 0 original, 1 result, 2 copy).
const SPACE_LISTS: u64 = 6;
/// Edge curves of the merge bodies.
const SPACE_MERGE_CURVE: u64 = 7;
/// Region source lists of the merge bodies.
const SPACE_MERGE_LISTS: u64 = 8;

fn topology_key(space: u64, handle: u64, entity: EntityRef) -> ObjectRef {
    let (type_code, index) = type_and_index(entity);
    ObjectRef {
        key: key(space, handle, type_code.unsigned_abs(), index),
        type_code,
    }
}

pub(crate) fn original_key(entity: EntityRef) -> ObjectRef {
    topology_key(SPACE_ORIGINAL, 0, entity)
}

pub(crate) fn result_key(entity: EntityRef) -> ObjectRef {
    topology_key(SPACE_RESULT, 0, entity)
}

pub(crate) fn copy_key(entity: EntityRef) -> ObjectRef {
    topology_key(SPACE_COPY, 0, entity)
}

fn curve_key(class: u64, edge: EdgeId, type_code: i64) -> ObjectRef {
    ObjectRef {
        key: key(SPACE_CURVE, class, type_code.unsigned_abs(), edge.index()),
        type_code,
    }
}

pub(crate) fn original_curve(edge: EdgeId, type_code: i64) -> ObjectRef {
    curve_key(0, edge, type_code)
}

pub(crate) fn result_curve(edge: EdgeId, type_code: i64) -> ObjectRef {
    curve_key(1, edge, type_code)
}

pub(crate) fn copy_curve(edge: EdgeId, type_code: i64) -> ObjectRef {
    curve_key(2, edge, type_code)
}

fn lists(class: u64, region: RegionId) -> [ObjectRef; 2] {
    [0_u64, 1].map(|list| ObjectRef {
        key: key(SPACE_LISTS, class, list, region.index()),
        type_code: 0,
    })
}

pub(crate) fn source_lists(result: bool, region: RegionId) -> [ObjectRef; 2] {
    lists(u64::from(result), region)
}

pub(crate) fn copy_source_lists(region: RegionId) -> [ObjectRef; 2] {
    lists(2, region)
}

/// The identities of a merging `PiecewiseMerge`'s bodies.
pub(crate) struct SimpleIdentities;

impl SweepIdentities for SimpleIdentities {
    fn topology(&self, brep: BrepId, entity: EntityRef) -> ObjectRef {
        topology_key(SPACE_MERGE, u64::from(brep.index()), entity)
    }
    fn owned_auxiliary_objects(&self, brep: BrepId, model: &Brep) -> Vec<ObjectKey> {
        model
            .edges()
            .iter()
            .filter_map(|(edge, e)| e.curve.as_ref().map(|c| self.curve(brep, edge, c).key))
            .collect()
    }
    fn curve(&self, brep: BrepId, edge: EdgeId, curve: &Curve) -> ObjectRef {
        let type_code = curve.type_code();
        ObjectRef {
            key: key(
                SPACE_MERGE_CURVE,
                u64::from(brep.index()),
                type_code.unsigned_abs(),
                edge.index(),
            ),
            type_code,
        }
    }
    fn new_curve(&self, brep: BrepId, edge: EdgeId, type_code: i64) -> Result<ObjectRef> {
        Ok(ObjectRef {
            key: key(
                SPACE_MERGE_CURVE,
                u64::from(brep.index()),
                type_code.unsigned_abs(),
                edge.index(),
            ),
            type_code,
        })
    }
    fn source_lists(&self, brep: BrepId, region: RegionId) -> Result<[ObjectRef; 2]> {
        Ok([0_u64, 1].map(|list| ObjectRef {
            key: key(
                SPACE_MERGE_LISTS,
                u64::from(brep.index()),
                list,
                region.index(),
            ),
            type_code: 0,
        }))
    }
}

/// The records of one simple-wrapper call: `source`'s objects under
/// [`original_key`] (and the B-rep under its own key), and the reserved result B-rep.
pub(crate) fn simple_records(source: &Brep) -> NotifyRecords {
    let mut records = NotifyRecords::new();
    records.insert(source.key(), Topology::new());
    records.insert(
        result_key(EntityRef::Brep(BrepId::from_index(0))).key,
        Topology::new(),
    );
    for e in live_entities(source) {
        if !matches!(e, EntityRef::Loop(_)) {
            records.insert(original_key(e).key, Topology::new());
        }
    }
    records
}
