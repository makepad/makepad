/*
* Copyright(c) 2021 Intel Corporation
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

/* clang-format off */
DECLARE_ALIGNED(32, static const uint8_t, bilinear_filters_avx2[512]) = {
  16,  0, 16,  0, 16,  0, 16,  0, 16,  0, 16,  0, 16,  0, 16,  0,
  16,  0, 16,  0, 16,  0, 16,  0, 16,  0, 16,  0, 16,  0, 16,  0,
  14,  2, 14,  2, 14,  2, 14,  2, 14,  2, 14,  2, 14,  2, 14,  2,
  14,  2, 14,  2, 14,  2, 14,  2, 14,  2, 14,  2, 14,  2, 14,  2,
  12,  4, 12,  4, 12,  4, 12,  4, 12,  4, 12,  4, 12,  4, 12,  4,
  12,  4, 12,  4, 12,  4, 12,  4, 12,  4, 12,  4, 12,  4, 12,  4,
  10,  6, 10,  6, 10,  6, 10,  6, 10,  6, 10,  6, 10,  6, 10,  6,
  10,  6, 10,  6, 10,  6, 10,  6, 10,  6, 10,  6, 10,  6, 10,  6,
   8,  8,  8,  8,  8,  8,  8,  8,  8,  8,  8,  8,  8,  8,  8,  8,
   8,  8,  8,  8,  8,  8,  8,  8,  8,  8,  8,  8,  8,  8,  8,  8,
   6, 10,  6, 10,  6, 10,  6, 10,  6, 10,  6, 10,  6, 10,  6, 10,
   6, 10,  6, 10,  6, 10,  6, 10,  6, 10,  6, 10,  6, 10,  6, 10,
   4, 12,  4, 12,  4, 12,  4, 12,  4, 12,  4, 12,  4, 12,  4, 12,
   4, 12,  4, 12,  4, 12,  4, 12,  4, 12,  4, 12,  4, 12,  4, 12,
   2, 14,  2, 14,  2, 14,  2, 14,  2, 14,  2, 14,  2, 14,  2, 14,
   2, 14,  2, 14,  2, 14,  2, 14,  2, 14,  2, 14,  2, 14,  2, 14,
};
/* clang-format on */

#define FILTER_SRC(filter)                                 \
    /* filter the source */                                \
    exp_src_lo = _mm512_maddubs_epi16(exp_src_lo, filter); \
    exp_src_hi = _mm512_maddubs_epi16(exp_src_hi, filter); \
                                                           \
    /* add 8 to source */                                  \
    exp_src_lo = _mm512_add_epi16(exp_src_lo, pw8);        \
    exp_src_hi = _mm512_add_epi16(exp_src_hi, pw8);        \
                                                           \
    /* divide source by 16 */                              \
    exp_src_lo = _mm512_srai_epi16(exp_src_lo, 4);         \
    exp_src_hi = _mm512_srai_epi16(exp_src_hi, 4);

#define MERGE_WITH_SRC(src_reg, reg)                 \
    exp_src_lo = _mm512_unpacklo_epi8(src_reg, reg); \
    exp_src_hi = _mm512_unpackhi_epi8(src_reg, reg);

#define LOAD_SRC_DST                                     \
    /* load source and destination */                    \
    src_reg = _mm512_loadu_si512((__m512i const*)(src)); \
    dst_reg = _mm512_loadu_si512((__m512i const*)(dst));

#define AVG_NEXT_SRC(src_reg, size_stride)                                  \
    src_next_reg = _mm512_loadu_si512((__m512i const*)(src + size_stride)); \
    /* average between current and next stride source */                    \
    src_reg = _mm512_avg_epu8(src_reg, src_next_reg);

#define MERGE_NEXT_SRC(src_reg, size_stride)                                \
    src_next_reg = _mm512_loadu_si512((__m512i const*)(src + size_stride)); \
    MERGE_WITH_SRC(src_reg, src_next_reg)

#define CALC_SUM_SSE_INSIDE_LOOP                            \
    /* expand each byte to 2 bytes */                       \
    exp_dst_lo = _mm512_unpacklo_epi8(dst_reg, zero_reg);   \
    exp_dst_hi = _mm512_unpackhi_epi8(dst_reg, zero_reg);   \
    /* source - dest */                                     \
    exp_src_lo = _mm512_sub_epi16(exp_src_lo, exp_dst_lo);  \
    exp_src_hi = _mm512_sub_epi16(exp_src_hi, exp_dst_hi);  \
    /* caculate sum */                                      \
    sum_reg    = _mm512_add_epi16(sum_reg, exp_src_lo);     \
    exp_src_lo = _mm512_madd_epi16(exp_src_lo, exp_src_lo); \
    sum_reg    = _mm512_add_epi16(sum_reg, exp_src_hi);     \
    exp_src_hi = _mm512_madd_epi16(exp_src_hi, exp_src_hi); \
    /* calculate sse */                                     \
    sse_reg = _mm512_add_epi32(sse_reg, exp_src_lo);        \
    sse_reg = _mm512_add_epi32(sse_reg, exp_src_hi);

// final calculation to sse
#define CALC_SSE                                                                                             \
    sse_reg_hi   = _mm512_bsrli_epi128(sse_reg, 8);                                                          \
    sse_reg      = _mm512_add_epi32(sse_reg, sse_reg_hi);                                                    \
    sse_reg_hi   = _mm512_bsrli_epi128(sse_reg, 4);                                                          \
    sse_reg      = _mm512_add_epi32(sse_reg, sse_reg_hi);                                                    \
    sse_tmp256   = _mm256_add_epi32(_mm512_castsi512_si256(sse_reg), _mm512_extracti64x4_epi64(sse_reg, 1)); \
    *((int*)sse) = _mm_cvtsi128_si32(_mm256_castsi256_si128(sse_tmp256)) +                                   \
        _mm_cvtsi128_si32(_mm256_extractf128_si256(sse_tmp256, 1));

