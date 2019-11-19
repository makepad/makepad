/// An extension trait for `f32`.
pub trait F32Ext {
    /// Linearly interpolate between `self` and `other` with parameter `t`.
    fn lerp(self, other: f32, t: f32) -> f32;
}

impl F32Ext for f32 {
    fn lerp(self, other: f32, t: f32) -> f32 {
        self * (1.0 - t) + other * t
    }
}
