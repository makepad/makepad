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

#include <assert.h>

#include "definitions.h"
#include "compute_sad_c.h"
#include "utility.h"
#include <smmintrin.h>

#include "compute_sad.h"
#include "mcomp.h"

#define UPDATE_BEST(s, k, offset)      \
    tem_sum = _mm_extract_epi32(s, k); \
    if (tem_sum < low_sum) {           \
        low_sum = tem_sum;             \
        x_best  = j + offset + k;      \
        y_best  = i;                   \
    }

void svt_ext_sad_calculation_32x32_64x64_sse4_intrin(uint32_t* p_sad16x16, uint32_t* p_best_sad_32x32,
                                                     uint32_t* p_best_sad_64x64, uint32_t* p_best_mv32x32,
                                                     uint32_t* p_best_mv64x64, uint32_t mv, uint32_t* p_sad32x32) {
    __m128i xmm_n1, sad32x32_greater_than_bitmask, sad32x32_less_than_or_eq_bitmask, best_sad32x32, best_mv_32x32,
        xmm_mv;
    __m128i sad_16x16_0_7_lo, sad_16x16_0_7_hi, sad_16x16_8_15_lo, sad_16x16_8_15_hi, xmm_sad64x64, xmm_sad64x64_total,
        xmm_p_best_sad_32x32, xmm_p_best_mv_32x32;

    sad_16x16_0_7_lo  = _mm_unpacklo_epi32(_mm_loadu_si128((__m128i*)p_sad16x16),
                                          _mm_loadu_si128((__m128i*)(p_sad16x16 + 4)));
    sad_16x16_0_7_hi  = _mm_unpackhi_epi32(_mm_loadu_si128((__m128i*)p_sad16x16),
                                          _mm_loadu_si128((__m128i*)(p_sad16x16 + 4)));
    sad_16x16_8_15_lo = _mm_unpacklo_epi32(_mm_loadu_si128((__m128i*)(p_sad16x16 + 8)),
                                           _mm_loadu_si128((__m128i*)(p_sad16x16 + 12)));
    sad_16x16_8_15_hi = _mm_unpackhi_epi32(_mm_loadu_si128((__m128i*)(p_sad16x16 + 8)),
                                           _mm_loadu_si128((__m128i*)(p_sad16x16 + 12)));

    xmm_sad64x64 = _mm_add_epi32(_mm_add_epi32(_mm_unpacklo_epi64(sad_16x16_0_7_lo, sad_16x16_8_15_lo),
                                               _mm_unpackhi_epi64(sad_16x16_0_7_lo, sad_16x16_8_15_lo)),
                                 _mm_add_epi32(_mm_unpacklo_epi64(sad_16x16_0_7_hi, sad_16x16_8_15_hi),
                                               _mm_unpackhi_epi64(sad_16x16_0_7_hi, sad_16x16_8_15_hi)));

    p_sad32x32[0] = _mm_extract_epi32(xmm_sad64x64, 0);
    p_sad32x32[1] = _mm_extract_epi32(xmm_sad64x64, 1);
    p_sad32x32[2] = _mm_extract_epi32(xmm_sad64x64, 2);
    p_sad32x32[3] = _mm_extract_epi32(xmm_sad64x64, 3);

    xmm_sad64x64_total = _mm_add_epi32(_mm_srli_si128(xmm_sad64x64, 8), xmm_sad64x64);

    xmm_sad64x64_total = _mm_add_epi32(_mm_srli_si128(xmm_sad64x64_total, 4), xmm_sad64x64_total);

    xmm_mv = _mm_cvtsi32_si128(mv);
    xmm_mv = _mm_unpacklo_epi32(xmm_mv, xmm_mv);
    xmm_mv = _mm_unpacklo_epi64(xmm_mv, xmm_mv);

    xmm_p_best_sad_32x32 = _mm_loadu_si128((__m128i*)p_best_sad_32x32);
    xmm_p_best_mv_32x32  = _mm_loadu_si128((__m128i*)p_best_mv32x32);

    sad32x32_greater_than_bitmask = _mm_cmpgt_epi32(
        xmm_p_best_sad_32x32, xmm_sad64x64); // _mm_cmplt_epi32(xmm_p_best_sad_32x32, xmm_sad64x64);

    xmm_n1                           = _mm_cmpeq_epi8(xmm_mv,
                            xmm_mv); // anything compared to itself is equal (get 0xFFFFFFFF)
    sad32x32_less_than_or_eq_bitmask = _mm_sub_epi32(xmm_n1, sad32x32_greater_than_bitmask);

    best_sad32x32 = _mm_or_si128(_mm_and_si128(xmm_p_best_sad_32x32, sad32x32_less_than_or_eq_bitmask),
                                 _mm_and_si128(xmm_sad64x64, sad32x32_greater_than_bitmask));
    best_mv_32x32 = _mm_or_si128(_mm_and_si128(xmm_p_best_mv_32x32, sad32x32_less_than_or_eq_bitmask),
                                 _mm_and_si128(xmm_mv, sad32x32_greater_than_bitmask));

    _mm_storeu_si128((__m128i*)p_best_sad_32x32, best_sad32x32);
    _mm_storeu_si128((__m128i*)p_best_mv32x32, best_mv_32x32);

    uint32_t sad64x64 = _mm_cvtsi128_si32(xmm_sad64x64_total);
    if (sad64x64 < p_best_sad_64x64[0]) {
        p_best_sad_64x64[0] = sad64x64;
        p_best_mv64x64[0]   = _mm_cvtsi128_si32(xmm_mv);
    }
}

uint32_t svt_sad_nxm_combined_sse4_1(const uint8_t* src, uint32_t src_stride, const uint8_t* ref, uint32_t ref_stride,
                                     int32_t height, int32_t width) {
    // these must match block widths implemented in svt_nxm_sad_kernel_helper_sse4_1
    // 1, 2, 3 are added for convenience and will be handled by C code
    static const int available_widths[] = {1, 2, 3, 4, 8, 16, 24, 32, 40, 48, 56, 64};
    int              i                  = sizeof(available_widths) / sizeof(available_widths[0]);
    uint32_t         sad                = 0;
    while (width > 0) {
        while (--i >= 0) {
            int w = available_widths[i];
            if (w <= width) {
                sad += svt_nxm_sad_kernel_helper_sse4_1(src, src_stride, ref, ref_stride, height, w);
                width -= w;
                src += w;
                ref += w;
                break;
            }
        }
    }
    return sad;
}

void svt_sad_loop_kernel_anyxh_sse4_1(uint8_t*  src, // input parameter, source samples Ptr
                                      uint32_t  src_stride, // input parameter, source stride
                                      uint8_t*  ref, // input parameter, reference samples Ptr
                                      uint32_t  ref_stride, // input parameter, reference stride
                                      uint32_t  block_height, // input parameter, block height (M)
                                      uint32_t  block_width, // input parameter, block width (N)
                                      uint64_t* best_sad, int16_t* x_search_center, int16_t* y_search_center,
                                      uint32_t src_stride_raw, // input parameter, source stride (no line skipping)
                                      uint8_t skip_search_line, int16_t search_area_width, int16_t search_area_height) {
    int16_t   x_search_index;
    int16_t   y_search_index;
    const int skip_line_value = (block_width == 16 && block_height <= 16 && skip_search_line) ? 0 : -1;

    for (y_search_index = 0; y_search_index < search_area_height; y_search_index++) {
        if ((y_search_index & 1) == skip_line_value) {
            ref += src_stride_raw;
            continue;
        }
        for (x_search_index = 0; x_search_index < search_area_width; x_search_index++) {
            uint32_t sad = svt_sad_nxm_combined_sse4_1(
                src, src_stride, ref + x_search_index, ref_stride, block_height, block_width);

            // Update results
            if (sad < *best_sad) {
                *best_sad        = sad;
                *x_search_center = x_search_index;
                *y_search_center = y_search_index;
            }
        }

        ref += src_stride_raw;
    }
}

