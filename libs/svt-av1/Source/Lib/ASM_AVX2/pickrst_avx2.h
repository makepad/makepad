/*
* Copyright (c) 2019, Alliance for Open Media. All rights reserved
*
* This source code is subject to the terms of the BSD 2 Clause License and
* the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
* was not distributed with this source code in the LICENSE file, you can
* obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
* Media Patent License 1.0 was not distributed with this source code in the
* PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
*/

#ifndef AOM_DSP_X86_PICKRST_AVX2_H_
#define AOM_DSP_X86_PICKRST_AVX2_H_

#include <immintrin.h> // AVX2
#include "aom_dsp_rtcd.h"
#include "restoration.h"
#include "synonyms_avx2.h"

EB_ALIGN(16)
static const uint8_t mask_8bit[16][16] = {
    {0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0},
    {0xFF, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0},
    {0xFF, 0xFF, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0},
    {0xFF, 0xFF, 0xFF, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0},
    {0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0},
    {0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0},
    {0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0},
    {0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0, 0, 0, 0, 0, 0, 0},
    {0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0, 0, 0, 0, 0, 0},
    {0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0, 0, 0, 0, 0},
    {0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0, 0, 0, 0},
    {0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0, 0, 0},
    {0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0, 0},
    {0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0},
    {0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0, 0},
    {0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0}};

EB_ALIGN(32)
static const uint16_t mask_16bit[16][16] = {
    {0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0},
    {0xFFFF, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0},
    {0xFFFF, 0xFFFF, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0},
    {0xFFFF, 0xFFFF, 0xFFFF, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0},
    {0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0},
    {0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0},
    {0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0},
    {0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0, 0, 0, 0, 0, 0, 0, 0, 0},
    {0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0, 0, 0, 0, 0, 0, 0, 0},
    {0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0, 0, 0, 0, 0, 0, 0},
    {0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0, 0, 0, 0, 0, 0},
    {0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0, 0, 0, 0, 0},
    {0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0, 0, 0, 0},
    {0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0, 0, 0},
    {0xFFFF,
     0xFFFF,
     0xFFFF,
     0xFFFF,
     0xFFFF,
     0xFFFF,
     0xFFFF,
     0xFFFF,
     0xFFFF,
     0xFFFF,
     0xFFFF,
     0xFFFF,
     0xFFFF,
     0xFFFF,
     0,
     0},
    {0xFFFF,
     0xFFFF,
     0xFFFF,
     0xFFFF,
     0xFFFF,
     0xFFFF,
     0xFFFF,
     0xFFFF,
     0xFFFF,
     0xFFFF,
     0xFFFF,
     0xFFFF,
     0xFFFF,
     0xFFFF,
     0xFFFF,
     0}};

static INLINE void add_six_32_to_64_avx2(const __m256i src, __m256i* const sum, __m128i* const sum128) {
    const __m256i s0 = _mm256_cvtepi32_epi64(_mm256_castsi256_si128(src));
    const __m128i s1 = _mm_cvtepi32_epi64(_mm256_extracti128_si256(src, 1));
    *sum             = _mm256_add_epi64(*sum, s0);
    *sum128          = _mm_add_epi64(*sum128, s1);
}

static INLINE __m128i add_hi_lo_64_avx2(const __m256i src) {
    const __m128i s0 = _mm256_castsi256_si128(src);
    const __m128i s1 = _mm256_extracti128_si256(src, 1);
    return _mm_add_epi64(s0, s1);
}

static INLINE __m128i sub_hi_lo_32_avx2(const __m256i src) {
    const __m128i s0 = _mm256_castsi256_si128(src);
    const __m128i s1 = _mm256_extracti128_si256(src, 1);
    return _mm_sub_epi32(s1, s0);
}

static INLINE __m256i hadd_32x8_to_64x4_avx2(const __m256i src) {
    const __m256i s0 = _mm256_cvtepi32_epi64(_mm256_castsi256_si128(src));
    const __m256i s1 = _mm256_cvtepi32_epi64(_mm256_extracti128_si256(src, 1));
    return _mm256_add_epi64(s0, s1);
}

static INLINE __m256i hsub_32x8_to_64x4_avx2(const __m256i src) {
    const __m256i s0 = _mm256_cvtepi32_epi64(_mm256_castsi256_si128(src));
    const __m256i s1 = _mm256_cvtepi32_epi64(_mm256_extracti128_si256(src, 1));
    return _mm256_sub_epi64(s1, s0);
}

