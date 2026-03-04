/*
 * Copyright (c) 2017, Alliance for Open Media. All rights reserved
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
 * Media Patent License 1.0 was not distributed with this source code in the
 * PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
 */
#include <immintrin.h>

#include "definitions.h"
#include "common_dsp_rtcd.h"

#include "convolve.h"
#include "convolve_avx2.h"
#include "synonyms.h"
#include "synonyms_avx2.h"

// -----------------------------------------------------------------------------
// Copy and average

void svt_av1_highbd_convolve_y_sr_avx2(const uint16_t* src, int32_t src_stride, uint16_t* dst, int32_t dst_stride,
                                       int32_t w, int32_t h, const InterpFilterParams* filter_params_x,
                                       const InterpFilterParams* filter_params_y, const int32_t subpel_x_q4,
                                       const int32_t subpel_y_q4, ConvolveParams* conv_params, int32_t bd) {
    int32_t               i, j;
    const int32_t         fo_vert = filter_params_y->taps / 2 - 1;
    const uint16_t* const src_ptr = src - fo_vert * src_stride;
    (void)filter_params_x;
    (void)subpel_x_q4;
    (void)conv_params;

    assert(conv_params->round_0 <= FILTER_BITS);
    assert(((conv_params->round_0 + conv_params->round_1) <= (FILTER_BITS + 1)) ||
           ((conv_params->round_0 + conv_params->round_1) == (2 * FILTER_BITS)));

    __m256i s[8], coeffs_y[4];

    const int32_t bits = FILTER_BITS;

    const __m128i round_shift_bits = _mm_cvtsi32_si128(bits);
    const __m256i round_const_bits = _mm256_set1_epi32((1 << bits) >> 1);
    const __m256i clp_pxl          = _mm256_set1_epi16(bd == 10 ? 1023 : (bd == 12 ? 4095 : 255));
    const __m256i zero             = _mm256_setzero_si256();

    prepare_coeffs_8tap_avx2(filter_params_y, subpel_y_q4, coeffs_y);

    for (j = 0; j < w; j += 8) {
        const uint16_t* data = &src_ptr[j];
        /* Vertical filter */
        {
            __m256i src6;
            __m256i s01 = _mm256_permute2x128_si256(
                _mm256_castsi128_si256(_mm_loadu_si128((__m128i*)(data + 0 * src_stride))),
                _mm256_castsi128_si256(_mm_loadu_si128((__m128i*)(data + 1 * src_stride))),
                0x20);
            __m256i s12 = _mm256_permute2x128_si256(
                _mm256_castsi128_si256(_mm_loadu_si128((__m128i*)(data + 1 * src_stride))),
                _mm256_castsi128_si256(_mm_loadu_si128((__m128i*)(data + 2 * src_stride))),
                0x20);
            __m256i s23 = _mm256_permute2x128_si256(
                _mm256_castsi128_si256(_mm_loadu_si128((__m128i*)(data + 2 * src_stride))),
                _mm256_castsi128_si256(_mm_loadu_si128((__m128i*)(data + 3 * src_stride))),
                0x20);
            __m256i s34 = _mm256_permute2x128_si256(
                _mm256_castsi128_si256(_mm_loadu_si128((__m128i*)(data + 3 * src_stride))),
                _mm256_castsi128_si256(_mm_loadu_si128((__m128i*)(data + 4 * src_stride))),
                0x20);
            __m256i s45 = _mm256_permute2x128_si256(
                _mm256_castsi128_si256(_mm_loadu_si128((__m128i*)(data + 4 * src_stride))),
                _mm256_castsi128_si256(_mm_loadu_si128((__m128i*)(data + 5 * src_stride))),
                0x20);
            src6        = _mm256_castsi128_si256(_mm_loadu_si128((__m128i*)(data + 6 * src_stride)));
            __m256i s56 = _mm256_permute2x128_si256(
                _mm256_castsi128_si256(_mm_loadu_si128((__m128i*)(data + 5 * src_stride))), src6, 0x20);

            s[0] = _mm256_unpacklo_epi16(s01, s12);
            s[1] = _mm256_unpacklo_epi16(s23, s34);
            s[2] = _mm256_unpacklo_epi16(s45, s56);

            s[4] = _mm256_unpackhi_epi16(s01, s12);
            s[5] = _mm256_unpackhi_epi16(s23, s34);
            s[6] = _mm256_unpackhi_epi16(s45, s56);

            for (i = 0; i < h; i += 2) {
                data = &src_ptr[i * src_stride + j];

                const __m256i s67 = _mm256_permute2x128_si256(
                    src6, _mm256_castsi128_si256(_mm_loadu_si128((__m128i*)(data + 7 * src_stride))), 0x20);

                src6 = _mm256_castsi128_si256(_mm_loadu_si128((__m128i*)(data + 8 * src_stride)));

                const __m256i s78 = _mm256_permute2x128_si256(
                    _mm256_castsi128_si256(_mm_loadu_si128((__m128i*)(data + 7 * src_stride))), src6, 0x20);

                s[3] = _mm256_unpacklo_epi16(s67, s78);
                s[7] = _mm256_unpackhi_epi16(s67, s78);

                const __m256i res_a = convolve16_8tap_avx2(s, coeffs_y);

                __m256i res_a_round = _mm256_sra_epi32(_mm256_add_epi32(res_a, round_const_bits), round_shift_bits);

                if (w - j > 4) {
                    const __m256i res_b = convolve16_8tap_avx2(s + 4, coeffs_y);
                    __m256i res_b_round = _mm256_sra_epi32(_mm256_add_epi32(res_b, round_const_bits), round_shift_bits);

                    __m256i res_16bit = _mm256_packs_epi32(res_a_round, res_b_round);
                    res_16bit         = _mm256_min_epi16(res_16bit, clp_pxl);
                    res_16bit         = _mm256_max_epi16(res_16bit, zero);

                    _mm_storeu_si128((__m128i*)&dst[i * dst_stride + j], _mm256_castsi256_si128(res_16bit));
                    _mm_storeu_si128((__m128i*)&dst[i * dst_stride + j + dst_stride],
                                     _mm256_extracti128_si256(res_16bit, 1));
                } else if (w == 4) {
                    res_a_round = _mm256_packs_epi32(res_a_round, res_a_round);
                    res_a_round = _mm256_min_epi16(res_a_round, clp_pxl);
                    res_a_round = _mm256_max_epi16(res_a_round, zero);

                    _mm_storel_epi64((__m128i*)&dst[i * dst_stride + j], _mm256_castsi256_si128(res_a_round));
                    _mm_storel_epi64((__m128i*)&dst[i * dst_stride + j + dst_stride],
                                     _mm256_extracti128_si256(res_a_round, 1));
                } else {
                    res_a_round = _mm256_packs_epi32(res_a_round, res_a_round);
                    res_a_round = _mm256_min_epi16(res_a_round, clp_pxl);
                    res_a_round = _mm256_max_epi16(res_a_round, zero);

                    xx_storel_32((__m128i*)&dst[i * dst_stride + j], _mm256_castsi256_si128(res_a_round));
                    xx_storel_32((__m128i*)&dst[i * dst_stride + j + dst_stride],
                                 _mm256_extracti128_si256(res_a_round, 1));
                }

                s[0] = s[1];
                s[1] = s[2];
                s[2] = s[3];

                s[4] = s[5];
                s[5] = s[6];
                s[6] = s[7];
            }
        }
    }
}

