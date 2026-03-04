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
#include <smmintrin.h>

#include "common_dsp_rtcd.h"
#include "warped_motion.h"

extern int8_t eb_av1_filter_8bit[WARPEDPIXEL_PREC_SHIFTS * 3 + 1][8];

// Shuffle masks: we want to convert a sequence of bytes 0, 1, 2, ..., 15
// in an SSE register into two sequences:
// 0, 2, 2, 4, ..., 12, 12, 14, <don't care>
// 1, 3, 3, 5, ..., 13, 13, 15, <don't care>
DECLARE_ALIGNED(16, static const uint8_t, even_mask[16]) = {0, 2, 2, 4, 4, 6, 6, 8, 8, 10, 10, 12, 12, 14, 14, 0};

DECLARE_ALIGNED(16, static const uint8_t, odd_mask[16]) = {1, 3, 3, 5, 5, 7, 7, 9, 9, 11, 11, 13, 13, 15, 15, 0};

DECLARE_ALIGNED(16, static const uint8_t, shuffle_alpha0_mask01[16]) = {0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1};

DECLARE_ALIGNED(16, static const uint8_t, shuffle_alpha0_mask23[16]) = {2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3, 2, 3};

DECLARE_ALIGNED(16, static const uint8_t, shuffle_alpha0_mask45[16]) = {4, 5, 4, 5, 4, 5, 4, 5, 4, 5, 4, 5, 4, 5, 4, 5};

DECLARE_ALIGNED(16, static const uint8_t, shuffle_alpha0_mask67[16]) = {6, 7, 6, 7, 6, 7, 6, 7, 6, 7, 6, 7, 6, 7, 6, 7};

DECLARE_ALIGNED(16, static const uint8_t, shuffle_gamma0_mask0[16]) = {0, 1, 2, 3, 0, 1, 2, 3, 0, 1, 2, 3, 0, 1, 2, 3};

DECLARE_ALIGNED(16, static const uint8_t, shuffle_gamma0_mask1[16]) = {4, 5, 6, 7, 4, 5, 6, 7, 4, 5, 6, 7, 4, 5, 6, 7};

DECLARE_ALIGNED(16, static const uint8_t, shuffle_gamma0_mask2[16]) = {
    8, 9, 10, 11, 8, 9, 10, 11, 8, 9, 10, 11, 8, 9, 10, 11};

DECLARE_ALIGNED(16, static const uint8_t, shuffle_gamma0_mask3[16]) = {
    12, 13, 14, 15, 12, 13, 14, 15, 12, 13, 14, 15, 12, 13, 14, 15};

static const uint8_t warp_highbd_arrange_bytes[16] = {0, 2, 4, 6, 8, 10, 12, 14, 1, 3, 5, 7, 9, 11, 13, 15};

static const uint8_t highbd_shuffle_alpha0_mask0[16] = {0, 1, 2, 3, 0, 1, 2, 3, 0, 1, 2, 3, 0, 1, 2, 3};
static const uint8_t highbd_shuffle_alpha0_mask1[16] = {4, 5, 6, 7, 4, 5, 6, 7, 4, 5, 6, 7, 4, 5, 6, 7};
static const uint8_t highbd_shuffle_alpha0_mask2[16] = {8, 9, 10, 11, 8, 9, 10, 11, 8, 9, 10, 11, 8, 9, 10, 11};
static const uint8_t highbd_shuffle_alpha0_mask3[16] = {12, 13, 14, 15, 12, 13, 14, 15, 12, 13, 14, 15, 12, 13, 14, 15};

static INLINE void svt_filter_src_pixels(__m128i src, __m128i* tmp, __m128i* coeff, const int offset_bits_horiz,
                                         const int reduce_bits_horiz, int k) {
    const __m128i src_even = _mm_shuffle_epi8(src, _mm_loadu_si128((__m128i*)even_mask));
    const __m128i src_odd  = _mm_shuffle_epi8(src, _mm_loadu_si128((__m128i*)odd_mask));
    // The pixel order we need for 'src' is:
    // 0 2 2 4 4 6 6 8 1 3 3 5 5 7 7 9
    const __m128i src_02 = _mm_unpacklo_epi64(src_even, src_odd);
    const __m128i res_02 = _mm_maddubs_epi16(src_02, coeff[0]);
    // 4 6 6 8 8 10 10 12 5 7 7 9 9 11 11 13
    const __m128i src_46 = _mm_unpacklo_epi64(_mm_srli_si128(src_even, 4), _mm_srli_si128(src_odd, 4));
    const __m128i res_46 = _mm_maddubs_epi16(src_46, coeff[1]);
    // 1 3 3 5 5 7 7 9 2 4 4 6 6 8 8 10
    const __m128i src_13 = _mm_unpacklo_epi64(src_odd, _mm_srli_si128(src_even, 2));
    const __m128i res_13 = _mm_maddubs_epi16(src_13, coeff[2]);
    // 5 7 7 9 9 11 11 13 6 8 8 10 10 12 12 14
    const __m128i src_57 = _mm_unpacklo_epi64(_mm_srli_si128(src_odd, 4), _mm_srli_si128(src_even, 6));
    const __m128i res_57 = _mm_maddubs_epi16(src_57, coeff[3]);

    const __m128i round_const = _mm_set1_epi16((1 << offset_bits_horiz) + ((1 << reduce_bits_horiz) >> 1));

    // Note: The values res_02 + res_46 and res_13 + res_57 both
    // fit into int16s at this point, but their sum may be too wide to fit
    // into an int16. However, once we also add round_const, the sum of
    // all of these fits into a uint16.
    //
    // The wrapping behaviour of _mm_add_* is used here to make sure we
    // get the correct result despite converting between different
    // (implicit) types.
    const __m128i res_even = _mm_add_epi16(res_02, res_46);
    const __m128i res_odd  = _mm_add_epi16(res_13, res_57);
    const __m128i res      = _mm_add_epi16(_mm_add_epi16(res_even, res_odd), round_const);
    tmp[k + 7]             = _mm_srl_epi16(res, _mm_cvtsi32_si128(reduce_bits_horiz));
}

static INLINE void svt_prepare_horizontal_filter_coeff(int alpha, int sx, __m128i* coeff) {
    // Filter even-index pixels
    const __m128i tmp_0 = _mm_loadl_epi64((__m128i*)&eb_av1_filter_8bit[(sx + 0 * alpha) >> WARPEDDIFF_PREC_BITS]);
    const __m128i tmp_1 = _mm_loadl_epi64((__m128i*)&eb_av1_filter_8bit[(sx + 1 * alpha) >> WARPEDDIFF_PREC_BITS]);
    const __m128i tmp_2 = _mm_loadl_epi64((__m128i*)&eb_av1_filter_8bit[(sx + 2 * alpha) >> WARPEDDIFF_PREC_BITS]);
    const __m128i tmp_3 = _mm_loadl_epi64((__m128i*)&eb_av1_filter_8bit[(sx + 3 * alpha) >> WARPEDDIFF_PREC_BITS]);
    const __m128i tmp_4 = _mm_loadl_epi64((__m128i*)&eb_av1_filter_8bit[(sx + 4 * alpha) >> WARPEDDIFF_PREC_BITS]);
    const __m128i tmp_5 = _mm_loadl_epi64((__m128i*)&eb_av1_filter_8bit[(sx + 5 * alpha) >> WARPEDDIFF_PREC_BITS]);
    const __m128i tmp_6 = _mm_loadl_epi64((__m128i*)&eb_av1_filter_8bit[(sx + 6 * alpha) >> WARPEDDIFF_PREC_BITS]);
    const __m128i tmp_7 = _mm_loadl_epi64((__m128i*)&eb_av1_filter_8bit[(sx + 7 * alpha) >> WARPEDDIFF_PREC_BITS]);

    // Coeffs 0 2 0 2 4 6 4 6 1 3 1 3 5 7 5 7 for pixels 0 2
    const __m128i tmp_8 = _mm_unpacklo_epi16(tmp_0, tmp_2);
    // Coeffs 0 2 0 2 4 6 4 6 1 3 1 3 5 7 5 7 for pixels 1 3
    const __m128i tmp_9 = _mm_unpacklo_epi16(tmp_1, tmp_3);
    // Coeffs 0 2 0 2 4 6 4 6 1 3 1 3 5 7 5 7 for pixels 4 6
    const __m128i tmp_10 = _mm_unpacklo_epi16(tmp_4, tmp_6);
    // Coeffs 0 2 0 2 4 6 4 6 1 3 1 3 5 7 5 7 for pixels 5 7
    const __m128i tmp_11 = _mm_unpacklo_epi16(tmp_5, tmp_7);

    // Coeffs 0 2 0 2 0 2 0 2 4 6 4 6 4 6 4 6 for pixels 0 2 4 6
    const __m128i tmp_12 = _mm_unpacklo_epi32(tmp_8, tmp_10);
    // Coeffs 1 3 1 3 1 3 1 3 5 7 5 7 5 7 5 7 for pixels 0 2 4 6
    const __m128i tmp_13 = _mm_unpackhi_epi32(tmp_8, tmp_10);
    // Coeffs 0 2 0 2 0 2 0 2 4 6 4 6 4 6 4 6 for pixels 1 3 5 7
    const __m128i tmp_14 = _mm_unpacklo_epi32(tmp_9, tmp_11);
    // Coeffs 1 3 1 3 1 3 1 3 5 7 5 7 5 7 5 7 for pixels 1 3 5 7
    const __m128i tmp_15 = _mm_unpackhi_epi32(tmp_9, tmp_11);

    // Coeffs 0 2 for pixels 0 2 4 6 1 3 5 7
    coeff[0] = _mm_unpacklo_epi64(tmp_12, tmp_14);
    // Coeffs 4 6 for pixels 0 2 4 6 1 3 5 7
    coeff[1] = _mm_unpackhi_epi64(tmp_12, tmp_14);
    // Coeffs 1 3 for pixels 0 2 4 6 1 3 5 7
    coeff[2] = _mm_unpacklo_epi64(tmp_13, tmp_15);
    // Coeffs 5 7 for pixels 0 2 4 6 1 3 5 7
    coeff[3] = _mm_unpackhi_epi64(tmp_13, tmp_15);
}

