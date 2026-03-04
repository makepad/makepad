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

#ifndef AOM_DSP_X86_CONVOLVE_AVX2_H_
#define AOM_DSP_X86_CONVOLVE_AVX2_H_

#include "convolve.h"
#include "definitions.h"
#include "inter_prediction.h"
#include "memory_avx2.h"
#include "memory_sse4_1.h"
#include "synonyms.h"
#include "synonyms_avx2.h"

#define LEFT_SHIFT (2 * FILTER_BITS - 3 - COMPOUND_ROUND1_BITS)

// filters for 16
DECLARE_ALIGNED(64, static const uint8_t, filt1_global_avx[]) = {
    0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8,
    0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8};

DECLARE_ALIGNED(64, static const uint8_t, filt2_global_avx[]) = {
    2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10,
    2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10};

DECLARE_ALIGNED(64, static const uint8_t, filt3_global_avx[]) = {
    4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12,
    4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12};

DECLARE_ALIGNED(64, static const uint8_t, filt4_global_avx[]) = {
    6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13, 14, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13, 14,
    6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13, 14, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13, 14};

void convolve_2d_sr_hor_4tap_ssse3(const uint8_t* const src, const int32_t src_stride, const int32_t w, const int32_t h,
                                   const InterpFilterParams* const filter_params_x, const int32_t subpel_x_q4,
                                   int16_t* const im_block);

void jnt_convolve_y_4tap_avx2(const uint8_t* const src, const int32_t src_stride, uint8_t* dst8,
                              const int32_t dst8_stride, const int32_t w, const int32_t h,
                              const InterpFilterParams* const filter_params_y, const int32_t subpel_y_q4,
                              const ConvolveParams* const conv_params);

void jnt_convolve_x_4tap_ssse3(const uint8_t* const src, const int32_t src_stride, uint8_t* dst8,
                               const int32_t dst8_stride, const int32_t w, const int32_t h,
                               const InterpFilterParams* const filter_params_x, const int32_t subpel_x_q4,
                               const ConvolveParams* const conv_params);

void jnt_convolve_2d_hor_4tap_avx2(const uint8_t* src, const int32_t src_stride, const int32_t w, const int32_t h,
                                   const InterpFilterParams* filter_params_x, const int32_t subpel_x_q4,
                                   int16_t* const im_block);

void jnt_convolve_2d_ver_4tap_avx2(const int16_t* const im_block, const int32_t w, const int32_t h,
                                   const InterpFilterParams* const filter_params_y, const int32_t subpel_y_q4,
                                   const ConvolveParams* const conv_params, uint8_t* dst8, const int32_t dst8_stride);

static INLINE bool is_convolve_2tap(const int16_t* const filter) {
    return (const void*)filter == (const void*)bilinear_filters;
}

static INLINE bool is_convolve_4tap(const int16_t* const filter) {
    return (const void*)filter == (const void*)sub_pel_filters_4 ||
        (const void*)filter == (const void*)sub_pel_filters_4smooth;
}

static INLINE bool is_convolve_6tap(const int16_t* const filter) {
    return (const void*)filter == (const void*)sub_pel_filters_8 ||
        (const void*)filter == (const void*)sub_pel_filters_8smooth;
}

static INLINE int32_t get_convolve_tap(const int16_t* const filter) {
    if (is_convolve_2tap(filter)) {
        return 2;
    } else if (is_convolve_4tap(filter)) {
        return 4;
    } else if (is_convolve_6tap(filter)) {
        return 6;
    } else {
        return 8;
    }
}

static INLINE void populate_coeffs_4tap_avx2(const __m128i coeffs_128, __m256i coeffs[2]) {
    const __m256i coeffs_256 = _mm256_broadcastsi128_si256(coeffs_128);

    // coeffs 2 3 2 3 2 3 2 3
    coeffs[0] = _mm256_shuffle_epi8(coeffs_256, _mm256_set1_epi16(0x0604u));
    // coeffs 4 5 4 5 4 5 4 5
    coeffs[1] = _mm256_shuffle_epi8(coeffs_256, _mm256_set1_epi16(0x0a08u));
}

static INLINE void populate_coeffs_6tap_avx2(const __m128i coeffs_128, __m256i coeffs[3]) {
    const __m256i coeffs_256 = _mm256_broadcastsi128_si256(coeffs_128);

    // coeffs 1 2 1 2 1 2 1 2
    coeffs[0] = _mm256_shuffle_epi8(coeffs_256, _mm256_set1_epi16(0x0402u));
    // coeffs 3 4 3 4 3 4 3 4
    coeffs[1] = _mm256_shuffle_epi8(coeffs_256, _mm256_set1_epi16(0x0806u));
    // coeffs 5 6 5 6 5 6 5 6
    coeffs[2] = _mm256_shuffle_epi8(coeffs_256, _mm256_set1_epi16(0x0C0Au));
}

static INLINE void populate_coeffs_8tap_avx2(const __m128i coeffs_128, __m256i coeffs[4]) {
    const __m256i coeffs_256 = _mm256_broadcastsi128_si256(coeffs_128);

    // coeffs 0 1 0 1 0 1 0 1
    coeffs[0] = _mm256_shuffle_epi8(coeffs_256, _mm256_set1_epi16(0x0200u));
    // coeffs 2 3 2 3 2 3 2 3
    coeffs[1] = _mm256_shuffle_epi8(coeffs_256, _mm256_set1_epi16(0x0604u));
    // coeffs 4 5 4 5 4 5 4 5
    coeffs[2] = _mm256_shuffle_epi8(coeffs_256, _mm256_set1_epi16(0x0a08u));
    // coeffs 6 7 6 7 6 7 6 7
    coeffs[3] = _mm256_shuffle_epi8(coeffs_256, _mm256_set1_epi16(0x0e0cu));
}

static INLINE void prepare_half_coeffs_2tap_ssse3(const InterpFilterParams* const filter_params,
                                                  const int32_t subpel_q4, __m128i* const coeffs /* [1] */) {
    const int16_t* const filter   = av1_get_interp_filter_subpel_kernel(*filter_params, subpel_q4 & SUBPEL_MASK);
    const __m128i        coeffs_8 = _mm_cvtsi32_si128(*(const int32_t*)(filter + 3));

    // right shift all filter co-efficients by 1 to reduce the bits required.
    // This extra right shift will be taken care of at the end while rounding
    // the result.
    // Since all filter co-efficients are even, this change will not affect the
    // end result
    assert(_mm_test_all_zeros(_mm_and_si128(coeffs_8, _mm_set1_epi16(1)), _mm_set1_epi16((short)0xffff)));

    const __m128i coeffs_1 = _mm_srai_epi16(coeffs_8, 1);

    // coeffs 3 4 3 4 3 4 3 4
    *coeffs = _mm_shuffle_epi8(coeffs_1, _mm_set1_epi16(0x0200u));
}

static INLINE void prepare_half_coeffs_4tap_ssse3(const InterpFilterParams* const filter_params,
                                                  const int32_t subpel_q4, __m128i* const coeffs /* [2] */) {
    const int16_t* const filter   = av1_get_interp_filter_subpel_kernel(*filter_params, subpel_q4 & SUBPEL_MASK);
    const __m128i        coeffs_8 = _mm_loadu_si128((__m128i*)filter);

    // right shift all filter co-efficients by 1 to reduce the bits required.
    // This extra right shift will be taken care of at the end while rounding
    // the result.
    // Since all filter co-efficients are even, this change will not affect the
    // end result
    assert(_mm_test_all_zeros(_mm_and_si128(coeffs_8, _mm_set1_epi16(1)), _mm_set1_epi16((short)0xffff)));

    const __m128i coeffs_1 = _mm_srai_epi16(coeffs_8, 1);

    // coeffs 2 3 2 3 2 3 2 3
    coeffs[0] = _mm_shuffle_epi8(coeffs_1, _mm_set1_epi16(0x0604u));
    // coeffs 4 5 4 5 4 5 4 5
    coeffs[1] = _mm_shuffle_epi8(coeffs_1, _mm_set1_epi16(0x0a08u));
}

static INLINE void prepare_half_coeffs_6tap_ssse3(const InterpFilterParams* const filter_params,
                                                  const int32_t subpel_q4, __m128i* const coeffs /* [3] */) {
    const int16_t* const filter   = av1_get_interp_filter_subpel_kernel(*filter_params, subpel_q4 & SUBPEL_MASK);
    const __m128i        coeffs_8 = _mm_loadu_si128((__m128i*)filter);

    // right shift all filter co-efficients by 1 to reduce the bits required.
    // This extra right shift will be taken care of at the end while rounding
    // the result.
    // Since all filter co-efficients are even, this change will not affect the
    // end result
    assert(_mm_test_all_zeros(_mm_and_si128(coeffs_8, _mm_set1_epi16(1)), _mm_set1_epi16((short)0xffff)));

    const __m128i coeffs_1 = _mm_srai_epi16(coeffs_8, 1);

    // coeffs 1 2 1 2 1 2 1 2
    coeffs[0] = _mm_shuffle_epi8(coeffs_1, _mm_set1_epi16(0x0402u));
    // coeffs 3 4 3 4 3 4 3 4
    coeffs[1] = _mm_shuffle_epi8(coeffs_1, _mm_set1_epi16(0x0806u));
    // coeffs 5 6 5 6 5 6 5 6
    coeffs[2] = _mm_shuffle_epi8(coeffs_1, _mm_set1_epi16(0x0C0Au));
}

static INLINE void prepare_half_coeffs_8tap_ssse3(const InterpFilterParams* const filter_params,
                                                  const int32_t subpel_q4, __m128i* const coeffs /* [4] */) {
    const int16_t* const filter   = av1_get_interp_filter_subpel_kernel(*filter_params, subpel_q4 & SUBPEL_MASK);
    const __m128i        coeffs_8 = _mm_loadu_si128((__m128i*)filter);

    // right shift all filter co-efficients by 1 to reduce the bits required.
    // This extra right shift will be taken care of at the end while rounding
    // the result.
    // Since all filter co-efficients are even, this change will not affect the
    // end result
    assert(_mm_test_all_zeros(_mm_and_si128(coeffs_8, _mm_set1_epi16(1)), _mm_set1_epi16((short)0xffff)));

    const __m128i coeffs_1 = _mm_srai_epi16(coeffs_8, 1);

    // coeffs 0 1 0 1 0 1 0 1
    coeffs[0] = _mm_shuffle_epi8(coeffs_1, _mm_set1_epi16(0x0200u));
    // coeffs 2 3 2 3 2 3 2 3
    coeffs[1] = _mm_shuffle_epi8(coeffs_1, _mm_set1_epi16(0x0604u));
    // coeffs 4 5 4 5 4 5 4 5
    coeffs[2] = _mm_shuffle_epi8(coeffs_1, _mm_set1_epi16(0x0a08u));
    // coeffs 6 7 6 7 6 7 6 7
    coeffs[3] = _mm_shuffle_epi8(coeffs_1, _mm_set1_epi16(0x0e0cu));
}

static INLINE void prepare_half_coeffs_2tap_avx2(const InterpFilterParams* const filter_params, const int32_t subpel_q4,
                                                 __m256i* const coeffs /* [1] */) {
    const int16_t* const filter        = av1_get_interp_filter_subpel_kernel(*filter_params, subpel_q4 & SUBPEL_MASK);
    const __m128i        coeffs_8      = _mm_cvtsi32_si128(*(const int32_t*)(filter + 3));
    const __m256i        filter_coeffs = _mm256_broadcastsi128_si256(coeffs_8);

    // right shift all filter co-efficients by 1 to reduce the bits required.
    // This extra right shift will be taken care of at the end while rounding
    // the result.
    // Since all filter co-efficients are even, this change will not affect the
    // end result
    assert(_mm_test_all_zeros(_mm_and_si128(coeffs_8, _mm_set1_epi16(1)), _mm_set1_epi16((short)0xffff)));

    const __m256i coeffs_1 = _mm256_srai_epi16(filter_coeffs, 1);

    // coeffs 3 4 3 4 3 4 3 4
    *coeffs = _mm256_shuffle_epi8(coeffs_1, _mm256_set1_epi16(0x0200u));
}

static INLINE void prepare_half_coeffs_4tap_avx2(const InterpFilterParams* const filter_params, const int32_t subpel_q4,
                                                 __m256i* const coeffs /* [2] */) {
    const int16_t* const filter   = av1_get_interp_filter_subpel_kernel(*filter_params, subpel_q4 & SUBPEL_MASK);
    const __m128i        coeffs_8 = _mm_loadu_si128((__m128i*)filter);

    // right shift all filter co-efficients by 1 to reduce the bits required.
    // This extra right shift will be taken care of at the end while rounding
    // the result.
    // Since all filter co-efficients are even, this change will not affect the
    // end result
    assert(_mm_test_all_zeros(_mm_and_si128(coeffs_8, _mm_set1_epi16(1)), _mm_set1_epi16((short)0xffff)));
    const __m128i coeffs_1 = _mm_srai_epi16(coeffs_8, 1);
    populate_coeffs_4tap_avx2(coeffs_1, coeffs);
}

static INLINE void prepare_half_coeffs_6tap_avx2(const InterpFilterParams* const filter_params, const int32_t subpel_q4,
                                                 __m256i* const coeffs /* [3] */) {
    const int16_t* const filter   = av1_get_interp_filter_subpel_kernel(*filter_params, subpel_q4 & SUBPEL_MASK);
    const __m128i        coeffs_8 = _mm_loadu_si128((__m128i*)filter);

    // right shift all filter co-efficients by 1 to reduce the bits required.
    // This extra right shift will be taken care of at the end while rounding
    // the result.
    // Since all filter co-efficients are even, this change will not affect the
    // end result
    assert(_mm_test_all_zeros(_mm_and_si128(coeffs_8, _mm_set1_epi16(1)), _mm_set1_epi16((short)0xffff)));
    const __m128i coeffs_1 = _mm_srai_epi16(coeffs_8, 1);
    populate_coeffs_6tap_avx2(coeffs_1, coeffs);
}

static INLINE void prepare_half_coeffs_8tap_avx2(const InterpFilterParams* const filter_params, const int32_t subpel_q4,
                                                 __m256i* const coeffs /* [4] */) {
    const int16_t* const filter   = av1_get_interp_filter_subpel_kernel(*filter_params, subpel_q4 & SUBPEL_MASK);
    const __m128i        coeffs_8 = _mm_loadu_si128((__m128i*)filter);

    // right shift all filter co-efficients by 1 to reduce the bits required.
    // This extra right shift will be taken care of at the end while rounding
    // the result.
    // Since all filter co-efficients are even, this change will not affect the
    // end result
    assert(_mm_test_all_zeros(_mm_and_si128(coeffs_8, _mm_set1_epi16(1)), _mm_set1_epi16((short)0xffff)));
    const __m128i coeffs_1 = _mm_srai_epi16(coeffs_8, 1);
    populate_coeffs_8tap_avx2(coeffs_1, coeffs);
}

static INLINE void prepare_coeffs_2tap_sse2(const InterpFilterParams* const filter_params, const int32_t subpel_q4,
                                            __m128i* const coeffs /* [1] */) {
    const int16_t* filter = av1_get_interp_filter_subpel_kernel(*filter_params, subpel_q4 & SUBPEL_MASK);

    const __m128i coeff = _mm_cvtsi32_si128(*(const int32_t*)(filter + 3));

    // coeffs 3 4 3 4 3 4 3 4
    coeffs[0] = _mm_shuffle_epi32(coeff, 0x00);
}

static INLINE void prepare_coeffs_4tap_sse2(const InterpFilterParams* const filter_params, const int32_t subpel_q4,
                                            __m128i* const coeffs /* [2] */) {
    const int16_t* filter = av1_get_interp_filter_subpel_kernel(*filter_params, subpel_q4 & SUBPEL_MASK);

    const __m128i coeff = _mm_loadu_si128((__m128i*)filter);

    // coeffs 2 3 2 3 2 3 2 3
    coeffs[0] = _mm_shuffle_epi32(coeff, 0x55);
    // coeffs 4 5 4 5 4 5 4 5
    coeffs[1] = _mm_shuffle_epi32(coeff, 0xaa);
}

static INLINE void prepare_coeffs_6tap_ssse3(const InterpFilterParams* const filter_params, const int32_t subpel_q4,
                                             __m128i* const coeffs /* [3] */) {
    const int16_t* const filter = av1_get_interp_filter_subpel_kernel(*filter_params, subpel_q4 & SUBPEL_MASK);
    const __m128i        coeff  = _mm_loadu_si128((__m128i*)filter);

    // coeffs 1 2 1 2 1 2 1 2
    coeffs[0] = _mm_shuffle_epi8(coeff, _mm_set1_epi32(0x05040302u));
    // coeffs 3 4 3 4 3 4 3 4
    coeffs[1] = _mm_shuffle_epi8(coeff, _mm_set1_epi32(0x09080706u));
    // coeffs 5 6 5 6 5 6 5 6
    coeffs[2] = _mm_shuffle_epi8(coeff, _mm_set1_epi32(0x0D0C0B0Au));
}

static INLINE void prepare_coeffs_8tap_sse2(const InterpFilterParams* const filter_params, const int32_t subpel_q4,
                                            __m128i* const coeffs /* [4] */) {
    const int16_t* filter = av1_get_interp_filter_subpel_kernel(*filter_params, subpel_q4 & SUBPEL_MASK);

    const __m128i coeff = _mm_loadu_si128((__m128i*)filter);

    // coeffs 0 1 0 1 0 1 0 1
    coeffs[0] = _mm_shuffle_epi32(coeff, 0x00);
    // coeffs 2 3 2 3 2 3 2 3
    coeffs[1] = _mm_shuffle_epi32(coeff, 0x55);
    // coeffs 4 5 4 5 4 5 4 5
    coeffs[2] = _mm_shuffle_epi32(coeff, 0xaa);
    // coeffs 6 7 6 7 6 7 6 7
    coeffs[3] = _mm_shuffle_epi32(coeff, 0xff);
}

static INLINE void prepare_coeffs_2tap_avx2(const InterpFilterParams* const filter_params, const int32_t subpel_q4,
                                            __m256i* const coeffs /* [1] */) {
    const int16_t* filter = av1_get_interp_filter_subpel_kernel(*filter_params, subpel_q4 & SUBPEL_MASK);

    const __m128i coeff_8 = _mm_cvtsi32_si128(*(const int32_t*)(filter + 3));
    const __m256i coeff   = _mm256_broadcastsi128_si256(coeff_8);

    // coeffs 3 4 3 4 3 4 3 4
    coeffs[0] = _mm256_shuffle_epi32(coeff, 0x00);
}

static INLINE void prepare_coeffs_4tap_avx2(const InterpFilterParams* const filter_params, const int32_t subpel_q4,
                                            __m256i* const coeffs /* [2] */) {
    const int16_t* filter = av1_get_interp_filter_subpel_kernel(*filter_params, subpel_q4 & SUBPEL_MASK);

    const __m128i coeff_8 = _mm_loadu_si128((__m128i*)filter);
    const __m256i coeff   = _mm256_broadcastsi128_si256(coeff_8);

    // coeffs 2 3 2 3 2 3 2 3
    coeffs[0] = _mm256_shuffle_epi32(coeff, 0x55);
    // coeffs 4 5 4 5 4 5 4 5
    coeffs[1] = _mm256_shuffle_epi32(coeff, 0xaa);
}

