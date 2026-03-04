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

#include <emmintrin.h>
#include "definitions.h"
#include "common_dsp_rtcd.h"
#include "filter.h"

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

static INLINE __m128i svt_aom_convolve(const __m128i* const s, const __m128i* const coeffs) {
    const __m128i res_0 = _mm_madd_epi16(s[0], coeffs[0]);
    const __m128i res_1 = _mm_madd_epi16(s[1], coeffs[1]);
    const __m128i res_2 = _mm_madd_epi16(s[2], coeffs[2]);
    const __m128i res_3 = _mm_madd_epi16(s[3], coeffs[3]);

    const __m128i res = _mm_add_epi32(_mm_add_epi32(res_0, res_1), _mm_add_epi32(res_2, res_3));

    return res;
}

static INLINE __m128i convolve_lo_x(const __m128i* const s, const __m128i* const coeffs) {
    __m128i ss[4];
    ss[0] = _mm_unpacklo_epi8(s[0], _mm_setzero_si128());
    ss[1] = _mm_unpacklo_epi8(s[1], _mm_setzero_si128());
    ss[2] = _mm_unpacklo_epi8(s[2], _mm_setzero_si128());
    ss[3] = _mm_unpacklo_epi8(s[3], _mm_setzero_si128());
    return svt_aom_convolve(ss, coeffs);
}

static INLINE __m128i convolve_lo_y(const __m128i* const s, const __m128i* const coeffs) {
    __m128i ss[4];
    ss[0] = _mm_unpacklo_epi8(s[0], _mm_setzero_si128());
    ss[1] = _mm_unpacklo_epi8(s[2], _mm_setzero_si128());
    ss[2] = _mm_unpacklo_epi8(s[4], _mm_setzero_si128());
    ss[3] = _mm_unpacklo_epi8(s[6], _mm_setzero_si128());
    return svt_aom_convolve(ss, coeffs);
}

static INLINE __m128i convolve_hi_y(const __m128i* const s, const __m128i* const coeffs) {
    __m128i ss[4];
    ss[0] = _mm_unpackhi_epi8(s[0], _mm_setzero_si128());
    ss[1] = _mm_unpackhi_epi8(s[2], _mm_setzero_si128());
    ss[2] = _mm_unpackhi_epi8(s[4], _mm_setzero_si128());
    ss[3] = _mm_unpackhi_epi8(s[6], _mm_setzero_si128());
    return svt_aom_convolve(ss, coeffs);
}

static INLINE __m128i comp_avg(const __m128i* const data_ref_0, const __m128i* const res_unsigned,
                               const __m128i* const wt, const int use_dist_wtd_avg) {
    __m128i res;
    if (use_dist_wtd_avg) {
        const __m128i data_lo = _mm_unpacklo_epi16(*data_ref_0, *res_unsigned);
        const __m128i data_hi = _mm_unpackhi_epi16(*data_ref_0, *res_unsigned);

        const __m128i wt_res_lo = _mm_madd_epi16(data_lo, *wt);
        const __m128i wt_res_hi = _mm_madd_epi16(data_hi, *wt);

        const __m128i res_lo = _mm_srai_epi32(wt_res_lo, DIST_PRECISION_BITS);
        const __m128i res_hi = _mm_srai_epi32(wt_res_hi, DIST_PRECISION_BITS);

        res = _mm_packs_epi32(res_lo, res_hi);
    } else {
        const __m128i wt_res = _mm_add_epi16(*data_ref_0, *res_unsigned);
        res                  = _mm_srai_epi16(wt_res, 1);
    }
    return res;
}

static INLINE __m128i convolve_rounding(const __m128i* const res_unsigned, const __m128i* const offset_const,
                                        const __m128i* const round_const, const int round_shift) {
    const __m128i res_signed = _mm_sub_epi16(*res_unsigned, *offset_const);
    const __m128i res_round  = _mm_srai_epi16(_mm_add_epi16(res_signed, *round_const), round_shift);
    return res_round;
}

