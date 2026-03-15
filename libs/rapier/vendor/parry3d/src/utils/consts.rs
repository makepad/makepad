#![allow(clippy::excessive_precision)] // Clippy will complain in f32 mode, but we also need these consts for f64.

use crate::math::Real;

// pub(crate) const SIN_10_DEGREES: Real = 0.17364817766;
// pub(crate) const COS_10_DEGREES: Real = 0.98480775301;
// pub(crate) const COS_45_DEGREES: Real = 0.70710678118;
// pub(crate) const SIN_45_DEGREES: Real = COS_45_DEGREES;
pub(crate) const COS_1_DEGREES: Real = 0.99984769515;
// pub(crate) const COS_5_DEGREES: Real = 0.99619469809;
pub(crate) const COS_FRAC_PI_8: Real = 0.92387953251;
#[cfg(feature = "dim2")]
pub(crate) const SIN_FRAC_PI_8: Real = 0.38268343236;
