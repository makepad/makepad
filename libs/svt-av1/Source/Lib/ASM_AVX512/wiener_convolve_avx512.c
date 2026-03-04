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
#include "definitions.h"

#if EN_AVX512_SUPPORT
#include <assert.h>
#include <immintrin.h>

#include "common_dsp_rtcd.h"
#include "convolve.h"
#include "convolve_avx2.h"
#include "convolve_avx512.h"
#include "synonyms.h"
#include "synonyms_avx2.h"
#include "synonyms_avx512.h"
#include "wiener_convolve_avx2.h"

SIMD_INLINE __m512i wiener_clip_avx512(const __m512i s, const __m512i r, const __m512i filt_center,
                                       const __m512i round_h0, const __m512i round_h1, const __m512i clamp_high) {
    const int     round_0   = WIENER_ROUND0_BITS;
    const __m512i clamp_low = _mm512_setzero_si512();
    __m512i       res       = _mm512_srai_epi16(_mm512_add_epi16(r, round_h0), round_0);
    __m512i       data_0    = _mm512_shuffle_epi8(s, filt_center);
    data_0                  = _mm512_slli_epi16(data_0, FILTER_BITS - round_0);
    res                     = _mm512_add_epi16(res, data_0);
    res                     = _mm512_add_epi16(res, round_h1);
    res                     = _mm512_max_epi16(res, clamp_low);
    return _mm512_min_epi16(res, clamp_high);
}

static INLINE __m512i wiener_convolve_tap3_avx512(const __m512i s, const __m512i coeffs[2], const __m512i filt[2],
                                                  const __m512i filt_center, const __m512i round_h0,
                                                  const __m512i round_h1, const __m512i clamp_high) {
    const __m512i res = x_convolve_4tap_avx512(s, coeffs, filt);
    return wiener_clip_avx512(s, res, filt_center, round_h0, round_h1, clamp_high);
}

SIMD_INLINE __m512i wiener_convolve_tap5_avx512(const __m512i s, const __m512i coeffs[3], const __m512i filt[3],
                                                const __m512i filt_center, const __m512i round_h0,
                                                const __m512i round_h1, const __m512i clamp_high) {
    const __m512i res = x_convolve_6tap_avx512(s, coeffs, filt);
    return wiener_clip_avx512(s, res, filt_center, round_h0, round_h1, clamp_high);
}

static INLINE __m512i wiener_convolve_tap7_avx512(const __m512i s, const __m512i coeffs[4], const __m512i filt[4],
                                                  const __m512i filt_center, const __m512i round_h0,
                                                  const __m512i round_h1, const __m512i clamp_high) {
    const __m512i res = x_convolve_8tap_avx512(s, coeffs, filt);
    return wiener_clip_avx512(s, res, filt_center, round_h0, round_h1, clamp_high);
}

SIMD_INLINE __m512i wiener_convolve_h16x2_tap3_avx512(const uint8_t* src, const ptrdiff_t stride,
                                                      const __m512i coeffs[2], const __m512i filt[2],
                                                      const __m512i filt_center, const __m512i round_h0,
                                                      const __m512i round_h1, const __m512i clamp_high) {
    const __m512i s = loadu_8bit_32x2_avx512(src, stride);
    return wiener_convolve_tap3_avx512(s, coeffs, filt, filt_center, round_h0, round_h1, clamp_high);
}

SIMD_INLINE __m512i wiener_convolve_h16x2_tap5_avx512(const uint8_t* src, const ptrdiff_t stride,
                                                      const __m512i coeffs[3], const __m512i filt[3],
                                                      const __m512i filt_center, const __m512i round_h0,
                                                      const __m512i round_h1, const __m512i clamp_high) {
    const __m512i s = loadu_8bit_32x2_avx512(src, stride);
    return wiener_convolve_tap5_avx512(s, coeffs, filt, filt_center, round_h0, round_h1, clamp_high);
}

static INLINE __m512i wiener_convolve_h16x2_tap7_avx512(const uint8_t* src, const ptrdiff_t stride,
                                                        const __m512i coeffs[4], const __m512i filt[4],
                                                        const __m512i filt_center, const __m512i round_h0,
                                                        const __m512i round_h1, const __m512i clamp_high) {
    const __m512i s = loadu_8bit_32x2_avx512(src, stride);
    return wiener_convolve_tap7_avx512(s, coeffs, filt, filt_center, round_h0, round_h1, clamp_high);
}

SIMD_INLINE __m512i wiener_convolve_h32_tap3_avx512(const uint8_t* src, const __m512i coeffs[2], const __m512i filt[2],
                                                    const __m512i filt_center, const __m512i round_h0,
                                                    const __m512i round_h1, const __m512i clamp_high) {
    const __m512i s = zz_loadu_512(src);
    return wiener_convolve_tap3_avx512(s, coeffs, filt, filt_center, round_h0, round_h1, clamp_high);
}

