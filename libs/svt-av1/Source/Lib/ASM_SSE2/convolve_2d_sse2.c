/*
 * Copyright (c) 2016, Alliance for Open Media. All rights reserved
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at www.aomedia.org/license/software. If the Alliance for Open
 * Media Patent License 1.0 was not distributed with this source code in the
 * PATENTS file, you can obtain it at www.aomedia.org/license/patent.
 */
#include <emmintrin.h>
#include "definitions.h"
#include "common_dsp_rtcd.h"
#include "filter.h"

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

void svt_av1_convolve_2d_sr_12tap_sse2(const uint8_t* src, int src_stride, uint8_t* dst, int dst_stride, int w, int h,
                                       const InterpFilterParams* filter_params_x,
                                       const InterpFilterParams* filter_params_y, const int subpel_x_qn,
                                       const int subpel_y_qn, ConvolveParams* conv_params) {
    const int bd = 8;

    DECLARE_ALIGNED(16, int16_t, im_block[(MAX_SB_SIZE + MAX_FILTER_TAP - 1) * MAX_SB_SIZE]);
    int                  im_h      = h + filter_params_y->taps - 1;
    int                  im_stride = w;
    int                  i, j;
    const int            fo_vert  = filter_params_y->taps / 2 - 1;
    const int            fo_horiz = filter_params_x->taps / 2 - 1;
    const uint8_t* const src_ptr  = src - fo_vert * src_stride - fo_horiz;

    const __m128i zero        = _mm_setzero_si128();
    const int     bits        = FILTER_BITS * 2 - conv_params->round_0 - conv_params->round_1;
    const int     offset_bits = bd + 2 * FILTER_BITS - conv_params->round_0;

    assert(conv_params->round_0 > 0);
    __m128i coeffs[6];

    /* Horizontal filter */
    {
        svt_prepare_coeffs_12tap(filter_params_x, subpel_x_qn, coeffs);

        const __m128i round_const = _mm_set1_epi32((1 << (bd + FILTER_BITS - 1)) + ((1 << conv_params->round_0) >> 1));
        const __m128i round_shift = _mm_cvtsi32_si128(conv_params->round_0);

        for (i = 0; i < im_h; ++i) {
            for (j = 0; j < w; j += 8) {
                const __m128i data   = _mm_loadu_si128((__m128i*)&src_ptr[i * src_stride + j]);
                const __m128i data_2 = _mm_loadu_si128((__m128i*)&src_ptr[i * src_stride + (j + 4)]);

                // Filter even-index pixels
                const __m128i src_0  = _mm_unpacklo_epi8(data, zero);
                const __m128i res_0  = _mm_madd_epi16(src_0, coeffs[0]);
                const __m128i src_2  = _mm_unpacklo_epi8(_mm_srli_si128(data, 2), zero);
                const __m128i res_2  = _mm_madd_epi16(src_2, coeffs[1]);
                const __m128i src_4  = _mm_unpacklo_epi8(data_2, zero);
                const __m128i res_4  = _mm_madd_epi16(src_4, coeffs[2]);
                const __m128i src_6  = _mm_unpacklo_epi8(_mm_srli_si128(data_2, 2), zero);
                const __m128i res_6  = _mm_madd_epi16(src_6, coeffs[3]);
                const __m128i src_8  = _mm_unpacklo_epi8(_mm_srli_si128(data_2, 4), zero);
                const __m128i res_8  = _mm_madd_epi16(src_8, coeffs[4]);
                const __m128i src_10 = _mm_unpacklo_epi8(_mm_srli_si128(data_2, 6), zero);
                const __m128i res_10 = _mm_madd_epi16(src_10, coeffs[5]);

                const __m128i res_0246 = _mm_add_epi32(_mm_add_epi32(res_0, res_4), _mm_add_epi32(res_2, res_6));
                __m128i       res_even = _mm_add_epi32(_mm_add_epi32(res_8, res_10), res_0246);
                res_even               = _mm_sra_epi32(_mm_add_epi32(res_even, round_const), round_shift);

                // Filter odd-index pixels
                const __m128i src_1  = _mm_unpacklo_epi8(_mm_srli_si128(data, 1), zero);
                const __m128i res_1  = _mm_madd_epi16(src_1, coeffs[0]);
                const __m128i src_3  = _mm_unpacklo_epi8(_mm_srli_si128(data, 3), zero);
                const __m128i res_3  = _mm_madd_epi16(src_3, coeffs[1]);
                const __m128i src_5  = _mm_unpacklo_epi8(_mm_srli_si128(data_2, 1), zero);
                const __m128i res_5  = _mm_madd_epi16(src_5, coeffs[2]);
                const __m128i src_7  = _mm_unpacklo_epi8(_mm_srli_si128(data_2, 3), zero);
                const __m128i res_7  = _mm_madd_epi16(src_7, coeffs[3]);
                const __m128i src_9  = _mm_unpacklo_epi8(_mm_srli_si128(data_2, 5), zero);
                const __m128i res_9  = _mm_madd_epi16(src_9, coeffs[4]);
                const __m128i src_11 = _mm_unpacklo_epi8(_mm_srli_si128(data_2, 7), zero);
                const __m128i res_11 = _mm_madd_epi16(src_11, coeffs[5]);

                const __m128i res_1357 = _mm_add_epi32(_mm_add_epi32(res_1, res_5), _mm_add_epi32(res_3, res_7));
                __m128i       res_odd  = _mm_add_epi32(_mm_add_epi32(res_9, res_11), res_1357);
                res_odd                = _mm_sra_epi32(_mm_add_epi32(res_odd, round_const), round_shift);

                // Pack in the column order 0, 2, 4, 6, 1, 3, 5, 7
                __m128i res = _mm_packs_epi32(res_even, res_odd);
                _mm_storeu_si128((__m128i*)&im_block[i * im_stride + j], res);
            }
        }
    }

    /* Vertical filter */
    {
        svt_prepare_coeffs_12tap(filter_params_y, subpel_y_qn, coeffs);

        const __m128i sum_round = _mm_set1_epi32((1 << offset_bits) + ((1 << conv_params->round_1) >> 1));
        const __m128i sum_shift = _mm_cvtsi32_si128(conv_params->round_1);

        const __m128i round_const = _mm_set1_epi32(((1 << bits) >> 1) - (1 << (offset_bits - conv_params->round_1)) -
                                                   ((1 << (offset_bits - conv_params->round_1)) >> 1));
        const __m128i round_shift = _mm_cvtsi32_si128(bits);

        for (i = 0; i < h; ++i) {
            for (j = 0; j < w; j += 8) {
                // Filter even-index pixels
                const int16_t* data   = &im_block[i * im_stride + j];
                const __m128i  src_0  = _mm_unpacklo_epi16(*(__m128i*)(data + 0 * im_stride),
                                                         *(__m128i*)(data + 1 * im_stride));
                const __m128i  src_2  = _mm_unpacklo_epi16(*(__m128i*)(data + 2 * im_stride),
                                                         *(__m128i*)(data + 3 * im_stride));
                const __m128i  src_4  = _mm_unpacklo_epi16(*(__m128i*)(data + 4 * im_stride),
                                                         *(__m128i*)(data + 5 * im_stride));
                const __m128i  src_6  = _mm_unpacklo_epi16(*(__m128i*)(data + 6 * im_stride),
                                                         *(__m128i*)(data + 7 * im_stride));
                const __m128i  src_8  = _mm_unpacklo_epi16(*(__m128i*)(data + 8 * im_stride),
                                                         *(__m128i*)(data + 9 * im_stride));
                const __m128i  src_10 = _mm_unpacklo_epi16(*(__m128i*)(data + 10 * im_stride),
                                                          *(__m128i*)(data + 11 * im_stride));

                const __m128i res_0  = _mm_madd_epi16(src_0, coeffs[0]);
                const __m128i res_2  = _mm_madd_epi16(src_2, coeffs[1]);
                const __m128i res_4  = _mm_madd_epi16(src_4, coeffs[2]);
                const __m128i res_6  = _mm_madd_epi16(src_6, coeffs[3]);
                const __m128i res_8  = _mm_madd_epi16(src_8, coeffs[4]);
                const __m128i res_10 = _mm_madd_epi16(src_10, coeffs[5]);

                const __m128i res_0246 = _mm_add_epi32(_mm_add_epi32(res_0, res_2), _mm_add_epi32(res_4, res_6));
                __m128i       res_even = _mm_add_epi32(_mm_add_epi32(res_8, res_10), res_0246);

                // Filter odd-index pixels
                const __m128i src_1  = _mm_unpackhi_epi16(*(__m128i*)(data + 0 * im_stride),
                                                         *(__m128i*)(data + 1 * im_stride));
                const __m128i src_3  = _mm_unpackhi_epi16(*(__m128i*)(data + 2 * im_stride),
                                                         *(__m128i*)(data + 3 * im_stride));
                const __m128i src_5  = _mm_unpackhi_epi16(*(__m128i*)(data + 4 * im_stride),
                                                         *(__m128i*)(data + 5 * im_stride));
                const __m128i src_7  = _mm_unpackhi_epi16(*(__m128i*)(data + 6 * im_stride),
                                                         *(__m128i*)(data + 7 * im_stride));
                const __m128i src_9  = _mm_unpackhi_epi16(*(__m128i*)(data + 8 * im_stride),
                                                         *(__m128i*)(data + 9 * im_stride));
                const __m128i src_11 = _mm_unpackhi_epi16(*(__m128i*)(data + 10 * im_stride),
                                                          *(__m128i*)(data + 11 * im_stride));

                const __m128i res_1  = _mm_madd_epi16(src_1, coeffs[0]);
                const __m128i res_3  = _mm_madd_epi16(src_3, coeffs[1]);
                const __m128i res_5  = _mm_madd_epi16(src_5, coeffs[2]);
                const __m128i res_7  = _mm_madd_epi16(src_7, coeffs[3]);
                const __m128i res_9  = _mm_madd_epi16(src_9, coeffs[4]);
                const __m128i res_11 = _mm_madd_epi16(src_11, coeffs[5]);

                const __m128i res_1357 = _mm_add_epi32(_mm_add_epi32(res_1, res_5), _mm_add_epi32(res_3, res_7));
                __m128i       res_odd  = _mm_add_epi32(_mm_add_epi32(res_9, res_11), res_1357);

                // Rearrange pixels back into the order 0 ... 7
                const __m128i res_lo = _mm_unpacklo_epi32(res_even, res_odd);
                const __m128i res_hi = _mm_unpackhi_epi32(res_even, res_odd);

                __m128i res_lo_round = _mm_sra_epi32(_mm_add_epi32(res_lo, sum_round), sum_shift);
                __m128i res_hi_round = _mm_sra_epi32(_mm_add_epi32(res_hi, sum_round), sum_shift);

                res_lo_round = _mm_sra_epi32(_mm_add_epi32(res_lo_round, round_const), round_shift);
                res_hi_round = _mm_sra_epi32(_mm_add_epi32(res_hi_round, round_const), round_shift);

                const __m128i res16 = _mm_packs_epi32(res_lo_round, res_hi_round);
                const __m128i res   = _mm_packus_epi16(res16, res16);

                // Accumulate values into the destination buffer
                __m128i* const p = (__m128i*)&dst[i * dst_stride + j];

                _mm_storel_epi64(p, res);
            }
        }
    }
}

