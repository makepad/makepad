//! Query dispatcher system for extensible geometric queries.
//!
//! # Overview
//!
//! This module provides the **query dispatcher** abstraction, which allows Parry to perform
//! geometric queries (distance, contact, ray casting, etc.) between different shape types in
//! an extensible and customizable way.
//!
//! # What is a Query Dispatcher?
//!
//! A query dispatcher is an object that knows how to perform geometric queries between pairs
//! of shapes. When you ask "what is the distance between shape A and shape B?", the query
//! dispatcher:
//!
//! 1. Examines the types of both shapes (Ball, Cuboid, ConvexMesh, etc.)
//! 2. Selects the most appropriate algorithm for that shape pair
//! 3. Executes the query and returns the result
//!
//! For example, computing distance between two balls uses a simple formula, while computing
//! distance between two convex meshes requires the GJK algorithm. The query dispatcher
//! handles this selection automatically.
//!
//! # Why Do Query Dispatchers Exist?
//!
//! Query dispatchers provide **extensibility** for several use cases:
//!
//! 1. **Custom Shapes**: If you implement a custom shape type (e.g., a superellipsoid or
//!    implicit surface), you can create a custom query dispatcher that knows how to handle
//!    queries involving your shape, while delegating other queries to the default dispatcher.
//!
//! 2. **Specialized Algorithms**: You might have a faster algorithm for specific shape pairs
//!    in your application domain. A custom dispatcher lets you override just those cases.
//!
//! 3. **Debugging and Profiling**: You can wrap the default dispatcher to log query calls,
//!    collect statistics, or inject debug visualizations.
//!
//! 4. **Fallback Chains**: Multiple dispatchers can be chained together using the `chain()`
//!    method, so if one dispatcher doesn't support a query, it automatically falls back to
//!    the next one.
//!
//! # When to Use Query Dispatchers
//!
//! Most users will never need to directly use or implement query dispatchers. The high-level
//! query functions in [`crate::query`] use the [`DefaultQueryDispatcher`](crate::query::DefaultQueryDispatcher) automatically.
//!
//! You should implement a custom query dispatcher when:
//!
//! - You have custom shape types not supported by Parry
//! - You need specialized algorithms for specific shape pairs
//! - You're integrating Parry into a larger system that needs to intercept queries
//! - You want to add logging, profiling, or debugging to queries
//!
//! # Basic Usage
//!
//! ## Using the Default Dispatcher
//!
//! The simplest way is to use the free functions in [`crate::query`], which use the default
//! dispatcher internally:
//!
//! ```
//! # #[cfg(all(feature = "dim3", feature = "f32"))] {
//! use parry3d::query;
//! use parry3d::shape::Ball;
//! use parry3d::math::Pose;
//!
//! let ball1 = Ball::new(0.5);
//! let ball2 = Ball::new(1.0);
//! let pos1 = Pose::identity();
//! let pos2 = Pose::translation(5.0, 0.0, 0.0);
//!
//! // Uses DefaultQueryDispatcher automatically
//! let distance = query::distance(&pos1, &ball1, &pos2, &ball2);
//! # }
//! ```
//!
//! ## Using a Dispatcher Directly
//!
//! If you need explicit control, you can create and use a dispatcher:
//!
//! ```
//! # #[cfg(all(feature = "dim3", feature = "f32"))] {
//! use parry3d::query::{QueryDispatcher, DefaultQueryDispatcher};
//! use parry3d::shape::Ball;
//! use parry3d::math::Pose;
//!
//! let dispatcher = DefaultQueryDispatcher;
//! let ball1 = Ball::new(0.5);
//! let ball2 = Ball::new(1.0);
//!
//! let pos1 = Pose::identity();
//! let pos2 = Pose::translation(5.0, 0.0, 0.0);
//!
//! // Compute relative position of shape2 in shape1's local space
//! let pos12 = pos1.inv_mul(&pos2);
//!
//! let distance = dispatcher.distance(&pos12, &ball1, &ball2).unwrap();
//! # }
//! ```
//!
//! ## Chaining Dispatchers
//!
//! You can chain multiple dispatchers to create a fallback sequence:
//!
//! ```ignore
//! use parry3d::query::{QueryDispatcher, DefaultQueryDispatcher};
//!
//! // Create a custom dispatcher for your custom shapes
//! let custom = MyCustomDispatcher::new();
//!
//! // Chain it with the default dispatcher as a fallback
//! let dispatcher = custom.chain(DefaultQueryDispatcher);
//!
//! // Now dispatcher will try your custom dispatcher first,
//! // and fall back to the default if it returns Unsupported
//! ```
//!
//! # Implementing a Custom Query Dispatcher
//!
//! To implement a custom query dispatcher, implement the [`QueryDispatcher`] trait:
//!
//! ```ignore
//! use parry3d::query::{QueryDispatcher, Unsupported};
//! use parry3d::shape::Shape;
//! use parry3d::math::{Pose, Real};
//!
//! struct MyDispatcher {
//!     // Your dispatcher state
//! }
//!
//! impl QueryDispatcher for MyDispatcher {
//!     fn intersection_test(
//!         &self,
//!         pos12: &Pose,
//!         g1: &dyn Shape,
//!         g2: &dyn Shape,
//!     ) -> Result<bool, Unsupported> {
//!         // Try to downcast to your custom shape types
//!         if let (Some(my_shape1), Some(my_shape2)) = (
//!             g1.as_any().downcast_ref::<MyCustomShape>(),
//!             g2.as_any().downcast_ref::<MyCustomShape>(),
//!         ) {
//!             // Implement your custom intersection test
//!             Ok(my_custom_intersection_test(my_shape1, my_shape2, pos12))
//!         } else {
//!             // Return Unsupported for shape pairs you don't handle
//!             Err(Unsupported)
//!         }
//!     }
//!
//!     fn distance(
//!         &self,
//!         pos12: &Pose,
//!         g1: &dyn Shape,
//!         g2: &dyn Shape,
//!     ) -> Result<Real, Unsupported> {
//!         // Implement other query methods similarly
//!         Err(Unsupported)
//!     }
//!
//!     // ... implement other required methods
//! }
//! ```
//!
//! # Important Notes
//!
//! ## The `pos12` Parameter
//!
//! All query methods take a `pos12` parameter, which is the **relative position** of shape 2
//! in shape 1's local coordinate frame. This is computed as:
//!
//! ```ignore
//! let pos12 = pos1.inv_mul(&pos2);  // Transform from g2's space to g1's space
//! ```
//!
//! This convention reduces the number of transformations needed during queries.
//!
//! ## Thread Safety
//!
//! Query dispatchers must implement `Send + Sync` because they may be shared across threads
//! in parallel collision detection scenarios (e.g., when using Rapier physics engine).
//!
//! ## Error Handling
//!
//! Query methods return `Result<T, Unsupported>`. Return `Err(Unsupported)` when:
//! - The dispatcher doesn't know how to handle the given shape pair
//! - The shapes cannot be queried with the requested operation
//!
//! When chaining dispatchers, returning `Unsupported` allows the next dispatcher in the
//! chain to try handling the query.

