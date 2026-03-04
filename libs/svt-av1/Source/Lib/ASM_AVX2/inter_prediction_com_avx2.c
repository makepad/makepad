/*
 * Copyright (c) 2017, Alliance for Open Media. All rights reserved
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
 * Media Patent License 1.0 was not distributed with this source code in the
 * PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
 */
#include <immintrin.h>
#include "synonyms.h"
#include "synonyms_avx2.h"

#include "definitions.h"

static INLINE __m256i calc_mask_d16_avx2(const __m256i* data_src0, const __m256i* data_src1, const __m256i* round_const,
                                         const __m256i* mask_base_16, const __m256i* clip_diff, int round) {
    const __m256i diffa       = _mm256_subs_epu16(*data_src0, *data_src1);
    const __m256i diffb       = _mm256_subs_epu16(*data_src1, *data_src0);
    const __m256i diff        = _mm256_max_epu16(diffa, diffb);
    const __m256i diff_round  = _mm256_srli_epi16(_mm256_adds_epu16(diff, *round_const), round);
    const __m256i diff_factor = _mm256_srli_epi16(diff_round, DIFF_FACTOR_LOG2);
    const __m256i diff_mask   = _mm256_adds_epi16(diff_factor, *mask_base_16);
    const __m256i diff_clamp  = _mm256_min_epi16(diff_mask, *clip_diff);
    return diff_clamp;
}

static INLINE __m256i calc_mask_d16_inv_avx2(const __m256i* data_src0, const __m256i* data_src1,
                                             const __m256i* round_const, const __m256i* mask_base_16,
                                             const __m256i* clip_diff, int round) {
    const __m256i diffa         = _mm256_subs_epu16(*data_src0, *data_src1);
    const __m256i diffb         = _mm256_subs_epu16(*data_src1, *data_src0);
    const __m256i diff          = _mm256_max_epu16(diffa, diffb);
    const __m256i diff_round    = _mm256_srli_epi16(_mm256_adds_epu16(diff, *round_const), round);
    const __m256i diff_factor   = _mm256_srli_epi16(diff_round, DIFF_FACTOR_LOG2);
    const __m256i diff_mask     = _mm256_adds_epi16(diff_factor, *mask_base_16);
    const __m256i diff_clamp    = _mm256_min_epi16(diff_mask, *clip_diff);
    const __m256i diff_const_16 = _mm256_sub_epi16(*clip_diff, diff_clamp);
    return diff_const_16;
}

