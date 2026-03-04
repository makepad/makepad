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

#ifndef AOM_DSP_X86_CONVOLVE_AVX512_H_
#define AOM_DSP_X86_CONVOLVE_AVX512_H_

#include "convolve.h"
#include "definitions.h"
#include "inter_prediction.h"
#include "memory_avx2.h"
#include "memory_sse4_1.h"
#include "synonyms_avx512.h"

#if EN_AVX512_SUPPORT

static INLINE __m512i svt_mm512_broadcast_i64x2(const __m128i v) {
#ifdef _WIN32
    // Work around of Visual Studio warning C4305: 'function': truncation from
    // 'int' to '__mmask8'. It's a flaw in header file zmmintrin.h.
    return _mm512_maskz_broadcast_i64x2((__mmask8)0xffff, v);
#else
    return _mm512_broadcast_i64x2(v);
#endif
}

static INLINE __m512i _mm512_setr_m256i(const __m256i src0, const __m256i src1) {
    return _mm512_inserti64x4(_mm512_castsi256_si512(src0), src1, 1);
}

static INLINE __m512i loadu_8bit_32x2_avx512(const void* const src, const ptrdiff_t strideInByte) {
    const __m256i src0 = _mm256_loadu_si256((__m256i*)src);
    const __m256i src1 = _mm256_loadu_si256((__m256i*)((uint8_t*)src + strideInByte));
    return _mm512_setr_m256i(src0, src1);
}

static INLINE __m512i loadu_u8_32x2_avx512(const uint8_t* const src, const ptrdiff_t stride) {
    return loadu_8bit_32x2_avx512(src, sizeof(*src) * stride);
}

static INLINE __m512i loadu_s16_16x2_avx512(const int16_t* const src, const ptrdiff_t stride) {
    return loadu_8bit_32x2_avx512(src, sizeof(*src) * stride);
}

static INLINE void storeu_8bit_32x2_avx512(const __m512i src, void* const dst, const ptrdiff_t strideInByte) {
    const __m256i d0 = _mm512_castsi512_si256(src);
    const __m256i d1 = _mm512_extracti64x4_epi64(src, 1);
    _mm256_storeu_si256((__m256i*)dst, d0);
    _mm256_storeu_si256((__m256i*)((uint8_t*)dst + strideInByte), d1);
}

static INLINE void storeu_u8_32x2_avx512(const __m512i src, uint8_t* const dst, const ptrdiff_t stride) {
    storeu_8bit_32x2_avx512(src, dst, sizeof(*dst) * stride);
}

static INLINE void populate_coeffs_4tap_avx512(const __m128i coeffs_128, __m512i coeffs[2]) {
    const __m512i coeffs_512 = svt_mm512_broadcast_i64x2(coeffs_128);

    // coeffs 2 3 2 3 2 3 2 3
    coeffs[0] = _mm512_shuffle_epi8(coeffs_512, _mm512_set1_epi16(0x0604u));
    // coeffs 4 5 4 5 4 5 4 5
    coeffs[1] = _mm512_shuffle_epi8(coeffs_512, _mm512_set1_epi16(0x0a08u));
}

static INLINE void populate_coeffs_6tap_avx512(const __m128i coeffs_128, __m512i coeffs[3]) {
    const __m512i coeffs_512 = svt_mm512_broadcast_i64x2(coeffs_128);

    // coeffs 1 2 1 2 1 2 1 2
    coeffs[0] = _mm512_shuffle_epi8(coeffs_512, _mm512_set1_epi16(0x0402u));
    // coeffs 3 4 3 4 3 4 3 4
    coeffs[1] = _mm512_shuffle_epi8(coeffs_512, _mm512_set1_epi16(0x0806u));
    // coeffs 5 6 5 6 5 6 5 6
    coeffs[2] = _mm512_shuffle_epi8(coeffs_512, _mm512_set1_epi16(0x0C0Au));
}

static INLINE void populate_coeffs_8tap_avx512(const __m128i coeffs_128, __m512i coeffs[4]) {
    const __m512i coeffs_512 = svt_mm512_broadcast_i64x2(coeffs_128);

    // coeffs 0 1 0 1 0 1 0 1
    coeffs[0] = _mm512_shuffle_epi8(coeffs_512, _mm512_set1_epi16(0x0200u));
    // coeffs 2 3 2 3 2 3 2 3
    coeffs[1] = _mm512_shuffle_epi8(coeffs_512, _mm512_set1_epi16(0x0604u));
    // coeffs 4 5 4 5 4 5 4 5
    coeffs[2] = _mm512_shuffle_epi8(coeffs_512, _mm512_set1_epi16(0x0a08u));
    // coeffs 6 7 6 7 6 7 6 7
    coeffs[3] = _mm512_shuffle_epi8(coeffs_512, _mm512_set1_epi16(0x0e0cu));
}

static INLINE void prepare_half_coeffs_2tap_avx512(const InterpFilterParams* const filter_params,
                                                   const int32_t subpel_q4, __m512i* const coeffs /* [1] */) {
    const int16_t* const filter        = av1_get_interp_filter_subpel_kernel(*filter_params, subpel_q4 & SUBPEL_MASK);
    const __m128i        coeffs_8      = _mm_cvtsi32_si128(*(const int32_t*)(filter + 3));
    const __m512i        filter_coeffs = _mm512_broadcastd_epi32(coeffs_8);

    // right shift all filter co-efficients by 1 to reduce the bits required.
    // This extra right shift will be taken care of at the end while rounding
    // the result.
    // Since all filter co-efficients are even, this change will not affect the
    // end result
    assert(_mm_test_all_zeros(_mm_and_si128(coeffs_8, _mm_set1_epi16(1)), _mm_set1_epi16((short)0xffff)));

    const __m512i coeffs_1 = _mm512_srai_epi16(filter_coeffs, 1);

    // coeffs 3 4 3 4 3 4 3 4
    *coeffs = _mm512_shuffle_epi8(coeffs_1, _mm512_set1_epi16(0x0200u));
}

static INLINE void prepare_half_coeffs_4tap_avx512(const InterpFilterParams* const filter_params,
                                                   const int32_t subpel_q4, __m512i* const coeffs /* [2] */) {
    const int16_t* const filter   = av1_get_interp_filter_subpel_kernel(*filter_params, subpel_q4 & SUBPEL_MASK);
    const __m128i        coeffs_8 = _mm_loadu_si128((__m128i*)filter);

    // right shift all filter co-efficients by 1 to reduce the bits required.
    // This extra right shift will be taken care of at the end while rounding
    // the result.
    // Since all filter co-efficients are even, this change will not affect the
    // end result
    assert(_mm_test_all_zeros(_mm_and_si128(coeffs_8, _mm_set1_epi16(1)), _mm_set1_epi16((short)0xffff)));
    const __m128i coeffs_1 = _mm_srai_epi16(coeffs_8, 1);
    populate_coeffs_4tap_avx512(coeffs_1, coeffs);
}

static INLINE void prepare_half_coeffs_6tap_avx512(const InterpFilterParams* const filter_params,
                                                   const int32_t subpel_q4, __m512i* const coeffs /* [3] */) {
    const int16_t* const filter   = av1_get_interp_filter_subpel_kernel(*filter_params, subpel_q4 & SUBPEL_MASK);
    const __m128i        coeffs_8 = _mm_loadu_si128((__m128i*)filter);

    // right shift all filter co-efficients by 1 to reduce the bits required.
    // This extra right shift will be taken care of at the end while rounding
    // the result.
    // Since all filter co-efficients are even, this change will not affect the
    // end result
    assert(_mm_test_all_zeros(_mm_and_si128(coeffs_8, _mm_set1_epi16(1)), _mm_set1_epi16((short)0xffff)));
    const __m128i coeffs_1 = _mm_srai_epi16(coeffs_8, 1);
    populate_coeffs_6tap_avx512(coeffs_1, coeffs);
}

