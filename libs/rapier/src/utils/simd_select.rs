//! SimdSelect trait for conditional selection.

#[cfg(feature = "simd-is-enabled")]
use crate::math::SimdReal;
use crate::math::{Real, Vector};
use crate::utils::ScalarType;

/// Trait for conditional selection between two values.
pub trait SimdSelect<N: ScalarType> {
    /// Select between `self` and `if_false` based on `condition`.
    fn select(self, condition: N::SimdBool, if_false: Self) -> Self;
}

impl SimdSelect<Real> for Vector {
    #[inline]
    fn select(self, condition: bool, if_false: Self) -> Self {
        if condition { self } else { if_false }
    }
}

#[cfg(feature = "simd-is-enabled")]
impl SimdSelect<SimdReal> for na::Vector3<SimdReal> {
    #[inline]
    fn select(self, condition: <SimdReal as na::SimdValue>::SimdBool, if_false: Self) -> Self {
        na::SimdValue::select(self, condition, if_false)
    }
}