/*******************************************************************************
 * Requirement: width   = 4, 6, 8, 12, 16, 24, 32, 48 or 64 to use SIMD
 * otherwise C version is used
 * Requirement: block_height <= 64
 * Requirement: block_height % 2 = 0 when width = 4, 6 or 8
*******************************************************************************/
void svt_sad_loop_kernel_sse4_1_intrin(uint8_t*  src, // input parameter, source samples Ptr
                                       uint32_t  src_stride, // input parameter, source stride
                                       uint8_t*  ref, // input parameter, reference samples Ptr
                                       uint32_t  ref_stride, // input parameter, reference stride
                                       uint32_t  block_height, // input parameter, block height (M)
                                       uint32_t  block_width, // input parameter, block width (N)
                                       uint64_t* best_sad, int16_t* x_search_center, int16_t* y_search_center,
                                       uint32_t src_stride_raw, // input parameter, source stride (no line skipping)
                                       uint8_t skip_search_line, int16_t search_area_width,
                                       int16_t search_area_height) {
    int16_t        x_best = *x_search_center, y_best = *y_search_center;
    uint32_t       low_sum = 0xffffff;
    uint32_t       tem_sum = 0;
    int16_t        i, j;
    uint32_t       k, l;
    uint32_t       leftover = search_area_width & 7;
    const uint8_t *p_ref, *p_src;
    __m128i        s0, s1, s2, s3, s4, s5, s6, s7, s8 = _mm_set1_epi32(-1);

    if (leftover) {
        for (k = 0; k < leftover; k++) {
            s8 = _mm_slli_si128(s8, 2);
        }
    }

    switch (block_width) {
    case 4:
        for (i = 0; i < search_area_height; i++) {
            for (j = 0; j <= search_area_width - 8; j += 8) {
                p_src = src;
                p_ref = ref + j;
                s3    = _mm_setzero_si128();
                for (k = 0; k + 2 <= block_height; k += 2) {
                    s0 = _mm_loadu_si128((__m128i*)p_ref);
                    s1 = _mm_loadu_si128((__m128i*)(p_ref + ref_stride));
                    s2 = _mm_cvtsi32_si128(*(uint32_t*)p_src);
                    s5 = _mm_cvtsi32_si128(*(uint32_t*)(p_src + src_stride));
                    s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                    s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s1, s5, 0));
                    p_src += src_stride << 1;
                    p_ref += ref_stride << 1;
                }

                if (k < block_height) {
                    s0 = _mm_loadu_si128((__m128i*)p_ref);
                    s2 = _mm_cvtsi32_si128(*(uint32_t*)p_src);
                    s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                    p_src += src_stride << 1;
                    p_ref += ref_stride << 1;
                }

                s3      = _mm_minpos_epu16(s3);
                tem_sum = _mm_extract_epi16(s3, 0);
                if (tem_sum < low_sum) {
                    low_sum = tem_sum;
                    x_best  = (int16_t)(j + _mm_extract_epi16(s3, 1));
                    y_best  = i;
                }
            }

            if (leftover) {
                p_src = src;
                p_ref = ref + j;
                s3    = _mm_setzero_si128();
                for (k = 0; k + 2 <= block_height; k += 2) {
                    s0 = _mm_loadu_si128((__m128i*)p_ref);
                    s1 = _mm_loadu_si128((__m128i*)(p_ref + ref_stride));
                    s2 = _mm_cvtsi32_si128(*(uint32_t*)p_src);
                    s5 = _mm_cvtsi32_si128(*(uint32_t*)(p_src + src_stride));
                    s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                    s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s1, s5, 0));
                    p_src += src_stride << 1;
                    p_ref += ref_stride << 1;
                }

                if (k < block_height) {
                    s0 = _mm_loadu_si128((__m128i*)p_ref);
                    s2 = _mm_cvtsi32_si128(*(uint32_t*)p_src);
                    s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                    p_src += src_stride << 1;
                    p_ref += ref_stride << 1;
                }

                s3      = _mm_or_si128(s3, s8);
                s3      = _mm_minpos_epu16(s3);
                tem_sum = _mm_extract_epi16(s3, 0);
                if (tem_sum < low_sum) {
                    low_sum = tem_sum;
                    x_best  = (int16_t)(j + _mm_extract_epi16(s3, 1));
                    y_best  = i;
                }
            }
            ref += src_stride_raw;
        }
        break;

    case 6:
        for (i = 0; i < search_area_height; i++) {
            for (j = 0; j <= search_area_width - 8; j += 8) {
                p_src = src;
                p_ref = ref + j;
                s3    = _mm_setzero_si128();
                for (k = 0; k + 2 <= block_height; k += 2) {
                    s0 = _mm_loadu_si128((__m128i*)p_ref);
                    s1 = _mm_loadu_si128((__m128i*)(p_ref + ref_stride));
                    s2 = _mm_cvtsi32_si128(*(uint32_t*)p_src);
                    s5 = _mm_cvtsi32_si128(*(uint32_t*)(p_src + src_stride));
                    s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                    s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s1, s5, 0));
                    p_src += src_stride << 1;
                    p_ref += ref_stride << 1;
                }

                if (k < block_height) {
                    s0 = _mm_loadu_si128((__m128i*)p_ref);
                    s2 = _mm_cvtsi32_si128(*(uint32_t*)p_src);
                    s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                    p_src += src_stride << 1;
                    p_ref += ref_stride << 1;
                }

                DECLARE_ALIGNED(16, uint16_t, tsum[8]);
                memset(tsum, 0, 8 * sizeof(uint16_t));
                p_ref = ref + j;
                for (uint32_t search_area = 0; search_area < 8; search_area++) {
                    for (uint32_t y = 0; y < block_height; y++) {
                        tsum[search_area] += EB_ABS_DIFF(src[y * src_stride + 4], p_ref[y * ref_stride + 4]) +
                            EB_ABS_DIFF(src[y * src_stride + 5], p_ref[y * ref_stride + 5]);
                    }
                    p_ref += 1;
                }
                s4 = _mm_loadu_si128((__m128i*)tsum);
                s3 = _mm_adds_epu16(s3, s4);

                s3      = _mm_minpos_epu16(s3);
                tem_sum = _mm_extract_epi16(s3, 0);
                if (tem_sum < low_sum) {
                    low_sum = tem_sum;
                    x_best  = (int16_t)(j + _mm_extract_epi16(s3, 1));
                    y_best  = i;
                }
            }

            if (leftover) {
                p_src = src;
                p_ref = ref + j;
                s3    = _mm_setzero_si128();
                for (k = 0; k + 2 <= block_height; k += 2) {
                    s0 = _mm_loadu_si128((__m128i*)p_ref);
                    s1 = _mm_loadu_si128((__m128i*)(p_ref + ref_stride));
                    s2 = _mm_cvtsi32_si128(*(uint32_t*)p_src);
                    s5 = _mm_cvtsi32_si128(*(uint32_t*)(p_src + src_stride));
                    s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                    s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s1, s5, 0));
                    p_src += src_stride << 1;
                    p_ref += ref_stride << 1;
                }

                if (k < block_height) {
                    s0 = _mm_loadu_si128((__m128i*)p_ref);
                    s2 = _mm_cvtsi32_si128(*(uint32_t*)p_src);
                    s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                    p_src += src_stride << 1;
                    p_ref += ref_stride << 1;
                }

                s3 = _mm_or_si128(s3, s8);

                DECLARE_ALIGNED(16, uint16_t, tsum[8]);
                memset(tsum, 0, 8 * sizeof(uint16_t));
                p_ref = ref + j;
                for (uint32_t search_area = 0; search_area < leftover; search_area++) {
                    for (uint32_t y = 0; y < block_height; y++) {
                        tsum[search_area] += EB_ABS_DIFF(src[y * src_stride + 4], p_ref[y * ref_stride + 4]) +
                            EB_ABS_DIFF(src[y * src_stride + 5], p_ref[y * ref_stride + 5]);
                    }
                    p_ref += 1;
                }
                s4 = _mm_loadu_si128((__m128i*)tsum);
                s3 = _mm_adds_epu16(s3, s4);

                s3      = _mm_minpos_epu16(s3);
                tem_sum = _mm_extract_epi16(s3, 0);
                if (tem_sum < low_sum) {
                    low_sum = tem_sum;
                    x_best  = (int16_t)(j + _mm_extract_epi16(s3, 1));
                    y_best  = i;
                }
            }
            ref += src_stride_raw;
        }
        break;

    case 8:
        for (i = 0; i < search_area_height; i++) {
            for (j = 0; j <= search_area_width - 8; j += 8) {
                p_src = src;
                p_ref = ref + j;
                s3 = s4 = _mm_setzero_si128();
                for (k = 0; k + 2 <= block_height; k += 2) {
                    s0 = _mm_loadu_si128((__m128i*)p_ref);
                    s1 = _mm_loadu_si128((__m128i*)(p_ref + ref_stride));
                    s2 = _mm_loadl_epi64((__m128i*)p_src);
                    s5 = _mm_loadl_epi64((__m128i*)(p_src + src_stride));
                    s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                    s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                    s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s1, s5, 0));
                    s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s1, s5, 5));
                    p_src += src_stride << 1;
                    p_ref += ref_stride << 1;
                }

                if (k < block_height) {
                    s0 = _mm_loadu_si128((__m128i*)p_ref);
                    s2 = _mm_loadl_epi64((__m128i*)p_src);
                    s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                    s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                    p_src += src_stride << 1;
                    p_ref += ref_stride << 1;
                }

                s3      = _mm_adds_epu16(s3, s4);
                s3      = _mm_minpos_epu16(s3);
                tem_sum = _mm_extract_epi16(s3, 0);
                if (tem_sum < low_sum) {
                    low_sum = tem_sum;
                    x_best  = (int16_t)(j + _mm_extract_epi16(s3, 1));
                    y_best  = i;
                }
            }

            if (leftover) {
                p_src = src;
                p_ref = ref + j;
                s3 = s4 = _mm_setzero_si128();
                for (k = 0; k + 2 <= block_height; k += 2) {
                    s0 = _mm_loadu_si128((__m128i*)p_ref);
                    s1 = _mm_loadu_si128((__m128i*)(p_ref + ref_stride));
                    s2 = _mm_loadl_epi64((__m128i*)p_src);
                    s5 = _mm_loadl_epi64((__m128i*)(p_src + src_stride));
                    s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                    s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                    s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s1, s5, 0));
                    s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s1, s5, 5));
                    p_src += src_stride << 1;
                    p_ref += ref_stride << 1;
                }

                if (k < block_height) {
                    s0 = _mm_loadu_si128((__m128i*)p_ref);
                    s2 = _mm_loadl_epi64((__m128i*)p_src);
                    s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                    s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                    p_src += src_stride << 1;
                    p_ref += ref_stride << 1;
                }

                s3      = _mm_adds_epu16(s3, s4);
                s3      = _mm_or_si128(s3, s8);
                s3      = _mm_minpos_epu16(s3);
                tem_sum = _mm_extract_epi16(s3, 0);
                if (tem_sum < low_sum) {
                    low_sum = tem_sum;
                    x_best  = (int16_t)(j + _mm_extract_epi16(s3, 1));
                    y_best  = i;
                }
            }
            ref += src_stride_raw;
        }
        break;

    case 12:
        if (block_height <= 16) {
            for (i = 0; i < search_area_height; i++) {
                for (j = 0; j <= search_area_width - 8; j += 8) {
                    p_src = src;
                    p_ref = ref + j;
                    s3 = s4 = s5 = s6 = _mm_setzero_si128();
                    for (k = 0; k < block_height; k++) {
                        s0 = _mm_loadu_si128((__m128i*)p_ref);
                        s1 = _mm_loadu_si128((__m128i*)(p_ref + 8));
                        s2 = _mm_loadu_si128((__m128i*)p_src);
                        s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                        s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                        s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                        p_src += src_stride;
                        p_ref += ref_stride;
                    }
                    s3      = _mm_adds_epu16(_mm_adds_epu16(s3, s4), s5);
                    s3      = _mm_minpos_epu16(s3);
                    tem_sum = _mm_extract_epi16(s3, 0);
                    if (tem_sum < low_sum) {
                        low_sum = tem_sum;
                        x_best  = (int16_t)(j + _mm_extract_epi16(s3, 1));
                        y_best  = i;
                    }
                }

                if (leftover) {
                    p_src = src;
                    p_ref = ref + j;
                    s3 = s4 = s5 = s6 = _mm_setzero_si128();
                    for (k = 0; k < block_height; k++) {
                        s0 = _mm_loadu_si128((__m128i*)p_ref);
                        s1 = _mm_loadu_si128((__m128i*)(p_ref + 8));
                        s2 = _mm_loadu_si128((__m128i*)p_src);
                        s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                        s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                        s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                        p_src += src_stride;
                        p_ref += ref_stride;
                    }
                    s3      = _mm_adds_epu16(_mm_adds_epu16(s3, s4), s5);
                    s3      = _mm_or_si128(s3, s8);
                    s3      = _mm_minpos_epu16(s3);
                    tem_sum = _mm_extract_epi16(s3, 0);
                    if (tem_sum < low_sum) {
                        low_sum = tem_sum;
                        x_best  = (int16_t)(j + _mm_extract_epi16(s3, 1));
                        y_best  = i;
                    }
                }
                ref += src_stride_raw;
            }
        } else {
            for (i = 0; i < search_area_height; i++) {
                for (j = 0; j <= search_area_width - 8; j += 8) {
                    p_src = src;
                    p_ref = ref + j;
                    s3 = s4 = s5 = _mm_setzero_si128();
                    for (k = 0; k < block_height; k++) {
                        s0 = _mm_loadu_si128((__m128i*)p_ref);
                        s1 = _mm_loadu_si128((__m128i*)(p_ref + 8));
                        s2 = _mm_loadu_si128((__m128i*)p_src);
                        s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                        s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                        s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                        p_src += src_stride;
                        p_ref += ref_stride;
                    }
                    s0      = _mm_adds_epu16(_mm_adds_epu16(s3, s4), s5);
                    s0      = _mm_minpos_epu16(s0);
                    tem_sum = _mm_extract_epi16(s0, 0);
                    if (tem_sum < low_sum) {
                        if (tem_sum != 0xFFFF) { // no overflow
                            low_sum = tem_sum;
                            x_best  = (int16_t)(j + _mm_extract_epi16(s0, 1));
                            y_best  = i;
                        } else {
                            s0 = _mm_unpacklo_epi16(s3, _mm_setzero_si128());
                            s3 = _mm_unpackhi_epi16(s3, _mm_setzero_si128());
                            s1 = _mm_unpacklo_epi16(s4, _mm_setzero_si128());
                            s4 = _mm_unpackhi_epi16(s4, _mm_setzero_si128());
                            s2 = _mm_unpacklo_epi16(s5, _mm_setzero_si128());
                            s5 = _mm_unpackhi_epi16(s5, _mm_setzero_si128());
                            s0 = _mm_add_epi32(_mm_add_epi32(s0, s1), s2);
                            s3 = _mm_add_epi32(_mm_add_epi32(s3, s4), s5);
                            UPDATE_BEST(s0, 0, 0);
                            UPDATE_BEST(s0, 1, 0);
                            UPDATE_BEST(s0, 2, 0);
                            UPDATE_BEST(s0, 3, 0);
                            UPDATE_BEST(s3, 0, 4);
                            UPDATE_BEST(s3, 1, 4);
                            UPDATE_BEST(s3, 2, 4);
                            UPDATE_BEST(s3, 3, 4);
                        }
                    }
                }

                if (leftover) {
                    p_src = src;
                    p_ref = ref + j;
                    s3 = s4 = s5 = _mm_setzero_si128();
                    for (k = 0; k < block_height; k++) {
                        s0 = _mm_loadu_si128((__m128i*)p_ref);
                        s1 = _mm_loadu_si128((__m128i*)(p_ref + 8));
                        s2 = _mm_loadu_si128((__m128i*)p_src);
                        s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                        s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                        s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                        p_src += src_stride;
                        p_ref += ref_stride;
                    }
                    s0      = _mm_adds_epu16(_mm_adds_epu16(s3, s4), s5);
                    s0      = _mm_or_si128(s0, s8);
                    s0      = _mm_minpos_epu16(s0);
                    tem_sum = _mm_extract_epi16(s0, 0);
                    if (tem_sum < low_sum) {
                        if (tem_sum != 0xFFFF) { // no overflow
                            low_sum = tem_sum;
                            x_best  = (int16_t)(j + _mm_extract_epi16(s0, 1));
                            y_best  = i;
                        } else {
                            s0 = _mm_unpacklo_epi16(s3, _mm_setzero_si128());
                            s3 = _mm_unpackhi_epi16(s3, _mm_setzero_si128());
                            s1 = _mm_unpacklo_epi16(s4, _mm_setzero_si128());
                            s4 = _mm_unpackhi_epi16(s4, _mm_setzero_si128());
                            s2 = _mm_unpacklo_epi16(s5, _mm_setzero_si128());
                            s5 = _mm_unpackhi_epi16(s5, _mm_setzero_si128());
                            s0 = _mm_add_epi32(_mm_add_epi32(s0, s1), s2);
                            s3 = _mm_add_epi32(_mm_add_epi32(s3, s4), s5);
                            k  = leftover;
                            while (k > 0) {
                                for (l = 0; l < 4 && k; l++, k--) {
                                    tem_sum = _mm_extract_epi32(s0, 0);
                                    s0      = _mm_srli_si128(s0, 4);
                                    if (tem_sum < low_sum) {
                                        low_sum = tem_sum;
                                        x_best  = (int16_t)(j + leftover - k);
                                        y_best  = i;
                                    }
                                }
                                s0 = s3;
                            }
                        }
                    }
                }
                ref += src_stride_raw;
            }
        }
        break;

    case 16:
        if (block_height <= 16) {
            for (i = 0; i < search_area_height; i++) {
                if (skip_search_line) {
                    if ((i & 1) == 0) {
                        ref += src_stride_raw;
                        continue;
                    }
                }
                for (j = 0; j <= search_area_width - 8; j += 8) {
                    p_src = src;
                    p_ref = ref + j;
                    s3 = s4 = s5 = s6 = _mm_setzero_si128();
                    for (k = 0; k < block_height; k++) {
                        s0 = _mm_loadu_si128((__m128i*)p_ref);
                        s1 = _mm_loadu_si128((__m128i*)(p_ref + 8));
                        s2 = _mm_loadu_si128((__m128i*)p_src);
                        s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                        s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                        s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                        s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                        p_src += src_stride;
                        p_ref += ref_stride;
                    }
                    s3      = _mm_adds_epu16(_mm_adds_epu16(s3, s4), _mm_adds_epu16(s5, s6));
                    s3      = _mm_minpos_epu16(s3);
                    tem_sum = _mm_extract_epi16(s3, 0);
                    if (tem_sum < low_sum) {
                        low_sum = tem_sum;
                        x_best  = (int16_t)(j + _mm_extract_epi16(s3, 1));
                        y_best  = i;
                    }
                }

                if (leftover) {
                    p_src = src;
                    p_ref = ref + j;
                    s3 = s4 = s5 = s6 = _mm_setzero_si128();
                    for (k = 0; k < block_height; k++) {
                        s0 = _mm_loadu_si128((__m128i*)p_ref);
                        s1 = _mm_loadu_si128((__m128i*)(p_ref + 8));
                        s2 = _mm_loadu_si128((__m128i*)p_src);
                        s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                        s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                        s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                        s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                        p_src += src_stride;
                        p_ref += ref_stride;
                    }
                    s3      = _mm_adds_epu16(_mm_adds_epu16(s3, s4), _mm_adds_epu16(s5, s6));
                    s3      = _mm_or_si128(s3, s8);
                    s3      = _mm_minpos_epu16(s3);
                    tem_sum = _mm_extract_epi16(s3, 0);
                    if (tem_sum < low_sum) {
                        low_sum = tem_sum;
                        x_best  = (int16_t)(j + _mm_extract_epi16(s3, 1));
                        y_best  = i;
                    }
                }
                ref += src_stride_raw;
            }
        } else if (block_height <= 32) {
            for (i = 0; i < search_area_height; i++) {
                for (j = 0; j <= search_area_width - 8; j += 8) {
                    p_src = src;
                    p_ref = ref + j;
                    s3 = s4 = s5 = s6 = _mm_setzero_si128();
                    for (k = 0; k < block_height; k++) {
                        s0 = _mm_loadu_si128((__m128i*)p_ref);
                        s1 = _mm_loadu_si128((__m128i*)(p_ref + 8));
                        s2 = _mm_loadu_si128((__m128i*)p_src);
                        s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                        s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                        s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                        s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                        p_src += src_stride;
                        p_ref += ref_stride;
                    }
                    s3      = _mm_adds_epu16(s3, s4);
                    s5      = _mm_adds_epu16(s5, s6);
                    s4      = _mm_minpos_epu16(s3);
                    s6      = _mm_minpos_epu16(s5);
                    s4      = _mm_unpacklo_epi16(s4, s4);
                    s4      = _mm_unpacklo_epi32(s4, s4);
                    s4      = _mm_unpacklo_epi64(s4, s4);
                    s6      = _mm_unpacklo_epi16(s6, s6);
                    s6      = _mm_unpacklo_epi32(s6, s6);
                    s6      = _mm_unpacklo_epi64(s6, s6);
                    s3      = _mm_sub_epi16(s3, s4);
                    s5      = _mm_adds_epu16(s5, s3);
                    s5      = _mm_sub_epi16(s5, s6);
                    s5      = _mm_minpos_epu16(s5);
                    tem_sum = _mm_extract_epi16(s5, 0);
                    tem_sum += _mm_extract_epi16(s4, 0);
                    tem_sum += _mm_extract_epi16(s6, 0);
                    if (tem_sum < low_sum) {
                        low_sum = tem_sum;
                        x_best  = (int16_t)(j + _mm_extract_epi16(s5, 1));
                        y_best  = i;
                    }
                }

                if (leftover) {
                    p_src = src;
                    p_ref = ref + j;
                    s3 = s4 = s5 = s6 = _mm_setzero_si128();
                    for (k = 0; k < block_height; k++) {
                        s0 = _mm_loadu_si128((__m128i*)p_ref);
                        s1 = _mm_loadu_si128((__m128i*)(p_ref + 8));
                        s2 = _mm_loadu_si128((__m128i*)p_src);
                        s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                        s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                        s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                        s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                        p_src += src_stride;
                        p_ref += ref_stride;
                    }
                    s3      = _mm_adds_epu16(s3, s4);
                    s5      = _mm_adds_epu16(s5, s6);
                    s3      = _mm_or_si128(s3, s8);
                    s5      = _mm_or_si128(s5, s8);
                    s4      = _mm_minpos_epu16(s3);
                    s6      = _mm_minpos_epu16(s5);
                    s4      = _mm_unpacklo_epi16(s4, s4);
                    s4      = _mm_unpacklo_epi32(s4, s4);
                    s4      = _mm_unpacklo_epi64(s4, s4);
                    s6      = _mm_unpacklo_epi16(s6, s6);
                    s6      = _mm_unpacklo_epi32(s6, s6);
                    s6      = _mm_unpacklo_epi64(s6, s6);
                    s3      = _mm_sub_epi16(s3, s4);
                    s5      = _mm_adds_epu16(s5, s3);
                    s5      = _mm_sub_epi16(s5, s6);
                    s5      = _mm_minpos_epu16(s5);
                    tem_sum = _mm_extract_epi16(s5, 0);
                    tem_sum += _mm_extract_epi16(s4, 0);
                    tem_sum += _mm_extract_epi16(s6, 0);
                    if (tem_sum < low_sum) {
                        low_sum = tem_sum;
                        x_best  = (int16_t)(j + _mm_extract_epi16(s5, 1));
                        y_best  = i;
                    }
                }
                ref += src_stride_raw;
            }
        } else {
            for (i = 0; i < search_area_height; i++) {
                for (j = 0; j <= search_area_width - 8; j += 8) {
                    p_src = src;
                    p_ref = ref + j;
                    s3 = s4 = s5 = s6 = _mm_setzero_si128();
                    for (k = 0; k < block_height; k++) {
                        s0 = _mm_loadu_si128((__m128i*)p_ref);
                        s1 = _mm_loadu_si128((__m128i*)(p_ref + 8));
                        s2 = _mm_loadu_si128((__m128i*)p_src);
                        s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                        s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                        s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                        s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                        p_src += src_stride;
                        p_ref += ref_stride;
                    }
                    s0      = _mm_adds_epu16(_mm_adds_epu16(s3, s4), _mm_adds_epu16(s5, s6));
                    s0      = _mm_minpos_epu16(s0);
                    tem_sum = _mm_extract_epi16(s0, 0);
                    if (tem_sum < low_sum) {
                        if (tem_sum != 0xFFFF) { // no overflow
                            low_sum = tem_sum;
                            x_best  = (int16_t)(j + _mm_extract_epi16(s0, 1));
                            y_best  = i;
                        } else {
                            s0 = _mm_unpacklo_epi16(s3, _mm_setzero_si128());
                            s3 = _mm_unpackhi_epi16(s3, _mm_setzero_si128());
                            s1 = _mm_unpacklo_epi16(s4, _mm_setzero_si128());
                            s4 = _mm_unpackhi_epi16(s4, _mm_setzero_si128());
                            s2 = _mm_unpacklo_epi16(s5, _mm_setzero_si128());
                            s5 = _mm_unpackhi_epi16(s5, _mm_setzero_si128());
                            s7 = _mm_unpacklo_epi16(s6, _mm_setzero_si128());
                            s6 = _mm_unpackhi_epi16(s6, _mm_setzero_si128());
                            s0 = _mm_add_epi32(_mm_add_epi32(s0, s1), _mm_add_epi32(s2, s7));
                            s3 = _mm_add_epi32(_mm_add_epi32(s3, s4), _mm_add_epi32(s5, s6));
                            UPDATE_BEST(s0, 0, 0);
                            UPDATE_BEST(s0, 1, 0);
                            UPDATE_BEST(s0, 2, 0);
                            UPDATE_BEST(s0, 3, 0);
                            UPDATE_BEST(s3, 0, 4);
                            UPDATE_BEST(s3, 1, 4);
                            UPDATE_BEST(s3, 2, 4);
                            UPDATE_BEST(s3, 3, 4);
                        }
                    }
                }

                if (leftover) {
                    p_src = src;
                    p_ref = ref + j;
                    s3 = s4 = s5 = s6 = _mm_setzero_si128();
                    for (k = 0; k < block_height; k++) {
                        s0 = _mm_loadu_si128((__m128i*)p_ref);
                        s1 = _mm_loadu_si128((__m128i*)(p_ref + 8));
                        s2 = _mm_loadu_si128((__m128i*)p_src);
                        s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                        s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                        s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                        s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                        p_src += src_stride;
                        p_ref += ref_stride;
                    }
                    s0      = _mm_adds_epu16(_mm_adds_epu16(s3, s4), _mm_adds_epu16(s5, s6));
                    s0      = _mm_or_si128(s0, s8);
                    s0      = _mm_minpos_epu16(s0);
                    tem_sum = _mm_extract_epi16(s0, 0);
                    if (tem_sum < low_sum) {
                        if (tem_sum != 0xFFFF) { // no overflow
                            low_sum = tem_sum;
                            x_best  = (int16_t)(j + _mm_extract_epi16(s0, 1));
                            y_best  = i;
                        } else {
                            s0 = _mm_unpacklo_epi16(s3, _mm_setzero_si128());
                            s3 = _mm_unpackhi_epi16(s3, _mm_setzero_si128());
                            s1 = _mm_unpacklo_epi16(s4, _mm_setzero_si128());
                            s4 = _mm_unpackhi_epi16(s4, _mm_setzero_si128());
                            s2 = _mm_unpacklo_epi16(s5, _mm_setzero_si128());
                            s5 = _mm_unpackhi_epi16(s5, _mm_setzero_si128());
                            s7 = _mm_unpacklo_epi16(s6, _mm_setzero_si128());
                            s6 = _mm_unpackhi_epi16(s6, _mm_setzero_si128());
                            s0 = _mm_add_epi32(_mm_add_epi32(s0, s1), _mm_add_epi32(s2, s7));
                            s3 = _mm_add_epi32(_mm_add_epi32(s3, s4), _mm_add_epi32(s5, s6));
                            k  = leftover;
                            while (k > 0) {
                                for (l = 0; l < 4 && k; l++, k--) {
                                    tem_sum = _mm_extract_epi32(s0, 0);
                                    s0      = _mm_srli_si128(s0, 4);
                                    if (tem_sum < low_sum) {
                                        low_sum = tem_sum;
                                        x_best  = (int16_t)(j + leftover - k);
                                        y_best  = i;
                                    }
                                }
                                s0 = s3;
                            }
                        }
                    }
                }
                ref += src_stride_raw;
            }
        }
        break;

    case 24:
        if (block_height <= 16) {
            for (i = 0; i < search_area_height; i++) {
                for (j = 0; j <= search_area_width - 8; j += 8) {
                    p_src = src;
                    p_ref = ref + j;
                    s3 = s4 = s5 = s6 = _mm_setzero_si128();
                    for (k = 0; k < block_height; k++) {
                        s0 = _mm_loadu_si128((__m128i*)p_ref);
                        s1 = _mm_loadu_si128((__m128i*)(p_ref + 8));
                        s2 = _mm_loadu_si128((__m128i*)p_src);
                        s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                        s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                        s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                        s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                        s0 = _mm_loadu_si128((__m128i*)(p_ref + 16));
                        s2 = _mm_loadl_epi64((__m128i*)(p_src + 16));
                        s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                        s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                        p_src += src_stride;
                        p_ref += ref_stride;
                    }
                    s3      = _mm_adds_epu16(s3, s4);
                    s5      = _mm_adds_epu16(s5, s6);
                    s4      = _mm_minpos_epu16(s3);
                    s6      = _mm_minpos_epu16(s5);
                    s4      = _mm_unpacklo_epi16(s4, s4);
                    s4      = _mm_unpacklo_epi32(s4, s4);
                    s4      = _mm_unpacklo_epi64(s4, s4);
                    s6      = _mm_unpacklo_epi16(s6, s6);
                    s6      = _mm_unpacklo_epi32(s6, s6);
                    s6      = _mm_unpacklo_epi64(s6, s6);
                    s3      = _mm_sub_epi16(s3, s4);
                    s5      = _mm_adds_epu16(s5, s3);
                    s5      = _mm_sub_epi16(s5, s6);
                    s5      = _mm_minpos_epu16(s5);
                    tem_sum = _mm_extract_epi16(s5, 0);
                    tem_sum += _mm_extract_epi16(s4, 0);
                    tem_sum += _mm_extract_epi16(s6, 0);
                    if (tem_sum < low_sum) {
                        low_sum = tem_sum;
                        x_best  = (int16_t)(j + _mm_extract_epi16(s5, 1));
                        y_best  = i;
                    }
                }

                if (leftover) {
                    p_src = src;
                    p_ref = ref + j;
                    s3 = s4 = s5 = s6 = _mm_setzero_si128();
                    for (k = 0; k < block_height; k++) {
                        s0 = _mm_loadu_si128((__m128i*)p_ref);
                        s1 = _mm_loadu_si128((__m128i*)(p_ref + 8));
                        s2 = _mm_loadu_si128((__m128i*)p_src);
                        s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                        s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                        s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                        s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                        s0 = _mm_loadu_si128((__m128i*)(p_ref + 16));
                        s2 = _mm_loadl_epi64((__m128i*)(p_src + 16));
                        s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                        s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                        p_src += src_stride;
                        p_ref += ref_stride;
                    }
                    s3      = _mm_adds_epu16(s3, s4);
                    s5      = _mm_adds_epu16(s5, s6);
                    s3      = _mm_or_si128(s3, s8);
                    s5      = _mm_or_si128(s5, s8);
                    s4      = _mm_minpos_epu16(s3);
                    s6      = _mm_minpos_epu16(s5);
                    s4      = _mm_unpacklo_epi16(s4, s4);
                    s4      = _mm_unpacklo_epi32(s4, s4);
                    s4      = _mm_unpacklo_epi64(s4, s4);
                    s6      = _mm_unpacklo_epi16(s6, s6);
                    s6      = _mm_unpacklo_epi32(s6, s6);
                    s6      = _mm_unpacklo_epi64(s6, s6);
                    s3      = _mm_sub_epi16(s3, s4);
                    s5      = _mm_adds_epu16(s5, s3);
                    s5      = _mm_sub_epi16(s5, s6);
                    s5      = _mm_minpos_epu16(s5);
                    tem_sum = _mm_extract_epi16(s5, 0);
                    tem_sum += _mm_extract_epi16(s4, 0);
                    tem_sum += _mm_extract_epi16(s6, 0);
                    if (tem_sum < low_sum) {
                        low_sum = tem_sum;
                        x_best  = (int16_t)(j + _mm_extract_epi16(s5, 1));
                        y_best  = i;
                    }
                }
                ref += src_stride_raw;
            }
        } else {
            for (i = 0; i < search_area_height; i++) {
                for (j = 0; j <= search_area_width - 8; j += 8) {
                    p_src = src;
                    p_ref = ref + j;
                    s3 = s4 = s5 = s6 = _mm_setzero_si128();
                    for (k = 0; k < block_height; k++) {
                        s0 = _mm_loadu_si128((__m128i*)p_ref);
                        s1 = _mm_loadu_si128((__m128i*)(p_ref + 8));
                        s2 = _mm_loadu_si128((__m128i*)p_src);
                        s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                        s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                        s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                        s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                        s0 = _mm_loadu_si128((__m128i*)(p_ref + 16));
                        s2 = _mm_loadl_epi64((__m128i*)(p_src + 16));
                        s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                        s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                        p_src += src_stride;
                        p_ref += ref_stride;
                    }
                    s0      = _mm_adds_epu16(_mm_adds_epu16(s3, s4), _mm_adds_epu16(s5, s6));
                    s0      = _mm_minpos_epu16(s0);
                    tem_sum = _mm_extract_epi16(s0, 0);
                    if (tem_sum < low_sum) {
                        if (tem_sum != 0xFFFF) { // no overflow
                            low_sum = tem_sum;
                            x_best  = (int16_t)(j + _mm_extract_epi16(s0, 1));
                            y_best  = i;
                        } else {
                            s0 = _mm_unpacklo_epi16(s3, _mm_setzero_si128());
                            s3 = _mm_unpackhi_epi16(s3, _mm_setzero_si128());
                            s1 = _mm_unpacklo_epi16(s4, _mm_setzero_si128());
                            s4 = _mm_unpackhi_epi16(s4, _mm_setzero_si128());
                            s2 = _mm_unpacklo_epi16(s5, _mm_setzero_si128());
                            s5 = _mm_unpackhi_epi16(s5, _mm_setzero_si128());
                            s7 = _mm_unpacklo_epi16(s6, _mm_setzero_si128());
                            s6 = _mm_unpackhi_epi16(s6, _mm_setzero_si128());
                            s0 = _mm_add_epi32(_mm_add_epi32(s0, s1), _mm_add_epi32(s2, s7));
                            s3 = _mm_add_epi32(_mm_add_epi32(s3, s4), _mm_add_epi32(s5, s6));
                            UPDATE_BEST(s0, 0, 0);
                            UPDATE_BEST(s0, 1, 0);
                            UPDATE_BEST(s0, 2, 0);
                            UPDATE_BEST(s0, 3, 0);
                            UPDATE_BEST(s3, 0, 4);
                            UPDATE_BEST(s3, 1, 4);
                            UPDATE_BEST(s3, 2, 4);
                            UPDATE_BEST(s3, 3, 4);
                        }
                    }
                }

                if (leftover) {
                    p_src = src;
                    p_ref = ref + j;
                    s3 = s4 = s5 = s6 = _mm_setzero_si128();
                    for (k = 0; k < block_height; k++) {
                        s0 = _mm_loadu_si128((__m128i*)p_ref);
                        s1 = _mm_loadu_si128((__m128i*)(p_ref + 8));
                        s2 = _mm_loadu_si128((__m128i*)p_src);
                        s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                        s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                        s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                        s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                        s0 = _mm_loadu_si128((__m128i*)(p_ref + 16));
                        s2 = _mm_loadl_epi64((__m128i*)(p_src + 16));
                        s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                        s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                        p_src += src_stride;
                        p_ref += ref_stride;
                    }
                    s0      = _mm_adds_epu16(_mm_adds_epu16(s3, s4), _mm_adds_epu16(s5, s6));
                    s0      = _mm_or_si128(s0, s8);
                    s0      = _mm_minpos_epu16(s0);
                    tem_sum = _mm_extract_epi16(s0, 0);
                    if (tem_sum < low_sum) {
                        if (tem_sum != 0xFFFF) { // no overflow
                            low_sum = tem_sum;
                            x_best  = (int16_t)(j + _mm_extract_epi16(s0, 1));
                            y_best  = i;
                        } else {
                            s0 = _mm_unpacklo_epi16(s3, _mm_setzero_si128());
                            s3 = _mm_unpackhi_epi16(s3, _mm_setzero_si128());
                            s1 = _mm_unpacklo_epi16(s4, _mm_setzero_si128());
                            s4 = _mm_unpackhi_epi16(s4, _mm_setzero_si128());
                            s2 = _mm_unpacklo_epi16(s5, _mm_setzero_si128());
                            s5 = _mm_unpackhi_epi16(s5, _mm_setzero_si128());
                            s7 = _mm_unpacklo_epi16(s6, _mm_setzero_si128());
                            s6 = _mm_unpackhi_epi16(s6, _mm_setzero_si128());
                            s0 = _mm_add_epi32(_mm_add_epi32(s0, s1), _mm_add_epi32(s2, s7));
                            s3 = _mm_add_epi32(_mm_add_epi32(s3, s4), _mm_add_epi32(s5, s6));
                            k  = leftover;
                            while (k > 0) {
                                for (l = 0; l < 4 && k; l++, k--) {
                                    tem_sum = _mm_extract_epi32(s0, 0);
                                    s0      = _mm_srli_si128(s0, 4);
                                    if (tem_sum < low_sum) {
                                        low_sum = tem_sum;
                                        x_best  = (int16_t)(j + leftover - k);
                                        y_best  = i;
                                    }
                                }
                                s0 = s3;
                            }
                        }
                    }
                }
                ref += src_stride_raw;
            }
        }
        break;

    case 32:
        if (block_height <= 32) {
            for (i = 0; i < search_area_height; i++) {
                for (j = 0; j <= search_area_width - 8; j += 8) {
                    p_src = src;
                    p_ref = ref + j;
                    s3 = s4 = s5 = s6 = _mm_setzero_si128();
                    for (k = 0; k < block_height; k++) {
                        s0 = _mm_loadu_si128((__m128i*)p_ref);
                        s1 = _mm_loadu_si128((__m128i*)(p_ref + 8));
                        s2 = _mm_loadu_si128((__m128i*)p_src);
                        s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                        s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                        s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                        s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                        s0 = _mm_loadu_si128((__m128i*)(p_ref + 16));
                        s1 = _mm_loadu_si128((__m128i*)(p_ref + 24));
                        s2 = _mm_loadu_si128((__m128i*)(p_src + 16));
                        s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                        s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                        s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                        s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                        p_src += src_stride;
                        p_ref += ref_stride;
                    }
                    s0      = _mm_adds_epu16(_mm_adds_epu16(s3, s4), _mm_adds_epu16(s5, s6));
                    s0      = _mm_minpos_epu16(s0);
                    tem_sum = _mm_extract_epi16(s0, 0);
                    tem_sum &= 0x0000FFFF;
                    if (tem_sum < low_sum) {
                        if (tem_sum != 0xFFFF) { // no overflow
                            low_sum = tem_sum;
                            x_best  = (int16_t)(j + _mm_extract_epi16(s0, 1));
                            y_best  = i;
                        } else {
                            s0 = _mm_unpacklo_epi16(s3, _mm_setzero_si128());
                            s3 = _mm_unpackhi_epi16(s3, _mm_setzero_si128());
                            s1 = _mm_unpacklo_epi16(s4, _mm_setzero_si128());
                            s4 = _mm_unpackhi_epi16(s4, _mm_setzero_si128());
                            s2 = _mm_unpacklo_epi16(s5, _mm_setzero_si128());
                            s5 = _mm_unpackhi_epi16(s5, _mm_setzero_si128());
                            s7 = _mm_unpacklo_epi16(s6, _mm_setzero_si128());
                            s6 = _mm_unpackhi_epi16(s6, _mm_setzero_si128());
                            s0 = _mm_add_epi32(_mm_add_epi32(s0, s1), _mm_add_epi32(s2, s7));
                            s3 = _mm_add_epi32(_mm_add_epi32(s3, s4), _mm_add_epi32(s5, s6));
                            UPDATE_BEST(s0, 0, 0);
                            UPDATE_BEST(s0, 1, 0);
                            UPDATE_BEST(s0, 2, 0);
                            UPDATE_BEST(s0, 3, 0);
                            UPDATE_BEST(s3, 0, 4);
                            UPDATE_BEST(s3, 1, 4);
                            UPDATE_BEST(s3, 2, 4);
                            UPDATE_BEST(s3, 3, 4);
                        }
                    }
                }

                if (leftover) {
                    p_src = src;
                    p_ref = ref + j;
                    s3 = s4 = s5 = s6 = _mm_setzero_si128();
                    for (k = 0; k < block_height; k++) {
                        s0 = _mm_loadu_si128((__m128i*)p_ref);
                        s1 = _mm_loadu_si128((__m128i*)(p_ref + 8));
                        s2 = _mm_loadu_si128((__m128i*)p_src);
                        s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                        s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                        s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                        s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                        s0 = _mm_loadu_si128((__m128i*)(p_ref + 16));
                        s1 = _mm_loadu_si128((__m128i*)(p_ref + 24));
                        s2 = _mm_loadu_si128((__m128i*)(p_src + 16));
                        s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                        s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                        s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                        s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                        p_src += src_stride;
                        p_ref += ref_stride;
                    }
                    s0      = _mm_adds_epu16(_mm_adds_epu16(s3, s4), _mm_adds_epu16(s5, s6));
                    s0      = _mm_or_si128(s0, s8);
                    s0      = _mm_minpos_epu16(s0);
                    tem_sum = _mm_extract_epi16(s0, 0);
                    tem_sum &= 0x0000FFFF;
                    if (tem_sum < low_sum) {
                        if (tem_sum != 0xFFFF) { // no overflow
                            low_sum = tem_sum;
                            x_best  = (int16_t)(j + _mm_extract_epi16(s0, 1));
                            y_best  = i;
                        } else {
                            s0 = _mm_unpacklo_epi16(s3, _mm_setzero_si128());
                            s3 = _mm_unpackhi_epi16(s3, _mm_setzero_si128());
                            s1 = _mm_unpacklo_epi16(s4, _mm_setzero_si128());
                            s4 = _mm_unpackhi_epi16(s4, _mm_setzero_si128());
                            s2 = _mm_unpacklo_epi16(s5, _mm_setzero_si128());
                            s5 = _mm_unpackhi_epi16(s5, _mm_setzero_si128());
                            s7 = _mm_unpacklo_epi16(s6, _mm_setzero_si128());
                            s6 = _mm_unpackhi_epi16(s6, _mm_setzero_si128());
                            s0 = _mm_add_epi32(_mm_add_epi32(s0, s1), _mm_add_epi32(s2, s7));
                            s3 = _mm_add_epi32(_mm_add_epi32(s3, s4), _mm_add_epi32(s5, s6));
                            k  = leftover;
                            while (k > 0) {
                                for (l = 0; l < 4 && k; l++, k--) {
                                    tem_sum = _mm_extract_epi32(s0, 0);
                                    s0      = _mm_srli_si128(s0, 4);
                                    if (tem_sum < low_sum) {
                                        low_sum = tem_sum;
                                        x_best  = (int16_t)(j + leftover - k);
                                        y_best  = i;
                                    }
                                }
                                s0 = s3;
                            }
                        }
                    }
                }
                ref += src_stride_raw;
            }
        } else {
            __m128i s9, s10, s11, s12;
            for (i = 0; i < search_area_height; i++) {
                for (j = 0; j <= search_area_width - 8; j += 8) {
                    p_src = src;
                    p_ref = ref + j;
                    s3 = s4 = s5 = s6 = _mm_setzero_si128();
                    for (k = 0; k < (block_height >> 1); k++) {
                        s0 = _mm_loadu_si128((__m128i*)p_ref);
                        s1 = _mm_loadu_si128((__m128i*)(p_ref + 8));
                        s2 = _mm_loadu_si128((__m128i*)p_src);
                        s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                        s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                        s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                        s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                        s0 = _mm_loadu_si128((__m128i*)(p_ref + 16));
                        s1 = _mm_loadu_si128((__m128i*)(p_ref + 24));
                        s2 = _mm_loadu_si128((__m128i*)(p_src + 16));
                        s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                        s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                        s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                        s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                        p_src += src_stride;
                        p_ref += ref_stride;
                    }
                    s9 = s10 = s11 = s12 = _mm_setzero_si128();
                    for (; k < block_height; k++) {
                        s0  = _mm_loadu_si128((__m128i*)p_ref);
                        s1  = _mm_loadu_si128((__m128i*)(p_ref + 8));
                        s2  = _mm_loadu_si128((__m128i*)p_src);
                        s9  = _mm_adds_epu16(s9, _mm_mpsadbw_epu8(s0, s2, 0));
                        s10 = _mm_adds_epu16(s10, _mm_mpsadbw_epu8(s0, s2, 5));
                        s11 = _mm_adds_epu16(s11, _mm_mpsadbw_epu8(s1, s2, 2));
                        s12 = _mm_adds_epu16(s12, _mm_mpsadbw_epu8(s1, s2, 7));
                        s0  = _mm_loadu_si128((__m128i*)(p_ref + 16));
                        s1  = _mm_loadu_si128((__m128i*)(p_ref + 24));
                        s2  = _mm_loadu_si128((__m128i*)(p_src + 16));
                        s9  = _mm_adds_epu16(s9, _mm_mpsadbw_epu8(s0, s2, 0));
                        s10 = _mm_adds_epu16(s10, _mm_mpsadbw_epu8(s0, s2, 5));
                        s11 = _mm_adds_epu16(s11, _mm_mpsadbw_epu8(s1, s2, 2));
                        s12 = _mm_adds_epu16(s12, _mm_mpsadbw_epu8(s1, s2, 7));
                        p_src += src_stride;
                        p_ref += ref_stride;
                    }
                    s0      = _mm_adds_epu16(_mm_adds_epu16(s3, s4), _mm_adds_epu16(s5, s6));
                    s0      = _mm_adds_epu16(s0, _mm_adds_epu16(_mm_adds_epu16(s9, s10), _mm_adds_epu16(s11, s12)));
                    s0      = _mm_minpos_epu16(s0);
                    tem_sum = _mm_extract_epi16(s0, 0);
                    tem_sum &= 0x0000FFFF;
                    if (tem_sum < low_sum) {
                        if (tem_sum != 0xFFFF) { // no overflow
                            low_sum = tem_sum;
                            x_best  = (int16_t)(j + _mm_extract_epi16(s0, 1));
                            y_best  = i;
                        } else {
                            s0  = _mm_unpacklo_epi16(s3, _mm_setzero_si128());
                            s3  = _mm_unpackhi_epi16(s3, _mm_setzero_si128());
                            s1  = _mm_unpacklo_epi16(s4, _mm_setzero_si128());
                            s4  = _mm_unpackhi_epi16(s4, _mm_setzero_si128());
                            s2  = _mm_unpacklo_epi16(s5, _mm_setzero_si128());
                            s5  = _mm_unpackhi_epi16(s5, _mm_setzero_si128());
                            s7  = _mm_unpacklo_epi16(s6, _mm_setzero_si128());
                            s6  = _mm_unpackhi_epi16(s6, _mm_setzero_si128());
                            s0  = _mm_add_epi32(_mm_add_epi32(s0, s1), _mm_add_epi32(s2, s7));
                            s3  = _mm_add_epi32(_mm_add_epi32(s3, s4), _mm_add_epi32(s5, s6));
                            s1  = _mm_unpacklo_epi16(s9, _mm_setzero_si128());
                            s9  = _mm_unpackhi_epi16(s9, _mm_setzero_si128());
                            s2  = _mm_unpacklo_epi16(s10, _mm_setzero_si128());
                            s10 = _mm_unpackhi_epi16(s10, _mm_setzero_si128());
                            s4  = _mm_unpacklo_epi16(s11, _mm_setzero_si128());
                            s11 = _mm_unpackhi_epi16(s11, _mm_setzero_si128());
                            s5  = _mm_unpacklo_epi16(s12, _mm_setzero_si128());
                            s12 = _mm_unpackhi_epi16(s12, _mm_setzero_si128());
                            s0  = _mm_add_epi32(s0, _mm_add_epi32(_mm_add_epi32(s1, s2), _mm_add_epi32(s4, s5)));
                            s3  = _mm_add_epi32(s3, _mm_add_epi32(_mm_add_epi32(s9, s10), _mm_add_epi32(s11, s12)));
                            UPDATE_BEST(s0, 0, 0);
                            UPDATE_BEST(s0, 1, 0);
                            UPDATE_BEST(s0, 2, 0);
                            UPDATE_BEST(s0, 3, 0);
                            UPDATE_BEST(s3, 0, 4);
                            UPDATE_BEST(s3, 1, 4);
                            UPDATE_BEST(s3, 2, 4);
                            UPDATE_BEST(s3, 3, 4);
                        }
                    }
                }

                if (leftover) {
                    p_src = src;
                    p_ref = ref + j;
                    s3 = s4 = s5 = s6 = _mm_setzero_si128();
                    for (k = 0; k < (block_height >> 1); k++) {
                        s0 = _mm_loadu_si128((__m128i*)p_ref);
                        s1 = _mm_loadu_si128((__m128i*)(p_ref + 8));
                        s2 = _mm_loadu_si128((__m128i*)p_src);
                        s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                        s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                        s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                        s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                        s0 = _mm_loadu_si128((__m128i*)(p_ref + 16));
                        s1 = _mm_loadu_si128((__m128i*)(p_ref + 24));
                        s2 = _mm_loadu_si128((__m128i*)(p_src + 16));
                        s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                        s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                        s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                        s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                        p_src += src_stride;
                        p_ref += ref_stride;
                    }
                    s9 = s10 = s11 = s12 = _mm_setzero_si128();
                    for (; k < block_height; k++) {
                        s0  = _mm_loadu_si128((__m128i*)p_ref);
                        s1  = _mm_loadu_si128((__m128i*)(p_ref + 8));
                        s2  = _mm_loadu_si128((__m128i*)p_src);
                        s9  = _mm_adds_epu16(s9, _mm_mpsadbw_epu8(s0, s2, 0));
                        s10 = _mm_adds_epu16(s10, _mm_mpsadbw_epu8(s0, s2, 5));
                        s11 = _mm_adds_epu16(s11, _mm_mpsadbw_epu8(s1, s2, 2));
                        s12 = _mm_adds_epu16(s12, _mm_mpsadbw_epu8(s1, s2, 7));
                        s0  = _mm_loadu_si128((__m128i*)(p_ref + 16));
                        s1  = _mm_loadu_si128((__m128i*)(p_ref + 24));
                        s2  = _mm_loadu_si128((__m128i*)(p_src + 16));
                        s9  = _mm_adds_epu16(s9, _mm_mpsadbw_epu8(s0, s2, 0));
                        s10 = _mm_adds_epu16(s10, _mm_mpsadbw_epu8(s0, s2, 5));
                        s11 = _mm_adds_epu16(s11, _mm_mpsadbw_epu8(s1, s2, 2));
                        s12 = _mm_adds_epu16(s12, _mm_mpsadbw_epu8(s1, s2, 7));
                        p_src += src_stride;
                        p_ref += ref_stride;
                    }
                    s0      = _mm_adds_epu16(_mm_adds_epu16(s3, s4), _mm_adds_epu16(s5, s6));
                    s0      = _mm_adds_epu16(s0, _mm_adds_epu16(_mm_adds_epu16(s9, s10), _mm_adds_epu16(s11, s12)));
                    s0      = _mm_or_si128(s0, s8);
                    s0      = _mm_minpos_epu16(s0);
                    tem_sum = _mm_extract_epi16(s0, 0);
                    tem_sum &= 0x0000FFFF;
                    if (tem_sum < low_sum) {
                        if (tem_sum != 0xFFFF) { // no overflow
                            low_sum = tem_sum;
                            x_best  = (int16_t)(j + _mm_extract_epi16(s0, 1));
                            y_best  = i;
                        } else {
                            s0  = _mm_unpacklo_epi16(s3, _mm_setzero_si128());
                            s3  = _mm_unpackhi_epi16(s3, _mm_setzero_si128());
                            s1  = _mm_unpacklo_epi16(s4, _mm_setzero_si128());
                            s4  = _mm_unpackhi_epi16(s4, _mm_setzero_si128());
                            s2  = _mm_unpacklo_epi16(s5, _mm_setzero_si128());
                            s5  = _mm_unpackhi_epi16(s5, _mm_setzero_si128());
                            s7  = _mm_unpacklo_epi16(s6, _mm_setzero_si128());
                            s6  = _mm_unpackhi_epi16(s6, _mm_setzero_si128());
                            s0  = _mm_add_epi32(_mm_add_epi32(s0, s1), _mm_add_epi32(s2, s7));
                            s3  = _mm_add_epi32(_mm_add_epi32(s3, s4), _mm_add_epi32(s5, s6));
                            s1  = _mm_unpacklo_epi16(s9, _mm_setzero_si128());
                            s9  = _mm_unpackhi_epi16(s9, _mm_setzero_si128());
                            s2  = _mm_unpacklo_epi16(s10, _mm_setzero_si128());
                            s10 = _mm_unpackhi_epi16(s10, _mm_setzero_si128());
                            s4  = _mm_unpacklo_epi16(s11, _mm_setzero_si128());
                            s11 = _mm_unpackhi_epi16(s11, _mm_setzero_si128());
                            s5  = _mm_unpacklo_epi16(s12, _mm_setzero_si128());
                            s12 = _mm_unpackhi_epi16(s12, _mm_setzero_si128());
                            s0  = _mm_add_epi32(s0, _mm_add_epi32(_mm_add_epi32(s1, s2), _mm_add_epi32(s4, s5)));
                            s3  = _mm_add_epi32(s3, _mm_add_epi32(_mm_add_epi32(s9, s10), _mm_add_epi32(s11, s12)));
                            k   = leftover;
                            while (k > 0) {
                                for (l = 0; l < 4 && k; l++, k--) {
                                    tem_sum = _mm_extract_epi32(s0, 0);
                                    s0      = _mm_srli_si128(s0, 4);
                                    if (tem_sum < low_sum) {
                                        low_sum = tem_sum;
                                        x_best  = (int16_t)(j + leftover - k);
                                        y_best  = i;
                                    }
                                }
                                s0 = s3;
                            }
                        }
                    }
                }
                ref += src_stride_raw;
            }
        }
        break;

    case 48:
        if (block_height <= 32) {
            __m128i s9, s10, s11, s12;
            for (i = 0; i < search_area_height; i++) {
                for (j = 0; j <= search_area_width - 8; j += 8) {
                    p_src = src;
                    p_ref = ref + j;
                    s3 = s4 = s5 = s6 = _mm_setzero_si128();
                    for (k = 0; k < (block_height >> 1); k++) {
                        s0 = _mm_loadu_si128((__m128i*)p_ref);
                        s1 = _mm_loadu_si128((__m128i*)(p_ref + 8));
                        s2 = _mm_loadu_si128((__m128i*)p_src);
                        s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                        s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                        s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                        s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                        s0 = _mm_loadu_si128((__m128i*)(p_ref + 16));
                        s1 = _mm_loadu_si128((__m128i*)(p_ref + 24));
                        s2 = _mm_loadu_si128((__m128i*)(p_src + 16));
                        s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                        s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                        s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                        s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                        s0 = _mm_loadu_si128((__m128i*)(p_ref + 32));
                        s1 = _mm_loadu_si128((__m128i*)(p_ref + 40));
                        s2 = _mm_loadu_si128((__m128i*)(p_src + 32));
                        s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                        s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                        s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                        s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                        p_src += src_stride;
                        p_ref += ref_stride;
                    }
                    s9 = s10 = s11 = s12 = _mm_setzero_si128();
                    for (; k < block_height; k++) {
                        s0  = _mm_loadu_si128((__m128i*)p_ref);
                        s1  = _mm_loadu_si128((__m128i*)(p_ref + 8));
                        s2  = _mm_loadu_si128((__m128i*)p_src);
                        s9  = _mm_adds_epu16(s9, _mm_mpsadbw_epu8(s0, s2, 0));
                        s10 = _mm_adds_epu16(s10, _mm_mpsadbw_epu8(s0, s2, 5));
                        s11 = _mm_adds_epu16(s11, _mm_mpsadbw_epu8(s1, s2, 2));
                        s12 = _mm_adds_epu16(s12, _mm_mpsadbw_epu8(s1, s2, 7));
                        s0  = _mm_loadu_si128((__m128i*)(p_ref + 16));
                        s1  = _mm_loadu_si128((__m128i*)(p_ref + 24));
                        s2  = _mm_loadu_si128((__m128i*)(p_src + 16));
                        s9  = _mm_adds_epu16(s9, _mm_mpsadbw_epu8(s0, s2, 0));
                        s10 = _mm_adds_epu16(s10, _mm_mpsadbw_epu8(s0, s2, 5));
                        s11 = _mm_adds_epu16(s11, _mm_mpsadbw_epu8(s1, s2, 2));
                        s12 = _mm_adds_epu16(s12, _mm_mpsadbw_epu8(s1, s2, 7));
                        s0  = _mm_loadu_si128((__m128i*)(p_ref + 32));
                        s1  = _mm_loadu_si128((__m128i*)(p_ref + 40));
                        s2  = _mm_loadu_si128((__m128i*)(p_src + 32));
                        s9  = _mm_adds_epu16(s9, _mm_mpsadbw_epu8(s0, s2, 0));
                        s10 = _mm_adds_epu16(s10, _mm_mpsadbw_epu8(s0, s2, 5));
                        s11 = _mm_adds_epu16(s11, _mm_mpsadbw_epu8(s1, s2, 2));
                        s12 = _mm_adds_epu16(s12, _mm_mpsadbw_epu8(s1, s2, 7));
                        p_src += src_stride;
                        p_ref += ref_stride;
                    }
                    s0      = _mm_adds_epu16(_mm_adds_epu16(s3, s4), _mm_adds_epu16(s5, s6));
                    s0      = _mm_adds_epu16(s0, _mm_adds_epu16(_mm_adds_epu16(s9, s10), _mm_adds_epu16(s11, s12)));
                    s0      = _mm_minpos_epu16(s0);
                    tem_sum = _mm_extract_epi16(s0, 0);
                    tem_sum &= 0x0000FFFF;
                    if (tem_sum < low_sum) {
                        if (tem_sum != 0xFFFF) { // no overflow
                            low_sum = tem_sum;
                            x_best  = (int16_t)(j + _mm_extract_epi16(s0, 1));
                            y_best  = i;
                        } else {
                            s0  = _mm_unpacklo_epi16(s3, _mm_setzero_si128());
                            s3  = _mm_unpackhi_epi16(s3, _mm_setzero_si128());
                            s1  = _mm_unpacklo_epi16(s4, _mm_setzero_si128());
                            s4  = _mm_unpackhi_epi16(s4, _mm_setzero_si128());
                            s2  = _mm_unpacklo_epi16(s5, _mm_setzero_si128());
                            s5  = _mm_unpackhi_epi16(s5, _mm_setzero_si128());
                            s7  = _mm_unpacklo_epi16(s6, _mm_setzero_si128());
                            s6  = _mm_unpackhi_epi16(s6, _mm_setzero_si128());
                            s0  = _mm_add_epi32(_mm_add_epi32(s0, s1), _mm_add_epi32(s2, s7));
                            s3  = _mm_add_epi32(_mm_add_epi32(s3, s4), _mm_add_epi32(s5, s6));
                            s1  = _mm_unpacklo_epi16(s9, _mm_setzero_si128());
                            s9  = _mm_unpackhi_epi16(s9, _mm_setzero_si128());
                            s2  = _mm_unpacklo_epi16(s10, _mm_setzero_si128());
                            s10 = _mm_unpackhi_epi16(s10, _mm_setzero_si128());
                            s4  = _mm_unpacklo_epi16(s11, _mm_setzero_si128());
                            s11 = _mm_unpackhi_epi16(s11, _mm_setzero_si128());
                            s5  = _mm_unpacklo_epi16(s12, _mm_setzero_si128());
                            s12 = _mm_unpackhi_epi16(s12, _mm_setzero_si128());
                            s0  = _mm_add_epi32(s0, _mm_add_epi32(_mm_add_epi32(s1, s2), _mm_add_epi32(s4, s5)));
                            s3  = _mm_add_epi32(s3, _mm_add_epi32(_mm_add_epi32(s9, s10), _mm_add_epi32(s11, s12)));
                            UPDATE_BEST(s0, 0, 0);
                            UPDATE_BEST(s0, 1, 0);
                            UPDATE_BEST(s0, 2, 0);
                            UPDATE_BEST(s0, 3, 0);
                            UPDATE_BEST(s3, 0, 4);
                            UPDATE_BEST(s3, 1, 4);
                            UPDATE_BEST(s3, 2, 4);
                            UPDATE_BEST(s3, 3, 4);
                        }
                    }
                }

                if (leftover) {
                    p_src = src;
                    p_ref = ref + j;
                    s3 = s4 = s5 = s6 = _mm_setzero_si128();
                    for (k = 0; k < (block_height >> 1); k++) {
                        s0 = _mm_loadu_si128((__m128i*)p_ref);
                        s1 = _mm_loadu_si128((__m128i*)(p_ref + 8));
                        s2 = _mm_loadu_si128((__m128i*)p_src);
                        s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                        s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                        s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                        s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                        s0 = _mm_loadu_si128((__m128i*)(p_ref + 16));
                        s1 = _mm_loadu_si128((__m128i*)(p_ref + 24));
                        s2 = _mm_loadu_si128((__m128i*)(p_src + 16));
                        s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                        s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                        s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                        s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                        s0 = _mm_loadu_si128((__m128i*)(p_ref + 32));
                        s1 = _mm_loadu_si128((__m128i*)(p_ref + 40));
                        s2 = _mm_loadu_si128((__m128i*)(p_src + 32));
                        s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                        s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                        s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                        s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                        p_src += src_stride;
                        p_ref += ref_stride;
                    }
                    s9 = s10 = s11 = s12 = _mm_setzero_si128();
                    for (; k < block_height; k++) {
                        s0  = _mm_loadu_si128((__m128i*)p_ref);
                        s1  = _mm_loadu_si128((__m128i*)(p_ref + 8));
                        s2  = _mm_loadu_si128((__m128i*)p_src);
                        s9  = _mm_adds_epu16(s9, _mm_mpsadbw_epu8(s0, s2, 0));
                        s10 = _mm_adds_epu16(s10, _mm_mpsadbw_epu8(s0, s2, 5));
                        s11 = _mm_adds_epu16(s11, _mm_mpsadbw_epu8(s1, s2, 2));
                        s12 = _mm_adds_epu16(s12, _mm_mpsadbw_epu8(s1, s2, 7));
                        s0  = _mm_loadu_si128((__m128i*)(p_ref + 16));
                        s1  = _mm_loadu_si128((__m128i*)(p_ref + 24));
                        s2  = _mm_loadu_si128((__m128i*)(p_src + 16));
                        s9  = _mm_adds_epu16(s9, _mm_mpsadbw_epu8(s0, s2, 0));
                        s10 = _mm_adds_epu16(s10, _mm_mpsadbw_epu8(s0, s2, 5));
                        s11 = _mm_adds_epu16(s11, _mm_mpsadbw_epu8(s1, s2, 2));
                        s12 = _mm_adds_epu16(s12, _mm_mpsadbw_epu8(s1, s2, 7));
                        s0  = _mm_loadu_si128((__m128i*)(p_ref + 32));
                        s1  = _mm_loadu_si128((__m128i*)(p_ref + 40));
                        s2  = _mm_loadu_si128((__m128i*)(p_src + 32));
                        s9  = _mm_adds_epu16(s9, _mm_mpsadbw_epu8(s0, s2, 0));
                        s10 = _mm_adds_epu16(s10, _mm_mpsadbw_epu8(s0, s2, 5));
                        s11 = _mm_adds_epu16(s11, _mm_mpsadbw_epu8(s1, s2, 2));
                        s12 = _mm_adds_epu16(s12, _mm_mpsadbw_epu8(s1, s2, 7));
                        p_src += src_stride;
                        p_ref += ref_stride;
                    }
                    s0      = _mm_adds_epu16(_mm_adds_epu16(s3, s4), _mm_adds_epu16(s5, s6));
                    s0      = _mm_adds_epu16(s0, _mm_adds_epu16(_mm_adds_epu16(s9, s10), _mm_adds_epu16(s11, s12)));
                    s0      = _mm_or_si128(s0, s8);
                    s0      = _mm_minpos_epu16(s0);
                    tem_sum = _mm_extract_epi16(s0, 0);
                    tem_sum &= 0x0000FFFF;
                    if (tem_sum < low_sum) {
                        if (tem_sum != 0xFFFF) { // no overflow
                            low_sum = tem_sum;
                            x_best  = (int16_t)(j + _mm_extract_epi16(s0, 1));
                            y_best  = i;
                        } else {
                            s0  = _mm_unpacklo_epi16(s3, _mm_setzero_si128());
                            s3  = _mm_unpackhi_epi16(s3, _mm_setzero_si128());
                            s1  = _mm_unpacklo_epi16(s4, _mm_setzero_si128());
                            s4  = _mm_unpackhi_epi16(s4, _mm_setzero_si128());
                            s2  = _mm_unpacklo_epi16(s5, _mm_setzero_si128());
                            s5  = _mm_unpackhi_epi16(s5, _mm_setzero_si128());
                            s7  = _mm_unpacklo_epi16(s6, _mm_setzero_si128());
                            s6  = _mm_unpackhi_epi16(s6, _mm_setzero_si128());
                            s0  = _mm_add_epi32(_mm_add_epi32(s0, s1), _mm_add_epi32(s2, s7));
                            s3  = _mm_add_epi32(_mm_add_epi32(s3, s4), _mm_add_epi32(s5, s6));
                            s1  = _mm_unpacklo_epi16(s9, _mm_setzero_si128());
                            s9  = _mm_unpackhi_epi16(s9, _mm_setzero_si128());
                            s2  = _mm_unpacklo_epi16(s10, _mm_setzero_si128());
                            s10 = _mm_unpackhi_epi16(s10, _mm_setzero_si128());
                            s4  = _mm_unpacklo_epi16(s11, _mm_setzero_si128());
                            s11 = _mm_unpackhi_epi16(s11, _mm_setzero_si128());
                            s5  = _mm_unpacklo_epi16(s12, _mm_setzero_si128());
                            s12 = _mm_unpackhi_epi16(s12, _mm_setzero_si128());
                            s0  = _mm_add_epi32(s0, _mm_add_epi32(_mm_add_epi32(s1, s2), _mm_add_epi32(s4, s5)));
                            s3  = _mm_add_epi32(s3, _mm_add_epi32(_mm_add_epi32(s9, s10), _mm_add_epi32(s11, s12)));
                            k   = leftover;
                            while (k > 0) {
                                for (l = 0; l < 4 && k; l++, k--) {
                                    tem_sum = _mm_extract_epi32(s0, 0);
                                    s0      = _mm_srli_si128(s0, 4);
                                    if (tem_sum < low_sum) {
                                        low_sum = tem_sum;
                                        x_best  = (int16_t)(j + leftover - k);
                                        y_best  = i;
                                    }
                                }
                                s0 = s3;
                            }
                        }
                    }
                }
                ref += src_stride_raw;
            }
        } else {
            __m128i s9, s10;
            for (i = 0; i < search_area_height; i++) {
                for (j = 0; j <= search_area_width - 8; j += 8) {
                    p_src = src;
                    p_ref = ref + j;
                    s9 = s10 = _mm_setzero_si128();
                    k        = 0;
                    while (k < block_height) {
                        s3 = s4 = s5 = s6 = _mm_setzero_si128();
                        for (l = 0; l < 21 && k < block_height; k++, l++) {
                            s0 = _mm_loadu_si128((__m128i*)p_ref);
                            s1 = _mm_loadu_si128((__m128i*)(p_ref + 8));
                            s2 = _mm_loadu_si128((__m128i*)p_src);
                            s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                            s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                            s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                            s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                            s0 = _mm_loadu_si128((__m128i*)(p_ref + 16));
                            s1 = _mm_loadu_si128((__m128i*)(p_ref + 24));
                            s2 = _mm_loadu_si128((__m128i*)(p_src + 16));
                            s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                            s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                            s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                            s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                            s0 = _mm_loadu_si128((__m128i*)(p_ref + 32));
                            s1 = _mm_loadu_si128((__m128i*)(p_ref + 40));
                            s2 = _mm_loadu_si128((__m128i*)(p_src + 32));
                            s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                            s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                            s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                            s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                            p_src += src_stride;
                            p_ref += ref_stride;
                        }
                        s0  = _mm_unpacklo_epi16(s3, _mm_setzero_si128());
                        s3  = _mm_unpackhi_epi16(s3, _mm_setzero_si128());
                        s1  = _mm_unpacklo_epi16(s4, _mm_setzero_si128());
                        s4  = _mm_unpackhi_epi16(s4, _mm_setzero_si128());
                        s2  = _mm_unpacklo_epi16(s5, _mm_setzero_si128());
                        s5  = _mm_unpackhi_epi16(s5, _mm_setzero_si128());
                        s7  = _mm_unpacklo_epi16(s6, _mm_setzero_si128());
                        s6  = _mm_unpackhi_epi16(s6, _mm_setzero_si128());
                        s9  = _mm_add_epi32(s9, _mm_add_epi32(_mm_add_epi32(s0, s1), _mm_add_epi32(s2, s7)));
                        s10 = _mm_add_epi32(s10, _mm_add_epi32(_mm_add_epi32(s3, s4), _mm_add_epi32(s5, s6)));
                    }
                    s0      = _mm_packus_epi32(s9, s10);
                    s0      = _mm_minpos_epu16(s0);
                    tem_sum = _mm_extract_epi16(s0, 0);
                    tem_sum &= 0x0000FFFF;
                    if (tem_sum < low_sum) {
                        if (tem_sum != 0xFFFF) { // no overflow
                            low_sum = tem_sum;
                            x_best  = (int16_t)(j + _mm_extract_epi16(s0, 1));
                            y_best  = i;
                        } else {
                            UPDATE_BEST(s9, 0, 0);
                            UPDATE_BEST(s9, 1, 0);
                            UPDATE_BEST(s9, 2, 0);
                            UPDATE_BEST(s9, 3, 0);
                            UPDATE_BEST(s10, 0, 4);
                            UPDATE_BEST(s10, 1, 4);
                            UPDATE_BEST(s10, 2, 4);
                            UPDATE_BEST(s10, 3, 4);
                        }
                    }
                }

                if (leftover) {
                    p_src = src;
                    p_ref = ref + j;
                    s9 = s10 = _mm_setzero_si128();
                    k        = 0;
                    while (k < block_height) {
                        s3 = s4 = s5 = s6 = _mm_setzero_si128();
                        for (l = 0; l < 21 && k < block_height; k++, l++) {
                            s0 = _mm_loadu_si128((__m128i*)p_ref);
                            s1 = _mm_loadu_si128((__m128i*)(p_ref + 8));
                            s2 = _mm_loadu_si128((__m128i*)p_src);
                            s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                            s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                            s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                            s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                            s0 = _mm_loadu_si128((__m128i*)(p_ref + 16));
                            s1 = _mm_loadu_si128((__m128i*)(p_ref + 24));
                            s2 = _mm_loadu_si128((__m128i*)(p_src + 16));
                            s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                            s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                            s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                            s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                            s0 = _mm_loadu_si128((__m128i*)(p_ref + 32));
                            s1 = _mm_loadu_si128((__m128i*)(p_ref + 40));
                            s2 = _mm_loadu_si128((__m128i*)(p_src + 32));
                            s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                            s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                            s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                            s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                            p_src += src_stride;
                            p_ref += ref_stride;
                        }
                        s0  = _mm_unpacklo_epi16(s3, _mm_setzero_si128());
                        s3  = _mm_unpackhi_epi16(s3, _mm_setzero_si128());
                        s1  = _mm_unpacklo_epi16(s4, _mm_setzero_si128());
                        s4  = _mm_unpackhi_epi16(s4, _mm_setzero_si128());
                        s2  = _mm_unpacklo_epi16(s5, _mm_setzero_si128());
                        s5  = _mm_unpackhi_epi16(s5, _mm_setzero_si128());
                        s7  = _mm_unpacklo_epi16(s6, _mm_setzero_si128());
                        s6  = _mm_unpackhi_epi16(s6, _mm_setzero_si128());
                        s9  = _mm_add_epi32(s9, _mm_add_epi32(_mm_add_epi32(s0, s1), _mm_add_epi32(s2, s7)));
                        s10 = _mm_add_epi32(s10, _mm_add_epi32(_mm_add_epi32(s3, s4), _mm_add_epi32(s5, s6)));
                    }
                    s0      = _mm_packus_epi32(s9, s10);
                    s0      = _mm_or_si128(s0, s8);
                    s0      = _mm_minpos_epu16(s0);
                    tem_sum = _mm_extract_epi16(s0, 0);
                    tem_sum &= 0x0000FFFF;
                    if (tem_sum < low_sum) {
                        if (tem_sum != 0xFFFF) { // no overflow
                            low_sum = tem_sum;
                            x_best  = (int16_t)(j + _mm_extract_epi16(s0, 1));
                            y_best  = i;
                        } else {
                            k = leftover;
                            while (k > 0) {
                                for (l = 0; l < 4 && k; l++, k--) {
                                    tem_sum = _mm_extract_epi32(s9, 0);
                                    s9      = _mm_srli_si128(s9, 4);
                                    if (tem_sum < low_sum) {
                                        low_sum = tem_sum;
                                        x_best  = (int16_t)(j + leftover - k);
                                        y_best  = i;
                                    }
                                }
                                s9 = s10;
                            }
                        }
                    }
                }
                ref += src_stride_raw;
            }
        }
        break;

    case 64:
        if (block_height <= 32) {
            __m128i s9, s10, s11, s12;
            for (i = 0; i < search_area_height; i++) {
                for (j = 0; j <= search_area_width - 8; j += 8) {
                    p_src = src;
                    p_ref = ref + j;
                    s3 = s4 = s5 = s6 = _mm_setzero_si128();
                    for (k = 0; k < (block_height >> 1); k++) {
                        s0 = _mm_loadu_si128((__m128i*)p_ref);
                        s1 = _mm_loadu_si128((__m128i*)(p_ref + 8));
                        s2 = _mm_loadu_si128((__m128i*)p_src);
                        s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                        s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                        s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                        s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                        s0 = _mm_loadu_si128((__m128i*)(p_ref + 16));
                        s1 = _mm_loadu_si128((__m128i*)(p_ref + 24));
                        s2 = _mm_loadu_si128((__m128i*)(p_src + 16));
                        s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                        s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                        s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                        s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                        s0 = _mm_loadu_si128((__m128i*)(p_ref + 32));
                        s1 = _mm_loadu_si128((__m128i*)(p_ref + 40));
                        s2 = _mm_loadu_si128((__m128i*)(p_src + 32));
                        s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                        s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                        s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                        s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                        s0 = _mm_loadu_si128((__m128i*)(p_ref + 48));
                        s1 = _mm_loadu_si128((__m128i*)(p_ref + 56));
                        s2 = _mm_loadu_si128((__m128i*)(p_src + 48));
                        s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                        s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                        s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                        s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                        p_src += src_stride;
                        p_ref += ref_stride;
                    }
                    s9 = s10 = s11 = s12 = _mm_setzero_si128();
                    for (; k < block_height; k++) {
                        s0  = _mm_loadu_si128((__m128i*)p_ref);
                        s1  = _mm_loadu_si128((__m128i*)(p_ref + 8));
                        s2  = _mm_loadu_si128((__m128i*)p_src);
                        s9  = _mm_adds_epu16(s9, _mm_mpsadbw_epu8(s0, s2, 0));
                        s10 = _mm_adds_epu16(s10, _mm_mpsadbw_epu8(s0, s2, 5));
                        s11 = _mm_adds_epu16(s11, _mm_mpsadbw_epu8(s1, s2, 2));
                        s12 = _mm_adds_epu16(s12, _mm_mpsadbw_epu8(s1, s2, 7));
                        s0  = _mm_loadu_si128((__m128i*)(p_ref + 16));
                        s1  = _mm_loadu_si128((__m128i*)(p_ref + 24));
                        s2  = _mm_loadu_si128((__m128i*)(p_src + 16));
                        s9  = _mm_adds_epu16(s9, _mm_mpsadbw_epu8(s0, s2, 0));
                        s10 = _mm_adds_epu16(s10, _mm_mpsadbw_epu8(s0, s2, 5));
                        s11 = _mm_adds_epu16(s11, _mm_mpsadbw_epu8(s1, s2, 2));
                        s12 = _mm_adds_epu16(s12, _mm_mpsadbw_epu8(s1, s2, 7));
                        s0  = _mm_loadu_si128((__m128i*)(p_ref + 32));
                        s1  = _mm_loadu_si128((__m128i*)(p_ref + 40));
                        s2  = _mm_loadu_si128((__m128i*)(p_src + 32));
                        s9  = _mm_adds_epu16(s9, _mm_mpsadbw_epu8(s0, s2, 0));
                        s10 = _mm_adds_epu16(s10, _mm_mpsadbw_epu8(s0, s2, 5));
                        s11 = _mm_adds_epu16(s11, _mm_mpsadbw_epu8(s1, s2, 2));
                        s12 = _mm_adds_epu16(s12, _mm_mpsadbw_epu8(s1, s2, 7));
                        s0  = _mm_loadu_si128((__m128i*)(p_ref + 48));
                        s1  = _mm_loadu_si128((__m128i*)(p_ref + 56));
                        s2  = _mm_loadu_si128((__m128i*)(p_src + 48));
                        s9  = _mm_adds_epu16(s9, _mm_mpsadbw_epu8(s0, s2, 0));
                        s10 = _mm_adds_epu16(s10, _mm_mpsadbw_epu8(s0, s2, 5));
                        s11 = _mm_adds_epu16(s11, _mm_mpsadbw_epu8(s1, s2, 2));
                        s12 = _mm_adds_epu16(s12, _mm_mpsadbw_epu8(s1, s2, 7));
                        p_src += src_stride;
                        p_ref += ref_stride;
                    }
                    s0      = _mm_adds_epu16(_mm_adds_epu16(s3, s4), _mm_adds_epu16(s5, s6));
                    s0      = _mm_adds_epu16(s0, _mm_adds_epu16(_mm_adds_epu16(s9, s10), _mm_adds_epu16(s11, s12)));
                    s0      = _mm_minpos_epu16(s0);
                    tem_sum = _mm_extract_epi16(s0, 0);
                    tem_sum &= 0x0000FFFF;
                    if (tem_sum < low_sum) {
                        if (tem_sum != 0xFFFF) { // no overflow
                            low_sum = tem_sum;
                            x_best  = (int16_t)(j + _mm_extract_epi16(s0, 1));
                            y_best  = i;
                        } else {
                            s0  = _mm_unpacklo_epi16(s3, _mm_setzero_si128());
                            s3  = _mm_unpackhi_epi16(s3, _mm_setzero_si128());
                            s1  = _mm_unpacklo_epi16(s4, _mm_setzero_si128());
                            s4  = _mm_unpackhi_epi16(s4, _mm_setzero_si128());
                            s2  = _mm_unpacklo_epi16(s5, _mm_setzero_si128());
                            s5  = _mm_unpackhi_epi16(s5, _mm_setzero_si128());
                            s7  = _mm_unpacklo_epi16(s6, _mm_setzero_si128());
                            s6  = _mm_unpackhi_epi16(s6, _mm_setzero_si128());
                            s0  = _mm_add_epi32(_mm_add_epi32(s0, s1), _mm_add_epi32(s2, s7));
                            s3  = _mm_add_epi32(_mm_add_epi32(s3, s4), _mm_add_epi32(s5, s6));
                            s1  = _mm_unpacklo_epi16(s9, _mm_setzero_si128());
                            s9  = _mm_unpackhi_epi16(s9, _mm_setzero_si128());
                            s2  = _mm_unpacklo_epi16(s10, _mm_setzero_si128());
                            s10 = _mm_unpackhi_epi16(s10, _mm_setzero_si128());
                            s4  = _mm_unpacklo_epi16(s11, _mm_setzero_si128());
                            s11 = _mm_unpackhi_epi16(s11, _mm_setzero_si128());
                            s5  = _mm_unpacklo_epi16(s12, _mm_setzero_si128());
                            s12 = _mm_unpackhi_epi16(s12, _mm_setzero_si128());
                            s0  = _mm_add_epi32(s0, _mm_add_epi32(_mm_add_epi32(s1, s2), _mm_add_epi32(s4, s5)));
                            s3  = _mm_add_epi32(s3, _mm_add_epi32(_mm_add_epi32(s9, s10), _mm_add_epi32(s11, s12)));
                            UPDATE_BEST(s0, 0, 0);
                            UPDATE_BEST(s0, 1, 0);
                            UPDATE_BEST(s0, 2, 0);
                            UPDATE_BEST(s0, 3, 0);
                            UPDATE_BEST(s3, 0, 4);
                            UPDATE_BEST(s3, 1, 4);
                            UPDATE_BEST(s3, 2, 4);
                            UPDATE_BEST(s3, 3, 4);
                        }
                    }
                }

                if (leftover) {
                    p_src = src;
                    p_ref = ref + j;
                    s3 = s4 = s5 = s6 = _mm_setzero_si128();
                    for (k = 0; k < (block_height >> 1); k++) {
                        s0 = _mm_loadu_si128((__m128i*)p_ref);
                        s1 = _mm_loadu_si128((__m128i*)(p_ref + 8));
                        s2 = _mm_loadu_si128((__m128i*)p_src);
                        s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                        s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                        s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                        s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                        s0 = _mm_loadu_si128((__m128i*)(p_ref + 16));
                        s1 = _mm_loadu_si128((__m128i*)(p_ref + 24));
                        s2 = _mm_loadu_si128((__m128i*)(p_src + 16));
                        s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                        s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                        s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                        s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                        s0 = _mm_loadu_si128((__m128i*)(p_ref + 32));
                        s1 = _mm_loadu_si128((__m128i*)(p_ref + 40));
                        s2 = _mm_loadu_si128((__m128i*)(p_src + 32));
                        s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                        s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                        s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                        s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                        s0 = _mm_loadu_si128((__m128i*)(p_ref + 48));
                        s1 = _mm_loadu_si128((__m128i*)(p_ref + 56));
                        s2 = _mm_loadu_si128((__m128i*)(p_src + 48));
                        s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                        s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                        s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                        s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                        p_src += src_stride;
                        p_ref += ref_stride;
                    }
                    s9 = s10 = s11 = s12 = _mm_setzero_si128();
                    for (; k < block_height; k++) {
                        s0  = _mm_loadu_si128((__m128i*)p_ref);
                        s1  = _mm_loadu_si128((__m128i*)(p_ref + 8));
                        s2  = _mm_loadu_si128((__m128i*)p_src);
                        s9  = _mm_adds_epu16(s9, _mm_mpsadbw_epu8(s0, s2, 0));
                        s10 = _mm_adds_epu16(s10, _mm_mpsadbw_epu8(s0, s2, 5));
                        s11 = _mm_adds_epu16(s11, _mm_mpsadbw_epu8(s1, s2, 2));
                        s12 = _mm_adds_epu16(s12, _mm_mpsadbw_epu8(s1, s2, 7));
                        s0  = _mm_loadu_si128((__m128i*)(p_ref + 16));
                        s1  = _mm_loadu_si128((__m128i*)(p_ref + 24));
                        s2  = _mm_loadu_si128((__m128i*)(p_src + 16));
                        s9  = _mm_adds_epu16(s9, _mm_mpsadbw_epu8(s0, s2, 0));
                        s10 = _mm_adds_epu16(s10, _mm_mpsadbw_epu8(s0, s2, 5));
                        s11 = _mm_adds_epu16(s11, _mm_mpsadbw_epu8(s1, s2, 2));
                        s12 = _mm_adds_epu16(s12, _mm_mpsadbw_epu8(s1, s2, 7));
                        s0  = _mm_loadu_si128((__m128i*)(p_ref + 32));
                        s1  = _mm_loadu_si128((__m128i*)(p_ref + 40));
                        s2  = _mm_loadu_si128((__m128i*)(p_src + 32));
                        s9  = _mm_adds_epu16(s9, _mm_mpsadbw_epu8(s0, s2, 0));
                        s10 = _mm_adds_epu16(s10, _mm_mpsadbw_epu8(s0, s2, 5));
                        s11 = _mm_adds_epu16(s11, _mm_mpsadbw_epu8(s1, s2, 2));
                        s12 = _mm_adds_epu16(s12, _mm_mpsadbw_epu8(s1, s2, 7));
                        s0  = _mm_loadu_si128((__m128i*)(p_ref + 48));
                        s1  = _mm_loadu_si128((__m128i*)(p_ref + 56));
                        s2  = _mm_loadu_si128((__m128i*)(p_src + 48));
                        s9  = _mm_adds_epu16(s9, _mm_mpsadbw_epu8(s0, s2, 0));
                        s10 = _mm_adds_epu16(s10, _mm_mpsadbw_epu8(s0, s2, 5));
                        s11 = _mm_adds_epu16(s11, _mm_mpsadbw_epu8(s1, s2, 2));
                        s12 = _mm_adds_epu16(s12, _mm_mpsadbw_epu8(s1, s2, 7));
                        p_src += src_stride;
                        p_ref += ref_stride;
                    }
                    s0      = _mm_adds_epu16(_mm_adds_epu16(s3, s4), _mm_adds_epu16(s5, s6));
                    s0      = _mm_adds_epu16(s0, _mm_adds_epu16(_mm_adds_epu16(s9, s10), _mm_adds_epu16(s11, s12)));
                    s0      = _mm_or_si128(s0, s8);
                    s0      = _mm_minpos_epu16(s0);
                    tem_sum = _mm_extract_epi16(s0, 0);
                    tem_sum &= 0x0000FFFF;
                    if (tem_sum < low_sum) {
                        if (tem_sum != 0xFFFF) { // no overflow
                            low_sum = tem_sum;
                            x_best  = (int16_t)(j + _mm_extract_epi16(s0, 1));
                            y_best  = i;
                        } else {
                            s0  = _mm_unpacklo_epi16(s3, _mm_setzero_si128());
                            s3  = _mm_unpackhi_epi16(s3, _mm_setzero_si128());
                            s1  = _mm_unpacklo_epi16(s4, _mm_setzero_si128());
                            s4  = _mm_unpackhi_epi16(s4, _mm_setzero_si128());
                            s2  = _mm_unpacklo_epi16(s5, _mm_setzero_si128());
                            s5  = _mm_unpackhi_epi16(s5, _mm_setzero_si128());
                            s7  = _mm_unpacklo_epi16(s6, _mm_setzero_si128());
                            s6  = _mm_unpackhi_epi16(s6, _mm_setzero_si128());
                            s0  = _mm_add_epi32(_mm_add_epi32(s0, s1), _mm_add_epi32(s2, s7));
                            s3  = _mm_add_epi32(_mm_add_epi32(s3, s4), _mm_add_epi32(s5, s6));
                            s1  = _mm_unpacklo_epi16(s9, _mm_setzero_si128());
                            s9  = _mm_unpackhi_epi16(s9, _mm_setzero_si128());
                            s2  = _mm_unpacklo_epi16(s10, _mm_setzero_si128());
                            s10 = _mm_unpackhi_epi16(s10, _mm_setzero_si128());
                            s4  = _mm_unpacklo_epi16(s11, _mm_setzero_si128());
                            s11 = _mm_unpackhi_epi16(s11, _mm_setzero_si128());
                            s5  = _mm_unpacklo_epi16(s12, _mm_setzero_si128());
                            s12 = _mm_unpackhi_epi16(s12, _mm_setzero_si128());
                            s0  = _mm_add_epi32(s0, _mm_add_epi32(_mm_add_epi32(s1, s2), _mm_add_epi32(s4, s5)));
                            s3  = _mm_add_epi32(s3, _mm_add_epi32(_mm_add_epi32(s9, s10), _mm_add_epi32(s11, s12)));
                            k   = leftover;
                            while (k > 0) {
                                for (l = 0; l < 4 && k; l++, k--) {
                                    tem_sum = _mm_extract_epi32(s0, 0);
                                    s0      = _mm_srli_si128(s0, 4);
                                    if (tem_sum < low_sum) {
                                        low_sum = tem_sum;
                                        x_best  = (int16_t)(j + leftover - k);
                                        y_best  = i;
                                    }
                                }
                                s0 = s3;
                            }
                        }
                    }
                }
                ref += src_stride_raw;
            }
        } else {
            __m128i s9, s10;
            for (i = 0; i < search_area_height; i++) {
                for (j = 0; j <= search_area_width - 8; j += 8) {
                    p_src = src;
                    p_ref = ref + j;
                    s9 = s10 = _mm_setzero_si128();
                    k        = 0;
                    while (k < block_height) {
                        s3 = s4 = s5 = s6 = _mm_setzero_si128();
                        for (l = 0; l < 16 && k < block_height; k++, l++) {
                            s0 = _mm_loadu_si128((__m128i*)p_ref);
                            s1 = _mm_loadu_si128((__m128i*)(p_ref + 8));
                            s2 = _mm_loadu_si128((__m128i*)p_src);
                            s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                            s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                            s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                            s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                            s0 = _mm_loadu_si128((__m128i*)(p_ref + 16));
                            s1 = _mm_loadu_si128((__m128i*)(p_ref + 24));
                            s2 = _mm_loadu_si128((__m128i*)(p_src + 16));
                            s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                            s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                            s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                            s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                            s0 = _mm_loadu_si128((__m128i*)(p_ref + 32));
                            s1 = _mm_loadu_si128((__m128i*)(p_ref + 40));
                            s2 = _mm_loadu_si128((__m128i*)(p_src + 32));
                            s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                            s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                            s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                            s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                            s0 = _mm_loadu_si128((__m128i*)(p_ref + 48));
                            s1 = _mm_loadu_si128((__m128i*)(p_ref + 56));
                            s2 = _mm_loadu_si128((__m128i*)(p_src + 48));
                            s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                            s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                            s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                            s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                            p_src += src_stride;
                            p_ref += ref_stride;
                        }
                        s0  = _mm_unpacklo_epi16(s3, _mm_setzero_si128());
                        s3  = _mm_unpackhi_epi16(s3, _mm_setzero_si128());
                        s1  = _mm_unpacklo_epi16(s4, _mm_setzero_si128());
                        s4  = _mm_unpackhi_epi16(s4, _mm_setzero_si128());
                        s2  = _mm_unpacklo_epi16(s5, _mm_setzero_si128());
                        s5  = _mm_unpackhi_epi16(s5, _mm_setzero_si128());
                        s7  = _mm_unpacklo_epi16(s6, _mm_setzero_si128());
                        s6  = _mm_unpackhi_epi16(s6, _mm_setzero_si128());
                        s9  = _mm_add_epi32(s9, _mm_add_epi32(_mm_add_epi32(s0, s1), _mm_add_epi32(s2, s7)));
                        s10 = _mm_add_epi32(s10, _mm_add_epi32(_mm_add_epi32(s3, s4), _mm_add_epi32(s5, s6)));
                    }
                    s0      = _mm_packus_epi32(s9, s10);
                    s0      = _mm_minpos_epu16(s0);
                    tem_sum = _mm_extract_epi16(s0, 0);
                    tem_sum &= 0x0000FFFF;
                    if (tem_sum < low_sum) {
                        if (tem_sum != 0xFFFF) { // no overflow
                            low_sum = tem_sum;
                            x_best  = (int16_t)(j + _mm_extract_epi16(s0, 1));
                            y_best  = i;
                        } else {
                            UPDATE_BEST(s9, 0, 0);
                            UPDATE_BEST(s9, 1, 0);
                            UPDATE_BEST(s9, 2, 0);
                            UPDATE_BEST(s9, 3, 0);
                            UPDATE_BEST(s10, 0, 4);
                            UPDATE_BEST(s10, 1, 4);
                            UPDATE_BEST(s10, 2, 4);
                            UPDATE_BEST(s10, 3, 4);
                        }
                    }
                }

                if (leftover) {
                    p_src = src;
                    p_ref = ref + j;
                    s9 = s10 = _mm_setzero_si128();
                    k        = 0;
                    while (k < block_height) {
                        s3 = s4 = s5 = s6 = _mm_setzero_si128();
                        for (l = 0; l < 16 && k < block_height; k++, l++) {
                            s0 = _mm_loadu_si128((__m128i*)p_ref);
                            s1 = _mm_loadu_si128((__m128i*)(p_ref + 8));
                            s2 = _mm_loadu_si128((__m128i*)p_src);
                            s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                            s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                            s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                            s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                            s0 = _mm_loadu_si128((__m128i*)(p_ref + 16));
                            s1 = _mm_loadu_si128((__m128i*)(p_ref + 24));
                            s2 = _mm_loadu_si128((__m128i*)(p_src + 16));
                            s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                            s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                            s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                            s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                            s0 = _mm_loadu_si128((__m128i*)(p_ref + 32));
                            s1 = _mm_loadu_si128((__m128i*)(p_ref + 40));
                            s2 = _mm_loadu_si128((__m128i*)(p_src + 32));
                            s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                            s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                            s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                            s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                            s0 = _mm_loadu_si128((__m128i*)(p_ref + 48));
                            s1 = _mm_loadu_si128((__m128i*)(p_ref + 56));
                            s2 = _mm_loadu_si128((__m128i*)(p_src + 48));
                            s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                            s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                            s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                            s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                            p_src += src_stride;
                            p_ref += ref_stride;
                        }
                        s0  = _mm_unpacklo_epi16(s3, _mm_setzero_si128());
                        s3  = _mm_unpackhi_epi16(s3, _mm_setzero_si128());
                        s1  = _mm_unpacklo_epi16(s4, _mm_setzero_si128());
                        s4  = _mm_unpackhi_epi16(s4, _mm_setzero_si128());
                        s2  = _mm_unpacklo_epi16(s5, _mm_setzero_si128());
                        s5  = _mm_unpackhi_epi16(s5, _mm_setzero_si128());
                        s7  = _mm_unpacklo_epi16(s6, _mm_setzero_si128());
                        s6  = _mm_unpackhi_epi16(s6, _mm_setzero_si128());
                        s9  = _mm_add_epi32(s9, _mm_add_epi32(_mm_add_epi32(s0, s1), _mm_add_epi32(s2, s7)));
                        s10 = _mm_add_epi32(s10, _mm_add_epi32(_mm_add_epi32(s3, s4), _mm_add_epi32(s5, s6)));
                    }
                    s0      = _mm_packus_epi32(s9, s10);
                    s0      = _mm_or_si128(s0, s8);
                    s0      = _mm_minpos_epu16(s0);
                    tem_sum = _mm_extract_epi16(s0, 0);
                    tem_sum &= 0x0000FFFF;
                    if (tem_sum < low_sum) {
                        if (tem_sum != 0xFFFF) { // no overflow
                            low_sum = tem_sum;
                            x_best  = (int16_t)(j + _mm_extract_epi16(s0, 1));
                            y_best  = i;
                        } else {
                            k = leftover;
                            while (k > 0) {
                                for (l = 0; l < 4 && k; l++, k--) {
                                    tem_sum = _mm_extract_epi32(s9, 0);
                                    s9      = _mm_srli_si128(s9, 4);
                                    if (tem_sum < low_sum) {
                                        low_sum = tem_sum;
                                        x_best  = (int16_t)(j + leftover - k);
                                        y_best  = i;
                                    }
                                }
                                s9 = s10;
                            }
                        }
                    }
                }
                ref += src_stride_raw;
            }
        }
        break;

    default:
        svt_sad_loop_kernel_anyxh_sse4_1(src,
                                         src_stride,
                                         ref,
                                         ref_stride,
                                         block_height,
                                         block_width,
                                         best_sad,
                                         x_search_center,
                                         y_search_center,
                                         src_stride_raw,
                                         skip_search_line,
                                         search_area_width,
                                         search_area_height);
        return;
    }

    *best_sad        = low_sum;
    *x_search_center = x_best;
    *y_search_center = y_best;
}

