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

#ifndef AOM_AOM_DSP_X86_BLEND_SSE4_H_
#define AOM_AOM_DSP_X86_BLEND_SSE4_H_

#include <assert.h>

#include "definitions.h"
#include <smmintrin.h>
#include "synonyms.h"

static const uint8_t g_blend_a64_mask_shuffle[32] = {
    0, 2, 4, 6, 8, 10, 12, 14, 1, 3, 5, 7, 9, 11, 13, 15, 0, 2, 4, 6, 8, 10, 12, 14, 1, 3, 5, 7, 9, 11, 13, 15,
};

//////////////////////////////////////////////////////////////////////////////
// Common kernels
//////////////////////////////////////////////////////////////////////////////

static INLINE __m128i blend_4(const uint8_t* src0, const uint8_t* src1, const __m128i* v_m0_w, const __m128i* v_m1_w) {
    const __m128i v_s0_b = xx_loadl_32(src0);
    const __m128i v_s1_b = xx_loadl_32(src1);
    const __m128i v_s0_w = _mm_cvtepu8_epi16(v_s0_b);
    const __m128i v_s1_w = _mm_cvtepu8_epi16(v_s1_b);

    const __m128i v_p0_w  = _mm_mullo_epi16(v_s0_w, *v_m0_w);
    const __m128i v_p1_w  = _mm_mullo_epi16(v_s1_w, *v_m1_w);
    const __m128i v_sum_w = _mm_add_epi16(v_p0_w, v_p1_w);
    const __m128i v_res_w = xx_roundn_epu16(v_sum_w, AOM_BLEND_A64_ROUND_BITS);

    return v_res_w;
}

static INLINE __m128i blend_8(const uint8_t* src0, const uint8_t* src1, const __m128i* v_m0_w, const __m128i* v_m1_w) {
    const __m128i v_s0_b = xx_loadl_64(src0);
    const __m128i v_s1_b = xx_loadl_64(src1);
    const __m128i v_s0_w = _mm_cvtepu8_epi16(v_s0_b);
    const __m128i v_s1_w = _mm_cvtepu8_epi16(v_s1_b);

    const __m128i v_p0_w = _mm_mullo_epi16(v_s0_w, *v_m0_w);
    const __m128i v_p1_w = _mm_mullo_epi16(v_s1_w, *v_m1_w);

    const __m128i v_sum_w = _mm_add_epi16(v_p0_w, v_p1_w);

    const __m128i v_res_w = xx_roundn_epu16(v_sum_w, AOM_BLEND_A64_ROUND_BITS);

    return v_res_w;
}

static INLINE __m128i blend_4_u8(const uint8_t* src0, const uint8_t* src1, const __m128i* v_m0_b, const __m128i* v_m1_b,
                                 const __m128i* rounding) {
    const __m128i v_s0_b = xx_loadl_32(src0);
    const __m128i v_s1_b = xx_loadl_32(src1);

    const __m128i v_p0_w = _mm_maddubs_epi16(_mm_unpacklo_epi8(v_s0_b, v_s1_b), _mm_unpacklo_epi8(*v_m0_b, *v_m1_b));

    const __m128i v_res_w = _mm_mulhrs_epi16(v_p0_w, *rounding);
    const __m128i v_res   = _mm_packus_epi16(v_res_w, v_res_w);
    return v_res;
}

static INLINE __m128i blend_8_u8(const uint8_t* src0, const uint8_t* src1, const __m128i* v_m0_b, const __m128i* v_m1_b,
                                 const __m128i* rounding) {
    const __m128i v_s0_b = xx_loadl_64(src0);
    const __m128i v_s1_b = xx_loadl_64(src1);

    const __m128i v_p0_w = _mm_maddubs_epi16(_mm_unpacklo_epi8(v_s0_b, v_s1_b), _mm_unpacklo_epi8(*v_m0_b, *v_m1_b));

    const __m128i v_res_w = _mm_mulhrs_epi16(v_p0_w, *rounding);
    const __m128i v_res   = _mm_packus_epi16(v_res_w, v_res_w);
    return v_res;
}

