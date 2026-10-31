//! SIMD intrinsics. Hot 128-bit operations use the compiler vector bridge.
//! Remaining scalar AVX2 packing, shuffles and unpacks operate in 128-bit halves.
//!
//! Signatures and operations follow stdarch's x86 modules and Intel pseudocode.
//! Pointer casts below read fully initialized, padding-free repr(C) vectors and
//! integer arrays; unaligned reads also work when the destination is more aligned.
//! The compiler selects target instructions for bridge operations.
//!
//! Unsafe entry points retain stdarch's pointer and immediate contracts. Pointers
//! must cover the full load/store; _mm_load_si128 and _mm_store_si128 require 16-byte alignment.
#![allow(non_camel_case_types, non_snake_case)]
use crate::intrinsics_vx as vx;

#[derive(Copy, Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(C)]
pub struct CpuidResult {
    pub eax: u32,
    pub ebx: u32,
    pub ecx: u32,
    pub edx: u32,
}

#[inline]
pub fn __cpuid_count(leaf: u32, sub_leaf: u32) -> CpuidResult {
    let (eax, ebx, ecx, edx) = unsafe { crate::intrinsics_rt::cpuid(leaf, sub_leaf) };
    CpuidResult { eax, ebx, ecx, edx }
}

#[inline]
pub fn __cpuid(leaf: u32) -> CpuidResult {
    __cpuid_count(leaf, 0)
}

#[inline]
pub fn __get_cpuid_max(leaf: u32) -> (u32, u32) {
    let r = __cpuid(leaf);
    (r.eax, r.ebx)
}

pub const _XCR_XFEATURE_ENABLED_MASK: u32 = 0;

/// Read an extended control register. The caller must ensure XSAVE/OSXSAVE
/// support and that `xcr` names an index supported by the processor.
#[inline]
pub unsafe fn _xgetbv(xcr: u32) -> u64 {
    crate::intrinsics_rt::xgetbv(xcr)
}

// There is no inline-assembly path in this module. Report failure so callers
// use their entropy fallback; RDRAND also clears its destination on failure.
#[inline]
#[cfg_attr(any(target_arch = "x86", target_arch = "x86_64"), target_feature(enable = "rdrand"))]
pub unsafe fn _rdrand32_step(val: &mut u32) -> i32 {
    *val = 0;
    0
}

#[inline]
#[cfg_attr(any(target_arch = "x86", target_arch = "x86_64"), target_feature(enable = "rdrand"))]
pub unsafe fn _rdrand64_step(val: &mut u64) -> i32 {
    *val = 0;
    0
}

#[derive(Copy, Clone)]
#[repr(C, align(16))]
pub struct __m128i(pub [i64; 2]);

// The float vector types (stdarch's __m128/__m128d/__m256/__m256d/__m512/__m512d): same size and
// alignment as the hardware types.
#[derive(Copy, Clone)]
#[repr(C, align(16))]
pub struct __m128(pub [f32; 4]);
#[derive(Copy, Clone)]
#[repr(C, align(16))]
pub struct __m128d(pub [f64; 2]);
#[derive(Copy, Clone)]
#[repr(C, align(32))]
pub struct __m256(pub [f32; 8]);
#[derive(Copy, Clone)]
#[repr(C, align(32))]
pub struct __m256d(pub [f64; 4]);
#[derive(Copy, Clone)]
#[repr(C, align(64))]
pub struct __m512(pub [f32; 16]);
#[derive(Copy, Clone)]
#[repr(C, align(64))]
pub struct __m512d(pub [f64; 8]);
#[derive(Copy, Clone)]
#[repr(C, align(16))]
pub struct __m128bh(pub [u16; 8]);
#[derive(Copy, Clone)]
#[repr(C, align(32))]
pub struct __m256bh(pub [u16; 16]);
#[derive(Copy, Clone)]
#[repr(C, align(64))]
pub struct __m512bh(pub [u16; 32]);

unsafe fn lanes128_i8(a: __m128i) -> [i8; 16] {
    crate::ptr::read_unaligned(&a as *const __m128i as *const [i8; 16])
}
unsafe fn from128_i8(a: [i8; 16]) -> __m128i {
    crate::ptr::read_unaligned(&a as *const [i8; 16] as *const __m128i)
}

unsafe fn lanes128_u8(a: __m128i) -> [u8; 16] {
    crate::ptr::read_unaligned(&a as *const __m128i as *const [u8; 16])
}
unsafe fn from128_u8(a: [u8; 16]) -> __m128i {
    crate::ptr::read_unaligned(&a as *const [u8; 16] as *const __m128i)
}

unsafe fn lanes128_i16(a: __m128i) -> [i16; 8] {
    crate::ptr::read_unaligned(&a as *const __m128i as *const [i16; 8])
}
unsafe fn from128_i16(a: [i16; 8]) -> __m128i {
    crate::ptr::read_unaligned(&a as *const [i16; 8] as *const __m128i)
}

unsafe fn lanes128_u16(a: __m128i) -> [u16; 8] {
    crate::ptr::read_unaligned(&a as *const __m128i as *const [u16; 8])
}
unsafe fn from128_u16(a: [u16; 8]) -> __m128i {
    crate::ptr::read_unaligned(&a as *const [u16; 8] as *const __m128i)
}

unsafe fn lanes128_i32(a: __m128i) -> [i32; 4] {
    crate::ptr::read_unaligned(&a as *const __m128i as *const [i32; 4])
}
unsafe fn from128_i32(a: [i32; 4]) -> __m128i {
    crate::ptr::read_unaligned(&a as *const [i32; 4] as *const __m128i)
}

unsafe fn lanes128_u32(a: __m128i) -> [u32; 4] {
    crate::ptr::read_unaligned(&a as *const __m128i as *const [u32; 4])
}
unsafe fn from128_u32(a: [u32; 4]) -> __m128i {
    crate::ptr::read_unaligned(&a as *const [u32; 4] as *const __m128i)
}

#[derive(Copy, Clone)]
#[repr(C, align(32))]
pub struct __m256i(pub [i64; 4]);

unsafe fn lanes256_i8(a: __m256i) -> [i8; 32] {
    crate::ptr::read_unaligned(&a as *const __m256i as *const [i8; 32])
}
unsafe fn from256_i8(a: [i8; 32]) -> __m256i {
    crate::ptr::read_unaligned(&a as *const [i8; 32] as *const __m256i)
}

unsafe fn lanes256_u8(a: __m256i) -> [u8; 32] {
    crate::ptr::read_unaligned(&a as *const __m256i as *const [u8; 32])
}
unsafe fn from256_u8(a: [u8; 32]) -> __m256i {
    crate::ptr::read_unaligned(&a as *const [u8; 32] as *const __m256i)
}

unsafe fn lanes256_i16(a: __m256i) -> [i16; 16] {
    crate::ptr::read_unaligned(&a as *const __m256i as *const [i16; 16])
}
unsafe fn from256_i16(a: [i16; 16]) -> __m256i {
    crate::ptr::read_unaligned(&a as *const [i16; 16] as *const __m256i)
}

unsafe fn lanes256_u16(a: __m256i) -> [u16; 16] {
    crate::ptr::read_unaligned(&a as *const __m256i as *const [u16; 16])
}
unsafe fn from256_u16(a: [u16; 16]) -> __m256i {
    crate::ptr::read_unaligned(&a as *const [u16; 16] as *const __m256i)
}

unsafe fn lanes256_i32(a: __m256i) -> [i32; 8] {
    crate::ptr::read_unaligned(&a as *const __m256i as *const [i32; 8])
}
unsafe fn from256_i32(a: [i32; 8]) -> __m256i {
    crate::ptr::read_unaligned(&a as *const [i32; 8] as *const __m256i)
}

#[derive(Copy, Clone)]
#[repr(C, align(64))]
pub struct __m512i(pub [i64; 8]);

unsafe fn lanes512_i8(a: __m512i) -> [i8; 64] {
    crate::ptr::read_unaligned(&a as *const __m512i as *const [i8; 64])
}
unsafe fn from512_i8(a: [i8; 64]) -> __m512i {
    crate::ptr::read_unaligned(&a as *const [i8; 64] as *const __m512i)
}

unsafe fn lanes512_u8(a: __m512i) -> [u8; 64] {
    crate::ptr::read_unaligned(&a as *const __m512i as *const [u8; 64])
}

unsafe fn lanes512_i16(a: __m512i) -> [i16; 32] {
    crate::ptr::read_unaligned(&a as *const __m512i as *const [i16; 32])
}
unsafe fn from512_i16(a: [i16; 32]) -> __m512i {
    crate::ptr::read_unaligned(&a as *const [i16; 32] as *const __m512i)
}

unsafe fn lanes512_i32(a: __m512i) -> [i32; 16] {
    crate::ptr::read_unaligned(&a as *const __m512i as *const [i32; 16])
}
unsafe fn from512_i32(a: [i32; 16]) -> __m512i {
    crate::ptr::read_unaligned(&a as *const [i32; 16] as *const __m512i)
}

unsafe fn lanes512_i128(a: __m512i) -> [__m128i; 4] {
    crate::ptr::read_unaligned(&a as *const __m512i as *const [__m128i; 4])
}
unsafe fn from512_i128(a: [__m128i; 4]) -> __m512i {
    crate::ptr::read_unaligned(&a as *const [__m128i; 4] as *const __m512i)
}

pub type _MM_PERM_ENUM = i32;

pub const fn _MM_SHUFFLE(z: u32, y: u32, x: u32, w: u32) -> i32 {
    ((z << 6) | (y << 4) | (x << 2) | w) as i32
}

// rustc library/stdarch/crates/core_arch/src/x86/avx2.rs: _mm256_blend_epi32.
#[cfg_attr(any(target_arch = "x86", target_arch = "x86_64"), target_feature(enable = "avx2"))]
#[cfg_attr(not(relax_check), rustc_legacy_const_generics(2))]
pub unsafe fn _mm256_blend_epi32<const IMM8: i32>(a: __m256i, b: __m256i) -> __m256i {
    const { assert!(IMM8 >= 0 && IMM8 < 256) };
    let a = lanes256_i32(a);
    let b = lanes256_i32(b);
    let mut r = [0i32; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = if (IMM8 >> i) & 1 != 0 { b[i] } else { a[i] };
        i += 1;
    }
    from256_i32(r)
}

// rustc library/stdarch/crates/core_arch/src/x86/avx2.rs: _mm256_shuffle_epi32.
// Repeat the selector within each 128-bit half; immediates are unsigned 8-bit values.
// Gate target_feature by architecture: this module is also mounted on non-x86 hosts.
#[cfg_attr(any(target_arch = "x86", target_arch = "x86_64"), target_feature(enable = "avx2"))]
#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
pub unsafe fn _mm256_shuffle_epi32<const MASK: i32>(a: __m256i) -> __m256i {
    const { assert!(MASK >= 0 && MASK < 256) };
    i_join(_mm_shuffle_epi32::<MASK>(i_lo(&a)), _mm_shuffle_epi32::<MASK>(i_hi(&a)))
}

// rustc library/stdarch/crates/core_arch/src/x86/avx2.rs: _mm256_permutevar8x32_epi32.
#[cfg_attr(any(target_arch = "x86", target_arch = "x86_64"), target_feature(enable = "avx2"))]
pub unsafe fn _mm256_permutevar8x32_epi32(a: __m256i, b: __m256i) -> __m256i {
    let a = lanes256_i32(a);
    let b = lanes256_i32(b);
    let mut r = [0i32; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = a[(b[i] as u32 & 7) as usize];
        i += 1;
    }
    from256_i32(r)
}

// rustc library/stdarch/crates/core_arch/src/x86/avx2.rs: _mm256_mul_epu32.
// Only the low unsigned dword in each qword participates in the 64-bit product.
#[cfg_attr(any(target_arch = "x86", target_arch = "x86_64"), target_feature(enable = "avx2"))]
pub unsafe fn _mm256_mul_epu32(a: __m256i, b: __m256i) -> __m256i {
    i_join(_mm_mul_epu32(i_lo(&a), i_lo(&b)), _mm_mul_epu32(i_hi(&a), i_hi(&b)))
}

pub unsafe fn _mm256_castsi256_si128(a: __m256i) -> __m128i {
    __m128i([a.0[0], a.0[1]])
}

pub unsafe fn _mm256_castsi128_si256(a: __m128i) -> __m256i {
    __m256i([a.0[0], a.0[1], 0, 0])
}

pub unsafe fn _mm256_set_epi64x(e3: i64, e2: i64, e1: i64, e0: i64) -> __m256i {
    __m256i([e0, e1, e2, e3])
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(2))]
pub unsafe fn _mm256_clmulepi64_epi128<const IMM8: i32>(a: __m256i, b: __m256i) -> __m256i {
    i_join(
        _mm_clmulepi64_si128::<IMM8>(i_lo(&a), i_lo(&b)),
        _mm_clmulepi64_si128::<IMM8>(i_hi(&a), i_hi(&b)),
    )
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
pub unsafe fn _mm256_extractf128_si256<const IMM1: i32>(a: __m256i) -> __m128i {
    __m128i([a.0[IMM1 as usize * 2], a.0[IMM1 as usize * 2 + 1]])
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
pub unsafe fn _mm256_extracti128_si256<const IMM1: i32>(a: __m256i) -> __m128i {
    __m128i([a.0[IMM1 as usize * 2], a.0[IMM1 as usize * 2 + 1]])
}

pub unsafe fn _mm256_mullo_epi16(a: __m256i, b: __m256i) -> __m256i {
    let a = lanes256_u16(a);
    let b = lanes256_u16(b);
    let mut r = [0u16; 16];
    let mut i = 0;
    while i < 16 {
        r[i] = a[i].wrapping_mul(b[i]);
        i += 1;
    }
    from256_u16(r)
}

pub unsafe fn _mm256_mullo_epi32(a: __m256i, b: __m256i) -> __m256i {
    let a = lanes256_i32(a);
    let b = lanes256_i32(b);
    let mut r = [0i32; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = a[i].wrapping_mul(b[i]);
        i += 1;
    }
    from256_i32(r)
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(2))]
pub unsafe fn _mm256_permute2x128_si256<const IMM8: i32>(a: __m256i, b: __m256i) -> __m256i {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = {
            let control = (IMM8 >> ((i / 2) * 4)) & 15;
            let j = ((control & 1) as usize) * 2 + i % 2;
            if control & 8 != 0 {
                0
            } else if control & 2 != 0 {
                b.0[j]
            } else {
                a.0[j]
            }
        };
        i += 1;
    }
    __m256i(r)
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
pub unsafe fn _mm256_permute4x64_epi64<const IMM8: i32>(a: __m256i) -> __m256i {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = a.0[(((IMM8 as u32) >> (i * 2)) & 3) as usize];
        i += 1;
    }
    __m256i(r)
}

pub unsafe fn _mm256_set1_epi16(a: i16) -> __m256i {
    from256_i16([a; 16])
}

pub unsafe fn _mm256_set1_epi32(a: i32) -> __m256i {
    from256_i32([a; 8])
}

pub unsafe fn _mm256_set_epi32(
    e0: i32,
    e1: i32,
    e2: i32,
    e3: i32,
    e4: i32,
    e5: i32,
    e6: i32,
    e7: i32,
) -> __m256i {
    from256_i32([e7, e6, e5, e4, e3, e2, e1, e0])
}

pub unsafe fn _mm256_set_epi8(
    e00: i8,
    e01: i8,
    e02: i8,
    e03: i8,
    e04: i8,
    e05: i8,
    e06: i8,
    e07: i8,
    e08: i8,
    e09: i8,
    e10: i8,
    e11: i8,
    e12: i8,
    e13: i8,
    e14: i8,
    e15: i8,
    e16: i8,
    e17: i8,
    e18: i8,
    e19: i8,
    e20: i8,
    e21: i8,
    e22: i8,
    e23: i8,
    e24: i8,
    e25: i8,
    e26: i8,
    e27: i8,
    e28: i8,
    e29: i8,
    e30: i8,
    e31: i8,
) -> __m256i {
    from256_i8([
        e31, e30, e29, e28, e27, e26, e25, e24, e23, e22, e21, e20, e19, e18, e17, e16, e15, e14,
        e13, e12, e11, e10, e09, e08, e07, e06, e05, e04, e03, e02, e01, e00,
    ])
}

pub unsafe fn _mm256_setr_epi8(
    e00: i8,
    e01: i8,
    e02: i8,
    e03: i8,
    e04: i8,
    e05: i8,
    e06: i8,
    e07: i8,
    e08: i8,
    e09: i8,
    e10: i8,
    e11: i8,
    e12: i8,
    e13: i8,
    e14: i8,
    e15: i8,
    e16: i8,
    e17: i8,
    e18: i8,
    e19: i8,
    e20: i8,
    e21: i8,
    e22: i8,
    e23: i8,
    e24: i8,
    e25: i8,
    e26: i8,
    e27: i8,
    e28: i8,
    e29: i8,
    e30: i8,
    e31: i8,
) -> __m256i {
    from256_i8([
        e00, e01, e02, e03, e04, e05, e06, e07, e08, e09, e10, e11, e12, e13, e14, e15, e16, e17,
        e18, e19, e20, e21, e22, e23, e24, e25, e26, e27, e28, e29, e30, e31,
    ])
}

pub unsafe fn _mm256_testz_si256(a: __m256i, b: __m256i) -> i32 {
    let mut r = 0i64;
    let mut i = 0;
    while i < 4 {
        r |= a.0[i] & b.0[i];
        i += 1;
    }
    if r == 0 {
        1
    } else {
        0
    }
}

unsafe fn mask_f32(b: bool) -> f32 {
    f32::from_bits(if b { 0xffff_ffff } else { 0 })
}

pub unsafe fn _mm256_cvtepi32_ps(a: __m256i) -> __m256 {
    let a = lanes256_i32(a);
    let mut r = [0f32; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = a[i] as f32;
        i += 1;
    }
    __m256(r)
}

pub unsafe fn _mm256_cvtepi8_epi32(a: __m128i) -> __m256i {
    let a = lanes128_i8(a);
    let mut r = [0i32; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = a[i] as i32;
        i += 1;
    }
    from256_i32(r)
}

pub unsafe fn _mm256_movemask_epi8(a: __m256i) -> i32 {
    let a = lanes256_u8(a);
    let mut r = 0u32;
    let mut i = 0;
    while i < 32 {
        r |= ((a[i] >> 7) as u32) << i;
        i += 1;
    }
    r as i32
}

pub unsafe fn _mm256_set1_epi8(a: i8) -> __m256i {
    from256_i8([a; 32])
}

// ---- 256-bit float vectors as two 128-bit halves -------------------------------------------
// A __m256 is 32 bytes in memory; every lane-wise operation loads its two 16-byte halves into
// V128 values, applies the 128-bit bridge operation to each half and stores the halves back,
// so the backend emits real vector code (and slot forwarding removes the copies). No 256-bit
// register class exists in the back end; the halves cost two instructions per operation
// where AVX has one. Lane order is memory order, so lane-wise ops are exact.

#[inline(always)]
unsafe fn ps_lo(a: &__m256) -> __m128 {
    vx::vx_load(a as *const __m256 as *const u8)
}
#[inline(always)]
unsafe fn ps_hi(a: &__m256) -> __m128 {
    vx::vx_load((a as *const __m256 as *const u8).add(16))
}
#[inline(always)]
unsafe fn ps_join(lo: __m128, hi: __m128) -> __m256 {
    crate::mem::transmute::<[__m128; 2], __m256>([lo, hi])
}

pub unsafe fn _mm256_add_ps(a: __m256, b: __m256) -> __m256 {
    ps_join(_mm_add_ps(ps_lo(&a), ps_lo(&b)), _mm_add_ps(ps_hi(&a), ps_hi(&b)))
}
pub unsafe fn _mm256_sub_ps(a: __m256, b: __m256) -> __m256 {
    ps_join(_mm_sub_ps(ps_lo(&a), ps_lo(&b)), _mm_sub_ps(ps_hi(&a), ps_hi(&b)))
}
pub unsafe fn _mm256_mul_ps(a: __m256, b: __m256) -> __m256 {
    ps_join(_mm_mul_ps(ps_lo(&a), ps_lo(&b)), _mm_mul_ps(ps_hi(&a), ps_hi(&b)))
}
pub unsafe fn _mm256_div_ps(a: __m256, b: __m256) -> __m256 {
    ps_join(_mm_div_ps(ps_lo(&a), ps_lo(&b)), _mm_div_ps(ps_hi(&a), ps_hi(&b)))
}
/// Intel vmaxps/vminps: the second operand when either is NaN (or both zero)
pub unsafe fn _mm256_max_ps(a: __m256, b: __m256) -> __m256 {
    ps_join(_mm_max_ps(ps_lo(&a), ps_lo(&b)), _mm_max_ps(ps_hi(&a), ps_hi(&b)))
}
pub unsafe fn _mm256_min_ps(a: __m256, b: __m256) -> __m256 {
    ps_join(_mm_min_ps(ps_lo(&a), ps_lo(&b)), _mm_min_ps(ps_hi(&a), ps_hi(&b)))
}
pub unsafe fn _mm256_and_ps(a: __m256, b: __m256) -> __m256 {
    ps_join(_mm_and_ps(ps_lo(&a), ps_lo(&b)), _mm_and_ps(ps_hi(&a), ps_hi(&b)))
}
pub unsafe fn _mm256_or_ps(a: __m256, b: __m256) -> __m256 {
    ps_join(_mm_or_ps(ps_lo(&a), ps_lo(&b)), _mm_or_ps(ps_hi(&a), ps_hi(&b)))
}
pub unsafe fn _mm256_xor_ps(a: __m256, b: __m256) -> __m256 {
    ps_join(_mm_xor_ps(ps_lo(&a), ps_lo(&b)), _mm_xor_ps(ps_hi(&a), ps_hi(&b)))
}
/// (!a) & b
pub unsafe fn _mm256_andnot_ps(a: __m256, b: __m256) -> __m256 {
    ps_join(_mm_andnot_ps(ps_lo(&a), ps_lo(&b)), _mm_andnot_ps(ps_hi(&a), ps_hi(&b)))
}

