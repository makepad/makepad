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

#include <immintrin.h>
#include "common_dsp_rtcd.h"
#include "convolve.h"
#include "convolve_avx2.h"
#include "definitions.h"
#include "memory_sse4_1.h"

static void jnt_convolve_2d_hor_2tap_avx2(const uint8_t* src, const int32_t src_stride, const int32_t w,
                                          const int32_t h, const InterpFilterParams* filter_params_x,
                                          const int32_t subpel_x_q4, int16_t* const im_block) {
    const uint8_t* src_ptr = src;
    int32_t        y       = h;
    int16_t*       im      = im_block;

    if (w <= 8) {
        __m128i coeffs_128;
        prepare_half_coeffs_2tap_ssse3(filter_params_x, subpel_x_q4, &coeffs_128);

        if (w == 2) {
            do {
                const __m128i r = x_convolve_2tap_2x2_sse4_1(src_ptr, src_stride, &coeffs_128);
                xy_x_round_store_2x2_sse2(r, im);
                src_ptr += 2 * src_stride;
                im += 2 * 2;
                y -= 2;
            } while (y);
        } else if (w == 4) {
            do {
                const __m128i r = x_convolve_2tap_4x2_ssse3(src_ptr, src_stride, &coeffs_128);
                xy_x_round_store_4x2_sse2(r, im);
                src_ptr += 2 * src_stride;
                im += 2 * 4;
                y -= 2;
            } while (y);
        } else {
            assert(w == 8);

            do {
                __m128i r[2];

                x_convolve_2tap_8x2_ssse3(src_ptr, src_stride, &coeffs_128, r);
                xy_x_round_store_8x2_sse2(r, im);
                src_ptr += 2 * src_stride;
                im += 2 * 8;
                y -= 2;
            } while (y);
        }
    } else {
        __m256i coeffs_256;
        prepare_half_coeffs_2tap_avx2(filter_params_x, subpel_x_q4, &coeffs_256);

        if (w == 16) {
            do {
                __m256i r[2];

                x_convolve_2tap_16x2_avx2(src_ptr, src_stride, &coeffs_256, r);
                xy_x_round_store_32_avx2(r, im);
                src_ptr += 2 * src_stride;
                im += 2 * 16;
                y -= 2;
            } while (y);
        } else if (w == 32) {
            do {
                xy_x_2tap_32_avx2(src_ptr, &coeffs_256, im);
                src_ptr += src_stride;
                im += 32;
            } while (--y);
        } else if (w == 64) {
            do {
                xy_x_2tap_32_avx2(src_ptr + 0 * 32, &coeffs_256, im + 0 * 32);
                xy_x_2tap_32_avx2(src_ptr + 1 * 32, &coeffs_256, im + 1 * 32);
                src_ptr += src_stride;
                im += 64;
            } while (--y);
        } else {
            assert(w == 128);

            do {
                xy_x_2tap_32_avx2(src_ptr + 0 * 32, &coeffs_256, im + 0 * 32);
                xy_x_2tap_32_avx2(src_ptr + 1 * 32, &coeffs_256, im + 1 * 32);
                xy_x_2tap_32_avx2(src_ptr + 2 * 32, &coeffs_256, im + 2 * 32);
                xy_x_2tap_32_avx2(src_ptr + 3 * 32, &coeffs_256, im + 3 * 32);
                src_ptr += src_stride;
                im += 128;
            } while (--y);
        }
    }
}

void jnt_convolve_2d_hor_4tap_avx2(const uint8_t* src, const int32_t src_stride, const int32_t w, const int32_t h,
                                   const InterpFilterParams* filter_params_x, const int32_t subpel_x_q4,
                                   int16_t* const im_block) {
    const uint8_t* src_ptr = src - 1;
    int32_t        y       = h;
    int16_t*       im      = im_block;
    __m128i        coeffs_128[4];

    prepare_half_coeffs_4tap_ssse3(filter_params_x, subpel_x_q4, coeffs_128);

    if (w == 2) {
        do {
            const __m128i r = x_convolve_4tap_2x2_ssse3(src_ptr, src_stride, coeffs_128);
            xy_x_round_store_2x2_sse2(r, im);
            src_ptr += 2 * src_stride;
            im += 2 * 2;
            y -= 2;
        } while (y);
    } else {
        assert(w == 4);

        do {
            const __m128i r = x_convolve_4tap_4x2_ssse3(src_ptr, src_stride, coeffs_128);
            xy_x_round_store_4x2_sse2(r, im);
            src_ptr += 2 * src_stride;
            im += 2 * 4;
            y -= 2;
        } while (y);
    }
};

static void jnt_convolve_2d_hor_6tap_avx2(const uint8_t* src, const int32_t src_stride, const int32_t w,
                                          const int32_t h, const InterpFilterParams* filter_params_x,
                                          const int32_t subpel_x_q4, int16_t* const im_block) {
    const uint8_t* src_ptr = src - 2;
    int32_t        y       = h;
    int16_t*       im      = im_block;
    __m256i        coeffs_256[3], filt_256[3];

    filt_256[0] = _mm256_loadu_si256((__m256i const*)filt1_global_avx);
    filt_256[1] = _mm256_loadu_si256((__m256i const*)filt2_global_avx);
    filt_256[2] = _mm256_loadu_si256((__m256i const*)filt3_global_avx);

    prepare_half_coeffs_6tap_avx2(filter_params_x, subpel_x_q4, coeffs_256);

    if (w == 8) {
        do {
            const __m256i res = x_convolve_6tap_8x2_avx2(src_ptr, src_stride, coeffs_256, filt_256);
            xy_x_round_store_8x2_avx2(res, im);
            src_ptr += 2 * src_stride;
            im += 2 * 8;
            y -= 2;
        } while (y);
    } else if (w == 16) {
        do {
            __m256i r[2];

            x_convolve_6tap_16x2_avx2(src_ptr, src_stride, coeffs_256, filt_256, r);
            xy_x_round_store_32_avx2(r, im);
            src_ptr += 2 * src_stride;
            im += 2 * 16;
            y -= 2;
        } while (y);
    } else if (w == 32) {
        do {
            xy_x_6tap_32_avx2(src_ptr, coeffs_256, filt_256, im);
            src_ptr += src_stride;
            im += 32;
        } while (--y);
    } else if (w == 64) {
        do {
            xy_x_6tap_32_avx2(src_ptr, coeffs_256, filt_256, im);
            xy_x_6tap_32_avx2(src_ptr + 32, coeffs_256, filt_256, im + 32);
            src_ptr += src_stride;
            im += 64;
        } while (--y);
    } else {
        assert(w == 128);

        do {
            xy_x_6tap_32_avx2(src_ptr, coeffs_256, filt_256, im);
            xy_x_6tap_32_avx2(src_ptr + 32, coeffs_256, filt_256, im + 32);
            xy_x_6tap_32_avx2(src_ptr + 64, coeffs_256, filt_256, im + 64);
            xy_x_6tap_32_avx2(src_ptr + 96, coeffs_256, filt_256, im + 96);
            src_ptr += src_stride;
            im += 128;
        } while (--y);
    }
}

static void jnt_convolve_2d_hor_8tap_avx2(const uint8_t* src, const int32_t src_stride, const int32_t w,
                                          const int32_t h, const InterpFilterParams* filter_params_x,
                                          const int32_t subpel_x_q4, int16_t* const im_block) {
    const uint8_t* src_ptr = src - 3;
    int32_t        y       = h;
    int16_t*       im      = im_block;
    __m256i        coeffs_256[4], filt_256[4];

    filt_256[0] = _mm256_loadu_si256((__m256i const*)filt1_global_avx);
    filt_256[1] = _mm256_loadu_si256((__m256i const*)filt2_global_avx);
    filt_256[2] = _mm256_loadu_si256((__m256i const*)filt3_global_avx);
    filt_256[3] = _mm256_loadu_si256((__m256i const*)filt4_global_avx);

    prepare_half_coeffs_8tap_avx2(filter_params_x, subpel_x_q4, coeffs_256);

    if (w == 8) {
        do {
            const __m256i res = x_convolve_8tap_8x2_avx2(src_ptr, src_stride, coeffs_256, filt_256);
            xy_x_round_store_8x2_avx2(res, im);
            src_ptr += 2 * src_stride;
            im += 2 * 8;
            y -= 2;
        } while (y);
    } else if (w == 16) {
        do {
            __m256i r[2];

            x_convolve_8tap_16x2_avx2(src_ptr, src_stride, coeffs_256, filt_256, r);
            xy_x_round_store_32_avx2(r, im);
            src_ptr += 2 * src_stride;
            im += 2 * 16;
            y -= 2;
        } while (y);
    } else if (w == 32) {
        do {
            xy_x_8tap_32_avx2(src_ptr, coeffs_256, filt_256, im);
            src_ptr += src_stride;
            im += 32;
        } while (--y);
    } else if (w == 64) {
        do {
            xy_x_8tap_32_avx2(src_ptr, coeffs_256, filt_256, im);
            xy_x_8tap_32_avx2(src_ptr + 32, coeffs_256, filt_256, im + 32);
            src_ptr += src_stride;
            im += 64;
        } while (--y);
    } else {
        assert(w == 128);

        do {
            xy_x_8tap_32_avx2(src_ptr, coeffs_256, filt_256, im);
            xy_x_8tap_32_avx2(src_ptr + 32, coeffs_256, filt_256, im + 32);
            xy_x_8tap_32_avx2(src_ptr + 64, coeffs_256, filt_256, im + 64);
            xy_x_8tap_32_avx2(src_ptr + 96, coeffs_256, filt_256, im + 96);
            src_ptr += src_stride;
            im += 128;
        } while (--y);
    }
}

