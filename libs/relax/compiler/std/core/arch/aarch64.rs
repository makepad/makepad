//! NEON, CRC32 and SHA-1 operations. Lane zero is the lowest-address lane.
//! Hot 128-bit NEON operations use the compiler vector bridge.
//!
//! Signatures and lane operations follow stdarch's aarch64 and arm_shared/neon.
//! CRC32 uses the reflected IEEE polynomial, without initial/final complements.
//! SHA-1 follows Arm's four-round and message-schedule pseudocode.
//!
//! Unsafe entry points retain stdarch's pointer and immediate contracts: pointers
//! must cover the full load/store and lane/shift constants must be in range.
//! These functions emulate result bits, not architectural status flags.
#![allow(non_camel_case_types)]
use crate::intrinsics_vx as vx;

#[derive(Copy, Clone)]
#[repr(C, align(8))]
pub struct int8x8_t(pub [i8; 8]);

#[derive(Copy, Clone)]
#[repr(C, align(16))]
pub struct int16x8_t(pub [i16; 8]);

#[derive(Copy, Clone)]
#[repr(C, align(16))]
pub struct int32x4_t(pub [i32; 4]);

#[derive(Copy, Clone)]
#[repr(C, align(16))]
pub struct int64x2_t(pub [i64; 2]);

#[derive(Copy, Clone)]
#[repr(C, align(16))]
pub struct uint16x8_t(pub [u16; 8]);

#[derive(Copy, Clone)]
#[repr(C, align(8))]
pub struct uint16x4_t(pub [u16; 4]);

#[derive(Copy, Clone)]
#[repr(C, align(8))]
pub struct uint32x2_t(pub [u32; 2]);

#[derive(Copy, Clone)]
#[repr(C, align(16))]
pub struct uint32x4_t(pub [u32; 4]);

#[derive(Copy, Clone)]
#[repr(C, align(8))]
pub struct uint64x1_t(pub [u64; 1]);

#[derive(Copy, Clone)]
#[repr(C, align(8))]
pub struct uint8x8_t(pub [u8; 8]);

#[derive(Copy, Clone)]
#[repr(C, align(16))]
pub struct uint8x16_t(pub [u8; 16]);

#[derive(Copy, Clone)]
#[repr(C, align(8))]
pub struct int16x4_t(pub [i16; 4]);

#[derive(Copy, Clone)]
#[repr(C, align(8))]
pub struct int32x2_t(pub [i32; 2]);

#[derive(Copy, Clone)]
#[repr(C, align(16))]
pub struct uint64x2_t(pub [u64; 2]);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct int16x8x2_t(pub int16x8_t, pub int16x8_t);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct int32x4x2_t(pub int32x4_t, pub int32x4_t);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct uint8x16x3_t(pub uint8x16_t, pub uint8x16_t, pub uint8x16_t);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct uint8x16x4_t(
    pub uint8x16_t,
    pub uint8x16_t,
    pub uint8x16_t,
    pub uint8x16_t,
);

/// This thread's FPCR (`mrs x, fpcr`): rounding mode, flush-to-zero (FZ, bit 24), default NaN
/// (DN, bit 25). A Relax builtin (no inline assembly under Relax): code shared with rustc reads
/// it under `#[cfg(relax)]` and with `asm!("mrs {0}, fpcr", ..)` otherwise.
pub unsafe fn get_fpcr() -> u64 {
    crate::intrinsics_rt::fp_env_get()
}

/// Sets this thread's FPCR (`msr fpcr, x`).
pub unsafe fn set_fpcr(x: u64) {
    crate::intrinsics_rt::fp_env_set(x)
}

pub unsafe fn __crc32b(crc: u32, data: u8) -> u32 {
    let mut r = crc ^ data as u32;
    let mut i = 0;
    while i < 8 {
        r = (r >> 1) ^ (0xedb8_8320u32 & (0u32.wrapping_sub(r & 1)));
        i += 1;
    }
    r
}

pub unsafe fn __crc32d(crc: u32, data: u64) -> u32 {
    let mut r = crc;
    let mut i = 0;
    while i < 8 {
        r = __crc32b(r, (data >> (i * 8)) as u8);
        i += 1;
    }
    r
}

#[inline(always)]
pub unsafe fn vaddq_s16(a: int16x8_t, b: int16x8_t) -> int16x8_t {
    vx::vx_binary(a, b, 0, 16)
}

#[inline(always)]
pub unsafe fn vaddq_s32(a: int32x4_t, b: int32x4_t) -> int32x4_t {
    vx::vx_binary(a, b, 0, 32)
}

#[inline(always)]
pub unsafe fn vaddq_u32(a: uint32x4_t, b: uint32x4_t) -> uint32x4_t {
    vx::vx_binary(a, b, 0, 32)
}

pub unsafe fn vaddvq_u32(a: uint32x4_t) -> u32 {
    let mut r = 0u32;
    let mut i = 0;
    while i < 4 {
        r = r.wrapping_add(a.0[i]);
        i += 1;
    }
    r
}

pub unsafe fn vaddw_u8(a: uint16x8_t, b: uint8x8_t) -> uint16x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = a.0[i].wrapping_add(b.0[i] as u16);
        i += 1;
    }
    uint16x8_t(r)
}

pub unsafe fn vceq_u8(a: uint8x8_t, b: uint8x8_t) -> uint8x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = if a.0[i] == b.0[i] { 255 } else { 0 };
        i += 1;
    }
    uint8x8_t(r)
}

pub unsafe fn vcgez_s8(a: int8x8_t) -> uint8x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = if a.0[i] >= 0 { 255 } else { 0 };
        i += 1;
    }
    uint8x8_t(r)
}

pub unsafe fn vcltz_s8(a: int8x8_t) -> uint8x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = if a.0[i] < 0 { 255 } else { 0 };
        i += 1;
    }
    uint8x8_t(r)
}

pub unsafe fn vcombine_s16(a: int16x4_t, b: int16x4_t) -> int16x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = if i < 4 { a.0[i] } else { b.0[i - 4] };
        i += 1;
    }
    int16x8_t(r)
}

pub unsafe fn vcombine_u16(a: uint16x4_t, b: uint16x4_t) -> uint16x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = if i < 4 { a.0[i] } else { b.0[i - 4] };
        i += 1;
    }
    uint16x8_t(r)
}

pub unsafe fn vcombine_u8(a: uint8x8_t, b: uint8x8_t) -> uint8x16_t {
    let mut r = [0; 16];
    let mut i = 0;
    while i < 16 {
        r[i] = if i < 8 { a.0[i] } else { b.0[i - 8] };
        i += 1;
    }
    uint8x16_t(r)
}

pub unsafe fn vcreate_s16(a: u64) -> int16x4_t {
    int16x4_t([
        a as i16,
        (a >> 16) as i16,
        (a >> 32) as i16,
        (a >> 48) as i16,
    ])
}

pub unsafe fn vdup_n_u8(value: u8) -> uint8x8_t {
    uint8x8_t([value; 8])
}

#[inline(always)]
pub unsafe fn vdupq_n_s16(value: i16) -> int16x8_t {
    vx::vx_splat(value as u64, 16)
}

#[inline(always)]
pub unsafe fn vdupq_n_s32(value: i32) -> int32x4_t {
    vx::vx_splat(value as u64, 32)
}

#[inline(always)]
pub unsafe fn vdupq_n_u16(value: u16) -> uint16x8_t {
    vx::vx_splat(value as u64, 16)
}

#[inline(always)]
pub unsafe fn vdupq_n_u32(value: u32) -> uint32x4_t {
    vx::vx_splat(value as u64, 32)
}

#[inline(always)]
pub unsafe fn vdupq_n_u8(value: u8) -> uint8x16_t {
    vx::vx_splat(value as u64, 8)
}

pub unsafe fn vget_high_u16(a: uint16x8_t) -> uint16x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = a.0[i + 4];
        i += 1;
    }
    uint16x4_t(r)
}

pub unsafe fn vget_high_u32(a: uint32x4_t) -> uint32x2_t {
    let mut r = [0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = a.0[i + 2];
        i += 1;
    }
    uint32x2_t(r)
}

pub unsafe fn vget_high_u8(a: uint8x16_t) -> uint8x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = a.0[i + 8];
        i += 1;
    }
    uint8x8_t(r)
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
pub unsafe fn vget_lane_u32<const LANE: i32>(v: uint32x2_t) -> u32 {
    v.0[LANE as usize]
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
pub unsafe fn vget_lane_u64<const LANE: i32>(v: uint64x1_t) -> u64 {
    v.0[LANE as usize]
}

pub unsafe fn vget_low_s16(a: int16x8_t) -> int16x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = a.0[i + 0];
        i += 1;
    }
    int16x4_t(r)
}

pub unsafe fn vget_low_u16(a: uint16x8_t) -> uint16x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = a.0[i + 0];
        i += 1;
    }
    uint16x4_t(r)
}

pub unsafe fn vget_low_u32(a: uint32x4_t) -> uint32x2_t {
    let mut r = [0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = a.0[i + 0];
        i += 1;
    }
    uint32x2_t(r)
}

pub unsafe fn vget_low_u8(a: uint8x16_t) -> uint8x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = a.0[i + 0];
        i += 1;
    }
    uint8x8_t(r)
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
pub unsafe fn vgetq_lane_u32<const LANE: i32>(v: uint32x4_t) -> u32 {
    v.0[LANE as usize]
}

pub unsafe fn vld1_u16(ptr: *const u16) -> uint16x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = crate::ptr::read_unaligned(ptr.add(i));
        i += 1;
    }
    uint16x4_t(r)
}

pub unsafe fn vld1_u8(ptr: *const u8) -> uint8x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = crate::ptr::read_unaligned(ptr.add(i));
        i += 1;
    }
    uint8x8_t(r)
}

#[inline(always)]
pub unsafe fn vld1q_s16(a: *const i16) -> int16x8_t {
    vx::vx_load(a as *const u8)
}

pub unsafe fn vld1q_s16_x2(a: *const i16) -> int16x8x2_t {
    int16x8x2_t(
        crate::ptr::read_unaligned(a as *const int16x8_t),
        crate::ptr::read_unaligned(a.add(8) as *const int16x8_t),
    )
}

pub unsafe fn vld1q_s32_x2(a: *const i32) -> int32x4x2_t {
    int32x4x2_t(
        crate::ptr::read_unaligned(a as *const int32x4_t),
        crate::ptr::read_unaligned(a.add(4) as *const int32x4_t),
    )
}

#[inline(always)]
pub unsafe fn vld1q_u16(a: *const u16) -> uint16x8_t {
    vx::vx_load(a as *const u8)
}

#[inline(always)]
pub unsafe fn vld1q_u32(a: *const u32) -> uint32x4_t {
    vx::vx_load(a as *const u8)
}

#[inline(always)]
pub unsafe fn vld1q_u8(a: *const u8) -> uint8x16_t {
    vx::vx_load(a as *const u8)
}

#[inline(always)]
pub unsafe fn vmaxq_s16(a: int16x8_t, b: int16x8_t) -> int16x8_t {
    vx::vx_binary(a, b, 14, 16)
}

pub unsafe fn vmaxvq_u32(a: uint32x4_t) -> u32 {
    let mut r = 0u32;
    let mut i = 0;
    while i < 4 {
        r = r.max(a.0[i]);
        i += 1;
    }
    r
}

#[inline(always)]
pub unsafe fn vminq_s16(a: int16x8_t, b: int16x8_t) -> int16x8_t {
    vx::vx_binary(a, b, 12, 16)
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(3))]
pub unsafe fn vmlal_high_laneq_s16<const LANE: i32>(
    a: int32x4_t,
    b: int16x8_t,
    c: int16x8_t,
) -> int32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = a.0[i].wrapping_add((b.0[i + 4] as i32) * (c.0[LANE as usize] as i32));
        i += 1;
    }
    int32x4_t(r)
}

pub unsafe fn vmlal_high_u16(a: uint32x4_t, b: uint16x8_t, c: uint16x8_t) -> uint32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = a.0[i].wrapping_add((b.0[i + 4] as u32) * (c.0[i + 4] as u32));
        i += 1;
    }
    uint32x4_t(r)
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(3))]
pub unsafe fn vmlal_laneq_s16<const LANE: i32>(
    a: int32x4_t,
    b: int16x4_t,
    c: int16x8_t,
) -> int32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = a.0[i].wrapping_add((b.0[i + 0] as i32) * (c.0[LANE as usize] as i32));
        i += 1;
    }
    int32x4_t(r)
}

pub unsafe fn vmlal_u16(a: uint32x4_t, b: uint16x4_t, c: uint16x4_t) -> uint32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = a.0[i].wrapping_add((b.0[i + 0] as u32) * (c.0[i + 0] as u32));
        i += 1;
    }
    uint32x4_t(r)
}

#[inline(always)]
pub unsafe fn vmulq_s16(a: int16x8_t, b: int16x8_t) -> int16x8_t {
    vx::vx_binary(a, b, 26, 16)
}

#[inline(always)]
pub unsafe fn vmulq_s32(a: int32x4_t, b: int32x4_t) -> int32x4_t {
    vx::vx_binary(a, b, 26, 32)
}

pub unsafe fn vorr_u8(a: uint8x8_t, b: uint8x8_t) -> uint8x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = a.0[i] | b.0[i];
        i += 1;
    }
    uint8x8_t(r)
}

#[inline(always)]
pub unsafe fn vorrq_s32(a: int32x4_t, b: int32x4_t) -> int32x4_t {
    vx::vx_binary(a, b, 7, 32)
}

pub unsafe fn vpadalq_u16(a: uint32x4_t, b: uint16x8_t) -> uint32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = a.0[i].wrapping_add(b.0[i * 2] as u32 + b.0[i * 2 + 1] as u32);
        i += 1;
    }
    uint32x4_t(r)
}

