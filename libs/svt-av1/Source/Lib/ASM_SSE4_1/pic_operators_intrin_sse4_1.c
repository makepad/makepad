/*
* Copyright(c) 2022 Intel Corporation
*
* This source code is subject to the terms of the BSD 2 Clause License and
* the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
* was not distributed with this source code in the LICENSE file, you can
* obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
* Media Patent License 1.0 was not distributed with this source code in the
* PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
*/

#include <smmintrin.h>
#include "common_dsp_rtcd.h"
#include "picture_operators_sse2.h"
#include "synonyms.h"
#include "pack_unpack_c.h"

static INLINE void spatial_full_distortion_kernel16_sse4_1_intrin(const uint8_t* const input,
                                                                  const uint8_t* const recon, __m128i* const sum) {
    const __m128i in8 = _mm_loadu_si128((__m128i*)input);
    const __m128i re8 = _mm_loadu_si128((__m128i*)recon);

    __m128i in8_L = _mm_unpacklo_epi8(in8, _mm_setzero_si128());
    __m128i in8_H = _mm_unpackhi_epi8(in8, _mm_setzero_si128());
    __m128i re8_L = _mm_unpacklo_epi8(re8, _mm_setzero_si128());
    __m128i re8_H = _mm_unpackhi_epi8(re8, _mm_setzero_si128());

    in8_L = _mm_sub_epi16(in8_L, re8_L);
    in8_H = _mm_sub_epi16(in8_H, re8_H);

    in8_L = _mm_madd_epi16(in8_L, in8_L);
    in8_H = _mm_madd_epi16(in8_H, in8_H);

    *sum = _mm_add_epi32(*sum, _mm_add_epi32(in8_L, in8_H));
}

uint64_t svt_spatial_full_distortion_kernel_sse4_1(uint8_t* input, uint32_t input_offset, uint32_t input_stride,
                                                   uint8_t* recon, int32_t recon_offset, uint32_t recon_stride,
                                                   uint32_t area_width, uint32_t area_height) {
    const uint32_t leftover = area_width & 15;
    int32_t        h;
    __m128i        sum = _mm_setzero_si128();
    input += input_offset;
    recon += recon_offset;

    if (leftover) {
        const uint8_t* inp = input + area_width - leftover;
        const uint8_t* rec = recon + area_width - leftover;

        if (leftover == 4) {
            h = area_height;
            do {
                const __m128i in0 = _mm_cvtsi32_si128(*(uint32_t*)inp);
                const __m128i re0 = _mm_cvtsi32_si128(*(uint32_t*)rec);

                __m128i in8_L = _mm_unpacklo_epi8(in0, _mm_setzero_si128());
                __m128i re8_L = _mm_unpacklo_epi8(re0, _mm_setzero_si128());

                in8_L = _mm_sub_epi16(in8_L, re8_L);
                in8_L = _mm_madd_epi16(in8_L, in8_L);
                sum   = _mm_add_epi32(sum, in8_L);

                inp += input_stride;
                rec += recon_stride;
                h -= 1;
            } while (h);
        } else if (leftover == 8) {
            h = area_height;
            do {
                const __m128i in0 = _mm_loadl_epi64((__m128i*)inp);
                const __m128i re0 = _mm_loadl_epi64((__m128i*)rec);

                __m128i in8_L = _mm_unpacklo_epi8(in0, _mm_setzero_si128());
                __m128i re8_L = _mm_unpacklo_epi8(re0, _mm_setzero_si128());

                in8_L = _mm_sub_epi16(in8_L, re8_L);
                in8_L = _mm_madd_epi16(in8_L, in8_L);
                sum   = _mm_add_epi32(sum, in8_L);

                inp += input_stride;
                rec += recon_stride;
                h -= 1;
            } while (h);
        } else { //(leftover < 16)
            h = area_height;
            do {
                __m128i in0   = _mm_loadl_epi64((__m128i*)inp);
                __m128i re0   = _mm_loadl_epi64((__m128i*)rec);
                __m128i in8_L = _mm_unpacklo_epi8(in0, _mm_setzero_si128());
                __m128i re8_L = _mm_unpacklo_epi8(re0, _mm_setzero_si128());
                in8_L         = _mm_sub_epi16(in8_L, re8_L);
                in8_L         = _mm_madd_epi16(in8_L, in8_L);
                sum           = _mm_add_epi32(sum, in8_L);

                in0   = _mm_cvtsi32_si128(*(uint32_t*)(inp + 8));
                re0   = _mm_cvtsi32_si128(*(uint32_t*)(rec + 8));
                in8_L = _mm_unpacklo_epi8(in0, _mm_setzero_si128());
                re8_L = _mm_unpacklo_epi8(re0, _mm_setzero_si128());
                in8_L = _mm_sub_epi16(in8_L, re8_L);
                in8_L = _mm_madd_epi16(in8_L, in8_L);
                sum   = _mm_add_epi32(sum, in8_L);

                inp += input_stride;
                rec += recon_stride;
            } while (--h);
        }
    }

    area_width -= leftover;

    if (area_width) {
        const uint8_t* inp = input;
        const uint8_t* rec = recon;
        h                  = area_height;

        if (area_width == 16) {
            do {
                spatial_full_distortion_kernel16_sse4_1_intrin(inp, rec, &sum);
                inp += input_stride;
                rec += recon_stride;
            } while (--h);
        } else if (area_width == 32) {
            do {
                spatial_full_distortion_kernel16_sse4_1_intrin(inp, rec, &sum);
                spatial_full_distortion_kernel16_sse4_1_intrin(inp + 16, rec + 16, &sum);
                inp += input_stride;
                rec += recon_stride;
            } while (--h);
        } else if (area_width == 64) {
            do {
                spatial_full_distortion_kernel16_sse4_1_intrin(inp, rec, &sum);
                spatial_full_distortion_kernel16_sse4_1_intrin(inp + 16, rec + 16, &sum);
                spatial_full_distortion_kernel16_sse4_1_intrin(inp + 32, rec + 32, &sum);
                spatial_full_distortion_kernel16_sse4_1_intrin(inp + 48, rec + 48, &sum);
                inp += input_stride;
                rec += recon_stride;
            } while (--h);
        } else {
            __m128i sum64 = _mm_setzero_si128();
            do {
                for (uint32_t w = 0; w < area_width; w += 16) {
                    spatial_full_distortion_kernel16_sse4_1_intrin(inp + w, rec + w, &sum);
                }
                inp += input_stride;
                rec += recon_stride;

                sum64 = _mm_add_epi64(sum64, _mm_unpacklo_epi32(sum, _mm_setzero_si128()));
                sum64 = _mm_add_epi64(sum64, _mm_unpackhi_epi32(sum, _mm_setzero_si128()));
                sum   = _mm_setzero_si128();
            } while (--h);
            return _mm_extract_epi64(sum64, 0) + _mm_extract_epi64(sum64, 1);
        }
    }

    return hadd32_sse2_intrin(sum);
}

