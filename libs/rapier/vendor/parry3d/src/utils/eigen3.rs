//! Re-export of SymmetricEigen3 from glamx.

#[cfg(feature = "f32")]
pub use glamx::SymmetricEigen3;

#[cfg(feature = "f64")]
pub use glamx::DSymmetricEigen3 as SymmetricEigen3;