pub unsafe fn vpadalq_u8(a: uint16x8_t, b: uint8x16_t) -> uint16x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = a.0[i].wrapping_add(b.0[i * 2] as u16 + b.0[i * 2 + 1] as u16);
        i += 1;
    }
    uint16x8_t(r)
}

pub unsafe fn vpadd_u32(a: uint32x2_t, b: uint32x2_t) -> uint32x2_t {
    uint32x2_t([a.0[0].wrapping_add(a.0[1]), b.0[0].wrapping_add(b.0[1])])
}

pub unsafe fn vpaddlq_u8(a: uint8x16_t) -> uint16x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = a.0[i * 2] as u16 + a.0[i * 2 + 1] as u16;
        i += 1;
    }
    uint16x8_t(r)
}

#[inline(always)]
pub unsafe fn vqmovn_s32(a: int32x4_t) -> int16x4_t {
    vx::vx_narrow(a, 21)
}

#[inline(always)]
pub unsafe fn vqmovn_u16(a: uint16x8_t) -> uint8x8_t {
    vx::vx_narrow(a, 30)
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
pub unsafe fn vqshlq_n_u32<const N: i32>(a: uint32x4_t) -> uint32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = ((a.0[i] as u64) << N).min(0xffff_ffff) as u32;
        i += 1;
    }
    uint32x4_t(r)
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
pub unsafe fn vqshrun_n_s32<const N: i32>(a: int32x4_t) -> uint16x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = (a.0[i] >> N).max(0).min(65535) as u16;
        i += 1;
    }
    uint16x4_t(r)
}

pub unsafe fn vreinterpret_s8_u8(a: uint8x8_t) -> int8x8_t {
    crate::ptr::read_unaligned((&a as *const uint8x8_t).cast())
}

pub unsafe fn vreinterpret_u64_u8(a: uint8x8_t) -> uint64x1_t {
    crate::ptr::read_unaligned((&a as *const uint8x8_t).cast())
}

pub unsafe fn vreinterpretq_s32_s64(a: int64x2_t) -> int32x4_t {
    crate::ptr::read_unaligned(&a as *const int64x2_t as *const int32x4_t)
}

pub unsafe fn vreinterpretq_s32_u32(a: uint32x4_t) -> int32x4_t {
    crate::ptr::read_unaligned(&a as *const uint32x4_t as *const int32x4_t)
}

pub unsafe fn vreinterpretq_s64_s32(a: int32x4_t) -> int64x2_t {
    crate::ptr::read_unaligned(&a as *const int32x4_t as *const int64x2_t)
}

pub unsafe fn vreinterpretq_u32_s32(a: int32x4_t) -> uint32x4_t {
    crate::ptr::read_unaligned(&a as *const int32x4_t as *const uint32x4_t)
}

pub unsafe fn vreinterpretq_u32_u8(a: uint8x16_t) -> uint32x4_t {
    crate::ptr::read_unaligned(&a as *const uint8x16_t as *const uint32x4_t)
}

pub unsafe fn vrev32q_u8(a: uint8x16_t) -> uint8x16_t {
    let mut r = [0; 16];
    let mut i = 0;
    while i < 16 {
        r[i] = a.0[(i & !3) + 3 - (i & 3)];
        i += 1;
    }
    uint8x16_t(r)
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(2))]
pub unsafe fn vsetq_lane_u32<const LANE: i32>(a: u32, b: uint32x4_t) -> uint32x4_t {
    let mut r = b;
    r.0[LANE as usize] = a;
    r
}

// Hash state is [A, B, C, D]; wk already includes the SHA round constant.
#[inline(always)]
pub unsafe fn vsha1cq_u32(hash_abcd: uint32x4_t, hash_e: u32, wk: uint32x4_t) -> uint32x4_t {
    vx::vx_ternary(hash_abcd, hash_e, wk, 1)
}

#[inline(always)]
pub unsafe fn vsha1h_u32(hash_e: u32) -> u32 {
    vx::vx_sha1h(hash_e)
}

#[inline(always)]
pub unsafe fn vsha1mq_u32(hash_abcd: uint32x4_t, hash_e: u32, wk: uint32x4_t) -> uint32x4_t {
    vx::vx_ternary(hash_abcd, hash_e, wk, 3)
}

#[inline(always)]
pub unsafe fn vsha1pq_u32(hash_abcd: uint32x4_t, hash_e: u32, wk: uint32x4_t) -> uint32x4_t {
    vx::vx_ternary(hash_abcd, hash_e, wk, 2)
}

#[inline(always)]
pub unsafe fn vsha1su0q_u32(w0_3: uint32x4_t, w4_7: uint32x4_t, w8_11: uint32x4_t) -> uint32x4_t {
    vx::vx_ternary(w0_3, w4_7, w8_11, 4)
}