static INLINE void full_distortion_kernel4_sse4_1_intrin(const uint16_t* const input, const uint16_t* const recon,
                                                         __m128i* const sum) {
    __m128i in   = _mm_loadl_epi64((__m128i*)input);
    __m128i re   = _mm_loadl_epi64((__m128i*)recon);
    __m128i max  = _mm_max_epu16(in, re);
    __m128i min  = _mm_min_epu16(in, re);
    __m128i diff = _mm_sub_epi16(max, min);
    diff         = _mm_madd_epi16(diff, diff);
    *sum         = _mm_add_epi32(*sum, diff);
}

static INLINE void full_distortion_kernel8_sse4_1_intrin(__m128i in, __m128i re, __m128i* const sum) {
    __m128i max  = _mm_max_epu16(in, re);
    __m128i min  = _mm_min_epu16(in, re);
    __m128i diff = _mm_sub_epi16(max, min);

    diff = _mm_madd_epi16(diff, diff);
    *sum = _mm_add_epi32(*sum, diff);
}

uint64_t svt_full_distortion_kernel16_bits_sse4_1(uint8_t* input, uint32_t input_offset, uint32_t input_stride,
                                                  uint8_t* recon, int32_t recon_offset, uint32_t recon_stride,
                                                  uint32_t area_width, uint32_t area_height) {
    const uint32_t leftover    = area_width & 7;
    __m128i        sum32       = _mm_setzero_si128();
    __m128i        sum64       = _mm_setzero_si128();
    uint16_t*      input_16bit = (uint16_t*)input;
    uint16_t*      recon_16bit = (uint16_t*)recon;
    input_16bit += input_offset;
    recon_16bit += recon_offset;

    if (leftover) { //(leftover == 4)
        const uint16_t* inp = input_16bit + area_width - leftover;
        const uint16_t* rec = recon_16bit + area_width - leftover;
        uint32_t        h   = area_height;

        do {
            full_distortion_kernel4_sse4_1_intrin(inp, rec, &sum32);
            inp += input_stride;
            rec += recon_stride;

            sum64 = _mm_add_epi64(sum64, _mm_unpacklo_epi32(sum32, _mm_setzero_si128()));
            sum64 = _mm_add_epi64(sum64, _mm_unpackhi_epi32(sum32, _mm_setzero_si128()));
            sum32 = _mm_setzero_si128();
        } while (--h);
    }

    area_width -= leftover;

    if (area_width) {
        const uint16_t* inp = input_16bit;
        const uint16_t* rec = recon_16bit;

        if (area_width == 8) {
            for (uint32_t h = 0; h < area_height; h += 1) {
                full_distortion_kernel8_sse4_1_intrin(
                    _mm_loadu_si128((__m128i*)inp), _mm_loadu_si128((__m128i*)rec), &sum32);
                inp += input_stride;
                rec += recon_stride;

                sum64 = _mm_add_epi64(sum64, _mm_unpacklo_epi32(sum32, _mm_setzero_si128()));
                sum64 = _mm_add_epi64(sum64, _mm_unpackhi_epi32(sum32, _mm_setzero_si128()));
                sum32 = _mm_setzero_si128();
            }

        } else if (area_width == 16) {
            for (uint32_t h = 0; h < area_height; h += 1) {
                full_distortion_kernel8_sse4_1_intrin(
                    _mm_loadu_si128((__m128i*)inp), _mm_loadu_si128((__m128i*)rec), &sum32);
                full_distortion_kernel8_sse4_1_intrin(
                    _mm_loadu_si128((__m128i*)(inp + 8)), _mm_loadu_si128((__m128i*)(rec + 8)), &sum32);
                inp += input_stride;
                rec += recon_stride;

                sum64 = _mm_add_epi64(sum64, _mm_unpacklo_epi32(sum32, _mm_setzero_si128()));
                sum64 = _mm_add_epi64(sum64, _mm_unpackhi_epi32(sum32, _mm_setzero_si128()));
                sum32 = _mm_setzero_si128();
            }
        } else {
            for (uint32_t h = 0; h < area_height; h++) {
                for (uint32_t w = 0; w < area_width; w += 8) {
                    full_distortion_kernel8_sse4_1_intrin(
                        _mm_loadu_si128((__m128i*)(inp + w)), _mm_loadu_si128((__m128i*)(rec + w)), &sum32);

                    sum64 = _mm_add_epi64(sum64, _mm_unpacklo_epi32(sum32, _mm_setzero_si128()));
                    sum64 = _mm_add_epi64(sum64, _mm_unpackhi_epi32(sum32, _mm_setzero_si128()));
                    sum32 = _mm_setzero_si128();
                }
                inp += input_stride;
                rec += recon_stride;
            }
        }
    }

    return _mm_extract_epi64(sum64, 0) + _mm_extract_epi64(sum64, 1);
}

