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

#include <smmintrin.h>
#include <assert.h>

#include "definitions.h"
#include "common_dsp_rtcd.h"
#include "convolve.h"

static INLINE __m128i svt_aom_convolve(const __m128i* const s, const __m128i* const coeffs) {
    const __m128i res_0 = _mm_madd_epi16(s[0], coeffs[0]);
    const __m128i res_1 = _mm_madd_epi16(s[1], coeffs[1]);
    const __m128i res_2 = _mm_madd_epi16(s[2], coeffs[2]);
    const __m128i res_3 = _mm_madd_epi16(s[3], coeffs[3]);

    const __m128i res = _mm_add_epi32(_mm_add_epi32(res_0, res_1), _mm_add_epi32(res_2, res_3));

    return res;
}

static INLINE __m128i highbd_convolve_rounding_sse2(const __m128i* const res_unsigned,
                                                    const __m128i* const offset_const, const __m128i* const round_const,
                                                    const int round_shift) {
    const __m128i res_signed = _mm_sub_epi32(*res_unsigned, *offset_const);
    const __m128i res_round  = _mm_srai_epi32(_mm_add_epi32(res_signed, *round_const), round_shift);

    return res_round;
}

static INLINE __m128i highbd_comp_avg_sse4_1(const __m128i* const data_ref_0, const __m128i* const res_unsigned,
                                             const __m128i* const wt0, const __m128i* const wt1,
                                             const int use_dist_wtd_avg) {
    __m128i res;
    if (use_dist_wtd_avg) {
        const __m128i wt0_res = _mm_mullo_epi32(*data_ref_0, *wt0);
        const __m128i wt1_res = _mm_mullo_epi32(*res_unsigned, *wt1);

        const __m128i wt_res = _mm_add_epi32(wt0_res, wt1_res);
        res                  = _mm_srai_epi32(wt_res, DIST_PRECISION_BITS);
    } else {
        const __m128i wt_res = _mm_add_epi32(*data_ref_0, *res_unsigned);
        res                  = _mm_srai_epi32(wt_res, 1);
    }
    return res;
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

void svt_av1_highbd_jnt_convolve_y_sse4_1(const uint16_t* src, int32_t src_stride, uint16_t* dst0, int32_t dst_stride0,
                                          int32_t w, int32_t h, const InterpFilterParams* filter_params_x,
                                          const InterpFilterParams* filter_params_y, const int32_t subpel_x_q4,
                                          const int32_t subpel_y_q4, ConvolveParams* conv_params, int32_t bd) {
    if (w <= 4) {
        svt_av1_highbd_jnt_convolve_y_c(src,
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
        return;
    }
    CONV_BUF_TYPE*        dst        = conv_params->dst;
    int                   dst_stride = conv_params->dst_stride;
    const int             fo_vert    = filter_params_y->taps / 2 - 1;
    const uint16_t* const src_ptr    = src - fo_vert * src_stride;
    const int             bits       = FILTER_BITS - conv_params->round_0;

    assert(bits >= 0);
    int       i, j;
    const int do_average       = conv_params->do_average;
    const int use_jnt_comp_avg = conv_params->use_jnt_comp_avg;

    const int     w0               = conv_params->fwd_offset;
    const int     w1               = conv_params->bck_offset;
    const __m128i wt0              = _mm_set1_epi32(w0);
    const __m128i wt1              = _mm_set1_epi32(w1);
    const __m128i round_const_y    = _mm_set1_epi32(((1 << conv_params->round_1) >> 1));
    const __m128i round_shift_y    = _mm_cvtsi32_si128(conv_params->round_1);
    const __m128i round_shift_bits = _mm_cvtsi32_si128(bits);

    const int     offset_0         = bd + 2 * FILTER_BITS - conv_params->round_0 - conv_params->round_1;
    const int     offset           = (1 << offset_0) + (1 << (offset_0 - 1));
    const __m128i offset_const     = _mm_set1_epi32(offset);
    const int     rounding_shift   = 2 * FILTER_BITS - conv_params->round_0 - conv_params->round_1;
    const __m128i rounding_const   = _mm_set1_epi32((1 << rounding_shift) >> 1);
    const __m128i clip_pixel_to_bd = _mm_set1_epi16(bd == 10 ? 1023 : (bd == 12 ? 4095 : 255));
    const __m128i zero             = _mm_setzero_si128();
    __m128i       s[16], coeffs_y[4];

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

                const __m128i res_a0       = svt_aom_convolve(s, coeffs_y);
                __m128i       res_a_round0 = _mm_sll_epi32(res_a0, round_shift_bits);
                res_a_round0               = _mm_sra_epi32(_mm_add_epi32(res_a_round0, round_const_y), round_shift_y);

                const __m128i res_a1       = svt_aom_convolve(s + 8, coeffs_y);
                __m128i       res_a_round1 = _mm_sll_epi32(res_a1, round_shift_bits);
                res_a_round1               = _mm_sra_epi32(_mm_add_epi32(res_a_round1, round_const_y), round_shift_y);

                __m128i res_unsigned_lo_0 = _mm_add_epi32(res_a_round0, offset_const);
                __m128i res_unsigned_lo_1 = _mm_add_epi32(res_a_round1, offset_const);

                if (w - j < 8) {
                    if (do_average) {
                        const __m128i data_0 = _mm_loadl_epi64((__m128i*)(&dst[i * dst_stride + j]));
                        const __m128i data_1 = _mm_loadl_epi64((__m128i*)(&dst[i * dst_stride + j + dst_stride]));

                        const __m128i data_ref_0 = _mm_unpacklo_epi16(data_0, zero);
                        const __m128i data_ref_1 = _mm_unpacklo_epi16(data_1, zero);

                        const __m128i comp_avg_res_0 = highbd_comp_avg_sse4_1(
                            &data_ref_0, &res_unsigned_lo_0, &wt0, &wt1, use_jnt_comp_avg);
                        const __m128i comp_avg_res_1 = highbd_comp_avg_sse4_1(
                            &data_ref_1, &res_unsigned_lo_1, &wt0, &wt1, use_jnt_comp_avg);

                        const __m128i round_result_0 = highbd_convolve_rounding_sse2(
                            &comp_avg_res_0, &offset_const, &rounding_const, rounding_shift);
                        const __m128i round_result_1 = highbd_convolve_rounding_sse2(
                            &comp_avg_res_1, &offset_const, &rounding_const, rounding_shift);

                        const __m128i res_16b_0  = _mm_packus_epi32(round_result_0, round_result_0);
                        const __m128i res_clip_0 = _mm_min_epi16(res_16b_0, clip_pixel_to_bd);
                        const __m128i res_16b_1  = _mm_packus_epi32(round_result_1, round_result_1);
                        const __m128i res_clip_1 = _mm_min_epi16(res_16b_1, clip_pixel_to_bd);

                        _mm_storel_epi64((__m128i*)(&dst0[i * dst_stride0 + j]), res_clip_0);
                        _mm_storel_epi64((__m128i*)(&dst0[i * dst_stride0 + j + dst_stride0]), res_clip_1);

                    } else {
                        __m128i res_16b_0 = _mm_packus_epi32(res_unsigned_lo_0, res_unsigned_lo_0);

                        __m128i res_16b_1 = _mm_packus_epi32(res_unsigned_lo_1, res_unsigned_lo_1);

                        _mm_storel_epi64((__m128i*)&dst[i * dst_stride + j], res_16b_0);
                        _mm_storel_epi64((__m128i*)&dst[i * dst_stride + j + dst_stride], res_16b_1);
                    }
                } else {
                    const __m128i res_b0       = svt_aom_convolve(s + 4, coeffs_y);
                    __m128i       res_b_round0 = _mm_sll_epi32(res_b0, round_shift_bits);
                    res_b_round0 = _mm_sra_epi32(_mm_add_epi32(res_b_round0, round_const_y), round_shift_y);

                    const __m128i res_b1       = svt_aom_convolve(s + 4 + 8, coeffs_y);
                    __m128i       res_b_round1 = _mm_sll_epi32(res_b1, round_shift_bits);
                    res_b_round1 = _mm_sra_epi32(_mm_add_epi32(res_b_round1, round_const_y), round_shift_y);

                    __m128i res_unsigned_hi_0 = _mm_add_epi32(res_b_round0, offset_const);
                    __m128i res_unsigned_hi_1 = _mm_add_epi32(res_b_round1, offset_const);

                    if (do_average) {
                        const __m128i data_0 = _mm_loadu_si128((__m128i*)(&dst[i * dst_stride + j]));
                        const __m128i data_1 = _mm_loadu_si128((__m128i*)(&dst[i * dst_stride + j + dst_stride]));
                        const __m128i data_ref_0_lo_0 = _mm_unpacklo_epi16(data_0, zero);
                        const __m128i data_ref_0_lo_1 = _mm_unpacklo_epi16(data_1, zero);

                        const __m128i data_ref_0_hi_0 = _mm_unpackhi_epi16(data_0, zero);
                        const __m128i data_ref_0_hi_1 = _mm_unpackhi_epi16(data_1, zero);

                        const __m128i comp_avg_res_lo_0 = highbd_comp_avg_sse4_1(
                            &data_ref_0_lo_0, &res_unsigned_lo_0, &wt0, &wt1, use_jnt_comp_avg);
                        const __m128i comp_avg_res_lo_1 = highbd_comp_avg_sse4_1(
                            &data_ref_0_lo_1, &res_unsigned_lo_1, &wt0, &wt1, use_jnt_comp_avg);
                        const __m128i comp_avg_res_hi_0 = highbd_comp_avg_sse4_1(
                            &data_ref_0_hi_0, &res_unsigned_hi_0, &wt0, &wt1, use_jnt_comp_avg);
                        const __m128i comp_avg_res_hi_1 = highbd_comp_avg_sse4_1(
                            &data_ref_0_hi_1, &res_unsigned_hi_1, &wt0, &wt1, use_jnt_comp_avg);

                        const __m128i round_result_lo_0 = highbd_convolve_rounding_sse2(
                            &comp_avg_res_lo_0, &offset_const, &rounding_const, rounding_shift);
                        const __m128i round_result_lo_1 = highbd_convolve_rounding_sse2(
                            &comp_avg_res_lo_1, &offset_const, &rounding_const, rounding_shift);
                        const __m128i round_result_hi_0 = highbd_convolve_rounding_sse2(
                            &comp_avg_res_hi_0, &offset_const, &rounding_const, rounding_shift);
                        const __m128i round_result_hi_1 = highbd_convolve_rounding_sse2(
                            &comp_avg_res_hi_1, &offset_const, &rounding_const, rounding_shift);

                        const __m128i res_16b_0  = _mm_packus_epi32(round_result_lo_0, round_result_hi_0);
                        const __m128i res_clip_0 = _mm_min_epi16(res_16b_0, clip_pixel_to_bd);

                        const __m128i res_16b_1  = _mm_packus_epi32(round_result_lo_1, round_result_hi_1);
                        const __m128i res_clip_1 = _mm_min_epi16(res_16b_1, clip_pixel_to_bd);

                        _mm_storeu_si128((__m128i*)(&dst0[i * dst_stride0 + j]), res_clip_0);
                        _mm_storeu_si128((__m128i*)(&dst0[i * dst_stride0 + j + dst_stride0]), res_clip_1);
                    } else {
                        __m128i res_16bit0 = _mm_packus_epi32(res_unsigned_lo_0, res_unsigned_hi_0);
                        __m128i res_16bit1 = _mm_packus_epi32(res_unsigned_lo_1, res_unsigned_hi_1);
                        _mm_storeu_si128((__m128i*)(&dst[i * dst_stride + j]), res_16bit0);
                        _mm_storeu_si128((__m128i*)(&dst[i * dst_stride + j + dst_stride]), res_16bit1);
                    }
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

void svt_av1_highbd_jnt_convolve_x_sse4_1(const uint16_t* src, int32_t src_stride, uint16_t* dst0, int32_t dst_stride0,
                                          int32_t w, int32_t h, const InterpFilterParams* filter_params_x,
                                          const InterpFilterParams* filter_params_y, const int32_t subpel_x_q4,
                                          const int32_t subpel_y_q4, ConvolveParams* conv_params, int32_t bd) {
    if (w <= 4) {
        svt_av1_highbd_jnt_convolve_x_c(src,
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
        return;
    }
    CONV_BUF_TYPE*        dst        = conv_params->dst;
    int                   dst_stride = conv_params->dst_stride;
    const int             fo_horiz   = filter_params_x->taps / 2 - 1;
    const uint16_t* const src_ptr    = src - fo_horiz;
    const int             bits       = FILTER_BITS - conv_params->round_1;

    int     i, j;
    __m128i s[4], coeffs_x[4];

    const int     do_average       = conv_params->do_average;
    const int     use_jnt_comp_avg = conv_params->use_jnt_comp_avg;
    const int     w0               = conv_params->fwd_offset;
    const int     w1               = conv_params->bck_offset;
    const __m128i wt0              = _mm_set1_epi32(w0);
    const __m128i wt1              = _mm_set1_epi32(w1);
    const __m128i zero             = _mm_setzero_si128();

    const __m128i round_const_x    = _mm_set1_epi32(((1 << conv_params->round_0) >> 1));
    const __m128i round_shift_x    = _mm_cvtsi32_si128(conv_params->round_0);
    const __m128i round_shift_bits = _mm_cvtsi32_si128(bits);

    const int     offset_0         = bd + 2 * FILTER_BITS - conv_params->round_0 - conv_params->round_1;
    const int     offset           = (1 << offset_0) + (1 << (offset_0 - 1));
    const __m128i offset_const     = _mm_set1_epi32(offset);
    const int     rounding_shift   = 2 * FILTER_BITS - conv_params->round_0 - conv_params->round_1;
    const __m128i rounding_const   = _mm_set1_epi32((1 << rounding_shift) >> 1);
    const __m128i clip_pixel_to_bd = _mm_set1_epi16(bd == 10 ? 1023 : (bd == 12 ? 4095 : 255));

    assert(bits >= 0);
    prepare_coeffs(filter_params_x, subpel_x_q4, coeffs_x);

    for (j = 0; j < w; j += 8) {
        /* Horizontal filter */
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

            res_even = _mm_sll_epi32(res_even, round_shift_bits);
            res_odd  = _mm_sll_epi32(res_odd, round_shift_bits);

            __m128i res1            = _mm_unpacklo_epi32(res_even, res_odd);
            __m128i res_unsigned_lo = _mm_add_epi32(res1, offset_const);
            if (w - j < 8) {
                if (do_average) {
                    const __m128i data_0     = _mm_loadl_epi64((__m128i*)(&dst[i * dst_stride + j]));
                    const __m128i data_ref_0 = _mm_unpacklo_epi16(data_0, zero);

                    const __m128i comp_avg_res = highbd_comp_avg_sse4_1(
                        &data_ref_0, &res_unsigned_lo, &wt0, &wt1, use_jnt_comp_avg);
                    const __m128i round_result = highbd_convolve_rounding_sse2(
                        &comp_avg_res, &offset_const, &rounding_const, rounding_shift);

                    const __m128i res_16b  = _mm_packus_epi32(round_result, round_result);
                    const __m128i res_clip = _mm_min_epi16(res_16b, clip_pixel_to_bd);
                    _mm_storel_epi64((__m128i*)(&dst0[i * dst_stride0 + j]), res_clip);
                } else {
                    __m128i res_16b = _mm_packus_epi32(res_unsigned_lo, res_unsigned_lo);
                    _mm_storel_epi64((__m128i*)&dst[i * dst_stride + j], res_16b);
                }
            } else {
                __m128i res2            = _mm_unpackhi_epi32(res_even, res_odd);
                __m128i res_unsigned_hi = _mm_add_epi32(res2, offset_const);
                if (do_average) {
                    const __m128i data_0        = _mm_loadu_si128((__m128i*)(&dst[i * dst_stride + j]));
                    const __m128i data_ref_0_lo = _mm_unpacklo_epi16(data_0, zero);
                    const __m128i data_ref_0_hi = _mm_unpackhi_epi16(data_0, zero);

                    const __m128i comp_avg_res_lo = highbd_comp_avg_sse4_1(
                        &data_ref_0_lo, &res_unsigned_lo, &wt0, &wt1, use_jnt_comp_avg);
                    const __m128i comp_avg_res_hi = highbd_comp_avg_sse4_1(
                        &data_ref_0_hi, &res_unsigned_hi, &wt0, &wt1, use_jnt_comp_avg);

                    const __m128i round_result_lo = highbd_convolve_rounding_sse2(
                        &comp_avg_res_lo, &offset_const, &rounding_const, rounding_shift);
                    const __m128i round_result_hi = highbd_convolve_rounding_sse2(
                        &comp_avg_res_hi, &offset_const, &rounding_const, rounding_shift);

                    const __m128i res_16b  = _mm_packus_epi32(round_result_lo, round_result_hi);
                    const __m128i res_clip = _mm_min_epi16(res_16b, clip_pixel_to_bd);
                    _mm_storeu_si128((__m128i*)(&dst0[i * dst_stride0 + j]), res_clip);
                } else {
                    __m128i res_16b = _mm_packus_epi32(res_unsigned_lo, res_unsigned_hi);
                    _mm_storeu_si128((__m128i*)(&dst[i * dst_stride + j]), res_16b);
                }
            }
        }
    }
}
