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

#include <emmintrin.h> // SSE2
#include <smmintrin.h> /* SSE4.1 */

#include "definitions.h"
#include "synonyms.h"

#include "cabac_context_model.h"
#include "full_loop.h"
#include "motion_estimation.h" //svt_aom_downsample_2d_c()

void svt_av1_txb_init_levels_sse4_1(const TranLow* const coeff, const int32_t width, const int32_t height,
                                    uint8_t* const levels) {
    const int     stride = width + TX_PAD_HOR;
    const __m128i zeros  = _mm_setzero_si128();

    int            i  = 0;
    uint8_t*       ls = levels;
    const TranLow* cf = coeff;
    if (width == 4) {
        xx_storeu_128(ls - 16, zeros);
        do {
            const __m128i coeffA  = xx_loadu_128(cf);
            const __m128i coeffB  = xx_loadu_128(cf + 4);
            const __m128i coeffAB = _mm_packs_epi32(coeffA, coeffB);
            const __m128i absAB   = _mm_abs_epi16(coeffAB);
            const __m128i absAB8  = _mm_packs_epi16(absAB, zeros);
            const __m128i lsAB    = _mm_unpacklo_epi32(absAB8, zeros);
            xx_storeu_128(ls, lsAB);
            ls += (stride << 1);
            cf += (width << 1);
            i += 2;
        } while (i < height);
        xx_storeu_128(ls, zeros);
        xx_storeu_128(ls + 16, zeros);
    } else if (width == 8) {
        xx_storeu_128(ls - 24, zeros);
        xx_storeu_128(ls - 8, zeros);
        do {
            const __m128i coeffA  = xx_loadu_128(cf);
            const __m128i coeffB  = xx_loadu_128(cf + 4);
            const __m128i coeffAB = _mm_packs_epi32(coeffA, coeffB);
            const __m128i absAB   = _mm_abs_epi16(coeffAB);
            const __m128i absAB8  = _mm_packs_epi16(absAB, zeros);
            xx_storeu_128(ls, absAB8);
            ls += stride;
            cf += width;
            i += 1;
        } while (i < height);
        xx_storeu_128(ls, zeros);
        xx_storeu_128(ls + 16, zeros);
        xx_storeu_128(ls + 32, zeros);
    } else if (width == 16) {
        xx_storeu_128(ls - 40, zeros);
        xx_storeu_128(ls - 24, zeros);
        xx_storeu_128(ls - 8, zeros);
        do {
            int j = 0;
            do {
                const __m128i coeffA  = xx_loadu_128(cf);
                const __m128i coeffB  = xx_loadu_128(cf + 4);
                const __m128i coeffC  = xx_loadu_128(cf + 8);
                const __m128i coeffD  = xx_loadu_128(cf + 12);
                const __m128i coeffAB = _mm_packs_epi32(coeffA, coeffB);
                const __m128i coeffCD = _mm_packs_epi32(coeffC, coeffD);
                const __m128i absAB   = _mm_abs_epi16(coeffAB);
                const __m128i absCD   = _mm_abs_epi16(coeffCD);
                const __m128i absABCD = _mm_packs_epi16(absAB, absCD);
                xx_storeu_128(ls + j, absABCD);
                j += 16;
                cf += 16;
            } while (j < width);
            *(int32_t*)(ls + width) = 0;
            ls += stride;
            i += 1;
        } while (i < height);
        xx_storeu_128(ls, zeros);
        xx_storeu_128(ls + 16, zeros);
        xx_storeu_128(ls + 32, zeros);
        xx_storeu_128(ls + 48, zeros);
        xx_storeu_128(ls + 64, zeros);
    } else {
        xx_storeu_128(ls - 72, zeros);
        xx_storeu_128(ls - 56, zeros);
        xx_storeu_128(ls - 40, zeros);
        xx_storeu_128(ls - 24, zeros);
        xx_storeu_128(ls - 8, zeros);
        do {
            int j = 0;
            do {
                const __m128i coeffA  = xx_loadu_128(cf);
                const __m128i coeffB  = xx_loadu_128(cf + 4);
                const __m128i coeffC  = xx_loadu_128(cf + 8);
                const __m128i coeffD  = xx_loadu_128(cf + 12);
                const __m128i coeffAB = _mm_packs_epi32(coeffA, coeffB);
                const __m128i coeffCD = _mm_packs_epi32(coeffC, coeffD);
                const __m128i absAB   = _mm_abs_epi16(coeffAB);
                const __m128i absCD   = _mm_abs_epi16(coeffCD);
                const __m128i absABCD = _mm_packs_epi16(absAB, absCD);
                xx_storeu_128(ls + j, absABCD);
                j += 16;
                cf += 16;
            } while (j < width);
            *(int32_t*)(ls + width) = 0;
            ls += stride;
            i += 1;
        } while (i < height);
        xx_storeu_128(ls, zeros);
        xx_storeu_128(ls + 16, zeros);
        xx_storeu_128(ls + 32, zeros);
        xx_storeu_128(ls + 48, zeros);
        xx_storeu_128(ls + 64, zeros);
        xx_storeu_128(ls + 80, zeros);
        xx_storeu_128(ls + 96, zeros);
        xx_storeu_128(ls + 112, zeros);
        xx_storeu_128(ls + 128, zeros);
    }
}