static INLINE __m128i hadd_64_avx2(const __m256i src) {
    const __m256i t0   = _mm256_srli_si256(src, 8);
    const __m256i sum  = _mm256_add_epi64(src, t0);
    const __m128i sum0 = _mm256_castsi256_si128(sum); // 00+01 10+11
    const __m128i sum1 = _mm256_extracti128_si256(sum, 1); // 02+03 12+13
    return _mm_add_epi64(sum0, sum1); // 00+01+02+03 10+11+12+13
}

static INLINE __m128i hadd_two_64_avx2(const __m256i src0, const __m256i src1) {
    const __m256i t0   = _mm256_unpacklo_epi64(src0, src1); // 00 10  02 12
    const __m256i t1   = _mm256_unpackhi_epi64(src0, src1); // 01 11  03 13
    const __m256i sum  = _mm256_add_epi64(t0, t1); // 00+01 10+11  02+03 12+13
    const __m128i sum0 = _mm256_castsi256_si128(sum); // 00+01 10+11
    const __m128i sum1 = _mm256_extracti128_si256(sum, 1); // 02+03 12+13
    return _mm_add_epi64(sum0, sum1); // 00+01+02+03 10+11+12+13
}

static INLINE __m128i hadd_two_32_to_64_avx2(const __m256i src0, const __m256i src1) {
    const __m256i s0 = hadd_32x8_to_64x4_avx2(src0); // 00 01  02 03
    const __m256i s1 = hadd_32x8_to_64x4_avx2(src1); // 10 11  12 13
    return hadd_two_64_avx2(s0, s1);
}

