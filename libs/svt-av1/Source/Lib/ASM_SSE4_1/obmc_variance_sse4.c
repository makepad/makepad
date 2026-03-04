/*
 * Copyright (c) 2016, Alliance for Open Media. All rights reserved
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
 * Media Patent License 1.0 was not distributed with this source code in the
 * PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
 */

#include "aom_dsp_rtcd.h"
#include "synonyms.h"

#include "filter.h"

void svt_aom_var_filter_block2d_bil_first_pass_ssse3(const uint8_t* a, uint16_t* b, unsigned int src_pixels_per_line,
                                                     unsigned int pixel_step, unsigned int output_height,
                                                     unsigned int output_width, const uint8_t* filter);

void svt_aom_var_filter_block2d_bil_second_pass_ssse3(const uint16_t* a, uint8_t* b, unsigned int src_pixels_per_line,
                                                      unsigned int pixel_step, unsigned int output_height,
                                                      unsigned int output_width, const uint8_t* filter);

static INLINE __m128i xx_load_128(const void* a) {
    return _mm_loadu_si128((const __m128i*)a);
}

static INLINE int32_t xx_hsum_epi32_si32(__m128i v_d) {
    v_d = _mm_hadd_epi32(v_d, v_d);
    v_d = _mm_hadd_epi32(v_d, v_d);
    return _mm_cvtsi128_si32(v_d);
}

static INLINE void obmc_variance_w8n(const uint8_t* pre, const int pre_stride, const int32_t* wsrc, const int32_t* mask,
                                     unsigned int* const sse, int* const sum, const int w, const int h) {
    const int pre_step = pre_stride - w;
    int       n        = 0;
    __m128i   v_sum_d  = _mm_setzero_si128();
    __m128i   v_sse_d  = _mm_setzero_si128();

    assert(w >= 8);
    assert(IS_POWER_OF_TWO(w));
    assert(IS_POWER_OF_TWO(h));

    do {
        const __m128i v_p1_b = xx_loadl_32(pre + n + 4);
        const __m128i v_m1_d = xx_load_128(mask + n + 4);
        const __m128i v_w1_d = xx_load_128(wsrc + n + 4);
        const __m128i v_p0_b = xx_loadl_32(pre + n);
        const __m128i v_m0_d = xx_load_128(mask + n);
        const __m128i v_w0_d = xx_load_128(wsrc + n);

        const __m128i v_p0_d = _mm_cvtepu8_epi32(v_p0_b);
        const __m128i v_p1_d = _mm_cvtepu8_epi32(v_p1_b);

        // Values in both pre and mask fit in 15 bits, and are packed at 32 bit
        // boundaries. We use pmaddwd, as it has lower latency on Haswell
        // than pmulld but produces the same result with these inputs.
        const __m128i v_pm0_d = _mm_madd_epi16(v_p0_d, v_m0_d);
        const __m128i v_pm1_d = _mm_madd_epi16(v_p1_d, v_m1_d);

        const __m128i v_diff0_d = _mm_sub_epi32(v_w0_d, v_pm0_d);
        const __m128i v_diff1_d = _mm_sub_epi32(v_w1_d, v_pm1_d);

        const __m128i v_rdiff0_d  = xx_roundn_epi32(v_diff0_d, 12);
        const __m128i v_rdiff1_d  = xx_roundn_epi32(v_diff1_d, 12);
        const __m128i v_rdiff01_w = _mm_packs_epi32(v_rdiff0_d, v_rdiff1_d);
        const __m128i v_sqrdiff_d = _mm_madd_epi16(v_rdiff01_w, v_rdiff01_w);

        v_sum_d = _mm_add_epi32(v_sum_d, v_rdiff0_d);
        v_sum_d = _mm_add_epi32(v_sum_d, v_rdiff1_d);
        v_sse_d = _mm_add_epi32(v_sse_d, v_sqrdiff_d);

        n += 8;

        if (n % w == 0) {
            pre += pre_step;
        }
    } while (n < w * h);

    *sum = xx_hsum_epi32_si32(v_sum_d);
    *sse = xx_hsum_epi32_si32(v_sse_d);
}