static INLINE __m128i compute_sum(__m128i* in, __m128i* prev_in) {
    const __m128i zero    = _mm_setzero_si128();
    const __m128i round_2 = _mm_set1_epi16(2);
    __m128i       prev_in_lo, in_lo, sum_;

    prev_in_lo = _mm_unpacklo_epi8(*prev_in, zero);
    *prev_in   = _mm_unpackhi_epi8(*prev_in, zero);
    in_lo      = _mm_unpacklo_epi8(*in, zero);
    *in        = _mm_unpackhi_epi8(*in, zero);

    sum_ = _mm_add_epi16(_mm_hadd_epi16(prev_in_lo, *prev_in), _mm_hadd_epi16(in_lo, *in));
    sum_ = _mm_add_epi16(sum_, round_2);
    sum_ = _mm_srli_epi16(sum_, 2);

    return _mm_packus_epi16(sum_, zero);
}

void svt_aom_downsample_2d_sse4_1(uint8_t* input_samples, // input parameter, input samples Ptr
                                  uint32_t input_stride, // input parameter, input stride
                                  uint32_t input_area_width, // input parameter, input area width
                                  uint32_t input_area_height, // input parameter, input area height
                                  uint8_t* decim_samples, // output parameter, decimated samples Ptr
                                  uint32_t decim_stride, // input parameter, output stride
                                  uint32_t decim_step) // input parameter, decimation amount in pixels
{
    uint32_t input_stripe_stride = input_stride * decim_step;
    uint32_t decim_horizontal_index;
    uint8_t* in_ptr        = input_samples;
    uint8_t* out_ptr       = decim_samples;
    uint32_t width_align16 = input_area_width - (input_area_width % 16);

    __m128i in, prev_in;
    __m128i sum_epu8;

    if (decim_step == 2) {
        in_ptr += input_stride;
        for (uint32_t vert_idx = 1; vert_idx < input_area_height; vert_idx += 2) {
            uint8_t* prev_in_line  = in_ptr - input_stride;
            decim_horizontal_index = 0;
            for (uint32_t horiz_idx = 1; horiz_idx < width_align16; horiz_idx += 16) {
                prev_in  = _mm_loadu_si128((__m128i*)(prev_in_line + horiz_idx - 1));
                in       = _mm_loadu_si128((__m128i*)(in_ptr + horiz_idx - 1));
                sum_epu8 = compute_sum(&in, &prev_in);
                _mm_storel_epi64((__m128i*)(out_ptr + decim_horizontal_index), sum_epu8);
                decim_horizontal_index += 8;
            }
            //complement when input_area_width is not multiple of 16
            if (width_align16 < input_area_width) {
                DECLARE_ALIGNED(16, uint8_t, tmp_buf[8]);
                prev_in   = _mm_loadu_si128((__m128i*)(prev_in_line + width_align16));
                in        = _mm_loadu_si128((__m128i*)(in_ptr + width_align16));
                sum_epu8  = compute_sum(&in, &prev_in);
                int count = (input_area_width - width_align16) >> 1;
                _mm_storel_epi64((__m128i*)(tmp_buf), sum_epu8);
                memcpy(out_ptr + decim_horizontal_index, tmp_buf, count * sizeof(uint8_t));
            }
            in_ptr += input_stripe_stride;
            out_ptr += decim_stride;
        }
    } else if (decim_step == 4) {
        const __m128i mask = _mm_set_epi64x(0x0F0D0B0907050301, 0x0E0C0A0806040200);
        in_ptr += 2 * input_stride;
        for (uint32_t vertical_index = 2; vertical_index < input_area_height; vertical_index += 4) {
            uint8_t* prev_in_line  = in_ptr - input_stride;
            decim_horizontal_index = 0;
            for (uint32_t horiz_idx = 2; horiz_idx < width_align16; horiz_idx += 16) {
                prev_in  = _mm_loadu_si128((__m128i*)(prev_in_line + horiz_idx - 1));
                in       = _mm_loadu_si128((__m128i*)(in_ptr + horiz_idx - 1));
                sum_epu8 = compute_sum(&in, &prev_in);
                sum_epu8 = _mm_shuffle_epi8(sum_epu8, mask);
                *(uint32_t*)(out_ptr + decim_horizontal_index) = _mm_cvtsi128_si32(sum_epu8);
                //_mm_storeu_si32((__m128i *)(out_ptr + decim_horizontal_index), sum_epu8);
                decim_horizontal_index += 4;
            }
            //complement when input_area_width is not multiple of 16
            if (width_align16 < input_area_width) {
                prev_in        = _mm_loadu_si128((__m128i*)(prev_in_line + width_align16 + 1));
                in             = _mm_loadu_si128((__m128i*)(in_ptr + width_align16 + 1));
                sum_epu8       = compute_sum(&in, &prev_in);
                sum_epu8       = _mm_shuffle_epi8(sum_epu8, mask);
                int      count = (input_area_width - width_align16) >> 2;
                uint32_t tmp   = _mm_cvtsi128_si32(sum_epu8);
                //_mm_storel_epi64((__m128i *)(tmp_buf), sum_epu8);
                memcpy(out_ptr + decim_horizontal_index, &tmp, count * sizeof(uint8_t));
            }
            in_ptr += input_stripe_stride;
            out_ptr += decim_stride;
        }
    } else {
        svt_aom_downsample_2d_c(
            input_samples, input_stride, input_area_width, input_area_height, decim_samples, decim_stride, decim_step);
    }
}
