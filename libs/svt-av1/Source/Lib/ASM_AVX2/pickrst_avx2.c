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

#include <immintrin.h> // AVX2
#include "synonyms.h"
#include "synonyms_avx2.h"
#include "aom_dsp_rtcd.h"
#include "pic_operators_inline_avx2.h"
#include "restoration.h"
#include "restoration_pick.h"
#include "utility.h"
#include "pickrst_avx2.h"
#include "transpose_sse2.h"
#include "transpose_avx2.h"

static INLINE uint8_t find_average_avx2(const uint8_t* src, int32_t h_start, int32_t h_end, int32_t v_start,
                                        int32_t v_end, int32_t stride) {
    const int32_t  width    = h_end - h_start;
    const int32_t  height   = v_end - v_start;
    const uint8_t* src_t    = src + v_start * stride + h_start;
    const int32_t  leftover = width & 31;
    int32_t        i        = height;
    __m256i        ss       = _mm256_setzero_si256();

    if (!leftover) {
        do {
            int32_t j = 0;
            do {
                const __m256i s   = _mm256_loadu_si256((__m256i*)(src_t + j));
                const __m256i sad = _mm256_sad_epu8(s, _mm256_setzero_si256());
                ss                = _mm256_add_epi32(ss, sad);
                j += 32;
            } while (j < width);

            src_t += stride;
        } while (--i);
    } else {
        const int32_t w32 = width - leftover;
        __m128i       mask_l, mask_h;

        if (leftover >= 16) {
            mask_l = _mm_set1_epi8(-1);
            mask_h = _mm_loadu_si128((__m128i*)(mask_8bit[leftover - 16]));
        } else {
            mask_l = _mm_loadu_si128((__m128i*)(mask_8bit[leftover]));
            mask_h = _mm_setzero_si128();
        }
        const __m256i mask = _mm256_inserti128_si256(_mm256_castsi128_si256(mask_l), mask_h, 1);

        do {
            int32_t j = 0;
            while (j < w32) {
                const __m256i s   = _mm256_loadu_si256((__m256i*)(src_t + j));
                const __m256i sad = _mm256_sad_epu8(s, _mm256_setzero_si256());
                ss                = _mm256_add_epi32(ss, sad);
                j += 32;
            };

            const __m256i s   = _mm256_loadu_si256((__m256i*)(src_t + j));
            const __m256i s_t = _mm256_and_si256(s, mask);
            const __m256i sad = _mm256_sad_epu8(s_t, _mm256_setzero_si256());
            ss                = _mm256_add_epi32(ss, sad);
            src_t += stride;
        } while (--i);
    }

    const uint32_t sum = hadd32_avx2_intrin(ss);
    const uint32_t avg = sum / (width * height);
    return (uint8_t)avg;
}

#if CONFIG_ENABLE_HIGH_BIT_DEPTH
static INLINE void add_u16_to_u32_avx2(const __m256i src, __m256i* const sum) {
    const __m256i s0 = _mm256_unpacklo_epi16(src, _mm256_setzero_si256());
    const __m256i s1 = _mm256_unpackhi_epi16(src, _mm256_setzero_si256());
    *sum             = _mm256_add_epi32(*sum, s0);
    *sum             = _mm256_add_epi32(*sum, s1);
}
#endif

static INLINE void add_32_to_64_avx2(const __m256i src, __m256i* const sum) {
    const __m256i s0 = _mm256_cvtepi32_epi64(_mm256_castsi256_si128(src));
    const __m256i s1 = _mm256_cvtepi32_epi64(_mm256_extracti128_si256(src, 1));
    *sum             = _mm256_add_epi64(*sum, s0);
    *sum             = _mm256_add_epi64(*sum, s1);
}
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
static INLINE uint16_t find_average_highbd_avx2(const uint16_t* src, int32_t h_start, int32_t h_end, int32_t v_start,
                                                int32_t v_end, int32_t stride, EbBitDepth bit_depth) {
    const int32_t   width    = h_end - h_start;
    const int32_t   height   = v_end - v_start;
    const uint16_t* src_t    = src + v_start * stride + h_start;
    const int32_t   leftover = width & 15;
    int32_t         i        = height;
    __m256i         sss      = _mm256_setzero_si256();

    if (bit_depth <= 10 || width <= 256) {
        if (!leftover) {
            do {
                __m256i ss = _mm256_setzero_si256();

                int32_t j = 0;
                do {
                    const __m256i s = _mm256_loadu_si256((__m256i*)(src_t + j));
                    ss              = _mm256_add_epi16(ss, s);
                    j += 16;
                } while (j < width);

                add_u16_to_u32_avx2(ss, &sss);

                src_t += stride;
            } while (--i);
        } else {
            const int32_t w16  = width - leftover;
            const __m256i mask = _mm256_loadu_si256((__m256i*)(mask_16bit[leftover]));

            do {
                __m256i ss = _mm256_setzero_si256();

                int32_t j = 0;
                while (j < w16) {
                    const __m256i s = _mm256_loadu_si256((__m256i*)(src_t + j));
                    ss              = _mm256_add_epi16(ss, s);
                    j += 16;
                };

                const __m256i s   = _mm256_loadu_si256((__m256i*)(src_t + j));
                const __m256i s_t = _mm256_and_si256(s, mask);
                ss                = _mm256_add_epi16(ss, s_t);

                add_u16_to_u32_avx2(ss, &sss);

                src_t += stride;
            } while (--i);
        }
    } else {
        if (!leftover) {
            do {
                __m256i ss = _mm256_setzero_si256();

                int32_t j = 0;
                do {
                    const __m256i s = _mm256_loadu_si256((__m256i*)(src_t + j));
                    ss              = _mm256_add_epi16(ss, s);
                    j += 16;
                    if (j % 256 == 0) {
                        add_u16_to_u32_avx2(ss, &sss);
                        ss = _mm256_setzero_si256();
                    }
                } while (j < width);

                add_u16_to_u32_avx2(ss, &sss);

                src_t += stride;
            } while (--i);
        } else {
            const int32_t w16  = width - leftover;
            const __m256i mask = _mm256_loadu_si256((__m256i*)(mask_16bit[leftover]));

            do {
                __m256i ss = _mm256_setzero_si256();

                int32_t j = 0;

                while (j < w16) {
                    const __m256i s = _mm256_loadu_si256((__m256i*)(src_t + j));
                    ss              = _mm256_add_epi16(ss, s);
                    j += 16;
                    if (j % 256 == 0) {
                        add_u16_to_u32_avx2(ss, &sss);
                        ss = _mm256_setzero_si256();
                    }
                }

                const __m256i s   = _mm256_loadu_si256((__m256i*)(src_t + j));
                const __m256i s_t = _mm256_and_si256(s, mask);
                ss                = _mm256_add_epi16(ss, s_t);

                add_u16_to_u32_avx2(ss, &sss);

                src_t += stride;
            } while (--i);
        }
    }

    const uint32_t sum = hadd32_avx2_intrin(sss);
    const uint32_t avg = sum / (width * height);
    return (uint16_t)avg;
}
#endif

// Note: when n = (width % 16) is not 0, it writes (16 - n) more data than
// required.
static INLINE void sub_avg_block_avx2(const uint8_t* src, const int32_t src_stride, const uint8_t avg,
                                      const int32_t width, const int32_t height, int16_t* dst,
                                      const int32_t dst_stride) {
    const __m256i a = _mm256_set1_epi16(avg);

    int32_t i = height + 1;
    do {
        int32_t j = 0;
        while (j < width) {
            const __m128i s  = _mm_loadu_si128((__m128i*)(src + j));
            const __m256i ss = _mm256_cvtepu8_epi16(s);
            const __m256i d  = _mm256_sub_epi16(ss, a);
            _mm256_storeu_si256((__m256i*)(dst + j), d);
            j += 16;
        };

        src += src_stride;
        dst += dst_stride;
    } while (--i);
}

#if CONFIG_ENABLE_HIGH_BIT_DEPTH
// Note: when n = (width % 16) is not 0, it writes (16 - n) more data than
// required.
static INLINE void sub_avg_block_highbd_avx2(const uint16_t* src, const int32_t src_stride, const uint16_t avg,
                                             const int32_t width, const int32_t height, int16_t* dst,
                                             const int32_t dst_stride) {
    const __m256i a = _mm256_set1_epi16(avg);

    int32_t i = height + 1;
    do {
        int32_t j = 0;
        while (j < width) {
            const __m256i s = _mm256_loadu_si256((__m256i*)(src + j));
            const __m256i d = _mm256_sub_epi16(s, a);
            _mm256_storeu_si256((__m256i*)(dst + j), d);
            j += 16;
        };

        src += src_stride;
        dst += dst_stride;
    } while (--i);
}
#endif

static INLINE void stats_top_win5_avx2(const __m256i src, const __m256i dgd, const int16_t* const d,
                                       const int32_t d_stride, __m256i sum_m[WIENER_WIN_CHROMA],
                                       __m256i sum_h[WIENER_WIN_CHROMA]) {
    __m256i dgds[WIENER_WIN_CHROMA];

    dgds[0] = _mm256_loadu_si256((__m256i*)(d + 0 * d_stride));
    dgds[1] = _mm256_loadu_si256((__m256i*)(d + 1 * d_stride));
    dgds[2] = _mm256_loadu_si256((__m256i*)(d + 2 * d_stride));
    dgds[3] = _mm256_loadu_si256((__m256i*)(d + 3 * d_stride));
    dgds[4] = _mm256_loadu_si256((__m256i*)(d + 4 * d_stride));

    madd_avx2(src, dgds[0], &sum_m[0]);
    madd_avx2(src, dgds[1], &sum_m[1]);
    madd_avx2(src, dgds[2], &sum_m[2]);
    madd_avx2(src, dgds[3], &sum_m[3]);
    madd_avx2(src, dgds[4], &sum_m[4]);

    madd_avx2(dgd, dgds[0], &sum_h[0]);
    madd_avx2(dgd, dgds[1], &sum_h[1]);
    madd_avx2(dgd, dgds[2], &sum_h[2]);
    madd_avx2(dgd, dgds[3], &sum_h[3]);
    madd_avx2(dgd, dgds[4], &sum_h[4]);
}

static INLINE void stats_top_win7_avx2(const __m256i src, const __m256i dgd, const int16_t* const d,
                                       const int32_t d_stride, __m256i sum_m[WIENER_WIN], __m256i sum_h[WIENER_WIN]) {
    __m256i dgds[WIENER_WIN];

    dgds[0] = _mm256_loadu_si256((__m256i*)(d + 0 * d_stride));
    dgds[1] = _mm256_loadu_si256((__m256i*)(d + 1 * d_stride));
    dgds[2] = _mm256_loadu_si256((__m256i*)(d + 2 * d_stride));
    dgds[3] = _mm256_loadu_si256((__m256i*)(d + 3 * d_stride));
    dgds[4] = _mm256_loadu_si256((__m256i*)(d + 4 * d_stride));
    dgds[5] = _mm256_loadu_si256((__m256i*)(d + 5 * d_stride));
    dgds[6] = _mm256_loadu_si256((__m256i*)(d + 6 * d_stride));

    madd_avx2(src, dgds[0], &sum_m[0]);
    madd_avx2(src, dgds[1], &sum_m[1]);
    madd_avx2(src, dgds[2], &sum_m[2]);
    madd_avx2(src, dgds[3], &sum_m[3]);
    madd_avx2(src, dgds[4], &sum_m[4]);
    madd_avx2(src, dgds[5], &sum_m[5]);
    madd_avx2(src, dgds[6], &sum_m[6]);

    madd_avx2(dgd, dgds[0], &sum_h[0]);
    madd_avx2(dgd, dgds[1], &sum_h[1]);
    madd_avx2(dgd, dgds[2], &sum_h[2]);
    madd_avx2(dgd, dgds[3], &sum_h[3]);
    madd_avx2(dgd, dgds[4], &sum_h[4]);
    madd_avx2(dgd, dgds[5], &sum_h[5]);
    madd_avx2(dgd, dgds[6], &sum_h[6]);
}

static INLINE void stats_left_win5_avx2(const __m256i src, const int16_t* d, const int32_t d_stride,
                                        __m256i sum[WIENER_WIN_CHROMA - 1]) {
    __m256i dgds[WIENER_WIN_CHROMA - 1];

    dgds[0] = _mm256_loadu_si256((__m256i*)(d + 1 * d_stride));
    dgds[1] = _mm256_loadu_si256((__m256i*)(d + 2 * d_stride));
    dgds[2] = _mm256_loadu_si256((__m256i*)(d + 3 * d_stride));
    dgds[3] = _mm256_loadu_si256((__m256i*)(d + 4 * d_stride));

    madd_avx2(src, dgds[0], &sum[0]);
    madd_avx2(src, dgds[1], &sum[1]);
    madd_avx2(src, dgds[2], &sum[2]);
    madd_avx2(src, dgds[3], &sum[3]);
}

static INLINE void stats_left_win7_avx2(const __m256i src, const int16_t* d, const int32_t d_stride,
                                        __m256i sum[WIENER_WIN - 1]) {
    __m256i dgds[WIENER_WIN - 1];

    dgds[0] = _mm256_loadu_si256((__m256i*)(d + 1 * d_stride));
    dgds[1] = _mm256_loadu_si256((__m256i*)(d + 2 * d_stride));
    dgds[2] = _mm256_loadu_si256((__m256i*)(d + 3 * d_stride));
    dgds[3] = _mm256_loadu_si256((__m256i*)(d + 4 * d_stride));
    dgds[4] = _mm256_loadu_si256((__m256i*)(d + 5 * d_stride));
    dgds[5] = _mm256_loadu_si256((__m256i*)(d + 6 * d_stride));

    madd_avx2(src, dgds[0], &sum[0]);
    madd_avx2(src, dgds[1], &sum[1]);
    madd_avx2(src, dgds[2], &sum[2]);
    madd_avx2(src, dgds[3], &sum[3]);
    madd_avx2(src, dgds[4], &sum[4]);
    madd_avx2(src, dgds[5], &sum[5]);
}

static INLINE void load_square_win5_avx2(const int16_t* const di, const int16_t* const d_j, const int32_t d_stride,
                                         const int32_t height, __m256i d_is[WIENER_WIN_CHROMA - 1],
                                         __m256i d_ie[WIENER_WIN_CHROMA - 1], __m256i d_js[WIENER_WIN_CHROMA - 1],
                                         __m256i d_je[WIENER_WIN_CHROMA - 1]) {
    d_is[0] = _mm256_loadu_si256((__m256i*)(di + 0 * d_stride));
    d_js[0] = _mm256_loadu_si256((__m256i*)(d_j + 0 * d_stride));
    d_is[1] = _mm256_loadu_si256((__m256i*)(di + 1 * d_stride));
    d_js[1] = _mm256_loadu_si256((__m256i*)(d_j + 1 * d_stride));
    d_is[2] = _mm256_loadu_si256((__m256i*)(di + 2 * d_stride));
    d_js[2] = _mm256_loadu_si256((__m256i*)(d_j + 2 * d_stride));
    d_is[3] = _mm256_loadu_si256((__m256i*)(di + 3 * d_stride));
    d_js[3] = _mm256_loadu_si256((__m256i*)(d_j + 3 * d_stride));

    d_ie[0] = _mm256_loadu_si256((__m256i*)(di + (0 + height) * d_stride));
    d_je[0] = _mm256_loadu_si256((__m256i*)(d_j + (0 + height) * d_stride));
    d_ie[1] = _mm256_loadu_si256((__m256i*)(di + (1 + height) * d_stride));
    d_je[1] = _mm256_loadu_si256((__m256i*)(d_j + (1 + height) * d_stride));
    d_ie[2] = _mm256_loadu_si256((__m256i*)(di + (2 + height) * d_stride));
    d_je[2] = _mm256_loadu_si256((__m256i*)(d_j + (2 + height) * d_stride));
    d_ie[3] = _mm256_loadu_si256((__m256i*)(di + (3 + height) * d_stride));
    d_je[3] = _mm256_loadu_si256((__m256i*)(d_j + (3 + height) * d_stride));
}

static INLINE void load_square_win7_avx2(const int16_t* const di, const int16_t* const d_j, const int32_t d_stride,
                                         const int32_t height, __m256i d_is[WIENER_WIN - 1],
                                         __m256i d_ie[WIENER_WIN - 1], __m256i d_js[WIENER_WIN - 1],
                                         __m256i d_je[WIENER_WIN - 1]) {
    d_is[0] = _mm256_loadu_si256((__m256i*)(di + 0 * d_stride));
    d_js[0] = _mm256_loadu_si256((__m256i*)(d_j + 0 * d_stride));
    d_is[1] = _mm256_loadu_si256((__m256i*)(di + 1 * d_stride));
    d_js[1] = _mm256_loadu_si256((__m256i*)(d_j + 1 * d_stride));
    d_is[2] = _mm256_loadu_si256((__m256i*)(di + 2 * d_stride));
    d_js[2] = _mm256_loadu_si256((__m256i*)(d_j + 2 * d_stride));
    d_is[3] = _mm256_loadu_si256((__m256i*)(di + 3 * d_stride));
    d_js[3] = _mm256_loadu_si256((__m256i*)(d_j + 3 * d_stride));
    d_is[4] = _mm256_loadu_si256((__m256i*)(di + 4 * d_stride));
    d_js[4] = _mm256_loadu_si256((__m256i*)(d_j + 4 * d_stride));
    d_is[5] = _mm256_loadu_si256((__m256i*)(di + 5 * d_stride));
    d_js[5] = _mm256_loadu_si256((__m256i*)(d_j + 5 * d_stride));

    d_ie[0] = _mm256_loadu_si256((__m256i*)(di + (0 + height) * d_stride));
    d_je[0] = _mm256_loadu_si256((__m256i*)(d_j + (0 + height) * d_stride));
    d_ie[1] = _mm256_loadu_si256((__m256i*)(di + (1 + height) * d_stride));
    d_je[1] = _mm256_loadu_si256((__m256i*)(d_j + (1 + height) * d_stride));
    d_ie[2] = _mm256_loadu_si256((__m256i*)(di + (2 + height) * d_stride));
    d_je[2] = _mm256_loadu_si256((__m256i*)(d_j + (2 + height) * d_stride));
    d_ie[3] = _mm256_loadu_si256((__m256i*)(di + (3 + height) * d_stride));
    d_je[3] = _mm256_loadu_si256((__m256i*)(d_j + (3 + height) * d_stride));
    d_ie[4] = _mm256_loadu_si256((__m256i*)(di + (4 + height) * d_stride));
    d_je[4] = _mm256_loadu_si256((__m256i*)(d_j + (4 + height) * d_stride));
    d_ie[5] = _mm256_loadu_si256((__m256i*)(di + (5 + height) * d_stride));
    d_je[5] = _mm256_loadu_si256((__m256i*)(d_j + (5 + height) * d_stride));
}

static INLINE void load_triangle_win5_avx2(const int16_t* const di, const int32_t d_stride, const int32_t height,
                                           __m256i d_is[WIENER_WIN_CHROMA - 1], __m256i d_ie[WIENER_WIN_CHROMA - 1]) {
    d_is[0] = _mm256_loadu_si256((__m256i*)(di + 0 * d_stride));
    d_is[1] = _mm256_loadu_si256((__m256i*)(di + 1 * d_stride));
    d_is[2] = _mm256_loadu_si256((__m256i*)(di + 2 * d_stride));
    d_is[3] = _mm256_loadu_si256((__m256i*)(di + 3 * d_stride));

    d_ie[0] = _mm256_loadu_si256((__m256i*)(di + (0 + height) * d_stride));
    d_ie[1] = _mm256_loadu_si256((__m256i*)(di + (1 + height) * d_stride));
    d_ie[2] = _mm256_loadu_si256((__m256i*)(di + (2 + height) * d_stride));
    d_ie[3] = _mm256_loadu_si256((__m256i*)(di + (3 + height) * d_stride));
}

static INLINE void load_triangle_win7_avx2(const int16_t* const di, const int32_t d_stride, const int32_t height,
                                           __m256i d_is[WIENER_WIN - 1], __m256i d_ie[WIENER_WIN - 1]) {
    d_is[0] = _mm256_loadu_si256((__m256i*)(di + 0 * d_stride));
    d_is[1] = _mm256_loadu_si256((__m256i*)(di + 1 * d_stride));
    d_is[2] = _mm256_loadu_si256((__m256i*)(di + 2 * d_stride));
    d_is[3] = _mm256_loadu_si256((__m256i*)(di + 3 * d_stride));
    d_is[4] = _mm256_loadu_si256((__m256i*)(di + 4 * d_stride));
    d_is[5] = _mm256_loadu_si256((__m256i*)(di + 5 * d_stride));

    d_ie[0] = _mm256_loadu_si256((__m256i*)(di + (0 + height) * d_stride));
    d_ie[1] = _mm256_loadu_si256((__m256i*)(di + (1 + height) * d_stride));
    d_ie[2] = _mm256_loadu_si256((__m256i*)(di + (2 + height) * d_stride));
    d_ie[3] = _mm256_loadu_si256((__m256i*)(di + (3 + height) * d_stride));
    d_ie[4] = _mm256_loadu_si256((__m256i*)(di + (4 + height) * d_stride));
    d_ie[5] = _mm256_loadu_si256((__m256i*)(di + (5 + height) * d_stride));
}

