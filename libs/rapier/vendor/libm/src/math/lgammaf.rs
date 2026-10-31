use super::lgammaf_r;

/// The natural logarithm of the
/// [Gamma function](https://en.wikipedia.org/wiki/Gamma_function) (f32).
pub fn lgammaf(x: f32) -> f32 {
    lgammaf_r(x).0
}
