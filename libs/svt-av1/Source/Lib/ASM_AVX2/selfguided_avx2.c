/*
 * Copyright (c) 2018, Alliance for Open Media. All rights reserved
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
 * Media Patent License 1.0 was not distributed with this source code in the
 * PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
 */

#include "definitions.h"
#include <immintrin.h>
#include "common_dsp_rtcd.h"
#include "restoration.h"
#include "synonyms.h"
#include "synonyms_avx2.h"
#include "transpose_avx2.h"
#include "transpose_sse2.h"

static INLINE void cvt_16to32bit_8x8(const __m128i s[8], __m256i r[8]) {
    r[0] = _mm256_cvtepu16_epi32(s[0]);
    r[1] = _mm256_cvtepu16_epi32(s[1]);
    r[2] = _mm256_cvtepu16_epi32(s[2]);
    r[3] = _mm256_cvtepu16_epi32(s[3]);
    r[4] = _mm256_cvtepu16_epi32(s[4]);
    r[5] = _mm256_cvtepu16_epi32(s[5]);
    r[6] = _mm256_cvtepu16_epi32(s[6]);
    r[7] = _mm256_cvtepu16_epi32(s[7]);
}

static INLINE void add_32bit_8x8(const __m256i neighbor, __m256i r[8]) {
    r[0] = _mm256_add_epi32(neighbor, r[0]);
    r[1] = _mm256_add_epi32(r[0], r[1]);
    r[2] = _mm256_add_epi32(r[1], r[2]);
    r[3] = _mm256_add_epi32(r[2], r[3]);
    r[4] = _mm256_add_epi32(r[3], r[4]);
    r[5] = _mm256_add_epi32(r[4], r[5]);
    r[6] = _mm256_add_epi32(r[5], r[6]);
    r[7] = _mm256_add_epi32(r[6], r[7]);
}

static INLINE void store_32bit_8x8(const __m256i r[8], int32_t* const buf, const int32_t buf_stride) {
    _mm256_storeu_si256((__m256i*)(buf + 0 * buf_stride), r[0]);
    _mm256_storeu_si256((__m256i*)(buf + 1 * buf_stride), r[1]);
    _mm256_storeu_si256((__m256i*)(buf + 2 * buf_stride), r[2]);
    _mm256_storeu_si256((__m256i*)(buf + 3 * buf_stride), r[3]);
    _mm256_storeu_si256((__m256i*)(buf + 4 * buf_stride), r[4]);
    _mm256_storeu_si256((__m256i*)(buf + 5 * buf_stride), r[5]);
    _mm256_storeu_si256((__m256i*)(buf + 6 * buf_stride), r[6]);
    _mm256_storeu_si256((__m256i*)(buf + 7 * buf_stride), r[7]);
}

static AOM_FORCE_INLINE void integral_images(const uint8_t* src, int32_t src_stride, int32_t width, int32_t height,
                                             int32_t* C, int32_t* D, int32_t buf_stride) {
    const uint8_t* src_t = src;
    int32_t*       ct    = C + buf_stride + 1;
    int32_t*       dt    = D + buf_stride + 1;
    const __m256i  zero  = _mm256_setzero_si256();

    memset(C, 0, sizeof(*C) * (width + 8));
    memset(D, 0, sizeof(*D) * (width + 8));

    int y = 0;
    do {
        __m256i c_left = _mm256_setzero_si256();
        __m256i d_left = _mm256_setzero_si256();

        // zero the left column.
        ct[0 * buf_stride - 1] = dt[0 * buf_stride - 1] = 0;
        ct[1 * buf_stride - 1] = dt[1 * buf_stride - 1] = 0;
        ct[2 * buf_stride - 1] = dt[2 * buf_stride - 1] = 0;
        ct[3 * buf_stride - 1] = dt[3 * buf_stride - 1] = 0;
        ct[4 * buf_stride - 1] = dt[4 * buf_stride - 1] = 0;
        ct[5 * buf_stride - 1] = dt[5 * buf_stride - 1] = 0;
        ct[6 * buf_stride - 1] = dt[6 * buf_stride - 1] = 0;
        ct[7 * buf_stride - 1] = dt[7 * buf_stride - 1] = 0;

        int x = 0;
        do {
            __m128i s[8];
            __m256i r32[8];

            s[0] = _mm_loadl_epi64((__m128i*)(src_t + 0 * src_stride + x));
            s[1] = _mm_loadl_epi64((__m128i*)(src_t + 1 * src_stride + x));
            s[2] = _mm_loadl_epi64((__m128i*)(src_t + 2 * src_stride + x));
            s[3] = _mm_loadl_epi64((__m128i*)(src_t + 3 * src_stride + x));
            s[4] = _mm_loadl_epi64((__m128i*)(src_t + 4 * src_stride + x));
            s[5] = _mm_loadl_epi64((__m128i*)(src_t + 5 * src_stride + x));
            s[6] = _mm_loadl_epi64((__m128i*)(src_t + 6 * src_stride + x));
            s[7] = _mm_loadl_epi64((__m128i*)(src_t + 7 * src_stride + x));

            partial_transpose_8bit_8x8(s, s);

            s[7] = _mm_unpackhi_epi8(s[3], _mm_setzero_si128());
            s[6] = _mm_unpacklo_epi8(s[3], _mm_setzero_si128());
            s[5] = _mm_unpackhi_epi8(s[2], _mm_setzero_si128());
            s[4] = _mm_unpacklo_epi8(s[2], _mm_setzero_si128());
            s[3] = _mm_unpackhi_epi8(s[1], _mm_setzero_si128());
            s[2] = _mm_unpacklo_epi8(s[1], _mm_setzero_si128());
            s[1] = _mm_unpackhi_epi8(s[0], _mm_setzero_si128());
            s[0] = _mm_unpacklo_epi8(s[0], _mm_setzero_si128());

            cvt_16to32bit_8x8(s, r32);
            add_32bit_8x8(d_left, r32);
            d_left = r32[7];

            transpose_32bit_8x8_avx2(r32, r32);

            const __m256i d_top = _mm256_loadu_si256((__m256i*)(dt - buf_stride + x));
            add_32bit_8x8(d_top, r32);
            store_32bit_8x8(r32, dt + x, buf_stride);

            s[0] = _mm_mullo_epi16(s[0], s[0]);
            s[1] = _mm_mullo_epi16(s[1], s[1]);
            s[2] = _mm_mullo_epi16(s[2], s[2]);
            s[3] = _mm_mullo_epi16(s[3], s[3]);
            s[4] = _mm_mullo_epi16(s[4], s[4]);
            s[5] = _mm_mullo_epi16(s[5], s[5]);
            s[6] = _mm_mullo_epi16(s[6], s[6]);
            s[7] = _mm_mullo_epi16(s[7], s[7]);

            cvt_16to32bit_8x8(s, r32);
            add_32bit_8x8(c_left, r32);
            c_left = r32[7];

            transpose_32bit_8x8_avx2(r32, r32);

            const __m256i c_top = _mm256_loadu_si256((__m256i*)(ct - buf_stride + x));
            add_32bit_8x8(c_top, r32);
            store_32bit_8x8(r32, ct + x, buf_stride);
            x += 8;
        } while (x < width);

        /* Used in calc_ab and calc_ab_fast, when calc out of right border */
        for (int ln = 0; ln < 8; ++ln) {
            _mm256_storeu_si256((__m256i*)(ct + x + ln * buf_stride), zero);
            _mm256_storeu_si256((__m256i*)(dt + x + ln * buf_stride), zero);
        }

        src_t += 8 * src_stride;
        ct += 8 * buf_stride;
        dt += 8 * buf_stride;
        y += 8;
    } while (y < height);
}

