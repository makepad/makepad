#[cfg(any(feature = "float_pt", feature = "core"))]
pub mod compatible;
#[cfg(any(feature = "float_pt", feature = "core"))]
pub mod number;

pub mod point;
pub mod rect;
pub mod vector;
