/*
* Copyright(c) 2019 Intel Corporation
*
* This source code is subject to the terms of the BSD 2 Clause License and
* the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
* was not distributed with this source code in the LICENSE file, you can
* obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
* Media Patent License 1.0 was not distributed with this source code in the
* PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
*/
#include "definitions.h"

#if EN_AVX512_SUPPORT

#include <immintrin.h>
#include "pic_operators_inline_avx2.h"
#include "picture_operators_sse2.h"
#include "memory_avx2.h"

/*******************************************************************************
 * Helper function that add 32bit values from sum32 to 64bit values in sum64
 * sum32 is also zeroed
*******************************************************************************/
static INLINE void sum32_to64_avx512(__m512i* const sum32, __m512i* const sum64) {
    //Save partial sum into large 64bit register instead of 32 bit (which could overflow)
    *sum64 = _mm512_add_epi64(*sum64, _mm512_unpacklo_epi32(*sum32, _mm512_setzero_si512()));
    *sum64 = _mm512_add_epi64(*sum64, _mm512_unpackhi_epi32(*sum32, _mm512_setzero_si512()));
    *sum32 = _mm512_setzero_si512();
}

static INLINE void residual32x2_avx512(const uint8_t* input, const uint32_t input_stride, const uint8_t* pred,
                                       const uint32_t pred_stride, int16_t* residual, const uint32_t residual_stride) {
    const __m512i zero  = _mm512_setzero_si512();
    const __m512i idx   = _mm512_setr_epi64(0, 4, 1, 5, 2, 6, 3, 7);
    const __m256i in0   = _mm256_loadu_si256((__m256i*)input);
    const __m256i in1   = _mm256_loadu_si256((__m256i*)(input + input_stride));
    const __m256i pr0   = _mm256_loadu_si256((__m256i*)pred);
    const __m256i pr1   = _mm256_loadu_si256((__m256i*)(pred + pred_stride));
    const __m512i in2   = _mm512_inserti64x4(_mm512_castsi256_si512(in0), in1, 1);
    const __m512i pr2   = _mm512_inserti64x4(_mm512_castsi256_si512(pr0), pr1, 1);
    const __m512i in3   = _mm512_permutexvar_epi64(idx, in2);
    const __m512i pr3   = _mm512_permutexvar_epi64(idx, pr2);
    const __m512i in_lo = _mm512_unpacklo_epi8(in3, zero);
    const __m512i in_hi = _mm512_unpackhi_epi8(in3, zero);
    const __m512i pr_lo = _mm512_unpacklo_epi8(pr3, zero);
    const __m512i pr_hi = _mm512_unpackhi_epi8(pr3, zero);
    const __m512i re_lo = _mm512_sub_epi16(in_lo, pr_lo);
    const __m512i re_hi = _mm512_sub_epi16(in_hi, pr_hi);
    _mm512_storeu_si512((__m512i*)(residual + 0 * residual_stride), re_lo);
    _mm512_storeu_si512((__m512i*)(residual + 1 * residual_stride), re_hi);
}

SIMD_INLINE void residual_kernel32_avx2(const uint8_t* input, const uint32_t input_stride, const uint8_t* pred,
                                        const uint32_t pred_stride, int16_t* residual, const uint32_t residual_stride,
                                        const uint32_t area_height) {
    uint32_t y = area_height;

    do {
        residual32x2_avx512(input, input_stride, pred, pred_stride, residual, residual_stride);
        input += 2 * input_stride;
        pred += 2 * pred_stride;
        residual += 2 * residual_stride;
        y -= 2;
    } while (y);
}