static INLINE void svt_prepare_horizontal_filter_coeff_alpha0(int sx, __m128i* coeff) {
    // Filter even-index pixels
    const __m128i tmp_0 = _mm_loadl_epi64((__m128i*)&eb_av1_filter_8bit[sx >> WARPEDDIFF_PREC_BITS]);

    // Coeffs 0 2 for pixels 0 2 4 6 1 3 5 7
    coeff[0] = _mm_shuffle_epi8(tmp_0, _mm_loadu_si128((__m128i*)shuffle_alpha0_mask01));
    // Coeffs 4 6 for pixels 0 2 4 6 1 3 5 7
    coeff[1] = _mm_shuffle_epi8(tmp_0, _mm_loadu_si128((__m128i*)shuffle_alpha0_mask23));
    // Coeffs 1 3 for pixels 0 2 4 6 1 3 5 7
    coeff[2] = _mm_shuffle_epi8(tmp_0, _mm_loadu_si128((__m128i*)shuffle_alpha0_mask45));
    // Coeffs 5 7 for pixels 0 2 4 6 1 3 5 7
    coeff[3] = _mm_shuffle_epi8(tmp_0, _mm_loadu_si128((__m128i*)shuffle_alpha0_mask67));
}

static INLINE void svt_horizontal_filter(__m128i src, __m128i* tmp, int sx, int alpha, int k,
                                         const int offset_bits_horiz, const int reduce_bits_horiz) {
    __m128i coeff[4];
    svt_prepare_horizontal_filter_coeff(alpha, sx, coeff);
    svt_filter_src_pixels(src, tmp, coeff, offset_bits_horiz, reduce_bits_horiz, k);
}

static INLINE void svt_warp_horizontal_filter(const uint8_t* ref, __m128i* tmp, int stride, int32_t ix4, int32_t iy4,
                                              int32_t sx4, int alpha, int beta, int p_height, int height, int i,
                                              const int offset_bits_horiz, const int reduce_bits_horiz) {
    int k;
    for (k = -7; k < AOMMIN(8, p_height - i); ++k) {
        int iy = iy4 + k;
        if (iy < 0) {
            iy = 0;
        } else if (iy > height - 1) {
            iy = height - 1;
        }
        int sx = sx4 + beta * (k + 4);

        // Load source pixels
        const __m128i src = _mm_loadu_si128((__m128i*)(ref + iy * stride + ix4 - 7));
        svt_horizontal_filter(src, tmp, sx, alpha, k, offset_bits_horiz, reduce_bits_horiz);
    }
}

static INLINE void svt_warp_horizontal_filter_alpha0(const uint8_t* ref, __m128i* tmp, int stride, int32_t ix4,
                                                     int32_t iy4, int32_t sx4, int alpha, int beta, int p_height,
                                                     int height, int i, const int offset_bits_horiz,
                                                     const int reduce_bits_horiz) {
    (void)alpha;
    int k;
    for (k = -7; k < AOMMIN(8, p_height - i); ++k) {
        int iy = iy4 + k;
        if (iy < 0) {
            iy = 0;
        } else if (iy > height - 1) {
            iy = height - 1;
        }
        int sx = sx4 + beta * (k + 4);

        // Load source pixels
        const __m128i src = _mm_loadu_si128((__m128i*)(ref + iy * stride + ix4 - 7));

        __m128i coeff[4];
        svt_prepare_horizontal_filter_coeff_alpha0(sx, coeff);
        svt_filter_src_pixels(src, tmp, coeff, offset_bits_horiz, reduce_bits_horiz, k);
    }
}

static INLINE void svt_warp_horizontal_filter_beta0(const uint8_t* ref, __m128i* tmp, int stride, int32_t ix4,
                                                    int32_t iy4, int32_t sx4, int alpha, int beta, int p_height,
                                                    int height, int i, const int offset_bits_horiz,
                                                    const int reduce_bits_horiz) {
    (void)beta;
    int     k;
    __m128i coeff[4];
    svt_prepare_horizontal_filter_coeff(alpha, sx4, coeff);

    for (k = -7; k < AOMMIN(8, p_height - i); ++k) {
        int iy = iy4 + k;
        if (iy < 0) {
            iy = 0;
        } else if (iy > height - 1) {
            iy = height - 1;
        }

        // Load source pixels
        const __m128i src = _mm_loadu_si128((__m128i*)(ref + iy * stride + ix4 - 7));
        svt_filter_src_pixels(src, tmp, coeff, offset_bits_horiz, reduce_bits_horiz, k);
    }
}

static INLINE void svt_warp_horizontal_filter_alpha0_beta0(const uint8_t* ref, __m128i* tmp, int stride, int32_t ix4,
                                                           int32_t iy4, int32_t sx4, int alpha, int beta, int p_height,
                                                           int height, int i, const int offset_bits_horiz,
                                                           const int reduce_bits_horiz) {
    (void)beta;
    (void)alpha;
    int k;

    __m128i coeff[4];
    svt_prepare_horizontal_filter_coeff_alpha0(sx4, coeff);

    for (k = -7; k < AOMMIN(8, p_height - i); ++k) {
        int iy = iy4 + k;
        if (iy < 0) {
            iy = 0;
        } else if (iy > height - 1) {
            iy = height - 1;
        }

        // Load source pixels
        const __m128i src = _mm_loadu_si128((__m128i*)(ref + iy * stride + ix4 - 7));
        svt_filter_src_pixels(src, tmp, coeff, offset_bits_horiz, reduce_bits_horiz, k);
    }
}

static INLINE void svt_unpack_weights_and_set_round_const(ConvolveParams* conv_params, const int round_bits,
                                                          const int offset_bits, __m128i* res_sub_const,
                                                          __m128i* round_bits_const, __m128i* wt) {
    *res_sub_const    = _mm_set1_epi16(-(1 << (offset_bits - conv_params->round_1)) -
                                    (1 << (offset_bits - conv_params->round_1 - 1)));
    *round_bits_const = _mm_set1_epi16(((1 << round_bits) >> 1));

    const int     w0  = conv_params->fwd_offset;
    const int     w1  = conv_params->bck_offset;
    const __m128i wt0 = _mm_set1_epi16((int16_t)w0);
    const __m128i wt1 = _mm_set1_epi16((int16_t)w1);
    *wt               = _mm_unpacklo_epi16(wt0, wt1);
}

static INLINE void svt_prepare_vertical_filter_coeffs(int gamma, int sy, __m128i* coeffs) {
    const __m128i tmp_0 = _mm_loadu_si128(
        (__m128i*)(svt_aom_warped_filter + ((sy + 0 * gamma) >> WARPEDDIFF_PREC_BITS)));
    const __m128i tmp_2 = _mm_loadu_si128(
        (__m128i*)(svt_aom_warped_filter + ((sy + 2 * gamma) >> WARPEDDIFF_PREC_BITS)));
    const __m128i tmp_4 = _mm_loadu_si128(
        (__m128i*)(svt_aom_warped_filter + ((sy + 4 * gamma) >> WARPEDDIFF_PREC_BITS)));
    const __m128i tmp_6 = _mm_loadu_si128(
        (__m128i*)(svt_aom_warped_filter + ((sy + 6 * gamma) >> WARPEDDIFF_PREC_BITS)));

    const __m128i tmp_8  = _mm_unpacklo_epi32(tmp_0, tmp_2);
    const __m128i tmp_10 = _mm_unpacklo_epi32(tmp_4, tmp_6);
    const __m128i tmp_12 = _mm_unpackhi_epi32(tmp_0, tmp_2);
    const __m128i tmp_14 = _mm_unpackhi_epi32(tmp_4, tmp_6);

    // even coeffs
    coeffs[0] = _mm_unpacklo_epi64(tmp_8, tmp_10);
    coeffs[1] = _mm_unpackhi_epi64(tmp_8, tmp_10);
    coeffs[2] = _mm_unpacklo_epi64(tmp_12, tmp_14);
    coeffs[3] = _mm_unpackhi_epi64(tmp_12, tmp_14);

    const __m128i tmp_1 = _mm_loadu_si128(
        (__m128i*)(svt_aom_warped_filter + ((sy + 1 * gamma) >> WARPEDDIFF_PREC_BITS)));
    const __m128i tmp_3 = _mm_loadu_si128(
        (__m128i*)(svt_aom_warped_filter + ((sy + 3 * gamma) >> WARPEDDIFF_PREC_BITS)));
    const __m128i tmp_5 = _mm_loadu_si128(
        (__m128i*)(svt_aom_warped_filter + ((sy + 5 * gamma) >> WARPEDDIFF_PREC_BITS)));
    const __m128i tmp_7 = _mm_loadu_si128(
        (__m128i*)(svt_aom_warped_filter + ((sy + 7 * gamma) >> WARPEDDIFF_PREC_BITS)));

    const __m128i tmp_9  = _mm_unpacklo_epi32(tmp_1, tmp_3);
    const __m128i tmp_11 = _mm_unpacklo_epi32(tmp_5, tmp_7);
    const __m128i tmp_13 = _mm_unpackhi_epi32(tmp_1, tmp_3);
    const __m128i tmp_15 = _mm_unpackhi_epi32(tmp_5, tmp_7);

    // odd coeffs
    coeffs[4] = _mm_unpacklo_epi64(tmp_9, tmp_11);
    coeffs[5] = _mm_unpackhi_epi64(tmp_9, tmp_11);
    coeffs[6] = _mm_unpacklo_epi64(tmp_13, tmp_15);
    coeffs[7] = _mm_unpackhi_epi64(tmp_13, tmp_15);
}