static INLINE void derive_square_win5_avx2(const __m256i d_is[WIENER_WIN_CHROMA - 1],
                                           const __m256i d_ie[WIENER_WIN_CHROMA - 1],
                                           const __m256i d_js[WIENER_WIN_CHROMA - 1],
                                           const __m256i d_je[WIENER_WIN_CHROMA - 1],
                                           __m256i       deltas[WIENER_WIN_CHROMA - 1][WIENER_WIN_CHROMA - 1]) {
    msub_avx2(d_is[0], d_js[0], &deltas[0][0]);
    msub_avx2(d_is[0], d_js[1], &deltas[0][1]);
    msub_avx2(d_is[0], d_js[2], &deltas[0][2]);
    msub_avx2(d_is[0], d_js[3], &deltas[0][3]);
    msub_avx2(d_is[1], d_js[0], &deltas[1][0]);
    msub_avx2(d_is[1], d_js[1], &deltas[1][1]);
    msub_avx2(d_is[1], d_js[2], &deltas[1][2]);
    msub_avx2(d_is[1], d_js[3], &deltas[1][3]);
    msub_avx2(d_is[2], d_js[0], &deltas[2][0]);
    msub_avx2(d_is[2], d_js[1], &deltas[2][1]);
    msub_avx2(d_is[2], d_js[2], &deltas[2][2]);
    msub_avx2(d_is[2], d_js[3], &deltas[2][3]);
    msub_avx2(d_is[3], d_js[0], &deltas[3][0]);
    msub_avx2(d_is[3], d_js[1], &deltas[3][1]);
    msub_avx2(d_is[3], d_js[2], &deltas[3][2]);
    msub_avx2(d_is[3], d_js[3], &deltas[3][3]);

    madd_avx2(d_ie[0], d_je[0], &deltas[0][0]);
    madd_avx2(d_ie[0], d_je[1], &deltas[0][1]);
    madd_avx2(d_ie[0], d_je[2], &deltas[0][2]);
    madd_avx2(d_ie[0], d_je[3], &deltas[0][3]);
    madd_avx2(d_ie[1], d_je[0], &deltas[1][0]);
    madd_avx2(d_ie[1], d_je[1], &deltas[1][1]);
    madd_avx2(d_ie[1], d_je[2], &deltas[1][2]);
    madd_avx2(d_ie[1], d_je[3], &deltas[1][3]);
    madd_avx2(d_ie[2], d_je[0], &deltas[2][0]);
    madd_avx2(d_ie[2], d_je[1], &deltas[2][1]);
    madd_avx2(d_ie[2], d_je[2], &deltas[2][2]);
    madd_avx2(d_ie[2], d_je[3], &deltas[2][3]);
    madd_avx2(d_ie[3], d_je[0], &deltas[3][0]);
    madd_avx2(d_ie[3], d_je[1], &deltas[3][1]);
    madd_avx2(d_ie[3], d_je[2], &deltas[3][2]);
    madd_avx2(d_ie[3], d_je[3], &deltas[3][3]);
}

static INLINE void derive_square_win7_avx2(const __m256i d_is[WIENER_WIN - 1], const __m256i d_ie[WIENER_WIN - 1],
                                           const __m256i d_js[WIENER_WIN - 1], const __m256i d_je[WIENER_WIN - 1],
                                           __m256i deltas[WIENER_WIN - 1][WIENER_WIN - 1]) {
    msub_avx2(d_is[0], d_js[0], &deltas[0][0]);
    msub_avx2(d_is[0], d_js[1], &deltas[0][1]);
    msub_avx2(d_is[0], d_js[2], &deltas[0][2]);
    msub_avx2(d_is[0], d_js[3], &deltas[0][3]);
    msub_avx2(d_is[0], d_js[4], &deltas[0][4]);
    msub_avx2(d_is[0], d_js[5], &deltas[0][5]);
    msub_avx2(d_is[1], d_js[0], &deltas[1][0]);
    msub_avx2(d_is[1], d_js[1], &deltas[1][1]);
    msub_avx2(d_is[1], d_js[2], &deltas[1][2]);
    msub_avx2(d_is[1], d_js[3], &deltas[1][3]);
    msub_avx2(d_is[1], d_js[4], &deltas[1][4]);
    msub_avx2(d_is[1], d_js[5], &deltas[1][5]);
    msub_avx2(d_is[2], d_js[0], &deltas[2][0]);
    msub_avx2(d_is[2], d_js[1], &deltas[2][1]);
    msub_avx2(d_is[2], d_js[2], &deltas[2][2]);
    msub_avx2(d_is[2], d_js[3], &deltas[2][3]);
    msub_avx2(d_is[2], d_js[4], &deltas[2][4]);
    msub_avx2(d_is[2], d_js[5], &deltas[2][5]);
    msub_avx2(d_is[3], d_js[0], &deltas[3][0]);
    msub_avx2(d_is[3], d_js[1], &deltas[3][1]);
    msub_avx2(d_is[3], d_js[2], &deltas[3][2]);
    msub_avx2(d_is[3], d_js[3], &deltas[3][3]);
    msub_avx2(d_is[3], d_js[4], &deltas[3][4]);
    msub_avx2(d_is[3], d_js[5], &deltas[3][5]);
    msub_avx2(d_is[4], d_js[0], &deltas[4][0]);
    msub_avx2(d_is[4], d_js[1], &deltas[4][1]);
    msub_avx2(d_is[4], d_js[2], &deltas[4][2]);
    msub_avx2(d_is[4], d_js[3], &deltas[4][3]);
    msub_avx2(d_is[4], d_js[4], &deltas[4][4]);
    msub_avx2(d_is[4], d_js[5], &deltas[4][5]);
    msub_avx2(d_is[5], d_js[0], &deltas[5][0]);
    msub_avx2(d_is[5], d_js[1], &deltas[5][1]);
    msub_avx2(d_is[5], d_js[2], &deltas[5][2]);
    msub_avx2(d_is[5], d_js[3], &deltas[5][3]);
    msub_avx2(d_is[5], d_js[4], &deltas[5][4]);
    msub_avx2(d_is[5], d_js[5], &deltas[5][5]);

    madd_avx2(d_ie[0], d_je[0], &deltas[0][0]);
    madd_avx2(d_ie[0], d_je[1], &deltas[0][1]);
    madd_avx2(d_ie[0], d_je[2], &deltas[0][2]);
    madd_avx2(d_ie[0], d_je[3], &deltas[0][3]);
    madd_avx2(d_ie[0], d_je[4], &deltas[0][4]);
    madd_avx2(d_ie[0], d_je[5], &deltas[0][5]);
    madd_avx2(d_ie[1], d_je[0], &deltas[1][0]);
    madd_avx2(d_ie[1], d_je[1], &deltas[1][1]);
    madd_avx2(d_ie[1], d_je[2], &deltas[1][2]);
    madd_avx2(d_ie[1], d_je[3], &deltas[1][3]);
    madd_avx2(d_ie[1], d_je[4], &deltas[1][4]);
    madd_avx2(d_ie[1], d_je[5], &deltas[1][5]);
    madd_avx2(d_ie[2], d_je[0], &deltas[2][0]);
    madd_avx2(d_ie[2], d_je[1], &deltas[2][1]);
    madd_avx2(d_ie[2], d_je[2], &deltas[2][2]);
    madd_avx2(d_ie[2], d_je[3], &deltas[2][3]);
    madd_avx2(d_ie[2], d_je[4], &deltas[2][4]);
    madd_avx2(d_ie[2], d_je[5], &deltas[2][5]);
    madd_avx2(d_ie[3], d_je[0], &deltas[3][0]);
    madd_avx2(d_ie[3], d_je[1], &deltas[3][1]);
    madd_avx2(d_ie[3], d_je[2], &deltas[3][2]);
    madd_avx2(d_ie[3], d_je[3], &deltas[3][3]);
    madd_avx2(d_ie[3], d_je[4], &deltas[3][4]);
    madd_avx2(d_ie[3], d_je[5], &deltas[3][5]);
    madd_avx2(d_ie[4], d_je[0], &deltas[4][0]);
    madd_avx2(d_ie[4], d_je[1], &deltas[4][1]);
    madd_avx2(d_ie[4], d_je[2], &deltas[4][2]);
    madd_avx2(d_ie[4], d_je[3], &deltas[4][3]);
    madd_avx2(d_ie[4], d_je[4], &deltas[4][4]);
    madd_avx2(d_ie[4], d_je[5], &deltas[4][5]);
    madd_avx2(d_ie[5], d_je[0], &deltas[5][0]);
    madd_avx2(d_ie[5], d_je[1], &deltas[5][1]);
    madd_avx2(d_ie[5], d_je[2], &deltas[5][2]);
    madd_avx2(d_ie[5], d_je[3], &deltas[5][3]);
    madd_avx2(d_ie[5], d_je[4], &deltas[5][4]);
    madd_avx2(d_ie[5], d_je[5], &deltas[5][5]);
}

static INLINE void derive_triangle_win5_avx2(const __m256i d_is[WIENER_WIN_CHROMA - 1],
                                             const __m256i d_ie[WIENER_WIN_CHROMA - 1],
                                             __m256i       deltas[WIENER_WIN_CHROMA * (WIENER_WIN_CHROMA - 1) / 2]) {
    msub_avx2(d_is[0], d_is[0], &deltas[0]);
    msub_avx2(d_is[0], d_is[1], &deltas[1]);
    msub_avx2(d_is[0], d_is[2], &deltas[2]);
    msub_avx2(d_is[0], d_is[3], &deltas[3]);
    msub_avx2(d_is[1], d_is[1], &deltas[4]);
    msub_avx2(d_is[1], d_is[2], &deltas[5]);
    msub_avx2(d_is[1], d_is[3], &deltas[6]);
    msub_avx2(d_is[2], d_is[2], &deltas[7]);
    msub_avx2(d_is[2], d_is[3], &deltas[8]);
    msub_avx2(d_is[3], d_is[3], &deltas[9]);

    madd_avx2(d_ie[0], d_ie[0], &deltas[0]);
    madd_avx2(d_ie[0], d_ie[1], &deltas[1]);
    madd_avx2(d_ie[0], d_ie[2], &deltas[2]);
    madd_avx2(d_ie[0], d_ie[3], &deltas[3]);
    madd_avx2(d_ie[1], d_ie[1], &deltas[4]);
    madd_avx2(d_ie[1], d_ie[2], &deltas[5]);
    madd_avx2(d_ie[1], d_ie[3], &deltas[6]);
    madd_avx2(d_ie[2], d_ie[2], &deltas[7]);
    madd_avx2(d_ie[2], d_ie[3], &deltas[8]);
    madd_avx2(d_ie[3], d_ie[3], &deltas[9]);
}

static INLINE void derive_triangle_win7_avx2(const __m256i d_is[WIENER_WIN - 1], const __m256i d_ie[WIENER_WIN - 1],
                                             __m256i deltas[WIENER_WIN * (WIENER_WIN - 1) / 2]) {
    msub_avx2(d_is[0], d_is[0], &deltas[0]);
    msub_avx2(d_is[0], d_is[1], &deltas[1]);
    msub_avx2(d_is[0], d_is[2], &deltas[2]);
    msub_avx2(d_is[0], d_is[3], &deltas[3]);
    msub_avx2(d_is[0], d_is[4], &deltas[4]);
    msub_avx2(d_is[0], d_is[5], &deltas[5]);
    msub_avx2(d_is[1], d_is[1], &deltas[6]);
    msub_avx2(d_is[1], d_is[2], &deltas[7]);
    msub_avx2(d_is[1], d_is[3], &deltas[8]);
    msub_avx2(d_is[1], d_is[4], &deltas[9]);
    msub_avx2(d_is[1], d_is[5], &deltas[10]);
    msub_avx2(d_is[2], d_is[2], &deltas[11]);
    msub_avx2(d_is[2], d_is[3], &deltas[12]);
    msub_avx2(d_is[2], d_is[4], &deltas[13]);
    msub_avx2(d_is[2], d_is[5], &deltas[14]);
    msub_avx2(d_is[3], d_is[3], &deltas[15]);
    msub_avx2(d_is[3], d_is[4], &deltas[16]);
    msub_avx2(d_is[3], d_is[5], &deltas[17]);
    msub_avx2(d_is[4], d_is[4], &deltas[18]);
    msub_avx2(d_is[4], d_is[5], &deltas[19]);
    msub_avx2(d_is[5], d_is[5], &deltas[20]);

    madd_avx2(d_ie[0], d_ie[0], &deltas[0]);
    madd_avx2(d_ie[0], d_ie[1], &deltas[1]);
    madd_avx2(d_ie[0], d_ie[2], &deltas[2]);
    madd_avx2(d_ie[0], d_ie[3], &deltas[3]);
    madd_avx2(d_ie[0], d_ie[4], &deltas[4]);
    madd_avx2(d_ie[0], d_ie[5], &deltas[5]);
    madd_avx2(d_ie[1], d_ie[1], &deltas[6]);
    madd_avx2(d_ie[1], d_ie[2], &deltas[7]);
    madd_avx2(d_ie[1], d_ie[3], &deltas[8]);
    madd_avx2(d_ie[1], d_ie[4], &deltas[9]);
    madd_avx2(d_ie[1], d_ie[5], &deltas[10]);
    madd_avx2(d_ie[2], d_ie[2], &deltas[11]);
    madd_avx2(d_ie[2], d_ie[3], &deltas[12]);
    madd_avx2(d_ie[2], d_ie[4], &deltas[13]);
    madd_avx2(d_ie[2], d_ie[5], &deltas[14]);
    madd_avx2(d_ie[3], d_ie[3], &deltas[15]);
    madd_avx2(d_ie[3], d_ie[4], &deltas[16]);
    madd_avx2(d_ie[3], d_ie[5], &deltas[17]);
    madd_avx2(d_ie[4], d_ie[4], &deltas[18]);
    madd_avx2(d_ie[4], d_ie[5], &deltas[19]);
    madd_avx2(d_ie[5], d_ie[5], &deltas[20]);
}

#if CONFIG_ENABLE_HIGH_BIT_DEPTH
static INLINE __m256i div4_avx2(const __m256i src) {
    __m256i sign, dst;

    // get sign
    sign = _mm256_srli_epi64(src, 63);
    sign = _mm256_sub_epi64(_mm256_setzero_si256(), sign);

    // abs
    dst = _mm256_xor_si256(src, sign);
    dst = _mm256_sub_epi64(dst, sign);

    // divide by 4
    dst = _mm256_srli_epi64(dst, 2);

    // apply sign
    dst = _mm256_xor_si256(dst, sign);
    return _mm256_sub_epi64(dst, sign);
}

static INLINE __m256i div16_avx2(const __m256i src) {
    __m256i sign, dst;

    // get sign
    sign = _mm256_srli_epi64(src, 63);
    sign = _mm256_sub_epi64(_mm256_setzero_si256(), sign);

    // abs
    dst = _mm256_xor_si256(src, sign);
    dst = _mm256_sub_epi64(dst, sign);

    // divide by 16
    dst = _mm256_srli_epi64(dst, 4);

    // apply sign
    dst = _mm256_xor_si256(dst, sign);
    return _mm256_sub_epi64(dst, sign);
}

static INLINE void div4_4x4_avx2(const int32_t wiener_win2, int64_t* const H, __m256i out[4]) {
    out[0] = _mm256_loadu_si256((__m256i*)(H + 0 * wiener_win2));
    out[1] = _mm256_loadu_si256((__m256i*)(H + 1 * wiener_win2));
    out[2] = _mm256_loadu_si256((__m256i*)(H + 2 * wiener_win2));
    out[3] = _mm256_loadu_si256((__m256i*)(H + 3 * wiener_win2));

    out[0] = div4_avx2(out[0]);
    out[1] = div4_avx2(out[1]);
    out[2] = div4_avx2(out[2]);
    out[3] = div4_avx2(out[3]);

    _mm256_storeu_si256((__m256i*)(H + 0 * wiener_win2), out[0]);
    _mm256_storeu_si256((__m256i*)(H + 1 * wiener_win2), out[1]);
    _mm256_storeu_si256((__m256i*)(H + 2 * wiener_win2), out[2]);
    _mm256_storeu_si256((__m256i*)(H + 3 * wiener_win2), out[3]);
}

static INLINE void div16_4x4_avx2(const int32_t wiener_win2, int64_t* const H, __m256i out[4]) {
    out[0] = _mm256_loadu_si256((__m256i*)(H + 0 * wiener_win2));
    out[1] = _mm256_loadu_si256((__m256i*)(H + 1 * wiener_win2));
    out[2] = _mm256_loadu_si256((__m256i*)(H + 2 * wiener_win2));
    out[3] = _mm256_loadu_si256((__m256i*)(H + 3 * wiener_win2));

    out[0] = div16_avx2(out[0]);
    out[1] = div16_avx2(out[1]);
    out[2] = div16_avx2(out[2]);
    out[3] = div16_avx2(out[3]);

    _mm256_storeu_si256((__m256i*)(H + 0 * wiener_win2), out[0]);
    _mm256_storeu_si256((__m256i*)(H + 1 * wiener_win2), out[1]);
    _mm256_storeu_si256((__m256i*)(H + 2 * wiener_win2), out[2]);
    _mm256_storeu_si256((__m256i*)(H + 3 * wiener_win2), out[3]);
}
#endif

// Transpose each 4x4 block starting from the second column, and save the needed
// points only.
static INLINE void diagonal_copy_stats_avx2(const int32_t wiener_win2, int64_t* const H) {
    for (int32_t i = 0; i < wiener_win2 - 1; i += 4) {
        __m256i in[4], out[4];

        in[0] = _mm256_loadu_si256((__m256i*)(H + (i + 0) * wiener_win2 + i + 1));
        in[1] = _mm256_loadu_si256((__m256i*)(H + (i + 1) * wiener_win2 + i + 1));
        in[2] = _mm256_loadu_si256((__m256i*)(H + (i + 2) * wiener_win2 + i + 1));
        in[3] = _mm256_loadu_si256((__m256i*)(H + (i + 3) * wiener_win2 + i + 1));

        transpose_64bit_4x4_avx2(in, out);

        _mm_storel_epi64((__m128i*)(H + (i + 1) * wiener_win2 + i), _mm256_castsi256_si128(out[0]));
        _mm_storeu_si128((__m128i*)(H + (i + 2) * wiener_win2 + i), _mm256_castsi256_si128(out[1]));
        _mm256_storeu_si256((__m256i*)(H + (i + 3) * wiener_win2 + i), out[2]);
        _mm256_storeu_si256((__m256i*)(H + (i + 4) * wiener_win2 + i), out[3]);

        for (int32_t j = i + 5; j < wiener_win2; j += 4) {
            in[0] = _mm256_loadu_si256((__m256i*)(H + (i + 0) * wiener_win2 + j));
            in[1] = _mm256_loadu_si256((__m256i*)(H + (i + 1) * wiener_win2 + j));
            in[2] = _mm256_loadu_si256((__m256i*)(H + (i + 2) * wiener_win2 + j));
            in[3] = _mm256_loadu_si256((__m256i*)(H + (i + 3) * wiener_win2 + j));

            transpose_64bit_4x4_avx2(in, out);

            _mm256_storeu_si256((__m256i*)(H + (j + 0) * wiener_win2 + i), out[0]);
            _mm256_storeu_si256((__m256i*)(H + (j + 1) * wiener_win2 + i), out[1]);
            _mm256_storeu_si256((__m256i*)(H + (j + 2) * wiener_win2 + i), out[2]);
            _mm256_storeu_si256((__m256i*)(H + (j + 3) * wiener_win2 + i), out[3]);
        }
    }
}

