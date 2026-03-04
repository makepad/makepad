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

#ifndef AOM_DSP_X86_WIENER_CONVOLVE_AVX2_H_
#define AOM_DSP_X86_WIENER_CONVOLVE_AVX2_H_

#include <immintrin.h>

#include "convolve.h"
#include "convolve_avx2.h"
#include "definitions.h"

DECLARE_ALIGNED(64, static const uint8_t, filt_center_tap7_global_avx[64]) = {
    3, 255, 4,  255, 5, 255, 6, 255, 7,  255, 8, 255, 9, 255, 10, 255, 3, 255, 4,  255, 5, 255,
    6, 255, 7,  255, 8, 255, 9, 255, 10, 255, 3, 255, 4, 255, 5,  255, 6, 255, 7,  255, 8, 255,
    9, 255, 10, 255, 3, 255, 4, 255, 5,  255, 6, 255, 7, 255, 8,  255, 9, 255, 10, 255};

DECLARE_ALIGNED(64, static const uint8_t, filt_center_tap5_global_avx[64]) = {
    2, 255, 3, 255, 4, 255, 5, 255, 6, 255, 7, 255, 8, 255, 9, 255, 2, 255, 3, 255, 4, 255,
    5, 255, 6, 255, 7, 255, 8, 255, 9, 255, 2, 255, 3, 255, 4, 255, 5, 255, 6, 255, 7, 255,
    8, 255, 9, 255, 2, 255, 3, 255, 4, 255, 5, 255, 6, 255, 7, 255, 8, 255, 9, 255};

DECLARE_ALIGNED(64, static const uint8_t, filt_center_tap3_global_avx[64]) = {
    1, 255, 2, 255, 3, 255, 4, 255, 5, 255, 6, 255, 7, 255, 8, 255, 1, 255, 2, 255, 3, 255,
    4, 255, 5, 255, 6, 255, 7, 255, 8, 255, 1, 255, 2, 255, 3, 255, 4, 255, 5, 255, 6, 255,
    7, 255, 8, 255, 1, 255, 2, 255, 3, 255, 4, 255, 5, 255, 6, 255, 7, 255, 8, 255};

static INLINE int calc_zero_coef(const int16_t* const filter_x, const int16_t* const filter_y) {
    int cnt = 0;
    if (!(filter_x[0] | filter_y[0])) {
        cnt++;
        if (!(filter_x[1] | filter_y[1])) {
            cnt++;
            if (!(filter_x[2] | filter_y[2])) {
                cnt++;
            }
        }
    }
    return cnt;
}

static INLINE __m256i wiener_clip(const __m256i s, const __m256i r, const __m256i filt_center, const __m256i round_h0,
                                  const __m256i round_h1, const __m256i clamp_high) {
    const int     round_0   = WIENER_ROUND0_BITS;
    const __m256i clamp_low = _mm256_setzero_si256();
    __m256i       res       = _mm256_srai_epi16(_mm256_add_epi16(r, round_h0), round_0);
    __m256i       data_0    = _mm256_shuffle_epi8(s, filt_center);
    data_0                  = _mm256_slli_epi16(data_0, FILTER_BITS - round_0);
    res                     = _mm256_add_epi16(res, data_0);
    res                     = _mm256_add_epi16(res, round_h1);
    res                     = _mm256_max_epi16(res, clamp_low);
    return _mm256_min_epi16(res, clamp_high);
}

SIMD_INLINE __m256i wiener_convolve_tap3(const __m256i s, const __m256i coeffs[2], const __m256i filt[2],
                                         const __m256i filt_center, const __m256i round_h0, const __m256i round_h1,
                                         const __m256i clamp_high) {
    const __m256i res = x_convolve_4tap_avx2(s, coeffs, filt);
    return wiener_clip(s, res, filt_center, round_h0, round_h1, clamp_high);
}

static INLINE __m256i wiener_convolve_tap5(const __m256i s, const __m256i coeffs[3], const __m256i filt[3],
                                           const __m256i filt_center, const __m256i round_h0, const __m256i round_h1,
                                           const __m256i clamp_high) {
    const __m256i res = x_convolve_6tap_avx2(s, coeffs, filt);
    return wiener_clip(s, res, filt_center, round_h0, round_h1, clamp_high);
}