SIMD_INLINE void prepare_half_coeffs_8tap_avx512(const InterpFilterParams* const filter_params, const int32_t subpel_q4,
                                                 __m512i* const coeffs /* [4] */) {
    const int16_t* const filter   = av1_get_interp_filter_subpel_kernel(*filter_params, subpel_q4 & SUBPEL_MASK);
    const __m128i        coeffs_8 = _mm_loadu_si128((__m128i*)filter);

    // right shift all filter co-efficients by 1 to reduce the bits required.
    // This extra right shift will be taken care of at the end while rounding
    // the result.
    // Since all filter co-efficients are even, this change will not affect the
    // end result
    assert(_mm_test_all_zeros(_mm_and_si128(coeffs_8, _mm_set1_epi16(1)), _mm_set1_epi16((short)0xffff)));
    const __m128i coeffs_1 = _mm_srai_epi16(coeffs_8, 1);
    populate_coeffs_8tap_avx512(coeffs_1, coeffs);
}

static INLINE void prepare_coeffs_2tap_avx512(const InterpFilterParams* const filter_params, const int32_t subpel_q4,
                                              __m512i* const coeffs /* [1] */) {
    const int16_t* filter = av1_get_interp_filter_subpel_kernel(*filter_params, subpel_q4 & SUBPEL_MASK);

    const __m128i coeff_8 = _mm_cvtsi32_si128(*(const int32_t*)(filter + 3));
    const __m512i coeff   = _mm512_broadcastd_epi32(coeff_8);

    // coeffs 3 4 3 4 3 4 3 4
    coeffs[0] = _mm512_shuffle_epi32(coeff, 0x00);
}

static INLINE void prepare_coeffs_4tap_avx512(const InterpFilterParams* const filter_params, const int32_t subpel_q4,
                                              __m512i* const coeffs /* [2] */) {
    const int16_t* filter = av1_get_interp_filter_subpel_kernel(*filter_params, subpel_q4 & SUBPEL_MASK);

    const __m128i coeff_8 = _mm_loadu_si128((__m128i*)filter);
    const __m512i coeff   = svt_mm512_broadcast_i64x2(coeff_8);

    // coeffs 2 3 2 3 2 3 2 3
    coeffs[0] = _mm512_shuffle_epi32(coeff, 0x55);
    // coeffs 4 5 4 5 4 5 4 5
    coeffs[1] = _mm512_shuffle_epi32(coeff, 0xaa);
}

static INLINE void prepare_coeffs_6tap_avx512(const InterpFilterParams* const filter_params, const int32_t subpel_q4,
                                              __m512i* const coeffs /* [3] */) {
    const int16_t* const filter   = av1_get_interp_filter_subpel_kernel(*filter_params, subpel_q4 & SUBPEL_MASK);
    const __m128i        coeffs_8 = _mm_loadu_si128((__m128i*)filter);
    const __m512i        coeff    = svt_mm512_broadcast_i64x2(coeffs_8);

    // coeffs 1 2 1 2 1 2 1 2
    coeffs[0] = _mm512_shuffle_epi8(coeff, _mm512_set1_epi32(0x05040302u));
    // coeffs 3 4 3 4 3 4 3 4
    coeffs[1] = _mm512_shuffle_epi8(coeff, _mm512_set1_epi32(0x09080706u));
    // coeffs 5 6 5 6 5 6 5 6
    coeffs[2] = _mm512_shuffle_epi8(coeff, _mm512_set1_epi32(0x0D0C0B0Au));
}

static INLINE void prepare_coeffs_8tap_avx512(const InterpFilterParams* const filter_params, const int32_t subpel_q4,
                                              __m512i* const coeffs /* [4] */) {
    const int16_t* filter = av1_get_interp_filter_subpel_kernel(*filter_params, subpel_q4 & SUBPEL_MASK);

    const __m128i coeff_8 = _mm_loadu_si128((__m128i*)filter);
    const __m512i coeff   = svt_mm512_broadcast_i64x2(coeff_8);

    // coeffs 0 1 0 1 0 1 0 1
    coeffs[0] = _mm512_shuffle_epi32(coeff, 0x00);
    // coeffs 2 3 2 3 2 3 2 3
    coeffs[1] = _mm512_shuffle_epi32(coeff, 0x55);
    // coeffs 4 5 4 5 4 5 4 5
    coeffs[2] = _mm512_shuffle_epi32(coeff, 0xaa);
    // coeffs 6 7 6 7 6 7 6 7
    coeffs[3] = _mm512_shuffle_epi32(coeff, 0xff);
}

static INLINE void load_16bit_32x7_avx512(const int16_t* const src, __m512i dst[7]) {
    dst[0] = loadu_s16_16x2_avx512(src + 0 * 16, 32);
    dst[1] = loadu_s16_16x2_avx512(src + 1 * 16, 32);
    dst[2] = loadu_s16_16x2_avx512(src + 4 * 16, 32);
    dst[3] = loadu_s16_16x2_avx512(src + 5 * 16, 32);
    dst[4] = loadu_s16_16x2_avx512(src + 8 * 16, 32);
    dst[5] = loadu_s16_16x2_avx512(src + 9 * 16, 32);
    dst[6] = loadu_s16_16x2_avx512(src + 12 * 16, 32);
}

SIMD_INLINE void load_16bit_4rows_avx512(const int16_t* const src, const ptrdiff_t stride, __m512i dst[4]) {
    dst[0] = zz_load_512(src + 0 * stride);
    dst[1] = zz_load_512(src + 1 * stride);
    dst[2] = zz_load_512(src + 2 * stride);
    dst[3] = zz_load_512(src + 3 * stride);
}

static INLINE void load_16bit_7rows_avx512(const int16_t* const src, const ptrdiff_t stride, __m512i dst[7]) {
    dst[0] = zz_load_512(src + 0 * stride);
    dst[1] = zz_load_512(src + 1 * stride);
    dst[2] = zz_load_512(src + 2 * stride);
    dst[3] = zz_load_512(src + 3 * stride);
    dst[4] = zz_load_512(src + 4 * stride);
    dst[5] = zz_load_512(src + 5 * stride);
    dst[6] = zz_load_512(src + 6 * stride);
}

SIMD_INLINE void loadu_unpack_16bit_5rows_avx512(const int16_t* const src, const ptrdiff_t stride, __m512i s_512[5],
                                                 __m512i ss_512[5], __m512i tt_512[5]) {
    s_512[0] = _mm512_loadu_si512((__m512i*)(src + 0 * stride));
    s_512[1] = _mm512_loadu_si512((__m512i*)(src + 1 * stride));
    s_512[2] = _mm512_loadu_si512((__m512i*)(src + 2 * stride));
    s_512[3] = _mm512_loadu_si512((__m512i*)(src + 3 * stride));
    s_512[4] = _mm512_loadu_si512((__m512i*)(src + 4 * stride));

    ss_512[0] = _mm512_unpacklo_epi16(s_512[0], s_512[1]);
    ss_512[1] = _mm512_unpacklo_epi16(s_512[2], s_512[3]);
    ss_512[3] = _mm512_unpackhi_epi16(s_512[0], s_512[1]);
    ss_512[4] = _mm512_unpackhi_epi16(s_512[2], s_512[3]);

    tt_512[0] = _mm512_unpacklo_epi16(s_512[1], s_512[2]);
    tt_512[1] = _mm512_unpacklo_epi16(s_512[3], s_512[4]);
    tt_512[3] = _mm512_unpackhi_epi16(s_512[1], s_512[2]);
    tt_512[4] = _mm512_unpackhi_epi16(s_512[3], s_512[4]);
}

