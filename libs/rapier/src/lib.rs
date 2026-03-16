//! # Rapier
//!
//! Rapier is a set of two Rust crates `rapier2d` and `rapier3d` for efficient cross-platform
//! physics simulation. It target application include video games, animation, robotics, etc.
//!
//! Rapier has some unique features for collaborative applications:
//! - The ability to snapshot the state of the physics engine, and restore it later.
//! - The ability to run a perfectly deterministic simulation on different machine, as long as they
//!   are compliant with the IEEE 754-2008 floating point standard.
//!
//! User documentation for Rapier is on [the official Rapier site](https://rapier.rs/docs/).

#![deny(bare_trait_objects)]
#![warn(missing_docs)]
#![allow(clippy::too_many_arguments)]
#![allow(clippy::needless_range_loop)] // TODO: remove this? I find that in the math code using indices adds clarity.
#![allow(clippy::module_inception)]
#![cfg_attr(feature = "simd-nightly", feature(portable_simd))]

#[cfg(all(feature = "dim2", feature = "f32"))]
pub extern crate parry2d as parry;
#[cfg(all(feature = "dim2", feature = "f64"))]
pub extern crate parry2d_f64 as parry;
#[cfg(all(feature = "dim3", feature = "f32"))]
pub extern crate parry3d as parry;
#[cfg(all(feature = "dim3", feature = "f64"))]
pub extern crate parry3d_f64 as parry;

pub extern crate nalgebra as na;
#[cfg(feature = "serde-serialize")]
#[macro_use]
extern crate serde;
extern crate num_traits as num;

#[cfg(feature = "parallel")]
pub use rayon;

#[cfg(all(
    feature = "simd-is-enabled",
    not(feature = "simd-stable"),
    not(feature = "simd-nightly")
))]
std::compile_error!(
    "The `simd-is-enabled` feature should not be enabled explicitly. Please enable the `simd-stable` or the `simd-nightly` feature instead."
);
#[cfg(all(feature = "simd-is-enabled", feature = "enhanced-determinism"))]
std::compile_error!(
    "SIMD cannot be enabled when the `enhanced-determinism` feature is also enabled."
);
#[cfg(any(feature = "simd-stable", feature = "simd-nightly"))]
std::compile_error!("The stripped deterministic build does not support SIMD features.");

macro_rules! enable_flush_to_zero(
    () => {
        let _flush_to_zero = crate::utils::FlushToZeroDenormalsAreZeroFlags::flush_denormal_to_zero();
    }
);

#[allow(unused_macros)]
macro_rules! gather(
    ($callback: expr) => {
        {
            #[inline(always)]
            #[allow(dead_code)]
            #[cfg(not(feature = "simd-is-enabled"))]
            fn create_arr<T>(mut callback: impl FnMut(usize) -> T) -> T {
                callback(0usize)
            }

            #[inline(always)]
            #[allow(dead_code)]
            #[cfg(feature = "simd-is-enabled")]
            fn create_arr<T>(mut callback: impl FnMut(usize) -> T) -> [T; SIMD_WIDTH] {
                [callback(0usize), callback(1usize), callback(2usize), callback(3usize)]
            }


            create_arr($callback)
        }
    }
);

macro_rules! array(
    ($callback: expr) => {
        {
            #[inline(always)]
            #[allow(dead_code)]
            fn create_arr<T>(mut callback: impl FnMut(usize) -> T) -> [T; SIMD_WIDTH] {
                #[cfg(not(feature = "simd-is-enabled"))]
                return [callback(0usize)];
                #[cfg(feature = "simd-is-enabled")]
                return [callback(0usize), callback(1usize), callback(2usize), callback(3usize)];
            }

            create_arr($callback)
        }
    }
);

#[allow(unused_macros)]
macro_rules! par_iter {
    ($t: expr) => {{
        #[cfg(not(feature = "parallel"))]
        let it = $t.iter();

        #[cfg(feature = "parallel")]
        let it = $t.par_iter();
        it
    }};
}

macro_rules! par_iter_mut {
    ($t: expr) => {{
        #[cfg(not(feature = "parallel"))]
        let it = $t.iter_mut();

        #[cfg(feature = "parallel")]
        let it = $t.par_iter_mut();
        it
    }};
}