SIMD_INLINE void residual_kernel4_sse4_1(const uint8_t* input, const uint32_t input_stride, const uint8_t* pred,
                                         const uint32_t pred_stride, int16_t* residual, const uint32_t residual_stride,
                                         const uint32_t area_height) {
    __m128i in, pr;

    const __m128i zero = _mm_setzero_si128();
    uint32_t      y    = area_height;

    do {
        in = _mm_cvtsi32_si128(*(int32_t*)(input + 0 * input_stride));
        in = _mm_insert_epi32(in, *(int32_t*)(input + 1 * input_stride), 1);
        pr = _mm_cvtsi32_si128(*(int32_t*)(pred + 0 * pred_stride));
        pr = _mm_insert_epi32(pr, *(int32_t*)(pred + 1 * pred_stride), 1);

        const __m128i in_lo = _mm_unpacklo_epi8(in, zero);
        const __m128i pr_lo = _mm_unpacklo_epi8(pr, zero);
        const __m128i re    = _mm_sub_epi16(in_lo, pr_lo);

        _mm_storel_epi64((__m128i*)residual, re);
        _mm_storeh_epi64((__m128i*)(residual + residual_stride), re);

        input += 2 * input_stride;
        pred += 2 * pred_stride;
        residual += 2 * residual_stride;
        y -= 2;
    } while (y);
}

SIMD_INLINE void residual_kernel8_sse4_1(const uint8_t* input, const uint32_t input_stride, const uint8_t* pred,
                                         const uint32_t pred_stride, int16_t* residual, const uint32_t residual_stride,
                                         const uint32_t area_height) {
    const __m128i zero = _mm_setzero_si128();
    uint32_t      y    = area_height;

    do {
        const __m128i in    = _mm_loadl_epi64((__m128i*)input);
        const __m128i pr    = _mm_loadl_epi64((__m128i*)pred);
        const __m128i in_lo = _mm_unpacklo_epi8(in, zero);
        const __m128i pr_lo = _mm_unpacklo_epi8(pr, zero);
        const __m128i re    = _mm_sub_epi16(in_lo, pr_lo);

        _mm_storeu_si128((__m128i*)residual, re);

        input += input_stride;
        pred += pred_stride;
        residual += residual_stride;
        y -= 1;
    } while (y);
}

SIMD_INLINE void residual_kernel_sse4_1(const uint8_t* input, const uint8_t* pred, int16_t* residual) {
    const __m128i zero  = _mm_setzero_si128();
    const __m128i in    = _mm_loadu_si128((__m128i*)input);
    const __m128i pr    = _mm_loadu_si128((__m128i*)pred);
    const __m128i in_lo = _mm_unpacklo_epi8(in, zero);
    const __m128i in_hi = _mm_unpackhi_epi8(in, zero);
    const __m128i pr_lo = _mm_unpacklo_epi8(pr, zero);
    const __m128i pr_hi = _mm_unpackhi_epi8(pr, zero);
    const __m128i re_lo = _mm_sub_epi16(in_lo, pr_lo);
    const __m128i re_hi = _mm_sub_epi16(in_hi, pr_hi);

    _mm_storeu_si128((__m128i*)residual, re_lo);
    _mm_storeu_si128((__m128i*)(residual + 8), re_hi);
}

SIMD_INLINE void residual_kernel16_sse4_1(const uint8_t* input, const uint32_t input_stride, const uint8_t* pred,
                                          const uint32_t pred_stride, int16_t* residual, const uint32_t residual_stride,
                                          const uint32_t area_height) {
    uint32_t y = area_height;

    do {
        residual_kernel_sse4_1(input, pred, residual);

        input += input_stride;
        pred += pred_stride;
        residual += residual_stride;
        y -= 1;
    } while (y);
}

SIMD_INLINE void residual_kernel32_sse4_1(const uint8_t* input, const uint32_t input_stride, const uint8_t* pred,
                                          const uint32_t pred_stride, int16_t* residual, const uint32_t residual_stride,
                                          const uint32_t area_height) {
    uint32_t y = area_height;

    do {
        residual_kernel_sse4_1(input, pred, residual);
        residual_kernel_sse4_1(input + 16, pred + 16, residual + 16);

        input += input_stride;
        pred += pred_stride;
        residual += residual_stride;
        y -= 1;
    } while (y);
}

SIMD_INLINE void residual_kernel64_sse4_1(const uint8_t* input, const uint32_t input_stride, const uint8_t* pred,
                                          const uint32_t pred_stride, int16_t* residual, const uint32_t residual_stride,
                                          const uint32_t area_height) {
    uint32_t y = area_height;

    do {
        residual_kernel_sse4_1(input, pred, residual);
        residual_kernel_sse4_1(input + 16, pred + 16, residual + 16);
        residual_kernel_sse4_1(input + 32, pred + 32, residual + 32);
        residual_kernel_sse4_1(input + 48, pred + 48, residual + 48);

        input += input_stride;
        pred += pred_stride;
        residual += residual_stride;
        y -= 1;
    } while (y);
}

SIMD_INLINE void residual_kernel128_sse4_1(const uint8_t* input, const uint32_t input_stride, const uint8_t* pred,
                                           const uint32_t pred_stride, int16_t* residual,
                                           const uint32_t residual_stride, const uint32_t area_height) {
    uint32_t y = area_height;

    do {
        residual_kernel_sse4_1(input, pred, residual);
        residual_kernel_sse4_1(input + 16, pred + 16, residual + 16);
        residual_kernel_sse4_1(input + 32, pred + 32, residual + 32);
        residual_kernel_sse4_1(input + 48, pred + 48, residual + 48);
        residual_kernel_sse4_1(input + 64, pred + 64, residual + 64);
        residual_kernel_sse4_1(input + 80, pred + 80, residual + 80);
        residual_kernel_sse4_1(input + 96, pred + 96, residual + 96);
        residual_kernel_sse4_1(input + 112, pred + 112, residual + 112);

        input += input_stride;
        pred += pred_stride;
        residual += residual_stride;
        y -= 1;
    } while (y);
}

