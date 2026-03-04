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
#include "common_dsp_rtcd.h"
#include "convolve.h"
#include "convolve_avx2.h"
#include "convolve_avx512.h"
#include "memory_sse4_1.h"

// TODO: When calculating conv_params->dst using AVX512 (such as those jnt
// convolve functions themselves and svt_av1_warp_affine etc.), always leave
// it twisted in memory, then when it is loaded as input, we don't need to
// twist it again to get what we want. In this way we eliminate permutation
// instructions in both sides (both storing and loading).
// This also applies to AVX2.

SIMD_INLINE void jnt_y_comp_avg_2tap_64_avx512(const uint8_t* const src, const __m512i* const coeffs,
                                               const __m512i factor, const __m512i offset, const __m512i s0,
                                               __m512i* const s1, const ConvBufType* const dst, uint8_t* const dst8) {
    __m512i r[2];

    y_convolve_2tap_64_avx512(src, coeffs, s0, s1, r);
    jnt_comp_avg_round_store_64_avx512(r, factor, offset, dst, dst8);
}

SIMD_INLINE void jnt_y_comp_avg_2tap_32x2_avx512(const uint8_t* const src, const int32_t src_stride,
                                                 const __m512i* const coeffs, const __m512i factor,
                                                 const __m512i offset, __m256i* const s, const ConvBufType* const dst,
                                                 const int32_t dst_stride, uint8_t* const dst8,
                                                 const int32_t dst8_stride) {
    __m512i r[2];

    y_convolve_2tap_32x2_avx512(src, src_stride, coeffs, s, r);
    jnt_comp_avg_round_store_32x2_avx512(r, factor, offset, dst, dst_stride, dst8, dst8_stride);
}

static INLINE void jnt_y_avg_2tap_32x2_avx512(const uint8_t* const src, const int32_t src_stride,
                                              const __m512i* const coeffs, const __m512i offset, __m256i* const s,
                                              const ConvBufType* const dst, const int32_t dst_stride,
                                              uint8_t* const dst8, const int32_t dst8_stride) {
    __m512i r[2];

    y_convolve_2tap_32x2_avx512(src, src_stride, coeffs, s, r);
    jnt_avg_round_store_32x2_avx512(r, offset, dst, dst_stride, dst8, dst8_stride);
}

static INLINE void jnt_y_avg_2tap_64_avx512(const uint8_t* const src, const __m512i* const coeffs, const __m512i offset,
                                            const __m512i s0, __m512i* const s1, const ConvBufType* const dst,
                                            uint8_t* const dst8) {
    __m512i r[2];

    y_convolve_2tap_64_avx512(src, coeffs, s0, s1, r);
    jnt_avg_round_store_64_avx512(r, offset, dst, dst8);
}

static INLINE void jnt_y_no_avg_2tap_32x2_avx512(const uint8_t* const src, const int32_t src_stride,
                                                 const __m512i* const coeffs, const __m512i offset, __m256i* const s,
                                                 ConvBufType* const dst, const int32_t dst_stride) {
    __m512i r[2];

    y_convolve_2tap_32x2_avx512(src, src_stride, coeffs, s, r);
    jnt_no_avg_round_store_32x2_avx512(r, offset, dst, dst_stride);
}

static INLINE void jnt_y_no_avg_2tap_64_avx512(const uint8_t* const src, const __m512i* const coeffs,
                                               const __m512i offset, const __m512i s0, __m512i* const s1,
                                               ConvBufType* const dst) {
    __m512i r[2];

    y_convolve_2tap_64_avx512(src, coeffs, s0, s1, r);
    jnt_no_avg_round_store_64_avx512(r, offset, dst);
}

