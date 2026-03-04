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

#include <immintrin.h>
#include "highbd_intrapred_sse2.h"
#include "definitions.h"
#include "common_dsp_rtcd.h"
#include "intra_prediction.h"

// =============================================================================

// DC RELATED PRED

// Handle number of elements: up to 64.
static INLINE __m128i dc_sum_large(const __m256i src) {
    const __m128i s_lo = _mm256_castsi256_si128(src);
    const __m128i s_hi = _mm256_extracti128_si256(src, 1);
    __m128i       sum, sum_hi;
    sum    = _mm_add_epi16(s_lo, s_hi);
    sum_hi = _mm_srli_si128(sum, 8);
    sum    = _mm_add_epi16(sum, sum_hi);
    // Unpack to avoid 12-bit overflow.
    sum = _mm_unpacklo_epi16(sum, _mm_setzero_si128());

    return dc_sum_4x32bit(sum);
}

// Handle number of elements: 65 to 128.
static INLINE __m128i dc_sum_larger(const __m256i src) {
    const __m128i s_lo = _mm256_castsi256_si128(src);
    const __m128i s_hi = _mm256_extracti128_si256(src, 1);
    __m128i       sum, sum_hi;
    sum = _mm_add_epi16(s_lo, s_hi);
    // Unpack to avoid 12-bit overflow.
    sum_hi = _mm_unpackhi_epi16(sum, _mm_setzero_si128());
    sum    = _mm_unpacklo_epi16(sum, _mm_setzero_si128());
    sum    = _mm_add_epi32(sum, sum_hi);

    return dc_sum_4x32bit(sum);
}

static INLINE __m128i dc_sum_16(const uint16_t* const src) {
    const __m256i s    = _mm256_loadu_si256((const __m256i*)src);
    const __m128i s_lo = _mm256_castsi256_si128(s);
    const __m128i s_hi = _mm256_extracti128_si256(s, 1);
    const __m128i sum  = _mm_add_epi16(s_lo, s_hi);
    return dc_sum_8x16bit(sum);
}

static INLINE __m128i dc_sum_32(const uint16_t* const src) {
    const __m256i s0  = _mm256_loadu_si256((const __m256i*)(src + 0x00));
    const __m256i s1  = _mm256_loadu_si256((const __m256i*)(src + 0x10));
    const __m256i sum = _mm256_add_epi16(s0, s1);
    return dc_sum_large(sum);
}

static INLINE __m128i dc_sum_64(const uint16_t* const src) {
    const __m256i s0  = _mm256_loadu_si256((const __m256i*)(src + 0x00));
    const __m256i s1  = _mm256_loadu_si256((const __m256i*)(src + 0x10));
    const __m256i s2  = _mm256_loadu_si256((const __m256i*)(src + 0x20));
    const __m256i s3  = _mm256_loadu_si256((const __m256i*)(src + 0x30));
    const __m256i s01 = _mm256_add_epi16(s0, s1);
    const __m256i s23 = _mm256_add_epi16(s2, s3);
    const __m256i sum = _mm256_add_epi16(s01, s23);
    return dc_sum_large(sum);
}

static INLINE __m128i dc_sum_4_16(const uint16_t* const src_4, const uint16_t* const src_16) {
    const __m128i s_4         = _mm_loadl_epi64((const __m128i*)src_4);
    const __m256i s_16        = _mm256_loadu_si256((const __m256i*)src_16);
    const __m128i s_lo        = _mm256_castsi256_si128(s_16);
    const __m128i s_hi        = _mm256_extracti128_si256(s_16, 1);
    const __m128i s_16_sum0   = _mm_add_epi16(s_lo, s_hi);
    const __m128i s_16_sum_hi = _mm_srli_si128(s_16_sum0, 8);
    const __m128i s_16_sum    = _mm_add_epi16(s_16_sum0, s_16_sum_hi);
    const __m128i sum         = _mm_add_epi16(s_16_sum, s_4);
    return dc_sum_4x16bit_large(sum);
}

static INLINE __m128i dc_sum_8_16(const uint16_t* const src_8, const uint16_t* const src_16) {
    const __m128i s_8      = _mm_loadu_si128((const __m128i*)src_8);
    const __m256i s_16     = _mm256_loadu_si256((const __m256i*)src_16);
    const __m128i s_lo     = _mm256_castsi256_si128(s_16);
    const __m128i s_hi     = _mm256_extracti128_si256(s_16, 1);
    const __m128i s_16_sum = _mm_add_epi16(s_lo, s_hi);
    const __m128i sum      = _mm_add_epi16(s_16_sum, s_8);
    return dc_sum_8x16bit_large(sum);
}

static INLINE __m128i dc_sum_8_32(const uint16_t* const src_8, const uint16_t* const src_32) {
    const __m128i s_8      = _mm_loadu_si128((const __m128i*)src_8);
    const __m256i s_32_0   = _mm256_loadu_si256((const __m256i*)(src_32 + 0x00));
    const __m256i s_32_1   = _mm256_loadu_si256((const __m256i*)(src_32 + 0x10));
    const __m256i s_32     = _mm256_add_epi16(s_32_0, s_32_1);
    const __m128i s_lo     = _mm256_castsi256_si128(s_32);
    const __m128i s_hi     = _mm256_extracti128_si256(s_32, 1);
    const __m128i s_16_sum = _mm_add_epi16(s_lo, s_hi);
    const __m128i sum      = _mm_add_epi16(s_8, s_16_sum);
    return dc_sum_8x16bit_large(sum);
}

static INLINE __m128i dc_sum_16_16(const uint16_t* const src0, const uint16_t* const src1) {
    const __m256i s0  = _mm256_loadu_si256((const __m256i*)src0);
    const __m256i s1  = _mm256_loadu_si256((const __m256i*)src1);
    const __m256i sum = _mm256_add_epi16(s0, s1);
    return dc_sum_large(sum);
}

static INLINE __m128i dc_sum_16_32(const uint16_t* const src_16, const uint16_t* const src_32) {
    const __m256i s_16   = _mm256_loadu_si256((const __m256i*)src_16);
    const __m256i s_32_0 = _mm256_loadu_si256((const __m256i*)(src_32 + 0x00));
    const __m256i s_32_1 = _mm256_loadu_si256((const __m256i*)(src_32 + 0x10));
    const __m256i sum0   = _mm256_add_epi16(s_16, s_32_0);
    const __m256i sum    = _mm256_add_epi16(sum0, s_32_1);
    return dc_sum_large(sum);
}

static INLINE __m128i dc_sum_32_32(const uint16_t* const src0, const uint16_t* const src1) {
    const __m256i s0_0 = _mm256_loadu_si256((const __m256i*)(src0 + 0x00));
    const __m256i s0_1 = _mm256_loadu_si256((const __m256i*)(src0 + 0x10));
    const __m256i s1_0 = _mm256_loadu_si256((const __m256i*)(src1 + 0x00));
    const __m256i s1_1 = _mm256_loadu_si256((const __m256i*)(src1 + 0x10));
    const __m256i sum0 = _mm256_add_epi16(s0_0, s1_0);
    const __m256i sum1 = _mm256_add_epi16(s0_1, s1_1);
    const __m256i sum  = _mm256_add_epi16(sum0, sum1);
    return dc_sum_large(sum);
}

static INLINE __m128i dc_sum_32_64(const uint16_t* const src_32, const uint16_t* const src_64) {
    const __m256i s_32_0 = _mm256_loadu_si256((const __m256i*)(src_32 + 0x00));
    const __m256i s_32_1 = _mm256_loadu_si256((const __m256i*)(src_32 + 0x10));
    const __m256i s_64_0 = _mm256_loadu_si256((const __m256i*)(src_64 + 0x00));
    const __m256i s_64_1 = _mm256_loadu_si256((const __m256i*)(src_64 + 0x10));
    const __m256i s_64_2 = _mm256_loadu_si256((const __m256i*)(src_64 + 0x20));
    const __m256i s_64_3 = _mm256_loadu_si256((const __m256i*)(src_64 + 0x30));
    const __m256i sum0   = _mm256_add_epi16(s_32_0, s_64_0);
    const __m256i sum1   = _mm256_add_epi16(s_32_1, s_64_1);
    const __m256i sum2   = _mm256_add_epi16(s_64_2, s_64_3);
    const __m256i sum3   = _mm256_add_epi16(sum0, sum1);
    const __m256i sum    = _mm256_add_epi16(sum2, sum3);
    return dc_sum_larger(sum);
}

static INLINE __m128i dc_sum_64_64(const uint16_t* const src0, const uint16_t* const src1) {
    const __m256i s0_0 = _mm256_loadu_si256((const __m256i*)(src0 + 0x00));
    const __m256i s0_1 = _mm256_loadu_si256((const __m256i*)(src0 + 0x10));
    const __m256i s0_2 = _mm256_loadu_si256((const __m256i*)(src0 + 0x20));
    const __m256i s0_3 = _mm256_loadu_si256((const __m256i*)(src0 + 0x30));
    const __m256i s1_0 = _mm256_loadu_si256((const __m256i*)(src1 + 0x00));
    const __m256i s1_1 = _mm256_loadu_si256((const __m256i*)(src1 + 0x10));
    const __m256i s1_2 = _mm256_loadu_si256((const __m256i*)(src1 + 0x20));
    const __m256i s1_3 = _mm256_loadu_si256((const __m256i*)(src1 + 0x30));
    const __m256i sum0 = _mm256_add_epi16(s0_0, s1_0);
    const __m256i sum1 = _mm256_add_epi16(s0_1, s1_1);
    const __m256i sum2 = _mm256_add_epi16(s0_2, s1_2);
    const __m256i sum3 = _mm256_add_epi16(s0_3, s1_3);
    const __m256i sum4 = _mm256_add_epi16(sum0, sum1);
    const __m256i sum5 = _mm256_add_epi16(sum2, sum3);
    const __m256i sum  = _mm256_add_epi16(sum4, sum5);
    return dc_sum_larger(sum);
}

static INLINE __m128i dc_sum_16_64(const uint16_t* const src_16, const uint16_t* const src_64) {
    const __m256i s_16   = _mm256_loadu_si256((const __m256i*)src_16);
    const __m256i s_64_0 = _mm256_loadu_si256((const __m256i*)(src_64 + 0x00));
    const __m256i s_64_1 = _mm256_loadu_si256((const __m256i*)(src_64 + 0x10));
    const __m256i s_64_2 = _mm256_loadu_si256((const __m256i*)(src_64 + 0x20));
    const __m256i s_64_3 = _mm256_loadu_si256((const __m256i*)(src_64 + 0x30));
    const __m256i s0     = _mm256_add_epi16(s_16, s_64_0);
    const __m256i s1     = _mm256_add_epi16(s0, s_64_1);
    const __m256i s2     = _mm256_add_epi16(s_64_2, s_64_3);
    const __m256i sum    = _mm256_add_epi16(s1, s2);
    return dc_sum_larger(sum);
}

static INLINE void dc_common_predictor_16xh_kernel(uint16_t* dst, const ptrdiff_t stride, const int32_t h,
                                                   const __m256i dc) {
    for (int32_t i = 0; i < h; i++) {
        _mm256_storeu_si256((__m256i*)dst, dc);
        dst += stride;
    }
}

static INLINE void dc_common_predictor_32xh_kernel(uint16_t* dst, const ptrdiff_t stride, const int32_t h,
                                                   const __m256i dc) {
    for (int32_t i = 0; i < h; i++) {
        _mm256_storeu_si256((__m256i*)(dst + 0x00), dc);
        _mm256_storeu_si256((__m256i*)(dst + 0x10), dc);
        dst += stride;
    }
}

static INLINE void dc_common_predictor_64xh_kernel(uint16_t* dst, const ptrdiff_t stride, const int32_t h,
                                                   const __m256i dc) {
    for (int32_t i = 0; i < h; i++) {
        _mm256_storeu_si256((__m256i*)(dst + 0x00), dc);
        _mm256_storeu_si256((__m256i*)(dst + 0x10), dc);
        _mm256_storeu_si256((__m256i*)(dst + 0x20), dc);
        _mm256_storeu_si256((__m256i*)(dst + 0x30), dc);
        dst += stride;
    }
}

static INLINE void dc_common_predictor_16xh(uint16_t* const dst, const ptrdiff_t stride, const int32_t h,
                                            const __m128i dc) {
    const __m256i expected_dc = _mm256_broadcastw_epi16(dc);
    dc_common_predictor_16xh_kernel(dst, stride, h, expected_dc);
}

static INLINE void dc_common_predictor_32xh(uint16_t* const dst, const ptrdiff_t stride, const int32_t h,
                                            const __m128i dc) {
    const __m256i expected_dc = _mm256_broadcastw_epi16(dc);
    dc_common_predictor_32xh_kernel(dst, stride, h, expected_dc);
}

static INLINE void dc_common_predictor_64xh(uint16_t* const dst, const ptrdiff_t stride, const int32_t h,
                                            const __m128i dc) {
    const __m256i expected_dc = _mm256_broadcastw_epi16(dc);
    dc_common_predictor_64xh_kernel(dst, stride, h, expected_dc);
}

// =============================================================================

// DC_128_PRED

// 16xN

static INLINE void dc_128_predictor_16xh(uint16_t* const dst, const ptrdiff_t stride, const int32_t h,
                                         const int32_t bd) {
    const __m256i dc = _mm256_set1_epi16(1 << (bd - 1));
    dc_common_predictor_16xh_kernel(dst, stride, h, dc);
}

void svt_aom_highbd_dc_128_predictor_16x4_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                               const uint16_t* left, int32_t bd) {
    (void)above;
    (void)left;
    dc_128_predictor_16xh(dst, stride, 4, bd);
}

void svt_aom_highbd_dc_128_predictor_16x8_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                               const uint16_t* left, int32_t bd) {
    (void)above;
    (void)left;
    dc_128_predictor_16xh(dst, stride, 8, bd);
}

void svt_aom_highbd_dc_128_predictor_16x16_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                const uint16_t* left, int32_t bd) {
    (void)above;
    (void)left;
    dc_128_predictor_16xh(dst, stride, 16, bd);
}

void svt_aom_highbd_dc_128_predictor_16x32_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                const uint16_t* left, int32_t bd) {
    (void)above;
    (void)left;
    dc_128_predictor_16xh(dst, stride, 32, bd);
}

