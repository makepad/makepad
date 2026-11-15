//! NEON, CRC32 and SHA-1 operations. Lane zero is the lowest-address lane.
//! Hot 128-bit NEON operations use the compiler vector bridge.
//!
//! Signatures and lane operations follow stdarch's aarch64 and arm_shared/neon.
//! CRC32 uses the reflected IEEE polynomial, without initial/final complements.
//! SHA-1 follows Arm's four-round and message-schedule pseudocode.
//!
//! The families (load/store, lane moves, arithmetic, shifts, compares, reinterpret) are tables
//! at the end of this file, one row per intrinsic.
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

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "crc"))]
pub fn __crc32b(crc: u32, data: u8) -> u32 {
    unsafe {
        crc32::<1>(crc, data as u64)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "crc"))]
pub fn __crc32d(crc: u32, data: u64) -> u32 {
    unsafe {
        crc32::<8>(crc, data)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "crc"))]
pub fn __crc32cb(crc: u32, data: u8) -> u32 {
    unsafe {
        crc32c::<1>(crc, data as u64)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "crc"))]
pub fn __crc32ch(crc: u32, data: u16) -> u32 {
    unsafe {
        crc32c::<2>(crc, data as u64)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "crc"))]
pub fn __crc32cw(crc: u32, data: u32) -> u32 {
    unsafe {
        crc32c::<4>(crc, data as u64)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "crc"))]
pub fn __crc32cd(crc: u32, data: u64) -> u32 {
    unsafe {
        crc32c::<8>(crc, data)
    }
}

/// CRC-32C (reflected 0x82F63B78, no inversions) of data's low BYTES bytes into crc: as crc32.
#[inline(always)]
unsafe fn crc32c<const BYTES: u32>(crc: u32, data: u64) -> u32 {
    #[cfg(target_arch = "aarch64")]
    {
        vx::vx_crc32c(crc, data, BYTES)
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        let mut r = crc;
        let mut i = 0;
        while i < BYTES * 8 {
            r ^= ((data >> i) & 1) as u32;
            r = (r >> 1) ^ (0x82f6_3b78u32 & (0u32.wrapping_sub(r & 1)));
            i += 1;
        }
        r
    }
}