SIMD_INLINE void loadu_unpack_16bit_32x3_avx512(const int16_t* const src, __m512i s_512[3], __m512i ss_512[3],
                                                __m512i tt_512[3]) {
    s_512[0] = loadu_s16_16x2_avx512(src + 0 * 16, 32);
    s_512[1] = loadu_s16_16x2_avx512(src + 1 * 16, 32);
    s_512[2] = loadu_s16_16x2_avx512(src + 4 * 16, 32);

    ss_512[0] = _mm512_unpacklo_epi16(s_512[0], s_512[1]);
    ss_512[2] = _mm512_unpackhi_epi16(s_512[0], s_512[1]);

    tt_512[0] = _mm512_unpacklo_epi16(s_512[1], s_512[2]);
    tt_512[2] = _mm512_unpackhi_epi16(s_512[1], s_512[2]);
}

SIMD_INLINE void loadu_unpack_16bit_32x5_avx512(const int16_t* const src, __m512i s_512[5], __m512i ss_512[5],
                                                __m512i tt_512[5]) {
    s_512[0] = loadu_s16_16x2_avx512(src + 0 * 16, 32);
    s_512[1] = loadu_s16_16x2_avx512(src + 1 * 16, 32);
    s_512[2] = loadu_s16_16x2_avx512(src + 4 * 16, 32);
    s_512[3] = loadu_s16_16x2_avx512(src + 5 * 16, 32);
    s_512[4] = loadu_s16_16x2_avx512(src + 8 * 16, 32);

    ss_512[0] = _mm512_unpacklo_epi16(s_512[0], s_512[1]);
    ss_512[1] = _mm512_unpacklo_epi16(s_512[2], s_512[3]);
    ss_512[3] = _mm512_unpackhi_epi16(s_512[0], s_512[1]);
    ss_512[4] = _mm512_unpackhi_epi16(s_512[2], s_512[3]);

    tt_512[0] = _mm512_unpacklo_epi16(s_512[1], s_512[2]);
    tt_512[1] = _mm512_unpacklo_epi16(s_512[3], s_512[4]);
    tt_512[3] = _mm512_unpackhi_epi16(s_512[1], s_512[2]);
    tt_512[4] = _mm512_unpackhi_epi16(s_512[3], s_512[4]);
}

static INLINE void convolve_8tap_unapck_avx512(const __m512i s[6], __m512i ss[7]) {
    ss[0] = _mm512_unpacklo_epi16(s[0], s[1]);
    ss[1] = _mm512_unpacklo_epi16(s[2], s[3]);
    ss[2] = _mm512_unpacklo_epi16(s[4], s[5]);
    ss[4] = _mm512_unpackhi_epi16(s[0], s[1]);
    ss[5] = _mm512_unpackhi_epi16(s[2], s[3]);
    ss[6] = _mm512_unpackhi_epi16(s[4], s[5]);
}

static INLINE __m512i convolve_2tap_avx512(const __m512i ss[1], const __m512i coeffs[1]) {
    return _mm512_maddubs_epi16(ss[0], coeffs[0]);
}

static INLINE __m512i convolve_4tap_avx512(const __m512i ss[2], const __m512i coeffs[2]) {
    const __m512i res_23 = _mm512_maddubs_epi16(ss[0], coeffs[0]);
    const __m512i res_45 = _mm512_maddubs_epi16(ss[1], coeffs[1]);
    return _mm512_add_epi16(res_23, res_45);
}

static INLINE __m512i convolve_6tap_avx512(const __m512i ss[3], const __m512i coeffs[3]) {
    const __m512i res_01   = _mm512_maddubs_epi16(ss[0], coeffs[0]);
    const __m512i res_23   = _mm512_maddubs_epi16(ss[1], coeffs[1]);
    const __m512i res_45   = _mm512_maddubs_epi16(ss[2], coeffs[2]);
    const __m512i res_0145 = _mm512_add_epi16(res_01, res_45);
    return _mm512_add_epi16(res_0145, res_23);
}

static INLINE __m512i convolve_8tap_avx512(const __m512i ss[4], const __m512i coeffs[4]) {
    const __m512i res_01   = _mm512_maddubs_epi16(ss[0], coeffs[0]);
    const __m512i res_23   = _mm512_maddubs_epi16(ss[1], coeffs[1]);
    const __m512i res_45   = _mm512_maddubs_epi16(ss[2], coeffs[2]);
    const __m512i res_67   = _mm512_maddubs_epi16(ss[3], coeffs[3]);
    const __m512i res_0145 = _mm512_add_epi16(res_01, res_45);
    const __m512i res_2367 = _mm512_add_epi16(res_23, res_67);
    return _mm512_add_epi16(res_0145, res_2367);
}

static INLINE __m512i convolve16_2tap_avx512(const __m512i ss[1], const __m512i coeffs[1]) {
    return _mm512_madd_epi16(ss[0], coeffs[0]);
}

static INLINE __m512i convolve16_4tap_avx512(const __m512i ss[2], const __m512i coeffs[2]) {
    const __m512i res_1 = _mm512_madd_epi16(ss[0], coeffs[0]);
    const __m512i res_2 = _mm512_madd_epi16(ss[1], coeffs[1]);
    return _mm512_add_epi32(res_1, res_2);
}

static INLINE __m512i convolve16_6tap_avx512(const __m512i ss[3], const __m512i coeffs[3]) {
    const __m512i res_01   = _mm512_madd_epi16(ss[0], coeffs[0]);
    const __m512i res_23   = _mm512_madd_epi16(ss[1], coeffs[1]);
    const __m512i res_45   = _mm512_madd_epi16(ss[2], coeffs[2]);
    const __m512i res_0123 = _mm512_add_epi32(res_01, res_23);
    return _mm512_add_epi32(res_0123, res_45);
}

static INLINE __m512i convolve16_8tap_avx512(const __m512i ss[4], const __m512i coeffs[4]) {
    const __m512i res_01   = _mm512_madd_epi16(ss[0], coeffs[0]);
    const __m512i res_23   = _mm512_madd_epi16(ss[1], coeffs[1]);
    const __m512i res_45   = _mm512_madd_epi16(ss[2], coeffs[2]);
    const __m512i res_67   = _mm512_madd_epi16(ss[3], coeffs[3]);
    const __m512i res_0123 = _mm512_add_epi32(res_01, res_23);
    const __m512i res_4567 = _mm512_add_epi32(res_45, res_67);
    return _mm512_add_epi32(res_0123, res_4567);
}

static INLINE __m512i sr_y_round_avx512(const __m512i src) {
    const __m512i round = _mm512_set1_epi16(32);
    const __m512i dst   = _mm512_add_epi16(src, round);
    return _mm512_srai_epi16(dst, FILTER_BITS - 1);
}

static INLINE __m512i xy_x_round_avx512(const __m512i src) {
    const __m512i round = _mm512_set1_epi16(2);
    const __m512i dst   = _mm512_add_epi16(src, round);
    return _mm512_srai_epi16(dst, 2);
}

static INLINE __m512i xy_y_round_avx512(const __m512i src) {
    const __m512i round = _mm512_set1_epi32(1024);
    const __m512i dst   = _mm512_add_epi32(src, round);
    return _mm512_srai_epi32(dst, 11);
}

static INLINE __m512i xy_y_round_16_avx512(const __m512i r0, const __m512i r1) {
    const __m512i d0 = xy_y_round_avx512(r0);
    const __m512i d1 = xy_y_round_avx512(r1);
    return _mm512_packs_epi32(d0, d1);
}