use crate::math::{Pose, Real, Vector};
use crate::query::details::ShapeCastOptions;
#[cfg(feature = "alloc")]
use crate::query::{
    contact_manifolds::{ContactManifoldsWorkspace, NormalConstraints},
    ContactManifold,
};
use crate::query::{ClosestPoints, Contact, NonlinearRigidMotion, ShapeCastHit, Unsupported};
use crate::shape::Shape;
#[cfg(feature = "alloc")]
use alloc::vec::Vec;

#[cfg(feature = "alloc")]
/// A query dispatcher for queries relying on spatial coherence, including contact-manifold computation.
///
/// This trait extends [`QueryDispatcher`] with methods that maintain persistent state between
/// queries to improve performance through **spatial and temporal coherence**.
///
/// # What is Spatial Coherence?
///
/// Spatial coherence is the principle that objects that are close to each other in one frame
/// are likely to remain close in subsequent frames. Contact manifolds exploit this by:
///
/// 1. Tracking contact points over multiple frames
/// 2. Reusing previous contact information to accelerate new queries
/// 3. Maintaining contact IDs for physics solvers to track persistent contacts
///
/// # Contact Manifolds vs Single Contacts
///
/// - **Single Contact** ([`QueryDispatcher::contact`]): Returns one contact point, suitable
///   for one-off queries or simple collision detection.
///
/// - **Contact Manifold** ([`PersistentQueryDispatcher::contact_manifolds`]): Returns multiple
///   contact points that persist across frames, essential for stable physics simulation.
///
/// # When to Use This Trait
///
/// Use `PersistentQueryDispatcher` when:
/// - Implementing physics simulation that needs stable contact tracking
/// - Building a custom physics engine on top of Parry
/// - Optimizing repeated collision queries between the same shape pairs
///
/// # Generic Parameters
///
/// - `ManifoldData`: Custom data attached to each contact manifold (default: `()`)
/// - `ContactData`: Custom data attached to each contact point (default: `()`)
///
/// These allow physics engines to attach solver-specific data to contacts without modifying
/// Parry's core types.
pub trait PersistentQueryDispatcher<ManifoldData = (), ContactData = ()>: QueryDispatcher {
    /// Compute all the contacts between two shapes.
    ///
    /// The output is written into `manifolds` and `context`. Both can persist
    /// between multiple calls to `contacts` by re-using the result of the previous
    /// call to `contacts`. This persistence can significantly improve collision
    /// detection performances by allowing the underlying algorithms to exploit
    /// spatial and temporal coherence.
    fn contact_manifolds(
        &self,
        pos12: &Pose,
        g1: &dyn Shape,
        g2: &dyn Shape,
        prediction: Real,
        manifolds: &mut Vec<ContactManifold<ManifoldData, ContactData>>,
        workspace: &mut Option<ContactManifoldsWorkspace>,
    ) -> Result<(), Unsupported>;

