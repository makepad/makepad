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
#include "synonyms.h"
#include <assert.h>
#include <immintrin.h>
#include "aom_dsp_rtcd.h"
#include "enc_inter_prediction.h"

////////////////////////////////////////////////////////////////////////////////
// 8 bit
////////////////////////////////////////////////////////////////////////////////
static INLINE int32_t xx_hsum_epi32_si32(__m128i v_d) {
    v_d = _mm_hadd_epi32(v_d, v_d);
    v_d = _mm_hadd_epi32(v_d, v_d);
    return _mm_cvtsi128_si32(v_d);
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

static INLINE void obmc_variance_w8n(const uint8_t* pre, const int pre_stride, const int32_t* wsrc, const int32_t* mask,
                                     unsigned int* const sse, int* const sum, const int w, const int h) {
    int           n = 0, height = h;
    __m128i       v_sum_d  = _mm_setzero_si128();
    __m128i       v_sse_d  = _mm_setzero_si128();
    const __m256i v_bias_d = _mm256_set1_epi32((1 << 12) >> 1);
    __m128i       v_d;
    assert(w >= 8);
    assert(IS_POWER_OF_TWO(w));
    assert(IS_POWER_OF_TWO(h));
    do {
        int            width    = w;
        const uint8_t* pre_temp = pre;
        do {
            const __m128i v_p_b  = _mm_loadl_epi64((const __m128i*)pre_temp);
            const __m256i v_m_d  = _mm256_loadu_si256((__m256i const*)(mask + n));
            const __m256i v_w_d  = _mm256_loadu_si256((__m256i const*)(wsrc + n));
            const __m256i v_p0_d = _mm256_cvtepu8_epi32(v_p_b);

            // Values in both pre and mask fit in 15 bits, and are packed at 32 bit
            // boundaries. We use pmaddwd, as it has lower latency on Haswell
            // than pmulld but produces the same result with these inputs.
            const __m256i v_pm_d    = _mm256_madd_epi16(v_p0_d, v_m_d);
            const __m256i v_diff0_d = _mm256_sub_epi32(v_w_d, v_pm_d);

            const __m256i v_sign_d   = _mm256_srai_epi32(v_diff0_d, 31);
            const __m256i v_tmp_d    = _mm256_add_epi32(_mm256_add_epi32(v_diff0_d, v_bias_d), v_sign_d);
            const __m256i v_rdiff0_d = _mm256_srai_epi32(v_tmp_d, 12);
            const __m128i v_rdiff_d  = _mm256_castsi256_si128(v_rdiff0_d);
            const __m128i v_rdiff1_d = _mm256_extracti128_si256(v_rdiff0_d, 1);

            const __m128i v_rdiff01_w = _mm_packs_epi32(v_rdiff_d, v_rdiff1_d);
            const __m128i v_sqrdiff_d = _mm_madd_epi16(v_rdiff01_w, v_rdiff01_w);

            v_sum_d = _mm_add_epi32(v_sum_d, v_rdiff_d);
            v_sum_d = _mm_add_epi32(v_sum_d, v_rdiff1_d);
            v_sse_d = _mm_add_epi32(v_sse_d, v_sqrdiff_d);

            pre_temp += 8;
            n += 8;
            width -= 8;
        } while (width > 0);
        pre += pre_stride;
        height -= 1;
    } while (height > 0);
    v_d  = _mm_hadd_epi32(v_sum_d, v_sse_d);
    v_d  = _mm_hadd_epi32(v_d, v_d);
    *sum = _mm_cvtsi128_si32(v_d);
    *sse = _mm_cvtsi128_si32(_mm_srli_si128(v_d, 4));
}

static INLINE void obmc_variance_w16n(const uint8_t* pre, const int pre_stride, const int32_t* wsrc,
                                      const int32_t* mask, unsigned int* const sse, int* const sum, const int w,
                                      const int h) {
    int           n = 0, height = h;
    __m256i       v_d;
    __m128i       res0;
    const __m256i v_bias_d = _mm256_set1_epi32((1 << 12) >> 1);
    __m256i       v_sum_d  = _mm256_setzero_si256();
    __m256i       v_sse_d  = _mm256_setzero_si256();

    assert(w >= 16);
    assert(IS_POWER_OF_TWO(w));
    assert(IS_POWER_OF_TWO(h));
    do {
        int            width    = w;
        const uint8_t* pre_temp = pre;
        do {
            const __m128i v_p_b  = _mm_loadu_si128((__m128i*)pre_temp);
            const __m256i v_m0_d = _mm256_loadu_si256((__m256i const*)(mask + n));
            const __m256i v_w0_d = _mm256_loadu_si256((__m256i const*)(wsrc + n));
            const __m256i v_m1_d = _mm256_loadu_si256((__m256i const*)(mask + n + 8));
            const __m256i v_w1_d = _mm256_loadu_si256((__m256i const*)(wsrc + n + 8));

            const __m256i v_p0_d = _mm256_cvtepu8_epi32(v_p_b);
            const __m256i v_p1_d = _mm256_cvtepu8_epi32(_mm_srli_si128(v_p_b, 8));

            const __m256i v_pm0_d = _mm256_madd_epi16(v_p0_d, v_m0_d);
            const __m256i v_pm1_d = _mm256_madd_epi16(v_p1_d, v_m1_d);

            const __m256i v_diff0_d = _mm256_sub_epi32(v_w0_d, v_pm0_d);
            const __m256i v_diff1_d = _mm256_sub_epi32(v_w1_d, v_pm1_d);

            const __m256i v_sign0_d = _mm256_srai_epi32(v_diff0_d, 31);
            const __m256i v_sign1_d = _mm256_srai_epi32(v_diff1_d, 31);

            const __m256i v_tmp0_d = _mm256_add_epi32(_mm256_add_epi32(v_diff0_d, v_bias_d), v_sign0_d);
            const __m256i v_tmp1_d = _mm256_add_epi32(_mm256_add_epi32(v_diff1_d, v_bias_d), v_sign1_d);

            const __m256i v_rdiff0_d = _mm256_srai_epi32(v_tmp0_d, 12);
            const __m256i v_rdiff2_d = _mm256_srai_epi32(v_tmp1_d, 12);

            const __m256i v_rdiff1_d  = _mm256_add_epi32(v_rdiff0_d, v_rdiff2_d);
            const __m256i v_rdiff01_w = _mm256_packs_epi32(v_rdiff0_d, v_rdiff2_d);
            const __m256i v_sqrdiff_d = _mm256_madd_epi16(v_rdiff01_w, v_rdiff01_w);

            v_sum_d = _mm256_add_epi32(v_sum_d, v_rdiff1_d);
            v_sse_d = _mm256_add_epi32(v_sse_d, v_sqrdiff_d);

            pre_temp += 16;
            n += 16;
            width -= 16;
        } while (width > 0);
        pre += pre_stride;
        height -= 1;
    } while (height > 0);

    v_d  = _mm256_hadd_epi32(v_sum_d, v_sse_d);
    v_d  = _mm256_hadd_epi32(v_d, v_d);
    res0 = _mm256_castsi256_si128(v_d);
    res0 = _mm_add_epi32(res0, _mm256_extractf128_si256(v_d, 1));
    *sum = _mm_cvtsi128_si32(res0);
    *sse = _mm_cvtsi128_si32(_mm_srli_si128(res0, 4));
}

#define OBMCVARWXH(W, H)                                                                                   \
    unsigned int svt_aom_obmc_variance##W##x##H##_avx2(                                                    \
        const uint8_t* pre, int pre_stride, const int32_t* wsrc, const int32_t* mask, unsigned int* sse) { \
        int sum;                                                                                           \
        if (W == 4) {                                                                                      \
            obmc_variance_w4(pre, pre_stride, wsrc, mask, sse, &sum, H);                                   \
        } else if (W == 8) {                                                                               \
            obmc_variance_w8n(pre, pre_stride, wsrc, mask, sse, &sum, W, H);                               \
        } else {                                                                                           \
            obmc_variance_w16n(pre, pre_stride, wsrc, mask, sse, &sum, W, H);                              \
        }                                                                                                  \
                                                                                                           \
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

void svt_av1_calc_target_weighted_pred_above_avx2(uint8_t is16bit, MacroBlockD* xd, int rel_mi_col, uint8_t nb_mi_width,
                                                  MbModeInfo* nb_mi, void* fun_ctxt) {
    (void)nb_mi;
    (void)is16bit;
    struct calc_target_weighted_pred_ctxt* ctxt = (struct calc_target_weighted_pred_ctxt*)fun_ctxt;

    const int            bw     = xd->n4_w << MI_SIZE_LOG2;
    const uint8_t* const mask1d = svt_av1_get_obmc_mask(ctxt->overlap);
    assert(mask1d != NULL);
    int32_t*       wsrc = ctxt->wsrc_buf + (rel_mi_col * MI_SIZE);
    int32_t*       mask = ctxt->mask_buf + (rel_mi_col * MI_SIZE);
    const uint8_t* tmp  = ctxt->tmp + rel_mi_col * MI_SIZE;

    uint32_t calc_w = nb_mi_width * MI_SIZE;
    if (calc_w == 4) {
        __m128i tmp_sse, wsrc_sse, m0_sse, m1_sse;
        for (int row = 0; row < ctxt->overlap; ++row) {
            m0_sse = _mm_set1_epi32(mask1d[row]);
            m1_sse = _mm_set1_epi32(AOM_BLEND_A64_MAX_ALPHA - mask1d[row]);

            tmp_sse  = _mm_cvtsi32_si128(*(int*)(tmp));
            tmp_sse  = _mm_unpacklo_epi8(tmp_sse, _mm_setzero_si128());
            tmp_sse  = _mm_unpacklo_epi16(tmp_sse, _mm_setzero_si128());
            wsrc_sse = _mm_mullo_epi32(m1_sse, tmp_sse);
            _mm_storeu_si128((__m128i*)(wsrc), wsrc_sse);
            _mm_storeu_si128((__m128i*)(mask), m0_sse);

            wsrc += bw;
            mask += bw;
            tmp += ctxt->tmp_stride;
        }
    } else if (calc_w == 8) {
        __m256i tmp_avx, wsrc_avx, m0_avx, m1_avx;
        __m128i tmp_sse;
        for (int row = 0; row < ctxt->overlap; ++row) {
            m0_avx = _mm256_set1_epi32(mask1d[row]);
            m1_avx = _mm256_set1_epi32(AOM_BLEND_A64_MAX_ALPHA - mask1d[row]);

            tmp_sse  = _mm_loadl_epi64((__m128i*)(tmp));
            tmp_sse  = _mm_unpacklo_epi8(tmp_sse, _mm_setzero_si128());
            tmp_avx  = _mm256_cvtepi16_epi32(tmp_sse);
            wsrc_avx = _mm256_mullo_epi32(m1_avx, tmp_avx);
            _mm256_storeu_si256((__m256i*)(wsrc), wsrc_avx);
            _mm256_storeu_si256((__m256i*)(mask), m0_avx);

            wsrc += bw;
            mask += bw;
            tmp += ctxt->tmp_stride;
        }
    } else if (calc_w == 16) {
        __m256i tmp_avx, wsrc_avx, m0_avx, m1_avx;
        __m128i tmp_sse1, tmp_sse2;
        for (int row = 0; row < ctxt->overlap; ++row) {
            m0_avx = _mm256_set1_epi32(mask1d[row]);
            m1_avx = _mm256_set1_epi32(AOM_BLEND_A64_MAX_ALPHA - mask1d[row]);

            tmp_sse1 = _mm_loadu_si128((__m128i*)(tmp));
            tmp_sse2 = _mm_unpacklo_epi8(tmp_sse1, _mm_setzero_si128());
            tmp_avx  = _mm256_cvtepi16_epi32(tmp_sse2);
            wsrc_avx = _mm256_mullo_epi32(m1_avx, tmp_avx);
            _mm256_storeu_si256((__m256i*)(wsrc), wsrc_avx);
            _mm256_storeu_si256((__m256i*)(mask), m0_avx);

            tmp_sse1 = _mm_unpackhi_epi8(tmp_sse1, _mm_setzero_si128());
            tmp_avx  = _mm256_cvtepi16_epi32(tmp_sse1);
            wsrc_avx = _mm256_mullo_epi32(m1_avx, tmp_avx);
            _mm256_storeu_si256((__m256i*)(wsrc + 8), wsrc_avx);
            _mm256_storeu_si256((__m256i*)(mask + 8), m0_avx);

            wsrc += bw;
            mask += bw;
            tmp += ctxt->tmp_stride;
        }
    } else {
        __m256i tmp_avx, wsrc_avx, m0_avx, m1_avx;
        __m128i tmp_sse1, tmp_sse2;
        for (int row = 0; row < ctxt->overlap; ++row) {
            m0_avx  = _mm256_set1_epi32(mask1d[row]);
            m1_avx  = _mm256_set1_epi32(AOM_BLEND_A64_MAX_ALPHA - mask1d[row]);
            int col = 0;
            for (; col <= (nb_mi_width * MI_SIZE) - 16; col += 16) {
                tmp_sse1 = _mm_loadu_si128((__m128i*)(tmp + col));
                tmp_sse2 = _mm_unpacklo_epi8(tmp_sse1, _mm_setzero_si128());
                tmp_avx  = _mm256_cvtepi16_epi32(tmp_sse2);
                wsrc_avx = _mm256_mullo_epi32(m1_avx, tmp_avx);
                _mm256_storeu_si256((__m256i*)(wsrc + col), wsrc_avx);
                _mm256_storeu_si256((__m256i*)(mask + col), m0_avx);

                tmp_sse1 = _mm_unpackhi_epi8(tmp_sse1, _mm_setzero_si128());
                tmp_avx  = _mm256_cvtepi16_epi32(tmp_sse1);
                wsrc_avx = _mm256_mullo_epi32(m1_avx, tmp_avx);
                _mm256_storeu_si256((__m256i*)(wsrc + col + 8), wsrc_avx);
                _mm256_storeu_si256((__m256i*)(mask + col + 8), m0_avx);
            }
            for (; col < nb_mi_width * MI_SIZE; ++col) {
                wsrc[col] = (AOM_BLEND_A64_MAX_ALPHA - mask1d[row]) * tmp[col];
                mask[col] = mask1d[row];
            }
            wsrc += bw;
            mask += bw;
            tmp += ctxt->tmp_stride;
        }
    }
}

void svt_av1_calc_target_weighted_pred_left_avx2(uint8_t is16bit, MacroBlockD* xd, int rel_mi_row, uint8_t nb_mi_height,
                                                 MbModeInfo* nb_mi, void* fun_ctxt) {
    (void)nb_mi;
    (void)is16bit;
    struct calc_target_weighted_pred_ctxt* ctxt = (struct calc_target_weighted_pred_ctxt*)fun_ctxt;

    const int            bw     = xd->n4_w << MI_SIZE_LOG2;
    const uint8_t* const mask1d = svt_av1_get_obmc_mask(ctxt->overlap);

    int32_t*       wsrc = ctxt->wsrc_buf + (rel_mi_row * MI_SIZE * bw);
    int32_t*       mask = ctxt->mask_buf + (rel_mi_row * MI_SIZE * bw);
    const uint8_t* tmp  = ctxt->tmp + (rel_mi_row * MI_SIZE * ctxt->tmp_stride);
    assert(mask1d != NULL);

    if (ctxt->overlap == 4) {
        __m128i tmp_sse, wsrc_sse, m0_sse, m1_sse, mask_sse;
        __m128i blend_max_alpha_sse = _mm_set1_epi32(AOM_BLEND_A64_MAX_ALPHA);
        m0_sse                      = _mm_cvtsi32_si128(*(int*)(mask1d));
        m0_sse                      = _mm_unpacklo_epi8(m0_sse, _mm_setzero_si128());
        m0_sse                      = _mm_unpacklo_epi16(m0_sse, _mm_setzero_si128());
        m1_sse                      = _mm_sub_epi32(blend_max_alpha_sse, m0_sse);
        for (int row = 0; row < nb_mi_height * MI_SIZE; ++row) {
            //(tmp[col] << AOM_BLEND_A64_ROUND_BITS) * m1
            tmp_sse = _mm_cvtsi32_si128(*(int*)(tmp));
            tmp_sse = _mm_unpacklo_epi8(tmp_sse, _mm_setzero_si128());
            tmp_sse = _mm_unpacklo_epi16(tmp_sse, _mm_setzero_si128());
            tmp_sse = _mm_slli_epi32(tmp_sse, AOM_BLEND_A64_ROUND_BITS);
            tmp_sse = _mm_mullo_epi32(tmp_sse, m1_sse);

            //(wsrc[col] >> AOM_BLEND_A64_ROUND_BITS) * m0
            wsrc_sse = _mm_loadu_si128((__m128i*)(wsrc));
            wsrc_sse = _mm_srai_epi32(wsrc_sse, AOM_BLEND_A64_ROUND_BITS);
            wsrc_sse = _mm_mullo_epi32(wsrc_sse, m0_sse);
            wsrc_sse = _mm_add_epi32(wsrc_sse, tmp_sse);
            _mm_storeu_si128((__m128i*)(wsrc), wsrc_sse);

            mask_sse = _mm_loadu_si128((__m128i*)(mask));
            mask_sse = _mm_srai_epi32(mask_sse, AOM_BLEND_A64_ROUND_BITS);
            mask_sse = _mm_mullo_epi32(mask_sse, m0_sse);
            _mm_storeu_si128((__m128i*)(mask), mask_sse);

            wsrc += bw;
            mask += bw;
            tmp += ctxt->tmp_stride;
        }
    } else if (ctxt->overlap == 8) {
        __m256i tmp_avx, wsrc_avx, m0_avx, m1_avx, mask_avx;
        __m128i m0_sse, tmp_sse;
        __m256i blend_max_alpha_sse = _mm256_set1_epi32(AOM_BLEND_A64_MAX_ALPHA);
        m0_sse                      = _mm_loadl_epi64((__m128i*)(mask1d));
        m0_sse                      = _mm_unpacklo_epi8(m0_sse, _mm_setzero_si128());
        m0_avx                      = _mm256_cvtepi16_epi32(m0_sse);
        m1_avx                      = _mm256_sub_epi32(blend_max_alpha_sse, m0_avx);
        for (int row = 0; row < nb_mi_height * MI_SIZE; ++row) {
            //(tmp[col] << AOM_BLEND_A64_ROUND_BITS) * m1
            tmp_sse = _mm_loadl_epi64((__m128i*)(tmp));
            tmp_sse = _mm_unpacklo_epi8(tmp_sse, _mm_setzero_si128());
            tmp_avx = _mm256_cvtepi16_epi32(tmp_sse);
            tmp_avx = _mm256_slli_epi32(tmp_avx, AOM_BLEND_A64_ROUND_BITS);
            tmp_avx = _mm256_mullo_epi32(tmp_avx, m1_avx);

            //(wsrc[col] >> AOM_BLEND_A64_ROUND_BITS) * m0
            wsrc_avx = _mm256_loadu_si256((__m256i*)(wsrc));
            wsrc_avx = _mm256_srai_epi32(wsrc_avx, AOM_BLEND_A64_ROUND_BITS);
            wsrc_avx = _mm256_mullo_epi32(wsrc_avx, m0_avx);
            wsrc_avx = _mm256_add_epi32(wsrc_avx, tmp_avx);
            _mm256_storeu_si256((__m256i*)(wsrc), wsrc_avx);

            mask_avx = _mm256_loadu_si256((__m256i*)(mask));
            mask_avx = _mm256_srai_epi32(mask_avx, AOM_BLEND_A64_ROUND_BITS);
            mask_avx = _mm256_mullo_epi32(mask_avx, m0_avx);
            _mm256_storeu_si256((__m256i*)(mask), mask_avx);

            wsrc += bw;
            mask += bw;
            tmp += ctxt->tmp_stride;
        }
    } else {
        __m256i tmp_avx, wsrc_avx, m0_avx, m1_avx, mask_avx;
        __m128i m0_sse, tmp_sse;
        __m256i blend_max_alpha_sse = _mm256_set1_epi32(AOM_BLEND_A64_MAX_ALPHA);
        for (int row = 0; row < nb_mi_height * MI_SIZE; ++row) {
            int col = 0;
            for (; col <= ctxt->overlap - 8; col += 8) {
                m0_sse = _mm_loadl_epi64((__m128i*)(mask1d + col));
                m0_sse = _mm_unpacklo_epi8(m0_sse, _mm_setzero_si128());
                m0_avx = _mm256_cvtepi16_epi32(m0_sse);
                m1_avx = _mm256_sub_epi32(blend_max_alpha_sse, m0_avx);
                //(tmp[col] << AOM_BLEND_A64_ROUND_BITS) * m1
                tmp_sse = _mm_loadl_epi64((__m128i*)(tmp + col));
                tmp_sse = _mm_unpacklo_epi8(tmp_sse, _mm_setzero_si128());
                tmp_avx = _mm256_cvtepi16_epi32(tmp_sse);
                tmp_avx = _mm256_slli_epi32(tmp_avx, AOM_BLEND_A64_ROUND_BITS);
                tmp_avx = _mm256_mullo_epi32(tmp_avx, m1_avx);

                //(wsrc[col] >> AOM_BLEND_A64_ROUND_BITS) * m0
                wsrc_avx = _mm256_loadu_si256((__m256i*)(wsrc + col));
                wsrc_avx = _mm256_srai_epi32(wsrc_avx, AOM_BLEND_A64_ROUND_BITS);
                wsrc_avx = _mm256_mullo_epi32(wsrc_avx, m0_avx);
                wsrc_avx = _mm256_add_epi32(wsrc_avx, tmp_avx);
                _mm256_storeu_si256((__m256i*)(wsrc + col), wsrc_avx);

                mask_avx = _mm256_loadu_si256((__m256i*)(mask + col));
                mask_avx = _mm256_srai_epi32(mask_avx, AOM_BLEND_A64_ROUND_BITS);
                mask_avx = _mm256_mullo_epi32(mask_avx, m0_avx);
                _mm256_storeu_si256((__m256i*)(mask + col), mask_avx);
            }
            for (; col < ctxt->overlap; ++col) {
                const uint8_t m0 = mask1d[col];
                const uint8_t m1 = AOM_BLEND_A64_MAX_ALPHA - m0;
                wsrc[col] = (wsrc[col] >> AOM_BLEND_A64_ROUND_BITS) * m0 + (tmp[col] << AOM_BLEND_A64_ROUND_BITS) * m1;
                mask[col] = (mask[col] >> AOM_BLEND_A64_ROUND_BITS) * m0;
            }
            wsrc += bw;
            mask += bw;
            tmp += ctxt->tmp_stride;
        }
    }
}
