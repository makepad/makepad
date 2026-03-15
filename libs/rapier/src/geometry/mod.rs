//! Structures related to geometry: colliders, shapes, etc.

pub use self::broad_phase_bvh::{BroadPhaseBvh, BvhOptimizationStrategy};
pub use self::broad_phase_pair_event::{BroadPhasePairEvent, ColliderPair};
pub use self::collider::{Collider, ColliderBuilder};
pub use self::collider_components::*;
pub use self::collider_set::{ColliderSet, ModifiedColliders};
pub use self::contact_pair::{
    ContactData, ContactManifoldData, ContactPair, IntersectionPair, SimdSolverContact,
    SolverContact, SolverFlags,
};
pub use self::interaction_graph::{
    ColliderGraphIndex, InteractionGraph, RigidBodyGraphIndex, TemporaryInteractionIndex,
};
pub use self::interaction_groups::{Group, InteractionGroups, InteractionTestMode};
pub use self::mesh_converter::{MeshConverter, MeshConverterError};
pub use self::narrow_phase::NarrowPhase;
pub use parry::utils::Array2;

pub use parry::bounding_volume::BoundingVolume;
pub use parry::partitioning::{Bvh, BvhBuildStrategy};
pub use parry::query::{PointQuery, PointQueryWithLocation, RayCast, TrackedContact};
pub use parry::shape::{SharedShape, VoxelState, VoxelType, Voxels};

use crate::math::{Real, Vector};

/// A contact between two colliders.
pub type Contact = parry::query::TrackedContact<ContactData>;
/// A contact manifold between two colliders.
pub type ContactManifold = parry::query::ContactManifold<ContactManifoldData, ContactData>;
/// A segment shape.
pub type Segment = parry::shape::Segment;
/// A cuboid shape.
pub type Cuboid = parry::shape::Cuboid;
/// A triangle shape.
pub type Triangle = parry::shape::Triangle;
/// A ball shape.
pub type Ball = parry::shape::Ball;
/// A capsule shape.
pub type Capsule = parry::shape::Capsule;
/// A heightfield shape.
pub type HeightField = parry::shape::HeightField;
/// A cylindrical shape.
#[cfg(feature = "dim3")]
pub type Cylinder = parry::shape::Cylinder;
/// A cone shape.
#[cfg(feature = "dim3")]
pub type Cone = parry::shape::Cone;
/// An axis-aligned bounding box.
pub type Aabb = parry::bounding_volume::Aabb;
/// A ray that can be cast against colliders.
pub type Ray = parry::query::Ray;
/// The intersection between a ray and a  collider.
pub type RayIntersection = parry::query::RayIntersection;
/// The projection of a point on a collider.
pub type PointProjection = parry::query::PointProjection;
/// The result of a shape-cast between two shapes.
pub type ShapeCastHit = parry::query::ShapeCastHit;
/// The default broad-phase implementation recommended for general-purpose usage.
pub type DefaultBroadPhase = BroadPhaseBvh;

bitflags::bitflags! {
    /// Flags providing more information regarding a collision event.
    #[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
    #[derive(Copy, Clone, PartialEq, Eq, Debug, Hash)]
    pub struct CollisionEventFlags: u32 {
        /// Flag set if at least one of the colliders involved in the
        /// collision was a sensor when the event was fired.
        const SENSOR = 0b0001;
        /// Flag set if a `CollisionEvent::Stopped` was fired because
        /// at least one of the colliders was removed.
        const REMOVED = 0b0010;
    }
}

#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[derive(Copy, Clone, Hash, Debug)]
/// Events triggered when two colliders start or stop touching.
///
/// Receive these through an [`EventHandler`](crate::pipeline::EventHandler) implementation.
/// At least one collider must have [`ActiveEvents::COLLISION_EVENTS`](crate::pipeline::ActiveEvents::COLLISION_EVENTS) enabled.
///
/// Use for:
/// - Trigger zones (player entered/exited area)
/// - Collectible items (player touched coin)
/// - Sound effects (objects started colliding)
/// - Game logic based on contact state
///
/// # Example
/// ```
/// # use rapier3d::prelude::*;
/// # let h1 = ColliderHandle::from_raw_parts(0, 0);
/// # let h2 = ColliderHandle::from_raw_parts(1, 0);
/// # let event = CollisionEvent::Started(h1, h2, CollisionEventFlags::empty());
/// match event {
///     CollisionEvent::Started(h1, h2, flags) => {
///         println!("Colliders {:?} and {:?} started touching", h1, h2);
///         if flags.contains(CollisionEventFlags::SENSOR) {
///             println!("At least one is a sensor!");
///         }
///     }
///     CollisionEvent::Stopped(h1, h2, _) => {
///         println!("Colliders {:?} and {:?} stopped touching", h1, h2);
///     }
/// }
/// ```
pub enum CollisionEvent {
    /// Two colliders just started touching this frame.
    Started(ColliderHandle, ColliderHandle, CollisionEventFlags),
    /// Two colliders just stopped touching this frame.
    Stopped(ColliderHandle, ColliderHandle, CollisionEventFlags),
}

