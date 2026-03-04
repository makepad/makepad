/*
 * Copyright(c) 2022 Intel Corporation
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
 * Media Patent License 1.0 was not distributed with this source code in the
 * PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
 */

#include <emmintrin.h>
#include <smmintrin.h>

#include "aom_dsp_rtcd.h"
#include "transpose_sse2.h"
#include "restoration.h"
#include "utility.h"
#include "picture_operators_sse2.h"

#define WIN_CHROMA ((WIENER_WIN_CHROMA - 1) * 2)
#define WIN_7 ((WIENER_WIN - 1) * 2)

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

static INLINE void add_32_to_64_sse4_1(const __m128i src, __m128i* const sum) {
    const __m128i s0 = _mm_cvtepi32_epi64(src);
    const __m128i s1 = _mm_cvtepi32_epi64(_mm_srli_si128(src, 8));
    *sum             = _mm_add_epi64(*sum, s0);
    *sum             = _mm_add_epi64(*sum, s1);
}

#if CONFIG_ENABLE_HIGH_BIT_DEPTH
static INLINE void add_u16_to_u32_sse4_1(const __m128i src, __m128i* const sum) {
    const __m128i s0 = _mm_unpacklo_epi16(src, _mm_setzero_si128());
    const __m128i s1 = _mm_unpackhi_epi16(src, _mm_setzero_si128());
    *sum             = _mm_add_epi32(*sum, s0);
    *sum             = _mm_add_epi32(*sum, s1);
}
#endif

static uint8_t find_average_sse4_1(const uint8_t* src, int32_t h_start, int32_t h_end, int32_t v_start, int32_t v_end,
                                   int32_t stride) {
    const int32_t  width    = h_end - h_start;
    const int32_t  height   = v_end - v_start;
    const uint8_t* src_t    = src + v_start * stride + h_start;
    const int32_t  leftover = width & 15;
    int32_t        i        = height;
    __m128i        ss       = _mm_setzero_si128();

    if (!leftover) {
        do {
            int32_t j = 0;
            do {
                const __m128i s   = _mm_loadu_si128((__m128i*)(src_t + j));
                const __m128i sad = _mm_sad_epu8(s, _mm_setzero_si128());
                ss                = _mm_add_epi32(ss, sad);
                j += 16;
            } while (j < width);

            src_t += stride;
        } while (--i);
    } else {
        const int32_t w16 = width - leftover;

        const __m128i mask = _mm_loadu_si128((__m128i*)(mask_8bit[leftover]));

        do {
            int32_t j = 0;
            while (j < w16) {
                const __m128i s   = _mm_loadu_si128((__m128i*)(src_t + j));
                const __m128i sad = _mm_sad_epu8(s, _mm_setzero_si128());
                ss                = _mm_add_epi32(ss, sad);
                j += 16;
            };

            const __m128i s   = _mm_loadu_si128((__m128i*)(src_t + j));
            const __m128i s_t = _mm_and_si128(s, mask);
            const __m128i sad = _mm_sad_epu8(s_t, _mm_setzero_si128());
            ss                = _mm_add_epi32(ss, sad);
            src_t += stride;
        } while (--i);
    }

    const uint32_t sum = hadd32_sse2_intrin(ss);
    const uint32_t avg = sum / (width * height);
    return (uint8_t)avg;
}

#if CONFIG_ENABLE_HIGH_BIT_DEPTH
static uint16_t find_average_highbd_sse4_1(const uint16_t* src, int32_t h_start, int32_t h_end, int32_t v_start,
                                           int32_t v_end, int32_t stride, EbBitDepth bit_depth) {
    UNUSED(bit_depth);
    const int32_t   width    = h_end - h_start;
    const int32_t   height   = v_end - v_start;
    const uint16_t* src_t    = src + v_start * stride + h_start;
    const int32_t   leftover = width & 7;
    int32_t         i        = height;
    __m128i         sss      = _mm_setzero_si128();

    assert(bit_depth <= 10);
    if (width <= 256) {
        if (!leftover) {
            do {
                __m128i ss = _mm_setzero_si128();

                int32_t j = 0;
                do {
                    const __m128i s = _mm_loadu_si128((__m128i*)(src_t + j));
                    ss              = _mm_add_epi16(ss, s);
                    j += 8;
                } while (j < width);

                add_u16_to_u32_sse4_1(ss, &sss);

                src_t += stride;
            } while (--i);
        } else {
            const int32_t w8   = width - leftover;
            const __m128i mask = _mm_loadu_si128((__m128i*)(mask_16bit[leftover]));

            do {
                __m128i ss = _mm_setzero_si128();

                int32_t j = 0;
                while (j < w8) {
                    const __m128i s = _mm_loadu_si128((__m128i*)(src_t + j));
                    ss              = _mm_add_epi16(ss, s);
                    j += 8;
                };

                const __m128i s   = _mm_loadu_si128((__m128i*)(src_t + j));
                const __m128i s_t = _mm_and_si128(s, mask);
                ss                = _mm_add_epi16(ss, s_t);

                add_u16_to_u32_sse4_1(ss, &sss);

                src_t += stride;
            } while (--i);
        }
    } else {
        if (!leftover) {
            do {
                __m128i ss = _mm_setzero_si128();

                int32_t j = 0;
                do {
                    const __m128i s = _mm_loadu_si128((__m128i*)(src_t + j));
                    ss              = _mm_add_epi16(ss, s);
                    j += 8;
                } while (j < 256);

                add_u16_to_u32_sse4_1(ss, &sss);
                ss = _mm_setzero_si128();

                do {
                    const __m128i s = _mm_loadu_si128((__m128i*)(src_t + j));
                    ss              = _mm_add_epi16(ss, s);
                    j += 8;
                } while (j < width);

                add_u16_to_u32_sse4_1(ss, &sss);

                src_t += stride;
            } while (--i);
        } else {
            const int32_t w8   = width - leftover;
            const __m128i mask = _mm_loadu_si128((__m128i*)(mask_16bit[leftover]));

            do {
                __m128i ss = _mm_setzero_si128();

                int32_t j = 0;
                while (j < 256) {
                    const __m128i s = _mm_loadu_si128((__m128i*)(src_t + j));
                    ss              = _mm_add_epi16(ss, s);
                    j += 8;
                };

                add_u16_to_u32_sse4_1(ss, &sss);
                ss = _mm_setzero_si128();

                while (j < w8) {
                    const __m128i s = _mm_loadu_si128((__m128i*)(src_t + j));
                    ss              = _mm_add_epi16(ss, s);
                    j += 8;
                }

                const __m128i s   = _mm_loadu_si128((__m128i*)(src_t + j));
                const __m128i s_t = _mm_and_si128(s, mask);
                ss                = _mm_add_epi16(ss, s_t);

                add_u16_to_u32_sse4_1(ss, &sss);

                src_t += stride;
            } while (--i);
        }
    }

    const uint32_t sum = hadd32_sse2_intrin(sss);
    const uint32_t avg = sum / (width * height);
    return (uint16_t)avg;
}
#endif

static void sub_avg_block_sse4_1(const uint8_t* src, const int32_t src_stride, const uint8_t avg, const int32_t width,
                                 const int32_t height, int16_t* dst, const int32_t dst_stride) {
    const __m128i a = _mm_set1_epi16(avg);

    int32_t i = height + 1;
    do {
        int32_t j = 0;
        while (j < width) {
            const __m128i s   = _mm_loadu_si128((__m128i*)(src + j));
            __m128i       ss1 = _mm_unpacklo_epi8(s, _mm_setzero_si128());
            __m128i       ss2 = _mm_unpackhi_epi8(s, _mm_setzero_si128());

            ss1 = _mm_subs_epi16(ss1, a);
            ss2 = _mm_subs_epi16(ss2, a);

            _mm_storeu_si128((__m128i*)(dst + j), ss1);
            _mm_storeu_si128((__m128i*)(dst + j + 8), ss2);
            j += 16;
        };

        src += src_stride;
        dst += dst_stride;
    } while (--i);
}

#if CONFIG_ENABLE_HIGH_BIT_DEPTH
static void sub_avg_block_highbd_sse4_1(const uint16_t* src, const int32_t src_stride, const uint16_t avg,
                                        const int32_t width, const int32_t height, int16_t* dst,
                                        const int32_t dst_stride) {
    const __m128i a = _mm_set1_epi16(avg);

    int32_t i = height + 1;
    do {
        int32_t j = 0;
        while (j < width) {
            const __m128i s = _mm_loadu_si128((__m128i*)(src + j));
            const __m128i d = _mm_sub_epi16(s, a);
            _mm_storeu_si128((__m128i*)(dst + j), d);
            j += 8;
        };

        src += src_stride;
        dst += dst_stride;
    } while (--i);
}
#endif

static void diagonal_copy_stats_sse4_1(const int32_t wiener_win2, int64_t* const H) {
    for (int32_t i = 0; i < wiener_win2 - 1; i += 4) {
        __m128i in[8], out[8];

        in[0] = _mm_loadu_si128((__m128i*)(H + (i + 0) * wiener_win2 + i + 1));
        in[1] = _mm_loadu_si128((__m128i*)(H + (i + 0) * wiener_win2 + i + 3));
        in[2] = _mm_loadu_si128((__m128i*)(H + (i + 1) * wiener_win2 + i + 1));
        in[3] = _mm_loadu_si128((__m128i*)(H + (i + 1) * wiener_win2 + i + 3));
        in[4] = _mm_loadu_si128((__m128i*)(H + (i + 2) * wiener_win2 + i + 1));
        in[5] = _mm_loadu_si128((__m128i*)(H + (i + 2) * wiener_win2 + i + 3));
        in[6] = _mm_loadu_si128((__m128i*)(H + (i + 3) * wiener_win2 + i + 1));
        in[7] = _mm_loadu_si128((__m128i*)(H + (i + 3) * wiener_win2 + i + 3));

        transpose_64bit_4x4_sse2(in, out);

        _mm_storel_epi64((__m128i*)(H + (i + 1) * wiener_win2 + i), out[0]);
        _mm_storeu_si128((__m128i*)(H + (i + 2) * wiener_win2 + i), out[2]);
        _mm_storeu_si128((__m128i*)(H + (i + 3) * wiener_win2 + i), out[4]);
        _mm_storeu_si128((__m128i*)(H + (i + 3) * wiener_win2 + i + 2), out[5]);
        _mm_storeu_si128((__m128i*)(H + (i + 4) * wiener_win2 + i), out[6]);
        _mm_storeu_si128((__m128i*)(H + (i + 4) * wiener_win2 + i + 2), out[7]);

        for (int32_t j = i + 5; j < wiener_win2; j += 4) {
            in[0] = _mm_loadu_si128((__m128i*)(H + (i + 0) * wiener_win2 + j));
            in[1] = _mm_loadu_si128((__m128i*)(H + (i + 0) * wiener_win2 + j + 2));
            in[2] = _mm_loadu_si128((__m128i*)(H + (i + 1) * wiener_win2 + j));
            in[3] = _mm_loadu_si128((__m128i*)(H + (i + 1) * wiener_win2 + j + 2));
            in[4] = _mm_loadu_si128((__m128i*)(H + (i + 2) * wiener_win2 + j));
            in[5] = _mm_loadu_si128((__m128i*)(H + (i + 2) * wiener_win2 + j + 2));
            in[6] = _mm_loadu_si128((__m128i*)(H + (i + 3) * wiener_win2 + j));
            in[7] = _mm_loadu_si128((__m128i*)(H + (i + 3) * wiener_win2 + j + 2));

            transpose_64bit_4x4_sse2(in, out);

            _mm_storeu_si128((__m128i*)(H + (j + 0) * wiener_win2 + i), out[0]);
            _mm_storeu_si128((__m128i*)(H + (j + 0) * wiener_win2 + i + 2), out[1]);
            _mm_storeu_si128((__m128i*)(H + (j + 1) * wiener_win2 + i), out[2]);
            _mm_storeu_si128((__m128i*)(H + (j + 1) * wiener_win2 + i + 2), out[3]);
            _mm_storeu_si128((__m128i*)(H + (j + 2) * wiener_win2 + i), out[4]);
            _mm_storeu_si128((__m128i*)(H + (j + 2) * wiener_win2 + i + 2), out[5]);
            _mm_storeu_si128((__m128i*)(H + (j + 3) * wiener_win2 + i), out[6]);
            _mm_storeu_si128((__m128i*)(H + (j + 3) * wiener_win2 + i + 2), out[7]);
        }
    }
}

#if CONFIG_ENABLE_HIGH_BIT_DEPTH
static INLINE __m128i div4_sse4_1(const __m128i src) {
    __m128i sign, dst;

    // get sign
    sign = _mm_srli_epi64(src, 63);
    sign = _mm_sub_epi64(_mm_setzero_si128(), sign);

    // abs
    dst = _mm_xor_si128(src, sign);
    dst = _mm_sub_epi64(dst, sign);

    // divide by 4
    dst = _mm_srli_epi64(dst, 2);

    // apply sign
    dst = _mm_xor_si128(dst, sign);
    return _mm_sub_epi64(dst, sign);
}

static INLINE void div4_4x4_sse4_1(const int32_t wiener_win2, int64_t* const H, __m128i out[8]) {
    out[0] = _mm_loadu_si128((__m128i*)(H + 0 * wiener_win2));
    out[1] = _mm_loadu_si128((__m128i*)(H + 0 * wiener_win2 + 2));
    out[2] = _mm_loadu_si128((__m128i*)(H + 1 * wiener_win2));
    out[3] = _mm_loadu_si128((__m128i*)(H + 1 * wiener_win2 + 2));
    out[4] = _mm_loadu_si128((__m128i*)(H + 2 * wiener_win2));
    out[5] = _mm_loadu_si128((__m128i*)(H + 2 * wiener_win2 + 2));
    out[6] = _mm_loadu_si128((__m128i*)(H + 3 * wiener_win2));
    out[7] = _mm_loadu_si128((__m128i*)(H + 3 * wiener_win2 + 2));

    out[0] = div4_sse4_1(out[0]);
    out[1] = div4_sse4_1(out[1]);
    out[2] = div4_sse4_1(out[2]);
    out[3] = div4_sse4_1(out[3]);
    out[4] = div4_sse4_1(out[4]);
    out[5] = div4_sse4_1(out[5]);
    out[6] = div4_sse4_1(out[6]);
    out[7] = div4_sse4_1(out[7]);

    _mm_storeu_si128((__m128i*)(H + 0 * wiener_win2), out[0]);
    _mm_storeu_si128((__m128i*)(H + 0 * wiener_win2 + 2), out[1]);
    _mm_storeu_si128((__m128i*)(H + 1 * wiener_win2), out[2]);
    _mm_storeu_si128((__m128i*)(H + 1 * wiener_win2 + 2), out[3]);
    _mm_storeu_si128((__m128i*)(H + 2 * wiener_win2), out[4]);
    _mm_storeu_si128((__m128i*)(H + 2 * wiener_win2 + 2), out[5]);
    _mm_storeu_si128((__m128i*)(H + 3 * wiener_win2), out[6]);
    _mm_storeu_si128((__m128i*)(H + 3 * wiener_win2 + 2), out[7]);
}

static void div4_diagonal_copy_stats_sse4_1(const int32_t wiener_win2, int64_t* const H) {
    for (int32_t i = 0; i < wiener_win2 - 1; i += 4) {
        __m128i in[8], out[8];

        div4_4x4_sse4_1(wiener_win2, H + i * wiener_win2 + i + 1, in);
        transpose_64bit_4x4_sse2(in, out);

        _mm_storel_epi64((__m128i*)(H + (i + 1) * wiener_win2 + i), out[0]);
        _mm_storeu_si128((__m128i*)(H + (i + 2) * wiener_win2 + i), out[2]);
        _mm_storeu_si128((__m128i*)(H + (i + 3) * wiener_win2 + i), out[4]);
        _mm_storeu_si128((__m128i*)(H + (i + 3) * wiener_win2 + i + 2), out[5]);
        _mm_storeu_si128((__m128i*)(H + (i + 4) * wiener_win2 + i), out[6]);
        _mm_storeu_si128((__m128i*)(H + (i + 4) * wiener_win2 + i + 2), out[7]);

        for (int32_t j = i + 5; j < wiener_win2; j += 4) {
            div4_4x4_sse4_1(wiener_win2, H + i * wiener_win2 + j, in);
            transpose_64bit_4x4_sse2(in, out);

            _mm_storeu_si128((__m128i*)(H + (j + 0) * wiener_win2 + i), out[0]);
            _mm_storeu_si128((__m128i*)(H + (j + 0) * wiener_win2 + i + 2), out[1]);
            _mm_storeu_si128((__m128i*)(H + (j + 1) * wiener_win2 + i), out[2]);
            _mm_storeu_si128((__m128i*)(H + (j + 1) * wiener_win2 + i + 2), out[3]);
            _mm_storeu_si128((__m128i*)(H + (j + 2) * wiener_win2 + i), out[4]);
            _mm_storeu_si128((__m128i*)(H + (j + 2) * wiener_win2 + i + 2), out[5]);
            _mm_storeu_si128((__m128i*)(H + (j + 3) * wiener_win2 + i), out[6]);
            _mm_storeu_si128((__m128i*)(H + (j + 3) * wiener_win2 + i + 2), out[7]);
        }
    }
}
#endif

static INLINE void load_win7_sse4_1(const int16_t* const d, const int32_t width, __m128i out[2]) {
    const __m128i ds = _mm_loadu_si128((__m128i*)d);
    const __m128i de = _mm_loadu_si128((__m128i*)(d + width));

    out[0] = _mm_unpacklo_epi16(ds, de);
    out[1] = _mm_unpackhi_epi16(ds, de);
}

static INLINE void load_more_64_sse4_1(const int16_t* const src, const int32_t width, __m128i* const dst) {
    dst[0] = _mm_srli_si128(dst[0], 8);
    dst[1] = _mm_srli_si128(dst[1], 8);
    dst[0] = _mm_insert_epi64(dst[0], *(int64_t*)src, 1);
    dst[1] = _mm_insert_epi64(dst[1], *(int64_t*)(src + width), 1);
}

static INLINE void madd_sse4_1(const __m128i src, const __m128i dgd, __m128i* sum) {
    const __m128i sd = _mm_madd_epi16(src, dgd);
    *sum             = _mm_add_epi32(*sum, sd);
}

static INLINE void msub_sse4_1(const __m128i src, const __m128i dgd, __m128i* sum) {
    const __m128i sd = _mm_madd_epi16(src, dgd);
    *sum             = _mm_sub_epi32(*sum, sd);
}

static void stats_top_win5_sse4_1(const __m128i src[2], const __m128i dgd[2], const int16_t* const d,
                                  const int32_t d_stride, __m128i* sum_m, __m128i* sum_h) {
    __m128i dgds[WIENER_WIN_CHROMA * 2];

    dgds[0] = _mm_loadu_si128((__m128i*)(d + 0 * d_stride));
    dgds[1] = _mm_loadu_si128((__m128i*)(d + 0 * d_stride + 8));
    dgds[2] = _mm_loadu_si128((__m128i*)(d + 1 * d_stride));
    dgds[3] = _mm_loadu_si128((__m128i*)(d + 1 * d_stride + 8));
    dgds[4] = _mm_loadu_si128((__m128i*)(d + 2 * d_stride));
    dgds[5] = _mm_loadu_si128((__m128i*)(d + 2 * d_stride + 8));
    dgds[6] = _mm_loadu_si128((__m128i*)(d + 3 * d_stride));
    dgds[7] = _mm_loadu_si128((__m128i*)(d + 3 * d_stride + 8));
    dgds[8] = _mm_loadu_si128((__m128i*)(d + 4 * d_stride));
    dgds[9] = _mm_loadu_si128((__m128i*)(d + 4 * d_stride + 8));

    madd_sse4_1(src[0], dgds[0], &sum_m[0]);
    madd_sse4_1(src[1], dgds[1], &sum_m[1]);
    madd_sse4_1(src[0], dgds[2], &sum_m[2]);
    madd_sse4_1(src[1], dgds[3], &sum_m[3]);
    madd_sse4_1(src[0], dgds[4], &sum_m[4]);
    madd_sse4_1(src[1], dgds[5], &sum_m[5]);
    madd_sse4_1(src[0], dgds[6], &sum_m[6]);
    madd_sse4_1(src[1], dgds[7], &sum_m[7]);
    madd_sse4_1(src[0], dgds[8], &sum_m[8]);
    madd_sse4_1(src[1], dgds[9], &sum_m[9]);

    madd_sse4_1(dgd[0], dgds[0], &sum_h[0]);
    madd_sse4_1(dgd[1], dgds[1], &sum_h[1]);
    madd_sse4_1(dgd[0], dgds[2], &sum_h[2]);
    madd_sse4_1(dgd[1], dgds[3], &sum_h[3]);
    madd_sse4_1(dgd[0], dgds[4], &sum_h[4]);
    madd_sse4_1(dgd[1], dgds[5], &sum_h[5]);
    madd_sse4_1(dgd[0], dgds[6], &sum_h[6]);
    madd_sse4_1(dgd[1], dgds[7], &sum_h[7]);
    madd_sse4_1(dgd[0], dgds[8], &sum_h[8]);
    madd_sse4_1(dgd[1], dgds[9], &sum_h[9]);
}

static void stats_top_win7_sse4_1(const __m128i src[2], const __m128i dgd[2], const int16_t* const d,
                                  const int32_t d_stride, __m128i* sum_m, __m128i* sum_h) {
    __m128i dgds[WIENER_WIN * 2];

    dgds[0]  = _mm_loadu_si128((__m128i*)(d + 0 * d_stride));
    dgds[1]  = _mm_loadu_si128((__m128i*)(d + 0 * d_stride + 8));
    dgds[2]  = _mm_loadu_si128((__m128i*)(d + 1 * d_stride));
    dgds[3]  = _mm_loadu_si128((__m128i*)(d + 1 * d_stride + 8));
    dgds[4]  = _mm_loadu_si128((__m128i*)(d + 2 * d_stride));
    dgds[5]  = _mm_loadu_si128((__m128i*)(d + 2 * d_stride + 8));
    dgds[6]  = _mm_loadu_si128((__m128i*)(d + 3 * d_stride));
    dgds[7]  = _mm_loadu_si128((__m128i*)(d + 3 * d_stride + 8));
    dgds[8]  = _mm_loadu_si128((__m128i*)(d + 4 * d_stride));
    dgds[9]  = _mm_loadu_si128((__m128i*)(d + 4 * d_stride + 8));
    dgds[10] = _mm_loadu_si128((__m128i*)(d + 5 * d_stride));
    dgds[11] = _mm_loadu_si128((__m128i*)(d + 5 * d_stride + 8));
    dgds[12] = _mm_loadu_si128((__m128i*)(d + 6 * d_stride));
    dgds[13] = _mm_loadu_si128((__m128i*)(d + 6 * d_stride + 8));

    madd_sse4_1(src[0], dgds[0], &sum_m[0]);
    madd_sse4_1(src[1], dgds[1], &sum_m[1]);
    madd_sse4_1(src[0], dgds[2], &sum_m[2]);
    madd_sse4_1(src[1], dgds[3], &sum_m[3]);
    madd_sse4_1(src[0], dgds[4], &sum_m[4]);
    madd_sse4_1(src[1], dgds[5], &sum_m[5]);
    madd_sse4_1(src[0], dgds[6], &sum_m[6]);
    madd_sse4_1(src[1], dgds[7], &sum_m[7]);
    madd_sse4_1(src[0], dgds[8], &sum_m[8]);
    madd_sse4_1(src[1], dgds[9], &sum_m[9]);
    madd_sse4_1(src[0], dgds[10], &sum_m[10]);
    madd_sse4_1(src[1], dgds[11], &sum_m[11]);
    madd_sse4_1(src[0], dgds[12], &sum_m[12]);
    madd_sse4_1(src[1], dgds[13], &sum_m[13]);

    madd_sse4_1(dgd[0], dgds[0], &sum_h[0]);
    madd_sse4_1(dgd[1], dgds[1], &sum_h[1]);
    madd_sse4_1(dgd[0], dgds[2], &sum_h[2]);
    madd_sse4_1(dgd[1], dgds[3], &sum_h[3]);
    madd_sse4_1(dgd[0], dgds[4], &sum_h[4]);
    madd_sse4_1(dgd[1], dgds[5], &sum_h[5]);
    madd_sse4_1(dgd[0], dgds[6], &sum_h[6]);
    madd_sse4_1(dgd[1], dgds[7], &sum_h[7]);
    madd_sse4_1(dgd[0], dgds[8], &sum_h[8]);
    madd_sse4_1(dgd[1], dgds[9], &sum_h[9]);
    madd_sse4_1(dgd[0], dgds[10], &sum_h[10]);
    madd_sse4_1(dgd[1], dgds[11], &sum_h[11]);
    madd_sse4_1(dgd[0], dgds[12], &sum_h[12]);
    madd_sse4_1(dgd[1], dgds[13], &sum_h[13]);
}