static INLINE void svt_prepare_vertical_filter_coeffs_gamma0(int sy, __m128i* coeffs) {
    const __m128i tmp_0 = _mm_loadu_si128((__m128i*)(svt_aom_warped_filter + (sy >> WARPEDDIFF_PREC_BITS)));

    // even coeffs
    coeffs[0] = _mm_shuffle_epi8(tmp_0, _mm_loadu_si128((__m128i*)shuffle_gamma0_mask0));
    coeffs[1] = _mm_shuffle_epi8(tmp_0, _mm_loadu_si128((__m128i*)shuffle_gamma0_mask1));
    coeffs[2] = _mm_shuffle_epi8(tmp_0, _mm_loadu_si128((__m128i*)shuffle_gamma0_mask2));
    coeffs[3] = _mm_shuffle_epi8(tmp_0, _mm_loadu_si128((__m128i*)shuffle_gamma0_mask3));

    // odd coeffs
    coeffs[4] = coeffs[0];
    coeffs[5] = coeffs[1];
    coeffs[6] = coeffs[2];
    coeffs[7] = coeffs[3];
}

static INLINE void svt_filter_src_pixels_vertical(__m128i* tmp, __m128i* coeffs, __m128i* res_lo, __m128i* res_hi,
                                                  int k) {
    // Load from tmp and rearrange pairs of consecutive rows into the
    // column order 0 0 2 2 4 4 6 6; 1 1 3 3 5 5 7 7
    const __m128i* src   = tmp + (k + 4);
    const __m128i  src_0 = _mm_unpacklo_epi16(src[0], src[1]);
    const __m128i  src_2 = _mm_unpacklo_epi16(src[2], src[3]);
    const __m128i  src_4 = _mm_unpacklo_epi16(src[4], src[5]);
    const __m128i  src_6 = _mm_unpacklo_epi16(src[6], src[7]);

    const __m128i res_0 = _mm_madd_epi16(src_0, coeffs[0]);
    const __m128i res_2 = _mm_madd_epi16(src_2, coeffs[1]);
    const __m128i res_4 = _mm_madd_epi16(src_4, coeffs[2]);
    const __m128i res_6 = _mm_madd_epi16(src_6, coeffs[3]);

    const __m128i res_even = _mm_add_epi32(_mm_add_epi32(res_0, res_2), _mm_add_epi32(res_4, res_6));

    // Filter odd-index pixels
    const __m128i src_1 = _mm_unpackhi_epi16(src[0], src[1]);
    const __m128i src_3 = _mm_unpackhi_epi16(src[2], src[3]);
    const __m128i src_5 = _mm_unpackhi_epi16(src[4], src[5]);
    const __m128i src_7 = _mm_unpackhi_epi16(src[6], src[7]);

    const __m128i res_1 = _mm_madd_epi16(src_1, coeffs[4]);
    const __m128i res_3 = _mm_madd_epi16(src_3, coeffs[5]);
    const __m128i res_5 = _mm_madd_epi16(src_5, coeffs[6]);
    const __m128i res_7 = _mm_madd_epi16(src_7, coeffs[7]);

    const __m128i res_odd = _mm_add_epi32(_mm_add_epi32(res_1, res_3), _mm_add_epi32(res_5, res_7));

    // Rearrange pixels back into the order 0 ... 7
    *res_lo = _mm_unpacklo_epi32(res_even, res_odd);
    *res_hi = _mm_unpackhi_epi32(res_even, res_odd);
}

static INLINE void svt_store_vertical_filter_output(__m128i* res_lo, __m128i* res_hi, const __m128i* res_add_const,
                                                    const __m128i* wt, const __m128i* res_sub_const,
                                                    __m128i* round_bits_const, uint8_t* pred,
                                                    ConvolveParams* conv_params, int i, int j, int k,
                                                    const int reduce_bits_vert, int p_stride, int p_width,
                                                    const int round_bits) {
    __m128i res_lo_1 = *res_lo;
    __m128i res_hi_1 = *res_hi;

    if (conv_params->is_compound) {
        __m128i* const p         = (__m128i*)&conv_params->dst[(i + k + 4) * conv_params->dst_stride + j];
        res_lo_1                 = _mm_srai_epi32(_mm_add_epi32(res_lo_1, *res_add_const), reduce_bits_vert);
        const __m128i temp_lo_16 = _mm_packus_epi32(res_lo_1, res_lo_1);
        __m128i       res_lo_16;
        if (conv_params->do_average) {
            __m128i* const dst8 = (__m128i*)&pred[(i + k + 4) * p_stride + j];
            const __m128i  p_16 = _mm_loadl_epi64(p);

            if (conv_params->use_jnt_comp_avg) {
                const __m128i p_16_lo    = _mm_unpacklo_epi16(p_16, temp_lo_16);
                const __m128i wt_res_lo  = _mm_madd_epi16(p_16_lo, *wt);
                const __m128i shifted_32 = _mm_srai_epi32(wt_res_lo, DIST_PRECISION_BITS);
                res_lo_16                = _mm_packus_epi32(shifted_32, shifted_32);
            } else {
                res_lo_16 = _mm_srai_epi16(_mm_add_epi16(p_16, temp_lo_16), 1);
            }

            res_lo_16 = _mm_add_epi16(res_lo_16, *res_sub_const);

            res_lo_16        = _mm_srai_epi16(_mm_add_epi16(res_lo_16, *round_bits_const), round_bits);
            __m128i res_8_lo = _mm_packus_epi16(res_lo_16, res_lo_16);
            *(uint32_t*)dst8 = _mm_cvtsi128_si32(res_8_lo);
        } else {
            _mm_storel_epi64(p, temp_lo_16);
        }
        if (p_width > 4) {
            __m128i* const p4        = (__m128i*)&conv_params->dst[(i + k + 4) * conv_params->dst_stride + j + 4];
            res_hi_1                 = _mm_srai_epi32(_mm_add_epi32(res_hi_1, *res_add_const), reduce_bits_vert);
            const __m128i temp_hi_16 = _mm_packus_epi32(res_hi_1, res_hi_1);
            __m128i       res_hi_16;

            if (conv_params->do_average) {
                __m128i* const dst8_4 = (__m128i*)&pred[(i + k + 4) * p_stride + j + 4];
                const __m128i  p4_16  = _mm_loadl_epi64(p4);

                if (conv_params->use_jnt_comp_avg) {
                    const __m128i p_16_hi    = _mm_unpacklo_epi16(p4_16, temp_hi_16);
                    const __m128i wt_res_hi  = _mm_madd_epi16(p_16_hi, *wt);
                    const __m128i shifted_32 = _mm_srai_epi32(wt_res_hi, DIST_PRECISION_BITS);
                    res_hi_16                = _mm_packus_epi32(shifted_32, shifted_32);
                } else {
                    res_hi_16 = _mm_srai_epi16(_mm_add_epi16(p4_16, temp_hi_16), 1);
                }
                res_hi_16 = _mm_add_epi16(res_hi_16, *res_sub_const);

                res_hi_16          = _mm_srai_epi16(_mm_add_epi16(res_hi_16, *round_bits_const), round_bits);
                __m128i res_8_hi   = _mm_packus_epi16(res_hi_16, res_hi_16);
                *(uint32_t*)dst8_4 = _mm_cvtsi128_si32(res_8_hi);

            } else {
                _mm_storel_epi64(p4, temp_hi_16);
            }
        }
    } else {
        const __m128i res_lo_round = _mm_srai_epi32(_mm_add_epi32(res_lo_1, *res_add_const), reduce_bits_vert);
        const __m128i res_hi_round = _mm_srai_epi32(_mm_add_epi32(res_hi_1, *res_add_const), reduce_bits_vert);

        const __m128i res_16bit = _mm_packs_epi32(res_lo_round, res_hi_round);
        __m128i       res_8bit  = _mm_packus_epi16(res_16bit, res_16bit);

        // Store, blending with 'pred' if needed
        __m128i* const p = (__m128i*)&pred[(i + k + 4) * p_stride + j];

        // Note: If we're outputting a 4x4 block, we need to be very careful
        // to only output 4 pixels at this point, to avoid encode/decode
        // mismatches when encoding with multiple threads.
        if (p_width == 4) {
            *(uint32_t*)p = _mm_cvtsi128_si32(res_8bit);
        } else {
            _mm_storel_epi64(p, res_8bit);
        }
    }
}

static INLINE void svt_warp_vertical_filter(uint8_t* pred, __m128i* tmp, ConvolveParams* conv_params, int16_t gamma,
                                            int16_t delta, int p_height, int p_stride, int p_width, int i, int j,
                                            int sy4, const int reduce_bits_vert, const __m128i* res_add_const,
                                            const int round_bits, const int offset_bits) {
    int     k;
    __m128i res_sub_const, round_bits_const, wt;
    svt_unpack_weights_and_set_round_const(
        conv_params, round_bits, offset_bits, &res_sub_const, &round_bits_const, &wt);
    // Vertical filter
    for (k = -4; k < AOMMIN(4, p_height - i - 4); ++k) {
        int sy = sy4 + delta * (k + 4);

        __m128i coeffs[8];
        svt_prepare_vertical_filter_coeffs(gamma, sy, coeffs);

        __m128i res_lo;
        __m128i res_hi;
        svt_filter_src_pixels_vertical(tmp, coeffs, &res_lo, &res_hi, k);

        svt_store_vertical_filter_output(&res_lo,
                                         &res_hi,
                                         res_add_const,
                                         &wt,
                                         &res_sub_const,
                                         &round_bits_const,
                                         pred,
                                         conv_params,
                                         i,
                                         j,
                                         k,
                                         reduce_bits_vert,
                                         p_stride,
                                         p_width,
                                         round_bits);
    }
}

