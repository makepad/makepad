/*
* Copyright(c) 2019 Intel Corporation
*
* This source code is subject to the terms of the BSD 2 Clause License and
* the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
* was not distributed with this source code in the LICENSE file, you can
* obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
* Media Patent License 1.0 was not distributed with this source code in the
* PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
*/

#include "definitions.h"

#if EN_AVX512_SUPPORT
#include <assert.h>
#include "compute_sad_avx2.h"
#include <immintrin.h>
#include "memory_avx2.h"
#include "synonyms_avx2.h"
#include "utility.h"
#include "compute_sad_c.h"

static INLINE void sad64_kernel_avx512(const __m512i s, const uint8_t* const ref, __m512i* const sum) {
    const __m512i r = _mm512_loadu_si512((__m512i*)ref);
    *sum            = _mm512_add_epi32(*sum, _mm512_sad_epu8(s, r));
}

static INLINE void sad64_avx512(const uint8_t* const src, const uint8_t* ref, __m512i* const sum) {
    const __m512i s = _mm512_loadu_si512((__m512i*)src);
    sad64_kernel_avx512(s, ref, sum);
}

static INLINE uint32_t sad_final_avx512(const __m512i zmm) {
    const __m256i zmm_L  = _mm512_castsi512_si256(zmm);
    const __m256i zmm_H  = _mm512_extracti64x4_epi64(zmm, 1);
    const __m256i ymm    = _mm256_add_epi32(zmm_L, zmm_H);
    const __m128i ymm_L  = _mm256_castsi256_si128(ymm);
    const __m128i ymm_H  = _mm256_extracti128_si256(ymm, 1);
    const __m128i xmm0   = _mm_add_epi32(ymm_L, ymm_H);
    const __m128i xmm0_H = _mm_srli_si128(xmm0, 8);
    const __m128i xmm1   = _mm_add_epi32(xmm0, xmm0_H);

    return _mm_extract_epi32(xmm1, 0);
}

/*******************************************************************************
* Requirement: height % 2 = 0
*******************************************************************************/
SIMD_INLINE uint32_t compute64x_m_sad_avx512_intrin(const uint8_t* src, uint32_t src_stride, const uint8_t* ref,
                                                    uint32_t ref_stride, uint32_t height) {
    uint32_t y   = height;
    __m512i  zmm = _mm512_setzero_si512();

    do {
        sad64_avx512(src + 0 * src_stride, ref + 0 * ref_stride, &zmm);
        sad64_avx512(src + 1 * src_stride, ref + 1 * ref_stride, &zmm);
        src += src_stride << 1;
        ref += ref_stride << 1;
        y -= 2;
    } while (y);

    return sad_final_avx512(zmm);
}

/*******************************************************************************
* Requirement: height % 2 = 0
*******************************************************************************/
SIMD_INLINE uint32_t compute128x_m_sad_avx512_intrin(const uint8_t* src, // input parameter, source samples Ptr
                                                     uint32_t       src_stride, // input parameter, source stride
                                                     const uint8_t* ref, // input parameter, reference samples Ptr
                                                     uint32_t       ref_stride, // input parameter, reference stride
                                                     uint32_t       height) // input parameter, block height (M)
{
    uint32_t y   = height;
    __m512i  zmm = _mm512_setzero_si512();

    do {
        sad64_avx512(src + 0 * src_stride + 0 * 64, ref + 0 * ref_stride + 0 * 64, &zmm);
        sad64_avx512(src + 0 * src_stride + 1 * 64, ref + 0 * ref_stride + 1 * 64, &zmm);
        sad64_avx512(src + 1 * src_stride + 0 * 64, ref + 1 * ref_stride + 0 * 64, &zmm);
        sad64_avx512(src + 1 * src_stride + 1 * 64, ref + 1 * ref_stride + 1 * 64, &zmm);
        src += src_stride << 1;
        ref += ref_stride << 1;
        y -= 2;
    } while (y);

    return sad_final_avx512(zmm);
}

uint32_t svt_aom_sad64x16_avx512(const uint8_t* src_ptr, int src_stride, const uint8_t* ref_ptr, int ref_stride) {
    return compute64x_m_sad_avx512_intrin(src_ptr, src_stride, ref_ptr, ref_stride, 16);
}

uint32_t svt_aom_sad64x32_avx512(const uint8_t* src_ptr, int src_stride, const uint8_t* ref_ptr, int ref_stride) {
    return compute64x_m_sad_avx512_intrin(src_ptr, src_stride, ref_ptr, ref_stride, 32);
}

uint32_t svt_aom_sad64x64_avx512(const uint8_t* src_ptr, int src_stride, const uint8_t* ref_ptr, int ref_stride) {
    return compute64x_m_sad_avx512_intrin(src_ptr, src_stride, ref_ptr, ref_stride, 64);
}

uint32_t svt_aom_sad64x128_avx512(const uint8_t* src_ptr, int src_stride, const uint8_t* ref_ptr, int ref_stride) {
    return compute64x_m_sad_avx512_intrin(src_ptr, src_stride, ref_ptr, ref_stride, 128);
}

uint32_t svt_aom_sad128x64_avx512(const uint8_t* src_ptr, int src_stride, const uint8_t* ref_ptr, int ref_stride) {
    return compute128x_m_sad_avx512_intrin(src_ptr, src_stride, ref_ptr, ref_stride, 64);
}

uint32_t svt_aom_sad128x128_avx512(const uint8_t* src_ptr, int src_stride, const uint8_t* ref_ptr, int ref_stride) {
    return compute128x_m_sad_avx512_intrin(src_ptr, src_stride, ref_ptr, ref_stride, 128);
}

static INLINE void sad64_4d_avx512(const uint8_t* const src, const uint8_t* const ref_array[4], const uint32_t offset,
                                   __m512i sum[4]) {
    const __m512i s = _mm512_loadu_si512((__m512i*)(src + offset));
    sad64_kernel_avx512(s, ref_array[0] + offset, &sum[0]);
    sad64_kernel_avx512(s, ref_array[1] + offset, &sum[1]);
    sad64_kernel_avx512(s, ref_array[2] + offset, &sum[2]);
    sad64_kernel_avx512(s, ref_array[3] + offset, &sum[3]);
}

static INLINE __m256i add_hi_lo_32_avx512(const __m512i src) {
    const __m256i s0 = _mm512_castsi512_si256(src);
    const __m256i s1 = _mm512_extracti64x4_epi64(src, 1);
    return _mm256_add_epi32(s0, s1);
}

static INLINE __m128i hadd_four_32_avx2(const __m256i src0, const __m256i src1, const __m256i src2,
                                        const __m256i src3) {
    const __m256i s01   = _mm256_hadd_epi32(src0, src1); // 0 0 1 1  0 0 1 1
    const __m256i s23   = _mm256_hadd_epi32(src2, src3); // 2 2 3 3  2 2 3 3
    const __m256i s0123 = _mm256_hadd_epi32(s01, s23); // 0 1 2 3  0 1 2 3
    const __m128i sum0  = _mm256_castsi256_si128(s0123); // 0 1 2 3
    const __m128i sum1  = _mm256_extracti128_si256(s0123, 1); // 0 1 2 3
    return _mm_add_epi32(sum0, sum1); // 0 1 2 3
}

static INLINE __m128i hadd_four_32_avx512(const __m512i src0, const __m512i src1, const __m512i src2,
                                          const __m512i src3) {
    __m256i s[4];

    s[0] = add_hi_lo_32_avx512(src0);
    s[1] = add_hi_lo_32_avx512(src1);
    s[2] = add_hi_lo_32_avx512(src2);
    s[3] = add_hi_lo_32_avx512(src3);

    return hadd_four_32_avx2(s[0], s[1], s[2], s[3]);
}

SIMD_INLINE void compute128x_m_4d_sad_avx512_intrin(const uint8_t* src, const uint32_t src_stride,
                                                    const uint8_t* const ref_array[4], const uint32_t ref_stride,
                                                    uint32_t sad_array[4], const uint32_t height) {
    const uint8_t* ref[4];
    uint32_t       y      = height;
    __m512i        zmm[4] = {0};

    ref[0] = ref_array[0];
    ref[1] = ref_array[1];
    ref[2] = ref_array[2];
    ref[3] = ref_array[3];

    do {
        sad64_4d_avx512(src, ref, 0 * 64, zmm);
        sad64_4d_avx512(src, ref, 1 * 64, zmm);
        src += src_stride;
        ref[0] += ref_stride;
        ref[1] += ref_stride;
        ref[2] += ref_stride;
        ref[3] += ref_stride;
    } while (--y);

    const __m128i sum = hadd_four_32_avx512(zmm[0], zmm[1], zmm[2], zmm[3]);
    _mm_storeu_si128((__m128i*)sad_array, sum);
}

void svt_aom_sad128x64x4d_avx512(const uint8_t* src, int src_stride, const uint8_t* const ref_array[4], int ref_stride,
                                 uint32_t sad_array[4]) {
    compute128x_m_4d_sad_avx512_intrin(src, src_stride, ref_array, ref_stride, sad_array, 64);
}

void svt_aom_sad128x128x4d_avx512(const uint8_t* src, int src_stride, const uint8_t* const ref_array[4], int ref_stride,
                                  uint32_t sad_array[4]) {
    compute128x_m_4d_sad_avx512_intrin(src, src_stride, ref_array, ref_stride, sad_array, 128);
}

// =============================================================================

static INLINE void add16x8x2to32bit(const __m256i sads256[2], __m128i sads128[2]) {
    const __m256i zero        = _mm256_setzero_si256();
    const __m256i sad256_0_lo = _mm256_unpacklo_epi16(sads256[0], zero);
    const __m256i sad256_0_hi = _mm256_unpackhi_epi16(sads256[0], zero);
    const __m256i sad256_1_lo = _mm256_unpacklo_epi16(sads256[1], zero);
    const __m256i sad256_1_hi = _mm256_unpackhi_epi16(sads256[1], zero);
    const __m256i sad256_lo   = _mm256_add_epi32(sad256_0_lo, sad256_1_lo);
    const __m256i sad256_hi   = _mm256_add_epi32(sad256_0_hi, sad256_1_hi);
    const __m128i sad128_ll   = _mm256_castsi256_si128(sad256_lo);
    const __m128i sad128_lh   = _mm256_extracti128_si256(sad256_lo, 1);
    const __m128i sad128_hl   = _mm256_castsi256_si128(sad256_hi);
    const __m128i sad128_hh   = _mm256_extracti128_si256(sad256_hi, 1);
    sads128[0]                = _mm_add_epi32(sad128_ll, sad128_lh);
    sads128[1]                = _mm_add_epi32(sad128_hl, sad128_hh);
}

SIMD_INLINE void add16x8x3to32bit(const __m256i sads256[3], __m128i sads128[2]) {
    const __m256i zero         = _mm256_setzero_si256();
    const __m256i sad256_0_lo  = _mm256_unpacklo_epi16(sads256[0], zero);
    const __m256i sad256_0_hi  = _mm256_unpackhi_epi16(sads256[0], zero);
    const __m256i sad256_1_lo  = _mm256_unpacklo_epi16(sads256[1], zero);
    const __m256i sad256_1_hi  = _mm256_unpackhi_epi16(sads256[1], zero);
    const __m256i sad256_2_lo  = _mm256_unpacklo_epi16(sads256[2], zero);
    const __m256i sad256_2_hi  = _mm256_unpackhi_epi16(sads256[2], zero);
    const __m256i sad256_01_lo = _mm256_add_epi32(sad256_0_lo, sad256_1_lo);
    const __m256i sad256_01_hi = _mm256_add_epi32(sad256_0_hi, sad256_1_hi);
    const __m256i sad256_lo    = _mm256_add_epi32(sad256_01_lo, sad256_2_lo);
    const __m256i sad256_hi    = _mm256_add_epi32(sad256_01_hi, sad256_2_hi);
    const __m128i sad128_ll    = _mm256_castsi256_si128(sad256_lo);
    const __m128i sad128_lh    = _mm256_extracti128_si256(sad256_lo, 1);
    const __m128i sad128_hl    = _mm256_castsi256_si128(sad256_hi);
    const __m128i sad128_hh    = _mm256_extracti128_si256(sad256_hi, 1);
    sads128[0]                 = _mm_add_epi32(sad128_ll, sad128_lh);
    sads128[1]                 = _mm_add_epi32(sad128_hl, sad128_hh);
}

SIMD_INLINE void add16x8x4to32bit(const __m256i sads256[4], __m128i sads128[2]) {
    const __m256i zero         = _mm256_setzero_si256();
    const __m256i sad256_0_lo  = _mm256_unpacklo_epi16(sads256[0], zero);
    const __m256i sad256_0_hi  = _mm256_unpackhi_epi16(sads256[0], zero);
    const __m256i sad256_1_lo  = _mm256_unpacklo_epi16(sads256[1], zero);
    const __m256i sad256_1_hi  = _mm256_unpackhi_epi16(sads256[1], zero);
    const __m256i sad256_2_lo  = _mm256_unpacklo_epi16(sads256[2], zero);
    const __m256i sad256_2_hi  = _mm256_unpackhi_epi16(sads256[2], zero);
    const __m256i sad256_3_lo  = _mm256_unpacklo_epi16(sads256[3], zero);
    const __m256i sad256_3_hi  = _mm256_unpackhi_epi16(sads256[3], zero);
    const __m256i sad256_01_lo = _mm256_add_epi32(sad256_0_lo, sad256_1_lo);
    const __m256i sad256_01_hi = _mm256_add_epi32(sad256_0_hi, sad256_1_hi);
    const __m256i sad256_23_lo = _mm256_add_epi32(sad256_2_lo, sad256_3_lo);
    const __m256i sad256_23_hi = _mm256_add_epi32(sad256_2_hi, sad256_3_hi);
    const __m256i sad256_lo    = _mm256_add_epi32(sad256_01_lo, sad256_23_lo);
    const __m256i sad256_hi    = _mm256_add_epi32(sad256_01_hi, sad256_23_hi);
    const __m128i sad128_ll    = _mm256_castsi256_si128(sad256_lo);
    const __m128i sad128_lh    = _mm256_extracti128_si256(sad256_lo, 1);
    const __m128i sad128_hl    = _mm256_castsi256_si128(sad256_hi);
    const __m128i sad128_hh    = _mm256_extracti128_si256(sad256_hi, 1);
    sads128[0]                 = _mm_add_epi32(sad128_ll, sad128_lh);
    sads128[1]                 = _mm_add_epi32(sad128_hl, sad128_hh);
}

SIMD_INLINE void add16x8x6to32bit(const __m256i sads256[6], __m128i sads128[2]) {
    const __m256i zero           = _mm256_setzero_si256();
    const __m256i sad256_0_lo    = _mm256_unpacklo_epi16(sads256[0], zero);
    const __m256i sad256_0_hi    = _mm256_unpackhi_epi16(sads256[0], zero);
    const __m256i sad256_1_lo    = _mm256_unpacklo_epi16(sads256[1], zero);
    const __m256i sad256_1_hi    = _mm256_unpackhi_epi16(sads256[1], zero);
    const __m256i sad256_2_lo    = _mm256_unpacklo_epi16(sads256[2], zero);
    const __m256i sad256_2_hi    = _mm256_unpackhi_epi16(sads256[2], zero);
    const __m256i sad256_3_lo    = _mm256_unpacklo_epi16(sads256[3], zero);
    const __m256i sad256_3_hi    = _mm256_unpackhi_epi16(sads256[3], zero);
    const __m256i sad256_4_lo    = _mm256_unpacklo_epi16(sads256[4], zero);
    const __m256i sad256_4_hi    = _mm256_unpackhi_epi16(sads256[4], zero);
    const __m256i sad256_5_lo    = _mm256_unpacklo_epi16(sads256[5], zero);
    const __m256i sad256_5_hi    = _mm256_unpackhi_epi16(sads256[5], zero);
    const __m256i sad256_01_lo   = _mm256_add_epi32(sad256_0_lo, sad256_1_lo);
    const __m256i sad256_01_hi   = _mm256_add_epi32(sad256_0_hi, sad256_1_hi);
    const __m256i sad256_23_lo   = _mm256_add_epi32(sad256_2_lo, sad256_3_lo);
    const __m256i sad256_23_hi   = _mm256_add_epi32(sad256_2_hi, sad256_3_hi);
    const __m256i sad256_45_lo   = _mm256_add_epi32(sad256_4_lo, sad256_5_lo);
    const __m256i sad256_45_hi   = _mm256_add_epi32(sad256_4_hi, sad256_5_hi);
    const __m256i sad256_0123_lo = _mm256_add_epi32(sad256_01_lo, sad256_23_lo);
    const __m256i sad256_0123_hi = _mm256_add_epi32(sad256_01_hi, sad256_23_hi);
    const __m256i sad256_lo      = _mm256_add_epi32(sad256_0123_lo, sad256_45_lo);
    const __m256i sad256_hi      = _mm256_add_epi32(sad256_0123_hi, sad256_45_hi);
    const __m128i sad128_ll      = _mm256_castsi256_si128(sad256_lo);
    const __m128i sad128_lh      = _mm256_extracti128_si256(sad256_lo, 1);
    const __m128i sad128_hl      = _mm256_castsi256_si128(sad256_hi);
    const __m128i sad128_hh      = _mm256_extracti128_si256(sad256_hi, 1);
    sads128[0]                   = _mm_add_epi32(sad128_ll, sad128_lh);
    sads128[1]                   = _mm_add_epi32(sad128_hl, sad128_hh);
}

SIMD_INLINE void add16x8x8to32bit(const __m256i sads256[8], __m128i sads128[2]) {
    const __m256i zero           = _mm256_setzero_si256();
    const __m256i sad256_0_lo    = _mm256_unpacklo_epi16(sads256[0], zero);
    const __m256i sad256_0_hi    = _mm256_unpackhi_epi16(sads256[0], zero);
    const __m256i sad256_1_lo    = _mm256_unpacklo_epi16(sads256[1], zero);
    const __m256i sad256_1_hi    = _mm256_unpackhi_epi16(sads256[1], zero);
    const __m256i sad256_2_lo    = _mm256_unpacklo_epi16(sads256[2], zero);
    const __m256i sad256_2_hi    = _mm256_unpackhi_epi16(sads256[2], zero);
    const __m256i sad256_3_lo    = _mm256_unpacklo_epi16(sads256[3], zero);
    const __m256i sad256_3_hi    = _mm256_unpackhi_epi16(sads256[3], zero);
    const __m256i sad256_4_lo    = _mm256_unpacklo_epi16(sads256[4], zero);
    const __m256i sad256_4_hi    = _mm256_unpackhi_epi16(sads256[4], zero);
    const __m256i sad256_5_lo    = _mm256_unpacklo_epi16(sads256[5], zero);
    const __m256i sad256_5_hi    = _mm256_unpackhi_epi16(sads256[5], zero);
    const __m256i sad256_6_lo    = _mm256_unpacklo_epi16(sads256[6], zero);
    const __m256i sad256_6_hi    = _mm256_unpackhi_epi16(sads256[6], zero);
    const __m256i sad256_7_lo    = _mm256_unpacklo_epi16(sads256[7], zero);
    const __m256i sad256_7_hi    = _mm256_unpackhi_epi16(sads256[7], zero);
    const __m256i sad256_01_lo   = _mm256_add_epi32(sad256_0_lo, sad256_1_lo);
    const __m256i sad256_01_hi   = _mm256_add_epi32(sad256_0_hi, sad256_1_hi);
    const __m256i sad256_23_lo   = _mm256_add_epi32(sad256_2_lo, sad256_3_lo);
    const __m256i sad256_23_hi   = _mm256_add_epi32(sad256_2_hi, sad256_3_hi);
    const __m256i sad256_45_lo   = _mm256_add_epi32(sad256_4_lo, sad256_5_lo);
    const __m256i sad256_45_hi   = _mm256_add_epi32(sad256_4_hi, sad256_5_hi);
    const __m256i sad256_67_lo   = _mm256_add_epi32(sad256_6_lo, sad256_7_lo);
    const __m256i sad256_67_hi   = _mm256_add_epi32(sad256_6_hi, sad256_7_hi);
    const __m256i sad256_0123_lo = _mm256_add_epi32(sad256_01_lo, sad256_23_lo);
    const __m256i sad256_0123_hi = _mm256_add_epi32(sad256_01_hi, sad256_23_hi);
    const __m256i sad256_4567_lo = _mm256_add_epi32(sad256_45_lo, sad256_67_lo);
    const __m256i sad256_4567_hi = _mm256_add_epi32(sad256_45_hi, sad256_67_hi);
    const __m256i sad256_lo      = _mm256_add_epi32(sad256_0123_lo, sad256_4567_lo);
    const __m256i sad256_hi      = _mm256_add_epi32(sad256_0123_hi, sad256_4567_hi);
    const __m128i sad128_ll      = _mm256_castsi256_si128(sad256_lo);
    const __m128i sad128_lh      = _mm256_extracti128_si256(sad256_lo, 1);
    const __m128i sad128_hl      = _mm256_castsi256_si128(sad256_hi);
    const __m128i sad128_hh      = _mm256_extracti128_si256(sad256_hi, 1);
    sads128[0]                   = _mm_add_epi32(sad128_ll, sad128_lh);
    sads128[1]                   = _mm_add_epi32(sad128_hl, sad128_hh);
}

SIMD_INLINE void add16x16x2to32bit(const __m512i sads512[2], __m256i sads256[2]) {
    const __m512i zero = _mm512_setzero_si512();

    const __m512i sad512_0_lo = _mm512_unpacklo_epi16(sads512[0], zero);
    const __m512i sad512_0_hi = _mm512_unpackhi_epi16(sads512[0], zero);
    const __m512i sad512_1_lo = _mm512_unpacklo_epi16(sads512[1], zero);
    const __m512i sad512_1_hi = _mm512_unpackhi_epi16(sads512[1], zero);

    // 0 1 2 3  8 9 A b   0 1 2 3  8 9 A b
    // 4 5 6 7  C D E F   4 5 6 7  C D E F
    const __m512i sad512_lo = _mm512_add_epi32(sad512_0_lo, sad512_1_lo);
    const __m512i sad512_hi = _mm512_add_epi32(sad512_0_hi, sad512_1_hi);

    const __m256i sad256_ll = _mm512_castsi512_si256(sad512_lo);
    const __m256i sad256_lh = _mm512_extracti64x4_epi64(sad512_lo, 1);
    const __m256i sad256_hl = _mm512_castsi512_si256(sad512_hi);
    const __m256i sad256_hh = _mm512_extracti64x4_epi64(sad512_hi, 1);

    // 0 1 2 3  8 9 A b
    // 4 5 6 7  C D E F
    const __m256i sad256_0 = _mm256_add_epi32(sad256_ll, sad256_lh);
    const __m256i sad256_1 = _mm256_add_epi32(sad256_hl, sad256_hh);

    // 0 1 2 3  4 5 6 7
    // 8 9 A b  C D E F
    sads256[0] = yy_unpacklo_epi128(sad256_0, sad256_1);
    sads256[1] = yy_unpackhi_epi128(sad256_0, sad256_1);
}

SIMD_INLINE void add16x16x3to32bit(const __m512i sads512[3], __m256i sads256[2]) {
    const __m512i zero = _mm512_setzero_si512();

    const __m512i sad512_0_lo = _mm512_unpacklo_epi16(sads512[0], zero);
    const __m512i sad512_0_hi = _mm512_unpackhi_epi16(sads512[0], zero);
    const __m512i sad512_1_lo = _mm512_unpacklo_epi16(sads512[1], zero);
    const __m512i sad512_1_hi = _mm512_unpackhi_epi16(sads512[1], zero);
    const __m512i sad512_2_lo = _mm512_unpacklo_epi16(sads512[2], zero);
    const __m512i sad512_2_hi = _mm512_unpackhi_epi16(sads512[2], zero);

    const __m512i sad512_01_lo = _mm512_add_epi32(sad512_0_lo, sad512_1_lo);
    const __m512i sad512_01_hi = _mm512_add_epi32(sad512_0_hi, sad512_1_hi);

    // 0 1 2 3  8 9 A b   0 1 2 3  8 9 A b
    // 4 5 6 7  C D E F   4 5 6 7  C D E F
    const __m512i sad512_lo = _mm512_add_epi32(sad512_01_lo, sad512_2_lo);
    const __m512i sad512_hi = _mm512_add_epi32(sad512_01_hi, sad512_2_hi);

    const __m256i sad256_ll = _mm512_castsi512_si256(sad512_lo);
    const __m256i sad256_lh = _mm512_extracti64x4_epi64(sad512_lo, 1);
    const __m256i sad256_hl = _mm512_castsi512_si256(sad512_hi);
    const __m256i sad256_hh = _mm512_extracti64x4_epi64(sad512_hi, 1);

    // 0 1 2 3  8 9 A b
    // 4 5 6 7  C D E F
    const __m256i sad256_0 = _mm256_add_epi32(sad256_ll, sad256_lh);
    const __m256i sad256_1 = _mm256_add_epi32(sad256_hl, sad256_hh);

    // 0 1 2 3  4 5 6 7
    // 8 9 A b  C D E F
    sads256[0] = yy_unpacklo_epi128(sad256_0, sad256_1);
    sads256[1] = yy_unpackhi_epi128(sad256_0, sad256_1);
}

SIMD_INLINE void add16x16x4to32bit(const __m512i sads512[4], __m256i sads256[2]) {
    // Don't call two add16x16x2to32bit(), which is slower.
    const __m512i zero = _mm512_setzero_si512();

    const __m512i sad512_0_lo = _mm512_unpacklo_epi16(sads512[0], zero);
    const __m512i sad512_0_hi = _mm512_unpackhi_epi16(sads512[0], zero);
    const __m512i sad512_1_lo = _mm512_unpacklo_epi16(sads512[1], zero);
    const __m512i sad512_1_hi = _mm512_unpackhi_epi16(sads512[1], zero);
    const __m512i sad512_2_lo = _mm512_unpacklo_epi16(sads512[2], zero);
    const __m512i sad512_2_hi = _mm512_unpackhi_epi16(sads512[2], zero);
    const __m512i sad512_3_lo = _mm512_unpacklo_epi16(sads512[3], zero);
    const __m512i sad512_3_hi = _mm512_unpackhi_epi16(sads512[3], zero);

    const __m512i sad512_01_lo = _mm512_add_epi32(sad512_0_lo, sad512_1_lo);
    const __m512i sad512_01_hi = _mm512_add_epi32(sad512_0_hi, sad512_1_hi);
    const __m512i sad512_23_lo = _mm512_add_epi32(sad512_2_lo, sad512_3_lo);
    const __m512i sad512_23_hi = _mm512_add_epi32(sad512_2_hi, sad512_3_hi);

    // 0 1 2 3  8 9 A b   0 1 2 3  8 9 A b
    // 4 5 6 7  C D E F   4 5 6 7  C D E F
    const __m512i sad512_lo = _mm512_add_epi32(sad512_01_lo, sad512_23_lo);
    const __m512i sad512_hi = _mm512_add_epi32(sad512_01_hi, sad512_23_hi);

    const __m256i sad256_ll = _mm512_castsi512_si256(sad512_lo);
    const __m256i sad256_lh = _mm512_extracti64x4_epi64(sad512_lo, 1);
    const __m256i sad256_hl = _mm512_castsi512_si256(sad512_hi);
    const __m256i sad256_hh = _mm512_extracti64x4_epi64(sad512_hi, 1);

    // 0 1 2 3  8 9 A b
    // 4 5 6 7  C D E F
    const __m256i sad256_0 = _mm256_add_epi32(sad256_ll, sad256_lh);
    const __m256i sad256_1 = _mm256_add_epi32(sad256_hl, sad256_hh);

    // 0 1 2 3  4 5 6 7
    // 8 9 A b  C D E F
    sads256[0] = yy_unpacklo_epi128(sad256_0, sad256_1);
    sads256[1] = yy_unpackhi_epi128(sad256_0, sad256_1);
}

