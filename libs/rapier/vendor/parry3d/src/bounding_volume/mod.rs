//! Bounding volumes.

#[doc(inline)]
pub use crate::bounding_volume::aabb::Aabb;

// #[cfg(feature = "simd-is-enabled")]
// pub use crate::bounding_volume::simd_aabb::SimdAabb;

#[doc(inline)]
pub use crate::bounding_volume::bounding_sphere::BoundingSphere;
#[doc(inline)]
pub use crate::bounding_volume::bounding_volume::BoundingVolume;

#[doc(hidden)]
pub mod bounding_volume;

#[doc(hidden)]
pub mod aabb;
mod aabb_ball;
#[cfg(feature = "dim2")]
#[cfg(feature = "alloc")]
mod aabb_convex_polygon;
#[cfg(feature = "dim3")]
#[cfg(feature = "alloc")]
mod aabb_convex_polyhedron;
mod aabb_cuboid;
mod aabb_halfspace;
#[cfg(feature = "alloc")]
mod aabb_heightfield;
mod aabb_support_map;
mod aabb_triangle;
mod aabb_utils;

mod aabb_capsule;
#[cfg(feature = "alloc")]
mod aabb_voxels;
#[doc(hidden)]
pub mod bounding_sphere;
mod bounding_sphere_ball;
mod bounding_sphere_capsule;
#[cfg(feature = "dim3")]
mod bounding_sphere_cone;
#[cfg(feature = "dim3")]
#[cfg(feature = "alloc")]
mod bounding_sphere_convex;
#[cfg(feature = "dim2")]
#[cfg(feature = "alloc")]
mod bounding_sphere_convex_polygon;
mod bounding_sphere_cuboid;
#[cfg(feature = "dim3")]
mod bounding_sphere_cylinder;
mod bounding_sphere_halfspace;
#[cfg(feature = "alloc")]
mod bounding_sphere_heightfield;
#[cfg(feature = "alloc")]
mod bounding_sphere_polyline;
mod bounding_sphere_segment;
mod bounding_sphere_triangle;
#[cfg(feature = "alloc")]
mod bounding_sphere_trimesh;
mod bounding_sphere_utils;
#[cfg(feature = "alloc")]
mod bounding_sphere_voxels;

// #[cfg(feature = "simd-is-enabled")]
// mod simd_aabb;

/// Free functions for some special cases of bounding-volume computation.
pub mod details {
    #[cfg(feature = "dim3")]
    pub use super::aabb_utils::support_map_aabb;
    pub use super::aabb_utils::{
        local_point_cloud_aabb, local_point_cloud_aabb_ref, local_support_map_aabb,
        point_cloud_aabb, point_cloud_aabb_ref,
    };
    pub use super::bounding_sphere_utils::point_cloud_bounding_sphere;
}
