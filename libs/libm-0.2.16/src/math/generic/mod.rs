// Note: generic functions are marked `#[inline]` because, even though generic functions are
// typically inlined, this does not seem to always be the case.

mod ceil;
mod copysign;
mod fabs;
mod floor;
mod fma;
mod fma_wide;
mod round;
mod scalbn;
mod sqrt;
mod trunc;

pub use ceil::ceil;
pub use copysign::copysign;
pub use fabs::fabs;
pub use floor::floor;
pub use fma::fma_round;
pub use fma_wide::fma_wide_round;
pub use round::round;
pub use scalbn::scalbn;
pub use sqrt::sqrt;
pub use trunc::trunc;