// final calculation to sum
#define CALC_SUM                                                                                           \
    sum_reg_lo = _mm512_cvtepi16_epi32(_mm512_castsi512_si256(sum_reg));                                   \
    sum_reg_hi = _mm512_cvtepi16_epi32(_mm512_extracti64x4_epi64(sum_reg, 1));                             \
    sum_reg    = _mm512_add_epi32(sum_reg_lo, sum_reg_hi);                                                 \
    sum_reg_hi = _mm512_bsrli_epi128(sum_reg, 8);                                                          \
    sum_reg    = _mm512_add_epi32(sum_reg, sum_reg_hi);                                                    \
    sum_reg_hi = _mm512_bsrli_epi128(sum_reg, 4);                                                          \
    sum_reg    = _mm512_add_epi32(sum_reg, sum_reg_hi);                                                    \
    sum_tmp256 = _mm256_add_epi32(_mm512_castsi512_si256(sum_reg), _mm512_extracti64x4_epi64(sum_reg, 1)); \
    sum        = _mm_cvtsi128_si32(_mm256_castsi256_si128(sum_tmp256)) +                                   \
        _mm_cvtsi128_si32(_mm256_extractf128_si256(sum_tmp256, 1));

// Functions related to sub pixel variance width 16
#define LOAD_SRC_DST_INSERT(src_stride, dst_stride)                                           \
    /* load source and destination of 2 rows and insert*/                                     \
    src_reg = _mm512_inserti64x4(_mm512_castsi256_si512(_mm256_loadu_si256((__m256i*)(src))), \
                                 _mm256_loadu_si256((__m256i*)(src + src_stride)),            \
                                 1);                                                          \
    dst_reg = _mm512_inserti64x4(_mm512_castsi256_si512(_mm256_loadu_si256((__m256i*)(dst))), \
                                 _mm256_loadu_si256((__m256i*)(dst + dst_stride)),            \
                                 1);

#define AVG_NEXT_SRC_INSERT(src_reg, size_stride)                                                                \
    src_next_reg = _mm512_inserti64x4(_mm512_castsi256_si512(_mm256_loadu_si256((__m256i*)(src + size_stride))), \
                                      _mm256_loadu_si256((__m256i*)(src + (size_stride << 1))),                  \
                                      1);                                                                        \
    /* average between current and next stride source */                                                         \
    src_reg = _mm512_avg_epu8(src_reg, src_next_reg);

#define MERGE_NEXT_SRC_INSERT(src_reg, size_stride)                                                              \
    src_next_reg = _mm512_inserti64x4(_mm512_castsi256_si512(_mm256_loadu_si256((__m256i*)(src + size_stride))), \
                                      _mm256_loadu_si256((__m256i*)(src + (src_stride + size_stride))),          \
                                      1);                                                                        \
    MERGE_WITH_SRC(src_reg, src_next_reg)

#define LOAD_SRC_NEXT_BYTE_INSERT                                                                      \
    /* load source and another source from next row   */                                               \
    src_reg = _mm512_inserti64x4(_mm512_castsi256_si512(_mm256_loadu_si256((__m256i*)(src))),          \
                                 _mm256_loadu_si256((__m256i*)(src + src_stride)),                     \
                                 1);                                                                   \
    /* load source and next row source from 1 byte onwards   */                                        \
    src_next_reg = _mm512_inserti64x4(_mm512_castsi256_si512(_mm256_loadu_si256((__m256i*)(src + 1))), \
                                      _mm256_loadu_si256((__m256i*)(src + src_stride + 1)),            \
                                      1);

#define LOAD_DST_INSERT                                                                       \
    dst_reg = _mm512_inserti64x4(_mm512_castsi256_si512(_mm256_loadu_si256((__m256i*)(dst))), \
                                 _mm256_loadu_si256((__m256i*)(dst + dst_stride)),            \
                                 1);

#define LOAD_SRC_MERGE_256BIT(filter)                                   \
    __m256i src_reg_0     = _mm256_loadu_si256((__m256i*)(src));        \
    __m256i src_reg_1     = _mm256_loadu_si256((__m256i*)(src + 1));    \
    __m256i src_lo        = _mm256_unpacklo_epi8(src_reg_0, src_reg_1); \
    __m256i src_hi        = _mm256_unpackhi_epi8(src_reg_0, src_reg_1); \
    __m256i filter_256bit = _mm512_castsi512_si256(filter);             \
    __m256i pw8_256bit    = _mm512_extracti64x4_epi64(pw8, 1);

#define FILTER_SRC_256BIT(filter)                  \
    /* filter the source */                        \
    src_lo = _mm256_maddubs_epi16(src_lo, filter); \
    src_hi = _mm256_maddubs_epi16(src_hi, filter); \
                                                   \
    /* add 8 to source */                          \
    src_lo = _mm256_add_epi16(src_lo, pw8_256bit); \
    src_hi = _mm256_add_epi16(src_hi, pw8_256bit); \
                                                   \
    /* divide source by 16 */                      \
    src_lo = _mm256_srai_epi16(src_lo, 4);         \
    src_hi = _mm256_srai_epi16(src_hi, 4);

