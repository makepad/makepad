//! (not upstream) The model's attribute store: the `SmAObject` attributes of every
//! object of the model's B-reps, in one store, as upstream's attributes are shared by
//! the objects of all B-reps (a reference attribute keeps one instance when objects of
//! several B-reps use it).
//!
//! Upstream: none of its own (`SmAObject::m_pAttributes` lives on each object).
//!
//! | Rust | Upstream |
//! |------|----------|
//! | `adopt` / `write_back` (crate) | handing a B-rep's objects, with their attributes, to and from the model |
//! | `enter` / `leave` (crate) | an edit notifying the objects themselves |
//!
//! Deviations: a B-rep's objects are named in the store by `model_key` (a notification
//! namespace per model B-rep, above the namespaces an edit uses). An edit renames the
//! lists of the B-reps it touches to the identities its notifications use (`enter`),
//! in place, so every handle and every user list keeps its identity, and back after it
//! (`leave`), when the lists of objects the edit deleted are cleared, as upstream's
//! object destructors do. A B-rep's own attributes ([`Brep::add_attribute`]) join the
//! store when an edit first touches it (`adopt`) and return to it when it leaves the
//! model (`write_back`). No recursion.

use crate::boolean::merge::p4::{entity_of_notification_key, notification_key};
use crate::error::{EntityRef, Result};
use crate::geom::attribute::{AttributeStore, ObjectKey};
use crate::geom::object::ObjectRef;
use crate::topology::loopuse::{entity_of_notify_key, notify_ref};
use crate::topology::{Brep, BrepId};

/// The first notification namespace of the model's B-reps (namespace 0 is the merge
/// engine's, 1 the source B-rep's of a two-B-rep edit).
const MODEL_NAMESPACE: u32 = 2;
/// The largest notification namespace (`notification_key`'s 26-bit block).
const MAX_NAMESPACE: u32 = 0x03ff_ffff;
/// The namespace of [`source_ref`].
const SOURCE_NAMESPACE: u32 = 1;

/// The store key of object `e` of model B-rep `id` (`None` past the namespace range).
pub(super) fn model_key(id: BrepId, e: EntityRef) -> Option<ObjectKey> {
    let namespace = id
        .index()
        .checked_add(MODEL_NAMESPACE)
        .filter(|n| *n <= MAX_NAMESPACE)?;
    Some(notification_key(namespace, false, e).key)
}

/// The model B-rep and object [`model_key`] names `key`, if any.
fn model_entity(key: ObjectKey) -> Option<(BrepId, EntityRef)> {
    let namespace = u32::try_from(key.0 >> 37).ok()?;
    let index = namespace.checked_sub(MODEL_NAMESPACE)?;
    match entity_of_notification_key(namespace, key)? {
        (false, e) if !matches!(e, EntityRef::Brep(_)) => Some((BrepId::from_index(index), e)),
        _ => None,
    }
}

/// The identities a two-B-rep edit gives the source B-rep's objects: apart from the
/// destination's ([`notify_ref`]) and from the model's.
pub(super) fn source_ref(e: EntityRef) -> ObjectRef {
    notification_key(SOURCE_NAMESPACE, true, e)
}

/// The object [`notify_ref`] names `key` (not the B-rep itself).
pub(super) fn notify_entity(key: ObjectKey) -> Option<EntityRef> {
    entity_of_notify_key(key).filter(|e| !matches!(e, EntityRef::Brep(_)))
}

/// The object [`source_ref`] names `key`.
pub(super) fn source_entity(key: ObjectKey) -> Option<EntityRef> {
    match entity_of_notification_key(SOURCE_NAMESPACE, key)? {
        (true, e) if !matches!(e, EntityRef::Brep(_)) => Some(e),
        _ => None,
    }
}

/// The operand object the merge engine (namespace 0) names `key`, on side `other`.
pub(super) fn engine_entity(other: bool, key: ObjectKey) -> Option<EntityRef> {
    match entity_of_notification_key(0, key)? {
        (o, e) if o == other && !matches!(e, EntityRef::Brep(_)) => Some(e),
        _ => None,
    }
}

/// Whether `key` names an object of a model B-rep or of a merge operand.
pub(super) fn is_object_key(key: ObjectKey) -> bool {
    model_entity(key).is_some()
        || engine_entity(false, key).is_some()
        || engine_entity(true, key).is_some()
}

/// Moves `b`'s own attributes (keyed by [`notify_ref`]) into `store` under B-rep `id`'s
/// [`model_key`]s; `b` keeps none. A list naming no object of `b` is dropped.
pub(super) fn adopt(store: &mut AttributeStore, id: BrepId, b: &mut Brep) -> Result<()> {
    let mut own = b.take_attributes();
    own.retain_rekeyed(&|key| notify_entity(key).and_then(|e| model_key(id, e)))?;
    store.absorb(own, &Ok)
}

/// Renames, in place, the lists of B-rep `id`'s objects to `view`'s identities.
pub(super) fn enter(
    store: &mut AttributeStore,
    id: BrepId,
    view: &dyn Fn(EntityRef) -> ObjectRef,
) -> Result<()> {
    store.retain_rekeyed(&|key| match model_entity(key) {
        Some((b, e)) if b == id => Some(view(e).key),
        _ => Some(key),
    })
}

/// Renames, in place, the lists `decode` recognises (an edit's identities of B-rep
/// `id`'s objects) back to [`model_key`]s; the lists of objects that are no longer
/// live objects of `b` (or of a B-rep that is gone, `None`) are cleared.
pub(super) fn leave(
    store: &mut AttributeStore,
    id: BrepId,
    b: Option<&Brep>,
    decode: &dyn Fn(ObjectKey) -> Option<EntityRef>,
) -> Result<()> {
    store.retain_rekeyed(&|key| match decode(key) {
        Some(e) => b
            .filter(|b| b.is_live_entity(e))
            .and_then(|_| model_key(id, e)),
        None => Some(key),
    })
}

/// Gives `b` (model B-rep `id`, leaving the model) the lists of its live objects,
/// keyed by [`notify_ref`], and removes every list of B-rep `id` from `store`.
pub(super) fn write_back(store: &mut AttributeStore, id: BrepId, b: &mut Brep) -> Result<()> {
    adopt(store, id, b)?;
    let mut own = store.clone();
    let live = &*b;
    own.retain_rekeyed(&|key| match model_entity(key) {
        Some((x, e)) if x == id && live.is_live_entity(e) => Some(notify_ref(e).key),
        _ => None,
    })?;
    release(store, id)?;
    b.set_attributes(own);
    Ok(())
}

/// Clears every list of B-rep `id`'s objects (the B-rep is deleted).
pub(super) fn release(store: &mut AttributeStore, id: BrepId) -> Result<()> {
    store.retain_rekeyed(&|key| match model_entity(key) {
        Some((x, _)) if x == id => None,
        _ => Some(key),
    })
}
