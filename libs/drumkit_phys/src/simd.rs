// Four-wide f32 vector primitives for the modal kernel: NEON on aarch64,
// SSE2 on x86_64 (both baseline for their targets, no runtime detection).
// Only the handful of operations the rotator needs; the kernel keeps the
// exact per-lane arithmetic and accumulation order of the scalar reference,
// so a given build renders bit-identically for any block decomposition.

#[cfg(target_arch = "aarch64")]
mod imp {
    use core::arch::aarch64::*;
    pub type V = float32x4_t;
    #[inline(always)]
    pub unsafe fn load(p: *const f32) -> V {
        vld1q_f32(p)
    }
    #[inline(always)]
    pub unsafe fn store(p: *mut f32, v: V) {
        vst1q_f32(p, v)
    }
    #[inline(always)]
    pub unsafe fn splat(x: f32) -> V {
        vdupq_n_f32(x)
    }
    #[inline(always)]
    pub unsafe fn zero() -> V {
        vdupq_n_f32(0.0)
    }
    #[inline(always)]
    pub unsafe fn add(a: V, b: V) -> V {
        vaddq_f32(a, b)
    }
    #[inline(always)]
    pub unsafe fn sub(a: V, b: V) -> V {
        vsubq_f32(a, b)
    }
    #[inline(always)]
    pub unsafe fn mul(a: V, b: V) -> V {
        vmulq_f32(a, b)
    }
}

#[cfg(target_arch = "x86_64")]
mod imp {
    use core::arch::x86_64::*;
    pub type V = __m128;
    #[inline(always)]
    pub unsafe fn load(p: *const f32) -> V {
        _mm_loadu_ps(p)
    }
    #[inline(always)]
    pub unsafe fn store(p: *mut f32, v: V) {
        _mm_storeu_ps(p, v)
    }
    #[inline(always)]
    pub unsafe fn splat(x: f32) -> V {
        _mm_set1_ps(x)
    }
    #[inline(always)]
    pub unsafe fn zero() -> V {
        _mm_setzero_ps()
    }
    #[inline(always)]
    pub unsafe fn add(a: V, b: V) -> V {
        _mm_add_ps(a, b)
    }
    #[inline(always)]
    pub unsafe fn sub(a: V, b: V) -> V {
        _mm_sub_ps(a, b)
    }
    #[inline(always)]
    pub unsafe fn mul(a: V, b: V) -> V {
        _mm_mul_ps(a, b)
    }
}

#[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
pub use imp::*;
