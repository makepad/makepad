// The aarch64 NEON intrinsics zmij's digit formatting imports: widening and saturating
// arithmetic, lane shuffles and the reinterpretations, each against rustc's std::arch.
#![cfg(target_arch = "aarch64")]
use std::arch::aarch64::*;

unsafe fn u8x16(v: uint8x16_t) -> [u8; 16] { std::mem::transmute(v) }
unsafe fn i16x8(v: int16x8_t) -> [i16; 8] { std::mem::transmute(v) }
unsafe fn i32x4(v: int32x4_t) -> [i32; 4] { std::mem::transmute(v) }
unsafe fn i32x2(v: int32x2_t) -> [i32; 2] { std::mem::transmute(v) }
unsafe fn u32x2(v: uint32x2_t) -> [u32; 2] { std::mem::transmute(v) }
unsafe fn u32x4(v: uint32x4_t) -> [u32; 4] { std::mem::transmute(v) }
unsafe fn u16x8(v: uint16x8_t) -> [u16; 8] { std::mem::transmute(v) }
unsafe fn u16x4(v: uint16x4_t) -> [u16; 4] { std::mem::transmute(v) }

#[test]
fn a01_compare_combine_create_dup() {
    unsafe {
        let s8: int8x16_t = std::mem::transmute([-3i8, 0, 1, 127, -128, 5, 0, 9, 1, 1, 1, 1, 0, 0, 0, 2]);
        println!("| cgtz {:?}", u8x16(vcgtzq_s8(s8)));
        let a: int32x2_t = std::mem::transmute([1i32, -2]);
        let b: int32x2_t = std::mem::transmute([3i32, i32::MIN]);
        println!("| combine {:?}", i32x4(vcombine_s32(a, b)));
        let c = vcreate_u64(0x0102_0304_0506_0708);
        println!("| create {:?} {:?}", i32x2(vreinterpret_s32_u64(c)), u8x16(vreinterpretq_u8_u64(vcombine_u64(c, c))));
        println!("| dup {:?}", i32x2(vdup_n_s32(-7)));
    }
}

#[test]
fn a02_multiply_accumulate_and_doubling_high() {
    unsafe {
        let a: int32x2_t = std::mem::transmute([10i32, i32::MAX]);
        let b: int32x2_t = std::mem::transmute([3i32, 2]);
        println!("| mla_n {:?}", i32x2(vmla_n_s32(a, b, 7)));
        let a16: int16x8_t = std::mem::transmute([1i16, 2, 3, 4, i16::MAX, i16::MIN, -1, 0]);
        println!("| mlaq_n_s16 {:?}", i16x8(vmlaq_n_s16(a16, a16, 300)));
        let a32: int32x4_t = std::mem::transmute([1i32, -2, i32::MAX, i32::MIN]);
        println!("| mlaq_n_s32 {:?}", i32x4(vmlaq_n_s32(a32, a32, 70_000)));
        println!("| qdmulh_n_s32 {:?} {:?}", i32x2(vqdmulh_n_s32(a, 0x4000_0000)), i32x2(vqdmulh_n_s32(std::mem::transmute([i32::MIN, 7]), i32::MIN)));
        println!("| qdmulhq_n_s16 {:?}", i16x8(vqdmulhq_n_s16(a16, 0x4000)));
        println!("| qdmulhq_n_s16 sat {:?}", i16x8(vqdmulhq_n_s16(a16, i16::MIN)));
        println!("| qdmulhq_n_s32 {:?}", i32x4(vqdmulhq_n_s32(a32, 0x3333_3333)));
    }
}

#[test]
fn a03_reinterpret_reverse_shift() {
    unsafe {
        let u: uint32x2_t = std::mem::transmute([0x8000_0001u32, 0x1234_5678]);
        let s = vreinterpret_s32_u32(u);
        println!("| reinterpret {:?} {:?} {:?}", i32x2(s), u16x4(vreinterpret_u16_s32(s)), u32x2(vreinterpret_u32_s32(s)));
        let s8: int8x16_t = std::mem::transmute([1i8, -1, 2, -2, 3, -3, 4, -4, 5, -5, 6, -6, 7, -7, 8, -8]);
        println!("| reinterpretq_u16_s8 {:?}", u16x8(vreinterpretq_u16_s8(s8)));
        let s16: int16x8_t = std::mem::transmute([0x0102i16, -2, 3, 4, 5, 6, 7, 8]);
        println!("| reinterpretq_u8_s16 {:?}", u8x16(vreinterpretq_u8_s16(s16)));
        let bytes: uint8x16_t = std::mem::transmute([0u8, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]);
        println!("| rev64q {:?}", u8x16(vrev64q_u8(bytes)));
        let w: uint16x4_t = std::mem::transmute([1u16, 0x8000, 0xffff, 7]);
        println!("| shll_n {:?} {:?}", u32x4(vshll_n_u16(w, 4)), u32x4(vshll_n_u16::<16>(w)));
        println!("| shr_n {:?} {:?}", u32x2(vshr_n_u32(u, 3)), u32x2(vshr_n_u32::<31>(u)));
        // the ends of N's range (1..=32): a shift by the lane width gives zero
        println!("| shr_n ends {:?} {:?}", u32x2(vshr_n_u32::<1>(u)), u32x2(vshr_n_u32::<32>(u)));
    }
}
