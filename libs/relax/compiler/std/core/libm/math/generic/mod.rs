//! Generic implementations that are shared by multiple types.
//!
//! Implementation and usage notes:
//!
//! * Generic functions are marked `#[inline]` because, even though generic functions are
//!   typically inlined, we seem to occasionally run into exceptions.
//! * Tests usually live wherever the functions are consumed (e.g. `src/ceil`) so they can be
//!   reused to test arch-specific implementations.

mod copysign;
mod fabs;
mod floor;
mod fma;
mod fma_wide;
mod fmod;
mod round;
mod scalbn;
mod sqrt;
mod trunc;

pub use copysign::copysign;
pub use fabs::fabs;
pub use floor::floor_status;
pub use fma::fma_round;
pub use fma_wide::fma_wide_round;
pub use fmod::fmod;
pub use round::round;
pub use scalbn::scalbn;
#[cfg(test)]
pub use sqrt::SqrtHelper;
pub use sqrt::sqrt_round;
pub use trunc::trunc_status;