static INLINE void prepare_coeffs_6tap_avx2(const InterpFilterParams* const filter_params, const int32_t subpel_q4,
                                            __m256i* const coeffs /* [3] */) {
    const int16_t* const filter   = av1_get_interp_filter_subpel_kernel(*filter_params, subpel_q4 & SUBPEL_MASK);
    const __m128i        coeffs_8 = _mm_loadu_si128((__m128i*)filter);
    const __m256i        coeff    = _mm256_broadcastsi128_si256(coeffs_8);

    // coeffs 1 2 1 2 1 2 1 2
    coeffs[0] = _mm256_shuffle_epi8(coeff, _mm256_set1_epi32(0x05040302u));
    // coeffs 3 4 3 4 3 4 3 4
    coeffs[1] = _mm256_shuffle_epi8(coeff, _mm256_set1_epi32(0x09080706u));
    // coeffs 5 6 5 6 5 6 5 6
    coeffs[2] = _mm256_shuffle_epi8(coeff, _mm256_set1_epi32(0x0D0C0B0Au));
}

static INLINE void prepare_coeffs_8tap_avx2(const InterpFilterParams* const filter_params, const int32_t subpel_q4,
                                            __m256i* const coeffs /* [4] */) {
    const int16_t* filter = av1_get_interp_filter_subpel_kernel(*filter_params, subpel_q4 & SUBPEL_MASK);

    const __m128i coeff_8 = _mm_loadu_si128((__m128i*)filter);
    const __m256i coeff   = _mm256_broadcastsi128_si256(coeff_8);

    // coeffs 0 1 0 1 0 1 0 1
    coeffs[0] = _mm256_shuffle_epi32(coeff, 0x00);
    // coeffs 2 3 2 3 2 3 2 3
    coeffs[1] = _mm256_shuffle_epi32(coeff, 0x55);
    // coeffs 4 5 4 5 4 5 4 5
    coeffs[2] = _mm256_shuffle_epi32(coeff, 0xaa);
    // coeffs 6 7 6 7 6 7 6 7
    coeffs[3] = _mm256_shuffle_epi32(coeff, 0xff);
}

static INLINE void load_16bit_5rows_avx2(const int16_t* const src, const ptrdiff_t stride, __m256i dst[5]) {
    dst[0] = _mm256_loadu_si256((__m256i*)(src + 0 * stride));
    dst[1] = _mm256_loadu_si256((__m256i*)(src + 1 * stride));
    dst[2] = _mm256_loadu_si256((__m256i*)(src + 2 * stride));
    dst[3] = _mm256_loadu_si256((__m256i*)(src + 3 * stride));
    dst[4] = _mm256_loadu_si256((__m256i*)(src + 4 * stride));
}

static INLINE void load_16bit_7rows_avx2(const int16_t* const src, const ptrdiff_t stride, __m256i dst[7]) {
    dst[0] = _mm256_loadu_si256((__m256i*)(src + 0 * stride));
    dst[1] = _mm256_loadu_si256((__m256i*)(src + 1 * stride));
    dst[2] = _mm256_loadu_si256((__m256i*)(src + 2 * stride));
    dst[3] = _mm256_loadu_si256((__m256i*)(src + 3 * stride));
    dst[4] = _mm256_loadu_si256((__m256i*)(src + 4 * stride));
    dst[5] = _mm256_loadu_si256((__m256i*)(src + 5 * stride));
    dst[6] = _mm256_loadu_si256((__m256i*)(src + 6 * stride));
}

SIMD_INLINE void load_16bit_8rows_avx2(const int16_t* const src, const ptrdiff_t stride, __m256i dst[8]) {
    dst[0] = _mm256_loadu_si256((__m256i*)(src + 0 * stride));
    dst[1] = _mm256_loadu_si256((__m256i*)(src + 1 * stride));
    dst[2] = _mm256_loadu_si256((__m256i*)(src + 2 * stride));
    dst[3] = _mm256_loadu_si256((__m256i*)(src + 3 * stride));
    dst[4] = _mm256_loadu_si256((__m256i*)(src + 4 * stride));
    dst[5] = _mm256_loadu_si256((__m256i*)(src + 5 * stride));
    dst[6] = _mm256_loadu_si256((__m256i*)(src + 6 * stride));
    dst[7] = _mm256_loadu_si256((__m256i*)(src + 7 * stride));
}

SIMD_INLINE void loadu_unpack_16bit_5rows_avx2(const int16_t* const src, const ptrdiff_t stride, __m256i s_256[5],
                                               __m256i ss_256[5], __m256i tt_256[5]) {
    s_256[0] = _mm256_loadu_si256((__m256i*)(src + 0 * stride));
    s_256[1] = _mm256_loadu_si256((__m256i*)(src + 1 * stride));
    s_256[2] = _mm256_loadu_si256((__m256i*)(src + 2 * stride));
    s_256[3] = _mm256_loadu_si256((__m256i*)(src + 3 * stride));
    s_256[4] = _mm256_loadu_si256((__m256i*)(src + 4 * stride));

    ss_256[0] = _mm256_unpacklo_epi16(s_256[0], s_256[1]);
    ss_256[1] = _mm256_unpacklo_epi16(s_256[2], s_256[3]);
    ss_256[3] = _mm256_unpackhi_epi16(s_256[0], s_256[1]);
    ss_256[4] = _mm256_unpackhi_epi16(s_256[2], s_256[3]);

    tt_256[0] = _mm256_unpacklo_epi16(s_256[1], s_256[2]);
    tt_256[1] = _mm256_unpacklo_epi16(s_256[3], s_256[4]);
    tt_256[3] = _mm256_unpackhi_epi16(s_256[1], s_256[2]);
    tt_256[4] = _mm256_unpackhi_epi16(s_256[3], s_256[4]);
}

SIMD_INLINE void loadu_unpack_16bit_3rows_avx2(const int16_t* const src, const ptrdiff_t stride, __m256i s_256[3],
                                               __m256i ss_256[3], __m256i tt_256[3]) {
    s_256[0] = _mm256_loadu_si256((__m256i*)(src + 0 * stride));
    s_256[1] = _mm256_loadu_si256((__m256i*)(src + 1 * stride));
    s_256[2] = _mm256_loadu_si256((__m256i*)(src + 2 * stride));

    ss_256[0] = _mm256_unpacklo_epi16(s_256[0], s_256[1]);
    ss_256[2] = _mm256_unpackhi_epi16(s_256[0], s_256[1]);

    tt_256[0] = _mm256_unpacklo_epi16(s_256[1], s_256[2]);
    tt_256[2] = _mm256_unpackhi_epi16(s_256[1], s_256[2]);
}

static INLINE void convolve_8tap_unapck_avx2(const __m256i s[6], __m256i ss[7]) {
    ss[0] = _mm256_unpacklo_epi16(s[0], s[1]);
    ss[1] = _mm256_unpacklo_epi16(s[2], s[3]);
    ss[2] = _mm256_unpacklo_epi16(s[4], s[5]);
    ss[4] = _mm256_unpackhi_epi16(s[0], s[1]);
    ss[5] = _mm256_unpackhi_epi16(s[2], s[3]);
    ss[6] = _mm256_unpackhi_epi16(s[4], s[5]);
}

static INLINE __m128i convolve_2tap_ssse3(const __m128i ss[1], const __m128i coeffs[1]) {
    return _mm_maddubs_epi16(ss[0], coeffs[0]);
}

static INLINE __m128i convolve_4tap_ssse3(const __m128i ss[2], const __m128i coeffs[2]) {
    const __m128i res_23 = _mm_maddubs_epi16(ss[0], coeffs[0]);
    const __m128i res_45 = _mm_maddubs_epi16(ss[1], coeffs[1]);
    return _mm_add_epi16(res_23, res_45);
}

static INLINE __m128i convolve_6tap_ssse3(const __m128i ss[3], const __m128i coeffs[3]) {
    const __m128i res_12   = _mm_maddubs_epi16(ss[0], coeffs[0]);
    const __m128i res_34   = _mm_maddubs_epi16(ss[1], coeffs[1]);
    const __m128i res_56   = _mm_maddubs_epi16(ss[2], coeffs[2]);
    const __m128i res_1256 = _mm_add_epi16(res_12, res_56);
    return _mm_add_epi16(res_1256, res_34);
}

static INLINE __m128i convolve_8tap_ssse3(const __m128i ss[4], const __m128i coeffs[4]) {
    const __m128i res_01   = _mm_maddubs_epi16(ss[0], coeffs[0]);
    const __m128i res_23   = _mm_maddubs_epi16(ss[1], coeffs[1]);
    const __m128i res_45   = _mm_maddubs_epi16(ss[2], coeffs[2]);
    const __m128i res_67   = _mm_maddubs_epi16(ss[3], coeffs[3]);
    const __m128i res_0145 = _mm_add_epi16(res_01, res_45);
    const __m128i res_2367 = _mm_add_epi16(res_23, res_67);
    return _mm_add_epi16(res_0145, res_2367);
}

static INLINE __m256i convolve_2tap_avx2(const __m256i ss[1], const __m256i coeffs[1]) {
    return _mm256_maddubs_epi16(ss[0], coeffs[0]);
}

static INLINE __m256i convolve_4tap_avx2(const __m256i ss[2], const __m256i coeffs[2]) {
    const __m256i res_23 = _mm256_maddubs_epi16(ss[0], coeffs[0]);
    const __m256i res_45 = _mm256_maddubs_epi16(ss[1], coeffs[1]);
    return _mm256_add_epi16(res_23, res_45);
}

static INLINE __m256i convolve_6tap_avx2(const __m256i ss[3], const __m256i coeffs[3]) {
    const __m256i res_01   = _mm256_maddubs_epi16(ss[0], coeffs[0]);
    const __m256i res_23   = _mm256_maddubs_epi16(ss[1], coeffs[1]);
    const __m256i res_45   = _mm256_maddubs_epi16(ss[2], coeffs[2]);
    const __m256i res_0145 = _mm256_add_epi16(res_01, res_45);
    return _mm256_add_epi16(res_0145, res_23);
}

static INLINE __m256i convolve_8tap_avx2(const __m256i ss[4], const __m256i coeffs[4]) {
    const __m256i res_01   = _mm256_maddubs_epi16(ss[0], coeffs[0]);
    const __m256i res_23   = _mm256_maddubs_epi16(ss[1], coeffs[1]);
    const __m256i res_45   = _mm256_maddubs_epi16(ss[2], coeffs[2]);
    const __m256i res_67   = _mm256_maddubs_epi16(ss[3], coeffs[3]);
    const __m256i res_0145 = _mm256_add_epi16(res_01, res_45);
    const __m256i res_2367 = _mm256_add_epi16(res_23, res_67);
    return _mm256_add_epi16(res_0145, res_2367);
}

static INLINE __m128i convolve16_2tap_sse2(const __m128i ss[1], const __m128i coeffs[1]) {
    return _mm_madd_epi16(ss[0], coeffs[0]);
}

static INLINE __m128i convolve16_4tap_sse2(const __m128i ss[2], const __m128i coeffs[2]) {
    const __m128i res_01 = _mm_madd_epi16(ss[0], coeffs[0]);
    const __m128i res_23 = _mm_madd_epi16(ss[1], coeffs[1]);
    return _mm_add_epi32(res_01, res_23);
}

static INLINE __m128i convolve16_6tap_sse2(const __m128i ss[3], const __m128i coeffs[3]) {
    const __m128i res_01   = _mm_madd_epi16(ss[0], coeffs[0]);
    const __m128i res_23   = _mm_madd_epi16(ss[1], coeffs[1]);
    const __m128i res_45   = _mm_madd_epi16(ss[2], coeffs[2]);
    const __m128i res_0123 = _mm_add_epi32(res_01, res_23);
    return _mm_add_epi32(res_0123, res_45);
}

static INLINE __m128i convolve16_8tap_sse2(const __m128i ss[4], const __m128i coeffs[4]) {
    const __m128i res_01   = _mm_madd_epi16(ss[0], coeffs[0]);
    const __m128i res_23   = _mm_madd_epi16(ss[1], coeffs[1]);
    const __m128i res_45   = _mm_madd_epi16(ss[2], coeffs[2]);
    const __m128i res_67   = _mm_madd_epi16(ss[3], coeffs[3]);
    const __m128i res_0123 = _mm_add_epi32(res_01, res_23);
    const __m128i res_4567 = _mm_add_epi32(res_45, res_67);
    return _mm_add_epi32(res_0123, res_4567);
}

static INLINE __m256i convolve16_2tap_avx2(const __m256i ss[1], const __m256i coeffs[1]) {
    return _mm256_madd_epi16(ss[0], coeffs[0]);
}

static INLINE __m256i convolve16_4tap_avx2(const __m256i ss[2], const __m256i coeffs[2]) {
    const __m256i res_1 = _mm256_madd_epi16(ss[0], coeffs[0]);
    const __m256i res_2 = _mm256_madd_epi16(ss[1], coeffs[1]);
    return _mm256_add_epi32(res_1, res_2);
}

static INLINE __m256i convolve16_6tap_avx2(const __m256i ss[3], const __m256i coeffs[3]) {
    const __m256i res_01   = _mm256_madd_epi16(ss[0], coeffs[0]);
    const __m256i res_23   = _mm256_madd_epi16(ss[1], coeffs[1]);
    const __m256i res_45   = _mm256_madd_epi16(ss[2], coeffs[2]);
    const __m256i res_0123 = _mm256_add_epi32(res_01, res_23);
    return _mm256_add_epi32(res_0123, res_45);
}

static INLINE __m256i convolve16_8tap_avx2(const __m256i ss[4], const __m256i coeffs[4]) {
    const __m256i res_01   = _mm256_madd_epi16(ss[0], coeffs[0]);
    const __m256i res_23   = _mm256_madd_epi16(ss[1], coeffs[1]);
    const __m256i res_45   = _mm256_madd_epi16(ss[2], coeffs[2]);
    const __m256i res_67   = _mm256_madd_epi16(ss[3], coeffs[3]);
    const __m256i res_0123 = _mm256_add_epi32(res_01, res_23);
    const __m256i res_4567 = _mm256_add_epi32(res_45, res_67);
    return _mm256_add_epi32(res_0123, res_4567);
}

static INLINE __m256i x_convolve_4tap_avx2(const __m256i data, const __m256i coeffs[2], const __m256i filt[2]) {
    __m256i ss[2];

    ss[0] = _mm256_shuffle_epi8(data, filt[0]);
    ss[1] = _mm256_shuffle_epi8(data, filt[1]);

    return convolve_4tap_avx2(ss, coeffs);
}

static INLINE __m256i x_convolve_6tap_avx2(const __m256i data, const __m256i coeffs[3], const __m256i filt[3]) {
    __m256i ss[3];

    ss[0] = _mm256_shuffle_epi8(data, filt[0]);
    ss[1] = _mm256_shuffle_epi8(data, filt[1]);
    ss[2] = _mm256_shuffle_epi8(data, filt[2]);

    return convolve_6tap_avx2(ss, coeffs);
}

static INLINE __m256i x_convolve_8tap_avx2(const __m256i data, const __m256i coeffs[4], const __m256i filt[4]) {
    __m256i ss[4];

    ss[0] = _mm256_shuffle_epi8(data, filt[0]);
    ss[1] = _mm256_shuffle_epi8(data, filt[1]);
    ss[2] = _mm256_shuffle_epi8(data, filt[2]);
    ss[3] = _mm256_shuffle_epi8(data, filt[3]);

    return convolve_8tap_avx2(ss, coeffs);
}

static INLINE __m256i sr_y_round_avx2(const __m256i src) {
    const __m256i round = _mm256_set1_epi16(32);
    const __m256i dst   = _mm256_add_epi16(src, round);
    return _mm256_srai_epi16(dst, FILTER_BITS - 1);
}

static INLINE __m128i xy_x_round_sse2(const __m128i src) {
    const __m128i round = _mm_set1_epi16(2);
    const __m128i dst   = _mm_add_epi16(src, round);
    return _mm_srai_epi16(dst, 2);
}

static INLINE __m256i xy_x_round_avx2(const __m256i src) {
    const __m256i round = _mm256_set1_epi16(2);
    const __m256i dst   = _mm256_add_epi16(src, round);
    return _mm256_srai_epi16(dst, 2);
}

static INLINE void xy_x_round_store_2x2_sse2(const __m128i res, int16_t* const dst) {
    const __m128i d = xy_x_round_sse2(res);
    _mm_storel_epi64((__m128i*)dst, d);
}

static INLINE void xy_x_round_store_4x2_sse2(const __m128i res, int16_t* const dst) {
    const __m128i d = xy_x_round_sse2(res);
    _mm_storeu_si128((__m128i*)dst, d);
}

static INLINE void xy_x_round_store_8x2_sse2(const __m128i res[2], int16_t* const dst) {
    __m128i r[2];

    r[0] = xy_x_round_sse2(res[0]);
    r[1] = xy_x_round_sse2(res[1]);
    _mm_storeu_si128((__m128i*)dst, r[0]);
    _mm_storeu_si128((__m128i*)(dst + 8), r[1]);
}

static INLINE void xy_x_round_store_8x2_avx2(const __m256i res, int16_t* const dst) {
    const __m256i d = xy_x_round_avx2(res);
    _mm256_storeu_si256((__m256i*)dst, d);
}

static INLINE void xy_x_round_store_32_avx2(const __m256i res[2], int16_t* const dst) {
    __m256i r[2];

    r[0]             = xy_x_round_avx2(res[0]);
    r[1]             = xy_x_round_avx2(res[1]);
    const __m256i d0 = yy_unpacklo_epi128(r[0], r[1]);
    const __m256i d1 = yy_unpackhi_epi128(r[0], r[1]);
    _mm256_storeu_si256((__m256i*)dst, d0);
    _mm256_storeu_si256((__m256i*)(dst + 16), d1);
}

static INLINE __m128i xy_y_round_sse2(const __m128i src) {
    const __m128i round = _mm_set1_epi32(1024);
    const __m128i dst   = _mm_add_epi32(src, round);
    return _mm_srai_epi32(dst, 11);
}

static INLINE __m128i xy_y_round_half_pel_sse2(const __m128i src) {
    const __m128i round = _mm_set1_epi16(16);
    const __m128i dst   = _mm_add_epi16(src, round);
    return _mm_srai_epi16(dst, 5);
}

static INLINE __m256i xy_y_round_avx2(const __m256i src) {
    const __m256i round = _mm256_set1_epi32(1024);
    const __m256i dst   = _mm256_add_epi32(src, round);
    return _mm256_srai_epi32(dst, 11);
}

static INLINE __m256i xy_y_round_16_avx2(const __m256i r[2]) {
    const __m256i r0 = xy_y_round_avx2(r[0]);
    const __m256i r1 = xy_y_round_avx2(r[1]);
    return _mm256_packs_epi32(r0, r1);
}

