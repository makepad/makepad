// The SSE2 / SSE4.1 / AVX2 intrinsics ppv-lite86 0.2.21 and rand_chacha 0.3.1 use beyond the
// common set (Robrix: rand 0.8's ChaCha rng), against rustc's core::arch.
#[cfg(target_arch = "x86_64")]
mod x86 {
use std::arch::x86_64::*;
use std::mem::transmute;

#[target_feature(enable = "sse4.1")]
unsafe fn sse41(a: [i64; 2]) -> ([i32; 4], [i64; 2], i64, i64) {
    let v = transmute::<[i64; 2], __m128i>(a);
    let i32s: [i32; 4] = transmute(_mm_insert_epi32(v, -7, 2));
    let i64s: [i64; 2] = transmute(_mm_insert_epi64(v, 0x0123_4567_89ab_cdef, 1));
    (i32s, i64s, _mm_extract_epi64(v, 0), _mm_extract_epi64(v, 1))
}

#[target_feature(enable = "sse4.1")]
unsafe fn cmpeq64(a: [i64; 2], b: [i64; 2]) -> [i64; 2] {
    transmute(_mm_cmpeq_epi64(transmute(a), transmute(b)))
}

unsafe fn sse2(a: [i16; 8]) -> ([i16; 8], [i16; 8], [i16; 8], [u8; 16], [u8; 16], [i64; 2]) {
    let v = transmute::<[i16; 8], __m128i>(a);
    (
        transmute(_mm_shufflelo_epi16(v, 0b00_01_10_11)),
        transmute(_mm_shufflehi_epi16(v, 0b11_00_10_01)),
        transmute(_mm_shufflehi_epi16(_mm_shufflelo_epi16(v, 0x39), 0x93)),
        transmute(_mm_slli_si128(v, 3)),
        transmute(_mm_slli_si128(v, 17)),
        transmute(_mm_move_epi64(v)),
    )
}

#[target_feature(enable = "avx2")]
unsafe fn avx2(a: [i64; 4], b: [i64; 4], h: [i64; 2]) -> ([i64; 4], [i64; 4], [i64; 4]) {
    let va = transmute::<[i64; 4], __m256i>(a);
    let vb = transmute::<[i64; 4], __m256i>(b);
    let vh = transmute::<[i64; 2], __m128i>(h);
    (
        transmute(_mm256_andnot_si256(va, vb)),
        transmute(_mm256_inserti128_si256(va, vh, 0)),
        transmute(_mm256_inserti128_si256(va, vh, 1)),
    )
}

#[test]
fn ppv_lite_intrinsics() {
    // expected values: rustc's core::arch on x86-64
    unsafe {
        let a = [0x1111_2222_3333_4444i64, -0x5555_6666_7777_8888i64];
        if is_x86_feature_detected!("sse4.1") {
            assert_eq!(sse41(a), ([858997828, 286335522, -7, -1431660135], [1229801703532086340, 81985529216486895], 1229801703532086340, -6148933456521300104));
            assert_eq!((cmpeq64(a, [a[0], 5]), cmpeq64(a, a)), ([-1, 0], [-1, -1]));
        }
        assert_eq!(sse2([1, -2, 3, -4, 5, -6, 7, -8]), ([-4, 3, -2, 1, 5, -6, 7, -8], [1, -2, 3, -4, -6, 7, 5, -8], [-2, 3, -4, 1, -8, 5, -6, 7],
            [0, 0, 0, 1, 0, 254, 255, 3, 0, 252, 255, 5, 0, 250, 255, 7], [0; 16], [-1125882727104511, 0]));
        if is_x86_feature_detected!("avx2") {
            assert_eq!(avx2([1, -1, 0x0f0f, i64::MIN], [-1, -1, 0xffff, -1], [9, 10]),
                ([-2, 0, 61680, 9223372036854775807], [9, 10, 3855, -9223372036854775808], [1, -1, 9, 10]));
            _mm256_zeroupper();
        }
    }
}
}