void svt_aom_highbd_dc_128_predictor_16x64_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                const uint16_t* left, int32_t bd) {
    (void)above;
    (void)left;
    dc_128_predictor_16xh(dst, stride, 64, bd);
}

// 32xN

static INLINE void dc_128_predictor_32xh(uint16_t* const dst, const ptrdiff_t stride, const int32_t h,
                                         const int32_t bd) {
    const __m256i dc = _mm256_set1_epi16(1 << (bd - 1));
    dc_common_predictor_32xh_kernel(dst, stride, h, dc);
}

void svt_aom_highbd_dc_128_predictor_32x8_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                               const uint16_t* left, int32_t bd) {
    (void)above;
    (void)left;
    dc_128_predictor_32xh(dst, stride, 8, bd);
}

void svt_aom_highbd_dc_128_predictor_32x16_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                const uint16_t* left, int32_t bd) {
    (void)above;
    (void)left;
    dc_128_predictor_32xh(dst, stride, 16, bd);
}

void svt_aom_highbd_dc_128_predictor_32x32_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                const uint16_t* left, int32_t bd) {
    (void)above;
    (void)left;
    dc_128_predictor_32xh(dst, stride, 32, bd);
}

void svt_aom_highbd_dc_128_predictor_32x64_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                const uint16_t* left, int32_t bd) {
    (void)above;
    (void)left;
    dc_128_predictor_32xh(dst, stride, 64, bd);
}

// 64xN

static INLINE void dc_128_predictor_64xh(uint16_t* const dst, const ptrdiff_t stride, const int32_t h,
                                         const int32_t bd) {
    const __m256i dc = _mm256_set1_epi16(1 << (bd - 1));
    dc_common_predictor_64xh_kernel(dst, stride, h, dc);
}

void svt_aom_highbd_dc_128_predictor_64x16_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                const uint16_t* left, int32_t bd) {
    (void)above;
    (void)left;
    dc_128_predictor_64xh(dst, stride, 16, bd);
}

void svt_aom_highbd_dc_128_predictor_64x32_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                const uint16_t* left, int32_t bd) {
    (void)above;
    (void)left;
    dc_128_predictor_64xh(dst, stride, 32, bd);
}

void svt_aom_highbd_dc_128_predictor_64x64_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                const uint16_t* left, int32_t bd) {
    (void)above;
    (void)left;
    dc_128_predictor_64xh(dst, stride, 64, bd);
}

// =============================================================================

// DC_LEFT_PRED

// 16xN

void svt_aom_highbd_dc_left_predictor_16x4_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                const uint16_t* left, int32_t bd) {
    const __m128i round = _mm_cvtsi32_si128(2);
    __m128i       sum;
    (void)above;
    (void)bd;

    sum = dc_sum_4(left);
    sum = _mm_add_epi16(sum, round);
    sum = _mm_srli_epi16(sum, 2);
    dc_common_predictor_16xh(dst, stride, 4, sum);
}

void svt_aom_highbd_dc_left_predictor_16x8_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                const uint16_t* left, int32_t bd) {
    const __m128i round = _mm_cvtsi32_si128(4);
    __m128i       sum;
    (void)above;
    (void)bd;

    sum = dc_sum_8(left);
    sum = _mm_add_epi16(sum, round);
    sum = _mm_srli_epi16(sum, 3);
    dc_common_predictor_16xh(dst, stride, 8, sum);
}

void svt_aom_highbd_dc_left_predictor_16x16_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                 const uint16_t* left, int32_t bd) {
    const __m128i round = _mm_cvtsi32_si128(8);
    __m128i       sum;
    (void)above;
    (void)bd;

    sum = dc_sum_16(left);
    sum = _mm_add_epi16(sum, round);
    sum = _mm_srli_epi16(sum, 4);
    dc_common_predictor_16xh(dst, stride, 16, sum);
}

void svt_aom_highbd_dc_left_predictor_16x32_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                 const uint16_t* left, int32_t bd) {
    const __m128i round = _mm_cvtsi32_si128(16);
    __m128i       sum;
    (void)above;
    (void)bd;

    sum = dc_sum_32(left);
    sum = _mm_add_epi32(sum, round);
    sum = _mm_srli_epi32(sum, 5);
    dc_common_predictor_16xh(dst, stride, 32, sum);
}

void svt_aom_highbd_dc_left_predictor_16x64_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                 const uint16_t* left, int32_t bd) {
    const __m128i round = _mm_cvtsi32_si128(32);
    __m128i       sum;
    (void)above;
    (void)bd;

    sum = dc_sum_64(left);
    sum = _mm_add_epi32(sum, round);
    sum = _mm_srli_epi32(sum, 6);
    dc_common_predictor_16xh(dst, stride, 64, sum);
}

// 32xN

void svt_aom_highbd_dc_left_predictor_32x8_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                const uint16_t* left, int32_t bd) {
    const __m128i round = _mm_cvtsi32_si128(4);
    __m128i       sum;
    (void)above;
    (void)bd;

    sum = dc_sum_8(left);
    sum = _mm_add_epi16(sum, round);
    sum = _mm_srli_epi16(sum, 3);
    dc_common_predictor_32xh(dst, stride, 8, sum);
}

void svt_aom_highbd_dc_left_predictor_32x16_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                 const uint16_t* left, int32_t bd) {
    const __m128i round = _mm_cvtsi32_si128(8);
    __m128i       sum;
    (void)above;
    (void)bd;

    sum = dc_sum_16(left);
    sum = _mm_add_epi16(sum, round);
    sum = _mm_srli_epi16(sum, 4);
    dc_common_predictor_32xh(dst, stride, 16, sum);
}

void svt_aom_highbd_dc_left_predictor_32x32_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                 const uint16_t* left, int32_t bd) {
    const __m128i round = _mm_cvtsi32_si128(16);
    __m128i       sum;
    (void)above;
    (void)bd;

    sum = dc_sum_32(left);
    sum = _mm_add_epi32(sum, round);
    sum = _mm_srli_epi32(sum, 5);
    dc_common_predictor_32xh(dst, stride, 32, sum);
}

void svt_aom_highbd_dc_left_predictor_32x64_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                 const uint16_t* left, int32_t bd) {
    const __m128i round = _mm_cvtsi32_si128(32);
    __m128i       sum;
    (void)above;
    (void)bd;

    sum = dc_sum_64(left);
    sum = _mm_add_epi32(sum, round);
    sum = _mm_srli_epi32(sum, 6);
    dc_common_predictor_32xh(dst, stride, 64, sum);
}

// 64xN

void svt_aom_highbd_dc_left_predictor_64x16_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                 const uint16_t* left, int32_t bd) {
    const __m128i round = _mm_cvtsi32_si128(8);
    __m128i       sum;
    (void)above;
    (void)bd;

    sum = dc_sum_16(left);
    sum = _mm_add_epi16(sum, round);
    sum = _mm_srli_epi16(sum, 4);
    dc_common_predictor_64xh(dst, stride, 16, sum);
}

void svt_aom_highbd_dc_left_predictor_64x32_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                 const uint16_t* left, int32_t bd) {
    const __m128i round = _mm_cvtsi32_si128(16);
    __m128i       sum;
    (void)above;
    (void)bd;

    sum = dc_sum_32(left);
    sum = _mm_add_epi32(sum, round);
    sum = _mm_srli_epi32(sum, 5);
    dc_common_predictor_64xh(dst, stride, 32, sum);
}

void svt_aom_highbd_dc_left_predictor_64x64_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                 const uint16_t* left, int32_t bd) {
    const __m128i round = _mm_cvtsi32_si128(32);
    __m128i       sum;
    (void)above;
    (void)bd;

    sum = dc_sum_64(left);
    sum = _mm_add_epi32(sum, round);
    sum = _mm_srli_epi32(sum, 6);
    dc_common_predictor_64xh(dst, stride, 64, sum);
}

// =============================================================================

// DC_TOP_PRED

// 16xN

static INLINE void dc_top_predictor_16xh(uint16_t* const dst, const ptrdiff_t stride, const uint16_t* const above,
                                         const int32_t h, const int32_t bd) {
    (void)bd;
    const __m128i round = _mm_cvtsi32_si128(8);
    __m128i       sum;

    sum = dc_sum_16(above);
    sum = _mm_add_epi16(sum, round);
    sum = _mm_srli_epi16(sum, 4);
    dc_common_predictor_16xh(dst, stride, h, sum);
}

void svt_aom_highbd_dc_top_predictor_16x4_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                               const uint16_t* left, int32_t bd) {
    (void)left;
    dc_top_predictor_16xh(dst, stride, above, 4, bd);
}

void svt_aom_highbd_dc_top_predictor_16x8_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                               const uint16_t* left, int32_t bd) {
    (void)left;
    dc_top_predictor_16xh(dst, stride, above, 8, bd);
}

void svt_aom_highbd_dc_top_predictor_16x16_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                const uint16_t* left, int32_t bd) {
    (void)left;
    dc_top_predictor_16xh(dst, stride, above, 16, bd);
}

void svt_aom_highbd_dc_top_predictor_16x32_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                const uint16_t* left, int32_t bd) {
    (void)left;
    dc_top_predictor_16xh(dst, stride, above, 32, bd);
}

void svt_aom_highbd_dc_top_predictor_16x64_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                const uint16_t* left, int32_t bd) {
    (void)left;
    dc_top_predictor_16xh(dst, stride, above, 64, bd);
}

// 32xN

static INLINE void dc_top_predictor_32xh(uint16_t* const dst, const ptrdiff_t stride, const uint16_t* const above,
                                         const int32_t h, const int32_t bd) {
    const __m128i round = _mm_cvtsi32_si128(16);
    __m128i       sum;
    (void)bd;

    sum = dc_sum_32(above);
    sum = _mm_add_epi32(sum, round);
    sum = _mm_srli_epi32(sum, 5);
    dc_common_predictor_32xh(dst, stride, h, sum);
}

void svt_aom_highbd_dc_top_predictor_32x8_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                               const uint16_t* left, int32_t bd) {
    (void)left;
    dc_top_predictor_32xh(dst, stride, above, 8, bd);
}

void svt_aom_highbd_dc_top_predictor_32x16_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                const uint16_t* left, int32_t bd) {
    (void)left;
    dc_top_predictor_32xh(dst, stride, above, 16, bd);
}

void svt_aom_highbd_dc_top_predictor_32x32_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                const uint16_t* left, int32_t bd) {
    (void)left;
    dc_top_predictor_32xh(dst, stride, above, 32, bd);
}

void svt_aom_highbd_dc_top_predictor_32x64_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                const uint16_t* left, int32_t bd) {
    (void)left;
    dc_top_predictor_32xh(dst, stride, above, 64, bd);
}

// 64xN

static INLINE void dc_top_predictor_64xh(uint16_t* const dst, const ptrdiff_t stride, const uint16_t* const above,
                                         const int32_t h, const int32_t bd) {
    const __m128i round = _mm_cvtsi32_si128(32);
    __m128i       sum;
    (void)bd;

    sum = dc_sum_64(above);
    sum = _mm_add_epi32(sum, round);
    sum = _mm_srli_epi32(sum, 6);
    dc_common_predictor_64xh(dst, stride, h, sum);
}

void svt_aom_highbd_dc_top_predictor_64x16_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                const uint16_t* left, int32_t bd) {
    (void)left;
    dc_top_predictor_64xh(dst, stride, above, 16, bd);
}

void svt_aom_highbd_dc_top_predictor_64x32_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                const uint16_t* left, int32_t bd) {
    (void)left;
    dc_top_predictor_64xh(dst, stride, above, 32, bd);
}

void svt_aom_highbd_dc_top_predictor_64x64_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                const uint16_t* left, int32_t bd) {
    (void)left;
    dc_top_predictor_64xh(dst, stride, above, 64, bd);
}

// =============================================================================

// DC_PRED

// 16xN

void svt_aom_highbd_dc_predictor_16x4_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above, const uint16_t* left,
                                           int32_t bd) {
    (void)bd;
    __m128i  sum   = dc_sum_4_16(left, above);
    uint32_t sum32 = _mm_cvtsi128_si32(sum);
    sum32 += 10;
    sum32 /= 20;
    const __m256i dc = _mm256_set1_epi16((int16_t)sum32);

    dc_common_predictor_16xh_kernel(dst, stride, 4, dc);
}

void svt_aom_highbd_dc_predictor_16x8_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above, const uint16_t* left,
                                           int32_t bd) {
    (void)bd;
    __m128i  sum   = dc_sum_8_16(left, above);
    uint32_t sum32 = _mm_cvtsi128_si32(sum);
    sum32 += 12;
    sum32 /= 24;
    const __m256i dc = _mm256_set1_epi16((int16_t)sum32);

    dc_common_predictor_16xh_kernel(dst, stride, 8, dc);
}

void svt_aom_highbd_dc_predictor_16x16_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                            const uint16_t* left, int32_t bd) {
    (void)bd;
    __m128i sum = dc_sum_16_16(above, left);
    sum         = _mm_add_epi32(sum, _mm_set1_epi32(16));
    sum         = _mm_srli_epi32(sum, 5);
    dc_common_predictor_16xh(dst, stride, 16, sum);
}

void svt_aom_highbd_dc_predictor_16x32_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                            const uint16_t* left, int32_t bd) {
    (void)bd;
    __m128i  sum   = dc_sum_16_32(above, left);
    uint32_t sum32 = _mm_cvtsi128_si32(sum);
    sum32 += 24;
    sum32 /= 48;
    const __m256i dc = _mm256_set1_epi16((int16_t)sum32);

    dc_common_predictor_16xh_kernel(dst, stride, 32, dc);
}

void svt_aom_highbd_dc_predictor_16x64_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                            const uint16_t* left, int32_t bd) {
    (void)bd;
    __m128i  sum   = dc_sum_16_64(above, left);
    uint32_t sum32 = _mm_cvtsi128_si32(sum);
    sum32 += 40;
    sum32 /= 80;
    const __m256i dc = _mm256_set1_epi16((int16_t)sum32);

    dc_common_predictor_16xh_kernel(dst, stride, 64, dc);
}

// 32xN

