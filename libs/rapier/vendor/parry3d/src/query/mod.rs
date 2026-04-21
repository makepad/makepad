//! Non-persistent geometric queries.
//!
//! # General cases
//! The most general methods provided by this module are:
//!
//! * [`closest_points()`] to compute the closest points between two shapes.
//! * [`distance()`] to compute the distance between two shapes.
//! * [`contact()`] to compute one pair of contact points between two shapes, including penetrating contact.
//! * [`intersection_test()`] to determine if two shapes are intersecting or not.
//! * [`cast_shapes()`] to determine when two shapes undergoing translational motions hit for the first time.
//! * [`cast_shapes_nonlinear()`] to determine when two shapes undergoing continuous rigid motions hit for the first time.
//!
//! Ray-casting and point-projection can be achieved by importing traits:
//!
//! * [`RayCast`] for ray-casting.
//! * [`PointQuery`] for point projection.
//!
//! # Specific cases
//! The functions exported by the `details` submodule are more specific versions of the ones described above.
//! For example `distance_ball_ball` computes the distance between two shapes known at compile-time to be balls.
//! They are less convenient to use than the most generic version but will be slightly faster due to the lack of dynamic dispatch.
//! The specific functions have the form `[operation]_[shape1]_[shape2]()` where:
//!
//! * `[operation]` can be `closest_points`, `distance`, `contact`, `intersection_test` or `time_of_impact`.
//! * `[shape1]` is the type of the first shape passed to the function, e.g., `ball`, or `halfspace`. Can also identify a trait implemented by supported shapes, e.g., `support_map`.
//! * `[shape2]` is the type of the second shape passed to the function, e.g., `ball`, or `halfspace`. Can also identify a trait implemented by supported shapes, e.g., `support_map`.

pub use self::closest_points::{closest_points, ClosestPoints};
pub use self::contact::{contact, Contact};
#[cfg(feature = "alloc")]
pub use self::contact_manifolds::{
    ContactManifold, ContactManifoldsWorkspace, TrackedContact, TypedWorkspaceData, WorkspaceData,
};
pub use self::default_query_dispatcher::DefaultQueryDispatcher;
pub use self::distance::distance;
pub use self::error::Unsupported;
pub use self::intersection_test::intersection_test;
pub use self::nonlinear_shape_cast::{cast_shapes_nonlinear, NonlinearRigidMotion};
pub use self::point::{PointProjection, PointQuery, PointQueryWithLocation};
#[cfg(feature = "alloc")]
pub use self::query_dispatcher::PersistentQueryDispatcher;
pub use self::query_dispatcher::{QueryDispatcher, QueryDispatcherChain};
pub use self::ray::{Ray, RayCast, RayIntersection, SimdRay};
pub use self::shape_cast::{cast_shapes, ShapeCastHit, ShapeCastOptions, ShapeCastStatus};
pub use self::split::{IntersectResult, SplitResult};

#[cfg(all(feature = "dim3", feature = "alloc"))]
pub use self::ray::RayCullingMode;

mod clip;
pub mod closest_points;
pub mod contact;
#[cfg(feature = "alloc")]
mod contact_manifolds;
mod default_query_dispatcher;
mod distance;
#[cfg(feature = "alloc")]
pub mod epa;
mod error;
pub mod gjk;
mod intersection_test;
mod nonlinear_shape_cast;
pub mod point;
mod query_dispatcher;
mod ray;
pub mod sat;
mod shape_cast;
mod split;

/// Queries dedicated to specific pairs of shapes.
pub mod details {
    pub use super::clip::*;
    pub use super::closest_points::*;
    pub use super::contact::*;
    #[cfg(feature = "alloc")]
    pub use super::contact_manifolds::*;
    pub use super::distance::*;
    pub use super::intersection_test::*;
    pub use super::nonlinear_shape_cast::*;
    pub use super::point::*;
    pub use super::ray::*;
    pub use super::shape_cast::*;
}