static INLINE __m256i xy_y_round_half_pel_avx2(const __m256i src) {
    const __m256i round = _mm256_set1_epi16(16);
    const __m256i dst   = _mm256_add_epi16(src, round);
    return _mm256_srai_epi16(dst, 5);
}

static INLINE __m128i jnt_y_round_sse2(const __m128i src) {
    const __m128i round = _mm_set1_epi16(2);
    const __m128i dst   = _mm_add_epi16(src, round);
    return _mm_srai_epi16(dst, 2);
}

static INLINE __m256i jnt_y_round_avx2(const __m256i src) {
    const __m256i round = _mm256_set1_epi16(2);
    const __m256i dst   = _mm256_add_epi16(src, round);
    return _mm256_srai_epi16(dst, 2);
}

static INLINE __m128i jnt_avg_round_sse2(const __m128i src, const __m128i offset) {
    const __m128i dst = _mm_add_epi16(src, offset);
    return _mm_srai_epi16(dst, 2);
}

static INLINE __m256i jnt_avg_round_avx2(const __m256i src, const __m256i offset) {
    const __m256i dst = _mm256_add_epi16(src, offset);
    return _mm256_srai_epi16(dst, 2);
}

static INLINE __m128i jnt_no_avg_round_sse2(const __m128i src, const __m128i offset) {
    const __m128i dst = _mm_add_epi16(src, offset);
    return _mm_srli_epi16(dst, 2);
}

static INLINE __m256i jnt_no_avg_round_avx2(const __m256i src, const __m256i offset) {
    const __m256i dst = _mm256_add_epi16(src, offset);
    return _mm256_srli_epi16(dst, 2);
}

static INLINE void pack_store_2x2_sse2(const __m128i res, uint8_t* const dst, const ptrdiff_t stride) {
    const __m128i d           = _mm_packus_epi16(res, res);
    *(int16_t*)dst            = (int16_t)_mm_cvtsi128_si32(d);
    *(int16_t*)(dst + stride) = (int16_t)_mm_extract_epi16(d, 1);
}

static INLINE void pack_store_4x2_sse2(const __m128i res, uint8_t* const dst, const ptrdiff_t stride) {
    const __m128i d = _mm_packus_epi16(res, res);
    store_u8_4x2_sse2(d, dst, stride);
}

static INLINE void pack_store_4x2_avx2(const __m256i res, uint8_t* const dst, const ptrdiff_t stride) {
    const __m256i d  = _mm256_packus_epi16(res, res);
    const __m128i d0 = _mm256_castsi256_si128(d);
    const __m128i d1 = _mm256_extracti128_si256(d, 1);
    xx_storel_32(dst, d0);
    xx_storel_32(dst + stride, d1);
}

static INLINE void pack_store_8x2_avx2(const __m256i res, uint8_t* const dst, const ptrdiff_t stride) {
    const __m256i d  = _mm256_packus_epi16(res, res);
    const __m128i d0 = _mm256_castsi256_si128(d);
    const __m128i d1 = _mm256_extracti128_si256(d, 1);
    _mm_storel_epi64((__m128i*)dst, d0);
    _mm_storel_epi64((__m128i*)(dst + stride), d1);
}

static INLINE void pack_store_16x2_avx2(const __m256i res0, const __m256i res1, uint8_t* const dst,
                                        const ptrdiff_t stride) {
    const __m256i d = _mm256_packus_epi16(res0, res1);
    storeu_u8_16x2_avx2(d, dst, stride);
}

static INLINE void xy_y_pack_store_16x2_avx2(const __m256i res0, const __m256i res1, uint8_t* const dst,
                                             const ptrdiff_t stride) {
    const __m256i t = _mm256_packus_epi16(res0, res1);
    const __m256i d = _mm256_permute4x64_epi64(t, 0xD8);
    storeu_u8_16x2_avx2(d, dst, stride);
}

static INLINE void pack_store_32_avx2(const __m256i res0, const __m256i res1, uint8_t* const dst) {
    const __m256i t = _mm256_packus_epi16(res0, res1);
    const __m256i d = _mm256_permute4x64_epi64(t, 0xD8);
    _mm256_storeu_si256((__m256i*)dst, d);
}

static INLINE void xy_y_round_store_2x2_sse2(const __m128i res, uint8_t* const dst, const ptrdiff_t stride) {
    const __m128i r  = xy_y_round_sse2(res);
    const __m128i rr = _mm_packs_epi32(r, r);
    pack_store_2x2_sse2(rr, dst, stride);
}

static INLINE void xy_y_round_store_4x2_avx2(const __m256i res, uint8_t* const dst, const ptrdiff_t stride) {
    const __m256i r  = xy_y_round_avx2(res);
    const __m256i rr = _mm256_packs_epi32(r, r);
    pack_store_4x2_avx2(rr, dst, stride);
}

static INLINE void xy_y_pack_store_32_avx2(const __m256i res0, const __m256i res1, uint8_t* const dst) {
    const __m256i d = _mm256_packus_epi16(res0, res1);
    // d = _mm256_permute4x64_epi64(d, 0xD8);
    _mm256_storeu_si256((__m256i*)dst, d);
}

static INLINE void xy_y_round_store_32_avx2(const __m256i r0[2], const __m256i r1[2], uint8_t* const dst) {
    const __m256i ra = xy_y_round_16_avx2(r0);
    const __m256i rb = xy_y_round_16_avx2(r1);
    xy_y_pack_store_32_avx2(ra, rb, dst);
}

static INLINE void convolve_store_32_avx2(const __m256i res0, const __m256i res1, uint8_t* const dst) {
    const __m256i d = _mm256_packus_epi16(res0, res1);
    _mm256_storeu_si256((__m256i*)dst, d);
}

static INLINE __m128i sr_x_round_sse2(const __m128i src) {
    const __m128i round = _mm_set1_epi16(34);
    const __m128i dst   = _mm_add_epi16(src, round);
    return _mm_srai_epi16(dst, 6);
}

static INLINE __m256i sr_x_round_avx2(const __m256i src) {
    const __m256i round = _mm256_set1_epi16(34);
    const __m256i dst   = _mm256_add_epi16(src, round);
    return _mm256_srai_epi16(dst, 6);
}

static INLINE __m128i sr_y_round_sse2(const __m128i src) {
    const __m128i round = _mm_set1_epi16(32);
    const __m128i dst   = _mm_add_epi16(src, round);
    return _mm_srai_epi16(dst, FILTER_BITS - 1);
}

static INLINE void sr_x_round_store_8x2_avx2(const __m256i res, uint8_t* const dst, const ptrdiff_t dst_stride) {
    const __m256i r = sr_x_round_avx2(res);
    pack_store_8x2_avx2(r, dst, dst_stride);
}

static INLINE void sr_x_round_store_16x2_avx2(const __m256i res[2], uint8_t* const dst, const ptrdiff_t dst_stride) {
    __m256i r[2];

    r[0] = sr_x_round_avx2(res[0]);
    r[1] = sr_x_round_avx2(res[1]);
    pack_store_16x2_avx2(r[0], r[1], dst, dst_stride);
}

static INLINE void sr_x_round_store_32_avx2(const __m256i res[2], uint8_t* const dst) {
    __m256i r[2];

    r[0] = sr_x_round_avx2(res[0]);
    r[1] = sr_x_round_avx2(res[1]);
    convolve_store_32_avx2(r[0], r[1], dst);
}

static INLINE void sr_y_round_store_8x2_avx2(const __m256i res, uint8_t* const dst, const ptrdiff_t dst_stride) {
    const __m256i r = sr_y_round_avx2(res);
    pack_store_8x2_avx2(r, dst, dst_stride);
}

static INLINE void sr_y_round_store_16x2_avx2(const __m256i res[2], uint8_t* const dst, const ptrdiff_t dst_stride) {
    __m256i r[2];

    r[0] = sr_y_round_avx2(res[0]);
    r[1] = sr_y_round_avx2(res[1]);
    pack_store_16x2_avx2(r[0], r[1], dst, dst_stride);
}

static INLINE void sr_y_2tap_32_avg_avx2(const uint8_t* const src, const __m256i s0, __m256i* const s1,
                                         uint8_t* const dst) {
    *s1             = _mm256_loadu_si256((__m256i*)src);
    const __m256i d = _mm256_avg_epu8(s0, *s1);
    _mm256_storeu_si256((__m256i*)dst, d);
}

static INLINE void sr_x_2tap_32_avg_avx2(const uint8_t* const src, uint8_t* const dst) {
    const __m256i s0 = _mm256_loadu_si256((__m256i*)src);
    const __m256i s1 = _mm256_loadu_si256((__m256i*)(src + 1));
    const __m256i d  = _mm256_avg_epu8(s0, s1);
    _mm256_storeu_si256((__m256i*)dst, d);
}

static INLINE void jnt_no_avg_store_16x2_avx2(const __m256i src0, const __m256i src1, ConvBufType* const dst,
                                              const ptrdiff_t stride) {
    const __m256i d0 = yy_unpacklo_epi128(src0, src1);
    const __m256i d1 = yy_unpackhi_epi128(src0, src1);
    _mm256_storeu_si256((__m256i*)dst, d0);
    _mm256_storeu_si256((__m256i*)(dst + stride), d1);
}

static INLINE __m128i x_convolve_2tap_2x2_sse4_1(const uint8_t* const src, const ptrdiff_t stride,
                                                 const __m128i coeffs[1]) {
    const __m128i sfl   = _mm_setr_epi8(0, 1, 1, 2, 4, 5, 5, 6, 0, 0, 0, 0, 0, 0, 0, 0);
    const __m128i s_128 = load_u8_4x2_sse4_1(src, stride);
    const __m128i ss    = _mm_shuffle_epi8(s_128, sfl);
    return convolve_2tap_ssse3(&ss, coeffs);
}

static INLINE __m128i x_convolve_2tap_4x2_ssse3(const uint8_t* const src, const ptrdiff_t stride,
                                                const __m128i coeffs[1]) {
    const __m128i sfl   = _mm_setr_epi8(0, 1, 1, 2, 2, 3, 3, 4, 8, 9, 9, 10, 10, 11, 11, 12);
    const __m128i s_128 = load_u8_8x2_sse2(src, stride);
    const __m128i ss    = _mm_shuffle_epi8(s_128, sfl);
    return convolve_2tap_ssse3(&ss, coeffs);
}

static INLINE void x_convolve_2tap_8x2_ssse3(const uint8_t* const src, const ptrdiff_t stride, const __m128i coeffs[1],
                                             __m128i r[2]) {
    __m128i       ss[2];
    const __m128i s00 = _mm_loadu_si128((__m128i*)src);
    const __m128i s10 = _mm_loadu_si128((__m128i*)(src + stride));
    const __m128i s01 = _mm_srli_si128(s00, 1);
    const __m128i s11 = _mm_srli_si128(s10, 1);
    ss[0]             = _mm_unpacklo_epi8(s00, s01);
    ss[1]             = _mm_unpacklo_epi8(s10, s11);

    r[0] = convolve_2tap_ssse3(&ss[0], coeffs);
    r[1] = convolve_2tap_ssse3(&ss[1], coeffs);
}

static INLINE __m256i x_convolve_2tap_8x2_avx2(const uint8_t* const src, const ptrdiff_t stride,
                                               const __m256i coeffs[1]) {
    __m128i s_128[2][2];
    __m256i s_256[2];

    s_128[0][0]      = _mm_loadu_si128((__m128i*)src);
    s_128[1][0]      = _mm_loadu_si128((__m128i*)(src + stride));
    s_128[0][1]      = _mm_srli_si128(s_128[0][0], 1);
    s_128[1][1]      = _mm_srli_si128(s_128[1][0], 1);
    s_256[0]         = _mm256_setr_m128i(s_128[0][0], s_128[1][0]);
    s_256[1]         = _mm256_setr_m128i(s_128[0][1], s_128[1][1]);
    const __m256i ss = _mm256_unpacklo_epi8(s_256[0], s_256[1]);
    return convolve_2tap_avx2(&ss, coeffs);
}

static INLINE void x_convolve_2tap_16x2_avx2(const uint8_t* const src, const ptrdiff_t stride, const __m256i coeffs[1],
                                             __m256i r[2]) {
    const __m256i s0_256 = loadu_8bit_16x2_avx2(src, stride);
    const __m256i s1_256 = loadu_8bit_16x2_avx2(src + 1, stride);
    const __m256i s0     = _mm256_unpacklo_epi8(s0_256, s1_256);
    const __m256i s1     = _mm256_unpackhi_epi8(s0_256, s1_256);
    r[0]                 = convolve_2tap_avx2(&s0, coeffs);
    r[1]                 = convolve_2tap_avx2(&s1, coeffs);
}

static INLINE void x_convolve_2tap_32_avx2(const uint8_t* const src, const __m256i coeffs[1], __m256i r[2]) {
    const __m256i s0  = _mm256_loadu_si256((__m256i*)src);
    const __m256i s1  = _mm256_loadu_si256((__m256i*)(src + 1));
    const __m256i ss0 = _mm256_unpacklo_epi8(s0, s1);
    const __m256i ss1 = _mm256_unpackhi_epi8(s0, s1);

    r[0] = convolve_2tap_avx2(&ss0, coeffs);
    r[1] = convolve_2tap_avx2(&ss1, coeffs);
}

static INLINE __m128i x_convolve_4tap_2x2_ssse3(const uint8_t* const src, const ptrdiff_t stride,
                                                const __m128i coeffs[2]) {
    const __m128i sfl0 = _mm_setr_epi8(0, 1, 1, 2, 8, 9, 9, 10, 0, 0, 0, 0, 0, 0, 0, 0);
    const __m128i sfl1 = _mm_setr_epi8(2, 3, 3, 4, 10, 11, 11, 12, 0, 0, 0, 0, 0, 0, 0, 0);
    const __m128i s    = load_u8_8x2_sse2(src, stride);
    __m128i       ss[2];

    ss[0] = _mm_shuffle_epi8(s, sfl0);
    ss[1] = _mm_shuffle_epi8(s, sfl1);
    return convolve_4tap_ssse3(ss, coeffs);
}

static INLINE __m128i x_convolve_4tap_4x2_ssse3(const uint8_t* const src, const ptrdiff_t stride,
                                                const __m128i coeffs[2]) {
    const __m128i s    = load_u8_8x2_sse2(src, stride);
    const __m128i sfl0 = _mm_setr_epi8(0, 1, 1, 2, 2, 3, 3, 4, 8, 9, 9, 10, 10, 11, 11, 12);
    const __m128i sfl1 = _mm_setr_epi8(2, 3, 3, 4, 4, 5, 5, 6, 10, 11, 11, 12, 12, 13, 13, 14);
    __m128i       ss[2];

    ss[0] = _mm_shuffle_epi8(s, sfl0);
    ss[1] = _mm_shuffle_epi8(s, sfl1);
    return convolve_4tap_ssse3(ss, coeffs);
}

static INLINE __m256i x_convolve_6tap_8x2_avx2(const uint8_t* const src, const ptrdiff_t stride,
                                               const __m256i coeffs[3], const __m256i filt[3]) {
    const __m256i s_256 = loadu_8bit_16x2_avx2(src, stride);
    return x_convolve_6tap_avx2(s_256, coeffs, filt);
}

static INLINE void x_convolve_6tap_16x2_avx2(const uint8_t* const src, const int32_t src_stride,
                                             const __m256i coeffs[3], const __m256i filt[3], __m256i r[2]) {
    r[0] = x_convolve_6tap_8x2_avx2(src + 0, src_stride, coeffs, filt);
    r[1] = x_convolve_6tap_8x2_avx2(src + 8, src_stride, coeffs, filt);
}

static INLINE void x_convolve_6tap_32_avx2(const uint8_t* const src, const __m256i coeffs[3], const __m256i filt[3],
                                           __m256i r[2]) {
    const __m256i s0_256 = _mm256_loadu_si256((__m256i*)src);
    const __m256i s1_256 = _mm256_loadu_si256((__m256i*)(src + 8));

    r[0] = x_convolve_6tap_avx2(s0_256, coeffs, filt);
    r[1] = x_convolve_6tap_avx2(s1_256, coeffs, filt);
}

static INLINE __m256i x_convolve_8tap_8x2_avx2(const uint8_t* const src, const ptrdiff_t stride,
                                               const __m256i coeffs[4], const __m256i filt[4]) {
    const __m256i s_256 = loadu_8bit_16x2_avx2(src, stride);
    return x_convolve_8tap_avx2(s_256, coeffs, filt);
}

SIMD_INLINE void x_convolve_8tap_16x2_avx2(const uint8_t* const src, const int32_t src_stride, const __m256i coeffs[4],
                                           const __m256i filt[4], __m256i r[2]) {
    r[0] = x_convolve_8tap_8x2_avx2(src + 0, src_stride, coeffs, filt);
    r[1] = x_convolve_8tap_8x2_avx2(src + 8, src_stride, coeffs, filt);
}

SIMD_INLINE void x_convolve_8tap_32_avx2(const uint8_t* const src, const __m256i coeffs[4], const __m256i filt[4],
                                         __m256i r[2]) {
    const __m256i s0_256 = _mm256_loadu_si256((__m256i*)src);
    const __m256i s1_256 = _mm256_loadu_si256((__m256i*)(src + 8));

    r[0] = x_convolve_8tap_avx2(s0_256, coeffs, filt);
    r[1] = x_convolve_8tap_avx2(s1_256, coeffs, filt);
}

static INLINE __m128i y_convolve_2tap_2x2_ssse3(const uint8_t* const src, const ptrdiff_t stride,
                                                const __m128i coeffs[1], __m128i s_16[2]) {
    __m128i s_128[2];

    s_16[1]          = _mm_cvtsi32_si128(*(int16_t*)(src + stride));
    s_128[0]         = _mm_unpacklo_epi16(s_16[0], s_16[1]);
    s_16[0]          = _mm_cvtsi32_si128(*(int16_t*)(src + 2 * stride));
    s_128[1]         = _mm_unpacklo_epi16(s_16[1], s_16[0]);
    const __m128i ss = _mm_unpacklo_epi8(s_128[0], s_128[1]);
    return convolve_2tap_ssse3(&ss, coeffs);
}

static INLINE __m128i y_convolve_2tap_4x2_ssse3(const uint8_t* const src, const ptrdiff_t stride,
                                                const __m128i coeffs[1], __m128i s_32[2]) {
    __m128i s_128[2];

    s_32[1]          = _mm_cvtsi32_si128(*(int32_t*)(src + stride));
    s_128[0]         = _mm_unpacklo_epi32(s_32[0], s_32[1]);
    s_32[0]          = _mm_cvtsi32_si128(*(int32_t*)(src + 2 * stride));
    s_128[1]         = _mm_unpacklo_epi32(s_32[1], s_32[0]);
    const __m128i ss = _mm_unpacklo_epi8(s_128[0], s_128[1]);
    return convolve_2tap_ssse3(&ss, coeffs);
}