void svt_av1_convolve_2d_sr_sse2(const uint8_t* src, int32_t src_stride, uint8_t* dst, int32_t dst_stride, int32_t w,
                                 int32_t h, const InterpFilterParams* filter_params_x,
                                 const InterpFilterParams* filter_params_y, const int32_t subpel_x_q4,
                                 const int32_t subpel_y_q4, ConvolveParams* conv_params) {
    if (filter_params_x->taps > 8) {
        if (w < 8) {
            svt_av1_convolve_2d_sr_c(src,
                                     src_stride,
                                     dst,
                                     dst_stride,
                                     w,
                                     h,
                                     filter_params_x,
                                     filter_params_y,
                                     subpel_x_q4,
                                     subpel_y_q4,
                                     conv_params);
        } else {
            svt_av1_convolve_2d_sr_12tap_sse2(src,
                                              src_stride,
                                              dst,
                                              dst_stride,
                                              w,
                                              h,
                                              filter_params_x,
                                              filter_params_y,
                                              subpel_x_q4,
                                              subpel_y_q4,
                                              conv_params);
        }
    } else {
        const int bd = 8;

        DECLARE_ALIGNED(16, int16_t, im_block[(MAX_SB_SIZE + MAX_FILTER_TAP - 1) * MAX_SB_SIZE]);
        int                  im_h      = h + filter_params_y->taps - 1;
        int                  im_stride = MAX_SB_SIZE;
        int                  i, j;
        const int            fo_vert  = filter_params_y->taps / 2 - 1;
        const int            fo_horiz = filter_params_x->taps / 2 - 1;
        const uint8_t* const src_ptr  = src - fo_vert * src_stride - fo_horiz;

        const __m128i zero        = _mm_setzero_si128();
        const int     bits        = FILTER_BITS * 2 - conv_params->round_0 - conv_params->round_1;
        const int     offset_bits = bd + 2 * FILTER_BITS - conv_params->round_0;

        assert(conv_params->round_0 > 0);

        /* Horizontal filter */
        {
            const int16_t* x_filter = av1_get_interp_filter_subpel_kernel(*filter_params_x, subpel_x_q4 & SUBPEL_MASK);
            const __m128i  coeffs_x = _mm_loadu_si128((__m128i*)x_filter);

            // coeffs 0 1 0 1 2 3 2 3
            const __m128i tmp_0 = _mm_unpacklo_epi32(coeffs_x, coeffs_x);
            // coeffs 4 5 4 5 6 7 6 7
            const __m128i tmp_1 = _mm_unpackhi_epi32(coeffs_x, coeffs_x);

            // coeffs 0 1 0 1 0 1 0 1
            const __m128i coeff_01 = _mm_unpacklo_epi64(tmp_0, tmp_0);
            // coeffs 2 3 2 3 2 3 2 3
            const __m128i coeff_23 = _mm_unpackhi_epi64(tmp_0, tmp_0);
            // coeffs 4 5 4 5 4 5 4 5
            const __m128i coeff_45 = _mm_unpacklo_epi64(tmp_1, tmp_1);
            // coeffs 6 7 6 7 6 7 6 7
            const __m128i coeff_67 = _mm_unpackhi_epi64(tmp_1, tmp_1);

            const __m128i round_const = _mm_set1_epi32((1 << (bd + FILTER_BITS - 1)) +
                                                       ((1 << conv_params->round_0) >> 1));
            const __m128i round_shift = _mm_cvtsi32_si128(conv_params->round_0);

            for (i = 0; i < im_h; ++i) {
                for (j = 0; j < w; j += 8) {
                    const __m128i data = _mm_loadu_si128((__m128i*)&src_ptr[i * src_stride + j]);

                    // Filter even-index pixels
                    const __m128i src_0 = _mm_unpacklo_epi8(data, zero);
                    const __m128i res_0 = _mm_madd_epi16(src_0, coeff_01);
                    const __m128i src_2 = _mm_unpacklo_epi8(_mm_srli_si128(data, 2), zero);
                    const __m128i res_2 = _mm_madd_epi16(src_2, coeff_23);
                    const __m128i src_4 = _mm_unpacklo_epi8(_mm_srli_si128(data, 4), zero);
                    const __m128i res_4 = _mm_madd_epi16(src_4, coeff_45);
                    const __m128i src_6 = _mm_unpacklo_epi8(_mm_srli_si128(data, 6), zero);
                    const __m128i res_6 = _mm_madd_epi16(src_6, coeff_67);

                    __m128i res_even = _mm_add_epi32(_mm_add_epi32(res_0, res_4), _mm_add_epi32(res_2, res_6));
                    res_even         = _mm_sra_epi32(_mm_add_epi32(res_even, round_const), round_shift);

                    // Filter odd-index pixels
                    const __m128i src_1 = _mm_unpacklo_epi8(_mm_srli_si128(data, 1), zero);
                    const __m128i res_1 = _mm_madd_epi16(src_1, coeff_01);
                    const __m128i src_3 = _mm_unpacklo_epi8(_mm_srli_si128(data, 3), zero);
                    const __m128i res_3 = _mm_madd_epi16(src_3, coeff_23);
                    const __m128i src_5 = _mm_unpacklo_epi8(_mm_srli_si128(data, 5), zero);
                    const __m128i res_5 = _mm_madd_epi16(src_5, coeff_45);
                    const __m128i src_7 = _mm_unpacklo_epi8(_mm_srli_si128(data, 7), zero);
                    const __m128i res_7 = _mm_madd_epi16(src_7, coeff_67);

                    __m128i res_odd = _mm_add_epi32(_mm_add_epi32(res_1, res_5), _mm_add_epi32(res_3, res_7));
                    res_odd         = _mm_sra_epi32(_mm_add_epi32(res_odd, round_const), round_shift);

                    // Pack in the column order 0, 2, 4, 6, 1, 3, 5, 7
                    __m128i res = _mm_packs_epi32(res_even, res_odd);
                    _mm_storeu_si128((__m128i*)&im_block[i * im_stride + j], res);
                }
            }
        }

        /* Vertical filter */
        {
            const int16_t* y_filter = av1_get_interp_filter_subpel_kernel(*filter_params_y, subpel_y_q4 & SUBPEL_MASK);
            const __m128i  coeffs_y = _mm_loadu_si128((__m128i*)y_filter);

            // coeffs 0 1 0 1 2 3 2 3
            const __m128i tmp_0 = _mm_unpacklo_epi32(coeffs_y, coeffs_y);
            // coeffs 4 5 4 5 6 7 6 7
            const __m128i tmp_1 = _mm_unpackhi_epi32(coeffs_y, coeffs_y);

            // coeffs 0 1 0 1 0 1 0 1
            const __m128i coeff_01 = _mm_unpacklo_epi64(tmp_0, tmp_0);
            // coeffs 2 3 2 3 2 3 2 3
            const __m128i coeff_23 = _mm_unpackhi_epi64(tmp_0, tmp_0);
            // coeffs 4 5 4 5 4 5 4 5
            const __m128i coeff_45 = _mm_unpacklo_epi64(tmp_1, tmp_1);
            // coeffs 6 7 6 7 6 7 6 7
            const __m128i coeff_67 = _mm_unpackhi_epi64(tmp_1, tmp_1);

            const __m128i sum_round = _mm_set1_epi32((1 << offset_bits) + ((1 << conv_params->round_1) >> 1));
            const __m128i sum_shift = _mm_cvtsi32_si128(conv_params->round_1);

            const __m128i round_const = _mm_set1_epi32(((1 << bits) >> 1) -
                                                       (1 << (offset_bits - conv_params->round_1)) -
                                                       ((1 << (offset_bits - conv_params->round_1)) >> 1));
            const __m128i round_shift = _mm_cvtsi32_si128(bits);

            for (i = 0; i < h; ++i) {
                for (j = 0; j < w; j += 8) {
                    // Filter even-index pixels
                    const int16_t* data  = &im_block[i * im_stride + j];
                    const __m128i  src_0 = _mm_unpacklo_epi16(*(__m128i*)(data + 0 * im_stride),
                                                             *(__m128i*)(data + 1 * im_stride));
                    const __m128i  src_2 = _mm_unpacklo_epi16(*(__m128i*)(data + 2 * im_stride),
                                                             *(__m128i*)(data + 3 * im_stride));
                    const __m128i  src_4 = _mm_unpacklo_epi16(*(__m128i*)(data + 4 * im_stride),
                                                             *(__m128i*)(data + 5 * im_stride));
                    const __m128i  src_6 = _mm_unpacklo_epi16(*(__m128i*)(data + 6 * im_stride),
                                                             *(__m128i*)(data + 7 * im_stride));

                    const __m128i res_0 = _mm_madd_epi16(src_0, coeff_01);
                    const __m128i res_2 = _mm_madd_epi16(src_2, coeff_23);
                    const __m128i res_4 = _mm_madd_epi16(src_4, coeff_45);
                    const __m128i res_6 = _mm_madd_epi16(src_6, coeff_67);

                    const __m128i res_even = _mm_add_epi32(_mm_add_epi32(res_0, res_2), _mm_add_epi32(res_4, res_6));

                    // Filter odd-index pixels
                    const __m128i src_1 = _mm_unpackhi_epi16(*(__m128i*)(data + 0 * im_stride),
                                                             *(__m128i*)(data + 1 * im_stride));
                    const __m128i src_3 = _mm_unpackhi_epi16(*(__m128i*)(data + 2 * im_stride),
                                                             *(__m128i*)(data + 3 * im_stride));
                    const __m128i src_5 = _mm_unpackhi_epi16(*(__m128i*)(data + 4 * im_stride),
                                                             *(__m128i*)(data + 5 * im_stride));
                    const __m128i src_7 = _mm_unpackhi_epi16(*(__m128i*)(data + 6 * im_stride),
                                                             *(__m128i*)(data + 7 * im_stride));

                    const __m128i res_1 = _mm_madd_epi16(src_1, coeff_01);
                    const __m128i res_3 = _mm_madd_epi16(src_3, coeff_23);
                    const __m128i res_5 = _mm_madd_epi16(src_5, coeff_45);
                    const __m128i res_7 = _mm_madd_epi16(src_7, coeff_67);

                    const __m128i res_odd = _mm_add_epi32(_mm_add_epi32(res_1, res_3), _mm_add_epi32(res_5, res_7));

                    // Rearrange pixels back into the order 0 ... 7
                    const __m128i res_lo = _mm_unpacklo_epi32(res_even, res_odd);
                    const __m128i res_hi = _mm_unpackhi_epi32(res_even, res_odd);

                    __m128i res_lo_round = _mm_sra_epi32(_mm_add_epi32(res_lo, sum_round), sum_shift);
                    __m128i res_hi_round = _mm_sra_epi32(_mm_add_epi32(res_hi, sum_round), sum_shift);

                    res_lo_round = _mm_sra_epi32(_mm_add_epi32(res_lo_round, round_const), round_shift);
                    res_hi_round = _mm_sra_epi32(_mm_add_epi32(res_hi_round, round_const), round_shift);

                    const __m128i res16 = _mm_packs_epi32(res_lo_round, res_hi_round);
                    const __m128i res   = _mm_packus_epi16(res16, res16);

                    // Accumulate values into the destination buffer
                    __m128i* const p = (__m128i*)&dst[i * dst_stride + j];

                    if (w == 2) {
                        *(uint16_t*)p = (uint16_t)_mm_cvtsi128_si32(res);
                    } else if (w == 4) {
                        *(uint32_t*)p = _mm_cvtsi128_si32(res);
                    } else {
                        _mm_storel_epi64(p, res);
                    }
                }
            }
        }
    }
}

