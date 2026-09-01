// 4-lane f32 vector for the modal-resonator kernels, following the
// libs/box3d/src/simd.rs precedent: an opaque wide type with cfg-selected
// backends and a scalar fallback that is always correct.
//
// Path selection:
// - aarch64: NEON (baseline on aarch64)
// - x86_64: SSE2 (baseline for the target); an additional 8-lane AVX2+FMA
//   kernel lives in modal.rs behind a runtime is_x86_feature_detected! check
// - other targets: scalar
//
// The representation is opaque outside this module: all consumers go through
// splat/load/store and the exported ops, so the type can be a raw __m128 /
// float32x4_t without touching call sites.
//
// Note on floating point: `fma` is fused on NEON and unfused (mul+add) on
// SSE2/scalar. Every path is bit-deterministic with respect to itself; the
// scalar and SIMD paths agree within a tolerance that tests/verify.rs proves.
// The horizontal sum uses the same (l0+l1)+(l2+l3) tree on every backend.

// ---------------------------------------------------------------------------
// NEON path (aarch64)
// ---------------------------------------------------------------------------
#[cfg(target_arch = "aarch64")]
mod imp {
    // SAFETY throughout this module: NEON is part of the aarch64 baseline, so
    // every intrinsic used here is unconditionally available on this target.
    use core::arch::aarch64::*;

    #[derive(Clone, Copy)]
    pub struct V4(pub(crate) float32x4_t);

    #[inline(always)]
    pub fn zero_v4() -> V4 {
        unsafe { V4(vdupq_n_f32(0.0)) }
    }

    #[inline(always)]
    pub fn splat_v4(x: f32) -> V4 {
        unsafe { V4(vdupq_n_f32(x)) }
    }

    /// Loads src[0..4]; caller guarantees len >= 4.
    #[inline(always)]
    pub fn load_v4(src: &[f32]) -> V4 {
        debug_assert!(src.len() >= 4);
        unsafe { V4(vld1q_f32(src.as_ptr())) }
    }

    /// Stores to dst[0..4]; caller guarantees len >= 4.
    #[inline(always)]
    pub fn store_v4(dst: &mut [f32], v: V4) {
        debug_assert!(dst.len() >= 4);
        unsafe { vst1q_f32(dst.as_mut_ptr(), v.0) }
    }

    #[inline(always)]
    pub fn add_v4(a: V4, b: V4) -> V4 {
        unsafe { V4(vaddq_f32(a.0, b.0)) }
    }

    #[inline(always)]
    pub fn sub_v4(a: V4, b: V4) -> V4 {
        unsafe { V4(vsubq_f32(a.0, b.0)) }
    }

    #[inline(always)]
    pub fn mul_v4(a: V4, b: V4) -> V4 {
        unsafe { V4(vmulq_f32(a.0, b.0)) }
    }

    /// a*b + c (fused on this backend).
    #[inline(always)]
    pub fn fma_v4(a: V4, b: V4, c: V4) -> V4 {
        unsafe { V4(vfmaq_f32(c.0, a.0, b.0)) }
    }

    /// (l0+l1) + (l2+l3) — same tree as the other backends.
    #[inline(always)]
    pub fn hsum_v4(a: V4) -> f32 {
        unsafe {
            let p = vpaddq_f32(a.0, a.0); // [0+1, 2+3, 0+1, 2+3]
            vgetq_lane_f32::<0>(p) + vgetq_lane_f32::<1>(p)
        }
    }
}

// ---------------------------------------------------------------------------
// SSE2 path (x86_64: SSE2 is baseline for the target)
// ---------------------------------------------------------------------------
#[cfg(target_arch = "x86_64")]
mod imp {
    // SAFETY throughout this module: SSE2 is part of the x86_64 baseline, so
    // every intrinsic used here is unconditionally available on this target.
    use core::arch::x86_64::*;

    #[derive(Clone, Copy)]
    pub struct V4(pub(crate) __m128);

    #[inline(always)]
    pub fn zero_v4() -> V4 {
        unsafe { V4(_mm_setzero_ps()) }
    }

    #[inline(always)]
    pub fn splat_v4(x: f32) -> V4 {
        unsafe { V4(_mm_set1_ps(x)) }
    }

