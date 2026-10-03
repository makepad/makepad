//! The platform math library (what Rust's std calls for these functions, so results match
//! rustc builds on the same platform bit for bit).

extern "C" {
    pub fn sin(x: f64) -> f64;
    pub fn cos(x: f64) -> f64;
    pub fn tan(x: f64) -> f64;
    pub fn asin(x: f64) -> f64;
    pub fn acos(x: f64) -> f64;
    pub fn atan(x: f64) -> f64;
    pub fn atan2(y: f64, x: f64) -> f64;
    pub fn sinh(x: f64) -> f64;
    pub fn cosh(x: f64) -> f64;
    pub fn tanh(x: f64) -> f64;
    pub fn exp(x: f64) -> f64;
    pub fn exp2(x: f64) -> f64;
    pub fn expm1(x: f64) -> f64;
    pub fn log(x: f64) -> f64;
    pub fn log2(x: f64) -> f64;
    pub fn log10(x: f64) -> f64;
    pub fn log1p(x: f64) -> f64;
    pub fn pow(x: f64, y: f64) -> f64;
    pub fn cbrt(x: f64) -> f64;
    pub fn hypot(x: f64, y: f64) -> f64;
    pub fn round(x: f64) -> f64;
    pub fn fma(x: f64, y: f64, z: f64) -> f64;
    pub fn fmod(x: f64, y: f64) -> f64;
    pub fn sinf(x: f32) -> f32;
    pub fn cosf(x: f32) -> f32;
    pub fn tanf(x: f32) -> f32;
    pub fn asinf(x: f32) -> f32;
    pub fn acosf(x: f32) -> f32;
    pub fn atanf(x: f32) -> f32;
    pub fn atan2f(y: f32, x: f32) -> f32;
    pub fn sinhf(x: f32) -> f32;
    pub fn coshf(x: f32) -> f32;
    pub fn tanhf(x: f32) -> f32;
    pub fn expf(x: f32) -> f32;
    pub fn exp2f(x: f32) -> f32;
    pub fn expm1f(x: f32) -> f32;
    pub fn logf(x: f32) -> f32;
    pub fn log2f(x: f32) -> f32;
    pub fn log10f(x: f32) -> f32;
    pub fn log1pf(x: f32) -> f32;
    pub fn powf(x: f32, y: f32) -> f32;
    pub fn cbrtf(x: f32) -> f32;
    pub fn hypotf(x: f32, y: f32) -> f32;
    pub fn roundf(x: f32) -> f32;
    pub fn fmaf(x: f32, y: f32, z: f32) -> f32;
    pub fn fmodf(x: f32, y: f32) -> f32;
}