#if CONFIG_ENABLE_HIGH_BIT_DEPTH
// Transpose each 4x4 block starting from the second column, and save the needed
// points only.
// H[4 * k * wiener_win2 + 4 * k] on the diagonal is omitted, and must be
// processed separately.
static INLINE void div4_diagonal_copy_stats_avx2(const int32_t wiener_win2, int64_t* const H) {
    for (int32_t i = 0; i < wiener_win2 - 1; i += 4) {
        __m256i in[4], out[4];

        div4_4x4_avx2(wiener_win2, H + i * wiener_win2 + i + 1, in);
        transpose_64bit_4x4_avx2(in, out);

        _mm_storel_epi64((__m128i*)(H + (i + 1) * wiener_win2 + i), _mm256_castsi256_si128(out[0]));
        _mm_storeu_si128((__m128i*)(H + (i + 2) * wiener_win2 + i), _mm256_castsi256_si128(out[1]));
        _mm256_storeu_si256((__m256i*)(H + (i + 3) * wiener_win2 + i), out[2]);
        _mm256_storeu_si256((__m256i*)(H + (i + 4) * wiener_win2 + i), out[3]);

        for (int32_t j = i + 5; j < wiener_win2; j += 4) {
            div4_4x4_avx2(wiener_win2, H + i * wiener_win2 + j, in);
            transpose_64bit_4x4_avx2(in, out);

            _mm256_storeu_si256((__m256i*)(H + (j + 0) * wiener_win2 + i), out[0]);
            _mm256_storeu_si256((__m256i*)(H + (j + 1) * wiener_win2 + i), out[1]);
            _mm256_storeu_si256((__m256i*)(H + (j + 2) * wiener_win2 + i), out[2]);
            _mm256_storeu_si256((__m256i*)(H + (j + 3) * wiener_win2 + i), out[3]);
        }
    }
}

// Transpose each 4x4 block starting from the second column, and save the needed
// points only.
// H[4 * k * wiener_win2 + 4 * k] on the diagonal is omitted, and must be
// processed separately.
static INLINE void div16_diagonal_copy_stats_avx2(const int32_t wiener_win2, int64_t* const H) {
    for (int32_t i = 0; i < wiener_win2 - 1; i += 4) {
        __m256i in[4], out[4];

        div16_4x4_avx2(wiener_win2, H + i * wiener_win2 + i + 1, in);
        transpose_64bit_4x4_avx2(in, out);

        _mm_storel_epi64((__m128i*)(H + (i + 1) * wiener_win2 + i), _mm256_castsi256_si128(out[0]));
        _mm_storeu_si128((__m128i*)(H + (i + 2) * wiener_win2 + i), _mm256_castsi256_si128(out[1]));
        _mm256_storeu_si256((__m256i*)(H + (i + 3) * wiener_win2 + i), out[2]);
        _mm256_storeu_si256((__m256i*)(H + (i + 4) * wiener_win2 + i), out[3]);

        for (int32_t j = i + 5; j < wiener_win2; j += 4) {
            div16_4x4_avx2(wiener_win2, H + i * wiener_win2 + j, in);
            transpose_64bit_4x4_avx2(in, out);

            _mm256_storeu_si256((__m256i*)(H + (j + 0) * wiener_win2 + i), out[0]);
            _mm256_storeu_si256((__m256i*)(H + (j + 1) * wiener_win2 + i), out[1]);
            _mm256_storeu_si256((__m256i*)(H + (j + 2) * wiener_win2 + i), out[2]);
            _mm256_storeu_si256((__m256i*)(H + (j + 3) * wiener_win2 + i), out[3]);
        }
    }
}
#endif