SIMD_INLINE void add16x16x6to32bit(const __m512i sads512[6], __m256i sads256[2]) {
    const __m512i zero = _mm512_setzero_si512();

    const __m512i sad512_0_lo = _mm512_unpacklo_epi16(sads512[0], zero);
    const __m512i sad512_0_hi = _mm512_unpackhi_epi16(sads512[0], zero);
    const __m512i sad512_1_lo = _mm512_unpacklo_epi16(sads512[1], zero);
    const __m512i sad512_1_hi = _mm512_unpackhi_epi16(sads512[1], zero);
    const __m512i sad512_2_lo = _mm512_unpacklo_epi16(sads512[2], zero);
    const __m512i sad512_2_hi = _mm512_unpackhi_epi16(sads512[2], zero);
    const __m512i sad512_3_lo = _mm512_unpacklo_epi16(sads512[3], zero);
    const __m512i sad512_3_hi = _mm512_unpackhi_epi16(sads512[3], zero);
    const __m512i sad512_4_lo = _mm512_unpacklo_epi16(sads512[4], zero);
    const __m512i sad512_4_hi = _mm512_unpackhi_epi16(sads512[4], zero);
    const __m512i sad512_5_lo = _mm512_unpacklo_epi16(sads512[5], zero);
    const __m512i sad512_5_hi = _mm512_unpackhi_epi16(sads512[5], zero);

    const __m512i sad512_01_lo   = _mm512_add_epi32(sad512_0_lo, sad512_1_lo);
    const __m512i sad512_01_hi   = _mm512_add_epi32(sad512_0_hi, sad512_1_hi);
    const __m512i sad512_23_lo   = _mm512_add_epi32(sad512_2_lo, sad512_3_lo);
    const __m512i sad512_23_hi   = _mm512_add_epi32(sad512_2_hi, sad512_3_hi);
    const __m512i sad512_45_lo   = _mm512_add_epi32(sad512_4_lo, sad512_5_lo);
    const __m512i sad512_45_hi   = _mm512_add_epi32(sad512_4_hi, sad512_5_hi);
    const __m512i sad512_0123_lo = _mm512_add_epi32(sad512_01_lo, sad512_23_lo);
    const __m512i sad512_0123_hi = _mm512_add_epi32(sad512_01_hi, sad512_23_hi);

    // 0 1 2 3  8 9 A b   0 1 2 3  8 9 A b
    // 4 5 6 7  C D E F   4 5 6 7  C D E F
    const __m512i sad512_lo = _mm512_add_epi32(sad512_0123_lo, sad512_45_lo);
    const __m512i sad512_hi = _mm512_add_epi32(sad512_0123_hi, sad512_45_hi);

    const __m256i sad256_ll = _mm512_castsi512_si256(sad512_lo);
    const __m256i sad256_lh = _mm512_extracti64x4_epi64(sad512_lo, 1);
    const __m256i sad256_hl = _mm512_castsi512_si256(sad512_hi);
    const __m256i sad256_hh = _mm512_extracti64x4_epi64(sad512_hi, 1);

    // 0 1 2 3  8 9 A b
    // 4 5 6 7  C D E F
    const __m256i sad256_0 = _mm256_add_epi32(sad256_ll, sad256_lh);
    const __m256i sad256_1 = _mm256_add_epi32(sad256_hl, sad256_hh);

    // 0 1 2 3  4 5 6 7
    // 8 9 A b  C D E F
    sads256[0] = yy_unpacklo_epi128(sad256_0, sad256_1);
    sads256[1] = yy_unpackhi_epi128(sad256_0, sad256_1);
}

SIMD_INLINE void add16x16x8to32bit(const __m512i sads512[8], __m256i sads256[2]) {
    // Don't call two add16x16x4to32bit(), which is slower.
    const __m512i zero = _mm512_setzero_si512();

    const __m512i sad512_0_lo = _mm512_unpacklo_epi16(sads512[0], zero);
    const __m512i sad512_0_hi = _mm512_unpackhi_epi16(sads512[0], zero);
    const __m512i sad512_1_lo = _mm512_unpacklo_epi16(sads512[1], zero);
    const __m512i sad512_1_hi = _mm512_unpackhi_epi16(sads512[1], zero);
    const __m512i sad512_2_lo = _mm512_unpacklo_epi16(sads512[2], zero);
    const __m512i sad512_2_hi = _mm512_unpackhi_epi16(sads512[2], zero);
    const __m512i sad512_3_lo = _mm512_unpacklo_epi16(sads512[3], zero);
    const __m512i sad512_3_hi = _mm512_unpackhi_epi16(sads512[3], zero);
    const __m512i sad512_4_lo = _mm512_unpacklo_epi16(sads512[4], zero);
    const __m512i sad512_4_hi = _mm512_unpackhi_epi16(sads512[4], zero);
    const __m512i sad512_5_lo = _mm512_unpacklo_epi16(sads512[5], zero);
    const __m512i sad512_5_hi = _mm512_unpackhi_epi16(sads512[5], zero);
    const __m512i sad512_6_lo = _mm512_unpacklo_epi16(sads512[6], zero);
    const __m512i sad512_6_hi = _mm512_unpackhi_epi16(sads512[6], zero);
    const __m512i sad512_7_lo = _mm512_unpacklo_epi16(sads512[7], zero);
    const __m512i sad512_7_hi = _mm512_unpackhi_epi16(sads512[7], zero);

    const __m512i sad512_01_lo   = _mm512_add_epi32(sad512_0_lo, sad512_1_lo);
    const __m512i sad512_01_hi   = _mm512_add_epi32(sad512_0_hi, sad512_1_hi);
    const __m512i sad512_23_lo   = _mm512_add_epi32(sad512_2_lo, sad512_3_lo);
    const __m512i sad512_23_hi   = _mm512_add_epi32(sad512_2_hi, sad512_3_hi);
    const __m512i sad512_45_lo   = _mm512_add_epi32(sad512_4_lo, sad512_5_lo);
    const __m512i sad512_45_hi   = _mm512_add_epi32(sad512_4_hi, sad512_5_hi);
    const __m512i sad512_67_lo   = _mm512_add_epi32(sad512_6_lo, sad512_7_lo);
    const __m512i sad512_67_hi   = _mm512_add_epi32(sad512_6_hi, sad512_7_hi);
    const __m512i sad512_0123_lo = _mm512_add_epi32(sad512_01_lo, sad512_23_lo);
    const __m512i sad512_0123_hi = _mm512_add_epi32(sad512_01_hi, sad512_23_hi);
    const __m512i sad512_4567_lo = _mm512_add_epi32(sad512_45_lo, sad512_67_lo);
    const __m512i sad512_4567_hi = _mm512_add_epi32(sad512_45_hi, sad512_67_hi);

    // 0 1 2 3  8 9 A b   0 1 2 3  8 9 A b
    // 4 5 6 7  C D E F   4 5 6 7  C D E F
    const __m512i sad512_lo = _mm512_add_epi32(sad512_0123_lo, sad512_4567_lo);
    const __m512i sad512_hi = _mm512_add_epi32(sad512_0123_hi, sad512_4567_hi);

    const __m256i sad256_ll = _mm512_castsi512_si256(sad512_lo);
    const __m256i sad256_lh = _mm512_extracti64x4_epi64(sad512_lo, 1);
    const __m256i sad256_hl = _mm512_castsi512_si256(sad512_hi);
    const __m256i sad256_hh = _mm512_extracti64x4_epi64(sad512_hi, 1);

    // 0 1 2 3  8 9 A b
    // 4 5 6 7  C D E F
    const __m256i sad256_0 = _mm256_add_epi32(sad256_ll, sad256_lh);
    const __m256i sad256_1 = _mm256_add_epi32(sad256_hl, sad256_hh);

    // 0 1 2 3  4 5 6 7
    // 8 9 A b  C D E F
    sads256[0] = yy_unpacklo_epi128(sad256_0, sad256_1);
    sads256[1] = yy_unpackhi_epi128(sad256_0, sad256_1);
}

static INLINE uint32_t saturate_add(const __m128i sum0, const __m128i sum1, __m128i* const minpos) {
    uint32_t min_val;
    __m128i  min0, min1;

    const __m128i minpos0 = _mm_minpos_epu16(sum0);
    const __m128i minpos1 = _mm_minpos_epu16(sum1);
    min0                  = _mm_unpacklo_epi16(minpos0, minpos0);
    min0                  = _mm_unpacklo_epi32(min0, min0);
    min0                  = _mm_unpacklo_epi64(min0, min0);
    min1                  = _mm_unpacklo_epi16(minpos1, minpos1);
    min1                  = _mm_unpacklo_epi32(min1, min1);
    min1                  = _mm_unpacklo_epi64(min1, min1);
    const __m128i t0      = _mm_sub_epi16(sum0, min0);
    const __m128i t1      = _mm_sub_epi16(sum1, min1);
    const __m128i sum     = _mm_adds_epu16(t0, t1);
    *minpos               = _mm_minpos_epu16(sum);
    min_val               = _mm_extract_epi16(*minpos, 0);
    min_val += _mm_extract_epi16(minpos0, 0);
    min_val += _mm_extract_epi16(minpos1, 0);

    return min_val;
}

static INLINE void sad_loop_kernel_4_avx2(const uint8_t* const src, const uint32_t src_stride, const uint8_t* const ref,
                                          const uint32_t ref_stride, __m256i* const sum) {
    const __m256i ss0 = _mm256_insertf128_si256(_mm256_castsi128_si256(_mm_cvtsi32_si128(*(uint32_t*)src)),
                                                _mm_cvtsi32_si128(*(uint32_t*)(src + src_stride)),
                                                1);
    const __m256i rr0 = _mm256_insertf128_si256(
        _mm256_castsi128_si256(_mm_lddqu_si128((__m128i*)ref)), _mm_lddqu_si128((__m128i*)(ref + ref_stride)), 1);
    *sum = _mm256_adds_epu16(*sum, _mm256_mpsadbw_epu8(rr0, ss0, 0));
}

/*******************************************************************************
* Function helper adds to "sum" vector SAD's for block width of 4, uses AVX2 instructions
* Requirement: width = 4
* Requirement: height = 1
* Compute one line
*******************************************************************************/
static INLINE void sad_loop_kernel_4_oneline_avx2(const uint8_t* const src, const uint8_t* const ref,
                                                  __m256i* const sum) {
    const __m256i ss0 = _mm256_insertf128_si256(
        _mm256_castsi128_si256(_mm_cvtsi32_si128(*(uint32_t*)src)), _mm_setzero_si128(), 1);
    const __m256i rr0 = _mm256_insertf128_si256(
        _mm256_castsi128_si256(_mm_lddqu_si128((__m128i*)ref)), _mm_setzero_si128(), 1);
    *sum = _mm256_adds_epu16(*sum, _mm256_mpsadbw_epu8(rr0, ss0, 0));
}

static INLINE void sad_loop_kernel_4_sse4_1(const uint8_t* const src, const uint32_t src_stride,
                                            const uint8_t* const ref, const uint32_t ref_stride, __m128i* const sum) {
    const __m128i s0 = _mm_cvtsi32_si128(*(uint32_t*)src);
    const __m128i s1 = _mm_cvtsi32_si128(*(uint32_t*)(src + src_stride));
    const __m128i r0 = _mm_lddqu_si128((__m128i*)ref);
    const __m128i r1 = _mm_lddqu_si128((__m128i*)(ref + ref_stride));
    *sum             = _mm_adds_epu16(*sum, _mm_mpsadbw_epu8(r0, s0, 0));
    *sum             = _mm_adds_epu16(*sum, _mm_mpsadbw_epu8(r1, s1, 0));
}

/*******************************************************************************
* Function helper adds to "sum" vector SAD's for block width of 4, uses sse4_1 instructions
* Requirement: width = 4
* Requirement: height = 1
* Compute one line
*******************************************************************************/
static INLINE void sad_loop_kernel_4_oneline_sse4_1(const uint8_t* const src, const uint8_t* const ref,
                                                    __m128i* const sum) {
    const __m128i s0 = _mm_cvtsi32_si128(*(uint32_t*)src);
    const __m128i r0 = _mm_lddqu_si128((__m128i*)ref);
    *sum             = _mm_adds_epu16(*sum, _mm_mpsadbw_epu8(r0, s0, 0));
}

static INLINE void sad_loop_kernel_8_avx2(const uint8_t* const src, const uint32_t src_stride, const uint8_t* const ref,
                                          const uint32_t ref_stride, __m256i* const sum) {
    const __m256i ss0 = _mm256_insertf128_si256(
        _mm256_castsi128_si256(_mm_loadl_epi64((__m128i*)src)), _mm_loadl_epi64((__m128i*)(src + 1 * src_stride)), 1);
    const __m256i rr0 = _mm256_insertf128_si256(
        _mm256_castsi128_si256(_mm_lddqu_si128((__m128i*)ref)), _mm_lddqu_si128((__m128i*)(ref + 1 * ref_stride)), 1);
    *sum = _mm256_adds_epu16(*sum, _mm256_mpsadbw_epu8(rr0, ss0, (0 << 3))); // 000 000
    *sum = _mm256_adds_epu16(*sum, _mm256_mpsadbw_epu8(rr0, ss0, (5 << 3) | 5)); // 101 101
}

/*******************************************************************************
* Function helper adds to "sum" vector SAD's for block width of 8, uses AVX2 instructions
* Requirement: width = 8
* Requirement: height = 1
* Compute one line
*******************************************************************************/
static INLINE void sad_loop_kernel_8_oneline_avx2(const uint8_t* const src, const uint8_t* const ref,
                                                  __m256i* const sum) {
    const __m256i ss0 = _mm256_insertf128_si256(
        _mm256_castsi128_si256(_mm_loadl_epi64((__m128i*)src)), _mm_setzero_si128(), 1);
    const __m256i rr0 = _mm256_insertf128_si256(
        _mm256_castsi128_si256(_mm_lddqu_si128((__m128i*)ref)), _mm_setzero_si128(), 1);
    *sum = _mm256_adds_epu16(*sum, _mm256_mpsadbw_epu8(rr0, ss0, (0 << 3))); // 000 000
    *sum = _mm256_adds_epu16(*sum, _mm256_mpsadbw_epu8(rr0, ss0, (5 << 3) | 5)); // 101 101
}

/*******************************************************************************
* Function helper adds to "sums" vectors SAD's for block width of 12, uses AVX512 instructions
* Requirement: width = 12
* Requirement: height = 2
*******************************************************************************/
SIMD_INLINE void sad_loop_kernel_12_avx512(const uint8_t* const src, const uint32_t src_stride,
                                           const uint8_t* const ref, const uint32_t ref_stride, __m512i* const sum) {
    const __m128i s0  = _mm_lddqu_si128((__m128i*)src);
    const __m128i s1  = _mm_lddqu_si128((__m128i*)(src + src_stride));
    const __m256i s01 = _mm256_insertf128_si256(_mm256_castsi128_si256(s0), s1, 1);
    const __m512i s   = _mm512_castsi256_si512(s01);
    const __m512i ss0 = _mm512_permutexvar_epi32(_mm512_setr_epi32(0, 0, 0, 0, 0, 0, 0, 0, 4, 4, 4, 4, 4, 4, 4, 4), s);
    const __m512i ss1 = _mm512_permutexvar_epi32(_mm512_setr_epi32(1, 1, 1, 1, 1, 1, 1, 1, 5, 5, 5, 5, 5, 5, 5, 5), s);
    const __m512i ss2 = _mm512_permutexvar_epi32(_mm512_setr_epi32(2, 2, 2, 2, 2, 2, 2, 2, 6, 6, 6, 6, 6, 6, 6, 6), s);

    const __m256i r0  = _mm256_loadu_si256((__m256i*)ref);
    const __m256i r1  = _mm256_loadu_si256((__m256i*)(ref + ref_stride));
    const __m512i r   = _mm512_inserti64x4(_mm512_castsi256_si512(r0), r1, 1);
    const __m512i rr0 = _mm512_permutexvar_epi64(_mm512_setr_epi64(0, 1, 1, 2, 4, 5, 5, 6), r);
    const __m512i rr1 = _mm512_permutexvar_epi64(_mm512_setr_epi64(1, 2, 2, 3, 5, 6, 6, 7), r);

    *sum = _mm512_adds_epu16(*sum, _mm512_dbsad_epu8(ss0, rr0, 0x94));
    *sum = _mm512_adds_epu16(*sum, _mm512_dbsad_epu8(ss1, rr0, 0xE9));
    *sum = _mm512_adds_epu16(*sum, _mm512_dbsad_epu8(ss2, rr1, 0x94));
}

/*******************************************************************************
* Function helper adds to "sums" vectors SAD's for block width of 12, uses AVX512 instructions
* Requirement: width = 12
* Requirement: height = 1
* Compute one line
*******************************************************************************/
SIMD_INLINE void sad_loop_kernel_12_oneline_avx512(const uint8_t* const src, const uint8_t* const ref,
                                                   __m512i* const sum) {
    const __m128i s0  = _mm_lddqu_si128((__m128i*)src);
    const __m256i s01 = _mm256_insertf128_si256(_mm256_castsi128_si256(s0), _mm_setzero_si128(), 1);
    const __m512i s   = _mm512_castsi256_si512(s01);
    const __m512i ss0 = _mm512_permutexvar_epi32(_mm512_setr_epi32(0, 0, 0, 0, 0, 0, 0, 0, 4, 4, 4, 4, 4, 4, 4, 4), s);
    const __m512i ss1 = _mm512_permutexvar_epi32(_mm512_setr_epi32(1, 1, 1, 1, 1, 1, 1, 1, 5, 5, 5, 5, 5, 5, 5, 5), s);
    const __m512i ss2 = _mm512_permutexvar_epi32(_mm512_setr_epi32(2, 2, 2, 2, 2, 2, 2, 2, 6, 6, 6, 6, 6, 6, 6, 6), s);

    const __m256i r0  = _mm256_loadu_si256((__m256i*)ref);
    const __m512i r   = _mm512_inserti64x4(_mm512_castsi256_si512(r0), _mm256_setzero_si256(), 1);
    const __m512i rr0 = _mm512_permutexvar_epi64(_mm512_setr_epi64(0, 1, 1, 2, 4, 5, 5, 6), r);
    const __m512i rr1 = _mm512_permutexvar_epi64(_mm512_setr_epi64(1, 2, 2, 3, 5, 6, 6, 7), r);

    *sum = _mm512_adds_epu16(*sum, _mm512_dbsad_epu8(ss0, rr0, 0x94));
    *sum = _mm512_adds_epu16(*sum, _mm512_dbsad_epu8(ss1, rr0, 0xE9));
    *sum = _mm512_adds_epu16(*sum, _mm512_dbsad_epu8(ss2, rr1, 0x94));
}

SIMD_INLINE void sad_loop_kernel_16_avx512(const uint8_t* const src, const uint32_t src_stride,
                                           const uint8_t* const ref, const uint32_t ref_stride, __m512i* const sum) {
    const __m128i s0  = _mm_lddqu_si128((__m128i*)src);
    const __m128i s1  = _mm_lddqu_si128((__m128i*)(src + src_stride));
    const __m256i s01 = _mm256_insertf128_si256(_mm256_castsi128_si256(s0), s1, 1);
    const __m512i s   = _mm512_castsi256_si512(s01);
    const __m512i ss0 = _mm512_permutexvar_epi32(_mm512_setr_epi32(0, 0, 0, 0, 0, 0, 0, 0, 4, 4, 4, 4, 4, 4, 4, 4), s);
    const __m512i ss1 = _mm512_permutexvar_epi32(_mm512_setr_epi32(1, 1, 1, 1, 1, 1, 1, 1, 5, 5, 5, 5, 5, 5, 5, 5), s);
    const __m512i ss2 = _mm512_permutexvar_epi32(_mm512_setr_epi32(2, 2, 2, 2, 2, 2, 2, 2, 6, 6, 6, 6, 6, 6, 6, 6), s);
    const __m512i ss3 = _mm512_permutexvar_epi32(_mm512_setr_epi32(3, 3, 3, 3, 3, 3, 3, 3, 7, 7, 7, 7, 7, 7, 7, 7), s);

    const __m256i r0  = _mm256_loadu_si256((__m256i*)ref);
    const __m256i r1  = _mm256_loadu_si256((__m256i*)(ref + ref_stride));
    const __m512i r   = _mm512_inserti64x4(_mm512_castsi256_si512(r0), r1, 1);
    const __m512i rr0 = _mm512_permutexvar_epi64(_mm512_setr_epi64(0, 1, 1, 2, 4, 5, 5, 6), r);
    const __m512i rr1 = _mm512_permutexvar_epi64(_mm512_setr_epi64(1, 2, 2, 3, 5, 6, 6, 7), r);

    *sum = _mm512_adds_epu16(*sum, _mm512_dbsad_epu8(ss0, rr0, 0x94));
    *sum = _mm512_adds_epu16(*sum, _mm512_dbsad_epu8(ss1, rr0, 0xE9));
    *sum = _mm512_adds_epu16(*sum, _mm512_dbsad_epu8(ss2, rr1, 0x94));
    *sum = _mm512_adds_epu16(*sum, _mm512_dbsad_epu8(ss3, rr1, 0xE9));
}

/*******************************************************************************
* Function helper adds to "sums" vectors SAD's for block width of 16, uses AVX512 instructions
* Requirement: width = 16
* Requirement: height = 1
* Compute one line
*******************************************************************************/
SIMD_INLINE void sad_loop_kernel_16_oneline_avx512(const uint8_t* const src, const uint8_t* const ref,
                                                   __m512i* const sum) {
    const __m128i s0  = _mm_lddqu_si128((__m128i*)src);
    const __m256i s01 = _mm256_insertf128_si256(_mm256_castsi128_si256(s0), _mm_setzero_si128(), 1);
    const __m512i s   = _mm512_castsi256_si512(s01);
    const __m512i ss0 = _mm512_permutexvar_epi32(_mm512_setr_epi32(0, 0, 0, 0, 0, 0, 0, 0, 4, 4, 4, 4, 4, 4, 4, 4), s);
    const __m512i ss1 = _mm512_permutexvar_epi32(_mm512_setr_epi32(1, 1, 1, 1, 1, 1, 1, 1, 5, 5, 5, 5, 5, 5, 5, 5), s);
    const __m512i ss2 = _mm512_permutexvar_epi32(_mm512_setr_epi32(2, 2, 2, 2, 2, 2, 2, 2, 6, 6, 6, 6, 6, 6, 6, 6), s);
    const __m512i ss3 = _mm512_permutexvar_epi32(_mm512_setr_epi32(3, 3, 3, 3, 3, 3, 3, 3, 7, 7, 7, 7, 7, 7, 7, 7), s);

    const __m256i r0  = _mm256_loadu_si256((__m256i*)ref);
    const __m512i r   = _mm512_inserti64x4(_mm512_castsi256_si512(r0), _mm256_setzero_si256(), 1);
    const __m512i rr0 = _mm512_permutexvar_epi64(_mm512_setr_epi64(0, 1, 1, 2, 4, 5, 5, 6), r);
    const __m512i rr1 = _mm512_permutexvar_epi64(_mm512_setr_epi64(1, 2, 2, 3, 5, 6, 6, 7), r);

    *sum = _mm512_adds_epu16(*sum, _mm512_dbsad_epu8(ss0, rr0, 0x94));
    *sum = _mm512_adds_epu16(*sum, _mm512_dbsad_epu8(ss1, rr0, 0xE9));
    *sum = _mm512_adds_epu16(*sum, _mm512_dbsad_epu8(ss2, rr1, 0x94));
    *sum = _mm512_adds_epu16(*sum, _mm512_dbsad_epu8(ss3, rr1, 0xE9));
}

/*******************************************************************************
* Function helper adds to two elements "sums" vectors SAD's for block width of 12, uses AVX512 instructions
* Requirement: width = 12
* Requirement: height = 2
*******************************************************************************/
SIMD_INLINE void sad_loop_kernel_12_2sum_avx512(const uint8_t* const src, const uint32_t src_stride,
                                                const uint8_t* const ref, const uint32_t ref_stride, __m512i sum[2]) {
    const __m128i s0  = _mm_lddqu_si128((__m128i*)src);
    const __m128i s1  = _mm_lddqu_si128((__m128i*)(src + src_stride));
    const __m256i s01 = _mm256_insertf128_si256(_mm256_castsi128_si256(s0), s1, 1);
    const __m512i s   = _mm512_castsi256_si512(s01);
    const __m512i ss0 = _mm512_permutexvar_epi32(_mm512_setr_epi32(0, 0, 0, 0, 0, 0, 0, 0, 4, 4, 4, 4, 4, 4, 4, 4), s);
    const __m512i ss1 = _mm512_permutexvar_epi32(_mm512_setr_epi32(1, 1, 1, 1, 1, 1, 1, 1, 5, 5, 5, 5, 5, 5, 5, 5), s);
    const __m512i ss2 = _mm512_permutexvar_epi32(_mm512_setr_epi32(2, 2, 2, 2, 2, 2, 2, 2, 6, 6, 6, 6, 6, 6, 6, 6), s);

    const __m256i r0  = _mm256_loadu_si256((__m256i*)ref);
    const __m256i r1  = _mm256_loadu_si256((__m256i*)(ref + ref_stride));
    const __m512i r   = _mm512_inserti64x4(_mm512_castsi256_si512(r0), r1, 1);
    const __m512i rr0 = _mm512_permutexvar_epi64(_mm512_setr_epi64(0, 1, 1, 2, 4, 5, 5, 6), r);
    const __m512i rr1 = _mm512_permutexvar_epi64(_mm512_setr_epi64(1, 2, 2, 3, 5, 6, 6, 7), r);

    sum[0] = _mm512_adds_epu16(sum[0], _mm512_dbsad_epu8(ss0, rr0, 0x94));
    sum[1] = _mm512_adds_epu16(sum[1], _mm512_dbsad_epu8(ss1, rr0, 0xE9));
    sum[0] = _mm512_adds_epu16(sum[0], _mm512_dbsad_epu8(ss2, rr1, 0x94));
}

