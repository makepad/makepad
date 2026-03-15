//! The GJK algorithm for distance computation.

pub use self::cso_point::CsoPoint;
#[cfg(feature = "dim2")]
pub use self::voronoi_simplex2::VoronoiSimplex;
#[cfg(feature = "dim3")]
pub use self::voronoi_simplex3::VoronoiSimplex;
pub use gjk::*;
pub use special_support_maps::*;

mod cso_point;
mod gjk;
mod special_support_maps;
#[cfg(feature = "dim2")]
mod voronoi_simplex2;
#[cfg(feature = "dim3")]
mod voronoi_simplex3;
