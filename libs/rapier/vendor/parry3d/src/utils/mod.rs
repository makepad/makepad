//! Various unsorted geometrical and logical operators.

pub use self::ccw_face_normal::ccw_face_normal;
pub use self::center::center;

#[cfg(feature = "alloc")]
pub use self::array2::Array2;
#[cfg(feature = "dim3")]
#[cfg(feature = "alloc")]
pub use self::cleanup::remove_unused_points;
pub use self::eigen2::SymmetricEigen2;
pub use self::eigen3::SymmetricEigen3;
pub(crate) use self::inv::inv;
pub use self::isometry_ops::{PoseOps, PoseOpt};
pub use self::median::median;
pub use self::point_cloud_support_point::{
    point_cloud_support_point, point_cloud_support_point_id,
};
pub use self::point_in_poly2d::{point_in_convex_poly2d, point_in_poly2d};
pub use self::sdp_matrix::{SdpMatrix2, SdpMatrix3};

#[cfg(feature = "alloc")]
pub use self::vec_map::VecMap;

pub use self::as_bytes::AsBytes;
#[cfg(any(feature = "alloc", feature = "std"))]
pub(crate) use self::consts::*;
pub use self::cov::{center_cov, cov};
pub use self::hashable_partial_eq::HashablePartialEq;
#[cfg(feature = "alloc")]
pub use self::interval::{find_root_intervals, find_root_intervals_to, Interval, IntervalFunction};
pub use self::obb::obb;
pub use self::segments_intersection::{segments_intersection2d, SegmentsIntersection};
#[cfg(feature = "dim3")]
pub use self::sort::sort2;
pub use self::sort::sort3;
pub use self::sorted_pair::SortedPair;
#[cfg(all(feature = "dim3", feature = "spade"))]
pub(crate) use self::spade::sanitize_spade_point;
pub(crate) use self::wops::{WBasis, WCross, WSign};

#[cfg(feature = "simd-is-enabled")]
#[allow(unused_imports)]
pub(crate) use self::wops::simd_swap;

#[cfg(feature = "alloc")]
mod array2;
mod as_bytes;
mod ccw_face_normal;
mod center;
#[cfg(feature = "dim3")]
#[cfg(feature = "alloc")]
mod cleanup;
#[cfg(any(feature = "alloc", feature = "std"))]
mod consts;
mod cov;
#[cfg(feature = "enhanced-determinism")]
mod fx_hasher;
mod hashable_partial_eq;
#[cfg(feature = "alloc")]
pub mod hashmap;
#[cfg(feature = "alloc")]
pub mod hashset;
#[cfg(feature = "alloc")]
mod interval;
mod inv;
mod isometry_ops;
mod median;
pub mod morton;
mod obb;
mod point_cloud_support_point;
mod point_in_poly2d;
#[cfg(feature = "dim2")]
pub mod point_in_triangle;
mod sdp_matrix;
mod segments_intersection;
mod sort;
mod sorted_pair;
#[cfg(all(feature = "dim3", feature = "spade"))]
mod spade;
mod wops;

mod eigen2;
mod eigen3;
#[cfg(feature = "alloc")]
mod vec_map;
