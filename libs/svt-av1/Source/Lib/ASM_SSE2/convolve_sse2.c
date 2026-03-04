/*
 * Copyright (c) 2017, Alliance for Open Media. All rights reserved
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
#include "synonyms.h"

static INLINE void svt_prepare_coeffs(const InterpFilterParams* const filter_params, const int subpel_q4,
                                      __m128i* const coeffs /* [4] */) {
    const int16_t* const y_filter = av1_get_interp_filter_subpel_kernel(*filter_params, subpel_q4 & SUBPEL_MASK);
    const __m128i        coeffs_y = _mm_loadu_si128((__m128i*)y_filter);
    // coeffs 0 1 0 1 2 3 2 3
    const __m128i tmp_0 = _mm_unpacklo_epi32(coeffs_y, coeffs_y);
    // coeffs 4 5 4 5 6 7 6 7
    const __m128i tmp_1 = _mm_unpackhi_epi32(coeffs_y, coeffs_y);

    coeffs[0] = _mm_unpacklo_epi64(tmp_0, tmp_0); // coeffs 0 1 0 1 0 1 0 1
    coeffs[1] = _mm_unpackhi_epi64(tmp_0, tmp_0); // coeffs 2 3 2 3 2 3 2 3
    coeffs[2] = _mm_unpacklo_epi64(tmp_1, tmp_1); // coeffs 4 5 4 5 4 5 4 5
    coeffs[3] = _mm_unpackhi_epi64(tmp_1, tmp_1); // coeffs 6 7 6 7 6 7 6 7
}

