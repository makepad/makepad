/*!
parry
========

**parry** is a 2 and 3-dimensional geometric library written with
the rust programming language.

*/

#![deny(non_camel_case_types)]
#![deny(unused_parens)]
#![deny(non_upper_case_globals)]
#![deny(unused_results)]
#![deny(unused_qualifications)]
#![warn(missing_docs)]
#![warn(unused_imports)]
#![allow(missing_copy_implementations)]
#![allow(clippy::too_many_arguments)] // Maybe revisit this one later.
#![allow(clippy::module_inception)]
#![allow(clippy::manual_range_contains)] // This usually makes it way more verbose that it could be.
#![allow(clippy::type_complexity)] // Complains about closures that are fairly simple.
#![cfg_attr(feature = "dim2", doc(html_root_url = "https://docs.rs/parry2d"))]
#![cfg_attr(feature = "dim3", doc(html_root_url = "https://docs.rs/parry3d"))]
#![no_std]

#[cfg(all(
    feature = "simd-is-enabled",
    not(feature = "simd-stable"),
    not(feature = "simd-nightly")
))]
std::compile_error!("The `simd-is-enabled` feature should not be enabled explicitly. Please enable the `simd-stable` or the `simd-nightly` feature instead.");
#[cfg(all(feature = "simd-is-enabled", feature = "enhanced-determinism"))]
std::compile_error!(
    "SIMD cannot be enabled when the `enhanced-determinism` feature is also enabled."
);
#[cfg(any(feature = "simd-stable", feature = "simd-nightly"))]
std::compile_error!("The stripped deterministic build does not support SIMD features.");
#[cfg(feature = "spade")]
std::compile_error!("The stripped deterministic build does not support the `spade` feature.");

#[cfg(feature = "simd-is-enabled")]
#[allow(unused_macros)]
macro_rules! array(
    ($callback: expr; SIMD_WIDTH) => {
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

#[cfg(feature = "std")]
extern crate std;

#[cfg(feature = "alloc")]
#[cfg_attr(test, macro_use)]
extern crate alloc;

#[cfg(feature = "serde")]
#[macro_use]
extern crate serde;
#[macro_use]
extern crate approx;
extern crate num_traits as num;

pub extern crate either;
pub extern crate glamx;
pub extern crate simba;

pub mod bounding_volume;
pub mod mass_properties;
pub mod math;
pub mod partitioning;
pub mod query;
pub mod shape;
#[cfg(feature = "alloc")]
pub mod transformation;
pub mod utils;

#[cfg(not(feature = "simd-is-enabled"))]
mod simd {
    /// The number of lanes of a SIMD number.
    pub const SIMD_WIDTH: usize = 1;
    /// SIMD_WIDTH - 1
    pub const SIMD_LAST_INDEX: usize = 0;

    /// A SIMD float with SIMD_WIDTH lanes.
    #[cfg(feature = "f32")]
    pub type SimdReal = f32;

    /// A SIMD float with SIMD_WIDTH lanes.
    #[cfg(feature = "f64")]
    pub type SimdReal = f64;

    /// A SIMD bool with SIMD_WIDTH lanes.
    pub type SimdBool = bool;
}

#[cfg(feature = "simd-is-enabled")]
mod simd {
    #[cfg(all(feature = "simd-nightly", feature = "f32"))]
    pub use simba::simd::{f32x4 as SimdReal, mask32x4 as SimdBool};
    #[cfg(all(feature = "simd-stable", feature = "f32"))]
    pub use simba::simd::{WideBoolF32x4 as SimdBool, WideF32x4 as SimdReal};

    #[cfg(all(feature = "simd-nightly", feature = "f64"))]
    pub use simba::simd::{f64x4 as SimdReal, mask64x4 as SimdBool};
    #[cfg(all(feature = "simd-stable", feature = "f64"))]
    pub use simba::simd::{WideBoolF64x4 as SimdBool, WideF64x4 as SimdReal};

    /// The number of lanes of a SIMD number.
    pub const SIMD_WIDTH: usize = 4;
    /// SIMD_WIDTH - 1
    pub const SIMD_LAST_INDEX: usize = 3;
}