void svt_residual_kernel8bit_sse4_1(uint8_t* input, uint32_t input_stride, uint8_t* pred, uint32_t pred_stride,
                                    int16_t* residual, uint32_t residual_stride, uint32_t area_width,
                                    uint32_t area_height) {
    switch (area_width) {
    case 4:
        residual_kernel4_sse4_1(input, input_stride, pred, pred_stride, residual, residual_stride, area_height);
        break;

    case 8:
        residual_kernel8_sse4_1(input, input_stride, pred, pred_stride, residual, residual_stride, area_height);
        break;

    case 16:
        residual_kernel16_sse4_1(input, input_stride, pred, pred_stride, residual, residual_stride, area_height);
        break;

    case 32:
        residual_kernel32_sse4_1(input, input_stride, pred, pred_stride, residual, residual_stride, area_height);
        break;

    case 64:
        residual_kernel64_sse4_1(input, input_stride, pred, pred_stride, residual, residual_stride, area_height);
        break;

    default: // 128
        residual_kernel128_sse4_1(input, input_stride, pred, pred_stride, residual, residual_stride, area_height);
        break;
    }
}

void svt_full_distortion_kernel32_bits_sse4_1(int32_t* coeff, uint32_t coeff_stride, int32_t* recon_coeff,
                                              uint32_t recon_coeff_stride, uint64_t distortion_result[DIST_CALC_TOTAL],
                                              uint32_t area_width, uint32_t area_height) {
    uint32_t      row_count;
    const __m128i zero = _mm_setzero_si128();
    __m128i       sum1 = _mm_setzero_si128();
    __m128i       sum2 = _mm_setzero_si128();
    __m128i       temp1, temp2, temp3;

    row_count = area_height;
    do {
        int32_t* coeff_temp       = coeff;
        int32_t* recon_coeff_temp = recon_coeff;

        uint32_t col_count = area_width / 4;
        do {
            __m128i x0, y0;
            __m128i x_lo, x_hi, y_lo, y_hi, z_lo, z_hi;
            x0 = _mm_loadu_si128((__m128i*)(coeff_temp));
            y0 = _mm_loadu_si128((__m128i*)(recon_coeff_temp));

            x_lo = _mm_unpacklo_epi32(x0, zero);
            x_hi = _mm_unpackhi_epi32(x0, zero);
            y_lo = _mm_unpacklo_epi32(y0, zero);
            y_hi = _mm_unpackhi_epi32(y0, zero);

            z_lo = _mm_mul_epi32(x_lo, x_lo);
            z_hi = _mm_mul_epi32(x_hi, x_hi);
            sum2 = _mm_add_epi64(sum2, z_lo);
            sum2 = _mm_add_epi64(sum2, z_hi);

            x_lo = _mm_sub_epi64(x_lo, y_lo);
            x_hi = _mm_sub_epi64(x_hi, y_hi);
            x_lo = _mm_mul_epi32(x_lo, x_lo);
            x_hi = _mm_mul_epi32(x_hi, x_hi);

            sum1 = _mm_add_epi64(sum1, x_lo);
            sum1 = _mm_add_epi64(sum1, x_hi);
            coeff_temp += 4;
            recon_coeff_temp += 4;
        } while (--col_count);

        coeff += coeff_stride;
        recon_coeff += recon_coeff_stride;
        row_count -= 1;
    } while (row_count > 0);

    temp2 = _mm_shuffle_epi32(sum1, 0x4e);
    temp3 = _mm_add_epi64(sum1, temp2);
    temp2 = _mm_shuffle_epi32(sum2, 0x4e);
    temp1 = _mm_add_epi64(sum2, temp2);
    temp1 = _mm_unpacklo_epi64(temp3, temp1);

    _mm_storeu_si128((__m128i*)distortion_result, temp1);
}

void svt_full_distortion_kernel_cbf_zero32_bits_sse4_1(int32_t* coeff, uint32_t coeff_stride,
                                                       uint64_t distortion_result[DIST_CALC_TOTAL], uint32_t area_width,
                                                       uint32_t area_height) {
    uint32_t      row_count;
    const __m128i zero = _mm_setzero_si128();
    __m128i       sum  = _mm_setzero_si128();
    __m128i       temp1, temp2;

    row_count = area_height;
    do {
        int32_t* coeff_temp = coeff;

        uint32_t col_count = area_width / 4;
        do {
            __m128i x0;
            __m128i x_lo, x_hi, y_lo, y_hi;
            x0 = _mm_loadu_si128((__m128i*)(coeff_temp));
            coeff_temp += 4;

            x_lo = _mm_unpacklo_epi32(x0, zero);
            x_hi = _mm_unpackhi_epi32(x0, zero);
            y_lo = _mm_mul_epi32(x_lo, x_lo);
            y_hi = _mm_mul_epi32(x_hi, x_hi);
            sum  = _mm_add_epi64(sum, y_lo);
            sum  = _mm_add_epi64(sum, y_hi);
        } while (--col_count);

        coeff += coeff_stride;
        row_count -= 1;
    } while (row_count > 0);

    temp2 = _mm_shuffle_epi32(sum, 0x4e);
    temp1 = _mm_add_epi64(sum, temp2);
    _mm_storeu_si128((__m128i*)distortion_result, temp1);
}