    /// Computes the contact-manifold between two convex shapes.
    fn contact_manifold_convex_convex(
        &self,
        pos12: &Pose,
        g1: &dyn Shape,
        g2: &dyn Shape,
        normal_constraints1: Option<&dyn NormalConstraints>,
        normal_constraints2: Option<&dyn NormalConstraints>,
        prediction: Real,
        manifold: &mut ContactManifold<ManifoldData, ContactData>,
    ) -> Result<(), Unsupported>;
}

/// Dispatcher for pairwise geometric queries between shapes.
///
/// This is the core trait for performing geometric queries (distance, intersection, contact, etc.)
/// between pairs of shapes in Parry. It provides a uniform interface for querying different shape
/// combinations.
///
/// # Purpose
///
/// The `QueryDispatcher` trait serves as an abstraction layer that:
///
/// 1. **Decouples query algorithms from shape types**: Different shape pairs may require different
///    algorithms (e.g., sphere-sphere uses simple math, while convex-convex uses GJK/EPA).
///
/// 2. **Enables extensibility**: You can implement custom dispatchers to add support for custom
///    shape types or specialized algorithms without modifying Parry's core code.
///
/// 3. **Provides fallback mechanisms**: Dispatchers can be chained together using [`chain()`](Self::chain),
///    allowing graceful fallback when a query is not supported.
///
/// # The `pos12` Parameter Convention
///
/// All query methods in this trait take a `pos12` parameter, which represents the **relative
/// position** of shape 2 (`g2`) in shape 1's (`g1`) local coordinate frame:
///
/// ```ignore
/// pos12 = pos1.inverse() * pos2
/// ```
///
/// This convention is used because:
/// - It reduces redundant transformations during query execution
/// - Many algorithms naturally work in one shape's local space
/// - It's more efficient than passing two separate world-space positions
///
/// When using the high-level query functions, this transformation is done automatically.
///
/// # Query Methods Overview
///
/// The trait provides several categories of queries:
///
/// ## Basic Queries
/// - [`intersection_test`](Self::intersection_test): Boolean intersection check (fastest)
/// - [`distance`](Self::distance): Minimum separating distance
/// - [`closest_points`](Self::closest_points): Pair of closest points
/// - [`contact`](Self::contact): Contact point with penetration depth
///
/// ## Motion Queries (Time of Impact)
/// - [`cast_shapes`](Self::cast_shapes): Linear motion (translation only)
/// - [`cast_shapes_nonlinear`](Self::cast_shapes_nonlinear): Nonlinear motion (translation + rotation)
///
/// # Thread Safety
///
/// Query dispatchers must be `Send + Sync` because they're often shared across threads in:
/// - Parallel collision detection
/// - Physics simulations with multi-threaded broad phase
/// - Game engines with parallel scene queries
///
/// Ensure your custom dispatcher implementations are thread-safe or use appropriate synchronization.
///
/// # Error Handling with `Unsupported`
///
/// All query methods return `Result<T, Unsupported>`. The `Unsupported` error indicates:
///
/// - The dispatcher doesn't know how to handle the given shape pair
/// - The shapes don't support this type of query
/// - The query is mathematically undefined for these shapes
///
/// When chaining dispatchers with [`chain()`](Self::chain), returning `Unsupported` causes the
/// next dispatcher in the chain to be tried.
///
/// # Example: Using the Default Dispatcher
///
/// ```
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::query::{QueryDispatcher, DefaultQueryDispatcher};
/// use parry3d::shape::{Ball, Cuboid};
/// use parry3d::math::{Pose, Vector};
///
/// let dispatcher = DefaultQueryDispatcher;
///
/// let ball = Ball::new(1.0);
/// let cuboid = Cuboid::new(Vector::splat(2.0));
///
/// let pos1 = Pose::identity();
/// let pos2 = Pose::translation(5.0, 0.0, 0.0);
/// let pos12 = pos1.inv_mul(&pos2);
///
/// // Test intersection
/// let intersects = dispatcher.intersection_test(&pos12, &ball, &cuboid).unwrap();
///
/// // Compute distance
/// let dist = dispatcher.distance(&pos12, &ball, &cuboid).unwrap();
///
/// println!("Intersects: {}, Distance: {}", intersects, dist);
/// # }
/// ```
///
/// # Example: Chaining Dispatchers
///
/// ```ignore
/// # {
/// use parry3d::query::{QueryDispatcher, DefaultQueryDispatcher};
///
/// struct MyCustomDispatcher;
///
/// impl QueryDispatcher for MyCustomDispatcher {
///     fn distance(
///         &self,
///         pos12: &Pose,
///         g1: &dyn Shape,
///         g2: &dyn Shape,
///     ) -> Result<Real, Unsupported> {
///         // Handle custom shape types
///         if let Some(my_shape) = g1.as_any().downcast_ref::<MyShape>() {
///             // ... custom distance computation
///             Ok(computed_distance)
///         } else {
///             // Don't know how to handle this, let the next dispatcher try
///             Err(Unsupported)
///         }
///     }
///
///     // ... implement other methods
/// }
///
/// // Chain custom dispatcher with default as fallback
/// let dispatcher = MyCustomDispatcher.chain(DefaultQueryDispatcher);
///
/// // Now all queries try custom dispatcher first, then default
/// let dist = dispatcher.distance(pos12, shape1, shape2)?;
/// # }
/// ```
///
/// # See Also
///
/// - [`DefaultQueryDispatcher`](crate::query::DefaultQueryDispatcher): The built-in implementation used by Parry
/// - [`PersistentQueryDispatcher`]: Extended trait for contact manifold queries
/// - [`crate::query`]: High-level query functions that use dispatchers internally
pub trait QueryDispatcher: Send + Sync {
    /// Tests whether two shapes are intersecting.
    fn intersection_test(
        &self,
        pos12: &Pose,
        g1: &dyn Shape,
        g2: &dyn Shape,
    ) -> Result<bool, Unsupported>;