static AOM_FORCE_INLINE void integral_images_highbd(const uint16_t* src, int32_t src_stride, int32_t width,
                                                    int32_t height, int32_t* C, int32_t* D, int32_t buf_stride) {
    const uint16_t* src_t = src;
    int32_t*        ct    = C + buf_stride + 1;
    int32_t*        dt    = D + buf_stride + 1;
    const __m256i   zero  = _mm256_setzero_si256();

    memset(C, 0, sizeof(*C) * (width + 8));
    memset(D, 0, sizeof(*D) * (width + 8));

    int y = 0;
    do {
        __m256i c_left = _mm256_setzero_si256();
        __m256i d_left = _mm256_setzero_si256();

        // zero the left column.
        ct[0 * buf_stride - 1] = dt[0 * buf_stride - 1] = 0;
        ct[1 * buf_stride - 1] = dt[1 * buf_stride - 1] = 0;
        ct[2 * buf_stride - 1] = dt[2 * buf_stride - 1] = 0;
        ct[3 * buf_stride - 1] = dt[3 * buf_stride - 1] = 0;
        ct[4 * buf_stride - 1] = dt[4 * buf_stride - 1] = 0;
        ct[5 * buf_stride - 1] = dt[5 * buf_stride - 1] = 0;
        ct[6 * buf_stride - 1] = dt[6 * buf_stride - 1] = 0;
        ct[7 * buf_stride - 1] = dt[7 * buf_stride - 1] = 0;

        int x = 0;
        do {
            __m128i s[8];
            __m256i r32[8], a32[8];

            s[0] = _mm_loadu_si128((__m128i*)(src_t + 0 * src_stride + x));
            s[1] = _mm_loadu_si128((__m128i*)(src_t + 1 * src_stride + x));
            s[2] = _mm_loadu_si128((__m128i*)(src_t + 2 * src_stride + x));
            s[3] = _mm_loadu_si128((__m128i*)(src_t + 3 * src_stride + x));
            s[4] = _mm_loadu_si128((__m128i*)(src_t + 4 * src_stride + x));
            s[5] = _mm_loadu_si128((__m128i*)(src_t + 5 * src_stride + x));
            s[6] = _mm_loadu_si128((__m128i*)(src_t + 6 * src_stride + x));
            s[7] = _mm_loadu_si128((__m128i*)(src_t + 7 * src_stride + x));

            transpose_16bit_8x8(s, s);

            cvt_16to32bit_8x8(s, r32);

            a32[0] = _mm256_madd_epi16(r32[0], r32[0]);
            a32[1] = _mm256_madd_epi16(r32[1], r32[1]);
            a32[2] = _mm256_madd_epi16(r32[2], r32[2]);
            a32[3] = _mm256_madd_epi16(r32[3], r32[3]);
            a32[4] = _mm256_madd_epi16(r32[4], r32[4]);
            a32[5] = _mm256_madd_epi16(r32[5], r32[5]);
            a32[6] = _mm256_madd_epi16(r32[6], r32[6]);
            a32[7] = _mm256_madd_epi16(r32[7], r32[7]);

            add_32bit_8x8(c_left, a32);
            c_left = a32[7];

            transpose_32bit_8x8_avx2(a32, a32);

            const __m256i c_top = _mm256_loadu_si256((__m256i*)(ct - buf_stride + x));
            add_32bit_8x8(c_top, a32);
            store_32bit_8x8(a32, ct + x, buf_stride);

            add_32bit_8x8(d_left, r32);
            d_left = r32[7];

            transpose_32bit_8x8_avx2(r32, r32);

            const __m256i d_top = _mm256_loadu_si256((__m256i*)(dt - buf_stride + x));
            add_32bit_8x8(d_top, r32);
            store_32bit_8x8(r32, dt + x, buf_stride);
            x += 8;
        } while (x < width);

        /* Used in calc_ab and calc_ab_fast, when calc out of right border */
        for (int ln = 0; ln < 8; ++ln) {
            _mm256_storeu_si256((__m256i*)(ct + x + ln * buf_stride), zero);
            _mm256_storeu_si256((__m256i*)(dt + x + ln * buf_stride), zero);
        }

        src_t += 8 * src_stride;
        ct += 8 * buf_stride;
        dt += 8 * buf_stride;
        y += 8;
    } while (y < height);
}