static INLINE __m256i y_convolve_2tap_8x2_avx2(const uint8_t* const src, const ptrdiff_t stride,
                                               const __m256i coeffs[1], __m128i s_64[2]) {
    __m256i s_256[2];

    s_64[1]          = _mm_loadl_epi64((__m128i*)(src + stride));
    s_256[0]         = _mm256_setr_m128i(s_64[0], s_64[1]);
    s_64[0]          = _mm_loadl_epi64((__m128i*)(src + 2 * stride));
    s_256[1]         = _mm256_setr_m128i(s_64[1], s_64[0]);
    const __m256i ss = _mm256_unpacklo_epi8(s_256[0], s_256[1]);
    return convolve_2tap_avx2(&ss, coeffs);
}

static INLINE void y_convolve_2tap_16x2_avx2(const uint8_t* const src, const ptrdiff_t stride, const __m256i coeffs[1],
                                             __m128i s_128[2], __m256i r[2]) {
    __m256i s_256[2];

    s_128[1]          = _mm_loadu_si128((__m128i*)(src + stride));
    s_256[0]          = _mm256_setr_m128i(s_128[0], s_128[1]);
    s_128[0]          = _mm_loadu_si128((__m128i*)(src + 2 * stride));
    s_256[1]          = _mm256_setr_m128i(s_128[1], s_128[0]);
    const __m256i ss0 = _mm256_unpacklo_epi8(s_256[0], s_256[1]);
    const __m256i ss1 = _mm256_unpackhi_epi8(s_256[0], s_256[1]);
    r[0]              = convolve_2tap_avx2(&ss0, coeffs);
    r[1]              = convolve_2tap_avx2(&ss1, coeffs);
}

static INLINE void y_convolve_2tap_32_avx2(const uint8_t* const src, const __m256i coeffs[1], const __m256i s0,
                                           __m256i* const s1, __m256i r[2]) {
    *s1               = _mm256_loadu_si256((__m256i*)src);
    const __m256i ss0 = _mm256_unpacklo_epi8(s0, *s1);
    const __m256i ss1 = _mm256_unpackhi_epi8(s0, *s1);
    r[0]              = convolve_2tap_avx2(&ss0, coeffs);
    r[1]              = convolve_2tap_avx2(&ss1, coeffs);
}

static INLINE __m128i y_convolve_4tap_2x2_ssse3(const uint8_t* const src, const ptrdiff_t stride,
                                                const __m128i coeffs[2], __m128i s_16[4], __m128i ss_128[2]) {
    s_16[3]             = _mm_cvtsi32_si128(*(int16_t*)(src + stride));
    const __m128i src23 = _mm_unpacklo_epi16(s_16[2], s_16[3]);
    s_16[2]             = _mm_cvtsi32_si128(*(int16_t*)(src + 2 * stride));
    const __m128i src34 = _mm_unpacklo_epi16(s_16[3], s_16[2]);
    ss_128[1]           = _mm_unpacklo_epi8(src23, src34);
    return convolve_4tap_ssse3(ss_128, coeffs);
}

static INLINE __m128i y_convolve_4tap_4x2_ssse3(const uint8_t* const src, const ptrdiff_t stride,
                                                const __m128i coeffs[2], __m128i s_32[4], __m128i ss_128[2]) {
    s_32[3]             = _mm_cvtsi32_si128(*(int32_t*)(src + stride));
    const __m128i src23 = _mm_unpacklo_epi32(s_32[2], s_32[3]);
    s_32[2]             = _mm_cvtsi32_si128(*(int32_t*)(src + 2 * stride));
    const __m128i src34 = _mm_unpacklo_epi32(s_32[3], s_32[2]);
    ss_128[1]           = _mm_unpacklo_epi8(src23, src34);
    return convolve_4tap_ssse3(ss_128, coeffs);
}

static INLINE __m256i y_convolve_4tap_8x2_avx2(const uint8_t* const src, const ptrdiff_t stride,
                                               const __m256i coeffs[2], __m128i s_64[4], __m256i ss_256[2]) {
    s_64[3]             = _mm_loadl_epi64((__m128i*)(src + stride));
    const __m256i src23 = _mm256_setr_m128i(s_64[2], s_64[3]);
    s_64[2]             = _mm_loadl_epi64((__m128i*)(src + 2 * stride));
    const __m256i src34 = _mm256_setr_m128i(s_64[3], s_64[2]);
    ss_256[1]           = _mm256_unpacklo_epi8(src23, src34);
    return convolve_4tap_avx2(ss_256, coeffs);
}

static INLINE void y_convolve_4tap_16x2_avx2(const uint8_t* const src, const ptrdiff_t stride, const __m256i coeffs[2],
                                             __m128i s_128[4], __m256i ss_256[4], __m256i r[2]) {
    s_128[3]            = _mm_loadu_si128((__m128i*)(src + stride));
    const __m256i src23 = _mm256_setr_m128i(s_128[2], s_128[3]);
    s_128[2]            = _mm_loadu_si128((__m128i*)(src + 2 * stride));
    const __m256i src34 = _mm256_setr_m128i(s_128[3], s_128[2]);
    ss_256[1]           = _mm256_unpacklo_epi8(src23, src34);
    ss_256[3]           = _mm256_unpackhi_epi8(src23, src34);
    r[0]                = convolve_4tap_avx2(ss_256, coeffs);
    r[1]                = convolve_4tap_avx2(ss_256 + 2, coeffs);
}

static INLINE __m128i y_convolve_6tap_2x2_ssse3(const uint8_t* const src, const ptrdiff_t stride,
                                                const __m128i coeffs[3], __m128i s_16[6], __m128i ss_128[3]) {
    s_16[5]             = _mm_cvtsi32_si128(*(int16_t*)(src + 3 * stride));
    const __m128i src45 = _mm_unpacklo_epi16(s_16[4], s_16[5]);
    s_16[4]             = _mm_cvtsi32_si128(*(int16_t*)(src + 4 * stride));
    const __m128i src56 = _mm_unpacklo_epi16(s_16[5], s_16[4]);
    ss_128[2]           = _mm_unpacklo_epi8(src45, src56);
    return convolve_6tap_ssse3(ss_128, coeffs);
}

static INLINE void y_convolve_4tap_32x2_avx2(const uint8_t* const src, const ptrdiff_t stride, const __m256i coeffs[2],
                                             __m256i s_256[4], __m256i ss_256[4], __m256i tt_256[4], __m256i r[4]) {
    s_256[3]  = _mm256_loadu_si256((__m256i*)(src + 1 * stride));
    ss_256[1] = _mm256_unpacklo_epi8(s_256[2], s_256[3]);
    ss_256[3] = _mm256_unpackhi_epi8(s_256[2], s_256[3]);
    s_256[2]  = _mm256_loadu_si256((__m256i*)(src + 2 * stride));
    tt_256[1] = _mm256_unpacklo_epi8(s_256[3], s_256[2]);
    tt_256[3] = _mm256_unpackhi_epi8(s_256[3], s_256[2]);
    r[0]      = convolve_4tap_avx2(ss_256 + 0, coeffs);
    r[1]      = convolve_4tap_avx2(ss_256 + 2, coeffs);
    r[2]      = convolve_4tap_avx2(tt_256 + 0, coeffs);
    r[3]      = convolve_4tap_avx2(tt_256 + 2, coeffs);
}

static INLINE __m128i y_convolve_6tap_4x2_ssse3(const uint8_t* const src, const ptrdiff_t stride,
                                                const __m128i coeffs[3], __m128i s_32[6], __m128i ss_128[3]) {
    s_32[5]             = _mm_cvtsi32_si128(*(int32_t*)(src + 3 * stride));
    const __m128i src45 = _mm_unpacklo_epi32(s_32[4], s_32[5]);
    s_32[4]             = _mm_cvtsi32_si128(*(int32_t*)(src + 4 * stride));
    const __m128i src56 = _mm_unpacklo_epi32(s_32[5], s_32[4]);
    ss_128[2]           = _mm_unpacklo_epi8(src45, src56);
    return convolve_6tap_ssse3(ss_128, coeffs);
}

static INLINE __m256i y_convolve_6tap_8x2_avx2(const uint8_t* const src, const ptrdiff_t stride,
                                               const __m256i coeffs[3], __m128i s_64[6], __m256i ss_256[3]) {
    s_64[5]             = _mm_loadl_epi64((__m128i*)(src + 3 * stride));
    const __m256i src45 = _mm256_setr_m128i(s_64[4], s_64[5]);
    s_64[4]             = _mm_loadl_epi64((__m128i*)(src + 4 * stride));
    const __m256i src56 = _mm256_setr_m128i(s_64[5], s_64[4]);
    ss_256[2]           = _mm256_unpacklo_epi8(src45, src56);
    return convolve_6tap_avx2(ss_256, coeffs);
}

static INLINE void y_convolve_6tap_16x2_avx2(const uint8_t* const src, const ptrdiff_t stride, const __m256i coeffs[3],
                                             __m128i s_128[6], __m256i ss_256[6], __m256i r[2]) {
    s_128[5]            = _mm_loadu_si128((__m128i*)(src + 3 * stride));
    const __m256i src45 = _mm256_setr_m128i(s_128[4], s_128[5]);
    s_128[4]            = _mm_loadu_si128((__m128i*)(src + 4 * stride));
    const __m256i src56 = _mm256_setr_m128i(s_128[5], s_128[4]);
    ss_256[2]           = _mm256_unpacklo_epi8(src45, src56);
    ss_256[5]           = _mm256_unpackhi_epi8(src45, src56);
    r[0]                = convolve_6tap_avx2(ss_256, coeffs);
    r[1]                = convolve_6tap_avx2(ss_256 + 3, coeffs);
}

static INLINE void y_convolve_6tap_32x2_avx2(const uint8_t* const src, const ptrdiff_t stride, const __m256i coeffs[3],
                                             __m256i s_256[6], __m256i ss_256[6], __m256i tt_256[6], __m256i r[4]) {
    s_256[5]  = _mm256_loadu_si256((__m256i*)(src + 3 * stride));
    ss_256[2] = _mm256_unpacklo_epi8(s_256[4], s_256[5]);
    ss_256[5] = _mm256_unpackhi_epi8(s_256[4], s_256[5]);
    s_256[4]  = _mm256_loadu_si256((__m256i*)(src + 4 * stride));
    tt_256[2] = _mm256_unpacklo_epi8(s_256[5], s_256[4]);
    tt_256[5] = _mm256_unpackhi_epi8(s_256[5], s_256[4]);
    r[0]      = convolve_6tap_avx2(ss_256 + 0, coeffs);
    r[1]      = convolve_6tap_avx2(ss_256 + 3, coeffs);
    r[2]      = convolve_6tap_avx2(tt_256 + 0, coeffs);
    r[3]      = convolve_6tap_avx2(tt_256 + 3, coeffs);
}

static INLINE __m128i y_convolve_8tap_2x2_ssse3(const uint8_t* const src, const ptrdiff_t stride,
                                                const __m128i coeffs[4], __m128i s_16[8], __m128i ss_128[4]) {
    s_16[7]             = _mm_cvtsi32_si128(*(int16_t*)(src + 7 * stride));
    const __m128i src67 = _mm_unpacklo_epi16(s_16[6], s_16[7]);
    s_16[6]             = _mm_cvtsi32_si128(*(int16_t*)(src + 8 * stride));
    const __m128i src78 = _mm_unpacklo_epi16(s_16[7], s_16[6]);
    ss_128[3]           = _mm_unpacklo_epi8(src67, src78);
    return convolve_8tap_ssse3(ss_128, coeffs);
}

static INLINE __m128i y_convolve_8tap_4x2_ssse3(const uint8_t* const src, const ptrdiff_t stride,
                                                const __m128i coeffs[4], __m128i s_32[8], __m128i ss_128[4]) {
    s_32[7]             = _mm_cvtsi32_si128(*(int32_t*)(src + 7 * stride));
    const __m128i src67 = _mm_unpacklo_epi32(s_32[6], s_32[7]);
    s_32[6]             = _mm_cvtsi32_si128(*(int32_t*)(src + 8 * stride));
    const __m128i src78 = _mm_unpacklo_epi32(s_32[7], s_32[6]);
    ss_128[3]           = _mm_unpacklo_epi8(src67, src78);
    return convolve_8tap_ssse3(ss_128, coeffs);
}

static INLINE __m256i y_convolve_8tap_8x2_avx2(const uint8_t* const src, const ptrdiff_t stride,
                                               const __m256i coeffs[4], __m128i s_64[8], __m256i ss_256[4]) {
    s_64[7]             = _mm_loadl_epi64((__m128i*)(src + 7 * stride));
    const __m256i src67 = _mm256_setr_m128i(s_64[6], s_64[7]);
    s_64[6]             = _mm_loadl_epi64((__m128i*)(src + 8 * stride));
    const __m256i src78 = _mm256_setr_m128i(s_64[7], s_64[6]);
    ss_256[3]           = _mm256_unpacklo_epi8(src67, src78);
    return convolve_8tap_avx2(ss_256, coeffs);
}

static INLINE void y_convolve_8tap_16x2_avx2(const uint8_t* const src, const ptrdiff_t stride, const __m256i coeffs[4],
                                             __m128i s_128[8], __m256i ss_256[8], __m256i r[2]) {
    s_128[7]            = _mm_loadu_si128((__m128i*)(src + 7 * stride));
    const __m256i src67 = _mm256_setr_m128i(s_128[6], s_128[7]);
    s_128[6]            = _mm_loadu_si128((__m128i*)(src + 8 * stride));
    const __m256i src78 = _mm256_setr_m128i(s_128[7], s_128[6]);
    ss_256[3]           = _mm256_unpacklo_epi8(src67, src78);
    ss_256[7]           = _mm256_unpackhi_epi8(src67, src78);
    r[0]                = convolve_8tap_avx2(ss_256, coeffs);
    r[1]                = convolve_8tap_avx2(ss_256 + 4, coeffs);
}

static INLINE void y_convolve_8tap_32x2_avx2(const uint8_t* const src, const ptrdiff_t stride, const __m256i coeffs[4],
                                             __m256i s_256[8], __m256i ss_256[8], __m256i tt_256[8], __m256i r[4]) {
    s_256[7]  = _mm256_loadu_si256((__m256i*)(src + 7 * stride));
    ss_256[3] = _mm256_unpacklo_epi8(s_256[6], s_256[7]);
    ss_256[7] = _mm256_unpackhi_epi8(s_256[6], s_256[7]);
    s_256[6]  = _mm256_loadu_si256((__m256i*)(src + 8 * stride));
    tt_256[3] = _mm256_unpacklo_epi8(s_256[7], s_256[6]);
    tt_256[7] = _mm256_unpackhi_epi8(s_256[7], s_256[6]);
    r[0]      = convolve_8tap_avx2(ss_256 + 0, coeffs);
    r[1]      = convolve_8tap_avx2(ss_256 + 4, coeffs);
    r[2]      = convolve_8tap_avx2(tt_256 + 0, coeffs);
    r[3]      = convolve_8tap_avx2(tt_256 + 4, coeffs);
}

static INLINE void xy_x_convolve_2tap_32_avx2(const uint8_t* const src, const __m256i coeffs[1], __m256i r[2]) {
    const __m256i s0  = _mm256_loadu_si256((__m256i*)src);
    const __m256i s1  = _mm256_loadu_si256((__m256i*)(src + 1));
    const __m256i ss0 = _mm256_unpacklo_epi8(s0, s1);
    const __m256i ss1 = _mm256_unpackhi_epi8(s0, s1);

    r[0] = convolve_2tap_avx2(&ss0, coeffs);
    r[1] = convolve_2tap_avx2(&ss1, coeffs);
}

static INLINE void xy_x_2tap_32_avx2(const uint8_t* const src, const __m256i coeffs[1], int16_t* const dst) {
    __m256i r[2];

    xy_x_convolve_2tap_32_avx2(src, coeffs, r);
    const __m256i d0 = xy_x_round_avx2(r[0]);
    const __m256i d1 = xy_x_round_avx2(r[1]);
    // d0 = yy_unpacklo_epi128(d0, d1);
    // d1 = yy_unpackhi_epi128(d0, d1);
    _mm256_storeu_si256((__m256i*)dst, d0);
    _mm256_storeu_si256((__m256i*)(dst + 16), d1);
}

static INLINE void xy_x_6tap_32_avx2(const uint8_t* const src, const __m256i coeffs[3], const __m256i filt[3],
                                     int16_t* const dst) {
    __m256i r[2];

    x_convolve_6tap_32_avx2(src, coeffs, filt, r);
    const __m256i d0 = xy_x_round_avx2(r[0]);
    const __m256i d1 = xy_x_round_avx2(r[1]);
    _mm256_storeu_si256((__m256i*)dst, d0);
    _mm256_storeu_si256((__m256i*)(dst + 16), d1);
}

static INLINE void xy_x_8tap_32_avx2(const uint8_t* const src, const __m256i coeffs[4], const __m256i filt[4],
                                     int16_t* const dst) {
    __m256i r[2];

    x_convolve_8tap_32_avx2(src, coeffs, filt, r);
    const __m256i d0 = xy_x_round_avx2(r[0]);
    const __m256i d1 = xy_x_round_avx2(r[1]);
    _mm256_storeu_si256((__m256i*)dst, d0);
    _mm256_storeu_si256((__m256i*)(dst + 16), d1);
}

static INLINE __m128i xy_y_convolve_2tap_2x2_sse2(const int16_t* const src, __m128i s_32[2], const __m128i coeffs[1]) {
    __m128i s_128[2];

    s_32[1]          = _mm_cvtsi32_si128(*(int32_t*)(src + 2));
    s_128[0]         = _mm_unpacklo_epi32(s_32[0], s_32[1]);
    s_32[0]          = _mm_cvtsi32_si128(*(int32_t*)(src + 2 * 2));
    s_128[1]         = _mm_unpacklo_epi32(s_32[1], s_32[0]);
    const __m128i ss = _mm_unpacklo_epi16(s_128[0], s_128[1]);
    return convolve16_2tap_sse2(&ss, coeffs);
}

static INLINE __m128i xy_y_convolve_2tap_2x2_half_pel_sse2(const int16_t* const src, __m128i s_32[2]) {
    __m128i s_128[2];

    s_32[1]  = _mm_cvtsi32_si128(*(int32_t*)(src + 2));
    s_128[0] = _mm_unpacklo_epi32(s_32[0], s_32[1]);
    s_32[0]  = _mm_cvtsi32_si128(*(int32_t*)(src + 2 * 2));
    s_128[1] = _mm_unpacklo_epi32(s_32[1], s_32[0]);
    return _mm_add_epi16(s_128[0], s_128[1]);
}

static INLINE void xy_y_convolve_2tap_4x2_sse2(const int16_t* const src, __m128i s_64[2], const __m128i coeffs[1],
                                               __m128i r[2]) {
    __m128i s_128[2];

    s_64[1]           = _mm_loadl_epi64((__m128i*)(src + 4));
    s_128[0]          = _mm_unpacklo_epi64(s_64[0], s_64[1]);
    s_64[0]           = _mm_loadl_epi64((__m128i*)(src + 2 * 4));
    s_128[1]          = _mm_unpacklo_epi64(s_64[1], s_64[0]);
    const __m128i ss0 = _mm_unpacklo_epi16(s_128[0], s_128[1]);
    const __m128i ss1 = _mm_unpackhi_epi16(s_128[0], s_128[1]);
    r[0]              = convolve16_2tap_sse2(&ss0, coeffs);
    r[1]              = convolve16_2tap_sse2(&ss1, coeffs);
}