static INLINE void svt_warp_vertical_filter_gamma0(uint8_t* pred, __m128i* tmp, ConvolveParams* conv_params,
                                                   int16_t gamma, int16_t delta, int p_height, int p_stride,
                                                   int p_width, int i, int j, int sy4, const int reduce_bits_vert,
                                                   const __m128i* res_add_const, const int round_bits,
                                                   const int offset_bits) {
    int k;
    (void)gamma;
    __m128i res_sub_const, round_bits_const, wt;
    svt_unpack_weights_and_set_round_const(
        conv_params, round_bits, offset_bits, &res_sub_const, &round_bits_const, &wt);
    // Vertical filter
    for (k = -4; k < AOMMIN(4, p_height - i - 4); ++k) {
        int sy = sy4 + delta * (k + 4);

        __m128i coeffs[8];
        svt_prepare_vertical_filter_coeffs_gamma0(sy, coeffs);

        __m128i res_lo;
        __m128i res_hi;
        svt_filter_src_pixels_vertical(tmp, coeffs, &res_lo, &res_hi, k);

        svt_store_vertical_filter_output(&res_lo,
                                         &res_hi,
                                         res_add_const,
                                         &wt,
                                         &res_sub_const,
                                         &round_bits_const,
                                         pred,
                                         conv_params,
                                         i,
                                         j,
                                         k,
                                         reduce_bits_vert,
                                         p_stride,
                                         p_width,
                                         round_bits);
    }
}

static INLINE void svt_warp_vertical_filter_delta0(uint8_t* pred, __m128i* tmp, ConvolveParams* conv_params,
                                                   int16_t gamma, int16_t delta, int p_height, int p_stride,
                                                   int p_width, int i, int j, int sy4, const int reduce_bits_vert,
                                                   const __m128i* res_add_const, const int round_bits,
                                                   const int offset_bits) {
    (void)delta;
    int     k;
    __m128i res_sub_const, round_bits_const, wt;
    svt_unpack_weights_and_set_round_const(
        conv_params, round_bits, offset_bits, &res_sub_const, &round_bits_const, &wt);

    __m128i coeffs[8];
    svt_prepare_vertical_filter_coeffs(gamma, sy4, coeffs);
    // Vertical filter
    for (k = -4; k < AOMMIN(4, p_height - i - 4); ++k) {
        __m128i res_lo;
        __m128i res_hi;
        svt_filter_src_pixels_vertical(tmp, coeffs, &res_lo, &res_hi, k);

        svt_store_vertical_filter_output(&res_lo,
                                         &res_hi,
                                         res_add_const,
                                         &wt,
                                         &res_sub_const,
                                         &round_bits_const,
                                         pred,
                                         conv_params,
                                         i,
                                         j,
                                         k,
                                         reduce_bits_vert,
                                         p_stride,
                                         p_width,
                                         round_bits);
    }
}

static INLINE void svt_warp_vertical_filter_gamma0_delta0(uint8_t* pred, __m128i* tmp, ConvolveParams* conv_params,
                                                          int16_t gamma, int16_t delta, int p_height, int p_stride,
                                                          int p_width, int i, int j, int sy4,
                                                          const int reduce_bits_vert, const __m128i* res_add_const,
                                                          const int round_bits, const int offset_bits) {
    (void)delta;
    (void)gamma;
    int     k;
    __m128i res_sub_const, round_bits_const, wt;
    svt_unpack_weights_and_set_round_const(
        conv_params, round_bits, offset_bits, &res_sub_const, &round_bits_const, &wt);

    __m128i coeffs[8];
    svt_prepare_vertical_filter_coeffs_gamma0(sy4, coeffs);
    // Vertical filter
    for (k = -4; k < AOMMIN(4, p_height - i - 4); ++k) {
        __m128i res_lo;
        __m128i res_hi;
        svt_filter_src_pixels_vertical(tmp, coeffs, &res_lo, &res_hi, k);

        svt_store_vertical_filter_output(&res_lo,
                                         &res_hi,
                                         res_add_const,
                                         &wt,
                                         &res_sub_const,
                                         &round_bits_const,
                                         pred,
                                         conv_params,
                                         i,
                                         j,
                                         k,
                                         reduce_bits_vert,
                                         p_stride,
                                         p_width,
                                         round_bits);
    }
}

static INLINE void svt_prepare_warp_vertical_filter(uint8_t* pred, __m128i* tmp, ConvolveParams* conv_params,
                                                    int16_t gamma, int16_t delta, int p_height, int p_stride,
                                                    int p_width, int i, int j, int sy4, const int reduce_bits_vert,
                                                    const __m128i* res_add_const, const int round_bits,
                                                    const int offset_bits) {
    if (gamma == 0 && delta == 0) {
        svt_warp_vertical_filter_gamma0_delta0(pred,
                                               tmp,
                                               conv_params,
                                               gamma,
                                               delta,
                                               p_height,
                                               p_stride,
                                               p_width,
                                               i,
                                               j,
                                               sy4,
                                               reduce_bits_vert,
                                               res_add_const,
                                               round_bits,
                                               offset_bits);
    } else if (gamma == 0 && delta != 0) {
        svt_warp_vertical_filter_gamma0(pred,
                                        tmp,
                                        conv_params,
                                        gamma,
                                        delta,
                                        p_height,
                                        p_stride,
                                        p_width,
                                        i,
                                        j,
                                        sy4,
                                        reduce_bits_vert,
                                        res_add_const,
                                        round_bits,
                                        offset_bits);
    } else if (gamma != 0 && delta == 0) {
        svt_warp_vertical_filter_delta0(pred,
                                        tmp,
                                        conv_params,
                                        gamma,
                                        delta,
                                        p_height,
                                        p_stride,
                                        p_width,
                                        i,
                                        j,
                                        sy4,
                                        reduce_bits_vert,
                                        res_add_const,
                                        round_bits,
                                        offset_bits);
    } else {
        svt_warp_vertical_filter(pred,
                                 tmp,
                                 conv_params,
                                 gamma,
                                 delta,
                                 p_height,
                                 p_stride,
                                 p_width,
                                 i,
                                 j,
                                 sy4,
                                 reduce_bits_vert,
                                 res_add_const,
                                 round_bits,
                                 offset_bits);
    }
}

static INLINE void svt_prepare_warp_horizontal_filter(const uint8_t* ref, __m128i* tmp, int stride, int32_t ix4,
                                                      int32_t iy4, int32_t sx4, int alpha, int beta, int p_height,
                                                      int height, int i, const int offset_bits_horiz,
                                                      const int reduce_bits_horiz) {
    if (alpha == 0 && beta == 0) {
        svt_warp_horizontal_filter_alpha0_beta0(
            ref, tmp, stride, ix4, iy4, sx4, alpha, beta, p_height, height, i, offset_bits_horiz, reduce_bits_horiz);
    } else if (alpha == 0 && beta != 0) {
        svt_warp_horizontal_filter_alpha0(
            ref, tmp, stride, ix4, iy4, sx4, alpha, beta, p_height, height, i, offset_bits_horiz, reduce_bits_horiz);
    } else if (alpha != 0 && beta == 0) {
        svt_warp_horizontal_filter_beta0(
            ref, tmp, stride, ix4, iy4, sx4, alpha, beta, p_height, height, i, offset_bits_horiz, reduce_bits_horiz);
    } else {
        svt_warp_horizontal_filter(
            ref, tmp, stride, ix4, iy4, sx4, alpha, beta, p_height, height, i, offset_bits_horiz, reduce_bits_horiz);
    }
}