/*******************************************************************************
* Function helper adds to two elements "sums" vectors SAD's for block width of 12, uses AVX512 instructions
* Requirement: width = 12
* Requirement: height = 1
* Compute one line
*******************************************************************************/
SIMD_INLINE void sad_loop_kernel_12_2sum_oneline_avx512(const uint8_t* const src, const uint8_t* const ref,
                                                        __m512i sum[2]) {
    const __m128i s0  = _mm_lddqu_si128((__m128i*)src);
    const __m256i s01 = _mm256_insertf128_si256(_mm256_castsi128_si256(s0), _mm_setzero_si128(), 1);
    const __m512i s   = _mm512_castsi256_si512(s01);
    const __m512i ss0 = _mm512_permutexvar_epi32(_mm512_setr_epi32(0, 0, 0, 0, 0, 0, 0, 0, 4, 4, 4, 4, 4, 4, 4, 4), s);
    const __m512i ss1 = _mm512_permutexvar_epi32(_mm512_setr_epi32(1, 1, 1, 1, 1, 1, 1, 1, 5, 5, 5, 5, 5, 5, 5, 5), s);
    const __m512i ss2 = _mm512_permutexvar_epi32(_mm512_setr_epi32(2, 2, 2, 2, 2, 2, 2, 2, 6, 6, 6, 6, 6, 6, 6, 6), s);

    const __m256i r0  = _mm256_loadu_si256((__m256i*)ref);
    const __m512i r   = _mm512_inserti64x4(_mm512_castsi256_si512(r0), _mm256_setzero_si256(), 1);
    const __m512i rr0 = _mm512_permutexvar_epi64(_mm512_setr_epi64(0, 1, 1, 2, 4, 5, 5, 6), r);
    const __m512i rr1 = _mm512_permutexvar_epi64(_mm512_setr_epi64(1, 2, 2, 3, 5, 6, 6, 7), r);

    sum[0] = _mm512_adds_epu16(sum[0], _mm512_dbsad_epu8(ss0, rr0, 0x94));
    sum[1] = _mm512_adds_epu16(sum[1], _mm512_dbsad_epu8(ss1, rr0, 0xE9));
    sum[0] = _mm512_adds_epu16(sum[0], _mm512_dbsad_epu8(ss2, rr1, 0x94));
}

SIMD_INLINE void sad_loop_kernel_16_2sum_avx512(const uint8_t* const src, const uint32_t src_stride,
                                                const uint8_t* const ref, const uint32_t ref_stride, __m512i sum[2]) {
    const __m128i s0  = _mm_lddqu_si128((__m128i*)src);
    const __m128i s1  = _mm_lddqu_si128((__m128i*)(src + src_stride));
    const __m256i s01 = _mm256_insertf128_si256(_mm256_castsi128_si256(s0), s1, 1);
    const __m512i s   = _mm512_castsi256_si512(s01);
    const __m512i ss0 = _mm512_permutexvar_epi32(_mm512_setr_epi32(0, 0, 0, 0, 0, 0, 0, 0, 4, 4, 4, 4, 4, 4, 4, 4), s);
    const __m512i ss1 = _mm512_permutexvar_epi32(_mm512_setr_epi32(1, 1, 1, 1, 1, 1, 1, 1, 5, 5, 5, 5, 5, 5, 5, 5), s);
    const __m512i ss2 = _mm512_permutexvar_epi32(_mm512_setr_epi32(2, 2, 2, 2, 2, 2, 2, 2, 6, 6, 6, 6, 6, 6, 6, 6), s);
    const __m512i ss3 = _mm512_permutexvar_epi32(_mm512_setr_epi32(3, 3, 3, 3, 3, 3, 3, 3, 7, 7, 7, 7, 7, 7, 7, 7), s);

    const __m256i r0  = _mm256_loadu_si256((__m256i*)ref);
    const __m256i r1  = _mm256_loadu_si256((__m256i*)(ref + ref_stride));
    const __m512i r   = _mm512_inserti64x4(_mm512_castsi256_si512(r0), r1, 1);
    const __m512i rr0 = _mm512_permutexvar_epi64(_mm512_setr_epi64(0, 1, 1, 2, 4, 5, 5, 6), r);
    const __m512i rr1 = _mm512_permutexvar_epi64(_mm512_setr_epi64(1, 2, 2, 3, 5, 6, 6, 7), r);

    sum[0] = _mm512_adds_epu16(sum[0], _mm512_dbsad_epu8(ss0, rr0, 0x94));
    sum[1] = _mm512_adds_epu16(sum[1], _mm512_dbsad_epu8(ss1, rr0, 0xE9));
    sum[0] = _mm512_adds_epu16(sum[0], _mm512_dbsad_epu8(ss2, rr1, 0x94));
    sum[1] = _mm512_adds_epu16(sum[1], _mm512_dbsad_epu8(ss3, rr1, 0xE9));
}

/*******************************************************************************
* Function helper adds to two elements "sums" vectors SAD's for block width of 16, uses AVX512 instructions
* Requirement: width = 16
* Requirement: height = 1
* Compute one line
*******************************************************************************/
SIMD_INLINE void sad_loop_kernel_16_2sum_oneline_avx512(const uint8_t* const src, const uint8_t* const ref,
                                                        __m512i sum[2]) {
    const __m128i s0  = _mm_lddqu_si128((__m128i*)src);
    const __m256i s01 = _mm256_insertf128_si256(_mm256_castsi128_si256(s0), _mm_setzero_si128(), 1);
    const __m512i s   = _mm512_castsi256_si512(s01);
    const __m512i ss0 = _mm512_permutexvar_epi32(_mm512_setr_epi32(0, 0, 0, 0, 0, 0, 0, 0, 4, 4, 4, 4, 4, 4, 4, 4), s);
    const __m512i ss1 = _mm512_permutexvar_epi32(_mm512_setr_epi32(1, 1, 1, 1, 1, 1, 1, 1, 5, 5, 5, 5, 5, 5, 5, 5), s);
    const __m512i ss2 = _mm512_permutexvar_epi32(_mm512_setr_epi32(2, 2, 2, 2, 2, 2, 2, 2, 6, 6, 6, 6, 6, 6, 6, 6), s);
    const __m512i ss3 = _mm512_permutexvar_epi32(_mm512_setr_epi32(3, 3, 3, 3, 3, 3, 3, 3, 7, 7, 7, 7, 7, 7, 7, 7), s);

    const __m256i r0  = _mm256_loadu_si256((__m256i*)ref);
    const __m512i r   = _mm512_inserti64x4(_mm512_castsi256_si512(r0), _mm256_setzero_si256(), 1);
    const __m512i rr0 = _mm512_permutexvar_epi64(_mm512_setr_epi64(0, 1, 1, 2, 4, 5, 5, 6), r);
    const __m512i rr1 = _mm512_permutexvar_epi64(_mm512_setr_epi64(1, 2, 2, 3, 5, 6, 6, 7), r);

    sum[0] = _mm512_adds_epu16(sum[0], _mm512_dbsad_epu8(ss0, rr0, 0x94));
    sum[1] = _mm512_adds_epu16(sum[1], _mm512_dbsad_epu8(ss1, rr0, 0xE9));
    sum[0] = _mm512_adds_epu16(sum[0], _mm512_dbsad_epu8(ss2, rr1, 0x94));
    sum[1] = _mm512_adds_epu16(sum[1], _mm512_dbsad_epu8(ss3, rr1, 0xE9));
}

/*******************************************************************************
* Function helper adds to "sums" vectors SAD's for block width of 12, uses AVX2 instructions
* Requirement: width = 12
* Requirement: height = 2
*******************************************************************************/
static INLINE void sad_loop_kernel_12_avx2(const uint8_t* const src, const uint32_t src_stride,
                                           const uint8_t* const ref, const uint32_t ref_stride, __m256i* const sum) {
    const __m256i ss0 = _mm256_insertf128_si256(
        _mm256_castsi128_si256(_mm_lddqu_si128((__m128i*)src)), _mm_lddqu_si128((__m128i*)(src + src_stride)), 1);
    const __m256i rr0 = _mm256_insertf128_si256(
        _mm256_castsi128_si256(_mm_lddqu_si128((__m128i*)ref)), _mm_lddqu_si128((__m128i*)(ref + ref_stride)), 1);
    const __m256i rr1 = _mm256_insertf128_si256(_mm256_castsi128_si256(_mm_lddqu_si128((__m128i*)(ref + 8))),
                                                _mm_lddqu_si128((__m128i*)(ref + ref_stride + 8)),
                                                1);
    *sum              = _mm256_adds_epu16(*sum, _mm256_mpsadbw_epu8(rr0, ss0, (0 << 3))); // 000 000
    *sum              = _mm256_adds_epu16(*sum, _mm256_mpsadbw_epu8(rr0, ss0, (5 << 3) | 5)); // 101 101
    *sum              = _mm256_adds_epu16(*sum, _mm256_mpsadbw_epu8(rr1, ss0, (2 << 3) | 2)); // 010 010
}

/*******************************************************************************
* Function helper adds to "sums" vectors SAD's for block width of 12, uses AVX2 instructions
* Requirement: width = 12
* Requirement: height = 1
* Compute one line
*******************************************************************************/
static INLINE void sad_loop_kernel_12_oneline_avx2(const uint8_t* const src, const uint8_t* const ref,
                                                   __m256i* const sum) {
    const __m256i ss0 = _mm256_insertf128_si256(
        _mm256_castsi128_si256(_mm_lddqu_si128((__m128i*)src)), _mm_setzero_si128(), 1);
    const __m256i rr0 = _mm256_insertf128_si256(
        _mm256_castsi128_si256(_mm_lddqu_si128((__m128i*)ref)), _mm_setzero_si128(), 1);
    const __m256i rr1 = _mm256_insertf128_si256(
        _mm256_castsi128_si256(_mm_lddqu_si128((__m128i*)(ref + 8))), _mm_setzero_si128(), 1);
    *sum = _mm256_adds_epu16(*sum, _mm256_mpsadbw_epu8(rr0, ss0, (0 << 3))); // 000 000
    *sum = _mm256_adds_epu16(*sum, _mm256_mpsadbw_epu8(rr0, ss0, (5 << 3) | 5)); // 101 101
    *sum = _mm256_adds_epu16(*sum, _mm256_mpsadbw_epu8(rr1, ss0, (2 << 3) | 2)); // 010 010
}

static INLINE void sad_loop_kernel_16_avx2(const uint8_t* const src, const uint32_t src_stride,
                                           const uint8_t* const ref, const uint32_t ref_stride, __m256i* const sum) {
    const __m256i ss0 = _mm256_insertf128_si256(
        _mm256_castsi128_si256(_mm_lddqu_si128((__m128i*)src)), _mm_lddqu_si128((__m128i*)(src + src_stride)), 1);
    const __m256i rr0 = _mm256_insertf128_si256(
        _mm256_castsi128_si256(_mm_lddqu_si128((__m128i*)ref)), _mm_lddqu_si128((__m128i*)(ref + ref_stride)), 1);
    const __m256i rr1 = _mm256_insertf128_si256(_mm256_castsi128_si256(_mm_lddqu_si128((__m128i*)(ref + 8))),
                                                _mm_lddqu_si128((__m128i*)(ref + ref_stride + 8)),
                                                1);
    *sum              = _mm256_adds_epu16(*sum, _mm256_mpsadbw_epu8(rr0, ss0, (0 << 3))); // 000 000
    *sum              = _mm256_adds_epu16(*sum, _mm256_mpsadbw_epu8(rr0, ss0, (5 << 3) | 5)); // 101 101
    *sum              = _mm256_adds_epu16(*sum, _mm256_mpsadbw_epu8(rr1, ss0, (2 << 3) | 2)); // 010 010
    *sum              = _mm256_adds_epu16(*sum, _mm256_mpsadbw_epu8(rr1, ss0, (7 << 3) | 7)); // 111 111
}

/*******************************************************************************
* Function helper adds to "sums" vectors SAD's for block width of 16, uses AVX512 instructions
* Requirement: width = 16
* Requirement: height = 1
* Compute one line
*******************************************************************************/
static INLINE void sad_loop_kernel_16_oneline_avx2(const uint8_t* const src, const uint8_t* const ref,
                                                   __m256i* const sum) {
    const __m256i ss0 = _mm256_insertf128_si256(
        _mm256_castsi128_si256(_mm_lddqu_si128((__m128i*)src)), _mm_setzero_si128(), 1);
    const __m256i rr0 = _mm256_insertf128_si256(
        _mm256_castsi128_si256(_mm_lddqu_si128((__m128i*)ref)), _mm_setzero_si128(), 1);
    const __m256i rr1 = _mm256_insertf128_si256(
        _mm256_castsi128_si256(_mm_lddqu_si128((__m128i*)(ref + 8))), _mm_setzero_si128(), 1);
    *sum = _mm256_adds_epu16(*sum, _mm256_mpsadbw_epu8(rr0, ss0, (0 << 3))); // 000 000
    *sum = _mm256_adds_epu16(*sum, _mm256_mpsadbw_epu8(rr0, ss0, (5 << 3) | 5)); // 101 101
    *sum = _mm256_adds_epu16(*sum, _mm256_mpsadbw_epu8(rr1, ss0, (2 << 3) | 2)); // 010 010
    *sum = _mm256_adds_epu16(*sum, _mm256_mpsadbw_epu8(rr1, ss0, (7 << 3) | 7)); // 111 111
}

/*******************************************************************************
* Function helper adds to two elements "sums" vectors SAD's for block width of 12, uses AVX2 instructions
* Requirement: width = 12
* Requirement: height = 2
*******************************************************************************/
static INLINE void sad_loop_kernel_12_2sum_avx2(const uint8_t* const src, const uint32_t src_stride,
                                                const uint8_t* const ref, const uint32_t ref_stride, __m256i sums[2]) {
    const __m256i ss0 = _mm256_insertf128_si256(
        _mm256_castsi128_si256(_mm_lddqu_si128((__m128i*)src)), _mm_lddqu_si128((__m128i*)(src + src_stride)), 1);
    const __m256i rr0 = _mm256_insertf128_si256(
        _mm256_castsi128_si256(_mm_lddqu_si128((__m128i*)ref)), _mm_lddqu_si128((__m128i*)(ref + ref_stride)), 1);
    const __m256i rr1 = _mm256_insertf128_si256(_mm256_castsi128_si256(_mm_lddqu_si128((__m128i*)(ref + 8))),
                                                _mm_lddqu_si128((__m128i*)(ref + ref_stride + 8)),
                                                1);
    sums[0]           = _mm256_adds_epu16(sums[0], _mm256_mpsadbw_epu8(rr0, ss0, (0 << 3))); // 000 000
    sums[1]           = _mm256_adds_epu16(sums[1], _mm256_mpsadbw_epu8(rr0, ss0, (5 << 3) | 5)); // 101 101
    sums[0]           = _mm256_adds_epu16(sums[0], _mm256_mpsadbw_epu8(rr1, ss0, (2 << 3) | 2)); // 010 010
}

/*******************************************************************************
* Function helper adds to two elements "sums" vectors SAD's for block width of 12, uses AVX2 instructions
* Requirement: width = 12
* Requirement: height = 1
* Compute one line
*******************************************************************************/
static INLINE void sad_loop_kernel_12_2sum_oneline_avx2(const uint8_t* const src, const uint8_t* const ref,
                                                        __m256i sums[2]) {
    const __m256i ss0 = _mm256_insertf128_si256(
        _mm256_castsi128_si256(_mm_lddqu_si128((__m128i*)src)), _mm_setzero_si128(), 1);
    const __m256i rr0 = _mm256_insertf128_si256(
        _mm256_castsi128_si256(_mm_lddqu_si128((__m128i*)ref)), _mm_setzero_si128(), 1);
    const __m256i rr1 = _mm256_insertf128_si256(
        _mm256_castsi128_si256(_mm_lddqu_si128((__m128i*)(ref + 8))), _mm_setzero_si128(), 1);
    sums[0] = _mm256_adds_epu16(sums[0], _mm256_mpsadbw_epu8(rr0, ss0, (0 << 3))); // 000 000
    sums[1] = _mm256_adds_epu16(sums[1], _mm256_mpsadbw_epu8(rr0, ss0, (5 << 3) | 5)); // 101 101
    sums[0] = _mm256_adds_epu16(sums[0], _mm256_mpsadbw_epu8(rr1, ss0, (2 << 3) | 2)); // 010 010
}

static INLINE void sad_loop_kernel_16_2sum_avx2(const uint8_t* const src, const uint32_t src_stride,
                                                const uint8_t* const ref, const uint32_t ref_stride, __m256i sums[2]) {
    const __m256i ss0 = _mm256_insertf128_si256(
        _mm256_castsi128_si256(_mm_lddqu_si128((__m128i*)src)), _mm_lddqu_si128((__m128i*)(src + src_stride)), 1);
    const __m256i rr0 = _mm256_insertf128_si256(
        _mm256_castsi128_si256(_mm_lddqu_si128((__m128i*)ref)), _mm_lddqu_si128((__m128i*)(ref + ref_stride)), 1);
    const __m256i rr1 = _mm256_insertf128_si256(_mm256_castsi128_si256(_mm_lddqu_si128((__m128i*)(ref + 8))),
                                                _mm_lddqu_si128((__m128i*)(ref + ref_stride + 8)),
                                                1);
    sums[0]           = _mm256_adds_epu16(sums[0], _mm256_mpsadbw_epu8(rr0, ss0, (0 << 3))); // 000 000
    sums[1]           = _mm256_adds_epu16(sums[1], _mm256_mpsadbw_epu8(rr0, ss0, (5 << 3) | 5)); // 101 101
    sums[0]           = _mm256_adds_epu16(sums[0], _mm256_mpsadbw_epu8(rr1, ss0, (2 << 3) | 2)); // 010 010
    sums[1]           = _mm256_adds_epu16(sums[1], _mm256_mpsadbw_epu8(rr1, ss0, (7 << 3) | 7)); // 111 111
}

/*******************************************************************************
* Function helper adds to two elements "sums" vectors SAD's for block width of 16, uses AVX2 instructions
* Requirement: width = 16
* Requirement: height = 1
* Compute one line
*******************************************************************************/
static INLINE void sad_loop_kernel_16_2sum_oneline_avx2(const uint8_t* const src, const uint8_t* const ref,
                                                        __m256i sums[2]) {
    const __m256i ss0 = _mm256_insertf128_si256(
        _mm256_castsi128_si256(_mm_lddqu_si128((__m128i*)src)), _mm_setzero_si128(), 1);
    const __m256i rr0 = _mm256_insertf128_si256(
        _mm256_castsi128_si256(_mm_lddqu_si128((__m128i*)ref)), _mm_setzero_si128(), 1);
    const __m256i rr1 = _mm256_insertf128_si256(
        _mm256_castsi128_si256(_mm_lddqu_si128((__m128i*)(ref + 8))), _mm_setzero_si128(), 1);
    sums[0] = _mm256_adds_epu16(sums[0], _mm256_mpsadbw_epu8(rr0, ss0, (0 << 3))); // 000 000
    sums[1] = _mm256_adds_epu16(sums[1], _mm256_mpsadbw_epu8(rr0, ss0, (5 << 3) | 5)); // 101 101
    sums[0] = _mm256_adds_epu16(sums[0], _mm256_mpsadbw_epu8(rr1, ss0, (2 << 3) | 2)); // 010 010
    sums[1] = _mm256_adds_epu16(sums[1], _mm256_mpsadbw_epu8(rr1, ss0, (7 << 3) | 7)); // 111 111
}

SIMD_INLINE void sad_loop_kernel_32_avx512(const uint8_t* const src, const uint8_t* const ref, __m512i* const sum) {
    const __m256i s01 = _mm256_loadu_si256((__m256i*)src);
    const __m512i s   = _mm512_castsi256_si512(s01);
    const __m512i ss0 = _mm512_permutexvar_epi32(_mm512_setr_epi32(0, 0, 0, 0, 0, 0, 0, 0, 4, 4, 4, 4, 4, 4, 4, 4), s);
    const __m512i ss1 = _mm512_permutexvar_epi32(_mm512_setr_epi32(1, 1, 1, 1, 1, 1, 1, 1, 5, 5, 5, 5, 5, 5, 5, 5), s);
    const __m512i ss2 = _mm512_permutexvar_epi32(_mm512_setr_epi32(2, 2, 2, 2, 2, 2, 2, 2, 6, 6, 6, 6, 6, 6, 6, 6), s);
    const __m512i ss3 = _mm512_permutexvar_epi32(_mm512_setr_epi32(3, 3, 3, 3, 3, 3, 3, 3, 7, 7, 7, 7, 7, 7, 7, 7), s);

    const __m512i r   = _mm512_loadu_si512((__m512i*)ref);
    const __m512i rr0 = _mm512_permutexvar_epi64(_mm512_setr_epi64(0, 1, 1, 2, 2, 3, 3, 4), r);
    const __m512i rr1 = _mm512_permutexvar_epi64(_mm512_setr_epi64(1, 2, 2, 3, 3, 4, 4, 5), r);

    *sum = _mm512_adds_epu16(*sum, _mm512_dbsad_epu8(ss0, rr0, 0x94));
    *sum = _mm512_adds_epu16(*sum, _mm512_dbsad_epu8(ss1, rr0, 0xE9));
    *sum = _mm512_adds_epu16(*sum, _mm512_dbsad_epu8(ss2, rr1, 0x94));
    *sum = _mm512_adds_epu16(*sum, _mm512_dbsad_epu8(ss3, rr1, 0xE9));
}

SIMD_INLINE void sad_loop_kernel_32_2sum_avx512(const uint8_t* const src, const uint8_t* const ref, __m512i sums[2]) {
    const __m256i s01 = _mm256_loadu_si256((__m256i*)src);
    const __m512i s   = _mm512_castsi256_si512(s01);
    const __m512i ss0 = _mm512_permutexvar_epi32(_mm512_setr_epi32(0, 0, 0, 0, 0, 0, 0, 0, 4, 4, 4, 4, 4, 4, 4, 4), s);
    const __m512i ss1 = _mm512_permutexvar_epi32(_mm512_setr_epi32(1, 1, 1, 1, 1, 1, 1, 1, 5, 5, 5, 5, 5, 5, 5, 5), s);
    const __m512i ss2 = _mm512_permutexvar_epi32(_mm512_setr_epi32(2, 2, 2, 2, 2, 2, 2, 2, 6, 6, 6, 6, 6, 6, 6, 6), s);
    const __m512i ss3 = _mm512_permutexvar_epi32(_mm512_setr_epi32(3, 3, 3, 3, 3, 3, 3, 3, 7, 7, 7, 7, 7, 7, 7, 7), s);

    const __m512i r   = _mm512_loadu_si512((__m512i*)ref);
    const __m512i rr0 = _mm512_permutexvar_epi64(_mm512_setr_epi64(0, 1, 1, 2, 2, 3, 3, 4), r);
    const __m512i rr1 = _mm512_permutexvar_epi64(_mm512_setr_epi64(1, 2, 2, 3, 3, 4, 4, 5), r);

    sums[0] = _mm512_adds_epu16(sums[0], _mm512_dbsad_epu8(ss0, rr0, 0x94));
    sums[1] = _mm512_adds_epu16(sums[1], _mm512_dbsad_epu8(ss1, rr0, 0xE9));
    sums[0] = _mm512_adds_epu16(sums[0], _mm512_dbsad_epu8(ss2, rr1, 0x94));
    sums[1] = _mm512_adds_epu16(sums[1], _mm512_dbsad_epu8(ss3, rr1, 0xE9));
}

SIMD_INLINE void sad_loop_kernel_32_4sum_avx512(const uint8_t* const src, const uint8_t* const ref, __m512i sums[4]) {
    const __m256i s01 = _mm256_loadu_si256((__m256i*)src);
    const __m512i s   = _mm512_castsi256_si512(s01);
    const __m512i ss0 = _mm512_permutexvar_epi32(_mm512_setr_epi32(0, 0, 0, 0, 0, 0, 0, 0, 4, 4, 4, 4, 4, 4, 4, 4), s);
    const __m512i ss1 = _mm512_permutexvar_epi32(_mm512_setr_epi32(1, 1, 1, 1, 1, 1, 1, 1, 5, 5, 5, 5, 5, 5, 5, 5), s);
    const __m512i ss2 = _mm512_permutexvar_epi32(_mm512_setr_epi32(2, 2, 2, 2, 2, 2, 2, 2, 6, 6, 6, 6, 6, 6, 6, 6), s);
    const __m512i ss3 = _mm512_permutexvar_epi32(_mm512_setr_epi32(3, 3, 3, 3, 3, 3, 3, 3, 7, 7, 7, 7, 7, 7, 7, 7), s);

    const __m512i r   = _mm512_loadu_si512((__m512i*)ref);
    const __m512i rr0 = _mm512_permutexvar_epi64(_mm512_setr_epi64(0, 1, 1, 2, 2, 3, 3, 4), r);
    const __m512i rr1 = _mm512_permutexvar_epi64(_mm512_setr_epi64(1, 2, 2, 3, 3, 4, 4, 5), r);

    sums[0] = _mm512_adds_epu16(sums[0], _mm512_dbsad_epu8(ss0, rr0, 0x94));
    sums[1] = _mm512_adds_epu16(sums[1], _mm512_dbsad_epu8(ss1, rr0, 0xE9));
    sums[2] = _mm512_adds_epu16(sums[2], _mm512_dbsad_epu8(ss2, rr1, 0x94));
    sums[3] = _mm512_adds_epu16(sums[3], _mm512_dbsad_epu8(ss3, rr1, 0xE9));
}

static INLINE void sad_loop_kernel_32_avx2(const uint8_t* const src, const uint8_t* const ref, __m256i* const sum) {
    const __m256i ss0 = _mm256_loadu_si256((__m256i*)src);
    const __m256i rr0 = _mm256_loadu_si256((__m256i*)ref);
    const __m256i rr1 = _mm256_loadu_si256((__m256i*)(ref + 8));
    *sum              = _mm256_adds_epu16(*sum, _mm256_mpsadbw_epu8(rr0, ss0, (0 << 3))); // 000 000
    *sum              = _mm256_adds_epu16(*sum, _mm256_mpsadbw_epu8(rr0, ss0, (5 << 3) | 5)); // 101 101
    *sum              = _mm256_adds_epu16(*sum, _mm256_mpsadbw_epu8(rr1, ss0, (2 << 3) | 2)); // 010 010
    *sum              = _mm256_adds_epu16(*sum, _mm256_mpsadbw_epu8(rr1, ss0, (7 << 3) | 7)); // 111 111
}

static INLINE void sad_loop_kernel_32_2sum_avx2(const uint8_t* const src, const uint8_t* const ref, __m256i sums[2]) {
    const __m256i ss0 = _mm256_loadu_si256((__m256i*)src);
    const __m256i rr0 = _mm256_loadu_si256((__m256i*)ref);
    const __m256i rr1 = _mm256_loadu_si256((__m256i*)(ref + 8));
    sums[0]           = _mm256_adds_epu16(sums[0], _mm256_mpsadbw_epu8(rr0, ss0, (0 << 3))); // 000 000
    sums[1]           = _mm256_adds_epu16(sums[1], _mm256_mpsadbw_epu8(rr0, ss0, (5 << 3) | 5)); // 101 101
    sums[0]           = _mm256_adds_epu16(sums[0], _mm256_mpsadbw_epu8(rr1, ss0, (2 << 3) | 2)); // 010 010
    sums[1]           = _mm256_adds_epu16(sums[1], _mm256_mpsadbw_epu8(rr1, ss0, (7 << 3) | 7)); // 111 111
}

SIMD_INLINE void sad_loop_kernel_32_4sum_avx2(const uint8_t* const src, const uint8_t* const ref, __m256i sums[4]) {
    const __m256i ss0 = _mm256_loadu_si256((__m256i*)src);
    const __m256i rr0 = _mm256_loadu_si256((__m256i*)ref);
    const __m256i rr1 = _mm256_loadu_si256((__m256i*)(ref + 8));
    sums[0]           = _mm256_adds_epu16(sums[0], _mm256_mpsadbw_epu8(rr0, ss0, (0 << 3))); // 000 000
    sums[1]           = _mm256_adds_epu16(sums[1], _mm256_mpsadbw_epu8(rr0, ss0, (5 << 3) | 5)); // 101 101
    sums[2]           = _mm256_adds_epu16(sums[2], _mm256_mpsadbw_epu8(rr1, ss0, (2 << 3) | 2)); // 010 010
    sums[3]           = _mm256_adds_epu16(sums[3], _mm256_mpsadbw_epu8(rr1, ss0, (7 << 3) | 7)); // 111 111
}

