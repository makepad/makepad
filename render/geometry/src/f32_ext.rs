pub trait F32Ext {
    fn lerp(self, other: f32, t: f32) -> f32;
}

impl F32Ext for f32 {
    fn lerp(self, other: f32, t: f32) -> f32 {
        self * (1.0 - t) + other * t
    }
}