static void jnt_convolve_y_2tap_avx512(const uint8_t* const src, const int32_t src_stride, uint8_t* dst8,
                                       const int32_t dst8_stride, const int32_t w, const int32_t h,
                                       const InterpFilterParams* const filter_params_y, const int32_t subpel_y_q4,
                                       const ConvolveParams* const conv_params) {
    const uint8_t* src_ptr      = src;
    const int32_t  dst_stride   = conv_params->dst_stride;
    const int32_t  round_0      = 3;
    const int32_t  round_1      = COMPOUND_ROUND1_BITS;
    const int32_t  bits         = FILTER_BITS - round_0;
    const int32_t  bd           = 8;
    const int32_t  round_bits   = 2 * FILTER_BITS - round_0 - round_1;
    const int32_t  offset_bits  = bd + round_bits;
    const int32_t  round_offset = (1 << offset_bits) + (1 << (offset_bits - 1));
    ConvBufType*   dst          = conv_params->dst;
    int32_t        y            = h;
    __m128i        coeffs_128[4];
    __m256i        coeffs_256[4];
    __m512i        coeffs_512[4];

    if (conv_params->do_average) {
        if (conv_params->use_jnt_comp_avg) {
            const int32_t factor          = conv_params->fwd_offset | (conv_params->bck_offset << 16);
            const int32_t offset_comp_avg = round_offset * conv_params->bck_offset +
                (1 << (round_bits + DIST_PRECISION_BITS - 1)) - (round_offset << DIST_PRECISION_BITS);

            if (w <= 4) {
                const __m128i factor_128          = _mm_set1_epi32(factor);
                const __m128i offset_comp_avg_128 = _mm_set1_epi32(offset_comp_avg);

                prepare_half_coeffs_2tap_ssse3(filter_params_y, subpel_y_q4, coeffs_128);

                if (w == 2) {
                    __m128i s_16[2];

                    s_16[0] = _mm_cvtsi32_si128(*(int16_t*)src_ptr);

                    do {
                        const __m128i res = y_convolve_2tap_2x2_ssse3(src_ptr, src_stride, coeffs_128, s_16);
                        jnt_comp_avg_round_store_2x2_sse2(
                            res, factor_128, offset_comp_avg_128, dst, dst_stride, dst8, dst8_stride);
                        src_ptr += 2 * src_stride;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                } else {
                    __m128i s_32[2];

                    assert(w == 4);

                    s_32[0] = _mm_cvtsi32_si128(*(int32_t*)src_ptr);

                    do {
                        const __m128i res = y_convolve_2tap_4x2_ssse3(src_ptr, src_stride, coeffs_128, s_32);
                        jnt_comp_avg_round_store_4x2_sse2(
                            res, factor_128, offset_comp_avg_128, dst, dst_stride, dst8, dst8_stride);
                        src_ptr += 2 * src_stride;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                }
            } else if (w <= 16) {
                const __m256i factor_256          = _mm256_set1_epi32(factor);
                const __m256i offset_comp_avg_256 = _mm256_set1_epi32(offset_comp_avg);

                prepare_half_coeffs_2tap_avx2(filter_params_y, subpel_y_q4, coeffs_256);

                if (w == 8) {
                    __m128i s_64[2];

                    s_64[0] = _mm_loadl_epi64((__m128i*)src_ptr);

                    do {
                        const __m256i res = y_convolve_2tap_8x2_avx2(src_ptr, src_stride, coeffs_256, s_64);
                        jnt_comp_avg_round_store_8x2_avx2(
                            res, factor_256, offset_comp_avg_256, dst, dst_stride, dst8, dst8_stride);
                        src_ptr += 2 * src_stride;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                } else {
                    __m128i s_128[2];
                    __m256i r[2];

                    assert(w == 16);

                    s_128[0] = _mm_loadu_si128((__m128i*)src_ptr);

                    do {
                        y_convolve_2tap_16x2_avx2(src_ptr, src_stride, coeffs_256, s_128, r);
                        jnt_comp_avg_round_store_16x2_avx2(
                            r, factor_256, offset_comp_avg_256, dst, dst_stride, dst8, dst8_stride);
                        src_ptr += 2 * src_stride;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                }
            } else {
                const __m512i factor_512          = _mm512_set1_epi32(factor);
                const __m512i offset_comp_avg_512 = _mm512_set1_epi32(offset_comp_avg);

                prepare_half_coeffs_2tap_avx512(filter_params_y, subpel_y_q4, coeffs_512);

                if (w == 32) {
                    __m256i s_256[2];

                    s_256[0] = _mm256_loadu_si256((__m256i*)src_ptr);

                    do {
                        jnt_y_comp_avg_2tap_32x2_avx512(src_ptr + src_stride,
                                                        src_stride,
                                                        coeffs_512,
                                                        factor_512,
                                                        offset_comp_avg_512,
                                                        s_256,
                                                        dst,
                                                        dst_stride,
                                                        dst8,
                                                        dst8_stride);
                        src_ptr += 2 * src_stride;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                } else if (w == 64) {
                    __m512i s_512[2];

                    s_512[0] = _mm512_loadu_si512((__m512i*)src_ptr);

                    do {
                        jnt_y_comp_avg_2tap_64_avx512(src_ptr + src_stride,
                                                      coeffs_512,
                                                      factor_512,
                                                      offset_comp_avg_512,
                                                      s_512[0],
                                                      &s_512[1],
                                                      dst,
                                                      dst8);
                        jnt_y_comp_avg_2tap_64_avx512(src_ptr + 2 * src_stride,
                                                      coeffs_512,
                                                      factor_512,
                                                      offset_comp_avg_512,
                                                      s_512[1],
                                                      &s_512[0],
                                                      dst + dst_stride,
                                                      dst8 + dst8_stride);

                        src_ptr += 2 * src_stride;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                } else {
                    __m512i s_512[2][2];

                    assert(w == 128);

                    s_512[0][0] = _mm512_loadu_si512((__m512i*)(src_ptr + 0 * 64));
                    s_512[0][1] = _mm512_loadu_si512((__m512i*)(src_ptr + 1 * 64));

                    do {
                        jnt_y_comp_avg_2tap_64_avx512(src_ptr + src_stride,
                                                      coeffs_512,
                                                      factor_512,
                                                      offset_comp_avg_512,
                                                      s_512[0][0],
                                                      &s_512[1][0],
                                                      dst,
                                                      dst8);
                        jnt_y_comp_avg_2tap_64_avx512(src_ptr + src_stride + 64,
                                                      coeffs_512,
                                                      factor_512,
                                                      offset_comp_avg_512,
                                                      s_512[0][1],
                                                      &s_512[1][1],
                                                      dst + 64,
                                                      dst8 + 64);
                        jnt_y_comp_avg_2tap_64_avx512(src_ptr + 2 * src_stride,
                                                      coeffs_512,
                                                      factor_512,
                                                      offset_comp_avg_512,
                                                      s_512[1][0],
                                                      &s_512[0][0],
                                                      dst + dst_stride,
                                                      dst8 + dst8_stride);
                        jnt_y_comp_avg_2tap_64_avx512(src_ptr + 2 * src_stride + 64,
                                                      coeffs_512,
                                                      factor_512,
                                                      offset_comp_avg_512,
                                                      s_512[1][1],
                                                      &s_512[0][1],
                                                      dst + dst_stride + 64,
                                                      dst8 + dst8_stride + 64);

                        src_ptr += 2 * src_stride;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                }
            }
        } else {
            const int16_t offset_avg = (1 << (FILTER_BITS - 1)) + (1 << (round_1 - bits - 2)) -
                (round_offset << (round_1 - bits - 1));

            if (w <= 4) {
                const __m128i offset_avg_128 = _mm_set1_epi16(offset_avg);

                prepare_half_coeffs_2tap_ssse3(filter_params_y, subpel_y_q4, coeffs_128);

                if (w == 2) {
                    __m128i s_16[2];

                    s_16[0] = _mm_cvtsi32_si128(*(int16_t*)src_ptr);

                    do {
                        const __m128i res = y_convolve_2tap_2x2_ssse3(src_ptr, src_stride, coeffs_128, s_16);
                        jnt_avg_round_store_2x2_sse2(res, offset_avg_128, dst, dst_stride, dst8, dst8_stride);
                        src_ptr += 2 * src_stride;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                } else {
                    __m128i s_32[2];

                    assert(w == 4);

                    s_32[0] = _mm_cvtsi32_si128(*(int32_t*)src_ptr);

                    do {
                        const __m128i res = y_convolve_2tap_4x2_ssse3(src_ptr, src_stride, coeffs_128, s_32);
                        jnt_avg_round_store_4x2_sse2(res, offset_avg_128, dst, dst_stride, dst8, dst8_stride);
                        src_ptr += 2 * src_stride;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                }
            } else if (w <= 16) {
                const __m256i offset_avg_256 = _mm256_set1_epi16(offset_avg);

                prepare_half_coeffs_2tap_avx2(filter_params_y, subpel_y_q4, coeffs_256);

                if (w == 8) {
                    __m128i s_64[2];

                    s_64[0] = _mm_loadl_epi64((__m128i*)src_ptr);

                    do {
                        const __m256i res = y_convolve_2tap_8x2_avx2(src_ptr, src_stride, coeffs_256, s_64);
                        jnt_avg_round_store_8x2_avx2(res, offset_avg_256, dst, dst_stride, dst8, dst8_stride);
                        src_ptr += 2 * src_stride;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                } else {
                    __m128i s_128[2];
                    __m256i r[2];

                    assert(w == 16);

                    s_128[0] = _mm_loadu_si128((__m128i*)src_ptr);

                    do {
                        y_convolve_2tap_16x2_avx2(src_ptr, src_stride, coeffs_256, s_128, r);
                        jnt_avg_round_store_16x2_avx2(r, offset_avg_256, dst, dst_stride, dst8, dst8_stride);
                        src_ptr += 2 * src_stride;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                }
            } else {
                const __m512i offset_avg_512 = _mm512_set1_epi16(offset_avg);

                prepare_half_coeffs_2tap_avx512(filter_params_y, subpel_y_q4, coeffs_512);

                if (w == 32) {
                    __m256i s_256;

                    s_256 = _mm256_loadu_si256((__m256i*)src_ptr);

                    do {
                        jnt_y_avg_2tap_32x2_avx512(src_ptr + src_stride,
                                                   src_stride,
                                                   coeffs_512,
                                                   offset_avg_512,
                                                   &s_256,
                                                   dst,
                                                   dst_stride,
                                                   dst8,
                                                   dst8_stride);
                        src_ptr += 2 * src_stride;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                } else if (w == 64) {
                    __m512i s_512[2];

                    s_512[0] = _mm512_loadu_si512((__m512i*)src_ptr);

                    do {
                        jnt_y_avg_2tap_64_avx512(
                            src_ptr + src_stride, coeffs_512, offset_avg_512, s_512[0], &s_512[1], dst, dst8);
                        jnt_y_avg_2tap_64_avx512(src_ptr + 2 * src_stride,
                                                 coeffs_512,
                                                 offset_avg_512,
                                                 s_512[1],
                                                 &s_512[0],
                                                 dst + dst_stride,
                                                 dst8 + dst8_stride);

                        src_ptr += 2 * src_stride;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                } else {
                    __m512i s_512[2][2];

                    assert(w == 128);

                    s_512[0][0] = _mm512_loadu_si512((__m512i*)(src_ptr + 0 * 64));
                    s_512[0][1] = _mm512_loadu_si512((__m512i*)(src_ptr + 1 * 64));

                    do {
                        jnt_y_avg_2tap_64_avx512(
                            src_ptr + src_stride, coeffs_512, offset_avg_512, s_512[0][0], &s_512[1][0], dst, dst8);
                        jnt_y_avg_2tap_64_avx512(src_ptr + src_stride + 64,
                                                 coeffs_512,
                                                 offset_avg_512,
                                                 s_512[0][1],
                                                 &s_512[1][1],
                                                 dst + 64,
                                                 dst8 + 64);
                        jnt_y_avg_2tap_64_avx512(src_ptr + 2 * src_stride,
                                                 coeffs_512,
                                                 offset_avg_512,
                                                 s_512[1][0],
                                                 &s_512[0][0],
                                                 dst + dst_stride,
                                                 dst8 + dst8_stride);
                        jnt_y_avg_2tap_64_avx512(src_ptr + 2 * src_stride + 64,
                                                 coeffs_512,
                                                 offset_avg_512,
                                                 s_512[1][1],
                                                 &s_512[0][1],
                                                 dst + dst_stride + 64,
                                                 dst8 + dst8_stride + 64);

                        src_ptr += 2 * src_stride;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                }
            }
        }
    } else {
        const int16_t offset_no_avg = (round_offset << (round_1 - bits - 1)) + (1 << (round_1 - bits - 2));

        if (w <= 4) {
            const __m128i offset_no_avg_128 = _mm_set1_epi16(offset_no_avg);

            prepare_half_coeffs_2tap_ssse3(filter_params_y, subpel_y_q4, coeffs_128);

            if (w == 2) {
                __m128i s_16[2];

                s_16[0] = _mm_cvtsi32_si128(*(int16_t*)src_ptr);

                do {
                    const __m128i res = y_convolve_2tap_2x2_ssse3(src_ptr, src_stride, coeffs_128, s_16);
                    jnt_no_avg_round_store_2x2_sse2(res, offset_no_avg_128, dst, dst_stride);
                    src_ptr += 2 * src_stride;
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            } else {
                __m128i s_32[2];

                assert(w == 4);

                s_32[0] = _mm_cvtsi32_si128(*(int32_t*)src_ptr);

                do {
                    const __m128i res = y_convolve_2tap_4x2_ssse3(src_ptr, src_stride, coeffs_128, s_32);
                    jnt_no_avg_round_store_4x2_sse2(res, offset_no_avg_128, dst, dst_stride);
                    src_ptr += 2 * src_stride;
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            }
        } else if (w <= 16) {
            const __m256i offset_no_avg_256 = _mm256_set1_epi16(offset_no_avg);

            prepare_half_coeffs_2tap_avx2(filter_params_y, subpel_y_q4, coeffs_256);

            if (w == 8) {
                __m128i s_64[2];

                s_64[0] = _mm_loadl_epi64((__m128i*)src_ptr);

                do {
                    const __m256i res = y_convolve_2tap_8x2_avx2(src_ptr, src_stride, coeffs_256, s_64);
                    jnt_no_avg_round_store_8x2_avx2(res, offset_no_avg_256, dst, dst_stride);
                    src_ptr += 2 * src_stride;
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            } else {
                __m128i s_128[2];
                __m256i r[2];

                assert(w == 16);

                s_128[0] = _mm_loadu_si128((__m128i*)src_ptr);

                do {
                    y_convolve_2tap_16x2_avx2(src_ptr, src_stride, coeffs_256, s_128, r);
                    jnt_no_avg_round_store_16x2_avx2(r, offset_no_avg_256, dst, dst_stride);
                    src_ptr += 2 * src_stride;
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            }
        } else {
            const __m512i offset_no_avg_512 = _mm512_set1_epi16(offset_no_avg);

            prepare_half_coeffs_2tap_avx512(filter_params_y, subpel_y_q4, coeffs_512);

            if (w == 32) {
                __m256i s_256 = _mm256_loadu_si256((__m256i*)src_ptr);

                do {
                    jnt_y_no_avg_2tap_32x2_avx512(
                        src_ptr + src_stride, src_stride, coeffs_512, offset_no_avg_512, &s_256, dst, dst_stride);
                    src_ptr += 2 * src_stride;
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            } else if (w == 64) {
                __m512i s_512[2];

                s_512[0] = _mm512_loadu_si512((__m512i*)src_ptr);

                do {
                    jnt_y_no_avg_2tap_64_avx512(
                        src_ptr + src_stride, coeffs_512, offset_no_avg_512, s_512[0], &s_512[1], dst);
                    jnt_y_no_avg_2tap_64_avx512(
                        src_ptr + 2 * src_stride, coeffs_512, offset_no_avg_512, s_512[1], &s_512[0], dst + dst_stride);

                    src_ptr += 2 * src_stride;
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            } else {
                __m512i s_512[2][2];

                assert(w == 128);

                s_512[0][0] = _mm512_loadu_si512((__m512i*)(src_ptr + 0 * 64));
                s_512[0][1] = _mm512_loadu_si512((__m512i*)(src_ptr + 1 * 64));

                do {
                    jnt_y_no_avg_2tap_64_avx512(
                        src_ptr + src_stride, coeffs_512, offset_no_avg_512, s_512[0][0], &s_512[1][0], dst);
                    jnt_y_no_avg_2tap_64_avx512(
                        src_ptr + src_stride + 64, coeffs_512, offset_no_avg_512, s_512[0][1], &s_512[1][1], dst + 64);
                    jnt_y_no_avg_2tap_64_avx512(src_ptr + 2 * src_stride,
                                                coeffs_512,
                                                offset_no_avg_512,
                                                s_512[1][0],
                                                &s_512[0][0],
                                                dst + dst_stride);
                    jnt_y_no_avg_2tap_64_avx512(src_ptr + 2 * src_stride + 64,
                                                coeffs_512,
                                                offset_no_avg_512,
                                                s_512[1][1],
                                                &s_512[0][1],
                                                dst + dst_stride + 64);

                    src_ptr += 2 * src_stride;
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            }
        }
    }
}

static void jnt_convolve_y_6tap_avx512(const uint8_t* const src, const int32_t src_stride, uint8_t* dst8,
                                       const int32_t dst8_stride, const int32_t w, const int32_t h,
                                       const InterpFilterParams* const filter_params_y, const int32_t subpel_y_q4,
                                       const ConvolveParams* const conv_params) {
    const uint8_t* src_ptr      = src - 2 * src_stride;
    const int32_t  dst_stride   = conv_params->dst_stride;
    const int32_t  round_0      = 3;
    const int32_t  round_1      = COMPOUND_ROUND1_BITS;
    const int32_t  bits         = FILTER_BITS - round_0;
    const int32_t  bd           = 8;
    const int32_t  round_bits   = 2 * FILTER_BITS - round_0 - round_1;
    const int32_t  offset_bits  = bd + round_bits;
    const int32_t  round_offset = (1 << offset_bits) + (1 << (offset_bits - 1));
    ConvBufType*   dst          = conv_params->dst;
    int32_t        x;
    int32_t        y = h;
    __m128i        coeffs_128[4];
    __m256i        coeffs_256[4];
    __m512i        coeffs_512[4];

    if (conv_params->do_average) {
        if (conv_params->use_jnt_comp_avg) {
            const int32_t factor          = conv_params->fwd_offset | (conv_params->bck_offset << 16);
            const int32_t offset_comp_avg = round_offset * conv_params->bck_offset +
                (1 << (round_bits + DIST_PRECISION_BITS - 1)) - (round_offset << DIST_PRECISION_BITS);

            if (w <= 4) {
                const __m128i factor_128          = _mm_set1_epi32(factor);
                const __m128i offset_comp_avg_128 = _mm_set1_epi32(offset_comp_avg);

                prepare_half_coeffs_6tap_ssse3(filter_params_y, subpel_y_q4, coeffs_128);

                y = h;

                if (w == 2) {
                    __m128i s_16[6], ss_128[3];

                    s_16[0] = _mm_cvtsi32_si128(*(int16_t*)(src_ptr + 0 * src_stride));
                    s_16[1] = _mm_cvtsi32_si128(*(int16_t*)(src_ptr + 1 * src_stride));
                    s_16[2] = _mm_cvtsi32_si128(*(int16_t*)(src_ptr + 2 * src_stride));
                    s_16[3] = _mm_cvtsi32_si128(*(int16_t*)(src_ptr + 3 * src_stride));
                    s_16[4] = _mm_cvtsi32_si128(*(int16_t*)(src_ptr + 4 * src_stride));

                    const __m128i src01 = _mm_unpacklo_epi16(s_16[0], s_16[1]);
                    const __m128i src12 = _mm_unpacklo_epi16(s_16[1], s_16[2]);
                    const __m128i src23 = _mm_unpacklo_epi16(s_16[2], s_16[3]);
                    const __m128i src34 = _mm_unpacklo_epi16(s_16[3], s_16[4]);

                    ss_128[0] = _mm_unpacklo_epi8(src01, src12);
                    ss_128[1] = _mm_unpacklo_epi8(src23, src34);

                    do {
                        src_ptr += 2 * src_stride;
                        const __m128i res = y_convolve_6tap_2x2_ssse3(src_ptr, src_stride, coeffs_128, s_16, ss_128);
                        jnt_comp_avg_round_store_2x2_sse2(
                            res, factor_128, offset_comp_avg_128, dst, dst_stride, dst8, dst8_stride);
                        ss_128[0] = ss_128[1];
                        ss_128[1] = ss_128[2];
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                } else {
                    __m128i s_32[6], ss_128[3];

                    assert(w == 4);

                    s_32[0] = _mm_cvtsi32_si128(*(int32_t*)(src_ptr + 0 * src_stride));
                    s_32[1] = _mm_cvtsi32_si128(*(int32_t*)(src_ptr + 1 * src_stride));
                    s_32[2] = _mm_cvtsi32_si128(*(int32_t*)(src_ptr + 2 * src_stride));
                    s_32[3] = _mm_cvtsi32_si128(*(int32_t*)(src_ptr + 3 * src_stride));
                    s_32[4] = _mm_cvtsi32_si128(*(int32_t*)(src_ptr + 4 * src_stride));

                    const __m128i src01 = _mm_unpacklo_epi32(s_32[0], s_32[1]);
                    const __m128i src12 = _mm_unpacklo_epi32(s_32[1], s_32[2]);
                    const __m128i src23 = _mm_unpacklo_epi32(s_32[2], s_32[3]);
                    const __m128i src34 = _mm_unpacklo_epi32(s_32[3], s_32[4]);

                    ss_128[0] = _mm_unpacklo_epi8(src01, src12);
                    ss_128[1] = _mm_unpacklo_epi8(src23, src34);

                    do {
                        src_ptr += 2 * src_stride;
                        const __m128i res = y_convolve_6tap_4x2_ssse3(src_ptr, src_stride, coeffs_128, s_32, ss_128);
                        jnt_comp_avg_round_store_4x2_sse2(
                            res, factor_128, offset_comp_avg_128, dst, dst_stride, dst8, dst8_stride);
                        ss_128[0] = ss_128[1];
                        ss_128[1] = ss_128[2];
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                }
            } else if (w <= 16) {
                const __m256i factor_256          = _mm256_set1_epi32(factor);
                const __m256i offset_comp_avg_256 = _mm256_set1_epi32(offset_comp_avg);

                prepare_half_coeffs_6tap_avx2(filter_params_y, subpel_y_q4, coeffs_256);

                if (w == 8) {
                    __m128i s_64[6];
                    __m256i ss_256[3];

                    s_64[0] = _mm_loadl_epi64((__m128i*)(src_ptr + 0 * src_stride));
                    s_64[1] = _mm_loadl_epi64((__m128i*)(src_ptr + 1 * src_stride));
                    s_64[2] = _mm_loadl_epi64((__m128i*)(src_ptr + 2 * src_stride));
                    s_64[3] = _mm_loadl_epi64((__m128i*)(src_ptr + 3 * src_stride));
                    s_64[4] = _mm_loadl_epi64((__m128i*)(src_ptr + 4 * src_stride));

                    // Load lines a and b. Line a to lower 128, line b to upper
                    // 128
                    const __m256i src01 = _mm256_setr_m128i(s_64[0], s_64[1]);
                    const __m256i src12 = _mm256_setr_m128i(s_64[1], s_64[2]);
                    const __m256i src23 = _mm256_setr_m128i(s_64[2], s_64[3]);
                    const __m256i src34 = _mm256_setr_m128i(s_64[3], s_64[4]);

                    ss_256[0] = _mm256_unpacklo_epi8(src01, src12);
                    ss_256[1] = _mm256_unpacklo_epi8(src23, src34);

                    y = h;
                    do {
                        src_ptr += 2 * src_stride;
                        const __m256i res = y_convolve_6tap_8x2_avx2(src_ptr, src_stride, coeffs_256, s_64, ss_256);
                        jnt_comp_avg_round_store_8x2_avx2(
                            res, factor_256, offset_comp_avg_256, dst, dst_stride, dst8, dst8_stride);
                        ss_256[0] = ss_256[1];
                        ss_256[1] = ss_256[2];
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                } else {
                    __m128i s_128[6];
                    __m256i ss_256[6], r[2];

                    assert(w == 16);

                    s_128[0] = _mm_loadu_si128((__m128i*)(src_ptr + 0 * src_stride));
                    s_128[1] = _mm_loadu_si128((__m128i*)(src_ptr + 1 * src_stride));
                    s_128[2] = _mm_loadu_si128((__m128i*)(src_ptr + 2 * src_stride));
                    s_128[3] = _mm_loadu_si128((__m128i*)(src_ptr + 3 * src_stride));
                    s_128[4] = _mm_loadu_si128((__m128i*)(src_ptr + 4 * src_stride));

                    // Load lines a and b. Line a to lower 128, line b to upper
                    // 128
                    const __m256i src01 = _mm256_setr_m128i(s_128[0], s_128[1]);
                    const __m256i src12 = _mm256_setr_m128i(s_128[1], s_128[2]);
                    const __m256i src23 = _mm256_setr_m128i(s_128[2], s_128[3]);
                    const __m256i src34 = _mm256_setr_m128i(s_128[3], s_128[4]);

                    ss_256[0] = _mm256_unpacklo_epi8(src01, src12);
                    ss_256[1] = _mm256_unpacklo_epi8(src23, src34);

                    ss_256[3] = _mm256_unpackhi_epi8(src01, src12);
                    ss_256[4] = _mm256_unpackhi_epi8(src23, src34);

                    y = h;
                    do {
                        src_ptr += 2 * src_stride;
                        y_convolve_6tap_16x2_avx2(src_ptr, src_stride, coeffs_256, s_128, ss_256, r);
                        jnt_comp_avg_round_store_16x2_avx2(
                            r, factor_256, offset_comp_avg_256, dst, dst_stride, dst8, dst8_stride);
                        ss_256[0] = ss_256[1];
                        ss_256[1] = ss_256[2];
                        ss_256[3] = ss_256[4];
                        ss_256[4] = ss_256[5];
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                }
            } else {
                const __m512i factor_512          = _mm512_set1_epi32(factor);
                const __m512i offset_comp_avg_512 = _mm512_set1_epi32(offset_comp_avg);

                prepare_half_coeffs_6tap_avx512(filter_params_y, subpel_y_q4, coeffs_512);

                if (w == 32) {
                    __m256i s_256[5];
                    __m512i s_512[4], ss_512[6], r[2];

                    s_256[0] = _mm256_loadu_si256((__m256i*)(src_ptr + 0 * src_stride));
                    s_256[1] = _mm256_loadu_si256((__m256i*)(src_ptr + 1 * src_stride));
                    s_256[2] = _mm256_loadu_si256((__m256i*)(src_ptr + 2 * src_stride));
                    s_256[3] = _mm256_loadu_si256((__m256i*)(src_ptr + 3 * src_stride));
                    s_256[4] = _mm256_loadu_si256((__m256i*)(src_ptr + 4 * src_stride));

                    s_512[0] = _mm512_setr_m256i(s_256[0], s_256[1]);
                    s_512[1] = _mm512_setr_m256i(s_256[1], s_256[2]);
                    s_512[2] = _mm512_setr_m256i(s_256[2], s_256[3]);
                    s_512[3] = _mm512_setr_m256i(s_256[3], s_256[4]);

                    ss_512[0] = _mm512_unpacklo_epi8(s_512[0], s_512[1]);
                    ss_512[1] = _mm512_unpacklo_epi8(s_512[2], s_512[3]);
                    ss_512[3] = _mm512_unpackhi_epi8(s_512[0], s_512[1]);
                    ss_512[4] = _mm512_unpackhi_epi8(s_512[2], s_512[3]);

                    y = h;
                    do {
                        src_ptr += 2 * src_stride;
                        y_convolve_6tap_32x2_avx512(src_ptr, src_stride, coeffs_512, s_256, ss_512, r);
                        jnt_comp_avg_round_store_32x2_avx512(
                            r, factor_512, offset_comp_avg_512, dst, dst_stride, dst8, dst8_stride);

                        ss_512[0] = ss_512[1];
                        ss_512[1] = ss_512[2];
                        ss_512[3] = ss_512[4];
                        ss_512[4] = ss_512[5];
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                } else {
                    __m512i s_512[6], ss_512[6], tt_512[6], r[4];

                    assert(!(w % 64));

                    x = 0;
                    do {
                        const uint8_t* s  = src_ptr + x;
                        ConvBufType*   d  = dst + x;
                        uint8_t*       d8 = dst8 + x;

                        s_512[0] = _mm512_loadu_si512((__m512i*)(s + 0 * src_stride));
                        s_512[1] = _mm512_loadu_si512((__m512i*)(s + 1 * src_stride));
                        s_512[2] = _mm512_loadu_si512((__m512i*)(s + 2 * src_stride));
                        s_512[3] = _mm512_loadu_si512((__m512i*)(s + 3 * src_stride));
                        s_512[4] = _mm512_loadu_si512((__m512i*)(s + 4 * src_stride));

                        ss_512[0] = _mm512_unpacklo_epi8(s_512[0], s_512[1]);
                        ss_512[1] = _mm512_unpacklo_epi8(s_512[2], s_512[3]);
                        ss_512[3] = _mm512_unpackhi_epi8(s_512[0], s_512[1]);
                        ss_512[4] = _mm512_unpackhi_epi8(s_512[2], s_512[3]);

                        tt_512[0] = _mm512_unpacklo_epi8(s_512[1], s_512[2]);
                        tt_512[1] = _mm512_unpacklo_epi8(s_512[3], s_512[4]);
                        tt_512[3] = _mm512_unpackhi_epi8(s_512[1], s_512[2]);
                        tt_512[4] = _mm512_unpackhi_epi8(s_512[3], s_512[4]);

                        y = h;
                        do {
                            s += 2 * src_stride;
                            y_convolve_6tap_64x2_avx512(s, src_stride, coeffs_512, s_512, ss_512, tt_512, r);
                            jnt_comp_avg_round_store_64_avx512(r, factor_512, offset_comp_avg_512, d, d8);
                            jnt_comp_avg_round_store_64_avx512(
                                r + 2, factor_512, offset_comp_avg_512, d + dst_stride, d8 + dst8_stride);

                            ss_512[0] = ss_512[1];
                            ss_512[1] = ss_512[2];
                            ss_512[3] = ss_512[4];
                            ss_512[4] = ss_512[5];

                            tt_512[0] = tt_512[1];
                            tt_512[1] = tt_512[2];
                            tt_512[3] = tt_512[4];
                            tt_512[4] = tt_512[5];
                            d += 2 * dst_stride;
                            d8 += 2 * dst8_stride;
                            y -= 2;
                        } while (y);

                        x += 64;
                    } while (x < w);
                }
            }
        } else {
            const int16_t offset_avg = (1 << (FILTER_BITS - 1)) + (1 << (round_1 - bits - 2)) -
                (round_offset << (round_1 - bits - 1));

            if (w <= 4) {
                const __m128i offset_avg_128 = _mm_set1_epi16(offset_avg);

                prepare_half_coeffs_6tap_ssse3(filter_params_y, subpel_y_q4, coeffs_128);

                y = h;

                if (w == 2) {
                    __m128i s_16[6], ss_128[3];

                    s_16[0] = _mm_cvtsi32_si128(*(int16_t*)(src_ptr + 0 * src_stride));
                    s_16[1] = _mm_cvtsi32_si128(*(int16_t*)(src_ptr + 1 * src_stride));
                    s_16[2] = _mm_cvtsi32_si128(*(int16_t*)(src_ptr + 2 * src_stride));
                    s_16[3] = _mm_cvtsi32_si128(*(int16_t*)(src_ptr + 3 * src_stride));
                    s_16[4] = _mm_cvtsi32_si128(*(int16_t*)(src_ptr + 4 * src_stride));

                    const __m128i src01 = _mm_unpacklo_epi16(s_16[0], s_16[1]);
                    const __m128i src12 = _mm_unpacklo_epi16(s_16[1], s_16[2]);
                    const __m128i src23 = _mm_unpacklo_epi16(s_16[2], s_16[3]);
                    const __m128i src34 = _mm_unpacklo_epi16(s_16[3], s_16[4]);

                    ss_128[0] = _mm_unpacklo_epi8(src01, src12);
                    ss_128[1] = _mm_unpacklo_epi8(src23, src34);

                    do {
                        src_ptr += 2 * src_stride;
                        const __m128i res = y_convolve_6tap_2x2_ssse3(src_ptr, src_stride, coeffs_128, s_16, ss_128);
                        jnt_avg_round_store_2x2_sse2(res, offset_avg_128, dst, dst_stride, dst8, dst8_stride);
                        ss_128[0] = ss_128[1];
                        ss_128[1] = ss_128[2];
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                } else {
                    __m128i s_32[6], ss_128[3];

                    assert(w == 4);

                    s_32[0] = _mm_cvtsi32_si128(*(int32_t*)(src_ptr + 0 * src_stride));
                    s_32[1] = _mm_cvtsi32_si128(*(int32_t*)(src_ptr + 1 * src_stride));
                    s_32[2] = _mm_cvtsi32_si128(*(int32_t*)(src_ptr + 2 * src_stride));
                    s_32[3] = _mm_cvtsi32_si128(*(int32_t*)(src_ptr + 3 * src_stride));
                    s_32[4] = _mm_cvtsi32_si128(*(int32_t*)(src_ptr + 4 * src_stride));

                    const __m128i src01 = _mm_unpacklo_epi32(s_32[0], s_32[1]);
                    const __m128i src12 = _mm_unpacklo_epi32(s_32[1], s_32[2]);
                    const __m128i src23 = _mm_unpacklo_epi32(s_32[2], s_32[3]);
                    const __m128i src34 = _mm_unpacklo_epi32(s_32[3], s_32[4]);

                    ss_128[0] = _mm_unpacklo_epi8(src01, src12);
                    ss_128[1] = _mm_unpacklo_epi8(src23, src34);

                    do {
                        src_ptr += 2 * src_stride;
                        const __m128i res = y_convolve_6tap_4x2_ssse3(src_ptr, src_stride, coeffs_128, s_32, ss_128);
                        jnt_avg_round_store_4x2_sse2(res, offset_avg_128, dst, dst_stride, dst8, dst8_stride);
                        ss_128[0] = ss_128[1];
                        ss_128[1] = ss_128[2];
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                }
            } else if (w <= 16) {
                const __m256i offset_avg_256 = _mm256_set1_epi16(offset_avg);

                prepare_half_coeffs_6tap_avx2(filter_params_y, subpel_y_q4, coeffs_256);

                if (w == 8) {
                    __m128i s_64[6];
                    __m256i ss_256[3];

                    s_64[0] = _mm_loadl_epi64((__m128i*)(src_ptr + 0 * src_stride));
                    s_64[1] = _mm_loadl_epi64((__m128i*)(src_ptr + 1 * src_stride));
                    s_64[2] = _mm_loadl_epi64((__m128i*)(src_ptr + 2 * src_stride));
                    s_64[3] = _mm_loadl_epi64((__m128i*)(src_ptr + 3 * src_stride));
                    s_64[4] = _mm_loadl_epi64((__m128i*)(src_ptr + 4 * src_stride));

                    // Load lines a and b. Line a to lower 128, line b to upper
                    // 128
                    const __m256i src01 = _mm256_setr_m128i(s_64[0], s_64[1]);
                    const __m256i src12 = _mm256_setr_m128i(s_64[1], s_64[2]);
                    const __m256i src23 = _mm256_setr_m128i(s_64[2], s_64[3]);
                    const __m256i src34 = _mm256_setr_m128i(s_64[3], s_64[4]);

                    ss_256[0] = _mm256_unpacklo_epi8(src01, src12);
                    ss_256[1] = _mm256_unpacklo_epi8(src23, src34);

                    y = h;
                    do {
                        src_ptr += 2 * src_stride;
                        const __m256i res = y_convolve_6tap_8x2_avx2(src_ptr, src_stride, coeffs_256, s_64, ss_256);
                        jnt_avg_round_store_8x2_avx2(res, offset_avg_256, dst, dst_stride, dst8, dst8_stride);
                        ss_256[0] = ss_256[1];
                        ss_256[1] = ss_256[2];
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                } else {
                    __m128i s_128[6];
                    __m256i ss_256[6], r[2];

                    s_128[0] = _mm_loadu_si128((__m128i*)(src_ptr + 0 * src_stride));
                    s_128[1] = _mm_loadu_si128((__m128i*)(src_ptr + 1 * src_stride));
                    s_128[2] = _mm_loadu_si128((__m128i*)(src_ptr + 2 * src_stride));
                    s_128[3] = _mm_loadu_si128((__m128i*)(src_ptr + 3 * src_stride));
                    s_128[4] = _mm_loadu_si128((__m128i*)(src_ptr + 4 * src_stride));

                    // Load lines a and b. Line a to lower 128, line b to upper
                    // 128
                    const __m256i src01 = _mm256_setr_m128i(s_128[0], s_128[1]);
                    const __m256i src12 = _mm256_setr_m128i(s_128[1], s_128[2]);
                    const __m256i src23 = _mm256_setr_m128i(s_128[2], s_128[3]);
                    const __m256i src34 = _mm256_setr_m128i(s_128[3], s_128[4]);

                    ss_256[0] = _mm256_unpacklo_epi8(src01, src12);
                    ss_256[1] = _mm256_unpacklo_epi8(src23, src34);

                    ss_256[3] = _mm256_unpackhi_epi8(src01, src12);
                    ss_256[4] = _mm256_unpackhi_epi8(src23, src34);

                    y = h;
                    do {
                        src_ptr += 2 * src_stride;
                        y_convolve_6tap_16x2_avx2(src_ptr, src_stride, coeffs_256, s_128, ss_256, r);
                        jnt_avg_round_store_16x2_avx2(r, offset_avg_256, dst, dst_stride, dst8, dst8_stride);
                        ss_256[0] = ss_256[1];
                        ss_256[1] = ss_256[2];
                        ss_256[3] = ss_256[4];
                        ss_256[4] = ss_256[5];
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                }
            } else {
                const __m512i offset_avg_512 = _mm512_set1_epi16(offset_avg);

                prepare_half_coeffs_6tap_avx512(filter_params_y, subpel_y_q4, coeffs_512);

                if (w == 32) {
                    __m256i s_256[5];
                    __m512i s_512[4], ss_512[6], r[2];

                    s_256[0] = _mm256_loadu_si256((__m256i*)(src_ptr + 0 * src_stride));
                    s_256[1] = _mm256_loadu_si256((__m256i*)(src_ptr + 1 * src_stride));
                    s_256[2] = _mm256_loadu_si256((__m256i*)(src_ptr + 2 * src_stride));
                    s_256[3] = _mm256_loadu_si256((__m256i*)(src_ptr + 3 * src_stride));
                    s_256[4] = _mm256_loadu_si256((__m256i*)(src_ptr + 4 * src_stride));

                    s_512[0] = _mm512_setr_m256i(s_256[0], s_256[1]);
                    s_512[1] = _mm512_setr_m256i(s_256[1], s_256[2]);
                    s_512[2] = _mm512_setr_m256i(s_256[2], s_256[3]);
                    s_512[3] = _mm512_setr_m256i(s_256[3], s_256[4]);

                    ss_512[0] = _mm512_unpacklo_epi8(s_512[0], s_512[1]);
                    ss_512[1] = _mm512_unpacklo_epi8(s_512[2], s_512[3]);
                    ss_512[3] = _mm512_unpackhi_epi8(s_512[0], s_512[1]);
                    ss_512[4] = _mm512_unpackhi_epi8(s_512[2], s_512[3]);

                    y = h;
                    do {
                        src_ptr += 2 * src_stride;
                        y_convolve_6tap_32x2_avx512(src_ptr, src_stride, coeffs_512, s_256, ss_512, r);
                        jnt_avg_round_store_32x2_avx512(r, offset_avg_512, dst, dst_stride, dst8, dst8_stride);

                        ss_512[0] = ss_512[1];
                        ss_512[1] = ss_512[2];
                        ss_512[3] = ss_512[4];
                        ss_512[4] = ss_512[5];
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                } else {
                    __m512i s_512[6], ss_512[6], tt_512[6], r[4];

                    assert(!(w % 64));

                    x = 0;
                    do {
                        const uint8_t* s  = src_ptr + x;
                        ConvBufType*   d  = dst + x;
                        uint8_t*       d8 = dst8 + x;

                        s_512[0] = _mm512_loadu_si512((__m512i*)(s + 0 * src_stride));
                        s_512[1] = _mm512_loadu_si512((__m512i*)(s + 1 * src_stride));
                        s_512[2] = _mm512_loadu_si512((__m512i*)(s + 2 * src_stride));
                        s_512[3] = _mm512_loadu_si512((__m512i*)(s + 3 * src_stride));
                        s_512[4] = _mm512_loadu_si512((__m512i*)(s + 4 * src_stride));

                        ss_512[0] = _mm512_unpacklo_epi8(s_512[0], s_512[1]);
                        ss_512[1] = _mm512_unpacklo_epi8(s_512[2], s_512[3]);
                        ss_512[3] = _mm512_unpackhi_epi8(s_512[0], s_512[1]);
                        ss_512[4] = _mm512_unpackhi_epi8(s_512[2], s_512[3]);

                        tt_512[0] = _mm512_unpacklo_epi8(s_512[1], s_512[2]);
                        tt_512[1] = _mm512_unpacklo_epi8(s_512[3], s_512[4]);
                        tt_512[3] = _mm512_unpackhi_epi8(s_512[1], s_512[2]);
                        tt_512[4] = _mm512_unpackhi_epi8(s_512[3], s_512[4]);

                        y = h;
                        do {
                            s += 2 * src_stride;
                            y_convolve_6tap_64x2_avx512(s, src_stride, coeffs_512, s_512, ss_512, tt_512, r);
                            jnt_avg_round_store_64_avx512(r, offset_avg_512, d, d8);
                            jnt_avg_round_store_64_avx512(r + 2, offset_avg_512, d + dst_stride, d8 + dst8_stride);

                            ss_512[0] = ss_512[1];
                            ss_512[1] = ss_512[2];
                            ss_512[3] = ss_512[4];
                            ss_512[4] = ss_512[5];

                            tt_512[0] = tt_512[1];
                            tt_512[1] = tt_512[2];
                            tt_512[3] = tt_512[4];
                            tt_512[4] = tt_512[5];
                            d += 2 * dst_stride;
                            d8 += 2 * dst8_stride;
                            y -= 2;
                        } while (y);

                        x += 64;
                    } while (x < w);
                }
            }
        }
    } else {
        const int16_t offset_no_avg = (round_offset << (round_1 - bits - 1)) + (1 << (round_1 - bits - 2));

        if (w <= 4) {
            const __m128i offset_no_avg_128 = _mm_set1_epi16(offset_no_avg);

            prepare_half_coeffs_6tap_ssse3(filter_params_y, subpel_y_q4, coeffs_128);

            y = h;

            if (w == 2) {
                __m128i s_16[6], ss_128[3];

                s_16[0] = _mm_cvtsi32_si128(*(int16_t*)(src_ptr + 0 * src_stride));
                s_16[1] = _mm_cvtsi32_si128(*(int16_t*)(src_ptr + 1 * src_stride));
                s_16[2] = _mm_cvtsi32_si128(*(int16_t*)(src_ptr + 2 * src_stride));
                s_16[3] = _mm_cvtsi32_si128(*(int16_t*)(src_ptr + 3 * src_stride));
                s_16[4] = _mm_cvtsi32_si128(*(int16_t*)(src_ptr + 4 * src_stride));

                const __m128i src01 = _mm_unpacklo_epi16(s_16[0], s_16[1]);
                const __m128i src12 = _mm_unpacklo_epi16(s_16[1], s_16[2]);
                const __m128i src23 = _mm_unpacklo_epi16(s_16[2], s_16[3]);
                const __m128i src34 = _mm_unpacklo_epi16(s_16[3], s_16[4]);

                ss_128[0] = _mm_unpacklo_epi8(src01, src12);
                ss_128[1] = _mm_unpacklo_epi8(src23, src34);

                do {
                    src_ptr += 2 * src_stride;
                    const __m128i res = y_convolve_6tap_2x2_ssse3(src_ptr, src_stride, coeffs_128, s_16, ss_128);
                    jnt_no_avg_round_store_2x2_sse2(res, offset_no_avg_128, dst, dst_stride);
                    ss_128[0] = ss_128[1];
                    ss_128[1] = ss_128[2];
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            } else {
                __m128i s_32[6], ss_128[3];

                assert(w == 4);

                s_32[0] = _mm_cvtsi32_si128(*(int32_t*)(src_ptr + 0 * src_stride));
                s_32[1] = _mm_cvtsi32_si128(*(int32_t*)(src_ptr + 1 * src_stride));
                s_32[2] = _mm_cvtsi32_si128(*(int32_t*)(src_ptr + 2 * src_stride));
                s_32[3] = _mm_cvtsi32_si128(*(int32_t*)(src_ptr + 3 * src_stride));
                s_32[4] = _mm_cvtsi32_si128(*(int32_t*)(src_ptr + 4 * src_stride));

                const __m128i src01 = _mm_unpacklo_epi32(s_32[0], s_32[1]);
                const __m128i src12 = _mm_unpacklo_epi32(s_32[1], s_32[2]);
                const __m128i src23 = _mm_unpacklo_epi32(s_32[2], s_32[3]);
                const __m128i src34 = _mm_unpacklo_epi32(s_32[3], s_32[4]);

                ss_128[0] = _mm_unpacklo_epi8(src01, src12);
                ss_128[1] = _mm_unpacklo_epi8(src23, src34);

                do {
                    src_ptr += 2 * src_stride;
                    const __m128i res = y_convolve_6tap_4x2_ssse3(src_ptr, src_stride, coeffs_128, s_32, ss_128);
                    jnt_no_avg_round_store_4x2_sse2(res, offset_no_avg_128, dst, dst_stride);
                    ss_128[0] = ss_128[1];
                    ss_128[1] = ss_128[2];
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            }
        } else if (w <= 16) {
            const __m256i offset_no_avg_256 = _mm256_set1_epi16(offset_no_avg);

            prepare_half_coeffs_6tap_avx2(filter_params_y, subpel_y_q4, coeffs_256);

            if (w == 8) {
                __m128i s_64[6];
                __m256i ss_256[3];

                s_64[0] = _mm_loadl_epi64((__m128i*)(src_ptr + 0 * src_stride));
                s_64[1] = _mm_loadl_epi64((__m128i*)(src_ptr + 1 * src_stride));
                s_64[2] = _mm_loadl_epi64((__m128i*)(src_ptr + 2 * src_stride));
                s_64[3] = _mm_loadl_epi64((__m128i*)(src_ptr + 3 * src_stride));
                s_64[4] = _mm_loadl_epi64((__m128i*)(src_ptr + 4 * src_stride));

                // Load lines a and b. Line a to lower 128, line b to upper 128
                const __m256i src01 = _mm256_setr_m128i(s_64[0], s_64[1]);
                const __m256i src12 = _mm256_setr_m128i(s_64[1], s_64[2]);
                const __m256i src23 = _mm256_setr_m128i(s_64[2], s_64[3]);
                const __m256i src34 = _mm256_setr_m128i(s_64[3], s_64[4]);

                ss_256[0] = _mm256_unpacklo_epi8(src01, src12);
                ss_256[1] = _mm256_unpacklo_epi8(src23, src34);

                y = h;
                do {
                    src_ptr += 2 * src_stride;
                    const __m256i res = y_convolve_6tap_8x2_avx2(src_ptr, src_stride, coeffs_256, s_64, ss_256);
                    jnt_no_avg_round_store_8x2_avx2(res, offset_no_avg_256, dst, dst_stride);
                    ss_256[0] = ss_256[1];
                    ss_256[1] = ss_256[2];
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            } else {
                __m128i s_128[6];
                __m256i ss_256[6], r[2];

                assert(w == 16);

                s_128[0] = _mm_loadu_si128((__m128i*)(src_ptr + 0 * src_stride));
                s_128[1] = _mm_loadu_si128((__m128i*)(src_ptr + 1 * src_stride));
                s_128[2] = _mm_loadu_si128((__m128i*)(src_ptr + 2 * src_stride));
                s_128[3] = _mm_loadu_si128((__m128i*)(src_ptr + 3 * src_stride));
                s_128[4] = _mm_loadu_si128((__m128i*)(src_ptr + 4 * src_stride));

                // Load lines a and b. Line a to lower 128, line b to upper 128
                const __m256i src01 = _mm256_setr_m128i(s_128[0], s_128[1]);
                const __m256i src12 = _mm256_setr_m128i(s_128[1], s_128[2]);
                const __m256i src23 = _mm256_setr_m128i(s_128[2], s_128[3]);
                const __m256i src34 = _mm256_setr_m128i(s_128[3], s_128[4]);

                ss_256[0] = _mm256_unpacklo_epi8(src01, src12);
                ss_256[1] = _mm256_unpacklo_epi8(src23, src34);

                ss_256[3] = _mm256_unpackhi_epi8(src01, src12);
                ss_256[4] = _mm256_unpackhi_epi8(src23, src34);

                y = h;
                do {
                    src_ptr += 2 * src_stride;
                    y_convolve_6tap_16x2_avx2(src_ptr, src_stride, coeffs_256, s_128, ss_256, r);
                    jnt_no_avg_round_store_16x2_avx2(r, offset_no_avg_256, dst, dst_stride);
                    ss_256[0] = ss_256[1];
                    ss_256[1] = ss_256[2];
                    ss_256[3] = ss_256[4];
                    ss_256[4] = ss_256[5];
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            }
        } else {
            const __m512i offset_no_avg_512 = _mm512_set1_epi16(offset_no_avg);

            prepare_half_coeffs_6tap_avx512(filter_params_y, subpel_y_q4, coeffs_512);

            if (w == 32) {
                __m256i s_256[5];
                __m512i s_512[4], ss_512[6], r[2];

                s_256[0] = _mm256_loadu_si256((__m256i*)(src_ptr + 0 * src_stride));
                s_256[1] = _mm256_loadu_si256((__m256i*)(src_ptr + 1 * src_stride));
                s_256[2] = _mm256_loadu_si256((__m256i*)(src_ptr + 2 * src_stride));
                s_256[3] = _mm256_loadu_si256((__m256i*)(src_ptr + 3 * src_stride));
                s_256[4] = _mm256_loadu_si256((__m256i*)(src_ptr + 4 * src_stride));

                s_512[0] = _mm512_setr_m256i(s_256[0], s_256[1]);
                s_512[1] = _mm512_setr_m256i(s_256[1], s_256[2]);
                s_512[2] = _mm512_setr_m256i(s_256[2], s_256[3]);
                s_512[3] = _mm512_setr_m256i(s_256[3], s_256[4]);

                ss_512[0] = _mm512_unpacklo_epi8(s_512[0], s_512[1]);
                ss_512[1] = _mm512_unpacklo_epi8(s_512[2], s_512[3]);
                ss_512[3] = _mm512_unpackhi_epi8(s_512[0], s_512[1]);
                ss_512[4] = _mm512_unpackhi_epi8(s_512[2], s_512[3]);

                y = h;
                do {
                    src_ptr += 2 * src_stride;
                    y_convolve_6tap_32x2_avx512(src_ptr, src_stride, coeffs_512, s_256, ss_512, r);
                    jnt_no_avg_round_store_32x2_avx512(r, offset_no_avg_512, dst, dst_stride);

                    ss_512[0] = ss_512[1];
                    ss_512[1] = ss_512[2];
                    ss_512[3] = ss_512[4];
                    ss_512[4] = ss_512[5];
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            } else {
                __m512i s_512[6], ss_512[6], tt_512[6], r[4];

                assert(!(w % 64));

                x = 0;
                do {
                    const uint8_t* s = src_ptr + x;
                    ConvBufType*   d = dst + x;

                    s_512[0] = _mm512_loadu_si512((__m512i*)(s + 0 * src_stride));
                    s_512[1] = _mm512_loadu_si512((__m512i*)(s + 1 * src_stride));
                    s_512[2] = _mm512_loadu_si512((__m512i*)(s + 2 * src_stride));
                    s_512[3] = _mm512_loadu_si512((__m512i*)(s + 3 * src_stride));
                    s_512[4] = _mm512_loadu_si512((__m512i*)(s + 4 * src_stride));

                    ss_512[0] = _mm512_unpacklo_epi8(s_512[0], s_512[1]);
                    ss_512[1] = _mm512_unpacklo_epi8(s_512[2], s_512[3]);
                    ss_512[3] = _mm512_unpackhi_epi8(s_512[0], s_512[1]);
                    ss_512[4] = _mm512_unpackhi_epi8(s_512[2], s_512[3]);

                    tt_512[0] = _mm512_unpacklo_epi8(s_512[1], s_512[2]);
                    tt_512[1] = _mm512_unpacklo_epi8(s_512[3], s_512[4]);
                    tt_512[3] = _mm512_unpackhi_epi8(s_512[1], s_512[2]);
                    tt_512[4] = _mm512_unpackhi_epi8(s_512[3], s_512[4]);

                    y = h;
                    do {
                        s += 2 * src_stride;
                        y_convolve_6tap_64x2_avx512(s, src_stride, coeffs_512, s_512, ss_512, tt_512, r);
                        jnt_no_avg_round_store_64_avx512(r, offset_no_avg_512, d);
                        jnt_no_avg_round_store_64_avx512(r + 2, offset_no_avg_512, d + dst_stride);

                        ss_512[0] = ss_512[1];
                        ss_512[1] = ss_512[2];
                        ss_512[3] = ss_512[4];
                        ss_512[4] = ss_512[5];

                        tt_512[0] = tt_512[1];
                        tt_512[1] = tt_512[2];
                        tt_512[3] = tt_512[4];
                        tt_512[4] = tt_512[5];
                        d += 2 * dst_stride;
                        y -= 2;
                    } while (y);

                    x += 64;
                } while (x < w);
            }
        }
    }
}

static void jnt_convolve_y_8tap_avx512(const uint8_t* const src, const int32_t src_stride, uint8_t* dst8,
                                       const int32_t dst8_stride, const int32_t w, const int32_t h,
                                       const InterpFilterParams* const filter_params_y, const int32_t subpel_y_q4,
                                       const ConvolveParams* const conv_params) {
    const uint8_t* src_ptr      = src - 3 * src_stride;
    const int32_t  dst_stride   = conv_params->dst_stride;
    const int32_t  round_0      = 3;
    const int32_t  round_1      = COMPOUND_ROUND1_BITS;
    const int32_t  bits         = FILTER_BITS - round_0;
    const int32_t  bd           = 8;
    const int32_t  round_bits   = 2 * FILTER_BITS - round_0 - round_1;
    const int32_t  offset_bits  = bd + round_bits;
    const int32_t  round_offset = (1 << offset_bits) + (1 << (offset_bits - 1));
    ConvBufType*   dst          = conv_params->dst;
    int32_t        x;
    int32_t        y = h;
    __m128i        coeffs_128[4];
    __m256i        coeffs_256[4];
    __m512i        coeffs_512[4];

    if (conv_params->do_average) {
        if (conv_params->use_jnt_comp_avg) {
            const int32_t factor          = conv_params->fwd_offset | (conv_params->bck_offset << 16);
            const int32_t offset_comp_avg = round_offset * conv_params->bck_offset +
                (1 << (round_bits + DIST_PRECISION_BITS - 1)) - (round_offset << DIST_PRECISION_BITS);

            if (w <= 4) {
                const __m128i factor_128          = _mm_set1_epi32(factor);
                const __m128i offset_comp_avg_128 = _mm_set1_epi32(offset_comp_avg);

                prepare_half_coeffs_8tap_ssse3(filter_params_y, subpel_y_q4, coeffs_128);

                y = h;

                if (w == 2) {
                    __m128i s_16[8], ss_128[4];

                    s_16[0] = _mm_cvtsi32_si128(*(int16_t*)(src_ptr + 0 * src_stride));
                    s_16[1] = _mm_cvtsi32_si128(*(int16_t*)(src_ptr + 1 * src_stride));
                    s_16[2] = _mm_cvtsi32_si128(*(int16_t*)(src_ptr + 2 * src_stride));
                    s_16[3] = _mm_cvtsi32_si128(*(int16_t*)(src_ptr + 3 * src_stride));
                    s_16[4] = _mm_cvtsi32_si128(*(int16_t*)(src_ptr + 4 * src_stride));
                    s_16[5] = _mm_cvtsi32_si128(*(int16_t*)(src_ptr + 5 * src_stride));
                    s_16[6] = _mm_cvtsi32_si128(*(int16_t*)(src_ptr + 6 * src_stride));

                    const __m128i src01 = _mm_unpacklo_epi16(s_16[0], s_16[1]);
                    const __m128i src12 = _mm_unpacklo_epi16(s_16[1], s_16[2]);
                    const __m128i src23 = _mm_unpacklo_epi16(s_16[2], s_16[3]);
                    const __m128i src34 = _mm_unpacklo_epi16(s_16[3], s_16[4]);
                    const __m128i src45 = _mm_unpacklo_epi16(s_16[4], s_16[5]);
                    const __m128i src56 = _mm_unpacklo_epi16(s_16[5], s_16[6]);

                    ss_128[0] = _mm_unpacklo_epi8(src01, src12);
                    ss_128[1] = _mm_unpacklo_epi8(src23, src34);
                    ss_128[2] = _mm_unpacklo_epi8(src45, src56);

                    do {
                        const __m128i res = y_convolve_8tap_2x2_ssse3(src_ptr, src_stride, coeffs_128, s_16, ss_128);
                        jnt_comp_avg_round_store_2x2_sse2(
                            res, factor_128, offset_comp_avg_128, dst, dst_stride, dst8, dst8_stride);
                        ss_128[0] = ss_128[1];
                        ss_128[1] = ss_128[2];
                        ss_128[2] = ss_128[3];
                        src_ptr += 2 * src_stride;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                } else {
                    __m128i s_32[8], ss_128[4];

                    assert(w == 4);

                    s_32[0] = _mm_cvtsi32_si128(*(int32_t*)(src_ptr + 0 * src_stride));
                    s_32[1] = _mm_cvtsi32_si128(*(int32_t*)(src_ptr + 1 * src_stride));
                    s_32[2] = _mm_cvtsi32_si128(*(int32_t*)(src_ptr + 2 * src_stride));
                    s_32[3] = _mm_cvtsi32_si128(*(int32_t*)(src_ptr + 3 * src_stride));
                    s_32[4] = _mm_cvtsi32_si128(*(int32_t*)(src_ptr + 4 * src_stride));
                    s_32[5] = _mm_cvtsi32_si128(*(int32_t*)(src_ptr + 5 * src_stride));
                    s_32[6] = _mm_cvtsi32_si128(*(int32_t*)(src_ptr + 6 * src_stride));

                    const __m128i src01 = _mm_unpacklo_epi32(s_32[0], s_32[1]);
                    const __m128i src12 = _mm_unpacklo_epi32(s_32[1], s_32[2]);
                    const __m128i src23 = _mm_unpacklo_epi32(s_32[2], s_32[3]);
                    const __m128i src34 = _mm_unpacklo_epi32(s_32[3], s_32[4]);
                    const __m128i src45 = _mm_unpacklo_epi32(s_32[4], s_32[5]);
                    const __m128i src56 = _mm_unpacklo_epi32(s_32[5], s_32[6]);

                    ss_128[0] = _mm_unpacklo_epi8(src01, src12);
                    ss_128[1] = _mm_unpacklo_epi8(src23, src34);
                    ss_128[2] = _mm_unpacklo_epi8(src45, src56);

                    do {
                        const __m128i res = y_convolve_8tap_4x2_ssse3(src_ptr, src_stride, coeffs_128, s_32, ss_128);
                        jnt_comp_avg_round_store_4x2_sse2(
                            res, factor_128, offset_comp_avg_128, dst, dst_stride, dst8, dst8_stride);
                        ss_128[0] = ss_128[1];
                        ss_128[1] = ss_128[2];
                        ss_128[2] = ss_128[3];
                        src_ptr += 2 * src_stride;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                }
            } else {
                if (w <= 16) {
                    const __m256i factor_256          = _mm256_set1_epi32(factor);
                    const __m256i offset_comp_avg_256 = _mm256_set1_epi32(offset_comp_avg);

                    prepare_half_coeffs_8tap_avx2(filter_params_y, subpel_y_q4, coeffs_256);

                    if (w == 8) {
                        __m128i s_64[8];
                        __m256i ss_256[4];

                        s_64[0] = _mm_loadl_epi64((__m128i*)(src_ptr + 0 * src_stride));
                        s_64[1] = _mm_loadl_epi64((__m128i*)(src_ptr + 1 * src_stride));
                        s_64[2] = _mm_loadl_epi64((__m128i*)(src_ptr + 2 * src_stride));
                        s_64[3] = _mm_loadl_epi64((__m128i*)(src_ptr + 3 * src_stride));
                        s_64[4] = _mm_loadl_epi64((__m128i*)(src_ptr + 4 * src_stride));
                        s_64[5] = _mm_loadl_epi64((__m128i*)(src_ptr + 5 * src_stride));
                        s_64[6] = _mm_loadl_epi64((__m128i*)(src_ptr + 6 * src_stride));

                        // Load lines a and b. Line a to lower 128, line b to
                        // upper 128
                        const __m256i src01 = _mm256_setr_m128i(s_64[0], s_64[1]);
                        const __m256i src12 = _mm256_setr_m128i(s_64[1], s_64[2]);
                        const __m256i src23 = _mm256_setr_m128i(s_64[2], s_64[3]);
                        const __m256i src34 = _mm256_setr_m128i(s_64[3], s_64[4]);
                        const __m256i src45 = _mm256_setr_m128i(s_64[4], s_64[5]);
                        const __m256i src56 = _mm256_setr_m128i(s_64[5], s_64[6]);

                        ss_256[0] = _mm256_unpacklo_epi8(src01, src12);
                        ss_256[1] = _mm256_unpacklo_epi8(src23, src34);
                        ss_256[2] = _mm256_unpacklo_epi8(src45, src56);

                        y = h;
                        do {
                            const __m256i res = y_convolve_8tap_8x2_avx2(src_ptr, src_stride, coeffs_256, s_64, ss_256);
                            jnt_comp_avg_round_store_8x2_avx2(
                                res, factor_256, offset_comp_avg_256, dst, dst_stride, dst8, dst8_stride);
                            ss_256[0] = ss_256[1];
                            ss_256[1] = ss_256[2];
                            ss_256[2] = ss_256[3];
                            src_ptr += 2 * src_stride;
                            dst += 2 * dst_stride;
                            dst8 += 2 * dst8_stride;
                            y -= 2;
                        } while (y);
                    } else {
                        __m128i s_128[8];
                        __m256i ss_256[8], r[2];

                        assert(w == 16);

                        s_128[0] = _mm_loadu_si128((__m128i*)(src_ptr + 0 * src_stride));
                        s_128[1] = _mm_loadu_si128((__m128i*)(src_ptr + 1 * src_stride));
                        s_128[2] = _mm_loadu_si128((__m128i*)(src_ptr + 2 * src_stride));
                        s_128[3] = _mm_loadu_si128((__m128i*)(src_ptr + 3 * src_stride));
                        s_128[4] = _mm_loadu_si128((__m128i*)(src_ptr + 4 * src_stride));
                        s_128[5] = _mm_loadu_si128((__m128i*)(src_ptr + 5 * src_stride));
                        s_128[6] = _mm_loadu_si128((__m128i*)(src_ptr + 6 * src_stride));

                        // Load lines a and b. Line a to lower 128, line b to
                        // upper 128
                        const __m256i src01 = _mm256_setr_m128i(s_128[0], s_128[1]);
                        const __m256i src12 = _mm256_setr_m128i(s_128[1], s_128[2]);
                        const __m256i src23 = _mm256_setr_m128i(s_128[2], s_128[3]);
                        const __m256i src34 = _mm256_setr_m128i(s_128[3], s_128[4]);
                        const __m256i src45 = _mm256_setr_m128i(s_128[4], s_128[5]);
                        const __m256i src56 = _mm256_setr_m128i(s_128[5], s_128[6]);

                        ss_256[0] = _mm256_unpacklo_epi8(src01, src12);
                        ss_256[1] = _mm256_unpacklo_epi8(src23, src34);
                        ss_256[2] = _mm256_unpacklo_epi8(src45, src56);

                        ss_256[4] = _mm256_unpackhi_epi8(src01, src12);
                        ss_256[5] = _mm256_unpackhi_epi8(src23, src34);
                        ss_256[6] = _mm256_unpackhi_epi8(src45, src56);

                        y = h;
                        do {
                            y_convolve_8tap_16x2_avx2(src_ptr, src_stride, coeffs_256, s_128, ss_256, r);
                            jnt_comp_avg_round_store_16x2_avx2(
                                r, factor_256, offset_comp_avg_256, dst, dst_stride, dst8, dst8_stride);
                            ss_256[0] = ss_256[1];
                            ss_256[1] = ss_256[2];
                            ss_256[2] = ss_256[3];
                            ss_256[4] = ss_256[5];
                            ss_256[5] = ss_256[6];
                            ss_256[6] = ss_256[7];
                            src_ptr += 2 * src_stride;
                            dst += 2 * dst_stride;
                            dst8 += 2 * dst8_stride;
                            y -= 2;
                        } while (y);
                    }
                } else {
                    const __m512i factor_512          = _mm512_set1_epi32(factor);
                    const __m512i offset_comp_avg_512 = _mm512_set1_epi32(offset_comp_avg);

                    prepare_half_coeffs_8tap_avx512(filter_params_y, subpel_y_q4, coeffs_512);

                    if (w == 32) {
                        __m256i s_256[7];
                        __m512i s_512[6], ss_512[8], r[2];

                        s_256[0] = _mm256_loadu_si256((__m256i*)(src_ptr + 0 * src_stride));
                        s_256[1] = _mm256_loadu_si256((__m256i*)(src_ptr + 1 * src_stride));
                        s_256[2] = _mm256_loadu_si256((__m256i*)(src_ptr + 2 * src_stride));
                        s_256[3] = _mm256_loadu_si256((__m256i*)(src_ptr + 3 * src_stride));
                        s_256[4] = _mm256_loadu_si256((__m256i*)(src_ptr + 4 * src_stride));
                        s_256[5] = _mm256_loadu_si256((__m256i*)(src_ptr + 5 * src_stride));
                        s_256[6] = _mm256_loadu_si256((__m256i*)(src_ptr + 6 * src_stride));

                        s_512[0] = _mm512_setr_m256i(s_256[0], s_256[1]);
                        s_512[1] = _mm512_setr_m256i(s_256[1], s_256[2]);
                        s_512[2] = _mm512_setr_m256i(s_256[2], s_256[3]);
                        s_512[3] = _mm512_setr_m256i(s_256[3], s_256[4]);
                        s_512[4] = _mm512_setr_m256i(s_256[4], s_256[5]);
                        s_512[5] = _mm512_setr_m256i(s_256[5], s_256[6]);

                        ss_512[0] = _mm512_unpacklo_epi8(s_512[0], s_512[1]);
                        ss_512[1] = _mm512_unpacklo_epi8(s_512[2], s_512[3]);
                        ss_512[2] = _mm512_unpacklo_epi8(s_512[4], s_512[5]);
                        ss_512[4] = _mm512_unpackhi_epi8(s_512[0], s_512[1]);
                        ss_512[5] = _mm512_unpackhi_epi8(s_512[2], s_512[3]);
                        ss_512[6] = _mm512_unpackhi_epi8(s_512[4], s_512[5]);

                        y = h;
                        do {
                            src_ptr += 2 * src_stride;
                            y_convolve_8tap_32x2_avx512(src_ptr, src_stride, coeffs_512, s_256, ss_512, r);
                            jnt_comp_avg_round_store_32x2_avx512(
                                r, factor_512, offset_comp_avg_512, dst, dst_stride, dst8, dst8_stride);

                            ss_512[0] = ss_512[1];
                            ss_512[1] = ss_512[2];
                            ss_512[2] = ss_512[3];
                            ss_512[4] = ss_512[5];
                            ss_512[5] = ss_512[6];
                            ss_512[6] = ss_512[7];
                            dst += 2 * dst_stride;
                            dst8 += 2 * dst8_stride;
                            y -= 2;
                        } while (y);
                    } else {
                        __m512i s_512[8], ss_512[8], tt_512[8], r[4];

                        assert(!(w % 64));

                        x = 0;
                        do {
                            const uint8_t* s  = src_ptr + x;
                            ConvBufType*   d  = dst + x;
                            uint8_t*       d8 = dst8 + x;

                            s_512[0] = _mm512_loadu_si512((__m512i*)(s + 0 * src_stride));
                            s_512[1] = _mm512_loadu_si512((__m512i*)(s + 1 * src_stride));
                            s_512[2] = _mm512_loadu_si512((__m512i*)(s + 2 * src_stride));
                            s_512[3] = _mm512_loadu_si512((__m512i*)(s + 3 * src_stride));
                            s_512[4] = _mm512_loadu_si512((__m512i*)(s + 4 * src_stride));
                            s_512[5] = _mm512_loadu_si512((__m512i*)(s + 5 * src_stride));
                            s_512[6] = _mm512_loadu_si512((__m512i*)(s + 6 * src_stride));

                            ss_512[0] = _mm512_unpacklo_epi8(s_512[0], s_512[1]);
                            ss_512[1] = _mm512_unpacklo_epi8(s_512[2], s_512[3]);
                            ss_512[2] = _mm512_unpacklo_epi8(s_512[4], s_512[5]);
                            ss_512[4] = _mm512_unpackhi_epi8(s_512[0], s_512[1]);
                            ss_512[5] = _mm512_unpackhi_epi8(s_512[2], s_512[3]);
                            ss_512[6] = _mm512_unpackhi_epi8(s_512[4], s_512[5]);

                            tt_512[0] = _mm512_unpacklo_epi8(s_512[1], s_512[2]);
                            tt_512[1] = _mm512_unpacklo_epi8(s_512[3], s_512[4]);
                            tt_512[2] = _mm512_unpacklo_epi8(s_512[5], s_512[6]);
                            tt_512[4] = _mm512_unpackhi_epi8(s_512[1], s_512[2]);
                            tt_512[5] = _mm512_unpackhi_epi8(s_512[3], s_512[4]);
                            tt_512[6] = _mm512_unpackhi_epi8(s_512[5], s_512[6]);

                            y = h;
                            do {
                                y_convolve_8tap_64x2_avx512(s, src_stride, coeffs_512, s_512, ss_512, tt_512, r);
                                jnt_comp_avg_round_store_64_avx512(r, factor_512, offset_comp_avg_512, d, d8);
                                jnt_comp_avg_round_store_64_avx512(
                                    r + 2, factor_512, offset_comp_avg_512, d + dst_stride, d8 + dst8_stride);

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
                                s += 2 * src_stride;
                                d += 2 * dst_stride;
                                d8 += 2 * dst8_stride;
                                y -= 2;
                            } while (y);

                            x += 64;
                        } while (x < w);
                    }
                }
            }
        } else {
            const int16_t offset_avg = (1 << (FILTER_BITS - 1)) + (1 << (round_1 - bits - 2)) -
                (round_offset << (round_1 - bits - 1));

            if (w <= 4) {
                const __m128i offset_avg_128 = _mm_set1_epi16(offset_avg);

                prepare_half_coeffs_8tap_ssse3(filter_params_y, subpel_y_q4, coeffs_128);

                y = h;

                if (w == 2) {
                    __m128i s_16[8], ss_128[4];

                    s_16[0] = _mm_cvtsi32_si128(*(int16_t*)(src_ptr + 0 * src_stride));
                    s_16[1] = _mm_cvtsi32_si128(*(int16_t*)(src_ptr + 1 * src_stride));
                    s_16[2] = _mm_cvtsi32_si128(*(int16_t*)(src_ptr + 2 * src_stride));
                    s_16[3] = _mm_cvtsi32_si128(*(int16_t*)(src_ptr + 3 * src_stride));
                    s_16[4] = _mm_cvtsi32_si128(*(int16_t*)(src_ptr + 4 * src_stride));
                    s_16[5] = _mm_cvtsi32_si128(*(int16_t*)(src_ptr + 5 * src_stride));
                    s_16[6] = _mm_cvtsi32_si128(*(int16_t*)(src_ptr + 6 * src_stride));

                    const __m128i src01 = _mm_unpacklo_epi16(s_16[0], s_16[1]);
                    const __m128i src12 = _mm_unpacklo_epi16(s_16[1], s_16[2]);
                    const __m128i src23 = _mm_unpacklo_epi16(s_16[2], s_16[3]);
                    const __m128i src34 = _mm_unpacklo_epi16(s_16[3], s_16[4]);
                    const __m128i src45 = _mm_unpacklo_epi16(s_16[4], s_16[5]);
                    const __m128i src56 = _mm_unpacklo_epi16(s_16[5], s_16[6]);

                    ss_128[0] = _mm_unpacklo_epi8(src01, src12);
                    ss_128[1] = _mm_unpacklo_epi8(src23, src34);
                    ss_128[2] = _mm_unpacklo_epi8(src45, src56);

                    do {
                        const __m128i res = y_convolve_8tap_2x2_ssse3(src_ptr, src_stride, coeffs_128, s_16, ss_128);
                        jnt_avg_round_store_2x2_sse2(res, offset_avg_128, dst, dst_stride, dst8, dst8_stride);
                        ss_128[0] = ss_128[1];
                        ss_128[1] = ss_128[2];
                        ss_128[2] = ss_128[3];
                        src_ptr += 2 * src_stride;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                } else {
                    __m128i s_32[8], ss_128[4];

                    assert(w == 4);

                    s_32[0] = _mm_cvtsi32_si128(*(int32_t*)(src_ptr + 0 * src_stride));
                    s_32[1] = _mm_cvtsi32_si128(*(int32_t*)(src_ptr + 1 * src_stride));
                    s_32[2] = _mm_cvtsi32_si128(*(int32_t*)(src_ptr + 2 * src_stride));
                    s_32[3] = _mm_cvtsi32_si128(*(int32_t*)(src_ptr + 3 * src_stride));
                    s_32[4] = _mm_cvtsi32_si128(*(int32_t*)(src_ptr + 4 * src_stride));
                    s_32[5] = _mm_cvtsi32_si128(*(int32_t*)(src_ptr + 5 * src_stride));
                    s_32[6] = _mm_cvtsi32_si128(*(int32_t*)(src_ptr + 6 * src_stride));

                    const __m128i src01 = _mm_unpacklo_epi32(s_32[0], s_32[1]);
                    const __m128i src12 = _mm_unpacklo_epi32(s_32[1], s_32[2]);
                    const __m128i src23 = _mm_unpacklo_epi32(s_32[2], s_32[3]);
                    const __m128i src34 = _mm_unpacklo_epi32(s_32[3], s_32[4]);
                    const __m128i src45 = _mm_unpacklo_epi32(s_32[4], s_32[5]);
                    const __m128i src56 = _mm_unpacklo_epi32(s_32[5], s_32[6]);

                    ss_128[0] = _mm_unpacklo_epi8(src01, src12);
                    ss_128[1] = _mm_unpacklo_epi8(src23, src34);
                    ss_128[2] = _mm_unpacklo_epi8(src45, src56);

                    do {
                        const __m128i res = y_convolve_8tap_4x2_ssse3(src_ptr, src_stride, coeffs_128, s_32, ss_128);
                        jnt_avg_round_store_4x2_sse2(res, offset_avg_128, dst, dst_stride, dst8, dst8_stride);
                        ss_128[0] = ss_128[1];
                        ss_128[1] = ss_128[2];
                        ss_128[2] = ss_128[3];
                        src_ptr += 2 * src_stride;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                }
            } else {
                if (w <= 16) {
                    const __m256i offset_avg_256 = _mm256_set1_epi16(offset_avg);

                    prepare_half_coeffs_8tap_avx2(filter_params_y, subpel_y_q4, coeffs_256);

                    if (w == 8) {
                        __m128i s_64[8];
                        __m256i ss_256[4];

                        s_64[0] = _mm_loadl_epi64((__m128i*)(src_ptr + 0 * src_stride));
                        s_64[1] = _mm_loadl_epi64((__m128i*)(src_ptr + 1 * src_stride));
                        s_64[2] = _mm_loadl_epi64((__m128i*)(src_ptr + 2 * src_stride));
                        s_64[3] = _mm_loadl_epi64((__m128i*)(src_ptr + 3 * src_stride));
                        s_64[4] = _mm_loadl_epi64((__m128i*)(src_ptr + 4 * src_stride));
                        s_64[5] = _mm_loadl_epi64((__m128i*)(src_ptr + 5 * src_stride));
                        s_64[6] = _mm_loadl_epi64((__m128i*)(src_ptr + 6 * src_stride));

                        // Load lines a and b. Line a to lower 128, line b to
                        // upper 128
                        const __m256i src01 = _mm256_setr_m128i(s_64[0], s_64[1]);
                        const __m256i src12 = _mm256_setr_m128i(s_64[1], s_64[2]);
                        const __m256i src23 = _mm256_setr_m128i(s_64[2], s_64[3]);
                        const __m256i src34 = _mm256_setr_m128i(s_64[3], s_64[4]);
                        const __m256i src45 = _mm256_setr_m128i(s_64[4], s_64[5]);
                        const __m256i src56 = _mm256_setr_m128i(s_64[5], s_64[6]);

                        ss_256[0] = _mm256_unpacklo_epi8(src01, src12);
                        ss_256[1] = _mm256_unpacklo_epi8(src23, src34);
                        ss_256[2] = _mm256_unpacklo_epi8(src45, src56);

                        y = h;
                        do {
                            const __m256i res = y_convolve_8tap_8x2_avx2(src_ptr, src_stride, coeffs_256, s_64, ss_256);
                            jnt_avg_round_store_8x2_avx2(res, offset_avg_256, dst, dst_stride, dst8, dst8_stride);
                            ss_256[0] = ss_256[1];
                            ss_256[1] = ss_256[2];
                            ss_256[2] = ss_256[3];
                            src_ptr += 2 * src_stride;
                            dst += 2 * dst_stride;
                            dst8 += 2 * dst8_stride;
                            y -= 2;
                        } while (y);
                    } else {
                        __m128i s_128[8];
                        __m256i ss_256[8], r[2];

                        assert(w == 16);

                        s_128[0] = _mm_loadu_si128((__m128i*)(src_ptr + 0 * src_stride));
                        s_128[1] = _mm_loadu_si128((__m128i*)(src_ptr + 1 * src_stride));
                        s_128[2] = _mm_loadu_si128((__m128i*)(src_ptr + 2 * src_stride));
                        s_128[3] = _mm_loadu_si128((__m128i*)(src_ptr + 3 * src_stride));
                        s_128[4] = _mm_loadu_si128((__m128i*)(src_ptr + 4 * src_stride));
                        s_128[5] = _mm_loadu_si128((__m128i*)(src_ptr + 5 * src_stride));
                        s_128[6] = _mm_loadu_si128((__m128i*)(src_ptr + 6 * src_stride));

                        // Load lines a and b. Line a to lower 128, line b to
                        // upper 128
                        const __m256i src01 = _mm256_setr_m128i(s_128[0], s_128[1]);
                        const __m256i src12 = _mm256_setr_m128i(s_128[1], s_128[2]);
                        const __m256i src23 = _mm256_setr_m128i(s_128[2], s_128[3]);
                        const __m256i src34 = _mm256_setr_m128i(s_128[3], s_128[4]);
                        const __m256i src45 = _mm256_setr_m128i(s_128[4], s_128[5]);
                        const __m256i src56 = _mm256_setr_m128i(s_128[5], s_128[6]);

                        ss_256[0] = _mm256_unpacklo_epi8(src01, src12);
                        ss_256[1] = _mm256_unpacklo_epi8(src23, src34);
                        ss_256[2] = _mm256_unpacklo_epi8(src45, src56);

                        ss_256[4] = _mm256_unpackhi_epi8(src01, src12);
                        ss_256[5] = _mm256_unpackhi_epi8(src23, src34);
                        ss_256[6] = _mm256_unpackhi_epi8(src45, src56);

                        y = h;
                        do {
                            y_convolve_8tap_16x2_avx2(src_ptr, src_stride, coeffs_256, s_128, ss_256, r);
                            jnt_avg_round_store_16x2_avx2(r, offset_avg_256, dst, dst_stride, dst8, dst8_stride);
                            ss_256[0] = ss_256[1];
                            ss_256[1] = ss_256[2];
                            ss_256[2] = ss_256[3];
                            ss_256[4] = ss_256[5];
                            ss_256[5] = ss_256[6];
                            ss_256[6] = ss_256[7];
                            src_ptr += 2 * src_stride;
                            dst += 2 * dst_stride;
                            dst8 += 2 * dst8_stride;
                            y -= 2;
                        } while (y);
                    }
                } else {
                    const __m512i offset_avg_512 = _mm512_set1_epi16(offset_avg);

                    prepare_half_coeffs_8tap_avx512(filter_params_y, subpel_y_q4, coeffs_512);

                    if (w == 32) {
                        __m256i s_256[7];
                        __m512i s_512[6], ss_512[8], r[2];

                        s_256[0] = _mm256_loadu_si256((__m256i*)(src_ptr + 0 * src_stride));
                        s_256[1] = _mm256_loadu_si256((__m256i*)(src_ptr + 1 * src_stride));
                        s_256[2] = _mm256_loadu_si256((__m256i*)(src_ptr + 2 * src_stride));
                        s_256[3] = _mm256_loadu_si256((__m256i*)(src_ptr + 3 * src_stride));
                        s_256[4] = _mm256_loadu_si256((__m256i*)(src_ptr + 4 * src_stride));
                        s_256[5] = _mm256_loadu_si256((__m256i*)(src_ptr + 5 * src_stride));
                        s_256[6] = _mm256_loadu_si256((__m256i*)(src_ptr + 6 * src_stride));

                        s_512[0] = _mm512_setr_m256i(s_256[0], s_256[1]);
                        s_512[1] = _mm512_setr_m256i(s_256[1], s_256[2]);
                        s_512[2] = _mm512_setr_m256i(s_256[2], s_256[3]);
                        s_512[3] = _mm512_setr_m256i(s_256[3], s_256[4]);
                        s_512[4] = _mm512_setr_m256i(s_256[4], s_256[5]);
                        s_512[5] = _mm512_setr_m256i(s_256[5], s_256[6]);

                        ss_512[0] = _mm512_unpacklo_epi8(s_512[0], s_512[1]);
                        ss_512[1] = _mm512_unpacklo_epi8(s_512[2], s_512[3]);
                        ss_512[2] = _mm512_unpacklo_epi8(s_512[4], s_512[5]);
                        ss_512[4] = _mm512_unpackhi_epi8(s_512[0], s_512[1]);
                        ss_512[5] = _mm512_unpackhi_epi8(s_512[2], s_512[3]);
                        ss_512[6] = _mm512_unpackhi_epi8(s_512[4], s_512[5]);

                        y = h;
                        do {
                            src_ptr += 2 * src_stride;
                            y_convolve_8tap_32x2_avx512(src_ptr, src_stride, coeffs_512, s_256, ss_512, r);
                            jnt_avg_round_store_32x2_avx512(r, offset_avg_512, dst, dst_stride, dst8, dst8_stride);

                            ss_512[0] = ss_512[1];
                            ss_512[1] = ss_512[2];
                            ss_512[2] = ss_512[3];
                            ss_512[4] = ss_512[5];
                            ss_512[5] = ss_512[6];
                            ss_512[6] = ss_512[7];
                            dst += 2 * dst_stride;
                            dst8 += 2 * dst8_stride;
                            y -= 2;
                        } while (y);
                    } else {
                        __m512i s_512[8], ss_512[8], tt_512[8], r[4];

                        assert(!(w % 64));

                        x = 0;
                        do {
                            const uint8_t* s  = src_ptr + x;
                            ConvBufType*   d  = dst + x;
                            uint8_t*       d8 = dst8 + x;

                            s_512[0] = _mm512_loadu_si512((__m512i*)(s + 0 * src_stride));
                            s_512[1] = _mm512_loadu_si512((__m512i*)(s + 1 * src_stride));
                            s_512[2] = _mm512_loadu_si512((__m512i*)(s + 2 * src_stride));
                            s_512[3] = _mm512_loadu_si512((__m512i*)(s + 3 * src_stride));
                            s_512[4] = _mm512_loadu_si512((__m512i*)(s + 4 * src_stride));
                            s_512[5] = _mm512_loadu_si512((__m512i*)(s + 5 * src_stride));
                            s_512[6] = _mm512_loadu_si512((__m512i*)(s + 6 * src_stride));

                            ss_512[0] = _mm512_unpacklo_epi8(s_512[0], s_512[1]);
                            ss_512[1] = _mm512_unpacklo_epi8(s_512[2], s_512[3]);
                            ss_512[2] = _mm512_unpacklo_epi8(s_512[4], s_512[5]);
                            ss_512[4] = _mm512_unpackhi_epi8(s_512[0], s_512[1]);
                            ss_512[5] = _mm512_unpackhi_epi8(s_512[2], s_512[3]);
                            ss_512[6] = _mm512_unpackhi_epi8(s_512[4], s_512[5]);

                            tt_512[0] = _mm512_unpacklo_epi8(s_512[1], s_512[2]);
                            tt_512[1] = _mm512_unpacklo_epi8(s_512[3], s_512[4]);
                            tt_512[2] = _mm512_unpacklo_epi8(s_512[5], s_512[6]);
                            tt_512[4] = _mm512_unpackhi_epi8(s_512[1], s_512[2]);
                            tt_512[5] = _mm512_unpackhi_epi8(s_512[3], s_512[4]);
                            tt_512[6] = _mm512_unpackhi_epi8(s_512[5], s_512[6]);

                            y = h;
                            do {
                                y_convolve_8tap_64x2_avx512(s, src_stride, coeffs_512, s_512, ss_512, tt_512, r);
                                jnt_avg_round_store_64_avx512(r, offset_avg_512, d, d8);
                                jnt_avg_round_store_64_avx512(r + 2, offset_avg_512, d + dst_stride, d8 + dst8_stride);

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
                                s += 2 * src_stride;
                                d += 2 * dst_stride;
                                d8 += 2 * dst8_stride;
                                y -= 2;
                            } while (y);

                            x += 64;
                        } while (x < w);
                    }
                }
            }
        }
    } else {
        const int16_t offset_no_avg = (round_offset << (round_1 - bits - 1)) + (1 << (round_1 - bits - 2));

        if (w <= 4) {
            const __m128i offset_no_avg_128 = _mm_set1_epi16(offset_no_avg);

            prepare_half_coeffs_8tap_ssse3(filter_params_y, subpel_y_q4, coeffs_128);

            y = h;

            if (w == 2) {
                __m128i s_16[8], ss_128[4];

                s_16[0] = _mm_cvtsi32_si128(*(int16_t*)(src_ptr + 0 * src_stride));
                s_16[1] = _mm_cvtsi32_si128(*(int16_t*)(src_ptr + 1 * src_stride));
                s_16[2] = _mm_cvtsi32_si128(*(int16_t*)(src_ptr + 2 * src_stride));
                s_16[3] = _mm_cvtsi32_si128(*(int16_t*)(src_ptr + 3 * src_stride));
                s_16[4] = _mm_cvtsi32_si128(*(int16_t*)(src_ptr + 4 * src_stride));
                s_16[5] = _mm_cvtsi32_si128(*(int16_t*)(src_ptr + 5 * src_stride));
                s_16[6] = _mm_cvtsi32_si128(*(int16_t*)(src_ptr + 6 * src_stride));

                const __m128i src01 = _mm_unpacklo_epi16(s_16[0], s_16[1]);
                const __m128i src12 = _mm_unpacklo_epi16(s_16[1], s_16[2]);
                const __m128i src23 = _mm_unpacklo_epi16(s_16[2], s_16[3]);
                const __m128i src34 = _mm_unpacklo_epi16(s_16[3], s_16[4]);
                const __m128i src45 = _mm_unpacklo_epi16(s_16[4], s_16[5]);
                const __m128i src56 = _mm_unpacklo_epi16(s_16[5], s_16[6]);

                ss_128[0] = _mm_unpacklo_epi8(src01, src12);
                ss_128[1] = _mm_unpacklo_epi8(src23, src34);
                ss_128[2] = _mm_unpacklo_epi8(src45, src56);

                do {
                    const __m128i res = y_convolve_8tap_2x2_ssse3(src_ptr, src_stride, coeffs_128, s_16, ss_128);
                    jnt_no_avg_round_store_2x2_sse2(res, offset_no_avg_128, dst, dst_stride);
                    ss_128[0] = ss_128[1];
                    ss_128[1] = ss_128[2];
                    ss_128[2] = ss_128[3];
                    src_ptr += 2 * src_stride;
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            } else {
                __m128i s_32[8], ss_128[4];

                assert(w == 4);

                s_32[0] = _mm_cvtsi32_si128(*(int32_t*)(src_ptr + 0 * src_stride));
                s_32[1] = _mm_cvtsi32_si128(*(int32_t*)(src_ptr + 1 * src_stride));
                s_32[2] = _mm_cvtsi32_si128(*(int32_t*)(src_ptr + 2 * src_stride));
                s_32[3] = _mm_cvtsi32_si128(*(int32_t*)(src_ptr + 3 * src_stride));
                s_32[4] = _mm_cvtsi32_si128(*(int32_t*)(src_ptr + 4 * src_stride));
                s_32[5] = _mm_cvtsi32_si128(*(int32_t*)(src_ptr + 5 * src_stride));
                s_32[6] = _mm_cvtsi32_si128(*(int32_t*)(src_ptr + 6 * src_stride));

                const __m128i src01 = _mm_unpacklo_epi32(s_32[0], s_32[1]);
                const __m128i src12 = _mm_unpacklo_epi32(s_32[1], s_32[2]);
                const __m128i src23 = _mm_unpacklo_epi32(s_32[2], s_32[3]);
                const __m128i src34 = _mm_unpacklo_epi32(s_32[3], s_32[4]);
                const __m128i src45 = _mm_unpacklo_epi32(s_32[4], s_32[5]);
                const __m128i src56 = _mm_unpacklo_epi32(s_32[5], s_32[6]);

                ss_128[0] = _mm_unpacklo_epi8(src01, src12);
                ss_128[1] = _mm_unpacklo_epi8(src23, src34);
                ss_128[2] = _mm_unpacklo_epi8(src45, src56);

                do {
                    const __m128i res = y_convolve_8tap_4x2_ssse3(src_ptr, src_stride, coeffs_128, s_32, ss_128);
                    jnt_no_avg_round_store_4x2_sse2(res, offset_no_avg_128, dst, dst_stride);
                    ss_128[0] = ss_128[1];
                    ss_128[1] = ss_128[2];
                    ss_128[2] = ss_128[3];
                    src_ptr += 2 * src_stride;
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            }
        } else {
            if (w <= 16) {
                const __m256i offset_no_avg_256 = _mm256_set1_epi16(offset_no_avg);

                prepare_half_coeffs_8tap_avx2(filter_params_y, subpel_y_q4, coeffs_256);

                if (w == 8) {
                    __m128i s_64[8];
                    __m256i ss_256[4];

                    s_64[0] = _mm_loadl_epi64((__m128i*)(src_ptr + 0 * src_stride));
                    s_64[1] = _mm_loadl_epi64((__m128i*)(src_ptr + 1 * src_stride));
                    s_64[2] = _mm_loadl_epi64((__m128i*)(src_ptr + 2 * src_stride));
                    s_64[3] = _mm_loadl_epi64((__m128i*)(src_ptr + 3 * src_stride));
                    s_64[4] = _mm_loadl_epi64((__m128i*)(src_ptr + 4 * src_stride));
                    s_64[5] = _mm_loadl_epi64((__m128i*)(src_ptr + 5 * src_stride));
                    s_64[6] = _mm_loadl_epi64((__m128i*)(src_ptr + 6 * src_stride));

                    // Load lines a and b. Line a to lower 128, line b to upper
                    // 128
                    const __m256i src01 = _mm256_setr_m128i(s_64[0], s_64[1]);
                    const __m256i src12 = _mm256_setr_m128i(s_64[1], s_64[2]);
                    const __m256i src23 = _mm256_setr_m128i(s_64[2], s_64[3]);
                    const __m256i src34 = _mm256_setr_m128i(s_64[3], s_64[4]);
                    const __m256i src45 = _mm256_setr_m128i(s_64[4], s_64[5]);
                    const __m256i src56 = _mm256_setr_m128i(s_64[5], s_64[6]);

                    ss_256[0] = _mm256_unpacklo_epi8(src01, src12);
                    ss_256[1] = _mm256_unpacklo_epi8(src23, src34);
                    ss_256[2] = _mm256_unpacklo_epi8(src45, src56);

                    y = h;
                    do {
                        const __m256i res = y_convolve_8tap_8x2_avx2(src_ptr, src_stride, coeffs_256, s_64, ss_256);
                        jnt_no_avg_round_store_8x2_avx2(res, offset_no_avg_256, dst, dst_stride);
                        ss_256[0] = ss_256[1];
                        ss_256[1] = ss_256[2];
                        ss_256[2] = ss_256[3];
                        src_ptr += 2 * src_stride;
                        dst += 2 * dst_stride;
                        y -= 2;
                    } while (y);
                } else {
                    __m128i s_128[8];
                    __m256i ss_256[8], r[2];

                    assert(w == 16);

                    s_128[0] = _mm_loadu_si128((__m128i*)(src_ptr + 0 * src_stride));
                    s_128[1] = _mm_loadu_si128((__m128i*)(src_ptr + 1 * src_stride));
                    s_128[2] = _mm_loadu_si128((__m128i*)(src_ptr + 2 * src_stride));
                    s_128[3] = _mm_loadu_si128((__m128i*)(src_ptr + 3 * src_stride));
                    s_128[4] = _mm_loadu_si128((__m128i*)(src_ptr + 4 * src_stride));
                    s_128[5] = _mm_loadu_si128((__m128i*)(src_ptr + 5 * src_stride));
                    s_128[6] = _mm_loadu_si128((__m128i*)(src_ptr + 6 * src_stride));

                    // Load lines a and b. Line a to lower 128, line b to upper
                    // 128
                    const __m256i src01 = _mm256_setr_m128i(s_128[0], s_128[1]);
                    const __m256i src12 = _mm256_setr_m128i(s_128[1], s_128[2]);
                    const __m256i src23 = _mm256_setr_m128i(s_128[2], s_128[3]);
                    const __m256i src34 = _mm256_setr_m128i(s_128[3], s_128[4]);
                    const __m256i src45 = _mm256_setr_m128i(s_128[4], s_128[5]);
                    const __m256i src56 = _mm256_setr_m128i(s_128[5], s_128[6]);

                    ss_256[0] = _mm256_unpacklo_epi8(src01, src12);
                    ss_256[1] = _mm256_unpacklo_epi8(src23, src34);
                    ss_256[2] = _mm256_unpacklo_epi8(src45, src56);

                    ss_256[4] = _mm256_unpackhi_epi8(src01, src12);
                    ss_256[5] = _mm256_unpackhi_epi8(src23, src34);
                    ss_256[6] = _mm256_unpackhi_epi8(src45, src56);

                    y = h;
                    do {
                        y_convolve_8tap_16x2_avx2(src_ptr, src_stride, coeffs_256, s_128, ss_256, r);
                        jnt_no_avg_round_store_16x2_avx2(r, offset_no_avg_256, dst, dst_stride);
                        ss_256[0] = ss_256[1];
                        ss_256[1] = ss_256[2];
                        ss_256[2] = ss_256[3];
                        ss_256[4] = ss_256[5];
                        ss_256[5] = ss_256[6];
                        ss_256[6] = ss_256[7];
                        src_ptr += 2 * src_stride;
                        dst += 2 * dst_stride;
                        y -= 2;
                    } while (y);
                }
            } else {
                const __m512i offset_no_avg_512 = _mm512_set1_epi16(offset_no_avg);

                prepare_half_coeffs_8tap_avx512(filter_params_y, subpel_y_q4, coeffs_512);

                if (w == 32) {
                    __m256i s_256[7];
                    __m512i s_512[6], ss_512[8], r[2];

                    s_256[0] = _mm256_loadu_si256((__m256i*)(src_ptr + 0 * src_stride));
                    s_256[1] = _mm256_loadu_si256((__m256i*)(src_ptr + 1 * src_stride));
                    s_256[2] = _mm256_loadu_si256((__m256i*)(src_ptr + 2 * src_stride));
                    s_256[3] = _mm256_loadu_si256((__m256i*)(src_ptr + 3 * src_stride));
                    s_256[4] = _mm256_loadu_si256((__m256i*)(src_ptr + 4 * src_stride));
                    s_256[5] = _mm256_loadu_si256((__m256i*)(src_ptr + 5 * src_stride));
                    s_256[6] = _mm256_loadu_si256((__m256i*)(src_ptr + 6 * src_stride));

                    s_512[0] = _mm512_setr_m256i(s_256[0], s_256[1]);
                    s_512[1] = _mm512_setr_m256i(s_256[1], s_256[2]);
                    s_512[2] = _mm512_setr_m256i(s_256[2], s_256[3]);
                    s_512[3] = _mm512_setr_m256i(s_256[3], s_256[4]);
                    s_512[4] = _mm512_setr_m256i(s_256[4], s_256[5]);
                    s_512[5] = _mm512_setr_m256i(s_256[5], s_256[6]);

                    ss_512[0] = _mm512_unpacklo_epi8(s_512[0], s_512[1]);
                    ss_512[1] = _mm512_unpacklo_epi8(s_512[2], s_512[3]);
                    ss_512[2] = _mm512_unpacklo_epi8(s_512[4], s_512[5]);
                    ss_512[4] = _mm512_unpackhi_epi8(s_512[0], s_512[1]);
                    ss_512[5] = _mm512_unpackhi_epi8(s_512[2], s_512[3]);
                    ss_512[6] = _mm512_unpackhi_epi8(s_512[4], s_512[5]);

                    y = h;
                    do {
                        src_ptr += 2 * src_stride;
                        y_convolve_8tap_32x2_avx512(src_ptr, src_stride, coeffs_512, s_256, ss_512, r);
                        jnt_no_avg_round_store_32x2_avx512(r, offset_no_avg_512, dst, dst_stride);

                        ss_512[0] = ss_512[1];
                        ss_512[1] = ss_512[2];
                        ss_512[2] = ss_512[3];
                        ss_512[4] = ss_512[5];
                        ss_512[5] = ss_512[6];
                        ss_512[6] = ss_512[7];
                        dst += 2 * dst_stride;
                        y -= 2;
                    } while (y);
                } else {
                    __m512i s_512[8], ss_512[8], tt_512[8], r[4];

                    assert(!(w % 64));

                    x = 0;
                    do {
                        const uint8_t* s = src_ptr + x;
                        ConvBufType*   d = dst + x;

                        s_512[0] = _mm512_loadu_si512((__m512i*)(s + 0 * src_stride));
                        s_512[1] = _mm512_loadu_si512((__m512i*)(s + 1 * src_stride));
                        s_512[2] = _mm512_loadu_si512((__m512i*)(s + 2 * src_stride));
                        s_512[3] = _mm512_loadu_si512((__m512i*)(s + 3 * src_stride));
                        s_512[4] = _mm512_loadu_si512((__m512i*)(s + 4 * src_stride));
                        s_512[5] = _mm512_loadu_si512((__m512i*)(s + 5 * src_stride));
                        s_512[6] = _mm512_loadu_si512((__m512i*)(s + 6 * src_stride));

                        ss_512[0] = _mm512_unpacklo_epi8(s_512[0], s_512[1]);
                        ss_512[1] = _mm512_unpacklo_epi8(s_512[2], s_512[3]);
                        ss_512[2] = _mm512_unpacklo_epi8(s_512[4], s_512[5]);
                        ss_512[4] = _mm512_unpackhi_epi8(s_512[0], s_512[1]);
                        ss_512[5] = _mm512_unpackhi_epi8(s_512[2], s_512[3]);
                        ss_512[6] = _mm512_unpackhi_epi8(s_512[4], s_512[5]);

                        tt_512[0] = _mm512_unpacklo_epi8(s_512[1], s_512[2]);
                        tt_512[1] = _mm512_unpacklo_epi8(s_512[3], s_512[4]);
                        tt_512[2] = _mm512_unpacklo_epi8(s_512[5], s_512[6]);
                        tt_512[4] = _mm512_unpackhi_epi8(s_512[1], s_512[2]);
                        tt_512[5] = _mm512_unpackhi_epi8(s_512[3], s_512[4]);
                        tt_512[6] = _mm512_unpackhi_epi8(s_512[5], s_512[6]);

                        y = h;
                        do {
                            y_convolve_8tap_64x2_avx512(s, src_stride, coeffs_512, s_512, ss_512, tt_512, r);
                            jnt_no_avg_round_store_64_avx512(r, offset_no_avg_512, d);
                            jnt_no_avg_round_store_64_avx512(r + 2, offset_no_avg_512, d + dst_stride);

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
                            s += 2 * src_stride;
                            d += 2 * dst_stride;
                            y -= 2;
                        } while (y);

                        x += 64;
                    } while (x < w);
                }
            }
        }
    }
}

typedef void (*JntConvolveYTapFunc)(const uint8_t* const src, const int32_t src_stride, uint8_t* dst8,
                                    const int32_t dst8_stride, const int32_t w, const int32_t h,
                                    const InterpFilterParams* const filter_params_y, const int32_t subpel_y_q4,
                                    const ConvolveParams* const conv_params);

void svt_av1_jnt_convolve_y_avx512(const uint8_t* src, int32_t src_stride, uint8_t* dst8, int32_t dst8_stride,
                                   int32_t w, int32_t h, const InterpFilterParams* filter_params_x,
                                   const InterpFilterParams* filter_params_y, const int32_t subpel_x_q4,
                                   const int32_t subpel_y_q4, ConvolveParams* conv_params) {
    static const JntConvolveYTapFunc jnt_convolve_y_tap_func_table[MAX_FILTER_TAP + 1] = {NULL,
                                                                                          NULL,
                                                                                          jnt_convolve_y_2tap_avx512,
                                                                                          NULL,
                                                                                          jnt_convolve_y_4tap_avx2,
                                                                                          NULL,
                                                                                          jnt_convolve_y_6tap_avx512,
                                                                                          NULL,
                                                                                          jnt_convolve_y_8tap_avx512};
    const int32_t                    tap_y = get_convolve_tap(filter_params_y->filter_ptr);

    (void)filter_params_x;
    (void)subpel_x_q4;

    assert(conv_params->round_0 == 3);
    assert(conv_params->round_1 == COMPOUND_ROUND1_BITS);

    jnt_convolve_y_tap_func_table[tap_y](
        src, src_stride, dst8, dst8_stride, w, h, filter_params_y, subpel_y_q4, conv_params);
}

// =============================================================================

static INLINE void jnt_copy_avg_round_store_32_avx512(const __m512i res, const __m512i offset,
                                                      const ConvBufType* const dst, uint8_t* const dst8) {
    __m512i d;

    const __m512i r = _mm512_add_epi16(res, offset);
    d               = _mm512_loadu_si512((__m512i*)(dst + 0 * 16));
    d               = jnt_avg_32_avx512(r, d);
    pack_store_32_avx512(d, dst8);
}

static INLINE void jnt_copy_avg_32_avx512(const uint8_t* const src, const __m512i offset_avg_512,
                                          const ConvBufType* const dst, uint8_t* const dst8) {
    const __m512i res = jnt_copy_load_src_32_avx512(src);
    jnt_copy_avg_round_store_32_avx512(res, offset_avg_512, dst, dst8);
}

static INLINE void jnt_copy_no_avg_32_avx512(const uint8_t* const src, const __m512i offset_no_avg_512,
                                             const ConvBufType* const dst) {
    const __m512i res = jnt_copy_load_src_32_avx512(src);
    const __m512i d   = _mm512_add_epi16(res, offset_no_avg_512);
    _mm512_storeu_si512((__m512i*)dst, d);
}

void svt_av1_jnt_convolve_2d_copy_avx512(const uint8_t* src, int32_t src_stride, uint8_t* dst8, int32_t dst8_stride,
                                         int32_t w, int32_t h, const InterpFilterParams* filter_params_x,
                                         const InterpFilterParams* filter_params_y, const int32_t subpel_x_q4,
                                         const int32_t subpel_y_q4, ConvolveParams* conv_params) {
    const int32_t round_0      = 3;
    const int32_t round_1      = COMPOUND_ROUND1_BITS;
    const int32_t bits         = 2 * FILTER_BITS - round_0 - round_1;
    const int32_t bd           = 8;
    const int32_t offset_bits  = bd + bits;
    const int32_t round_offset = (1 << offset_bits) + (1 << (offset_bits - 1));
    ConvBufType*  dst          = conv_params->dst;
    int32_t       dst_stride   = conv_params->dst_stride;

    (void)filter_params_x;
    (void)filter_params_y;
    (void)subpel_x_q4;
    (void)subpel_y_q4;

    if (conv_params->do_average) {
        if (conv_params->use_jnt_comp_avg) {
            const int32_t factor          = conv_params->fwd_offset | (conv_params->bck_offset << 16);
            const int32_t offset_comp_avg = round_offset * conv_params->bck_offset +
                (1 << (bits + DIST_PRECISION_BITS - 1)) - (round_offset << DIST_PRECISION_BITS);

            if (w <= 4) {
                const __m128i factor_128          = _mm_set1_epi32(factor);
                const __m128i offset_comp_avg_128 = _mm_set1_epi32(offset_comp_avg);

                if (w == 2) {
                    do {
                        const __m128i res = jnt_copy_load_src_2x2_sse2(src, src_stride);
                        jnt_comp_avg_round_store_2x2_kernel_sse2(
                            res, factor_128, offset_comp_avg_128, dst, dst_stride, dst8, dst8_stride);
                        src += 2 * src_stride;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        h -= 2;
                    } while (h);
                } else {
                    assert(w == 4);

                    do {
                        const __m128i res = jnt_copy_load_src_4x2_sse4_1(src, src_stride);
                        jnt_comp_avg_round_store_4x2_kernel_sse2(
                            res, factor_128, offset_comp_avg_128, dst, dst_stride, dst8, dst8_stride);
                        src += 2 * src_stride;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        h -= 2;
                    } while (h);
                }
            } else if (w <= 16) {
                const __m256i factor_256          = _mm256_set1_epi32(factor);
                const __m256i offset_comp_avg_256 = _mm256_set1_epi32(offset_comp_avg);

                if (w == 8) {
                    do {
                        const __m256i res = jnt_copy_load_src_8x2_avx2(src, src_stride);
                        jnt_comp_avg_round_store_8x2_kernel_avx2(
                            res, factor_256, offset_comp_avg_256, dst, dst_stride, dst8, dst8_stride);
                        src += 2 * src_stride;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        h -= 2;
                    } while (h);
                } else {
                    assert(w == 16);

                    do {
                        __m256i res[2];
                        res[0] = jnt_copy_load_src_16_avx2(src);
                        res[1] = jnt_copy_load_src_16_avx2(src + src_stride);
                        jnt_comp_avg_round_store_16x2_kernel_avx2(
                            res, factor_256, offset_comp_avg_256, dst, dst_stride, dst8, dst8_stride);
                        src += 2 * src_stride;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        h -= 2;
                    } while (h);
                }
            } else {
                const __m512i factor_512          = _mm512_set1_epi32(factor);
                const __m512i offset_comp_avg_512 = _mm512_set1_epi32(offset_comp_avg);

                if (w == 32) {
                    do {
                        jnt_copy_comp_avg_32_avx512(src, factor_512, offset_comp_avg_512, dst, dst8);
                        src += src_stride;
                        dst += dst_stride;
                        dst8 += dst8_stride;
                    } while (--h);
                } else if (w == 64) {
                    do {
                        jnt_copy_comp_avg_32_avx512(
                            src + 0 * 32, factor_512, offset_comp_avg_512, dst + 0 * 32, dst8 + 0 * 32);
                        jnt_copy_comp_avg_32_avx512(
                            src + 1 * 32, factor_512, offset_comp_avg_512, dst + 1 * 32, dst8 + 1 * 32);
                        src += src_stride;
                        dst += dst_stride;
                        dst8 += dst8_stride;
                    } while (--h);
                } else {
                    assert(w == 128);

                    do {
                        jnt_copy_comp_avg_32_avx512(
                            src + 0 * 32, factor_512, offset_comp_avg_512, dst + 0 * 32, dst8 + 0 * 32);
                        jnt_copy_comp_avg_32_avx512(
                            src + 1 * 32, factor_512, offset_comp_avg_512, dst + 1 * 32, dst8 + 1 * 32);
                        jnt_copy_comp_avg_32_avx512(
                            src + 2 * 32, factor_512, offset_comp_avg_512, dst + 2 * 32, dst8 + 2 * 32);
                        jnt_copy_comp_avg_32_avx512(
                            src + 3 * 32, factor_512, offset_comp_avg_512, dst + 3 * 32, dst8 + 3 * 32);
                        src += src_stride;
                        dst += dst_stride;
                        dst8 += dst8_stride;
                    } while (--h);
                }
            }
        } else {
            const int16_t offset_avg = (1 << bits) - round_offset;

            if (w <= 4) {
                const __m128i offset_avg_128 = _mm_set1_epi16(offset_avg);

                if (w == 2) {
                    do {
                        const __m128i res = jnt_copy_load_src_2x2_sse2(src, src_stride);
                        jnt_copy_avg_round_store_2x2_sse2(res, offset_avg_128, dst, dst_stride, dst8, dst8_stride);
                        src += 2 * src_stride;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        h -= 2;
                    } while (h);
                } else {
                    assert(w == 4);

                    do {
                        const __m128i res = jnt_copy_load_src_4x2_sse4_1(src, src_stride);
                        jnt_copy_avg_round_store_4x2_sse2(res, offset_avg_128, dst, dst_stride, dst8, dst8_stride);
                        src += 2 * src_stride;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        h -= 2;
                    } while (h);
                }
            } else if (w <= 16) {
                const __m256i offset_avg_256 = _mm256_set1_epi16(offset_avg);

                if (w == 8) {
                    do {
                        const __m256i res = jnt_copy_load_src_8x2_avx2(src, src_stride);
                        jnt_copy_avg_round_store_8x2_avx2(res, offset_avg_256, dst, dst_stride, dst8, dst8_stride);
                        src += 2 * src_stride;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        h -= 2;
                    } while (h);
                } else {
                    assert(w == 16);

                    do {
                        __m256i res[2];
                        res[0] = jnt_copy_load_src_16_avx2(src);
                        res[1] = jnt_copy_load_src_16_avx2(src + src_stride);
                        jnt_copy_avg_round_store_16x2_avx2(res, offset_avg_256, dst, dst_stride, dst8, dst8_stride);
                        src += 2 * src_stride;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        h -= 2;
                    } while (h);
                }
            } else {
                const __m512i offset_avg_512 = _mm512_set1_epi16(offset_avg);

                if (w == 32) {
                    do {
                        jnt_copy_avg_32_avx512(src, offset_avg_512, dst, dst8);
                        src += src_stride;
                        dst += dst_stride;
                        dst8 += dst8_stride;
                    } while (--h);
                } else if (w == 64) {
                    do {
                        jnt_copy_avg_32_avx512(src + 0 * 32, offset_avg_512, dst + 0 * 32, dst8 + 0 * 32);
                        jnt_copy_avg_32_avx512(src + 1 * 32, offset_avg_512, dst + 1 * 32, dst8 + 1 * 32);
                        src += src_stride;
                        dst += dst_stride;
                        dst8 += dst8_stride;
                    } while (--h);
                } else {
                    assert(w == 128);

                    do {
                        jnt_copy_avg_32_avx512(src + 0 * 32, offset_avg_512, dst + 0 * 32, dst8 + 0 * 32);
                        jnt_copy_avg_32_avx512(src + 1 * 32, offset_avg_512, dst + 1 * 32, dst8 + 1 * 32);
                        jnt_copy_avg_32_avx512(src + 2 * 32, offset_avg_512, dst + 2 * 32, dst8 + 2 * 32);
                        jnt_copy_avg_32_avx512(src + 3 * 32, offset_avg_512, dst + 3 * 32, dst8 + 3 * 32);
                        src += src_stride;
                        dst += dst_stride;
                        dst8 += dst8_stride;
                    } while (--h);
                }
            }
        }
    } else {
        const int32_t offset_no_avg = (1 << offset_bits) + (1 << (offset_bits - 1));

        if (w <= 4) {
            const __m128i offset_no_avg_128 = _mm_set1_epi16(offset_no_avg);

            if (w == 2) {
                do {
                    const __m128i res              = jnt_copy_load_src_2x2_sse2(src, src_stride);
                    const __m128i r                = _mm_add_epi16(res, offset_no_avg_128);
                    *(uint32_t*)dst                = _mm_cvtsi128_si32(r);
                    *(uint32_t*)(dst + dst_stride) = _mm_extract_epi32(r, 1);
                    src += 2 * src_stride;
                    dst += 2 * dst_stride;
                    h -= 2;
                } while (h);
            } else {
                assert(w == 4);

                do {
                    const __m128i res = jnt_copy_load_src_4x2_sse4_1(src, src_stride);
                    const __m128i r   = _mm_add_epi16(res, offset_no_avg_128);
                    store_u16_4x2_sse2(r, dst, dst_stride);
                    src += 2 * src_stride;
                    dst += 2 * dst_stride;
                    h -= 2;
                } while (h);
            }
        } else if (w <= 16) {
            const __m256i offset_no_avg_256 = _mm256_set1_epi16(offset_no_avg);

            if (w == 8) {
                do {
                    const __m256i res = jnt_copy_load_src_8x2_avx2(src, src_stride);
                    const __m256i r   = _mm256_add_epi16(res, offset_no_avg_256);
                    storeu_u16_8x2_avx2(r, dst, dst_stride);
                    src += 2 * src_stride;
                    dst += 2 * dst_stride;
                    h -= 2;
                } while (h);
            } else {
                assert(w == 16);

                do {
                    __m256i d[2];
                    d[0] = jnt_copy_load_src_16_avx2(src);
                    d[1] = jnt_copy_load_src_16_avx2(src + src_stride);
                    d[0] = _mm256_add_epi16(d[0], offset_no_avg_256);
                    d[1] = _mm256_add_epi16(d[1], offset_no_avg_256);
                    _mm256_storeu_si256((__m256i*)(dst + 0 * dst_stride), d[0]);
                    _mm256_storeu_si256((__m256i*)(dst + 1 * dst_stride), d[1]);
                    src += 2 * src_stride;
                    dst += 2 * dst_stride;
                    h -= 2;
                } while (h);
            }
        } else {
            const __m512i offset_no_avg_512 = _mm512_set1_epi16(offset_no_avg);

            if (w == 32) {
                do {
                    jnt_copy_no_avg_32_avx512(src, offset_no_avg_512, dst);
                    src += src_stride;
                    dst += dst_stride;
                } while (--h);
            } else if (w == 64) {
                do {
                    jnt_copy_no_avg_32_avx512(src + 0 * 32, offset_no_avg_512, dst + 0 * 32);
                    jnt_copy_no_avg_32_avx512(src + 1 * 32, offset_no_avg_512, dst + 1 * 32);
                    src += src_stride;
                    dst += dst_stride;
                } while (--h);
            } else {
                assert(w == 128);

                do {
                    jnt_copy_no_avg_32_avx512(src + 0 * 32, offset_no_avg_512, dst + 0 * 32);
                    jnt_copy_no_avg_32_avx512(src + 1 * 32, offset_no_avg_512, dst + 1 * 32);
                    jnt_copy_no_avg_32_avx512(src + 2 * 32, offset_no_avg_512, dst + 2 * 32);
                    jnt_copy_no_avg_32_avx512(src + 3 * 32, offset_no_avg_512, dst + 3 * 32);
                    src += src_stride;
                    dst += dst_stride;
                } while (--h);
            }
        }
    }
}

// =============================================================================

SIMD_INLINE void jnt_x_comp_avg_2tap_32x2_avx512(const uint8_t* const src, const int32_t src_stride,
                                                 const __m512i* const coeffs, const __m512i factor,
                                                 const __m512i offset, const ConvBufType* const dst,
                                                 const int32_t dst_stride, uint8_t* const dst8,
                                                 const int32_t dst8_stride) {
    __m512i r[2];

    x_convolve_2tap_32x2_avx512(src, src_stride, coeffs, r);
    jnt_comp_avg_round_store_32x2_avx512(r, factor, offset, dst, dst_stride, dst8, dst8_stride);
}

SIMD_INLINE void jnt_x_comp_avg_2tap_64_avx512(const uint8_t* const src, const __m512i* const coeffs,
                                               const __m512i factor, const __m512i offset, ConvBufType* const dst,
                                               uint8_t* const dst8) {
    __m512i r[2];

    x_convolve_2tap_64_avx512(src, coeffs, r);
    jnt_comp_avg_round_store_64_avx512(r, factor, offset, dst, dst8);
}

static INLINE void jnt_x_avg_2tap_32x2_avx512(const uint8_t* const src, const int32_t src_stride,
                                              const __m512i* const coeffs, const __m512i offset,
                                              const ConvBufType* const dst, const int32_t dst_stride,
                                              uint8_t* const dst8, const int32_t dst8_stride) {
    __m512i r[2];

    x_convolve_2tap_32x2_avx512(src, src_stride, coeffs, r);
    jnt_avg_round_store_32x2_avx512(r, offset, dst, dst_stride, dst8, dst8_stride);
}

static INLINE void jnt_x_avg_2tap_64_avx512(const uint8_t* const src, const __m512i* const coeffs, const __m512i offset,
                                            const ConvBufType* const dst, uint8_t* const dst8) {
    __m512i r[2];

    x_convolve_2tap_64_avx512(src, coeffs, r);
    jnt_avg_round_store_64_avx512(r, offset, dst, dst8);
}

static INLINE void jnt_x_no_avg_2tap_32x2_avx512(const uint8_t* const src, const int32_t src_stride,
                                                 const __m512i* const coeffs, const __m512i offset,
                                                 ConvBufType* const dst, const int32_t dst_stride) {
    __m512i r[2];

    x_convolve_2tap_32x2_avx512(src, src_stride, coeffs, r);
    jnt_no_avg_round_store_32x2_avx512(r, offset, dst, dst_stride);
}

static INLINE void jnt_x_no_avg_2tap_64_avx512(const uint8_t* const src, const __m512i* const coeffs,
                                               const __m512i offset, ConvBufType* const dst) {
    __m512i r[2];

    x_convolve_2tap_64_avx512(src, coeffs, r);
    jnt_no_avg_round_store_64_avx512(r, offset, dst);
}

SIMD_INLINE void jnt_x_comp_avg_6tap_16x2_avx2(const uint8_t* const src, const int32_t src_stride,
                                               const __m256i coeffs[3], const __m256i filt[3], const __m256i factor,
                                               const __m256i offset, ConvBufType* const dst, const int32_t dst_stride,
                                               uint8_t* const dst8, const int32_t dst8_stride) {
    __m256i r[2];

    x_convolve_6tap_16x2_avx2(src, src_stride, coeffs, filt, r);
    jnt_comp_avg_round_store_16x2_avx2(r, factor, offset, dst, dst_stride, dst8, dst8_stride);
}

SIMD_INLINE void jnt_x_comp_avg_6tap_32x2_avx512(const uint8_t* const src, const int32_t src_stride,
                                                 const __m512i coeffs[3], const __m512i filt[3], const __m512i factor,
                                                 const __m512i offset, ConvBufType* const dst, const int32_t dst_stride,
                                                 uint8_t* const dst8, const int32_t dst8_stride) {
    __m512i r[2];

    x_convolve_6tap_32x2_avx512(src, src_stride, coeffs, filt, r);
    jnt_comp_avg_round_store_32x2_avx512(r, factor, offset, dst, dst_stride, dst8, dst8_stride);
}

SIMD_INLINE void jnt_x_comp_avg_6tap_64_avx512(const uint8_t* const src, const __m512i coeffs[3], const __m512i filt[3],
                                               const __m512i factor, const __m512i offset, ConvBufType* const dst,
                                               uint8_t* const dst8) {
    __m512i r[2];

    x_convolve_6tap_64_avx512(src, coeffs, filt, r);
    jnt_comp_avg_round_store_64_avx512(r, factor, offset, dst, dst8);
}

SIMD_INLINE void jnt_x_avg_6tap_16x2_avx2(const uint8_t* const src, const int32_t src_stride, const __m256i coeffs[3],
                                          const __m256i filt[3], const __m256i offset, ConvBufType* const dst,
                                          const int32_t dst_stride, uint8_t* const dst8, const int32_t dst8_stride) {
    __m256i r[2];

    x_convolve_6tap_16x2_avx2(src, src_stride, coeffs, filt, r);
    jnt_avg_round_store_16x2_avx2(r, offset, dst, dst_stride, dst8, dst8_stride);
}

SIMD_INLINE void jnt_x_avg_6tap_32x2_avx512(const uint8_t* const src, const int32_t src_stride, const __m512i coeffs[3],
                                            const __m512i filt[3], const __m512i offset, ConvBufType* const dst,
                                            const int32_t dst_stride, uint8_t* const dst8, const int32_t dst8_stride) {
    __m512i r[2];

    x_convolve_6tap_32x2_avx512(src, src_stride, coeffs, filt, r);
    jnt_avg_round_store_32x2_avx512(r, offset, dst, dst_stride, dst8, dst8_stride);
}

SIMD_INLINE void jnt_x_avg_6tap_64_avx512(const uint8_t* const src, const __m512i coeffs[3], const __m512i filt[3],
                                          const __m512i offset, ConvBufType* const dst, uint8_t* const dst8) {
    __m512i r[2];

    x_convolve_6tap_64_avx512(src, coeffs, filt, r);
    jnt_avg_round_store_64_avx512(r, offset, dst, dst8);
}

SIMD_INLINE void jnt_x_no_avg_6tap_16x2_avx2(const uint8_t* const src, const int32_t src_stride,
                                             const __m256i coeffs[3], const __m256i filt[3], const __m256i offset,
                                             ConvBufType* const dst, const int32_t dst_stride) {
    __m256i r[2];

    x_convolve_6tap_16x2_avx2(src, src_stride, coeffs, filt, r);
    jnt_no_avg_round_store_16x2_avx2(r, offset, dst, dst_stride);
}

static INLINE void jnt_x_no_avg_6tap_32x2_avx512(const uint8_t* const src, const int32_t src_stride,
                                                 const __m512i coeffs[3], const __m512i filt[3], const __m512i offset,
                                                 ConvBufType* const dst, const int32_t dst_stride) {
    __m512i r[2];

    x_convolve_6tap_32x2_avx512(src, src_stride, coeffs, filt, r);
    jnt_no_avg_round_store_32x2_avx512(r, offset, dst, dst_stride);
}

SIMD_INLINE void jnt_x_no_avg_6tap_64_avx512(const uint8_t* const src, const __m512i coeffs[3], const __m512i filt[3],
                                             const __m512i offset, ConvBufType* const dst) {
    __m512i r[2];

    x_convolve_6tap_64_avx512(src, coeffs, filt, r);
    jnt_no_avg_round_store_64_avx512(r, offset, dst);
}

static INLINE void jnt_x_comp_avg_8tap_16x2_avx2(const uint8_t* const src, const int32_t src_stride,
                                                 const __m256i coeffs[4], const __m256i filt[4], const __m256i factor,
                                                 const __m256i offset, ConvBufType* const dst, const int32_t dst_stride,
                                                 uint8_t* const dst8, const int32_t dst8_stride) {
    __m256i r[2];

    x_convolve_8tap_16x2_avx2(src, src_stride, coeffs, filt, r);
    jnt_comp_avg_round_store_16x2_avx2(r, factor, offset, dst, dst_stride, dst8, dst8_stride);
}

SIMD_INLINE void jnt_x_comp_avg_8tap_32x2_avx512(const uint8_t* const src, const int32_t src_stride,
                                                 const __m512i coeffs[4], const __m512i filt[4], const __m512i factor,
                                                 const __m512i offset, ConvBufType* const dst, const int32_t dst_stride,
                                                 uint8_t* const dst8, const int32_t dst8_stride) {
    __m512i r[2];

    x_convolve_8tap_32x2_avx512(src, src_stride, coeffs, filt, r);
    jnt_comp_avg_round_store_32x2_avx512(r, factor, offset, dst, dst_stride, dst8, dst8_stride);
}

SIMD_INLINE void jnt_x_comp_avg_8tap_64_avx512(const uint8_t* const src, const __m512i coeffs[4], const __m512i filt[4],
                                               const __m512i factor, const __m512i offset, ConvBufType* const dst,
                                               uint8_t* const dst8) {
    __m512i r[2];

    x_convolve_8tap_64_avx512(src, coeffs, filt, r);
    jnt_comp_avg_round_store_64_avx512(r, factor, offset, dst, dst8);
}

SIMD_INLINE void jnt_x_avg_8tap_16x2_avx2(const uint8_t* const src, const int32_t src_stride, const __m256i coeffs[4],
                                          const __m256i filt[4], const __m256i offset, ConvBufType* const dst,
                                          const int32_t dst_stride, uint8_t* const dst8, const int32_t dst8_stride) {
    __m256i r[2];

    x_convolve_8tap_16x2_avx2(src, src_stride, coeffs, filt, r);
    jnt_avg_round_store_16x2_avx2(r, offset, dst, dst_stride, dst8, dst8_stride);
}

SIMD_INLINE void jnt_x_avg_8tap_32x2_avx512(const uint8_t* const src, const int32_t src_stride, const __m512i coeffs[4],
                                            const __m512i filt[4], const __m512i offset, ConvBufType* const dst,
                                            const int32_t dst_stride, uint8_t* const dst8, const int32_t dst8_stride) {
    __m512i r[2];

    x_convolve_8tap_32x2_avx512(src, src_stride, coeffs, filt, r);
    jnt_avg_round_store_32x2_avx512(r, offset, dst, dst_stride, dst8, dst8_stride);
}

SIMD_INLINE void jnt_x_avg_8tap_64_avx512(const uint8_t* const src, const __m512i coeffs[4], const __m512i filt[4],
                                          const __m512i offset, ConvBufType* const dst, uint8_t* const dst8) {
    __m512i r[2];

    x_convolve_8tap_64_avx512(src, coeffs, filt, r);
    jnt_avg_round_store_64_avx512(r, offset, dst, dst8);
}

static INLINE void jnt_x_no_avg_8tap_16x2_avx2(const uint8_t* const src, const int32_t src_stride,
                                               const __m256i coeffs[4], const __m256i filt[4], const __m256i offset,
                                               ConvBufType* const dst, const int32_t dst_stride) {
    __m256i r[2];

    x_convolve_8tap_16x2_avx2(src, src_stride, coeffs, filt, r);
    jnt_no_avg_round_store_16x2_avx2(r, offset, dst, dst_stride);
}

SIMD_INLINE void jnt_x_no_avg_8tap_32x2_avx512(const uint8_t* const src, const int32_t src_stride,
                                               const __m512i coeffs[4], const __m512i filt[4], const __m512i offset,
                                               ConvBufType* const dst, const int32_t dst_stride) {
    __m512i r[2];

    x_convolve_8tap_32x2_avx512(src, src_stride, coeffs, filt, r);
    jnt_no_avg_round_store_32x2_avx512(r, offset, dst, dst_stride);
}

SIMD_INLINE void jnt_x_no_avg_8tap_64_avx512(const uint8_t* const src, const __m512i coeffs[4], const __m512i filt[4],
                                             const __m512i offset, ConvBufType* const dst) {
    __m512i r[2];

    x_convolve_8tap_64_avx512(src, coeffs, filt, r);
    jnt_no_avg_round_store_64_avx512(r, offset, dst);
}

static void jnt_convolve_x_2tap_avx512(const uint8_t* const src, const int32_t src_stride, uint8_t* dst8,
                                       const int32_t dst8_stride, const int32_t w, const int32_t h,
                                       const InterpFilterParams* const filter_params_x, const int32_t subpel_x_q4,
                                       const ConvolveParams* const conv_params) {
    const uint8_t* src_ptr      = src;
    const int32_t  dst_stride   = conv_params->dst_stride;
    const int32_t  round_0      = 3;
    const int32_t  round_1      = COMPOUND_ROUND1_BITS;
    const int32_t  bits         = FILTER_BITS - round_1;
    const int32_t  bd           = 8;
    const int32_t  offset_bits  = bd + 2 * FILTER_BITS - round_0 - round_1;
    const int32_t  round_offset = (1 << offset_bits) + (1 << (offset_bits - 1));
    ConvBufType*   dst          = conv_params->dst;
    int32_t        y            = h;
    __m128i        coeffs_128[4];
    __m256i        coeffs_256[4];
    __m512i        coeffs_512[4];

    if (conv_params->do_average) {
        if (conv_params->use_jnt_comp_avg) {
            const int32_t round_bits      = 2 * FILTER_BITS - round_0 - round_1;
            const int32_t factor          = conv_params->fwd_offset | (conv_params->bck_offset << 16);
            const int32_t offset_comp_avg = round_offset * conv_params->bck_offset +
                (1 << (round_bits + DIST_PRECISION_BITS - 1)) - (round_offset << DIST_PRECISION_BITS);

            if (w <= 4) {
                const __m128i factor_128          = _mm_set1_epi32(factor);
                const __m128i offset_comp_avg_128 = _mm_set1_epi32(offset_comp_avg);

                prepare_half_coeffs_2tap_ssse3(filter_params_x, subpel_x_q4, coeffs_128);

                if (w == 2) {
                    do {
                        const __m128i res = x_convolve_2tap_2x2_sse4_1(src_ptr, src_stride, coeffs_128);
                        jnt_comp_avg_round_store_2x2_sse2(
                            res, factor_128, offset_comp_avg_128, dst, dst_stride, dst8, dst8_stride);
                        src_ptr += 2 * src_stride;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                } else {
                    assert(w == 4);

                    do {
                        const __m128i res = x_convolve_2tap_4x2_ssse3(src_ptr, src_stride, coeffs_128);
                        jnt_comp_avg_round_store_4x2_sse2(
                            res, factor_128, offset_comp_avg_128, dst, dst_stride, dst8, dst8_stride);
                        src_ptr += 2 * src_stride;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                }
            } else if (w <= 16) {
                const __m256i factor_256          = _mm256_set1_epi32(factor);
                const __m256i offset_comp_avg_256 = _mm256_set1_epi32(offset_comp_avg);
                __m256i       r[2];

                prepare_half_coeffs_2tap_avx2(filter_params_x, subpel_x_q4, coeffs_256);

                if (w == 8) {
                    do {
                        const __m256i res = x_convolve_2tap_8x2_avx2(src_ptr, src_stride, coeffs_256);
                        jnt_comp_avg_round_store_8x2_avx2(
                            res, factor_256, offset_comp_avg_256, dst, dst_stride, dst8, dst8_stride);
                        src_ptr += 2 * src_stride;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                } else {
                    assert(w == 16);

                    do {
                        x_convolve_2tap_16x2_avx2(src_ptr, src_stride, coeffs_256, r);
                        jnt_comp_avg_round_store_16x2_avx2(
                            r, factor_256, offset_comp_avg_256, dst, dst_stride, dst8, dst8_stride);
                        src_ptr += 2 * src_stride;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                }
            } else {
                const __m512i factor_512          = _mm512_set1_epi32(factor);
                const __m512i offset_comp_avg_512 = _mm512_set1_epi32(offset_comp_avg);
                prepare_half_coeffs_2tap_avx512(filter_params_x, subpel_x_q4, coeffs_512);

                if (w == 32) {
                    do {
                        jnt_x_comp_avg_2tap_32x2_avx512(src_ptr,
                                                        src_stride,
                                                        coeffs_512,
                                                        factor_512,
                                                        offset_comp_avg_512,
                                                        dst,
                                                        dst_stride,
                                                        dst8,
                                                        dst8_stride);
                        src_ptr += 2 * src_stride;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                } else if (w == 64) {
                    do {
                        jnt_x_comp_avg_2tap_64_avx512(src_ptr, coeffs_512, factor_512, offset_comp_avg_512, dst, dst8);
                        src_ptr += src_stride;
                        dst += dst_stride;
                        dst8 += dst8_stride;
                    } while (--y);
                } else {
                    assert(w == 128);

                    do {
                        jnt_x_comp_avg_2tap_64_avx512(src_ptr, coeffs_512, factor_512, offset_comp_avg_512, dst, dst8);
                        jnt_x_comp_avg_2tap_64_avx512(
                            src_ptr + 64, coeffs_512, factor_512, offset_comp_avg_512, dst + 64, dst8 + 64);
                        src_ptr += src_stride;
                        dst += dst_stride;
                        dst8 += dst8_stride;
                    } while (--y);
                }
            }
        } else {
            const int16_t offset_avg = (1 << (FILTER_BITS - 1)) + (1 << (round_0 - bits - 2)) -
                (round_offset << (round_0 - bits - 1));

            if (w <= 4) {
                const __m128i offset_avg_128 = _mm_set1_epi16(offset_avg);

                prepare_half_coeffs_2tap_ssse3(filter_params_x, subpel_x_q4, coeffs_128);

                if (w == 2) {
                    do {
                        const __m128i res = x_convolve_2tap_2x2_sse4_1(src_ptr, src_stride, coeffs_128);
                        jnt_avg_round_store_2x2_sse2(res, offset_avg_128, dst, dst_stride, dst8, dst8_stride);
                        src_ptr += 2 * src_stride;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                } else {
                    assert(w == 4);

                    do {
                        const __m128i res = x_convolve_2tap_4x2_ssse3(src_ptr, src_stride, coeffs_128);
                        jnt_avg_round_store_4x2_sse2(res, offset_avg_128, dst, dst_stride, dst8, dst8_stride);
                        src_ptr += 2 * src_stride;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                }
            } else if (w <= 16) {
                const __m256i offset_avg_256 = _mm256_set1_epi16(offset_avg);
                __m256i       r[2];

                prepare_half_coeffs_2tap_avx2(filter_params_x, subpel_x_q4, coeffs_256);

                if (w == 8) {
                    do {
                        const __m256i res = x_convolve_2tap_8x2_avx2(src_ptr, src_stride, coeffs_256);
                        jnt_avg_round_store_8x2_avx2(res, offset_avg_256, dst, dst_stride, dst8, dst8_stride);
                        src_ptr += 2 * src_stride;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                } else {
                    assert(w == 16);

                    do {
                        x_convolve_2tap_16x2_avx2(src_ptr, src_stride, coeffs_256, r);
                        jnt_avg_round_store_16x2_avx2(r, offset_avg_256, dst, dst_stride, dst8, dst8_stride);
                        src_ptr += 2 * src_stride;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                }
            } else {
                const __m512i offset_avg_512 = _mm512_set1_epi16(offset_avg);
                prepare_half_coeffs_2tap_avx512(filter_params_x, subpel_x_q4, coeffs_512);

                if (w == 32) {
                    do {
                        jnt_x_avg_2tap_32x2_avx512(
                            src_ptr, src_stride, coeffs_512, offset_avg_512, dst, dst_stride, dst8, dst8_stride);
                        src_ptr += 2 * src_stride;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                } else if (w == 64) {
                    do {
                        jnt_x_avg_2tap_64_avx512(src_ptr, coeffs_512, offset_avg_512, dst, dst8);
                        src_ptr += src_stride;
                        dst += dst_stride;
                        dst8 += dst8_stride;
                    } while (--y);
                } else {
                    assert(w == 128);

                    do {
                        jnt_x_avg_2tap_64_avx512(src_ptr, coeffs_512, offset_avg_512, dst, dst8);
                        jnt_x_avg_2tap_64_avx512(src_ptr + 64, coeffs_512, offset_avg_512, dst + 64, dst8 + 64);
                        src_ptr += src_stride;
                        dst += dst_stride;
                        dst8 += dst8_stride;
                    } while (--y);
                }
            }
        }
    } else {
        const int16_t offset_no_avg = (round_offset << (round_0 - bits - 1)) + (1 << (round_0 - bits - 2));

        if (w <= 4) {
            const __m128i offset_no_avg_128 = _mm_set1_epi16(offset_no_avg);

            prepare_half_coeffs_2tap_ssse3(filter_params_x, subpel_x_q4, coeffs_128);

            if (w == 2) {
                do {
                    const __m128i res = x_convolve_2tap_2x2_sse4_1(src_ptr, src_stride, coeffs_128);
                    jnt_no_avg_round_store_2x2_sse2(res, offset_no_avg_128, dst, dst_stride);
                    src_ptr += 2 * src_stride;
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            } else {
                assert(w == 4);

                do {
                    const __m128i res = x_convolve_2tap_4x2_ssse3(src_ptr, src_stride, coeffs_128);
                    jnt_no_avg_round_store_4x2_sse2(res, offset_no_avg_128, dst, dst_stride);
                    src_ptr += 2 * src_stride;
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            }
        } else if (w <= 16) {
            const __m256i offset_no_avg_256 = _mm256_set1_epi16(offset_no_avg);
            __m256i       r[2];

            prepare_half_coeffs_2tap_avx2(filter_params_x, subpel_x_q4, coeffs_256);

            if (w == 8) {
                do {
                    const __m256i res = x_convolve_2tap_8x2_avx2(src_ptr, src_stride, coeffs_256);
                    jnt_no_avg_round_store_8x2_avx2(res, offset_no_avg_256, dst, dst_stride);
                    src_ptr += 2 * src_stride;
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            } else {
                assert(w == 16);

                do {
                    x_convolve_2tap_16x2_avx2(src_ptr, src_stride, coeffs_256, r);
                    jnt_no_avg_round_store_16x2_avx2(r, offset_no_avg_256, dst, dst_stride);
                    src_ptr += 2 * src_stride;
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            }
        } else {
            const __m512i offset_no_avg_512 = _mm512_set1_epi16(offset_no_avg);

            prepare_half_coeffs_2tap_avx512(filter_params_x, subpel_x_q4, coeffs_512);

            if (w == 32) {
                do {
                    jnt_x_no_avg_2tap_32x2_avx512(src_ptr, src_stride, coeffs_512, offset_no_avg_512, dst, dst_stride);
                    src_ptr += 2 * src_stride;
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            } else if (w == 64) {
                do {
                    jnt_x_no_avg_2tap_64_avx512(src_ptr, coeffs_512, offset_no_avg_512, dst);
                    src_ptr += src_stride;
                    dst += dst_stride;
                } while (--y);
            } else {
                assert(w == 128);

                do {
                    jnt_x_no_avg_2tap_64_avx512(src_ptr, coeffs_512, offset_no_avg_512, dst);
                    jnt_x_no_avg_2tap_64_avx512(src_ptr + 64, coeffs_512, offset_no_avg_512, dst + 64);
                    src_ptr += src_stride;
                    dst += dst_stride;
                } while (--y);
            }
        }
    }
}

static void jnt_convolve_x_6tap_avx512(const uint8_t* const src, const int32_t src_stride, uint8_t* dst8,
                                       const int32_t dst8_stride, const int32_t w, const int32_t h,
                                       const InterpFilterParams* const filter_params_x, const int32_t subpel_x_q4,
                                       const ConvolveParams* const conv_params) {
    const uint8_t* src_ptr      = src - 2;
    const int32_t  dst_stride   = conv_params->dst_stride;
    const int32_t  round_0      = 3;
    const int32_t  round_1      = COMPOUND_ROUND1_BITS;
    const int32_t  bits         = FILTER_BITS - round_1;
    const int32_t  bd           = 8;
    const int32_t  round_bits   = 2 * FILTER_BITS - round_0 - round_1;
    const int32_t  offset_bits  = bd + round_bits;
    const int32_t  round_offset = (1 << offset_bits) + (1 << (offset_bits - 1));
    ConvBufType*   dst          = conv_params->dst;
    int32_t        y            = h;

    if (conv_params->do_average) {
        if (conv_params->use_jnt_comp_avg) {
            const int32_t factor          = conv_params->fwd_offset | (conv_params->bck_offset << 16);
            const int32_t offset_comp_avg = round_offset * conv_params->bck_offset +
                (1 << (round_bits + DIST_PRECISION_BITS - 1)) - (round_offset << DIST_PRECISION_BITS);

            if (w <= 16) {
                const __m256i factor_256          = _mm256_set1_epi32(factor);
                const __m256i offset_comp_avg_256 = _mm256_set1_epi32(offset_comp_avg);
                __m256i       coeffs_256[3], filt_256[3];

                filt_256[0] = _mm256_loadu_si256((__m256i const*)filt1_global_avx);
                filt_256[1] = _mm256_loadu_si256((__m256i const*)filt2_global_avx);
                filt_256[2] = _mm256_loadu_si256((__m256i const*)filt3_global_avx);
                prepare_half_coeffs_6tap_avx2(filter_params_x, subpel_x_q4, coeffs_256);

                if (w == 8) {
                    do {
                        const __m256i res = x_convolve_6tap_8x2_avx2(src_ptr, src_stride, coeffs_256, filt_256);
                        jnt_comp_avg_round_store_8x2_avx2(
                            res, factor_256, offset_comp_avg_256, dst, dst_stride, dst8, dst8_stride);
                        src_ptr += 2 * src_stride;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                } else {
                    assert(w == 16);

                    do {
                        jnt_x_comp_avg_6tap_16x2_avx2(src_ptr,
                                                      src_stride,
                                                      coeffs_256,
                                                      filt_256,
                                                      factor_256,
                                                      offset_comp_avg_256,
                                                      dst,
                                                      dst_stride,
                                                      dst8,
                                                      dst8_stride);
                        src_ptr += 2 * src_stride;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                }
            } else {
                const __m512i factor_512          = _mm512_set1_epi32(factor);
                const __m512i offset_comp_avg_512 = _mm512_set1_epi32(offset_comp_avg);
                __m512i       coeffs_512[4], filt_512[4];

                filt_512[0] = zz_load_512(filt1_global_avx);
                filt_512[1] = zz_load_512(filt2_global_avx);
                filt_512[2] = zz_load_512(filt3_global_avx);
                prepare_half_coeffs_6tap_avx512(filter_params_x, subpel_x_q4, coeffs_512);

                if (w == 32) {
                    do {
                        jnt_x_comp_avg_6tap_32x2_avx512(src_ptr,
                                                        src_stride,
                                                        coeffs_512,
                                                        filt_512,
                                                        factor_512,
                                                        offset_comp_avg_512,
                                                        dst,
                                                        dst_stride,
                                                        dst8,
                                                        dst8_stride);
                        src_ptr += 2 * src_stride;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                } else if (w == 64) {
                    do {
                        jnt_x_comp_avg_6tap_64_avx512(
                            src_ptr, coeffs_512, filt_512, factor_512, offset_comp_avg_512, dst, dst8);
                        src_ptr += src_stride;
                        dst += dst_stride;
                        dst8 += dst8_stride;
                    } while (--y);
                } else {
                    assert(w == 128);

                    do {
                        jnt_x_comp_avg_6tap_64_avx512(
                            src_ptr, coeffs_512, filt_512, factor_512, offset_comp_avg_512, dst, dst8);
                        jnt_x_comp_avg_6tap_64_avx512(
                            src_ptr + 64, coeffs_512, filt_512, factor_512, offset_comp_avg_512, dst + 64, dst8 + 64);
                        src_ptr += src_stride;
                        dst += dst_stride;
                        dst8 += dst8_stride;
                    } while (--y);
                }
            }
        } else {
            const int16_t offset_avg = (1 << (FILTER_BITS - 1)) + (1 << (round_0 - bits - 2)) -
                (round_offset << (round_0 - bits - 1));

            if (w <= 16) {
                const __m256i offset_avg_256 = _mm256_set1_epi16(offset_avg);
                __m256i       coeffs_256[3], filt_256[3];

                filt_256[0] = _mm256_loadu_si256((__m256i const*)filt1_global_avx);
                filt_256[1] = _mm256_loadu_si256((__m256i const*)filt2_global_avx);
                filt_256[2] = _mm256_loadu_si256((__m256i const*)filt3_global_avx);
                prepare_half_coeffs_6tap_avx2(filter_params_x, subpel_x_q4, coeffs_256);

                if (w == 8) {
                    do {
                        const __m256i res = x_convolve_6tap_8x2_avx2(src_ptr, src_stride, coeffs_256, filt_256);
                        jnt_avg_round_store_8x2_avx2(res, offset_avg_256, dst, dst_stride, dst8, dst8_stride);
                        src_ptr += 2 * src_stride;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                } else {
                    assert(w == 16);

                    do {
                        jnt_x_avg_6tap_16x2_avx2(src_ptr,
                                                 src_stride,
                                                 coeffs_256,
                                                 filt_256,
                                                 offset_avg_256,
                                                 dst,
                                                 dst_stride,
                                                 dst8,
                                                 dst8_stride);
                        src_ptr += 2 * src_stride;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                }
            } else {
                const __m512i offset_avg_512 = _mm512_set1_epi16(offset_avg);
                __m512i       coeffs_512[4], filt_512[4];

                filt_512[0] = zz_load_512(filt1_global_avx);
                filt_512[1] = zz_load_512(filt2_global_avx);
                filt_512[2] = zz_load_512(filt3_global_avx);
                prepare_half_coeffs_6tap_avx512(filter_params_x, subpel_x_q4, coeffs_512);

                if (w == 32) {
                    do {
                        jnt_x_avg_6tap_32x2_avx512(src_ptr,
                                                   src_stride,
                                                   coeffs_512,
                                                   filt_512,
                                                   offset_avg_512,
                                                   dst,
                                                   dst_stride,
                                                   dst8,
                                                   dst8_stride);
                        src_ptr += 2 * src_stride;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                } else if (w == 64) {
                    do {
                        jnt_x_avg_6tap_64_avx512(src_ptr, coeffs_512, filt_512, offset_avg_512, dst, dst8);
                        src_ptr += src_stride;
                        dst += dst_stride;
                        dst8 += dst8_stride;
                    } while (--y);
                } else {
                    assert(w == 128);

                    do {
                        jnt_x_avg_6tap_64_avx512(src_ptr, coeffs_512, filt_512, offset_avg_512, dst, dst8);
                        jnt_x_avg_6tap_64_avx512(
                            src_ptr + 64, coeffs_512, filt_512, offset_avg_512, dst + 64, dst8 + 64);
                        src_ptr += src_stride;
                        dst += dst_stride;
                        dst8 += dst8_stride;
                    } while (--y);
                }
            }
        }
    } else {
        const int16_t offset_no_avg = (round_offset << (round_0 - bits - 1)) + (1 << (round_0 - bits - 2));

        if (w <= 16) {
            const __m256i offset_no_avg_256 = _mm256_set1_epi16(offset_no_avg);
            __m256i       coeffs_256[3], filt_256[3];

            filt_256[0] = _mm256_loadu_si256((__m256i const*)filt1_global_avx);
            filt_256[1] = _mm256_loadu_si256((__m256i const*)filt2_global_avx);
            filt_256[2] = _mm256_loadu_si256((__m256i const*)filt3_global_avx);
            prepare_half_coeffs_6tap_avx2(filter_params_x, subpel_x_q4, coeffs_256);

            if (w == 8) {
                do {
                    const __m256i res = x_convolve_6tap_8x2_avx2(src_ptr, src_stride, coeffs_256, filt_256);
                    jnt_no_avg_round_store_8x2_avx2(res, offset_no_avg_256, dst, dst_stride);
                    src_ptr += 2 * src_stride;
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            } else {
                assert(w == 16);

                do {
                    jnt_x_no_avg_6tap_16x2_avx2(
                        src_ptr, src_stride, coeffs_256, filt_256, offset_no_avg_256, dst, dst_stride);
                    src_ptr += 2 * src_stride;
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            }
        } else {
            const __m512i offset_no_avg_512 = _mm512_set1_epi16(offset_no_avg);
            __m512i       coeffs_512[4], filt_512[4];

            filt_512[0] = zz_load_512(filt1_global_avx);
            filt_512[1] = zz_load_512(filt2_global_avx);
            filt_512[2] = zz_load_512(filt3_global_avx);
            prepare_half_coeffs_6tap_avx512(filter_params_x, subpel_x_q4, coeffs_512);

            if (w == 32) {
                do {
                    jnt_x_no_avg_6tap_32x2_avx512(
                        src_ptr, src_stride, coeffs_512, filt_512, offset_no_avg_512, dst, dst_stride);
                    src_ptr += 2 * src_stride;
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            } else if (w == 64) {
                do {
                    jnt_x_no_avg_6tap_64_avx512(src_ptr, coeffs_512, filt_512, offset_no_avg_512, dst);
                    src_ptr += src_stride;
                    dst += dst_stride;
                } while (--y);
            } else {
                assert(w == 128);

                do {
                    jnt_x_no_avg_6tap_64_avx512(src_ptr, coeffs_512, filt_512, offset_no_avg_512, dst);
                    jnt_x_no_avg_6tap_64_avx512(src_ptr + 64, coeffs_512, filt_512, offset_no_avg_512, dst + 64);
                    src_ptr += src_stride;
                    dst += dst_stride;
                } while (--y);
            }
        }
    }
}

static void jnt_convolve_x_8tap_avx512(const uint8_t* const src, const int32_t src_stride, uint8_t* dst8,
                                       const int32_t dst8_stride, const int32_t w, const int32_t h,
                                       const InterpFilterParams* const filter_params_x, const int32_t subpel_x_q4,
                                       const ConvolveParams* const conv_params) {
    const uint8_t* src_ptr      = src - 3;
    const int32_t  dst_stride   = conv_params->dst_stride;
    const int32_t  round_0      = 3;
    const int32_t  round_1      = COMPOUND_ROUND1_BITS;
    const int32_t  bits         = FILTER_BITS - round_1;
    const int32_t  bd           = 8;
    const int32_t  round_bits   = 2 * FILTER_BITS - round_0 - round_1;
    const int32_t  offset_bits  = bd + round_bits;
    const int32_t  round_offset = (1 << offset_bits) + (1 << (offset_bits - 1));
    ConvBufType*   dst          = conv_params->dst;
    int32_t        y            = h;

    if (conv_params->do_average) {
        if (conv_params->use_jnt_comp_avg) {
            const int32_t factor          = conv_params->fwd_offset | (conv_params->bck_offset << 16);
            const int32_t offset_comp_avg = round_offset * conv_params->bck_offset +
                (1 << (round_bits + DIST_PRECISION_BITS - 1)) - (round_offset << DIST_PRECISION_BITS);
            const __m256i factor_256          = _mm256_set1_epi32(factor);
            const __m256i offset_comp_avg_256 = _mm256_set1_epi32(offset_comp_avg);
            const __m512i factor_512          = _mm512_set1_epi32(factor);
            const __m512i offset_comp_avg_512 = _mm512_set1_epi32(offset_comp_avg);

            if (w <= 16) {
                __m256i coeffs_256[4], filt_256[4];

                filt_256[0] = _mm256_loadu_si256((__m256i const*)filt1_global_avx);
                filt_256[1] = _mm256_loadu_si256((__m256i const*)filt2_global_avx);
                filt_256[2] = _mm256_loadu_si256((__m256i const*)filt3_global_avx);
                filt_256[3] = _mm256_loadu_si256((__m256i const*)filt4_global_avx);
                prepare_half_coeffs_8tap_avx2(filter_params_x, subpel_x_q4, coeffs_256);

                if (w == 8) {
                    do {
                        const __m256i res = x_convolve_8tap_8x2_avx2(src_ptr, src_stride, coeffs_256, filt_256);
                        jnt_comp_avg_round_store_8x2_avx2(
                            res, factor_256, offset_comp_avg_256, dst, dst_stride, dst8, dst8_stride);
                        src_ptr += 2 * src_stride;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                } else {
                    assert(w == 16);

                    do {
                        jnt_x_comp_avg_8tap_16x2_avx2(src_ptr,
                                                      src_stride,
                                                      coeffs_256,
                                                      filt_256,
                                                      factor_256,
                                                      offset_comp_avg_256,
                                                      dst,
                                                      dst_stride,
                                                      dst8,
                                                      dst8_stride);
                        src_ptr += 2 * src_stride;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                }
            } else {
                __m512i coeffs_512[4], filt_512[4];

                filt_512[0] = zz_load_512(filt1_global_avx);
                filt_512[1] = zz_load_512(filt2_global_avx);
                filt_512[2] = zz_load_512(filt3_global_avx);
                filt_512[3] = zz_load_512(filt4_global_avx);
                prepare_half_coeffs_8tap_avx512(filter_params_x, subpel_x_q4, coeffs_512);

                if (w == 32) {
                    do {
                        jnt_x_comp_avg_8tap_32x2_avx512(src_ptr,
                                                        src_stride,
                                                        coeffs_512,
                                                        filt_512,
                                                        factor_512,
                                                        offset_comp_avg_512,
                                                        dst,
                                                        dst_stride,
                                                        dst8,
                                                        dst8_stride);
                        src_ptr += 2 * src_stride;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                } else if (w == 64) {
                    do {
                        jnt_x_comp_avg_8tap_64_avx512(
                            src_ptr, coeffs_512, filt_512, factor_512, offset_comp_avg_512, dst, dst8);
                        src_ptr += src_stride;
                        dst += dst_stride;
                        dst8 += dst8_stride;
                    } while (--y);
                } else {
                    assert(w == 128);

                    do {
                        jnt_x_comp_avg_8tap_64_avx512(
                            src_ptr, coeffs_512, filt_512, factor_512, offset_comp_avg_512, dst, dst8);
                        jnt_x_comp_avg_8tap_64_avx512(
                            src_ptr + 64, coeffs_512, filt_512, factor_512, offset_comp_avg_512, dst + 64, dst8 + 64);
                        src_ptr += src_stride;
                        dst += dst_stride;
                        dst8 += dst8_stride;
                    } while (--y);
                }
            }
        } else {
            const int16_t offset_avg = (1 << (FILTER_BITS - 1)) + (1 << (round_0 - bits - 2)) -
                (round_offset << (round_0 - bits - 1));
            const __m256i offset_avg_256 = _mm256_set1_epi16(offset_avg);
            const __m512i offset_avg_512 = _mm512_set1_epi16(offset_avg);

            if (w <= 16) {
                __m256i coeffs_256[4], filt_256[4];

                filt_256[0] = _mm256_loadu_si256((__m256i const*)filt1_global_avx);
                filt_256[1] = _mm256_loadu_si256((__m256i const*)filt2_global_avx);
                filt_256[2] = _mm256_loadu_si256((__m256i const*)filt3_global_avx);
                filt_256[3] = _mm256_loadu_si256((__m256i const*)filt4_global_avx);
                prepare_half_coeffs_8tap_avx2(filter_params_x, subpel_x_q4, coeffs_256);

                if (w == 8) {
                    do {
                        const __m256i res = x_convolve_8tap_8x2_avx2(src_ptr, src_stride, coeffs_256, filt_256);
                        jnt_avg_round_store_8x2_avx2(res, offset_avg_256, dst, dst_stride, dst8, dst8_stride);
                        src_ptr += 2 * src_stride;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                } else {
                    assert(w == 16);

                    do {
                        jnt_x_avg_8tap_16x2_avx2(src_ptr,
                                                 src_stride,
                                                 coeffs_256,
                                                 filt_256,
                                                 offset_avg_256,
                                                 dst,
                                                 dst_stride,
                                                 dst8,
                                                 dst8_stride);
                        src_ptr += 2 * src_stride;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                }
            } else {
                __m512i coeffs_512[4], filt_512[4];

                filt_512[0] = zz_load_512(filt1_global_avx);
                filt_512[1] = zz_load_512(filt2_global_avx);
                filt_512[2] = zz_load_512(filt3_global_avx);
                filt_512[3] = zz_load_512(filt4_global_avx);
                prepare_half_coeffs_8tap_avx512(filter_params_x, subpel_x_q4, coeffs_512);

                if (w == 32) {
                    do {
                        jnt_x_avg_8tap_32x2_avx512(src_ptr,
                                                   src_stride,
                                                   coeffs_512,
                                                   filt_512,
                                                   offset_avg_512,
                                                   dst,
                                                   dst_stride,
                                                   dst8,
                                                   dst8_stride);
                        src_ptr += 2 * src_stride;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                } else if (w == 64) {
                    do {
                        jnt_x_avg_8tap_64_avx512(src_ptr, coeffs_512, filt_512, offset_avg_512, dst, dst8);
                        src_ptr += src_stride;
                        dst += dst_stride;
                        dst8 += dst8_stride;
                    } while (--y);
                } else {
                    assert(w == 128);

                    do {
                        jnt_x_avg_8tap_64_avx512(src_ptr, coeffs_512, filt_512, offset_avg_512, dst, dst8);
                        jnt_x_avg_8tap_64_avx512(
                            src_ptr + 64, coeffs_512, filt_512, offset_avg_512, dst + 64, dst8 + 64);
                        src_ptr += src_stride;
                        dst += dst_stride;
                        dst8 += dst8_stride;
                    } while (--y);
                }
            }
        }
    } else {
        const int16_t offset_no_avg     = (round_offset << (round_0 - bits - 1)) + (1 << (round_0 - bits - 2));
        const __m256i offset_no_avg_256 = _mm256_set1_epi16(offset_no_avg);
        const __m512i offset_no_avg_512 = _mm512_set1_epi16(offset_no_avg);

        if (w <= 16) {
            __m256i coeffs_256[4], filt_256[4];

            filt_256[0] = _mm256_loadu_si256((__m256i const*)filt1_global_avx);
            filt_256[1] = _mm256_loadu_si256((__m256i const*)filt2_global_avx);
            filt_256[2] = _mm256_loadu_si256((__m256i const*)filt3_global_avx);
            filt_256[3] = _mm256_loadu_si256((__m256i const*)filt4_global_avx);
            prepare_half_coeffs_8tap_avx2(filter_params_x, subpel_x_q4, coeffs_256);

            if (w == 8) {
                do {
                    const __m256i res = x_convolve_8tap_8x2_avx2(src_ptr, src_stride, coeffs_256, filt_256);
                    jnt_no_avg_round_store_8x2_avx2(res, offset_no_avg_256, dst, dst_stride);
                    src_ptr += 2 * src_stride;
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            } else {
                assert(w == 16);

                do {
                    jnt_x_no_avg_8tap_16x2_avx2(
                        src_ptr, src_stride, coeffs_256, filt_256, offset_no_avg_256, dst, dst_stride);
                    src_ptr += 2 * src_stride;
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            }
        } else {
            __m512i coeffs_512[4], filt_512[4];

            filt_512[0] = zz_load_512(filt1_global_avx);
            filt_512[1] = zz_load_512(filt2_global_avx);
            filt_512[2] = zz_load_512(filt3_global_avx);
            filt_512[3] = zz_load_512(filt4_global_avx);
            prepare_half_coeffs_8tap_avx512(filter_params_x, subpel_x_q4, coeffs_512);

            if (w == 32) {
                do {
                    jnt_x_no_avg_8tap_32x2_avx512(
                        src_ptr, src_stride, coeffs_512, filt_512, offset_no_avg_512, dst, dst_stride);
                    src_ptr += 2 * src_stride;
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            } else if (w == 64) {
                do {
                    jnt_x_no_avg_8tap_64_avx512(src_ptr, coeffs_512, filt_512, offset_no_avg_512, dst);
                    src_ptr += src_stride;
                    dst += dst_stride;
                } while (--y);
            } else {
                assert(w == 128);

                do {
                    jnt_x_no_avg_8tap_64_avx512(src_ptr, coeffs_512, filt_512, offset_no_avg_512, dst);
                    jnt_x_no_avg_8tap_64_avx512(src_ptr + 64, coeffs_512, filt_512, offset_no_avg_512, dst + 64);
                    src_ptr += src_stride;
                    dst += dst_stride;
                } while (--y);
            }
        }
    }
}

typedef void (*JntConvolveXTapFunc)(const uint8_t* const src, const int32_t src_stride, uint8_t* dst8,
                                    const int32_t dst8_stride, const int32_t w, const int32_t h,
                                    const InterpFilterParams* const filter_params_x, const int32_t subpel_x_q4,
                                    const ConvolveParams* const conv_params);

void svt_av1_jnt_convolve_x_avx512(const uint8_t* src, int32_t src_stride, uint8_t* dst8, int32_t dst8_stride,
                                   int32_t w, int32_t h, const InterpFilterParams* filter_params_x,
                                   const InterpFilterParams* filter_params_y, const int32_t subpel_x_q4,
                                   const int32_t subpel_y_q4, ConvolveParams* conv_params) {
    static const JntConvolveXTapFunc jnt_convolve_x_tap_func_table[MAX_FILTER_TAP + 1] = {NULL,
                                                                                          NULL,
                                                                                          jnt_convolve_x_2tap_avx512,
                                                                                          NULL,
                                                                                          jnt_convolve_x_4tap_ssse3,
                                                                                          NULL,
                                                                                          jnt_convolve_x_6tap_avx512,
                                                                                          NULL,
                                                                                          jnt_convolve_x_8tap_avx512};
    const int32_t                    tap_x = get_convolve_tap(filter_params_x->filter_ptr);

    (void)filter_params_y;
    (void)subpel_y_q4;

    assert(conv_params->round_0 == 3);
    assert(conv_params->round_1 == COMPOUND_ROUND1_BITS);

    jnt_convolve_x_tap_func_table[tap_x](
        src, src_stride, dst8, dst8_stride, w, h, filter_params_x, subpel_x_q4, conv_params);
}
#endif // EN_AVX512_SUPPORT
