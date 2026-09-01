//! Headless music-notation primitives shared by score renderers and editors.
//!
//! This crate deliberately has no dependency on Makepad's UI or GPU layers.

pub mod smufl;
pub mod symbol;
pub mod units;
pub mod model;

pub use symbol::Symbol;
pub use units::{DesignUnits, FontMetrics, LayoutPoint, StaffPoint, StaffSize, StaffSpaces, StaffStep};

#[cfg(test)]
mod tests;