static INLINE __m512i wiener_convolve_h32_tap5_avx512(const uint8_t* src, const __m512i coeffs[3],
                                                      const __m512i filt[3], const __m512i filt_center,
                                                      const __m512i round_h0, const __m512i round_h1,
                                                      const __m512i clamp_high) {
    const __m512i s = zz_loadu_512(src);
    return wiener_convolve_tap5_avx512(s, coeffs, filt, filt_center, round_h0, round_h1, clamp_high);
}

static INLINE __m512i wiener_convolve_h32_tap7_avx512(const uint8_t* src, const __m512i coeffs[4],
                                                      const __m512i filt[4], const __m512i filt_center,
                                                      const __m512i round_h0, const __m512i round_h1,
                                                      const __m512i clamp_high) {
    const __m512i s = zz_loadu_512(src);
    return wiener_convolve_tap7_avx512(s, coeffs, filt, filt_center, round_h0, round_h1, clamp_high);
}

SIMD_INLINE void wiener_convolve_h32x2_tap3(const uint8_t* src, const ptrdiff_t stride, const __m512i coeffs[2],
                                            const __m512i filt[2], const __m512i filt_center, const __m512i round_h0,
                                            const __m512i round_h1, const __m512i clamp_high, __m512i* const dst0,
                                            __m512i* const dst1) {
    *dst0 = wiener_convolve_h16x2_tap3_avx512(
        src + 0, stride, coeffs, filt, filt_center, round_h0, round_h1, clamp_high);
    *dst1 = wiener_convolve_h16x2_tap3_avx512(
        src + 8, stride, coeffs, filt, filt_center, round_h0, round_h1, clamp_high);
}

SIMD_INLINE void wiener_convolve_h32x2_tap5(const uint8_t* src, const ptrdiff_t stride, const __m512i coeffs[3],
                                            const __m512i filt[3], const __m512i filt_center, const __m512i round_h0,
                                            const __m512i round_h1, const __m512i clamp_high, __m512i* const dst0,
                                            __m512i* const dst1) {
    *dst0 = wiener_convolve_h16x2_tap5_avx512(
        src + 0, stride, coeffs, filt, filt_center, round_h0, round_h1, clamp_high);
    *dst1 = wiener_convolve_h16x2_tap5_avx512(
        src + 8, stride, coeffs, filt, filt_center, round_h0, round_h1, clamp_high);
}

SIMD_INLINE void wiener_convolve_h32x2_tap7(const uint8_t* src, const ptrdiff_t stride, const __m512i coeffs[4],
                                            const __m512i filt[4], const __m512i filt_center, const __m512i round_h0,
                                            const __m512i round_h1, const __m512i clamp_high, __m512i* const dst0,
                                            __m512i* const dst1) {
    *dst0 = wiener_convolve_h16x2_tap7_avx512(
        src + 0, stride, coeffs, filt, filt_center, round_h0, round_h1, clamp_high);
    *dst1 = wiener_convolve_h16x2_tap7_avx512(
        src + 8, stride, coeffs, filt, filt_center, round_h0, round_h1, clamp_high);
}

static INLINE void wiener_convolve_h64_tap3(const uint8_t* src, const __m512i coeffs[2], const __m512i filt[2],
                                            const __m512i filt_center, const __m512i round_h0, const __m512i round_h1,
                                            const __m512i clamp_high, __m512i* const dst0, __m512i* const dst1) {
    *dst0 = wiener_convolve_h32_tap3_avx512(src + 0, coeffs, filt, filt_center, round_h0, round_h1, clamp_high);
    *dst1 = wiener_convolve_h32_tap3_avx512(src + 8, coeffs, filt, filt_center, round_h0, round_h1, clamp_high);
}

static INLINE void wiener_convolve_h64_tap5(const uint8_t* src, const __m512i coeffs[3], const __m512i filt[3],
                                            const __m512i filt_center, const __m512i round_h0, const __m512i round_h1,
                                            const __m512i clamp_high, __m512i* const dst0, __m512i* const dst1) {
    *dst0 = wiener_convolve_h32_tap5_avx512(src + 0, coeffs, filt, filt_center, round_h0, round_h1, clamp_high);
    *dst1 = wiener_convolve_h32_tap5_avx512(src + 8, coeffs, filt, filt_center, round_h0, round_h1, clamp_high);
}

