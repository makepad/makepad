// makepad-csg-exact: Exact arithmetic for robust geometric constructions.
//
// Provides expansion arithmetic (exact real numbers as sums of non-overlapping f64s)
// and exact 3D vector operations for computing intersection points without
// rounding error in the corefinement CSG algorithm, and dyadic numbers without an exponent
// range for predicates whose degree takes expansions below the f64 range.

pub mod dyadic;
pub mod exact_vec3;
pub mod expansion;

pub use dyadic::*;
pub use exact_vec3::*;
pub use expansion::*;