void svt_aom_highbd_dc_predictor_32x8_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above, const uint16_t* left,
                                           int32_t bd) {
    (void)bd;
    __m128i  sum   = dc_sum_8_32(left, above);
    uint32_t sum32 = _mm_cvtsi128_si32(sum);
    sum32 += 20;
    sum32 /= 40;
    const __m256i dc = _mm256_set1_epi16((int16_t)sum32);

    dc_common_predictor_32xh_kernel(dst, stride, 8, dc);
}

void svt_aom_highbd_dc_predictor_32x16_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                            const uint16_t* left, int32_t bd) {
    (void)bd;
    __m128i  sum   = dc_sum_16_32(left, above);
    uint32_t sum32 = _mm_cvtsi128_si32(sum);
    sum32 += 24;
    sum32 /= 48;
    const __m256i dc = _mm256_set1_epi16((int16_t)sum32);

    dc_common_predictor_32xh_kernel(dst, stride, 16, dc);
}

void svt_aom_highbd_dc_predictor_32x32_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                            const uint16_t* left, int32_t bd) {
    (void)bd;
    __m128i sum = dc_sum_32_32(above, left);
    sum         = _mm_add_epi32(sum, _mm_set1_epi32(32));
    sum         = _mm_srli_epi32(sum, 6);
    dc_common_predictor_32xh(dst, stride, 32, sum);
}

void svt_aom_highbd_dc_predictor_32x64_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                            const uint16_t* left, int32_t bd) {
    (void)bd;
    __m128i  sum   = dc_sum_32_64(above, left);
    uint32_t sum32 = _mm_cvtsi128_si32(sum);
    sum32 += 48;
    sum32 /= 96;
    const __m256i dc = _mm256_set1_epi16((int16_t)sum32);

    dc_common_predictor_32xh_kernel(dst, stride, 64, dc);
}

// 64xN

void svt_aom_highbd_dc_predictor_64x16_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                            const uint16_t* left, int32_t bd) {
    (void)bd;
    __m128i  sum   = dc_sum_16_64(left, above);
    uint32_t sum32 = _mm_cvtsi128_si32(sum);
    sum32 += 40;
    sum32 /= 80;
    const __m256i dc = _mm256_set1_epi16((int16_t)sum32);

    dc_common_predictor_64xh_kernel(dst, stride, 16, dc);
}

void svt_aom_highbd_dc_predictor_64x32_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                            const uint16_t* left, int32_t bd) {
    (void)bd;
    __m128i  sum   = dc_sum_32_64(left, above);
    uint32_t sum32 = _mm_cvtsi128_si32(sum);
    sum32 += 48;
    sum32 /= 96;
    const __m256i dc = _mm256_set1_epi16((int16_t)sum32);

    dc_common_predictor_64xh_kernel(dst, stride, 32, dc);
}

void svt_aom_highbd_dc_predictor_64x64_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                            const uint16_t* left, int32_t bd) {
    (void)bd;
    __m128i sum = dc_sum_64_64(above, left);
    sum         = _mm_add_epi32(sum, _mm_set1_epi32(64));
    sum         = _mm_srli_epi32(sum, 7);
    dc_common_predictor_64xh(dst, stride, 64, sum);
}

// =============================================================================

// H_PRED

// 16xN

static INLINE void h_pred_16(uint16_t** const dst, const ptrdiff_t stride, const __m128i left) {
    // Broadcast the 16-bit left pixel to 256-bit register.
    const __m256i row = _mm256_broadcastw_epi16(left);

    _mm256_storeu_si256((__m256i*)(*dst + 0x00), row);
    *dst += stride;
}

// Process 8 rows.
static INLINE void h_pred_16x8(uint16_t** dst, const ptrdiff_t stride, const uint16_t* const left) {
    const __m128i left_u16 = _mm_loadu_si128((const __m128i*)left);

    h_pred_16(dst, stride, _mm_srli_si128(left_u16, 0));
    h_pred_16(dst, stride, _mm_srli_si128(left_u16, 2));
    h_pred_16(dst, stride, _mm_srli_si128(left_u16, 4));
    h_pred_16(dst, stride, _mm_srli_si128(left_u16, 6));
    h_pred_16(dst, stride, _mm_srli_si128(left_u16, 8));
    h_pred_16(dst, stride, _mm_srli_si128(left_u16, 10));
    h_pred_16(dst, stride, _mm_srli_si128(left_u16, 12));
    h_pred_16(dst, stride, _mm_srli_si128(left_u16, 14));
}

// 16x4

void svt_aom_highbd_h_predictor_16x4_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above, const uint16_t* left,
                                          int32_t bd) {
    (void)above;
    (void)bd;
    const __m128i left_u16 = _mm_loadl_epi64((const __m128i*)left);

    h_pred_16(&dst, stride, _mm_srli_si128(left_u16, 0));
    h_pred_16(&dst, stride, _mm_srli_si128(left_u16, 2));
    h_pred_16(&dst, stride, _mm_srli_si128(left_u16, 4));
    h_pred_16(&dst, stride, _mm_srli_si128(left_u16, 6));
}

// 16x64

void svt_aom_highbd_h_predictor_16x64_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above, const uint16_t* left,
                                           int32_t bd) {
    (void)above;
    (void)bd;

    for (int32_t i = 0; i < 8; i++, left += 8) {
        h_pred_16x8(&dst, stride, left);
    }
}

// -----------------------------------------------------------------------------

// 32xN

static INLINE void h_pred_32(uint16_t** const dst, const ptrdiff_t stride, const __m128i left) {
    // Broadcast the 16-bit left pixel to 256-bit register.
    const __m256i row = _mm256_broadcastw_epi16(left);

    _mm256_storeu_si256((__m256i*)(*dst + 0x00), row);
    _mm256_storeu_si256((__m256i*)(*dst + 0x10), row);
    *dst += stride;
}

// Process 8 rows.
static INLINE void h_pred_32x8(uint16_t** dst, const ptrdiff_t stride, const uint16_t* const left) {
    const __m128i left_u16 = _mm_loadu_si128((const __m128i*)left);

    h_pred_32(dst, stride, _mm_srli_si128(left_u16, 0));
    h_pred_32(dst, stride, _mm_srli_si128(left_u16, 2));
    h_pred_32(dst, stride, _mm_srli_si128(left_u16, 4));
    h_pred_32(dst, stride, _mm_srli_si128(left_u16, 6));
    h_pred_32(dst, stride, _mm_srli_si128(left_u16, 8));
    h_pred_32(dst, stride, _mm_srli_si128(left_u16, 10));
    h_pred_32(dst, stride, _mm_srli_si128(left_u16, 12));
    h_pred_32(dst, stride, _mm_srli_si128(left_u16, 14));
}

// 32x8

void svt_aom_highbd_h_predictor_32x8_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above, const uint16_t* left,
                                          int32_t bd) {
    (void)above;
    (void)bd;

    h_pred_32x8(&dst, stride, left);
}

// 32x64

void svt_aom_highbd_h_predictor_32x64_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above, const uint16_t* left,
                                           int32_t bd) {
    (void)above;
    (void)bd;

    for (int32_t i = 0; i < 8; i++, left += 8) {
        h_pred_32x8(&dst, stride, left);
    }
}

// -----------------------------------------------------------------------------

// 64xN

static INLINE void h_pred_64(uint16_t** const dst, const ptrdiff_t stride, const __m128i left) {
    // Broadcast the 16-bit left pixel to 256-bit register.
    const __m256i row = _mm256_broadcastw_epi16(left);

    _mm256_storeu_si256((__m256i*)(*dst + 0x00), row);
    _mm256_storeu_si256((__m256i*)(*dst + 0x10), row);
    _mm256_storeu_si256((__m256i*)(*dst + 0x20), row);
    _mm256_storeu_si256((__m256i*)(*dst + 0x30), row);
    *dst += stride;
}

// Process 8 rows.
static INLINE void h_pred_64x8(uint16_t** dst, const ptrdiff_t stride, const uint16_t* const left) {
    const __m128i left_u16 = _mm_loadu_si128((const __m128i*)left);

    h_pred_64(dst, stride, _mm_srli_si128(left_u16, 0));
    h_pred_64(dst, stride, _mm_srli_si128(left_u16, 2));
    h_pred_64(dst, stride, _mm_srli_si128(left_u16, 4));
    h_pred_64(dst, stride, _mm_srli_si128(left_u16, 6));
    h_pred_64(dst, stride, _mm_srli_si128(left_u16, 8));
    h_pred_64(dst, stride, _mm_srli_si128(left_u16, 10));
    h_pred_64(dst, stride, _mm_srli_si128(left_u16, 12));
    h_pred_64(dst, stride, _mm_srli_si128(left_u16, 14));
}

// 64x16

void svt_aom_highbd_h_predictor_64x16_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above, const uint16_t* left,
                                           int32_t bd) {
    (void)above;
    (void)bd;

    for (int32_t i = 0; i < 2; i++, left += 8) {
        h_pred_64x8(&dst, stride, left);
    }
}

// 64x32

void svt_aom_highbd_h_predictor_64x32_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above, const uint16_t* left,
                                           int32_t bd) {
    (void)above;
    (void)bd;

    for (int32_t i = 0; i < 4; i++, left += 8) {
        h_pred_64x8(&dst, stride, left);
    }
}

// 64x64

void svt_aom_highbd_h_predictor_64x64_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above, const uint16_t* left,
                                           int32_t bd) {
    (void)above;
    (void)bd;

    for (int32_t i = 0; i < 8; i++, left += 8) {
        h_pred_64x8(&dst, stride, left);
    }
}

// =============================================================================

// V_PRED

// 16xN

static INLINE void v_pred_16(uint16_t** const dst, const ptrdiff_t stride, const __m256i above0) {
    _mm256_storeu_si256((__m256i*)(*dst + 0x00), above0);
    *dst += stride;
}

// Process 8 rows.
static INLINE void v_pred_16x8(uint16_t** const dst, const ptrdiff_t stride, const __m256i above) {
    v_pred_16(dst, stride, above);
    v_pred_16(dst, stride, above);
    v_pred_16(dst, stride, above);
    v_pred_16(dst, stride, above);
    v_pred_16(dst, stride, above);
    v_pred_16(dst, stride, above);
    v_pred_16(dst, stride, above);
    v_pred_16(dst, stride, above);
}

// 16x4

void svt_aom_highbd_v_predictor_16x4_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above, const uint16_t* left,
                                          int32_t bd) {
    // Load all 16 pixels in a row into 256-bit registers.
    const __m256i above0 = _mm256_loadu_si256((const __m256i*)(above + 0x00));

    (void)left;
    (void)bd;

    v_pred_16(&dst, stride, above0);
    v_pred_16(&dst, stride, above0);
    v_pred_16(&dst, stride, above0);
    v_pred_16(&dst, stride, above0);
}

// 16x8

void svt_aom_highbd_v_predictor_16x8_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above, const uint16_t* left,
                                          int32_t bd) {
    // Load all 16 pixels in a row into 256-bit registers.
    const __m256i above0 = _mm256_loadu_si256((const __m256i*)(above + 0x00));

    (void)left;
    (void)bd;

    v_pred_16x8(&dst, stride, above0);
}

// 16x16

void svt_aom_highbd_v_predictor_16x16_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above, const uint16_t* left,
                                           int32_t bd) {
    // Load all 16 pixels in a row into 256-bit registers.
    const __m256i above0 = _mm256_loadu_si256((const __m256i*)(above + 0x00));

    (void)left;
    (void)bd;

    for (int32_t i = 0; i < 2; i++) {
        v_pred_16x8(&dst, stride, above0);
    }
}

// 16x32

void svt_aom_highbd_v_predictor_16x32_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above, const uint16_t* left,
                                           int32_t bd) {
    // Load all 16 pixels in a row into 256-bit registers.
    const __m256i above0 = _mm256_loadu_si256((const __m256i*)(above + 0x00));

    (void)left;
    (void)bd;

    for (int32_t i = 0; i < 4; i++) {
        v_pred_16x8(&dst, stride, above0);
    }
}

// 16x64

void svt_aom_highbd_v_predictor_16x64_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above, const uint16_t* left,
                                           int32_t bd) {
    // Load all 16 pixels in a row into 256-bit registers.
    const __m256i above0 = _mm256_loadu_si256((const __m256i*)(above + 0x00));

    (void)left;
    (void)bd;

    for (int32_t i = 0; i < 8; i++) {
        v_pred_16x8(&dst, stride, above0);
    }
}

// -----------------------------------------------------------------------------

// 32xN

static INLINE void v_pred_32(uint16_t** const dst, const ptrdiff_t stride, const __m256i above0, const __m256i above1) {
    _mm256_storeu_si256((__m256i*)(*dst + 0x00), above0);
    _mm256_storeu_si256((__m256i*)(*dst + 0x10), above1);
    *dst += stride;
}

// Process 8 rows.
static INLINE void v_pred_32x8(uint16_t** const dst, const ptrdiff_t stride, const __m256i above0,
                               const __m256i above1) {
    v_pred_32(dst, stride, above0, above1);
    v_pred_32(dst, stride, above0, above1);
    v_pred_32(dst, stride, above0, above1);
    v_pred_32(dst, stride, above0, above1);
    v_pred_32(dst, stride, above0, above1);
    v_pred_32(dst, stride, above0, above1);
    v_pred_32(dst, stride, above0, above1);
    v_pred_32(dst, stride, above0, above1);
}

// 32x8

void svt_aom_highbd_v_predictor_32x8_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above, const uint16_t* left,
                                          int32_t bd) {
    // Load all 32 pixels in a row into 256-bit registers.
    const __m256i above0 = _mm256_loadu_si256((const __m256i*)(above + 0x00));
    const __m256i above1 = _mm256_loadu_si256((const __m256i*)(above + 0x10));

    (void)left;
    (void)bd;

    v_pred_32x8(&dst, stride, above0, above1);
}

// 32x16

void svt_aom_highbd_v_predictor_32x16_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above, const uint16_t* left,
                                           int32_t bd) {
    // Load all 32 pixels in a row into 256-bit registers.
    const __m256i above0 = _mm256_loadu_si256((const __m256i*)(above + 0x00));
    const __m256i above1 = _mm256_loadu_si256((const __m256i*)(above + 0x10));

    (void)left;
    (void)bd;

    for (int32_t i = 0; i < 2; i++) {
        v_pred_32x8(&dst, stride, above0, above1);
    }
}