static void stats_left_win5_sse4_1(const __m128i src[2], const int16_t* d, const int32_t d_stride, __m128i* sum) {
    __m128i dgds[WIN_CHROMA];

    dgds[0] = _mm_loadu_si128((__m128i*)(d + 1 * d_stride));
    dgds[1] = _mm_loadu_si128((__m128i*)(d + 1 * d_stride + 8));
    dgds[2] = _mm_loadu_si128((__m128i*)(d + 2 * d_stride));
    dgds[3] = _mm_loadu_si128((__m128i*)(d + 2 * d_stride + 8));
    dgds[4] = _mm_loadu_si128((__m128i*)(d + 3 * d_stride));
    dgds[5] = _mm_loadu_si128((__m128i*)(d + 3 * d_stride + 8));
    dgds[6] = _mm_loadu_si128((__m128i*)(d + 4 * d_stride));
    dgds[7] = _mm_loadu_si128((__m128i*)(d + 4 * d_stride + 8));

    madd_sse4_1(src[0], dgds[0], &sum[0]);
    madd_sse4_1(src[1], dgds[1], &sum[1]);
    madd_sse4_1(src[0], dgds[2], &sum[2]);
    madd_sse4_1(src[1], dgds[3], &sum[3]);
    madd_sse4_1(src[0], dgds[4], &sum[4]);
    madd_sse4_1(src[1], dgds[5], &sum[5]);
    madd_sse4_1(src[0], dgds[6], &sum[6]);
    madd_sse4_1(src[1], dgds[7], &sum[7]);
}

static void stats_left_win7_sse4_1(const __m128i src[2], const int16_t* d, const int32_t d_stride, __m128i* sum) {
    __m128i dgds[WIN_7];

    dgds[0]  = _mm_loadu_si128((__m128i*)(d + 1 * d_stride));
    dgds[1]  = _mm_loadu_si128((__m128i*)(d + 1 * d_stride + 8));
    dgds[2]  = _mm_loadu_si128((__m128i*)(d + 2 * d_stride));
    dgds[3]  = _mm_loadu_si128((__m128i*)(d + 2 * d_stride + 8));
    dgds[4]  = _mm_loadu_si128((__m128i*)(d + 3 * d_stride));
    dgds[5]  = _mm_loadu_si128((__m128i*)(d + 3 * d_stride + 8));
    dgds[6]  = _mm_loadu_si128((__m128i*)(d + 4 * d_stride));
    dgds[7]  = _mm_loadu_si128((__m128i*)(d + 4 * d_stride + 8));
    dgds[8]  = _mm_loadu_si128((__m128i*)(d + 5 * d_stride));
    dgds[9]  = _mm_loadu_si128((__m128i*)(d + 5 * d_stride + 8));
    dgds[10] = _mm_loadu_si128((__m128i*)(d + 6 * d_stride));
    dgds[11] = _mm_loadu_si128((__m128i*)(d + 6 * d_stride + 8));

    madd_sse4_1(src[0], dgds[0], &sum[0]);
    madd_sse4_1(src[1], dgds[1], &sum[1]);
    madd_sse4_1(src[0], dgds[2], &sum[2]);
    madd_sse4_1(src[1], dgds[3], &sum[3]);
    madd_sse4_1(src[0], dgds[4], &sum[4]);
    madd_sse4_1(src[1], dgds[5], &sum[5]);
    madd_sse4_1(src[0], dgds[6], &sum[6]);
    madd_sse4_1(src[1], dgds[7], &sum[7]);
    madd_sse4_1(src[0], dgds[8], &sum[8]);
    madd_sse4_1(src[1], dgds[9], &sum[9]);
    madd_sse4_1(src[0], dgds[10], &sum[10]);
    madd_sse4_1(src[1], dgds[11], &sum[11]);
}

static void load_square_win5_sse4_1(const int16_t* const di, const int16_t* const d_j, const int32_t d_stride,
                                    const int32_t height, __m128i* d_is, __m128i* d_ie, __m128i* d_js, __m128i* d_je) {
    d_is[0] = _mm_loadu_si128((__m128i*)(di + 0 * d_stride));
    d_is[1] = _mm_loadu_si128((__m128i*)(di + 0 * d_stride + 8));
    d_js[0] = _mm_loadu_si128((__m128i*)(d_j + 0 * d_stride));
    d_js[1] = _mm_loadu_si128((__m128i*)(d_j + 0 * d_stride + 8));
    d_is[2] = _mm_loadu_si128((__m128i*)(di + 1 * d_stride));
    d_is[3] = _mm_loadu_si128((__m128i*)(di + 1 * d_stride + 8));
    d_js[2] = _mm_loadu_si128((__m128i*)(d_j + 1 * d_stride));
    d_js[3] = _mm_loadu_si128((__m128i*)(d_j + 1 * d_stride + 8));
    d_is[4] = _mm_loadu_si128((__m128i*)(di + 2 * d_stride));
    d_is[5] = _mm_loadu_si128((__m128i*)(di + 2 * d_stride + 8));
    d_js[4] = _mm_loadu_si128((__m128i*)(d_j + 2 * d_stride));
    d_js[5] = _mm_loadu_si128((__m128i*)(d_j + 2 * d_stride + 8));
    d_is[6] = _mm_loadu_si128((__m128i*)(di + 3 * d_stride));
    d_is[7] = _mm_loadu_si128((__m128i*)(di + 3 * d_stride + 8));
    d_js[6] = _mm_loadu_si128((__m128i*)(d_j + 3 * d_stride));
    d_js[7] = _mm_loadu_si128((__m128i*)(d_j + 3 * d_stride + 8));

    d_ie[0] = _mm_loadu_si128((__m128i*)(di + (0 + height) * d_stride));
    d_ie[1] = _mm_loadu_si128((__m128i*)(di + (0 + height) * d_stride + 8));
    d_je[0] = _mm_loadu_si128((__m128i*)(d_j + (0 + height) * d_stride));
    d_je[1] = _mm_loadu_si128((__m128i*)(d_j + (0 + height) * d_stride + 8));
    d_ie[2] = _mm_loadu_si128((__m128i*)(di + (1 + height) * d_stride));
    d_ie[3] = _mm_loadu_si128((__m128i*)(di + (1 + height) * d_stride + 8));
    d_je[2] = _mm_loadu_si128((__m128i*)(d_j + (1 + height) * d_stride));
    d_je[3] = _mm_loadu_si128((__m128i*)(d_j + (1 + height) * d_stride + 8));
    d_ie[4] = _mm_loadu_si128((__m128i*)(di + (2 + height) * d_stride));
    d_ie[5] = _mm_loadu_si128((__m128i*)(di + (2 + height) * d_stride + 8));
    d_je[4] = _mm_loadu_si128((__m128i*)(d_j + (2 + height) * d_stride));
    d_je[5] = _mm_loadu_si128((__m128i*)(d_j + (2 + height) * d_stride + 8));
    d_ie[6] = _mm_loadu_si128((__m128i*)(di + (3 + height) * d_stride));
    d_ie[7] = _mm_loadu_si128((__m128i*)(di + (3 + height) * d_stride + 8));
    d_je[6] = _mm_loadu_si128((__m128i*)(d_j + (3 + height) * d_stride));
    d_je[7] = _mm_loadu_si128((__m128i*)(d_j + (3 + height) * d_stride + 8));
}

static void load_square_win7_sse4_1(const int16_t* const di, const int16_t* const d_j, const int32_t d_stride,
                                    const int32_t height, __m128i* d_is, __m128i* d_ie, __m128i* d_js, __m128i* d_je) {
    d_is[0]  = _mm_loadu_si128((__m128i*)(di + 0 * d_stride));
    d_is[1]  = _mm_loadu_si128((__m128i*)(di + 0 * d_stride + 8));
    d_js[0]  = _mm_loadu_si128((__m128i*)(d_j + 0 * d_stride));
    d_js[1]  = _mm_loadu_si128((__m128i*)(d_j + 0 * d_stride + 8));
    d_is[2]  = _mm_loadu_si128((__m128i*)(di + 1 * d_stride));
    d_is[3]  = _mm_loadu_si128((__m128i*)(di + 1 * d_stride + 8));
    d_js[2]  = _mm_loadu_si128((__m128i*)(d_j + 1 * d_stride));
    d_js[3]  = _mm_loadu_si128((__m128i*)(d_j + 1 * d_stride + 8));
    d_is[4]  = _mm_loadu_si128((__m128i*)(di + 2 * d_stride));
    d_is[5]  = _mm_loadu_si128((__m128i*)(di + 2 * d_stride + 8));
    d_js[4]  = _mm_loadu_si128((__m128i*)(d_j + 2 * d_stride));
    d_js[5]  = _mm_loadu_si128((__m128i*)(d_j + 2 * d_stride + 8));
    d_is[6]  = _mm_loadu_si128((__m128i*)(di + 3 * d_stride));
    d_is[7]  = _mm_loadu_si128((__m128i*)(di + 3 * d_stride + 8));
    d_js[6]  = _mm_loadu_si128((__m128i*)(d_j + 3 * d_stride));
    d_js[7]  = _mm_loadu_si128((__m128i*)(d_j + 3 * d_stride + 8));
    d_is[8]  = _mm_loadu_si128((__m128i*)(di + 4 * d_stride));
    d_is[9]  = _mm_loadu_si128((__m128i*)(di + 4 * d_stride + 8));
    d_js[8]  = _mm_loadu_si128((__m128i*)(d_j + 4 * d_stride));
    d_js[9]  = _mm_loadu_si128((__m128i*)(d_j + 4 * d_stride + 8));
    d_is[10] = _mm_loadu_si128((__m128i*)(di + 5 * d_stride));
    d_is[11] = _mm_loadu_si128((__m128i*)(di + 5 * d_stride + 8));
    d_js[10] = _mm_loadu_si128((__m128i*)(d_j + 5 * d_stride));
    d_js[11] = _mm_loadu_si128((__m128i*)(d_j + 5 * d_stride + 8));

    d_ie[0]  = _mm_loadu_si128((__m128i*)(di + (0 + height) * d_stride));
    d_ie[1]  = _mm_loadu_si128((__m128i*)(di + (0 + height) * d_stride + 8));
    d_je[0]  = _mm_loadu_si128((__m128i*)(d_j + (0 + height) * d_stride));
    d_je[1]  = _mm_loadu_si128((__m128i*)(d_j + (0 + height) * d_stride + 8));
    d_ie[2]  = _mm_loadu_si128((__m128i*)(di + (1 + height) * d_stride));
    d_ie[3]  = _mm_loadu_si128((__m128i*)(di + (1 + height) * d_stride + 8));
    d_je[2]  = _mm_loadu_si128((__m128i*)(d_j + (1 + height) * d_stride));
    d_je[3]  = _mm_loadu_si128((__m128i*)(d_j + (1 + height) * d_stride + 8));
    d_ie[4]  = _mm_loadu_si128((__m128i*)(di + (2 + height) * d_stride));
    d_ie[5]  = _mm_loadu_si128((__m128i*)(di + (2 + height) * d_stride + 8));
    d_je[4]  = _mm_loadu_si128((__m128i*)(d_j + (2 + height) * d_stride));
    d_je[5]  = _mm_loadu_si128((__m128i*)(d_j + (2 + height) * d_stride + 8));
    d_ie[6]  = _mm_loadu_si128((__m128i*)(di + (3 + height) * d_stride));
    d_ie[7]  = _mm_loadu_si128((__m128i*)(di + (3 + height) * d_stride + 8));
    d_je[6]  = _mm_loadu_si128((__m128i*)(d_j + (3 + height) * d_stride));
    d_je[7]  = _mm_loadu_si128((__m128i*)(d_j + (3 + height) * d_stride + 8));
    d_ie[8]  = _mm_loadu_si128((__m128i*)(di + (4 + height) * d_stride));
    d_ie[9]  = _mm_loadu_si128((__m128i*)(di + (4 + height) * d_stride + 8));
    d_je[8]  = _mm_loadu_si128((__m128i*)(d_j + (4 + height) * d_stride));
    d_je[9]  = _mm_loadu_si128((__m128i*)(d_j + (4 + height) * d_stride + 8));
    d_ie[10] = _mm_loadu_si128((__m128i*)(di + (5 + height) * d_stride));
    d_ie[11] = _mm_loadu_si128((__m128i*)(di + (5 + height) * d_stride + 8));
    d_je[10] = _mm_loadu_si128((__m128i*)(d_j + (5 + height) * d_stride));
    d_je[11] = _mm_loadu_si128((__m128i*)(d_j + (5 + height) * d_stride + 8));
}

static void load_triangle_win5_sse4_1(const int16_t* const di, const int32_t d_stride, const int32_t height,
                                      __m128i* d_is, __m128i* d_ie) {
    d_is[0] = _mm_loadu_si128((__m128i*)(di + 0 * d_stride));
    d_is[1] = _mm_loadu_si128((__m128i*)(di + 0 * d_stride + 8));
    d_is[2] = _mm_loadu_si128((__m128i*)(di + 1 * d_stride));
    d_is[3] = _mm_loadu_si128((__m128i*)(di + 1 * d_stride + 8));
    d_is[4] = _mm_loadu_si128((__m128i*)(di + 2 * d_stride));
    d_is[5] = _mm_loadu_si128((__m128i*)(di + 2 * d_stride + 8));
    d_is[6] = _mm_loadu_si128((__m128i*)(di + 3 * d_stride));
    d_is[7] = _mm_loadu_si128((__m128i*)(di + 3 * d_stride + 8));

    d_ie[0] = _mm_loadu_si128((__m128i*)(di + (0 + height) * d_stride));
    d_ie[1] = _mm_loadu_si128((__m128i*)(di + (0 + height) * d_stride + 8));
    d_ie[2] = _mm_loadu_si128((__m128i*)(di + (1 + height) * d_stride));
    d_ie[3] = _mm_loadu_si128((__m128i*)(di + (1 + height) * d_stride + 8));
    d_ie[4] = _mm_loadu_si128((__m128i*)(di + (2 + height) * d_stride));
    d_ie[5] = _mm_loadu_si128((__m128i*)(di + (2 + height) * d_stride + 8));
    d_ie[6] = _mm_loadu_si128((__m128i*)(di + (3 + height) * d_stride));
    d_ie[7] = _mm_loadu_si128((__m128i*)(di + (3 + height) * d_stride + 8));
}

static void load_triangle_win7_sse4_1(const int16_t* const di, const int32_t d_stride, const int32_t height,
                                      __m128i* d_is, __m128i* d_ie) {
    d_is[0]  = _mm_loadu_si128((__m128i*)(di + 0 * d_stride));
    d_is[1]  = _mm_loadu_si128((__m128i*)(di + 0 * d_stride + 8));
    d_is[2]  = _mm_loadu_si128((__m128i*)(di + 1 * d_stride));
    d_is[3]  = _mm_loadu_si128((__m128i*)(di + 1 * d_stride + 8));
    d_is[4]  = _mm_loadu_si128((__m128i*)(di + 2 * d_stride));
    d_is[5]  = _mm_loadu_si128((__m128i*)(di + 2 * d_stride + 8));
    d_is[6]  = _mm_loadu_si128((__m128i*)(di + 3 * d_stride));
    d_is[7]  = _mm_loadu_si128((__m128i*)(di + 3 * d_stride + 8));
    d_is[8]  = _mm_loadu_si128((__m128i*)(di + 4 * d_stride));
    d_is[9]  = _mm_loadu_si128((__m128i*)(di + 4 * d_stride + 8));
    d_is[10] = _mm_loadu_si128((__m128i*)(di + 5 * d_stride));
    d_is[11] = _mm_loadu_si128((__m128i*)(di + 5 * d_stride + 8));

    d_ie[0]  = _mm_loadu_si128((__m128i*)(di + (0 + height) * d_stride));
    d_ie[1]  = _mm_loadu_si128((__m128i*)(di + (0 + height) * d_stride + 8));
    d_ie[2]  = _mm_loadu_si128((__m128i*)(di + (1 + height) * d_stride));
    d_ie[3]  = _mm_loadu_si128((__m128i*)(di + (1 + height) * d_stride + 8));
    d_ie[4]  = _mm_loadu_si128((__m128i*)(di + (2 + height) * d_stride));
    d_ie[5]  = _mm_loadu_si128((__m128i*)(di + (2 + height) * d_stride + 8));
    d_ie[6]  = _mm_loadu_si128((__m128i*)(di + (3 + height) * d_stride));
    d_ie[7]  = _mm_loadu_si128((__m128i*)(di + (3 + height) * d_stride + 8));
    d_ie[8]  = _mm_loadu_si128((__m128i*)(di + (4 + height) * d_stride));
    d_ie[9]  = _mm_loadu_si128((__m128i*)(di + (4 + height) * d_stride + 8));
    d_ie[10] = _mm_loadu_si128((__m128i*)(di + (5 + height) * d_stride));
    d_ie[11] = _mm_loadu_si128((__m128i*)(di + (5 + height) * d_stride + 8));
}

static void derive_square_win5_sse4_1(const __m128i* d_is, const __m128i* d_ie, const __m128i* d_js,
                                      const __m128i* d_je, __m128i deltas[][WIN_CHROMA]) {
    msub_sse4_1(d_is[0], d_js[0], &deltas[0][0]);
    msub_sse4_1(d_is[1], d_js[1], &deltas[0][1]);
    msub_sse4_1(d_is[0], d_js[2], &deltas[0][2]);
    msub_sse4_1(d_is[1], d_js[3], &deltas[0][3]);
    msub_sse4_1(d_is[0], d_js[4], &deltas[0][4]);
    msub_sse4_1(d_is[1], d_js[5], &deltas[0][5]);
    msub_sse4_1(d_is[0], d_js[6], &deltas[0][6]);
    msub_sse4_1(d_is[1], d_js[7], &deltas[0][7]);

    msub_sse4_1(d_is[2], d_js[0], &deltas[1][0]);
    msub_sse4_1(d_is[3], d_js[1], &deltas[1][1]);
    msub_sse4_1(d_is[2], d_js[2], &deltas[1][2]);
    msub_sse4_1(d_is[3], d_js[3], &deltas[1][3]);
    msub_sse4_1(d_is[2], d_js[4], &deltas[1][4]);
    msub_sse4_1(d_is[3], d_js[5], &deltas[1][5]);
    msub_sse4_1(d_is[2], d_js[6], &deltas[1][6]);
    msub_sse4_1(d_is[3], d_js[7], &deltas[1][7]);

    msub_sse4_1(d_is[4], d_js[0], &deltas[2][0]);
    msub_sse4_1(d_is[5], d_js[1], &deltas[2][1]);
    msub_sse4_1(d_is[4], d_js[2], &deltas[2][2]);
    msub_sse4_1(d_is[5], d_js[3], &deltas[2][3]);
    msub_sse4_1(d_is[4], d_js[4], &deltas[2][4]);
    msub_sse4_1(d_is[5], d_js[5], &deltas[2][5]);
    msub_sse4_1(d_is[4], d_js[6], &deltas[2][6]);
    msub_sse4_1(d_is[5], d_js[7], &deltas[2][7]);

    msub_sse4_1(d_is[6], d_js[0], &deltas[3][0]);
    msub_sse4_1(d_is[7], d_js[1], &deltas[3][1]);
    msub_sse4_1(d_is[6], d_js[2], &deltas[3][2]);
    msub_sse4_1(d_is[7], d_js[3], &deltas[3][3]);
    msub_sse4_1(d_is[6], d_js[4], &deltas[3][4]);
    msub_sse4_1(d_is[7], d_js[5], &deltas[3][5]);
    msub_sse4_1(d_is[6], d_js[6], &deltas[3][6]);
    msub_sse4_1(d_is[7], d_js[7], &deltas[3][7]);

    madd_sse4_1(d_ie[0], d_je[0], &deltas[0][0]);
    madd_sse4_1(d_ie[1], d_je[1], &deltas[0][1]);
    madd_sse4_1(d_ie[0], d_je[2], &deltas[0][2]);
    madd_sse4_1(d_ie[1], d_je[3], &deltas[0][3]);
    madd_sse4_1(d_ie[0], d_je[4], &deltas[0][4]);
    madd_sse4_1(d_ie[1], d_je[5], &deltas[0][5]);
    madd_sse4_1(d_ie[0], d_je[6], &deltas[0][6]);
    madd_sse4_1(d_ie[1], d_je[7], &deltas[0][7]);

    madd_sse4_1(d_ie[2], d_je[0], &deltas[1][0]);
    madd_sse4_1(d_ie[3], d_je[1], &deltas[1][1]);
    madd_sse4_1(d_ie[2], d_je[2], &deltas[1][2]);
    madd_sse4_1(d_ie[3], d_je[3], &deltas[1][3]);
    madd_sse4_1(d_ie[2], d_je[4], &deltas[1][4]);
    madd_sse4_1(d_ie[3], d_je[5], &deltas[1][5]);
    madd_sse4_1(d_ie[2], d_je[6], &deltas[1][6]);
    madd_sse4_1(d_ie[3], d_je[7], &deltas[1][7]);

    madd_sse4_1(d_ie[4], d_je[0], &deltas[2][0]);
    madd_sse4_1(d_ie[5], d_je[1], &deltas[2][1]);
    madd_sse4_1(d_ie[4], d_je[2], &deltas[2][2]);
    madd_sse4_1(d_ie[5], d_je[3], &deltas[2][3]);
    madd_sse4_1(d_ie[4], d_je[4], &deltas[2][4]);
    madd_sse4_1(d_ie[5], d_je[5], &deltas[2][5]);
    madd_sse4_1(d_ie[4], d_je[6], &deltas[2][6]);
    madd_sse4_1(d_ie[5], d_je[7], &deltas[2][7]);

    madd_sse4_1(d_ie[6], d_je[0], &deltas[3][0]);
    madd_sse4_1(d_ie[7], d_je[1], &deltas[3][1]);
    madd_sse4_1(d_ie[6], d_je[2], &deltas[3][2]);
    madd_sse4_1(d_ie[7], d_je[3], &deltas[3][3]);
    madd_sse4_1(d_ie[6], d_je[4], &deltas[3][4]);
    madd_sse4_1(d_ie[7], d_je[5], &deltas[3][5]);
    madd_sse4_1(d_ie[6], d_je[6], &deltas[3][6]);
    madd_sse4_1(d_ie[7], d_je[7], &deltas[3][7]);
}