static INLINE unsigned int svt_aom_sub_pixel_variance64xh_avx512(const uint8_t* src, int src_stride, int x_offset,
                                                                 int y_offset, const uint8_t* dst, int dst_stride,
                                                                 int height, unsigned int* sse) {
    __m512i src_reg, dst_reg, exp_src_lo, exp_src_hi, exp_dst_lo, exp_dst_hi;
    __m512i sse_reg, sum_reg;
    __m512i zero_reg;
    __m512i sse_reg_hi, sum_reg_lo, sum_reg_hi;
    __m256i sse_tmp256, sum_tmp256;
    int     i, sum;
    sum_reg  = _mm512_set1_epi16(0);
    sse_reg  = _mm512_set1_epi16(0);
    zero_reg = _mm512_set1_epi16(0);

    // x_offset = 0 and y_offset = 0
    if (x_offset == 0) {
        if (y_offset == 0) {
            for (i = 0; i < height; i++) {
                LOAD_SRC_DST
                // expend each byte to 2 bytes
                MERGE_WITH_SRC(src_reg, zero_reg)
                CALC_SUM_SSE_INSIDE_LOOP
                src += src_stride;
                dst += dst_stride;
            }
            // x_offset = 0 and y_offset = 4
        } else if (y_offset == 4) {
            __m512i src_next_reg;
            for (i = 0; i < height; i++) {
                LOAD_SRC_DST
                AVG_NEXT_SRC(src_reg, src_stride)
                // expend each byte to 2 bytes
                MERGE_WITH_SRC(src_reg, zero_reg)
                CALC_SUM_SSE_INSIDE_LOOP
                src += src_stride;
                dst += dst_stride;
            }
            // x_offset = 0 and y_offset = bilin interpolation
        } else {
            __m512i filter, pw8, src_next_reg;
            __m256i filter256;
            y_offset <<= 5;
            filter256 = _mm256_load_si256((__m256i const*)(bilinear_filters_avx2 + y_offset));
            filter    = _mm512_castsi256_si512(filter256);
            filter    = _mm512_inserti64x4(filter, filter256, 1);
            pw8       = _mm512_set1_epi16(8);
            for (i = 0; i < height; i++) {
                LOAD_SRC_DST
                MERGE_NEXT_SRC(src_reg, src_stride)
                FILTER_SRC(filter)
                CALC_SUM_SSE_INSIDE_LOOP
                src += src_stride;
                dst += dst_stride;
            }
        }
        // x_offset = 4  and y_offset = 0
    } else if (x_offset == 4) {
        if (y_offset == 0) {
            __m512i src_next_reg;
            for (i = 0; i < height; i++) {
                LOAD_SRC_DST
                AVG_NEXT_SRC(src_reg, 1)
                // expand each byte to 2 bytes
                MERGE_WITH_SRC(src_reg, zero_reg)
                CALC_SUM_SSE_INSIDE_LOOP
                src += src_stride;
                dst += dst_stride;
            }
            // x_offset = 4  and y_offset = 4
        } else if (y_offset == 4) {
            __m512i src_next_reg, src_avg;
            // load source and another source starting from the next
            // following byte
            src_reg = _mm512_loadu_si512((__m512i const*)(src));
            AVG_NEXT_SRC(src_reg, 1)
            for (i = 0; i < height; i++) {
                src_avg = src_reg;
                src += src_stride;
                LOAD_SRC_DST
                AVG_NEXT_SRC(src_reg, 1)
                // average between previous average to current average
                src_avg = _mm512_avg_epu8(src_avg, src_reg);
                // expand each byte to 2 bytes
                MERGE_WITH_SRC(src_avg, zero_reg)
                // save current source average
                CALC_SUM_SSE_INSIDE_LOOP
                dst += dst_stride;
            }
            // x_offset = 4  and y_offset = bilin interpolation
        } else {
            __m512i filter, pw8, src_next_reg, src_avg;
            __m256i filter256;
            y_offset <<= 5;
            filter256 = _mm256_load_si256((__m256i const*)(bilinear_filters_avx2 + y_offset));
            filter    = _mm512_castsi256_si512(filter256);
            filter    = _mm512_inserti64x4(filter, filter256, 1);
            pw8       = _mm512_set1_epi16(8);
            // load source and another source starting from the next
            // following byte
            src_reg = _mm512_loadu_si512((__m512i const*)(src));
            AVG_NEXT_SRC(src_reg, 1)
            for (i = 0; i < height; i++) {
                // save current source average
                src_avg = src_reg;
                src += src_stride;
                LOAD_SRC_DST
                AVG_NEXT_SRC(src_reg, 1)
                MERGE_WITH_SRC(src_avg, src_reg)
                FILTER_SRC(filter)
                CALC_SUM_SSE_INSIDE_LOOP
                dst += dst_stride;
            }
        }
        // x_offset = bilin interpolation and y_offset = 0
    } else {
        if (y_offset == 0) {
            __m512i filter, pw8, src_next_reg;
            __m256i filter256;
            x_offset <<= 5;
            filter256 = _mm256_load_si256((__m256i const*)(bilinear_filters_avx2 + x_offset));
            filter    = _mm512_castsi256_si512(filter256);
            filter    = _mm512_inserti64x4(filter, filter256, 1);

            //Fiter fix
            pw8 = _mm512_set1_epi16(8);
            for (i = 0; i < height; i++) {
                LOAD_SRC_DST
                MERGE_NEXT_SRC(src_reg, 1)
                FILTER_SRC(filter)
                CALC_SUM_SSE_INSIDE_LOOP
                src += src_stride;
                dst += dst_stride;
            }
            // x_offset = bilin interpolation and y_offset = 4
        } else if (y_offset == 4) {
            __m512i filter, pw8, src_next_reg, src_pack;
            __m256i filter256;
            x_offset <<= 5;
            filter256 = _mm256_load_si256((__m256i const*)(bilinear_filters_avx2 + x_offset));
            filter    = _mm512_castsi256_si512(filter256);
            filter    = _mm512_inserti64x4(filter, filter256, 1);
            pw8       = _mm512_set1_epi16(8);
            src_reg   = _mm512_loadu_si512((__m512i const*)(src));
            MERGE_NEXT_SRC(src_reg, 1)
            FILTER_SRC(filter)
            // convert each 16 bit to 8 bit to each low and high lane source
            src_pack = _mm512_packus_epi16(exp_src_lo, exp_src_hi);
            for (i = 0; i < height; i++) {
                src += src_stride;
                LOAD_SRC_DST
                MERGE_NEXT_SRC(src_reg, 1)
                FILTER_SRC(filter)
                src_reg = _mm512_packus_epi16(exp_src_lo, exp_src_hi);
                // average between previous pack to the current
                src_pack = _mm512_avg_epu8(src_pack, src_reg);
                MERGE_WITH_SRC(src_pack, zero_reg)
                CALC_SUM_SSE_INSIDE_LOOP
                src_pack = src_reg;
                dst += dst_stride;
            }
            // x_offset = bilin interpolation and y_offset = bilin interpolation
        } else {
            __m512i xfilter, yfilter, pw8, src_next_reg, src_pack;
            __m256i xfilter256, yfilter256;
            x_offset <<= 5;
            y_offset <<= 5;

            xfilter256 = _mm256_load_si256((__m256i const*)(bilinear_filters_avx2 + x_offset));
            xfilter    = _mm512_castsi256_si512(xfilter256);
            xfilter    = _mm512_inserti64x4(xfilter, xfilter256, 1);

            yfilter256 = _mm256_load_si256((__m256i const*)(bilinear_filters_avx2 + y_offset));
            yfilter    = _mm512_castsi256_si512(yfilter256);
            yfilter    = _mm512_inserti64x4(yfilter, yfilter256, 1);

            pw8 = _mm512_set1_epi16(8);
            // load source and another source starting from the next
            // following byte
            src_reg = _mm512_loadu_si512((__m256i const*)(src));
            MERGE_NEXT_SRC(src_reg, 1)

            FILTER_SRC(xfilter)
            // convert each 16 bit to 8 bit to each low and high lane source
            src_pack = _mm512_packus_epi16(exp_src_lo, exp_src_hi);
            for (i = 0; i < height; i++) {
                src += src_stride;
                LOAD_SRC_DST
                MERGE_NEXT_SRC(src_reg, 1)
                FILTER_SRC(xfilter)
                src_reg = _mm512_packus_epi16(exp_src_lo, exp_src_hi);
                // merge previous pack to current pack source
                MERGE_WITH_SRC(src_pack, src_reg)
                // filter the source
                FILTER_SRC(yfilter)
                src_pack = src_reg;
                CALC_SUM_SSE_INSIDE_LOOP
                dst += dst_stride;
            }
        }
    }

    CALC_SSE
    CALC_SUM
    return sum;
}

