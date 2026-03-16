//! SimdRealCopy trait for real numbers used by Rapier.

use crate::math::Real;
use na::SimdRealField;

/// The trait for real numbers used by Rapier.
///
/// This includes `f32`, `f64` and their related SIMD types.
pub trait SimdRealCopy: SimdRealField<Element = Real> + Copy {}
impl<T: SimdRealField<Element = Real> + Copy> SimdRealCopy for T {}