void svt_ext_sad_calculation_8x8_16x16_sse4_1_intrin(uint8_t* src, uint32_t src_stride, uint8_t* ref,
                                                     uint32_t ref_stride, uint32_t* p_best_sad_8x8,
                                                     uint32_t* p_best_sad_16x16, uint32_t* p_best_mv8x8,
                                                     uint32_t* p_best_mv16x16, uint32_t mv, uint32_t* p_sad16x16,
                                                     uint32_t* p_sad8x8, bool sub_sad) {
    __m128i xmm_sad16x16, xmm_sad16x16_total, sad8x8_0_3;
    __m128i sad8x8_less_than_bitmask, best_mv8x8;
    __m128i best_sad8x8, xmm_best_sad8x8, xmm_best_mv8x8;
    __m128i sad8x8_0_1, sad8x8_2_3, src_128_1, src_128_2, ref_128_1, ref_128_2;
    __m128i xmm_mv = _mm_set1_epi32(mv);

    src_128_1  = _mm_loadu_si128((__m128i const*)(src + 0 * src_stride));
    src_128_2  = _mm_loadu_si128((__m128i const*)(src + 8 * src_stride));
    ref_128_1  = _mm_loadu_si128((__m128i const*)(ref + 0 * ref_stride));
    ref_128_2  = _mm_loadu_si128((__m128i const*)(ref + 8 * ref_stride));
    sad8x8_0_1 = _mm_sad_epu8(src_128_1, ref_128_1);
    sad8x8_2_3 = _mm_sad_epu8(src_128_2, ref_128_2);

    src_128_1  = _mm_loadu_si128((__m128i const*)(src + 2 * src_stride));
    src_128_2  = _mm_loadu_si128((__m128i const*)(src + 10 * src_stride));
    ref_128_1  = _mm_loadu_si128((__m128i const*)(ref + 2 * ref_stride));
    ref_128_2  = _mm_loadu_si128((__m128i const*)(ref + 10 * ref_stride));
    sad8x8_0_1 = _mm_add_epi32(_mm_sad_epu8(src_128_1, ref_128_1), sad8x8_0_1);
    sad8x8_2_3 = _mm_add_epi32(_mm_sad_epu8(src_128_2, ref_128_2), sad8x8_2_3);

    src_128_1  = _mm_loadu_si128((__m128i const*)(src + 4 * src_stride));
    src_128_2  = _mm_loadu_si128((__m128i const*)(src + 12 * src_stride));
    ref_128_1  = _mm_loadu_si128((__m128i const*)(ref + 4 * ref_stride));
    ref_128_2  = _mm_loadu_si128((__m128i const*)(ref + 12 * ref_stride));
    sad8x8_0_1 = _mm_add_epi32(_mm_sad_epu8(src_128_1, ref_128_1), sad8x8_0_1);
    sad8x8_2_3 = _mm_add_epi32(_mm_sad_epu8(src_128_2, ref_128_2), sad8x8_2_3);

    src_128_1  = _mm_loadu_si128((__m128i const*)(src + 6 * src_stride));
    src_128_2  = _mm_loadu_si128((__m128i const*)(src + 14 * src_stride));
    ref_128_1  = _mm_loadu_si128((__m128i const*)(ref + 6 * ref_stride));
    ref_128_2  = _mm_loadu_si128((__m128i const*)(ref + 14 * ref_stride));
    sad8x8_0_1 = _mm_add_epi32(_mm_sad_epu8(src_128_1, ref_128_1), sad8x8_0_1);
    sad8x8_2_3 = _mm_add_epi32(_mm_sad_epu8(src_128_2, ref_128_2), sad8x8_2_3);

    if (sub_sad) {
        sad8x8_0_1 = _mm_slli_epi32(sad8x8_0_1, 1);
        sad8x8_2_3 = _mm_slli_epi32(sad8x8_2_3, 1);
    } else {
        src_128_1  = _mm_loadu_si128((__m128i const*)(src + 1 * src_stride));
        src_128_2  = _mm_loadu_si128((__m128i const*)(src + 9 * src_stride));
        ref_128_1  = _mm_loadu_si128((__m128i const*)(ref + 1 * ref_stride));
        ref_128_2  = _mm_loadu_si128((__m128i const*)(ref + 9 * ref_stride));
        sad8x8_0_1 = _mm_add_epi32(_mm_sad_epu8(src_128_1, ref_128_1), sad8x8_0_1);
        sad8x8_2_3 = _mm_add_epi32(_mm_sad_epu8(src_128_2, ref_128_2), sad8x8_2_3);

        src_128_1  = _mm_loadu_si128((__m128i const*)(src + 3 * src_stride));
        src_128_2  = _mm_loadu_si128((__m128i const*)(src + 11 * src_stride));
        ref_128_1  = _mm_loadu_si128((__m128i const*)(ref + 3 * ref_stride));
        ref_128_2  = _mm_loadu_si128((__m128i const*)(ref + 11 * ref_stride));
        sad8x8_0_1 = _mm_add_epi32(_mm_sad_epu8(src_128_1, ref_128_1), sad8x8_0_1);
        sad8x8_2_3 = _mm_add_epi32(_mm_sad_epu8(src_128_2, ref_128_2), sad8x8_2_3);

        src_128_1  = _mm_loadu_si128((__m128i const*)(src + 5 * src_stride));
        src_128_2  = _mm_loadu_si128((__m128i const*)(src + 13 * src_stride));
        ref_128_1  = _mm_loadu_si128((__m128i const*)(ref + 5 * ref_stride));
        ref_128_2  = _mm_loadu_si128((__m128i const*)(ref + 13 * ref_stride));
        sad8x8_0_1 = _mm_add_epi32(_mm_sad_epu8(src_128_1, ref_128_1), sad8x8_0_1);
        sad8x8_2_3 = _mm_add_epi32(_mm_sad_epu8(src_128_2, ref_128_2), sad8x8_2_3);

        src_128_1  = _mm_loadu_si128((__m128i const*)(src + 7 * src_stride));
        src_128_2  = _mm_loadu_si128((__m128i const*)(src + 15 * src_stride));
        ref_128_1  = _mm_loadu_si128((__m128i const*)(ref + 7 * ref_stride));
        ref_128_2  = _mm_loadu_si128((__m128i const*)(ref + 15 * ref_stride));
        sad8x8_0_1 = _mm_add_epi32(_mm_sad_epu8(src_128_1, ref_128_1), sad8x8_0_1);
        sad8x8_2_3 = _mm_add_epi32(_mm_sad_epu8(src_128_2, ref_128_2), sad8x8_2_3);
    }

    sad8x8_0_3 = _mm_packs_epi32(sad8x8_0_1, sad8x8_2_3);
    _mm_storeu_si128((__m128i*)p_sad8x8, sad8x8_0_3);

    xmm_best_sad8x8 = _mm_loadu_si128((__m128i const*)p_best_sad_8x8);
    xmm_best_mv8x8  = _mm_loadu_si128((__m128i const*)p_best_mv8x8);

    // sad8x8_0 < p_best_sad_8x8[0] for 0 to 3
    sad8x8_less_than_bitmask = _mm_cmplt_epi32(sad8x8_0_3, xmm_best_sad8x8);
    best_sad8x8              = _mm_blendv_epi8(xmm_best_sad8x8, sad8x8_0_3, sad8x8_less_than_bitmask);
    best_mv8x8               = _mm_blendv_epi8(xmm_best_mv8x8, xmm_mv, sad8x8_less_than_bitmask);
    _mm_storeu_si128((__m128i*)p_best_sad_8x8, best_sad8x8);
    _mm_storeu_si128((__m128i*)p_best_mv8x8, best_mv8x8);

    xmm_sad16x16       = _mm_add_epi32(sad8x8_0_1, sad8x8_2_3);
    xmm_sad16x16_total = _mm_add_epi32(_mm_srli_si128(xmm_sad16x16, 8), xmm_sad16x16);
    p_sad16x16[0]      = _mm_cvtsi128_si32(xmm_sad16x16_total);

    if (p_sad16x16[0] < p_best_sad_16x16[0]) {
        p_best_sad_16x16[0] = p_sad16x16[0];
        p_best_mv16x16[0]   = mv;
    }
}

