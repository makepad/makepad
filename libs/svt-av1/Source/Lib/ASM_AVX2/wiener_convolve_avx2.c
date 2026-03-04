/*
 * Copyright (c) 2018, Alliance for Open Media. All rights reserved
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
 * Media Patent License 1.0 was not distributed with this source code in the
 * PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
 */

#include <assert.h>
#include <immintrin.h>

#include "common_dsp_rtcd.h"
#include "convolve.h"
#include "convolve_avx2.h"
#include "definitions.h"
#include "synonyms.h"
#include "synonyms_avx2.h"
#include "wiener_convolve_avx2.h"

static INLINE __m256i wiener_convolve_h16_tap3(const uint8_t* src, const __m256i coeffs[2], const __m256i filt[2],
                                               const __m256i filt_center, const __m256i round_h0,
                                               const __m256i round_h1, const __m256i clamp_high) {
    const __m256i s = yy_loadu_256(src);
    return wiener_convolve_tap3(s, coeffs, filt, filt_center, round_h0, round_h1, clamp_high);
}

static INLINE __m256i wiener_convolve_h16_tap5(const uint8_t* src, const __m256i coeffs[3], const __m256i filt[3],
                                               const __m256i filt_center, const __m256i round_h0,
                                               const __m256i round_h1, const __m256i clamp_high) {
    const __m256i s = yy_loadu_256(src);
    return wiener_convolve_tap5(s, coeffs, filt, filt_center, round_h0, round_h1, clamp_high);
}

static INLINE __m256i wiener_convolve_h16_tap7(const uint8_t* src, const __m256i coeffs[4], const __m256i filt[4],
                                               const __m256i filt_center, const __m256i round_h0,
                                               const __m256i round_h1, const __m256i clamp_high) {
    const __m256i s = yy_loadu_256(src);
    return wiener_convolve_tap7(s, coeffs, filt, filt_center, round_h0, round_h1, clamp_high);
}

static INLINE void wiener_convolve_h32_tap3(const uint8_t* src, const __m256i coeffs[2], const __m256i filt[2],
                                            const __m256i filt_center, const __m256i round_h0, const __m256i round_h1,
                                            const __m256i clamp_high, __m256i* const dst0, __m256i* const dst1) {
    *dst0 = wiener_convolve_h16_tap3(src + 0, coeffs, filt, filt_center, round_h0, round_h1, clamp_high);
    *dst1 = wiener_convolve_h16_tap3(src + 8, coeffs, filt, filt_center, round_h0, round_h1, clamp_high);
}

static INLINE void wiener_convolve_h32_tap5(const uint8_t* src, const __m256i coeffs[3], const __m256i filt[3],
                                            const __m256i filt_center, const __m256i round_h0, const __m256i round_h1,
                                            const __m256i clamp_high, __m256i* const dst0, __m256i* const dst1) {
    *dst0 = wiener_convolve_h16_tap5(src + 0, coeffs, filt, filt_center, round_h0, round_h1, clamp_high);
    *dst1 = wiener_convolve_h16_tap5(src + 8, coeffs, filt, filt_center, round_h0, round_h1, clamp_high);
}

static INLINE void wiener_convolve_h32_tap7(const uint8_t* src, const __m256i coeffs[4], const __m256i filt[4],
                                            const __m256i filt_center, const __m256i round_h0, const __m256i round_h1,
                                            const __m256i clamp_high, __m256i* const dst0, __m256i* const dst1) {
    *dst0 = wiener_convolve_h16_tap7(src + 0, coeffs, filt, filt_center, round_h0, round_h1, clamp_high);
    *dst1 = wiener_convolve_h16_tap7(src + 8, coeffs, filt, filt_center, round_h0, round_h1, clamp_high);
}

static INLINE __m256i wiener_convolve_v16_tap3(const __m256i coeffs[2], const __m256i round_v, __m256i s[3]) {
    const __m256i dst = wiener_convolve_v_tap3_kernel(coeffs, round_v, s);
    s[0]              = s[1];
    s[1]              = s[2];
    return dst;
}

static INLINE __m256i wiener_convolve_v16_tap5(const __m256i coeffs[2], const __m256i round_v, __m256i s[5]) {
    const __m256i dst = wiener_convolve_v_tap5_kernel(coeffs, round_v, s);
    s[0]              = s[1];
    s[1]              = s[2];
    s[2]              = s[3];
    s[3]              = s[4];
    return dst;
}

static INLINE __m256i wiener_convolve_v16_tap7(const __m256i coeffs[2], const __m256i round_v, __m256i s[7]) {
    const __m256i dst = wiener_convolve_v_tap7_kernel(coeffs, round_v, s);
    s[0]              = s[1];
    s[1]              = s[2];
    s[2]              = s[3];
    s[3]              = s[4];
    s[4]              = s[5];
    s[5]              = s[6];
    return dst;
}