static INLINE void sad_loop_kernel_64_2sum_avx2(const uint8_t* const src, const uint8_t* const ref, __m256i sums[2]) {
    sad_loop_kernel_32_2sum_avx2(src + 0 * 32, ref + 0 * 32, sums);
    sad_loop_kernel_32_2sum_avx2(src + 1 * 32, ref + 1 * 32, sums);
}

static INLINE void sad_loop_kernel_64_4sum_avx2(const uint8_t* const src, const uint8_t* const ref, __m256i sums[4]) {
    sad_loop_kernel_32_4sum_avx2(src + 0 * 32, ref + 0 * 32, sums);
    sad_loop_kernel_32_4sum_avx2(src + 1 * 32, ref + 1 * 32, sums);
}

static INLINE void sad_loop_kernel_64_8sum_avx2(const uint8_t* const src, const uint8_t* const ref, __m256i sums[8]) {
    //sad_loop_kernel_32_4sum_avx2(src + 0 * 32, ref + 0 * 32, sums + 0);
    //sad_loop_kernel_32_4sum_avx2(src + 1 * 32, ref + 1 * 32, sums + 4);
    const __m256i ss0 = _mm256_loadu_si256((__m256i*)src);
    const __m256i ss1 = _mm256_loadu_si256((__m256i*)(src + 32));
    const __m256i rr0 = _mm256_loadu_si256((__m256i*)ref);
    const __m256i rr1 = _mm256_loadu_si256((__m256i*)(ref + 8));
    const __m256i rr2 = _mm256_loadu_si256((__m256i*)(ref + 32));
    const __m256i rr3 = _mm256_loadu_si256((__m256i*)(ref + 40));

    sums[0] = _mm256_adds_epu16(sums[0], _mm256_mpsadbw_epu8(rr0, ss0, (0 << 3))); // 000 000
    sums[1] = _mm256_adds_epu16(sums[1], _mm256_mpsadbw_epu8(rr0, ss0, (5 << 3) | 5)); // 101 101
    sums[2] = _mm256_adds_epu16(sums[2], _mm256_mpsadbw_epu8(rr1, ss0, (2 << 3) | 2)); // 010 010
    sums[3] = _mm256_adds_epu16(sums[3], _mm256_mpsadbw_epu8(rr1, ss0, (7 << 3) | 7)); // 111 111
    sums[4] = _mm256_adds_epu16(sums[4], _mm256_mpsadbw_epu8(rr2, ss1, (0 << 3))); // 000 000
    sums[5] = _mm256_adds_epu16(sums[5], _mm256_mpsadbw_epu8(rr2, ss1, (5 << 3) | 5)); // 101 101
    sums[6] = _mm256_adds_epu16(sums[6], _mm256_mpsadbw_epu8(rr3, ss1, (2 << 3) | 2)); // 010 010
    sums[7] = _mm256_adds_epu16(sums[7], _mm256_mpsadbw_epu8(rr3, ss1, (7 << 3) | 7)); // 111 111
}

#define UPDATE_BEST(sum, idx, offset, best_s, best_x, best_y) \
    {                                                         \
        const uint32_t sad = _mm_extract_epi32(sum, idx);     \
                                                              \
        if (sad < best_s) {                                   \
            best_s = sad;                                     \
            best_x = (offset) + (idx);                        \
            best_y = y;                                       \
        }                                                     \
    }

static INLINE void update_best_kernel(const uint32_t sad, const __m128i minpos, const int32_t x, const int32_t y,
                                      uint32_t* const best_s, int32_t* const best_x, int32_t* const best_y) {
    if (sad < *best_s) {
        const int32_t x_offset = _mm_extract_epi16(minpos, 1);
        *best_s                = sad;
        *best_x                = x + x_offset;
        *best_y                = y;
    }
}

static INLINE void update_best(const __m128i sad, const int32_t x, const int32_t y, uint32_t* const best_s,
                               int32_t* const best_x, int32_t* const best_y) {
    const __m128i  minpos = _mm_minpos_epu16(sad);
    const uint32_t min0   = _mm_extract_epi16(minpos, 0);

    if (min0 < *best_s) {
        const int32_t x_offset = _mm_extract_epi16(minpos, 1);
        *best_s                = min0;
        *best_x                = x + x_offset;
        *best_y                = y;
    }
}

SIMD_INLINE void update_8_best(const __m128i sads[2], const int32_t x, const int32_t y, uint32_t* const best_s,
                               int32_t* const best_x, int32_t* const best_y) {
    UPDATE_BEST(sads[0], 0, x + 0, *best_s, *best_x, *best_y);
    UPDATE_BEST(sads[0], 1, x + 0, *best_s, *best_x, *best_y);
    UPDATE_BEST(sads[0], 2, x + 0, *best_s, *best_x, *best_y);
    UPDATE_BEST(sads[0], 3, x + 0, *best_s, *best_x, *best_y);
    UPDATE_BEST(sads[1], 0, x + 4, *best_s, *best_x, *best_y);
    UPDATE_BEST(sads[1], 1, x + 4, *best_s, *best_x, *best_y);
    UPDATE_BEST(sads[1], 2, x + 4, *best_s, *best_x, *best_y);
    UPDATE_BEST(sads[1], 3, x + 4, *best_s, *best_x, *best_y);
}

static INLINE void update_small_pel(const __m256i sum256, const int32_t x, const int32_t y, uint32_t* const best_s,
                                    int32_t* const best_x, int32_t* const best_y) {
    const __m128i sum256_lo = _mm256_castsi256_si128(sum256);
    const __m128i sum256_hi = _mm256_extracti128_si256(sum256, 1);
    const __m128i sad       = _mm_adds_epu16(sum256_lo, sum256_hi);
    update_best(sad, x, y, best_s, best_x, best_y);
}

static INLINE void update_some_pel(const __m256i sum256, const int32_t x, const int32_t y, uint32_t* const best_s,
                                   int32_t* const best_x, int32_t* const best_y) {
    const __m128i  sum0 = _mm256_castsi256_si128(sum256);
    const __m128i  sum1 = _mm256_extracti128_si256(sum256, 1);
    __m128i        minpos;
    const uint32_t min0 = saturate_add(sum0, sum1, &minpos);
    update_best_kernel(min0, minpos, x, y, best_s, best_x, best_y);
}

static INLINE void update_256_pel(const __m512i sum512, const int32_t x, const int32_t y, uint32_t* const best_s,
                                  int32_t* const best_x, int32_t* const best_y) {
    const __m256i sum512_lo = _mm512_castsi512_si256(sum512);
    const __m256i sum512_hi = _mm512_extracti64x4_epi64(sum512, 1);
    const __m256i sum256    = _mm256_adds_epu16(sum512_lo, sum512_hi);
    const __m128i sum256_lo = _mm256_castsi256_si128(sum256);
    const __m128i sum256_hi = _mm256_extracti128_si256(sum256, 1);
    update_best(sum256_lo, x + 0, y, best_s, best_x, best_y);
    update_best(sum256_hi, x + 8, y, best_s, best_x, best_y);
}

SIMD_INLINE void update_384_pel(const __m512i sum512, const __m256i sums256[2], const int32_t x, const int32_t y,
                                uint32_t* const best_s, int32_t* const best_x, int32_t* const best_y) {
    const __m256i sum512_lo = _mm512_castsi512_si256(sum512);
    const __m256i sum512_hi = _mm512_extracti64x4_epi64(sum512, 1);
    const __m256i sad256    = _mm256_adds_epu16(sum512_lo, sum512_hi);
    const __m128i sad256_lo = _mm256_castsi256_si128(sad256);
    const __m128i sad256_hi = _mm256_extracti128_si256(sad256, 1);

    const __m128i sum256_0_lo = _mm256_castsi256_si128(sums256[0]);
    const __m128i sum256_0_hi = _mm256_extracti128_si256(sums256[0], 1);
    const __m128i sad128_0    = _mm_adds_epu16(sum256_0_lo, sum256_0_hi);

    const __m128i sum256_1_lo = _mm256_castsi256_si128(sums256[1]);
    const __m128i sum256_1_hi = _mm256_extracti128_si256(sums256[1], 1);
    const __m128i sad128_1    = _mm_adds_epu16(sum256_1_lo, sum256_1_hi);

    __m128i        minpos_lo, minpos_hi;
    const uint32_t min_lo = saturate_add(sad256_lo, sad128_0, &minpos_lo);
    update_best_kernel(min_lo, minpos_lo, x + 0, y, best_s, best_x, best_y);
    const uint32_t min_hi = saturate_add(sad256_hi, sad128_1, &minpos_hi);
    update_best_kernel(min_hi, minpos_hi, x + 8, y, best_s, best_x, best_y);
}

SIMD_INLINE void update_512_pel(const __m512i sum512, const int32_t x, const int32_t y, uint32_t* const best_s,
                                int32_t* const best_x, int32_t* const best_y) {
    const __m256i sum512_lo = _mm512_castsi512_si256(sum512);
    const __m256i sum512_hi = _mm512_extracti64x4_epi64(sum512, 1);
    __m128i       minpos_lo, minpos_hi;

    const __m128i  sum128_0 = _mm256_castsi256_si128(sum512_lo);
    const __m128i  sum128_1 = _mm256_castsi256_si128(sum512_hi);
    const uint32_t min_lo   = saturate_add(sum128_0, sum128_1, &minpos_lo);
    update_best_kernel(min_lo, minpos_lo, x + 0, y, best_s, best_x, best_y);

    const __m128i  sum128_2 = _mm256_extracti128_si256(sum512_lo, 1);
    const __m128i  sum128_3 = _mm256_extracti128_si256(sum512_hi, 1);
    const uint32_t min_hi   = saturate_add(sum128_2, sum128_3, &minpos_hi);
    update_best_kernel(min_hi, minpos_hi, x + 8, y, best_s, best_x, best_y);
}

SIMD_INLINE void update_1024_pel(const __m512i sums512[2], const int32_t x, const int32_t y, uint32_t* const best_s,
                                 int32_t* const best_x, int32_t* const best_y) {
    const __m512i  sum       = _mm512_adds_epu16(sums512[0], sums512[1]);
    const __m256i  sum_lo    = _mm512_castsi512_si256(sum);
    const __m256i  sum_hi    = _mm512_extracti64x4_epi64(sum, 1);
    const __m256i  sad       = _mm256_adds_epu16(sum_lo, sum_hi);
    const __m128i  sad_lo    = _mm256_castsi256_si128(sad);
    const __m128i  sad_hi    = _mm256_extracti128_si256(sad, 1);
    const __m128i  minpos_lo = _mm_minpos_epu16(sad_lo);
    const __m128i  minpos_hi = _mm_minpos_epu16(sad_hi);
    const uint32_t min0      = _mm_extract_epi16(minpos_lo, 0);
    const uint32_t min1      = _mm_extract_epi16(minpos_hi, 0);
    uint32_t       minmin, delta;
    __m128i        minpos;

    if (min0 <= min1) {
        minmin = min0;
        delta  = 0;
        minpos = minpos_lo;
    } else {
        minmin = min1;
        delta  = 8;
        minpos = minpos_hi;
    }

    if (minmin < *best_s) {
        if (minmin != 0xFFFF) { // no overflow
            *best_s = minmin;
            *best_x = x + delta + _mm_extract_epi16(minpos, 1);
            *best_y = y;
        } else { // overflow
            __m256i sads256[2];
            __m128i sads128[2];

            add16x16x2to32bit(sums512, sads256);

            sads128[0] = _mm256_castsi256_si128(sads256[0]);
            sads128[1] = _mm256_extracti128_si256(sads256[0], 1);
            update_8_best(sads128, x + 0, y, best_s, best_x, best_y);

            sads128[0] = _mm256_castsi256_si128(sads256[1]);
            sads128[1] = _mm256_extracti128_si256(sads256[1], 1);
            update_8_best(sads128, x + 8, y, best_s, best_x, best_y);
        }
    }
}

SIMD_INLINE void update_768_pel(const __m512i sum512, const __m256i sums256[2], const int32_t x, const int32_t y,
                                uint32_t* const best_s, int32_t* const best_x, int32_t* const best_y) {
    __m512i sums512[2];

    sums512[0] = sum512;
    sums512[1] = _mm512_inserti64x4(_mm512_castsi256_si512(sums256[0]), sums256[1], 1);
    sums512[1] = _mm512_permutexvar_epi64(_mm512_setr_epi64(0, 1, 4, 5, 2, 3, 6, 7), sums512[1]);
    update_1024_pel(sums512, x, y, best_s, best_x, best_y);
}

SIMD_INLINE void update_1536_pel(const __m512i sums512[3], const int32_t x, const int32_t y, uint32_t* const best_s,
                                 int32_t* const best_x, int32_t* const best_y) {
    const __m512i  sum01     = _mm512_adds_epu16(sums512[0], sums512[1]);
    const __m512i  sum       = _mm512_adds_epu16(sum01, sums512[2]);
    const __m256i  sum_lo    = _mm512_castsi512_si256(sum);
    const __m256i  sum_hi    = _mm512_extracti64x4_epi64(sum, 1);
    const __m256i  sad       = _mm256_adds_epu16(sum_lo, sum_hi);
    const __m128i  sad_lo    = _mm256_castsi256_si128(sad);
    const __m128i  sad_hi    = _mm256_extracti128_si256(sad, 1);
    const __m128i  minpos_lo = _mm_minpos_epu16(sad_lo);
    const __m128i  minpos_hi = _mm_minpos_epu16(sad_hi);
    const uint32_t min0      = _mm_extract_epi16(minpos_lo, 0);
    const uint32_t min1      = _mm_extract_epi16(minpos_hi, 0);
    uint32_t       minmin, delta;
    __m128i        minpos;

    if (min0 <= min1) {
        minmin = min0;
        delta  = 0;
        minpos = minpos_lo;
    } else {
        minmin = min1;
        delta  = 8;
        minpos = minpos_hi;
    }

    if (minmin < *best_s) {
        if (minmin != 0xFFFF) { // no overflow
            *best_s = minmin;
            *best_x = x + delta + _mm_extract_epi16(minpos, 1);
            *best_y = y;
        } else { // overflow
            __m256i sads256[2];
            __m128i sads128[2];

            add16x16x3to32bit(sums512, sads256);

            sads128[0] = _mm256_castsi256_si128(sads256[0]);
            sads128[1] = _mm256_extracti128_si256(sads256[0], 1);
            update_8_best(sads128, x + 0, y, best_s, best_x, best_y);

            sads128[0] = _mm256_castsi256_si128(sads256[1]);
            sads128[1] = _mm256_extracti128_si256(sads256[1], 1);
            update_8_best(sads128, x + 8, y, best_s, best_x, best_y);
        }
    }
}

SIMD_INLINE void update_2048_pel(const __m512i sums512[4], const int32_t x, const int32_t y, uint32_t* const best_s,
                                 int32_t* const best_x, int32_t* const best_y) {
    const __m512i  sum01     = _mm512_adds_epu16(sums512[0], sums512[1]);
    const __m512i  sum23     = _mm512_adds_epu16(sums512[2], sums512[3]);
    const __m512i  sum       = _mm512_adds_epu16(sum01, sum23);
    const __m256i  sum_lo    = _mm512_castsi512_si256(sum);
    const __m256i  sum_hi    = _mm512_extracti64x4_epi64(sum, 1);
    const __m256i  sad       = _mm256_adds_epu16(sum_lo, sum_hi);
    const __m128i  sad_lo    = _mm256_castsi256_si128(sad);
    const __m128i  sad_hi    = _mm256_extracti128_si256(sad, 1);
    const __m128i  minpos_lo = _mm_minpos_epu16(sad_lo);
    const __m128i  minpos_hi = _mm_minpos_epu16(sad_hi);
    const uint32_t min0      = _mm_extract_epi16(minpos_lo, 0);
    const uint32_t min1      = _mm_extract_epi16(minpos_hi, 0);
    uint32_t       minmin, delta;
    __m128i        minpos;

    if (min0 <= min1) {
        minmin = min0;
        delta  = 0;
        minpos = minpos_lo;
    } else {
        minmin = min1;
        delta  = 8;
        minpos = minpos_hi;
    }

    if (minmin < *best_s) {
        if (minmin != 0xFFFF) { // no overflow
            *best_s = minmin;
            *best_x = x + delta + _mm_extract_epi16(minpos, 1);
            *best_y = y;
        } else { // overflow
            __m256i sads256[2];
            __m128i sads128[2];

            add16x16x4to32bit(sums512, sads256);

            sads128[0] = _mm256_castsi256_si128(sads256[0]);
            sads128[1] = _mm256_extracti128_si256(sads256[0], 1);
            update_8_best(sads128, x + 0, y, best_s, best_x, best_y);

            sads128[0] = _mm256_castsi256_si128(sads256[1]);
            sads128[1] = _mm256_extracti128_si256(sads256[1], 1);
            update_8_best(sads128, x + 8, y, best_s, best_x, best_y);
        }
    }
}

static INLINE void update_leftover_small_pel(const __m256i sum256, const int32_t x, const int32_t y, const __m128i mask,
                                             uint32_t* const best_s, int32_t* const best_x, int32_t* const best_y) {
    const __m128i sum256_lo = _mm256_castsi256_si128(sum256);
    const __m128i sum256_hi = _mm256_extracti128_si256(sum256, 1);
    __m128i       sad       = _mm_adds_epu16(sum256_lo, sum256_hi);
    sad                     = _mm_or_si128(sad, mask);
    update_best(sad, x, y, best_s, best_x, best_y);
}

static INLINE void update_leftover_256_pel(const __m256i sum256, const int16_t search_area_width, const int32_t x,
                                           const int32_t y, const __m128i mask, uint32_t* const best_s,
                                           int32_t* const best_x, int32_t* const best_y) {
    const __m128i sum256_lo = _mm256_castsi256_si128(sum256);
    const __m128i sum256_hi = _mm256_extracti128_si256(sum256, 1);
    __m128i       sad       = _mm_adds_epu16(sum256_lo, sum256_hi);
    if ((x + 8) > search_area_width) {
        sad = _mm_or_si128(sad, mask);
    }
    update_best(sad, x, y, best_s, best_x, best_y);
}

static INLINE void update_leftover_512_pel(const __m256i sum256, const int16_t search_area_width, const int32_t x,
                                           const int32_t y, const __m256i mask, uint32_t* const best_s,
                                           int32_t* const best_x, int32_t* const best_y) {
    __m256i sum = sum256;
    if ((x + 8) > search_area_width) {
        sum = _mm256_or_si256(sum256, mask);
    }
    const __m128i  sum0 = _mm256_castsi256_si128(sum);
    const __m128i  sum1 = _mm256_extracti128_si256(sum, 1);
    __m128i        minpos;
    const uint32_t min0 = saturate_add(sum0, sum1, &minpos);
    update_best_kernel(min0, minpos, x, y, best_s, best_x, best_y);
}

SIMD_INLINE void update_leftover8_1024_pel(const __m256i sums256[2], const int32_t x, const int32_t y,
                                           uint32_t* const best_s, int32_t* const best_x, int32_t* const best_y) {
    const __m256i  sum256 = _mm256_adds_epu16(sums256[0], sums256[1]);
    const __m128i  sum_lo = _mm256_castsi256_si128(sum256);
    const __m128i  sum_hi = _mm256_extracti128_si256(sum256, 1);
    const __m128i  sad    = _mm_adds_epu16(sum_lo, sum_hi);
    const __m128i  minpos = _mm_minpos_epu16(sad);
    const uint32_t min0   = _mm_extract_epi16(minpos, 0);

    if (min0 < *best_s) {
        if (min0 != 0xFFFF) { // no overflow
            *best_s = min0;
            *best_x = x + _mm_extract_epi16(minpos, 1);
            *best_y = y;
        } else { // overflow
            __m128i sads[2];
            add16x8x2to32bit(sums256, sads);
            update_8_best(sads, x, y, best_s, best_x, best_y);
        }
    }
}

SIMD_INLINE void update_leftover_1024_pel(const __m256i sums256[2], const int16_t search_area_width, const int32_t x,
                                          const int32_t y, const uint32_t leftover, const __m128i mask,
                                          uint32_t* const best_s, int32_t* const best_x, int32_t* const best_y) {
    const __m256i  sum256 = _mm256_adds_epu16(sums256[0], sums256[1]);
    const __m128i  sum_lo = _mm256_castsi256_si128(sum256);
    const __m128i  sum_hi = _mm256_extracti128_si256(sum256, 1);
    const __m128i  sad0   = _mm_adds_epu16(sum_lo, sum_hi);
    const __m128i  sad1   = _mm_or_si128(sad0, mask);
    const __m128i  minpos = _mm_minpos_epu16(sad1);
    const uint32_t min0   = _mm_extract_epi16(minpos, 0);
    int32_t        xx     = x;

    if (min0 < *best_s) {
        if (min0 != 0xFFFF) { // no overflow
            *best_s = min0;
            *best_x = xx + _mm_extract_epi16(minpos, 1);
            *best_y = y;
        } else { // overflow
            const int32_t num = xx + ((leftover < 4) ? leftover : 4);
            __m128i       sads[2];

            add16x8x2to32bit(sums256, sads);

            do {
                UPDATE_BEST(sads[0], 0, xx, *best_s, *best_x, *best_y);
                sads[0] = _mm_srli_si128(sads[0], 4);
            } while (++xx < num);

            while (xx < search_area_width) {
                UPDATE_BEST(sads[1], 0, xx, *best_s, *best_x, *best_y);
                sads[1] = _mm_srli_si128(sads[1], 4);
                xx++;
            }
        }
    }
}

SIMD_INLINE void update_leftover8_1536_pel(const __m256i sums256[3], const int32_t x, const int32_t y,
                                           uint32_t* const best_s, int32_t* const best_x, int32_t* const best_y) {
    const __m256i  sum01  = _mm256_adds_epu16(sums256[0], sums256[1]);
    const __m256i  sum256 = _mm256_adds_epu16(sum01, sums256[2]);
    const __m128i  sum_lo = _mm256_castsi256_si128(sum256);
    const __m128i  sum_hi = _mm256_extracti128_si256(sum256, 1);
    const __m128i  sad    = _mm_adds_epu16(sum_lo, sum_hi);
    const __m128i  minpos = _mm_minpos_epu16(sad);
    const uint32_t min0   = _mm_extract_epi16(minpos, 0);

    if (min0 < *best_s) {
        if (min0 != 0xFFFF) { // no overflow
            *best_s = min0;
            *best_x = x + _mm_extract_epi16(minpos, 1);
            *best_y = y;
        } else { // overflow
            __m128i sads[2];
            add16x8x3to32bit(sums256, sads);
            update_8_best(sads, x, y, best_s, best_x, best_y);
        }
    }
}

SIMD_INLINE void update_leftover_1536_pel(const __m256i sums256[3], const int16_t search_area_width, const int32_t x,
                                          const int32_t y, const uint32_t leftover, const __m128i mask,
                                          uint32_t* const best_s, int32_t* const best_x, int32_t* const best_y) {
    const __m256i  sum01  = _mm256_adds_epu16(sums256[0], sums256[1]);
    const __m256i  sum256 = _mm256_adds_epu16(sum01, sums256[2]);
    const __m128i  sum_lo = _mm256_castsi256_si128(sum256);
    const __m128i  sum_hi = _mm256_extracti128_si256(sum256, 1);
    const __m128i  sad0   = _mm_adds_epu16(sum_lo, sum_hi);
    const __m128i  sad1   = _mm_or_si128(sad0, mask);
    const __m128i  minpos = _mm_minpos_epu16(sad1);
    const uint32_t min0   = _mm_extract_epi16(minpos, 0);
    int32_t        xx     = x;

    if (min0 < *best_s) {
        if (min0 != 0xFFFF) { // no overflow
            *best_s = min0;
            *best_x = xx + _mm_extract_epi16(minpos, 1);
            *best_y = y;
        } else { // overflow
            const int32_t num = xx + ((leftover < 4) ? leftover : 4);
            __m128i       sads[2];

            add16x8x3to32bit(sums256, sads);

            do {
                UPDATE_BEST(sads[0], 0, xx, *best_s, *best_x, *best_y);
                sads[0] = _mm_srli_si128(sads[0], 4);
            } while (++xx < num);

            while (xx < search_area_width) {
                UPDATE_BEST(sads[1], 0, xx, *best_s, *best_x, *best_y);
                sads[1] = _mm_srli_si128(sads[1], 4);
                xx++;
            }
        }
    }
}

SIMD_INLINE void update_leftover8_2048_pel(const __m256i sums256[4], const int32_t x, const int32_t y,
                                           uint32_t* const best_s, int32_t* const best_x, int32_t* const best_y) {
    const __m256i  sum01  = _mm256_adds_epu16(sums256[0], sums256[1]);
    const __m256i  sum23  = _mm256_adds_epu16(sums256[2], sums256[3]);
    const __m256i  sum256 = _mm256_adds_epu16(sum01, sum23);
    const __m128i  sum_lo = _mm256_castsi256_si128(sum256);
    const __m128i  sum_hi = _mm256_extracti128_si256(sum256, 1);
    const __m128i  sad    = _mm_adds_epu16(sum_lo, sum_hi);
    const __m128i  minpos = _mm_minpos_epu16(sad);
    const uint32_t min0   = _mm_extract_epi16(minpos, 0);

    if (min0 < *best_s) {
        if (min0 != 0xFFFF) { // no overflow
            *best_s = min0;
            *best_x = x + _mm_extract_epi16(minpos, 1);
            *best_y = y;
        } else { // overflow
            __m128i sads[2];
            add16x8x4to32bit(sums256, sads);
            update_8_best(sads, x, y, best_s, best_x, best_y);
        }
    }
}

SIMD_INLINE void update_leftover_2048_pel(const __m256i sums256[4], const int16_t search_area_width, const int32_t x,
                                          const int32_t y, const uint32_t leftover, const __m128i mask,
                                          uint32_t* const best_s, int32_t* const best_x, int32_t* const best_y) {
    const __m256i  sum01  = _mm256_adds_epu16(sums256[0], sums256[1]);
    const __m256i  sum23  = _mm256_adds_epu16(sums256[2], sums256[3]);
    const __m256i  sum256 = _mm256_adds_epu16(sum01, sum23);
    const __m128i  sum_lo = _mm256_castsi256_si128(sum256);
    const __m128i  sum_hi = _mm256_extracti128_si256(sum256, 1);
    const __m128i  sad0   = _mm_adds_epu16(sum_lo, sum_hi);
    const __m128i  sad1   = _mm_or_si128(sad0, mask);
    const __m128i  minpos = _mm_minpos_epu16(sad1);
    const uint32_t min0   = _mm_extract_epi16(minpos, 0);
    int32_t        xx     = x;

    if (min0 < *best_s) {
        if (min0 != 0xFFFF) { // no overflow
            *best_s = min0;
            *best_x = xx + _mm_extract_epi16(minpos, 1);
            *best_y = y;
        } else { // overflow
            const int32_t num = xx + ((leftover < 4) ? leftover : 4);
            __m128i       sads[2];

            add16x8x4to32bit(sums256, sads);

            do {
                UPDATE_BEST(sads[0], 0, xx, *best_s, *best_x, *best_y);
                sads[0] = _mm_srli_si128(sads[0], 4);
            } while (++xx < num);

            while (xx < search_area_width) {
                UPDATE_BEST(sads[1], 0, xx, *best_s, *best_x, *best_y);
                sads[1] = _mm_srli_si128(sads[1], 4);
                xx++;
            }
        }
    }
}