#[inline(always)]
pub unsafe fn vsha1su1q_u32(tw0_3: uint32x4_t, w12_15: uint32x4_t) -> uint32x4_t {
    vx::vx_binary(tw0_3, w12_15, 80, 32)
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
#[inline(always)]
pub unsafe fn vshlq_n_u32<const N: i32>(a: uint32x4_t) -> uint32x4_t {
    vx::vx_unary(a, 1, 32, N)
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
#[inline(always)]
pub unsafe fn vshrq_n_s16<const N: i32>(a: int16x8_t) -> int16x8_t {
    vx::vx_unary(a, 3, 16, N)
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
#[inline(always)]
pub unsafe fn vshrq_n_s32<const N: i32>(a: int32x4_t) -> int32x4_t {
    vx::vx_unary(a, 3, 32, N)
}

pub unsafe fn vst1_u8(ptr: *mut u8, a: uint8x8_t) {
    let mut i = 0;
    while i < 8 {
        crate::ptr::write_unaligned(ptr.add(i), a.0[i]);
        i += 1;
    }
}

#[inline(always)]
pub unsafe fn vst1q_s16(a: *mut i16, b: int16x8_t) {
    vx::vx_store(a as *mut u8, b)
}

pub unsafe fn vst1q_s16_x2(a: *mut i16, b: int16x8x2_t) {
    vst1q_s16(a, b.0);
    vst1q_s16(a.add(8), b.1);
}

#[inline(always)]
pub unsafe fn vst1q_u32(a: *mut u32, b: uint32x4_t) {
    vx::vx_store(a as *mut u8, b)
}

pub unsafe fn vst3q_u8(a: *mut u8, b: uint8x16x3_t) {
    let mut i = 0;
    while i < 16 {
        crate::ptr::write_unaligned(a.add(i * 3 + 0), b.0 .0[i]);
        crate::ptr::write_unaligned(a.add(i * 3 + 1), b.1 .0[i]);
        crate::ptr::write_unaligned(a.add(i * 3 + 2), b.2 .0[i]);
        i += 1;
    }
}

pub unsafe fn vst4q_u8(a: *mut u8, b: uint8x16x4_t) {
    let mut i = 0;
    while i < 16 {
        crate::ptr::write_unaligned(a.add(i * 4 + 0), b.0 .0[i]);
        crate::ptr::write_unaligned(a.add(i * 4 + 1), b.1 .0[i]);
        crate::ptr::write_unaligned(a.add(i * 4 + 2), b.2 .0[i]);
        crate::ptr::write_unaligned(a.add(i * 4 + 3), b.3 .0[i]);
        i += 1;
    }
}

#[inline(always)]
pub unsafe fn vsubq_s16(a: int16x8_t, b: int16x8_t) -> int16x8_t {
    vx::vx_binary(a, b, 1, 16)
}

#[inline(always)]
pub unsafe fn vsubq_s32(a: int32x4_t, b: int32x4_t) -> int32x4_t {
    vx::vx_binary(a, b, 1, 32)
}

pub unsafe fn vtrn1q_s64(a: int64x2_t, b: int64x2_t) -> int64x2_t {
    int64x2_t([a.0[0], b.0[0]])
}

pub unsafe fn vtrn2q_s64(a: int64x2_t, b: int64x2_t) -> int64x2_t {
    int64x2_t([a.0[1], b.0[1]])
}

pub unsafe fn vtrnq_s32(a: int32x4_t, b: int32x4_t) -> int32x4x2_t {
    int32x4x2_t(
        int32x4_t([a.0[0], b.0[0], a.0[2], b.0[2]]),
        int32x4_t([a.0[1], b.0[1], a.0[3], b.0[3]]),
    )
}

#[inline(always)]
pub unsafe fn vzip1q_s16(a: int16x8_t, b: int16x8_t) -> int16x8_t {
    vx::vx_binary(a, b, 17, 16)
}

#[inline(always)]
pub unsafe fn vzip2q_s16(a: int16x8_t, b: int16x8_t) -> int16x8_t {
    vx::vx_binary(a, b, 18, 16)
}

// ---- SHA-256 (FEAT_SHA256), as the Arm ARM pseudocode defines SHA256H / SHA256H2 / SHA256SU0 /
// SHA256SU1 (shared/functions/crypto SHA256hash, SHAchoose, SHAmajority, SHAhashSIGMA0/1).

/// SHA256H: hash_abcd updated by four rounds.
#[inline(always)]
pub unsafe fn vsha256hq_u32(hash_abcd: uint32x4_t, hash_efgh: uint32x4_t, wk: uint32x4_t) -> uint32x4_t {
    vx::vx_ternary(hash_abcd, hash_efgh, wk, 5)
}

/// SHA256H2: hash_efgh updated by the same four rounds (hash_abcd = the pre-round abcd).
#[inline(always)]
pub unsafe fn vsha256h2q_u32(hash_efgh: uint32x4_t, hash_abcd: uint32x4_t, wk: uint32x4_t) -> uint32x4_t {
    vx::vx_ternary(hash_efgh, hash_abcd, wk, 6)
}

/// SHA256SU0: message schedule, first part.
#[inline(always)]
pub unsafe fn vsha256su0q_u32(w0_3: uint32x4_t, w4_7: uint32x4_t) -> uint32x4_t {
    vx::vx_binary(w0_3, w4_7, 81, 32)
}

/// SHA256SU1: message schedule, second part.
#[inline(always)]
pub unsafe fn vsha256su1q_u32(tw0_3: uint32x4_t, w8_11: uint32x4_t, w12_15: uint32x4_t) -> uint32x4_t {
    vx::vx_ternary(tw0_3, w8_11, w12_15, 7)
}

#[derive(Copy, Clone)]
#[repr(C, align(8))]
pub struct float32x2_t(pub [f32; 2]);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct float32x2x2_t(pub float32x2_t, pub float32x2_t);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct float32x2x3_t(pub float32x2_t, pub float32x2_t, pub float32x2_t);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct float32x2x4_t(
    pub float32x2_t,
    pub float32x2_t,
    pub float32x2_t,
    pub float32x2_t,
);

#[derive(Copy, Clone)]
#[repr(C, align(16))]
pub struct float32x4_t(pub [f32; 4]);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct float32x4x2_t(pub float32x4_t, pub float32x4_t);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct float32x4x3_t(pub float32x4_t, pub float32x4_t, pub float32x4_t);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct float32x4x4_t(
    pub float32x4_t,
    pub float32x4_t,
    pub float32x4_t,
    pub float32x4_t,
);

#[derive(Copy, Clone)]
#[repr(C, align(8))]
pub struct float64x1_t(pub [f64; 1]);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct float64x1x2_t(pub float64x1_t, pub float64x1_t);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct float64x1x3_t(pub float64x1_t, pub float64x1_t, pub float64x1_t);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct float64x1x4_t(
    pub float64x1_t,
    pub float64x1_t,
    pub float64x1_t,
    pub float64x1_t,
);

#[derive(Copy, Clone)]
#[repr(C, align(16))]
pub struct float64x2_t(pub [f64; 2]);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct float64x2x2_t(pub float64x2_t, pub float64x2_t);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct float64x2x3_t(pub float64x2_t, pub float64x2_t, pub float64x2_t);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct float64x2x4_t(
    pub float64x2_t,
    pub float64x2_t,
    pub float64x2_t,
    pub float64x2_t,
);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct int16x4x2_t(pub int16x4_t, pub int16x4_t);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct int16x4x3_t(pub int16x4_t, pub int16x4_t, pub int16x4_t);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct int16x4x4_t(pub int16x4_t, pub int16x4_t, pub int16x4_t, pub int16x4_t);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct int16x8x3_t(pub int16x8_t, pub int16x8_t, pub int16x8_t);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct int16x8x4_t(pub int16x8_t, pub int16x8_t, pub int16x8_t, pub int16x8_t);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct int32x2x2_t(pub int32x2_t, pub int32x2_t);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct int32x2x3_t(pub int32x2_t, pub int32x2_t, pub int32x2_t);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct int32x2x4_t(pub int32x2_t, pub int32x2_t, pub int32x2_t, pub int32x2_t);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct int32x4x3_t(pub int32x4_t, pub int32x4_t, pub int32x4_t);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct int32x4x4_t(pub int32x4_t, pub int32x4_t, pub int32x4_t, pub int32x4_t);

#[derive(Copy, Clone)]
#[repr(C, align(8))]
pub struct int64x1_t(pub [i64; 1]);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct int64x1x2_t(pub int64x1_t, pub int64x1_t);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct int64x1x3_t(pub int64x1_t, pub int64x1_t, pub int64x1_t);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct int64x1x4_t(pub int64x1_t, pub int64x1_t, pub int64x1_t, pub int64x1_t);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct int64x2x2_t(pub int64x2_t, pub int64x2_t);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct int64x2x3_t(pub int64x2_t, pub int64x2_t, pub int64x2_t);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct int64x2x4_t(pub int64x2_t, pub int64x2_t, pub int64x2_t, pub int64x2_t);

#[derive(Copy, Clone)]
#[repr(C, align(16))]
pub struct int8x16_t(pub [i8; 16]);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct int8x16x2_t(pub int8x16_t, pub int8x16_t);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct int8x16x3_t(pub int8x16_t, pub int8x16_t, pub int8x16_t);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct int8x16x4_t(pub int8x16_t, pub int8x16_t, pub int8x16_t, pub int8x16_t);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct int8x8x2_t(pub int8x8_t, pub int8x8_t);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct int8x8x3_t(pub int8x8_t, pub int8x8_t, pub int8x8_t);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct int8x8x4_t(pub int8x8_t, pub int8x8_t, pub int8x8_t, pub int8x8_t);

#[derive(Copy, Clone)]
#[repr(C, align(8))]
pub struct poly16x4_t(pub [u16; 4]);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct poly16x4x2_t(pub poly16x4_t, pub poly16x4_t);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct poly16x4x3_t(pub poly16x4_t, pub poly16x4_t, pub poly16x4_t);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct poly16x4x4_t(
    pub poly16x4_t,
    pub poly16x4_t,
    pub poly16x4_t,
    pub poly16x4_t,
);

#[derive(Copy, Clone)]
#[repr(C, align(16))]
pub struct poly16x8_t(pub [u16; 8]);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct poly16x8x2_t(pub poly16x8_t, pub poly16x8_t);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct poly16x8x3_t(pub poly16x8_t, pub poly16x8_t, pub poly16x8_t);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct poly16x8x4_t(
    pub poly16x8_t,
    pub poly16x8_t,
    pub poly16x8_t,
    pub poly16x8_t,
);

#[derive(Copy, Clone)]
#[repr(C, align(8))]
pub struct poly64x1_t(pub [u64; 1]);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct poly64x1x2_t(pub poly64x1_t, pub poly64x1_t);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct poly64x1x3_t(pub poly64x1_t, pub poly64x1_t, pub poly64x1_t);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct poly64x1x4_t(
    pub poly64x1_t,
    pub poly64x1_t,
    pub poly64x1_t,
    pub poly64x1_t,
);

#[derive(Copy, Clone)]
#[repr(C, align(16))]
pub struct poly64x2_t(pub [u64; 2]);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct poly64x2x2_t(pub poly64x2_t, pub poly64x2_t);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct poly64x2x3_t(pub poly64x2_t, pub poly64x2_t, pub poly64x2_t);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct poly64x2x4_t(
    pub poly64x2_t,
    pub poly64x2_t,
    pub poly64x2_t,
    pub poly64x2_t,
);

#[derive(Copy, Clone)]
#[repr(C, align(16))]
pub struct poly8x16_t(pub [u8; 16]);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct poly8x16x2_t(pub poly8x16_t, pub poly8x16_t);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct poly8x16x3_t(pub poly8x16_t, pub poly8x16_t, pub poly8x16_t);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct poly8x16x4_t(
    pub poly8x16_t,
    pub poly8x16_t,
    pub poly8x16_t,
    pub poly8x16_t,
);

#[derive(Copy, Clone)]
#[repr(C, align(8))]
pub struct poly8x8_t(pub [u8; 8]);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct poly8x8x2_t(pub poly8x8_t, pub poly8x8_t);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct poly8x8x3_t(pub poly8x8_t, pub poly8x8_t, pub poly8x8_t);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct poly8x8x4_t(pub poly8x8_t, pub poly8x8_t, pub poly8x8_t, pub poly8x8_t);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct uint16x4x2_t(pub uint16x4_t, pub uint16x4_t);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct uint16x4x3_t(pub uint16x4_t, pub uint16x4_t, pub uint16x4_t);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct uint16x4x4_t(
    pub uint16x4_t,
    pub uint16x4_t,
    pub uint16x4_t,
    pub uint16x4_t,
);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct uint16x8x2_t(pub uint16x8_t, pub uint16x8_t);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct uint16x8x3_t(pub uint16x8_t, pub uint16x8_t, pub uint16x8_t);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct uint16x8x4_t(
    pub uint16x8_t,
    pub uint16x8_t,
    pub uint16x8_t,
    pub uint16x8_t,
);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct uint32x2x2_t(pub uint32x2_t, pub uint32x2_t);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct uint32x2x3_t(pub uint32x2_t, pub uint32x2_t, pub uint32x2_t);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct uint32x2x4_t(
    pub uint32x2_t,
    pub uint32x2_t,
    pub uint32x2_t,
    pub uint32x2_t,
);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct uint32x4x2_t(pub uint32x4_t, pub uint32x4_t);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct uint32x4x3_t(pub uint32x4_t, pub uint32x4_t, pub uint32x4_t);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct uint32x4x4_t(
    pub uint32x4_t,
    pub uint32x4_t,
    pub uint32x4_t,
    pub uint32x4_t,
);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct uint64x1x2_t(pub uint64x1_t, pub uint64x1_t);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct uint64x1x3_t(pub uint64x1_t, pub uint64x1_t, pub uint64x1_t);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct uint64x1x4_t(
    pub uint64x1_t,
    pub uint64x1_t,
    pub uint64x1_t,
    pub uint64x1_t,
);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct uint64x2x2_t(pub uint64x2_t, pub uint64x2_t);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct uint64x2x3_t(pub uint64x2_t, pub uint64x2_t, pub uint64x2_t);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct uint64x2x4_t(
    pub uint64x2_t,
    pub uint64x2_t,
    pub uint64x2_t,
    pub uint64x2_t,
);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct uint8x16x2_t(pub uint8x16_t, pub uint8x16_t);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct uint8x8x2_t(pub uint8x8_t, pub uint8x8_t);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct uint8x8x3_t(pub uint8x8_t, pub uint8x8_t, pub uint8x8_t);

#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct uint8x8x4_t(pub uint8x8_t, pub uint8x8_t, pub uint8x8_t, pub uint8x8_t);

// Arm selects signaling NaNs before quiet NaNs, in operand order.
fn neon_snan_f32(a: f32) -> bool {
    a.is_nan() && a.to_bits() & 0x0040_0000 == 0
}

fn neon_quiet_f32(a: f32) -> f32 {
    f32::from_bits(a.to_bits() | 0x0040_0000)
}

fn neon_result_f32(a: f32, b: f32, result: f32) -> f32 {
    if neon_snan_f32(a) {
        neon_quiet_f32(a)
    } else if neon_snan_f32(b) {
        neon_quiet_f32(b)
    } else if a.is_nan() {
        a
    } else if b.is_nan() {
        b
    } else if result.is_nan() {
        f32::from_bits(0x7fc0_0000)
    } else {
        result
    }
}

fn neon_add_f32(a: f32, b: f32) -> f32 {
    neon_result_f32(a, b, a + b)
}

fn neon_mul_f32(a: f32, b: f32) -> f32 {
    neon_result_f32(a, b, a * b)
}

fn neon_fma_f32(a: f32, b: f32, c: f32) -> f32 {
    let result = b.mul_add(c, a);
    if neon_snan_f32(a) {
        neon_quiet_f32(a)
    } else if neon_snan_f32(b) {
        neon_quiet_f32(b)
    } else if neon_snan_f32(c) {
        neon_quiet_f32(c)
    } else if (b.is_infinite() && c == 0.0) || (c.is_infinite() && b == 0.0) {
        f32::from_bits(0x7fc0_0000)
    } else if a.is_nan() {
        a
    } else if b.is_nan() {
        b
    } else if c.is_nan() {
        c
    } else if result.is_nan() {
        f32::from_bits(0x7fc0_0000)
    } else {
        result
    }
}

fn neon_minmax_f32(a: f32, b: f32, maximum: bool, number: bool) -> f32 {
    if neon_snan_f32(a) {
        return neon_quiet_f32(a);
    }
    if neon_snan_f32(b) {
        return neon_quiet_f32(b);
    }
    if number {
        if a.is_nan() && !b.is_nan() {
            return b;
        }
        if b.is_nan() && !a.is_nan() {
            return a;
        }
    }
    if a.is_nan() {
        return a;
    }
    if b.is_nan() {
        return b;
    }
    if a == b {
        return f32::from_bits(if maximum {
            a.to_bits() & b.to_bits()
        } else {
            a.to_bits() | b.to_bits()
        });
    }
    if (a > b) == maximum {
        a
    } else {
        b
    }
}

// Arm selects signaling NaNs before quiet NaNs, in operand order.
fn neon_snan_f64(a: f64) -> bool {
    a.is_nan() && a.to_bits() & 0x0008_0000_0000_0000 == 0
}

fn neon_quiet_f64(a: f64) -> f64 {
    f64::from_bits(a.to_bits() | 0x0008_0000_0000_0000)
}

fn neon_result_f64(a: f64, b: f64, result: f64) -> f64 {
    if neon_snan_f64(a) {
        neon_quiet_f64(a)
    } else if neon_snan_f64(b) {
        neon_quiet_f64(b)
    } else if a.is_nan() {
        a
    } else if b.is_nan() {
        b
    } else if result.is_nan() {
        f64::from_bits(0x7ff8_0000_0000_0000)
    } else {
        result
    }
}

fn neon_fma_f64(a: f64, b: f64, c: f64) -> f64 {
    let result = b.mul_add(c, a);
    if neon_snan_f64(a) {
        neon_quiet_f64(a)
    } else if neon_snan_f64(b) {
        neon_quiet_f64(b)
    } else if neon_snan_f64(c) {
        neon_quiet_f64(c)
    } else if (b.is_infinite() && c == 0.0) || (c.is_infinite() && b == 0.0) {
        f64::from_bits(0x7ff8_0000_0000_0000)
    } else if a.is_nan() {
        a
    } else if b.is_nan() {
        b
    } else if c.is_nan() {
        c
    } else if result.is_nan() {
        f64::from_bits(0x7ff8_0000_0000_0000)
    } else {
        result
    }
}

fn neon_minmax_f64(a: f64, b: f64, maximum: bool, number: bool) -> f64 {
    if neon_snan_f64(a) {
        return neon_quiet_f64(a);
    }
    if neon_snan_f64(b) {
        return neon_quiet_f64(b);
    }
    if number {
        if a.is_nan() && !b.is_nan() {
            return b;
        }
        if b.is_nan() && !a.is_nan() {
            return a;
        }
    }
    if a.is_nan() {
        return a;
    }
    if b.is_nan() {
        return b;
    }
    if a == b {
        return f64::from_bits(if maximum {
            a.to_bits() & b.to_bits()
        } else {
            a.to_bits() | b.to_bits()
        });
    }
    if (a > b) == maximum {
        a
    } else {
        b
    }
}

pub unsafe fn vabsq_f32(a: float32x4_t) -> float32x4_t {
    vx::vx_unary(a, 8, 32, 0)
}

pub unsafe fn vabsq_f64(a: float64x2_t) -> float64x2_t {
    let mut r = [0.0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = f64::from_bits(a.0[i].to_bits() & 0x7fffffffffffffff);
        i += 1;
    }
    float64x2_t(r)
}

#[inline(always)]
pub unsafe fn vabsq_s16(a: int16x8_t) -> int16x8_t {
    vx::vx_unary(a, 0, 16, 0)
}

#[inline(always)]
pub unsafe fn vabsq_s32(a: int32x4_t) -> int32x4_t {
    vx::vx_unary(a, 0, 32, 0)
}

#[inline(always)]
pub unsafe fn vabsq_s64(a: int64x2_t) -> int64x2_t {
    vx::vx_unary(a, 0, 64, 0)
}

#[inline(always)]
pub unsafe fn vabsq_s8(a: int8x16_t) -> int8x16_t {
    vx::vx_unary(a, 0, 8, 0)
}

#[inline(always)]
pub unsafe fn vaddq_f32(a: float32x4_t, b: float32x4_t) -> float32x4_t {
    vx::vx_binary(a, b, 60, 32)
}

pub unsafe fn vaddq_f64(a: float64x2_t, b: float64x2_t) -> float64x2_t {
    let mut r = [0.0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = neon_result_f64(a.0[i], b.0[i], a.0[i] + b.0[i]);
        i += 1;
    }
    float64x2_t(r)
}

#[inline(always)]
pub unsafe fn vaddq_s64(a: int64x2_t, b: int64x2_t) -> int64x2_t {
    vx::vx_binary(a, b, 0, 64)
}

#[inline(always)]
pub unsafe fn vaddq_s8(a: int8x16_t, b: int8x16_t) -> int8x16_t {
    vx::vx_binary(a, b, 0, 8)
}

#[inline(always)]
pub unsafe fn vaddq_u16(a: uint16x8_t, b: uint16x8_t) -> uint16x8_t {
    vx::vx_binary(a, b, 0, 16)
}

#[inline(always)]
pub unsafe fn vaddq_u64(a: uint64x2_t, b: uint64x2_t) -> uint64x2_t {
    vx::vx_binary(a, b, 0, 64)
}

#[inline(always)]
pub unsafe fn vaddq_u8(a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    vx::vx_binary(a, b, 0, 8)
}

pub unsafe fn vaddvq_f32(a: float32x4_t) -> f32 {
    neon_add_f32(neon_add_f32(a.0[0], a.0[1]), neon_add_f32(a.0[2], a.0[3]))
}

pub unsafe fn vaddvq_s16(a: int16x8_t) -> i16 {
    let mut r = a.0[0];
    let mut i = 1;
    while i < 8 {
        r = r.wrapping_add(a.0[i]);
        i += 1;
    }
    r
}

pub unsafe fn vaddvq_s32(a: int32x4_t) -> i32 {
    let mut r = a.0[0];
    let mut i = 1;
    while i < 4 {
        r = r.wrapping_add(a.0[i]);
        i += 1;
    }
    r
}

pub unsafe fn vaddvq_u16(a: uint16x8_t) -> u16 {
    let mut r = a.0[0];
    let mut i = 1;
    while i < 8 {
        r = r.wrapping_add(a.0[i]);
        i += 1;
    }
    r
}

#[inline(always)]
pub unsafe fn vandq_s16(a: int16x8_t, b: int16x8_t) -> int16x8_t {
    vx::vx_binary(a, b, 6, 16)
}

#[inline(always)]
pub unsafe fn vandq_s32(a: int32x4_t, b: int32x4_t) -> int32x4_t {
    vx::vx_binary(a, b, 6, 32)
}

#[inline(always)]
pub unsafe fn vandq_s64(a: int64x2_t, b: int64x2_t) -> int64x2_t {
    vx::vx_binary(a, b, 6, 64)
}

#[inline(always)]
pub unsafe fn vandq_s8(a: int8x16_t, b: int8x16_t) -> int8x16_t {
    vx::vx_binary(a, b, 6, 8)
}

#[inline(always)]
pub unsafe fn vandq_u16(a: uint16x8_t, b: uint16x8_t) -> uint16x8_t {
    vx::vx_binary(a, b, 6, 16)
}

#[inline(always)]
pub unsafe fn vandq_u32(a: uint32x4_t, b: uint32x4_t) -> uint32x4_t {
    vx::vx_binary(a, b, 6, 32)
}

#[inline(always)]
pub unsafe fn vandq_u64(a: uint64x2_t, b: uint64x2_t) -> uint64x2_t {
    vx::vx_binary(a, b, 6, 64)
}

#[inline(always)]
pub unsafe fn vandq_u8(a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    vx::vx_binary(a, b, 6, 8)
}

#[inline(always)]
pub unsafe fn vbicq_u32(a: uint32x4_t, b: uint32x4_t) -> uint32x4_t {
    vx::vx_binary(a, b, 9, 32)
}

pub unsafe fn vbslq_f32(a: uint32x4_t, b: float32x4_t, c: float32x4_t) -> float32x4_t {
    let mut r = [0.0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = f32::from_bits((a.0[i] & b.0[i].to_bits()) | (!a.0[i] & c.0[i].to_bits()));
        i += 1;
    }
    float32x4_t(r)
}

pub unsafe fn vbslq_s16(a: uint16x8_t, b: int16x8_t, c: int16x8_t) -> int16x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = ((a.0[i] & b.0[i] as u16) | (!a.0[i] & c.0[i] as u16)) as i16;
        i += 1;
    }
    int16x8_t(r)
}

pub unsafe fn vbslq_s32(a: uint32x4_t, b: int32x4_t, c: int32x4_t) -> int32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = ((a.0[i] & b.0[i] as u32) | (!a.0[i] & c.0[i] as u32)) as i32;
        i += 1;
    }
    int32x4_t(r)
}