// Note: If this function crash in Windows, please pay attention to the pointer
// filter_x, which could be overridden by other instructions. It's a bug from
// Visual Studio compiler. Please adjust the positions of the following 2
// instructions randomly to work around.
// 1. const __m128i coeffs_x = xx_loadu_128(filter_x);
// 2. const int cnt_zero_coef = calc_zero_coef(filter_x, filter_y);
void svt_av1_wiener_convolve_add_src_avx2(const uint8_t* const src, const ptrdiff_t src_stride, uint8_t* const dst,
                                          const ptrdiff_t dst_stride, const int16_t* const filter_x,
                                          const int16_t* const filter_y, const int32_t w, const int32_t h,
                                          const ConvolveParams* const conv_params) {
    const int32_t  bd         = 8;
    const int      center_tap = (SUBPEL_TAPS - 1) / 2;
    const int      round_0    = WIENER_ROUND0_BITS;
    const int      round_1    = 2 * FILTER_BITS - WIENER_ROUND0_BITS;
    const uint8_t* src_ptr    = src - center_tap * src_stride - center_tap;
    const __m256i  round_h0   = _mm256_set1_epi16((1 << (round_0 - 1)));
    const __m256i  round_h1   = _mm256_set1_epi16((1 << (bd + FILTER_BITS - round_0 - 1)));
    const __m256i  round_v    = _mm256_set1_epi32((1 << (round_1 - 1)) - (1 << (bd + round_1 - 1)));
    const __m256i  clamp_high = _mm256_set1_epi16(WIENER_CLAMP_LIMIT(round_0, bd) - 1);
    const __m128i  zero_128   = _mm_setzero_si128();
    const __m128i  offset_0   = _mm_insert_epi16(zero_128, 1 << FILTER_BITS, 3);
    // Note: Don't adjust the position of the following instruction.
    // Please see comments on top of this function.
    const __m128i coeffs_x        = xx_loadu_128(filter_x);
    const __m128i coeffs_y        = _mm_add_epi16(xx_loadu_128(filter_y), offset_0);
    const __m256i filter_coeffs_y = _mm256_broadcastsi128_si256(coeffs_y);
    int32_t       width           = w;
    uint8_t*      dst_ptr         = dst;
    __m256i       filt[4], coeffs_h[4], coeffs_v[2];

    (void)conv_params;
    assert(!(w % 8));
    assert(conv_params->round_0 == round_0);
    assert(conv_params->round_1 == round_1);

    filt[0] = yy_load_256(filt1_global_avx);
    filt[1] = yy_load_256(filt2_global_avx);

    // Note: Don't adjust the position of the following instruction.
    // Please see comments on top of this function.
    const int cnt_zero_coef = calc_zero_coef(filter_x, filter_y);

    if (!cnt_zero_coef) {
        const __m256i filt_center = yy_load_256(filt_center_tap7_global_avx);
        int32_t       x           = width & ~31;

        filt[2] = yy_load_256(filt3_global_avx);
        filt[3] = yy_load_256(filt4_global_avx);
        populate_coeffs_8tap_avx2(coeffs_x, coeffs_h);
        // coeffs 0 1 0 1 0 1 0 1
        coeffs_v[0] = _mm256_shuffle_epi32(filter_coeffs_y, 0x00);
        // coeffs 2 3 2 3 2 3 2 3
        coeffs_v[1] = _mm256_shuffle_epi32(filter_coeffs_y, 0x55);

        width -= x;
        while (x) {
            const uint8_t* src_p = src_ptr;
            __m256i        s[2][7];

            wiener_convolve_h32_tap7(
                src_p, coeffs_h, filt, filt_center, round_h0, round_h1, clamp_high, &s[0][0], &s[1][0]);
            src_p += src_stride;
            wiener_convolve_h32_tap7(
                src_p, coeffs_h, filt, filt_center, round_h0, round_h1, clamp_high, &s[0][1], &s[1][1]);
            src_p += src_stride;
            wiener_convolve_h32_tap7(
                src_p, coeffs_h, filt, filt_center, round_h0, round_h1, clamp_high, &s[0][2], &s[1][2]);
            src_p += src_stride;
            wiener_convolve_h32_tap7(
                src_p, coeffs_h, filt, filt_center, round_h0, round_h1, clamp_high, &s[0][3], &s[1][3]);
            src_p += src_stride;
            wiener_convolve_h32_tap7(
                src_p, coeffs_h, filt, filt_center, round_h0, round_h1, clamp_high, &s[0][4], &s[1][4]);
            src_p += src_stride;
            wiener_convolve_h32_tap7(
                src_p, coeffs_h, filt, filt_center, round_h0, round_h1, clamp_high, &s[0][5], &s[1][5]);
            src_p += src_stride;

            int y = 0;
            do {
                wiener_convolve_h32_tap7(
                    src_p, coeffs_h, filt, filt_center, round_h0, round_h1, clamp_high, &s[0][6], &s[1][6]);
                src_p += src_stride;
                const __m256i r0 = wiener_convolve_v16_tap7(coeffs_v, round_v, s[0]);
                const __m256i r1 = wiener_convolve_v16_tap7(coeffs_v, round_v, s[1]);
                convolve_store_32_avx2(r0, r1, dst_ptr + y * dst_stride);
            } while (++y < h);

            src_ptr += 32;
            dst_ptr += 32;
            x -= 32;
        }

        if (width >= 16) {
            const uint8_t* src_p = src_ptr;
            uint8_t*       dst_p = dst_ptr;
            __m256i        s[2][7];

            wiener_convolve_h16x2_tap7(
                src_p, src_stride, coeffs_h, filt, filt_center, round_h0, round_h1, clamp_high, &s[0][0], &s[1][0]);
            src_p += 2 * src_stride;
            wiener_convolve_h16x2_tap7(
                src_p, src_stride, coeffs_h, filt, filt_center, round_h0, round_h1, clamp_high, &s[0][2], &s[1][2]);
            src_p += 2 * src_stride;
            wiener_convolve_h16x2_tap7(
                src_p, src_stride, coeffs_h, filt, filt_center, round_h0, round_h1, clamp_high, &s[0][4], &s[1][4]);
            src_p += 2 * src_stride;

            s[0][1] = _mm256_setr_m128i(_mm256_extracti128_si256(s[0][0], 1), _mm256_castsi256_si128(s[0][2]));
            s[1][1] = _mm256_setr_m128i(_mm256_extracti128_si256(s[1][0], 1), _mm256_castsi256_si128(s[1][2]));
            s[0][3] = _mm256_setr_m128i(_mm256_extracti128_si256(s[0][2], 1), _mm256_castsi256_si128(s[0][4]));
            s[1][3] = _mm256_setr_m128i(_mm256_extracti128_si256(s[1][2], 1), _mm256_castsi256_si128(s[1][4]));

            int y = h;
            do {
                wiener_convolve_h16x2_tap7(
                    src_p, src_stride, coeffs_h, filt, filt_center, round_h0, round_h1, clamp_high, &s[0][6], &s[1][6]);
                s[0][5] = _mm256_setr_m128i(_mm256_extracti128_si256(s[0][4], 1), _mm256_castsi256_si128(s[0][6]));
                s[1][5] = _mm256_setr_m128i(_mm256_extracti128_si256(s[1][4], 1), _mm256_castsi256_si128(s[1][6]));
                src_p += 2 * src_stride;
                const __m256i r0 = wiener_convolve_v8x2_tap7(coeffs_v, round_v, s[0]);
                const __m256i r1 = wiener_convolve_v8x2_tap7(coeffs_v, round_v, s[1]);
                if (y == 1) {
                    const __m256i d  = _mm256_packus_epi16(r0, r1);
                    const __m128i d0 = _mm256_castsi256_si128(d);
                    _mm_storeu_si128((__m128i*)dst_p, d0);
                } else {
                    pack_store_16x2_avx2(r0, r1, dst_p, dst_stride);
                }
                dst_p += 2 * dst_stride;
                y -= 2;
            } while (y > 0);

            src_ptr += 16;
            dst_ptr += 16;
            width -= 16;
        }

        if (width) {
            const uint8_t* src_p = src_ptr;
            __m256i        s[7];

            assert(width == 8);

            s[0] = wiener_convolve_h8x2_tap7(
                src_p, src_stride, coeffs_h, filt, filt_center, round_h0, round_h1, clamp_high);
            src_p += 2 * src_stride;
            s[2] = wiener_convolve_h8x2_tap7(
                src_p, src_stride, coeffs_h, filt, filt_center, round_h0, round_h1, clamp_high);
            src_p += 2 * src_stride;
            s[4] = wiener_convolve_h8x2_tap7(
                src_p, src_stride, coeffs_h, filt, filt_center, round_h0, round_h1, clamp_high);
            src_p += 2 * src_stride;

            s[1] = _mm256_setr_m128i(_mm256_extracti128_si256(s[0], 1), _mm256_castsi256_si128(s[2]));
            s[3] = _mm256_setr_m128i(_mm256_extracti128_si256(s[2], 1), _mm256_castsi256_si128(s[4]));

            int y = h;
            do {
                s[6] = wiener_convolve_h8x2_tap7(
                    src_p, src_stride, coeffs_h, filt, filt_center, round_h0, round_h1, clamp_high);
                s[5] = _mm256_setr_m128i(_mm256_extracti128_si256(s[4], 1), _mm256_castsi256_si128(s[6]));
                src_p += 2 * src_stride;
                const __m256i r = wiener_convolve_v8x2_tap7(coeffs_v, round_v, s);
                if (y == 1) {
                    const __m256i d  = _mm256_packus_epi16(r, r);
                    const __m128i d0 = _mm256_castsi256_si128(d);
                    _mm_storel_epi64((__m128i*)dst_ptr, d0);
                } else {
                    pack_store_8x2_avx2(r, dst_ptr, dst_stride);
                }
                dst_ptr += 2 * dst_stride;
                y -= 2;
            } while (y > 0);
        }
    } else if (cnt_zero_coef == 1) {
        const __m256i filt_center = yy_load_256(filt_center_tap5_global_avx);
        int32_t       x           = width & ~31;

        src_ptr += src_stride + 1;
        filt[2] = yy_load_256(filt3_global_avx);
        populate_coeffs_6tap_avx2(coeffs_x, coeffs_h);
        // coeffs 1 2 1 2 1 2 1 2
        coeffs_v[0] = _mm256_shuffle_epi8(filter_coeffs_y, _mm256_set1_epi32(0x05040302u));
        // coeffs 3 4 3 4 3 4 3 4
        coeffs_v[1] = _mm256_shuffle_epi8(filter_coeffs_y, _mm256_set1_epi32(0x09080706u));

        width -= x;
        while (x) {
            const uint8_t* src_p = src_ptr;
            __m256i        s[2][5];

            wiener_convolve_h32_tap5(
                src_p, coeffs_h, filt, filt_center, round_h0, round_h1, clamp_high, &s[0][0], &s[1][0]);
            src_p += src_stride;
            wiener_convolve_h32_tap5(
                src_p, coeffs_h, filt, filt_center, round_h0, round_h1, clamp_high, &s[0][1], &s[1][1]);
            src_p += src_stride;
            wiener_convolve_h32_tap5(
                src_p, coeffs_h, filt, filt_center, round_h0, round_h1, clamp_high, &s[0][2], &s[1][2]);
            src_p += src_stride;
            wiener_convolve_h32_tap5(
                src_p, coeffs_h, filt, filt_center, round_h0, round_h1, clamp_high, &s[0][3], &s[1][3]);
            src_p += src_stride;

            int y = 0;
            do {
                wiener_convolve_h32_tap5(
                    src_p, coeffs_h, filt, filt_center, round_h0, round_h1, clamp_high, &s[0][4], &s[1][4]);
                src_p += src_stride;
                const __m256i r0 = wiener_convolve_v16_tap5(coeffs_v, round_v, s[0]);
                const __m256i r1 = wiener_convolve_v16_tap5(coeffs_v, round_v, s[1]);
                convolve_store_32_avx2(r0, r1, dst_ptr + y * dst_stride);
            } while (++y < h);

            src_ptr += 32;
            dst_ptr += 32;
            x -= 32;
        }

        if (width >= 16) {
            const uint8_t* src_p = src_ptr;
            uint8_t*       dst_p = dst_ptr;
            __m256i        s[2][5];

            wiener_convolve_h16x2_tap5(
                src_p, src_stride, coeffs_h, filt, filt_center, round_h0, round_h1, clamp_high, &s[0][0], &s[1][0]);
            src_p += 2 * src_stride;
            wiener_convolve_h16x2_tap5(
                src_p, src_stride, coeffs_h, filt, filt_center, round_h0, round_h1, clamp_high, &s[0][2], &s[1][2]);
            src_p += 2 * src_stride;

            s[0][1] = _mm256_setr_m128i(_mm256_extracti128_si256(s[0][0], 1), _mm256_castsi256_si128(s[0][2]));
            s[1][1] = _mm256_setr_m128i(_mm256_extracti128_si256(s[1][0], 1), _mm256_castsi256_si128(s[1][2]));

            int y = h;
            do {
                wiener_convolve_h16x2_tap5(
                    src_p, src_stride, coeffs_h, filt, filt_center, round_h0, round_h1, clamp_high, &s[0][4], &s[1][4]);
                s[0][3] = _mm256_setr_m128i(_mm256_extracti128_si256(s[0][2], 1), _mm256_castsi256_si128(s[0][4]));
                s[1][3] = _mm256_setr_m128i(_mm256_extracti128_si256(s[1][2], 1), _mm256_castsi256_si128(s[1][4]));
                src_p += 2 * src_stride;
                const __m256i r0 = wiener_convolve_v8x2_tap5(coeffs_v, round_v, s[0]);
                const __m256i r1 = wiener_convolve_v8x2_tap5(coeffs_v, round_v, s[1]);
                if (y == 1) {
                    const __m256i d  = _mm256_packus_epi16(r0, r1);
                    const __m128i d0 = _mm256_castsi256_si128(d);
                    _mm_storeu_si128((__m128i*)dst_p, d0);
                } else {
                    pack_store_16x2_avx2(r0, r1, dst_p, dst_stride);
                }
                dst_p += 2 * dst_stride;
                y -= 2;
            } while (y > 0);

            src_ptr += 16;
            dst_ptr += 16;
            width -= 16;
        }

        if (width) {
            const uint8_t* src_p = src_ptr;
            __m256i        s[5];

            assert(width == 8);

            s[0] = wiener_convolve_h8x2_tap5(
                src_p, src_stride, coeffs_h, filt, filt_center, round_h0, round_h1, clamp_high);
            src_p += 2 * src_stride;
            s[2] = wiener_convolve_h8x2_tap5(
                src_p, src_stride, coeffs_h, filt, filt_center, round_h0, round_h1, clamp_high);
            src_p += 2 * src_stride;

            s[1] = _mm256_setr_m128i(_mm256_extracti128_si256(s[0], 1), _mm256_castsi256_si128(s[2]));

            int y = h;
            do {
                s[4] = wiener_convolve_h8x2_tap5(
                    src_p, src_stride, coeffs_h, filt, filt_center, round_h0, round_h1, clamp_high);
                s[3] = _mm256_setr_m128i(_mm256_extracti128_si256(s[2], 1), _mm256_castsi256_si128(s[4]));
                src_p += 2 * src_stride;
                const __m256i r = wiener_convolve_v8x2_tap5(coeffs_v, round_v, s);
                if (y == 1) {
                    const __m256i d  = _mm256_packus_epi16(r, r);
                    const __m128i d0 = _mm256_castsi256_si128(d);
                    _mm_storel_epi64((__m128i*)dst_ptr, d0);
                } else {
                    pack_store_8x2_avx2(r, dst_ptr, dst_stride);
                }
                dst_ptr += 2 * dst_stride;
                y -= 2;
            } while (y > 0);
        }
    } else {
        const __m256i filt_center = yy_load_256(filt_center_tap3_global_avx);
        int32_t       x           = width & ~31;

        src_ptr += 2 * src_stride + 2;
        populate_coeffs_4tap_avx2(coeffs_x, coeffs_h);
        // coeffs 2 3 2 3 2 3 2 3
        coeffs_v[0] = _mm256_shuffle_epi32(filter_coeffs_y, 0x55);
        // coeffs 4 5 4 5 4 5 4 5
        coeffs_v[1] = _mm256_shuffle_epi32(filter_coeffs_y, 0xaa);

        width -= x;
        while (x) {
            const uint8_t* src_p = src_ptr;
            __m256i        s[2][3];

            wiener_convolve_h32_tap3(
                src_p, coeffs_h, filt, filt_center, round_h0, round_h1, clamp_high, &s[0][0], &s[1][0]);
            src_p += src_stride;
            wiener_convolve_h32_tap3(
                src_p, coeffs_h, filt, filt_center, round_h0, round_h1, clamp_high, &s[0][1], &s[1][1]);
            src_p += src_stride;

            int y = 0;
            do {
                wiener_convolve_h32_tap3(
                    src_p, coeffs_h, filt, filt_center, round_h0, round_h1, clamp_high, &s[0][2], &s[1][2]);
                src_p += src_stride;
                const __m256i r0 = wiener_convolve_v16_tap3(coeffs_v, round_v, s[0]);
                const __m256i r1 = wiener_convolve_v16_tap3(coeffs_v, round_v, s[1]);
                convolve_store_32_avx2(r0, r1, dst_ptr + y * dst_stride);
            } while (++y < h);

            src_ptr += 32;
            dst_ptr += 32;
            x -= 32;
        }

        if (width >= 16) {
            const uint8_t* src_p = src_ptr;
            uint8_t*       dst_p = dst_ptr;
            __m256i        s[2][3];

            wiener_convolve_h16x2_tap3(
                src_p, src_stride, coeffs_h, filt, filt_center, round_h0, round_h1, clamp_high, &s[0][0], &s[1][0]);
            src_p += 2 * src_stride;

            int y = h;
            do {
                wiener_convolve_h16x2_tap3(
                    src_p, src_stride, coeffs_h, filt, filt_center, round_h0, round_h1, clamp_high, &s[0][2], &s[1][2]);
                s[0][1] = _mm256_setr_m128i(_mm256_extracti128_si256(s[0][0], 1), _mm256_castsi256_si128(s[0][2]));
                s[1][1] = _mm256_setr_m128i(_mm256_extracti128_si256(s[1][0], 1), _mm256_castsi256_si128(s[1][2]));
                src_p += 2 * src_stride;
                const __m256i r0 = wiener_convolve_v8x2_tap3(coeffs_v, round_v, s[0]);
                const __m256i r1 = wiener_convolve_v8x2_tap3(coeffs_v, round_v, s[1]);
                if (y == 1) {
                    const __m256i d  = _mm256_packus_epi16(r0, r1);
                    const __m128i d0 = _mm256_castsi256_si128(d);
                    _mm_storeu_si128((__m128i*)dst_p, d0);
                } else {
                    pack_store_16x2_avx2(r0, r1, dst_p, dst_stride);
                }
                dst_p += 2 * dst_stride;
                y -= 2;
            } while (y > 0);

            src_ptr += 16;
            dst_ptr += 16;
            width -= 16;
        }

        if (width) {
            const uint8_t* src_p = src_ptr;
            __m256i        s[5];

            assert(width == 8);

            s[0] = wiener_convolve_h8x2_tap3(
                src_p, src_stride, coeffs_h, filt, filt_center, round_h0, round_h1, clamp_high);
            src_p += 2 * src_stride;

            int y = h;
            do {
                s[2] = wiener_convolve_h8x2_tap3(
                    src_p, src_stride, coeffs_h, filt, filt_center, round_h0, round_h1, clamp_high);
                s[1] = _mm256_setr_m128i(_mm256_extracti128_si256(s[0], 1), _mm256_castsi256_si128(s[2]));
                src_p += 2 * src_stride;
                const __m256i r = wiener_convolve_v8x2_tap3(coeffs_v, round_v, s);
                if (y == 1) {
                    const __m256i d  = _mm256_packus_epi16(r, r);
                    const __m128i d0 = _mm256_castsi256_si128(d);
                    _mm_storel_epi64((__m128i*)dst_ptr, d0);
                } else {
                    pack_store_8x2_avx2(r, dst_ptr, dst_stride);
                }
                dst_ptr += 2 * dst_stride;
                y -= 2;
            } while (y > 0);
        }
    }
}