void svt_av1_warp_affine_sse4_1(const int32_t* mat, const uint8_t* ref, int width, int height, int stride,
                                uint8_t* pred, int p_col, int p_row, int p_width, int p_height, int p_stride,
                                int subsampling_x, int subsampling_y, ConvolveParams* conv_params, int16_t alpha,
                                int16_t beta, int16_t gamma, int16_t delta) {
    __m128i   tmp[15];
    int       i, j, k;
    const int bd                = 8;
    const int reduce_bits_horiz = conv_params->round_0;
    const int reduce_bits_vert  = conv_params->is_compound ? conv_params->round_1 : 2 * FILTER_BITS - reduce_bits_horiz;
    const int offset_bits_horiz = bd + FILTER_BITS - 1;
    assert(IMPLIES(conv_params->is_compound, conv_params->dst != NULL));

    const int     offset_bits_vert       = bd + 2 * FILTER_BITS - reduce_bits_horiz;
    const __m128i reduce_bits_vert_const = _mm_set1_epi32(((1 << reduce_bits_vert) >> 1));
    const __m128i res_add_const          = _mm_set1_epi32(1 << offset_bits_vert);
    const int     round_bits             = 2 * FILTER_BITS - conv_params->round_0 - conv_params->round_1;
    const int     offset_bits            = bd + 2 * FILTER_BITS - conv_params->round_0;
    assert(IMPLIES(conv_params->do_average, conv_params->is_compound));

    /* Note: For this code to work, the left/right frame borders need to be
  extended by at least 13 pixels each. By the time we get here, other
  code will have set up this border, but we allow an explicit check
  for debugging purposes.
  */
    /*for (i = 0; i < height; ++i) {
  for (j = 0; j < 13; ++j) {
  assert(ref[i * stride - 13 + j] == ref[i * stride]);
  assert(ref[i * stride + width + j] == ref[i * stride + (width - 1)]);
  }
  }*/
    __m128i res_add_const_1;
    if (conv_params->is_compound == 1) {
        res_add_const_1 = _mm_add_epi32(reduce_bits_vert_const, res_add_const);
    } else {
        res_add_const_1 = _mm_set1_epi32(-(1 << (bd + reduce_bits_vert - 1)) + ((1 << reduce_bits_vert) >> 1));
    }

    for (i = 0; i < p_height; i += 8) {
        for (j = 0; j < p_width; j += 8) {
            const int32_t src_x = (p_col + j + 4) << subsampling_x;
            const int32_t src_y = (p_row + i + 4) << subsampling_y;
            const int64_t dst_x = (int64_t)mat[2] * src_x + (int64_t)mat[3] * src_y + (int64_t)mat[0];
            const int64_t dst_y = (int64_t)mat[4] * src_x + (int64_t)mat[5] * src_y + (int64_t)mat[1];
            const int64_t x4    = dst_x >> subsampling_x;
            const int64_t y4    = dst_y >> subsampling_y;

            int32_t ix4 = (int32_t)(x4 >> WARPEDMODEL_PREC_BITS);
            int32_t sx4 = x4 & ((1 << WARPEDMODEL_PREC_BITS) - 1);
            int32_t iy4 = (int32_t)(y4 >> WARPEDMODEL_PREC_BITS);
            int32_t sy4 = y4 & ((1 << WARPEDMODEL_PREC_BITS) - 1);

            // Add in all the constant terms, including rounding and offset
            sx4 += alpha * (-4) + beta * (-4) + (1 << (WARPEDDIFF_PREC_BITS - 1)) +
                (WARPEDPIXEL_PREC_SHIFTS << WARPEDDIFF_PREC_BITS);
            sy4 += gamma * (-4) + delta * (-4) + (1 << (WARPEDDIFF_PREC_BITS - 1)) +
                (WARPEDPIXEL_PREC_SHIFTS << WARPEDDIFF_PREC_BITS);

            sx4 &= ~((1 << WARP_PARAM_REDUCE_BITS) - 1);
            sy4 &= ~((1 << WARP_PARAM_REDUCE_BITS) - 1);

            // Horizontal filter
            // If the block is aligned such that, after clamping, every sample
            // would be taken from the leftmost/rightmost column, then we can
            // skip the expensive horizontal filter.
            if (ix4 <= -7) {
                for (k = -7; k < AOMMIN(8, p_height - i); ++k) {
                    int iy = iy4 + k;
                    if (iy < 0) {
                        iy = 0;
                    } else if (iy > height - 1) {
                        iy = height - 1;
                    }
                    tmp[k + 7] = _mm_set1_epi16((1 << (bd + FILTER_BITS - reduce_bits_horiz - 1)) +
                                                ref[iy * stride] * (1 << (FILTER_BITS - reduce_bits_horiz)));
                }
            } else if (ix4 >= width + 6) {
                for (k = -7; k < AOMMIN(8, p_height - i); ++k) {
                    int iy = iy4 + k;
                    if (iy < 0) {
                        iy = 0;
                    } else if (iy > height - 1) {
                        iy = height - 1;
                    }
                    tmp[k + 7] = _mm_set1_epi16((1 << (bd + FILTER_BITS - reduce_bits_horiz - 1)) +
                                                ref[iy * stride + (width - 1)] *
                                                    (1 << (FILTER_BITS - reduce_bits_horiz)));
                }
            } else if (((ix4 - 7) < 0) || ((ix4 + 9) > width)) {
                const int out_of_boundary_left  = -(ix4 - 6);
                const int out_of_boundary_right = (ix4 + 8) - width;
                for (k = -7; k < AOMMIN(8, p_height - i); ++k) {
                    int iy = iy4 + k;
                    if (iy < 0) {
                        iy = 0;
                    } else if (iy > height - 1) {
                        iy = height - 1;
                    }
                    int sx = sx4 + beta * (k + 4);

                    // Load source pixels
                    __m128i src = _mm_loadu_si128((__m128i*)(ref + iy * stride + ix4 - 7));
                    if (out_of_boundary_left >= 0) {
                        const __m128i shuffle_reg_left = _mm_loadu_si128((__m128i*)warp_pad_left[out_of_boundary_left]);
                        src                            = _mm_shuffle_epi8(src, shuffle_reg_left);
                    }
                    if (out_of_boundary_right >= 0) {
                        const __m128i shuffle_reg_right = _mm_loadu_si128(
                            (__m128i*)warp_pad_right[out_of_boundary_right]);
                        src = _mm_shuffle_epi8(src, shuffle_reg_right);
                    }
                    svt_horizontal_filter(src, tmp, sx, alpha, k, offset_bits_horiz, reduce_bits_horiz);
                }
            } else {
                svt_prepare_warp_horizontal_filter(ref,
                                                   tmp,
                                                   stride,
                                                   ix4,
                                                   iy4,
                                                   sx4,
                                                   alpha,
                                                   beta,
                                                   p_height,
                                                   height,
                                                   i,
                                                   offset_bits_horiz,
                                                   reduce_bits_horiz);
            }

            // Vertical filter
            svt_prepare_warp_vertical_filter(pred,
                                             tmp,
                                             conv_params,
                                             gamma,
                                             delta,
                                             p_height,
                                             p_stride,
                                             p_width,
                                             i,
                                             j,
                                             sy4,
                                             reduce_bits_vert,
                                             &res_add_const_1,
                                             round_bits,
                                             offset_bits);
        }
    }
}

static INLINE __m128i load_ref_2buffer(const uint8_t* ref8b, const uint8_t* ref2b) {
    __m128i in_2_bit = _mm_loadl_epi64((__m128i*)ref2b);
    __m128i in_8_bit = _mm_loadl_epi64((__m128i*)ref8b);

    return _mm_srli_epi16(_mm_unpacklo_epi8(in_2_bit, in_8_bit), 6);
}

static INLINE void highbd_prepare_horizontal_filter_coeff_alpha0(int sx, __m128i* coeff) {
    // Filter coeff
    const __m128i tmp_0 = _mm_loadu_si128((__m128i*)(svt_aom_warped_filter + (sx >> WARPEDDIFF_PREC_BITS)));

    coeff[0] = _mm_shuffle_epi8(tmp_0, _mm_loadu_si128((__m128i*)highbd_shuffle_alpha0_mask0));
    coeff[2] = _mm_shuffle_epi8(tmp_0, _mm_loadu_si128((__m128i*)highbd_shuffle_alpha0_mask1));
    coeff[4] = _mm_shuffle_epi8(tmp_0, _mm_loadu_si128((__m128i*)highbd_shuffle_alpha0_mask2));
    coeff[6] = _mm_shuffle_epi8(tmp_0, _mm_loadu_si128((__m128i*)highbd_shuffle_alpha0_mask3));

    coeff[1] = coeff[0];
    coeff[3] = coeff[2];
    coeff[5] = coeff[4];
    coeff[7] = coeff[6];
}

static INLINE void highbd_prepare_horizontal_filter_coeff(int alpha, int sx, __m128i* coeff) {
    // Filter even-index pixels
    const __m128i tmp_0 = _mm_loadu_si128(
        (__m128i*)(svt_aom_warped_filter + ((sx + 0 * alpha) >> WARPEDDIFF_PREC_BITS)));
    const __m128i tmp_2 = _mm_loadu_si128(
        (__m128i*)(svt_aom_warped_filter + ((sx + 2 * alpha) >> WARPEDDIFF_PREC_BITS)));
    const __m128i tmp_4 = _mm_loadu_si128(
        (__m128i*)(svt_aom_warped_filter + ((sx + 4 * alpha) >> WARPEDDIFF_PREC_BITS)));
    const __m128i tmp_6 = _mm_loadu_si128(
        (__m128i*)(svt_aom_warped_filter + ((sx + 6 * alpha) >> WARPEDDIFF_PREC_BITS)));

    // coeffs 0 1 0 1 2 3 2 3 for pixels 0, 2
    const __m128i tmp_8 = _mm_unpacklo_epi32(tmp_0, tmp_2);
    // coeffs 0 1 0 1 2 3 2 3 for pixels 4, 6
    const __m128i tmp_10 = _mm_unpacklo_epi32(tmp_4, tmp_6);
    // coeffs 4 5 4 5 6 7 6 7 for pixels 0, 2
    const __m128i tmp_12 = _mm_unpackhi_epi32(tmp_0, tmp_2);
    // coeffs 4 5 4 5 6 7 6 7 for pixels 4, 6
    const __m128i tmp_14 = _mm_unpackhi_epi32(tmp_4, tmp_6);

    // coeffs 0 1 0 1 0 1 0 1 for pixels 0, 2, 4, 6
    coeff[0] = _mm_unpacklo_epi64(tmp_8, tmp_10);
    // coeffs 2 3 2 3 2 3 2 3 for pixels 0, 2, 4, 6
    coeff[2] = _mm_unpackhi_epi64(tmp_8, tmp_10);
    // coeffs 4 5 4 5 4 5 4 5 for pixels 0, 2, 4, 6
    coeff[4] = _mm_unpacklo_epi64(tmp_12, tmp_14);
    // coeffs 6 7 6 7 6 7 6 7 for pixels 0, 2, 4, 6
    coeff[6] = _mm_unpackhi_epi64(tmp_12, tmp_14);

    // Filter odd-index pixels
    const __m128i tmp_1 = _mm_loadu_si128(
        (__m128i*)(svt_aom_warped_filter + ((sx + 1 * alpha) >> WARPEDDIFF_PREC_BITS)));
    const __m128i tmp_3 = _mm_loadu_si128(
        (__m128i*)(svt_aom_warped_filter + ((sx + 3 * alpha) >> WARPEDDIFF_PREC_BITS)));
    const __m128i tmp_5 = _mm_loadu_si128(
        (__m128i*)(svt_aom_warped_filter + ((sx + 5 * alpha) >> WARPEDDIFF_PREC_BITS)));
    const __m128i tmp_7 = _mm_loadu_si128(
        (__m128i*)(svt_aom_warped_filter + ((sx + 7 * alpha) >> WARPEDDIFF_PREC_BITS)));

    const __m128i tmp_9  = _mm_unpacklo_epi32(tmp_1, tmp_3);
    const __m128i tmp_11 = _mm_unpacklo_epi32(tmp_5, tmp_7);
    const __m128i tmp_13 = _mm_unpackhi_epi32(tmp_1, tmp_3);
    const __m128i tmp_15 = _mm_unpackhi_epi32(tmp_5, tmp_7);

    coeff[1] = _mm_unpacklo_epi64(tmp_9, tmp_11);
    coeff[3] = _mm_unpackhi_epi64(tmp_9, tmp_11);
    coeff[5] = _mm_unpacklo_epi64(tmp_13, tmp_15);
    coeff[7] = _mm_unpackhi_epi64(tmp_13, tmp_15);
}

