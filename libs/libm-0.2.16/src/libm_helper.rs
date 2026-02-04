use core::marker::PhantomData;

use crate::*;

/// Generic helper for libm functions, abstracting over f32 and f64. <br/>
/// # Type Parameter:
/// - `T`: Either `f32` or `f64`
///
/// # Examples
/// ```rust
/// use libm::{self, Libm};
///
/// const PI_F32: f32 = 3.1415927410e+00;
/// const PI_F64: f64 = 3.1415926535897931160e+00;
///
/// assert!(Libm::<f32>::cos(0.0f32) == libm::cosf(0.0));
/// assert!(Libm::<f32>::sin(PI_F32) == libm::sinf(PI_F32));
///
/// assert!(Libm::<f64>::cos(0.0f64) == libm::cos(0.0));
/// assert!(Libm::<f64>::sin(PI_F64) == libm::sin(PI_F64));
/// ```
pub struct Libm<T>(PhantomData<T>);

macro_rules! libm_helper {
    ($t:ident, funcs: $funcs:tt) => {
        impl Libm<$t> {
            #![allow(unused_parens)]

            libm_helper! { $funcs }
        }
    };

    ({$($func:tt;)*}) => {
        $(
            libm_helper! { $func }
        )*
    };

    ((fn $func:ident($($arg:ident: $arg_typ:ty),*) -> ($($ret_typ:ty),*); => $libm_fn:ident)) => {
        #[inline(always)]
        pub fn $func($($arg: $arg_typ),*) -> ($($ret_typ),*) {
            $libm_fn($($arg),*)
        }
    };
}

// Stripped to only functions used by core_maths
libm_helper! {
    f32,
    funcs: {
        (fn acos(x: f32) -> (f32);                  => acosf);
        (fn asin(x: f32) -> (f32);                  => asinf);
        (fn atan(x: f32) -> (f32);                  => atanf);
        (fn atan2(y: f32, x: f32) -> (f32);         => atan2f);
        (fn cbrt(x: f32) -> (f32);                  => cbrtf);
        (fn ceil(x: f32) -> (f32);                  => ceilf);
        (fn copysign(x: f32, y: f32) -> (f32);      => copysignf);
        (fn cos(x: f32) -> (f32);                   => cosf);
        (fn cosh(x: f32) -> (f32);                  => coshf);
        (fn exp(x: f32) -> (f32);                   => expf);
        (fn exp2(x: f32) -> (f32);                  => exp2f);
        (fn expm1(x: f32) -> (f32);                 => expm1f);
        (fn fabs(x: f32) -> (f32);                  => fabsf);
        (fn floor(x: f32) -> (f32);                 => floorf);
        (fn fma(x: f32, y: f32, z: f32) -> (f32);   => fmaf);
        (fn hypot(x: f32, y: f32) -> (f32);         => hypotf);
        (fn log(x: f32) -> (f32);                   => logf);
        (fn log10(x: f32) -> (f32);                 => log10f);
        (fn log1p(x: f32) -> (f32);                 => log1pf);
        (fn log2(x: f32) -> (f32);                  => log2f);
        (fn pow(x: f32, y: f32) -> (f32);           => powf);
        (fn round(x: f32) -> (f32);                 => roundf);
        (fn sin(x: f32) -> (f32);                   => sinf);
        (fn sinh(x: f32) -> (f32);                  => sinhf);
        (fn sqrt(x: f32) -> (f32);                  => sqrtf);
        (fn tan(x: f32) -> (f32);                   => tanf);
        (fn tanh(x: f32) -> (f32);                  => tanhf);
        (fn trunc(x: f32) -> (f32);                 => truncf);
    }
}

libm_helper! {
    f64,
    funcs: {
        (fn acos(x: f64) -> (f64);                  => acos);
        (fn asin(x: f64) -> (f64);                  => asin);
        (fn atan(x: f64) -> (f64);                  => atan);
        (fn atan2(y: f64, x: f64) -> (f64);         => atan2);
        (fn cbrt(x: f64) -> (f64);                  => cbrt);
        (fn ceil(x: f64) -> (f64);                  => ceil);
        (fn copysign(x: f64, y: f64) -> (f64);      => copysign);
        (fn cos(x: f64) -> (f64);                   => cos);
        (fn cosh(x: f64) -> (f64);                  => cosh);
        (fn exp(x: f64) -> (f64);                   => exp);
        (fn exp2(x: f64) -> (f64);                  => exp2);
        (fn expm1(x: f64) -> (f64);                 => expm1);
        (fn fabs(x: f64) -> (f64);                  => fabs);
        (fn floor(x: f64) -> (f64);                 => floor);
        (fn fma(x: f64, y: f64, z: f64) -> (f64);   => fma);
        (fn hypot(x: f64, y: f64) -> (f64);         => hypot);
        (fn log(x: f64) -> (f64);                   => log);
        (fn log10(x: f64) -> (f64);                 => log10);
        (fn log1p(x: f64) -> (f64);                 => log1p);
        (fn log2(x: f64) -> (f64);                  => log2);
        (fn pow(x: f64, y: f64) -> (f64);           => pow);
        (fn round(x: f64) -> (f64);                 => round);
        (fn sin(x: f64) -> (f64);                   => sin);
        (fn sinh(x: f64) -> (f64);                  => sinh);
        (fn sqrt(x: f64) -> (f64);                  => sqrt);
        (fn tan(x: f64) -> (f64);                   => tan);
        (fn tanh(x: f64) -> (f64);                  => tanh);
        (fn trunc(x: f64) -> (f64);                 => trunc);
    }
}

#[cfg(f16_enabled)]
libm_helper! {
    f16,
    funcs: {
        (fn ceil(x: f16) -> (f16);                  => ceilf16);
        (fn copysign(x: f16, y: f16) -> (f16);      => copysignf16);
        (fn fabs(x: f16) -> (f16);                  => fabsf16);
        (fn floor(x: f16) -> (f16);                 => floorf16);
        (fn round(x: f16) -> (f16);                 => roundf16);
        (fn sqrtf(x: f16) -> (f16);                 => sqrtf16);
        (fn truncf(x: f16) -> (f16);                => truncf16);
    }
}

#[cfg(f128_enabled)]
libm_helper! {
    f128,
    funcs: {
        (fn ceil(x: f128) -> (f128);                => ceilf128);
        (fn copysign(x: f128, y: f128) -> (f128);   => copysignf128);
        (fn fabs(x: f128) -> (f128);                => fabsf128);
        (fn floor(x: f128) -> (f128);               => floorf128);
        (fn fma(x: f128, y: f128, z: f128) -> (f128); => fmaf128);
        (fn round(x: f128) -> (f128);               => roundf128);
        (fn sqrt(x: f128) -> (f128);                => sqrtf128);
        (fn trunc(x: f128) -> (f128);               => truncf128);
    }
}