static void derive_square_win7_sse4_1(const __m128i* d_is, const __m128i* d_ie, const __m128i* d_js,
                                      const __m128i* d_je, __m128i deltas[][WIN_7]) {
    msub_sse4_1(d_is[0], d_js[0], &deltas[0][0]);
    msub_sse4_1(d_is[1], d_js[1], &deltas[0][1]);
    msub_sse4_1(d_is[0], d_js[2], &deltas[0][2]);
    msub_sse4_1(d_is[1], d_js[3], &deltas[0][3]);
    msub_sse4_1(d_is[0], d_js[4], &deltas[0][4]);
    msub_sse4_1(d_is[1], d_js[5], &deltas[0][5]);
    msub_sse4_1(d_is[0], d_js[6], &deltas[0][6]);
    msub_sse4_1(d_is[1], d_js[7], &deltas[0][7]);
    msub_sse4_1(d_is[0], d_js[8], &deltas[0][8]);
    msub_sse4_1(d_is[1], d_js[9], &deltas[0][9]);
    msub_sse4_1(d_is[0], d_js[10], &deltas[0][10]);
    msub_sse4_1(d_is[1], d_js[11], &deltas[0][11]);

    msub_sse4_1(d_is[2], d_js[0], &deltas[1][0]);
    msub_sse4_1(d_is[3], d_js[1], &deltas[1][1]);
    msub_sse4_1(d_is[2], d_js[2], &deltas[1][2]);
    msub_sse4_1(d_is[3], d_js[3], &deltas[1][3]);
    msub_sse4_1(d_is[2], d_js[4], &deltas[1][4]);
    msub_sse4_1(d_is[3], d_js[5], &deltas[1][5]);
    msub_sse4_1(d_is[2], d_js[6], &deltas[1][6]);
    msub_sse4_1(d_is[3], d_js[7], &deltas[1][7]);
    msub_sse4_1(d_is[2], d_js[8], &deltas[1][8]);
    msub_sse4_1(d_is[3], d_js[9], &deltas[1][9]);
    msub_sse4_1(d_is[2], d_js[10], &deltas[1][10]);
    msub_sse4_1(d_is[3], d_js[11], &deltas[1][11]);

    msub_sse4_1(d_is[4], d_js[0], &deltas[2][0]);
    msub_sse4_1(d_is[5], d_js[1], &deltas[2][1]);
    msub_sse4_1(d_is[4], d_js[2], &deltas[2][2]);
    msub_sse4_1(d_is[5], d_js[3], &deltas[2][3]);
    msub_sse4_1(d_is[4], d_js[4], &deltas[2][4]);
    msub_sse4_1(d_is[5], d_js[5], &deltas[2][5]);
    msub_sse4_1(d_is[4], d_js[6], &deltas[2][6]);
    msub_sse4_1(d_is[5], d_js[7], &deltas[2][7]);
    msub_sse4_1(d_is[4], d_js[8], &deltas[2][8]);
    msub_sse4_1(d_is[5], d_js[9], &deltas[2][9]);
    msub_sse4_1(d_is[4], d_js[10], &deltas[2][10]);
    msub_sse4_1(d_is[5], d_js[11], &deltas[2][11]);

    msub_sse4_1(d_is[6], d_js[0], &deltas[3][0]);
    msub_sse4_1(d_is[7], d_js[1], &deltas[3][1]);
    msub_sse4_1(d_is[6], d_js[2], &deltas[3][2]);
    msub_sse4_1(d_is[7], d_js[3], &deltas[3][3]);
    msub_sse4_1(d_is[6], d_js[4], &deltas[3][4]);
    msub_sse4_1(d_is[7], d_js[5], &deltas[3][5]);
    msub_sse4_1(d_is[6], d_js[6], &deltas[3][6]);
    msub_sse4_1(d_is[7], d_js[7], &deltas[3][7]);
    msub_sse4_1(d_is[6], d_js[8], &deltas[3][8]);
    msub_sse4_1(d_is[7], d_js[9], &deltas[3][9]);
    msub_sse4_1(d_is[6], d_js[10], &deltas[3][10]);
    msub_sse4_1(d_is[7], d_js[11], &deltas[3][11]);

    msub_sse4_1(d_is[8], d_js[0], &deltas[4][0]);
    msub_sse4_1(d_is[9], d_js[1], &deltas[4][1]);
    msub_sse4_1(d_is[8], d_js[2], &deltas[4][2]);
    msub_sse4_1(d_is[9], d_js[3], &deltas[4][3]);
    msub_sse4_1(d_is[8], d_js[4], &deltas[4][4]);
    msub_sse4_1(d_is[9], d_js[5], &deltas[4][5]);
    msub_sse4_1(d_is[8], d_js[6], &deltas[4][6]);
    msub_sse4_1(d_is[9], d_js[7], &deltas[4][7]);
    msub_sse4_1(d_is[8], d_js[8], &deltas[4][8]);
    msub_sse4_1(d_is[9], d_js[9], &deltas[4][9]);
    msub_sse4_1(d_is[8], d_js[10], &deltas[4][10]);
    msub_sse4_1(d_is[9], d_js[11], &deltas[4][11]);

    msub_sse4_1(d_is[10], d_js[0], &deltas[5][0]);
    msub_sse4_1(d_is[11], d_js[1], &deltas[5][1]);
    msub_sse4_1(d_is[10], d_js[2], &deltas[5][2]);
    msub_sse4_1(d_is[11], d_js[3], &deltas[5][3]);
    msub_sse4_1(d_is[10], d_js[4], &deltas[5][4]);
    msub_sse4_1(d_is[11], d_js[5], &deltas[5][5]);
    msub_sse4_1(d_is[10], d_js[6], &deltas[5][6]);
    msub_sse4_1(d_is[11], d_js[7], &deltas[5][7]);
    msub_sse4_1(d_is[10], d_js[8], &deltas[5][8]);
    msub_sse4_1(d_is[11], d_js[9], &deltas[5][9]);
    msub_sse4_1(d_is[10], d_js[10], &deltas[5][10]);
    msub_sse4_1(d_is[11], d_js[11], &deltas[5][11]);

    madd_sse4_1(d_ie[0], d_je[0], &deltas[0][0]);
    madd_sse4_1(d_ie[1], d_je[1], &deltas[0][1]);
    madd_sse4_1(d_ie[0], d_je[2], &deltas[0][2]);
    madd_sse4_1(d_ie[1], d_je[3], &deltas[0][3]);
    madd_sse4_1(d_ie[0], d_je[4], &deltas[0][4]);
    madd_sse4_1(d_ie[1], d_je[5], &deltas[0][5]);
    madd_sse4_1(d_ie[0], d_je[6], &deltas[0][6]);
    madd_sse4_1(d_ie[1], d_je[7], &deltas[0][7]);
    madd_sse4_1(d_ie[0], d_je[8], &deltas[0][8]);
    madd_sse4_1(d_ie[1], d_je[9], &deltas[0][9]);
    madd_sse4_1(d_ie[0], d_je[10], &deltas[0][10]);
    madd_sse4_1(d_ie[1], d_je[11], &deltas[0][11]);

    madd_sse4_1(d_ie[2], d_je[0], &deltas[1][0]);
    madd_sse4_1(d_ie[3], d_je[1], &deltas[1][1]);
    madd_sse4_1(d_ie[2], d_je[2], &deltas[1][2]);
    madd_sse4_1(d_ie[3], d_je[3], &deltas[1][3]);
    madd_sse4_1(d_ie[2], d_je[4], &deltas[1][4]);
    madd_sse4_1(d_ie[3], d_je[5], &deltas[1][5]);
    madd_sse4_1(d_ie[2], d_je[6], &deltas[1][6]);
    madd_sse4_1(d_ie[3], d_je[7], &deltas[1][7]);
    madd_sse4_1(d_ie[2], d_je[8], &deltas[1][8]);
    madd_sse4_1(d_ie[3], d_je[9], &deltas[1][9]);
    madd_sse4_1(d_ie[2], d_je[10], &deltas[1][10]);
    madd_sse4_1(d_ie[3], d_je[11], &deltas[1][11]);

    madd_sse4_1(d_ie[4], d_je[0], &deltas[2][0]);
    madd_sse4_1(d_ie[5], d_je[1], &deltas[2][1]);
    madd_sse4_1(d_ie[4], d_je[2], &deltas[2][2]);
    madd_sse4_1(d_ie[5], d_je[3], &deltas[2][3]);
    madd_sse4_1(d_ie[4], d_je[4], &deltas[2][4]);
    madd_sse4_1(d_ie[5], d_je[5], &deltas[2][5]);
    madd_sse4_1(d_ie[4], d_je[6], &deltas[2][6]);
    madd_sse4_1(d_ie[5], d_je[7], &deltas[2][7]);
    madd_sse4_1(d_ie[4], d_je[8], &deltas[2][8]);
    madd_sse4_1(d_ie[5], d_je[9], &deltas[2][9]);
    madd_sse4_1(d_ie[4], d_je[10], &deltas[2][10]);
    madd_sse4_1(d_ie[5], d_je[11], &deltas[2][11]);

    madd_sse4_1(d_ie[6], d_je[0], &deltas[3][0]);
    madd_sse4_1(d_ie[7], d_je[1], &deltas[3][1]);
    madd_sse4_1(d_ie[6], d_je[2], &deltas[3][2]);
    madd_sse4_1(d_ie[7], d_je[3], &deltas[3][3]);
    madd_sse4_1(d_ie[6], d_je[4], &deltas[3][4]);
    madd_sse4_1(d_ie[7], d_je[5], &deltas[3][5]);
    madd_sse4_1(d_ie[6], d_je[6], &deltas[3][6]);
    madd_sse4_1(d_ie[7], d_je[7], &deltas[3][7]);
    madd_sse4_1(d_ie[6], d_je[8], &deltas[3][8]);
    madd_sse4_1(d_ie[7], d_je[9], &deltas[3][9]);
    madd_sse4_1(d_ie[6], d_je[10], &deltas[3][10]);
    madd_sse4_1(d_ie[7], d_je[11], &deltas[3][11]);

    madd_sse4_1(d_ie[8], d_je[0], &deltas[4][0]);
    madd_sse4_1(d_ie[9], d_je[1], &deltas[4][1]);
    madd_sse4_1(d_ie[8], d_je[2], &deltas[4][2]);
    madd_sse4_1(d_ie[9], d_je[3], &deltas[4][3]);
    madd_sse4_1(d_ie[8], d_je[4], &deltas[4][4]);
    madd_sse4_1(d_ie[9], d_je[5], &deltas[4][5]);
    madd_sse4_1(d_ie[8], d_je[6], &deltas[4][6]);
    madd_sse4_1(d_ie[9], d_je[7], &deltas[4][7]);
    madd_sse4_1(d_ie[8], d_je[8], &deltas[4][8]);
    madd_sse4_1(d_ie[9], d_je[9], &deltas[4][9]);
    madd_sse4_1(d_ie[8], d_je[10], &deltas[4][10]);
    madd_sse4_1(d_ie[9], d_je[11], &deltas[4][11]);

    madd_sse4_1(d_ie[10], d_je[0], &deltas[5][0]);
    madd_sse4_1(d_ie[11], d_je[1], &deltas[5][1]);
    madd_sse4_1(d_ie[10], d_je[2], &deltas[5][2]);
    madd_sse4_1(d_ie[11], d_je[3], &deltas[5][3]);
    madd_sse4_1(d_ie[10], d_je[4], &deltas[5][4]);
    madd_sse4_1(d_ie[11], d_je[5], &deltas[5][5]);
    madd_sse4_1(d_ie[10], d_je[6], &deltas[5][6]);
    madd_sse4_1(d_ie[11], d_je[7], &deltas[5][7]);
    madd_sse4_1(d_ie[10], d_je[8], &deltas[5][8]);
    madd_sse4_1(d_ie[11], d_je[9], &deltas[5][9]);
    madd_sse4_1(d_ie[10], d_je[10], &deltas[5][10]);
    madd_sse4_1(d_ie[11], d_je[11], &deltas[5][11]);
}

static void derive_triangle_win5_sse4_1(const __m128i* d_is, const __m128i* d_ie, __m128i* deltas) {
    msub_sse4_1(d_is[0], d_is[0], &deltas[0]);
    msub_sse4_1(d_is[1], d_is[1], &deltas[1]);
    msub_sse4_1(d_is[0], d_is[2], &deltas[2]);
    msub_sse4_1(d_is[1], d_is[3], &deltas[3]);
    msub_sse4_1(d_is[0], d_is[4], &deltas[4]);
    msub_sse4_1(d_is[1], d_is[5], &deltas[5]);
    msub_sse4_1(d_is[0], d_is[6], &deltas[6]);
    msub_sse4_1(d_is[1], d_is[7], &deltas[7]);
    msub_sse4_1(d_is[2], d_is[2], &deltas[8]);
    msub_sse4_1(d_is[3], d_is[3], &deltas[9]);
    msub_sse4_1(d_is[2], d_is[4], &deltas[10]);
    msub_sse4_1(d_is[3], d_is[5], &deltas[11]);
    msub_sse4_1(d_is[2], d_is[6], &deltas[12]);
    msub_sse4_1(d_is[3], d_is[7], &deltas[13]);
    msub_sse4_1(d_is[4], d_is[4], &deltas[14]);
    msub_sse4_1(d_is[5], d_is[5], &deltas[15]);
    msub_sse4_1(d_is[4], d_is[6], &deltas[16]);
    msub_sse4_1(d_is[5], d_is[7], &deltas[17]);
    msub_sse4_1(d_is[6], d_is[6], &deltas[18]);
    msub_sse4_1(d_is[7], d_is[7], &deltas[19]);

    madd_sse4_1(d_ie[0], d_ie[0], &deltas[0]);
    madd_sse4_1(d_ie[1], d_ie[1], &deltas[1]);
    madd_sse4_1(d_ie[0], d_ie[2], &deltas[2]);
    madd_sse4_1(d_ie[1], d_ie[3], &deltas[3]);
    madd_sse4_1(d_ie[0], d_ie[4], &deltas[4]);
    madd_sse4_1(d_ie[1], d_ie[5], &deltas[5]);
    madd_sse4_1(d_ie[0], d_ie[6], &deltas[6]);
    madd_sse4_1(d_ie[1], d_ie[7], &deltas[7]);
    madd_sse4_1(d_ie[2], d_ie[2], &deltas[8]);
    madd_sse4_1(d_ie[3], d_ie[3], &deltas[9]);
    madd_sse4_1(d_ie[2], d_ie[4], &deltas[10]);
    madd_sse4_1(d_ie[3], d_ie[5], &deltas[11]);
    madd_sse4_1(d_ie[2], d_ie[6], &deltas[12]);
    madd_sse4_1(d_ie[3], d_ie[7], &deltas[13]);
    madd_sse4_1(d_ie[4], d_ie[4], &deltas[14]);
    madd_sse4_1(d_ie[5], d_ie[5], &deltas[15]);
    madd_sse4_1(d_ie[4], d_ie[6], &deltas[16]);
    madd_sse4_1(d_ie[5], d_ie[7], &deltas[17]);
    madd_sse4_1(d_ie[6], d_ie[6], &deltas[18]);
    madd_sse4_1(d_ie[7], d_ie[7], &deltas[19]);
}

static void derive_triangle_win7_sse4_1(const __m128i* d_is, const __m128i* d_ie, __m128i* deltas) {
    msub_sse4_1(d_is[0], d_is[0], &deltas[0]);
    msub_sse4_1(d_is[1], d_is[1], &deltas[1]);
    msub_sse4_1(d_is[0], d_is[2], &deltas[2]);
    msub_sse4_1(d_is[1], d_is[3], &deltas[3]);
    msub_sse4_1(d_is[0], d_is[4], &deltas[4]);
    msub_sse4_1(d_is[1], d_is[5], &deltas[5]);
    msub_sse4_1(d_is[0], d_is[6], &deltas[6]);
    msub_sse4_1(d_is[1], d_is[7], &deltas[7]);
    msub_sse4_1(d_is[0], d_is[8], &deltas[8]);
    msub_sse4_1(d_is[1], d_is[9], &deltas[9]);
    msub_sse4_1(d_is[0], d_is[10], &deltas[10]);
    msub_sse4_1(d_is[1], d_is[11], &deltas[11]);

    msub_sse4_1(d_is[2], d_is[2], &deltas[12]);
    msub_sse4_1(d_is[3], d_is[3], &deltas[13]);
    msub_sse4_1(d_is[2], d_is[4], &deltas[14]);
    msub_sse4_1(d_is[3], d_is[5], &deltas[15]);
    msub_sse4_1(d_is[2], d_is[6], &deltas[16]);
    msub_sse4_1(d_is[3], d_is[7], &deltas[17]);
    msub_sse4_1(d_is[2], d_is[8], &deltas[18]);
    msub_sse4_1(d_is[3], d_is[9], &deltas[19]);
    msub_sse4_1(d_is[2], d_is[10], &deltas[20]);
    msub_sse4_1(d_is[3], d_is[11], &deltas[21]);

    msub_sse4_1(d_is[4], d_is[4], &deltas[22]);
    msub_sse4_1(d_is[5], d_is[5], &deltas[23]);
    msub_sse4_1(d_is[4], d_is[6], &deltas[24]);
    msub_sse4_1(d_is[5], d_is[7], &deltas[25]);
    msub_sse4_1(d_is[4], d_is[8], &deltas[26]);
    msub_sse4_1(d_is[5], d_is[9], &deltas[27]);
    msub_sse4_1(d_is[4], d_is[10], &deltas[28]);
    msub_sse4_1(d_is[5], d_is[11], &deltas[29]);

    msub_sse4_1(d_is[6], d_is[6], &deltas[30]);
    msub_sse4_1(d_is[7], d_is[7], &deltas[31]);
    msub_sse4_1(d_is[6], d_is[8], &deltas[32]);
    msub_sse4_1(d_is[7], d_is[9], &deltas[33]);
    msub_sse4_1(d_is[6], d_is[10], &deltas[34]);
    msub_sse4_1(d_is[7], d_is[11], &deltas[35]);

    msub_sse4_1(d_is[8], d_is[8], &deltas[36]);
    msub_sse4_1(d_is[9], d_is[9], &deltas[37]);
    msub_sse4_1(d_is[8], d_is[10], &deltas[38]);
    msub_sse4_1(d_is[9], d_is[11], &deltas[39]);

    msub_sse4_1(d_is[10], d_is[10], &deltas[40]);
    msub_sse4_1(d_is[11], d_is[11], &deltas[41]);

    madd_sse4_1(d_ie[0], d_ie[0], &deltas[0]);
    madd_sse4_1(d_ie[1], d_ie[1], &deltas[1]);
    madd_sse4_1(d_ie[0], d_ie[2], &deltas[2]);
    madd_sse4_1(d_ie[1], d_ie[3], &deltas[3]);
    madd_sse4_1(d_ie[0], d_ie[4], &deltas[4]);
    madd_sse4_1(d_ie[1], d_ie[5], &deltas[5]);
    madd_sse4_1(d_ie[0], d_ie[6], &deltas[6]);
    madd_sse4_1(d_ie[1], d_ie[7], &deltas[7]);
    madd_sse4_1(d_ie[0], d_ie[8], &deltas[8]);
    madd_sse4_1(d_ie[1], d_ie[9], &deltas[9]);
    madd_sse4_1(d_ie[0], d_ie[10], &deltas[10]);
    madd_sse4_1(d_ie[1], d_ie[11], &deltas[11]);

    madd_sse4_1(d_ie[2], d_ie[2], &deltas[12]);
    madd_sse4_1(d_ie[3], d_ie[3], &deltas[13]);
    madd_sse4_1(d_ie[2], d_ie[4], &deltas[14]);
    madd_sse4_1(d_ie[3], d_ie[5], &deltas[15]);
    madd_sse4_1(d_ie[2], d_ie[6], &deltas[16]);
    madd_sse4_1(d_ie[3], d_ie[7], &deltas[17]);
    madd_sse4_1(d_ie[2], d_ie[8], &deltas[18]);
    madd_sse4_1(d_ie[3], d_ie[9], &deltas[19]);
    madd_sse4_1(d_ie[2], d_ie[10], &deltas[20]);
    madd_sse4_1(d_ie[3], d_ie[11], &deltas[21]);

    madd_sse4_1(d_ie[4], d_ie[4], &deltas[22]);
    madd_sse4_1(d_ie[5], d_ie[5], &deltas[23]);
    madd_sse4_1(d_ie[4], d_ie[6], &deltas[24]);
    madd_sse4_1(d_ie[5], d_ie[7], &deltas[25]);
    madd_sse4_1(d_ie[4], d_ie[8], &deltas[26]);
    madd_sse4_1(d_ie[5], d_ie[9], &deltas[27]);
    madd_sse4_1(d_ie[4], d_ie[10], &deltas[28]);
    madd_sse4_1(d_ie[5], d_ie[11], &deltas[29]);

    madd_sse4_1(d_ie[6], d_ie[6], &deltas[30]);
    madd_sse4_1(d_ie[7], d_ie[7], &deltas[31]);
    madd_sse4_1(d_ie[6], d_ie[8], &deltas[32]);
    madd_sse4_1(d_ie[7], d_ie[9], &deltas[33]);
    madd_sse4_1(d_ie[6], d_ie[10], &deltas[34]);
    madd_sse4_1(d_ie[7], d_ie[11], &deltas[35]);

    madd_sse4_1(d_ie[8], d_ie[8], &deltas[36]);
    madd_sse4_1(d_ie[9], d_ie[9], &deltas[37]);
    madd_sse4_1(d_ie[8], d_ie[10], &deltas[38]);
    madd_sse4_1(d_ie[9], d_ie[11], &deltas[39]);

    madd_sse4_1(d_ie[10], d_ie[10], &deltas[40]);
    madd_sse4_1(d_ie[11], d_ie[11], &deltas[41]);
}

static INLINE __m128i hadd_two_32_to_64_sse4_1(const __m128i src0, const __m128i src1) {
    __m128i s[4];

    s[0] = _mm_cvtepi32_epi64(src0);
    s[1] = _mm_cvtepi32_epi64(src1);
    s[2] = _mm_cvtepi32_epi64(_mm_srli_si128(src0, 8));
    s[3] = _mm_cvtepi32_epi64(_mm_srli_si128(src1, 8));

    s[0] = _mm_add_epi64(s[0], s[1]);
    s[2] = _mm_add_epi64(s[2], s[3]);
    s[0] = _mm_add_epi64(s[0], s[2]);

    return _mm_add_epi64(s[0], _mm_srli_si128(s[0], 8));
}

static INLINE __m128i hadd_2_two_64_sse4_1(const __m128i src0, const __m128i src1) {
    __m128i s[2];

    s[0] = _mm_add_epi64(src0, _mm_srli_si128(src0, 8));
    s[1] = _mm_add_epi64(src1, _mm_srli_si128(src1, 8));
    return _mm_add_epi64(s[0], s[1]);
}

static INLINE __m128i hadd_four_32_sse4_1(const __m128i src0, const __m128i src1, const __m128i src2,
                                          const __m128i src3) {
    const __m128i s0  = _mm_hadd_epi32(src0, src1);
    const __m128i s1  = _mm_hadd_epi32(src2, src3);
    const __m128i s01 = _mm_hadd_epi32(s0, s1);
    return _mm_hadd_epi32(s01, s01);
}

static INLINE void load_more_16_sse4_1(const int16_t* const src, const int32_t width, const __m128i org[2],
                                       __m128i dst[2]) {
    dst[0] = _mm_srli_si128(org[0], 2);
    dst[1] = _mm_srli_si128(org[1], 2);
    dst[0] = _mm_insert_epi16(dst[0], *(int32_t*)src, 7);
    dst[1] = _mm_insert_epi16(dst[1], *(int32_t*)(src + width), 7);
}