static INLINE __m128i blend_16_u8(const uint8_t* src0, const uint8_t* src1, const __m128i* v_m0_b,
                                  const __m128i* v_m1_b, const __m128i* rounding) {
    const __m128i v_s0_b = xx_loadu_128(src0);
    const __m128i v_s1_b = xx_loadu_128(src1);

    const __m128i v_p0_w = _mm_maddubs_epi16(_mm_unpacklo_epi8(v_s0_b, v_s1_b), _mm_unpacklo_epi8(*v_m0_b, *v_m1_b));
    const __m128i v_p1_w = _mm_maddubs_epi16(_mm_unpackhi_epi8(v_s0_b, v_s1_b), _mm_unpackhi_epi8(*v_m0_b, *v_m1_b));

    const __m128i v_res0_w = _mm_mulhrs_epi16(v_p0_w, *rounding);
    const __m128i v_res1_w = _mm_mulhrs_epi16(v_p1_w, *rounding);
    const __m128i v_res    = _mm_packus_epi16(v_res0_w, v_res1_w);
    return v_res;
}

typedef __m128i (*BlendUnitFn)(const uint16_t* src0, const uint16_t* src1, const __m128i v_m0_w, const __m128i v_m1_w);

static INLINE __m128i blend_4_b10(const uint16_t* src0, const uint16_t* src1, const __m128i v_m0_w,
                                  const __m128i v_m1_w) {
    const __m128i v_s0_w = xx_loadl_64(src0);
    const __m128i v_s1_w = xx_loadl_64(src1);

    const __m128i v_p0_w = _mm_mullo_epi16(v_s0_w, v_m0_w);
    const __m128i v_p1_w = _mm_mullo_epi16(v_s1_w, v_m1_w);

    const __m128i v_sum_w = _mm_add_epi16(v_p0_w, v_p1_w);

    const __m128i v_res_w = xx_roundn_epu16(v_sum_w, AOM_BLEND_A64_ROUND_BITS);

    return v_res_w;
}

static INLINE __m128i blend_8_b10(const uint16_t* src0, const uint16_t* src1, const __m128i v_m0_w,
                                  const __m128i v_m1_w) {
    const __m128i v_s0_w = xx_loadu_128(src0);
    const __m128i v_s1_w = xx_loadu_128(src1);

    const __m128i v_p0_w = _mm_mullo_epi16(v_s0_w, v_m0_w);
    const __m128i v_p1_w = _mm_mullo_epi16(v_s1_w, v_m1_w);

    const __m128i v_sum_w = _mm_add_epi16(v_p0_w, v_p1_w);

    const __m128i v_res_w = xx_roundn_epu16(v_sum_w, AOM_BLEND_A64_ROUND_BITS);

    return v_res_w;
}

static INLINE __m128i blend_4_b12(const uint16_t* src0, const uint16_t* src1, const __m128i v_m0_w,
                                  const __m128i v_m1_w) {
    const __m128i v_s0_w = xx_loadl_64(src0);
    const __m128i v_s1_w = xx_loadl_64(src1);

    // Interleave
    const __m128i v_m01_w = _mm_unpacklo_epi16(v_m0_w, v_m1_w);
    const __m128i v_s01_w = _mm_unpacklo_epi16(v_s0_w, v_s1_w);

    // Multiply-Add
    const __m128i v_sum_d = _mm_madd_epi16(v_s01_w, v_m01_w);

    // Scale
    const __m128i v_ssum_d = _mm_srli_epi32(v_sum_d, AOM_BLEND_A64_ROUND_BITS - 1);

    // Pack
    const __m128i v_pssum_d = _mm_packs_epi32(v_ssum_d, v_ssum_d);

    // Round
    const __m128i v_res_w = xx_round_epu16(v_pssum_d);

    return v_res_w;
}

static INLINE __m128i blend_8_b12(const uint16_t* src0, const uint16_t* src1, const __m128i v_m0_w,
                                  const __m128i v_m1_w) {
    const __m128i v_s0_w = xx_loadu_128(src0);
    const __m128i v_s1_w = xx_loadu_128(src1);

    // Interleave
    const __m128i v_m01l_w = _mm_unpacklo_epi16(v_m0_w, v_m1_w);
    const __m128i v_m01h_w = _mm_unpackhi_epi16(v_m0_w, v_m1_w);
    const __m128i v_s01l_w = _mm_unpacklo_epi16(v_s0_w, v_s1_w);
    const __m128i v_s01h_w = _mm_unpackhi_epi16(v_s0_w, v_s1_w);

    // Multiply-Add
    const __m128i v_suml_d = _mm_madd_epi16(v_s01l_w, v_m01l_w);
    const __m128i v_sumh_d = _mm_madd_epi16(v_s01h_w, v_m01h_w);

    // Scale
    const __m128i v_ssuml_d = _mm_srli_epi32(v_suml_d, AOM_BLEND_A64_ROUND_BITS - 1);
    const __m128i v_ssumh_d = _mm_srli_epi32(v_sumh_d, AOM_BLEND_A64_ROUND_BITS - 1);

    // Pack
    const __m128i v_pssum_d = _mm_packs_epi32(v_ssuml_d, v_ssumh_d);

    // Round
    const __m128i v_res_w = xx_round_epu16(v_pssum_d);

    return v_res_w;
}