void svt_av1_jnt_convolve_x_sse2(const uint8_t* src, int32_t src_stride, uint8_t* dst8, int32_t dst8_stride, int32_t w,
                                 int32_t h, const InterpFilterParams* filter_params_x,
                                 const InterpFilterParams* filter_params_y, const int32_t subpel_x_q4,
                                 const int32_t subpel_y_q4, ConvolveParams* conv_params) {
    const int      bd               = 8;
    CONV_BUF_TYPE* dst              = conv_params->dst;
    const int      dst_stride       = conv_params->dst_stride;
    const int      fo_horiz         = filter_params_x->taps / 2 - 1;
    const uint8_t* src_ptr          = src - fo_horiz;
    const int      bits             = FILTER_BITS - conv_params->round_1;
    const __m128i  left_shift       = _mm_cvtsi32_si128(bits);
    const __m128i  round_const      = _mm_set1_epi32((1 << conv_params->round_0) >> 1);
    const __m128i  round_shift      = _mm_cvtsi32_si128(conv_params->round_0);
    const int      w0               = conv_params->fwd_offset;
    const int      w1               = conv_params->bck_offset;
    const __m128i  wt0              = _mm_set1_epi16(w0);
    const __m128i  wt1              = _mm_set1_epi16(w1);
    const __m128i  wt               = _mm_unpacklo_epi16(wt0, wt1);
    const int      do_average       = conv_params->do_average;
    const int      use_jnt_comp_avg = conv_params->use_jnt_comp_avg;
    const int      offset_0         = bd + 2 * FILTER_BITS - conv_params->round_0 - conv_params->round_1;
    const int      offset           = (1 << offset_0) + (1 << (offset_0 - 1));
    const __m128i  offset_const     = _mm_set1_epi16(offset);
    const int      rounding_shift   = 2 * FILTER_BITS - conv_params->round_0 - conv_params->round_1;
    const __m128i  rounding_const   = _mm_set1_epi16((1 << rounding_shift) >> 1);
    __m128i        coeffs[4];

    prepare_coeffs(filter_params_x, subpel_x_q4, coeffs);
    if (w < 4) {
        svt_av1_jnt_convolve_x_c(src,
                                 src_stride,
                                 dst8,
                                 dst8_stride,
                                 w,
                                 h,
                                 filter_params_x,
                                 filter_params_y,
                                 subpel_x_q4,
                                 subpel_y_q4,
                                 conv_params);
    } else if (w == 4) {
        do {
            const __m128i data = _mm_loadu_si128((__m128i*)src_ptr);
            __m128i       s[4];

            s[0]                       = _mm_unpacklo_epi8(data, _mm_srli_si128(data, 1));
            s[1]                       = _mm_unpacklo_epi8(_mm_srli_si128(data, 2), _mm_srli_si128(data, 3));
            s[2]                       = _mm_unpacklo_epi8(_mm_srli_si128(data, 4), _mm_srli_si128(data, 5));
            s[3]                       = _mm_unpacklo_epi8(_mm_srli_si128(data, 6), _mm_srli_si128(data, 7));
            const __m128i res_lo       = convolve_lo_x(s, coeffs);
            const __m128i res_lo_round = _mm_sra_epi32(_mm_add_epi32(res_lo, round_const), round_shift);
            const __m128i res_lo_shift = _mm_sll_epi32(res_lo_round, left_shift);

            const __m128i res_16b      = _mm_packs_epi32(res_lo_shift, res_lo_shift);
            const __m128i res_unsigned = _mm_add_epi16(res_16b, offset_const);

            // Accumulate values into the destination buffer
            if (do_average) {
                const __m128i data_ref_0 = _mm_loadu_si128((__m128i*)dst);

                const __m128i comp_avg_res = comp_avg(&data_ref_0, &res_unsigned, &wt, use_jnt_comp_avg);

                const __m128i round_result = convolve_rounding(
                    &comp_avg_res, &offset_const, &rounding_const, rounding_shift);

                const __m128i res_8    = _mm_packus_epi16(round_result, round_result);
                *(uint32_t*)(&dst8[0]) = _mm_cvtsi128_si32(res_8);
            } else {
                _mm_storel_epi64((__m128i*)(&dst[0]), res_unsigned);
            }
            src_ptr += src_stride;
            dst += dst_stride;
            dst8 += dst8_stride;
        } while (--h);
    } else {
        assert(!(w % 8));
        int i = 0;
        do {
            int j = 0;
            do {
                const __m128i data = _mm_loadu_si128((__m128i*)&src_ptr[i * src_stride + j]);
                __m128i       s[4];

                // Filter even-index pixels
                s[0]                   = data;
                s[1]                   = _mm_srli_si128(data, 2);
                s[2]                   = _mm_srli_si128(data, 4);
                s[3]                   = _mm_srli_si128(data, 6);
                const __m128i res_even = convolve_lo_x(s, coeffs);

                // Filter odd-index pixels
                s[0]                  = _mm_srli_si128(data, 1);
                s[1]                  = _mm_srli_si128(data, 3);
                s[2]                  = _mm_srli_si128(data, 5);
                s[3]                  = _mm_srli_si128(data, 7);
                const __m128i res_odd = convolve_lo_x(s, coeffs);

                // Rearrange pixels back into the order 0 ... 7
                const __m128i res_lo       = _mm_unpacklo_epi32(res_even, res_odd);
                const __m128i res_hi       = _mm_unpackhi_epi32(res_even, res_odd);
                const __m128i res_lo_round = _mm_sra_epi32(_mm_add_epi32(res_lo, round_const), round_shift);
                const __m128i res_hi_round = _mm_sra_epi32(_mm_add_epi32(res_hi, round_const), round_shift);
                const __m128i res_lo_shift = _mm_sll_epi32(res_lo_round, left_shift);
                const __m128i res_hi_shift = _mm_sll_epi32(res_hi_round, left_shift);

                const __m128i res_16b      = _mm_packs_epi32(res_lo_shift, res_hi_shift);
                const __m128i res_unsigned = _mm_add_epi16(res_16b, offset_const);

                // Accumulate values into the destination buffer
                if (do_average) {
                    const __m128i data_ref_0 = _mm_loadu_si128((__m128i*)(&dst[i * dst_stride + j]));

                    const __m128i comp_avg_res = comp_avg(&data_ref_0, &res_unsigned, &wt, use_jnt_comp_avg);

                    const __m128i round_result = convolve_rounding(
                        &comp_avg_res, &offset_const, &rounding_const, rounding_shift);

                    const __m128i res_8 = _mm_packus_epi16(round_result, round_result);
                    _mm_storel_epi64((__m128i*)(&dst8[i * dst8_stride + j]), res_8);
                } else {
                    _mm_storeu_si128((__m128i*)(&dst[i * dst_stride + j]), res_unsigned);
                }
                j += 8;
            } while (j < w);
        } while (++i < h);
    }
}