static INLINE __m128i xy_y_convolve_2tap_4x2_half_pel_sse2(const int16_t* const src, __m128i s_64[2]) {
    __m128i s_128[2];

    s_64[1]  = _mm_loadl_epi64((__m128i*)(src + 4));
    s_128[0] = _mm_unpacklo_epi64(s_64[0], s_64[1]);
    s_64[0]  = _mm_loadl_epi64((__m128i*)(src + 2 * 4));
    s_128[1] = _mm_unpacklo_epi64(s_64[1], s_64[0]);
    return _mm_add_epi16(s_128[0], s_128[1]);
}

static INLINE void xy_y_convolve_2tap_16_avx2(const __m256i s0, const __m256i s1, const __m256i coeffs[1],
                                              __m256i r[2]) {
    const __m256i ss0 = _mm256_unpacklo_epi16(s0, s1);
    const __m256i ss1 = _mm256_unpackhi_epi16(s0, s1);
    r[0]              = convolve16_2tap_avx2(&ss0, coeffs);
    r[1]              = convolve16_2tap_avx2(&ss1, coeffs);
}

static INLINE void xy_y_convolve_2tap_8x2_avx2(const int16_t* const src, __m128i s_128[2], const __m256i coeffs[1],
                                               __m256i r[2]) {
    __m256i s_256[2];
    s_128[1] = _mm_loadu_si128((__m128i*)(src + 8));
    s_256[0] = _mm256_setr_m128i(s_128[0], s_128[1]);
    s_128[0] = _mm_loadu_si128((__m128i*)(src + 2 * 8));
    s_256[1] = _mm256_setr_m128i(s_128[1], s_128[0]);
    xy_y_convolve_2tap_16_avx2(s_256[0], s_256[1], coeffs, r);
}

static INLINE __m256i xy_y_convolve_2tap_8x2_half_pel_avx2(const int16_t* const src, __m128i s_128[2]) {
    __m256i s_256[2];
    s_128[1] = _mm_loadu_si128((__m128i*)(src + 8));
    s_256[0] = _mm256_setr_m128i(s_128[0], s_128[1]);
    s_128[0] = _mm_loadu_si128((__m128i*)(src + 2 * 8));
    s_256[1] = _mm256_setr_m128i(s_128[1], s_128[0]);
    return _mm256_add_epi16(s_256[0], s_256[1]);
}

static INLINE void xy_y_convolve_2tap_16x2_half_pel_avx2(const int16_t* const src, __m256i s_256[2], __m256i r[2]) {
    s_256[1] = _mm256_loadu_si256((__m256i*)(src + 16));
    r[0]     = _mm256_add_epi16(s_256[0], s_256[1]);
    s_256[0] = _mm256_loadu_si256((__m256i*)(src + 2 * 16));
    r[1]     = _mm256_add_epi16(s_256[1], s_256[0]);
}

static INLINE void xy_y_store_16x2_avx2(const __m256i r[2], uint8_t* const dst, const ptrdiff_t stride) {
    const __m256i t = _mm256_packus_epi16(r[0], r[1]);
    const __m256i d = _mm256_permute4x64_epi64(t, 0xD8);
    storeu_u8_16x2_avx2(d, dst, stride);
}

static INLINE void xy_y_convolve_2tap_16x2_avx2(const int16_t* const src, __m256i s[2], const __m256i coeffs[1],
                                                __m256i r[4]) {
    s[1] = _mm256_loadu_si256((__m256i*)(src + 16));
    xy_y_convolve_2tap_16_avx2(s[0], s[1], coeffs, r + 0);
    s[0] = _mm256_loadu_si256((__m256i*)(src + 2 * 16));
    xy_y_convolve_2tap_16_avx2(s[1], s[0], coeffs, r + 2);
}

static INLINE void xy_y_convolve_2tap_32_avx2(const int16_t* const src, const __m256i s0[2], __m256i s1[2],
                                              const __m256i coeffs[1], __m256i r[4]) {
    s1[0] = _mm256_loadu_si256((__m256i*)src);
    s1[1] = _mm256_loadu_si256((__m256i*)(src + 16));
    xy_y_convolve_2tap_16_avx2(s0[0], s1[0], coeffs, r + 0);
    xy_y_convolve_2tap_16_avx2(s0[1], s1[1], coeffs, r + 2);
}

static INLINE void xy_y_convolve_2tap_32_all_avx2(const int16_t* const src, const __m256i s0[2], __m256i s1[2],
                                                  const __m256i coeffs[1], uint8_t* const dst) {
    __m256i r[4];

    xy_y_convolve_2tap_32_avx2(src, s0, s1, coeffs, r);
    xy_y_round_store_32_avx2(r + 0, r + 2, dst);
}

static INLINE void xy_y_convolve_2tap_half_pel_32_avx2(const int16_t* const src, const __m256i s0[2], __m256i s1[2],
                                                       __m256i r[2]) {
    s1[0] = _mm256_loadu_si256((__m256i*)src);
    s1[1] = _mm256_loadu_si256((__m256i*)(src + 16));
    r[0]  = _mm256_add_epi16(s0[0], s1[0]);
    r[1]  = _mm256_add_epi16(s0[1], s1[1]);
}

static INLINE void xy_y_convolve_2tap_half_pel_32_all_avx2(const int16_t* const src, const __m256i s0[2], __m256i s1[2],
                                                           uint8_t* const dst) {
    __m256i r[2];

    xy_y_convolve_2tap_half_pel_32_avx2(src, s0, s1, r);
    r[0] = xy_y_round_half_pel_avx2(r[0]);
    r[1] = xy_y_round_half_pel_avx2(r[1]);
    xy_y_pack_store_32_avx2(r[0], r[1], dst);
}

static INLINE __m128i xy_y_convolve_4tap_2x2_sse2(const int16_t* const src, __m128i s_32[4], __m128i ss_128[2],
                                                  const __m128i coeffs[2]) {
    s_32[3]             = _mm_cvtsi32_si128(*(int32_t*)(src + 3 * 2));
    const __m128i src23 = _mm_unpacklo_epi32(s_32[2], s_32[3]);
    s_32[2]             = _mm_cvtsi32_si128(*(int32_t*)(src + 4 * 2));
    const __m128i src34 = _mm_unpacklo_epi32(s_32[3], s_32[2]);
    ss_128[1]           = _mm_unpacklo_epi16(src23, src34);
    const __m128i r     = convolve16_4tap_sse2(ss_128, coeffs);
    ss_128[0]           = ss_128[1];
    return r;
}

static INLINE __m256i xy_y_convolve_4tap_4x2_avx2(const int16_t* const src, __m128i s_64[4], __m256i ss_256[2],
                                                  const __m256i coeffs[2]) {
    __m256i s_256[2];
    s_64[3]         = _mm_loadl_epi64((__m128i*)(src + 3 * 4));
    s_256[0]        = _mm256_setr_m128i(s_64[2], s_64[3]);
    s_64[2]         = _mm_loadl_epi64((__m128i*)(src + 4 * 4));
    s_256[1]        = _mm256_setr_m128i(s_64[3], s_64[2]);
    ss_256[1]       = _mm256_unpacklo_epi16(s_256[0], s_256[1]);
    const __m256i r = convolve16_4tap_avx2(ss_256, coeffs);
    ss_256[0]       = ss_256[1];
    return r;
}

static INLINE void xy_y_convolve_4tap_16_avx2(const __m256i* const ss, const __m256i coeffs[2], __m256i r[2]) {
    r[0] = convolve16_4tap_avx2(ss, coeffs);
    r[1] = convolve16_4tap_avx2(ss + 2, coeffs);
}

static INLINE void xy_y_convolve_4tap_8x2_avx2(const int16_t* const src, __m256i ss_256[4], const __m256i coeffs[2],
                                               __m256i r[2]) {
    __m256i s_256[2];
    s_256[0]  = _mm256_loadu_si256((__m256i*)(src + 2 * 8));
    s_256[1]  = _mm256_loadu_si256((__m256i*)(src + 3 * 8));
    ss_256[1] = _mm256_unpacklo_epi16(s_256[0], s_256[1]);
    ss_256[3] = _mm256_unpackhi_epi16(s_256[0], s_256[1]);
    xy_y_convolve_4tap_16_avx2(ss_256, coeffs, r);
    ss_256[0] = ss_256[1];
    ss_256[2] = ss_256[3];
}

static INLINE void xy_y_convolve_4tap_8x2_half_pel_avx2(const int16_t* const src, const __m256i coeffs[1],
                                                        __m256i s_256[4], __m256i r[2]) {
    __m256i a_256[2];
    s_256[2] = _mm256_loadu_si256((__m256i*)(src + 2 * 8));
    s_256[3] = _mm256_loadu_si256((__m256i*)(src + 3 * 8));
    a_256[0] = _mm256_add_epi16(s_256[0], s_256[3]);
    a_256[1] = _mm256_add_epi16(s_256[1], s_256[2]);
    xy_y_convolve_2tap_16_avx2(a_256[0], a_256[1], coeffs, r);
    s_256[0] = s_256[2];
    s_256[1] = s_256[3];
}

static INLINE void xy_y_convolve_4tap_16x2_avx2(const int16_t* const src, __m256i s_256[4], __m256i ss_256[4],
                                                __m256i tt_256[4], const __m256i coeffs[2], __m256i r[4]) {
    s_256[3]  = _mm256_loadu_si256((__m256i*)(src + 3 * 16));
    ss_256[1] = _mm256_unpacklo_epi16(s_256[2], s_256[3]);
    ss_256[3] = _mm256_unpackhi_epi16(s_256[2], s_256[3]);
    s_256[2]  = _mm256_loadu_si256((__m256i*)(src + 4 * 16));
    tt_256[1] = _mm256_unpacklo_epi16(s_256[3], s_256[2]);
    tt_256[3] = _mm256_unpackhi_epi16(s_256[3], s_256[2]);
    xy_y_convolve_4tap_16_avx2(ss_256, coeffs, r + 0);
    xy_y_convolve_4tap_16_avx2(tt_256, coeffs, r + 2);
    ss_256[0] = ss_256[1];
    ss_256[2] = ss_256[3];
    tt_256[0] = tt_256[1];
    tt_256[2] = tt_256[3];
}

static INLINE void xy_y_convolve_4tap_32x2_avx2(const int16_t* const src, const ptrdiff_t stride, __m256i s_256[4],
                                                __m256i ss_256[4], __m256i tt_256[4], const __m256i coeffs[2],
                                                __m256i r[4]) {
    s_256[3]  = _mm256_loadu_si256((__m256i*)(src + 3 * stride));
    ss_256[1] = _mm256_unpacklo_epi16(s_256[2], s_256[3]);
    ss_256[3] = _mm256_unpackhi_epi16(s_256[2], s_256[3]);
    s_256[2]  = _mm256_loadu_si256((__m256i*)(src + 4 * stride));
    tt_256[1] = _mm256_unpacklo_epi16(s_256[3], s_256[2]);
    tt_256[3] = _mm256_unpackhi_epi16(s_256[3], s_256[2]);
    xy_y_convolve_4tap_16_avx2(ss_256, coeffs, r + 0);
    xy_y_convolve_4tap_16_avx2(tt_256, coeffs, r + 2);
    ss_256[0] = ss_256[1];
    ss_256[2] = ss_256[3];
    tt_256[0] = tt_256[1];
    tt_256[2] = tt_256[3];
}

static INLINE void xy_y_convolve_4tap_16x2_half_pelavx2(const int16_t* const src, __m256i s_256[5],
                                                        const __m256i coeffs[1], __m256i r[4]) {
    __m256i a_256[2];

    s_256[3] = _mm256_loadu_si256((__m256i*)(src + 3 * 16));
    s_256[4] = _mm256_loadu_si256((__m256i*)(src + 4 * 16));

    a_256[0] = _mm256_add_epi16(s_256[0], s_256[3]);
    a_256[1] = _mm256_add_epi16(s_256[1], s_256[2]);
    xy_y_convolve_2tap_16_avx2(a_256[0], a_256[1], coeffs, r + 0);

    a_256[0] = _mm256_add_epi16(s_256[1], s_256[4]);
    a_256[1] = _mm256_add_epi16(s_256[2], s_256[3]);
    xy_y_convolve_2tap_16_avx2(a_256[0], a_256[1], coeffs, r + 2);

    s_256[0] = s_256[2];
    s_256[1] = s_256[3];
    s_256[2] = s_256[4];
}

static INLINE __m128i xy_y_convolve_6tap_2x2_sse2(const int16_t* const src, __m128i s_32[6], __m128i ss_128[3],
                                                  const __m128i coeffs[3]) {
    s_32[5]             = _mm_cvtsi32_si128(*(int32_t*)(src + 5 * 2));
    const __m128i src45 = _mm_unpacklo_epi32(s_32[4], s_32[5]);
    s_32[4]             = _mm_cvtsi32_si128(*(int32_t*)(src + 6 * 2));
    const __m128i src56 = _mm_unpacklo_epi32(s_32[5], s_32[4]);
    ss_128[2]           = _mm_unpacklo_epi16(src45, src56);
    const __m128i r     = convolve16_6tap_sse2(ss_128, coeffs);
    ss_128[0]           = ss_128[1];
    ss_128[1]           = ss_128[2];
    return r;
}

static INLINE __m256i xy_y_convolve_6tap_4x2_avx2(const int16_t* const src, __m128i s_64[6], __m256i ss_256[3],
                                                  const __m256i coeffs[3]) {
    __m256i s_256[2];
    s_64[5]         = _mm_loadl_epi64((__m128i*)(src + 5 * 4));
    s_256[0]        = _mm256_setr_m128i(s_64[4], s_64[5]);
    s_64[4]         = _mm_loadl_epi64((__m128i*)(src + 6 * 4));
    s_256[1]        = _mm256_setr_m128i(s_64[5], s_64[4]);
    ss_256[2]       = _mm256_unpacklo_epi16(s_256[0], s_256[1]);
    const __m256i r = convolve16_6tap_avx2(ss_256, coeffs);
    ss_256[0]       = ss_256[1];
    ss_256[1]       = ss_256[2];
    return r;
}

static INLINE void xy_y_convolve_6tap_16_avx2(const __m256i ss[6], const __m256i coeffs[3], __m256i r[2]) {
    r[0] = convolve16_6tap_avx2(ss, coeffs);
    r[1] = convolve16_6tap_avx2(ss + 3, coeffs);
}

static INLINE void xy_y_convolve_6tap_8x2_avx2(const int16_t* const src, __m256i ss_256[6], const __m256i coeffs[3],
                                               __m256i r[2]) {
    __m256i s_256[2];
    s_256[0]  = _mm256_loadu_si256((__m256i*)(src + 4 * 8));
    s_256[1]  = _mm256_loadu_si256((__m256i*)(src + 5 * 8));
    ss_256[2] = _mm256_unpacklo_epi16(s_256[0], s_256[1]);
    ss_256[5] = _mm256_unpackhi_epi16(s_256[0], s_256[1]);
    xy_y_convolve_6tap_16_avx2(ss_256, coeffs, r);
    ss_256[0] = ss_256[1];
    ss_256[1] = ss_256[2];
    ss_256[3] = ss_256[4];
    ss_256[4] = ss_256[5];
}

static INLINE void xy_y_convolve_6tap_8x2_half_pel_avx2(const int16_t* const src, const __m256i coeffs[2],
                                                        __m256i s_256[6], __m256i r[2]) {
    __m256i a_256[2], ss_256[4];
    s_256[4]  = _mm256_loadu_si256((__m256i*)(src + 4 * 8));
    s_256[5]  = _mm256_loadu_si256((__m256i*)(src + 5 * 8));
    a_256[0]  = _mm256_add_epi16(s_256[0], s_256[5]);
    a_256[1]  = _mm256_add_epi16(s_256[1], s_256[4]);
    ss_256[0] = _mm256_unpacklo_epi16(a_256[0], a_256[1]);
    ss_256[1] = _mm256_unpacklo_epi16(s_256[2], s_256[3]);
    ss_256[2] = _mm256_unpackhi_epi16(a_256[0], a_256[1]);
    ss_256[3] = _mm256_unpackhi_epi16(s_256[2], s_256[3]);
    xy_y_convolve_4tap_16_avx2(ss_256, coeffs, r);
    s_256[0] = s_256[2];
    s_256[1] = s_256[3];
    s_256[2] = s_256[4];
    s_256[3] = s_256[5];
}

static INLINE void xy_y_convolve_6tap_16x2_avx2(const int16_t* const src, const ptrdiff_t stride, __m256i s_256[6],
                                                __m256i ss_256[6], __m256i tt_256[6], const __m256i coeffs[3],
                                                __m256i r[4]) {
    s_256[5]  = _mm256_loadu_si256((__m256i*)(src + 5 * stride));
    ss_256[2] = _mm256_unpacklo_epi16(s_256[4], s_256[5]);
    ss_256[5] = _mm256_unpackhi_epi16(s_256[4], s_256[5]);
    s_256[4]  = _mm256_loadu_si256((__m256i*)(src + 6 * stride));
    tt_256[2] = _mm256_unpacklo_epi16(s_256[5], s_256[4]);
    tt_256[5] = _mm256_unpackhi_epi16(s_256[5], s_256[4]);

    xy_y_convolve_6tap_16_avx2(ss_256, coeffs, r + 0);
    xy_y_convolve_6tap_16_avx2(tt_256, coeffs, r + 2);

    ss_256[0] = ss_256[1];
    ss_256[1] = ss_256[2];
    ss_256[3] = ss_256[4];
    ss_256[4] = ss_256[5];

    tt_256[0] = tt_256[1];
    tt_256[1] = tt_256[2];
    tt_256[3] = tt_256[4];
    tt_256[4] = tt_256[5];
}

static INLINE void xy_y_convolve_6tap_16x2_half_pel_avx2(const int16_t* const src, const ptrdiff_t stride,
                                                         __m256i s_256[6], __m256i ss_256[4], const __m256i coeffs[2],
                                                         __m256i r[4]) {
    __m256i a_256[2];

    s_256[5]  = _mm256_loadu_si256((__m256i*)(src + 5 * stride));
    a_256[0]  = _mm256_add_epi16(s_256[0], s_256[5]);
    a_256[1]  = _mm256_add_epi16(s_256[1], s_256[4]);
    ss_256[0] = _mm256_unpacklo_epi16(a_256[0], a_256[1]);
    ss_256[1] = _mm256_unpacklo_epi16(s_256[2], s_256[3]);
    ss_256[2] = _mm256_unpackhi_epi16(a_256[0], a_256[1]);
    ss_256[3] = _mm256_unpackhi_epi16(s_256[2], s_256[3]);
    xy_y_convolve_4tap_16_avx2(ss_256, coeffs, r + 0);

    a_256[1]  = _mm256_add_epi16(s_256[2], s_256[5]);
    s_256[0]  = s_256[2];
    s_256[2]  = s_256[4];
    s_256[4]  = _mm256_loadu_si256((__m256i*)(src + 6 * stride));
    a_256[0]  = _mm256_add_epi16(s_256[1], s_256[4]);
    s_256[1]  = s_256[3];
    s_256[3]  = s_256[5];
    ss_256[0] = _mm256_unpacklo_epi16(a_256[0], a_256[1]);
    ss_256[1] = _mm256_unpacklo_epi16(s_256[1], s_256[2]);
    ss_256[2] = _mm256_unpackhi_epi16(a_256[0], a_256[1]);
    ss_256[3] = _mm256_unpackhi_epi16(s_256[1], s_256[2]);
    xy_y_convolve_4tap_16_avx2(ss_256, coeffs, r + 2);
}

