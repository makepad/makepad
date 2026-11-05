// Arm's __crc32b / __crc32d and Intel's _mm_clmulepi64_si128 (a CRC's hardware steps: Arm's crc32
// instructions, x86's carry-less multiply for folding) against bitwise references, on the host's
// own architecture; called often enough to tier up. Run with `makepad test` and rustc -O --test.
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
    use core::arch::aarch64::{__crc32b, __crc32d};
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
    let v = inputs();
    for round in 0..300 {
        for (k, &a) in v.iter().enumerate() {
            let b = v[(k + round) % v.len()];
            assert_eq!(unsafe { __crc32d(a as u32, b) }, crc32_bits(a as u32, b, 64), "a {a:#x} b {b:#x}");
            assert_eq!(unsafe { __crc32b(a as u32, b as u8) }, crc32_bits(a as u32, b & 0xff, 8), "a {a:#x} b {b:#x}");
        }
    }
    println!("crc32 intrinsics: ok");
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