static INLINE unsigned int svt_aom_sub_pixel_variance32xh_avx512(const uint8_t* src, int src_stride, int x_offset,
                                                                 int y_offset, const uint8_t* dst, int dst_stride,
                                                                 int height, unsigned int* sse) {
    __m512i src_reg, dst_reg, exp_src_lo, exp_src_hi, exp_dst_lo, exp_dst_hi;
    __m512i sse_reg, sum_reg, sse_reg_hi, sum_reg_lo, sum_reg_hi;
    __m512i zero_reg;
    __m256i sse_tmp256, sum_tmp256;
    int     i, sum;
    sum_reg  = _mm512_set1_epi16(0);
    sse_reg  = _mm512_set1_epi16(0);
    zero_reg = _mm512_set1_epi16(0);

    // x_offset = 0 and y_offset = 0
    if (x_offset == 0) {
        if (y_offset == 0) {
            for (i = 0; i < height; i += 2) {
                LOAD_SRC_DST_INSERT(src_stride, dst_stride)
                // expend each byte to 2 bytes
                MERGE_WITH_SRC(src_reg, zero_reg)
                CALC_SUM_SSE_INSIDE_LOOP
                src += (src_stride << 1);
                dst += (dst_stride << 1);
            }
            // x_offset = 0 and y_offset = 4
        } else if (y_offset == 4) {
            __m512i src_next_reg;
            for (i = 0; i < height; i += 2) {
                LOAD_SRC_DST_INSERT(src_stride, dst_stride)
                AVG_NEXT_SRC_INSERT(src_reg, src_stride)
                // expend each byte to 2 bytes
                MERGE_WITH_SRC(src_reg, zero_reg)
                CALC_SUM_SSE_INSIDE_LOOP
                src += (src_stride << 1);
                dst += (dst_stride << 1);
            }
            // x_offset = 0 and y_offset = bilin interpolation
        } else {
            __m512i filter, pw8, src_next_reg;
            __m256i yfilter256;
            y_offset <<= 5;
            yfilter256 = _mm256_load_si256((__m256i const*)(bilinear_filters_avx2 + y_offset));
            filter     = _mm512_castsi256_si512(yfilter256);
            filter     = _mm512_inserti64x4(filter, yfilter256, 1);
            pw8        = _mm512_set1_epi16(8);
            for (i = 0; i < height; i += 2) {
                LOAD_SRC_DST_INSERT(src_stride, dst_stride)
                MERGE_NEXT_SRC_INSERT(src_reg, src_stride)
                FILTER_SRC(filter)
                CALC_SUM_SSE_INSIDE_LOOP
                src += (src_stride << 1);
                dst += (dst_stride << 1);
            }
        }
        // x_offset = 4  and y_offset = 0
    } else if (x_offset == 4) {
        if (y_offset == 0) {
            __m512i src_next_reg;
            for (i = 0; i < height; i += 2) {
                LOAD_SRC_NEXT_BYTE_INSERT
                LOAD_DST_INSERT
                /* average between current and next stride source */
                src_reg = _mm512_avg_epu8(src_reg, src_next_reg);
                // expand each byte to 2 bytes
                MERGE_WITH_SRC(src_reg, zero_reg)
                CALC_SUM_SSE_INSIDE_LOOP
                src += (src_stride << 1);
                dst += (dst_stride << 1);
            }
            // x_offset = 4  and y_offset = 4
        } else if (y_offset == 4) {
            __m512i src_next_reg, src_avg, src_temp;
            // load and insert source and next row source
            LOAD_SRC_NEXT_BYTE_INSERT
            src_avg = _mm512_avg_epu8(src_reg, src_next_reg);
            src += src_stride << 1;
            for (i = 0; i < height - 2; i += 2) {
                LOAD_SRC_NEXT_BYTE_INSERT
                src_next_reg = _mm512_avg_epu8(src_reg, src_next_reg);
                src_temp     = _mm512_inserti64x4(_mm512_castsi256_si512(_mm512_extracti64x4_epi64(src_avg, 1)),
                                              _mm512_castsi512_si256(src_next_reg),
                                              1);
                src_temp     = _mm512_avg_epu8(src_avg, src_temp);
                LOAD_DST_INSERT
                // expand each byte to 2 bytes
                MERGE_WITH_SRC(src_temp, zero_reg)
                // save current source average
                src_avg = src_next_reg;
                CALC_SUM_SSE_INSIDE_LOOP
                dst += dst_stride << 1;
                src += src_stride << 1;
            }
            // last 2 rows processing happens here
            __m256i src_reg_0 = _mm256_loadu_si256((__m256i*)(src));
            __m256i src_reg_1 = _mm256_loadu_si256((__m256i*)(src + 1));
            src_reg_0         = _mm256_avg_epu8(src_reg_0, src_reg_1);
            src_next_reg      = _mm512_inserti64x4(
                _mm512_castsi256_si512(_mm512_extracti64x4_epi64(src_avg, 1)), src_reg_0, 1);
            LOAD_DST_INSERT
            src_avg = _mm512_avg_epu8(src_avg, src_next_reg);
            MERGE_WITH_SRC(src_avg, zero_reg)
            CALC_SUM_SSE_INSIDE_LOOP
        } else {
            // x_offset = 4  and y_offset = bilin interpolation
            __m512i filter, pw8, src_next_reg, src_avg, src_temp;
            __m256i yfilter256;
            y_offset <<= 5;
            yfilter256 = _mm256_load_si256((__m256i const*)(bilinear_filters_avx2 + y_offset));
            filter     = _mm512_castsi256_si512(yfilter256);
            filter     = _mm512_inserti64x4(filter, yfilter256, 1);
            pw8        = _mm512_set1_epi16(8);
            // load and insert source and next row source
            LOAD_SRC_NEXT_BYTE_INSERT
            src_avg = _mm512_avg_epu8(src_reg, src_next_reg);
            src += src_stride << 1;
            for (i = 0; i < height - 2; i += 2) {
                LOAD_SRC_NEXT_BYTE_INSERT
                src_next_reg = _mm512_avg_epu8(src_reg, src_next_reg);
                src_temp     = _mm512_inserti64x4(_mm512_castsi256_si512(_mm512_extracti64x4_epi64(src_avg, 1)),
                                              _mm512_castsi512_si256(src_next_reg),
                                              1);
                LOAD_DST_INSERT
                MERGE_WITH_SRC(src_avg, src_temp)
                // save current source average
                src_avg = src_next_reg;
                FILTER_SRC(filter)
                CALC_SUM_SSE_INSIDE_LOOP
                dst += dst_stride << 1;
                src += src_stride << 1;
            }
            // last 2 rows processing happens here
            __m256i src_reg_0 = _mm256_loadu_si256((__m256i*)(src));
            __m256i src_reg_1 = _mm256_loadu_si256((__m256i*)(src + 1));
            src_reg_0         = _mm256_avg_epu8(src_reg_0, src_reg_1);
            src_next_reg      = _mm512_inserti64x4(
                _mm512_castsi256_si512(_mm512_extracti64x4_epi64(src_avg, 1)), src_reg_0, 1);
            LOAD_DST_INSERT
            MERGE_WITH_SRC(src_avg, src_next_reg)
            FILTER_SRC(filter)
            CALC_SUM_SSE_INSIDE_LOOP
        }
        //  // x_offset = bilin interpolation and y_offset = 0
    } else {
        if (y_offset == 0) {
            __m512i filter, pw8, src_next_reg;
            __m256i xfilter256;
            x_offset <<= 5;
            xfilter256 = _mm256_load_si256((__m256i const*)(bilinear_filters_avx2 + x_offset));
            filter     = _mm512_castsi256_si512(xfilter256);
            filter     = _mm512_inserti64x4(filter, xfilter256, 1);
            pw8        = _mm512_set1_epi16(8);
            for (i = 0; i < height; i += 2) {
                LOAD_SRC_DST_INSERT(src_stride, dst_stride)
                MERGE_NEXT_SRC_INSERT(src_reg, 1)
                FILTER_SRC(filter)
                CALC_SUM_SSE_INSIDE_LOOP
                src += (src_stride << 1);
                dst += (dst_stride << 1);
            }
            // x_offset = bilin interpolation and y_offset = 4
        } else if (y_offset == 4) {
            __m512i filter, pw8, src_next_reg, src_pack;
            __m256i xfilter256;
            x_offset <<= 5;
            xfilter256 = _mm256_load_si256((__m256i const*)(bilinear_filters_avx2 + x_offset));
            filter     = _mm512_castsi256_si512(xfilter256);
            filter     = _mm512_inserti64x4(filter, xfilter256, 1);
            pw8        = _mm512_set1_epi16(8);
            // load and insert source and next row source
            LOAD_SRC_NEXT_BYTE_INSERT
            MERGE_WITH_SRC(src_reg, src_next_reg)
            FILTER_SRC(filter)
            // convert each 16 bit to 8 bit to each low and high lane source
            src_pack = _mm512_packus_epi16(exp_src_lo, exp_src_hi);
            src += src_stride << 1;
            for (i = 0; i < height - 2; i += 2) {
                LOAD_SRC_NEXT_BYTE_INSERT
                LOAD_DST_INSERT
                MERGE_WITH_SRC(src_reg, src_next_reg)
                FILTER_SRC(filter)
                src_reg      = _mm512_packus_epi16(exp_src_lo, exp_src_hi);
                src_next_reg = _mm512_inserti64x4(
                    _mm512_castsi256_si512(_mm512_extracti64x4_epi64(src_pack, 1)), _mm512_castsi512_si256(src_reg), 1);

                // average between previous pack to the current
                src_pack = _mm512_avg_epu8(src_pack, src_next_reg);
                MERGE_WITH_SRC(src_pack, zero_reg)
                CALC_SUM_SSE_INSIDE_LOOP
                src_pack = src_reg;
                src += src_stride << 1;
                dst += dst_stride << 1;
            }
            // last 2 rows processing happens here
            LOAD_SRC_MERGE_256BIT(filter)
            LOAD_DST_INSERT
            FILTER_SRC_256BIT(filter_256bit)
            src_reg_0    = _mm256_packus_epi16(src_lo, src_hi);
            src_next_reg = _mm512_inserti64x4(
                _mm512_castsi256_si512(_mm512_extracti64x4_epi64(src_pack, 1)), src_reg_0, 1);
            // average between previous pack to the current
            src_pack = _mm512_avg_epu8(src_pack, src_next_reg);
            MERGE_WITH_SRC(src_pack, zero_reg)
            CALC_SUM_SSE_INSIDE_LOOP
        } else {
            // x_offset = bilin interpolation and y_offset = bilin interpolation
            __m512i xfilter, yfilter, pw8, src_next_reg, src_pack;
            __m256i xfilter256, yfilter256;
            x_offset <<= 5;
            y_offset <<= 5;

            xfilter256 = _mm256_load_si256((__m256i const*)(bilinear_filters_avx2 + x_offset));
            xfilter    = _mm512_castsi256_si512(xfilter256);
            xfilter    = _mm512_inserti64x4(xfilter, xfilter256, 1);

            yfilter256 = _mm256_load_si256((__m256i const*)(bilinear_filters_avx2 + y_offset));
            yfilter    = _mm512_castsi256_si512(yfilter256);
            yfilter    = _mm512_inserti64x4(yfilter, yfilter256, 1);
            pw8        = _mm512_set1_epi16(8);

            // load and insert source and next row source
            LOAD_SRC_NEXT_BYTE_INSERT
            MERGE_WITH_SRC(src_reg, src_next_reg)
            FILTER_SRC(xfilter)
            // convert each 16 bit to 8 bit to each low and high lane source
            src_pack = _mm512_packus_epi16(exp_src_lo, exp_src_hi);
            src += src_stride << 1;
            for (i = 0; i < height - 2; i += 2) {
                LOAD_SRC_NEXT_BYTE_INSERT
                LOAD_DST_INSERT
                MERGE_WITH_SRC(src_reg, src_next_reg)
                FILTER_SRC(xfilter)
                src_reg      = _mm512_packus_epi16(exp_src_lo, exp_src_hi);
                src_next_reg = _mm512_inserti64x4(
                    _mm512_castsi256_si512(_mm512_extracti64x4_epi64(src_pack, 1)), _mm512_castsi512_si256(src_reg), 1);

                // average between previous pack to the current
                MERGE_WITH_SRC(src_pack, src_next_reg)
                // filter the source
                FILTER_SRC(yfilter)
                src_pack = src_reg;
                CALC_SUM_SSE_INSIDE_LOOP
                src += src_stride << 1;
                dst += dst_stride << 1;
            }
            // last 2 rows processing happens here
            LOAD_SRC_MERGE_256BIT(xfilter)
            LOAD_DST_INSERT
            FILTER_SRC_256BIT(filter_256bit)
            src_reg_0    = _mm256_packus_epi16(src_lo, src_hi);
            src_next_reg = _mm512_inserti64x4(
                _mm512_castsi256_si512(_mm512_extracti64x4_epi64(src_pack, 1)), src_reg_0, 1);

            MERGE_WITH_SRC(src_pack, src_next_reg)
            FILTER_SRC(yfilter)
            CALC_SUM_SSE_INSIDE_LOOP
        }
    }

    CALC_SSE
    CALC_SUM
    return sum;
}

