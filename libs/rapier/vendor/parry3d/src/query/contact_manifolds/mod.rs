//! Contact manifolds for persistent collision detection.
//!
//! # What are Contact Manifolds?
//!
//! A **contact manifold** is a collection of contact points between two shapes that share
//! the same contact normal and contact kinematics. Contact manifolds are fundamental to
//! physics simulation because they provide stable, persistent contact information that can
//! be reused across multiple simulation frames.
//!
//! Unlike single-shot contact queries that compute contact information from scratch each
//! frame, contact manifolds maintain and update contact information over time, leading to
//! more stable and efficient physics simulations.
//!
//! # Why are Contact Manifolds Important?
//!
//! Contact manifolds are critical for physics simulation for several reasons:
//!
//! 1. **Stability**: By maintaining persistent contact information across frames, physics
//!    engines can provide more stable contact resolution without jittering.
//!
//! 2. **Performance**: Updating existing contact points is often faster than recomputing
//!    them from scratch, especially for complex shapes.
//!
//! 3. **Contact Tracking**: Each contact point in a manifold has a unique identifier
//!    (feature IDs) that allows physics engines to track contacts over time and maintain
//!    contact-specific data like accumulated impulses for warm-starting.
//!
//! 4. **Multiple Contact Points**: Many shape pairs require multiple contact points for
//!    stable physics simulation. For example, a box resting on a plane typically needs
//!    4 contact points (one for each corner).
//!
//! # Basic Usage
//!
//! ```rust
//! # #[cfg(all(feature = "dim3", feature = "f32"))] {
//! use parry3d::query::{ContactManifold, TrackedContact};
//! use parry3d::query::details::contact_manifold_ball_ball;
//! use parry3d::shape::Ball;
//! use parry3d::math::{Pose, Vector};
//!
//! // Create two balls
//! let ball1 = Ball::new(1.0);
//! let ball2 = Ball::new(1.0);
//!
//! // Position them so they're touching
//! let pos12 = Pose::translation(1.9, 0.0, 0.0);
//!
//! // Create an empty contact manifold
//! // Note: ManifoldData and ContactData are user-defined types for tracking data
//! let mut manifold = ContactManifold::<(), ()>::new();
//!
//! // Compute the contact manifold with a prediction distance
//! let prediction = 0.1; // Look ahead distance
//! contact_manifold_ball_ball(&pos12, &ball1, &ball2, prediction, &mut manifold);
//!
//! // Check if we have any contacts
//! if let Some(contact) = manifold.points.first() {
//!     println!("Contact distance: {}", contact.dist);
//!     println!("Contact point on ball1: {:?}", contact.local_p1);
//!     println!("Contact point on ball2: {:?}", contact.local_p2);
//! }
//! # }
//! ```
//!
//! # Updating Contact Manifolds
//!
//! One of the key benefits of contact manifolds is the ability to efficiently update them
//! using spatial coherence:
//!
//! ```rust
//! # #[cfg(all(feature = "dim3", feature = "f32"))] {
//! use parry3d::query::{ContactManifold, TrackedContact};
//! use parry3d::query::details::contact_manifold_ball_ball;
//! use parry3d::shape::Ball;
//! use parry3d::math::Pose;
//!
//! let ball1 = Ball::new(1.0);
//! let ball2 = Ball::new(1.0);
//! let mut manifold = ContactManifold::<(), ()>::new();
//!
//! // Initial contact computation
//! let pos12_frame1 = Pose::translation(1.9, 0.0, 0.0);
//! contact_manifold_ball_ball(&pos12_frame1, &ball1, &ball2, 0.1, &mut manifold);
//!
//! // On the next frame, try to update using spatial coherence
//! let pos12_frame2 = Pose::translation(1.85, 0.0, 0.0);
//!
//! // Try to update contacts efficiently
//! if !manifold.try_update_contacts(&pos12_frame2) {
//!     // Update failed (shapes moved too much or orientation changed significantly)
//!     // Fall back to full recomputation
//!     contact_manifold_ball_ball(&pos12_frame2, &ball1, &ball2, 0.1, &mut manifold);
//! }
//! # }
//! ```
//!
//! # Contact Prediction
//!
//! The `prediction` parameter is a distance threshold that allows detecting contacts
//! slightly before shapes actually touch. This is useful for:
//!
//! - **Continuous Collision Detection**: Predicting contacts helps prevent tunneling
//! - **Speculative Contacts**: Physics engines can prepare for imminent collisions
//! - **Contact Caching**: Maintaining contacts even when shapes briefly separate
//!
//! A typical prediction value might be 1-10% of the smallest shape dimension.
//!
//! # Shape-Specific Contact Computation
//!
//! This module provides specialized contact manifold computation functions for various
//! shape pairs. Each function is optimized for the specific geometric properties of the
//! shapes involved:
//!
//! - `contact_manifold_ball_ball`: Two spheres (simplest case, always 1 contact)
//! - `contact_manifold_cuboid_cuboid`: Two boxes (up to 8 contacts in 3D)
//! - `contact_manifold_capsule_capsule`: Two capsules
//! - `contact_manifold_convex_ball`: Convex shape vs sphere
//! - And many more specialized functions for different shape combinations
//!
//! # Working with Composite Shapes
//!
//! For composite shapes (triangle meshes, heightfields, compounds), contact manifold
//! computation is more complex and requires workspace objects for efficient memory management:
//!
//! - `contact_manifolds_trimesh_shape`: Triangle mesh vs any shape
//! - `contact_manifolds_heightfield_shape`: Heightfield terrain vs any shape
//! - `contact_manifolds_composite_shape_shape`: Compound shape vs any shape
//! - `contact_manifolds_voxels_shape`: Voxel grid vs any shape
//!
//! These functions use workspace objects to avoid repeated allocations.