// Compute 8 values of boxsum from the given integral image. ii should point
// at the middle of the box (for the first value). r is the box radius.
static INLINE __m256i boxsum_from_ii(const int32_t* ii, int32_t stride, int32_t r) {
    const __m256i tl = yy_loadu_256(ii - (r + 1) - (r + 1) * stride);
    const __m256i tr = yy_loadu_256(ii + (r + 0) - (r + 1) * stride);
    const __m256i bl = yy_loadu_256(ii - (r + 1) + r * stride);
    const __m256i br = yy_loadu_256(ii + (r + 0) + r * stride);
    const __m256i u  = _mm256_sub_epi32(tr, tl);
    const __m256i v  = _mm256_sub_epi32(br, bl);
    return _mm256_sub_epi32(v, u);
}

static INLINE __m256i round_for_shift(unsigned shift) {
    return _mm256_set1_epi32((1 << shift) >> 1);
}

static INLINE __m256i compute_p(__m256i sum1, __m256i sum2, int32_t n) {
    const __m256i bb = _mm256_madd_epi16(sum1, sum1);
    const __m256i an = _mm256_mullo_epi32(sum2, _mm256_set1_epi32(n));
    return _mm256_sub_epi32(an, bb);
}

static INLINE __m256i compute_p_highbd(__m256i sum1, __m256i sum2, int32_t bit_depth, int32_t n) {
    const __m256i rounding_a = round_for_shift(2 * (bit_depth - 8));
    const __m256i rounding_b = round_for_shift(bit_depth - 8);
    const __m128i shift_a    = _mm_cvtsi32_si128(2 * (bit_depth - 8));
    const __m128i shift_b    = _mm_cvtsi32_si128(bit_depth - 8);
    const __m256i a          = _mm256_srl_epi32(_mm256_add_epi32(sum2, rounding_a), shift_a);
    const __m256i b          = _mm256_srl_epi32(_mm256_add_epi32(sum1, rounding_b), shift_b);
    // b < 2^14, so we can use a 16-bit madd rather than a 32-bit
    // mullo to square it
    const __m256i bb = _mm256_madd_epi16(b, b);
    const __m256i an = _mm256_max_epi32(_mm256_mullo_epi32(a, _mm256_set1_epi32(n)), bb);
    return _mm256_sub_epi32(an, bb);
}

// Assumes that C, D are integral images for the original buffer which has been
// extended to have a padding of SGRPROJ_BORDER_VERT/SGRPROJ_BORDER_HORZ pixels
// on the sides. A, b, C, D point at logical position (0, 0).
static AOM_FORCE_INLINE void calc_ab(int32_t* A, int32_t* b, const int32_t* C, const int32_t* D, int32_t width,
                                     int32_t height, int32_t buf_stride, int32_t bit_depth, int32_t sgr_params_idx,
                                     int32_t radius_idx) {
    const SgrParamsType* const params = &svt_aom_eb_sgr_params[sgr_params_idx];
    const int32_t              r      = params->r[radius_idx];
    const int32_t              n      = (2 * r + 1) * (2 * r + 1);
    const __m256i              s      = _mm256_set1_epi32(params->s[radius_idx]);
    // one_over_n[n-1] is 2^12/n, so easily fits in an int16
    const __m256i one_over_n = _mm256_set1_epi32(svt_aom_eb_one_by_x[n - 1]);
    const __m256i rnd_z      = round_for_shift(SGRPROJ_MTABLE_BITS);
    const __m256i rnd_res    = round_for_shift(SGRPROJ_RECIP_BITS);

    A -= buf_stride + 1;
    b -= buf_stride + 1;
    C -= buf_stride + 1;
    D -= buf_stride + 1;

    int32_t i = height + 2;

    if (bit_depth == 8) {
        do {
            int32_t j = 0;
            do {
                const __m256i sum1 = boxsum_from_ii(D + j, buf_stride, r);
                const __m256i sum2 = boxsum_from_ii(C + j, buf_stride, r);
                const __m256i p    = compute_p(sum1, sum2, n);
                const __m256i z    = _mm256_min_epi32(
                    _mm256_srli_epi32(_mm256_add_epi32(_mm256_mullo_epi32(p, s), rnd_z), SGRPROJ_MTABLE_BITS),
                    _mm256_set1_epi32(255));
                const __m256i a_res = _mm256_i32gather_epi32(svt_aom_eb_x_by_xplus1, z, 4);
                yy_storeu_256(A + j, a_res);

                const __m256i a_complement = _mm256_sub_epi32(_mm256_set1_epi32(SGRPROJ_SGR), a_res);

                // sum1 might have lanes greater than 2^15, so we can't use madd to do
                // multiplication involving sum1. However, a_complement and one_over_n
                // are both less than 256, so we can multiply them first.
                const __m256i a_comp_over_n = _mm256_madd_epi16(a_complement, one_over_n);
                const __m256i b_int         = _mm256_mullo_epi32(a_comp_over_n, sum1);
                const __m256i b_res         = _mm256_srli_epi32(_mm256_add_epi32(b_int, rnd_res), SGRPROJ_RECIP_BITS);
                yy_storeu_256(b + j, b_res);
                j += 8;
            } while (j < width + 2);

            A += buf_stride;
            b += buf_stride;
            C += buf_stride;
            D += buf_stride;
        } while (--i);
    } else {
        do {
            int32_t j = 0;
            do {
                const __m256i sum1 = boxsum_from_ii(D + j, buf_stride, r);
                const __m256i sum2 = boxsum_from_ii(C + j, buf_stride, r);
                const __m256i p    = compute_p_highbd(sum1, sum2, bit_depth, n);
                const __m256i z    = _mm256_min_epi32(
                    _mm256_srli_epi32(_mm256_add_epi32(_mm256_mullo_epi32(p, s), rnd_z), SGRPROJ_MTABLE_BITS),
                    _mm256_set1_epi32(255));
                const __m256i a_res = _mm256_i32gather_epi32(svt_aom_eb_x_by_xplus1, z, 4);
                yy_storeu_256(A + j, a_res);

                const __m256i a_complement = _mm256_sub_epi32(_mm256_set1_epi32(SGRPROJ_SGR), a_res);

                // sum1 might have lanes greater than 2^15, so we can't use madd to do
                // multiplication involving sum1. However, a_complement and one_over_n
                // are both less than 256, so we can multiply them first.
                const __m256i a_comp_over_n = _mm256_madd_epi16(a_complement, one_over_n);
                const __m256i b_int         = _mm256_mullo_epi32(a_comp_over_n, sum1);
                const __m256i b_res         = _mm256_srli_epi32(_mm256_add_epi32(b_int, rnd_res), SGRPROJ_RECIP_BITS);
                yy_storeu_256(b + j, b_res);
                j += 8;
            } while (j < width + 2);

            A += buf_stride;
            b += buf_stride;
            C += buf_stride;
            D += buf_stride;
        } while (--i);
    }
}