// 32x32

void svt_aom_highbd_v_predictor_32x32_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above, const uint16_t* left,
                                           int32_t bd) {
    // Load all 32 pixels in a row into 256-bit registers.
    const __m256i above0 = _mm256_loadu_si256((const __m256i*)(above + 0x00));
    const __m256i above1 = _mm256_loadu_si256((const __m256i*)(above + 0x10));

    (void)left;
    (void)bd;

    for (int32_t i = 0; i < 4; i++) {
        v_pred_32x8(&dst, stride, above0, above1);
    }
}

// 32x64

void svt_aom_highbd_v_predictor_32x64_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above, const uint16_t* left,
                                           int32_t bd) {
    // Load all 32 pixels in a row into 256-bit registers.
    const __m256i above0 = _mm256_loadu_si256((const __m256i*)(above + 0x00));
    const __m256i above1 = _mm256_loadu_si256((const __m256i*)(above + 0x10));

    (void)left;
    (void)bd;

    for (int32_t i = 0; i < 8; i++) {
        v_pred_32x8(&dst, stride, above0, above1);
    }
}

// -----------------------------------------------------------------------------

// 64xN

static INLINE void v_pred_64(uint16_t** const dst, const ptrdiff_t stride, const __m256i above0, const __m256i above1,
                             const __m256i above2, const __m256i above3) {
    _mm256_storeu_si256((__m256i*)(*dst + 0x00), above0);
    _mm256_storeu_si256((__m256i*)(*dst + 0x10), above1);
    _mm256_storeu_si256((__m256i*)(*dst + 0x20), above2);
    _mm256_storeu_si256((__m256i*)(*dst + 0x30), above3);
    *dst += stride;
}

// Process 8 rows.
static INLINE void v_pred_64x8(uint16_t** const dst, const ptrdiff_t stride, const __m256i above0, const __m256i above1,
                               const __m256i above2, const __m256i above3) {
    v_pred_64(dst, stride, above0, above1, above2, above3);
    v_pred_64(dst, stride, above0, above1, above2, above3);
    v_pred_64(dst, stride, above0, above1, above2, above3);
    v_pred_64(dst, stride, above0, above1, above2, above3);
    v_pred_64(dst, stride, above0, above1, above2, above3);
    v_pred_64(dst, stride, above0, above1, above2, above3);
    v_pred_64(dst, stride, above0, above1, above2, above3);
    v_pred_64(dst, stride, above0, above1, above2, above3);
}

// 64x16

void svt_aom_highbd_v_predictor_64x16_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above, const uint16_t* left,
                                           int32_t bd) {
    // Load all 64 pixels in a row into 256-bit registers.
    const __m256i above0 = _mm256_loadu_si256((const __m256i*)(above + 0x00));
    const __m256i above1 = _mm256_loadu_si256((const __m256i*)(above + 0x10));
    const __m256i above2 = _mm256_loadu_si256((const __m256i*)(above + 0x20));
    const __m256i above3 = _mm256_loadu_si256((const __m256i*)(above + 0x30));

    (void)left;
    (void)bd;

    for (int32_t i = 0; i < 2; i++) {
        v_pred_64x8(&dst, stride, above0, above1, above2, above3);
    }
}

// 64x32

void svt_aom_highbd_v_predictor_64x32_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above, const uint16_t* left,
                                           int32_t bd) {
    // Load all 64 pixels in a row into 256-bit registers.
    const __m256i above0 = _mm256_loadu_si256((const __m256i*)(above + 0x00));
    const __m256i above1 = _mm256_loadu_si256((const __m256i*)(above + 0x10));
    const __m256i above2 = _mm256_loadu_si256((const __m256i*)(above + 0x20));
    const __m256i above3 = _mm256_loadu_si256((const __m256i*)(above + 0x30));

    (void)left;
    (void)bd;

    for (int32_t i = 0; i < 4; i++) {
        v_pred_64x8(&dst, stride, above0, above1, above2, above3);
    }
}

// 64x64

void svt_aom_highbd_v_predictor_64x64_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above, const uint16_t* left,
                                           int32_t bd) {
    // Load all 64 pixels in a row into 256-bit registers.
    const __m256i above0 = _mm256_loadu_si256((const __m256i*)(above + 0x00));
    const __m256i above1 = _mm256_loadu_si256((const __m256i*)(above + 0x10));
    const __m256i above2 = _mm256_loadu_si256((const __m256i*)(above + 0x20));
    const __m256i above3 = _mm256_loadu_si256((const __m256i*)(above + 0x30));

    (void)left;
    (void)bd;

    for (int32_t i = 0; i < 8; i++) {
        v_pred_64x8(&dst, stride, above0, above1, above2, above3);
    }
}

// =============================================================================

// Repeat for AVX2 optimizations.

// bs = 4
EB_ALIGN(32)
static const uint16_t sm_weights_d_4[16] = {
    255,
    1,
    149,
    107,
    85,
    171,
    64,
    192, //  0  1  2  3
    255,
    1,
    149,
    107,
    85,
    171,
    64,
    192 //  0  1  2  3
};

// bs = 8
EB_ALIGN(32)
static const uint16_t sm_weights_d_8[32] = {
    255, 1,   197, 59,  146, 110, 105, 151, //  0  1  2  3
    255, 1,   197, 59,  146, 110, 105, 151, //  0  1  2  3
    73,  183, 50,  206, 37,  219, 32,  224, //  4  5  6  7
    73,  183, 50,  206, 37,  219, 32,  224 //  4  5  6  7
};

// bs = 16
EB_ALIGN(32)
static const uint16_t sm_weights_d_16[64] = {
    255, 1,   225, 31,  196, 60,  170, 86, //  0  1  2  3
    255, 1,   225, 31,  196, 60,  170, 86, //  0  1  2  3
    145, 111, 123, 133, 102, 154, 84,  172, //  4  5  6  7
    145, 111, 123, 133, 102, 154, 84,  172, //  4  5  6  7
    68,  188, 54,  202, 43,  213, 33,  223, //  8  9 10 11
    68,  188, 54,  202, 43,  213, 33,  223, //  8  9 10 11
    26,  230, 20,  236, 17,  239, 16,  240, // 12 13 14 15
    26,  230, 20,  236, 17,  239, 16,  240 // 12 13 14 15
};

// bs = 32
EB_ALIGN(32)
static const uint16_t sm_weights_d_32[128] = {
    255, 1,   240, 16,  225, 31,  210, 46, //  0  1  2  3
    255, 1,   240, 16,  225, 31,  210, 46, //  0  1  2  3
    196, 60,  182, 74,  169, 87,  157, 99, //  4  5  6  7
    196, 60,  182, 74,  169, 87,  157, 99, //  4  5  6  7
    145, 111, 133, 123, 122, 134, 111, 145, //  8  9 10 11
    145, 111, 133, 123, 122, 134, 111, 145, //  8  9 10 11
    101, 155, 92,  164, 83,  173, 74,  182, // 12 13 14 15
    101, 155, 92,  164, 83,  173, 74,  182, // 12 13 14 15
    66,  190, 59,  197, 52,  204, 45,  211, // 16 17 18 19
    66,  190, 59,  197, 52,  204, 45,  211, // 16 17 18 19
    39,  217, 34,  222, 29,  227, 25,  231, // 20 21 22 23
    39,  217, 34,  222, 29,  227, 25,  231, // 20 21 22 23
    21,  235, 17,  239, 14,  242, 12,  244, // 24 25 26 27
    21,  235, 17,  239, 14,  242, 12,  244, // 24 25 26 27
    10,  246, 9,   247, 8,   248, 8,   248, // 28 29 30 31
    10,  246, 9,   247, 8,   248, 8,   248 // 28 29 30 31
};

// bs = 64
EB_ALIGN(32)
static const uint16_t sm_weights_d_64[256] = {
    255, 1,   248, 8,   240, 16,  233, 23, //  0  1  2  3
    255, 1,   248, 8,   240, 16,  233, 23, //  0  1  2  3
    225, 31,  218, 38,  210, 46,  203, 53, //  4  5  6  7
    225, 31,  218, 38,  210, 46,  203, 53, //  4  5  6  7
    196, 60,  189, 67,  182, 74,  176, 80, //  8  9 10 11
    196, 60,  189, 67,  182, 74,  176, 80, //  8  9 10 11
    169, 87,  163, 93,  156, 100, 150, 106, // 12 13 14 15
    169, 87,  163, 93,  156, 100, 150, 106, // 12 13 14 15
    144, 112, 138, 118, 133, 123, 127, 129, // 16 17 18 19
    144, 112, 138, 118, 133, 123, 127, 129, // 16 17 18 19
    121, 135, 116, 140, 111, 145, 106, 150, // 20 21 22 23
    121, 135, 116, 140, 111, 145, 106, 150, // 20 21 22 23
    101, 155, 96,  160, 91,  165, 86,  170, // 24 25 26 27
    101, 155, 96,  160, 91,  165, 86,  170, // 24 25 26 27
    82,  174, 77,  179, 73,  183, 69,  187, // 28 29 30 31
    82,  174, 77,  179, 73,  183, 69,  187, // 28 29 30 31
    65,  191, 61,  195, 57,  199, 54,  202, // 32 33 34 35
    65,  191, 61,  195, 57,  199, 54,  202, // 32 33 34 35
    50,  206, 47,  209, 44,  212, 41,  215, // 36 37 38 39
    50,  206, 47,  209, 44,  212, 41,  215, // 36 37 38 39
    38,  218, 35,  221, 32,  224, 29,  227, // 40 41 42 43
    38,  218, 35,  221, 32,  224, 29,  227, // 40 41 42 43
    27,  229, 25,  231, 22,  234, 20,  236, // 44 45 46 47
    27,  229, 25,  231, 22,  234, 20,  236, // 44 45 46 47
    18,  238, 16,  240, 15,  241, 13,  243, // 48 49 50 51
    18,  238, 16,  240, 15,  241, 13,  243, // 48 49 50 51
    12,  244, 10,  246, 9,   247, 8,   248, // 52 53 54 55
    12,  244, 10,  246, 9,   247, 8,   248, // 52 53 54 55
    7,   249, 6,   250, 6,   250, 5,   251, // 56 57 58 59
    7,   249, 6,   250, 6,   250, 5,   251, // 56 57 58 59
    5,   251, 4,   252, 4,   252, 4,   252, // 60 61 62 63
    5,   251, 4,   252, 4,   252, 4,   252 // 60 61 62 63
};

// -----------------------------------------------------------------------------

// Shuffle for AVX2 optimizations.

// bs = 16
EB_ALIGN(32)
static const uint16_t sm_weights_16[32] = {
    255, 1,   225, 31,  196, 60,  170, 86, //  0  1  2  3
    68,  188, 54,  202, 43,  213, 33,  223, //  8  9 10 11
    145, 111, 123, 133, 102, 154, 84,  172, //  4  5  6  7
    26,  230, 20,  236, 17,  239, 16,  240 // 12 13 14 15
};

// bs = 32
EB_ALIGN(32)
static const uint16_t sm_weights_32[64] = {
    255, 1,   240, 16,  225, 31,  210, 46, //  0  1  2  3
    145, 111, 133, 123, 122, 134, 111, 145, //  8  9 10 11
    196, 60,  182, 74,  169, 87,  157, 99, //  4  5  6  7
    101, 155, 92,  164, 83,  173, 74,  182, // 12 13 14 15
    66,  190, 59,  197, 52,  204, 45,  211, // 16 17 18 19
    21,  235, 17,  239, 14,  242, 12,  244, // 24 25 26 27
    39,  217, 34,  222, 29,  227, 25,  231, // 20 21 22 23
    10,  246, 9,   247, 8,   248, 8,   248 // 28 29 30 31
};

// bs = 64
EB_ALIGN(32)
static const uint16_t sm_weights_64[128] = {
    255, 1,   248, 8,   240, 16,  233, 23, //  0  1  2  3
    196, 60,  189, 67,  182, 74,  176, 80, //  8  9 10 11
    225, 31,  218, 38,  210, 46,  203, 53, //  4  5  6  7
    169, 87,  163, 93,  156, 100, 150, 106, // 12 13 14 15
    144, 112, 138, 118, 133, 123, 127, 129, // 16 17 18 19
    101, 155, 96,  160, 91,  165, 86,  170, // 24 25 26 27
    121, 135, 116, 140, 111, 145, 106, 150, // 20 21 22 23
    82,  174, 77,  179, 73,  183, 69,  187, // 28 29 30 31
    65,  191, 61,  195, 57,  199, 54,  202, // 32 33 34 35
    38,  218, 35,  221, 32,  224, 29,  227, // 40 41 42 43
    50,  206, 47,  209, 44,  212, 41,  215, // 36 37 38 39
    27,  229, 25,  231, 22,  234, 20,  236, // 44 45 46 47
    18,  238, 16,  240, 15,  241, 13,  243, // 48 49 50 51
    7,   249, 6,   250, 6,   250, 5,   251, // 56 57 58 59
    12,  244, 10,  246, 9,   247, 8,   248, // 52 53 54 55
    5,   251, 4,   252, 4,   252, 4,   252 // 60 61 62 63
};

// SMOOTH_PRED

// 8xN

static INLINE void load_right_weights_8(const uint16_t* const above, __m256i* const r, __m256i* const weights) {
    *r = _mm256_set1_epi16((uint16_t)above[7]);

    // 0 1 2 3  0 1 2 3
    weights[0] = _mm256_loadu_si256((const __m256i*)(sm_weights_d_8 + 0x00));
    // 4 5 6 7  4 5 6 7
    weights[1] = _mm256_loadu_si256((const __m256i*)(sm_weights_d_8 + 0x10));
}

static INLINE __m256i load_left_4(const uint16_t* const left, const __m256i r) {
    const __m128i l0 = _mm_loadl_epi64((const __m128i*)left);
    // 0 1 2 3 x x x x  0 1 2 3 x x x x
    const __m256i l = _mm256_inserti128_si256(_mm256_castsi128_si256(l0), l0, 1);
    return _mm256_unpacklo_epi16(l, r); // 0 1 2 3  0 1 2 3
}