static void step3_win5_sse4_1(const int16_t** const d, const int32_t d_stride, const int32_t width,
                              const int32_t height, __m128i* const dd, __m128i* ds, __m128i* deltas) {
    // 16-bit idx: 0, 4, 1, 5, 2, 6, 3, 7
    const __m128i shf = _mm_setr_epi8(0, 1, 8, 9, 2, 3, 10, 11, 4, 5, 12, 13, 6, 7, 14, 15);

    int32_t y = height;
    do {
        *d += 2 * d_stride;

        // 30s 31s 32s 33s 40s 41s 42s 43s  30e 31e 32e 33e 40e 41e 42e 43e
        load_more_64_sse4_1(*d + 2 * d_stride, width, dd);
        // 30s 40s 31s 41s 32s 42s 33s 43s  30e 40e 31e 41e 32e 42e 33e 43e
        ds[6] = _mm_shuffle_epi8(dd[0], shf);
        ds[7] = _mm_shuffle_epi8(dd[1], shf);

        // 40s 41s 42s 43s 50s 51s 52s 53s  40e 41e 42e 43e 50e 51e 52e 53e
        load_more_64_sse4_1(*d + 3 * d_stride, width, dd);
        // 40s 50s 41s 51s 42s 52s 43s 53s  40e 50e 41e 51e 42e 52e 43e 53e
        ds[8] = _mm_shuffle_epi8(dd[0], shf);
        ds[9] = _mm_shuffle_epi8(dd[1], shf);

        madd_sse4_1(ds[0], ds[0], &deltas[0]);
        madd_sse4_1(ds[1], ds[1], &deltas[1]);
        madd_sse4_1(ds[0], ds[2], &deltas[2]);
        madd_sse4_1(ds[1], ds[3], &deltas[3]);
        madd_sse4_1(ds[0], ds[4], &deltas[4]);
        madd_sse4_1(ds[1], ds[5], &deltas[5]);
        madd_sse4_1(ds[0], ds[6], &deltas[6]);
        madd_sse4_1(ds[1], ds[7], &deltas[7]);
        madd_sse4_1(ds[0], ds[8], &deltas[8]);
        madd_sse4_1(ds[1], ds[9], &deltas[9]);

        ds[0] = ds[4];
        ds[1] = ds[5];
        ds[2] = ds[6];
        ds[3] = ds[7];
        ds[4] = ds[8];
        ds[5] = ds[9];
        y -= 2;
    } while (y);
}

static void step3_win7_sse4_1(const int16_t** const d, const int32_t d_stride, const int32_t width,
                              const int32_t height, __m128i* ds, __m128i* deltas) {
    const __m128i const_n1_0 = _mm_setr_epi16(0xFFFF, 0, 0xFFFF, 0, 0xFFFF, 0, 0xFFFF, 0);

    int32_t y = height;
    do {
        __m128i dd[2];

        dd[0] = ds[0];
        dd[1] = ds[1];
        dd[0] = _mm_xor_si128(dd[0], const_n1_0);
        dd[1] = _mm_xor_si128(dd[1], const_n1_0);
        dd[0] = _mm_sub_epi16(dd[0], const_n1_0);
        dd[1] = _mm_sub_epi16(dd[1], const_n1_0);

        // 60s 60e 61s 61e 62s 62e 63s 63e  64s 64e 65s 65e 66s 66e 67s 67e
        load_win7_sse4_1(*d, width, &ds[12]);

        madd_sse4_1(dd[0], ds[0], &deltas[0]);
        madd_sse4_1(dd[1], ds[1], &deltas[1]);
        madd_sse4_1(dd[0], ds[2], &deltas[2]);
        madd_sse4_1(dd[1], ds[3], &deltas[3]);
        madd_sse4_1(dd[0], ds[4], &deltas[4]);
        madd_sse4_1(dd[1], ds[5], &deltas[5]);
        madd_sse4_1(dd[0], ds[6], &deltas[6]);
        madd_sse4_1(dd[1], ds[7], &deltas[7]);
        madd_sse4_1(dd[0], ds[8], &deltas[8]);
        madd_sse4_1(dd[1], ds[9], &deltas[9]);
        madd_sse4_1(dd[0], ds[10], &deltas[10]);
        madd_sse4_1(dd[1], ds[11], &deltas[11]);
        madd_sse4_1(dd[0], ds[12], &deltas[12]);
        madd_sse4_1(dd[1], ds[13], &deltas[13]);

        ds[0]  = ds[2];
        ds[1]  = ds[3];
        ds[2]  = ds[4];
        ds[3]  = ds[5];
        ds[4]  = ds[6];
        ds[5]  = ds[7];
        ds[6]  = ds[8];
        ds[7]  = ds[9];
        ds[8]  = ds[10];
        ds[9]  = ds[11];
        ds[10] = ds[12];
        ds[11] = ds[13];

        *d += d_stride;
    } while (--y);
}

static void step3_win5_oneline_sse4_1(const int16_t** const d, const int32_t d_stride, const int32_t width,
                                      const int32_t height, __m128i* ds, __m128i* deltas) {
    const __m128i const_n1_0 = _mm_setr_epi16(0xFFFF, 0, 0xFFFF, 0, 0xFFFF, 0, 0xFFFF, 0);

    int32_t y = height;
    do {
        __m128i dd1, dd2;

        dd1 = ds[0];
        dd2 = ds[1];
        dd1 = _mm_xor_si128(dd1, const_n1_0);
        dd2 = _mm_xor_si128(dd2, const_n1_0);
        dd1 = _mm_sub_epi16(dd1, const_n1_0);
        dd2 = _mm_sub_epi16(dd2, const_n1_0);

        // 60s 60e 61s 61e 62s 62e 63s 63e  64s 64e 65s 65e 66s 66e 67s 67e
        load_win7_sse4_1(*d, width, &ds[8]);

        madd_sse4_1(dd1, ds[0], &deltas[0]);
        madd_sse4_1(dd2, ds[1], &deltas[1]);
        madd_sse4_1(dd1, ds[2], &deltas[2]);
        madd_sse4_1(dd2, ds[3], &deltas[3]);
        madd_sse4_1(dd1, ds[4], &deltas[4]);
        madd_sse4_1(dd2, ds[5], &deltas[5]);
        madd_sse4_1(dd1, ds[6], &deltas[6]);
        madd_sse4_1(dd2, ds[7], &deltas[7]);
        madd_sse4_1(dd1, ds[8], &deltas[8]);
        madd_sse4_1(dd2, ds[9], &deltas[9]);

        ds[0] = ds[2];
        ds[1] = ds[3];
        ds[2] = ds[4];
        ds[3] = ds[5];
        ds[4] = ds[6];
        ds[5] = ds[7];
        ds[6] = ds[8];
        ds[7] = ds[9];

        *d += d_stride;
    } while (--y);
}

static INLINE void update_2_stats_sse2(const int64_t* const src, const __m128i delta, int64_t* const dst) {
    const __m128i s = _mm_loadu_si128((__m128i*)src);
    const __m128i d = _mm_add_epi64(s, delta);
    _mm_storeu_si128((__m128i*)dst, d);
}

static INLINE void update_4_stats_sse4_1(const int64_t* const src, const __m128i delta, int64_t* const dst) {
    const __m128i s1 = _mm_loadu_si128((__m128i*)src);
    const __m128i s2 = _mm_loadu_si128((__m128i*)(src + 2));

    const __m128i dlt_1 = _mm_cvtepi32_epi64(delta);
    const __m128i dlt_2 = _mm_cvtepi32_epi64(_mm_srli_si128(delta, 8));

    const __m128i d1 = _mm_add_epi64(s1, dlt_1);
    const __m128i d2 = _mm_add_epi64(s2, dlt_2);

    _mm_storeu_si128((__m128i*)dst, d1);
    _mm_storeu_si128((__m128i*)(dst + 2), d2);
}

static INLINE void update_5_stats_sse4_1(const int64_t* const src, const __m128i delta, const int64_t delta4,
                                         int64_t* const dst) {
    update_4_stats_sse4_1(src + 0, delta, dst + 0);
    dst[4] = src[4] + delta4;
}

static INLINE void update_8_stats_sse4_1(const int64_t* const src, const __m128i* delta, int64_t* const dst) {
    update_4_stats_sse4_1(src + 0, delta[0], dst + 0);
    update_4_stats_sse4_1(src + 4, delta[1], dst + 4);
}

static INLINE void hadd_update_4_stats_sse4_1(const int64_t* const src, const __m128i* deltas, int64_t* const dst) {
    const __m128i delta1 = hadd_four_32_sse4_1(deltas[0], deltas[1], deltas[2], deltas[3]);
    const __m128i delta2 = hadd_four_32_sse4_1(deltas[4], deltas[5], deltas[6], deltas[7]);
    update_2_stats_sse2(src, _mm_cvtepi32_epi64(delta1), dst);
    update_2_stats_sse2(src + 2, _mm_cvtepi32_epi64(delta2), dst + 2);
}

static INLINE void hadd_update_6_stats_sse4_1(const int64_t* const src, const __m128i* deltas, int64_t* const dst) {
    const __m128i delta1 = hadd_four_32_sse4_1(deltas[0], deltas[1], deltas[2], deltas[3]);
    const __m128i delta2 = hadd_four_32_sse4_1(deltas[4], deltas[5], deltas[6], deltas[7]);
    const __m128i delta3 = hadd_four_32_sse4_1(deltas[8], deltas[9], deltas[10], deltas[11]);
    update_2_stats_sse2(src, _mm_cvtepi32_epi64(delta1), dst);
    update_2_stats_sse2(src + 2, _mm_cvtepi32_epi64(delta2), dst + 2);
    update_2_stats_sse2(src + 4, _mm_cvtepi32_epi64(delta3), dst + 4);
}

static INLINE void shift_right_4b_2x128(__m128i vec[2]) {
    int32_t tmp1 = _mm_cvtsi128_si32(vec[0]);
    int32_t tmp2 = _mm_cvtsi128_si32(vec[1]);
    vec[0]       = _mm_srli_si128(vec[0], 4);
    vec[1]       = _mm_srli_si128(vec[1], 4);
    vec[0]       = _mm_insert_epi32(vec[0], tmp2, 3);
    vec[1]       = _mm_insert_epi32(vec[1], tmp1, 3);
}