// Fused multiply-add, one rounding: a*b + c (fmadd), a*b - c (fmsub), -(a*b) + c (fnmadd),
// -(a*b) - c (fnmsub). With hardware FMA each is the packed fused op (c +- a*b) and, for
// fmsub / fnmsub, a sign flip of the exact result (negation commutes with round-to-nearest);
// without it, one libm fmaf per lane.
#[inline(always)]
unsafe fn fma_ps(a: __m128, b: __m128, c: __m128, neg_prod: bool, neg_add: bool) -> __m128 {
    if crate::intrinsics::fma_native() {
        // c' + a*b (selector 8) or c' - a*b (selector 9), c' = c or -c. The addend is negated
        // before the fused op (so a*b - c has one rounding, signed zeros included), on non-NaN
        // lanes only: hardware leaves a NaN operand's sign alone.
        let c = if neg_add {
            let is_nan: __m128 = vx::vx_binary(c, c, 47, 32);
            _mm_xor_ps(c, _mm_andnot_ps(is_nan, _mm_set1_ps(-0.0)))
        } else {
            c
        };
        if neg_prod { vx::vx_ternary(c, a, b, 9) } else { vx::vx_ternary(c, a, b, 8) }
    } else {
        let (x, y, z) = (a.0, b.0, c.0);
        let mut r = [0f32; 4];
        let mut i = 0;
        while i < 4 {
            let p = if neg_prod { -x[i] } else { x[i] };
            let q = if neg_add { -z[i] } else { z[i] };
            r[i] = p.mul_add(y[i], q);
            i += 1;
        }
        __m128(r)
    }
}
pub unsafe fn _mm_fmadd_ps(a: __m128, b: __m128, c: __m128) -> __m128 { fma_ps(a, b, c, false, false) }
pub unsafe fn _mm_fmsub_ps(a: __m128, b: __m128, c: __m128) -> __m128 { fma_ps(a, b, c, false, true) }
pub unsafe fn _mm_fnmadd_ps(a: __m128, b: __m128, c: __m128) -> __m128 { fma_ps(a, b, c, true, false) }
pub unsafe fn _mm_fnmsub_ps(a: __m128, b: __m128, c: __m128) -> __m128 { fma_ps(a, b, c, true, true) }
pub unsafe fn _mm256_fmadd_ps(a: __m256, b: __m256, c: __m256) -> __m256 {
    ps_join(fma_ps(ps_lo(&a), ps_lo(&b), ps_lo(&c), false, false), fma_ps(ps_hi(&a), ps_hi(&b), ps_hi(&c), false, false))
}
pub unsafe fn _mm256_fmsub_ps(a: __m256, b: __m256, c: __m256) -> __m256 {
    ps_join(fma_ps(ps_lo(&a), ps_lo(&b), ps_lo(&c), false, true), fma_ps(ps_hi(&a), ps_hi(&b), ps_hi(&c), false, true))
}
pub unsafe fn _mm256_fnmadd_ps(a: __m256, b: __m256, c: __m256) -> __m256 {
    ps_join(fma_ps(ps_lo(&a), ps_lo(&b), ps_lo(&c), true, false), fma_ps(ps_hi(&a), ps_hi(&b), ps_hi(&c), true, false))
}
pub unsafe fn _mm256_fnmsub_ps(a: __m256, b: __m256, c: __m256) -> __m256 {
    ps_join(fma_ps(ps_lo(&a), ps_lo(&b), ps_lo(&c), true, true), fma_ps(ps_hi(&a), ps_hi(&b), ps_hi(&c), true, true))
}

pub unsafe fn _mm256_loadu_ps(mem_addr: *const f32) -> __m256 {
    ps_join(vx::vx_load(mem_addr as *const u8), vx::vx_load((mem_addr as *const u8).add(16)))
}
/// 32-byte alignment is the caller's contract (as on hardware); the loads are the same.
pub unsafe fn _mm256_load_ps(mem_addr: *const f32) -> __m256 {
    _mm256_loadu_ps(mem_addr)
}
pub unsafe fn _mm256_storeu_ps(mem_addr: *mut f32, a: __m256) {
    vx::vx_store(mem_addr as *mut u8, ps_lo(&a));
    vx::vx_store((mem_addr as *mut u8).add(16), ps_hi(&a));
}
pub unsafe fn _mm256_store_ps(mem_addr: *mut f32, a: __m256) {
    _mm256_storeu_ps(mem_addr, a)
}
pub unsafe fn _mm256_set1_ps(a: f32) -> __m256 {
    let v: __m128 = vx::vx_splat(a.to_bits() as u64, 32);
    ps_join(v, v)
}
pub unsafe fn _mm256_setzero_ps() -> __m256 {
    let z = _mm_setzero_ps();
    ps_join(z, z)
}
pub unsafe fn _mm256_castps256_ps128(a: __m256) -> __m128 {
    ps_lo(&a)
}
#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
pub unsafe fn _mm256_extractf128_ps<const IMM1: i32>(a: __m256) -> __m128 {
    if IMM1 & 1 == 0 { ps_lo(&a) } else { ps_hi(&a) }
}

#[inline(always)]
unsafe fn i_lo(a: &__m256i) -> __m128i {
    vx::vx_load(a as *const __m256i as *const u8)
}
#[inline(always)]
unsafe fn i_hi(a: &__m256i) -> __m128i {
    vx::vx_load((a as *const __m256i as *const u8).add(16))
}
#[inline(always)]
unsafe fn i_join(lo: __m128i, hi: __m128i) -> __m256i {
    crate::mem::transmute::<[__m128i; 2], __m256i>([lo, hi])
}

// AVX2 integer operations that work within each 128-bit lane are the 128-bit bridge operation on
// each half (lane order is memory order); lane-crossing ones (permutes, extracts, converts)
// stay scalar.
pub unsafe fn _mm256_add_epi16(a: __m256i, b: __m256i) -> __m256i {
    i_join(_mm_add_epi16(i_lo(&a), i_lo(&b)), _mm_add_epi16(i_hi(&a), i_hi(&b)))
}
pub unsafe fn _mm256_add_epi32(a: __m256i, b: __m256i) -> __m256i {
    i_join(_mm_add_epi32(i_lo(&a), i_lo(&b)), _mm_add_epi32(i_hi(&a), i_hi(&b)))
}
// rustc library/stdarch/crates/core_arch/src/x86/avx2.rs: _mm256_add_epi64 / _mm256_sub_epi64.
#[cfg_attr(any(target_arch = "x86", target_arch = "x86_64"), target_feature(enable = "avx2"))]
pub unsafe fn _mm256_add_epi64(a: __m256i, b: __m256i) -> __m256i {
    i_join(_mm_add_epi64(i_lo(&a), i_lo(&b)), _mm_add_epi64(i_hi(&a), i_hi(&b)))
}
#[cfg_attr(any(target_arch = "x86", target_arch = "x86_64"), target_feature(enable = "avx2"))]
pub unsafe fn _mm256_sub_epi64(a: __m256i, b: __m256i) -> __m256i {
    i_join(_mm_sub_epi64(i_lo(&a), i_lo(&b)), _mm_sub_epi64(i_hi(&a), i_hi(&b)))
}
pub unsafe fn _mm256_sub_epi16(a: __m256i, b: __m256i) -> __m256i {
    i_join(_mm_sub_epi16(i_lo(&a), i_lo(&b)), _mm_sub_epi16(i_hi(&a), i_hi(&b)))
}
pub unsafe fn _mm256_sub_epi32(a: __m256i, b: __m256i) -> __m256i {
    i_join(_mm_sub_epi32(i_lo(&a), i_lo(&b)), _mm_sub_epi32(i_hi(&a), i_hi(&b)))
}
pub unsafe fn _mm256_cmpeq_epi8(a: __m256i, b: __m256i) -> __m256i {
    i_join(_mm_cmpeq_epi8(i_lo(&a), i_lo(&b)), _mm_cmpeq_epi8(i_hi(&a), i_hi(&b)))
}
pub unsafe fn _mm256_cmpgt_epi8(a: __m256i, b: __m256i) -> __m256i {
    i_join(_mm_cmpgt_epi8(i_lo(&a), i_lo(&b)), _mm_cmpgt_epi8(i_hi(&a), i_hi(&b)))
}
#[cfg_attr(not(relax_check), rustc_legacy_const_generics(2))]
pub unsafe fn _mm256_alignr_epi8<const IMM8: i32>(a: __m256i, b: __m256i) -> __m256i {
    i_join(_mm_alignr_epi8::<IMM8>(i_lo(&a), i_lo(&b)), _mm_alignr_epi8::<IMM8>(i_hi(&a), i_hi(&b)))
}
pub unsafe fn _mm256_madd_epi16(a: __m256i, b: __m256i) -> __m256i {
    i_join(_mm_madd_epi16(i_lo(&a), i_lo(&b)), _mm_madd_epi16(i_hi(&a), i_hi(&b)))
}
pub unsafe fn _mm256_max_epi16(a: __m256i, b: __m256i) -> __m256i {
    i_join(_mm_max_epi16(i_lo(&a), i_lo(&b)), _mm_max_epi16(i_hi(&a), i_hi(&b)))
}
pub unsafe fn _mm256_min_epi16(a: __m256i, b: __m256i) -> __m256i {
    i_join(_mm_min_epi16(i_lo(&a), i_lo(&b)), _mm_min_epi16(i_hi(&a), i_hi(&b)))
}
pub unsafe fn _mm256_sad_epu8(a: __m256i, b: __m256i) -> __m256i {
    i_join(_mm_sad_epu8(i_lo(&a), i_lo(&b)), _mm_sad_epu8(i_hi(&a), i_hi(&b)))
}
pub unsafe fn _mm256_shuffle_epi8(a: __m256i, b: __m256i) -> __m256i {
    i_join(_mm_shuffle_epi8(i_lo(&a), i_lo(&b)), _mm_shuffle_epi8(i_hi(&a), i_hi(&b)))
}
pub unsafe fn _mm256_packs_epi32(a: __m256i, b: __m256i) -> __m256i {
    i_join(_mm_packs_epi32(i_lo(&a), i_lo(&b)), _mm_packs_epi32(i_hi(&a), i_hi(&b)))
}
pub unsafe fn _mm256_packus_epi16(a: __m256i, b: __m256i) -> __m256i {
    i_join(_mm_packus_epi16(i_lo(&a), i_lo(&b)), _mm_packus_epi16(i_hi(&a), i_hi(&b)))
}
pub unsafe fn _mm256_packus_epi32(a: __m256i, b: __m256i) -> __m256i {
    i_join(_mm_packus_epi32(i_lo(&a), i_lo(&b)), _mm_packus_epi32(i_hi(&a), i_hi(&b)))
}
pub unsafe fn _mm256_maddubs_epi16(a: __m256i, b: __m256i) -> __m256i {
    i_join(_mm_maddubs_epi16(i_lo(&a), i_lo(&b)), _mm_maddubs_epi16(i_hi(&a), i_hi(&b)))
}
pub unsafe fn _mm256_unpacklo_epi8(a: __m256i, b: __m256i) -> __m256i {
    i_join(_mm_unpacklo_epi8(i_lo(&a), i_lo(&b)), _mm_unpacklo_epi8(i_hi(&a), i_hi(&b)))
}
pub unsafe fn _mm256_unpackhi_epi8(a: __m256i, b: __m256i) -> __m256i {
    i_join(_mm_unpackhi_epi8(i_lo(&a), i_lo(&b)), _mm_unpackhi_epi8(i_hi(&a), i_hi(&b)))
}
pub unsafe fn _mm256_unpacklo_epi16(a: __m256i, b: __m256i) -> __m256i {
    i_join(_mm_unpacklo_epi16(i_lo(&a), i_lo(&b)), _mm_unpacklo_epi16(i_hi(&a), i_hi(&b)))
}
pub unsafe fn _mm256_unpackhi_epi16(a: __m256i, b: __m256i) -> __m256i {
    i_join(_mm_unpackhi_epi16(i_lo(&a), i_lo(&b)), _mm_unpackhi_epi16(i_hi(&a), i_hi(&b)))
}
pub unsafe fn _mm256_unpacklo_epi32(a: __m256i, b: __m256i) -> __m256i {
    i_join(_mm_unpacklo_epi32(i_lo(&a), i_lo(&b)), _mm_unpacklo_epi32(i_hi(&a), i_hi(&b)))
}
pub unsafe fn _mm256_unpackhi_epi32(a: __m256i, b: __m256i) -> __m256i {
    i_join(_mm_unpackhi_epi32(i_lo(&a), i_lo(&b)), _mm_unpackhi_epi32(i_hi(&a), i_hi(&b)))
}
pub unsafe fn _mm256_unpacklo_epi64(a: __m256i, b: __m256i) -> __m256i {
    i_join(_mm_unpacklo_epi64(i_lo(&a), i_lo(&b)), _mm_unpacklo_epi64(i_hi(&a), i_hi(&b)))
}
pub unsafe fn _mm256_unpackhi_epi64(a: __m256i, b: __m256i) -> __m256i {
    i_join(_mm_unpackhi_epi64(i_lo(&a), i_lo(&b)), _mm_unpackhi_epi64(i_hi(&a), i_hi(&b)))
}
pub unsafe fn _mm256_blendv_epi8(a: __m256i, b: __m256i, mask: __m256i) -> __m256i {
    i_join(_mm_blendv_epi8(i_lo(&a), i_lo(&b), i_lo(&mask)), _mm_blendv_epi8(i_hi(&a), i_hi(&b), i_hi(&mask)))
}
pub unsafe fn _mm256_and_si256(a: __m256i, b: __m256i) -> __m256i {
    i_join(_mm_and_si128(i_lo(&a), i_lo(&b)), _mm_and_si128(i_hi(&a), i_hi(&b)))
}
pub unsafe fn _mm256_or_si256(a: __m256i, b: __m256i) -> __m256i {
    i_join(_mm_or_si128(i_lo(&a), i_lo(&b)), _mm_or_si128(i_hi(&a), i_hi(&b)))
}
pub unsafe fn _mm256_xor_si256(a: __m256i, b: __m256i) -> __m256i {
    i_join(_mm_xor_si128(i_lo(&a), i_lo(&b)), _mm_xor_si128(i_hi(&a), i_hi(&b)))
}
pub unsafe fn _mm256_subs_epu8(a: __m256i, b: __m256i) -> __m256i {
    i_join(_mm_subs_epu8(i_lo(&a), i_lo(&b)), _mm_subs_epu8(i_hi(&a), i_hi(&b)))
}
pub unsafe fn _mm_testz_si128(a: __m128i, b: __m128i) -> i32 {
    if (a.0[0] & b.0[0]) | (a.0[1] & b.0[1]) == 0 { 1 } else { 0 }
}
pub const _MM_HINT_T0: i32 = 3;
pub const _MM_HINT_T1: i32 = 2;
pub const _MM_HINT_T2: i32 = 1;
pub const _MM_HINT_NTA: i32 = 0;
pub const _MM_HINT_ET0: i32 = 7;
pub const _MM_HINT_ET1: i32 = 6;
#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
pub unsafe fn _mm_prefetch<const STRATEGY: i32>(_p: *const i8) {}
#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
pub unsafe fn _mm256_slli_epi16<const IMM8: i32>(a: __m256i) -> __m256i {
    i_join(_mm_slli_epi16::<IMM8>(i_lo(&a)), _mm_slli_epi16::<IMM8>(i_hi(&a)))
}
#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
pub unsafe fn _mm256_slli_epi32<const IMM8: i32>(a: __m256i) -> __m256i {
    i_join(_mm_slli_epi32::<IMM8>(i_lo(&a)), _mm_slli_epi32::<IMM8>(i_hi(&a)))
}
// rustc library/stdarch/crates/core_arch/src/x86/avx2.rs: _mm256_slli_epi64 / _mm256_srli_epi64.
// Counts >= 64 zero every lane, rather than wrapping the count modulo 64.
#[cfg_attr(any(target_arch = "x86", target_arch = "x86_64"), target_feature(enable = "avx2"))]
#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
pub unsafe fn _mm256_slli_epi64<const IMM8: i32>(a: __m256i) -> __m256i {
    const { assert!(IMM8 >= 0 && IMM8 < 256) };
    if IMM8 >= 64 {
        _mm256_setzero_si256()
    } else {
        i_join(_mm_slli_epi64::<IMM8>(i_lo(&a)), _mm_slli_epi64::<IMM8>(i_hi(&a)))
    }
}
#[cfg_attr(any(target_arch = "x86", target_arch = "x86_64"), target_feature(enable = "avx2"))]
#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
pub unsafe fn _mm256_srli_epi64<const IMM8: i32>(a: __m256i) -> __m256i {
    const { assert!(IMM8 >= 0 && IMM8 < 256) };
    if IMM8 >= 64 {
        _mm256_setzero_si256()
    } else {
        i_join(_mm_srli_epi64::<IMM8>(i_lo(&a)), _mm_srli_epi64::<IMM8>(i_hi(&a)))
    }
}
#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
pub unsafe fn _mm256_srai_epi16<const IMM8: i32>(a: __m256i) -> __m256i {
    i_join(_mm_srai_epi16::<IMM8>(i_lo(&a)), _mm_srai_epi16::<IMM8>(i_hi(&a)))
}
#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
pub unsafe fn _mm256_srai_epi32<const IMM8: i32>(a: __m256i) -> __m256i {
    i_join(_mm_srai_epi32::<IMM8>(i_lo(&a)), _mm_srai_epi32::<IMM8>(i_hi(&a)))
}
#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
pub unsafe fn _mm256_srli_epi16<const IMM8: i32>(a: __m256i) -> __m256i {
    i_join(_mm_srli_epi16::<IMM8>(i_lo(&a)), _mm_srli_epi16::<IMM8>(i_hi(&a)))
}
#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
pub unsafe fn _mm256_srli_epi32<const IMM8: i32>(a: __m256i) -> __m256i {
    i_join(_mm_srli_epi32::<IMM8>(i_lo(&a)), _mm_srli_epi32::<IMM8>(i_hi(&a)))
}
pub unsafe fn _mm256_loadu_si256(mem_addr: *const __m256i) -> __m256i {
    i_join(vx::vx_load(mem_addr as *const u8), vx::vx_load((mem_addr as *const u8).add(16)))
}
pub unsafe fn _mm256_load_si256(mem_addr: *const __m256i) -> __m256i {
    _mm256_loadu_si256(mem_addr)
}
pub unsafe fn _mm256_storeu_si256(mem_addr: *mut __m256i, a: __m256i) {
    vx::vx_store(mem_addr as *mut u8, i_lo(&a));
    vx::vx_store((mem_addr as *mut u8).add(16), i_hi(&a));
}
pub unsafe fn _mm256_setzero_si256() -> __m256i {
    let z = _mm_setzero_si128();
    i_join(z, z)
}

// vzeroupper / vzeroall clear the upper halves of the ymm registers; a value held in a variable is
// never one of them here (a __m256 is two halves in memory or xmm registers): no effect to model
// (rand_chacha's refill_wide AVX2 path ends with _mm256_zeroupper)
pub unsafe fn _mm256_zeroupper() {}
pub unsafe fn _mm256_zeroall() {}

pub unsafe fn _mm512_add_epi32(a: __m512i, b: __m512i) -> __m512i {
    let a = lanes512_i32(a);
    let b = lanes512_i32(b);
    let mut r = [0i32; 16];
    let mut i = 0;
    while i < 16 {
        r[i] = a[i].wrapping_add(b[i]);
        i += 1;
    }
    from512_i32(r)
}

pub unsafe fn _mm512_loadu_si512(mem_addr: *const __m512i) -> __m512i {
    crate::ptr::read_unaligned(mem_addr)
}

pub unsafe fn _mm512_madd_epi16(a: __m512i, b: __m512i) -> __m512i {
    let a = lanes512_i16(a);
    let b = lanes512_i16(b);
    let mut r = [0i32; 16];
    let mut i = 0;
    while i < 16 {
        r[i] = (a[i * 2] as i32 * b[i * 2] as i32)
            .wrapping_add(a[i * 2 + 1] as i32 * b[i * 2 + 1] as i32);
        i += 1;
    }
    from512_i32(r)
}

pub unsafe fn _mm512_maddubs_epi16(a: __m512i, b: __m512i) -> __m512i {
    let a = lanes512_u8(a);
    let b = lanes512_i8(b);
    let mut r = [0i16; 32];
    let mut i = 0;
    while i < 32 {
        r[i] = ((a[i * 2] as i32 * b[i * 2] as i32) + (a[i * 2 + 1] as i32 * b[i * 2 + 1] as i32))
            .max(-32768)
            .min(32767) as i16;
        i += 1;
    }
    from512_i16(r)
}

pub unsafe fn _mm512_sad_epu8(a: __m512i, b: __m512i) -> __m512i {
    let a = lanes512_u8(a);
    let b = lanes512_u8(b);
    let mut r = [0i64; 8];
    let mut i = 0;
    while i < 64 {
        r[i / 8] += ((a[i] as i16) - (b[i] as i16)).abs() as i64;
        i += 1;
    }
    __m512i(r)
}

pub unsafe fn _mm512_set1_epi16(a: i16) -> __m512i {
    from512_i16([a; 32])
}

pub unsafe fn _mm512_set_epi32(
    e15: i32,
    e14: i32,
    e13: i32,
    e12: i32,
    e11: i32,
    e10: i32,
    e9: i32,
    e8: i32,
    e7: i32,
    e6: i32,
    e5: i32,
    e4: i32,
    e3: i32,
    e2: i32,
    e1: i32,
    e0: i32,
) -> __m512i {
    from512_i32([
        e0, e1, e2, e3, e4, e5, e6, e7, e8, e9, e10, e11, e12, e13, e14, e15,
    ])
}

pub unsafe fn _mm512_set_epi8(
    e63: i8,
    e62: i8,
    e61: i8,
    e60: i8,
    e59: i8,
    e58: i8,
    e57: i8,
    e56: i8,
    e55: i8,
    e54: i8,
    e53: i8,
    e52: i8,
    e51: i8,
    e50: i8,
    e49: i8,
    e48: i8,
    e47: i8,
    e46: i8,
    e45: i8,
    e44: i8,
    e43: i8,
    e42: i8,
    e41: i8,
    e40: i8,
    e39: i8,
    e38: i8,
    e37: i8,
    e36: i8,
    e35: i8,
    e34: i8,
    e33: i8,
    e32: i8,
    e31: i8,
    e30: i8,
    e29: i8,
    e28: i8,
    e27: i8,
    e26: i8,
    e25: i8,
    e24: i8,
    e23: i8,
    e22: i8,
    e21: i8,
    e20: i8,
    e19: i8,
    e18: i8,
    e17: i8,
    e16: i8,
    e15: i8,
    e14: i8,
    e13: i8,
    e12: i8,
    e11: i8,
    e10: i8,
    e9: i8,
    e8: i8,
    e7: i8,
    e6: i8,
    e5: i8,
    e4: i8,
    e3: i8,
    e2: i8,
    e1: i8,
    e0: i8,
) -> __m512i {
    from512_i8([
        e0, e1, e2, e3, e4, e5, e6, e7, e8, e9, e10, e11, e12, e13, e14, e15, e16, e17, e18, e19,
        e20, e21, e22, e23, e24, e25, e26, e27, e28, e29, e30, e31, e32, e33, e34, e35, e36, e37,
        e38, e39, e40, e41, e42, e43, e44, e45, e46, e47, e48, e49, e50, e51, e52, e53, e54, e55,
        e56, e57, e58, e59, e60, e61, e62, e63,
    ])
}

