#![cfg(not(target_arch = "x86_64"))]
#![allow(non_snake_case)]
include!("common/mod.rs");
use relax_std_arch_check::hr_x86 as hr;
#[path = "common/intel.rs"]
mod intel;

#[test]
fn _MM_SHUFFLE() {
    intel::_MM_SHUFFLE();
}

#[test]
fn _mm256_add_epi16() {
    intel::_mm256_add_epi16();
}

#[test]
fn _mm256_add_epi32() {
    intel::_mm256_add_epi32();
}

#[test]
fn _mm256_blend_epi32() {
    intel::_mm256_blend_epi32();
}

#[test]
fn _mm256_blendv_epi8() {
    intel::_mm256_blendv_epi8();
}

#[test]
fn _mm256_castsi256_si128() {
    intel::_mm256_castsi256_si128();
}

#[test]
fn _mm256_extractf128_si256() {
    intel::_mm256_extractf128_si256();
}

#[test]
fn _mm256_extracti128_si256() {
    intel::_mm256_extracti128_si256();
}

#[test]
fn _mm256_loadu_si256() {
    intel::_mm256_loadu_si256();
}

#[test]
fn _mm256_madd_epi16() {
    intel::_mm256_madd_epi16();
}

#[test]
fn _mm256_maddubs_epi16() {
    intel::_mm256_maddubs_epi16();
}

#[test]
fn _mm256_max_epi16() {
    intel::_mm256_max_epi16();
}

#[test]
fn _mm256_min_epi16() {
    intel::_mm256_min_epi16();
}

#[test]
fn _mm256_mullo_epi16() {
    intel::_mm256_mullo_epi16();
}

#[test]
fn _mm256_mullo_epi32() {
    intel::_mm256_mullo_epi32();
}

#[test]
fn _mm256_or_si256() {
    intel::_mm256_or_si256();
}

#[test]
fn _mm256_packs_epi32() {
    intel::_mm256_packs_epi32();
}

#[test]
fn _mm256_packus_epi16() {
    intel::_mm256_packus_epi16();
}

#[test]
fn _mm256_packus_epi32() {
    intel::_mm256_packus_epi32();
}

#[test]
fn _mm256_permute2x128_si256() {
    intel::_mm256_permute2x128_si256();
}

#[test]
fn _mm256_permute4x64_epi64() {
    intel::_mm256_permute4x64_epi64();
}

#[test]
fn _mm256_sad_epu8() {
    intel::_mm256_sad_epu8();
}

#[test]
fn _mm256_set1_epi16() {
    intel::_mm256_set1_epi16();
}

#[test]
fn _mm256_set1_epi32() {
    intel::_mm256_set1_epi32();
}

#[test]
fn _mm256_set_epi32() {
    intel::_mm256_set_epi32();
}

#[test]
fn _mm256_set_epi8() {
    intel::_mm256_set_epi8();
}

#[test]
fn _mm256_setr_epi8() {
    intel::_mm256_setr_epi8();
}

#[test]
fn _mm256_setzero_si256() {
    intel::_mm256_setzero_si256();
}

#[test]
fn _mm256_shuffle_epi8() {
    intel::_mm256_shuffle_epi8();
}

#[test]
fn _mm256_slli_epi16() {
    intel::_mm256_slli_epi16();
}

#[test]
fn _mm256_slli_epi32() {
    intel::_mm256_slli_epi32();
}

#[test]
fn _mm256_srai_epi16() {
    intel::_mm256_srai_epi16();
}

#[test]
fn _mm256_srai_epi32() {
    intel::_mm256_srai_epi32();
}

#[test]
fn _mm256_storeu_si256() {
    intel::_mm256_storeu_si256();
}

#[test]
fn _mm256_sub_epi16() {
    intel::_mm256_sub_epi16();
}

#[test]
fn _mm256_sub_epi32() {
    intel::_mm256_sub_epi32();
}

#[test]
fn _mm256_testz_si256() {
    intel::_mm256_testz_si256();
}

#[test]
fn _mm256_unpackhi_epi16() {
    intel::_mm256_unpackhi_epi16();
}

#[test]
fn _mm256_unpackhi_epi32() {
    intel::_mm256_unpackhi_epi32();
}

#[test]
fn _mm256_unpackhi_epi64() {
    intel::_mm256_unpackhi_epi64();
}

#[test]
fn _mm256_unpackhi_epi8() {
    intel::_mm256_unpackhi_epi8();
}

#[test]
fn _mm256_unpacklo_epi16() {
    intel::_mm256_unpacklo_epi16();
}

#[test]
fn _mm256_unpacklo_epi32() {
    intel::_mm256_unpacklo_epi32();
}

#[test]
fn _mm256_unpacklo_epi64() {
    intel::_mm256_unpacklo_epi64();
}

#[test]
fn _mm256_unpacklo_epi8() {
    intel::_mm256_unpacklo_epi8();
}

#[test]
fn _mm512_add_epi32() {
    intel::_mm512_add_epi32();
}

#[test]
fn _mm512_loadu_si512() {
    intel::_mm512_loadu_si512();
}

#[test]
fn _mm512_madd_epi16() {
    intel::_mm512_madd_epi16();
}