static void compute_stats_win5_sse4_1(const int16_t* const d, const int32_t d_stride, const int16_t* const s,
                                      const int32_t s_stride, const int32_t width, const int32_t height,
                                      int64_t* const M, int64_t* const H, EbBitDepth bit_depth) {
    const int32_t wiener_win  = WIENER_WIN_CHROMA;
    const int32_t wiener_win2 = wiener_win * wiener_win;
    const int32_t w16         = width & ~15;
    const int32_t h8          = height & ~7;
    const __m128i mask[2]     = {_mm_loadu_si128((__m128i*)(mask_16bit[width - w16])),
                                 _mm_loadu_si128((__m128i*)(mask_16bit[width - w16] + 8))};
    int32_t       i, j, x, y;

    if (bit_depth == EB_EIGHT_BIT) {
        // Step 1: Calculate the top edge of the whole matrix, i.e., the top
        // edge of each triangle and square on the top row.
        j = 0;
        do {
            const int16_t* s_t                          = s;
            const int16_t* d_t                          = d;
            __m128i        sum_m[WIENER_WIN_CHROMA * 2] = {_mm_setzero_si128()};
            __m128i        sum_h[WIENER_WIN_CHROMA * 2] = {_mm_setzero_si128()};
            __m128i        src[2], dgd[2];

            y = height;
            do {
                x = 0;
                while (x < w16) {
                    src[0] = _mm_loadu_si128((__m128i*)(s_t + x));
                    src[1] = _mm_loadu_si128((__m128i*)(s_t + x + 8));
                    dgd[0] = _mm_loadu_si128((__m128i*)(d_t + x));
                    dgd[1] = _mm_loadu_si128((__m128i*)(d_t + x + 8));
                    stats_top_win5_sse4_1(src, dgd, d_t + j + x, d_stride, sum_m, sum_h);
                    x += 16;
                };

                if (w16 != width) {
                    src[0] = _mm_loadu_si128((__m128i*)(s_t + w16));
                    src[1] = _mm_loadu_si128((__m128i*)(s_t + w16 + 8));
                    dgd[0] = _mm_loadu_si128((__m128i*)(d_t + w16));
                    dgd[1] = _mm_loadu_si128((__m128i*)(d_t + w16 + 8));
                    src[0] = _mm_and_si128(src[0], mask[0]);
                    src[1] = _mm_and_si128(src[1], mask[1]);
                    dgd[0] = _mm_and_si128(dgd[0], mask[0]);
                    dgd[1] = _mm_and_si128(dgd[1], mask[1]);
                    stats_top_win5_sse4_1(src, dgd, d_t + j + w16, d_stride, sum_m, sum_h);
                }

                s_t += s_stride;
                d_t += d_stride;
            } while (--y);

            _mm_storel_epi64((__m128i*)&M[wiener_win * j], hadd_two_32_to_64_sse4_1(sum_m[0], sum_m[1]));
            _mm_storel_epi64((__m128i*)&M[wiener_win * j + 1], hadd_two_32_to_64_sse4_1(sum_m[2], sum_m[3]));
            _mm_storel_epi64((__m128i*)&M[wiener_win * j + 2], hadd_two_32_to_64_sse4_1(sum_m[4], sum_m[5]));
            _mm_storel_epi64((__m128i*)&M[wiener_win * j + 3], hadd_two_32_to_64_sse4_1(sum_m[6], sum_m[7]));
            _mm_storel_epi64((__m128i*)&M[wiener_win * j + 4], hadd_two_32_to_64_sse4_1(sum_m[8], sum_m[9]));
            _mm_storel_epi64((__m128i*)(H + wiener_win * j), hadd_two_32_to_64_sse4_1(sum_h[0], sum_h[1]));
            _mm_storel_epi64((__m128i*)(H + wiener_win * j + 1), hadd_two_32_to_64_sse4_1(sum_h[2], sum_h[3]));
            _mm_storel_epi64((__m128i*)(H + wiener_win * j + 2), hadd_two_32_to_64_sse4_1(sum_h[4], sum_h[5]));
            _mm_storel_epi64((__m128i*)(H + wiener_win * j + 3), hadd_two_32_to_64_sse4_1(sum_h[6], sum_h[7]));
            _mm_storel_epi64((__m128i*)(H + wiener_win * j + 4), hadd_two_32_to_64_sse4_1(sum_h[8], sum_h[9]));
        } while (++j < wiener_win);

        // Step 2: Calculate the left edge of each square on the top row.
        j = 1;
        do {
            const int16_t* d_t               = d;
            __m128i        sum_h[WIN_CHROMA] = {_mm_setzero_si128()};
            __m128i        dgd[2];

            y = height;
            do {
                x = 0;
                while (x < w16) {
                    dgd[0] = _mm_loadu_si128((__m128i*)(d_t + j + x));
                    dgd[1] = _mm_loadu_si128((__m128i*)(d_t + j + x + 8));
                    stats_left_win5_sse4_1(dgd, d_t + x, d_stride, sum_h);
                    x += 16;
                };

                if (w16 != width) {
                    dgd[0] = _mm_loadu_si128((__m128i*)(d_t + j + x));
                    dgd[1] = _mm_loadu_si128((__m128i*)(d_t + j + x + 8));
                    dgd[0] = _mm_and_si128(dgd[0], mask[0]);
                    dgd[1] = _mm_and_si128(dgd[1], mask[1]);
                    stats_left_win5_sse4_1(dgd, d_t + x, d_stride, sum_h);
                }

                d_t += d_stride;
            } while (--y);

            _mm_storel_epi64((__m128i*)&H[1 * wiener_win2 + j * wiener_win],
                             hadd_two_32_to_64_sse4_1(sum_h[0], sum_h[1]));
            _mm_storel_epi64((__m128i*)&H[2 * wiener_win2 + j * wiener_win],
                             hadd_two_32_to_64_sse4_1(sum_h[2], sum_h[3]));
            _mm_storel_epi64((__m128i*)&H[3 * wiener_win2 + j * wiener_win],
                             hadd_two_32_to_64_sse4_1(sum_h[4], sum_h[5]));
            _mm_storel_epi64((__m128i*)&H[4 * wiener_win2 + j * wiener_win],
                             hadd_two_32_to_64_sse4_1(sum_h[6], sum_h[7]));
        } while (++j < wiener_win);
    } else {
        const int32_t num_bit_left = 32 - 1 /* sign */ - 2 * bit_depth /* energy */ + 3 /* SIMD */;
        const int32_t h_allowed    = (1 << num_bit_left) / (w16 + ((w16 != width) ? 16 : 0));

        // Step 1: Calculate the top edge of the whole matrix, i.e., the top
        // edge of each triangle and square on the top row.
        j = 0;
        do {
            const int16_t* s_t                          = s;
            const int16_t* d_t                          = d;
            int32_t        height_t                     = 0;
            __m128i        sum_m[WIENER_WIN_CHROMA * 2] = {_mm_setzero_si128()};
            __m128i        sum_h[WIENER_WIN_CHROMA * 2] = {_mm_setzero_si128()};
            __m128i        src[2], dgd[2];

            do {
                const int32_t h_t = ((height - height_t) < h_allowed) ? (height - height_t) : h_allowed;
                __m128i       row_m[WIENER_WIN_CHROMA * 2] = {_mm_setzero_si128()};
                __m128i       row_h[WIENER_WIN_CHROMA * 2] = {_mm_setzero_si128()};

                y = h_t;
                do {
                    x = 0;
                    while (x < w16) {
                        src[0] = _mm_loadu_si128((__m128i*)(s_t + x));
                        src[1] = _mm_loadu_si128((__m128i*)(s_t + x + 8));
                        dgd[0] = _mm_loadu_si128((__m128i*)(d_t + x));
                        dgd[1] = _mm_loadu_si128((__m128i*)(d_t + x + 8));
                        stats_top_win5_sse4_1(src, dgd, d_t + j + x, d_stride, row_m, row_h);
                        x += 16;
                    };

                    if (w16 != width) {
                        src[0] = _mm_loadu_si128((__m128i*)(s_t + w16));
                        src[1] = _mm_loadu_si128((__m128i*)(s_t + w16 + 8));
                        dgd[0] = _mm_loadu_si128((__m128i*)(d_t + w16));
                        dgd[1] = _mm_loadu_si128((__m128i*)(d_t + w16 + 8));
                        src[0] = _mm_and_si128(src[0], mask[0]);
                        src[1] = _mm_and_si128(src[1], mask[1]);
                        dgd[0] = _mm_and_si128(dgd[0], mask[0]);
                        dgd[1] = _mm_and_si128(dgd[1], mask[1]);
                        stats_top_win5_sse4_1(src, dgd, d_t + j + w16, d_stride, row_m, row_h);
                    }

                    s_t += s_stride;
                    d_t += d_stride;
                } while (--y);

                add_32_to_64_sse4_1(row_m[0], &sum_m[0]);
                add_32_to_64_sse4_1(row_m[1], &sum_m[1]);
                add_32_to_64_sse4_1(row_m[2], &sum_m[2]);
                add_32_to_64_sse4_1(row_m[3], &sum_m[3]);
                add_32_to_64_sse4_1(row_m[4], &sum_m[4]);
                add_32_to_64_sse4_1(row_m[5], &sum_m[5]);
                add_32_to_64_sse4_1(row_m[6], &sum_m[6]);
                add_32_to_64_sse4_1(row_m[7], &sum_m[7]);
                add_32_to_64_sse4_1(row_m[8], &sum_m[8]);
                add_32_to_64_sse4_1(row_m[9], &sum_m[9]);
                add_32_to_64_sse4_1(row_h[0], &sum_h[0]);
                add_32_to_64_sse4_1(row_h[1], &sum_h[1]);
                add_32_to_64_sse4_1(row_h[2], &sum_h[2]);
                add_32_to_64_sse4_1(row_h[3], &sum_h[3]);
                add_32_to_64_sse4_1(row_h[4], &sum_h[4]);
                add_32_to_64_sse4_1(row_h[5], &sum_h[5]);
                add_32_to_64_sse4_1(row_h[6], &sum_h[6]);
                add_32_to_64_sse4_1(row_h[7], &sum_h[7]);
                add_32_to_64_sse4_1(row_h[8], &sum_h[8]);
                add_32_to_64_sse4_1(row_h[9], &sum_h[9]);

                height_t += h_t;
            } while (height_t < height);

            _mm_storel_epi64((__m128i*)&M[wiener_win * j], hadd_2_two_64_sse4_1(sum_m[0], sum_m[1]));
            _mm_storel_epi64((__m128i*)&M[wiener_win * j + 1], hadd_2_two_64_sse4_1(sum_m[2], sum_m[3]));
            _mm_storel_epi64((__m128i*)&M[wiener_win * j + 2], hadd_2_two_64_sse4_1(sum_m[4], sum_m[5]));
            _mm_storel_epi64((__m128i*)&M[wiener_win * j + 3], hadd_2_two_64_sse4_1(sum_m[6], sum_m[7]));
            _mm_storel_epi64((__m128i*)&M[wiener_win * j + 4], hadd_2_two_64_sse4_1(sum_m[8], sum_m[9]));
            _mm_storel_epi64((__m128i*)(H + wiener_win * j), hadd_2_two_64_sse4_1(sum_h[0], sum_h[1]));
            _mm_storel_epi64((__m128i*)(H + wiener_win * j + 1), hadd_2_two_64_sse4_1(sum_h[2], sum_h[3]));
            _mm_storel_epi64((__m128i*)(H + wiener_win * j + 2), hadd_2_two_64_sse4_1(sum_h[4], sum_h[5]));
            _mm_storel_epi64((__m128i*)(H + wiener_win * j + 3), hadd_2_two_64_sse4_1(sum_h[6], sum_h[7]));
            _mm_storel_epi64((__m128i*)(H + wiener_win * j + 4), hadd_2_two_64_sse4_1(sum_h[8], sum_h[9]));
        } while (++j < wiener_win);

        // Step 2: Calculate the left edge of each square on the top row.
        j = 1;
        do {
            const int16_t* d_t               = d;
            int32_t        height_t          = 0;
            __m128i        sum_h[WIN_CHROMA] = {_mm_setzero_si128()};
            __m128i        dgd[2];

            do {
                const int32_t h_t               = ((height - height_t) < h_allowed) ? (height - height_t) : h_allowed;
                __m128i       row_h[WIN_CHROMA] = {_mm_setzero_si128()};

                y = h_t;
                do {
                    x = 0;
                    while (x < w16) {
                        dgd[0] = _mm_loadu_si128((__m128i*)(d_t + j + x));
                        dgd[1] = _mm_loadu_si128((__m128i*)(d_t + j + x + 8));
                        stats_left_win5_sse4_1(dgd, d_t + x, d_stride, row_h);
                        x += 16;
                    };

                    if (w16 != width) {
                        dgd[0] = _mm_loadu_si128((__m128i*)(d_t + j + x));
                        dgd[1] = _mm_loadu_si128((__m128i*)(d_t + j + x + 8));
                        dgd[0] = _mm_and_si128(dgd[0], mask[0]);
                        dgd[1] = _mm_and_si128(dgd[1], mask[1]);
                        stats_left_win5_sse4_1(dgd, d_t + x, d_stride, row_h);
                    }

                    d_t += d_stride;
                } while (--y);

                add_32_to_64_sse4_1(row_h[0], &sum_h[0]);
                add_32_to_64_sse4_1(row_h[1], &sum_h[1]);
                add_32_to_64_sse4_1(row_h[2], &sum_h[2]);
                add_32_to_64_sse4_1(row_h[3], &sum_h[3]);
                add_32_to_64_sse4_1(row_h[4], &sum_h[4]);
                add_32_to_64_sse4_1(row_h[5], &sum_h[5]);
                add_32_to_64_sse4_1(row_h[6], &sum_h[6]);
                add_32_to_64_sse4_1(row_h[7], &sum_h[7]);

                height_t += h_t;
            } while (height_t < height);

            _mm_storel_epi64((__m128i*)&H[1 * wiener_win2 + j * wiener_win], hadd_2_two_64_sse4_1(sum_h[0], sum_h[1]));
            _mm_storel_epi64((__m128i*)&H[2 * wiener_win2 + j * wiener_win], hadd_2_two_64_sse4_1(sum_h[2], sum_h[3]));
            _mm_storel_epi64((__m128i*)&H[3 * wiener_win2 + j * wiener_win], hadd_2_two_64_sse4_1(sum_h[4], sum_h[5]));
            _mm_storel_epi64((__m128i*)&H[4 * wiener_win2 + j * wiener_win], hadd_2_two_64_sse4_1(sum_h[6], sum_h[7]));
        } while (++j < wiener_win);
    }

    // Step 3: Derive the top edge of each triangle along the diagonal. No
    // triangle in top row.
    {
        const int16_t* d_t = d;

        if (height % 2) {
            __m128i deltas[(WIENER_WIN + 1) * 2] = {_mm_setzero_si128()};
            __m128i ds[WIENER_WIN * 2];

            load_win7_sse4_1(d_t + 0 * d_stride, width, &ds[0]);
            load_win7_sse4_1(d_t + 1 * d_stride, width, &ds[2]);
            load_win7_sse4_1(d_t + 2 * d_stride, width, &ds[4]);
            load_win7_sse4_1(d_t + 3 * d_stride, width, &ds[6]);
            d_t += 4 * d_stride;

            step3_win5_oneline_sse4_1(&d_t, d_stride, width, height, ds, deltas);
            transpose_32bit_8x8_sse2(deltas, deltas);

            update_5_stats_sse4_1(H + 0 * wiener_win * wiener_win2 + 0 * wiener_win,
                                  deltas[0],
                                  _mm_cvtsi128_si32(deltas[1]),
                                  H + 1 * wiener_win * wiener_win2 + 1 * wiener_win);

            update_5_stats_sse4_1(H + 1 * wiener_win * wiener_win2 + 1 * wiener_win,
                                  deltas[2],
                                  _mm_cvtsi128_si32(deltas[3]),
                                  H + 2 * wiener_win * wiener_win2 + 2 * wiener_win);

            update_5_stats_sse4_1(H + 2 * wiener_win * wiener_win2 + 2 * wiener_win,
                                  deltas[4],
                                  _mm_cvtsi128_si32(deltas[5]),
                                  H + 3 * wiener_win * wiener_win2 + 3 * wiener_win);

            update_5_stats_sse4_1(H + 3 * wiener_win * wiener_win2 + 3 * wiener_win,
                                  deltas[6],
                                  _mm_cvtsi128_si32(deltas[7]),
                                  H + 4 * wiener_win * wiener_win2 + 4 * wiener_win);

        } else {
            // 16-bit idx: 0, 4, 1, 5, 2, 6, 3, 7
            const __m128i shf = _mm_setr_epi8(0, 1, 8, 9, 2, 3, 10, 11, 4, 5, 12, 13, 6, 7, 14, 15);
            __m128i       deltas[WIENER_WIN_CHROMA * 2] = {_mm_setzero_si128()};
            __m128i       dd[2]                         = {_mm_setzero_si128()}; // Initialize to avoid warning.
            __m128i       ds[WIENER_WIN_CHROMA * 2];

            // 00s 01s 02s 03s 10s 11s 12s 13s  00e 01e 02e 03e 10e 11e 12e 13e
            dd[0] = _mm_insert_epi64(dd[0], *(int64_t*)(d_t + 0 * d_stride), 0);
            dd[1] = _mm_insert_epi64(dd[1], *(int64_t*)(d_t + 0 * d_stride + width), 0);
            dd[0] = _mm_insert_epi64(dd[0], *(int64_t*)(d_t + 1 * d_stride), 1);
            dd[1] = _mm_insert_epi64(dd[1], *(int64_t*)(d_t + 1 * d_stride + width), 1);
            // 00s 10s 01s 11s 02s 12s 03s 13s  00e 10e 01e 11e 02e 12e 03e 13e
            ds[0] = _mm_shuffle_epi8(dd[0], shf);
            ds[1] = _mm_shuffle_epi8(dd[1], shf);

            // 10s 11s 12s 13s 20s 21s 22s 23s  10e 11e 12e 13e 20e 21e 22e 23e
            load_more_64_sse4_1(d_t + 2 * d_stride, width, dd);
            // 10s 20s 11s 21s 12s 22s 13s 23s  10e 20e 11e 21e 12e 22e 13e 23e
            ds[2] = _mm_shuffle_epi8(dd[0], shf);
            ds[3] = _mm_shuffle_epi8(dd[1], shf);

            // 20s 21s 22s 23s 30s 31s 32s 33s  20e 21e 22e 23e 30e 31e 32e 33e
            load_more_64_sse4_1(d_t + 3 * d_stride, width, dd);
            // 20s 30s 21s 31s 22s 32s 23s 33s  20e 30e 21e 31e 22e 32e 23e 33e
            ds[4] = _mm_shuffle_epi8(dd[0], shf);
            ds[5] = _mm_shuffle_epi8(dd[1], shf);

            step3_win5_sse4_1(&d_t, d_stride, width, height, dd, ds, deltas);

            deltas[0] = _mm_sub_epi32(deltas[1], deltas[0]);
            deltas[1] = _mm_sub_epi32(deltas[3], deltas[2]);
            deltas[2] = _mm_sub_epi32(deltas[5], deltas[4]);
            deltas[3] = _mm_sub_epi32(deltas[7], deltas[6]);
            deltas[4] = _mm_sub_epi32(deltas[9], deltas[8]);

            transpose_32bit_4x4(deltas, deltas);

            update_5_stats_sse4_1(H + 0 * wiener_win * wiener_win2 + 0 * wiener_win,
                                  deltas[0],
                                  _mm_extract_epi32(deltas[4], 0),
                                  H + 1 * wiener_win * wiener_win2 + 1 * wiener_win);

            update_5_stats_sse4_1(H + 1 * wiener_win * wiener_win2 + 1 * wiener_win,
                                  deltas[1],
                                  _mm_extract_epi32(deltas[4], 1),
                                  H + 2 * wiener_win * wiener_win2 + 2 * wiener_win);

            update_5_stats_sse4_1(H + 2 * wiener_win * wiener_win2 + 2 * wiener_win,
                                  deltas[2],
                                  _mm_extract_epi32(deltas[4], 2),
                                  H + 3 * wiener_win * wiener_win2 + 3 * wiener_win);

            update_5_stats_sse4_1(H + 3 * wiener_win * wiener_win2 + 3 * wiener_win,
                                  deltas[3],
                                  _mm_extract_epi32(deltas[4], 3),
                                  H + 4 * wiener_win * wiener_win2 + 4 * wiener_win);
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
            __m128i        delta[3];
            __m128i        deltas[(2 * WIENER_WIN_CHROMA - 1) * 2] = {_mm_setzero_si128()};
            __m128i        dd[WIENER_WIN_CHROMA * 2], ds[WIENER_WIN_CHROMA * 2];

            dd[0] = _mm_setzero_si128(); // Initialize to avoid warning.
            dd[1] = _mm_setzero_si128(); // Initialize to avoid warning.
            ds[0] = _mm_setzero_si128(); // Initialize to avoid warning.
            ds[1] = _mm_setzero_si128(); // Initialize to avoid warning.

            dd[0] = _mm_insert_epi16(dd[0], di[0 * d_stride], 0);
            dd[0] = _mm_insert_epi16(dd[0], di[1 * d_stride], 1);
            dd[0] = _mm_insert_epi16(dd[0], di[2 * d_stride], 2);
            dd[0] = _mm_insert_epi16(dd[0], di[3 * d_stride], 3);
            dd[1] = _mm_insert_epi16(dd[1], di[0 * d_stride + width], 0);
            dd[1] = _mm_insert_epi16(dd[1], di[1 * d_stride + width], 1);
            dd[1] = _mm_insert_epi16(dd[1], di[2 * d_stride + width], 2);
            dd[1] = _mm_insert_epi16(dd[1], di[3 * d_stride + width], 3);

            ds[0] = _mm_insert_epi16(ds[0], d_j[0 * d_stride], 0);
            ds[0] = _mm_insert_epi16(ds[0], d_j[1 * d_stride], 1);
            ds[0] = _mm_insert_epi16(ds[0], d_j[2 * d_stride], 2);
            ds[0] = _mm_insert_epi16(ds[0], d_j[3 * d_stride], 3);
            ds[1] = _mm_insert_epi16(ds[1], d_j[0 * d_stride + width], 0);
            ds[1] = _mm_insert_epi16(ds[1], d_j[1 * d_stride + width], 1);
            ds[1] = _mm_insert_epi16(ds[1], d_j[2 * d_stride + width], 2);
            ds[1] = _mm_insert_epi16(ds[1], d_j[3 * d_stride + width], 3);

            y = 0;
            while (y < h8) {
                // 00s 10s 20s 30s 40s 50s 60s 70s  00e 10e 20e 30e 40e 50e 60e 70e
                dd[0] = _mm_insert_epi16(dd[0], di[4 * d_stride], 4);
                dd[0] = _mm_insert_epi16(dd[0], di[5 * d_stride], 5);
                dd[0] = _mm_insert_epi16(dd[0], di[6 * d_stride], 6);
                dd[0] = _mm_insert_epi16(dd[0], di[7 * d_stride], 7);
                dd[1] = _mm_insert_epi16(dd[1], di[4 * d_stride + width], 4);
                dd[1] = _mm_insert_epi16(dd[1], di[5 * d_stride + width], 5);
                dd[1] = _mm_insert_epi16(dd[1], di[6 * d_stride + width], 6);
                dd[1] = _mm_insert_epi16(dd[1], di[7 * d_stride + width], 7);

                // 01s 11s 21s 31s 41s 51s 61s 71s  01e 11e 21e 31e 41e 51e 61e 71e
                ds[0] = _mm_insert_epi16(ds[0], d_j[4 * d_stride], 4);
                ds[0] = _mm_insert_epi16(ds[0], d_j[5 * d_stride], 5);
                ds[0] = _mm_insert_epi16(ds[0], d_j[6 * d_stride], 6);
                ds[0] = _mm_insert_epi16(ds[0], d_j[7 * d_stride], 7);
                ds[1] = _mm_insert_epi16(ds[1], d_j[4 * d_stride + width], 4);
                ds[1] = _mm_insert_epi16(ds[1], d_j[5 * d_stride + width], 5);
                ds[1] = _mm_insert_epi16(ds[1], d_j[6 * d_stride + width], 6);
                ds[1] = _mm_insert_epi16(ds[1], d_j[7 * d_stride + width], 7);

                load_more_16_sse4_1(di + 8 * d_stride, width, &dd[0], &dd[2]);
                load_more_16_sse4_1(d_j + 8 * d_stride, width, &ds[0], &ds[2]);
                load_more_16_sse4_1(di + 9 * d_stride, width, &dd[2], &dd[4]);
                load_more_16_sse4_1(d_j + 9 * d_stride, width, &ds[2], &ds[4]);
                load_more_16_sse4_1(di + 10 * d_stride, width, &dd[4], &dd[6]);
                load_more_16_sse4_1(d_j + 10 * d_stride, width, &ds[4], &ds[6]);
                load_more_16_sse4_1(di + 11 * d_stride, width, &dd[6], &dd[8]);
                load_more_16_sse4_1(d_j + 11 * d_stride, width, &ds[6], &ds[8]);

                madd_sse4_1(dd[0], ds[0], &deltas[0]);
                madd_sse4_1(dd[1], ds[1], &deltas[1]);
                madd_sse4_1(dd[0], ds[2], &deltas[2]);
                madd_sse4_1(dd[1], ds[3], &deltas[3]);
                madd_sse4_1(dd[0], ds[4], &deltas[4]);
                madd_sse4_1(dd[1], ds[5], &deltas[5]);
                madd_sse4_1(dd[0], ds[6], &deltas[6]);
                madd_sse4_1(dd[1], ds[7], &deltas[7]);
                madd_sse4_1(dd[0], ds[8], &deltas[8]);
                madd_sse4_1(dd[1], ds[9], &deltas[9]);
                madd_sse4_1(dd[2], ds[0], &deltas[10]);
                madd_sse4_1(dd[3], ds[1], &deltas[11]);
                madd_sse4_1(dd[4], ds[0], &deltas[12]);
                madd_sse4_1(dd[5], ds[1], &deltas[13]);
                madd_sse4_1(dd[6], ds[0], &deltas[14]);
                madd_sse4_1(dd[7], ds[1], &deltas[15]);
                madd_sse4_1(dd[8], ds[0], &deltas[16]);
                madd_sse4_1(dd[9], ds[1], &deltas[17]);

                dd[0] = _mm_srli_si128(dd[8], 8);
                dd[1] = _mm_srli_si128(dd[9], 8);
                ds[0] = _mm_srli_si128(ds[8], 8);
                ds[1] = _mm_srli_si128(ds[9], 8);
                di += 8 * d_stride;
                d_j += 8 * d_stride;
                y += 8;
            };

            deltas[0] = _mm_hadd_epi32(deltas[0], deltas[2]);
            deltas[1] = _mm_hadd_epi32(deltas[1], deltas[3]);
            deltas[2] = _mm_hadd_epi32(deltas[4], deltas[6]);
            deltas[3] = _mm_hadd_epi32(deltas[5], deltas[7]);
            deltas[4] = _mm_hadd_epi32(deltas[8], deltas[8]);
            deltas[5] = _mm_hadd_epi32(deltas[9], deltas[9]);
            deltas[6] = _mm_hadd_epi32(deltas[10], deltas[12]);
            deltas[7] = _mm_hadd_epi32(deltas[11], deltas[13]);
            deltas[8] = _mm_hadd_epi32(deltas[14], deltas[16]);
            deltas[9] = _mm_hadd_epi32(deltas[15], deltas[17]);
            deltas[0] = _mm_hadd_epi32(deltas[0], deltas[2]);
            deltas[1] = _mm_hadd_epi32(deltas[1], deltas[3]);
            deltas[2] = _mm_hadd_epi32(deltas[4], deltas[4]);
            deltas[3] = _mm_hadd_epi32(deltas[5], deltas[5]);
            deltas[4] = _mm_hadd_epi32(deltas[6], deltas[8]);
            deltas[5] = _mm_hadd_epi32(deltas[7], deltas[9]);
            delta[0]  = _mm_sub_epi32(deltas[1], deltas[0]);
            delta[1]  = _mm_sub_epi32(deltas[3], deltas[2]);
            delta[2]  = _mm_sub_epi32(deltas[5], deltas[4]);

            if (h8 != height) {
                __m128i dd128, ds128;

                ds[0] = _mm_insert_epi16(ds[0], d_j[0 * d_stride], 0);
                ds[0] = _mm_insert_epi16(ds[0], d_j[0 * d_stride + width], 1);

                dd128 = _mm_cvtsi32_si128(-di[1 * d_stride]);
                dd128 = _mm_insert_epi16(dd128, di[1 * d_stride + width], 1);
                ds[0] = _mm_insert_epi16(ds[0], d_j[1 * d_stride], 2);
                ds[0] = _mm_insert_epi16(ds[0], d_j[1 * d_stride + width], 3);

                dd128 = _mm_insert_epi16(dd128, -di[2 * d_stride], 2);
                dd128 = _mm_insert_epi16(dd128, di[2 * d_stride + width], 3);
                ds[0] = _mm_insert_epi16(ds[0], d_j[2 * d_stride], 4);
                ds[0] = _mm_insert_epi16(ds[0], d_j[2 * d_stride + width], 5);

                dd128 = _mm_insert_epi16(dd128, -di[3 * d_stride], 4);
                dd128 = _mm_insert_epi16(dd128, di[3 * d_stride + width], 5);
                ds[0] = _mm_insert_epi16(ds[0], d_j[3 * d_stride], 6);
                ds[0] = _mm_insert_epi16(ds[0], d_j[3 * d_stride + width], 7);

                do {
                    __m128i t;

                    t     = _mm_cvtsi32_si128(-di[0 * d_stride]);
                    t     = _mm_insert_epi16(t, di[0 * d_stride + width], 1);
                    dd[0] = dd[1] = _mm_set1_epi32(_mm_cvtsi128_si32(t));

                    ds128 = _mm_cvtsi32_si128(d_j[0 * d_stride]);
                    ds128 = _mm_insert_epi16(ds128, d_j[0 * d_stride + width], 1);
                    ds128 = _mm_unpacklo_epi32(ds128, ds128);
                    ds128 = _mm_unpacklo_epi32(ds128, ds128);

                    dd128 = _mm_insert_epi16(dd128, -di[4 * d_stride], 6);
                    dd128 = _mm_insert_epi16(dd128, di[4 * d_stride + width], 7);
                    ds[1] = _mm_insert_epi16(ds[1], d_j[4 * d_stride], 0);
                    ds[1] = _mm_insert_epi16(ds[1], d_j[4 * d_stride + width], 1);

                    madd_sse4_1(dd[0], ds[0], &delta[0]);
                    madd_sse4_1(dd[1], ds[1], &delta[1]);
                    madd_sse4_1(dd128, ds128, &delta[2]);

                    // right shift 4 bytes
                    shift_right_4b_2x128(&ds[0]);

                    dd128 = _mm_srli_si128(dd128, 4);

                    di += d_stride;
                    d_j += d_stride;
                } while (++y < height);
            }

            update_4_stats_sse4_1(H + (i - 1) * wiener_win * wiener_win2 + (j - 1) * wiener_win,
                                  delta[0],
                                  H + i * wiener_win * wiener_win2 + j * wiener_win);
            H[i * wiener_win * wiener_win2 + j * wiener_win + 4] =
                H[(i - 1) * wiener_win * wiener_win2 + (j - 1) * wiener_win + 4] + _mm_cvtsi128_si32(delta[1]);

            H[(i * wiener_win + 1) * wiener_win2 + j * wiener_win] =
                H[((i - 1) * wiener_win + 1) * wiener_win2 + (j - 1) * wiener_win] + _mm_cvtsi128_si32(delta[2]);
            H[(i * wiener_win + 2) * wiener_win2 + j * wiener_win] =
                H[((i - 1) * wiener_win + 2) * wiener_win2 + (j - 1) * wiener_win] + _mm_extract_epi32(delta[2], 1);
            H[(i * wiener_win + 3) * wiener_win2 + j * wiener_win] =
                H[((i - 1) * wiener_win + 3) * wiener_win2 + (j - 1) * wiener_win] + _mm_extract_epi32(delta[2], 2);
            H[(i * wiener_win + 4) * wiener_win2 + j * wiener_win] =
                H[((i - 1) * wiener_win + 4) * wiener_win2 + (j - 1) * wiener_win] + _mm_extract_epi32(delta[2], 3);

        } while (++j < wiener_win);
    } while (++i < wiener_win - 1);

    // Step 5: Derive other points of each square. No square in bottom row.
    i = 0;
    do {
        const int16_t* const di = d + i;

        j = i + 1;
        do {
            const int16_t* const d_j                          = d + j;
            __m128i deltas[WIENER_WIN_CHROMA - 1][WIN_CHROMA] = {{_mm_setzero_si128()}, {_mm_setzero_si128()}};
            __m128i d_is[WIN_CHROMA], d_ie[WIN_CHROMA];
            __m128i d_js[WIN_CHROMA], d_je[WIN_CHROMA];

            x = 0;
            while (x < w16) {
                load_square_win5_sse4_1(di + x, d_j + x, d_stride, height, d_is, d_ie, d_js, d_je);
                derive_square_win5_sse4_1(d_is, d_ie, d_js, d_je, deltas);
                x += 16;
            };

            if (w16 != width) {
                load_square_win5_sse4_1(di + x, d_j + x, d_stride, height, d_is, d_ie, d_js, d_je);
                d_is[0] = _mm_and_si128(d_is[0], mask[0]);
                d_is[1] = _mm_and_si128(d_is[1], mask[1]);
                d_is[2] = _mm_and_si128(d_is[2], mask[0]);
                d_is[3] = _mm_and_si128(d_is[3], mask[1]);
                d_is[4] = _mm_and_si128(d_is[4], mask[0]);
                d_is[5] = _mm_and_si128(d_is[5], mask[1]);
                d_is[6] = _mm_and_si128(d_is[6], mask[0]);
                d_is[7] = _mm_and_si128(d_is[7], mask[1]);
                d_ie[0] = _mm_and_si128(d_ie[0], mask[0]);
                d_ie[1] = _mm_and_si128(d_ie[1], mask[1]);
                d_ie[2] = _mm_and_si128(d_ie[2], mask[0]);
                d_ie[3] = _mm_and_si128(d_ie[3], mask[1]);
                d_ie[4] = _mm_and_si128(d_ie[4], mask[0]);
                d_ie[5] = _mm_and_si128(d_ie[5], mask[1]);
                d_ie[6] = _mm_and_si128(d_ie[6], mask[0]);
                d_ie[7] = _mm_and_si128(d_ie[7], mask[1]);
                derive_square_win5_sse4_1(d_is, d_ie, d_js, d_je, deltas);
            }

            hadd_update_4_stats_sse4_1(H + (i * wiener_win + 0) * wiener_win2 + j * wiener_win,
                                       deltas[0],
                                       H + (i * wiener_win + 1) * wiener_win2 + j * wiener_win + 1);
            hadd_update_4_stats_sse4_1(H + (i * wiener_win + 1) * wiener_win2 + j * wiener_win,
                                       deltas[1],
                                       H + (i * wiener_win + 2) * wiener_win2 + j * wiener_win + 1);
            hadd_update_4_stats_sse4_1(H + (i * wiener_win + 2) * wiener_win2 + j * wiener_win,
                                       deltas[2],
                                       H + (i * wiener_win + 3) * wiener_win2 + j * wiener_win + 1);
            hadd_update_4_stats_sse4_1(H + (i * wiener_win + 3) * wiener_win2 + j * wiener_win,
                                       deltas[3],
                                       H + (i * wiener_win + 4) * wiener_win2 + j * wiener_win + 1);
        } while (++j < wiener_win);
    } while (++i < wiener_win - 1);

    // Step 6: Derive other points of each upper triangle along the diagonal.
    i = 0;
    do {
        const int16_t* const di                                                  = d + i;
        __m128i              deltas[WIENER_WIN_CHROMA * (WIENER_WIN_CHROMA - 1)] = {_mm_setzero_si128()};
        __m128i              d_is[WIN_CHROMA], d_ie[WIN_CHROMA];

        x = 0;
        while (x < w16) {
            load_triangle_win5_sse4_1(di + x, d_stride, height, d_is, d_ie);
            derive_triangle_win5_sse4_1(d_is, d_ie, deltas);
            x += 16;
        };

        if (w16 != width) {
            load_triangle_win5_sse4_1(di + x, d_stride, height, d_is, d_ie);
            d_is[0] = _mm_and_si128(d_is[0], mask[0]);
            d_is[1] = _mm_and_si128(d_is[1], mask[1]);
            d_is[2] = _mm_and_si128(d_is[2], mask[0]);
            d_is[3] = _mm_and_si128(d_is[3], mask[1]);
            d_is[4] = _mm_and_si128(d_is[4], mask[0]);
            d_is[5] = _mm_and_si128(d_is[5], mask[1]);
            d_is[6] = _mm_and_si128(d_is[6], mask[0]);
            d_is[7] = _mm_and_si128(d_is[7], mask[1]);
            d_ie[0] = _mm_and_si128(d_ie[0], mask[0]);
            d_ie[1] = _mm_and_si128(d_ie[1], mask[1]);
            d_ie[2] = _mm_and_si128(d_ie[2], mask[0]);
            d_ie[3] = _mm_and_si128(d_ie[3], mask[1]);
            d_ie[4] = _mm_and_si128(d_ie[4], mask[0]);
            d_ie[5] = _mm_and_si128(d_ie[5], mask[1]);
            d_ie[6] = _mm_and_si128(d_ie[6], mask[0]);
            d_ie[7] = _mm_and_si128(d_ie[7], mask[1]);
            derive_triangle_win5_sse4_1(d_is, d_ie, deltas);
        }

        hadd_update_4_stats_sse4_1(H + (i * wiener_win + 0) * wiener_win2 + i * wiener_win,
                                   deltas,
                                   H + (i * wiener_win + 1) * wiener_win2 + i * wiener_win + 1);

        __m128i delta32 = hadd_four_32_sse4_1(deltas[8], deltas[9], deltas[10], deltas[11]);
        __m128i delta64 = _mm_cvtepi32_epi64(delta32);

        update_2_stats_sse2(H + (i * wiener_win + 1) * wiener_win2 + i * wiener_win + 1,
                            delta64,
                            H + (i * wiener_win + 2) * wiener_win2 + i * wiener_win + 2);

        delta32 = hadd_four_32_sse4_1(deltas[12], deltas[13], deltas[18], deltas[19]);
        H[(i * wiener_win + 2) * wiener_win2 + i * wiener_win + 4] =
            H[(i * wiener_win + 1) * wiener_win2 + i * wiener_win + 3] + _mm_cvtsi128_si32(delta32);
        int64_t last_stat = _mm_extract_epi32(delta32, 1);

        delta32 = hadd_four_32_sse4_1(deltas[14], deltas[15], deltas[16], deltas[17]);
        delta64 = _mm_cvtepi32_epi64(delta32);

        update_2_stats_sse2(H + (i * wiener_win + 2) * wiener_win2 + i * wiener_win + 2,
                            delta64,
                            H + (i * wiener_win + 3) * wiener_win2 + i * wiener_win + 3);
        H[(i * wiener_win + 4) * wiener_win2 + i * wiener_win + 4] =
            H[(i * wiener_win + 3) * wiener_win2 + i * wiener_win + 3] + last_stat;
    } while (++i < wiener_win);
}

