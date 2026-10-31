//! Wide float types for the fractal kernels, after the libs/box3d and
//! libs/piano_model pattern: an opaque vector type with a cfg-selected
//! `core::arch` backend (NEON on aarch64, SSE2 on x86_64, both baseline for
//! their target) and a scalar fallback for every other target.
//!
//! Two base types: `F64x2` (deep zoom) and `F32x4` (twice the lanes while the
//! view is shallow enough for f32). `Pair<T>` runs two of them side by side;
//! the Mandelbrot step is one long dependency chain, so two independent
//! chains per loop keep the float units busy (`F64x4 = Pair<F64x2>`,
//! `F32x8 = Pair<F32x4>`).
//!
//! A mask is the same type with every bit of a lane set (true) or clear.

use std::ops::{Add, Div, Mul, Sub};

pub trait Wide: Copy + Add<Output = Self> + Sub<Output = Self> + Mul<Output = Self> + Div<Output = Self> {
    /// Lanes per vector.
    const LANES: usize;
    fn splat(v: f64) -> Self;
    /// `base + i * step` in lane `i`.
    fn ramp(base: f64, step: f64) -> Self;
    /// Lanes from `v[..LANES]`.
    fn load(v: &[f64]) -> Self;
    fn abs(self) -> Self;
    fn sqrt(self) -> Self;
    /// Lane mask of `self < o`.
    fn lt(self, o: Self) -> Self;
    /// Bitwise and (a mask and a value: the value where the mask is set, else 0).
    fn and(self, o: Self) -> Self;
    /// Bitwise `!self & o`.
    fn and_not(self, o: Self) -> Self;
    fn or(self, o: Self) -> Self;
    /// True when any lane of the mask is set.
    fn any(self) -> bool;
    /// The lanes as f64, into `out[..LANES]`.
    fn store(self, out: &mut [f64]);

    #[inline(always)]
    fn select(mask: Self, a: Self, b: Self) -> Self {
        mask.and(a).or(mask.and_not(b))
    }
}

// ---------------------------------------------------------------------------
// NEON (aarch64)
// ---------------------------------------------------------------------------
#[cfg(target_arch = "aarch64")]
mod imp {
    // SAFETY throughout this module: NEON is part of the aarch64 baseline, so
    // every intrinsic used here is unconditionally available on this target.
    use super::Wide;
    use core::arch::aarch64::*;
    use std::ops::{Add, Div, Mul, Sub};

    #[derive(Clone, Copy)]
    pub struct F64x2(float64x2_t);
    #[derive(Clone, Copy)]
    pub struct F32x4(float32x4_t);

    #[inline(always)]
    fn u64s(a: float64x2_t) -> uint64x2_t {
        unsafe { vreinterpretq_u64_f64(a) }
    }
    #[inline(always)]
    fn f64s(a: uint64x2_t) -> float64x2_t {
        unsafe { vreinterpretq_f64_u64(a) }
    }
    #[inline(always)]
    fn u32s(a: float32x4_t) -> uint32x4_t {
        unsafe { vreinterpretq_u32_f32(a) }
    }
    #[inline(always)]
    fn f32s(a: uint32x4_t) -> float32x4_t {
        unsafe { vreinterpretq_f32_u32(a) }
    }