/*Functions from convolve_avx2.c*/
static INLINE void blend_a64_d16_mask_w4_sse41(uint8_t* dst, const CONV_BUF_TYPE* src0, const CONV_BUF_TYPE* src1,
                                               const __m128i* m, const __m128i* v_round_offset, const __m128i* v_maxval,
                                               int shift) {
    const __m128i max_minus_m   = _mm_sub_epi16(*v_maxval, *m);
    const __m128i s0            = xx_loadl_64(src0);
    const __m128i s1            = xx_loadl_64(src1);
    const __m128i s0_s1         = _mm_unpacklo_epi16(s0, s1);
    const __m128i m_max_minus_m = _mm_unpacklo_epi16(*m, max_minus_m);
    const __m128i res_a         = _mm_madd_epi16(s0_s1, m_max_minus_m);
    const __m128i res_c         = _mm_sub_epi32(res_a, *v_round_offset);
    const __m128i res_d         = _mm_srai_epi32(res_c, shift);
    const __m128i res_e         = _mm_packs_epi32(res_d, res_d);
    const __m128i res           = _mm_packus_epi16(res_e, res_e);

    xx_storel_32(dst, res);
}

static INLINE void blend_a64_d16_mask_w8_sse41(uint8_t* dst, const CONV_BUF_TYPE* src0, const CONV_BUF_TYPE* src1,
                                               const __m128i* m, const __m128i* v_round_offset, const __m128i* v_maxval,
                                               int shift) {
    const __m128i max_minus_m = _mm_sub_epi16(*v_maxval, *m);
    const __m128i s0          = xx_loadu_128(src0);
    const __m128i s1          = xx_loadu_128(src1);
    __m128i       res_lo      = _mm_madd_epi16(_mm_unpacklo_epi16(s0, s1), _mm_unpacklo_epi16(*m, max_minus_m));
    __m128i       res_hi      = _mm_madd_epi16(_mm_unpackhi_epi16(s0, s1), _mm_unpackhi_epi16(*m, max_minus_m));
    res_lo                    = _mm_srai_epi32(_mm_sub_epi32(res_lo, *v_round_offset), shift);
    res_hi                    = _mm_srai_epi32(_mm_sub_epi32(res_hi, *v_round_offset), shift);
    const __m128i res_e       = _mm_packs_epi32(res_lo, res_hi);
    const __m128i res         = _mm_packus_epi16(res_e, res_e);

    _mm_storel_epi64((__m128i*)(dst), res);
}

static INLINE void aom_lowbd_blend_a64_d16_mask_subw0_subh0_w4_sse4_1(uint8_t* dst, uint32_t dst_stride,
                                                                      const CONV_BUF_TYPE* src0, uint32_t src0_stride,
                                                                      const CONV_BUF_TYPE* src1, uint32_t src1_stride,
                                                                      const uint8_t* mask, uint32_t mask_stride, int h,
                                                                      const __m128i* round_offset, int shift) {
    const __m128i v_maxval = _mm_set1_epi16(AOM_BLEND_A64_MAX_ALPHA);
    for (int i = 0; i < h; ++i) {
        const __m128i m0 = xx_loadl_32(mask);
        const __m128i m  = _mm_cvtepu8_epi16(m0);

        blend_a64_d16_mask_w4_sse41(dst, src0, src1, &m, round_offset, &v_maxval, shift);
        mask += mask_stride;
        dst += dst_stride;
        src0 += src0_stride;
        src1 += src1_stride;
    }
}