static void compute_stats_win7_sse4_1(const int16_t* const d, const int32_t d_stride, const int16_t* const s,
                                      const int32_t s_stride, const int32_t width, const int32_t height,
                                      int64_t* const M, int64_t* const H, EbBitDepth bit_depth) {
    const int32_t wiener_win  = WIENER_WIN;
    const int32_t wiener_win2 = wiener_win * wiener_win;
    const int32_t w16         = width & ~15;
    const int32_t h8          = height & ~7;
    const __m128i mask[2]     = {_mm_loadu_si128((__m128i*)(mask_16bit[width - w16])),
                                 _mm_loadu_si128((__m128i*)(mask_16bit[width - w16] + 8))};
    int32_t       i, j, x, y;

    if (bit_depth == EB_EIGHT_BIT) {
        // Step 1: Calculate the top edge of the whole matrix, i.e., the top
        // edge of each triangle and square on the top row.
        j = 0;
        do {
            const int16_t* s_t                   = s;
            const int16_t* d_t                   = d;
            __m128i        sum_m[WIENER_WIN * 2] = {_mm_setzero_si128()};
            __m128i        sum_h[WIENER_WIN * 2] = {_mm_setzero_si128()};
            __m128i        src[2], dgd[2];

            y = height;
            do {
                x = 0;
                while (x < w16) {
                    src[0] = _mm_loadu_si128((__m128i*)(s_t + x));
                    src[1] = _mm_loadu_si128((__m128i*)(s_t + x + 8));
                    dgd[0] = _mm_loadu_si128((__m128i*)(d_t + x));
                    dgd[1] = _mm_loadu_si128((__m128i*)(d_t + x + 8));
                    stats_top_win7_sse4_1(src, dgd, d_t + j + x, d_stride, sum_m, sum_h);
                    x += 16;
                };

                if (w16 != width) {
                    src[0] = _mm_loadu_si128((__m128i*)(s_t + w16));
                    src[1] = _mm_loadu_si128((__m128i*)(s_t + w16 + 8));
                    dgd[0] = _mm_loadu_si128((__m128i*)(d_t + w16));
                    dgd[1] = _mm_loadu_si128((__m128i*)(d_t + w16 + 8));
                    src[0] = _mm_and_si128(src[0], mask[0]);
                    src[1] = _mm_and_si128(src[1], mask[1]);
                    dgd[0] = _mm_and_si128(dgd[0], mask[0]);
                    dgd[1] = _mm_and_si128(dgd[1], mask[1]);
                    stats_top_win7_sse4_1(src, dgd, d_t + j + w16, d_stride, sum_m, sum_h);
                }

                s_t += s_stride;
                d_t += d_stride;
            } while (--y);
            _mm_storel_epi64((__m128i*)&M[wiener_win * j], hadd_two_32_to_64_sse4_1(sum_m[0], sum_m[1]));
            _mm_storel_epi64((__m128i*)&M[wiener_win * j + 1], hadd_two_32_to_64_sse4_1(sum_m[2], sum_m[3]));
            _mm_storel_epi64((__m128i*)&M[wiener_win * j + 2], hadd_two_32_to_64_sse4_1(sum_m[4], sum_m[5]));
            _mm_storel_epi64((__m128i*)&M[wiener_win * j + 3], hadd_two_32_to_64_sse4_1(sum_m[6], sum_m[7]));
            _mm_storel_epi64((__m128i*)&M[wiener_win * j + 4], hadd_two_32_to_64_sse4_1(sum_m[8], sum_m[9]));
            _mm_storel_epi64((__m128i*)&M[wiener_win * j + 5], hadd_two_32_to_64_sse4_1(sum_m[10], sum_m[11]));
            _mm_storel_epi64((__m128i*)&M[wiener_win * j + 6], hadd_two_32_to_64_sse4_1(sum_m[12], sum_m[13]));
            _mm_storel_epi64((__m128i*)(H + wiener_win * j), hadd_two_32_to_64_sse4_1(sum_h[0], sum_h[1]));
            _mm_storel_epi64((__m128i*)(H + wiener_win * j + 1), hadd_two_32_to_64_sse4_1(sum_h[2], sum_h[3]));
            _mm_storel_epi64((__m128i*)(H + wiener_win * j + 2), hadd_two_32_to_64_sse4_1(sum_h[4], sum_h[5]));
            _mm_storel_epi64((__m128i*)(H + wiener_win * j + 3), hadd_two_32_to_64_sse4_1(sum_h[6], sum_h[7]));
            _mm_storel_epi64((__m128i*)(H + wiener_win * j + 4), hadd_two_32_to_64_sse4_1(sum_h[8], sum_h[9]));
            _mm_storel_epi64((__m128i*)(H + wiener_win * j + 5), hadd_two_32_to_64_sse4_1(sum_h[10], sum_h[11]));
            _mm_storel_epi64((__m128i*)(H + wiener_win * j + 6), hadd_two_32_to_64_sse4_1(sum_h[12], sum_h[13]));
        } while (++j < wiener_win);

        // Step 2: Calculate the left edge of each square on the top row.
        j = 1;
        do {
            const int16_t* d_t          = d;
            __m128i        sum_h[WIN_7] = {_mm_setzero_si128()};
            __m128i        dgd[2];

            y = height;
            do {
                x = 0;
                while (x < w16) {
                    dgd[0] = _mm_loadu_si128((__m128i*)(d_t + j + x));
                    dgd[1] = _mm_loadu_si128((__m128i*)(d_t + j + x + 8));
                    stats_left_win7_sse4_1(dgd, d_t + x, d_stride, sum_h);
                    x += 16;
                };

                if (w16 != width) {
                    dgd[0] = _mm_loadu_si128((__m128i*)(d_t + j + x));
                    dgd[1] = _mm_loadu_si128((__m128i*)(d_t + j + x + 8));
                    dgd[0] = _mm_and_si128(dgd[0], mask[0]);
                    dgd[1] = _mm_and_si128(dgd[1], mask[1]);
                    stats_left_win7_sse4_1(dgd, d_t + x, d_stride, sum_h);
                }

                d_t += d_stride;
            } while (--y);
            _mm_storel_epi64((__m128i*)&H[1 * wiener_win2 + j * wiener_win],
                             hadd_two_32_to_64_sse4_1(sum_h[0], sum_h[1]));
            _mm_storel_epi64((__m128i*)&H[2 * wiener_win2 + j * wiener_win],
                             hadd_two_32_to_64_sse4_1(sum_h[2], sum_h[3]));
            _mm_storel_epi64((__m128i*)&H[3 * wiener_win2 + j * wiener_win],
                             hadd_two_32_to_64_sse4_1(sum_h[4], sum_h[5]));
            _mm_storel_epi64((__m128i*)&H[4 * wiener_win2 + j * wiener_win],
                             hadd_two_32_to_64_sse4_1(sum_h[6], sum_h[7]));
            _mm_storel_epi64((__m128i*)&H[5 * wiener_win2 + j * wiener_win],
                             hadd_two_32_to_64_sse4_1(sum_h[8], sum_h[9]));
            _mm_storel_epi64((__m128i*)&H[6 * wiener_win2 + j * wiener_win],
                             hadd_two_32_to_64_sse4_1(sum_h[10], sum_h[11]));
        } while (++j < wiener_win);
    } else {
        const int32_t num_bit_left = 32 - 1 /* sign */ - 2 * bit_depth /* energy */ + 3 /* SIMD */;
        const int32_t h_allowed    = (1 << num_bit_left) / (w16 + ((w16 != width) ? 16 : 0));

        // Step 1: Calculate the top edge of the whole matrix, i.e., the top
        // edge of each triangle and square on the top row.
        j = 0;
        do {
            const int16_t* s_t                   = s;
            const int16_t* d_t                   = d;
            int32_t        height_t              = 0;
            __m128i        sum_m[WIENER_WIN * 2] = {_mm_setzero_si128()};
            __m128i        sum_h[WIENER_WIN * 2] = {_mm_setzero_si128()};
            __m128i        src[2], dgd[2];

            do {
                const int32_t h_t = ((height - height_t) < h_allowed) ? (height - height_t) : h_allowed;
                __m128i       row_m[WIENER_WIN * 2] = {_mm_setzero_si128()};
                __m128i       row_h[WIENER_WIN * 2] = {_mm_setzero_si128()};

                y = h_t;
                do {
                    x = 0;
                    while (x < w16) {
                        src[0] = _mm_loadu_si128((__m128i*)(s_t + x));
                        src[1] = _mm_loadu_si128((__m128i*)(s_t + x + 8));
                        dgd[0] = _mm_loadu_si128((__m128i*)(d_t + x));
                        dgd[1] = _mm_loadu_si128((__m128i*)(d_t + x + 8));
                        stats_top_win7_sse4_1(src, dgd, d_t + j + x, d_stride, row_m, row_h);
                        x += 16;
                    };

                    if (w16 != width) {
                        src[0] = _mm_loadu_si128((__m128i*)(s_t + w16));
                        src[1] = _mm_loadu_si128((__m128i*)(s_t + w16 + 8));
                        dgd[0] = _mm_loadu_si128((__m128i*)(d_t + w16));
                        dgd[1] = _mm_loadu_si128((__m128i*)(d_t + w16 + 8));
                        src[0] = _mm_and_si128(src[0], mask[0]);
                        src[1] = _mm_and_si128(src[1], mask[1]);
                        dgd[0] = _mm_and_si128(dgd[0], mask[0]);
                        dgd[1] = _mm_and_si128(dgd[1], mask[1]);
                        stats_top_win7_sse4_1(src, dgd, d_t + j + w16, d_stride, row_m, row_h);
                    }

                    s_t += s_stride;
                    d_t += d_stride;
                } while (--y);

                add_32_to_64_sse4_1(row_m[0], &sum_m[0]);
                add_32_to_64_sse4_1(row_m[1], &sum_m[1]);
                add_32_to_64_sse4_1(row_m[2], &sum_m[2]);
                add_32_to_64_sse4_1(row_m[3], &sum_m[3]);
                add_32_to_64_sse4_1(row_m[4], &sum_m[4]);
                add_32_to_64_sse4_1(row_m[5], &sum_m[5]);
                add_32_to_64_sse4_1(row_m[6], &sum_m[6]);
                add_32_to_64_sse4_1(row_m[7], &sum_m[7]);
                add_32_to_64_sse4_1(row_m[8], &sum_m[8]);
                add_32_to_64_sse4_1(row_m[9], &sum_m[9]);
                add_32_to_64_sse4_1(row_m[10], &sum_m[10]);
                add_32_to_64_sse4_1(row_m[11], &sum_m[11]);
                add_32_to_64_sse4_1(row_m[12], &sum_m[12]);
                add_32_to_64_sse4_1(row_m[13], &sum_m[13]);

                add_32_to_64_sse4_1(row_h[0], &sum_h[0]);
                add_32_to_64_sse4_1(row_h[1], &sum_h[1]);
                add_32_to_64_sse4_1(row_h[2], &sum_h[2]);
                add_32_to_64_sse4_1(row_h[3], &sum_h[3]);
                add_32_to_64_sse4_1(row_h[4], &sum_h[4]);
                add_32_to_64_sse4_1(row_h[5], &sum_h[5]);
                add_32_to_64_sse4_1(row_h[6], &sum_h[6]);
                add_32_to_64_sse4_1(row_h[7], &sum_h[7]);
                add_32_to_64_sse4_1(row_h[8], &sum_h[8]);
                add_32_to_64_sse4_1(row_h[9], &sum_h[9]);
                add_32_to_64_sse4_1(row_h[10], &sum_h[10]);
                add_32_to_64_sse4_1(row_h[11], &sum_h[11]);
                add_32_to_64_sse4_1(row_h[12], &sum_h[12]);
                add_32_to_64_sse4_1(row_h[13], &sum_h[13]);

                height_t += h_t;
            } while (height_t < height);
            _mm_storel_epi64((__m128i*)&M[wiener_win * j], hadd_2_two_64_sse4_1(sum_m[0], sum_m[1]));
            _mm_storel_epi64((__m128i*)&M[wiener_win * j + 1], hadd_2_two_64_sse4_1(sum_m[2], sum_m[3]));
            _mm_storel_epi64((__m128i*)&M[wiener_win * j + 2], hadd_2_two_64_sse4_1(sum_m[4], sum_m[5]));
            _mm_storel_epi64((__m128i*)&M[wiener_win * j + 3], hadd_2_two_64_sse4_1(sum_m[6], sum_m[7]));
            _mm_storel_epi64((__m128i*)&M[wiener_win * j + 4], hadd_2_two_64_sse4_1(sum_m[8], sum_m[9]));
            _mm_storel_epi64((__m128i*)&M[wiener_win * j + 5], hadd_2_two_64_sse4_1(sum_m[10], sum_m[11]));
            _mm_storel_epi64((__m128i*)&M[wiener_win * j + 6], hadd_2_two_64_sse4_1(sum_m[12], sum_m[13]));

            _mm_storel_epi64((__m128i*)(H + wiener_win * j), hadd_2_two_64_sse4_1(sum_h[0], sum_h[1]));
            _mm_storel_epi64((__m128i*)(H + wiener_win * j + 1), hadd_2_two_64_sse4_1(sum_h[2], sum_h[3]));
            _mm_storel_epi64((__m128i*)(H + wiener_win * j + 2), hadd_2_two_64_sse4_1(sum_h[4], sum_h[5]));
            _mm_storel_epi64((__m128i*)(H + wiener_win * j + 3), hadd_2_two_64_sse4_1(sum_h[6], sum_h[7]));
            _mm_storel_epi64((__m128i*)(H + wiener_win * j + 4), hadd_2_two_64_sse4_1(sum_h[8], sum_h[9]));
            _mm_storel_epi64((__m128i*)(H + wiener_win * j + 5), hadd_2_two_64_sse4_1(sum_h[10], sum_h[11]));
            _mm_storel_epi64((__m128i*)(H + wiener_win * j + 6), hadd_2_two_64_sse4_1(sum_h[12], sum_h[13]));
        } while (++j < wiener_win);

        // Step 2: Calculate the left edge of each square on the top row.
        j = 1;
        do {
            const int16_t* d_t          = d;
            int32_t        height_t     = 0;
            __m128i        sum_h[WIN_7] = {_mm_setzero_si128()};
            __m128i        dgd[2];

            do {
                const int32_t h_t          = ((height - height_t) < h_allowed) ? (height - height_t) : h_allowed;
                __m128i       row_h[WIN_7] = {_mm_setzero_si128()};

                y = h_t;
                do {
                    x = 0;
                    while (x < w16) {
                        dgd[0] = _mm_loadu_si128((__m128i*)(d_t + j + x));
                        dgd[1] = _mm_loadu_si128((__m128i*)(d_t + j + x + 8));
                        stats_left_win7_sse4_1(dgd, d_t + x, d_stride, row_h);
                        x += 16;
                    };

                    if (w16 != width) {
                        dgd[0] = _mm_loadu_si128((__m128i*)(d_t + j + x));
                        dgd[1] = _mm_loadu_si128((__m128i*)(d_t + j + x + 8));
                        dgd[0] = _mm_and_si128(dgd[0], mask[0]);
                        dgd[1] = _mm_and_si128(dgd[1], mask[1]);
                        stats_left_win7_sse4_1(dgd, d_t + x, d_stride, row_h);
                    }

                    d_t += d_stride;
                } while (--y);

                add_32_to_64_sse4_1(row_h[0], &sum_h[0]);
                add_32_to_64_sse4_1(row_h[1], &sum_h[1]);
                add_32_to_64_sse4_1(row_h[2], &sum_h[2]);
                add_32_to_64_sse4_1(row_h[3], &sum_h[3]);
                add_32_to_64_sse4_1(row_h[4], &sum_h[4]);
                add_32_to_64_sse4_1(row_h[5], &sum_h[5]);
                add_32_to_64_sse4_1(row_h[6], &sum_h[6]);
                add_32_to_64_sse4_1(row_h[7], &sum_h[7]);
                add_32_to_64_sse4_1(row_h[8], &sum_h[8]);
                add_32_to_64_sse4_1(row_h[9], &sum_h[9]);
                add_32_to_64_sse4_1(row_h[10], &sum_h[10]);
                add_32_to_64_sse4_1(row_h[11], &sum_h[11]);

                height_t += h_t;
            } while (height_t < height);
            _mm_storel_epi64((__m128i*)&H[1 * wiener_win2 + j * wiener_win], hadd_2_two_64_sse4_1(sum_h[0], sum_h[1]));
            _mm_storel_epi64((__m128i*)&H[2 * wiener_win2 + j * wiener_win], hadd_2_two_64_sse4_1(sum_h[2], sum_h[3]));
            _mm_storel_epi64((__m128i*)&H[3 * wiener_win2 + j * wiener_win], hadd_2_two_64_sse4_1(sum_h[4], sum_h[5]));
            _mm_storel_epi64((__m128i*)&H[4 * wiener_win2 + j * wiener_win], hadd_2_two_64_sse4_1(sum_h[6], sum_h[7]));
            _mm_storel_epi64((__m128i*)&H[5 * wiener_win2 + j * wiener_win], hadd_2_two_64_sse4_1(sum_h[8], sum_h[9]));
            _mm_storel_epi64((__m128i*)&H[6 * wiener_win2 + j * wiener_win],
                             hadd_2_two_64_sse4_1(sum_h[10], sum_h[11]));
        } while (++j < wiener_win);
    }

    // Step 3: Derive the top edge of each triangle along the diagonal. No
    // triangle in top row.
    {
        const int16_t* d_t = d;
        // Pad to call transpose function.
        __m128i deltas[(WIENER_WIN + 1) * 2] = {_mm_setzero_si128()};
        __m128i ds[WIENER_WIN * 2];

        // 00s 00e 01s 01e 02s 02e 03s 03e  04s 04e 05s 05e 06s 06e 07s 07e
        // 10s 10e 11s 11e 12s 12e 13s 13e  14s 14e 15s 15e 16s 16e 17s 17e
        // 20s 20e 21s 21e 22s 22e 23s 23e  24s 24e 25s 25e 26s 26e 27s 27e
        // 30s 30e 31s 31e 32s 32e 33s 33e  34s 34e 35s 35e 36s 36e 37s 37e
        // 40s 40e 41s 41e 42s 42e 43s 43e  44s 44e 45s 45e 46s 46e 47s 47e
        // 50s 50e 51s 51e 52s 52e 53s 53e  54s 54e 55s 55e 56s 56e 57s 57e
        load_win7_sse4_1(d_t + 0 * d_stride, width, &ds[0]);
        load_win7_sse4_1(d_t + 1 * d_stride, width, &ds[2]);
        load_win7_sse4_1(d_t + 2 * d_stride, width, &ds[4]);
        load_win7_sse4_1(d_t + 3 * d_stride, width, &ds[6]);
        load_win7_sse4_1(d_t + 4 * d_stride, width, &ds[8]);
        load_win7_sse4_1(d_t + 5 * d_stride, width, &ds[10]);
        d_t += 6 * d_stride;

        step3_win7_sse4_1(&d_t, d_stride, width, height, ds, deltas);

        transpose_32bit_8x8_sse2(deltas, deltas);

        update_8_stats_sse4_1(H + 0 * wiener_win * wiener_win2 + 0 * wiener_win,
                              &deltas[0],
                              H + 1 * wiener_win * wiener_win2 + 1 * wiener_win);
        update_8_stats_sse4_1(H + 1 * wiener_win * wiener_win2 + 1 * wiener_win,
                              &deltas[2],
                              H + 2 * wiener_win * wiener_win2 + 2 * wiener_win);
        update_8_stats_sse4_1(H + 2 * wiener_win * wiener_win2 + 2 * wiener_win,
                              &deltas[4],
                              H + 3 * wiener_win * wiener_win2 + 3 * wiener_win);
        update_8_stats_sse4_1(H + 3 * wiener_win * wiener_win2 + 3 * wiener_win,
                              &deltas[6],
                              H + 4 * wiener_win * wiener_win2 + 4 * wiener_win);
        update_8_stats_sse4_1(H + 4 * wiener_win * wiener_win2 + 4 * wiener_win,
                              &deltas[8],
                              H + 5 * wiener_win * wiener_win2 + 5 * wiener_win);
        update_8_stats_sse4_1(H + 5 * wiener_win * wiener_win2 + 5 * wiener_win,
                              &deltas[10],
                              H + 6 * wiener_win * wiener_win2 + 6 * wiener_win);
    }

    // Step 4: Derive the top and left edge of each square. No square in top and
    // bottom row.
    i = 1;
    do {
        j = i + 1;
        do {
            const int16_t* di                               = d + i - 1;
            const int16_t* d_j                              = d + j - 1;
            __m128i        deltas[(2 * WIENER_WIN - 1) * 2] = {_mm_setzero_si128()};
            __m128i        dd[WIENER_WIN * 2], ds[WIENER_WIN * 2];

            dd[5] = _mm_setzero_si128(); // Initialize to avoid warning.

            dd[0] = _mm_setr_epi16(di[0 * d_stride],
                                   di[1 * d_stride],
                                   di[2 * d_stride],
                                   di[3 * d_stride],
                                   di[4 * d_stride],
                                   di[5 * d_stride],
                                   0,
                                   0);
            dd[1] = _mm_setr_epi16(di[0 * d_stride + width],
                                   di[1 * d_stride + width],
                                   di[2 * d_stride + width],
                                   di[3 * d_stride + width],
                                   di[4 * d_stride + width],
                                   di[5 * d_stride + width],
                                   0,
                                   0);

            ds[0] = _mm_setr_epi16(d_j[0 * d_stride],
                                   d_j[1 * d_stride],
                                   d_j[2 * d_stride],
                                   d_j[3 * d_stride],
                                   d_j[4 * d_stride],
                                   d_j[5 * d_stride],
                                   0,
                                   0);
            ds[1] = _mm_setr_epi16(d_j[0 * d_stride + width],
                                   d_j[1 * d_stride + width],
                                   d_j[2 * d_stride + width],
                                   d_j[3 * d_stride + width],
                                   d_j[4 * d_stride + width],
                                   d_j[5 * d_stride + width],
                                   0,
                                   0);

            y = 0;
            while (y < h8) {
                // 00s 10s 20s 30s 40s 50s 60s 70s  00e 10e 20e 30e 40e 50e 60e
                // 70e
                dd[0] = _mm_insert_epi16(dd[0], di[6 * d_stride], 6);
                dd[0] = _mm_insert_epi16(dd[0], di[7 * d_stride], 7);
                dd[1] = _mm_insert_epi16(dd[1], di[6 * d_stride + width], 6);
                dd[1] = _mm_insert_epi16(dd[1], di[7 * d_stride + width], 7);

                // 00s 10s 20s 30s 40s 50s 60s 70s  00e 10e 20e 30e 40e 50e 60e
                // 70e 01s 11s 21s 31s 41s 51s 61s 71s  01e 11e 21e 31e 41e 51e
                // 61e 71e
                ds[0] = _mm_insert_epi16(ds[0], d_j[6 * d_stride], 6);
                ds[0] = _mm_insert_epi16(ds[0], d_j[7 * d_stride], 7);
                ds[1] = _mm_insert_epi16(ds[1], d_j[6 * d_stride + width], 6);
                ds[1] = _mm_insert_epi16(ds[1], d_j[7 * d_stride + width], 7);

                load_more_16_sse4_1(di + 8 * d_stride, width, &dd[0], &dd[2]);
                load_more_16_sse4_1(d_j + 8 * d_stride, width, &ds[0], &ds[2]);
                load_more_16_sse4_1(di + 9 * d_stride, width, &dd[2], &dd[4]);
                load_more_16_sse4_1(d_j + 9 * d_stride, width, &ds[2], &ds[4]);
                load_more_16_sse4_1(di + 10 * d_stride, width, &dd[4], &dd[6]);
                load_more_16_sse4_1(d_j + 10 * d_stride, width, &ds[4], &ds[6]);
                load_more_16_sse4_1(di + 11 * d_stride, width, &dd[6], &dd[8]);
                load_more_16_sse4_1(d_j + 11 * d_stride, width, &ds[6], &ds[8]);
                load_more_16_sse4_1(di + 12 * d_stride, width, &dd[8], &dd[10]);
                load_more_16_sse4_1(d_j + 12 * d_stride, width, &ds[8], &ds[10]);
                load_more_16_sse4_1(di + 13 * d_stride, width, &dd[10], &dd[12]);
                load_more_16_sse4_1(d_j + 13 * d_stride, width, &ds[10], &ds[12]);

                madd_sse4_1(dd[0], ds[0], &deltas[0]);
                madd_sse4_1(dd[1], ds[1], &deltas[1]);
                madd_sse4_1(dd[0], ds[2], &deltas[2]);
                madd_sse4_1(dd[1], ds[3], &deltas[3]);
                madd_sse4_1(dd[0], ds[4], &deltas[4]);
                madd_sse4_1(dd[1], ds[5], &deltas[5]);
                madd_sse4_1(dd[0], ds[6], &deltas[6]);
                madd_sse4_1(dd[1], ds[7], &deltas[7]);
                madd_sse4_1(dd[0], ds[8], &deltas[8]);
                madd_sse4_1(dd[1], ds[9], &deltas[9]);
                madd_sse4_1(dd[0], ds[10], &deltas[10]);
                madd_sse4_1(dd[1], ds[11], &deltas[11]);
                madd_sse4_1(dd[0], ds[12], &deltas[12]);
                madd_sse4_1(dd[1], ds[13], &deltas[13]);
                madd_sse4_1(dd[2], ds[0], &deltas[14]);
                madd_sse4_1(dd[3], ds[1], &deltas[15]);
                madd_sse4_1(dd[4], ds[0], &deltas[16]);
                madd_sse4_1(dd[5], ds[1], &deltas[17]);
                madd_sse4_1(dd[6], ds[0], &deltas[18]);
                madd_sse4_1(dd[7], ds[1], &deltas[19]);
                madd_sse4_1(dd[8], ds[0], &deltas[20]);
                madd_sse4_1(dd[9], ds[1], &deltas[21]);
                madd_sse4_1(dd[10], ds[0], &deltas[22]);
                madd_sse4_1(dd[11], ds[1], &deltas[23]);
                madd_sse4_1(dd[12], ds[0], &deltas[24]);
                madd_sse4_1(dd[13], ds[1], &deltas[25]);

                dd[0] = _mm_srli_si128(dd[12], 4);
                dd[1] = _mm_srli_si128(dd[13], 4);
                ds[0] = _mm_srli_si128(ds[12], 4);
                ds[1] = _mm_srli_si128(ds[13], 4);
                di += 8 * d_stride;
                d_j += 8 * d_stride;
                y += 8;
            };

            deltas[0]  = _mm_hadd_epi32(deltas[0], deltas[2]);
            deltas[1]  = _mm_hadd_epi32(deltas[1], deltas[3]);
            deltas[2]  = _mm_hadd_epi32(deltas[4], deltas[6]);
            deltas[3]  = _mm_hadd_epi32(deltas[5], deltas[7]);
            deltas[4]  = _mm_hadd_epi32(deltas[8], deltas[10]);
            deltas[5]  = _mm_hadd_epi32(deltas[9], deltas[11]);
            deltas[6]  = _mm_hadd_epi32(deltas[12], deltas[12]);
            deltas[7]  = _mm_hadd_epi32(deltas[13], deltas[13]);
            deltas[8]  = _mm_hadd_epi32(deltas[14], deltas[16]);
            deltas[9]  = _mm_hadd_epi32(deltas[15], deltas[17]);
            deltas[10] = _mm_hadd_epi32(deltas[18], deltas[20]);
            deltas[11] = _mm_hadd_epi32(deltas[19], deltas[21]);
            deltas[12] = _mm_hadd_epi32(deltas[22], deltas[24]);
            deltas[13] = _mm_hadd_epi32(deltas[23], deltas[25]);
            deltas[0]  = _mm_hadd_epi32(deltas[0], deltas[2]);
            deltas[1]  = _mm_hadd_epi32(deltas[1], deltas[3]);
            deltas[2]  = _mm_hadd_epi32(deltas[4], deltas[6]);
            deltas[3]  = _mm_hadd_epi32(deltas[5], deltas[7]);
            deltas[4]  = _mm_hadd_epi32(deltas[8], deltas[10]);
            deltas[5]  = _mm_hadd_epi32(deltas[9], deltas[11]);
            deltas[6]  = _mm_hadd_epi32(deltas[12], deltas[12]);
            deltas[7]  = _mm_hadd_epi32(deltas[13], deltas[13]);
            deltas[0]  = _mm_sub_epi32(deltas[1], deltas[0]);
            deltas[1]  = _mm_sub_epi32(deltas[3], deltas[2]);
            deltas[2]  = _mm_sub_epi32(deltas[5], deltas[4]);
            deltas[3]  = _mm_sub_epi32(deltas[7], deltas[6]);

            if (h8 != height) {
                ds[0] = _mm_setr_epi16(d_j[0 * d_stride],
                                       d_j[0 * d_stride + width],
                                       d_j[1 * d_stride],
                                       d_j[1 * d_stride + width],
                                       d_j[2 * d_stride],
                                       d_j[2 * d_stride + width],
                                       d_j[3 * d_stride],
                                       d_j[3 * d_stride + width]);

                ds[1] = _mm_insert_epi16(ds[1], d_j[4 * d_stride], 0);
                ds[1] = _mm_insert_epi16(ds[1], d_j[4 * d_stride + width], 1);
                ds[1] = _mm_insert_epi16(ds[1], d_j[5 * d_stride], 2);
                ds[1] = _mm_insert_epi16(ds[1], d_j[5 * d_stride + width], 3);

                dd[4] = _mm_setr_epi16(-di[1 * d_stride],
                                       di[1 * d_stride + width],
                                       -di[2 * d_stride],
                                       di[2 * d_stride + width],
                                       -di[3 * d_stride],
                                       di[3 * d_stride + width],
                                       -di[4 * d_stride],
                                       di[4 * d_stride + width]);

                dd[5] = _mm_insert_epi16(dd[5], -di[5 * d_stride], 0);
                dd[5] = _mm_insert_epi16(dd[5], di[5 * d_stride + width], 1);
                do {
                    dd[0] = _mm_set1_epi16(-di[0 * d_stride]);
                    dd[2] = dd[3] = _mm_set1_epi16(di[0 * d_stride + width]);
                    dd[0] = dd[1] = _mm_unpacklo_epi16(dd[0], dd[2]);

                    ds[4] = _mm_set1_epi16(d_j[0 * d_stride]);
                    ds[6] = ds[7] = _mm_set1_epi16(d_j[0 * d_stride + width]);
                    ds[4] = ds[5] = _mm_unpacklo_epi16(ds[4], ds[6]);

                    dd[5] = _mm_insert_epi16(dd[5], -di[6 * d_stride], 2);
                    dd[5] = _mm_insert_epi16(dd[5], di[6 * d_stride + width], 3);
                    ds[1] = _mm_insert_epi16(ds[1], d_j[6 * d_stride], 4);
                    ds[1] = _mm_insert_epi16(ds[1], d_j[6 * d_stride + width], 5);

                    madd_sse4_1(dd[0], ds[0], &deltas[0]);
                    madd_sse4_1(dd[1], ds[1], &deltas[1]);
                    madd_sse4_1(dd[4], ds[4], &deltas[2]);
                    madd_sse4_1(dd[5], ds[5], &deltas[3]);

                    // right shift 4 bytes
                    shift_right_4b_2x128(&ds[0]);
                    shift_right_4b_2x128(&dd[4]);

                    di += d_stride;
                    d_j += d_stride;
                } while (++y < height);
            }

            // Writing one more H on the top edge of a square falls to the next
            // square in the same row or the first H in the next row, which
            // would be calculated later, so it won't overflow.
            update_8_stats_sse4_1(H + (i - 1) * wiener_win * wiener_win2 + (j - 1) * wiener_win,
                                  &deltas[0],
                                  H + i * wiener_win * wiener_win2 + j * wiener_win);

            H[(i * wiener_win + 1) * wiener_win2 + j * wiener_win] =
                H[((i - 1) * wiener_win + 1) * wiener_win2 + (j - 1) * wiener_win] + _mm_cvtsi128_si32(deltas[2]);
            H[(i * wiener_win + 2) * wiener_win2 + j * wiener_win] =
                H[((i - 1) * wiener_win + 2) * wiener_win2 + (j - 1) * wiener_win] + _mm_extract_epi32(deltas[2], 1);
            H[(i * wiener_win + 3) * wiener_win2 + j * wiener_win] =
                H[((i - 1) * wiener_win + 3) * wiener_win2 + (j - 1) * wiener_win] + _mm_extract_epi32(deltas[2], 2);
            H[(i * wiener_win + 4) * wiener_win2 + j * wiener_win] =
                H[((i - 1) * wiener_win + 4) * wiener_win2 + (j - 1) * wiener_win] + _mm_extract_epi32(deltas[2], 3);
            H[(i * wiener_win + 5) * wiener_win2 + j * wiener_win] =
                H[((i - 1) * wiener_win + 5) * wiener_win2 + (j - 1) * wiener_win] + _mm_extract_epi32(deltas[3], 0);
            H[(i * wiener_win + 6) * wiener_win2 + j * wiener_win] =
                H[((i - 1) * wiener_win + 6) * wiener_win2 + (j - 1) * wiener_win] + _mm_extract_epi32(deltas[3], 1);
        } while (++j < wiener_win);
    } while (++i < wiener_win - 1);

    // Step 5: Derive other points of each square. No square in bottom row.
    i = 0;
    do {
        const int16_t* const di = d + i;

        j = i + 1;
        do {
            const int16_t* const d_j                           = d + j;
            __m128i              deltas[WIENER_WIN - 1][WIN_7] = {{_mm_setzero_si128()}, {_mm_setzero_si128()}};
            __m128i              d_is[WIN_7], d_ie[WIN_7];
            __m128i              d_js[WIN_7], d_je[WIN_7];

            x = 0;
            while (x < w16) {
                load_square_win7_sse4_1(di + x, d_j + x, d_stride, height, d_is, d_ie, d_js, d_je);
                derive_square_win7_sse4_1(d_is, d_ie, d_js, d_je, deltas);
                x += 16;
            };

            if (w16 != width) {
                load_square_win7_sse4_1(di + x, d_j + x, d_stride, height, d_is, d_ie, d_js, d_je);
                d_is[0]  = _mm_and_si128(d_is[0], mask[0]);
                d_is[1]  = _mm_and_si128(d_is[1], mask[1]);
                d_is[2]  = _mm_and_si128(d_is[2], mask[0]);
                d_is[3]  = _mm_and_si128(d_is[3], mask[1]);
                d_is[4]  = _mm_and_si128(d_is[4], mask[0]);
                d_is[5]  = _mm_and_si128(d_is[5], mask[1]);
                d_is[6]  = _mm_and_si128(d_is[6], mask[0]);
                d_is[7]  = _mm_and_si128(d_is[7], mask[1]);
                d_is[8]  = _mm_and_si128(d_is[8], mask[0]);
                d_is[9]  = _mm_and_si128(d_is[9], mask[1]);
                d_is[10] = _mm_and_si128(d_is[10], mask[0]);
                d_is[11] = _mm_and_si128(d_is[11], mask[1]);
                d_ie[0]  = _mm_and_si128(d_ie[0], mask[0]);
                d_ie[1]  = _mm_and_si128(d_ie[1], mask[1]);
                d_ie[2]  = _mm_and_si128(d_ie[2], mask[0]);
                d_ie[3]  = _mm_and_si128(d_ie[3], mask[1]);
                d_ie[4]  = _mm_and_si128(d_ie[4], mask[0]);
                d_ie[5]  = _mm_and_si128(d_ie[5], mask[1]);
                d_ie[6]  = _mm_and_si128(d_ie[6], mask[0]);
                d_ie[7]  = _mm_and_si128(d_ie[7], mask[1]);
                d_ie[8]  = _mm_and_si128(d_ie[8], mask[0]);
                d_ie[9]  = _mm_and_si128(d_ie[9], mask[1]);
                d_ie[10] = _mm_and_si128(d_ie[10], mask[0]);
                d_ie[11] = _mm_and_si128(d_ie[11], mask[1]);
                derive_square_win7_sse4_1(d_is, d_ie, d_js, d_je, deltas);
            }

            hadd_update_6_stats_sse4_1(H + (i * wiener_win + 0) * wiener_win2 + j * wiener_win,
                                       deltas[0],
                                       H + (i * wiener_win + 1) * wiener_win2 + j * wiener_win + 1);
            hadd_update_6_stats_sse4_1(H + (i * wiener_win + 1) * wiener_win2 + j * wiener_win,
                                       deltas[1],
                                       H + (i * wiener_win + 2) * wiener_win2 + j * wiener_win + 1);
            hadd_update_6_stats_sse4_1(H + (i * wiener_win + 2) * wiener_win2 + j * wiener_win,
                                       deltas[2],
                                       H + (i * wiener_win + 3) * wiener_win2 + j * wiener_win + 1);
            hadd_update_6_stats_sse4_1(H + (i * wiener_win + 3) * wiener_win2 + j * wiener_win,
                                       deltas[3],
                                       H + (i * wiener_win + 4) * wiener_win2 + j * wiener_win + 1);
            hadd_update_6_stats_sse4_1(H + (i * wiener_win + 4) * wiener_win2 + j * wiener_win,
                                       deltas[4],
                                       H + (i * wiener_win + 5) * wiener_win2 + j * wiener_win + 1);
            hadd_update_6_stats_sse4_1(H + (i * wiener_win + 5) * wiener_win2 + j * wiener_win,
                                       deltas[5],
                                       H + (i * wiener_win + 6) * wiener_win2 + j * wiener_win + 1);
        } while (++j < wiener_win);
    } while (++i < wiener_win - 1);

    // Step 6: Derive other points of each upper triangle along the diagonal.
    i = 0;
    do {
        const int16_t* const di                                    = d + i;
        __m128i              deltas[WIENER_WIN * (WIENER_WIN - 1)] = {_mm_setzero_si128()};
        __m128i              d_is[WIN_7], d_ie[WIN_7];

        x = 0;
        while (x < w16) {
            load_triangle_win7_sse4_1(di + x, d_stride, height, d_is, d_ie);
            derive_triangle_win7_sse4_1(d_is, d_ie, deltas);
            x += 16;
        };

        if (w16 != width) {
            load_triangle_win7_sse4_1(di + x, d_stride, height, d_is, d_ie);
            d_is[0]  = _mm_and_si128(d_is[0], mask[0]);
            d_is[1]  = _mm_and_si128(d_is[1], mask[1]);
            d_is[2]  = _mm_and_si128(d_is[2], mask[0]);
            d_is[3]  = _mm_and_si128(d_is[3], mask[1]);
            d_is[4]  = _mm_and_si128(d_is[4], mask[0]);
            d_is[5]  = _mm_and_si128(d_is[5], mask[1]);
            d_is[6]  = _mm_and_si128(d_is[6], mask[0]);
            d_is[7]  = _mm_and_si128(d_is[7], mask[1]);
            d_is[8]  = _mm_and_si128(d_is[8], mask[0]);
            d_is[9]  = _mm_and_si128(d_is[9], mask[1]);
            d_is[10] = _mm_and_si128(d_is[10], mask[0]);
            d_is[11] = _mm_and_si128(d_is[11], mask[1]);
            d_ie[0]  = _mm_and_si128(d_ie[0], mask[0]);
            d_ie[1]  = _mm_and_si128(d_ie[1], mask[1]);
            d_ie[2]  = _mm_and_si128(d_ie[2], mask[0]);
            d_ie[3]  = _mm_and_si128(d_ie[3], mask[1]);
            d_ie[4]  = _mm_and_si128(d_ie[4], mask[0]);
            d_ie[5]  = _mm_and_si128(d_ie[5], mask[1]);
            d_ie[6]  = _mm_and_si128(d_ie[6], mask[0]);
            d_ie[7]  = _mm_and_si128(d_ie[7], mask[1]);
            d_ie[8]  = _mm_and_si128(d_ie[8], mask[0]);
            d_ie[9]  = _mm_and_si128(d_ie[9], mask[1]);
            d_ie[10] = _mm_and_si128(d_ie[10], mask[0]);
            d_ie[11] = _mm_and_si128(d_ie[11], mask[1]);
            derive_triangle_win7_sse4_1(d_is, d_ie, deltas);
        }

        // Row 1: 6 points
        hadd_update_6_stats_sse4_1(H + (i * wiener_win + 0) * wiener_win2 + i * wiener_win,
                                   deltas,
                                   H + (i * wiener_win + 1) * wiener_win2 + i * wiener_win + 1);

        __m128i delta64;
        __m128i delta32   = hadd_four_32_sse4_1(deltas[34], deltas[35], deltas[20], deltas[21]);
        __m128i delta_tmp = _mm_cvtepi32_epi64(delta32);

        // Row 2: 5 points
        hadd_update_4_stats_sse4_1(H + (i * wiener_win + 1) * wiener_win2 + i * wiener_win + 1,
                                   deltas + 12,
                                   H + (i * wiener_win + 2) * wiener_win2 + i * wiener_win + 2);
        H[(i * wiener_win + 2) * wiener_win2 + i * wiener_win + 6] =
            H[(i * wiener_win + 1) * wiener_win2 + i * wiener_win + 5] + _mm_extract_epi64(delta_tmp, 1);

        // Row 3: 4 points
        hadd_update_4_stats_sse4_1(H + (i * wiener_win + 2) * wiener_win2 + i * wiener_win + 2,
                                   deltas + 22,
                                   H + (i * wiener_win + 3) * wiener_win2 + i * wiener_win + 3);

        delta32 = hadd_four_32_sse4_1(deltas[30], deltas[31], deltas[32], deltas[33]);
        delta64 = _mm_cvtepi32_epi64(delta32);

        // Row 4: 3 points
        update_2_stats_sse2(H + (i * wiener_win + 3) * wiener_win2 + i * wiener_win + 3,
                            delta64,
                            H + (i * wiener_win + 4) * wiener_win2 + i * wiener_win + 4);
        H[(i * wiener_win + 4) * wiener_win2 + i * wiener_win + 6] =
            H[(i * wiener_win + 3) * wiener_win2 + i * wiener_win + 5] + _mm_extract_epi64(delta_tmp, 0);

        delta32 = hadd_four_32_sse4_1(deltas[36], deltas[37], deltas[38], deltas[39]);
        delta64 = _mm_cvtepi32_epi64(delta32);

        // Row 5: 2 points
        update_2_stats_sse2(H + (i * wiener_win + 4) * wiener_win2 + i * wiener_win + 4,
                            delta64,
                            H + (i * wiener_win + 5) * wiener_win2 + i * wiener_win + 5);

        delta32 = hadd_four_32_sse4_1(deltas[40], deltas[41], deltas[40], deltas[41]);
        delta64 = _mm_cvtepi32_epi64(delta32);

        // Row 6: 1 points
        H[(i * wiener_win + 6) * wiener_win2 + i * wiener_win + 6] =
            H[(i * wiener_win + 5) * wiener_win2 + i * wiener_win + 5] + _mm_extract_epi64(delta64, 0);
    } while (++i < wiener_win);
}

