//! 3D rotation types (re-exported from glam).
//!
//! This module provides type aliases for glam's quaternion types for consistency
//! with the naming convention used by other types in this crate.

/// A 3D rotation represented as a unit quaternion (f32 precision).
///
/// This is a direct re-export of `glam::Quat`.
pub type Rot3 = glam::Quat;

/// A 3D rotation represented as a unit quaternion (f64 precision).
///
/// This is a direct re-export of `glam::DQuat`.
pub type DRot3 = glam::DQuat;
