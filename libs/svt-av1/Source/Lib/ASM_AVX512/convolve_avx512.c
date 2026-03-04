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
#include "inter_prediction.h"
#include "memory_avx2.h"
#include "synonyms.h"

static INLINE __m512i sr_x_round_avx512(const __m512i src) {
    const __m512i round = _mm512_set1_epi16(34);
    const __m512i dst   = _mm512_add_epi16(src, round);
    return _mm512_srai_epi16(dst, 6);
}

static INLINE void sr_x_round_store_32x2_avx512(const __m512i res[2], uint8_t* const dst, const int32_t dst_stride) {
    __m512i r[2];

    r[0] = sr_x_round_avx512(res[0]);
    r[1] = sr_x_round_avx512(res[1]);
    convolve_store_32x2_avx512(r[0], r[1], dst, dst_stride);
}

static INLINE void sr_x_round_store_64_avx512(const __m512i res[2], uint8_t* const dst) {
    __m512i r[2];

    r[0] = sr_x_round_avx512(res[0]);
    r[1] = sr_x_round_avx512(res[1]);
    convolve_store_64_avx512(r[0], r[1], dst);
}

static INLINE void sr_y_round_store_64_avx512(const __m512i res[2], uint8_t* const dst) {
    __m512i r[2];

    r[0] = sr_y_round_avx512(res[0]);
    r[1] = sr_y_round_avx512(res[1]);
    convolve_store_64_avx512(r[0], r[1], dst);
}

static INLINE void sr_y_round_store_64x2_avx512(const __m512i res[4], uint8_t* const dst, const int32_t dst_stride) {
    sr_y_round_store_64_avx512(res, dst);
    sr_y_round_store_64_avx512(res + 2, dst + dst_stride);
}

static INLINE void sr_y_round_store_32x2_avx512(const __m512i res[2], uint8_t* const dst, const int32_t dst_stride) {
    __m512i r[2];

    r[0] = sr_y_round_avx512(res[0]);
    r[1] = sr_y_round_avx512(res[1]);
    convolve_store_32x2_avx512(r[0], r[1], dst, dst_stride);
}

static INLINE void sr_y_2tap_32x2_avx512(const uint8_t* const src, const int32_t src_stride, const __m512i coeffs[1],
                                         __m256i* const s, uint8_t* const dst, const int32_t dst_stride) {
    __m512i r[2];
    y_convolve_2tap_32x2_avx512(src, src_stride, coeffs, s, r);
    sr_y_round_store_32x2_avx512(r, dst, dst_stride);
}

static INLINE void sr_y_2tap_64_avx512(const uint8_t* const src, const __m512i coeffs[1], const __m512i s0,
                                       __m512i* const s1, uint8_t* const dst) {
    __m512i r[2];
    y_convolve_2tap_64_avx512(src, coeffs, s0, s1, r);
    sr_y_round_store_64_avx512(r, dst);
}

static INLINE void sr_y_2tap_64_avg_avx512(const uint8_t* const src, const __m512i s0, __m512i* const s1,
                                           uint8_t* const dst) {
    *s1             = _mm512_loadu_si512((__m512i*)src);
    const __m512i d = _mm512_avg_epu8(s0, *s1);
    _mm512_storeu_si512((__m512i*)dst, d);
}

