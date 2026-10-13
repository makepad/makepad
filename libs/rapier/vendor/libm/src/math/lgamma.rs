use super::lgamma_r;

/// The natural logarithm of the
/// [Gamma function](https://en.wikipedia.org/wiki/Gamma_function) (f64).
pub fn lgamma(x: f64) -> f64 {
    lgamma_r(x).0
}