static INLINE void copy_64(const uint8_t* src, uint8_t* dst) {
    __m128i s[4];
    s[0] = _mm_loadu_si128((__m128i*)src);
    s[1] = _mm_loadu_si128((__m128i*)(src + 16));
    s[2] = _mm_loadu_si128((__m128i*)(src + 32));
    s[3] = _mm_loadu_si128((__m128i*)(src + 48));
    _mm_storeu_si128((__m128i*)dst, s[0]);
    _mm_storeu_si128((__m128i*)(dst + 16), s[1]);
    _mm_storeu_si128((__m128i*)(dst + 32), s[2]);
    _mm_storeu_si128((__m128i*)(dst + 48), s[3]);
}

void svt_av1_convolve_2d_copy_sr_sse2(const uint8_t* src, int32_t src_stride, uint8_t* dst, int32_t dst_stride,
                                      int32_t w, int32_t h, const InterpFilterParams* filter_params_x,
                                      const InterpFilterParams* filter_params_y, const int32_t subpel_x_q4,
                                      const int32_t subpel_y_q4, ConvolveParams* conv_params) {
    (void)filter_params_x;
    (void)filter_params_y;
    (void)subpel_x_q4;
    (void)subpel_y_q4;
    (void)conv_params;

    if (w == 2) {
        do {
            svt_memcpy_intrin_sse(dst, src, 2 * sizeof(*src));
            src += src_stride;
            dst += dst_stride;
            svt_memcpy_intrin_sse(dst, src, 2 * sizeof(*src));
            src += src_stride;
            dst += dst_stride;
            h -= 2;
        } while (h);
    } else if (w == 4) {
        do {
            svt_memcpy_intrin_sse(dst, src, 4 * sizeof(*src));
            src += src_stride;
            dst += dst_stride;
            svt_memcpy_intrin_sse(dst, src, 4 * sizeof(*src));
            src += src_stride;
            dst += dst_stride;
            h -= 2;
        } while (h);
    } else if (w == 8) {
        do {
            __m128i s[2];
            s[0] = _mm_loadl_epi64((__m128i*)src);
            src += src_stride;
            s[1] = _mm_loadl_epi64((__m128i*)src);
            src += src_stride;
            _mm_storel_epi64((__m128i*)dst, s[0]);
            dst += dst_stride;
            _mm_storel_epi64((__m128i*)dst, s[1]);
            dst += dst_stride;
            h -= 2;
        } while (h);
    } else if (w == 16) {
        do {
            __m128i s[2];
            s[0] = _mm_loadu_si128((__m128i*)src);
            src += src_stride;
            s[1] = _mm_loadu_si128((__m128i*)src);
            src += src_stride;
            _mm_storeu_si128((__m128i*)dst, s[0]);
            dst += dst_stride;
            _mm_storeu_si128((__m128i*)dst, s[1]);
            dst += dst_stride;
            h -= 2;
        } while (h);
    } else if (w == 32) {
        do {
            __m128i s[4];
            s[0] = _mm_loadu_si128((__m128i*)src);
            s[1] = _mm_loadu_si128((__m128i*)(src + 16));
            src += src_stride;
            s[2] = _mm_loadu_si128((__m128i*)src);
            s[3] = _mm_loadu_si128((__m128i*)(src + 16));
            src += src_stride;
            _mm_storeu_si128((__m128i*)dst, s[0]);
            _mm_storeu_si128((__m128i*)(dst + 16), s[1]);
            dst += dst_stride;
            _mm_storeu_si128((__m128i*)dst, s[2]);
            _mm_storeu_si128((__m128i*)(dst + 16), s[3]);
            dst += dst_stride;
            h -= 2;
        } while (h);
    } else if (w == 64) {
        do {
            copy_64(src, dst);
            src += src_stride;
            dst += dst_stride;
            copy_64(src, dst);
            src += src_stride;
            dst += dst_stride;
            h -= 2;
        } while (h);
    } else {
        do {
            copy_64(src, dst);
            copy_64(src + 64, dst + 64);
            src += src_stride;
            dst += dst_stride;
            copy_64(src, dst);
            copy_64(src + 64, dst + 64);
            src += src_stride;
            dst += dst_stride;
            h -= 2;
        } while (h);
    }
}