pub unsafe fn vbslq_s64(a: uint64x2_t, b: int64x2_t, c: int64x2_t) -> int64x2_t {
    let mut r = [0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = ((a.0[i] & b.0[i] as u64) | (!a.0[i] & c.0[i] as u64)) as i64;
        i += 1;
    }
    int64x2_t(r)
}

pub unsafe fn vbslq_s8(a: uint8x16_t, b: int8x16_t, c: int8x16_t) -> int8x16_t {
    let mut r = [0; 16];
    let mut i = 0;
    while i < 16 {
        r[i] = ((a.0[i] & b.0[i] as u8) | (!a.0[i] & c.0[i] as u8)) as i8;
        i += 1;
    }
    int8x16_t(r)
}

pub unsafe fn vbslq_u16(a: uint16x8_t, b: uint16x8_t, c: uint16x8_t) -> uint16x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = ((a.0[i] & b.0[i] as u16) | (!a.0[i] & c.0[i] as u16)) as u16;
        i += 1;
    }
    uint16x8_t(r)
}

pub unsafe fn vbslq_u32(a: uint32x4_t, b: uint32x4_t, c: uint32x4_t) -> uint32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = ((a.0[i] & b.0[i] as u32) | (!a.0[i] & c.0[i] as u32)) as u32;
        i += 1;
    }
    uint32x4_t(r)
}

pub unsafe fn vbslq_u64(a: uint64x2_t, b: uint64x2_t, c: uint64x2_t) -> uint64x2_t {
    let mut r = [0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = ((a.0[i] & b.0[i] as u64) | (!a.0[i] & c.0[i] as u64)) as u64;
        i += 1;
    }
    uint64x2_t(r)
}

pub unsafe fn vbslq_u8(a: uint8x16_t, b: uint8x16_t, c: uint8x16_t) -> uint8x16_t {
    let mut r = [0; 16];
    let mut i = 0;
    while i < 16 {
        r[i] = ((a.0[i] & b.0[i] as u8) | (!a.0[i] & c.0[i] as u8)) as u8;
        i += 1;
    }
    uint8x16_t(r)
}

#[inline(always)]
pub unsafe fn vceqq_f32(a: float32x4_t, b: float32x4_t) -> uint32x4_t {
    vx::vx_binary(a, b, 46, 32)
}

pub unsafe fn vceqq_f64(a: float64x2_t, b: float64x2_t) -> uint64x2_t {
    let mut r = [0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = if a.0[i] == b.0[i] { u64::MAX } else { 0 };
        i += 1;
    }
    uint64x2_t(r)
}

#[inline(always)]
pub unsafe fn vceqq_s16(a: int16x8_t, b: int16x8_t) -> uint16x8_t {
    vx::vx_binary(a, b, 10, 16)
}

#[inline(always)]
pub unsafe fn vceqq_s32(a: int32x4_t, b: int32x4_t) -> uint32x4_t {
    vx::vx_binary(a, b, 10, 32)
}

#[inline(always)]
pub unsafe fn vceqq_s64(a: int64x2_t, b: int64x2_t) -> uint64x2_t {
    vx::vx_binary(a, b, 10, 64)
}

#[inline(always)]
pub unsafe fn vceqq_s8(a: int8x16_t, b: int8x16_t) -> uint8x16_t {
    vx::vx_binary(a, b, 10, 8)
}

#[inline(always)]
pub unsafe fn vceqq_u16(a: uint16x8_t, b: uint16x8_t) -> uint16x8_t {
    vx::vx_binary(a, b, 10, 16)
}

#[inline(always)]
pub unsafe fn vceqq_u32(a: uint32x4_t, b: uint32x4_t) -> uint32x4_t {
    vx::vx_binary(a, b, 10, 32)
}

#[inline(always)]
pub unsafe fn vceqq_u64(a: uint64x2_t, b: uint64x2_t) -> uint64x2_t {
    vx::vx_binary(a, b, 10, 64)
}

#[inline(always)]
pub unsafe fn vceqq_u8(a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    vx::vx_binary(a, b, 10, 8)
}

pub unsafe fn vcgeq_f32(a: float32x4_t, b: float32x4_t) -> uint32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = if a.0[i] >= b.0[i] { u32::MAX } else { 0 };
        i += 1;
    }
    uint32x4_t(r)
}

pub unsafe fn vcgeq_f64(a: float64x2_t, b: float64x2_t) -> uint64x2_t {
    let mut r = [0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = if a.0[i] >= b.0[i] { u64::MAX } else { 0 };
        i += 1;
    }
    uint64x2_t(r)
}

#[inline(always)]
pub unsafe fn vcgtq_f32(a: float32x4_t, b: float32x4_t) -> uint32x4_t {
    vx::vx_binary(a, b, 50, 32)
}

pub unsafe fn vcgtq_f64(a: float64x2_t, b: float64x2_t) -> uint64x2_t {
    let mut r = [0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = if a.0[i] > b.0[i] { u64::MAX } else { 0 };
        i += 1;
    }
    uint64x2_t(r)
}

#[inline(always)]
pub unsafe fn vcgtq_s16(a: int16x8_t, b: int16x8_t) -> uint16x8_t {
    vx::vx_binary(a, b, 11, 16)
}

#[inline(always)]
pub unsafe fn vcgtq_s32(a: int32x4_t, b: int32x4_t) -> uint32x4_t {
    vx::vx_binary(a, b, 11, 32)
}

#[inline(always)]
pub unsafe fn vcgtq_s64(a: int64x2_t, b: int64x2_t) -> uint64x2_t {
    vx::vx_binary(a, b, 11, 64)
}

#[inline(always)]
pub unsafe fn vcgtq_s8(a: int8x16_t, b: int8x16_t) -> uint8x16_t {
    vx::vx_binary(a, b, 11, 8)
}

#[inline(always)]
pub unsafe fn vcgtq_u32(a: uint32x4_t, b: uint32x4_t) -> uint32x4_t {
    vx::vx_binary(a, b, 29, 32)
}

#[inline(always)]
pub unsafe fn vcgtq_u64(a: uint64x2_t, b: uint64x2_t) -> uint64x2_t {
    vx::vx_binary(a, b, 29, 64)
}

pub unsafe fn vcleq_f32(a: float32x4_t, b: float32x4_t) -> uint32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = if a.0[i] <= b.0[i] { u32::MAX } else { 0 };
        i += 1;
    }
    uint32x4_t(r)
}

pub unsafe fn vcleq_f64(a: float64x2_t, b: float64x2_t) -> uint64x2_t {
    let mut r = [0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = if a.0[i] <= b.0[i] { u64::MAX } else { 0 };
        i += 1;
    }
    uint64x2_t(r)
}

#[inline(always)]
pub unsafe fn vcltq_f32(a: float32x4_t, b: float32x4_t) -> uint32x4_t {
    vx::vx_binary(a, b, 48, 32)
}

pub unsafe fn vcltq_f64(a: float64x2_t, b: float64x2_t) -> uint64x2_t {
    let mut r = [0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = if a.0[i] < b.0[i] { u64::MAX } else { 0 };
        i += 1;
    }
    uint64x2_t(r)
}

#[inline(always)]
pub unsafe fn vcltq_s16(a: int16x8_t, b: int16x8_t) -> uint16x8_t {
    vx::vx_binary(b, a, 11, 16)
}

#[inline(always)]
pub unsafe fn vcltq_s32(a: int32x4_t, b: int32x4_t) -> uint32x4_t {
    vx::vx_binary(b, a, 11, 32)
}

#[inline(always)]
pub unsafe fn vcltq_s64(a: int64x2_t, b: int64x2_t) -> uint64x2_t {
    vx::vx_binary(b, a, 11, 64)
}

#[inline(always)]
pub unsafe fn vcltq_s8(a: int8x16_t, b: int8x16_t) -> uint8x16_t {
    vx::vx_binary(b, a, 11, 8)
}

pub unsafe fn vcombine_s8(a: int8x8_t, b: int8x8_t) -> int8x16_t {
    let mut r = [0; 16];
    let mut i = 0;
    while i < 16 {
        r[i] = if i < 8 { a.0[i] } else { b.0[i - 8] };
        i += 1;
    }
    int8x16_t(r)
}

pub unsafe fn vcombine_u32(a: uint32x2_t, b: uint32x2_t) -> uint32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = if i < 2 { a.0[i] } else { b.0[i - 2] };
        i += 1;
    }
    uint32x4_t(r)
}

pub unsafe fn vcvtnq_s32_f32(a: float32x4_t) -> int32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = a.0[i].round_ties_even() as i32;
        i += 1;
    }
    int32x4_t(r)
}

pub unsafe fn vcvtq_f32_s32(a: int32x4_t) -> float32x4_t {
    let mut r = [0.0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = a.0[i] as f32;
        i += 1;
    }
    float32x4_t(r)
}

pub unsafe fn vcvtq_f64_s64(a: int64x2_t) -> float64x2_t {
    let mut r = [0.0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = a.0[i] as f64;
        i += 1;
    }
    float64x2_t(r)
}

pub unsafe fn vcvtq_s32_f32(a: float32x4_t) -> int32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = a.0[i] as i32;
        i += 1;
    }
    int32x4_t(r)
}

#[inline(always)]
pub unsafe fn vdivq_f32(a: float32x4_t, b: float32x4_t) -> float32x4_t {
    vx::vx_binary(a, b, 63, 32)
}

pub unsafe fn vdivq_f64(a: float64x2_t, b: float64x2_t) -> float64x2_t {
    let mut r = [0.0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = neon_result_f64(a.0[i], b.0[i], a.0[i] / b.0[i]);
        i += 1;
    }
    float64x2_t(r)
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
pub unsafe fn vdupq_laneq_f32<const N: i32>(a: float32x4_t) -> float32x4_t {
    float32x4_t([a.0[N as usize]; 4])
}

#[inline(always)]
pub unsafe fn vdupq_n_f32(value: f32) -> float32x4_t {
    vx::vx_splat(value.to_bits() as u64, 32)
}

#[inline(always)]
pub unsafe fn vdupq_n_f64(value: f64) -> float64x2_t {
    vx::vx_splat(value.to_bits() as u64, 64)
}

#[inline(always)]
pub unsafe fn vdupq_n_s64(value: i64) -> int64x2_t {
    vx::vx_splat(value as u64, 64)
}

#[inline(always)]
pub unsafe fn vdupq_n_s8(value: i8) -> int8x16_t {
    vx::vx_splat(value as u64, 8)
}

#[inline(always)]
pub unsafe fn vdupq_n_u64(value: u64) -> uint64x2_t {
    vx::vx_splat(value as u64, 64)
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
pub unsafe fn vdups_laneq_f32<const N: i32>(a: float32x4_t) -> f32 {
    a.0[N as usize]
}

#[inline(always)]
pub unsafe fn veorq_s16(a: int16x8_t, b: int16x8_t) -> int16x8_t {
    vx::vx_binary(a, b, 8, 16)
}

#[inline(always)]
pub unsafe fn veorq_s32(a: int32x4_t, b: int32x4_t) -> int32x4_t {
    vx::vx_binary(a, b, 8, 32)
}

#[inline(always)]
pub unsafe fn veorq_s64(a: int64x2_t, b: int64x2_t) -> int64x2_t {
    vx::vx_binary(a, b, 8, 64)
}

#[inline(always)]
pub unsafe fn veorq_s8(a: int8x16_t, b: int8x16_t) -> int8x16_t {
    vx::vx_binary(a, b, 8, 8)
}

#[inline(always)]
pub unsafe fn veorq_u16(a: uint16x8_t, b: uint16x8_t) -> uint16x8_t {
    vx::vx_binary(a, b, 8, 16)
}

#[inline(always)]
pub unsafe fn veorq_u32(a: uint32x4_t, b: uint32x4_t) -> uint32x4_t {
    vx::vx_binary(a, b, 8, 32)
}

#[inline(always)]
pub unsafe fn veorq_u64(a: uint64x2_t, b: uint64x2_t) -> uint64x2_t {
    vx::vx_binary(a, b, 8, 64)
}

#[inline(always)]
pub unsafe fn veorq_u8(a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    vx::vx_binary(a, b, 8, 8)
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(2))]
pub unsafe fn vextq_f32<const N: i32>(a: float32x4_t, b: float32x4_t) -> float32x4_t {
    let mut r = [0.0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = if i + (N as usize) < 4 {
            a.0[i + N as usize]
        } else {
            b.0[i + N as usize - 4]
        };
        i += 1;
    }
    float32x4_t(r)
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(2))]
pub unsafe fn vextq_u32<const N: i32>(a: uint32x4_t, b: uint32x4_t) -> uint32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = if i + (N as usize) < 4 {
            a.0[i + N as usize]
        } else {
            b.0[i + N as usize - 4]
        };
        i += 1;
    }
    uint32x4_t(r)
}