void svt_av1_jnt_convolve_y_sse2(const uint8_t* src, int32_t src_stride, uint8_t* dst8, int32_t dst8_stride, int32_t w,
                                 int32_t h, const InterpFilterParams* filter_params_x,
                                 const InterpFilterParams* filter_params_y, const int32_t subpel_x_q4,
                                 const int32_t subpel_y_q4, ConvolveParams* conv_params) {
    const int      bd               = 8;
    CONV_BUF_TYPE* dst              = conv_params->dst;
    const int      dst_stride       = conv_params->dst_stride;
    const int      fo_vert          = filter_params_y->taps / 2 - 1;
    const uint8_t* src_ptr          = src - fo_vert * src_stride;
    const int      bits             = FILTER_BITS - conv_params->round_0;
    const __m128i  left_shift       = _mm_cvtsi32_si128(bits);
    const __m128i  wt0              = _mm_set1_epi16(conv_params->fwd_offset);
    const __m128i  wt1              = _mm_set1_epi16(conv_params->bck_offset);
    const __m128i  wt               = _mm_unpacklo_epi16(wt0, wt1);
    const int      do_average       = conv_params->do_average;
    const int      use_jnt_comp_avg = conv_params->use_jnt_comp_avg;
    const int      offset_0         = bd + 2 * FILTER_BITS - conv_params->round_0 - conv_params->round_1;
    const int      offset           = (1 << offset_0) + (1 << (offset_0 - 1));
    const __m128i  offset_const     = _mm_set1_epi16(offset);
    const int      rounding_shift   = 2 * FILTER_BITS - conv_params->round_0 - conv_params->round_1;
    const __m128i  rounding_const   = _mm_set1_epi16((1 << rounding_shift) >> 1);
    const __m128i  round_const      = _mm_set1_epi32((1 << conv_params->round_1) >> 1);
    const __m128i  round_shift      = _mm_cvtsi32_si128(conv_params->round_1);
    __m128i        coeffs[4];

    prepare_coeffs(filter_params_y, subpel_y_q4, coeffs);
    if (w < 4) {
        svt_av1_jnt_convolve_y_c(src,
                                 src_stride,
                                 dst8,
                                 dst8_stride,
                                 w,
                                 h,
                                 filter_params_x,
                                 filter_params_y,
                                 subpel_x_q4,
                                 subpel_y_q4,
                                 conv_params);
    } else if (w == 4) {
        __m128i s[8], src6, res, res_shift;
        src6 = _mm_cvtsi32_si128(*(uint32_t*)(src_ptr + 6 * src_stride));
        s[0] = _mm_unpacklo_epi8(_mm_cvtsi32_si128(*(uint32_t*)(src_ptr + 0 * src_stride)),
                                 _mm_cvtsi32_si128(*(uint32_t*)(src_ptr + 1 * src_stride)));
        s[1] = _mm_unpacklo_epi8(_mm_cvtsi32_si128(*(uint32_t*)(src_ptr + 1 * src_stride)),
                                 _mm_cvtsi32_si128(*(uint32_t*)(src_ptr + 2 * src_stride)));
        s[2] = _mm_unpacklo_epi8(_mm_cvtsi32_si128(*(uint32_t*)(src_ptr + 2 * src_stride)),
                                 _mm_cvtsi32_si128(*(uint32_t*)(src_ptr + 3 * src_stride)));
        s[3] = _mm_unpacklo_epi8(_mm_cvtsi32_si128(*(uint32_t*)(src_ptr + 3 * src_stride)),
                                 _mm_cvtsi32_si128(*(uint32_t*)(src_ptr + 4 * src_stride)));
        s[4] = _mm_unpacklo_epi8(_mm_cvtsi32_si128(*(uint32_t*)(src_ptr + 4 * src_stride)),
                                 _mm_cvtsi32_si128(*(uint32_t*)(src_ptr + 5 * src_stride)));
        s[5] = _mm_unpacklo_epi8(_mm_cvtsi32_si128(*(uint32_t*)(src_ptr + 5 * src_stride)), src6);

        do {
            s[6] = _mm_unpacklo_epi8(src6, _mm_cvtsi32_si128(*(uint32_t*)(src_ptr + 7 * src_stride)));
            src6 = _mm_cvtsi32_si128(*(uint32_t*)(src_ptr + 8 * src_stride));
            s[7] = _mm_unpacklo_epi8(_mm_cvtsi32_si128(*(uint32_t*)(src_ptr + 7 * src_stride)), src6);

            res       = convolve_lo_y(s + 0, coeffs);
            res_shift = _mm_sll_epi32(res, left_shift);
            res_shift = _mm_sra_epi32(_mm_add_epi32(res_shift, round_const), round_shift);

            __m128i res_16b      = _mm_packs_epi32(res_shift, res_shift);
            __m128i res_unsigned = _mm_add_epi16(res_16b, offset_const);

            // Accumulate values into the destination buffer
            if (do_average) {
                const __m128i data_ref_0 = _mm_loadu_si128((__m128i*)dst);

                const __m128i comp_avg_res = comp_avg(&data_ref_0, &res_unsigned, &wt, use_jnt_comp_avg);

                const __m128i round_result = convolve_rounding(
                    &comp_avg_res, &offset_const, &rounding_const, rounding_shift);

                const __m128i res_8    = _mm_packus_epi16(round_result, round_result);
                *(uint32_t*)(&dst8[0]) = _mm_cvtsi128_si32(res_8);

            } else {
                _mm_storel_epi64((__m128i*)dst, res_unsigned);
            }

            src_ptr += src_stride;
            dst += dst_stride;
            dst8 += dst8_stride;

            res       = convolve_lo_y(s + 1, coeffs);
            res_shift = _mm_sll_epi32(res, left_shift);
            res_shift = _mm_sra_epi32(_mm_add_epi32(res_shift, round_const), round_shift);

            res_16b      = _mm_packs_epi32(res_shift, res_shift);
            res_unsigned = _mm_add_epi16(res_16b, offset_const);

            // Accumulate values into the destination buffer
            if (do_average) {
                const __m128i data_ref_0 = _mm_loadu_si128((__m128i*)dst);

                const __m128i comp_avg_res = comp_avg(&data_ref_0, &res_unsigned, &wt, use_jnt_comp_avg);

                const __m128i round_result = convolve_rounding(
                    &comp_avg_res, &offset_const, &rounding_const, rounding_shift);

                const __m128i res_8    = _mm_packus_epi16(round_result, round_result);
                *(uint32_t*)(&dst8[0]) = _mm_cvtsi128_si32(res_8);

            } else {
                _mm_storel_epi64((__m128i*)dst, res_unsigned);
            }

            src_ptr += src_stride;
            dst += dst_stride;
            dst8 += dst8_stride;

            s[0] = s[2];
            s[1] = s[3];
            s[2] = s[4];
            s[3] = s[5];
            s[4] = s[6];
            s[5] = s[7];
            h -= 2;
        } while (h);
    } else {
        assert(!(w % 8));
        int j = 0;
        do {
            __m128i        s[8], src6, res_lo, res_hi, res_lo_shift, res_hi_shift;
            const uint8_t* data = &src_ptr[j];

            src6 = _mm_loadl_epi64((__m128i*)(data + 6 * src_stride));
            s[0] = _mm_unpacklo_epi8(_mm_loadl_epi64((__m128i*)(data + 0 * src_stride)),
                                     _mm_loadl_epi64((__m128i*)(data + 1 * src_stride)));
            s[1] = _mm_unpacklo_epi8(_mm_loadl_epi64((__m128i*)(data + 1 * src_stride)),
                                     _mm_loadl_epi64((__m128i*)(data + 2 * src_stride)));
            s[2] = _mm_unpacklo_epi8(_mm_loadl_epi64((__m128i*)(data + 2 * src_stride)),
                                     _mm_loadl_epi64((__m128i*)(data + 3 * src_stride)));
            s[3] = _mm_unpacklo_epi8(_mm_loadl_epi64((__m128i*)(data + 3 * src_stride)),
                                     _mm_loadl_epi64((__m128i*)(data + 4 * src_stride)));
            s[4] = _mm_unpacklo_epi8(_mm_loadl_epi64((__m128i*)(data + 4 * src_stride)),
                                     _mm_loadl_epi64((__m128i*)(data + 5 * src_stride)));
            s[5] = _mm_unpacklo_epi8(_mm_loadl_epi64((__m128i*)(data + 5 * src_stride)), src6);

            int i = 0;
            do {
                data = &src_ptr[i * src_stride + j];
                s[6] = _mm_unpacklo_epi8(src6, _mm_loadl_epi64((__m128i*)(data + 7 * src_stride)));
                src6 = _mm_loadl_epi64((__m128i*)(data + 8 * src_stride));
                s[7] = _mm_unpacklo_epi8(_mm_loadl_epi64((__m128i*)(data + 7 * src_stride)), src6);

                res_lo       = convolve_lo_y(s, coeffs); // Filter low index pixels
                res_hi       = convolve_hi_y(s, coeffs); // Filter high index pixels
                res_lo_shift = _mm_sll_epi32(res_lo, left_shift);
                res_hi_shift = _mm_sll_epi32(res_hi, left_shift);
                res_lo_shift = _mm_sra_epi32(_mm_add_epi32(res_lo_shift, round_const), round_shift);
                res_hi_shift = _mm_sra_epi32(_mm_add_epi32(res_hi_shift, round_const), round_shift);

                __m128i res_16b      = _mm_packs_epi32(res_lo_shift, res_hi_shift);
                __m128i res_unsigned = _mm_add_epi16(res_16b, offset_const);

                // Accumulate values into the destination buffer
                if (do_average) {
                    const __m128i data_ref_0 = _mm_loadu_si128((__m128i*)(&dst[i * dst_stride + j]));

                    const __m128i comp_avg_res = comp_avg(&data_ref_0, &res_unsigned, &wt, use_jnt_comp_avg);

                    const __m128i round_result = convolve_rounding(
                        &comp_avg_res, &offset_const, &rounding_const, rounding_shift);

                    const __m128i res_8 = _mm_packus_epi16(round_result, round_result);
                    _mm_storel_epi64((__m128i*)(&dst8[i * dst8_stride + j]), res_8);
                } else {
                    _mm_storeu_si128((__m128i*)(&dst[i * dst_stride + j]), res_unsigned);
                }
                i++;

                res_lo       = convolve_lo_y(s + 1, coeffs); // Filter low index pixels
                res_hi       = convolve_hi_y(s + 1, coeffs); // Filter high index pixels
                res_lo_shift = _mm_sll_epi32(res_lo, left_shift);
                res_hi_shift = _mm_sll_epi32(res_hi, left_shift);
                res_lo_shift = _mm_sra_epi32(_mm_add_epi32(res_lo_shift, round_const), round_shift);
                res_hi_shift = _mm_sra_epi32(_mm_add_epi32(res_hi_shift, round_const), round_shift);
                res_16b      = _mm_packs_epi32(res_lo_shift, res_hi_shift);
                res_unsigned = _mm_add_epi16(res_16b, offset_const);

                // Accumulate values into the destination buffer
                if (do_average) {
                    __m128i data_ref_0 = _mm_loadu_si128((__m128i*)(&dst[i * dst_stride + j]));

                    const __m128i comp_avg_res = comp_avg(&data_ref_0, &res_unsigned, &wt, use_jnt_comp_avg);

                    const __m128i round_result = convolve_rounding(
                        &comp_avg_res, &offset_const, &rounding_const, rounding_shift);

                    const __m128i res_8 = _mm_packus_epi16(round_result, round_result);
                    _mm_storel_epi64((__m128i*)(&dst8[i * dst8_stride + j]), res_8);
                } else {
                    _mm_storeu_si128((__m128i*)(&dst[i * dst_stride + j]), res_unsigned);
                }
                i++;

                s[0] = s[2];
                s[1] = s[3];
                s[2] = s[4];
                s[3] = s[5];
                s[4] = s[6];
                s[5] = s[7];
            } while (i < h);
            j += 8;
        } while (j < w);
    }
}
