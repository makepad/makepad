// Arm's __crc32b/h/w/d, Arm's vmull_p64 / vmull_p8 family and Intel's _mm_clmulepi64_si128 (a
// CRC's hardware steps: the crc32 instructions, the carry-less multiplies for folding) against
// bitwise references, on the host's own architecture; called often enough to tier up. Run with
// `makepad test` and rustc -O --test.
#![allow(unused)]
use std::hint::black_box;

fn crc32_bits(mut r: u32, data: u64, bits: u32) -> u32 {
    for i in 0..bits {
        r ^= ((data >> i) & 1) as u32;
        r = (r >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(r & 1));
    }
    r
}

fn clmul_ref(x: u64, y: u64) -> u128 {
    let mut r = 0u128;
    for i in 0..64 {
        if (y >> i) & 1 != 0 {
            r ^= (x as u128) << i;
        }
    }
    r
}

fn inputs() -> Vec<u64> {
    let mut s = 0x9e37_79b9_7f4a_7c15u64;
    let mut v = vec![0, 1, u64::MAX, 1 << 63, 0xffff_ffff, 0x8000_0001, 0x0123_4567_89ab_cdef];
    for _ in 0..60 {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        v.push(s);
    }
    v
}

#[cfg(target_arch = "aarch64")]
#[test]
fn crc32_intrinsics() {
    use core::arch::aarch64::{__crc32b, __crc32d, __crc32h, __crc32w};
    if !std::arch::is_aarch64_feature_detected!("crc") {
        return;
    }
    // CRC-32's check value over "123456789", byte steps and one 8-byte step + a byte
    let mut r = !0u32;
    for &c in b"123456789" {
        r = unsafe { __crc32b(r, black_box(c)) };
    }
    assert_eq!(!r, 0xcbf4_3926);
    let w = u64::from_le_bytes(*b"12345678");
    assert_eq!(!unsafe { __crc32b(__crc32d(!0, black_box(w)), b'9') }, 0xcbf4_3926);
    let (w4, h2) = (u32::from_le_bytes(*b"1234"), u16::from_le_bytes(*b"56"));
    assert_eq!(!unsafe { __crc32b(__crc32h(__crc32h(__crc32w(!0, black_box(w4)), h2), u16::from_le_bytes(*b"78")), b'9') }, 0xcbf4_3926);
    let v = inputs();
    for round in 0..300 {
        for (k, &a) in v.iter().enumerate() {
            let b = v[(k + round) % v.len()];
            assert_eq!(unsafe { __crc32d(a as u32, b) }, crc32_bits(a as u32, b, 64), "a {a:#x} b {b:#x}");
            assert_eq!(unsafe { __crc32b(a as u32, b as u8) }, crc32_bits(a as u32, b & 0xff, 8), "a {a:#x} b {b:#x}");
            assert_eq!(unsafe { __crc32h(a as u32, b as u16) }, crc32_bits(a as u32, b & 0xffff, 16), "h: a {a:#x} b {b:#x}");
            assert_eq!(unsafe { __crc32w(a as u32, b as u32) }, crc32_bits(a as u32, b & 0xffff_ffff, 32), "w: a {a:#x} b {b:#x}");
        }
    }
    println!("crc32 intrinsics: ok");
}

#[cfg(target_arch = "aarch64")]
#[test]
fn pmull_intrinsics() {
    use core::arch::aarch64::{vcombine_p64, vcreate_p64, vcreate_p8, vmul_p8, vmull_high_p64, vmull_p64, vmull_p8};
    if !std::arch::is_aarch64_feature_detected!("aes") {
        return;
    }
    let v = inputs();
    for round in 0..100 {
        for (k, &a) in v.iter().enumerate() {
            let b = v[(k * 7 + round) % v.len()];
            assert_eq!(unsafe { vmull_p64(black_box(a), b) }, clmul_ref(a, b), "vmull_p64 a {a:#x} b {b:#x}");
            let (va, vb) = unsafe { (vcombine_p64(vcreate_p64(b), vcreate_p64(a)), vcombine_p64(vcreate_p64(a), vcreate_p64(b))) };
            assert_eq!(unsafe { vmull_high_p64(black_box(va), vb) }, clmul_ref(a, b), "vmull_high_p64 a {a:#x} b {b:#x}");
            let (x, y) = unsafe { (vcreate_p8(black_box(a)), vcreate_p8(b)) };
            let wide: [u16; 8] = unsafe { core::mem::transmute(vmull_p8(x, y)) };
            let low: [u8; 8] = unsafe { core::mem::transmute(vmul_p8(x, y)) };
            for i in 0..8 {
                let r = clmul_ref((a >> (8 * i)) & 0xff, (b >> (8 * i)) & 0xff) as u16;
                assert_eq!((wide[i], low[i]), (r, r as u8), "vmull_p8 / vmul_p8 lane {i} a {a:#x} b {b:#x}");
            }
        }
    }
    println!("pmull intrinsics: ok");
}

#[cfg(target_arch = "x86_64")]
#[test]
fn clmul_intrinsic() {
    use core::arch::x86_64::*;
    if !std::arch::is_x86_feature_detected!("pclmulqdq") {
        return;
    }
    #[target_feature(enable = "pclmulqdq")]
    unsafe fn all(a: __m128i, b: __m128i) -> [u128; 4] {
        let f = |r: __m128i| core::mem::transmute::<__m128i, u128>(r);
        [f(_mm_clmulepi64_si128::<0x00>(a, b)), f(_mm_clmulepi64_si128::<0x01>(a, b)), f(_mm_clmulepi64_si128::<0x10>(a, b)), f(_mm_clmulepi64_si128::<0x11>(a, b))]
    }
    let v = inputs();
    for round in 0..300 {
        for (k, &x0) in v.iter().enumerate() {
            let (x1, y0, y1) = (v[(k + round) % v.len()], v[(k * 7 + 3) % v.len()], v[(k + 2 * round + 1) % v.len()]);
            let a = unsafe { _mm_set_epi64x(x1 as i64, x0 as i64) };
            let b = unsafe { _mm_set_epi64x(y1 as i64, y0 as i64) };
            let got = unsafe { all(black_box(a), black_box(b)) };
            assert_eq!(got, [clmul_ref(x0, y0), clmul_ref(x1, y0), clmul_ref(x0, y1), clmul_ref(x1, y1)], "{x0:#x} {x1:#x} {y0:#x} {y1:#x}");
        }
    }
    println!("clmul intrinsic: ok");
}