#define AOM_SUB_PIXEL_VAR_AVX512(w, h, wf, wlog2, hlog2)                                         \
    unsigned int svt_aom_sub_pixel_variance##w##x##h##_avx512(const uint8_t* src,                \
                                                              int            src_stride,         \
                                                              int            x_offset,           \
                                                              int            y_offset,           \
                                                              const uint8_t* dst,                \
                                                              int            dst_stride,         \
                                                              unsigned int*  sse_ptr) {           \
        /*Avoid overflow in helper by capping height.*/                                          \
        const int    hf  = AOMMIN(h, 64);                                                        \
        const int    wf2 = AOMMIN(wf, 128);                                                      \
        unsigned int sse = 0;                                                                    \
        int          se  = 0;                                                                    \
        for (int i = 0; i < (w / wf2); ++i) {                                                    \
            const uint8_t* src_ptr = src;                                                        \
            const uint8_t* dst_ptr = dst;                                                        \
            for (int j = 0; j < (h / hf); ++j) {                                                 \
                unsigned int sse2;                                                               \
                const int    se2 = svt_aom_sub_pixel_variance##wf##xh_avx512(                    \
                    src_ptr, src_stride, x_offset, y_offset, dst_ptr, dst_stride, hf, &sse2); \
                dst_ptr += hf * dst_stride;                                                      \
                src_ptr += hf * src_stride;                                                      \
                se += se2;                                                                       \
                sse += sse2;                                                                     \
            }                                                                                    \
            src += wf;                                                                           \
            dst += wf;                                                                           \
        }                                                                                        \
        *sse_ptr = sse;                                                                          \
        return sse - (unsigned int)(((int64_t)se * se) >> (wlog2 + hlog2));                      \
    }