#[test]
fn _mm512_maddubs_epi16() {
    intel::_mm512_maddubs_epi16();
}

#[test]
fn _mm512_sad_epu8() {
    intel::_mm512_sad_epu8();
}

#[test]
fn _mm512_set1_epi16() {
    intel::_mm512_set1_epi16();
}

#[test]
fn _mm512_set_epi32() {
    intel::_mm512_set_epi32();
}

#[test]
fn _mm512_set_epi8() {
    intel::_mm512_set_epi8();
}

#[test]
fn _mm512_setzero_si512() {
    intel::_mm512_setzero_si512();
}

#[test]
fn _mm512_slli_epi32() {
    intel::_mm512_slli_epi32();
}

#[test]
fn _mm_abs_epi16() {
    intel::_mm_abs_epi16();
}

#[test]
fn _mm_add_epi16() {
    intel::_mm_add_epi16();
}

#[test]
fn _mm_add_epi32() {
    intel::_mm_add_epi32();
}

#[test]
fn _mm_add_epi8() {
    intel::_mm_add_epi8();
}

#[test]
fn _mm_and_si128() {
    intel::_mm_and_si128();
}

#[test]
fn _mm_andnot_si128() {
    intel::_mm_andnot_si128();
}

#[test]
fn _mm_avg_epu8() {
    intel::_mm_avg_epu8();
}

#[test]
fn _mm_blendv_epi8() {
    intel::_mm_blendv_epi8();
}

#[test]
fn _mm_clmulepi64_si128() {
    intel::_mm_clmulepi64_si128();
}

#[test]
fn _mm_cmpeq_epi16() {
    intel::_mm_cmpeq_epi16();
}

#[test]
fn _mm_cvtsi128_si32() {
    intel::_mm_cvtsi128_si32();
}

#[test]
fn _mm_cvtsi32_si128() {
    intel::_mm_cvtsi32_si128();
}

#[test]
fn _mm_extract_epi32() {
    intel::_mm_extract_epi32();
}

#[test]
fn _mm_load_si128() {
    intel::_mm_load_si128();
}

#[test]
fn _mm_loadu_si128() {
    intel::_mm_loadu_si128();
}

#[test]
fn _mm_madd_epi16() {
    intel::_mm_madd_epi16();
}

#[test]
fn _mm_maddubs_epi16() {
    intel::_mm_maddubs_epi16();
}

#[test]
fn _mm_min_epi16() {
    intel::_mm_min_epi16();
}

#[test]
fn _mm_or_si128() {
    intel::_mm_or_si128();
}

#[test]
fn _mm_packus_epi16() {
    intel::_mm_packus_epi16();
}

#[test]
fn _mm_sad_epu8() {
    intel::_mm_sad_epu8();
}

#[test]
fn _mm_set1_epi16() {
    intel::_mm_set1_epi16();
}

#[test]
fn _mm_set1_epi8() {
    intel::_mm_set1_epi8();
}

#[test]
fn _mm_set_epi32() {
    intel::_mm_set_epi32();
}

#[test]
fn _mm_set_epi64x() {
    intel::_mm_set_epi64x();
}

#[test]
fn _mm_set_epi8() {
    intel::_mm_set_epi8();
}

#[test]
fn _mm_setzero_si128() {
    intel::_mm_setzero_si128();
}

#[test]
fn _mm_sha1msg1_epu32() {
    intel::_mm_sha1msg1_epu32();
}

#[test]
fn _mm_sha1msg2_epu32() {
    intel::_mm_sha1msg2_epu32();
}

#[test]
fn _mm_sha1nexte_epu32() {
    intel::_mm_sha1nexte_epu32();
}

#[test]
fn _mm_sha1rnds4_epu32() {
    intel::_mm_sha1rnds4_epu32();
}

#[test]
fn _mm_shuffle_epi32() {
    intel::_mm_shuffle_epi32();
}

#[test]
fn _mm_shuffle_epi8() {
    intel::_mm_shuffle_epi8();
}

#[test]
fn _mm_slli_epi32() {
    intel::_mm_slli_epi32();
}

#[test]
fn _mm_srli_si128() {
    intel::_mm_srli_si128();
}

#[test]
fn _mm_storeu_si128() {
    intel::_mm_storeu_si128();
}

#[test]
fn _mm_sub_epi16() {
    intel::_mm_sub_epi16();
}

#[test]
fn _mm_sub_epi8() {
    intel::_mm_sub_epi8();
}

#[test]
fn _mm_unpackhi_epi64() {
    intel::_mm_unpackhi_epi64();
}

#[test]
fn _mm_unpackhi_epi8() {
    intel::_mm_unpackhi_epi8();
}

#[test]
fn _mm_unpacklo_epi8() {
    intel::_mm_unpacklo_epi8();
}

#[test]
fn _mm_xor_si128() {
    intel::_mm_xor_si128();
}

#[test]
fn _mm_cmpeq_epi8() {
    intel::_mm_cmpeq_epi8();
}

#[test]
fn _mm_cmpgt_epi8() {
    intel::_mm_cmpgt_epi8();
}

#[test]
fn _mm_movemask_epi8() {
    intel::_mm_movemask_epi8();
}

#[test]
fn _mm_store_si128() {
    intel::_mm_store_si128();
}