static INLINE void unpack_and_2bcompress_32_sse(uint16_t* in16b_buffer, uint8_t* out8b_buffer, uint8_t* out2b_buffer,
                                                uint32_t width_rep) {
    __m128i ymm_00ff = _mm_set1_epi16(0x00FF);
    __m128i msk_2b   = _mm_set1_epi16(0x0003); //0000.0000.0000.0011
    __m128i in1, in2, out8_u8;
    __m128i tmp_2b1, tmp_2b2, tmp_2b;
    __m128i ext0, ext1, ext2, ext3, ext0123;
    __m128i msk0, msk1, msk2;

    msk0 = _mm_set1_epi32(0x000000C0); //1100.0000
    msk1 = _mm_set1_epi32(0x00000030); //0011.0000
    msk2 = _mm_set1_epi32(0x0000000C); //0000.1100
    for (uint32_t w = 0; w < width_rep; w++) {
        in1 = _mm_loadu_si128((__m128i*)(in16b_buffer + w * 16));
        in2 = _mm_loadu_si128((__m128i*)(in16b_buffer + w * 16 + 8));

        tmp_2b1 = _mm_and_si128(in1, msk_2b); //0000.0011.1111.1111 -> 0000.0000.0000.0011
        tmp_2b2 = _mm_and_si128(in2, msk_2b);
        tmp_2b  = _mm_packus_epi16(tmp_2b1, tmp_2b2);

        ext0 = _mm_srli_epi32(
            tmp_2b,
            3 * 8); //0000.0011.0000.0000.0000.0000.0000.0000 -> 0000.0000.0000.0000.0000.0000.0000.0011
        ext1    = _mm_and_si128(_mm_srli_epi32(tmp_2b, 1 * 8 + 6),
                             msk2); //0000.0000.0000.0011.0000.0000.0000.0000 -> 0000.0000.0000.0000.0000.0000.0000.1100
        ext2    = _mm_and_si128(_mm_srli_epi32(tmp_2b, 4),
                             msk1); //0000.0000.0000.0000.0000.0011.0000.0000 -> 0000.0000.0000.0000.0000.0000.0011.0000
        ext3    = _mm_and_si128(_mm_slli_epi32(tmp_2b, 6),
                             msk0); //0000.0000.0000.0000.0000.0000.0000.0011 -> 0000.0000.0000.0000.0000.0000.1100.0000
        ext0123 = _mm_or_si128(_mm_or_si128(ext0, ext1),
                               _mm_or_si128(ext2, ext3)); //0000.0000.0000.0000.0000.0000.1111.1111

        ext0123 = _mm_packus_epi32(ext0123, ext0123);
        ext0123 = _mm_packus_epi16(ext0123, ext0123);

        (void)(*(int*)(out2b_buffer + w * 4) = _mm_cvtsi128_si32((ext0123)));

        out8_u8 = _mm_packus_epi16(_mm_and_si128(_mm_srli_epi16(in1, 2), ymm_00ff),
                                   _mm_and_si128(_mm_srli_epi16(in2, 2), ymm_00ff));
        /*If we assume that in16b_buffer is 10bit max, then we can do:
        out8_u8 = _mm256_packus_epi16(_mm256_srli_epi16(in1, 2),
                                              _mm256_srli_epi16(in2, 2));
        */

        _mm_storeu_si128((__m128i*)(out8b_buffer + w * 16), out8_u8);
    }
}

static INLINE void svt_unpack_and_2bcompress_remainder(uint16_t* in16b_buffer, uint8_t* out8b_buffer,
                                                       uint8_t* out2b_buffer, uint32_t width) {
    uint32_t col;
    uint16_t in_pixel;
    uint8_t  tmp_pixel;

    uint32_t w_m4  = (width / 4) * 4;
    uint32_t w_rem = width - w_m4;

    for (col = 0; col < w_m4; col += 4) {
        uint8_t compressed_unpacked_pixel = 0;
        //+0
        in_pixel              = in16b_buffer[col + 0];
        out8b_buffer[col + 0] = (uint8_t)(in_pixel >> 2);
        tmp_pixel             = (uint8_t)(in_pixel << 6);
        compressed_unpacked_pixel |= ((tmp_pixel >> 0) & 0xC0); //1100.0000

        //+1
        in_pixel              = in16b_buffer[col + 1];
        out8b_buffer[col + 1] = (uint8_t)(in_pixel >> 2);
        tmp_pixel             = (uint8_t)(in_pixel << 6);
        compressed_unpacked_pixel |= ((tmp_pixel >> 2) & 0x30); //0011.0000

        //+2
        in_pixel              = in16b_buffer[col + 2];
        out8b_buffer[col + 2] = (uint8_t)(in_pixel >> 2);
        tmp_pixel             = (uint8_t)(in_pixel << 6);
        compressed_unpacked_pixel |= ((tmp_pixel >> 4) & 0x0C); //0000.1100

        //+3
        in_pixel              = in16b_buffer[col + 3];
        out8b_buffer[col + 3] = (uint8_t)(in_pixel >> 2);
        tmp_pixel             = (uint8_t)(in_pixel << 6);
        compressed_unpacked_pixel |= ((tmp_pixel >> 6) & 0x03); //0000.0011

        out2b_buffer[col / 4] = compressed_unpacked_pixel;
    }

    //we can have up to 3 pixels remaining
    if (w_rem > 0) {
        uint8_t compressed_unpacked_pixel = 0;
        //+0
        in_pixel              = in16b_buffer[col + 0];
        out8b_buffer[col + 0] = (uint8_t)(in_pixel >> 2);
        tmp_pixel             = (uint8_t)(in_pixel << 6);
        compressed_unpacked_pixel |= ((tmp_pixel >> 0) & 0xC0); //1100.0000

        if (w_rem > 1) {
            //+1
            in_pixel              = in16b_buffer[col + 1];
            out8b_buffer[col + 1] = (uint8_t)(in_pixel >> 2);
            tmp_pixel             = (uint8_t)(in_pixel << 6);
            compressed_unpacked_pixel |= ((tmp_pixel >> 2) & 0x30); //0011.0000
        }
        if (w_rem > 2) {
            //+2
            in_pixel              = in16b_buffer[col + 2];
            out8b_buffer[col + 2] = (uint8_t)(in_pixel >> 2);
            tmp_pixel             = (uint8_t)(in_pixel << 6);
            compressed_unpacked_pixel |= ((tmp_pixel >> 4) & 0x0C); //0000.1100
        }

        out2b_buffer[col / 4] = compressed_unpacked_pixel;
    }
}