/*******************************************************************************
* Requirement: search_size <= 8,
* Returns "search_size" of SAD's for all height in 5th and 6th col
*******************************************************************************/
static INLINE __m128i complement_4_to_6(uint8_t* ref, uint32_t ref_stride, uint8_t* src, uint32_t src_stride,
                                        uint32_t height, uint32_t search_size) {
    __m128i sum;
    DECLARE_ALIGNED(16, uint16_t, tsum[8]);
    memset(tsum, 0, 8 * sizeof(uint16_t));
    for (uint32_t search_area = 0; search_area < search_size; search_area++) {
        for (uint32_t y = 0; y < height; y++) {
            tsum[search_area] += EB_ABS_DIFF(src[y * src_stride + 4], ref[y * ref_stride + 4]) +
                EB_ABS_DIFF(src[y * src_stride + 5], ref[y * ref_stride + 5]);
        }
        ref += 1;
    }
    sum = _mm_lddqu_si128((__m128i*)tsum);
    return sum;
}

/*******************************************************************************
 * Requirement: block_height < 64
 * General version for SAD computing that support any block width and height
*******************************************************************************/
void sad_loop_kernel_generalized_avx512(uint8_t*  src, // input parameter, source samples Ptr
                                        uint32_t  src_stride, // input parameter, source stride
                                        uint8_t*  ref, // input parameter, reference samples Ptr
                                        uint32_t  ref_stride, // input parameter, reference stride
                                        uint32_t  block_height, // input parameter, block height (M)
                                        uint32_t  block_width, // input parameter, block width (N)
                                        uint64_t* best_sad, int16_t* x_search_center, int16_t* y_search_center,
                                        uint32_t src_stride_raw, // input parameter, source stride (no line skipping)
                                        int16_t search_area_width, int16_t search_area_height) {
    int16_t        i, j;
    uint32_t       k, l;
    const uint8_t *p_ref, *p_src;
    uint32_t       low_sum = 0xffffff;
    int32_t        x_best = *x_search_center, y_best = *y_search_center;
    uint32_t       leftover = search_area_width & 15;

    __m128i leftover_mask    = _mm_set1_epi32(-1);
    __m128i leftover_mask32b = _mm_set1_epi32(-1);
    if (leftover) {
        for (k = 0; k < (uint32_t)(search_area_width & 7); k++) {
            leftover_mask = _mm_slli_si128(leftover_mask, 2);
        }
        for (k = 0; k < (uint32_t)(search_area_width & 3); k++) {
            leftover_mask32b = _mm_slli_si128(leftover_mask32b, 4);
        }
    }

    for (i = 0; i < search_area_height; i++) {
        for (j = 0; j < search_area_width; j += 16) {
            p_src = src;
            p_ref = ref + j;

            __m512i sums512[8] = {_mm512_setzero_si512(),
                                  _mm512_setzero_si512(),
                                  _mm512_setzero_si512(),
                                  _mm512_setzero_si512(),
                                  _mm512_setzero_si512(),
                                  _mm512_setzero_si512(),
                                  _mm512_setzero_si512(),
                                  _mm512_setzero_si512()};
            __m256i sum256_1   = _mm256_setzero_si256();
            __m256i sum256_2   = _mm256_setzero_si256();
            __m256i sum256_3   = _mm256_setzero_si256();
            __m256i sum256_4   = _mm256_setzero_si256();
            __m256i sum256     = _mm256_setzero_si256();
            for (k = 0; k + 2 <= block_height; k += 2) {
                uint32_t       width_calc = block_width;
                const uint8_t* temp_src   = p_src;
                const uint8_t* temp_ref   = p_ref;
                if (width_calc >= 32) {
                    sad_loop_kernel_32_4sum_avx512(temp_src, temp_ref, &sums512[0]);
                    sad_loop_kernel_32_4sum_avx512(temp_src + src_stride, temp_ref + ref_stride, &sums512[4]);

                    width_calc -= 32;
                    temp_src += 32;
                    temp_ref += 32;
                }
                if (width_calc >= 16) {
                    sad_loop_kernel_16_2sum_avx512(temp_src, src_stride, temp_ref, ref_stride, &sums512[0]);

                    width_calc -= 16;
                    temp_src += 16;
                    temp_ref += 16;
                }
                if (width_calc >= 8) {
                    sad_loop_kernel_8_avx2(temp_src, src_stride, temp_ref, ref_stride, &sum256_1);
                    sad_loop_kernel_8_avx2(temp_src, src_stride, temp_ref + 8, ref_stride, &sum256_2);

                    width_calc -= 8;
                    temp_src += 8;
                    temp_ref += 8;
                }
                if (width_calc >= 4) {
                    sad_loop_kernel_4_avx2(temp_src, src_stride, temp_ref, ref_stride, &sum256_3);
                    sad_loop_kernel_4_avx2(temp_src, src_stride, temp_ref + 8, ref_stride, &sum256_4);

                    width_calc -= 4;
                    temp_src += 4;
                    temp_ref += 4;
                }
                if (width_calc > 0) {
                    DECLARE_ALIGNED(16, uint16_t, tsum[16]);
                    memset(tsum, 0, 16 * sizeof(uint16_t));
                    for (uint32_t search_area = 0; search_area < 16; search_area++) {
                        for (l = 0; l < width_calc; l++) {
                            tsum[search_area] += EB_ABS_DIFF(temp_src[l], temp_ref[l]) +
                                EB_ABS_DIFF(temp_src[src_stride + l], temp_ref[ref_stride + l]);
                        }
                        temp_ref += 1;
                    }
                    sum256 = _mm256_adds_epu16(sum256, _mm256_loadu_si256((__m256i*)tsum));
                }
                p_src += 2 * src_stride;
                p_ref += 2 * ref_stride;
            }
            //when height is not multiple of 2,then compute last line
            if (k < block_height) {
                uint32_t       width_calc = block_width;
                const uint8_t* temp_src   = p_src;
                const uint8_t* temp_ref   = p_ref;
                if (width_calc >= 32) {
                    sad_loop_kernel_32_4sum_avx512(temp_src, temp_ref, &sums512[0]);

                    width_calc -= 32;
                    temp_src += 32;
                    temp_ref += 32;
                }
                if (width_calc >= 16) {
                    sad_loop_kernel_16_2sum_oneline_avx512(temp_src, temp_ref, &sums512[4]);

                    width_calc -= 16;
                    temp_src += 16;
                    temp_ref += 16;
                }
                if (width_calc >= 8) {
                    sad_loop_kernel_8_oneline_avx2(temp_src, temp_ref, &sum256_1);
                    sad_loop_kernel_8_oneline_avx2(temp_src, temp_ref + 8, &sum256_2);

                    width_calc -= 8;
                    temp_src += 8;
                    temp_ref += 8;
                }
                if (width_calc >= 4) {
                    sad_loop_kernel_4_oneline_avx2(temp_src, temp_ref, &sum256_3);
                    sad_loop_kernel_4_oneline_avx2(temp_src, temp_ref + 8, &sum256_4);

                    width_calc -= 4;
                    temp_src += 4;
                    temp_ref += 4;
                }
                if (width_calc > 0) {
                    DECLARE_ALIGNED(16, uint16_t, tsum[16]);
                    memset(tsum, 0, 16 * sizeof(uint16_t));
                    for (uint32_t search_area = 0; search_area < 16; search_area++) {
                        for (l = 0; l < width_calc; l++) {
                            tsum[search_area] += EB_ABS_DIFF(temp_src[l], temp_ref[l]);
                        }
                        temp_ref += 1;
                    }
                    sum256 = _mm256_adds_epu16(sum256, _mm256_loadu_si256((__m256i*)tsum));
                }
                p_src += src_stride;
                p_ref += ref_stride;
            }

            //update all
            __m512i sum512 = _mm512_inserti64x4(_mm512_castsi256_si512(sum256_1), sum256_2, 0x1);
            sum512         = _mm512_permutexvar_epi64(_mm512_setr_epi64(0, 1, 4, 5, 2, 3, 6, 7), sum512);
            sums512[5]     = _mm512_adds_epu16(sums512[5], sum512);

            sum512     = _mm512_inserti64x4(_mm512_castsi256_si512(sum256_3), sum256_4, 0x1);
            sum512     = _mm512_permutexvar_epi64(_mm512_setr_epi64(0, 1, 4, 5, 2, 3, 6, 7), sum512);
            sums512[6] = _mm512_adds_epu16(sums512[6], sum512);

            sums512[7] = _mm512_adds_epu16(sums512[7], _mm512_inserti64x4(_mm512_setzero_si512(), sum256, 0));

            const __m512i sum512_01   = _mm512_adds_epu16(sums512[0], sums512[1]);
            const __m512i sum512_23   = _mm512_adds_epu16(sums512[2], sums512[3]);
            const __m512i sum512_45   = _mm512_adds_epu16(sums512[4], sums512[5]);
            const __m512i sum512_67   = _mm512_adds_epu16(sums512[6], sums512[7]);
            const __m512i sum512_0123 = _mm512_adds_epu16(sum512_01, sum512_23);
            const __m512i sum512_4567 = _mm512_adds_epu16(sum512_45, sum512_67);
            sum512                    = _mm512_adds_epu16(sum512_0123, sum512_4567);
            const __m256i sum_lo      = _mm512_castsi512_si256(sum512);
            const __m256i sum_hi      = _mm512_extracti64x4_epi64(sum512, 1);
            const __m256i sad         = _mm256_adds_epu16(sum_lo, sum_hi);
            __m128i       sad_lo      = _mm256_castsi256_si128(sad);
            __m128i       sad_hi      = _mm256_extracti128_si256(sad, 1);
            if (leftover && (j + 16) >= search_area_width) {
                if (leftover < 8) {
                    sad_lo = _mm_or_si128(sad_lo, leftover_mask);
                    sad_hi = _mm_set1_epi32(-1);
                } else {
                    sad_hi = _mm_or_si128(sad_hi, leftover_mask);
                }
            }
            const __m128i  minpos_lo = _mm_minpos_epu16(sad_lo);
            const __m128i  minpos_hi = _mm_minpos_epu16(sad_hi);
            const uint32_t min0      = _mm_extract_epi16(minpos_lo, 0);
            const uint32_t min1      = _mm_extract_epi16(minpos_hi, 0);
            uint32_t       minmin, delta;
            __m128i        minpos;

            if (min0 <= min1) {
                minmin = min0;
                delta  = 0;
                minpos = minpos_lo;
            } else {
                minmin = min1;
                delta  = 8;
                minpos = minpos_hi;
            }

            if (minmin < low_sum) {
                if (minmin != 0xFFFF) { // no overflow
                    low_sum = minmin;
                    x_best  = j + delta + _mm_extract_epi16(minpos, 1);
                    y_best  = i;
                } else { // overflow
                    __m256i sads256[2];
                    __m128i sads128[4];

                    add16x16x8to32bit(sums512, sads256);

                    sads128[0] = _mm256_castsi256_si128(sads256[0]);
                    sads128[1] = _mm256_extracti128_si256(sads256[0], 1);
                    sads128[2] = _mm256_castsi256_si128(sads256[1]);
                    sads128[3] = _mm256_extracti128_si256(sads256[1], 1);
                    if (leftover && (j + 16) >= search_area_width) {
                        if (leftover < 4) {
                            sads128[0] = _mm_or_si128(sads128[0], leftover_mask32b);
                            sads128[1] = sads128[2] = sads128[3] = _mm_set1_epi32(-1);
                        } else if (leftover < 8) {
                            sads128[1] = _mm_or_si128(sads128[1], leftover_mask32b);
                            sads128[2] = sads128[3] = _mm_set1_epi32(-1);
                        } else if (leftover < 12) {
                            sads128[2] = _mm_or_si128(sads128[2], leftover_mask32b);
                            sads128[3] = _mm_set1_epi32(-1);
                        } else {
                            sads128[3] = _mm_or_si128(sads128[3], leftover_mask32b);
                        }
                    }

                    update_8_best(&sads128[0], j + 0, i, &low_sum, &x_best, &y_best);
                    update_8_best(&sads128[2], j + 8, i, &low_sum, &x_best, &y_best);
                }
            }
        }
        ref += src_stride_raw;
    }

    *best_sad        = low_sum;
    *x_search_center = (int16_t)x_best;
    *y_search_center = (int16_t)y_best;
}

