// Bitwise differential against rustc 1.97.1 wasm32 compiler-builtins, including
// A single canonical NaN avoids unspecified selection between NaN payloads.
// Every distinct function in Relax core::libm (48 declarations, 46 names).
#![allow(dead_code, unexpected_cfgs)]
use std::fmt::Write;
extern "C" { fn js_console_log(ptr: *const u8, len: usize); }
fn out(s: &str) { unsafe { js_console_log(s.as_ptr(), s.len()) } }
macro_rules! say {
    ($($t:tt)*) => {{ let mut s = String::new(); let _ = writeln!(s, $($t)*); out(&s); }};
}
#[cfg(not(libm_reference))]
use core::libm as math;
#[cfg(libm_reference)]
mod math {
    pub fn sin(x: f64) -> f64 { x.sin() }
    pub fn cos(x: f64) -> f64 { x.cos() }
    pub fn tan(x: f64) -> f64 { x.tan() }
    pub fn asin(x: f64) -> f64 { x.asin() }
    pub fn acos(x: f64) -> f64 { x.acos() }
    pub fn atan(x: f64) -> f64 { x.atan() }
    pub fn atan2(x: f64, y: f64) -> f64 { x.atan2(y) }
    pub fn sinh(x: f64) -> f64 { x.sinh() }
    pub fn cosh(x: f64) -> f64 { x.cosh() }
    pub fn tanh(x: f64) -> f64 { x.tanh() }
    pub fn exp(x: f64) -> f64 { x.exp() }
    pub fn exp2(x: f64) -> f64 { x.exp2() }
    pub fn expm1(x: f64) -> f64 { x.exp_m1() }
    pub fn log(x: f64) -> f64 { x.ln() }
    pub fn log2(x: f64) -> f64 { x.log2() }
    pub fn log10(x: f64) -> f64 { x.log10() }
    pub fn log1p(x: f64) -> f64 { x.ln_1p() }
    pub fn pow(x: f64, y: f64) -> f64 { x.powf(y) }
    pub fn cbrt(x: f64) -> f64 { x.cbrt() }
    pub fn hypot(x: f64, y: f64) -> f64 { x.hypot(y) }
    pub fn round(x: f64) -> f64 { x.round() }
    pub fn fma(x: f64, y: f64, z: f64) -> f64 { x.mul_add(y, z) }
    pub fn fmod(x: f64, y: f64) -> f64 { x % y }
    pub fn sinf(x: f32) -> f32 { x.sin() }
    pub fn cosf(x: f32) -> f32 { x.cos() }
    pub fn tanf(x: f32) -> f32 { x.tan() }
    pub fn asinf(x: f32) -> f32 { x.asin() }
    pub fn acosf(x: f32) -> f32 { x.acos() }
    pub fn atanf(x: f32) -> f32 { x.atan() }
    pub fn atan2f(x: f32, y: f32) -> f32 { x.atan2(y) }
    pub fn sinhf(x: f32) -> f32 { x.sinh() }
    pub fn coshf(x: f32) -> f32 { x.cosh() }
    pub fn tanhf(x: f32) -> f32 { x.tanh() }
    pub fn expf(x: f32) -> f32 { x.exp() }
    pub fn exp2f(x: f32) -> f32 { x.exp2() }
    pub fn expm1f(x: f32) -> f32 { x.exp_m1() }
    pub fn logf(x: f32) -> f32 { x.ln() }
    pub fn log2f(x: f32) -> f32 { x.log2() }
    pub fn log10f(x: f32) -> f32 { x.log10() }
    pub fn log1pf(x: f32) -> f32 { x.ln_1p() }
    pub fn powf(x: f32, y: f32) -> f32 { x.powf(y) }
    pub fn cbrtf(x: f32) -> f32 { x.cbrt() }
    pub fn hypotf(x: f32, y: f32) -> f32 { x.hypot(y) }
    pub fn roundf(x: f32) -> f32 { x.round() }
    pub fn fmaf(x: f32, y: f32, z: f32) -> f32 { x.mul_add(y, z) }
    pub fn fmodf(x: f32, y: f32) -> f32 { x % y }
}
fn body() {
    let xs: [f64; 22] = [0.0, -0.0, 1.0, -1.0, 0.5, -0.5, 1.5, -1.5,
        2.0, -2.0, 3.141592653589793, 10.0, -10.0, 1e20, -1e20,
        f64::MAX, f64::MIN_POSITIVE, f64::from_bits(1), -f64::from_bits(1),
        f64::INFINITY, f64::NEG_INFINITY, f64::NAN];
    for (i, x) in xs.iter().copied().enumerate() {
        let x = std::hint::black_box(x);
        say!("sin {} {:x}", i, math::sin(x).to_bits());
        say!("cos {} {:x}", i, math::cos(x).to_bits());
        say!("tan {} {:x}", i, math::tan(x).to_bits());
        say!("asin {} {:x}", i, math::asin(x).to_bits());
        say!("acos {} {:x}", i, math::acos(x).to_bits());
        say!("atan {} {:x}", i, math::atan(x).to_bits());
        say!("sinh {} {:x}", i, math::sinh(x).to_bits());
        say!("cosh {} {:x}", i, math::cosh(x).to_bits());
        say!("tanh {} {:x}", i, math::tanh(x).to_bits());
        say!("exp {} {:x}", i, math::exp(x).to_bits());
        say!("exp2 {} {:x}", i, math::exp2(x).to_bits());
        say!("expm1 {} {:x}", i, math::expm1(x).to_bits());
        say!("log {} {:x}", i, math::log(x).to_bits());
        say!("log2 {} {:x}", i, math::log2(x).to_bits());
        say!("log10 {} {:x}", i, math::log10(x).to_bits());
        say!("log1p {} {:x}", i, math::log1p(x).to_bits());
        say!("cbrt {} {:x}", i, math::cbrt(x).to_bits());
        say!("round {} {:x}", i, math::round(x).to_bits());
        for (j, y) in xs.iter().copied().enumerate() {
            let y = std::hint::black_box(y);
            say!("pow {} {} {:x}", i, j, math::pow(x, y).to_bits());
            say!("hypot {} {} {:x}", i, j, math::hypot(x, y).to_bits());
            say!("atan2 {} {} {:x}", i, j, math::atan2(x, y).to_bits());
            say!("fmod {} {} {:x}", i, j, (x % y).to_bits());
            for (k, z) in [0.0, -0.0, 1.0, -1.0, f64::from_bits(1), f64::INFINITY, f64::NAN].iter().copied().enumerate() {
                say!("fma {} {} {} {:x}", i, j, k, math::fma(x, y, std::hint::black_box(z)).to_bits());
            }
        }
    }
    let xs: [f32; 22] = [0.0, -0.0, 1.0, -1.0, 0.5, -0.5, 1.5, -1.5,
        2.0, -2.0, 3.141592653589793, 10.0, -10.0, 1e20, -1e20,
        f32::MAX, f32::MIN_POSITIVE, f32::from_bits(1), -f32::from_bits(1),
        f32::INFINITY, f32::NEG_INFINITY, f32::NAN];
    for (i, x) in xs.iter().copied().enumerate() {
        let x = std::hint::black_box(x);
        say!("sinf {} {:x}", i, math::sinf(x).to_bits());
        say!("cosf {} {:x}", i, math::cosf(x).to_bits());
        say!("tanf {} {:x}", i, math::tanf(x).to_bits());
        say!("asinf {} {:x}", i, math::asinf(x).to_bits());
        say!("acosf {} {:x}", i, math::acosf(x).to_bits());
        say!("atanf {} {:x}", i, math::atanf(x).to_bits());
        say!("sinhf {} {:x}", i, math::sinhf(x).to_bits());
        say!("coshf {} {:x}", i, math::coshf(x).to_bits());
        say!("tanhf {} {:x}", i, math::tanhf(x).to_bits());
        say!("expf {} {:x}", i, math::expf(x).to_bits());
        say!("exp2f {} {:x}", i, math::exp2f(x).to_bits());
        say!("expm1f {} {:x}", i, math::expm1f(x).to_bits());
        say!("logf {} {:x}", i, math::logf(x).to_bits());
        say!("log2f {} {:x}", i, math::log2f(x).to_bits());
        say!("log10f {} {:x}", i, math::log10f(x).to_bits());
        say!("log1pf {} {:x}", i, math::log1pf(x).to_bits());
        say!("cbrtf {} {:x}", i, math::cbrtf(x).to_bits());
        say!("roundf {} {:x}", i, math::roundf(x).to_bits());
        for (j, y) in xs.iter().copied().enumerate() {
            let y = std::hint::black_box(y);
            say!("powf {} {} {:x}", i, j, math::powf(x, y).to_bits());
            say!("hypotf {} {} {:x}", i, j, math::hypotf(x, y).to_bits());
            say!("atan2f {} {} {:x}", i, j, math::atan2f(x, y).to_bits());
            say!("fmodf {} {} {:x}", i, j, (x % y).to_bits());
            for (k, z) in [0.0, -0.0, 1.0, -1.0, f32::from_bits(1), f32::INFINITY, f32::NAN].iter().copied().enumerate() {
                say!("fmaf {} {} {} {:x}", i, j, k, math::fmaf(x, y, std::hint::black_box(z)).to_bits());
            }
        }
    }
}
#[no_mangle]
pub extern "C" fn run() { body() }
fn main() { body() }