static INLINE __m128i svt_aom_convolve(const __m128i* const s, const __m128i* const coeffs) {
    const __m128i d0 = _mm_madd_epi16(s[0], coeffs[0]);
    const __m128i d1 = _mm_madd_epi16(s[1], coeffs[1]);
    const __m128i d2 = _mm_madd_epi16(s[2], coeffs[2]);
    const __m128i d3 = _mm_madd_epi16(s[3], coeffs[3]);
    const __m128i d  = _mm_add_epi32(_mm_add_epi32(d0, d1), _mm_add_epi32(d2, d3));
    return d;
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

static INLINE __m128i convolve_lo_x_12tap(const __m128i* s, const __m128i* coeffs, const __m128i zero) {
    __m128i ss[6];
    ss[0] = _mm_unpacklo_epi8(s[0], zero); //  0  1  1  2  2  3  3  4
    ss[1] = _mm_unpacklo_epi8(s[1], zero); //  2  3  3  4  4  5  5  6
    ss[2] = _mm_unpacklo_epi8(s[2], zero); //  4  5  5  6  6  7  7  8
    ss[3] = _mm_unpacklo_epi8(s[3], zero); //  6  7  7  8  8  9  9 10
    ss[4] = _mm_unpackhi_epi8(s[2], zero); //  8  9  9 10 10 11 11 12
    ss[5] = _mm_unpackhi_epi8(s[3], zero); // 10 11 11 12 12 13 13 14
    return convolve_12tap(ss, coeffs);
}

static INLINE __m128i convolve_lo_y_12tap(const __m128i* s, const __m128i* coeffs) {
    __m128i       ss[6];
    const __m128i zero = _mm_setzero_si128();
    ss[0]              = _mm_unpacklo_epi8(s[0], zero);
    ss[1]              = _mm_unpacklo_epi8(s[2], zero);
    ss[2]              = _mm_unpacklo_epi8(s[4], zero);
    ss[3]              = _mm_unpacklo_epi8(s[6], zero);
    ss[4]              = _mm_unpacklo_epi8(s[8], zero);
    ss[5]              = _mm_unpacklo_epi8(s[10], zero);
    return convolve_12tap(ss, coeffs);
}

static INLINE __m128i convolve_hi_y_12tap(const __m128i* s, const __m128i* coeffs) {
    __m128i       ss[6];
    const __m128i zero = _mm_setzero_si128();
    ss[0]              = _mm_unpackhi_epi8(s[0], zero);
    ss[1]              = _mm_unpackhi_epi8(s[2], zero);
    ss[2]              = _mm_unpackhi_epi8(s[4], zero);
    ss[3]              = _mm_unpackhi_epi8(s[6], zero);
    ss[4]              = _mm_unpackhi_epi8(s[8], zero);
    ss[5]              = _mm_unpackhi_epi8(s[10], zero);
    return convolve_12tap(ss, coeffs);
}

static INLINE void prepare_coeffs_12tap(const InterpFilterParams* filter_params, int subpel_q4,
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

void svt_av1_convolve_y_sr_12tap_sse2(const uint8_t* src, int src_stride, uint8_t* dst, int dst_stride, int w, int h,
                                      const InterpFilterParams* filter_params_y, int subpel_y_qn) {
    const int      fo_vert     = filter_params_y->taps / 2 - 1;
    const uint8_t* src_ptr     = src - fo_vert * src_stride;
    const __m128i  round_const = _mm_set1_epi32((1 << FILTER_BITS) >> 1);
    const __m128i  round_shift = _mm_cvtsi32_si128(FILTER_BITS);
    __m128i        coeffs[6];

    prepare_coeffs_12tap(filter_params_y, subpel_y_qn, coeffs);

    int j = 0;
    do {
        __m128i        s[12], src10, res_lo, res_hi;
        __m128i        res_lo_round, res_hi_round, res16, res;
        const uint8_t* data = &src_ptr[j];

        src10 = _mm_loadl_epi64((__m128i*)(data + 10 * src_stride));
        s[0]  = _mm_unpacklo_epi8(_mm_loadl_epi64((__m128i*)(data + 0 * src_stride)),
                                 _mm_loadl_epi64((__m128i*)(data + 1 * src_stride)));
        s[1]  = _mm_unpacklo_epi8(_mm_loadl_epi64((__m128i*)(data + 1 * src_stride)),
                                 _mm_loadl_epi64((__m128i*)(data + 2 * src_stride)));
        s[2]  = _mm_unpacklo_epi8(_mm_loadl_epi64((__m128i*)(data + 2 * src_stride)),
                                 _mm_loadl_epi64((__m128i*)(data + 3 * src_stride)));
        s[3]  = _mm_unpacklo_epi8(_mm_loadl_epi64((__m128i*)(data + 3 * src_stride)),
                                 _mm_loadl_epi64((__m128i*)(data + 4 * src_stride)));
        s[4]  = _mm_unpacklo_epi8(_mm_loadl_epi64((__m128i*)(data + 4 * src_stride)),
                                 _mm_loadl_epi64((__m128i*)(data + 5 * src_stride)));
        s[5]  = _mm_unpacklo_epi8(_mm_loadl_epi64((__m128i*)(data + 5 * src_stride)),
                                 _mm_loadl_epi64((__m128i*)(data + 6 * src_stride)));
        s[6]  = _mm_unpacklo_epi8(_mm_loadl_epi64((__m128i*)(data + 6 * src_stride)),
                                 _mm_loadl_epi64((__m128i*)(data + 7 * src_stride)));
        s[7]  = _mm_unpacklo_epi8(_mm_loadl_epi64((__m128i*)(data + 7 * src_stride)),
                                 _mm_loadl_epi64((__m128i*)(data + 8 * src_stride)));
        s[8]  = _mm_unpacklo_epi8(_mm_loadl_epi64((__m128i*)(data + 8 * src_stride)),
                                 _mm_loadl_epi64((__m128i*)(data + 9 * src_stride)));
        s[9]  = _mm_unpacklo_epi8(_mm_loadl_epi64((__m128i*)(data + 9 * src_stride)), src10);

        int i = 0;
        do {
            data  = &src_ptr[i * src_stride + j];
            s[10] = _mm_unpacklo_epi8(src10, _mm_loadl_epi64((__m128i*)(data + 11 * src_stride)));
            src10 = _mm_loadl_epi64((__m128i*)(data + 12 * src_stride));
            s[11] = _mm_unpacklo_epi8(_mm_loadl_epi64((__m128i*)(data + 11 * src_stride)), src10);

            res_lo = convolve_lo_y_12tap(s, coeffs); // Filter low index pixels
            res_hi = convolve_hi_y_12tap(s, coeffs); // Filter high index pixels

            res_lo_round = _mm_sra_epi32(_mm_add_epi32(res_lo, round_const), round_shift);
            res_hi_round = _mm_sra_epi32(_mm_add_epi32(res_hi, round_const), round_shift);

            res16 = _mm_packs_epi32(res_lo_round, res_hi_round);
            res   = _mm_packus_epi16(res16, res16);

            _mm_storel_epi64((__m128i*)(dst + i * dst_stride + j), res);
            i++;

            res_lo = convolve_lo_y_12tap(s + 1, coeffs); // Filter low index pixels
            res_hi = convolve_hi_y_12tap(s + 1, coeffs); // Filter high index pixels

            res_lo_round = _mm_sra_epi32(_mm_add_epi32(res_lo, round_const), round_shift);
            res_hi_round = _mm_sra_epi32(_mm_add_epi32(res_hi, round_const), round_shift);

            res16 = _mm_packs_epi32(res_lo_round, res_hi_round);
            res   = _mm_packus_epi16(res16, res16);

            _mm_storel_epi64((__m128i*)(dst + i * dst_stride + j), res);
            i++;

            s[0] = s[2];
            s[1] = s[3];
            s[2] = s[4];
            s[3] = s[5];
            s[4] = s[6];
            s[5] = s[7];
            s[6] = s[8];
            s[7] = s[9];
            s[8] = s[10];
            s[9] = s[11];
        } while (i < h);
        j += 8;
    } while (j < w);
}

void svt_av1_convolve_y_sr_sse2(const uint8_t* src, int32_t src_stride, uint8_t* dst, int32_t dst_stride, int32_t w,
                                int32_t h, const InterpFilterParams* filter_params_x,
                                const InterpFilterParams* filter_params_y, const int32_t subpel_x_q4,
                                const int32_t subpel_y_q4, ConvolveParams* conv_params) {
    if (filter_params_y->taps > 8) {
        if (w < 8) {
            svt_av1_convolve_y_sr_c(src,
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
            svt_av1_convolve_y_sr_12tap_sse2(src, src_stride, dst, dst_stride, w, h, filter_params_y, subpel_y_q4);
        }
    } else {
        const int      fo_vert     = filter_params_y->taps / 2 - 1;
        const uint8_t* src_ptr     = src - fo_vert * src_stride;
        const __m128i  round_const = _mm_set1_epi32((1 << FILTER_BITS) >> 1);
        const __m128i  round_shift = _mm_cvtsi32_si128(FILTER_BITS);
        __m128i        coeffs[4];

        svt_prepare_coeffs(filter_params_y, subpel_y_q4, coeffs);

        if (w <= 4) {
            __m128i s[8], src6, res, res_round, res16;
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

                res              = convolve_lo_y(s + 0, coeffs);
                res_round        = _mm_sra_epi32(_mm_add_epi32(res, round_const), round_shift);
                res16            = _mm_packs_epi32(res_round, res_round);
                uint32_t res_int = _mm_cvtsi128_si32(_mm_packus_epi16(res16, res16));

                if (w == 2) {
                    *(uint16_t*)dst = (uint16_t)res_int;
                } else {
                    *(uint32_t*)dst = res_int;
                }

                src_ptr += src_stride;
                dst += dst_stride;

                res       = convolve_lo_y(s + 1, coeffs);
                res_round = _mm_sra_epi32(_mm_add_epi32(res, round_const), round_shift);
                res16     = _mm_packs_epi32(res_round, res_round);
                res_int   = _mm_cvtsi128_si32(_mm_packus_epi16(res16, res16));

                if (w == 2) {
                    *(uint16_t*)dst = (uint16_t)res_int;
                } else {
                    *(uint32_t*)dst = res_int;
                }

                src_ptr += src_stride;
                dst += dst_stride;

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
                __m128i        s[8], src6, res_lo, res_hi;
                __m128i        res_lo_round, res_hi_round, res16, res;
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

                    res_lo = convolve_lo_y(s, coeffs); // Filter low index pixels
                    res_hi = convolve_hi_y(s, coeffs); // Filter high index pixels

                    res_lo_round = _mm_sra_epi32(_mm_add_epi32(res_lo, round_const), round_shift);
                    res_hi_round = _mm_sra_epi32(_mm_add_epi32(res_hi, round_const), round_shift);

                    res16 = _mm_packs_epi32(res_lo_round, res_hi_round);
                    res   = _mm_packus_epi16(res16, res16);

                    _mm_storel_epi64((__m128i*)(dst + i * dst_stride + j), res);
                    i++;

                    res_lo = convolve_lo_y(s + 1, coeffs); // Filter low index pixels
                    res_hi = convolve_hi_y(s + 1, coeffs); // Filter high index pixels

                    res_lo_round = _mm_sra_epi32(_mm_add_epi32(res_lo, round_const), round_shift);
                    res_hi_round = _mm_sra_epi32(_mm_add_epi32(res_hi, round_const), round_shift);

                    res16 = _mm_packs_epi32(res_lo_round, res_hi_round);
                    res   = _mm_packus_epi16(res16, res16);

                    _mm_storel_epi64((__m128i*)(dst + i * dst_stride + j), res);
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
}

void svt_av1_convolve_x_sr_12tap_sse2(const uint8_t* src, int src_stride, uint8_t* dst, int dst_stride, int w, int h,
                                      const InterpFilterParams* filter_params_x, int subpel_x_qn,
                                      ConvolveParams* conv_params) {
    const int      fo_horiz      = filter_params_x->taps / 2 - 1;
    const uint8_t* src_ptr       = src - fo_horiz;
    const int      bits          = FILTER_BITS - conv_params->round_0;
    const __m128i  round_0_const = _mm_set1_epi32((1 << conv_params->round_0) >> 1);
    const __m128i  round_const   = _mm_set1_epi32((1 << bits) >> 1);
    const __m128i  round_0_shift = _mm_cvtsi32_si128(conv_params->round_0);
    const __m128i  round_shift   = _mm_cvtsi32_si128(bits);
    const __m128i  zero          = _mm_setzero_si128();
    __m128i        coeffs[6];

    assert(bits >= 0);
    assert((FILTER_BITS - conv_params->round_1) >= 0 ||
           ((conv_params->round_0 + conv_params->round_1) == 2 * FILTER_BITS));

    prepare_coeffs_12tap(filter_params_x, subpel_x_qn, coeffs);

    int i = 0;
    do {
        int j = 0;
        do {
            const __m128i data = _mm_loadu_si128((__m128i*)&src_ptr[i * src_stride + j]);
            __m128i       s[4];

            s[0] = _mm_unpacklo_epi16(data, _mm_srli_si128(data, 1));
            s[1] = _mm_unpacklo_epi16(_mm_srli_si128(data, 2), _mm_srli_si128(data, 3));
            s[2] = _mm_unpacklo_epi16(_mm_srli_si128(data, 4), _mm_srli_si128(data, 5));
            s[3] = _mm_unpacklo_epi16(_mm_srli_si128(data, 6), _mm_srli_si128(data, 7));

            const __m128i res32 = convolve_lo_x_12tap(s, coeffs, zero);

            __m128i res32_round = _mm_sra_epi32(_mm_add_epi32(res32, round_0_const), round_0_shift);
            res32_round         = _mm_sra_epi32(_mm_add_epi32(res32_round, round_const), round_shift);

            const __m128i res16 = _mm_packs_epi32(res32_round, zero);
            const __m128i res   = _mm_packus_epi16(res16, zero);

            const int val = _mm_cvtsi128_si32(res);
            memcpy((dst + i * dst_stride + j), &val, sizeof(val));
            j += 4;
        } while (j < w);
    } while (++i < h);
}

void svt_av1_convolve_x_sr_sse2(const uint8_t* src, int32_t src_stride, uint8_t* dst, int32_t dst_stride, int32_t w,
                                int32_t h, const InterpFilterParams* filter_params_x,
                                const InterpFilterParams* filter_params_y, const int32_t subpel_x_q4,
                                const int32_t subpel_y_q4, ConvolveParams* conv_params) {
    if (filter_params_x->taps > 8) {
        if (w < 4) {
            svt_av1_convolve_x_sr_c(src,
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
            svt_av1_convolve_x_sr_12tap_sse2(
                src, src_stride, dst, dst_stride, w, h, filter_params_x, subpel_x_q4, conv_params);
        }
    } else {
        const int      fo_horiz      = filter_params_x->taps / 2 - 1;
        const uint8_t* src_ptr       = src - fo_horiz;
        const int      bits          = FILTER_BITS - conv_params->round_0;
        const __m128i  round_0_const = _mm_set1_epi32((1 << conv_params->round_0) >> 1);
        const __m128i  round_const   = _mm_set1_epi32((1 << bits) >> 1);
        const __m128i  round_0_shift = _mm_cvtsi32_si128(conv_params->round_0);
        const __m128i  round_shift   = _mm_cvtsi32_si128(bits);
        __m128i        coeffs[4];

        assert(bits >= 0);
        assert((FILTER_BITS - conv_params->round_1) >= 0 ||
               ((conv_params->round_0 + conv_params->round_1) == 2 * FILTER_BITS));

        svt_prepare_coeffs(filter_params_x, subpel_x_q4, coeffs);

        if (w <= 4) {
            do {
                const __m128i data = _mm_loadu_si128((__m128i*)src_ptr);
                __m128i       s[4];

                s[0]                       = _mm_unpacklo_epi8(data, _mm_srli_si128(data, 1));
                s[1]                       = _mm_unpacklo_epi8(_mm_srli_si128(data, 2), _mm_srli_si128(data, 3));
                s[2]                       = _mm_unpacklo_epi8(_mm_srli_si128(data, 4), _mm_srli_si128(data, 5));
                s[3]                       = _mm_unpacklo_epi8(_mm_srli_si128(data, 6), _mm_srli_si128(data, 7));
                const __m128i res_lo       = convolve_lo_x(s, coeffs);
                __m128i       res_lo_round = _mm_sra_epi32(_mm_add_epi32(res_lo, round_0_const), round_0_shift);
                res_lo_round               = _mm_sra_epi32(_mm_add_epi32(res_lo_round, round_const), round_shift);

                const __m128i res16 = _mm_packs_epi32(res_lo_round, res_lo_round);
                const __m128i res   = _mm_packus_epi16(res16, res16);

                uint32_t r = _mm_cvtsi128_si32(res);
                if (w == 2) {
                    *(uint16_t*)dst = (uint16_t)r;
                } else {
                    *(uint32_t*)dst = r;
                }

                src_ptr += src_stride;
                dst += dst_stride;
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
                    __m128i       res_lo_round = _mm_sra_epi32(_mm_add_epi32(res_lo, round_0_const), round_0_shift);
                    res_lo_round               = _mm_sra_epi32(_mm_add_epi32(res_lo_round, round_const), round_shift);
                    __m128i res_hi_round       = _mm_sra_epi32(_mm_add_epi32(res_hi, round_0_const), round_0_shift);
                    res_hi_round               = _mm_sra_epi32(_mm_add_epi32(res_hi_round, round_const), round_shift);

                    const __m128i res16 = _mm_packs_epi32(res_lo_round, res_hi_round);
                    const __m128i res   = _mm_packus_epi16(res16, res16);

                    _mm_storel_epi64((__m128i*)(dst + i * dst_stride + j), res);
                    j += 8;
                } while (j < w);
            } while (++i < h);
        }
    }
}

#define MAX_MASK_VALUE (1 << WEDGE_WEIGHT_BITS)

uint64_t svt_av1_wedge_sse_from_residuals_sse2(const int16_t* r1, const int16_t* d, const uint8_t* m, int N) {
    int n  = -N;
    int n8 = n + 8;

    uint64_t csse;

    const __m128i v_mask_max_w = _mm_set1_epi16(MAX_MASK_VALUE);
    const __m128i v_zext_q     = xx_set1_64_from_32i(0xffffffff);

    __m128i v_acc0_q = _mm_setzero_si128();

    assert(N % 64 == 0);

    r1 += N;
    d += N;
    m += N;

    do {
        const __m128i v_r0_w  = _mm_loadu_si128((__m128i*)(r1 + n));
        const __m128i v_r1_w  = _mm_loadu_si128((__m128i*)(r1 + n8));
        const __m128i v_d0_w  = _mm_loadu_si128((__m128i*)(d + n));
        const __m128i v_d1_w  = _mm_loadu_si128((__m128i*)(d + n8));
        const __m128i v_m01_b = _mm_loadu_si128((__m128i*)(m + n));

        const __m128i v_rd0l_w = _mm_unpacklo_epi16(v_d0_w, v_r0_w);
        const __m128i v_rd0h_w = _mm_unpackhi_epi16(v_d0_w, v_r0_w);
        const __m128i v_rd1l_w = _mm_unpacklo_epi16(v_d1_w, v_r1_w);
        const __m128i v_rd1h_w = _mm_unpackhi_epi16(v_d1_w, v_r1_w);
        const __m128i v_m0_w   = _mm_unpacklo_epi8(v_m01_b, _mm_setzero_si128());
        const __m128i v_m1_w   = _mm_unpackhi_epi8(v_m01_b, _mm_setzero_si128());

        const __m128i v_m0l_w = _mm_unpacklo_epi16(v_m0_w, v_mask_max_w);
        const __m128i v_m0h_w = _mm_unpackhi_epi16(v_m0_w, v_mask_max_w);
        const __m128i v_m1l_w = _mm_unpacklo_epi16(v_m1_w, v_mask_max_w);
        const __m128i v_m1h_w = _mm_unpackhi_epi16(v_m1_w, v_mask_max_w);

        const __m128i v_t0l_d = _mm_madd_epi16(v_rd0l_w, v_m0l_w);
        const __m128i v_t0h_d = _mm_madd_epi16(v_rd0h_w, v_m0h_w);
        const __m128i v_t1l_d = _mm_madd_epi16(v_rd1l_w, v_m1l_w);
        const __m128i v_t1h_d = _mm_madd_epi16(v_rd1h_w, v_m1h_w);

        const __m128i v_t0_w = _mm_packs_epi32(v_t0l_d, v_t0h_d);
        const __m128i v_t1_w = _mm_packs_epi32(v_t1l_d, v_t1h_d);

        const __m128i v_sq0_d = _mm_madd_epi16(v_t0_w, v_t0_w);
        const __m128i v_sq1_d = _mm_madd_epi16(v_t1_w, v_t1_w);

        const __m128i v_sum0_q = _mm_add_epi64(_mm_and_si128(v_sq0_d, v_zext_q), _mm_srli_epi64(v_sq0_d, 32));
        const __m128i v_sum1_q = _mm_add_epi64(_mm_and_si128(v_sq1_d, v_zext_q), _mm_srli_epi64(v_sq1_d, 32));

        v_acc0_q = _mm_add_epi64(v_acc0_q, v_sum0_q);
        v_acc0_q = _mm_add_epi64(v_acc0_q, v_sum1_q);

        n8 += 16;
        n += 16;
    } while (n);

    v_acc0_q = _mm_add_epi64(v_acc0_q, _mm_srli_si128(v_acc0_q, 8));

#if ARCH_X86_64
    csse = (uint64_t)_mm_cvtsi128_si64(v_acc0_q);
#else
    xx_storel_64(&csse, v_acc0_q);
#endif

    return ROUND_POWER_OF_TWO(csse, 2 * WEDGE_WEIGHT_BITS);
}

int8_t svt_av1_wedge_sign_from_residuals_sse2(const int16_t* ds, const uint8_t* m, int N, int64_t limit) {
    int64_t acc;

    __m128i v_sign_d;
    __m128i v_acc0_d = _mm_setzero_si128();
    __m128i v_acc1_d = _mm_setzero_si128();
    __m128i v_acc_q;

    // Input size limited to 8192 by the use of 32 bit accumulators and m
    // being between [0, 64]. Overflow might happen at larger sizes,
    // though it is practically impossible on real video input.
    assert(N < 8192);
    assert(N % 64 == 0);

    do {
        const __m128i v_m01_b = _mm_loadu_si128((__m128i*)(m));
        const __m128i v_m23_b = _mm_loadu_si128((__m128i*)(m + 16));
        const __m128i v_m45_b = _mm_loadu_si128((__m128i*)(m + 32));
        const __m128i v_m67_b = _mm_loadu_si128((__m128i*)(m + 48));

        const __m128i v_d0_w = _mm_loadu_si128((__m128i*)(ds));
        const __m128i v_d1_w = _mm_loadu_si128((__m128i*)(ds + 8));
        const __m128i v_d2_w = _mm_loadu_si128((__m128i*)(ds + 16));
        const __m128i v_d3_w = _mm_loadu_si128((__m128i*)(ds + 24));
        const __m128i v_d4_w = _mm_loadu_si128((__m128i*)(ds + 32));
        const __m128i v_d5_w = _mm_loadu_si128((__m128i*)(ds + 40));
        const __m128i v_d6_w = _mm_loadu_si128((__m128i*)(ds + 48));
        const __m128i v_d7_w = _mm_loadu_si128((__m128i*)(ds + 56));

        const __m128i v_m0_w = _mm_unpacklo_epi8(v_m01_b, _mm_setzero_si128());
        const __m128i v_m1_w = _mm_unpackhi_epi8(v_m01_b, _mm_setzero_si128());
        const __m128i v_m2_w = _mm_unpacklo_epi8(v_m23_b, _mm_setzero_si128());
        const __m128i v_m3_w = _mm_unpackhi_epi8(v_m23_b, _mm_setzero_si128());
        const __m128i v_m4_w = _mm_unpacklo_epi8(v_m45_b, _mm_setzero_si128());
        const __m128i v_m5_w = _mm_unpackhi_epi8(v_m45_b, _mm_setzero_si128());
        const __m128i v_m6_w = _mm_unpacklo_epi8(v_m67_b, _mm_setzero_si128());
        const __m128i v_m7_w = _mm_unpackhi_epi8(v_m67_b, _mm_setzero_si128());

        const __m128i v_p0_d = _mm_madd_epi16(v_d0_w, v_m0_w);
        const __m128i v_p1_d = _mm_madd_epi16(v_d1_w, v_m1_w);
        const __m128i v_p2_d = _mm_madd_epi16(v_d2_w, v_m2_w);
        const __m128i v_p3_d = _mm_madd_epi16(v_d3_w, v_m3_w);
        const __m128i v_p4_d = _mm_madd_epi16(v_d4_w, v_m4_w);
        const __m128i v_p5_d = _mm_madd_epi16(v_d5_w, v_m5_w);
        const __m128i v_p6_d = _mm_madd_epi16(v_d6_w, v_m6_w);
        const __m128i v_p7_d = _mm_madd_epi16(v_d7_w, v_m7_w);

        const __m128i v_p01_d = _mm_add_epi32(v_p0_d, v_p1_d);
        const __m128i v_p23_d = _mm_add_epi32(v_p2_d, v_p3_d);
        const __m128i v_p45_d = _mm_add_epi32(v_p4_d, v_p5_d);
        const __m128i v_p67_d = _mm_add_epi32(v_p6_d, v_p7_d);

        const __m128i v_p0123_d = _mm_add_epi32(v_p01_d, v_p23_d);
        const __m128i v_p4567_d = _mm_add_epi32(v_p45_d, v_p67_d);

        v_acc0_d = _mm_add_epi32(v_acc0_d, v_p0123_d);
        v_acc1_d = _mm_add_epi32(v_acc1_d, v_p4567_d);

        ds += 64;
        m += 64;

        N -= 64;
    } while (N);

    v_sign_d = _mm_cmplt_epi32(v_acc0_d, _mm_setzero_si128());
    v_acc0_d = _mm_add_epi64(_mm_unpacklo_epi32(v_acc0_d, v_sign_d), _mm_unpackhi_epi32(v_acc0_d, v_sign_d));

    v_sign_d = _mm_cmplt_epi32(v_acc1_d, _mm_setzero_si128());
    v_acc1_d = _mm_add_epi64(_mm_unpacklo_epi32(v_acc1_d, v_sign_d), _mm_unpackhi_epi32(v_acc1_d, v_sign_d));

    v_acc_q = _mm_add_epi64(v_acc0_d, v_acc1_d);

    v_acc_q = _mm_add_epi64(v_acc_q, _mm_srli_si128(v_acc_q, 8));

#if ARCH_X86_64
    acc = (uint64_t)_mm_cvtsi128_si64(v_acc_q);
#else
    xx_storel_64(&acc, v_acc_q);
#endif

    return acc > limit;
}

// Loads and stores to do away with the tedium of casting the address
// to the right type.
static INLINE __m128i xx_load_128(const void* a) {
    return _mm_loadu_si128((const __m128i*)a);
}

static INLINE uint64_t xx_cvtsi128_si64(__m128i a) {
#if ARCH_X86_64
    return (uint64_t)_mm_cvtsi128_si64(a);
#else
    {
        uint64_t tmp;
        _mm_storel_epi64((__m128i*)&tmp, a);
        return tmp;
    }
#endif
}

static uint64_t aom_sum_squares_i16_64n_sse2(const int16_t* src, uint32_t n) {
    const __m128i v_zext_mask_q = xx_set1_64_from_32i(0xffffffff);
    __m128i       v_acc0_q      = _mm_setzero_si128();
    __m128i       v_acc1_q      = _mm_setzero_si128();

    const int16_t* const end = src + n;

    assert(n % 64 == 0);

    while (src < end) {
        const __m128i v_val_0_w = xx_load_128(src);
        const __m128i v_val_1_w = xx_load_128(src + 8);
        const __m128i v_val_2_w = xx_load_128(src + 16);
        const __m128i v_val_3_w = xx_load_128(src + 24);
        const __m128i v_val_4_w = xx_load_128(src + 32);
        const __m128i v_val_5_w = xx_load_128(src + 40);
        const __m128i v_val_6_w = xx_load_128(src + 48);
        const __m128i v_val_7_w = xx_load_128(src + 56);

        const __m128i v_sq_0_d = _mm_madd_epi16(v_val_0_w, v_val_0_w);
        const __m128i v_sq_1_d = _mm_madd_epi16(v_val_1_w, v_val_1_w);
        const __m128i v_sq_2_d = _mm_madd_epi16(v_val_2_w, v_val_2_w);
        const __m128i v_sq_3_d = _mm_madd_epi16(v_val_3_w, v_val_3_w);
        const __m128i v_sq_4_d = _mm_madd_epi16(v_val_4_w, v_val_4_w);
        const __m128i v_sq_5_d = _mm_madd_epi16(v_val_5_w, v_val_5_w);
        const __m128i v_sq_6_d = _mm_madd_epi16(v_val_6_w, v_val_6_w);
        const __m128i v_sq_7_d = _mm_madd_epi16(v_val_7_w, v_val_7_w);

        const __m128i v_sum_01_d = _mm_add_epi32(v_sq_0_d, v_sq_1_d);
        const __m128i v_sum_23_d = _mm_add_epi32(v_sq_2_d, v_sq_3_d);
        const __m128i v_sum_45_d = _mm_add_epi32(v_sq_4_d, v_sq_5_d);
        const __m128i v_sum_67_d = _mm_add_epi32(v_sq_6_d, v_sq_7_d);

        const __m128i v_sum_0123_d = _mm_add_epi32(v_sum_01_d, v_sum_23_d);
        const __m128i v_sum_4567_d = _mm_add_epi32(v_sum_45_d, v_sum_67_d);

        const __m128i v_sum_d = _mm_add_epi32(v_sum_0123_d, v_sum_4567_d);

        v_acc0_q = _mm_add_epi64(v_acc0_q, _mm_and_si128(v_sum_d, v_zext_mask_q));
        v_acc1_q = _mm_add_epi64(v_acc1_q, _mm_srli_epi64(v_sum_d, 32));

        src += 64;
    }

    v_acc0_q = _mm_add_epi64(v_acc0_q, v_acc1_q);
    v_acc0_q = _mm_add_epi64(v_acc0_q, _mm_srli_si128(v_acc0_q, 8));
    return xx_cvtsi128_si64(v_acc0_q);
}

uint64_t svt_aom_sum_squares_i16_sse2(const int16_t* src, uint32_t n) {
    if (n % 64 == 0) {
        return aom_sum_squares_i16_64n_sse2(src, n);
    } else if (n > 64) {
        int k = n & ~(64 - 1);
        return aom_sum_squares_i16_64n_sse2(src, k) + svt_aom_sum_squares_i16_c(src + k, n - k);
    } else {
        return svt_aom_sum_squares_i16_c(src, n);
    }
}