pub unsafe fn _mm512_setzero_si512() -> __m512i {
    __m512i([0; 8])
}

pub unsafe fn _mm512_castsi128_si512(a: __m128i) -> __m512i {
    __m512i([a.0[0], a.0[1], 0, 0, 0, 0, 0, 0])
}

pub unsafe fn _mm512_castsi512_si128(a: __m512i) -> __m128i {
    __m128i([a.0[0], a.0[1]])
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
pub unsafe fn _mm512_extracti32x4_epi32<const IMM8: i32>(a: __m512i) -> __m128i {
    let idx = ((IMM8 & 3) as usize) * 2;
    __m128i([a.0[idx], a.0[idx + 1]])
}

pub unsafe fn _mm512_set_epi64(
    e7: i64,
    e6: i64,
    e5: i64,
    e4: i64,
    e3: i64,
    e2: i64,
    e1: i64,
    e0: i64,
) -> __m512i {
    __m512i([e0, e1, e2, e3, e4, e5, e6, e7])
}

pub unsafe fn _mm512_xor_si512(a: __m512i, b: __m512i) -> __m512i {
    let mut r = [0i64; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = a.0[i] ^ b.0[i];
        i += 1;
    }
    __m512i(r)
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(2))]
pub unsafe fn _mm512_clmulepi64_epi128<const IMM8: i32>(a: __m512i, b: __m512i) -> __m512i {
    let l0 = _mm_clmulepi64_si128::<IMM8>(__m128i([a.0[0], a.0[1]]), __m128i([b.0[0], b.0[1]]));
    let l1 = _mm_clmulepi64_si128::<IMM8>(__m128i([a.0[2], a.0[3]]), __m128i([b.0[2], b.0[3]]));
    let l2 = _mm_clmulepi64_si128::<IMM8>(__m128i([a.0[4], a.0[5]]), __m128i([b.0[4], b.0[5]]));
    let l3 = _mm_clmulepi64_si128::<IMM8>(__m128i([a.0[6], a.0[7]]), __m128i([b.0[6], b.0[7]]));
    __m512i([l0.0[0], l0.0[1], l1.0[0], l1.0[1], l2.0[0], l2.0[1], l3.0[0], l3.0[1]])
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
pub unsafe fn _mm512_slli_epi32<const IMM8: u32>(a: __m512i) -> __m512i {
    let a = lanes512_i32(a);
    let mut r = [0i32; 16];
    let mut i = 0;
    while i < 16 {
        r[i] = if (IMM8 as u32) < 32 {
            a[i].wrapping_shl(IMM8 as u32)
        } else {
            0
        };
        i += 1;
    }
    from512_i32(r)
}

#[inline(always)]
pub unsafe fn _mm_abs_epi16(a: __m128i) -> __m128i {
    vx::vx_unary(a, 0, 16, 0)
}

#[inline(always)]
pub unsafe fn _mm_add_epi16(a: __m128i, b: __m128i) -> __m128i {
    vx::vx_binary(a, b, 0, 16)
}

#[inline(always)]
pub unsafe fn _mm_add_epi32(a: __m128i, b: __m128i) -> __m128i {
    vx::vx_binary(a, b, 0, 32)
}

#[inline(always)]
pub unsafe fn _mm_add_epi8(a: __m128i, b: __m128i) -> __m128i {
    vx::vx_binary(a, b, 0, 8)
}

#[inline(always)]
pub unsafe fn _mm_and_si128(a: __m128i, b: __m128i) -> __m128i {
    vx::vx_binary(a, b, 6, 32)
}

#[inline(always)]
pub unsafe fn _mm_andnot_si128(a: __m128i, b: __m128i) -> __m128i {
    vx::vx_binary(b, a, 9, 32)
}

#[inline(always)]
pub unsafe fn _mm_avg_epu8(a: __m128i, b: __m128i) -> __m128i {
    vx::vx_binary(a, b, 16, 8)
}

#[inline(always)]
pub unsafe fn _mm_blendv_epi8(a: __m128i, b: __m128i, mask: __m128i) -> __m128i {
    vx::vx_blend(a, b, mask)
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(2))]
pub unsafe fn _mm_clmulepi64_si128<const IMM8: i32>(a: __m128i, b: __m128i) -> __m128i {
    let x = a.0[(IMM8 & 1) as usize] as u64;
    let y = b.0[((IMM8 >> 4) & 1) as usize] as u64;
    let mut lo = 0u64;
    let mut hi = 0u64;
    let mut i = 0;
    while i < 64 {
        if (y >> i) & 1 != 0 {
            lo ^= x << i;
            if i != 0 {
                hi ^= x >> (64 - i);
            }
        }
        i += 1;
    }
    __m128i([lo as i64, hi as i64])
}

#[inline(always)]
pub unsafe fn _mm_cmpeq_epi8(a: __m128i, b: __m128i) -> __m128i {
    vx::vx_binary(a, b, 10, 8)
}

#[inline(always)]
pub unsafe fn _mm_cmpeq_epi16(a: __m128i, b: __m128i) -> __m128i {
    vx::vx_binary(a, b, 10, 16)
}

#[inline(always)]
pub unsafe fn _mm_cmpgt_epi8(a: __m128i, b: __m128i) -> __m128i {
    vx::vx_binary(a, b, 11, 8)
}

pub unsafe fn _mm_cvtsi128_si32(a: __m128i) -> i32 {
    a.0[0] as i32
}

pub unsafe fn _mm_cvtsi128_si64(a: __m128i) -> i64 {
    a.0[0]
}

pub unsafe fn _mm_cvtsi128_si64x(a: __m128i) -> i64 {
    a.0[0]
}

pub unsafe fn _mm_cvtsi32_si128(a: i32) -> __m128i {
    __m128i([a as u32 as i64, 0])
}

pub unsafe fn _mm_cvtsi64_si128(a: i64) -> __m128i {
    __m128i([a, 0])
}

pub unsafe fn _mm_cvtsi64x_si128(a: i64) -> __m128i {
    __m128i([a, 0])
}

pub unsafe fn _mm_mul_epu32(a: __m128i, b: __m128i) -> __m128i {
    let a = lanes128_u32(a);
    let b = lanes128_u32(b);
    __m128i([
        (a[0] as u64 * b[0] as u64) as i64,
        (a[2] as u64 * b[2] as u64) as i64,
    ])
}