static INLINE __m256i wiener_convolve_tap7(const __m256i s, const __m256i coeffs[4], const __m256i filt[4],
                                           const __m256i filt_center, const __m256i round_h0, const __m256i round_h1,
                                           const __m256i clamp_high) {
    const __m256i res = x_convolve_8tap_avx2(s, coeffs, filt);
    return wiener_clip(s, res, filt_center, round_h0, round_h1, clamp_high);
}

static INLINE __m256i wiener_convolve_h8x2_tap3(const uint8_t* src, const ptrdiff_t stride, const __m256i coeffs[2],
                                                const __m256i filt[2], const __m256i filt_center,
                                                const __m256i round_h0, const __m256i round_h1,
                                                const __m256i clamp_high) {
    const __m256i s = loadu_8bit_16x2_avx2(src, stride);
    return wiener_convolve_tap3(s, coeffs, filt, filt_center, round_h0, round_h1, clamp_high);
}

static INLINE __m256i wiener_convolve_h8x2_tap5(const uint8_t* src, const ptrdiff_t stride, const __m256i coeffs[3],
                                                const __m256i filt[3], const __m256i filt_center,
                                                const __m256i round_h0, const __m256i round_h1,
                                                const __m256i clamp_high) {
    const __m256i s = loadu_8bit_16x2_avx2(src, stride);
    return wiener_convolve_tap5(s, coeffs, filt, filt_center, round_h0, round_h1, clamp_high);
}

static INLINE __m256i wiener_convolve_h8x2_tap7(const uint8_t* src, const ptrdiff_t stride, const __m256i coeffs[4],
                                                const __m256i filt[4], const __m256i filt_center,
                                                const __m256i round_h0, const __m256i round_h1,
                                                const __m256i clamp_high) {
    const __m256i s = loadu_8bit_16x2_avx2(src, stride);
    return wiener_convolve_tap7(s, coeffs, filt, filt_center, round_h0, round_h1, clamp_high);
}

SIMD_INLINE void wiener_convolve_h16x2_tap3(const uint8_t* src, const ptrdiff_t stride, const __m256i coeffs[2],
                                            const __m256i filt[2], const __m256i filt_center, const __m256i round_h0,
                                            const __m256i round_h1, const __m256i clamp_high, __m256i* const dst0,
                                            __m256i* const dst1) {
    *dst0 = wiener_convolve_h8x2_tap3(src + 0, stride, coeffs, filt, filt_center, round_h0, round_h1, clamp_high);
    *dst1 = wiener_convolve_h8x2_tap3(src + 8, stride, coeffs, filt, filt_center, round_h0, round_h1, clamp_high);
}

SIMD_INLINE void wiener_convolve_h16x2_tap5(const uint8_t* src, const ptrdiff_t stride, const __m256i coeffs[3],
                                            const __m256i filt[3], const __m256i filt_center, const __m256i round_h0,
                                            const __m256i round_h1, const __m256i clamp_high, __m256i* const dst0,
                                            __m256i* const dst1) {
    *dst0 = wiener_convolve_h8x2_tap5(src + 0, stride, coeffs, filt, filt_center, round_h0, round_h1, clamp_high);
    *dst1 = wiener_convolve_h8x2_tap5(src + 8, stride, coeffs, filt, filt_center, round_h0, round_h1, clamp_high);
}

SIMD_INLINE void wiener_convolve_h16x2_tap7(const uint8_t* src, const ptrdiff_t stride, const __m256i coeffs[4],
                                            const __m256i filt[4], const __m256i filt_center, const __m256i round_h0,
                                            const __m256i round_h1, const __m256i clamp_high, __m256i* const dst0,
                                            __m256i* const dst1) {
    *dst0 = wiener_convolve_h8x2_tap7(src + 0, stride, coeffs, filt, filt_center, round_h0, round_h1, clamp_high);
    *dst1 = wiener_convolve_h8x2_tap7(src + 8, stride, coeffs, filt, filt_center, round_h0, round_h1, clamp_high);
}

