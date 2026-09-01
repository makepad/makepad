//! Music engraving layout kernel.
//!
//! Pure algorithms over abstract geometric input: no score data model, no
//! fonts, no UI. A caller feeds plain numbers (staff-space lengths, collision
//! rectangles, duration ratios) and gets back positions, break decisions and
//! curve geometry. That isolation keeps the hard math independently testable.
//!
//! The kernel covers six areas:
//!
//! * [`sp`] — the staff-space unit type and small geometry helpers.
//! * [`style`] — the versioned style sheet holding every numeric default.
//! * [`spacing`] — the constrained spring-and-rod horizontal spacing solver.
//! * [`skyline`] — piecewise-linear collision skylines and distance queries.
//! * [`breaking`] — dynamic-programming line and page breaking.
//! * [`curve`] — scored cubic candidates for slurs and ties.
//! * [`incremental`] — the invalidation/cache seam for editor relayout.
//!
//! All coordinates are `f64` staff spaces ([`Sp`]); the y axis grows
//! downward, matching staff notation ("lower" skylines have larger y).
//! Every algorithm is deterministic: identical input produces bit-identical
//! output on every platform (fixed iteration order, no hashing, total-order
//! float sorts).

#![warn(missing_docs)]

pub mod breaking;
pub mod curve;
pub mod incremental;
pub mod skyline;
pub mod sp;
pub mod spacing;
pub mod style;

#[cfg(test)]
mod testutil;

#[cfg(test)]
mod tests;

pub use breaking::*;
pub use curve::*;
pub use incremental::*;
pub use skyline::*;
pub use sp::*;
pub use spacing::*;
pub use style::*;