impl CollisionEvent {
    /// Returns `true` if this is a Started event (colliders began touching).
    pub fn started(self) -> bool {
        matches!(self, CollisionEvent::Started(..))
    }

    /// Returns `true` if this is a Stopped event (colliders stopped touching).
    pub fn stopped(self) -> bool {
        matches!(self, CollisionEvent::Stopped(..))
    }

    /// Returns the handle of the first collider in this collision.
    pub fn collider1(self) -> ColliderHandle {
        match self {
            Self::Started(h, _, _) | Self::Stopped(h, _, _) => h,
        }
    }

    /// Returns the handle of the second collider in this collision.
    pub fn collider2(self) -> ColliderHandle {
        match self {
            Self::Started(_, h, _) | Self::Stopped(_, h, _) => h,
        }
    }

    /// Was at least one of the colliders involved in the collision a sensor?
    pub fn sensor(self) -> bool {
        match self {
            Self::Started(_, _, f) | Self::Stopped(_, _, f) => {
                f.contains(CollisionEventFlags::SENSOR)
            }
        }
    }

    /// Was at least one of the colliders involved in the collision removed?
    pub fn removed(self) -> bool {
        match self {
            Self::Started(_, _, f) | Self::Stopped(_, _, f) => {
                f.contains(CollisionEventFlags::REMOVED)
            }
        }
    }
}

#[derive(Copy, Clone, PartialEq, Debug, Default)]
/// Event occurring when the sum of the magnitudes of the contact forces
/// between two colliders exceed a threshold.
pub struct ContactForceEvent {
    /// The first collider involved in the contact.
    pub collider1: ColliderHandle,
    /// The second collider involved in the contact.
    pub collider2: ColliderHandle,
    /// The sum of all the forces between the two colliders.
    pub total_force: Vector,
    /// The sum of the magnitudes of each force between the two colliders.
    ///
    /// Note that this is **not** the same as the magnitude of `self.total_force`.
    /// Here we are summing the magnitude of all the forces, instead of taking
    /// the magnitude of their sum.
    pub total_force_magnitude: Real,
    /// The world-space (unit) direction of the force with strongest magnitude.
    pub max_force_direction: Vector,
    /// The magnitude of the largest force at a contact point of this contact pair.
    pub max_force_magnitude: Real,
}

impl ContactForceEvent {
    /// Init a contact force event from a contact pair.
    pub fn from_contact_pair(dt: Real, pair: &ContactPair, total_force_magnitude: Real) -> Self {
        let mut result = ContactForceEvent {
            collider1: pair.collider1,
            collider2: pair.collider2,
            total_force_magnitude,
            ..ContactForceEvent::default()
        };

        for m in &pair.manifolds {
            let mut total_manifold_impulse = 0.0;
            for pt in m.contacts() {
                total_manifold_impulse += pt.data.impulse;

                if pt.data.impulse > result.max_force_magnitude {
                    result.max_force_magnitude = pt.data.impulse;
                    result.max_force_direction = m.data.normal;
                }
            }

            result.total_force += m.data.normal * total_manifold_impulse;
        }

        let inv_dt = crate::utils::inv(dt);
        // NOTE: convert impulses to forces. Note that we
        //       don’t need to convert the `total_force_magnitude`
        //       because it’s an input of this function already
        //       assumed to be a force instead of an impulse.
        result.total_force *= inv_dt;
        result.max_force_magnitude *= inv_dt;
        result
    }
}

pub(crate) use self::narrow_phase::ContactManifoldIndex;
pub use parry::shape::*;

#[cfg(feature = "serde-serialize")]
pub(crate) fn default_persistent_query_dispatcher()
-> std::sync::Arc<dyn parry::query::PersistentQueryDispatcher<ContactManifoldData, ContactData>> {
    std::sync::Arc::new(parry::query::DefaultQueryDispatcher)
}

mod collider_components;
mod contact_pair;
mod interaction_graph;
mod interaction_groups;
mod narrow_phase;

mod broad_phase_bvh;
mod broad_phase_pair_event;
mod collider;
mod collider_set;
mod mesh_converter;

#[cfg(feature = "dim3")]
mod manifold_reduction;