static INLINE void load_left_8(const uint16_t* const left, const __m256i r, __m256i* const lr) {
    const __m128i l0 = _mm_loadu_si128((const __m128i*)left);
    // 0 1 2 3 4 5 6 7  0 1 2 3 4 5 6 7
    const __m256i l = _mm256_inserti128_si256(_mm256_castsi128_si256(l0), l0, 1);
    lr[0]           = _mm256_unpacklo_epi16(l, r); // 0 1 2 3  0 1 2 3
    lr[1]           = _mm256_unpackhi_epi16(l, r); // 4 5 6 7  4 5 6 7
}

static INLINE void init_8(const uint16_t* const above, const uint16_t* const left, const int32_t h, __m256i* const ab,
                          __m256i* const r, __m256i* const weights_w, __m256i* const rep) {
    const __m128i a0 = _mm_loadl_epi64(((const __m128i*)(above + 0)));
    const __m128i a1 = _mm_loadl_epi64(((const __m128i*)(above + 4)));
    const __m256i b  = _mm256_set1_epi16((uint16_t)left[h - 1]);
    __m256i       a[2];
    a[0]  = _mm256_inserti128_si256(_mm256_castsi128_si256(a0), a0, 1);
    a[1]  = _mm256_inserti128_si256(_mm256_castsi128_si256(a1), a1, 1);
    ab[0] = _mm256_unpacklo_epi16(a[0], b);
    ab[1] = _mm256_unpacklo_epi16(a[1], b);
    load_right_weights_8(above, r, weights_w);

    const __m128i rep0 = _mm_set1_epi32(0x03020100);
    const __m128i rep1 = _mm_set1_epi32(0x07060504);
    const __m128i rep2 = _mm_set1_epi32(0x0B0A0908);
    const __m128i rep3 = _mm_set1_epi32(0x0F0E0D0C);
    rep[0]             = _mm256_inserti128_si256(_mm256_castsi128_si256(rep0), rep1, 1);
    rep[1]             = _mm256_inserti128_si256(_mm256_castsi128_si256(rep2), rep3, 1);
}

static INLINE __m256i smooth_pred_kernel(const __m256i* const weights_w, const __m256i weights_h, const __m256i rep,
                                         const __m256i* const ab, const __m256i lr) {
    const __m256i round = _mm256_set1_epi32((1 << sm_weight_log2_scale));
    __m256i       s[2], sum[2];
    // 0 0 0 0  1 1 1 1
    const __m256i w = _mm256_shuffle_epi8(weights_h, rep);
    const __m256i t = _mm256_shuffle_epi8(lr, rep);
    s[0]            = _mm256_madd_epi16(ab[0], w);
    s[1]            = _mm256_madd_epi16(ab[1], w);
    // width 8: 00 01 02 03  10 11 12 13
    // width 16: 0 1 2 3  8 9 A b
    sum[0] = _mm256_madd_epi16(t, weights_w[0]);
    // width 8: 04 05 06 07  14 15 16 17
    // width 16: 4 5 6 7  C D E F
    sum[1] = _mm256_madd_epi16(t, weights_w[1]);
    sum[0] = _mm256_add_epi32(sum[0], s[0]);
    sum[1] = _mm256_add_epi32(sum[1], s[1]);
    sum[0] = _mm256_add_epi32(sum[0], round);
    sum[1] = _mm256_add_epi32(sum[1], round);
    sum[0] = _mm256_srai_epi32(sum[0], 1 + sm_weight_log2_scale);
    sum[1] = _mm256_srai_epi32(sum[1], 1 + sm_weight_log2_scale);
    // width 8: 00 01 02 03 04 05 06 07  10 11 12 13 14 15 16 17
    // width 16: 0 1 2 3 4 5 6 7  8 9 A b C D E F
    return _mm256_packs_epi32(sum[0], sum[1]);
}

static INLINE void smooth_pred_8x2(const __m256i* const weights_w, const __m256i weights_h, const __m256i rep,
                                   const __m256i* const ab, const __m256i lr, uint16_t** const dst,
                                   const ptrdiff_t stride) {
    // 00 01 02 03 04 05 06 07  10 11 12 13 14 15 16 17
    const __m256i d = smooth_pred_kernel(weights_w, weights_h, rep, ab, lr);
    _mm_storeu_si128((__m128i*)*dst, _mm256_castsi256_si128(d));
    *dst += stride;
    _mm_storeu_si128((__m128i*)*dst, _mm256_extracti128_si256(d, 1));
    *dst += stride;
}

static INLINE void smooth_pred_8x4(const __m256i* const weights_w, const uint16_t* const sm_weights_h,
                                   const __m256i* const rep, const __m256i* const ab, const __m256i lr,
                                   uint16_t** const dst, const ptrdiff_t stride) {
    const __m256i weights_h = _mm256_loadu_si256((const __m256i*)sm_weights_h);
    smooth_pred_8x2(weights_w, weights_h, rep[0], ab, lr, dst, stride);
    smooth_pred_8x2(weights_w, weights_h, rep[1], ab, lr, dst, stride);
}

static INLINE void smooth_pred_8x8(const uint16_t* const left, const __m256i* const weights_w,
                                   const uint16_t* const sm_weights_h, const __m256i* const rep,
                                   const __m256i* const ab, const __m256i r, uint16_t** const dst,
                                   const ptrdiff_t stride) {
    __m256i lr[2];
    load_left_8(left, r, lr);

    smooth_pred_8x4(weights_w, sm_weights_h + 0, rep, ab, lr[0], dst, stride);
    smooth_pred_8x4(weights_w, sm_weights_h + 16, rep, ab, lr[1], dst, stride);
}

// 8x4

void svt_aom_highbd_smooth_predictor_8x4_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                              const uint16_t* left, int32_t bd) {
    __m256i ab[2], r, lr, weights_w[2], rep[2];
    (void)bd;

    init_8(above, left, 4, ab, &r, weights_w, rep);
    lr = load_left_4(left, r);
    smooth_pred_8x4(weights_w, sm_weights_d_4, rep, ab, lr, &dst, stride);
}

// 8x8

void svt_aom_highbd_smooth_predictor_8x8_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                              const uint16_t* left, int32_t bd) {
    __m256i ab[2], r, weights_w[2], rep[2];
    (void)bd;

    init_8(above, left, 8, ab, &r, weights_w, rep);

    smooth_pred_8x8(left, weights_w, sm_weights_d_8, rep, ab, r, &dst, stride);
}

// 8x16

void svt_aom_highbd_smooth_predictor_8x16_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                               const uint16_t* left, int32_t bd) {
    __m256i ab[2], r, weights_w[2], rep[2];
    (void)bd;

    init_8(above, left, 16, ab, &r, weights_w, rep);

    for (int32_t i = 0; i < 2; i++) {
        smooth_pred_8x8(left + 8 * i, weights_w, sm_weights_d_16 + 32 * i, rep, ab, r, &dst, stride);
    }
}

// 8x32

void svt_aom_highbd_smooth_predictor_8x32_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                               const uint16_t* left, int32_t bd) {
    __m256i ab[2], r, weights_w[2], rep[2];
    (void)bd;

    init_8(above, left, 32, ab, &r, weights_w, rep);

    for (int32_t i = 0; i < 4; i++) {
        smooth_pred_8x8(left + 8 * i, weights_w, sm_weights_d_32 + 32 * i, rep, ab, r, &dst, stride);
    }
}

// -----------------------------------------------------------------------------
// 16xN

static INLINE void load_right_weights_16(const uint16_t* const above, __m256i* const r, __m256i* const weights) {
    *r = _mm256_set1_epi16((uint16_t)above[15]);

    //  0  1  2  3   8  9 10 11
    weights[0] = _mm256_loadu_si256((const __m256i*)(sm_weights_16 + 0x00));
    //  4  5  6  7  12 13 14 15
    weights[1] = _mm256_loadu_si256((const __m256i*)(sm_weights_16 + 0x10));
}

static INLINE void prepare_ab(const uint16_t* const above, const __m256i b, __m256i* const ab) {
    const __m256i a = _mm256_loadu_si256((const __m256i*)above);
    ab[0]           = _mm256_unpacklo_epi16(a, b);
    ab[1]           = _mm256_unpackhi_epi16(a, b);
}

static INLINE void init_16(const uint16_t* const above, const uint16_t* const left, const int32_t h, __m256i* const ab,
                           __m256i* const r, __m256i* const weights_w, __m256i* const rep) {
    const __m256i b = _mm256_set1_epi16((uint16_t)left[h - 1]);
    prepare_ab(above, b, ab);
    load_right_weights_16(above, r, weights_w);

    rep[0] = _mm256_set1_epi32(0x03020100);
    rep[1] = _mm256_set1_epi32(0x07060504);
    rep[2] = _mm256_set1_epi32(0x0B0A0908);
    rep[3] = _mm256_set1_epi32(0x0F0E0D0C);
}

static INLINE void smooth_pred_16(const __m256i* const weights_w, const __m256i weights_h, const __m256i rep,
                                  const __m256i* const ab, const __m256i lr, uint16_t** const dst,
                                  const ptrdiff_t stride) {
    const __m256i d = smooth_pred_kernel(weights_w, weights_h, rep, ab, lr);
    _mm256_storeu_si256((__m256i*)*dst, d);
    *dst += stride;
}

static INLINE void smooth_pred_16x4(const __m256i* const weights_w, const uint16_t* const sm_weights_h,
                                    const __m256i* const rep, const __m256i* const ab, const __m256i lr,
                                    uint16_t** const dst, const ptrdiff_t stride) {
    const __m256i weights_h = _mm256_loadu_si256((const __m256i*)sm_weights_h);
    smooth_pred_16(weights_w, weights_h, rep[0], ab, lr, dst, stride);
    smooth_pred_16(weights_w, weights_h, rep[1], ab, lr, dst, stride);
    smooth_pred_16(weights_w, weights_h, rep[2], ab, lr, dst, stride);
    smooth_pred_16(weights_w, weights_h, rep[3], ab, lr, dst, stride);
}

static INLINE void smooth_pred_16x8(const uint16_t* const left, const __m256i* const weights_w,
                                    const uint16_t* const sm_weights_h, const __m256i* const rep,
                                    const __m256i* const ab, const __m256i r, uint16_t** const dst,
                                    const ptrdiff_t stride) {
    __m256i lr[2];
    load_left_8(left, r, lr);

    smooth_pred_16x4(weights_w, sm_weights_h + 0, rep, ab, lr[0], dst, stride);
    smooth_pred_16x4(weights_w, sm_weights_h + 16, rep, ab, lr[1], dst, stride);
}

// 16x4

void svt_aom_highbd_smooth_predictor_16x4_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                               const uint16_t* left, int32_t bd) {
    __m256i ab[2], r, lr, weights_w[2], rep[4];
    (void)bd;

    init_16(above, left, 4, ab, &r, weights_w, rep);
    lr = load_left_4(left, r);
    smooth_pred_16x4(weights_w, sm_weights_d_4, rep, ab, lr, &dst, stride);
}

// 16x8

void svt_aom_highbd_smooth_predictor_16x8_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                               const uint16_t* left, int32_t bd) {
    __m256i ab[2], r, weights_w[2], rep[4];
    (void)bd;

    init_16(above, left, 8, ab, &r, weights_w, rep);

    smooth_pred_16x8(left, weights_w, sm_weights_d_8, rep, ab, r, &dst, stride);
}

// 16x16

void svt_aom_highbd_smooth_predictor_16x16_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                const uint16_t* left, int32_t bd) {
    __m256i ab[2], r, weights_w[2], rep[4];
    (void)bd;

    init_16(above, left, 16, ab, &r, weights_w, rep);

    for (int32_t i = 0; i < 2; i++) {
        smooth_pred_16x8(left + 8 * i, weights_w, sm_weights_d_16 + 32 * i, rep, ab, r, &dst, stride);
    }
}

// 16x32

void svt_aom_highbd_smooth_predictor_16x32_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                const uint16_t* left, int32_t bd) {
    __m256i ab[2], r, weights_w[2], rep[4];
    (void)bd;

    init_16(above, left, 32, ab, &r, weights_w, rep);

    for (int32_t i = 0; i < 4; i++) {
        smooth_pred_16x8(left + 8 * i, weights_w, sm_weights_d_32 + 32 * i, rep, ab, r, &dst, stride);
    }
}

// 16x64

void svt_aom_highbd_smooth_predictor_16x64_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                const uint16_t* left, int32_t bd) {
    __m256i ab[2], r, weights_w[2], rep[4];
    (void)bd;

    init_16(above, left, 64, ab, &r, weights_w, rep);

    for (int32_t i = 0; i < 8; i++) {
        smooth_pred_16x8(left + 8 * i, weights_w, sm_weights_d_64 + 32 * i, rep, ab, r, &dst, stride);
    }
}

// -----------------------------------------------------------------------------
// 32xN

static INLINE void load_right_weights_32(const uint16_t* const above, __m256i* const r, __m256i* const weights) {
    *r = _mm256_set1_epi16((uint16_t)above[31]);

    //  0  1  2  3   8  9 10 11
    weights[0] = _mm256_loadu_si256((const __m256i*)(sm_weights_32 + 0x00));
    //  4  5  6  7  12 13 14 15
    weights[1] = _mm256_loadu_si256((const __m256i*)(sm_weights_32 + 0x10));
    // 16 17 18 19  24 25 26 27
    weights[2] = _mm256_loadu_si256((const __m256i*)(sm_weights_32 + 0x20));
    // 20 21 22 23  28 29 30 31
    weights[3] = _mm256_loadu_si256((const __m256i*)(sm_weights_32 + 0x30));
}

static INLINE void init_32(const uint16_t* const above, const uint16_t* const left, const int32_t h, __m256i* const ab,
                           __m256i* const r, __m256i* const weights_w, __m256i* const rep) {
    const __m256i b = _mm256_set1_epi16((uint16_t)left[h - 1]);
    prepare_ab(above + 0x00, b, ab + 0);
    prepare_ab(above + 0x10, b, ab + 2);
    load_right_weights_32(above, r, weights_w);

    rep[0] = _mm256_set1_epi32(0x03020100);
    rep[1] = _mm256_set1_epi32(0x07060504);
    rep[2] = _mm256_set1_epi32(0x0B0A0908);
    rep[3] = _mm256_set1_epi32(0x0F0E0D0C);
}