static INLINE void obmc_variance_w4(const uint8_t* pre, const int pre_stride, const int32_t* wsrc, const int32_t* mask,
                                    unsigned int* const sse, int* const sum, const int h) {
    const int pre_step = pre_stride - 4;
    int       n        = 0;
    __m128i   v_sum_d  = _mm_setzero_si128();
    __m128i   v_sse_d  = _mm_setzero_si128();

    assert(IS_POWER_OF_TWO(h));

    do {
        const __m128i v_p_b = _mm_cvtsi32_si128(*(const uint32_t*)(pre + n));
        const __m128i v_m_d = _mm_loadu_si128((const __m128i*)(mask + n));
        const __m128i v_w_d = _mm_loadu_si128((const __m128i*)(wsrc + n));

        const __m128i v_p_d = _mm_cvtepu8_epi32(v_p_b);

        // Values in both pre and mask fit in 15 bits, and are packed at 32 bit
        // boundaries. We use pmaddwd, as it has lower latency on Haswell
        // than pmulld but produces the same result with these inputs.
        const __m128i v_pm_d = _mm_madd_epi16(v_p_d, v_m_d);

        const __m128i v_diff_d    = _mm_sub_epi32(v_w_d, v_pm_d);
        const __m128i v_rdiff_d   = xx_roundn_epi32(v_diff_d, 12);
        const __m128i v_sqrdiff_d = _mm_mullo_epi32(v_rdiff_d, v_rdiff_d);

        v_sum_d = _mm_add_epi32(v_sum_d, v_rdiff_d);
        v_sse_d = _mm_add_epi32(v_sse_d, v_sqrdiff_d);

        n += 4;

        if (n % 4 == 0) {
            pre += pre_step;
        }
    } while (n < 4 * h);

    *sum = xx_hsum_epi32_si32(v_sum_d);
    *sse = xx_hsum_epi32_si32(v_sse_d);
}

#define OBMCVARWXH(W, H)                                                                                   \
    unsigned int svt_aom_obmc_variance##W##x##H##_sse4_1(                                                  \
        const uint8_t* pre, int pre_stride, const int32_t* wsrc, const int32_t* mask, unsigned int* sse) { \
        int sum;                                                                                           \
        if (W == 4) {                                                                                      \
            obmc_variance_w4(pre, pre_stride, wsrc, mask, sse, &sum, H);                                   \
        } else {                                                                                           \
            obmc_variance_w8n(pre, pre_stride, wsrc, mask, sse, &sum, W, H);                               \
        }                                                                                                  \
        return *sse - (unsigned int)(((int64_t)sum * sum) / (W * H));                                      \
    }

OBMCVARWXH(128, 128)
OBMCVARWXH(128, 64)
OBMCVARWXH(64, 128)
OBMCVARWXH(64, 64)
OBMCVARWXH(64, 32)
OBMCVARWXH(32, 64)
OBMCVARWXH(32, 32)
OBMCVARWXH(32, 16)
OBMCVARWXH(16, 32)
OBMCVARWXH(16, 16)
OBMCVARWXH(16, 8)
OBMCVARWXH(8, 16)
OBMCVARWXH(8, 8)
OBMCVARWXH(8, 4)
OBMCVARWXH(4, 8)
OBMCVARWXH(4, 4)
OBMCVARWXH(4, 16)
OBMCVARWXH(16, 4)
OBMCVARWXH(8, 32)
OBMCVARWXH(32, 8)
OBMCVARWXH(16, 64)
OBMCVARWXH(64, 16)

#define OBMC_SUBPIX_VAR(W, H)                                                                                      \
    uint32_t svt_aom_obmc_sub_pixel_variance##W##x##H##_sse4_1(const uint8_t* pre,                                 \
                                                               int            pre_stride,                          \
                                                               int            xoffset,                             \
                                                               int            yoffset,                             \
                                                               const int32_t* wsrc,                                \
                                                               const int32_t* mask,                                \
                                                               unsigned int*  sse) {                                \
        uint16_t fdata3[(H + 1) * W];                                                                              \
        uint8_t  temp2[H * W];                                                                                     \
                                                                                                                   \
        svt_aom_var_filter_block2d_bil_first_pass_ssse3(                                                           \
            pre, fdata3, pre_stride, 1, H + 1, W, bilinear_filters_2t[xoffset]);                                   \
        svt_aom_var_filter_block2d_bil_second_pass_ssse3(fdata3, temp2, W, W, H, W, bilinear_filters_2t[yoffset]); \
                                                                                                                   \
        return svt_aom_obmc_variance##W##x##H##_sse4_1(temp2, W, wsrc, mask, sse);                                 \
    }

