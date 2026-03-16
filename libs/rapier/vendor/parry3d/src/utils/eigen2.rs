//! Re-export of SymmetricEigen2 from glamx.

#[cfg(feature = "f32")]
pub use glamx::SymmetricEigen2;

#[cfg(feature = "f64")]
pub use glamx::DSymmetricEigen2 as SymmetricEigen2;
