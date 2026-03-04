/*
* Copyright(c) 2019 Intel Corporation
* Copyright (c) 2016, Alliance for Open Media. All rights reserved
*
* This source code is subject to the terms of the BSD 2 Clause License and
* the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
* was not distributed with this source code in the LICENSE file, you can
* obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
* Media Patent License 1.0 was not distributed with this source code in the
* PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
*/

#include <immintrin.h>
#include <assert.h>

#include "definitions.h"
#include "common_dsp_rtcd.h"

#include "convolve_avx2.h"
#include "convolve.h"
#include "synonyms_avx2.h"

#include "inter_prediction.h"

void svt_av1_highbd_jnt_convolve_2d_copy_avx2(const uint16_t* src, int32_t src_stride, uint16_t* dst0,
                                              int32_t dst_stride0, int32_t w, int32_t h,
                                              const InterpFilterParams* filter_params_x,
                                              const InterpFilterParams* filter_params_y, const int32_t subpel_x_q4,
                                              const int32_t subpel_y_q4, ConvolveParams* conv_params, int32_t bd) {
    ConvBufType* dst        = conv_params->dst;
    int32_t      dst_stride = conv_params->dst_stride;
    (void)filter_params_x;
    (void)filter_params_y;
    (void)subpel_x_q4;
    (void)subpel_y_q4;

    const int32_t bits             = FILTER_BITS * 2 - conv_params->round_1 - conv_params->round_0;
    const __m128i left_shift       = _mm_cvtsi32_si128(bits);
    const int32_t do_average       = conv_params->do_average;
    const int32_t use_jnt_comp_avg = conv_params->use_jnt_comp_avg;
    const int32_t w0               = conv_params->fwd_offset;
    const int32_t w1               = conv_params->bck_offset;
    const __m256i wt0              = _mm256_set1_epi32(w0);
    const __m256i wt1              = _mm256_set1_epi32(w1);
    const __m256i zero             = _mm256_setzero_si256();
    int32_t       i, j;

    const int32_t offset_0         = bd + 2 * FILTER_BITS - conv_params->round_0 - conv_params->round_1;
    const int32_t offset           = (1 << offset_0) + (1 << (offset_0 - 1));
    const __m256i offset_const     = _mm256_set1_epi32(offset);
    const __m256i offset_const_16b = _mm256_set1_epi16(offset);
    const int32_t rounding_shift   = 2 * FILTER_BITS - conv_params->round_0 - conv_params->round_1;
    const __m256i rounding_const   = _mm256_set1_epi32((1 << rounding_shift) >> 1);
    const __m256i clip_pixel_to_bd = _mm256_set1_epi16(bd == 10 ? 1023 : (bd == 12 ? 4095 : 255));

    assert(bits <= 4);

    if (!(w % 16)) {
        for (i = 0; i < h; i += 1) {
            for (j = 0; j < w; j += 16) {
                const __m256i src_16bit = _mm256_loadu_si256((__m256i*)(&src[i * src_stride + j]));

                const __m256i res = _mm256_sll_epi16(src_16bit, left_shift);

                if (do_average) {
                    const __m256i data_0 = _mm256_loadu_si256((__m256i*)(&dst[i * dst_stride + j]));

                    const __m256i data_ref_0_lo = _mm256_unpacklo_epi16(data_0, zero);
                    const __m256i data_ref_0_hi = _mm256_unpackhi_epi16(data_0, zero);

                    const __m256i res_32b_lo      = _mm256_unpacklo_epi16(res, zero);
                    const __m256i res_unsigned_lo = _mm256_add_epi32(res_32b_lo, offset_const);

                    const __m256i comp_avg_res_lo = highbd_comp_avg(
                        &data_ref_0_lo, &res_unsigned_lo, &wt0, &wt1, use_jnt_comp_avg);

                    const __m256i res_32b_hi      = _mm256_unpackhi_epi16(res, zero);
                    const __m256i res_unsigned_hi = _mm256_add_epi32(res_32b_hi, offset_const);

                    const __m256i comp_avg_res_hi = highbd_comp_avg(
                        &data_ref_0_hi, &res_unsigned_hi, &wt0, &wt1, use_jnt_comp_avg);

                    const __m256i round_result_lo = highbd_convolve_rounding(
                        &comp_avg_res_lo, &offset_const, &rounding_const, rounding_shift);
                    const __m256i round_result_hi = highbd_convolve_rounding(
                        &comp_avg_res_hi, &offset_const, &rounding_const, rounding_shift);

                    const __m256i res_16b  = _mm256_packus_epi32(round_result_lo, round_result_hi);
                    const __m256i res_clip = _mm256_min_epi16(res_16b, clip_pixel_to_bd);

                    _mm256_storeu_si256((__m256i*)(&dst0[i * dst_stride0 + j]), res_clip);
                } else {
                    const __m256i res_unsigned_16b = _mm256_adds_epu16(res, offset_const_16b);

                    _mm256_storeu_si256((__m256i*)(&dst[i * dst_stride + j]), res_unsigned_16b);
                }
            }
        }
    } else if (!(w % 4)) {
        for (i = 0; i < h; i += 2) {
            for (j = 0; j < w; j += 8) {
                const __m128i src_row_0 = _mm_loadu_si128((__m128i*)(&src[i * src_stride + j]));
                const __m128i src_row_1 = _mm_loadu_si128((__m128i*)(&src[i * src_stride + j + src_stride]));
                const __m256i src_10    = yy_setr_m128i(src_row_0, src_row_1);

                const __m256i res = _mm256_sll_epi16(src_10, left_shift);

                if (w - j < 8) {
                    if (do_average) {
                        const __m256i data_0 = _mm256_castsi128_si256(
                            _mm_loadl_epi64((__m128i*)(&dst[i * dst_stride + j])));
                        const __m256i data_1 = _mm256_castsi128_si256(
                            _mm_loadl_epi64((__m128i*)(&dst[i * dst_stride + j + dst_stride])));
                        const __m256i data_01 = _mm256_permute2x128_si256(data_0, data_1, 0x20);

                        const __m256i data_ref_0 = _mm256_unpacklo_epi16(data_01, zero);

                        const __m256i res_32b         = _mm256_unpacklo_epi16(res, zero);
                        const __m256i res_unsigned_lo = _mm256_add_epi32(res_32b, offset_const);

                        const __m256i comp_avg_res = highbd_comp_avg(
                            &data_ref_0, &res_unsigned_lo, &wt0, &wt1, use_jnt_comp_avg);

                        const __m256i round_result = highbd_convolve_rounding(
                            &comp_avg_res, &offset_const, &rounding_const, rounding_shift);

                        const __m256i res_16b  = _mm256_packus_epi32(round_result, round_result);
                        const __m256i res_clip = _mm256_min_epi16(res_16b, clip_pixel_to_bd);

                        const __m128i res_0 = _mm256_castsi256_si128(res_clip);
                        const __m128i res_1 = _mm256_extracti128_si256(res_clip, 1);

                        _mm_storel_epi64((__m128i*)(&dst0[i * dst_stride0 + j]), res_0);
                        _mm_storel_epi64((__m128i*)(&dst0[i * dst_stride0 + j + dst_stride0]), res_1);
                    } else {
                        const __m256i res_unsigned_16b = _mm256_adds_epu16(res, offset_const_16b);

                        const __m128i res_0 = _mm256_castsi256_si128(res_unsigned_16b);
                        const __m128i res_1 = _mm256_extracti128_si256(res_unsigned_16b, 1);

                        _mm_storel_epi64((__m128i*)(&dst[i * dst_stride + j]), res_0);
                        _mm_storel_epi64((__m128i*)(&dst[i * dst_stride + j + dst_stride]), res_1);
                    }
                } else {
                    if (do_average) {
                        const __m128i data_0  = _mm_loadu_si128((__m128i*)(&dst[i * dst_stride + j]));
                        const __m128i data_1  = _mm_loadu_si128((__m128i*)(&dst[i * dst_stride + j + dst_stride]));
                        const __m256i data_01 = yy_setr_m128i(data_0, data_1);

                        const __m256i data_ref_0_lo = _mm256_unpacklo_epi16(data_01, zero);
                        const __m256i data_ref_0_hi = _mm256_unpackhi_epi16(data_01, zero);

                        const __m256i res_32b_lo      = _mm256_unpacklo_epi16(res, zero);
                        const __m256i res_unsigned_lo = _mm256_add_epi32(res_32b_lo, offset_const);

                        const __m256i comp_avg_res_lo = highbd_comp_avg(
                            &data_ref_0_lo, &res_unsigned_lo, &wt0, &wt1, use_jnt_comp_avg);

                        const __m256i res_32b_hi      = _mm256_unpackhi_epi16(res, zero);
                        const __m256i res_unsigned_hi = _mm256_add_epi32(res_32b_hi, offset_const);

                        const __m256i comp_avg_res_hi = highbd_comp_avg(
                            &data_ref_0_hi, &res_unsigned_hi, &wt0, &wt1, use_jnt_comp_avg);

                        const __m256i round_result_lo = highbd_convolve_rounding(
                            &comp_avg_res_lo, &offset_const, &rounding_const, rounding_shift);
                        const __m256i round_result_hi = highbd_convolve_rounding(
                            &comp_avg_res_hi, &offset_const, &rounding_const, rounding_shift);

                        const __m256i res_16b  = _mm256_packus_epi32(round_result_lo, round_result_hi);
                        const __m256i res_clip = _mm256_min_epi16(res_16b, clip_pixel_to_bd);

                        const __m128i res_0 = _mm256_castsi256_si128(res_clip);
                        const __m128i res_1 = _mm256_extracti128_si256(res_clip, 1);

                        _mm_storeu_si128((__m128i*)(&dst0[i * dst_stride0 + j]), res_0);
                        _mm_storeu_si128((__m128i*)(&dst0[i * dst_stride0 + j + dst_stride0]), res_1);
                    } else {
                        const __m256i res_unsigned_16b = _mm256_adds_epu16(res, offset_const_16b);
                        const __m128i res_0            = _mm256_castsi256_si128(res_unsigned_16b);
                        const __m128i res_1            = _mm256_extracti128_si256(res_unsigned_16b, 1);

                        _mm_storeu_si128((__m128i*)(&dst[i * dst_stride + j]), res_0);
                        _mm_storeu_si128((__m128i*)(&dst[i * dst_stride + j + dst_stride]), res_1);
                    }
                }
            }
        }
    } else {
        svt_av1_highbd_jnt_convolve_2d_c(src,
                                         src_stride,
                                         dst0,
                                         dst_stride0,
                                         w,
                                         h,
                                         filter_params_x,
                                         filter_params_y,
                                         subpel_x_q4,
                                         subpel_y_q4,
                                         conv_params,
                                         bd);
    }
}