static INLINE void build_compound_diffwtd_mask_d16_avx2(uint8_t* mask, const CONV_BUF_TYPE* src0, int src0_stride,
                                                        const CONV_BUF_TYPE* src1, int src1_stride, int h, int w,
                                                        int shift) {
    const int     mask_base = 38;
    const __m256i _r        = _mm256_set1_epi16((1 << shift) >> 1);
    const __m256i y38       = _mm256_set1_epi16(mask_base);
    const __m256i y64       = _mm256_set1_epi16(AOM_BLEND_A64_MAX_ALPHA);
    int           i         = 0;
    if (w == 4) {
        do {
            const __m128i s0_a = xx_loadl_64(src0);
            const __m128i s0_b = xx_loadl_64(src0 + src0_stride);
            const __m128i s0_c = xx_loadl_64(src0 + src0_stride * 2);
            const __m128i s0_d = xx_loadl_64(src0 + src0_stride * 3);
            const __m128i s1_a = xx_loadl_64(src1);
            const __m128i s1_b = xx_loadl_64(src1 + src1_stride);
            const __m128i s1_c = xx_loadl_64(src1 + src1_stride * 2);
            const __m128i s1_d = xx_loadl_64(src1 + src1_stride * 3);
            const __m256i s0   = yy_set_m128i(_mm_unpacklo_epi64(s0_c, s0_d), _mm_unpacklo_epi64(s0_a, s0_b));
            const __m256i s1   = yy_set_m128i(_mm_unpacklo_epi64(s1_c, s1_d), _mm_unpacklo_epi64(s1_a, s1_b));
            const __m256i m16  = calc_mask_d16_avx2(&s0, &s1, &_r, &y38, &y64, shift);
            const __m256i m8   = _mm256_packus_epi16(m16, _mm256_setzero_si256());
            xx_storeu_128(mask, _mm256_castsi256_si128(_mm256_permute4x64_epi64(m8, 0xd8)));
            src0 += src0_stride << 2;
            src1 += src1_stride << 2;
            mask += 16;
            i += 4;
        } while (i < h);
    } else if (w == 8) {
        do {
            const __m256i s0_a_b  = yy_loadu2_128(src0 + src0_stride, src0);
            const __m256i s0_c_d  = yy_loadu2_128(src0 + src0_stride * 3, src0 + src0_stride * 2);
            const __m256i s1_a_b  = yy_loadu2_128(src1 + src1_stride, src1);
            const __m256i s1_c_d  = yy_loadu2_128(src1 + src1_stride * 3, src1 + src1_stride * 2);
            const __m256i m16_a_b = calc_mask_d16_avx2(&s0_a_b, &s1_a_b, &_r, &y38, &y64, shift);
            const __m256i m16_c_d = calc_mask_d16_avx2(&s0_c_d, &s1_c_d, &_r, &y38, &y64, shift);
            const __m256i m8      = _mm256_packus_epi16(m16_a_b, m16_c_d);
            yy_storeu_256(mask, _mm256_permute4x64_epi64(m8, 0xd8));
            src0 += src0_stride << 2;
            src1 += src1_stride << 2;
            mask += 32;
            i += 4;
        } while (i < h);
    } else if (w == 16) {
        do {
            const __m256i s0_a  = yy_loadu_256(src0);
            const __m256i s0_b  = yy_loadu_256(src0 + src0_stride);
            const __m256i s1_a  = yy_loadu_256(src1);
            const __m256i s1_b  = yy_loadu_256(src1 + src1_stride);
            const __m256i m16_a = calc_mask_d16_avx2(&s0_a, &s1_a, &_r, &y38, &y64, shift);
            const __m256i m16_b = calc_mask_d16_avx2(&s0_b, &s1_b, &_r, &y38, &y64, shift);
            const __m256i m8    = _mm256_packus_epi16(m16_a, m16_b);
            yy_storeu_256(mask, _mm256_permute4x64_epi64(m8, 0xd8));
            src0 += src0_stride << 1;
            src1 += src1_stride << 1;
            mask += 32;
            i += 2;
        } while (i < h);
    } else if (w == 32) {
        do {
            const __m256i s0_a  = yy_loadu_256(src0);
            const __m256i s0_b  = yy_loadu_256(src0 + 16);
            const __m256i s1_a  = yy_loadu_256(src1);
            const __m256i s1_b  = yy_loadu_256(src1 + 16);
            const __m256i m16_a = calc_mask_d16_avx2(&s0_a, &s1_a, &_r, &y38, &y64, shift);
            const __m256i m16_b = calc_mask_d16_avx2(&s0_b, &s1_b, &_r, &y38, &y64, shift);
            const __m256i m8    = _mm256_packus_epi16(m16_a, m16_b);
            yy_storeu_256(mask, _mm256_permute4x64_epi64(m8, 0xd8));
            src0 += src0_stride;
            src1 += src1_stride;
            mask += 32;
            i += 1;
        } while (i < h);
    } else if (w == 64) {
        do {
            const __m256i s0_a   = yy_loadu_256(src0);
            const __m256i s0_b   = yy_loadu_256(src0 + 16);
            const __m256i s0_c   = yy_loadu_256(src0 + 32);
            const __m256i s0_d   = yy_loadu_256(src0 + 48);
            const __m256i s1_a   = yy_loadu_256(src1);
            const __m256i s1_b   = yy_loadu_256(src1 + 16);
            const __m256i s1_c   = yy_loadu_256(src1 + 32);
            const __m256i s1_d   = yy_loadu_256(src1 + 48);
            const __m256i m16_a  = calc_mask_d16_avx2(&s0_a, &s1_a, &_r, &y38, &y64, shift);
            const __m256i m16_b  = calc_mask_d16_avx2(&s0_b, &s1_b, &_r, &y38, &y64, shift);
            const __m256i m16_c  = calc_mask_d16_avx2(&s0_c, &s1_c, &_r, &y38, &y64, shift);
            const __m256i m16_d  = calc_mask_d16_avx2(&s0_d, &s1_d, &_r, &y38, &y64, shift);
            const __m256i m8_a_b = _mm256_packus_epi16(m16_a, m16_b);
            const __m256i m8_c_d = _mm256_packus_epi16(m16_c, m16_d);
            yy_storeu_256(mask, _mm256_permute4x64_epi64(m8_a_b, 0xd8));
            yy_storeu_256(mask + 32, _mm256_permute4x64_epi64(m8_c_d, 0xd8));
            src0 += src0_stride;
            src1 += src1_stride;
            mask += 64;
            i += 1;
        } while (i < h);
    } else {
        do {
            const __m256i s0_a   = yy_loadu_256(src0);
            const __m256i s0_b   = yy_loadu_256(src0 + 16);
            const __m256i s0_c   = yy_loadu_256(src0 + 32);
            const __m256i s0_d   = yy_loadu_256(src0 + 48);
            const __m256i s0_e   = yy_loadu_256(src0 + 64);
            const __m256i s0_f   = yy_loadu_256(src0 + 80);
            const __m256i s0_g   = yy_loadu_256(src0 + 96);
            const __m256i s0_h   = yy_loadu_256(src0 + 112);
            const __m256i s1_a   = yy_loadu_256(src1);
            const __m256i s1_b   = yy_loadu_256(src1 + 16);
            const __m256i s1_c   = yy_loadu_256(src1 + 32);
            const __m256i s1_d   = yy_loadu_256(src1 + 48);
            const __m256i s1_e   = yy_loadu_256(src1 + 64);
            const __m256i s1_f   = yy_loadu_256(src1 + 80);
            const __m256i s1_g   = yy_loadu_256(src1 + 96);
            const __m256i s1_h   = yy_loadu_256(src1 + 112);
            const __m256i m16_a  = calc_mask_d16_avx2(&s0_a, &s1_a, &_r, &y38, &y64, shift);
            const __m256i m16_b  = calc_mask_d16_avx2(&s0_b, &s1_b, &_r, &y38, &y64, shift);
            const __m256i m16_c  = calc_mask_d16_avx2(&s0_c, &s1_c, &_r, &y38, &y64, shift);
            const __m256i m16_d  = calc_mask_d16_avx2(&s0_d, &s1_d, &_r, &y38, &y64, shift);
            const __m256i m16_e  = calc_mask_d16_avx2(&s0_e, &s1_e, &_r, &y38, &y64, shift);
            const __m256i m16_f  = calc_mask_d16_avx2(&s0_f, &s1_f, &_r, &y38, &y64, shift);
            const __m256i m16_g  = calc_mask_d16_avx2(&s0_g, &s1_g, &_r, &y38, &y64, shift);
            const __m256i m16_h  = calc_mask_d16_avx2(&s0_h, &s1_h, &_r, &y38, &y64, shift);
            const __m256i m8_a_b = _mm256_packus_epi16(m16_a, m16_b);
            const __m256i m8_c_d = _mm256_packus_epi16(m16_c, m16_d);
            const __m256i m8_e_f = _mm256_packus_epi16(m16_e, m16_f);
            const __m256i m8_g_h = _mm256_packus_epi16(m16_g, m16_h);
            yy_storeu_256(mask, _mm256_permute4x64_epi64(m8_a_b, 0xd8));
            yy_storeu_256(mask + 32, _mm256_permute4x64_epi64(m8_c_d, 0xd8));
            yy_storeu_256(mask + 64, _mm256_permute4x64_epi64(m8_e_f, 0xd8));
            yy_storeu_256(mask + 96, _mm256_permute4x64_epi64(m8_g_h, 0xd8));
            src0 += src0_stride;
            src1 += src1_stride;
            mask += 128;
            i += 1;
        } while (i < h);
    }
}