static INLINE void aom_lowbd_blend_a64_d16_mask_subw0_subh0_w8_sse4_1(uint8_t* dst, uint32_t dst_stride,
                                                                      const CONV_BUF_TYPE* src0, uint32_t src0_stride,
                                                                      const CONV_BUF_TYPE* src1, uint32_t src1_stride,
                                                                      const uint8_t* mask, uint32_t mask_stride, int h,
                                                                      const __m128i* round_offset, int shift) {
    const __m128i v_maxval = _mm_set1_epi16(AOM_BLEND_A64_MAX_ALPHA);
    for (int i = 0; i < h; ++i) {
        const __m128i m0 = xx_loadl_64(mask);
        const __m128i m  = _mm_cvtepu8_epi16(m0);
        blend_a64_d16_mask_w8_sse41(dst, src0, src1, &m, round_offset, &v_maxval, shift);
        mask += mask_stride;
        dst += dst_stride;
        src0 += src0_stride;
        src1 += src1_stride;
    }
}

static INLINE void aom_lowbd_blend_a64_d16_mask_subw1_subh1_w4_sse4_1(uint8_t* dst, uint32_t dst_stride,
                                                                      const CONV_BUF_TYPE* src0, uint32_t src0_stride,
                                                                      const CONV_BUF_TYPE* src1, uint32_t src1_stride,
                                                                      const uint8_t* mask, uint32_t mask_stride, int h,
                                                                      const __m128i* round_offset, int shift) {
    const __m128i v_maxval = _mm_set1_epi16(AOM_BLEND_A64_MAX_ALPHA);
    const __m128i one_b    = _mm_set1_epi8(1);
    const __m128i two_w    = _mm_set1_epi16(2);
    for (int i = 0; i < h; ++i) {
        const __m128i m_i0     = xx_loadl_64(mask);
        const __m128i m_i1     = xx_loadl_64(mask + mask_stride);
        const __m128i m_ac     = _mm_adds_epu8(m_i0, m_i1);
        const __m128i m_acbd   = _mm_maddubs_epi16(m_ac, one_b);
        const __m128i m_acbd_2 = _mm_add_epi16(m_acbd, two_w);
        const __m128i m        = _mm_srli_epi16(m_acbd_2, 2);

        blend_a64_d16_mask_w4_sse41(dst, src0, src1, &m, round_offset, &v_maxval, shift);
        mask += mask_stride << 1;
        dst += dst_stride;
        src0 += src0_stride;
        src1 += src1_stride;
    }
}

static INLINE void aom_lowbd_blend_a64_d16_mask_subw1_subh1_w8_sse4_1(uint8_t* dst, uint32_t dst_stride,
                                                                      const CONV_BUF_TYPE* src0, uint32_t src0_stride,
                                                                      const CONV_BUF_TYPE* src1, uint32_t src1_stride,
                                                                      const uint8_t* mask, uint32_t mask_stride, int h,
                                                                      const __m128i* round_offset, int shift) {
    const __m128i v_maxval = _mm_set1_epi16(AOM_BLEND_A64_MAX_ALPHA);
    const __m128i one_b    = _mm_set1_epi8(1);
    const __m128i two_w    = _mm_set1_epi16(2);
    for (int i = 0; i < h; ++i) {
        const __m128i m_i0     = xx_loadu_128(mask);
        const __m128i m_i1     = xx_loadu_128(mask + mask_stride);
        const __m128i m_ac     = _mm_adds_epu8(m_i0, m_i1);
        const __m128i m_acbd   = _mm_maddubs_epi16(m_ac, one_b);
        const __m128i m_acbd_2 = _mm_add_epi16(m_acbd, two_w);
        const __m128i m        = _mm_srli_epi16(m_acbd_2, 2);

        blend_a64_d16_mask_w8_sse41(dst, src0, src1, &m, round_offset, &v_maxval, shift);
        mask += mask_stride << 1;
        dst += dst_stride;
        src0 += src0_stride;
        src1 += src1_stride;
    }
}

static INLINE void aom_lowbd_blend_a64_d16_mask_subw1_subh0_w4_sse4_1(uint8_t* dst, uint32_t dst_stride,
                                                                      const CONV_BUF_TYPE* src0, uint32_t src0_stride,
                                                                      const CONV_BUF_TYPE* src1, uint32_t src1_stride,
                                                                      const uint8_t* mask, uint32_t mask_stride, int h,
                                                                      const __m128i* round_offset, int shift) {
    const __m128i v_maxval = _mm_set1_epi16(AOM_BLEND_A64_MAX_ALPHA);
    const __m128i one_b    = _mm_set1_epi8(1);
    const __m128i zeros    = _mm_setzero_si128();
    for (int i = 0; i < h; ++i) {
        const __m128i m_i0 = xx_loadl_64(mask);
        const __m128i m_ac = _mm_maddubs_epi16(m_i0, one_b);
        const __m128i m    = _mm_avg_epu16(m_ac, zeros);

        blend_a64_d16_mask_w4_sse41(dst, src0, src1, &m, round_offset, &v_maxval, shift);
        mask += mask_stride;
        dst += dst_stride;
        src0 += src0_stride;
        src1 += src1_stride;
    }
}