void svt_ext_all_sad_calculation_8x8_16x16_sse4_1(uint8_t* src, uint32_t src_stride, uint8_t* ref, uint32_t ref_stride,
                                                  uint32_t mv, uint32_t* p_best_sad_8x8, uint32_t* p_best_sad_16x16,
                                                  uint32_t* p_best_mv8x8, uint32_t* p_best_mv16x16,
                                                  uint32_t p_eight_sad16x16[16][8], uint32_t p_eight_sad8x8[64][8],
                                                  bool sub_sad) {
    static const char offsets[16] = {0, 1, 4, 5, 2, 3, 6, 7, 8, 9, 12, 13, 10, 11, 14, 15};

    //---- 16x16 : 0, 1, 4, 5, 2, 3, 6, 7, 8, 9, 12, 13, 10, 11, 14, 15
    for (int y = 0; y < 4; y++) {
        for (int x = 0; x < 4; x++) {
            const uint32_t start_16x16_pos = offsets[4 * y + x];
            const uint32_t start_8x8_pos   = 4 * start_16x16_pos;
            const uint8_t* s               = src + 16 * y * src_stride + 16 * x;
            const uint8_t* r               = ref + 16 * y * ref_stride + 16 * x;
            __m128i        sad0, sad1, sad2, sad3;
            sad0 = sad1 = sad2 = sad3 = _mm_setzero_si128();

            if (sub_sad) {
                for (int i = 0; i < 4; i++) {
                    const __m128i src01 = _mm_loadu_si128((__m128i*)(s + 0 * src_stride));
                    const __m128i src23 = _mm_loadu_si128((__m128i*)(s + 8 * src_stride));
                    const __m128i ref0  = _mm_loadu_si128((__m128i*)(r + 0 * ref_stride + 0));
                    const __m128i ref1  = _mm_loadu_si128((__m128i*)(r + 0 * ref_stride + 8));
                    const __m128i ref2  = _mm_loadu_si128((__m128i*)(r + 8 * ref_stride + 0));
                    const __m128i ref3  = _mm_loadu_si128((__m128i*)(r + 8 * ref_stride + 8));

                    sad0 = _mm_adds_epu16(sad0, _mm_mpsadbw_epu8(ref0, src01, 0));
                    sad2 = _mm_adds_epu16(sad2, _mm_mpsadbw_epu8(ref2, src23, 0));
                    sad0 = _mm_adds_epu16(sad0, _mm_mpsadbw_epu8(ref0, src01, 45));
                    sad2 = _mm_adds_epu16(sad2, _mm_mpsadbw_epu8(ref2, src23, 45));
                    sad1 = _mm_adds_epu16(sad1, _mm_mpsadbw_epu8(ref1, src01, 18));
                    sad3 = _mm_adds_epu16(sad3, _mm_mpsadbw_epu8(ref3, src23, 18));
                    sad1 = _mm_adds_epu16(sad1, _mm_mpsadbw_epu8(ref1, src01, 63));
                    sad3 = _mm_adds_epu16(sad3, _mm_mpsadbw_epu8(ref3, src23, 63));

                    s += 2 * src_stride;
                    r += 2 * ref_stride;
                }

                sad0 = _mm_slli_epi16(sad0, 1);
                sad1 = _mm_slli_epi16(sad1, 1);
                sad2 = _mm_slli_epi16(sad2, 1);
                sad3 = _mm_slli_epi16(sad3, 1);
            } else {
                for (int i = 0; i < 8; i++) {
                    const __m128i src01 = _mm_loadu_si128((__m128i*)(s + 0 * src_stride));
                    const __m128i src23 = _mm_loadu_si128((__m128i*)(s + 8 * src_stride));
                    const __m128i ref0  = _mm_loadu_si128((__m128i*)(r + 0 * ref_stride + 0));
                    const __m128i ref1  = _mm_loadu_si128((__m128i*)(r + 0 * ref_stride + 8));
                    const __m128i ref2  = _mm_loadu_si128((__m128i*)(r + 8 * ref_stride + 0));
                    const __m128i ref3  = _mm_loadu_si128((__m128i*)(r + 8 * ref_stride + 8));

                    sad0 = _mm_adds_epu16(sad0, _mm_mpsadbw_epu8(ref0, src01, 0));
                    sad2 = _mm_adds_epu16(sad2, _mm_mpsadbw_epu8(ref2, src23, 0));
                    sad0 = _mm_adds_epu16(sad0, _mm_mpsadbw_epu8(ref0, src01, 45));
                    sad2 = _mm_adds_epu16(sad2, _mm_mpsadbw_epu8(ref2, src23, 45));
                    sad1 = _mm_adds_epu16(sad1, _mm_mpsadbw_epu8(ref1, src01, 18));
                    sad3 = _mm_adds_epu16(sad3, _mm_mpsadbw_epu8(ref3, src23, 18));
                    sad1 = _mm_adds_epu16(sad1, _mm_mpsadbw_epu8(ref1, src01, 63));
                    sad3 = _mm_adds_epu16(sad3, _mm_mpsadbw_epu8(ref3, src23, 63));

                    s += src_stride;
                    r += ref_stride;
                }
            }

            (void)p_eight_sad8x8;

            const __m128i minpos0 = _mm_minpos_epu16(sad0);
            const __m128i minpos1 = _mm_minpos_epu16(sad1);
            const __m128i minpos2 = _mm_minpos_epu16(sad2);
            const __m128i minpos3 = _mm_minpos_epu16(sad3);

            const __m128i minpos01   = _mm_unpacklo_epi16(minpos0, minpos1);
            const __m128i minpos23   = _mm_unpacklo_epi16(minpos2, minpos3);
            const __m128i minpos0123 = _mm_unpacklo_epi32(minpos01, minpos23);
            const __m128i sad8x8     = _mm_unpacklo_epi16(minpos0123, _mm_setzero_si128());
            const __m128i pos8x8     = _mm_unpackhi_epi16(minpos0123, _mm_setzero_si128());

            __m128i       best_sad8x8 = _mm_loadu_si128((__m128i*)(p_best_sad_8x8 + start_8x8_pos));
            const __m128i mask        = _mm_cmplt_epi32(sad8x8, best_sad8x8);
            best_sad8x8               = _mm_min_epi32(best_sad8x8, sad8x8);
            _mm_storeu_si128((__m128i*)(p_best_sad_8x8 + start_8x8_pos), best_sad8x8);

            const __m128i mvs        = _mm_set1_epi32(mv);
            __m128i       best_mv8x8 = _mm_loadu_si128((__m128i*)(p_best_mv8x8 + start_8x8_pos));
            const __m128i mv8x8      = _mm_add_epi16(mvs, pos8x8);
            best_mv8x8               = _mm_blendv_epi8(best_mv8x8, mv8x8, mask);
            _mm_storeu_si128((__m128i*)(p_best_mv8x8 + start_8x8_pos), best_mv8x8);
            const __m128i sum01       = _mm_add_epi16(sad0, sad1);
            const __m128i sum23       = _mm_add_epi16(sad2, sad3);
            const __m128i sad16x16_16 = _mm_add_epi16(sum01, sum23);

            _mm_storeu_si128((__m128i*)(p_eight_sad16x16[start_16x16_pos]), _mm_cvtepu16_epi32(sad16x16_16));
            _mm_storeu_si128((__m128i*)(p_eight_sad16x16[start_16x16_pos] + 4),
                             _mm_cvtepu16_epi32(_mm_srli_si128(sad16x16_16, 8)));

            const __m128i  minpos16x16 = _mm_minpos_epu16(sad16x16_16);
            const uint32_t min16x16    = _mm_extract_epi16(minpos16x16, 0);

            if (min16x16 < p_best_sad_16x16[start_16x16_pos]) {
                p_best_sad_16x16[start_16x16_pos] = min16x16;

                const __m128i pos16x16          = _mm_srli_si128(minpos16x16, 2);
                const __m128i mv16x16           = _mm_add_epi16(mvs, pos16x16);
                p_best_mv16x16[start_16x16_pos] = _mm_extract_epi32(mv16x16, 0);
            }
        }
    }
}