static INLINE __m512i xy_y_round_32_avx512(const __m512i r[2]) {
    const __m512i r0 = xy_y_round_avx512(r[0]);
    const __m512i r1 = xy_y_round_avx512(r[1]);
    return _mm512_packs_epi32(r0, r1);
}

static INLINE __m512i xy_y_round_half_pel_avx512(const __m512i src) {
    const __m512i round = _mm512_set1_epi16(16);
    const __m512i dst   = _mm512_add_epi16(src, round);
    return _mm512_srai_epi16(dst, 5);
}

static INLINE __m512i jnt_y_round_avx512(const __m512i src) {
    const __m512i round = _mm512_set1_epi16(2);
    const __m512i dst   = _mm512_add_epi16(src, round);
    return _mm512_srai_epi16(dst, 2);
}

static INLINE __m512i jnt_avg_round_avx512(const __m512i src, const __m512i offset) {
    const __m512i dst = _mm512_add_epi16(src, offset);
    return _mm512_srai_epi16(dst, 2);
}

static INLINE __m512i jnt_no_avg_round_avx512(const __m512i src, const __m512i offset) {
    const __m512i dst = _mm512_add_epi16(src, offset);
    return _mm512_srli_epi16(dst, 2);
}

static INLINE void pack_store_32_avx512(const __m512i res, uint8_t* const dst) {
    const __m256i r0 = _mm512_castsi512_si256(res);
    const __m256i r1 = _mm512_extracti64x4_epi64(res, 1);
    pack_store_32_avx2(r0, r1, dst);
}

static INLINE void xy_y_pack_store_32x2_avx512(const __m512i res0, const __m512i res1, uint8_t* const dst,
                                               const ptrdiff_t stride) {
    const __m512i t = _mm512_packus_epi16(res0, res1);
    const __m512i d = _mm512_permutexvar_epi64(_mm512_setr_epi64(0, 4, 2, 6, 1, 5, 3, 7), t);
    storeu_u8_32x2_avx512(d, dst, stride);
}

static INLINE void xy_y_pack_store_64_avx512(const __m512i res0, const __m512i res1, uint8_t* const dst) {
    const __m512i d = _mm512_packus_epi16(res0, res1);
    // d = _mm512_permute4x64_epi64(d, 0xD8);
    _mm512_storeu_si512((__m512i*)dst, d);
}

static INLINE void xy_y_round_store_32x2_avx512(const __m512i r0[2], const __m512i r1[2], uint8_t* const dst,
                                                const ptrdiff_t stride) {
    const __m512i ra = xy_y_round_32_avx512(r0);
    const __m512i rb = xy_y_round_32_avx512(r1);
    xy_y_pack_store_32x2_avx512(ra, rb, dst, stride);
}

static INLINE void xy_y_round_store_64_avx512(const __m512i r0[2], const __m512i r1[2], uint8_t* const dst) {
    const __m512i ra = xy_y_round_32_avx512(r0);
    const __m512i rb = xy_y_round_32_avx512(r1);
    xy_y_pack_store_64_avx512(ra, rb, dst);
}

static INLINE void convolve_store_32x2_avx512(const __m512i res0, const __m512i res1, uint8_t* const dst,
                                              const ptrdiff_t dst_stride) {
    const __m512i d = _mm512_packus_epi16(res0, res1);
    storeu_u8_32x2_avx512(d, dst, dst_stride);
}

static INLINE void convolve_store_64_avx512(const __m512i res0, const __m512i res1, uint8_t* const dst) {
    const __m512i d = _mm512_packus_epi16(res0, res1);
    _mm512_storeu_si512((__m512i*)dst, d);
}

static INLINE void jnt_no_avg_store_32x2_avx512(const __m512i src0, const __m512i src1, ConvBufType* const dst,
                                                const ptrdiff_t stride) {
    const __m512i d0 = _mm512_permutex2var_epi64(src0, _mm512_setr_epi64(0, 1, 8, 9, 2, 3, 10, 11), src1);
    const __m512i d1 = _mm512_permutex2var_epi64(src0, _mm512_setr_epi64(4, 5, 12, 13, 6, 7, 14, 15), src1);
    _mm512_storeu_si512((__m512i*)dst, d0);
    _mm512_storeu_si512((__m512i*)(dst + stride), d1);
}

static INLINE void jnt_no_avg_store_64_avx512(const __m512i src0, const __m512i src1, ConvBufType* const dst) {
    const __m512i d0 = _mm512_permutex2var_epi64(src0, _mm512_setr_epi64(0, 1, 8, 9, 2, 3, 10, 11), src1);
    const __m512i d1 = _mm512_permutex2var_epi64(src0, _mm512_setr_epi64(4, 5, 12, 13, 6, 7, 14, 15), src1);
    _mm512_storeu_si512((__m512i*)dst, d0);
    _mm512_storeu_si512((__m512i*)(dst + 32), d1);
}

static INLINE void x_convolve_2tap_32x2_avx512(const uint8_t* const src, const ptrdiff_t src_stride,
                                               const __m512i coeffs[1], __m512i r[2]) {
    const __m512i s0_256 = loadu_u8_32x2_avx512(src, src_stride);
    const __m512i s1_256 = loadu_u8_32x2_avx512(src + 1, src_stride);
    const __m512i ss0    = _mm512_unpacklo_epi8(s0_256, s1_256);
    const __m512i ss1    = _mm512_unpackhi_epi8(s0_256, s1_256);

    r[0] = convolve_2tap_avx512(&ss0, coeffs);
    r[1] = convolve_2tap_avx512(&ss1, coeffs);
}

static INLINE void x_convolve_2tap_64_avx512(const uint8_t* const src, const __m512i coeffs[1], __m512i r[2]) {
    const __m512i s0  = _mm512_loadu_si512((__m512i*)src);
    const __m512i s1  = _mm512_loadu_si512((__m512i*)(src + 1));
    const __m512i ss0 = _mm512_unpacklo_epi8(s0, s1);
    const __m512i ss1 = _mm512_unpackhi_epi8(s0, s1);

    r[0] = convolve_2tap_avx512(&ss0, coeffs);
    r[1] = convolve_2tap_avx512(&ss1, coeffs);
}

static INLINE __m512i x_convolve_4tap_avx512(const __m512i data, const __m512i coeffs[2], const __m512i filt[2]) {
    __m512i ss[2];

    ss[0] = _mm512_shuffle_epi8(data, filt[0]);
    ss[1] = _mm512_shuffle_epi8(data, filt[1]);

    return convolve_4tap_avx512(ss, coeffs);
}

static INLINE __m512i x_convolve_6tap_avx512(const __m512i data, const __m512i coeffs[3], const __m512i filt[3]) {
    __m512i ss[3];

    ss[0] = _mm512_shuffle_epi8(data, filt[0]);
    ss[1] = _mm512_shuffle_epi8(data, filt[1]);
    ss[2] = _mm512_shuffle_epi8(data, filt[2]);

    return convolve_6tap_avx512(ss, coeffs);
}

static INLINE __m512i x_convolve_8tap_avx512(const __m512i data, const __m512i coeffs[4], const __m512i filt[4]) {
    __m512i ss[4];

    ss[0] = _mm512_shuffle_epi8(data, filt[0]);
    ss[1] = _mm512_shuffle_epi8(data, filt[1]);
    ss[2] = _mm512_shuffle_epi8(data, filt[2]);
    ss[3] = _mm512_shuffle_epi8(data, filt[3]);

    return convolve_8tap_avx512(ss, coeffs);
}

static INLINE void x_convolve_6tap_32x2_avx512(const uint8_t* const src, const ptrdiff_t src_stride,
                                               const __m512i coeffs[3], const __m512i filt[3], __m512i r[2]) {
    const __m512i s0_512 = loadu_u8_32x2_avx512(src, src_stride);
    const __m512i s1_512 = loadu_u8_32x2_avx512(src + 8, src_stride);

    r[0] = x_convolve_6tap_avx512(s0_512, coeffs, filt);
    r[1] = x_convolve_6tap_avx512(s1_512, coeffs, filt);
}