static INLINE void aom_lowbd_blend_a64_d16_mask_subw1_subh0_w8_sse4_1(uint8_t* dst, uint32_t dst_stride,
                                                                      const CONV_BUF_TYPE* src0, uint32_t src0_stride,
                                                                      const CONV_BUF_TYPE* src1, uint32_t src1_stride,
                                                                      const uint8_t* mask, uint32_t mask_stride, int h,
                                                                      const __m128i* round_offset, int shift) {
    const __m128i v_maxval = _mm_set1_epi16(AOM_BLEND_A64_MAX_ALPHA);
    const __m128i one_b    = _mm_set1_epi8(1);
    const __m128i zeros    = _mm_setzero_si128();
    for (int i = 0; i < h; ++i) {
        const __m128i m_i0 = xx_loadu_128(mask);
        const __m128i m_ac = _mm_maddubs_epi16(m_i0, one_b);
        const __m128i m    = _mm_avg_epu16(m_ac, zeros);

        blend_a64_d16_mask_w8_sse41(dst, src0, src1, &m, round_offset, &v_maxval, shift);
        mask += mask_stride;
        dst += dst_stride;
        src0 += src0_stride;
        src1 += src1_stride;
    }
}

static INLINE void aom_lowbd_blend_a64_d16_mask_subw0_subh1_w4_sse4_1(uint8_t* dst, uint32_t dst_stride,
                                                                      const CONV_BUF_TYPE* src0, uint32_t src0_stride,
                                                                      const CONV_BUF_TYPE* src1, uint32_t src1_stride,
                                                                      const uint8_t* mask, uint32_t mask_stride, int h,
                                                                      const __m128i* round_offset, int shift) {
    const __m128i v_maxval = _mm_set1_epi16(AOM_BLEND_A64_MAX_ALPHA);
    const __m128i zeros    = _mm_setzero_si128();
    for (int i = 0; i < h; ++i) {
        const __m128i m_i0 = xx_loadl_64(mask);
        const __m128i m_i1 = xx_loadl_64(mask + mask_stride);
        const __m128i m_ac = _mm_adds_epu8(m_i0, m_i1);
        const __m128i m    = _mm_cvtepu8_epi16(_mm_avg_epu8(m_ac, zeros));

        blend_a64_d16_mask_w4_sse41(dst, src0, src1, &m, round_offset, &v_maxval, shift);
        mask += mask_stride << 1;
        dst += dst_stride;
        src0 += src0_stride;
        src1 += src1_stride;
    }
}

static INLINE void aom_lowbd_blend_a64_d16_mask_subw0_subh1_w8_sse4_1(uint8_t* dst, uint32_t dst_stride,
                                                                      const CONV_BUF_TYPE* src0, uint32_t src0_stride,
                                                                      const CONV_BUF_TYPE* src1, uint32_t src1_stride,
                                                                      const uint8_t* mask, uint32_t mask_stride, int h,
                                                                      const __m128i* round_offset, int shift) {
    const __m128i v_maxval = _mm_set1_epi16(AOM_BLEND_A64_MAX_ALPHA);
    const __m128i zeros    = _mm_setzero_si128();
    for (int i = 0; i < h; ++i) {
        const __m128i m_i0 = xx_loadl_64(mask);
        const __m128i m_i1 = xx_loadl_64(mask + mask_stride);
        const __m128i m_ac = _mm_adds_epu8(m_i0, m_i1);
        const __m128i m    = _mm_cvtepu8_epi16(_mm_avg_epu8(m_ac, zeros));

        blend_a64_d16_mask_w8_sse41(dst, src0, src1, &m, round_offset, &v_maxval, shift);
        mask += mask_stride << 1;
        dst += dst_stride;
        src0 += src0_stride;
        src1 += src1_stride;
    }
}
#endif // AOM_AOM_DSP_X86_BLEND_SSE4_H_