#define UPDATE_BEST_PME_32(s, k, offset)                                \
    tem_sum_1 = _mm_extract_epi32(s, k);                                \
    best_mv.x = mvx + (search_position_start_x + j + offset + k) * 8;   \
    best_mv.y = mvy + (search_position_start_y + i) * 8;                \
    tem_sum_1 += svt_aom_fp_mv_err_cost(&best_mv, mv_cost_params);      \
    if (tem_sum_1 < low_sum) {                                          \
        low_sum = tem_sum_1;                                            \
        x_best  = mvx + (search_position_start_x + j + offset + k) * 8; \
        y_best  = mvy + (search_position_start_y + i) * 8;              \
    }

#define UPDATE_BEST_PME_16(s, k)                                   \
    tem_sum_1 = _mm_extract_epi16(s, k);                           \
    best_mv.x = mvx + (search_position_start_x + j + k) * 8;       \
    best_mv.y = mvy + (search_position_start_y + i) * 8;           \
    tem_sum_1 += svt_aom_fp_mv_err_cost(&best_mv, mv_cost_params); \
    if (tem_sum_1 < low_sum) {                                     \
        low_sum = tem_sum_1;                                       \
        x_best  = mvx + (search_position_start_x + j + k) * 8;     \
        y_best  = mvy + (search_position_start_y + i) * 8;         \
    }

