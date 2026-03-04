/*
 * Copyright (c) 2018, Alliance for Open Media. All rights reserved
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at www.aomedia.org/license/software. If the Alliance for Open
 * Media Patent License 1.0 was not distributed with this source code in the
 * PATENTS file, you can obtain it at www.aomedia.org/license/patent.
 */

#include <tmmintrin.h>
#include <assert.h>

#include "definitions.h"
#include "common_dsp_rtcd.h"
#include "convolve.h"

static INLINE void svt_prepare_coeffs_12tap(const InterpFilterParams* filter_params, int subpel_q4,
                                            __m128i* coeffs /* [6] */) {
    const int16_t* const y_filter = av1_get_interp_filter_subpel_kernel(*filter_params, subpel_q4 & SUBPEL_MASK);

    __m128i coeffs_y = _mm_loadu_si128((__m128i*)y_filter);

    coeffs[0] = _mm_shuffle_epi32(coeffs_y, 0); // coeffs 0 1 0 1 0 1 0 1
    coeffs[1] = _mm_shuffle_epi32(coeffs_y, 85); // coeffs 2 3 2 3 2 3 2 3
    coeffs[2] = _mm_shuffle_epi32(coeffs_y, 170); // coeffs 4 5 4 5 4 5 4 5
    coeffs[3] = _mm_shuffle_epi32(coeffs_y, 255); // coeffs 6 7 6 7 6 7 6 7

    coeffs_y = _mm_loadl_epi64((__m128i*)(y_filter + 8));

    coeffs[4] = _mm_shuffle_epi32(coeffs_y, 0); // coeffs 8 9 8 9 8 9 8 9
    coeffs[5] = _mm_shuffle_epi32(coeffs_y, 85); // coeffs 10 11 10 11 10 11 10 11
}

static INLINE void prepare_coeffs(const InterpFilterParams* const filter_params, const int subpel_q4,
                                  __m128i* const coeffs /* [4] */) {
    const int16_t* filter = av1_get_interp_filter_subpel_kernel(*filter_params, subpel_q4 & SUBPEL_MASK);
    const __m128i  coeff  = _mm_loadu_si128((__m128i*)filter);

    // coeffs 0 1 0 1 0 1 0 1
    coeffs[0] = _mm_shuffle_epi32(coeff, 0x00);
    // coeffs 2 3 2 3 2 3 2 3
    coeffs[1] = _mm_shuffle_epi32(coeff, 0x55);
    // coeffs 4 5 4 5 4 5 4 5
    coeffs[2] = _mm_shuffle_epi32(coeff, 0xaa);
    // coeffs 6 7 6 7 6 7 6 7
    coeffs[3] = _mm_shuffle_epi32(coeff, 0xff);
}

static INLINE __m128i convolve_12tap(const __m128i* s, const __m128i* coeffs) {
    const __m128i d0     = _mm_madd_epi16(s[0], coeffs[0]);
    const __m128i d1     = _mm_madd_epi16(s[1], coeffs[1]);
    const __m128i d2     = _mm_madd_epi16(s[2], coeffs[2]);
    const __m128i d3     = _mm_madd_epi16(s[3], coeffs[3]);
    const __m128i d4     = _mm_madd_epi16(s[4], coeffs[4]);
    const __m128i d5     = _mm_madd_epi16(s[5], coeffs[5]);
    const __m128i d_0123 = _mm_add_epi32(_mm_add_epi32(d0, d1), _mm_add_epi32(d2, d3));
    const __m128i d      = _mm_add_epi32(_mm_add_epi32(d4, d5), d_0123);
    return d;
}

static INLINE __m128i svt_aom_convolve(const __m128i* const s, const __m128i* const coeffs) {
    const __m128i res_0 = _mm_madd_epi16(s[0], coeffs[0]);
    const __m128i res_1 = _mm_madd_epi16(s[1], coeffs[1]);
    const __m128i res_2 = _mm_madd_epi16(s[2], coeffs[2]);
    const __m128i res_3 = _mm_madd_epi16(s[3], coeffs[3]);

    const __m128i res = _mm_add_epi32(_mm_add_epi32(res_0, res_1), _mm_add_epi32(res_2, res_3));

    return res;
}

