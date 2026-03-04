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
#include <immintrin.h>
#include "highbd_intrapred_sse2.h"
#include "common_dsp_rtcd.h"
#include "intra_prediction.h"
#include "synonyms_avx512.h"

#define FOUR 4U

EB_ALIGN(32)
static const uint16_t sm_weights_32[64] = {
    255, 1,   240, 16,  225, 31,  210, 46, //  0  1  2  3
    145, 111, 133, 123, 122, 134, 111, 145, //  8  9 10 11
    66,  190, 59,  197, 52,  204, 45,  211, // 16 17 18 19
    21,  235, 17,  239, 14,  242, 12,  244, // 24 25 26 27
    196, 60,  182, 74,  169, 87,  157, 99, //  4  5  6  7
    101, 155, 92,  164, 83,  173, 74,  182, // 12 13 14 15
    39,  217, 34,  222, 29,  227, 25,  231, // 20 21 22 23
    10,  246, 9,   247, 8,   248, 8,   248 // 28 29 30 31
};

// bs = 8
EB_ALIGN(32)
static const uint16_t sm_weights_d_8[64] = {
    255, 1,   197, 59,  146, 110, 105, 151, //  0  1  2  3
    255, 1,   197, 59,  146, 110, 105, 151, //  0  1  2  3
    255, 1,   197, 59,  146, 110, 105, 151, //  0  1  2  3
    255, 1,   197, 59,  146, 110, 105, 151, //  0  1  2  3
    73,  183, 50,  206, 37,  219, 32,  224, //  4  5  6  7
    73,  183, 50,  206, 37,  219, 32,  224, //  4  5  6  7
    73,  183, 50,  206, 37,  219, 32,  224, //  4  5  6  7
    73,  183, 50,  206, 37,  219, 32,  224 //  4  5  6  7
};

//bs = 16
EB_ALIGN(32)
static const uint16_t sm_weights_d_16[128] = {
    255, 1,   225, 31,  196, 60,  170, 86, //  0  1  2  3
    255, 1,   225, 31,  196, 60,  170, 86, //  0  1  2  3
    255, 1,   225, 31,  196, 60,  170, 86, //  0  1  2  3
    255, 1,   225, 31,  196, 60,  170, 86, //  0  1  2  3
    145, 111, 123, 133, 102, 154, 84,  172, //  4  5  6  7
    145, 111, 123, 133, 102, 154, 84,  172, //  4  5  6  7
    145, 111, 123, 133, 102, 154, 84,  172, //  4  5  6  7
    145, 111, 123, 133, 102, 154, 84,  172, //  4  5  6  7
    68,  188, 54,  202, 43,  213, 33,  223, //  8  9 10 11
    68,  188, 54,  202, 43,  213, 33,  223, //  8  9 10 11
    68,  188, 54,  202, 43,  213, 33,  223, //  8  9 10 11
    68,  188, 54,  202, 43,  213, 33,  223, //  8  9 10 11
    26,  230, 20,  236, 17,  239, 16,  240, // 12 13 14 15
    26,  230, 20,  236, 17,  239, 16,  240, // 12 13 14 15
    26,  230, 20,  236, 17,  239, 16,  240, // 12 13 14 15
    26,  230, 20,  236, 17,  239, 16,  240 // 12 13 14 15
};

// bs = 32
EB_ALIGN(32)
static const uint16_t sm_weights_d_32[256] = {
    255, 1,   240, 16,  225, 31,  210, 46, //  0  1  2  3
    255, 1,   240, 16,  225, 31,  210, 46, //  0  1  2  3
    255, 1,   240, 16,  225, 31,  210, 46, //  0  1  2  3
    255, 1,   240, 16,  225, 31,  210, 46, //  0  1  2  3
    196, 60,  182, 74,  169, 87,  157, 99, //  4  5  6  7
    196, 60,  182, 74,  169, 87,  157, 99, //  4  5  6  7
    196, 60,  182, 74,  169, 87,  157, 99, //  4  5  6  7
    196, 60,  182, 74,  169, 87,  157, 99, //  4  5  6  7
    145, 111, 133, 123, 122, 134, 111, 145, //  8  9 10 11
    145, 111, 133, 123, 122, 134, 111, 145, //  8  9 10 11
    145, 111, 133, 123, 122, 134, 111, 145, //  8  9 10 11
    145, 111, 133, 123, 122, 134, 111, 145, //  8  9 10 11
    101, 155, 92,  164, 83,  173, 74,  182, // 12 13 14 15
    101, 155, 92,  164, 83,  173, 74,  182, // 12 13 14 15
    101, 155, 92,  164, 83,  173, 74,  182, // 12 13 14 15
    101, 155, 92,  164, 83,  173, 74,  182, // 12 13 14 15
    66,  190, 59,  197, 52,  204, 45,  211, // 16 17 18 19
    66,  190, 59,  197, 52,  204, 45,  211, // 16 17 18 19
    66,  190, 59,  197, 52,  204, 45,  211, // 16 17 18 19
    66,  190, 59,  197, 52,  204, 45,  211, // 16 17 18 19
    39,  217, 34,  222, 29,  227, 25,  231, // 20 21 22 23
    39,  217, 34,  222, 29,  227, 25,  231, // 20 21 22 23
    39,  217, 34,  222, 29,  227, 25,  231, // 20 21 22 23
    39,  217, 34,  222, 29,  227, 25,  231, // 20 21 22 23
    21,  235, 17,  239, 14,  242, 12,  244, // 24 25 26 27
    21,  235, 17,  239, 14,  242, 12,  244, // 24 25 26 27
    21,  235, 17,  239, 14,  242, 12,  244, // 24 25 26 27
    21,  235, 17,  239, 14,  242, 12,  244, // 24 25 26 27
    10,  246, 9,   247, 8,   248, 8,   248, // 28 29 30 31
    10,  246, 9,   247, 8,   248, 8,   248, // 28 29 30 31
    10,  246, 9,   247, 8,   248, 8,   248, // 28 29 30 31
    10,  246, 9,   247, 8,   248, 8,   248 // 28 29 30 31
};