void svt_pme_sad_loop_kernel_sse4_1(const svt_mv_cost_param* mv_cost_params,
                                    uint8_t*                 src, // input parameter, source samples Ptr
                                    uint32_t                 src_stride, // input parameter, source stride
                                    uint8_t*                 ref, // input parameter, reference samples Ptr
                                    uint32_t                 ref_stride, // input parameter, reference stride
                                    uint32_t                 block_height, // input parameter, block height (M)
                                    uint32_t                 block_width, // input parameter, block width (N)
                                    uint32_t* best_cost, int16_t* best_mvx, int16_t* best_mvy,
                                    int16_t search_position_start_x, int16_t search_position_start_y,
                                    int16_t search_area_width, int16_t search_area_height, int16_t search_step,
                                    int16_t mvx, int16_t mvy) {
    int16_t        x_best = *best_mvx, y_best = *best_mvy;
    uint32_t       low_sum   = *best_cost;
    uint32_t       tem_sum_1 = 0;
    int16_t        i, j;
    uint32_t       k;
    const uint8_t *p_ref, *p_src;
    __m128i        s0, s1, s2, s3, s4, s5, s6, s7 = _mm_set1_epi32(-1);
    Mv             best_mv;
    switch (block_width) {
    case 4:
        for (i = 0; i < search_area_height; i += search_step) {
            for (j = 0; j <= search_area_width - 8; j += (8 + search_step - 1)) {
                if ((search_area_width - j) < 8) {
                    continue;
                }
                p_src = src;
                p_ref = ref + j;
                s3    = _mm_setzero_si128();
                for (k = 0; k < block_height; k += 2) {
                    s0 = _mm_loadu_si128((__m128i*)p_ref);
                    s1 = _mm_loadu_si128((__m128i*)(p_ref + ref_stride));
                    s2 = _mm_cvtsi32_si128(*(uint32_t*)p_src);
                    s5 = _mm_cvtsi32_si128(*(uint32_t*)(p_src + src_stride));
                    s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                    s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s1, s5, 0));
                    p_src += src_stride << 1;
                    p_ref += ref_stride << 1;
                }
                UPDATE_BEST_PME_16(s3, 0);
                UPDATE_BEST_PME_16(s3, 1);
                UPDATE_BEST_PME_16(s3, 2);
                UPDATE_BEST_PME_16(s3, 3);
                UPDATE_BEST_PME_16(s3, 4);
                UPDATE_BEST_PME_16(s3, 5);
                UPDATE_BEST_PME_16(s3, 6);
                UPDATE_BEST_PME_16(s3, 7);
            }
            ref += search_step * ref_stride;
        }
        break;

    case 8:
        for (i = 0; i < search_area_height; i += search_step) {
            for (j = 0; j <= search_area_width - 8; j += (8 + search_step - 1)) {
                if ((search_area_width - j) < 8) {
                    continue;
                }
                p_src = src;
                p_ref = ref + j;
                s3 = s4 = _mm_setzero_si128();
                for (k = 0; k < block_height; k += 2) {
                    s0 = _mm_loadu_si128((__m128i*)p_ref);
                    s1 = _mm_loadu_si128((__m128i*)(p_ref + ref_stride));
                    s2 = _mm_loadl_epi64((__m128i*)p_src);
                    s5 = _mm_loadl_epi64((__m128i*)(p_src + src_stride));
                    s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                    s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                    s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s1, s5, 0));
                    s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s1, s5, 5));
                    p_src += src_stride << 1;
                    p_ref += ref_stride << 1;
                }
                s3 = _mm_adds_epu16(s3, s4);
                UPDATE_BEST_PME_16(s3, 0);
                UPDATE_BEST_PME_16(s3, 1);
                UPDATE_BEST_PME_16(s3, 2);
                UPDATE_BEST_PME_16(s3, 3);
                UPDATE_BEST_PME_16(s3, 4);
                UPDATE_BEST_PME_16(s3, 5);
                UPDATE_BEST_PME_16(s3, 6);
                UPDATE_BEST_PME_16(s3, 7);
            }
            ref += search_step * ref_stride;
        }
        break;

    case 16:
        if (block_height <= 16) {
            for (i = 0; i < search_area_height; i += search_step) {
                for (j = 0; j <= search_area_width - 8; j += (8 + search_step - 1)) {
                    if ((search_area_width - j) < 8) {
                        continue;
                    }
                    p_src = src;
                    p_ref = ref + j;
                    s4 = s5 = _mm_setzero_si128();
                    for (k = 0; k < block_height; k++) {
                        s0 = _mm_loadu_si128((__m128i*)p_ref);
                        s1 = _mm_loadu_si128((__m128i*)(p_ref + 8));
                        s2 = _mm_loadu_si128((__m128i*)p_src);

                        s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 0));
                        s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s0, s2, 45));
                        s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s1, s2, 18));
                        s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 63));

                        p_src += src_stride;
                        p_ref += ref_stride;
                    }
                    s4 = _mm_adds_epu16(s4, s5);
                    UPDATE_BEST_PME_16(s4, 0);
                    UPDATE_BEST_PME_16(s4, 1);
                    UPDATE_BEST_PME_16(s4, 2);
                    UPDATE_BEST_PME_16(s4, 3);
                    UPDATE_BEST_PME_16(s4, 4);
                    UPDATE_BEST_PME_16(s4, 5);
                    UPDATE_BEST_PME_16(s4, 6);
                    UPDATE_BEST_PME_16(s4, 7);
                }
                ref += search_step * ref_stride;
            }
        } else {
            for (i = 0; i < search_area_height; i += search_step) {
                for (j = 0; j <= search_area_width - 8; j += (8 + search_step - 1)) {
                    if ((search_area_width - j) < 8) {
                        continue;
                    }
                    p_src = src;
                    p_ref = ref + j;

                    s4 = s5 = s6 = s7 = _mm_setzero_si128();
                    for (k = 0; k < block_height; k++) {
                        s0 = _mm_loadu_si128((__m128i*)p_ref);
                        s1 = _mm_loadu_si128((__m128i*)(p_ref + 8));
                        s2 = _mm_loadu_si128((__m128i*)p_src);

                        s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 0));
                        s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s0, s2, 45));
                        s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 18));
                        s7 = _mm_adds_epu16(s7, _mm_mpsadbw_epu8(s1, s2, 63));

                        p_src += src_stride;
                        p_ref += ref_stride;
                    }
                    s0 = _mm_unpacklo_epi16(s4, _mm_setzero_si128());
                    s1 = _mm_unpackhi_epi16(s4, _mm_setzero_si128());
                    s2 = _mm_unpacklo_epi16(s5, _mm_setzero_si128());
                    s3 = _mm_unpackhi_epi16(s5, _mm_setzero_si128());
                    s4 = _mm_unpacklo_epi16(s6, _mm_setzero_si128());
                    s5 = _mm_unpackhi_epi16(s6, _mm_setzero_si128());
                    s6 = _mm_unpacklo_epi16(s7, _mm_setzero_si128());
                    s7 = _mm_unpackhi_epi16(s7, _mm_setzero_si128());

                    s0 = _mm_add_epi32(s0, s2);
                    s0 = _mm_add_epi32(s0, s4);
                    s0 = _mm_add_epi32(s0, s6);
                    s3 = _mm_add_epi32(s1, s3);
                    s3 = _mm_add_epi32(s5, s3);
                    s3 = _mm_add_epi32(s7, s3);

                    UPDATE_BEST_PME_32(s0, 0, 0);
                    UPDATE_BEST_PME_32(s0, 1, 0);
                    UPDATE_BEST_PME_32(s0, 2, 0);
                    UPDATE_BEST_PME_32(s0, 3, 0);
                    UPDATE_BEST_PME_32(s3, 0, 4);
                    UPDATE_BEST_PME_32(s3, 1, 4);
                    UPDATE_BEST_PME_32(s3, 2, 4);
                    UPDATE_BEST_PME_32(s3, 3, 4);
                }
                ref += search_step * ref_stride;
            }
        }
        break;

    case 24:
        for (i = 0; i < search_area_height; i += search_step) {
            for (j = 0; j <= search_area_width - 8; j += (8 + search_step - 1)) {
                if ((search_area_width - j) < 8) {
                    continue;
                }
                p_src = src;
                p_ref = ref + j;
                s3 = s4 = s5 = s6 = _mm_setzero_si128();
                for (k = 0; k < block_height; k++) {
                    s0 = _mm_loadu_si128((__m128i*)p_ref);
                    s1 = _mm_loadu_si128((__m128i*)(p_ref + 8));
                    s2 = _mm_loadu_si128((__m128i*)p_src);
                    s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                    s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                    s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                    s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                    s0 = _mm_loadu_si128((__m128i*)(p_ref + 16));
                    s2 = _mm_loadl_epi64((__m128i*)(p_src + 16));
                    s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                    s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                    p_src += src_stride;
                    p_ref += ref_stride;
                }
                s0 = _mm_unpacklo_epi16(s3, _mm_setzero_si128());
                s3 = _mm_unpackhi_epi16(s3, _mm_setzero_si128());
                s1 = _mm_unpacklo_epi16(s4, _mm_setzero_si128());
                s4 = _mm_unpackhi_epi16(s4, _mm_setzero_si128());
                s2 = _mm_unpacklo_epi16(s5, _mm_setzero_si128());
                s5 = _mm_unpackhi_epi16(s5, _mm_setzero_si128());
                s7 = _mm_unpacklo_epi16(s6, _mm_setzero_si128());
                s6 = _mm_unpackhi_epi16(s6, _mm_setzero_si128());
                s0 = _mm_add_epi32(_mm_add_epi32(s0, s1), _mm_add_epi32(s2, s7));
                s3 = _mm_add_epi32(_mm_add_epi32(s3, s4), _mm_add_epi32(s5, s6));

                UPDATE_BEST_PME_32(s0, 0, 0);
                UPDATE_BEST_PME_32(s0, 1, 0);
                UPDATE_BEST_PME_32(s0, 2, 0);
                UPDATE_BEST_PME_32(s0, 3, 0);
                UPDATE_BEST_PME_32(s3, 0, 4);
                UPDATE_BEST_PME_32(s3, 1, 4);
                UPDATE_BEST_PME_32(s3, 2, 4);
                UPDATE_BEST_PME_32(s3, 3, 4);
            }
            ref += search_step * ref_stride;
        }
        break;

    case 32:
        if (block_height <= 32) {
            for (i = 0; i < search_area_height; i += search_step) {
                for (j = 0; j <= search_area_width - 8; j += (8 + (search_step - 1))) {
                    if ((search_area_width - j) < 8) {
                        continue;
                    }
                    p_src = src;
                    p_ref = ref + j;
                    s3 = s4 = s5 = s6 = _mm_setzero_si128();
                    for (k = 0; k < block_height; k++) {
                        s0 = _mm_loadu_si128((__m128i*)p_ref);
                        s1 = _mm_loadu_si128((__m128i*)(p_ref + 8));
                        s2 = _mm_loadu_si128((__m128i*)p_src);
                        s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                        s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                        s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                        s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                        s0 = _mm_loadu_si128((__m128i*)(p_ref + 16));
                        s1 = _mm_loadu_si128((__m128i*)(p_ref + 24));
                        s2 = _mm_loadu_si128((__m128i*)(p_src + 16));
                        s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                        s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                        s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                        s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                        p_src += src_stride;
                        p_ref += ref_stride;
                    }
                    s0 = _mm_unpacklo_epi16(s3, _mm_setzero_si128());
                    s3 = _mm_unpackhi_epi16(s3, _mm_setzero_si128());
                    s1 = _mm_unpacklo_epi16(s4, _mm_setzero_si128());
                    s4 = _mm_unpackhi_epi16(s4, _mm_setzero_si128());
                    s2 = _mm_unpacklo_epi16(s5, _mm_setzero_si128());
                    s5 = _mm_unpackhi_epi16(s5, _mm_setzero_si128());
                    s7 = _mm_unpacklo_epi16(s6, _mm_setzero_si128());
                    s6 = _mm_unpackhi_epi16(s6, _mm_setzero_si128());
                    s0 = _mm_add_epi32(_mm_add_epi32(s0, s1), _mm_add_epi32(s2, s7));
                    s3 = _mm_add_epi32(_mm_add_epi32(s3, s4), _mm_add_epi32(s5, s6));

                    UPDATE_BEST_PME_32(s0, 0, 0);
                    UPDATE_BEST_PME_32(s0, 1, 0);
                    UPDATE_BEST_PME_32(s0, 2, 0);
                    UPDATE_BEST_PME_32(s0, 3, 0);
                    UPDATE_BEST_PME_32(s3, 0, 4);
                    UPDATE_BEST_PME_32(s3, 1, 4);
                    UPDATE_BEST_PME_32(s3, 2, 4);
                    UPDATE_BEST_PME_32(s3, 3, 4);
                }
                ref += search_step * ref_stride;
            }
        } else {
            __m128i s9, s10, s11, s12;
            for (i = 0; i < search_area_height; i += search_step) {
                for (j = 0; j <= search_area_width - 8; j += (8 + search_step - 1)) {
                    if ((search_area_width - j) < 8) {
                        continue;
                    }
                    p_src = src;
                    p_ref = ref + j;
                    s3 = s4 = s5 = s6 = _mm_setzero_si128();
                    for (k = 0; k < (block_height >> 1); k++) {
                        s0 = _mm_loadu_si128((__m128i*)p_ref);
                        s1 = _mm_loadu_si128((__m128i*)(p_ref + 8));
                        s2 = _mm_loadu_si128((__m128i*)p_src);
                        s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                        s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                        s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                        s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                        s0 = _mm_loadu_si128((__m128i*)(p_ref + 16));
                        s1 = _mm_loadu_si128((__m128i*)(p_ref + 24));
                        s2 = _mm_loadu_si128((__m128i*)(p_src + 16));
                        s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                        s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                        s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                        s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                        p_src += src_stride;
                        p_ref += ref_stride;
                    }
                    s9 = s10 = s11 = s12 = _mm_setzero_si128();
                    for (; k < block_height; k++) {
                        s0  = _mm_loadu_si128((__m128i*)p_ref);
                        s1  = _mm_loadu_si128((__m128i*)(p_ref + 8));
                        s2  = _mm_loadu_si128((__m128i*)p_src);
                        s9  = _mm_adds_epu16(s9, _mm_mpsadbw_epu8(s0, s2, 0));
                        s10 = _mm_adds_epu16(s10, _mm_mpsadbw_epu8(s0, s2, 5));
                        s11 = _mm_adds_epu16(s11, _mm_mpsadbw_epu8(s1, s2, 2));
                        s12 = _mm_adds_epu16(s12, _mm_mpsadbw_epu8(s1, s2, 7));
                        s0  = _mm_loadu_si128((__m128i*)(p_ref + 16));
                        s1  = _mm_loadu_si128((__m128i*)(p_ref + 24));
                        s2  = _mm_loadu_si128((__m128i*)(p_src + 16));
                        s9  = _mm_adds_epu16(s9, _mm_mpsadbw_epu8(s0, s2, 0));
                        s10 = _mm_adds_epu16(s10, _mm_mpsadbw_epu8(s0, s2, 5));
                        s11 = _mm_adds_epu16(s11, _mm_mpsadbw_epu8(s1, s2, 2));
                        s12 = _mm_adds_epu16(s12, _mm_mpsadbw_epu8(s1, s2, 7));
                        p_src += src_stride;
                        p_ref += ref_stride;
                    }
                    s0  = _mm_unpacklo_epi16(s3, _mm_setzero_si128());
                    s3  = _mm_unpackhi_epi16(s3, _mm_setzero_si128());
                    s1  = _mm_unpacklo_epi16(s4, _mm_setzero_si128());
                    s4  = _mm_unpackhi_epi16(s4, _mm_setzero_si128());
                    s2  = _mm_unpacklo_epi16(s5, _mm_setzero_si128());
                    s5  = _mm_unpackhi_epi16(s5, _mm_setzero_si128());
                    s7  = _mm_unpacklo_epi16(s6, _mm_setzero_si128());
                    s6  = _mm_unpackhi_epi16(s6, _mm_setzero_si128());
                    s0  = _mm_add_epi32(_mm_add_epi32(s0, s1), _mm_add_epi32(s2, s7));
                    s3  = _mm_add_epi32(_mm_add_epi32(s3, s4), _mm_add_epi32(s5, s6));
                    s1  = _mm_unpacklo_epi16(s9, _mm_setzero_si128());
                    s9  = _mm_unpackhi_epi16(s9, _mm_setzero_si128());
                    s2  = _mm_unpacklo_epi16(s10, _mm_setzero_si128());
                    s10 = _mm_unpackhi_epi16(s10, _mm_setzero_si128());
                    s4  = _mm_unpacklo_epi16(s11, _mm_setzero_si128());
                    s11 = _mm_unpackhi_epi16(s11, _mm_setzero_si128());
                    s5  = _mm_unpacklo_epi16(s12, _mm_setzero_si128());
                    s12 = _mm_unpackhi_epi16(s12, _mm_setzero_si128());
                    s0  = _mm_add_epi32(s0, _mm_add_epi32(_mm_add_epi32(s1, s2), _mm_add_epi32(s4, s5)));
                    s3  = _mm_add_epi32(s3, _mm_add_epi32(_mm_add_epi32(s9, s10), _mm_add_epi32(s11, s12)));
                    UPDATE_BEST_PME_32(s0, 0, 0);
                    UPDATE_BEST_PME_32(s0, 1, 0);
                    UPDATE_BEST_PME_32(s0, 2, 0);
                    UPDATE_BEST_PME_32(s0, 3, 0);
                    UPDATE_BEST_PME_32(s3, 0, 4);
                    UPDATE_BEST_PME_32(s3, 1, 4);
                    UPDATE_BEST_PME_32(s3, 2, 4);
                    UPDATE_BEST_PME_32(s3, 3, 4);
                }
                ref += search_step * ref_stride;
            }
        }
        break;

    case 48:
        if (block_height <= 32) {
            __m128i s9, s10, s11, s12;
            for (i = 0; i < search_area_height; i += search_step) {
                for (j = 0; j <= search_area_width - 8; j += (8 + search_step - 1)) {
                    if ((search_area_width - j) < 8) {
                        continue;
                    }
                    p_src = src;
                    p_ref = ref + j;
                    s3 = s4 = s5 = s6 = _mm_setzero_si128();
                    for (k = 0; k < (block_height >> 1); k++) {
                        s0 = _mm_loadu_si128((__m128i*)p_ref);
                        s1 = _mm_loadu_si128((__m128i*)(p_ref + 8));
                        s2 = _mm_loadu_si128((__m128i*)p_src);
                        s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                        s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                        s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                        s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                        s0 = _mm_loadu_si128((__m128i*)(p_ref + 16));
                        s1 = _mm_loadu_si128((__m128i*)(p_ref + 24));
                        s2 = _mm_loadu_si128((__m128i*)(p_src + 16));
                        s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                        s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                        s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                        s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                        s0 = _mm_loadu_si128((__m128i*)(p_ref + 32));
                        s1 = _mm_loadu_si128((__m128i*)(p_ref + 40));
                        s2 = _mm_loadu_si128((__m128i*)(p_src + 32));
                        s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                        s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                        s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                        s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                        p_src += src_stride;
                        p_ref += ref_stride;
                    }
                    s9 = s10 = s11 = s12 = _mm_setzero_si128();
                    for (; k < block_height; k++) {
                        s0  = _mm_loadu_si128((__m128i*)p_ref);
                        s1  = _mm_loadu_si128((__m128i*)(p_ref + 8));
                        s2  = _mm_loadu_si128((__m128i*)p_src);
                        s9  = _mm_adds_epu16(s9, _mm_mpsadbw_epu8(s0, s2, 0));
                        s10 = _mm_adds_epu16(s10, _mm_mpsadbw_epu8(s0, s2, 5));
                        s11 = _mm_adds_epu16(s11, _mm_mpsadbw_epu8(s1, s2, 2));
                        s12 = _mm_adds_epu16(s12, _mm_mpsadbw_epu8(s1, s2, 7));
                        s0  = _mm_loadu_si128((__m128i*)(p_ref + 16));
                        s1  = _mm_loadu_si128((__m128i*)(p_ref + 24));
                        s2  = _mm_loadu_si128((__m128i*)(p_src + 16));
                        s9  = _mm_adds_epu16(s9, _mm_mpsadbw_epu8(s0, s2, 0));
                        s10 = _mm_adds_epu16(s10, _mm_mpsadbw_epu8(s0, s2, 5));
                        s11 = _mm_adds_epu16(s11, _mm_mpsadbw_epu8(s1, s2, 2));
                        s12 = _mm_adds_epu16(s12, _mm_mpsadbw_epu8(s1, s2, 7));
                        s0  = _mm_loadu_si128((__m128i*)(p_ref + 32));
                        s1  = _mm_loadu_si128((__m128i*)(p_ref + 40));
                        s2  = _mm_loadu_si128((__m128i*)(p_src + 32));
                        s9  = _mm_adds_epu16(s9, _mm_mpsadbw_epu8(s0, s2, 0));
                        s10 = _mm_adds_epu16(s10, _mm_mpsadbw_epu8(s0, s2, 5));
                        s11 = _mm_adds_epu16(s11, _mm_mpsadbw_epu8(s1, s2, 2));
                        s12 = _mm_adds_epu16(s12, _mm_mpsadbw_epu8(s1, s2, 7));
                        p_src += src_stride;
                        p_ref += ref_stride;
                    }
                    s0  = _mm_unpacklo_epi16(s3, _mm_setzero_si128());
                    s3  = _mm_unpackhi_epi16(s3, _mm_setzero_si128());
                    s1  = _mm_unpacklo_epi16(s4, _mm_setzero_si128());
                    s4  = _mm_unpackhi_epi16(s4, _mm_setzero_si128());
                    s2  = _mm_unpacklo_epi16(s5, _mm_setzero_si128());
                    s5  = _mm_unpackhi_epi16(s5, _mm_setzero_si128());
                    s7  = _mm_unpacklo_epi16(s6, _mm_setzero_si128());
                    s6  = _mm_unpackhi_epi16(s6, _mm_setzero_si128());
                    s0  = _mm_add_epi32(_mm_add_epi32(s0, s1), _mm_add_epi32(s2, s7));
                    s3  = _mm_add_epi32(_mm_add_epi32(s3, s4), _mm_add_epi32(s5, s6));
                    s1  = _mm_unpacklo_epi16(s9, _mm_setzero_si128());
                    s9  = _mm_unpackhi_epi16(s9, _mm_setzero_si128());
                    s2  = _mm_unpacklo_epi16(s10, _mm_setzero_si128());
                    s10 = _mm_unpackhi_epi16(s10, _mm_setzero_si128());
                    s4  = _mm_unpacklo_epi16(s11, _mm_setzero_si128());
                    s11 = _mm_unpackhi_epi16(s11, _mm_setzero_si128());
                    s5  = _mm_unpacklo_epi16(s12, _mm_setzero_si128());
                    s12 = _mm_unpackhi_epi16(s12, _mm_setzero_si128());
                    s0  = _mm_add_epi32(s0, _mm_add_epi32(_mm_add_epi32(s1, s2), _mm_add_epi32(s4, s5)));
                    s3  = _mm_add_epi32(s3, _mm_add_epi32(_mm_add_epi32(s9, s10), _mm_add_epi32(s11, s12)));
                    UPDATE_BEST_PME_32(s0, 0, 0);
                    UPDATE_BEST_PME_32(s0, 1, 0);
                    UPDATE_BEST_PME_32(s0, 2, 0);
                    UPDATE_BEST_PME_32(s0, 3, 0);
                    UPDATE_BEST_PME_32(s3, 0, 4);
                    UPDATE_BEST_PME_32(s3, 1, 4);
                    UPDATE_BEST_PME_32(s3, 2, 4);
                    UPDATE_BEST_PME_32(s3, 3, 4);
                }
                ref += search_step * ref_stride;
            }
        } else {
            __m128i s9, s10;
            for (i = 0; i < search_area_height; i += search_step) {
                for (j = 0; j <= search_area_width - 8; j += (8 + search_step - 1)) {
                    if ((search_area_width - j) < 8) {
                        continue;
                    }
                    p_src = src;
                    p_ref = ref + j;
                    s9 = s10 = _mm_setzero_si128();
                    k        = 0;
                    while (k < block_height) {
                        s3 = s4 = s5 = s6 = _mm_setzero_si128();
                        for (uint32_t l = 0; l < 21 && k < block_height; k++, l++) {
                            s0 = _mm_loadu_si128((__m128i*)p_ref);
                            s1 = _mm_loadu_si128((__m128i*)(p_ref + 8));
                            s2 = _mm_loadu_si128((__m128i*)p_src);
                            s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                            s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                            s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                            s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                            s0 = _mm_loadu_si128((__m128i*)(p_ref + 16));
                            s1 = _mm_loadu_si128((__m128i*)(p_ref + 24));
                            s2 = _mm_loadu_si128((__m128i*)(p_src + 16));
                            s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                            s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                            s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                            s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                            s0 = _mm_loadu_si128((__m128i*)(p_ref + 32));
                            s1 = _mm_loadu_si128((__m128i*)(p_ref + 40));
                            s2 = _mm_loadu_si128((__m128i*)(p_src + 32));
                            s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                            s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                            s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                            s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                            p_src += src_stride;
                            p_ref += ref_stride;
                        }
                        s0  = _mm_unpacklo_epi16(s3, _mm_setzero_si128());
                        s3  = _mm_unpackhi_epi16(s3, _mm_setzero_si128());
                        s1  = _mm_unpacklo_epi16(s4, _mm_setzero_si128());
                        s4  = _mm_unpackhi_epi16(s4, _mm_setzero_si128());
                        s2  = _mm_unpacklo_epi16(s5, _mm_setzero_si128());
                        s5  = _mm_unpackhi_epi16(s5, _mm_setzero_si128());
                        s7  = _mm_unpacklo_epi16(s6, _mm_setzero_si128());
                        s6  = _mm_unpackhi_epi16(s6, _mm_setzero_si128());
                        s9  = _mm_add_epi32(s9, _mm_add_epi32(_mm_add_epi32(s0, s1), _mm_add_epi32(s2, s7)));
                        s10 = _mm_add_epi32(s10, _mm_add_epi32(_mm_add_epi32(s3, s4), _mm_add_epi32(s5, s6)));
                    }
                    UPDATE_BEST_PME_32(s9, 0, 0);
                    UPDATE_BEST_PME_32(s9, 1, 0);
                    UPDATE_BEST_PME_32(s9, 2, 0);
                    UPDATE_BEST_PME_32(s9, 3, 0);
                    UPDATE_BEST_PME_32(s10, 0, 4);
                    UPDATE_BEST_PME_32(s10, 1, 4);
                    UPDATE_BEST_PME_32(s10, 2, 4);
                    UPDATE_BEST_PME_32(s10, 3, 4);
                }
                ref += search_step * ref_stride;
            }
        }
        break;

    case 64:
        if (block_height <= 32) {
            __m128i s9, s10, s11, s12;
            for (i = 0; i < search_area_height; i += search_step) {
                for (j = 0; j <= search_area_width - 8; j += (8 + search_step - 1)) {
                    if ((search_area_width - j) < 8) {
                        continue;
                    }
                    p_src = src;
                    p_ref = ref + j;
                    s3 = s4 = s5 = s6 = _mm_setzero_si128();
                    for (k = 0; k < (block_height >> 1); k++) {
                        s0 = _mm_loadu_si128((__m128i*)p_ref);
                        s1 = _mm_loadu_si128((__m128i*)(p_ref + 8));
                        s2 = _mm_loadu_si128((__m128i*)p_src);
                        s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                        s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                        s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                        s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                        s0 = _mm_loadu_si128((__m128i*)(p_ref + 16));
                        s1 = _mm_loadu_si128((__m128i*)(p_ref + 24));
                        s2 = _mm_loadu_si128((__m128i*)(p_src + 16));
                        s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                        s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                        s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                        s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                        s0 = _mm_loadu_si128((__m128i*)(p_ref + 32));
                        s1 = _mm_loadu_si128((__m128i*)(p_ref + 40));
                        s2 = _mm_loadu_si128((__m128i*)(p_src + 32));
                        s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                        s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                        s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                        s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                        s0 = _mm_loadu_si128((__m128i*)(p_ref + 48));
                        s1 = _mm_loadu_si128((__m128i*)(p_ref + 56));
                        s2 = _mm_loadu_si128((__m128i*)(p_src + 48));
                        s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                        s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                        s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                        s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                        p_src += src_stride;
                        p_ref += ref_stride;
                    }
                    s9 = s10 = s11 = s12 = _mm_setzero_si128();
                    for (; k < block_height; k++) {
                        s0  = _mm_loadu_si128((__m128i*)p_ref);
                        s1  = _mm_loadu_si128((__m128i*)(p_ref + 8));
                        s2  = _mm_loadu_si128((__m128i*)p_src);
                        s9  = _mm_adds_epu16(s9, _mm_mpsadbw_epu8(s0, s2, 0));
                        s10 = _mm_adds_epu16(s10, _mm_mpsadbw_epu8(s0, s2, 5));
                        s11 = _mm_adds_epu16(s11, _mm_mpsadbw_epu8(s1, s2, 2));
                        s12 = _mm_adds_epu16(s12, _mm_mpsadbw_epu8(s1, s2, 7));
                        s0  = _mm_loadu_si128((__m128i*)(p_ref + 16));
                        s1  = _mm_loadu_si128((__m128i*)(p_ref + 24));
                        s2  = _mm_loadu_si128((__m128i*)(p_src + 16));
                        s9  = _mm_adds_epu16(s9, _mm_mpsadbw_epu8(s0, s2, 0));
                        s10 = _mm_adds_epu16(s10, _mm_mpsadbw_epu8(s0, s2, 5));
                        s11 = _mm_adds_epu16(s11, _mm_mpsadbw_epu8(s1, s2, 2));
                        s12 = _mm_adds_epu16(s12, _mm_mpsadbw_epu8(s1, s2, 7));
                        s0  = _mm_loadu_si128((__m128i*)(p_ref + 32));
                        s1  = _mm_loadu_si128((__m128i*)(p_ref + 40));
                        s2  = _mm_loadu_si128((__m128i*)(p_src + 32));
                        s9  = _mm_adds_epu16(s9, _mm_mpsadbw_epu8(s0, s2, 0));
                        s10 = _mm_adds_epu16(s10, _mm_mpsadbw_epu8(s0, s2, 5));
                        s11 = _mm_adds_epu16(s11, _mm_mpsadbw_epu8(s1, s2, 2));
                        s12 = _mm_adds_epu16(s12, _mm_mpsadbw_epu8(s1, s2, 7));
                        s0  = _mm_loadu_si128((__m128i*)(p_ref + 48));
                        s1  = _mm_loadu_si128((__m128i*)(p_ref + 56));
                        s2  = _mm_loadu_si128((__m128i*)(p_src + 48));
                        s9  = _mm_adds_epu16(s9, _mm_mpsadbw_epu8(s0, s2, 0));
                        s10 = _mm_adds_epu16(s10, _mm_mpsadbw_epu8(s0, s2, 5));
                        s11 = _mm_adds_epu16(s11, _mm_mpsadbw_epu8(s1, s2, 2));
                        s12 = _mm_adds_epu16(s12, _mm_mpsadbw_epu8(s1, s2, 7));
                        p_src += src_stride;
                        p_ref += ref_stride;
                    }
                    s0  = _mm_unpacklo_epi16(s3, _mm_setzero_si128());
                    s3  = _mm_unpackhi_epi16(s3, _mm_setzero_si128());
                    s1  = _mm_unpacklo_epi16(s4, _mm_setzero_si128());
                    s4  = _mm_unpackhi_epi16(s4, _mm_setzero_si128());
                    s2  = _mm_unpacklo_epi16(s5, _mm_setzero_si128());
                    s5  = _mm_unpackhi_epi16(s5, _mm_setzero_si128());
                    s7  = _mm_unpacklo_epi16(s6, _mm_setzero_si128());
                    s6  = _mm_unpackhi_epi16(s6, _mm_setzero_si128());
                    s0  = _mm_add_epi32(_mm_add_epi32(s0, s1), _mm_add_epi32(s2, s7));
                    s3  = _mm_add_epi32(_mm_add_epi32(s3, s4), _mm_add_epi32(s5, s6));
                    s1  = _mm_unpacklo_epi16(s9, _mm_setzero_si128());
                    s9  = _mm_unpackhi_epi16(s9, _mm_setzero_si128());
                    s2  = _mm_unpacklo_epi16(s10, _mm_setzero_si128());
                    s10 = _mm_unpackhi_epi16(s10, _mm_setzero_si128());
                    s4  = _mm_unpacklo_epi16(s11, _mm_setzero_si128());
                    s11 = _mm_unpackhi_epi16(s11, _mm_setzero_si128());
                    s5  = _mm_unpacklo_epi16(s12, _mm_setzero_si128());
                    s12 = _mm_unpackhi_epi16(s12, _mm_setzero_si128());
                    s0  = _mm_add_epi32(s0, _mm_add_epi32(_mm_add_epi32(s1, s2), _mm_add_epi32(s4, s5)));
                    s3  = _mm_add_epi32(s3, _mm_add_epi32(_mm_add_epi32(s9, s10), _mm_add_epi32(s11, s12)));

                    UPDATE_BEST_PME_32(s0, 0, 0);
                    UPDATE_BEST_PME_32(s0, 1, 0);
                    UPDATE_BEST_PME_32(s0, 2, 0);
                    UPDATE_BEST_PME_32(s0, 3, 0);
                    UPDATE_BEST_PME_32(s3, 0, 4);
                    UPDATE_BEST_PME_32(s3, 1, 4);
                    UPDATE_BEST_PME_32(s3, 2, 4);
                    UPDATE_BEST_PME_32(s3, 3, 4);
                }
                ref += search_step * ref_stride;
            }
        } else {
            __m128i s9, s10;
            for (i = 0; i < search_area_height; i += search_step) {
                for (j = 0; j <= search_area_width - 8; j += (8 + search_step - 1)) {
                    if ((search_area_width - j) < 8) {
                        continue;
                    }
                    p_src = src;
                    p_ref = ref + j;
                    s9 = s10 = _mm_setzero_si128();
                    k        = 0;
                    while (k < block_height) {
                        s3 = s4 = s5 = s6 = _mm_setzero_si128();
                        for (uint32_t l = 0; l < 16 && k < block_height; k++, l++) {
                            s0 = _mm_loadu_si128((__m128i*)p_ref);
                            s1 = _mm_loadu_si128((__m128i*)(p_ref + 8));
                            s2 = _mm_loadu_si128((__m128i*)p_src);
                            s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                            s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                            s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                            s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                            s0 = _mm_loadu_si128((__m128i*)(p_ref + 16));
                            s1 = _mm_loadu_si128((__m128i*)(p_ref + 24));
                            s2 = _mm_loadu_si128((__m128i*)(p_src + 16));
                            s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                            s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                            s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                            s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                            s0 = _mm_loadu_si128((__m128i*)(p_ref + 32));
                            s1 = _mm_loadu_si128((__m128i*)(p_ref + 40));
                            s2 = _mm_loadu_si128((__m128i*)(p_src + 32));
                            s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                            s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                            s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                            s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                            s0 = _mm_loadu_si128((__m128i*)(p_ref + 48));
                            s1 = _mm_loadu_si128((__m128i*)(p_ref + 56));
                            s2 = _mm_loadu_si128((__m128i*)(p_src + 48));
                            s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                            s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                            s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                            s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                            p_src += src_stride;
                            p_ref += ref_stride;
                        }
                        s0  = _mm_unpacklo_epi16(s3, _mm_setzero_si128());
                        s3  = _mm_unpackhi_epi16(s3, _mm_setzero_si128());
                        s1  = _mm_unpacklo_epi16(s4, _mm_setzero_si128());
                        s4  = _mm_unpackhi_epi16(s4, _mm_setzero_si128());
                        s2  = _mm_unpacklo_epi16(s5, _mm_setzero_si128());
                        s5  = _mm_unpackhi_epi16(s5, _mm_setzero_si128());
                        s7  = _mm_unpacklo_epi16(s6, _mm_setzero_si128());
                        s6  = _mm_unpackhi_epi16(s6, _mm_setzero_si128());
                        s9  = _mm_add_epi32(s9, _mm_add_epi32(_mm_add_epi32(s0, s1), _mm_add_epi32(s2, s7)));
                        s10 = _mm_add_epi32(s10, _mm_add_epi32(_mm_add_epi32(s3, s4), _mm_add_epi32(s5, s6)));
                    }
                    UPDATE_BEST_PME_32(s9, 0, 0);
                    UPDATE_BEST_PME_32(s9, 1, 0);
                    UPDATE_BEST_PME_32(s9, 2, 0);
                    UPDATE_BEST_PME_32(s9, 3, 0);
                    UPDATE_BEST_PME_32(s10, 0, 4);
                    UPDATE_BEST_PME_32(s10, 1, 4);
                    UPDATE_BEST_PME_32(s10, 2, 4);
                    UPDATE_BEST_PME_32(s10, 3, 4);
                }
                ref += search_step * ref_stride;
            }
        }
        break;
    case 128: {
        __m128i s9, s10;
        for (i = 0; i < search_area_height; i += search_step) {
            for (j = 0; j <= search_area_width - 8; j += (8 + search_step - 1)) {
                if ((search_area_width - j) < 8) {
                    continue;
                }
                p_src = src;
                p_ref = ref + j;
                s9 = s10 = _mm_setzero_si128();
                k        = 0;
                while (k < block_height) {
                    s3 = s4 = s5 = s6 = _mm_setzero_si128();
                    for (uint32_t l = 0; l < 8 && k < block_height; k++, l++) {
                        s0 = _mm_loadu_si128((__m128i*)p_ref);
                        s1 = _mm_loadu_si128((__m128i*)(p_ref + 8));
                        s2 = _mm_loadu_si128((__m128i*)p_src);
                        s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                        s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                        s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                        s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                        s0 = _mm_loadu_si128((__m128i*)(p_ref + 16));
                        s1 = _mm_loadu_si128((__m128i*)(p_ref + 24));
                        s2 = _mm_loadu_si128((__m128i*)(p_src + 16));
                        s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                        s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                        s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                        s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                        s0 = _mm_loadu_si128((__m128i*)(p_ref + 32));
                        s1 = _mm_loadu_si128((__m128i*)(p_ref + 40));
                        s2 = _mm_loadu_si128((__m128i*)(p_src + 32));
                        s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                        s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                        s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                        s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                        s0 = _mm_loadu_si128((__m128i*)(p_ref + 48));
                        s1 = _mm_loadu_si128((__m128i*)(p_ref + 56));
                        s2 = _mm_loadu_si128((__m128i*)(p_src + 48));
                        s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                        s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                        s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                        s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));

                        s0 = _mm_loadu_si128((__m128i*)(p_ref + 64));
                        s1 = _mm_loadu_si128((__m128i*)(p_ref + 72));
                        s2 = _mm_loadu_si128((__m128i*)(p_src + 64));
                        s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                        s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                        s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                        s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                        s0 = _mm_loadu_si128((__m128i*)(p_ref + 80));
                        s1 = _mm_loadu_si128((__m128i*)(p_ref + 88));
                        s2 = _mm_loadu_si128((__m128i*)(p_src + 80));
                        s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                        s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                        s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                        s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                        s0 = _mm_loadu_si128((__m128i*)(p_ref + 96));
                        s1 = _mm_loadu_si128((__m128i*)(p_ref + 104));
                        s2 = _mm_loadu_si128((__m128i*)(p_src + 96));
                        s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                        s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                        s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                        s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                        s0 = _mm_loadu_si128((__m128i*)(p_ref + 112));
                        s1 = _mm_loadu_si128((__m128i*)(p_ref + 120));
                        s2 = _mm_loadu_si128((__m128i*)(p_src + 112));
                        s3 = _mm_adds_epu16(s3, _mm_mpsadbw_epu8(s0, s2, 0));
                        s4 = _mm_adds_epu16(s4, _mm_mpsadbw_epu8(s0, s2, 5));
                        s5 = _mm_adds_epu16(s5, _mm_mpsadbw_epu8(s1, s2, 2));
                        s6 = _mm_adds_epu16(s6, _mm_mpsadbw_epu8(s1, s2, 7));
                        p_src += src_stride;
                        p_ref += ref_stride;
                    }
                    s0  = _mm_unpacklo_epi16(s3, _mm_setzero_si128());
                    s3  = _mm_unpackhi_epi16(s3, _mm_setzero_si128());
                    s1  = _mm_unpacklo_epi16(s4, _mm_setzero_si128());
                    s4  = _mm_unpackhi_epi16(s4, _mm_setzero_si128());
                    s2  = _mm_unpacklo_epi16(s5, _mm_setzero_si128());
                    s5  = _mm_unpackhi_epi16(s5, _mm_setzero_si128());
                    s7  = _mm_unpacklo_epi16(s6, _mm_setzero_si128());
                    s6  = _mm_unpackhi_epi16(s6, _mm_setzero_si128());
                    s9  = _mm_add_epi32(s9, _mm_add_epi32(_mm_add_epi32(s0, s1), _mm_add_epi32(s2, s7)));
                    s10 = _mm_add_epi32(s10, _mm_add_epi32(_mm_add_epi32(s3, s4), _mm_add_epi32(s5, s6)));
                }
                UPDATE_BEST_PME_32(s9, 0, 0);
                UPDATE_BEST_PME_32(s9, 1, 0);
                UPDATE_BEST_PME_32(s9, 2, 0);
                UPDATE_BEST_PME_32(s9, 3, 0);
                UPDATE_BEST_PME_32(s10, 0, 4);
                UPDATE_BEST_PME_32(s10, 1, 4);
                UPDATE_BEST_PME_32(s10, 2, 4);
                UPDATE_BEST_PME_32(s10, 3, 4);
            }
            ref += search_step * ref_stride;
        }
    } break;

    default:
        svt_pme_sad_loop_kernel_c(mv_cost_params,
                                  src,
                                  src_stride,
                                  ref,
                                  ref_stride,
                                  block_height,
                                  block_width,
                                  &low_sum,
                                  &x_best,
                                  &y_best,
                                  search_position_start_x,
                                  search_position_start_y,
                                  search_area_width,
                                  search_area_height,
                                  search_step,
                                  mvx,
                                  mvy);
        break;
    }

    *best_cost = low_sum;
    *best_mvx  = x_best;
    *best_mvy  = y_best;
}