    impl Add for F64x2 {
        type Output = Self;
        #[inline(always)]
        fn add(self, o: Self) -> Self {
            unsafe { F64x2(vaddq_f64(self.0, o.0)) }
        }
    }
    impl Sub for F64x2 {
        type Output = Self;
        #[inline(always)]
        fn sub(self, o: Self) -> Self {
            unsafe { F64x2(vsubq_f64(self.0, o.0)) }
        }
    }
    impl Mul for F64x2 {
        type Output = Self;
        #[inline(always)]
        fn mul(self, o: Self) -> Self {
            unsafe { F64x2(vmulq_f64(self.0, o.0)) }
        }
    }
    impl Div for F64x2 {
        type Output = Self;
        #[inline(always)]
        fn div(self, o: Self) -> Self {
            unsafe { F64x2(vdivq_f64(self.0, o.0)) }
        }
    }
    impl Wide for F64x2 {
        const LANES: usize = 2;
        #[inline(always)]
        fn splat(v: f64) -> Self {
            unsafe { F64x2(vdupq_n_f64(v)) }
        }
        #[inline(always)]
        fn ramp(base: f64, step: f64) -> Self {
            let a = [base, base + step];
            unsafe { F64x2(vld1q_f64(a.as_ptr())) }
        }
        #[inline(always)]
        fn load(v: &[f64]) -> Self {
            assert!(v.len() >= 2);
            unsafe { F64x2(vld1q_f64(v.as_ptr())) }
        }
        #[inline(always)]
        fn abs(self) -> Self {
            unsafe { F64x2(vabsq_f64(self.0)) }
        }
        #[inline(always)]
        fn sqrt(self) -> Self {
            unsafe { F64x2(vsqrtq_f64(self.0)) }
        }
        #[inline(always)]
        fn lt(self, o: Self) -> Self {
            unsafe { F64x2(f64s(vcltq_f64(self.0, o.0))) }
        }
        #[inline(always)]
        fn and(self, o: Self) -> Self {
            unsafe { F64x2(f64s(vandq_u64(u64s(self.0), u64s(o.0)))) }
        }
        #[inline(always)]
        fn and_not(self, o: Self) -> Self {
            // vbic(a, b) = a & !b
            unsafe { F64x2(f64s(vbicq_u64(u64s(o.0), u64s(self.0)))) }
        }
        #[inline(always)]
        fn or(self, o: Self) -> Self {
            unsafe { F64x2(f64s(vorrq_u64(u64s(self.0), u64s(o.0)))) }
        }
        #[inline(always)]
        fn any(self) -> bool {
            let m = u64s(self.0);
            unsafe { (vgetq_lane_u64::<0>(m) | vgetq_lane_u64::<1>(m)) != 0 }
        }
        #[inline(always)]
        fn store(self, out: &mut [f64]) {
            assert!(out.len() >= 2);
            unsafe { vst1q_f64(out.as_mut_ptr(), self.0) }
        }
    }