// bs = 64
EB_ALIGN(32)
static const uint16_t sm_weights_d_64[512] = {
    255, 1,   248, 8,   240, 16,  233, 23, //  0  1  2  3
    255, 1,   248, 8,   240, 16,  233, 23, //  0  1  2  3
    255, 1,   248, 8,   240, 16,  233, 23, //  0  1  2  3
    255, 1,   248, 8,   240, 16,  233, 23, //  0  1  2  3
    225, 31,  218, 38,  210, 46,  203, 53, //  4  5  6  7
    225, 31,  218, 38,  210, 46,  203, 53, //  4  5  6  7
    225, 31,  218, 38,  210, 46,  203, 53, //  4  5  6  7
    225, 31,  218, 38,  210, 46,  203, 53, //  4  5  6  7
    196, 60,  189, 67,  182, 74,  176, 80, //  8  9 10 11
    196, 60,  189, 67,  182, 74,  176, 80, //  8  9 10 11
    196, 60,  189, 67,  182, 74,  176, 80, //  8  9 10 11
    196, 60,  189, 67,  182, 74,  176, 80, //  8  9 10 11
    169, 87,  163, 93,  156, 100, 150, 106, // 12 13 14 15
    169, 87,  163, 93,  156, 100, 150, 106, // 12 13 14 15
    169, 87,  163, 93,  156, 100, 150, 106, // 12 13 14 15
    169, 87,  163, 93,  156, 100, 150, 106, // 12 13 14 15
    144, 112, 138, 118, 133, 123, 127, 129, // 16 17 18 19
    144, 112, 138, 118, 133, 123, 127, 129, // 16 17 18 19
    144, 112, 138, 118, 133, 123, 127, 129, // 16 17 18 19
    144, 112, 138, 118, 133, 123, 127, 129, // 16 17 18 19
    121, 135, 116, 140, 111, 145, 106, 150, // 20 21 22 23
    121, 135, 116, 140, 111, 145, 106, 150, // 20 21 22 23
    121, 135, 116, 140, 111, 145, 106, 150, // 20 21 22 23
    121, 135, 116, 140, 111, 145, 106, 150, // 20 21 22 23
    101, 155, 96,  160, 91,  165, 86,  170, // 24 25 26 27
    101, 155, 96,  160, 91,  165, 86,  170, // 24 25 26 27
    101, 155, 96,  160, 91,  165, 86,  170, // 24 25 26 27
    101, 155, 96,  160, 91,  165, 86,  170, // 24 25 26 27
    82,  174, 77,  179, 73,  183, 69,  187, // 28 29 30 31
    82,  174, 77,  179, 73,  183, 69,  187, // 28 29 30 31
    82,  174, 77,  179, 73,  183, 69,  187, // 28 29 30 31
    82,  174, 77,  179, 73,  183, 69,  187, // 28 29 30 31
    65,  191, 61,  195, 57,  199, 54,  202, // 32 33 34 35
    65,  191, 61,  195, 57,  199, 54,  202, // 32 33 34 35
    65,  191, 61,  195, 57,  199, 54,  202, // 32 33 34 35
    65,  191, 61,  195, 57,  199, 54,  202, // 32 33 34 35
    50,  206, 47,  209, 44,  212, 41,  215, // 36 37 38 39
    50,  206, 47,  209, 44,  212, 41,  215, // 36 37 38 39
    50,  206, 47,  209, 44,  212, 41,  215, // 36 37 38 39
    50,  206, 47,  209, 44,  212, 41,  215, // 36 37 38 39
    38,  218, 35,  221, 32,  224, 29,  227, // 40 41 42 43
    38,  218, 35,  221, 32,  224, 29,  227, // 40 41 42 43
    38,  218, 35,  221, 32,  224, 29,  227, // 40 41 42 43
    38,  218, 35,  221, 32,  224, 29,  227, // 40 41 42 43
    27,  229, 25,  231, 22,  234, 20,  236, // 44 45 46 47
    27,  229, 25,  231, 22,  234, 20,  236, // 44 45 46 47
    27,  229, 25,  231, 22,  234, 20,  236, // 44 45 46 47
    27,  229, 25,  231, 22,  234, 20,  236, // 44 45 46 47
    18,  238, 16,  240, 15,  241, 13,  243, // 48 49 50 51
    18,  238, 16,  240, 15,  241, 13,  243, // 48 49 50 51
    18,  238, 16,  240, 15,  241, 13,  243, // 48 49 50 51
    18,  238, 16,  240, 15,  241, 13,  243, // 48 49 50 51
    12,  244, 10,  246, 9,   247, 8,   248, // 52 53 54 55
    12,  244, 10,  246, 9,   247, 8,   248, // 52 53 54 55
    12,  244, 10,  246, 9,   247, 8,   248, // 52 53 54 55
    12,  244, 10,  246, 9,   247, 8,   248, // 52 53 54 55
    7,   249, 6,   250, 6,   250, 5,   251, // 56 57 58 59
    7,   249, 6,   250, 6,   250, 5,   251, // 56 57 58 59
    7,   249, 6,   250, 6,   250, 5,   251, // 56 57 58 59
    7,   249, 6,   250, 6,   250, 5,   251, // 56 57 58 59
    5,   251, 4,   252, 4,   252, 4,   252, // 60 61 62 63
    5,   251, 4,   252, 4,   252, 4,   252, // 60 61 62 63
    5,   251, 4,   252, 4,   252, 4,   252, // 60 61 62 63
    5,   251, 4,   252, 4,   252, 4,   252 // 60 61 62 63
};

// bs = 64
EB_ALIGN(32)
static const uint16_t sm_weights_64[128] = {
    255, 1,   248, 8,   240, 16,  233, 23, //  0  1  2  3
    196, 60,  189, 67,  182, 74,  176, 80, //  8  9 10 11
    144, 112, 138, 118, 133, 123, 127, 129, // 16 17 18 19
    101, 155, 96,  160, 91,  165, 86,  170, // 24 25 26 27
    225, 31,  218, 38,  210, 46,  203, 53, //  4  5  6  7
    169, 87,  163, 93,  156, 100, 150, 106, // 12 13 14 15
    121, 135, 116, 140, 111, 145, 106, 150, // 20 21 22 23
    82,  174, 77,  179, 73,  183, 69,  187, // 28 29 30 31
    65,  191, 61,  195, 57,  199, 54,  202, // 32 33 34 35
    38,  218, 35,  221, 32,  224, 29,  227, // 40 41 42 43
    18,  238, 16,  240, 15,  241, 13,  243, // 48 49 50 51
    7,   249, 6,   250, 6,   250, 5,   251, // 56 57 58 59
    50,  206, 47,  209, 44,  212, 41,  215, // 36 37 38 39
    27,  229, 25,  231, 22,  234, 20,  236, // 44 45 46 47
    12,  244, 10,  246, 9,   247, 8,   248, // 52 53 54 55
    5,   251, 4,   252, 4,   252, 4,   252 // 60 61 62 63
};

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

static INLINE void dc_common_predictor_32xh_kernel_avx512(uint16_t* dst, const ptrdiff_t stride, const int32_t h,
                                                          const __m512i dc) {
    for (int32_t i = 0; i < h; i++) {
        _mm512_storeu_si512((__m512i*)dst, dc);
        dst += stride;
    }
}

static INLINE void dc_common_predictor_32xh(uint16_t* const dst, const ptrdiff_t stride, const int32_t h,
                                            const __m128i dc) {
    const __m512i expected_dc = _mm512_broadcastw_epi16(dc);
    dc_common_predictor_32xh_kernel_avx512(dst, stride, h, expected_dc);
}

static INLINE void dc_common_predictor_64xh_kernel_avx512(uint16_t* dst, const ptrdiff_t stride, const int32_t h,
                                                          const __m512i dc) {
    for (int32_t i = 0; i < h; i++) {
        _mm512_storeu_si512((__m512i*)(dst + 0x00), dc);
        _mm512_storeu_si512((__m512i*)(dst + 0x20), dc);
        dst += stride;
    }
}

static INLINE void dc_common_predictor_64xh(uint16_t* const dst, const ptrdiff_t stride, const int32_t h,
                                            const __m128i dc) {
    const __m512i expected_dc = _mm512_broadcastw_epi16(dc);
    dc_common_predictor_64xh_kernel_avx512(dst, stride, h, expected_dc);
}

static INLINE __m128i dc_sum_16(const uint16_t* const src) {
    const __m256i s    = _mm256_loadu_si256((const __m256i*)src);
    const __m128i s_lo = _mm256_castsi256_si128(s);
    const __m128i s_hi = _mm256_extracti128_si256(s, 1);
    const __m128i sum  = _mm_add_epi16(s_lo, s_hi);
    return dc_sum_8x16bit(sum);
}

static INLINE __m128i dc_sum_32(const uint16_t* const src) {
    const __m512i s32 = _mm512_loadu_si512((const __m512i*)src);
    const __m256i s0  = _mm512_castsi512_si256(s32);
    const __m256i s1  = _mm512_extracti64x4_epi64(s32, 1);
    const __m256i sum = _mm256_add_epi16(s0, s1);
    return dc_sum_large(sum);
}

static INLINE __m128i dc_sum_64(const uint16_t* const src) {
    const __m512i s0  = _mm512_loadu_si512((const __m512i*)(src + 0x00));
    const __m512i s1  = _mm512_loadu_si512((const __m512i*)(src + 0x20));
    const __m512i s01 = _mm512_add_epi16(s0, s1);

    const __m256i s2 = _mm512_castsi512_si256(s01);
    const __m256i s3 = _mm512_extracti64x4_epi64(s01, 1);

    const __m256i sum = _mm256_add_epi16(s2, s3);
    return dc_sum_large(sum);
}

// 32xN

