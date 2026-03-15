use crate::geometry::ColliderHandle;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
/// A pair of collider handles.
pub struct ColliderPair {
    /// The handle of the first collider involved in this pair.
    pub collider1: ColliderHandle,
    /// The handle of the second ocllider involved in this pair.
    pub collider2: ColliderHandle,
}

impl ColliderPair {
    /// Creates a new pair of collider handles.
    pub fn new(collider1: ColliderHandle, collider2: ColliderHandle) -> Self {
        ColliderPair {
            collider1,
            collider2,
        }
    }

    /// Swaps the two collider handles in `self`.
    pub fn swap(self) -> Self {
        Self::new(self.collider2, self.collider1)
    }

    /// Constructs a pair of artificial handles that are not guaranteed to be valid..
    pub fn zero() -> Self {
        Self {
            collider1: ColliderHandle::from_raw_parts(0, 0),
            collider2: ColliderHandle::from_raw_parts(0, 0),
        }
    }
}

impl Default for ColliderPair {
    fn default() -> Self {
        ColliderPair::zero()
    }
}

/// An event emitted by the broad-phase.
pub enum BroadPhasePairEvent {
    /// A potential new collision pair has been detected by the broad-phase.
    AddPair(ColliderPair),
    /// The two colliders are guaranteed not to touch any more.
    DeletePair(ColliderPair),
}