static INLINE void highbd_filter_src_pixels(const __m128i* src, const __m128i* src2, __m128i* tmp, __m128i* coeff,
                                            const int offset_bits_horiz, const int reduce_bits_horiz, int k) {
    const __m128i src_1  = *src;
    const __m128i src2_1 = *src2;

    const __m128i round_const = _mm_set1_epi32((1 << offset_bits_horiz) + ((1 << reduce_bits_horiz) >> 1));

    const __m128i res_0 = _mm_madd_epi16(src_1, coeff[0]);
    const __m128i res_2 = _mm_madd_epi16(_mm_alignr_epi8(src2_1, src_1, 4), coeff[2]);
    const __m128i res_4 = _mm_madd_epi16(_mm_alignr_epi8(src2_1, src_1, 8), coeff[4]);
    const __m128i res_6 = _mm_madd_epi16(_mm_alignr_epi8(src2_1, src_1, 12), coeff[6]);

    __m128i res_even = _mm_add_epi32(_mm_add_epi32(res_0, res_4), _mm_add_epi32(res_2, res_6));
    res_even         = _mm_sra_epi32(_mm_add_epi32(res_even, round_const), _mm_cvtsi32_si128(reduce_bits_horiz));

    const __m128i res_1 = _mm_madd_epi16(_mm_alignr_epi8(src2_1, src_1, 2), coeff[1]);
    const __m128i res_3 = _mm_madd_epi16(_mm_alignr_epi8(src2_1, src_1, 6), coeff[3]);
    const __m128i res_5 = _mm_madd_epi16(_mm_alignr_epi8(src2_1, src_1, 10), coeff[5]);
    const __m128i res_7 = _mm_madd_epi16(_mm_alignr_epi8(src2_1, src_1, 14), coeff[7]);

    __m128i res_odd = _mm_add_epi32(_mm_add_epi32(res_1, res_5), _mm_add_epi32(res_3, res_7));
    res_odd         = _mm_sra_epi32(_mm_add_epi32(res_odd, round_const), _mm_cvtsi32_si128(reduce_bits_horiz));

    // Combine results into one register.
    // We store the columns in the order 0, 2, 4, 6, 1, 3, 5, 7
    // as this order helps with the vertical filter.
    tmp[k + 7] = _mm_packs_epi32(res_even, res_odd);
}

static INLINE void highbd_horiz_filter(const __m128i* src, const __m128i* src2, __m128i* tmp, int sx, int alpha, int k,
                                       const int offset_bits_horiz, const int reduce_bits_horiz) {
    __m128i coeff[8];
    highbd_prepare_horizontal_filter_coeff(alpha, sx, coeff);
    highbd_filter_src_pixels(src, src2, tmp, coeff, offset_bits_horiz, reduce_bits_horiz, k);
}

static INLINE void svt_highbd_warp_horizontal_filter_alpha0_beta0(const uint8_t* ref8b, const uint8_t* ref2b,
                                                                  __m128i* tmp, int stride8b, int stride2b, int32_t ix4,
                                                                  int32_t iy4, int32_t sx4, int alpha, int beta,
                                                                  int p_height, int height, int i,
                                                                  const int offset_bits_horiz,
                                                                  const int reduce_bits_horiz) {
    (void)beta;
    (void)alpha;
    int k;

    __m128i coeff[8];
    highbd_prepare_horizontal_filter_coeff_alpha0(sx4, coeff);

    for (k = -7; k < AOMMIN(8, p_height - i); ++k) {
        int iy = iy4 + k;
        if (iy < 0) {
            iy = 0;
        } else if (iy > height - 1) {
            iy = height - 1;
        }

        // Load source pixels
        //const __m128i src  = _mm_loadu_si128((__m128i *)(ref + iy * stride + ix4 - 7));
        //const __m128i src2 = _mm_loadu_si128((__m128i *)(ref + iy * stride + ix4 + 1));
        const __m128i src  = load_ref_2buffer(&ref8b[iy * stride8b + ix4 - 7], &ref2b[iy * stride2b + ix4 - 7]);
        const __m128i src2 = load_ref_2buffer(&ref8b[iy * stride8b + ix4 + 1], &ref2b[iy * stride2b + ix4 + 1]);
        highbd_filter_src_pixels(&src, &src2, tmp, coeff, offset_bits_horiz, reduce_bits_horiz, k);
    }
}

static INLINE void svt_highbd_warp_horizontal_filter_alpha0(const uint8_t* ref8b, const uint8_t* ref2b, __m128i* tmp,
                                                            int stride8b, int stride2b, int32_t ix4, int32_t iy4,
                                                            int32_t sx4, int alpha, int beta, int p_height, int height,
                                                            int i, const int offset_bits_horiz,
                                                            const int reduce_bits_horiz) {
    (void)alpha;
    int k;
    for (k = -7; k < AOMMIN(8, p_height - i); ++k) {
        int iy = iy4 + k;
        if (iy < 0) {
            iy = 0;
        } else if (iy > height - 1) {
            iy = height - 1;
        }
        int sx = sx4 + beta * (k + 4);

        // Load source pixels
        //const __m128i src  = _mm_loadu_si128((__m128i *)(ref + iy * stride + ix4 - 7));
        //const __m128i src2 = _mm_loadu_si128((__m128i *)(ref + iy * stride + ix4 + 1));
        const __m128i src  = load_ref_2buffer(&ref8b[iy * stride8b + ix4 - 7], &ref2b[iy * stride2b + ix4 - 7]);
        const __m128i src2 = load_ref_2buffer(&ref8b[iy * stride8b + ix4 + 1], &ref2b[iy * stride2b + ix4 + 1]);

        __m128i coeff[8];
        highbd_prepare_horizontal_filter_coeff_alpha0(sx, coeff);
        highbd_filter_src_pixels(&src, &src2, tmp, coeff, offset_bits_horiz, reduce_bits_horiz, k);
    }
}

static INLINE void svt_highbd_warp_horizontal_filter_beta0(const uint8_t* ref8b, const uint8_t* ref2b, __m128i* tmp,
                                                           int stride8b, int stride2b, int32_t ix4, int32_t iy4,
                                                           int32_t sx4, int alpha, int beta, int p_height, int height,
                                                           int i, const int offset_bits_horiz,
                                                           const int reduce_bits_horiz) {
    (void)beta;
    int     k;
    __m128i coeff[8];
    highbd_prepare_horizontal_filter_coeff(alpha, sx4, coeff);

    for (k = -7; k < AOMMIN(8, p_height - i); ++k) {
        int iy = iy4 + k;
        if (iy < 0) {
            iy = 0;
        } else if (iy > height - 1) {
            iy = height - 1;
        }

        // Load source pixels
        //const __m128i src  = _mm_loadu_si128((__m128i *)(ref + iy * stride + ix4 - 7));
        //const __m128i src2 = _mm_loadu_si128((__m128i *)(ref + iy * stride + ix4 + 1));
        const __m128i src  = load_ref_2buffer(&ref8b[iy * stride8b + ix4 - 7], &ref2b[iy * stride2b + ix4 - 7]);
        const __m128i src2 = load_ref_2buffer(&ref8b[iy * stride8b + ix4 + 1], &ref2b[iy * stride2b + ix4 + 1]);

        highbd_filter_src_pixels(&src, &src2, tmp, coeff, offset_bits_horiz, reduce_bits_horiz, k);
    }
}

static INLINE void svt_highbd_warp_horizontal_filter(const uint8_t* ref8b, const uint8_t* ref2b, __m128i* tmp,
                                                     int stride8b, int stride2b, int32_t ix4, int32_t iy4, int32_t sx4,
                                                     int alpha, int beta, int p_height, int height, int i,
                                                     const int offset_bits_horiz, const int reduce_bits_horiz) {
    int k;
    for (k = -7; k < AOMMIN(8, p_height - i); ++k) {
        int iy = iy4 + k;
        if (iy < 0) {
            iy = 0;
        } else if (iy > height - 1) {
            iy = height - 1;
        }
        int sx = sx4 + beta * (k + 4);

        // Load source pixels
        //const __m128i src  = _mm_loadu_si128((__m128i *)(ref + iy * stride + ix4 - 7));
        //const __m128i src2 = _mm_loadu_si128((__m128i *)(ref + iy * stride + ix4 + 1));
        const __m128i src  = load_ref_2buffer(&ref8b[iy * stride8b + ix4 - 7], &ref2b[iy * stride2b + ix4 - 7]);
        const __m128i src2 = load_ref_2buffer(&ref8b[iy * stride8b + ix4 + 1], &ref2b[iy * stride2b + ix4 + 1]);

        highbd_horiz_filter(&src, &src2, tmp, sx, alpha, k, offset_bits_horiz, reduce_bits_horiz);
    }
}

static INLINE void svt_highbd_prepare_warp_horizontal_filter(const uint8_t* ref8b, const uint8_t* ref2b, __m128i* tmp,
                                                             int stride8b, int stride2b, int32_t ix4, int32_t iy4,
                                                             int32_t sx4, int alpha, int beta, int p_height, int height,
                                                             int i, const int offset_bits_horiz,
                                                             const int reduce_bits_horiz) {
    if (alpha == 0 && beta == 0) {
        svt_highbd_warp_horizontal_filter_alpha0_beta0(ref8b,
                                                       ref2b,
                                                       tmp,
                                                       stride8b,
                                                       stride2b,
                                                       ix4,
                                                       iy4,
                                                       sx4,
                                                       alpha,
                                                       beta,
                                                       p_height,
                                                       height,
                                                       i,
                                                       offset_bits_horiz,
                                                       reduce_bits_horiz);
    }

    else if (alpha == 0 && beta != 0) {
        svt_highbd_warp_horizontal_filter_alpha0(ref8b,
                                                 ref2b,
                                                 tmp,
                                                 stride8b,
                                                 stride2b,
                                                 ix4,
                                                 iy4,
                                                 sx4,
                                                 alpha,
                                                 beta,
                                                 p_height,
                                                 height,
                                                 i,
                                                 offset_bits_horiz,
                                                 reduce_bits_horiz);
    }

    else if (alpha != 0 && beta == 0) {
        svt_highbd_warp_horizontal_filter_beta0(ref8b,
                                                ref2b,
                                                tmp,
                                                stride8b,
                                                stride2b,
                                                ix4,
                                                iy4,
                                                sx4,
                                                alpha,
                                                beta,
                                                p_height,
                                                height,
                                                i,
                                                offset_bits_horiz,
                                                reduce_bits_horiz);
    } else {
        svt_highbd_warp_horizontal_filter(ref8b,
                                          ref2b,
                                          tmp,
                                          stride8b,
                                          stride2b,
                                          ix4,
                                          iy4,
                                          sx4,
                                          alpha,
                                          beta,
                                          p_height,
                                          height,
                                          i,
                                          offset_bits_horiz,
                                          reduce_bits_horiz);
    }
}