// macro_rules! par_chunks_mut {
//     ($t: expr, $sz: expr) => {{
//         #[cfg(not(feature = "parallel"))]
//         let it = $t.chunks_mut($sz);
//
//         #[cfg(feature = "parallel")]
//         let it = $t.par_chunks_mut($sz);
//         it
//     }};
// }

#[allow(unused_macros)]
macro_rules! try_ret {
    ($val: expr) => {
        try_ret!($val, ())
    };
    ($val: expr, $ret: expr) => {
        if let Some(val) = $val {
            val
        } else {
            return $ret;
        }
    };
}

// macro_rules! try_continue {
//     ($val: expr) => {
//         if let Some(val) = $val {
//             val
//         } else {
//             continue;
//         }
//     };
// }

pub(crate) const INVALID_U32: u32 = u32::MAX;
pub(crate) const INVALID_USIZE: usize = INVALID_U32 as usize;

/// The string version of Rapier.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod control;
pub mod counters;
pub mod data;
pub mod dynamics;
pub mod geometry;
pub mod pipeline;
pub mod utils;

/// Elementary mathematical entities (vectors, matrices, isometries, etc).
pub mod math {
    pub use parry::math::*;

    /// Creates a rotation from an angular vector.
    ///
    /// In 2D, the angular vector is a scalar angle in radians.
    /// In 3D, the angular vector is a scaled axis-angle (axis * angle).
    #[cfg(feature = "dim2")]
    #[inline]
    pub fn rotation_from_angle(angle: AngVector) -> Rotation {
        Rotation::new(angle)
    }

    /// Creates a rotation from an angular vector.
    ///
    /// In 2D, the angular vector is a scalar angle in radians.
    /// In 3D, the angular vector is a scaled axis-angle (axis * angle).
    #[cfg(feature = "dim3")]
    #[inline]
    pub fn rotation_from_angle(angle: AngVector) -> Rotation {
        Rotation::from_scaled_axis(angle)
    }

    // Generic nalgebra type aliases for SIMD/generic code (where N is SimdReal or similar)
    // These use nalgebra types which support generic scalars
    // Note: These override the non-generic versions above when used with <T> syntax

    /// Generic vector type (nalgebra) for SoA SIMD code
    #[cfg(feature = "dim2")]
    pub type SimdVector<N> = na::Vector2<N>;
    /// Generic vector type (nalgebra) for SoA SIMD code
    #[cfg(feature = "dim3")]
    pub type SimdVector<N> = na::Vector3<N>;
    /// Generic angular vector type (nalgebra) for SoA SIMD code
    #[cfg(feature = "dim2")]
    pub type SimdAngVector<N> = N;
    /// Generic angular vector type (nalgebra) for SoA SIMD code
    #[cfg(feature = "dim3")]
    pub type SimdAngVector<N> = na::Vector3<N>;
    /// Generic point type (nalgebra) for SoA SIMD code
    #[cfg(feature = "dim2")]
    pub type SimdPoint<N> = na::Point2<N>;
    /// Generic point type (nalgebra) for SoA SIMD code
    #[cfg(feature = "dim3")]
    pub type SimdPoint<N> = na::Point3<N>;
    /// Generic isometry type (nalgebra) for SoA SIMD code
    #[cfg(feature = "dim2")]
    pub type SimdPose<N> = na::Isometry2<N>;
    /// Generic isometry type (nalgebra) for SoA SIMD code
    #[cfg(feature = "dim3")]
    pub type SimdPose<N> = na::Isometry3<N>;
    /// Generic rotation type (nalgebra) for SoA SIMD code
    #[cfg(feature = "dim2")]
    pub type SimdRotation<N> = na::UnitComplex<N>;
    /// Generic rotation type (nalgebra) for SoA SIMD code
    #[cfg(feature = "dim3")]
    pub type SimdRotation<N> = na::UnitQuaternion<N>;
    /// Generic angular inertia type for SoA SIMD code (scalar in 2D, SdpMatrix3 in 3D)
    #[cfg(feature = "dim2")]
    pub type SimdAngularInertia<N> = N;
    /// Generic angular inertia type for SoA SIMD code (scalar in 2D, SdpMatrix3 in 3D)
    #[cfg(feature = "dim3")]
    pub type SimdAngularInertia<N> = parry::utils::SdpMatrix3<N>;
    /// Generic 2D/3D square matrix for SoA SIMD code
    #[cfg(feature = "dim2")]
    pub type SimdMatrix<N> = na::Matrix2<N>;
    /// Generic 2D/3D square matrix for SoA SIMD code
    #[cfg(feature = "dim3")]
    pub type SimdMatrix<N> = na::Matrix3<N>;