// a + b * c: stdarch's full-vector FMLA operand order; indexed FMLA uses b, c.
#[inline(always)]
pub unsafe fn vfmaq_f32(a: float32x4_t, b: float32x4_t, c: float32x4_t) -> float32x4_t {
    vx::vx_ternary(a, b, c, 8)
}

pub unsafe fn vfmaq_f64(a: float64x2_t, b: float64x2_t, c: float64x2_t) -> float64x2_t {
    let mut r = [0.0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = neon_fma_f64(a.0[i], c.0[i], b.0[i]);
        i += 1;
    }
    float64x2_t(r)
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(3))]
pub unsafe fn vfmaq_laneq_f32<const LANE: i32>(
    a: float32x4_t,
    b: float32x4_t,
    c: float32x4_t,
) -> float32x4_t {
    let mut r = [0.0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = neon_fma_f32(a.0[i], b.0[i], c.0[LANE as usize]);
        i += 1;
    }
    float32x4_t(r)
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(3))]
pub unsafe fn vfmaq_laneq_f64<const LANE: i32>(
    a: float64x2_t,
    b: float64x2_t,
    c: float64x2_t,
) -> float64x2_t {
    let mut r = [0.0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = neon_fma_f64(a.0[i], b.0[i], c.0[LANE as usize]);
        i += 1;
    }
    float64x2_t(r)
}

// a - b * c: stdarch's full-vector FMLS operand order.
#[inline(always)]
pub unsafe fn vfmsq_f32(a: float32x4_t, b: float32x4_t, c: float32x4_t) -> float32x4_t {
    vx::vx_ternary(a, b, c, 9)
}

pub unsafe fn vget_high_s16(a: int16x8_t) -> int16x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = a.0[i + 4];
        i += 1;
    }
    int16x4_t(r)
}

pub unsafe fn vget_high_s32(a: int32x4_t) -> int32x2_t {
    let mut r = [0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = a.0[i + 2];
        i += 1;
    }
    int32x2_t(r)
}

pub unsafe fn vget_high_s8(a: int8x16_t) -> int8x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = a.0[i + 8];
        i += 1;
    }
    int8x8_t(r)
}

pub unsafe fn vget_low_s32(a: int32x4_t) -> int32x2_t {
    let mut r = [0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = a.0[i + 0];
        i += 1;
    }
    int32x2_t(r)
}

pub unsafe fn vget_low_s8(a: int8x16_t) -> int8x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = a.0[i + 0];
        i += 1;
    }
    int8x8_t(r)
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
pub unsafe fn vgetq_lane_f32<const IMM5: i32>(v: float32x4_t) -> f32 {
    v.0[IMM5 as usize]
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
pub unsafe fn vgetq_lane_f64<const IMM5: i32>(a: float64x2_t) -> f64 {
    a.0[IMM5 as usize]
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
pub unsafe fn vgetq_lane_s64<const IMM5: i32>(v: int64x2_t) -> i64 {
    v.0[IMM5 as usize]
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
pub unsafe fn vgetq_lane_u64<const IMM5: i32>(v: uint64x2_t) -> u64 {
    v.0[IMM5 as usize]
}

pub unsafe fn vld1_s8(ptr: *const i8) -> int8x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = crate::ptr::read_unaligned(ptr.add(i));
        i += 1;
    }
    int8x8_t(r)
}

pub unsafe fn vld1q_dup_f32(ptr: *const f32) -> float32x4_t {
    float32x4_t([crate::ptr::read_unaligned(ptr); 4])
}

pub unsafe fn vld1q_dup_f64(ptr: *const f64) -> float64x2_t {
    float64x2_t([crate::ptr::read_unaligned(ptr); 2])
}

#[inline(always)]
pub unsafe fn vld1q_f32(ptr: *const f32) -> float32x4_t {
    vx::vx_load(ptr as *const u8)
}

#[inline(always)]
pub unsafe fn vld1q_f64(ptr: *const f64) -> float64x2_t {
    vx::vx_load(ptr as *const u8)
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(2))]
pub unsafe fn vld1q_lane_f32<const LANE: i32>(ptr: *const f32, src: float32x4_t) -> float32x4_t {
    let mut r = src;
    r.0[LANE as usize] = crate::ptr::read_unaligned(ptr);
    r
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(2))]
pub unsafe fn vld1q_lane_f64<const LANE: i32>(ptr: *const f64, src: float64x2_t) -> float64x2_t {
    let mut r = src;
    r.0[LANE as usize] = crate::ptr::read_unaligned(ptr);
    r
}

#[inline(always)]
pub unsafe fn vld1q_s32(a: *const i32) -> int32x4_t {
    vx::vx_load(a as *const u8)
}

#[inline(always)]
pub unsafe fn vld1q_s8(a: *const i8) -> int8x16_t {
    vx::vx_load(a as *const u8)
}

pub unsafe fn vmaxnmq_f32(a: float32x4_t, b: float32x4_t) -> float32x4_t {
    let mut r = [0.0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = neon_minmax_f32(a.0[i], b.0[i], true, true);
        i += 1;
    }
    float32x4_t(r)
}

pub unsafe fn vmaxnmq_f64(a: float64x2_t, b: float64x2_t) -> float64x2_t {
    let mut r = [0.0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = neon_minmax_f64(a.0[i], b.0[i], true, true);
        i += 1;
    }
    float64x2_t(r)
}

pub unsafe fn vmaxnmvq_f32(a: float32x4_t) -> f32 {
    neon_minmax_f32(
        neon_minmax_f32(a.0[0], a.0[1], true, true),
        neon_minmax_f32(a.0[2], a.0[3], true, true),
        true,
        true,
    )
}

pub unsafe fn vmaxq_f32(a: float32x4_t, b: float32x4_t) -> float32x4_t {
    vx::vx_binary(a, b, 53, 32)
}

pub unsafe fn vmaxq_f64(a: float64x2_t, b: float64x2_t) -> float64x2_t {
    let mut r = [0.0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = neon_minmax_f64(a.0[i], b.0[i], true, false);
        i += 1;
    }
    float64x2_t(r)
}

#[inline(always)]
pub unsafe fn vmaxq_s8(a: int8x16_t, b: int8x16_t) -> int8x16_t {
    vx::vx_binary(a, b, 14, 8)
}

#[inline(always)]
pub unsafe fn vmaxq_u16(a: uint16x8_t, b: uint16x8_t) -> uint16x8_t {
    vx::vx_binary(a, b, 15, 16)
}

#[inline(always)]
pub unsafe fn vmaxq_u32(a: uint32x4_t, b: uint32x4_t) -> uint32x4_t {
    vx::vx_binary(a, b, 15, 32)
}

#[inline(always)]
pub unsafe fn vmaxq_u8(a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    vx::vx_binary(a, b, 15, 8)
}

pub unsafe fn vmaxvq_s16(a: int16x8_t) -> i16 {
    let mut r = a.0[0];
    let mut i = 1;
    while i < 8 {
        r = r.max(a.0[i]);
        i += 1;
    }
    r
}

pub unsafe fn vmaxvq_s32(a: int32x4_t) -> i32 {
    let mut r = a.0[0];
    let mut i = 1;
    while i < 4 {
        r = r.max(a.0[i]);
        i += 1;
    }
    r
}

pub unsafe fn vmaxvq_s8(a: int8x16_t) -> i8 {
    let mut r = a.0[0];
    let mut i = 1;
    while i < 16 {
        r = r.max(a.0[i]);
        i += 1;
    }
    r
}

pub unsafe fn vminnmq_f32(a: float32x4_t, b: float32x4_t) -> float32x4_t {
    let mut r = [0.0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = neon_minmax_f32(a.0[i], b.0[i], false, true);
        i += 1;
    }
    float32x4_t(r)
}

pub unsafe fn vminnmq_f64(a: float64x2_t, b: float64x2_t) -> float64x2_t {
    let mut r = [0.0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = neon_minmax_f64(a.0[i], b.0[i], false, true);
        i += 1;
    }
    float64x2_t(r)
}

pub unsafe fn vminnmvq_f32(a: float32x4_t) -> f32 {
    neon_minmax_f32(
        neon_minmax_f32(a.0[0], a.0[1], false, true),
        neon_minmax_f32(a.0[2], a.0[3], false, true),
        false,
        true,
    )
}

pub unsafe fn vminq_f32(a: float32x4_t, b: float32x4_t) -> float32x4_t {
    vx::vx_binary(a, b, 52, 32)
}

pub unsafe fn vminq_f64(a: float64x2_t, b: float64x2_t) -> float64x2_t {
    let mut r = [0.0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = neon_minmax_f64(a.0[i], b.0[i], false, false);
        i += 1;
    }
    float64x2_t(r)
}

#[inline(always)]
pub unsafe fn vminq_s32(a: int32x4_t, b: int32x4_t) -> int32x4_t {
    vx::vx_binary(a, b, 12, 32)
}

#[inline(always)]
pub unsafe fn vminq_s8(a: int8x16_t, b: int8x16_t) -> int8x16_t {
    vx::vx_binary(a, b, 12, 8)
}

#[inline(always)]
pub unsafe fn vminq_u16(a: uint16x8_t, b: uint16x8_t) -> uint16x8_t {
    vx::vx_binary(a, b, 13, 16)
}

#[inline(always)]
pub unsafe fn vminq_u32(a: uint32x4_t, b: uint32x4_t) -> uint32x4_t {
    vx::vx_binary(a, b, 13, 32)
}

#[inline(always)]
pub unsafe fn vminq_u8(a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    vx::vx_binary(a, b, 13, 8)
}

pub unsafe fn vminvq_s16(a: int16x8_t) -> i16 {
    let mut r = a.0[0];
    let mut i = 1;
    while i < 8 {
        r = r.min(a.0[i]);
        i += 1;
    }
    r
}

pub unsafe fn vminvq_s32(a: int32x4_t) -> i32 {
    let mut r = a.0[0];
    let mut i = 1;
    while i < 4 {
        r = r.min(a.0[i]);
        i += 1;
    }
    r
}

pub unsafe fn vminvq_s8(a: int8x16_t) -> i8 {
    let mut r = a.0[0];
    let mut i = 1;
    while i < 16 {
        r = r.min(a.0[i]);
        i += 1;
    }
    r
}

pub unsafe fn vminvq_u16(a: uint16x8_t) -> u16 {
    let mut r = a.0[0];
    let mut i = 1;
    while i < 8 {
        r = r.min(a.0[i]);
        i += 1;
    }
    r
}

pub unsafe fn vminvq_u32(a: uint32x4_t) -> u32 {
    let mut r = a.0[0];
    let mut i = 1;
    while i < 4 {
        r = r.min(a.0[i]);
        i += 1;
    }
    r
}

pub unsafe fn vminvq_u8(a: uint8x16_t) -> u8 {
    let mut r = a.0[0];
    let mut i = 1;
    while i < 16 {
        r = r.min(a.0[i]);
        i += 1;
    }
    r
}

pub unsafe fn vmlaq_s32(a: int32x4_t, b: int32x4_t, c: int32x4_t) -> int32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = a.0[i].wrapping_add(b.0[i].wrapping_mul(c.0[i]));
        i += 1;
    }
    int32x4_t(r)
}

pub unsafe fn vmlsq_f32(a: float32x4_t, b: float32x4_t, c: float32x4_t) -> float32x4_t {
    let mut r = [0.0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = neon_result_f32(
            a.0[i],
            neon_mul_f32(b.0[i], c.0[i]),
            a.0[i] - neon_mul_f32(b.0[i], c.0[i]),
        );
        i += 1;
    }
    float32x4_t(r)
}

pub unsafe fn vmovl_s16(a: int16x4_t) -> int32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = a.0[i] as i32;
        i += 1;
    }
    int32x4_t(r)
}

pub unsafe fn vmovl_s32(a: int32x2_t) -> int64x2_t {
    let mut r = [0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = a.0[i] as i64;
        i += 1;
    }
    int64x2_t(r)
}

pub unsafe fn vmovl_s8(a: int8x8_t) -> int16x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = a.0[i] as i16;
        i += 1;
    }
    int16x8_t(r)
}

pub unsafe fn vmovq_n_f32(value: f32) -> float32x4_t {
    float32x4_t([value; 4])
}

pub unsafe fn vmovq_n_f64(value: f64) -> float64x2_t {
    float64x2_t([value; 2])
}