void svt_av1_highbd_convolve_y_sr_ssse3(const uint16_t* src, int32_t src_stride, uint16_t* dst, int32_t dst_stride,
                                        int32_t w, int32_t h, const InterpFilterParams* filter_params_x,
                                        const InterpFilterParams* filter_params_y, const int32_t subpel_x_q4,
                                        const int32_t subpel_y_q4, ConvolveParams* conv_params, int32_t bd) {
    (void)filter_params_x;
    (void)subpel_x_q4;
    (void)conv_params;
    int                   i, j;
    const int             fo_vert = filter_params_y->taps / 2 - 1;
    const uint16_t* const src_ptr = src - fo_vert * src_stride;
    const int             bits    = FILTER_BITS;

    const __m128i round_shift_bits = _mm_cvtsi32_si128(bits);
    const __m128i round_const_bits = _mm_set1_epi32((1 << bits) >> 1);
    const __m128i clp_pxl          = _mm_set1_epi16(bd == 10 ? 1023 : (bd == 12 ? 4095 : 255));
    const __m128i zero             = _mm_setzero_si128();
    if (filter_params_y->taps == 12) {
        __m128i s[24], coeffs_y[6];

        svt_prepare_coeffs_12tap(filter_params_y, subpel_y_q4, coeffs_y);

        for (j = 0; j < w; j += 8) {
            const uint16_t* data = &src_ptr[j];
            /* Vertical filter */
            __m128i s0  = _mm_loadu_si128((__m128i*)(data + 0 * src_stride));
            __m128i s1  = _mm_loadu_si128((__m128i*)(data + 1 * src_stride));
            __m128i s2  = _mm_loadu_si128((__m128i*)(data + 2 * src_stride));
            __m128i s3  = _mm_loadu_si128((__m128i*)(data + 3 * src_stride));
            __m128i s4  = _mm_loadu_si128((__m128i*)(data + 4 * src_stride));
            __m128i s5  = _mm_loadu_si128((__m128i*)(data + 5 * src_stride));
            __m128i s6  = _mm_loadu_si128((__m128i*)(data + 6 * src_stride));
            __m128i s7  = _mm_loadu_si128((__m128i*)(data + 7 * src_stride));
            __m128i s8  = _mm_loadu_si128((__m128i*)(data + 8 * src_stride));
            __m128i s9  = _mm_loadu_si128((__m128i*)(data + 9 * src_stride));
            __m128i s10 = _mm_loadu_si128((__m128i*)(data + 10 * src_stride));

            s[0] = _mm_unpacklo_epi16(s0, s1);
            s[1] = _mm_unpacklo_epi16(s2, s3);
            s[2] = _mm_unpacklo_epi16(s4, s5);
            s[3] = _mm_unpacklo_epi16(s6, s7);
            s[4] = _mm_unpacklo_epi16(s8, s9);

            s[6]  = _mm_unpackhi_epi16(s0, s1);
            s[7]  = _mm_unpackhi_epi16(s2, s3);
            s[8]  = _mm_unpackhi_epi16(s4, s5);
            s[9]  = _mm_unpackhi_epi16(s6, s7);
            s[10] = _mm_unpackhi_epi16(s8, s9);

            s[12] = _mm_unpacklo_epi16(s1, s2);
            s[13] = _mm_unpacklo_epi16(s3, s4);
            s[14] = _mm_unpacklo_epi16(s5, s6);
            s[15] = _mm_unpacklo_epi16(s7, s8);
            s[16] = _mm_unpacklo_epi16(s9, s10);

            s[18] = _mm_unpackhi_epi16(s1, s2);
            s[19] = _mm_unpackhi_epi16(s3, s4);
            s[20] = _mm_unpackhi_epi16(s5, s6);
            s[21] = _mm_unpackhi_epi16(s7, s8);
            s[22] = _mm_unpackhi_epi16(s9, s10);

            for (i = 0; i < h; i += 2) {
                data = &src_ptr[i * src_stride + j];

                __m128i s11 = _mm_loadu_si128((__m128i*)(data + 11 * src_stride));
                __m128i s12 = _mm_loadu_si128((__m128i*)(data + 12 * src_stride));

                s[5]  = _mm_unpacklo_epi16(s10, s11);
                s[11] = _mm_unpackhi_epi16(s10, s11);

                s[17] = _mm_unpacklo_epi16(s11, s12);
                s[23] = _mm_unpackhi_epi16(s11, s12);

                const __m128i res_a0       = convolve_12tap(s, coeffs_y);
                __m128i       res_a_round0 = _mm_sra_epi32(_mm_add_epi32(res_a0, round_const_bits), round_shift_bits);

                const __m128i res_a1       = convolve_12tap(s + 12, coeffs_y);
                __m128i       res_a_round1 = _mm_sra_epi32(_mm_add_epi32(res_a1, round_const_bits), round_shift_bits);

                if (w - j > 4) {
                    const __m128i res_b0 = convolve_12tap(s + 6, coeffs_y);
                    __m128i res_b_round0 = _mm_sra_epi32(_mm_add_epi32(res_b0, round_const_bits), round_shift_bits);

                    const __m128i res_b1 = convolve_12tap(s + 18, coeffs_y);
                    __m128i res_b_round1 = _mm_sra_epi32(_mm_add_epi32(res_b1, round_const_bits), round_shift_bits);

                    __m128i res_16bit0 = _mm_packs_epi32(res_a_round0, res_b_round0);
                    res_16bit0         = _mm_min_epi16(res_16bit0, clp_pxl);
                    res_16bit0         = _mm_max_epi16(res_16bit0, zero);

                    __m128i res_16bit1 = _mm_packs_epi32(res_a_round1, res_b_round1);
                    res_16bit1         = _mm_min_epi16(res_16bit1, clp_pxl);
                    res_16bit1         = _mm_max_epi16(res_16bit1, zero);

                    _mm_storeu_si128((__m128i*)&dst[i * dst_stride + j], res_16bit0);
                    _mm_storeu_si128((__m128i*)&dst[i * dst_stride + j + dst_stride], res_16bit1);
                } else if (w == 4) {
                    res_a_round0 = _mm_packs_epi32(res_a_round0, res_a_round0);
                    res_a_round0 = _mm_min_epi16(res_a_round0, clp_pxl);
                    res_a_round0 = _mm_max_epi16(res_a_round0, zero);

                    res_a_round1 = _mm_packs_epi32(res_a_round1, res_a_round1);
                    res_a_round1 = _mm_min_epi16(res_a_round1, clp_pxl);
                    res_a_round1 = _mm_max_epi16(res_a_round1, zero);

                    _mm_storel_epi64((__m128i*)&dst[i * dst_stride + j], res_a_round0);
                    _mm_storel_epi64((__m128i*)&dst[i * dst_stride + j + dst_stride], res_a_round1);
                } else {
                    res_a_round0 = _mm_packs_epi32(res_a_round0, res_a_round0);
                    res_a_round0 = _mm_min_epi16(res_a_round0, clp_pxl);
                    res_a_round0 = _mm_max_epi16(res_a_round0, zero);

                    res_a_round1 = _mm_packs_epi32(res_a_round1, res_a_round1);
                    res_a_round1 = _mm_min_epi16(res_a_round1, clp_pxl);
                    res_a_round1 = _mm_max_epi16(res_a_round1, zero);

                    *((uint32_t*)(&dst[i * dst_stride + j])) = _mm_cvtsi128_si32(res_a_round0);

                    *((uint32_t*)(&dst[i * dst_stride + j + dst_stride])) = _mm_cvtsi128_si32(res_a_round1);
                }

                s[0] = s[1];
                s[1] = s[2];
                s[2] = s[3];
                s[3] = s[4];
                s[4] = s[5];

                s[6]  = s[7];
                s[7]  = s[8];
                s[8]  = s[9];
                s[9]  = s[10];
                s[10] = s[11];

                s[12] = s[13];
                s[13] = s[14];
                s[14] = s[15];
                s[15] = s[16];
                s[16] = s[17];

                s[18] = s[19];
                s[19] = s[20];
                s[20] = s[21];
                s[21] = s[22];
                s[22] = s[23];

                s10 = s12;
            }
        }
    } else {
        __m128i s[16], coeffs_y[4];

        prepare_coeffs(filter_params_y, subpel_y_q4, coeffs_y);

        for (j = 0; j < w; j += 8) {
            const uint16_t* data = &src_ptr[j];
            /* Vertical filter */
            {
                __m128i s0 = _mm_loadu_si128((__m128i*)(data + 0 * src_stride));
                __m128i s1 = _mm_loadu_si128((__m128i*)(data + 1 * src_stride));
                __m128i s2 = _mm_loadu_si128((__m128i*)(data + 2 * src_stride));
                __m128i s3 = _mm_loadu_si128((__m128i*)(data + 3 * src_stride));
                __m128i s4 = _mm_loadu_si128((__m128i*)(data + 4 * src_stride));
                __m128i s5 = _mm_loadu_si128((__m128i*)(data + 5 * src_stride));
                __m128i s6 = _mm_loadu_si128((__m128i*)(data + 6 * src_stride));

                s[0] = _mm_unpacklo_epi16(s0, s1);
                s[1] = _mm_unpacklo_epi16(s2, s3);
                s[2] = _mm_unpacklo_epi16(s4, s5);

                s[4] = _mm_unpackhi_epi16(s0, s1);
                s[5] = _mm_unpackhi_epi16(s2, s3);
                s[6] = _mm_unpackhi_epi16(s4, s5);

                s[0 + 8] = _mm_unpacklo_epi16(s1, s2);
                s[1 + 8] = _mm_unpacklo_epi16(s3, s4);
                s[2 + 8] = _mm_unpacklo_epi16(s5, s6);

                s[4 + 8] = _mm_unpackhi_epi16(s1, s2);
                s[5 + 8] = _mm_unpackhi_epi16(s3, s4);
                s[6 + 8] = _mm_unpackhi_epi16(s5, s6);

                for (i = 0; i < h; i += 2) {
                    data = &src_ptr[i * src_stride + j];

                    __m128i s7 = _mm_loadu_si128((__m128i*)(data + 7 * src_stride));
                    __m128i s8 = _mm_loadu_si128((__m128i*)(data + 8 * src_stride));

                    s[3] = _mm_unpacklo_epi16(s6, s7);
                    s[7] = _mm_unpackhi_epi16(s6, s7);

                    s[3 + 8] = _mm_unpacklo_epi16(s7, s8);
                    s[7 + 8] = _mm_unpackhi_epi16(s7, s8);

                    const __m128i res_a0 = svt_aom_convolve(s, coeffs_y);
                    __m128i res_a_round0 = _mm_sra_epi32(_mm_add_epi32(res_a0, round_const_bits), round_shift_bits);

                    const __m128i res_a1 = svt_aom_convolve(s + 8, coeffs_y);
                    __m128i res_a_round1 = _mm_sra_epi32(_mm_add_epi32(res_a1, round_const_bits), round_shift_bits);

                    if (w - j > 4) {
                        const __m128i res_b0 = svt_aom_convolve(s + 4, coeffs_y);
                        __m128i res_b_round0 = _mm_sra_epi32(_mm_add_epi32(res_b0, round_const_bits), round_shift_bits);

                        const __m128i res_b1 = svt_aom_convolve(s + 4 + 8, coeffs_y);
                        __m128i res_b_round1 = _mm_sra_epi32(_mm_add_epi32(res_b1, round_const_bits), round_shift_bits);

                        __m128i res_16bit0 = _mm_packs_epi32(res_a_round0, res_b_round0);
                        res_16bit0         = _mm_min_epi16(res_16bit0, clp_pxl);
                        res_16bit0         = _mm_max_epi16(res_16bit0, zero);

                        __m128i res_16bit1 = _mm_packs_epi32(res_a_round1, res_b_round1);
                        res_16bit1         = _mm_min_epi16(res_16bit1, clp_pxl);
                        res_16bit1         = _mm_max_epi16(res_16bit1, zero);

                        _mm_storeu_si128((__m128i*)&dst[i * dst_stride + j], res_16bit0);
                        _mm_storeu_si128((__m128i*)&dst[i * dst_stride + j + dst_stride], res_16bit1);
                    } else if (w == 4) {
                        res_a_round0 = _mm_packs_epi32(res_a_round0, res_a_round0);
                        res_a_round0 = _mm_min_epi16(res_a_round0, clp_pxl);
                        res_a_round0 = _mm_max_epi16(res_a_round0, zero);

                        res_a_round1 = _mm_packs_epi32(res_a_round1, res_a_round1);
                        res_a_round1 = _mm_min_epi16(res_a_round1, clp_pxl);
                        res_a_round1 = _mm_max_epi16(res_a_round1, zero);

                        _mm_storel_epi64((__m128i*)&dst[i * dst_stride + j], res_a_round0);
                        _mm_storel_epi64((__m128i*)&dst[i * dst_stride + j + dst_stride], res_a_round1);
                    } else {
                        res_a_round0 = _mm_packs_epi32(res_a_round0, res_a_round0);
                        res_a_round0 = _mm_min_epi16(res_a_round0, clp_pxl);
                        res_a_round0 = _mm_max_epi16(res_a_round0, zero);

                        res_a_round1 = _mm_packs_epi32(res_a_round1, res_a_round1);
                        res_a_round1 = _mm_min_epi16(res_a_round1, clp_pxl);
                        res_a_round1 = _mm_max_epi16(res_a_round1, zero);

                        *((uint32_t*)(&dst[i * dst_stride + j])) = _mm_cvtsi128_si32(res_a_round0);

                        *((uint32_t*)(&dst[i * dst_stride + j + dst_stride])) = _mm_cvtsi128_si32(res_a_round1);
                    }

                    s[0] = s[1];
                    s[1] = s[2];
                    s[2] = s[3];

                    s[4] = s[5];
                    s[5] = s[6];
                    s[6] = s[7];

                    s[0 + 8] = s[1 + 8];
                    s[1 + 8] = s[2 + 8];
                    s[2 + 8] = s[3 + 8];

                    s[4 + 8] = s[5 + 8];
                    s[5 + 8] = s[6 + 8];
                    s[6 + 8] = s[7 + 8];

                    s6 = s8;
                }
            }
        }
    }
}