void svt_av1_highbd_warp_affine_sse4_1(const int32_t* mat, const uint8_t* ref8b, const uint8_t* ref2b, int width,
                                       int height, int stride8b, int stride2b, uint16_t* pred, int p_col, int p_row,
                                       int p_width, int p_height, int p_stride, int subsampling_x, int subsampling_y,
                                       int bd, ConvolveParams* conv_params, int16_t alpha, int16_t beta, int16_t gamma,
                                       int16_t delta) {
    __m128i   tmp[15];
    int       i, j, k;
    const int reduce_bits_horiz = conv_params->round_0 + AOMMAX(bd + FILTER_BITS - conv_params->round_0 - 14, 0);
    const int reduce_bits_vert  = conv_params->is_compound ? conv_params->round_1 : 2 * FILTER_BITS - reduce_bits_horiz;
    const int offset_bits_horiz = bd + FILTER_BITS - 1;
    assert(IMPLIES(conv_params->is_compound, conv_params->dst != NULL));
    assert(!(bd == 12 && reduce_bits_horiz < 5));
    assert(IMPLIES(conv_params->do_average, conv_params->is_compound));

    const int     offset_bits_vert       = bd + 2 * FILTER_BITS - reduce_bits_horiz;
    const __m128i clp_pxl                = _mm_set1_epi16(bd == 10 ? 1023 : (bd == 12 ? 4095 : 255));
    const __m128i reduce_bits_vert_shift = _mm_cvtsi32_si128(reduce_bits_vert);
    const __m128i reduce_bits_vert_const = _mm_set1_epi32(((1 << reduce_bits_vert) >> 1));
    const __m128i res_add_const          = _mm_set1_epi32(1 << offset_bits_vert);
    const int     round_bits             = 2 * FILTER_BITS - conv_params->round_0 - conv_params->round_1;
    const int     offset_bits            = bd + 2 * FILTER_BITS - conv_params->round_0;
    const __m128i res_sub_const          = _mm_set1_epi32(-(1 << (offset_bits - conv_params->round_1)) -
                                                 (1 << (offset_bits - conv_params->round_1 - 1)));
    __m128i       round_bits_shift       = _mm_cvtsi32_si128(round_bits);
    __m128i       round_bits_const       = _mm_set1_epi32(((1 << round_bits) >> 1));

    const int     w0  = conv_params->fwd_offset;
    const int     w1  = conv_params->bck_offset;
    const __m128i wt0 = _mm_set1_epi32(w0);
    const __m128i wt1 = _mm_set1_epi32(w1);

    /* Note: For this code to work, the left/right frame borders need to be
  extended by at least 13 pixels each. By the time we get here, other
  code will have set up this border, but we allow an explicit check
  for debugging purposes.
  */
    /*for (i = 0; i < height; ++i) {
  for (j = 0; j < 13; ++j) {
  assert(ref[i * stride - 13 + j] == ref[i * stride]);
  assert(ref[i * stride + width + j] == ref[i * stride + (width - 1)]);
  }
  }*/

    for (i = 0; i < p_height; i += 8) {
        for (j = 0; j < p_width; j += 8) {
            const int32_t src_x = (p_col + j + 4) << subsampling_x;
            const int32_t src_y = (p_row + i + 4) << subsampling_y;
            const int64_t dst_x = (int64_t)mat[2] * src_x + (int64_t)mat[3] * src_y + (int64_t)mat[0];
            const int64_t dst_y = (int64_t)mat[4] * src_x + (int64_t)mat[5] * src_y + (int64_t)mat[1];
            const int64_t x4    = dst_x >> subsampling_x;
            const int64_t y4    = dst_y >> subsampling_y;

            int32_t ix4 = (int32_t)(x4 >> WARPEDMODEL_PREC_BITS);
            int32_t sx4 = x4 & ((1 << WARPEDMODEL_PREC_BITS) - 1);
            int32_t iy4 = (int32_t)(y4 >> WARPEDMODEL_PREC_BITS);
            int32_t sy4 = y4 & ((1 << WARPEDMODEL_PREC_BITS) - 1);

            // Add in all the constant terms, including rounding and offset
            sx4 += alpha * (-4) + beta * (-4) + (1 << (WARPEDDIFF_PREC_BITS - 1)) +
                (WARPEDPIXEL_PREC_SHIFTS << WARPEDDIFF_PREC_BITS);
            sy4 += gamma * (-4) + delta * (-4) + (1 << (WARPEDDIFF_PREC_BITS - 1)) +
                (WARPEDPIXEL_PREC_SHIFTS << WARPEDDIFF_PREC_BITS);

            sx4 &= ~((1 << WARP_PARAM_REDUCE_BITS) - 1);
            sy4 &= ~((1 << WARP_PARAM_REDUCE_BITS) - 1);

            // Horizontal filter
            // If the block is aligned such that, after clamping, every sample
            // would be taken from the leftmost/rightmost column, then we can
            // skip the expensive horizontal filter.
            if (ix4 <= -7) {
                for (k = -7; k < AOMMIN(8, p_height - i); ++k) {
                    int iy = iy4 + k;
                    if (iy < 0) {
                        iy = 0;
                    } else if (iy > height - 1) {
                        iy = height - 1;
                    }
                    uint16_t ref_val = (ref8b[iy * stride8b] << 2) | ((ref2b[iy * stride2b] >> 6) & 3);
                    tmp[k + 7]       = _mm_set1_epi16((1 << (bd + FILTER_BITS - reduce_bits_horiz - 1)) +
                                                ref_val * (1 << (FILTER_BITS - reduce_bits_horiz)));
                }
            } else if (ix4 >= width + 6) {
                for (k = -7; k < AOMMIN(8, p_height - i); ++k) {
                    int iy = iy4 + k;
                    if (iy < 0) {
                        iy = 0;
                    } else if (iy > height - 1) {
                        iy = height - 1;
                    }
                    uint16_t ref_val = (ref8b[iy * stride8b + (width - 1)] << 2) |
                        ((ref2b[iy * stride2b + (width - 1)] >> 6) & 3);
                    tmp[k + 7] = _mm_set1_epi16((1 << (bd + FILTER_BITS - reduce_bits_horiz - 1)) +
                                                ref_val * (1 << (FILTER_BITS - reduce_bits_horiz)));
                }
            } else if (((ix4 - 7) < 0) || ((ix4 + 9) > width)) {
                const int out_of_boundary_left  = -(ix4 - 6);
                const int out_of_boundary_right = (ix4 + 8) - width;

                for (k = -7; k < AOMMIN(8, p_height - i); ++k) {
                    int iy = iy4 + k;
                    if (iy < 0) {
                        iy = 0;
                    } else if (iy > height - 1) {
                        iy = height - 1;
                    }
                    int sx = sx4 + beta * (k + 4);

                    // Load source pixels
                    //const __m128i src  = _mm_loadu_si128((__m128i *)(ref + iy * stride + ix4 - 7));
                    //const __m128i src2 = _mm_loadu_si128((__m128i *)(ref + iy * stride + ix4 + 1));
                    const __m128i src  = load_ref_2buffer(&ref8b[iy * stride8b + ix4 - 7],
                                                         &ref2b[iy * stride2b + ix4 - 7]);
                    const __m128i src2 = load_ref_2buffer(&ref8b[iy * stride8b + ix4 + 1],
                                                          &ref2b[iy * stride2b + ix4 + 1]);

                    const __m128i src_01  = _mm_shuffle_epi8(src, _mm_loadu_si128((__m128i*)warp_highbd_arrange_bytes));
                    const __m128i src2_01 = _mm_shuffle_epi8(src2,
                                                             _mm_loadu_si128((__m128i*)warp_highbd_arrange_bytes));

                    __m128i src_lo = _mm_unpacklo_epi64(src_01, src2_01);
                    __m128i src_hi = _mm_unpackhi_epi64(src_01, src2_01);

                    if (out_of_boundary_left >= 0) {
                        const __m128i shuffle_reg_left = _mm_loadu_si128((__m128i*)warp_pad_left[out_of_boundary_left]);
                        src_lo                         = _mm_shuffle_epi8(src_lo, shuffle_reg_left);
                        src_hi                         = _mm_shuffle_epi8(src_hi, shuffle_reg_left);
                    }

                    if (out_of_boundary_right >= 0) {
                        const __m128i shuffle_reg_right = _mm_loadu_si128(
                            (__m128i*)warp_pad_right[out_of_boundary_right]);
                        src_lo = _mm_shuffle_epi8(src_lo, shuffle_reg_right);
                        src_hi = _mm_shuffle_epi8(src_hi, shuffle_reg_right);
                    }

                    const __m128i src_padded  = _mm_unpacklo_epi8(src_lo, src_hi);
                    const __m128i src2_padded = _mm_unpackhi_epi8(src_lo, src_hi);

                    highbd_horiz_filter(
                        &src_padded, &src2_padded, tmp, sx, alpha, k, offset_bits_horiz, reduce_bits_horiz);
                }
            } else {
                svt_highbd_prepare_warp_horizontal_filter(ref8b,
                                                          ref2b,
                                                          tmp,
                                                          stride8b,
                                                          stride2b,
                                                          ix4,
                                                          iy4,
                                                          sx4,
                                                          alpha,
                                                          beta,
                                                          p_height,
                                                          height,
                                                          i,
                                                          offset_bits_horiz,
                                                          reduce_bits_horiz);
            }

            // Vertical filter
            for (k = -4; k < AOMMIN(4, p_height - i - 4); ++k) {
                int sy = sy4 + delta * (k + 4);

                // Load from tmp and rearrange pairs of consecutive rows into the
                // column order 0 0 2 2 4 4 6 6; 1 1 3 3 5 5 7 7
                const __m128i* src   = tmp + (k + 4);
                const __m128i  src_0 = _mm_unpacklo_epi16(src[0], src[1]);
                const __m128i  src_2 = _mm_unpacklo_epi16(src[2], src[3]);
                const __m128i  src_4 = _mm_unpacklo_epi16(src[4], src[5]);
                const __m128i  src_6 = _mm_unpacklo_epi16(src[6], src[7]);

                // Filter even-index pixels
                const __m128i tmp_0 = _mm_loadu_si128(
                    (__m128i*)(svt_aom_warped_filter + ((sy + 0 * gamma) >> WARPEDDIFF_PREC_BITS)));
                const __m128i tmp_2 = _mm_loadu_si128(
                    (__m128i*)(svt_aom_warped_filter + ((sy + 2 * gamma) >> WARPEDDIFF_PREC_BITS)));
                const __m128i tmp_4 = _mm_loadu_si128(
                    (__m128i*)(svt_aom_warped_filter + ((sy + 4 * gamma) >> WARPEDDIFF_PREC_BITS)));
                const __m128i tmp_6 = _mm_loadu_si128(
                    (__m128i*)(svt_aom_warped_filter + ((sy + 6 * gamma) >> WARPEDDIFF_PREC_BITS)));

                const __m128i tmp_8  = _mm_unpacklo_epi32(tmp_0, tmp_2);
                const __m128i tmp_10 = _mm_unpacklo_epi32(tmp_4, tmp_6);
                const __m128i tmp_12 = _mm_unpackhi_epi32(tmp_0, tmp_2);
                const __m128i tmp_14 = _mm_unpackhi_epi32(tmp_4, tmp_6);

                const __m128i coeff_0 = _mm_unpacklo_epi64(tmp_8, tmp_10);
                const __m128i coeff_2 = _mm_unpackhi_epi64(tmp_8, tmp_10);
                const __m128i coeff_4 = _mm_unpacklo_epi64(tmp_12, tmp_14);
                const __m128i coeff_6 = _mm_unpackhi_epi64(tmp_12, tmp_14);

                const __m128i res_0 = _mm_madd_epi16(src_0, coeff_0);
                const __m128i res_2 = _mm_madd_epi16(src_2, coeff_2);
                const __m128i res_4 = _mm_madd_epi16(src_4, coeff_4);
                const __m128i res_6 = _mm_madd_epi16(src_6, coeff_6);

                const __m128i res_even = _mm_add_epi32(_mm_add_epi32(res_0, res_2), _mm_add_epi32(res_4, res_6));

                // Filter odd-index pixels
                const __m128i src_1 = _mm_unpackhi_epi16(src[0], src[1]);
                const __m128i src_3 = _mm_unpackhi_epi16(src[2], src[3]);
                const __m128i src_5 = _mm_unpackhi_epi16(src[4], src[5]);
                const __m128i src_7 = _mm_unpackhi_epi16(src[6], src[7]);

                const __m128i tmp_1 = _mm_loadu_si128(
                    (__m128i*)(svt_aom_warped_filter + ((sy + 1 * gamma) >> WARPEDDIFF_PREC_BITS)));
                const __m128i tmp_3 = _mm_loadu_si128(
                    (__m128i*)(svt_aom_warped_filter + ((sy + 3 * gamma) >> WARPEDDIFF_PREC_BITS)));
                const __m128i tmp_5 = _mm_loadu_si128(
                    (__m128i*)(svt_aom_warped_filter + ((sy + 5 * gamma) >> WARPEDDIFF_PREC_BITS)));
                const __m128i tmp_7 = _mm_loadu_si128(
                    (__m128i*)(svt_aom_warped_filter + ((sy + 7 * gamma) >> WARPEDDIFF_PREC_BITS)));

                const __m128i tmp_9  = _mm_unpacklo_epi32(tmp_1, tmp_3);
                const __m128i tmp_11 = _mm_unpacklo_epi32(tmp_5, tmp_7);
                const __m128i tmp_13 = _mm_unpackhi_epi32(tmp_1, tmp_3);
                const __m128i tmp_15 = _mm_unpackhi_epi32(tmp_5, tmp_7);

                const __m128i coeff_1 = _mm_unpacklo_epi64(tmp_9, tmp_11);
                const __m128i coeff_3 = _mm_unpackhi_epi64(tmp_9, tmp_11);
                const __m128i coeff_5 = _mm_unpacklo_epi64(tmp_13, tmp_15);
                const __m128i coeff_7 = _mm_unpackhi_epi64(tmp_13, tmp_15);

                const __m128i res_1 = _mm_madd_epi16(src_1, coeff_1);
                const __m128i res_3 = _mm_madd_epi16(src_3, coeff_3);
                const __m128i res_5 = _mm_madd_epi16(src_5, coeff_5);
                const __m128i res_7 = _mm_madd_epi16(src_7, coeff_7);

                const __m128i res_odd = _mm_add_epi32(_mm_add_epi32(res_1, res_3), _mm_add_epi32(res_5, res_7));

                // Rearrange pixels back into the order 0 ... 7
                __m128i res_lo = _mm_unpacklo_epi32(res_even, res_odd);
                __m128i res_hi = _mm_unpackhi_epi32(res_even, res_odd);

                if (conv_params->is_compound) {
                    __m128i* const p = (__m128i*)&conv_params->dst[(i + k + 4) * conv_params->dst_stride + j];
                    res_lo           = _mm_add_epi32(res_lo, res_add_const);
                    res_lo = _mm_sra_epi32(_mm_add_epi32(res_lo, reduce_bits_vert_const), reduce_bits_vert_shift);

                    if (conv_params->do_average) {
                        __m128i* const dst16 = (__m128i*)&pred[(i + k + 4) * p_stride + j];
                        __m128i        p_32  = _mm_cvtepu16_epi32(_mm_loadl_epi64(p));

                        if (conv_params->use_jnt_comp_avg) {
                            res_lo = _mm_add_epi32(_mm_mullo_epi32(p_32, wt0), _mm_mullo_epi32(res_lo, wt1));
                            res_lo = _mm_srai_epi32(res_lo, DIST_PRECISION_BITS);
                        } else {
                            res_lo = _mm_srai_epi32(_mm_add_epi32(p_32, res_lo), 1);
                        }

                        __m128i res32_lo = _mm_add_epi32(res_lo, res_sub_const);
                        res32_lo         = _mm_sra_epi32(_mm_add_epi32(res32_lo, round_bits_const), round_bits_shift);

                        __m128i res16_lo = _mm_packus_epi32(res32_lo, res32_lo);
                        res16_lo         = _mm_min_epi16(res16_lo, clp_pxl);
                        _mm_storel_epi64(dst16, res16_lo);
                    } else {
                        res_lo = _mm_packus_epi32(res_lo, res_lo);
                        _mm_storel_epi64(p, res_lo);
                    }
                    if (p_width > 4) {
                        __m128i* const p4 = (__m128i*)&conv_params->dst[(i + k + 4) * conv_params->dst_stride + j + 4];

                        res_hi = _mm_add_epi32(res_hi, res_add_const);
                        res_hi = _mm_sra_epi32(_mm_add_epi32(res_hi, reduce_bits_vert_const), reduce_bits_vert_shift);
                        if (conv_params->do_average) {
                            __m128i* const dst16_4 = (__m128i*)&pred[(i + k + 4) * p_stride + j + 4];
                            __m128i        p4_32   = _mm_cvtepu16_epi32(_mm_loadl_epi64(p4));

                            if (conv_params->use_jnt_comp_avg) {
                                res_hi = _mm_add_epi32(_mm_mullo_epi32(p4_32, wt0), _mm_mullo_epi32(res_hi, wt1));
                                res_hi = _mm_srai_epi32(res_hi, DIST_PRECISION_BITS);
                            } else {
                                res_hi = _mm_srai_epi32(_mm_add_epi32(p4_32, res_hi), 1);
                            }

                            __m128i res32_hi = _mm_add_epi32(res_hi, res_sub_const);
                            res32_hi = _mm_sra_epi32(_mm_add_epi32(res32_hi, round_bits_const), round_bits_shift);
                            __m128i res16_hi = _mm_packus_epi32(res32_hi, res32_hi);
                            res16_hi         = _mm_min_epi16(res16_hi, clp_pxl);
                            _mm_storel_epi64(dst16_4, res16_hi);
                        } else {
                            res_hi = _mm_packus_epi32(res_hi, res_hi);
                            _mm_storel_epi64(p4, res_hi);
                        }
                    }
                } else {
                    // Round and pack into 8 bits
                    const __m128i round_const = _mm_set1_epi32(-(1 << (bd + reduce_bits_vert - 1)) +
                                                               ((1 << reduce_bits_vert) >> 1));

                    const __m128i res_lo_round = _mm_srai_epi32(_mm_add_epi32(res_lo, round_const), reduce_bits_vert);
                    const __m128i res_hi_round = _mm_srai_epi32(_mm_add_epi32(res_hi, round_const), reduce_bits_vert);

                    __m128i res_16bit = _mm_packs_epi32(res_lo_round, res_hi_round);
                    // Clamp res_16bit to the range [0, 2^bd - 1]
                    const __m128i max_val = _mm_set1_epi16((1 << bd) - 1);
                    const __m128i zero    = _mm_setzero_si128();
                    res_16bit             = _mm_max_epi16(_mm_min_epi16(res_16bit, max_val), zero);

                    // Store, blending with 'pred' if needed
                    __m128i* const p = (__m128i*)&pred[(i + k + 4) * p_stride + j];

                    // Note: If we're outputting a 4x4 block, we need to be very careful
                    // to only output 4 pixels at this point, to avoid encode/decode
                    // mismatches when encoding with multiple threads.
                    if (p_width == 4) {
                        _mm_storel_epi64(p, res_16bit);
                    } else {
                        _mm_storeu_si128(p, res_16bit);
                    }
                }
            }
        }
    }
}