    /// Computes the minimum distance separating two shapes.
    ///
    /// Returns `0.0` if the objects are touching or penetrating.
    fn distance(&self, pos12: &Pose, g1: &dyn Shape, g2: &dyn Shape) -> Result<Real, Unsupported>;

    /// Computes one pair of contact points point between two shapes.
    ///
    /// Returns `None` if the objects are separated by a distance greater than `prediction`.
    fn contact(
        &self,
        pos12: &Pose,
        g1: &dyn Shape,
        g2: &dyn Shape,
        prediction: Real,
    ) -> Result<Option<Contact>, Unsupported>;

    /// Computes the pair of closest points between two shapes.
    ///
    /// Returns `ClosestPoints::Disjoint` if the objects are separated by a distance greater than `max_dist`.
    fn closest_points(
        &self,
        pos12: &Pose,
        g1: &dyn Shape,
        g2: &dyn Shape,
        max_dist: Real,
    ) -> Result<ClosestPoints, Unsupported>;

    /// Computes the smallest time when two shapes under translational movement are separated by a
    /// distance smaller or equal to `distance`.
    ///
    /// Returns `0.0` if the objects are touching or penetrating.
    ///
    /// # Parameters
    /// - `pos12`: the position of the second shape relative to the first shape.
    /// - `local_vel12`: the relative velocity between the two shapes, expressed in the local-space
    ///   of the first shape. In other world: `pos1.inverse() * (vel2 - vel1)`.
    /// - `g1`: the first shape involved in the shape-cast.
    /// - `g2`: the second shape involved in the shape-cast.
    /// - `target_dist`: a hit will be returned as soon as the two shapes get closer than `target_dist`.
    /// - `max_time_of_impact`: the maximum allowed travel time. This method returns `None` if the time-of-impact
    ///   detected is theater than this value.
    fn cast_shapes(
        &self,
        pos12: &Pose,
        local_vel12: Vector,
        g1: &dyn Shape,
        g2: &dyn Shape,
        options: ShapeCastOptions,
    ) -> Result<Option<ShapeCastHit>, Unsupported>;