static INLINE __m128i xy_y_convolve_8tap_2x2_sse2(const int16_t* const src, __m128i s_32[8], __m128i ss_128[4],
                                                  const __m128i coeffs[4]) {
    s_32[7]             = _mm_cvtsi32_si128(*(int32_t*)(src + 7 * 2));
    const __m128i src67 = _mm_unpacklo_epi32(s_32[6], s_32[7]);
    s_32[6]             = _mm_cvtsi32_si128(*(int32_t*)(src + 8 * 2));
    const __m128i src78 = _mm_unpacklo_epi32(s_32[7], s_32[6]);
    ss_128[3]           = _mm_unpacklo_epi16(src67, src78);
    const __m128i r     = convolve16_8tap_sse2(ss_128, coeffs);
    ss_128[0]           = ss_128[1];
    ss_128[1]           = ss_128[2];
    ss_128[2]           = ss_128[3];
    return r;
}

static INLINE __m256i xy_y_convolve_8tap_4x2_avx2(const int16_t* const src, __m128i s_64[8], __m256i ss_256[4],
                                                  const __m256i coeffs[4]) {
    __m256i s_256[2];
    s_64[7]         = _mm_loadl_epi64((__m128i*)(src + 7 * 4));
    s_256[0]        = _mm256_setr_m128i(s_64[6], s_64[7]);
    s_64[6]         = _mm_loadl_epi64((__m128i*)(src + 8 * 4));
    s_256[1]        = _mm256_setr_m128i(s_64[7], s_64[6]);
    ss_256[3]       = _mm256_unpacklo_epi16(s_256[0], s_256[1]);
    const __m256i r = convolve16_8tap_avx2(ss_256, coeffs);
    ss_256[0]       = ss_256[1];
    ss_256[1]       = ss_256[2];
    ss_256[2]       = ss_256[3];
    return r;
}

static INLINE void xy_y_convolve_8tap_16_avx2(const __m256i* const ss, const __m256i coeffs[4], __m256i r[2]) {
    r[0] = convolve16_8tap_avx2(ss, coeffs);
    r[1] = convolve16_8tap_avx2(ss + 4, coeffs);
}

static INLINE void xy_y_convolve_8tap_8x2_avx2(const int16_t* const src, __m256i ss_256[8], const __m256i coeffs[4],
                                               __m256i r[2]) {
    __m256i s_256[2];
    s_256[0]  = _mm256_loadu_si256((__m256i*)(src + 6 * 8));
    s_256[1]  = _mm256_loadu_si256((__m256i*)(src + 7 * 8));
    ss_256[3] = _mm256_unpacklo_epi16(s_256[0], s_256[1]);
    ss_256[7] = _mm256_unpackhi_epi16(s_256[0], s_256[1]);
    xy_y_convolve_8tap_16_avx2(ss_256, coeffs, r);
    ss_256[0] = ss_256[1];
    ss_256[1] = ss_256[2];
    ss_256[2] = ss_256[3];
    ss_256[4] = ss_256[5];
    ss_256[5] = ss_256[6];
    ss_256[6] = ss_256[7];
}

static INLINE void xy_y_convolve_8tap_8x2_half_pel_avx2(const int16_t* const src, const __m256i coeffs[2],
                                                        __m256i s_256[8], __m256i r[2]) {
    __m256i a_256[4], ss_256[4];

    s_256[6]  = _mm256_loadu_si256((__m256i*)(src + 6 * 8));
    s_256[7]  = _mm256_loadu_si256((__m256i*)(src + 7 * 8));
    a_256[0]  = _mm256_add_epi16(s_256[0], s_256[7]);
    a_256[1]  = _mm256_add_epi16(s_256[1], s_256[6]);
    a_256[2]  = _mm256_add_epi16(s_256[2], s_256[5]);
    a_256[3]  = _mm256_add_epi16(s_256[3], s_256[4]);
    ss_256[0] = _mm256_unpacklo_epi16(a_256[0], a_256[1]);
    ss_256[1] = _mm256_unpacklo_epi16(a_256[2], a_256[3]);
    ss_256[2] = _mm256_unpackhi_epi16(a_256[0], a_256[1]);
    ss_256[3] = _mm256_unpackhi_epi16(a_256[2], a_256[3]);
    xy_y_convolve_4tap_16_avx2(ss_256, coeffs, r);
    s_256[0] = s_256[2];
    s_256[1] = s_256[3];
    s_256[2] = s_256[4];
    s_256[3] = s_256[5];
    s_256[4] = s_256[6];
    s_256[5] = s_256[7];
}

SIMD_INLINE void xy_y_convolve_8tap_16x2_avx2(const int16_t* const src, const ptrdiff_t stride, const __m256i coeffs[4],
                                              __m256i s_256[8], __m256i ss_256[8], __m256i tt_256[8], __m256i r[4]) {
    s_256[7]  = _mm256_loadu_si256((__m256i*)(src + 7 * stride));
    ss_256[3] = _mm256_unpacklo_epi16(s_256[6], s_256[7]);
    ss_256[7] = _mm256_unpackhi_epi16(s_256[6], s_256[7]);
    s_256[6]  = _mm256_loadu_si256((__m256i*)(src + 8 * stride));
    tt_256[3] = _mm256_unpacklo_epi16(s_256[7], s_256[6]);
    tt_256[7] = _mm256_unpackhi_epi16(s_256[7], s_256[6]);

    xy_y_convolve_8tap_16_avx2(ss_256, coeffs, r + 0);
    xy_y_convolve_8tap_16_avx2(tt_256, coeffs, r + 2);

    ss_256[0] = ss_256[1];
    ss_256[1] = ss_256[2];
    ss_256[2] = ss_256[3];
    ss_256[4] = ss_256[5];
    ss_256[5] = ss_256[6];
    ss_256[6] = ss_256[7];

    tt_256[0] = tt_256[1];
    tt_256[1] = tt_256[2];
    tt_256[2] = tt_256[3];
    tt_256[4] = tt_256[5];
    tt_256[5] = tt_256[6];
    tt_256[6] = tt_256[7];
}

static INLINE void xy_y_convolve_8tap_16x2_half_pel_avx2(const int16_t* const src, const ptrdiff_t stride,
                                                         const __m256i coeffs[4], __m256i s_256[8], __m256i r[4]) {
    __m256i a_256[4], ss_256[4];
    s_256[7] = _mm256_loadu_si256((__m256i*)(src + 7 * stride));

    a_256[0]  = _mm256_add_epi16(s_256[0], s_256[7]);
    a_256[1]  = _mm256_add_epi16(s_256[1], s_256[6]);
    a_256[2]  = _mm256_add_epi16(s_256[2], s_256[5]);
    a_256[3]  = _mm256_add_epi16(s_256[3], s_256[4]);
    ss_256[0] = _mm256_unpacklo_epi16(a_256[0], a_256[1]);
    ss_256[1] = _mm256_unpacklo_epi16(a_256[2], a_256[3]);
    ss_256[2] = _mm256_unpackhi_epi16(a_256[0], a_256[1]);
    ss_256[3] = _mm256_unpackhi_epi16(a_256[2], a_256[3]);

    xy_y_convolve_4tap_16_avx2(ss_256, coeffs, r + 0);

    a_256[1] = _mm256_add_epi16(s_256[2], s_256[7]);
    a_256[2] = _mm256_add_epi16(s_256[3], s_256[6]);
    a_256[3] = _mm256_add_epi16(s_256[4], s_256[5]);
    s_256[0] = s_256[2];
    s_256[2] = s_256[4];
    s_256[4] = s_256[6];
    s_256[6] = _mm256_loadu_si256((__m256i*)(src + 8 * stride));

    a_256[0]  = _mm256_add_epi16(s_256[1], s_256[6]);
    s_256[1]  = s_256[3];
    s_256[3]  = s_256[5];
    s_256[5]  = s_256[7];
    ss_256[0] = _mm256_unpacklo_epi16(a_256[0], a_256[1]);
    ss_256[1] = _mm256_unpacklo_epi16(a_256[2], a_256[3]);
    ss_256[2] = _mm256_unpackhi_epi16(a_256[0], a_256[1]);
    ss_256[3] = _mm256_unpackhi_epi16(a_256[2], a_256[3]);

    xy_y_convolve_4tap_16_avx2(ss_256, coeffs, r + 2);
}

static INLINE void jnt_comp_avg_round_store_2x2_kernel_sse2(const __m128i res, const __m128i factor,
                                                            const __m128i offset, const ConvBufType* const dst,
                                                            const ptrdiff_t dst_stride, uint8_t* const dst8,
                                                            const int32_t dst8_stride) {
    __m128i d;

    d = load_u16_2x2_sse4_1(dst, dst_stride);
    d = _mm_unpacklo_epi16(d, res);
    d = _mm_madd_epi16(d, factor);
    d = _mm_add_epi32(d, offset);
    d = _mm_srai_epi32(d, 8);
    d = _mm_packs_epi32(d, d);
    pack_store_2x2_sse2(d, dst8, dst8_stride);
}

static INLINE void jnt_comp_avg_round_store_2x2_sse2(const __m128i res, const __m128i factor, const __m128i offset,
                                                     const ConvBufType* const dst, const ptrdiff_t dst_stride,
                                                     uint8_t* const dst8, const int32_t dst8_stride) {
    const __m128i r = jnt_y_round_sse2(res);

    jnt_comp_avg_round_store_2x2_kernel_sse2(r, factor, offset, dst, dst_stride, dst8, dst8_stride);
}

static INLINE void jnt_comp_avg_round_store_4x2_kernel_sse2(const __m128i res, const __m128i factor,
                                                            const __m128i offset, const ConvBufType* const dst,
                                                            const ptrdiff_t dst_stride, uint8_t* const dst8,
                                                            const int32_t dst8_stride) {
    const __m128i dst_128 = load_u16_4x2_sse2(dst, dst_stride);
    __m128i       d[2];

    d[0] = _mm_unpacklo_epi16(dst_128, res);
    d[1] = _mm_unpackhi_epi16(dst_128, res);
    d[0] = _mm_madd_epi16(d[0], factor);
    d[1] = _mm_madd_epi16(d[1], factor);
    d[0] = _mm_add_epi32(d[0], offset);
    d[1] = _mm_add_epi32(d[1], offset);
    d[0] = _mm_srai_epi32(d[0], 8);
    d[1] = _mm_srai_epi32(d[1], 8);
    d[0] = _mm_packs_epi32(d[0], d[1]);
    pack_store_4x2_sse2(d[0], dst8, dst8_stride);
}

static INLINE void jnt_comp_avg_round_store_4x2_sse2(const __m128i res, const __m128i factor, const __m128i offset,
                                                     const ConvBufType* const dst, const ptrdiff_t dst_stride,
                                                     uint8_t* const dst8, const int32_t dst8_stride) {
    const __m128i r = jnt_y_round_sse2(res);
    jnt_comp_avg_round_store_4x2_kernel_sse2(r, factor, offset, dst, dst_stride, dst8, dst8_stride);
}

static INLINE __m256i jnt_comp_avg_convolve_16_avx2(const __m256i res, const __m256i dst, const __m256i factor,
                                                    const __m256i offset) {
    __m256i d[2];

    d[0] = _mm256_unpacklo_epi16(dst, res);
    d[1] = _mm256_unpackhi_epi16(dst, res);
    d[0] = _mm256_madd_epi16(d[0], factor);
    d[1] = _mm256_madd_epi16(d[1], factor);
    d[0] = _mm256_add_epi32(d[0], offset);
    d[1] = _mm256_add_epi32(d[1], offset);
    d[0] = _mm256_srai_epi32(d[0], 8);
    d[1] = _mm256_srai_epi32(d[1], 8);
    return _mm256_packs_epi32(d[0], d[1]);
}

static INLINE void jnt_comp_avg_round_store_8x2_kernel_avx2(const __m256i res, const __m256i factor,
                                                            const __m256i offset, const ConvBufType* const dst,
                                                            const ptrdiff_t dst_stride, uint8_t* const dst8,
                                                            const int32_t dst8_stride) {
    __m256i d;

    d = loadu_u16_8x2_avx2(dst, dst_stride);
    d = jnt_comp_avg_convolve_16_avx2(res, d, factor, offset);
    pack_store_8x2_avx2(d, dst8, dst8_stride);
}

static INLINE void jnt_comp_avg_round_store_16x2_kernel_avx2(const __m256i res[2], const __m256i factor,
                                                             const __m256i offset, const ConvBufType* const dst,
                                                             const ptrdiff_t dst_stride, uint8_t* const dst8,
                                                             const int32_t dst8_stride) {
    __m256i d[2];

    d[0] = _mm256_loadu_si256((__m256i*)dst);
    d[1] = _mm256_loadu_si256((__m256i*)(dst + dst_stride));
    d[0] = jnt_comp_avg_convolve_16_avx2(res[0], d[0], factor, offset);
    d[1] = jnt_comp_avg_convolve_16_avx2(res[1], d[1], factor, offset);
    xy_y_pack_store_16x2_avx2(d[0], d[1], dst8, dst8_stride);
}

static INLINE void jnt_comp_avg_round_store_32_kernel_avx2(const __m256i res[2], const __m256i factor,
                                                           const __m256i offset, const ConvBufType* const dst,
                                                           uint8_t* const dst8) {
    __m256i d[2];

    d[0] = _mm256_loadu_si256((__m256i*)(dst + 0 * 16));
    d[1] = _mm256_loadu_si256((__m256i*)(dst + 1 * 16));
    d[0] = jnt_comp_avg_convolve_16_avx2(res[0], d[0], factor, offset);
    d[1] = jnt_comp_avg_convolve_16_avx2(res[1], d[1], factor, offset);
    pack_store_32_avx2(d[0], d[1], dst8);
}

static INLINE void jnt_comp_avg_round_store_8x2_avx2(const __m256i res, const __m256i factor, const __m256i offset,
                                                     const ConvBufType* const dst, const ptrdiff_t dst_stride,
                                                     uint8_t* const dst8, const int32_t dst8_stride) {
    const __m256i r = jnt_y_round_avx2(res);
    jnt_comp_avg_round_store_8x2_kernel_avx2(r, factor, offset, dst, dst_stride, dst8, dst8_stride);
}

SIMD_INLINE void jnt_comp_avg_round_store_16x2_avx2(const __m256i res[2], const __m256i factor, const __m256i offset,
                                                    const ConvBufType* const dst, const ptrdiff_t dst_stride,
                                                    uint8_t* const dst8, const int32_t dst8_stride) {
    __m256i r[2], d[2];

    r[0] = jnt_y_round_avx2(res[0]);
    r[1] = jnt_y_round_avx2(res[1]);
    d[0] = loadu_u16_8x2_avx2(dst, dst_stride);
    d[1] = loadu_u16_8x2_avx2(dst + 8, dst_stride);
    d[0] = jnt_comp_avg_convolve_16_avx2(r[0], d[0], factor, offset);
    d[1] = jnt_comp_avg_convolve_16_avx2(r[1], d[1], factor, offset);
    pack_store_16x2_avx2(d[0], d[1], dst8, dst8_stride);
}

SIMD_INLINE void jnt_comp_avg_round_store_32_avx2(const __m256i res[2], const __m256i factor, const __m256i offset,
                                                  const ConvBufType* const dst, uint8_t* const dst8) {
    __m256i r[2], d[2];

    r[0] = jnt_y_round_avx2(res[0]);
    r[1] = jnt_y_round_avx2(res[1]);
    d[0] = loadu_u16_8x2_avx2(dst, 16);
    d[1] = loadu_u16_8x2_avx2(dst + 8, 16);
    d[0] = jnt_comp_avg_convolve_16_avx2(r[0], d[0], factor, offset);
    d[1] = jnt_comp_avg_convolve_16_avx2(r[1], d[1], factor, offset);
    convolve_store_32_avx2(d[0], d[1], dst8);
}

static INLINE __m128i jnt_avg_8_sse2(const __m128i res, const __m128i dst) {
    const __m128i d = _mm_add_epi16(res, dst);
    return _mm_srai_epi16(d, 5);
}

static INLINE void jnt_copy_avg_round_store_2x2_sse2(const __m128i res, const __m128i offset,
                                                     const ConvBufType* const dst, const ptrdiff_t dst_stride,
                                                     uint8_t* const dst8, const int32_t dst8_stride) {
    const __m128i r = _mm_add_epi16(res, offset);
    __m128i       d;

    d = load_u16_2x2_sse4_1(dst, dst_stride);
    d = jnt_avg_8_sse2(r, d);
    pack_store_2x2_sse2(d, dst8, dst8_stride);
}

static INLINE void jnt_avg_round_store_2x2_sse2(const __m128i res, const __m128i offset, const ConvBufType* const dst,
                                                const ptrdiff_t dst_stride, uint8_t* const dst8,
                                                const int32_t dst8_stride) {
    const __m128i r = jnt_avg_round_sse2(res, offset);
    __m128i       d;

    d = load_u16_2x2_sse4_1(dst, dst_stride);
    d = jnt_avg_8_sse2(r, d);
    pack_store_2x2_sse2(d, dst8, dst8_stride);
}

static INLINE void jnt_copy_avg_round_store_4x2_sse2(const __m128i res, const __m128i offset,
                                                     const ConvBufType* const dst, const ptrdiff_t dst_stride,
                                                     uint8_t* const dst8, const int32_t dst8_stride) {
    const __m128i r = _mm_add_epi16(res, offset);
    __m128i       d;

    d = load_u16_4x2_sse2(dst, dst_stride);
    d = jnt_avg_8_sse2(r, d);
    pack_store_4x2_sse2(d, dst8, dst8_stride);
}

static INLINE void jnt_avg_round_store_4x2_sse2(const __m128i res, const __m128i offset, const ConvBufType* const dst,
                                                const ptrdiff_t dst_stride, uint8_t* const dst8,
                                                const int32_t dst8_stride) {
    const __m128i r = jnt_avg_round_sse2(res, offset);
    __m128i       d;

    d = load_u16_4x2_sse2(dst, dst_stride);
    d = jnt_avg_8_sse2(r, d);
    pack_store_4x2_sse2(d, dst8, dst8_stride);
}

static INLINE __m256i jnt_avg_16_avx2(const __m256i res, const __m256i dst) {
    const __m256i d = _mm256_add_epi16(res, dst);
    return _mm256_srai_epi16(d, 5);
}

