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

/// Read an extended control register. The caller must ensure XSAVE/OSXSAVE
/// support and that `xcr` names an index supported by the processor.
#[inline]
pub unsafe fn _xgetbv(xcr: u32) -> u64 {
    crate::intrinsics_rt::xgetbv(xcr)
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

pub const fn _MM_SHUFFLE(z: u32, y: u32, x: u32, w: u32) -> i32 {
    ((z << 6) | (y << 4) | (x << 2) | w) as i32
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(2))]
pub unsafe fn _mm256_blend_epi32<const IMM8: i32>(a: __m256i, b: __m256i) -> __m256i {
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

pub unsafe fn _mm256_castsi256_si128(a: __m256i) -> __m128i {
    __m128i([a.0[0], a.0[1]])
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
pub unsafe fn _mm256_sub_epi16(a: __m256i, b: __m256i) -> __m256i {
    i_join(_mm_sub_epi16(i_lo(&a), i_lo(&b)), _mm_sub_epi16(i_hi(&a), i_hi(&b)))
}
pub unsafe fn _mm256_sub_epi32(a: __m256i, b: __m256i) -> __m256i {
    i_join(_mm_sub_epi32(i_lo(&a), i_lo(&b)), _mm_sub_epi32(i_hi(&a), i_hi(&b)))
}
pub unsafe fn _mm256_cmpeq_epi8(a: __m256i, b: __m256i) -> __m256i {
    i_join(_mm_cmpeq_epi8(i_lo(&a), i_lo(&b)), _mm_cmpeq_epi8(i_hi(&a), i_hi(&b)))
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
#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
pub unsafe fn _mm256_slli_epi16<const IMM8: i32>(a: __m256i) -> __m256i {
    i_join(_mm_slli_epi16::<IMM8>(i_lo(&a)), _mm_slli_epi16::<IMM8>(i_hi(&a)))
}
#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
pub unsafe fn _mm256_slli_epi32<const IMM8: i32>(a: __m256i) -> __m256i {
    i_join(_mm_slli_epi32::<IMM8>(i_lo(&a)), _mm_slli_epi32::<IMM8>(i_hi(&a)))
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

pub unsafe fn _mm_cvtsi32_si128(a: i32) -> __m128i {
    __m128i([a as u32 as i64, 0])
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
simd_debug!(__m128i __m128 __m128d __m256 __m256d __m512 __m512d __m256i __m512i);

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