static INLINE __m256i round_store(const __m256i res0, const __m256i res1, const __m256i round_v) {
    const int     round_1 = 2 * FILTER_BITS - WIENER_ROUND0_BITS;
    const __m256i r0      = _mm256_srai_epi32(_mm256_add_epi32(res0, round_v), round_1);
    const __m256i r1      = _mm256_srai_epi32(_mm256_add_epi32(res1, round_v), round_1);
    return _mm256_packs_epi32(r0, r1);
}

SIMD_INLINE __m256i wiener_convolve_v_tap3_kernel(const __m256i coeffs[2], const __m256i round_v, const __m256i s[3]) {
    const __m256i s0 = _mm256_add_epi16(s[0], s[2]);
    __m256i       ss[2];
    ss[0]              = _mm256_unpacklo_epi16(s0, s[1]);
    ss[1]              = _mm256_unpackhi_epi16(s0, s[1]);
    const __m256i res0 = convolve16_2tap_avx2(&ss[0], coeffs);
    const __m256i res1 = convolve16_2tap_avx2(&ss[1], coeffs);
    return round_store(res0, res1, round_v);
}

SIMD_INLINE __m256i wiener_convolve_v_tap5_kernel(const __m256i coeffs[2], const __m256i round_v, const __m256i s[5]) {
    const __m256i s0 = _mm256_add_epi16(s[0], s[4]);
    const __m256i s1 = _mm256_add_epi16(s[1], s[3]);
    __m256i       ss[4];
    ss[0]              = _mm256_unpacklo_epi16(s0, s1);
    ss[1]              = _mm256_unpacklo_epi16(s[2], _mm256_setzero_si256());
    ss[2]              = _mm256_unpackhi_epi16(s0, s1);
    ss[3]              = _mm256_unpackhi_epi16(s[2], _mm256_setzero_si256());
    const __m256i res0 = convolve16_4tap_avx2(ss + 0, coeffs);
    const __m256i res1 = convolve16_4tap_avx2(ss + 2, coeffs);
    return round_store(res0, res1, round_v);
}

SIMD_INLINE __m256i wiener_convolve_v_tap7_kernel(const __m256i coeffs[2], const __m256i round_v, const __m256i s[7]) {
    const __m256i s0 = _mm256_add_epi16(s[0], s[6]);
    const __m256i s1 = _mm256_add_epi16(s[1], s[5]);
    const __m256i s2 = _mm256_add_epi16(s[2], s[4]);
    __m256i       ss[4];
    ss[0]              = _mm256_unpacklo_epi16(s0, s1);
    ss[1]              = _mm256_unpacklo_epi16(s2, s[3]);
    ss[2]              = _mm256_unpackhi_epi16(s0, s1);
    ss[3]              = _mm256_unpackhi_epi16(s2, s[3]);
    const __m256i res0 = convolve16_4tap_avx2(ss + 0, coeffs);
    const __m256i res1 = convolve16_4tap_avx2(ss + 2, coeffs);
    return round_store(res0, res1, round_v);
}

SIMD_INLINE __m256i wiener_convolve_v8x2_tap3(const __m256i coeffs[2], const __m256i round_v, __m256i s[3]) {
    const __m256i dst = wiener_convolve_v_tap3_kernel(coeffs, round_v, s);
    s[0]              = s[2];
    return dst;
}

SIMD_INLINE __m256i wiener_convolve_v8x2_tap5(const __m256i coeffs[2], const __m256i round_v, __m256i s[5]) {
    const __m256i dst = wiener_convolve_v_tap5_kernel(coeffs, round_v, s);
    s[0]              = s[2];
    s[1]              = s[3];
    s[2]              = s[4];
    return dst;
}

static INLINE __m256i wiener_convolve_v8x2_tap7(const __m256i coeffs[2], const __m256i round_v, __m256i s[7]) {
    const __m256i dst = wiener_convolve_v_tap7_kernel(coeffs, round_v, s);
    s[0]              = s[2];
    s[1]              = s[3];
    s[2]              = s[4];
    s[3]              = s[5];
    s[4]              = s[6];
    return dst;
}

#endif // !AOM_DSP_X86_WIENER_CONVOLVE_AVX2_H_