    impl Add for F32x4 {
        type Output = Self;
        #[inline(always)]
        fn add(self, o: Self) -> Self {
            unsafe { F32x4(vaddq_f32(self.0, o.0)) }
        }
    }
    impl Sub for F32x4 {
        type Output = Self;
        #[inline(always)]
        fn sub(self, o: Self) -> Self {
            unsafe { F32x4(vsubq_f32(self.0, o.0)) }
        }
    }
    impl Mul for F32x4 {
        type Output = Self;
        #[inline(always)]
        fn mul(self, o: Self) -> Self {
            unsafe { F32x4(vmulq_f32(self.0, o.0)) }
        }
    }
    impl Div for F32x4 {
        type Output = Self;
        #[inline(always)]
        fn div(self, o: Self) -> Self {
            unsafe { F32x4(vdivq_f32(self.0, o.0)) }
        }
    }
    impl Wide for F32x4 {
        const LANES: usize = 4;
        #[inline(always)]
        fn splat(v: f64) -> Self {
            unsafe { F32x4(vdupq_n_f32(v as f32)) }
        }
        #[inline(always)]
        fn ramp(base: f64, step: f64) -> Self {
            let a = [base as f32, (base + step) as f32, (base + 2.0 * step) as f32, (base + 3.0 * step) as f32];
            unsafe { F32x4(vld1q_f32(a.as_ptr())) }
        }
        #[inline(always)]
        fn load(v: &[f64]) -> Self {
            let a = [v[0] as f32, v[1] as f32, v[2] as f32, v[3] as f32];
            unsafe { F32x4(vld1q_f32(a.as_ptr())) }
        }
        #[inline(always)]
        fn abs(self) -> Self {
            unsafe { F32x4(vabsq_f32(self.0)) }
        }
        #[inline(always)]
        fn sqrt(self) -> Self {
            unsafe { F32x4(vsqrtq_f32(self.0)) }
        }
        #[inline(always)]
        fn lt(self, o: Self) -> Self {
            unsafe { F32x4(f32s(vcltq_f32(self.0, o.0))) }
        }
        #[inline(always)]
        fn and(self, o: Self) -> Self {
            unsafe { F32x4(f32s(vandq_u32(u32s(self.0), u32s(o.0)))) }
        }
        #[inline(always)]
        fn and_not(self, o: Self) -> Self {
            unsafe { F32x4(f32s(vbicq_u32(u32s(o.0), u32s(self.0)))) }
        }
        #[inline(always)]
        fn or(self, o: Self) -> Self {
            unsafe { F32x4(f32s(vorrq_u32(u32s(self.0), u32s(o.0)))) }
        }
        #[inline(always)]
        fn any(self) -> bool {
            unsafe { vmaxvq_u32(u32s(self.0)) != 0 }
        }
        #[inline(always)]
        fn store(self, out: &mut [f64]) {
            let mut a = [0f32; 4];
            unsafe { vst1q_f32(a.as_mut_ptr(), self.0) };
            for i in 0..4 {
                out[i] = a[i] as f64;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// SSE2 (x86_64)
// ---------------------------------------------------------------------------
#[cfg(target_arch = "x86_64")]
mod imp {
    // SAFETY throughout this module: SSE2 is part of the x86_64 baseline, so
    // every intrinsic used here is unconditionally available on this target.
    use super::Wide;
    use core::arch::x86_64::*;
    use std::ops::{Add, Div, Mul, Sub};

    #[derive(Clone, Copy)]
    pub struct F64x2(__m128d);
    #[derive(Clone, Copy)]
    pub struct F32x4(__m128);

    impl Add for F64x2 {
        type Output = Self;
        #[inline(always)]
        fn add(self, o: Self) -> Self {
            unsafe { F64x2(_mm_add_pd(self.0, o.0)) }
        }
    }
    impl Sub for F64x2 {
        type Output = Self;
        #[inline(always)]
        fn sub(self, o: Self) -> Self {
            unsafe { F64x2(_mm_sub_pd(self.0, o.0)) }
        }
    }
    impl Mul for F64x2 {
        type Output = Self;
        #[inline(always)]
        fn mul(self, o: Self) -> Self {
            unsafe { F64x2(_mm_mul_pd(self.0, o.0)) }
        }
    }
    impl Div for F64x2 {
        type Output = Self;
        #[inline(always)]
        fn div(self, o: Self) -> Self {
            unsafe { F64x2(_mm_div_pd(self.0, o.0)) }
        }
    }
    impl Wide for F64x2 {
        const LANES: usize = 2;
        #[inline(always)]
        fn splat(v: f64) -> Self {
            unsafe { F64x2(_mm_set1_pd(v)) }
        }
        #[inline(always)]
        fn ramp(base: f64, step: f64) -> Self {
            unsafe { F64x2(_mm_setr_pd(base, base + step)) }
        }
        #[inline(always)]
        fn load(v: &[f64]) -> Self {
            unsafe { F64x2(_mm_setr_pd(v[0], v[1])) }
        }
        #[inline(always)]
        fn abs(self) -> Self {
            unsafe { F64x2(_mm_andnot_pd(_mm_set1_pd(-0.0), self.0)) }
        }
        #[inline(always)]
        fn sqrt(self) -> Self {
            unsafe { F64x2(_mm_sqrt_pd(self.0)) }
        }
        #[inline(always)]
        fn lt(self, o: Self) -> Self {
            unsafe { F64x2(_mm_cmplt_pd(self.0, o.0)) }
        }
        #[inline(always)]
        fn and(self, o: Self) -> Self {
            unsafe { F64x2(_mm_and_pd(self.0, o.0)) }
        }
        #[inline(always)]
        fn and_not(self, o: Self) -> Self {
            unsafe { F64x2(_mm_andnot_pd(self.0, o.0)) }
        }
        #[inline(always)]
        fn or(self, o: Self) -> Self {
            unsafe { F64x2(_mm_or_pd(self.0, o.0)) }
        }
        #[inline(always)]
        fn any(self) -> bool {
            unsafe { _mm_movemask_pd(self.0) != 0 }
        }
        #[inline(always)]
        fn store(self, out: &mut [f64]) {
            assert!(out.len() >= 2);
            unsafe { _mm_storeu_pd(out.as_mut_ptr(), self.0) }
        }
    }

    impl Add for F32x4 {
        type Output = Self;
        #[inline(always)]
        fn add(self, o: Self) -> Self {
            unsafe { F32x4(_mm_add_ps(self.0, o.0)) }
        }
    }
    impl Sub for F32x4 {
        type Output = Self;
        #[inline(always)]
        fn sub(self, o: Self) -> Self {
            unsafe { F32x4(_mm_sub_ps(self.0, o.0)) }
        }
    }
    impl Mul for F32x4 {
        type Output = Self;
        #[inline(always)]
        fn mul(self, o: Self) -> Self {
            unsafe { F32x4(_mm_mul_ps(self.0, o.0)) }
        }
    }
    impl Div for F32x4 {
        type Output = Self;
        #[inline(always)]
        fn div(self, o: Self) -> Self {
            unsafe { F32x4(_mm_div_ps(self.0, o.0)) }
        }
    }
    impl Wide for F32x4 {
        const LANES: usize = 4;
        #[inline(always)]
        fn splat(v: f64) -> Self {
            unsafe { F32x4(_mm_set1_ps(v as f32)) }
        }
        #[inline(always)]
        fn ramp(base: f64, step: f64) -> Self {
            unsafe {
                F32x4(_mm_setr_ps(base as f32, (base + step) as f32, (base + 2.0 * step) as f32, (base + 3.0 * step) as f32))
            }
        }
        #[inline(always)]
        fn load(v: &[f64]) -> Self {
            unsafe { F32x4(_mm_setr_ps(v[0] as f32, v[1] as f32, v[2] as f32, v[3] as f32)) }
        }
        #[inline(always)]
        fn abs(self) -> Self {
            unsafe { F32x4(_mm_andnot_ps(_mm_set1_ps(-0.0), self.0)) }
        }
        #[inline(always)]
        fn sqrt(self) -> Self {
            unsafe { F32x4(_mm_sqrt_ps(self.0)) }
        }
        #[inline(always)]
        fn lt(self, o: Self) -> Self {
            unsafe { F32x4(_mm_cmplt_ps(self.0, o.0)) }
        }
        #[inline(always)]
        fn and(self, o: Self) -> Self {
            unsafe { F32x4(_mm_and_ps(self.0, o.0)) }
        }
        #[inline(always)]
        fn and_not(self, o: Self) -> Self {
            unsafe { F32x4(_mm_andnot_ps(self.0, o.0)) }
        }
        #[inline(always)]
        fn or(self, o: Self) -> Self {
            unsafe { F32x4(_mm_or_ps(self.0, o.0)) }
        }
        #[inline(always)]
        fn any(self) -> bool {
            unsafe { _mm_movemask_ps(self.0) != 0 }
        }
        #[inline(always)]
        fn store(self, out: &mut [f64]) {
            let mut a = [0f32; 4];
            unsafe { _mm_storeu_ps(a.as_mut_ptr(), self.0) };
            for i in 0..4 {
                out[i] = a[i] as f64;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Scalar fallback (every other target)
// ---------------------------------------------------------------------------
#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
mod imp {
    use super::Wide;
    use std::ops::{Add, Div, Mul, Sub};

    macro_rules! scalar_wide {
        ($name:ident, $t:ty, $bits:ty, $n:literal) => {
            #[derive(Clone, Copy)]
            pub struct $name([$t; $n]);
            impl $name {
                #[inline(always)]
                fn map2(self, o: Self, f: impl Fn($t, $t) -> $t) -> Self {
                    let mut r = self.0;
                    for i in 0..$n {
                        r[i] = f(self.0[i], o.0[i]);
                    }
                    $name(r)
                }
                #[inline(always)]
                fn mask(b: bool) -> $t {
                    <$t>::from_bits(if b { <$bits>::MAX } else { 0 })
                }
            }
            impl Add for $name {
                type Output = Self;
                fn add(self, o: Self) -> Self {
                    self.map2(o, |a, b| a + b)
                }
            }
            impl Sub for $name {
                type Output = Self;
                fn sub(self, o: Self) -> Self {
                    self.map2(o, |a, b| a - b)
                }
            }
            impl Mul for $name {
                type Output = Self;
                fn mul(self, o: Self) -> Self {
                    self.map2(o, |a, b| a * b)
                }
            }
            impl Div for $name {
                type Output = Self;
                fn div(self, o: Self) -> Self {
                    self.map2(o, |a, b| a / b)
                }
            }
            impl Wide for $name {
                const LANES: usize = $n;
                fn splat(v: f64) -> Self {
                    $name([v as $t; $n])
                }
                fn ramp(base: f64, step: f64) -> Self {
                    let mut r = [0 as $t; $n];
                    for i in 0..$n {
                        r[i] = (base + i as f64 * step) as $t;
                    }
                    $name(r)
                }
                fn load(v: &[f64]) -> Self {
                    let mut r = [0 as $t; $n];
                    for i in 0..$n {
                        r[i] = v[i] as $t;
                    }
                    $name(r)
                }
                fn abs(self) -> Self {
                    self.map2(self, |a, _| a.abs())
                }
                fn sqrt(self) -> Self {
                    self.map2(self, |a, _| a.sqrt())
                }
                fn lt(self, o: Self) -> Self {
                    self.map2(o, |a, b| Self::mask(a < b))
                }
                fn and(self, o: Self) -> Self {
                    self.map2(o, |a, b| <$t>::from_bits(a.to_bits() & b.to_bits()))
                }
                fn and_not(self, o: Self) -> Self {
                    self.map2(o, |a, b| <$t>::from_bits(!a.to_bits() & b.to_bits()))
                }
                fn or(self, o: Self) -> Self {
                    self.map2(o, |a, b| <$t>::from_bits(a.to_bits() | b.to_bits()))
                }
                fn any(self) -> bool {
                    self.0.iter().any(|v| v.to_bits() != 0)
                }
                fn store(self, out: &mut [f64]) {
                    for i in 0..$n {
                        out[i] = self.0[i] as f64;
                    }
                }
            }
        };
    }
    scalar_wide!(F64x2, f64, u64, 2);
    scalar_wide!(F32x4, f32, u32, 4);
}

pub use imp::{F32x4, F64x2};

/// Two vectors processed side by side: twice the lanes, two independent
/// dependency chains per step.
#[derive(Clone, Copy)]
pub struct Pair<T>(T, T);

pub type F64x4 = Pair<F64x2>;
pub type F32x8 = Pair<F32x4>;

impl<T: Wide> Add for Pair<T> {
    type Output = Self;
    #[inline(always)]
    fn add(self, o: Self) -> Self {
        Pair(self.0 + o.0, self.1 + o.1)
    }
}
impl<T: Wide> Sub for Pair<T> {
    type Output = Self;
    #[inline(always)]
    fn sub(self, o: Self) -> Self {
        Pair(self.0 - o.0, self.1 - o.1)
    }
}
impl<T: Wide> Mul for Pair<T> {
    type Output = Self;
    #[inline(always)]
    fn mul(self, o: Self) -> Self {
        Pair(self.0 * o.0, self.1 * o.1)
    }
}
impl<T: Wide> Div for Pair<T> {
    type Output = Self;
    #[inline(always)]
    fn div(self, o: Self) -> Self {
        Pair(self.0 / o.0, self.1 / o.1)
    }
}
impl<T: Wide> Wide for Pair<T> {
    const LANES: usize = 2 * T::LANES;
    #[inline(always)]
    fn splat(v: f64) -> Self {
        Pair(T::splat(v), T::splat(v))
    }
    #[inline(always)]
    fn ramp(base: f64, step: f64) -> Self {
        Pair(T::ramp(base, step), T::ramp(base + T::LANES as f64 * step, step))
    }
    #[inline(always)]
    fn load(v: &[f64]) -> Self {
        Pair(T::load(&v[..T::LANES]), T::load(&v[T::LANES..]))
    }
    #[inline(always)]
    fn abs(self) -> Self {
        Pair(self.0.abs(), self.1.abs())
    }
    #[inline(always)]
    fn sqrt(self) -> Self {
        Pair(self.0.sqrt(), self.1.sqrt())
    }
    #[inline(always)]
    fn lt(self, o: Self) -> Self {
        Pair(self.0.lt(o.0), self.1.lt(o.1))
    }
    #[inline(always)]
    fn and(self, o: Self) -> Self {
        Pair(self.0.and(o.0), self.1.and(o.1))
    }
    #[inline(always)]
    fn and_not(self, o: Self) -> Self {
        Pair(self.0.and_not(o.0), self.1.and_not(o.1))
    }
    #[inline(always)]
    fn or(self, o: Self) -> Self {
        Pair(self.0.or(o.0), self.1.or(o.1))
    }
    #[inline(always)]
    fn any(self) -> bool {
        self.0.or(self.1).any()
    }
    #[inline(always)]
    fn store(self, out: &mut [f64]) {
        self.0.store(&mut out[..T::LANES]);
        self.1.store(&mut out[T::LANES..]);
    }
}