static INLINE void compute_stats_win5_avx2(const int16_t* const d, const int32_t d_stride, const int16_t* const s,
                                           const int32_t s_stride, const int32_t width, const int32_t height,
                                           int64_t* const M, int64_t* const H, EbBitDepth bit_depth) {
    const int32_t wiener_win  = WIENER_WIN_CHROMA;
    const int32_t wiener_win2 = wiener_win * wiener_win;
    const int32_t w16         = width & ~15;
    const int32_t h8          = height & ~7;
    const __m256i mask        = _mm256_loadu_si256((__m256i*)(mask_16bit[width - w16]));
    int32_t       i, j, x, y;

    if (bit_depth == EB_EIGHT_BIT) {
        // Step 1: Calculate the top edge of the whole matrix, i.e., the top
        // edge of each triangle and square on the top row.
        j = 0;
        do {
            const int16_t* s_t                      = s;
            const int16_t* d_t                      = d;
            __m256i        sum_m[WIENER_WIN_CHROMA] = {_mm256_setzero_si256()};
            __m256i        sum_h[WIENER_WIN_CHROMA] = {_mm256_setzero_si256()};

            y = height;
            do {
                x = 0;
                while (x < w16) {
                    const __m256i src = _mm256_loadu_si256((__m256i*)(s_t + x));
                    const __m256i dgd = _mm256_loadu_si256((__m256i*)(d_t + x));
                    stats_top_win5_avx2(src, dgd, d_t + j + x, d_stride, sum_m, sum_h);
                    x += 16;
                };

                if (w16 != width) {
                    const __m256i src      = _mm256_loadu_si256((__m256i*)(s_t + w16));
                    const __m256i dgd      = _mm256_loadu_si256((__m256i*)(d_t + w16));
                    const __m256i src_mask = _mm256_and_si256(src, mask);
                    const __m256i dgd_mask = _mm256_and_si256(dgd, mask);
                    stats_top_win5_avx2(src_mask, dgd_mask, d_t + j + w16, d_stride, sum_m, sum_h);
                }

                s_t += s_stride;
                d_t += d_stride;
            } while (--y);

            const __m256i s_m   = hadd_four_32_to_64_avx2(sum_m[0], sum_m[1], sum_m[2], sum_m[3]);
            const __m128i s_m_h = hadd_two_32_to_64_avx2(sum_m[4], sum_h[4]);
            _mm256_storeu_si256((__m256i*)(M + wiener_win * j), s_m);
            _mm_storel_epi64((__m128i*)&M[wiener_win * j + 4], s_m_h);

            const __m256i s_h = hadd_four_32_to_64_avx2(sum_h[0], sum_h[1], sum_h[2], sum_h[3]);
            _mm256_storeu_si256((__m256i*)(H + wiener_win * j), s_h);
            _mm_storeh_epi64((__m128i*)&H[wiener_win * j + 4], s_m_h);
        } while (++j < wiener_win);

        // Step 2: Calculate the left edge of each square on the top row.
        j = 1;
        do {
            const int16_t* d_t                          = d;
            __m256i        sum_h[WIENER_WIN_CHROMA - 1] = {_mm256_setzero_si256()};

            y = height;
            do {
                x = 0;
                while (x < w16) {
                    const __m256i dgd = _mm256_loadu_si256((__m256i*)(d_t + j + x));
                    stats_left_win5_avx2(dgd, d_t + x, d_stride, sum_h);
                    x += 16;
                };

                if (w16 != width) {
                    const __m256i dgd      = _mm256_loadu_si256((__m256i*)(d_t + j + x));
                    const __m256i dgd_mask = _mm256_and_si256(dgd, mask);
                    stats_left_win5_avx2(dgd_mask, d_t + x, d_stride, sum_h);
                }

                d_t += d_stride;
            } while (--y);

            const __m256i sum = hadd_four_32_to_64_avx2(sum_h[0], sum_h[1], sum_h[2], sum_h[3]);
            _mm_storel_epi64((__m128i*)&H[1 * wiener_win2 + j * wiener_win], _mm256_castsi256_si128(sum));
            _mm_storeh_epi64((__m128i*)&H[2 * wiener_win2 + j * wiener_win], _mm256_castsi256_si128(sum));
            _mm_storel_epi64((__m128i*)&H[3 * wiener_win2 + j * wiener_win], _mm256_extracti128_si256(sum, 1));
            _mm_storeh_epi64((__m128i*)&H[4 * wiener_win2 + j * wiener_win], _mm256_extracti128_si256(sum, 1));
        } while (++j < wiener_win);
    } else {
        const int32_t num_bit_left = 32 - 1 /* sign */ - 2 * bit_depth /* energy */ + 3 /* SIMD */;
        const int32_t h_allowed    = (1 << num_bit_left) / (w16 + ((w16 != width) ? 16 : 0));

        // Step 1: Calculate the top edge of the whole matrix, i.e., the top
        // edge of each triangle and square on the top row.
        j = 0;
        do {
            const int16_t* s_t                      = s;
            const int16_t* d_t                      = d;
            int32_t        height_t                 = 0;
            __m256i        sum_m[WIENER_WIN_CHROMA] = {_mm256_setzero_si256()};
            __m256i        sum_h[WIENER_WIN_CHROMA] = {_mm256_setzero_si256()};

            do {
                const int32_t h_t = ((height - height_t) < h_allowed) ? (height - height_t) : h_allowed;
                __m256i       row_m[WIENER_WIN_CHROMA] = {_mm256_setzero_si256()};
                __m256i       row_h[WIENER_WIN_CHROMA] = {_mm256_setzero_si256()};

                y = h_t;
                do {
                    x = 0;
                    while (x < w16) {
                        const __m256i src = _mm256_loadu_si256((__m256i*)(s_t + x));
                        const __m256i dgd = _mm256_loadu_si256((__m256i*)(d_t + x));
                        stats_top_win5_avx2(src, dgd, d_t + j + x, d_stride, row_m, row_h);
                        x += 16;
                    };

                    if (w16 != width) {
                        const __m256i src      = _mm256_loadu_si256((__m256i*)(s_t + w16));
                        const __m256i dgd      = _mm256_loadu_si256((__m256i*)(d_t + w16));
                        const __m256i src_mask = _mm256_and_si256(src, mask);
                        const __m256i dgd_mask = _mm256_and_si256(dgd, mask);
                        stats_top_win5_avx2(src_mask, dgd_mask, d_t + j + w16, d_stride, row_m, row_h);
                    }

                    s_t += s_stride;
                    d_t += d_stride;
                } while (--y);

                add_32_to_64_avx2(row_m[0], &sum_m[0]);
                add_32_to_64_avx2(row_m[1], &sum_m[1]);
                add_32_to_64_avx2(row_m[2], &sum_m[2]);
                add_32_to_64_avx2(row_m[3], &sum_m[3]);
                add_32_to_64_avx2(row_m[4], &sum_m[4]);
                add_32_to_64_avx2(row_h[0], &sum_h[0]);
                add_32_to_64_avx2(row_h[1], &sum_h[1]);
                add_32_to_64_avx2(row_h[2], &sum_h[2]);
                add_32_to_64_avx2(row_h[3], &sum_h[3]);
                add_32_to_64_avx2(row_h[4], &sum_h[4]);

                height_t += h_t;
            } while (height_t < height);

            const __m256i s_m   = hadd_four_64_avx2(sum_m[0], sum_m[1], sum_m[2], sum_m[3]);
            const __m128i s_m_h = hadd_two_64_avx2(sum_m[4], sum_h[4]);
            _mm256_storeu_si256((__m256i*)(M + wiener_win * j), s_m);
            M[wiener_win * j + 4] = _mm_cvtsi128_si64(s_m_h);

            const __m256i s_h = hadd_four_64_avx2(sum_h[0], sum_h[1], sum_h[2], sum_h[3]);
            _mm256_storeu_si256((__m256i*)(H + wiener_win * j), s_h);
            _mm_storeh_epi64((__m128i*)&H[wiener_win * j + 4], s_m_h);
        } while (++j < wiener_win);

        // Step 2: Calculate the left edge of each square on the top row.
        j = 1;
        do {
            const int16_t* d_t                          = d;
            int32_t        height_t                     = 0;
            __m256i        sum_h[WIENER_WIN_CHROMA - 1] = {_mm256_setzero_si256()};

            do {
                const int32_t h_t = ((height - height_t) < h_allowed) ? (height - height_t) : h_allowed;
                __m256i       row_h[WIENER_WIN_CHROMA - 1] = {_mm256_setzero_si256()};

                y = h_t;
                do {
                    x = 0;
                    while (x < w16) {
                        const __m256i dgd = _mm256_loadu_si256((__m256i*)(d_t + j + x));
                        stats_left_win5_avx2(dgd, d_t + x, d_stride, row_h);
                        x += 16;
                    };

                    if (w16 != width) {
                        const __m256i dgd      = _mm256_loadu_si256((__m256i*)(d_t + j + x));
                        const __m256i dgd_mask = _mm256_and_si256(dgd, mask);
                        stats_left_win5_avx2(dgd_mask, d_t + x, d_stride, row_h);
                    }

                    d_t += d_stride;
                } while (--y);

                add_32_to_64_avx2(row_h[0], &sum_h[0]);
                add_32_to_64_avx2(row_h[1], &sum_h[1]);
                add_32_to_64_avx2(row_h[2], &sum_h[2]);
                add_32_to_64_avx2(row_h[3], &sum_h[3]);

                height_t += h_t;
            } while (height_t < height);

            const __m256i sum = hadd_four_64_avx2(sum_h[0], sum_h[1], sum_h[2], sum_h[3]);
            _mm_storel_epi64((__m128i*)&H[1 * wiener_win2 + j * wiener_win], _mm256_castsi256_si128(sum));
            _mm_storeh_epi64((__m128i*)&H[2 * wiener_win2 + j * wiener_win], _mm256_castsi256_si128(sum));
            _mm_storel_epi64((__m128i*)&H[3 * wiener_win2 + j * wiener_win], _mm256_extracti128_si256(sum, 1));
            _mm_storeh_epi64((__m128i*)&H[4 * wiener_win2 + j * wiener_win], _mm256_extracti128_si256(sum, 1));
        } while (++j < wiener_win);
    }

    // Step 3: Derive the top edge of each triangle along the diagonal. No
    // triangle in top row.
    {
        const int16_t* d_t = d;

        if (height % 2) {
            __m256i deltas[WIENER_WIN + 1] = {_mm256_setzero_si256()};
            __m256i ds[WIENER_WIN];

            ds[0] = load_win7_avx2(d_t + 0 * d_stride, width);
            ds[1] = load_win7_avx2(d_t + 1 * d_stride, width);
            ds[2] = load_win7_avx2(d_t + 2 * d_stride, width);
            ds[3] = load_win7_avx2(d_t + 3 * d_stride, width);
            d_t += 4 * d_stride;

            if (bit_depth < EB_TWELVE_BIT) {
                step3_win5_oneline_avx2(&d_t, d_stride, width, height, ds, deltas);
                transpose_32bit_8x8_avx2(deltas, deltas);

                update_5_stats_avx2(H + 0 * wiener_win * wiener_win2 + 0 * wiener_win,
                                    _mm256_castsi256_si128(deltas[0]),
                                    _mm256_extract_epi32(deltas[0], 4),
                                    H + 1 * wiener_win * wiener_win2 + 1 * wiener_win);

                update_5_stats_avx2(H + 1 * wiener_win * wiener_win2 + 1 * wiener_win,
                                    _mm256_castsi256_si128(deltas[1]),
                                    _mm256_extract_epi32(deltas[1], 4),
                                    H + 2 * wiener_win * wiener_win2 + 2 * wiener_win);

                update_5_stats_avx2(H + 2 * wiener_win * wiener_win2 + 2 * wiener_win,
                                    _mm256_castsi256_si128(deltas[2]),
                                    _mm256_extract_epi32(deltas[2], 4),
                                    H + 3 * wiener_win * wiener_win2 + 3 * wiener_win);

                update_5_stats_avx2(H + 3 * wiener_win * wiener_win2 + 3 * wiener_win,
                                    _mm256_castsi256_si128(deltas[3]),
                                    _mm256_extract_epi32(deltas[3], 4),
                                    H + 4 * wiener_win * wiener_win2 + 4 * wiener_win);
            } else {
                __m128i deltas128[WIENER_WIN] = {_mm_setzero_si128()};
                int32_t height_t              = 0;

                do {
                    __m256i       deltas_t[WIENER_WIN] = {_mm256_setzero_si256()};
                    const int32_t h_t                  = ((height - height_t) < 128) ? (height - height_t) : 128;

                    step3_win5_oneline_avx2(&d_t, d_stride, width, h_t, ds, deltas_t);
                    add_six_32_to_64_avx2(deltas_t[0], &deltas[0], &deltas128[0]);
                    add_six_32_to_64_avx2(deltas_t[1], &deltas[1], &deltas128[1]);
                    add_six_32_to_64_avx2(deltas_t[2], &deltas[2], &deltas128[2]);
                    add_six_32_to_64_avx2(deltas_t[3], &deltas[3], &deltas128[3]);
                    add_six_32_to_64_avx2(deltas_t[4], &deltas[4], &deltas128[4]);
                    add_six_32_to_64_avx2(deltas_t[5], &deltas[5], &deltas128[5]);
                    add_six_32_to_64_avx2(deltas_t[6], &deltas[6], &deltas128[6]);

                    height_t += h_t;
                } while (height_t < height);

                transpose_64bit_4x8_avx2(deltas, deltas);

                update_5_stats_highbd_avx2(H + 0 * wiener_win * wiener_win2 + 0 * wiener_win,
                                           deltas[0],
                                           _mm256_extract_epi64(deltas[1], 0),
                                           H + 1 * wiener_win * wiener_win2 + 1 * wiener_win);

                update_5_stats_highbd_avx2(H + 1 * wiener_win * wiener_win2 + 1 * wiener_win,
                                           deltas[2],
                                           _mm256_extract_epi64(deltas[3], 0),
                                           H + 2 * wiener_win * wiener_win2 + 2 * wiener_win);

                update_5_stats_highbd_avx2(H + 2 * wiener_win * wiener_win2 + 2 * wiener_win,
                                           deltas[4],
                                           _mm256_extract_epi64(deltas[5], 0),
                                           H + 3 * wiener_win * wiener_win2 + 3 * wiener_win);

                update_5_stats_highbd_avx2(H + 3 * wiener_win * wiener_win2 + 3 * wiener_win,
                                           deltas[6],
                                           _mm256_extract_epi64(deltas[7], 0),
                                           H + 4 * wiener_win * wiener_win2 + 4 * wiener_win);
            }

        } else {
            // 16-bit idx: 0, 4, 1, 5, 2, 6, 3, 7
            const __m256i shf                       = _mm256_setr_epi8(0,
                                                 1,
                                                 8,
                                                 9,
                                                 2,
                                                 3,
                                                 10,
                                                 11,
                                                 4,
                                                 5,
                                                 12,
                                                 13,
                                                 6,
                                                 7,
                                                 14,
                                                 15,
                                                 0,
                                                 1,
                                                 8,
                                                 9,
                                                 2,
                                                 3,
                                                 10,
                                                 11,
                                                 4,
                                                 5,
                                                 12,
                                                 13,
                                                 6,
                                                 7,
                                                 14,
                                                 15);
            __m256i       deltas[WIENER_WIN_CHROMA] = {_mm256_setzero_si256()};
            __m256i       dd                        = _mm256_setzero_si256(); // Initialize to avoid warning.
            __m256i       ds[WIENER_WIN_CHROMA];

            // 00s 01s 02s 03s 10s 11s 12s 13s  00e 01e 02e 03e 10e 11e 12e 13e
            dd = _mm256_insert_epi64(dd, *(int64_t*)(d_t + 0 * d_stride), 0);
            dd = _mm256_insert_epi64(dd, *(int64_t*)(d_t + 0 * d_stride + width), 2);
            dd = _mm256_insert_epi64(dd, *(int64_t*)(d_t + 1 * d_stride), 1);
            dd = _mm256_insert_epi64(dd, *(int64_t*)(d_t + 1 * d_stride + width), 3);
            // 00s 10s 01s 11s 02s 12s 03s 13s  00e 10e 01e 11e 02e 12e 03e 13e
            ds[0] = _mm256_shuffle_epi8(dd, shf);

            // 10s 11s 12s 13s 20s 21s 22s 23s  10e 11e 12e 13e 20e 21e 22e 23e
            load_more_64_avx2(d_t + 2 * d_stride, width, &dd);
            // 10s 20s 11s 21s 12s 22s 13s 23s  10e 20e 11e 21e 12e 22e 13e 23e
            ds[1] = _mm256_shuffle_epi8(dd, shf);

            // 20s 21s 22s 23s 30s 31s 32s 33s  20e 21e 22e 23e 30e 31e 32e 33e
            load_more_64_avx2(d_t + 3 * d_stride, width, &dd);
            // 20s 30s 21s 31s 22s 32s 23s 33s  20e 30e 21e 31e 22e 32e 23e 33e
            ds[2] = _mm256_shuffle_epi8(dd, shf);

            if (bit_depth < EB_TWELVE_BIT) {
                __m128i dlts[WIENER_WIN_CHROMA];

                step3_win5_avx2(&d_t, d_stride, width, height, &dd, ds, deltas);

                dlts[0] = sub_hi_lo_32_avx2(deltas[0]);
                dlts[1] = sub_hi_lo_32_avx2(deltas[1]);
                dlts[2] = sub_hi_lo_32_avx2(deltas[2]);
                dlts[3] = sub_hi_lo_32_avx2(deltas[3]);
                dlts[4] = sub_hi_lo_32_avx2(deltas[4]);

                transpose_32bit_4x4(dlts, dlts);
                deltas[4] = _mm256_cvtepi32_epi64(dlts[4]);

                update_5_stats_avx2(H + 0 * wiener_win * wiener_win2 + 0 * wiener_win,
                                    dlts[0],
                                    _mm256_extract_epi64(deltas[4], 0),
                                    H + 1 * wiener_win * wiener_win2 + 1 * wiener_win);

                update_5_stats_avx2(H + 1 * wiener_win * wiener_win2 + 1 * wiener_win,
                                    dlts[1],
                                    _mm256_extract_epi64(deltas[4], 1),
                                    H + 2 * wiener_win * wiener_win2 + 2 * wiener_win);

                update_5_stats_avx2(H + 2 * wiener_win * wiener_win2 + 2 * wiener_win,
                                    dlts[2],
                                    _mm256_extract_epi64(deltas[4], 2),
                                    H + 3 * wiener_win * wiener_win2 + 3 * wiener_win);

                update_5_stats_avx2(H + 3 * wiener_win * wiener_win2 + 3 * wiener_win,
                                    dlts[3],
                                    _mm256_extract_epi64(deltas[4], 3),
                                    H + 4 * wiener_win * wiener_win2 + 4 * wiener_win);
            } else {
                int32_t height_t = 0;

                do {
                    __m256i       deltas_t[WIENER_WIN_CHROMA] = {_mm256_setzero_si256()};
                    const int32_t h_t                         = ((height - height_t) < 128) ? (height - height_t) : 128;

                    step3_win5_avx2(&d_t, d_stride, width, h_t, &dd, ds, deltas_t);

                    deltas_t[0] = hsub_32x8_to_64x4_avx2(deltas_t[0]);
                    deltas_t[1] = hsub_32x8_to_64x4_avx2(deltas_t[1]);
                    deltas_t[2] = hsub_32x8_to_64x4_avx2(deltas_t[2]);
                    deltas_t[3] = hsub_32x8_to_64x4_avx2(deltas_t[3]);
                    deltas_t[4] = hsub_32x8_to_64x4_avx2(deltas_t[4]);
                    deltas[0]   = _mm256_add_epi64(deltas[0], deltas_t[0]);
                    deltas[1]   = _mm256_add_epi64(deltas[1], deltas_t[1]);
                    deltas[2]   = _mm256_add_epi64(deltas[2], deltas_t[2]);
                    deltas[3]   = _mm256_add_epi64(deltas[3], deltas_t[3]);
                    deltas[4]   = _mm256_add_epi64(deltas[4], deltas_t[4]);

                    height_t += h_t;
                } while (height_t < height);

                transpose_64bit_4x4_avx2(deltas, deltas);

                update_5_stats_highbd_avx2(H + 0 * wiener_win * wiener_win2 + 0 * wiener_win,
                                           deltas[0],
                                           _mm256_extract_epi64(deltas[4], 0),
                                           H + 1 * wiener_win * wiener_win2 + 1 * wiener_win);

                update_5_stats_highbd_avx2(H + 1 * wiener_win * wiener_win2 + 1 * wiener_win,
                                           deltas[1],
                                           _mm256_extract_epi64(deltas[4], 1),
                                           H + 2 * wiener_win * wiener_win2 + 2 * wiener_win);

                update_5_stats_highbd_avx2(H + 2 * wiener_win * wiener_win2 + 2 * wiener_win,
                                           deltas[2],
                                           _mm256_extract_epi64(deltas[4], 2),
                                           H + 3 * wiener_win * wiener_win2 + 3 * wiener_win);

                update_5_stats_highbd_avx2(H + 3 * wiener_win * wiener_win2 + 3 * wiener_win,
                                           deltas[3],
                                           _mm256_extract_epi64(deltas[4], 3),
                                           H + 4 * wiener_win * wiener_win2 + 4 * wiener_win);
            }
        }
    }

    // Step 4: Derive the top and left edge of each square. No square in top and
    // bottom row.
    i = 1;
    do {
        j = i + 1;
        do {
            const int16_t* di  = d + i - 1;
            const int16_t* d_j = d + j - 1;
            __m128i        delta128, delta4;
            __m256i        delta;
            __m256i        deltas[2 * WIENER_WIN_CHROMA - 1] = {_mm256_setzero_si256()};
            __m256i        dd[WIENER_WIN_CHROMA], ds[WIENER_WIN_CHROMA];

            delta4 = _mm_setzero_si128(); // Initialize to avoid warning.

            dd[0] = _mm256_setzero_si256(); // Initialize to avoid warning.
            ds[0] = _mm256_setzero_si256(); // Initialize to avoid warning.

            dd[0] = _mm256_insert_epi16(dd[0], di[0 * d_stride], 0);
            dd[0] = _mm256_insert_epi16(dd[0], di[0 * d_stride + width], 8);
            dd[0] = _mm256_insert_epi16(dd[0], di[1 * d_stride], 1);
            dd[0] = _mm256_insert_epi16(dd[0], di[1 * d_stride + width], 9);
            dd[0] = _mm256_insert_epi16(dd[0], di[2 * d_stride], 2);
            dd[0] = _mm256_insert_epi16(dd[0], di[2 * d_stride + width], 10);
            dd[0] = _mm256_insert_epi16(dd[0], di[3 * d_stride], 3);
            dd[0] = _mm256_insert_epi16(dd[0], di[3 * d_stride + width], 11);

            ds[0] = _mm256_insert_epi16(ds[0], d_j[0 * d_stride], 0);
            ds[0] = _mm256_insert_epi16(ds[0], d_j[0 * d_stride + width], 8);
            ds[0] = _mm256_insert_epi16(ds[0], d_j[1 * d_stride], 1);
            ds[0] = _mm256_insert_epi16(ds[0], d_j[1 * d_stride + width], 9);
            ds[0] = _mm256_insert_epi16(ds[0], d_j[2 * d_stride], 2);
            ds[0] = _mm256_insert_epi16(ds[0], d_j[2 * d_stride + width], 10);
            ds[0] = _mm256_insert_epi16(ds[0], d_j[3 * d_stride], 3);
            ds[0] = _mm256_insert_epi16(ds[0], d_j[3 * d_stride + width], 11);

            y = 0;
            while (y < h8) {
                // 00s 10s 20s 30s 40s 50s 60s 70s  00e 10e 20e 30e 40e 50e 60e
                // 70e
                dd[0] = _mm256_insert_epi16(dd[0], di[4 * d_stride], 4);
                dd[0] = _mm256_insert_epi16(dd[0], di[4 * d_stride + width], 12);
                dd[0] = _mm256_insert_epi16(dd[0], di[5 * d_stride], 5);
                dd[0] = _mm256_insert_epi16(dd[0], di[5 * d_stride + width], 13);
                dd[0] = _mm256_insert_epi16(dd[0], di[6 * d_stride], 6);
                dd[0] = _mm256_insert_epi16(dd[0], di[6 * d_stride + width], 14);
                dd[0] = _mm256_insert_epi16(dd[0], di[7 * d_stride], 7);
                dd[0] = _mm256_insert_epi16(dd[0], di[7 * d_stride + width], 15);

                // 01s 11s 21s 31s 41s 51s 61s 71s  01e 11e 21e 31e 41e 51e 61e
                // 71e
                ds[0] = _mm256_insert_epi16(ds[0], d_j[4 * d_stride], 4);
                ds[0] = _mm256_insert_epi16(ds[0], d_j[4 * d_stride + width], 12);
                ds[0] = _mm256_insert_epi16(ds[0], d_j[5 * d_stride], 5);
                ds[0] = _mm256_insert_epi16(ds[0], d_j[5 * d_stride + width], 13);
                ds[0] = _mm256_insert_epi16(ds[0], d_j[6 * d_stride], 6);
                ds[0] = _mm256_insert_epi16(ds[0], d_j[6 * d_stride + width], 14);
                ds[0] = _mm256_insert_epi16(ds[0], d_j[7 * d_stride], 7);
                ds[0] = _mm256_insert_epi16(ds[0], d_j[7 * d_stride + width], 15);

                load_more_16_avx2(di + 8 * d_stride, width, dd[0], &dd[1]);
                load_more_16_avx2(d_j + 8 * d_stride, width, ds[0], &ds[1]);
                load_more_16_avx2(di + 9 * d_stride, width, dd[1], &dd[2]);
                load_more_16_avx2(d_j + 9 * d_stride, width, ds[1], &ds[2]);
                load_more_16_avx2(di + 10 * d_stride, width, dd[2], &dd[3]);
                load_more_16_avx2(d_j + 10 * d_stride, width, ds[2], &ds[3]);
                load_more_16_avx2(di + 11 * d_stride, width, dd[3], &dd[4]);
                load_more_16_avx2(d_j + 11 * d_stride, width, ds[3], &ds[4]);

                madd_avx2(dd[0], ds[0], &deltas[0]);
                madd_avx2(dd[0], ds[1], &deltas[1]);
                madd_avx2(dd[0], ds[2], &deltas[2]);
                madd_avx2(dd[0], ds[3], &deltas[3]);
                madd_avx2(dd[0], ds[4], &deltas[4]);
                madd_avx2(dd[1], ds[0], &deltas[5]);
                madd_avx2(dd[2], ds[0], &deltas[6]);
                madd_avx2(dd[3], ds[0], &deltas[7]);
                madd_avx2(dd[4], ds[0], &deltas[8]);

                dd[0] = _mm256_srli_si256(dd[4], 8);
                ds[0] = _mm256_srli_si256(ds[4], 8);
                di += 8 * d_stride;
                d_j += 8 * d_stride;
                y += 8;
            };

            if (bit_depth < EB_TWELVE_BIT) {
                deltas[0]            = _mm256_hadd_epi32(deltas[0], deltas[1]);
                deltas[2]            = _mm256_hadd_epi32(deltas[2], deltas[3]);
                deltas[4]            = _mm256_hadd_epi32(deltas[4], deltas[4]);
                deltas[5]            = _mm256_hadd_epi32(deltas[5], deltas[6]);
                deltas[7]            = _mm256_hadd_epi32(deltas[7], deltas[8]);
                deltas[0]            = _mm256_hadd_epi32(deltas[0], deltas[2]);
                deltas[4]            = _mm256_hadd_epi32(deltas[4], deltas[4]);
                deltas[5]            = _mm256_hadd_epi32(deltas[5], deltas[7]);
                const __m128i delta0 = sub_hi_lo_32_avx2(deltas[0]);
                const __m128i delta1 = sub_hi_lo_32_avx2(deltas[4]);
                delta128             = sub_hi_lo_32_avx2(deltas[5]);
                delta                = _mm256_inserti128_si256(_mm256_castsi128_si256(delta0), delta1, 1);
            } else {
                deltas[0] = hsub_32x8_to_64x4_avx2(deltas[0]);
                deltas[1] = hsub_32x8_to_64x4_avx2(deltas[1]);
                deltas[2] = hsub_32x8_to_64x4_avx2(deltas[2]);
                deltas[3] = hsub_32x8_to_64x4_avx2(deltas[3]);
                deltas[4] = hsub_32x8_to_64x4_avx2(deltas[4]);
                deltas[5] = hsub_32x8_to_64x4_avx2(deltas[5]);
                deltas[6] = hsub_32x8_to_64x4_avx2(deltas[6]);
                deltas[7] = hsub_32x8_to_64x4_avx2(deltas[7]);
                deltas[8] = hsub_32x8_to_64x4_avx2(deltas[8]);

                transpose_64bit_4x4_avx2(deltas + 0, deltas + 0);
                transpose_64bit_4x4_avx2(deltas + 5, deltas + 5);

                deltas[0] = _mm256_add_epi64(deltas[0], deltas[1]);
                deltas[2] = _mm256_add_epi64(deltas[2], deltas[3]);
                deltas[0] = _mm256_add_epi64(deltas[0], deltas[2]);
                deltas[5] = _mm256_add_epi64(deltas[5], deltas[6]);
                deltas[7] = _mm256_add_epi64(deltas[7], deltas[8]);
                deltas[5] = _mm256_add_epi64(deltas[5], deltas[7]);
                delta4    = hadd_64_avx2(deltas[4]);
                delta128  = _mm_setzero_si128();
                delta     = _mm256_setzero_si256();
            }

            if (h8 != height) {
                const __m256i perm = _mm256_setr_epi32(1, 2, 3, 4, 5, 6, 7, 0);
                __m128i       dd128, ds128;

                ds[0] = _mm256_insert_epi16(ds[0], d_j[0 * d_stride], 0);
                ds[0] = _mm256_insert_epi16(ds[0], d_j[0 * d_stride + width], 1);

                dd128 = _mm_cvtsi32_si128(-di[1 * d_stride]);
                dd128 = _mm_insert_epi16(dd128, di[1 * d_stride + width], 1);
                ds[0] = _mm256_insert_epi16(ds[0], d_j[1 * d_stride], 2);
                ds[0] = _mm256_insert_epi16(ds[0], d_j[1 * d_stride + width], 3);

                dd128 = _mm_insert_epi16(dd128, -di[2 * d_stride], 2);
                dd128 = _mm_insert_epi16(dd128, di[2 * d_stride + width], 3);
                ds[0] = _mm256_insert_epi16(ds[0], d_j[2 * d_stride], 4);
                ds[0] = _mm256_insert_epi16(ds[0], d_j[2 * d_stride + width], 5);

                dd128 = _mm_insert_epi16(dd128, -di[3 * d_stride], 4);
                dd128 = _mm_insert_epi16(dd128, di[3 * d_stride + width], 5);
                ds[0] = _mm256_insert_epi16(ds[0], d_j[3 * d_stride], 6);
                ds[0] = _mm256_insert_epi16(ds[0], d_j[3 * d_stride + width], 7);

                do {
                    __m128i t;

                    t     = _mm_cvtsi32_si128(-di[0 * d_stride]);
                    t     = _mm_insert_epi16(t, di[0 * d_stride + width], 1);
                    dd[0] = _mm256_broadcastd_epi32(t);

                    ds128 = _mm_cvtsi32_si128(d_j[0 * d_stride]);
                    ds128 = _mm_insert_epi16(ds128, d_j[0 * d_stride + width], 1);
                    ds128 = _mm_unpacklo_epi32(ds128, ds128);
                    ds128 = _mm_unpacklo_epi32(ds128, ds128);

                    dd128 = _mm_insert_epi16(dd128, -di[4 * d_stride], 6);
                    dd128 = _mm_insert_epi16(dd128, di[4 * d_stride + width], 7);
                    ds[0] = _mm256_insert_epi16(ds[0], d_j[4 * d_stride], 8);
                    ds[0] = _mm256_insert_epi16(ds[0], d_j[4 * d_stride + width], 9);

                    madd_avx2(dd[0], ds[0], &delta);
                    madd_sse2(dd128, ds128, &delta128);

                    // right shift 4 bytes
                    ds[0] = _mm256_permutevar8x32_epi32(ds[0], perm);
                    dd128 = _mm_srli_si128(dd128, 4);
                    di += d_stride;
                    d_j += d_stride;
                } while (++y < height);
            }

            if (bit_depth < EB_TWELVE_BIT) {
                update_4_stats_avx2(H + (i - 1) * wiener_win * wiener_win2 + (j - 1) * wiener_win,
                                    _mm256_castsi256_si128(delta),
                                    H + i * wiener_win * wiener_win2 + j * wiener_win);
                H[i * wiener_win * wiener_win2 + j * wiener_win + 4] =
                    H[(i - 1) * wiener_win * wiener_win2 + (j - 1) * wiener_win + 4] + _mm256_extract_epi32(delta, 4);

                H[(i * wiener_win + 1) * wiener_win2 + j * wiener_win] =
                    H[((i - 1) * wiener_win + 1) * wiener_win2 + (j - 1) * wiener_win] + _mm_extract_epi32(delta128, 0);
                H[(i * wiener_win + 2) * wiener_win2 + j * wiener_win] =
                    H[((i - 1) * wiener_win + 2) * wiener_win2 + (j - 1) * wiener_win] + _mm_extract_epi32(delta128, 1);
                H[(i * wiener_win + 3) * wiener_win2 + j * wiener_win] =
                    H[((i - 1) * wiener_win + 3) * wiener_win2 + (j - 1) * wiener_win] + _mm_extract_epi32(delta128, 2);
                H[(i * wiener_win + 4) * wiener_win2 + j * wiener_win] =
                    H[((i - 1) * wiener_win + 4) * wiener_win2 + (j - 1) * wiener_win] + _mm_extract_epi32(delta128, 3);
            } else {
                const __m256i d0 = _mm256_cvtepi32_epi64(_mm256_castsi256_si128(delta));
                const __m128i d1 = _mm_cvtepi32_epi64(_mm256_extracti128_si256(delta, 1));
                const __m256i d2 = _mm256_cvtepi32_epi64(delta128);
                deltas[0]        = _mm256_add_epi64(deltas[0], d0);
                delta4           = _mm_add_epi64(delta4, d1);
                deltas[5]        = _mm256_add_epi64(deltas[5], d2);

                update_4_stats_highbd_avx2(H + (i - 1) * wiener_win * wiener_win2 + (j - 1) * wiener_win,
                                           deltas[0],
                                           H + i * wiener_win * wiener_win2 + j * wiener_win);
                H[i * wiener_win * wiener_win2 + j * wiener_win + 4] =
                    H[(i - 1) * wiener_win * wiener_win2 + (j - 1) * wiener_win + 4] + _mm_extract_epi64(delta4, 0);

                H[(i * wiener_win + 1) * wiener_win2 + j * wiener_win] =
                    H[((i - 1) * wiener_win + 1) * wiener_win2 + (j - 1) * wiener_win] +
                    _mm256_extract_epi64(deltas[5], 0);
                H[(i * wiener_win + 2) * wiener_win2 + j * wiener_win] =
                    H[((i - 1) * wiener_win + 2) * wiener_win2 + (j - 1) * wiener_win] +
                    _mm256_extract_epi64(deltas[5], 1);
                H[(i * wiener_win + 3) * wiener_win2 + j * wiener_win] =
                    H[((i - 1) * wiener_win + 3) * wiener_win2 + (j - 1) * wiener_win] +
                    _mm256_extract_epi64(deltas[5], 2);
                H[(i * wiener_win + 4) * wiener_win2 + j * wiener_win] =
                    H[((i - 1) * wiener_win + 4) * wiener_win2 + (j - 1) * wiener_win] +
                    _mm256_extract_epi64(deltas[5], 3);
            }
        } while (++j < wiener_win);
    } while (++i < wiener_win - 1);

    // Step 5: Derive other points of each square. No square in bottom row.
    i = 0;
    do {
        const int16_t* const di = d + i;

        j = i + 1;
        do {
            const int16_t* const d_j                                                  = d + j;
            __m256i              deltas[WIENER_WIN_CHROMA - 1][WIENER_WIN_CHROMA - 1] = {{_mm256_setzero_si256()},
                                                                                         {_mm256_setzero_si256()}};
            __m256i              d_is[WIENER_WIN_CHROMA - 1], d_ie[WIENER_WIN_CHROMA - 1];
            __m256i              d_js[WIENER_WIN_CHROMA - 1], d_je[WIENER_WIN_CHROMA - 1];

            x = 0;
            while (x < w16) {
                load_square_win5_avx2(di + x, d_j + x, d_stride, height, d_is, d_ie, d_js, d_je);
                derive_square_win5_avx2(d_is, d_ie, d_js, d_je, deltas);

                x += 16;
            };

            if (w16 != width) {
                load_square_win5_avx2(di + x, d_j + x, d_stride, height, d_is, d_ie, d_js, d_je);

                d_is[0] = _mm256_and_si256(d_is[0], mask);
                d_is[1] = _mm256_and_si256(d_is[1], mask);
                d_is[2] = _mm256_and_si256(d_is[2], mask);
                d_is[3] = _mm256_and_si256(d_is[3], mask);
                d_ie[0] = _mm256_and_si256(d_ie[0], mask);
                d_ie[1] = _mm256_and_si256(d_ie[1], mask);
                d_ie[2] = _mm256_and_si256(d_ie[2], mask);
                d_ie[3] = _mm256_and_si256(d_ie[3], mask);

                derive_square_win5_avx2(d_is, d_ie, d_js, d_je, deltas);
            }

            if (bit_depth < EB_TWELVE_BIT) {
                hadd_update_4_stats_avx2(H + (i * wiener_win + 0) * wiener_win2 + j * wiener_win,
                                         deltas[0],
                                         H + (i * wiener_win + 1) * wiener_win2 + j * wiener_win + 1);
                hadd_update_4_stats_avx2(H + (i * wiener_win + 1) * wiener_win2 + j * wiener_win,
                                         deltas[1],
                                         H + (i * wiener_win + 2) * wiener_win2 + j * wiener_win + 1);
                hadd_update_4_stats_avx2(H + (i * wiener_win + 2) * wiener_win2 + j * wiener_win,
                                         deltas[2],
                                         H + (i * wiener_win + 3) * wiener_win2 + j * wiener_win + 1);
                hadd_update_4_stats_avx2(H + (i * wiener_win + 3) * wiener_win2 + j * wiener_win,
                                         deltas[3],
                                         H + (i * wiener_win + 4) * wiener_win2 + j * wiener_win + 1);
            } else {
                hadd_update_4_stats_highbd_avx2(H + (i * wiener_win + 0) * wiener_win2 + j * wiener_win,
                                                deltas[0],
                                                H + (i * wiener_win + 1) * wiener_win2 + j * wiener_win + 1);
                hadd_update_4_stats_highbd_avx2(H + (i * wiener_win + 1) * wiener_win2 + j * wiener_win,
                                                deltas[1],
                                                H + (i * wiener_win + 2) * wiener_win2 + j * wiener_win + 1);
                hadd_update_4_stats_highbd_avx2(H + (i * wiener_win + 2) * wiener_win2 + j * wiener_win,
                                                deltas[2],
                                                H + (i * wiener_win + 3) * wiener_win2 + j * wiener_win + 1);
                hadd_update_4_stats_highbd_avx2(H + (i * wiener_win + 3) * wiener_win2 + j * wiener_win,
                                                deltas[3],
                                                H + (i * wiener_win + 4) * wiener_win2 + j * wiener_win + 1);
            }
        } while (++j < wiener_win);
    } while (++i < wiener_win - 1);

    // Step 6: Derive other points of each upper triangle along the diagonal.
    i = 0;
    do {
        const int16_t* const di                                                      = d + i;
        __m256i              deltas[WIENER_WIN_CHROMA * (WIENER_WIN_CHROMA - 1) / 2] = {_mm256_setzero_si256()};
        __m256i              d_is[WIENER_WIN_CHROMA - 1], d_ie[WIENER_WIN_CHROMA - 1];

        x = 0;
        while (x < w16) {
            load_triangle_win5_avx2(di + x, d_stride, height, d_is, d_ie);
            derive_triangle_win5_avx2(d_is, d_ie, deltas);

            x += 16;
        };

        if (w16 != width) {
            load_triangle_win5_avx2(di + x, d_stride, height, d_is, d_ie);

            d_is[0] = _mm256_and_si256(d_is[0], mask);
            d_is[1] = _mm256_and_si256(d_is[1], mask);
            d_is[2] = _mm256_and_si256(d_is[2], mask);
            d_is[3] = _mm256_and_si256(d_is[3], mask);
            d_ie[0] = _mm256_and_si256(d_ie[0], mask);
            d_ie[1] = _mm256_and_si256(d_ie[1], mask);
            d_ie[2] = _mm256_and_si256(d_ie[2], mask);
            d_ie[3] = _mm256_and_si256(d_ie[3], mask);

            derive_triangle_win5_avx2(d_is, d_ie, deltas);
        }

        if (bit_depth < EB_TWELVE_BIT) {
            hadd_update_4_stats_avx2(H + (i * wiener_win + 0) * wiener_win2 + i * wiener_win,
                                     deltas,
                                     H + (i * wiener_win + 1) * wiener_win2 + i * wiener_win + 1);

            const __m128i delta32 = hadd_four_32_avx2(deltas[4], deltas[5], deltas[6], deltas[9]);
            const __m128i delta64 = _mm_cvtepi32_epi64(delta32);

            update_2_stats_sse2(H + (i * wiener_win + 1) * wiener_win2 + i * wiener_win + 1,
                                delta64,
                                H + (i * wiener_win + 2) * wiener_win2 + i * wiener_win + 2);
            H[(i * wiener_win + 2) * wiener_win2 + i * wiener_win + 4] =
                H[(i * wiener_win + 1) * wiener_win2 + i * wiener_win + 3] + _mm_extract_epi32(delta32, 2);

            const __m128i d32 = hadd_two_32_avx2(deltas[7], deltas[8]);
            const __m128i d64 = _mm_cvtepi32_epi64(d32);

            update_2_stats_sse2(H + (i * wiener_win + 2) * wiener_win2 + i * wiener_win + 2,
                                d64,
                                H + (i * wiener_win + 3) * wiener_win2 + i * wiener_win + 3);
            H[(i * wiener_win + 4) * wiener_win2 + i * wiener_win + 4] =
                H[(i * wiener_win + 3) * wiener_win2 + i * wiener_win + 3] + _mm_extract_epi32(delta32, 3);
        } else {
            hadd_update_4_stats_highbd_avx2(H + (i * wiener_win + 0) * wiener_win2 + i * wiener_win,
                                            deltas,
                                            H + (i * wiener_win + 1) * wiener_win2 + i * wiener_win + 1);

            const __m256i delta64 = hadd_four_31_to_64_avx2(deltas[4], deltas[5], deltas[6], deltas[9]);

            update_2_stats_sse2(H + (i * wiener_win + 1) * wiener_win2 + i * wiener_win + 1,
                                _mm256_castsi256_si128(delta64),
                                H + (i * wiener_win + 2) * wiener_win2 + i * wiener_win + 2);
            H[(i * wiener_win + 2) * wiener_win2 + i * wiener_win + 4] =
                H[(i * wiener_win + 1) * wiener_win2 + i * wiener_win + 3] + _mm256_extract_epi64(delta64, 2);

            const __m128i d64 = hadd_two_31_to_64_avx2(deltas[7], deltas[8]);

            update_2_stats_sse2(H + (i * wiener_win + 2) * wiener_win2 + i * wiener_win + 2,
                                d64,
                                H + (i * wiener_win + 3) * wiener_win2 + i * wiener_win + 3);
            H[(i * wiener_win + 4) * wiener_win2 + i * wiener_win + 4] =
                H[(i * wiener_win + 3) * wiener_win2 + i * wiener_win + 3] + _mm256_extract_epi64(delta64, 3);
        }
    } while (++i < wiener_win);
}

