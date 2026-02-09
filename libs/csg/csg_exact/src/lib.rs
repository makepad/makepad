// makepad-csg-exact: Exact arithmetic for robust geometric constructions.
//
// Provides expansion arithmetic (exact real numbers as sums of non-overlapping f64s)
// and exact 3D vector operations for computing intersection points without
// rounding error in the corefinement CSG algorithm.

pub mod exact_vec3;
pub mod expansion;

pub use exact_vec3::*;
pub use expansion::*;