static INLINE void x_convolve_6tap_64_avx512(const uint8_t* const src, const __m512i coeffs[3], const __m512i filt[3],
                                             __m512i r[2]) {
    const __m512i s0_512 = _mm512_loadu_si512((__m512i*)src);
    const __m512i s1_512 = _mm512_loadu_si512((__m512i*)(src + 8));

    r[0] = x_convolve_6tap_avx512(s0_512, coeffs, filt);
    r[1] = x_convolve_6tap_avx512(s1_512, coeffs, filt);
}

SIMD_INLINE void x_convolve_8tap_32x2_avx512(const uint8_t* const src, const ptrdiff_t src_stride,
                                             const __m512i coeffs[4], const __m512i filt[4], __m512i r[2]) {
    const __m512i s0_512 = loadu_u8_32x2_avx512(src, src_stride);
    const __m512i s1_512 = loadu_u8_32x2_avx512(src + 8, src_stride);

    r[0] = x_convolve_8tap_avx512(s0_512, coeffs, filt);
    r[1] = x_convolve_8tap_avx512(s1_512, coeffs, filt);
}

SIMD_INLINE void x_convolve_8tap_64_avx512(const uint8_t* const src, const __m512i coeffs[4], const __m512i filt[4],
                                           __m512i r[2]) {
    const __m512i s0_512 = _mm512_loadu_si512((__m512i*)src);
    const __m512i s1_512 = _mm512_loadu_si512((__m512i*)(src + 8));

    r[0] = x_convolve_8tap_avx512(s0_512, coeffs, filt);
    r[1] = x_convolve_8tap_avx512(s1_512, coeffs, filt);
}

static INLINE void y_convolve_2tap_32x2_avx512(const uint8_t* const src, const ptrdiff_t stride,
                                               const __m512i coeffs[1], __m256i* const s0_256, __m512i r[2]) {
    const __m256i s1_256 = _mm256_loadu_si256((__m256i*)src);
    const __m512i s0_512 = _mm512_setr_m256i(*s0_256, s1_256);
    *s0_256              = _mm256_loadu_si256((__m256i*)(src + stride));
    const __m512i s1_512 = _mm512_setr_m256i(s1_256, *s0_256);
    const __m512i ss0    = _mm512_unpacklo_epi8(s0_512, s1_512);
    const __m512i ss1    = _mm512_unpackhi_epi8(s0_512, s1_512);
    r[0]                 = convolve_2tap_avx512(&ss0, coeffs);
    r[1]                 = convolve_2tap_avx512(&ss1, coeffs);
}

static INLINE void y_convolve_2tap_64_avx512(const uint8_t* const src, const __m512i coeffs[1], const __m512i s0,
                                             __m512i* const s1, __m512i r[2]) {
    *s1               = _mm512_loadu_si512((__m512i*)src);
    const __m512i ss0 = _mm512_unpacklo_epi8(s0, *s1);
    const __m512i ss1 = _mm512_unpackhi_epi8(s0, *s1);
    r[0]              = convolve_2tap_avx512(&ss0, coeffs);
    r[1]              = convolve_2tap_avx512(&ss1, coeffs);
}

static INLINE void y_convolve_4tap_32x2_avx512(const uint8_t* const src, const ptrdiff_t stride,
                                               const __m512i coeffs[2], __m256i s_256[3], __m512i ss_512[4],
                                               __m512i r[2]) {
    s_256[1]             = _mm256_loadu_si256((__m256i*)(src + 1 * stride));
    const __m512i s0_512 = _mm512_setr_m256i(s_256[2], s_256[1]);
    s_256[2]             = _mm256_loadu_si256((__m256i*)(src + 2 * stride));
    const __m512i s1_512 = _mm512_setr_m256i(s_256[1], s_256[2]);
    ss_512[1]            = _mm512_unpacklo_epi8(s0_512, s1_512);
    ss_512[3]            = _mm512_unpackhi_epi8(s0_512, s1_512);
    r[0]                 = convolve_4tap_avx512(ss_512 + 0, coeffs);
    r[1]                 = convolve_4tap_avx512(ss_512 + 2, coeffs);
}

static INLINE void y_convolve_6tap_32x2_avx512(const uint8_t* const src, const ptrdiff_t stride,
                                               const __m512i coeffs[3], __m256i s_256[5], __m512i ss_512[6],
                                               __m512i r[2]) {
    s_256[3]             = _mm256_loadu_si256((__m256i*)(src + 3 * stride));
    const __m512i s0_512 = _mm512_setr_m256i(s_256[4], s_256[3]);
    s_256[4]             = _mm256_loadu_si256((__m256i*)(src + 4 * stride));
    const __m512i s1_512 = _mm512_setr_m256i(s_256[3], s_256[4]);
    ss_512[2]            = _mm512_unpacklo_epi8(s0_512, s1_512);
    ss_512[5]            = _mm512_unpackhi_epi8(s0_512, s1_512);
    r[0]                 = convolve_6tap_avx512(ss_512 + 0, coeffs);
    r[1]                 = convolve_6tap_avx512(ss_512 + 3, coeffs);
}

static INLINE void y_convolve_6tap_64x2_avx512(const uint8_t* const src, const ptrdiff_t stride,
                                               const __m512i coeffs[3], __m512i s_512[6], __m512i ss_512[6],
                                               __m512i tt_512[6], __m512i r[4]) {
    s_512[5]  = _mm512_loadu_si512((__m512i*)(src + 3 * stride));
    ss_512[2] = _mm512_unpacklo_epi8(s_512[4], s_512[5]);
    ss_512[5] = _mm512_unpackhi_epi8(s_512[4], s_512[5]);
    s_512[4]  = _mm512_loadu_si512((__m512i*)(src + 4 * stride));
    tt_512[2] = _mm512_unpacklo_epi8(s_512[5], s_512[4]);
    tt_512[5] = _mm512_unpackhi_epi8(s_512[5], s_512[4]);
    r[0]      = convolve_6tap_avx512(ss_512 + 0, coeffs);
    r[1]      = convolve_6tap_avx512(ss_512 + 3, coeffs);
    r[2]      = convolve_6tap_avx512(tt_512 + 0, coeffs);
    r[3]      = convolve_6tap_avx512(tt_512 + 3, coeffs);
}

static INLINE void y_convolve_8tap_32x2_avx512(const uint8_t* const src, const ptrdiff_t stride,
                                               const __m512i coeffs[4], __m256i s_256[7], __m512i ss_512[8],
                                               __m512i r[2]) {
    s_256[5]             = _mm256_loadu_si256((__m256i*)(src + 5 * stride));
    const __m512i s0_512 = _mm512_setr_m256i(s_256[6], s_256[5]);
    s_256[6]             = _mm256_loadu_si256((__m256i*)(src + 6 * stride));
    const __m512i s1_512 = _mm512_setr_m256i(s_256[5], s_256[6]);
    ss_512[3]            = _mm512_unpacklo_epi8(s0_512, s1_512);
    ss_512[7]            = _mm512_unpackhi_epi8(s0_512, s1_512);
    r[0]                 = convolve_8tap_avx512(ss_512 + 0, coeffs);
    r[1]                 = convolve_8tap_avx512(ss_512 + 4, coeffs);
}