void svt_av1_compute_stats_sse4_1(int32_t wiener_win, const uint8_t* dgd, const uint8_t* src, int32_t h_start,
                                  int32_t h_end, int32_t v_start, int32_t v_end, int32_t dgd_stride, int32_t src_stride,
                                  int64_t* M, int64_t* H) {
    const int32_t wiener_win2    = wiener_win * wiener_win;
    const int32_t wiener_halfwin = wiener_win >> 1;
    const uint8_t avg            = find_average_sse4_1(dgd, h_start, h_end, v_start, v_end, dgd_stride);
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

    sub_avg_block_sse4_1(src + v_start * src_stride + h_start, src_stride, avg, width, height, s, s_stride);
    sub_avg_block_sse4_1(dgd + (v_start - wiener_halfwin) * dgd_stride + h_start - wiener_halfwin,
                         dgd_stride,
                         avg,
                         width + 2 * wiener_halfwin,
                         height + 2 * wiener_halfwin,
                         d,
                         d_stride);

    if (wiener_win == WIENER_WIN) {
        compute_stats_win7_sse4_1(d, d_stride, s, s_stride, width, height, M, H, 8);
    } else {
        assert(wiener_win == WIENER_WIN_CHROMA);
        compute_stats_win5_sse4_1(d, d_stride, s, s_stride, width, height, M, H, 8);
    }

    // H is a symmetric matrix, so we only need to fill out the upper triangle.
    // We can copy it down to the lower triangle outside the (i, j) loops.
    diagonal_copy_stats_sse4_1(wiener_win2, H);

    svt_aom_free(d);
}