    // Dimension types for nalgebra matrix operations (used in multibody code)
    /// The dimension type constant (U2 for 2D).
    #[cfg(feature = "dim2")]
    pub type Dim = na::U2;
    /// The dimension type constant (U3 for 3D).
    #[cfg(feature = "dim3")]
    pub type Dim = na::U3;
    /// The angular dimension type constant (U1 for 2D).
    #[cfg(feature = "dim2")]
    pub type AngDim = na::U1;
    /// The angular dimension type constant (U3 for 3D).
    #[cfg(feature = "dim3")]
    pub type AngDim = na::U3;

    /// Dynamic vector type for multibody/solver code
    pub type DVector = na::DVector<Real>;
    /// Dynamic matrix type for multibody/solver code
    pub type DMatrix = na::DMatrix<Real>;

    /*
     * 2D
     */
    /// Max number of pairs of contact points from the same
    /// contact manifold that can be solved as part of a
    /// single contact constraint.
    #[cfg(feature = "dim2")]
    pub const MAX_MANIFOLD_POINTS: usize = 2;

    /// The type of a constraint Jacobian in twist coordinates.
    #[cfg(feature = "dim2")]
    pub type Jacobian<N> = na::Matrix3xX<N>;

    /// The type of a slice of the constraint Jacobian in twist coordinates.
    #[cfg(feature = "dim2")]
    pub type JacobianView<'a, N> = na::MatrixView3xX<'a, N>;

    /// The type of a mutable slice of the constraint Jacobian in twist coordinates.
    #[cfg(feature = "dim2")]
    pub type JacobianViewMut<'a, N> = na::MatrixViewMut3xX<'a, N>;

    /// The type of impulse applied for friction constraints.
    #[cfg(feature = "dim2")]
    pub type TangentImpulse<N> = na::Vector1<N>;

    /// The maximum number of possible rotations and translations of a rigid body.
    #[cfg(feature = "dim2")]
    pub const SPATIAL_DIM: usize = 3;

    /// The maximum number of rotational degrees of freedom of a rigid-body.
    #[cfg(feature = "dim2")]
    pub const ANG_DIM: usize = 1;

    /*
     * 3D
     */
    /// Max number of pairs of contact points from the same
    /// contact manifold that can be solved as part of a
    /// single contact constraint.
    #[cfg(feature = "dim3")]
    pub const MAX_MANIFOLD_POINTS: usize = 4;

    /// The type of a constraint Jacobian in twist coordinates.
    #[cfg(feature = "dim3")]
    pub type Jacobian<N> = na::Matrix6xX<N>;

    /// The type of a slice of the constraint Jacobian in twist coordinates.
    #[cfg(feature = "dim3")]
    pub type JacobianView<'a, N> = na::MatrixView6xX<'a, N>;

    /// The type of a mutable slice of the constraint Jacobian in twist coordinates.
    #[cfg(feature = "dim3")]
    pub type JacobianViewMut<'a, N> = na::MatrixViewMut6xX<'a, N>;

    /// The type of impulse applied for friction constraints.
    #[cfg(feature = "dim3")]
    pub type TangentImpulse<N> = na::Vector2<N>;

    /// The maximum number of possible rotations and translations of a rigid body.
    #[cfg(feature = "dim3")]
    pub const SPATIAL_DIM: usize = 6;

    /// The maximum number of rotational degrees of freedom of a rigid-body.
    #[cfg(feature = "dim3")]
    pub const ANG_DIM: usize = 3;
}

/// Prelude containing the common types defined by Rapier.
pub mod prelude {
    pub use crate::dynamics::*;
    pub use crate::geometry::*;
    pub use crate::math::*;
    pub use crate::pipeline::*;
    pub extern crate nalgebra;
}