static INLINE void y_convolve_8tap_64x2_avx512(const uint8_t* const src, const ptrdiff_t stride,
                                               const __m512i coeffs[4], __m512i s_512[8], __m512i ss_512[8],
                                               __m512i tt_512[8], __m512i r[4]) {
    s_512[7]  = _mm512_loadu_si512((__m512i*)(src + 7 * stride));
    ss_512[3] = _mm512_unpacklo_epi8(s_512[6], s_512[7]);
    ss_512[7] = _mm512_unpackhi_epi8(s_512[6], s_512[7]);
    s_512[6]  = _mm512_loadu_si512((__m512i*)(src + 8 * stride));
    tt_512[3] = _mm512_unpacklo_epi8(s_512[7], s_512[6]);
    tt_512[7] = _mm512_unpackhi_epi8(s_512[7], s_512[6]);
    r[0]      = convolve_8tap_avx512(ss_512 + 0, coeffs);
    r[1]      = convolve_8tap_avx512(ss_512 + 4, coeffs);
    r[2]      = convolve_8tap_avx512(tt_512 + 0, coeffs);
    r[3]      = convolve_8tap_avx512(tt_512 + 4, coeffs);
}

static INLINE void xy_x_convolve_2tap_32x2_avx512(const uint8_t* const src, const ptrdiff_t stride,
                                                  const __m512i coeffs[1], __m512i r[2]) {
    const __m512i s0  = loadu_u8_32x2_avx512(src, stride);
    const __m512i s1  = loadu_u8_32x2_avx512(src + 1, stride);
    const __m512i ss0 = _mm512_unpacklo_epi8(s0, s1);
    const __m512i ss1 = _mm512_unpackhi_epi8(s0, s1);

    r[0] = convolve_2tap_avx512(&ss0, coeffs);
    r[1] = convolve_2tap_avx512(&ss1, coeffs);
}

static INLINE void xy_x_convolve_2tap_64_avx512(const uint8_t* const src, const __m512i coeffs[1], __m512i r[2]) {
    const __m512i s0  = _mm512_loadu_si512((__m512i*)src);
    const __m512i s1  = _mm512_loadu_si512((__m512i*)(src + 1));
    const __m512i ss0 = _mm512_unpacklo_epi8(s0, s1);
    const __m512i ss1 = _mm512_unpackhi_epi8(s0, s1);

    r[0] = convolve_2tap_avx512(&ss0, coeffs);
    r[1] = convolve_2tap_avx512(&ss1, coeffs);
}

static INLINE void xy_x_2tap_32x2_avx512(const uint8_t* const src, const ptrdiff_t src_stride, const __m512i coeffs[1],
                                         int16_t* const dst) {
    __m512i r[2];

    xy_x_convolve_2tap_32x2_avx512(src, src_stride, coeffs, r);
    const __m512i d0 = xy_x_round_avx512(r[0]);
    const __m512i d1 = xy_x_round_avx512(r[1]);
    // const __m512i d0 = _mm512_inserti64x4(t0, _mm512_castsi512_si256(t1), 1);
    // const __m512i d1 = _mm512_inserti64x4(t1, _mm512_extracti64x4_epi64(t0,
    // 1), 0);
    zz_store_512(dst, d0);
    zz_store_512(dst + 32, d1);
}

static INLINE void xy_x_store_64_avx512(const __m512i d[2], int16_t* const dst) {
    const __m512i d0 = xy_x_round_avx512(d[0]);
    const __m512i d1 = xy_x_round_avx512(d[1]);
    // const __m512i d0 = _mm512_inserti64x4(t0, _mm512_castsi512_si256(t1), 1);
    // const __m512i d1 = _mm512_inserti64x4(t1, _mm512_extracti64x4_epi64(t0,
    // 1), 0);
    zz_store_512(dst, d0);
    zz_store_512(dst + 32, d1);
}

static INLINE void xy_x_2tap_64_avx512(const uint8_t* const src, const __m512i coeffs[1], int16_t* const dst) {
    __m512i d[2];

    xy_x_convolve_2tap_64_avx512(src, coeffs, d);
    xy_x_store_64_avx512(d, dst);
}

static INLINE void xy_x_6tap_32x2_avx512(const uint8_t* const src, const ptrdiff_t src_stride, const __m512i coeffs[3],
                                         const __m512i filt[3], int16_t* const dst) {
    __m512i d[2];

    x_convolve_6tap_32x2_avx512(src, src_stride, coeffs, filt, d);
    xy_x_store_64_avx512(d, dst);
}

static INLINE void xy_x_6tap_64_avx512(const uint8_t* const src, const __m512i coeffs[3], const __m512i filt[3],
                                       int16_t* const dst) {
    __m512i d[2];

    x_convolve_6tap_64_avx512(src, coeffs, filt, d);
    xy_x_store_64_avx512(d, dst);
}

static INLINE void xy_x_8tap_32x2_avx512(const uint8_t* const src, const ptrdiff_t src_stride, const __m512i coeffs[4],
                                         const __m512i filt[4], int16_t* const dst) {
    __m512i d[2];

    x_convolve_8tap_32x2_avx512(src, src_stride, coeffs, filt, d);
    xy_x_store_64_avx512(d, dst);
}

static INLINE void xy_x_8tap_64_avx512(const uint8_t* const src, const __m512i coeffs[4], const __m512i filt[4],
                                       int16_t* const dst) {
    __m512i d[2];

    x_convolve_8tap_64_avx512(src, coeffs, filt, d);
    xy_x_store_64_avx512(d, dst);
}

static INLINE void xy_y_convolve_2tap_32_avx512(const __m512i s0, const __m512i s1, const __m512i coeffs[1],
                                                __m512i r[2]) {
    const __m512i ss0 = _mm512_unpacklo_epi16(s0, s1);
    const __m512i ss1 = _mm512_unpackhi_epi16(s0, s1);
    r[0]              = convolve16_2tap_avx512(&ss0, coeffs);
    r[1]              = convolve16_2tap_avx512(&ss1, coeffs);
}

static INLINE void xy_y_convolve_2tap_32x2_avx512(const int16_t* const src, __m512i s[2], const __m512i coeffs[1],
                                                  __m512i r[4]) {
    s[1] = loadu_s16_16x2_avx512(src + 16, 32);
    xy_y_convolve_2tap_32_avx512(s[0], s[1], coeffs, r + 0);
    s[0] = loadu_s16_16x2_avx512(src + 64, 32);
    xy_y_convolve_2tap_32_avx512(s[1], s[0], coeffs, r + 2);
}

static INLINE void xy_y_convolve_2tap_64_avx512(const int16_t* const src, const __m512i s0[2], __m512i s1[2],
                                                const __m512i coeffs[1], __m512i r[4]) {
    s1[0] = zz_load_512(src);
    s1[1] = zz_load_512(src + 32);
    xy_y_convolve_2tap_32_avx512(s0[0], s1[0], coeffs, r + 0);
    xy_y_convolve_2tap_32_avx512(s0[1], s1[1], coeffs, r + 2);
}

static INLINE void xy_y_convolve_2tap_64_all_avx512(const int16_t* const src, const __m512i s0[2], __m512i s1[2],
                                                    const __m512i coeffs[1], uint8_t* const dst) {
    __m512i r[4];

    xy_y_convolve_2tap_64_avx512(src, s0, s1, coeffs, r);
    xy_y_round_store_64_avx512(r + 0, r + 2, dst);
}

static INLINE void xy_y_convolve_2tap_half_pel_32x2_avx512(const int16_t* const src, __m512i s[2], __m512i r[2]) {
    s[1] = loadu_s16_16x2_avx512(src, 32);
    r[0] = _mm512_add_epi16(s[0], s[1]);
    s[0] = loadu_s16_16x2_avx512(src + 48, 32);
    r[1] = _mm512_add_epi16(s[1], s[0]);
}