AOM_SUB_PIXEL_VAR_AVX512(128, 128, 64, 7, 7);
AOM_SUB_PIXEL_VAR_AVX512(128, 64, 64, 7, 6);
AOM_SUB_PIXEL_VAR_AVX512(64, 128, 64, 6, 7);
AOM_SUB_PIXEL_VAR_AVX512(64, 64, 64, 6, 6);
AOM_SUB_PIXEL_VAR_AVX512(64, 32, 64, 6, 5);

AOM_SUB_PIXEL_VAR_AVX512(32, 64, 32, 5, 6);
AOM_SUB_PIXEL_VAR_AVX512(32, 32, 32, 5, 5);
AOM_SUB_PIXEL_VAR_AVX512(32, 16, 32, 5, 4);

static INLINE __m128i mm512_add_hi_lo_epi16(const __m512i val) {
    const __m256i val_lo = _mm512_castsi512_si256(val);
    const __m256i val_hi = _mm512_extracti64x4_epi64(val, 1);
    const __m256i val2   = _mm256_add_epi16(val_lo, val_hi);

    return _mm_add_epi16(_mm256_castsi256_si128(val2), _mm256_extractf128_si256(val2, 1));
}

static INLINE __m128i mm512_sum_to_128_epi32(const __m512i val) {
    const __m256i val_lo = _mm512_castsi512_si256(val);
    const __m256i val_hi = _mm512_extracti64x4_epi64(val, 1);
    const __m256i val2   = _mm256_add_epi32(val_lo, val_hi);

    return _mm_add_epi32(_mm256_castsi256_si128(val2), _mm256_extractf128_si256(val2, 1));
}

