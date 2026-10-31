// rustc_middle/src/ty/consts/valtree.rs:114-121: const arguments retain their declared scalar type.
#[test]
fn signed_intrinsic_const_argument() {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        use core::arch::x86_64::{_mm_set1_epi32, _mm_shuffle_epi32, _mm_cvtsi128_si32, __m128i};
        let lanes = _mm_set1_epi32(17);
        let shuffle: unsafe fn(__m128i) -> __m128i = _mm_shuffle_epi32::<0x1b>;
        let shuffle = std::hint::black_box(shuffle);
        assert_eq!(_mm_cvtsi128_si32(shuffle(lanes)), 17);
    }
}
