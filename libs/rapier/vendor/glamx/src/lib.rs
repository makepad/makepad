//! # glamx - Extensions for glam
//!
//! This crate provides additional types and utilities for the `glam` math library:
//!
//! - [`Rot2`] / [`DRot2`]: 2D rotations represented as unit complex numbers
//! - [`Rot3`] / [`DRot3`]: 3D rotations (re-exports of glam's quaternions)
//! - [`Pose2`] / [`DPose2`]: 2D rigid body transformations (rotation + translation)
//! - [`Pose3`] / [`DPose3`]: 3D rigid body transformations (rotation + translation)
//! - [`MatExt`]: Extension traits for glam matrix types
//! - [`SymmetricEigen2`] / [`SymmetricEigen3`]: Eigendecomposition for symmetric matrices
//! - [`Svd2`] / [`Svd3`]: Singular Value Decomposition for 2x2 and 3x3 matrices
//!
//! ## Naming Convention
//!
//! Types without a prefix use `f32` precision, while types with a `D` prefix use `f64` precision:
//! - `Rot2` (f32) / `DRot2` (f64)
//! - `Pose2` (f32) / `DPose2` (f64)
//! - `SymmetricEigen2` (f32) / `DSymmetricEigen2` (f64)
//!
//! ## Feature Flags
//!
//! - `std` (default): Enables standard library support
//! - `serde`: Enables serialization support via serde
//! - `bytemuck`: Enables bytemuck derive for Rot2/DRot2
//! - `nalgebra`: Enables conversions to/from nalgebra types
//! - `libm`: Uses libm for no_std math operations
//!
//! ## glam Re-exports
//!
//! This crate re-exports all of glam's types so you can use it as a drop-in replacement without
//! an explicit dependency to `glam`.
//! Alternatively, depending on your coding style preferences, you can use the `glam` re-export.

#![no_std]

#[cfg(feature = "std")]
extern crate std;

extern crate alloc;

// Re-export approx for convenience
#[cfg(feature = "approx")]
pub use approx;
/// Re-export of the glam crate.
pub use glam;

mod eigen2;
mod eigen3;
mod matrix_ext;
mod pose2;
mod pose3;
mod rot2;
mod rot3;
mod svd2;
mod svd3;

pub use eigen2::{DSymmetricEigen2, SymmetricEigen2};
pub use eigen3::{DSymmetricEigen3, SymmetricEigen3, SymmetricEigen3A};
pub use glam::*;
pub use matrix_ext::MatExt;
pub use pose2::{DPose2, Pose2};
pub use pose3::{DPose3, Pose3, Pose3A};
pub use rot2::{DRot2, Rot2};
pub use rot3::{DRot3, Rot3};
pub use svd2::{DSvd2, Svd2};
pub use svd3::{DSvd3, Svd3, Svd3A};

/// Prelude module for convenient imports.
///
/// This module re-exports all public types and traits.
pub mod prelude {
    pub use crate::eigen2::{DSymmetricEigen2, SymmetricEigen2};
    pub use crate::eigen3::{DSymmetricEigen3, SymmetricEigen3, SymmetricEigen3A};
    pub use crate::matrix_ext::MatExt;
    pub use crate::pose2::{DPose2, Pose2};
    pub use crate::pose3::{DPose3, Pose3};
    pub use crate::rot2::{DRot2, Rot2};
    pub use crate::rot3::{DRot3, Rot3};
    pub use crate::svd2::{DSvd2, Svd2};
    pub use crate::svd3::{DSvd3, Svd3, Svd3A};
}