static INLINE __m128i hadd_two_32_avx2(const __m256i src0, const __m256i src1) {
    const __m256i s01  = _mm256_hadd_epi32(src0, src1); // 0 0 1 1  0 0 1 1
    const __m128i sum0 = _mm256_castsi256_si128(s01); // 0 0 1 1
    const __m128i sum1 = _mm256_extracti128_si256(s01, 1); // 0 0 1 1
    const __m128i sum  = _mm_add_epi32(sum0, sum1); // 0 0 1 1
    return _mm_hadd_epi32(sum, sum); // 0 1 0 1
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

static INLINE __m256i hadd_four_64_avx2(const __m256i src0, const __m256i src1, const __m256i src2,
                                        const __m256i src3) {
    __m256i s[2], t[4];

    // 00 01  02 03
    // 10 11  12 13
    // 20 21  22 23
    // 30 31  32 33

    t[0] = _mm256_unpacklo_epi64(src0, src1); // 00 10  02 12
    t[1] = _mm256_unpackhi_epi64(src0, src1); // 01 11  03 13
    t[2] = _mm256_unpacklo_epi64(src2, src3); // 20 30  22 32
    t[3] = _mm256_unpackhi_epi64(src2, src3); // 21 31  23 33

    s[0] = _mm256_add_epi64(t[0], t[1]); // 00+01 10+11  02+03 12+13
    s[1] = _mm256_add_epi64(t[2], t[3]); // 20+21 30+31  22+23 32+33

    // 00+01 10+11  20+21 30+31
    t[0] = yy_unpacklo_epi128(s[0], s[1]);
    // 02+03 12+13  22+23 32+33
    t[1] = yy_unpackhi_epi128(s[0], s[1]);

    // 00+01+02+03 10+11+12+13  20+21+22+23 30+31+32+33
    return _mm256_add_epi64(t[0], t[1]);
}

// inputs' value range is 31-bit
static INLINE __m128i hadd_two_31_to_64_avx2(const __m256i src0, const __m256i src1) {
    __m256i s;
    s = _mm256_hadd_epi32(src0, src1); // 0 0 1 1  0 0 1 1
    s = hadd_32x8_to_64x4_avx2(s); // 0 0 1 1
    s = _mm256_permute4x64_epi64(s, 0xD8); // 0 1 0 1

    return add_hi_lo_64_avx2(s);
}

static INLINE __m256i hadd_x_64_avx2(const __m256i src01, const __m256i src23) {
    // 0 0 1 1
    // 2 2 3 3
    const __m256i t0 = _mm256_unpacklo_epi64(src01, src23); // 0 2 1 3
    const __m256i t1 = _mm256_unpackhi_epi64(src01, src23); // 0 2 1 3
    const __m256i t  = _mm256_add_epi64(t0, t1); // 0 2 1 3

    return _mm256_permute4x64_epi64(t, 0xD8); // 0 1 2 3
}

// inputs' value range is 31-bit
static INLINE __m256i hadd_four_31_to_64_avx2(const __m256i src0, const __m256i src1, const __m256i src2,
                                              const __m256i src3) {
    __m256i s[2];
    s[0] = _mm256_hadd_epi32(src0, src1); // 0 0 1 1  0 0 1 1
    s[1] = _mm256_hadd_epi32(src2, src3); // 2 2 3 3  2 2 3 3
    s[0] = hadd_32x8_to_64x4_avx2(s[0]); // 0 0 1 1
    s[1] = hadd_32x8_to_64x4_avx2(s[1]); // 2 2 3 3

    return hadd_x_64_avx2(s[0], s[1]);
}

static INLINE __m256i hadd_four_32_to_64_avx2(const __m256i src0, const __m256i src1, const __m256i src2,
                                              const __m256i src3) {
    __m256i s[4];

    s[0] = hadd_32x8_to_64x4_avx2(src0); // 00 01  02 03
    s[1] = hadd_32x8_to_64x4_avx2(src1); // 10 11  12 13
    s[2] = hadd_32x8_to_64x4_avx2(src2); // 20 21  22 23
    s[3] = hadd_32x8_to_64x4_avx2(src3); // 30 31  32 33

    return hadd_four_64_avx2(s[0], s[1], s[2], s[3]);
}

static INLINE void madd_sse2(const __m128i src, const __m128i dgd, __m128i* sum) {
    const __m128i sd = _mm_madd_epi16(src, dgd);
    *sum             = _mm_add_epi32(*sum, sd);
}

static INLINE void madd_avx2(const __m256i src, const __m256i dgd, __m256i* sum) {
    const __m256i sd = _mm256_madd_epi16(src, dgd);
    *sum             = _mm256_add_epi32(*sum, sd);
}

static INLINE void msub_avx2(const __m256i src, const __m256i dgd, __m256i* sum) {
    const __m256i sd = _mm256_madd_epi16(src, dgd);
    *sum             = _mm256_sub_epi32(*sum, sd);
}

static INLINE void update_2_stats_sse2(const int64_t* const src, const __m128i delta, int64_t* const dst) {
    const __m128i s = _mm_loadu_si128((__m128i*)src);
    const __m128i d = _mm_add_epi64(s, delta);
    _mm_storeu_si128((__m128i*)dst, d);
}

static INLINE void update_4_stats_avx2(const int64_t* const src, const __m128i delta, int64_t* const dst) {
    const __m256i s   = _mm256_loadu_si256((__m256i*)src);
    const __m256i dlt = _mm256_cvtepi32_epi64(delta);
    const __m256i d   = _mm256_add_epi64(s, dlt);
    _mm256_storeu_si256((__m256i*)dst, d);
}

static INLINE void update_4_stats_highbd_avx2(const int64_t* const src, const __m256i delta, int64_t* const dst) {
    const __m256i s = _mm256_loadu_si256((__m256i*)src);
    const __m256i d = _mm256_add_epi64(s, delta);
    _mm256_storeu_si256((__m256i*)dst, d);
}

static INLINE void update_5_stats_avx2(const int64_t* const src, const __m128i delta, const int64_t delta4,
                                       int64_t* const dst) {
    update_4_stats_avx2(src + 0, delta, dst + 0);
    dst[4] = src[4] + delta4;
}

static INLINE void update_5_stats_highbd_avx2(const int64_t* const src, const __m256i delta, const int64_t delta4,
                                              int64_t* const dst) {
    update_4_stats_highbd_avx2(src + 0, delta, dst + 0);
    dst[4] = src[4] + delta4;
}

static INLINE void update_8_stats_avx2(const int64_t* const src, const __m256i delta, int64_t* const dst) {
    update_4_stats_avx2(src + 0, _mm256_castsi256_si128(delta), dst + 0);
    update_4_stats_avx2(src + 4, _mm256_extracti128_si256(delta, 1), dst + 4);
}

static INLINE void hadd_update_4_stats_avx2(const int64_t* const src, const __m256i deltas[4], int64_t* const dst) {
    const __m128i delta = hadd_four_32_avx2(deltas[0], deltas[1], deltas[2], deltas[3]);
    update_4_stats_avx2(src, delta, dst);
}

static INLINE void hadd_update_4_stats_highbd_avx2(const int64_t* const src, const __m256i deltas[4],
                                                   int64_t* const dst) {
    const __m256i delta = hadd_four_31_to_64_avx2(deltas[0], deltas[1], deltas[2], deltas[3]);
    update_4_stats_highbd_avx2(src, delta, dst);
}

static INLINE void hadd_update_6_stats_avx2(const int64_t* const src, const __m256i deltas[6], int64_t* const dst) {
    const __m128i delta0123 = hadd_four_32_avx2(deltas[0], deltas[1], deltas[2], deltas[3]);
    const __m128i delta45   = hadd_two_32_avx2(deltas[4], deltas[5]);
    const __m128i delta45_t = _mm_cvtepi32_epi64(delta45);
    update_4_stats_avx2(src + 0, delta0123, dst + 0);
    update_2_stats_sse2(src + 4, delta45_t, dst + 4);
}

static INLINE void hadd_update_6_stats_highbd_avx2(const int64_t* const src, const __m256i deltas[6],
                                                   int64_t* const dst) {
    const __m256i delta0123 = hadd_four_31_to_64_avx2(deltas[0], deltas[1], deltas[2], deltas[3]);
    const __m128i delta45   = hadd_two_31_to_64_avx2(deltas[4], deltas[5]);
    update_4_stats_highbd_avx2(src + 0, delta0123, dst + 0);
    update_2_stats_sse2(src + 4, delta45, dst + 4);
}

static INLINE void load_more_16_avx2(const int16_t* const src, const int32_t width, const __m256i org,
                                     __m256i* const dst) {
    *dst = _mm256_srli_si256(org, 2);
    *dst = _mm256_insert_epi16(*dst, *(int32_t*)src, 7);
    *dst = _mm256_insert_epi16(*dst, *(int32_t*)(src + width), 15);
}

static INLINE void load_more_32_avx2(const int16_t* const src, const int32_t width, __m256i* const dst) {
    *dst = _mm256_srli_si256(*dst, 4);
    *dst = _mm256_insert_epi32(*dst, *(int32_t*)src, 3);
    *dst = _mm256_insert_epi32(*dst, *(int32_t*)(src + width), 7);
}

static INLINE void load_more_64_avx2(const int16_t* const src, const int32_t width, __m256i* const dst) {
    *dst = _mm256_srli_si256(*dst, 8);
    *dst = _mm256_insert_epi64(*dst, *(int64_t*)src, 1);
    *dst = _mm256_insert_epi64(*dst, *(int64_t*)(src + width), 3);
}

static INLINE __m256i load_win7_avx2(const int16_t* const d, const int32_t width) {
    // 16-bit idx: 0, 4, 1, 5, 2, 6, 3, 7
    const __m256i shf = _mm256_setr_epi8(
        0, 1, 8, 9, 2, 3, 10, 11, 4, 5, 12, 13, 6, 7, 14, 15, 0, 1, 8, 9, 2, 3, 10, 11, 4, 5, 12, 13, 6, 7, 14, 15);
    // 00s 01s 02s 03s 04s 05s 06s 07s
    const __m128i ds = _mm_loadu_si128((__m128i*)d);
    // 00e 01e 02e 03e 04e 05e 06e 07e
    const __m128i de = _mm_loadu_si128((__m128i*)(d + width));
    const __m256i t0 = _mm256_inserti128_si256(_mm256_castsi128_si256(ds), de, 1);
    // 00s 01s 02s 03s 00e 01e 02e 03e  04s 05s 06s 07s 04e 05e 06e 07e
    const __m256i t1 = _mm256_permute4x64_epi64(t0, 0xD8);
    // 00s 00e 01s 01e 02s 02e 03s 03e  04s 04e 05s 05e 06s 06e 07s 07e
    return _mm256_shuffle_epi8(t1, shf);
}

static INLINE void step3_win5_avx2(const int16_t** const d, const int32_t d_stride, const int32_t width,
                                   const int32_t height, __m256i* const dd, __m256i ds[WIENER_WIN_CHROMA],
                                   __m256i deltas[WIENER_WIN_CHROMA]) {
    // 16-bit idx: 0, 4, 1, 5, 2, 6, 3, 7
    const __m256i shf = _mm256_setr_epi8(
        0, 1, 8, 9, 2, 3, 10, 11, 4, 5, 12, 13, 6, 7, 14, 15, 0, 1, 8, 9, 2, 3, 10, 11, 4, 5, 12, 13, 6, 7, 14, 15);

    int32_t y = height;
    do {
        *d += 2 * d_stride;

        // 30s 31s 32s 33s 40s 41s 42s 43s  30e 31e 32e 33e 40e 41e 42e 43e
        load_more_64_avx2(*d + 2 * d_stride, width, dd);
        // 30s 40s 31s 41s 32s 42s 33s 43s  30e 40e 31e 41e 32e 42e 33e 43e
        ds[3] = _mm256_shuffle_epi8(*dd, shf);

        // 40s 41s 42s 43s 50s 51s 52s 53s  40e 41e 42e 43e 50e 51e 52e 53e
        load_more_64_avx2(*d + 3 * d_stride, width, dd);
        // 40s 50s 41s 51s 42s 52s 43s 53s  40e 50e 41e 51e 42e 52e 43e 53e
        ds[4] = _mm256_shuffle_epi8(*dd, shf);

        madd_avx2(ds[0], ds[0], &deltas[0]);
        madd_avx2(ds[0], ds[1], &deltas[1]);
        madd_avx2(ds[0], ds[2], &deltas[2]);
        madd_avx2(ds[0], ds[3], &deltas[3]);
        madd_avx2(ds[0], ds[4], &deltas[4]);

        ds[0] = ds[2];
        ds[1] = ds[3];
        ds[2] = ds[4];
        y -= 2;
    } while (y);
}

static INLINE void step3_win5_oneline_avx2(const int16_t** const d, const int32_t d_stride, const int32_t width,
                                           const int32_t height, __m256i ds[WIENER_WIN_CHROMA],
                                           __m256i deltas[WIENER_WIN_CHROMA]) {
    const __m256i const_n1_0 = _mm256_setr_epi16(
        0xFFFF, 0, 0xFFFF, 0, 0xFFFF, 0, 0xFFFF, 0, 0xFFFF, 0, 0xFFFF, 0, 0xFFFF, 0, 0xFFFF, 0);

    int32_t y = height;
    do {
        __m256i dd;

        dd = ds[0];
        dd = _mm256_xor_si256(dd, const_n1_0);
        dd = _mm256_sub_epi16(dd, const_n1_0);

        // 60s 60e 61s 61e 62s 62e 63s 63e  64s 64e 65s 65e 66s 66e 67s 67e
        ds[4] = load_win7_avx2(*d, width);

        madd_avx2(dd, ds[0], &deltas[0]);
        madd_avx2(dd, ds[1], &deltas[1]);
        madd_avx2(dd, ds[2], &deltas[2]);
        madd_avx2(dd, ds[3], &deltas[3]);
        madd_avx2(dd, ds[4], &deltas[4]);

        ds[0] = ds[1];
        ds[1] = ds[2];
        ds[2] = ds[3];
        ds[3] = ds[4];

        *d += d_stride;
    } while (--y);
}

static INLINE void step3_win7_avx2(const int16_t** const d, const int32_t d_stride, const int32_t width,
                                   const int32_t height, __m256i ds[WIENER_WIN], __m256i deltas[WIENER_WIN]) {
    const __m256i const_n1_0 = _mm256_setr_epi16(
        0xFFFF, 0, 0xFFFF, 0, 0xFFFF, 0, 0xFFFF, 0, 0xFFFF, 0, 0xFFFF, 0, 0xFFFF, 0, 0xFFFF, 0);

    int32_t y = height;
    do {
        __m256i dd;

        dd = ds[0];
        dd = _mm256_xor_si256(dd, const_n1_0);
        dd = _mm256_sub_epi16(dd, const_n1_0);

        // 60s 60e 61s 61e 62s 62e 63s 63e  64s 64e 65s 65e 66s 66e 67s 67e
        ds[6] = load_win7_avx2(*d, width);

        madd_avx2(dd, ds[0], &deltas[0]);
        madd_avx2(dd, ds[1], &deltas[1]);
        madd_avx2(dd, ds[2], &deltas[2]);
        madd_avx2(dd, ds[3], &deltas[3]);
        madd_avx2(dd, ds[4], &deltas[4]);
        madd_avx2(dd, ds[5], &deltas[5]);
        madd_avx2(dd, ds[6], &deltas[6]);

        ds[0] = ds[1];
        ds[1] = ds[2];
        ds[2] = ds[3];
        ds[3] = ds[4];
        ds[4] = ds[5];
        ds[5] = ds[6];
        *d += d_stride;
    } while (--y);
}

#endif // AOM_DSP_X86_PICKRST_AVX2_H_