static void jnt_convolve_2d_ver_2tap_avx2(const int16_t* const im_block, const int32_t w, const int32_t h,
                                          const InterpFilterParams* const filter_params_y, const int32_t subpel_y_q4,
                                          const ConvolveParams* const conv_params, uint8_t* dst8,
                                          const int32_t dst8_stride) {
    const int32_t  dst_stride  = conv_params->dst_stride;
    const int32_t  bd          = 8;
    const int32_t  round_0     = 3;
    const int16_t* im          = im_block;
    const int32_t  round_1     = COMPOUND_ROUND1_BITS;
    const int32_t  offset_bits = bd + 2 * FILTER_BITS - round_0; // 19
    const int32_t  round_bits  = 2 * FILTER_BITS - round_0 - round_1; // 4
    const int32_t  offset_avg  = (1 << (round_1 - 1)) + (1 << (round_bits + round_1)) - (1 << offset_bits) -
        (1 << (offset_bits - 1));
    const int32_t offset_no_avg = (1 << (round_1 - 1)) + (1 << offset_bits) + (1 << (offset_bits - 1));
    ConvBufType*  dst           = conv_params->dst;
    int32_t       y             = h;

    if (w <= 4) {
        const __m128i offset_avg_128    = _mm_set1_epi32(offset_avg);
        const __m128i offset_no_avg_128 = _mm_set1_epi32(offset_no_avg);
        __m128i       coeffs_128;

        prepare_coeffs_2tap_sse2(filter_params_y, subpel_y_q4, &coeffs_128);

        if (w == 2) {
            __m128i s_32[2];

            s_32[0] = _mm_cvtsi32_si128(*(int32_t*)im);

            if (conv_params->do_average) {
                if (conv_params->use_jnt_comp_avg) {
                    const int32_t round_offset    = 1 << (offset_bits - round_1);
                    const int32_t factor          = conv_params->fwd_offset | (conv_params->bck_offset << 16);
                    const int32_t offset_comp_avg = (round_offset + (round_offset >> 1)) * conv_params->bck_offset -
                        (round_offset << DIST_PRECISION_BITS) - (round_offset << (DIST_PRECISION_BITS - 1)) +
                        (1 << (round_bits + DIST_PRECISION_BITS - 1));
                    const __m128i factor_128          = _mm_set1_epi32(factor);
                    const __m128i offset_comp_avg_128 = _mm_set1_epi32(offset_comp_avg);
                    do {
                        const __m128i res = xy_y_convolve_2tap_2x2_sse2(im, s_32, &coeffs_128);
                        jnt_2d_comp_avg_round_store_2x2_sse2(
                            res, factor_128, offset_comp_avg_128, dst, dst_stride, dst8, dst8_stride);
                        im += 2 * 2;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                } else {
                    do {
                        const __m128i res = xy_y_convolve_2tap_2x2_sse2(im, s_32, &coeffs_128);
                        jnt_2d_avg_round_store_2x2_sse2(res, offset_avg_128, dst, dst_stride, dst8, dst8_stride);
                        im += 2 * 2;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                }
            } else {
                do {
                    const __m128i res = xy_y_convolve_2tap_2x2_sse2(im, s_32, &coeffs_128);
                    jnt_2d_no_avg_round_store_2x2_sse2(res, offset_no_avg_128, dst, dst_stride);
                    im += 2 * 2;
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            }
        } else {
            __m128i s_64[2], r[2];

            assert(w == 4);

            s_64[0] = _mm_loadl_epi64((__m128i*)im);

            if (conv_params->do_average) {
                if (conv_params->use_jnt_comp_avg) {
                    const int32_t round_offset    = 1 << (offset_bits - round_1);
                    const int32_t factor          = conv_params->fwd_offset | (conv_params->bck_offset << 16);
                    const int32_t offset_comp_avg = (round_offset + (round_offset >> 1)) * conv_params->bck_offset -
                        (round_offset << DIST_PRECISION_BITS) - (round_offset << (DIST_PRECISION_BITS - 1)) +
                        (1 << (round_bits + DIST_PRECISION_BITS - 1));
                    const __m128i factor_128          = _mm_set1_epi32(factor);
                    const __m128i offset_comp_avg_128 = _mm_set1_epi32(offset_comp_avg);
                    do {
                        xy_y_convolve_2tap_4x2_sse2(im, s_64, &coeffs_128, r);
                        jnt_2d_comp_avg_round_store_4x2_sse2(
                            r, factor_128, offset_comp_avg_128, dst, dst_stride, dst8, dst8_stride);
                        im += 2 * 4;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                } else {
                    do {
                        xy_y_convolve_2tap_4x2_sse2(im, s_64, &coeffs_128, r);
                        jnt_2d_avg_round_store_4x2_sse2(r, offset_avg_128, dst, dst_stride, dst8, dst8_stride);
                        im += 2 * 4;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                }
            } else {
                do {
                    xy_y_convolve_2tap_4x2_sse2(im, s_64, &coeffs_128, r);
                    jnt_2d_no_avg_round_store_4x2_sse2(r, offset_no_avg_128, dst, dst_stride);
                    im += 2 * 4;
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            }
        }
    } else {
        const __m256i offset_avg_256    = _mm256_set1_epi32(offset_avg);
        const __m256i offset_no_avg_256 = _mm256_set1_epi32(offset_no_avg);
        __m256i       coeffs_256;

        prepare_coeffs_2tap_avx2(filter_params_y, subpel_y_q4, &coeffs_256);

        if (w == 8) {
            __m128i s_128[2];
            __m256i r[2];

            s_128[0] = _mm_loadu_si128((__m128i*)im);

            if (conv_params->do_average) {
                if (conv_params->use_jnt_comp_avg) {
                    do {
                        const int32_t round_offset    = 1 << (offset_bits - round_1);
                        const int32_t factor          = conv_params->fwd_offset | (conv_params->bck_offset << 16);
                        const int32_t offset_comp_avg = (round_offset + (round_offset >> 1)) * conv_params->bck_offset -
                            (round_offset << DIST_PRECISION_BITS) - (round_offset << (DIST_PRECISION_BITS - 1)) +
                            (1 << (round_bits + DIST_PRECISION_BITS - 1));
                        const __m256i factor_256          = _mm256_set1_epi32(factor);
                        const __m256i offset_comp_avg_256 = _mm256_set1_epi32(offset_comp_avg);
                        xy_y_convolve_2tap_8x2_avx2(im, s_128, &coeffs_256, r);
                        jnt_2d_comp_avg_round_store_8x2_avx2(
                            r, factor_256, offset_comp_avg_256, dst, dst_stride, dst8, dst8_stride);
                        im += 2 * 8;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                } else {
                    do {
                        xy_y_convolve_2tap_8x2_avx2(im, s_128, &coeffs_256, r);
                        jnt_2d_avg_round_store_8x2_avx2(r, offset_avg_256, dst, dst_stride, dst8, dst8_stride);
                        im += 2 * 8;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                }
            } else {
                do {
                    xy_y_convolve_2tap_8x2_avx2(im, s_128, &coeffs_256, r);
                    jnt_2d_no_avg_round_store_8x2_avx2(r, offset_no_avg_256, dst, dst_stride);
                    im += 2 * 8;
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            }
        } else if (w == 16) {
            __m256i s_256[2], r[4];

            s_256[0] = _mm256_loadu_si256((__m256i*)im);

            if (conv_params->do_average) {
                if (conv_params->use_jnt_comp_avg) {
                    const int32_t round_offset    = 1 << (offset_bits - round_1);
                    const int32_t factor          = conv_params->fwd_offset | (conv_params->bck_offset << 16);
                    const int32_t offset_comp_avg = (round_offset + (round_offset >> 1)) * conv_params->bck_offset -
                        (round_offset << DIST_PRECISION_BITS) - (round_offset << (DIST_PRECISION_BITS - 1)) +
                        (1 << (round_bits + DIST_PRECISION_BITS - 1));
                    const __m256i factor_256          = _mm256_set1_epi32(factor);
                    const __m256i offset_comp_avg_256 = _mm256_set1_epi32(offset_comp_avg);
                    do {
                        xy_y_convolve_2tap_16x2_avx2(im, s_256, &coeffs_256, r);
                        jnt_2d_comp_avg_round_store_16x2_avx2(
                            r, factor_256, offset_comp_avg_256, dst, dst_stride, dst8, dst8_stride);
                        im += 2 * 16;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                } else {
                    do {
                        xy_y_convolve_2tap_16x2_avx2(im, s_256, &coeffs_256, r);
                        jnt_2d_avg_round_store_16x2_avx2(r, offset_avg_256, dst, dst_stride, dst8, dst8_stride);
                        im += 2 * 16;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                }
            } else {
                do {
                    xy_y_convolve_2tap_16x2_avx2(im, s_256, &coeffs_256, r);
                    jnt_2d_no_avg_round_store_16x2_avx2(r, offset_no_avg_256, dst, dst_stride);
                    im += 2 * 16;
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            }
        } else if (w == 32) {
            __m256i s_256[2][2], r[4];

            s_256[0][0] = _mm256_loadu_si256((__m256i*)(im + 0 * 16));
            s_256[0][1] = _mm256_loadu_si256((__m256i*)(im + 1 * 16));

            if (conv_params->do_average) {
                if (conv_params->use_jnt_comp_avg) {
                    do {
                        const int32_t round_offset    = 1 << (offset_bits - round_1);
                        const int32_t factor          = conv_params->fwd_offset | (conv_params->bck_offset << 16);
                        const int32_t offset_comp_avg = (round_offset + (round_offset >> 1)) * conv_params->bck_offset -
                            (round_offset << DIST_PRECISION_BITS) - (round_offset << (DIST_PRECISION_BITS - 1)) +
                            (1 << (round_bits + DIST_PRECISION_BITS - 1));
                        const __m256i factor_256          = _mm256_set1_epi32(factor);
                        const __m256i offset_comp_avg_256 = _mm256_set1_epi32(offset_comp_avg);
                        xy_y_convolve_2tap_32_avx2(im + 1 * 32, s_256[0], s_256[1], &coeffs_256, r);
                        jnt_2d_comp_avg_round_store_32_avx2(r + 0, r + 2, factor_256, offset_comp_avg_256, dst, dst8);

                        xy_y_convolve_2tap_32_avx2(im + 2 * 32, s_256[1], s_256[0], &coeffs_256, r);
                        jnt_2d_comp_avg_round_store_32_avx2(
                            r + 0, r + 2, factor_256, offset_comp_avg_256, dst + dst_stride, dst8 + dst8_stride);

                        im += 2 * 32;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                } else {
                    do {
                        xy_y_convolve_2tap_32_avx2(im + 1 * 32, s_256[0], s_256[1], &coeffs_256, r);
                        jnt_2d_avg_round_store_32_avx2(r + 0, r + 2, offset_avg_256, dst, dst8);

                        xy_y_convolve_2tap_32_avx2(im + 2 * 32, s_256[1], s_256[0], &coeffs_256, r);
                        jnt_2d_avg_round_store_32_avx2(
                            r + 0, r + 2, offset_avg_256, dst + dst_stride, dst8 + dst8_stride);

                        im += 2 * 32;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                }
            } else {
                do {
                    xy_y_convolve_2tap_32_avx2(im + 1 * 32, s_256[0], s_256[1], &coeffs_256, r);
                    jnt_2d_no_avg_round_store_32_avx2(r + 0, r + 2, offset_no_avg_256, dst);

                    xy_y_convolve_2tap_32_avx2(im + 2 * 32, s_256[1], s_256[0], &coeffs_256, r);
                    jnt_2d_no_avg_round_store_32_avx2(r + 0, r + 2, offset_no_avg_256, dst + dst_stride);

                    im += 2 * 32;
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            }
        } else if (w == 64) {
            __m256i s_256[2][4];

            s_256[0][0] = _mm256_loadu_si256((__m256i*)(im + 0 * 16));
            s_256[0][1] = _mm256_loadu_si256((__m256i*)(im + 1 * 16));
            s_256[0][2] = _mm256_loadu_si256((__m256i*)(im + 2 * 16));
            s_256[0][3] = _mm256_loadu_si256((__m256i*)(im + 3 * 16));

            if (conv_params->do_average) {
                if (conv_params->use_jnt_comp_avg) {
                    const int32_t round_offset    = 1 << (offset_bits - round_1);
                    const int32_t factor          = conv_params->fwd_offset | (conv_params->bck_offset << 16);
                    const int32_t offset_comp_avg = (round_offset + (round_offset >> 1)) * conv_params->bck_offset -
                        (round_offset << DIST_PRECISION_BITS) - (round_offset << (DIST_PRECISION_BITS - 1)) +
                        (1 << (round_bits + DIST_PRECISION_BITS - 1));
                    const __m256i factor_256          = _mm256_set1_epi32(factor);
                    const __m256i offset_comp_avg_256 = _mm256_set1_epi32(offset_comp_avg);
                    do {
                        __m256i r[4];

                        xy_y_convolve_2tap_32_avx2(im + 2 * 32, s_256[0] + 0, s_256[1] + 0, &coeffs_256, r);
                        jnt_2d_comp_avg_round_store_32_avx2(r + 0, r + 2, factor_256, offset_comp_avg_256, dst, dst8);

                        xy_y_convolve_2tap_32_avx2(im + 3 * 32, s_256[0] + 2, s_256[1] + 2, &coeffs_256, r);
                        jnt_2d_comp_avg_round_store_32_avx2(
                            r + 0, r + 2, factor_256, offset_comp_avg_256, dst + 32, dst8 + 32);
                        im += 2 * 64;

                        xy_y_convolve_2tap_32_avx2(im + 0 * 32, s_256[1] + 0, s_256[0] + 0, &coeffs_256, r);
                        jnt_2d_comp_avg_round_store_32_avx2(
                            r + 0, r + 2, factor_256, offset_comp_avg_256, dst + dst8_stride, dst8 + dst8_stride);

                        xy_y_convolve_2tap_32_avx2(im + 1 * 32, s_256[1] + 2, s_256[0] + 2, &coeffs_256, r);
                        jnt_2d_comp_avg_round_store_32_avx2(r + 0,
                                                            r + 2,
                                                            factor_256,
                                                            offset_comp_avg_256,
                                                            dst + dst8_stride + 32,
                                                            dst8 + dst8_stride + 32);

                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                } else {
                    do {
                        __m256i r[4];

                        xy_y_convolve_2tap_32_avx2(im + 2 * 32, s_256[0] + 0, s_256[1] + 0, &coeffs_256, r);
                        jnt_2d_avg_round_store_32_avx2(r + 0, r + 2, offset_avg_256, dst, dst8);

                        xy_y_convolve_2tap_32_avx2(im + 3 * 32, s_256[0] + 2, s_256[1] + 2, &coeffs_256, r);
                        jnt_2d_avg_round_store_32_avx2(r + 0, r + 2, offset_avg_256, dst + 32, dst8 + 32);
                        im += 2 * 64;

                        xy_y_convolve_2tap_32_avx2(im + 0 * 32, s_256[1] + 0, s_256[0] + 0, &coeffs_256, r);
                        jnt_2d_avg_round_store_32_avx2(
                            r + 0, r + 2, offset_avg_256, dst + dst_stride, dst8 + dst8_stride);

                        xy_y_convolve_2tap_32_avx2(im + 1 * 32, s_256[1] + 2, s_256[0] + 2, &coeffs_256, r);
                        jnt_2d_avg_round_store_32_avx2(
                            r + 0, r + 2, offset_avg_256, dst + dst_stride + 32, dst8 + dst8_stride + 32);

                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                }
            } else {
                do {
                    __m256i r[4];

                    xy_y_convolve_2tap_32_avx2(im + 2 * 32, s_256[0] + 0, s_256[1] + 0, &coeffs_256, r);
                    jnt_2d_no_avg_round_store_32_avx2(r + 0, r + 2, offset_no_avg_256, dst);

                    xy_y_convolve_2tap_32_avx2(im + 3 * 32, s_256[0] + 2, s_256[1] + 2, &coeffs_256, r);
                    jnt_2d_no_avg_round_store_32_avx2(r + 0, r + 2, offset_no_avg_256, dst + 32);
                    im += 2 * 64;

                    xy_y_convolve_2tap_32_avx2(im + 0 * 32, s_256[1] + 0, s_256[0] + 0, &coeffs_256, r);
                    jnt_2d_no_avg_round_store_32_avx2(r + 0, r + 2, offset_no_avg_256, dst + dst_stride);

                    xy_y_convolve_2tap_32_avx2(im + 1 * 32, s_256[1] + 2, s_256[0] + 2, &coeffs_256, r);
                    jnt_2d_no_avg_round_store_32_avx2(r + 0, r + 2, offset_no_avg_256, dst + dst_stride + 32);

                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            }
        } else {
            __m256i s_256[2][8];

            assert(w == 128);

            load_16bit_8rows_avx2(im, 16, s_256[0]);

            if (conv_params->do_average) {
                if (conv_params->use_jnt_comp_avg) {
                    const int32_t round_offset    = 1 << (offset_bits - round_1);
                    const int32_t factor          = conv_params->fwd_offset | (conv_params->bck_offset << 16);
                    const int32_t offset_comp_avg = (round_offset + (round_offset >> 1)) * conv_params->bck_offset -
                        (round_offset << DIST_PRECISION_BITS) - (round_offset << (DIST_PRECISION_BITS - 1)) +
                        (1 << (round_bits + DIST_PRECISION_BITS - 1));
                    const __m256i factor_256          = _mm256_set1_epi32(factor);
                    const __m256i offset_comp_avg_256 = _mm256_set1_epi32(offset_comp_avg);
                    do {
                        __m256i r[4];

                        xy_y_convolve_2tap_32_avx2(im + 4 * 32, s_256[0] + 0, s_256[1] + 0, &coeffs_256, r);
                        jnt_2d_comp_avg_round_store_32_avx2(r + 0, r + 2, factor_256, offset_comp_avg_256, dst, dst8);

                        xy_y_convolve_2tap_32_avx2(im + 5 * 32, s_256[0] + 2, s_256[1] + 2, &coeffs_256, r);
                        jnt_2d_comp_avg_round_store_32_avx2(
                            r + 0, r + 2, factor_256, offset_comp_avg_256, dst + 1 * 32, dst8 + 1 * 32);

                        xy_y_convolve_2tap_32_avx2(im + 6 * 32, s_256[0] + 4, s_256[1] + 4, &coeffs_256, r);
                        jnt_2d_comp_avg_round_store_32_avx2(
                            r + 0, r + 2, factor_256, offset_comp_avg_256, dst + 2 * 32, dst8 + 2 * 32);

                        xy_y_convolve_2tap_32_avx2(im + 7 * 32, s_256[0] + 6, s_256[1] + 6, &coeffs_256, r);
                        jnt_2d_comp_avg_round_store_32_avx2(
                            r + 0, r + 2, factor_256, offset_comp_avg_256, dst + 3 * 32, dst8 + 3 * 32);
                        im += 2 * 128;

                        xy_y_convolve_2tap_32_avx2(im + 0 * 32, s_256[1] + 0, s_256[0] + 0, &coeffs_256, r);
                        jnt_2d_comp_avg_round_store_32_avx2(r + 0,
                                                            r + 2,
                                                            factor_256,
                                                            offset_comp_avg_256,
                                                            dst + dst8_stride + 0 * 32,
                                                            dst8 + dst8_stride + 0 * 32);

                        xy_y_convolve_2tap_32_avx2(im + 1 * 32, s_256[1] + 2, s_256[0] + 2, &coeffs_256, r);
                        jnt_2d_comp_avg_round_store_32_avx2(r + 0,
                                                            r + 2,
                                                            factor_256,
                                                            offset_comp_avg_256,
                                                            dst + dst8_stride + 1 * 32,
                                                            dst8 + dst8_stride + 1 * 32);

                        xy_y_convolve_2tap_32_avx2(im + 2 * 32, s_256[1] + 4, s_256[0] + 4, &coeffs_256, r);
                        jnt_2d_comp_avg_round_store_32_avx2(r + 0,
                                                            r + 2,
                                                            factor_256,
                                                            offset_comp_avg_256,
                                                            dst + dst8_stride + 2 * 32,
                                                            dst8 + dst8_stride + 2 * 32);

                        xy_y_convolve_2tap_32_avx2(im + 3 * 32, s_256[1] + 6, s_256[0] + 6, &coeffs_256, r);
                        jnt_2d_comp_avg_round_store_32_avx2(r + 0,
                                                            r + 2,
                                                            factor_256,
                                                            offset_comp_avg_256,
                                                            dst + dst8_stride + 3 * 32,
                                                            dst8 + dst8_stride + 3 * 32);

                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                } else {
                    do {
                        __m256i r[4];

                        xy_y_convolve_2tap_32_avx2(im + 4 * 32, s_256[0] + 0, s_256[1] + 0, &coeffs_256, r);
                        jnt_2d_avg_round_store_32_avx2(r + 0, r + 2, offset_avg_256, dst + 0 * 32, dst8 + 0 * 32);

                        xy_y_convolve_2tap_32_avx2(im + 5 * 32, s_256[0] + 2, s_256[1] + 2, &coeffs_256, r);
                        jnt_2d_avg_round_store_32_avx2(r + 0, r + 2, offset_avg_256, dst + 1 * 32, dst8 + 1 * 32);

                        xy_y_convolve_2tap_32_avx2(im + 6 * 32, s_256[0] + 4, s_256[1] + 4, &coeffs_256, r);
                        jnt_2d_avg_round_store_32_avx2(r + 0, r + 2, offset_avg_256, dst + 2 * 32, dst8 + 2 * 32);

                        xy_y_convolve_2tap_32_avx2(im + 7 * 32, s_256[0] + 6, s_256[1] + 6, &coeffs_256, r);
                        jnt_2d_avg_round_store_32_avx2(r + 0, r + 2, offset_avg_256, dst + 3 * 32, dst8 + 3 * 32);
                        im += 2 * 128;

                        xy_y_convolve_2tap_32_avx2(im + 0 * 32, s_256[1] + 0, s_256[0] + 0, &coeffs_256, r);
                        jnt_2d_avg_round_store_32_avx2(
                            r + 0, r + 2, offset_avg_256, dst + dst_stride + 0 * 32, dst8 + dst8_stride + 0 * 32);

                        xy_y_convolve_2tap_32_avx2(im + 1 * 32, s_256[1] + 2, s_256[0] + 2, &coeffs_256, r);
                        jnt_2d_avg_round_store_32_avx2(
                            r + 0, r + 2, offset_avg_256, dst + dst_stride + 1 * 32, dst8 + dst8_stride + 1 * 32);

                        xy_y_convolve_2tap_32_avx2(im + 2 * 32, s_256[1] + 4, s_256[0] + 4, &coeffs_256, r);
                        jnt_2d_avg_round_store_32_avx2(
                            r + 0, r + 2, offset_avg_256, dst + dst_stride + 2 * 32, dst8 + dst8_stride + 2 * 32);

                        xy_y_convolve_2tap_32_avx2(im + 3 * 32, s_256[1] + 6, s_256[0] + 6, &coeffs_256, r);
                        jnt_2d_avg_round_store_32_avx2(
                            r + 0, r + 2, offset_avg_256, dst + dst_stride + 3 * 32, dst8 + dst8_stride + 3 * 32);

                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                }
            } else {
                do {
                    __m256i r[4];

                    xy_y_convolve_2tap_32_avx2(im + 4 * 32, s_256[0] + 0, s_256[1] + 0, &coeffs_256, r);
                    jnt_2d_no_avg_round_store_32_avx2(r + 0, r + 2, offset_no_avg_256, dst + 0 * 32);

                    xy_y_convolve_2tap_32_avx2(im + 5 * 32, s_256[0] + 2, s_256[1] + 2, &coeffs_256, r);
                    jnt_2d_no_avg_round_store_32_avx2(r + 0, r + 2, offset_no_avg_256, dst + 1 * 32);

                    xy_y_convolve_2tap_32_avx2(im + 6 * 32, s_256[0] + 4, s_256[1] + 4, &coeffs_256, r);
                    jnt_2d_no_avg_round_store_32_avx2(r + 0, r + 2, offset_no_avg_256, dst + 2 * 32);

                    xy_y_convolve_2tap_32_avx2(im + 7 * 32, s_256[0] + 6, s_256[1] + 6, &coeffs_256, r);
                    jnt_2d_no_avg_round_store_32_avx2(r + 0, r + 2, offset_no_avg_256, dst + 3 * 32);
                    im += 2 * 128;

                    xy_y_convolve_2tap_32_avx2(im + 0 * 32, s_256[1] + 0, s_256[0] + 0, &coeffs_256, r);
                    jnt_2d_no_avg_round_store_32_avx2(r + 0, r + 2, offset_no_avg_256, dst + dst_stride + 0 * 32);

                    xy_y_convolve_2tap_32_avx2(im + 1 * 32, s_256[1] + 2, s_256[0] + 2, &coeffs_256, r);
                    jnt_2d_no_avg_round_store_32_avx2(r + 0, r + 2, offset_no_avg_256, dst + dst_stride + 1 * 32);

                    xy_y_convolve_2tap_32_avx2(im + 2 * 32, s_256[1] + 4, s_256[0] + 4, &coeffs_256, r);
                    jnt_2d_no_avg_round_store_32_avx2(r + 0, r + 2, offset_no_avg_256, dst + dst_stride + 2 * 32);

                    xy_y_convolve_2tap_32_avx2(im + 3 * 32, s_256[1] + 6, s_256[0] + 6, &coeffs_256, r);
                    jnt_2d_no_avg_round_store_32_avx2(r + 0, r + 2, offset_no_avg_256, dst + dst_stride + 3 * 32);

                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            }
        }
    }
}

static void jnt_convolve_2d_ver_2tap_half_avx2(const int16_t* const im_block, const int32_t w, const int32_t h,
                                               const InterpFilterParams* const filter_params_y,
                                               const int32_t subpel_y_q4, const ConvolveParams* const conv_params,
                                               uint8_t* dst8, const int32_t dst8_stride) {
    const int32_t  dst_stride  = conv_params->dst_stride;
    const int32_t  bd          = 8;
    const int32_t  round_0     = 3;
    const int16_t* im          = im_block;
    const int32_t  round_1     = COMPOUND_ROUND1_BITS;
    const int32_t  offset_bits = bd + 2 * FILTER_BITS - round_0; // 19
    const int32_t  round_bits  = 2 * FILTER_BITS - round_0 - round_1; // 4
    const int32_t  offset_avg  = (1 << (round_1 - COMPOUND_ROUND1_BITS)) +
        (1 << (round_bits + round_1 - COMPOUND_ROUND1_BITS + 1)) - (1 << (offset_bits - COMPOUND_ROUND1_BITS + 1)) -
        (1 << (offset_bits - COMPOUND_ROUND1_BITS));
    const int32_t offset_no_avg = (1 << (round_1 - COMPOUND_ROUND1_BITS)) +
        (1 << (offset_bits - COMPOUND_ROUND1_BITS + 1)) + (1 << (offset_bits - COMPOUND_ROUND1_BITS));
    ConvBufType* dst = conv_params->dst;
    int32_t      y   = h;

    (void)filter_params_y;
    (void)subpel_y_q4;

    if (w <= 4) {
        const __m128i offset_avg_128    = _mm_set1_epi16(offset_avg);
        const __m128i offset_no_avg_128 = _mm_set1_epi16(offset_no_avg);

        if (w == 2) {
            __m128i s_32[2];

            s_32[0] = _mm_cvtsi32_si128(*(int32_t*)im);

            if (conv_params->do_average) {
                if (conv_params->use_jnt_comp_avg) {
                    const int32_t round_offset    = 1 << (offset_bits - round_1);
                    const int32_t factor          = conv_params->fwd_offset | (conv_params->bck_offset << 16);
                    const int32_t offset_comp_avg = (round_offset + (round_offset >> 1)) * conv_params->bck_offset -
                        (round_offset << DIST_PRECISION_BITS) - (round_offset << (DIST_PRECISION_BITS - 1)) +
                        (1 << (round_bits + DIST_PRECISION_BITS - 1));
                    const __m128i factor_128          = _mm_set1_epi32(factor);
                    const __m128i offset_comp_avg_128 = _mm_set1_epi32(offset_comp_avg);
                    do {
                        const __m128i res = xy_y_convolve_2tap_2x2_half_pel_sse2(im, s_32);
                        jnt_2d_comp_avg_round_store_half_pel_2x2_sse2(
                            res, factor_128, offset_comp_avg_128, dst, dst_stride, dst8, dst8_stride);
                        im += 2 * 2;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                } else {
                    do {
                        const __m128i res = xy_y_convolve_2tap_2x2_half_pel_sse2(im, s_32);
                        jnt_2d_avg_round_store_half_pel_2x2_sse2(
                            res, offset_avg_128, dst, dst_stride, dst8, dst8_stride);
                        im += 2 * 2;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                }
            } else {
                do {
                    const __m128i res = xy_y_convolve_2tap_2x2_half_pel_sse2(im, s_32);
                    jnt_2d_no_avg_round_store_half_pel_2x2_sse2(res, offset_no_avg_128, dst, dst_stride);
                    im += 2 * 2;
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            }
        } else {
            __m128i s_64[2];

            assert(w == 4);

            s_64[0] = _mm_loadl_epi64((__m128i*)im);

            if (conv_params->do_average) {
                if (conv_params->use_jnt_comp_avg) {
                    const int32_t round_offset    = 1 << (offset_bits - round_1);
                    const int32_t factor          = conv_params->fwd_offset | (conv_params->bck_offset << 16);
                    const int32_t offset_comp_avg = (round_offset + (round_offset >> 1)) * conv_params->bck_offset -
                        (round_offset << DIST_PRECISION_BITS) - (round_offset << (DIST_PRECISION_BITS - 1)) +
                        (1 << (round_bits + DIST_PRECISION_BITS - 1));
                    const __m128i factor_128          = _mm_set1_epi32(factor);
                    const __m128i offset_comp_avg_128 = _mm_set1_epi32(offset_comp_avg);
                    do {
                        const __m128i res = xy_y_convolve_2tap_4x2_half_pel_sse2(im, s_64);
                        jnt_2d_comp_avg_round_store_half_pel_4x2_sse2(
                            res, factor_128, offset_comp_avg_128, dst, dst_stride, dst8, dst8_stride);
                        im += 2 * 4;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                } else {
                    do {
                        const __m128i res = xy_y_convolve_2tap_4x2_half_pel_sse2(im, s_64);
                        jnt_2d_avg_round_store_half_pel_4x2_sse2(
                            res, offset_avg_128, dst, dst_stride, dst8, dst8_stride);
                        im += 2 * 4;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                }
            } else {
                do {
                    const __m128i res = xy_y_convolve_2tap_4x2_half_pel_sse2(im, s_64);
                    jnt_2d_no_avg_round_store_half_pel_4x2_sse2(res, offset_no_avg_128, dst, dst_stride);
                    im += 2 * 4;
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            }
        }
    } else {
        const __m256i offset_avg_256    = _mm256_set1_epi16(offset_avg);
        const __m256i offset_no_avg_256 = _mm256_set1_epi16(offset_no_avg);

        if (w == 8) {
            __m128i s_128[2];

            s_128[0] = _mm_loadu_si128((__m128i*)im);

            if (conv_params->do_average) {
                if (conv_params->use_jnt_comp_avg) {
                    const int32_t round_offset    = 1 << (offset_bits - round_1);
                    const int32_t factor          = conv_params->fwd_offset | (conv_params->bck_offset << 16);
                    const int32_t offset_comp_avg = (round_offset + (round_offset >> 1)) * conv_params->bck_offset -
                        (round_offset << DIST_PRECISION_BITS) - (round_offset << (DIST_PRECISION_BITS - 1)) +
                        (1 << (round_bits + DIST_PRECISION_BITS - 1));
                    const __m256i factor_256          = _mm256_set1_epi32(factor);
                    const __m256i offset_comp_avg_256 = _mm256_set1_epi32(offset_comp_avg);
                    do {
                        const __m256i res = xy_y_convolve_2tap_8x2_half_pel_avx2(im, s_128);
                        jnt_2d_comp_avg_round_store_half_pel_8x2_avx2(
                            res, factor_256, offset_comp_avg_256, dst, dst_stride, dst8, dst8_stride);
                        im += 2 * 8;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                } else {
                    do {
                        const __m256i res = xy_y_convolve_2tap_8x2_half_pel_avx2(im, s_128);
                        jnt_2d_avg_round_store_half_pel_8x2_avx2(
                            res, offset_avg_256, dst, dst_stride, dst8, dst8_stride);
                        im += 2 * 8;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                }
            } else {
                do {
                    const __m256i res = xy_y_convolve_2tap_8x2_half_pel_avx2(im, s_128);
                    jnt_2d_no_avg_round_store_half_pel_8x2_avx2(res, offset_no_avg_256, dst, dst_stride);
                    im += 2 * 8;
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            }
        } else if (w == 16) {
            __m256i s_256[2], r[2];

            s_256[0] = _mm256_loadu_si256((__m256i*)im);

            if (conv_params->do_average) {
                if (conv_params->use_jnt_comp_avg) {
                    const int32_t round_offset    = 1 << (offset_bits - round_1);
                    const int32_t factor          = conv_params->fwd_offset | (conv_params->bck_offset << 16);
                    const int32_t offset_comp_avg = (round_offset + (round_offset >> 1)) * conv_params->bck_offset -
                        (round_offset << DIST_PRECISION_BITS) - (round_offset << (DIST_PRECISION_BITS - 1)) +
                        (1 << (round_bits + DIST_PRECISION_BITS - 1));
                    const __m256i factor_256          = _mm256_set1_epi32(factor);
                    const __m256i offset_comp_avg_256 = _mm256_set1_epi32(offset_comp_avg);
                    do {
                        xy_y_convolve_2tap_16x2_half_pel_avx2(im, s_256, r);
                        jnt_2d_comp_avg_round_store_half_pel_16x2_avx2(
                            r, factor_256, offset_comp_avg_256, dst, dst_stride, dst8, dst8_stride);
                        im += 2 * 16;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                } else {
                    do {
                        xy_y_convolve_2tap_16x2_half_pel_avx2(im, s_256, r);
                        jnt_2d_avg_round_store_half_pel_16x2_avx2(
                            r, offset_avg_256, dst, dst_stride, dst8, dst8_stride);
                        im += 2 * 16;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                }
            } else {
                do {
                    xy_y_convolve_2tap_16x2_half_pel_avx2(im, s_256, r);
                    jnt_2d_no_avg_round_store_half_pel_16x2_avx2(r, offset_no_avg_256, dst, dst_stride);
                    im += 2 * 16;
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            }
        } else if (w == 32) {
            __m256i s_256[2][2], r[2];

            s_256[0][0] = _mm256_loadu_si256((__m256i*)(im + 0 * 16));
            s_256[0][1] = _mm256_loadu_si256((__m256i*)(im + 1 * 16));

            if (conv_params->do_average) {
                if (conv_params->use_jnt_comp_avg) {
                    const int32_t round_offset    = 1 << (offset_bits - round_1);
                    const int32_t factor          = conv_params->fwd_offset | (conv_params->bck_offset << 16);
                    const int32_t offset_comp_avg = (round_offset + (round_offset >> 1)) * conv_params->bck_offset -
                        (round_offset << DIST_PRECISION_BITS) - (round_offset << (DIST_PRECISION_BITS - 1)) +
                        (1 << (round_bits + DIST_PRECISION_BITS - 1));
                    const __m256i factor_256          = _mm256_set1_epi32(factor);
                    const __m256i offset_comp_avg_256 = _mm256_set1_epi32(offset_comp_avg);
                    do {
                        xy_y_convolve_2tap_half_pel_32_avx2(im + 1 * 32, s_256[0], s_256[1], r);
                        jnt_2d_comp_avg_round_store_half_pel_32_avx2(r, factor_256, offset_comp_avg_256, dst, dst8);

                        xy_y_convolve_2tap_half_pel_32_avx2(im + 2 * 32, s_256[1], s_256[0], r);
                        jnt_2d_comp_avg_round_store_half_pel_32_avx2(
                            r, factor_256, offset_comp_avg_256, dst + dst_stride, dst8 + dst8_stride);

                        im += 2 * 32;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                } else {
                    do {
                        xy_y_convolve_2tap_half_pel_32_avx2(im + 1 * 32, s_256[0], s_256[1], r);
                        jnt_2d_avg_round_store_half_pel_32_avx2(r, offset_avg_256, dst, dst8);

                        xy_y_convolve_2tap_half_pel_32_avx2(im + 2 * 32, s_256[1], s_256[0], r);
                        jnt_2d_avg_round_store_half_pel_32_avx2(
                            r, offset_avg_256, dst + dst_stride, dst8 + dst8_stride);

                        im += 2 * 32;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                }
            } else {
                do {
                    xy_y_convolve_2tap_half_pel_32_avx2(im + 1 * 32, s_256[0], s_256[1], r);
                    jnt_2d_no_avg_round_store_half_pel_32_avx2(r, offset_no_avg_256, dst);

                    xy_y_convolve_2tap_half_pel_32_avx2(im + 2 * 32, s_256[1], s_256[0], r);
                    jnt_2d_no_avg_round_store_half_pel_32_avx2(r, offset_no_avg_256, dst + dst_stride);

                    im += 2 * 32;
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            }
        } else if (w == 64) {
            __m256i s_256[2][4];

            s_256[0][0] = _mm256_loadu_si256((__m256i*)(im + 0 * 16));
            s_256[0][1] = _mm256_loadu_si256((__m256i*)(im + 1 * 16));
            s_256[0][2] = _mm256_loadu_si256((__m256i*)(im + 2 * 16));
            s_256[0][3] = _mm256_loadu_si256((__m256i*)(im + 3 * 16));

            if (conv_params->do_average) {
                if (conv_params->use_jnt_comp_avg) {
                    const int32_t round_offset    = 1 << (offset_bits - round_1);
                    const int32_t factor          = conv_params->fwd_offset | (conv_params->bck_offset << 16);
                    const int32_t offset_comp_avg = (round_offset + (round_offset >> 1)) * conv_params->bck_offset -
                        (round_offset << DIST_PRECISION_BITS) - (round_offset << (DIST_PRECISION_BITS - 1)) +
                        (1 << (round_bits + DIST_PRECISION_BITS - 1));
                    const __m256i factor_256          = _mm256_set1_epi32(factor);
                    const __m256i offset_comp_avg_256 = _mm256_set1_epi32(offset_comp_avg);
                    do {
                        __m256i r[2];

                        xy_y_convolve_2tap_half_pel_32_avx2(im + 2 * 32, s_256[0] + 0, s_256[1] + 0, r);
                        jnt_2d_comp_avg_round_store_half_pel_32_avx2(r, factor_256, offset_comp_avg_256, dst, dst8);

                        xy_y_convolve_2tap_half_pel_32_avx2(im + 3 * 32, s_256[0] + 2, s_256[1] + 2, r);
                        jnt_2d_comp_avg_round_store_half_pel_32_avx2(
                            r, factor_256, offset_comp_avg_256, dst + 32, dst8 + 32);
                        im += 2 * 64;

                        xy_y_convolve_2tap_half_pel_32_avx2(im + 0 * 32, s_256[1] + 0, s_256[0] + 0, r);
                        jnt_2d_comp_avg_round_store_half_pel_32_avx2(
                            r, factor_256, offset_comp_avg_256, dst + dst_stride, dst8 + dst8_stride);

                        xy_y_convolve_2tap_half_pel_32_avx2(im + 1 * 32, s_256[1] + 2, s_256[0] + 2, r);
                        jnt_2d_comp_avg_round_store_half_pel_32_avx2(
                            r, factor_256, offset_comp_avg_256, dst + dst_stride + 32, dst8 + dst8_stride + 32);

                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                } else {
                    do {
                        __m256i r[2];

                        xy_y_convolve_2tap_half_pel_32_avx2(im + 2 * 32, s_256[0] + 0, s_256[1] + 0, r);
                        jnt_2d_avg_round_store_half_pel_32_avx2(r, offset_avg_256, dst, dst8);

                        xy_y_convolve_2tap_half_pel_32_avx2(im + 3 * 32, s_256[0] + 2, s_256[1] + 2, r);
                        jnt_2d_avg_round_store_half_pel_32_avx2(r, offset_avg_256, dst + 32, dst8 + 32);
                        im += 2 * 64;

                        xy_y_convolve_2tap_half_pel_32_avx2(im + 0 * 32, s_256[1] + 0, s_256[0] + 0, r);
                        jnt_2d_avg_round_store_half_pel_32_avx2(
                            r, offset_avg_256, dst + dst_stride, dst8 + dst8_stride);

                        xy_y_convolve_2tap_half_pel_32_avx2(im + 1 * 32, s_256[1] + 2, s_256[0] + 2, r);
                        jnt_2d_avg_round_store_half_pel_32_avx2(
                            r, offset_avg_256, dst + dst_stride + 32, dst8 + dst8_stride + 32);

                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                }
            } else {
                do {
                    __m256i r[2];

                    xy_y_convolve_2tap_half_pel_32_avx2(im + 2 * 32, s_256[0] + 0, s_256[1] + 0, r);
                    jnt_2d_no_avg_round_store_half_pel_32_avx2(r, offset_no_avg_256, dst);

                    xy_y_convolve_2tap_half_pel_32_avx2(im + 3 * 32, s_256[0] + 2, s_256[1] + 2, r);
                    jnt_2d_no_avg_round_store_half_pel_32_avx2(r, offset_no_avg_256, dst + 32);
                    im += 2 * 64;

                    xy_y_convolve_2tap_half_pel_32_avx2(im + 0 * 32, s_256[1] + 0, s_256[0] + 0, r);
                    jnt_2d_no_avg_round_store_half_pel_32_avx2(r, offset_no_avg_256, dst + dst_stride);

                    xy_y_convolve_2tap_half_pel_32_avx2(im + 1 * 32, s_256[1] + 2, s_256[0] + 2, r);
                    jnt_2d_no_avg_round_store_half_pel_32_avx2(r, offset_no_avg_256, dst + dst_stride + 32);

                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            }
        } else {
            __m256i s_256[2][8];

            assert(w == 128);

            load_16bit_8rows_avx2(im, 16, s_256[0]);

            if (conv_params->do_average) {
                if (conv_params->use_jnt_comp_avg) {
                    const int32_t round_offset    = 1 << (offset_bits - round_1);
                    const int32_t factor          = conv_params->fwd_offset | (conv_params->bck_offset << 16);
                    const int32_t offset_comp_avg = (round_offset + (round_offset >> 1)) * conv_params->bck_offset -
                        (round_offset << DIST_PRECISION_BITS) - (round_offset << (DIST_PRECISION_BITS - 1)) +
                        (1 << (round_bits + DIST_PRECISION_BITS - 1));
                    const __m256i factor_256          = _mm256_set1_epi32(factor);
                    const __m256i offset_comp_avg_256 = _mm256_set1_epi32(offset_comp_avg);
                    do {
                        __m256i r[2];

                        xy_y_convolve_2tap_half_pel_32_avx2(im + 4 * 32, s_256[0] + 0, s_256[1] + 0, r);
                        jnt_2d_comp_avg_round_store_half_pel_32_avx2(
                            r, factor_256, offset_comp_avg_256, dst + 0 * 32, dst8 + 0 * 32);

                        xy_y_convolve_2tap_half_pel_32_avx2(im + 5 * 32, s_256[0] + 2, s_256[1] + 2, r);
                        jnt_2d_comp_avg_round_store_half_pel_32_avx2(
                            r, factor_256, offset_comp_avg_256, dst + 1 * 32, dst8 + 1 * 32);

                        xy_y_convolve_2tap_half_pel_32_avx2(im + 6 * 32, s_256[0] + 4, s_256[1] + 4, r);
                        jnt_2d_comp_avg_round_store_half_pel_32_avx2(
                            r, factor_256, offset_comp_avg_256, dst + 2 * 32, dst8 + 2 * 32);

                        xy_y_convolve_2tap_half_pel_32_avx2(im + 7 * 32, s_256[0] + 6, s_256[1] + 6, r);
                        jnt_2d_comp_avg_round_store_half_pel_32_avx2(
                            r, factor_256, offset_comp_avg_256, dst + 3 * 32, dst8 + 3 * 32);
                        im += 2 * 128;

                        xy_y_convolve_2tap_half_pel_32_avx2(im + 0 * 32, s_256[1] + 0, s_256[0] + 0, r);
                        jnt_2d_comp_avg_round_store_half_pel_32_avx2(
                            r, factor_256, offset_comp_avg_256, dst + dst_stride + 0 * 32, dst8 + dst8_stride + 0 * 32);

                        xy_y_convolve_2tap_half_pel_32_avx2(im + 1 * 32, s_256[1] + 2, s_256[0] + 2, r);
                        jnt_2d_comp_avg_round_store_half_pel_32_avx2(
                            r, factor_256, offset_comp_avg_256, dst + dst_stride + 1 * 32, dst8 + dst8_stride + 1 * 32);

                        xy_y_convolve_2tap_half_pel_32_avx2(im + 2 * 32, s_256[1] + 4, s_256[0] + 4, r);
                        jnt_2d_comp_avg_round_store_half_pel_32_avx2(
                            r, factor_256, offset_comp_avg_256, dst + dst_stride + 2 * 32, dst8 + dst8_stride + 2 * 32);

                        xy_y_convolve_2tap_half_pel_32_avx2(im + 3 * 32, s_256[1] + 6, s_256[0] + 6, r);
                        jnt_2d_comp_avg_round_store_half_pel_32_avx2(
                            r, factor_256, offset_comp_avg_256, dst + dst_stride + 3 * 32, dst8 + dst8_stride + 3 * 32);

                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                } else {
                    do {
                        __m256i r[2];

                        xy_y_convolve_2tap_half_pel_32_avx2(im + 4 * 32, s_256[0] + 0, s_256[1] + 0, r);
                        jnt_2d_avg_round_store_half_pel_32_avx2(r, offset_avg_256, dst + 0 * 32, dst8 + 0 * 32);

                        xy_y_convolve_2tap_half_pel_32_avx2(im + 5 * 32, s_256[0] + 2, s_256[1] + 2, r);
                        jnt_2d_avg_round_store_half_pel_32_avx2(r, offset_avg_256, dst + 1 * 32, dst8 + 1 * 32);

                        xy_y_convolve_2tap_half_pel_32_avx2(im + 6 * 32, s_256[0] + 4, s_256[1] + 4, r);
                        jnt_2d_avg_round_store_half_pel_32_avx2(r, offset_avg_256, dst + 2 * 32, dst8 + 2 * 32);

                        xy_y_convolve_2tap_half_pel_32_avx2(im + 7 * 32, s_256[0] + 6, s_256[1] + 6, r);
                        jnt_2d_avg_round_store_half_pel_32_avx2(r, offset_avg_256, dst + 3 * 32, dst8 + 3 * 32);
                        im += 2 * 128;

                        xy_y_convolve_2tap_half_pel_32_avx2(im + 0 * 32, s_256[1] + 0, s_256[0] + 0, r);
                        jnt_2d_avg_round_store_half_pel_32_avx2(
                            r, offset_avg_256, dst + dst_stride + 0 * 32, dst8 + dst8_stride + 0 * 32);

                        xy_y_convolve_2tap_half_pel_32_avx2(im + 1 * 32, s_256[1] + 2, s_256[0] + 2, r);
                        jnt_2d_avg_round_store_half_pel_32_avx2(
                            r, offset_avg_256, dst + dst_stride + 1 * 32, dst8 + dst8_stride + 1 * 32);

                        xy_y_convolve_2tap_half_pel_32_avx2(im + 2 * 32, s_256[1] + 4, s_256[0] + 4, r);
                        jnt_2d_avg_round_store_half_pel_32_avx2(
                            r, offset_avg_256, dst + dst_stride + 2 * 32, dst8 + dst8_stride + 2 * 32);

                        xy_y_convolve_2tap_half_pel_32_avx2(im + 3 * 32, s_256[1] + 6, s_256[0] + 6, r);
                        jnt_2d_avg_round_store_half_pel_32_avx2(
                            r, offset_avg_256, dst + dst_stride + 3 * 32, dst8 + dst8_stride + 3 * 32);

                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                }
            } else {
                do {
                    __m256i r[2];

                    xy_y_convolve_2tap_half_pel_32_avx2(im + 4 * 32, s_256[0] + 0, s_256[1] + 0, r);
                    jnt_2d_no_avg_round_store_half_pel_32_avx2(r, offset_no_avg_256, dst + 0 * 32);

                    xy_y_convolve_2tap_half_pel_32_avx2(im + 5 * 32, s_256[0] + 2, s_256[1] + 2, r);
                    jnt_2d_no_avg_round_store_half_pel_32_avx2(r, offset_no_avg_256, dst + 1 * 32);

                    xy_y_convolve_2tap_half_pel_32_avx2(im + 6 * 32, s_256[0] + 4, s_256[1] + 4, r);
                    jnt_2d_no_avg_round_store_half_pel_32_avx2(r, offset_no_avg_256, dst + 2 * 32);

                    xy_y_convolve_2tap_half_pel_32_avx2(im + 7 * 32, s_256[0] + 6, s_256[1] + 6, r);
                    jnt_2d_no_avg_round_store_half_pel_32_avx2(r, offset_no_avg_256, dst + 3 * 32);
                    im += 2 * 128;

                    xy_y_convolve_2tap_half_pel_32_avx2(im + 0 * 32, s_256[1] + 0, s_256[0] + 0, r);
                    jnt_2d_no_avg_round_store_half_pel_32_avx2(r, offset_no_avg_256, dst + dst_stride + 0 * 32);

                    xy_y_convolve_2tap_half_pel_32_avx2(im + 1 * 32, s_256[1] + 2, s_256[0] + 2, r);
                    jnt_2d_no_avg_round_store_half_pel_32_avx2(r, offset_no_avg_256, dst + dst_stride + 1 * 32);

                    xy_y_convolve_2tap_half_pel_32_avx2(im + 2 * 32, s_256[1] + 4, s_256[0] + 4, r);
                    jnt_2d_no_avg_round_store_half_pel_32_avx2(r, offset_no_avg_256, dst + dst_stride + 2 * 32);

                    xy_y_convolve_2tap_half_pel_32_avx2(im + 3 * 32, s_256[1] + 6, s_256[0] + 6, r);
                    jnt_2d_no_avg_round_store_half_pel_32_avx2(r, offset_no_avg_256, dst + dst_stride + 3 * 32);

                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            }
        }
    }
}

void jnt_convolve_2d_ver_4tap_avx2(const int16_t* const im_block, const int32_t w, const int32_t h,
                                   const InterpFilterParams* const filter_params_y, const int32_t subpel_y_q4,
                                   const ConvolveParams* const conv_params, uint8_t* dst8, const int32_t dst8_stride) {
    const int32_t  dst_stride  = conv_params->dst_stride;
    const int32_t  bd          = 8;
    const int32_t  round_0     = 3;
    const int16_t* im          = im_block;
    const int32_t  round_1     = COMPOUND_ROUND1_BITS;
    const int32_t  offset_bits = bd + 2 * FILTER_BITS - round_0; // 19
    const int32_t  round_bits  = 2 * FILTER_BITS - round_0 - round_1; // 4
    const int32_t  offset_avg  = (1 << (round_1 - 1)) + (1 << (round_bits + round_1)) - (1 << offset_bits) -
        (1 << (offset_bits - 1));
    const int32_t offset_no_avg = (1 << (round_1 - 1)) + (1 << offset_bits) + (1 << (offset_bits - 1));
    int32_t       y             = h;
    ConvBufType*  dst           = conv_params->dst;

    if (w == 2) {
        const __m128i offset_avg_128    = _mm_set1_epi32(offset_avg);
        const __m128i offset_no_avg_128 = _mm_set1_epi32(offset_no_avg);
        __m128i       coeffs_128[4], s_32[4], ss_128[2];

        prepare_coeffs_4tap_sse2(filter_params_y, subpel_y_q4, coeffs_128);

        s_32[0] = _mm_cvtsi32_si128(*(int32_t*)(im + 0 * 2));
        s_32[1] = _mm_cvtsi32_si128(*(int32_t*)(im + 1 * 2));
        s_32[2] = _mm_cvtsi32_si128(*(int32_t*)(im + 2 * 2));

        const __m128i src01 = _mm_unpacklo_epi32(s_32[0], s_32[1]);
        const __m128i src12 = _mm_unpacklo_epi32(s_32[1], s_32[2]);

        ss_128[0] = _mm_unpacklo_epi16(src01, src12);

        if (conv_params->do_average) {
            if (conv_params->use_jnt_comp_avg) {
                const int32_t round_offset    = 1 << (offset_bits - round_1);
                const int32_t factor          = conv_params->fwd_offset | (conv_params->bck_offset << 16);
                const int32_t offset_comp_avg = (round_offset + (round_offset >> 1)) * conv_params->bck_offset -
                    (round_offset << DIST_PRECISION_BITS) - (round_offset << (DIST_PRECISION_BITS - 1)) +
                    (1 << (round_bits + DIST_PRECISION_BITS - 1));
                const __m128i factor_128          = _mm_set1_epi32(factor);
                const __m128i offset_comp_avg_128 = _mm_set1_epi32(offset_comp_avg);
                do {
                    const __m128i res = xy_y_convolve_4tap_2x2_sse2(im, s_32, ss_128, coeffs_128);
                    jnt_2d_comp_avg_round_store_2x2_sse2(
                        res, factor_128, offset_comp_avg_128, dst, dst_stride, dst8, dst8_stride);
                    im += 2 * 2;
                    dst += 2 * dst_stride;
                    dst8 += 2 * dst8_stride;
                    y -= 2;
                } while (y);
            } else {
                do {
                    const __m128i res = xy_y_convolve_4tap_2x2_sse2(im, s_32, ss_128, coeffs_128);
                    jnt_2d_avg_round_store_2x2_sse2(res, offset_avg_128, dst, dst_stride, dst8, dst8_stride);
                    im += 2 * 2;
                    dst += 2 * dst_stride;
                    dst8 += 2 * dst8_stride;
                    y -= 2;
                } while (y);
            }
        } else {
            do {
                const __m128i res = xy_y_convolve_4tap_2x2_sse2(im, s_32, ss_128, coeffs_128);
                jnt_2d_no_avg_round_store_2x2_sse2(res, offset_no_avg_128, dst, dst_stride);
                im += 2 * 2;
                dst += 2 * dst_stride;
                y -= 2;
            } while (y);
        }
    } else {
        const __m256i offset_avg_256    = _mm256_set1_epi32(offset_avg);
        const __m256i offset_no_avg_256 = _mm256_set1_epi32(offset_no_avg);
        __m256i       coeffs_256[4];

        prepare_coeffs_4tap_avx2(filter_params_y, subpel_y_q4, coeffs_256);

        if (w == 4) {
            __m128i s_64[4];
            __m256i s_256[2], ss_256[2];

            s_64[0] = _mm_loadl_epi64((__m128i*)(im + 0 * 4));
            s_64[1] = _mm_loadl_epi64((__m128i*)(im + 1 * 4));
            s_64[2] = _mm_loadl_epi64((__m128i*)(im + 2 * 4));

            // Load lines a and b. Line a to lower 128, line b to upper 128
            s_256[0] = _mm256_setr_m128i(s_64[0], s_64[1]);
            s_256[1] = _mm256_setr_m128i(s_64[1], s_64[2]);

            ss_256[0] = _mm256_unpacklo_epi16(s_256[0], s_256[1]);

            if (conv_params->do_average) {
                if (conv_params->use_jnt_comp_avg) {
                    const int32_t round_offset    = 1 << (offset_bits - round_1);
                    const int32_t factor          = conv_params->fwd_offset | (conv_params->bck_offset << 16);
                    const int32_t offset_comp_avg = (round_offset + (round_offset >> 1)) * conv_params->bck_offset -
                        (round_offset << DIST_PRECISION_BITS) - (round_offset << (DIST_PRECISION_BITS - 1)) +
                        (1 << (round_bits + DIST_PRECISION_BITS - 1));
                    const __m256i factor_256          = _mm256_set1_epi32(factor);
                    const __m256i offset_comp_avg_256 = _mm256_set1_epi32(offset_comp_avg);
                    do {
                        const __m256i res = xy_y_convolve_4tap_4x2_avx2(im, s_64, ss_256, coeffs_256);
                        jnt_2d_comp_avg_round_store_4x2_avx2(
                            res, factor_256, offset_comp_avg_256, dst, dst_stride, dst8, dst8_stride);
                        im += 2 * 4;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                } else {
                    do {
                        const __m256i res = xy_y_convolve_4tap_4x2_avx2(im, s_64, ss_256, coeffs_256);
                        jnt_2d_avg_round_store_4x2_avx2(res, offset_avg_256, dst, dst_stride, dst8, dst8_stride);
                        im += 2 * 4;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                }
            } else {
                do {
                    const __m256i res = xy_y_convolve_4tap_4x2_avx2(im, s_64, ss_256, coeffs_256);
                    jnt_2d_no_avg_round_store_4x2_avx2(res, offset_no_avg_256, dst, dst_stride);
                    im += 2 * 4;
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            }
        } else if (w == 8) {
            __m256i s_256[4], r[2];

            s_256[0] = _mm256_loadu_si256((__m256i*)(im + 0 * 8));
            s_256[1] = _mm256_loadu_si256((__m256i*)(im + 1 * 8));

            __m256i ss_256[4];

            ss_256[0] = _mm256_unpacklo_epi16(s_256[0], s_256[1]);
            ss_256[2] = _mm256_unpackhi_epi16(s_256[0], s_256[1]);

            if (conv_params->do_average) {
                if (conv_params->use_jnt_comp_avg) {
                    const int32_t round_offset    = 1 << (offset_bits - round_1);
                    const int32_t factor          = conv_params->fwd_offset | (conv_params->bck_offset << 16);
                    const int32_t offset_comp_avg = (round_offset + (round_offset >> 1)) * conv_params->bck_offset -
                        (round_offset << DIST_PRECISION_BITS) - (round_offset << (DIST_PRECISION_BITS - 1)) +
                        (1 << (round_bits + DIST_PRECISION_BITS - 1));
                    const __m256i factor_256          = _mm256_set1_epi32(factor);
                    const __m256i offset_comp_avg_256 = _mm256_set1_epi32(offset_comp_avg);
                    do {
                        xy_y_convolve_4tap_8x2_avx2(im, ss_256, coeffs_256, r);
                        jnt_2d_comp_avg_round_store_8x2_avx2(
                            r, factor_256, offset_comp_avg_256, dst, dst_stride, dst8, dst8_stride);
                        im += 2 * 8;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                } else {
                    do {
                        xy_y_convolve_4tap_8x2_avx2(im, ss_256, coeffs_256, r);
                        jnt_2d_avg_round_store_8x2_avx2(r, offset_avg_256, dst, dst_stride, dst8, dst8_stride);
                        im += 2 * 8;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                }
            } else {
                do {
                    xy_y_convolve_4tap_8x2_avx2(im, ss_256, coeffs_256, r);
                    jnt_2d_no_avg_round_store_8x2_avx2(r, offset_no_avg_256, dst, dst_stride);
                    im += 2 * 8;
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            }
        } else {
            __m256i s_256[5];

            assert(w == 16);

            s_256[0] = _mm256_loadu_si256((__m256i*)(im + 0 * 16));
            s_256[1] = _mm256_loadu_si256((__m256i*)(im + 1 * 16));
            s_256[2] = _mm256_loadu_si256((__m256i*)(im + 2 * 16));

            __m256i ss_256[4], tt_256[4], r[4];

            ss_256[0] = _mm256_unpacklo_epi16(s_256[0], s_256[1]);
            ss_256[2] = _mm256_unpackhi_epi16(s_256[0], s_256[1]);

            tt_256[0] = _mm256_unpacklo_epi16(s_256[1], s_256[2]);
            tt_256[2] = _mm256_unpackhi_epi16(s_256[1], s_256[2]);

            if (conv_params->do_average) {
                if (conv_params->use_jnt_comp_avg) {
                    const int32_t round_offset    = 1 << (offset_bits - round_1);
                    const int32_t factor          = conv_params->fwd_offset | (conv_params->bck_offset << 16);
                    const int32_t offset_comp_avg = (round_offset + (round_offset >> 1)) * conv_params->bck_offset -
                        (round_offset << DIST_PRECISION_BITS) - (round_offset << (DIST_PRECISION_BITS - 1)) +
                        (1 << (round_bits + DIST_PRECISION_BITS - 1));
                    const __m256i factor_256          = _mm256_set1_epi32(factor);
                    const __m256i offset_comp_avg_256 = _mm256_set1_epi32(offset_comp_avg);
                    do {
                        xy_y_convolve_4tap_16x2_avx2(im, s_256, ss_256, tt_256, coeffs_256, r);
                        jnt_2d_comp_avg_round_store_16x2_avx2(
                            r, factor_256, offset_comp_avg_256, dst, dst_stride, dst8, dst8_stride);
                        im += 2 * 16;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                } else {
                    do {
                        xy_y_convolve_4tap_16x2_avx2(im, s_256, ss_256, tt_256, coeffs_256, r);
                        jnt_2d_avg_round_store_16x2_avx2(r, offset_avg_256, dst, dst_stride, dst8, dst8_stride);
                        im += 2 * 16;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                }
            } else {
                do {
                    xy_y_convolve_4tap_16x2_avx2(im, s_256, ss_256, tt_256, coeffs_256, r);
                    jnt_2d_no_avg_round_store_16x2_avx2(r, offset_no_avg_256, dst, dst_stride);
                    im += 2 * 16;
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            }
        }
    }
}

static void jnt_convolve_2d_ver_6tap_avx2(const int16_t* const im_block, const int32_t w, const int32_t h,
                                          const InterpFilterParams* const filter_params_y, const int32_t subpel_y_q4,
                                          const ConvolveParams* const conv_params, uint8_t* dst8,
                                          const int32_t dst8_stride) {
    const int32_t  dst_stride  = conv_params->dst_stride;
    const int32_t  bd          = 8;
    const int32_t  round_0     = 3;
    const int16_t* im          = im_block;
    const int32_t  round_1     = COMPOUND_ROUND1_BITS;
    const int32_t  offset_bits = bd + 2 * FILTER_BITS - round_0; // 19
    const int32_t  round_bits  = 2 * FILTER_BITS - round_0 - round_1; // 4
    const int32_t  offset_avg  = (1 << (round_1 - 1)) + (1 << (round_bits + round_1)) - (1 << offset_bits) -
        (1 << (offset_bits - 1));
    const int32_t offset_no_avg = (1 << (round_1 - 1)) + (1 << offset_bits) + (1 << (offset_bits - 1));
    ConvBufType*  dst           = conv_params->dst;

    if (w == 2) {
        const __m128i offset_avg_128    = _mm_set1_epi32(offset_avg);
        const __m128i offset_no_avg_128 = _mm_set1_epi32(offset_no_avg);
        __m128i       coeffs_128[3], s_32[6], ss_128[3];

        prepare_coeffs_6tap_ssse3(filter_params_y, subpel_y_q4, coeffs_128);

        s_32[0] = _mm_cvtsi32_si128(*(int32_t*)(im + 0 * 2));
        s_32[1] = _mm_cvtsi32_si128(*(int32_t*)(im + 1 * 2));
        s_32[2] = _mm_cvtsi32_si128(*(int32_t*)(im + 2 * 2));
        s_32[3] = _mm_cvtsi32_si128(*(int32_t*)(im + 3 * 2));
        s_32[4] = _mm_cvtsi32_si128(*(int32_t*)(im + 4 * 2));

        const __m128i src01 = _mm_unpacklo_epi32(s_32[0], s_32[1]);
        const __m128i src12 = _mm_unpacklo_epi32(s_32[1], s_32[2]);
        const __m128i src23 = _mm_unpacklo_epi32(s_32[2], s_32[3]);
        const __m128i src34 = _mm_unpacklo_epi32(s_32[3], s_32[4]);

        ss_128[0] = _mm_unpacklo_epi16(src01, src12);
        ss_128[1] = _mm_unpacklo_epi16(src23, src34);

        int32_t y = h;

        if (conv_params->do_average) {
            if (conv_params->use_jnt_comp_avg) {
                const int32_t round_offset    = 1 << (offset_bits - round_1);
                const int32_t factor          = conv_params->fwd_offset | (conv_params->bck_offset << 16);
                const int32_t offset_comp_avg = (round_offset + (round_offset >> 1)) * conv_params->bck_offset -
                    (round_offset << DIST_PRECISION_BITS) - (round_offset << (DIST_PRECISION_BITS - 1)) +
                    (1 << (round_bits + DIST_PRECISION_BITS - 1));
                const __m128i factor_128          = _mm_set1_epi32(factor);
                const __m128i offset_comp_avg_128 = _mm_set1_epi32(offset_comp_avg);
                do {
                    const __m128i res = xy_y_convolve_6tap_2x2_sse2(im, s_32, ss_128, coeffs_128);
                    jnt_2d_comp_avg_round_store_2x2_sse2(
                        res, factor_128, offset_comp_avg_128, dst, dst_stride, dst8, dst8_stride);
                    im += 2 * 2;
                    dst += 2 * dst_stride;
                    dst8 += 2 * dst8_stride;
                    y -= 2;
                } while (y);
            } else {
                do {
                    const __m128i res = xy_y_convolve_6tap_2x2_sse2(im, s_32, ss_128, coeffs_128);
                    jnt_2d_avg_round_store_2x2_sse2(res, offset_avg_128, dst, dst_stride, dst8, dst8_stride);
                    im += 2 * 2;
                    dst += 2 * dst_stride;
                    dst8 += 2 * dst8_stride;
                    y -= 2;
                } while (y);
            }
        } else {
            do {
                const __m128i res = xy_y_convolve_6tap_2x2_sse2(im, s_32, ss_128, coeffs_128);
                jnt_2d_no_avg_round_store_2x2_sse2(res, offset_no_avg_128, dst, dst_stride);
                im += 2 * 2;
                dst += 2 * dst_stride;
                y -= 2;
            } while (y);
        }
    } else {
        const __m256i offset_avg_256    = _mm256_set1_epi32(offset_avg);
        const __m256i offset_no_avg_256 = _mm256_set1_epi32(offset_no_avg);
        __m256i       coeffs_256[3];

        prepare_coeffs_6tap_avx2(filter_params_y, subpel_y_q4, coeffs_256);

        if (w == 4) {
            __m128i s_64[6];
            __m256i s_256[6], ss_256[3];

            s_64[0] = _mm_loadl_epi64((__m128i*)(im + 0 * 4));
            s_64[1] = _mm_loadl_epi64((__m128i*)(im + 1 * 4));
            s_64[2] = _mm_loadl_epi64((__m128i*)(im + 2 * 4));
            s_64[3] = _mm_loadl_epi64((__m128i*)(im + 3 * 4));
            s_64[4] = _mm_loadl_epi64((__m128i*)(im + 4 * 4));

            // Load lines a and b. Line a to lower 128, line b to upper 128
            s_256[0] = _mm256_setr_m128i(s_64[0], s_64[1]);
            s_256[1] = _mm256_setr_m128i(s_64[1], s_64[2]);
            s_256[2] = _mm256_setr_m128i(s_64[2], s_64[3]);
            s_256[3] = _mm256_setr_m128i(s_64[3], s_64[4]);

            ss_256[0] = _mm256_unpacklo_epi16(s_256[0], s_256[1]);
            ss_256[1] = _mm256_unpacklo_epi16(s_256[2], s_256[3]);

            int32_t y = h;

            if (conv_params->do_average) {
                if (conv_params->use_jnt_comp_avg) {
                    const int32_t round_offset    = 1 << (offset_bits - round_1);
                    const int32_t factor          = conv_params->fwd_offset | (conv_params->bck_offset << 16);
                    const int32_t offset_comp_avg = (round_offset + (round_offset >> 1)) * conv_params->bck_offset -
                        (round_offset << DIST_PRECISION_BITS) - (round_offset << (DIST_PRECISION_BITS - 1)) +
                        (1 << (round_bits + DIST_PRECISION_BITS - 1));
                    const __m256i factor_256          = _mm256_set1_epi32(factor);
                    const __m256i offset_comp_avg_256 = _mm256_set1_epi32(offset_comp_avg);
                    do {
                        const __m256i res = xy_y_convolve_6tap_4x2_avx2(im, s_64, ss_256, coeffs_256);
                        jnt_2d_comp_avg_round_store_4x2_avx2(
                            res, factor_256, offset_comp_avg_256, dst, dst_stride, dst8, dst8_stride);
                        im += 2 * 4;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                } else {
                    do {
                        const __m256i res = xy_y_convolve_6tap_4x2_avx2(im, s_64, ss_256, coeffs_256);
                        jnt_2d_avg_round_store_4x2_avx2(res, offset_avg_256, dst, dst_stride, dst8, dst8_stride);
                        im += 2 * 4;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                }
            } else {
                do {
                    const __m256i res = xy_y_convolve_6tap_4x2_avx2(im, s_64, ss_256, coeffs_256);
                    jnt_2d_no_avg_round_store_4x2_avx2(res, offset_no_avg_256, dst, dst_stride);
                    im += 2 * 4;
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            }
        } else if (w == 8) {
            __m256i s_256[6], r[2];

            s_256[0]  = _mm256_loadu_si256((__m256i*)(im + 0 * 8));
            s_256[1]  = _mm256_loadu_si256((__m256i*)(im + 1 * 8));
            s_256[2]  = _mm256_loadu_si256((__m256i*)(im + 2 * 8));
            s_256[3]  = _mm256_loadu_si256((__m256i*)(im + 3 * 8));
            int32_t y = h;

            __m256i ss_256[6];

            ss_256[0] = _mm256_unpacklo_epi16(s_256[0], s_256[1]);
            ss_256[1] = _mm256_unpacklo_epi16(s_256[2], s_256[3]);

            ss_256[3] = _mm256_unpackhi_epi16(s_256[0], s_256[1]);
            ss_256[4] = _mm256_unpackhi_epi16(s_256[2], s_256[3]);

            if (conv_params->do_average) {
                if (conv_params->use_jnt_comp_avg) {
                    const int32_t round_offset    = 1 << (offset_bits - round_1);
                    const int32_t factor          = conv_params->fwd_offset | (conv_params->bck_offset << 16);
                    const int32_t offset_comp_avg = (round_offset + (round_offset >> 1)) * conv_params->bck_offset -
                        (round_offset << DIST_PRECISION_BITS) - (round_offset << (DIST_PRECISION_BITS - 1)) +
                        (1 << (round_bits + DIST_PRECISION_BITS - 1));
                    const __m256i factor_256          = _mm256_set1_epi32(factor);
                    const __m256i offset_comp_avg_256 = _mm256_set1_epi32(offset_comp_avg);
                    do {
                        xy_y_convolve_6tap_8x2_avx2(im, ss_256, coeffs_256, r);
                        jnt_2d_comp_avg_round_store_8x2_avx2(
                            r, factor_256, offset_comp_avg_256, dst, dst_stride, dst8, dst8_stride);
                        im += 2 * 8;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                } else {
                    do {
                        xy_y_convolve_6tap_8x2_avx2(im, ss_256, coeffs_256, r);
                        jnt_2d_avg_round_store_8x2_avx2(r, offset_avg_256, dst, dst_stride, dst8, dst8_stride);
                        im += 2 * 8;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                }
            } else {
                do {
                    xy_y_convolve_6tap_8x2_avx2(im, ss_256, coeffs_256, r);
                    jnt_2d_no_avg_round_store_8x2_avx2(r, offset_no_avg_256, dst, dst_stride);
                    im += 2 * 8;
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            }
        } else if (w == 16) {
            __m256i s_256[6], ss_256[6], tt_256[6], r[4];

            loadu_unpack_16bit_5rows_avx2(im, 16, s_256, ss_256, tt_256);
            int32_t y = h;

            if (conv_params->do_average) {
                if (conv_params->use_jnt_comp_avg) {
                    const int32_t round_offset    = 1 << (offset_bits - round_1);
                    const int32_t factor          = conv_params->fwd_offset | (conv_params->bck_offset << 16);
                    const int32_t offset_comp_avg = (round_offset + (round_offset >> 1)) * conv_params->bck_offset -
                        (round_offset << DIST_PRECISION_BITS) - (round_offset << (DIST_PRECISION_BITS - 1)) +
                        (1 << (round_bits + DIST_PRECISION_BITS - 1));
                    const __m256i factor_256          = _mm256_set1_epi32(factor);
                    const __m256i offset_comp_avg_256 = _mm256_set1_epi32(offset_comp_avg);
                    do {
                        xy_y_convolve_6tap_16x2_avx2(im, 16, s_256, ss_256, tt_256, coeffs_256, r);
                        jnt_2d_comp_avg_round_store_16x2_avx2(
                            r, factor_256, offset_comp_avg_256, dst, dst_stride, dst8, dst8_stride);
                        im += 2 * 16;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                } else {
                    do {
                        xy_y_convolve_6tap_16x2_avx2(im, 16, s_256, ss_256, tt_256, coeffs_256, r);
                        jnt_2d_avg_round_store_16x2_avx2(r, offset_avg_256, dst, dst_stride, dst8, dst8_stride);
                        im += 2 * 16;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                }
            } else {
                do {
                    xy_y_convolve_6tap_16x2_avx2(im, 16, s_256, ss_256, tt_256, coeffs_256, r);
                    jnt_2d_no_avg_round_store_16x2_avx2(r, offset_no_avg_256, dst, dst_stride);
                    im += 2 * 16;
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            }
        } else {
            __m256i s_256[2][6], ss_256[2][6], tt_256[2][6], r0[4], r1[4];

            assert(!(w % 32));

            int32_t x = 0;
            do {
                const int16_t* s  = im + x;
                ConvBufType*   d  = dst + x;
                uint8_t*       d8 = dst8 + x;

                loadu_unpack_16bit_5rows_avx2(s, w, s_256[0], ss_256[0], tt_256[0]);
                loadu_unpack_16bit_5rows_avx2(s + 16, w, s_256[1], ss_256[1], tt_256[1]);

                int32_t y = h;

                if (conv_params->do_average) {
                    if (conv_params->use_jnt_comp_avg) {
                        const int32_t round_offset    = 1 << (offset_bits - round_1);
                        const int32_t factor          = conv_params->fwd_offset | (conv_params->bck_offset << 16);
                        const int32_t offset_comp_avg = (round_offset + (round_offset >> 1)) * conv_params->bck_offset -
                            (round_offset << DIST_PRECISION_BITS) - (round_offset << (DIST_PRECISION_BITS - 1)) +
                            (1 << (round_bits + DIST_PRECISION_BITS - 1));
                        const __m256i factor_256          = _mm256_set1_epi32(factor);
                        const __m256i offset_comp_avg_256 = _mm256_set1_epi32(offset_comp_avg);
                        do {
                            xy_y_convolve_6tap_16x2_avx2(s, w, s_256[0], ss_256[0], tt_256[0], coeffs_256, r0);
                            xy_y_convolve_6tap_16x2_avx2(s + 16, w, s_256[1], ss_256[1], tt_256[1], coeffs_256, r1);
                            jnt_2d_comp_avg_round_store_32_avx2(r0 + 0, r1 + 0, factor_256, offset_comp_avg_256, d, d8);
                            jnt_2d_comp_avg_round_store_32_avx2(
                                r0 + 2, r1 + 2, factor_256, offset_comp_avg_256, d + dst_stride, d8 + dst8_stride);
                            s += 2 * w;
                            d += 2 * dst_stride;
                            d8 += 2 * dst8_stride;
                            y -= 2;
                        } while (y);
                    } else {
                        do {
                            xy_y_convolve_6tap_16x2_avx2(s, w, s_256[0], ss_256[0], tt_256[0], coeffs_256, r0);
                            xy_y_convolve_6tap_16x2_avx2(s + 16, w, s_256[1], ss_256[1], tt_256[1], coeffs_256, r1);
                            jnt_2d_avg_round_store_32_avx2(r0 + 0, r1 + 0, offset_avg_256, d, d8);
                            jnt_2d_avg_round_store_32_avx2(
                                r0 + 2, r1 + 2, offset_avg_256, d + dst_stride, d8 + dst8_stride);
                            s += 2 * w;
                            d += 2 * dst_stride;
                            d8 += 2 * dst8_stride;
                            y -= 2;
                        } while (y);
                    }
                } else {
                    do {
                        xy_y_convolve_6tap_16x2_avx2(s, w, s_256[0], ss_256[0], tt_256[0], coeffs_256, r0);
                        xy_y_convolve_6tap_16x2_avx2(s + 16, w, s_256[1], ss_256[1], tt_256[1], coeffs_256, r1);
                        jnt_2d_no_avg_round_store_32_avx2(r0 + 0, r1 + 0, offset_no_avg_256, d);
                        jnt_2d_no_avg_round_store_32_avx2(r0 + 2, r1 + 2, offset_no_avg_256, d + dst_stride);
                        s += 2 * w;
                        d += 2 * dst_stride;
                        y -= 2;
                    } while (y);
                }

                x += 32;
            } while (x < w);
        }
    }
}

static void jnt_convolve_2d_ver_8tap_avx2(const int16_t* const im_block, const int32_t w, const int32_t h,
                                          const InterpFilterParams* const filter_params_y, const int32_t subpel_y_q4,
                                          const ConvolveParams* const conv_params, uint8_t* dst8,
                                          const int32_t dst8_stride) {
    const int32_t  dst_stride  = conv_params->dst_stride;
    const int32_t  bd          = 8;
    const int32_t  round_0     = 3;
    const int16_t* im          = im_block;
    const int32_t  round_1     = COMPOUND_ROUND1_BITS;
    const int32_t  offset_bits = bd + 2 * FILTER_BITS - round_0; // 19
    const int32_t  round_bits  = 2 * FILTER_BITS - round_0 - round_1; // 4
    const int32_t  offset_avg  = (1 << (round_1 - 1)) + (1 << (round_bits + round_1)) - (1 << offset_bits) -
        (1 << (offset_bits - 1));
    const int32_t offset_no_avg = (1 << (round_1 - 1)) + (1 << offset_bits) + (1 << (offset_bits - 1));
    ConvBufType*  dst           = conv_params->dst;

    if (w == 2) {
        const __m128i offset_avg_128    = _mm_set1_epi32(offset_avg);
        const __m128i offset_no_avg_128 = _mm_set1_epi32(offset_no_avg);
        __m128i       coeffs_128[4], s_32[8], ss_128[4];

        prepare_coeffs_8tap_sse2(filter_params_y, subpel_y_q4, coeffs_128);

        s_32[0] = _mm_cvtsi32_si128(*(int32_t*)(im + 0 * 2));
        s_32[1] = _mm_cvtsi32_si128(*(int32_t*)(im + 1 * 2));
        s_32[2] = _mm_cvtsi32_si128(*(int32_t*)(im + 2 * 2));
        s_32[3] = _mm_cvtsi32_si128(*(int32_t*)(im + 3 * 2));
        s_32[4] = _mm_cvtsi32_si128(*(int32_t*)(im + 4 * 2));
        s_32[5] = _mm_cvtsi32_si128(*(int32_t*)(im + 5 * 2));
        s_32[6] = _mm_cvtsi32_si128(*(int32_t*)(im + 6 * 2));

        const __m128i src01 = _mm_unpacklo_epi32(s_32[0], s_32[1]);
        const __m128i src12 = _mm_unpacklo_epi32(s_32[1], s_32[2]);
        const __m128i src23 = _mm_unpacklo_epi32(s_32[2], s_32[3]);
        const __m128i src34 = _mm_unpacklo_epi32(s_32[3], s_32[4]);
        const __m128i src45 = _mm_unpacklo_epi32(s_32[4], s_32[5]);
        const __m128i src56 = _mm_unpacklo_epi32(s_32[5], s_32[6]);

        ss_128[0] = _mm_unpacklo_epi16(src01, src12);
        ss_128[1] = _mm_unpacklo_epi16(src23, src34);
        ss_128[2] = _mm_unpacklo_epi16(src45, src56);

        int32_t y = h;

        if (conv_params->do_average) {
            if (conv_params->use_jnt_comp_avg) {
                const int32_t round_offset    = 1 << (offset_bits - round_1);
                const int32_t factor          = conv_params->fwd_offset | (conv_params->bck_offset << 16);
                const int32_t offset_comp_avg = (round_offset + (round_offset >> 1)) * conv_params->bck_offset -
                    (round_offset << DIST_PRECISION_BITS) - (round_offset << (DIST_PRECISION_BITS - 1)) +
                    (1 << (round_bits + DIST_PRECISION_BITS - 1));
                const __m128i factor_128          = _mm_set1_epi32(factor);
                const __m128i offset_comp_avg_128 = _mm_set1_epi32(offset_comp_avg);
                do {
                    const __m128i res = xy_y_convolve_8tap_2x2_sse2(im, s_32, ss_128, coeffs_128);
                    jnt_2d_comp_avg_round_store_2x2_sse2(
                        res, factor_128, offset_comp_avg_128, dst, dst_stride, dst8, dst8_stride);
                    im += 2 * 2;
                    dst += 2 * dst_stride;
                    dst8 += 2 * dst8_stride;
                    y -= 2;
                } while (y);
            } else {
                do {
                    const __m128i res = xy_y_convolve_8tap_2x2_sse2(im, s_32, ss_128, coeffs_128);
                    jnt_2d_avg_round_store_2x2_sse2(res, offset_avg_128, dst, dst_stride, dst8, dst8_stride);
                    im += 2 * 2;
                    dst += 2 * dst_stride;
                    dst8 += 2 * dst8_stride;
                    y -= 2;
                } while (y);
            }
        } else {
            do {
                const __m128i res = xy_y_convolve_8tap_2x2_sse2(im, s_32, ss_128, coeffs_128);
                jnt_2d_no_avg_round_store_2x2_sse2(res, offset_no_avg_128, dst, dst_stride);
                im += 2 * 2;
                dst += 2 * dst_stride;
                y -= 2;
            } while (y);
        }
    } else {
        const __m256i offset_avg_256    = _mm256_set1_epi32(offset_avg);
        const __m256i offset_no_avg_256 = _mm256_set1_epi32(offset_no_avg);
        __m256i       coeffs_256[4];

        prepare_coeffs_8tap_avx2(filter_params_y, subpel_y_q4, coeffs_256);

        if (w == 4) {
            __m128i s_64[8];
            __m256i s_256[8], ss_256[4];

            s_64[0] = _mm_loadl_epi64((__m128i*)(im + 0 * 4));
            s_64[1] = _mm_loadl_epi64((__m128i*)(im + 1 * 4));
            s_64[2] = _mm_loadl_epi64((__m128i*)(im + 2 * 4));
            s_64[3] = _mm_loadl_epi64((__m128i*)(im + 3 * 4));
            s_64[4] = _mm_loadl_epi64((__m128i*)(im + 4 * 4));
            s_64[5] = _mm_loadl_epi64((__m128i*)(im + 5 * 4));
            s_64[6] = _mm_loadl_epi64((__m128i*)(im + 6 * 4));

            // Load lines a and b. Line a to lower 128, line b to upper 128
            s_256[0] = _mm256_setr_m128i(s_64[0], s_64[1]);
            s_256[1] = _mm256_setr_m128i(s_64[1], s_64[2]);
            s_256[2] = _mm256_setr_m128i(s_64[2], s_64[3]);
            s_256[3] = _mm256_setr_m128i(s_64[3], s_64[4]);
            s_256[4] = _mm256_setr_m128i(s_64[4], s_64[5]);
            s_256[5] = _mm256_setr_m128i(s_64[5], s_64[6]);

            ss_256[0] = _mm256_unpacklo_epi16(s_256[0], s_256[1]);
            ss_256[1] = _mm256_unpacklo_epi16(s_256[2], s_256[3]);
            ss_256[2] = _mm256_unpacklo_epi16(s_256[4], s_256[5]);

            int32_t y = h;

            if (conv_params->do_average) {
                if (conv_params->use_jnt_comp_avg) {
                    const int32_t round_offset    = 1 << (offset_bits - round_1);
                    const int32_t factor          = conv_params->fwd_offset | (conv_params->bck_offset << 16);
                    const int32_t offset_comp_avg = (round_offset + (round_offset >> 1)) * conv_params->bck_offset -
                        (round_offset << DIST_PRECISION_BITS) - (round_offset << (DIST_PRECISION_BITS - 1)) +
                        (1 << (round_bits + DIST_PRECISION_BITS - 1));
                    const __m256i factor_256          = _mm256_set1_epi32(factor);
                    const __m256i offset_comp_avg_256 = _mm256_set1_epi32(offset_comp_avg);
                    do {
                        const __m256i res = xy_y_convolve_8tap_4x2_avx2(im, s_64, ss_256, coeffs_256);
                        jnt_2d_comp_avg_round_store_4x2_avx2(
                            res, factor_256, offset_comp_avg_256, dst, dst_stride, dst8, dst8_stride);
                        im += 2 * 4;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                } else {
                    do {
                        const __m256i res = xy_y_convolve_8tap_4x2_avx2(im, s_64, ss_256, coeffs_256);
                        jnt_2d_avg_round_store_4x2_avx2(res, offset_avg_256, dst, dst_stride, dst8, dst8_stride);
                        im += 2 * 4;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                }
            } else {
                do {
                    const __m256i res = xy_y_convolve_8tap_4x2_avx2(im, s_64, ss_256, coeffs_256);
                    jnt_2d_no_avg_round_store_4x2_avx2(res, offset_no_avg_256, dst, dst_stride);
                    im += 2 * 4;
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            }
        } else if (w == 8) {
            __m256i s_256[8], r[2];

            s_256[0]  = _mm256_loadu_si256((__m256i*)(im + 0 * 8));
            s_256[1]  = _mm256_loadu_si256((__m256i*)(im + 1 * 8));
            s_256[2]  = _mm256_loadu_si256((__m256i*)(im + 2 * 8));
            s_256[3]  = _mm256_loadu_si256((__m256i*)(im + 3 * 8));
            s_256[4]  = _mm256_loadu_si256((__m256i*)(im + 4 * 8));
            s_256[5]  = _mm256_loadu_si256((__m256i*)(im + 5 * 8));
            int32_t y = h;

            __m256i ss_256[8];

            convolve_8tap_unapck_avx2(s_256, ss_256);

            if (conv_params->do_average) {
                if (conv_params->use_jnt_comp_avg) {
                    const int32_t round_offset    = 1 << (offset_bits - round_1);
                    const int32_t factor          = conv_params->fwd_offset | (conv_params->bck_offset << 16);
                    const int32_t offset_comp_avg = (round_offset + (round_offset >> 1)) * conv_params->bck_offset -
                        (round_offset << DIST_PRECISION_BITS) - (round_offset << (DIST_PRECISION_BITS - 1)) +
                        (1 << (round_bits + DIST_PRECISION_BITS - 1));
                    const __m256i factor_256          = _mm256_set1_epi32(factor);
                    const __m256i offset_comp_avg_256 = _mm256_set1_epi32(offset_comp_avg);
                    do {
                        xy_y_convolve_8tap_8x2_avx2(im, ss_256, coeffs_256, r);
                        jnt_2d_comp_avg_round_store_8x2_avx2(
                            r, factor_256, offset_comp_avg_256, dst, dst_stride, dst8, dst8_stride);
                        im += 2 * 8;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                } else {
                    do {
                        xy_y_convolve_8tap_8x2_avx2(im, ss_256, coeffs_256, r);
                        jnt_2d_avg_round_store_8x2_avx2(r, offset_avg_256, dst, dst_stride, dst8, dst8_stride);
                        im += 2 * 8;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                }
            } else {
                do {
                    xy_y_convolve_8tap_8x2_avx2(im, ss_256, coeffs_256, r);
                    jnt_2d_no_avg_round_store_8x2_avx2(r, offset_no_avg_256, dst, dst_stride);
                    im += 2 * 8;
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            }
        } else if (w == 16) {
            __m256i s_256[8], ss_256[8], tt_256[8], r[4];

            load_16bit_7rows_avx2(im, 16, s_256);
            int32_t y = h;

            convolve_8tap_unapck_avx2(s_256, ss_256);
            convolve_8tap_unapck_avx2(s_256 + 1, tt_256);

            if (conv_params->do_average) {
                if (conv_params->use_jnt_comp_avg) {
                    const int32_t round_offset    = 1 << (offset_bits - round_1);
                    const int32_t factor          = conv_params->fwd_offset | (conv_params->bck_offset << 16);
                    const int32_t offset_comp_avg = (round_offset + (round_offset >> 1)) * conv_params->bck_offset -
                        (round_offset << DIST_PRECISION_BITS) - (round_offset << (DIST_PRECISION_BITS - 1)) +
                        (1 << (round_bits + DIST_PRECISION_BITS - 1));
                    const __m256i factor_256          = _mm256_set1_epi32(factor);
                    const __m256i offset_comp_avg_256 = _mm256_set1_epi32(offset_comp_avg);
                    do {
                        xy_y_convolve_8tap_16x2_avx2(im, 16, coeffs_256, s_256, ss_256, tt_256, r);
                        jnt_2d_comp_avg_round_store_16x2_avx2(
                            r, factor_256, offset_comp_avg_256, dst, dst_stride, dst8, dst8_stride);
                        im += 2 * 16;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                } else {
                    do {
                        xy_y_convolve_8tap_16x2_avx2(im, 16, coeffs_256, s_256, ss_256, tt_256, r);
                        jnt_2d_avg_round_store_16x2_avx2(r, offset_avg_256, dst, dst_stride, dst8, dst8_stride);
                        im += 2 * 16;
                        dst += 2 * dst_stride;
                        dst8 += 2 * dst8_stride;
                        y -= 2;
                    } while (y);
                }
            } else {
                do {
                    xy_y_convolve_8tap_16x2_avx2(im, 16, coeffs_256, s_256, ss_256, tt_256, r);
                    jnt_2d_no_avg_round_store_16x2_avx2(r, offset_no_avg_256, dst, dst_stride);
                    im += 2 * 16;
                    dst += 2 * dst_stride;
                    y -= 2;
                } while (y);
            }
        } else {
            __m256i s_256[2][8], r0[4], r1[4], ss_256[2][8], tt_256[2][8];

            assert(!(w % 32));

            int32_t x = 0;
            do {
                const int16_t* s  = im + x;
                ConvBufType*   d  = dst + x;
                uint8_t*       d8 = dst8 + x;

                load_16bit_7rows_avx2(s, w, s_256[0]);
                convolve_8tap_unapck_avx2(s_256[0], ss_256[0]);
                convolve_8tap_unapck_avx2(s_256[0] + 1, tt_256[0]);

                load_16bit_7rows_avx2(s + 16, w, s_256[1]);
                convolve_8tap_unapck_avx2(s_256[1], ss_256[1]);
                convolve_8tap_unapck_avx2(s_256[1] + 1, tt_256[1]);

                int32_t y = h;

                if (conv_params->do_average) {
                    if (conv_params->use_jnt_comp_avg) {
                        const int32_t round_offset    = 1 << (offset_bits - round_1);
                        const int32_t factor          = conv_params->fwd_offset | (conv_params->bck_offset << 16);
                        const int32_t offset_comp_avg = (round_offset + (round_offset >> 1)) * conv_params->bck_offset -
                            (round_offset << DIST_PRECISION_BITS) - (round_offset << (DIST_PRECISION_BITS - 1)) +
                            (1 << (round_bits + DIST_PRECISION_BITS - 1));
                        const __m256i factor_256          = _mm256_set1_epi32(factor);
                        const __m256i offset_comp_avg_256 = _mm256_set1_epi32(offset_comp_avg);
                        do {
                            xy_y_convolve_8tap_16x2_avx2(s, w, coeffs_256, s_256[0], ss_256[0], tt_256[0], r0);
                            xy_y_convolve_8tap_16x2_avx2(s + 16, w, coeffs_256, s_256[1], ss_256[1], tt_256[1], r1);
                            jnt_2d_comp_avg_round_store_32_avx2(r0 + 0, r1 + 0, factor_256, offset_comp_avg_256, d, d8);
                            jnt_2d_comp_avg_round_store_32_avx2(
                                r0 + 2, r1 + 2, factor_256, offset_comp_avg_256, d + dst_stride, d8 + dst8_stride);
                            s += 2 * w;
                            d += 2 * dst_stride;
                            d8 += 2 * dst8_stride;
                            y -= 2;
                        } while (y);
                    } else {
                        do {
                            xy_y_convolve_8tap_16x2_avx2(s, w, coeffs_256, s_256[0], ss_256[0], tt_256[0], r0);
                            xy_y_convolve_8tap_16x2_avx2(s + 16, w, coeffs_256, s_256[1], ss_256[1], tt_256[1], r1);
                            jnt_2d_avg_round_store_32_avx2(r0 + 0, r1 + 0, offset_avg_256, d, d8);
                            jnt_2d_avg_round_store_32_avx2(
                                r0 + 2, r1 + 2, offset_avg_256, d + dst_stride, d8 + dst8_stride);
                            s += 2 * w;
                            d += 2 * dst_stride;
                            d8 += 2 * dst8_stride;
                            y -= 2;
                        } while (y);
                    }
                } else {
                    do {
                        xy_y_convolve_8tap_16x2_avx2(s, w, coeffs_256, s_256[0], ss_256[0], tt_256[0], r0);
                        xy_y_convolve_8tap_16x2_avx2(s + 16, w, coeffs_256, s_256[1], ss_256[1], tt_256[1], r1);
                        jnt_2d_no_avg_round_store_32_avx2(r0 + 0, r1 + 0, offset_no_avg_256, d);
                        jnt_2d_no_avg_round_store_32_avx2(r0 + 2, r1 + 2, offset_no_avg_256, d + dst_stride);
                        s += 2 * w;
                        d += 2 * dst_stride;
                        y -= 2;
                    } while (y);
                }

                x += 32;
            } while (x < w);
        }
    }
}

typedef void (*JntConvolve2dHorTapFunc)(const uint8_t* src, const int32_t src_stride, const int32_t w, const int32_t h,
                                        const InterpFilterParams* filter_params_x, const int32_t subpel_x_q4,
                                        int16_t* const im_block);

typedef void (*JntConvolve2dVerTapFunc)(const int16_t* const im_block, const int32_t w, const int32_t h,
                                        const InterpFilterParams* const filter_params_y, const int32_t subpel_y_q4,
                                        const ConvolveParams* const conv_params, uint8_t* dst8,
                                        const int32_t dst8_stride);

void svt_av1_jnt_convolve_2d_avx2(const uint8_t* src, int32_t src_stride, uint8_t* dst8, int32_t dst8_stride, int32_t w,
                                  int32_t h, const InterpFilterParams* filter_params_x,
                                  const InterpFilterParams* filter_params_y, const int32_t subpel_x_q4,
                                  const int32_t subpel_y_q4, ConvolveParams* conv_params) {
    static const JntConvolve2dHorTapFunc jnt_convolve_2d_hor_tap_func_table[MAX_FILTER_TAP + 1] = {
        NULL,
        NULL,
        jnt_convolve_2d_hor_2tap_avx2,
        NULL,
        jnt_convolve_2d_hor_4tap_avx2,
        NULL,
        jnt_convolve_2d_hor_6tap_avx2,
        NULL,
        jnt_convolve_2d_hor_8tap_avx2};
    static const JntConvolve2dVerTapFunc jnt_convolve_2d_ver_tap_func_table[MAX_FILTER_TAP + 1] = {
        NULL,
        jnt_convolve_2d_ver_2tap_half_avx2,
        jnt_convolve_2d_ver_2tap_avx2,
        jnt_convolve_2d_ver_4tap_avx2,
        jnt_convolve_2d_ver_4tap_avx2,
        jnt_convolve_2d_ver_6tap_avx2,
        jnt_convolve_2d_ver_6tap_avx2,
        jnt_convolve_2d_ver_8tap_avx2,
        jnt_convolve_2d_ver_8tap_avx2};
    const int32_t  tap_x   = get_convolve_tap(filter_params_x->filter_ptr);
    const int32_t  tap_y   = get_convolve_tap(filter_params_y->filter_ptr);
    const uint8_t* src_ptr = src + ((MAX_FILTER_TAP - tap_y) / 2 - 3) * src_stride;
    // Note: im_block is 8-pixel interlaced for width 32 and up, to avoid data
    //       permutation.
    DECLARE_ALIGNED(64, int16_t, im_block[(MAX_SB_SIZE + MAX_FILTER_TAP) * MAX_SB_SIZE]);

    assert(conv_params->round_0 == 3);
    assert(conv_params->round_1 == COMPOUND_ROUND1_BITS);

    // horizontal filter

    // Have to calculate 1 more row for small widths, since 2 lines are
    // calculated in each loop for them.
    const int32_t hh = h + tap_y - (w >= 32);

    jnt_convolve_2d_hor_tap_func_table[tap_x](src_ptr, src_stride, w, hh, filter_params_x, subpel_x_q4, im_block);

    // vertical filter
    jnt_convolve_2d_ver_tap_func_table[tap_y - (subpel_y_q4 == 8)](
        im_block, w, h, filter_params_y, subpel_y_q4, conv_params, dst8, dst8_stride);
}