// 128-bit xmmwords are written as [ ... ] with the MSB on the left.
// 256-bit ymmwords are written as two xmmwords, [ ... ][ ... ] with the MSB
// on the left.
// A row of, say, 8-bit pixels with values p0, p1, p2, ..., p30, p31 will be
// loaded and stored as [ p31 ... p17 p16 ][ p15 ... p1 p0 ].

// Exploiting the range of wiener filter coefficients,
// horizontal filtering can be done in 16 bit intermediate precision.
// The details are as follows :
// Consider the horizontal wiener filter coefficients of the following form :
//      [c0, C1, C2, 2^(FILTER_BITS) -2 * (c0 + C1 + C2), C2, C1, c0]
// Subtracting  2^(FILTER_BITS) from the centre tap we get the following  :
//      [c0, C1, C2,     -2 * (c0 + C1 + C2),             C2, C1, c0]
// The sum of the product "c0 * p0 + C1 * p1 + C2 * p2 -2 * (c0 + C1 + C2) * p3
// + C2 * p4 + C1 * p5 + c0 * p6" would be in the range of signed 16 bit
// precision. Finally, after rounding the above result by round_0, we multiply
// the centre pixel by 2^(FILTER_BITS - round_0) and add it to get the
// horizontal filter output.

// 128-bit xmmwords are written as [ ... ] with the MSB on the left.
// 256-bit ymmwords are written as two xmmwords, [ ... ][ ... ] with the MSB
// on the left.
// A row of, say, 16-bit pixels with values p0, p1, p2, ..., p14, p15 will be
// loaded and stored as [ p15 ... p9 p8 ][ p7 ... p1 p0 ].
void svt_av1_highbd_wiener_convolve_add_src_avx2(const uint8_t* const src, const ptrdiff_t src_stride,
                                                 uint8_t* const dst, const ptrdiff_t dst_stride,
                                                 const int16_t* const filter_x, const int16_t* const filter_y,
                                                 const int32_t w, const int32_t h,
                                                 const ConvolveParams* const conv_params, const int32_t bd) {
    assert(!(w & 7));
    assert(bd + FILTER_BITS - conv_params->round_0 + 2 <= 16);

    const uint16_t* const src16 = CONVERT_TO_SHORTPTR(src);
    uint16_t* const       dst16 = CONVERT_TO_SHORTPTR(dst);

    DECLARE_ALIGNED(32, uint16_t, temp[(MAX_SB_SIZE + SUBPEL_TAPS - 1) * MAX_SB_SIZE]);
    int32_t               intermediate_height = h + SUBPEL_TAPS - 1;
    const int32_t         center_tap          = ((SUBPEL_TAPS - 1) / 2);
    const uint16_t* const src_ptr             = src16 - center_tap * src_stride - center_tap;

    const __m128i zero_128 = _mm_setzero_si128();
    const __m256i zero_256 = _mm256_setzero_si256();

    // Add an offset to account for the "add_src" part of the convolve function.
    const __m128i offset = _mm_insert_epi16(zero_128, 1 << FILTER_BITS, 3);

    const __m256i clamp_low = zero_256;

    /* Horizontal filter */
    {
        const __m256i clamp_high_ep = _mm256_set1_epi16(WIENER_CLAMP_LIMIT(conv_params->round_0, bd) - 1);

        // coeffs [ f7 f6 f5 f4 f3 f2 f1 f0 ]
        const __m128i coeffs_x = _mm_add_epi16(xx_loadu_128(filter_x), offset);

        // coeffs [ f3 f2 f3 f2 f1 f0 f1 f0 ]
        const __m128i coeffs_0123 = _mm_unpacklo_epi32(coeffs_x, coeffs_x);
        // coeffs [ f7 f6 f7 f6 f5 f4 f5 f4 ]
        const __m128i coeffs_4567 = _mm_unpackhi_epi32(coeffs_x, coeffs_x);

        // coeffs [ f1 f0 f1 f0 f1 f0 f1 f0 ]
        const __m128i coeffs_01_128 = _mm_unpacklo_epi64(coeffs_0123, coeffs_0123);
        // coeffs [ f3 f2 f3 f2 f3 f2 f3 f2 ]
        const __m128i coeffs_23_128 = _mm_unpackhi_epi64(coeffs_0123, coeffs_0123);
        // coeffs [ f5 f4 f5 f4 f5 f4 f5 f4 ]
        const __m128i coeffs_45_128 = _mm_unpacklo_epi64(coeffs_4567, coeffs_4567);
        // coeffs [ f7 f6 f7 f6 f7 f6 f7 f6 ]
        const __m128i coeffs_67_128 = _mm_unpackhi_epi64(coeffs_4567, coeffs_4567);

        // coeffs [ f1 f0 f1 f0 f1 f0 f1 f0 ][ f1 f0 f1 f0 f1 f0 f1 f0 ]
        const __m256i coeffs_01 = yy_set_m128i(coeffs_01_128, coeffs_01_128);
        // coeffs [ f3 f2 f3 f2 f3 f2 f3 f2 ][ f3 f2 f3 f2 f3 f2 f3 f2 ]
        const __m256i coeffs_23 = yy_set_m128i(coeffs_23_128, coeffs_23_128);
        // coeffs [ f5 f4 f5 f4 f5 f4 f5 f4 ][ f5 f4 f5 f4 f5 f4 f5 f4 ]
        const __m256i coeffs_45 = yy_set_m128i(coeffs_45_128, coeffs_45_128);
        // coeffs [ f7 f6 f7 f6 f7 f6 f7 f6 ][ f7 f6 f7 f6 f7 f6 f7 f6 ]
        const __m256i coeffs_67 = yy_set_m128i(coeffs_67_128, coeffs_67_128);

        const __m256i round_const = _mm256_set1_epi32((1 << (conv_params->round_0 - 1)) +
                                                      (1 << (bd + FILTER_BITS - 1)));

        for (int32_t i = 0; i < intermediate_height; ++i) {
            for (int32_t j = 0; j < w; j += 16) {
                const uint16_t* src_ij = src_ptr + i * src_stride + j;

                // Load 16-bit src data
                const __m256i src_0 = yy_loadu_256(src_ij + 0);
                const __m256i src_1 = yy_loadu_256(src_ij + 1);
                const __m256i src_2 = yy_loadu_256(src_ij + 2);
                const __m256i src_3 = yy_loadu_256(src_ij + 3);
                const __m256i src_4 = yy_loadu_256(src_ij + 4);
                const __m256i src_5 = yy_loadu_256(src_ij + 5);
                const __m256i src_6 = yy_loadu_256(src_ij + 6);
                const __m256i src_7 = yy_loadu_256(src_ij + 7);

                // Multiply src data by filter coeffs and sum pairs
                const __m256i res_0 = _mm256_madd_epi16(src_0, coeffs_01);
                const __m256i res_1 = _mm256_madd_epi16(src_1, coeffs_01);
                const __m256i res_2 = _mm256_madd_epi16(src_2, coeffs_23);
                const __m256i res_3 = _mm256_madd_epi16(src_3, coeffs_23);
                const __m256i res_4 = _mm256_madd_epi16(src_4, coeffs_45);
                const __m256i res_5 = _mm256_madd_epi16(src_5, coeffs_45);
                const __m256i res_6 = _mm256_madd_epi16(src_6, coeffs_67);
                const __m256i res_7 = _mm256_madd_epi16(src_7, coeffs_67);

                // Calculate scalar product for even- and odd-indices separately,
                // increasing to 32-bit precision
                const __m256i res_even_sum = _mm256_add_epi32(_mm256_add_epi32(res_0, res_4),
                                                              _mm256_add_epi32(res_2, res_6));
                const __m256i res_even     = _mm256_srai_epi32(_mm256_add_epi32(res_even_sum, round_const),
                                                           conv_params->round_0);

                const __m256i res_odd_sum = _mm256_add_epi32(_mm256_add_epi32(res_1, res_5),
                                                             _mm256_add_epi32(res_3, res_7));
                const __m256i res_odd     = _mm256_srai_epi32(_mm256_add_epi32(res_odd_sum, round_const),
                                                          conv_params->round_0);

                // Reduce to 16-bit precision and pack even- and odd-index results
                // back into one register. The _mm256_packs_epi32 intrinsic returns
                // a register with the pixels ordered as follows:
                // [ 15 13 11 9 14 12 10 8 ] [ 7 5 3 1 6 4 2 0 ]
                const __m256i res         = _mm256_packs_epi32(res_even, res_odd);
                const __m256i res_clamped = _mm256_min_epi16(_mm256_max_epi16(res, clamp_low), clamp_high_ep);

                // Store in a temporary array
                yy_storeu_256(temp + i * MAX_SB_SIZE + j, res_clamped);
            }
        }
    }

    /* Vertical filter */
    {
        const __m256i clamp_high = _mm256_set1_epi16((1 << bd) - 1);

        // coeffs [ f7 f6 f5 f4 f3 f2 f1 f0 ]
        const __m128i coeffs_y = _mm_add_epi16(xx_loadu_128(filter_y), offset);

        // coeffs [ f3 f2 f3 f2 f1 f0 f1 f0 ]
        const __m128i coeffs_0123 = _mm_unpacklo_epi32(coeffs_y, coeffs_y);
        // coeffs [ f7 f6 f7 f6 f5 f4 f5 f4 ]
        const __m128i coeffs_4567 = _mm_unpackhi_epi32(coeffs_y, coeffs_y);

        // coeffs [ f1 f0 f1 f0 f1 f0 f1 f0 ]
        const __m128i coeffs_01_128 = _mm_unpacklo_epi64(coeffs_0123, coeffs_0123);
        // coeffs [ f3 f2 f3 f2 f3 f2 f3 f2 ]
        const __m128i coeffs_23_128 = _mm_unpackhi_epi64(coeffs_0123, coeffs_0123);
        // coeffs [ f5 f4 f5 f4 f5 f4 f5 f4 ]
        const __m128i coeffs_45_128 = _mm_unpacklo_epi64(coeffs_4567, coeffs_4567);
        // coeffs [ f7 f6 f7 f6 f7 f6 f7 f6 ]
        const __m128i coeffs_67_128 = _mm_unpackhi_epi64(coeffs_4567, coeffs_4567);

        // coeffs [ f1 f0 f1 f0 f1 f0 f1 f0 ][ f1 f0 f1 f0 f1 f0 f1 f0 ]
        const __m256i coeffs_01 = yy_set_m128i(coeffs_01_128, coeffs_01_128);
        // coeffs [ f3 f2 f3 f2 f3 f2 f3 f2 ][ f3 f2 f3 f2 f3 f2 f3 f2 ]
        const __m256i coeffs_23 = yy_set_m128i(coeffs_23_128, coeffs_23_128);
        // coeffs [ f5 f4 f5 f4 f5 f4 f5 f4 ][ f5 f4 f5 f4 f5 f4 f5 f4 ]
        const __m256i coeffs_45 = yy_set_m128i(coeffs_45_128, coeffs_45_128);
        // coeffs [ f7 f6 f7 f6 f7 f6 f7 f6 ][ f7 f6 f7 f6 f7 f6 f7 f6 ]
        const __m256i coeffs_67 = yy_set_m128i(coeffs_67_128, coeffs_67_128);

        const __m256i round_const = _mm256_set1_epi32((1 << (conv_params->round_1 - 1)) -
                                                      (1 << (bd + conv_params->round_1 - 1)));

        for (int32_t i = 0; i < h; ++i) {
            for (int32_t j = 0; j < w; j += 16) {
                const uint16_t* temp_ij = temp + i * MAX_SB_SIZE + j;

                // Load 16-bit data from the output of the horizontal filter in
                // which the pixels are ordered as follows:
                // [ 15 13 11 9 14 12 10 8 ] [ 7 5 3 1 6 4 2 0 ]
                const __m256i data_0 = yy_loadu_256(temp_ij + 0 * MAX_SB_SIZE);
                const __m256i data_1 = yy_loadu_256(temp_ij + 1 * MAX_SB_SIZE);
                const __m256i data_2 = yy_loadu_256(temp_ij + 2 * MAX_SB_SIZE);
                const __m256i data_3 = yy_loadu_256(temp_ij + 3 * MAX_SB_SIZE);
                const __m256i data_4 = yy_loadu_256(temp_ij + 4 * MAX_SB_SIZE);
                const __m256i data_5 = yy_loadu_256(temp_ij + 5 * MAX_SB_SIZE);
                const __m256i data_6 = yy_loadu_256(temp_ij + 6 * MAX_SB_SIZE);
                const __m256i data_7 = yy_loadu_256(temp_ij + 7 * MAX_SB_SIZE);

                // Filter the even-indices, increasing to 32-bit precision
                const __m256i src_0 = _mm256_unpacklo_epi16(data_0, data_1);
                const __m256i src_2 = _mm256_unpacklo_epi16(data_2, data_3);
                const __m256i src_4 = _mm256_unpacklo_epi16(data_4, data_5);
                const __m256i src_6 = _mm256_unpacklo_epi16(data_6, data_7);

                const __m256i res_0 = _mm256_madd_epi16(src_0, coeffs_01);
                const __m256i res_2 = _mm256_madd_epi16(src_2, coeffs_23);
                const __m256i res_4 = _mm256_madd_epi16(src_4, coeffs_45);
                const __m256i res_6 = _mm256_madd_epi16(src_6, coeffs_67);

                const __m256i res_even = _mm256_add_epi32(_mm256_add_epi32(res_0, res_2),
                                                          _mm256_add_epi32(res_4, res_6));

                // Filter the odd-indices, increasing to 32-bit precision
                const __m256i src_1 = _mm256_unpackhi_epi16(data_0, data_1);
                const __m256i src_3 = _mm256_unpackhi_epi16(data_2, data_3);
                const __m256i src_5 = _mm256_unpackhi_epi16(data_4, data_5);
                const __m256i src_7 = _mm256_unpackhi_epi16(data_6, data_7);

                const __m256i res_1 = _mm256_madd_epi16(src_1, coeffs_01);
                const __m256i res_3 = _mm256_madd_epi16(src_3, coeffs_23);
                const __m256i res_5 = _mm256_madd_epi16(src_5, coeffs_45);
                const __m256i res_7 = _mm256_madd_epi16(src_7, coeffs_67);

                const __m256i res_odd = _mm256_add_epi32(_mm256_add_epi32(res_1, res_3),
                                                         _mm256_add_epi32(res_5, res_7));

                // Pixels are currently in the following order:
                // res_even order: [ 14 12 10 8 ] [ 6 4 2 0 ]
                // res_odd order:  [ 15 13 11 9 ] [ 7 5 3 1 ]
                //
                // Rearrange the pixels into the following order:
                // res_lo order: [ 11 10  9  8 ] [ 3 2 1 0 ]
                // res_hi order: [ 15 14 13 12 ] [ 7 6 5 4 ]
                const __m256i res_lo = _mm256_unpacklo_epi32(res_even, res_odd);
                const __m256i res_hi = _mm256_unpackhi_epi32(res_even, res_odd);

                const __m256i res_lo_round = _mm256_srai_epi32(_mm256_add_epi32(res_lo, round_const),
                                                               conv_params->round_1);
                const __m256i res_hi_round = _mm256_srai_epi32(_mm256_add_epi32(res_hi, round_const),
                                                               conv_params->round_1);

                // Reduce to 16-bit precision and pack into the correct order:
                // [ 15 14 13 12 11 10 9 8 ][ 7 6 5 4 3 2 1 0 ]
                const __m256i res_16bit         = _mm256_packs_epi32(res_lo_round, res_hi_round);
                const __m256i res_16bit_clamped = _mm256_min_epi16(_mm256_max_epi16(res_16bit, clamp_low), clamp_high);

                // Store in the dst array
                if (j + 8 < w) {
                    yy_storeu_256(dst16 + i * dst_stride + j, res_16bit_clamped);
                } else {
                    _mm_storeu_si128((__m128i*)(dst16 + i * dst_stride + j), _mm256_castsi256_si128(res_16bit_clamped));
                }
            }
        }
    }
}