    /// Construct a `QueryDispatcher` that falls back on `other` for cases not handled by `self`
    fn chain<U: QueryDispatcher>(self, other: U) -> QueryDispatcherChain<Self, U>
    where
        Self: Sized,
    {
        QueryDispatcherChain(self, other)
    }

    /// Computes the smallest time of impact of two shapes under translational and rotational movement.
    ///
    /// # Parameters
    /// * `motion1` - The motion of the first shape.
    /// * `g1` - The first shape involved in the query.
    /// * `motion2` - The motion of the second shape.
    /// * `g2` - The second shape involved in the query.
    /// * `start_time` - The starting time of the interval where the motion takes place.
    /// * `end_time` - The end time of the interval where the motion takes place.
    /// * `stop_at_penetration` - If the casted shape starts in a penetration state with any
    ///   collider, two results are possible. If `stop_at_penetration` is `true` then, the
    ///   result will have a `time_of_impact` equal to `start_time`. If `stop_at_penetration` is `false`
    ///   then the nonlinear shape-casting will see if further motion wrt. the penetration normal
    ///   would result in tunnelling. If it does not (i.e. we have a separating velocity along
    ///   that normal) then the nonlinear shape-casting will attempt to find another impact,
    ///   at a time `> start_time` that could result in tunnelling.
    fn cast_shapes_nonlinear(
        &self,
        motion1: &NonlinearRigidMotion,
        g1: &dyn Shape,
        motion2: &NonlinearRigidMotion,
        g2: &dyn Shape,
        start_time: Real,
        end_time: Real,
        stop_at_penetration: bool,
    ) -> Result<Option<ShapeCastHit>, Unsupported>;
}

/// A chain of two query dispatchers that provides fallback behavior.
///
/// This struct is created by calling [`QueryDispatcher::chain()`] and allows combining multiple
/// dispatchers into a fallback sequence. When a query is performed:
///
/// 1. The first dispatcher (`T`) is tried
/// 2. If it returns `Err(Unsupported)`, the second dispatcher (`U`) is tried
/// 3. If the second also returns `Unsupported`, the error is propagated
///
/// # Use Cases
///
/// Dispatcher chains are useful for:
///
/// - **Custom shapes with default fallback**: Handle your custom shapes specifically, but fall
///   back to Parry's default dispatcher for standard shapes
///
/// - **Performance optimization**: Try a fast specialized algorithm first, fall back to a general
///   but slower algorithm if needed
///
/// - **Progressive feature support**: Layer dispatchers that support different feature sets
///
/// - **Debugging**: Wrap the default dispatcher with a logging dispatcher that passes through
///
/// # Example
///
/// ```ignore
/// # {
/// use parry3d::query::{QueryDispatcher, DefaultQueryDispatcher};
///
/// // A dispatcher that handles only custom shapes
/// struct CustomShapeDispatcher;
/// impl QueryDispatcher for CustomShapeDispatcher {
///     fn distance(
///         &self,
///         pos12: &Pose,
///         g1: &dyn Shape,
///         g2: &dyn Shape,
///     ) -> Result<Real, Unsupported> {
///         // Try to handle custom shapes
///         match (g1.as_any().downcast_ref::<MyShape>(),
///                g2.as_any().downcast_ref::<MyShape>()) {
///             (Some(s1), Some(s2)) => Ok(custom_distance(s1, s2, pos12)),
///             _ => Err(Unsupported), // Let the next dispatcher handle it
///         }
///     }
///     // ... other methods return Err(Unsupported)
/// }
///
/// // Chain: try custom first, fall back to default
/// let dispatcher = CustomShapeDispatcher.chain(DefaultQueryDispatcher);
///
/// // This will use CustomShapeDispatcher if both are MyShape,
/// // otherwise DefaultQueryDispatcher handles it
/// let dist = dispatcher.distance(pos12, shape1, shape2)?;
/// # }
/// ```
///
/// # Performance Note
///
/// Chaining has minimal overhead - it's just two function calls in the worst case. The first
/// dispatcher is always tried first, so place your most likely-to-succeed dispatcher at the
/// front of the chain for best performance.
///
/// # Multiple Chains
///
/// You can chain more than two dispatchers by chaining chains:
///
/// ```ignore
/// let dispatcher = custom1
///     .chain(custom2)
///     .chain(DefaultQueryDispatcher);
/// ```
///
/// This creates a chain of three dispatchers: `custom1` -> `custom2` -> `DefaultQueryDispatcher`.
pub struct QueryDispatcherChain<T, U>(T, U);