void svt_unpack_and_2bcompress_sse4_1(uint16_t* in16b_buffer, uint32_t in16b_stride, uint8_t* out8b_buffer,
                                      uint32_t out8b_stride, uint8_t* out2b_buffer, uint32_t out2b_stride,
                                      uint32_t width, uint32_t height) {
    if (width == 32) {
        for (uint32_t h = 0; h < height; h++) {
            unpack_and_2bcompress_32_sse(
                in16b_buffer + h * in16b_stride, out8b_buffer + h * out8b_stride, out2b_buffer + h * out2b_stride, 2);
        }
    } else if (width == 64) {
        for (uint32_t h = 0; h < height; h++) {
            unpack_and_2bcompress_32_sse(
                in16b_buffer + h * in16b_stride, out8b_buffer + h * out8b_stride, out2b_buffer + h * out2b_stride, 4);
        }
    } else {
        uint32_t offset_rem   = width & 0xfffffff0;
        uint32_t offset2b_rem = offset_rem >> 2;
        uint32_t remainder    = width & 0xf;
        for (uint32_t h = 0; h < height; h++) {
            unpack_and_2bcompress_32_sse(in16b_buffer + h * in16b_stride,
                                         out8b_buffer + h * out8b_stride,
                                         out2b_buffer + h * out2b_stride,
                                         width >> 4);
            if (remainder) {
                svt_unpack_and_2bcompress_remainder(in16b_buffer + h * in16b_stride + offset_rem,
                                                    out8b_buffer + h * out8b_stride + offset_rem,
                                                    out2b_buffer + h * out2b_stride + offset2b_rem,
                                                    remainder);
            }
        }
    }
}

static INLINE void compressed_packmsb_32x2h(uint8_t* in8_bit_buffer, uint32_t in8_stride, uint8_t* inn_bit_buffer,
                                            uint32_t inn_stride, uint16_t* out16_bit_buffer, uint32_t out_stride,
                                            uint32_t height) {
    uint32_t y;
    __m128i  in_8_bit0, in_8_bit1, in_8_bit2, in_8_bit3, concat0, concat1, concat2, concat3;

    __m128i in_2_bit, ext0, ext1, ext2, ext3, ext01, ext23, ext01h, ext23h, ext0_15, ext16_31, ext32_47, ext48_63;
    __m128i msk0;

    msk0 = _mm_set1_epi8((int8_t)0xC0); //1100.000

    //processing 2 lines for chroma
    for (y = 0; y < height; y += 2) {
        //2 Lines Stored in 1D format-Could be replaced by 2 _mm_loadl_epi64
        in_2_bit = _mm_unpacklo_epi64(_mm_loadl_epi64((__m128i*)inn_bit_buffer),
                                      _mm_loadl_epi64((__m128i*)(inn_bit_buffer + inn_stride)));

        ext0 = _mm_and_si128(in_2_bit, msk0);
        ext1 = _mm_and_si128(_mm_slli_epi16(in_2_bit, 2), msk0);
        ext2 = _mm_and_si128(_mm_slli_epi16(in_2_bit, 4), msk0);
        ext3 = _mm_and_si128(_mm_slli_epi16(in_2_bit, 6), msk0);

        ext01    = _mm_unpacklo_epi8(ext0, ext1);
        ext23    = _mm_unpacklo_epi8(ext2, ext3);
        ext0_15  = _mm_unpacklo_epi16(ext01, ext23);
        ext16_31 = _mm_unpackhi_epi16(ext01, ext23);

        ext01h   = _mm_unpackhi_epi8(ext0, ext1);
        ext23h   = _mm_unpackhi_epi8(ext2, ext3);
        ext32_47 = _mm_unpacklo_epi16(ext01h, ext23h);
        ext48_63 = _mm_unpackhi_epi16(ext01h, ext23h);

        in_8_bit0 = _mm_loadu_si128((__m128i*)in8_bit_buffer);
        in_8_bit1 = _mm_loadu_si128((__m128i*)(in8_bit_buffer + 16));
        in_8_bit2 = _mm_loadu_si128((__m128i*)(in8_bit_buffer + in8_stride));
        in_8_bit3 = _mm_loadu_si128((__m128i*)(in8_bit_buffer + in8_stride + 16));

        //(out_pixel | n_bit_pixel) concatenation is done with unpacklo_epi8 and unpackhi_epi8
        concat0 = _mm_srli_epi16(_mm_unpacklo_epi8(ext0_15, in_8_bit0), 6);
        concat1 = _mm_srli_epi16(_mm_unpackhi_epi8(ext0_15, in_8_bit0), 6);
        concat2 = _mm_srli_epi16(_mm_unpacklo_epi8(ext16_31, in_8_bit1), 6);
        concat3 = _mm_srli_epi16(_mm_unpackhi_epi8(ext16_31, in_8_bit1), 6);

        _mm_storeu_si128((__m128i*)out16_bit_buffer, concat0);
        _mm_storeu_si128((__m128i*)(out16_bit_buffer + 8), concat1);
        _mm_storeu_si128((__m128i*)(out16_bit_buffer + 16), concat2);
        _mm_storeu_si128((__m128i*)(out16_bit_buffer + 24), concat3);

        //(out_pixel | n_bit_pixel) concatenation is done with unpacklo_epi8 and unpackhi_epi8
        concat0 = _mm_srli_epi16(_mm_unpacklo_epi8(ext32_47, in_8_bit2), 6);
        concat1 = _mm_srli_epi16(_mm_unpackhi_epi8(ext32_47, in_8_bit2), 6);
        concat2 = _mm_srli_epi16(_mm_unpacklo_epi8(ext48_63, in_8_bit3), 6);
        concat3 = _mm_srli_epi16(_mm_unpackhi_epi8(ext48_63, in_8_bit3), 6);

        _mm_storeu_si128((__m128i*)(out16_bit_buffer + out_stride), concat0);
        _mm_storeu_si128((__m128i*)(out16_bit_buffer + out_stride + 8), concat1);
        _mm_storeu_si128((__m128i*)(out16_bit_buffer + out_stride + 16), concat2);
        _mm_storeu_si128((__m128i*)(out16_bit_buffer + out_stride + 24), concat3);

        in8_bit_buffer += in8_stride << 1;
        inn_bit_buffer += inn_stride << 1;
        out16_bit_buffer += out_stride << 1;
    }
}

