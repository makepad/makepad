/// An extension trait for `f32`.
pub trait F64Ext {
    /// Linearly interpolate between `self` and `other` with parameter `t`.
    fn ext_lerp(self, other: f64, t: f64) -> f64;
}

impl F64Ext for f64 {
    fn ext_lerp(self, other: f64, t: f64) -> f64 {
        self * (1.0 - t) + other * t
    }
}