static INLINE void smooth_pred_32(const __m256i* const weights_w, const __m256i weights_h, const __m256i rep,
                                  const __m256i* const ab, const __m256i lr, uint16_t** const dst,
                                  const ptrdiff_t stride) {
    __m256i d;

    //  0  1  2  3  4  5  6  7   8  9 10 11 12 13 14 15
    d = smooth_pred_kernel(weights_w + 0, weights_h, rep, ab + 0, lr);
    _mm256_storeu_si256((__m256i*)(*dst + 0x00), d);

    // 16 17 18 19 20 21 22 23  24 25 26 27 28 29 30 31
    d = smooth_pred_kernel(weights_w + 2, weights_h, rep, ab + 2, lr);
    _mm256_storeu_si256((__m256i*)(*dst + 0x10), d);
    *dst += stride;
}

static INLINE void smooth_pred_32x4(const __m256i* const weights_w, const uint16_t* const sm_weights_h,
                                    const __m256i* const rep, const __m256i* const ab, const __m256i lr,
                                    uint16_t** const dst, const ptrdiff_t stride) {
    const __m256i weights_h = _mm256_loadu_si256((const __m256i*)sm_weights_h);
    smooth_pred_32(weights_w, weights_h, rep[0], ab, lr, dst, stride);
    smooth_pred_32(weights_w, weights_h, rep[1], ab, lr, dst, stride);
    smooth_pred_32(weights_w, weights_h, rep[2], ab, lr, dst, stride);
    smooth_pred_32(weights_w, weights_h, rep[3], ab, lr, dst, stride);
}

static INLINE void smooth_pred_32x8(const uint16_t* const left, const __m256i* const weights_w,
                                    const uint16_t* const sm_weights_h, const __m256i* const rep,
                                    const __m256i* const ab, const __m256i r, uint16_t** const dst,
                                    const ptrdiff_t stride) {
    __m256i lr[2];
    load_left_8(left, r, lr);

    smooth_pred_32x4(weights_w, sm_weights_h + 0, rep, ab, lr[0], dst, stride);
    smooth_pred_32x4(weights_w, sm_weights_h + 16, rep, ab, lr[1], dst, stride);
}

// 32x8

void svt_aom_highbd_smooth_predictor_32x8_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                               const uint16_t* left, int32_t bd) {
    __m256i ab[4], r, weights_w[4], rep[4];
    (void)bd;

    init_32(above, left, 8, ab, &r, weights_w, rep);

    smooth_pred_32x8(left, weights_w, sm_weights_d_8, rep, ab, r, &dst, stride);
}

// 32x16

void svt_aom_highbd_smooth_predictor_32x16_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                const uint16_t* left, int32_t bd) {
    __m256i ab[4], r, weights_w[4], rep[4];
    (void)bd;

    init_32(above, left, 16, ab, &r, weights_w, rep);

    for (int32_t i = 0; i < 2; i++) {
        smooth_pred_32x8(left + 8 * i, weights_w, sm_weights_d_16 + 32 * i, rep, ab, r, &dst, stride);
    }
}

// 32x32

void svt_aom_highbd_smooth_predictor_32x32_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                const uint16_t* left, int32_t bd) {
    __m256i ab[4], r, weights_w[4], rep[4];
    (void)bd;

    init_32(above, left, 32, ab, &r, weights_w, rep);

    for (int32_t i = 0; i < 4; i++) {
        smooth_pred_32x8(left + 8 * i, weights_w, sm_weights_d_32 + 32 * i, rep, ab, r, &dst, stride);
    }
}

// 32x64

void svt_aom_highbd_smooth_predictor_32x64_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                const uint16_t* left, int32_t bd) {
    __m256i ab[4], r, weights_w[4], rep[4];
    (void)bd;

    init_32(above, left, 64, ab, &r, weights_w, rep);

    for (int32_t i = 0; i < 8; i++) {
        smooth_pred_32x8(left + 8 * i, weights_w, sm_weights_d_64 + 32 * i, rep, ab, r, &dst, stride);
    }
}

// -----------------------------------------------------------------------------
// 64xN

static INLINE void load_right_weights_64(const uint16_t* const above, __m256i* const r, __m256i* const weights) {
    *r = _mm256_set1_epi16((uint16_t)above[63]);

    //  0  1  2  3   8  9 10 11
    weights[0] = _mm256_loadu_si256((const __m256i*)(sm_weights_64 + 0x00));
    //  4  5  6  7  12 13 14 15
    weights[1] = _mm256_loadu_si256((const __m256i*)(sm_weights_64 + 0x10));
    // 16 17 18 19  24 25 26 27
    weights[2] = _mm256_loadu_si256((const __m256i*)(sm_weights_64 + 0x20));
    // 20 21 22 23  28 29 30 31
    weights[3] = _mm256_loadu_si256((const __m256i*)(sm_weights_64 + 0x30));
    // 32 33 34 35  40 41 42 43
    weights[4] = _mm256_loadu_si256((const __m256i*)(sm_weights_64 + 0x40));
    // 36 37 38 39  44 45 46 47
    weights[5] = _mm256_loadu_si256((const __m256i*)(sm_weights_64 + 0x50));
    // 48 49 50 51  56 57 58 59
    weights[6] = _mm256_loadu_si256((const __m256i*)(sm_weights_64 + 0x60));
    // 52 53 54 55  60 61 62 63
    weights[7] = _mm256_loadu_si256((const __m256i*)(sm_weights_64 + 0x70));
}

static INLINE void init_64(const uint16_t* const above, const uint16_t* const left, const int32_t h, __m256i* const ab,
                           __m256i* const r, __m256i* const weights_w, __m256i* const rep) {
    const __m256i b = _mm256_set1_epi16((uint16_t)left[h - 1]);
    prepare_ab(above + 0x00, b, ab + 0);
    prepare_ab(above + 0x10, b, ab + 2);
    prepare_ab(above + 0x20, b, ab + 4);
    prepare_ab(above + 0x30, b, ab + 6);
    load_right_weights_64(above, r, weights_w);

    rep[0] = _mm256_set1_epi32(0x03020100);
    rep[1] = _mm256_set1_epi32(0x07060504);
    rep[2] = _mm256_set1_epi32(0x0B0A0908);
    rep[3] = _mm256_set1_epi32(0x0F0E0D0C);
}

static INLINE void smooth_pred_64(const __m256i* const weights_w, const __m256i weights_h, const __m256i rep,
                                  const __m256i* const ab, const __m256i lr, uint16_t** const dst,
                                  const ptrdiff_t stride) {
    __m256i d;

    //  0  1  2  3  4  5  6  7   8  9 10 11 12 13 14 15
    d = smooth_pred_kernel(weights_w + 0, weights_h, rep, ab + 0, lr);
    _mm256_storeu_si256((__m256i*)(*dst + 0x00), d);

    // 16 17 18 19 20 21 22 23  24 25 26 27 28 29 30 31
    d = smooth_pred_kernel(weights_w + 2, weights_h, rep, ab + 2, lr);
    _mm256_storeu_si256((__m256i*)(*dst + 0x10), d);

    // 32 33 34 35 36 37 38 39  40 41 42 43 44 45 46 47
    d = smooth_pred_kernel(weights_w + 4, weights_h, rep, ab + 4, lr);
    _mm256_storeu_si256((__m256i*)(*dst + 0x20), d);

    // 48 49 50 51 52 53 54 55  56 57 58 59 60 61 62 63
    d = smooth_pred_kernel(weights_w + 6, weights_h, rep, ab + 6, lr);
    _mm256_storeu_si256((__m256i*)(*dst + 0x30), d);
    *dst += stride;
}

static INLINE void smooth_pred_64x4(const __m256i* const weights_w, const uint16_t* const sm_weights_h,
                                    const __m256i* const rep, const __m256i* const ab, const __m256i lr,
                                    uint16_t** const dst, const ptrdiff_t stride) {
    const __m256i weights_h = _mm256_loadu_si256((const __m256i*)sm_weights_h);
    smooth_pred_64(weights_w, weights_h, rep[0], ab, lr, dst, stride);
    smooth_pred_64(weights_w, weights_h, rep[1], ab, lr, dst, stride);
    smooth_pred_64(weights_w, weights_h, rep[2], ab, lr, dst, stride);
    smooth_pred_64(weights_w, weights_h, rep[3], ab, lr, dst, stride);
}

static INLINE void smooth_pred_64x8(const uint16_t* const left, const __m256i* const weights_w,
                                    const uint16_t* const sm_weights_h, const __m256i* const rep,
                                    const __m256i* const ab, const __m256i r, uint16_t** const dst,
                                    const ptrdiff_t stride) {
    __m256i lr[2];
    load_left_8(left, r, lr);

    smooth_pred_64x4(weights_w, sm_weights_h + 0, rep, ab, lr[0], dst, stride);
    smooth_pred_64x4(weights_w, sm_weights_h + 16, rep, ab, lr[1], dst, stride);
}

// 64x16

void svt_aom_highbd_smooth_predictor_64x16_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                const uint16_t* left, int32_t bd) {
    __m256i ab[8], r, weights_w[8], rep[4];
    (void)bd;

    init_64(above, left, 16, ab, &r, weights_w, rep);

    for (int32_t i = 0; i < 2; i++) {
        smooth_pred_64x8(left + 8 * i, weights_w, sm_weights_d_16 + 32 * i, rep, ab, r, &dst, stride);
    }
}

// 64x32

void svt_aom_highbd_smooth_predictor_64x32_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                const uint16_t* left, int32_t bd) {
    __m256i ab[8], r, weights_w[8], rep[4];
    (void)bd;

    init_64(above, left, 32, ab, &r, weights_w, rep);

    for (int32_t i = 0; i < 4; i++) {
        smooth_pred_64x8(left + 8 * i, weights_w, sm_weights_d_32 + 32 * i, rep, ab, r, &dst, stride);
    }
}

// 64x64

void svt_aom_highbd_smooth_predictor_64x64_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                const uint16_t* left, int32_t bd) {
    __m256i ab[8], r, weights_w[8], rep[4];
    (void)bd;

    init_64(above, left, 64, ab, &r, weights_w, rep);

    for (int32_t i = 0; i < 8; i++) {
        smooth_pred_64x8(left + 8 * i, weights_w, sm_weights_d_64 + 32 * i, rep, ab, r, &dst, stride);
    }
}

// =============================================================================

// SMOOTH_H_PRED

// 8xN

static INLINE __m256i smooth_h_pred_kernel(const __m256i* const weights, const __m256i lr) {
    const __m256i round = _mm256_set1_epi32((1 << (sm_weight_log2_scale - 1)));
    __m256i       sum[2];
    // width 8: 00 01 02 03  10 11 12 13
    // width 16: 0 1 2 3  8 9 A b
    sum[0] = _mm256_madd_epi16(lr, weights[0]);
    // width 8: 04 05 06 07  14 15 16 17
    // width 16: 4 5 6 7  C D E F
    sum[1] = _mm256_madd_epi16(lr, weights[1]);
    sum[0] = _mm256_add_epi32(sum[0], round);
    sum[1] = _mm256_add_epi32(sum[1], round);
    sum[0] = _mm256_srai_epi32(sum[0], sm_weight_log2_scale);
    sum[1] = _mm256_srai_epi32(sum[1], sm_weight_log2_scale);
    // width 8: 00 01 02 03 04 05 06 07  10 11 12 13 14 15 16 17
    // width 16: 0 1 2 3 4 5 6 7  8 9 A b C D E F
    return _mm256_packs_epi32(sum[0], sum[1]);
}

static INLINE void smooth_h_pred_8x2(const __m256i* const weights, __m256i* const lr, uint16_t** const dst,
                                     const ptrdiff_t stride) {
    const __m256i rep = _mm256_set1_epi32(0x03020100);
    // lr: 0 1 2 3  1 2 3 4
    const __m256i t = _mm256_shuffle_epi8(*lr, rep); // 0 0 0 0  1 1 1 1
    // 00 01 02 03 04 05 06 07  10 11 12 13 14 15 16 17
    const __m256i d = smooth_h_pred_kernel(weights, t);
    _mm_storeu_si128((__m128i*)*dst, _mm256_castsi256_si128(d));
    *dst += stride;
    _mm_storeu_si128((__m128i*)*dst, _mm256_extracti128_si256(d, 1));
    *dst += stride;
    *lr = _mm256_srli_si256(*lr, 8); // 2 3 x x  3 4 x x
}

static INLINE void smooth_h_pred_8x4(const __m256i* const weights, __m256i* const lr, uint16_t** const dst,
                                     const ptrdiff_t stride) {
    smooth_h_pred_8x2(weights, lr, dst, stride);
    smooth_h_pred_8x2(weights, lr, dst, stride);
}

static INLINE void smooth_h_pred_8x8(const uint16_t* const left, const __m256i r, const __m256i* const weights,
                                     uint16_t** const dst, const ptrdiff_t stride) {
    const __m128i l0 = _mm_loadu_si128((const __m128i*)left);
    const __m128i l1 = _mm_srli_si128(l0, 2);
    // 0 1 2 3 4 5 6 7  1 2 3 4 5 6 7 x
    const __m256i l = _mm256_inserti128_si256(_mm256_castsi128_si256(l0), l1, 1);
    __m256i       lr[2];
    lr[0] = _mm256_unpacklo_epi16(l, r); // 0 1 2 3  1 2 3 4
    lr[1] = _mm256_unpackhi_epi16(l, r); // 4 5 6 7  5 6 7 x
    smooth_h_pred_8x4(weights, &lr[0], dst, stride);
    smooth_h_pred_8x4(weights, &lr[1], dst, stride);
}

// 8x4

void svt_aom_highbd_smooth_h_predictor_8x4_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                const uint16_t* left, int32_t bd) {
    const __m128i l0 = _mm_loadl_epi64((const __m128i*)left);
    const __m128i l1 = _mm_srli_si128(l0, 2);
    // 0 1 2 3 x x x x  1 2 3 4 x x x x
    const __m256i l = _mm256_inserti128_si256(_mm256_castsi128_si256(l0), l1, 1);
    __m256i       r, weights[2];
    (void)bd;

    load_right_weights_8(above, &r, weights);
    __m256i lr = _mm256_unpacklo_epi16(l, r); // 0 1 2 3  1 2 3 4
    smooth_h_pred_8x4(weights, &lr, &dst, stride);
}

// 8x8