/// CRC-32 (reflected 0xEDB88320, no inversions) of data's low BYTES bytes into crc: the
/// instruction on aarch64 targets, bit by bit elsewhere (this module compiles on every target).
#[inline(always)]
unsafe fn crc32<const BYTES: u32>(crc: u32, data: u64) -> u32 {
    #[cfg(target_arch = "aarch64")]
    {
        vx::vx_crc32(crc, data, BYTES)
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        let mut r = crc;
        let mut i = 0;
        while i < BYTES * 8 {
            r ^= ((data >> i) & 1) as u32;
            r = (r >> 1) ^ (0xedb8_8320u32 & (0u32.wrapping_sub(r & 1)));
            i += 1;
        }
        r
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vaddq_s16(a: int16x8_t, b: int16x8_t) -> int16x8_t {
    unsafe {
        vx::vx_binary(a, b, 0, 16)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vaddq_s32(a: int32x4_t, b: int32x4_t) -> int32x4_t {
    unsafe {
        vx::vx_binary(a, b, 0, 32)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vaddq_u32(a: uint32x4_t, b: uint32x4_t) -> uint32x4_t {
    unsafe {
        vx::vx_binary(a, b, 0, 32)
    }
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vaddvq_u32(a: uint32x4_t) -> u32 {
    let mut r = 0u32;
    let mut i = 0;
    while i < 4 {
        r = r.wrapping_add(a.0[i]);
        i += 1;
    }
    r
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vaddw_u8(a: uint16x8_t, b: uint8x8_t) -> uint16x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = a.0[i].wrapping_add(b.0[i] as u16);
        i += 1;
    }
    uint16x8_t(r)
}

/// vaddw_high: the upper half of `b` widened and added
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vaddw_high_u8(a: uint16x8_t, b: uint8x16_t) -> uint16x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = a.0[i].wrapping_add(b.0[i + 8] as u16);
        i += 1;
    }
    uint16x8_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vceq_u8(a: uint8x8_t, b: uint8x8_t) -> uint8x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = if a.0[i] == b.0[i] { 255 } else { 0 };
        i += 1;
    }
    uint8x8_t(r)
}

/// TBL with one 8-byte table: an index >= 8 gives 0
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vtbl1_u8(a: uint8x8_t, b: uint8x8_t) -> uint8x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = if b.0[i] < 8 { a.0[b.0[i] as usize] } else { 0 };
        i += 1;
    }
    uint8x8_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcgez_s8(a: int8x8_t) -> uint8x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = if a.0[i] >= 0 { 255 } else { 0 };
        i += 1;
    }
    uint8x8_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcltz_s8(a: int8x8_t) -> uint8x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = if a.0[i] < 0 { 255 } else { 0 };
        i += 1;
    }
    uint8x8_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcombine_s16(a: int16x4_t, b: int16x4_t) -> int16x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = if i < 4 { a.0[i] } else { b.0[i - 4] };
        i += 1;
    }
    int16x8_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcombine_u16(a: uint16x4_t, b: uint16x4_t) -> uint16x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = if i < 4 { a.0[i] } else { b.0[i - 4] };
        i += 1;
    }
    uint16x8_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcombine_u8(a: uint8x8_t, b: uint8x8_t) -> uint8x16_t {
    let mut r = [0; 16];
    let mut i = 0;
    while i < 16 {
        r[i] = if i < 8 { a.0[i] } else { b.0[i - 8] };
        i += 1;
    }
    uint8x16_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcreate_s16(a: u64) -> int16x4_t {
    int16x4_t([
        a as i16,
        (a >> 16) as i16,
        (a >> 32) as i16,
        (a >> 48) as i16,
    ])
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vdup_n_u8(value: u8) -> uint8x8_t {
    uint8x8_t([value; 8])
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vdupq_n_s16(value: i16) -> int16x8_t {
    unsafe {
        vx::vx_splat(value as u64, 16)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vdupq_n_s32(value: i32) -> int32x4_t {
    unsafe {
        vx::vx_splat(value as u64, 32)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vdupq_n_u16(value: u16) -> uint16x8_t {
    unsafe {
        vx::vx_splat(value as u64, 16)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vdupq_n_u32(value: u32) -> uint32x4_t {
    unsafe {
        vx::vx_splat(value as u64, 32)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vdupq_n_u8(value: u8) -> uint8x16_t {
    unsafe {
        vx::vx_splat(value as u64, 8)
    }
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vget_high_u16(a: uint16x8_t) -> uint16x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = a.0[i + 4];
        i += 1;
    }
    uint16x4_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vget_high_u32(a: uint32x4_t) -> uint32x2_t {
    let mut r = [0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = a.0[i + 2];
        i += 1;
    }
    uint32x2_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vget_high_u8(a: uint8x16_t) -> uint8x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = a.0[i + 8];
        i += 1;
    }
    uint8x8_t(r)
}

#[rustc_legacy_const_generics(1)]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vget_lane_u32<const LANE: i32>(v: uint32x2_t) -> u32 {
    v.0[LANE as usize]
}

#[rustc_legacy_const_generics(1)]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vget_lane_u64<const LANE: i32>(v: uint64x1_t) -> u64 {
    v.0[LANE as usize]
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vget_low_s16(a: int16x8_t) -> int16x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = a.0[i + 0];
        i += 1;
    }
    int16x4_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vget_low_u16(a: uint16x8_t) -> uint16x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = a.0[i + 0];
        i += 1;
    }
    uint16x4_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vget_low_u32(a: uint32x4_t) -> uint32x2_t {
    let mut r = [0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = a.0[i + 0];
        i += 1;
    }
    uint32x2_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vget_low_u8(a: uint8x16_t) -> uint8x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = a.0[i + 0];
        i += 1;
    }
    uint8x8_t(r)
}

#[rustc_legacy_const_generics(1)]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vgetq_lane_u32<const LANE: i32>(v: uint32x4_t) -> u32 {
    v.0[LANE as usize]
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub unsafe fn vld1_u16(ptr: *const u16) -> uint16x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = crate::ptr::read_unaligned(ptr.add(i));
        i += 1;
    }
    uint16x4_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub unsafe fn vld1_u8(ptr: *const u8) -> uint8x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = crate::ptr::read_unaligned(ptr.add(i));
        i += 1;
    }
    uint8x8_t(r)
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub unsafe fn vld1q_s16(a: *const i16) -> int16x8_t {
    vx::vx_load(a as *const u8)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub unsafe fn vld1q_s16_x2(a: *const i16) -> int16x8x2_t {
    int16x8x2_t(
        crate::ptr::read_unaligned(a as *const int16x8_t),
        crate::ptr::read_unaligned(a.add(8) as *const int16x8_t),
    )
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub unsafe fn vld1q_s32_x2(a: *const i32) -> int32x4x2_t {
    int32x4x2_t(
        crate::ptr::read_unaligned(a as *const int32x4_t),
        crate::ptr::read_unaligned(a.add(4) as *const int32x4_t),
    )
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub unsafe fn vld1q_u16(a: *const u16) -> uint16x8_t {
    vx::vx_load(a as *const u8)
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub unsafe fn vld1q_u32(a: *const u32) -> uint32x4_t {
    vx::vx_load(a as *const u8)
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub unsafe fn vld1q_u8(a: *const u8) -> uint8x16_t {
    vx::vx_load(a as *const u8)
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmaxq_s16(a: int16x8_t, b: int16x8_t) -> int16x8_t {
    unsafe {
        vx::vx_binary(a, b, 14, 16)
    }
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmaxvq_u32(a: uint32x4_t) -> u32 {
    let mut r = 0u32;
    let mut i = 0;
    while i < 4 {
        r = r.max(a.0[i]);
        i += 1;
    }
    r
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vminq_s16(a: int16x8_t, b: int16x8_t) -> int16x8_t {
    unsafe {
        vx::vx_binary(a, b, 12, 16)
    }
}

#[rustc_legacy_const_generics(3)]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmlal_high_laneq_s16<const LANE: i32>(
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

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmlal_high_u16(a: uint32x4_t, b: uint16x8_t, c: uint16x8_t) -> uint32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = a.0[i].wrapping_add((b.0[i + 4] as u32) * (c.0[i + 4] as u32));
        i += 1;
    }
    uint32x4_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmlal_high_u32(a: uint64x2_t, b: uint32x4_t, c: uint32x4_t) -> uint64x2_t {
    let mut r = [0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = a.0[i].wrapping_add((b.0[i + 2] as u64) * (c.0[i + 2] as u64));
        i += 1;
    }
    uint64x2_t(r)
}

#[rustc_legacy_const_generics(3)]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmlal_laneq_s16<const LANE: i32>(
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

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmlal_s16(a: int32x4_t, b: int16x4_t, c: int16x4_t) -> int32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = a.0[i].wrapping_add((b.0[i] as i32) * (c.0[i] as i32));
        i += 1;
    }
    int32x4_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmlal_u16(a: uint32x4_t, b: uint16x4_t, c: uint16x4_t) -> uint32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = a.0[i].wrapping_add((b.0[i + 0] as u32) * (c.0[i + 0] as u32));
        i += 1;
    }
    uint32x4_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmlal_u32(a: uint64x2_t, b: uint32x2_t, c: uint32x2_t) -> uint64x2_t {
    let mut r = [0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = a.0[i].wrapping_add((b.0[i] as u64) * (c.0[i] as u64));
        i += 1;
    }
    uint64x2_t(r)
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmulq_s16(a: int16x8_t, b: int16x8_t) -> int16x8_t {
    unsafe {
        vx::vx_binary(a, b, 26, 16)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmulq_s32(a: int32x4_t, b: int32x4_t) -> int32x4_t {
    unsafe {
        vx::vx_binary(a, b, 26, 32)
    }
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vorr_u8(a: uint8x8_t, b: uint8x8_t) -> uint8x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = a.0[i] | b.0[i];
        i += 1;
    }
    uint8x8_t(r)
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vorrq_s32(a: int32x4_t, b: int32x4_t) -> int32x4_t {
    unsafe {
        vx::vx_binary(a, b, 7, 32)
    }
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vpadalq_u16(a: uint32x4_t, b: uint16x8_t) -> uint32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = a.0[i].wrapping_add(b.0[i * 2] as u32 + b.0[i * 2 + 1] as u32);
        i += 1;
    }
    uint32x4_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vpadalq_u8(a: uint16x8_t, b: uint8x16_t) -> uint16x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = a.0[i].wrapping_add(b.0[i * 2] as u16 + b.0[i * 2 + 1] as u16);
        i += 1;
    }
    uint16x8_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vpadd_u32(a: uint32x2_t, b: uint32x2_t) -> uint32x2_t {
    uint32x2_t([a.0[0].wrapping_add(a.0[1]), b.0[0].wrapping_add(b.0[1])])
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vpaddlq_u8(a: uint8x16_t) -> uint16x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = a.0[i * 2] as u16 + a.0[i * 2 + 1] as u16;
        i += 1;
    }
    uint16x8_t(r)
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vqmovn_s32(a: int32x4_t) -> int16x4_t {
    unsafe {
        vx::vx_narrow(a, 21)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vqmovn_u16(a: uint16x8_t) -> uint8x8_t {
    unsafe {
        vx::vx_narrow(a, 30)
    }
}

#[rustc_legacy_const_generics(1)]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vqshlq_n_u32<const N: i32>(a: uint32x4_t) -> uint32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = ((a.0[i] as u64) << N).min(0xffff_ffff) as u32;
        i += 1;
    }
    uint32x4_t(r)
}

#[rustc_legacy_const_generics(1)]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vqshrun_n_s32<const N: i32>(a: int32x4_t) -> uint16x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = (a.0[i] >> N).max(0).min(65535) as u16;
        i += 1;
    }
    uint16x4_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vrev32q_u8(a: uint8x16_t) -> uint8x16_t {
    let mut r = [0; 16];
    let mut i = 0;
    while i < 16 {
        r[i] = a.0[(i & !3) + 3 - (i & 3)];
        i += 1;
    }
    uint8x16_t(r)
}

#[rustc_legacy_const_generics(2)]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vsetq_lane_u32<const LANE: i32>(a: u32, b: uint32x4_t) -> uint32x4_t {
    let mut r = b;
    r.0[LANE as usize] = a;
    r
}

// Hash state is [A, B, C, D]; wk already includes the SHA round constant.
#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "sha2"))]
pub fn vsha1cq_u32(hash_abcd: uint32x4_t, hash_e: u32, wk: uint32x4_t) -> uint32x4_t {
    unsafe {
        vx::vx_ternary(hash_abcd, hash_e, wk, 1)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "sha2"))]
pub fn vsha1h_u32(hash_e: u32) -> u32 {
    unsafe {
        vx::vx_sha1h(hash_e)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "sha2"))]
pub fn vsha1mq_u32(hash_abcd: uint32x4_t, hash_e: u32, wk: uint32x4_t) -> uint32x4_t {
    unsafe {
        vx::vx_ternary(hash_abcd, hash_e, wk, 3)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "sha2"))]
pub fn vsha1pq_u32(hash_abcd: uint32x4_t, hash_e: u32, wk: uint32x4_t) -> uint32x4_t {
    unsafe {
        vx::vx_ternary(hash_abcd, hash_e, wk, 2)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "sha2"))]
pub fn vsha1su0q_u32(w0_3: uint32x4_t, w4_7: uint32x4_t, w8_11: uint32x4_t) -> uint32x4_t {
    unsafe {
        vx::vx_ternary(w0_3, w4_7, w8_11, 4)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "sha2"))]
pub fn vsha1su1q_u32(tw0_3: uint32x4_t, w12_15: uint32x4_t) -> uint32x4_t {
    unsafe {
        vx::vx_binary(tw0_3, w12_15, 80, 32)
    }
}

#[rustc_legacy_const_generics(1)]
#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vshlq_n_u32<const N: i32>(a: uint32x4_t) -> uint32x4_t {
    unsafe {
        vx::vx_unary(a, 1, 32, N)
    }
}

#[rustc_legacy_const_generics(1)]
#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vshrq_n_s16<const N: i32>(a: int16x8_t) -> int16x8_t {
    unsafe {
        vx::vx_unary(a, 3, 16, N)
    }
}

#[rustc_legacy_const_generics(1)]
#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vshrq_n_s32<const N: i32>(a: int32x4_t) -> int32x4_t {
    unsafe {
        vx::vx_unary(a, 3, 32, N)
    }
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub unsafe fn vst1_u8(ptr: *mut u8, a: uint8x8_t) {
    let mut i = 0;
    while i < 8 {
        crate::ptr::write_unaligned(ptr.add(i), a.0[i]);
        i += 1;
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub unsafe fn vst1q_s16(a: *mut i16, b: int16x8_t) {
    vx::vx_store(a as *mut u8, b)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub unsafe fn vst1q_s16_x2(a: *mut i16, b: int16x8x2_t) {
    vst1q_s16(a, b.0);
    vst1q_s16(a.add(8), b.1);
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub unsafe fn vst1q_u32(a: *mut u32, b: uint32x4_t) {
    vx::vx_store(a as *mut u8, b)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub unsafe fn vst3q_u8(a: *mut u8, b: uint8x16x3_t) {
    let mut i = 0;
    while i < 16 {
        crate::ptr::write_unaligned(a.add(i * 3 + 0), b.0 .0[i]);
        crate::ptr::write_unaligned(a.add(i * 3 + 1), b.1 .0[i]);
        crate::ptr::write_unaligned(a.add(i * 3 + 2), b.2 .0[i]);
        i += 1;
    }
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
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

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vsubq_s16(a: int16x8_t, b: int16x8_t) -> int16x8_t {
    unsafe {
        vx::vx_binary(a, b, 1, 16)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vsubq_s32(a: int32x4_t, b: int32x4_t) -> int32x4_t {
    unsafe {
        vx::vx_binary(a, b, 1, 32)
    }
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vtrn1q_s64(a: int64x2_t, b: int64x2_t) -> int64x2_t {
    int64x2_t([a.0[0], b.0[0]])
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vtrn2q_s64(a: int64x2_t, b: int64x2_t) -> int64x2_t {
    int64x2_t([a.0[1], b.0[1]])
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vtrnq_s32(a: int32x4_t, b: int32x4_t) -> int32x4x2_t {
    int32x4x2_t(
        int32x4_t([a.0[0], b.0[0], a.0[2], b.0[2]]),
        int32x4_t([a.0[1], b.0[1], a.0[3], b.0[3]]),
    )
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vzip1q_s16(a: int16x8_t, b: int16x8_t) -> int16x8_t {
    unsafe {
        vx::vx_binary(a, b, 17, 16)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vzip2q_s16(a: int16x8_t, b: int16x8_t) -> int16x8_t {
    unsafe {
        vx::vx_binary(a, b, 18, 16)
    }
}

// ---- SHA-256 (FEAT_SHA256), as the Arm ARM pseudocode defines SHA256H / SHA256H2 / SHA256SU0 /
// SHA256SU1 (shared/functions/crypto SHA256hash, SHAchoose, SHAmajority, SHAhashSIGMA0/1).

/// SHA256H: hash_abcd updated by four rounds.
#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "sha2"))]
pub fn vsha256hq_u32(hash_abcd: uint32x4_t, hash_efgh: uint32x4_t, wk: uint32x4_t) -> uint32x4_t {
    unsafe {
        vx::vx_ternary(hash_abcd, hash_efgh, wk, 5)
    }
}

/// SHA256H2: hash_efgh updated by the same four rounds (hash_abcd = the pre-round abcd).
#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "sha2"))]
pub fn vsha256h2q_u32(hash_efgh: uint32x4_t, hash_abcd: uint32x4_t, wk: uint32x4_t) -> uint32x4_t {
    unsafe {
        vx::vx_ternary(hash_efgh, hash_abcd, wk, 6)
    }
}

/// SHA256SU0: message schedule, first part.
#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "sha2"))]
pub fn vsha256su0q_u32(w0_3: uint32x4_t, w4_7: uint32x4_t) -> uint32x4_t {
    unsafe {
        vx::vx_binary(w0_3, w4_7, 81, 32)
    }
}

/// SHA256SU1: message schedule, second part.
#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "sha2"))]
pub fn vsha256su1q_u32(tw0_3: uint32x4_t, w8_11: uint32x4_t, w12_15: uint32x4_t) -> uint32x4_t {
    unsafe {
        vx::vx_ternary(tw0_3, w8_11, w12_15, 7)
    }
}

// ---- SHA-512 and SHA-3 (FEAT_SHA512, FEAT_SHA3: target feature `sha3`), as the Arm ARM pseudocode
// defines SHA512H / SHA512H2 / SHA512SU0 / SHA512SU1 and EOR3 / BCAX / RAX1 / XAR. Lane 0 is the
// pseudocode's <63:0>, lane 1 its <127:64>; every addition wraps.

fn sha512_sum1(x: u64) -> u64 {
    x.rotate_right(14) ^ x.rotate_right(18) ^ x.rotate_right(41)
}

fn sha512_sum0(x: u64) -> u64 {
    x.rotate_right(28) ^ x.rotate_right(34) ^ x.rotate_right(39)
}

/// SHA512H (Qd = hash_ed, Qn, Vm): two rounds' Ch and Sigma1 part. The second round takes the
/// first's result as its e.
#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "sha3"))]
pub fn vsha512hq_u64(a: uint64x2_t, b: uint64x2_t, c: uint64x2_t) -> uint64x2_t {
    let (w, x, y) = (a.0, b.0, c.0);
    let hi = ((y[1] & x[0]) ^ (!y[1] & x[1])).wrapping_add(sha512_sum1(y[1])).wrapping_add(w[1]);
    let e = hi.wrapping_add(y[0]);
    let lo = ((e & y[1]) ^ (!e & x[0])).wrapping_add(sha512_sum1(e)).wrapping_add(w[0]);
    uint64x2_t([lo, hi])
}

/// SHA512H2 (Qd, Qn, Vm): two rounds' Maj and Sigma0 part. The second round takes the first's
/// result as its a.
#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "sha3"))]
pub fn vsha512h2q_u64(a: uint64x2_t, b: uint64x2_t, c: uint64x2_t) -> uint64x2_t {
    let (w, x, y) = (a.0, b.0, c.0);
    let hi = ((x[0] & y[1]) ^ (x[0] & y[0]) ^ (y[1] & y[0])).wrapping_add(sha512_sum0(y[0])).wrapping_add(w[1]);
    let lo = ((hi & y[0]) ^ (hi & y[1]) ^ (y[1] & y[0])).wrapping_add(sha512_sum0(hi)).wrapping_add(w[0]);
    uint64x2_t([lo, hi])
}

/// SHA512SU0 (Vd, Vn): message schedule, sigma0 of the next two words added.
#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "sha3"))]
pub fn vsha512su0q_u64(a: uint64x2_t, b: uint64x2_t) -> uint64x2_t {
    let sigma0 = |v: u64| v.rotate_right(1) ^ v.rotate_right(8) ^ (v >> 7);
    uint64x2_t([a.0[0].wrapping_add(sigma0(a.0[1])), a.0[1].wrapping_add(sigma0(b.0[0]))])
}

/// SHA512SU1 (Vd, Vn, Vm): message schedule, sigma1 of b and c added lane by lane.
#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "sha3"))]
pub fn vsha512su1q_u64(a: uint64x2_t, b: uint64x2_t, c: uint64x2_t) -> uint64x2_t {
    let sigma1 = |v: u64| v.rotate_right(19) ^ v.rotate_right(61) ^ (v >> 6);
    uint64x2_t([
        a.0[0].wrapping_add(sigma1(b.0[0])).wrapping_add(c.0[0]),
        a.0[1].wrapping_add(sigma1(b.0[1])).wrapping_add(c.0[1]),
    ])
}

/// RAX1: a ^ (b rotated left by one), per lane.
#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "sha3"))]
pub fn vrax1q_u64(a: uint64x2_t, b: uint64x2_t) -> uint64x2_t {
    uint64x2_t([a.0[0] ^ b.0[0].rotate_left(1), a.0[1] ^ b.0[1].rotate_left(1)])
}

/// XAR: (a ^ b) rotated right by IMM6, per lane.
#[inline]
#[rustc_legacy_const_generics(2)]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "sha3"))]
pub fn vxarq_u64<const IMM6: i32>(a: uint64x2_t, b: uint64x2_t) -> uint64x2_t {
    let r = IMM6 as u32;
    uint64x2_t([(a.0[0] ^ b.0[0]).rotate_right(r), (a.0[1] ^ b.0[1]).rotate_right(r)])
}

macro_rules! neon_sha3_bitwise {
    ($($eor3:ident $bcax:ident $t:ident;)*) => {$(
        /// EOR3: a ^ b ^ c.
        #[inline]
        #[cfg_attr(target_arch = "aarch64", target_feature(enable = "sha3"))]
        pub fn $eor3(a: $t, b: $t, c: $t) -> $t {
            let mut r = a;
            let mut i = 0;
            while i < r.0.len() {
                r.0[i] = a.0[i] ^ b.0[i] ^ c.0[i];
                i += 1;
            }
            r
        }

        /// BCAX: a ^ (b & !c).
        #[inline]
        #[cfg_attr(target_arch = "aarch64", target_feature(enable = "sha3"))]
        pub fn $bcax(a: $t, b: $t, c: $t) -> $t {
            let mut r = a;
            let mut i = 0;
            while i < r.0.len() {
                r.0[i] = a.0[i] ^ (b.0[i] & !c.0[i]);
                i += 1;
            }
            r
        }
    )*};
}

neon_sha3_bitwise! {
    veor3q_s8 vbcaxq_s8 int8x16_t;
    veor3q_s16 vbcaxq_s16 int16x8_t;
    veor3q_s32 vbcaxq_s32 int32x4_t;
    veor3q_s64 vbcaxq_s64 int64x2_t;
    veor3q_u8 vbcaxq_u8 uint8x16_t;
    veor3q_u16 vbcaxq_u16 uint16x8_t;
    veor3q_u32 vbcaxq_u32 uint32x4_t;
    veor3q_u64 vbcaxq_u64 uint64x2_t;
}

// ---- AES (FEAT_AES), as the Arm ARM pseudocode defines AESE / AESD / AESMC / AESIMC. A state is
// FIPS-197's in column-major memory order: byte i is row i % 4 of column i / 4. The S-boxes and
// MixColumns are x86_64.rs's (both modules compile on every target).

/// SubBytes(ShiftRows(data ^ key)), or their inverses.
fn neon_aes_sub_shift(data: uint8x16_t, key: uint8x16_t, inverse: bool) -> uint8x16_t {
    let mut r = [0; 16];
    let mut i = 0;
    while i < 16 {
        let row = i % 4;
        let column = if inverse { (i / 4 + 4 - row) % 4 } else { (i / 4 + row) % 4 };
        let from = column * 4 + row;
        r[i] = super::x86_64::AES_SBOXES[inverse as usize][(data.0[from] ^ key.0[from]) as usize];
        i += 1;
    }
    uint8x16_t(r)
}

/// AESE: SubBytes(ShiftRows(data ^ key)). The round's MixColumns is AESMC.
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "aes"))]
pub fn vaeseq_u8(data: uint8x16_t, key: uint8x16_t) -> uint8x16_t {
    neon_aes_sub_shift(data, key, false)
}

/// AESD: InvSubBytes(InvShiftRows(data ^ key)). The round's InvMixColumns is AESIMC.
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "aes"))]
pub fn vaesdq_u8(data: uint8x16_t, key: uint8x16_t) -> uint8x16_t {
    neon_aes_sub_shift(data, key, true)
}

/// AESMC: MixColumns.
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "aes"))]
pub fn vaesmcq_u8(data: uint8x16_t) -> uint8x16_t {
    uint8x16_t(super::x86_64::aes_mix_columns(data.0, false))
}

/// AESIMC: InvMixColumns.
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "aes"))]
pub fn vaesimcq_u8(data: uint8x16_t) -> uint8x16_t {
    uint8x16_t(super::x86_64::aes_mix_columns(data.0, true))
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

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vabsq_f32(a: float32x4_t) -> float32x4_t {
    unsafe {
        vx::vx_unary(a, 8, 32, 0)
    }
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vabsq_f64(a: float64x2_t) -> float64x2_t {
    let mut r = [0.0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = f64::from_bits(a.0[i].to_bits() & 0x7fffffffffffffff);
        i += 1;
    }
    float64x2_t(r)
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vabsq_s16(a: int16x8_t) -> int16x8_t {
    unsafe {
        vx::vx_unary(a, 0, 16, 0)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vabsq_s32(a: int32x4_t) -> int32x4_t {
    unsafe {
        vx::vx_unary(a, 0, 32, 0)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vabsq_s64(a: int64x2_t) -> int64x2_t {
    unsafe {
        vx::vx_unary(a, 0, 64, 0)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vabsq_s8(a: int8x16_t) -> int8x16_t {
    unsafe {
        vx::vx_unary(a, 0, 8, 0)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vaddq_f32(a: float32x4_t, b: float32x4_t) -> float32x4_t {
    unsafe {
        vx::vx_binary(a, b, 60, 32)
    }
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vaddq_f64(a: float64x2_t, b: float64x2_t) -> float64x2_t {
    let mut r = [0.0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = neon_result_f64(a.0[i], b.0[i], a.0[i] + b.0[i]);
        i += 1;
    }
    float64x2_t(r)
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vaddq_s64(a: int64x2_t, b: int64x2_t) -> int64x2_t {
    unsafe {
        vx::vx_binary(a, b, 0, 64)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vaddq_s8(a: int8x16_t, b: int8x16_t) -> int8x16_t {
    unsafe {
        vx::vx_binary(a, b, 0, 8)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vaddq_u16(a: uint16x8_t, b: uint16x8_t) -> uint16x8_t {
    unsafe {
        vx::vx_binary(a, b, 0, 16)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vaddq_u64(a: uint64x2_t, b: uint64x2_t) -> uint64x2_t {
    unsafe {
        vx::vx_binary(a, b, 0, 64)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vaddq_u8(a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    unsafe {
        vx::vx_binary(a, b, 0, 8)
    }
}

// Widening add across the vector (stdarch vaddlv_u8 / vaddlvq_u8). The sum of the lanes is at
// most 8 * 255 = 2040, or 16 * 255 = 4080 for the 128-bit form, so the u16 accumulator cannot
// overflow.
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vaddlv_u8(a: uint8x8_t) -> u16 {
    let mut r = 0u16;
    let mut i = 0;
    while i < 8 {
        r += a.0[i] as u16;
        i += 1;
    }
    r
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vaddlvq_u8(a: uint8x16_t) -> u16 {
    let mut r = 0u16;
    let mut i = 0;
    while i < 16 {
        r += a.0[i] as u16;
        i += 1;
    }
    r
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vaddvq_f32(a: float32x4_t) -> f32 {
    neon_add_f32(neon_add_f32(a.0[0], a.0[1]), neon_add_f32(a.0[2], a.0[3]))
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vaddvq_s16(a: int16x8_t) -> i16 {
    let mut r = a.0[0];
    let mut i = 1;
    while i < 8 {
        r = r.wrapping_add(a.0[i]);
        i += 1;
    }
    r
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vaddvq_s32(a: int32x4_t) -> i32 {
    let mut r = a.0[0];
    let mut i = 1;
    while i < 4 {
        r = r.wrapping_add(a.0[i]);
        i += 1;
    }
    r
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vaddvq_u16(a: uint16x8_t) -> u16 {
    let mut r = a.0[0];
    let mut i = 1;
    while i < 8 {
        r = r.wrapping_add(a.0[i]);
        i += 1;
    }
    r
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vandq_s16(a: int16x8_t, b: int16x8_t) -> int16x8_t {
    unsafe {
        vx::vx_binary(a, b, 6, 16)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vandq_s32(a: int32x4_t, b: int32x4_t) -> int32x4_t {
    unsafe {
        vx::vx_binary(a, b, 6, 32)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vandq_s64(a: int64x2_t, b: int64x2_t) -> int64x2_t {
    unsafe {
        vx::vx_binary(a, b, 6, 64)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vandq_s8(a: int8x16_t, b: int8x16_t) -> int8x16_t {
    unsafe {
        vx::vx_binary(a, b, 6, 8)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vandq_u16(a: uint16x8_t, b: uint16x8_t) -> uint16x8_t {
    unsafe {
        vx::vx_binary(a, b, 6, 16)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vandq_u32(a: uint32x4_t, b: uint32x4_t) -> uint32x4_t {
    unsafe {
        vx::vx_binary(a, b, 6, 32)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vandq_u64(a: uint64x2_t, b: uint64x2_t) -> uint64x2_t {
    unsafe {
        vx::vx_binary(a, b, 6, 64)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vandq_u8(a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    unsafe {
        vx::vx_binary(a, b, 6, 8)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vbicq_u32(a: uint32x4_t, b: uint32x4_t) -> uint32x4_t {
    unsafe {
        vx::vx_binary(a, b, 9, 32)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vbslq_f32(a: uint32x4_t, b: float32x4_t, c: float32x4_t) -> float32x4_t {
    unsafe {
        vx::vx_ternary(crate::ptr::read_unaligned(&a as *const uint32x4_t as *const float32x4_t), b, c, 10)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vbslq_f64(a: uint64x2_t, b: float64x2_t, c: float64x2_t) -> float64x2_t {
    unsafe {
        vx::vx_ternary(crate::ptr::read_unaligned(&a as *const uint64x2_t as *const float64x2_t), b, c, 10)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vbslq_s16(a: uint16x8_t, b: int16x8_t, c: int16x8_t) -> int16x8_t {
    unsafe {
        vx::vx_ternary(crate::ptr::read_unaligned(&a as *const uint16x8_t as *const int16x8_t), b, c, 10)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vbslq_s32(a: uint32x4_t, b: int32x4_t, c: int32x4_t) -> int32x4_t {
    unsafe {
        vx::vx_ternary(crate::ptr::read_unaligned(&a as *const uint32x4_t as *const int32x4_t), b, c, 10)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vbslq_s64(a: uint64x2_t, b: int64x2_t, c: int64x2_t) -> int64x2_t {
    unsafe {
        vx::vx_ternary(crate::ptr::read_unaligned(&a as *const uint64x2_t as *const int64x2_t), b, c, 10)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vbslq_s8(a: uint8x16_t, b: int8x16_t, c: int8x16_t) -> int8x16_t {
    unsafe {
        vx::vx_ternary(crate::ptr::read_unaligned(&a as *const uint8x16_t as *const int8x16_t), b, c, 10)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vbslq_u16(a: uint16x8_t, b: uint16x8_t, c: uint16x8_t) -> uint16x8_t {
    unsafe {
        vx::vx_ternary(a, b, c, 10)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vbslq_u32(a: uint32x4_t, b: uint32x4_t, c: uint32x4_t) -> uint32x4_t {
    unsafe {
        vx::vx_ternary(a, b, c, 10)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vbslq_u64(a: uint64x2_t, b: uint64x2_t, c: uint64x2_t) -> uint64x2_t {
    unsafe {
        vx::vx_ternary(a, b, c, 10)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vbslq_u8(a: uint8x16_t, b: uint8x16_t, c: uint8x16_t) -> uint8x16_t {
    unsafe {
        vx::vx_ternary(a, b, c, 10)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vceqq_f32(a: float32x4_t, b: float32x4_t) -> uint32x4_t {
    unsafe {
        vx::vx_binary(a, b, 46, 32)
    }
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vceqq_f64(a: float64x2_t, b: float64x2_t) -> uint64x2_t {
    let mut r = [0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = if a.0[i] == b.0[i] { u64::MAX } else { 0 };
        i += 1;
    }
    uint64x2_t(r)
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vceqq_s16(a: int16x8_t, b: int16x8_t) -> uint16x8_t {
    unsafe {
        vx::vx_binary(a, b, 10, 16)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vceqq_s32(a: int32x4_t, b: int32x4_t) -> uint32x4_t {
    unsafe {
        vx::vx_binary(a, b, 10, 32)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vceqq_s64(a: int64x2_t, b: int64x2_t) -> uint64x2_t {
    unsafe {
        vx::vx_binary(a, b, 10, 64)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vceqq_s8(a: int8x16_t, b: int8x16_t) -> uint8x16_t {
    unsafe {
        vx::vx_binary(a, b, 10, 8)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vceqq_u16(a: uint16x8_t, b: uint16x8_t) -> uint16x8_t {
    unsafe {
        vx::vx_binary(a, b, 10, 16)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vceqq_u32(a: uint32x4_t, b: uint32x4_t) -> uint32x4_t {
    unsafe {
        vx::vx_binary(a, b, 10, 32)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vceqq_u64(a: uint64x2_t, b: uint64x2_t) -> uint64x2_t {
    unsafe {
        vx::vx_binary(a, b, 10, 64)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vceqq_u8(a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    unsafe {
        vx::vx_binary(a, b, 10, 8)
    }
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcgeq_f32(a: float32x4_t, b: float32x4_t) -> uint32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = if a.0[i] >= b.0[i] { u32::MAX } else { 0 };
        i += 1;
    }
    uint32x4_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcgeq_f64(a: float64x2_t, b: float64x2_t) -> uint64x2_t {
    let mut r = [0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = if a.0[i] >= b.0[i] { u64::MAX } else { 0 };
        i += 1;
    }
    uint64x2_t(r)
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcgtq_f32(a: float32x4_t, b: float32x4_t) -> uint32x4_t {
    unsafe {
        vx::vx_binary(a, b, 50, 32)
    }
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcgtq_f64(a: float64x2_t, b: float64x2_t) -> uint64x2_t {
    let mut r = [0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = if a.0[i] > b.0[i] { u64::MAX } else { 0 };
        i += 1;
    }
    uint64x2_t(r)
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcgtq_s16(a: int16x8_t, b: int16x8_t) -> uint16x8_t {
    unsafe {
        vx::vx_binary(a, b, 11, 16)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcgtq_s32(a: int32x4_t, b: int32x4_t) -> uint32x4_t {
    unsafe {
        vx::vx_binary(a, b, 11, 32)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcgtq_s64(a: int64x2_t, b: int64x2_t) -> uint64x2_t {
    unsafe {
        vx::vx_binary(a, b, 11, 64)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcgtq_s8(a: int8x16_t, b: int8x16_t) -> uint8x16_t {
    unsafe {
        vx::vx_binary(a, b, 11, 8)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcgtq_u32(a: uint32x4_t, b: uint32x4_t) -> uint32x4_t {
    unsafe {
        vx::vx_binary(a, b, 29, 32)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcgtq_u64(a: uint64x2_t, b: uint64x2_t) -> uint64x2_t {
    unsafe {
        vx::vx_binary(a, b, 29, 64)
    }
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcleq_f32(a: float32x4_t, b: float32x4_t) -> uint32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = if a.0[i] <= b.0[i] { u32::MAX } else { 0 };
        i += 1;
    }
    uint32x4_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcleq_f64(a: float64x2_t, b: float64x2_t) -> uint64x2_t {
    let mut r = [0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = if a.0[i] <= b.0[i] { u64::MAX } else { 0 };
        i += 1;
    }
    uint64x2_t(r)
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcltq_f32(a: float32x4_t, b: float32x4_t) -> uint32x4_t {
    unsafe {
        vx::vx_binary(a, b, 48, 32)
    }
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcltq_f64(a: float64x2_t, b: float64x2_t) -> uint64x2_t {
    let mut r = [0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = if a.0[i] < b.0[i] { u64::MAX } else { 0 };
        i += 1;
    }
    uint64x2_t(r)
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcltq_s16(a: int16x8_t, b: int16x8_t) -> uint16x8_t {
    unsafe {
        vx::vx_binary(b, a, 11, 16)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcltq_s32(a: int32x4_t, b: int32x4_t) -> uint32x4_t {
    unsafe {
        vx::vx_binary(b, a, 11, 32)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcltq_s64(a: int64x2_t, b: int64x2_t) -> uint64x2_t {
    unsafe {
        vx::vx_binary(b, a, 11, 64)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcltq_s8(a: int8x16_t, b: int8x16_t) -> uint8x16_t {
    unsafe {
        vx::vx_binary(b, a, 11, 8)
    }
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcombine_s8(a: int8x8_t, b: int8x8_t) -> int8x16_t {
    let mut r = [0; 16];
    let mut i = 0;
    while i < 16 {
        r[i] = if i < 8 { a.0[i] } else { b.0[i - 8] };
        i += 1;
    }
    int8x16_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcombine_u32(a: uint32x2_t, b: uint32x2_t) -> uint32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = if i < 2 { a.0[i] } else { b.0[i - 2] };
        i += 1;
    }
    uint32x4_t(r)
}

/// FCVTAU: to nearest, ties away from zero.
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcvtaq_u32_f32(a: float32x4_t) -> uint32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = a.0[i].round() as u32;
        i += 1;
    }
    uint32x4_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcvtnq_s32_f32(a: float32x4_t) -> int32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = a.0[i].round_ties_even() as i32;
        i += 1;
    }
    int32x4_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcvtq_f32_s32(a: int32x4_t) -> float32x4_t {
    let mut r = [0.0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = a.0[i] as f32;
        i += 1;
    }
    float32x4_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcvtq_f32_u32(a: uint32x4_t) -> float32x4_t {
    let mut r = [0.0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = a.0[i] as f32;
        i += 1;
    }
    float32x4_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcvtq_f64_s64(a: int64x2_t) -> float64x2_t {
    let mut r = [0.0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = a.0[i] as f64;
        i += 1;
    }
    float64x2_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcvtq_s32_f32(a: float32x4_t) -> int32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = a.0[i] as i32;
        i += 1;
    }
    int32x4_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcvtq_s64_f64(a: float64x2_t) -> int64x2_t {
    let mut r = [0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = a.0[i] as i64;
        i += 1;
    }
    int64x2_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcvtq_u32_f32(a: float32x4_t) -> uint32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = a.0[i] as u32;
        i += 1;
    }
    uint32x4_t(r)
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vdivq_f32(a: float32x4_t, b: float32x4_t) -> float32x4_t {
    unsafe {
        vx::vx_binary(a, b, 63, 32)
    }
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vdivq_f64(a: float64x2_t, b: float64x2_t) -> float64x2_t {
    let mut r = [0.0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = neon_result_f64(a.0[i], b.0[i], a.0[i] / b.0[i]);
        i += 1;
    }
    float64x2_t(r)
}

#[rustc_legacy_const_generics(1)]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vdupq_laneq_f32<const N: i32>(a: float32x4_t) -> float32x4_t {
    float32x4_t([a.0[N as usize]; 4])
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vdupq_n_f32(value: f32) -> float32x4_t {
    unsafe {
        vx::vx_splat(value.to_bits() as u64, 32)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vdupq_n_f64(value: f64) -> float64x2_t {
    unsafe {
        vx::vx_splat(value.to_bits() as u64, 64)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vdupq_n_s64(value: i64) -> int64x2_t {
    unsafe {
        vx::vx_splat(value as u64, 64)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vdupq_n_s8(value: i8) -> int8x16_t {
    unsafe {
        vx::vx_splat(value as u64, 8)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vdupq_n_u64(value: u64) -> uint64x2_t {
    unsafe {
        vx::vx_splat(value as u64, 64)
    }
}

#[rustc_legacy_const_generics(1)]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vdups_laneq_f32<const N: i32>(a: float32x4_t) -> f32 {
    a.0[N as usize]
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn veorq_s16(a: int16x8_t, b: int16x8_t) -> int16x8_t {
    unsafe {
        vx::vx_binary(a, b, 8, 16)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn veorq_s32(a: int32x4_t, b: int32x4_t) -> int32x4_t {
    unsafe {
        vx::vx_binary(a, b, 8, 32)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn veorq_s64(a: int64x2_t, b: int64x2_t) -> int64x2_t {
    unsafe {
        vx::vx_binary(a, b, 8, 64)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn veorq_s8(a: int8x16_t, b: int8x16_t) -> int8x16_t {
    unsafe {
        vx::vx_binary(a, b, 8, 8)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn veorq_u16(a: uint16x8_t, b: uint16x8_t) -> uint16x8_t {
    unsafe {
        vx::vx_binary(a, b, 8, 16)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn veorq_u32(a: uint32x4_t, b: uint32x4_t) -> uint32x4_t {
    unsafe {
        vx::vx_binary(a, b, 8, 32)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn veorq_u64(a: uint64x2_t, b: uint64x2_t) -> uint64x2_t {
    unsafe {
        vx::vx_binary(a, b, 8, 64)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn veorq_u8(a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    unsafe {
        vx::vx_binary(a, b, 8, 8)
    }
}

// a + b * c (the indexed forms below multiply b by a lane of c).
#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vfmaq_f32(a: float32x4_t, b: float32x4_t, c: float32x4_t) -> float32x4_t {
    // FMLA with c as its first multiplicand, as rustc's: a NaN in c takes precedence over one in b
    unsafe {
        vx::vx_ternary(a, c, b, 8)
    }
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vfmaq_f64(a: float64x2_t, b: float64x2_t, c: float64x2_t) -> float64x2_t {
    let mut r = [0.0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = neon_fma_f64(a.0[i], c.0[i], b.0[i]);
        i += 1;
    }
    float64x2_t(r)
}

#[rustc_legacy_const_generics(3)]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vfmaq_laneq_f32<const LANE: i32>(
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

#[rustc_legacy_const_generics(3)]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vfmaq_laneq_f64<const LANE: i32>(
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

// a - b * c: FMLS with c as its (negated) first multiplicand, as rustc's: a NaN in c takes
// precedence over one in b and comes out with its sign flipped
#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vfmsq_f32(a: float32x4_t, b: float32x4_t, c: float32x4_t) -> float32x4_t {
    unsafe {
        vx::vx_ternary(a, c, b, 9)
    }
}

// a - b * c: FMLS with c as its (negated) first multiplicand, as rustc's and vfmsq_f32: a NaN in
// c takes precedence over one in b and comes out with its sign flipped, one in b keeps its sign
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vfmsq_f64(a: float64x2_t, b: float64x2_t, c: float64x2_t) -> float64x2_t {
    let mut r = [0.0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = neon_fma_f64(a.0[i], -c.0[i], b.0[i]);
        i += 1;
    }
    float64x2_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vget_high_s16(a: int16x8_t) -> int16x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = a.0[i + 4];
        i += 1;
    }
    int16x4_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vget_high_s32(a: int32x4_t) -> int32x2_t {
    let mut r = [0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = a.0[i + 2];
        i += 1;
    }
    int32x2_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vget_high_s8(a: int8x16_t) -> int8x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = a.0[i + 8];
        i += 1;
    }
    int8x8_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vget_low_s32(a: int32x4_t) -> int32x2_t {
    let mut r = [0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = a.0[i + 0];
        i += 1;
    }
    int32x2_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vget_low_s8(a: int8x16_t) -> int8x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = a.0[i + 0];
        i += 1;
    }
    int8x8_t(r)
}

#[rustc_legacy_const_generics(1)]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vgetq_lane_f32<const IMM5: i32>(v: float32x4_t) -> f32 {
    v.0[IMM5 as usize]
}

#[rustc_legacy_const_generics(1)]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vgetq_lane_f64<const IMM5: i32>(a: float64x2_t) -> f64 {
    a.0[IMM5 as usize]
}

#[rustc_legacy_const_generics(1)]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vgetq_lane_s64<const IMM5: i32>(v: int64x2_t) -> i64 {
    v.0[IMM5 as usize]
}

#[rustc_legacy_const_generics(1)]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vgetq_lane_u64<const IMM5: i32>(v: uint64x2_t) -> u64 {
    v.0[IMM5 as usize]
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub unsafe fn vld1_s8(ptr: *const i8) -> int8x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = crate::ptr::read_unaligned(ptr.add(i));
        i += 1;
    }
    int8x8_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub unsafe fn vld1q_dup_f32(ptr: *const f32) -> float32x4_t {
    float32x4_t([crate::ptr::read_unaligned(ptr); 4])
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub unsafe fn vld1q_dup_f64(ptr: *const f64) -> float64x2_t {
    float64x2_t([crate::ptr::read_unaligned(ptr); 2])
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub unsafe fn vld1q_f32(ptr: *const f32) -> float32x4_t {
    vx::vx_load(ptr as *const u8)
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub unsafe fn vld1q_f64(ptr: *const f64) -> float64x2_t {
    vx::vx_load(ptr as *const u8)
}

#[rustc_legacy_const_generics(2)]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub unsafe fn vld1q_lane_f32<const LANE: i32>(ptr: *const f32, src: float32x4_t) -> float32x4_t {
    let mut r = src;
    r.0[LANE as usize] = crate::ptr::read_unaligned(ptr);
    r
}

#[rustc_legacy_const_generics(2)]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub unsafe fn vld1q_lane_f64<const LANE: i32>(ptr: *const f64, src: float64x2_t) -> float64x2_t {
    let mut r = src;
    r.0[LANE as usize] = crate::ptr::read_unaligned(ptr);
    r
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub unsafe fn vld1q_s32(a: *const i32) -> int32x4_t {
    vx::vx_load(a as *const u8)
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub unsafe fn vld1q_s8(a: *const i8) -> int8x16_t {
    vx::vx_load(a as *const u8)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmaxnmq_f32(a: float32x4_t, b: float32x4_t) -> float32x4_t {
    let mut r = [0.0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = neon_minmax_f32(a.0[i], b.0[i], true, true);
        i += 1;
    }
    float32x4_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmaxnmq_f64(a: float64x2_t, b: float64x2_t) -> float64x2_t {
    let mut r = [0.0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = neon_minmax_f64(a.0[i], b.0[i], true, true);
        i += 1;
    }
    float64x2_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmaxnmvq_f32(a: float32x4_t) -> f32 {
    neon_minmax_f32(
        neon_minmax_f32(a.0[0], a.0[1], true, true),
        neon_minmax_f32(a.0[2], a.0[3], true, true),
        true,
        true,
    )
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmaxq_f32(a: float32x4_t, b: float32x4_t) -> float32x4_t {
    unsafe {
        vx::vx_binary(a, b, 53, 32)
    }
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmaxq_f64(a: float64x2_t, b: float64x2_t) -> float64x2_t {
    let mut r = [0.0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = neon_minmax_f64(a.0[i], b.0[i], true, false);
        i += 1;
    }
    float64x2_t(r)
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmaxq_s8(a: int8x16_t, b: int8x16_t) -> int8x16_t {
    unsafe {
        vx::vx_binary(a, b, 14, 8)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmaxq_u16(a: uint16x8_t, b: uint16x8_t) -> uint16x8_t {
    unsafe {
        vx::vx_binary(a, b, 15, 16)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmaxq_u32(a: uint32x4_t, b: uint32x4_t) -> uint32x4_t {
    unsafe {
        vx::vx_binary(a, b, 15, 32)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmaxq_u8(a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    unsafe {
        vx::vx_binary(a, b, 15, 8)
    }
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmaxvq_s16(a: int16x8_t) -> i16 {
    let mut r = a.0[0];
    let mut i = 1;
    while i < 8 {
        r = r.max(a.0[i]);
        i += 1;
    }
    r
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmaxvq_s32(a: int32x4_t) -> i32 {
    let mut r = a.0[0];
    let mut i = 1;
    while i < 4 {
        r = r.max(a.0[i]);
        i += 1;
    }
    r
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmaxvq_s8(a: int8x16_t) -> i8 {
    let mut r = a.0[0];
    let mut i = 1;
    while i < 16 {
        r = r.max(a.0[i]);
        i += 1;
    }
    r
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmaxvq_u16(a: uint16x8_t) -> u16 {
    let mut r = a.0[0];
    let mut i = 1;
    while i < 8 {
        r = r.max(a.0[i]);
        i += 1;
    }
    r
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmaxvq_u8(a: uint8x16_t) -> u8 {
    let mut r = a.0[0];
    let mut i = 1;
    while i < 16 {
        r = r.max(a.0[i]);
        i += 1;
    }
    r
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vminnmq_f32(a: float32x4_t, b: float32x4_t) -> float32x4_t {
    let mut r = [0.0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = neon_minmax_f32(a.0[i], b.0[i], false, true);
        i += 1;
    }
    float32x4_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vminnmq_f64(a: float64x2_t, b: float64x2_t) -> float64x2_t {
    let mut r = [0.0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = neon_minmax_f64(a.0[i], b.0[i], false, true);
        i += 1;
    }
    float64x2_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vminnmvq_f32(a: float32x4_t) -> f32 {
    neon_minmax_f32(
        neon_minmax_f32(a.0[0], a.0[1], false, true),
        neon_minmax_f32(a.0[2], a.0[3], false, true),
        false,
        true,
    )
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vminq_f32(a: float32x4_t, b: float32x4_t) -> float32x4_t {
    unsafe {
        vx::vx_binary(a, b, 52, 32)
    }
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vminq_f64(a: float64x2_t, b: float64x2_t) -> float64x2_t {
    let mut r = [0.0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = neon_minmax_f64(a.0[i], b.0[i], false, false);
        i += 1;
    }
    float64x2_t(r)
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vminq_s32(a: int32x4_t, b: int32x4_t) -> int32x4_t {
    unsafe {
        vx::vx_binary(a, b, 12, 32)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vminq_s8(a: int8x16_t, b: int8x16_t) -> int8x16_t {
    unsafe {
        vx::vx_binary(a, b, 12, 8)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vminq_u16(a: uint16x8_t, b: uint16x8_t) -> uint16x8_t {
    unsafe {
        vx::vx_binary(a, b, 13, 16)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vminq_u32(a: uint32x4_t, b: uint32x4_t) -> uint32x4_t {
    unsafe {
        vx::vx_binary(a, b, 13, 32)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vminq_u8(a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    unsafe {
        vx::vx_binary(a, b, 13, 8)
    }
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vminvq_s16(a: int16x8_t) -> i16 {
    let mut r = a.0[0];
    let mut i = 1;
    while i < 8 {
        r = r.min(a.0[i]);
        i += 1;
    }
    r
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vminvq_s32(a: int32x4_t) -> i32 {
    let mut r = a.0[0];
    let mut i = 1;
    while i < 4 {
        r = r.min(a.0[i]);
        i += 1;
    }
    r
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vminvq_s8(a: int8x16_t) -> i8 {
    let mut r = a.0[0];
    let mut i = 1;
    while i < 16 {
        r = r.min(a.0[i]);
        i += 1;
    }
    r
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vminvq_u16(a: uint16x8_t) -> u16 {
    let mut r = a.0[0];
    let mut i = 1;
    while i < 8 {
        r = r.min(a.0[i]);
        i += 1;
    }
    r
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vminvq_u32(a: uint32x4_t) -> u32 {
    let mut r = a.0[0];
    let mut i = 1;
    while i < 4 {
        r = r.min(a.0[i]);
        i += 1;
    }
    r
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vminvq_u8(a: uint8x16_t) -> u8 {
    let mut r = a.0[0];
    let mut i = 1;
    while i < 16 {
        r = r.min(a.0[i]);
        i += 1;
    }
    r
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmlaq_s32(a: int32x4_t, b: int32x4_t, c: int32x4_t) -> int32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = a.0[i].wrapping_add(b.0[i].wrapping_mul(c.0[i]));
        i += 1;
    }
    int32x4_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmlsq_f32(a: float32x4_t, b: float32x4_t, c: float32x4_t) -> float32x4_t {
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

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmovl_s16(a: int16x4_t) -> int32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = a.0[i] as i32;
        i += 1;
    }
    int32x4_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmovl_s32(a: int32x2_t) -> int64x2_t {
    let mut r = [0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = a.0[i] as i64;
        i += 1;
    }
    int64x2_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmovl_s8(a: int8x8_t) -> int16x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = a.0[i] as i16;
        i += 1;
    }
    int16x8_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmovq_n_f32(value: f32) -> float32x4_t {
    float32x4_t([value; 4])
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmovq_n_f64(value: f64) -> float64x2_t {
    float64x2_t([value; 2])
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmovq_n_s16(value: i16) -> int16x8_t {
    int16x8_t([value; 8])
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmovq_n_s32(value: i32) -> int32x4_t {
    int32x4_t([value; 4])
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmovq_n_s64(value: i64) -> int64x2_t {
    int64x2_t([value; 2])
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmovq_n_u32(value: u32) -> uint32x4_t {
    uint32x4_t([value; 4])
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmull_high_s16(a: int16x8_t, b: int16x8_t) -> int32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = (a.0[i + 4] as i32) * (b.0[i + 4] as i32);
        i += 1;
    }
    int32x4_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmull_s16(a: int16x4_t, b: int16x4_t) -> int32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = (a.0[i] as i32) * (b.0[i] as i32);
        i += 1;
    }
    int32x4_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmull_s32(a: int32x2_t, b: int32x2_t) -> int64x2_t {
    let mut r = [0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = (a.0[i] as i64) * (b.0[i] as i64);
        i += 1;
    }
    int64x2_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmull_u16(a: uint16x4_t, b: uint16x4_t) -> uint32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = (a.0[i] as u32) * (b.0[i] as u32);
        i += 1;
    }
    uint32x4_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmull_u32(a: uint32x2_t, b: uint32x2_t) -> uint64x2_t {
    let mut r = [0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = (a.0[i] as u64) * (b.0[i] as u64);
        i += 1;
    }
    uint64x2_t(r)
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmulq_f32(a: float32x4_t, b: float32x4_t) -> float32x4_t {
    unsafe {
        vx::vx_binary(a, b, 62, 32)
    }
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmulq_f64(a: float64x2_t, b: float64x2_t) -> float64x2_t {
    let mut r = [0.0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = neon_result_f64(a.0[i], b.0[i], a.0[i] * b.0[i]);
        i += 1;
    }
    float64x2_t(r)
}

#[rustc_legacy_const_generics(2)]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmulq_laneq_f32<const LANE: i32>(a: float32x4_t, b: float32x4_t) -> float32x4_t {
    let mut r = [0.0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = neon_result_f32(a.0[i], b.0[LANE as usize], a.0[i] * b.0[LANE as usize]);
        i += 1;
    }
    float32x4_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmulq_n_f32(a: float32x4_t, b: f32) -> float32x4_t {
    let mut r = [0.0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = neon_result_f32(a.0[i], b, a.0[i] * b);
        i += 1;
    }
    float32x4_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmulq_n_f64(a: float64x2_t, b: f64) -> float64x2_t {
    let mut r = [0.0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = neon_result_f64(a.0[i], b, a.0[i] * b);
        i += 1;
    }
    float64x2_t(r)
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmulq_u16(a: uint16x8_t, b: uint16x8_t) -> uint16x8_t {
    unsafe {
        vx::vx_binary(a, b, 26, 16)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmulq_u32(a: uint32x4_t, b: uint32x4_t) -> uint32x4_t {
    unsafe {
        vx::vx_binary(a, b, 26, 32)
    }
}

#[rustc_legacy_const_generics(2)]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmuls_laneq_f32<const LANE: i32>(a: f32, b: float32x4_t) -> f32 {
    neon_mul_f32(a, b.0[LANE as usize])
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmvnq_u32(a: uint32x4_t) -> uint32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = !a.0[i];
        i += 1;
    }
    uint32x4_t(r)
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vnegq_f32(a: float32x4_t) -> float32x4_t {
    unsafe {
        vx::vx_unary(a, 10, 32, 0)
    }
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vnegq_s32(a: int32x4_t) -> int32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = a.0[i].wrapping_neg();
        i += 1;
    }
    int32x4_t(r)
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vorrq_s16(a: int16x8_t, b: int16x8_t) -> int16x8_t {
    unsafe {
        vx::vx_binary(a, b, 7, 16)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vorrq_s64(a: int64x2_t, b: int64x2_t) -> int64x2_t {
    unsafe {
        vx::vx_binary(a, b, 7, 64)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vorrq_s8(a: int8x16_t, b: int8x16_t) -> int8x16_t {
    unsafe {
        vx::vx_binary(a, b, 7, 8)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vorrq_u16(a: uint16x8_t, b: uint16x8_t) -> uint16x8_t {
    unsafe {
        vx::vx_binary(a, b, 7, 16)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vorrq_u32(a: uint32x4_t, b: uint32x4_t) -> uint32x4_t {
    unsafe {
        vx::vx_binary(a, b, 7, 32)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vorrq_u64(a: uint64x2_t, b: uint64x2_t) -> uint64x2_t {
    unsafe {
        vx::vx_binary(a, b, 7, 64)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vorrq_u8(a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    unsafe {
        vx::vx_binary(a, b, 7, 8)
    }
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vpaddlq_s16(a: int16x8_t) -> int32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = a.0[i * 2] as i32 + a.0[i * 2 + 1] as i32;
        i += 1;
    }
    int32x4_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vpaddq_f32(a: float32x4_t, b: float32x4_t) -> float32x4_t {
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

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vpaddq_s32(a: int32x4_t, b: int32x4_t) -> int32x4_t {
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

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vpaddq_u8(a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    let mut r = [0; 16];
    let mut i = 0;
    while i < 16 {
        r[i] = if i < 8 {
            a.0[i * 2].wrapping_add(a.0[i * 2 + 1])
        } else {
            b.0[(i - 8) * 2].wrapping_add(b.0[(i - 8) * 2 + 1])
        };
        i += 1;
    }
    uint8x16_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vpmaxq_u8(a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
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

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vqaddq_s16(a: int16x8_t, b: int16x8_t) -> int16x8_t {
    unsafe {
        vx::vx_binary(a, b, 2, 16)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vqaddq_s8(a: int8x16_t, b: int8x16_t) -> int8x16_t {
    unsafe {
        vx::vx_binary(a, b, 2, 8)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vqaddq_u16(a: uint16x8_t, b: uint16x8_t) -> uint16x8_t {
    unsafe {
        vx::vx_binary(a, b, 3, 16)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vqaddq_u8(a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    unsafe {
        vx::vx_binary(a, b, 3, 8)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vqmovn_s16(a: int16x8_t) -> int8x8_t {
    unsafe {
        vx::vx_narrow(a, 20)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vqmovun_s16(a: int16x8_t) -> uint8x8_t {
    unsafe {
        vx::vx_narrow(a, 19)
    }
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vqmovun_s32(a: int32x4_t) -> uint16x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = a.0[i].max(0).min(65535) as u16;
        i += 1;
    }
    uint16x4_t(r)
}

/// SQRDMLAH / SQRDMLSH (FEAT_RDM) on one i16 lane: the high half of (a << 16) + 2 * b * c (minus
/// for SQRDMLSH), rounded, saturated once at the end.
fn neon_qrdmlah_s16(a: i16, b: i16, c: i16, subtract: bool) -> i16 {
    let product = b as i64 * c as i64 * 2;
    let sum = ((a as i64) << 16) + if subtract { -product } else { product } + (1 << 15);
    (sum >> 16).max(i16::MIN as i64).min(i16::MAX as i64) as i16
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "rdm"))]
pub fn vqrdmlah_s16(a: int16x4_t, b: int16x4_t, c: int16x4_t) -> int16x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = neon_qrdmlah_s16(a.0[i], b.0[i], c.0[i], false);
        i += 1;
    }
    int16x4_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "rdm"))]
pub fn vqrdmlahq_s16(a: int16x8_t, b: int16x8_t, c: int16x8_t) -> int16x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = neon_qrdmlah_s16(a.0[i], b.0[i], c.0[i], false);
        i += 1;
    }
    int16x8_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "rdm"))]
pub fn vqrdmlahq_s32(a: int32x4_t, b: int32x4_t, c: int32x4_t) -> int32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = ((((a.0[i] as i128) << 32) + b.0[i] as i128 * c.0[i] as i128 * 2 + (1 << 31)) >> 32)
            .max(i32::MIN as i128)
            .min(i32::MAX as i128) as i32;
        i += 1;
    }
    int32x4_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "rdm"))]
pub fn vqrdmlsh_s16(a: int16x4_t, b: int16x4_t, c: int16x4_t) -> int16x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = neon_qrdmlah_s16(a.0[i], b.0[i], c.0[i], true);
        i += 1;
    }
    int16x4_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "rdm"))]
pub fn vqrdmlshq_s16(a: int16x8_t, b: int16x8_t, c: int16x8_t) -> int16x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = neon_qrdmlah_s16(a.0[i], b.0[i], c.0[i], true);
        i += 1;
    }
    int16x8_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vqrdmulhq_n_s16(a: int16x8_t, b: i16) -> int16x8_t {
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

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vqrdmulhq_s16(a: int16x8_t, b: int16x8_t) -> int16x8_t {
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

/// SQRDMLAH / SQRDMLSH (FEAT_RDM) on one i32 lane: the high half of (a << 32) + 2 * b * c (minus
/// for SQRDMLSH), rounded, saturated once at the end.
fn neon_qrdmlah_s32(a: i32, b: i32, c: i32, subtract: bool) -> i32 {
    let product = b as i128 * c as i128 * 2;
    let sum = ((a as i128) << 32) + if subtract { -product } else { product } + (1 << 31);
    (sum >> 32).max(i32::MIN as i128).min(i32::MAX as i128) as i32
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vtrn1q_f64(a: float64x2_t, b: float64x2_t) -> float64x2_t {
    float64x2_t([a.0[0], b.0[0]])
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vtrn2q_f64(a: float64x2_t, b: float64x2_t) -> float64x2_t {
    float64x2_t([a.0[1], b.0[1]])
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vtrn1q_u64(a: uint64x2_t, b: uint64x2_t) -> uint64x2_t {
    uint64x2_t([a.0[0], b.0[0]])
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vtrn2q_u64(a: uint64x2_t, b: uint64x2_t) -> uint64x2_t {
    uint64x2_t([a.0[1], b.0[1]])
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vuzp1q_s64(a: int64x2_t, b: int64x2_t) -> int64x2_t {
    int64x2_t([a.0[0], b.0[0]])
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vuzp2q_s64(a: int64x2_t, b: int64x2_t) -> int64x2_t {
    int64x2_t([a.0[1], b.0[1]])
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vuzp1q_u64(a: uint64x2_t, b: uint64x2_t) -> uint64x2_t {
    uint64x2_t([a.0[0], b.0[0]])
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vuzp2q_u64(a: uint64x2_t, b: uint64x2_t) -> uint64x2_t {
    uint64x2_t([a.0[1], b.0[1]])
}

/// TRN1: the even lanes of a and b, interleaved.
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vtrn1q_s8(a: int8x16_t, b: int8x16_t) -> int8x16_t {
    let mut r = [0; 16];
    let mut i = 0;
    while i < 16 {
        r[i] = if i % 2 == 0 { a.0[i] } else { b.0[i - 1] };
        i += 1;
    }
    int8x16_t(r)
}

/// TRN2: the odd lanes of a and b, interleaved.
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vtrn2q_s8(a: int8x16_t, b: int8x16_t) -> int8x16_t {
    let mut r = [0; 16];
    let mut i = 0;
    while i < 16 {
        r[i] = if i % 2 == 0 { a.0[i + 1] } else { b.0[i] };
        i += 1;
    }
    int8x16_t(r)
}

/// TRN1: the even lanes of a and b, interleaved.
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vtrn1q_u8(a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    let mut r = [0; 16];
    let mut i = 0;
    while i < 16 {
        r[i] = if i % 2 == 0 { a.0[i] } else { b.0[i - 1] };
        i += 1;
    }
    uint8x16_t(r)
}

/// TRN2: the odd lanes of a and b, interleaved.
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vtrn2q_u8(a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    let mut r = [0; 16];
    let mut i = 0;
    while i < 16 {
        r[i] = if i % 2 == 0 { a.0[i + 1] } else { b.0[i] };
        i += 1;
    }
    uint8x16_t(r)
}

/// TRN1: the even lanes of a and b, interleaved.
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vtrn1q_s16(a: int16x8_t, b: int16x8_t) -> int16x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = if i % 2 == 0 { a.0[i] } else { b.0[i - 1] };
        i += 1;
    }
    int16x8_t(r)
}

/// TRN2: the odd lanes of a and b, interleaved.
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vtrn2q_s16(a: int16x8_t, b: int16x8_t) -> int16x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = if i % 2 == 0 { a.0[i + 1] } else { b.0[i] };
        i += 1;
    }
    int16x8_t(r)
}

/// TRN1: the even lanes of a and b, interleaved.
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vtrn1q_u16(a: uint16x8_t, b: uint16x8_t) -> uint16x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = if i % 2 == 0 { a.0[i] } else { b.0[i - 1] };
        i += 1;
    }
    uint16x8_t(r)
}

/// TRN2: the odd lanes of a and b, interleaved.
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vtrn2q_u16(a: uint16x8_t, b: uint16x8_t) -> uint16x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = if i % 2 == 0 { a.0[i + 1] } else { b.0[i] };
        i += 1;
    }
    uint16x8_t(r)
}

/// TRN1: the even lanes of a and b, interleaved.
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vtrn1q_s32(a: int32x4_t, b: int32x4_t) -> int32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = if i % 2 == 0 { a.0[i] } else { b.0[i - 1] };
        i += 1;
    }
    int32x4_t(r)
}

/// TRN2: the odd lanes of a and b, interleaved.
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vtrn2q_s32(a: int32x4_t, b: int32x4_t) -> int32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = if i % 2 == 0 { a.0[i + 1] } else { b.0[i] };
        i += 1;
    }
    int32x4_t(r)
}

/// TRN1: the even lanes of a and b, interleaved.
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vtrn1q_u32(a: uint32x4_t, b: uint32x4_t) -> uint32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = if i % 2 == 0 { a.0[i] } else { b.0[i - 1] };
        i += 1;
    }
    uint32x4_t(r)
}

/// TRN2: the odd lanes of a and b, interleaved.
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vtrn2q_u32(a: uint32x4_t, b: uint32x4_t) -> uint32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = if i % 2 == 0 { a.0[i + 1] } else { b.0[i] };
        i += 1;
    }
    uint32x4_t(r)
}

#[rustc_legacy_const_generics(2)]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmulq_laneq_f64<const LANE: i32>(a: float64x2_t, b: float64x2_t) -> float64x2_t {
    let mut r = [0.0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = neon_result_f64(a.0[i], b.0[LANE as usize], a.0[i] * b.0[LANE as usize]);
        i += 1;
    }
    float64x2_t(r)
}

#[rustc_legacy_const_generics(2)]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmul_lane_f32<const LANE: i32>(a: float32x2_t, b: float32x2_t) -> float32x2_t {
    let mut r = [0.0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = neon_result_f32(a.0[i], b.0[LANE as usize], a.0[i] * b.0[LANE as usize]);
        i += 1;
    }
    float32x2_t(r)
}

#[rustc_legacy_const_generics(2)]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmul_laneq_f32<const LANE: i32>(a: float32x2_t, b: float32x4_t) -> float32x2_t {
    let mut r = [0.0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = neon_result_f32(a.0[i], b.0[LANE as usize], a.0[i] * b.0[LANE as usize]);
        i += 1;
    }
    float32x2_t(r)
}

#[rustc_legacy_const_generics(2)]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmulq_lane_f32<const LANE: i32>(a: float32x4_t, b: float32x2_t) -> float32x4_t {
    let mut r = [0.0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = neon_result_f32(a.0[i], b.0[LANE as usize], a.0[i] * b.0[LANE as usize]);
        i += 1;
    }
    float32x4_t(r)
}

#[rustc_legacy_const_generics(2)]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmul_lane_f64<const LANE: i32>(a: float64x1_t, b: float64x1_t) -> float64x1_t {
    let mut r = [0.0; 1];
    let mut i = 0;
    while i < 1 {
        r[i] = neon_result_f64(a.0[i], b.0[LANE as usize], a.0[i] * b.0[LANE as usize]);
        i += 1;
    }
    float64x1_t(r)
}

#[rustc_legacy_const_generics(2)]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmul_laneq_f64<const LANE: i32>(a: float64x1_t, b: float64x2_t) -> float64x1_t {
    let mut r = [0.0; 1];
    let mut i = 0;
    while i < 1 {
        r[i] = neon_result_f64(a.0[i], b.0[LANE as usize], a.0[i] * b.0[LANE as usize]);
        i += 1;
    }
    float64x1_t(r)
}

#[rustc_legacy_const_generics(2)]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmulq_lane_f64<const LANE: i32>(a: float64x2_t, b: float64x1_t) -> float64x2_t {
    let mut r = [0.0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = neon_result_f64(a.0[i], b.0[LANE as usize], a.0[i] * b.0[LANE as usize]);
        i += 1;
    }
    float64x2_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmlal_s8(a: int16x8_t, b: int8x8_t, c: int8x8_t) -> int16x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = a.0[i].wrapping_add((b.0[i] as i16) * (c.0[i] as i16));
        i += 1;
    }
    int16x8_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmlsl_s8(a: int16x8_t, b: int8x8_t, c: int8x8_t) -> int16x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = a.0[i].wrapping_sub((b.0[i] as i16) * (c.0[i] as i16));
        i += 1;
    }
    int16x8_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmlsl_s16(a: int32x4_t, b: int16x4_t, c: int16x4_t) -> int32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = a.0[i].wrapping_sub((b.0[i] as i32) * (c.0[i] as i32));
        i += 1;
    }
    int32x4_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmlal_s32(a: int64x2_t, b: int32x2_t, c: int32x2_t) -> int64x2_t {
    let mut r = [0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = a.0[i].wrapping_add((b.0[i] as i64) * (c.0[i] as i64));
        i += 1;
    }
    int64x2_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmlsl_s32(a: int64x2_t, b: int32x2_t, c: int32x2_t) -> int64x2_t {
    let mut r = [0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = a.0[i].wrapping_sub((b.0[i] as i64) * (c.0[i] as i64));
        i += 1;
    }
    int64x2_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmlal_u8(a: uint16x8_t, b: uint8x8_t, c: uint8x8_t) -> uint16x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = a.0[i].wrapping_add((b.0[i] as u16) * (c.0[i] as u16));
        i += 1;
    }
    uint16x8_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmlsl_u8(a: uint16x8_t, b: uint8x8_t, c: uint8x8_t) -> uint16x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = a.0[i].wrapping_sub((b.0[i] as u16) * (c.0[i] as u16));
        i += 1;
    }
    uint16x8_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmlsl_u16(a: uint32x4_t, b: uint16x4_t, c: uint16x4_t) -> uint32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = a.0[i].wrapping_sub((b.0[i] as u32) * (c.0[i] as u32));
        i += 1;
    }
    uint32x4_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmlsl_u32(a: uint64x2_t, b: uint32x2_t, c: uint32x2_t) -> uint64x2_t {
    let mut r = [0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = a.0[i].wrapping_sub((b.0[i] as u64) * (c.0[i] as u64));
        i += 1;
    }
    uint64x2_t(r)
}

/// SQXTUN: signed to unsigned, saturated.
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vqmovun_s64(a: int64x2_t) -> uint32x2_t {
    let mut r = [0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = a.0[i].max(0).min(u32::MAX as i64) as u32;
        i += 1;
    }
    uint32x2_t(r)
}

/// FCVTA: to nearest, ties away from zero, saturated; NaN is 0.
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcvta_s32_f32(a: float32x2_t) -> int32x2_t {
    let mut r = [0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = a.0[i].round() as i32;
        i += 1;
    }
    int32x2_t(r)
}

/// FCVTA: to nearest, ties away from zero, saturated; NaN is 0.
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcvtaq_s32_f32(a: float32x4_t) -> int32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = a.0[i].round() as i32;
        i += 1;
    }
    int32x4_t(r)
}

/// FCVTA: to nearest, ties away from zero, saturated; NaN is 0.
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcvta_u32_f32(a: float32x2_t) -> uint32x2_t {
    let mut r = [0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = a.0[i].round() as u32;
        i += 1;
    }
    uint32x2_t(r)
}

/// FCVTA: to nearest, ties away from zero, saturated; NaN is 0.
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcvta_s64_f64(a: float64x1_t) -> int64x1_t {
    let mut r = [0; 1];
    let mut i = 0;
    while i < 1 {
        r[i] = a.0[i].round() as i64;
        i += 1;
    }
    int64x1_t(r)
}

/// FCVTA: to nearest, ties away from zero, saturated; NaN is 0.
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcvtaq_s64_f64(a: float64x2_t) -> int64x2_t {
    let mut r = [0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = a.0[i].round() as i64;
        i += 1;
    }
    int64x2_t(r)
}

/// FCVTA: to nearest, ties away from zero, saturated; NaN is 0.
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcvta_u64_f64(a: float64x1_t) -> uint64x1_t {
    let mut r = [0; 1];
    let mut i = 0;
    while i < 1 {
        r[i] = a.0[i].round() as u64;
        i += 1;
    }
    uint64x1_t(r)
}

/// FCVTA: to nearest, ties away from zero, saturated; NaN is 0.
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcvtaq_u64_f64(a: float64x2_t) -> uint64x2_t {
    let mut r = [0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = a.0[i].round() as u64;
        i += 1;
    }
    uint64x2_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "rdm"))]
pub fn vqrdmlah_s32(a: int32x2_t, b: int32x2_t, c: int32x2_t) -> int32x2_t {
    let mut r = [0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = neon_qrdmlah_s32(a.0[i], b.0[i], c.0[i], false);
        i += 1;
    }
    int32x2_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "rdm"))]
pub fn vqrdmlsh_s32(a: int32x2_t, b: int32x2_t, c: int32x2_t) -> int32x2_t {
    let mut r = [0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = neon_qrdmlah_s32(a.0[i], b.0[i], c.0[i], true);
        i += 1;
    }
    int32x2_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "rdm"))]
pub fn vqrdmlshq_s32(a: int32x4_t, b: int32x4_t, c: int32x4_t) -> int32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = neon_qrdmlah_s32(a.0[i], b.0[i], c.0[i], true);
        i += 1;
    }
    int32x4_t(r)
}

/// BSL: the bits of b where a is set, of c elsewhere.
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vbsl_f64(a: uint64x1_t, b: float64x1_t, c: float64x1_t) -> float64x1_t {
    float64x1_t([f64::from_bits((a.0[0] & b.0[0].to_bits()) | (!a.0[0] & c.0[0].to_bits()))])
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcvt_f32_s32(a: int32x2_t) -> float32x2_t {
    let mut r = [0.0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = a.0[i] as f32;
        i += 1;
    }
    float32x2_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcvt_f32_u32(a: uint32x2_t) -> float32x2_t {
    let mut r = [0.0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = a.0[i] as f32;
        i += 1;
    }
    float32x2_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcvtq_f64_u64(a: uint64x2_t) -> float64x2_t {
    let mut r = [0.0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = a.0[i] as f64;
        i += 1;
    }
    float64x2_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcvt_s32_f32(a: float32x2_t) -> int32x2_t {
    let mut r = [0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = a.0[i] as i32;
        i += 1;
    }
    int32x2_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcvt_u32_f32(a: float32x2_t) -> uint32x2_t {
    let mut r = [0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = a.0[i] as u32;
        i += 1;
    }
    uint32x2_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcvtq_u64_f64(a: float64x2_t) -> uint64x2_t {
    let mut r = [0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = a.0[i] as u64;
        i += 1;
    }
    uint64x2_t(r)
}

/// a + b * c: FMLA with c as its first multiplicand, as vfmaq_f32.
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vfma_f32(a: float32x2_t, b: float32x2_t, c: float32x2_t) -> float32x2_t {
    let mut r = [0.0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = neon_fma_f32(a.0[i], c.0[i], b.0[i]);
        i += 1;
    }
    float32x2_t(r)
}

/// a - b * c: FMLS with c as its (negated) first multiplicand, as vfmsq_f32.
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vfms_f32(a: float32x2_t, b: float32x2_t, c: float32x2_t) -> float32x2_t {
    let mut r = [0.0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = neon_fma_f32(a.0[i], -c.0[i], b.0[i]);
        i += 1;
    }
    float32x2_t(r)
}

/// FRINTI: to an integral value in the current rounding mode (to nearest, ties to even).
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vrndiq_f32(a: float32x4_t) -> float32x4_t {
    let mut r = [0.0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = neon_result_f32(a.0[i], a.0[i], a.0[i].round_ties_even());
        i += 1;
    }
    float32x4_t(r)
}

/// FRINTX: as FRINTI, signalling inexact.
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vrndxq_f32(a: float32x4_t) -> float32x4_t {
    let mut r = [0.0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = neon_result_f32(a.0[i], a.0[i], a.0[i].round_ties_even());
        i += 1;
    }
    float32x4_t(r)
}

/// FRINTI: to an integral value in the current rounding mode (to nearest, ties to even).
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vrndiq_f64(a: float64x2_t) -> float64x2_t {
    let mut r = [0.0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = neon_result_f64(a.0[i], a.0[i], a.0[i].round_ties_even());
        i += 1;
    }
    float64x2_t(r)
}

/// FRINTX: as FRINTI, signalling inexact.
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vrndxq_f64(a: float64x2_t) -> float64x2_t {
    let mut r = [0.0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = neon_result_f64(a.0[i], a.0[i], a.0[i].round_ties_even());
        i += 1;
    }
    float64x2_t(r)
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vqsubq_s16(a: int16x8_t, b: int16x8_t) -> int16x8_t {
    unsafe {
        vx::vx_binary(a, b, 4, 16)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vqsubq_s8(a: int8x16_t, b: int8x16_t) -> int8x16_t {
    unsafe {
        vx::vx_binary(a, b, 4, 8)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vqsubq_u16(a: uint16x8_t, b: uint16x8_t) -> uint16x8_t {
    unsafe {
        vx::vx_binary(a, b, 5, 16)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vqsubq_u8(a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    unsafe {
        vx::vx_binary(a, b, 5, 8)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vqtbl1q_s8(a: int8x16_t, b: uint8x16_t) -> int8x16_t {
    unsafe {
        vx::vx_table(a, b)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vqtbl1q_u8(a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    unsafe {
        vx::vx_table(a, b)
    }
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vqtbl4q_u8(a: uint8x16x4_t, b: uint8x16_t) -> uint8x16_t {
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

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vrev64q_f32(a: float32x4_t) -> float32x4_t {
    let mut r = [0.0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = a.0[i ^ 1];
        i += 1;
    }
    float32x4_t(r)
}

/// FRECPE on one f32: Arm's FPRecipEstimate under the FPCR every OS sets (round to nearest, no
/// flush to zero, NaNs kept): an 8-bit estimate of 1/x from the top 8 bits of x's fraction.
fn neon_recpe_f32(x: f32) -> f32 {
    let bits = x.to_bits();
    let sign = bits & 0x8000_0000;
    let exp = ((bits >> 23) & 0xff) as i32;
    let frac = bits & 0x7f_ffff;
    if exp == 0xff {
        // a NaN comes back quieted, an infinity as zero
        return f32::from_bits(if frac != 0 { bits | 0x40_0000 } else { sign });
    }
    // zero, and anything below 2^-128 (a subnormal under 2^21 ulps), overflow to infinity
    if exp == 0 && frac < 0x20_0000 {
        return f32::from_bits(sign | 0x7f80_0000);
    }
    // the fraction as the pseudocode's 52 bits; a subnormal is normalised by one or two places
    const F52: u64 = (1 << 52) - 1;
    let mut e = exp;
    let mut f = (frac as u64) << 29;
    if e == 0 {
        if f & (1 << 51) == 0 {
            e = -1;
            f = (f << 2) & F52;
        } else {
            f = (f << 1) & F52;
        }
    }
    // RecipEstimate: 1/a for a in [0.5, 1) in 1/512 steps, rounded to nearest, in [1, 2)
    let a = (256 | (f >> 44) as u32) * 2 + 1;
    let r = ((1u32 << 19) / a + 1) / 2;
    let mut result_exp = 253 - e;
    let mut f = ((r & 0xff) as u64) << 44;
    if result_exp == 0 {
        f = (1 << 51) | (f >> 1);
    } else if result_exp == -1 {
        f = (1 << 50) | (f >> 2);
        result_exp = 0;
    }
    f32::from_bits(sign | ((result_exp as u32) << 23) | (f >> 29) as u32)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vrecpe_f32(a: float32x2_t) -> float32x2_t {
    float32x2_t([neon_recpe_f32(a.0[0]), neon_recpe_f32(a.0[1])])
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vrecpeq_f32(a: float32x4_t) -> float32x4_t {
    let mut r = [0.0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = neon_recpe_f32(a.0[i]);
        i += 1;
    }
    float32x4_t(r)
}

/// FRECPS on one f32 pair: Arm's FPRecipStepFused, 2 - a * b rounded once (the Newton step that
/// refines FRECPE's estimate); an infinity times a zero is 2. A NaN comes back as FMSUB's does.
fn neon_recps_f32(a: f32, b: f32) -> f32 {
    if (a.is_infinite() && b == 0.0) || (a == 0.0 && b.is_infinite()) {
        return 2.0;
    }
    (-a).mul_add(b, 2.0)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vrecps_f32(a: float32x2_t, b: float32x2_t) -> float32x2_t {
    float32x2_t([neon_recps_f32(a.0[0], b.0[0]), neon_recps_f32(a.0[1], b.0[1])])
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vrecpsq_f32(a: float32x4_t, b: float32x4_t) -> float32x4_t {
    let mut r = [0.0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = neon_recps_f32(a.0[i], b.0[i]);
        i += 1;
    }
    float32x4_t(r)
}

/// FRSQRTE on one f32: Arm's FPRSqrtEstimate under the FPCR every OS sets (round to nearest, no
/// flush to zero, NaNs kept): an 8-bit estimate of 1/sqrt(x) from the exponent's parity and the
/// top bits of x's fraction. A zero comes back as the infinity of its sign, a negative number as
/// the default NaN, an infinity as zero.
fn neon_rsqrte_f32(x: f32) -> f32 {
    let bits = x.to_bits();
    let exp = ((bits >> 23) & 0xff) as i32;
    let frac = bits & 0x7f_ffff;
    if exp == 0xff && frac != 0 {
        return f32::from_bits(bits | 0x40_0000);
    }
    if bits & 0x7fff_ffff == 0 {
        return f32::from_bits(bits | 0x7f80_0000);
    }
    if bits & 0x8000_0000 != 0 {
        return f32::from_bits(0x7fc0_0000);
    }
    if exp == 0xff {
        return 0.0;
    }
    // the fraction as the pseudocode's 52 bits; a subnormal is normalised, its exponent below 0
    const F52: u64 = (1 << 52) - 1;
    let mut e = exp;
    let mut f = (frac as u64) << 29;
    if e == 0 {
        while f & (1 << 51) == 0 {
            f = (f << 1) & F52;
            e -= 1;
        }
        f = (f << 1) & F52;
    }
    // x scaled into [0.25, 1) by an even power of two, in 1/512 steps: 128..512
    let scaled = if e & 1 == 0 { 256 | (f >> 44) as u32 } else { 128 | (f >> 45) as u32 };
    // RecipSqrtEstimate: `a` the scaled value at its midpoint (1/1024 below 0.5, 1/512 above),
    // b the largest number with a * b^2 < 2^28 (b < 2^14 / sqrt(a)), at least 512, and the
    // estimate (b + 1) / 2 in 256..512; the pseudocode finds b by counting up from 512
    let a = if scaled < 256 { scaled * 2 + 1 } else { ((scaled >> 1) << 1) * 2 + 2 };
    let b = (((1u32 << 28) - 1) / a).isqrt().max(512);
    let estimate = (b + 1) / 2;
    let result_exp = (380 - e) / 2;
    f32::from_bits(((result_exp as u32) << 23) | ((estimate & 0xff) << 15))
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vrsqrte_f32(a: float32x2_t) -> float32x2_t {
    float32x2_t([neon_rsqrte_f32(a.0[0]), neon_rsqrte_f32(a.0[1])])
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vrsqrteq_f32(a: float32x4_t) -> float32x4_t {
    let mut r = [0.0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = neon_rsqrte_f32(a.0[i]);
        i += 1;
    }
    float32x4_t(r)
}

/// FRSQRTS on one f32 pair: Arm's FPRSqrtStepFused, (3 - a * b) / 2 rounded once (the Newton
/// step that refines FRSQRTE's estimate); an infinity times a zero is 1.5. A NaN comes back as
/// FMSUB's does. With a halved first (exact for one above the smallest exponents) the fused step
/// rounds the halved value itself, so a product near the top of the range does not overflow
/// before the halving; below those exponents a * b is under 8 and the halving after is exact.
fn neon_rsqrts_f32(a: f32, b: f32) -> f32 {
    if (a.is_infinite() && b == 0.0) || (a == 0.0 && b.is_infinite()) {
        return 1.5;
    }
    if !a.is_nan() && !b.is_nan() && a.abs() >= f32::from_bits(0x0100_0000) {
        return (-(a * 0.5)).mul_add(b, 1.5);
    }
    (-a).mul_add(b, 3.0) * 0.5
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vrsqrts_f32(a: float32x2_t, b: float32x2_t) -> float32x2_t {
    float32x2_t([neon_rsqrts_f32(a.0[0], b.0[0]), neon_rsqrts_f32(a.0[1], b.0[1])])
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vrsqrtsq_f32(a: float32x4_t, b: float32x4_t) -> float32x4_t {
    let mut r = [0.0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = neon_rsqrts_f32(a.0[i], b.0[i]);
        i += 1;
    }
    float32x4_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vrndmq_f32(a: float32x4_t) -> float32x4_t {
    let mut r = [0.0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = neon_result_f32(a.0[i], a.0[i], a.0[i].floor());
        i += 1;
    }
    float32x4_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vrndmq_f64(a: float64x2_t) -> float64x2_t {
    let mut r = [0.0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = neon_result_f64(a.0[i], a.0[i], a.0[i].floor());
        i += 1;
    }
    float64x2_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vrndnq_f32(a: float32x4_t) -> float32x4_t {
    let mut r = [0.0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = neon_result_f32(a.0[i], a.0[i], a.0[i].round_ties_even());
        i += 1;
    }
    float32x4_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vrndnq_f64(a: float64x2_t) -> float64x2_t {
    let mut r = [0.0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = neon_result_f64(a.0[i], a.0[i], a.0[i].round_ties_even());
        i += 1;
    }
    float64x2_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vrndpq_f32(a: float32x4_t) -> float32x4_t {
    let mut r = [0.0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = neon_result_f32(a.0[i], a.0[i], a.0[i].ceil());
        i += 1;
    }
    float32x4_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vrndpq_f64(a: float64x2_t) -> float64x2_t {
    let mut r = [0.0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = neon_result_f64(a.0[i], a.0[i], a.0[i].ceil());
        i += 1;
    }
    float64x2_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vrndq_f32(a: float32x4_t) -> float32x4_t {
    let mut r = [0.0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = neon_result_f32(a.0[i], a.0[i], a.0[i].trunc());
        i += 1;
    }
    float32x4_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vrndq_f64(a: float64x2_t) -> float64x2_t {
    let mut r = [0.0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = neon_result_f64(a.0[i], a.0[i], a.0[i].trunc());
        i += 1;
    }
    float64x2_t(r)
}

#[rustc_legacy_const_generics(2)]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vsetq_lane_f32<const LANE: i32>(a: f32, b: float32x4_t) -> float32x4_t {
    let mut r = b;
    r.0[LANE as usize] = a;
    r
}

#[rustc_legacy_const_generics(2)]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vsetq_lane_u64<const LANE: i32>(a: u64, b: uint64x2_t) -> uint64x2_t {
    let mut r = b;
    r.0[LANE as usize] = a;
    r
}

#[rustc_legacy_const_generics(1)]
#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vshlq_n_s32<const N: i32>(a: int32x4_t) -> int32x4_t {
    unsafe {
        vx::vx_unary(a, 1, 32, N)
    }
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vshlq_s16(a: int16x8_t, b: int16x8_t) -> int16x8_t {
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

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vshlq_s32(a: int32x4_t, b: int32x4_t) -> int32x4_t {
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

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vshlq_s64(a: int64x2_t, b: int64x2_t) -> int64x2_t {
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

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vshlq_s8(a: int8x16_t, b: int8x16_t) -> int8x16_t {
    let mut r = [0; 16];
    let mut i = 0;
    while i < 16 {
        r[i] = {
            let shift = b.0[i] as i32;
            if shift >= 8 {
                0
            } else if shift >= 0 {
                a.0[i] << shift
            } else if shift <= -8 {
                a.0[i] >> 7
            } else {
                a.0[i] >> -shift
            }
        };
        i += 1;
    }
    int8x16_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vshlq_u16(a: uint16x8_t, b: int16x8_t) -> uint16x8_t {
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

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vshlq_u32(a: uint32x4_t, b: int32x4_t) -> uint32x4_t {
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

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vshlq_u64(a: uint64x2_t, b: int64x2_t) -> uint64x2_t {
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

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vshlq_u8(a: uint8x16_t, b: int8x16_t) -> uint8x16_t {
    let mut r = [0; 16];
    let mut i = 0;
    while i < 16 {
        r[i] = {
            let shift = b.0[i] as i32;
            if shift >= 8 {
                0
            } else if shift >= 0 {
                a.0[i] << shift
            } else if shift <= -8 {
                0
            } else {
                a.0[i] >> -shift
            }
        };
        i += 1;
    }
    uint8x16_t(r)
}

#[rustc_legacy_const_generics(1)]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vshrn_n_u16<const N: i32>(a: uint16x8_t) -> uint8x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = (a.0[i] >> N) as u8;
        i += 1;
    }
    uint8x8_t(r)
}

#[rustc_legacy_const_generics(1)]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vshrn_n_u64<const N: i32>(a: uint64x2_t) -> uint32x2_t {
    let mut r = [0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = (a.0[i] >> N) as u32;
        i += 1;
    }
    uint32x2_t(r)
}

#[rustc_legacy_const_generics(1)]
#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vshrq_n_u32<const N: i32>(a: uint32x4_t) -> uint32x4_t {
    unsafe {
        vx::vx_unary(a, 2, 32, N)
    }
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vsqrtq_f32(a: float32x4_t) -> float32x4_t {
    unsafe {
        vx::vx_unary(a, 9, 32, 0)
    }
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vsqrtq_f64(a: float64x2_t) -> float64x2_t {
    let mut r = [0.0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = neon_result_f64(a.0[i], a.0[i], a.0[i].sqrt());
        i += 1;
    }
    float64x2_t(r)
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub unsafe fn vst1q_f32(ptr: *mut f32, a: float32x4_t) {
    vx::vx_store(ptr as *mut u8, a)
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub unsafe fn vst1q_f64(ptr: *mut f64, a: float64x2_t) {
    vx::vx_store(ptr as *mut u8, a)
}

#[rustc_legacy_const_generics(2)]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub unsafe fn vst1q_lane_f32<const LANE: i32>(a: *mut f32, b: float32x4_t) {
    crate::ptr::write_unaligned(a, b.0[LANE as usize]);
}

#[rustc_legacy_const_generics(2)]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub unsafe fn vst1q_lane_f64<const LANE: i32>(a: *mut f64, b: float64x2_t) {
    crate::ptr::write_unaligned(a, b.0[LANE as usize]);
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub unsafe fn vst1q_s32(a: *mut i32, b: int32x4_t) {
    vx::vx_store(a as *mut u8, b)
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub unsafe fn vst1q_u8(a: *mut u8, b: uint8x16_t) {
    vx::vx_store(a as *mut u8, b)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub unsafe fn vst3q_f32(a: *mut f32, b: float32x4x3_t) {
    let mut i = 0;
    while i < 4 {
        crate::ptr::write_unaligned(a.add(i * 3 + 0), b.0 .0[i]);
        crate::ptr::write_unaligned(a.add(i * 3 + 1), b.1 .0[i]);
        crate::ptr::write_unaligned(a.add(i * 3 + 2), b.2 .0[i]);
        i += 1;
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vsubq_f32(a: float32x4_t, b: float32x4_t) -> float32x4_t {
    unsafe {
        vx::vx_binary(a, b, 61, 32)
    }
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vsubq_f64(a: float64x2_t, b: float64x2_t) -> float64x2_t {
    let mut r = [0.0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = neon_result_f64(a.0[i], b.0[i], a.0[i] - b.0[i]);
        i += 1;
    }
    float64x2_t(r)
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vsubq_s64(a: int64x2_t, b: int64x2_t) -> int64x2_t {
    unsafe {
        vx::vx_binary(a, b, 1, 64)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vsubq_s8(a: int8x16_t, b: int8x16_t) -> int8x16_t {
    unsafe {
        vx::vx_binary(a, b, 1, 8)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vsubq_u16(a: uint16x8_t, b: uint16x8_t) -> uint16x8_t {
    unsafe {
        vx::vx_binary(a, b, 1, 16)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vsubq_u32(a: uint32x4_t, b: uint32x4_t) -> uint32x4_t {
    unsafe {
        vx::vx_binary(a, b, 1, 32)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vsubq_u64(a: uint64x2_t, b: uint64x2_t) -> uint64x2_t {
    unsafe {
        vx::vx_binary(a, b, 1, 64)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vsubq_u8(a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    unsafe {
        vx::vx_binary(a, b, 1, 8)
    }
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vtrn1q_f32(a: float32x4_t, b: float32x4_t) -> float32x4_t {
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

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vtrn2q_f32(a: float32x4_t, b: float32x4_t) -> float32x4_t {
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

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vtrnq_s16(a: int16x8_t, b: int16x8_t) -> int16x8x2_t {
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

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vuzp1q_f32(a: float32x4_t, b: float32x4_t) -> float32x4_t {
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

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vuzp1q_f64(a: float64x2_t, b: float64x2_t) -> float64x2_t {
    float64x2_t([a.0[0], b.0[0]])
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vuzp1q_s16(a: int16x8_t, b: int16x8_t) -> int16x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = if i < 4 {
            a.0[i * 2]
        } else {
            b.0[(i - 4) * 2]
        };
        i += 1;
    }
    int16x8_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vuzp1q_s32(a: int32x4_t, b: int32x4_t) -> int32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = if i < 2 {
            a.0[i * 2]
        } else {
            b.0[(i - 2) * 2]
        };
        i += 1;
    }
    int32x4_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vuzp1q_s8(a: int8x16_t, b: int8x16_t) -> int8x16_t {
    let mut r = [0; 16];
    let mut i = 0;
    while i < 16 {
        r[i] = if i < 8 {
            a.0[i * 2]
        } else {
            b.0[(i - 8) * 2]
        };
        i += 1;
    }
    int8x16_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vuzp1q_u16(a: uint16x8_t, b: uint16x8_t) -> uint16x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = if i < 4 {
            a.0[i * 2]
        } else {
            b.0[(i - 4) * 2]
        };
        i += 1;
    }
    uint16x8_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vuzp1q_u32(a: uint32x4_t, b: uint32x4_t) -> uint32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = if i < 2 {
            a.0[i * 2]
        } else {
            b.0[(i - 2) * 2]
        };
        i += 1;
    }
    uint32x4_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vuzp1q_u8(a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    let mut r = [0; 16];
    let mut i = 0;
    while i < 16 {
        r[i] = if i < 8 {
            a.0[i * 2]
        } else {
            b.0[(i - 8) * 2]
        };
        i += 1;
    }
    uint8x16_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vuzp2q_f32(a: float32x4_t, b: float32x4_t) -> float32x4_t {
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

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vuzp2q_f64(a: float64x2_t, b: float64x2_t) -> float64x2_t {
    float64x2_t([a.0[1], b.0[1]])
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vuzp2q_s16(a: int16x8_t, b: int16x8_t) -> int16x8_t {
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
    int16x8_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vuzp2q_s32(a: int32x4_t, b: int32x4_t) -> int32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = if i < 2 {
            a.0[i * 2 + 1]
        } else {
            b.0[(i - 2) * 2 + 1]
        };
        i += 1;
    }
    int32x4_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vuzp2q_s8(a: int8x16_t, b: int8x16_t) -> int8x16_t {
    let mut r = [0; 16];
    let mut i = 0;
    while i < 16 {
        r[i] = if i < 8 {
            a.0[i * 2 + 1]
        } else {
            b.0[(i - 8) * 2 + 1]
        };
        i += 1;
    }
    int8x16_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vuzp2q_u16(a: uint16x8_t, b: uint16x8_t) -> uint16x8_t {
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
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vuzp2q_u32(a: uint32x4_t, b: uint32x4_t) -> uint32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = if i < 2 {
            a.0[i * 2 + 1]
        } else {
            b.0[(i - 2) * 2 + 1]
        };
        i += 1;
    }
    uint32x4_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vuzp2q_u8(a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    let mut r = [0; 16];
    let mut i = 0;
    while i < 16 {
        r[i] = if i < 8 {
            a.0[i * 2 + 1]
        } else {
            b.0[(i - 8) * 2 + 1]
        };
        i += 1;
    }
    uint8x16_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vuzpq_u16(a: uint16x8_t, b: uint16x8_t) -> uint16x8x2_t {
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

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vuzpq_u32(a: uint32x4_t, b: uint32x4_t) -> uint32x4x2_t {
    uint32x4x2_t(
        uint32x4_t([a.0[0], a.0[2], b.0[0], b.0[2]]),
        uint32x4_t([a.0[1], a.0[3], b.0[1], b.0[3]]),
    )
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vzip1q_f32(a: float32x4_t, b: float32x4_t) -> float32x4_t {
    unsafe {
        vx::vx_binary(a, b, 17, 32)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vzip1q_f64(a: float64x2_t, b: float64x2_t) -> float64x2_t {
    unsafe {
        vx::vx_binary(a, b, 17, 64)
    }
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vzip2q_f32(a: float32x4_t, b: float32x4_t) -> float32x4_t {
    unsafe {
        vx::vx_binary(a, b, 18, 32)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vzip2q_f64(a: float64x2_t, b: float64x2_t) -> float64x2_t {
    unsafe {
        vx::vx_binary(a, b, 18, 64)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vzip2q_u64(a: uint64x2_t, b: uint64x2_t) -> uint64x2_t {
    unsafe {
        vx::vx_binary(a, b, 18, 64)
    }
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vzip_u8(a: uint8x8_t, b: uint8x8_t) -> uint8x8x2_t {
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

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmovl_u8(a: uint8x8_t) -> uint16x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = a.0[i] as u16;
        i += 1;
    }
    uint16x8_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmovl_u16(a: uint16x4_t) -> uint32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = a.0[i] as u32;
        i += 1;
    }
    uint32x4_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmovl_u32(a: uint32x2_t) -> uint64x2_t {
    let mut r = [0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = a.0[i] as u64;
        i += 1;
    }
    uint64x2_t(r)
}

// ---------------------------------------------------------------- the rest of what relax-codegen's NEON interpreter (interp/vx.rs) calls:
// the high-half widening moves, pairwise widening adds, the u8 / u64 sums across, the u32 rounding
// halving add, the u8 absolute difference, round to nearest with ties away, and f64 <-> f32 conversions

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmovl_high_u8(a: uint8x16_t) -> uint16x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = a.0[8 + i] as u16;
        i += 1;
    }
    uint16x8_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmovl_high_s8(a: int8x16_t) -> int16x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = a.0[8 + i] as i16;
        i += 1;
    }
    int16x8_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmovl_high_u16(a: uint16x8_t) -> uint32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = a.0[4 + i] as u32;
        i += 1;
    }
    uint32x4_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmovl_high_s16(a: int16x8_t) -> int32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = a.0[4 + i] as i32;
        i += 1;
    }
    int32x4_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmovl_high_u32(a: uint32x4_t) -> uint64x2_t {
    let mut r = [0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = a.0[2 + i] as u64;
        i += 1;
    }
    uint64x2_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmovl_high_s32(a: int32x4_t) -> int64x2_t {
    let mut r = [0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = a.0[2 + i] as i64;
        i += 1;
    }
    int64x2_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vpaddlq_u16(a: uint16x8_t) -> uint32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = a.0[i * 2] as u32 + a.0[i * 2 + 1] as u32;
        i += 1;
    }
    uint32x4_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vpaddlq_u32(a: uint32x4_t) -> uint64x2_t {
    uint64x2_t([a.0[0] as u64 + a.0[1] as u64, a.0[2] as u64 + a.0[3] as u64])
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vaddvq_u8(a: uint8x16_t) -> u8 {
    let mut r = 0u8;
    let mut i = 0;
    while i < 16 {
        r = r.wrapping_add(a.0[i]);
        i += 1;
    }
    r
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vaddvq_u64(a: uint64x2_t) -> u64 {
    a.0[0].wrapping_add(a.0[1])
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vrhaddq_u32(a: uint32x4_t, b: uint32x4_t) -> uint32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = ((a.0[i] as u64 + b.0[i] as u64 + 1) >> 1) as u32;
        i += 1;
    }
    uint32x4_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vabdq_u8(a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    let mut r = [0; 16];
    let mut i = 0;
    while i < 16 {
        r[i] = a.0[i].abs_diff(b.0[i]);
        i += 1;
    }
    uint8x16_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vrndaq_f32(a: float32x4_t) -> float32x4_t {
    let mut r = [0.0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = neon_result_f32(a.0[i], a.0[i], a.0[i].round());
        i += 1;
    }
    float32x4_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vrndaq_f64(a: float64x2_t) -> float64x2_t {
    let mut r = [0.0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = neon_result_f64(a.0[i], a.0[i], a.0[i].round());
        i += 1;
    }
    float64x2_t(r)
}

/// FCVTN: each f64 lane rounded to f32 (nearest, ties to even); a NaN keeps its sign and the top
/// of its payload, quieted.
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcvt_f32_f64(a: float64x2_t) -> float32x2_t {
    float32x2_t([a.0[0] as f32, a.0[1] as f32])
}

/// FCVTL: each f32 lane widened exactly; a NaN keeps its sign and payload, quieted.
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcvt_f64_f32(a: float32x2_t) -> float64x2_t {
    float64x2_t([a.0[0] as f64, a.0[1] as f64])
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcgeq_u8(a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    let mut r = [0; 16];
    let mut i = 0;
    while i < 16 {
        r[i] = if a.0[i] >= b.0[i] { u8::MAX } else { 0 };
        i += 1;
    }
    uint8x16_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcgeq_u16(a: uint16x8_t, b: uint16x8_t) -> uint16x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = if a.0[i] >= b.0[i] { u16::MAX } else { 0 };
        i += 1;
    }
    uint16x8_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcgeq_u32(a: uint32x4_t, b: uint32x4_t) -> uint32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = if a.0[i] >= b.0[i] { u32::MAX } else { 0 };
        i += 1;
    }
    uint32x4_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcgeq_u64(a: uint64x2_t, b: uint64x2_t) -> uint64x2_t {
    let mut r = [0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = if a.0[i] >= b.0[i] { u64::MAX } else { 0 };
        i += 1;
    }
    uint64x2_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcgeq_s8(a: int8x16_t, b: int8x16_t) -> uint8x16_t {
    let mut r = [0; 16];
    let mut i = 0;
    while i < 16 {
        r[i] = if a.0[i] >= b.0[i] { u8::MAX } else { 0 };
        i += 1;
    }
    uint8x16_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcgeq_s16(a: int16x8_t, b: int16x8_t) -> uint16x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = if a.0[i] >= b.0[i] { u16::MAX } else { 0 };
        i += 1;
    }
    uint16x8_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcgeq_s32(a: int32x4_t, b: int32x4_t) -> uint32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = if a.0[i] >= b.0[i] { u32::MAX } else { 0 };
        i += 1;
    }
    uint32x4_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcgeq_s64(a: int64x2_t, b: int64x2_t) -> uint64x2_t {
    let mut r = [0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = if a.0[i] >= b.0[i] { u64::MAX } else { 0 };
        i += 1;
    }
    uint64x2_t(r)
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcgtq_u8(a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    unsafe {
        vx::vx_binary(a, b, 29, 8)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcgtq_u16(a: uint16x8_t, b: uint16x8_t) -> uint16x8_t {
    unsafe {
        vx::vx_binary(a, b, 29, 16)
    }
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcleq_u8(a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    let mut r = [0; 16];
    let mut i = 0;
    while i < 16 {
        r[i] = if a.0[i] <= b.0[i] { u8::MAX } else { 0 };
        i += 1;
    }
    uint8x16_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcleq_u16(a: uint16x8_t, b: uint16x8_t) -> uint16x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = if a.0[i] <= b.0[i] { u16::MAX } else { 0 };
        i += 1;
    }
    uint16x8_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcleq_u32(a: uint32x4_t, b: uint32x4_t) -> uint32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = if a.0[i] <= b.0[i] { u32::MAX } else { 0 };
        i += 1;
    }
    uint32x4_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcleq_u64(a: uint64x2_t, b: uint64x2_t) -> uint64x2_t {
    let mut r = [0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = if a.0[i] <= b.0[i] { u64::MAX } else { 0 };
        i += 1;
    }
    uint64x2_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcleq_s8(a: int8x16_t, b: int8x16_t) -> uint8x16_t {
    let mut r = [0; 16];
    let mut i = 0;
    while i < 16 {
        r[i] = if a.0[i] <= b.0[i] { u8::MAX } else { 0 };
        i += 1;
    }
    uint8x16_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcleq_s16(a: int16x8_t, b: int16x8_t) -> uint16x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = if a.0[i] <= b.0[i] { u16::MAX } else { 0 };
        i += 1;
    }
    uint16x8_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcleq_s32(a: int32x4_t, b: int32x4_t) -> uint32x4_t {
    let mut r = [0; 4];
    let mut i = 0;
    while i < 4 {
        r[i] = if a.0[i] <= b.0[i] { u32::MAX } else { 0 };
        i += 1;
    }
    uint32x4_t(r)
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcleq_s64(a: int64x2_t, b: int64x2_t) -> uint64x2_t {
    let mut r = [0; 2];
    let mut i = 0;
    while i < 2 {
        r[i] = if a.0[i] <= b.0[i] { u64::MAX } else { 0 };
        i += 1;
    }
    uint64x2_t(r)
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcltq_u8(a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    unsafe {
        vx::vx_binary(b, a, 29, 8)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcltq_u16(a: uint16x8_t, b: uint16x8_t) -> uint16x8_t {
    unsafe {
        vx::vx_binary(b, a, 29, 16)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcltq_u32(a: uint32x4_t, b: uint32x4_t) -> uint32x4_t {
    unsafe {
        vx::vx_binary(b, a, 29, 32)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vcltq_u64(a: uint64x2_t, b: uint64x2_t) -> uint64x2_t {
    unsafe {
        vx::vx_binary(b, a, 29, 64)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vrhaddq_u8(a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    unsafe {
        vx::vx_binary(a, b, 16, 8)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vrhaddq_u16(a: uint16x8_t, b: uint16x8_t) -> uint16x8_t {
    unsafe {
        vx::vx_binary(a, b, 16, 16)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vbicq_s8(a: int8x16_t, b: int8x16_t) -> int8x16_t {
    unsafe {
        vx::vx_binary(a, b, 9, 8)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vzip1q_s8(a: int8x16_t, b: int8x16_t) -> int8x16_t {
    unsafe {
        vx::vx_binary(a, b, 17, 8)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vzip2q_s8(a: int8x16_t, b: int8x16_t) -> int8x16_t {
    unsafe {
        vx::vx_binary(a, b, 18, 8)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
#[rustc_legacy_const_generics(1)]
pub fn vshlq_n_s8<const N: i32>(a: int8x16_t) -> int8x16_t {
    unsafe {
        vx::vx_unary(a, 1, 8, N)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
#[rustc_legacy_const_generics(1)]
pub fn vshrq_n_s8<const N: i32>(a: int8x16_t) -> int8x16_t {
    unsafe {
        vx::vx_unary(a, 3, 8, N)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub unsafe fn vst1q_s8(a: *mut i8, b: int8x16_t) {
    vx::vx_store(a as *mut u8, b)
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vbicq_s16(a: int16x8_t, b: int16x8_t) -> int16x8_t {
    unsafe {
        vx::vx_binary(a, b, 9, 16)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
#[rustc_legacy_const_generics(1)]
pub fn vshlq_n_s16<const N: i32>(a: int16x8_t) -> int16x8_t {
    unsafe {
        vx::vx_unary(a, 1, 16, N)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vbicq_s32(a: int32x4_t, b: int32x4_t) -> int32x4_t {
    unsafe {
        vx::vx_binary(a, b, 9, 32)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vzip1q_s32(a: int32x4_t, b: int32x4_t) -> int32x4_t {
    unsafe {
        vx::vx_binary(a, b, 17, 32)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vzip2q_s32(a: int32x4_t, b: int32x4_t) -> int32x4_t {
    unsafe {
        vx::vx_binary(a, b, 18, 32)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vbicq_s64(a: int64x2_t, b: int64x2_t) -> int64x2_t {
    unsafe {
        vx::vx_binary(a, b, 9, 64)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vzip1q_s64(a: int64x2_t, b: int64x2_t) -> int64x2_t {
    unsafe {
        vx::vx_binary(a, b, 17, 64)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vzip2q_s64(a: int64x2_t, b: int64x2_t) -> int64x2_t {
    unsafe {
        vx::vx_binary(a, b, 18, 64)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
#[rustc_legacy_const_generics(1)]
pub fn vshlq_n_s64<const N: i32>(a: int64x2_t) -> int64x2_t {
    unsafe {
        vx::vx_unary(a, 1, 64, N)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
#[rustc_legacy_const_generics(1)]
pub fn vshrq_n_s64<const N: i32>(a: int64x2_t) -> int64x2_t {
    unsafe {
        vx::vx_unary(a, 3, 64, N)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub unsafe fn vld1q_s64(a: *const i64) -> int64x2_t {
    vx::vx_load(a as *const u8)
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub unsafe fn vst1q_s64(a: *mut i64, b: int64x2_t) {
    vx::vx_store(a as *mut u8, b)
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vbicq_u8(a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    unsafe {
        vx::vx_binary(a, b, 9, 8)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vzip1q_u8(a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    unsafe {
        vx::vx_binary(a, b, 17, 8)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vzip2q_u8(a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    unsafe {
        vx::vx_binary(a, b, 18, 8)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
#[rustc_legacy_const_generics(1)]
pub fn vshlq_n_u8<const N: i32>(a: uint8x16_t) -> uint8x16_t {
    unsafe {
        vx::vx_unary(a, 1, 8, N)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
#[rustc_legacy_const_generics(1)]
pub fn vshrq_n_u8<const N: i32>(a: uint8x16_t) -> uint8x16_t {
    unsafe {
        vx::vx_unary(a, 2, 8, N)
    }
}

/// vsri: shift right and insert. Each lane of `b` shifted right by N (1..=8) replaces all but the
/// top N bits of `a`'s lane; N = 8 keeps `a` whole.
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
#[rustc_legacy_const_generics(2)]
pub fn vsriq_n_u8<const N: i32>(a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    let n = N as u32;
    let low = if n >= 8 { 0 } else { 0xffu8 >> n };
    let mut r = [0; 16];
    let mut i = 0;
    while i < 16 {
        r[i] = (a.0[i] & !low) | (b.0[i].checked_shr(n).unwrap_or(0) & low);
        i += 1;
    }
    uint8x16_t(r)
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vbicq_u16(a: uint16x8_t, b: uint16x8_t) -> uint16x8_t {
    unsafe {
        vx::vx_binary(a, b, 9, 16)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vzip1q_u16(a: uint16x8_t, b: uint16x8_t) -> uint16x8_t {
    unsafe {
        vx::vx_binary(a, b, 17, 16)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vzip2q_u16(a: uint16x8_t, b: uint16x8_t) -> uint16x8_t {
    unsafe {
        vx::vx_binary(a, b, 18, 16)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
#[rustc_legacy_const_generics(1)]
pub fn vshlq_n_u16<const N: i32>(a: uint16x8_t) -> uint16x8_t {
    unsafe {
        vx::vx_unary(a, 1, 16, N)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
#[rustc_legacy_const_generics(1)]
pub fn vshrq_n_u16<const N: i32>(a: uint16x8_t) -> uint16x8_t {
    unsafe {
        vx::vx_unary(a, 2, 16, N)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub unsafe fn vst1q_u16(a: *mut u16, b: uint16x8_t) {
    vx::vx_store(a as *mut u8, b)
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vzip1q_u32(a: uint32x4_t, b: uint32x4_t) -> uint32x4_t {
    unsafe {
        vx::vx_binary(a, b, 17, 32)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vzip2q_u32(a: uint32x4_t, b: uint32x4_t) -> uint32x4_t {
    unsafe {
        vx::vx_binary(a, b, 18, 32)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vbicq_u64(a: uint64x2_t, b: uint64x2_t) -> uint64x2_t {
    unsafe {
        vx::vx_binary(a, b, 9, 64)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vzip1q_u64(a: uint64x2_t, b: uint64x2_t) -> uint64x2_t {
    unsafe {
        vx::vx_binary(a, b, 17, 64)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
#[rustc_legacy_const_generics(1)]
pub fn vshlq_n_u64<const N: i32>(a: uint64x2_t) -> uint64x2_t {
    unsafe {
        vx::vx_unary(a, 1, 64, N)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
#[rustc_legacy_const_generics(1)]
pub fn vshrq_n_u64<const N: i32>(a: uint64x2_t) -> uint64x2_t {
    unsafe {
        vx::vx_unary(a, 2, 64, N)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub unsafe fn vld1q_u64(a: *const u64) -> uint64x2_t {
    vx::vx_load(a as *const u8)
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub unsafe fn vst1q_u64(a: *mut u64, b: uint64x2_t) {
    vx::vx_store(a as *mut u8, b)
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmaxq_s32(a: int32x4_t, b: int32x4_t) -> int32x4_t {
    unsafe {
        vx::vx_binary(a, b, 14, 32)
    }
}

// ---- Intrinsic families ----------------------------------------------------------------------
// One table row per intrinsic, one macro per shape. The lane-wise ones go through the generic SIMD
// intrinsics (lowered to the vector IR where the target has the operation, per lane elsewhere), the
// rest move bytes. Polynomial addition is XOR. Loads and stores accept any alignment.

// stdarch keeps these aliases crate-private: the signatures show u8, u16, u64, u128
pub(crate) type p8 = u8;
pub(crate) type p16 = u16;
pub(crate) type p64 = u64;
pub(crate) type p128 = u128;

use crate::intrinsics::simd;

/// All bits set: the operand of NOT.
#[inline(always)]
unsafe fn ones<T>() -> T {
    let mut v = crate::mem::MaybeUninit::<T>::uninit();
    crate::ptr::write_bytes(v.as_mut_ptr(), 0xff, 1);
    v.assume_init()
}

/// vld2..vld4: element k of vector j is p[k * N + j].
#[inline(always)]
unsafe fn ld_n<E: Copy, const L: usize, const N: usize>(p: *const E) -> [[E; L]; N] {
    let mut r: [[E; L]; N] = crate::mem::zeroed();
    let mut k = 0;
    while k < L {
        let mut j = 0;
        while j < N {
            r[j][k] = p.add(k * N + j).read_unaligned();
            j += 1;
        }
        k += 1;
    }
    r
}

/// vst2..vst4: the inverse of ld_n.
#[inline(always)]
unsafe fn st_n<E: Copy, const L: usize, const N: usize>(p: *mut E, v: [[E; L]; N]) {
    let mut k = 0;
    while k < L {
        let mut j = 0;
        while j < N {
            p.add(k * N + j).write_unaligned(v[j][k]);
            j += 1;
        }
        k += 1;
    }
}

/// vrev16/32/64: the lanes of each group of `group` lanes in reverse order.
#[inline(always)]
fn rev_groups<E: Copy, const L: usize>(a: [E; L], group: usize) -> [E; L] {
    let mut r = a;
    let mut i = 0;
    while i < L {
        r[i] = a[i - i % group + group - 1 - i % group];
        i += 1;
    }
    r
}

/// Carry-less product of two bytes (polynomial multiplication over GF(2)).
#[inline(always)]
fn clmul8(a: u8, b: u8) -> u16 {
    let mut r = 0u16;
    let mut i = 0;
    while i < 8 {
        if (b >> i) & 1 != 0 {
            r ^= (a as u16) << i;
        }
        i += 1;
    }
    r
}

macro_rules! neon_reinterpret {
    ($feat:literal: $($name:ident $from:ident => $to:ident);* $(;)?) => {$(
        #[inline]
        #[cfg_attr(target_arch = "aarch64", target_feature(enable = $feat))]
        pub fn $name(a: $from) -> $to {
            unsafe {
                crate::mem::transmute::<$from, $to>(a)
            }
        }
    )*};
}

/// vext / vextq: lanes N.. of `a`, then the low lanes of `b` (stdarch's `vext*`, any element type).
macro_rules! neon_ext {
    ($feat:literal: $($name:ident $v:ident $l:literal);* $(;)?) => {$(
        #[inline]
        #[cfg_attr(target_arch = "aarch64", target_feature(enable = $feat))]
        #[rustc_legacy_const_generics(2)]
        pub fn $name<const N: i32>(a: $v, b: $v) -> $v {
            let n = N as usize;
            let mut r = a;
            let mut i = 0;
            while i < $l {
                r.0[i] = if i + n < $l { a.0[i + n] } else { b.0[i + n - $l] };
                i += 1;
            }
            r
        }
    )*};
}

macro_rules! neon_ld1 {
    ($feat:literal: $($name:ident $e:ident $v:ident);* $(;)?) => {$(
        #[inline]
        #[cfg_attr(target_arch = "aarch64", target_feature(enable = $feat))]
        pub unsafe fn $name(ptr: *const $e) -> $v {
            crate::ptr::read_unaligned(ptr as *const $v)
        }
    )*};
}

macro_rules! neon_st1 {
    ($feat:literal: $($name:ident $e:ident $v:ident);* $(;)?) => {$(
        #[inline]
        #[cfg_attr(target_arch = "aarch64", target_feature(enable = $feat))]
        pub unsafe fn $name(ptr: *mut $e, a: $v) {
            crate::ptr::write_unaligned(ptr as *mut $v, a)
        }
    )*};
}

macro_rules! neon_ld1_dup {
    ($feat:literal: $($name:ident $e:ident $v:ident);* $(;)?) => {$(
        #[inline]
        #[cfg_attr(target_arch = "aarch64", target_feature(enable = $feat))]
        pub unsafe fn $name(ptr: *const $e) -> $v {
            simd::simd_splat::<$v, $e>(ptr.read_unaligned())
        }
    )*};
}

macro_rules! neon_ld1_lane {
    ($feat:literal: $($name:ident $e:ident $v:ident);* $(;)?) => {$(
        #[inline]
        #[cfg_attr(target_arch = "aarch64", target_feature(enable = $feat))]
        #[rustc_legacy_const_generics(2)]
        pub unsafe fn $name<const LANE: i32>(ptr: *const $e, src: $v) -> $v {
            let mut r = src;
            r.0[LANE as usize] = ptr.read_unaligned();
            r
        }
    )*};
}

macro_rules! neon_st1_lane {
    ($feat:literal: $($name:ident $e:ident $v:ident);* $(;)?) => {$(
        #[inline]
        #[cfg_attr(target_arch = "aarch64", target_feature(enable = $feat))]
        #[rustc_legacy_const_generics(2)]
        pub unsafe fn $name<const LANE: i32>(ptr: *mut $e, a: $v) {
            ptr.write_unaligned(a.0[LANE as usize])
        }
    )*};
}

/// vld1_x2..x4 / vst1_x2..x4: consecutive whole vectors.
macro_rules! neon_ld1_x {
    ($feat:literal: $($name:ident $e:ident $t:ident);* $(;)?) => {$(
        #[inline]
        #[cfg_attr(target_arch = "aarch64", target_feature(enable = $feat))]
        pub unsafe fn $name(ptr: *const $e) -> $t {
            crate::ptr::read_unaligned(ptr as *const $t)
        }
    )*};
}

macro_rules! neon_st1_x {
    ($feat:literal: $($name:ident $e:ident $t:ident);* $(;)?) => {$(
        #[inline]
        #[cfg_attr(target_arch = "aarch64", target_feature(enable = $feat))]
        pub unsafe fn $name(ptr: *mut $e, a: $t) {
            crate::ptr::write_unaligned(ptr as *mut $t, a)
        }
    )*};
}

macro_rules! neon_ld_n {
    ($feat:literal: $($name:ident $e:ident $t:ident $l:literal $n:literal);* $(;)?) => {$(
        #[inline]
        #[cfg_attr(target_arch = "aarch64", target_feature(enable = $feat))]
        pub unsafe fn $name(ptr: *const $e) -> $t {
            crate::mem::transmute::<[[$e; $l]; $n], $t>(ld_n::<$e, $l, $n>(ptr))
        }
    )*};
}

macro_rules! neon_st_n {
    ($feat:literal: $($name:ident $e:ident $t:ident $l:literal $n:literal);* $(;)?) => {$(
        #[inline]
        #[cfg_attr(target_arch = "aarch64", target_feature(enable = $feat))]
        pub unsafe fn $name(ptr: *mut $e, a: $t) {
            st_n::<$e, $l, $n>(ptr, crate::mem::transmute::<$t, [[$e; $l]; $n]>(a))
        }
    )*};
}

macro_rules! neon_dup_n {
    ($feat:literal: $($name:ident $e:ident $v:ident);* $(;)?) => {$(
        #[inline]
        #[cfg_attr(target_arch = "aarch64", target_feature(enable = $feat))]
        pub fn $name(value: $e) -> $v {
            unsafe {
                simd::simd_splat::<$v, $e>(value)
            }
        }
    )*};
}

macro_rules! neon_create {
    ($feat:literal: $($name:ident $v:ident);* $(;)?) => {$(
        #[inline]
        #[cfg_attr(target_arch = "aarch64", target_feature(enable = $feat))]
        pub fn $name(a: u64) -> $v {
            unsafe {
                crate::mem::transmute::<u64, $v>(a)
            }
        }
    )*};
}

macro_rules! neon_combine {
    ($feat:literal: $($name:ident $h:ident $q:ident);* $(;)?) => {$(
        #[inline]
        #[cfg_attr(target_arch = "aarch64", target_feature(enable = $feat))]
        pub fn $name(low: $h, high: $h) -> $q {
            unsafe {
                crate::mem::transmute::<[$h; 2], $q>([low, high])
            }
        }
    )*};
}

macro_rules! neon_get_half {
    ($feat:literal: $($name:ident $i:literal $h:ident $q:ident);* $(;)?) => {$(
        #[inline]
        #[cfg_attr(target_arch = "aarch64", target_feature(enable = $feat))]
        pub fn $name(a: $q) -> $h {
            unsafe {
                crate::mem::transmute::<$q, [$h; 2]>(a)[$i]
            }
        }
    )*};
}

macro_rules! neon_get_lane {
    ($feat:literal: $($name:ident $e:ident $v:ident);* $(;)?) => {$(
        #[inline]
        #[cfg_attr(target_arch = "aarch64", target_feature(enable = $feat))]
        #[rustc_legacy_const_generics(1)]
        pub fn $name<const IMM5: i32>(v: $v) -> $e {
            v.0[IMM5 as usize]
        }
    )*};
}

macro_rules! neon_set_lane {
    ($feat:literal: $($name:ident $e:ident $v:ident);* $(;)?) => {$(
        #[inline]
        #[cfg_attr(target_arch = "aarch64", target_feature(enable = $feat))]
        #[rustc_legacy_const_generics(2)]
        pub fn $name<const LANE: i32>(a: $e, b: $v) -> $v {
            let mut r = b;
            r.0[LANE as usize] = a;
            r
        }
    )*};
}

macro_rules! neon_rev {
    ($feat:literal: $($name:ident $v:ident $group:literal);* $(;)?) => {$(
        #[inline]
        #[cfg_attr(target_arch = "aarch64", target_feature(enable = $feat))]
        pub fn $name(a: $v) -> $v {
            $v(rev_groups(a.0, $group))
        }
    )*};
}

macro_rules! neon_binary {
    ($feat:literal, $op:ident: $($name:ident $v:ident);* $(;)?) => {$(
        #[inline]
        #[cfg_attr(target_arch = "aarch64", target_feature(enable = $feat))]
        pub fn $name(a: $v, b: $v) -> $v {
            unsafe {
                simd::$op(a, b)
            }
        }
    )*};
}

/// vmax / vmin: the lane the compare picks.
macro_rules! neon_max_min {
    ($feat:literal: $($name:ident $cmp:ident $v:ident $m:ident);* $(;)?) => {$(
        #[inline]
        #[cfg_attr(target_arch = "aarch64", target_feature(enable = $feat))]
        pub fn $name(a: $v, b: $v) -> $v {
            unsafe {
                simd::simd_select(simd::$cmp::<$v, $m>(a, b), a, b)
            }
        }
    )*};
}

/// vbic (a AND NOT b) / vorn (a OR NOT b).
macro_rules! neon_with_not {
    ($feat:literal: $($name:ident $op:ident $v:ident);* $(;)?) => {$(
        #[inline]
        #[cfg_attr(target_arch = "aarch64", target_feature(enable = $feat))]
        pub fn $name(a: $v, b: $v) -> $v {
            unsafe {
                simd::$op(a, simd::simd_xor(b, ones::<$v>()))
            }
        }
    )*};
}

macro_rules! neon_not {
    ($feat:literal: $($name:ident $v:ident);* $(;)?) => {$(
        #[inline]
        #[cfg_attr(target_arch = "aarch64", target_feature(enable = $feat))]
        pub fn $name(a: $v) -> $v {
            unsafe {
                simd::simd_xor(a, ones::<$v>())
            }
        }
    )*};
}

/// Wrapping negation: the most negative lane stays as it is.
macro_rules! neon_neg {
    ($feat:literal: $($name:ident $v:ident);* $(;)?) => {$(
        #[inline]
        #[cfg_attr(target_arch = "aarch64", target_feature(enable = $feat))]
        pub fn $name(a: $v) -> $v {
            unsafe {
                simd::simd_neg(a)
            }
        }
    )*};
}

/// Wrapping absolute value: the most negative lane stays as it is.
macro_rules! neon_abs {
    ($feat:literal: $($name:ident $v:ident $m:ident);* $(;)?) => {$(
        #[inline]
        #[cfg_attr(target_arch = "aarch64", target_feature(enable = $feat))]
        pub fn $name(a: $v) -> $v {
            unsafe {
                simd::simd_select(simd::simd_lt::<$v, $m>(a, crate::mem::zeroed()), simd::simd_neg(a), a)
            }
        }
    )*};
}

macro_rules! neon_fabs {
    ($feat:literal: $($name:ident $v:ident);* $(;)?) => {$(
        #[inline]
        #[cfg_attr(target_arch = "aarch64", target_feature(enable = $feat))]
        pub fn $name(a: $v) -> $v {
            unsafe {
                simd::simd_fabs(a)
            }
        }
    )*};
}

/// vmla (a + b * c) / vmls (a - b * c), not fused.
macro_rules! neon_mul_acc {
    ($feat:literal: $($name:ident $op:ident $v:ident);* $(;)?) => {$(
        #[inline]
        #[cfg_attr(target_arch = "aarch64", target_feature(enable = $feat))]
        pub fn $name(a: $v, b: $v, c: $v) -> $v {
            unsafe {
                simd::$op(a, simd::simd_mul(b, c))
            }
        }
    )*};
}

macro_rules! neon_mul_n {
    ($feat:literal: $($name:ident $e:ident $v:ident);* $(;)?) => {$(
        #[inline]
        #[cfg_attr(target_arch = "aarch64", target_feature(enable = $feat))]
        pub fn $name(a: $v, b: $e) -> $v {
            unsafe {
                simd::simd_mul(a, simd::simd_splat::<$v, $e>(b))
            }
        }
    )*};
}

macro_rules! neon_mul_acc_n {
    ($feat:literal: $($name:ident $op:ident $e:ident $v:ident);* $(;)?) => {$(
        #[inline]
        #[cfg_attr(target_arch = "aarch64", target_feature(enable = $feat))]
        pub fn $name(a: $v, b: $v, c: $e) -> $v {
            unsafe {
                simd::$op(a, simd::simd_mul(b, simd::simd_splat::<$v, $e>(c)))
            }
        }
    )*};
}

/// vqdmulh / vqrdmulh: the high half of 2 * a * b (plus half an ulp of that half when rounding),
/// saturated; computed in a type twice as wide.
macro_rules! neon_sat_doubling_mul_high {
    ($feat:literal: $($name:ident $e:ident $w:ident $v:ident $round:literal);* $(;)?) => {$(
        #[inline]
        #[cfg_attr(target_arch = "aarch64", target_feature(enable = $feat))]
        pub fn $name(a: $v, b: $v) -> $v {
            let mut r = a;
            let mut i = 0;
            while i < r.0.len() {
                let mut p = 2 * (a.0[i] as $w) * (b.0[i] as $w);
                if $round {
                    p += (1 as $w) << ($e::BITS - 1);
                }
                r.0[i] = (p >> $e::BITS).clamp($e::MIN as $w, $e::MAX as $w) as $e;
                i += 1;
            }
            r
        }
    )*};
}

macro_rules! neon_sat_doubling_mul_high_n {
    ($feat:literal: $($name:ident $base:ident $e:ident $v:ident);* $(;)?) => {$(
        #[inline]
        #[cfg_attr(target_arch = "aarch64", target_feature(enable = $feat))]
        pub fn $name(a: $v, b: $e) -> $v {
            unsafe {
                $base(a, simd::simd_splat::<$v, $e>(b))
            }
        }
    )*};
}

macro_rules! neon_shl_n {
    ($feat:literal: $($name:ident $e:ident $v:ident);* $(;)?) => {$(
        #[inline]
        #[cfg_attr(target_arch = "aarch64", target_feature(enable = $feat))]
        #[rustc_legacy_const_generics(1)]
        pub fn $name<const N: i32>(a: $v) -> $v {
            unsafe {
                simd::simd_shl(a, simd::simd_splat::<$v, $e>(N as $e))
            }
        }
    )*};
}

/// Arithmetic right shift by 1..=bits; a shift by the lane width fills the lane with its sign.
macro_rules! neon_shr_n_signed {
    ($feat:literal: $($name:ident $e:ident $v:ident);* $(;)?) => {$(
        #[inline]
        #[cfg_attr(target_arch = "aarch64", target_feature(enable = $feat))]
        #[rustc_legacy_const_generics(1)]
        pub fn $name<const N: i32>(a: $v) -> $v {
            unsafe {
                let n = if N as u32 >= $e::BITS { $e::BITS - 1 } else { N as u32 };
                simd::simd_shr(a, simd::simd_splat::<$v, $e>(n as $e))
            }
        }
    )*};
}

/// Logical right shift by 1..=bits; a shift by the lane width gives zero.
macro_rules! neon_shr_n_unsigned {
    ($feat:literal: $($name:ident $e:ident $v:ident);* $(;)?) => {$(
        #[inline]
        #[cfg_attr(target_arch = "aarch64", target_feature(enable = $feat))]
        #[rustc_legacy_const_generics(1)]
        pub fn $name<const N: i32>(a: $v) -> $v {
            unsafe {
                if N as u32 >= $e::BITS {
                    return crate::mem::zeroed();
                }
                simd::simd_shr(a, simd::simd_splat::<$v, $e>(N as $e))
            }
        }
    )*};
}

/// vshll_n: widen each lane, then shift it left.
macro_rules! neon_shll_n {
    ($feat:literal: $($name:ident $v:ident $we:ident $w:ident);* $(;)?) => {$(
        #[inline]
        #[cfg_attr(target_arch = "aarch64", target_feature(enable = $feat))]
        #[rustc_legacy_const_generics(1)]
        pub fn $name<const N: i32>(a: $v) -> $w {
            unsafe {
                simd::simd_shl(simd::simd_cast::<$v, $w>(a), simd::simd_splat::<$w, $we>(N as $we))
            }
        }
    )*};
}

/// vshrn_n: shift each lane right, then keep its low half.
macro_rules! neon_shrn_n {
    ($feat:literal: $($name:ident $e:ident $v:ident $n:ident);* $(;)?) => {$(
        #[inline]
        #[cfg_attr(target_arch = "aarch64", target_feature(enable = $feat))]
        #[rustc_legacy_const_generics(1)]
        pub fn $name<const N: i32>(a: $v) -> $n {
            unsafe {
                simd::simd_cast::<$v, $n>(simd::simd_shr(a, simd::simd_splat::<$v, $e>(N as $e)))
            }
        }
    )*};
}

macro_rules! neon_movn {
    ($feat:literal: $($name:ident $v:ident $n:ident);* $(;)?) => {$(
        #[inline]
        #[cfg_attr(target_arch = "aarch64", target_feature(enable = $feat))]
        pub fn $name(a: $v) -> $n {
            unsafe {
                simd::simd_cast::<$v, $n>(a)
            }
        }
    )*};
}

/// Compares give all ones in each lane where they hold.
macro_rules! neon_compare {
    ($feat:literal, $op:ident: $($name:ident $v:ident $m:ident);* $(;)?) => {$(
        #[inline]
        #[cfg_attr(target_arch = "aarch64", target_feature(enable = $feat))]
        pub fn $name(a: $v, b: $v) -> $m {
            unsafe {
                simd::$op::<$v, $m>(a, b)
            }
        }
    )*};
}

macro_rules! neon_compare_zero {
    ($feat:literal, $op:ident: $($name:ident $v:ident $m:ident);* $(;)?) => {$(
        #[inline]
        #[cfg_attr(target_arch = "aarch64", target_feature(enable = $feat))]
        pub fn $name(a: $v) -> $m {
            unsafe {
                simd::$op::<$v, $m>(a, crate::mem::zeroed())
            }
        }
    )*};
}

/// vtst: all ones where a AND b is not zero.
macro_rules! neon_test_bits {
    ($feat:literal: $($name:ident $v:ident $m:ident);* $(;)?) => {$(
        #[inline]
        #[cfg_attr(target_arch = "aarch64", target_feature(enable = $feat))]
        pub fn $name(a: $v, b: $v) -> $m {
            unsafe {
                simd::simd_ne::<$v, $m>(simd::simd_and(a, b), crate::mem::zeroed())
            }
        }
    )*};
}

// ---- CRC32 halves and words, polynomial multiplication ----------------------------------------

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "crc"))]
pub fn __crc32h(crc: u32, data: u16) -> u32 {
    unsafe {
        crc32::<2>(crc, data as u64)
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "crc"))]
pub fn __crc32w(crc: u32, data: u32) -> u32 {
    unsafe {
        crc32::<4>(crc, data as u64)
    }
}

/// 64 x 64 -> 128-bit carry-less product (PMULL).
#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon,aes"))]
pub fn vmull_p64(a: p64, b: p64) -> p128 {
    unsafe {
        crate::mem::transmute::<uint64x2_t, p128>(vx::vx_binary(uint64x2_t([a, 0]), uint64x2_t([b, 0]), 90, 64))
    }
}

/// The carry-less product of the high lanes (PMULL2).
#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon,aes"))]
pub fn vmull_high_p64(a: poly64x2_t, b: poly64x2_t) -> p128 {
    unsafe {
        crate::mem::transmute::<uint64x2_t, p128>(vx::vx_binary(uint64x2_t(a.0), uint64x2_t(b.0), 93, 64))
    }
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmul_p8(a: poly8x8_t, b: poly8x8_t) -> poly8x8_t {
    let mut r = a;
    let mut i = 0;
    while i < 8 {
        r.0[i] = clmul8(a.0[i], b.0[i]) as u8;
        i += 1;
    }
    r
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmulq_p8(a: poly8x16_t, b: poly8x16_t) -> poly8x16_t {
    let mut r = a;
    let mut i = 0;
    while i < 16 {
        r.0[i] = clmul8(a.0[i], b.0[i]) as u8;
        i += 1;
    }
    r
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmull_p8(a: poly8x8_t, b: poly8x8_t) -> poly16x8_t {
    let mut r = [0u16; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = clmul8(a.0[i], b.0[i]);
        i += 1;
    }
    poly16x8_t(r)
}

#[inline]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vmull_high_p8(a: poly8x16_t, b: poly8x16_t) -> poly16x8_t {
    let mut r = [0u16; 8];
    let mut i = 0;
    while i < 8 {
        r[i] = clmul8(a.0[i + 8], b.0[i + 8]);
        i += 1;
    }
    poly16x8_t(r)
}

// every vreinterpret: the same bits as another type
neon_reinterpret! { "neon":
    vreinterpret_s8_s16 int16x4_t => int8x8_t; vreinterpret_s8_s32 int32x2_t => int8x8_t;
    vreinterpret_s8_s64 int64x1_t => int8x8_t; vreinterpret_s8_u8 uint8x8_t => int8x8_t;
    vreinterpret_s8_u16 uint16x4_t => int8x8_t; vreinterpret_s8_u32 uint32x2_t => int8x8_t;
    vreinterpret_s8_u64 uint64x1_t => int8x8_t; vreinterpret_s8_f32 float32x2_t => int8x8_t;
    vreinterpret_s8_f64 float64x1_t => int8x8_t; vreinterpret_s8_p8 poly8x8_t => int8x8_t;
    vreinterpret_s8_p16 poly16x4_t => int8x8_t; vreinterpret_s8_p64 poly64x1_t => int8x8_t;
    vreinterpret_s16_s8 int8x8_t => int16x4_t; vreinterpret_s16_s32 int32x2_t => int16x4_t;
    vreinterpret_s16_s64 int64x1_t => int16x4_t; vreinterpret_s16_u8 uint8x8_t => int16x4_t;
    vreinterpret_s16_u16 uint16x4_t => int16x4_t; vreinterpret_s16_u32 uint32x2_t => int16x4_t;
    vreinterpret_s16_u64 uint64x1_t => int16x4_t; vreinterpret_s16_f32 float32x2_t => int16x4_t;
    vreinterpret_s16_f64 float64x1_t => int16x4_t; vreinterpret_s16_p8 poly8x8_t => int16x4_t;
    vreinterpret_s16_p16 poly16x4_t => int16x4_t; vreinterpret_s16_p64 poly64x1_t => int16x4_t;
    vreinterpret_s32_s8 int8x8_t => int32x2_t; vreinterpret_s32_s16 int16x4_t => int32x2_t;
    vreinterpret_s32_s64 int64x1_t => int32x2_t; vreinterpret_s32_u8 uint8x8_t => int32x2_t;
    vreinterpret_s32_u16 uint16x4_t => int32x2_t; vreinterpret_s32_u32 uint32x2_t => int32x2_t;
    vreinterpret_s32_u64 uint64x1_t => int32x2_t; vreinterpret_s32_f32 float32x2_t => int32x2_t;
    vreinterpret_s32_f64 float64x1_t => int32x2_t; vreinterpret_s32_p8 poly8x8_t => int32x2_t;
    vreinterpret_s32_p16 poly16x4_t => int32x2_t; vreinterpret_s32_p64 poly64x1_t => int32x2_t;
    vreinterpret_s64_s8 int8x8_t => int64x1_t; vreinterpret_s64_s16 int16x4_t => int64x1_t;
    vreinterpret_s64_s32 int32x2_t => int64x1_t; vreinterpret_s64_u8 uint8x8_t => int64x1_t;
    vreinterpret_s64_u16 uint16x4_t => int64x1_t; vreinterpret_s64_u32 uint32x2_t => int64x1_t;
    vreinterpret_s64_u64 uint64x1_t => int64x1_t; vreinterpret_s64_f32 float32x2_t => int64x1_t;
    vreinterpret_s64_f64 float64x1_t => int64x1_t; vreinterpret_s64_p8 poly8x8_t => int64x1_t;
    vreinterpret_s64_p16 poly16x4_t => int64x1_t; vreinterpret_s64_p64 poly64x1_t => int64x1_t;
    vreinterpret_u8_s8 int8x8_t => uint8x8_t; vreinterpret_u8_s16 int16x4_t => uint8x8_t;
    vreinterpret_u8_s32 int32x2_t => uint8x8_t; vreinterpret_u8_s64 int64x1_t => uint8x8_t;
    vreinterpret_u8_u16 uint16x4_t => uint8x8_t; vreinterpret_u8_u32 uint32x2_t => uint8x8_t;
    vreinterpret_u8_u64 uint64x1_t => uint8x8_t; vreinterpret_u8_f32 float32x2_t => uint8x8_t;
    vreinterpret_u8_f64 float64x1_t => uint8x8_t; vreinterpret_u8_p8 poly8x8_t => uint8x8_t;
    vreinterpret_u8_p16 poly16x4_t => uint8x8_t; vreinterpret_u8_p64 poly64x1_t => uint8x8_t;
    vreinterpret_u16_s8 int8x8_t => uint16x4_t; vreinterpret_u16_s16 int16x4_t => uint16x4_t;
    vreinterpret_u16_s32 int32x2_t => uint16x4_t; vreinterpret_u16_s64 int64x1_t => uint16x4_t;
    vreinterpret_u16_u8 uint8x8_t => uint16x4_t; vreinterpret_u16_u32 uint32x2_t => uint16x4_t;
    vreinterpret_u16_u64 uint64x1_t => uint16x4_t; vreinterpret_u16_f32 float32x2_t => uint16x4_t;
    vreinterpret_u16_f64 float64x1_t => uint16x4_t; vreinterpret_u16_p8 poly8x8_t => uint16x4_t;
    vreinterpret_u16_p16 poly16x4_t => uint16x4_t; vreinterpret_u16_p64 poly64x1_t => uint16x4_t;
    vreinterpret_u32_s8 int8x8_t => uint32x2_t; vreinterpret_u32_s16 int16x4_t => uint32x2_t;
    vreinterpret_u32_s32 int32x2_t => uint32x2_t; vreinterpret_u32_s64 int64x1_t => uint32x2_t;
    vreinterpret_u32_u8 uint8x8_t => uint32x2_t; vreinterpret_u32_u16 uint16x4_t => uint32x2_t;
    vreinterpret_u32_u64 uint64x1_t => uint32x2_t; vreinterpret_u32_f32 float32x2_t => uint32x2_t;
    vreinterpret_u32_f64 float64x1_t => uint32x2_t; vreinterpret_u32_p8 poly8x8_t => uint32x2_t;
    vreinterpret_u32_p16 poly16x4_t => uint32x2_t; vreinterpret_u32_p64 poly64x1_t => uint32x2_t;
    vreinterpret_u64_s8 int8x8_t => uint64x1_t; vreinterpret_u64_s16 int16x4_t => uint64x1_t;
    vreinterpret_u64_s32 int32x2_t => uint64x1_t; vreinterpret_u64_s64 int64x1_t => uint64x1_t;
    vreinterpret_u64_u8 uint8x8_t => uint64x1_t; vreinterpret_u64_u16 uint16x4_t => uint64x1_t;
    vreinterpret_u64_u32 uint32x2_t => uint64x1_t; vreinterpret_u64_f32 float32x2_t => uint64x1_t;
    vreinterpret_u64_f64 float64x1_t => uint64x1_t; vreinterpret_u64_p8 poly8x8_t => uint64x1_t;
    vreinterpret_u64_p16 poly16x4_t => uint64x1_t; vreinterpret_u64_p64 poly64x1_t => uint64x1_t;
    vreinterpret_f32_s8 int8x8_t => float32x2_t; vreinterpret_f32_s16 int16x4_t => float32x2_t;
    vreinterpret_f32_s32 int32x2_t => float32x2_t; vreinterpret_f32_s64 int64x1_t => float32x2_t;
    vreinterpret_f32_u8 uint8x8_t => float32x2_t; vreinterpret_f32_u16 uint16x4_t => float32x2_t;
    vreinterpret_f32_u32 uint32x2_t => float32x2_t; vreinterpret_f32_u64 uint64x1_t => float32x2_t;
    vreinterpret_f32_f64 float64x1_t => float32x2_t; vreinterpret_f32_p8 poly8x8_t => float32x2_t;
    vreinterpret_f32_p16 poly16x4_t => float32x2_t; vreinterpret_f32_p64 poly64x1_t => float32x2_t;
    vreinterpret_f64_s8 int8x8_t => float64x1_t; vreinterpret_f64_s16 int16x4_t => float64x1_t;
    vreinterpret_f64_s32 int32x2_t => float64x1_t; vreinterpret_f64_s64 int64x1_t => float64x1_t;
    vreinterpret_f64_u8 uint8x8_t => float64x1_t; vreinterpret_f64_u16 uint16x4_t => float64x1_t;
    vreinterpret_f64_u32 uint32x2_t => float64x1_t; vreinterpret_f64_u64 uint64x1_t => float64x1_t;
    vreinterpret_f64_f32 float32x2_t => float64x1_t; vreinterpret_f64_p8 poly8x8_t => float64x1_t;
    vreinterpret_f64_p16 poly16x4_t => float64x1_t; vreinterpret_f64_p64 poly64x1_t => float64x1_t;
    vreinterpret_p8_s8 int8x8_t => poly8x8_t; vreinterpret_p8_s16 int16x4_t => poly8x8_t;
    vreinterpret_p8_s32 int32x2_t => poly8x8_t; vreinterpret_p8_s64 int64x1_t => poly8x8_t;
    vreinterpret_p8_u8 uint8x8_t => poly8x8_t; vreinterpret_p8_u16 uint16x4_t => poly8x8_t;
    vreinterpret_p8_u32 uint32x2_t => poly8x8_t; vreinterpret_p8_u64 uint64x1_t => poly8x8_t;
    vreinterpret_p8_f32 float32x2_t => poly8x8_t; vreinterpret_p8_f64 float64x1_t => poly8x8_t;
    vreinterpret_p8_p16 poly16x4_t => poly8x8_t; vreinterpret_p8_p64 poly64x1_t => poly8x8_t;
    vreinterpret_p16_s8 int8x8_t => poly16x4_t; vreinterpret_p16_s16 int16x4_t => poly16x4_t;
    vreinterpret_p16_s32 int32x2_t => poly16x4_t; vreinterpret_p16_s64 int64x1_t => poly16x4_t;
    vreinterpret_p16_u8 uint8x8_t => poly16x4_t; vreinterpret_p16_u16 uint16x4_t => poly16x4_t;
    vreinterpret_p16_u32 uint32x2_t => poly16x4_t; vreinterpret_p16_u64 uint64x1_t => poly16x4_t;
    vreinterpret_p16_f32 float32x2_t => poly16x4_t; vreinterpret_p16_f64 float64x1_t => poly16x4_t;
    vreinterpret_p16_p8 poly8x8_t => poly16x4_t; vreinterpret_p16_p64 poly64x1_t => poly16x4_t;
    vreinterpret_p64_s8 int8x8_t => poly64x1_t; vreinterpret_p64_s16 int16x4_t => poly64x1_t;
    vreinterpret_p64_s32 int32x2_t => poly64x1_t; vreinterpret_p64_s64 int64x1_t => poly64x1_t;
    vreinterpret_p64_u8 uint8x8_t => poly64x1_t; vreinterpret_p64_u16 uint16x4_t => poly64x1_t;
    vreinterpret_p64_u32 uint32x2_t => poly64x1_t; vreinterpret_p64_u64 uint64x1_t => poly64x1_t;
    vreinterpret_p64_f32 float32x2_t => poly64x1_t; vreinterpret_p64_f64 float64x1_t => poly64x1_t;
    vreinterpret_p64_p8 poly8x8_t => poly64x1_t; vreinterpret_p64_p16 poly16x4_t => poly64x1_t;
    vreinterpretq_s8_s16 int16x8_t => int8x16_t; vreinterpretq_s8_s32 int32x4_t => int8x16_t;
    vreinterpretq_s8_s64 int64x2_t => int8x16_t; vreinterpretq_s8_u8 uint8x16_t => int8x16_t;
    vreinterpretq_s8_u16 uint16x8_t => int8x16_t; vreinterpretq_s8_u32 uint32x4_t => int8x16_t;
    vreinterpretq_s8_u64 uint64x2_t => int8x16_t; vreinterpretq_s8_f32 float32x4_t => int8x16_t;
    vreinterpretq_s8_f64 float64x2_t => int8x16_t; vreinterpretq_s8_p8 poly8x16_t => int8x16_t;
    vreinterpretq_s8_p16 poly16x8_t => int8x16_t; vreinterpretq_s8_p64 poly64x2_t => int8x16_t;
    vreinterpretq_s16_s8 int8x16_t => int16x8_t; vreinterpretq_s16_s32 int32x4_t => int16x8_t;
    vreinterpretq_s16_s64 int64x2_t => int16x8_t; vreinterpretq_s16_u8 uint8x16_t => int16x8_t;
    vreinterpretq_s16_u16 uint16x8_t => int16x8_t; vreinterpretq_s16_u32 uint32x4_t => int16x8_t;
    vreinterpretq_s16_u64 uint64x2_t => int16x8_t; vreinterpretq_s16_f32 float32x4_t => int16x8_t;
    vreinterpretq_s16_f64 float64x2_t => int16x8_t; vreinterpretq_s16_p8 poly8x16_t => int16x8_t;
    vreinterpretq_s16_p16 poly16x8_t => int16x8_t; vreinterpretq_s16_p64 poly64x2_t => int16x8_t;
    vreinterpretq_s32_s8 int8x16_t => int32x4_t; vreinterpretq_s32_s16 int16x8_t => int32x4_t;
    vreinterpretq_s32_s64 int64x2_t => int32x4_t; vreinterpretq_s32_u8 uint8x16_t => int32x4_t;
    vreinterpretq_s32_u16 uint16x8_t => int32x4_t; vreinterpretq_s32_u32 uint32x4_t => int32x4_t;
    vreinterpretq_s32_u64 uint64x2_t => int32x4_t; vreinterpretq_s32_f32 float32x4_t => int32x4_t;
    vreinterpretq_s32_f64 float64x2_t => int32x4_t; vreinterpretq_s32_p8 poly8x16_t => int32x4_t;
    vreinterpretq_s32_p16 poly16x8_t => int32x4_t; vreinterpretq_s32_p64 poly64x2_t => int32x4_t;
    vreinterpretq_s64_s8 int8x16_t => int64x2_t; vreinterpretq_s64_s16 int16x8_t => int64x2_t;
    vreinterpretq_s64_s32 int32x4_t => int64x2_t; vreinterpretq_s64_u8 uint8x16_t => int64x2_t;
    vreinterpretq_s64_u16 uint16x8_t => int64x2_t; vreinterpretq_s64_u32 uint32x4_t => int64x2_t;
    vreinterpretq_s64_u64 uint64x2_t => int64x2_t; vreinterpretq_s64_f32 float32x4_t => int64x2_t;
    vreinterpretq_s64_f64 float64x2_t => int64x2_t; vreinterpretq_s64_p8 poly8x16_t => int64x2_t;
    vreinterpretq_s64_p16 poly16x8_t => int64x2_t; vreinterpretq_s64_p64 poly64x2_t => int64x2_t;
    vreinterpretq_u8_s8 int8x16_t => uint8x16_t; vreinterpretq_u8_s16 int16x8_t => uint8x16_t;
    vreinterpretq_u8_s32 int32x4_t => uint8x16_t; vreinterpretq_u8_s64 int64x2_t => uint8x16_t;
    vreinterpretq_u8_u16 uint16x8_t => uint8x16_t; vreinterpretq_u8_u32 uint32x4_t => uint8x16_t;
    vreinterpretq_u8_u64 uint64x2_t => uint8x16_t; vreinterpretq_u8_f32 float32x4_t => uint8x16_t;
    vreinterpretq_u8_f64 float64x2_t => uint8x16_t; vreinterpretq_u8_p8 poly8x16_t => uint8x16_t;
    vreinterpretq_u8_p16 poly16x8_t => uint8x16_t; vreinterpretq_u8_p64 poly64x2_t => uint8x16_t;
    vreinterpretq_u16_s8 int8x16_t => uint16x8_t; vreinterpretq_u16_s16 int16x8_t => uint16x8_t;
    vreinterpretq_u16_s32 int32x4_t => uint16x8_t; vreinterpretq_u16_s64 int64x2_t => uint16x8_t;
    vreinterpretq_u16_u8 uint8x16_t => uint16x8_t; vreinterpretq_u16_u32 uint32x4_t => uint16x8_t;
    vreinterpretq_u16_u64 uint64x2_t => uint16x8_t; vreinterpretq_u16_f32 float32x4_t => uint16x8_t;
    vreinterpretq_u16_f64 float64x2_t => uint16x8_t; vreinterpretq_u16_p8 poly8x16_t => uint16x8_t;
    vreinterpretq_u16_p16 poly16x8_t => uint16x8_t; vreinterpretq_u16_p64 poly64x2_t => uint16x8_t;
    vreinterpretq_u32_s8 int8x16_t => uint32x4_t; vreinterpretq_u32_s16 int16x8_t => uint32x4_t;
    vreinterpretq_u32_s32 int32x4_t => uint32x4_t; vreinterpretq_u32_s64 int64x2_t => uint32x4_t;
    vreinterpretq_u32_u8 uint8x16_t => uint32x4_t; vreinterpretq_u32_u16 uint16x8_t => uint32x4_t;
    vreinterpretq_u32_u64 uint64x2_t => uint32x4_t; vreinterpretq_u32_f32 float32x4_t => uint32x4_t;
    vreinterpretq_u32_f64 float64x2_t => uint32x4_t; vreinterpretq_u32_p8 poly8x16_t => uint32x4_t;
    vreinterpretq_u32_p16 poly16x8_t => uint32x4_t; vreinterpretq_u32_p64 poly64x2_t => uint32x4_t;
    vreinterpretq_u64_s8 int8x16_t => uint64x2_t; vreinterpretq_u64_s16 int16x8_t => uint64x2_t;
    vreinterpretq_u64_s32 int32x4_t => uint64x2_t; vreinterpretq_u64_s64 int64x2_t => uint64x2_t;
    vreinterpretq_u64_u8 uint8x16_t => uint64x2_t; vreinterpretq_u64_u16 uint16x8_t => uint64x2_t;
    vreinterpretq_u64_u32 uint32x4_t => uint64x2_t; vreinterpretq_u64_f32 float32x4_t => uint64x2_t;
    vreinterpretq_u64_f64 float64x2_t => uint64x2_t; vreinterpretq_u64_p8 poly8x16_t => uint64x2_t;
    vreinterpretq_u64_p16 poly16x8_t => uint64x2_t; vreinterpretq_u64_p64 poly64x2_t => uint64x2_t;
    vreinterpretq_f32_s8 int8x16_t => float32x4_t; vreinterpretq_f32_s16 int16x8_t => float32x4_t;
    vreinterpretq_f32_s32 int32x4_t => float32x4_t; vreinterpretq_f32_s64 int64x2_t => float32x4_t;
    vreinterpretq_f32_u8 uint8x16_t => float32x4_t; vreinterpretq_f32_u16 uint16x8_t => float32x4_t;
    vreinterpretq_f32_u32 uint32x4_t => float32x4_t;
    vreinterpretq_f32_u64 uint64x2_t => float32x4_t;
    vreinterpretq_f32_f64 float64x2_t => float32x4_t;
    vreinterpretq_f32_p8 poly8x16_t => float32x4_t; vreinterpretq_f32_p16 poly16x8_t => float32x4_t;
    vreinterpretq_f32_p64 poly64x2_t => float32x4_t; vreinterpretq_f64_s8 int8x16_t => float64x2_t;
    vreinterpretq_f64_s16 int16x8_t => float64x2_t; vreinterpretq_f64_s32 int32x4_t => float64x2_t;
    vreinterpretq_f64_s64 int64x2_t => float64x2_t; vreinterpretq_f64_u8 uint8x16_t => float64x2_t;
    vreinterpretq_f64_u16 uint16x8_t => float64x2_t;
    vreinterpretq_f64_u32 uint32x4_t => float64x2_t;
    vreinterpretq_f64_u64 uint64x2_t => float64x2_t;
    vreinterpretq_f64_f32 float32x4_t => float64x2_t;
    vreinterpretq_f64_p8 poly8x16_t => float64x2_t; vreinterpretq_f64_p16 poly16x8_t => float64x2_t;
    vreinterpretq_f64_p64 poly64x2_t => float64x2_t; vreinterpretq_p8_s8 int8x16_t => poly8x16_t;
    vreinterpretq_p8_s16 int16x8_t => poly8x16_t; vreinterpretq_p8_s32 int32x4_t => poly8x16_t;
    vreinterpretq_p8_s64 int64x2_t => poly8x16_t; vreinterpretq_p8_u8 uint8x16_t => poly8x16_t;
    vreinterpretq_p8_u16 uint16x8_t => poly8x16_t; vreinterpretq_p8_u32 uint32x4_t => poly8x16_t;
    vreinterpretq_p8_u64 uint64x2_t => poly8x16_t; vreinterpretq_p8_f32 float32x4_t => poly8x16_t;
    vreinterpretq_p8_f64 float64x2_t => poly8x16_t; vreinterpretq_p8_p16 poly16x8_t => poly8x16_t;
    vreinterpretq_p8_p64 poly64x2_t => poly8x16_t; vreinterpretq_p16_s8 int8x16_t => poly16x8_t;
    vreinterpretq_p16_s16 int16x8_t => poly16x8_t; vreinterpretq_p16_s32 int32x4_t => poly16x8_t;
    vreinterpretq_p16_s64 int64x2_t => poly16x8_t; vreinterpretq_p16_u8 uint8x16_t => poly16x8_t;
    vreinterpretq_p16_u16 uint16x8_t => poly16x8_t; vreinterpretq_p16_u32 uint32x4_t => poly16x8_t;
    vreinterpretq_p16_u64 uint64x2_t => poly16x8_t; vreinterpretq_p16_f32 float32x4_t => poly16x8_t;
    vreinterpretq_p16_f64 float64x2_t => poly16x8_t; vreinterpretq_p16_p8 poly8x16_t => poly16x8_t;
    vreinterpretq_p16_p64 poly64x2_t => poly16x8_t; vreinterpretq_p64_s8 int8x16_t => poly64x2_t;
    vreinterpretq_p64_s16 int16x8_t => poly64x2_t; vreinterpretq_p64_s32 int32x4_t => poly64x2_t;
    vreinterpretq_p64_s64 int64x2_t => poly64x2_t; vreinterpretq_p64_u8 uint8x16_t => poly64x2_t;
    vreinterpretq_p64_u16 uint16x8_t => poly64x2_t; vreinterpretq_p64_u32 uint32x4_t => poly64x2_t;
    vreinterpretq_p64_u64 uint64x2_t => poly64x2_t; vreinterpretq_p64_f32 float32x4_t => poly64x2_t;
    vreinterpretq_p64_f64 float64x2_t => poly64x2_t; vreinterpretq_p64_p8 poly8x16_t => poly64x2_t;
    vreinterpretq_p64_p16 poly16x8_t => poly64x2_t; vreinterpretq_p128_s8 int8x16_t => p128;
    vreinterpretq_p128_s16 int16x8_t => p128; vreinterpretq_p128_s32 int32x4_t => p128;
    vreinterpretq_p128_s64 int64x2_t => p128; vreinterpretq_p128_u8 uint8x16_t => p128;
    vreinterpretq_p128_u16 uint16x8_t => p128; vreinterpretq_p128_u32 uint32x4_t => p128;
    vreinterpretq_p128_u64 uint64x2_t => p128; vreinterpretq_p128_f32 float32x4_t => p128;
    vreinterpretq_f32_p128 p128 => float32x4_t; vreinterpretq_p128_f64 float64x2_t => p128;
    vreinterpretq_f64_p128 p128 => float64x2_t; vreinterpretq_p128_p8 poly8x16_t => p128;
    vreinterpretq_p128_p16 poly16x8_t => p128;
}
neon_reinterpret! { "neon,aes":
    vreinterpretq_s8_p128 p128 => int8x16_t; vreinterpretq_s16_p128 p128 => int16x8_t;
    vreinterpretq_s32_p128 p128 => int32x4_t; vreinterpretq_s64_p128 p128 => int64x2_t;
    vreinterpretq_u8_p128 p128 => uint8x16_t; vreinterpretq_u16_p128 p128 => uint16x8_t;
    vreinterpretq_u32_p128 p128 => uint32x4_t; vreinterpretq_u64_p128 p128 => uint64x2_t;
    vreinterpretq_p8_p128 p128 => poly8x16_t; vreinterpretq_p16_p128 p128 => poly16x8_t;
    vreinterpretq_p128_p64 poly64x2_t => p128; vreinterpretq_p64_p128 p128 => poly64x2_t;
}
// vld1 / vst1 and their _dup, _lane and _x2.._x4 forms; vld2..4 / vst2..4 (de)interleave
neon_ext! { "neon":
    vext_s8 int8x8_t 8; vextq_s8 int8x16_t 16; vext_s16 int16x4_t 4; vextq_s16 int16x8_t 8;
    vext_s32 int32x2_t 2; vextq_s32 int32x4_t 4; vext_s64 int64x1_t 1; vextq_s64 int64x2_t 2;
    vext_u8 uint8x8_t 8; vextq_u8 uint8x16_t 16; vext_u16 uint16x4_t 4; vextq_u16 uint16x8_t 8;
    vext_u32 uint32x2_t 2; vextq_u32 uint32x4_t 4; vext_u64 uint64x1_t 1; vextq_u64 uint64x2_t 2;
    vext_p8 poly8x8_t 8; vextq_p8 poly8x16_t 16; vext_p16 poly16x4_t 4; vextq_p16 poly16x8_t 8;
    vext_f32 float32x2_t 2; vextq_f32 float32x4_t 4; vext_f64 float64x1_t 1; vextq_f64 float64x2_t 2;
    vext_p64 poly64x1_t 1; vextq_p64 poly64x2_t 2;
}
neon_ld1! { "neon":
    vld1_s16 i16 int16x4_t; vld1_s32 i32 int32x2_t; vld1_s64 i64 int64x1_t; vld1_u32 u32 uint32x2_t;
    vld1_u64 u64 uint64x1_t; vld1_f32 f32 float32x2_t; vld1_f64 f64 float64x1_t;
    vld1_p8 p8 poly8x8_t; vld1q_p8 p8 poly8x16_t; vld1_p16 p16 poly16x4_t; vld1q_p16 p16 poly16x8_t;
}
neon_ld1! { "neon,aes":
    vld1_p64 p64 poly64x1_t; vld1q_p64 p64 poly64x2_t;
}
neon_st1! { "neon":
    vst1_s8 i8 int8x8_t; vst1_s16 i16 int16x4_t; vst1_s32 i32 int32x2_t; vst1_s64 i64 int64x1_t;
    vst1_u16 u16 uint16x4_t; vst1_u32 u32 uint32x2_t; vst1_u64 u64 uint64x1_t;
    vst1_f32 f32 float32x2_t; vst1_f64 f64 float64x1_t; vst1_p8 p8 poly8x8_t;
    vst1q_p8 p8 poly8x16_t; vst1_p16 p16 poly16x4_t; vst1q_p16 p16 poly16x8_t;
}
neon_st1! { "neon,aes":
    vst1_p64 p64 poly64x1_t; vst1q_p64 p64 poly64x2_t;
}
neon_ld1_dup! { "neon":
    vld1_dup_s8 i8 int8x8_t; vld1q_dup_s8 i8 int8x16_t; vld1_dup_s16 i16 int16x4_t;
    vld1q_dup_s16 i16 int16x8_t; vld1_dup_s32 i32 int32x2_t; vld1q_dup_s32 i32 int32x4_t;
    vld1_dup_s64 i64 int64x1_t; vld1q_dup_s64 i64 int64x2_t; vld1_dup_u8 u8 uint8x8_t;
    vld1q_dup_u8 u8 uint8x16_t; vld1_dup_u16 u16 uint16x4_t; vld1q_dup_u16 u16 uint16x8_t;
    vld1_dup_u32 u32 uint32x2_t; vld1q_dup_u32 u32 uint32x4_t; vld1_dup_u64 u64 uint64x1_t;
    vld1q_dup_u64 u64 uint64x2_t; vld1_dup_f32 f32 float32x2_t; vld1_dup_f64 f64 float64x1_t;
    vld1_dup_p8 p8 poly8x8_t; vld1q_dup_p8 p8 poly8x16_t; vld1_dup_p16 p16 poly16x4_t;
    vld1q_dup_p16 p16 poly16x8_t;
}
neon_ld1_dup! { "neon,aes":
    vld1_dup_p64 p64 poly64x1_t; vld1q_dup_p64 p64 poly64x2_t;
}
neon_ld1_lane! { "neon":
    vld1_lane_s8 i8 int8x8_t; vld1q_lane_s8 i8 int8x16_t; vld1_lane_s16 i16 int16x4_t;
    vld1q_lane_s16 i16 int16x8_t; vld1_lane_s32 i32 int32x2_t; vld1q_lane_s32 i32 int32x4_t;
    vld1_lane_s64 i64 int64x1_t; vld1q_lane_s64 i64 int64x2_t; vld1_lane_u8 u8 uint8x8_t;
    vld1q_lane_u8 u8 uint8x16_t; vld1_lane_u16 u16 uint16x4_t; vld1q_lane_u16 u16 uint16x8_t;
    vld1_lane_u32 u32 uint32x2_t; vld1q_lane_u32 u32 uint32x4_t; vld1_lane_u64 u64 uint64x1_t;
    vld1q_lane_u64 u64 uint64x2_t; vld1_lane_f32 f32 float32x2_t; vld1_lane_f64 f64 float64x1_t;
    vld1_lane_p8 p8 poly8x8_t; vld1q_lane_p8 p8 poly8x16_t; vld1_lane_p16 p16 poly16x4_t;
    vld1q_lane_p16 p16 poly16x8_t;
}
neon_ld1_lane! { "neon,aes":
    vld1_lane_p64 p64 poly64x1_t; vld1q_lane_p64 p64 poly64x2_t;
}
neon_st1_lane! { "neon":
    vst1_lane_s8 i8 int8x8_t; vst1q_lane_s8 i8 int8x16_t; vst1_lane_s16 i16 int16x4_t;
    vst1q_lane_s16 i16 int16x8_t; vst1_lane_s32 i32 int32x2_t; vst1q_lane_s32 i32 int32x4_t;
    vst1_lane_s64 i64 int64x1_t; vst1q_lane_s64 i64 int64x2_t; vst1_lane_u8 u8 uint8x8_t;
    vst1q_lane_u8 u8 uint8x16_t; vst1_lane_u16 u16 uint16x4_t; vst1q_lane_u16 u16 uint16x8_t;
    vst1_lane_u32 u32 uint32x2_t; vst1q_lane_u32 u32 uint32x4_t; vst1_lane_u64 u64 uint64x1_t;
    vst1q_lane_u64 u64 uint64x2_t; vst1_lane_f32 f32 float32x2_t; vst1_lane_f64 f64 float64x1_t;
    vst1_lane_p8 p8 poly8x8_t; vst1q_lane_p8 p8 poly8x16_t; vst1_lane_p16 p16 poly16x4_t;
    vst1q_lane_p16 p16 poly16x8_t;
}
neon_st1_lane! { "neon,aes":
    vst1_lane_p64 p64 poly64x1_t; vst1q_lane_p64 p64 poly64x2_t;
}
neon_ld1_x! { "neon":
    vld1_s8_x2 i8 int8x8x2_t; vld1q_s8_x2 i8 int8x16x2_t; vld1_s16_x2 i16 int16x4x2_t;
    vld1_s32_x2 i32 int32x2x2_t; vld1_s64_x2 i64 int64x1x2_t; vld1q_s64_x2 i64 int64x2x2_t;
    vld1_u8_x2 u8 uint8x8x2_t; vld1q_u8_x2 u8 uint8x16x2_t; vld1_u16_x2 u16 uint16x4x2_t;
    vld1q_u16_x2 u16 uint16x8x2_t; vld1_u32_x2 u32 uint32x2x2_t; vld1q_u32_x2 u32 uint32x4x2_t;
    vld1_u64_x2 u64 uint64x1x2_t; vld1q_u64_x2 u64 uint64x2x2_t; vld1_f32_x2 f32 float32x2x2_t;
    vld1q_f32_x2 f32 float32x4x2_t; vld1_f64_x2 f64 float64x1x2_t; vld1q_f64_x2 f64 float64x2x2_t;
    vld1_p8_x2 p8 poly8x8x2_t; vld1q_p8_x2 p8 poly8x16x2_t; vld1_p16_x2 p16 poly16x4x2_t;
    vld1q_p16_x2 p16 poly16x8x2_t;
}
neon_ld1_x! { "neon,aes":
    vld1_p64_x2 p64 poly64x1x2_t; vld1q_p64_x2 p64 poly64x2x2_t;
}
neon_st1_x! { "neon":
    vst1_s8_x2 i8 int8x8x2_t; vst1q_s8_x2 i8 int8x16x2_t; vst1_s16_x2 i16 int16x4x2_t;
    vst1_s32_x2 i32 int32x2x2_t; vst1q_s32_x2 i32 int32x4x2_t; vst1_s64_x2 i64 int64x1x2_t;
    vst1q_s64_x2 i64 int64x2x2_t; vst1_u8_x2 u8 uint8x8x2_t; vst1q_u8_x2 u8 uint8x16x2_t;
    vst1_u16_x2 u16 uint16x4x2_t; vst1q_u16_x2 u16 uint16x8x2_t; vst1_u32_x2 u32 uint32x2x2_t;
    vst1q_u32_x2 u32 uint32x4x2_t; vst1_u64_x2 u64 uint64x1x2_t; vst1q_u64_x2 u64 uint64x2x2_t;
    vst1_f32_x2 f32 float32x2x2_t; vst1q_f32_x2 f32 float32x4x2_t; vst1_f64_x2 f64 float64x1x2_t;
    vst1q_f64_x2 f64 float64x2x2_t; vst1_p8_x2 p8 poly8x8x2_t; vst1q_p8_x2 p8 poly8x16x2_t;
    vst1_p16_x2 p16 poly16x4x2_t; vst1q_p16_x2 p16 poly16x8x2_t;
}
neon_st1_x! { "neon,aes":
    vst1_p64_x2 p64 poly64x1x2_t; vst1q_p64_x2 p64 poly64x2x2_t;
}
neon_ld1_x! { "neon":
    vld1_s8_x3 i8 int8x8x3_t; vld1q_s8_x3 i8 int8x16x3_t; vld1_s16_x3 i16 int16x4x3_t;
    vld1q_s16_x3 i16 int16x8x3_t; vld1_s32_x3 i32 int32x2x3_t; vld1q_s32_x3 i32 int32x4x3_t;
    vld1_s64_x3 i64 int64x1x3_t; vld1q_s64_x3 i64 int64x2x3_t; vld1_u8_x3 u8 uint8x8x3_t;
    vld1q_u8_x3 u8 uint8x16x3_t; vld1_u16_x3 u16 uint16x4x3_t; vld1q_u16_x3 u16 uint16x8x3_t;
    vld1_u32_x3 u32 uint32x2x3_t; vld1q_u32_x3 u32 uint32x4x3_t; vld1_u64_x3 u64 uint64x1x3_t;
    vld1q_u64_x3 u64 uint64x2x3_t; vld1_f32_x3 f32 float32x2x3_t; vld1q_f32_x3 f32 float32x4x3_t;
    vld1_f64_x3 f64 float64x1x3_t; vld1q_f64_x3 f64 float64x2x3_t; vld1_p8_x3 p8 poly8x8x3_t;
    vld1q_p8_x3 p8 poly8x16x3_t; vld1_p16_x3 p16 poly16x4x3_t; vld1q_p16_x3 p16 poly16x8x3_t;
}
neon_ld1_x! { "neon,aes":
    vld1_p64_x3 p64 poly64x1x3_t; vld1q_p64_x3 p64 poly64x2x3_t;
}
neon_st1_x! { "neon":
    vst1_s8_x3 i8 int8x8x3_t; vst1q_s8_x3 i8 int8x16x3_t; vst1_s16_x3 i16 int16x4x3_t;
    vst1q_s16_x3 i16 int16x8x3_t; vst1_s32_x3 i32 int32x2x3_t; vst1q_s32_x3 i32 int32x4x3_t;
    vst1_s64_x3 i64 int64x1x3_t; vst1q_s64_x3 i64 int64x2x3_t; vst1_u8_x3 u8 uint8x8x3_t;
    vst1q_u8_x3 u8 uint8x16x3_t; vst1_u16_x3 u16 uint16x4x3_t; vst1q_u16_x3 u16 uint16x8x3_t;
    vst1_u32_x3 u32 uint32x2x3_t; vst1q_u32_x3 u32 uint32x4x3_t; vst1_u64_x3 u64 uint64x1x3_t;
    vst1q_u64_x3 u64 uint64x2x3_t; vst1_f32_x3 f32 float32x2x3_t; vst1q_f32_x3 f32 float32x4x3_t;
    vst1_f64_x3 f64 float64x1x3_t; vst1q_f64_x3 f64 float64x2x3_t; vst1_p8_x3 p8 poly8x8x3_t;
    vst1q_p8_x3 p8 poly8x16x3_t; vst1_p16_x3 p16 poly16x4x3_t; vst1q_p16_x3 p16 poly16x8x3_t;
}
neon_st1_x! { "neon,aes":
    vst1_p64_x3 p64 poly64x1x3_t; vst1q_p64_x3 p64 poly64x2x3_t;
}
neon_ld1_x! { "neon":
    vld1_s8_x4 i8 int8x8x4_t; vld1q_s8_x4 i8 int8x16x4_t; vld1_s16_x4 i16 int16x4x4_t;
    vld1q_s16_x4 i16 int16x8x4_t; vld1_s32_x4 i32 int32x2x4_t; vld1q_s32_x4 i32 int32x4x4_t;
    vld1_s64_x4 i64 int64x1x4_t; vld1q_s64_x4 i64 int64x2x4_t; vld1_u8_x4 u8 uint8x8x4_t;
    vld1q_u8_x4 u8 uint8x16x4_t; vld1_u16_x4 u16 uint16x4x4_t; vld1q_u16_x4 u16 uint16x8x4_t;
    vld1_u32_x4 u32 uint32x2x4_t; vld1q_u32_x4 u32 uint32x4x4_t; vld1_u64_x4 u64 uint64x1x4_t;
    vld1q_u64_x4 u64 uint64x2x4_t; vld1_f32_x4 f32 float32x2x4_t; vld1q_f32_x4 f32 float32x4x4_t;
    vld1_f64_x4 f64 float64x1x4_t; vld1q_f64_x4 f64 float64x2x4_t; vld1_p8_x4 p8 poly8x8x4_t;
    vld1q_p8_x4 p8 poly8x16x4_t; vld1_p16_x4 p16 poly16x4x4_t; vld1q_p16_x4 p16 poly16x8x4_t;
}
neon_ld1_x! { "neon,aes":
    vld1_p64_x4 p64 poly64x1x4_t; vld1q_p64_x4 p64 poly64x2x4_t;
}
neon_st1_x! { "neon":
    vst1_s8_x4 i8 int8x8x4_t; vst1q_s8_x4 i8 int8x16x4_t; vst1_s16_x4 i16 int16x4x4_t;
    vst1q_s16_x4 i16 int16x8x4_t; vst1_s32_x4 i32 int32x2x4_t; vst1q_s32_x4 i32 int32x4x4_t;
    vst1_s64_x4 i64 int64x1x4_t; vst1q_s64_x4 i64 int64x2x4_t; vst1_u8_x4 u8 uint8x8x4_t;
    vst1q_u8_x4 u8 uint8x16x4_t; vst1_u16_x4 u16 uint16x4x4_t; vst1q_u16_x4 u16 uint16x8x4_t;
    vst1_u32_x4 u32 uint32x2x4_t; vst1q_u32_x4 u32 uint32x4x4_t; vst1_u64_x4 u64 uint64x1x4_t;
    vst1q_u64_x4 u64 uint64x2x4_t; vst1_f32_x4 f32 float32x2x4_t; vst1q_f32_x4 f32 float32x4x4_t;
    vst1_f64_x4 f64 float64x1x4_t; vst1q_f64_x4 f64 float64x2x4_t; vst1_p8_x4 p8 poly8x8x4_t;
    vst1q_p8_x4 p8 poly8x16x4_t; vst1_p16_x4 p16 poly16x4x4_t; vst1q_p16_x4 p16 poly16x8x4_t;
}
neon_st1_x! { "neon,aes":
    vst1_p64_x4 p64 poly64x1x4_t; vst1q_p64_x4 p64 poly64x2x4_t;
}
neon_ld_n! { "neon":
    vld2_s8 i8 int8x8x2_t 8 2; vld2q_s8 i8 int8x16x2_t 16 2; vld2_s16 i16 int16x4x2_t 4 2;
    vld2q_s16 i16 int16x8x2_t 8 2; vld2_s32 i32 int32x2x2_t 2 2; vld2q_s32 i32 int32x4x2_t 4 2;
    vld2_s64 i64 int64x1x2_t 1 2; vld2q_s64 i64 int64x2x2_t 2 2; vld2_u8 u8 uint8x8x2_t 8 2;
    vld2q_u8 u8 uint8x16x2_t 16 2; vld2_u16 u16 uint16x4x2_t 4 2; vld2q_u16 u16 uint16x8x2_t 8 2;
    vld2_u32 u32 uint32x2x2_t 2 2; vld2q_u32 u32 uint32x4x2_t 4 2; vld2_u64 u64 uint64x1x2_t 1 2;
    vld2q_u64 u64 uint64x2x2_t 2 2; vld2_f32 f32 float32x2x2_t 2 2; vld2q_f32 f32 float32x4x2_t 4 2;
    vld2_f64 f64 float64x1x2_t 1 2; vld2q_f64 f64 float64x2x2_t 2 2; vld2_p8 p8 poly8x8x2_t 8 2;
    vld2q_p8 p8 poly8x16x2_t 16 2; vld2_p16 p16 poly16x4x2_t 4 2; vld2q_p16 p16 poly16x8x2_t 8 2;
}
neon_ld_n! { "neon,aes":
    vld2_p64 p64 poly64x1x2_t 1 2; vld2q_p64 p64 poly64x2x2_t 2 2;
}
neon_st_n! { "neon":
    vst2_s8 i8 int8x8x2_t 8 2; vst2q_s8 i8 int8x16x2_t 16 2; vst2_s16 i16 int16x4x2_t 4 2;
    vst2q_s16 i16 int16x8x2_t 8 2; vst2_s32 i32 int32x2x2_t 2 2; vst2q_s32 i32 int32x4x2_t 4 2;
    vst2_s64 i64 int64x1x2_t 1 2; vst2q_s64 i64 int64x2x2_t 2 2; vst2_u8 u8 uint8x8x2_t 8 2;
    vst2q_u8 u8 uint8x16x2_t 16 2; vst2_u16 u16 uint16x4x2_t 4 2; vst2q_u16 u16 uint16x8x2_t 8 2;
    vst2_u32 u32 uint32x2x2_t 2 2; vst2q_u32 u32 uint32x4x2_t 4 2; vst2_u64 u64 uint64x1x2_t 1 2;
    vst2q_u64 u64 uint64x2x2_t 2 2; vst2_f32 f32 float32x2x2_t 2 2; vst2q_f32 f32 float32x4x2_t 4 2;
    vst2_f64 f64 float64x1x2_t 1 2; vst2q_f64 f64 float64x2x2_t 2 2; vst2_p8 p8 poly8x8x2_t 8 2;
    vst2q_p8 p8 poly8x16x2_t 16 2; vst2_p16 p16 poly16x4x2_t 4 2; vst2q_p16 p16 poly16x8x2_t 8 2;
}
neon_st_n! { "neon,aes":
    vst2_p64 p64 poly64x1x2_t 1 2; vst2q_p64 p64 poly64x2x2_t 2 2;
}
neon_ld_n! { "neon":
    vld3_s8 i8 int8x8x3_t 8 3; vld3q_s8 i8 int8x16x3_t 16 3; vld3_s16 i16 int16x4x3_t 4 3;
    vld3q_s16 i16 int16x8x3_t 8 3; vld3_s32 i32 int32x2x3_t 2 3; vld3q_s32 i32 int32x4x3_t 4 3;
    vld3_s64 i64 int64x1x3_t 1 3; vld3q_s64 i64 int64x2x3_t 2 3; vld3_u8 u8 uint8x8x3_t 8 3;
    vld3q_u8 u8 uint8x16x3_t 16 3; vld3_u16 u16 uint16x4x3_t 4 3; vld3q_u16 u16 uint16x8x3_t 8 3;
    vld3_u32 u32 uint32x2x3_t 2 3; vld3q_u32 u32 uint32x4x3_t 4 3; vld3_u64 u64 uint64x1x3_t 1 3;
    vld3q_u64 u64 uint64x2x3_t 2 3; vld3_f32 f32 float32x2x3_t 2 3; vld3q_f32 f32 float32x4x3_t 4 3;
    vld3_f64 f64 float64x1x3_t 1 3; vld3q_f64 f64 float64x2x3_t 2 3; vld3_p8 p8 poly8x8x3_t 8 3;
    vld3q_p8 p8 poly8x16x3_t 16 3; vld3_p16 p16 poly16x4x3_t 4 3; vld3q_p16 p16 poly16x8x3_t 8 3;
}
neon_ld_n! { "neon,aes":
    vld3_p64 p64 poly64x1x3_t 1 3; vld3q_p64 p64 poly64x2x3_t 2 3;
}
neon_st_n! { "neon":
    vst3_s8 i8 int8x8x3_t 8 3; vst3q_s8 i8 int8x16x3_t 16 3; vst3_s16 i16 int16x4x3_t 4 3;
    vst3q_s16 i16 int16x8x3_t 8 3; vst3_s32 i32 int32x2x3_t 2 3; vst3q_s32 i32 int32x4x3_t 4 3;
    vst3_s64 i64 int64x1x3_t 1 3; vst3q_s64 i64 int64x2x3_t 2 3; vst3_u8 u8 uint8x8x3_t 8 3;
    vst3_u16 u16 uint16x4x3_t 4 3; vst3q_u16 u16 uint16x8x3_t 8 3; vst3_u32 u32 uint32x2x3_t 2 3;
    vst3q_u32 u32 uint32x4x3_t 4 3; vst3_u64 u64 uint64x1x3_t 1 3; vst3q_u64 u64 uint64x2x3_t 2 3;
    vst3_f32 f32 float32x2x3_t 2 3; vst3_f64 f64 float64x1x3_t 1 3; vst3q_f64 f64 float64x2x3_t 2 3;
    vst3_p8 p8 poly8x8x3_t 8 3; vst3q_p8 p8 poly8x16x3_t 16 3; vst3_p16 p16 poly16x4x3_t 4 3;
    vst3q_p16 p16 poly16x8x3_t 8 3;
}
neon_st_n! { "neon,aes":
    vst3_p64 p64 poly64x1x3_t 1 3; vst3q_p64 p64 poly64x2x3_t 2 3;
}
neon_ld_n! { "neon":
    vld4_s8 i8 int8x8x4_t 8 4; vld4q_s8 i8 int8x16x4_t 16 4; vld4_s16 i16 int16x4x4_t 4 4;
    vld4q_s16 i16 int16x8x4_t 8 4; vld4_s32 i32 int32x2x4_t 2 4; vld4q_s32 i32 int32x4x4_t 4 4;
    vld4_s64 i64 int64x1x4_t 1 4; vld4q_s64 i64 int64x2x4_t 2 4; vld4_u8 u8 uint8x8x4_t 8 4;
    vld4q_u8 u8 uint8x16x4_t 16 4; vld4_u16 u16 uint16x4x4_t 4 4; vld4q_u16 u16 uint16x8x4_t 8 4;
    vld4_u32 u32 uint32x2x4_t 2 4; vld4q_u32 u32 uint32x4x4_t 4 4; vld4_u64 u64 uint64x1x4_t 1 4;
    vld4q_u64 u64 uint64x2x4_t 2 4; vld4_f32 f32 float32x2x4_t 2 4; vld4q_f32 f32 float32x4x4_t 4 4;
    vld4_f64 f64 float64x1x4_t 1 4; vld4q_f64 f64 float64x2x4_t 2 4; vld4_p8 p8 poly8x8x4_t 8 4;
    vld4q_p8 p8 poly8x16x4_t 16 4; vld4_p16 p16 poly16x4x4_t 4 4; vld4q_p16 p16 poly16x8x4_t 8 4;
}
neon_ld_n! { "neon,aes":
    vld4_p64 p64 poly64x1x4_t 1 4; vld4q_p64 p64 poly64x2x4_t 2 4;
}
neon_st_n! { "neon":
    vst4_s8 i8 int8x8x4_t 8 4; vst4q_s8 i8 int8x16x4_t 16 4; vst4_s16 i16 int16x4x4_t 4 4;
    vst4q_s16 i16 int16x8x4_t 8 4; vst4_s32 i32 int32x2x4_t 2 4; vst4q_s32 i32 int32x4x4_t 4 4;
    vst4_s64 i64 int64x1x4_t 1 4; vst4q_s64 i64 int64x2x4_t 2 4; vst4_u8 u8 uint8x8x4_t 8 4;
    vst4_u16 u16 uint16x4x4_t 4 4; vst4q_u16 u16 uint16x8x4_t 8 4; vst4_u32 u32 uint32x2x4_t 2 4;
    vst4q_u32 u32 uint32x4x4_t 4 4; vst4_u64 u64 uint64x1x4_t 1 4; vst4q_u64 u64 uint64x2x4_t 2 4;
    vst4_f32 f32 float32x2x4_t 2 4; vst4q_f32 f32 float32x4x4_t 4 4; vst4_f64 f64 float64x1x4_t 1 4;
    vst4q_f64 f64 float64x2x4_t 2 4; vst4_p8 p8 poly8x8x4_t 8 4; vst4q_p8 p8 poly8x16x4_t 16 4;
    vst4_p16 p16 poly16x4x4_t 4 4; vst4q_p16 p16 poly16x8x4_t 8 4;
}
neon_st_n! { "neon,aes":
    vst4_p64 p64 poly64x1x4_t 1 4; vst4q_p64 p64 poly64x2x4_t 2 4;
}
// vector from lanes and lanes from vectors
neon_dup_n! { "neon":
    vdup_n_s8 i8 int8x8_t; vdup_n_s16 i16 int16x4_t; vdup_n_s32 i32 int32x2_t;
    vdup_n_s64 i64 int64x1_t; vdup_n_u16 u16 uint16x4_t; vdup_n_u32 u32 uint32x2_t;
    vdup_n_u64 u64 uint64x1_t; vdup_n_f32 f32 float32x2_t; vdup_n_f64 f64 float64x1_t;
    vdup_n_p8 p8 poly8x8_t; vdupq_n_p8 p8 poly8x16_t; vdup_n_p16 p16 poly16x4_t;
    vdupq_n_p16 p16 poly16x8_t; vdup_n_p64 p64 poly64x1_t; vdupq_n_p64 p64 poly64x2_t;
}
neon_dup_n! { "neon":
    vmov_n_s8 i8 int8x8_t; vmovq_n_s8 i8 int8x16_t; vmov_n_s16 i16 int16x4_t;
    vmov_n_s32 i32 int32x2_t; vmov_n_s64 i64 int64x1_t; vmov_n_u8 u8 uint8x8_t;
    vmovq_n_u8 u8 uint8x16_t; vmov_n_u16 u16 uint16x4_t; vmovq_n_u16 u16 uint16x8_t;
    vmov_n_u32 u32 uint32x2_t; vmov_n_u64 u64 uint64x1_t; vmovq_n_u64 u64 uint64x2_t;
    vmov_n_f32 f32 float32x2_t; vmov_n_f64 f64 float64x1_t; vmov_n_p8 p8 poly8x8_t;
    vmovq_n_p8 p8 poly8x16_t; vmov_n_p16 p16 poly16x4_t; vmovq_n_p16 p16 poly16x8_t;
}
neon_create! { "neon":
    vcreate_s8 int8x8_t; vcreate_s32 int32x2_t; vcreate_s64 int64x1_t; vcreate_u8 uint8x8_t;
    vcreate_u16 uint16x4_t; vcreate_u32 uint32x2_t; vcreate_u64 uint64x1_t; vcreate_f32 float32x2_t;
    vcreate_f64 float64x1_t; vcreate_p8 poly8x8_t; vcreate_p16 poly16x4_t;
}
neon_create! { "neon,aes":
    vcreate_p64 poly64x1_t;
}
neon_combine! { "neon":
    vcombine_s32 int32x2_t int32x4_t; vcombine_s64 int64x1_t int64x2_t;
    vcombine_u64 uint64x1_t uint64x2_t; vcombine_f32 float32x2_t float32x4_t;
    vcombine_f64 float64x1_t float64x2_t; vcombine_p8 poly8x8_t poly8x16_t;
    vcombine_p16 poly16x4_t poly16x8_t; vcombine_p64 poly64x1_t poly64x2_t;
}
neon_get_half! { "neon":
    vget_low_s64 0 int64x1_t int64x2_t; vget_low_u64 0 uint64x1_t uint64x2_t;
    vget_low_f32 0 float32x2_t float32x4_t; vget_low_f64 0 float64x1_t float64x2_t;
    vget_low_p8 0 poly8x8_t poly8x16_t; vget_low_p16 0 poly16x4_t poly16x8_t;
    vget_low_p64 0 poly64x1_t poly64x2_t;
}
neon_get_half! { "neon":
    vget_high_s64 1 int64x1_t int64x2_t; vget_high_u64 1 uint64x1_t uint64x2_t;
    vget_high_f32 1 float32x2_t float32x4_t; vget_high_f64 1 float64x1_t float64x2_t;
    vget_high_p8 1 poly8x8_t poly8x16_t; vget_high_p16 1 poly16x4_t poly16x8_t;
    vget_high_p64 1 poly64x1_t poly64x2_t;
}
neon_get_lane! { "neon":
    vget_lane_s8 i8 int8x8_t; vgetq_lane_s8 i8 int8x16_t; vget_lane_s16 i16 int16x4_t;
    vgetq_lane_s16 i16 int16x8_t; vget_lane_s32 i32 int32x2_t; vgetq_lane_s32 i32 int32x4_t;
    vget_lane_s64 i64 int64x1_t; vget_lane_u8 u8 uint8x8_t; vgetq_lane_u8 u8 uint8x16_t;
    vget_lane_u16 u16 uint16x4_t; vgetq_lane_u16 u16 uint16x8_t; vget_lane_f32 f32 float32x2_t;
    vget_lane_f64 f64 float64x1_t; vget_lane_p8 p8 poly8x8_t; vgetq_lane_p8 p8 poly8x16_t;
    vget_lane_p16 p16 poly16x4_t; vgetq_lane_p16 p16 poly16x8_t; vget_lane_p64 p64 poly64x1_t;
    vgetq_lane_p64 p64 poly64x2_t;
}
neon_set_lane! { "neon":
    vset_lane_s8 i8 int8x8_t; vsetq_lane_s8 i8 int8x16_t; vset_lane_s16 i16 int16x4_t;
    vsetq_lane_s16 i16 int16x8_t; vset_lane_s32 i32 int32x2_t; vsetq_lane_s32 i32 int32x4_t;
    vset_lane_s64 i64 int64x1_t; vsetq_lane_s64 i64 int64x2_t; vset_lane_u8 u8 uint8x8_t;
    vsetq_lane_u8 u8 uint8x16_t; vset_lane_u16 u16 uint16x4_t; vsetq_lane_u16 u16 uint16x8_t;
    vset_lane_u32 u32 uint32x2_t; vset_lane_u64 u64 uint64x1_t; vset_lane_f32 f32 float32x2_t;
    vset_lane_f64 f64 float64x1_t; vsetq_lane_f64 f64 float64x2_t; vset_lane_p8 p8 poly8x8_t;
    vsetq_lane_p8 p8 poly8x16_t; vset_lane_p16 p16 poly16x4_t; vsetq_lane_p16 p16 poly16x8_t;
}
neon_set_lane! { "neon,aes":
    vset_lane_p64 p64 poly64x1_t; vsetq_lane_p64 p64 poly64x2_t;
}
neon_rev! { "neon":
    vrev16_s8 int8x8_t 2; vrev16q_s8 int8x16_t 2; vrev16_u8 uint8x8_t 2; vrev16q_u8 uint8x16_t 2;
    vrev16_p8 poly8x8_t 2; vrev16q_p8 poly8x16_t 2; vrev32_s8 int8x8_t 4; vrev32q_s8 int8x16_t 4;
    vrev32_s16 int16x4_t 2; vrev32q_s16 int16x8_t 2; vrev32_u8 uint8x8_t 4; vrev32_u16 uint16x4_t 2;
    vrev32q_u16 uint16x8_t 2; vrev32_p8 poly8x8_t 4; vrev32q_p8 poly8x16_t 4;
    vrev32_p16 poly16x4_t 2; vrev32q_p16 poly16x8_t 2; vrev64_s8 int8x8_t 8; vrev64q_s8 int8x16_t 8;
    vrev64_s16 int16x4_t 4; vrev64q_s16 int16x8_t 4; vrev64_s32 int32x2_t 2;
    vrev64q_s32 int32x4_t 2; vrev64_u8 uint8x8_t 8; vrev64q_u8 uint8x16_t 8;
    vrev64_u16 uint16x4_t 4; vrev64q_u16 uint16x8_t 4; vrev64_u32 uint32x2_t 2;
    vrev64q_u32 uint32x4_t 2; vrev64_f32 float32x2_t 2; vrev64_p8 poly8x8_t 8;
    vrev64q_p8 poly8x16_t 8; vrev64_p16 poly16x4_t 4; vrev64q_p16 poly16x8_t 4;
}
// lane-wise arithmetic and bitwise operations
neon_binary! { "neon", simd_add:
    vadd_s8 int8x8_t; vadd_s16 int16x4_t; vadd_s32 int32x2_t; vadd_s64 int64x1_t; vadd_u8 uint8x8_t;
    vadd_u16 uint16x4_t; vadd_u32 uint32x2_t; vadd_u64 uint64x1_t; vadd_f32 float32x2_t;
    vadd_f64 float64x1_t;
}
neon_binary! { "neon", simd_xor:
    vadd_p8 poly8x8_t; vaddq_p8 poly8x16_t; vadd_p16 poly16x4_t; vaddq_p16 poly16x8_t;
    vadd_p64 poly64x1_t; vaddq_p64 poly64x2_t;
}
neon_binary! { "neon", simd_sub:
    vsub_s8 int8x8_t; vsub_s16 int16x4_t; vsub_s32 int32x2_t; vsub_s64 int64x1_t; vsub_u8 uint8x8_t;
    vsub_u16 uint16x4_t; vsub_u32 uint32x2_t; vsub_u64 uint64x1_t; vsub_f32 float32x2_t;
    vsub_f64 float64x1_t;
}
neon_binary! { "neon", simd_mul:
    vmul_s8 int8x8_t; vmulq_s8 int8x16_t; vmul_s16 int16x4_t; vmul_s32 int32x2_t; vmul_u8 uint8x8_t;
    vmulq_u8 uint8x16_t; vmul_u16 uint16x4_t; vmul_u32 uint32x2_t; vmul_f32 float32x2_t;
    vmul_f64 float64x1_t;
}
neon_binary! { "neon", simd_and:
    vand_s8 int8x8_t; vand_s16 int16x4_t; vand_s32 int32x2_t; vand_s64 int64x1_t; vand_u8 uint8x8_t;
    vand_u16 uint16x4_t; vand_u32 uint32x2_t; vand_u64 uint64x1_t;
}
neon_binary! { "neon", simd_or:
    vorr_s8 int8x8_t; vorr_s16 int16x4_t; vorr_s32 int32x2_t; vorr_s64 int64x1_t;
    vorr_u16 uint16x4_t; vorr_u32 uint32x2_t; vorr_u64 uint64x1_t;
}
neon_binary! { "neon", simd_xor:
    veor_s8 int8x8_t; veor_s16 int16x4_t; veor_s32 int32x2_t; veor_s64 int64x1_t; veor_u8 uint8x8_t;
    veor_u16 uint16x4_t; veor_u32 uint32x2_t; veor_u64 uint64x1_t;
}
neon_binary! { "neon", simd_saturating_add:
    vqadd_s8 int8x8_t; vqadd_s16 int16x4_t; vqadd_s32 int32x2_t; vqaddq_s32 int32x4_t;
    vqadd_s64 int64x1_t; vqaddq_s64 int64x2_t; vqadd_u8 uint8x8_t; vqadd_u16 uint16x4_t;
    vqadd_u32 uint32x2_t; vqaddq_u32 uint32x4_t; vqadd_u64 uint64x1_t; vqaddq_u64 uint64x2_t;
}
neon_binary! { "neon", simd_saturating_sub:
    vqsub_s8 int8x8_t; vqsub_s16 int16x4_t; vqsub_s32 int32x2_t; vqsubq_s32 int32x4_t;
    vqsub_s64 int64x1_t; vqsubq_s64 int64x2_t; vqsub_u8 uint8x8_t; vqsub_u16 uint16x4_t;
    vqsub_u32 uint32x2_t; vqsubq_u32 uint32x4_t; vqsub_u64 uint64x1_t; vqsubq_u64 uint64x2_t;
}
neon_max_min! { "neon":
    vmax_s8 simd_gt int8x8_t uint8x8_t; vmax_s16 simd_gt int16x4_t uint16x4_t;
    vmax_s32 simd_gt int32x2_t uint32x2_t; vmax_u8 simd_gt uint8x8_t uint8x8_t;
    vmax_u16 simd_gt uint16x4_t uint16x4_t; vmax_u32 simd_gt uint32x2_t uint32x2_t;
}
neon_max_min! { "neon":
    vmin_s8 simd_lt int8x8_t uint8x8_t; vmin_s16 simd_lt int16x4_t uint16x4_t;
    vmin_s32 simd_lt int32x2_t uint32x2_t; vmin_u8 simd_lt uint8x8_t uint8x8_t;
    vmin_u16 simd_lt uint16x4_t uint16x4_t; vmin_u32 simd_lt uint32x2_t uint32x2_t;
}
neon_with_not! { "neon":
    vbic_s8 simd_and int8x8_t; vbic_s16 simd_and int16x4_t; vbic_s32 simd_and int32x2_t;
    vbic_s64 simd_and int64x1_t; vbic_u8 simd_and uint8x8_t; vbic_u16 simd_and uint16x4_t;
    vbic_u32 simd_and uint32x2_t; vbic_u64 simd_and uint64x1_t;
}
neon_with_not! { "neon":
    vorn_s8 simd_or int8x8_t; vornq_s8 simd_or int8x16_t; vorn_s16 simd_or int16x4_t;
    vornq_s16 simd_or int16x8_t; vorn_s32 simd_or int32x2_t; vornq_s32 simd_or int32x4_t;
    vorn_s64 simd_or int64x1_t; vornq_s64 simd_or int64x2_t; vorn_u8 simd_or uint8x8_t;
    vornq_u8 simd_or uint8x16_t; vorn_u16 simd_or uint16x4_t; vornq_u16 simd_or uint16x8_t;
    vorn_u32 simd_or uint32x2_t; vornq_u32 simd_or uint32x4_t; vorn_u64 simd_or uint64x1_t;
    vornq_u64 simd_or uint64x2_t;
}
neon_not! { "neon":
    vmvn_s8 int8x8_t; vmvnq_s8 int8x16_t; vmvn_s16 int16x4_t; vmvnq_s16 int16x8_t;
    vmvn_s32 int32x2_t; vmvnq_s32 int32x4_t; vmvn_u8 uint8x8_t; vmvnq_u8 uint8x16_t;
    vmvn_u16 uint16x4_t; vmvnq_u16 uint16x8_t; vmvn_u32 uint32x2_t; vmvn_p8 poly8x8_t;
    vmvnq_p8 poly8x16_t;
}
neon_neg! { "neon":
    vneg_s8 int8x8_t; vnegq_s8 int8x16_t; vneg_s16 int16x4_t; vnegq_s16 int16x8_t;
    vneg_s32 int32x2_t; vneg_s64 int64x1_t; vnegq_s64 int64x2_t; vneg_f32 float32x2_t;
    vneg_f64 float64x1_t; vnegq_f64 float64x2_t;
}
neon_abs! { "neon":
    vabs_s8 int8x8_t uint8x8_t; vabs_s16 int16x4_t uint16x4_t; vabs_s32 int32x2_t uint32x2_t;
    vabs_s64 int64x1_t uint64x1_t;
}
neon_fabs! { "neon":
    vabs_f32 float32x2_t; vabs_f64 float64x1_t;
}
neon_mul_acc! { "neon":
    vmla_s8 simd_add int8x8_t; vmlaq_s8 simd_add int8x16_t; vmla_s16 simd_add int16x4_t;
    vmlaq_s16 simd_add int16x8_t; vmla_s32 simd_add int32x2_t; vmla_u8 simd_add uint8x8_t;
    vmlaq_u8 simd_add uint8x16_t; vmla_u16 simd_add uint16x4_t; vmlaq_u16 simd_add uint16x8_t;
    vmla_u32 simd_add uint32x2_t; vmlaq_u32 simd_add uint32x4_t; vmla_f32 simd_add float32x2_t;
    vmlaq_f32 simd_add float32x4_t; vmla_f64 simd_add float64x1_t; vmlaq_f64 simd_add float64x2_t;
}
neon_mul_acc! { "neon":
    vmls_s8 simd_sub int8x8_t; vmlsq_s8 simd_sub int8x16_t; vmls_s16 simd_sub int16x4_t;
    vmlsq_s16 simd_sub int16x8_t; vmls_s32 simd_sub int32x2_t; vmlsq_s32 simd_sub int32x4_t;
    vmls_u8 simd_sub uint8x8_t; vmlsq_u8 simd_sub uint8x16_t; vmls_u16 simd_sub uint16x4_t;
    vmlsq_u16 simd_sub uint16x8_t; vmls_u32 simd_sub uint32x2_t; vmlsq_u32 simd_sub uint32x4_t;
    vmls_f32 simd_sub float32x2_t; vmls_f64 simd_sub float64x1_t; vmlsq_f64 simd_sub float64x2_t;
}
neon_mul_n! { "neon":
    vmul_n_s16 i16 int16x4_t; vmulq_n_s16 i16 int16x8_t; vmul_n_s32 i32 int32x2_t;
    vmulq_n_s32 i32 int32x4_t; vmul_n_u16 u16 uint16x4_t; vmulq_n_u16 u16 uint16x8_t;
    vmul_n_u32 u32 uint32x2_t; vmulq_n_u32 u32 uint32x4_t; vmul_n_f32 f32 float32x2_t;
    vmul_n_f64 f64 float64x1_t;
}
neon_mul_acc_n! { "neon":
    vmla_n_s16 simd_add i16 int16x4_t; vmlaq_n_s16 simd_add i16 int16x8_t;
    vmla_n_s32 simd_add i32 int32x2_t; vmlaq_n_s32 simd_add i32 int32x4_t;
    vmla_n_u16 simd_add u16 uint16x4_t; vmlaq_n_u16 simd_add u16 uint16x8_t;
    vmla_n_u32 simd_add u32 uint32x2_t; vmlaq_n_u32 simd_add u32 uint32x4_t;
    vmla_n_f32 simd_add f32 float32x2_t; vmlaq_n_f32 simd_add f32 float32x4_t;
}
neon_mul_acc_n! { "neon":
    vmls_n_s16 simd_sub i16 int16x4_t; vmlsq_n_s16 simd_sub i16 int16x8_t;
    vmls_n_s32 simd_sub i32 int32x2_t; vmlsq_n_s32 simd_sub i32 int32x4_t;
    vmls_n_u16 simd_sub u16 uint16x4_t; vmlsq_n_u16 simd_sub u16 uint16x8_t;
    vmls_n_u32 simd_sub u32 uint32x2_t; vmlsq_n_u32 simd_sub u32 uint32x4_t;
    vmls_n_f32 simd_sub f32 float32x2_t; vmlsq_n_f32 simd_sub f32 float32x4_t;
}
neon_sat_doubling_mul_high! { "neon":
    vqdmulh_s16 i16 i64 int16x4_t false; vqdmulhq_s16 i16 i64 int16x8_t false;
    vqdmulh_s32 i32 i128 int32x2_t false; vqdmulhq_s32 i32 i128 int32x4_t false;
}
neon_sat_doubling_mul_high_n! { "neon":
    vqdmulh_n_s16 vqdmulh_s16 i16 int16x4_t; vqdmulhq_n_s16 vqdmulhq_s16 i16 int16x8_t;
    vqdmulh_n_s32 vqdmulh_s32 i32 int32x2_t; vqdmulhq_n_s32 vqdmulhq_s32 i32 int32x4_t;
}
neon_sat_doubling_mul_high! { "neon":
    vqrdmulh_s16 i16 i64 int16x4_t true; vqrdmulh_s32 i32 i128 int32x2_t true;
    vqrdmulhq_s32 i32 i128 int32x4_t true;
}
neon_sat_doubling_mul_high_n! { "neon":
    vqrdmulh_n_s16 vqrdmulh_s16 i16 int16x4_t; vqrdmulh_n_s32 vqrdmulh_s32 i32 int32x2_t;
    vqrdmulhq_n_s32 vqrdmulhq_s32 i32 int32x4_t;
}
// shifts by a constant, widening and narrowing
neon_shl_n! { "neon":
    vshl_n_s8 i8 int8x8_t; vshl_n_s16 i16 int16x4_t; vshl_n_s32 i32 int32x2_t;
    vshl_n_s64 i64 int64x1_t; vshl_n_u8 u8 uint8x8_t; vshl_n_u16 u16 uint16x4_t;
    vshl_n_u32 u32 uint32x2_t; vshl_n_u64 u64 uint64x1_t;
}
neon_shr_n_signed! { "neon":
    vshr_n_s8 i8 int8x8_t; vshr_n_s16 i16 int16x4_t; vshr_n_s32 i32 int32x2_t;
    vshr_n_s64 i64 int64x1_t;
}
neon_shr_n_unsigned! { "neon":
    vshr_n_u8 u8 uint8x8_t; vshr_n_u16 u16 uint16x4_t; vshr_n_u32 u32 uint32x2_t;
    vshr_n_u64 u64 uint64x1_t;
}
neon_shll_n! { "neon":
    vshll_n_s8 int8x8_t i16 int16x8_t; vshll_n_s16 int16x4_t i32 int32x4_t;
    vshll_n_s32 int32x2_t i64 int64x2_t; vshll_n_u8 uint8x8_t u16 uint16x8_t;
    vshll_n_u16 uint16x4_t u32 uint32x4_t; vshll_n_u32 uint32x2_t u64 uint64x2_t;
}
neon_shrn_n! { "neon":
    vshrn_n_s16 i16 int16x8_t int8x8_t; vshrn_n_s32 i32 int32x4_t int16x4_t;
    vshrn_n_s64 i64 int64x2_t int32x2_t; vshrn_n_u32 u32 uint32x4_t uint16x4_t;
}
neon_movn! { "neon":
    vmovn_s16 int16x8_t int8x8_t; vmovn_s32 int32x4_t int16x4_t; vmovn_s64 int64x2_t int32x2_t;
    vmovn_u16 uint16x8_t uint8x8_t; vmovn_u32 uint32x4_t uint16x4_t;
    vmovn_u64 uint64x2_t uint32x2_t;
}
// compares: all ones where true
neon_compare! { "neon", simd_eq:
    vceq_s8 int8x8_t uint8x8_t; vceq_s16 int16x4_t uint16x4_t; vceq_s32 int32x2_t uint32x2_t;
    vceq_s64 int64x1_t uint64x1_t; vceq_u16 uint16x4_t uint16x4_t; vceq_u32 uint32x2_t uint32x2_t;
    vceq_u64 uint64x1_t uint64x1_t; vceq_f32 float32x2_t uint32x2_t;
    vceq_f64 float64x1_t uint64x1_t; vceq_p8 poly8x8_t uint8x8_t; vceqq_p8 poly8x16_t uint8x16_t;
    vceq_p64 poly64x1_t uint64x1_t; vceqq_p64 poly64x2_t uint64x2_t;
}
neon_compare! { "neon", simd_ge:
    vcge_s8 int8x8_t uint8x8_t; vcge_s16 int16x4_t uint16x4_t; vcge_s32 int32x2_t uint32x2_t;
    vcge_s64 int64x1_t uint64x1_t; vcge_u8 uint8x8_t uint8x8_t; vcge_u16 uint16x4_t uint16x4_t;
    vcge_u32 uint32x2_t uint32x2_t; vcge_u64 uint64x1_t uint64x1_t; vcge_f32 float32x2_t uint32x2_t;
    vcge_f64 float64x1_t uint64x1_t;
}
neon_compare! { "neon", simd_gt:
    vcgt_s8 int8x8_t uint8x8_t; vcgt_s16 int16x4_t uint16x4_t; vcgt_s32 int32x2_t uint32x2_t;
    vcgt_s64 int64x1_t uint64x1_t; vcgt_u8 uint8x8_t uint8x8_t; vcgt_u16 uint16x4_t uint16x4_t;
    vcgt_u32 uint32x2_t uint32x2_t; vcgt_u64 uint64x1_t uint64x1_t; vcgt_f32 float32x2_t uint32x2_t;
    vcgt_f64 float64x1_t uint64x1_t;
}
neon_compare! { "neon", simd_le:
    vcle_s8 int8x8_t uint8x8_t; vcle_s16 int16x4_t uint16x4_t; vcle_s32 int32x2_t uint32x2_t;
    vcle_s64 int64x1_t uint64x1_t; vcle_u8 uint8x8_t uint8x8_t; vcle_u16 uint16x4_t uint16x4_t;
    vcle_u32 uint32x2_t uint32x2_t; vcle_u64 uint64x1_t uint64x1_t; vcle_f32 float32x2_t uint32x2_t;
    vcle_f64 float64x1_t uint64x1_t;
}
neon_compare! { "neon", simd_lt:
    vclt_s8 int8x8_t uint8x8_t; vclt_s16 int16x4_t uint16x4_t; vclt_s32 int32x2_t uint32x2_t;
    vclt_s64 int64x1_t uint64x1_t; vclt_u8 uint8x8_t uint8x8_t; vclt_u16 uint16x4_t uint16x4_t;
    vclt_u32 uint32x2_t uint32x2_t; vclt_u64 uint64x1_t uint64x1_t; vclt_f32 float32x2_t uint32x2_t;
    vclt_f64 float64x1_t uint64x1_t;
}
neon_compare_zero! { "neon", simd_eq:
    vceqz_s8 int8x8_t uint8x8_t; vceqzq_s8 int8x16_t uint8x16_t; vceqz_s16 int16x4_t uint16x4_t;
    vceqzq_s16 int16x8_t uint16x8_t; vceqz_s32 int32x2_t uint32x2_t;
    vceqzq_s32 int32x4_t uint32x4_t; vceqz_s64 int64x1_t uint64x1_t;
    vceqzq_s64 int64x2_t uint64x2_t; vceqz_u8 uint8x8_t uint8x8_t; vceqzq_u8 uint8x16_t uint8x16_t;
    vceqz_u16 uint16x4_t uint16x4_t; vceqzq_u16 uint16x8_t uint16x8_t;
    vceqz_u32 uint32x2_t uint32x2_t; vceqzq_u32 uint32x4_t uint32x4_t;
    vceqz_u64 uint64x1_t uint64x1_t; vceqzq_u64 uint64x2_t uint64x2_t;
    vceqz_f32 float32x2_t uint32x2_t; vceqzq_f32 float32x4_t uint32x4_t;
    vceqz_f64 float64x1_t uint64x1_t; vceqzq_f64 float64x2_t uint64x2_t;
    vceqz_p8 poly8x8_t uint8x8_t; vceqzq_p8 poly8x16_t uint8x16_t; vceqz_p64 poly64x1_t uint64x1_t;
    vceqzq_p64 poly64x2_t uint64x2_t;
}
neon_compare_zero! { "neon", simd_ge:
    vcgezq_s8 int8x16_t uint8x16_t; vcgez_s16 int16x4_t uint16x4_t; vcgezq_s16 int16x8_t uint16x8_t;
    vcgez_s32 int32x2_t uint32x2_t; vcgezq_s32 int32x4_t uint32x4_t; vcgez_s64 int64x1_t uint64x1_t;
    vcgezq_s64 int64x2_t uint64x2_t; vcgez_f32 float32x2_t uint32x2_t;
    vcgezq_f32 float32x4_t uint32x4_t; vcgez_f64 float64x1_t uint64x1_t;
    vcgezq_f64 float64x2_t uint64x2_t;
}
neon_compare_zero! { "neon", simd_gt:
    vcgtz_s8 int8x8_t uint8x8_t; vcgtzq_s8 int8x16_t uint8x16_t; vcgtz_s16 int16x4_t uint16x4_t;
    vcgtzq_s16 int16x8_t uint16x8_t; vcgtz_s32 int32x2_t uint32x2_t;
    vcgtzq_s32 int32x4_t uint32x4_t; vcgtz_s64 int64x1_t uint64x1_t;
    vcgtzq_s64 int64x2_t uint64x2_t; vcgtz_f32 float32x2_t uint32x2_t;
    vcgtzq_f32 float32x4_t uint32x4_t; vcgtz_f64 float64x1_t uint64x1_t;
    vcgtzq_f64 float64x2_t uint64x2_t;
}
neon_compare_zero! { "neon", simd_le:
    vclez_s8 int8x8_t uint8x8_t; vclezq_s8 int8x16_t uint8x16_t; vclez_s16 int16x4_t uint16x4_t;
    vclezq_s16 int16x8_t uint16x8_t; vclez_s32 int32x2_t uint32x2_t;
    vclezq_s32 int32x4_t uint32x4_t; vclez_s64 int64x1_t uint64x1_t;
    vclezq_s64 int64x2_t uint64x2_t; vclez_f32 float32x2_t uint32x2_t;
    vclezq_f32 float32x4_t uint32x4_t; vclez_f64 float64x1_t uint64x1_t;
    vclezq_f64 float64x2_t uint64x2_t;
}
neon_compare_zero! { "neon", simd_lt:
    vcltzq_s8 int8x16_t uint8x16_t; vcltz_s16 int16x4_t uint16x4_t; vcltzq_s16 int16x8_t uint16x8_t;
    vcltz_s32 int32x2_t uint32x2_t; vcltzq_s32 int32x4_t uint32x4_t; vcltz_s64 int64x1_t uint64x1_t;
    vcltzq_s64 int64x2_t uint64x2_t; vcltz_f32 float32x2_t uint32x2_t;
    vcltzq_f32 float32x4_t uint32x4_t; vcltz_f64 float64x1_t uint64x1_t;
    vcltzq_f64 float64x2_t uint64x2_t;
}
neon_test_bits! { "neon":
    vtst_s8 int8x8_t uint8x8_t; vtstq_s8 int8x16_t uint8x16_t; vtst_s16 int16x4_t uint16x4_t;
    vtstq_s16 int16x8_t uint16x8_t; vtst_s32 int32x2_t uint32x2_t; vtstq_s32 int32x4_t uint32x4_t;
    vtst_s64 int64x1_t uint64x1_t; vtstq_s64 int64x2_t uint64x2_t; vtst_u8 uint8x8_t uint8x8_t;
    vtstq_u8 uint8x16_t uint8x16_t; vtst_u16 uint16x4_t uint16x4_t; vtstq_u16 uint16x8_t uint16x8_t;
    vtst_u32 uint32x2_t uint32x2_t; vtstq_u32 uint32x4_t uint32x4_t; vtst_u64 uint64x1_t uint64x1_t;
    vtstq_u64 uint64x2_t uint64x2_t; vtst_p8 poly8x8_t uint8x8_t; vtstq_p8 poly8x16_t uint8x16_t;
    vtst_p16 poly16x4_t uint16x4_t; vtstq_p16 poly16x8_t uint16x8_t; vtst_p64 poly64x1_t uint64x1_t;
    vtstq_p64 poly64x2_t uint64x2_t;
}

// ---- pairwise min/max, two-register tables, unzips and zips (the NEON paths of base64 and hex
// codecs). Lane by lane, as Arm's UMAXP/UMINP, TBL, UZP1/UZP2, ZIP1/ZIP2.

/// UMAXP: the larger of each adjacent pair, `a`'s pairs first.
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vpmax_u32(a: uint32x2_t, b: uint32x2_t) -> uint32x2_t {
    uint32x2_t([a.0[0].max(a.0[1]), b.0[0].max(b.0[1])])
}

/// UMINP: the smaller of each adjacent pair, `a`'s pairs first.
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vpmin_u8(a: uint8x8_t, b: uint8x8_t) -> uint8x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 4 {
        r[i] = a.0[2 * i].min(a.0[2 * i + 1]);
        r[4 + i] = b.0[2 * i].min(b.0[2 * i + 1]);
        i += 1;
    }
    uint8x8_t(r)
}

/// TBL with a 32-byte table: an index >= 32 gives 0.
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vqtbl2q_u8(a: uint8x16x2_t, b: uint8x16_t) -> uint8x16_t {
    let mut r = [0; 16];
    let mut i = 0;
    while i < 16 {
        let k = b.0[i] as usize;
        r[i] = if k < 16 { a.0 .0[k] } else if k < 32 { a.1 .0[k - 16] } else { 0 };
        i += 1;
    }
    uint8x16_t(r)
}

/// TBL with a 16-byte table of two 8-byte halves: an index >= 16 gives 0.
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vtbl2_u8(a: uint8x8x2_t, b: uint8x8_t) -> uint8x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 8 {
        let k = b.0[i] as usize;
        r[i] = if k < 8 { a.0 .0[k] } else if k < 16 { a.1 .0[k - 8] } else { 0 };
        i += 1;
    }
    uint8x8_t(r)
}

/// UZP1: the even lanes of `a` then of `b`.
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vuzp1_u8(a: uint8x8_t, b: uint8x8_t) -> uint8x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 4 {
        r[i] = a.0[2 * i];
        r[4 + i] = b.0[2 * i];
        i += 1;
    }
    uint8x8_t(r)
}

/// UZP2: the odd lanes of `a` then of `b`.
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vuzp2_u8(a: uint8x8_t, b: uint8x8_t) -> uint8x8_t {
    let mut r = [0; 8];
    let mut i = 0;
    while i < 4 {
        r[i] = a.0[2 * i + 1];
        r[4 + i] = b.0[2 * i + 1];
        i += 1;
    }
    uint8x8_t(r)
}

/// UZP1 and UZP2 together.
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vuzp_u8(a: uint8x8_t, b: uint8x8_t) -> uint8x8x2_t {
    uint8x8x2_t(vuzp1_u8(a, b), vuzp2_u8(a, b))
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vuzpq_u8(a: uint8x16_t, b: uint8x16_t) -> uint8x16x2_t {
    uint8x16x2_t(vuzp1q_u8(a, b), vuzp2q_u8(a, b))
}

/// ZIP1 and ZIP2 together: the low halves of `a` and `b` interleaved, then the high halves.
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vzipq_u8(a: uint8x16_t, b: uint8x16_t) -> uint8x16x2_t {
    let (mut lo, mut hi) = ([0; 16], [0; 16]);
    let mut i = 0;
    while i < 8 {
        lo[2 * i] = a.0[i];
        lo[2 * i + 1] = b.0[i];
        hi[2 * i] = a.0[8 + i];
        hi[2 * i + 1] = b.0[8 + i];
        i += 1;
    }
    uint8x16x2_t(uint8x16_t(lo), uint8x16_t(hi))
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vzipq_u16(a: uint16x8_t, b: uint16x8_t) -> uint16x8x2_t {
    let (mut lo, mut hi) = ([0; 8], [0; 8]);
    let mut i = 0;
    while i < 4 {
        lo[2 * i] = a.0[i];
        lo[2 * i + 1] = b.0[i];
        hi[2 * i] = a.0[4 + i];
        hi[2 * i + 1] = b.0[4 + i];
        i += 1;
    }
    uint16x8x2_t(uint16x8_t(lo), uint16x8_t(hi))
}

#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
pub fn vzipq_u32(a: uint32x4_t, b: uint32x4_t) -> uint32x4x2_t {
    uint32x4x2_t(uint32x4_t([a.0[0], b.0[0], a.0[1], b.0[1]]), uint32x4_t([a.0[2], b.0[2], a.0[3], b.0[3]]))
}