static INLINE void jnt_copy_avg_round_store_8x2_avx2(const __m256i res, const __m256i offset,
                                                     const ConvBufType* const dst, const ptrdiff_t dst_stride,
                                                     uint8_t* const dst8, const int32_t dst8_stride) {
    const __m256i r = _mm256_add_epi16(res, offset);
    __m256i       d;

    d = loadu_u16_8x2_avx2(dst, dst_stride);
    d = jnt_avg_16_avx2(r, d);
    pack_store_8x2_avx2(d, dst8, dst8_stride);
}

static INLINE void jnt_copy_avg_round_store_16x2_avx2(const __m256i res[2], const __m256i offset,
                                                      const ConvBufType* const dst, const ptrdiff_t dst_stride,
                                                      uint8_t* const dst8, const int32_t dst8_stride) {
    __m256i r[2], d[2];

    r[0] = _mm256_add_epi16(res[0], offset);
    r[1] = _mm256_add_epi16(res[1], offset);
    d[0] = _mm256_loadu_si256((__m256i*)dst);
    d[1] = _mm256_loadu_si256((__m256i*)(dst + dst_stride));
    d[0] = jnt_avg_16_avx2(r[0], d[0]);
    d[1] = jnt_avg_16_avx2(r[1], d[1]);
    xy_y_pack_store_16x2_avx2(d[0], d[1], dst8, dst8_stride);
}

static INLINE void jnt_copy_avg_round_store_32_avx2(const __m256i res[2], const __m256i offset,
                                                    const ConvBufType* const dst, uint8_t* const dst8) {
    __m256i r[2], d[2];

    r[0] = _mm256_add_epi16(res[0], offset);
    r[1] = _mm256_add_epi16(res[1], offset);
    d[0] = _mm256_loadu_si256((__m256i*)(dst + 0 * 16));
    d[1] = _mm256_loadu_si256((__m256i*)(dst + 1 * 16));
    d[0] = jnt_avg_16_avx2(r[0], d[0]);
    d[1] = jnt_avg_16_avx2(r[1], d[1]);
    pack_store_32_avx2(d[0], d[1], dst8);
}

static INLINE __m128i jnt_copy_load_src_2x2_sse2(const uint8_t* const src, const int32_t src_stride) {
    const __m128i zero_128 = _mm_setzero_si128();
    const __m128i s        = load_u8_2x2_sse2(src, src_stride);
    const __m128i s16      = _mm_unpacklo_epi8(s, zero_128);
    return _mm_slli_epi16(s16, LEFT_SHIFT);
}

static INLINE __m128i jnt_copy_load_src_4x2_sse4_1(const uint8_t* const src, const int32_t src_stride) {
    const __m128i zero_128 = _mm_setzero_si128();
    const __m128i s        = load_u8_4x2_sse4_1(src, src_stride);
    const __m128i s16      = _mm_unpacklo_epi8(s, zero_128);
    return _mm_slli_epi16(s16, LEFT_SHIFT);
}

static INLINE __m256i jnt_copy_load_src_8x2_avx2(const uint8_t* const src, const int32_t src_stride) {
    const __m256i zero_256 = _mm256_setzero_si256();
    const __m256i s        = load_u8_8x2_avx2(src, src_stride);
    const __m256i s16      = _mm256_unpacklo_epi8(s, zero_256);
    return _mm256_slli_epi16(s16, LEFT_SHIFT);
}

static INLINE __m256i jnt_copy_load_src_16_avx2(const uint8_t* const src) {
    const __m128i s   = _mm_loadu_si128((__m128i*)src);
    const __m256i s16 = _mm256_cvtepu8_epi16(s);
    return _mm256_slli_epi16(s16, LEFT_SHIFT);
}

static INLINE void jnt_copy_load_src_32_avx2(const uint8_t* const src, __m256i s_256[2]) {
    const __m128i s8_lo  = _mm_loadu_si128((__m128i*)src);
    const __m128i s8_hi  = _mm_loadu_si128((__m128i*)(src + 16));
    const __m256i s16_lo = _mm256_cvtepu8_epi16(s8_lo);
    const __m256i s16_hi = _mm256_cvtepu8_epi16(s8_hi);
    s_256[0]             = _mm256_slli_epi16(s16_lo, LEFT_SHIFT);
    s_256[1]             = _mm256_slli_epi16(s16_hi, LEFT_SHIFT);
}

static INLINE void jnt_copy_comp_avg_32_avx2(const uint8_t* const src, const __m256i factor_256,
                                             const __m256i offset_comp_avg_256, const ConvBufType* const dst,
                                             uint8_t* const dst8) {
    __m256i res[2];
    jnt_copy_load_src_32_avx2(src, res);
    jnt_comp_avg_round_store_32_kernel_avx2(res, factor_256, offset_comp_avg_256, dst, dst8);
}

static INLINE void jnt_avg_round_store_8x2_avx2(const __m256i res, const __m256i offset, const ConvBufType* const dst,
                                                const ptrdiff_t dst_stride, uint8_t* const dst8,
                                                const int32_t dst8_stride) {
    const __m256i r = jnt_avg_round_avx2(res, offset);
    __m256i       d;

    d = loadu_u16_8x2_avx2(dst, dst_stride);
    d = jnt_avg_16_avx2(r, d);
    pack_store_8x2_avx2(d, dst8, dst8_stride);
}

static INLINE void jnt_avg_round_store_16x2_avx2(const __m256i res[2], const __m256i offset,
                                                 const ConvBufType* const dst, const ptrdiff_t dst_stride,
                                                 uint8_t* const dst8, const int32_t dst8_stride) {
    __m256i r[2], d[2];

    r[0] = jnt_avg_round_avx2(res[0], offset);
    r[1] = jnt_avg_round_avx2(res[1], offset);
    d[0] = loadu_u16_8x2_avx2(dst, dst_stride);
    d[1] = loadu_u16_8x2_avx2(dst + 8, dst_stride);
    d[0] = jnt_avg_16_avx2(r[0], d[0]);
    d[1] = jnt_avg_16_avx2(r[1], d[1]);
    pack_store_16x2_avx2(d[0], d[1], dst8, dst8_stride);
}

static INLINE void jnt_avg_round_store_32_avx2(const __m256i res[2], const __m256i offset, const ConvBufType* const dst,
                                               uint8_t* const dst8) {
    __m256i r[2], d[2];

    r[0] = jnt_avg_round_avx2(res[0], offset);
    r[1] = jnt_avg_round_avx2(res[1], offset);
    d[0] = loadu_u16_8x2_avx2(dst, 16);
    d[1] = loadu_u16_8x2_avx2(dst + 8, 16);
    d[0] = jnt_avg_16_avx2(r[0], d[0]);
    d[1] = jnt_avg_16_avx2(r[1], d[1]);
    convolve_store_32_avx2(d[0], d[1], dst8);
}

static INLINE void jnt_no_avg_round_store_2x2_sse2(const __m128i res, const __m128i offset, ConvBufType* const dst,
                                                   const ptrdiff_t dst_stride) {
    const __m128i d = jnt_no_avg_round_sse2(res, offset);
    store_u16_2x2_sse2(d, dst, dst_stride);
}

static INLINE void jnt_no_avg_round_store_4x2_sse2(const __m128i res, const __m128i offset, ConvBufType* const dst,
                                                   const ptrdiff_t dst_stride) {
    const __m128i d = jnt_no_avg_round_sse2(res, offset);
    store_u16_4x2_sse2(d, dst, dst_stride);
}

static INLINE void jnt_no_avg_round_store_8x2_avx2(const __m256i res, const __m256i offset, ConvBufType* const dst,
                                                   const ptrdiff_t dst_stride) {
    const __m256i d = jnt_no_avg_round_avx2(res, offset);
    storeu_u16_8x2_avx2(d, dst, dst_stride);
}

static INLINE void jnt_no_avg_round_store_16x2_avx2(const __m256i res[2], const __m256i offset, ConvBufType* const dst,
                                                    const ptrdiff_t dst_stride) {
    __m256i d[2];

    d[0] = jnt_no_avg_round_avx2(res[0], offset);
    d[1] = jnt_no_avg_round_avx2(res[1], offset);
    jnt_no_avg_store_16x2_avx2(d[0], d[1], dst, dst_stride);
}

static INLINE void jnt_no_avg_round_store_32_avx2(const __m256i res[2], const __m256i offset, ConvBufType* const dst) {
    __m256i d[2];

    d[0] = jnt_no_avg_round_avx2(res[0], offset);
    d[1] = jnt_no_avg_round_avx2(res[1], offset);
    jnt_no_avg_store_16x2_avx2(d[0], d[1], dst, 16);
}

static INLINE void xy_y_round_store_8x2_avx2(const __m256i res[2], uint8_t* const dst, const ptrdiff_t stride) {
    const __m256i r = xy_y_round_16_avx2(res);
    pack_store_8x2_avx2(r, dst, stride);
}

static INLINE void xy_y_round_store_16x2_avx2(const __m256i res[4], uint8_t* const dst, const ptrdiff_t stride) {
    const __m256i r0 = xy_y_round_16_avx2(res + 0);
    const __m256i r1 = xy_y_round_16_avx2(res + 2);
    xy_y_pack_store_16x2_avx2(r0, r1, dst, stride);
}

static INLINE __m128i jnt_2d_comp_avg_round_4_sse2(const __m128i src) {
    const __m128i round = _mm_set1_epi32(1 << (COMPOUND_ROUND1_BITS - 1));
    const __m128i dst   = _mm_add_epi32(src, round);
    const __m128i d     = _mm_srai_epi32(dst, COMPOUND_ROUND1_BITS);
    return _mm_packs_epi32(d, d);
}

static INLINE __m128i jnt_2d_comp_avg_round_half_pel_sse2(const __m128i src) {
    const __m128i round = _mm_set1_epi16(1);
    const __m128i dst   = _mm_add_epi16(src, round);
    return _mm_srai_epi16(dst, 1);
}

static INLINE __m128i jnt_2d_comp_avg_round_4x2_sse2(const __m128i src[2]) {
    const __m128i round = _mm_set1_epi32(1 << (COMPOUND_ROUND1_BITS - 1));
    const __m128i dst0  = _mm_add_epi32(src[0], round);
    const __m128i dst1  = _mm_add_epi32(src[1], round);
    const __m128i d0    = _mm_srai_epi32(dst0, COMPOUND_ROUND1_BITS);
    const __m128i d1    = _mm_srai_epi32(dst1, COMPOUND_ROUND1_BITS);
    return _mm_packs_epi32(d0, d1);
}

static INLINE __m256i jnt_2d_comp_avg_round_8_avx2(const __m256i src) {
    const __m256i round = _mm256_set1_epi32(1 << (COMPOUND_ROUND1_BITS - 1));
    const __m256i dst   = _mm256_add_epi32(src, round);
    const __m256i d     = _mm256_srai_epi32(dst, COMPOUND_ROUND1_BITS);
    return _mm256_packs_epi32(d, d);
}

static INLINE __m256i jnt_2d_comp_avg_round_16_avx2(const __m256i src[2]) {
    const __m256i round = _mm256_set1_epi32(1 << (COMPOUND_ROUND1_BITS - 1));
    const __m256i dst0  = _mm256_add_epi32(src[0], round);
    const __m256i dst1  = _mm256_add_epi32(src[1], round);
    const __m256i d0    = _mm256_srai_epi32(dst0, COMPOUND_ROUND1_BITS);
    const __m256i d1    = _mm256_srai_epi32(dst1, COMPOUND_ROUND1_BITS);
    return _mm256_packs_epi32(d0, d1);
}

static INLINE __m256i jnt_2d_comp_avg_round_half_pel_avx2(const __m256i src) {
    const __m256i round = _mm256_set1_epi16(1);
    const __m256i dst   = _mm256_add_epi16(src, round);
    return _mm256_srai_epi16(dst, 1);
}

static INLINE void jnt_2d_comp_avg_round_store_2x2_sse2(const __m128i res, const __m128i factor, const __m128i offset,
                                                        const ConvBufType* const dst, const ptrdiff_t dst_stride,
                                                        uint8_t* const dst8, const int32_t dst8_stride) {
    const __m128i r = jnt_2d_comp_avg_round_4_sse2(res);
    __m128i       d;

    d = load_u16_2x2_sse4_1(dst, dst_stride);
    d = _mm_unpacklo_epi16(d, r);
    d = _mm_madd_epi16(d, factor);
    d = _mm_add_epi32(d, offset);
    d = _mm_srai_epi32(d, 8);
    d = _mm_packs_epi32(d, d);
    pack_store_2x2_sse2(d, dst8, dst8_stride);
}

static INLINE void jnt_2d_comp_avg_round_store_half_pel_2x2_sse2(const __m128i res, const __m128i factor,
                                                                 const __m128i offset, const ConvBufType* const dst,
                                                                 const ptrdiff_t dst_stride, uint8_t* const dst8,
                                                                 const int32_t dst8_stride) {
    const __m128i r = jnt_2d_comp_avg_round_half_pel_sse2(res);
    __m128i       d;

    d = load_u16_2x2_sse4_1(dst, dst_stride);
    d = _mm_unpacklo_epi16(d, r);
    d = _mm_madd_epi16(d, factor);
    d = _mm_add_epi32(d, offset);
    d = _mm_srai_epi32(d, 8);
    d = _mm_packs_epi32(d, d);
    pack_store_2x2_sse2(d, dst8, dst8_stride);
}

static INLINE void jnt_2d_comp_avg_round_store_4x2_sse2(const __m128i res[2], const __m128i factor,
                                                        const __m128i offset, const ConvBufType* const dst,
                                                        const ptrdiff_t dst_stride, uint8_t* const dst8,
                                                        const int32_t dst8_stride) {
    const __m128i r = jnt_2d_comp_avg_round_4x2_sse2(res);
    const __m128i d = load_u16_4x2_sse2(dst, dst_stride);
    __m128i       dd[2];

    dd[0] = _mm_unpacklo_epi16(d, r);
    dd[1] = _mm_unpackhi_epi16(d, r);
    dd[0] = _mm_madd_epi16(dd[0], factor);
    dd[1] = _mm_madd_epi16(dd[1], factor);
    dd[0] = _mm_add_epi32(dd[0], offset);
    dd[1] = _mm_add_epi32(dd[1], offset);
    dd[0] = _mm_srai_epi32(dd[0], 8);
    dd[1] = _mm_srai_epi32(dd[1], 8);
    dd[0] = _mm_packs_epi32(dd[0], dd[1]);
    pack_store_4x2_sse2(dd[0], dst8, dst8_stride);
}

static INLINE void jnt_2d_comp_avg_round_store_half_pel_4x2_sse2(const __m128i res, const __m128i factor,
                                                                 const __m128i offset, const ConvBufType* const dst,
                                                                 const ptrdiff_t dst_stride, uint8_t* const dst8,
                                                                 const int32_t dst8_stride) {
    const __m128i r = jnt_2d_comp_avg_round_half_pel_sse2(res);
    const __m128i d = load_u16_4x2_sse2(dst, dst_stride);
    __m128i       dd[2];

    dd[0] = _mm_unpacklo_epi16(d, r);
    dd[1] = _mm_unpackhi_epi16(d, r);
    dd[0] = _mm_madd_epi16(dd[0], factor);
    dd[1] = _mm_madd_epi16(dd[1], factor);
    dd[0] = _mm_add_epi32(dd[0], offset);
    dd[1] = _mm_add_epi32(dd[1], offset);
    dd[0] = _mm_srai_epi32(dd[0], 8);
    dd[1] = _mm_srai_epi32(dd[1], 8);
    dd[0] = _mm_packs_epi32(dd[0], dd[1]);
    pack_store_4x2_sse2(dd[0], dst8, dst8_stride);
}

static INLINE __m256i jnt_2d_comp_avg_round_pack_8_avx2(const __m256i res, const __m256i factor, const __m256i offset,
                                                        const __m256i dst) {
    const __m256i r = jnt_2d_comp_avg_round_8_avx2(res);
    __m256i       d[2];

    d[0] = _mm256_unpacklo_epi16(dst, r);
    d[0] = _mm256_madd_epi16(d[0], factor);
    d[0] = _mm256_add_epi32(d[0], offset);
    d[0] = _mm256_srai_epi32(d[0], 8);
    return _mm256_packs_epi32(d[0], d[0]);
}

static INLINE __m256i jnt_2d_comp_avg_round_pack_16_avx2(const __m256i res[2], const __m256i factor,
                                                         const __m256i offset, const __m256i dst) {
    const __m256i r = jnt_2d_comp_avg_round_16_avx2(res);
    __m256i       d[2];

    d[0] = _mm256_unpacklo_epi16(dst, r);
    d[1] = _mm256_unpackhi_epi16(dst, r);
    d[0] = _mm256_madd_epi16(d[0], factor);
    d[1] = _mm256_madd_epi16(d[1], factor);
    d[0] = _mm256_add_epi32(d[0], offset);
    d[1] = _mm256_add_epi32(d[1], offset);
    d[0] = _mm256_srai_epi32(d[0], 8);
    d[1] = _mm256_srai_epi32(d[1], 8);
    return _mm256_packs_epi32(d[0], d[1]);
}

static INLINE __m256i jnt_2d_comp_avg_round_pack_half_pel_avx2(const __m256i res, const __m256i factor,
                                                               const __m256i offset, const __m256i dst) {
    const __m256i r = jnt_2d_comp_avg_round_half_pel_avx2(res);
    __m256i       d[2];

    d[0] = _mm256_unpacklo_epi16(dst, r);
    d[1] = _mm256_unpackhi_epi16(dst, r);
    d[0] = _mm256_madd_epi16(d[0], factor);
    d[1] = _mm256_madd_epi16(d[1], factor);
    d[0] = _mm256_add_epi32(d[0], offset);
    d[1] = _mm256_add_epi32(d[1], offset);
    d[0] = _mm256_srai_epi32(d[0], 8);
    d[1] = _mm256_srai_epi32(d[1], 8);
    return _mm256_packs_epi32(d[0], d[1]);
}

static INLINE void jnt_2d_comp_avg_round_store_4x2_avx2(const __m256i res, const __m256i factor, const __m256i offset,
                                                        const ConvBufType* const dst, const ptrdiff_t dst_stride,
                                                        uint8_t* const dst8, const int32_t dst8_stride) {
    __m256i d;

    d                = loadu_u16_8x2_avx2(dst, dst_stride);
    d                = jnt_2d_comp_avg_round_pack_8_avx2(res, factor, offset, d);
    d                = _mm256_packus_epi16(d, d);
    const __m128i d0 = _mm256_castsi256_si128(d);
    const __m128i d1 = _mm256_extracti128_si256(d, 1);
    xx_storel_32(dst8, d0);
    xx_storel_32(dst8 + dst8_stride, d1);
}

static INLINE void jnt_2d_comp_avg_round_store_8x2_avx2(const __m256i res[2], const __m256i factor,
                                                        const __m256i offset, const ConvBufType* const dst,
                                                        const ptrdiff_t dst_stride, uint8_t* const dst8,
                                                        const int32_t dst8_stride) {
    const __m256i d  = loadu_u16_8x2_avx2(dst, dst_stride);
    const __m256i dd = jnt_2d_comp_avg_round_pack_16_avx2(res, factor, offset, d);
    pack_store_8x2_avx2(dd, dst8, dst8_stride);
}