pub unsafe fn vmovq_n_s16(value: i16) -> int16x8_t {
    int16x8_t([value; 8])
}

pub unsafe fn vmovq_n_s32(value: i32) -> int32x4_t {
    int32x4_t([value; 4])
}

pub unsafe fn vmovq_n_s64(value: i64) -> int64x2_t {
    int64x2_t([value; 2])
}

pub unsafe fn vmovq_n_u32(value: u32) -> uint32x4_t {
    uint32x4_t([value; 4])
}

pub unsafe fn vmull_high_s16(a: int16x8_t, b: int16x8_t) -> int32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = (a.0[i + 4] as i32) * (b.0[i + 4] as i32);
        i += 1;
    }
    int32x4_t(r)
}

pub unsafe fn vmull_s16(a: int16x4_t, b: int16x4_t) -> int32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = (a.0[i] as i32) * (b.0[i] as i32);
        i += 1;
    }
    int32x4_t(r)
}

pub unsafe fn vmull_s32(a: int32x2_t, b: int32x2_t) -> int64x2_t {
    let mut r = [0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = (a.0[i] as i64) * (b.0[i] as i64);
        i += 1;
    }
    int64x2_t(r)
}

pub unsafe fn vmull_u16(a: uint16x4_t, b: uint16x4_t) -> uint32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = (a.0[i] as u32) * (b.0[i] as u32);
        i += 1;
    }
    uint32x4_t(r)
}

pub unsafe fn vmull_u32(a: uint32x2_t, b: uint32x2_t) -> uint64x2_t {
    let mut r = [0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = (a.0[i] as u64) * (b.0[i] as u64);
        i += 1;
    }
    uint64x2_t(r)
}

#[inline(always)]
pub unsafe fn vmulq_f32(a: float32x4_t, b: float32x4_t) -> float32x4_t {
    vx::vx_binary(a, b, 62, 32)
}

pub unsafe fn vmulq_f64(a: float64x2_t, b: float64x2_t) -> float64x2_t {
    let mut r = [0.0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = neon_result_f64(a.0[i], b.0[i], a.0[i] * b.0[i]);
        i += 1;
    }
    float64x2_t(r)
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(2))]
pub unsafe fn vmulq_laneq_f32<const LANE: i32>(a: float32x4_t, b: float32x4_t) -> float32x4_t {
    let mut r = [0.0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = neon_result_f32(a.0[i], b.0[LANE as usize], a.0[i] * b.0[LANE as usize]);
        i += 1;
    }
    float32x4_t(r)
}

pub unsafe fn vmulq_n_f32(a: float32x4_t, b: f32) -> float32x4_t {
    let mut r = [0.0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = neon_result_f32(a.0[i], b, a.0[i] * b);
        i += 1;
    }
    float32x4_t(r)
}

pub unsafe fn vmulq_n_f64(a: float64x2_t, b: f64) -> float64x2_t {
    let mut r = [0.0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = neon_result_f64(a.0[i], b, a.0[i] * b);
        i += 1;
    }
    float64x2_t(r)
}

#[inline(always)]
pub unsafe fn vmulq_u16(a: uint16x8_t, b: uint16x8_t) -> uint16x8_t {
    vx::vx_binary(a, b, 26, 16)
}

#[inline(always)]
pub unsafe fn vmulq_u32(a: uint32x4_t, b: uint32x4_t) -> uint32x4_t {
    vx::vx_binary(a, b, 26, 32)
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(2))]
pub unsafe fn vmuls_laneq_f32<const LANE: i32>(a: f32, b: float32x4_t) -> f32 {
    neon_mul_f32(a, b.0[LANE as usize])
}

pub unsafe fn vmvnq_u32(a: uint32x4_t) -> uint32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = !a.0[i];
        i += 1;
    }
    uint32x4_t(r)
}

pub unsafe fn vnegq_f32(a: float32x4_t) -> float32x4_t {
    let mut r = [0.0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = f32::from_bits(a.0[i].to_bits() ^ 0x80000000);
        i += 1;
    }
    float32x4_t(r)
}

pub unsafe fn vnegq_s32(a: int32x4_t) -> int32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = a.0[i].wrapping_neg();
        i += 1;
    }
    int32x4_t(r)
}

#[inline(always)]
pub unsafe fn vorrq_s16(a: int16x8_t, b: int16x8_t) -> int16x8_t {
    vx::vx_binary(a, b, 7, 16)
}

#[inline(always)]
pub unsafe fn vorrq_s64(a: int64x2_t, b: int64x2_t) -> int64x2_t {
    vx::vx_binary(a, b, 7, 64)
}

#[inline(always)]
pub unsafe fn vorrq_s8(a: int8x16_t, b: int8x16_t) -> int8x16_t {
    vx::vx_binary(a, b, 7, 8)
}

#[inline(always)]
pub unsafe fn vorrq_u16(a: uint16x8_t, b: uint16x8_t) -> uint16x8_t {
    vx::vx_binary(a, b, 7, 16)
}

#[inline(always)]
pub unsafe fn vorrq_u32(a: uint32x4_t, b: uint32x4_t) -> uint32x4_t {
    vx::vx_binary(a, b, 7, 32)
}

#[inline(always)]
pub unsafe fn vorrq_u64(a: uint64x2_t, b: uint64x2_t) -> uint64x2_t {
    vx::vx_binary(a, b, 7, 64)
}

#[inline(always)]
pub unsafe fn vorrq_u8(a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    vx::vx_binary(a, b, 7, 8)
}

pub unsafe fn vpaddlq_s16(a: int16x8_t) -> int32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = a.0[i * 2] as i32 + a.0[i * 2 + 1] as i32;
        i += 1;
    }
    int32x4_t(r)
}

pub unsafe fn vpaddq_f32(a: float32x4_t, b: float32x4_t) -> float32x4_t {
    let mut r = [0.0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = if i < 2 {
            neon_add_f32(a.0[i * 2], a.0[i * 2 + 1])
        } else {
            neon_add_f32(b.0[(i - 2) * 2], b.0[(i - 2) * 2 + 1])
        };
        i += 1;
    }
    float32x4_t(r)
}

pub unsafe fn vpaddq_s32(a: int32x4_t, b: int32x4_t) -> int32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = if i < 2 {
            a.0[i * 2].wrapping_add(a.0[i * 2 + 1])
        } else {
            b.0[(i - 2) * 2].wrapping_add(b.0[(i - 2) * 2 + 1])
        };
        i += 1;
    }
    int32x4_t(r)
}

pub unsafe fn vpmaxq_u8(a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    let mut r = [0; 16];
    let mut i = 0;
    while i < 16 {
        r[i] = if i < 8 {
            a.0[i * 2].max(a.0[i * 2 + 1])
        } else {
            b.0[(i - 8) * 2].max(b.0[(i - 8) * 2 + 1])
        };
        i += 1;
    }
    uint8x16_t(r)
}

#[inline(always)]
pub unsafe fn vqaddq_s16(a: int16x8_t, b: int16x8_t) -> int16x8_t {
    vx::vx_binary(a, b, 2, 16)
}

#[inline(always)]
pub unsafe fn vqaddq_s8(a: int8x16_t, b: int8x16_t) -> int8x16_t {
    vx::vx_binary(a, b, 2, 8)
}

#[inline(always)]
pub unsafe fn vqaddq_u16(a: uint16x8_t, b: uint16x8_t) -> uint16x8_t {
    vx::vx_binary(a, b, 3, 16)
}

#[inline(always)]
pub unsafe fn vqaddq_u8(a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    vx::vx_binary(a, b, 3, 8)
}

#[inline(always)]
pub unsafe fn vqmovn_s16(a: int16x8_t) -> int8x8_t {
    vx::vx_narrow(a, 20)
}

#[inline(always)]
pub unsafe fn vqmovun_s16(a: int16x8_t) -> uint8x8_t {
    vx::vx_narrow(a, 19)
}

pub unsafe fn vqrdmulhq_n_s16(a: int16x8_t, b: i16) -> int16x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = (((a.0[i] as i64 * b as i64 * 2 + (1 << 15)) >> 16)
            .max(i16::MIN as i64)
            .min(i16::MAX as i64)) as i16;
        i += 1;
    }
    int16x8_t(r)
}

pub unsafe fn vqrdmulhq_s16(a: int16x8_t, b: int16x8_t) -> int16x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = (((a.0[i] as i64 * b.0[i] as i64 * 2 + (1 << 15)) >> 16)
            .max(i16::MIN as i64)
            .min(i16::MAX as i64)) as i16;
        i += 1;
    }
    int16x8_t(r)
}

#[inline(always)]
pub unsafe fn vqsubq_s16(a: int16x8_t, b: int16x8_t) -> int16x8_t {
    vx::vx_binary(a, b, 4, 16)
}

#[inline(always)]
pub unsafe fn vqsubq_s8(a: int8x16_t, b: int8x16_t) -> int8x16_t {
    vx::vx_binary(a, b, 4, 8)
}

#[inline(always)]
pub unsafe fn vqsubq_u16(a: uint16x8_t, b: uint16x8_t) -> uint16x8_t {
    vx::vx_binary(a, b, 5, 16)
}

#[inline(always)]
pub unsafe fn vqsubq_u8(a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    vx::vx_binary(a, b, 5, 8)
}

#[inline(always)]
pub unsafe fn vqtbl1q_s8(a: int8x16_t, b: uint8x16_t) -> int8x16_t {
    vx::vx_table(a, b)
}

#[inline(always)]
pub unsafe fn vqtbl1q_u8(a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    vx::vx_table(a, b)
}

pub unsafe fn vqtbl4q_u8(a: uint8x16x4_t, b: uint8x16_t) -> uint8x16_t {
    let mut r = [0; 16];
    let mut i = 0;
    while i < 16 {
        r[i] = {
            let index = b.0[i] as usize;
            match index / 16 {
                0 => a.0 .0[index % 16],
                1 => a.1 .0[index % 16],
                2 => a.2 .0[index % 16],
                3 => a.3 .0[index % 16],
                _ => 0,
            }
        };
        i += 1;
    }
    uint8x16_t(r)
}

pub unsafe fn vreinterpretq_f32_s32(a: int32x4_t) -> float32x4_t {
    crate::ptr::read_unaligned(&a as *const int32x4_t as *const float32x4_t)
}

pub unsafe fn vreinterpretq_f32_u32(a: uint32x4_t) -> float32x4_t {
    crate::ptr::read_unaligned(&a as *const uint32x4_t as *const float32x4_t)
}

pub unsafe fn vreinterpretq_f32_u64(a: uint64x2_t) -> float32x4_t {
    crate::ptr::read_unaligned(&a as *const uint64x2_t as *const float32x4_t)
}

pub unsafe fn vreinterpretq_f32_u8(a: uint8x16_t) -> float32x4_t {
    crate::ptr::read_unaligned(&a as *const uint8x16_t as *const float32x4_t)
}

pub unsafe fn vreinterpretq_f64_u32(a: uint32x4_t) -> float64x2_t {
    crate::ptr::read_unaligned(&a as *const uint32x4_t as *const float64x2_t)
}

pub unsafe fn vreinterpretq_f64_u64(a: uint64x2_t) -> float64x2_t {
    crate::ptr::read_unaligned(&a as *const uint64x2_t as *const float64x2_t)
}

pub unsafe fn vreinterpretq_s16_s32(a: int32x4_t) -> int16x8_t {
    crate::ptr::read_unaligned(&a as *const int32x4_t as *const int16x8_t)
}

pub unsafe fn vreinterpretq_s16_u16(a: uint16x8_t) -> int16x8_t {
    crate::ptr::read_unaligned(&a as *const uint16x8_t as *const int16x8_t)
}

pub unsafe fn vreinterpretq_s32_f32(a: float32x4_t) -> int32x4_t {
    crate::ptr::read_unaligned(&a as *const float32x4_t as *const int32x4_t)
}

pub unsafe fn vreinterpretq_s32_s16(a: int16x8_t) -> int32x4_t {
    crate::ptr::read_unaligned(&a as *const int16x8_t as *const int32x4_t)
}

pub unsafe fn vreinterpretq_s64_u64(a: uint64x2_t) -> int64x2_t {
    crate::ptr::read_unaligned(&a as *const uint64x2_t as *const int64x2_t)
}

pub unsafe fn vreinterpretq_s8_u8(a: uint8x16_t) -> int8x16_t {
    crate::ptr::read_unaligned(&a as *const uint8x16_t as *const int8x16_t)
}

pub unsafe fn vreinterpretq_u16_s16(a: int16x8_t) -> uint16x8_t {
    crate::ptr::read_unaligned(&a as *const int16x8_t as *const uint16x8_t)
}

pub unsafe fn vreinterpretq_u16_s32(a: int32x4_t) -> uint16x8_t {
    crate::ptr::read_unaligned(&a as *const int32x4_t as *const uint16x8_t)
}

pub unsafe fn vreinterpretq_u16_u32(a: uint32x4_t) -> uint16x8_t {
    crate::ptr::read_unaligned(&a as *const uint32x4_t as *const uint16x8_t)
}

pub unsafe fn vreinterpretq_u16_u8(a: uint8x16_t) -> uint16x8_t {
    crate::ptr::read_unaligned(&a as *const uint8x16_t as *const uint16x8_t)
}

pub unsafe fn vreinterpretq_u32_f32(a: float32x4_t) -> uint32x4_t {
    crate::ptr::read_unaligned(&a as *const float32x4_t as *const uint32x4_t)
}

pub unsafe fn vreinterpretq_u32_f64(a: float64x2_t) -> uint32x4_t {
    crate::ptr::read_unaligned(&a as *const float64x2_t as *const uint32x4_t)
}

pub unsafe fn vreinterpretq_u64_f32(a: float32x4_t) -> uint64x2_t {
    crate::ptr::read_unaligned(&a as *const float32x4_t as *const uint64x2_t)
}

