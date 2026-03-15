//! ComponentMul trait for element-wise vector operations.

use crate::math::Vector;
use crate::utils::SimdRealCopy;
use na::{Vector2, Vector3};

/// Extension trait for element-wise vector operations
pub trait ComponentMul: Sized {
    /// Element-wise multiplication
    fn component_mul(&self, other: &Self) -> Self;
}

impl ComponentMul for Vector {
    #[inline]
    fn component_mul(&self, other: &Self) -> Self {
        *self * *other
    }
}

// Nalgebra vector implementations for SIMD support
impl<N: SimdRealCopy> ComponentMul for Vector2<N> {
    #[inline]
    fn component_mul(&self, other: &Self) -> Self {
        nalgebra::Vector2::component_mul(self, other)
    }
}

impl<N: SimdRealCopy> ComponentMul for Vector3<N> {
    #[inline]
    fn component_mul(&self, other: &Self) -> Self {
        nalgebra::Vector3::component_mul(self, other)
    }
}