static INLINE void residual64_avx512(const uint8_t* const input, const uint8_t* const pred, int16_t* const residual) {
    const __m512i zero  = _mm512_setzero_si512();
    const __m512i idx   = _mm512_setr_epi64(0, 4, 1, 5, 2, 6, 3, 7);
    const __m512i in0   = _mm512_loadu_si512((__m512i*)input);
    const __m512i pr0   = _mm512_loadu_si512((__m512i*)pred);
    const __m512i in1   = _mm512_permutexvar_epi64(idx, in0);
    const __m512i pr1   = _mm512_permutexvar_epi64(idx, pr0);
    const __m512i in_lo = _mm512_unpacklo_epi8(in1, zero);
    const __m512i in_hi = _mm512_unpackhi_epi8(in1, zero);
    const __m512i pr_lo = _mm512_unpacklo_epi8(pr1, zero);
    const __m512i pr_hi = _mm512_unpackhi_epi8(pr1, zero);
    const __m512i re_lo = _mm512_sub_epi16(in_lo, pr_lo);
    const __m512i re_hi = _mm512_sub_epi16(in_hi, pr_hi);
    _mm512_storeu_si512((__m512i*)(residual + 0 * 32), re_lo);
    _mm512_storeu_si512((__m512i*)(residual + 1 * 32), re_hi);
}

SIMD_INLINE void residual_kernel64_avx2(const uint8_t* input, const uint32_t input_stride, const uint8_t* pred,
                                        const uint32_t pred_stride, int16_t* residual, const uint32_t residual_stride,
                                        const uint32_t area_height) {
    uint32_t y = area_height;

    do {
        residual64_avx512(input, pred, residual);
        input += input_stride;
        pred += pred_stride;
        residual += residual_stride;
    } while (--y);
}

SIMD_INLINE void residual_kernel128_avx2(const uint8_t* input, const uint32_t input_stride, const uint8_t* pred,
                                         const uint32_t pred_stride, int16_t* residual, const uint32_t residual_stride,
                                         const uint32_t area_height) {
    uint32_t y = area_height;

    do {
        residual64_avx512(input + 0 * 64, pred + 0 * 64, residual + 0 * 64);
        residual64_avx512(input + 1 * 64, pred + 1 * 64, residual + 1 * 64);
        input += input_stride;
        pred += pred_stride;
        residual += residual_stride;
    } while (--y);
}

void svt_residual_kernel8bit_avx512(uint8_t* input, uint32_t input_stride, uint8_t* pred, uint32_t pred_stride,
                                    int16_t* residual, uint32_t residual_stride, uint32_t area_width,
                                    uint32_t area_height) {
    switch (area_width) {
    case 4:
        residual_kernel4_avx2(input, input_stride, pred, pred_stride, residual, residual_stride, area_height);
        break;

    case 8:
        residual_kernel8_avx2(input, input_stride, pred, pred_stride, residual, residual_stride, area_height);
        break;

    case 16:
        residual_kernel16_avx2(input, input_stride, pred, pred_stride, residual, residual_stride, area_height);
        break;

    case 32:
        residual_kernel32_avx2(input, input_stride, pred, pred_stride, residual, residual_stride, area_height);
        break;

    case 64:
        residual_kernel64_avx2(input, input_stride, pred, pred_stride, residual, residual_stride, area_height);
        break;

    default: // 128
        residual_kernel128_avx2(input, input_stride, pred, pred_stride, residual, residual_stride, area_height);
        break;
    }
}

static INLINE void Distortion_AVX512_INTRIN(const __m256i input, const __m256i recon, __m512i* const sum) {
    const __m512i in   = _mm512_cvtepu8_epi16(input);
    const __m512i re   = _mm512_cvtepu8_epi16(recon);
    const __m512i diff = _mm512_sub_epi16(in, re);
    const __m512i dist = _mm512_madd_epi16(diff, diff);
    *sum               = _mm512_add_epi32(*sum, dist);
}

static INLINE void SpatialFullDistortionKernel32_AVX512_INTRIN(const uint8_t* const input, const uint8_t* const recon,
                                                               __m512i* const sum) {
    const __m256i in = _mm256_loadu_si256((__m256i*)input);
    const __m256i re = _mm256_loadu_si256((__m256i*)recon);
    Distortion_AVX512_INTRIN(in, re, sum);
}