static INLINE int variance_final_from_32bit_sum_avx512(__m512i vsse, __m128i vsum, unsigned int* const sse) {
    // extract the low lane and add it to the high lane
    const __m128i sse_reg_128 = mm512_sum_to_128_epi32(vsse);

    // unpack sse and sum registers and add
    const __m128i sse_sum_lo = _mm_unpacklo_epi32(sse_reg_128, vsum);
    const __m128i sse_sum_hi = _mm_unpackhi_epi32(sse_reg_128, vsum);
    const __m128i sse_sum    = _mm_add_epi32(sse_sum_lo, sse_sum_hi);

    // perform the final summation and extract the results
    const __m128i res = _mm_add_epi32(sse_sum, _mm_srli_si128(sse_sum, 8));
    *((int*)sse)      = _mm_cvtsi128_si32(res);
    return _mm_extract_epi32(res, 1);
}

static INLINE __m512i sum_to_32bit_avx512(const __m512i sum) {
    const __m512i sum_lo = _mm512_cvtepi16_epi32(_mm512_castsi512_si256(sum));
    const __m512i sum_hi = _mm512_cvtepi16_epi32(_mm512_extracti64x4_epi64(sum, 1));
    return _mm512_add_epi32(sum_lo, sum_hi);
}

static INLINE void variance_kernel_avx512(const __m512i src, const __m512i ref, __m512i* const sse,
                                          __m512i* const sum) {
    const __m512i adj_sub = _mm512_set1_epi16(0xff01); // (1,-1)

    // unpack into pairs of source and reference values
    const __m512i src_ref0 = _mm512_unpacklo_epi8(src, ref);
    const __m512i src_ref1 = _mm512_unpackhi_epi8(src, ref);

    // subtract adjacent elements using src*1 + ref*-1
    const __m512i diff0 = _mm512_maddubs_epi16(src_ref0, adj_sub);
    const __m512i diff1 = _mm512_maddubs_epi16(src_ref1, adj_sub);
    const __m512i madd0 = _mm512_madd_epi16(diff0, diff0);
    const __m512i madd1 = _mm512_madd_epi16(diff1, diff1);

    // add to the running totals
    *sum = _mm512_add_epi16(*sum, _mm512_add_epi16(diff0, diff1));
    *sse = _mm512_add_epi32(*sse, _mm512_add_epi32(madd0, madd1));
}

static INLINE int variance_final_512_avx512(__m512i vsse, __m512i vsum, unsigned int* const sse) {
    // extract the low lane and add it to the high lane
    const __m128i vsum_128  = mm512_add_hi_lo_epi16(vsum);
    const __m128i vsum_64   = _mm_add_epi16(vsum_128, _mm_srli_si128(vsum_128, 8));
    const __m128i sum_int32 = _mm_cvtepi16_epi32(vsum_64);
    return variance_final_from_32bit_sum_avx512(vsse, sum_int32, sse);
}

static INLINE int variance_final_1024_avx512(__m512i vsse, __m512i vsum, unsigned int* const sse) {
    // extract the low lane and add it to the high lane
    const __m128i vsum_128 = mm512_add_hi_lo_epi16(vsum);
    const __m128i vsum_64  = _mm_add_epi32(_mm_cvtepi16_epi32(vsum_128),
                                          _mm_cvtepi16_epi32(_mm_srli_si128(vsum_128, 8)));
    return variance_final_from_32bit_sum_avx512(vsse, vsum_64, sse);
}