void svt_aom_highbd_smooth_h_predictor_8x8_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                const uint16_t* left, int32_t bd) {
    __m256i r, weights[2];
    (void)bd;

    load_right_weights_8(above, &r, weights);
    smooth_h_pred_8x8(left, r, weights, &dst, stride);
}

// 8x16

void svt_aom_highbd_smooth_h_predictor_8x16_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                 const uint16_t* left, int32_t bd) {
    __m256i r, weights[2];
    (void)bd;

    load_right_weights_8(above, &r, weights);
    smooth_h_pred_8x8(left + 0, r, weights, &dst, stride);
    smooth_h_pred_8x8(left + 8, r, weights, &dst, stride);
}

// 8x32

void svt_aom_highbd_smooth_h_predictor_8x32_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                 const uint16_t* left, int32_t bd) {
    __m256i r, weights[2];
    (void)bd;

    load_right_weights_8(above, &r, weights);

    for (int32_t i = 0; i < 2; i++) {
        smooth_h_pred_8x8(left + 0, r, weights, &dst, stride);
        smooth_h_pred_8x8(left + 8, r, weights, &dst, stride);
        left += 16;
    }
}

// -----------------------------------------------------------------------------
// 16xN

static INLINE void smooth_h_pred_16(const __m256i* const weights, __m256i* const lr, uint16_t** const dst,
                                    const ptrdiff_t stride) {
    const __m256i rep = _mm256_set1_epi32(0x03020100);
    // lr: 0 1 2 3  0 1 2 3
    const __m256i t = _mm256_shuffle_epi8(*lr, rep); // 0 0 0 0  0 0 0 0
    //  0  1  2  3  4  5  6  7   8  9 10 11 12 13 14 15
    const __m256i d = smooth_h_pred_kernel(weights, t);
    _mm256_storeu_si256((__m256i*)*dst, d);
    *dst += stride;
    *lr = _mm256_srli_si256(*lr, 4); // 1 2 3 x  1 2 3 x
}

static INLINE void smooth_h_pred_16x4(const __m256i* const weights, __m256i* const lr, uint16_t** const dst,
                                      const ptrdiff_t stride) {
    smooth_h_pred_16(weights, lr, dst, stride);
    smooth_h_pred_16(weights, lr, dst, stride);
    smooth_h_pred_16(weights, lr, dst, stride);
    smooth_h_pred_16(weights, lr, dst, stride);
}

static INLINE void smooth_h_pred_16x8(const uint16_t* const left, const __m256i r, const __m256i* const weights,
                                      uint16_t** const dst, const ptrdiff_t stride) {
    __m256i lr[2];
    load_left_8(left, r, lr);
    smooth_h_pred_16x4(weights, &lr[0], dst, stride);
    smooth_h_pred_16x4(weights, &lr[1], dst, stride);
}

static INLINE void smooth_h_predictor_16x16(uint16_t* dst, const ptrdiff_t stride, const uint16_t* const above,
                                            const uint16_t* left, const int32_t n) {
    __m256i r, weights[2];

    load_right_weights_16(above, &r, weights);

    for (int32_t i = 0; i < n; i++) {
        smooth_h_pred_16x8(left + 0, r, weights, &dst, stride);
        smooth_h_pred_16x8(left + 8, r, weights, &dst, stride);
        left += 16;
    }
}

// 16x4

void svt_aom_highbd_smooth_h_predictor_16x4_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                 const uint16_t* left, int32_t bd) {
    __m256i r, lr, weights[2];
    (void)bd;

    load_right_weights_16(above, &r, weights);
    lr = load_left_4(left, r);
    smooth_h_pred_16x4(weights, &lr, &dst, stride);
}

// 16x8

void svt_aom_highbd_smooth_h_predictor_16x8_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                 const uint16_t* left, int32_t bd) {
    __m256i r, weights[2];
    (void)bd;

    load_right_weights_16(above, &r, weights);
    smooth_h_pred_16x8(left, r, weights, &dst, stride);
}

// 16x16

void svt_aom_highbd_smooth_h_predictor_16x16_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                  const uint16_t* left, int32_t bd) {
    (void)bd;
    smooth_h_predictor_16x16(dst, stride, above, left, 1);
}

// 16x32

void svt_aom_highbd_smooth_h_predictor_16x32_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                  const uint16_t* left, int32_t bd) {
    (void)bd;
    smooth_h_predictor_16x16(dst, stride, above, left, 2);
}

// 16x64

void svt_aom_highbd_smooth_h_predictor_16x64_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                  const uint16_t* left, int32_t bd) {
    (void)bd;
    smooth_h_predictor_16x16(dst, stride, above, left, 4);
}

// -----------------------------------------------------------------------------
// 32xN

static INLINE void smooth_h_pred_32(const __m256i* const weights, __m256i* const lr, uint16_t** const dst,
                                    const ptrdiff_t stride) {
    const __m256i rep = _mm256_set1_epi32(0x03020100);
    // lr: 0 1 2 3  0 1 2 3
    const __m256i t = _mm256_shuffle_epi8(*lr, rep); // 0 0 0 0  0 0 0 0
    __m256i       d;

    //  0  1  2  3  4  5  6  7   8  9 10 11 12 13 14 15
    d = smooth_h_pred_kernel(weights + 0, t);
    _mm256_storeu_si256((__m256i*)(*dst + 0x00), d);

    // 16 17 18 19 20 21 22 23  24 25 26 27 28 29 30 31
    d = smooth_h_pred_kernel(weights + 2, t);
    _mm256_storeu_si256((__m256i*)(*dst + 0x10), d);
    *dst += stride;
    *lr = _mm256_srli_si256(*lr, 4); // 1 2 3 x  1 2 3 x
}

static INLINE void smooth_h_pred_32x4(const __m256i* const weights, __m256i* const lr, uint16_t** const dst,
                                      const ptrdiff_t stride) {
    smooth_h_pred_32(weights, lr, dst, stride);
    smooth_h_pred_32(weights, lr, dst, stride);
    smooth_h_pred_32(weights, lr, dst, stride);
    smooth_h_pred_32(weights, lr, dst, stride);
}

static INLINE void smooth_h_pred_32x8(uint16_t* dst, const ptrdiff_t stride, const uint16_t* const above,
                                      const uint16_t* left, const int32_t n) {
    __m256i r, lr[2], weights[4];

    load_right_weights_32(above, &r, weights);

    for (int32_t i = 0; i < n; i++) {
        load_left_8(left, r, lr);
        smooth_h_pred_32x4(weights, &lr[0], &dst, stride);
        smooth_h_pred_32x4(weights, &lr[1], &dst, stride);
        left += 8;
    }
}

// 32x8

void svt_aom_highbd_smooth_h_predictor_32x8_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                 const uint16_t* left, int32_t bd) {
    (void)bd;
    smooth_h_pred_32x8(dst, stride, above, left, 1);
}

// 32x16

void svt_aom_highbd_smooth_h_predictor_32x16_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                  const uint16_t* left, int32_t bd) {
    (void)bd;
    smooth_h_pred_32x8(dst, stride, above, left, 2);
}

// 32x32

void svt_aom_highbd_smooth_h_predictor_32x32_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                  const uint16_t* left, int32_t bd) {
    (void)bd;
    smooth_h_pred_32x8(dst, stride, above, left, 4);
}

// 32x64

void svt_aom_highbd_smooth_h_predictor_32x64_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                  const uint16_t* left, int32_t bd) {
    (void)bd;
    smooth_h_pred_32x8(dst, stride, above, left, 8);
}

// -----------------------------------------------------------------------------
// 64xN

static INLINE void smooth_h_pred_64(const __m256i* const weights, __m256i* const lr, uint16_t** const dst,
                                    const ptrdiff_t stride) {
    const __m256i rep = _mm256_set1_epi32(0x03020100);
    // lr: 0 1 2 3  0 1 2 3
    const __m256i t = _mm256_shuffle_epi8(*lr, rep); // 0 0 0 0  0 0 0 0
    __m256i       d;

    //  0  1  2  3  4  5  6  7   8  9 10 11 12 13 14 15
    d = smooth_h_pred_kernel(weights + 0, t);
    _mm256_storeu_si256((__m256i*)(*dst + 0x00), d);

    // 16 17 18 19 20 21 22 23  24 25 26 27 28 29 30 31
    d = smooth_h_pred_kernel(weights + 2, t);
    _mm256_storeu_si256((__m256i*)(*dst + 0x10), d);

    // 32 33 34 35 36 37 38 39  40 41 42 43 44 45 46 47
    d = smooth_h_pred_kernel(weights + 4, t);
    _mm256_storeu_si256((__m256i*)(*dst + 0x20), d);

    // 48 49 50 51 52 53 54 55  56 57 58 59 60 61 62 63
    d = smooth_h_pred_kernel(weights + 6, t);
    _mm256_storeu_si256((__m256i*)(*dst + 0x30), d);
    *dst += stride;
    *lr = _mm256_srli_si256(*lr, 4); // 1 2 3 x  1 2 3 x
}

static INLINE void smooth_h_pred_64x4(const __m256i* const weights, __m256i* const lr, uint16_t** const dst,
                                      const ptrdiff_t stride) {
    smooth_h_pred_64(weights, lr, dst, stride);
    smooth_h_pred_64(weights, lr, dst, stride);
    smooth_h_pred_64(weights, lr, dst, stride);
    smooth_h_pred_64(weights, lr, dst, stride);
}

static INLINE void smooth_h_pred_64x8(uint16_t* dst, const ptrdiff_t stride, const uint16_t* const above,
                                      const uint16_t* left, const int32_t n) {
    __m256i r, lr[2], weights[8];

    load_right_weights_64(above, &r, weights);

    for (int32_t i = 0; i < n; i++) {
        load_left_8(left, r, lr);
        smooth_h_pred_64x4(weights, &lr[0], &dst, stride);
        smooth_h_pred_64x4(weights, &lr[1], &dst, stride);
        left += 8;
    }
}

// 64x16

void svt_aom_highbd_smooth_h_predictor_64x16_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                  const uint16_t* left, int32_t bd) {
    (void)bd;
    smooth_h_pred_64x8(dst, stride, above, left, 2);
}

// 64x32

void svt_aom_highbd_smooth_h_predictor_64x32_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                  const uint16_t* left, int32_t bd) {
    (void)bd;
    smooth_h_pred_64x8(dst, stride, above, left, 4);
}

// 64x64

void svt_aom_highbd_smooth_h_predictor_64x64_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                  const uint16_t* left, int32_t bd) {
    (void)bd;
    smooth_h_pred_64x8(dst, stride, above, left, 8);
}

// =============================================================================

// SMOOTH_V_PRED

// 8xN

static INLINE void smooth_v_init_8(const uint16_t* const above, const uint16_t* const left, const int32_t h,
                                   __m256i* const ab, __m256i* const rep) {
    const __m128i a0 = _mm_loadl_epi64(((const __m128i*)(above + 0)));
    const __m128i a1 = _mm_loadl_epi64(((const __m128i*)(above + 4)));
    const __m256i b  = _mm256_set1_epi16((uint16_t)left[h - 1]);
    __m256i       a[2];
    a[0]  = _mm256_inserti128_si256(_mm256_castsi128_si256(a0), a0, 1);
    a[1]  = _mm256_inserti128_si256(_mm256_castsi128_si256(a1), a1, 1);
    ab[0] = _mm256_unpacklo_epi16(a[0], b);
    ab[1] = _mm256_unpacklo_epi16(a[1], b);

    const __m128i rep0 = _mm_set1_epi32(0x03020100);
    const __m128i rep1 = _mm_set1_epi32(0x07060504);
    const __m128i rep2 = _mm_set1_epi32(0x0B0A0908);
    const __m128i rep3 = _mm_set1_epi32(0x0F0E0D0C);
    rep[0]             = _mm256_inserti128_si256(_mm256_castsi128_si256(rep0), rep1, 1);
    rep[1]             = _mm256_inserti128_si256(_mm256_castsi128_si256(rep2), rep3, 1);
}

static INLINE __m256i smooth_v_pred_kernel(const __m256i weights, const __m256i rep, const __m256i* const ab) {
    const __m256i round = _mm256_set1_epi32((1 << (sm_weight_log2_scale - 1)));
    __m256i       sum[2];
    // 0 0 0 0  1 1 1 1
    const __m256i w = _mm256_shuffle_epi8(weights, rep);
    sum[0]          = _mm256_madd_epi16(ab[0], w);
    sum[1]          = _mm256_madd_epi16(ab[1], w);
    sum[0]          = _mm256_add_epi32(sum[0], round);
    sum[1]          = _mm256_add_epi32(sum[1], round);
    sum[0]          = _mm256_srai_epi32(sum[0], sm_weight_log2_scale);
    sum[1]          = _mm256_srai_epi32(sum[1], sm_weight_log2_scale);
    // width 8: 00 01 02 03 04 05 06 07  10 11 12 13 14 15 16 17
    // width 16: 0 1 2 3 4 5 6 7  8 9 A b C D E F
    return _mm256_packs_epi32(sum[0], sum[1]);
}

static INLINE void smooth_v_pred_8x2(const __m256i weights, const __m256i rep, const __m256i* const ab,
                                     uint16_t** const dst, const ptrdiff_t stride) {
    // 00 01 02 03 04 05 06 07  10 11 12 13 14 15 16 17
    const __m256i d = smooth_v_pred_kernel(weights, rep, ab);
    _mm_storeu_si128((__m128i*)*dst, _mm256_castsi256_si128(d));
    *dst += stride;
    _mm_storeu_si128((__m128i*)*dst, _mm256_extracti128_si256(d, 1));
    *dst += stride;
}

static INLINE void smooth_v_pred_8x4(const uint16_t* const sm_weights_h, const __m256i* const rep,
                                     const __m256i* const ab, uint16_t** const dst, const ptrdiff_t stride) {
    const __m256i weights = _mm256_loadu_si256((const __m256i*)sm_weights_h);
    smooth_v_pred_8x2(weights, rep[0], ab, dst, stride);
    smooth_v_pred_8x2(weights, rep[1], ab, dst, stride);
}

static INLINE void smooth_v_pred_8x8(const uint16_t* const sm_weights_h, const __m256i* const rep,
                                     const __m256i* const ab, uint16_t** const dst, const ptrdiff_t stride) {
    smooth_v_pred_8x4(sm_weights_h + 0, rep, ab, dst, stride);
    smooth_v_pred_8x4(sm_weights_h + 16, rep, ab, dst, stride);
}

