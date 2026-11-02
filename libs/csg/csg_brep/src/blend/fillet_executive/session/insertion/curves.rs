//! Actual operation-owned curve copies. Handles never impersonate topology.

use crate::curve::curve::Curve;
use crate::geom::attribute::AttributeStore;
use crate::geom::context::Context;
use crate::geom::object::{NotifyOperation, ObjectRef};
use crate::{Result, SolidError};

/// An issued identity and its live operation-owned Curve slot.
/// Valid only while the issuing store and object remain alive.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct CurveBinding {
    slot: usize,
    identity: ObjectRef,
    // Last only to keep valid-store ordering determined by slot and issued identity.
    // Equality checks the actual live object; expired-store handles are invalid.
    instance: usize,
}

impl std::fmt::Debug for CurveBinding {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CurveBinding")
            .field("slot", &self.slot)
            .field("identity", &self.identity)
            .finish_non_exhaustive()
    }
}

/// Curves retained for the whole insertion operation, including partial failures.
#[derive(Debug)]
struct OwnedCurve {
    // Stable instance identity while the operation's storage vector grows; `None`
    // once destroyed (the slot is never reused, so stale bindings stay invalid).
    curve: Option<Box<Curve>>,
}
/// Curves retained for the whole insertion operation, including partial failures.
#[derive(Debug, Default)]
pub struct InsertionCurves {
    curves: Vec<OwnedCurve>,
}
impl InsertionCurves {
    /// Move an actual curve copy into operation ownership and issue its identity.
    /// # Errors
    /// Identity-domain issuance failure; the curve is retained on this failure too.
    pub fn insert(&mut self, context: &mut Context, curve: Curve) -> Result<CurveBinding> {
        let slot = self.curves.len();
        self.curves.push(OwnedCurve {
            curve: Some(Box::new(curve)),
        });
        let curve = self
            .curves
            .get_mut(slot)
            .and_then(|owned| owned.curve.as_deref_mut())
            .ok_or_else(|| {
                SolidError::invalid_input(
                    "blend::fillet_executive::InsertionCurves",
                    "inserted curve slot is absent",
                )
            })?;
        let type_code = curve.type_code();
        let key = context.geometry_key(curve.notify_identity_slot())?;
        Ok(CurveBinding {
            slot,
            identity: ObjectRef { type_code, key },
            instance: std::ptr::from_ref(curve).addr(),
        })
    }
    /// Borrow the actual retained object, rejecting a foreign/replaced binding.
    #[must_use]
    pub fn get(&self, binding: CurveBinding) -> Option<&Curve> {
        let curve = self.curves.get(binding.slot)?.curve.as_deref()?;
        (std::ptr::from_ref(curve).addr() == binding.instance
            && curve.notify_key() == Some(binding.identity.key)
            && curve.type_code() == binding.identity.type_code)
            .then_some(curve)
    }
    /// Mutably borrow the same retained object after identity validation.
    pub fn get_mut(&mut self, binding: CurveBinding) -> Option<&mut Curve> {
        self.get(binding)?;
        self.curves.get_mut(binding.slot)?.curve.as_deref_mut()
    }
    /// The live retained curves, in slot order.
    pub fn live(&self) -> impl Iterator<Item = &Curve> {
        self.curves
            .iter()
            .filter_map(|owned| owned.curve.as_deref())
    }
    /// `delete pCurve` of an operation-owned temporary copy (`SmObjDelete`):
    /// `~SmCurve`'s `Notify(SM_NO_DESTRUCTION, this, NULL, NULL)` (the curve's cache
    /// memo goes with it; attribute handlers and context callbacks run, the status is
    /// ignored as in a destructor), then `~SmAObject`'s `ClearAttributes`. The slot
    /// stays empty, so the binding and any copy of it no longer resolve.
    /// # Errors
    /// A binding from a different store or an already destroyed curve; the
    /// [`SolidError::Unported`] / representation failures of the notification and
    /// those of `ClearAttributes`.
    pub fn destroy(
        &mut self,
        context: &mut Context,
        attributes: &mut AttributeStore,
        binding: CurveBinding,
    ) -> Result<()> {
        self.checked(binding)?;
        let this = binding.identity;
        crate::error::ignore_status(crate::geom::a_object::notify(
            attributes,
            Some(context),
            NotifyOperation::Destruction,
            this,
            None,
            None,
            None,
        ))?;
        if let Some(owned) = self.curves.get_mut(binding.slot) {
            owned.curve = None;
        }
        attributes.clear_attributes(this.key)
    }
    /// Resolve a binding with an explicit failure at a provider capability boundary.
    /// # Errors
    /// A binding from a different store or a replaced object.
    pub fn checked(&self, binding: CurveBinding) -> Result<&Curve> {
        self.get(binding).ok_or_else(|| {
            SolidError::invalid_input(
                "blend::fillet_executive::InsertionCurves",
                "foreign or replaced curve binding",
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::line::Line;
    use crate::math::Point3;
    use crate::Kernel;

    #[test]
    fn independent_context_numeric_identity_collision_does_not_alias_live_objects() {
        let mut a = Kernel::default();
        let mut b = Kernel::default();
        let mut first = InsertionCurves::default();
        let mut second = InsertionCurves::default();
        let curve = || {
            Curve::from(Line::from_points(
                Point3::new(0.0, 0.0, 0.0),
                Point3::new(1.0, 0.0, 0.0),
                3,
            ))
        };
        let one = first.insert(&mut a.context, curve()).unwrap();
        let two = second.insert(&mut b.context, curve()).unwrap();
        assert_eq!(one.slot, two.slot);
        assert_eq!(one.identity, two.identity);
        assert_ne!(one.instance, two.instance);
        assert!(first.get(two).is_none());
        assert!(second.get(one).is_none());
        assert!(first.get(one).is_some());
        assert!(second.get(two).is_some());
        // Growing the owner vector must not invalidate an existing live object.
        for _ in 0..64 {
            first.insert(&mut a.context, curve()).unwrap();
        }
        assert!(first.get(one).is_some());
    }

    #[test]
    fn destroyed_temporary_curve_notifies_clears_attributes_and_stops_resolving() {
        use crate::geom::attribute::{Attribute, Behavior};
        use crate::geom::context::NotifyCallback;
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
        let events = Arc::new(Mutex::new(Vec::new()));
        k.context
            .set_user_notify_callback(Some(Box::new(Capture(events.clone()))));
        let mut store = InsertionCurves::default();
        let mut attributes = AttributeStore::default();
        let line = |x: f64| {
            Curve::from(Line::from_points(
                Point3::new(x, 0.0, 0.0),
                Point3::new(x + 1.0, 0.0, 0.0),
                3,
            ))
        };
        let kept = store.insert(&mut k.context, line(0.0)).unwrap();
        let temporary = store.insert(&mut k.context, line(5.0)).unwrap();
        let attribute = attributes
            .insert(Attribute::long(7, 1, Behavior::StandaloneCopy))
            .unwrap();
        attributes
            .add_attribute(temporary.identity.key, attribute)
            .unwrap();
        store
            .destroy(&mut k.context, &mut attributes, temporary)
            .unwrap();
        // ~SmCurve: Notify(SM_NO_DESTRUCTION, this, NULL, NULL) reaches the context.
        assert!(events
            .lock()
            .unwrap()
            .contains(&(NotifyOperation::Destruction, temporary.identity)));
        // ~SmAObject: ClearAttributes.
        assert!(attributes.attributes(temporary.identity.key).is_empty());
        assert!(store.get(temporary).is_none());
        assert!(store.checked(temporary).is_err());
        assert!(store
            .destroy(&mut k.context, &mut attributes, temporary)
            .is_err());
        // The other copy and later insertions keep their own slots.
        assert!(store.get(kept).is_some());
        let later = store.insert(&mut k.context, line(9.0)).unwrap();
        assert_ne!(later.slot, temporary.slot);
        assert_eq!(store.live().count(), 2);
    }
}