pub unsafe fn _mm_mullo_epi16(a: __m128i, b: __m128i) -> __m128i {
    let a = lanes128_u16(a);
    let b = lanes128_u16(b);
    let mut r = [0u16; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = a[i].wrapping_mul(b[i]);
        i += 1;
    }
    from128_u16(r)
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
pub unsafe fn _mm_extract_epi32<const IMM8: i32>(a: __m128i) -> i32 {
    lanes128_i32(a)[IMM8 as usize]
}

#[inline(always)]
pub unsafe fn _mm_load_si128(mem_addr: *const __m128i) -> __m128i {
    vx::vx_load(mem_addr as *const u8)
}

#[inline(always)]
pub unsafe fn _mm_loadu_si128(mem_addr: *const __m128i) -> __m128i {
    vx::vx_load(mem_addr as *const u8)
}

#[inline(always)]
pub unsafe fn _mm_madd_epi16(a: __m128i, b: __m128i) -> __m128i {
    vx::vx_binary(a, b, 23, 16)
}

#[inline(always)]
pub unsafe fn _mm_maddubs_epi16(a: __m128i, b: __m128i) -> __m128i {
    vx::vx_binary(a, b, 27, 8)
}

#[inline(always)]
pub unsafe fn _mm_min_epi16(a: __m128i, b: __m128i) -> __m128i {
    vx::vx_binary(a, b, 12, 16)
}

#[inline(always)]
pub unsafe fn _mm_movemask_epi8(a: __m128i) -> i32 {
    vx::vx_mask(a, 8)
}

#[inline(always)]
pub unsafe fn _mm_or_si128(a: __m128i, b: __m128i) -> __m128i {
    vx::vx_binary(a, b, 7, 32)
}

#[inline(always)]
pub unsafe fn _mm_packus_epi16(a: __m128i, b: __m128i) -> __m128i {
    vx::vx_binary(a, b, 19, 16)
}

#[inline(always)]
pub unsafe fn _mm_sad_epu8(a: __m128i, b: __m128i) -> __m128i {
    vx::vx_binary(a, b, 24, 8)
}

#[inline(always)]
pub unsafe fn _mm_set1_epi16(a: i16) -> __m128i {
    vx::vx_splat(a as u64, 16)
}

#[inline(always)]
pub unsafe fn _mm_set1_epi8(a: i8) -> __m128i {
    vx::vx_splat(a as u64, 8)
}

pub unsafe fn _mm_set_epi32(e3: i32, e2: i32, e1: i32, e0: i32) -> __m128i {
    from128_i32([e0, e1, e2, e3])
}

pub unsafe fn _mm_set_epi64x(e1: i64, e0: i64) -> __m128i {
    __m128i([e0, e1])
}

pub unsafe fn _mm_set_epi8(
    e15: i8,
    e14: i8,
    e13: i8,
    e12: i8,
    e11: i8,
    e10: i8,
    e9: i8,
    e8: i8,
    e7: i8,
    e6: i8,
    e5: i8,
    e4: i8,
    e3: i8,
    e2: i8,
    e1: i8,
    e0: i8,
) -> __m128i {
    from128_i8([
        e0, e1, e2, e3, e4, e5, e6, e7, e8, e9, e10, e11, e12, e13, e14, e15,
    ])
}

#[inline(always)]
pub unsafe fn _mm_setzero_si128() -> __m128i {
    vx::vx_splat(0, 32)
}

#[inline(always)]
pub unsafe fn _mm_sha1msg1_epu32(a: __m128i, b: __m128i) -> __m128i {
    vx::vx_binary(a, b, 71, 32)
}

#[inline(always)]
pub unsafe fn _mm_sha1msg2_epu32(a: __m128i, b: __m128i) -> __m128i {
    vx::vx_binary(a, b, 72, 32)
}

#[inline(always)]
pub unsafe fn _mm_sha1nexte_epu32(a: __m128i, b: __m128i) -> __m128i {
    vx::vx_binary(a, b, 70, 32)
}

// Intel stores [D, C, B, A] from low to high; b[3] includes the initial E.
// SHA1RNDS4 adds the selected constant here (unlike Arm SHA1C/P/M).
#[cfg_attr(not(relax_check), rustc_legacy_const_generics(2))]
#[inline(always)]
pub unsafe fn _mm_sha1rnds4_epu32<const FUNC: i32>(a: __m128i, b: __m128i) -> __m128i {
    vx::vx_sha1rnds4(a, b, FUNC)
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
#[inline(always)]
pub unsafe fn _mm_shuffle_epi32<const IMM8: i32>(a: __m128i) -> __m128i {
    vx::vx_unary(a, 4, 32, IMM8)
}

#[inline(always)]
pub unsafe fn _mm_shuffle_epi8(a: __m128i, b: __m128i) -> __m128i {
    vx::vx_binary(a, b, 25, 8)
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
#[inline(always)]
pub unsafe fn _mm_slli_epi32<const IMM8: i32>(a: __m128i) -> __m128i {
    vx::vx_unary(a, 1, 32, IMM8)
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
#[inline(always)]
pub unsafe fn _mm_srli_si128<const IMM8: i32>(a: __m128i) -> __m128i {
    vx::vx_unary(a, 5, 8, IMM8)
}

#[inline(always)]
pub unsafe fn _mm_store_si128(mem_addr: *mut __m128i, a: __m128i) {
    vx::vx_store(mem_addr as *mut u8, a)
}

#[inline(always)]
pub unsafe fn _mm_storeu_si128(mem_addr: *mut __m128i, a: __m128i) {
    vx::vx_store(mem_addr as *mut u8, a)
}

#[inline(always)]
pub unsafe fn _mm_sub_epi16(a: __m128i, b: __m128i) -> __m128i {
    vx::vx_binary(a, b, 1, 16)
}

#[inline(always)]
pub unsafe fn _mm_sub_epi8(a: __m128i, b: __m128i) -> __m128i {
    vx::vx_binary(a, b, 1, 8)
}

#[inline(always)]
pub unsafe fn _mm_unpackhi_epi64(a: __m128i, b: __m128i) -> __m128i {
    vx::vx_binary(a, b, 18, 64)
}

#[inline(always)]
pub unsafe fn _mm_unpackhi_epi8(a: __m128i, b: __m128i) -> __m128i {
    vx::vx_binary(a, b, 18, 8)
}

#[inline(always)]
pub unsafe fn _mm_unpacklo_epi8(a: __m128i, b: __m128i) -> __m128i {
    vx::vx_binary(a, b, 17, 8)
}

#[inline(always)]
pub unsafe fn _mm_xor_si128(a: __m128i, b: __m128i) -> __m128i {
    vx::vx_binary(a, b, 8, 32)
}

#[inline(always)]
pub unsafe fn _mm_add_ps(a: __m128, b: __m128) -> __m128 {
    vx::vx_binary(a, b, 40, 32)
}

pub unsafe fn _mm_add_ss(a: __m128, b: __m128) -> __m128 {
    __m128([a.0[0] + b.0[0], a.0[1], a.0[2], a.0[3]])
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(2))]
pub unsafe fn _mm_alignr_epi8<const IMM8: i32>(a: __m128i, b: __m128i) -> __m128i {
    let a = lanes128_u8(a);
    let b = lanes128_u8(b);
    let mut r = [0u8; 16];
    let mut i = 0;
    while i < 16 {
        let j = i + IMM8 as u32 as usize;
        r[i] = if j < 16 {
            b[j]
        } else if j < 32 {
            a[j - 16]
        } else {
            0
        };
        i += 1;
    }
    from128_u8(r)
}

#[inline(always)]
pub unsafe fn _mm_and_ps(a: __m128, b: __m128) -> __m128 {
    vx::vx_binary(a, b, 6, 32)
}

#[inline(always)]
pub unsafe fn _mm_andnot_ps(a: __m128, b: __m128) -> __m128 {
    vx::vx_binary(b, a, 9, 32)
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(2))]
pub unsafe fn _mm_blend_epi16<const IMM8: i32>(a: __m128i, b: __m128i) -> __m128i {
    let a = lanes128_i16(a);
    let b = lanes128_i16(b);
    let mut r = [0i16; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = if (IMM8 >> i) & 1 != 0 { b[i] } else { a[i] };
        i += 1;
    }
    from128_i16(r)
}

#[inline(always)]
pub unsafe fn _mm_cmpeq_epi32(a: __m128i, b: __m128i) -> __m128i {
    vx::vx_binary(a, b, 10, 32)
}

#[inline(always)]
pub unsafe fn _mm_cmpeq_ps(a: __m128, b: __m128) -> __m128 {
    vx::vx_binary(a, b, 46, 32)
}

#[inline(always)]
pub unsafe fn _mm_cmpge_ps(a: __m128, b: __m128) -> __m128 {
    vx::vx_binary(a, b, 51, 32)
}

#[inline(always)]
pub unsafe fn _mm_cmpgt_epi16(a: __m128i, b: __m128i) -> __m128i {
    vx::vx_binary(a, b, 11, 16)
}

#[inline(always)]
pub unsafe fn _mm_cmpgt_epi32(a: __m128i, b: __m128i) -> __m128i {
    vx::vx_binary(a, b, 11, 32)
}

#[inline(always)]
pub unsafe fn _mm_cmplt_epi8(a: __m128i, b: __m128i) -> __m128i {
    _mm_cmpgt_epi8(b, a)
}

#[inline(always)]
pub unsafe fn _mm_cmplt_epi16(a: __m128i, b: __m128i) -> __m128i {
    _mm_cmpgt_epi16(b, a)
}

#[inline(always)]
pub unsafe fn _mm_cmplt_epi32(a: __m128i, b: __m128i) -> __m128i {
    _mm_cmpgt_epi32(b, a)
}

#[inline(always)]
pub unsafe fn _mm_cmpgt_ps(a: __m128, b: __m128) -> __m128 {
    vx::vx_binary(a, b, 50, 32)
}

#[inline(always)]
pub unsafe fn _mm_cmple_ps(a: __m128, b: __m128) -> __m128 {
    vx::vx_binary(a, b, 49, 32)
}

#[inline(always)]
pub unsafe fn _mm_cmplt_ps(a: __m128, b: __m128) -> __m128 {
    vx::vx_binary(a, b, 48, 32)
}

#[inline(always)]
pub unsafe fn _mm_cmpord_ps(a: __m128, b: __m128) -> __m128 {
    _mm_and_ps(_mm_cmpeq_ps(a, a), _mm_cmpeq_ps(b, b))
}

#[inline(always)]
pub unsafe fn _mm_cmpunord_ps(a: __m128, b: __m128) -> __m128 {
    _mm_or_ps(_mm_cmpneq_ps(a, a), _mm_cmpneq_ps(b, b))
}

pub unsafe fn _mm_cvtss_f32(a: __m128) -> f32 {
    a.0[0]
}

#[inline(always)]
pub unsafe fn _mm_div_ps(a: __m128, b: __m128) -> __m128 {
    vx::vx_binary(a, b, 43, 32)
}

pub unsafe fn _mm_loadl_epi64(mem_addr: *const __m128i) -> __m128i {
    __m128i([crate::ptr::read_unaligned(mem_addr as *const i64), 0])
}

#[inline(always)]
pub unsafe fn _mm_load_ps(mem_addr: *const f32) -> __m128 {
    vx::vx_load(mem_addr as *const u8)
}

#[inline(always)]
pub unsafe fn _mm_loadu_ps(mem_addr: *const f32) -> __m128 {
    vx::vx_load(mem_addr as *const u8)
}

// Intel: a < b ? a : b / a > b ? a : b, so either operand NaN returns b.
#[inline(always)]
pub unsafe fn _mm_max_ps(a: __m128, b: __m128) -> __m128 {
    vx::vx_binary(a, b, 45, 32)
}

#[inline(always)]
pub unsafe fn _mm_min_ps(a: __m128, b: __m128) -> __m128 {
    vx::vx_binary(a, b, 44, 32)
}

pub unsafe fn _mm_movehdup_ps(a: __m128) -> __m128 {
    __m128([a.0[1], a.0[1], a.0[3], a.0[3]])
}

pub unsafe fn _mm_movehl_ps(a: __m128, b: __m128) -> __m128 {
    vx::vx_binary(b, a, 18, 64)
}

// the sign bit of each f32 lane, lane i at bit i (movmskps)
#[inline(always)]
pub unsafe fn _mm_movemask_ps(a: __m128) -> i32 {
    vx::vx_mask(a, 32)
}

#[inline(always)]
pub unsafe fn _mm_mul_ps(a: __m128, b: __m128) -> __m128 {
    vx::vx_binary(a, b, 42, 32)
}

#[inline(always)]
pub unsafe fn _mm_or_ps(a: __m128, b: __m128) -> __m128 {
    vx::vx_binary(a, b, 7, 32)
}

// computed exactly; hardware returns a 12-bit approximation
pub unsafe fn _mm_rsqrt_ps(a: __m128) -> __m128 {
    let mut r = [0f32; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = 1.0 / a.0[i].sqrt();
        i += 1;
    }
    __m128(r)
}

#[inline(always)]
pub unsafe fn _mm_set1_ps(a: f32) -> __m128 {
    vx::vx_splat(a.to_bits() as u64, 32)
}

#[inline(always)]
pub unsafe fn _mm_set_ps1(a: f32) -> __m128 {
    _mm_set1_ps(a)
}

pub unsafe fn _mm_set_ps(e3: f32, e2: f32, e1: f32, e0: f32) -> __m128 {
    __m128([e0, e1, e2, e3])
}

pub unsafe fn _mm_set_ss(a: f32) -> __m128 {
    __m128([a, 0.0, 0.0, 0.0])
}

pub unsafe fn _mm_setr_epi8(
    e0: i8,
    e1: i8,
    e2: i8,
    e3: i8,
    e4: i8,
    e5: i8,
    e6: i8,
    e7: i8,
    e8: i8,
    e9: i8,
    e10: i8,
    e11: i8,
    e12: i8,
    e13: i8,
    e14: i8,
    e15: i8,
) -> __m128i {
    from128_i8([
        e0, e1, e2, e3, e4, e5, e6, e7, e8, e9, e10, e11, e12, e13, e14, e15,
    ])
}

pub unsafe fn _mm_setr_ps(e0: f32, e1: f32, e2: f32, e3: f32) -> __m128 {
    __m128([e0, e1, e2, e3])
}

pub unsafe fn _mm_setzero_ps() -> __m128 {
    vx::vx_splat(0, 32)
}

// Intel SDM SHA256MSG1: dst[i] = W[i] + sigma0(W[i + 1]) with W0..W3 = a, W4 = b[0].
#[inline(always)]
pub unsafe fn _mm_sha256msg1_epu32(a: __m128i, b: __m128i) -> __m128i {
    vx::vx_binary(a, b, 73, 32)
}

// Intel SDM SHA256MSG2: W16..W19 from a (W16..W19 partial sums) and W14, W15 = b[2], b[3].
#[inline(always)]
pub unsafe fn _mm_sha256msg2_epu32(a: __m128i, b: __m128i) -> __m128i {
    vx::vx_binary(a, b, 74, 32)
}

// Intel SDM SHA256RNDS2: a = CDGH, b = ABEF (each high to low), k[0..2] = WK0, WK1; result = ABEF.
#[inline(always)]
pub unsafe fn _mm_sha256rnds2_epu32(a: __m128i, b: __m128i, k: __m128i) -> __m128i {
    vx::vx_ternary(a, b, k, 0)
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(2))]
pub unsafe fn _mm_shuffle_ps<const MASK: i32>(a: __m128, b: __m128) -> __m128 {
    // lanes 0,1 from a (fields 0..4 of MASK), lanes 2,3 from b (fields 4..8): two lane shuffles
    // and a 64-bit zip, all bridge operations
    let x: __m128 = vx::vx_unary(a, 4, 32, MASK & 0x0f);
    let y: __m128 = vx::vx_unary(b, 4, 32, (MASK >> 4) & 0x0f);
    vx::vx_binary(x, y, 17, 64)
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
#[inline(always)]
pub unsafe fn _mm_slli_epi16<const IMM8: i32>(a: __m128i) -> __m128i {
    vx::vx_unary(a, 1, 16, IMM8)
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
#[inline(always)]
pub unsafe fn _mm_slli_epi64<const IMM8: i32>(a: __m128i) -> __m128i {
    vx::vx_unary(a, 1, 64, IMM8)
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
#[inline(always)]
pub unsafe fn _mm_srai_epi32<const IMM8: i32>(a: __m128i) -> __m128i {
    vx::vx_unary(a, 3, 32, IMM8)
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
#[inline(always)]
pub unsafe fn _mm_srli_epi16<const IMM8: i32>(a: __m128i) -> __m128i {
    vx::vx_unary(a, 2, 16, IMM8)
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
#[inline(always)]
pub unsafe fn _mm_srli_epi32<const IMM8: i32>(a: __m128i) -> __m128i {
    vx::vx_unary(a, 2, 32, IMM8)
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
#[inline(always)]
pub unsafe fn _mm_srli_epi64<const IMM8: i32>(a: __m128i) -> __m128i {
    vx::vx_unary(a, 2, 64, IMM8)
}

#[inline(always)]
pub unsafe fn _mm_sqrt_ps(a: __m128) -> __m128 {
    vx::vx_unary(a, 7, 32, 0)
}

#[inline(always)]
pub unsafe fn _mm_store_ps(mem_addr: *mut f32, a: __m128) {
    vx::vx_store(mem_addr as *mut u8, a)
}

#[inline(always)]
pub unsafe fn _mm_storeu_ps(mem_addr: *mut f32, a: __m128) {
    vx::vx_store(mem_addr as *mut u8, a)
}

#[inline(always)]
pub unsafe fn _mm_sub_ps(a: __m128, b: __m128) -> __m128 {
    vx::vx_binary(a, b, 41, 32)
}

#[inline(always)]
pub unsafe fn _mm_unpackhi_epi16(a: __m128i, b: __m128i) -> __m128i {
    vx::vx_binary(a, b, 18, 16)
}

#[inline(always)]
pub unsafe fn _mm_unpackhi_epi32(a: __m128i, b: __m128i) -> __m128i {
    vx::vx_binary(a, b, 18, 32)
}

pub unsafe fn _mm_unpackhi_ps(a: __m128, b: __m128) -> __m128 {
    vx::vx_binary(a, b, 18, 32)
}

#[inline(always)]
pub unsafe fn _mm_unpacklo_epi16(a: __m128i, b: __m128i) -> __m128i {
    vx::vx_binary(a, b, 17, 16)
}

#[inline(always)]
pub unsafe fn _mm_unpacklo_epi32(a: __m128i, b: __m128i) -> __m128i {
    vx::vx_binary(a, b, 17, 32)
}

#[inline(always)]
pub unsafe fn _mm_unpacklo_epi64(a: __m128i, b: __m128i) -> __m128i {
    vx::vx_binary(a, b, 17, 64)
}

pub unsafe fn _mm_unpacklo_ps(a: __m128, b: __m128) -> __m128 {
    vx::vx_binary(a, b, 17, 32)
}

#[inline(always)]
pub unsafe fn _mm_xor_ps(a: __m128, b: __m128) -> __m128 {
    vx::vx_binary(a, b, 8, 32)
}

// ---------------------------------------------------------------- SSE2 double precision (_pd / _sd)
// stdarch's x86/sse2.rs and x86_64/sse2.rs f64 family. The packed float ops are the bridge's SSE
// float selectors at lane width 64 (addpd .. cmppd, Intel NaN results: the first NaN operand
// quieted, else the negative default NaN; on every host); the scalar `_sd` forms are the packed op
// with lane 1 taken from `a` (move_sd), as the instructions define them. Float -> integer
// conversions follow Intel: rounding by MXCSR (or toward zero for cvtt*), NaN and out-of-range
// lanes give the integer indefinite (0x8000_0000 / 0x8000_0000_0000_0000).

#[inline(always)]
pub unsafe fn _mm_add_pd(a: __m128d, b: __m128d) -> __m128d {
    vx::vx_binary(a, b, 40, 64)
}
#[inline(always)]
pub unsafe fn _mm_sub_pd(a: __m128d, b: __m128d) -> __m128d {
    vx::vx_binary(a, b, 41, 64)
}
#[inline(always)]
pub unsafe fn _mm_mul_pd(a: __m128d, b: __m128d) -> __m128d {
    vx::vx_binary(a, b, 42, 64)
}
#[inline(always)]
pub unsafe fn _mm_div_pd(a: __m128d, b: __m128d) -> __m128d {
    vx::vx_binary(a, b, 43, 64)
}
/// minpd / maxpd: a < b ? a : b / a > b ? a : b, so either operand NaN (or both zero) gives b
#[inline(always)]
pub unsafe fn _mm_min_pd(a: __m128d, b: __m128d) -> __m128d {
    vx::vx_binary(a, b, 44, 64)
}
#[inline(always)]
pub unsafe fn _mm_max_pd(a: __m128d, b: __m128d) -> __m128d {
    vx::vx_binary(a, b, 45, 64)
}
#[inline(always)]
pub unsafe fn _mm_sqrt_pd(a: __m128d) -> __m128d {
    vx::vx_unary(a, 7, 64, 0)
}
#[inline(always)]
pub unsafe fn _mm_and_pd(a: __m128d, b: __m128d) -> __m128d {
    vx::vx_binary(a, b, 6, 64)
}
/// (!a) & b
#[inline(always)]
pub unsafe fn _mm_andnot_pd(a: __m128d, b: __m128d) -> __m128d {
    vx::vx_binary(b, a, 9, 64)
}
#[inline(always)]
pub unsafe fn _mm_or_pd(a: __m128d, b: __m128d) -> __m128d {
    vx::vx_binary(a, b, 7, 64)
}
#[inline(always)]
pub unsafe fn _mm_xor_pd(a: __m128d, b: __m128d) -> __m128d {
    vx::vx_binary(a, b, 8, 64)
}

/// all ones per f64 lane
#[inline(always)]
unsafe fn pd_ones() -> __m128d {
    vx::vx_splat(u64::MAX, 64)
}

// compares: all ones / zero per lane. eq, lt, le, gt, ge are ordered (false for NaN lanes);
// neq and the negated forms (nlt, nle, ngt, nge) are unordered (true for NaN lanes).
#[inline(always)]
pub unsafe fn _mm_cmpeq_pd(a: __m128d, b: __m128d) -> __m128d {
    vx::vx_binary(a, b, 46, 64)
}
#[inline(always)]
pub unsafe fn _mm_cmpneq_pd(a: __m128d, b: __m128d) -> __m128d {
    vx::vx_binary(a, b, 47, 64)
}
#[inline(always)]
pub unsafe fn _mm_cmplt_pd(a: __m128d, b: __m128d) -> __m128d {
    vx::vx_binary(a, b, 48, 64)
}
#[inline(always)]
pub unsafe fn _mm_cmple_pd(a: __m128d, b: __m128d) -> __m128d {
    vx::vx_binary(a, b, 49, 64)
}
#[inline(always)]
pub unsafe fn _mm_cmpgt_pd(a: __m128d, b: __m128d) -> __m128d {
    vx::vx_binary(a, b, 50, 64)
}
#[inline(always)]
pub unsafe fn _mm_cmpge_pd(a: __m128d, b: __m128d) -> __m128d {
    vx::vx_binary(a, b, 51, 64)
}
/// neither lane NaN
#[inline(always)]
pub unsafe fn _mm_cmpord_pd(a: __m128d, b: __m128d) -> __m128d {
    _mm_and_pd(_mm_cmpeq_pd(a, a), _mm_cmpeq_pd(b, b))
}
/// either lane NaN
#[inline(always)]
pub unsafe fn _mm_cmpunord_pd(a: __m128d, b: __m128d) -> __m128d {
    _mm_or_pd(_mm_cmpneq_pd(a, a), _mm_cmpneq_pd(b, b))
}
#[inline(always)]
pub unsafe fn _mm_cmpnlt_pd(a: __m128d, b: __m128d) -> __m128d {
    _mm_xor_pd(_mm_cmplt_pd(a, b), pd_ones())
}
#[inline(always)]
pub unsafe fn _mm_cmpnle_pd(a: __m128d, b: __m128d) -> __m128d {
    _mm_xor_pd(_mm_cmple_pd(a, b), pd_ones())
}
#[inline(always)]
pub unsafe fn _mm_cmpngt_pd(a: __m128d, b: __m128d) -> __m128d {
    _mm_xor_pd(_mm_cmpgt_pd(a, b), pd_ones())
}
#[inline(always)]
pub unsafe fn _mm_cmpnge_pd(a: __m128d, b: __m128d) -> __m128d {
    _mm_xor_pd(_mm_cmpge_pd(a, b), pd_ones())
}

/// [b0, a1]
#[inline(always)]
pub unsafe fn _mm_move_sd(a: __m128d, b: __m128d) -> __m128d {
    // zip_hi([b0, b0], a) = [b0, a1]
    let lo: __m128d = vx::vx_binary(b, b, 17, 64);
    vx::vx_binary(lo, a, 18, 64)
}

pub unsafe fn _mm_add_sd(a: __m128d, b: __m128d) -> __m128d {
    _mm_move_sd(a, _mm_add_pd(a, b))
}
pub unsafe fn _mm_sub_sd(a: __m128d, b: __m128d) -> __m128d {
    _mm_move_sd(a, _mm_sub_pd(a, b))
}
pub unsafe fn _mm_mul_sd(a: __m128d, b: __m128d) -> __m128d {
    _mm_move_sd(a, _mm_mul_pd(a, b))
}
pub unsafe fn _mm_div_sd(a: __m128d, b: __m128d) -> __m128d {
    _mm_move_sd(a, _mm_div_pd(a, b))
}
pub unsafe fn _mm_min_sd(a: __m128d, b: __m128d) -> __m128d {
    _mm_move_sd(a, _mm_min_pd(a, b))
}
pub unsafe fn _mm_max_sd(a: __m128d, b: __m128d) -> __m128d {
    _mm_move_sd(a, _mm_max_pd(a, b))
}
/// [sqrt(b0), a1]
pub unsafe fn _mm_sqrt_sd(a: __m128d, b: __m128d) -> __m128d {
    _mm_move_sd(a, _mm_sqrt_pd(b))
}
pub unsafe fn _mm_cmpeq_sd(a: __m128d, b: __m128d) -> __m128d {
    _mm_move_sd(a, _mm_cmpeq_pd(a, b))
}
pub unsafe fn _mm_cmpneq_sd(a: __m128d, b: __m128d) -> __m128d {
    _mm_move_sd(a, _mm_cmpneq_pd(a, b))
}
pub unsafe fn _mm_cmplt_sd(a: __m128d, b: __m128d) -> __m128d {
    _mm_move_sd(a, _mm_cmplt_pd(a, b))
}
pub unsafe fn _mm_cmple_sd(a: __m128d, b: __m128d) -> __m128d {
    _mm_move_sd(a, _mm_cmple_pd(a, b))
}
pub unsafe fn _mm_cmpgt_sd(a: __m128d, b: __m128d) -> __m128d {
    _mm_move_sd(a, _mm_cmpgt_pd(a, b))
}
pub unsafe fn _mm_cmpge_sd(a: __m128d, b: __m128d) -> __m128d {
    _mm_move_sd(a, _mm_cmpge_pd(a, b))
}
pub unsafe fn _mm_cmpord_sd(a: __m128d, b: __m128d) -> __m128d {
    _mm_move_sd(a, _mm_cmpord_pd(a, b))
}
pub unsafe fn _mm_cmpunord_sd(a: __m128d, b: __m128d) -> __m128d {
    _mm_move_sd(a, _mm_cmpunord_pd(a, b))
}
pub unsafe fn _mm_cmpnlt_sd(a: __m128d, b: __m128d) -> __m128d {
    _mm_move_sd(a, _mm_cmpnlt_pd(a, b))
}
pub unsafe fn _mm_cmpnle_sd(a: __m128d, b: __m128d) -> __m128d {
    _mm_move_sd(a, _mm_cmpnle_pd(a, b))
}
pub unsafe fn _mm_cmpngt_sd(a: __m128d, b: __m128d) -> __m128d {
    _mm_move_sd(a, _mm_cmpngt_pd(a, b))
}
pub unsafe fn _mm_cmpnge_sd(a: __m128d, b: __m128d) -> __m128d {
    _mm_move_sd(a, _mm_cmpnge_pd(a, b))
}

// comisd / ucomisd as rustc's LLVM lowers them: eq, lt, le, gt, ge are 0 for unordered operands,
// neq is 1 (the two families differ only in the invalid-operation exception for quiet NaNs).
pub unsafe fn _mm_comieq_sd(a: __m128d, b: __m128d) -> i32 {
    (a.0[0] == b.0[0]) as i32
}
pub unsafe fn _mm_comilt_sd(a: __m128d, b: __m128d) -> i32 {
    (a.0[0] < b.0[0]) as i32
}
pub unsafe fn _mm_comile_sd(a: __m128d, b: __m128d) -> i32 {
    (a.0[0] <= b.0[0]) as i32
}
pub unsafe fn _mm_comigt_sd(a: __m128d, b: __m128d) -> i32 {
    (a.0[0] > b.0[0]) as i32
}
pub unsafe fn _mm_comige_sd(a: __m128d, b: __m128d) -> i32 {
    (a.0[0] >= b.0[0]) as i32
}
pub unsafe fn _mm_comineq_sd(a: __m128d, b: __m128d) -> i32 {
    (a.0[0] != b.0[0]) as i32
}
pub unsafe fn _mm_ucomieq_sd(a: __m128d, b: __m128d) -> i32 {
    _mm_comieq_sd(a, b)
}
pub unsafe fn _mm_ucomilt_sd(a: __m128d, b: __m128d) -> i32 {
    _mm_comilt_sd(a, b)
}
pub unsafe fn _mm_ucomile_sd(a: __m128d, b: __m128d) -> i32 {
    _mm_comile_sd(a, b)
}
pub unsafe fn _mm_ucomigt_sd(a: __m128d, b: __m128d) -> i32 {
    _mm_comigt_sd(a, b)
}
pub unsafe fn _mm_ucomige_sd(a: __m128d, b: __m128d) -> i32 {
    _mm_comige_sd(a, b)
}
pub unsafe fn _mm_ucomineq_sd(a: __m128d, b: __m128d) -> i32 {
    _mm_comineq_sd(a, b)
}

/// the sign bit of each f64 lane, lane i at bit i (movmskpd)
#[inline(always)]
pub unsafe fn _mm_movemask_pd(a: __m128d) -> i32 {
    vx::vx_mask(a, 64)
}

#[inline(always)]
pub unsafe fn _mm_unpacklo_pd(a: __m128d, b: __m128d) -> __m128d {
    vx::vx_binary(a, b, 17, 64)
}
#[inline(always)]
pub unsafe fn _mm_unpackhi_pd(a: __m128d, b: __m128d) -> __m128d {
    vx::vx_binary(a, b, 18, 64)
}
/// [a[MASK & 1], b[(MASK >> 1) & 1]]
#[cfg_attr(not(relax_check), rustc_legacy_const_generics(2))]
#[inline(always)]
pub unsafe fn _mm_shuffle_pd<const MASK: i32>(a: __m128d, b: __m128d) -> __m128d {
    shuffle_pd(a, b, MASK)
}
/// [a[k & 1], b[(k >> 1) & 1]] for a constant k
#[inline(always)]
unsafe fn shuffle_pd(a: __m128d, b: __m128d, k: i32) -> __m128d {
    // zip_lo picks lane 0 of each input, zip_hi lane 1; a lane picked from the other half of `a`
    // first becomes both lanes of `a`
    match k & 3 {
        0 => vx::vx_binary(a, b, 17, 64),
        1 => {
            let x: __m128d = vx::vx_binary(a, a, 18, 64);
            vx::vx_binary(x, b, 17, 64)
        }
        2 => {
            let x: __m128d = vx::vx_binary(a, a, 17, 64);
            vx::vx_binary(x, b, 18, 64)
        }
        _ => vx::vx_binary(a, b, 18, 64),
    }
}

// ---- set / load / store
#[inline(always)]
pub unsafe fn _mm_set1_pd(a: f64) -> __m128d {
    vx::vx_splat(a.to_bits(), 64)
}
#[inline(always)]
pub unsafe fn _mm_set_pd1(a: f64) -> __m128d {
    _mm_set1_pd(a)
}
/// [b, a] (the arguments high to low)
pub unsafe fn _mm_set_pd(a: f64, b: f64) -> __m128d {
    __m128d([b, a])
}
pub unsafe fn _mm_setr_pd(a: f64, b: f64) -> __m128d {
    __m128d([a, b])
}
pub unsafe fn _mm_set_sd(a: f64) -> __m128d {
    __m128d([a, 0.0])
}
#[inline(always)]
pub unsafe fn _mm_setzero_pd() -> __m128d {
    vx::vx_splat(0, 64)
}
/// stdarch returns zeros
pub unsafe fn _mm_undefined_pd() -> __m128d {
    _mm_setzero_pd()
}
pub unsafe fn _mm_undefined_si128() -> __m128i {
    __m128i([0, 0])
}
pub unsafe fn _mm_undefined_ps() -> __m128 {
    _mm_setzero_ps()
}

/// 16-byte alignment is the caller's contract (as on hardware); the loads are the same.
#[inline(always)]
pub unsafe fn _mm_load_pd(mem_addr: *const f64) -> __m128d {
    vx::vx_load(mem_addr as *const u8)
}
#[inline(always)]
pub unsafe fn _mm_loadu_pd(mem_addr: *const f64) -> __m128d {
    vx::vx_load(mem_addr as *const u8)
}
pub unsafe fn _mm_load_sd(mem_addr: *const f64) -> __m128d {
    __m128d([crate::ptr::read_unaligned(mem_addr), 0.0])
}
pub unsafe fn _mm_load1_pd(mem_addr: *const f64) -> __m128d {
    _mm_set1_pd(*mem_addr)
}
pub unsafe fn _mm_load_pd1(mem_addr: *const f64) -> __m128d {
    _mm_load1_pd(mem_addr)
}
/// [p[1], p[0]]
pub unsafe fn _mm_loadr_pd(mem_addr: *const f64) -> __m128d {
    let a = _mm_load_pd(mem_addr);
    _mm_shuffle_pd::<1>(a, a)
}
/// [a0, *p]
pub unsafe fn _mm_loadh_pd(a: __m128d, mem_addr: *const f64) -> __m128d {
    __m128d([a.0[0], crate::ptr::read_unaligned(mem_addr)])
}
/// [*p, a1]
pub unsafe fn _mm_loadl_pd(a: __m128d, mem_addr: *const f64) -> __m128d {
    __m128d([crate::ptr::read_unaligned(mem_addr), a.0[1]])
}

#[inline(always)]
pub unsafe fn _mm_store_pd(mem_addr: *mut f64, a: __m128d) {
    vx::vx_store(mem_addr as *mut u8, a)
}
#[inline(always)]
pub unsafe fn _mm_storeu_pd(mem_addr: *mut f64, a: __m128d) {
    vx::vx_store(mem_addr as *mut u8, a)
}
/// a non-temporal hint on hardware; the same store here
pub unsafe fn _mm_stream_pd(mem_addr: *mut f64, a: __m128d) {
    _mm_store_pd(mem_addr, a)
}
pub unsafe fn _mm_store_sd(mem_addr: *mut f64, a: __m128d) {
    crate::ptr::write_unaligned(mem_addr, a.0[0])
}
/// [a0, a0]
pub unsafe fn _mm_store1_pd(mem_addr: *mut f64, a: __m128d) {
    _mm_store_pd(mem_addr, _mm_unpacklo_pd(a, a))
}
pub unsafe fn _mm_store_pd1(mem_addr: *mut f64, a: __m128d) {
    _mm_store1_pd(mem_addr, a)
}
/// [a1, a0]
pub unsafe fn _mm_storer_pd(mem_addr: *mut f64, a: __m128d) {
    _mm_store_pd(mem_addr, _mm_shuffle_pd::<1>(a, a))
}
pub unsafe fn _mm_storeh_pd(mem_addr: *mut f64, a: __m128d) {
    crate::ptr::write_unaligned(mem_addr, a.0[1])
}
pub unsafe fn _mm_storel_pd(mem_addr: *mut f64, a: __m128d) {
    crate::ptr::write_unaligned(mem_addr, a.0[0])
}

// ---- casts (bit reinterpretations)
#[inline(always)]
pub unsafe fn _mm_castpd_ps(a: __m128d) -> __m128 {
    crate::mem::transmute::<__m128d, __m128>(a)
}
#[inline(always)]
pub unsafe fn _mm_castpd_si128(a: __m128d) -> __m128i {
    crate::mem::transmute::<__m128d, __m128i>(a)
}
#[inline(always)]
pub unsafe fn _mm_castps_pd(a: __m128) -> __m128d {
    crate::mem::transmute::<__m128, __m128d>(a)
}
#[inline(always)]
pub unsafe fn _mm_castps_si128(a: __m128) -> __m128i {
    crate::mem::transmute::<__m128, __m128i>(a)
}
#[inline(always)]
pub unsafe fn _mm_castsi128_pd(a: __m128i) -> __m128d {
    crate::mem::transmute::<__m128i, __m128d>(a)
}
#[inline(always)]
pub unsafe fn _mm_castsi128_ps(a: __m128i) -> __m128 {
    crate::mem::transmute::<__m128i, __m128>(a)
}

// ---- conversions
/// f64 rounded to an integral value by the MXCSR rounding mode `rm` (0 nearest-even, 1 down,
/// 2 up, 3 toward zero)
fn round_f64(x: f64, rm: u32) -> f64 {
    match rm {
        0 => x.round_ties_even(),
        1 => x.floor(),
        2 => x.ceil(),
        _ => x.trunc(),
    }
}
fn round_f32(x: f32, rm: u32) -> f32 {
    match rm {
        0 => x.round_ties_even(),
        1 => x.floor(),
        2 => x.ceil(),
        _ => x.trunc(),
    }
}
/// CVT(T)SD2SI / CVT(T)PD2DQ lane: the rounded value, or the integer indefinite i32::MIN
fn f64_to_i32(x: f64, rm: u32) -> i32 {
    let r = round_f64(x, rm);
    if r >= -2147483648.0 && r < 2147483648.0 { r as i32 } else { i32::MIN }
}
fn f64_to_i64(x: f64, rm: u32) -> i64 {
    let r = round_f64(x, rm);
    if r >= -9223372036854775808.0 && r < 9223372036854775808.0 { r as i64 } else { i64::MIN }
}
fn f32_to_i32(x: f32, rm: u32) -> i32 {
    let r = round_f32(x, rm);
    if r >= -2147483648.0 && r < 2147483648.0 { r as i32 } else { i32::MIN }
}
/// the MXCSR rounding mode (RC, bits 13..15)
unsafe fn mxcsr_rm() -> u32 {
    (_mm_getcsr() >> 13) & 3
}

/// [a0 as f64, a1 as f64] (exact)
pub unsafe fn _mm_cvtepi32_pd(a: __m128i) -> __m128d {
    let a = lanes128_i32(a);
    __m128d([a[0] as f64, a[1] as f64])
}
pub unsafe fn _mm_cvtepi32_ps(a: __m128i) -> __m128 {
    let a = lanes128_i32(a);
    __m128([a[0] as f32, a[1] as f32, a[2] as f32, a[3] as f32])
}
pub unsafe fn _mm_cvtps_epi32(a: __m128) -> __m128i {
    let rm = mxcsr_rm();
    from128_i32([f32_to_i32(a.0[0], rm), f32_to_i32(a.0[1], rm), f32_to_i32(a.0[2], rm), f32_to_i32(a.0[3], rm)])
}
pub unsafe fn _mm_cvttps_epi32(a: __m128) -> __m128i {
    from128_i32([f32_to_i32(a.0[0], 3), f32_to_i32(a.0[1], 3), f32_to_i32(a.0[2], 3), f32_to_i32(a.0[3], 3)])
}
/// [a0 as f32, a1 as f32, 0, 0]
pub unsafe fn _mm_cvtpd_ps(a: __m128d) -> __m128 {
    __m128([a.0[0] as f32, a.0[1] as f32, 0.0, 0.0])
}
/// [a0 as f64, a1 as f64] (exact)
pub unsafe fn _mm_cvtps_pd(a: __m128) -> __m128d {
    __m128d([a.0[0] as f64, a.0[1] as f64])
}
/// [a0, a1 as i32 by MXCSR, 0, 0]
pub unsafe fn _mm_cvtpd_epi32(a: __m128d) -> __m128i {
    let rm = mxcsr_rm();
    from128_i32([f64_to_i32(a.0[0], rm), f64_to_i32(a.0[1], rm), 0, 0])
}
pub unsafe fn _mm_cvttpd_epi32(a: __m128d) -> __m128i {
    from128_i32([f64_to_i32(a.0[0], 3), f64_to_i32(a.0[1], 3), 0, 0])
}
pub unsafe fn _mm_cvtsd_si32(a: __m128d) -> i32 {
    f64_to_i32(a.0[0], mxcsr_rm())
}
pub unsafe fn _mm_cvttsd_si32(a: __m128d) -> i32 {
    f64_to_i32(a.0[0], 3)
}
pub unsafe fn _mm_cvtsd_si64(a: __m128d) -> i64 {
    f64_to_i64(a.0[0], mxcsr_rm())
}
pub unsafe fn _mm_cvtsd_si64x(a: __m128d) -> i64 {
    _mm_cvtsd_si64(a)
}
pub unsafe fn _mm_cvttsd_si64(a: __m128d) -> i64 {
    f64_to_i64(a.0[0], 3)
}
pub unsafe fn _mm_cvttsd_si64x(a: __m128d) -> i64 {
    _mm_cvttsd_si64(a)
}
pub unsafe fn _mm_cvtsd_f64(a: __m128d) -> f64 {
    a.0[0]
}
/// [b0 as f32, a1, a2, a3]
pub unsafe fn _mm_cvtsd_ss(a: __m128, b: __m128d) -> __m128 {
    __m128([b.0[0] as f32, a.0[1], a.0[2], a.0[3]])
}
/// [b0 as f64, a1]
pub unsafe fn _mm_cvtss_sd(a: __m128d, b: __m128) -> __m128d {
    __m128d([b.0[0] as f64, a.0[1]])
}
/// [b as f64, a1]
pub unsafe fn _mm_cvtsi32_sd(a: __m128d, b: i32) -> __m128d {
    __m128d([b as f64, a.0[1]])
}
pub unsafe fn _mm_cvtsi64_sd(a: __m128d, b: i64) -> __m128d {
    __m128d([b as f64, a.0[1]])
}
pub unsafe fn _mm_cvtsi64x_sd(a: __m128d, b: i64) -> __m128d {
    _mm_cvtsi64_sd(a, b)
}

// ---- SSE3 / SSE4.1 f64 (stdarch x86/sse3.rs, x86/sse41.rs)
/// [a0 - b0, a1 + b1]
pub unsafe fn _mm_addsub_pd(a: __m128d, b: __m128d) -> __m128d {
    _mm_move_sd(_mm_add_pd(a, b), _mm_sub_pd(a, b))
}
/// [a0 + a1, b0 + b1]
pub unsafe fn _mm_hadd_pd(a: __m128d, b: __m128d) -> __m128d {
    _mm_add_pd(_mm_unpacklo_pd(a, b), _mm_unpackhi_pd(a, b))
}
/// [a0 - a1, b0 - b1]
pub unsafe fn _mm_hsub_pd(a: __m128d, b: __m128d) -> __m128d {
    _mm_sub_pd(_mm_unpacklo_pd(a, b), _mm_unpackhi_pd(a, b))
}
/// [a0, a0]
#[inline(always)]
pub unsafe fn _mm_movedup_pd(a: __m128d) -> __m128d {
    _mm_unpacklo_pd(a, a)
}
pub unsafe fn _mm_loaddup_pd(mem_addr: *const f64) -> __m128d {
    _mm_set1_pd(crate::ptr::read_unaligned(mem_addr))
}
/// lane i = bit i of IMM2 ? b[i] : a[i]
#[cfg_attr(not(relax_check), rustc_legacy_const_generics(2))]
#[inline(always)]
pub unsafe fn _mm_blend_pd<const IMM2: i32>(a: __m128d, b: __m128d) -> __m128d {
    blend_pd(a, b, IMM2)
}
/// lane i = bit i of k ? b[i] : a[i], for a constant k
#[inline(always)]
unsafe fn blend_pd(a: __m128d, b: __m128d, k: i32) -> __m128d {
    match k & 3 {
        0 => a,
        1 => _mm_move_sd(a, b),
        2 => _mm_move_sd(b, a),
        _ => b,
    }
}
/// lane i = sign bit of mask[i] ? b[i] : a[i]
#[inline(always)]
pub unsafe fn _mm_blendv_pd(a: __m128d, b: __m128d, mask: __m128d) -> __m128d {
    // the sign bit across the lane (arithmetic shift right by 63)
    let m: __m128d = vx::vx_unary(mask, 3, 64, 63);
    _mm_or_pd(_mm_and_pd(m, b), _mm_andnot_pd(m, a))
}
/// DPPD: the products of the lanes selected by IMM8 bits 4, 5 (others +0.0), summed lane 0 first,
/// stored to the lanes selected by bits 0, 1 (others +0.0)
#[cfg_attr(not(relax_check), rustc_legacy_const_generics(2))]
pub unsafe fn _mm_dp_pd<const IMM8: i32>(a: __m128d, b: __m128d) -> __m128d {
    let lane = |bit: i32| if IMM8 & bit != 0 { u64::MAX } else { 0 };
    let take: __m128d = crate::mem::transmute::<[u64; 2], __m128d>([lane(0x10), lane(0x20)]);
    let put: __m128d = crate::mem::transmute::<[u64; 2], __m128d>([lane(0x01), lane(0x02)]);
    let t = _mm_and_pd(_mm_mul_pd(a, b), take);
    let s = _mm_add_pd(t, _mm_shuffle_pd::<1>(t, t));
    _mm_and_pd(_mm_unpacklo_pd(s, s), put)
}
/// roundpd: ROUNDING & 3 (nearest-even, down, up, toward zero), or MXCSR's mode with bit 2
#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
#[inline(always)]
pub unsafe fn _mm_round_pd<const ROUNDING: i32>(a: __m128d) -> __m128d {
    if ROUNDING & 4 != 0 {
        match mxcsr_rm() {
            0 => vx::vx_unary(a, 11, 64, 0),
            1 => vx::vx_unary(a, 11, 64, 1),
            2 => vx::vx_unary(a, 11, 64, 2),
            _ => vx::vx_unary(a, 11, 64, 3),
        }
    } else {
        vx::vx_unary(a, 11, 64, ROUNDING & 3)
    }
}
#[inline(always)]
pub unsafe fn _mm_floor_pd(a: __m128d) -> __m128d {
    vx::vx_unary(a, 11, 64, 1)
}
#[inline(always)]
pub unsafe fn _mm_ceil_pd(a: __m128d) -> __m128d {
    vx::vx_unary(a, 11, 64, 2)
}
/// [round(b0), a1]
#[cfg_attr(not(relax_check), rustc_legacy_const_generics(2))]
pub unsafe fn _mm_round_sd<const ROUNDING: i32>(a: __m128d, b: __m128d) -> __m128d {
    _mm_move_sd(a, _mm_round_pd::<ROUNDING>(b))
}
pub unsafe fn _mm_floor_sd(a: __m128d, b: __m128d) -> __m128d {
    _mm_move_sd(a, _mm_floor_pd(b))
}
pub unsafe fn _mm_ceil_sd(a: __m128d, b: __m128d) -> __m128d {
    _mm_move_sd(a, _mm_ceil_pd(b))
}

// ---------------------------------------------------------------- AVX / AVX2 / FMA f64 (_mm256_*_pd, _mm_*_pd)
// stdarch x86/avx.rs, avx2.rs, fma.rs. A __m256d is two 128-bit halves (as the _ps forms above): each
// lane-wise op is the 128-bit SSE2 op on both halves, which pair256 turns back into one ymm instruction on
// AVX hosts (add/sub/mul/div/min/max/compares/logic). Lane order is memory order.

#[inline(always)]
unsafe fn pd_lo(a: &__m256d) -> __m128d {
    vx::vx_load(a as *const __m256d as *const u8)
}
#[inline(always)]
unsafe fn pd_hi(a: &__m256d) -> __m128d {
    vx::vx_load((a as *const __m256d as *const u8).add(16))
}
#[inline(always)]
unsafe fn pd_join(lo: __m128d, hi: __m128d) -> __m256d {
    crate::mem::transmute::<[__m128d; 2], __m256d>([lo, hi])
}

macro_rules! pd256_binary {
    ($($name:ident => $op:ident;)*) => {$(
        #[inline(always)]
        pub unsafe fn $name(a: __m256d, b: __m256d) -> __m256d {
            pd_join($op(pd_lo(&a), pd_lo(&b)), $op(pd_hi(&a), pd_hi(&b)))
        }
    )*};
}
pd256_binary! {
    _mm256_add_pd => _mm_add_pd;
    _mm256_sub_pd => _mm_sub_pd;
    _mm256_mul_pd => _mm_mul_pd;
    _mm256_div_pd => _mm_div_pd;
    _mm256_min_pd => _mm_min_pd;
    _mm256_max_pd => _mm_max_pd;
    _mm256_and_pd => _mm_and_pd;
    _mm256_andnot_pd => _mm_andnot_pd;
    _mm256_or_pd => _mm_or_pd;
    _mm256_xor_pd => _mm_xor_pd;
    _mm256_addsub_pd => _mm_addsub_pd;
    _mm256_hadd_pd => _mm_hadd_pd;
    _mm256_hsub_pd => _mm_hsub_pd;
    _mm256_unpackhi_pd => _mm_unpackhi_pd;
    _mm256_unpacklo_pd => _mm_unpacklo_pd;
}
#[inline(always)]
pub unsafe fn _mm256_sqrt_pd(a: __m256d) -> __m256d {
    pd_join(_mm_sqrt_pd(pd_lo(&a)), _mm_sqrt_pd(pd_hi(&a)))
}
/// [a0, a0, a2, a2]
pub unsafe fn _mm256_movedup_pd(a: __m256d) -> __m256d {
    pd_join(_mm_movedup_pd(pd_lo(&a)), _mm_movedup_pd(pd_hi(&a)))
}
#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
pub unsafe fn _mm256_round_pd<const ROUNDING: i32>(a: __m256d) -> __m256d {
    pd_join(_mm_round_pd::<ROUNDING>(pd_lo(&a)), _mm_round_pd::<ROUNDING>(pd_hi(&a)))
}
pub unsafe fn _mm256_floor_pd(a: __m256d) -> __m256d {
    pd_join(_mm_floor_pd(pd_lo(&a)), _mm_floor_pd(pd_hi(&a)))
}
pub unsafe fn _mm256_ceil_pd(a: __m256d) -> __m256d {
    pd_join(_mm_ceil_pd(pd_lo(&a)), _mm_ceil_pd(pd_hi(&a)))
}
/// per 128-bit half: bits 0, 1 of MASK for the low half, bits 2, 3 for the high half
#[cfg_attr(not(relax_check), rustc_legacy_const_generics(2))]
pub unsafe fn _mm256_shuffle_pd<const MASK: i32>(a: __m256d, b: __m256d) -> __m256d {
    pd_join(shuffle_pd(pd_lo(&a), pd_lo(&b), MASK), shuffle_pd(pd_hi(&a), pd_hi(&b), MASK >> 2))
}
#[cfg_attr(not(relax_check), rustc_legacy_const_generics(2))]
pub unsafe fn _mm256_blend_pd<const IMM4: i32>(a: __m256d, b: __m256d) -> __m256d {
    pd_join(blend_pd(pd_lo(&a), pd_lo(&b), IMM4), blend_pd(pd_hi(&a), pd_hi(&b), IMM4 >> 2))
}
pub unsafe fn _mm256_blendv_pd(a: __m256d, b: __m256d, c: __m256d) -> __m256d {
    pd_join(_mm_blendv_pd(pd_lo(&a), pd_lo(&b), pd_lo(&c)), _mm_blendv_pd(pd_hi(&a), pd_hi(&b), pd_hi(&c)))
}
/// lane i = a[bit i of IMM2] (vpermilpd)
#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
pub unsafe fn _mm_permute_pd<const IMM2: i32>(a: __m128d) -> __m128d {
    shuffle_pd(a, a, IMM2)
}
#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
pub unsafe fn _mm256_permute_pd<const IMM4: i32>(a: __m256d) -> __m256d {
    let (lo, hi) = (pd_lo(&a), pd_hi(&a));
    pd_join(shuffle_pd(lo, lo, IMM4), shuffle_pd(hi, hi, IMM4 >> 2))
}
/// lane i = the lane of its own 128-bit half named by bit 1 of b[i]
pub unsafe fn _mm_permutevar_pd(a: __m128d, b: __m128i) -> __m128d {
    __m128d([a.0[(b.0[0] >> 1 & 1) as usize], a.0[(b.0[1] >> 1 & 1) as usize]])
}
pub unsafe fn _mm256_permutevar_pd(a: __m256d, b: __m256i) -> __m256d {
    __m256d([a.0[(b.0[0] >> 1 & 1) as usize], a.0[(b.0[1] >> 1 & 1) as usize], a.0[2 + (b.0[2] >> 1 & 1) as usize], a.0[2 + (b.0[3] >> 1 & 1) as usize]])
}
/// each half from IMM8 (bits 0..2 low half, 4..6 high half): 0 a.lo, 1 a.hi, 2 b.lo, 3 b.hi; bit 3 / 7 zero
#[cfg_attr(not(relax_check), rustc_legacy_const_generics(2))]
pub unsafe fn _mm256_permute2f128_pd<const IMM8: i32>(a: __m256d, b: __m256d) -> __m256d {
    let pick = |c: i32| -> __m128d {
        if c & 8 != 0 {
            _mm_setzero_pd()
        } else {
            match c & 3 {
                0 => pd_lo(&a),
                1 => pd_hi(&a),
                2 => pd_lo(&b),
                _ => pd_hi(&b),
            }
        }
    };
    pd_join(pick(IMM8 & 15), pick((IMM8 >> 4) & 15))
}
/// lane i = a[(IMM8 >> 2i) & 3] (AVX2 vpermpd)
#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
pub unsafe fn _mm256_permute4x64_pd<const IMM8: i32>(a: __m256d) -> __m256d {
    let s = |i: i32| a.0[((IMM8 >> (2 * i)) & 3) as usize];
    __m256d([s(0), s(1), s(2), s(3)])
}

/// the 32 AVX compare predicates (IMM5 & 15; 16..31 differ only in signaling quiet NaNs)
#[cfg_attr(not(relax_check), rustc_legacy_const_generics(2))]
#[inline(always)]
pub unsafe fn _mm_cmp_pd<const IMM5: i32>(a: __m128d, b: __m128d) -> __m128d {
    match IMM5 & 15 {
        0 => _mm_cmpeq_pd(a, b),
        1 => _mm_cmplt_pd(a, b),
        2 => _mm_cmple_pd(a, b),
        3 => _mm_cmpunord_pd(a, b),
        4 => _mm_cmpneq_pd(a, b),
        5 => _mm_cmpnlt_pd(a, b),
        6 => _mm_cmpnle_pd(a, b),
        7 => _mm_cmpord_pd(a, b),
        // EQ_UQ: equal or unordered
        8 => _mm_or_pd(_mm_cmpeq_pd(a, b), _mm_cmpunord_pd(a, b)),
        9 => _mm_cmpnge_pd(a, b),
        10 => _mm_cmpngt_pd(a, b),
        11 => _mm_setzero_pd(),
        // NEQ_OQ: ordered and not equal
        12 => _mm_andnot_pd(_mm_cmpeq_pd(a, b), _mm_cmpord_pd(a, b)),
        13 => _mm_cmpge_pd(a, b),
        14 => _mm_cmpgt_pd(a, b),
        _ => pd_ones(),
    }
}
#[cfg_attr(not(relax_check), rustc_legacy_const_generics(2))]
pub unsafe fn _mm_cmp_sd<const IMM5: i32>(a: __m128d, b: __m128d) -> __m128d {
    _mm_move_sd(a, _mm_cmp_pd::<IMM5>(a, b))
}
#[cfg_attr(not(relax_check), rustc_legacy_const_generics(2))]
pub unsafe fn _mm256_cmp_pd<const IMM5: i32>(a: __m256d, b: __m256d) -> __m256d {
    pd_join(_mm_cmp_pd::<IMM5>(pd_lo(&a), pd_lo(&b)), _mm_cmp_pd::<IMM5>(pd_hi(&a), pd_hi(&b)))
}
pub unsafe fn _mm256_movemask_pd(a: __m256d) -> i32 {
    _mm_movemask_pd(pd_lo(&a)) | _mm_movemask_pd(pd_hi(&a)) << 2
}
// vtestpd on the sign bits: ZF = (a & b) signs all clear, CF = (!a & b) signs all clear
pub unsafe fn _mm_testz_pd(a: __m128d, b: __m128d) -> i32 {
    (_mm_movemask_pd(_mm_and_pd(a, b)) == 0) as i32
}
pub unsafe fn _mm_testc_pd(a: __m128d, b: __m128d) -> i32 {
    (_mm_movemask_pd(_mm_andnot_pd(a, b)) == 0) as i32
}
pub unsafe fn _mm_testnzc_pd(a: __m128d, b: __m128d) -> i32 {
    (_mm_testz_pd(a, b) == 0 && _mm_testc_pd(a, b) == 0) as i32
}
pub unsafe fn _mm256_testz_pd(a: __m256d, b: __m256d) -> i32 {
    (_mm256_movemask_pd(_mm256_and_pd(a, b)) == 0) as i32
}
pub unsafe fn _mm256_testc_pd(a: __m256d, b: __m256d) -> i32 {
    (_mm256_movemask_pd(_mm256_andnot_pd(a, b)) == 0) as i32
}
pub unsafe fn _mm256_testnzc_pd(a: __m256d, b: __m256d) -> i32 {
    (_mm256_testz_pd(a, b) == 0 && _mm256_testc_pd(a, b) == 0) as i32
}

// ---- set / load / store / broadcast
pub unsafe fn _mm256_set1_pd(a: f64) -> __m256d {
    let v = _mm_set1_pd(a);
    pd_join(v, v)
}
/// [d, c, b, a] (the arguments high to low)
pub unsafe fn _mm256_set_pd(a: f64, b: f64, c: f64, d: f64) -> __m256d {
    __m256d([d, c, b, a])
}
pub unsafe fn _mm256_setr_pd(a: f64, b: f64, c: f64, d: f64) -> __m256d {
    __m256d([a, b, c, d])
}
pub unsafe fn _mm256_setzero_pd() -> __m256d {
    let z = _mm_setzero_pd();
    pd_join(z, z)
}
/// stdarch returns zeros
pub unsafe fn _mm256_undefined_pd() -> __m256d {
    _mm256_setzero_pd()
}
pub unsafe fn _mm256_loadu_pd(mem_addr: *const f64) -> __m256d {
    pd_join(vx::vx_load(mem_addr as *const u8), vx::vx_load((mem_addr as *const u8).add(16)))
}
/// 32-byte alignment is the caller's contract (as on hardware); the loads are the same.
pub unsafe fn _mm256_load_pd(mem_addr: *const f64) -> __m256d {
    _mm256_loadu_pd(mem_addr)
}
pub unsafe fn _mm256_storeu_pd(mem_addr: *mut f64, a: __m256d) {
    vx::vx_store(mem_addr as *mut u8, pd_lo(&a));
    vx::vx_store((mem_addr as *mut u8).add(16), pd_hi(&a));
}
pub unsafe fn _mm256_store_pd(mem_addr: *mut f64, a: __m256d) {
    _mm256_storeu_pd(mem_addr, a)
}
pub unsafe fn _mm256_stream_pd(mem_addr: *mut f64, a: __m256d) {
    _mm256_storeu_pd(mem_addr, a)
}
/// lanes whose mask sign bit is set are read, the others are 0.0 and not touched in memory
pub unsafe fn _mm_maskload_pd(mem_addr: *const f64, mask: __m128i) -> __m128d {
    let l = |i: usize| if mask.0[i] < 0 { crate::ptr::read_unaligned(mem_addr.add(i)) } else { 0.0 };
    __m128d([l(0), l(1)])
}
pub unsafe fn _mm256_maskload_pd(mem_addr: *const f64, mask: __m256i) -> __m256d {
    let l = |i: usize| if mask.0[i] < 0 { crate::ptr::read_unaligned(mem_addr.add(i)) } else { 0.0 };
    __m256d([l(0), l(1), l(2), l(3)])
}
pub unsafe fn _mm_maskstore_pd(mem_addr: *mut f64, mask: __m128i, a: __m128d) {
    let mut i = 0;
    while i < 2 {
        if mask.0[i] < 0 {
            crate::ptr::write_unaligned(mem_addr.add(i), a.0[i]);
        }
        i += 1;
    }
}
pub unsafe fn _mm256_maskstore_pd(mem_addr: *mut f64, mask: __m256i, a: __m256d) {
    let mut i = 0;
    while i < 4 {
        if mask.0[i] < 0 {
            crate::ptr::write_unaligned(mem_addr.add(i), a.0[i]);
        }
        i += 1;
    }
}
pub unsafe fn _mm256_broadcast_sd(f: &f64) -> __m256d {
    _mm256_set1_pd(*f)
}
pub unsafe fn _mm256_broadcast_pd(a: &__m128d) -> __m256d {
    pd_join(*a, *a)
}
/// AVX2: lane 0 to every lane
pub unsafe fn _mm256_broadcastsd_pd(a: __m128d) -> __m256d {
    let v = _mm_movedup_pd(a);
    pd_join(v, v)
}
pub unsafe fn _mm_broadcastsd_pd(a: __m128d) -> __m128d {
    _mm_movedup_pd(a)
}

// ---- casts, 128 <-> 256
#[inline(always)]
pub unsafe fn _mm256_castpd_ps(a: __m256d) -> __m256 {
    crate::mem::transmute::<__m256d, __m256>(a)
}
#[inline(always)]
pub unsafe fn _mm256_castps_pd(a: __m256) -> __m256d {
    crate::mem::transmute::<__m256, __m256d>(a)
}
#[inline(always)]
pub unsafe fn _mm256_castpd_si256(a: __m256d) -> __m256i {
    crate::mem::transmute::<__m256d, __m256i>(a)
}
#[inline(always)]
pub unsafe fn _mm256_castsi256_pd(a: __m256i) -> __m256d {
    crate::mem::transmute::<__m256i, __m256d>(a)
}
#[inline(always)]
pub unsafe fn _mm256_castps_si256(a: __m256) -> __m256i {
    crate::mem::transmute::<__m256, __m256i>(a)
}
#[inline(always)]
pub unsafe fn _mm256_castsi256_ps(a: __m256i) -> __m256 {
    crate::mem::transmute::<__m256i, __m256>(a)
}
/// [e0, e1, e2, e3] (low to high)
pub unsafe fn _mm_setr_epi32(e0: i32, e1: i32, e2: i32, e3: i32) -> __m128i {
    from128_i32([e0, e1, e2, e3])
}
/// the upper half is stdarch's _mm_undefined_pd: zeros
pub unsafe fn _mm256_castpd128_pd256(a: __m128d) -> __m256d {
    pd_join(a, _mm_setzero_pd())
}
pub unsafe fn _mm256_zextpd128_pd256(a: __m128d) -> __m256d {
    pd_join(a, _mm_setzero_pd())
}
pub unsafe fn _mm256_castpd256_pd128(a: __m256d) -> __m128d {
    pd_lo(&a)
}
#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
pub unsafe fn _mm256_extractf128_pd<const IMM1: i32>(a: __m256d) -> __m128d {
    if IMM1 & 1 == 0 { pd_lo(&a) } else { pd_hi(&a) }
}
#[cfg_attr(not(relax_check), rustc_legacy_const_generics(2))]
pub unsafe fn _mm256_insertf128_pd<const IMM1: i32>(a: __m256d, b: __m128d) -> __m256d {
    if IMM1 & 1 == 0 { pd_join(b, pd_hi(&a)) } else { pd_join(pd_lo(&a), b) }
}

// ---- conversions (rules as the 128-bit forms)
pub unsafe fn _mm256_cvtepi32_pd(a: __m128i) -> __m256d {
    let a = lanes128_i32(a);
    __m256d([a[0] as f64, a[1] as f64, a[2] as f64, a[3] as f64])
}
pub unsafe fn _mm256_cvtpd_epi32(a: __m256d) -> __m128i {
    let rm = mxcsr_rm();
    from128_i32([f64_to_i32(a.0[0], rm), f64_to_i32(a.0[1], rm), f64_to_i32(a.0[2], rm), f64_to_i32(a.0[3], rm)])
}
pub unsafe fn _mm256_cvttpd_epi32(a: __m256d) -> __m128i {
    from128_i32([f64_to_i32(a.0[0], 3), f64_to_i32(a.0[1], 3), f64_to_i32(a.0[2], 3), f64_to_i32(a.0[3], 3)])
}
pub unsafe fn _mm256_cvtpd_ps(a: __m256d) -> __m128 {
    __m128([a.0[0] as f32, a.0[1] as f32, a.0[2] as f32, a.0[3] as f32])
}
pub unsafe fn _mm256_cvtps_pd(a: __m128) -> __m256d {
    __m256d([a.0[0] as f64, a.0[1] as f64, a.0[2] as f64, a.0[3] as f64])
}

// ---- AVX2 gathers: lane i from slice + offsets[i] * SCALE bytes; the mask forms read lanes whose mask
// sign bit is set and keep src elsewhere (no access for those lanes)
#[inline(always)]
unsafe fn gather_f64(slice: *const f64, off: i64, scale: i32) -> f64 {
    crate::ptr::read_unaligned((slice as *const u8).offset((off * scale as i64) as isize) as *const f64)
}
#[cfg_attr(not(relax_check), rustc_legacy_const_generics(2))]
pub unsafe fn _mm_i32gather_pd<const SCALE: i32>(slice: *const f64, offsets: __m128i) -> __m128d {
    let o = lanes128_i32(offsets);
    __m128d([gather_f64(slice, o[0] as i64, SCALE), gather_f64(slice, o[1] as i64, SCALE)])
}
#[cfg_attr(not(relax_check), rustc_legacy_const_generics(4))]
pub unsafe fn _mm_mask_i32gather_pd<const SCALE: i32>(src: __m128d, slice: *const f64, offsets: __m128i, mask: __m128d) -> __m128d {
    let o = lanes128_i32(offsets);
    let l = |i: usize| if mask.0[i].is_sign_negative() { gather_f64(slice, o[i] as i64, SCALE) } else { src.0[i] };
    __m128d([l(0), l(1)])
}
#[cfg_attr(not(relax_check), rustc_legacy_const_generics(2))]
pub unsafe fn _mm256_i32gather_pd<const SCALE: i32>(slice: *const f64, offsets: __m128i) -> __m256d {
    let o = lanes128_i32(offsets);
    let l = |i: usize| gather_f64(slice, o[i] as i64, SCALE);
    __m256d([l(0), l(1), l(2), l(3)])
}
#[cfg_attr(not(relax_check), rustc_legacy_const_generics(4))]
pub unsafe fn _mm256_mask_i32gather_pd<const SCALE: i32>(src: __m256d, slice: *const f64, offsets: __m128i, mask: __m256d) -> __m256d {
    let o = lanes128_i32(offsets);
    let l = |i: usize| if mask.0[i].is_sign_negative() { gather_f64(slice, o[i] as i64, SCALE) } else { src.0[i] };
    __m256d([l(0), l(1), l(2), l(3)])
}
#[cfg_attr(not(relax_check), rustc_legacy_const_generics(2))]
pub unsafe fn _mm_i64gather_pd<const SCALE: i32>(slice: *const f64, offsets: __m128i) -> __m128d {
    __m128d([gather_f64(slice, offsets.0[0], SCALE), gather_f64(slice, offsets.0[1], SCALE)])
}
#[cfg_attr(not(relax_check), rustc_legacy_const_generics(4))]
pub unsafe fn _mm_mask_i64gather_pd<const SCALE: i32>(src: __m128d, slice: *const f64, offsets: __m128i, mask: __m128d) -> __m128d {
    let l = |i: usize| if mask.0[i].is_sign_negative() { gather_f64(slice, offsets.0[i], SCALE) } else { src.0[i] };
    __m128d([l(0), l(1)])
}
#[cfg_attr(not(relax_check), rustc_legacy_const_generics(2))]
pub unsafe fn _mm256_i64gather_pd<const SCALE: i32>(slice: *const f64, offsets: __m256i) -> __m256d {
    let l = |i: usize| gather_f64(slice, offsets.0[i], SCALE);
    __m256d([l(0), l(1), l(2), l(3)])
}
#[cfg_attr(not(relax_check), rustc_legacy_const_generics(4))]
pub unsafe fn _mm256_mask_i64gather_pd<const SCALE: i32>(src: __m256d, slice: *const f64, offsets: __m256i, mask: __m256d) -> __m256d {
    let l = |i: usize| if mask.0[i].is_sign_negative() { gather_f64(slice, offsets.0[i], SCALE) } else { src.0[i] };
    __m256d([l(0), l(1), l(2), l(3)])
}

// ---- FMA f64: one rounding per lane, (neg_prod ? -(a*b) : a*b) + (neg_add ? -c : c), with the x86 NaN rule:
// the first NaN operand of a, b, c quieted (its sign as given: the negations do not apply to NaNs), else
// the negative default NaN for an invalid lane (inf * 0, inf - inf). Per lane (no f64 vector fma op yet).
#[inline(always)]
fn fma_f64(a: f64, b: f64, c: f64, neg_prod: bool, neg_add: bool) -> f64 {
    const QUIET: u64 = 1 << 51;
    if a.is_nan() {
        return f64::from_bits(a.to_bits() | QUIET);
    }
    if b.is_nan() {
        return f64::from_bits(b.to_bits() | QUIET);
    }
    if c.is_nan() {
        return f64::from_bits(c.to_bits() | QUIET);
    }
    let r = (if neg_prod { -a } else { a }).mul_add(b, if neg_add { -c } else { c });
    if r.is_nan() { f64::from_bits(0xfff8_0000_0000_0000) } else { r }
}
#[inline(always)]
unsafe fn fma_pd(a: __m128d, b: __m128d, c: __m128d, neg_prod: bool, neg_add: [bool; 2]) -> __m128d {
    __m128d([fma_f64(a.0[0], b.0[0], c.0[0], neg_prod, neg_add[0]), fma_f64(a.0[1], b.0[1], c.0[1], neg_prod, neg_add[1])])
}
#[inline(always)]
unsafe fn fma_pd256(a: __m256d, b: __m256d, c: __m256d, neg_prod: bool, neg_add: [bool; 2]) -> __m256d {
    pd_join(fma_pd(pd_lo(&a), pd_lo(&b), pd_lo(&c), neg_prod, neg_add), fma_pd(pd_hi(&a), pd_hi(&b), pd_hi(&c), neg_prod, neg_add))
}
pub unsafe fn _mm_fmadd_pd(a: __m128d, b: __m128d, c: __m128d) -> __m128d { fma_pd(a, b, c, false, [false; 2]) }
pub unsafe fn _mm_fmsub_pd(a: __m128d, b: __m128d, c: __m128d) -> __m128d { fma_pd(a, b, c, false, [true; 2]) }
pub unsafe fn _mm_fnmadd_pd(a: __m128d, b: __m128d, c: __m128d) -> __m128d { fma_pd(a, b, c, true, [false; 2]) }
pub unsafe fn _mm_fnmsub_pd(a: __m128d, b: __m128d, c: __m128d) -> __m128d { fma_pd(a, b, c, true, [true; 2]) }
/// even lanes a*b - c, odd lanes a*b + c
pub unsafe fn _mm_fmaddsub_pd(a: __m128d, b: __m128d, c: __m128d) -> __m128d { fma_pd(a, b, c, false, [true, false]) }
/// even lanes a*b + c, odd lanes a*b - c
pub unsafe fn _mm_fmsubadd_pd(a: __m128d, b: __m128d, c: __m128d) -> __m128d { fma_pd(a, b, c, false, [false, true]) }
pub unsafe fn _mm_fmadd_sd(a: __m128d, b: __m128d, c: __m128d) -> __m128d { _mm_move_sd(a, _mm_fmadd_pd(a, b, c)) }
pub unsafe fn _mm_fmsub_sd(a: __m128d, b: __m128d, c: __m128d) -> __m128d { _mm_move_sd(a, _mm_fmsub_pd(a, b, c)) }
pub unsafe fn _mm_fnmadd_sd(a: __m128d, b: __m128d, c: __m128d) -> __m128d { _mm_move_sd(a, _mm_fnmadd_pd(a, b, c)) }
pub unsafe fn _mm_fnmsub_sd(a: __m128d, b: __m128d, c: __m128d) -> __m128d { _mm_move_sd(a, _mm_fnmsub_pd(a, b, c)) }
pub unsafe fn _mm256_fmadd_pd(a: __m256d, b: __m256d, c: __m256d) -> __m256d { fma_pd256(a, b, c, false, [false; 2]) }
pub unsafe fn _mm256_fmsub_pd(a: __m256d, b: __m256d, c: __m256d) -> __m256d { fma_pd256(a, b, c, false, [true; 2]) }
pub unsafe fn _mm256_fnmadd_pd(a: __m256d, b: __m256d, c: __m256d) -> __m256d { fma_pd256(a, b, c, true, [false; 2]) }
pub unsafe fn _mm256_fnmsub_pd(a: __m256d, b: __m256d, c: __m256d) -> __m256d { fma_pd256(a, b, c, true, [true; 2]) }
pub unsafe fn _mm256_fmaddsub_pd(a: __m256d, b: __m256d, c: __m256d) -> __m256d { fma_pd256(a, b, c, false, [true, false]) }
pub unsafe fn _mm256_fmsubadd_pd(a: __m256d, b: __m256d, c: __m256d) -> __m256d { fma_pd256(a, b, c, false, [false, true]) }

// a vector prints its lanes as a tuple, as stdarch's Debug impls (`uint8x16_t(3, 3, ..)`)
macro_rules! simd_debug {
    ($($t:ident)*) => {$(
        impl crate::fmt::Debug for $t {
            fn fmt(&self, f: &mut crate::fmt::Formatter<'_>) -> crate::fmt::Result {
                let mut d = f.debug_tuple(stringify!($t));
                for x in self.0.iter() {
                    d.field(x);
                }
                d.finish()
            }
        }
    )*};
}
simd_debug!(__m128i __m128 __m128d __m256 __m256d __m512 __m512d __m256i __m512i __m128bh __m256bh __m512bh);

// ---------------------------------------------------------------- MXCSR, f16 conversions (F16C), AVX2 gaps

pub const _MM_FLUSH_ZERO_MASK: u32 = 0x8000;
pub const _MM_FLUSH_ZERO_ON: u32 = 0x8000;
pub const _MM_FLUSH_ZERO_OFF: u32 = 0x0000;
pub const _MM_ROUND_MASK: u32 = 0x6000;
pub const _MM_ROUND_NEAREST: u32 = 0x0000;
pub const _MM_ROUND_DOWN: u32 = 0x2000;
pub const _MM_ROUND_UP: u32 = 0x4000;
pub const _MM_ROUND_TOWARD_ZERO: u32 = 0x6000;
pub const _MM_FROUND_TO_NEAREST_INT: i32 = 0x00;
pub const _MM_FROUND_TO_NEG_INF: i32 = 0x01;
pub const _MM_FROUND_TO_POS_INF: i32 = 0x02;
pub const _MM_FROUND_TO_ZERO: i32 = 0x03;
pub const _MM_FROUND_CUR_DIRECTION: i32 = 0x04;
pub const _MM_FROUND_RAISE_EXC: i32 = 0x00;
pub const _MM_FROUND_NO_EXC: i32 = 0x08;
pub const _CMP_EQ_OQ: i32 = 0x00;
pub const _CMP_LT_OS: i32 = 0x01;
pub const _CMP_LE_OS: i32 = 0x02;
pub const _CMP_UNORD_Q: i32 = 0x03;
pub const _CMP_NEQ_UQ: i32 = 0x04;
pub const _CMP_NLT_US: i32 = 0x05;
pub const _CMP_NLE_US: i32 = 0x06;
pub const _CMP_ORD_Q: i32 = 0x07;
pub const _CMP_EQ_UQ: i32 = 0x08;
pub const _CMP_NGE_US: i32 = 0x09;
pub const _CMP_NGT_US: i32 = 0x0a;
pub const _CMP_FALSE_OQ: i32 = 0x0b;
pub const _CMP_NEQ_OQ: i32 = 0x0c;
pub const _CMP_GE_OS: i32 = 0x0d;
pub const _CMP_GT_OS: i32 = 0x0e;
pub const _CMP_TRUE_UQ: i32 = 0x0f;
pub const _CMP_EQ_OS: i32 = 0x10;
pub const _CMP_LT_OQ: i32 = 0x11;
pub const _CMP_LE_OQ: i32 = 0x12;
pub const _CMP_UNORD_S: i32 = 0x13;
pub const _CMP_NEQ_US: i32 = 0x14;
pub const _CMP_NLT_UQ: i32 = 0x15;
pub const _CMP_NLE_UQ: i32 = 0x16;
pub const _CMP_ORD_S: i32 = 0x17;
pub const _CMP_EQ_US: i32 = 0x18;
pub const _CMP_NGE_UQ: i32 = 0x19;
pub const _CMP_NGT_UQ: i32 = 0x1a;
pub const _CMP_FALSE_OS: i32 = 0x1b;
pub const _CMP_NEQ_OS: i32 = 0x1c;
pub const _CMP_GE_OQ: i32 = 0x1d;
pub const _CMP_GT_OQ: i32 = 0x1e;
pub const _CMP_TRUE_US: i32 = 0x1f;

/// This thread's MXCSR (stmxcsr): rounding, flush-to-zero, denormals-are-zero, the exception
/// masks and flags.
pub unsafe fn _mm_getcsr() -> u32 {
    crate::intrinsics_rt::fp_env_get() as u32
}

/// Sets this thread's MXCSR (ldmxcsr); the scalar and vector float code reads it as rustc's.
pub unsafe fn _mm_setcsr(a: u32) {
    crate::intrinsics_rt::fp_env_set(a as u64)
}

/// Rounding mode 0..3 (nearest-even, down, up, toward zero) of an `_MM_FROUND_*` immediate.
unsafe fn round_mode(imm: i32) -> u32 {
    if imm & 4 != 0 {
        (_mm_getcsr() >> 13) & 3
    } else {
        (imm & 3) as u32
    }
}

/// f32 -> IEEE binary16 bits, rounded by `rm` (0 nearest-even, 1 down, 2 up, 3 toward zero),
/// as VCVTPS2PH does (NaNs stay quiet NaNs keeping their top payload bits).
fn f32_to_f16_bits(x: f32, rm: u32) -> u16 {
    let bits = x.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let neg = sign != 0;
    let exp = ((bits >> 23) & 0xff) as i32;
    let man = bits & 0x7f_ffff;
    if exp == 0xff {
        if man != 0 {
            return sign | 0x7e00 | ((man >> 13) as u16 & 0x3ff);
        }
        return sign | 0x7c00;
    }
    if exp == 0 && man == 0 {
        return sign;
    }
    // value = m * 2^(e - 15 - 10) once shifted into a 10-bit mantissa field
    let (m, e) = if exp == 0 { (man, 1 - 127 + 15) } else { (man | 0x80_0000, exp - 127 + 15) };
    // overflow to infinity or the largest finite value, by rounding direction
    let overflow = |neg: bool| -> u16 {
        let inf = match rm {
            0 => true,
            1 => neg,
            2 => !neg,
            _ => false,
        };
        if inf { 0x7c00 } else { 0x7bff }
    };
    if e >= 31 {
        return sign | overflow(neg);
    }
    let shift: u32 = if e >= 1 { 13 } else { (13 + (1 - e)) as u32 };
    let (mut q, rem, half) = if shift >= 32 {
        (0u32, m, 0u32)
    } else {
        (m >> shift, m & ((1u32 << shift) - 1), 1u32 << (shift - 1))
    };
    let inc = match rm {
        0 => if shift >= 32 { false } else { rem > half || (rem == half && q & 1 == 1) },
        1 => neg && rem != 0,
        2 => !neg && rem != 0,
        _ => false,
    };
    if inc {
        q += 1;
    }
    let h = if e >= 1 { (((e - 1) as u32) << 10) + q } else { q };
    if h >= 0x7c00 {
        return sign | overflow(neg);
    }
    sign | h as u16
}

/// IEEE binary16 bits -> f32 (exact).
fn f16_bits_to_f32(h: u16) -> f32 {
    let sign = ((h & 0x8000) as u32) << 16;
    let exp = ((h >> 10) & 0x1f) as u32;
    let man = (h & 0x3ff) as u32;
    let bits = if exp == 0x1f {
        sign | 0x7f80_0000 | (man << 13)
    } else if exp == 0 {
        if man == 0 {
            sign
        } else {
            // subnormal: normalise
            let mut e: i32 = -14;
            let mut m = man;
            while m & 0x400 == 0 {
                m <<= 1;
                e -= 1;
            }
            sign | (((e + 127) as u32) << 23) | ((m & 0x3ff) << 13)
        }
    } else {
        sign | ((exp + 127 - 15) << 23) | (man << 13)
    };
    f32::from_bits(bits)
}

pub unsafe fn _mm_cvtph_ps(a: __m128i) -> __m128 {
    let h = lanes128_u16(a);
    __m128([f16_bits_to_f32(h[0]), f16_bits_to_f32(h[1]), f16_bits_to_f32(h[2]), f16_bits_to_f32(h[3])])
}

pub unsafe fn _mm256_cvtph_ps(a: __m128i) -> __m256 {
    let h = lanes128_u16(a);
    let mut r = [0f32; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = f16_bits_to_f32(h[i]);
        i += 1;
    }
    __m256(r)
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
pub unsafe fn _mm_cvtps_ph<const IMM_ROUNDING: i32>(a: __m128) -> __m128i {
    let rm = round_mode(IMM_ROUNDING);
    let mut r = [0u16; 8];
    let mut i = 0;
    while i < 4 {
        r[i] = f32_to_f16_bits(a.0[i], rm);
        i += 1;
    }
    from128_u16(r)
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
pub unsafe fn _mm256_cvtps_ph<const IMM_ROUNDING: i32>(a: __m256) -> __m128i {
    let rm = round_mode(IMM_ROUNDING);
    let mut r = [0u16; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = f32_to_f16_bits(a.0[i], rm);
        i += 1;
    }
    from128_u16(r)
}

pub unsafe fn _mm_mulhi_epu16(a: __m128i, b: __m128i) -> __m128i {
    let a = lanes128_u16(a);
    let b = lanes128_u16(b);
    let mut r = [0u16; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = ((a[i] as u32 * b[i] as u32) >> 16) as u16;
        i += 1;
    }
    from128_u16(r)
}

pub unsafe fn _mm256_set_epi16(
    e00: i16,
    e01: i16,
    e02: i16,
    e03: i16,
    e04: i16,
    e05: i16,
    e06: i16,
    e07: i16,
    e08: i16,
    e09: i16,
    e10: i16,
    e11: i16,
    e12: i16,
    e13: i16,
    e14: i16,
    e15: i16,
) -> __m256i {
    from256_i16([e15, e14, e13, e12, e11, e10, e09, e08, e07, e06, e05, e04, e03, e02, e01, e00])
}

/// `a` negated where `b` is negative, zeroed where `b` is zero (VPSIGNW, wrapping).
pub unsafe fn _mm256_sign_epi16(a: __m256i, b: __m256i) -> __m256i {
    let a = lanes256_i16(a);
    let b = lanes256_i16(b);
    let mut r = [0i16; 16];
    let mut i = 0;
    while i < 16 {
        r[i] = if b[i] < 0 {
            a[i].wrapping_neg()
        } else if b[i] == 0 {
            0
        } else {
            a[i]
        };
        i += 1;
    }
    from256_i16(r)
}

#[inline(always)]
pub unsafe fn _mm_add_epi64(a: __m128i, b: __m128i) -> __m128i {
    vx::vx_binary(a, b, 0, 64)
}

#[inline(always)]
pub unsafe fn _mm_sub_epi32(a: __m128i, b: __m128i) -> __m128i {
    vx::vx_binary(a, b, 1, 32)
}

#[inline(always)]
pub unsafe fn _mm_sub_epi64(a: __m128i, b: __m128i) -> __m128i {
    vx::vx_binary(a, b, 1, 64)
}

#[inline(always)]
pub unsafe fn _mm_adds_epi8(a: __m128i, b: __m128i) -> __m128i {
    vx::vx_binary(a, b, 2, 8)
}

#[inline(always)]
pub unsafe fn _mm_adds_epi16(a: __m128i, b: __m128i) -> __m128i {
    vx::vx_binary(a, b, 2, 16)
}

#[inline(always)]
pub unsafe fn _mm_adds_epu8(a: __m128i, b: __m128i) -> __m128i {
    vx::vx_binary(a, b, 3, 8)
}

#[inline(always)]
pub unsafe fn _mm_adds_epu16(a: __m128i, b: __m128i) -> __m128i {
    vx::vx_binary(a, b, 3, 16)
}

#[inline(always)]
pub unsafe fn _mm_subs_epi8(a: __m128i, b: __m128i) -> __m128i {
    vx::vx_binary(a, b, 4, 8)
}

#[inline(always)]
pub unsafe fn _mm_subs_epi16(a: __m128i, b: __m128i) -> __m128i {
    vx::vx_binary(a, b, 4, 16)
}

#[inline(always)]
pub unsafe fn _mm_subs_epu8(a: __m128i, b: __m128i) -> __m128i {
    vx::vx_binary(a, b, 5, 8)
}

#[inline(always)]
pub unsafe fn _mm_subs_epu16(a: __m128i, b: __m128i) -> __m128i {
    vx::vx_binary(a, b, 5, 16)
}

#[inline(always)]
pub unsafe fn _mm_min_epi8(a: __m128i, b: __m128i) -> __m128i {
    vx::vx_binary(a, b, 12, 8)
}

#[inline(always)]
pub unsafe fn _mm_min_epi32(a: __m128i, b: __m128i) -> __m128i {
    vx::vx_binary(a, b, 12, 32)
}

#[inline(always)]
pub unsafe fn _mm_min_epu8(a: __m128i, b: __m128i) -> __m128i {
    vx::vx_binary(a, b, 13, 8)
}

#[inline(always)]
pub unsafe fn _mm_min_epu16(a: __m128i, b: __m128i) -> __m128i {
    vx::vx_binary(a, b, 13, 16)
}

#[inline(always)]
pub unsafe fn _mm_min_epu32(a: __m128i, b: __m128i) -> __m128i {
    vx::vx_binary(a, b, 13, 32)
}

#[inline(always)]
pub unsafe fn _mm_max_epi8(a: __m128i, b: __m128i) -> __m128i {
    vx::vx_binary(a, b, 14, 8)
}

#[inline(always)]
pub unsafe fn _mm_max_epi16(a: __m128i, b: __m128i) -> __m128i {
    vx::vx_binary(a, b, 14, 16)
}

#[inline(always)]
pub unsafe fn _mm_max_epi32(a: __m128i, b: __m128i) -> __m128i {
    vx::vx_binary(a, b, 14, 32)
}

#[inline(always)]
pub unsafe fn _mm_max_epu8(a: __m128i, b: __m128i) -> __m128i {
    vx::vx_binary(a, b, 15, 8)
}

#[inline(always)]
pub unsafe fn _mm_max_epu16(a: __m128i, b: __m128i) -> __m128i {
    vx::vx_binary(a, b, 15, 16)
}

#[inline(always)]
pub unsafe fn _mm_max_epu32(a: __m128i, b: __m128i) -> __m128i {
    vx::vx_binary(a, b, 15, 32)
}

#[inline(always)]
pub unsafe fn _mm_avg_epu16(a: __m128i, b: __m128i) -> __m128i {
    vx::vx_binary(a, b, 16, 16)
}

#[inline(always)]
pub unsafe fn _mm_abs_epi8(a: __m128i) -> __m128i {
    vx::vx_unary(a, 0, 8, 0)
}

#[inline(always)]
pub unsafe fn _mm_abs_epi32(a: __m128i) -> __m128i {
    vx::vx_unary(a, 0, 32, 0)
}

#[inline(always)]
pub unsafe fn _mm_set1_epi32(a: i32) -> __m128i {
    vx::vx_splat(a as u64, 32)
}

#[inline(always)]
pub unsafe fn _mm_set1_epi64x(a: i64) -> __m128i {
    vx::vx_splat(a as u64, 64)
}

#[inline(always)]
pub unsafe fn _mm_packs_epi16(a: __m128i, b: __m128i) -> __m128i {
    vx::vx_binary(a, b, 20, 16)
}

#[inline(always)]
pub unsafe fn _mm_packs_epi32(a: __m128i, b: __m128i) -> __m128i {
    vx::vx_binary(a, b, 21, 32)
}

#[inline(always)]
pub unsafe fn _mm_packus_epi32(a: __m128i, b: __m128i) -> __m128i {
    vx::vx_binary(a, b, 22, 32)
}

#[inline(always)]
pub unsafe fn _mm_srai_epi16<const IMM8: i32>(a: __m128i) -> __m128i {
    vx::vx_unary(a, 3, 16, IMM8)
}

#[inline(always)]
pub unsafe fn _mm_cmpneq_ps(a: __m128, b: __m128) -> __m128 {
    vx::vx_binary(a, b, 47, 32)
}

// SSE has no arithmetic qword shift; the backend synthesizes its sign extension.
#[inline(always)]
pub unsafe fn _mm_srai_epi64<const IMM8: i32>(a: __m128i) -> __m128i {
    vx::vx_unary(a, 3, 64, IMM8)
}

#[inline(always)]
pub unsafe fn _mm_blendv_ps(a: __m128, b: __m128, mask: __m128) -> __m128 {
    let m: __m128 = vx::vx_unary(mask, 3, 32, 31);
    _mm_or_ps(_mm_and_ps(m, b), _mm_andnot_ps(m, a))
}

#[inline(always)]
pub unsafe fn _mm_cvtepu8_epi16(a: __m128i) -> __m128i {
    _mm_unpacklo_epi8(a, _mm_setzero_si128())
}

pub unsafe fn _mm_mullo_epi32(a: __m128i, b: __m128i) -> __m128i {
    let a = lanes128_i32(a);
    let b = lanes128_i32(b);
    from128_i32([
        a[0].wrapping_mul(b[0]),
        a[1].wrapping_mul(b[1]),
        a[2].wrapping_mul(b[2]),
        a[3].wrapping_mul(b[3]),
    ])
}

pub unsafe fn _mm_rcp_ps(a: __m128) -> __m128 {
    __m128([1.0 / a.0[0], 1.0 / a.0[1], 1.0 / a.0[2], 1.0 / a.0[3]])
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
#[inline(always)]
pub unsafe fn _mm_round_ps<const ROUNDING: i32>(a: __m128) -> __m128 {
    if ROUNDING & 4 != 0 {
        match mxcsr_rm() {
            0 => vx::vx_unary(a, 11, 32, 0),
            1 => vx::vx_unary(a, 11, 32, 1),
            2 => vx::vx_unary(a, 11, 32, 2),
            _ => vx::vx_unary(a, 11, 32, 3),
        }
    } else {
        vx::vx_unary(a, 11, 32, ROUNDING & 3)
    }
}

#[inline(always)]
pub unsafe fn _mm_floor_ps(a: __m128) -> __m128 {
    vx::vx_unary(a, 11, 32, 1)
}

#[inline(always)]
pub unsafe fn _mm_ceil_ps(a: __m128) -> __m128 {
    vx::vx_unary(a, 11, 32, 2)
}

pub unsafe fn _mm_sll_epi32(a: __m128i, count: __m128i) -> __m128i {
    let c = count.0[0] as u64;
    if c > 31 {
        return _mm_setzero_si128();
    }
    let a = lanes128_u32(a);
    let s = c as u32;
    from128_u32([a[0] << s, a[1] << s, a[2] << s, a[3] << s])
}

pub unsafe fn _mm_sllv_epi32(a: __m128i, count: __m128i) -> __m128i {
    let a = lanes128_u32(a);
    let c = lanes128_u32(count);
    let s = |x: u32, n: u32| if n < 32 { x << n } else { 0 };
    from128_u32([s(a[0], c[0]), s(a[1], c[1]), s(a[2], c[2]), s(a[3], c[3])])
}

pub unsafe fn _mm_srl_epi16(a: __m128i, count: __m128i) -> __m128i {
    let c = count.0[0] as u64;
    if c > 15 {
        return _mm_setzero_si128();
    }
    let a = lanes128_u16(a);
    let s = c as u32;
    from128_u16([a[0] >> s, a[1] >> s, a[2] >> s, a[3] >> s, a[4] >> s, a[5] >> s, a[6] >> s, a[7] >> s])
}

pub unsafe fn _mm_srl_epi32(a: __m128i, count: __m128i) -> __m128i {
    let c = count.0[0] as u64;
    if c > 31 {
        return _mm_setzero_si128();
    }
    let a = lanes128_u32(a);
    let s = c as u32;
    from128_u32([a[0] >> s, a[1] >> s, a[2] >> s, a[3] >> s])
}

pub unsafe fn _mm256_add_epi8(a: __m256i, b: __m256i) -> __m256i {
    i_join(_mm_add_epi8(i_lo(&a), i_lo(&b)), _mm_add_epi8(i_hi(&a), i_hi(&b)))
}

pub unsafe fn _mm256_sub_epi8(a: __m256i, b: __m256i) -> __m256i {
    i_join(_mm_sub_epi8(i_lo(&a), i_lo(&b)), _mm_sub_epi8(i_hi(&a), i_hi(&b)))
}

pub unsafe fn _mm256_cmpeq_epi16(a: __m256i, b: __m256i) -> __m256i {
    i_join(_mm_cmpeq_epi16(i_lo(&a), i_lo(&b)), _mm_cmpeq_epi16(i_hi(&a), i_hi(&b)))
}

pub unsafe fn _mm256_cmpeq_epi32(a: __m256i, b: __m256i) -> __m256i {
    i_join(_mm_cmpeq_epi32(i_lo(&a), i_lo(&b)), _mm_cmpeq_epi32(i_hi(&a), i_hi(&b)))
}

pub unsafe fn _mm256_cmpgt_epi16(a: __m256i, b: __m256i) -> __m256i {
    i_join(_mm_cmpgt_epi16(i_lo(&a), i_lo(&b)), _mm_cmpgt_epi16(i_hi(&a), i_hi(&b)))
}

pub unsafe fn _mm256_cmpgt_epi32(a: __m256i, b: __m256i) -> __m256i {
    i_join(_mm_cmpgt_epi32(i_lo(&a), i_lo(&b)), _mm_cmpgt_epi32(i_hi(&a), i_hi(&b)))
}

pub unsafe fn _mm256_max_epu8(a: __m256i, b: __m256i) -> __m256i {
    i_join(_mm_max_epu8(i_lo(&a), i_lo(&b)), _mm_max_epu8(i_hi(&a), i_hi(&b)))
}

pub unsafe fn _mm256_max_epu16(a: __m256i, b: __m256i) -> __m256i {
    i_join(_mm_max_epu16(i_lo(&a), i_lo(&b)), _mm_max_epu16(i_hi(&a), i_hi(&b)))
}

pub unsafe fn _mm256_max_epu32(a: __m256i, b: __m256i) -> __m256i {
    i_join(_mm_max_epu32(i_lo(&a), i_lo(&b)), _mm_max_epu32(i_hi(&a), i_hi(&b)))
}

pub unsafe fn _mm256_min_epu8(a: __m256i, b: __m256i) -> __m256i {
    i_join(_mm_min_epu8(i_lo(&a), i_lo(&b)), _mm_min_epu8(i_hi(&a), i_hi(&b)))
}

pub unsafe fn _mm256_min_epu16(a: __m256i, b: __m256i) -> __m256i {
    i_join(_mm_min_epu16(i_lo(&a), i_lo(&b)), _mm_min_epu16(i_hi(&a), i_hi(&b)))
}

pub unsafe fn _mm256_min_epu32(a: __m256i, b: __m256i) -> __m256i {
    i_join(_mm_min_epu32(i_lo(&a), i_lo(&b)), _mm_min_epu32(i_hi(&a), i_hi(&b)))
}

pub unsafe fn _mm256_cvtepu8_epi16(a: __m128i) -> __m256i {
    let z = _mm_setzero_si128();
    i_join(_mm_unpacklo_epi8(a, z), _mm_unpackhi_epi8(a, z))
}

pub unsafe fn _mm256_cvttps_epi32(a: __m256) -> __m256i {
    i_join(_mm_cvttps_epi32(ps_lo(&a)), _mm_cvttps_epi32(ps_hi(&a)))
}

pub unsafe fn _mm256_srl_epi16(a: __m256i, count: __m128i) -> __m256i {
    i_join(_mm_srl_epi16(i_lo(&a), count), _mm_srl_epi16(i_hi(&a), count))
}

pub unsafe fn _mm256_srlv_epi32(a: __m256i, count: __m256i) -> __m256i {
    let a = lanes256_i32(a);
    let c = lanes256_i32(count);
    let mut r = [0i32; 8];
    let mut i = 0;
    while i < 8 {
        let n = c[i] as u32;
        r[i] = if n < 32 { ((a[i] as u32) >> n) as i32 } else { 0 };
        i += 1;
    }
    from256_i32(r)
}

pub unsafe fn _mm256_setr_epi32(
    e0: i32,
    e1: i32,
    e2: i32,
    e3: i32,
    e4: i32,
    e5: i32,
    e6: i32,
    e7: i32,
) -> __m256i {
    from256_i32([e0, e1, e2, e3, e4, e5, e6, e7])
}

// rustc library/stdarch/crates/core_arch/src/x86/avx.rs: _mm256_set1_epi64x.
#[cfg_attr(any(target_arch = "x86", target_arch = "x86_64"), target_feature(enable = "avx"))]
pub unsafe fn _mm256_set1_epi64x(a: i64) -> __m256i {
    __m256i([a; 4])
}

pub unsafe fn _mm256_setr_ps(e0: f32, e1: f32, e2: f32, e3: f32, e4: f32, e5: f32, e6: f32, e7: f32) -> __m256 {
    ps_join(_mm_setr_ps(e0, e1, e2, e3), _mm_setr_ps(e4, e5, e6, e7))
}

pub unsafe fn _mm256_set_ps(e7: f32, e6: f32, e5: f32, e4: f32, e3: f32, e2: f32, e1: f32, e0: f32) -> __m256 {
    _mm256_setr_ps(e0, e1, e2, e3, e4, e5, e6, e7)
}

pub unsafe fn _mm256_setr_m128(lo: __m128, hi: __m128) -> __m256 {
    ps_join(lo, hi)
}

pub unsafe fn _mm256_setr_m128i(lo: __m128i, hi: __m128i) -> __m256i {
    i_join(lo, hi)
}

pub unsafe fn _mm256_blendv_ps(a: __m256, b: __m256, mask: __m256) -> __m256 {
    ps_join(_mm_blendv_ps(ps_lo(&a), ps_lo(&b), ps_lo(&mask)), _mm_blendv_ps(ps_hi(&a), ps_hi(&b), ps_hi(&mask)))
}

pub unsafe fn _mm256_sqrt_ps(a: __m256) -> __m256 {
    ps_join(_mm_sqrt_ps(ps_lo(&a)), _mm_sqrt_ps(ps_hi(&a)))
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
pub unsafe fn _mm256_round_ps<const ROUNDING: i32>(a: __m256) -> __m256 {
    ps_join(_mm_round_ps::<ROUNDING>(ps_lo(&a)), _mm_round_ps::<ROUNDING>(ps_hi(&a)))
}

pub unsafe fn _mm256_floor_ps(a: __m256) -> __m256 {
    ps_join(_mm_floor_ps(ps_lo(&a)), _mm_floor_ps(ps_hi(&a)))
}

pub unsafe fn _mm256_ceil_ps(a: __m256) -> __m256 {
    ps_join(_mm_ceil_ps(ps_lo(&a)), _mm_ceil_ps(ps_hi(&a)))
}

pub unsafe fn _mm256_unpacklo_ps(a: __m256, b: __m256) -> __m256 {
    ps_join(_mm_unpacklo_ps(ps_lo(&a), ps_lo(&b)), _mm_unpacklo_ps(ps_hi(&a), ps_hi(&b)))
}

pub unsafe fn _mm256_unpackhi_ps(a: __m256, b: __m256) -> __m256 {
    ps_join(_mm_unpackhi_ps(ps_lo(&a), ps_lo(&b)), _mm_unpackhi_ps(ps_hi(&a), ps_hi(&b)))
}

pub unsafe fn _mm256_movemask_ps(a: __m256) -> i32 {
    _mm_movemask_ps(ps_lo(&a)) | (_mm_movemask_ps(ps_hi(&a)) << 4)
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(2))]
pub unsafe fn _mm256_permute2f128_ps<const IMM8: i32>(a: __m256, b: __m256) -> __m256 {
    let pick = |c: i32| -> __m128 {
        if c & 8 != 0 {
            _mm_setzero_ps()
        } else {
            match c & 3 {
                0 => ps_lo(&a),
                1 => ps_hi(&a),
                2 => ps_lo(&b),
                _ => ps_hi(&b),
            }
        }
    };
    ps_join(pick(IMM8 & 15), pick((IMM8 >> 4) & 15))
}

pub unsafe fn _mm256_permutevar8x32_ps(a: __m256, idx: __m256i) -> __m256 {
    let idx = lanes256_i32(idx);
    let mut r = [0f32; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = a.0[(idx[i] as usize) & 7];
        i += 1;
    }
    __m256(r)
}

#[inline(always)]
unsafe fn ps_ones() -> __m128 {
    vx::vx_splat(u32::MAX as u64, 32)
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(2))]
#[inline(always)]
pub unsafe fn _mm_cmp_ps<const IMM5: i32>(a: __m128, b: __m128) -> __m128 {
    let ones = ps_ones();
    match IMM5 & 15 {
        0 => _mm_cmpeq_ps(a, b),
        1 => _mm_cmplt_ps(a, b),
        2 => _mm_cmple_ps(a, b),
        3 => _mm_cmpunord_ps(a, b),
        4 => _mm_cmpneq_ps(a, b),
        5 => _mm_xor_ps(_mm_cmplt_ps(a, b), ones),
        6 => _mm_xor_ps(_mm_cmple_ps(a, b), ones),
        7 => _mm_cmpord_ps(a, b),
        8 => _mm_or_ps(_mm_cmpeq_ps(a, b), _mm_cmpunord_ps(a, b)),
        9 => _mm_xor_ps(_mm_cmpge_ps(a, b), ones),
        10 => _mm_xor_ps(_mm_cmpgt_ps(a, b), ones),
        11 => _mm_setzero_ps(),
        12 => _mm_andnot_ps(_mm_cmpeq_ps(a, b), _mm_cmpord_ps(a, b)),
        13 => _mm_cmpge_ps(a, b),
        14 => _mm_cmpgt_ps(a, b),
        _ => ones,
    }
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(2))]
pub unsafe fn _mm256_cmp_ps<const IMM5: i32>(a: __m256, b: __m256) -> __m256 {
    ps_join(_mm_cmp_ps::<IMM5>(ps_lo(&a), ps_lo(&b)), _mm_cmp_ps::<IMM5>(ps_hi(&a), ps_hi(&b)))
}

// ppv-lite86 0.2.21 / rand_chacha 0.3.1 (Robrix's rand 0.8 ChaCha): the SSE2 / SSE4.1 / AVX2
// integer intrinsics they use, lane by lane (core_arch x86 sse2.rs, sse41.rs, avx2.rs semantics)
pub unsafe fn _mm256_andnot_si256(a: __m256i, b: __m256i) -> __m256i {
    i_join(_mm_andnot_si128(i_lo(&a), i_lo(&b)), _mm_andnot_si128(i_hi(&a), i_hi(&b)))
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(2))]
pub unsafe fn _mm256_inserti128_si256<const IMM1: i32>(a: __m256i, b: __m128i) -> __m256i {
    let mut r = a;
    let h = (IMM1 & 1) as usize;
    r.0[h * 2] = b.0[0];
    r.0[h * 2 + 1] = b.0[1];
    r
}

pub unsafe fn _mm_cmpeq_epi64(a: __m128i, b: __m128i) -> __m128i {
    __m128i([if a.0[0] == b.0[0] { -1 } else { 0 }, if a.0[1] == b.0[1] { -1 } else { 0 }])
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
pub unsafe fn _mm_extract_epi64<const IMM1: i32>(a: __m128i) -> i64 {
    a.0[(IMM1 & 1) as usize]
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(2))]
pub unsafe fn _mm_insert_epi32<const IMM8: i32>(a: __m128i, i: i32) -> __m128i {
    let mut l = lanes128_i32(a);
    l[(IMM8 & 3) as usize] = i;
    from128_i32(l)
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(2))]
pub unsafe fn _mm_insert_epi64<const IMM1: i32>(a: __m128i, i: i64) -> __m128i {
    let mut r = a;
    r.0[(IMM1 & 1) as usize] = i;
    r
}

pub unsafe fn _mm_move_epi64(a: __m128i) -> __m128i {
    __m128i([a.0[0], 0])
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
pub unsafe fn _mm_shufflehi_epi16<const IMM8: i32>(a: __m128i) -> __m128i {
    let l = lanes128_i16(a);
    let mut r = l;
    let mut i = 0;
    while i < 4 {
        r[4 + i] = l[4 + ((IMM8 >> (2 * i)) & 3) as usize];
        i += 1;
    }
    from128_i16(r)
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
pub unsafe fn _mm_shufflelo_epi16<const IMM8: i32>(a: __m128i) -> __m128i {
    let l = lanes128_i16(a);
    let mut r = l;
    let mut i = 0;
    while i < 4 {
        r[i] = l[((IMM8 >> (2 * i)) & 3) as usize];
        i += 1;
    }
    from128_i16(r)
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
pub unsafe fn _mm_slli_si128<const IMM8: i32>(a: __m128i) -> __m128i {
    let l = lanes128_u8(a);
    let n = (IMM8 & 0xff) as usize;
    let mut r = [0u8; 16];
    let mut i = n;
    while i < 16 {
        r[i] = l[i - n];
        i += 1;
    }
    from128_u8(r)
}

// ---- AES-NI and VAES: FIPS-197 byte states, in column-major memory order.

// Multiplication modulo x^8 + x^4 + x^3 + x + 1.
const fn aes_gf_mul(mut a: u8, mut b: u8) -> u8 {
    let mut r = 0;
    while b != 0 {
        if b & 1 != 0 {
            r ^= a;
        }
        let high = a & 0x80;
        a <<= 1;
        if high != 0 {
            a ^= 0x1b;
        }
        b >>= 1;
    }
    r
}

// Derive both tables at compile time: x^-1 = x^254 (with 0 mapped to 0),
// followed by the AES affine map. Invert that permutation for InvSubBytes.
const fn aes_sboxes() -> [[u8; 256]; 2] {
    let mut tables = [[0u8; 256]; 2];
    let mut x = 0;
    while x < 256 {
        let mut base = x as u8;
        let mut exponent = 254;
        let mut inverse = 1;
        while exponent != 0 {
            if exponent & 1 != 0 {
                inverse = aes_gf_mul(inverse, base);
            }
            base = aes_gf_mul(base, base);
            exponent >>= 1;
        }
        let s = inverse ^ inverse.rotate_left(1) ^ inverse.rotate_left(2)
            ^ inverse.rotate_left(3) ^ inverse.rotate_left(4) ^ 0x63;
        tables[0][x] = s;
        tables[1][s as usize] = x as u8;
        x += 1;
    }
    tables
}

const AES_SBOXES: [[u8; 256]; 2] = aes_sboxes();

fn aes_mix_columns(a: [u8; 16], inverse: bool) -> [u8; 16] {
    let coefficients = if inverse { [14, 11, 13, 9] } else { [2, 3, 1, 1] };
    let mut r = [0u8; 16];
    let mut column = 0;
    while column < 4 {
        let mut row = 0;
        while row < 4 {
            let mut k = 0;
            while k < 4 {
                r[column * 4 + row] ^=
                    aes_gf_mul(a[column * 4 + (row + k) % 4], coefficients[k]);
                k += 1;
            }
            row += 1;
        }
        column += 1;
    }
    r
}

unsafe fn aes_round(a: __m128i, round_key: __m128i, inverse: bool, last: bool) -> __m128i {
    let a = lanes128_u8(a);
    let key = lanes128_u8(round_key);
    let mut r = [0u8; 16];
    let mut i = 0;
    while i < 16 {
        let row = i % 4;
        let column = if inverse { (i / 4 + 4 - row) % 4 } else { (i / 4 + row) % 4 };
        r[i] = AES_SBOXES[inverse as usize][a[column * 4 + row] as usize];
        i += 1;
    }
    if !last {
        r = aes_mix_columns(r, inverse);
    }
    i = 0;
    while i < 16 {
        r[i] ^= key[i];
        i += 1;
    }
    from128_u8(r)
}

#[cfg_attr(any(target_arch = "x86", target_arch = "x86_64"), target_feature(enable = "aes"))]
pub unsafe fn _mm_aesenc_si128(a: __m128i, round_key: __m128i) -> __m128i {
    aes_round(a, round_key, false, false)
}

#[cfg_attr(any(target_arch = "x86", target_arch = "x86_64"), target_feature(enable = "aes"))]
pub unsafe fn _mm_aesenclast_si128(a: __m128i, round_key: __m128i) -> __m128i {
    aes_round(a, round_key, false, true)
}

#[cfg_attr(any(target_arch = "x86", target_arch = "x86_64"), target_feature(enable = "aes"))]
pub unsafe fn _mm_aesdec_si128(a: __m128i, round_key: __m128i) -> __m128i {
    // AESDEC uses the equivalent inverse cipher: InvMixColumns precedes
    // AddRoundKey, so intermediate decryption keys must already be transformed.
    aes_round(a, round_key, true, false)
}

#[cfg_attr(any(target_arch = "x86", target_arch = "x86_64"), target_feature(enable = "aes"))]
pub unsafe fn _mm_aesdeclast_si128(a: __m128i, round_key: __m128i) -> __m128i {
    aes_round(a, round_key, true, true)
}

#[cfg_attr(any(target_arch = "x86", target_arch = "x86_64"), target_feature(enable = "aes"))]
pub unsafe fn _mm_aesimc_si128(a: __m128i) -> __m128i {
    from128_u8(aes_mix_columns(lanes128_u8(a), true))
}

#[cfg_attr(any(target_arch = "x86", target_arch = "x86_64"), target_feature(enable = "aes"))]
#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
pub unsafe fn _mm_aeskeygenassist_si128<const IMM8: i32>(a: __m128i) -> __m128i {
    const { assert!(IMM8 >= 0 && IMM8 < 256) };
    let a = lanes128_u8(a);
    let mut r = [0u8; 16];
    let mut word = 0;
    while word < 2 {
        let mut byte = 0;
        while byte < 4 {
            // SubWord(X1), RotWord(SubWord(X1)) XOR RCON, then the same for X3.
            r[word * 8 + byte] = AES_SBOXES[0][a[word * 8 + 4 + byte] as usize];
            r[word * 8 + 4 + byte] = AES_SBOXES[0][a[word * 8 + 4 + (byte + 1) % 4] as usize];
            byte += 1;
        }
        r[word * 8 + 4] ^= IMM8 as u8;
        word += 1;
    }
    from128_u8(r)
}

unsafe fn aes_round256(a: __m256i, round_key: __m256i, inverse: bool, last: bool) -> __m256i {
    i_join(
        aes_round(i_lo(&a), i_lo(&round_key), inverse, last),
        aes_round(i_hi(&a), i_hi(&round_key), inverse, last),
    )
}

unsafe fn aes_round512(a: __m512i, round_key: __m512i, inverse: bool, last: bool) -> __m512i {
    let mut a = lanes512_i128(a);
    let key = lanes512_i128(round_key);
    let mut i = 0;
    while i < 4 {
        a[i] = aes_round(a[i], key[i], inverse, last);
        i += 1;
    }
    from512_i128(a)
}

#[cfg_attr(any(target_arch = "x86", target_arch = "x86_64"), target_feature(enable = "vaes"))]
pub unsafe fn _mm256_aesenc_epi128(a: __m256i, round_key: __m256i) -> __m256i {
    aes_round256(a, round_key, false, false)
}

#[cfg_attr(any(target_arch = "x86", target_arch = "x86_64"), target_feature(enable = "vaes"))]
pub unsafe fn _mm256_aesenclast_epi128(a: __m256i, round_key: __m256i) -> __m256i {
    aes_round256(a, round_key, false, true)
}

#[cfg_attr(any(target_arch = "x86", target_arch = "x86_64"), target_feature(enable = "vaes"))]
pub unsafe fn _mm256_aesdec_epi128(a: __m256i, round_key: __m256i) -> __m256i {
    aes_round256(a, round_key, true, false)
}

#[cfg_attr(any(target_arch = "x86", target_arch = "x86_64"), target_feature(enable = "vaes"))]
pub unsafe fn _mm256_aesdeclast_epi128(a: __m256i, round_key: __m256i) -> __m256i {
    aes_round256(a, round_key, true, true)
}

#[cfg_attr(any(target_arch = "x86", target_arch = "x86_64"), target_feature(enable = "vaes,avx512f"))]
pub unsafe fn _mm512_aesenc_epi128(a: __m512i, round_key: __m512i) -> __m512i {
    aes_round512(a, round_key, false, false)
}

#[cfg_attr(any(target_arch = "x86", target_arch = "x86_64"), target_feature(enable = "vaes,avx512f"))]
pub unsafe fn _mm512_aesenclast_epi128(a: __m512i, round_key: __m512i) -> __m512i {
    aes_round512(a, round_key, false, true)
}

#[cfg_attr(any(target_arch = "x86", target_arch = "x86_64"), target_feature(enable = "vaes,avx512f"))]
pub unsafe fn _mm512_aesdec_epi128(a: __m512i, round_key: __m512i) -> __m512i {
    aes_round512(a, round_key, true, false)
}

#[cfg_attr(any(target_arch = "x86", target_arch = "x86_64"), target_feature(enable = "vaes,avx512f"))]
pub unsafe fn _mm512_aesdeclast_epi128(a: __m512i, round_key: __m512i) -> __m512i {
    aes_round512(a, round_key, true, true)
}

// ---- Integer broadcasts, lane selection, arithmetic and unaligned memory.

#[cfg_attr(any(target_arch = "x86", target_arch = "x86_64"), target_feature(enable = "avx2"))]
pub unsafe fn _mm256_broadcastsi128_si256(a: __m128i) -> __m256i {
    i_join(a, a)
}

#[cfg_attr(any(target_arch = "x86", target_arch = "x86_64"), target_feature(enable = "avx512f,avx512vl"))]
pub unsafe fn _mm256_broadcast_i32x4(a: __m128i) -> __m256i {
    i_join(a, a)
}

#[cfg_attr(any(target_arch = "x86", target_arch = "x86_64"), target_feature(enable = "avx512f"))]
pub unsafe fn _mm512_broadcast_i32x4(a: __m128i) -> __m512i {
    from512_i128([a; 4])
}

#[cfg_attr(any(target_arch = "x86", target_arch = "x86_64"), target_feature(enable = "avx2"))]
pub unsafe fn _mm256_broadcastd_epi32(a: __m128i) -> __m256i {
    from256_i32([a.0[0] as i32; 8])
}

#[cfg_attr(any(target_arch = "x86", target_arch = "x86_64"), target_feature(enable = "avx"))]
#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
pub unsafe fn _mm256_extract_epi32<const INDEX: i32>(a: __m256i) -> i32 {
    const { assert!(INDEX >= 0 && INDEX < 8) };
    lanes256_i32(a)[INDEX as usize]
}

#[cfg_attr(any(target_arch = "x86", target_arch = "x86_64"), target_feature(enable = "avx"))]
#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
pub unsafe fn _mm256_extract_epi64<const INDEX: i32>(a: __m256i) -> i64 {
    const { assert!(INDEX >= 0 && INDEX < 4) };
    a.0[INDEX as usize]
}

#[cfg_attr(any(target_arch = "x86", target_arch = "x86_64"), target_feature(enable = "avx"))]
pub unsafe fn _mm256_setr_epi64x(a: i64, b: i64, c: i64, d: i64) -> __m256i {
    __m256i([a, b, c, d])
}

#[cfg_attr(any(target_arch = "x86", target_arch = "x86_64"), target_feature(enable = "avx512f"))]
pub unsafe fn _mm512_setr_epi64(
    e0: i64,
    e1: i64,
    e2: i64,
    e3: i64,
    e4: i64,
    e5: i64,
    e6: i64,
    e7: i64,
) -> __m512i {
    __m512i([e0, e1, e2, e3, e4, e5, e6, e7])
}

#[cfg_attr(any(target_arch = "x86", target_arch = "x86_64"), target_feature(enable = "avx"))]
pub unsafe fn _mm256_set_m128i(hi: __m128i, lo: __m128i) -> __m256i {
    i_join(lo, hi)
}

#[cfg_attr(any(target_arch = "x86", target_arch = "x86_64"), target_feature(enable = "avx"))]
#[cfg_attr(not(relax_check), rustc_legacy_const_generics(2))]
pub unsafe fn _mm256_insertf128_si256<const IMM1: i32>(a: __m256i, b: __m128i) -> __m256i {
    const { assert!(IMM1 >= 0 && IMM1 < 2) };
    if IMM1 == 0 { i_join(b, i_hi(&a)) } else { i_join(i_lo(&a), b) }
}

#[cfg_attr(any(target_arch = "x86", target_arch = "x86_64"), target_feature(enable = "avx512f,avx512vl"))]
pub unsafe fn _mm256_permutex2var_epi64(a: __m256i, idx: __m256i, b: __m256i) -> __m256i {
    let mut r = [0i64; 4];
    let mut i = 0;
    while i < 4 {
        let selector = (idx.0[i] as u64 & 7) as usize;
        r[i] = if selector < 4 { a.0[selector] } else { b.0[selector - 4] };
        i += 1;
    }
    __m256i(r)
}

#[cfg_attr(any(target_arch = "x86", target_arch = "x86_64"), target_feature(enable = "avx512f"))]
pub unsafe fn _mm512_permutex2var_epi64(a: __m512i, idx: __m512i, b: __m512i) -> __m512i {
    let mut r = [0i64; 8];
    let mut i = 0;
    while i < 8 {
        let selector = (idx.0[i] as u64 & 15) as usize;
        r[i] = if selector < 8 { a.0[selector] } else { b.0[selector - 8] };
        i += 1;
    }
    __m512i(r)
}

#[cfg_attr(any(target_arch = "x86", target_arch = "x86_64"), target_feature(enable = "avx512f"))]
#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
pub unsafe fn _mm512_shuffle_epi32<const MASK: _MM_PERM_ENUM>(a: __m512i) -> __m512i {
    const { assert!(MASK >= 0 && MASK < 256) };
    let a = lanes512_i32(a);
    let mut r = [0i32; 16];
    let mut i = 0;
    while i < 16 {
        r[i] = a[(i / 4) * 4 + ((MASK >> (2 * (i % 4))) & 3) as usize];
        i += 1;
    }
    from512_i32(r)
}

#[cfg_attr(any(target_arch = "x86", target_arch = "x86_64"), target_feature(enable = "avx512f"))]
#[cfg_attr(not(relax_check), rustc_legacy_const_generics(2))]
pub unsafe fn _mm512_shuffle_i32x4<const MASK: i32>(a: __m512i, b: __m512i) -> __m512i {
    const { assert!(MASK >= 0 && MASK < 256) };
    let a = lanes512_i128(a);
    let b = lanes512_i128(b);
    from512_i128([
        a[(MASK & 3) as usize],
        a[((MASK >> 2) & 3) as usize],
        b[((MASK >> 4) & 3) as usize],
        b[((MASK >> 6) & 3) as usize],
    ])
}

#[cfg_attr(any(target_arch = "x86", target_arch = "x86_64"), target_feature(enable = "avx512f"))]
pub unsafe fn _mm512_add_epi64(a: __m512i, b: __m512i) -> __m512i {
    let mut r = [0i64; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = a.0[i].wrapping_add(b.0[i]);
        i += 1;
    }
    __m512i(r)
}

#[cfg_attr(any(target_arch = "x86", target_arch = "x86_64"), target_feature(enable = "avx512f,avx512vl"))]
#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
pub unsafe fn _mm_rol_epi32<const IMM8: i32>(a: __m128i) -> __m128i {
    const { assert!(IMM8 >= 0 && IMM8 < 256) };
    let mut r = lanes128_u32(a);
    let mut i = 0;
    while i < 4 {
        r[i] = r[i].rotate_left(IMM8 as u32);
        i += 1;
    }
    from128_u32(r)
}

#[cfg_attr(any(target_arch = "x86", target_arch = "x86_64"), target_feature(enable = "avx512f,avx512vl"))]
#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
pub unsafe fn _mm256_rol_epi32<const IMM8: i32>(a: __m256i) -> __m256i {
    const { assert!(IMM8 >= 0 && IMM8 < 256) };
    i_join(_mm_rol_epi32::<IMM8>(i_lo(&a)), _mm_rol_epi32::<IMM8>(i_hi(&a)))
}

#[cfg_attr(any(target_arch = "x86", target_arch = "x86_64"), target_feature(enable = "avx512f"))]
#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
pub unsafe fn _mm512_rol_epi32<const IMM8: i32>(a: __m512i) -> __m512i {
    const { assert!(IMM8 >= 0 && IMM8 < 256) };
    let mut r = lanes512_i32(a);
    let mut i = 0;
    while i < 16 {
        r[i] = (r[i] as u32).rotate_left(IMM8 as u32) as i32;
        i += 1;
    }
    from512_i32(r)
}

#[cfg_attr(any(target_arch = "x86", target_arch = "x86_64"), target_feature(enable = "avx512f,avx512vl"))]
pub unsafe fn _mm_loadu_epi32(mem_addr: *const i32) -> __m128i {
    crate::ptr::read_unaligned(mem_addr as *const __m128i)
}

#[cfg_attr(any(target_arch = "x86", target_arch = "x86_64"), target_feature(enable = "avx512f,avx512vl"))]
pub unsafe fn _mm_storeu_epi32(mem_addr: *mut i32, a: __m128i) {
    crate::ptr::write_unaligned(mem_addr as *mut __m128i, a);
}

#[cfg_attr(any(target_arch = "x86", target_arch = "x86_64"), target_feature(enable = "avx512f,avx512vl"))]
pub unsafe fn _mm256_storeu_epi32(mem_addr: *mut i32, a: __m256i) {
    crate::ptr::write_unaligned(mem_addr as *mut __m256i, a);
}

#[cfg_attr(any(target_arch = "x86", target_arch = "x86_64"), target_feature(enable = "avx512f"))]
pub unsafe fn _mm512_storeu_si512(mem_addr: *mut __m512i, a: __m512i) {
    crate::ptr::write_unaligned(mem_addr, a);
}

#[cfg_attr(any(target_arch = "x86", target_arch = "x86_64"), target_feature(enable = "sse2"))]
pub unsafe fn _mm_storel_epi64(mem_addr: *mut __m128i, a: __m128i) {
    crate::ptr::write_unaligned(mem_addr as *mut i64, a.0[0]);
}

#[cfg_attr(any(target_arch = "x86", target_arch = "x86_64"), target_feature(enable = "avx2"))]
pub unsafe fn _mm256_mulhi_epu16(a: __m256i, b: __m256i) -> __m256i {
    i_join(_mm_mulhi_epu16(i_lo(&a), i_lo(&b)), _mm_mulhi_epu16(i_hi(&a), i_hi(&b)))
}

#[cfg_attr(any(target_arch = "x86", target_arch = "x86_64"), target_feature(enable = "avx2"))]
pub unsafe fn _mm256_sllv_epi32(a: __m256i, count: __m256i) -> __m256i {
    i_join(_mm_sllv_epi32(i_lo(&a), i_lo(&count)), _mm_sllv_epi32(i_hi(&a), i_hi(&count)))
}

#[cfg_attr(any(target_arch = "x86", target_arch = "x86_64"), target_feature(enable = "avx2"))]
pub unsafe fn _mm256_srlv_epi64(a: __m256i, count: __m256i) -> __m256i {
    let mut r = [0i64; 4];
    let mut i = 0;
    while i < 4 {
        let n = count.0[i] as u64;
        r[i] = if n < 64 { ((a.0[i] as u64) >> n) as i64 } else { 0 };
        i += 1;
    }
    __m256i(r)
}

#[cfg_attr(any(target_arch = "x86", target_arch = "x86_64"), target_feature(enable = "sse2"))]
pub unsafe fn _mm_set_epi16(
    e7: i16,
    e6: i16,
    e5: i16,
    e4: i16,
    e3: i16,
    e2: i16,
    e1: i16,
    e0: i16,
) -> __m128i {
    from128_i16([e0, e1, e2, e3, e4, e5, e6, e7])
}

// IFMA ignores the top 12 bits of each multiplicand, but retains all 64
// accumulator bits. Only the addition wraps; the product has 104 bits.
#[cfg_attr(any(target_arch = "x86", target_arch = "x86_64"), target_feature(enable = "avx512ifma,avx512vl"))]
pub unsafe fn _mm256_madd52lo_epu64(a: __m256i, b: __m256i, c: __m256i) -> __m256i {
    let mask = (1u64 << 52) - 1;
    let mut r = [0i64; 4];
    let mut i = 0;
    while i < 4 {
        let product = ((b.0[i] as u64 & mask) as u128) * ((c.0[i] as u64 & mask) as u128);
        r[i] = (a.0[i] as u64).wrapping_add(product as u64 & mask) as i64;
        i += 1;
    }
    __m256i(r)
}

#[cfg_attr(any(target_arch = "x86", target_arch = "x86_64"), target_feature(enable = "avx512ifma,avx512vl"))]
pub unsafe fn _mm256_madd52hi_epu64(a: __m256i, b: __m256i, c: __m256i) -> __m256i {
    let mask = (1u64 << 52) - 1;
    let mut r = [0i64; 4];
    let mut i = 0;
    while i < 4 {
        let product = ((b.0[i] as u64 & mask) as u128) * ((c.0[i] as u64 & mask) as u128);
        r[i] = (a.0[i] as u64).wrapping_add((product >> 52) as u64) as i64;
        i += 1;
    }
    __m256i(r)
}
