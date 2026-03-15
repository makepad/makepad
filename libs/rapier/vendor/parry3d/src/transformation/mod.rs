//! Transformation, simplification and decomposition of meshes.

#[cfg(feature = "dim3")]
pub(crate) use self::convex_hull2::convex_hull2_idx;
#[cfg(feature = "dim2")]
pub use self::convex_hull2::{convex_hull2 as convex_hull, convex_hull2_idx as convex_hull_idx};
#[cfg(all(feature = "dim3", feature = "std"))]
pub use self::convex_hull3::check_convex_hull;
#[cfg(feature = "dim3")]
pub use self::convex_hull3::{convex_hull, try_convex_hull, ConvexHullError};
#[cfg(all(feature = "dim3", feature = "spade"))]
pub use self::mesh_intersection::{
    intersect_meshes, intersect_meshes_with_tolerances, MeshIntersectionError,
    MeshIntersectionTolerances,
};
pub use self::polygon_intersection::{
    convex_polygons_intersection, convex_polygons_intersection_points,
    convex_polygons_intersection_points_with_tolerances,
    convex_polygons_intersection_with_tolerances, polygons_intersection,
    polygons_intersection_points, PolygonIntersectionTolerances, PolygonsIntersectionError,
    PolylinePointLocation,
};

mod convex_hull2;
#[cfg(feature = "dim3")]
mod convex_hull3;
pub(crate) mod convex_hull_utils;

mod polygon_intersection;
/// Approximate convex decomposition using the VHACD algorithm.
pub mod vhacd;
/// Voxelization of a 2D polyline or 3D triangle mesh.
pub mod voxelization;

#[cfg(feature = "dim2")]
pub(crate) mod ear_clipping;
#[cfg(feature = "dim2")]
pub(crate) mod hertel_mehlhorn;
#[cfg(feature = "dim2")]
pub use hertel_mehlhorn::{hertel_mehlhorn, hertel_mehlhorn_idx};
#[cfg(all(feature = "dim3", feature = "spade"))]
mod mesh_intersection;
#[cfg(feature = "dim3")]
mod to_outline;
#[cfg(feature = "dim2")]
mod to_polyline;
mod to_trimesh;
pub mod utils;

#[cfg(feature = "wavefront")]
mod wavefront;