static INLINE void jnt_2d_comp_avg_round_store_half_pel_8x2_avx2(const __m256i res, const __m256i factor,
                                                                 const __m256i offset, const ConvBufType* const dst,
                                                                 const ptrdiff_t dst_stride, uint8_t* const dst8,
                                                                 const int32_t dst8_stride) {
    const __m256i d  = loadu_u16_8x2_avx2(dst, dst_stride);
    const __m256i dd = jnt_2d_comp_avg_round_pack_half_pel_avx2(res, factor, offset, d);
    pack_store_8x2_avx2(dd, dst8, dst8_stride);
}

static INLINE void jnt_2d_comp_avg_round_store_16x2_avx2(const __m256i res[4], const __m256i factor,
                                                         const __m256i offset, const ConvBufType* const dst,
                                                         const ptrdiff_t dst_stride, uint8_t* const dst8,
                                                         const int32_t dst8_stride) {
    __m256i d[2];

    d[0] = _mm256_loadu_si256((__m256i*)dst);
    d[1] = _mm256_loadu_si256((__m256i*)(dst + dst_stride));
    d[0] = jnt_2d_comp_avg_round_pack_16_avx2(res + 0, factor, offset, d[0]);
    d[1] = jnt_2d_comp_avg_round_pack_16_avx2(res + 2, factor, offset, d[1]);
    xy_y_pack_store_16x2_avx2(d[0], d[1], dst8, dst8_stride);
}

static INLINE void jnt_2d_comp_avg_round_store_half_pel_16x2_avx2(const __m256i res[2], const __m256i factor,
                                                                  const __m256i offset, const ConvBufType* const dst,
                                                                  const ptrdiff_t dst_stride, uint8_t* const dst8,
                                                                  const int32_t dst8_stride) {
    __m256i d[2];

    d[0] = _mm256_loadu_si256((__m256i*)dst);
    d[1] = _mm256_loadu_si256((__m256i*)(dst + dst_stride));
    d[0] = jnt_2d_comp_avg_round_pack_half_pel_avx2(res[0], factor, offset, d[0]);
    d[1] = jnt_2d_comp_avg_round_pack_half_pel_avx2(res[1], factor, offset, d[1]);
    xy_y_pack_store_16x2_avx2(d[0], d[1], dst8, dst8_stride);
}

SIMD_INLINE void jnt_2d_comp_avg_round_store_32_avx2(const __m256i r0[2], const __m256i r1[2], const __m256i factor,
                                                     const __m256i offset, const ConvBufType* const dst,
                                                     uint8_t* const dst8) {
    __m256i d[2];

    d[0] = loadu_u16_8x2_avx2(dst, 16);
    d[1] = loadu_u16_8x2_avx2(dst + 8, 16);
    d[0] = jnt_2d_comp_avg_round_pack_16_avx2(r0, factor, offset, d[0]);
    d[1] = jnt_2d_comp_avg_round_pack_16_avx2(r1, factor, offset, d[1]);
    convolve_store_32_avx2(d[0], d[1], dst8);
}

SIMD_INLINE void jnt_2d_comp_avg_round_store_half_pel_32_avx2(const __m256i res[2], const __m256i factor,
                                                              const __m256i offset, const ConvBufType* const dst,
                                                              uint8_t* const dst8) {
    __m256i d[2];

    d[0] = loadu_u16_8x2_avx2(dst, 16);
    d[1] = loadu_u16_8x2_avx2(dst + 8, 16);
    d[0] = jnt_2d_comp_avg_round_pack_half_pel_avx2(res[0], factor, offset, d[0]);
    d[1] = jnt_2d_comp_avg_round_pack_half_pel_avx2(res[1], factor, offset, d[1]);
    convolve_store_32_avx2(d[0], d[1], dst8);
}

static INLINE __m128i jnt_2d_round_4_sse2(const __m128i src, const __m128i offset) {
    const __m128i dst = _mm_add_epi32(src, offset);
    const __m128i d   = _mm_srai_epi32(dst, COMPOUND_ROUND1_BITS);
    return _mm_packs_epi32(d, d);
}

static INLINE __m128i jnt_2d_round_half_pel_sse2(const __m128i src, const __m128i offset) {
    const __m128i dst = _mm_add_epi16(src, offset);
    return _mm_srai_epi16(dst, 1);
}

static INLINE __m128i jnt_2d_round_4x2_sse2(const __m128i src[2], const __m128i offset) {
    const __m128i dst0 = _mm_add_epi32(src[0], offset);
    const __m128i dst1 = _mm_add_epi32(src[1], offset);
    const __m128i d0   = _mm_srai_epi32(dst0, COMPOUND_ROUND1_BITS);
    const __m128i d1   = _mm_srai_epi32(dst1, COMPOUND_ROUND1_BITS);
    return _mm_packs_epi32(d0, d1);
}

static INLINE __m256i jnt_2d_round_4x2_avx2(const __m256i src, const __m256i offset) {
    const __m256i dst = _mm256_add_epi32(src, offset);
    const __m256i d   = _mm256_srai_epi32(dst, COMPOUND_ROUND1_BITS);
    return _mm256_packs_epi32(d, d);
}

static INLINE __m256i jnt_2d_round_16_avx2(const __m256i src[2], const __m256i offset) {
    const __m256i dst0 = _mm256_add_epi32(src[0], offset);
    const __m256i dst1 = _mm256_add_epi32(src[1], offset);
    const __m256i d0   = _mm256_srai_epi32(dst0, COMPOUND_ROUND1_BITS);
    const __m256i d1   = _mm256_srai_epi32(dst1, COMPOUND_ROUND1_BITS);
    return _mm256_packs_epi32(d0, d1);
}

static INLINE __m256i jnt_2d_round_half_pel_avx2(const __m256i src, const __m256i offset) {
    const __m256i dst0 = _mm256_add_epi16(src, offset);
    return _mm256_srai_epi16(dst0, 1);
}

static INLINE void jnt_2d_avg_round_store_2x2_sse2(const __m128i res, const __m128i offset,
                                                   const ConvBufType* const dst, const ptrdiff_t dst_stride,
                                                   uint8_t* const dst8, const int32_t dst8_stride) {
    const __m128i r = jnt_2d_round_4_sse2(res, offset);
    __m128i       d;

    d = load_u16_2x2_sse4_1(dst, dst_stride);
    d = jnt_avg_8_sse2(r, d);
    pack_store_2x2_sse2(d, dst8, dst8_stride);
}

static INLINE void jnt_2d_avg_round_store_half_pel_2x2_sse2(const __m128i res, const __m128i offset,
                                                            const ConvBufType* const dst, const ptrdiff_t dst_stride,
                                                            uint8_t* const dst8, const int32_t dst8_stride) {
    const __m128i r = jnt_2d_round_half_pel_sse2(res, offset);
    __m128i       d;

    d = load_u16_2x2_sse4_1(dst, dst_stride);
    d = jnt_avg_8_sse2(r, d);
    pack_store_2x2_sse2(d, dst8, dst8_stride);
}

static INLINE void jnt_2d_avg_round_store_4x2_sse2(const __m128i res[2], const __m128i offset,
                                                   const ConvBufType* const dst, const ptrdiff_t dst_stride,
                                                   uint8_t* const dst8, const int32_t dst8_stride) {
    const __m128i r = jnt_2d_round_4x2_sse2(res, offset);
    __m128i       d;

    d = load_u16_4x2_sse2(dst, dst_stride);
    d = jnt_avg_8_sse2(r, d);
    pack_store_4x2_sse2(d, dst8, dst8_stride);
}

static INLINE void jnt_2d_avg_round_store_half_pel_4x2_sse2(const __m128i res, const __m128i offset,
                                                            const ConvBufType* const dst, const ptrdiff_t dst_stride,
                                                            uint8_t* const dst8, const int32_t dst8_stride) {
    const __m128i r = jnt_2d_round_half_pel_sse2(res, offset);
    __m128i       d;

    d = load_u16_4x2_sse2(dst, dst_stride);
    d = jnt_avg_8_sse2(r, d);
    pack_store_4x2_sse2(d, dst8, dst8_stride);
}

static INLINE void jnt_2d_avg_round_store_4x2_avx2(const __m256i res, const __m256i offset,
                                                   const ConvBufType* const dst, const ptrdiff_t dst_stride,
                                                   uint8_t* const dst8, const int32_t dst8_stride) {
    const __m256i r = jnt_2d_round_4x2_avx2(res, offset);
    __m128i       d_128[2];
    __m256i       d;

    d_128[0]         = _mm_loadl_epi64((__m128i*)dst);
    d_128[1]         = _mm_loadl_epi64((__m128i*)(dst + dst_stride));
    d                = _mm256_setr_m128i(d_128[0], d_128[1]);
    d                = jnt_avg_16_avx2(r, d);
    d                = _mm256_packus_epi16(d, d);
    const __m128i d0 = _mm256_castsi256_si128(d);
    const __m128i d1 = _mm256_extracti128_si256(d, 1);
    xx_storel_32(dst8, d0);
    xx_storel_32(dst8 + dst8_stride, d1);
}

static INLINE void jnt_2d_avg_round_store_8x2_avx2(const __m256i res[2], const __m256i offset,
                                                   const ConvBufType* const dst, const ptrdiff_t dst_stride,
                                                   uint8_t* const dst8, const int32_t dst8_stride) {
    const __m256i r = jnt_2d_round_16_avx2(res, offset);
    __m256i       d;

    d = loadu_u16_8x2_avx2(dst, dst_stride);
    d = jnt_avg_16_avx2(r, d);
    pack_store_8x2_avx2(d, dst8, dst8_stride);
}

static INLINE void jnt_2d_avg_round_store_half_pel_8x2_avx2(const __m256i res, const __m256i offset,
                                                            const ConvBufType* const dst, const ptrdiff_t dst_stride,
                                                            uint8_t* const dst8, const int32_t dst8_stride) {
    const __m256i r = jnt_2d_round_half_pel_avx2(res, offset);
    __m256i       d;

    d = loadu_u16_8x2_avx2(dst, dst_stride);
    d = jnt_avg_16_avx2(r, d);
    pack_store_8x2_avx2(d, dst8, dst8_stride);
}

static INLINE void jnt_2d_avg_round_store_16x2_avx2(const __m256i res[4], const __m256i offset,
                                                    const ConvBufType* const dst, const ptrdiff_t dst_stride,
                                                    uint8_t* const dst8, const int32_t dst8_stride) {
    __m256i r[2], d[2];

    r[0] = jnt_2d_round_16_avx2(res + 0, offset);
    r[1] = jnt_2d_round_16_avx2(res + 2, offset);
    d[0] = _mm256_loadu_si256((__m256i*)dst);
    d[1] = _mm256_loadu_si256((__m256i*)(dst + dst_stride));
    d[0] = jnt_avg_16_avx2(r[0], d[0]);
    d[1] = jnt_avg_16_avx2(r[1], d[1]);
    xy_y_pack_store_16x2_avx2(d[0], d[1], dst8, dst8_stride);
}

static INLINE void jnt_2d_avg_round_store_half_pel_16x2_avx2(const __m256i res[2], const __m256i offset,
                                                             const ConvBufType* const dst, const ptrdiff_t dst_stride,
                                                             uint8_t* const dst8, const int32_t dst8_stride) {
    __m256i r[2], d[2];

    r[0] = jnt_2d_round_half_pel_avx2(res[0], offset);
    r[1] = jnt_2d_round_half_pel_avx2(res[1], offset);
    d[0] = _mm256_loadu_si256((__m256i*)dst);
    d[1] = _mm256_loadu_si256((__m256i*)(dst + dst_stride));
    d[0] = jnt_avg_16_avx2(r[0], d[0]);
    d[1] = jnt_avg_16_avx2(r[1], d[1]);
    xy_y_pack_store_16x2_avx2(d[0], d[1], dst8, dst8_stride);
}

SIMD_INLINE void jnt_2d_avg_round_store_32_avx2(const __m256i r0[2], const __m256i r1[2], const __m256i offset,
                                                const ConvBufType* const dst, uint8_t* const dst8) {
    __m256i r[2], d[2];

    r[0] = jnt_2d_round_16_avx2(r0, offset);
    r[1] = jnt_2d_round_16_avx2(r1, offset);
    d[0] = loadu_u16_8x2_avx2(dst, 16);
    d[1] = loadu_u16_8x2_avx2(dst + 8, 16);
    d[0] = jnt_avg_16_avx2(r[0], d[0]);
    d[1] = jnt_avg_16_avx2(r[1], d[1]);
    convolve_store_32_avx2(d[0], d[1], dst8);
}

static INLINE void jnt_2d_avg_round_store_half_pel_32_avx2(const __m256i res[2], const __m256i offset,
                                                           const ConvBufType* const dst, uint8_t* const dst8) {
    __m256i r[2], d[2];

    r[0] = jnt_2d_round_half_pel_avx2(res[0], offset);
    r[1] = jnt_2d_round_half_pel_avx2(res[1], offset);
    d[0] = loadu_u16_8x2_avx2(dst, 16);
    d[1] = loadu_u16_8x2_avx2(dst + 8, 16);
    d[0] = jnt_avg_16_avx2(r[0], d[0]);
    d[1] = jnt_avg_16_avx2(r[1], d[1]);
    convolve_store_32_avx2(d[0], d[1], dst8);
}

static INLINE void jnt_2d_no_avg_round_store_2x2_sse2(const __m128i res, const __m128i offset, ConvBufType* const dst,
                                                      const ptrdiff_t dst_stride) {
    const __m128i d = jnt_2d_round_4_sse2(res, offset);
    store_u16_2x2_sse2(d, dst, dst_stride);
}

static INLINE void jnt_2d_no_avg_round_store_half_pel_2x2_sse2(const __m128i res, const __m128i offset,
                                                               ConvBufType* const dst, const ptrdiff_t dst_stride) {
    const __m128i d = jnt_2d_round_half_pel_sse2(res, offset);
    store_u16_2x2_sse2(d, dst, dst_stride);
}

static INLINE void jnt_2d_no_avg_round_store_4x2_sse2(const __m128i res[2], const __m128i offset,
                                                      ConvBufType* const dst, const ptrdiff_t dst_stride) {
    const __m128i d = jnt_2d_round_4x2_sse2(res, offset);
    store_u16_4x2_sse2(d, dst, dst_stride);
}

static INLINE void jnt_2d_no_avg_round_store_4x2_avx2(const __m256i res, const __m256i offset, ConvBufType* const dst,
                                                      const ptrdiff_t dst_stride) {
    const __m256i d  = jnt_2d_round_4x2_avx2(res, offset);
    const __m128i d0 = _mm256_castsi256_si128(d);
    const __m128i d1 = _mm256_extracti128_si256(d, 1);
    _mm_storel_epi64((__m128i*)dst, d0);
    _mm_storel_epi64((__m128i*)(dst + dst_stride), d1);
}

static INLINE void jnt_2d_no_avg_round_store_half_pel_4x2_sse2(const __m128i res, const __m128i offset,
                                                               ConvBufType* const dst, const ptrdiff_t dst_stride) {
    const __m128i d = jnt_2d_round_half_pel_sse2(res, offset);
    store_u16_4x2_sse2(d, dst, dst_stride);
}

static INLINE void jnt_2d_no_avg_round_store_8x2_avx2(const __m256i res[2], const __m256i offset,
                                                      ConvBufType* const dst, const ptrdiff_t dst_stride) {
    const __m256i d = jnt_2d_round_16_avx2(res, offset);
    storeu_u16_8x2_avx2(d, dst, dst_stride);
}

static INLINE void jnt_2d_no_avg_round_store_half_pel_8x2_avx2(const __m256i res, const __m256i offset,
                                                               ConvBufType* const dst, const ptrdiff_t dst_stride) {
    const __m256i d = jnt_2d_round_half_pel_avx2(res, offset);
    storeu_u16_8x2_avx2(d, dst, dst_stride);
}

static INLINE void jnt_2d_no_avg_round_store_16x2_avx2(const __m256i res[4], const __m256i offset,
                                                       ConvBufType* const dst, const ptrdiff_t dst_stride) {
    const __m256i d0 = jnt_2d_round_16_avx2(res + 0, offset);
    const __m256i d1 = jnt_2d_round_16_avx2(res + 2, offset);
    _mm256_storeu_si256((__m256i*)dst, d0);
    _mm256_storeu_si256((__m256i*)(dst + dst_stride), d1);
}

static INLINE void jnt_2d_no_avg_round_store_half_pel_16x2_avx2(const __m256i res[2], const __m256i offset,
                                                                ConvBufType* const dst, const ptrdiff_t dst_stride) {
    const __m256i d0 = jnt_2d_round_half_pel_avx2(res[0], offset);
    const __m256i d1 = jnt_2d_round_half_pel_avx2(res[1], offset);
    _mm256_storeu_si256((__m256i*)dst, d0);
    _mm256_storeu_si256((__m256i*)(dst + dst_stride), d1);
}

static INLINE void jnt_2d_no_avg_round_store_32_avx2(const __m256i r0[2], const __m256i r1[2], const __m256i offset,
                                                     ConvBufType* const dst) {
    const __m256i d0 = jnt_2d_round_16_avx2(r0, offset);
    const __m256i d1 = jnt_2d_round_16_avx2(r1, offset);
    jnt_no_avg_store_16x2_avx2(d0, d1, dst, 16);
}

static INLINE void jnt_2d_no_avg_round_store_half_pel_32_avx2(const __m256i res[2], const __m256i offset,
                                                              ConvBufType* const dst) {
    const __m256i d0 = jnt_2d_round_half_pel_avx2(res[0], offset);
    const __m256i d1 = jnt_2d_round_half_pel_avx2(res[1], offset);
    jnt_no_avg_store_16x2_avx2(d0, d1, dst, 16);
}

static INLINE __m256i highbd_comp_avg(const __m256i* const data_ref_0, const __m256i* const res_unsigned,
                                      const __m256i* const wt0, const __m256i* const wt1,
                                      const int32_t use_jnt_comp_avg) {
    __m256i res;
    if (use_jnt_comp_avg) {
        const __m256i wt0_res = _mm256_mullo_epi32(*data_ref_0, *wt0);
        const __m256i wt1_res = _mm256_mullo_epi32(*res_unsigned, *wt1);
        const __m256i wt_res  = _mm256_add_epi32(wt0_res, wt1_res);
        res                   = _mm256_srai_epi32(wt_res, DIST_PRECISION_BITS);
    } else {
        const __m256i wt_res = _mm256_add_epi32(*data_ref_0, *res_unsigned);
        res                  = _mm256_srai_epi32(wt_res, 1);
    }
    return res;
}

static INLINE __m256i highbd_convolve_rounding(const __m256i* const res_unsigned, const __m256i* const offset_const,
                                               const __m256i* const round_const, const int32_t round_shift) {
    const __m256i res_signed = _mm256_sub_epi32(*res_unsigned, *offset_const);
    const __m256i res_round  = _mm256_srai_epi32(_mm256_add_epi32(res_signed, *round_const), round_shift);

    return res_round;
}

#endif