static INLINE int variance_final_2048_avx512(__m512i vsse, __m512i vsum, unsigned int* const sse) {
    vsum                   = sum_to_32bit_avx512(vsum);
    const __m128i vsum_128 = mm512_sum_to_128_epi32(vsum);
    return variance_final_from_32bit_sum_avx512(vsse, vsum_128, sse);
}

static INLINE void variance32_avx512(const uint8_t* src, const int src_stride, const uint8_t* ref, const int ref_stride,
                                     const int h, __m512i* const vsse, __m512i* const vsum) {
    *vsum = _mm512_setzero_si512();

    for (int i = 0; i < (h >> 1); i++) {
        const __m512i s = _mm512_inserti64x4(_mm512_castsi256_si512(_mm256_loadu_si256((__m256i const*)src)),
                                             _mm256_loadu_si256((__m256i const*)(src + src_stride)),
                                             1);
        const __m512i r = _mm512_inserti64x4(_mm512_castsi256_si512(_mm256_loadu_si256((__m256i const*)ref)),
                                             _mm256_loadu_si256((__m256i const*)(ref + ref_stride)),
                                             1);

        variance_kernel_avx512(s, r, vsse, vsum);

        src += (src_stride << 1);
        ref += (ref_stride << 1);
    }
}

static INLINE void variance64_avx512(const uint8_t* src, const int src_stride, const uint8_t* ref, const int ref_stride,
                                     const int h, __m512i* const vsse, __m512i* const vsum) {
    *vsum = _mm512_setzero_si512();

    for (int i = 0; i < h; i++) {
        const __m512i s = _mm512_loadu_si512((__m512i const*)(src));
        const __m512i r = _mm512_loadu_si512((__m512i const*)(ref));
        variance_kernel_avx512(s, r, vsse, vsum);

        src += src_stride;
        ref += ref_stride;
    }
}

static INLINE void variance128_avx512(const uint8_t* src, const int src_stride, const uint8_t* ref,
                                      const int ref_stride, const int h, __m512i* const vsse, __m512i* const vsum) {
    *vsum = _mm512_setzero_si512();

    for (int i = 0; i < h; i++) {
        __m512i s = _mm512_loadu_si512((__m512i const*)(src));
        __m512i r = _mm512_loadu_si512((__m512i const*)(ref));
        variance_kernel_avx512(s, r, vsse, vsum);

        s = _mm512_loadu_si512((__m512i const*)(src + 64));
        r = _mm512_loadu_si512((__m512i const*)(ref + 64));
        variance_kernel_avx512(s, r, vsse, vsum);

        src += src_stride;
        ref += ref_stride;
    }
}

#define AOM_VAR_LOOP_AVX512(bw, bh, bits, uh)                                                        \
    unsigned int svt_aom_variance##bw##x##bh##_avx512(                                               \
        const uint8_t* src, int src_stride, const uint8_t* ref, int ref_stride, unsigned int* sse) { \
        __m512i vsse = _mm512_setzero_si512();                                                       \
        __m512i vsum = _mm512_setzero_si512();                                                       \
        for (int i = 0; i < (bh / uh); i++) {                                                        \
            __m512i vsum16;                                                                          \
            variance##bw##_avx512(src, src_stride, ref, ref_stride, uh, &vsse, &vsum16);             \
            vsum = _mm512_add_epi32(vsum, sum_to_32bit_avx512(vsum16));                              \
            src += uh * src_stride;                                                                  \
            ref += uh * ref_stride;                                                                  \
        }                                                                                            \
        const __m128i vsum_128 = mm512_sum_to_128_epi32(vsum);                                       \
        const int     sum      = variance_final_from_32bit_sum_avx512(vsse, vsum_128, sse);          \
        return *sse - (unsigned int)(((int64_t)sum * sum) >> bits);                                  \
    }

AOM_VAR_LOOP_AVX512(64, 64, 12, 32); // 64x32 * ( 64/32)
AOM_VAR_LOOP_AVX512(64, 128, 13, 32); // 64x32 * (128/32)
AOM_VAR_LOOP_AVX512(128, 64, 13, 16); // 128x16 * ( 64/16)
AOM_VAR_LOOP_AVX512(128, 128, 14, 16); // 128x16 * (128/16)

#define AOM_VAR_NO_LOOP_AVX512(bw, bh, bits, max_pixel)                                              \
    unsigned int svt_aom_variance##bw##x##bh##_avx512(                                               \
        const uint8_t* src, int src_stride, const uint8_t* ref, int ref_stride, unsigned int* sse) { \
        __m512i vsse = _mm512_setzero_si512();                                                       \
        __m512i vsum = _mm512_setzero_si512();                                                       \
        variance##bw##_avx512(src, src_stride, ref, ref_stride, bh, &vsse, &vsum);                   \
        const int sum = variance_final_##max_pixel##_avx512(vsse, vsum, sse);                        \
        return *sse - (uint32_t)(((int64_t)sum * sum) >> bits);                                      \
    }

AOM_VAR_NO_LOOP_AVX512(32, 8, 8, 512);
AOM_VAR_NO_LOOP_AVX512(32, 16, 9, 512);
AOM_VAR_NO_LOOP_AVX512(32, 32, 10, 1024);
AOM_VAR_NO_LOOP_AVX512(32, 64, 11, 2048);

AOM_VAR_NO_LOOP_AVX512(64, 16, 10, 1024);
AOM_VAR_NO_LOOP_AVX512(64, 32, 11, 2048);

#endif // !EN_AVX512_SUPPORT