static INLINE void wiener_convolve_h64_tap7_avx512(const uint8_t* src, const __m512i coeffs[4], const __m512i filt[4],
                                                   const __m512i filt_center, const __m512i round_h0,
                                                   const __m512i round_h1, const __m512i clamp_high,
                                                   __m512i* const dst0, __m512i* const dst1) {
    *dst0 = wiener_convolve_h32_tap7_avx512(src + 0, coeffs, filt, filt_center, round_h0, round_h1, clamp_high);
    *dst1 = wiener_convolve_h32_tap7_avx512(src + 8, coeffs, filt, filt_center, round_h0, round_h1, clamp_high);
}

static INLINE __m512i round_store_avx512(const __m512i res0, const __m512i res1, const __m512i round_v) {
    const int     round_1 = 2 * FILTER_BITS - WIENER_ROUND0_BITS;
    const __m512i r0      = _mm512_srai_epi32(_mm512_add_epi32(res0, round_v), round_1);
    const __m512i r1      = _mm512_srai_epi32(_mm512_add_epi32(res1, round_v), round_1);
    return _mm512_packs_epi32(r0, r1);
}

SIMD_INLINE __m512i wiener_convolve_v_tap3_kernel_avx512(const __m512i coeffs[2], const __m512i round_v,
                                                         const __m512i s[3]) {
    const __m512i s0 = _mm512_add_epi16(s[0], s[2]);
    __m512i       ss[2];
    ss[0]              = _mm512_unpacklo_epi16(s0, s[1]);
    ss[1]              = _mm512_unpackhi_epi16(s0, s[1]);
    const __m512i res0 = convolve16_2tap_avx512(&ss[0], coeffs);
    const __m512i res1 = convolve16_2tap_avx512(&ss[1], coeffs);
    return round_store_avx512(res0, res1, round_v);
}

SIMD_INLINE __m512i wiener_convolve_v_tap5_kernel_avx512(const __m512i coeffs[2], const __m512i round_v,
                                                         const __m512i s[5]) {
    const __m512i s0 = _mm512_add_epi16(s[0], s[4]);
    const __m512i s1 = _mm512_add_epi16(s[1], s[3]);
    __m512i       ss[4];
    ss[0]              = _mm512_unpacklo_epi16(s0, s1);
    ss[1]              = _mm512_unpacklo_epi16(s[2], _mm512_setzero_si512());
    ss[2]              = _mm512_unpackhi_epi16(s0, s1);
    ss[3]              = _mm512_unpackhi_epi16(s[2], _mm512_setzero_si512());
    const __m512i res0 = convolve16_4tap_avx512(ss + 0, coeffs);
    const __m512i res1 = convolve16_4tap_avx512(ss + 2, coeffs);
    return round_store_avx512(res0, res1, round_v);
}

SIMD_INLINE __m512i wiener_convolve_v_tap7_kernel_avx512(const __m512i coeffs[2], const __m512i round_v,
                                                         const __m512i s[7]) {
    const __m512i s0 = _mm512_add_epi16(s[0], s[6]);
    const __m512i s1 = _mm512_add_epi16(s[1], s[5]);
    const __m512i s2 = _mm512_add_epi16(s[2], s[4]);
    __m512i       ss[4];
    ss[0]              = _mm512_unpacklo_epi16(s0, s1);
    ss[1]              = _mm512_unpacklo_epi16(s2, s[3]);
    ss[2]              = _mm512_unpackhi_epi16(s0, s1);
    ss[3]              = _mm512_unpackhi_epi16(s2, s[3]);
    const __m512i res0 = convolve16_4tap_avx512(ss + 0, coeffs);
    const __m512i res1 = convolve16_4tap_avx512(ss + 2, coeffs);
    return round_store_avx512(res0, res1, round_v);
}

SIMD_INLINE __m512i wiener_convolve_v16x2_tap3(const __m512i coeffs[2], const __m512i round_v, __m512i s[3]) {
    const __m512i dst = wiener_convolve_v_tap3_kernel_avx512(coeffs, round_v, s);
    s[0]              = s[2];
    return dst;
}

SIMD_INLINE __m512i wiener_convolve_v16x2_tap5(const __m512i coeffs[2], const __m512i round_v, __m512i s[5]) {
    const __m512i dst = wiener_convolve_v_tap5_kernel_avx512(coeffs, round_v, s);
    s[0]              = s[2];
    s[1]              = s[3];
    s[2]              = s[4];
    return dst;
}

SIMD_INLINE __m512i wiener_convolve_v16x2_tap7(const __m512i coeffs[2], const __m512i round_v, __m512i s[7]) {
    const __m512i dst = wiener_convolve_v_tap7_kernel_avx512(coeffs, round_v, s);
    s[0]              = s[2];
    s[1]              = s[3];
    s[2]              = s[4];
    s[3]              = s[5];
    s[4]              = s[6];
    return dst;
}