// Calculate 8 values of the "cross sum" starting at buf. This is a 3x3 filter
// where the outer four corners have weight 3 and all other pixels have weight
// 4.
//
// Pixels are indexed as follows:
// xtl  xt   xtr
// xl    x   xr
// xbl  xb   xbr
//
// buf points to x
//
// fours = xl + xt + xr + xb + x
// threes = xtl + xtr + xbr + xbl
// cross_sum = 4 * fours + 3 * threes
//           = 4 * (fours + threes) - threes
//           = (fours + threes) << 2 - threes
static INLINE __m256i cross_sum(const int32_t* buf, int32_t stride) {
    const __m256i xtl = yy_loadu_256(buf - 1 - stride);
    const __m256i xt  = yy_loadu_256(buf - stride);
    const __m256i xtr = yy_loadu_256(buf + 1 - stride);
    const __m256i xl  = yy_loadu_256(buf - 1);
    const __m256i x   = yy_loadu_256(buf);
    const __m256i xr  = yy_loadu_256(buf + 1);
    const __m256i xbl = yy_loadu_256(buf - 1 + stride);
    const __m256i xb  = yy_loadu_256(buf + stride);
    const __m256i xbr = yy_loadu_256(buf + 1 + stride);

    const __m256i fours  = _mm256_add_epi32(xl, _mm256_add_epi32(xt, _mm256_add_epi32(xr, _mm256_add_epi32(xb, x))));
    const __m256i threes = _mm256_add_epi32(xtl, _mm256_add_epi32(xtr, _mm256_add_epi32(xbr, xbl)));

    return _mm256_sub_epi32(_mm256_slli_epi32(_mm256_add_epi32(fours, threes), 2), threes);
}

// The final filter for self-guided restoration. Computes a weighted average
// across A, b with "cross sums" (see cross_sum implementation above).
static AOM_FORCE_INLINE void final_filter(int32_t* dst, int32_t dst_stride, const int32_t* A, const int32_t* B,
                                          int32_t buf_stride, const uint8_t* dgd8, int32_t dgd_stride, int32_t width,
                                          int32_t height, int32_t highbd) {
    const int32_t nb       = 5;
    const __m256i rounding = round_for_shift(SGRPROJ_SGR_BITS + nb - SGRPROJ_RST_BITS);
    int32_t       i        = height;

    if (!highbd) {
        do {
            int32_t j = 0;
            do {
                const __m256i a   = cross_sum(A + j, buf_stride);
                const __m256i b   = cross_sum(B + j, buf_stride);
                const __m128i raw = xx_loadl_64(dgd8 + j);
                const __m256i src = _mm256_cvtepu8_epi32(raw);
                const __m256i v   = _mm256_add_epi32(_mm256_madd_epi16(a, src), b);
                const __m256i w   = _mm256_srai_epi32(_mm256_add_epi32(v, rounding),
                                                    SGRPROJ_SGR_BITS + nb - SGRPROJ_RST_BITS);
                yy_storeu_256(dst + j, w);
                j += 8;
            } while (j < width);

            A += buf_stride;
            B += buf_stride;
            dgd8 += dgd_stride;
            dst += dst_stride;
        } while (--i);
    } else {
        const uint16_t* dgd_real = CONVERT_TO_SHORTPTR(dgd8);

        do {
            int32_t j = 0;
            do {
                const __m256i a   = cross_sum(A + j, buf_stride);
                const __m256i b   = cross_sum(B + j, buf_stride);
                const __m128i raw = xx_loadu_128(dgd_real + j);
                const __m256i src = _mm256_cvtepu16_epi32(raw);
                const __m256i v   = _mm256_add_epi32(_mm256_madd_epi16(a, src), b);
                const __m256i w   = _mm256_srai_epi32(_mm256_add_epi32(v, rounding),
                                                    SGRPROJ_SGR_BITS + nb - SGRPROJ_RST_BITS);
                yy_storeu_256(dst + j, w);
                j += 8;
            } while (j < width);

            A += buf_stride;
            B += buf_stride;
            dgd_real += dgd_stride;
            dst += dst_stride;
        } while (--i);
    }
}