static INLINE void compute_stats_win7_avx2(const int16_t* const d, const int32_t d_stride, const int16_t* const s,
                                           const int32_t s_stride, const int32_t width, const int32_t height,
                                           int64_t* const M, int64_t* const H, EbBitDepth bit_depth) {
    const int32_t wiener_win  = WIENER_WIN;
    const int32_t wiener_win2 = wiener_win * wiener_win;
    const int32_t w16         = width & ~15;
    const int32_t h8          = height & ~7;
    const __m256i mask        = _mm256_loadu_si256((__m256i*)(mask_16bit[width - w16]));
    int32_t       i, j, x, y;

    if (bit_depth == EB_EIGHT_BIT) {
        // Step 1: Calculate the top edge of the whole matrix, i.e., the top
        // edge of each triangle and square on the top row.
        j = 0;
        do {
            const int16_t* s_t               = s;
            const int16_t* d_t               = d;
            __m256i        sum_m[WIENER_WIN] = {_mm256_setzero_si256()};
            __m256i        sum_h[WIENER_WIN] = {_mm256_setzero_si256()};

            y = height;
            do {
                x = 0;
                while (x < w16) {
                    const __m256i src = _mm256_loadu_si256((__m256i*)(s_t + x));
                    const __m256i dgd = _mm256_loadu_si256((__m256i*)(d_t + x));
                    stats_top_win7_avx2(src, dgd, d_t + j + x, d_stride, sum_m, sum_h);
                    x += 16;
                };

                if (w16 != width) {
                    const __m256i src      = _mm256_loadu_si256((__m256i*)(s_t + w16));
                    const __m256i dgd      = _mm256_loadu_si256((__m256i*)(d_t + w16));
                    const __m256i src_mask = _mm256_and_si256(src, mask);
                    const __m256i dgd_mask = _mm256_and_si256(dgd, mask);
                    stats_top_win7_avx2(src_mask, dgd_mask, d_t + j + w16, d_stride, sum_m, sum_h);
                }

                s_t += s_stride;
                d_t += d_stride;
            } while (--y);

            const __m256i s_m0 = hadd_four_32_to_64_avx2(sum_m[0], sum_m[1], sum_m[2], sum_m[3]);
            const __m256i s_m1 = hadd_four_32_to_64_avx2(sum_m[4], sum_m[5], sum_m[6], sum_m[6]);
            _mm256_storeu_si256((__m256i*)(M + wiener_win * j + 0), s_m0);
            _mm_storeu_si128((__m128i*)(M + wiener_win * j + 4), _mm256_castsi256_si128(s_m1));
            _mm_storel_epi64((__m128i*)&M[wiener_win * j + 6], _mm256_extracti128_si256(s_m1, 1));

            const __m256i sh_0 = hadd_four_32_to_64_avx2(sum_h[0], sum_h[1], sum_h[2], sum_h[3]);
            const __m256i sh_1 = hadd_four_32_to_64_avx2(sum_h[4], sum_h[5], sum_h[6], sum_h[6]);
            _mm256_storeu_si256((__m256i*)(H + wiener_win * j + 0), sh_0);
            // Writing one more H on the top edge falls to the second row, so it
            // won't overflow.
            _mm256_storeu_si256((__m256i*)(H + wiener_win * j + 4), sh_1);
        } while (++j < wiener_win);

        // Step 2: Calculate the left edge of each square on the top row.
        j = 1;
        do {
            const int16_t* d_t                   = d;
            __m256i        sum_h[WIENER_WIN - 1] = {_mm256_setzero_si256()};

            y = height;
            do {
                x = 0;
                while (x < w16) {
                    const __m256i dgd = _mm256_loadu_si256((__m256i*)(d_t + j + x));
                    stats_left_win7_avx2(dgd, d_t + x, d_stride, sum_h);
                    x += 16;
                };

                if (w16 != width) {
                    const __m256i dgd      = _mm256_loadu_si256((__m256i*)(d_t + j + x));
                    const __m256i dgd_mask = _mm256_and_si256(dgd, mask);
                    stats_left_win7_avx2(dgd_mask, d_t + x, d_stride, sum_h);
                }

                d_t += d_stride;
            } while (--y);

            const __m256i sum0123 = hadd_four_32_to_64_avx2(sum_h[0], sum_h[1], sum_h[2], sum_h[3]);
            _mm_storel_epi64((__m128i*)&H[1 * wiener_win2 + j * wiener_win], _mm256_castsi256_si128(sum0123));
            _mm_storeh_epi64((__m128i*)&H[2 * wiener_win2 + j * wiener_win], _mm256_castsi256_si128(sum0123));
            _mm_storel_epi64((__m128i*)&H[3 * wiener_win2 + j * wiener_win], _mm256_extracti128_si256(sum0123, 1));
            _mm_storeh_epi64((__m128i*)&H[4 * wiener_win2 + j * wiener_win], _mm256_extracti128_si256(sum0123, 1));

            const __m128i sum45 = hadd_two_32_to_64_avx2(sum_h[4], sum_h[5]);
            _mm_storel_epi64((__m128i*)&H[5 * wiener_win2 + j * wiener_win], sum45);
            _mm_storeh_epi64((__m128i*)&H[6 * wiener_win2 + j * wiener_win], sum45);
        } while (++j < wiener_win);
    } else {
        const int32_t num_bit_left = 32 - 1 /* sign */ - 2 * bit_depth /* energy */ + 3 /* SIMD */;
        const int32_t h_allowed    = (1 << num_bit_left) / (w16 + ((w16 != width) ? 16 : 0));

        // Step 1: Calculate the top edge of the whole matrix, i.e., the top
        // edge of each triangle and square on the top row.
        j = 0;
        do {
            const int16_t* s_t               = s;
            const int16_t* d_t               = d;
            int32_t        height_t          = 0;
            __m256i        sum_m[WIENER_WIN] = {_mm256_setzero_si256()};
            __m256i        sum_h[WIENER_WIN] = {_mm256_setzero_si256()};

            do {
                const int32_t h_t               = ((height - height_t) < h_allowed) ? (height - height_t) : h_allowed;
                __m256i       row_m[WIENER_WIN] = {_mm256_setzero_si256()};
                __m256i       row_h[WIENER_WIN] = {_mm256_setzero_si256()};

                y = h_t;
                do {
                    x = 0;
                    while (x < w16) {
                        const __m256i src = _mm256_loadu_si256((__m256i*)(s_t + x));
                        const __m256i dgd = _mm256_loadu_si256((__m256i*)(d_t + x));
                        stats_top_win7_avx2(src, dgd, d_t + j + x, d_stride, row_m, row_h);
                        x += 16;
                    };

                    if (w16 != width) {
                        const __m256i src      = _mm256_loadu_si256((__m256i*)(s_t + w16));
                        const __m256i dgd      = _mm256_loadu_si256((__m256i*)(d_t + w16));
                        const __m256i src_mask = _mm256_and_si256(src, mask);
                        const __m256i dgd_mask = _mm256_and_si256(dgd, mask);
                        stats_top_win7_avx2(src_mask, dgd_mask, d_t + j + w16, d_stride, row_m, row_h);
                    }

                    s_t += s_stride;
                    d_t += d_stride;
                } while (--y);

                add_32_to_64_avx2(row_m[0], &sum_m[0]);
                add_32_to_64_avx2(row_m[1], &sum_m[1]);
                add_32_to_64_avx2(row_m[2], &sum_m[2]);
                add_32_to_64_avx2(row_m[3], &sum_m[3]);
                add_32_to_64_avx2(row_m[4], &sum_m[4]);
                add_32_to_64_avx2(row_m[5], &sum_m[5]);
                add_32_to_64_avx2(row_m[6], &sum_m[6]);
                add_32_to_64_avx2(row_h[0], &sum_h[0]);
                add_32_to_64_avx2(row_h[1], &sum_h[1]);
                add_32_to_64_avx2(row_h[2], &sum_h[2]);
                add_32_to_64_avx2(row_h[3], &sum_h[3]);
                add_32_to_64_avx2(row_h[4], &sum_h[4]);
                add_32_to_64_avx2(row_h[5], &sum_h[5]);
                add_32_to_64_avx2(row_h[6], &sum_h[6]);

                height_t += h_t;
            } while (height_t < height);

            const __m256i s_m0 = hadd_four_64_avx2(sum_m[0], sum_m[1], sum_m[2], sum_m[3]);
            const __m256i s_m1 = hadd_four_64_avx2(sum_m[4], sum_m[5], sum_m[6], sum_m[6]);
            _mm256_storeu_si256((__m256i*)(M + wiener_win * j + 0), s_m0);
            _mm_storeu_si128((__m128i*)(M + wiener_win * j + 4), _mm256_castsi256_si128(s_m1));
            _mm_storel_epi64((__m128i*)&M[wiener_win * j + 6], _mm256_extracti128_si256(s_m1, 1));

            const __m256i sh_0 = hadd_four_64_avx2(sum_h[0], sum_h[1], sum_h[2], sum_h[3]);
            const __m256i sh_1 = hadd_four_64_avx2(sum_h[4], sum_h[5], sum_h[6], sum_h[6]);
            _mm256_storeu_si256((__m256i*)(H + wiener_win * j + 0), sh_0);
            // Writing one more H on the top edge falls to the second row, so it
            // won't overflow.
            _mm256_storeu_si256((__m256i*)(H + wiener_win * j + 4), sh_1);
        } while (++j < wiener_win);

        // Step 2: Calculate the left edge of each square on the top row.
        j = 1;
        do {
            const int16_t* d_t                   = d;
            int32_t        height_t              = 0;
            __m256i        sum_h[WIENER_WIN - 1] = {_mm256_setzero_si256()};

            do {
                const int32_t h_t = ((height - height_t) < h_allowed) ? (height - height_t) : h_allowed;
                __m256i       row_h[WIENER_WIN - 1] = {_mm256_setzero_si256()};

                y = h_t;
                do {
                    x = 0;
                    while (x < w16) {
                        const __m256i dgd = _mm256_loadu_si256((__m256i*)(d_t + j + x));
                        stats_left_win7_avx2(dgd, d_t + x, d_stride, row_h);
                        x += 16;
                    };

                    if (w16 != width) {
                        const __m256i dgd      = _mm256_loadu_si256((__m256i*)(d_t + j + x));
                        const __m256i dgd_mask = _mm256_and_si256(dgd, mask);
                        stats_left_win7_avx2(dgd_mask, d_t + x, d_stride, row_h);
                    }

                    d_t += d_stride;
                } while (--y);

                add_32_to_64_avx2(row_h[0], &sum_h[0]);
                add_32_to_64_avx2(row_h[1], &sum_h[1]);
                add_32_to_64_avx2(row_h[2], &sum_h[2]);
                add_32_to_64_avx2(row_h[3], &sum_h[3]);
                add_32_to_64_avx2(row_h[4], &sum_h[4]);
                add_32_to_64_avx2(row_h[5], &sum_h[5]);

                height_t += h_t;
            } while (height_t < height);

            const __m256i sum0123 = hadd_four_64_avx2(sum_h[0], sum_h[1], sum_h[2], sum_h[3]);
            _mm_storel_epi64((__m128i*)&H[1 * wiener_win2 + j * wiener_win], _mm256_castsi256_si128(sum0123));
            _mm_storeh_epi64((__m128i*)&H[2 * wiener_win2 + j * wiener_win], _mm256_castsi256_si128(sum0123));
            _mm_storel_epi64((__m128i*)&H[3 * wiener_win2 + j * wiener_win], _mm256_extracti128_si256(sum0123, 1));
            _mm_storeh_epi64((__m128i*)&H[4 * wiener_win2 + j * wiener_win], _mm256_extracti128_si256(sum0123, 1));

            const __m128i sum45 = hadd_two_64_avx2(sum_h[4], sum_h[5]);
            _mm_storel_epi64((__m128i*)&H[5 * wiener_win2 + j * wiener_win], sum45);
            _mm_storeh_epi64((__m128i*)&H[6 * wiener_win2 + j * wiener_win], sum45);
        } while (++j < wiener_win);
    }

    // Step 3: Derive the top edge of each triangle along the diagonal. No
    // triangle in top row.
    {
        const int16_t* d_t = d;
        // Pad to call transpose function.
        __m256i deltas[WIENER_WIN + 1] = {_mm256_setzero_si256()};
        __m256i ds[WIENER_WIN];

        // 00s 00e 01s 01e 02s 02e 03s 03e  04s 04e 05s 05e 06s 06e 07s 07e
        // 10s 10e 11s 11e 12s 12e 13s 13e  14s 14e 15s 15e 16s 16e 17s 17e
        // 20s 20e 21s 21e 22s 22e 23s 23e  24s 24e 25s 25e 26s 26e 27s 27e
        // 30s 30e 31s 31e 32s 32e 33s 33e  34s 34e 35s 35e 36s 36e 37s 37e
        // 40s 40e 41s 41e 42s 42e 43s 43e  44s 44e 45s 45e 46s 46e 47s 47e
        // 50s 50e 51s 51e 52s 52e 53s 53e  54s 54e 55s 55e 56s 56e 57s 57e
        ds[0] = load_win7_avx2(d_t + 0 * d_stride, width);
        ds[1] = load_win7_avx2(d_t + 1 * d_stride, width);
        ds[2] = load_win7_avx2(d_t + 2 * d_stride, width);
        ds[3] = load_win7_avx2(d_t + 3 * d_stride, width);
        ds[4] = load_win7_avx2(d_t + 4 * d_stride, width);
        ds[5] = load_win7_avx2(d_t + 5 * d_stride, width);
        d_t += 6 * d_stride;

        if (bit_depth < EB_TWELVE_BIT) {
            step3_win7_avx2(&d_t, d_stride, width, height, ds, deltas);

            transpose_32bit_8x8_avx2(deltas, deltas);

            update_8_stats_avx2(H + 0 * wiener_win * wiener_win2 + 0 * wiener_win,
                                deltas[0],
                                H + 1 * wiener_win * wiener_win2 + 1 * wiener_win);
            update_8_stats_avx2(H + 1 * wiener_win * wiener_win2 + 1 * wiener_win,
                                deltas[1],
                                H + 2 * wiener_win * wiener_win2 + 2 * wiener_win);
            update_8_stats_avx2(H + 2 * wiener_win * wiener_win2 + 2 * wiener_win,
                                deltas[2],
                                H + 3 * wiener_win * wiener_win2 + 3 * wiener_win);
            update_8_stats_avx2(H + 3 * wiener_win * wiener_win2 + 3 * wiener_win,
                                deltas[3],
                                H + 4 * wiener_win * wiener_win2 + 4 * wiener_win);
            update_8_stats_avx2(H + 4 * wiener_win * wiener_win2 + 4 * wiener_win,
                                deltas[4],
                                H + 5 * wiener_win * wiener_win2 + 5 * wiener_win);
            update_8_stats_avx2(H + 5 * wiener_win * wiener_win2 + 5 * wiener_win,
                                deltas[5],
                                H + 6 * wiener_win * wiener_win2 + 6 * wiener_win);
        } else {
            __m128i deltas128[WIENER_WIN] = {_mm_setzero_si128()};
            int32_t height_t              = 0;

            do {
                __m256i       deltas_t[WIENER_WIN] = {_mm256_setzero_si256()};
                const int32_t h_t                  = ((height - height_t) < 128) ? (height - height_t) : 128;

                step3_win7_avx2(&d_t, d_stride, width, h_t, ds, deltas_t);

                add_six_32_to_64_avx2(deltas_t[0], &deltas[0], &deltas128[0]);
                add_six_32_to_64_avx2(deltas_t[1], &deltas[1], &deltas128[1]);
                add_six_32_to_64_avx2(deltas_t[2], &deltas[2], &deltas128[2]);
                add_six_32_to_64_avx2(deltas_t[3], &deltas[3], &deltas128[3]);
                add_six_32_to_64_avx2(deltas_t[4], &deltas[4], &deltas128[4]);
                add_six_32_to_64_avx2(deltas_t[5], &deltas[5], &deltas128[5]);
                add_six_32_to_64_avx2(deltas_t[6], &deltas[6], &deltas128[6]);

                height_t += h_t;
            } while (height_t < height);

            transpose_64bit_4x8_avx2(deltas, deltas);
            update_4_stats_highbd_avx2(H + 0 * wiener_win * wiener_win2 + 0 * wiener_win + 0,
                                       deltas[0],
                                       H + 1 * wiener_win * wiener_win2 + 1 * wiener_win + 0);
            update_4_stats_highbd_avx2(H + 0 * wiener_win * wiener_win2 + 0 * wiener_win + 4,
                                       deltas[1],
                                       H + 1 * wiener_win * wiener_win2 + 1 * wiener_win + 4);
            update_4_stats_highbd_avx2(H + 1 * wiener_win * wiener_win2 + 1 * wiener_win + 0,
                                       deltas[2],
                                       H + 2 * wiener_win * wiener_win2 + 2 * wiener_win + 0);
            update_4_stats_highbd_avx2(H + 1 * wiener_win * wiener_win2 + 1 * wiener_win + 4,
                                       deltas[3],
                                       H + 2 * wiener_win * wiener_win2 + 2 * wiener_win + 4);
            update_4_stats_highbd_avx2(H + 2 * wiener_win * wiener_win2 + 2 * wiener_win + 0,
                                       deltas[4],
                                       H + 3 * wiener_win * wiener_win2 + 3 * wiener_win + 0);
            update_4_stats_highbd_avx2(H + 2 * wiener_win * wiener_win2 + 2 * wiener_win + 4,
                                       deltas[5],
                                       H + 3 * wiener_win * wiener_win2 + 3 * wiener_win + 4);
            update_4_stats_highbd_avx2(H + 3 * wiener_win * wiener_win2 + 3 * wiener_win + 0,
                                       deltas[6],
                                       H + 4 * wiener_win * wiener_win2 + 4 * wiener_win + 0);
            update_4_stats_highbd_avx2(H + 3 * wiener_win * wiener_win2 + 3 * wiener_win + 4,
                                       deltas[7],
                                       H + 4 * wiener_win * wiener_win2 + 4 * wiener_win + 4);

            const __m128i d0 = _mm_unpacklo_epi64(deltas128[0], deltas128[1]);
            const __m128i d1 = _mm_unpacklo_epi64(deltas128[2], deltas128[3]);
            const __m128i d2 = _mm_unpacklo_epi64(deltas128[4], deltas128[5]);
            const __m128i d3 = _mm_unpacklo_epi64(deltas128[6], deltas128[6]);
            const __m128i d4 = _mm_unpackhi_epi64(deltas128[0], deltas128[1]);
            const __m128i d5 = _mm_unpackhi_epi64(deltas128[2], deltas128[3]);
            const __m128i d6 = _mm_unpackhi_epi64(deltas128[4], deltas128[5]);
            const __m128i d7 = _mm_unpackhi_epi64(deltas128[6], deltas128[6]);

            deltas[0] = _mm256_inserti128_si256(_mm256_castsi128_si256(d0), d1, 1);
            deltas[1] = _mm256_inserti128_si256(_mm256_castsi128_si256(d2), d3, 1);
            deltas[2] = _mm256_inserti128_si256(_mm256_castsi128_si256(d4), d5, 1);
            deltas[3] = _mm256_inserti128_si256(_mm256_castsi128_si256(d6), d7, 1);
            update_4_stats_highbd_avx2(H + 4 * wiener_win * wiener_win2 + 4 * wiener_win + 0,
                                       deltas[0],
                                       H + 5 * wiener_win * wiener_win2 + 5 * wiener_win + 0);
            update_4_stats_highbd_avx2(H + 4 * wiener_win * wiener_win2 + 4 * wiener_win + 4,
                                       deltas[1],
                                       H + 5 * wiener_win * wiener_win2 + 5 * wiener_win + 4);
            update_4_stats_highbd_avx2(H + 5 * wiener_win * wiener_win2 + 5 * wiener_win + 0,
                                       deltas[2],
                                       H + 6 * wiener_win * wiener_win2 + 6 * wiener_win + 0);
            update_4_stats_highbd_avx2(H + 5 * wiener_win * wiener_win2 + 5 * wiener_win + 4,
                                       deltas[3],
                                       H + 6 * wiener_win * wiener_win2 + 6 * wiener_win + 4);
        }
    }

    // Step 4: Derive the top and left edge of each square. No square in top and
    // bottom row.
    i = 1;
    do {
        j = i + 1;
        do {
            const int16_t* di                         = d + i - 1;
            const int16_t* d_j                        = d + j - 1;
            __m256i        deltas[2 * WIENER_WIN - 1] = {_mm256_setzero_si256()};
            __m256i        deltas_t[8], deltas_tt[4];

            deltas_tt[0] = _mm256_setzero_si256();
            deltas_tt[1] = _mm256_setzero_si256();
            deltas_tt[2] = _mm256_setzero_si256();
            deltas_tt[3] = _mm256_setzero_si256();

            __m256i dd[WIENER_WIN] = {_mm256_setzero_si256()}, ds[WIENER_WIN];
            dd[0]                  = _mm256_setzero_si256(); // Initialize to avoid warning.
            ds[0]                  = _mm256_setzero_si256(); // Initialize to avoid warning.

            dd[0] = _mm256_insert_epi16(dd[0], di[0 * d_stride], 0);
            dd[0] = _mm256_insert_epi16(dd[0], di[0 * d_stride + width], 8);
            dd[0] = _mm256_insert_epi16(dd[0], di[1 * d_stride], 1);
            dd[0] = _mm256_insert_epi16(dd[0], di[1 * d_stride + width], 9);
            dd[0] = _mm256_insert_epi16(dd[0], di[2 * d_stride], 2);
            dd[0] = _mm256_insert_epi16(dd[0], di[2 * d_stride + width], 10);
            dd[0] = _mm256_insert_epi16(dd[0], di[3 * d_stride], 3);
            dd[0] = _mm256_insert_epi16(dd[0], di[3 * d_stride + width], 11);
            dd[0] = _mm256_insert_epi16(dd[0], di[4 * d_stride], 4);
            dd[0] = _mm256_insert_epi16(dd[0], di[4 * d_stride + width], 12);
            dd[0] = _mm256_insert_epi16(dd[0], di[5 * d_stride], 5);
            dd[0] = _mm256_insert_epi16(dd[0], di[5 * d_stride + width], 13);

            ds[0] = _mm256_insert_epi16(ds[0], d_j[0 * d_stride], 0);
            ds[0] = _mm256_insert_epi16(ds[0], d_j[0 * d_stride + width], 8);
            ds[0] = _mm256_insert_epi16(ds[0], d_j[1 * d_stride], 1);
            ds[0] = _mm256_insert_epi16(ds[0], d_j[1 * d_stride + width], 9);
            ds[0] = _mm256_insert_epi16(ds[0], d_j[2 * d_stride], 2);
            ds[0] = _mm256_insert_epi16(ds[0], d_j[2 * d_stride + width], 10);
            ds[0] = _mm256_insert_epi16(ds[0], d_j[3 * d_stride], 3);
            ds[0] = _mm256_insert_epi16(ds[0], d_j[3 * d_stride + width], 11);
            ds[0] = _mm256_insert_epi16(ds[0], d_j[4 * d_stride], 4);
            ds[0] = _mm256_insert_epi16(ds[0], d_j[4 * d_stride + width], 12);
            ds[0] = _mm256_insert_epi16(ds[0], d_j[5 * d_stride], 5);
            ds[0] = _mm256_insert_epi16(ds[0], d_j[5 * d_stride + width], 13);

            y = 0;
            while (y < h8) {
                // 00s 10s 20s 30s 40s 50s 60s 70s  00e 10e 20e 30e 40e 50e 60e
                // 70e
                dd[0] = _mm256_insert_epi16(dd[0], di[6 * d_stride], 6);
                dd[0] = _mm256_insert_epi16(dd[0], di[6 * d_stride + width], 14);
                dd[0] = _mm256_insert_epi16(dd[0], di[7 * d_stride], 7);
                dd[0] = _mm256_insert_epi16(dd[0], di[7 * d_stride + width], 15);

                // 00s 10s 20s 30s 40s 50s 60s 70s  00e 10e 20e 30e 40e 50e 60e
                // 70e 01s 11s 21s 31s 41s 51s 61s 71s  01e 11e 21e 31e 41e 51e
                // 61e 71e
                ds[0] = _mm256_insert_epi16(ds[0], d_j[6 * d_stride], 6);
                ds[0] = _mm256_insert_epi16(ds[0], d_j[6 * d_stride + width], 14);
                ds[0] = _mm256_insert_epi16(ds[0], d_j[7 * d_stride], 7);
                ds[0] = _mm256_insert_epi16(ds[0], d_j[7 * d_stride + width], 15);

                load_more_16_avx2(di + 8 * d_stride, width, dd[0], &dd[1]);
                load_more_16_avx2(d_j + 8 * d_stride, width, ds[0], &ds[1]);
                load_more_16_avx2(di + 9 * d_stride, width, dd[1], &dd[2]);
                load_more_16_avx2(d_j + 9 * d_stride, width, ds[1], &ds[2]);
                load_more_16_avx2(di + 10 * d_stride, width, dd[2], &dd[3]);
                load_more_16_avx2(d_j + 10 * d_stride, width, ds[2], &ds[3]);
                load_more_16_avx2(di + 11 * d_stride, width, dd[3], &dd[4]);
                load_more_16_avx2(d_j + 11 * d_stride, width, ds[3], &ds[4]);
                load_more_16_avx2(di + 12 * d_stride, width, dd[4], &dd[5]);
                load_more_16_avx2(d_j + 12 * d_stride, width, ds[4], &ds[5]);
                load_more_16_avx2(di + 13 * d_stride, width, dd[5], &dd[6]);
                load_more_16_avx2(d_j + 13 * d_stride, width, ds[5], &ds[6]);

                madd_avx2(dd[0], ds[0], &deltas[0]);
                madd_avx2(dd[0], ds[1], &deltas[1]);
                madd_avx2(dd[0], ds[2], &deltas[2]);
                madd_avx2(dd[0], ds[3], &deltas[3]);
                madd_avx2(dd[0], ds[4], &deltas[4]);
                madd_avx2(dd[0], ds[5], &deltas[5]);
                madd_avx2(dd[0], ds[6], &deltas[6]);
                madd_avx2(dd[1], ds[0], &deltas[7]);
                madd_avx2(dd[2], ds[0], &deltas[8]);
                madd_avx2(dd[3], ds[0], &deltas[9]);
                madd_avx2(dd[4], ds[0], &deltas[10]);
                madd_avx2(dd[5], ds[0], &deltas[11]);
                madd_avx2(dd[6], ds[0], &deltas[12]);

                dd[0] = _mm256_srli_si256(dd[6], 4);
                ds[0] = _mm256_srli_si256(ds[6], 4);
                di += 8 * d_stride;
                d_j += 8 * d_stride;
                y += 8;
            };

            if (bit_depth < EB_TWELVE_BIT) {
                deltas[0]            = _mm256_hadd_epi32(deltas[0], deltas[1]);
                deltas[2]            = _mm256_hadd_epi32(deltas[2], deltas[3]);
                deltas[4]            = _mm256_hadd_epi32(deltas[4], deltas[5]);
                deltas[6]            = _mm256_hadd_epi32(deltas[6], deltas[6]);
                deltas[7]            = _mm256_hadd_epi32(deltas[7], deltas[8]);
                deltas[9]            = _mm256_hadd_epi32(deltas[9], deltas[10]);
                deltas[11]           = _mm256_hadd_epi32(deltas[11], deltas[12]);
                deltas[0]            = _mm256_hadd_epi32(deltas[0], deltas[2]);
                deltas[4]            = _mm256_hadd_epi32(deltas[4], deltas[6]);
                deltas[7]            = _mm256_hadd_epi32(deltas[7], deltas[9]);
                deltas[11]           = _mm256_hadd_epi32(deltas[11], deltas[11]);
                const __m128i delta0 = sub_hi_lo_32_avx2(deltas[0]);
                const __m128i delta1 = sub_hi_lo_32_avx2(deltas[4]);
                const __m128i delta2 = sub_hi_lo_32_avx2(deltas[7]);
                const __m128i delta3 = sub_hi_lo_32_avx2(deltas[11]);
                deltas[0]            = _mm256_inserti128_si256(_mm256_castsi128_si256(delta0), delta1, 1);
                deltas[1]            = _mm256_inserti128_si256(_mm256_castsi128_si256(delta2), delta3, 1);
            } else {
                deltas[0]  = hsub_32x8_to_64x4_avx2(deltas[0]);
                deltas[1]  = hsub_32x8_to_64x4_avx2(deltas[1]);
                deltas[2]  = hsub_32x8_to_64x4_avx2(deltas[2]);
                deltas[3]  = hsub_32x8_to_64x4_avx2(deltas[3]);
                deltas[4]  = hsub_32x8_to_64x4_avx2(deltas[4]);
                deltas[5]  = hsub_32x8_to_64x4_avx2(deltas[5]);
                deltas[6]  = hsub_32x8_to_64x4_avx2(deltas[6]);
                deltas[7]  = hsub_32x8_to_64x4_avx2(deltas[7]);
                deltas[8]  = hsub_32x8_to_64x4_avx2(deltas[8]);
                deltas[9]  = hsub_32x8_to_64x4_avx2(deltas[9]);
                deltas[10] = hsub_32x8_to_64x4_avx2(deltas[10]);
                deltas[11] = hsub_32x8_to_64x4_avx2(deltas[11]);
                deltas[12] = hsub_32x8_to_64x4_avx2(deltas[12]);

                transpose_64bit_4x8_avx2(deltas + 0, deltas_t);
                deltas_t[0]  = _mm256_add_epi64(deltas_t[0], deltas_t[2]);
                deltas_t[4]  = _mm256_add_epi64(deltas_t[4], deltas_t[6]);
                deltas_t[1]  = _mm256_add_epi64(deltas_t[1], deltas_t[3]);
                deltas_t[5]  = _mm256_add_epi64(deltas_t[5], deltas_t[7]);
                deltas_tt[0] = _mm256_add_epi64(deltas_t[0], deltas_t[4]);
                deltas_tt[1] = _mm256_add_epi64(deltas_t[1], deltas_t[5]);

                transpose_64bit_4x6_avx2(deltas + 7, deltas_t);
                deltas_t[0]  = _mm256_add_epi64(deltas_t[0], deltas_t[2]);
                deltas_t[4]  = _mm256_add_epi64(deltas_t[4], deltas_t[6]);
                deltas_t[1]  = _mm256_add_epi64(deltas_t[1], deltas_t[3]);
                deltas_t[5]  = _mm256_add_epi64(deltas_t[5], deltas_t[7]);
                deltas_tt[2] = _mm256_add_epi64(deltas_t[0], deltas_t[4]);
                deltas_tt[3] = _mm256_add_epi64(deltas_t[1], deltas_t[5]);

                deltas[0] = _mm256_setzero_si256();
                deltas[1] = _mm256_setzero_si256();
            }

            if (h8 != height) {
                const __m256i perm = _mm256_setr_epi32(1, 2, 3, 4, 5, 6, 7, 0);

                ds[0] = _mm256_insert_epi16(ds[0], d_j[0 * d_stride], 0);
                ds[0] = _mm256_insert_epi16(ds[0], d_j[0 * d_stride + width], 1);

                dd[2] = _mm256_insert_epi16(dd[2], -di[1 * d_stride], 0);
                dd[2] = _mm256_insert_epi16(dd[2], di[1 * d_stride + width], 1);
                ds[0] = _mm256_insert_epi16(ds[0], d_j[1 * d_stride], 2);
                ds[0] = _mm256_insert_epi16(ds[0], d_j[1 * d_stride + width], 3);

                dd[2] = _mm256_insert_epi16(dd[2], -di[2 * d_stride], 2);
                dd[2] = _mm256_insert_epi16(dd[2], di[2 * d_stride + width], 3);
                ds[0] = _mm256_insert_epi16(ds[0], d_j[2 * d_stride], 4);
                ds[0] = _mm256_insert_epi16(ds[0], d_j[2 * d_stride + width], 5);

                dd[2] = _mm256_insert_epi16(dd[2], -di[3 * d_stride], 4);
                dd[2] = _mm256_insert_epi16(dd[2], di[3 * d_stride + width], 5);
                ds[0] = _mm256_insert_epi16(ds[0], d_j[3 * d_stride], 6);
                ds[0] = _mm256_insert_epi16(ds[0], d_j[3 * d_stride + width], 7);

                dd[2] = _mm256_insert_epi16(dd[2], -di[4 * d_stride], 6);
                dd[2] = _mm256_insert_epi16(dd[2], di[4 * d_stride + width], 7);
                ds[0] = _mm256_insert_epi16(ds[0], d_j[4 * d_stride], 8);
                ds[0] = _mm256_insert_epi16(ds[0], d_j[4 * d_stride + width], 9);

                dd[2] = _mm256_insert_epi16(dd[2], -di[5 * d_stride], 8);
                dd[2] = _mm256_insert_epi16(dd[2], di[5 * d_stride + width], 9);
                ds[0] = _mm256_insert_epi16(ds[0], d_j[5 * d_stride], 10);
                ds[0] = _mm256_insert_epi16(ds[0], d_j[5 * d_stride + width], 11);

                do {
                    dd[0] = _mm256_set1_epi16(-di[0 * d_stride]);
                    dd[1] = _mm256_set1_epi16(di[0 * d_stride + width]);
                    dd[0] = _mm256_unpacklo_epi16(dd[0], dd[1]);

                    ds[2] = _mm256_set1_epi16(d_j[0 * d_stride]);
                    ds[3] = _mm256_set1_epi16(d_j[0 * d_stride + width]);
                    ds[2] = _mm256_unpacklo_epi16(ds[2], ds[3]);

                    dd[2] = _mm256_insert_epi16(dd[2], -di[6 * d_stride], 10);
                    dd[2] = _mm256_insert_epi16(dd[2], di[6 * d_stride + width], 11);
                    ds[0] = _mm256_insert_epi16(ds[0], d_j[6 * d_stride], 12);
                    ds[0] = _mm256_insert_epi16(ds[0], d_j[6 * d_stride + width], 13);

                    madd_avx2(dd[0], ds[0], &deltas[0]);
                    madd_avx2(dd[2], ds[2], &deltas[1]);

                    // right shift 4 bytes
                    dd[2] = _mm256_permutevar8x32_epi32(dd[2], perm);
                    ds[0] = _mm256_permutevar8x32_epi32(ds[0], perm);
                    di += d_stride;
                    d_j += d_stride;
                } while (++y < height);
            }

            // Writing one more H on the top edge of a square falls to the next
            // square in the same row or the first H in the next row, which
            // would be calculated later, so it won't overflow.
            if (bit_depth < EB_TWELVE_BIT) {
                update_8_stats_avx2(H + (i - 1) * wiener_win * wiener_win2 + (j - 1) * wiener_win,
                                    deltas[0],
                                    H + i * wiener_win * wiener_win2 + j * wiener_win);

                H[(i * wiener_win + 1) * wiener_win2 + j * wiener_win] =
                    H[((i - 1) * wiener_win + 1) * wiener_win2 + (j - 1) * wiener_win] +
                    _mm256_extract_epi32(deltas[1], 0);
                H[(i * wiener_win + 2) * wiener_win2 + j * wiener_win] =
                    H[((i - 1) * wiener_win + 2) * wiener_win2 + (j - 1) * wiener_win] +
                    _mm256_extract_epi32(deltas[1], 1);
                H[(i * wiener_win + 3) * wiener_win2 + j * wiener_win] =
                    H[((i - 1) * wiener_win + 3) * wiener_win2 + (j - 1) * wiener_win] +
                    _mm256_extract_epi32(deltas[1], 2);
                H[(i * wiener_win + 4) * wiener_win2 + j * wiener_win] =
                    H[((i - 1) * wiener_win + 4) * wiener_win2 + (j - 1) * wiener_win] +
                    _mm256_extract_epi32(deltas[1], 3);
                H[(i * wiener_win + 5) * wiener_win2 + j * wiener_win] =
                    H[((i - 1) * wiener_win + 5) * wiener_win2 + (j - 1) * wiener_win] +
                    _mm256_extract_epi32(deltas[1], 4);
                H[(i * wiener_win + 6) * wiener_win2 + j * wiener_win] =
                    H[((i - 1) * wiener_win + 6) * wiener_win2 + (j - 1) * wiener_win] +
                    _mm256_extract_epi32(deltas[1], 5);
            } else {
                const __m256i d0 = _mm256_cvtepi32_epi64(_mm256_castsi256_si128(deltas[0]));
                const __m256i d1 = _mm256_cvtepi32_epi64(_mm256_extracti128_si256(deltas[0], 1));
                const __m256i d2 = _mm256_cvtepi32_epi64(_mm256_castsi256_si128(deltas[1]));
                const __m256i d3 = _mm256_cvtepi32_epi64(_mm256_extracti128_si256(deltas[1], 1));

                deltas[0] = _mm256_add_epi64(deltas_tt[0], d0);
                deltas[1] = _mm256_add_epi64(deltas_tt[1], d1);
                deltas[2] = _mm256_add_epi64(deltas_tt[2], d2);
                deltas[3] = _mm256_add_epi64(deltas_tt[3], d3);

                update_4_stats_highbd_avx2(H + (i - 1) * wiener_win * wiener_win2 + (j - 1) * wiener_win + 0,
                                           deltas[0],
                                           H + i * wiener_win * wiener_win2 + j * wiener_win + 0);
                update_4_stats_highbd_avx2(H + (i - 1) * wiener_win * wiener_win2 + (j - 1) * wiener_win + 4,
                                           deltas[1],
                                           H + i * wiener_win * wiener_win2 + j * wiener_win + 4);

                H[(i * wiener_win + 1) * wiener_win2 + j * wiener_win] =
                    H[((i - 1) * wiener_win + 1) * wiener_win2 + (j - 1) * wiener_win] +
                    _mm256_extract_epi64(deltas[2], 0);
                H[(i * wiener_win + 2) * wiener_win2 + j * wiener_win] =
                    H[((i - 1) * wiener_win + 2) * wiener_win2 + (j - 1) * wiener_win] +
                    _mm256_extract_epi64(deltas[2], 1);
                H[(i * wiener_win + 3) * wiener_win2 + j * wiener_win] =
                    H[((i - 1) * wiener_win + 3) * wiener_win2 + (j - 1) * wiener_win] +
                    _mm256_extract_epi64(deltas[2], 2);
                H[(i * wiener_win + 4) * wiener_win2 + j * wiener_win] =
                    H[((i - 1) * wiener_win + 4) * wiener_win2 + (j - 1) * wiener_win] +
                    _mm256_extract_epi64(deltas[2], 3);
                H[(i * wiener_win + 5) * wiener_win2 + j * wiener_win] =
                    H[((i - 1) * wiener_win + 5) * wiener_win2 + (j - 1) * wiener_win] +
                    _mm256_extract_epi64(deltas[3], 0);
                H[(i * wiener_win + 6) * wiener_win2 + j * wiener_win] =
                    H[((i - 1) * wiener_win + 6) * wiener_win2 + (j - 1) * wiener_win] +
                    _mm256_extract_epi64(deltas[3], 1);
            }
        } while (++j < wiener_win);
    } while (++i < wiener_win - 1);

    // Step 5: Derive other points of each square. No square in bottom row.
    i = 0;
    do {
        const int16_t* const di = d + i;

        j = i + 1;
        do {
            const int16_t* const d_j                       = d + j;
            __m256i deltas[WIENER_WIN - 1][WIENER_WIN - 1] = {{_mm256_setzero_si256()}, {_mm256_setzero_si256()}};
            __m256i d_is[WIENER_WIN - 1], d_ie[WIENER_WIN - 1];
            __m256i d_js[WIENER_WIN - 1], d_je[WIENER_WIN - 1];

            x = 0;
            while (x < w16) {
                load_square_win7_avx2(di + x, d_j + x, d_stride, height, d_is, d_ie, d_js, d_je);
                derive_square_win7_avx2(d_is, d_ie, d_js, d_je, deltas);

                x += 16;
            };

            if (w16 != width) {
                load_square_win7_avx2(di + x, d_j + x, d_stride, height, d_is, d_ie, d_js, d_je);

                d_is[0] = _mm256_and_si256(d_is[0], mask);
                d_is[1] = _mm256_and_si256(d_is[1], mask);
                d_is[2] = _mm256_and_si256(d_is[2], mask);
                d_is[3] = _mm256_and_si256(d_is[3], mask);
                d_is[4] = _mm256_and_si256(d_is[4], mask);
                d_is[5] = _mm256_and_si256(d_is[5], mask);
                d_ie[0] = _mm256_and_si256(d_ie[0], mask);
                d_ie[1] = _mm256_and_si256(d_ie[1], mask);
                d_ie[2] = _mm256_and_si256(d_ie[2], mask);
                d_ie[3] = _mm256_and_si256(d_ie[3], mask);
                d_ie[4] = _mm256_and_si256(d_ie[4], mask);
                d_ie[5] = _mm256_and_si256(d_ie[5], mask);

                derive_square_win7_avx2(d_is, d_ie, d_js, d_je, deltas);
            }

            if (bit_depth < EB_TWELVE_BIT) {
                hadd_update_6_stats_avx2(H + (i * wiener_win + 0) * wiener_win2 + j * wiener_win,
                                         deltas[0],
                                         H + (i * wiener_win + 1) * wiener_win2 + j * wiener_win + 1);
                hadd_update_6_stats_avx2(H + (i * wiener_win + 1) * wiener_win2 + j * wiener_win,
                                         deltas[1],
                                         H + (i * wiener_win + 2) * wiener_win2 + j * wiener_win + 1);
                hadd_update_6_stats_avx2(H + (i * wiener_win + 2) * wiener_win2 + j * wiener_win,
                                         deltas[2],
                                         H + (i * wiener_win + 3) * wiener_win2 + j * wiener_win + 1);
                hadd_update_6_stats_avx2(H + (i * wiener_win + 3) * wiener_win2 + j * wiener_win,
                                         deltas[3],
                                         H + (i * wiener_win + 4) * wiener_win2 + j * wiener_win + 1);
                hadd_update_6_stats_avx2(H + (i * wiener_win + 4) * wiener_win2 + j * wiener_win,
                                         deltas[4],
                                         H + (i * wiener_win + 5) * wiener_win2 + j * wiener_win + 1);
                hadd_update_6_stats_avx2(H + (i * wiener_win + 5) * wiener_win2 + j * wiener_win,
                                         deltas[5],
                                         H + (i * wiener_win + 6) * wiener_win2 + j * wiener_win + 1);
            } else {
                hadd_update_6_stats_highbd_avx2(H + (i * wiener_win + 0) * wiener_win2 + j * wiener_win,
                                                deltas[0],
                                                H + (i * wiener_win + 1) * wiener_win2 + j * wiener_win + 1);
                hadd_update_6_stats_highbd_avx2(H + (i * wiener_win + 1) * wiener_win2 + j * wiener_win,
                                                deltas[1],
                                                H + (i * wiener_win + 2) * wiener_win2 + j * wiener_win + 1);
                hadd_update_6_stats_highbd_avx2(H + (i * wiener_win + 2) * wiener_win2 + j * wiener_win,
                                                deltas[2],
                                                H + (i * wiener_win + 3) * wiener_win2 + j * wiener_win + 1);
                hadd_update_6_stats_highbd_avx2(H + (i * wiener_win + 3) * wiener_win2 + j * wiener_win,
                                                deltas[3],
                                                H + (i * wiener_win + 4) * wiener_win2 + j * wiener_win + 1);
                hadd_update_6_stats_highbd_avx2(H + (i * wiener_win + 4) * wiener_win2 + j * wiener_win,
                                                deltas[4],
                                                H + (i * wiener_win + 5) * wiener_win2 + j * wiener_win + 1);
                hadd_update_6_stats_highbd_avx2(H + (i * wiener_win + 5) * wiener_win2 + j * wiener_win,
                                                deltas[5],
                                                H + (i * wiener_win + 6) * wiener_win2 + j * wiener_win + 1);
            }
        } while (++j < wiener_win);
    } while (++i < wiener_win - 1);

    // Step 6: Derive other points of each upper triangle along the diagonal.
    i = 0;
    do {
        const int16_t* const di                                        = d + i;
        __m256i              deltas[WIENER_WIN * (WIENER_WIN - 1) / 2] = {_mm256_setzero_si256()};
        __m256i              d_is[WIENER_WIN - 1], d_ie[WIENER_WIN - 1];

        x = 0;
        while (x < w16) {
            load_triangle_win7_avx2(di + x, d_stride, height, d_is, d_ie);
            derive_triangle_win7_avx2(d_is, d_ie, deltas);

            x += 16;
        };

        if (w16 != width) {
            load_triangle_win7_avx2(di + x, d_stride, height, d_is, d_ie);

            d_is[0] = _mm256_and_si256(d_is[0], mask);
            d_is[1] = _mm256_and_si256(d_is[1], mask);
            d_is[2] = _mm256_and_si256(d_is[2], mask);
            d_is[3] = _mm256_and_si256(d_is[3], mask);
            d_is[4] = _mm256_and_si256(d_is[4], mask);
            d_is[5] = _mm256_and_si256(d_is[5], mask);
            d_ie[0] = _mm256_and_si256(d_ie[0], mask);
            d_ie[1] = _mm256_and_si256(d_ie[1], mask);
            d_ie[2] = _mm256_and_si256(d_ie[2], mask);
            d_ie[3] = _mm256_and_si256(d_ie[3], mask);
            d_ie[4] = _mm256_and_si256(d_ie[4], mask);
            d_ie[5] = _mm256_and_si256(d_ie[5], mask);

            derive_triangle_win7_avx2(d_is, d_ie, deltas);
        }

        if (bit_depth < EB_TWELVE_BIT) {
            // Row 1: 6 points
            hadd_update_6_stats_avx2(H + (i * wiener_win + 0) * wiener_win2 + i * wiener_win,
                                     deltas,
                                     H + (i * wiener_win + 1) * wiener_win2 + i * wiener_win + 1);

            const __m128i delta0 = hadd_four_32_avx2(deltas[15], deltas[16], deltas[17], deltas[10]);
            const __m128i delta1 = hadd_four_32_avx2(deltas[18], deltas[19], deltas[20], deltas[20]);
            const __m128i delta2 = _mm_cvtepi32_epi64(delta0);
            const __m128i delta3 = _mm_cvtepi32_epi64(delta1);

            // Row 2: 5 points
            hadd_update_4_stats_avx2(H + (i * wiener_win + 1) * wiener_win2 + i * wiener_win + 1,
                                     deltas + 6,
                                     H + (i * wiener_win + 2) * wiener_win2 + i * wiener_win + 2);
            H[(i * wiener_win + 2) * wiener_win2 + i * wiener_win + 6] =
                H[(i * wiener_win + 1) * wiener_win2 + i * wiener_win + 5] + _mm_extract_epi32(delta0, 3);

            // Row 3: 4 points
            hadd_update_4_stats_avx2(H + (i * wiener_win + 2) * wiener_win2 + i * wiener_win + 2,
                                     deltas + 11,
                                     H + (i * wiener_win + 3) * wiener_win2 + i * wiener_win + 3);

            // Row 4: 3 points
            update_2_stats_sse2(H + (i * wiener_win + 3) * wiener_win2 + i * wiener_win + 3,
                                delta2,
                                H + (i * wiener_win + 4) * wiener_win2 + i * wiener_win + 4);
            H[(i * wiener_win + 4) * wiener_win2 + i * wiener_win + 6] =
                H[(i * wiener_win + 3) * wiener_win2 + i * wiener_win + 5] + _mm_extract_epi32(delta0, 2);

            // Row 5: 2 points
            update_2_stats_sse2(H + (i * wiener_win + 4) * wiener_win2 + i * wiener_win + 4,
                                delta3,
                                H + (i * wiener_win + 5) * wiener_win2 + i * wiener_win + 5);

            // Row 6: 1 points
            H[(i * wiener_win + 6) * wiener_win2 + i * wiener_win + 6] =
                H[(i * wiener_win + 5) * wiener_win2 + i * wiener_win + 5] + _mm_extract_epi32(delta1, 2);
        } else {
            // Row 1: 6 points
            hadd_update_6_stats_highbd_avx2(H + (i * wiener_win + 0) * wiener_win2 + i * wiener_win,
                                            deltas,
                                            H + (i * wiener_win + 1) * wiener_win2 + i * wiener_win + 1);

            const __m256i delta0 = hadd_four_31_to_64_avx2(deltas[15], deltas[16], deltas[17], deltas[10]);
            const __m256i delta1 = hadd_four_31_to_64_avx2(deltas[18], deltas[19], deltas[20], deltas[20]);

            // Row 2: 5 points
            hadd_update_4_stats_highbd_avx2(H + (i * wiener_win + 1) * wiener_win2 + i * wiener_win + 1,
                                            deltas + 6,
                                            H + (i * wiener_win + 2) * wiener_win2 + i * wiener_win + 2);
            H[(i * wiener_win + 2) * wiener_win2 + i * wiener_win + 6] =
                H[(i * wiener_win + 1) * wiener_win2 + i * wiener_win + 5] + _mm256_extract_epi64(delta0, 3);

            // Row 3: 4 points
            hadd_update_4_stats_highbd_avx2(H + (i * wiener_win + 2) * wiener_win2 + i * wiener_win + 2,
                                            deltas + 11,
                                            H + (i * wiener_win + 3) * wiener_win2 + i * wiener_win + 3);

            // Row 4: 3 points
            update_2_stats_sse2(H + (i * wiener_win + 3) * wiener_win2 + i * wiener_win + 3,
                                _mm256_castsi256_si128(delta0),
                                H + (i * wiener_win + 4) * wiener_win2 + i * wiener_win + 4);
            H[(i * wiener_win + 4) * wiener_win2 + i * wiener_win + 6] =
                H[(i * wiener_win + 3) * wiener_win2 + i * wiener_win + 5] + _mm256_extract_epi64(delta0, 2);

            // Row 5: 2 points
            update_2_stats_sse2(H + (i * wiener_win + 4) * wiener_win2 + i * wiener_win + 4,
                                _mm256_castsi256_si128(delta1),
                                H + (i * wiener_win + 5) * wiener_win2 + i * wiener_win + 5);

            // Row 6: 1 points
            H[(i * wiener_win + 6) * wiener_win2 + i * wiener_win + 6] =
                H[(i * wiener_win + 5) * wiener_win2 + i * wiener_win + 5] + _mm256_extract_epi64(delta1, 2);
        }
    } while (++i < wiener_win);
}