/*******************************************************************************
* Requirement: width   = 4, 6, 8, 12, 16, 24, 32, 48 or 64 to use SIMD
* otherwise general/slower SIMD verison is used
* Requirement: height <= 64
* Requirement: height % 2 = 0
*******************************************************************************/
void svt_sad_loop_kernel_avx512_intrin(uint8_t*  src, // input parameter, source samples Ptr
                                       uint32_t  src_stride, // input parameter, source stride
                                       uint8_t*  ref, // input parameter, reference samples Ptr
                                       uint32_t  ref_stride, // input parameter, reference stride
                                       uint32_t  height, // input parameter, block height (M)
                                       uint32_t  width, // input parameter, block width (N)
                                       uint64_t* best_sad, int16_t* x_search_center, int16_t* y_search_center,
                                       uint32_t src_stride_raw, // input parameter, source stride (no line skipping)
                                       uint8_t skip_search_line, int16_t search_area_width,
                                       int16_t search_area_height) {
    const uint32_t height2 = height >> 1;
    const uint8_t *s, *r;
    int32_t        best_x = *x_search_center, best_y = *y_search_center;
    uint32_t       best_s = 0xffffff;
    int32_t        x, y;
    uint32_t       h;

    if (search_area_width == 8) {
        switch (width) {
        case 4:
            if (height <= 4) {
                y = 0;
                do {
                    __m128i sum = _mm_setzero_si128();

                    s = src;
                    r = ref;

                    h = height;
                    while (h >= 2) {
                        sad_loop_kernel_4_sse4_1(s, src_stride, r, ref_stride, &sum);
                        s += 2 * src_stride;
                        r += 2 * ref_stride;
                        h -= 2;
                    };

                    if (h) {
                        sad_loop_kernel_4_oneline_sse4_1(s, r, &sum);
                    }

                    update_best(sum, 0, y, &best_s, &best_x, &best_y);
                    ref += src_stride_raw;
                } while (++y < search_area_height);
            } else {
                y = 0;
                do {
                    __m256i sum256 = _mm256_setzero_si256();

                    s = src;
                    r = ref;

                    h = height;
                    while (h >= 2) {
                        sad_loop_kernel_4_avx2(s, src_stride, r, ref_stride, &sum256);
                        s += 2 * src_stride;
                        r += 2 * ref_stride;
                        h -= 2;
                    };

                    if (h) {
                        sad_loop_kernel_4_oneline_avx2(s, r, &sum256);
                    }

                    update_small_pel(sum256, 0, y, &best_s, &best_x, &best_y);
                    ref += src_stride_raw;
                } while (++y < search_area_height);
            }
            break;

        case 6:
            if (height <= 4) {
                y = 0;
                do {
                    __m128i sum = _mm_setzero_si128();

                    s = src;
                    r = ref;

                    h = height;
                    while (h >= 2) {
                        sad_loop_kernel_4_sse4_1(s, src_stride, r, ref_stride, &sum);
                        s += 2 * src_stride;
                        r += 2 * ref_stride;
                        h -= 2;
                    };

                    if (h) {
                        sad_loop_kernel_4_oneline_sse4_1(s, r, &sum);
                    }

                    __m128i sum2 = complement_4_to_6(ref, ref_stride, src, src_stride, height, 8);
                    sum          = _mm_adds_epu16(sum, sum2);

                    update_best(sum, 0, y, &best_s, &best_x, &best_y);
                    ref += src_stride_raw;
                } while (++y < search_area_height);
            } else {
                y = 0;
                do {
                    __m256i sum256 = _mm256_setzero_si256();

                    s = src;
                    r = ref;

                    h = height;
                    while (h >= 2) {
                        sad_loop_kernel_4_avx2(s, src_stride, r, ref_stride, &sum256);
                        s += 2 * src_stride;
                        r += 2 * ref_stride;
                        h -= 2;
                    };

                    if (h) {
                        sad_loop_kernel_4_oneline_avx2(s, r, &sum256);
                    }

                    __m128i sum = complement_4_to_6(ref, ref_stride, src, src_stride, height, 8);
                    sum256      = _mm256_adds_epu16(sum256, _mm256_insertf128_si256(_mm256_setzero_si256(), sum, 0));

                    update_small_pel(sum256, 0, y, &best_s, &best_x, &best_y);
                    ref += src_stride_raw;
                } while (++y < search_area_height);
            }
            break;

        case 8:
            // Note: Tried _mm512_dbsad_epu8 but is even slower.
            y = 0;
            do {
                __m256i sum256 = _mm256_setzero_si256();

                s = src;
                r = ref;

                h = height;
                while (h >= 2) {
                    sad_loop_kernel_8_avx2(s, src_stride, r, ref_stride, &sum256);
                    s += 2 * src_stride;
                    r += 2 * ref_stride;
                    h -= 2;
                };

                if (h) {
                    sad_loop_kernel_8_oneline_avx2(s, r, &sum256);
                };

                update_small_pel(sum256, 0, y, &best_s, &best_x, &best_y);
                ref += src_stride_raw;
            } while (++y < search_area_height);
            break;

        case 12:
            if (height <= 16) {
                y = 0;
                do {
                    __m256i sum256 = _mm256_setzero_si256();

                    s = src;
                    r = ref;

                    h = height;
                    while (h >= 2) {
                        sad_loop_kernel_12_avx2(s, src_stride, r, ref_stride, &sum256);
                        s += 2 * src_stride;
                        r += 2 * ref_stride;
                        h -= 2;
                    };

                    if (h) {
                        sad_loop_kernel_12_oneline_avx2(s, r, &sum256);
                    }

                    update_small_pel(sum256, 0, y, &best_s, &best_x, &best_y);
                    ref += src_stride_raw;
                } while (++y < search_area_height);
            } else if (height <= 32) {
                y = 0;
                do {
                    __m256i sum256 = _mm256_setzero_si256();

                    s = src;
                    r = ref;

                    h = height;
                    while (h >= 2) {
                        sad_loop_kernel_12_avx2(s, src_stride, r, ref_stride, &sum256);
                        s += 2 * src_stride;
                        r += 2 * ref_stride;
                        h -= 2;
                    };

                    if (h) {
                        sad_loop_kernel_12_oneline_avx2(s, r, &sum256);
                    }

                    update_some_pel(sum256, 0, y, &best_s, &best_x, &best_y);
                    ref += src_stride_raw;
                } while (++y < search_area_height);
            } else {
                y = 0;
                do {
                    __m256i sums256[2] = {_mm256_setzero_si256(), _mm256_setzero_si256()};

                    s = src;
                    r = ref;

                    h = height;
                    while (h >= 2) {
                        sad_loop_kernel_12_2sum_avx2(s, src_stride, r, ref_stride, sums256);
                        s += 2 * src_stride;
                        r += 2 * ref_stride;
                        h -= 2;
                    };

                    if (h) {
                        sad_loop_kernel_12_2sum_oneline_avx2(s, r, sums256);
                    }

                    update_leftover8_1024_pel(sums256, 0, y, &best_s, &best_x, &best_y);
                    ref += src_stride_raw;
                } while (++y < search_area_height);
            }
            break;

        case 16:
            if (height <= 16) {
                y = 0;
                do {
                    if (skip_search_line) {
                        if ((y & 1) == 0) {
                            ref += src_stride_raw;
                            continue;
                        }
                    }
                    __m256i sum256 = _mm256_setzero_si256();

                    s = src;
                    r = ref;

                    h = height;
                    while (h >= 2) {
                        sad_loop_kernel_16_avx2(s, src_stride, r, ref_stride, &sum256);
                        s += 2 * src_stride;
                        r += 2 * ref_stride;
                        h -= 2;
                    };

                    if (h) {
                        sad_loop_kernel_16_oneline_avx2(s, r, &sum256);
                    };

                    update_small_pel(sum256, 0, y, &best_s, &best_x, &best_y);
                    ref += src_stride_raw;
                } while (++y < search_area_height);
            } else if (height <= 32) {
                y = 0;
                do {
                    __m256i sum256 = _mm256_setzero_si256();

                    s = src;
                    r = ref;

                    h = height;
                    while (h >= 2) {
                        sad_loop_kernel_16_avx2(s, src_stride, r, ref_stride, &sum256);
                        s += 2 * src_stride;
                        r += 2 * ref_stride;
                        h -= 2;
                    };

                    if (h) {
                        sad_loop_kernel_16_oneline_avx2(s, r, &sum256);
                    };

                    update_some_pel(sum256, 0, y, &best_s, &best_x, &best_y);
                    ref += src_stride_raw;
                } while (++y < search_area_height);
            } else {
                y = 0;
                do {
                    __m256i sums256[2] = {_mm256_setzero_si256(), _mm256_setzero_si256()};

                    s = src;
                    r = ref;

                    h = height;
                    while (h >= 2) {
                        sad_loop_kernel_16_2sum_avx2(s, src_stride, r, ref_stride, sums256);
                        s += 2 * src_stride;
                        r += 2 * ref_stride;
                        h -= 2;
                    };

                    if (h) {
                        sad_loop_kernel_16_2sum_oneline_avx2(s, r, sums256);
                    };

                    update_leftover8_1024_pel(sums256, 0, y, &best_s, &best_x, &best_y);
                    ref += src_stride_raw;
                } while (++y < search_area_height);
            }
            break;

        case 24:
            if (height <= 16) {
                y = 0;
                do {
                    __m256i sum256 = _mm256_setzero_si256();

                    s = src;
                    r = ref;

                    h = height;
                    while (h >= 2) {
                        sad_loop_kernel_16_avx2(s, src_stride, r, ref_stride, &sum256);
                        sad_loop_kernel_8_avx2(s + 16, src_stride, r + 16, ref_stride, &sum256);
                        s += 2 * src_stride;
                        r += 2 * ref_stride;
                        h -= 2;
                    };

                    if (h) {
                        sad_loop_kernel_16_oneline_avx2(s, r, &sum256);
                        sad_loop_kernel_8_oneline_avx2(s + 16, r + 16, &sum256);
                    }

                    update_some_pel(sum256, 0, y, &best_s, &best_x, &best_y);
                    ref += src_stride_raw;
                } while (++y < search_area_height);
            } else {
                y = 0;
                do {
                    __m256i sums256[2] = {_mm256_setzero_si256(), _mm256_setzero_si256()};

                    s = src;
                    r = ref;

                    h = height;
                    while (h >= 2) {
                        sad_loop_kernel_16_avx2(s, src_stride, r, ref_stride, &sums256[0]);
                        sad_loop_kernel_8_avx2(s + 16, src_stride, r + 16, ref_stride, &sums256[1]);
                        s += 2 * src_stride;
                        r += 2 * ref_stride;
                        h -= 2;
                    };

                    if (h) {
                        sad_loop_kernel_16_oneline_avx2(s, r, &sums256[0]);
                        sad_loop_kernel_8_oneline_avx2(s + 16, r + 16, &sums256[1]);
                    }

                    update_leftover8_1024_pel(sums256, 0, y, &best_s, &best_x, &best_y);
                    ref += src_stride_raw;
                } while (++y < search_area_height);
            }
            break;

        case 32:
            if (height <= 8) {
                y = 0;
                do {
                    __m256i sum256 = _mm256_setzero_si256();

                    s = src;
                    r = ref;

                    h = height;
                    do {
                        sad_loop_kernel_32_avx2(s, r, &sum256);
                        s += src_stride;
                        r += ref_stride;
                    } while (--h);

                    update_small_pel(sum256, 0, y, &best_s, &best_x, &best_y);
                    ref += src_stride_raw;
                } while (++y < search_area_height);
            } else if (height <= 16) {
                y = 0;
                do {
                    __m256i sum256 = _mm256_setzero_si256();

                    s = src;
                    r = ref;

                    h = height;
                    do {
                        sad_loop_kernel_32_avx2(s, r, &sum256);
                        s += src_stride;
                        r += ref_stride;
                    } while (--h);

                    update_some_pel(sum256, 0, y, &best_s, &best_x, &best_y);
                    ref += src_stride_raw;
                } while (++y < search_area_height);
            } else if (height <= 32) {
                y = 0;
                do {
                    __m256i sums256[2] = {_mm256_setzero_si256(), _mm256_setzero_si256()};

                    s = src;
                    r = ref;

                    h = height;
                    do {
                        sad_loop_kernel_32_2sum_avx2(s, r, sums256);
                        s += src_stride;
                        r += ref_stride;
                    } while (--h);

                    update_leftover8_1024_pel(sums256, 0, y, &best_s, &best_x, &best_y);
                    ref += src_stride_raw;
                } while (++y < search_area_height);
            } else {
                y = 0;
                do {
                    __m256i sums256[4] = {
                        _mm256_setzero_si256(), _mm256_setzero_si256(), _mm256_setzero_si256(), _mm256_setzero_si256()};

                    s = src;
                    r = ref;

                    h = height;
                    do {
                        sad_loop_kernel_32_4sum_avx2(s, r, sums256);
                        s += src_stride;
                        r += ref_stride;
                    } while (--h);

                    update_leftover8_2048_pel(sums256, 0, y, &best_s, &best_x, &best_y);
                    ref += src_stride_raw;
                } while (++y < search_area_height);
            }
            break;

        case 48:
            if (height <= 32) {
                y = 0;
                do {
                    __m256i sums256[3] = {_mm256_setzero_si256(), _mm256_setzero_si256(), _mm256_setzero_si256()};

                    s = src;
                    r = ref;

                    h = height2;
                    do {
                        sad_loop_kernel_32_2sum_avx2(s, r, sums256);
                        sad_loop_kernel_32_2sum_avx2(s + src_stride, r + ref_stride, sums256);
                        sad_loop_kernel_16_avx2(s + 32, src_stride, r + 32, ref_stride, &sums256[2]);
                        s += 2 * src_stride;
                        r += 2 * ref_stride;
                    } while (--h);

                    update_leftover8_1536_pel(sums256, 0, y, &best_s, &best_x, &best_y);
                    ref += src_stride_raw;
                } while (++y < search_area_height);
            } else {
                y = 0;
                do {
                    __m256i sums256[6] = {_mm256_setzero_si256(),
                                          _mm256_setzero_si256(),
                                          _mm256_setzero_si256(),
                                          _mm256_setzero_si256(),
                                          _mm256_setzero_si256(),
                                          _mm256_setzero_si256()};

                    s = src;
                    r = ref;

                    h = height2;
                    do {
                        sad_loop_kernel_32_4sum_avx2(s, r, sums256);
                        sad_loop_kernel_32_4sum_avx2(s + src_stride, r + ref_stride, sums256);
                        sad_loop_kernel_16_2sum_avx2(s + 32, src_stride, r + 32, ref_stride, &sums256[4]);
                        s += 2 * src_stride;
                        r += 2 * ref_stride;
                    } while (--h);

                    const __m256i  sum01   = _mm256_adds_epu16(sums256[0], sums256[1]);
                    const __m256i  sum23   = _mm256_adds_epu16(sums256[2], sums256[3]);
                    const __m256i  sum45   = _mm256_adds_epu16(sums256[4], sums256[5]);
                    const __m256i  sum0123 = _mm256_adds_epu16(sum01, sum23);
                    const __m256i  sum256  = _mm256_adds_epu16(sum0123, sum45);
                    const __m128i  sum_lo  = _mm256_castsi256_si128(sum256);
                    const __m128i  sum_hi  = _mm256_extracti128_si256(sum256, 1);
                    const __m128i  sad     = _mm_adds_epu16(sum_lo, sum_hi);
                    const __m128i  minpos  = _mm_minpos_epu16(sad);
                    const uint32_t min0    = _mm_extract_epi16(minpos, 0);

                    if (min0 < best_s) {
                        if (min0 != 0xFFFF) { // no overflow
                            best_s = min0;
                            best_x = _mm_extract_epi16(minpos, 1);
                            best_y = y;
                        } else { // overflow
                            __m128i sads[2];

                            add16x8x6to32bit(sums256, sads);
                            update_8_best(sads, 0, y, &best_s, &best_x, &best_y);
                        }
                    }

                    ref += src_stride_raw;
                } while (++y < search_area_height);
            }
            break;

        case 64:
            if (height <= 16) {
                y = 0;
                do {
                    __m256i sums256[2] = {_mm256_setzero_si256(), _mm256_setzero_si256()};

                    s = src;
                    r = ref;

                    h = height;
                    do {
                        sad_loop_kernel_64_2sum_avx2(s, r, sums256);
                        s += src_stride;
                        r += ref_stride;
                    } while (--h);

                    update_leftover8_1024_pel(sums256, 0, y, &best_s, &best_x, &best_y);
                    ref += src_stride_raw;
                } while (++y < search_area_height);
            } else if (height <= 32) {
                y = 0;
                do {
                    __m256i sums256[4] = {
                        _mm256_setzero_si256(), _mm256_setzero_si256(), _mm256_setzero_si256(), _mm256_setzero_si256()};

                    s = src;
                    r = ref;

                    h = height;
                    do {
                        sad_loop_kernel_64_4sum_avx2(s, r, sums256);
                        s += src_stride;
                        r += ref_stride;
                    } while (--h);

                    update_leftover8_2048_pel(sums256, 0, y, &best_s, &best_x, &best_y);
                    ref += src_stride_raw;
                } while (++y < search_area_height);
            } else {
                y = 0;
                do {
                    __m256i sums256[8] = {_mm256_setzero_si256(),
                                          _mm256_setzero_si256(),
                                          _mm256_setzero_si256(),
                                          _mm256_setzero_si256(),
                                          _mm256_setzero_si256(),
                                          _mm256_setzero_si256(),
                                          _mm256_setzero_si256(),
                                          _mm256_setzero_si256()};

                    s = src;
                    r = ref;

                    h = height;
                    do {
                        sad_loop_kernel_64_8sum_avx2(s, r, sums256);
                        s += src_stride;
                        r += ref_stride;
                    } while (--h);

                    const __m256i  sum01   = _mm256_adds_epu16(sums256[0], sums256[1]);
                    const __m256i  sum23   = _mm256_adds_epu16(sums256[2], sums256[3]);
                    const __m256i  sum45   = _mm256_adds_epu16(sums256[4], sums256[5]);
                    const __m256i  sum67   = _mm256_adds_epu16(sums256[6], sums256[7]);
                    const __m256i  sum0123 = _mm256_adds_epu16(sum01, sum23);
                    const __m256i  sum4567 = _mm256_adds_epu16(sum45, sum67);
                    const __m256i  sum256  = _mm256_adds_epu16(sum0123, sum4567);
                    const __m128i  sum_lo  = _mm256_castsi256_si128(sum256);
                    const __m128i  sum_hi  = _mm256_extracti128_si256(sum256, 1);
                    const __m128i  sad     = _mm_adds_epu16(sum_lo, sum_hi);
                    const __m128i  minpos  = _mm_minpos_epu16(sad);
                    const uint32_t min0    = _mm_extract_epi16(minpos, 0);

                    if (min0 < best_s) {
                        if (min0 != 0xFFFF) { // no overflow
                            best_s = min0;
                            best_x = _mm_extract_epi16(minpos, 1);
                            best_y = y;
                        } else { // overflow
                            __m128i sads[2];

                            add16x8x8to32bit(sums256, sads);
                            update_8_best(sads, 0, y, &best_s, &best_x, &best_y);
                        }
                    }

                    ref += src_stride_raw;
                } while (++y < search_area_height);
            }
            break;

        default:
            sad_loop_kernel_generalized_avx512(src,
                                               src_stride,
                                               ref,
                                               ref_stride,
                                               height,
                                               width,
                                               best_sad,
                                               x_search_center,
                                               y_search_center,
                                               src_stride_raw,
                                               search_area_width,
                                               search_area_height);
            return;
        }
    } else if (search_area_width == 16) {
        switch (width) {
        case 4:
            if (height <= 4) {
                y = 0;
                do {
                    {
                        __m128i sum = _mm_setzero_si128();

                        s = src;
                        r = ref;

                        h = height;
                        while (h >= 2) {
                            sad_loop_kernel_4_sse4_1(s, src_stride, r, ref_stride, &sum);
                            s += 2 * src_stride;
                            r += 2 * ref_stride;
                            h -= 2;
                        };

                        if (h) {
                            sad_loop_kernel_4_oneline_sse4_1(s, r, &sum);
                        }

                        update_best(sum, 0, y, &best_s, &best_x, &best_y);
                    }

                    {
                        __m128i sum = _mm_setzero_si128();

                        s = src;
                        r = ref + 8;

                        h = height;
                        while (h >= 2) {
                            sad_loop_kernel_4_sse4_1(s, src_stride, r, ref_stride, &sum);
                            s += 2 * src_stride;
                            r += 2 * ref_stride;
                            h -= 2;
                        };

                        if (h) {
                            sad_loop_kernel_4_oneline_sse4_1(s, r, &sum);
                        }

                        update_best(sum, 8, y, &best_s, &best_x, &best_y);
                    }

                    ref += src_stride_raw;
                } while (++y < search_area_height);
            } else {
                y = 0;
                do {
                    {
                        __m256i sum256 = _mm256_setzero_si256();

                        s = src;
                        r = ref;

                        h = height;
                        while (h >= 2) {
                            sad_loop_kernel_4_avx2(s, src_stride, r, ref_stride, &sum256);
                            s += 2 * src_stride;
                            r += 2 * ref_stride;
                            h -= 2;
                        };

                        if (h) {
                            sad_loop_kernel_4_oneline_avx2(s, r, &sum256);
                        }

                        update_small_pel(sum256, 0, y, &best_s, &best_x, &best_y);
                    }

                    {
                        __m256i sum256 = _mm256_setzero_si256();

                        s = src;
                        r = ref + 8;

                        h = height;
                        while (h >= 2) {
                            sad_loop_kernel_4_avx2(s, src_stride, r, ref_stride, &sum256);
                            s += 2 * src_stride;
                            r += 2 * ref_stride;
                            h -= 2;
                        };

                        if (h) {
                            sad_loop_kernel_4_oneline_avx2(s, r, &sum256);
                        }

                        update_small_pel(sum256, 8, y, &best_s, &best_x, &best_y);
                    }

                    ref += src_stride_raw;
                } while (++y < search_area_height);
            }
            break;

        case 6:
            if (height <= 4) {
                y = 0;
                do {
                    {
                        __m128i sum = _mm_setzero_si128();

                        s = src;
                        r = ref;

                        h = height;
                        while (h >= 2) {
                            sad_loop_kernel_4_sse4_1(s, src_stride, r, ref_stride, &sum);
                            s += 2 * src_stride;
                            r += 2 * ref_stride;
                            h -= 2;
                        };

                        if (h) {
                            sad_loop_kernel_4_oneline_sse4_1(s, r, &sum);
                        }

                        __m128i sum2 = complement_4_to_6(ref, ref_stride, src, src_stride, height, 8);
                        sum          = _mm_adds_epu16(sum, sum2);

                        update_best(sum, 0, y, &best_s, &best_x, &best_y);
                    }

                    {
                        __m128i sum = _mm_setzero_si128();

                        s = src;
                        r = ref + 8;

                        h = height;
                        while (h >= 2) {
                            sad_loop_kernel_4_sse4_1(s, src_stride, r, ref_stride, &sum);
                            s += 2 * src_stride;
                            r += 2 * ref_stride;
                            h -= 2;
                        };

                        if (h) {
                            sad_loop_kernel_4_oneline_sse4_1(s, r, &sum);
                        }

                        __m128i sum2 = complement_4_to_6(ref + 8, ref_stride, src, src_stride, height, 8);
                        sum          = _mm_adds_epu16(sum, sum2);

                        update_best(sum, 8, y, &best_s, &best_x, &best_y);
                    }

                    ref += src_stride_raw;
                } while (++y < search_area_height);
            } else {
                y = 0;
                do {
                    {
                        __m256i sum256 = _mm256_setzero_si256();

                        s = src;
                        r = ref;

                        h = height;
                        while (h >= 2) {
                            sad_loop_kernel_4_avx2(s, src_stride, r, ref_stride, &sum256);
                            s += 2 * src_stride;
                            r += 2 * ref_stride;
                            h -= 2;
                        };

                        if (h) {
                            sad_loop_kernel_4_oneline_avx2(s, r, &sum256);
                        }

                        __m128i sum = complement_4_to_6(ref, ref_stride, src, src_stride, height, 8);
                        sum256 = _mm256_adds_epu16(sum256, _mm256_insertf128_si256(_mm256_setzero_si256(), sum, 0));

                        update_small_pel(sum256, 0, y, &best_s, &best_x, &best_y);
                    }

                    {
                        __m256i sum256 = _mm256_setzero_si256();

                        s = src;
                        r = ref + 8;

                        h = height;
                        while (h >= 2) {
                            sad_loop_kernel_4_avx2(s, src_stride, r, ref_stride, &sum256);
                            s += 2 * src_stride;
                            r += 2 * ref_stride;
                            h -= 2;
                        };

                        if (h) {
                            sad_loop_kernel_4_oneline_avx2(s, r, &sum256);
                        }

                        __m128i sum = complement_4_to_6(ref + 8, ref_stride, src, src_stride, height, 8);
                        sum256 = _mm256_adds_epu16(sum256, _mm256_insertf128_si256(_mm256_setzero_si256(), sum, 0));

                        update_small_pel(sum256, 8, y, &best_s, &best_x, &best_y);
                    }

                    ref += src_stride_raw;
                } while (++y < search_area_height);
            }
            break;

        case 8:
            // Note: Tried _mm512_dbsad_epu8 but is even slower.
            y = 0;
            do {
                {
                    __m256i sum256 = _mm256_setzero_si256();

                    s = src;
                    r = ref;

                    h = height;
                    while (h >= 2) {
                        sad_loop_kernel_8_avx2(s, src_stride, r, ref_stride, &sum256);
                        s += 2 * src_stride;
                        r += 2 * ref_stride;
                        h -= 2;
                    };

                    if (h) {
                        sad_loop_kernel_8_oneline_avx2(s, r, &sum256);
                    };

                    update_small_pel(sum256, 0, y, &best_s, &best_x, &best_y);
                }

                {
                    __m256i sum256 = _mm256_setzero_si256();

                    s = src;
                    r = ref + 8;

                    h = height;
                    while (h >= 2) {
                        sad_loop_kernel_8_avx2(s, src_stride, r, ref_stride, &sum256);
                        s += 2 * src_stride;
                        r += 2 * ref_stride;
                        h -= 2;
                    };

                    if (h) {
                        sad_loop_kernel_8_oneline_avx2(s, r, &sum256);
                    };

                    update_small_pel(sum256, 8, y, &best_s, &best_x, &best_y);
                }

                ref += src_stride_raw;
            } while (++y < search_area_height);
            break;

        case 12:
            if (height <= 16) {
                y = 0;
                do {
                    __m512i sum512 = _mm512_setzero_si512();

                    s = src;
                    r = ref;

                    h = height;
                    while (h >= 2) {
                        sad_loop_kernel_12_avx512(s, src_stride, r, ref_stride, &sum512);
                        s += 2 * src_stride;
                        r += 2 * ref_stride;
                        h -= 2;
                    };

                    if (h) {
                        sad_loop_kernel_12_oneline_avx512(s, r, &sum512);
                    }

                    update_256_pel(sum512, 0, y, &best_s, &best_x, &best_y);
                    ref += src_stride_raw;
                } while (++y < search_area_height);
            } else if (height <= 32) {
                y = 0;
                do {
                    __m512i sum512 = _mm512_setzero_si512();

                    s = src;
                    r = ref;

                    h = height;
                    while (h >= 2) {
                        sad_loop_kernel_12_avx512(s, src_stride, r, ref_stride, &sum512);
                        s += 2 * src_stride;
                        r += 2 * ref_stride;
                        h -= 2;
                    };

                    if (h) {
                        sad_loop_kernel_12_oneline_avx512(s, r, &sum512);
                    }

                    update_512_pel(sum512, 0, y, &best_s, &best_x, &best_y);
                    ref += src_stride_raw;
                } while (++y < search_area_height);
            } else {
                y = 0;
                do {
                    __m512i sums512[2] = {_mm512_setzero_si512(), _mm512_setzero_si512()};

                    s = src;
                    r = ref;

                    h = height;
                    while (h >= 2) {
                        sad_loop_kernel_12_2sum_avx512(s, src_stride, r, ref_stride, sums512);
                        s += 2 * src_stride;
                        r += 2 * ref_stride;
                        h -= 2;
                    };

                    if (h) {
                        sad_loop_kernel_12_2sum_oneline_avx512(s, r, sums512);
                    }

                    update_1024_pel(sums512, 0, y, &best_s, &best_x, &best_y);
                    ref += src_stride_raw;
                } while (++y < search_area_height);
            }
            break;

        case 16:
            if (height <= 16) {
                y = 0;
                do {
                    if (skip_search_line) {
                        if ((y & 1) == 0) {
                            ref += src_stride_raw;
                            continue;
                        }
                    }
                    __m512i sum512 = _mm512_setzero_si512();

                    s = src;
                    r = ref;

                    h = height;
                    while (h >= 2) {
                        sad_loop_kernel_16_avx512(s, src_stride, r, ref_stride, &sum512);
                        s += 2 * src_stride;
                        r += 2 * ref_stride;
                        h -= 2;
                    };

                    if (h) {
                        sad_loop_kernel_16_oneline_avx512(s, r, &sum512);
                    };

                    update_256_pel(sum512, 0, y, &best_s, &best_x, &best_y);
                    ref += src_stride_raw;
                } while (++y < search_area_height);
            } else if (height <= 32) {
                y = 0;
                do {
                    __m512i sum512 = _mm512_setzero_si512();

                    s = src;
                    r = ref;

                    h = height;
                    while (h >= 2) {
                        sad_loop_kernel_16_avx512(s, src_stride, r, ref_stride, &sum512);
                        s += 2 * src_stride;
                        r += 2 * ref_stride;
                        h -= 2;
                    };

                    if (h) {
                        sad_loop_kernel_16_oneline_avx512(s, r, &sum512);
                    };

                    update_512_pel(sum512, 0, y, &best_s, &best_x, &best_y);
                    ref += src_stride_raw;
                } while (++y < search_area_height);
            } else {
                y = 0;
                do {
                    __m512i sums512[2] = {_mm512_setzero_si512(), _mm512_setzero_si512()};

                    s = src;
                    r = ref;

                    h = height;
                    while (h >= 2) {
                        sad_loop_kernel_16_2sum_avx512(s, src_stride, r, ref_stride, sums512);
                        s += 2 * src_stride;
                        r += 2 * ref_stride;
                        h -= 2;
                    };

                    if (h) {
                        sad_loop_kernel_16_2sum_oneline_avx512(s, r, sums512);
                    };

                    update_1024_pel(sums512, 0, y, &best_s, &best_x, &best_y);
                    ref += src_stride_raw;
                } while (++y < search_area_height);
            }
            break;

        case 24:
            if (height <= 16) {
                y = 0;
                do {
                    __m512i sum512     = _mm512_setzero_si512();
                    __m256i sums256[2] = {_mm256_setzero_si256(), _mm256_setzero_si256()};

                    s = src;
                    r = ref;

                    h = height;
                    while (h >= 2) {
                        sad_loop_kernel_16_avx512(s, src_stride, r, ref_stride, &sum512);
                        sad_loop_kernel_8_avx2(s + 16, src_stride, r + 16, ref_stride, &sums256[0]);
                        sad_loop_kernel_8_avx2(s + 16, src_stride, r + 24, ref_stride, &sums256[1]);
                        s += 2 * src_stride;
                        r += 2 * ref_stride;
                        h -= 2;
                    };

                    if (h) {
                        sad_loop_kernel_16_oneline_avx512(s, r, &sum512);
                        sad_loop_kernel_8_oneline_avx2(s + 16, r + 16, &sums256[0]);
                        sad_loop_kernel_8_oneline_avx2(s + 16, r + 24, &sums256[1]);
                    }

                    update_384_pel(sum512, sums256, 0, y, &best_s, &best_x, &best_y);
                    ref += src_stride_raw;
                } while (++y < search_area_height);
            } else {
                y = 0;
                do {
                    __m512i sum512     = _mm512_setzero_si512();
                    __m256i sums256[2] = {_mm256_setzero_si256(), _mm256_setzero_si256()};

                    s = src;
                    r = ref;

                    h = height;
                    while (h >= 2) {
                        sad_loop_kernel_16_avx512(s, src_stride, r, ref_stride, &sum512);
                        sad_loop_kernel_8_avx2(s + 16, src_stride, r + 16, ref_stride, &sums256[0]);
                        sad_loop_kernel_8_avx2(s + 16, src_stride, r + 24, ref_stride, &sums256[1]);
                        s += 2 * src_stride;
                        r += 2 * ref_stride;
                        h -= 2;
                    };

                    if (h) {
                        sad_loop_kernel_16_oneline_avx512(s, r, &sum512);
                        sad_loop_kernel_8_oneline_avx2(s + 16, r + 16, &sums256[0]);
                        sad_loop_kernel_8_oneline_avx2(s + 16, r + 24, &sums256[1]);
                    }

                    update_768_pel(sum512, sums256, 0, y, &best_s, &best_x, &best_y);
                    ref += src_stride_raw;
                } while (++y < search_area_height);
            }
            break;

        case 32:
            if (height <= 16) {
                y = 0;
                do {
                    __m512i sum512 = _mm512_setzero_si512();

                    s = src;
                    r = ref;

                    // Note: faster than looping 2 rows.
                    h = height;
                    do {
                        sad_loop_kernel_32_avx512(s, r, &sum512);
                        s += src_stride;
                        r += ref_stride;
                    } while (--h);

                    update_512_pel(sum512, 0, y, &best_s, &best_x, &best_y);
                    ref += src_stride_raw;
                } while (++y < search_area_height);
            } else if (height <= 32) {
                y = 0;
                do {
                    __m512i sums512[2] = {_mm512_setzero_si512(), _mm512_setzero_si512()};

                    s = src;
                    r = ref;

                    h = height;
                    do {
                        sad_loop_kernel_32_2sum_avx512(s, r, sums512);
                        s += src_stride;
                        r += ref_stride;
                    } while (--h);

                    update_1024_pel(sums512, 0, y, &best_s, &best_x, &best_y);
                    ref += src_stride_raw;
                } while (++y < search_area_height);
            } else {
                y = 0;
                do {
                    __m512i sums512[4] = {
                        _mm512_setzero_si512(), _mm512_setzero_si512(), _mm512_setzero_si512(), _mm512_setzero_si512()};

                    s = src;
                    r = ref;

                    h = height;
                    do {
                        sad_loop_kernel_32_4sum_avx512(s, r, sums512);
                        s += src_stride;
                        r += ref_stride;
                    } while (--h);

                    update_2048_pel(sums512, 0, y, &best_s, &best_x, &best_y);
                    ref += src_stride_raw;
                } while (++y < search_area_height);
            }
            break;

        case 48:
            if (height <= 32) {
                y = 0;
                do {
                    __m512i sums512[3] = {_mm512_setzero_si512(), _mm512_setzero_si512(), _mm512_setzero_si512()};

                    s = src;
                    r = ref;

                    h = height2;
                    do {
                        sad_loop_kernel_32_2sum_avx512(s, r, sums512);
                        sad_loop_kernel_32_2sum_avx512(s + src_stride, r + ref_stride, sums512);
                        sad_loop_kernel_16_avx512(s + 32, src_stride, r + 32, ref_stride, &sums512[2]);
                        s += 2 * src_stride;
                        r += 2 * ref_stride;
                    } while (--h);

                    update_1536_pel(sums512, 0, y, &best_s, &best_x, &best_y);
                    ref += src_stride_raw;
                } while (++y < search_area_height);
            } else {
                y = 0;
                do {
                    __m512i sums512[6] = {_mm512_setzero_si512(),
                                          _mm512_setzero_si512(),
                                          _mm512_setzero_si512(),
                                          _mm512_setzero_si512(),
                                          _mm512_setzero_si512(),
                                          _mm512_setzero_si512()};

                    s = src;
                    r = ref;

                    h = height2;
                    do {
                        sad_loop_kernel_32_4sum_avx512(s, r, sums512);
                        sad_loop_kernel_32_4sum_avx512(s + src_stride, r + ref_stride, sums512);
                        sad_loop_kernel_16_2sum_avx512(s + 32, src_stride, r + 32, ref_stride, &sums512[4]);
                        s += 2 * src_stride;
                        r += 2 * ref_stride;
                    } while (--h);

                    const __m512i  sum512_01   = _mm512_adds_epu16(sums512[0], sums512[1]);
                    const __m512i  sum512_23   = _mm512_adds_epu16(sums512[2], sums512[3]);
                    const __m512i  sum512_45   = _mm512_adds_epu16(sums512[4], sums512[5]);
                    const __m512i  sum512_0123 = _mm512_adds_epu16(sum512_01, sum512_23);
                    const __m512i  sum512      = _mm512_adds_epu16(sum512_0123, sum512_45);
                    const __m256i  sum_lo      = _mm512_castsi512_si256(sum512);
                    const __m256i  sum_hi      = _mm512_extracti64x4_epi64(sum512, 1);
                    const __m256i  sad         = _mm256_adds_epu16(sum_lo, sum_hi);
                    const __m128i  sad_lo      = _mm256_castsi256_si128(sad);
                    const __m128i  sad_hi      = _mm256_extracti128_si256(sad, 1);
                    const __m128i  minpos_lo   = _mm_minpos_epu16(sad_lo);
                    const __m128i  minpos_hi   = _mm_minpos_epu16(sad_hi);
                    const uint32_t min0        = _mm_extract_epi16(minpos_lo, 0);
                    const uint32_t min1        = _mm_extract_epi16(minpos_hi, 0);
                    uint32_t       minmin, delta;
                    __m128i        minpos;

                    if (min0 <= min1) {
                        minmin = min0;
                        delta  = 0;
                        minpos = minpos_lo;
                    } else {
                        minmin = min1;
                        delta  = 8;
                        minpos = minpos_hi;
                    }

                    if (minmin < best_s) {
                        if (minmin != 0xFFFF) { // no overflow
                            best_s = minmin;
                            best_x = delta + _mm_extract_epi16(minpos, 1);
                            best_y = y;
                        } else { // overflow
                            __m256i sads256[2];
                            __m128i sads128[2];

                            add16x16x6to32bit(sums512, sads256);

                            sads128[0] = _mm256_castsi256_si128(sads256[0]);
                            sads128[1] = _mm256_extracti128_si256(sads256[0], 1);
                            update_8_best(sads128, 0, y, &best_s, &best_x, &best_y);

                            sads128[0] = _mm256_castsi256_si128(sads256[1]);
                            sads128[1] = _mm256_extracti128_si256(sads256[1], 1);
                            update_8_best(sads128, 8, y, &best_s, &best_x, &best_y);
                        }
                    }

                    ref += src_stride_raw;
                } while (++y < search_area_height);
            }
            break;

        case 64:
            if (height <= 16) {
                y = 0;
                do {
                    __m512i sums512[2] = {_mm512_setzero_si512(), _mm512_setzero_si512()};

                    s = src;
                    r = ref;

                    h = height;
                    do {
                        sad_loop_kernel_32_2sum_avx512(s + 0 * 32, r + 0 * 32, sums512);
                        sad_loop_kernel_32_2sum_avx512(s + 1 * 32, r + 1 * 32, sums512);
                        s += src_stride;
                        r += ref_stride;
                    } while (--h);

                    update_1024_pel(sums512, 0, y, &best_s, &best_x, &best_y);
                    ref += src_stride_raw;
                } while (++y < search_area_height);
            } else if (height <= 32) {
                y = 0;
                do {
                    __m512i sums512[4] = {
                        _mm512_setzero_si512(), _mm512_setzero_si512(), _mm512_setzero_si512(), _mm512_setzero_si512()};

                    s = src;
                    r = ref;

                    h = height;
                    do {
                        sad_loop_kernel_32_4sum_avx512(s + 0 * 32, r + 0 * 32, sums512);
                        sad_loop_kernel_32_4sum_avx512(s + 1 * 32, r + 1 * 32, sums512);
                        s += src_stride;
                        r += ref_stride;
                    } while (--h);

                    update_2048_pel(sums512, 0, y, &best_s, &best_x, &best_y);
                    ref += src_stride_raw;
                } while (++y < search_area_height);
            } else {
                y = 0;
                do {
                    __m512i sums512[8] = {_mm512_setzero_si512(),
                                          _mm512_setzero_si512(),
                                          _mm512_setzero_si512(),
                                          _mm512_setzero_si512(),
                                          _mm512_setzero_si512(),
                                          _mm512_setzero_si512(),
                                          _mm512_setzero_si512(),
                                          _mm512_setzero_si512()};

                    s = src;
                    r = ref;

                    h = height;
                    do {
                        sad_loop_kernel_32_4sum_avx512(s + 0 * 32, r + 0 * 32, sums512 + 0);
                        sad_loop_kernel_32_4sum_avx512(s + 1 * 32, r + 1 * 32, sums512 + 4);
                        s += src_stride;
                        r += ref_stride;
                    } while (--h);

                    const __m512i  sum512_01   = _mm512_adds_epu16(sums512[0], sums512[1]);
                    const __m512i  sum512_23   = _mm512_adds_epu16(sums512[2], sums512[3]);
                    const __m512i  sum512_45   = _mm512_adds_epu16(sums512[4], sums512[5]);
                    const __m512i  sum512_67   = _mm512_adds_epu16(sums512[6], sums512[7]);
                    const __m512i  sum512_0123 = _mm512_adds_epu16(sum512_01, sum512_23);
                    const __m512i  sum512_4567 = _mm512_adds_epu16(sum512_45, sum512_67);
                    const __m512i  sum512      = _mm512_adds_epu16(sum512_0123, sum512_4567);
                    const __m256i  sum_lo      = _mm512_castsi512_si256(sum512);
                    const __m256i  sum_hi      = _mm512_extracti64x4_epi64(sum512, 1);
                    const __m256i  sad         = _mm256_adds_epu16(sum_lo, sum_hi);
                    const __m128i  sad_lo      = _mm256_castsi256_si128(sad);
                    const __m128i  sad_hi      = _mm256_extracti128_si256(sad, 1);
                    const __m128i  minpos_lo   = _mm_minpos_epu16(sad_lo);
                    const __m128i  minpos_hi   = _mm_minpos_epu16(sad_hi);
                    const uint32_t min0        = _mm_extract_epi16(minpos_lo, 0);
                    const uint32_t min1        = _mm_extract_epi16(minpos_hi, 0);
                    uint32_t       minmin, delta;
                    __m128i        minpos;

                    if (min0 <= min1) {
                        minmin = min0;
                        delta  = 0;
                        minpos = minpos_lo;
                    } else {
                        minmin = min1;
                        delta  = 8;
                        minpos = minpos_hi;
                    }

                    if (minmin < best_s) {
                        if (minmin != 0xFFFF) { // no overflow
                            best_s = minmin;
                            best_x = delta + _mm_extract_epi16(minpos, 1);
                            best_y = y;
                        } else { // overflow
                            __m256i sads256[2];
                            __m128i sads128[2];

                            add16x16x8to32bit(sums512, sads256);

                            sads128[0] = _mm256_castsi256_si128(sads256[0]);
                            sads128[1] = _mm256_extracti128_si256(sads256[0], 1);
                            update_8_best(sads128, 0, y, &best_s, &best_x, &best_y);

                            sads128[0] = _mm256_castsi256_si128(sads256[1]);
                            sads128[1] = _mm256_extracti128_si256(sads256[1], 1);
                            update_8_best(sads128, 8, y, &best_s, &best_x, &best_y);
                        }
                    }

                    ref += src_stride_raw;
                } while (++y < search_area_height);
            }
            break;

        default:
            sad_loop_kernel_generalized_avx512(src,
                                               src_stride,
                                               ref,
                                               ref_stride,
                                               height,
                                               width,
                                               best_sad,
                                               x_search_center,
                                               y_search_center,
                                               src_stride_raw,
                                               search_area_width,
                                               search_area_height);
            return;
        }
    } else {
        const uint32_t leftover = search_area_width & 7;
        __m128i        mask128;
        __m256i        mask256;

        mask128 = _mm_set1_epi32(-1);
        for (x = 0; x < (int32_t)leftover; x++) {
            mask128 = _mm_slli_si128(mask128, 2);
        }
        mask256 = _mm256_insertf128_si256(_mm256_castsi128_si256(mask128), mask128, 1);

        switch (width) {
        case 4:
            if (height <= 4) {
                y = 0;
                do {
                    for (x = 0; x <= search_area_width - 8; x += 8) {
                        __m128i sum = _mm_setzero_si128();

                        s = src;
                        r = ref + x;

                        h = height;
                        while (h >= 2) {
                            sad_loop_kernel_4_sse4_1(s, src_stride, r, ref_stride, &sum);
                            s += 2 * src_stride;
                            r += 2 * ref_stride;
                            h -= 2;
                        };

                        if (h) {
                            sad_loop_kernel_4_oneline_sse4_1(s, r, &sum);
                        }

                        update_best(sum, x, y, &best_s, &best_x, &best_y);
                    }

                    if (leftover) {
                        __m128i sum = _mm_setzero_si128();

                        s = src;
                        r = ref + x;

                        h = height;
                        while (h >= 2) {
                            sad_loop_kernel_4_sse4_1(s, src_stride, r, ref_stride, &sum);
                            s += 2 * src_stride;
                            r += 2 * ref_stride;
                            h -= 2;
                        };

                        if (h) {
                            sad_loop_kernel_4_oneline_sse4_1(s, r, &sum);
                        }

                        sum = _mm_or_si128(sum, mask128);
                        update_best(sum, x, y, &best_s, &best_x, &best_y);
                    }

                    ref += src_stride_raw;
                } while (++y < search_area_height);
            } else {
                y = 0;
                do {
                    for (x = 0; x <= search_area_width - 8; x += 8) {
                        __m256i sum256 = _mm256_setzero_si256();

                        s = src;
                        r = ref + x;

                        h = height;
                        while (h >= 2) {
                            sad_loop_kernel_4_avx2(s, src_stride, r, ref_stride, &sum256);
                            s += 2 * src_stride;
                            r += 2 * ref_stride;
                            h -= 2;
                        };

                        if (h) {
                            sad_loop_kernel_4_oneline_avx2(s, r, &sum256);
                        }

                        update_small_pel(sum256, x, y, &best_s, &best_x, &best_y);
                    }

                    if (leftover) {
                        __m256i sum256 = _mm256_setzero_si256();

                        s = src;
                        r = ref + x;

                        h = height;
                        while (h >= 2) {
                            sad_loop_kernel_4_avx2(s, src_stride, r, ref_stride, &sum256);
                            s += 2 * src_stride;
                            r += 2 * ref_stride;
                            h -= 2;
                        };
                        if (h) {
                            sad_loop_kernel_4_oneline_avx2(s, r, &sum256);
                        }

                        update_leftover_small_pel(sum256, x, y, mask128, &best_s, &best_x, &best_y);
                    }

                    ref += src_stride_raw;
                } while (++y < search_area_height);
            }
            break;

        case 6:
            if (height <= 4) {
                y = 0;
                do {
                    for (x = 0; x <= search_area_width - 8; x += 8) {
                        __m128i sum = _mm_setzero_si128();

                        s = src;
                        r = ref + x;

                        h = height;
                        while (h >= 2) {
                            sad_loop_kernel_4_sse4_1(s, src_stride, r, ref_stride, &sum);
                            s += 2 * src_stride;
                            r += 2 * ref_stride;
                            h -= 2;
                        };

                        if (h) {
                            sad_loop_kernel_4_oneline_sse4_1(s, r, &sum);
                        }

                        __m128i sum2 = complement_4_to_6(ref + x, ref_stride, src, src_stride, height, 8);
                        sum          = _mm_adds_epu16(sum, sum2);

                        update_best(sum, x, y, &best_s, &best_x, &best_y);
                    }

                    if (leftover) {
                        __m128i sum = _mm_setzero_si128();

                        s = src;
                        r = ref + x;

                        h = height;
                        while (h >= 2) {
                            sad_loop_kernel_4_sse4_1(s, src_stride, r, ref_stride, &sum);
                            s += 2 * src_stride;
                            r += 2 * ref_stride;
                            h -= 2;
                        };

                        if (h) {
                            sad_loop_kernel_4_oneline_sse4_1(s, r, &sum);
                        }

                        sum = _mm_or_si128(sum, mask128);

                        __m128i sum2 = complement_4_to_6(ref + x, ref_stride, src, src_stride, height, leftover);
                        sum          = _mm_adds_epu16(sum, sum2);

                        update_best(sum, x, y, &best_s, &best_x, &best_y);
                    }

                    ref += src_stride_raw;
                } while (++y < search_area_height);
            } else {
                y = 0;
                do {
                    for (x = 0; x <= search_area_width - 8; x += 8) {
                        __m256i sum256 = _mm256_setzero_si256();

                        s = src;
                        r = ref + x;

                        h = height;
                        while (h >= 2) {
                            sad_loop_kernel_4_avx2(s, src_stride, r, ref_stride, &sum256);
                            s += 2 * src_stride;
                            r += 2 * ref_stride;
                            h -= 2;
                        };

                        if (h) {
                            sad_loop_kernel_4_oneline_avx2(s, r, &sum256);
                        }

                        __m128i sum = complement_4_to_6(ref + x, ref_stride, src, src_stride, height, 8);
                        sum256 = _mm256_adds_epu16(sum256, _mm256_insertf128_si256(_mm256_setzero_si256(), sum, 0));

                        update_small_pel(sum256, x, y, &best_s, &best_x, &best_y);
                    }

                    if (leftover) {
                        __m256i sum256 = _mm256_setzero_si256();

                        s = src;
                        r = ref + x;

                        h = height;
                        while (h >= 2) {
                            sad_loop_kernel_4_avx2(s, src_stride, r, ref_stride, &sum256);
                            s += 2 * src_stride;
                            r += 2 * ref_stride;
                            h -= 2;
                        };

                        if (h) {
                            sad_loop_kernel_4_oneline_avx2(s, r, &sum256);
                        }

                        __m128i sum = complement_4_to_6(ref + x, ref_stride, src, src_stride, height, leftover);
                        sum256 = _mm256_adds_epu16(sum256, _mm256_insertf128_si256(_mm256_setzero_si256(), sum, 0));

                        update_leftover_small_pel(sum256, x, y, mask128, &best_s, &best_x, &best_y);
                    }

                    ref += src_stride_raw;
                } while (++y < search_area_height);
            }
            break;

        case 8:
            // Note: Tried _mm512_dbsad_epu8 but is even slower.
            y = 0;
            do {
                for (x = 0; x <= search_area_width - 8; x += 8) {
                    __m256i sum256 = _mm256_setzero_si256();

                    s = src;
                    r = ref + x;

                    h = height;
                    while (h >= 2) {
                        sad_loop_kernel_8_avx2(s, src_stride, r, ref_stride, &sum256);
                        s += 2 * src_stride;
                        r += 2 * ref_stride;
                        h -= 2;
                    };

                    if (h) {
                        sad_loop_kernel_8_oneline_avx2(s, r, &sum256);
                    };

                    update_small_pel(sum256, x, y, &best_s, &best_x, &best_y);
                }

                if (leftover) {
                    __m256i sum256 = _mm256_setzero_si256();

                    s = src;
                    r = ref + x;

                    h = height;
                    while (h >= 2) {
                        sad_loop_kernel_8_avx2(s, src_stride, r, ref_stride, &sum256);
                        s += 2 * src_stride;
                        r += 2 * ref_stride;
                        h -= 2;
                    };

                    if (h) {
                        sad_loop_kernel_8_oneline_avx2(s, r, &sum256);
                    };

                    update_leftover_small_pel(sum256, x, y, mask128, &best_s, &best_x, &best_y);
                }

                ref += src_stride_raw;
            } while (++y < search_area_height);
            break;

        case 12:
            if (height <= 16) {
                y = 0;
                do {
                    for (x = 0; x <= search_area_width - 16; x += 16) {
                        __m512i sum512 = _mm512_setzero_si512();

                        s = src;
                        r = ref + x;

                        h = height;
                        while (h >= 2) {
                            sad_loop_kernel_12_avx512(s, src_stride, r, ref_stride, &sum512);
                            s += 2 * src_stride;
                            r += 2 * ref_stride;
                            h -= 2;
                        };

                        if (h) {
                            sad_loop_kernel_12_oneline_avx512(s, r, &sum512);
                        }

                        update_256_pel(sum512, x, y, &best_s, &best_x, &best_y);
                    }

                    // leftover
                    for (; x < search_area_width; x += 8) {
                        __m256i sum256 = _mm256_setzero_si256();

                        s = src;
                        r = ref + x;

                        h = height;
                        while (h >= 2) {
                            sad_loop_kernel_12_avx2(s, src_stride, r, ref_stride, &sum256);
                            s += 2 * src_stride;
                            r += 2 * ref_stride;
                            h -= 2;
                        };

                        if (h) {
                            sad_loop_kernel_12_oneline_avx2(s, r, &sum256);
                        }

                        update_leftover_256_pel(sum256, search_area_width, x, y, mask128, &best_s, &best_x, &best_y);
                    }

                    ref += src_stride_raw;
                } while (++y < search_area_height);
            } else if (height <= 32) {
                y = 0;
                do {
                    for (x = 0; x <= search_area_width - 16; x += 16) {
                        __m512i sum512 = _mm512_setzero_si512();

                        s = src;
                        r = ref + x;

                        h = height;
                        while (h >= 2) {
                            sad_loop_kernel_12_avx512(s, src_stride, r, ref_stride, &sum512);
                            s += 2 * src_stride;
                            r += 2 * ref_stride;
                            h -= 2;
                        };

                        if (h) {
                            sad_loop_kernel_12_oneline_avx512(s, r, &sum512);
                        }

                        update_512_pel(sum512, x, y, &best_s, &best_x, &best_y);
                    }

                    // leftover
                    for (; x < search_area_width; x += 8) {
                        __m256i sum256 = _mm256_setzero_si256();

                        s = src;
                        r = ref + x;

                        h = height;
                        while (h >= 2) {
                            sad_loop_kernel_12_avx2(s, src_stride, r, ref_stride, &sum256);
                            s += 2 * src_stride;
                            r += 2 * ref_stride;
                            h -= 2;
                        };

                        if (h) {
                            sad_loop_kernel_12_oneline_avx2(s, r, &sum256);
                        }

                        update_leftover_512_pel(sum256, search_area_width, x, y, mask256, &best_s, &best_x, &best_y);
                    }

                    ref += src_stride_raw;
                } while (++y < search_area_height);
            } else {
                const uint32_t leftover16 = search_area_width & 15;

                y = 0;
                do {
                    for (x = 0; x <= search_area_width - 16; x += 16) {
                        __m512i sums512[2] = {_mm512_setzero_si512(), _mm512_setzero_si512()};

                        s = src;
                        r = ref + x;

                        h = height;
                        while (h >= 2) {
                            sad_loop_kernel_12_2sum_avx512(s, src_stride, r, ref_stride, sums512);
                            s += 2 * src_stride;
                            r += 2 * ref_stride;
                            h -= 2;
                        };

                        if (h) {
                            sad_loop_kernel_12_2sum_oneline_avx512(s, r, sums512);
                        }

                        update_1024_pel(sums512, x, y, &best_s, &best_x, &best_y);
                    }

                    if (leftover16 >= 8) {
                        __m256i sums256[2] = {_mm256_setzero_si256(), _mm256_setzero_si256()};

                        s = src;
                        r = ref + x;

                        h = height;
                        while (h >= 2) {
                            sad_loop_kernel_12_2sum_avx2(s, src_stride, r, ref_stride, sums256);
                            s += 2 * src_stride;
                            r += 2 * ref_stride;
                            h -= 2;
                        };

                        if (h) {
                            sad_loop_kernel_12_2sum_oneline_avx2(s, r, sums256);
                        }

                        update_leftover8_1024_pel(sums256, x, y, &best_s, &best_x, &best_y);
                        x += 8;
                    }

                    if (leftover) {
                        __m256i sums256[2] = {_mm256_setzero_si256(), _mm256_setzero_si256()};

                        s = src;
                        r = ref + x;

                        h = height;
                        while (h >= 2) {
                            sad_loop_kernel_12_2sum_avx2(s, src_stride, r, ref_stride, sums256);
                            s += 2 * src_stride;
                            r += 2 * ref_stride;
                            h -= 2;
                        };

                        if (h) {
                            sad_loop_kernel_12_2sum_oneline_avx2(s, r, sums256);
                        }

                        update_leftover_1024_pel(
                            sums256, search_area_width, x, y, leftover, mask128, &best_s, &best_x, &best_y);
                    }

                    ref += src_stride_raw;
                } while (++y < search_area_height);
            }
            break;

        case 16:
            if (height <= 16) {
                y = 0;
                do {
                    if (skip_search_line) {
                        if ((y & 1) == 0) {
                            ref += src_stride_raw;
                            continue;
                        }
                    }
                    for (x = 0; x <= search_area_width - 16; x += 16) {
                        __m512i sum512 = _mm512_setzero_si512();

                        s = src;
                        r = ref + x;

                        h = height;
                        while (h >= 2) {
                            sad_loop_kernel_16_avx512(s, src_stride, r, ref_stride, &sum512);
                            s += 2 * src_stride;
                            r += 2 * ref_stride;
                            h -= 2;
                        };

                        if (h) {
                            sad_loop_kernel_16_oneline_avx512(s, r, &sum512);
                        };

                        update_256_pel(sum512, x, y, &best_s, &best_x, &best_y);
                    }

                    // leftover
                    for (; x < search_area_width; x += 8) {
                        __m256i sum256 = _mm256_setzero_si256();

                        s = src;
                        r = ref + x;

                        h = height;
                        while (h >= 2) {
                            sad_loop_kernel_16_avx2(s, src_stride, r, ref_stride, &sum256);
                            s += 2 * src_stride;
                            r += 2 * ref_stride;
                            h -= 2;
                        }

                        if (h) {
                            sad_loop_kernel_16_oneline_avx2(s, r, &sum256);
                        }

                        update_leftover_256_pel(sum256, search_area_width, x, y, mask128, &best_s, &best_x, &best_y);
                    }

                    ref += src_stride_raw;
                } while (++y < search_area_height);
            } else if (height <= 32) {
                y = 0;
                do {
                    for (x = 0; x <= search_area_width - 16; x += 16) {
                        __m512i sum512 = _mm512_setzero_si512();

                        s = src;
                        r = ref + x;

                        h = height;
                        while (h >= 2) {
                            sad_loop_kernel_16_avx512(s, src_stride, r, ref_stride, &sum512);
                            s += 2 * src_stride;
                            r += 2 * ref_stride;
                            h -= 2;
                        }

                        if (h) {
                            sad_loop_kernel_16_oneline_avx512(s, r, &sum512);
                        }

                        update_512_pel(sum512, x, y, &best_s, &best_x, &best_y);
                    }

                    // leftover
                    for (; x < search_area_width; x += 8) {
                        __m256i sum256 = _mm256_setzero_si256();

                        s = src;
                        r = ref + x;

                        h = height;
                        while (h >= 2) {
                            sad_loop_kernel_16_avx2(s, src_stride, r, ref_stride, &sum256);
                            s += 2 * src_stride;
                            r += 2 * ref_stride;
                            h -= 2;
                        }

                        if (h) {
                            sad_loop_kernel_16_oneline_avx2(s, r, &sum256);
                        }

                        update_leftover_512_pel(sum256, search_area_width, x, y, mask256, &best_s, &best_x, &best_y);
                    }

                    ref += src_stride_raw;
                } while (++y < search_area_height);
            } else {
                const uint32_t leftover16 = search_area_width & 15;

                y = 0;
                do {
                    for (x = 0; x <= search_area_width - 16; x += 16) {
                        __m512i sums512[2] = {_mm512_setzero_si512(), _mm512_setzero_si512()};

                        s = src;
                        r = ref + x;

                        h = height;
                        while (h >= 2) {
                            sad_loop_kernel_16_2sum_avx512(s, src_stride, r, ref_stride, sums512);
                            s += 2 * src_stride;
                            r += 2 * ref_stride;
                            h -= 2;
                        }

                        if (h) {
                            sad_loop_kernel_16_2sum_oneline_avx512(s, r, sums512);
                        }

                        update_1024_pel(sums512, x, y, &best_s, &best_x, &best_y);
                    }

                    if (leftover16 >= 8) {
                        __m256i sums256[2] = {_mm256_setzero_si256(), _mm256_setzero_si256()};

                        s = src;
                        r = ref + x;

                        h = height;
                        while (h >= 2) {
                            sad_loop_kernel_16_2sum_avx2(s, src_stride, r, ref_stride, sums256);
                            s += 2 * src_stride;
                            r += 2 * ref_stride;
                            h -= 2;
                        }

                        if (h) {
                            sad_loop_kernel_16_2sum_oneline_avx2(s, r, sums256);
                        }

                        update_leftover8_1024_pel(sums256, x, y, &best_s, &best_x, &best_y);
                        x += 8;
                    }

                    if (leftover) {
                        __m256i sums256[2] = {_mm256_setzero_si256(), _mm256_setzero_si256()};

                        s = src;
                        r = ref + x;

                        h = height;
                        while (h >= 2) {
                            sad_loop_kernel_16_2sum_avx2(s, src_stride, r, ref_stride, sums256);
                            s += 2 * src_stride;
                            r += 2 * ref_stride;
                            h -= 2;
                        }

                        if (h) {
                            sad_loop_kernel_16_2sum_oneline_avx2(s, r, sums256);
                        }

                        update_leftover_1024_pel(
                            sums256, search_area_width, x, y, leftover, mask128, &best_s, &best_x, &best_y);
                    }

                    ref += src_stride_raw;
                } while (++y < search_area_height);
            }
            break;

        case 24:
            if (height <= 16) {
                y = 0;
                do {
                    for (x = 0; x <= search_area_width - 16; x += 16) {
                        __m512i sum512     = _mm512_setzero_si512();
                        __m256i sums256[2] = {_mm256_setzero_si256(), _mm256_setzero_si256()};

                        s = src;
                        r = ref + x;

                        h = height;
                        while (h >= 2) {
                            sad_loop_kernel_16_avx512(s, src_stride, r, ref_stride, &sum512);
                            sad_loop_kernel_8_avx2(s + 16, src_stride, r + 16, ref_stride, &sums256[0]);
                            sad_loop_kernel_8_avx2(s + 16, src_stride, r + 24, ref_stride, &sums256[1]);
                            s += 2 * src_stride;
                            r += 2 * ref_stride;
                            h -= 2;
                        };

                        if (h) {
                            sad_loop_kernel_16_oneline_avx512(s, r, &sum512);
                            sad_loop_kernel_8_oneline_avx2(s + 16, r + 16, &sums256[0]);
                            sad_loop_kernel_8_oneline_avx2(s + 16, r + 24, &sums256[1]);
                        }

                        update_384_pel(sum512, sums256, x, y, &best_s, &best_x, &best_y);
                    }

                    // leftover
                    for (; x < search_area_width; x += 8) {
                        __m256i sum256 = _mm256_setzero_si256();

                        s = src;
                        r = ref + x;

                        h = height;
                        while (h >= 2) {
                            sad_loop_kernel_16_avx2(s, src_stride, r, ref_stride, &sum256);
                            sad_loop_kernel_8_avx2(s + 16, src_stride, r + 16, ref_stride, &sum256);
                            s += 2 * src_stride;
                            r += 2 * ref_stride;
                            h -= 2;
                        };

                        if (h) {
                            sad_loop_kernel_16_oneline_avx2(s, r, &sum256);
                            sad_loop_kernel_8_oneline_avx2(s + 16, r + 16, &sum256);
                        }

                        update_leftover_512_pel(sum256, search_area_width, x, y, mask256, &best_s, &best_x, &best_y);
                    }

                    ref += src_stride_raw;
                } while (++y < search_area_height);
            } else {
                const uint32_t leftover16 = search_area_width & 15;

                y = 0;
                do {
                    for (x = 0; x <= search_area_width - 16; x += 16) {
                        __m512i sum512     = _mm512_setzero_si512();
                        __m256i sums256[2] = {_mm256_setzero_si256(), _mm256_setzero_si256()};

                        s = src;
                        r = ref + x;

                        h = height;
                        while (h >= 2) {
                            sad_loop_kernel_16_avx512(s, src_stride, r, ref_stride, &sum512);
                            sad_loop_kernel_8_avx2(s + 16, src_stride, r + 16, ref_stride, &sums256[0]);
                            sad_loop_kernel_8_avx2(s + 16, src_stride, r + 24, ref_stride, &sums256[1]);
                            s += 2 * src_stride;
                            r += 2 * ref_stride;
                            h -= 2;
                        };

                        if (h) {
                            sad_loop_kernel_16_oneline_avx512(s, r, &sum512);
                            sad_loop_kernel_8_oneline_avx2(s + 16, r + 16, &sums256[0]);
                            sad_loop_kernel_8_oneline_avx2(s + 16, r + 24, &sums256[1]);
                        }

                        update_768_pel(sum512, sums256, x, y, &best_s, &best_x, &best_y);
                    }

                    // leftover
                    if (leftover16 >= 8) {
                        __m256i sums256[2] = {_mm256_setzero_si256(), _mm256_setzero_si256()};

                        s = src;
                        r = ref + x;

                        h = height;
                        while (h >= 2) {
                            sad_loop_kernel_16_avx2(s, src_stride, r, ref_stride, &sums256[0]);
                            sad_loop_kernel_8_avx2(s + 16, src_stride, r + 16, ref_stride, &sums256[1]);
                            s += 2 * src_stride;
                            r += 2 * ref_stride;
                            h -= 2;
                        };

                        if (h) {
                            sad_loop_kernel_16_oneline_avx2(s, r, &sums256[0]);
                            sad_loop_kernel_8_oneline_avx2(s + 16, r + 16, &sums256[1]);
                        }

                        update_leftover8_1024_pel(sums256, x, y, &best_s, &best_x, &best_y);

                        x += 8;
                    }

                    if (leftover) {
                        __m256i sums256[2] = {_mm256_setzero_si256(), _mm256_setzero_si256()};

                        s = src;
                        r = ref + x;

                        h = height;
                        while (h >= 2) {
                            sad_loop_kernel_16_avx2(s, src_stride, r, ref_stride, &sums256[0]);
                            sad_loop_kernel_8_avx2(s + 16, src_stride, r + 16, ref_stride, &sums256[1]);
                            s += 2 * src_stride;
                            r += 2 * ref_stride;
                            h -= 2;
                        };

                        if (h) {
                            sad_loop_kernel_16_oneline_avx2(s, r, &sums256[0]);
                            sad_loop_kernel_8_oneline_avx2(s + 16, r + 16, &sums256[1]);
                        }

                        update_leftover_1024_pel(
                            sums256, search_area_width, x, y, leftover, mask128, &best_s, &best_x, &best_y);
                    }

                    ref += src_stride_raw;
                } while (++y < search_area_height);
            }
            break;

        case 32:
            if (height <= 16) {
                y = 0;
                do {
                    for (x = 0; x <= search_area_width - 16; x += 16) {
                        __m512i sum512 = _mm512_setzero_si512();

                        s = src;
                        r = ref + x;

                        // Note: faster than looping 2 rows.
                        h = height;
                        do {
                            sad_loop_kernel_32_avx512(s, r, &sum512);
                            s += src_stride;
                            r += ref_stride;
                        } while (--h);

                        update_512_pel(sum512, x, y, &best_s, &best_x, &best_y);
                    }

                    // leftover
                    for (; x < search_area_width; x += 8) {
                        __m256i sum256 = _mm256_setzero_si256();

                        s = src;
                        r = ref + x;

                        h = height;
                        do {
                            sad_loop_kernel_32_avx2(s, r, &sum256);
                            s += src_stride;
                            r += ref_stride;
                        } while (--h);

                        update_leftover_512_pel(sum256, search_area_width, x, y, mask256, &best_s, &best_x, &best_y);
                    }

                    ref += src_stride_raw;
                } while (++y < search_area_height);
            } else if (height <= 32) {
                const uint32_t leftover16 = search_area_width & 15;

                y = 0;
                do {
                    for (x = 0; x <= search_area_width - 16; x += 16) {
                        __m512i sums512[2] = {_mm512_setzero_si512(), _mm512_setzero_si512()};

                        s = src;
                        r = ref + x;

                        h = height;
                        do {
                            sad_loop_kernel_32_2sum_avx512(s, r, sums512);
                            s += src_stride;
                            r += ref_stride;
                        } while (--h);

                        update_1024_pel(sums512, x, y, &best_s, &best_x, &best_y);
                    }

                    if (leftover16 >= 8) {
                        __m256i sums256[2] = {_mm256_setzero_si256(), _mm256_setzero_si256()};

                        s = src;
                        r = ref + x;

                        h = height;
                        do {
                            sad_loop_kernel_32_2sum_avx2(s, r, sums256);
                            s += src_stride;
                            r += ref_stride;
                        } while (--h);

                        update_leftover8_1024_pel(sums256, x, y, &best_s, &best_x, &best_y);
                        x += 8;
                    }

                    if (leftover) {
                        __m256i sums256[2] = {_mm256_setzero_si256(), _mm256_setzero_si256()};

                        s = src;
                        r = ref + x;

                        h = height;
                        do {
                            sad_loop_kernel_32_2sum_avx2(s, r, sums256);
                            s += src_stride;
                            r += ref_stride;
                        } while (--h);

                        update_leftover_1024_pel(
                            sums256, search_area_width, x, y, leftover, mask128, &best_s, &best_x, &best_y);
                    }

                    ref += src_stride_raw;
                } while (++y < search_area_height);
            } else {
                const uint32_t leftover16 = search_area_width & 15;

                y = 0;
                do {
                    for (x = 0; x <= search_area_width - 16; x += 16) {
                        __m512i sums512[4] = {_mm512_setzero_si512(),
                                              _mm512_setzero_si512(),
                                              _mm512_setzero_si512(),
                                              _mm512_setzero_si512()};

                        s = src;
                        r = ref + x;

                        h = height;
                        do {
                            sad_loop_kernel_32_4sum_avx512(s, r, sums512);
                            s += src_stride;
                            r += ref_stride;
                        } while (--h);

                        update_2048_pel(sums512, x, y, &best_s, &best_x, &best_y);
                    }

                    if (leftover16 >= 8) {
                        __m256i sums256[4] = {_mm256_setzero_si256(),
                                              _mm256_setzero_si256(),
                                              _mm256_setzero_si256(),
                                              _mm256_setzero_si256()};

                        s = src;
                        r = ref + x;

                        h = height;
                        do {
                            sad_loop_kernel_32_4sum_avx2(s, r, sums256);
                            s += src_stride;
                            r += ref_stride;
                        } while (--h);

                        update_leftover8_2048_pel(sums256, x, y, &best_s, &best_x, &best_y);
                        x += 8;
                    }

                    if (leftover) {
                        __m256i sums256[4] = {_mm256_setzero_si256(),
                                              _mm256_setzero_si256(),
                                              _mm256_setzero_si256(),
                                              _mm256_setzero_si256()};

                        s = src;
                        r = ref + x;

                        h = height;
                        do {
                            sad_loop_kernel_32_4sum_avx2(s, r, sums256);
                            s += src_stride;
                            r += ref_stride;
                        } while (--h);

                        update_leftover_2048_pel(
                            sums256, search_area_width, x, y, leftover, mask128, &best_s, &best_x, &best_y);
                    }

                    ref += src_stride_raw;
                } while (++y < search_area_height);
            }
            break;

        case 48:
            if (height <= 32) {
                const uint32_t leftover16 = search_area_width & 15;

                y = 0;
                do {
                    for (x = 0; x <= search_area_width - 16; x += 16) {
                        __m512i sums512[3] = {_mm512_setzero_si512(), _mm512_setzero_si512(), _mm512_setzero_si512()};

                        s = src;
                        r = ref + x;

                        h = height2;
                        do {
                            sad_loop_kernel_32_2sum_avx512(s, r, sums512);
                            sad_loop_kernel_32_2sum_avx512(s + src_stride, r + ref_stride, sums512);
                            sad_loop_kernel_16_avx512(s + 32, src_stride, r + 32, ref_stride, &sums512[2]);
                            s += 2 * src_stride;
                            r += 2 * ref_stride;
                        } while (--h);

                        update_1536_pel(sums512, x, y, &best_s, &best_x, &best_y);
                    }

                    if (leftover16 >= 8) {
                        __m256i sums256[3] = {_mm256_setzero_si256(), _mm256_setzero_si256(), _mm256_setzero_si256()};

                        s = src;
                        r = ref + x;

                        h = height2;
                        do {
                            sad_loop_kernel_32_2sum_avx2(s, r, sums256);
                            sad_loop_kernel_32_2sum_avx2(s + src_stride, r + ref_stride, sums256);
                            sad_loop_kernel_16_avx2(s + 32, src_stride, r + 32, ref_stride, &sums256[2]);
                            s += 2 * src_stride;
                            r += 2 * ref_stride;
                        } while (--h);

                        update_leftover8_1536_pel(sums256, x, y, &best_s, &best_x, &best_y);
                        x += 8;
                    }

                    if (leftover) {
                        __m256i sums256[3] = {_mm256_setzero_si256(), _mm256_setzero_si256(), _mm256_setzero_si256()};

                        s = src;
                        r = ref + x;

                        h = height2;
                        do {
                            sad_loop_kernel_32_2sum_avx2(s, r, sums256);
                            sad_loop_kernel_32_2sum_avx2(s + src_stride, r + ref_stride, sums256);
                            sad_loop_kernel_16_avx2(s + 32, src_stride, r + 32, ref_stride, &sums256[2]);
                            s += 2 * src_stride;
                            r += 2 * ref_stride;
                        } while (--h);

                        update_leftover_1536_pel(
                            sums256, search_area_width, x, y, leftover, mask128, &best_s, &best_x, &best_y);
                    }

                    ref += src_stride_raw;
                } while (++y < search_area_height);
            } else {
                const uint32_t leftover16 = search_area_width & 15;

                y = 0;
                do {
                    for (x = 0; x <= search_area_width - 16; x += 16) {
                        __m512i sums512[6] = {_mm512_setzero_si512(),
                                              _mm512_setzero_si512(),
                                              _mm512_setzero_si512(),
                                              _mm512_setzero_si512(),
                                              _mm512_setzero_si512(),
                                              _mm512_setzero_si512()};

                        s = src;
                        r = ref + x;

                        h = height2;
                        do {
                            sad_loop_kernel_32_4sum_avx512(s, r, sums512);
                            sad_loop_kernel_32_4sum_avx512(s + src_stride, r + ref_stride, sums512);
                            sad_loop_kernel_16_2sum_avx512(s + 32, src_stride, r + 32, ref_stride, &sums512[4]);
                            s += 2 * src_stride;
                            r += 2 * ref_stride;
                        } while (--h);

                        const __m512i  sum512_01   = _mm512_adds_epu16(sums512[0], sums512[1]);
                        const __m512i  sum512_23   = _mm512_adds_epu16(sums512[2], sums512[3]);
                        const __m512i  sum512_45   = _mm512_adds_epu16(sums512[4], sums512[5]);
                        const __m512i  sum512_0123 = _mm512_adds_epu16(sum512_01, sum512_23);
                        const __m512i  sum512      = _mm512_adds_epu16(sum512_0123, sum512_45);
                        const __m256i  sum_lo      = _mm512_castsi512_si256(sum512);
                        const __m256i  sum_hi      = _mm512_extracti64x4_epi64(sum512, 1);
                        const __m256i  sad         = _mm256_adds_epu16(sum_lo, sum_hi);
                        const __m128i  sad_lo      = _mm256_castsi256_si128(sad);
                        const __m128i  sad_hi      = _mm256_extracti128_si256(sad, 1);
                        const __m128i  minpos_lo   = _mm_minpos_epu16(sad_lo);
                        const __m128i  minpos_hi   = _mm_minpos_epu16(sad_hi);
                        const uint32_t min0        = _mm_extract_epi16(minpos_lo, 0);
                        const uint32_t min1        = _mm_extract_epi16(minpos_hi, 0);
                        uint32_t       minmin, delta;
                        __m128i        minpos;

                        if (min0 <= min1) {
                            minmin = min0;
                            delta  = 0;
                            minpos = minpos_lo;
                        } else {
                            minmin = min1;
                            delta  = 8;
                            minpos = minpos_hi;
                        }

                        if (minmin < best_s) {
                            if (minmin != 0xFFFF) { // no overflow
                                best_s = minmin;
                                best_x = x + delta + _mm_extract_epi16(minpos, 1);
                                best_y = y;
                            } else { // overflow
                                __m256i sads256[2];
                                __m128i sads128[2];

                                add16x16x6to32bit(sums512, sads256);

                                sads128[0] = _mm256_castsi256_si128(sads256[0]);
                                sads128[1] = _mm256_extracti128_si256(sads256[0], 1);
                                update_8_best(sads128, x + 0, y, &best_s, &best_x, &best_y);

                                sads128[0] = _mm256_castsi256_si128(sads256[1]);
                                sads128[1] = _mm256_extracti128_si256(sads256[1], 1);
                                update_8_best(sads128, x + 8, y, &best_s, &best_x, &best_y);
                            }
                        }
                    }

                    if (leftover16 >= 8) {
                        __m256i sums256[6] = {_mm256_setzero_si256(),
                                              _mm256_setzero_si256(),
                                              _mm256_setzero_si256(),
                                              _mm256_setzero_si256(),
                                              _mm256_setzero_si256(),
                                              _mm256_setzero_si256()};

                        s = src;
                        r = ref + x;

                        h = height2;
                        do {
                            sad_loop_kernel_32_4sum_avx2(s, r, sums256);
                            sad_loop_kernel_32_4sum_avx2(s + src_stride, r + ref_stride, sums256);
                            sad_loop_kernel_16_2sum_avx2(s + 32, src_stride, r + 32, ref_stride, &sums256[4]);
                            s += 2 * src_stride;
                            r += 2 * ref_stride;
                        } while (--h);

                        const __m256i  sum01   = _mm256_adds_epu16(sums256[0], sums256[1]);
                        const __m256i  sum23   = _mm256_adds_epu16(sums256[2], sums256[3]);
                        const __m256i  sum45   = _mm256_adds_epu16(sums256[4], sums256[5]);
                        const __m256i  sum0123 = _mm256_adds_epu16(sum01, sum23);
                        const __m256i  sum256  = _mm256_adds_epu16(sum0123, sum45);
                        const __m128i  sum_lo  = _mm256_castsi256_si128(sum256);
                        const __m128i  sum_hi  = _mm256_extracti128_si256(sum256, 1);
                        const __m128i  sad     = _mm_adds_epu16(sum_lo, sum_hi);
                        const __m128i  minpos  = _mm_minpos_epu16(sad);
                        const uint32_t min0    = _mm_extract_epi16(minpos, 0);

                        if (min0 < best_s) {
                            if (min0 != 0xFFFF) { // no overflow
                                best_s = min0;
                                best_x = x + _mm_extract_epi16(minpos, 1);
                                best_y = y;
                            } else { // overflow
                                __m128i sads[2];

                                add16x8x6to32bit(sums256, sads);
                                update_8_best(sads, x, y, &best_s, &best_x, &best_y);
                            }
                        }

                        x += 8;
                    }

                    if (leftover) {
                        __m256i sums256[6] = {_mm256_setzero_si256(),
                                              _mm256_setzero_si256(),
                                              _mm256_setzero_si256(),
                                              _mm256_setzero_si256(),
                                              _mm256_setzero_si256(),
                                              _mm256_setzero_si256()};

                        s = src;
                        r = ref + x;

                        h = height2;
                        do {
                            sad_loop_kernel_32_4sum_avx2(s, r, sums256);
                            sad_loop_kernel_32_4sum_avx2(s + src_stride, r + ref_stride, sums256);
                            sad_loop_kernel_16_2sum_avx2(s + 32, src_stride, r + 32, ref_stride, &sums256[4]);
                            s += 2 * src_stride;
                            r += 2 * ref_stride;
                        } while (--h);

                        const __m256i  sum01   = _mm256_adds_epu16(sums256[0], sums256[1]);
                        const __m256i  sum23   = _mm256_adds_epu16(sums256[2], sums256[3]);
                        const __m256i  sum45   = _mm256_adds_epu16(sums256[4], sums256[5]);
                        const __m256i  sum0123 = _mm256_adds_epu16(sum01, sum23);
                        const __m256i  sum256  = _mm256_adds_epu16(sum0123, sum45);
                        const __m128i  sum_lo  = _mm256_castsi256_si128(sum256);
                        const __m128i  sum_hi  = _mm256_extracti128_si256(sum256, 1);
                        const __m128i  sad0    = _mm_adds_epu16(sum_lo, sum_hi);
                        const __m128i  sad1    = _mm_or_si128(sad0, mask128);
                        const __m128i  minpos  = _mm_minpos_epu16(sad1);
                        const uint32_t min0    = _mm_extract_epi16(minpos, 0);

                        if (min0 < best_s) {
                            if (min0 != 0xFFFF) { // no overflow
                                best_s = min0;
                                best_x = x + _mm_extract_epi16(minpos, 1);
                                best_y = y;
                            } else { // overflow
                                const int32_t num = x + ((leftover < 4) ? leftover : 4);
                                __m128i       sads[2];

                                add16x8x6to32bit(sums256, sads);

                                do {
                                    UPDATE_BEST(sads[0], 0, x, best_s, best_x, best_y);
                                    sads[0] = _mm_srli_si128(sads[0], 4);
                                } while (++x < num);

                                while (x < search_area_width) {
                                    UPDATE_BEST(sads[1], 0, x, best_s, best_x, best_y);
                                    sads[1] = _mm_srli_si128(sads[1], 4);
                                    x++;
                                }
                            }
                        }
                    }

                    ref += src_stride_raw;
                } while (++y < search_area_height);
            }
            break;

        case 64:
            if (height <= 16) {
                const uint32_t leftover16 = search_area_width & 15;

                y = 0;
                do {
                    for (x = 0; x <= search_area_width - 16; x += 16) {
                        __m512i sums512[2] = {_mm512_setzero_si512(), _mm512_setzero_si512()};

                        s = src;
                        r = ref + x;

                        h = height;
                        do {
                            sad_loop_kernel_32_2sum_avx512(s + 0 * 32, r + 0 * 32, sums512);
                            sad_loop_kernel_32_2sum_avx512(s + 1 * 32, r + 1 * 32, sums512);
                            s += src_stride;
                            r += ref_stride;
                        } while (--h);

                        update_1024_pel(sums512, x, y, &best_s, &best_x, &best_y);
                    }

                    if (leftover16 >= 8) {
                        __m256i sums256[2] = {_mm256_setzero_si256(), _mm256_setzero_si256()};

                        s = src;
                        r = ref + x;

                        h = height;
                        do {
                            sad_loop_kernel_64_2sum_avx2(s, r, sums256);
                            s += src_stride;
                            r += ref_stride;
                        } while (--h);

                        update_leftover8_1024_pel(sums256, x, y, &best_s, &best_x, &best_y);
                        x += 8;
                    }

                    if (leftover) {
                        __m256i sums256[2] = {_mm256_setzero_si256(), _mm256_setzero_si256()};

                        s = src;
                        r = ref + x;

                        h = height;
                        do {
                            sad_loop_kernel_64_2sum_avx2(s, r, sums256);
                            s += src_stride;
                            r += ref_stride;
                        } while (--h);

                        update_leftover_1024_pel(
                            sums256, search_area_width, x, y, leftover, mask128, &best_s, &best_x, &best_y);
                    }

                    ref += src_stride_raw;
                } while (++y < search_area_height);
            } else if (height <= 32) {
                const uint32_t leftover16 = search_area_width & 15;

                y = 0;
                do {
                    for (x = 0; x <= search_area_width - 16; x += 16) {
                        __m512i sums512[4] = {_mm512_setzero_si512(),
                                              _mm512_setzero_si512(),
                                              _mm512_setzero_si512(),
                                              _mm512_setzero_si512()};

                        s = src;
                        r = ref + x;

                        h = height;
                        do {
                            sad_loop_kernel_32_4sum_avx512(s + 0 * 32, r + 0 * 32, sums512);
                            sad_loop_kernel_32_4sum_avx512(s + 1 * 32, r + 1 * 32, sums512);
                            s += src_stride;
                            r += ref_stride;
                        } while (--h);

                        update_2048_pel(sums512, x, y, &best_s, &best_x, &best_y);
                    }

                    if (leftover16 >= 8) {
                        __m256i sums256[4] = {_mm256_setzero_si256(),
                                              _mm256_setzero_si256(),
                                              _mm256_setzero_si256(),
                                              _mm256_setzero_si256()};

                        s = src;
                        r = ref + x;

                        h = height;
                        do {
                            sad_loop_kernel_64_4sum_avx2(s, r, sums256);
                            s += src_stride;
                            r += ref_stride;
                        } while (--h);

                        update_leftover8_2048_pel(sums256, x, y, &best_s, &best_x, &best_y);
                        x += 8;
                    }

                    if (leftover) {
                        __m256i sums256[4] = {_mm256_setzero_si256(),
                                              _mm256_setzero_si256(),
                                              _mm256_setzero_si256(),
                                              _mm256_setzero_si256()};

                        s = src;
                        r = ref + x;

                        h = height;
                        do {
                            sad_loop_kernel_64_4sum_avx2(s, r, sums256);
                            s += src_stride;
                            r += ref_stride;
                        } while (--h);

                        update_leftover_2048_pel(
                            sums256, search_area_width, x, y, leftover, mask128, &best_s, &best_x, &best_y);
                    }

                    ref += src_stride_raw;
                } while (++y < search_area_height);
            } else {
                const uint32_t leftover16 = search_area_width & 15;

                y = 0;
                do {
                    for (x = 0; x <= search_area_width - 16; x += 16) {
                        __m512i sums512[8] = {_mm512_setzero_si512(),
                                              _mm512_setzero_si512(),
                                              _mm512_setzero_si512(),
                                              _mm512_setzero_si512(),
                                              _mm512_setzero_si512(),
                                              _mm512_setzero_si512(),
                                              _mm512_setzero_si512(),
                                              _mm512_setzero_si512()};

                        s = src;
                        r = ref + x;

                        h = height;
                        do {
                            sad_loop_kernel_32_4sum_avx512(s + 0 * 32, r + 0 * 32, sums512 + 0);
                            sad_loop_kernel_32_4sum_avx512(s + 1 * 32, r + 1 * 32, sums512 + 4);
                            s += src_stride;
                            r += ref_stride;
                        } while (--h);

                        const __m512i  sum512_01   = _mm512_adds_epu16(sums512[0], sums512[1]);
                        const __m512i  sum512_23   = _mm512_adds_epu16(sums512[2], sums512[3]);
                        const __m512i  sum512_45   = _mm512_adds_epu16(sums512[4], sums512[5]);
                        const __m512i  sum512_67   = _mm512_adds_epu16(sums512[6], sums512[7]);
                        const __m512i  sum512_0123 = _mm512_adds_epu16(sum512_01, sum512_23);
                        const __m512i  sum512_4567 = _mm512_adds_epu16(sum512_45, sum512_67);
                        const __m512i  sum512      = _mm512_adds_epu16(sum512_0123, sum512_4567);
                        const __m256i  sum_lo      = _mm512_castsi512_si256(sum512);
                        const __m256i  sum_hi      = _mm512_extracti64x4_epi64(sum512, 1);
                        const __m256i  sad         = _mm256_adds_epu16(sum_lo, sum_hi);
                        const __m128i  sad_lo      = _mm256_castsi256_si128(sad);
                        const __m128i  sad_hi      = _mm256_extracti128_si256(sad, 1);
                        const __m128i  minpos_lo   = _mm_minpos_epu16(sad_lo);
                        const __m128i  minpos_hi   = _mm_minpos_epu16(sad_hi);
                        const uint32_t min0        = _mm_extract_epi16(minpos_lo, 0);
                        const uint32_t min1        = _mm_extract_epi16(minpos_hi, 0);
                        uint32_t       minmin, delta;
                        __m128i        minpos;

                        if (min0 <= min1) {
                            minmin = min0;
                            delta  = 0;
                            minpos = minpos_lo;
                        } else {
                            minmin = min1;
                            delta  = 8;
                            minpos = minpos_hi;
                        }

                        if (minmin < best_s) {
                            if (minmin != 0xFFFF) { // no overflow
                                best_s = minmin;
                                best_x = x + delta + _mm_extract_epi16(minpos, 1);
                                best_y = y;
                            } else { // overflow
                                __m256i sads256[2];
                                __m128i sads128[2];

                                add16x16x8to32bit(sums512, sads256);

                                sads128[0] = _mm256_castsi256_si128(sads256[0]);
                                sads128[1] = _mm256_extracti128_si256(sads256[0], 1);
                                update_8_best(sads128, x + 0, y, &best_s, &best_x, &best_y);

                                sads128[0] = _mm256_castsi256_si128(sads256[1]);
                                sads128[1] = _mm256_extracti128_si256(sads256[1], 1);
                                update_8_best(sads128, x + 8, y, &best_s, &best_x, &best_y);
                            }
                        }
                    }

                    if (leftover16 >= 8) {
                        __m256i sums256[8] = {_mm256_setzero_si256(),
                                              _mm256_setzero_si256(),
                                              _mm256_setzero_si256(),
                                              _mm256_setzero_si256(),
                                              _mm256_setzero_si256(),
                                              _mm256_setzero_si256(),
                                              _mm256_setzero_si256(),
                                              _mm256_setzero_si256()};

                        s = src;
                        r = ref + x;

                        h = height;
                        do {
                            sad_loop_kernel_64_8sum_avx2(s, r, sums256);
                            s += src_stride;
                            r += ref_stride;
                        } while (--h);

                        const __m256i  sum01   = _mm256_adds_epu16(sums256[0], sums256[1]);
                        const __m256i  sum23   = _mm256_adds_epu16(sums256[2], sums256[3]);
                        const __m256i  sum45   = _mm256_adds_epu16(sums256[4], sums256[5]);
                        const __m256i  sum67   = _mm256_adds_epu16(sums256[6], sums256[7]);
                        const __m256i  sum0123 = _mm256_adds_epu16(sum01, sum23);
                        const __m256i  sum4567 = _mm256_adds_epu16(sum45, sum67);
                        const __m256i  sum256  = _mm256_adds_epu16(sum0123, sum4567);
                        const __m128i  sum_lo  = _mm256_castsi256_si128(sum256);
                        const __m128i  sum_hi  = _mm256_extracti128_si256(sum256, 1);
                        const __m128i  sad     = _mm_adds_epu16(sum_lo, sum_hi);
                        const __m128i  minpos  = _mm_minpos_epu16(sad);
                        const uint32_t min0    = _mm_extract_epi16(minpos, 0);

                        if (min0 < best_s) {
                            if (min0 != 0xFFFF) { // no overflow
                                best_s = min0;
                                best_x = x + _mm_extract_epi16(minpos, 1);
                                best_y = y;
                            } else { // overflow
                                __m128i sads[2];

                                add16x8x8to32bit(sums256, sads);
                                update_8_best(sads, x, y, &best_s, &best_x, &best_y);
                            }
                        }

                        x += 8;
                    }

                    if (leftover) {
                        __m256i sums256[8] = {_mm256_setzero_si256(),
                                              _mm256_setzero_si256(),
                                              _mm256_setzero_si256(),
                                              _mm256_setzero_si256(),
                                              _mm256_setzero_si256(),
                                              _mm256_setzero_si256(),
                                              _mm256_setzero_si256(),
                                              _mm256_setzero_si256()};

                        s = src;
                        r = ref + x;

                        h = height;
                        do {
                            sad_loop_kernel_64_8sum_avx2(s, r, sums256);
                            s += src_stride;
                            r += ref_stride;
                        } while (--h);

                        const __m256i  sum01   = _mm256_adds_epu16(sums256[0], sums256[1]);
                        const __m256i  sum23   = _mm256_adds_epu16(sums256[2], sums256[3]);
                        const __m256i  sum45   = _mm256_adds_epu16(sums256[4], sums256[5]);
                        const __m256i  sum67   = _mm256_adds_epu16(sums256[6], sums256[7]);
                        const __m256i  sum0123 = _mm256_adds_epu16(sum01, sum23);
                        const __m256i  sum4567 = _mm256_adds_epu16(sum45, sum67);
                        const __m256i  sum256  = _mm256_adds_epu16(sum0123, sum4567);
                        const __m128i  sum_lo  = _mm256_castsi256_si128(sum256);
                        const __m128i  sum_hi  = _mm256_extracti128_si256(sum256, 1);
                        const __m128i  sad0    = _mm_adds_epu16(sum_lo, sum_hi);
                        const __m128i  sad1    = _mm_or_si128(sad0, mask128);
                        const __m128i  minpos  = _mm_minpos_epu16(sad1);
                        const uint32_t min0    = _mm_extract_epi16(minpos, 0);

                        if (min0 < best_s) {
                            if (min0 != 0xFFFF) { // no overflow
                                best_s = min0;
                                best_x = x + _mm_extract_epi16(minpos, 1);
                                best_y = y;
                            } else { // overflow
                                const int32_t num = x + ((leftover < 4) ? leftover : 4);
                                __m128i       sads[2];

                                add16x8x8to32bit(sums256, sads);

                                do {
                                    UPDATE_BEST(sads[0], 0, x, best_s, best_x, best_y);
                                    sads[0] = _mm_srli_si128(sads[0], 4);
                                } while (++x < num);

                                while (x < search_area_width) {
                                    UPDATE_BEST(sads[1], 0, x, best_s, best_x, best_y);
                                    sads[1] = _mm_srli_si128(sads[1], 4);
                                    x++;
                                }
                            }
                        }
                    }

                    ref += src_stride_raw;
                } while (++y < search_area_height);
            }
            break;

        default:
            sad_loop_kernel_generalized_avx512(src,
                                               src_stride,
                                               ref,
                                               ref_stride,
                                               height,
                                               width,
                                               best_sad,
                                               x_search_center,
                                               y_search_center,
                                               src_stride_raw,
                                               search_area_width,
                                               search_area_height);
            return;
        }
    }

    *best_sad        = best_s;
    *x_search_center = (int16_t)best_x;
    *y_search_center = (int16_t)best_y;
}