    #[inline(always)]
    pub fn load_v4(src: &[f32]) -> V4 {
        debug_assert!(src.len() >= 4);
        unsafe { V4(_mm_loadu_ps(src.as_ptr())) }
    }

    #[inline(always)]
    pub fn store_v4(dst: &mut [f32], v: V4) {
        debug_assert!(dst.len() >= 4);
        unsafe { _mm_storeu_ps(dst.as_mut_ptr(), v.0) }
    }

    #[inline(always)]
    pub fn add_v4(a: V4, b: V4) -> V4 {
        unsafe { V4(_mm_add_ps(a.0, b.0)) }
    }

    #[inline(always)]
    pub fn sub_v4(a: V4, b: V4) -> V4 {
        unsafe { V4(_mm_sub_ps(a.0, b.0)) }
    }

    #[inline(always)]
    pub fn mul_v4(a: V4, b: V4) -> V4 {
        unsafe { V4(_mm_mul_ps(a.0, b.0)) }
    }

    /// a*b + c (unfused: SSE2 has no FMA; the AVX2 kernel uses real FMA).
    #[inline(always)]
    pub fn fma_v4(a: V4, b: V4, c: V4) -> V4 {
        unsafe { V4(_mm_add_ps(_mm_mul_ps(a.0, b.0), c.0)) }
    }

    /// (l0+l1) + (l2+l3) — same tree as the other backends.
    #[inline(always)]
    pub fn hsum_v4(a: V4) -> f32 {
        unsafe {
            let sh = _mm_shuffle_ps(a.0, a.0, 0b10_11_00_01); // [1,0,3,2]
            let s = _mm_add_ps(a.0, sh); // [0+1, 1+0, 2+3, 3+2]
            let hi = _mm_movehl_ps(s, s); // [2+3, 3+2, ..]
            _mm_cvtss_f32(_mm_add_ss(s, hi))
        }
    }
}

// ---------------------------------------------------------------------------
// Scalar path (all other targets) — always correct, never fast
// ---------------------------------------------------------------------------
#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
mod imp {
    #[derive(Clone, Copy)]
    pub struct V4(pub(crate) [f32; 4]);

    #[inline(always)]
    pub fn zero_v4() -> V4 {
        V4([0.0; 4])
    }

    #[inline(always)]
    pub fn splat_v4(x: f32) -> V4 {
        V4([x; 4])
    }

    #[inline(always)]
    pub fn load_v4(src: &[f32]) -> V4 {
        V4([src[0], src[1], src[2], src[3]])
    }

    #[inline(always)]
    pub fn store_v4(dst: &mut [f32], v: V4) {
        dst[0] = v.0[0];
        dst[1] = v.0[1];
        dst[2] = v.0[2];
        dst[3] = v.0[3];
    }

    #[inline(always)]
    pub fn add_v4(a: V4, b: V4) -> V4 {
        V4([a.0[0] + b.0[0], a.0[1] + b.0[1], a.0[2] + b.0[2], a.0[3] + b.0[3]])
    }

    #[inline(always)]
    pub fn sub_v4(a: V4, b: V4) -> V4 {
        V4([a.0[0] - b.0[0], a.0[1] - b.0[1], a.0[2] - b.0[2], a.0[3] - b.0[3]])
    }

    #[inline(always)]
    pub fn mul_v4(a: V4, b: V4) -> V4 {
        V4([a.0[0] * b.0[0], a.0[1] * b.0[1], a.0[2] * b.0[2], a.0[3] * b.0[3]])
    }

    /// a*b + c (unfused, matching SSE2).
    #[inline(always)]
    pub fn fma_v4(a: V4, b: V4, c: V4) -> V4 {
        V4([
            a.0[0] * b.0[0] + c.0[0],
            a.0[1] * b.0[1] + c.0[1],
            a.0[2] * b.0[2] + c.0[2],
            a.0[3] * b.0[3] + c.0[3],
        ])
    }

    /// (l0+l1) + (l2+l3) — same tree as the other backends.
    #[inline(always)]
    pub fn hsum_v4(a: V4) -> f32 {
        (a.0[0] + a.0[1]) + (a.0[2] + a.0[3])
    }
}

pub use imp::*;