void svt_av1_highbd_jnt_convolve_2d_avx2(const uint16_t* src, int32_t src_stride, uint16_t* dst0, int32_t dst_stride0,
                                         int32_t w, int32_t h, const InterpFilterParams* filter_params_x,
                                         const InterpFilterParams* filter_params_y, const int32_t subpel_x_q4,
                                         const int32_t subpel_y_q4, ConvolveParams* conv_params, int32_t bd) {
    DECLARE_ALIGNED(32, int16_t, im_block[(MAX_SB_SIZE + MAX_FILTER_TAP) * 8]);
    ConvBufType*          dst        = conv_params->dst;
    int32_t               dst_stride = conv_params->dst_stride;
    int32_t               im_h       = h + filter_params_y->taps - 1;
    int32_t               im_stride  = 8;
    int32_t               i, j;
    const int32_t         fo_vert  = filter_params_y->taps / 2 - 1;
    const int32_t         fo_horiz = filter_params_x->taps / 2 - 1;
    const uint16_t* const src_ptr  = src - fo_vert * src_stride - fo_horiz;

    // Check that, even with 12-bit input, the intermediate values will fit
    // into an unsigned 16-bit intermediate array.
    assert(bd + FILTER_BITS + 2 - conv_params->round_0 <= 16);

    __m256i       s[8], coeffs_y[4], coeffs_x[4];
    const int32_t do_average       = conv_params->do_average;
    const int32_t use_jnt_comp_avg = conv_params->use_jnt_comp_avg;

    const int32_t w0   = conv_params->fwd_offset;
    const int32_t w1   = conv_params->bck_offset;
    const __m256i wt0  = _mm256_set1_epi32(w0);
    const __m256i wt1  = _mm256_set1_epi32(w1);
    const __m256i zero = _mm256_setzero_si256();

    const __m256i round_const_x = _mm256_set1_epi32(((1 << conv_params->round_0) >> 1) + (1 << (bd + FILTER_BITS - 1)));
    const __m128i round_shift_x = _mm_cvtsi32_si128(conv_params->round_0);

    const __m256i round_const_y = _mm256_set1_epi32(((1 << conv_params->round_1) >> 1) -
                                                    (1 << (bd + 2 * FILTER_BITS - conv_params->round_0 - 1)));
    const __m128i round_shift_y = _mm_cvtsi32_si128(conv_params->round_1);

    const int32_t offset_0       = bd + 2 * FILTER_BITS - conv_params->round_0 - conv_params->round_1;
    const int32_t offset         = (1 << offset_0) + (1 << (offset_0 - 1));
    const __m256i offset_const   = _mm256_set1_epi32(offset);
    const int32_t rounding_shift = 2 * FILTER_BITS - conv_params->round_0 - conv_params->round_1;
    const __m256i rounding_const = _mm256_set1_epi32((1 << rounding_shift) >> 1);

    const __m256i clip_pixel_to_bd = _mm256_set1_epi16(bd == 10 ? 1023 : (bd == 12 ? 4095 : 255));

    prepare_coeffs_8tap_avx2(filter_params_x, subpel_x_q4, coeffs_x);
    prepare_coeffs_8tap_avx2(filter_params_y, subpel_y_q4, coeffs_y);

    for (j = 0; j < w - 2; j += 8) {
        /* Horizontal filter */
        {
            for (i = 0; i < im_h; i += 2) {
                const __m256i row0 = _mm256_loadu_si256((__m256i*)&src_ptr[i * src_stride + j]);
                __m256i       row1 = _mm256_set1_epi16(0);
                if (i + 1 < im_h) {
                    row1 = _mm256_loadu_si256((__m256i*)&src_ptr[(i + 1) * src_stride + j]);
                }

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

                __m256i res_even1 = _mm256_packs_epi32(res_even, res_even);
                __m256i res_odd1  = _mm256_packs_epi32(res_odd, res_odd);
                __m256i res       = _mm256_unpacklo_epi16(res_even1, res_odd1);

                _mm256_storeu_si256((__m256i*)&im_block[i * im_stride], res);
            }
        }

        /* Vertical filter */
        {
            __m256i s0 = _mm256_loadu_si256((__m256i*)(im_block + 0 * im_stride));
            __m256i s1 = _mm256_loadu_si256((__m256i*)(im_block + 1 * im_stride));
            __m256i s2 = _mm256_loadu_si256((__m256i*)(im_block + 2 * im_stride));
            __m256i s3 = _mm256_loadu_si256((__m256i*)(im_block + 3 * im_stride));
            __m256i s4 = _mm256_loadu_si256((__m256i*)(im_block + 4 * im_stride));
            __m256i s5 = _mm256_loadu_si256((__m256i*)(im_block + 5 * im_stride));

            s[0] = _mm256_unpacklo_epi16(s0, s1);
            s[1] = _mm256_unpacklo_epi16(s2, s3);
            s[2] = _mm256_unpacklo_epi16(s4, s5);

            s[4] = _mm256_unpackhi_epi16(s0, s1);
            s[5] = _mm256_unpackhi_epi16(s2, s3);
            s[6] = _mm256_unpackhi_epi16(s4, s5);

            for (i = 0; i < h; i += 2) {
                const int16_t* data = &im_block[i * im_stride];

                const __m256i s6 = _mm256_loadu_si256((__m256i*)(data + 6 * im_stride));
                const __m256i s7 = _mm256_loadu_si256((__m256i*)(data + 7 * im_stride));

                s[3] = _mm256_unpacklo_epi16(s6, s7);
                s[7] = _mm256_unpackhi_epi16(s6, s7);

                const __m256i res_a = convolve16_8tap_avx2(s, coeffs_y);

                const __m256i res_a_round = _mm256_sra_epi32(_mm256_add_epi32(res_a, round_const_y), round_shift_y);

                const __m256i res_unsigned_lo = _mm256_add_epi32(res_a_round, offset_const);

                if (w - j < 8) {
                    if (do_average) {
                        const __m256i data_0 = _mm256_castsi128_si256(
                            _mm_loadl_epi64((__m128i*)(&dst[i * dst_stride + j])));
                        const __m256i data_1 = _mm256_castsi128_si256(
                            _mm_loadl_epi64((__m128i*)(&dst[i * dst_stride + j + dst_stride])));
                        const __m256i data_01 = _mm256_permute2x128_si256(data_0, data_1, 0x20);

                        const __m256i data_ref_0 = _mm256_unpacklo_epi16(data_01, zero);

                        const __m256i comp_avg_res = highbd_comp_avg(
                            &data_ref_0, &res_unsigned_lo, &wt0, &wt1, use_jnt_comp_avg);

                        const __m256i round_result = highbd_convolve_rounding(
                            &comp_avg_res, &offset_const, &rounding_const, rounding_shift);

                        const __m256i res_16b  = _mm256_packus_epi32(round_result, round_result);
                        const __m256i res_clip = _mm256_min_epi16(res_16b, clip_pixel_to_bd);

                        const __m128i res_0 = _mm256_castsi256_si128(res_clip);
                        const __m128i res_1 = _mm256_extracti128_si256(res_clip, 1);

                        _mm_storel_epi64((__m128i*)(&dst0[i * dst_stride0 + j]), res_0);
                        _mm_storel_epi64((__m128i*)(&dst0[i * dst_stride0 + j + dst_stride0]), res_1);
                    } else {
                        __m256i       res_16b = _mm256_packus_epi32(res_unsigned_lo, res_unsigned_lo);
                        const __m128i res_0   = _mm256_castsi256_si128(res_16b);
                        const __m128i res_1   = _mm256_extracti128_si256(res_16b, 1);

                        _mm_storel_epi64((__m128i*)(&dst[i * dst_stride + j]), res_0);
                        _mm_storel_epi64((__m128i*)(&dst[i * dst_stride + j + dst_stride]), res_1);
                    }
                } else {
                    const __m256i res_b       = convolve16_8tap_avx2(s + 4, coeffs_y);
                    const __m256i res_b_round = _mm256_sra_epi32(_mm256_add_epi32(res_b, round_const_y), round_shift_y);

                    __m256i res_unsigned_hi = _mm256_add_epi32(res_b_round, offset_const);

                    if (do_average) {
                        const __m128i data_0  = _mm_loadu_si128((__m128i*)(&dst[i * dst_stride + j]));
                        const __m128i data_1  = _mm_loadu_si128((__m128i*)(&dst[i * dst_stride + j + dst_stride]));
                        const __m256i data_01 = yy_setr_m128i(data_0, data_1);

                        const __m256i data_ref_0_lo = _mm256_unpacklo_epi16(data_01, zero);
                        const __m256i data_ref_0_hi = _mm256_unpackhi_epi16(data_01, zero);

                        const __m256i comp_avg_res_lo = highbd_comp_avg(
                            &data_ref_0_lo, &res_unsigned_lo, &wt0, &wt1, use_jnt_comp_avg);
                        const __m256i comp_avg_res_hi = highbd_comp_avg(
                            &data_ref_0_hi, &res_unsigned_hi, &wt0, &wt1, use_jnt_comp_avg);

                        const __m256i round_result_lo = highbd_convolve_rounding(
                            &comp_avg_res_lo, &offset_const, &rounding_const, rounding_shift);
                        const __m256i round_result_hi = highbd_convolve_rounding(
                            &comp_avg_res_hi, &offset_const, &rounding_const, rounding_shift);

                        const __m256i res_16b  = _mm256_packus_epi32(round_result_lo, round_result_hi);
                        const __m256i res_clip = _mm256_min_epi16(res_16b, clip_pixel_to_bd);

                        const __m128i res_0 = _mm256_castsi256_si128(res_clip);
                        const __m128i res_1 = _mm256_extracti128_si256(res_clip, 1);

                        _mm_storeu_si128((__m128i*)(&dst0[i * dst_stride0 + j]), res_0);
                        _mm_storeu_si128((__m128i*)(&dst0[i * dst_stride0 + j + dst_stride0]), res_1);
                    } else {
                        __m256i       res_16b = _mm256_packus_epi32(res_unsigned_lo, res_unsigned_hi);
                        const __m128i res_0   = _mm256_castsi256_si128(res_16b);
                        const __m128i res_1   = _mm256_extracti128_si256(res_16b, 1);

                        _mm_storeu_si128((__m128i*)(&dst[i * dst_stride + j]), res_0);
                        _mm_storeu_si128((__m128i*)(&dst[i * dst_stride + j + dst_stride]), res_1);
                    }
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
    if (j < w) {
        svt_av1_highbd_jnt_convolve_2d_c(src + src_stride * j,
                                         src_stride,
                                         dst0 + dst_stride0 * j,
                                         dst_stride0,
                                         w - j,
                                         h,
                                         filter_params_x,
                                         filter_params_y,
                                         subpel_x_q4,
                                         subpel_y_q4,
                                         conv_params,
                                         bd);
    }
}

void svt_av1_highbd_jnt_convolve_x_avx2(const uint16_t* src, int32_t src_stride, uint16_t* dst0, int32_t dst_stride0,
                                        int32_t w, int32_t h, const InterpFilterParams* filter_params_x,
                                        const InterpFilterParams* filter_params_y, const int32_t subpel_x_q4,
                                        const int32_t subpel_y_q4, ConvolveParams* conv_params, int32_t bd) {
    ConvBufType*          dst        = conv_params->dst;
    int32_t               dst_stride = conv_params->dst_stride;
    const int32_t         fo_horiz   = filter_params_x->taps / 2 - 1;
    const uint16_t* const src_ptr    = src - fo_horiz;
    const int32_t         bits       = FILTER_BITS - conv_params->round_1;
    (void)filter_params_y;
    (void)subpel_y_q4;

    int32_t i, j;
    __m256i s[4], coeffs_x[4];

    const int32_t do_average       = conv_params->do_average;
    const int32_t use_jnt_comp_avg = conv_params->use_jnt_comp_avg;
    const int32_t w0               = conv_params->fwd_offset;
    const int32_t w1               = conv_params->bck_offset;
    const __m256i wt0              = _mm256_set1_epi32(w0);
    const __m256i wt1              = _mm256_set1_epi32(w1);
    const __m256i zero             = _mm256_setzero_si256();

    const __m256i round_const_x    = _mm256_set1_epi32(((1 << conv_params->round_0) >> 1));
    const __m128i round_shift_x    = _mm_cvtsi32_si128(conv_params->round_0);
    const __m128i round_shift_bits = _mm_cvtsi32_si128(bits);

    const int32_t offset_0         = bd + 2 * FILTER_BITS - conv_params->round_0 - conv_params->round_1;
    const int32_t offset           = (1 << offset_0) + (1 << (offset_0 - 1));
    const __m256i offset_const     = _mm256_set1_epi32(offset);
    const int32_t rounding_shift   = 2 * FILTER_BITS - conv_params->round_0 - conv_params->round_1;
    const __m256i rounding_const   = _mm256_set1_epi32((1 << rounding_shift) >> 1);
    const __m256i clip_pixel_to_bd = _mm256_set1_epi16(bd == 10 ? 1023 : (bd == 12 ? 4095 : 255));

    assert(bits >= 0);
    prepare_coeffs_8tap_avx2(filter_params_x, subpel_x_q4, coeffs_x);

    for (j = 0; j < w - 2; j += 8) {
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

            res_even = _mm256_sll_epi32(res_even, round_shift_bits);
            res_odd  = _mm256_sll_epi32(res_odd, round_shift_bits);

            __m256i res1 = _mm256_unpacklo_epi32(res_even, res_odd);

            __m256i res_unsigned_lo = _mm256_add_epi32(res1, offset_const);

            if (w - j < 8) {
                if (do_average) {
                    const __m256i data_0 = _mm256_castsi128_si256(
                        _mm_loadl_epi64((__m128i*)(&dst[i * dst_stride + j])));
                    const __m256i data_1 = _mm256_castsi128_si256(
                        _mm_loadl_epi64((__m128i*)(&dst[i * dst_stride + j + dst_stride])));
                    const __m256i data_01 = _mm256_permute2x128_si256(data_0, data_1, 0x20);

                    const __m256i data_ref_0 = _mm256_unpacklo_epi16(data_01, zero);

                    const __m256i comp_avg_res = highbd_comp_avg(
                        &data_ref_0, &res_unsigned_lo, &wt0, &wt1, use_jnt_comp_avg);

                    const __m256i round_result = highbd_convolve_rounding(
                        &comp_avg_res, &offset_const, &rounding_const, rounding_shift);

                    const __m256i res_16b  = _mm256_packus_epi32(round_result, round_result);
                    const __m256i res_clip = _mm256_min_epi16(res_16b, clip_pixel_to_bd);

                    const __m128i res_0 = _mm256_castsi256_si128(res_clip);
                    const __m128i res_1 = _mm256_extracti128_si256(res_clip, 1);

                    _mm_storel_epi64((__m128i*)(&dst0[i * dst_stride0 + j]), res_0);
                    _mm_storel_epi64((__m128i*)(&dst0[i * dst_stride0 + j + dst_stride0]), res_1);
                } else {
                    __m256i       res_16b = _mm256_packus_epi32(res_unsigned_lo, res_unsigned_lo);
                    const __m128i res_0   = _mm256_castsi256_si128(res_16b);
                    const __m128i res_1   = _mm256_extracti128_si256(res_16b, 1);

                    _mm_storel_epi64((__m128i*)(&dst[i * dst_stride + j]), res_0);
                    _mm_storel_epi64((__m128i*)(&dst[i * dst_stride + j + dst_stride]), res_1);
                }
            } else {
                __m256i res2            = _mm256_unpackhi_epi32(res_even, res_odd);
                __m256i res_unsigned_hi = _mm256_add_epi32(res2, offset_const);

                if (do_average) {
                    const __m128i data_0  = _mm_loadu_si128((__m128i*)(&dst[i * dst_stride + j]));
                    const __m128i data_1  = _mm_loadu_si128((__m128i*)(&dst[i * dst_stride + j + dst_stride]));
                    const __m256i data_01 = yy_setr_m128i(data_0, data_1);

                    const __m256i data_ref_0_lo = _mm256_unpacklo_epi16(data_01, zero);
                    const __m256i data_ref_0_hi = _mm256_unpackhi_epi16(data_01, zero);

                    const __m256i comp_avg_res_lo = highbd_comp_avg(
                        &data_ref_0_lo, &res_unsigned_lo, &wt0, &wt1, use_jnt_comp_avg);
                    const __m256i comp_avg_res_hi = highbd_comp_avg(
                        &data_ref_0_hi, &res_unsigned_hi, &wt0, &wt1, use_jnt_comp_avg);

                    const __m256i round_result_lo = highbd_convolve_rounding(
                        &comp_avg_res_lo, &offset_const, &rounding_const, rounding_shift);
                    const __m256i round_result_hi = highbd_convolve_rounding(
                        &comp_avg_res_hi, &offset_const, &rounding_const, rounding_shift);

                    const __m256i res_16b  = _mm256_packus_epi32(round_result_lo, round_result_hi);
                    const __m256i res_clip = _mm256_min_epi16(res_16b, clip_pixel_to_bd);

                    const __m128i res_0 = _mm256_castsi256_si128(res_clip);
                    const __m128i res_1 = _mm256_extracti128_si256(res_clip, 1);

                    _mm_storeu_si128((__m128i*)(&dst0[i * dst_stride0 + j]), res_0);
                    _mm_storeu_si128((__m128i*)(&dst0[i * dst_stride0 + j + dst_stride0]), res_1);
                } else {
                    __m256i       res_16b = _mm256_packus_epi32(res_unsigned_lo, res_unsigned_hi);
                    const __m128i res_0   = _mm256_castsi256_si128(res_16b);
                    const __m128i res_1   = _mm256_extracti128_si256(res_16b, 1);

                    _mm_storeu_si128((__m128i*)(&dst[i * dst_stride + j]), res_0);
                    _mm_storeu_si128((__m128i*)(&dst[i * dst_stride + j + dst_stride]), res_1);
                }
            }
        }
    }
    if (j < w) {
        svt_av1_highbd_jnt_convolve_x_c(src + src_stride * j,
                                        src_stride,
                                        dst0 + dst_stride0 * j,
                                        dst_stride0,
                                        w - j,
                                        h,
                                        filter_params_x,
                                        filter_params_y,
                                        subpel_x_q4,
                                        subpel_y_q4,
                                        conv_params,
                                        bd);
    }
}

void svt_av1_highbd_jnt_convolve_y_avx2(const uint16_t* src, int32_t src_stride, uint16_t* dst0, int32_t dst_stride0,
                                        int32_t w, int32_t h, const InterpFilterParams* filter_params_x,
                                        const InterpFilterParams* filter_params_y, const int32_t subpel_x_q4,
                                        const int32_t subpel_y_q4, ConvolveParams* conv_params, int32_t bd) {
    ConvBufType*          dst        = conv_params->dst;
    int32_t               dst_stride = conv_params->dst_stride;
    const int32_t         fo_vert    = filter_params_y->taps / 2 - 1;
    const uint16_t* const src_ptr    = src - fo_vert * src_stride;
    const int32_t         bits       = FILTER_BITS - conv_params->round_0;
    (void)filter_params_x;
    (void)subpel_x_q4;

    assert(bits >= 0);
    int32_t       i, j;
    __m256i       s[8], coeffs_y[4];
    const int32_t do_average       = conv_params->do_average;
    const int32_t use_jnt_comp_avg = conv_params->use_jnt_comp_avg;

    const int32_t w0               = conv_params->fwd_offset;
    const int32_t w1               = conv_params->bck_offset;
    const __m256i wt0              = _mm256_set1_epi32(w0);
    const __m256i wt1              = _mm256_set1_epi32(w1);
    const __m256i round_const_y    = _mm256_set1_epi32(((1 << conv_params->round_1) >> 1));
    const __m128i round_shift_y    = _mm_cvtsi32_si128(conv_params->round_1);
    const __m128i round_shift_bits = _mm_cvtsi32_si128(bits);

    const int32_t offset_0         = bd + 2 * FILTER_BITS - conv_params->round_0 - conv_params->round_1;
    const int32_t offset           = (1 << offset_0) + (1 << (offset_0 - 1));
    const __m256i offset_const     = _mm256_set1_epi32(offset);
    const int32_t rounding_shift   = 2 * FILTER_BITS - conv_params->round_0 - conv_params->round_1;
    const __m256i rounding_const   = _mm256_set1_epi32((1 << rounding_shift) >> 1);
    const __m256i clip_pixel_to_bd = _mm256_set1_epi16(bd == 10 ? 1023 : (bd == 12 ? 4095 : 255));
    const __m256i zero             = _mm256_setzero_si256();

    prepare_coeffs_8tap_avx2(filter_params_y, subpel_y_q4, coeffs_y);

    for (j = 0; j < w - 2; j += 8) {
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

                __m256i res_a_round = _mm256_sll_epi32(res_a, round_shift_bits);
                res_a_round         = _mm256_sra_epi32(_mm256_add_epi32(res_a_round, round_const_y), round_shift_y);

                __m256i res_unsigned_lo = _mm256_add_epi32(res_a_round, offset_const);

                if (w - j < 8) {
                    if (do_average) {
                        const __m256i data_0 = _mm256_castsi128_si256(
                            _mm_loadl_epi64((__m128i*)(&dst[i * dst_stride + j])));
                        const __m256i data_1 = _mm256_castsi128_si256(
                            _mm_loadl_epi64((__m128i*)(&dst[i * dst_stride + j + dst_stride])));
                        const __m256i data_01 = _mm256_permute2x128_si256(data_0, data_1, 0x20);

                        const __m256i data_ref_0 = _mm256_unpacklo_epi16(data_01, zero);

                        const __m256i comp_avg_res = highbd_comp_avg(
                            &data_ref_0, &res_unsigned_lo, &wt0, &wt1, use_jnt_comp_avg);

                        const __m256i round_result = highbd_convolve_rounding(
                            &comp_avg_res, &offset_const, &rounding_const, rounding_shift);

                        const __m256i res_16b  = _mm256_packus_epi32(round_result, round_result);
                        const __m256i res_clip = _mm256_min_epi16(res_16b, clip_pixel_to_bd);

                        const __m128i res_0 = _mm256_castsi256_si128(res_clip);
                        const __m128i res_1 = _mm256_extracti128_si256(res_clip, 1);

                        _mm_storel_epi64((__m128i*)(&dst0[i * dst_stride0 + j]), res_0);
                        _mm_storel_epi64((__m128i*)(&dst0[i * dst_stride0 + j + dst_stride0]), res_1);
                    } else {
                        __m256i       res_16b = _mm256_packus_epi32(res_unsigned_lo, res_unsigned_lo);
                        const __m128i res_0   = _mm256_castsi256_si128(res_16b);
                        const __m128i res_1   = _mm256_extracti128_si256(res_16b, 1);

                        _mm_storel_epi64((__m128i*)(&dst[i * dst_stride + j]), res_0);
                        _mm_storel_epi64((__m128i*)(&dst[i * dst_stride + j + dst_stride]), res_1);
                    }
                } else {
                    const __m256i res_b       = convolve16_8tap_avx2(s + 4, coeffs_y);
                    __m256i       res_b_round = _mm256_sll_epi32(res_b, round_shift_bits);
                    res_b_round = _mm256_sra_epi32(_mm256_add_epi32(res_b_round, round_const_y), round_shift_y);

                    __m256i res_unsigned_hi = _mm256_add_epi32(res_b_round, offset_const);

                    if (do_average) {
                        const __m128i data_0  = _mm_loadu_si128((__m128i*)(&dst[i * dst_stride + j]));
                        const __m128i data_1  = _mm_loadu_si128((__m128i*)(&dst[i * dst_stride + j + dst_stride]));
                        const __m256i data_01 = yy_setr_m128i(data_0, data_1);

                        const __m256i data_ref_0_lo = _mm256_unpacklo_epi16(data_01, zero);
                        const __m256i data_ref_0_hi = _mm256_unpackhi_epi16(data_01, zero);

                        const __m256i comp_avg_res_lo = highbd_comp_avg(
                            &data_ref_0_lo, &res_unsigned_lo, &wt0, &wt1, use_jnt_comp_avg);
                        const __m256i comp_avg_res_hi = highbd_comp_avg(
                            &data_ref_0_hi, &res_unsigned_hi, &wt0, &wt1, use_jnt_comp_avg);

                        const __m256i round_result_lo = highbd_convolve_rounding(
                            &comp_avg_res_lo, &offset_const, &rounding_const, rounding_shift);
                        const __m256i round_result_hi = highbd_convolve_rounding(
                            &comp_avg_res_hi, &offset_const, &rounding_const, rounding_shift);

                        const __m256i res_16b  = _mm256_packus_epi32(round_result_lo, round_result_hi);
                        const __m256i res_clip = _mm256_min_epi16(res_16b, clip_pixel_to_bd);

                        const __m128i res_0 = _mm256_castsi256_si128(res_clip);
                        const __m128i res_1 = _mm256_extracti128_si256(res_clip, 1);

                        _mm_storeu_si128((__m128i*)(&dst0[i * dst_stride0 + j]), res_0);
                        _mm_storeu_si128((__m128i*)(&dst0[i * dst_stride0 + j + dst_stride0]), res_1);
                    } else {
                        __m256i       res_16b = _mm256_packus_epi32(res_unsigned_lo, res_unsigned_hi);
                        const __m128i res_0   = _mm256_castsi256_si128(res_16b);
                        const __m128i res_1   = _mm256_extracti128_si256(res_16b, 1);

                        _mm_storeu_si128((__m128i*)(&dst[i * dst_stride + j]), res_0);
                        _mm_storeu_si128((__m128i*)(&dst[i * dst_stride + j + dst_stride]), res_1);
                    }
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
    if (j < w) {
        svt_av1_highbd_jnt_convolve_y_c(src + src_stride * j,
                                        src_stride,
                                        dst0 + dst_stride0 * j,
                                        dst_stride0,
                                        w - j,
                                        h,
                                        filter_params_x,
                                        filter_params_y,
                                        subpel_x_q4,
                                        subpel_y_q4,
                                        conv_params,
                                        bd);
    }
}