static INLINE void xy_y_convolve_2tap_half_pel_64_avx512(const int16_t* const src, const __m512i s0[2], __m512i s1[2],
                                                         __m512i r[2]) {
    s1[0] = zz_load_512(src);
    s1[1] = zz_load_512(src + 32);
    r[0]  = _mm512_add_epi16(s0[0], s1[0]);
    r[1]  = _mm512_add_epi16(s0[1], s1[1]);
}

static INLINE void xy_y_convolve_2tap_half_pel_64_all_avx512(const int16_t* const src, const __m512i s0[2],
                                                             __m512i s1[2], uint8_t* const dst) {
    __m512i r[2];

    xy_y_convolve_2tap_half_pel_64_avx512(src, s0, s1, r);
    r[0] = xy_y_round_half_pel_avx512(r[0]);
    r[1] = xy_y_round_half_pel_avx512(r[1]);
    xy_y_pack_store_64_avx512(r[0], r[1], dst);
}

static INLINE void xy_y_convolve_4tap_32_avx512(const __m512i ss[4], const __m512i coeffs[2], __m512i r[2]) {
    r[0] = convolve16_4tap_avx512(ss, coeffs);
    r[1] = convolve16_4tap_avx512(ss + 2, coeffs);
}

static INLINE void xy_y_convolve_4tap_width32x2_avx512(const int16_t* const src, __m512i s_512[4], __m512i ss_512[4],
                                                       __m512i tt_512[4], const __m512i coeffs[2], __m512i r[4]) {
    s_512[3]  = loadu_s16_16x2_avx512(src + 5 * 16, 32);
    ss_512[1] = _mm512_unpacklo_epi16(s_512[2], s_512[3]);
    ss_512[3] = _mm512_unpackhi_epi16(s_512[2], s_512[3]);
    s_512[2]  = loadu_s16_16x2_avx512(src + 8 * 16, 32);
    tt_512[1] = _mm512_unpacklo_epi16(s_512[3], s_512[2]);
    tt_512[3] = _mm512_unpackhi_epi16(s_512[3], s_512[2]);
    xy_y_convolve_4tap_32_avx512(ss_512, coeffs, r + 0);
    xy_y_convolve_4tap_32_avx512(tt_512, coeffs, r + 2);
    ss_512[0] = ss_512[1];
    ss_512[2] = ss_512[3];
    tt_512[0] = tt_512[1];
    tt_512[2] = tt_512[3];
}

static INLINE void xy_y_convolve_6tap_32_avx512(const __m512i ss[6], const __m512i coeffs[3], __m512i r[2]) {
    r[0] = convolve16_6tap_avx512(ss, coeffs);
    r[1] = convolve16_6tap_avx512(ss + 3, coeffs);
}

SIMD_INLINE void xy_y_convolve_6tap_width32x2_avx512(const int16_t* const src, const __m512i coeffs[3],
                                                     __m512i s_512[6], __m512i ss_512[6], __m512i tt_512[6],
                                                     __m512i r[4]) {
    s_512[5]  = loadu_s16_16x2_avx512(src + 9 * 16, 32);
    ss_512[2] = _mm512_unpacklo_epi16(s_512[4], s_512[5]);
    ss_512[5] = _mm512_unpackhi_epi16(s_512[4], s_512[5]);
    s_512[4]  = loadu_s16_16x2_avx512(src + 12 * 16, 32);
    tt_512[2] = _mm512_unpacklo_epi16(s_512[5], s_512[4]);
    tt_512[5] = _mm512_unpackhi_epi16(s_512[5], s_512[4]);

    xy_y_convolve_6tap_32_avx512(ss_512, coeffs, r + 0);
    xy_y_convolve_6tap_32_avx512(tt_512, coeffs, r + 2);

    ss_512[0] = ss_512[1];
    ss_512[1] = ss_512[2];
    ss_512[3] = ss_512[4];
    ss_512[4] = ss_512[5];

    tt_512[0] = tt_512[1];
    tt_512[1] = tt_512[2];
    tt_512[3] = tt_512[4];
    tt_512[4] = tt_512[5];
}

static INLINE void xy_y_convolve_6tap_32x2_avx512(const int16_t* const src, const ptrdiff_t stride, __m512i s_512[6],
                                                  __m512i ss_512[6], __m512i tt_512[6], const __m512i coeffs[3],
                                                  __m512i r[4]) {
    s_512[5]  = _mm512_loadu_si512((__m512i*)(src + 5 * stride));
    ss_512[2] = _mm512_unpacklo_epi16(s_512[4], s_512[5]);
    ss_512[5] = _mm512_unpackhi_epi16(s_512[4], s_512[5]);
    s_512[4]  = _mm512_loadu_si512((__m512i*)(src + 6 * stride));
    tt_512[2] = _mm512_unpacklo_epi16(s_512[5], s_512[4]);
    tt_512[5] = _mm512_unpackhi_epi16(s_512[5], s_512[4]);

    xy_y_convolve_6tap_32_avx512(ss_512, coeffs, r + 0);
    xy_y_convolve_6tap_32_avx512(tt_512, coeffs, r + 2);

    ss_512[0] = ss_512[1];
    ss_512[1] = ss_512[2];
    ss_512[3] = ss_512[4];
    ss_512[4] = ss_512[5];

    tt_512[0] = tt_512[1];
    tt_512[1] = tt_512[2];
    tt_512[3] = tt_512[4];
    tt_512[4] = tt_512[5];
}

SIMD_INLINE void xy_y_convolve_8tap_32_avx512(const __m512i* const ss, const __m512i coeffs[4], __m512i r[2]) {
    r[0] = convolve16_8tap_avx512(ss, coeffs);
    r[1] = convolve16_8tap_avx512(ss + 4, coeffs);
}

SIMD_INLINE void xy_y_convolve_8tap_width32x2_avx512(const int16_t* const src, const __m512i coeffs[4],
                                                     __m512i s_512[8], __m512i ss_512[8], __m512i tt_512[8],
                                                     __m512i r[4]) {
    s_512[7]  = loadu_s16_16x2_avx512(src + 13 * 16, 32);
    ss_512[3] = _mm512_unpacklo_epi16(s_512[6], s_512[7]);
    ss_512[7] = _mm512_unpackhi_epi16(s_512[6], s_512[7]);
    s_512[6]  = loadu_s16_16x2_avx512(src + 16 * 16, 32);
    tt_512[3] = _mm512_unpacklo_epi16(s_512[7], s_512[6]);
    tt_512[7] = _mm512_unpackhi_epi16(s_512[7], s_512[6]);

    xy_y_convolve_8tap_32_avx512(ss_512, coeffs, r + 0);
    xy_y_convolve_8tap_32_avx512(tt_512, coeffs, r + 2);

    ss_512[0] = ss_512[1];
    ss_512[1] = ss_512[2];
    ss_512[2] = ss_512[3];
    ss_512[4] = ss_512[5];
    ss_512[5] = ss_512[6];
    ss_512[6] = ss_512[7];

    tt_512[0] = tt_512[1];
    tt_512[1] = tt_512[2];
    tt_512[2] = tt_512[3];
    tt_512[4] = tt_512[5];
    tt_512[5] = tt_512[6];
    tt_512[6] = tt_512[7];
}