// Assumes that C, D are integral images for the original buffer which has been
// extended to have a padding of SGRPROJ_BORDER_VERT/SGRPROJ_BORDER_HORZ pixels
// on the sides. A, b, C, D point at logical position (0, 0).
static AOM_FORCE_INLINE void calc_ab_fast(int32_t* A, int32_t* b, const int32_t* C, const int32_t* D, int32_t width,
                                          int32_t height, int32_t buf_stride, int32_t bit_depth, int32_t sgr_params_idx,
                                          int32_t radius_idx) {
    const SgrParamsType* const params = &svt_aom_eb_sgr_params[sgr_params_idx];
    const int32_t              r      = params->r[radius_idx];
    const int32_t              n      = (2 * r + 1) * (2 * r + 1);
    const __m256i              s      = _mm256_set1_epi32(params->s[radius_idx]);
    // one_over_n[n-1] is 2^12/n, so easily fits in an int16
    const __m256i one_over_n = _mm256_set1_epi32(svt_aom_eb_one_by_x[n - 1]);
    const __m256i rnd_z      = round_for_shift(SGRPROJ_MTABLE_BITS);
    const __m256i rnd_res    = round_for_shift(SGRPROJ_RECIP_BITS);

    A -= buf_stride + 1;
    b -= buf_stride + 1;
    C -= buf_stride + 1;
    D -= buf_stride + 1;

    int32_t i = 0;
    if (bit_depth == 8) {
        do {
            int32_t j = 0;
            do {
                const __m256i sum1 = boxsum_from_ii(D + j, buf_stride, r);
                const __m256i sum2 = boxsum_from_ii(C + j, buf_stride, r);
                const __m256i p    = compute_p(sum1, sum2, n);
                const __m256i z    = _mm256_min_epi32(
                    _mm256_srli_epi32(_mm256_add_epi32(_mm256_mullo_epi32(p, s), rnd_z), SGRPROJ_MTABLE_BITS),
                    _mm256_set1_epi32(255));
                const __m256i a_res = _mm256_i32gather_epi32(svt_aom_eb_x_by_xplus1, z, 4);
                yy_storeu_256(A + j, a_res);

                const __m256i a_complement = _mm256_sub_epi32(_mm256_set1_epi32(SGRPROJ_SGR), a_res);

                // sum1 might have lanes greater than 2^15, so we can't use madd to do
                // multiplication involving sum1. However, a_complement and one_over_n
                // are both less than 256, so we can multiply them first.
                const __m256i a_comp_over_n = _mm256_madd_epi16(a_complement, one_over_n);
                const __m256i b_int         = _mm256_mullo_epi32(a_comp_over_n, sum1);
                const __m256i b_res         = _mm256_srli_epi32(_mm256_add_epi32(b_int, rnd_res), SGRPROJ_RECIP_BITS);
                yy_storeu_256(b + j, b_res);
                j += 8;
            } while (j < width + 2);

            A += 2 * buf_stride;
            b += 2 * buf_stride;
            C += 2 * buf_stride;
            D += 2 * buf_stride;
            i += 2;
        } while (i < height + 2);
    } else {
        do {
            int32_t j = 0;
            do {
                const __m256i sum1 = boxsum_from_ii(D + j, buf_stride, r);
                const __m256i sum2 = boxsum_from_ii(C + j, buf_stride, r);
                const __m256i p    = compute_p_highbd(sum1, sum2, bit_depth, n);
                const __m256i z    = _mm256_min_epi32(
                    _mm256_srli_epi32(_mm256_add_epi32(_mm256_mullo_epi32(p, s), rnd_z), SGRPROJ_MTABLE_BITS),
                    _mm256_set1_epi32(255));
                const __m256i a_res = _mm256_i32gather_epi32(svt_aom_eb_x_by_xplus1, z, 4);
                yy_storeu_256(A + j, a_res);

                const __m256i a_complement = _mm256_sub_epi32(_mm256_set1_epi32(SGRPROJ_SGR), a_res);

                // sum1 might have lanes greater than 2^15, so we can't use madd to do
                // multiplication involving sum1. However, a_complement and one_over_n
                // are both less than 256, so we can multiply them first.
                const __m256i a_comp_over_n = _mm256_madd_epi16(a_complement, one_over_n);
                const __m256i b_int         = _mm256_mullo_epi32(a_comp_over_n, sum1);
                const __m256i b_res         = _mm256_srli_epi32(_mm256_add_epi32(b_int, rnd_res), SGRPROJ_RECIP_BITS);
                yy_storeu_256(b + j, b_res);
                j += 8;
            } while (j < width + 2);

            A += 2 * buf_stride;
            b += 2 * buf_stride;
            C += 2 * buf_stride;
            D += 2 * buf_stride;
            i += 2;
        } while (i < height + 2);
    }
}

// Calculate 8 values of the "cross sum" starting at buf.
//
// Pixels are indexed like this:
// xtl  xt   xtr
//  -   buf   -
// xbl  xb   xbr
//
// Pixels are weighted like this:
//  5    6    5
//  0    0    0
//  5    6    5
//
// fives = xtl + xtr + xbl + xbr
// sixes = xt + xb
// cross_sum = 6 * sixes + 5 * fives
//           = 5 * (fives + sixes) - sixes
//           = (fives + sixes) << 2 + (fives + sixes) + sixes
static INLINE __m256i cross_sum_fast_even_row(const int32_t* buf, int32_t stride) {
    const __m256i xtl = yy_loadu_256(buf - 1 - stride);
    const __m256i xt  = yy_loadu_256(buf - stride);
    const __m256i xtr = yy_loadu_256(buf + 1 - stride);
    const __m256i xbl = yy_loadu_256(buf - 1 + stride);
    const __m256i xb  = yy_loadu_256(buf + stride);
    const __m256i xbr = yy_loadu_256(buf + 1 + stride);

    const __m256i fives            = _mm256_add_epi32(xtl, _mm256_add_epi32(xtr, _mm256_add_epi32(xbr, xbl)));
    const __m256i sixes            = _mm256_add_epi32(xt, xb);
    const __m256i fives_plus_sixes = _mm256_add_epi32(fives, sixes);

    return _mm256_add_epi32(_mm256_add_epi32(_mm256_slli_epi32(fives_plus_sixes, 2), fives_plus_sixes), sixes);
}