OBMC_SUBPIX_VAR(128, 128)
OBMC_SUBPIX_VAR(128, 64)
OBMC_SUBPIX_VAR(64, 128)
OBMC_SUBPIX_VAR(64, 64)
OBMC_SUBPIX_VAR(64, 32)
OBMC_SUBPIX_VAR(32, 64)
OBMC_SUBPIX_VAR(32, 32)
OBMC_SUBPIX_VAR(32, 16)
OBMC_SUBPIX_VAR(16, 32)
OBMC_SUBPIX_VAR(16, 16)
OBMC_SUBPIX_VAR(16, 8)
OBMC_SUBPIX_VAR(8, 16)
OBMC_SUBPIX_VAR(8, 8)
OBMC_SUBPIX_VAR(8, 4)
OBMC_SUBPIX_VAR(4, 8)
OBMC_SUBPIX_VAR(4, 4)
OBMC_SUBPIX_VAR(4, 16)
OBMC_SUBPIX_VAR(16, 4)
OBMC_SUBPIX_VAR(8, 32)
OBMC_SUBPIX_VAR(32, 8)
OBMC_SUBPIX_VAR(16, 64)
OBMC_SUBPIX_VAR(64, 16)

static void aom_highbd_calc16x16var_sse4_1(const uint16_t* src, int src_stride, const uint16_t* ref, int ref_stride,
                                           uint32_t* sse, int* sum) {
    __m128i       v_sum_d = _mm_setzero_si128();
    __m128i       v_sse_d = _mm_setzero_si128();
    const __m128i one     = _mm_set1_epi16(1);
    for (int i = 0; i < 16; ++i) {
        __m128i v_p_a     = _mm_loadu_si128((const __m128i*)src);
        __m128i v_p_b     = _mm_loadu_si128((const __m128i*)ref);
        __m128i v_diff    = _mm_sub_epi16(v_p_a, v_p_b);
        __m128i v_sqrdiff = _mm_madd_epi16(v_diff, v_diff);
        v_sum_d           = _mm_add_epi16(v_sum_d, v_diff);
        v_sse_d           = _mm_add_epi32(v_sse_d, v_sqrdiff);
        v_p_a             = _mm_loadu_si128((const __m128i*)(src + 8));
        v_p_b             = _mm_loadu_si128((const __m128i*)(ref + 8));
        v_diff            = _mm_sub_epi16(v_p_a, v_p_b);
        v_sqrdiff         = _mm_madd_epi16(v_diff, v_diff);
        v_sum_d           = _mm_add_epi16(v_sum_d, v_diff);
        v_sse_d           = _mm_add_epi32(v_sse_d, v_sqrdiff);
        src += src_stride;
        ref += ref_stride;
    }
    __m128i v_sum0 = _mm_madd_epi16(v_sum_d, one);

    __m128i v_d_l = _mm_unpacklo_epi32(v_sum0, v_sse_d);
    __m128i v_d_h = _mm_unpackhi_epi32(v_sum0, v_sse_d);

    __m128i v_d = _mm_add_epi32(v_d_l, v_d_h);
    v_d         = _mm_add_epi32(v_d, _mm_srli_si128(v_d, 8));
    *sum        = _mm_extract_epi32(v_d, 0);
    *sse        = _mm_extract_epi32(v_d, 1);
}

static inline void variance_highbd_32x32_sse4_1(const uint16_t* src, int src_stride, const uint16_t* ref,
                                                int ref_stride, uint32_t* sse, int* sum) {
    uint32_t sse0;
    int      sum0;

    for (int i = 0; i < 32; i += 16) {
        for (int j = 0; j < 32; j += 16) {
            aom_highbd_calc16x16var_sse4_1(
                src + src_stride * i + j, src_stride, ref + ref_stride * i + j, ref_stride, &sse0, &sum0);
            *sum += sum0;
            *sse += sse0;
        }
    }
}

/*
* Helper function to compute variance with 16 bit input for square blocks of size 16 and 32
*/
uint32_t svt_aom_variance_highbd_sse4_1(const uint16_t* a, int a_stride, const uint16_t* b, int b_stride, int w, int h,
                                        uint32_t* sse) {
    assert(w == h);

    int sum = 0;
    *sse    = 0;

    switch (w) {
    case 16:
        aom_highbd_calc16x16var_sse4_1(a, a_stride, b, b_stride, sse, &sum);
        break;
    case 32:
        variance_highbd_32x32_sse4_1(a, a_stride, b, b_stride, sse, &sum);
        break;
    default:
        assert(0);
    }

    return *sse - ((int64_t)sum * sum) / (w * h);
}