void svt_av1_compute_stats_avx2(int32_t wiener_win, const uint8_t* dgd, const uint8_t* src, int32_t h_start,
                                int32_t h_end, int32_t v_start, int32_t v_end, int32_t dgd_stride, int32_t src_stride,
                                int64_t* M, int64_t* H) {
    const int32_t wiener_win2    = wiener_win * wiener_win;
    const int32_t wiener_halfwin = wiener_win >> 1;
    const uint8_t avg            = find_average_avx2(dgd, h_start, h_end, v_start, v_end, dgd_stride);
    const int32_t width          = h_end - h_start;
    const int32_t height         = v_end - v_start;
    const int32_t d_stride       = (width + 2 * wiener_halfwin + 15) & ~15;
    const int32_t s_stride       = (width + 15) & ~15;
    int16_t *     d, *s;

    // The maximum input size is width * height, which is
    // (9 / 4) * RESTORATION_UNITSIZE_MAX * RESTORATION_UNITSIZE_MAX. Enlarge to
    // 3 * RESTORATION_UNITSIZE_MAX * RESTORATION_UNITSIZE_MAX considering
    // paddings.
    d = svt_aom_memalign(32, sizeof(*d) * 6 * RESTORATION_UNITSIZE_MAX * RESTORATION_UNITSIZE_MAX);
    s = d + 3 * RESTORATION_UNITSIZE_MAX * RESTORATION_UNITSIZE_MAX;

    sub_avg_block_avx2(src + v_start * src_stride + h_start, src_stride, avg, width, height, s, s_stride);
    sub_avg_block_avx2(dgd + (v_start - wiener_halfwin) * dgd_stride + h_start - wiener_halfwin,
                       dgd_stride,
                       avg,
                       width + 2 * wiener_halfwin,
                       height + 2 * wiener_halfwin,
                       d,
                       d_stride);

    if (wiener_win == WIENER_WIN) {
        compute_stats_win7_avx2(d, d_stride, s, s_stride, width, height, M, H, 8);
    } else {
        assert(wiener_win == WIENER_WIN_CHROMA);
        compute_stats_win5_avx2(d, d_stride, s, s_stride, width, height, M, H, 8);
    }

    // H is a symmetric matrix, so we only need to fill out the upper triangle.
    // We can copy it down to the lower triangle outside the (i, j) loops.
    diagonal_copy_stats_avx2(wiener_win2, H);

    svt_aom_free(d);
}