void aom_highbd_dc_left_predictor_32x8_avx512(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
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

void aom_highbd_dc_left_predictor_32x16_avx512(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
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

void aom_highbd_dc_left_predictor_32x32_avx512(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
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

void aom_highbd_dc_left_predictor_32x64_avx512(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
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

void aom_highbd_dc_left_predictor_64x16_avx512(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
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

void aom_highbd_dc_left_predictor_64x32_avx512(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
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

void aom_highbd_dc_left_predictor_64x64_avx512(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
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

/*highbd dc top predictors */

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

void aom_highbd_dc_top_predictor_32x8_avx512(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                             const uint16_t* left, int32_t bd) {
    (void)left;

    dc_top_predictor_32xh(dst, stride, above, 8, bd);
}

void aom_highbd_dc_top_predictor_32x16_avx512(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                              const uint16_t* left, int32_t bd) {
    (void)left;
    dc_top_predictor_32xh(dst, stride, above, 16, bd);
}

void aom_highbd_dc_top_predictor_32x32_avx512(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                              const uint16_t* left, int32_t bd) {
    (void)left;
    dc_top_predictor_32xh(dst, stride, above, 32, bd);
}

void aom_highbd_dc_top_predictor_32x64_avx512(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
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

void aom_highbd_dc_top_predictor_64x16_avx512(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                              const uint16_t* left, int32_t bd) {
    (void)left;
    dc_top_predictor_64xh(dst, stride, above, 16, bd);
}

void aom_highbd_dc_top_predictor_64x32_avx512(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                              const uint16_t* left, int32_t bd) {
    (void)left;
    dc_top_predictor_64xh(dst, stride, above, 32, bd);
}

void aom_highbd_dc_top_predictor_64x64_avx512(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                              const uint16_t* left, int32_t bd) {
    (void)left;
    dc_top_predictor_64xh(dst, stride, above, 64, bd);
}

/* highbd dc predictor */

// 32xN

static INLINE __m128i dc_sum_8_32(const uint16_t* const src_8, const uint16_t* const src_32) {
    const __m128i s_8      = _mm_loadu_si128((const __m128i*)src_8);
    const __m512i s32_01   = _mm512_loadu_si512((const __m512i*)(src_32 + 0x00));
    const __m256i s_32_0   = _mm512_castsi512_si256(s32_01);
    const __m256i s_32_1   = _mm512_extracti64x4_epi64(s32_01, 1);
    const __m256i s_32     = _mm256_add_epi16(s_32_0, s_32_1);
    const __m128i s_lo     = _mm256_castsi256_si128(s_32);
    const __m128i s_hi     = _mm256_extracti128_si256(s_32, 1);
    const __m128i s_16_sum = _mm_add_epi16(s_lo, s_hi);
    const __m128i sum      = _mm_add_epi16(s_8, s_16_sum);
    return dc_sum_8x16bit_large(sum);
}

static INLINE __m128i dc_sum_16_32(const uint16_t* const src_16, const uint16_t* const src_32) {
    const __m256i s_16   = _mm256_loadu_si256((const __m256i*)src_16);
    const __m512i s32_01 = _mm512_loadu_si512((const __m512i*)(src_32 + 0x00));
    const __m256i s_32_0 = _mm512_castsi512_si256(s32_01);
    const __m256i s_32_1 = _mm512_extracti64x4_epi64(s32_01, 1);
    const __m256i sum0   = _mm256_add_epi16(s_16, s_32_0);
    const __m256i sum    = _mm256_add_epi16(sum0, s_32_1);
    return dc_sum_large(sum);
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

static INLINE __m128i dc_sum_32_32(const uint16_t* const src0, const uint16_t* const src1) {
    const __m512i s_32_0    = _mm512_loadu_si512((const __m512i*)(src0 + 0x00));
    const __m512i s_32_1    = _mm512_loadu_si512((const __m512i*)(src1 + 0x00));
    const __m512i sum_32_01 = _mm512_add_epi16(s_32_0, s_32_1);
    const __m256i sum_16_0  = _mm512_castsi512_si256(sum_32_01);
    const __m256i sum_16_1  = _mm512_extracti64x4_epi64(sum_32_01, 1);
    const __m256i sum       = _mm256_add_epi16(sum_16_0, sum_16_1);
    return dc_sum_large(sum);
}

static INLINE __m128i dc_sum_16_64(const uint16_t* const src_16, const uint16_t* const src_64) {
    const __m256i s_16      = _mm256_loadu_si256((const __m256i*)src_16);
    const __m512i s_64_0    = _mm512_loadu_si512((const __m512i*)(src_64 + 0x00));
    const __m512i s_64_1    = _mm512_loadu_si512((const __m512i*)(src_64 + 0x20));
    const __m512i sum_64_01 = _mm512_add_epi16(s_64_1, s_64_0);
    const __m256i s1        = _mm512_castsi512_si256(sum_64_01);
    const __m256i s2        = _mm512_extracti64x4_epi64(sum_64_01, 1);
    const __m256i s3        = _mm256_add_epi16(s1, s_16);
    const __m256i sum       = _mm256_add_epi16(s2, s3);
    return dc_sum_larger(sum);
}

static INLINE __m128i dc_sum_32_64(const uint16_t* const src_32, const uint16_t* const src_64) {
    const __m512i s_32_0 = _mm512_loadu_si512((const __m512i*)(src_32 + 0x00));
    const __m512i s_64_0 = _mm512_loadu_si512((const __m256i*)(src_64 + 0x00));
    const __m512i s_64_1 = _mm512_loadu_si512((const __m256i*)(src_64 + 0x20));

    const __m512i sum0 = _mm512_add_epi16(s_32_0, s_64_0);
    const __m512i sum1 = _mm512_add_epi16(sum0, s_64_1);

    const __m256i sum2 = _mm512_castsi512_si256(sum1);
    const __m256i sum3 = _mm512_extracti64x4_epi64(sum1, 1);
    const __m256i sum  = _mm256_add_epi16(sum2, sum3);
    return dc_sum_larger(sum);
}

static INLINE __m128i dc_sum_64_64(const uint16_t* const src0, const uint16_t* const src1) {
    const __m512i s0 = _mm512_loadu_si512((const __m512i*)(src0 + 0x00));
    const __m512i s1 = _mm512_loadu_si512((const __m256i*)(src0 + 0x20));
    const __m512i s2 = _mm512_loadu_si512((const __m256i*)(src1 + 0x00));
    const __m512i s3 = _mm512_loadu_si512((const __m256i*)(src1 + 0x20));

    const __m512i sum01 = _mm512_add_epi16(s0, s1);
    const __m512i sum23 = _mm512_add_epi16(s2, s3);
    const __m512i sum03 = _mm512_add_epi16(sum01, sum23);

    const __m256i sum03_1 = _mm512_castsi512_si256(sum03);
    const __m256i sum03_2 = _mm512_extracti64x4_epi64(sum03, 1);

    const __m256i sum = _mm256_add_epi16(sum03_1, sum03_2);
    return dc_sum_larger(sum);
}

void aom_highbd_dc_predictor_32x8_avx512(uint16_t* dst, ptrdiff_t stride, const uint16_t* above, const uint16_t* left,
                                         int32_t bd) {
    (void)bd;
    __m128i  sum   = dc_sum_8_32(left, above);
    uint32_t sum32 = _mm_cvtsi128_si32(sum);
    sum32 += 20;
    sum32 /= 40;
    const __m512i dc = _mm512_set1_epi16((int16_t)sum32);

    dc_common_predictor_32xh_kernel_avx512(dst, stride, 8, dc);
}

void aom_highbd_dc_predictor_32x16_avx512(uint16_t* dst, ptrdiff_t stride, const uint16_t* above, const uint16_t* left,
                                          int32_t bd) {
    (void)bd;
    __m128i  sum   = dc_sum_16_32(left, above);
    uint32_t sum32 = _mm_cvtsi128_si32(sum);
    sum32 += 24;
    sum32 /= 48;
    const __m512i dc = _mm512_set1_epi16((int16_t)sum32);

    dc_common_predictor_32xh_kernel_avx512(dst, stride, 16, dc);
}

void aom_highbd_dc_predictor_32x32_avx512(uint16_t* dst, ptrdiff_t stride, const uint16_t* above, const uint16_t* left,
                                          int32_t bd) {
    (void)bd;
    __m128i sum = dc_sum_32_32(above, left);
    sum         = _mm_add_epi32(sum, _mm_set1_epi32(32));
    sum         = _mm_srli_epi32(sum, 6);
    dc_common_predictor_32xh(dst, stride, 32, sum);
}

void aom_highbd_dc_predictor_32x64_avx512(uint16_t* dst, ptrdiff_t stride, const uint16_t* above, const uint16_t* left,
                                          int32_t bd) {
    (void)bd;
    __m128i  sum   = dc_sum_32_64(above, left);
    uint32_t sum32 = _mm_cvtsi128_si32(sum);
    sum32 += 48;
    sum32 /= 96;
    const __m512i dc = _mm512_set1_epi16((int16_t)sum32);

    dc_common_predictor_32xh_kernel_avx512(dst, stride, 64, dc);
}

// 64xN

void aom_highbd_dc_predictor_64x16_avx512(uint16_t* dst, ptrdiff_t stride, const uint16_t* above, const uint16_t* left,
                                          int32_t bd) {
    (void)bd;
    __m128i  sum   = dc_sum_16_64(left, above);
    uint32_t sum32 = _mm_cvtsi128_si32(sum);
    sum32 += 40;
    sum32 /= 80;
    const __m512i dc = _mm512_set1_epi16((int16_t)sum32);

    dc_common_predictor_64xh_kernel_avx512(dst, stride, 16, dc);
}

void aom_highbd_dc_predictor_64x32_avx512(uint16_t* dst, ptrdiff_t stride, const uint16_t* above, const uint16_t* left,
                                          int32_t bd) {
    (void)bd;
    __m128i  sum   = dc_sum_32_64(left, above);
    uint32_t sum32 = _mm_cvtsi128_si32(sum);
    sum32 += 48;
    sum32 /= 96;
    const __m512i dc = _mm512_set1_epi16((int16_t)sum32);

    dc_common_predictor_64xh_kernel_avx512(dst, stride, 32, dc);
}

void aom_highbd_dc_predictor_64x64_avx512(uint16_t* dst, ptrdiff_t stride, const uint16_t* above, const uint16_t* left,
                                          int32_t bd) {
    (void)bd;
    __m128i sum = dc_sum_64_64(above, left);
    sum         = _mm_add_epi32(sum, _mm_set1_epi32(64));
    sum         = _mm_srli_epi32(sum, 7);
    dc_common_predictor_64xh(dst, stride, 64, sum);
}

// =============================================================================

// H_PRED

// -----------------------------------------------------------------------------

// 32xN

static INLINE void h_pred_32(uint16_t** const dst, const ptrdiff_t stride, const __m128i left) {
    // Broadcast the 16-bit left pixel to 256-bit register.
    const __m512i row = _mm512_broadcastw_epi16(left);

    _mm512_storeu_si512((__m512i*)(*dst + 0x00), row);
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

void aom_highbd_h_predictor_32x8_avx512(uint16_t* dst, ptrdiff_t stride, const uint16_t* above, const uint16_t* left,
                                        int32_t bd) {
    (void)above;
    (void)bd;

    h_pred_32x8(&dst, stride, left);
}

// 32x64

void aom_highbd_h_predictor_32x64_avx512(uint16_t* dst, ptrdiff_t stride, const uint16_t* above, const uint16_t* left,
                                         int32_t bd) {
    (void)above;
    (void)bd;

    for (int32_t i = 0; i < 8; i++, left += 8) {
        h_pred_32x8(&dst, stride, left);
    }
}

//32x16 32x32
static INLINE void h_store_32_unpacklo(uint16_t** dst, const ptrdiff_t stride, const __m128i* row) {
    const __m128i val  = _mm_unpacklo_epi64(*row, *row);
    const __m512i val2 = _mm512_broadcast_i32x4(val);
    _mm512_storeu_si512((__m512i*)(*dst), val2);
    *dst += stride;
}

static INLINE void h_store_32_unpackhi(uint16_t** dst, const ptrdiff_t stride, const __m128i* row) {
    const __m128i val  = _mm_unpackhi_epi64(*row, *row);
    const __m512i val2 = _mm512_broadcast_i32x4(val);
    _mm512_storeu_si512((__m512i*)(*dst), val2);
    *dst += stride;
}

static INLINE void h_predictor_32x8(uint16_t* dst, ptrdiff_t stride, const uint16_t* left) {
    const __m128i left_u16 = _mm_loadu_si128((const __m128i*)left);
    const __m128i row0     = _mm_shufflelo_epi16(left_u16, 0x0);
    const __m128i row1     = _mm_shufflelo_epi16(left_u16, 0x55);
    const __m128i row2     = _mm_shufflelo_epi16(left_u16, 0xaa);
    const __m128i row3     = _mm_shufflelo_epi16(left_u16, 0xff);
    const __m128i row4     = _mm_shufflehi_epi16(left_u16, 0x0);
    const __m128i row5     = _mm_shufflehi_epi16(left_u16, 0x55);
    const __m128i row6     = _mm_shufflehi_epi16(left_u16, 0xaa);
    const __m128i row7     = _mm_shufflehi_epi16(left_u16, 0xff);
    h_store_32_unpacklo(&dst, stride, &row0);
    h_store_32_unpacklo(&dst, stride, &row1);
    h_store_32_unpacklo(&dst, stride, &row2);
    h_store_32_unpacklo(&dst, stride, &row3);
    h_store_32_unpackhi(&dst, stride, &row4);
    h_store_32_unpackhi(&dst, stride, &row5);
    h_store_32_unpackhi(&dst, stride, &row6);
    h_store_32_unpackhi(&dst, stride, &row7);
}

void aom_highbd_h_predictor_32x16_avx512(uint16_t* dst, ptrdiff_t stride, const uint16_t* above, const uint16_t* left,
                                         int32_t bd) {
    int32_t i;
    (void)above;
    (void)bd;

    for (i = 0; i < 2; i++, left += 8) {
        h_predictor_32x8(dst, stride, left);
        dst += stride << 3;
    }
}

void aom_highbd_h_predictor_32x32_avx512(uint16_t* dst, ptrdiff_t stride, const uint16_t* above, const uint16_t* left,
                                         int32_t bd) {
    int32_t i;
    (void)above;
    (void)bd;

    for (i = 0; i < 4; i++, left += 8) {
        h_predictor_32x8(dst, stride, left);
        dst += stride << 3;
    }
}

// ---------------------------------------------------------------------------- -

// 64xN

static INLINE void h_pred_64(uint16_t** const dst, const ptrdiff_t stride, const __m128i left) {
    // Broadcast the 16-bit left pixel to 256-bit register.
    const __m512i row = _mm512_broadcastw_epi16(left);

    zz_storeu_512(*dst + 0x00, row);
    zz_storeu_512(*dst + 0x20, row);

    *dst += stride;
}

// Process 8 rows.
static INLINE void h_pred_64x8(uint16_t** dst, const ptrdiff_t stride, const uint16_t* const left) {
    __m128i left_u16 = _mm_loadu_si128((const __m128i*)left);

    for (int16_t j = 0; j < 8; j++) {
        h_pred_64(dst, stride, left_u16);
        left_u16 = _mm_srli_si128(left_u16, 2);
    }
}

// 64x16

void aom_highbd_h_predictor_64x16_avx512(uint16_t* dst, ptrdiff_t stride, const uint16_t* above, const uint16_t* left,
                                         int32_t bd) {
    (void)above;
    (void)bd;

    for (int32_t i = 0; i < 2; i++, left += 8) {
        h_pred_64x8(&dst, stride, left);
    }
}

// 64x32

void aom_highbd_h_predictor_64x32_avx512(uint16_t* dst, ptrdiff_t stride, const uint16_t* above, const uint16_t* left,
                                         int32_t bd) {
    (void)above;
    (void)bd;

    for (int32_t i = 0; i < 4; i++, left += 8) {
        h_pred_64x8(&dst, stride, left);
    }
}

// 64x64

void aom_highbd_h_predictor_64x64_avx512(uint16_t* dst, ptrdiff_t stride, const uint16_t* above, const uint16_t* left,
                                         int32_t bd) {
    (void)above;
    (void)bd;

    for (int32_t i = 0; i < 8; i++, left += 8) {
        h_pred_64x8(&dst, stride, left);
    }
}

// =============================================================================

// V_PRED

// -----------------------------------------------------------------------------

// 32xN

static INLINE void v_pred_32(uint16_t** const dst, const ptrdiff_t stride, const __m512i above01) {
    _mm512_storeu_si512((__m512i*)(*dst + 0x00), above01);
    *dst += stride;
}

// Process 8 rows.
static INLINE void v_pred_32x8(uint16_t** const dst, const ptrdiff_t stride, const __m512i above01) {
    v_pred_32(dst, stride, above01);
    v_pred_32(dst, stride, above01);
    v_pred_32(dst, stride, above01);
    v_pred_32(dst, stride, above01);
    v_pred_32(dst, stride, above01);
    v_pred_32(dst, stride, above01);
    v_pred_32(dst, stride, above01);
    v_pred_32(dst, stride, above01);
}

// 32x8

void aom_highbd_v_predictor_32x8_avx512(uint16_t* dst, ptrdiff_t stride, const uint16_t* above, const uint16_t* left,
                                        int32_t bd) {
    // Load all 32 pixels in a row into 256-bit registers.
    const __m512i above01 = _mm512_loadu_si512((const __m512i*)(above + 0x00));

    (void)left;
    (void)bd;

    v_pred_32x8(&dst, stride, above01);
}

// 32x16

void aom_highbd_v_predictor_32x16_avx512(uint16_t* dst, ptrdiff_t stride, const uint16_t* above, const uint16_t* left,
                                         int32_t bd) {
    // Load all 32 pixels in a row into 512-bit registers.
    const __m512i above01 = _mm512_loadu_si512((const __m512i*)(above + 0x00));

    (void)left;
    (void)bd;

    for (int32_t i = 0; i < 2; i++) {
        v_pred_32x8(&dst, stride, above01);
    }
}

// 32x32

void aom_highbd_v_predictor_32x32_avx512(uint16_t* dst, ptrdiff_t stride, const uint16_t* above, const uint16_t* left,
                                         int32_t bd) {
    // Load all 32 pixels in a row into 512-bit registers.
    const __m512i above01 = _mm512_loadu_si512((const __m512i*)(above + 0x00));

    (void)left;
    (void)bd;

    for (int32_t i = 0; i < 4; i++) {
        v_pred_32x8(&dst, stride, above01);
    }
}

// 32x64

void aom_highbd_v_predictor_32x64_avx512(uint16_t* dst, ptrdiff_t stride, const uint16_t* above, const uint16_t* left,
                                         int32_t bd) {
    // Load all 32 pixels in a row into 512-bit registers.
    const __m512i above01 = _mm512_loadu_si512((const __m512i*)(above + 0x00));

    (void)left;
    (void)bd;

    for (int32_t i = 0; i < 8; i++) {
        v_pred_32x8(&dst, stride, above01);
    }
}

// -----------------------------------------------------------------------------

// 64xN

static INLINE void v_pred_64(uint16_t** const dst, const ptrdiff_t stride, const __m512i above0, const __m512i above1) {
    _mm512_storeu_si512((__m512i*)(*dst + 0x00), above0);
    _mm512_storeu_si512((__m512i*)(*dst + 0x20), above1);
    *dst += stride;
}

// Process 8 rows.
static INLINE void v_pred_64x8(uint16_t** const dst, const ptrdiff_t stride, const __m512i above0,
                               const __m512i above1) {
    v_pred_64(dst, stride, above0, above1);
    v_pred_64(dst, stride, above0, above1);
    v_pred_64(dst, stride, above0, above1);
    v_pred_64(dst, stride, above0, above1);
    v_pred_64(dst, stride, above0, above1);
    v_pred_64(dst, stride, above0, above1);
    v_pred_64(dst, stride, above0, above1);
    v_pred_64(dst, stride, above0, above1);
}

// 64x16

void aom_highbd_v_predictor_64x16_avx512(uint16_t* dst, ptrdiff_t stride, const uint16_t* above, const uint16_t* left,
                                         int32_t bd) {
    // Load all 64 pixels in a row into 512-bit registers.
    const __m512i above0 = _mm512_loadu_si512((const __m512i*)(above + 0x00));
    const __m512i above1 = _mm512_loadu_si512((const __m512i*)(above + 0x20));

    (void)left;
    (void)bd;

    for (int32_t i = 0; i < 2; i++) {
        v_pred_64x8(&dst, stride, above0, above1);
    }
}

// 64x32

void aom_highbd_v_predictor_64x32_avx512(uint16_t* dst, ptrdiff_t stride, const uint16_t* above, const uint16_t* left,
                                         int32_t bd) {
    // Load all 64 pixels in a row into 512-bit registers.
    const __m512i above0 = _mm512_loadu_si512((const __m512i*)(above + 0x00));
    const __m512i above1 = _mm512_loadu_si512((const __m512i*)(above + 0x20));

    (void)left;
    (void)bd;

    for (int32_t i = 0; i < 4; i++) {
        v_pred_64x8(&dst, stride, above0, above1);
    }
}

// 64x64

void aom_highbd_v_predictor_64x64_avx512(uint16_t* dst, ptrdiff_t stride, const uint16_t* above, const uint16_t* left,
                                         int32_t bd) {
    // Load all 64 pixels in a row into 512-bit registers.
    const __m512i above0 = _mm512_loadu_si512((const __m512i*)(above + 0x00));
    const __m512i above1 = _mm512_loadu_si512((const __m512i*)(above + 0x20));

    (void)left;
    (void)bd;

    for (int32_t i = 0; i < 8; i++) {
        v_pred_64x8(&dst, stride, above0, above1);
    }
}

// =============================================================================

// High bd Smooth Predictor Kernel

// -----------------------------------------------------------------------------

static INLINE void load_left_8(const uint16_t* const left, const __m512i r, __m512i* lr) {
    const __m128i l0 = _mm_loadu_si128((const __m128i*)left);
    __m512i       l  = _mm512_broadcast_i32x4(l0);
    lr[0]            = _mm512_unpacklo_epi16(l, r);
    lr[1]            = _mm512_unpackhi_epi16(l, r);
}

// -----------------------------------------------------------------------------
// 32xN

static INLINE void load_right_weights_32(const uint16_t* const above, __m512i* const r, __m512i* const weights) {
    *r = _mm512_set1_epi16((uint16_t)above[31]);

    weights[0] = _mm512_loadu_si512((const __m512i*)(sm_weights_32));
    weights[1] = _mm512_loadu_si512((const __m512i*)(sm_weights_32 + 0x20));
}

// -----------------------------------------------------------------------------
// 64xN

static INLINE void load_right_weights_64(const uint16_t* const above, __m512i* const r, __m512i* const weights) {
    *r         = _mm512_set1_epi16((uint16_t)above[63]);
    weights[0] = _mm512_loadu_si512((const __m512i*)(sm_weights_64 + 0x00));
    weights[1] = _mm512_loadu_si512((const __m512i*)(sm_weights_64 + 0x20));
    weights[2] = _mm512_loadu_si512((const __m512i*)(sm_weights_64 + 0x40));
    weights[3] = _mm512_loadu_si512((const __m512i*)(sm_weights_64 + 0x60));
}

static INLINE void prepare_ab(const uint16_t* const above, const __m512i b, __m512i* const ab) {
    const __m512i a = _mm512_loadu_si512((const __m512i*)above);
    ab[0]           = _mm512_unpacklo_epi16(a, b);
    ab[1]           = _mm512_unpackhi_epi16(a, b);
}

static INLINE void init_32(const uint16_t* const above, const uint16_t* const left, const int32_t h, __m512i* const ab,
                           __m512i* const r, __m512i* const weights_w, __m512i* const rep) {
    const __m512i b = _mm512_set1_epi16((uint16_t)left[h - 1]);
    prepare_ab(above + 0x00, b, ab + 0);
    load_right_weights_32(above, r, weights_w);

    rep[0] = _mm512_set1_epi32(0x03020100);
    rep[1] = _mm512_set1_epi32(0x07060504);
    rep[2] = _mm512_set1_epi32(0x0B0A0908);
    rep[3] = _mm512_set1_epi32(0x0F0E0D0C);
}

static INLINE __m512i smooth_pred_kernel(const __m512i* const weights_w, const __m512i weights_h, const __m512i rep,
                                         const __m512i* ab, const __m512i lr) {
    const __m512i round = _mm512_set1_epi32((1 << sm_weight_log2_scale));
    __m512i       s[2], sum[2];
    __m512i       w = _mm512_shuffle_epi8(weights_h, rep);
    __m512i       t = _mm512_shuffle_epi8(lr, rep);
    s[0]            = _mm512_madd_epi16(ab[0], w);
    s[1]            = _mm512_madd_epi16(ab[1], w);
    sum[0]          = _mm512_madd_epi16(t, weights_w[0]);
    sum[1]          = _mm512_madd_epi16(t, weights_w[1]);
    sum[0]          = _mm512_add_epi32(sum[0], s[0]);
    sum[1]          = _mm512_add_epi32(sum[1], s[1]);
    sum[0]          = _mm512_add_epi32(sum[0], round);
    sum[1]          = _mm512_add_epi32(sum[1], round);
    sum[0]          = _mm512_srai_epi32(sum[0], 1 + sm_weight_log2_scale);
    sum[1]          = _mm512_srai_epi32(sum[1], 1 + sm_weight_log2_scale);
    return _mm512_packs_epi32(sum[0], sum[1]);
}

static INLINE void smooth_pred_32(const __m512i* const weights_w, const __m512i weights_h, const __m512i rep,
                                  const __m512i* const ab, const __m512i lr, uint16_t** const dst,
                                  const ptrdiff_t stride) {
    __m512i d;

    d = smooth_pred_kernel(weights_w + 0, weights_h, rep, ab, lr);
    _mm512_storeu_si512((__m512i*)(*dst + 0x00), d);

    *dst += stride;
}

static INLINE void smooth_pred_32x4(const __m512i* const weights_w, const uint16_t* const sm_weights_h,
                                    const __m512i* const rep, const __m512i* const ab, const __m512i lr,
                                    uint16_t** const dst, const ptrdiff_t stride) {
    const __m512i weights_h = _mm512_loadu_si512((const __m512i*)sm_weights_h);
    smooth_pred_32(weights_w, weights_h, rep[0], ab, lr, dst, stride);
    smooth_pred_32(weights_w, weights_h, rep[1], ab, lr, dst, stride);
    smooth_pred_32(weights_w, weights_h, rep[2], ab, lr, dst, stride);
    smooth_pred_32(weights_w, weights_h, rep[3], ab, lr, dst, stride);
}

static INLINE void smooth_pred_32x8(const uint16_t* const left, const __m512i* const weights_w,
                                    const uint16_t* const sm_weights_h, const __m512i* const rep,
                                    const __m512i* const ab, const __m512i r, uint16_t** const dst,
                                    const ptrdiff_t stride) {
    __m512i lr[2];
    load_left_8(left, r, lr);

    smooth_pred_32x4(weights_w, sm_weights_h + 0, rep, ab, lr[0], dst, stride);
    smooth_pred_32x4(weights_w, sm_weights_h + 32, rep, ab, lr[1], dst, stride);
}

// 32x8

void aom_highbd_smooth_predictor_32x8_avx512(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                             const uint16_t* left, int32_t bd) {
    __m512i ab[2], r, weights_w[2], rep[4];
    (void)bd;

    init_32(above, left, 8, ab, &r, weights_w, rep);

    smooth_pred_32x8(left, weights_w, sm_weights_d_8, rep, ab, r, &dst, stride);
}

// 32x16

void aom_highbd_smooth_predictor_32x16_avx512(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                              const uint16_t* left, int32_t bd) {
    __m512i ab[2], r, weights_w[2], rep[4];
    (void)bd;

    init_32(above, left, 16, ab, &r, weights_w, rep);

    for (int32_t i = 0; i < 2; i++) {
        smooth_pred_32x8(left + 8 * i, weights_w, sm_weights_d_16 + 64 * i, rep, ab, r, &dst, stride);
    }
}

//32x32
void aom_highbd_smooth_predictor_32x32_avx512(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                              const uint16_t* left, int32_t bd) {
    __m512i ab[2], r, weights_w[2], rep[4];
    (void)bd;

    init_32(above, left, 32, ab, &r, weights_w, rep);

    for (int32_t i = 0; i < 4; i++) {
        smooth_pred_32x8(left + 8 * i, weights_w, sm_weights_d_32 + 64 * i, rep, ab, r, &dst, stride);
    }
}

// 32x64

void aom_highbd_smooth_predictor_32x64_avx512(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                              const uint16_t* left, int32_t bd) {
    __m512i ab[2], r, weights_w[2], rep[4];
    (void)bd;

    init_32(above, left, 64, ab, &r, weights_w, rep);

    for (int32_t i = 0; i < 8; i++) {
        smooth_pred_32x8(left + 8 * i, weights_w, sm_weights_d_64 + 64 * i, rep, ab, r, &dst, stride);
    }
}

static INLINE void init_64(const uint16_t* const above, const uint16_t* const left, const int32_t h, __m512i* const ab,
                           __m512i* r, __m512i* const weights_w, __m512i* const rep) {
    const __m512i b = _mm512_set1_epi16((uint16_t)left[h - 1]);
    prepare_ab(above + 0x00, b, ab + 0);
    prepare_ab(above + 0x20, b, ab + 2);
    load_right_weights_64(above, r, weights_w);

    rep[0] = _mm512_set1_epi32(0x03020100);
    rep[1] = _mm512_set1_epi32(0x07060504);
    rep[2] = _mm512_set1_epi32(0x0B0A0908);
    rep[3] = _mm512_set1_epi32(0x0F0E0D0C);
}

static INLINE void smooth_pred_64(const __m512i* const weights_w, const __m512i weights_h, const __m512i rep,
                                  const __m512i* const ab, const __m512i lr, uint16_t** const dst,
                                  const ptrdiff_t stride) {
    __m512i d;

    d = smooth_pred_kernel(weights_w + 0, weights_h, rep, ab + 0, lr);
    _mm512_storeu_si512((__m512i*)(*dst + 0x00), d);

    d = smooth_pred_kernel(weights_w + 2, weights_h, rep, ab + 2, lr);
    _mm512_storeu_si512((__m512i*)(*dst + 0x20), d);
    *dst += stride;
}

static INLINE void smooth_pred_64x4(const __m512i* const weights_w, const uint16_t* const sm_weights_h,
                                    const __m512i* const rep, const __m512i* const ab, const __m512i lr,
                                    uint16_t** const dst, const ptrdiff_t stride) {
    const __m512i weights_h = _mm512_loadu_si512((const __m512i*)sm_weights_h);
    smooth_pred_64(weights_w, weights_h, rep[0], ab, lr, dst, stride);
    smooth_pred_64(weights_w, weights_h, rep[1], ab, lr, dst, stride);
    smooth_pred_64(weights_w, weights_h, rep[2], ab, lr, dst, stride);
    smooth_pred_64(weights_w, weights_h, rep[3], ab, lr, dst, stride);
}

static INLINE void smooth_pred_64x8(const uint16_t* const left, const __m512i* const weights_w,
                                    const uint16_t* const sm_weights_h, const __m512i* const rep,
                                    const __m512i* const ab, const __m512i r, uint16_t** const dst,
                                    const ptrdiff_t stride) {
    __m512i lr[2];
    load_left_8(left, r, lr);

    smooth_pred_64x4(weights_w, sm_weights_h + 0, rep, ab, lr[0], dst, stride);
    smooth_pred_64x4(weights_w, sm_weights_h + 32, rep, ab, lr[1], dst, stride);
}

// 64x16

void aom_highbd_smooth_predictor_64x16_avx512(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                              const uint16_t* left, int32_t bd) {
    __m512i ab[4], r, weights_w[4], rep[4];
    (void)bd;

    init_64(above, left, 16, ab, &r, weights_w, rep);

    for (int32_t i = 0; i < 2; i++) {
        smooth_pred_64x8(left + 8 * i, weights_w, sm_weights_d_16 + 64 * i, rep, ab, r, &dst, stride);
    }
}

// 64x32

void aom_highbd_smooth_predictor_64x32_avx512(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                              const uint16_t* left, int32_t bd) {
    __m512i ab[4], r, weights_w[4], rep[4];
    (void)bd;

    init_64(above, left, 32, ab, &r, weights_w, rep);

    for (int32_t i = 0; i < 4; i++) {
        smooth_pred_64x8(left + 8 * i, weights_w, sm_weights_d_32 + 64 * i, rep, ab, r, &dst, stride);
    }
}

// 64x64

void aom_highbd_smooth_predictor_64x64_avx512(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                              const uint16_t* left, int32_t bd) {
    __m512i ab[4], r, weights_w[4], rep[4];
    (void)bd;

    init_64(above, left, 64, ab, &r, weights_w, rep);

    for (int32_t i = 0; i < 8; i++) {
        smooth_pred_64x8(left + 8 * i, weights_w, sm_weights_d_64 + 64 * i, rep, ab, r, &dst, stride);
    }
}

// =============================================================================

// High bd Smooth V Predictor Kernel

// -----------------------------------------------------------------------------

static INLINE __m512i smooth_v_pred_kernel(const __m512i weights, const __m512i rep, const __m512i* const ab) {
    const __m512i round = _mm512_set1_epi32((1 << (sm_weight_log2_scale - 1)));
    __m512i       sum[2];
    const __m512i w = _mm512_shuffle_epi8(weights, rep);
    sum[0]          = _mm512_madd_epi16(ab[0], w);
    sum[1]          = _mm512_madd_epi16(ab[1], w);
    sum[0]          = _mm512_add_epi32(sum[0], round);
    sum[1]          = _mm512_add_epi32(sum[1], round);
    sum[0]          = _mm512_srai_epi32(sum[0], sm_weight_log2_scale);
    sum[1]          = _mm512_srai_epi32(sum[1], sm_weight_log2_scale);
    return _mm512_packs_epi32(sum[0], sum[1]);
}

static INLINE void smooth_v_pred_32(const __m512i weights, const __m512i rep, const __m512i* const ab,
                                    uint16_t** const dst, const ptrdiff_t stride) {
    __m512i d;

    d = smooth_v_pred_kernel(weights, rep, ab + 0);
    _mm512_storeu_si512((__m512i*)(*dst + 0x00), d);
    *dst += stride;
}

static INLINE void smooth_v_pred_32x4(const uint16_t* const sm_weights_h, const __m512i* const rep,
                                      const __m512i* const ab, uint16_t** const dst, const ptrdiff_t stride) {
    const __m512i weights = _mm512_loadu_si512((const __m512i*)sm_weights_h);
    smooth_v_pred_32(weights, rep[0], ab, dst, stride);
    smooth_v_pred_32(weights, rep[1], ab, dst, stride);
    smooth_v_pred_32(weights, rep[2], ab, dst, stride);
    smooth_v_pred_32(weights, rep[3], ab, dst, stride);
}

// 32x8

static INLINE void smooth_v_pred_32x8(const uint16_t* const sm_weights_h, const __m512i* const rep,
                                      const __m512i* const ab, uint16_t** const dst, const ptrdiff_t stride) {
    smooth_v_pred_32x4(sm_weights_h + 0, rep, ab, dst, stride);
    smooth_v_pred_32x4(sm_weights_h + 32, rep, ab, dst, stride);
}

static INLINE void smooth_v_init_32(const uint16_t* const above, const uint16_t* const left, const int32_t h,
                                    __m512i* const ab, __m512i* const rep) {
    const __m512i b = _mm512_set1_epi16((uint16_t)left[h - 1]);
    prepare_ab(above + 0x00, b, ab + 0);

    rep[0] = _mm512_set1_epi32(0x03020100);
    rep[1] = _mm512_set1_epi32(0x07060504);
    rep[2] = _mm512_set1_epi32(0x0B0A0908);
    rep[3] = _mm512_set1_epi32(0x0F0E0D0C);
}

void aom_highbd_smooth_v_predictor_32x8_avx512(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                               const uint16_t* left, int32_t bd) {
    __m512i ab[2], rep[4];
    (void)bd;

    smooth_v_init_32(above, left, 8, ab, rep);

    smooth_v_pred_32x8(sm_weights_d_8, rep, ab, &dst, stride);
}

// 32x16

void aom_highbd_smooth_v_predictor_32x16_avx512(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                const uint16_t* left, int32_t bd) {
    __m512i ab[2], rep[4];
    (void)bd;

    smooth_v_init_32(above, left, 16, ab, rep);

    for (int32_t i = 0; i < 2; i++) {
        smooth_v_pred_32x8(sm_weights_d_16 + 64 * i, rep, ab, &dst, stride);
    }
}

// 32x32

void aom_highbd_smooth_v_predictor_32x32_avx512(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                const uint16_t* left, int32_t bd) {
    __m512i ab[2], rep[4];
    (void)bd;

    smooth_v_init_32(above, left, 32, ab, rep);

    for (int32_t i = 0; i < 4; i++) {
        smooth_v_pred_32x8(sm_weights_d_32 + 64 * i, rep, ab, &dst, stride);
    }
}

// 32x64

void aom_highbd_smooth_v_predictor_32x64_avx512(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                const uint16_t* left, int32_t bd) {
    __m512i ab[2], rep[4];
    (void)bd;

    smooth_v_init_32(above, left, 64, ab, rep);

    for (int32_t i = 0; i < 8; i++) {
        smooth_v_pred_32x8(sm_weights_d_64 + 64 * i, rep, ab, &dst, stride);
    }
}

// -----------------------------------------------------------------------------
// 64xN

static INLINE void smooth_v_init_64(const uint16_t* const above, const uint16_t* const left, const int32_t h,
                                    __m512i* const ab, __m512i* const rep) {
    const __m512i b = _mm512_set1_epi16((uint16_t)left[h - 1]);
    prepare_ab(above + 0x00, b, ab + 0);
    prepare_ab(above + 0x20, b, ab + 2);

    rep[0] = _mm512_set1_epi32(0x03020100);
    rep[1] = _mm512_set1_epi32(0x07060504);
    rep[2] = _mm512_set1_epi32(0x0B0A0908);
    rep[3] = _mm512_set1_epi32(0x0F0E0D0C);
}

static INLINE void smooth_v_pred_64(const __m512i weights, const __m512i rep, const __m512i* const ab,
                                    uint16_t** const dst, const ptrdiff_t stride) {
    __m512i d;

    d = smooth_v_pred_kernel(weights, rep, ab + 0);
    _mm512_storeu_si512((__m512i*)(*dst + 0x00), d);

    d = smooth_v_pred_kernel(weights, rep, ab + 2);
    _mm512_storeu_si512((__m512i*)(*dst + 0x20), d);

    *dst += stride;
}

static INLINE void smooth_v_pred_64x4(const uint16_t* const sm_weights_h, const __m512i* const rep,
                                      const __m512i* const ab, uint16_t** const dst, const ptrdiff_t stride) {
    const __m512i weights = _mm512_loadu_si512((const __m512i*)sm_weights_h);
    smooth_v_pred_64(weights, rep[0], ab, dst, stride);
    smooth_v_pred_64(weights, rep[1], ab, dst, stride);
    smooth_v_pred_64(weights, rep[2], ab, dst, stride);
    smooth_v_pred_64(weights, rep[3], ab, dst, stride);
}

static INLINE void smooth_v_pred_64x8(const uint16_t* const sm_weights_h, const __m512i* const rep,
                                      const __m512i* const ab, uint16_t** const dst, const ptrdiff_t stride) {
    smooth_v_pred_64x4(sm_weights_h + 0, rep, ab, dst, stride);
    smooth_v_pred_64x4(sm_weights_h + 32, rep, ab, dst, stride);
}

// 64x16

void aom_highbd_smooth_v_predictor_64x16_avx512(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                const uint16_t* left, int32_t bd) {
    __m512i ab[4], rep[4];
    (void)bd;

    smooth_v_init_64(above, left, 16, ab, rep);

    for (int32_t i = 0; i < 2; i++) {
        smooth_v_pred_64x8(sm_weights_d_16 + 64 * i, rep, ab, &dst, stride);
    }
}

// 64x32

void aom_highbd_smooth_v_predictor_64x32_avx512(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                const uint16_t* left, int32_t bd) {
    __m512i ab[4], rep[4];
    (void)bd;

    smooth_v_init_64(above, left, 32, ab, rep);

    for (int32_t i = 0; i < 4; i++) {
        smooth_v_pred_64x8(sm_weights_d_32 + 64 * i, rep, ab, &dst, stride);
    }
}

// 64x64

void aom_highbd_smooth_v_predictor_64x64_avx512(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                const uint16_t* left, int32_t bd) {
    __m512i ab[4], rep[4];
    (void)bd;

    smooth_v_init_64(above, left, 64, ab, rep);

    for (int32_t i = 0; i < 8; i++) {
        smooth_v_pred_64x8(sm_weights_d_64 + 64 * i, rep, ab, &dst, stride);
    }
}

// =============================================================================

// SMOOTH_H_PRED

// -----------------------------------------------------------------------------
static INLINE __m512i smooth_h_pred_kernel(const __m512i* const weights, const __m512i lr) {
    const __m512i round = _mm512_set1_epi32((1 << (sm_weight_log2_scale - 1)));
    __m512i       sum[2];
    sum[0] = _mm512_madd_epi16(lr, weights[0]);
    sum[1] = _mm512_madd_epi16(lr, weights[1]);
    sum[0] = _mm512_add_epi32(sum[0], round);
    sum[1] = _mm512_add_epi32(sum[1], round);
    sum[0] = _mm512_srai_epi32(sum[0], sm_weight_log2_scale);
    sum[1] = _mm512_srai_epi32(sum[1], sm_weight_log2_scale);
    return _mm512_packs_epi32(sum[0], sum[1]);
}

// -----------------------------------------------------------------------------
// 32xN

static INLINE void smooth_h_pred_32(const __m512i* const weights, __m512i* const lr, uint16_t** const dst,
                                    const ptrdiff_t stride) {
    const __m512i rep = _mm512_set1_epi32(0x03020100);
    const __m512i t   = _mm512_shuffle_epi8(*lr, rep);
    __m512i       d;

    d = smooth_h_pred_kernel(weights + 0, t);
    _mm512_storeu_si512((__m512i*)(*dst + 0x00), d);
    *dst += stride;
    *lr = _mm512_bsrli_epi128(*lr, FOUR);
}

static INLINE void smooth_h_pred_32x4(const __m512i* const weights, __m512i* const lr, uint16_t** const dst,
                                      const ptrdiff_t stride) {
    smooth_h_pred_32(weights, lr, dst, stride);
    smooth_h_pred_32(weights, lr, dst, stride);
    smooth_h_pred_32(weights, lr, dst, stride);
    smooth_h_pred_32(weights, lr, dst, stride);
}

static INLINE void smooth_h_pred_32x8(uint16_t* dst, const ptrdiff_t stride, const uint16_t* const above,
                                      const uint16_t* left, const int32_t n) {
    __m512i lr[2], weights[2];
    __m512i r;

    load_right_weights_32(above, &r, weights);

    for (int32_t i = 0; i < n; i++) {
        load_left_8(left, r, lr);
        smooth_h_pred_32x4(weights, ((__m512i*)&lr + 0), &dst, stride);
        smooth_h_pred_32x4(weights, ((__m512i*)&lr + 1), &dst, stride);
        left += 8;
    }
}

// 32x8

void aom_highbd_smooth_h_predictor_32x8_avx512(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                               const uint16_t* left, int32_t bd) {
    (void)bd;
    smooth_h_pred_32x8(dst, stride, above, left, 1);
}

// 32x16

void aom_highbd_smooth_h_predictor_32x16_avx512(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                const uint16_t* left, int32_t bd) {
    (void)bd;
    smooth_h_pred_32x8(dst, stride, above, left, 2);
}

// 32x32

void aom_highbd_smooth_h_predictor_32x32_avx512(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                const uint16_t* left, int32_t bd) {
    (void)bd;
    smooth_h_pred_32x8(dst, stride, above, left, 4);
}

// 32x64

void aom_highbd_smooth_h_predictor_32x64_avx512(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                const uint16_t* left, int32_t bd) {
    (void)bd;
    smooth_h_pred_32x8(dst, stride, above, left, 8);
}

static INLINE void smooth_h_pred_64(const __m512i* const weights, __m512i* const lr, uint16_t** const dst,
                                    const ptrdiff_t stride) {
    const __m512i rep = _mm512_set1_epi32(0x03020100);
    const __m512i t   = _mm512_shuffle_epi8(*lr, rep);
    __m512i       d0, d1;

    *((__m512i*)&d0 + 0) = smooth_h_pred_kernel(weights + 0, t);
    _mm512_storeu_si512((__m512i*)(*dst + 0x00), d0);

    *((__m512i*)&d1 + 0) = smooth_h_pred_kernel(weights + 2, t);
    _mm512_storeu_si512((__m512i*)(*dst + 0x20), d1);

    *dst += stride;
    *lr = _mm512_bsrli_epi128(*lr, FOUR);
}

static INLINE void smooth_h_pred_64x4(const __m512i* const weights, __m512i* const lr, uint16_t** const dst,
                                      const ptrdiff_t stride) {
    smooth_h_pred_64(weights, lr, dst, stride);
    smooth_h_pred_64(weights, lr, dst, stride);
    smooth_h_pred_64(weights, lr, dst, stride);
    smooth_h_pred_64(weights, lr, dst, stride);
}

static INLINE void smooth_h_pred_64x8(uint16_t* dst, const ptrdiff_t stride, const uint16_t* const above,
                                      const uint16_t* left, const int32_t n) {
    __m512i r;
    __m512i lr[2], weights[4];

    load_right_weights_64(above, &r, weights);

    for (int32_t i = 0; i < n; i++) {
        load_left_8(left, r, lr);
        smooth_h_pred_64x4(weights, (__m512i*)&lr + 0, &dst, stride);
        smooth_h_pred_64x4(weights, (__m512i*)&lr + 1, &dst, stride);
        left += 8;
    }
}

// 64x16

void aom_highbd_smooth_h_predictor_64x16_avx512(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                const uint16_t* left, int32_t bd) {
    (void)bd;
    smooth_h_pred_64x8(dst, stride, above, left, 2);
}

// 64x32

void aom_highbd_smooth_h_predictor_64x32_avx512(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                const uint16_t* left, int32_t bd) {
    (void)bd;
    smooth_h_pred_64x8(dst, stride, above, left, 4);
}

// 64x64

void aom_highbd_smooth_h_predictor_64x64_avx512(uint16_t* dst, ptrdiff_t stride, const uint16_t* above,
                                                const uint16_t* left, int32_t bd) {
    (void)bd;
    smooth_h_pred_64x8(dst, stride, above, left, 8);
}
#endif