macro_rules! chain_method {
    ($name:ident ( $( $arg:ident : $ty:ty,)*) -> $result:ty) => {
        fn $name(&self, $($arg : $ty,)*
        ) -> Result<$result, Unsupported> {
            (self.0).$name($($arg,)*)
                .or_else(|Unsupported| (self.1).$name($($arg,)*))
        }
    }
}

impl<T, U> QueryDispatcher for QueryDispatcherChain<T, U>
where
    T: QueryDispatcher,
    U: QueryDispatcher,
{
    chain_method!(intersection_test(
        pos12: &Pose,
        g1: &dyn Shape,
        g2: &dyn Shape,
    ) -> bool);

    chain_method!(distance(pos12: &Pose, g1: &dyn Shape, g2: &dyn Shape,) -> Real);

    chain_method!(contact(
        pos12: &Pose,
        g1: &dyn Shape,
        g2: &dyn Shape,
        prediction: Real,
    ) -> Option<Contact>);

    chain_method!(closest_points(
        pos12: &Pose,
        g1: &dyn Shape,
        g2: &dyn Shape,
        max_dist: Real,
    ) -> ClosestPoints);

    chain_method!(cast_shapes(
        pos12: &Pose,
        vel12: Vector,
        g1: &dyn Shape,
        g2: &dyn Shape,
        options: ShapeCastOptions,
    ) -> Option<ShapeCastHit>);

    chain_method!(cast_shapes_nonlinear(
        motion1: &NonlinearRigidMotion,
        g1: &dyn Shape,
        motion2: &NonlinearRigidMotion,
        g2: &dyn Shape,
        start_time: Real,
        end_time: Real,
        stop_at_penetration: bool,
    ) -> Option<ShapeCastHit>);
}

#[cfg(feature = "alloc")]
impl<ManifoldData, ContactData, T, U> PersistentQueryDispatcher<ManifoldData, ContactData>
    for QueryDispatcherChain<T, U>
where
    T: PersistentQueryDispatcher<ManifoldData, ContactData>,
    U: PersistentQueryDispatcher<ManifoldData, ContactData>,
{
    chain_method!(contact_manifolds(
        pos12: &Pose,
        g1: &dyn Shape,
        g2: &dyn Shape,
        prediction: Real,
        manifolds: &mut Vec<ContactManifold<ManifoldData, ContactData>>,
        workspace: &mut Option<ContactManifoldsWorkspace>,
    ) -> ());

    chain_method!(contact_manifold_convex_convex(
        pos12: &Pose,
        g1: &dyn Shape,
        g2: &dyn Shape,
        normal_constraints1: Option<&dyn NormalConstraints>,
        normal_constraints2: Option<&dyn NormalConstraints>,
        prediction: Real,
        manifold: &mut ContactManifold<ManifoldData, ContactData>,
    ) -> ());
}