SIMD_INLINE __m512i wiener_convolve_v32_tap3(const __m512i coeffs[2], const __m512i round_v, __m512i s[3]) {
    const __m512i dst = wiener_convolve_v_tap3_kernel_avx512(coeffs, round_v, s);
    s[0]              = s[1];
    s[1]              = s[2];
    return dst;
}

static INLINE __m512i wiener_convolve_v32_tap5(const __m512i coeffs[2], const __m512i round_v, __m512i s[5]) {
    const __m512i dst = wiener_convolve_v_tap5_kernel_avx512(coeffs, round_v, s);
    s[0]              = s[1];
    s[1]              = s[2];
    s[2]              = s[3];
    s[3]              = s[4];
    return dst;
}

static INLINE __m512i wiener_convolve_v32_tap7(const __m512i coeffs[2], const __m512i round_v, __m512i s[7]) {
    const __m512i dst = wiener_convolve_v_tap7_kernel_avx512(coeffs, round_v, s);
    s[0]              = s[1];
    s[1]              = s[2];
    s[2]              = s[3];
    s[3]              = s[4];
    s[4]              = s[5];
    s[5]              = s[6];
    return dst;
}

static INLINE void pack_store_32x2_avx512(const __m512i res0, const __m512i res1, uint8_t* const dst,
                                          const ptrdiff_t stride) {
    const __m512i d = _mm512_packus_epi16(res0, res1);
    storeu_u8_32x2_avx512(d, dst, stride);
}