#if CONFIG_ENABLE_HIGH_BIT_DEPTH
void svt_av1_compute_stats_highbd_sse4_1(int32_t wiener_win, const uint8_t* dgd8, const uint8_t* src8, int32_t h_start,
                                         int32_t h_end, int32_t v_start, int32_t v_end, int32_t dgd_stride,
                                         int32_t src_stride, int64_t* M, int64_t* H, EbBitDepth bit_depth) {
    if (bit_depth == EB_TWELVE_BIT) {
        svt_av1_compute_stats_highbd_c(
            wiener_win, dgd8, src8, h_start, h_end, v_start, v_end, dgd_stride, src_stride, M, H, bit_depth);
        return;
    }

    const int32_t   wiener_win2    = wiener_win * wiener_win;
    const int32_t   wiener_halfwin = (wiener_win >> 1);
    const uint16_t* src            = CONVERT_TO_SHORTPTR(src8);
    const uint16_t* dgd            = CONVERT_TO_SHORTPTR(dgd8);
    const uint16_t  avg      = find_average_highbd_sse4_1(dgd, h_start, h_end, v_start, v_end, dgd_stride, bit_depth);
    const int32_t   width    = h_end - h_start;
    const int32_t   height   = v_end - v_start;
    const int32_t   d_stride = (width + 2 * wiener_halfwin + 15) & ~15;
    const int32_t   s_stride = (width + 15) & ~15;
    int16_t *       d, *s;

    // The maximum input size is width * height, which is
    // (9 / 4) * RESTORATION_UNITSIZE_MAX * RESTORATION_UNITSIZE_MAX. Enlarge to
    // 3 * RESTORATION_UNITSIZE_MAX * RESTORATION_UNITSIZE_MAX considering
    // paddings.
    d = svt_aom_memalign(32, sizeof(*d) * 6 * RESTORATION_UNITSIZE_MAX * RESTORATION_UNITSIZE_MAX);
    s = d + 3 * RESTORATION_UNITSIZE_MAX * RESTORATION_UNITSIZE_MAX;

    sub_avg_block_highbd_sse4_1(src + v_start * src_stride + h_start, src_stride, avg, width, height, s, s_stride);
    sub_avg_block_highbd_sse4_1(dgd + (v_start - wiener_halfwin) * dgd_stride + h_start - wiener_halfwin,
                                dgd_stride,
                                avg,
                                width + 2 * wiener_halfwin,
                                height + 2 * wiener_halfwin,
                                d,
                                d_stride);

    if (wiener_win == WIENER_WIN) {
        compute_stats_win7_sse4_1(d, d_stride, s, s_stride, width, height, M, H, bit_depth);
    } else {
        assert(wiener_win == WIENER_WIN_CHROMA);
        compute_stats_win5_sse4_1(d, d_stride, s, s_stride, width, height, M, H, bit_depth);
    }

    // H is a symmetric matrix, so we only need to fill out the upper triangle.
    // We can copy it down to the lower triangle outside the (i, j) loops.
    if (bit_depth == EB_EIGHT_BIT) {
        diagonal_copy_stats_sse4_1(wiener_win2, H);
    } else { //bit_depth == EB_TEN_BIT
        const int32_t k4 = wiener_win2 & ~3;

        int32_t k = 0;
        do {
            __m128i dst = div4_sse4_1(_mm_loadu_si128((__m128i*)(M + k)));
            _mm_storeu_si128((__m128i*)(M + k), dst);
            dst = div4_sse4_1(_mm_loadu_si128((__m128i*)(M + k + 2)));
            _mm_storeu_si128((__m128i*)(M + k + 2), dst);
            H[k * wiener_win2 + k] /= 4;
            k += 4;
        } while (k < k4);

        H[k * wiener_win2 + k] /= 4;

        for (; k < wiener_win2; ++k) {
            M[k] /= 4;
        }

        div4_diagonal_copy_stats_sse4_1(wiener_win2, H);
    }

    svt_aom_free(d);
}
#endif

static INLINE __m128i pair_set_epi16(int a, int b) {
    return _mm_set1_epi32((int32_t)(((uint16_t)(a)) | (((uint32_t)(b)) << 16)));
}

int64_t svt_av1_lowbd_pixel_proj_error_sse4_1(const uint8_t* src8, int32_t width, int32_t height, int32_t src_stride,
                                              const uint8_t* dat8, int32_t dat_stride, int32_t* flt0,
                                              int32_t flt0_stride, int32_t* flt1, int32_t flt1_stride,
                                              const int32_t xq[2], const SgrParamsType* params) {
    int            i, j, k;
    const int32_t  shift    = SGRPROJ_RST_BITS + SGRPROJ_PRJ_BITS;
    const __m128i  rounding = _mm_set1_epi32(1 << (shift - 1));
    __m128i        sum64    = _mm_setzero_si128();
    const uint8_t* src      = src8;
    const uint8_t* dat      = dat8;
    int64_t        err      = 0;
    if (params->r[0] > 0 && params->r[1] > 0) {
        __m128i xq_coeff = pair_set_epi16(xq[0], xq[1]);
        for (i = 0; i < height; ++i) {
            __m128i sum32 = _mm_setzero_si128();
            for (j = 0; j <= width - 8; j += 8) {
                const __m128i d0           = _mm_cvtepu8_epi16(_mm_loadl_epi64((__m128i*)(dat + j)));
                const __m128i s0           = _mm_cvtepu8_epi16(_mm_loadl_epi64((__m128i*)(src + j)));
                const __m128i flt0_16b     = _mm_packs_epi32(_mm_loadu_si128((__m128i*)(flt0 + j)),
                                                         _mm_loadu_si128((__m128i*)(flt0 + j + 4)));
                const __m128i flt1_16b     = _mm_packs_epi32(_mm_loadu_si128((__m128i*)(flt1 + j)),
                                                         _mm_loadu_si128((__m128i*)(flt1 + j + 4)));
                const __m128i u0           = _mm_slli_epi16(d0, SGRPROJ_RST_BITS);
                const __m128i flt0_0_sub_u = _mm_sub_epi16(flt0_16b, u0);
                const __m128i flt1_0_sub_u = _mm_sub_epi16(flt1_16b, u0);
                const __m128i v0           = _mm_madd_epi16(xq_coeff, _mm_unpacklo_epi16(flt0_0_sub_u, flt1_0_sub_u));
                const __m128i v1           = _mm_madd_epi16(xq_coeff, _mm_unpackhi_epi16(flt0_0_sub_u, flt1_0_sub_u));
                const __m128i vr0          = _mm_srai_epi32(_mm_add_epi32(v0, rounding), shift);
                const __m128i vr1          = _mm_srai_epi32(_mm_add_epi32(v1, rounding), shift);
                const __m128i e0           = _mm_sub_epi16(_mm_add_epi16(_mm_packs_epi32(vr0, vr1), d0), s0);
                const __m128i err0         = _mm_madd_epi16(e0, e0);
                sum32                      = _mm_add_epi32(sum32, err0);
            }
            for (k = j; k < width; ++k) {
                const int32_t u = (int32_t)(dat[k] << SGRPROJ_RST_BITS);
                int32_t       v = xq[0] * (flt0[k] - u) + xq[1] * (flt1[k] - u);
                const int32_t e = ROUND_POWER_OF_TWO(v, shift) + dat[k] - src[k];
                err += ((int64_t)e * e);
            }
            dat += dat_stride;
            src += src_stride;
            flt0 += flt0_stride;
            flt1 += flt1_stride;
            const __m128i sum64_0 = _mm_cvtepi32_epi64(sum32);
            const __m128i sum64_1 = _mm_cvtepi32_epi64(_mm_srli_si128(sum32, 8));
            sum64                 = _mm_add_epi64(sum64, sum64_0);
            sum64                 = _mm_add_epi64(sum64, sum64_1);
        }
    } else if (params->r[0] > 0 || params->r[1] > 0) {
        const int      xq_active  = (params->r[0] > 0) ? xq[0] : xq[1];
        const __m128i  xq_coeff   = pair_set_epi16(xq_active, -(xq_active << SGRPROJ_RST_BITS));
        const int32_t* flt        = (params->r[0] > 0) ? flt0 : flt1;
        const int      flt_stride = (params->r[0] > 0) ? flt0_stride : flt1_stride;
        for (i = 0; i < height; ++i) {
            __m128i sum32 = _mm_setzero_si128();
            for (j = 0; j <= width - 8; j += 8) {
                const __m128i d0      = _mm_cvtepu8_epi16(_mm_loadl_epi64((__m128i*)(dat + j)));
                const __m128i s0      = _mm_cvtepu8_epi16(_mm_loadl_epi64((__m128i*)(src + j)));
                const __m128i flt_16b = _mm_packs_epi32(_mm_loadu_si128((__m128i*)(flt + j)),
                                                        _mm_loadu_si128((__m128i*)(flt + j + 4)));
                const __m128i v0      = _mm_madd_epi16(xq_coeff, _mm_unpacklo_epi16(flt_16b, d0));
                const __m128i v1      = _mm_madd_epi16(xq_coeff, _mm_unpackhi_epi16(flt_16b, d0));
                const __m128i vr0     = _mm_srai_epi32(_mm_add_epi32(v0, rounding), shift);
                const __m128i vr1     = _mm_srai_epi32(_mm_add_epi32(v1, rounding), shift);
                const __m128i e0      = _mm_sub_epi16(_mm_add_epi16(_mm_packs_epi32(vr0, vr1), d0), s0);
                const __m128i err0    = _mm_madd_epi16(e0, e0);
                sum32                 = _mm_add_epi32(sum32, err0);
            }
            for (k = j; k < width; ++k) {
                const int32_t u = (int32_t)(dat[k] << SGRPROJ_RST_BITS);
                int32_t       v = xq_active * (flt[k] - u);
                const int32_t e = ROUND_POWER_OF_TWO(v, shift) + dat[k] - src[k];
                err += ((int64_t)e * e);
            }
            dat += dat_stride;
            src += src_stride;
            flt += flt_stride;
            const __m128i sum64_0 = _mm_cvtepi32_epi64(sum32);
            const __m128i sum64_1 = _mm_cvtepi32_epi64(_mm_srli_si128(sum32, 8));
            sum64                 = _mm_add_epi64(sum64, sum64_0);
            sum64                 = _mm_add_epi64(sum64, sum64_1);
        }
    } else {
        __m128i sum32 = _mm_setzero_si128();
        for (i = 0; i < height; ++i) {
            for (j = 0; j <= width - 16; j += 16) {
                const __m128i d     = _mm_loadu_si128((__m128i*)(dat + j));
                const __m128i s     = _mm_loadu_si128((__m128i*)(src + j));
                const __m128i d0    = _mm_cvtepu8_epi16(d);
                const __m128i d1    = _mm_cvtepu8_epi16(_mm_srli_si128(d, 8));
                const __m128i s0    = _mm_cvtepu8_epi16(s);
                const __m128i s1    = _mm_cvtepu8_epi16(_mm_srli_si128(s, 8));
                const __m128i diff0 = _mm_sub_epi16(d0, s0);
                const __m128i diff1 = _mm_sub_epi16(d1, s1);
                const __m128i err0  = _mm_madd_epi16(diff0, diff0);
                const __m128i err1  = _mm_madd_epi16(diff1, diff1);
                sum32               = _mm_add_epi32(sum32, err0);
                sum32               = _mm_add_epi32(sum32, err1);
            }
            for (k = j; k < width; ++k) {
                const int32_t e = (int32_t)(dat[k]) - src[k];
                err += ((int64_t)e * e);
            }
            dat += dat_stride;
            src += src_stride;
        }
        const __m128i sum64_0 = _mm_cvtepi32_epi64(sum32);
        const __m128i sum64_1 = _mm_cvtepi32_epi64(_mm_srli_si128(sum32, 8));
        sum64                 = _mm_add_epi64(sum64_0, sum64_1);
    }
    int64_t sum[2];
    _mm_storeu_si128((__m128i*)sum, sum64);
    err += sum[0] + sum[1];
    return err;
}

int64_t svt_av1_highbd_pixel_proj_error_sse4_1(const uint8_t* src8, int32_t width, int32_t height, int32_t src_stride,
                                               const uint8_t* dat8, int32_t dat_stride, int32_t* flt0,
                                               int32_t flt0_stride, int32_t* flt1, int32_t flt1_stride,
                                               const int32_t xq[2], const SgrParamsType* params) {
    int             i, j, k;
    const int32_t   shift    = SGRPROJ_RST_BITS + SGRPROJ_PRJ_BITS;
    const __m128i   rounding = _mm_set1_epi32(1 << (shift - 1));
    __m128i         sum64    = _mm_setzero_si128();
    const uint16_t* src      = CONVERT_TO_SHORTPTR(src8);
    const uint16_t* dat      = CONVERT_TO_SHORTPTR(dat8);
    int64_t         err      = 0;
    if (params->r[0] > 0 && params->r[1] > 0) { // Both filters are enabled
        const __m128i xq0 = _mm_set1_epi32(xq[0]);
        const __m128i xq1 = _mm_set1_epi32(xq[1]);

        for (i = 0; i < height; ++i) {
            __m128i sum32 = _mm_setzero_si128();
            for (j = 0; j <= width - 8; j += 8) {
                // Load 8x pixels from source image
                const __m128i s0 = _mm_loadu_si128((__m128i*)(src + j));
                // s0 = [7 6 5 4 3 2 1 0] as i16 (indices of src[])

                // Load 8x pixels from corrupted image
                const __m128i d0 = _mm_loadu_si128((__m128i*)(dat + j));
                // d0 = [7 6 5 4 3 2 1 0] as i16 (indices of dat[])

                // Shift each pixel value up by SGRPROJ_RST_BITS
                const __m128i u0 = _mm_slli_epi16(d0, SGRPROJ_RST_BITS);

                // Split u0 into two halves and pad each from u16 to i32
                const __m128i u0l = _mm_cvtepu16_epi32(u0);
                const __m128i u0h = _mm_cvtepu16_epi32(_mm_srli_si128(u0, 8));
                // u0h = [7 6 5 4] as i32, u0l = [3 2 1 0] as i32, all dat[] indices

                // Load 8 pixels from first and second filtered images
                const __m128i flt0l = _mm_loadu_si128((__m128i*)(flt0 + j));
                const __m128i flt0h = _mm_loadu_si128((__m128i*)(flt0 + j + 4));
                const __m128i flt1l = _mm_loadu_si128((__m128i*)(flt1 + j));
                const __m128i flt1h = _mm_loadu_si128((__m128i*)(flt1 + j + 4));
                // flt0 = [7 6 5 4] [3 2 1 0] as i32 (indices of flt0+j)
                // flt1 = [7 6 5 4] [3 2 1 0] as i32 (indices of flt1+j)

                // Subtract shifted corrupt image from each filtered image
                // This gives our two basis vectors for the projection
                const __m128i flt0l_subu = _mm_sub_epi32(flt0l, u0l);
                const __m128i flt0h_subu = _mm_sub_epi32(flt0h, u0h);
                const __m128i flt1l_subu = _mm_sub_epi32(flt1l, u0l);
                const __m128i flt1h_subu = _mm_sub_epi32(flt1h, u0h);
                // flt?h_subu = [ f[7]-u[7] f[6]-u[6] f[5]-u[5] f[4]-u[4] ] as i32
                // flt?l_subu = [ f[3]-u[3] f[2]-u[2] f[1]-u[1] f[0]-u[0] ] as i32

                // Multiply each basis vector by the corresponding coefficient
                const __m128i v0l = _mm_mullo_epi32(flt0l_subu, xq0);
                const __m128i v0h = _mm_mullo_epi32(flt0h_subu, xq0);
                const __m128i v1l = _mm_mullo_epi32(flt1l_subu, xq1);
                const __m128i v1h = _mm_mullo_epi32(flt1h_subu, xq1);

                // Add together the contribution from each scaled basis vector
                const __m128i vl = _mm_add_epi32(v0l, v1l);
                const __m128i vh = _mm_add_epi32(v0h, v1h);

                // Right-shift v with appropriate rounding
                const __m128i vrl = _mm_srai_epi32(_mm_add_epi32(vl, rounding), shift);
                const __m128i vrh = _mm_srai_epi32(_mm_add_epi32(vh, rounding), shift);

                // Saturate each i32 value to i16 and combine lower and upper halves
                const __m128i vr = _mm_packs_epi32(vrl, vrh);

                // Add twin-subspace-sgr-filter to corrupt image then subtract source
                const __m128i e0 = _mm_sub_epi16(_mm_add_epi16(vr, d0), s0);

                // Calculate squared error and add adjacent values
                const __m128i err0 = _mm_madd_epi16(e0, e0);

                sum32 = _mm_add_epi32(sum32, err0);
            }

            const __m128i sum32l = _mm_cvtepu32_epi64(sum32);
            sum64                = _mm_add_epi64(sum64, sum32l);
            const __m128i sum32h = _mm_cvtepu32_epi64(_mm_srli_si128(sum32, 8));
            sum64                = _mm_add_epi64(sum64, sum32h);

            // Process remaining pixels in this row (modulo 8)
            for (k = j; k < width; ++k) {
                const int32_t u = (int32_t)(dat[k] << SGRPROJ_RST_BITS);
                int32_t       v = xq[0] * (flt0[k] - u) + xq[1] * (flt1[k] - u);
                const int32_t e = ROUND_POWER_OF_TWO(v, shift) + dat[k] - src[k];
                err += ((int64_t)e * e);
            }
            dat += dat_stride;
            src += src_stride;
            flt0 += flt0_stride;
            flt1 += flt1_stride;
        }
    } else if (params->r[0] > 0 || params->r[1] > 0) { // Only one filter enabled
        const int32_t  xq_on       = (params->r[0] > 0) ? xq[0] : xq[1];
        const __m128i  xq_active   = _mm_set1_epi32(xq_on);
        const __m128i  xq_inactive = _mm_set1_epi32(-xq_on * (1 << SGRPROJ_RST_BITS));
        const int32_t* flt         = (params->r[0] > 0) ? flt0 : flt1;
        const int      flt_stride  = (params->r[0] > 0) ? flt0_stride : flt1_stride;
        for (i = 0; i < height; ++i) {
            __m128i sum32 = _mm_setzero_si128();
            for (j = 0; j <= width - 8; j += 8) {
                // Load 8x pixels from source image
                const __m128i s0 = _mm_loadu_si128((__m128i*)(src + j));
                // s0 = [7 6 5 4 3 2 1 0] as u16 (indices of src[])

                // Load 8x pixels from corrupted image and pad each u16 to i32
                const __m128i d0  = _mm_loadu_si128((__m128i*)(dat + j));
                const __m128i d0h = _mm_cvtepu16_epi32(_mm_srli_si128(d0, 8));
                const __m128i d0l = _mm_cvtepu16_epi32(d0);
                // d0h, d0l = [7 6 5 4], [3 2 1 0] as u32 (indices of dat[])

                // Load 8 pixels from the filtered image
                const __m128i flth = _mm_loadu_si128((__m128i*)(flt + j + 4));
                const __m128i fltl = _mm_loadu_si128((__m128i*)(flt + j));
                // flth, fltl = [7 6 5 4], [3 2 1 0] as i32 (indices of flt+j)

                const __m128i flth_xq = _mm_mullo_epi32(flth, xq_active);
                const __m128i fltl_xq = _mm_mullo_epi32(fltl, xq_active);
                const __m128i d0h_xq  = _mm_mullo_epi32(d0h, xq_inactive);
                const __m128i d0l_xq  = _mm_mullo_epi32(d0l, xq_inactive);

                const __m128i vh = _mm_add_epi32(flth_xq, d0h_xq);
                const __m128i vl = _mm_add_epi32(fltl_xq, d0l_xq);
                // vh = [ xq0(f[7]-d[7]) xq0(f[6]-d[6]) xq0(f[5]-d[5]) xq0(f[4]-d[4]) ]
                // vl = [ xq0(f[3]-d[3]) xq0(f[2]-d[2]) xq0(f[1]-d[1]) xq0(f[0]-d[0]) ]

                // Shift this down with appropriate rounding
                const __m128i vrh = _mm_srai_epi32(_mm_add_epi32(vh, rounding), shift);
                const __m128i vrl = _mm_srai_epi32(_mm_add_epi32(vl, rounding), shift);

                // Saturate vr0 and vr1 from i32 to i16 then pack together
                const __m128i vr = _mm_packs_epi32(vrl, vrh);

                // Subtract twin-subspace-sgr filtered from source image to get error
                const __m128i e0 = _mm_sub_epi16(_mm_add_epi16(vr, d0), s0);

                // Calculate squared error and add adjacent values
                const __m128i err0 = _mm_madd_epi16(e0, e0);

                sum32 = _mm_add_epi32(sum32, err0);
            }

            const __m128i sum32l = _mm_cvtepu32_epi64(sum32);
            sum64                = _mm_add_epi64(sum64, sum32l);
            const __m128i sum32h = _mm_cvtepu32_epi64(_mm_srli_si128(sum32, 8));
            sum64                = _mm_add_epi64(sum64, sum32h);

            // Process remaining pixels in this row (modulo 8)
            for (k = j; k < width; ++k) {
                const int32_t u = (int32_t)(dat[k] << SGRPROJ_RST_BITS);
                int32_t       v = xq_on * (flt[k] - u);
                const int32_t e = ROUND_POWER_OF_TWO(v, shift) + dat[k] - src[k];
                err += ((int64_t)e * e);
            }
            dat += dat_stride;
            src += src_stride;
            flt += flt_stride;
        }
    } else { // Neither filter is enabled
        for (i = 0; i < height; ++i) {
            __m128i sum32 = _mm_setzero_si128();
            for (j = 0; j <= width - 16; j += 16) {
                // Load 2x8 u16 from source image
                const __m128i s0 = _mm_loadu_si128((__m128i*)(src + j));
                const __m128i s1 = _mm_loadu_si128((__m128i*)(src + j + 8));
                // Load 2x8 u16 from corrupted image
                const __m128i d0 = _mm_loadu_si128((__m128i*)(dat + j));
                const __m128i d1 = _mm_loadu_si128((__m128i*)(dat + j + 8));

                // Subtract corrupted image from source image
                const __m128i diff0 = _mm_sub_epi16(d0, s0);
                const __m128i diff1 = _mm_sub_epi16(d1, s1);

                // Square error and add adjacent values
                const __m128i err0 = _mm_madd_epi16(diff0, diff0);
                const __m128i err1 = _mm_madd_epi16(diff1, diff1);

                sum32 = _mm_add_epi32(sum32, err0);
                sum32 = _mm_add_epi32(sum32, err1);
            }

            const __m128i sum32l = _mm_cvtepu32_epi64(sum32);
            sum64                = _mm_add_epi64(sum64, sum32l);
            const __m128i sum32h = _mm_cvtepu32_epi64(_mm_srli_si128(sum32, 8));
            sum64                = _mm_add_epi64(sum64, sum32h);

            // Process remaining pixels (modulu 8)
            for (k = j; k < width; ++k) {
                const int32_t e = (int32_t)(dat[k]) - src[k];
                err += ((int64_t)e * e);
            }
            dat += dat_stride;
            src += src_stride;
        }
    }

    // Sum 4 values from sum64l and sum64h into err
    int64_t sum[2];
    _mm_storeu_si128((__m128i*)sum, sum64);
    err += sum[0] + sum[1];
    return err;
}
