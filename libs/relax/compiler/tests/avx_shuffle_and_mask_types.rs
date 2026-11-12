//! `_mm256_shuffle_ps` shuffles each 128-bit half with the same immediate, and the AVX-512 mask
//! types `__mmask8` to `__mmask64` are the integer aliases stdarch declares (fearless_simd 0.7.0's
//! generated avx512.rs names them in its signatures).
#![cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;

#[target_feature(enable = "avx")]
unsafe fn shuffle() -> [f32; 8] {
    let a = _mm256_setr_ps(0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0);
    let b = _mm256_setr_ps(10.0, 11.0, 12.0, 13.0, 14.0, 15.0, 16.0, 17.0);
    let r = _mm256_shuffle_ps(a, b, 0b01_00_11_10);
    let mut out = [0.0f32; 8];
    _mm256_storeu_ps(out.as_mut_ptr(), r);
    out
}

#[test]
fn mm256_shuffle_ps_shuffles_each_half() {
    if !std::arch::is_x86_feature_detected!("avx") {
        return;
    }
    let out = unsafe { shuffle() };
    assert_eq!(out, [2.0, 3.0, 10.0, 11.0, 6.0, 7.0, 14.0, 15.0], "lanes 2,3 of a then 0,1 of b, per half");
}

#[test]
fn avx512_mask_types_are_integers() {
    let masks: (__mmask8, __mmask16, __mmask32, __mmask64) = (0xff, 0xffff, u32::MAX, u64::MAX);
    assert_eq!(
        (masks.0.count_ones(), masks.1.count_ones(), masks.2.count_ones(), masks.3.count_ones()),
        (8, 16, 32, 64),
        "one bit per lane"
    );
    assert_eq!(std::mem::size_of::<__mmask16>(), 2, "__mmask16 is a u16");
}
