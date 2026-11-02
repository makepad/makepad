//! Caller identities for the production manifold Boolean adapter.
//!
//! Upstream: `source/SMLib/src/SmMerge.cpp`, `source/SMLib/inc/SmMerge.h`.
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`ManifoldNotificationKeys`] | operand object pointers and region source-list pointers |
//!
//! Deviations: borrowed identity callbacks replace pointers. Source-list handles
//! are caller-reserved, not guessed from topology keys. No recursion is introduced.

use crate::geom::object::ObjectRef;
use crate::topology::{EdgeId, RegionId};
use crate::EntityRef;

/// Identities shared with the caller's topology records and attribute store.
///
/// Each callback must be stable for the operation's lifetime, cover newly created
/// entities, and distinguish both arenas and all object classes. Curve identities
/// follow the caller's geometry ownership convention. `source_lists` reserves two
/// distinct non-object handles (`type_code == 0`) per primary region, disjoint from
/// every object and other source-list key in the shared stores. They identify the
/// primary-source list and other-source list, respectively, in `RegPropagation`.
/// Swap the operand callbacks when swapping operands; the result uses primary keys.
#[derive(Clone, Copy)]
pub struct ManifoldNotificationKeys<'a> {
    /// Primary topology identities.
    pub primary: &'a dyn Fn(EntityRef) -> ObjectRef,
    /// Other topology identities.
    pub other: &'a dyn Fn(EntityRef) -> ObjectRef,
    /// Primary edge-curve identities (edge id, curve type).
    pub primary_curve: &'a dyn Fn(EdgeId, i64) -> crate::Result<ObjectRef>,
    /// Other edge-curve identities (edge id, curve type).
    pub other_curve: &'a dyn Fn(EdgeId, i64) -> crate::Result<ObjectRef>,
    /// Reserved primary-region source-list handles, in primary/other order.
    pub source_lists: &'a dyn Fn(RegionId) -> crate::Result<[ObjectRef; 2]>,
    /// Whether a curve the merge allocates at an edge split takes its actual
    /// Curve identity from the edit's notification context at construction
    /// (`p9::CurveIdentity::Actual`) instead of the curve callbacks.
    pub actual_curve_identities: bool,
}

impl core::fmt::Debug for ManifoldNotificationKeys<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ManifoldNotificationKeys")
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) enum NotificationKeys<'a> {
    Namespace(u32),
    Caller(ManifoldNotificationKeys<'a>),
}

impl NotificationKeys<'_> {
    pub(super) fn object(self, other: bool, entity: EntityRef) -> ObjectRef {
        match self {
            Self::Namespace(namespace) => {
                super::kernel_adapter::notification_key(namespace, other, entity)
            }
            Self::Caller(keys) => (if other { keys.other } else { keys.primary })(entity),
        }
    }

    /// Whether new split curves take their actual identity at construction.
    pub(super) const fn actual_curve_identities(self) -> bool {
        match self {
            Self::Namespace(_) => false,
            Self::Caller(keys) => keys.actual_curve_identities,
        }
    }

    pub(super) fn curve(
        self,
        other: bool,
        edge: EdgeId,
        type_code: i64,
    ) -> crate::Result<ObjectRef> {
        match self {
            Self::Namespace(_) => Ok(ObjectRef {
                type_code,
                ..self.object(other, EntityRef::Edge(edge))
            }),
            Self::Caller(keys) => (if other {
                keys.other_curve
            } else {
                keys.primary_curve
            })(edge, type_code),
        }
    }
}