uint32_t svt_nxm_sad_kernel_helper_avx512(const uint8_t* src, uint32_t src_stride, const uint8_t* ref,
                                          uint32_t ref_stride, uint32_t height, uint32_t width) {
    uint32_t nxm_sad = 0;

    switch (width) {
    case 4:
        nxm_sad = svt_compute4x_m_sad_avx2_intrin(src, src_stride, ref, ref_stride, height, width);
        break;
    case 8:
        nxm_sad = svt_compute8x_m_sad_avx2_intrin(src, src_stride, ref, ref_stride, height, width);
        break;
    case 16:
        nxm_sad = svt_compute16x_m_sad_avx2_intrin(src, src_stride, ref, ref_stride, height, width);
        break;
    case 24:
        nxm_sad = svt_compute24x_m_sad_avx2_intrin(src, src_stride, ref, ref_stride, height, width);
        break;
    case 32:
        nxm_sad = svt_compute32x_m_sad_avx2_intrin(src, src_stride, ref, ref_stride, height, width);
        break;
    case 40:
        nxm_sad = svt_compute40x_m_sad_avx2_intrin(src, src_stride, ref, ref_stride, height, width);
        break;
    case 48:
        nxm_sad = svt_compute48x_m_sad_avx2_intrin(src, src_stride, ref, ref_stride, height, width);
        break;
    case 56:
        nxm_sad = svt_compute56x_m_sad_avx2_intrin(src, src_stride, ref, ref_stride, height, width);
        break;
    case 64:
        nxm_sad = compute64x_m_sad_avx512_intrin(src, src_stride, ref, ref_stride, height);
        break;
    case 128:
        nxm_sad = compute128x_m_sad_avx512_intrin(src, src_stride, ref, ref_stride, height);
        break;
    default:
        nxm_sad = svt_nxm_sad_kernel_helper_c(src, src_stride, ref, ref_stride, height, width);
    }

    return nxm_sad;
}

#endif // EN_AVX512_SUPPORT