uint32_t svt_av1_compute4x_m_sad_sse4_1_intrin(const uint8_t* src, // input parameter, source samples Ptr
                                               uint32_t       src_stride, // input parameter, source stride
                                               const uint8_t* ref, // input parameter, reference samples Ptr
                                               uint32_t       ref_stride, // input parameter, reference stride
                                               uint32_t       height, // input parameter, block height (M)
                                               uint32_t       width) // input parameter, block width (N)
{
    uint32_t y = 0;
    (void)width;
    __m128i xmm_sad = _mm_setzero_si128();

    while (y + 4 <= height) {
        xmm_sad = _mm_add_epi16(xmm_sad,
                                _mm_sad_epu8(_mm_cvtsi32_si128(*(uint32_t*)src), _mm_cvtsi32_si128(*(uint32_t*)ref)));
        xmm_sad = _mm_add_epi16(xmm_sad,
                                _mm_sad_epu8(_mm_cvtsi32_si128(*(uint32_t*)(src + src_stride)),
                                             _mm_cvtsi32_si128(*(uint32_t*)(ref + ref_stride))));
        xmm_sad = _mm_add_epi16(xmm_sad,
                                _mm_sad_epu8(_mm_cvtsi32_si128(*(uint32_t*)(src + (src_stride << 1))),
                                             _mm_cvtsi32_si128(*(uint32_t*)(ref + (ref_stride << 1)))));
        xmm_sad = _mm_add_epi16(xmm_sad,
                                _mm_sad_epu8(_mm_cvtsi32_si128(*(uint32_t*)(src + 3 * src_stride)),
                                             _mm_cvtsi32_si128(*(uint32_t*)(ref + 3 * ref_stride))));
        src += (src_stride << 2);
        ref += (ref_stride << 2);
        y += 4;
    }

    while (y < height) {
        xmm_sad = _mm_add_epi16(xmm_sad,
                                _mm_sad_epu8(_mm_cvtsi32_si128(*(uint32_t*)src), _mm_cvtsi32_si128(*(uint32_t*)ref)));
        src += src_stride;
        ref += ref_stride;
        y++;
    }
    return _mm_cvtsi128_si32(xmm_sad);
}

uint32_t svt_av1_compute8x_m_sad_sse4_1_intrin(const uint8_t* src, // input parameter, source samples Ptr
                                               uint32_t       src_stride, // input parameter, source stride
                                               const uint8_t* ref, // input parameter, reference samples Ptr
                                               uint32_t       ref_stride, // input parameter, reference stride
                                               uint32_t       height, // input parameter, block height (M)
                                               uint32_t       width) // input parameter, block width (N)
{
    uint32_t y;
    (void)width;
    __m128i xmm_sad = _mm_setzero_si128();

    y = 0;
    while (y + 4 <= height) {
        xmm_sad = _mm_add_epi16(xmm_sad, _mm_sad_epu8(_mm_loadl_epi64((__m128i*)src), _mm_loadl_epi64((__m128i*)ref)));
        xmm_sad = _mm_add_epi16(
            xmm_sad,
            _mm_sad_epu8(_mm_loadl_epi64((__m128i*)(src + src_stride)), _mm_loadl_epi64((__m128i*)(ref + ref_stride))));
        xmm_sad = _mm_add_epi16(xmm_sad,
                                _mm_sad_epu8(_mm_loadl_epi64((__m128i*)(src + (src_stride << 1))),
                                             _mm_loadl_epi64((__m128i*)(ref + (ref_stride << 1)))));
        xmm_sad = _mm_add_epi16(xmm_sad,
                                _mm_sad_epu8(_mm_loadl_epi64((__m128i*)(src + 3 * src_stride)),
                                             _mm_loadl_epi64((__m128i*)(ref + 3 * ref_stride))));

        src += (src_stride << 2);
        ref += (ref_stride << 2);
        y += 4;
    }

    while (y < height) {
        xmm_sad = _mm_add_epi16(xmm_sad, _mm_sad_epu8(_mm_loadl_epi64((__m128i*)src), _mm_loadl_epi64((__m128i*)ref)));
        src += (src_stride);
        ref += (ref_stride);
        y++;
    }

    return _mm_cvtsi128_si32(xmm_sad);
}

uint32_t svt_av1_compute16x_m_sad_sse4_1_intrin(const uint8_t* src, // input parameter, source samples Ptr
                                                uint32_t       src_stride, // input parameter, source stride
                                                const uint8_t* ref, // input parameter, reference samples Ptr
                                                uint32_t       ref_stride, // input parameter, reference stride
                                                uint32_t       height, // input parameter, block height (M)
                                                uint32_t       width) // input parameter, block width (N)
{
    __m128i  xmm_sad = _mm_setzero_si128();
    uint32_t y       = height;
    (void)width;

    while (y >= 4) {
        xmm_sad = _mm_add_epi32(xmm_sad,
                                _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src)), _mm_loadu_si128((__m128i*)(ref))));
        xmm_sad = _mm_add_epi32(
            xmm_sad,
            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + src_stride)), _mm_loadu_si128((__m128i*)(ref + ref_stride))));
        xmm_sad = _mm_add_epi32(xmm_sad,
                                _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + (src_stride << 1))),
                                             _mm_loadu_si128((__m128i*)(ref + (ref_stride << 1)))));
        xmm_sad = _mm_add_epi32(xmm_sad,
                                _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + 3 * src_stride)),
                                             _mm_loadu_si128((__m128i*)(ref + 3 * ref_stride))));

        src += (src_stride << 2);
        ref += (ref_stride << 2);
        y -= 4;
    }

    while (y) {
        xmm_sad = _mm_add_epi32(xmm_sad,
                                _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src)), _mm_loadu_si128((__m128i*)(ref))));

        src += src_stride;
        ref += ref_stride;
        y -= 1;
    };

    xmm_sad = _mm_add_epi32(xmm_sad, _mm_srli_si128(xmm_sad, 8));
    return _mm_cvtsi128_si32(xmm_sad);
}

uint32_t svt_av1_compute24x_m_sad_sse4_1_intrin(const uint8_t* src, // input parameter, source samples Ptr
                                                uint32_t       src_stride, // input parameter, source stride
                                                const uint8_t* ref, // input parameter, reference samples Ptr
                                                uint32_t       ref_stride, // input parameter, reference stride
                                                uint32_t       height, // input parameter, block height (M)
                                                uint32_t       width) // input parameter, block width (N)
{
    __m128i  xmm_sad = _mm_setzero_si128();
    uint32_t y       = height;
    (void)width;

    while (y >= 4) {
        xmm_sad = _mm_add_epi32(xmm_sad,
                                _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src)), _mm_loadu_si128((__m128i*)(ref))));
        xmm_sad = _mm_add_epi32(
            xmm_sad, _mm_sad_epu8(_mm_loadl_epi64((__m128i*)(src + 16)), _mm_loadl_epi64((__m128i*)(ref + 16))));

        xmm_sad = _mm_add_epi32(
            xmm_sad,
            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + src_stride)), _mm_loadu_si128((__m128i*)(ref + ref_stride))));
        xmm_sad = _mm_add_epi32(xmm_sad,
                                _mm_sad_epu8(_mm_loadl_epi64((__m128i*)(src + src_stride + 16)),
                                             _mm_loadl_epi64((__m128i*)(ref + ref_stride + 16))));

        xmm_sad = _mm_add_epi32(xmm_sad,
                                _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + (src_stride << 1))),
                                             _mm_loadu_si128((__m128i*)(ref + (ref_stride << 1)))));
        xmm_sad = _mm_add_epi32(xmm_sad,
                                _mm_sad_epu8(_mm_loadl_epi64((__m128i*)(src + (src_stride << 1) + 16)),
                                             _mm_loadl_epi64((__m128i*)(ref + (ref_stride << 1) + 16))));

        xmm_sad = _mm_add_epi32(xmm_sad,
                                _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + 3 * src_stride)),
                                             _mm_loadu_si128((__m128i*)(ref + 3 * ref_stride))));
        xmm_sad = _mm_add_epi32(xmm_sad,
                                _mm_sad_epu8(_mm_loadl_epi64((__m128i*)(src + 3 * src_stride + 16)),
                                             _mm_loadl_epi64((__m128i*)(ref + 3 * ref_stride + 16))));

        src += (src_stride << 2);
        ref += (ref_stride << 2);
        y -= 4;
    }

    while (y) {
        xmm_sad = _mm_add_epi32(xmm_sad,
                                _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src)), _mm_loadu_si128((__m128i*)(ref))));
        xmm_sad = _mm_add_epi32(
            xmm_sad, _mm_sad_epu8(_mm_loadl_epi64((__m128i*)(src + 16)), _mm_loadl_epi64((__m128i*)(ref + 16))));

        src += src_stride;
        ref += ref_stride;
        y -= 1;
    };

    xmm_sad = _mm_add_epi32(xmm_sad, _mm_srli_si128(xmm_sad, 8));
    return _mm_cvtsi128_si32(xmm_sad);
}

uint32_t svt_av1_compute32x_m_sad_sse4_1_intrin(const uint8_t* src, // input parameter, source samples Ptr
                                                uint32_t       src_stride, // input parameter, source stride
                                                const uint8_t* ref, // input parameter, reference samples Ptr
                                                uint32_t       ref_stride, // input parameter, reference stride
                                                uint32_t       height, // input parameter, block height (M)
                                                uint32_t       width) // input parameter, block width (N)
{
    __m128i sad;
    (void)width;
    uint32_t y = height;

    sad = _mm_setzero_si128();

    while (y >= 4) {
        sad = _mm_add_epi32(sad, _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src)), _mm_loadu_si128((__m128i*)(ref))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + 16)), _mm_loadu_si128((__m128i*)(ref + 16))));

        sad = _mm_add_epi32(
            sad,
            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + src_stride)), _mm_loadu_si128((__m128i*)(ref + ref_stride))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + src_stride + 16)),
                                         _mm_loadu_si128((__m128i*)(ref + ref_stride + 16))));

        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + 2 * src_stride)),
                                         _mm_loadu_si128((__m128i*)(ref + 2 * ref_stride))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + 2 * src_stride + 16)),
                                         _mm_loadu_si128((__m128i*)(ref + 2 * ref_stride + 16))));

        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + (src_stride * 3))),
                                         _mm_loadu_si128((__m128i*)(ref + (ref_stride * 3)))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + (src_stride * 3) + 16)),
                                         _mm_loadu_si128((__m128i*)(ref + (ref_stride * 3) + 16))));

        src += 4 * src_stride;
        ref += 4 * ref_stride;
        y -= 4;
    }

    while (y) {
        sad = _mm_add_epi32(sad, _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src)), _mm_loadu_si128((__m128i*)(ref))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + 16)), _mm_loadu_si128((__m128i*)(ref + 16))));

        src += src_stride;
        ref += ref_stride;
        y -= 1;
    };

    sad = _mm_add_epi32(sad, _mm_srli_si128(sad, 8));
    return _mm_cvtsi128_si32(sad);
}