pub use self::contact_manifold::{ContactManifold, TrackedContact};
pub use self::contact_manifolds_ball_ball::{
    contact_manifold_ball_ball, contact_manifold_ball_ball_shapes,
};
pub use self::contact_manifolds_capsule_capsule::{
    contact_manifold_capsule_capsule, contact_manifold_capsule_capsule_shapes,
};
pub use self::contact_manifolds_convex_ball::{
    contact_manifold_convex_ball, contact_manifold_convex_ball_shapes,
};
// pub use self::contact_manifolds_cuboid_capsule::{
//     contact_manifold_cuboid_capsule, contact_manifold_cuboid_capsule_shapes,
// };
pub use self::contact_manifolds_composite_shape_composite_shape::contact_manifolds_composite_shape_composite_shape;
pub use self::contact_manifolds_composite_shape_shape::contact_manifolds_composite_shape_shape;
pub use self::contact_manifolds_cuboid_cuboid::{
    contact_manifold_cuboid_cuboid, contact_manifold_cuboid_cuboid_shapes,
};
pub use self::contact_manifolds_cuboid_triangle::{
    contact_manifold_cuboid_triangle, contact_manifold_cuboid_triangle_shapes,
};
pub use self::contact_manifolds_halfspace_pfm::{
    contact_manifold_halfspace_pfm, contact_manifold_halfspace_pfm_shapes,
};
pub use self::contact_manifolds_heightfield_composite_shape::contact_manifolds_heightfield_composite_shape;
pub use self::contact_manifolds_heightfield_shape::{
    contact_manifolds_heightfield_shape, contact_manifolds_heightfield_shape_shapes,
};
pub use self::contact_manifolds_pfm_pfm::{
    contact_manifold_pfm_pfm, contact_manifold_pfm_pfm_shapes,
};
pub use self::contact_manifolds_trimesh_shape::{
    contact_manifolds_trimesh_shape, contact_manifolds_trimesh_shape_shapes,
};
pub use self::contact_manifolds_voxels_ball::contact_manifolds_voxels_ball_shapes;
pub use self::contact_manifolds_voxels_composite_shape::{
    contact_manifolds_voxels_composite_shape, contact_manifolds_voxels_composite_shape_shapes,
};
pub use self::contact_manifolds_voxels_shape::{
    contact_manifolds_voxels_shape, contact_manifolds_voxels_shape_shapes,
    VoxelsShapeContactManifoldsWorkspace,
};
pub(crate) use self::contact_manifolds_voxels_shape::{
    CanonicalVoxelShape, VoxelsShapeSubDetector,
};
pub use self::contact_manifolds_voxels_voxels::{
    contact_manifolds_voxels_voxels, contact_manifolds_voxels_voxels_shapes,
};
pub use self::contact_manifolds_workspace::{
    ContactManifoldsWorkspace, TypedWorkspaceData, WorkspaceData,
};
pub use self::normals_constraint::{NormalConstraints, NormalConstraintsPair};

use {
    self::contact_manifolds_composite_shape_composite_shape::CompositeShapeCompositeShapeContactManifoldsWorkspace,
    self::contact_manifolds_composite_shape_shape::CompositeShapeShapeContactManifoldsWorkspace,
    self::contact_manifolds_heightfield_composite_shape::HeightFieldCompositeShapeContactManifoldsWorkspace,
    self::contact_manifolds_heightfield_shape::HeightFieldShapeContactManifoldsWorkspace,
    self::contact_manifolds_trimesh_shape::TriMeshShapeContactManifoldsWorkspace,
};

mod contact_manifold;
mod contact_manifolds_ball_ball;
mod contact_manifolds_capsule_capsule;
mod contact_manifolds_convex_ball;
// mod contact_manifolds_cuboid_capsule;
mod contact_manifolds_composite_shape_composite_shape;
mod contact_manifolds_composite_shape_shape;
mod contact_manifolds_cuboid_cuboid;
mod contact_manifolds_cuboid_triangle;
mod contact_manifolds_halfspace_pfm;
mod contact_manifolds_heightfield_composite_shape;
mod contact_manifolds_heightfield_shape;
mod contact_manifolds_pfm_pfm;
mod contact_manifolds_trimesh_shape;
mod contact_manifolds_voxels_ball;
mod contact_manifolds_voxels_composite_shape;
mod contact_manifolds_voxels_shape;
mod contact_manifolds_voxels_voxels;
mod contact_manifolds_workspace;
mod normals_constraint;
