/// Round `x` to the nearest integer, breaking ties away from zero.
#[cfg(f16_enabled)]
pub fn roundf16(x: f16) -> f16 {
    super::generic::round(x)
}

/// Round `x` to the nearest integer, breaking ties away from zero.
pub fn roundf(x: f32) -> f32 {
    super::generic::round(x)
}

/// Round `x` to the nearest integer, breaking ties away from zero.
pub fn round(x: f64) -> f64 {
    super::generic::round(x)
}

/// Round `x` to the nearest integer, breaking ties away from zero.
#[cfg(f128_enabled)]
pub fn roundf128(x: f128) -> f128 {
    super::generic::round(x)
}