pub unsafe fn vreinterpretq_u64_f64(a: float64x2_t) -> uint64x2_t {
    crate::ptr::read_unaligned(&a as *const float64x2_t as *const uint64x2_t)
}

pub unsafe fn vreinterpretq_u64_s64(a: int64x2_t) -> uint64x2_t {
    crate::ptr::read_unaligned(&a as *const int64x2_t as *const uint64x2_t)
}

pub unsafe fn vreinterpretq_u64_u8(a: uint8x16_t) -> uint64x2_t {
    crate::ptr::read_unaligned(&a as *const uint8x16_t as *const uint64x2_t)
}

pub unsafe fn vreinterpretq_u8_s32(a: int32x4_t) -> uint8x16_t {
    crate::ptr::read_unaligned(&a as *const int32x4_t as *const uint8x16_t)
}

pub unsafe fn vreinterpretq_u8_s8(a: int8x16_t) -> uint8x16_t {
    crate::ptr::read_unaligned(&a as *const int8x16_t as *const uint8x16_t)
}

pub unsafe fn vrev64q_f32(a: float32x4_t) -> float32x4_t {
    let mut r = [0.0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = a.0[i ^ 1];
        i += 1;
    }
    float32x4_t(r)
}

pub unsafe fn vrndmq_f32(a: float32x4_t) -> float32x4_t {
    let mut r = [0.0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = neon_result_f32(a.0[i], a.0[i], a.0[i].floor());
        i += 1;
    }
    float32x4_t(r)
}

pub unsafe fn vrndmq_f64(a: float64x2_t) -> float64x2_t {
    let mut r = [0.0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = neon_result_f64(a.0[i], a.0[i], a.0[i].floor());
        i += 1;
    }
    float64x2_t(r)
}

pub unsafe fn vrndnq_f32(a: float32x4_t) -> float32x4_t {
    let mut r = [0.0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = neon_result_f32(a.0[i], a.0[i], a.0[i].round_ties_even());
        i += 1;
    }
    float32x4_t(r)
}

pub unsafe fn vrndpq_f32(a: float32x4_t) -> float32x4_t {
    let mut r = [0.0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = neon_result_f32(a.0[i], a.0[i], a.0[i].ceil());
        i += 1;
    }
    float32x4_t(r)
}

pub unsafe fn vrndpq_f64(a: float64x2_t) -> float64x2_t {
    let mut r = [0.0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = neon_result_f64(a.0[i], a.0[i], a.0[i].ceil());
        i += 1;
    }
    float64x2_t(r)
}

pub unsafe fn vrndq_f32(a: float32x4_t) -> float32x4_t {
    let mut r = [0.0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = neon_result_f32(a.0[i], a.0[i], a.0[i].trunc());
        i += 1;
    }
    float32x4_t(r)
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(2))]
pub unsafe fn vsetq_lane_f32<const LANE: i32>(a: f32, b: float32x4_t) -> float32x4_t {
    let mut r = b;
    r.0[LANE as usize] = a;
    r
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(2))]
pub unsafe fn vsetq_lane_u64<const LANE: i32>(a: u64, b: uint64x2_t) -> uint64x2_t {
    let mut r = b;
    r.0[LANE as usize] = a;
    r
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
#[inline(always)]
pub unsafe fn vshlq_n_s32<const N: i32>(a: int32x4_t) -> int32x4_t {
    vx::vx_unary(a, 1, 32, N)
}

pub unsafe fn vshlq_s16(a: int16x8_t, b: int16x8_t) -> int16x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = {
            let shift = b.0[i] as i8 as i32;
            if shift >= 16 {
                0
            } else if shift >= 0 {
                a.0[i] << shift
            } else if shift <= -16 {
                a.0[i] >> 15
            } else {
                a.0[i] >> -shift
            }
        };
        i += 1;
    }
    int16x8_t(r)
}

pub unsafe fn vshlq_s32(a: int32x4_t, b: int32x4_t) -> int32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = {
            let shift = b.0[i] as i8 as i32;
            if shift >= 32 {
                0
            } else if shift >= 0 {
                a.0[i] << shift
            } else if shift <= -32 {
                a.0[i] >> 31
            } else {
                a.0[i] >> -shift
            }
        };
        i += 1;
    }
    int32x4_t(r)
}

pub unsafe fn vshlq_s64(a: int64x2_t, b: int64x2_t) -> int64x2_t {
    let mut r = [0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = {
            let shift = b.0[i] as i8 as i32;
            if shift >= 64 {
                0
            } else if shift >= 0 {
                a.0[i] << shift
            } else if shift <= -64 {
                a.0[i] >> 63
            } else {
                a.0[i] >> -shift
            }
        };
        i += 1;
    }
    int64x2_t(r)
}

pub unsafe fn vshlq_u16(a: uint16x8_t, b: int16x8_t) -> uint16x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = {
            let shift = b.0[i] as i8 as i32;
            if shift >= 16 {
                0
            } else if shift >= 0 {
                a.0[i] << shift
            } else if shift <= -16 {
                0
            } else {
                a.0[i] >> -shift
            }
        };
        i += 1;
    }
    uint16x8_t(r)
}

pub unsafe fn vshlq_u32(a: uint32x4_t, b: int32x4_t) -> uint32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = {
            let shift = b.0[i] as i8 as i32;
            if shift >= 32 {
                0
            } else if shift >= 0 {
                a.0[i] << shift
            } else if shift <= -32 {
                0
            } else {
                a.0[i] >> -shift
            }
        };
        i += 1;
    }
    uint32x4_t(r)
}

pub unsafe fn vshlq_u64(a: uint64x2_t, b: int64x2_t) -> uint64x2_t {
    let mut r = [0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = {
            let shift = b.0[i] as i8 as i32;
            if shift >= 64 {
                0
            } else if shift >= 0 {
                a.0[i] << shift
            } else if shift <= -64 {
                0
            } else {
                a.0[i] >> -shift
            }
        };
        i += 1;
    }
    uint64x2_t(r)
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
pub unsafe fn vshrn_n_u16<const N: i32>(a: uint16x8_t) -> uint8x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = (a.0[i] >> N) as u8;
        i += 1;
    }
    uint8x8_t(r)
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
pub unsafe fn vshrn_n_u64<const N: i32>(a: uint64x2_t) -> uint32x2_t {
    let mut r = [0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = (a.0[i] >> N) as u32;
        i += 1;
    }
    uint32x2_t(r)
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(1))]
#[inline(always)]
pub unsafe fn vshrq_n_u32<const N: i32>(a: uint32x4_t) -> uint32x4_t {
    vx::vx_unary(a, 2, 32, N)
}

pub unsafe fn vsqrtq_f32(a: float32x4_t) -> float32x4_t {
    vx::vx_unary(a, 9, 32, 0)
}

pub unsafe fn vsqrtq_f64(a: float64x2_t) -> float64x2_t {
    let mut r = [0.0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = neon_result_f64(a.0[i], a.0[i], a.0[i].sqrt());
        i += 1;
    }
    float64x2_t(r)
}

#[inline(always)]
pub unsafe fn vst1q_f32(ptr: *mut f32, a: float32x4_t) {
    vx::vx_store(ptr as *mut u8, a)
}

#[inline(always)]
pub unsafe fn vst1q_f64(ptr: *mut f64, a: float64x2_t) {
    vx::vx_store(ptr as *mut u8, a)
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(2))]
pub unsafe fn vst1q_lane_f32<const LANE: i32>(a: *mut f32, b: float32x4_t) {
    crate::ptr::write_unaligned(a, b.0[LANE as usize]);
}

#[cfg_attr(not(relax_check), rustc_legacy_const_generics(2))]
pub unsafe fn vst1q_lane_f64<const LANE: i32>(a: *mut f64, b: float64x2_t) {
    crate::ptr::write_unaligned(a, b.0[LANE as usize]);
}

#[inline(always)]
pub unsafe fn vst1q_s32(a: *mut i32, b: int32x4_t) {
    vx::vx_store(a as *mut u8, b)
}

#[inline(always)]
pub unsafe fn vst1q_u8(a: *mut u8, b: uint8x16_t) {
    vx::vx_store(a as *mut u8, b)
}

pub unsafe fn vst3q_f32(a: *mut f32, b: float32x4x3_t) {
    let mut i = 0;
    while i < 4 {
        crate::ptr::write_unaligned(a.add(i * 3 + 0), b.0 .0[i]);
        crate::ptr::write_unaligned(a.add(i * 3 + 1), b.1 .0[i]);
        crate::ptr::write_unaligned(a.add(i * 3 + 2), b.2 .0[i]);
        i += 1;
    }
}

#[inline(always)]
pub unsafe fn vsubq_f32(a: float32x4_t, b: float32x4_t) -> float32x4_t {
    vx::vx_binary(a, b, 61, 32)
}

pub unsafe fn vsubq_f64(a: float64x2_t, b: float64x2_t) -> float64x2_t {
    let mut r = [0.0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = neon_result_f64(a.0[i], b.0[i], a.0[i] - b.0[i]);
        i += 1;
    }
    float64x2_t(r)
}

#[inline(always)]
pub unsafe fn vsubq_s64(a: int64x2_t, b: int64x2_t) -> int64x2_t {
    vx::vx_binary(a, b, 1, 64)
}

#[inline(always)]
pub unsafe fn vsubq_s8(a: int8x16_t, b: int8x16_t) -> int8x16_t {
    vx::vx_binary(a, b, 1, 8)
}

#[inline(always)]
pub unsafe fn vsubq_u16(a: uint16x8_t, b: uint16x8_t) -> uint16x8_t {
    vx::vx_binary(a, b, 1, 16)
}

#[inline(always)]
pub unsafe fn vsubq_u32(a: uint32x4_t, b: uint32x4_t) -> uint32x4_t {
    vx::vx_binary(a, b, 1, 32)
}

#[inline(always)]
pub unsafe fn vsubq_u64(a: uint64x2_t, b: uint64x2_t) -> uint64x2_t {
    vx::vx_binary(a, b, 1, 64)
}

#[inline(always)]
pub unsafe fn vsubq_u8(a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    vx::vx_binary(a, b, 1, 8)
}

pub unsafe fn vtrn1q_f32(a: float32x4_t, b: float32x4_t) -> float32x4_t {
    let mut r = [0.0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = if i % 2 == 0 {
            a.0[(i / 2) * 2 + 0]
        } else {
            b.0[(i / 2) * 2 + 0]
        };
        i += 1;
    }
    float32x4_t(r)
}

pub unsafe fn vtrn2q_f32(a: float32x4_t, b: float32x4_t) -> float32x4_t {
    let mut r = [0.0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = if i % 2 == 0 {
            a.0[(i / 2) * 2 + 1]
        } else {
            b.0[(i / 2) * 2 + 1]
        };
        i += 1;
    }
    float32x4_t(r)
}

pub unsafe fn vtrnq_s16(a: int16x8_t, b: int16x8_t) -> int16x8x2_t {
    int16x8x2_t(
        {
            let mut r = [0; 8];
            let mut i = 0;
            while i < 8 {
                r[i] = if i % 2 == 0 {
                    a.0[(i / 2) * 2 + 0]
                } else {
                    b.0[(i / 2) * 2 + 0]
                };
                i += 1;
            }
            int16x8_t(r)
        },
        {
            let mut r = [0; 8];
            let mut i = 0;
            while i < 8 {
                r[i] = if i % 2 == 0 {
                    a.0[(i / 2) * 2 + 1]
                } else {
                    b.0[(i / 2) * 2 + 1]
                };
                i += 1;
            }
            int16x8_t(r)
        },
    )
}

pub unsafe fn vuzp1q_f32(a: float32x4_t, b: float32x4_t) -> float32x4_t {
    let mut r = [0.0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = if i < 2 {
            a.0[i * 2 + 0]
        } else {
            b.0[(i - 2) * 2 + 0]
        };
        i += 1;
    }
    float32x4_t(r)
}

pub unsafe fn vuzp2q_f32(a: float32x4_t, b: float32x4_t) -> float32x4_t {
    let mut r = [0.0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = if i < 2 {
            a.0[i * 2 + 1]
        } else {
            b.0[(i - 2) * 2 + 1]
        };
        i += 1;
    }
    float32x4_t(r)
}

pub unsafe fn vuzpq_u16(a: uint16x8_t, b: uint16x8_t) -> uint16x8x2_t {
    uint16x8x2_t(
        {
            let mut r = [0; 8];
            let mut i = 0;
            while i < 8 {
                r[i] = if i < 4 {
                    a.0[i * 2 + 0]
                } else {
                    b.0[(i - 4) * 2 + 0]
                };
                i += 1;
            }
            uint16x8_t(r)
        },
        {
            let mut r = [0; 8];
            let mut i = 0;
            while i < 8 {
                r[i] = if i < 4 {
                    a.0[i * 2 + 1]
                } else {
                    b.0[(i - 4) * 2 + 1]
                };
                i += 1;
            }
            uint16x8_t(r)
        },
    )
}

pub unsafe fn vzip1q_f32(a: float32x4_t, b: float32x4_t) -> float32x4_t {
    vx::vx_binary(a, b, 17, 32)
}

pub unsafe fn vzip2q_f32(a: float32x4_t, b: float32x4_t) -> float32x4_t {
    vx::vx_binary(a, b, 18, 32)
}

#[inline(always)]
pub unsafe fn vzip2q_u64(a: uint64x2_t, b: uint64x2_t) -> uint64x2_t {
    vx::vx_binary(a, b, 18, 64)
}