// Calculate 8 values of the "cross sum" starting at buf.
//
// Pixels are indexed like this:
// xl    x   xr
//
// Pixels are weighted like this:
//  5    6    5
//
// buf points to x
//
// fives = xl + xr
// sixes = x
// cross_sum = 5 * fives + 6 * sixes
//           = 4 * (fives + sixes) + (fives + sixes) + sixes
//           = (fives + sixes) << 2 + (fives + sixes) + sixes
static INLINE __m256i cross_sum_fast_odd_row(const int32_t* buf) {
    const __m256i xl = yy_loadu_256(buf - 1);
    const __m256i x  = yy_loadu_256(buf);
    const __m256i xr = yy_loadu_256(buf + 1);

    const __m256i fives = _mm256_add_epi32(xl, xr);
    const __m256i sixes = x;

    const __m256i fives_plus_sixes = _mm256_add_epi32(fives, sixes);

    return _mm256_add_epi32(_mm256_add_epi32(_mm256_slli_epi32(fives_plus_sixes, 2), fives_plus_sixes), sixes);
}

// The final filter for the self-guided restoration. Computes a
// weighted average across A, b with "cross sums" (see cross_sum_...
// implementations above).
static AOM_FORCE_INLINE void final_filter_fast(int32_t* dst, int32_t dst_stride, const int32_t* A, const int32_t* B,
                                               int32_t buf_stride, const uint8_t* dgd8, int32_t dgd_stride,
                                               int32_t width, int32_t height, int32_t highbd) {
    const int32_t nb0       = 5;
    const int32_t nb1       = 4;
    const __m256i rounding0 = round_for_shift(SGRPROJ_SGR_BITS + nb0 - SGRPROJ_RST_BITS);
    const __m256i rounding1 = round_for_shift(SGRPROJ_SGR_BITS + nb1 - SGRPROJ_RST_BITS);
    int32_t       i         = 0;

    if (!highbd) {
        do {
            if (!(i & 1)) { // even row
                int32_t j = 0;
                do {
                    const __m256i a   = cross_sum_fast_even_row(A + j, buf_stride);
                    const __m256i b   = cross_sum_fast_even_row(B + j, buf_stride);
                    const __m128i raw = xx_loadl_64(dgd8 + j);
                    const __m256i src = _mm256_cvtepu8_epi32(raw);
                    const __m256i v   = _mm256_add_epi32(_mm256_madd_epi16(a, src), b);
                    const __m256i w   = _mm256_srai_epi32(_mm256_add_epi32(v, rounding0),
                                                        SGRPROJ_SGR_BITS + nb0 - SGRPROJ_RST_BITS);
                    yy_storeu_256(dst + j, w);
                    j += 8;
                } while (j < width);
            } else { // odd row
                int32_t j = 0;
                do {
                    const __m256i a   = cross_sum_fast_odd_row(A + j);
                    const __m256i b   = cross_sum_fast_odd_row(B + j);
                    const __m128i raw = xx_loadl_64(dgd8 + j);
                    const __m256i src = _mm256_cvtepu8_epi32(raw);
                    const __m256i v   = _mm256_add_epi32(_mm256_madd_epi16(a, src), b);
                    const __m256i w   = _mm256_srai_epi32(_mm256_add_epi32(v, rounding1),
                                                        SGRPROJ_SGR_BITS + nb1 - SGRPROJ_RST_BITS);
                    yy_storeu_256(dst + j, w);
                    j += 8;
                } while (j < width);
            }

            A += buf_stride;
            B += buf_stride;
            dgd8 += dgd_stride;
            dst += dst_stride;
        } while (++i < height);
    } else {
        const uint16_t* dgd_real = CONVERT_TO_SHORTPTR(dgd8);

        do {
            if (!(i & 1)) { // even row
                int32_t j = 0;
                do {
                    const __m256i a   = cross_sum_fast_even_row(A + j, buf_stride);
                    const __m256i b   = cross_sum_fast_even_row(B + j, buf_stride);
                    const __m128i raw = xx_loadu_128(dgd_real + j);
                    const __m256i src = _mm256_cvtepu16_epi32(raw);
                    const __m256i v   = _mm256_add_epi32(_mm256_madd_epi16(a, src), b);
                    const __m256i w   = _mm256_srai_epi32(_mm256_add_epi32(v, rounding0),
                                                        SGRPROJ_SGR_BITS + nb0 - SGRPROJ_RST_BITS);
                    yy_storeu_256(dst + j, w);
                    j += 8;
                } while (j < width);
            } else { // odd row
                int32_t j = 0;
                do {
                    const __m256i a   = cross_sum_fast_odd_row(A + j);
                    const __m256i b   = cross_sum_fast_odd_row(B + j);
                    const __m128i raw = xx_loadu_128(dgd_real + j);
                    const __m256i src = _mm256_cvtepu16_epi32(raw);
                    const __m256i v   = _mm256_add_epi32(_mm256_madd_epi16(a, src), b);
                    const __m256i w   = _mm256_srai_epi32(_mm256_add_epi32(v, rounding1),
                                                        SGRPROJ_SGR_BITS + nb1 - SGRPROJ_RST_BITS);
                    yy_storeu_256(dst + j, w);
                    j += 8;
                } while (j < width);
            }

            A += buf_stride;
            B += buf_stride;
            dgd_real += dgd_stride;
            dst += dst_stride;
        } while (++i < height);
    }
}

