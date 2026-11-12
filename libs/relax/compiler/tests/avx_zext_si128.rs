//! `_mm256_zextsi128_si256` puts a 128-bit vector in the low half of a 256-bit one and zeroes the
//! high half (zlib-rs's AVX2 adler32 calls it).
#![cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;

#[target_feature(enable = "avx")]
unsafe fn zext(lo: i64, hi: i64) -> [i64; 4] {
    let r = _mm256_zextsi128_si256(_mm_set_epi64x(hi, lo));
    let mut out = [0i64; 4];
    _mm256_storeu_si256(out.as_mut_ptr().cast(), r);
    out
}

#[test]
fn mm256_zextsi128_si256_zeroes_the_high_half() {
    if !std::arch::is_x86_feature_detected!("avx") {
        return;
    }
    assert_eq!(unsafe { zext(-1, 7) }, [-1, 7, 0, 0], "the low half is the input, the high half zero");
}