#if CONFIG_ENABLE_HIGH_BIT_DEPTH
void svt_av1_compute_stats_highbd_avx2(int32_t wiener_win, const uint8_t* dgd8, const uint8_t* src8, int32_t h_start,
                                       int32_t h_end, int32_t v_start, int32_t v_end, int32_t dgd_stride,
                                       int32_t src_stride, int64_t* M, int64_t* H, EbBitDepth bit_depth) {
    const int32_t   wiener_win2    = wiener_win * wiener_win;
    const int32_t   wiener_halfwin = (wiener_win >> 1);
    const uint16_t* src            = CONVERT_TO_SHORTPTR(src8);
    const uint16_t* dgd            = CONVERT_TO_SHORTPTR(dgd8);
    const uint16_t  avg      = find_average_highbd_avx2(dgd, h_start, h_end, v_start, v_end, dgd_stride, bit_depth);
    const int32_t   width    = h_end - h_start;
    const int32_t   height   = v_end - v_start;
    const int32_t   d_stride = (width + 2 * wiener_halfwin + 15) & ~15;
    const int32_t   s_stride = (width + 15) & ~15;
    int32_t         k;
    int16_t *       d, *s;

    // The maximum input size is width * height, which is
    // (9 / 4) * RESTORATION_UNITSIZE_MAX * RESTORATION_UNITSIZE_MAX. Enlarge to
    // 3 * RESTORATION_UNITSIZE_MAX * RESTORATION_UNITSIZE_MAX considering
    // paddings.
    d = svt_aom_memalign(32, sizeof(*d) * 6 * RESTORATION_UNITSIZE_MAX * RESTORATION_UNITSIZE_MAX);
    s = d + 3 * RESTORATION_UNITSIZE_MAX * RESTORATION_UNITSIZE_MAX;

    sub_avg_block_highbd_avx2(src + v_start * src_stride + h_start, src_stride, avg, width, height, s, s_stride);
    sub_avg_block_highbd_avx2(dgd + (v_start - wiener_halfwin) * dgd_stride + h_start - wiener_halfwin,
                              dgd_stride,
                              avg,
                              width + 2 * wiener_halfwin,
                              height + 2 * wiener_halfwin,
                              d,
                              d_stride);

    if (wiener_win == WIENER_WIN) {
        compute_stats_win7_avx2(d, d_stride, s, s_stride, width, height, M, H, bit_depth);
    } else {
        assert(wiener_win == WIENER_WIN_CHROMA);
        compute_stats_win5_avx2(d, d_stride, s, s_stride, width, height, M, H, bit_depth);
    }

    // H is a symmetric matrix, so we only need to fill out the upper triangle.
    // We can copy it down to the lower triangle outside the (i, j) loops.
    if (bit_depth == EB_EIGHT_BIT) {
        diagonal_copy_stats_avx2(wiener_win2, H);
    } else if (bit_depth == EB_TEN_BIT) {
        const int32_t k4 = wiener_win2 & ~3;

        k = 0;
        do {
            const __m256i dst = div4_avx2(_mm256_loadu_si256((__m256i*)(M + k)));
            _mm256_storeu_si256((__m256i*)(M + k), dst);
            H[k * wiener_win2 + k] /= 4;
            k += 4;
        } while (k < k4);

        H[k * wiener_win2 + k] /= 4;

        for (; k < wiener_win2; ++k) {
            M[k] /= 4;
        }

        div4_diagonal_copy_stats_avx2(wiener_win2, H);
    } else {
        const int32_t k4 = wiener_win2 & ~3;

        k = 0;
        do {
            const __m256i dst = div16_avx2(_mm256_loadu_si256((__m256i*)(M + k)));
            _mm256_storeu_si256((__m256i*)(M + k), dst);
            H[k * wiener_win2 + k] /= 16;
            k += 4;
        } while (k < k4);

        H[k * wiener_win2 + k] /= 16;

        for (; k < wiener_win2; ++k) {
            M[k] /= 16;
        }

        div16_diagonal_copy_stats_avx2(wiener_win2, H);
    }

    svt_aom_free(d);
}
#endif