uint32_t svt_av1_compute40x_m_sad_sse4_1_intrin(const uint8_t* src, // input parameter, source samples Ptr
                                                uint32_t       src_stride, // input parameter, source stride
                                                const uint8_t* ref, // input parameter, reference samples Ptr
                                                uint32_t       ref_stride, // input parameter, reference stride
                                                uint32_t       height, // input parameter, block height (M)
                                                uint32_t       width) // input parameter, block width (N)
{
    __m128i sad;
    (void)width;
    uint32_t y = height;

    sad = _mm_setzero_si128();

    while (y >= 4) {
        sad = _mm_add_epi32(sad, _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src)), _mm_loadu_si128((__m128i*)(ref))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + 16)), _mm_loadu_si128((__m128i*)(ref + 16))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadl_epi64((__m128i*)(src + 32)), _mm_loadl_epi64((__m128i*)(ref + 32))));

        sad = _mm_add_epi32(
            sad,
            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + src_stride)), _mm_loadu_si128((__m128i*)(ref + ref_stride))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + src_stride + 16)),
                                         _mm_loadu_si128((__m128i*)(ref + ref_stride + 16))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadl_epi64((__m128i*)(src + src_stride + 32)),
                                         _mm_loadl_epi64((__m128i*)(ref + ref_stride + 32))));

        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + 2 * src_stride)),
                                         _mm_loadu_si128((__m128i*)(ref + 2 * ref_stride))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + 2 * src_stride + 16)),
                                         _mm_loadu_si128((__m128i*)(ref + 2 * ref_stride + 16))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadl_epi64((__m128i*)(src + 2 * src_stride + 32)),
                                         _mm_loadl_epi64((__m128i*)(ref + 2 * ref_stride + 32))));

        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + (src_stride * 3))),
                                         _mm_loadu_si128((__m128i*)(ref + (ref_stride * 3)))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + (src_stride * 3) + 16)),
                                         _mm_loadu_si128((__m128i*)(ref + (ref_stride * 3) + 16))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadl_epi64((__m128i*)(src + (src_stride * 3) + 32)),
                                         _mm_loadl_epi64((__m128i*)(ref + (ref_stride * 3) + 32))));

        src += 4 * src_stride;
        ref += 4 * ref_stride;
        y -= 4;
    }

    while (y) {
        sad = _mm_add_epi32(sad, _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src)), _mm_loadu_si128((__m128i*)(ref))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + 16)), _mm_loadu_si128((__m128i*)(ref + 16))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadl_epi64((__m128i*)(src + 32)), _mm_loadl_epi64((__m128i*)(ref + 32))));

        src += src_stride;
        ref += ref_stride;
        y -= 1;
    };

    sad = _mm_add_epi32(sad, _mm_srli_si128(sad, 8));
    return _mm_cvtsi128_si32(sad);
}

uint32_t svt_av1_compute48x_m_sad_sse4_1_intrin(const uint8_t* src, // input parameter, source samples Ptr
                                                uint32_t       src_stride, // input parameter, source stride
                                                const uint8_t* ref, // input parameter, reference samples Ptr
                                                uint32_t       ref_stride, // input parameter, reference stride
                                                uint32_t       height, // input parameter, block height (M)
                                                uint32_t       width) // input parameter, block width (N)
{
    __m128i sad;
    (void)width;
    uint32_t y = height;

    sad = _mm_setzero_si128();

    while (y >= 4) {
        sad = _mm_add_epi32(sad, _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src)), _mm_loadu_si128((__m128i*)(ref))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + 16)), _mm_loadu_si128((__m128i*)(ref + 16))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + 32)), _mm_loadu_si128((__m128i*)(ref + 32))));

        sad = _mm_add_epi32(
            sad,
            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + src_stride)), _mm_loadu_si128((__m128i*)(ref + ref_stride))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + src_stride + 16)),
                                         _mm_loadu_si128((__m128i*)(ref + ref_stride + 16))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + src_stride + 32)),
                                         _mm_loadu_si128((__m128i*)(ref + ref_stride + 32))));

        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + 2 * src_stride)),
                                         _mm_loadu_si128((__m128i*)(ref + 2 * ref_stride))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + 2 * src_stride + 16)),
                                         _mm_loadu_si128((__m128i*)(ref + 2 * ref_stride + 16))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + 2 * src_stride + 32)),
                                         _mm_loadu_si128((__m128i*)(ref + 2 * ref_stride + 32))));

        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + (src_stride * 3))),
                                         _mm_loadu_si128((__m128i*)(ref + (ref_stride * 3)))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + (src_stride * 3) + 16)),
                                         _mm_loadu_si128((__m128i*)(ref + (ref_stride * 3) + 16))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + (src_stride * 3) + 32)),
                                         _mm_loadu_si128((__m128i*)(ref + (ref_stride * 3) + 32))));

        src += 4 * src_stride;
        ref += 4 * ref_stride;
        y -= 4;
    }

    while (y) {
        sad = _mm_add_epi32(sad, _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src)), _mm_loadu_si128((__m128i*)(ref))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + 16)), _mm_loadu_si128((__m128i*)(ref + 16))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + 32)), _mm_loadu_si128((__m128i*)(ref + 32))));

        src += src_stride;
        ref += ref_stride;
        y -= 1;
    };

    sad = _mm_add_epi32(sad, _mm_srli_si128(sad, 8));
    return _mm_cvtsi128_si32(sad);
}

uint32_t svt_av1_compute56x_m_sad_sse4_1_intrin(const uint8_t* src, // input parameter, source samples Ptr
                                                uint32_t       src_stride, // input parameter, source stride
                                                const uint8_t* ref, // input parameter, reference samples Ptr
                                                uint32_t       ref_stride, // input parameter, reference stride
                                                uint32_t       height, // input parameter, block height (M)
                                                uint32_t       width) // input parameter, block width (N)
{
    __m128i sad;
    (void)width;
    uint32_t y = height;

    sad = _mm_setzero_si128();

    while (y >= 4) {
        sad = _mm_add_epi32(sad, _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src)), _mm_loadu_si128((__m128i*)(ref))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + 16)), _mm_loadu_si128((__m128i*)(ref + 16))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + 32)), _mm_loadu_si128((__m128i*)(ref + 32))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadl_epi64((__m128i*)(src + 48)), _mm_loadl_epi64((__m128i*)(ref + 48))));

        sad = _mm_add_epi32(
            sad,
            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + src_stride)), _mm_loadu_si128((__m128i*)(ref + ref_stride))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + src_stride + 16)),
                                         _mm_loadu_si128((__m128i*)(ref + ref_stride + 16))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + src_stride + 32)),
                                         _mm_loadu_si128((__m128i*)(ref + ref_stride + 32))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadl_epi64((__m128i*)(src + src_stride + 48)),
                                         _mm_loadl_epi64((__m128i*)(ref + ref_stride + 48))));

        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + 2 * src_stride)),
                                         _mm_loadu_si128((__m128i*)(ref + 2 * ref_stride))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + 2 * src_stride + 16)),
                                         _mm_loadu_si128((__m128i*)(ref + 2 * ref_stride + 16))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + 2 * src_stride + 32)),
                                         _mm_loadu_si128((__m128i*)(ref + 2 * ref_stride + 32))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadl_epi64((__m128i*)(src + 2 * src_stride + 48)),
                                         _mm_loadl_epi64((__m128i*)(ref + 2 * ref_stride + 48))));

        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + (src_stride * 3))),
                                         _mm_loadu_si128((__m128i*)(ref + (ref_stride * 3)))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + (src_stride * 3) + 16)),
                                         _mm_loadu_si128((__m128i*)(ref + (ref_stride * 3) + 16))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + (src_stride * 3) + 32)),
                                         _mm_loadu_si128((__m128i*)(ref + (ref_stride * 3) + 32))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadl_epi64((__m128i*)(src + (src_stride * 3) + 48)),
                                         _mm_loadl_epi64((__m128i*)(ref + (ref_stride * 3) + 48))));

        src += 4 * src_stride;
        ref += 4 * ref_stride;
        y -= 4;
    }

    while (y) {
        sad = _mm_add_epi32(sad, _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src)), _mm_loadu_si128((__m128i*)(ref))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + 16)), _mm_loadu_si128((__m128i*)(ref + 16))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + 32)), _mm_loadu_si128((__m128i*)(ref + 32))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadl_epi64((__m128i*)(src + 48)), _mm_loadl_epi64((__m128i*)(ref + 48))));

        src += src_stride;
        ref += ref_stride;
        y -= 1;
    };

    sad = _mm_add_epi32(sad, _mm_srli_si128(sad, 8));
    return _mm_cvtsi128_si32(sad);
}

uint32_t svt_av1_compute64x_m_sad_sse4_1_intrin(const uint8_t* src, // input parameter, source samples Ptr
                                                uint32_t       src_stride, // input parameter, source stride
                                                const uint8_t* ref, // input parameter, reference samples Ptr
                                                uint32_t       ref_stride, // input parameter, reference stride
                                                uint32_t       height, // input parameter, block height (M)
                                                uint32_t       width) // input parameter, block width (N)
{
    __m128i sad;
    (void)width;
    uint32_t y;

    sad = _mm_setzero_si128();

    for (y = 0; y < height; y += 4) {
        sad = _mm_add_epi32(sad, _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src)), _mm_loadu_si128((__m128i*)(ref))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + 16)), _mm_loadu_si128((__m128i*)(ref + 16))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + 32)), _mm_loadu_si128((__m128i*)(ref + 32))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + 48)), _mm_loadu_si128((__m128i*)(ref + 48))));

        sad = _mm_add_epi32(
            sad,
            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + src_stride)), _mm_loadu_si128((__m128i*)(ref + ref_stride))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + src_stride + 16)),
                                         _mm_loadu_si128((__m128i*)(ref + ref_stride + 16))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + src_stride + 32)),
                                         _mm_loadu_si128((__m128i*)(ref + ref_stride + 32))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + src_stride + 48)),
                                         _mm_loadu_si128((__m128i*)(ref + ref_stride + 48))));

        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + 2 * src_stride)),
                                         _mm_loadu_si128((__m128i*)(ref + 2 * ref_stride))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + 2 * src_stride + 16)),
                                         _mm_loadu_si128((__m128i*)(ref + 2 * ref_stride + 16))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + 2 * src_stride + 32)),
                                         _mm_loadu_si128((__m128i*)(ref + 2 * ref_stride + 32))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + 2 * src_stride + 48)),
                                         _mm_loadu_si128((__m128i*)(ref + 2 * ref_stride + 48))));

        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + 3 * src_stride)),
                                         _mm_loadu_si128((__m128i*)(ref + 3 * ref_stride))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + 3 * src_stride + 16)),
                                         _mm_loadu_si128((__m128i*)(ref + 3 * ref_stride + 16))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + 3 * src_stride + 32)),
                                         _mm_loadu_si128((__m128i*)(ref + 3 * ref_stride + 32))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + 3 * src_stride + 48)),
                                         _mm_loadu_si128((__m128i*)(ref + 3 * ref_stride + 48))));

        src += 4 * src_stride;
        ref += 4 * ref_stride;
    }

    sad = _mm_add_epi32(sad, _mm_srli_si128(sad, 8));
    return _mm_cvtsi128_si32(sad);
}

uint32_t svt_av1_compute128x_m_sad_sse4_1_intrin(const uint8_t* src, // input parameter, source samples Ptr
                                                 uint32_t       src_stride, // input parameter, source stride
                                                 const uint8_t* ref, // input parameter, reference samples Ptr
                                                 uint32_t       ref_stride, // input parameter, reference stride
                                                 uint32_t       height, // input parameter, block height (M)
                                                 uint32_t       width) // input parameter, block width (N)
{
    __m128i sad;
    (void)width;
    uint32_t y;

    sad = _mm_setzero_si128();

    for (y = 0; y < height; y += 4) {
        sad = _mm_add_epi32(sad, _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src)), _mm_loadu_si128((__m128i*)(ref))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + 16)), _mm_loadu_si128((__m128i*)(ref + 16))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + 32)), _mm_loadu_si128((__m128i*)(ref + 32))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + 48)), _mm_loadu_si128((__m128i*)(ref + 48))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + 64)), _mm_loadu_si128((__m128i*)(ref + 64))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + 80)), _mm_loadu_si128((__m128i*)(ref + 80))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + 96)), _mm_loadu_si128((__m128i*)(ref + 96))));
        sad = _mm_add_epi32(
            sad, _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + 112)), _mm_loadu_si128((__m128i*)(ref + 112))));

        sad = _mm_add_epi32(
            sad,
            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + src_stride)), _mm_loadu_si128((__m128i*)(ref + ref_stride))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + src_stride + 16)),
                                         _mm_loadu_si128((__m128i*)(ref + ref_stride + 16))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + src_stride + 32)),
                                         _mm_loadu_si128((__m128i*)(ref + ref_stride + 32))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + src_stride + 48)),
                                         _mm_loadu_si128((__m128i*)(ref + ref_stride + 48))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + src_stride + 64)),
                                         _mm_loadu_si128((__m128i*)(ref + ref_stride + 64))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + src_stride + 80)),
                                         _mm_loadu_si128((__m128i*)(ref + ref_stride + 80))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + src_stride + 96)),
                                         _mm_loadu_si128((__m128i*)(ref + ref_stride + 96))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + src_stride + 112)),
                                         _mm_loadu_si128((__m128i*)(ref + ref_stride + 112))));

        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + 2 * src_stride)),
                                         _mm_loadu_si128((__m128i*)(ref + 2 * ref_stride))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + 2 * src_stride + 16)),
                                         _mm_loadu_si128((__m128i*)(ref + 2 * ref_stride + 16))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + 2 * src_stride + 32)),
                                         _mm_loadu_si128((__m128i*)(ref + 2 * ref_stride + 32))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + 2 * src_stride + 48)),
                                         _mm_loadu_si128((__m128i*)(ref + 2 * ref_stride + 48))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + 2 * src_stride + 64)),
                                         _mm_loadu_si128((__m128i*)(ref + 2 * ref_stride + 64))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + 2 * src_stride + 80)),
                                         _mm_loadu_si128((__m128i*)(ref + 2 * ref_stride + 80))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + 2 * src_stride + 96)),
                                         _mm_loadu_si128((__m128i*)(ref + 2 * ref_stride + 96))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + 2 * src_stride + 112)),
                                         _mm_loadu_si128((__m128i*)(ref + 2 * ref_stride + 112))));

        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + 3 * src_stride)),
                                         _mm_loadu_si128((__m128i*)(ref + 3 * ref_stride))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + 3 * src_stride + 16)),
                                         _mm_loadu_si128((__m128i*)(ref + 3 * ref_stride + 16))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + 3 * src_stride + 32)),
                                         _mm_loadu_si128((__m128i*)(ref + 3 * ref_stride + 32))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + 3 * src_stride + 48)),
                                         _mm_loadu_si128((__m128i*)(ref + 3 * ref_stride + 48))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + 3 * src_stride + 64)),
                                         _mm_loadu_si128((__m128i*)(ref + 3 * ref_stride + 64))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + 3 * src_stride + 80)),
                                         _mm_loadu_si128((__m128i*)(ref + 3 * ref_stride + 80))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + 3 * src_stride + 96)),
                                         _mm_loadu_si128((__m128i*)(ref + 3 * ref_stride + 96))));
        sad = _mm_add_epi32(sad,
                            _mm_sad_epu8(_mm_loadu_si128((__m128i*)(src + 3 * src_stride + 112)),
                                         _mm_loadu_si128((__m128i*)(ref + 3 * ref_stride + 112))));

        src += 4 * src_stride;
        ref += 4 * ref_stride;
    }

    sad = _mm_add_epi32(sad, _mm_srli_si128(sad, 8));
    return _mm_cvtsi128_si32(sad);
}

uint32_t svt_nxm_sad_kernel_helper_sse4_1(const uint8_t* src, uint32_t src_stride, const uint8_t* ref,
                                          uint32_t ref_stride, uint32_t height, uint32_t width) {
    uint32_t nxm_sad = 0;

    switch (width) {
    case 4:
        nxm_sad = svt_av1_compute4x_m_sad_sse4_1_intrin(src, src_stride, ref, ref_stride, height, width);
        break;
    case 8:
        nxm_sad = svt_av1_compute8x_m_sad_sse4_1_intrin(src, src_stride, ref, ref_stride, height, width);
        break;
    case 16:
        nxm_sad = svt_av1_compute16x_m_sad_sse4_1_intrin(src, src_stride, ref, ref_stride, height, width);
        break;
    case 24:
        nxm_sad = svt_av1_compute24x_m_sad_sse4_1_intrin(src, src_stride, ref, ref_stride, height, width);
        break;
    case 32:
        nxm_sad = svt_av1_compute32x_m_sad_sse4_1_intrin(src, src_stride, ref, ref_stride, height, width);
        break;
    case 40:
        nxm_sad = svt_av1_compute40x_m_sad_sse4_1_intrin(src, src_stride, ref, ref_stride, height, width);
        break;
    case 48:
        nxm_sad = svt_av1_compute48x_m_sad_sse4_1_intrin(src, src_stride, ref, ref_stride, height, width);
        break;
    case 56:
        nxm_sad = svt_av1_compute56x_m_sad_sse4_1_intrin(src, src_stride, ref, ref_stride, height, width);
        break;
    case 64:
        nxm_sad = svt_av1_compute64x_m_sad_sse4_1_intrin(src, src_stride, ref, ref_stride, height, width);
        break;
    case 128:
        nxm_sad = svt_av1_compute128x_m_sad_sse4_1_intrin(src, src_stride, ref, ref_stride, height, width);
        break;
    default:
        nxm_sad = svt_nxm_sad_kernel_helper_c(src, src_stride, ref, ref_stride, height, width);
    }

    return nxm_sad;
}

void svt_ext_eight_sad_calculation_32x32_64x64_sse4_1(const uint32_t p_sad16x16[16][8], uint32_t* p_best_sad_32x32,
                                                      uint32_t* p_best_sad_64x64, uint32_t* p_best_mv32x32,
                                                      uint32_t* p_best_mv64x64, uint32_t mv,
                                                      uint32_t p_sad32x32[4][8]) {
    __m128i       tmp0     = _mm_add_epi32(_mm_loadu_si128((__m128i const*)p_sad16x16[0]),
                                 _mm_loadu_si128((__m128i const*)p_sad16x16[1]));
    __m128i       tmp1     = _mm_add_epi32(_mm_loadu_si128((__m128i const*)p_sad16x16[2]),
                                 _mm_loadu_si128((__m128i const*)p_sad16x16[3]));
    const __m128i sad32_a1 = _mm_add_epi32(tmp0, tmp1);
    _mm_storeu_si128((__m128i*)p_sad32x32[0], sad32_a1);

    tmp0                   = _mm_add_epi32(_mm_loadu_si128((__m128i const*)(p_sad16x16[0] + 4)),
                         _mm_loadu_si128((__m128i const*)(p_sad16x16[1] + 4)));
    tmp1                   = _mm_add_epi32(_mm_loadu_si128((__m128i const*)(p_sad16x16[2] + 4)),
                         _mm_loadu_si128((__m128i const*)(p_sad16x16[3] + 4)));
    const __m128i sad32_a2 = _mm_add_epi32(tmp0, tmp1);
    _mm_storeu_si128((__m128i*)(p_sad32x32[0] + 4), sad32_a2);

    tmp0                   = _mm_add_epi32(_mm_loadu_si128((__m128i const*)p_sad16x16[4]),
                         _mm_loadu_si128((__m128i const*)p_sad16x16[5]));
    tmp1                   = _mm_add_epi32(_mm_loadu_si128((__m128i const*)p_sad16x16[6]),
                         _mm_loadu_si128((__m128i const*)p_sad16x16[7]));
    const __m128i sad32_b1 = _mm_add_epi32(tmp0, tmp1);
    _mm_storeu_si128((__m128i*)p_sad32x32[1], sad32_b1);

    tmp0                   = _mm_add_epi32(_mm_loadu_si128((__m128i const*)(p_sad16x16[4] + 4)),
                         _mm_loadu_si128((__m128i const*)(p_sad16x16[5] + 4)));
    tmp1                   = _mm_add_epi32(_mm_loadu_si128((__m128i const*)(p_sad16x16[6] + 4)),
                         _mm_loadu_si128((__m128i const*)(p_sad16x16[7] + 4)));
    const __m128i sad32_b2 = _mm_add_epi32(tmp0, tmp1);
    _mm_storeu_si128((__m128i*)(p_sad32x32[1] + 4), sad32_b2);

    tmp0                   = _mm_add_epi32(_mm_loadu_si128((__m128i const*)p_sad16x16[8]),
                         _mm_loadu_si128((__m128i const*)p_sad16x16[9]));
    tmp1                   = _mm_add_epi32(_mm_loadu_si128((__m128i const*)p_sad16x16[10]),
                         _mm_loadu_si128((__m128i const*)p_sad16x16[11]));
    const __m128i sad32_c1 = _mm_add_epi32(tmp0, tmp1);
    _mm_storeu_si128((__m128i*)p_sad32x32[2], sad32_c1);

    tmp0                   = _mm_add_epi32(_mm_loadu_si128((__m128i const*)(p_sad16x16[8] + 4)),
                         _mm_loadu_si128((__m128i const*)(p_sad16x16[9] + 4)));
    tmp1                   = _mm_add_epi32(_mm_loadu_si128((__m128i const*)(p_sad16x16[10] + 4)),
                         _mm_loadu_si128((__m128i const*)(p_sad16x16[11] + 4)));
    const __m128i sad32_c2 = _mm_add_epi32(tmp0, tmp1);
    _mm_storeu_si128((__m128i*)(p_sad32x32[2] + 4), sad32_c2);

    tmp0                   = _mm_add_epi32(_mm_loadu_si128((__m128i const*)p_sad16x16[12]),
                         _mm_loadu_si128((__m128i const*)p_sad16x16[13]));
    tmp1                   = _mm_add_epi32(_mm_loadu_si128((__m128i const*)p_sad16x16[14]),
                         _mm_loadu_si128((__m128i const*)p_sad16x16[15]));
    const __m128i sad32_d1 = _mm_add_epi32(tmp0, tmp1);
    _mm_storeu_si128((__m128i*)p_sad32x32[3], sad32_d1);

    tmp0                   = _mm_add_epi32(_mm_loadu_si128((__m128i const*)(p_sad16x16[12] + 4)),
                         _mm_loadu_si128((__m128i const*)(p_sad16x16[13] + 4)));
    tmp1                   = _mm_add_epi32(_mm_loadu_si128((__m128i const*)(p_sad16x16[14] + 4)),
                         _mm_loadu_si128((__m128i const*)(p_sad16x16[15] + 4)));
    const __m128i sad32_d2 = _mm_add_epi32(tmp0, tmp1);
    _mm_storeu_si128((__m128i*)(p_sad32x32[3] + 4), sad32_d2);

    DECLARE_ALIGNED(32, uint32_t, p_sad64x64[8]);
    tmp0 = _mm_add_epi32(sad32_a1, sad32_b1);
    tmp1 = _mm_add_epi32(sad32_c1, sad32_d1);
    _mm_storeu_si128((__m128i*)p_sad64x64, _mm_add_epi32(tmp0, tmp1));
    tmp0 = _mm_add_epi32(sad32_a2, sad32_b2);
    tmp1 = _mm_add_epi32(sad32_c2, sad32_d2);
    _mm_storeu_si128((__m128i*)(p_sad64x64 + 4), _mm_add_epi32(tmp0, tmp1));

    DECLARE_ALIGNED(32, uint32_t, computed_idx[8]);
    __m128i       search_idx = _mm_setr_epi32(0, 1, 2, 3);
    const __m128i mv_sse     = _mm_set1_epi32(mv);
    __m128i       new_mv_sse = _mm_add_epi32(search_idx, mv_sse);
    new_mv_sse               = _mm_and_si128(new_mv_sse, _mm_set1_epi32(0xffff));
    _mm_storeu_si128((__m128i*)computed_idx,
                     _mm_or_si128(new_mv_sse, _mm_and_si128(mv_sse, _mm_set1_epi32(0xffff0000))));

    search_idx = _mm_setr_epi32(4, 5, 6, 7);
    new_mv_sse = _mm_add_epi32(search_idx, mv_sse);
    new_mv_sse = _mm_and_si128(new_mv_sse, _mm_set1_epi32(0xffff));
    _mm_storeu_si128((__m128i*)(computed_idx + 4),
                     _mm_or_si128(new_mv_sse, _mm_and_si128(mv_sse, _mm_set1_epi32(0xffff0000))));

    for (int i = 0; i < 4; i++) {
        for (int j = 0; j < 8; j++) {
            if (p_sad32x32[i][j] < p_best_sad_32x32[i]) {
                p_best_sad_32x32[i] = p_sad32x32[i][j];
                p_best_mv32x32[i]   = computed_idx[j];
            }
        }
    }

    for (int j = 0; j < 8; j++) {
        if (p_sad64x64[j] < p_best_sad_64x64[0]) {
            p_best_sad_64x64[0] = p_sad64x64[j];
            p_best_mv64x64[0]   = computed_idx[j];
        }
    }
}
