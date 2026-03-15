//! Ray-casting related definitions and implementations.

#[doc(inline)]
pub use self::ray::{Ray, RayCast, RayIntersection};
pub use self::ray_ball::ray_toi_with_ball;
pub use self::ray_halfspace::{line_toi_with_halfspace, ray_toi_with_halfspace};
pub use self::ray_support_map::local_ray_intersection_with_support_map_with_params;
#[cfg(feature = "dim3")]
pub use self::ray_triangle::local_ray_intersection_with_triangle;
#[cfg(all(feature = "dim3", feature = "alloc"))]
pub use self::ray_trimesh::RayCullingMode;
pub use self::simd_ray::SimdRay;

#[doc(hidden)]
pub mod ray;
mod ray_aabb;
mod ray_ball;
mod ray_bounding_sphere;
#[cfg(feature = "alloc")]
mod ray_composite_shape;
mod ray_cuboid;
mod ray_halfspace;
#[cfg(feature = "alloc")]
mod ray_heightfield;
mod ray_round_shape;
mod ray_support_map;
mod ray_triangle;
#[cfg(feature = "alloc")]
mod ray_trimesh;
#[cfg(feature = "alloc")]
mod ray_voxels;
mod simd_ray;