void svt_av1_selfguided_restoration_avx2(const uint8_t* dgd8, int32_t width, int32_t height, int32_t dgd_stride,
                                         int32_t* flt0, int32_t* flt1, int32_t flt_stride, int32_t sgr_params_idx,
                                         int32_t bit_depth, int32_t highbd) {
    // The ALIGN_POWER_OF_TWO macro here ensures that column 1 of atl, btl,
    // ctl and dtl is 32-byte aligned.
    const int32_t buf_elts = ALIGN_POWER_OF_TWO(RESTORATION_PROC_UNIT_PELS, 3);

    DECLARE_ALIGNED(32, int32_t, buf[4 * ALIGN_POWER_OF_TWO(RESTORATION_PROC_UNIT_PELS, 3)]);

    const int32_t width_ext  = width + 2 * SGRPROJ_BORDER_HORZ;
    const int32_t height_ext = height + 2 * SGRPROJ_BORDER_VERT;

    // Adjusting the stride of A and b here appears to avoid bad cache effects,
    // leading to a significant speed improvement.
    // We also align the stride to a multiple of 32 bytes for efficiency.
    int32_t buf_stride = ALIGN_POWER_OF_TWO(width_ext + 16, 3);

    // The "tl" pointers point at the top-left of the initialised data for the
    // array.
    int32_t* atl = buf + 0 * buf_elts + 7;
    int32_t* btl = buf + 1 * buf_elts + 7;
    int32_t* ctl = buf + 2 * buf_elts + 7;
    int32_t* dtl = buf + 3 * buf_elts + 7;

    // The "0" pointers are (- SGRPROJ_BORDER_VERT, -SGRPROJ_BORDER_HORZ). Note
    // there's a zero row and column in A, b (integral images), so we move down
    // and right one for them.
    const int32_t buf_diag_border = SGRPROJ_BORDER_HORZ + buf_stride * SGRPROJ_BORDER_VERT;

    int32_t* a0 = atl + 1 + buf_stride;
    int32_t* b0 = btl + 1 + buf_stride;
    int32_t* c0 = ctl + 1 + buf_stride;
    int32_t* d0 = dtl + 1 + buf_stride;

    // Finally, A, b, C, D point at position (0, 0).
    int32_t* A = a0 + buf_diag_border;
    int32_t* b = b0 + buf_diag_border;
    int32_t* C = c0 + buf_diag_border;
    int32_t* D = d0 + buf_diag_border;

    const int32_t  dgd_diag_border = SGRPROJ_BORDER_HORZ + dgd_stride * SGRPROJ_BORDER_VERT;
    const uint8_t* dgd0            = dgd8 - dgd_diag_border;

    // Generate integral images from the input. C will contain sums of squares; D
    // will contain just sums
    if (highbd) {
        integral_images_highbd(CONVERT_TO_SHORTPTR(dgd0), dgd_stride, width_ext, height_ext, ctl, dtl, buf_stride);
    } else {
        integral_images(dgd0, dgd_stride, width_ext, height_ext, ctl, dtl, buf_stride);
    }

    const SgrParamsType* const params = &svt_aom_eb_sgr_params[sgr_params_idx];
    // Write to flt0 and flt1
    // If params->r == 0 we skip the corresponding filter. We only allow one of
    // the radii to be 0, as having both equal to 0 would be equivalent to
    // skipping SGR entirely.
    assert(!(params->r[0] == 0 && params->r[1] == 0));
    assert(params->r[0] < 3); // AOMMIN(SGRPROJ_BORDER_VERT, SGRPROJ_BORDER_HORZ) == 3
    assert(params->r[1] < 3); // AOMMIN(SGRPROJ_BORDER_VERT, SGRPROJ_BORDER_HORZ) == 3

    if (params->r[0] > 0) {
        calc_ab_fast(A, b, C, D, width, height, buf_stride, bit_depth, sgr_params_idx, 0);
        final_filter_fast(flt0, flt_stride, A, b, buf_stride, dgd8, dgd_stride, width, height, highbd);
    }

    if (params->r[1] > 0) {
        calc_ab(A, b, C, D, width, height, buf_stride, bit_depth, sgr_params_idx, 1);
        final_filter(flt1, flt_stride, A, b, buf_stride, dgd8, dgd_stride, width, height, highbd);
    }
}