void svt_av1_highbd_convolve_x_sr_ssse3(const uint16_t* src, int32_t src_stride, uint16_t* dst, int32_t dst_stride,
                                        int32_t w, int32_t h, const InterpFilterParams* filter_params_x,
                                        const InterpFilterParams* filter_params_y, const int32_t subpel_x_q4,
                                        const int32_t subpel_y_q4, ConvolveParams* conv_params, int32_t bd) {
    (void)filter_params_y;
    (void)subpel_y_q4;
    int                   i, j;
    const int             fo_horiz = filter_params_x->taps / 2 - 1;
    const uint16_t* const src_ptr  = src - fo_horiz;

    // Check that, even with 12-bit input, the intermediate values will fit
    // into an unsigned 16-bit intermediate array.
    assert(bd + FILTER_BITS + 2 - conv_params->round_0 <= 16);

    const __m128i round_const_x = _mm_set1_epi32(((1 << conv_params->round_0) >> 1));
    const __m128i round_shift_x = _mm_cvtsi32_si128(conv_params->round_0);

    const int bits = FILTER_BITS - conv_params->round_0;

    const __m128i round_shift_bits = _mm_cvtsi32_si128(bits);
    const __m128i round_const_bits = _mm_set1_epi32((1 << bits) >> 1);
    const __m128i clp_pxl          = _mm_set1_epi16(bd == 10 ? 1023 : (bd == 12 ? 4095 : 255));
    const __m128i zero             = _mm_setzero_si128();

    if (filter_params_x->taps == 12) {
        __m128i s[6], coeffs_x[6];

        svt_prepare_coeffs_12tap(filter_params_x, subpel_x_q4, coeffs_x);

        for (j = 0; j < w; j += 8) {
            /* Horizontal filter */
            {
                for (i = 0; i < h; i += 1) {
                    const __m128i row00 = _mm_loadu_si128((__m128i*)&src_ptr[i * src_stride + j]);
                    const __m128i row01 = _mm_loadu_si128((__m128i*)&src_ptr[i * src_stride + (j + 8)]);
                    const __m128i row02 = _mm_loadu_si128((__m128i*)&src_ptr[i * src_stride + (j + 16)]);

                    // even pixels
                    s[0] = _mm_alignr_epi8(row01, row00, 0);
                    s[1] = _mm_alignr_epi8(row01, row00, 4);
                    s[2] = _mm_alignr_epi8(row01, row00, 8);
                    s[3] = _mm_alignr_epi8(row01, row00, 12);
                    s[4] = _mm_alignr_epi8(row02, row01, 0);
                    s[5] = _mm_alignr_epi8(row02, row01, 4);

                    __m128i res_even = convolve_12tap(s, coeffs_x);
                    res_even         = _mm_sra_epi32(_mm_add_epi32(res_even, round_const_x), round_shift_x);
                    res_even         = _mm_sra_epi32(_mm_add_epi32(res_even, round_const_bits), round_shift_bits);

                    // odd pixels
                    s[0] = _mm_alignr_epi8(row01, row00, 2);
                    s[1] = _mm_alignr_epi8(row01, row00, 6);
                    s[2] = _mm_alignr_epi8(row01, row00, 10);
                    s[3] = _mm_alignr_epi8(row01, row00, 14);
                    s[4] = _mm_alignr_epi8(row02, row01, 2);
                    s[5] = _mm_alignr_epi8(row02, row01, 6);

                    __m128i res_odd = convolve_12tap(s, coeffs_x);
                    res_odd         = _mm_sra_epi32(_mm_add_epi32(res_odd, round_const_x), round_shift_x);
                    res_odd         = _mm_sra_epi32(_mm_add_epi32(res_odd, round_const_bits), round_shift_bits);

                    __m128i res_even1 = _mm_packs_epi32(res_even, res_even);
                    __m128i res_odd1  = _mm_packs_epi32(res_odd, res_odd);
                    __m128i res       = _mm_unpacklo_epi16(res_even1, res_odd1);

                    res = _mm_min_epi16(res, clp_pxl);
                    res = _mm_max_epi16(res, zero);

                    if (w - j > 4) {
                        _mm_storeu_si128((__m128i*)&dst[i * dst_stride + j], res);
                    } else if (w == 4) {
                        _mm_storel_epi64((__m128i*)&dst[i * dst_stride + j], res);
                    } else {
                        *((uint32_t*)(&dst[i * dst_stride + j])) = _mm_cvtsi128_si32(res);
                    }
                }
            }
        }
    } else {
        __m128i s[4], coeffs_x[4];
        prepare_coeffs(filter_params_x, subpel_x_q4, coeffs_x);

        for (j = 0; j < w; j += 8) {
            /* Horizontal filter */
            {
                for (i = 0; i < h; i += 1) {
                    const __m128i row00 = _mm_loadu_si128((__m128i*)&src_ptr[i * src_stride + j]);
                    const __m128i row01 = _mm_loadu_si128((__m128i*)&src_ptr[i * src_stride + (j + 8)]);

                    // even pixels
                    s[0] = _mm_alignr_epi8(row01, row00, 0);
                    s[1] = _mm_alignr_epi8(row01, row00, 4);
                    s[2] = _mm_alignr_epi8(row01, row00, 8);
                    s[3] = _mm_alignr_epi8(row01, row00, 12);

                    __m128i res_even = svt_aom_convolve(s, coeffs_x);
                    res_even         = _mm_sra_epi32(_mm_add_epi32(res_even, round_const_x), round_shift_x);

                    // odd pixels
                    s[0] = _mm_alignr_epi8(row01, row00, 2);
                    s[1] = _mm_alignr_epi8(row01, row00, 6);
                    s[2] = _mm_alignr_epi8(row01, row00, 10);
                    s[3] = _mm_alignr_epi8(row01, row00, 14);

                    __m128i res_odd = svt_aom_convolve(s, coeffs_x);
                    res_odd         = _mm_sra_epi32(_mm_add_epi32(res_odd, round_const_x), round_shift_x);

                    res_even = _mm_sra_epi32(_mm_add_epi32(res_even, round_const_bits), round_shift_bits);
                    res_odd  = _mm_sra_epi32(_mm_add_epi32(res_odd, round_const_bits), round_shift_bits);

                    __m128i res_even1 = _mm_packs_epi32(res_even, res_even);
                    __m128i res_odd1  = _mm_packs_epi32(res_odd, res_odd);
                    __m128i res       = _mm_unpacklo_epi16(res_even1, res_odd1);

                    res = _mm_min_epi16(res, clp_pxl);
                    res = _mm_max_epi16(res, zero);

                    if (w - j > 4) {
                        _mm_storeu_si128((__m128i*)&dst[i * dst_stride + j], res);
                    } else if (w == 4) {
                        _mm_storel_epi64((__m128i*)&dst[i * dst_stride + j], res);
                    } else {
                        *((uint32_t*)(&dst[i * dst_stride + j])) = _mm_cvtsi128_si32(res);
                    }
                }
            }
        }
    }
}