void svt_av1_highbd_convolve_x_sr_avx2(const uint16_t* src, int32_t src_stride, uint16_t* dst, int32_t dst_stride,
                                       int32_t w, int32_t h, const InterpFilterParams* filter_params_x,
                                       const InterpFilterParams* filter_params_y, const int32_t subpel_x_q4,
                                       const int32_t subpel_y_q4, ConvolveParams* conv_params, int32_t bd) {
    int32_t               i, j;
    const int32_t         fo_horiz = filter_params_x->taps / 2 - 1;
    const uint16_t* const src_ptr  = src - fo_horiz;
    (void)subpel_y_q4;
    (void)filter_params_y;

    // Check that, even with 12-bit input, the intermediate values will fit
    // into an unsigned 16-bit intermediate array.
    assert(bd + FILTER_BITS + 2 - conv_params->round_0 <= 16);

    __m256i s[4], coeffs_x[4];

    const __m256i round_const_x = _mm256_set1_epi32(((1 << conv_params->round_0) >> 1));
    const __m128i round_shift_x = _mm_cvtsi32_si128(conv_params->round_0);

    const int32_t bits             = FILTER_BITS - conv_params->round_0;
    const __m128i round_shift_bits = _mm_cvtsi32_si128(bits);
    const __m256i round_const_bits = _mm256_set1_epi32((1 << bits) >> 1);
    const __m256i clp_pxl          = _mm256_set1_epi16(bd == 10 ? 1023 : (bd == 12 ? 4095 : 255));
    const __m256i zero             = _mm256_setzero_si256();

    assert(bits >= 0);
    assert((FILTER_BITS - conv_params->round_1) >= 0 ||
           ((conv_params->round_0 + conv_params->round_1) == 2 * FILTER_BITS));

    prepare_coeffs_8tap_avx2(filter_params_x, subpel_x_q4, coeffs_x);

    for (j = 0; j < w; j += 8) {
        /* Horizontal filter */
        for (i = 0; i < h; i += 2) {
            const __m256i row0 = _mm256_loadu_si256((__m256i*)&src_ptr[i * src_stride + j]);
            __m256i       row1 = _mm256_loadu_si256((__m256i*)&src_ptr[(i + 1) * src_stride + j]);

            const __m256i r0 = yy_unpacklo_epi128(row0, row1);
            const __m256i r1 = yy_unpackhi_epi128(row0, row1);

            // even pixels
            s[0] = _mm256_alignr_epi8(r1, r0, 0);
            s[1] = _mm256_alignr_epi8(r1, r0, 4);
            s[2] = _mm256_alignr_epi8(r1, r0, 8);
            s[3] = _mm256_alignr_epi8(r1, r0, 12);

            __m256i res_even = convolve16_8tap_avx2(s, coeffs_x);
            res_even         = _mm256_sra_epi32(_mm256_add_epi32(res_even, round_const_x), round_shift_x);

            // odd pixels
            s[0] = _mm256_alignr_epi8(r1, r0, 2);
            s[1] = _mm256_alignr_epi8(r1, r0, 6);
            s[2] = _mm256_alignr_epi8(r1, r0, 10);
            s[3] = _mm256_alignr_epi8(r1, r0, 14);

            __m256i res_odd = convolve16_8tap_avx2(s, coeffs_x);
            res_odd         = _mm256_sra_epi32(_mm256_add_epi32(res_odd, round_const_x), round_shift_x);

            res_even = _mm256_sra_epi32(_mm256_add_epi32(res_even, round_const_bits), round_shift_bits);
            res_odd  = _mm256_sra_epi32(_mm256_add_epi32(res_odd, round_const_bits), round_shift_bits);

            __m256i res_even1 = _mm256_packs_epi32(res_even, res_even);
            __m256i res_odd1  = _mm256_packs_epi32(res_odd, res_odd);

            __m256i res = _mm256_unpacklo_epi16(res_even1, res_odd1);
            res         = _mm256_min_epi16(res, clp_pxl);
            res         = _mm256_max_epi16(res, zero);

            if (w - j > 4) {
                _mm_storeu_si128((__m128i*)&dst[i * dst_stride + j], _mm256_castsi256_si128(res));
                _mm_storeu_si128((__m128i*)&dst[i * dst_stride + j + dst_stride], _mm256_extracti128_si256(res, 1));
            } else if (w == 4) {
                _mm_storel_epi64((__m128i*)&dst[i * dst_stride + j], _mm256_castsi256_si128(res));
                _mm_storel_epi64((__m128i*)&dst[i * dst_stride + j + dst_stride], _mm256_extracti128_si256(res, 1));
            } else {
                xx_storel_32((__m128i*)&dst[i * dst_stride + j], _mm256_castsi256_si128(res));
                xx_storel_32((__m128i*)&dst[i * dst_stride + j + dst_stride], _mm256_extracti128_si256(res, 1));
            }
        }
    }
}

// -----------------------------------------------------------------------------
// Horizontal Filtering

//HIGH_FUN_CONV_1D(horiz, x_step_q4, filter_x, h, src, , avx2);
//HIGH_FUN_CONV_1D(vert, y_step_q4, filter_y, v, src - src_stride * 3, , avx2);

#undef HIGHBD_FUNC