SIMD_INLINE void xy_y_convolve_8tap_32x2_avx512(const int16_t* const src, const ptrdiff_t stride,
                                                const __m512i coeffs[4], __m512i s_512[8], __m512i ss_512[8],
                                                __m512i tt_512[8], __m512i r[4]) {
    s_512[7]  = zz_load_512(src + 7 * stride);
    ss_512[3] = _mm512_unpacklo_epi16(s_512[6], s_512[7]);
    ss_512[7] = _mm512_unpackhi_epi16(s_512[6], s_512[7]);
    s_512[6]  = zz_load_512(src + 8 * stride);
    tt_512[3] = _mm512_unpacklo_epi16(s_512[7], s_512[6]);
    tt_512[7] = _mm512_unpackhi_epi16(s_512[7], s_512[6]);

    xy_y_convolve_8tap_32_avx512(ss_512, coeffs, r + 0);
    xy_y_convolve_8tap_32_avx512(tt_512, coeffs, r + 2);

    ss_512[0] = ss_512[1];
    ss_512[1] = ss_512[2];
    ss_512[2] = ss_512[3];
    ss_512[4] = ss_512[5];
    ss_512[5] = ss_512[6];
    ss_512[6] = ss_512[7];

    tt_512[0] = tt_512[1];
    tt_512[1] = tt_512[2];
    tt_512[2] = tt_512[3];
    tt_512[4] = tt_512[5];
    tt_512[5] = tt_512[6];
    tt_512[6] = tt_512[7];
}

static INLINE __m512i jnt_comp_avg_convolve_32_avx512(const __m512i res, const __m512i dst, const __m512i factor,
                                                      const __m512i offset) {
    __m512i d[2];

    d[0] = _mm512_unpacklo_epi16(dst, res);
    d[1] = _mm512_unpackhi_epi16(dst, res);
    d[0] = _mm512_madd_epi16(d[0], factor);
    d[1] = _mm512_madd_epi16(d[1], factor);
    d[0] = _mm512_add_epi32(d[0], offset);
    d[1] = _mm512_add_epi32(d[1], offset);
    d[0] = _mm512_srai_epi32(d[0], 8);
    d[1] = _mm512_srai_epi32(d[1], 8);
    return _mm512_packs_epi32(d[0], d[1]);
}

static INLINE void jnt_comp_avg_round_store_32_kernel_avx512(const __m512i res, const __m512i factor,
                                                             const __m512i offset, const ConvBufType* const dst,
                                                             uint8_t* const dst8) {
    __m512i d;

    d = _mm512_loadu_si512((__m512i*)dst);
    d = jnt_comp_avg_convolve_32_avx512(res, d, factor, offset);
    pack_store_32_avx512(d, dst8);
}

static INLINE void jnt_loadu_u16_8x4x2_avx512(const ConvBufType* const src, const ptrdiff_t stride, __m512i d[2]) {
    const __m512i s0 = _mm512_loadu_si512((__m512i*)(src + 0 * stride));
    const __m512i s1 = _mm512_loadu_si512((__m512i*)(src + 1 * stride));
    d[0]             = _mm512_permutex2var_epi64(s0, _mm512_setr_epi64(0, 1, 4, 5, 8, 9, 12, 13), s1);
    d[1]             = _mm512_permutex2var_epi64(s0, _mm512_setr_epi64(2, 3, 6, 7, 10, 11, 14, 15), s1);
}

SIMD_INLINE void jnt_comp_avg_round_store_32x2_avx512(const __m512i res[2], const __m512i factor, const __m512i offset,
                                                      const ConvBufType* const dst, const ptrdiff_t dst_stride,
                                                      uint8_t* const dst8, const ptrdiff_t dst8_stride) {
    __m512i r[2], d[2];

    r[0] = jnt_y_round_avx512(res[0]);
    r[1] = jnt_y_round_avx512(res[1]);
    jnt_loadu_u16_8x4x2_avx512(dst, dst_stride, d);
    d[0] = jnt_comp_avg_convolve_32_avx512(r[0], d[0], factor, offset);
    d[1] = jnt_comp_avg_convolve_32_avx512(r[1], d[1], factor, offset);
    convolve_store_32x2_avx512(d[0], d[1], dst8, dst8_stride);
}

SIMD_INLINE void jnt_comp_avg_round_store_64_avx512(const __m512i res[2], const __m512i factor, const __m512i offset,
                                                    const ConvBufType* const dst, uint8_t* const dst8) {
    __m512i r[2], d[2];

    r[0] = jnt_y_round_avx512(res[0]);
    r[1] = jnt_y_round_avx512(res[1]);
    jnt_loadu_u16_8x4x2_avx512(dst, 32, d);
    d[0] = jnt_comp_avg_convolve_32_avx512(r[0], d[0], factor, offset);
    d[1] = jnt_comp_avg_convolve_32_avx512(r[1], d[1], factor, offset);
    convolve_store_64_avx512(d[0], d[1], dst8);
}

static INLINE __m512i jnt_avg_32_avx512(const __m512i res, const __m512i dst) {
    const __m512i d = _mm512_add_epi16(res, dst);
    return _mm512_srai_epi16(d, 5);
}

static INLINE __m512i jnt_copy_load_src_32_avx512(const uint8_t* const src) {
    const __m256i s8  = _mm256_loadu_si256((__m256i*)src);
    const __m512i s16 = _mm512_cvtepu8_epi16(s8);
    return _mm512_slli_epi16(s16, LEFT_SHIFT);
}

static INLINE void jnt_copy_comp_avg_32_avx512(const uint8_t* const src, const __m512i factor_512,
                                               const __m512i offset_comp_avg_512, const ConvBufType* const dst,
                                               uint8_t* const dst8) {
    const __m512i res = jnt_copy_load_src_32_avx512(src);
    jnt_comp_avg_round_store_32_kernel_avx512(res, factor_512, offset_comp_avg_512, dst, dst8);
}

static INLINE void jnt_avg_round_store_32x2_avx512(const __m512i res[2], const __m512i offset,
                                                   const ConvBufType* const dst, const ptrdiff_t dst_stride,
                                                   uint8_t* const dst8, const ptrdiff_t dst8_stride) {
    __m512i r[2], d[2];

    r[0] = jnt_avg_round_avx512(res[0], offset);
    r[1] = jnt_avg_round_avx512(res[1], offset);
    jnt_loadu_u16_8x4x2_avx512(dst, dst_stride, d);
    d[0] = jnt_avg_32_avx512(r[0], d[0]);
    d[1] = jnt_avg_32_avx512(r[1], d[1]);
    convolve_store_32x2_avx512(d[0], d[1], dst8, dst8_stride);
}

static INLINE void jnt_avg_round_store_64_avx512(const __m512i res[2], const __m512i offset,
                                                 const ConvBufType* const dst, uint8_t* const dst8) {
    __m512i r[2], d[2];

    r[0] = jnt_avg_round_avx512(res[0], offset);
    r[1] = jnt_avg_round_avx512(res[1], offset);
    jnt_loadu_u16_8x4x2_avx512(dst, 32, d);
    d[0] = jnt_avg_32_avx512(r[0], d[0]);
    d[1] = jnt_avg_32_avx512(r[1], d[1]);
    convolve_store_64_avx512(d[0], d[1], dst8);
}

static INLINE void jnt_no_avg_round_store_32x2_avx512(const __m512i res[2], const __m512i offset,
                                                      ConvBufType* const dst, const ptrdiff_t dst_stride) {
    __m512i d[2];

    d[0] = jnt_no_avg_round_avx512(res[0], offset);
    d[1] = jnt_no_avg_round_avx512(res[1], offset);
    jnt_no_avg_store_32x2_avx512(d[0], d[1], dst, dst_stride);
}

static INLINE void jnt_no_avg_round_store_64_avx512(const __m512i res[2], const __m512i offset,
                                                    ConvBufType* const dst) {
    __m512i d[2];

    d[0] = jnt_no_avg_round_avx512(res[0], offset);
    d[1] = jnt_no_avg_round_avx512(res[1], offset);
    jnt_no_avg_store_64_avx512(d[0], d[1], dst);
}

#endif // EN_AVX512_SUPPORT

#endif // AOM_DSP_X86_CONVOLVE_AVX512_H_