// 8x4

void svt_aom_highbd_smooth_v_predictor_8x4_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                const uint16_t* left, int32_t bd) {
    __m256i ab[2], rep[2];
    (void)bd;

    smooth_v_init_8(above, left, 4, ab, rep);
    smooth_v_pred_8x4(sm_weights_d_4, rep, ab, &dst, stride);
}

// 8x8

void svt_aom_highbd_smooth_v_predictor_8x8_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                const uint16_t* left, int32_t bd) {
    __m256i ab[2], rep[2];
    (void)bd;

    smooth_v_init_8(above, left, 8, ab, rep);

    smooth_v_pred_8x8(sm_weights_d_8, rep, ab, &dst, stride);
}

// 8x16

void svt_aom_highbd_smooth_v_predictor_8x16_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                 const uint16_t* left, int32_t bd) {
    __m256i ab[2], rep[2];
    (void)bd;

    smooth_v_init_8(above, left, 16, ab, rep);

    for (int32_t i = 0; i < 2; i++) {
        smooth_v_pred_8x8(sm_weights_d_16 + 32 * i, rep, ab, &dst, stride);
    }
}

// 8x32

void svt_aom_highbd_smooth_v_predictor_8x32_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                 const uint16_t* left, int32_t bd) {
    __m256i ab[2], rep[2];
    (void)bd;

    smooth_v_init_8(above, left, 32, ab, rep);

    for (int32_t i = 0; i < 4; i++) {
        smooth_v_pred_8x8(sm_weights_d_32 + 32 * i, rep, ab, &dst, stride);
    }
}

// -----------------------------------------------------------------------------
// 16xN

static INLINE void smooth_v_prepare_ab(const uint16_t* const above, const __m256i b, __m256i* const ab) {
    const __m256i a = _mm256_loadu_si256((const __m256i*)above);
    ab[0]           = _mm256_unpacklo_epi16(a, b);
    ab[1]           = _mm256_unpackhi_epi16(a, b);
}

static INLINE void smooth_v_init_16(const uint16_t* const above, const uint16_t* const left, const int32_t h,
                                    __m256i* const ab, __m256i* const rep) {
    const __m256i b = _mm256_set1_epi16((uint16_t)left[h - 1]);
    smooth_v_prepare_ab(above, b, ab);

    rep[0] = _mm256_set1_epi32(0x03020100);
    rep[1] = _mm256_set1_epi32(0x07060504);
    rep[2] = _mm256_set1_epi32(0x0B0A0908);
    rep[3] = _mm256_set1_epi32(0x0F0E0D0C);
}

static INLINE void smooth_v_pred_16(const __m256i weights, const __m256i rep, const __m256i* const ab,
                                    uint16_t** const dst, const ptrdiff_t stride) {
    const __m256i d = smooth_v_pred_kernel(weights, rep, ab);
    _mm256_storeu_si256((__m256i*)*dst, d);
    *dst += stride;
}

static INLINE void smooth_v_pred_16x4(const uint16_t* const sm_weights_h, const __m256i* const rep,
                                      const __m256i* const ab, uint16_t** const dst, const ptrdiff_t stride) {
    const __m256i weights = _mm256_loadu_si256((const __m256i*)sm_weights_h);
    smooth_v_pred_16(weights, rep[0], ab, dst, stride);
    smooth_v_pred_16(weights, rep[1], ab, dst, stride);
    smooth_v_pred_16(weights, rep[2], ab, dst, stride);
    smooth_v_pred_16(weights, rep[3], ab, dst, stride);
}

static INLINE void smooth_v_pred_16x8(const uint16_t* const sm_weights_h, const __m256i* const rep,
                                      const __m256i* const ab, uint16_t** const dst, const ptrdiff_t stride) {
    smooth_v_pred_16x4(sm_weights_h + 0, rep, ab, dst, stride);
    smooth_v_pred_16x4(sm_weights_h + 16, rep, ab, dst, stride);
}

// 16x4

void svt_aom_highbd_smooth_v_predictor_16x4_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                 const uint16_t* left, int32_t bd) {
    __m256i ab[2], rep[4];
    (void)bd;

    smooth_v_init_16(above, left, 4, ab, rep);
    smooth_v_pred_16x4(sm_weights_d_4, rep, ab, &dst, stride);
}

// 16x8

void svt_aom_highbd_smooth_v_predictor_16x8_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                 const uint16_t* left, int32_t bd) {
    __m256i ab[2], rep[4];
    (void)bd;

    smooth_v_init_16(above, left, 8, ab, rep);

    smooth_v_pred_16x8(sm_weights_d_8, rep, ab, &dst, stride);
}

// 16x16

void svt_aom_highbd_smooth_v_predictor_16x16_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                  const uint16_t* left, int32_t bd) {
    __m256i ab[2], rep[4];
    (void)bd;

    smooth_v_init_16(above, left, 16, ab, rep);

    for (int32_t i = 0; i < 2; i++) {
        smooth_v_pred_16x8(sm_weights_d_16 + 32 * i, rep, ab, &dst, stride);
    }
}

// 16x32

void svt_aom_highbd_smooth_v_predictor_16x32_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                  const uint16_t* left, int32_t bd) {
    __m256i ab[2], rep[4];
    (void)bd;

    smooth_v_init_16(above, left, 32, ab, rep);

    for (int32_t i = 0; i < 4; i++) {
        smooth_v_pred_16x8(sm_weights_d_32 + 32 * i, rep, ab, &dst, stride);
    }
}

// 16x64

void svt_aom_highbd_smooth_v_predictor_16x64_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                  const uint16_t* left, int32_t bd) {
    __m256i ab[2], rep[4];
    (void)bd;

    smooth_v_init_16(above, left, 64, ab, rep);

    for (int32_t i = 0; i < 8; i++) {
        smooth_v_pred_16x8(sm_weights_d_64 + 32 * i, rep, ab, &dst, stride);
    }
}

// -----------------------------------------------------------------------------
// 32xN

static INLINE void smooth_v_init_32(const uint16_t* const above, const uint16_t* const left, const int32_t h,
                                    __m256i* const ab, __m256i* const rep) {
    const __m256i b = _mm256_set1_epi16((uint16_t)left[h - 1]);
    smooth_v_prepare_ab(above + 0x00, b, ab + 0);
    smooth_v_prepare_ab(above + 0x10, b, ab + 2);

    rep[0] = _mm256_set1_epi32(0x03020100);
    rep[1] = _mm256_set1_epi32(0x07060504);
    rep[2] = _mm256_set1_epi32(0x0B0A0908);
    rep[3] = _mm256_set1_epi32(0x0F0E0D0C);
}

static INLINE void smooth_v_pred_32(const __m256i weights, const __m256i rep, const __m256i* const ab,
                                    uint16_t** const dst, const ptrdiff_t stride) {
    __m256i d;

    //  0  1  2  3  4  5  6  7   8  9 10 11 12 13 14 15
    d = smooth_v_pred_kernel(weights, rep, ab + 0);
    _mm256_storeu_si256((__m256i*)(*dst + 0x00), d);

    // 16 17 18 19 20 21 22 23  24 25 26 27 28 29 30 31
    d = smooth_v_pred_kernel(weights, rep, ab + 2);
    _mm256_storeu_si256((__m256i*)(*dst + 0x10), d);
    *dst += stride;
}

static INLINE void smooth_v_pred_32x4(const uint16_t* const sm_weights_h, const __m256i* const rep,
                                      const __m256i* const ab, uint16_t** const dst, const ptrdiff_t stride) {
    const __m256i weights = _mm256_loadu_si256((const __m256i*)sm_weights_h);
    smooth_v_pred_32(weights, rep[0], ab, dst, stride);
    smooth_v_pred_32(weights, rep[1], ab, dst, stride);
    smooth_v_pred_32(weights, rep[2], ab, dst, stride);
    smooth_v_pred_32(weights, rep[3], ab, dst, stride);
}

static INLINE void smooth_v_pred_32x8(const uint16_t* const sm_weights_h, const __m256i* const rep,
                                      const __m256i* const ab, uint16_t** const dst, const ptrdiff_t stride) {
    smooth_v_pred_32x4(sm_weights_h + 0, rep, ab, dst, stride);
    smooth_v_pred_32x4(sm_weights_h + 16, rep, ab, dst, stride);
}

// 32x8

void svt_aom_highbd_smooth_v_predictor_32x8_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                 const uint16_t* left, int32_t bd) {
    __m256i ab[4], rep[4];
    (void)bd;

    smooth_v_init_32(above, left, 8, ab, rep);

    smooth_v_pred_32x8(sm_weights_d_8, rep, ab, &dst, stride);
}

// 32x16

void svt_aom_highbd_smooth_v_predictor_32x16_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                  const uint16_t* left, int32_t bd) {
    __m256i ab[4], rep[4];
    (void)bd;

    smooth_v_init_32(above, left, 16, ab, rep);

    for (int32_t i = 0; i < 2; i++) {
        smooth_v_pred_32x8(sm_weights_d_16 + 32 * i, rep, ab, &dst, stride);
    }
}

// 32x32

void svt_aom_highbd_smooth_v_predictor_32x32_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                  const uint16_t* left, int32_t bd) {
    __m256i ab[4], rep[4];
    (void)bd;

    smooth_v_init_32(above, left, 32, ab, rep);

    for (int32_t i = 0; i < 4; i++) {
        smooth_v_pred_32x8(sm_weights_d_32 + 32 * i, rep, ab, &dst, stride);
    }
}

// 32x64

void svt_aom_highbd_smooth_v_predictor_32x64_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                  const uint16_t* left, int32_t bd) {
    __m256i ab[4], rep[4];
    (void)bd;

    smooth_v_init_32(above, left, 64, ab, rep);

    for (int32_t i = 0; i < 8; i++) {
        smooth_v_pred_32x8(sm_weights_d_64 + 32 * i, rep, ab, &dst, stride);
    }
}

// -----------------------------------------------------------------------------
// 64xN

static INLINE void smooth_v_init_64(const uint16_t* const above, const uint16_t* const left, const int32_t h,
                                    __m256i* const ab, __m256i* const rep) {
    const __m256i b = _mm256_set1_epi16((uint16_t)left[h - 1]);
    smooth_v_prepare_ab(above + 0x00, b, ab + 0);
    smooth_v_prepare_ab(above + 0x10, b, ab + 2);
    smooth_v_prepare_ab(above + 0x20, b, ab + 4);
    smooth_v_prepare_ab(above + 0x30, b, ab + 6);

    rep[0] = _mm256_set1_epi32(0x03020100);
    rep[1] = _mm256_set1_epi32(0x07060504);
    rep[2] = _mm256_set1_epi32(0x0B0A0908);
    rep[3] = _mm256_set1_epi32(0x0F0E0D0C);
}

static INLINE void smooth_v_pred_64(const __m256i weights, const __m256i rep, const __m256i* const ab,
                                    uint16_t** const dst, const ptrdiff_t stride) {
    __m256i d;

    //  0  1  2  3  4  5  6  7   8  9 10 11 12 13 14 15
    d = smooth_v_pred_kernel(weights, rep, ab + 0);
    _mm256_storeu_si256((__m256i*)(*dst + 0x00), d);

    // 16 17 18 19 20 21 22 23  24 25 26 27 28 29 30 31
    d = smooth_v_pred_kernel(weights, rep, ab + 2);
    _mm256_storeu_si256((__m256i*)(*dst + 0x10), d);

    // 32 33 34 35 36 37 38 39  40 41 42 43 44 45 46 47
    d = smooth_v_pred_kernel(weights, rep, ab + 4);
    _mm256_storeu_si256((__m256i*)(*dst + 0x20), d);

    // 48 49 50 51 52 53 54 55  56 57 58 59 60 61 62 63
    d = smooth_v_pred_kernel(weights, rep, ab + 6);
    _mm256_storeu_si256((__m256i*)(*dst + 0x30), d);
    *dst += stride;
}

static INLINE void smooth_v_pred_64x4(const uint16_t* const sm_weights_h, const __m256i* const rep,
                                      const __m256i* const ab, uint16_t** const dst, const ptrdiff_t stride) {
    const __m256i weights = _mm256_loadu_si256((const __m256i*)sm_weights_h);
    smooth_v_pred_64(weights, rep[0], ab, dst, stride);
    smooth_v_pred_64(weights, rep[1], ab, dst, stride);
    smooth_v_pred_64(weights, rep[2], ab, dst, stride);
    smooth_v_pred_64(weights, rep[3], ab, dst, stride);
}

static INLINE void smooth_v_pred_64x8(const uint16_t* const sm_weights_h, const __m256i* const rep,
                                      const __m256i* const ab, uint16_t** const dst, const ptrdiff_t stride) {
    smooth_v_pred_64x4(sm_weights_h + 0, rep, ab, dst, stride);
    smooth_v_pred_64x4(sm_weights_h + 16, rep, ab, dst, stride);
}

// 64x16

void svt_aom_highbd_smooth_v_predictor_64x16_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                  const uint16_t* left, int32_t bd) {
    __m256i ab[8], rep[4];
    (void)bd;

    smooth_v_init_64(above, left, 16, ab, rep);

    for (int32_t i = 0; i < 2; i++) {
        smooth_v_pred_64x8(sm_weights_d_16 + 32 * i, rep, ab, &dst, stride);
    }
}

// 64x32

void svt_aom_highbd_smooth_v_predictor_64x32_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                  const uint16_t* left, int32_t bd) {
    __m256i ab[8], rep[4];
    (void)bd;

    smooth_v_init_64(above, left, 32, ab, rep);

    for (int32_t i = 0; i < 4; i++) {
        smooth_v_pred_64x8(sm_weights_d_32 + 32 * i, rep, ab, &dst, stride);
    }
}

// 64x64

void svt_aom_highbd_smooth_v_predictor_64x64_avx2(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                  const uint16_t* left, int32_t bd) {
    __m256i ab[8], rep[4];
    (void)bd;

    smooth_v_init_64(above, left, 64, ab, rep);

    for (int32_t i = 0; i < 8; i++) {
        smooth_v_pred_64x8(sm_weights_d_64 + 32 * i, rep, ab, &dst, stride);
    }
}