void svt_av1_convolve_y_sr_avx512(const uint8_t* src, int32_t src_stride, uint8_t* dst, int32_t dst_stride, int32_t w,
                                  int32_t h, const InterpFilterParams* filter_params_x,
                                  const InterpFilterParams* filter_params_y, const int32_t subpel_x_q4,
                                  const int32_t subpel_y_q4, ConvolveParams* conv_params) {
    int32_t x, y;
    __m128i coeffs_128[4];
    __m256i coeffs_256[4];
    __m512i coeffs_512[4];

    (void)filter_params_x;
    (void)subpel_x_q4;
    (void)conv_params;

    if (is_convolve_2tap(filter_params_y->filter_ptr)) {
        // vert_filt as 2 tap
        const uint8_t* src_ptr = src;

        y = h;

        if (subpel_y_q4 != 8) {
            if (w <= 8) {
                prepare_half_coeffs_2tap_ssse3(filter_params_y, subpel_y_q4, coeffs_128);

                if (w == 2) {
                    __m128i s_16[2];

                    s_16[0] = _mm_cvtsi32_si128(*(int16_t*)src_ptr);

                    do {
                        const __m128i res = y_convolve_2tap_2x2_ssse3(src_ptr, src_stride, coeffs_128, s_16);
                        const __m128i r   = sr_y_round_sse2(res);
                        pack_store_2x2_sse2(r, dst, dst_stride);
                        src_ptr += 2 * src_stride;
                        dst += 2 * dst_stride;
                        y -= 2;
                    } while (y);
                } else if (w == 4) {
                    __m128i s_32[2];

                    s_32[0] = _mm_cvtsi32_si128(*(int32_t*)src_ptr);

                    do {
                        const __m128i res = y_convolve_2tap_4x2_ssse3(src_ptr, src_stride, coeffs_128, s_32);
                        const __m128i r   = sr_y_round_sse2(res);
                        pack_store_4x2_sse2(r, dst, dst_stride);
                        src_ptr += 2 * src_stride;
                        dst += 2 * dst_stride;
                        y -= 2;
                    } while (y);
                } else {
                    __m128i s_64[2], s_128[2];

                    assert(w == 8);

                    s_64[0] = _mm_loadl_epi64((__m128i*)src_ptr);

                    do {
                        // Note: Faster than binding to AVX2 registers.
                        s_64[1]            = _mm_loadl_epi64((__m128i*)(src_ptr + src_stride));
                        s_128[0]           = _mm_unpacklo_epi64(s_64[0], s_64[1]);
                        s_64[0]            = _mm_loadl_epi64((__m128i*)(src_ptr + 2 * src_stride));
                        s_128[1]           = _mm_unpacklo_epi64(s_64[1], s_64[0]);
                        const __m128i ss0  = _mm_unpacklo_epi8(s_128[0], s_128[1]);
                        const __m128i ss1  = _mm_unpackhi_epi8(s_128[0], s_128[1]);
                        const __m128i res0 = convolve_2tap_ssse3(&ss0, coeffs_128);
                        const __m128i res1 = convolve_2tap_ssse3(&ss1, coeffs_128);
                        const __m128i r0   = sr_y_round_sse2(res0);
                        const __m128i r1   = sr_y_round_sse2(res1);
                        const __m128i d    = _mm_packus_epi16(r0, r1);
                        _mm_storel_epi64((__m128i*)dst, d);
                        _mm_storeh_epi64((__m128i*)(dst + dst_stride), d);
                        src_ptr += 2 * src_stride;
                        dst += 2 * dst_stride;
                        y -= 2;
                    } while (y);
                }
            } else if (w == 16) {
                __m128i s_128[2];

                prepare_half_coeffs_2tap_avx2(filter_params_y, subpel_y_q4, coeffs_256);

                s_128[0] = _mm_loadu_si128((__m128i*)src_ptr);

                do {
                    __m256i r[2];

                    y_convolve_2tap_16x2_avx2(src_ptr, src_stride, coeffs_256, s_128, r);
                    sr_y_round_store_16x2_avx2(r, dst, dst_stride);
                    src_ptr += 2 * src_stride;
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            } else {
                prepare_half_coeffs_2tap_avx512(filter_params_y, subpel_y_q4, coeffs_512);

                if (w == 32) {
                    __m256i s_256 = _mm256_loadu_si256((__m256i*)src_ptr);

                    do {
                        sr_y_2tap_32x2_avx512(src_ptr + src_stride, src_stride, coeffs_512, &s_256, dst, dst_stride);
                        src_ptr += 2 * src_stride;
                        dst += 2 * dst_stride;
                        y -= 2;
                    } while (y);
                } else if (w == 64) {
                    __m512i s_512[2];

                    s_512[0] = _mm512_loadu_si512((__m512i*)src_ptr);

                    do {
                        sr_y_2tap_64_avx512(src_ptr + src_stride, coeffs_512, s_512[0], &s_512[1], dst);
                        sr_y_2tap_64_avx512(
                            src_ptr + 2 * src_stride, coeffs_512, s_512[1], &s_512[0], dst + dst_stride);
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
                        sr_y_2tap_64_avx512(
                            src_ptr + 1 * src_stride + 0 * 64, coeffs_512, s_512[0][0], &s_512[1][0], dst);
                        sr_y_2tap_64_avx512(
                            src_ptr + 1 * src_stride + 1 * 64, coeffs_512, s_512[0][1], &s_512[1][1], dst + 2 * 32);

                        sr_y_2tap_64_avx512(
                            src_ptr + 2 * src_stride + 0 * 64, coeffs_512, s_512[1][0], &s_512[0][0], dst + dst_stride);
                        sr_y_2tap_64_avx512(src_ptr + 2 * src_stride + 1 * 64,
                                            coeffs_512,
                                            s_512[1][1],
                                            &s_512[0][1],
                                            dst + dst_stride + 2 * 32);

                        src_ptr += 2 * src_stride;
                        dst += 2 * dst_stride;
                        y -= 2;
                    } while (y);
                }
            }
        } else {
            // average to get half pel
            if (w <= 8) {
                if (w == 2) {
                    __m128i s_16[2];

                    s_16[0] = _mm_cvtsi32_si128(*(int16_t*)src_ptr);

                    do {
                        s_16[1]                       = _mm_cvtsi32_si128(*(int16_t*)(src_ptr + src_stride));
                        const __m128i d0              = _mm_avg_epu8(s_16[0], s_16[1]);
                        *(int16_t*)dst                = (int16_t)_mm_cvtsi128_si32(d0);
                        s_16[0]                       = _mm_cvtsi32_si128(*(int16_t*)(src_ptr + 2 * src_stride));
                        const __m128i d1              = _mm_avg_epu8(s_16[1], s_16[0]);
                        *(int16_t*)(dst + dst_stride) = (int16_t)_mm_cvtsi128_si32(d1);
                        src_ptr += 2 * src_stride;
                        dst += 2 * dst_stride;
                        y -= 2;
                    } while (y);
                } else if (w == 4) {
                    __m128i s_32[2];

                    s_32[0] = _mm_cvtsi32_si128(*(int32_t*)src_ptr);

                    do {
                        s_32[1]          = _mm_cvtsi32_si128(*(int32_t*)(src_ptr + src_stride));
                        const __m128i d0 = _mm_avg_epu8(s_32[0], s_32[1]);
                        xx_storel_32(dst, d0);
                        s_32[0]          = _mm_cvtsi32_si128(*(int32_t*)(src_ptr + 2 * src_stride));
                        const __m128i d1 = _mm_avg_epu8(s_32[1], s_32[0]);
                        xx_storel_32(dst + dst_stride, d1);
                        src_ptr += 2 * src_stride;
                        dst += 2 * dst_stride;
                        y -= 2;
                    } while (y);
                } else {
                    __m128i s_64[2];

                    assert(w == 8);

                    s_64[0] = _mm_loadl_epi64((__m128i*)src_ptr);

                    do {
                        // Note: Faster than binding to AVX2 registers.
                        s_64[1]          = _mm_loadl_epi64((__m128i*)(src_ptr + src_stride));
                        const __m128i d0 = _mm_avg_epu8(s_64[0], s_64[1]);
                        _mm_storel_epi64((__m128i*)dst, d0);
                        s_64[0]          = _mm_loadl_epi64((__m128i*)(src_ptr + 2 * src_stride));
                        const __m128i d1 = _mm_avg_epu8(s_64[1], s_64[0]);
                        _mm_storel_epi64((__m128i*)(dst + dst_stride), d1);
                        src_ptr += 2 * src_stride;
                        dst += 2 * dst_stride;
                        y -= 2;
                    } while (y);
                }
            } else if (w == 16) {
                __m128i s_128[2];

                s_128[0] = _mm_loadu_si128((__m128i*)src_ptr);

                do {
                    s_128[1]         = _mm_loadu_si128((__m128i*)(src_ptr + src_stride));
                    const __m128i d0 = _mm_avg_epu8(s_128[0], s_128[1]);
                    _mm_storeu_si128((__m128i*)dst, d0);
                    s_128[0]         = _mm_loadu_si128((__m128i*)(src_ptr + 2 * src_stride));
                    const __m128i d1 = _mm_avg_epu8(s_128[1], s_128[0]);
                    _mm_storeu_si128((__m128i*)(dst + dst_stride), d1);
                    src_ptr += 2 * src_stride;
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            } else if (w == 32) {
                __m256i s_256[2];

                s_256[0] = _mm256_loadu_si256((__m256i*)src_ptr);

                do {
                    sr_y_2tap_32_avg_avx2(src_ptr + src_stride, s_256[0], &s_256[1], dst);
                    sr_y_2tap_32_avg_avx2(src_ptr + 2 * src_stride, s_256[1], &s_256[0], dst + dst_stride);
                    src_ptr += 2 * src_stride;
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            } else if (w == 64) {
                __m512i s_512[2];

                s_512[0] = _mm512_loadu_si512((__m512i*)src_ptr);

                do {
                    sr_y_2tap_64_avg_avx512(src_ptr + src_stride, s_512[0], &s_512[1], dst);
                    sr_y_2tap_64_avg_avx512(src_ptr + 2 * src_stride, s_512[1], &s_512[0], dst + dst_stride);
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
                    sr_y_2tap_64_avg_avx512(src_ptr + src_stride + 0 * 6, s_512[0][0], &s_512[1][0], dst + 0 * 6);
                    sr_y_2tap_64_avg_avx512(src_ptr + src_stride + 1 * 64, s_512[0][1], &s_512[1][1], dst + 1 * 64);

                    sr_y_2tap_64_avg_avx512(
                        src_ptr + 2 * src_stride + 0 * 6, s_512[1][0], &s_512[0][0], dst + dst_stride + 0 * 6);
                    sr_y_2tap_64_avg_avx512(
                        src_ptr + 2 * src_stride + 1 * 64, s_512[1][1], &s_512[0][1], dst + dst_stride + 1 * 64);

                    src_ptr += 2 * src_stride;
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            }
        }
    } else if (is_convolve_4tap(filter_params_y->filter_ptr)) {
        // vert_filt as 4 tap
        const uint8_t* src_ptr = src - src_stride;

        y = h;

        if (w <= 4) {
            prepare_half_coeffs_4tap_ssse3(filter_params_y, subpel_y_q4, coeffs_128);

            if (w == 2) {
                __m128i s_16[4], ss_128[2];

                s_16[0] = _mm_cvtsi32_si128(*(int16_t*)(src_ptr + 0 * src_stride));
                s_16[1] = _mm_cvtsi32_si128(*(int16_t*)(src_ptr + 1 * src_stride));
                s_16[2] = _mm_cvtsi32_si128(*(int16_t*)(src_ptr + 2 * src_stride));

                const __m128i src01 = _mm_unpacklo_epi16(s_16[0], s_16[1]);
                const __m128i src12 = _mm_unpacklo_epi16(s_16[1], s_16[2]);

                ss_128[0] = _mm_unpacklo_epi8(src01, src12);

                do {
                    src_ptr += 2 * src_stride;
                    const __m128i res = y_convolve_4tap_2x2_ssse3(src_ptr, src_stride, coeffs_128, s_16, ss_128);
                    const __m128i r   = sr_y_round_sse2(res);
                    pack_store_2x2_sse2(r, dst, dst_stride);

                    ss_128[0] = ss_128[1];
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            } else {
                __m128i s_32[4], ss_128[2];

                assert(w == 4);

                s_32[0] = _mm_cvtsi32_si128(*(int32_t*)(src_ptr + 0 * src_stride));
                s_32[1] = _mm_cvtsi32_si128(*(int32_t*)(src_ptr + 1 * src_stride));
                s_32[2] = _mm_cvtsi32_si128(*(int32_t*)(src_ptr + 2 * src_stride));

                const __m128i src01 = _mm_unpacklo_epi32(s_32[0], s_32[1]);
                const __m128i src12 = _mm_unpacklo_epi32(s_32[1], s_32[2]);

                ss_128[0] = _mm_unpacklo_epi8(src01, src12);

                do {
                    src_ptr += 2 * src_stride;
                    const __m128i res = y_convolve_4tap_4x2_ssse3(src_ptr, src_stride, coeffs_128, s_32, ss_128);
                    const __m128i r   = sr_y_round_sse2(res);
                    pack_store_4x2_sse2(r, dst, dst_stride);

                    ss_128[0] = ss_128[1];
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            }
        } else {
            prepare_half_coeffs_4tap_avx2(filter_params_y, subpel_y_q4, coeffs_256);

            if (w == 8) {
                __m128i s_64[4];
                __m256i ss_256[2];

                s_64[0] = _mm_loadl_epi64((__m128i*)(src_ptr + 0 * src_stride));
                s_64[1] = _mm_loadl_epi64((__m128i*)(src_ptr + 1 * src_stride));
                s_64[2] = _mm_loadl_epi64((__m128i*)(src_ptr + 2 * src_stride));

                // Load lines a and b. Line a to lower 128, line b to upper 128
                const __m256i src01 = _mm256_setr_m128i(s_64[0], s_64[1]);
                const __m256i src12 = _mm256_setr_m128i(s_64[1], s_64[2]);

                ss_256[0] = _mm256_unpacklo_epi8(src01, src12);

                do {
                    src_ptr += 2 * src_stride;
                    const __m256i res = y_convolve_4tap_8x2_avx2(src_ptr, src_stride, coeffs_256, s_64, ss_256);
                    sr_y_round_store_8x2_avx2(res, dst, dst_stride);

                    ss_256[0] = ss_256[1];
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            } else if (w == 16) {
                __m128i s_128[4];
                __m256i ss_256[4], r[2];

                assert(w == 16);

                s_128[0] = _mm_loadu_si128((__m128i*)(src_ptr + 0 * src_stride));
                s_128[1] = _mm_loadu_si128((__m128i*)(src_ptr + 1 * src_stride));
                s_128[2] = _mm_loadu_si128((__m128i*)(src_ptr + 2 * src_stride));

                // Load lines a and b. Line a to lower 128, line b to upper 128
                const __m256i src01 = _mm256_setr_m128i(s_128[0], s_128[1]);
                const __m256i src12 = _mm256_setr_m128i(s_128[1], s_128[2]);

                ss_256[0] = _mm256_unpacklo_epi8(src01, src12);
                ss_256[2] = _mm256_unpackhi_epi8(src01, src12);

                do {
                    src_ptr += 2 * src_stride;
                    y_convolve_4tap_16x2_avx2(src_ptr, src_stride, coeffs_256, s_128, ss_256, r);
                    sr_y_round_store_16x2_avx2(r, dst, dst_stride);

                    ss_256[0] = ss_256[1];
                    ss_256[2] = ss_256[3];
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            } else {
                // AV1 standard won't have 32x4 case.
                // This only favors some optimization feature which
                // subsamples 32x8 to 32x4 and triggers 4-tap filter.

                __m256i s_256[3];
                __m512i s_512[2], ss_512[4], r[2];

                assert(w == 32);

                prepare_half_coeffs_4tap_avx512(filter_params_y, subpel_y_q4, coeffs_512);

                s_256[0] = _mm256_loadu_si256((__m256i*)(src_ptr + 0 * src_stride));
                s_256[1] = _mm256_loadu_si256((__m256i*)(src_ptr + 1 * src_stride));
                s_256[2] = _mm256_loadu_si256((__m256i*)(src_ptr + 2 * src_stride));

                s_512[0] = _mm512_setr_m256i(s_256[0], s_256[1]);
                s_512[1] = _mm512_setr_m256i(s_256[1], s_256[2]);

                ss_512[0] = _mm512_unpacklo_epi8(s_512[0], s_512[1]);
                ss_512[2] = _mm512_unpackhi_epi8(s_512[0], s_512[1]);

                do {
                    src_ptr += 2 * src_stride;
                    y_convolve_4tap_32x2_avx512(src_ptr, src_stride, coeffs_512, s_256, ss_512, r);
                    sr_y_round_store_32x2_avx512(r, dst, dst_stride);

                    ss_512[0] = ss_512[1];
                    ss_512[2] = ss_512[3];
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            }
        }
    } else if (is_convolve_6tap(filter_params_y->filter_ptr)) {
        // vert_filt as 6 tap
        const uint8_t* src_ptr = src - 2 * src_stride;

        if (w <= 4) {
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
                    const __m128i r   = sr_y_round_sse2(res);
                    pack_store_2x2_sse2(r, dst, dst_stride);

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
                    const __m128i r   = sr_y_round_sse2(res);
                    pack_store_4x2_sse2(r, dst, dst_stride);

                    ss_128[0] = ss_128[1];
                    ss_128[1] = ss_128[2];
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            }
        } else if (w <= 16) {
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
                    sr_y_round_store_8x2_avx2(res, dst, dst_stride);

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
                    sr_y_round_store_16x2_avx2(r, dst, dst_stride);

                    ss_256[0] = ss_256[1];
                    ss_256[1] = ss_256[2];

                    ss_256[3] = ss_256[4];
                    ss_256[4] = ss_256[5];
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            }
        } else {
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
                    sr_y_round_store_32x2_avx512(r, dst, dst_stride);

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
                    uint8_t*       d = dst + x;

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
                        sr_y_round_store_64x2_avx512(r, d, dst_stride);

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
    } else {
        // vert_filt as 8 tap
        const uint8_t* src_ptr = src - 3 * src_stride;

        if (w <= 4) {
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
                    const __m128i r   = sr_y_round_sse2(res);
                    pack_store_2x2_sse2(r, dst, dst_stride);
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
                    const __m128i r   = sr_y_round_sse2(res);
                    pack_store_4x2_sse2(r, dst, dst_stride);
                    ss_128[0] = ss_128[1];
                    ss_128[1] = ss_128[2];
                    ss_128[2] = ss_128[3];
                    src_ptr += 2 * src_stride;
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            }
        } else if (w <= 16) {
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

                // Load lines a and b. Line a to lower 128, line b to upper 128
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
                    sr_y_round_store_8x2_avx2(res, dst, dst_stride);
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

                // Load lines a and b. Line a to lower 128, line b to upper 128
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
                    sr_y_round_store_16x2_avx2(r, dst, dst_stride);

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
                    sr_y_round_store_32x2_avx512(r, dst, dst_stride);

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
                    uint8_t*       d = dst + x;

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
                        sr_y_round_store_64x2_avx512(r, d, dst_stride);

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

static INLINE void sr_x_2tap_32x2_avx512(const uint8_t* const src, const int32_t src_stride, const __m512i coeffs[1],
                                         uint8_t* const dst, const int32_t dst_stride) {
    __m512i r[2];

    x_convolve_2tap_32x2_avx512(src, src_stride, coeffs, r);
    sr_x_round_store_32x2_avx512(r, dst, dst_stride);
}

static INLINE void sr_x_2tap_64_avx512(const uint8_t* const src, const __m512i coeffs[1], uint8_t* const dst) {
    __m512i r[2];

    x_convolve_2tap_64_avx512(src, coeffs, r);
    sr_x_round_store_64_avx512(r, dst);
}

static INLINE void sr_x_2tap_64_avg_avx512(const uint8_t* const src, uint8_t* const dst) {
    const __m512i s0 = _mm512_loadu_si512((__m512i*)src);
    const __m512i s1 = _mm512_loadu_si512((__m512i*)(src + 1));
    const __m512i d  = _mm512_avg_epu8(s0, s1);
    _mm512_storeu_si512((__m512i*)dst, d);
}

static INLINE void sr_x_6tap_32x2_avx512(const uint8_t* const src, const int32_t src_stride, const __m512i coeffs[3],
                                         const __m512i filt[3], uint8_t* const dst, const int32_t dst_stride) {
    __m512i r[2];

    x_convolve_6tap_32x2_avx512(src, src_stride, coeffs, filt, r);
    sr_x_round_store_32x2_avx512(r, dst, dst_stride);
}

static INLINE void sr_x_6tap_64_avx512(const uint8_t* const src, const __m512i coeffs[3], const __m512i filt[3],
                                       uint8_t* const dst) {
    __m512i r[2];

    x_convolve_6tap_64_avx512(src, coeffs, filt, r);
    sr_x_round_store_64_avx512(r, dst);
}

SIMD_INLINE void sr_x_8tap_32x2_avx512(const uint8_t* const src, const int32_t src_stride, const __m512i coeffs[4],
                                       const __m512i filt[4], uint8_t* const dst, const int32_t dst_stride) {
    __m512i r[2];

    x_convolve_8tap_32x2_avx512(src, src_stride, coeffs, filt, r);
    sr_x_round_store_32x2_avx512(r, dst, dst_stride);
}

SIMD_INLINE void sr_x_8tap_64_avx512(const uint8_t* const src, const __m512i coeffs[4], const __m512i filt[4],
                                     uint8_t* const dst) {
    __m512i r[2];

    x_convolve_8tap_64_avx512(src, coeffs, filt, r);
    sr_x_round_store_64_avx512(r, dst);
}

void svt_av1_convolve_x_sr_avx512(const uint8_t* src, int32_t src_stride, uint8_t* dst, int32_t dst_stride, int32_t w,
                                  int32_t h, const InterpFilterParams* filter_params_x,
                                  const InterpFilterParams* filter_params_y, const int32_t subpel_x_q4,
                                  const int32_t subpel_y_q4, ConvolveParams* conv_params) {
    int32_t y = h;
    __m128i coeffs_128[4];
    __m256i coeffs_256[4];
    __m512i coeffs_512[4];

    (void)filter_params_y;
    (void)subpel_y_q4;
    (void)conv_params;

    assert(conv_params->round_0 == 3);
    assert((FILTER_BITS - conv_params->round_1) >= 0 ||
           ((conv_params->round_0 + conv_params->round_1) == 2 * FILTER_BITS));

    if (is_convolve_2tap(filter_params_x->filter_ptr)) {
        // horz_filt as 2 tap
        const uint8_t* src_ptr = src;

        if (subpel_x_q4 != 8) {
            if (w <= 8) {
                prepare_half_coeffs_2tap_ssse3(filter_params_x, subpel_x_q4, coeffs_128);

                if (w == 2) {
                    do {
                        const __m128i res = x_convolve_2tap_2x2_sse4_1(src_ptr, src_stride, coeffs_128);
                        const __m128i r   = sr_x_round_sse2(res);
                        pack_store_2x2_sse2(r, dst, dst_stride);
                        src_ptr += 2 * src_stride;
                        dst += 2 * dst_stride;
                        y -= 2;
                    } while (y);
                } else if (w == 4) {
                    do {
                        const __m128i res = x_convolve_2tap_4x2_ssse3(src_ptr, src_stride, coeffs_128);
                        const __m128i r   = sr_x_round_sse2(res);
                        pack_store_4x2_sse2(r, dst, dst_stride);
                        src_ptr += 2 * src_stride;
                        dst += 2 * dst_stride;
                        y -= 2;
                    } while (y);
                } else {
                    assert(w == 8);

                    do {
                        __m128i res[2];

                        x_convolve_2tap_8x2_ssse3(src_ptr, src_stride, coeffs_128, res);
                        res[0]          = sr_x_round_sse2(res[0]);
                        res[1]          = sr_x_round_sse2(res[1]);
                        const __m128i d = _mm_packus_epi16(res[0], res[1]);
                        _mm_storel_epi64((__m128i*)dst, d);
                        _mm_storeh_epi64((__m128i*)(dst + dst_stride), d);

                        src_ptr += 2 * src_stride;
                        dst += 2 * dst_stride;
                        y -= 2;
                    } while (y);
                }
            } else if (w == 16) {
                prepare_half_coeffs_2tap_avx2(filter_params_x, subpel_x_q4, coeffs_256);

                do {
                    __m256i r[2];

                    x_convolve_2tap_16x2_avx2(src_ptr, src_stride, coeffs_256, r);
                    sr_x_round_store_16x2_avx2(r, dst, dst_stride);
                    src_ptr += 2 * src_stride;
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            } else {
                prepare_half_coeffs_2tap_avx512(filter_params_x, subpel_x_q4, coeffs_512);

                if (w == 32) {
                    do {
                        // Slower than avx2.
                        sr_x_2tap_32x2_avx512(src_ptr, src_stride, coeffs_512, dst, dst_stride);
                        src_ptr += 2 * src_stride;
                        dst += 2 * dst_stride;
                        y -= 2;
                    } while (y);
                } else if (w == 64) {
                    do {
                        sr_x_2tap_64_avx512(src_ptr, coeffs_512, dst);
                        src_ptr += src_stride;
                        dst += dst_stride;
                    } while (--y);
                } else {
                    assert(w == 128);

                    do {
                        sr_x_2tap_64_avx512(src_ptr + 0 * 64, coeffs_512, dst + 0 * 64);
                        sr_x_2tap_64_avx512(src_ptr + 1 * 64, coeffs_512, dst + 1 * 64);
                        src_ptr += src_stride;
                        dst += dst_stride;
                    } while (--y);
                }
            }
        } else {
            // average to get half pel
            if (w == 2) {
                do {
                    __m128i s_128;

                    s_128                          = load_u8_4x2_sse4_1(src_ptr, src_stride);
                    const __m128i s1               = _mm_srli_si128(s_128, 1);
                    const __m128i d                = _mm_avg_epu8(s_128, s1);
                    *(uint16_t*)dst                = (uint16_t)_mm_cvtsi128_si32(d);
                    *(uint16_t*)(dst + dst_stride) = _mm_extract_epi16(d, 2);

                    src_ptr += 2 * src_stride;
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            } else if (w == 4) {
                do {
                    __m128i s_128;

                    s_128            = load_u8_8x2_sse2(src_ptr, src_stride);
                    const __m128i s1 = _mm_srli_si128(s_128, 1);
                    const __m128i d  = _mm_avg_epu8(s_128, s1);
                    xx_storel_32(dst, d);
                    *(int32_t*)(dst + dst_stride) = _mm_extract_epi32(d, 2);

                    src_ptr += 2 * src_stride;
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            } else if (w == 8) {
                do {
                    const __m128i s00 = _mm_loadu_si128((__m128i*)src_ptr);
                    const __m128i s10 = _mm_loadu_si128((__m128i*)(src_ptr + src_stride));
                    const __m128i s01 = _mm_srli_si128(s00, 1);
                    const __m128i s11 = _mm_srli_si128(s10, 1);
                    const __m128i d0  = _mm_avg_epu8(s00, s01);
                    const __m128i d1  = _mm_avg_epu8(s10, s11);
                    _mm_storel_epi64((__m128i*)dst, d0);
                    _mm_storel_epi64((__m128i*)(dst + dst_stride), d1);

                    src_ptr += 2 * src_stride;
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            } else if (w == 16) {
                do {
                    const __m128i s00 = _mm_loadu_si128((__m128i*)src_ptr);
                    const __m128i s01 = _mm_loadu_si128((__m128i*)(src_ptr + 1));
                    const __m128i s10 = _mm_loadu_si128((__m128i*)(src_ptr + src_stride));
                    const __m128i s11 = _mm_loadu_si128((__m128i*)(src_ptr + src_stride + 1));
                    const __m128i d0  = _mm_avg_epu8(s00, s01);
                    const __m128i d1  = _mm_avg_epu8(s10, s11);
                    _mm_storeu_si128((__m128i*)dst, d0);
                    _mm_storeu_si128((__m128i*)(dst + dst_stride), d1);

                    src_ptr += 2 * src_stride;
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            } else if (w == 32) {
                do {
                    sr_x_2tap_32_avg_avx2(src_ptr, dst);
                    src_ptr += src_stride;
                    dst += dst_stride;
                } while (--y);
            } else if (w == 64) {
                do {
                    sr_x_2tap_64_avg_avx512(src_ptr, dst);
                    src_ptr += src_stride;
                    dst += dst_stride;
                } while (--y);
            } else {
                assert(w == 128);

                do {
                    sr_x_2tap_64_avg_avx512(src_ptr + 0 * 64, dst + 0 * 64);
                    sr_x_2tap_64_avg_avx512(src_ptr + 1 * 64, dst + 1 * 64);
                    src_ptr += src_stride;
                    dst += dst_stride;
                } while (--y);
            }
        }
    } else if (is_convolve_4tap(filter_params_x->filter_ptr)) {
        // horz_filt as 4 tap
        const uint8_t* src_ptr = src - 1;

        prepare_half_coeffs_4tap_ssse3(filter_params_x, subpel_x_q4, coeffs_128);

        if (w == 2) {
            do {
                const __m128i res = x_convolve_4tap_2x2_ssse3(src_ptr, src_stride, coeffs_128);
                const __m128i r   = sr_x_round_sse2(res);
                pack_store_2x2_sse2(r, dst, dst_stride);
                src_ptr += 2 * src_stride;
                dst += 2 * dst_stride;
                y -= 2;
            } while (y);
        } else {
            assert(w == 4);

            do {
                const __m128i res = x_convolve_4tap_4x2_ssse3(src_ptr, src_stride, coeffs_128);
                const __m128i r   = sr_x_round_sse2(res);
                pack_store_4x2_sse2(r, dst, dst_stride);
                src_ptr += 2 * src_stride;
                dst += 2 * dst_stride;
                y -= 2;
            } while (y);
        }
    } else {
        if (is_convolve_6tap(filter_params_x->filter_ptr)) {
            // horz_filt as 6 tap
            const uint8_t* src_ptr = src - 2;

            if (w <= 16) {
                __m256i filt_256[3];

                prepare_half_coeffs_6tap_avx2(filter_params_x, subpel_x_q4, coeffs_256);

                filt_256[0] = _mm256_loadu_si256((__m256i const*)filt1_global_avx);
                filt_256[1] = _mm256_loadu_si256((__m256i const*)filt2_global_avx);
                filt_256[2] = _mm256_loadu_si256((__m256i const*)filt3_global_avx);

                if (w == 8) {
                    do {
                        const __m256i res = x_convolve_6tap_8x2_avx2(src_ptr, src_stride, coeffs_256, filt_256);
                        sr_x_round_store_8x2_avx2(res, dst, dst_stride);
                        src_ptr += 2 * src_stride;
                        dst += 2 * dst_stride;
                        y -= 2;
                    } while (y);
                } else {
                    assert(w == 16);

                    do {
                        __m256i r[2];

                        x_convolve_6tap_16x2_avx2(src_ptr, src_stride, coeffs_256, filt_256, r);
                        sr_x_round_store_16x2_avx2(r, dst, dst_stride);
                        src_ptr += 2 * src_stride;
                        dst += 2 * dst_stride;
                        y -= 2;
                    } while (y);
                }
            } else {
                __m512i filt_512[3];

                prepare_half_coeffs_6tap_avx512(filter_params_x, subpel_x_q4, coeffs_512);

                filt_512[0] = zz_load_512(filt1_global_avx);
                filt_512[1] = zz_load_512(filt2_global_avx);
                filt_512[2] = zz_load_512(filt3_global_avx);

                if (w == 32) {
                    do {
                        sr_x_6tap_32x2_avx512(src_ptr, src_stride, coeffs_512, filt_512, dst, dst_stride);
                        src_ptr += 2 * src_stride;
                        dst += 2 * dst_stride;
                        y -= 2;
                    } while (y);
                } else if (w == 64) {
                    do {
                        sr_x_6tap_64_avx512(src_ptr, coeffs_512, filt_512, dst);
                        src_ptr += src_stride;
                        dst += dst_stride;
                    } while (--y);
                } else {
                    assert(w == 128);

                    do {
                        sr_x_6tap_64_avx512(src_ptr, coeffs_512, filt_512, dst);
                        sr_x_6tap_64_avx512(src_ptr + 64, coeffs_512, filt_512, dst + 64);
                        src_ptr += src_stride;
                        dst += dst_stride;
                    } while (--y);
                }
            }
        } else {
            // horz_filt as 8 tap
            const uint8_t* src_ptr = src - 3;

            if (w <= 16) {
                __m256i filt_256[4];

                prepare_half_coeffs_8tap_avx2(filter_params_x, subpel_x_q4, coeffs_256);

                filt_256[3] = _mm256_loadu_si256((__m256i const*)filt4_global_avx);
                filt_256[0] = _mm256_loadu_si256((__m256i const*)filt1_global_avx);
                filt_256[1] = _mm256_loadu_si256((__m256i const*)filt2_global_avx);
                filt_256[2] = _mm256_loadu_si256((__m256i const*)filt3_global_avx);

                if (w == 8) {
                    do {
                        const __m256i res = x_convolve_8tap_8x2_avx2(src_ptr, src_stride, coeffs_256, filt_256);
                        sr_x_round_store_8x2_avx2(res, dst, dst_stride);
                        src_ptr += 2 * src_stride;
                        dst += 2 * dst_stride;
                        y -= 2;
                    } while (y);
                } else {
                    assert(w == 16);

                    do {
                        __m256i r[2];

                        x_convolve_8tap_16x2_avx2(src_ptr, src_stride, coeffs_256, filt_256, r);
                        sr_x_round_store_16x2_avx2(r, dst, dst_stride);
                        src_ptr += 2 * src_stride;
                        dst += 2 * dst_stride;
                        y -= 2;
                    } while (y);
                }
            } else {
                __m512i filt_512[4];

                prepare_half_coeffs_8tap_avx512(filter_params_x, subpel_x_q4, coeffs_512);

                filt_512[0] = zz_load_512(filt1_global_avx);
                filt_512[1] = zz_load_512(filt2_global_avx);
                filt_512[2] = zz_load_512(filt3_global_avx);
                filt_512[3] = zz_load_512(filt4_global_avx);

                if (w == 32) {
                    do {
                        sr_x_8tap_32x2_avx512(src_ptr, src_stride, coeffs_512, filt_512, dst, dst_stride);
                        src_ptr += 2 * src_stride;
                        dst += 2 * dst_stride;
                        y -= 2;
                    } while (y);
                } else if (w == 64) {
                    do {
                        sr_x_8tap_64_avx512(src_ptr, coeffs_512, filt_512, dst);
                        src_ptr += src_stride;
                        dst += dst_stride;
                    } while (--y);
                } else {
                    assert(w == 128);

                    do {
                        sr_x_8tap_64_avx512(src_ptr, coeffs_512, filt_512, dst);
                        sr_x_8tap_64_avx512(src_ptr + 64, coeffs_512, filt_512, dst + 64);
                        src_ptr += src_stride;
                        dst += dst_stride;
                    } while (--y);
                }
            }
        }
    }
}

#endif // EN_AVX512_SUPPORT