static INLINE __m256i pair_set_epi16(int32_t a, int32_t b) {
    return _mm256_set1_epi32((int32_t)(((uint16_t)(a)) | (((uint32_t)(b)) << 16)));
}

int64_t svt_av1_lowbd_pixel_proj_error_avx2(const uint8_t* src8, int32_t width, int32_t height, int32_t src_stride,
                                            const uint8_t* dat8, int32_t dat_stride, int32_t* flt0, int32_t flt0_stride,
                                            int32_t* flt1, int32_t flt1_stride, const int32_t xq[2],
                                            const SgrParamsType* params) {
    const int32_t  shift = SGRPROJ_RST_BITS + SGRPROJ_PRJ_BITS;
    const uint8_t* src   = src8;
    const uint8_t* dat   = dat8;
    int64_t        err   = 0;
    int32_t        y     = height;
    int32_t        j;
    const __m256i  rounding = _mm256_set1_epi32(1 << (shift - 1));
    __m256i        sum64    = _mm256_setzero_si256();

    if (params->r[0] > 0 && params->r[1] > 0) {
        const __m256i xq_coeff = pair_set_epi16(xq[0], xq[1]);

        do {
            __m256i sum32 = _mm256_setzero_si256();

            for (j = 0; j <= width - 16; j += 16) {
                const __m256i d0       = _mm256_cvtepu8_epi16(xx_loadu_128(dat + j));
                const __m256i s0       = _mm256_cvtepu8_epi16(xx_loadu_128(src + j));
                const __m256i flt0_16b = _mm256_permute4x64_epi64(
                    _mm256_packs_epi32(yy_loadu_256(flt0 + j), yy_loadu_256(flt0 + j + 8)), 0xd8);
                const __m256i flt1_16b = _mm256_permute4x64_epi64(
                    _mm256_packs_epi32(yy_loadu_256(flt1 + j), yy_loadu_256(flt1 + j + 8)), 0xd8);
                const __m256i u0           = _mm256_slli_epi16(d0, SGRPROJ_RST_BITS);
                const __m256i flt0_0_sub_u = _mm256_sub_epi16(flt0_16b, u0);
                const __m256i flt1_0_sub_u = _mm256_sub_epi16(flt1_16b, u0);
                const __m256i v0   = _mm256_madd_epi16(xq_coeff, _mm256_unpacklo_epi16(flt0_0_sub_u, flt1_0_sub_u));
                const __m256i v1   = _mm256_madd_epi16(xq_coeff, _mm256_unpackhi_epi16(flt0_0_sub_u, flt1_0_sub_u));
                const __m256i vr0  = _mm256_srai_epi32(_mm256_add_epi32(v0, rounding), shift);
                const __m256i vr1  = _mm256_srai_epi32(_mm256_add_epi32(v1, rounding), shift);
                const __m256i e0   = _mm256_sub_epi16(_mm256_add_epi16(_mm256_packs_epi32(vr0, vr1), d0), s0);
                const __m256i err0 = _mm256_madd_epi16(e0, e0);
                sum32              = _mm256_add_epi32(sum32, err0);
            }

            for (; j < width; ++j) {
                const int32_t u = (int32_t)(dat[j] << SGRPROJ_RST_BITS);
                int32_t       v = xq[0] * (flt0[j] - u) + xq[1] * (flt1[j] - u);
                const int32_t e = ROUND_POWER_OF_TWO(v, shift) + dat[j] - src[j];
                err += e * e;
            }

            dat += dat_stride;
            src += src_stride;
            flt0 += flt0_stride;
            flt1 += flt1_stride;
            const __m256i sum64_0 = _mm256_cvtepi32_epi64(_mm256_castsi256_si128(sum32));
            const __m256i sum64_1 = _mm256_cvtepi32_epi64(_mm256_extracti128_si256(sum32, 1));
            sum64                 = _mm256_add_epi64(sum64, sum64_0);
            sum64                 = _mm256_add_epi64(sum64, sum64_1);
        } while (--y);
    } else if (params->r[0] > 0 || params->r[1] > 0) {
        const int32_t  xq_active  = (params->r[0] > 0) ? xq[0] : xq[1];
        const __m256i  xq_coeff   = pair_set_epi16(xq_active, (-xq_active * (1 << SGRPROJ_RST_BITS)));
        const int32_t* flt        = (params->r[0] > 0) ? flt0 : flt1;
        const int32_t  flt_stride = (params->r[0] > 0) ? flt0_stride : flt1_stride;

        do {
            __m256i sum32 = _mm256_setzero_si256();

            for (j = 0; j <= width - 16; j += 16) {
                const __m256i d0      = _mm256_cvtepu8_epi16(xx_loadu_128(dat + j));
                const __m256i s0      = _mm256_cvtepu8_epi16(xx_loadu_128(src + j));
                const __m256i flt_16b = _mm256_permute4x64_epi64(
                    _mm256_packs_epi32(yy_loadu_256(flt + j), yy_loadu_256(flt + j + 8)), 0xd8);
                const __m256i v0   = _mm256_madd_epi16(xq_coeff, _mm256_unpacklo_epi16(flt_16b, d0));
                const __m256i v1   = _mm256_madd_epi16(xq_coeff, _mm256_unpackhi_epi16(flt_16b, d0));
                const __m256i vr0  = _mm256_srai_epi32(_mm256_add_epi32(v0, rounding), shift);
                const __m256i vr1  = _mm256_srai_epi32(_mm256_add_epi32(v1, rounding), shift);
                const __m256i e0   = _mm256_sub_epi16(_mm256_add_epi16(_mm256_packs_epi32(vr0, vr1), d0), s0);
                const __m256i err0 = _mm256_madd_epi16(e0, e0);
                sum32              = _mm256_add_epi32(sum32, err0);
            }

            for (; j < width; ++j) {
                const int32_t u = (int32_t)(dat[j] << SGRPROJ_RST_BITS);
                int32_t       v = xq_active * (flt[j] - u);
                const int32_t e = ROUND_POWER_OF_TWO(v, shift) + dat[j] - src[j];
                err += e * e;
            }

            dat += dat_stride;
            src += src_stride;
            flt += flt_stride;
            const __m256i sum64_0 = _mm256_cvtepi32_epi64(_mm256_castsi256_si128(sum32));
            const __m256i sum64_1 = _mm256_cvtepi32_epi64(_mm256_extracti128_si256(sum32, 1));
            sum64                 = _mm256_add_epi64(sum64, sum64_0);
            sum64                 = _mm256_add_epi64(sum64, sum64_1);
        } while (--y);
    } else {
        __m256i sum32 = _mm256_setzero_si256();

        do {
            for (j = 0; j <= width - 16; j += 16) {
                const __m256i d0    = _mm256_cvtepu8_epi16(xx_loadu_128(dat + j));
                const __m256i s0    = _mm256_cvtepu8_epi16(xx_loadu_128(src + j));
                const __m256i diff0 = _mm256_sub_epi16(d0, s0);
                const __m256i err0  = _mm256_madd_epi16(diff0, diff0);
                sum32               = _mm256_add_epi32(sum32, err0);
            }

            for (; j < width; ++j) {
                const int32_t e = (int32_t)(dat[j]) - src[j];
                err += e * e;
            }

            dat += dat_stride;
            src += src_stride;
        } while (--y);

        const __m256i sum64_0 = _mm256_cvtepi32_epi64(_mm256_castsi256_si128(sum32));
        const __m256i sum64_1 = _mm256_cvtepi32_epi64(_mm256_extracti128_si256(sum32, 1));
        sum64                 = _mm256_add_epi64(sum64_0, sum64_1);
    }

    return err + _mm_cvtsi128_si64(hadd_64_avx2(sum64));
}

int64_t svt_av1_highbd_pixel_proj_error_avx2(const uint8_t* src8, int32_t width, int32_t height, int32_t src_stride,
                                             const uint8_t* dat8, int32_t dat_stride, int32_t* flt0,
                                             int32_t flt0_stride, int32_t* flt1, int32_t flt1_stride,
                                             const int32_t xq[2], const SgrParamsType* params) {
    int32_t         i, j, k;
    const int32_t   shift    = SGRPROJ_RST_BITS + SGRPROJ_PRJ_BITS;
    const __m256i   rounding = _mm256_set1_epi32(1 << (shift - 1));
    __m256i         sum64    = _mm256_setzero_si256();
    const uint16_t* src      = CONVERT_TO_SHORTPTR(src8);
    const uint16_t* dat      = CONVERT_TO_SHORTPTR(dat8);
    int64_t         err      = 0;
    if (params->r[0] > 0 && params->r[1] > 0) { // Both filters are enabled
        const __m256i xq0 = _mm256_set1_epi32(xq[0]);
        const __m256i xq1 = _mm256_set1_epi32(xq[1]);
        for (i = 0; i < height; ++i) {
            __m256i sum32 = _mm256_setzero_si256();
            for (j = 0; j <= width - 16; j += 16) { // Process 16 pixels at a time
                // Load 16 pixels each from source image and corrupted image
                const __m256i s0 = yy_loadu_256(src + j);
                const __m256i d0 = yy_loadu_256(dat + j);
                // s0 = [15 14 13 12 11 10 9 8] [7 6 5 4 3 2 1 0] as u16 (indices)

                // Shift-up each pixel to match filtered image scaling
                const __m256i u0 = _mm256_slli_epi16(d0, SGRPROJ_RST_BITS);

                // Split u0 into two halves and pad each from u16 to i32
                const __m256i u0l = _mm256_cvtepu16_epi32(_mm256_castsi256_si128(u0));
                const __m256i u0h = _mm256_cvtepu16_epi32(_mm256_extracti128_si256(u0, 1));
                // u0h, u0l = [15 14 13 12] [11 10 9 8], [7 6 5 4] [3 2 1 0] as u32

                // Load 16 pixels from each filtered image
                const __m256i flt0l = yy_loadu_256(flt0 + j);
                const __m256i flt0h = yy_loadu_256(flt0 + j + 8);
                const __m256i flt1l = yy_loadu_256(flt1 + j);
                const __m256i flt1h = yy_loadu_256(flt1 + j + 8);
                // flt?l, flt?h = [15 14 13 12] [11 10 9 8], [7 6 5 4] [3 2 1 0] as u32

                // Subtract shifted corrupt image from each filtered image
                const __m256i flt0l_subu = _mm256_sub_epi32(flt0l, u0l);
                const __m256i flt0h_subu = _mm256_sub_epi32(flt0h, u0h);
                const __m256i flt1l_subu = _mm256_sub_epi32(flt1l, u0l);
                const __m256i flt1h_subu = _mm256_sub_epi32(flt1h, u0h);

                // Multiply basis vectors by appropriate coefficients
                const __m256i v0l = _mm256_mullo_epi32(flt0l_subu, xq0);
                const __m256i v0h = _mm256_mullo_epi32(flt0h_subu, xq0);
                const __m256i v1l = _mm256_mullo_epi32(flt1l_subu, xq1);
                const __m256i v1h = _mm256_mullo_epi32(flt1h_subu, xq1);

                // Add together the contributions from the two basis vectors
                const __m256i vl = _mm256_add_epi32(v0l, v1l);
                const __m256i vh = _mm256_add_epi32(v0h, v1h);

                // Right-shift v with appropriate rounding
                const __m256i vrl = _mm256_srai_epi32(_mm256_add_epi32(vl, rounding), shift);
                const __m256i vrh = _mm256_srai_epi32(_mm256_add_epi32(vh, rounding), shift);
                // vrh, vrl = [15 14 13 12] [11 10 9 8], [7 6 5 4] [3 2 1 0]

                // Saturate each i32 to an i16 then combine both halves
                // The permute (control=[3 1 2 0]) fixes weird ordering from AVX lanes
                const __m256i vr = _mm256_permute4x64_epi64(_mm256_packs_epi32(vrl, vrh), 0xd8);
                // intermediate = [15 14 13 12 7 6 5 4] [11 10 9 8 3 2 1 0]
                // vr = [15 14 13 12 11 10 9 8] [7 6 5 4 3 2 1 0]

                // Add twin-subspace-sgr-filter to corrupt image then subtract source
                const __m256i e0 = _mm256_sub_epi16(_mm256_add_epi16(vr, d0), s0);

                // Calculate squared error and add adjacent values
                const __m256i err0 = _mm256_madd_epi16(e0, e0);

                sum32 = _mm256_add_epi32(sum32, err0);
            }

            const __m256i sum32l = _mm256_cvtepu32_epi64(_mm256_castsi256_si128(sum32));
            sum64                = _mm256_add_epi64(sum64, sum32l);
            const __m256i sum32h = _mm256_cvtepu32_epi64(_mm256_extracti128_si256(sum32, 1));
            sum64                = _mm256_add_epi64(sum64, sum32h);

            // Process remaining pixels in this row (modulo 16)
            for (k = j; k < width; ++k) {
                const int32_t u = (int32_t)(dat[k] << SGRPROJ_RST_BITS);
                int32_t       v = xq[0] * (flt0[k] - u) + xq[1] * (flt1[k] - u);
                const int32_t e = ROUND_POWER_OF_TWO(v, shift) + dat[k] - src[k];
                err += e * e;
            }
            dat += dat_stride;
            src += src_stride;
            flt0 += flt0_stride;
            flt1 += flt1_stride;
        }
    } else if (params->r[0] > 0 || params->r[1] > 0) { // Only one filter enabled
        const int32_t  xq_on       = (params->r[0] > 0) ? xq[0] : xq[1];
        const __m256i  xq_active   = _mm256_set1_epi32(xq_on);
        const __m256i  xq_inactive = _mm256_set1_epi32(-xq_on * (1 << SGRPROJ_RST_BITS));
        const int32_t* flt         = (params->r[0] > 0) ? flt0 : flt1;
        const int32_t  flt_stride  = (params->r[0] > 0) ? flt0_stride : flt1_stride;
        for (i = 0; i < height; ++i) {
            __m256i sum32 = _mm256_setzero_si256();
            for (j = 0; j <= width - 16; j += 16) {
                // Load 16 pixels from source image
                const __m256i s0 = yy_loadu_256(src + j);
                // s0 = [15 14 13 12 11 10 9 8] [7 6 5 4 3 2 1 0] as u16

                // Load 16 pixels from corrupted image and pad each u16 to i32
                const __m256i d0  = yy_loadu_256(dat + j);
                const __m256i d0h = _mm256_cvtepu16_epi32(_mm256_extracti128_si256(d0, 1));
                const __m256i d0l = _mm256_cvtepu16_epi32(_mm256_castsi256_si128(d0));
                // d0 = [15 14 13 12 11 10 9 8] [7 6 5 4 3 2 1 0] as u16
                // d0h, d0l = [15 14 13 12] [11 10 9 8], [7 6 5 4] [3 2 1 0] as i32

                // Load 16 pixels from the filtered image
                const __m256i flth = yy_loadu_256(flt + j + 8);
                const __m256i fltl = yy_loadu_256(flt + j);
                // flth, fltl = [15 14 13 12] [11 10 9 8], [7 6 5 4] [3 2 1 0] as i32

                const __m256i flth_xq = _mm256_mullo_epi32(flth, xq_active);
                const __m256i fltl_xq = _mm256_mullo_epi32(fltl, xq_active);
                const __m256i d0h_xq  = _mm256_mullo_epi32(d0h, xq_inactive);
                const __m256i d0l_xq  = _mm256_mullo_epi32(d0l, xq_inactive);

                const __m256i vh = _mm256_add_epi32(flth_xq, d0h_xq);
                const __m256i vl = _mm256_add_epi32(fltl_xq, d0l_xq);

                // Shift this down with appropriate rounding
                const __m256i vrh = _mm256_srai_epi32(_mm256_add_epi32(vh, rounding), shift);
                const __m256i vrl = _mm256_srai_epi32(_mm256_add_epi32(vl, rounding), shift);
                // vrh, vrl = [15 14 13 12] [11 10 9 8], [7 6 5 4] [3 2 1 0] as i32

                // Saturate each i32 to an i16 then combine both halves
                // The permute (control=[3 1 2 0]) fixes weird ordering from AVX lanes
                const __m256i vr = _mm256_permute4x64_epi64(_mm256_packs_epi32(vrl, vrh), 0xd8);
                // intermediate = [15 14 13 12 7 6 5 4] [11 10 9 8 3 2 1 0] as u16
                // vr = [15 14 13 12 11 10 9 8] [7 6 5 4 3 2 1 0] as u16

                // Subtract twin-subspace-sgr filtered from source image to get error
                const __m256i e0 = _mm256_sub_epi16(_mm256_add_epi16(vr, d0), s0);

                // Calculate squared error and add adjacent values
                const __m256i err0 = _mm256_madd_epi16(e0, e0);

                sum32 = _mm256_add_epi32(sum32, err0);
            }

            const __m256i sum32l = _mm256_cvtepu32_epi64(_mm256_castsi256_si128(sum32));
            sum64                = _mm256_add_epi64(sum64, sum32l);
            const __m256i sum32h = _mm256_cvtepu32_epi64(_mm256_extracti128_si256(sum32, 1));
            sum64                = _mm256_add_epi64(sum64, sum32h);

            // Process remaining pixels in this row (modulo 16)
            for (k = j; k < width; ++k) {
                const int32_t u = (int32_t)(dat[k] << SGRPROJ_RST_BITS);
                int32_t       v = xq_on * (flt[k] - u);
                const int32_t e = ROUND_POWER_OF_TWO(v, shift) + dat[k] - src[k];
                err += e * e;
            }
            dat += dat_stride;
            src += src_stride;
            flt += flt_stride;
        }
    } else { // Neither filter is enabled
        for (i = 0; i < height; ++i) {
            __m256i sum32 = _mm256_setzero_si256();
            for (j = 0; j <= width - 32; j += 32) {
                // Load 2x16 u16 from source image
                const __m256i s0l = yy_loadu_256(src + j);
                const __m256i s0h = yy_loadu_256(src + j + 16);

                // Load 2x16 u16 from corrupted image
                const __m256i d0l = yy_loadu_256(dat + j);
                const __m256i d0h = yy_loadu_256(dat + j + 16);

                // Subtract corrupted image from source image
                const __m256i diffl = _mm256_sub_epi16(d0l, s0l);
                const __m256i diffh = _mm256_sub_epi16(d0h, s0h);

                // Square error and add adjacent values
                const __m256i err0l = _mm256_madd_epi16(diffl, diffl);
                const __m256i err0h = _mm256_madd_epi16(diffh, diffh);

                sum32 = _mm256_add_epi32(sum32, err0l);
                sum32 = _mm256_add_epi32(sum32, err0h);
            }

            const __m256i sum32l = _mm256_cvtepu32_epi64(_mm256_castsi256_si128(sum32));
            sum64                = _mm256_add_epi64(sum64, sum32l);
            const __m256i sum32h = _mm256_cvtepu32_epi64(_mm256_extracti128_si256(sum32, 1));
            sum64                = _mm256_add_epi64(sum64, sum32h);

            // Process remaining pixels (modulu 16)
            for (k = j; k < width; ++k) {
                const int32_t e = (int32_t)(dat[k]) - src[k];
                err += e * e;
            }
            dat += dat_stride;
            src += src_stride;
        }
    }

    // Sum 4 values from sum64l and sum64h into err
    int64_t sum[4];
    yy_storeu_256(sum, sum64);
    err += sum[0] + sum[1] + sum[2] + sum[3];
    return err;
}