static INLINE void compressed_packmsb_64xh(uint8_t* in8_bit_buffer, uint32_t in8_stride, uint8_t* inn_bit_buffer,
                                           uint32_t inn_stride, uint16_t* out16_bit_buffer, uint32_t out_stride,
                                           uint32_t height) {
    uint32_t y;
    __m128i  in_8_bit0, in_8_bit1, in_8_bit2, in_8_bit3;
    __m128i  concat0, concat1, concat2, concat3;

    __m128i in_2_bit, ext0, ext1, ext2, ext3, ext01, ext23, ext01h, ext23h, ext0_15, ext16_31, ext32_47, ext48_63;
    __m128i msk;

    msk = _mm_set1_epi8((int8_t)0xC0); //1100.000

    //One row per iter
    for (y = 0; y < height; y++) {
        in_2_bit = _mm_loadu_si128((__m128i*)inn_bit_buffer);

        ext0 = _mm_and_si128(in_2_bit, msk);
        ext1 = _mm_and_si128(_mm_slli_epi16(in_2_bit, 2), msk);
        ext2 = _mm_and_si128(_mm_slli_epi16(in_2_bit, 4), msk);
        ext3 = _mm_and_si128(_mm_slli_epi16(in_2_bit, 6), msk);

        ext01    = _mm_unpacklo_epi8(ext0, ext1);
        ext23    = _mm_unpacklo_epi8(ext2, ext3);
        ext0_15  = _mm_unpacklo_epi16(ext01, ext23);
        ext16_31 = _mm_unpackhi_epi16(ext01, ext23);

        ext01h   = _mm_unpackhi_epi8(ext0, ext1);
        ext23h   = _mm_unpackhi_epi8(ext2, ext3);
        ext32_47 = _mm_unpacklo_epi16(ext01h, ext23h);
        ext48_63 = _mm_unpackhi_epi16(ext01h, ext23h);

        in_8_bit0 = _mm_loadu_si128((__m128i*)in8_bit_buffer);
        in_8_bit1 = _mm_loadu_si128((__m128i*)(in8_bit_buffer + 16));
        in_8_bit2 = _mm_loadu_si128((__m128i*)(in8_bit_buffer + 32));
        in_8_bit3 = _mm_loadu_si128((__m128i*)(in8_bit_buffer + 48));

        //(out_pixel | n_bit_pixel) concatenation
        concat0 = _mm_srli_epi16(_mm_unpacklo_epi8(ext0_15, in_8_bit0), 6);
        concat1 = _mm_srli_epi16(_mm_unpackhi_epi8(ext0_15, in_8_bit0), 6);
        concat2 = _mm_srli_epi16(_mm_unpacklo_epi8(ext16_31, in_8_bit1), 6);
        concat3 = _mm_srli_epi16(_mm_unpackhi_epi8(ext16_31, in_8_bit1), 6);

        _mm_storeu_si128((__m128i*)out16_bit_buffer, concat0);
        _mm_storeu_si128((__m128i*)(out16_bit_buffer + 8), concat1);
        _mm_storeu_si128((__m128i*)(out16_bit_buffer + 16), concat2);
        _mm_storeu_si128((__m128i*)(out16_bit_buffer + 24), concat3);

        //(out_pixel | n_bit_pixel) concatenation
        concat0 = _mm_srli_epi16(_mm_unpacklo_epi8(ext32_47, in_8_bit2), 6);
        concat1 = _mm_srli_epi16(_mm_unpackhi_epi8(ext32_47, in_8_bit2), 6);
        concat2 = _mm_srli_epi16(_mm_unpacklo_epi8(ext48_63, in_8_bit3), 6);
        concat3 = _mm_srli_epi16(_mm_unpackhi_epi8(ext48_63, in_8_bit3), 6);

        _mm_storeu_si128((__m128i*)(out16_bit_buffer + 32), concat0);
        _mm_storeu_si128((__m128i*)(out16_bit_buffer + 40), concat1);
        _mm_storeu_si128((__m128i*)(out16_bit_buffer + 48), concat2);
        _mm_storeu_si128((__m128i*)(out16_bit_buffer + 56), concat3);

        in8_bit_buffer += in8_stride;
        inn_bit_buffer += inn_stride;
        out16_bit_buffer += out_stride;
    }
}