static INLINE void build_compound_diffwtd_mask_d16_inv_avx2(uint8_t* mask, const CONV_BUF_TYPE* src0, int src0_stride,
                                                            const CONV_BUF_TYPE* src1, int src1_stride, int h, int w,
                                                            int shift) {
    const int     mask_base = 38;
    const __m256i _r        = _mm256_set1_epi16((1 << shift) >> 1);
    const __m256i y38       = _mm256_set1_epi16(mask_base);
    const __m256i y64       = _mm256_set1_epi16(AOM_BLEND_A64_MAX_ALPHA);
    int           i         = 0;
    if (w == 4) {
        do {
            const __m128i s0_a = xx_loadl_64(src0);
            const __m128i s0_b = xx_loadl_64(src0 + src0_stride);
            const __m128i s0_c = xx_loadl_64(src0 + src0_stride * 2);
            const __m128i s0_d = xx_loadl_64(src0 + src0_stride * 3);
            const __m128i s1_a = xx_loadl_64(src1);
            const __m128i s1_b = xx_loadl_64(src1 + src1_stride);
            const __m128i s1_c = xx_loadl_64(src1 + src1_stride * 2);
            const __m128i s1_d = xx_loadl_64(src1 + src1_stride * 3);
            const __m256i s0   = yy_set_m128i(_mm_unpacklo_epi64(s0_c, s0_d), _mm_unpacklo_epi64(s0_a, s0_b));
            const __m256i s1   = yy_set_m128i(_mm_unpacklo_epi64(s1_c, s1_d), _mm_unpacklo_epi64(s1_a, s1_b));
            const __m256i m16  = calc_mask_d16_inv_avx2(&s0, &s1, &_r, &y38, &y64, shift);
            const __m256i m8   = _mm256_packus_epi16(m16, _mm256_setzero_si256());
            xx_storeu_128(mask, _mm256_castsi256_si128(_mm256_permute4x64_epi64(m8, 0xd8)));
            src0 += src0_stride << 2;
            src1 += src1_stride << 2;
            mask += 16;
            i += 4;
        } while (i < h);
    } else if (w == 8) {
        do {
            const __m256i s0_a_b  = yy_loadu2_128(src0 + src0_stride, src0);
            const __m256i s0_c_d  = yy_loadu2_128(src0 + src0_stride * 3, src0 + src0_stride * 2);
            const __m256i s1_a_b  = yy_loadu2_128(src1 + src1_stride, src1);
            const __m256i s1_c_d  = yy_loadu2_128(src1 + src1_stride * 3, src1 + src1_stride * 2);
            const __m256i m16_a_b = calc_mask_d16_inv_avx2(&s0_a_b, &s1_a_b, &_r, &y38, &y64, shift);
            const __m256i m16_c_d = calc_mask_d16_inv_avx2(&s0_c_d, &s1_c_d, &_r, &y38, &y64, shift);
            const __m256i m8      = _mm256_packus_epi16(m16_a_b, m16_c_d);
            yy_storeu_256(mask, _mm256_permute4x64_epi64(m8, 0xd8));
            src0 += src0_stride << 2;
            src1 += src1_stride << 2;
            mask += 32;
            i += 4;
        } while (i < h);
    } else if (w == 16) {
        do {
            const __m256i s0_a  = yy_loadu_256(src0);
            const __m256i s0_b  = yy_loadu_256(src0 + src0_stride);
            const __m256i s1_a  = yy_loadu_256(src1);
            const __m256i s1_b  = yy_loadu_256(src1 + src1_stride);
            const __m256i m16_a = calc_mask_d16_inv_avx2(&s0_a, &s1_a, &_r, &y38, &y64, shift);
            const __m256i m16_b = calc_mask_d16_inv_avx2(&s0_b, &s1_b, &_r, &y38, &y64, shift);
            const __m256i m8    = _mm256_packus_epi16(m16_a, m16_b);
            yy_storeu_256(mask, _mm256_permute4x64_epi64(m8, 0xd8));
            src0 += src0_stride << 1;
            src1 += src1_stride << 1;
            mask += 32;
            i += 2;
        } while (i < h);
    } else if (w == 32) {
        do {
            const __m256i s0_a  = yy_loadu_256(src0);
            const __m256i s0_b  = yy_loadu_256(src0 + 16);
            const __m256i s1_a  = yy_loadu_256(src1);
            const __m256i s1_b  = yy_loadu_256(src1 + 16);
            const __m256i m16_a = calc_mask_d16_inv_avx2(&s0_a, &s1_a, &_r, &y38, &y64, shift);
            const __m256i m16_b = calc_mask_d16_inv_avx2(&s0_b, &s1_b, &_r, &y38, &y64, shift);
            const __m256i m8    = _mm256_packus_epi16(m16_a, m16_b);
            yy_storeu_256(mask, _mm256_permute4x64_epi64(m8, 0xd8));
            src0 += src0_stride;
            src1 += src1_stride;
            mask += 32;
            i += 1;
        } while (i < h);
    } else if (w == 64) {
        do {
            const __m256i s0_a   = yy_loadu_256(src0);
            const __m256i s0_b   = yy_loadu_256(src0 + 16);
            const __m256i s0_c   = yy_loadu_256(src0 + 32);
            const __m256i s0_d   = yy_loadu_256(src0 + 48);
            const __m256i s1_a   = yy_loadu_256(src1);
            const __m256i s1_b   = yy_loadu_256(src1 + 16);
            const __m256i s1_c   = yy_loadu_256(src1 + 32);
            const __m256i s1_d   = yy_loadu_256(src1 + 48);
            const __m256i m16_a  = calc_mask_d16_inv_avx2(&s0_a, &s1_a, &_r, &y38, &y64, shift);
            const __m256i m16_b  = calc_mask_d16_inv_avx2(&s0_b, &s1_b, &_r, &y38, &y64, shift);
            const __m256i m16_c  = calc_mask_d16_inv_avx2(&s0_c, &s1_c, &_r, &y38, &y64, shift);
            const __m256i m16_d  = calc_mask_d16_inv_avx2(&s0_d, &s1_d, &_r, &y38, &y64, shift);
            const __m256i m8_a_b = _mm256_packus_epi16(m16_a, m16_b);
            const __m256i m8_c_d = _mm256_packus_epi16(m16_c, m16_d);
            yy_storeu_256(mask, _mm256_permute4x64_epi64(m8_a_b, 0xd8));
            yy_storeu_256(mask + 32, _mm256_permute4x64_epi64(m8_c_d, 0xd8));
            src0 += src0_stride;
            src1 += src1_stride;
            mask += 64;
            i += 1;
        } while (i < h);
    } else {
        do {
            const __m256i s0_a   = yy_loadu_256(src0);
            const __m256i s0_b   = yy_loadu_256(src0 + 16);
            const __m256i s0_c   = yy_loadu_256(src0 + 32);
            const __m256i s0_d   = yy_loadu_256(src0 + 48);
            const __m256i s0_e   = yy_loadu_256(src0 + 64);
            const __m256i s0_f   = yy_loadu_256(src0 + 80);
            const __m256i s0_g   = yy_loadu_256(src0 + 96);
            const __m256i s0_h   = yy_loadu_256(src0 + 112);
            const __m256i s1_a   = yy_loadu_256(src1);
            const __m256i s1_b   = yy_loadu_256(src1 + 16);
            const __m256i s1_c   = yy_loadu_256(src1 + 32);
            const __m256i s1_d   = yy_loadu_256(src1 + 48);
            const __m256i s1_e   = yy_loadu_256(src1 + 64);
            const __m256i s1_f   = yy_loadu_256(src1 + 80);
            const __m256i s1_g   = yy_loadu_256(src1 + 96);
            const __m256i s1_h   = yy_loadu_256(src1 + 112);
            const __m256i m16_a  = calc_mask_d16_inv_avx2(&s0_a, &s1_a, &_r, &y38, &y64, shift);
            const __m256i m16_b  = calc_mask_d16_inv_avx2(&s0_b, &s1_b, &_r, &y38, &y64, shift);
            const __m256i m16_c  = calc_mask_d16_inv_avx2(&s0_c, &s1_c, &_r, &y38, &y64, shift);
            const __m256i m16_d  = calc_mask_d16_inv_avx2(&s0_d, &s1_d, &_r, &y38, &y64, shift);
            const __m256i m16_e  = calc_mask_d16_inv_avx2(&s0_e, &s1_e, &_r, &y38, &y64, shift);
            const __m256i m16_f  = calc_mask_d16_inv_avx2(&s0_f, &s1_f, &_r, &y38, &y64, shift);
            const __m256i m16_g  = calc_mask_d16_inv_avx2(&s0_g, &s1_g, &_r, &y38, &y64, shift);
            const __m256i m16_h  = calc_mask_d16_inv_avx2(&s0_h, &s1_h, &_r, &y38, &y64, shift);
            const __m256i m8_a_b = _mm256_packus_epi16(m16_a, m16_b);
            const __m256i m8_c_d = _mm256_packus_epi16(m16_c, m16_d);
            const __m256i m8_e_f = _mm256_packus_epi16(m16_e, m16_f);
            const __m256i m8_g_h = _mm256_packus_epi16(m16_g, m16_h);
            yy_storeu_256(mask, _mm256_permute4x64_epi64(m8_a_b, 0xd8));
            yy_storeu_256(mask + 32, _mm256_permute4x64_epi64(m8_c_d, 0xd8));
            yy_storeu_256(mask + 64, _mm256_permute4x64_epi64(m8_e_f, 0xd8));
            yy_storeu_256(mask + 96, _mm256_permute4x64_epi64(m8_g_h, 0xd8));
            src0 += src0_stride;
            src1 += src1_stride;
            mask += 128;
            i += 1;
        } while (i < h);
    }
}

void svt_av1_build_compound_diffwtd_mask_d16_avx2(uint8_t* mask, DIFFWTD_MASK_TYPE mask_type, const CONV_BUF_TYPE* src0,
                                                  int src0_stride, const CONV_BUF_TYPE* src1, int src1_stride, int h,
                                                  int w, ConvolveParams* conv_params, int bd) {
    const int shift = 2 * FILTER_BITS - conv_params->round_0 - conv_params->round_1 + (bd - 8);
    // When rounding constant is added, there is a possibility of overflow.
    // However that much precision is not required. Code should very well work for
    // other values of DIFF_FACTOR_LOG2 and AOM_BLEND_A64_MAX_ALPHA as well. But
    // there is a possibility of corner case bugs.
    assert(DIFF_FACTOR_LOG2 == 4);
    assert(AOM_BLEND_A64_MAX_ALPHA == 64);

    if (mask_type == DIFFWTD_38) {
        build_compound_diffwtd_mask_d16_avx2(mask, src0, src0_stride, src1, src1_stride, h, w, shift);
    } else {
        build_compound_diffwtd_mask_d16_inv_avx2(mask, src0, src0_stride, src1, src1_stride, h, w, shift);
    }
}