// Note: If this function crash in Windows, please pay attention to the pointer
// filter_x, which could be overridden by other instructions. It's a bug from
// Visual Studio compiler. Please adjust the positions of the following 2
// instructions randomly to work around, or even duplicate instruction 1 to
// several locations before coeffs_x is referenced.
// 1. const __m128i coeffs_x = xx_loadu_128(filter_x);
// 2. const int cnt_zero_coef = calc_zero_coef(filter_x, filter_y);
void svt_av1_wiener_convolve_add_src_avx512(const uint8_t* const src, const ptrdiff_t src_stride, uint8_t* const dst,
                                            const ptrdiff_t dst_stride, const int16_t* const filter_x,
                                            const int16_t* const filter_y, const int32_t w, const int32_t h,
                                            const ConvolveParams* const conv_params) {
    const int32_t  bd                  = 8;
    const int      center_tap          = (SUBPEL_TAPS - 1) / 2;
    const int      round_0             = WIENER_ROUND0_BITS;
    const int      round_1             = 2 * FILTER_BITS - WIENER_ROUND0_BITS;
    const int      cnt_zero_coef       = calc_zero_coef(filter_x, filter_y);
    const uint8_t* src_ptr             = src - center_tap * src_stride - center_tap;
    const __m256i  round_h0            = _mm256_set1_epi16((1 << (round_0 - 1)));
    const __m512i  round_h0_512        = _mm512_set1_epi16((1 << (round_0 - 1)));
    const __m256i  round_h1            = _mm256_set1_epi16((1 << (bd + FILTER_BITS - round_0 - 1)));
    const __m512i  round_h1_512        = _mm512_set1_epi16((1 << (bd + FILTER_BITS - round_0 - 1)));
    const __m256i  round_v             = _mm256_set1_epi32((1 << (round_1 - 1)) - (1 << (bd + round_1 - 1)));
    const __m512i  round_v_512         = _mm512_set1_epi32((1 << (round_1 - 1)) - (1 << (bd + round_1 - 1)));
    const __m256i  clamp_high          = _mm256_set1_epi16(WIENER_CLAMP_LIMIT(round_0, bd) - 1);
    const __m512i  clamp_high_512      = _mm512_set1_epi16(WIENER_CLAMP_LIMIT(round_0, bd) - 1);
    const __m128i  zero_128            = _mm_setzero_si128();
    const __m128i  offset_0            = _mm_insert_epi16(zero_128, 1 << FILTER_BITS, 3);
    const __m128i  coeffs_y            = _mm_add_epi16(xx_loadu_128(filter_y), offset_0);
    const __m256i  filter_coeffs_y     = _mm256_broadcastsi128_si256(coeffs_y);
    const __m512i  filter_coeffs_y_512 = svt_mm512_broadcast_i64x2(coeffs_y);
    int32_t        width               = w;
    uint8_t*       dst_ptr             = dst;
    __m256i        filt[4], coeffs_h[4], coeffs_v[2];
    __m512i        filt_512[4], coeffs_h_512[4], coeffs_v_512[2];

    (void)conv_params;
    assert(!(w % 8));
    assert(conv_params->round_0 == round_0);
    assert(conv_params->round_1 == round_1);

    filt[0]     = yy_load_256(filt1_global_avx);
    filt[1]     = yy_load_256(filt2_global_avx);
    filt_512[0] = zz_load_512(filt1_global_avx);
    filt_512[1] = zz_load_512(filt2_global_avx);

    if (!cnt_zero_coef) {
        const __m128i coeffs_x = xx_loadu_128(filter_x);
        if (width >= 32) {
            int32_t x = width & ~63;

            const __m512i filt_center_512 = zz_load_512(filt_center_tap7_global_avx);
            filt_512[2]                   = zz_load_512(filt3_global_avx);
            filt_512[3]                   = zz_load_512(filt4_global_avx);
            populate_coeffs_8tap_avx512(coeffs_x, coeffs_h_512);
            // coeffs 0 1 0 1 0 1 0 1
            coeffs_v_512[0] = _mm512_shuffle_epi32(filter_coeffs_y_512, 0x00);
            // coeffs 2 3 2 3 2 3 2 3
            coeffs_v_512[1] = _mm512_shuffle_epi32(filter_coeffs_y_512, 0x55);

            width -= x;
            while (x) {
                const uint8_t* src_p = src_ptr;
                __m512i        s[2][7];

                wiener_convolve_h64_tap7_avx512(src_p,
                                                coeffs_h_512,
                                                filt_512,
                                                filt_center_512,
                                                round_h0_512,
                                                round_h1_512,
                                                clamp_high_512,
                                                &s[0][0],
                                                &s[1][0]);
                src_p += src_stride;
                wiener_convolve_h64_tap7_avx512(src_p,
                                                coeffs_h_512,
                                                filt_512,
                                                filt_center_512,
                                                round_h0_512,
                                                round_h1_512,
                                                clamp_high_512,
                                                &s[0][1],
                                                &s[1][1]);
                src_p += src_stride;
                wiener_convolve_h64_tap7_avx512(src_p,
                                                coeffs_h_512,
                                                filt_512,
                                                filt_center_512,
                                                round_h0_512,
                                                round_h1_512,
                                                clamp_high_512,
                                                &s[0][2],
                                                &s[1][2]);
                src_p += src_stride;
                wiener_convolve_h64_tap7_avx512(src_p,
                                                coeffs_h_512,
                                                filt_512,
                                                filt_center_512,
                                                round_h0_512,
                                                round_h1_512,
                                                clamp_high_512,
                                                &s[0][3],
                                                &s[1][3]);
                src_p += src_stride;
                wiener_convolve_h64_tap7_avx512(src_p,
                                                coeffs_h_512,
                                                filt_512,
                                                filt_center_512,
                                                round_h0_512,
                                                round_h1_512,
                                                clamp_high_512,
                                                &s[0][4],
                                                &s[1][4]);
                src_p += src_stride;
                wiener_convolve_h64_tap7_avx512(src_p,
                                                coeffs_h_512,
                                                filt_512,
                                                filt_center_512,
                                                round_h0_512,
                                                round_h1_512,
                                                clamp_high_512,
                                                &s[0][5],
                                                &s[1][5]);
                src_p += src_stride;

                int y = 0;
                do {
                    wiener_convolve_h64_tap7_avx512(src_p,
                                                    coeffs_h_512,
                                                    filt_512,
                                                    filt_center_512,
                                                    round_h0_512,
                                                    round_h1_512,
                                                    clamp_high_512,
                                                    &s[0][6],
                                                    &s[1][6]);
                    src_p += src_stride;
                    const __m512i r0 = wiener_convolve_v32_tap7(coeffs_v_512, round_v_512, s[0]);
                    const __m512i r1 = wiener_convolve_v32_tap7(coeffs_v_512, round_v_512, s[1]);
                    convolve_store_64_avx512(r0, r1, dst_ptr + y * dst_stride);
                } while (++y < h);

                src_ptr += 64;
                dst_ptr += 64;
                x -= 64;
            }

            if (!width) {
                return;
            }

            x = width & ~31;
            if (x) {
                const uint8_t* src_p = src_ptr;
                uint8_t*       dst_p = dst_ptr;
                __m512i        s[2][7];

                wiener_convolve_h32x2_tap7(src_p,
                                           src_stride,
                                           coeffs_h_512,
                                           filt_512,
                                           filt_center_512,
                                           round_h0_512,
                                           round_h1_512,
                                           clamp_high_512,
                                           &s[0][0],
                                           &s[1][0]);
                src_p += 2 * src_stride;
                wiener_convolve_h32x2_tap7(src_p,
                                           src_stride,
                                           coeffs_h_512,
                                           filt_512,
                                           filt_center_512,
                                           round_h0_512,
                                           round_h1_512,
                                           clamp_high_512,
                                           &s[0][2],
                                           &s[1][2]);
                src_p += 2 * src_stride;
                wiener_convolve_h32x2_tap7(src_p,
                                           src_stride,
                                           coeffs_h_512,
                                           filt_512,
                                           filt_center_512,
                                           round_h0_512,
                                           round_h1_512,
                                           clamp_high_512,
                                           &s[0][4],
                                           &s[1][4]);
                src_p += 2 * src_stride;

                s[0][1] = _mm512_setr_m256i(_mm512_extracti64x4_epi64(s[0][0], 1), _mm512_castsi512_si256(s[0][2]));
                s[1][1] = _mm512_setr_m256i(_mm512_extracti64x4_epi64(s[1][0], 1), _mm512_castsi512_si256(s[1][2]));
                s[0][3] = _mm512_setr_m256i(_mm512_extracti64x4_epi64(s[0][2], 1), _mm512_castsi512_si256(s[0][4]));
                s[1][3] = _mm512_setr_m256i(_mm512_extracti64x4_epi64(s[1][2], 1), _mm512_castsi512_si256(s[1][4]));

                int y = h;
                do {
                    wiener_convolve_h32x2_tap7(src_p,
                                               src_stride,
                                               coeffs_h_512,
                                               filt_512,
                                               filt_center_512,
                                               round_h0_512,
                                               round_h1_512,
                                               clamp_high_512,
                                               &s[0][6],
                                               &s[1][6]);
                    s[0][5] = _mm512_setr_m256i(_mm512_extracti64x4_epi64(s[0][4], 1), _mm512_castsi512_si256(s[0][6]));
                    s[1][5] = _mm512_setr_m256i(_mm512_extracti64x4_epi64(s[1][4], 1), _mm512_castsi512_si256(s[1][6]));
                    src_p += 2 * src_stride;
                    const __m512i r0 = wiener_convolve_v16x2_tap7(coeffs_v_512, round_v_512, s[0]);
                    const __m512i r1 = wiener_convolve_v16x2_tap7(coeffs_v_512, round_v_512, s[1]);
                    if (y == 1) {
                        const __m512i d  = _mm512_packus_epi16(r0, r1);
                        const __m256i d0 = _mm512_castsi512_si256(d);
                        _mm256_storeu_si256((__m256i*)dst_p, d0);
                    } else {
                        pack_store_32x2_avx512(r0, r1, dst_p, dst_stride);
                    }

                    dst_p += 2 * dst_stride;
                    y -= 2;
                } while (y > 0);

                src_ptr += 32;
                dst_ptr += 32;
                width -= 32;
            }

            if (!width) {
                return;
            }
        }

        const __m256i filt_center = yy_load_256(filt_center_tap7_global_avx);
        filt[2]                   = yy_load_256(filt3_global_avx);
        filt[3]                   = yy_load_256(filt4_global_avx);
        populate_coeffs_8tap_avx2(coeffs_x, coeffs_h);
        // coeffs 0 1 0 1 0 1 0 1
        coeffs_v[0] = _mm256_shuffle_epi32(filter_coeffs_y, 0x00);
        // coeffs 2 3 2 3 2 3 2 3
        coeffs_v[1] = _mm256_shuffle_epi32(filter_coeffs_y, 0x55);

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
        __m128i coeffs_x = xx_loadu_128(filter_x);
        src_ptr += src_stride + 1;

        if (width >= 32) {
            int32_t x = width & ~63;

            const __m512i filt_center_512 = zz_load_512(filt_center_tap5_global_avx);
            filt_512[2]                   = zz_load_512(filt3_global_avx);
            populate_coeffs_6tap_avx512(coeffs_x, coeffs_h_512);
            // coeffs 1 2 1 2 1 2 1 2
            coeffs_v_512[0] = _mm512_shuffle_epi8(filter_coeffs_y_512, _mm512_set1_epi32(0x05040302u));
            // coeffs 3 4 3 4 3 4 3 4
            coeffs_v_512[1] = _mm512_shuffle_epi8(filter_coeffs_y_512, _mm512_set1_epi32(0x09080706u));

            width -= x;
            while (x) {
                const uint8_t* src_p = src_ptr;
                __m512i        s[2][5];

                wiener_convolve_h64_tap5(src_p,
                                         coeffs_h_512,
                                         filt_512,
                                         filt_center_512,
                                         round_h0_512,
                                         round_h1_512,
                                         clamp_high_512,
                                         &s[0][0],
                                         &s[1][0]);
                src_p += src_stride;
                wiener_convolve_h64_tap5(src_p,
                                         coeffs_h_512,
                                         filt_512,
                                         filt_center_512,
                                         round_h0_512,
                                         round_h1_512,
                                         clamp_high_512,
                                         &s[0][1],
                                         &s[1][1]);
                src_p += src_stride;
                wiener_convolve_h64_tap5(src_p,
                                         coeffs_h_512,
                                         filt_512,
                                         filt_center_512,
                                         round_h0_512,
                                         round_h1_512,
                                         clamp_high_512,
                                         &s[0][2],
                                         &s[1][2]);
                src_p += src_stride;
                wiener_convolve_h64_tap5(src_p,
                                         coeffs_h_512,
                                         filt_512,
                                         filt_center_512,
                                         round_h0_512,
                                         round_h1_512,
                                         clamp_high_512,
                                         &s[0][3],
                                         &s[1][3]);
                src_p += src_stride;

                int y = 0;
                do {
                    wiener_convolve_h64_tap5(src_p,
                                             coeffs_h_512,
                                             filt_512,
                                             filt_center_512,
                                             round_h0_512,
                                             round_h1_512,
                                             clamp_high_512,
                                             &s[0][4],
                                             &s[1][4]);
                    src_p += src_stride;
                    const __m512i r0 = wiener_convolve_v32_tap5(coeffs_v_512, round_v_512, s[0]);
                    const __m512i r1 = wiener_convolve_v32_tap5(coeffs_v_512, round_v_512, s[1]);
                    convolve_store_64_avx512(r0, r1, dst_ptr + y * dst_stride);
                } while (++y < h);

                src_ptr += 64;
                dst_ptr += 64;
                x -= 64;
            }

            if (!width) {
                return;
            }

            x = width & ~31;
            if (x) {
                const uint8_t* src_p = src_ptr;
                uint8_t*       dst_p = dst_ptr;
                __m512i        s[2][5];

                wiener_convolve_h32x2_tap5(src_p,
                                           src_stride,
                                           coeffs_h_512,
                                           filt_512,
                                           filt_center_512,
                                           round_h0_512,
                                           round_h1_512,
                                           clamp_high_512,
                                           &s[0][0],
                                           &s[1][0]);
                src_p += 2 * src_stride;
                wiener_convolve_h32x2_tap5(src_p,
                                           src_stride,
                                           coeffs_h_512,
                                           filt_512,
                                           filt_center_512,
                                           round_h0_512,
                                           round_h1_512,
                                           clamp_high_512,
                                           &s[0][2],
                                           &s[1][2]);
                src_p += 2 * src_stride;

                s[0][1] = _mm512_setr_m256i(_mm512_extracti64x4_epi64(s[0][0], 1), _mm512_castsi512_si256(s[0][2]));
                s[1][1] = _mm512_setr_m256i(_mm512_extracti64x4_epi64(s[1][0], 1), _mm512_castsi512_si256(s[1][2]));

                int y = h;
                do {
                    wiener_convolve_h32x2_tap5(src_p,
                                               src_stride,
                                               coeffs_h_512,
                                               filt_512,
                                               filt_center_512,
                                               round_h0_512,
                                               round_h1_512,
                                               clamp_high_512,
                                               &s[0][4],
                                               &s[1][4]);
                    s[0][3] = _mm512_setr_m256i(_mm512_extracti64x4_epi64(s[0][2], 1), _mm512_castsi512_si256(s[0][4]));
                    s[1][3] = _mm512_setr_m256i(_mm512_extracti64x4_epi64(s[1][2], 1), _mm512_castsi512_si256(s[1][4]));
                    src_p += 2 * src_stride;
                    const __m512i r0 = wiener_convolve_v16x2_tap5(coeffs_v_512, round_v_512, s[0]);
                    const __m512i r1 = wiener_convolve_v16x2_tap5(coeffs_v_512, round_v_512, s[1]);
                    if (y == 1) {
                        const __m512i d  = _mm512_packus_epi16(r0, r1);
                        const __m256i d0 = _mm512_castsi512_si256(d);
                        _mm256_storeu_si256((__m256i*)dst_p, d0);
                    } else {
                        pack_store_32x2_avx512(r0, r1, dst_p, dst_stride);
                    }

                    dst_p += 2 * dst_stride;
                    y -= 2;
                } while (y > 0);

                src_ptr += 32;
                dst_ptr += 32;
                width -= 32;
            }

            if (!width) {
                return;
            }
        }

        const __m256i filt_center = yy_load_256(filt_center_tap5_global_avx);
        filt[2]                   = yy_load_256(filt3_global_avx);
        coeffs_x                  = xx_loadu_128(filter_x);
        populate_coeffs_6tap_avx2(coeffs_x, coeffs_h);
        // coeffs 1 2 1 2 1 2 1 2
        coeffs_v[0] = _mm256_shuffle_epi8(filter_coeffs_y, _mm256_set1_epi32(0x05040302u));
        // coeffs 3 4 3 4 3 4 3 4
        coeffs_v[1] = _mm256_shuffle_epi8(filter_coeffs_y, _mm256_set1_epi32(0x09080706u));

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
        const __m128i coeffs_x = xx_loadu_128(filter_x);
        src_ptr += 2 * src_stride + 2;

        if (width >= 32) {
            int32_t x = width & ~63;

            const __m512i filt_center_512 = zz_load_512(filt_center_tap3_global_avx);
            populate_coeffs_4tap_avx512(coeffs_x, coeffs_h_512);
            // coeffs 2 3 2 3 2 3 2 3
            coeffs_v_512[0] = _mm512_shuffle_epi32(filter_coeffs_y_512, 0x55);
            // coeffs 4 5 4 5 4 5 4 5
            coeffs_v_512[1] = _mm512_shuffle_epi32(filter_coeffs_y_512, 0xaa);

            width -= x;
            while (x) {
                const uint8_t* src_p = src_ptr;
                __m512i        s[2][3];

                wiener_convolve_h64_tap3(src_p,
                                         coeffs_h_512,
                                         filt_512,
                                         filt_center_512,
                                         round_h0_512,
                                         round_h1_512,
                                         clamp_high_512,
                                         &s[0][0],
                                         &s[1][0]);
                src_p += src_stride;
                wiener_convolve_h64_tap3(src_p,
                                         coeffs_h_512,
                                         filt_512,
                                         filt_center_512,
                                         round_h0_512,
                                         round_h1_512,
                                         clamp_high_512,
                                         &s[0][1],
                                         &s[1][1]);
                src_p += src_stride;

                int y = 0;
                do {
                    wiener_convolve_h64_tap3(src_p,
                                             coeffs_h_512,
                                             filt_512,
                                             filt_center_512,
                                             round_h0_512,
                                             round_h1_512,
                                             clamp_high_512,
                                             &s[0][2],
                                             &s[1][2]);
                    src_p += src_stride;
                    const __m512i r0 = wiener_convolve_v32_tap3(coeffs_v_512, round_v_512, s[0]);
                    const __m512i r1 = wiener_convolve_v32_tap3(coeffs_v_512, round_v_512, s[1]);
                    convolve_store_64_avx512(r0, r1, dst_ptr + y * dst_stride);
                } while (++y < h);

                src_ptr += 64;
                dst_ptr += 64;
                x -= 64;
            }

            if (!width) {
                return;
            }

            x = width & ~31;
            if (x) {
                const uint8_t* src_p = src_ptr;
                uint8_t*       dst_p = dst_ptr;
                __m512i        s[2][3];

                wiener_convolve_h32x2_tap3(src_p,
                                           src_stride,
                                           coeffs_h_512,
                                           filt_512,
                                           filt_center_512,
                                           round_h0_512,
                                           round_h1_512,
                                           clamp_high_512,
                                           &s[0][0],
                                           &s[1][0]);
                src_p += 2 * src_stride;

                int y = h;
                do {
                    wiener_convolve_h32x2_tap3(src_p,
                                               src_stride,
                                               coeffs_h_512,
                                               filt_512,
                                               filt_center_512,
                                               round_h0_512,
                                               round_h1_512,
                                               clamp_high_512,
                                               &s[0][2],
                                               &s[1][2]);
                    s[0][1] = _mm512_setr_m256i(_mm512_extracti64x4_epi64(s[0][0], 1), _mm512_castsi512_si256(s[0][2]));
                    s[1][1] = _mm512_setr_m256i(_mm512_extracti64x4_epi64(s[1][0], 1), _mm512_castsi512_si256(s[1][2]));
                    src_p += 2 * src_stride;
                    const __m512i r0 = wiener_convolve_v16x2_tap3(coeffs_v_512, round_v_512, s[0]);
                    const __m512i r1 = wiener_convolve_v16x2_tap3(coeffs_v_512, round_v_512, s[1]);
                    if (y == 1) {
                        const __m512i d  = _mm512_packus_epi16(r0, r1);
                        const __m256i d0 = _mm512_castsi512_si256(d);
                        _mm256_storeu_si256((__m256i*)dst_p, d0);
                    } else {
                        pack_store_32x2_avx512(r0, r1, dst_p, dst_stride);
                    }

                    dst_p += 2 * dst_stride;
                    y -= 2;
                } while (y > 0);

                src_ptr += 32;
                dst_ptr += 32;
                width -= 32;
            }

            if (!width) {
                return;
            }
        }

        const __m256i filt_center = yy_load_256(filt_center_tap3_global_avx);
        populate_coeffs_4tap_avx2(coeffs_x, coeffs_h);
        // coeffs 2 3 2 3 2 3 2 3
        coeffs_v[0] = _mm256_shuffle_epi32(filter_coeffs_y, 0x55);
        // coeffs 4 5 4 5 4 5 4 5
        coeffs_v[1] = _mm256_shuffle_epi32(filter_coeffs_y, 0xaa);

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

#endif // EN_AVX512_SUPPORT