pub unsafe fn vzip_u8(a: uint8x8_t, b: uint8x8_t) -> uint8x8x2_t {
    uint8x8x2_t(
        {
            let mut r = [0; 8];
            let mut i = 0;
            while i < 8 {
                r[i] = if i % 2 == 0 {
                    a.0[i / 2 + 0]
                } else {
                    b.0[i / 2 + 0]
                };
                i += 1;
            }
            uint8x8_t(r)
        },
        {
            let mut r = [0; 8];
            let mut i = 0;
            while i < 8 {
                r[i] = if i % 2 == 0 {
                    a.0[i / 2 + 4]
                } else {
                    b.0[i / 2 + 4]
                };
                i += 1;
            }
            uint8x8_t(r)
        },
    )
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
simd_debug!(int8x8_t int16x8_t int32x4_t int64x2_t uint16x8_t uint16x4_t uint32x2_t uint32x4_t uint64x1_t uint8x8_t uint8x16_t int16x4_t int32x2_t uint64x2_t float32x2_t float32x4_t float64x1_t float64x2_t int64x1_t int8x16_t poly16x4_t poly16x8_t poly64x1_t poly64x2_t poly8x16_t poly8x8_t);

// ---------------------------------------------------------------- widening moves and integer compares (unsigned vmovl, full vcge/vcle/vcgt/vclt families)

pub unsafe fn vmovl_u8(a: uint8x8_t) -> uint16x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = a.0[i] as u16;
        i += 1;
    }
    uint16x8_t(r)
}

pub unsafe fn vmovl_u16(a: uint16x4_t) -> uint32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = a.0[i] as u32;
        i += 1;
    }
    uint32x4_t(r)
}

pub unsafe fn vmovl_u32(a: uint32x2_t) -> uint64x2_t {
    let mut r = [0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = a.0[i] as u64;
        i += 1;
    }
    uint64x2_t(r)
}

pub unsafe fn vcgeq_u8(a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    let mut r = [0; 16];
    let mut i = 0;
    while i < 16 {
        r[i] = if a.0[i] >= b.0[i] { u8::MAX } else { 0 };
        i += 1;
    }
    uint8x16_t(r)
}

pub unsafe fn vcgeq_u16(a: uint16x8_t, b: uint16x8_t) -> uint16x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = if a.0[i] >= b.0[i] { u16::MAX } else { 0 };
        i += 1;
    }
    uint16x8_t(r)
}

pub unsafe fn vcgeq_u32(a: uint32x4_t, b: uint32x4_t) -> uint32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = if a.0[i] >= b.0[i] { u32::MAX } else { 0 };
        i += 1;
    }
    uint32x4_t(r)
}

pub unsafe fn vcgeq_u64(a: uint64x2_t, b: uint64x2_t) -> uint64x2_t {
    let mut r = [0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = if a.0[i] >= b.0[i] { u64::MAX } else { 0 };
        i += 1;
    }
    uint64x2_t(r)
}

pub unsafe fn vcgeq_s8(a: int8x16_t, b: int8x16_t) -> uint8x16_t {
    let mut r = [0; 16];
    let mut i = 0;
    while i < 16 {
        r[i] = if a.0[i] >= b.0[i] { u8::MAX } else { 0 };
        i += 1;
    }
    uint8x16_t(r)
}

pub unsafe fn vcgeq_s16(a: int16x8_t, b: int16x8_t) -> uint16x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = if a.0[i] >= b.0[i] { u16::MAX } else { 0 };
        i += 1;
    }
    uint16x8_t(r)
}

pub unsafe fn vcgeq_s32(a: int32x4_t, b: int32x4_t) -> uint32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = if a.0[i] >= b.0[i] { u32::MAX } else { 0 };
        i += 1;
    }
    uint32x4_t(r)
}

pub unsafe fn vcgeq_s64(a: int64x2_t, b: int64x2_t) -> uint64x2_t {
    let mut r = [0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = if a.0[i] >= b.0[i] { u64::MAX } else { 0 };
        i += 1;
    }
    uint64x2_t(r)
}

#[inline(always)]
pub unsafe fn vcgtq_u8(a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    vx::vx_binary(a, b, 29, 8)
}

#[inline(always)]
pub unsafe fn vcgtq_u16(a: uint16x8_t, b: uint16x8_t) -> uint16x8_t {
    vx::vx_binary(a, b, 29, 16)
}

pub unsafe fn vcleq_u8(a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    let mut r = [0; 16];
    let mut i = 0;
    while i < 16 {
        r[i] = if a.0[i] <= b.0[i] { u8::MAX } else { 0 };
        i += 1;
    }
    uint8x16_t(r)
}

pub unsafe fn vcleq_u16(a: uint16x8_t, b: uint16x8_t) -> uint16x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = if a.0[i] <= b.0[i] { u16::MAX } else { 0 };
        i += 1;
    }
    uint16x8_t(r)
}

pub unsafe fn vcleq_u32(a: uint32x4_t, b: uint32x4_t) -> uint32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = if a.0[i] <= b.0[i] { u32::MAX } else { 0 };
        i += 1;
    }
    uint32x4_t(r)
}

pub unsafe fn vcleq_u64(a: uint64x2_t, b: uint64x2_t) -> uint64x2_t {
    let mut r = [0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = if a.0[i] <= b.0[i] { u64::MAX } else { 0 };
        i += 1;
    }
    uint64x2_t(r)
}

pub unsafe fn vcleq_s8(a: int8x16_t, b: int8x16_t) -> uint8x16_t {
    let mut r = [0; 16];
    let mut i = 0;
    while i < 16 {
        r[i] = if a.0[i] <= b.0[i] { u8::MAX } else { 0 };
        i += 1;
    }
    uint8x16_t(r)
}

pub unsafe fn vcleq_s16(a: int16x8_t, b: int16x8_t) -> uint16x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = if a.0[i] <= b.0[i] { u16::MAX } else { 0 };
        i += 1;
    }
    uint16x8_t(r)
}

pub unsafe fn vcleq_s32(a: int32x4_t, b: int32x4_t) -> uint32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = if a.0[i] <= b.0[i] { u32::MAX } else { 0 };
        i += 1;
    }
    uint32x4_t(r)
}

pub unsafe fn vcleq_s64(a: int64x2_t, b: int64x2_t) -> uint64x2_t {
    let mut r = [0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = if a.0[i] <= b.0[i] { u64::MAX } else { 0 };
        i += 1;
    }
    uint64x2_t(r)
}

#[inline(always)]
pub unsafe fn vcltq_u8(a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    vx::vx_binary(b, a, 29, 8)
}

#[inline(always)]
pub unsafe fn vcltq_u16(a: uint16x8_t, b: uint16x8_t) -> uint16x8_t {
    vx::vx_binary(b, a, 29, 16)
}

#[inline(always)]
pub unsafe fn vcltq_u32(a: uint32x4_t, b: uint32x4_t) -> uint32x4_t {
    vx::vx_binary(b, a, 29, 32)
}

#[inline(always)]
pub unsafe fn vcltq_u64(a: uint64x2_t, b: uint64x2_t) -> uint64x2_t {
    vx::vx_binary(b, a, 29, 64)
}

#[inline(always)]
pub unsafe fn vrhaddq_u8(a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    vx::vx_binary(a, b, 16, 8)
}

#[inline(always)]
pub unsafe fn vrhaddq_u16(a: uint16x8_t, b: uint16x8_t) -> uint16x8_t {
    vx::vx_binary(a, b, 16, 16)
}

#[inline(always)]
pub unsafe fn vbicq_s8(a: int8x16_t, b: int8x16_t) -> int8x16_t {
    vx::vx_binary(a, b, 9, 8)
}

#[inline(always)]
pub unsafe fn vzip1q_s8(a: int8x16_t, b: int8x16_t) -> int8x16_t {
    vx::vx_binary(a, b, 17, 8)
}

#[inline(always)]
pub unsafe fn vzip2q_s8(a: int8x16_t, b: int8x16_t) -> int8x16_t {
    vx::vx_binary(a, b, 18, 8)
}

#[inline(always)]
pub unsafe fn vshlq_n_s8<const N: i32>(a: int8x16_t) -> int8x16_t {
    vx::vx_unary(a, 1, 8, N)
}

#[inline(always)]
pub unsafe fn vshrq_n_s8<const N: i32>(a: int8x16_t) -> int8x16_t {
    vx::vx_unary(a, 3, 8, N)
}

#[inline(always)]
pub unsafe fn vst1q_s8(a: *mut i8, b: int8x16_t) {
    vx::vx_store(a as *mut u8, b)
}

#[inline(always)]
pub unsafe fn vbicq_s16(a: int16x8_t, b: int16x8_t) -> int16x8_t {
    vx::vx_binary(a, b, 9, 16)
}

#[inline(always)]
pub unsafe fn vshlq_n_s16<const N: i32>(a: int16x8_t) -> int16x8_t {
    vx::vx_unary(a, 1, 16, N)
}

#[inline(always)]
pub unsafe fn vbicq_s32(a: int32x4_t, b: int32x4_t) -> int32x4_t {
    vx::vx_binary(a, b, 9, 32)
}

#[inline(always)]
pub unsafe fn vzip1q_s32(a: int32x4_t, b: int32x4_t) -> int32x4_t {
    vx::vx_binary(a, b, 17, 32)
}

#[inline(always)]
pub unsafe fn vzip2q_s32(a: int32x4_t, b: int32x4_t) -> int32x4_t {
    vx::vx_binary(a, b, 18, 32)
}

#[inline(always)]
pub unsafe fn vbicq_s64(a: int64x2_t, b: int64x2_t) -> int64x2_t {
    vx::vx_binary(a, b, 9, 64)
}

#[inline(always)]
pub unsafe fn vzip1q_s64(a: int64x2_t, b: int64x2_t) -> int64x2_t {
    vx::vx_binary(a, b, 17, 64)
}

#[inline(always)]
pub unsafe fn vzip2q_s64(a: int64x2_t, b: int64x2_t) -> int64x2_t {
    vx::vx_binary(a, b, 18, 64)
}

#[inline(always)]
pub unsafe fn vshlq_n_s64<const N: i32>(a: int64x2_t) -> int64x2_t {
    vx::vx_unary(a, 1, 64, N)
}

#[inline(always)]
pub unsafe fn vshrq_n_s64<const N: i32>(a: int64x2_t) -> int64x2_t {
    vx::vx_unary(a, 3, 64, N)
}

#[inline(always)]
pub unsafe fn vld1q_s64(a: *const i64) -> int64x2_t {
    vx::vx_load(a as *const u8)
}

#[inline(always)]
pub unsafe fn vst1q_s64(a: *mut i64, b: int64x2_t) {
    vx::vx_store(a as *mut u8, b)
}

#[inline(always)]
pub unsafe fn vbicq_u8(a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    vx::vx_binary(a, b, 9, 8)
}

#[inline(always)]
pub unsafe fn vzip1q_u8(a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    vx::vx_binary(a, b, 17, 8)
}

#[inline(always)]
pub unsafe fn vzip2q_u8(a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    vx::vx_binary(a, b, 18, 8)
}

#[inline(always)]
pub unsafe fn vshlq_n_u8<const N: i32>(a: uint8x16_t) -> uint8x16_t {
    vx::vx_unary(a, 1, 8, N)
}

#[inline(always)]
pub unsafe fn vshrq_n_u8<const N: i32>(a: uint8x16_t) -> uint8x16_t {
    vx::vx_unary(a, 2, 8, N)
}

#[inline(always)]
pub unsafe fn vbicq_u16(a: uint16x8_t, b: uint16x8_t) -> uint16x8_t {
    vx::vx_binary(a, b, 9, 16)
}

#[inline(always)]
pub unsafe fn vzip1q_u16(a: uint16x8_t, b: uint16x8_t) -> uint16x8_t {
    vx::vx_binary(a, b, 17, 16)
}

#[inline(always)]
pub unsafe fn vzip2q_u16(a: uint16x8_t, b: uint16x8_t) -> uint16x8_t {
    vx::vx_binary(a, b, 18, 16)
}

#[inline(always)]
pub unsafe fn vshlq_n_u16<const N: i32>(a: uint16x8_t) -> uint16x8_t {
    vx::vx_unary(a, 1, 16, N)
}

#[inline(always)]
pub unsafe fn vshrq_n_u16<const N: i32>(a: uint16x8_t) -> uint16x8_t {
    vx::vx_unary(a, 2, 16, N)
}

#[inline(always)]
pub unsafe fn vst1q_u16(a: *mut u16, b: uint16x8_t) {
    vx::vx_store(a as *mut u8, b)
}

#[inline(always)]
pub unsafe fn vzip1q_u32(a: uint32x4_t, b: uint32x4_t) -> uint32x4_t {
    vx::vx_binary(a, b, 17, 32)
}

#[inline(always)]
pub unsafe fn vzip2q_u32(a: uint32x4_t, b: uint32x4_t) -> uint32x4_t {
    vx::vx_binary(a, b, 18, 32)
}

#[inline(always)]
pub unsafe fn vbicq_u64(a: uint64x2_t, b: uint64x2_t) -> uint64x2_t {
    vx::vx_binary(a, b, 9, 64)
}

#[inline(always)]
pub unsafe fn vzip1q_u64(a: uint64x2_t, b: uint64x2_t) -> uint64x2_t {
    vx::vx_binary(a, b, 17, 64)
}

#[inline(always)]
pub unsafe fn vshlq_n_u64<const N: i32>(a: uint64x2_t) -> uint64x2_t {
    vx::vx_unary(a, 1, 64, N)
}

#[inline(always)]
pub unsafe fn vshrq_n_u64<const N: i32>(a: uint64x2_t) -> uint64x2_t {
    vx::vx_unary(a, 2, 64, N)
}

#[inline(always)]
pub unsafe fn vld1q_u64(a: *const u64) -> uint64x2_t {
    vx::vx_load(a as *const u8)
}

#[inline(always)]
pub unsafe fn vst1q_u64(a: *mut u64, b: uint64x2_t) {
    vx::vx_store(a as *mut u8, b)
}

#[inline(always)]
pub unsafe fn vmaxq_s32(a: int32x4_t, b: int32x4_t) -> int32x4_t {
    vx::vx_binary(a, b, 14, 32)
}