static INLINE void SpatialFullDistortionKernel64_AVX512_INTRIN(const uint8_t* const input, const uint8_t* const recon,
                                                               __m512i* const sum) {
    const __m512i in     = _mm512_loadu_si512((__m512i*)input);
    const __m512i re     = _mm512_loadu_si512((__m512i*)recon);
    const __m512i max    = _mm512_max_epu8(in, re);
    const __m512i min    = _mm512_min_epu8(in, re);
    const __m512i diff   = _mm512_sub_epi8(max, min);
    const __m512i diff_l = _mm512_unpacklo_epi8(diff, _mm512_setzero_si512());
    const __m512i diff_h = _mm512_unpackhi_epi8(diff, _mm512_setzero_si512());
    const __m512i dist_l = _mm512_madd_epi16(diff_l, diff_l);
    const __m512i dist_h = _mm512_madd_epi16(diff_h, diff_h);
    const __m512i dist   = _mm512_add_epi32(dist_l, dist_h);
    *sum                 = _mm512_add_epi32(*sum, dist);
}

uint64_t svt_spatial_full_distortion_kernel_avx512(uint8_t* input, uint32_t input_offset, uint32_t input_stride,
                                                   uint8_t* recon, int32_t recon_offset, uint32_t recon_stride,
                                                   uint32_t area_width, uint32_t area_height) {
    const uint32_t leftover = area_width & 31;
    int32_t        h;
    __m256i        sum = _mm256_setzero_si256();
    __m128i        sum_l, sum_h, s;
    input += input_offset;
    recon += recon_offset;

    if (leftover) {
        const uint8_t* inp = input + area_width - leftover;
        const uint8_t* rec = recon + area_width - leftover;

        if (leftover == 4) {
            h = area_height;
            do {
                const __m128i in0 = _mm_cvtsi32_si128(*(uint32_t*)inp);
                const __m128i in1 = _mm_cvtsi32_si128(*(uint32_t*)(inp + input_stride));
                const __m128i re0 = _mm_cvtsi32_si128(*(uint32_t*)rec);
                const __m128i re1 = _mm_cvtsi32_si128(*(uint32_t*)(rec + recon_stride));
                const __m256i in  = _mm256_setr_m128i(in0, in1);
                const __m256i re  = _mm256_setr_m128i(re0, re1);
                distortion_avx2_intrin(in, re, &sum);
                inp += 2 * input_stride;
                rec += 2 * recon_stride;
                h -= 2;
            } while (h);

            if (area_width == 4) {
                sum_l                       = _mm256_castsi256_si128(sum);
                sum_h                       = _mm256_extracti128_si256(sum, 1);
                s                           = _mm_add_epi32(sum_l, sum_h);
                s                           = _mm_add_epi32(s, _mm_srli_si128(s, 4));
                uint64_t spatial_distortion = _mm_cvtsi128_si32(s);
                return spatial_distortion;
            }
        } else if (leftover == 8) {
            h = area_height;
            do {
                const __m128i in0 = _mm_loadl_epi64((__m128i*)inp);
                const __m128i in1 = _mm_loadl_epi64((__m128i*)(inp + input_stride));
                const __m128i re0 = _mm_loadl_epi64((__m128i*)rec);
                const __m128i re1 = _mm_loadl_epi64((__m128i*)(rec + recon_stride));
                const __m256i in  = _mm256_setr_m128i(in0, in1);
                const __m256i re  = _mm256_setr_m128i(re0, re1);
                distortion_avx2_intrin(in, re, &sum);
                inp += 2 * input_stride;
                rec += 2 * recon_stride;
                h -= 2;
            } while (h);
        } else if (leftover <= 16) {
            h = area_height;
            do {
                spatial_full_distortion_kernel16_avx2_intrin(inp, rec, &sum);
                inp += input_stride;
                rec += recon_stride;
            } while (--h);

            if (leftover == 12) {
                const __m256i mask = _mm256_setr_epi32(-1, -1, -1, -1, -1, -1, 0, 0);
                sum                = _mm256_and_si256(sum, mask);
            }
        } else {
            __m256i sum1 = _mm256_setzero_si256();
            h            = area_height;
            do {
                spatial_full_distortion_kernel32_leftover_avx2_intrin(inp, rec, &sum, &sum1);
                inp += input_stride;
                rec += recon_stride;
            } while (--h);

            __m256i mask[2];
            if (leftover == 20) {
                mask[0] = _mm256_setr_epi32(-1, -1, -1, -1, -1, -1, 0, 0);
                mask[1] = _mm256_setr_epi32(-1, -1, -1, -1, 0, 0, 0, 0);
            } else if (leftover == 24) {
                mask[0] = _mm256_setr_epi32(-1, -1, -1, -1, -1, -1, -1, -1);
                mask[1] = _mm256_setr_epi32(-1, -1, -1, -1, 0, 0, 0, 0);
            } else { // leftover = 28
                mask[0] = _mm256_setr_epi32(-1, -1, -1, -1, -1, -1, -1, -1);
                mask[1] = _mm256_setr_epi32(-1, -1, -1, -1, -1, -1, 0, 0);
            }

            sum  = _mm256_and_si256(sum, mask[0]);
            sum1 = _mm256_and_si256(sum1, mask[1]);
            sum  = _mm256_add_epi32(sum, sum1);
        }
    }

    area_width -= leftover;

    if (area_width) {
        uint8_t* inp = input;
        uint8_t* rec = recon;
        h            = area_height;

        if (area_width == 32) {
            do {
                spatial_full_distortion_kernel32_avx2_intrin(inp, rec, &sum);
                inp += input_stride;
                rec += recon_stride;
            } while (--h);
        } else {
            __m512i sum512 = _mm512_setzero_si512();

            if (area_width == 64) {
                do {
                    SpatialFullDistortionKernel64_AVX512_INTRIN(inp, rec, &sum512);
                    inp += input_stride;
                    rec += recon_stride;
                } while (--h);
            } else if (area_width == 96) {
                do {
                    SpatialFullDistortionKernel64_AVX512_INTRIN(inp, rec, &sum512);
                    SpatialFullDistortionKernel32_AVX512_INTRIN(inp + 64, rec + 64, &sum512);
                    inp += input_stride;
                    rec += recon_stride;
                } while (--h);
            } else if (area_width == 128) {
                do {
                    SpatialFullDistortionKernel64_AVX512_INTRIN(inp, rec, &sum512);
                    SpatialFullDistortionKernel64_AVX512_INTRIN(inp + 64, rec + 64, &sum512);
                    inp += input_stride;
                    rec += recon_stride;
                } while (--h);
            } else {
                __m512i sum64 = _mm512_setzero_si512();

                if (area_width & 32) {
                    do {
                        SpatialFullDistortionKernel32_AVX512_INTRIN(inp, rec, &sum512);
                        inp += input_stride;
                        rec += recon_stride;
                    } while (--h);
                    inp = input + 32;
                    rec = recon + 32;
                    h   = area_height;
                    area_width -= 32;
                }

                do {
                    for (uint32_t w = 0; w < area_width; w += 64) {
                        SpatialFullDistortionKernel64_AVX512_INTRIN(inp + w, rec + w, &sum512);
                    }
                    sum32_to64_avx512(&sum512, &sum64);
                    inp += input_stride;
                    rec += recon_stride;
                } while (--h);

                const __m256i sum_L          = _mm512_castsi512_si256(sum64);
                const __m256i sum_H          = _mm512_extracti64x4_epi64(sum64, 1);
                __m256i       leftover_sum64 = _mm256_unpacklo_epi32(sum, _mm256_setzero_si256());
                leftover_sum64 = _mm256_add_epi64(leftover_sum64, _mm256_unpackhi_epi32(sum, _mm256_setzero_si256()));
                sum            = _mm256_add_epi64(sum_L, sum_H);
                sum            = _mm256_add_epi64(sum, leftover_sum64);
                s              = _mm_add_epi64(_mm256_castsi256_si128(sum), _mm256_extracti128_si256(sum, 1));
                return _mm_extract_epi64(s, 0) + _mm_extract_epi64(s, 1);
            }

            const __m256i sum512_L = _mm512_castsi512_si256(sum512);
            const __m256i sum512_H = _mm512_extracti64x4_epi64(sum512, 1);
            sum                    = _mm256_add_epi32(sum, sum512_L);
            sum                    = _mm256_add_epi32(sum, sum512_H);
        }
    }

    return hadd32_avx2_intrin(sum);
}

#endif // EN_AVX512_SUPPORT