static INLINE void compressed_packmsb_64(uint8_t* in8_bit_buffer, uint8_t* inn_bit_buffer, uint16_t* out16_bit_buffer,
                                         uint32_t width_rep) {
    __m128i in_8_bit0, in_8_bit1, in_8_bit2, in_8_bit3;
    __m128i concat0, concat1, concat2, concat3;

    __m128i in_2_bit, ext0, ext1, ext2, ext3, ext01, ext23, ext01h, ext23h, ext0_15, ext16_31, ext32_47, ext48_63;
    __m128i msk;

    msk = _mm_set1_epi8((int8_t)0xC0); //1100.000

    //One row per iter
    for (uint32_t w = 0; w < width_rep; w++) {
        in_2_bit = _mm_loadu_si128((__m128i*)inn_bit_buffer);

        ext0 = _mm_and_si128(in_2_bit, msk);
        ext1 = _mm_and_si128(_mm_slli_epi16(in_2_bit, 2), msk);
        ext2 = _mm_and_si128(_mm_slli_epi16(in_2_bit, 4), msk);
        ext3 = _mm_and_si128(_mm_slli_epi16(in_2_bit, 6), msk);

        ext01    = _mm_unpacklo_epi8(ext0, ext1);
        ext23    = _mm_unpacklo_epi8(ext2, ext3);
        ext0_15  = _mm_unpacklo_epi16(ext01, ext23);
        ext16_31 = _mm_unpackhi_epi16(ext01, ext23);

        ext01h   = _mm_unpackhi_epi8(ext0, ext1);
        ext23h   = _mm_unpackhi_epi8(ext2, ext3);
        ext32_47 = _mm_unpacklo_epi16(ext01h, ext23h);
        ext48_63 = _mm_unpackhi_epi16(ext01h, ext23h);

        in_8_bit0 = _mm_loadu_si128((__m128i*)in8_bit_buffer);
        in_8_bit1 = _mm_loadu_si128((__m128i*)(in8_bit_buffer + 16));
        in_8_bit2 = _mm_loadu_si128((__m128i*)(in8_bit_buffer + 32));
        in_8_bit3 = _mm_loadu_si128((__m128i*)(in8_bit_buffer + 48));

        //(out_pixel | n_bit_pixel) concatenation
        concat0 = _mm_srli_epi16(_mm_unpacklo_epi8(ext0_15, in_8_bit0), 6);
        concat1 = _mm_srli_epi16(_mm_unpackhi_epi8(ext0_15, in_8_bit0), 6);
        concat2 = _mm_srli_epi16(_mm_unpacklo_epi8(ext16_31, in_8_bit1), 6);
        concat3 = _mm_srli_epi16(_mm_unpackhi_epi8(ext16_31, in_8_bit1), 6);

        _mm_storeu_si128((__m128i*)out16_bit_buffer, concat0);
        _mm_storeu_si128((__m128i*)(out16_bit_buffer + 8), concat1);
        _mm_storeu_si128((__m128i*)(out16_bit_buffer + 16), concat2);
        _mm_storeu_si128((__m128i*)(out16_bit_buffer + 24), concat3);

        //(out_pixel | n_bit_pixel) concatenation
        concat0 = _mm_srli_epi16(_mm_unpacklo_epi8(ext32_47, in_8_bit2), 6);
        concat1 = _mm_srli_epi16(_mm_unpackhi_epi8(ext32_47, in_8_bit2), 6);
        concat2 = _mm_srli_epi16(_mm_unpacklo_epi8(ext48_63, in_8_bit3), 6);
        concat3 = _mm_srli_epi16(_mm_unpackhi_epi8(ext48_63, in_8_bit3), 6);

        _mm_storeu_si128((__m128i*)(out16_bit_buffer + 32), concat0);
        _mm_storeu_si128((__m128i*)(out16_bit_buffer + 40), concat1);
        _mm_storeu_si128((__m128i*)(out16_bit_buffer + 48), concat2);
        _mm_storeu_si128((__m128i*)(out16_bit_buffer + 56), concat3);

        in8_bit_buffer += 64;
        inn_bit_buffer += 16;
        out16_bit_buffer += 64;
    }
}

void svt_compressed_packmsb_sse4_1_intrin(uint8_t* in8_bit_buffer, uint32_t in8_stride, uint8_t* inn_bit_buffer,
                                          uint32_t inn_stride, uint16_t* out16_bit_buffer, uint32_t out_stride,
                                          uint32_t width, uint32_t height) {
    if (width == 32) {
        compressed_packmsb_32x2h(
            in8_bit_buffer, in8_stride, inn_bit_buffer, inn_stride, out16_bit_buffer, out_stride, height);
    } else if (width == 64) {
        compressed_packmsb_64xh(
            in8_bit_buffer, in8_stride, inn_bit_buffer, inn_stride, out16_bit_buffer, out_stride, height);
    } else {
        int32_t  leftover     = width;
        uint32_t offset8b_16b = 0;
        uint32_t offset2b     = 0;
        if (leftover >= 64) {
            uint32_t offset = width & 0xffffff40;
            for (uint32_t y = 0; y < height; y++) {
                compressed_packmsb_64(in8_bit_buffer + y * in8_stride,
                                      inn_bit_buffer + y * inn_stride,
                                      out16_bit_buffer + y * out_stride,
                                      width >> 6);
            }
            offset8b_16b += offset;
            offset2b += offset >> 2;
            leftover -= offset;
        }
        if (leftover >= 32) {
            compressed_packmsb_32x2h(in8_bit_buffer + offset8b_16b,
                                     in8_stride,
                                     inn_bit_buffer + offset2b,
                                     inn_stride,
                                     out16_bit_buffer + offset8b_16b,
                                     out_stride,
                                     height);
            offset8b_16b += 32;
            offset2b += 8;
            leftover -= 32;
        }
        if (leftover) {
            svt_compressed_packmsb_c(in8_bit_buffer + offset8b_16b,
                                     in8_stride,
                                     inn_bit_buffer + offset2b,
                                     inn_stride,
                                     out16_bit_buffer + offset8b_16b,
                                     out_stride,
                                     leftover,
                                     height);
        }
    }
}