void svt_apply_selfguided_restoration_avx2(const uint8_t* dat8, int32_t width, int32_t height, int32_t stride,
                                           int32_t eps, const int32_t* xqd, uint8_t* dst8, int32_t dst_stride,
                                           int32_t* tmpbuf, int32_t bit_depth, int32_t highbd) {
    int32_t* flt0 = tmpbuf;
    int32_t* flt1 = flt0 + RESTORATION_UNITPELS_MAX;
    assert(width * height <= RESTORATION_UNITPELS_MAX);
    svt_av1_selfguided_restoration_avx2(dat8, width, height, stride, flt0, flt1, width, eps, bit_depth, highbd);
    const SgrParamsType* const params = &svt_aom_eb_sgr_params[eps];
    int32_t                    xq[2];
    svt_decode_xq(xqd, xq, params);

    const __m256i xq0      = _mm256_set1_epi32(xq[0]);
    const __m256i xq1      = _mm256_set1_epi32(xq[1]);
    const __m256i rounding = round_for_shift(SGRPROJ_PRJ_BITS + SGRPROJ_RST_BITS);

    int32_t i = height;

    if (!highbd) {
        const __m256i idx = _mm256_setr_epi32(0, 4, 1, 5, 0, 0, 0, 0);

        do {
            // Calculate output in batches of 16 pixels
            int32_t j = 0;
            do {
                const __m128i src  = xx_loadu_128(dat8 + j);
                const __m256i ep_0 = _mm256_cvtepu8_epi32(src);
                const __m256i ep_1 = _mm256_cvtepu8_epi32(_mm_srli_si128(src, 8));
                const __m256i u_0  = _mm256_slli_epi32(ep_0, SGRPROJ_RST_BITS);
                const __m256i u_1  = _mm256_slli_epi32(ep_1, SGRPROJ_RST_BITS);
                __m256i       v_0  = _mm256_slli_epi32(u_0, SGRPROJ_PRJ_BITS);
                __m256i       v_1  = _mm256_slli_epi32(u_1, SGRPROJ_PRJ_BITS);

                if (params->r[0] > 0) {
                    const __m256i f1_0 = _mm256_sub_epi32(yy_loadu_256(&flt0[j + 0]), u_0);
                    const __m256i f1_1 = _mm256_sub_epi32(yy_loadu_256(&flt0[j + 8]), u_1);
                    v_0                = _mm256_add_epi32(v_0, _mm256_mullo_epi32(xq0, f1_0));
                    v_1                = _mm256_add_epi32(v_1, _mm256_mullo_epi32(xq0, f1_1));
                }

                if (params->r[1] > 0) {
                    const __m256i f2_0 = _mm256_sub_epi32(yy_loadu_256(&flt1[j + 0]), u_0);
                    const __m256i f2_1 = _mm256_sub_epi32(yy_loadu_256(&flt1[j + 8]), u_1);
                    v_0                = _mm256_add_epi32(v_0, _mm256_mullo_epi32(xq1, f2_0));
                    v_1                = _mm256_add_epi32(v_1, _mm256_mullo_epi32(xq1, f2_1));
                }

                const __m256i w_0 = _mm256_srai_epi32(_mm256_add_epi32(v_0, rounding),
                                                      SGRPROJ_PRJ_BITS + SGRPROJ_RST_BITS);
                const __m256i w_1 = _mm256_srai_epi32(_mm256_add_epi32(v_1, rounding),
                                                      SGRPROJ_PRJ_BITS + SGRPROJ_RST_BITS);

                // Pack into 8 bits and clamp to [0, 256)
                // Note that each pack messes up the order of the bits,
                // so we use a permute function to correct this
                // 0, 1, 4, 5, 2, 3, 6, 7
                const __m256i tmp = _mm256_packus_epi32(w_0, w_1);
                // 0, 1, 4, 5, 2, 3, 6, 7, 0, 1, 4, 5, 2, 3, 6, 7
                const __m256i tmp2 = _mm256_packus_epi16(tmp, tmp);
                // 0, 1, 2, 3, 4, 5, 6, 7, ...
                const __m256i tmp3 = _mm256_permutevar8x32_epi32(tmp2, idx);
                const __m128i res  = _mm256_castsi256_si128(tmp3);
                xx_storeu_128(dst8 + j, res);
                j += 16;
            } while (j < width);

            dat8 += stride;
            flt0 += width;
            flt1 += width;
            dst8 += dst_stride;
        } while (--i);
    } else {
        const __m256i   max   = _mm256_set1_epi16((1 << bit_depth) - 1);
        const uint16_t* dat16 = CONVERT_TO_SHORTPTR(dat8);
        uint16_t*       dst16 = CONVERT_TO_SHORTPTR(dst8);

        do {
            // Calculate output in batches of 16 pixels
            int32_t j = 0;
            do {
                const __m128i src_0 = xx_loadu_128(dat16 + j + 0);
                const __m128i src_1 = xx_loadu_128(dat16 + j + 8);
                const __m256i ep_0  = _mm256_cvtepu16_epi32(src_0);
                const __m256i ep_1  = _mm256_cvtepu16_epi32(src_1);
                const __m256i u_0   = _mm256_slli_epi32(ep_0, SGRPROJ_RST_BITS);
                const __m256i u_1   = _mm256_slli_epi32(ep_1, SGRPROJ_RST_BITS);
                __m256i       v_0   = _mm256_slli_epi32(u_0, SGRPROJ_PRJ_BITS);
                __m256i       v_1   = _mm256_slli_epi32(u_1, SGRPROJ_PRJ_BITS);

                if (params->r[0] > 0) {
                    const __m256i f1_0 = _mm256_sub_epi32(yy_loadu_256(&flt0[j + 0]), u_0);
                    const __m256i f1_1 = _mm256_sub_epi32(yy_loadu_256(&flt0[j + 8]), u_1);
                    v_0                = _mm256_add_epi32(v_0, _mm256_mullo_epi32(xq0, f1_0));
                    v_1                = _mm256_add_epi32(v_1, _mm256_mullo_epi32(xq0, f1_1));
                }

                if (params->r[1] > 0) {
                    const __m256i f2_0 = _mm256_sub_epi32(yy_loadu_256(&flt1[j + 0]), u_0);
                    const __m256i f2_1 = _mm256_sub_epi32(yy_loadu_256(&flt1[j + 8]), u_1);
                    v_0                = _mm256_add_epi32(v_0, _mm256_mullo_epi32(xq1, f2_0));
                    v_1                = _mm256_add_epi32(v_1, _mm256_mullo_epi32(xq1, f2_1));
                }

                const __m256i w_0 = _mm256_srai_epi32(_mm256_add_epi32(v_0, rounding),
                                                      SGRPROJ_PRJ_BITS + SGRPROJ_RST_BITS);
                const __m256i w_1 = _mm256_srai_epi32(_mm256_add_epi32(v_1, rounding),
                                                      SGRPROJ_PRJ_BITS + SGRPROJ_RST_BITS);

                // Pack into 16 bits and clamp to [0, 2^bit_depth)
                // Note that packing into 16 bits messes up the order of the bits,
                // so we use a permute function to correct this
                const __m256i tmp  = _mm256_packus_epi32(w_0, w_1);
                const __m256i tmp2 = _mm256_permute4x64_epi64(tmp, 0xd8);
                const __m256i res  = _mm256_min_epi16(tmp2, max);
                yy_storeu_256(dst16 + j, res);
                j += 16;
            } while (j < width);

            dat16 += stride;
            flt0 += width;
            flt1 += width;
            dst16 += dst_stride;
        } while (--i);
    }
}
