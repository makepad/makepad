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
#include <assert.h>
#include <immintrin.h>
#include "definitions.h"
#include "common_dsp_rtcd.h"
#include "inv_transforms.h"
#include "av1_inv_txfm_ssse3.h"
#include "txfm_common_avx2.h"
#include "itx_hbd.h"
#include "transpose_sse2.h"
#include "synonyms_avx2.h"

static INLINE void round_shift_4x4_avx2(__m256i* in, int shift) {
    if (shift != 0) {
        __m256i rnding = _mm256_set1_epi32(1 << (shift - 1));
        in[0]          = _mm256_add_epi32(in[0], rnding);
        in[1]          = _mm256_add_epi32(in[1], rnding);
        in[2]          = _mm256_add_epi32(in[2], rnding);
        in[3]          = _mm256_add_epi32(in[3], rnding);

        in[0] = _mm256_srai_epi32(in[0], shift);
        in[1] = _mm256_srai_epi32(in[1], shift);
        in[2] = _mm256_srai_epi32(in[2], shift);
        in[3] = _mm256_srai_epi32(in[3], shift);
    }
}

static INLINE void round_shift_8x8_avx2(__m256i* in, int shift) {
    round_shift_4x4_avx2(in, shift);
    round_shift_4x4_avx2(in + 4, shift);
    round_shift_4x4_avx2(in + 8, shift);
    round_shift_4x4_avx2(in + 12, shift);
}

static INLINE void highbd_clamp_epi32(__m256i* x, int32_t bd) {
    const __m256i zero = _mm256_setzero_si256();
    const __m256i max  = _mm256_set1_epi32((1 << bd) - 1);

    *x = _mm256_min_epi32(*x, max);
    *x = _mm256_max_epi32(*x, zero);
}

static INLINE __m256i highbd_clamp_epi16_avx2(__m256i u, int32_t bd) {
    const __m256i zero = _mm256_setzero_si256();
    const __m256i one  = _mm256_set1_epi16(1);
    const __m256i max  = _mm256_sub_epi16(_mm256_slli_epi16(one, bd), one);
    __m256i       clamped, mask;

    mask    = _mm256_cmpgt_epi16(u, max);
    clamped = _mm256_andnot_si256(mask, u);
    mask    = _mm256_and_si256(mask, max);
    clamped = _mm256_or_si256(mask, clamped);
    mask    = _mm256_cmpgt_epi16(clamped, zero);
    clamped = _mm256_and_si256(clamped, mask);

    return clamped;
}

static INLINE __m256i half_btf_avx2(const __m256i* w0, const __m256i* n0, const __m256i* w1, const __m256i* n1,
                                    const __m256i* rounding, int32_t bit) {
    __m256i x, y;

    x = _mm256_mullo_epi32(*w0, *n0);
    y = _mm256_mullo_epi32(*w1, *n1);
    x = _mm256_add_epi32(x, y);
    x = _mm256_add_epi32(x, *rounding);
    x = _mm256_srai_epi32(x, bit);
    return x;
}

static void addsub_avx2(const __m256i in0, const __m256i in1, __m256i* out0, __m256i* out1, const __m256i* clamp_lo,
                        const __m256i* clamp_hi) {
    __m256i a0 = _mm256_add_epi32(in0, in1);
    __m256i a1 = _mm256_sub_epi32(in0, in1);

    a0 = _mm256_max_epi32(a0, *clamp_lo);
    a0 = _mm256_min_epi32(a0, *clamp_hi);
    a1 = _mm256_max_epi32(a1, *clamp_lo);
    a1 = _mm256_min_epi32(a1, *clamp_hi);

    *out0 = a0;
    *out1 = a1;
}

static void highbd_clamp_epi32_avx2(const __m256i* in, __m256i* out, const __m256i* clamp_lo, const __m256i* clamp_hi,
                                    int32_t size) {
    __m256i a0, a1;
    for (int32_t i = 0; i < size; i += 4) {
        a0     = _mm256_max_epi32(in[i], *clamp_lo);
        out[i] = _mm256_min_epi32(a0, *clamp_hi);

        a1         = _mm256_max_epi32(in[i + 1], *clamp_lo);
        out[i + 1] = _mm256_min_epi32(a1, *clamp_hi);

        a0         = _mm256_max_epi32(in[i + 2], *clamp_lo);
        out[i + 2] = _mm256_min_epi32(a0, *clamp_hi);

        a1         = _mm256_max_epi32(in[i + 3], *clamp_lo);
        out[i + 3] = _mm256_min_epi32(a1, *clamp_hi);
    }
}

static void neg_shift_avx2(const __m256i in0, const __m256i in1, __m256i* out0, __m256i* out1, const __m256i* clamp_lo,
                           const __m256i* clamp_hi, int32_t shift) {
    __m256i offset = _mm256_set1_epi32((1 << shift) >> 1);
    __m256i a0     = _mm256_add_epi32(offset, in0);
    __m256i a1     = _mm256_sub_epi32(offset, in1);

    a0 = _mm256_sra_epi32(a0, _mm_cvtsi32_si128(shift));
    a1 = _mm256_sra_epi32(a1, _mm_cvtsi32_si128(shift));

    a0 = _mm256_max_epi32(a0, *clamp_lo);
    a0 = _mm256_min_epi32(a0, *clamp_hi);
    a1 = _mm256_max_epi32(a1, *clamp_lo);
    a1 = _mm256_min_epi32(a1, *clamp_hi);

    *out0 = a0;
    *out1 = a1;
}

static INLINE void idct32_stage4_avx2(__m256i* bf1, const __m256i* cospim8, const __m256i* cospi56,
                                      const __m256i* cospi8, const __m256i* cospim56, const __m256i* cospim40,
                                      const __m256i* cospi24, const __m256i* cospi40, const __m256i* cospim24,
                                      const __m256i* rounding, int32_t bit) {
    __m256i temp1, temp2;
    temp1   = half_btf_avx2(cospim8, &bf1[17], cospi56, &bf1[30], rounding, bit);
    bf1[30] = half_btf_avx2(cospi56, &bf1[17], cospi8, &bf1[30], rounding, bit);
    bf1[17] = temp1;

    temp2   = half_btf_avx2(cospim56, &bf1[18], cospim8, &bf1[29], rounding, bit);
    bf1[29] = half_btf_avx2(cospim8, &bf1[18], cospi56, &bf1[29], rounding, bit);
    bf1[18] = temp2;

    temp1   = half_btf_avx2(cospim40, &bf1[21], cospi24, &bf1[26], rounding, bit);
    bf1[26] = half_btf_avx2(cospi24, &bf1[21], cospi40, &bf1[26], rounding, bit);
    bf1[21] = temp1;

    temp2   = half_btf_avx2(cospim24, &bf1[22], cospim40, &bf1[25], rounding, bit);
    bf1[25] = half_btf_avx2(cospim40, &bf1[22], cospi24, &bf1[25], rounding, bit);
    bf1[22] = temp2;
}

static INLINE void idct32_stage5_avx2(__m256i* bf1, const __m256i* cospim16, const __m256i* cospi48,
                                      const __m256i* cospi16, const __m256i* cospim48, const __m256i* clamp_lo,
                                      const __m256i* clamp_hi, const __m256i* rounding, int32_t bit) {
    __m256i temp1, temp2;
    temp1   = half_btf_avx2(cospim16, &bf1[9], cospi48, &bf1[14], rounding, bit);
    bf1[14] = half_btf_avx2(cospi48, &bf1[9], cospi16, &bf1[14], rounding, bit);
    bf1[9]  = temp1;

    temp2   = half_btf_avx2(cospim48, &bf1[10], cospim16, &bf1[13], rounding, bit);
    bf1[13] = half_btf_avx2(cospim16, &bf1[10], cospi48, &bf1[13], rounding, bit);
    bf1[10] = temp2;

    addsub_avx2(bf1[16], bf1[19], bf1 + 16, bf1 + 19, clamp_lo, clamp_hi);
    addsub_avx2(bf1[17], bf1[18], bf1 + 17, bf1 + 18, clamp_lo, clamp_hi);
    addsub_avx2(bf1[23], bf1[20], bf1 + 23, bf1 + 20, clamp_lo, clamp_hi);
    addsub_avx2(bf1[22], bf1[21], bf1 + 22, bf1 + 21, clamp_lo, clamp_hi);
    addsub_avx2(bf1[24], bf1[27], bf1 + 24, bf1 + 27, clamp_lo, clamp_hi);
    addsub_avx2(bf1[25], bf1[26], bf1 + 25, bf1 + 26, clamp_lo, clamp_hi);
    addsub_avx2(bf1[31], bf1[28], bf1 + 31, bf1 + 28, clamp_lo, clamp_hi);
    addsub_avx2(bf1[30], bf1[29], bf1 + 30, bf1 + 29, clamp_lo, clamp_hi);
}

static INLINE void idct32_stage6_avx2(__m256i* bf1, const __m256i* cospim32, const __m256i* cospi32,
                                      const __m256i* cospim16, const __m256i* cospi48, const __m256i* cospi16,
                                      const __m256i* cospim48, const __m256i* clamp_lo, const __m256i* clamp_hi,
                                      const __m256i* rounding, int32_t bit) {
    __m256i temp1, temp2;
    temp1  = half_btf_avx2(cospim32, &bf1[5], cospi32, &bf1[6], rounding, bit);
    bf1[6] = half_btf_avx2(cospi32, &bf1[5], cospi32, &bf1[6], rounding, bit);
    bf1[5] = temp1;

    addsub_avx2(bf1[8], bf1[11], bf1 + 8, bf1 + 11, clamp_lo, clamp_hi);
    addsub_avx2(bf1[9], bf1[10], bf1 + 9, bf1 + 10, clamp_lo, clamp_hi);
    addsub_avx2(bf1[15], bf1[12], bf1 + 15, bf1 + 12, clamp_lo, clamp_hi);
    addsub_avx2(bf1[14], bf1[13], bf1 + 14, bf1 + 13, clamp_lo, clamp_hi);

    temp1   = half_btf_avx2(cospim16, &bf1[18], cospi48, &bf1[29], rounding, bit);
    bf1[29] = half_btf_avx2(cospi48, &bf1[18], cospi16, &bf1[29], rounding, bit);
    bf1[18] = temp1;
    temp2   = half_btf_avx2(cospim16, &bf1[19], cospi48, &bf1[28], rounding, bit);
    bf1[28] = half_btf_avx2(cospi48, &bf1[19], cospi16, &bf1[28], rounding, bit);
    bf1[19] = temp2;
    temp1   = half_btf_avx2(cospim48, &bf1[20], cospim16, &bf1[27], rounding, bit);
    bf1[27] = half_btf_avx2(cospim16, &bf1[20], cospi48, &bf1[27], rounding, bit);
    bf1[20] = temp1;
    temp2   = half_btf_avx2(cospim48, &bf1[21], cospim16, &bf1[26], rounding, bit);
    bf1[26] = half_btf_avx2(cospim16, &bf1[21], cospi48, &bf1[26], rounding, bit);
    bf1[21] = temp2;
}

static INLINE void idct32_stage7_avx2(__m256i* bf1, const __m256i* cospim32, const __m256i* cospi32,
                                      const __m256i* clamp_lo, const __m256i* clamp_hi, const __m256i* rounding,
                                      int32_t bit) {
    __m256i temp1, temp2;
    addsub_avx2(bf1[0], bf1[7], bf1 + 0, bf1 + 7, clamp_lo, clamp_hi);
    addsub_avx2(bf1[1], bf1[6], bf1 + 1, bf1 + 6, clamp_lo, clamp_hi);
    addsub_avx2(bf1[2], bf1[5], bf1 + 2, bf1 + 5, clamp_lo, clamp_hi);
    addsub_avx2(bf1[3], bf1[4], bf1 + 3, bf1 + 4, clamp_lo, clamp_hi);

    temp1   = half_btf_avx2(cospim32, &bf1[10], cospi32, &bf1[13], rounding, bit);
    bf1[13] = half_btf_avx2(cospi32, &bf1[10], cospi32, &bf1[13], rounding, bit);
    bf1[10] = temp1;
    temp2   = half_btf_avx2(cospim32, &bf1[11], cospi32, &bf1[12], rounding, bit);
    bf1[12] = half_btf_avx2(cospi32, &bf1[11], cospi32, &bf1[12], rounding, bit);
    bf1[11] = temp2;

    addsub_avx2(bf1[16], bf1[23], bf1 + 16, bf1 + 23, clamp_lo, clamp_hi);
    addsub_avx2(bf1[17], bf1[22], bf1 + 17, bf1 + 22, clamp_lo, clamp_hi);
    addsub_avx2(bf1[18], bf1[21], bf1 + 18, bf1 + 21, clamp_lo, clamp_hi);
    addsub_avx2(bf1[19], bf1[20], bf1 + 19, bf1 + 20, clamp_lo, clamp_hi);
    addsub_avx2(bf1[31], bf1[24], bf1 + 31, bf1 + 24, clamp_lo, clamp_hi);
    addsub_avx2(bf1[30], bf1[25], bf1 + 30, bf1 + 25, clamp_lo, clamp_hi);
    addsub_avx2(bf1[29], bf1[26], bf1 + 29, bf1 + 26, clamp_lo, clamp_hi);
    addsub_avx2(bf1[28], bf1[27], bf1 + 28, bf1 + 27, clamp_lo, clamp_hi);
}

static INLINE void idct32_stage8_avx2(__m256i* bf1, const __m256i* cospim32, const __m256i* cospi32,
                                      const __m256i* clamp_lo, const __m256i* clamp_hi, const __m256i* rounding,
                                      int32_t bit) {
    __m256i temp1, temp2;
    addsub_avx2(bf1[0], bf1[15], bf1 + 0, bf1 + 15, clamp_lo, clamp_hi);
    addsub_avx2(bf1[1], bf1[14], bf1 + 1, bf1 + 14, clamp_lo, clamp_hi);
    addsub_avx2(bf1[2], bf1[13], bf1 + 2, bf1 + 13, clamp_lo, clamp_hi);
    addsub_avx2(bf1[3], bf1[12], bf1 + 3, bf1 + 12, clamp_lo, clamp_hi);
    addsub_avx2(bf1[4], bf1[11], bf1 + 4, bf1 + 11, clamp_lo, clamp_hi);
    addsub_avx2(bf1[5], bf1[10], bf1 + 5, bf1 + 10, clamp_lo, clamp_hi);
    addsub_avx2(bf1[6], bf1[9], bf1 + 6, bf1 + 9, clamp_lo, clamp_hi);
    addsub_avx2(bf1[7], bf1[8], bf1 + 7, bf1 + 8, clamp_lo, clamp_hi);

    temp1   = half_btf_avx2(cospim32, &bf1[20], cospi32, &bf1[27], rounding, bit);
    bf1[27] = half_btf_avx2(cospi32, &bf1[20], cospi32, &bf1[27], rounding, bit);
    bf1[20] = temp1;
    temp2   = half_btf_avx2(cospim32, &bf1[21], cospi32, &bf1[26], rounding, bit);
    bf1[26] = half_btf_avx2(cospi32, &bf1[21], cospi32, &bf1[26], rounding, bit);
    bf1[21] = temp2;
    temp1   = half_btf_avx2(cospim32, &bf1[22], cospi32, &bf1[25], rounding, bit);
    bf1[25] = half_btf_avx2(cospi32, &bf1[22], cospi32, &bf1[25], rounding, bit);
    bf1[22] = temp1;
    temp2   = half_btf_avx2(cospim32, &bf1[23], cospi32, &bf1[24], rounding, bit);
    bf1[24] = half_btf_avx2(cospi32, &bf1[23], cospi32, &bf1[24], rounding, bit);
    bf1[23] = temp2;
}

static INLINE void idct32_stage9_avx2(__m256i* bf1, __m256i* out, const int32_t do_cols, const int32_t bd,
                                      const int32_t out_shift, const __m256i* clamp_lo, const __m256i* clamp_hi) {
    addsub_avx2(bf1[0], bf1[31], out + 0, out + 31, clamp_lo, clamp_hi);
    addsub_avx2(bf1[1], bf1[30], out + 1, out + 30, clamp_lo, clamp_hi);
    addsub_avx2(bf1[2], bf1[29], out + 2, out + 29, clamp_lo, clamp_hi);
    addsub_avx2(bf1[3], bf1[28], out + 3, out + 28, clamp_lo, clamp_hi);
    addsub_avx2(bf1[4], bf1[27], out + 4, out + 27, clamp_lo, clamp_hi);
    addsub_avx2(bf1[5], bf1[26], out + 5, out + 26, clamp_lo, clamp_hi);
    addsub_avx2(bf1[6], bf1[25], out + 6, out + 25, clamp_lo, clamp_hi);
    addsub_avx2(bf1[7], bf1[24], out + 7, out + 24, clamp_lo, clamp_hi);
    addsub_avx2(bf1[8], bf1[23], out + 8, out + 23, clamp_lo, clamp_hi);
    addsub_avx2(bf1[9], bf1[22], out + 9, out + 22, clamp_lo, clamp_hi);
    addsub_avx2(bf1[10], bf1[21], out + 10, out + 21, clamp_lo, clamp_hi);
    addsub_avx2(bf1[11], bf1[20], out + 11, out + 20, clamp_lo, clamp_hi);
    addsub_avx2(bf1[12], bf1[19], out + 12, out + 19, clamp_lo, clamp_hi);
    addsub_avx2(bf1[13], bf1[18], out + 13, out + 18, clamp_lo, clamp_hi);
    addsub_avx2(bf1[14], bf1[17], out + 14, out + 17, clamp_lo, clamp_hi);
    addsub_avx2(bf1[15], bf1[16], out + 15, out + 16, clamp_lo, clamp_hi);
    if (!do_cols) {
        const int     log_range_out = AOMMAX(16, bd + 6);
        const __m256i clamp_lo_out  = _mm256_set1_epi32(-(1 << (log_range_out - 1)));
        const __m256i clamp_hi_out  = _mm256_set1_epi32((1 << (log_range_out - 1)) - 1);
        round_shift_8x8_avx2(out, out_shift);
        round_shift_8x8_avx2(out + 16, out_shift);
        highbd_clamp_epi32_avx2(out, out, &clamp_lo_out, &clamp_hi_out, 32);
    }
}

static INLINE void idct64_stage8_avx2(__m256i* u, const __m256i* cospim32, const __m256i* cospi32,
                                      const __m256i* cospim16, const __m256i* cospi48, const __m256i* cospi16,
                                      const __m256i* cospim48, const __m256i* clamp_lo, const __m256i* clamp_hi,
                                      const __m256i* rnding, int32_t bit) {
    int32_t i;
    __m256i temp1, temp2, temp3, temp4;
    temp1 = half_btf_avx2(cospim32, &u[10], cospi32, &u[13], rnding, bit);
    u[13] = half_btf_avx2(cospi32, &u[10], cospi32, &u[13], rnding, bit);
    u[10] = temp1;
    temp2 = half_btf_avx2(cospim32, &u[11], cospi32, &u[12], rnding, bit);
    u[12] = half_btf_avx2(cospi32, &u[11], cospi32, &u[12], rnding, bit);
    u[11] = temp2;

    for (i = 16; i < 20; ++i) {
        addsub_avx2(u[i], u[i ^ 7], &u[i], &u[i ^ 7], clamp_lo, clamp_hi);
        addsub_avx2(u[i ^ 15], u[i ^ 8], &u[i ^ 15], &u[i ^ 8], clamp_lo, clamp_hi);
    }

    temp1 = half_btf_avx2(cospim16, &u[36], cospi48, &u[59], rnding, bit);
    temp2 = half_btf_avx2(cospim16, &u[37], cospi48, &u[58], rnding, bit);
    temp3 = half_btf_avx2(cospim16, &u[38], cospi48, &u[57], rnding, bit);
    temp4 = half_btf_avx2(cospim16, &u[39], cospi48, &u[56], rnding, bit);
    u[56] = half_btf_avx2(cospi48, &u[39], cospi16, &u[56], rnding, bit);
    u[57] = half_btf_avx2(cospi48, &u[38], cospi16, &u[57], rnding, bit);
    u[58] = half_btf_avx2(cospi48, &u[37], cospi16, &u[58], rnding, bit);
    u[59] = half_btf_avx2(cospi48, &u[36], cospi16, &u[59], rnding, bit);
    u[36] = temp1;
    u[37] = temp2;
    u[38] = temp3;
    u[39] = temp4;

    temp1 = half_btf_avx2(cospim48, &u[40], cospim16, &u[55], rnding, bit);
    temp2 = half_btf_avx2(cospim48, &u[41], cospim16, &u[54], rnding, bit);
    temp3 = half_btf_avx2(cospim48, &u[42], cospim16, &u[53], rnding, bit);
    temp4 = half_btf_avx2(cospim48, &u[43], cospim16, &u[52], rnding, bit);
    u[52] = half_btf_avx2(cospim16, &u[43], cospi48, &u[52], rnding, bit);
    u[53] = half_btf_avx2(cospim16, &u[42], cospi48, &u[53], rnding, bit);
    u[54] = half_btf_avx2(cospim16, &u[41], cospi48, &u[54], rnding, bit);
    u[55] = half_btf_avx2(cospim16, &u[40], cospi48, &u[55], rnding, bit);
    u[40] = temp1;
    u[41] = temp2;
    u[42] = temp3;
    u[43] = temp4;
}

static INLINE void idct64_stage9_avx2(__m256i* u, const __m256i* cospim32, const __m256i* cospi32,
                                      const __m256i* clamp_lo, const __m256i* clamp_hi, const __m256i* rnding,
                                      int32_t bit) {
    int32_t i;
    __m256i temp1, temp2, temp3, temp4;
    for (i = 0; i < 8; ++i) {
        addsub_avx2(u[i], u[15 - i], &u[i], &u[15 - i], clamp_lo, clamp_hi);
    }
    temp1 = half_btf_avx2(cospim32, &u[20], cospi32, &u[27], rnding, bit);
    temp2 = half_btf_avx2(cospim32, &u[21], cospi32, &u[26], rnding, bit);
    temp3 = half_btf_avx2(cospim32, &u[22], cospi32, &u[25], rnding, bit);
    temp4 = half_btf_avx2(cospim32, &u[23], cospi32, &u[24], rnding, bit);
    u[24] = half_btf_avx2(cospi32, &u[23], cospi32, &u[24], rnding, bit);
    u[25] = half_btf_avx2(cospi32, &u[22], cospi32, &u[25], rnding, bit);
    u[26] = half_btf_avx2(cospi32, &u[21], cospi32, &u[26], rnding, bit);
    u[27] = half_btf_avx2(cospi32, &u[20], cospi32, &u[27], rnding, bit);
    u[20] = temp1;
    u[21] = temp2;
    u[22] = temp3;
    u[23] = temp4;
    for (i = 32; i < 40; i++) {
        addsub_avx2(u[i], u[i ^ 15], &u[i], &u[i ^ 15], clamp_lo, clamp_hi);
    }
    for (i = 48; i < 56; i++) {
        addsub_avx2(u[i ^ 15], u[i], &u[i ^ 15], &u[i], clamp_lo, clamp_hi);
    }
}

static INLINE void idct64_stage10_avx2(__m256i* u, const __m256i* cospim32, const __m256i* cospi32,
                                       const __m256i* clamp_lo, const __m256i* clamp_hi, const __m256i* rnding,
                                       int32_t bit) {
    __m256i temp1, temp2, temp3, temp4;
    for (int32_t i = 0; i < 16; i++) {
        addsub_avx2(u[i], u[31 - i], &u[i], &u[31 - i], clamp_lo, clamp_hi);
    }
    temp1 = half_btf_avx2(cospim32, &u[40], cospi32, &u[55], rnding, bit);
    temp2 = half_btf_avx2(cospim32, &u[41], cospi32, &u[54], rnding, bit);
    temp3 = half_btf_avx2(cospim32, &u[42], cospi32, &u[53], rnding, bit);
    temp4 = half_btf_avx2(cospim32, &u[43], cospi32, &u[52], rnding, bit);
    u[52] = half_btf_avx2(cospi32, &u[43], cospi32, &u[52], rnding, bit);
    u[53] = half_btf_avx2(cospi32, &u[42], cospi32, &u[53], rnding, bit);
    u[54] = half_btf_avx2(cospi32, &u[41], cospi32, &u[54], rnding, bit);
    u[55] = half_btf_avx2(cospi32, &u[40], cospi32, &u[55], rnding, bit);
    u[40] = temp1;
    u[41] = temp2;
    u[42] = temp3;
    u[43] = temp4;

    temp1 = half_btf_avx2(cospim32, &u[44], cospi32, &u[51], rnding, bit);
    temp2 = half_btf_avx2(cospim32, &u[45], cospi32, &u[50], rnding, bit);
    temp3 = half_btf_avx2(cospim32, &u[46], cospi32, &u[49], rnding, bit);
    temp4 = half_btf_avx2(cospim32, &u[47], cospi32, &u[48], rnding, bit);
    u[48] = half_btf_avx2(cospi32, &u[47], cospi32, &u[48], rnding, bit);
    u[49] = half_btf_avx2(cospi32, &u[46], cospi32, &u[49], rnding, bit);
    u[50] = half_btf_avx2(cospi32, &u[45], cospi32, &u[50], rnding, bit);
    u[51] = half_btf_avx2(cospi32, &u[44], cospi32, &u[51], rnding, bit);
    u[44] = temp1;
    u[45] = temp2;
    u[46] = temp3;
    u[47] = temp4;
}

static INLINE void idct64_stage11_avx2(__m256i* u, __m256i* out, int32_t do_cols, int32_t bd, int32_t out_shift,
                                       const __m256i* clamp_lo, const __m256i* clamp_hi) {
    for (int i = 0; i < 32; i++) {
        addsub_avx2(u[i], u[63 - i], &out[(i)], &out[(63 - i)], clamp_lo, clamp_hi);
    }

    if (!do_cols) {
        const int     log_range_out = AOMMAX(16, bd + 6);
        const __m256i clamp_lo_out  = _mm256_set1_epi32(-(1 << (log_range_out - 1)));
        const __m256i clamp_hi_out  = _mm256_set1_epi32((1 << (log_range_out - 1)) - 1);

        round_shift_8x8_avx2(out, out_shift);
        round_shift_8x8_avx2(out + 16, out_shift);
        round_shift_8x8_avx2(out + 32, out_shift);
        round_shift_8x8_avx2(out + 48, out_shift);
        highbd_clamp_epi32_avx2(out, out, &clamp_lo_out, &clamp_hi_out, 64);
    }
}

static void transpose_8x8_flip_avx2(const __m256i* in, __m256i* out) {
    __m256i u0, u1, u2, u3, u4, u5, u6, u7;
    __m256i x0, x1;

    u0 = _mm256_unpacklo_epi32(in[7], in[6]);
    u1 = _mm256_unpackhi_epi32(in[7], in[6]);

    u2 = _mm256_unpacklo_epi32(in[5], in[4]);
    u3 = _mm256_unpackhi_epi32(in[5], in[4]);

    u4 = _mm256_unpacklo_epi32(in[3], in[2]);
    u5 = _mm256_unpackhi_epi32(in[3], in[2]);

    u6 = _mm256_unpacklo_epi32(in[1], in[0]);
    u7 = _mm256_unpackhi_epi32(in[1], in[0]);

    x0     = _mm256_unpacklo_epi64(u0, u2);
    x1     = _mm256_unpacklo_epi64(u4, u6);
    out[0] = _mm256_permute2f128_si256(x0, x1, 0x20);
    out[4] = _mm256_permute2f128_si256(x0, x1, 0x31);

    x0     = _mm256_unpackhi_epi64(u0, u2);
    x1     = _mm256_unpackhi_epi64(u4, u6);
    out[1] = _mm256_permute2f128_si256(x0, x1, 0x20);
    out[5] = _mm256_permute2f128_si256(x0, x1, 0x31);

    x0     = _mm256_unpacklo_epi64(u1, u3);
    x1     = _mm256_unpacklo_epi64(u5, u7);
    out[2] = _mm256_permute2f128_si256(x0, x1, 0x20);
    out[6] = _mm256_permute2f128_si256(x0, x1, 0x31);

    x0     = _mm256_unpackhi_epi64(u1, u3);
    x1     = _mm256_unpackhi_epi64(u5, u7);
    out[3] = _mm256_permute2f128_si256(x0, x1, 0x20);
    out[7] = _mm256_permute2f128_si256(x0, x1, 0x31);
}

static INLINE __m256i highbd_get_recon_8x8_avx2(const __m256i pred, __m256i res, const int32_t bd) {
    __m256i x0 = pred;
    x0         = _mm256_add_epi32(res, x0);
    x0         = _mm256_packus_epi32(x0, x0);
    x0         = _mm256_permute4x64_epi64(x0, 0xd8);
    x0         = highbd_clamp_epi16_avx2(x0, bd);
    return x0;
}

static INLINE void highbd_write_buffer_8xn_avx2(__m256i* in, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                                                int32_t stride_w, int32_t flipud, int32_t height, const int32_t bd) {
    int32_t       j = flipud ? (height - 1) : 0;
    __m128i       temp;
    const int32_t step = flipud ? -1 : 1;
    for (int32_t i = 0; i < height; ++i, j += step) {
        temp       = _mm_loadu_si128((__m128i const*)(output_r + i * stride_r));
        __m256i v  = _mm256_cvtepi16_epi32(temp);
        __m256i u  = highbd_get_recon_8x8_avx2(v, in[j], bd);
        __m128i u1 = _mm256_castsi256_si128(u);
        _mm_storeu_si128((__m128i*)(output_w + i * stride_w), u1);
    }
}

static INLINE __m256i highbd_get_recon_16x8_avx2(const __m256i pred, __m256i res0, __m256i res1, const int32_t bd) {
    __m256i x0 = _mm256_cvtepi16_epi32(_mm256_castsi256_si128(pred));
    __m256i x1 = _mm256_cvtepi16_epi32(_mm256_extractf128_si256(pred, 1));

    x0 = _mm256_add_epi32(res0, x0);
    x1 = _mm256_add_epi32(res1, x1);
    x0 = _mm256_packus_epi32(x0, x1);
    x0 = _mm256_permute4x64_epi64(x0, 0xd8);
    x0 = highbd_clamp_epi16_avx2(x0, bd);
    return x0;
}

static INLINE void highbd_write_buffer_16xn_avx2(__m256i* in, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                                                 int32_t stride_w, int32_t flipud, int32_t height, const int32_t bd) {
    int32_t       j    = flipud ? (height - 1) : 0;
    const int32_t step = flipud ? -1 : 1;
    for (int32_t i = 0; i < height; ++i, j += step) {
        __m256i v = _mm256_loadu_si256((__m256i const*)(output_r + i * stride_r));
        __m256i u = highbd_get_recon_16x8_avx2(v, in[j], in[j + height], bd);

        _mm256_storeu_si256((__m256i*)(output_w + i * stride_w), u);
    }
}

static INLINE void load_buffer_4x4(const int32_t* coeff, __m256i* in) {
    in[0] = _mm256_loadu_si256((const __m256i*)coeff);
    in[1] = _mm256_loadu_si256((const __m256i*)(coeff + 8));
}

static INLINE void write_buffer_4x4(__m256i* in, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                                    int32_t stride_w, int32_t fliplr, int32_t flipud, int32_t bd) {
    __m256i       u0, x0, x1, v0, v1;
    const __m256i zero = _mm256_setzero_si256();

    if (fliplr) {
        in[0] = _mm256_shuffle_epi32(in[0], 0x1B);
        in[1] = _mm256_shuffle_epi32(in[1], 0x1B);
    }

    if (flipud) {
        u0 = _mm256_set_epi64x(*(uint64_t*)(output_r + 0 * stride_r),
                               *(uint64_t*)(output_r + 2 * stride_r),
                               *(uint64_t*)(output_r + 1 * stride_r),
                               *(uint64_t*)(output_r + 3 * stride_r));
    } else {
        // Load 64bits in order ACBD
        u0 = _mm256_set_epi64x(*(uint64_t*)(output_r + 3 * stride_r),
                               *(uint64_t*)(output_r + 1 * stride_r),
                               *(uint64_t*)(output_r + 2 * stride_r),
                               *(uint64_t*)(output_r + 0 * stride_r));
    }

    // Unpack and Swap 128bits from ACBD to ABCD
    x0 = _mm256_unpacklo_epi16(u0, zero);
    x1 = _mm256_unpackhi_epi16(u0, zero);

    v0 = _mm256_add_epi32(in[0], x0);
    v1 = _mm256_add_epi32(in[1], x1);

    highbd_clamp_epi32(&v0, bd);
    highbd_clamp_epi32(&v1, bd);

    // Pack and Swap 128bits from ABCD to ACBD
    v0 = _mm256_packus_epi32(v0, v1);

    if (flipud) {
        _mm_storel_epi64((__m128i*)(output_w + 3 * stride_w), _mm256_castsi256_si128(v0));
        _mm_storel_epi64((__m128i*)(output_w + 2 * stride_w), _mm256_extractf128_si256(v0, 0x1));
        //Move up  memory 64bites
        v0 = _mm256_permute4x64_epi64(v0, 1 + (3 << 4));
        _mm_storel_epi64((__m128i*)(output_w + 1 * stride_w), _mm256_castsi256_si128(v0));
        _mm_storel_epi64((__m128i*)(output_w + 0 * stride_w), _mm256_extractf128_si256(v0, 0x1));
    } else {
        // Store in order from ACBD to ABCD
        _mm_storel_epi64((__m128i*)(output_w + 0 * stride_w), _mm256_castsi256_si128(v0));
        _mm_storel_epi64((__m128i*)(output_w + 1 * stride_w), _mm256_extractf128_si256(v0, 0x1));
        //Move up  memory 64bites
        v0 = _mm256_permute4x64_epi64(v0, 1 + (3 << 4));
        _mm_storel_epi64((__m128i*)(output_w + 2 * stride_w), _mm256_castsi256_si128(v0));
        _mm_storel_epi64((__m128i*)(output_w + 3 * stride_w), _mm256_extractf128_si256(v0, 0x1));
    }
}

static INLINE void round_shift_4x4(__m256i* in, int32_t shift) {
    __m256i rnding = _mm256_set1_epi32(1 << (shift - 1));

    in[0] = _mm256_add_epi32(in[0], rnding);
    in[0] = _mm256_srai_epi32(in[0], shift);
    in[1] = _mm256_add_epi32(in[1], rnding);
    in[1] = _mm256_srai_epi32(in[1], shift);
}

static INLINE void iidentity4_and_round_shift_avx2(__m256i* input, int32_t shift) {
    // Input takes 18 bits, can be multiplied with new_sqrt2 in 32 bits space.
    // round_shift(new_sqrt2_bits) and next round_shift(shift) in one pass.
    const __m256i scalar = _mm256_set1_epi32(new_sqrt2);
    const __m256i rnding = _mm256_set1_epi32((1 << (new_sqrt2_bits - 1)) + (!!(shift) << (shift + new_sqrt2_bits - 1)));

    for (int32_t i = 0; i < 2; i++) {
        input[i] = _mm256_mullo_epi32(input[i], scalar);
        input[i] = _mm256_add_epi32(input[i], rnding);
        input[i] = _mm256_srai_epi32(input[i], new_sqrt2_bits + shift);
    }
}

static INLINE void idct4_row_avx2(__m256i* in, int8_t cos_bit) {
    const int32_t* cospi      = cospi_arr(cos_bit);
    const __m256i  cospi32    = _mm256_set1_epi32(cospi[32]);
    const __m256i  cospi32x16 = _mm256_blend_epi32(cospi32, _mm256_set1_epi32(cospi[16]), 0xAA);
    const __m256i  cospi32x48 = _mm256_blend_epi32(cospi32, _mm256_set1_epi32(cospi[48]), 0xAA);
    const __m256i  rnding     = _mm256_set1_epi32((1 << (cos_bit - 1)));
    const __m256i  minplus    = _mm256_blend_epi32(_mm256_set1_epi32(-1), _mm256_set1_epi32(1), 0xAA);
    __m256i        v0, v1, x, y;
    __m256i        step[2];

    v0 = _mm256_unpacklo_epi64(in[0], in[1]);
    v1 = _mm256_unpackhi_epi64(in[0], in[1]);

    x       = _mm256_mullo_epi32(cospi32x16, v0);
    y       = _mm256_mullo_epi32(cospi32x48, v1);
    step[0] = _mm256_add_epi32(x, y);

    x       = _mm256_mullo_epi32(cospi32x48, v0);
    y       = _mm256_mullo_epi32(cospi32x16, v1);
    step[1] = _mm256_sub_epi32(x, y);

    step[0] = _mm256_add_epi32(step[0], rnding);
    step[0] = _mm256_srai_epi32(step[0], cos_bit);
    step[1] = _mm256_add_epi32(step[1], rnding);
    step[1] = _mm256_srai_epi32(step[1], cos_bit);

    v0 = _mm256_shuffle_epi32(step[0], 0xB1);
    v1 = _mm256_shuffle_epi32(step[1], 0xB1);

    v0 = _mm256_mullo_epi32(minplus, v0);
    v1 = _mm256_mullo_epi32(minplus, v1);

    v0 = _mm256_add_epi32(v0, step[0]);
    v1 = _mm256_add_epi32(v1, step[1]);

    v0 = _mm256_shuffle_epi32(v0, 0x2D);
    v1 = _mm256_shuffle_epi32(v1, 0x87);

    in[0] = _mm256_blend_epi32(v0, v1, 0x66);

    v0    = _mm256_blend_epi32(v0, v1, 0x99);
    in[1] = _mm256_shuffle_epi32(v0, 0xB1);
}

static INLINE void idct4_col_avx2(__m256i* in, int8_t cos_bit) {
    const int32_t* cospi      = cospi_arr(cos_bit);
    const __m256i  cospi32    = _mm256_set1_epi32(cospi[32]);
    const __m256i  cospi32x16 = _mm256_blend_epi32(_mm256_set1_epi32(cospi[16]), cospi32, 0x0F);
    const __m256i  cospi32x48 = _mm256_blend_epi32(_mm256_set1_epi32(cospi[48]), cospi32, 0x0F);
    const __m256i  rnding     = _mm256_set1_epi32((1 << (cos_bit - 1)));
    __m256i        x, y;
    __m256i        step[2];

    x       = _mm256_mullo_epi32(cospi32x16, in[0]);
    y       = _mm256_mullo_epi32(cospi32x48, in[1]);
    step[0] = _mm256_add_epi32(x, y);

    x       = _mm256_mullo_epi32(cospi32x48, in[0]);
    y       = _mm256_mullo_epi32(cospi32x16, in[1]);
    step[1] = _mm256_sub_epi32(x, y);

    step[0] = _mm256_add_epi32(step[0], rnding);
    step[0] = _mm256_srai_epi32(step[0], cos_bit);
    step[1] = _mm256_add_epi32(step[1], rnding);
    step[1] = _mm256_srai_epi32(step[1], cos_bit);

    x     = yy_unpacklo_epi128(step[0], step[1]);
    y     = yy_unpackhi_epi128(step[0], step[1]);
    in[0] = _mm256_add_epi32(x, y);

    x     = yy_unpacklo_epi128(step[1], step[0]);
    y     = yy_unpackhi_epi128(step[1], step[0]);
    in[1] = _mm256_sub_epi32(x, y);
}

static INLINE void iadst4_row_avx2(__m256i* in, int8_t cos_bit) {
    const int32_t  bit    = cos_bit;
    const int32_t* sinpi  = sinpi_arr(bit);
    const __m256i  rnding = _mm256_set1_epi32(1 << (bit - 1));
    const __m128i  sinpi1 = _mm_set1_epi32((int32_t)sinpi[1]);
    const __m128i  sinpi2 = _mm_set1_epi32((int32_t)sinpi[2]);
    const __m128i  sinpi3 = _mm_set1_epi32((int32_t)sinpi[3]);
    const __m128i  sinpi4 = _mm_set1_epi32((int32_t)sinpi[4]);
    __m128i        t;
    __m128i        s0, s1, s2, s3, s4, s5, s6, s7;
    __m128i        x0, x1, x2, x3;
    __m128i        u0, u1, u2, u3;
    __m256i        y0;

    u0 = _mm256_extractf128_si256(in[0], 0x1);
    u1 = _mm256_extractf128_si256(in[1], 0x1);

    s0 = _mm_unpacklo_epi32(_mm256_castsi256_si128(in[0]), u0);
    s1 = _mm_unpackhi_epi32(_mm256_castsi256_si128(in[0]), u0);
    s2 = _mm_unpacklo_epi32(_mm256_castsi256_si128(in[1]), u1);
    s3 = _mm_unpackhi_epi32(_mm256_castsi256_si128(in[1]), u1);

    x0 = _mm_unpacklo_epi64(s0, s2);
    x1 = _mm_unpackhi_epi64(s0, s2);
    x2 = _mm_unpacklo_epi64(s1, s3);
    x3 = _mm_unpackhi_epi64(s1, s3);

    s0 = _mm_mullo_epi32(x0, sinpi1);
    s1 = _mm_mullo_epi32(x0, sinpi2);
    s2 = _mm_mullo_epi32(x1, sinpi3);
    s3 = _mm_mullo_epi32(x2, sinpi4);
    s4 = _mm_mullo_epi32(x2, sinpi1);
    s5 = _mm_mullo_epi32(x3, sinpi2);
    s6 = _mm_mullo_epi32(x3, sinpi4);
    t  = _mm_sub_epi32(x0, x2);
    s7 = _mm_add_epi32(t, x3);

    t  = _mm_add_epi32(s0, s3);
    s0 = _mm_add_epi32(t, s5);
    t  = _mm_sub_epi32(s1, s4);
    s1 = _mm_sub_epi32(t, s6);
    u2 = _mm_mullo_epi32(s7, sinpi3);

    u0 = _mm_add_epi32(s0, s2);
    u1 = _mm_add_epi32(s1, s2);
    t  = _mm_add_epi32(s0, s1);
    u3 = _mm_sub_epi32(t, s2);

    s0 = _mm_unpacklo_epi32(u0, u1);
    s1 = _mm_unpackhi_epi32(u0, u1);
    s2 = _mm_unpacklo_epi32(u2, u3);
    s3 = _mm_unpackhi_epi32(u2, u3);

    u0 = _mm_unpacklo_epi64(s0, s2);
    u1 = _mm_unpackhi_epi64(s0, s2);
    u2 = _mm_unpacklo_epi64(s1, s3);
    u3 = _mm_unpackhi_epi64(s1, s3);

    y0    = _mm256_insertf128_si256(_mm256_castsi128_si256(u0), (u1), 0x1);
    y0    = _mm256_add_epi32(y0, rnding);
    in[0] = _mm256_srai_epi32(y0, bit);

    y0    = _mm256_insertf128_si256(_mm256_castsi128_si256(u2), (u3), 0x1);
    y0    = _mm256_add_epi32(y0, rnding);
    in[1] = _mm256_srai_epi32(y0, bit);
}

static INLINE void iadst4_col_avx2(__m256i* in, int8_t cos_bit) {
    const int32_t  bit    = cos_bit;
    const int32_t* sinpi  = sinpi_arr(bit);
    const __m256i  rnding = _mm256_set1_epi32(1 << (bit - 1));
    const __m128i  sinpi1 = _mm_set1_epi32((int32_t)sinpi[1]);
    const __m128i  sinpi2 = _mm_set1_epi32((int32_t)sinpi[2]);
    const __m128i  sinpi3 = _mm_set1_epi32((int32_t)sinpi[3]);
    const __m128i  sinpi4 = _mm_set1_epi32((int32_t)sinpi[4]);
    __m128i        t;
    __m128i        s0, s1, s2, s3, s4, s5, s6, s7;
    __m128i        x0, x1;
    __m128i        u0, u1, u3;
    __m256i        y0;

    x0 = _mm256_extractf128_si256(in[0], 0x1);
    x1 = _mm256_extractf128_si256(in[1], 0x1);

    s0 = _mm_mullo_epi32(_mm256_castsi256_si128(in[0]), sinpi1);
    s1 = _mm_mullo_epi32(_mm256_castsi256_si128(in[0]), sinpi2);
    s2 = _mm_mullo_epi32(x0, sinpi3);
    s3 = _mm_mullo_epi32(_mm256_castsi256_si128(in[1]), sinpi4);
    s4 = _mm_mullo_epi32(_mm256_castsi256_si128(in[1]), sinpi1);
    s5 = _mm_mullo_epi32(x1, sinpi2);
    s6 = _mm_mullo_epi32(x1, sinpi4);
    t  = _mm_sub_epi32(_mm256_castsi256_si128(in[0]), _mm256_castsi256_si128(in[1]));
    s7 = _mm_add_epi32(t, x1);

    t  = _mm_add_epi32(s0, s3);
    s0 = _mm_add_epi32(t, s5);
    t  = _mm_sub_epi32(s1, s4);
    s1 = _mm_sub_epi32(t, s6);
    s3 = _mm_mullo_epi32(s7, sinpi3);

    u0 = _mm_add_epi32(s0, s2);
    u1 = _mm_add_epi32(s1, s2);

    t  = _mm_add_epi32(s0, s1);
    u3 = _mm_sub_epi32(t, s2);

    y0    = _mm256_insertf128_si256(_mm256_castsi128_si256(u0), (u1), 0x1);
    y0    = _mm256_add_epi32(y0, rnding);
    in[0] = _mm256_srai_epi32(y0, bit);

    y0    = _mm256_insertf128_si256(_mm256_castsi128_si256(s3), (u3), 0x1);
    y0    = _mm256_add_epi32(y0, rnding);
    in[1] = _mm256_srai_epi32(y0, bit);
}

void svt_av1_inv_txfm2d_add_4x4_avx2(const int32_t* input, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                                     int32_t stride_w, TxType tx_type, int32_t bd) {
    __m256i       in[2];
    const int8_t* shift   = svt_aom_inv_txfm_shift_ls[TX_4X4];
    const int32_t txw_idx = get_txw_idx(TX_4X4);
    const int32_t txh_idx = get_txh_idx(TX_4X4);

    switch (tx_type) {
    case IDTX:
        load_buffer_4x4(input, in);
        iidentity4_and_round_shift_avx2(in, -shift[0]);
        iidentity4_and_round_shift_avx2(in, -shift[1]);
        write_buffer_4x4(in, output_r, stride_r, output_w, stride_w, 0, 0, bd);
        break;
    case V_DCT:
        load_buffer_4x4(input, in);
        iidentity4_and_round_shift_avx2(in, -shift[0]);
        idct4_col_avx2(in, inv_cos_bit_col[txw_idx][txh_idx]);
        round_shift_4x4(in, -shift[1]);
        write_buffer_4x4(in, output_r, stride_r, output_w, stride_w, 0, 0, bd);
        break;
    case H_DCT:
        load_buffer_4x4(input, in);
        idct4_row_avx2(in, inv_cos_bit_row[txw_idx][txh_idx]);
        iidentity4_and_round_shift_avx2(in, -shift[1]);
        write_buffer_4x4(in, output_r, stride_r, output_w, stride_w, 0, 0, bd);
        break;
    case V_ADST:
        load_buffer_4x4(input, in);
        iidentity4_and_round_shift_avx2(in, -shift[0]);
        iadst4_col_avx2(in, inv_cos_bit_col[txw_idx][txh_idx]);
        round_shift_4x4(in, -shift[1]);
        write_buffer_4x4(in, output_r, stride_r, output_w, stride_w, 0, 0, bd);
        break;
    case H_ADST:
        load_buffer_4x4(input, in);
        iadst4_row_avx2(in, inv_cos_bit_row[txw_idx][txh_idx]);
        iidentity4_and_round_shift_avx2(in, -shift[1]);
        write_buffer_4x4(in, output_r, stride_r, output_w, stride_w, 0, 0, bd);
        break;
    case V_FLIPADST:
        load_buffer_4x4(input, in);
        iidentity4_and_round_shift_avx2(in, -shift[0]);
        iadst4_col_avx2(in, inv_cos_bit_col[txw_idx][txh_idx]);
        round_shift_4x4(in, -shift[1]);
        write_buffer_4x4(in, output_r, stride_r, output_w, stride_w, 0, 1, bd);
        break;
    case H_FLIPADST:
        load_buffer_4x4(input, in);
        iadst4_row_avx2(in, inv_cos_bit_row[txw_idx][txh_idx]);
        iidentity4_and_round_shift_avx2(in, -shift[1]);
        write_buffer_4x4(in, output_r, stride_r, output_w, stride_w, 1, 0, bd);
        break;
    default:
        svt_av1_inv_txfm2d_add_4x4_sse4_1(input, output_r, stride_r, output_w, stride_w, tx_type, bd);
        break;
    }
}

#define TRANSPOSE_4X4_AVX2(x0, x1, x2, x3, y0, y1, y2, y3) \
    do {                                                   \
        __m256i u0, u1, u2, u3;                            \
        u0 = _mm256_unpacklo_epi32(x0, x1);                \
        u1 = _mm256_unpackhi_epi32(x0, x1);                \
        u2 = _mm256_unpacklo_epi32(x2, x3);                \
        u3 = _mm256_unpackhi_epi32(x2, x3);                \
        y0 = _mm256_unpacklo_epi64(u0, u2);                \
        y1 = _mm256_unpackhi_epi64(u0, u2);                \
        y2 = _mm256_unpacklo_epi64(u1, u3);                \
        y3 = _mm256_unpackhi_epi64(u1, u3);                \
    } while (0)

static INLINE void transpose_8x8_avx2(const __m256i* in, __m256i* out) {
    __m256i out1[8];
    TRANSPOSE_4X4_AVX2(in[0], in[1], in[2], in[3], out1[0], out1[1], out1[4], out1[5]);
    TRANSPOSE_4X4_AVX2(in[4], in[5], in[6], in[7], out1[2], out1[3], out1[6], out1[7]);
    out[0] = yy_unpacklo_epi128(out1[0], out1[2]);
    out[1] = yy_unpacklo_epi128(out1[1], out1[3]);
    out[2] = yy_unpacklo_epi128(out1[4], out1[6]);
    out[3] = yy_unpacklo_epi128(out1[5], out1[7]);
    out[4] = yy_unpackhi_epi128(out1[0], out1[2]);
    out[5] = yy_unpackhi_epi128(out1[1], out1[3]);
    out[6] = yy_unpackhi_epi128(out1[4], out1[6]);
    out[7] = yy_unpackhi_epi128(out1[5], out1[7]);
}

static INLINE void transpose_16x16_avx2(const __m256i* in, __m256i* out) {
    __m256i temp[32];
    TRANSPOSE_4X4_AVX2(in[0], in[2], in[4], in[6], temp[0], temp[2], temp[4], temp[6]);
    TRANSPOSE_4X4_AVX2(in[8], in[10], in[12], in[14], temp[17], temp[19], temp[21], temp[23]);
    TRANSPOSE_4X4_AVX2(in[1], in[3], in[5], in[7], temp[16], temp[18], temp[20], temp[22]);
    TRANSPOSE_4X4_AVX2(in[9], in[11], in[13], in[15], temp[25], temp[27], temp[29], temp[31]);
    TRANSPOSE_4X4_AVX2(in[16], in[18], in[20], in[22], temp[1], temp[3], temp[5], temp[7]);
    TRANSPOSE_4X4_AVX2(in[24], in[26], in[28], in[30], temp[9], temp[11], temp[13], temp[15]);
    TRANSPOSE_4X4_AVX2(in[17], in[19], in[21], in[23], temp[8], temp[10], temp[12], temp[14]);
    TRANSPOSE_4X4_AVX2(in[25], in[27], in[29], in[31], temp[24], temp[26], temp[28], temp[30]);

    out[0]  = yy_unpacklo_epi128(temp[0], temp[17]);
    out[1]  = yy_unpacklo_epi128(temp[1], temp[9]);
    out[2]  = yy_unpacklo_epi128(temp[2], temp[19]);
    out[3]  = yy_unpacklo_epi128(temp[3], temp[11]);
    out[4]  = yy_unpacklo_epi128(temp[4], temp[21]);
    out[5]  = yy_unpacklo_epi128(temp[5], temp[13]);
    out[6]  = yy_unpacklo_epi128(temp[6], temp[23]);
    out[7]  = yy_unpacklo_epi128(temp[7], temp[15]);
    out[8]  = yy_unpackhi_epi128(temp[0], temp[17]);
    out[9]  = yy_unpackhi_epi128(temp[1], temp[9]);
    out[10] = yy_unpackhi_epi128(temp[2], temp[19]);
    out[11] = yy_unpackhi_epi128(temp[3], temp[11]);
    out[12] = yy_unpackhi_epi128(temp[4], temp[21]);
    out[13] = yy_unpackhi_epi128(temp[5], temp[13]);
    out[14] = yy_unpackhi_epi128(temp[6], temp[23]);
    out[15] = yy_unpackhi_epi128(temp[7], temp[15]);
    out[16] = yy_unpacklo_epi128(temp[16], temp[25]);
    out[17] = yy_unpacklo_epi128(temp[8], temp[24]);
    out[18] = yy_unpacklo_epi128(temp[18], temp[27]);
    out[19] = yy_unpacklo_epi128(temp[10], temp[26]);
    out[20] = yy_unpacklo_epi128(temp[20], temp[29]);
    out[21] = yy_unpacklo_epi128(temp[12], temp[28]);
    out[22] = yy_unpacklo_epi128(temp[22], temp[31]);
    out[23] = yy_unpacklo_epi128(temp[14], temp[30]);
    out[24] = yy_unpackhi_epi128(temp[16], temp[25]);
    out[25] = yy_unpackhi_epi128(temp[8], temp[24]);
    out[26] = yy_unpackhi_epi128(temp[18], temp[27]);
    out[27] = yy_unpackhi_epi128(temp[10], temp[26]);
    out[28] = yy_unpackhi_epi128(temp[20], temp[29]);
    out[29] = yy_unpackhi_epi128(temp[12], temp[28]);
    out[30] = yy_unpackhi_epi128(temp[22], temp[31]);
    out[31] = yy_unpackhi_epi128(temp[14], temp[30]);
}

static void load_buffer_8x8(const int32_t* coeff, __m256i* in) {
    int32_t i;
    for (i = 0; i < 8; ++i) {
        in[i] = _mm256_loadu_si256((const __m256i*)coeff);
        coeff += 8;
    }
}

static INLINE void write_buffer_8x8(__m256i* in, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                                    int32_t stride_w, int32_t fliplr, int32_t flipud, int32_t bd) {
    __m256i       u0, x0, x1, v0, v1;
    const __m256i zero = _mm256_setzero_si256();
    int32_t       i    = 0;
    int32_t       step = 1;

    if (flipud) {
        i    = 7;
        step = -1;
    }

    while (i < 8 && i > -1) {
        u0 = _mm256_inserti128_si256(_mm256_castsi128_si256(_mm_loadu_si128((__m128i*)(output_r))),
                                     _mm_loadu_si128((__m128i*)(output_r + stride_r)),
                                     1);

        // Swap 64bits from ABCD to ACBD
        u0 = _mm256_permute4x64_epi64(u0, 0xD8);

        // Unpack and Swap 128bits from ACBD to ABCD
        x0 = _mm256_unpacklo_epi16(u0, zero);
        x1 = _mm256_unpackhi_epi16(u0, zero);

        if (fliplr) {
            v0 = _mm256_permute4x64_epi64(in[i], 0x1B);
            v0 = _mm256_shuffle_epi32(v0, 0xB1);
            v0 = _mm256_add_epi32(v0, x0);
            i += step;
            v1 = _mm256_permute4x64_epi64(in[i], 0x1B);
            v1 = _mm256_shuffle_epi32(v1, 0xB1);
            v1 = _mm256_add_epi32(v1, x1);
            i += step;
        } else {
            v0 = _mm256_add_epi32(in[i], x0);
            i += step;
            v1 = _mm256_add_epi32(in[i], x1);
            i += step;
        }

        highbd_clamp_epi32(&v0, bd);
        highbd_clamp_epi32(&v1, bd);

        // Pack and Swap 128bits from ABCD to ACBD
        v0 = _mm256_packus_epi32(v0, v1);
        // Swap 64bits from ACBD to ABCD
        v0 = _mm256_permute4x64_epi64(v0, 0xD8);

        _mm_storeu_si128((__m128i*)output_w, _mm256_castsi256_si128(v0));
        _mm_storeu_si128((__m128i*)(output_w + stride_w), _mm256_extractf128_si256(v0, 0x1));

        output_r += 2 * stride_r;
        output_w += 2 * stride_w;
    }
}

static INLINE void round_shift_8x8(__m256i* in, int32_t shift) {
    __m256i rnding = _mm256_set1_epi32(1 << (shift - 1));
    int32_t i      = 0;

    while (i < 8) {
        in[i] = _mm256_add_epi32(in[i], rnding);
        in[i] = _mm256_srai_epi32(in[i], shift);
        i++;
    }
}

static INLINE void round_shift_8x8_double(__m256i* in, int32_t first, int32_t second) {
    __m256i rnding = _mm256_set1_epi32((1 << (first - 1)) + (1 << (first + second - 1)));
    int32_t i      = 0;

    while (i < 8) {
        in[i] = _mm256_add_epi32(in[i], rnding);
        in[i] = _mm256_srai_epi32(in[i], first + second);
        i++;
    }
}

static INLINE void idct8_col_avx2(__m256i* in, __m256i* out, int32_t bit) {
    const int32_t* cospi    = cospi_arr(bit);
    const __m256i  cospi56  = _mm256_set1_epi32(cospi[56]);
    const __m256i  cospim8  = _mm256_set1_epi32(-cospi[8]);
    const __m256i  cospi24  = _mm256_set1_epi32(cospi[24]);
    const __m256i  cospim40 = _mm256_set1_epi32(-cospi[40]);
    const __m256i  cospi40  = _mm256_set1_epi32(cospi[40]);
    const __m256i  cospi8   = _mm256_set1_epi32(cospi[8]);
    const __m256i  cospi32  = _mm256_set1_epi32(cospi[32]);
    const __m256i  cospim32 = _mm256_set1_epi32(-cospi[32]);
    const __m256i  cospi48  = _mm256_set1_epi32(cospi[48]);
    const __m256i  cospim16 = _mm256_set1_epi32(-cospi[16]);
    const __m256i  cospi16  = _mm256_set1_epi32(cospi[16]);
    const __m256i  rounding = _mm256_set1_epi32(1 << (bit - 1));
    __m256i        tmp[8], tmp2[8];

    //stage 1

    //stage 2
    tmp[4] = half_btf_avx2(&cospi56, &in[1], &cospim8, &in[7], &rounding, bit);
    tmp[5] = half_btf_avx2(&cospi24, &in[5], &cospim40, &in[3], &rounding, bit);
    tmp[6] = half_btf_avx2(&cospi40, &in[5], &cospi24, &in[3], &rounding, bit);
    tmp[7] = half_btf_avx2(&cospi8, &in[1], &cospi56, &in[7], &rounding, bit);

    //stage 3
    tmp2[0] = half_btf_avx2(&cospi32, &in[0], &cospi32, &in[4], &rounding, bit);
    tmp2[1] = half_btf_avx2(&cospi32, &in[0], &cospim32, &in[4], &rounding, bit);
    tmp2[2] = half_btf_avx2(&cospi48, &in[2], &cospim16, &in[6], &rounding, bit);
    tmp2[3] = half_btf_avx2(&cospi16, &in[2], &cospi48, &in[6], &rounding, bit);
    tmp2[4] = _mm256_add_epi32(tmp[4], tmp[5]);
    tmp2[5] = _mm256_sub_epi32(tmp[4], tmp[5]);
    tmp2[6] = _mm256_sub_epi32(tmp[7], tmp[6]);
    tmp2[7] = _mm256_add_epi32(tmp[6], tmp[7]);

    //stage 4
    tmp[0] = _mm256_add_epi32(tmp2[0], tmp2[3]);
    tmp[1] = _mm256_add_epi32(tmp2[1], tmp2[2]);
    tmp[2] = _mm256_sub_epi32(tmp2[1], tmp2[2]);
    tmp[3] = _mm256_sub_epi32(tmp2[0], tmp2[3]);
    tmp[5] = half_btf_avx2(&cospim32, &tmp2[5], &cospi32, &tmp2[6], &rounding, bit);
    tmp[6] = half_btf_avx2(&cospi32, &tmp2[5], &cospi32, &tmp2[6], &rounding, bit);

    //stage 5
    out[0] = _mm256_add_epi32(tmp[0], tmp2[7]);
    out[1] = _mm256_add_epi32(tmp[1], tmp[6]);
    out[2] = _mm256_add_epi32(tmp[2], tmp[5]);
    out[3] = _mm256_add_epi32(tmp[3], tmp2[4]);
    out[4] = _mm256_sub_epi32(tmp[3], tmp2[4]);
    out[5] = _mm256_sub_epi32(tmp[2], tmp[5]);
    out[6] = _mm256_sub_epi32(tmp[1], tmp[6]);
    out[7] = _mm256_sub_epi32(tmp[0], tmp2[7]);
}

static INLINE void iadst8_col_avx2(__m256i* in, __m256i* out, int8_t cos_bit) {
    const int32_t* cospi    = cospi_arr(cos_bit);
    const __m256i  cospi4   = _mm256_set1_epi32(cospi[4]);
    const __m256i  cospi60  = _mm256_set1_epi32(cospi[60]);
    const __m256i  cospi20  = _mm256_set1_epi32(cospi[20]);
    const __m256i  cospi44  = _mm256_set1_epi32(cospi[44]);
    const __m256i  cospi36  = _mm256_set1_epi32(cospi[36]);
    const __m256i  cospi28  = _mm256_set1_epi32(cospi[28]);
    const __m256i  cospi52  = _mm256_set1_epi32(cospi[52]);
    const __m256i  cospi12  = _mm256_set1_epi32(cospi[12]);
    const __m256i  cospim4  = _mm256_set1_epi32(-cospi[4]);
    const __m256i  cospim20 = _mm256_set1_epi32(-cospi[20]);
    const __m256i  cospim36 = _mm256_set1_epi32(-cospi[36]);
    const __m256i  cospim52 = _mm256_set1_epi32(-cospi[52]);
    const __m256i  cospi16  = _mm256_set1_epi32(cospi[16]);
    const __m256i  cospi48  = _mm256_set1_epi32(cospi[48]);
    const __m256i  cospim48 = _mm256_set1_epi32(-cospi[48]);
    const __m256i  cospim16 = _mm256_set1_epi32(-cospi[16]);
    const __m256i  cospi32  = _mm256_set1_epi32(cospi[32]);
    const __m256i  cospim32 = _mm256_set1_epi32(-cospi[32]);
    const __m256i  negative = _mm256_set1_epi32(-1);
    const __m256i  rounding = _mm256_set1_epi32(1 << (cos_bit - 1));
    __m256i        tmp[8], tmp2[4];

    //stage 1
    //stage 2
    tmp[0] = half_btf_avx2(&cospi4, &in[7], &cospi60, &in[0], &rounding, cos_bit);
    tmp[1] = half_btf_avx2(&cospi60, &in[7], &cospim4, &in[0], &rounding, cos_bit);
    tmp[2] = half_btf_avx2(&cospi20, &in[5], &cospi44, &in[2], &rounding, cos_bit);
    tmp[3] = half_btf_avx2(&cospi44, &in[5], &cospim20, &in[2], &rounding, cos_bit);
    tmp[4] = half_btf_avx2(&cospi36, &in[3], &cospi28, &in[4], &rounding, cos_bit);
    tmp[5] = half_btf_avx2(&cospi28, &in[3], &cospim36, &in[4], &rounding, cos_bit);
    tmp[6] = half_btf_avx2(&cospi52, &in[1], &cospi12, &in[6], &rounding, cos_bit);
    tmp[7] = half_btf_avx2(&cospi12, &in[1], &cospim52, &in[6], &rounding, cos_bit);

    //stage 3
    out[7]  = _mm256_add_epi32(tmp[0], tmp[4]);
    out[1]  = _mm256_add_epi32(tmp[1], tmp[5]);
    out[2]  = _mm256_add_epi32(tmp[2], tmp[6]);
    out[3]  = _mm256_add_epi32(tmp[3], tmp[7]);
    tmp2[0] = _mm256_sub_epi32(tmp[0], tmp[4]);
    tmp2[1] = _mm256_sub_epi32(tmp[1], tmp[5]);
    tmp2[2] = _mm256_sub_epi32(tmp[2], tmp[6]);
    tmp2[3] = _mm256_sub_epi32(tmp[3], tmp[7]);

    //stage 4
    tmp[4] = half_btf_avx2(&cospi16, &tmp2[0], &cospi48, &tmp2[1], &rounding, cos_bit);
    tmp[5] = half_btf_avx2(&cospi48, &tmp2[0], &cospim16, &tmp2[1], &rounding, cos_bit);
    tmp[6] = half_btf_avx2(&cospim48, &tmp2[2], &cospi16, &tmp2[3], &rounding, cos_bit);
    tmp[7] = half_btf_avx2(&cospi16, &tmp2[2], &cospi48, &tmp2[3], &rounding, cos_bit);

    //stage 5
    out[0]  = _mm256_add_epi32(out[7], out[2]);
    tmp[1]  = _mm256_add_epi32(out[1], out[3]);
    tmp2[0] = _mm256_sub_epi32(out[7], out[2]);
    tmp2[1] = _mm256_sub_epi32(out[1], out[3]);
    out[1]  = _mm256_add_epi32(tmp[4], tmp[6]);
    out[6]  = _mm256_add_epi32(tmp[5], tmp[7]);
    tmp2[2] = _mm256_sub_epi32(tmp[4], tmp[6]);
    tmp2[3] = _mm256_sub_epi32(tmp[5], tmp[7]);

    //stage 6
    tmp[2] = half_btf_avx2(&cospi32, &tmp2[0], &cospi32, &tmp2[1], &rounding, cos_bit);
    out[4] = half_btf_avx2(&cospi32, &tmp2[0], &cospim32, &tmp2[1], &rounding, cos_bit);
    out[2] = half_btf_avx2(&cospi32, &tmp2[2], &cospi32, &tmp2[3], &rounding, cos_bit);
    tmp[7] = half_btf_avx2(&cospi32, &tmp2[2], &cospim32, &tmp2[3], &rounding, cos_bit);

    //stage 7
    out[1] = _mm256_sign_epi32(out[1], negative);
    out[3] = _mm256_sign_epi32(tmp[2], negative);
    out[5] = _mm256_sign_epi32(tmp[7], negative);
    out[7] = _mm256_sign_epi32(tmp[1], negative);
}

void svt_av1_inv_txfm2d_add_8x8_avx2(const int32_t* input, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                                     int32_t stride_w, TxType tx_type, int32_t bd) {
    __m256i       in[8], out[8];
    const int8_t* shift   = svt_aom_inv_txfm_shift_ls[TX_8X8];
    const int32_t txw_idx = get_txw_idx(TX_8X8);
    const int32_t txh_idx = get_txh_idx(TX_8X8);

    switch (tx_type) {
    case DCT_DCT:
        load_buffer_8x8(input, in);
        transpose_8x8_avx2(in, out);
        idct8_col_avx2(out, in, inv_cos_bit_row[txw_idx][txh_idx]);
        transpose_8x8_avx2(in, out);
        round_shift_8x8(out, -shift[0]);
        idct8_col_avx2(out, in, inv_cos_bit_col[txw_idx][txh_idx]);
        round_shift_8x8(in, -shift[1]);
        write_buffer_8x8(in, output_r, stride_r, output_w, stride_w, 0, 0, bd);
        break;
    case DCT_ADST:
        load_buffer_8x8(input, in);
        transpose_8x8_avx2(in, out);
        iadst8_col_avx2(out, in, inv_cos_bit_row[txw_idx][txh_idx]);
        transpose_8x8_avx2(in, out);
        round_shift_8x8(out, -shift[0]);
        idct8_col_avx2(out, in, inv_cos_bit_col[txw_idx][txh_idx]);
        round_shift_8x8(in, -shift[1]);
        write_buffer_8x8(in, output_r, stride_r, output_w, stride_w, 0, 0, bd);
        break;
    case ADST_DCT:
        load_buffer_8x8(input, in);
        transpose_8x8_avx2(in, out);
        idct8_col_avx2(out, in, inv_cos_bit_row[txw_idx][txh_idx]);
        transpose_8x8_avx2(in, out);
        round_shift_8x8(out, -shift[0]);
        iadst8_col_avx2(out, in, inv_cos_bit_col[txw_idx][txh_idx]);
        round_shift_8x8(in, -shift[1]);
        write_buffer_8x8(in, output_r, stride_r, output_w, stride_w, 0, 0, bd);
        break;
    case ADST_ADST:
        load_buffer_8x8(input, in);
        transpose_8x8_avx2(in, out);
        iadst8_col_avx2(out, in, inv_cos_bit_row[txw_idx][txh_idx]);
        transpose_8x8_avx2(in, out);
        round_shift_8x8(out, -shift[0]);
        iadst8_col_avx2(out, in, inv_cos_bit_col[txw_idx][txh_idx]);
        round_shift_8x8(in, -shift[1]);
        write_buffer_8x8(in, output_r, stride_r, output_w, stride_w, 0, 0, bd);
        break;
    case IDTX:
        load_buffer_8x8(input, in);
        // Operations can be joined together without losing precision
        // svt_av1_iidentity8_c() shift left 1 bits
        // round_shift_8x8(, -shift[0]) shift right 1 bits
        // svt_av1_iidentity8_c() shift left 1 bits
        // round_shift_8x8(, -shift[1]) shift right 4 bits with complement
        round_shift_8x8(in, -shift[0] - shift[1] - 2);
        write_buffer_8x8(in, output_r, stride_r, output_w, stride_w, 0, 0, bd);
        break;
    case V_DCT:
        load_buffer_8x8(input, in);
        // svt_av1_iidentity8_c() shift left 1 bits
        // round_shift_8x8(, -shift[0]) shift right 1 bits
        idct8_col_avx2(in, out, inv_cos_bit_col[txw_idx][txh_idx]);
        round_shift_8x8(out, -shift[1]);
        write_buffer_8x8(out, output_r, stride_r, output_w, stride_w, 0, 0, bd);
        break;
    case H_DCT:
        load_buffer_8x8(input, in);
        transpose_8x8_avx2(in, out);
        idct8_col_avx2(out, in, inv_cos_bit_row[txw_idx][txh_idx]);
        transpose_8x8_avx2(in, out);
        // svt_av1_iidentity8_c() shift left 1 bits
        // round_shift_8x8(, -shift[1]) shift right 4 bits with complement
        round_shift_8x8_double(out, -shift[0], -shift[1] - 1);
        write_buffer_8x8(out, output_r, stride_r, output_w, stride_w, 0, 0, bd);
        break;
    case V_ADST:
        load_buffer_8x8(input, in);
        // svt_av1_iidentity8_c() shift left 1 bits
        // round_shift_8x8(, -shift[0]) shift right 1 bits
        iadst8_col_avx2(in, out, inv_cos_bit_col[txw_idx][txh_idx]);
        round_shift_8x8(out, -shift[1]);
        write_buffer_8x8(out, output_r, stride_r, output_w, stride_w, 0, 0, bd);
        break;
    case H_ADST:
        load_buffer_8x8(input, in);
        transpose_8x8_avx2(in, out);
        iadst8_col_avx2(out, in, inv_cos_bit_row[txw_idx][txh_idx]);
        transpose_8x8_avx2(in, out);
        // svt_av1_iidentity8_c() shift left 1 bits
        // round_shift_8x8(, -shift[1]) shift right 4 bits with complement
        round_shift_8x8_double(out, -shift[0], -shift[1] - 1);
        write_buffer_8x8(out, output_r, stride_r, output_w, stride_w, 0, 0, bd);
        break;
    case V_FLIPADST:
        load_buffer_8x8(input, in);
        // svt_av1_iidentity8_c() shift left 1 bits
        // round_shift_8x8(, -shift[0]) shift right 1 bits
        iadst8_col_avx2(in, out, inv_cos_bit_col[txw_idx][txh_idx]);
        round_shift_8x8(out, -shift[1]);
        write_buffer_8x8(out, output_r, stride_r, output_w, stride_w, 0, 1, bd);
        break;
    case H_FLIPADST:
        load_buffer_8x8(input, in);
        transpose_8x8_avx2(in, out);
        iadst8_col_avx2(out, in, inv_cos_bit_row[txw_idx][txh_idx]);
        transpose_8x8_avx2(in, out);
        // svt_av1_iidentity8_c() shift left 1 bits
        // round_shift_8x8(, -shift[1]) shift right 4 bits with complement
        round_shift_8x8_double(out, -shift[0], -shift[1] - 1);
        write_buffer_8x8(out, output_r, stride_r, output_w, stride_w, 1, 0, bd);
        break;
    default:
        svt_av1_inv_txfm2d_add_8x8_sse4_1(input, output_r, stride_r, output_w, stride_w, tx_type, bd);
        break;
    }
}

static void load_buffer_16x16(const int32_t* coeff, __m256i* in) {
    int32_t i;
    for (i = 0; i < 32; ++i) {
        in[i] = _mm256_loadu_si256((const __m256i*)coeff);
        coeff += 8;
    }
}

static INLINE void write_buffer_16x16(__m256i* in, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                                      int32_t stride_w, int32_t fliplr, int32_t flipud, int32_t bd) {
    __m256i       u0, x0, x1, v0, v1;
    const __m256i zero = _mm256_setzero_si256();
    int32_t       i    = 0;

    if (flipud) {
        output_r += stride_r * 15;
        stride_r = -stride_r;
        output_w += stride_w * 15;
        stride_w = -stride_w;
    }

    while (i < 32) {
        u0 = _mm256_loadu_si256((const __m256i*)output_r);

        x0 = _mm256_unpacklo_epi16(u0, zero);
        x1 = _mm256_unpackhi_epi16(u0, zero);

        if (fliplr) {
            v0 = _mm256_permute2f128_si256(in[i], in[i + 1], 0x13);
            v0 = _mm256_shuffle_epi32(v0, 0x1B);
            v1 = _mm256_permute2f128_si256(in[i], in[i + 1], 0x02);
            v1 = _mm256_shuffle_epi32(v1, 0x1B);
        } else {
            v0 = _mm256_permute2f128_si256(in[i], in[i + 1], 0x20);
            v1 = _mm256_permute2f128_si256(in[i], in[i + 1], 0x31);
        }

        v0 = _mm256_add_epi32(v0, x0);
        v1 = _mm256_add_epi32(v1, x1);
        highbd_clamp_epi32(&v0, bd);
        highbd_clamp_epi32(&v1, bd);

        v0 = _mm256_packus_epi32(v0, v1);

        _mm256_storeu_si256((__m256i*)output_w, v0);

        output_r += stride_r;
        output_w += stride_w;
        i += 2;
    }
}

static INLINE void write_buffer_16x16_new(__m256i* in, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                                          int32_t stride_w, int32_t bd) {
    __m128i u0_a, u0_b;
    __m256i u1_a, u1_b;
    __m256i v0_a, v0_b;
    __m128i p, q, r, s;

    int32_t i = 0;

    while (i < 32) {
        u0_a = _mm_loadu_si128((const __m128i*)output_r);
        u0_b = _mm_loadu_si128((const __m128i*)(output_r + 8));

        u1_a = _mm256_cvtepu16_epi32(u0_a);
        u1_b = _mm256_cvtepu16_epi32(u0_b);
        v0_a = in[i];
        v0_b = in[i + 1];

        v0_a = _mm256_add_epi32(v0_a, u1_a);
        v0_b = _mm256_add_epi32(v0_b, u1_b);

        p    = _mm256_castsi256_si128(v0_a);
        q    = _mm256_extracti128_si256(v0_a, 0x1);
        r    = _mm256_castsi256_si128(v0_b);
        s    = _mm256_extracti128_si256(v0_b, 0x1);
        p    = _mm_packus_epi32(p, q);
        r    = _mm_packus_epi32(r, s);
        u1_a = _mm256_castsi128_si256(p);
        u1_a = _mm256_insertf128_si256(u1_a, r, 0x1);
        u1_a = highbd_clamp_epi16_avx2(u1_a, bd);
        _mm256_storeu_si256((__m256i*)output_w, u1_a);

        output_r += stride_r;
        output_w += stride_w;
        i += 2;
    }
}

static INLINE void round_shift_16x16(__m256i* in, int32_t shift) {
    __m256i rnding = _mm256_set1_epi32(1 << (shift - 1));
    int32_t i      = 0;

    while (i < 32) {
        in[i] = _mm256_add_epi32(in[i], rnding);
        in[i] = _mm256_srai_epi32(in[i], shift);
        i++;
    }
}

static INLINE void iidentity16_and_round_shift_avx2(__m256i* input, int32_t shift) {
    // Input takes 18 bits, can be multiplied with new_sqrt2 in 32 bits space.
    // Multiplied by half value new_sqrt2, instead (2*new_sqrt2),
    // and round_shift() by one bit less (new_sqrt2_bits-1).
    // round_shift(new_sqrt2_bits-1) and next round_shift(shift) in one pass.
    const __m256i scalar = _mm256_set1_epi32(new_sqrt2);
    const __m256i rnding = _mm256_set1_epi32((1 << (new_sqrt2_bits - 2)) + (!!(shift) << (shift + new_sqrt2_bits - 2)));

    for (int32_t i = 0; i < 32; i++) {
        input[i] = _mm256_mullo_epi32(input[i], scalar);
        input[i] = _mm256_add_epi32(input[i], rnding);
        input[i] = _mm256_srai_epi32(input[i], new_sqrt2_bits - 1 + shift);
    }
}

static INLINE void idct16_col_avx2(__m256i* in, __m256i* out, int32_t bit, const int8_t* shift) {
    (void)shift;
    const int32_t* cospi    = cospi_arr(bit);
    const __m256i  cospi60  = _mm256_set1_epi32(cospi[60]);
    const __m256i  cospi28  = _mm256_set1_epi32(cospi[28]);
    const __m256i  cospi44  = _mm256_set1_epi32(cospi[44]);
    const __m256i  cospi12  = _mm256_set1_epi32(cospi[12]);
    const __m256i  cospi52  = _mm256_set1_epi32(cospi[52]);
    const __m256i  cospi20  = _mm256_set1_epi32(cospi[20]);
    const __m256i  cospi36  = _mm256_set1_epi32(cospi[36]);
    const __m256i  cospi4   = _mm256_set1_epi32(cospi[4]);
    const __m256i  cospi56  = _mm256_set1_epi32(cospi[56]);
    const __m256i  cospi24  = _mm256_set1_epi32(cospi[24]);
    const __m256i  cospi40  = _mm256_set1_epi32(cospi[40]);
    const __m256i  cospi8   = _mm256_set1_epi32(cospi[8]);
    const __m256i  cospi32  = _mm256_set1_epi32(cospi[32]);
    const __m256i  cospi48  = _mm256_set1_epi32(cospi[48]);
    const __m256i  cospi16  = _mm256_set1_epi32(cospi[16]);
    const __m256i  cospim4  = _mm256_set1_epi32(-cospi[4]);
    const __m256i  cospim36 = _mm256_set1_epi32(-cospi[36]);
    const __m256i  cospim20 = _mm256_set1_epi32(-cospi[20]);
    const __m256i  cospim52 = _mm256_set1_epi32(-cospi[52]);
    const __m256i  cospim8  = _mm256_set1_epi32(-cospi[8]);
    const __m256i  cospim40 = _mm256_set1_epi32(-cospi[40]);
    const __m256i  cospim32 = _mm256_set1_epi32(-cospi[32]);
    const __m256i  cospim48 = _mm256_set1_epi32(-cospi[48]);
    const __m256i  cospim16 = _mm256_set1_epi32(-cospi[16]);
    const __m256i  rounding = _mm256_set1_epi32(1 << (bit - 1));
    __m256i        tmp[16], tmp2[16];
    int32_t        col = 0;

    for (col = 0; col < 2; ++col) {
        //stage 1

        //stage 2
        tmp[8]  = half_btf_avx2(&cospi60, &in[1 * 2 + col], &cospim4, &in[15 * 2 + col], &rounding, bit);
        tmp[9]  = half_btf_avx2(&cospi28, &in[9 * 2 + col], &cospim36, &in[7 * 2 + col], &rounding, bit);
        tmp[10] = half_btf_avx2(&cospi44, &in[5 * 2 + col], &cospim20, &in[11 * 2 + col], &rounding, bit);
        tmp[11] = half_btf_avx2(&cospi12, &in[13 * 2 + col], &cospim52, &in[3 * 2 + col], &rounding, bit);
        tmp[12] = half_btf_avx2(&cospi52, &in[13 * 2 + col], &cospi12, &in[3 * 2 + col], &rounding, bit);
        tmp[13] = half_btf_avx2(&cospi20, &in[5 * 2 + col], &cospi44, &in[11 * 2 + col], &rounding, bit);
        tmp[14] = half_btf_avx2(&cospi36, &in[9 * 2 + col], &cospi28, &in[7 * 2 + col], &rounding, bit);
        tmp[15] = half_btf_avx2(&cospi4, &in[1 * 2 + col], &cospi60, &in[15 * 2 + col], &rounding, bit);

        //stage 3
        tmp2[0]  = half_btf_avx2(&cospi56, &in[2 * 2 + col], &cospim8, &in[14 * 2 + col], &rounding, bit);
        tmp2[1]  = half_btf_avx2(&cospi24, &in[10 * 2 + col], &cospim40, &in[6 * 2 + col], &rounding, bit);
        tmp2[2]  = half_btf_avx2(&cospi40, &in[10 * 2 + col], &cospi24, &in[6 * 2 + col], &rounding, bit);
        tmp2[3]  = half_btf_avx2(&cospi8, &in[2 * 2 + col], &cospi56, &in[14 * 2 + col], &rounding, bit);
        tmp2[4]  = _mm256_add_epi32(tmp[8], tmp[9]);
        tmp2[5]  = _mm256_sub_epi32(tmp[8], tmp[9]);
        tmp2[6]  = _mm256_sub_epi32(tmp[11], tmp[10]);
        tmp2[7]  = _mm256_add_epi32(tmp[10], tmp[11]);
        tmp2[8]  = _mm256_add_epi32(tmp[12], tmp[13]);
        tmp2[9]  = _mm256_sub_epi32(tmp[12], tmp[13]);
        tmp2[10] = _mm256_sub_epi32(tmp[15], tmp[14]);
        tmp2[11] = _mm256_add_epi32(tmp[14], tmp[15]);

        //stage 4
        tmp[0]  = half_btf_avx2(&cospi32, &in[0 * 2 + col], &cospi32, &in[8 * 2 + col], &rounding, bit);
        tmp[1]  = half_btf_avx2(&cospi32, &in[0 * 2 + col], &cospim32, &in[8 * 2 + col], &rounding, bit);
        tmp[2]  = half_btf_avx2(&cospi48, &in[4 * 2 + col], &cospim16, &in[12 * 2 + col], &rounding, bit);
        tmp[3]  = half_btf_avx2(&cospi16, &in[4 * 2 + col], &cospi48, &in[12 * 2 + col], &rounding, bit);
        tmp[4]  = _mm256_add_epi32(tmp2[0], tmp2[1]);
        tmp[5]  = _mm256_sub_epi32(tmp2[0], tmp2[1]);
        tmp[6]  = _mm256_sub_epi32(tmp2[3], tmp2[2]);
        tmp[7]  = _mm256_add_epi32(tmp2[2], tmp2[3]);
        tmp[9]  = half_btf_avx2(&cospim16, &tmp2[5], &cospi48, &tmp2[10], &rounding, bit);
        tmp[10] = half_btf_avx2(&cospim48, &tmp2[6], &cospim16, &tmp2[9], &rounding, bit);
        tmp[13] = half_btf_avx2(&cospim16, &tmp2[6], &cospi48, &tmp2[9], &rounding, bit);
        tmp[14] = half_btf_avx2(&cospi48, &tmp2[5], &cospi16, &tmp2[10], &rounding, bit);

        //stage 5
        tmp2[12] = _mm256_sub_epi32(tmp2[11], tmp2[8]);
        tmp2[15] = _mm256_add_epi32(tmp2[8], tmp2[11]);
        tmp2[8]  = _mm256_add_epi32(tmp2[4], tmp2[7]);
        tmp2[11] = _mm256_sub_epi32(tmp2[4], tmp2[7]);
        tmp2[0]  = _mm256_add_epi32(tmp[0], tmp[3]);
        tmp2[1]  = _mm256_add_epi32(tmp[1], tmp[2]);
        tmp2[2]  = _mm256_sub_epi32(tmp[1], tmp[2]);
        tmp2[3]  = _mm256_sub_epi32(tmp[0], tmp[3]);
        tmp2[5]  = half_btf_avx2(&cospim32, &tmp[5], &cospi32, &tmp[6], &rounding, bit);
        tmp2[6]  = half_btf_avx2(&cospi32, &tmp[5], &cospi32, &tmp[6], &rounding, bit);
        tmp2[9]  = _mm256_add_epi32(tmp[9], tmp[10]);
        tmp2[10] = _mm256_sub_epi32(tmp[9], tmp[10]);
        tmp2[13] = _mm256_sub_epi32(tmp[14], tmp[13]);
        tmp2[14] = _mm256_add_epi32(tmp[13], tmp[14]);

        //stage 6
        tmp[0]  = _mm256_add_epi32(tmp2[0], tmp[7]);
        tmp[1]  = _mm256_add_epi32(tmp2[1], tmp2[6]);
        tmp[2]  = _mm256_add_epi32(tmp2[2], tmp2[5]);
        tmp[3]  = _mm256_add_epi32(tmp2[3], tmp[4]);
        tmp[4]  = _mm256_sub_epi32(tmp2[3], tmp[4]);
        tmp[5]  = _mm256_sub_epi32(tmp2[2], tmp2[5]);
        tmp[6]  = _mm256_sub_epi32(tmp2[1], tmp2[6]);
        tmp[7]  = _mm256_sub_epi32(tmp2[0], tmp[7]);
        tmp[10] = half_btf_avx2(&cospim32, &tmp2[10], &cospi32, &tmp2[13], &rounding, bit);
        tmp[11] = half_btf_avx2(&cospim32, &tmp2[11], &cospi32, &tmp2[12], &rounding, bit);
        tmp[12] = half_btf_avx2(&cospi32, &tmp2[11], &cospi32, &tmp2[12], &rounding, bit);
        tmp[13] = half_btf_avx2(&cospi32, &tmp2[10], &cospi32, &tmp2[13], &rounding, bit);

        //stage 7
        out[0 * 2 + col]  = _mm256_add_epi32(tmp[0], tmp2[15]);
        out[1 * 2 + col]  = _mm256_add_epi32(tmp[1], tmp2[14]);
        out[2 * 2 + col]  = _mm256_add_epi32(tmp[2], tmp[13]);
        out[3 * 2 + col]  = _mm256_add_epi32(tmp[3], tmp[12]);
        out[4 * 2 + col]  = _mm256_add_epi32(tmp[4], tmp[11]);
        out[5 * 2 + col]  = _mm256_add_epi32(tmp[5], tmp[10]);
        out[6 * 2 + col]  = _mm256_add_epi32(tmp[6], tmp2[9]);
        out[7 * 2 + col]  = _mm256_add_epi32(tmp[7], tmp2[8]);
        out[8 * 2 + col]  = _mm256_sub_epi32(tmp[7], tmp2[8]);
        out[9 * 2 + col]  = _mm256_sub_epi32(tmp[6], tmp2[9]);
        out[10 * 2 + col] = _mm256_sub_epi32(tmp[5], tmp[10]);
        out[11 * 2 + col] = _mm256_sub_epi32(tmp[4], tmp[11]);
        out[12 * 2 + col] = _mm256_sub_epi32(tmp[3], tmp[12]);
        out[13 * 2 + col] = _mm256_sub_epi32(tmp[2], tmp[13]);
        out[14 * 2 + col] = _mm256_sub_epi32(tmp[1], tmp2[14]);
        out[15 * 2 + col] = _mm256_sub_epi32(tmp[0], tmp2[15]);
    }
}

static INLINE void iadst16_col_avx2(__m256i* in, __m256i* out, int8_t cos_bit) {
    const int32_t* cospi   = cospi_arr(cos_bit);
    const __m256i  cospi2  = _mm256_set1_epi32(cospi[2]);
    const __m256i  cospi62 = _mm256_set1_epi32(cospi[62]);
    const __m256i  cospi10 = _mm256_set1_epi32(cospi[10]);
    const __m256i  cospi54 = _mm256_set1_epi32(cospi[54]);
    const __m256i  cospi18 = _mm256_set1_epi32(cospi[18]);
    const __m256i  cospi46 = _mm256_set1_epi32(cospi[46]);
    const __m256i  cospi26 = _mm256_set1_epi32(cospi[26]);
    const __m256i  cospi38 = _mm256_set1_epi32(cospi[38]);
    const __m256i  cospi34 = _mm256_set1_epi32(cospi[34]);
    const __m256i  cospi30 = _mm256_set1_epi32(cospi[30]);
    const __m256i  cospi42 = _mm256_set1_epi32(cospi[42]);
    const __m256i  cospi22 = _mm256_set1_epi32(cospi[22]);
    const __m256i  cospi50 = _mm256_set1_epi32(cospi[50]);
    const __m256i  cospi14 = _mm256_set1_epi32(cospi[14]);
    const __m256i  cospi58 = _mm256_set1_epi32(cospi[58]);
    const __m256i  cospi6  = _mm256_set1_epi32(cospi[6]);

    const __m256i cospi8  = _mm256_set1_epi32(cospi[8]);
    const __m256i cospi56 = _mm256_set1_epi32(cospi[56]);
    const __m256i cospi40 = _mm256_set1_epi32(cospi[40]);
    const __m256i cospi24 = _mm256_set1_epi32(cospi[24]);

    const __m256i cospi16 = _mm256_set1_epi32(cospi[16]);
    const __m256i cospi48 = _mm256_set1_epi32(cospi[48]);
    const __m256i cospi32 = _mm256_set1_epi32(cospi[32]);

    const __m256i cospim2  = _mm256_set1_epi32(-cospi[2]);
    const __m256i cospim10 = _mm256_set1_epi32(-cospi[10]);
    const __m256i cospim18 = _mm256_set1_epi32(-cospi[18]);
    const __m256i cospim26 = _mm256_set1_epi32(-cospi[26]);
    const __m256i cospim34 = _mm256_set1_epi32(-cospi[34]);
    const __m256i cospim42 = _mm256_set1_epi32(-cospi[42]);
    const __m256i cospim50 = _mm256_set1_epi32(-cospi[50]);
    const __m256i cospim58 = _mm256_set1_epi32(-cospi[58]);
    const __m256i cospim56 = _mm256_set1_epi32(-cospi[56]);
    const __m256i cospim24 = _mm256_set1_epi32(-cospi[24]);
    const __m256i cospim8  = _mm256_set1_epi32(-cospi[8]);
    const __m256i cospim40 = _mm256_set1_epi32(-cospi[40]);
    const __m256i cospim48 = _mm256_set1_epi32(-cospi[48]);
    const __m256i cospim16 = _mm256_set1_epi32(-cospi[16]);
    const __m256i cospim32 = _mm256_set1_epi32(-cospi[32]);

    const __m256i negative = _mm256_set1_epi32(-1);
    const __m256i rounding = _mm256_set1_epi32(1 << (cos_bit - 1));

    __m256i tmp[16], tmp2[16], tmp3[16];

    int32_t col = 0;

    for (col = 0; col < 2; ++col) {
        //stage 1

        //stage 2
        tmp[0]  = half_btf_avx2(&cospi2, &in[15 * 2 + col], &cospi62, &in[0 * 2 + col], &rounding, cos_bit);
        tmp[1]  = half_btf_avx2(&cospi62, &in[15 * 2 + col], &cospim2, &in[0 * 2 + col], &rounding, cos_bit);
        tmp[2]  = half_btf_avx2(&cospi10, &in[13 * 2 + col], &cospi54, &in[2 * 2 + col], &rounding, cos_bit);
        tmp[3]  = half_btf_avx2(&cospi54, &in[13 * 2 + col], &cospim10, &in[2 * 2 + col], &rounding, cos_bit);
        tmp[4]  = half_btf_avx2(&cospi18, &in[11 * 2 + col], &cospi46, &in[4 * 2 + col], &rounding, cos_bit);
        tmp[5]  = half_btf_avx2(&cospi46, &in[11 * 2 + col], &cospim18, &in[4 * 2 + col], &rounding, cos_bit);
        tmp[6]  = half_btf_avx2(&cospi26, &in[9 * 2 + col], &cospi38, &in[6 * 2 + col], &rounding, cos_bit);
        tmp[7]  = half_btf_avx2(&cospi38, &in[9 * 2 + col], &cospim26, &in[6 * 2 + col], &rounding, cos_bit);
        tmp[8]  = half_btf_avx2(&cospi34, &in[7 * 2 + col], &cospi30, &in[8 * 2 + col], &rounding, cos_bit);
        tmp[9]  = half_btf_avx2(&cospi30, &in[7 * 2 + col], &cospim34, &in[8 * 2 + col], &rounding, cos_bit);
        tmp[10] = half_btf_avx2(&cospi42, &in[5 * 2 + col], &cospi22, &in[10 * 2 + col], &rounding, cos_bit);
        tmp[11] = half_btf_avx2(&cospi22, &in[5 * 2 + col], &cospim42, &in[10 * 2 + col], &rounding, cos_bit);
        tmp[12] = half_btf_avx2(&cospi50, &in[3 * 2 + col], &cospi14, &in[12 * 2 + col], &rounding, cos_bit);
        tmp[13] = half_btf_avx2(&cospi14, &in[3 * 2 + col], &cospim50, &in[12 * 2 + col], &rounding, cos_bit);
        tmp[14] = half_btf_avx2(&cospi58, &in[1 * 2 + col], &cospi6, &in[14 * 2 + col], &rounding, cos_bit);
        tmp[15] = half_btf_avx2(&cospi6, &in[1 * 2 + col], &cospim58, &in[14 * 2 + col], &rounding, cos_bit);

        //stage 3
        tmp3[0]  = _mm256_add_epi32(tmp[0], tmp[8]);
        tmp3[1]  = _mm256_add_epi32(tmp[1], tmp[9]);
        tmp3[2]  = _mm256_add_epi32(tmp[2], tmp[10]);
        tmp3[3]  = _mm256_add_epi32(tmp[3], tmp[11]);
        tmp3[4]  = _mm256_add_epi32(tmp[4], tmp[12]);
        tmp3[5]  = _mm256_add_epi32(tmp[5], tmp[13]);
        tmp3[6]  = _mm256_add_epi32(tmp[6], tmp[14]);
        tmp3[7]  = _mm256_add_epi32(tmp[7], tmp[15]);
        tmp2[8]  = _mm256_sub_epi32(tmp[0], tmp[8]);
        tmp2[9]  = _mm256_sub_epi32(tmp[1], tmp[9]);
        tmp2[10] = _mm256_sub_epi32(tmp[2], tmp[10]);
        tmp2[11] = _mm256_sub_epi32(tmp[3], tmp[11]);
        tmp2[12] = _mm256_sub_epi32(tmp[4], tmp[12]);
        tmp2[13] = _mm256_sub_epi32(tmp[5], tmp[13]);
        tmp2[14] = _mm256_sub_epi32(tmp[6], tmp[14]);
        tmp2[15] = _mm256_sub_epi32(tmp[7], tmp[15]);

        //stage 4
        tmp[8]  = half_btf_avx2(&cospi8, &tmp2[8], &cospi56, &tmp2[9], &rounding, cos_bit);
        tmp[9]  = half_btf_avx2(&cospi56, &tmp2[8], &cospim8, &tmp2[9], &rounding, cos_bit);
        tmp[10] = half_btf_avx2(&cospi40, &tmp2[10], &cospi24, &tmp2[11], &rounding, cos_bit);
        tmp[11] = half_btf_avx2(&cospi24, &tmp2[10], &cospim40, &tmp2[11], &rounding, cos_bit);
        tmp[12] = half_btf_avx2(&cospim56, &tmp2[12], &cospi8, &tmp2[13], &rounding, cos_bit);
        tmp[13] = half_btf_avx2(&cospi8, &tmp2[12], &cospi56, &tmp2[13], &rounding, cos_bit);
        tmp[14] = half_btf_avx2(&cospim24, &tmp2[14], &cospi40, &tmp2[15], &rounding, cos_bit);
        tmp[15] = half_btf_avx2(&cospi40, &tmp2[14], &cospi24, &tmp2[15], &rounding, cos_bit);

        //stage 5
        tmp3[8]  = _mm256_add_epi32(tmp3[0], tmp3[4]);
        tmp3[9]  = _mm256_add_epi32(tmp3[1], tmp3[5]);
        tmp3[10] = _mm256_add_epi32(tmp3[2], tmp3[6]);
        tmp3[11] = _mm256_add_epi32(tmp3[3], tmp3[7]);
        tmp2[4]  = _mm256_sub_epi32(tmp3[0], tmp3[4]);
        tmp2[5]  = _mm256_sub_epi32(tmp3[1], tmp3[5]);
        tmp2[6]  = _mm256_sub_epi32(tmp3[2], tmp3[6]);
        tmp2[7]  = _mm256_sub_epi32(tmp3[3], tmp3[7]);
        tmp3[12] = _mm256_add_epi32(tmp[8], tmp[12]);
        tmp3[13] = _mm256_add_epi32(tmp[9], tmp[13]);
        tmp3[14] = _mm256_add_epi32(tmp[10], tmp[14]);
        tmp3[15] = _mm256_add_epi32(tmp[11], tmp[15]);
        tmp2[12] = _mm256_sub_epi32(tmp[8], tmp[12]);
        tmp2[13] = _mm256_sub_epi32(tmp[9], tmp[13]);
        tmp2[14] = _mm256_sub_epi32(tmp[10], tmp[14]);
        tmp2[15] = _mm256_sub_epi32(tmp[11], tmp[15]);

        //stage 6
        tmp[4]  = half_btf_avx2(&cospi16, &tmp2[4], &cospi48, &tmp2[5], &rounding, cos_bit);
        tmp[5]  = half_btf_avx2(&cospi48, &tmp2[4], &cospim16, &tmp2[5], &rounding, cos_bit);
        tmp[6]  = half_btf_avx2(&cospim48, &tmp2[6], &cospi16, &tmp2[7], &rounding, cos_bit);
        tmp[7]  = half_btf_avx2(&cospi16, &tmp2[6], &cospi48, &tmp2[7], &rounding, cos_bit);
        tmp[12] = half_btf_avx2(&cospi16, &tmp2[12], &cospi48, &tmp2[13], &rounding, cos_bit);
        tmp[13] = half_btf_avx2(&cospi48, &tmp2[12], &cospim16, &tmp2[13], &rounding, cos_bit);
        tmp[14] = half_btf_avx2(&cospim48, &tmp2[14], &cospi16, &tmp2[15], &rounding, cos_bit);
        tmp[15] = half_btf_avx2(&cospi16, &tmp2[14], &cospi48, &tmp2[15], &rounding, cos_bit);

        //stage 7
        out[0 * 2 + col]  = _mm256_add_epi32(tmp3[8], tmp3[10]);
        out[2 * 2 + col]  = _mm256_add_epi32(tmp[12], tmp[14]);
        out[12 * 2 + col] = _mm256_add_epi32(tmp[5], tmp[7]);
        out[14 * 2 + col] = _mm256_add_epi32(tmp3[13], tmp3[15]);
        tmp2[1]           = _mm256_add_epi32(tmp3[9], tmp3[11]);
        tmp2[2]           = _mm256_sub_epi32(tmp3[8], tmp3[10]);
        tmp2[3]           = _mm256_sub_epi32(tmp3[9], tmp3[11]);
        tmp2[4]           = _mm256_add_epi32(tmp[4], tmp[6]);
        tmp2[6]           = _mm256_sub_epi32(tmp[4], tmp[6]);
        tmp2[7]           = _mm256_sub_epi32(tmp[5], tmp[7]);
        tmp2[8]           = _mm256_add_epi32(tmp3[12], tmp3[14]);
        tmp2[10]          = _mm256_sub_epi32(tmp3[12], tmp3[14]);
        tmp2[11]          = _mm256_sub_epi32(tmp3[13], tmp3[15]);
        tmp2[13]          = _mm256_add_epi32(tmp[13], tmp[15]);
        tmp2[14]          = _mm256_sub_epi32(tmp[12], tmp[14]);
        tmp2[15]          = _mm256_sub_epi32(tmp[13], tmp[15]);

        //stage 8
        out[4 * 2 + col]  = half_btf_avx2(&cospi32, &tmp2[6], &cospi32, &tmp2[7], &rounding, cos_bit);
        out[6 * 2 + col]  = half_btf_avx2(&cospi32, &tmp2[10], &cospi32, &tmp2[11], &rounding, cos_bit);
        out[8 * 2 + col]  = half_btf_avx2(&cospi32, &tmp2[2], &cospim32, &tmp2[3], &rounding, cos_bit);
        out[10 * 2 + col] = half_btf_avx2(&cospi32, &tmp2[14], &cospim32, &tmp2[15], &rounding, cos_bit);
        tmp[2]            = half_btf_avx2(&cospi32, &tmp2[2], &cospi32, &tmp2[3], &rounding, cos_bit);
        tmp[7]            = half_btf_avx2(&cospi32, &tmp2[6], &cospim32, &tmp2[7], &rounding, cos_bit);
        tmp[11]           = half_btf_avx2(&cospi32, &tmp2[10], &cospim32, &tmp2[11], &rounding, cos_bit);
        tmp[14]           = half_btf_avx2(&cospi32, &tmp2[14], &cospi32, &tmp2[15], &rounding, cos_bit);
        //range_check_buf(stage, input, bf1, size, stage_range[stage]);

        //stage 9
        out[1 * 2 + col]  = _mm256_sign_epi32(tmp2[8], negative);
        out[3 * 2 + col]  = _mm256_sign_epi32(tmp2[4], negative);
        out[5 * 2 + col]  = _mm256_sign_epi32(tmp[14], negative);
        out[7 * 2 + col]  = _mm256_sign_epi32(tmp[2], negative);
        out[9 * 2 + col]  = _mm256_sign_epi32(tmp[11], negative);
        out[11 * 2 + col] = _mm256_sign_epi32(tmp[7], negative);
        out[13 * 2 + col] = _mm256_sign_epi32(tmp2[13], negative);
        out[15 * 2 + col] = _mm256_sign_epi32(tmp2[1], negative);
    }
}

void svt_av1_inv_txfm2d_add_16x16_avx2(const int32_t* input, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                                       int32_t stride_w, TxType tx_type, int32_t bd) {
    __m256i       in[32], out[32];
    const int8_t* shift   = svt_aom_inv_txfm_shift_ls[TX_16X16];
    const int32_t txw_idx = get_txw_idx(TX_16X16);
    const int32_t txh_idx = get_txh_idx(TX_16X16);

    switch (tx_type) {
    case DCT_DCT:
        load_buffer_16x16(input, in);
        transpose_16x16_avx2(in, out);
        idct16_col_avx2(out, in, inv_cos_bit_row[txw_idx][txh_idx], shift);
        round_shift_16x16(in, -shift[0]);
        transpose_16x16_avx2(in, out);
        idct16_col_avx2(out, in, inv_cos_bit_col[txw_idx][txh_idx], shift);
        round_shift_16x16(in, -shift[1]);
        write_buffer_16x16_new(in, output_r, stride_r, output_w, stride_w, bd);
        break;
    case DCT_ADST:
        load_buffer_16x16(input, in);
        transpose_16x16_avx2(in, out);
        iadst16_col_avx2(out, in, inv_cos_bit_row[txw_idx][txh_idx]);
        round_shift_16x16(in, -shift[0]);
        transpose_16x16_avx2(in, out);
        idct16_col_avx2(out, in, inv_cos_bit_col[txw_idx][txh_idx], shift);
        round_shift_16x16(in, -shift[1]);
        write_buffer_16x16_new(in, output_r, stride_r, output_w, stride_w, bd);
        break;
    case ADST_DCT:
        load_buffer_16x16(input, in);
        transpose_16x16_avx2(in, out);
        idct16_col_avx2(out, in, inv_cos_bit_row[txw_idx][txh_idx], shift);
        round_shift_16x16(in, -shift[0]);
        transpose_16x16_avx2(in, out);
        iadst16_col_avx2(out, in, inv_cos_bit_col[txw_idx][txh_idx]);
        round_shift_16x16(in, -shift[1]);
        write_buffer_16x16_new(in, output_r, stride_r, output_w, stride_w, bd);
        break;
    case ADST_ADST:
        load_buffer_16x16(input, in);
        transpose_16x16_avx2(in, out);
        iadst16_col_avx2(out, in, inv_cos_bit_row[txw_idx][txh_idx]);
        round_shift_16x16(in, -shift[0]);
        transpose_16x16_avx2(in, out);
        iadst16_col_avx2(out, in, inv_cos_bit_col[txw_idx][txh_idx]);
        round_shift_16x16(in, -shift[1]);
        write_buffer_16x16_new(in, output_r, stride_r, output_w, stride_w, bd);
        break;
    case IDTX:
        load_buffer_16x16(input, in);
        iidentity16_and_round_shift_avx2(in, -shift[0]);
        iidentity16_and_round_shift_avx2(in, -shift[1]);
        write_buffer_16x16(in, output_r, stride_r, output_w, stride_w, 0, 0, bd);
        break;
    case V_DCT:
        load_buffer_16x16(input, in);
        iidentity16_and_round_shift_avx2(in, -shift[0]);
        idct16_col_avx2(in, out, inv_cos_bit_col[txw_idx][txh_idx], shift);
        round_shift_16x16(out, -shift[1]);
        write_buffer_16x16(out, output_r, stride_r, output_w, stride_w, 0, 0, bd);
        break;
    case H_DCT:
        load_buffer_16x16(input, in);
        transpose_16x16_avx2(in, out);
        idct16_col_avx2(out, in, inv_cos_bit_row[txw_idx][txh_idx], shift);
        transpose_16x16_avx2(in, out);
        round_shift_16x16(out, -shift[0]);
        iidentity16_and_round_shift_avx2(out, -shift[1]);
        write_buffer_16x16(out, output_r, stride_r, output_w, stride_w, 0, 0, bd);
        break;
    case V_ADST:
        load_buffer_16x16(input, in);
        iidentity16_and_round_shift_avx2(in, -shift[0]);
        iadst16_col_avx2(in, out, inv_cos_bit_col[txw_idx][txh_idx]);
        round_shift_16x16(out, -shift[1]);
        write_buffer_16x16(out, output_r, stride_r, output_w, stride_w, 0, 0, bd);
        break;
    case H_ADST:
        load_buffer_16x16(input, in);
        transpose_16x16_avx2(in, out);
        iadst16_col_avx2(out, in, inv_cos_bit_row[txw_idx][txh_idx]);
        transpose_16x16_avx2(in, out);
        round_shift_16x16(out, -shift[0]);
        iidentity16_and_round_shift_avx2(out, -shift[1]);
        write_buffer_16x16(out, output_r, stride_r, output_w, stride_w, 0, 0, bd);
        break;
    case V_FLIPADST:
        load_buffer_16x16(input, in);
        iidentity16_and_round_shift_avx2(in, -shift[0]);
        iadst16_col_avx2(in, out, inv_cos_bit_col[txw_idx][txh_idx]);
        round_shift_16x16(out, -shift[1]);
        write_buffer_16x16(out, output_r, stride_r, output_w, stride_w, 0, 1, bd);
        break;
    case H_FLIPADST:
        load_buffer_16x16(input, in);
        transpose_16x16_avx2(in, out);
        iadst16_col_avx2(out, in, inv_cos_bit_row[txw_idx][txh_idx]);
        transpose_16x16_avx2(in, out);
        round_shift_16x16(out, -shift[0]);
        iidentity16_and_round_shift_avx2(out, -shift[1]);
        write_buffer_16x16(out, output_r, stride_r, output_w, stride_w, 1, 0, bd);
        break;
    default:
        svt_av1_inv_txfm2d_add_16x16_sse4_1(input, output_r, stride_r, output_w, stride_w, tx_type, bd);
        break;
    }
}

// Note:
//  Total 32x4 registers to represent 32x32 block coefficients.
//  For high bit depth, each coefficient is 4-byte.
//  Each __m256i register holds 8 coefficients.
//  So each "row" we needs 4 register. Totally 32 rows
//  Register layout:
//   v0,   v1,   v2,   v3,
//   v4,   v5,   v6,   v7,
//   ... ...
//   v124, v125, v126, v127

static void transpose_32x32_8x8(const __m256i* in, __m256i* out) {
    __m256i u0, u1, u2, u3, u4, u5, u6, u7;
    __m256i x0, x1;

    u0 = _mm256_unpacklo_epi32(in[0], in[4]);
    u1 = _mm256_unpackhi_epi32(in[0], in[4]);

    u2 = _mm256_unpacklo_epi32(in[8], in[12]);
    u3 = _mm256_unpackhi_epi32(in[8], in[12]);

    u4 = _mm256_unpacklo_epi32(in[16], in[20]);
    u5 = _mm256_unpackhi_epi32(in[16], in[20]);

    u6 = _mm256_unpacklo_epi32(in[24], in[28]);
    u7 = _mm256_unpackhi_epi32(in[24], in[28]);

    x0      = _mm256_unpacklo_epi64(u0, u2);
    x1      = _mm256_unpacklo_epi64(u4, u6);
    out[0]  = _mm256_permute2f128_si256(x0, x1, 0x20);
    out[16] = _mm256_permute2f128_si256(x0, x1, 0x31);

    x0      = _mm256_unpackhi_epi64(u0, u2);
    x1      = _mm256_unpackhi_epi64(u4, u6);
    out[4]  = _mm256_permute2f128_si256(x0, x1, 0x20);
    out[20] = _mm256_permute2f128_si256(x0, x1, 0x31);

    x0      = _mm256_unpacklo_epi64(u1, u3);
    x1      = _mm256_unpacklo_epi64(u5, u7);
    out[8]  = _mm256_permute2f128_si256(x0, x1, 0x20);
    out[24] = _mm256_permute2f128_si256(x0, x1, 0x31);

    x0      = _mm256_unpackhi_epi64(u1, u3);
    x1      = _mm256_unpackhi_epi64(u5, u7);
    out[12] = _mm256_permute2f128_si256(x0, x1, 0x20);
    out[28] = _mm256_permute2f128_si256(x0, x1, 0x31);
}

static void transpose_32x32_16x16(const __m256i* in, __m256i* out) {
    transpose_32x32_8x8(&in[0], &out[0]);
    transpose_32x32_8x8(&in[1], &out[32]);
    transpose_32x32_8x8(&in[32], &out[1]);
    transpose_32x32_8x8(&in[33], &out[33]);
}

static void transpose_32x32(const __m256i* in, __m256i* out) {
    transpose_32x32_16x16(&in[0], &out[0]);
    transpose_32x32_16x16(&in[2], &out[64]);
    transpose_32x32_16x16(&in[64], &out[2]);
    transpose_32x32_16x16(&in[66], &out[66]);
}

static void load_buffer_32x32_new(const int32_t* coeff, __m256i* in, int32_t input_stiride, int32_t size) {
    int32_t i;
    for (i = 0; i < size; ++i) {
        in[i] = _mm256_loadu_si256((const __m256i*)(coeff + i * input_stiride));
    }
}

static void load_buffer_32x32(const int32_t* coeff, __m256i* in) {
    int32_t i;
    for (i = 0; i < 128; ++i) {
        in[i] = _mm256_loadu_si256((const __m256i*)coeff);
        coeff += 8;
    }
}

static INLINE __m256i half_btf_0_avx2(const __m256i* w0, const __m256i* n0, const __m256i* rounding, int32_t bit) {
    __m256i x;
    x = _mm256_mullo_epi32(*w0, *n0);
    x = _mm256_add_epi32(x, *rounding);
    x = _mm256_srai_epi32(x, bit);
    return x;
}

static INLINE void round_shift_32x32(__m256i* in, int32_t shift) {
    __m256i rnding = _mm256_set1_epi32(1 << (shift - 1));
    int32_t i      = 0;

    while (i < 128) {
        in[i] = _mm256_add_epi32(in[i], rnding);
        in[i] = _mm256_srai_epi32(in[i], shift);
        i++;
    }
}

static void write_buffer_32x32(__m256i* in, uint16_t* output_r, int32_t stride_r, uint16_t* output_w, int32_t stride_w,
                               int32_t fliplr, int32_t flipud, int32_t bd) {
    __m256i       u0, u1, x0, x1, x2, x3, v0, v1, v2, v3;
    const __m256i zero = _mm256_setzero_si256();
    int32_t       i    = 0;
    (void)fliplr;
    (void)flipud;

    while (i < 128) {
        u0 = _mm256_loadu_si256((const __m256i*)output_r);
        u1 = _mm256_loadu_si256((const __m256i*)(output_r + 16));

        x0 = _mm256_unpacklo_epi16(u0, zero);
        x1 = _mm256_unpackhi_epi16(u0, zero);
        x2 = _mm256_unpacklo_epi16(u1, zero);
        x3 = _mm256_unpackhi_epi16(u1, zero);

        v0 = _mm256_permute2f128_si256(in[i], in[i + 1], 0x20);
        v1 = _mm256_permute2f128_si256(in[i], in[i + 1], 0x31);
        v2 = _mm256_permute2f128_si256(in[i + 2], in[i + 3], 0x20);
        v3 = _mm256_permute2f128_si256(in[i + 2], in[i + 3], 0x31);

        v0 = _mm256_add_epi32(v0, x0);
        v1 = _mm256_add_epi32(v1, x1);
        v2 = _mm256_add_epi32(v2, x2);
        v3 = _mm256_add_epi32(v3, x3);

        highbd_clamp_epi32(&v0, bd);
        highbd_clamp_epi32(&v1, bd);
        highbd_clamp_epi32(&v2, bd);
        highbd_clamp_epi32(&v3, bd);

        v0 = _mm256_packus_epi32(v0, v1);
        v2 = _mm256_packus_epi32(v2, v3);

        _mm256_storeu_si256((__m256i*)output_w, v0);
        _mm256_storeu_si256((__m256i*)(output_w + 16), v2);
        output_r += stride_r;
        output_w += stride_w;
        i += 4;
    }
}

static void idct32_avx2(__m256i* in, __m256i* out, int32_t bit) {
    const int32_t* cospi    = cospi_arr(bit);
    const __m256i  cospi62  = _mm256_set1_epi32(cospi[62]);
    const __m256i  cospi30  = _mm256_set1_epi32(cospi[30]);
    const __m256i  cospi46  = _mm256_set1_epi32(cospi[46]);
    const __m256i  cospi14  = _mm256_set1_epi32(cospi[14]);
    const __m256i  cospi54  = _mm256_set1_epi32(cospi[54]);
    const __m256i  cospi22  = _mm256_set1_epi32(cospi[22]);
    const __m256i  cospi38  = _mm256_set1_epi32(cospi[38]);
    const __m256i  cospi6   = _mm256_set1_epi32(cospi[6]);
    const __m256i  cospi58  = _mm256_set1_epi32(cospi[58]);
    const __m256i  cospi26  = _mm256_set1_epi32(cospi[26]);
    const __m256i  cospi42  = _mm256_set1_epi32(cospi[42]);
    const __m256i  cospi10  = _mm256_set1_epi32(cospi[10]);
    const __m256i  cospi50  = _mm256_set1_epi32(cospi[50]);
    const __m256i  cospi18  = _mm256_set1_epi32(cospi[18]);
    const __m256i  cospi34  = _mm256_set1_epi32(cospi[34]);
    const __m256i  cospi2   = _mm256_set1_epi32(cospi[2]);
    const __m256i  cospim58 = _mm256_set1_epi32(-cospi[58]);
    const __m256i  cospim26 = _mm256_set1_epi32(-cospi[26]);
    const __m256i  cospim42 = _mm256_set1_epi32(-cospi[42]);
    const __m256i  cospim10 = _mm256_set1_epi32(-cospi[10]);
    const __m256i  cospim50 = _mm256_set1_epi32(-cospi[50]);
    const __m256i  cospim18 = _mm256_set1_epi32(-cospi[18]);
    const __m256i  cospim34 = _mm256_set1_epi32(-cospi[34]);
    const __m256i  cospim2  = _mm256_set1_epi32(-cospi[2]);
    const __m256i  cospi60  = _mm256_set1_epi32(cospi[60]);
    const __m256i  cospi28  = _mm256_set1_epi32(cospi[28]);
    const __m256i  cospi44  = _mm256_set1_epi32(cospi[44]);
    const __m256i  cospi12  = _mm256_set1_epi32(cospi[12]);
    const __m256i  cospi52  = _mm256_set1_epi32(cospi[52]);
    const __m256i  cospi20  = _mm256_set1_epi32(cospi[20]);
    const __m256i  cospi36  = _mm256_set1_epi32(cospi[36]);
    const __m256i  cospi4   = _mm256_set1_epi32(cospi[4]);
    const __m256i  cospim52 = _mm256_set1_epi32(-cospi[52]);
    const __m256i  cospim20 = _mm256_set1_epi32(-cospi[20]);
    const __m256i  cospim36 = _mm256_set1_epi32(-cospi[36]);
    const __m256i  cospim4  = _mm256_set1_epi32(-cospi[4]);
    const __m256i  cospi56  = _mm256_set1_epi32(cospi[56]);
    const __m256i  cospi24  = _mm256_set1_epi32(cospi[24]);
    const __m256i  cospi40  = _mm256_set1_epi32(cospi[40]);
    const __m256i  cospi8   = _mm256_set1_epi32(cospi[8]);
    const __m256i  cospim40 = _mm256_set1_epi32(-cospi[40]);
    const __m256i  cospim8  = _mm256_set1_epi32(-cospi[8]);
    const __m256i  cospim56 = _mm256_set1_epi32(-cospi[56]);
    const __m256i  cospim24 = _mm256_set1_epi32(-cospi[24]);
    const __m256i  cospi32  = _mm256_set1_epi32(cospi[32]);
    const __m256i  cospim32 = _mm256_set1_epi32(-cospi[32]);
    const __m256i  cospi48  = _mm256_set1_epi32(cospi[48]);
    const __m256i  cospim48 = _mm256_set1_epi32(-cospi[48]);
    const __m256i  cospi16  = _mm256_set1_epi32(cospi[16]);
    const __m256i  cospim16 = _mm256_set1_epi32(-cospi[16]);
    const __m256i  rounding = _mm256_set1_epi32(1 << (bit - 1));
    __m256i        bf1[32], bf0[32];
    int32_t        col;

    for (col = 0; col < 4; ++col) {
        // stage 0
        // stage 1
        bf1[0]  = in[0 * 4 + col];
        bf1[1]  = in[16 * 4 + col];
        bf1[2]  = in[8 * 4 + col];
        bf1[3]  = in[24 * 4 + col];
        bf1[4]  = in[4 * 4 + col];
        bf1[5]  = in[20 * 4 + col];
        bf1[6]  = in[12 * 4 + col];
        bf1[7]  = in[28 * 4 + col];
        bf1[8]  = in[2 * 4 + col];
        bf1[9]  = in[18 * 4 + col];
        bf1[10] = in[10 * 4 + col];
        bf1[11] = in[26 * 4 + col];
        bf1[12] = in[6 * 4 + col];
        bf1[13] = in[22 * 4 + col];
        bf1[14] = in[14 * 4 + col];
        bf1[15] = in[30 * 4 + col];
        bf1[16] = in[1 * 4 + col];
        bf1[17] = in[17 * 4 + col];
        bf1[18] = in[9 * 4 + col];
        bf1[19] = in[25 * 4 + col];
        bf1[20] = in[5 * 4 + col];
        bf1[21] = in[21 * 4 + col];
        bf1[22] = in[13 * 4 + col];
        bf1[23] = in[29 * 4 + col];
        bf1[24] = in[3 * 4 + col];
        bf1[25] = in[19 * 4 + col];
        bf1[26] = in[11 * 4 + col];
        bf1[27] = in[27 * 4 + col];
        bf1[28] = in[7 * 4 + col];
        bf1[29] = in[23 * 4 + col];
        bf1[30] = in[15 * 4 + col];
        bf1[31] = in[31 * 4 + col];

        // stage 2
        bf0[0]  = bf1[0];
        bf0[1]  = bf1[1];
        bf0[2]  = bf1[2];
        bf0[3]  = bf1[3];
        bf0[4]  = bf1[4];
        bf0[5]  = bf1[5];
        bf0[6]  = bf1[6];
        bf0[7]  = bf1[7];
        bf0[8]  = bf1[8];
        bf0[9]  = bf1[9];
        bf0[10] = bf1[10];
        bf0[11] = bf1[11];
        bf0[12] = bf1[12];
        bf0[13] = bf1[13];
        bf0[14] = bf1[14];
        bf0[15] = bf1[15];
        bf0[16] = half_btf_avx2(&cospi62, &bf1[16], &cospim2, &bf1[31], &rounding, bit);
        bf0[17] = half_btf_avx2(&cospi30, &bf1[17], &cospim34, &bf1[30], &rounding, bit);
        bf0[18] = half_btf_avx2(&cospi46, &bf1[18], &cospim18, &bf1[29], &rounding, bit);
        bf0[19] = half_btf_avx2(&cospi14, &bf1[19], &cospim50, &bf1[28], &rounding, bit);
        bf0[20] = half_btf_avx2(&cospi54, &bf1[20], &cospim10, &bf1[27], &rounding, bit);
        bf0[21] = half_btf_avx2(&cospi22, &bf1[21], &cospim42, &bf1[26], &rounding, bit);
        bf0[22] = half_btf_avx2(&cospi38, &bf1[22], &cospim26, &bf1[25], &rounding, bit);
        bf0[23] = half_btf_avx2(&cospi6, &bf1[23], &cospim58, &bf1[24], &rounding, bit);
        bf0[24] = half_btf_avx2(&cospi58, &bf1[23], &cospi6, &bf1[24], &rounding, bit);
        bf0[25] = half_btf_avx2(&cospi26, &bf1[22], &cospi38, &bf1[25], &rounding, bit);
        bf0[26] = half_btf_avx2(&cospi42, &bf1[21], &cospi22, &bf1[26], &rounding, bit);
        bf0[27] = half_btf_avx2(&cospi10, &bf1[20], &cospi54, &bf1[27], &rounding, bit);
        bf0[28] = half_btf_avx2(&cospi50, &bf1[19], &cospi14, &bf1[28], &rounding, bit);
        bf0[29] = half_btf_avx2(&cospi18, &bf1[18], &cospi46, &bf1[29], &rounding, bit);
        bf0[30] = half_btf_avx2(&cospi34, &bf1[17], &cospi30, &bf1[30], &rounding, bit);
        bf0[31] = half_btf_avx2(&cospi2, &bf1[16], &cospi62, &bf1[31], &rounding, bit);

        // stage 3
        bf1[0]  = bf0[0];
        bf1[1]  = bf0[1];
        bf1[2]  = bf0[2];
        bf1[3]  = bf0[3];
        bf1[4]  = bf0[4];
        bf1[5]  = bf0[5];
        bf1[6]  = bf0[6];
        bf1[7]  = bf0[7];
        bf1[8]  = half_btf_avx2(&cospi60, &bf0[8], &cospim4, &bf0[15], &rounding, bit);
        bf1[9]  = half_btf_avx2(&cospi28, &bf0[9], &cospim36, &bf0[14], &rounding, bit);
        bf1[10] = half_btf_avx2(&cospi44, &bf0[10], &cospim20, &bf0[13], &rounding, bit);
        bf1[11] = half_btf_avx2(&cospi12, &bf0[11], &cospim52, &bf0[12], &rounding, bit);
        bf1[12] = half_btf_avx2(&cospi52, &bf0[11], &cospi12, &bf0[12], &rounding, bit);
        bf1[13] = half_btf_avx2(&cospi20, &bf0[10], &cospi44, &bf0[13], &rounding, bit);
        bf1[14] = half_btf_avx2(&cospi36, &bf0[9], &cospi28, &bf0[14], &rounding, bit);
        bf1[15] = half_btf_avx2(&cospi4, &bf0[8], &cospi60, &bf0[15], &rounding, bit);
        bf1[16] = _mm256_add_epi32(bf0[16], bf0[17]);
        bf1[17] = _mm256_sub_epi32(bf0[16], bf0[17]);
        bf1[18] = _mm256_sub_epi32(bf0[19], bf0[18]);
        bf1[19] = _mm256_add_epi32(bf0[18], bf0[19]);
        bf1[20] = _mm256_add_epi32(bf0[20], bf0[21]);
        bf1[21] = _mm256_sub_epi32(bf0[20], bf0[21]);
        bf1[22] = _mm256_sub_epi32(bf0[23], bf0[22]);
        bf1[23] = _mm256_add_epi32(bf0[22], bf0[23]);
        bf1[24] = _mm256_add_epi32(bf0[24], bf0[25]);
        bf1[25] = _mm256_sub_epi32(bf0[24], bf0[25]);
        bf1[26] = _mm256_sub_epi32(bf0[27], bf0[26]);
        bf1[27] = _mm256_add_epi32(bf0[26], bf0[27]);
        bf1[28] = _mm256_add_epi32(bf0[28], bf0[29]);
        bf1[29] = _mm256_sub_epi32(bf0[28], bf0[29]);
        bf1[30] = _mm256_sub_epi32(bf0[31], bf0[30]);
        bf1[31] = _mm256_add_epi32(bf0[30], bf0[31]);

        // stage 4
        bf0[0]  = bf1[0];
        bf0[1]  = bf1[1];
        bf0[2]  = bf1[2];
        bf0[3]  = bf1[3];
        bf0[4]  = half_btf_avx2(&cospi56, &bf1[4], &cospim8, &bf1[7], &rounding, bit);
        bf0[5]  = half_btf_avx2(&cospi24, &bf1[5], &cospim40, &bf1[6], &rounding, bit);
        bf0[6]  = half_btf_avx2(&cospi40, &bf1[5], &cospi24, &bf1[6], &rounding, bit);
        bf0[7]  = half_btf_avx2(&cospi8, &bf1[4], &cospi56, &bf1[7], &rounding, bit);
        bf0[8]  = _mm256_add_epi32(bf1[8], bf1[9]);
        bf0[9]  = _mm256_sub_epi32(bf1[8], bf1[9]);
        bf0[10] = _mm256_sub_epi32(bf1[11], bf1[10]);
        bf0[11] = _mm256_add_epi32(bf1[10], bf1[11]);
        bf0[12] = _mm256_add_epi32(bf1[12], bf1[13]);
        bf0[13] = _mm256_sub_epi32(bf1[12], bf1[13]);
        bf0[14] = _mm256_sub_epi32(bf1[15], bf1[14]);
        bf0[15] = _mm256_add_epi32(bf1[14], bf1[15]);
        bf0[16] = bf1[16];
        bf0[17] = half_btf_avx2(&cospim8, &bf1[17], &cospi56, &bf1[30], &rounding, bit);
        bf0[18] = half_btf_avx2(&cospim56, &bf1[18], &cospim8, &bf1[29], &rounding, bit);
        bf0[19] = bf1[19];
        bf0[20] = bf1[20];
        bf0[21] = half_btf_avx2(&cospim40, &bf1[21], &cospi24, &bf1[26], &rounding, bit);
        bf0[22] = half_btf_avx2(&cospim24, &bf1[22], &cospim40, &bf1[25], &rounding, bit);
        bf0[23] = bf1[23];
        bf0[24] = bf1[24];
        bf0[25] = half_btf_avx2(&cospim40, &bf1[22], &cospi24, &bf1[25], &rounding, bit);
        bf0[26] = half_btf_avx2(&cospi24, &bf1[21], &cospi40, &bf1[26], &rounding, bit);
        bf0[27] = bf1[27];
        bf0[28] = bf1[28];
        bf0[29] = half_btf_avx2(&cospim8, &bf1[18], &cospi56, &bf1[29], &rounding, bit);
        bf0[30] = half_btf_avx2(&cospi56, &bf1[17], &cospi8, &bf1[30], &rounding, bit);
        bf0[31] = bf1[31];

        // stage 5
        bf1[0]  = half_btf_avx2(&cospi32, &bf0[0], &cospi32, &bf0[1], &rounding, bit);
        bf1[1]  = half_btf_avx2(&cospi32, &bf0[0], &cospim32, &bf0[1], &rounding, bit);
        bf1[2]  = half_btf_avx2(&cospi48, &bf0[2], &cospim16, &bf0[3], &rounding, bit);
        bf1[3]  = half_btf_avx2(&cospi16, &bf0[2], &cospi48, &bf0[3], &rounding, bit);
        bf1[4]  = _mm256_add_epi32(bf0[4], bf0[5]);
        bf1[5]  = _mm256_sub_epi32(bf0[4], bf0[5]);
        bf1[6]  = _mm256_sub_epi32(bf0[7], bf0[6]);
        bf1[7]  = _mm256_add_epi32(bf0[6], bf0[7]);
        bf1[8]  = bf0[8];
        bf1[9]  = half_btf_avx2(&cospim16, &bf0[9], &cospi48, &bf0[14], &rounding, bit);
        bf1[10] = half_btf_avx2(&cospim48, &bf0[10], &cospim16, &bf0[13], &rounding, bit);
        bf1[11] = bf0[11];
        bf1[12] = bf0[12];
        bf1[13] = half_btf_avx2(&cospim16, &bf0[10], &cospi48, &bf0[13], &rounding, bit);
        bf1[14] = half_btf_avx2(&cospi48, &bf0[9], &cospi16, &bf0[14], &rounding, bit);
        bf1[15] = bf0[15];
        bf1[16] = _mm256_add_epi32(bf0[16], bf0[19]);
        bf1[17] = _mm256_add_epi32(bf0[17], bf0[18]);
        bf1[18] = _mm256_sub_epi32(bf0[17], bf0[18]);
        bf1[19] = _mm256_sub_epi32(bf0[16], bf0[19]);
        bf1[20] = _mm256_sub_epi32(bf0[23], bf0[20]);
        bf1[21] = _mm256_sub_epi32(bf0[22], bf0[21]);
        bf1[22] = _mm256_add_epi32(bf0[21], bf0[22]);
        bf1[23] = _mm256_add_epi32(bf0[20], bf0[23]);
        bf1[24] = _mm256_add_epi32(bf0[24], bf0[27]);
        bf1[25] = _mm256_add_epi32(bf0[25], bf0[26]);
        bf1[26] = _mm256_sub_epi32(bf0[25], bf0[26]);
        bf1[27] = _mm256_sub_epi32(bf0[24], bf0[27]);
        bf1[28] = _mm256_sub_epi32(bf0[31], bf0[28]);
        bf1[29] = _mm256_sub_epi32(bf0[30], bf0[29]);
        bf1[30] = _mm256_add_epi32(bf0[29], bf0[30]);
        bf1[31] = _mm256_add_epi32(bf0[28], bf0[31]);

        // stage 6
        bf0[0]  = _mm256_add_epi32(bf1[0], bf1[3]);
        bf0[1]  = _mm256_add_epi32(bf1[1], bf1[2]);
        bf0[2]  = _mm256_sub_epi32(bf1[1], bf1[2]);
        bf0[3]  = _mm256_sub_epi32(bf1[0], bf1[3]);
        bf0[4]  = bf1[4];
        bf0[5]  = half_btf_avx2(&cospim32, &bf1[5], &cospi32, &bf1[6], &rounding, bit);
        bf0[6]  = half_btf_avx2(&cospi32, &bf1[5], &cospi32, &bf1[6], &rounding, bit);
        bf0[7]  = bf1[7];
        bf0[8]  = _mm256_add_epi32(bf1[8], bf1[11]);
        bf0[9]  = _mm256_add_epi32(bf1[9], bf1[10]);
        bf0[10] = _mm256_sub_epi32(bf1[9], bf1[10]);
        bf0[11] = _mm256_sub_epi32(bf1[8], bf1[11]);
        bf0[12] = _mm256_sub_epi32(bf1[15], bf1[12]);
        bf0[13] = _mm256_sub_epi32(bf1[14], bf1[13]);
        bf0[14] = _mm256_add_epi32(bf1[13], bf1[14]);
        bf0[15] = _mm256_add_epi32(bf1[12], bf1[15]);
        bf0[16] = bf1[16];
        bf0[17] = bf1[17];
        bf0[18] = half_btf_avx2(&cospim16, &bf1[18], &cospi48, &bf1[29], &rounding, bit);
        bf0[19] = half_btf_avx2(&cospim16, &bf1[19], &cospi48, &bf1[28], &rounding, bit);
        bf0[20] = half_btf_avx2(&cospim48, &bf1[20], &cospim16, &bf1[27], &rounding, bit);
        bf0[21] = half_btf_avx2(&cospim48, &bf1[21], &cospim16, &bf1[26], &rounding, bit);
        bf0[22] = bf1[22];
        bf0[23] = bf1[23];
        bf0[24] = bf1[24];
        bf0[25] = bf1[25];
        bf0[26] = half_btf_avx2(&cospim16, &bf1[21], &cospi48, &bf1[26], &rounding, bit);
        bf0[27] = half_btf_avx2(&cospim16, &bf1[20], &cospi48, &bf1[27], &rounding, bit);
        bf0[28] = half_btf_avx2(&cospi48, &bf1[19], &cospi16, &bf1[28], &rounding, bit);
        bf0[29] = half_btf_avx2(&cospi48, &bf1[18], &cospi16, &bf1[29], &rounding, bit);
        bf0[30] = bf1[30];
        bf0[31] = bf1[31];

        // stage 7
        bf1[0]  = _mm256_add_epi32(bf0[0], bf0[7]);
        bf1[1]  = _mm256_add_epi32(bf0[1], bf0[6]);
        bf1[2]  = _mm256_add_epi32(bf0[2], bf0[5]);
        bf1[3]  = _mm256_add_epi32(bf0[3], bf0[4]);
        bf1[4]  = _mm256_sub_epi32(bf0[3], bf0[4]);
        bf1[5]  = _mm256_sub_epi32(bf0[2], bf0[5]);
        bf1[6]  = _mm256_sub_epi32(bf0[1], bf0[6]);
        bf1[7]  = _mm256_sub_epi32(bf0[0], bf0[7]);
        bf1[8]  = bf0[8];
        bf1[9]  = bf0[9];
        bf1[10] = half_btf_avx2(&cospim32, &bf0[10], &cospi32, &bf0[13], &rounding, bit);
        bf1[11] = half_btf_avx2(&cospim32, &bf0[11], &cospi32, &bf0[12], &rounding, bit);
        bf1[12] = half_btf_avx2(&cospi32, &bf0[11], &cospi32, &bf0[12], &rounding, bit);
        bf1[13] = half_btf_avx2(&cospi32, &bf0[10], &cospi32, &bf0[13], &rounding, bit);
        bf1[14] = bf0[14];
        bf1[15] = bf0[15];
        bf1[16] = _mm256_add_epi32(bf0[16], bf0[23]);
        bf1[17] = _mm256_add_epi32(bf0[17], bf0[22]);
        bf1[18] = _mm256_add_epi32(bf0[18], bf0[21]);
        bf1[19] = _mm256_add_epi32(bf0[19], bf0[20]);
        bf1[20] = _mm256_sub_epi32(bf0[19], bf0[20]);
        bf1[21] = _mm256_sub_epi32(bf0[18], bf0[21]);
        bf1[22] = _mm256_sub_epi32(bf0[17], bf0[22]);
        bf1[23] = _mm256_sub_epi32(bf0[16], bf0[23]);
        bf1[24] = _mm256_sub_epi32(bf0[31], bf0[24]);
        bf1[25] = _mm256_sub_epi32(bf0[30], bf0[25]);
        bf1[26] = _mm256_sub_epi32(bf0[29], bf0[26]);
        bf1[27] = _mm256_sub_epi32(bf0[28], bf0[27]);
        bf1[28] = _mm256_add_epi32(bf0[27], bf0[28]);
        bf1[29] = _mm256_add_epi32(bf0[26], bf0[29]);
        bf1[30] = _mm256_add_epi32(bf0[25], bf0[30]);
        bf1[31] = _mm256_add_epi32(bf0[24], bf0[31]);

        // stage 8
        bf0[0]  = _mm256_add_epi32(bf1[0], bf1[15]);
        bf0[1]  = _mm256_add_epi32(bf1[1], bf1[14]);
        bf0[2]  = _mm256_add_epi32(bf1[2], bf1[13]);
        bf0[3]  = _mm256_add_epi32(bf1[3], bf1[12]);
        bf0[4]  = _mm256_add_epi32(bf1[4], bf1[11]);
        bf0[5]  = _mm256_add_epi32(bf1[5], bf1[10]);
        bf0[6]  = _mm256_add_epi32(bf1[6], bf1[9]);
        bf0[7]  = _mm256_add_epi32(bf1[7], bf1[8]);
        bf0[8]  = _mm256_sub_epi32(bf1[7], bf1[8]);
        bf0[9]  = _mm256_sub_epi32(bf1[6], bf1[9]);
        bf0[10] = _mm256_sub_epi32(bf1[5], bf1[10]);
        bf0[11] = _mm256_sub_epi32(bf1[4], bf1[11]);
        bf0[12] = _mm256_sub_epi32(bf1[3], bf1[12]);
        bf0[13] = _mm256_sub_epi32(bf1[2], bf1[13]);
        bf0[14] = _mm256_sub_epi32(bf1[1], bf1[14]);
        bf0[15] = _mm256_sub_epi32(bf1[0], bf1[15]);
        bf0[16] = bf1[16];
        bf0[17] = bf1[17];
        bf0[18] = bf1[18];
        bf0[19] = bf1[19];
        bf0[20] = half_btf_avx2(&cospim32, &bf1[20], &cospi32, &bf1[27], &rounding, bit);
        bf0[21] = half_btf_avx2(&cospim32, &bf1[21], &cospi32, &bf1[26], &rounding, bit);
        bf0[22] = half_btf_avx2(&cospim32, &bf1[22], &cospi32, &bf1[25], &rounding, bit);
        bf0[23] = half_btf_avx2(&cospim32, &bf1[23], &cospi32, &bf1[24], &rounding, bit);
        bf0[24] = half_btf_avx2(&cospi32, &bf1[23], &cospi32, &bf1[24], &rounding, bit);
        bf0[25] = half_btf_avx2(&cospi32, &bf1[22], &cospi32, &bf1[25], &rounding, bit);
        bf0[26] = half_btf_avx2(&cospi32, &bf1[21], &cospi32, &bf1[26], &rounding, bit);
        bf0[27] = half_btf_avx2(&cospi32, &bf1[20], &cospi32, &bf1[27], &rounding, bit);
        bf0[28] = bf1[28];
        bf0[29] = bf1[29];
        bf0[30] = bf1[30];
        bf0[31] = bf1[31];

        // stage 9
        out[0 * 4 + col]  = _mm256_add_epi32(bf0[0], bf0[31]);
        out[1 * 4 + col]  = _mm256_add_epi32(bf0[1], bf0[30]);
        out[2 * 4 + col]  = _mm256_add_epi32(bf0[2], bf0[29]);
        out[3 * 4 + col]  = _mm256_add_epi32(bf0[3], bf0[28]);
        out[4 * 4 + col]  = _mm256_add_epi32(bf0[4], bf0[27]);
        out[5 * 4 + col]  = _mm256_add_epi32(bf0[5], bf0[26]);
        out[6 * 4 + col]  = _mm256_add_epi32(bf0[6], bf0[25]);
        out[7 * 4 + col]  = _mm256_add_epi32(bf0[7], bf0[24]);
        out[8 * 4 + col]  = _mm256_add_epi32(bf0[8], bf0[23]);
        out[9 * 4 + col]  = _mm256_add_epi32(bf0[9], bf0[22]);
        out[10 * 4 + col] = _mm256_add_epi32(bf0[10], bf0[21]);
        out[11 * 4 + col] = _mm256_add_epi32(bf0[11], bf0[20]);
        out[12 * 4 + col] = _mm256_add_epi32(bf0[12], bf0[19]);
        out[13 * 4 + col] = _mm256_add_epi32(bf0[13], bf0[18]);
        out[14 * 4 + col] = _mm256_add_epi32(bf0[14], bf0[17]);
        out[15 * 4 + col] = _mm256_add_epi32(bf0[15], bf0[16]);
        out[16 * 4 + col] = _mm256_sub_epi32(bf0[15], bf0[16]);
        out[17 * 4 + col] = _mm256_sub_epi32(bf0[14], bf0[17]);
        out[18 * 4 + col] = _mm256_sub_epi32(bf0[13], bf0[18]);
        out[19 * 4 + col] = _mm256_sub_epi32(bf0[12], bf0[19]);
        out[20 * 4 + col] = _mm256_sub_epi32(bf0[11], bf0[20]);
        out[21 * 4 + col] = _mm256_sub_epi32(bf0[10], bf0[21]);
        out[22 * 4 + col] = _mm256_sub_epi32(bf0[9], bf0[22]);
        out[23 * 4 + col] = _mm256_sub_epi32(bf0[8], bf0[23]);
        out[24 * 4 + col] = _mm256_sub_epi32(bf0[7], bf0[24]);
        out[25 * 4 + col] = _mm256_sub_epi32(bf0[6], bf0[25]);
        out[26 * 4 + col] = _mm256_sub_epi32(bf0[5], bf0[26]);
        out[27 * 4 + col] = _mm256_sub_epi32(bf0[4], bf0[27]);
        out[28 * 4 + col] = _mm256_sub_epi32(bf0[3], bf0[28]);
        out[29 * 4 + col] = _mm256_sub_epi32(bf0[2], bf0[29]);
        out[30 * 4 + col] = _mm256_sub_epi32(bf0[1], bf0[30]);
        out[31 * 4 + col] = _mm256_sub_epi32(bf0[0], bf0[31]);
    }
}

void svt_av1_inv_txfm2d_add_32x32_avx2(const int32_t* coeff, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                                       int32_t stride_w, TxType tx_type, int32_t bd) {
    __m256i       in[128], out[128];
    const int8_t* shift   = svt_aom_inv_txfm_shift_ls[TX_32X32];
    const int32_t txw_idx = get_txw_idx(TX_32X32);
    const int32_t txh_idx = get_txh_idx(TX_32X32);

    switch (tx_type) {
    case DCT_DCT:
        load_buffer_32x32(coeff, in);
        transpose_32x32(in, out);
        idct32_avx2(out, in, inv_cos_bit_row[txw_idx][txh_idx]);
        round_shift_32x32(in, -shift[0]);
        transpose_32x32(in, out);
        idct32_avx2(out, in, inv_cos_bit_col[txw_idx][txh_idx]);
        round_shift_32x32(in, -shift[1]);
        write_buffer_32x32(in, output_r, stride_r, output_w, stride_w, 0, 0, bd);
        break;
    case IDTX:
        load_buffer_32x32(coeff, in);
        // Operations can be joined together without losing precision
        // svt_av1_iidentity32_c() shift left 2 bits
        // round_shift_32x32(, -shift[0]) shift right 2 bits
        // svt_av1_iidentity32_c() shift left 2 bits
        // round_shift_32x32(, -shift[1]) shift right 4 bits with complement
        round_shift_32x32(in, -shift[0] - shift[1] - 4);
        write_buffer_32x32(in, output_r, stride_r, output_w, stride_w, 0, 0, bd);
        break;
    default:
        assert(0);
    }
}

static void idct8x8_low1_avx2(__m256i* in, __m256i* out, int32_t bit, int32_t do_cols, int32_t bd, int32_t out_shift) {
    const int32_t* cospi     = cospi_arr(bit);
    const __m256i  cospi32   = _mm256_set1_epi32(cospi[32]);
    const __m256i  rnding    = _mm256_set1_epi32(1 << (bit - 1));
    const int32_t  log_range = AOMMAX(16, bd + (do_cols ? 6 : 8));
    __m256i        clamp_lo  = _mm256_set1_epi32(-(1 << (log_range - 1)));
    __m256i        clamp_hi  = _mm256_set1_epi32((1 << (log_range - 1)) - 1);
    __m256i        x;

    // stage 0
    // stage 1
    // stage 2
    // stage 3
    x = _mm256_mullo_epi32(in[0], cospi32);
    x = _mm256_add_epi32(x, rnding);
    x = _mm256_srai_epi32(x, bit);

    // stage 4
    // stage 5
    if (!do_cols) {
        const int log_range_out = AOMMAX(16, bd + 6);
        __m256i   offset        = _mm256_set1_epi32((1 << out_shift) >> 1);
        clamp_lo                = _mm256_set1_epi32(-(1 << (log_range_out - 1)));
        clamp_hi                = _mm256_set1_epi32((1 << (log_range_out - 1)) - 1);
        x                       = _mm256_add_epi32(x, offset);
        x                       = _mm256_sra_epi32(x, _mm_cvtsi32_si128(out_shift));
    }
    x      = _mm256_max_epi32(x, clamp_lo);
    x      = _mm256_min_epi32(x, clamp_hi);
    out[0] = x;
    out[1] = x;
    out[2] = x;
    out[3] = x;
    out[4] = x;
    out[5] = x;
    out[6] = x;
    out[7] = x;
}

static void idct8x8_avx2(__m256i* in, __m256i* out, int32_t bit, int32_t do_cols, int32_t bd, int32_t out_shift) {
    const int32_t* cospi     = cospi_arr(bit);
    const __m256i  cospi56   = _mm256_set1_epi32(cospi[56]);
    const __m256i  cospim8   = _mm256_set1_epi32(-cospi[8]);
    const __m256i  cospi24   = _mm256_set1_epi32(cospi[24]);
    const __m256i  cospim40  = _mm256_set1_epi32(-cospi[40]);
    const __m256i  cospi40   = _mm256_set1_epi32(cospi[40]);
    const __m256i  cospi8    = _mm256_set1_epi32(cospi[8]);
    const __m256i  cospi32   = _mm256_set1_epi32(cospi[32]);
    const __m256i  cospi48   = _mm256_set1_epi32(cospi[48]);
    const __m256i  cospim16  = _mm256_set1_epi32(-cospi[16]);
    const __m256i  cospi16   = _mm256_set1_epi32(cospi[16]);
    const __m256i  rnding    = _mm256_set1_epi32(1 << (bit - 1));
    const int32_t  log_range = AOMMAX(16, bd + (do_cols ? 6 : 8));
    const __m256i  clamp_lo  = _mm256_set1_epi32(-(1 << (log_range - 1)));
    const __m256i  clamp_hi  = _mm256_set1_epi32((1 << (log_range - 1)) - 1);
    __m256i        u0, u1, u2, u3, u4, u5, u6, u7;
    __m256i        v0, v1, v2, v3, v4, v5, v6, v7;
    __m256i        x, y;

    // stage 0
    // stage 1
    // stage 2
    u0 = in[0];
    u1 = in[4];
    u2 = in[2];
    u3 = in[6];

    x  = _mm256_mullo_epi32(in[1], cospi56);
    y  = _mm256_mullo_epi32(in[7], cospim8);
    u4 = _mm256_add_epi32(x, y);
    u4 = _mm256_add_epi32(u4, rnding);
    u4 = _mm256_srai_epi32(u4, bit);

    x  = _mm256_mullo_epi32(in[1], cospi8);
    y  = _mm256_mullo_epi32(in[7], cospi56);
    u7 = _mm256_add_epi32(x, y);
    u7 = _mm256_add_epi32(u7, rnding);
    u7 = _mm256_srai_epi32(u7, bit);

    x  = _mm256_mullo_epi32(in[5], cospi24);
    y  = _mm256_mullo_epi32(in[3], cospim40);
    u5 = _mm256_add_epi32(x, y);
    u5 = _mm256_add_epi32(u5, rnding);
    u5 = _mm256_srai_epi32(u5, bit);

    x  = _mm256_mullo_epi32(in[5], cospi40);
    y  = _mm256_mullo_epi32(in[3], cospi24);
    u6 = _mm256_add_epi32(x, y);
    u6 = _mm256_add_epi32(u6, rnding);
    u6 = _mm256_srai_epi32(u6, bit);

    // stage 3
    x  = _mm256_mullo_epi32(u0, cospi32);
    y  = _mm256_mullo_epi32(u1, cospi32);
    v0 = _mm256_add_epi32(x, y);
    v0 = _mm256_add_epi32(v0, rnding);
    v0 = _mm256_srai_epi32(v0, bit);

    v1 = _mm256_sub_epi32(x, y);
    v1 = _mm256_add_epi32(v1, rnding);
    v1 = _mm256_srai_epi32(v1, bit);

    x  = _mm256_mullo_epi32(u2, cospi48);
    y  = _mm256_mullo_epi32(u3, cospim16);
    v2 = _mm256_add_epi32(x, y);
    v2 = _mm256_add_epi32(v2, rnding);
    v2 = _mm256_srai_epi32(v2, bit);

    x  = _mm256_mullo_epi32(u2, cospi16);
    y  = _mm256_mullo_epi32(u3, cospi48);
    v3 = _mm256_add_epi32(x, y);
    v3 = _mm256_add_epi32(v3, rnding);
    v3 = _mm256_srai_epi32(v3, bit);

    addsub_avx2(u4, u5, &v4, &v5, &clamp_lo, &clamp_hi);
    addsub_avx2(u7, u6, &v7, &v6, &clamp_lo, &clamp_hi);

    // stage 4
    addsub_avx2(v0, v3, &u0, &u3, &clamp_lo, &clamp_hi);
    addsub_avx2(v1, v2, &u1, &u2, &clamp_lo, &clamp_hi);
    u4 = v4;
    u7 = v7;

    x  = _mm256_mullo_epi32(v5, cospi32);
    y  = _mm256_mullo_epi32(v6, cospi32);
    u6 = _mm256_add_epi32(y, x);
    u6 = _mm256_add_epi32(u6, rnding);
    u6 = _mm256_srai_epi32(u6, bit);

    u5 = _mm256_sub_epi32(y, x);
    u5 = _mm256_add_epi32(u5, rnding);
    u5 = _mm256_srai_epi32(u5, bit);

    // stage 5
    addsub_avx2(u0, u7, out + 0, out + 7, &clamp_lo, &clamp_hi);
    addsub_avx2(u1, u6, out + 1, out + 6, &clamp_lo, &clamp_hi);
    addsub_avx2(u2, u5, out + 2, out + 5, &clamp_lo, &clamp_hi);
    addsub_avx2(u3, u4, out + 3, out + 4, &clamp_lo, &clamp_hi);
    // stage 5
    if (!do_cols) {
        const int     log_range_out = AOMMAX(16, bd + 6);
        const __m256i clamp_lo_out  = _mm256_set1_epi32(-(1 << (log_range_out - 1)));
        const __m256i clamp_hi_out  = _mm256_set1_epi32((1 << (log_range_out - 1)) - 1);

        round_shift_4x4_avx2(out, out_shift);
        round_shift_4x4_avx2(out + 4, out_shift);
        highbd_clamp_epi32_avx2(out, out, &clamp_lo_out, &clamp_hi_out, 8);
    }
}

static void iadst8x8_low1_avx2(__m256i* in, __m256i* out, int32_t bit, int32_t do_cols, int32_t bd, int32_t out_shift) {
    const int32_t* cospi   = cospi_arr(bit);
    const __m256i  cospi4  = _mm256_set1_epi32(cospi[4]);
    const __m256i  cospi60 = _mm256_set1_epi32(cospi[60]);
    const __m256i  cospi16 = _mm256_set1_epi32(cospi[16]);
    const __m256i  cospi48 = _mm256_set1_epi32(cospi[48]);
    const __m256i  cospi32 = _mm256_set1_epi32(cospi[32]);
    const __m256i  rnding  = _mm256_set1_epi32(1 << (bit - 1));
    const __m256i  k_zero  = _mm256_setzero_si256();
    __m256i        u[8], x;

    // stage 0
    // stage 1
    // stage 2

    x    = _mm256_mullo_epi32(in[0], cospi60);
    u[0] = _mm256_add_epi32(x, rnding);
    u[0] = _mm256_srai_epi32(u[0], bit);

    x    = _mm256_mullo_epi32(in[0], cospi4);
    u[1] = _mm256_sub_epi32(k_zero, x);
    u[1] = _mm256_add_epi32(u[1], rnding);
    u[1] = _mm256_srai_epi32(u[1], bit);

    // stage 3
    // stage 4
    __m256i temp1, temp2;
    temp1 = _mm256_mullo_epi32(u[0], cospi16);
    x     = _mm256_mullo_epi32(u[1], cospi48);
    temp1 = _mm256_add_epi32(temp1, x);
    temp1 = _mm256_add_epi32(temp1, rnding);
    temp1 = _mm256_srai_epi32(temp1, bit);
    u[4]  = temp1;

    temp2 = _mm256_mullo_epi32(u[0], cospi48);
    x     = _mm256_mullo_epi32(u[1], cospi16);
    u[5]  = _mm256_sub_epi32(temp2, x);
    u[5]  = _mm256_add_epi32(u[5], rnding);
    u[5]  = _mm256_srai_epi32(u[5], bit);

    // stage 5
    // stage 6
    temp1 = _mm256_mullo_epi32(u[0], cospi32);
    x     = _mm256_mullo_epi32(u[1], cospi32);
    u[2]  = _mm256_add_epi32(temp1, x);
    u[2]  = _mm256_add_epi32(u[2], rnding);
    u[2]  = _mm256_srai_epi32(u[2], bit);

    u[3] = _mm256_sub_epi32(temp1, x);
    u[3] = _mm256_add_epi32(u[3], rnding);
    u[3] = _mm256_srai_epi32(u[3], bit);

    temp1 = _mm256_mullo_epi32(u[4], cospi32);
    x     = _mm256_mullo_epi32(u[5], cospi32);
    u[6]  = _mm256_add_epi32(temp1, x);
    u[6]  = _mm256_add_epi32(u[6], rnding);
    u[6]  = _mm256_srai_epi32(u[6], bit);

    u[7] = _mm256_sub_epi32(temp1, x);
    u[7] = _mm256_add_epi32(u[7], rnding);
    u[7] = _mm256_srai_epi32(u[7], bit);

    // stage 7
    if (do_cols) {
        out[0] = u[0];
        out[1] = _mm256_sub_epi32(k_zero, u[4]);
        out[2] = u[6];
        out[3] = _mm256_sub_epi32(k_zero, u[2]);
        out[4] = u[3];
        out[5] = _mm256_sub_epi32(k_zero, u[7]);
        out[6] = u[5];
        out[7] = _mm256_sub_epi32(k_zero, u[1]);
    } else {
        const int32_t log_range_out = AOMMAX(16, bd + 6);
        const __m256i clamp_lo_out  = _mm256_set1_epi32(-(1 << (log_range_out - 1)));
        const __m256i clamp_hi_out  = _mm256_set1_epi32((1 << (log_range_out - 1)) - 1);

        neg_shift_avx2(u[0], u[4], out + 0, out + 1, &clamp_lo_out, &clamp_hi_out, out_shift);
        neg_shift_avx2(u[6], u[2], out + 2, out + 3, &clamp_lo_out, &clamp_hi_out, out_shift);
        neg_shift_avx2(u[3], u[7], out + 4, out + 5, &clamp_lo_out, &clamp_hi_out, out_shift);
        neg_shift_avx2(u[5], u[1], out + 6, out + 7, &clamp_lo_out, &clamp_hi_out, out_shift);
    }
}

static void iadst8x8_avx2(__m256i* in, __m256i* out, int32_t bit, int32_t do_cols, int32_t bd, int32_t out_shift) {
    const int32_t* cospi     = cospi_arr(bit);
    const __m256i  cospi4    = _mm256_set1_epi32(cospi[4]);
    const __m256i  cospi60   = _mm256_set1_epi32(cospi[60]);
    const __m256i  cospi20   = _mm256_set1_epi32(cospi[20]);
    const __m256i  cospi44   = _mm256_set1_epi32(cospi[44]);
    const __m256i  cospi36   = _mm256_set1_epi32(cospi[36]);
    const __m256i  cospi28   = _mm256_set1_epi32(cospi[28]);
    const __m256i  cospi52   = _mm256_set1_epi32(cospi[52]);
    const __m256i  cospi12   = _mm256_set1_epi32(cospi[12]);
    const __m256i  cospi16   = _mm256_set1_epi32(cospi[16]);
    const __m256i  cospi48   = _mm256_set1_epi32(cospi[48]);
    const __m256i  cospim48  = _mm256_set1_epi32(-cospi[48]);
    const __m256i  cospi32   = _mm256_set1_epi32(cospi[32]);
    const __m256i  rnding    = _mm256_set1_epi32(1 << (bit - 1));
    const __m256i  k_zero    = _mm256_setzero_si256();
    const int32_t  log_range = AOMMAX(16, bd + (do_cols ? 6 : 8));
    const __m256i  clamp_lo  = _mm256_set1_epi32(-(1 << (log_range - 1)));
    const __m256i  clamp_hi  = _mm256_set1_epi32((1 << (log_range - 1)) - 1);
    __m256i        u[8], v[8], x;

    // stage 0
    // stage 1
    // stage 2

    u[0] = _mm256_mullo_epi32(in[7], cospi4);
    x    = _mm256_mullo_epi32(in[0], cospi60);
    u[0] = _mm256_add_epi32(u[0], x);
    u[0] = _mm256_add_epi32(u[0], rnding);
    u[0] = _mm256_srai_epi32(u[0], bit);

    u[1] = _mm256_mullo_epi32(in[7], cospi60);
    x    = _mm256_mullo_epi32(in[0], cospi4);
    u[1] = _mm256_sub_epi32(u[1], x);
    u[1] = _mm256_add_epi32(u[1], rnding);
    u[1] = _mm256_srai_epi32(u[1], bit);

    u[2] = _mm256_mullo_epi32(in[5], cospi20);
    x    = _mm256_mullo_epi32(in[2], cospi44);
    u[2] = _mm256_add_epi32(u[2], x);
    u[2] = _mm256_add_epi32(u[2], rnding);
    u[2] = _mm256_srai_epi32(u[2], bit);

    u[3] = _mm256_mullo_epi32(in[5], cospi44);
    x    = _mm256_mullo_epi32(in[2], cospi20);
    u[3] = _mm256_sub_epi32(u[3], x);
    u[3] = _mm256_add_epi32(u[3], rnding);
    u[3] = _mm256_srai_epi32(u[3], bit);

    u[4] = _mm256_mullo_epi32(in[3], cospi36);
    x    = _mm256_mullo_epi32(in[4], cospi28);
    u[4] = _mm256_add_epi32(u[4], x);
    u[4] = _mm256_add_epi32(u[4], rnding);
    u[4] = _mm256_srai_epi32(u[4], bit);

    u[5] = _mm256_mullo_epi32(in[3], cospi28);
    x    = _mm256_mullo_epi32(in[4], cospi36);
    u[5] = _mm256_sub_epi32(u[5], x);
    u[5] = _mm256_add_epi32(u[5], rnding);
    u[5] = _mm256_srai_epi32(u[5], bit);

    u[6] = _mm256_mullo_epi32(in[1], cospi52);
    x    = _mm256_mullo_epi32(in[6], cospi12);
    u[6] = _mm256_add_epi32(u[6], x);
    u[6] = _mm256_add_epi32(u[6], rnding);
    u[6] = _mm256_srai_epi32(u[6], bit);

    u[7] = _mm256_mullo_epi32(in[1], cospi12);
    x    = _mm256_mullo_epi32(in[6], cospi52);
    u[7] = _mm256_sub_epi32(u[7], x);
    u[7] = _mm256_add_epi32(u[7], rnding);
    u[7] = _mm256_srai_epi32(u[7], bit);

    // stage 3
    addsub_avx2(u[0], u[4], &v[0], &v[4], &clamp_lo, &clamp_hi);
    addsub_avx2(u[1], u[5], &v[1], &v[5], &clamp_lo, &clamp_hi);
    addsub_avx2(u[2], u[6], &v[2], &v[6], &clamp_lo, &clamp_hi);
    addsub_avx2(u[3], u[7], &v[3], &v[7], &clamp_lo, &clamp_hi);

    // stage 4
    u[0] = v[0];
    u[1] = v[1];
    u[2] = v[2];
    u[3] = v[3];

    u[4] = _mm256_mullo_epi32(v[4], cospi16);
    x    = _mm256_mullo_epi32(v[5], cospi48);
    u[4] = _mm256_add_epi32(u[4], x);
    u[4] = _mm256_add_epi32(u[4], rnding);
    u[4] = _mm256_srai_epi32(u[4], bit);

    u[5] = _mm256_mullo_epi32(v[4], cospi48);
    x    = _mm256_mullo_epi32(v[5], cospi16);
    u[5] = _mm256_sub_epi32(u[5], x);
    u[5] = _mm256_add_epi32(u[5], rnding);
    u[5] = _mm256_srai_epi32(u[5], bit);

    u[6] = _mm256_mullo_epi32(v[6], cospim48);
    x    = _mm256_mullo_epi32(v[7], cospi16);
    u[6] = _mm256_add_epi32(u[6], x);
    u[6] = _mm256_add_epi32(u[6], rnding);
    u[6] = _mm256_srai_epi32(u[6], bit);

    u[7] = _mm256_mullo_epi32(v[6], cospi16);
    x    = _mm256_mullo_epi32(v[7], cospim48);
    u[7] = _mm256_sub_epi32(u[7], x);
    u[7] = _mm256_add_epi32(u[7], rnding);
    u[7] = _mm256_srai_epi32(u[7], bit);

    // stage 5
    addsub_avx2(u[0], u[2], &v[0], &v[2], &clamp_lo, &clamp_hi);
    addsub_avx2(u[1], u[3], &v[1], &v[3], &clamp_lo, &clamp_hi);
    addsub_avx2(u[4], u[6], &v[4], &v[6], &clamp_lo, &clamp_hi);
    addsub_avx2(u[5], u[7], &v[5], &v[7], &clamp_lo, &clamp_hi);

    // stage 6
    u[0] = v[0];
    u[1] = v[1];
    u[4] = v[4];
    u[5] = v[5];

    v[0] = _mm256_mullo_epi32(v[2], cospi32);
    x    = _mm256_mullo_epi32(v[3], cospi32);
    u[2] = _mm256_add_epi32(v[0], x);
    u[2] = _mm256_add_epi32(u[2], rnding);
    u[2] = _mm256_srai_epi32(u[2], bit);

    u[3] = _mm256_sub_epi32(v[0], x);
    u[3] = _mm256_add_epi32(u[3], rnding);
    u[3] = _mm256_srai_epi32(u[3], bit);

    v[0] = _mm256_mullo_epi32(v[6], cospi32);
    x    = _mm256_mullo_epi32(v[7], cospi32);
    u[6] = _mm256_add_epi32(v[0], x);
    u[6] = _mm256_add_epi32(u[6], rnding);
    u[6] = _mm256_srai_epi32(u[6], bit);

    u[7] = _mm256_sub_epi32(v[0], x);
    u[7] = _mm256_add_epi32(u[7], rnding);
    u[7] = _mm256_srai_epi32(u[7], bit);

    // stage 7
    if (do_cols) {
        out[0] = u[0];
        out[1] = _mm256_sub_epi32(k_zero, u[4]);
        out[2] = u[6];
        out[3] = _mm256_sub_epi32(k_zero, u[2]);
        out[4] = u[3];
        out[5] = _mm256_sub_epi32(k_zero, u[7]);
        out[6] = u[5];
        out[7] = _mm256_sub_epi32(k_zero, u[1]);
    } else {
        const int32_t log_range_out = AOMMAX(16, bd + 6);
        const __m256i clamp_lo_out  = _mm256_set1_epi32(-(1 << (log_range_out - 1)));
        const __m256i clamp_hi_out  = _mm256_set1_epi32((1 << (log_range_out - 1)) - 1);

        neg_shift_avx2(u[0], u[4], out + 0, out + 1, &clamp_lo_out, &clamp_hi_out, out_shift);
        neg_shift_avx2(u[6], u[2], out + 2, out + 3, &clamp_lo_out, &clamp_hi_out, out_shift);
        neg_shift_avx2(u[3], u[7], out + 4, out + 5, &clamp_lo_out, &clamp_hi_out, out_shift);
        neg_shift_avx2(u[5], u[1], out + 6, out + 7, &clamp_lo_out, &clamp_hi_out, out_shift);
    }
}

static void shift_avx2(const __m256i* in, __m256i* out, const __m256i* clamp_lo, const __m256i* clamp_hi, int32_t shift,
                       int32_t size) {
    __m256i offset    = _mm256_set1_epi32((1 << shift) >> 1);
    __m128i shift_vec = _mm_cvtsi32_si128(shift);
    __m256i a0, a1;
    for (int32_t i = 0; i < size; i += 4) {
        a0         = _mm256_add_epi32(in[i], offset);
        a1         = _mm256_add_epi32(in[i + 1], offset);
        a0         = _mm256_sra_epi32(a0, shift_vec);
        a1         = _mm256_sra_epi32(a1, shift_vec);
        a0         = _mm256_max_epi32(a0, *clamp_lo);
        a1         = _mm256_max_epi32(a1, *clamp_lo);
        out[i]     = _mm256_min_epi32(a0, *clamp_hi);
        out[i + 1] = _mm256_min_epi32(a1, *clamp_hi);

        a0         = _mm256_add_epi32(in[i + 2], offset);
        a1         = _mm256_add_epi32(in[i + 3], offset);
        a0         = _mm256_sra_epi32(a0, shift_vec);
        a1         = _mm256_sra_epi32(a1, shift_vec);
        a0         = _mm256_max_epi32(a0, *clamp_lo);
        a1         = _mm256_max_epi32(a1, *clamp_lo);
        out[i + 2] = _mm256_min_epi32(a0, *clamp_hi);
        out[i + 3] = _mm256_min_epi32(a1, *clamp_hi);
    }
}

static void iidentity8_avx2(__m256i* in, __m256i* out, int32_t bit, int32_t do_cols, int32_t bd, int32_t out_shift) {
    (void)bit;
    const int32_t log_range = AOMMAX(16, bd + (do_cols ? 6 : 8));
    const __m256i clamp_lo  = _mm256_set1_epi32(-(1 << (log_range - 1)));
    const __m256i clamp_hi  = _mm256_set1_epi32((1 << (log_range - 1)) - 1);
    __m256i       v[8];
    v[0] = _mm256_add_epi32(in[0], in[0]);
    v[1] = _mm256_add_epi32(in[1], in[1]);
    v[2] = _mm256_add_epi32(in[2], in[2]);
    v[3] = _mm256_add_epi32(in[3], in[3]);
    v[4] = _mm256_add_epi32(in[4], in[4]);
    v[5] = _mm256_add_epi32(in[5], in[5]);
    v[6] = _mm256_add_epi32(in[6], in[6]);
    v[7] = _mm256_add_epi32(in[7], in[7]);

    if (!do_cols) {
        const int32_t log_range_out = AOMMAX(16, bd + 6);
        const __m256i clamp_lo_out  = _mm256_set1_epi32(
            AOMMAX(-(1 << (log_range_out - 1)), -(1 << (log_range - 1 - out_shift))));
        const __m256i clamp_hi_out = _mm256_set1_epi32(
            AOMMIN((1 << (log_range_out - 1)) - 1, (1 << (log_range - 1 - out_shift))));

        shift_avx2(v, out, &clamp_lo_out, &clamp_hi_out, out_shift, 8);
    } else {
        highbd_clamp_epi32_avx2(v, out, &clamp_lo, &clamp_hi, 8);
    }
}

static void idct16_low1_avx2(__m256i* in, __m256i* out, int32_t bit, int32_t do_cols, int32_t bd, int32_t out_shift) {
    const int32_t* cospi     = cospi_arr(bit);
    const __m256i  cospi32   = _mm256_set1_epi32(cospi[32]);
    const __m256i  rnding    = _mm256_set1_epi32(1 << (bit - 1));
    const int32_t  log_range = AOMMAX(16, bd + (do_cols ? 6 : 8));
    __m256i        clamp_lo  = _mm256_set1_epi32(-(1 << (log_range - 1)));
    __m256i        clamp_hi  = _mm256_set1_epi32((1 << (log_range - 1)) - 1);

    {
        // stage 0
        // stage 1
        // stage 2
        // stage 3
        // stage 4
        in[0] = _mm256_mullo_epi32(in[0], cospi32);
        in[0] = _mm256_add_epi32(in[0], rnding);
        in[0] = _mm256_srai_epi32(in[0], bit);

        // stage 5
        // stage 6
        // stage 7
        if (!do_cols) {
            const int log_range_out = AOMMAX(16, bd + 6);
            clamp_lo                = _mm256_set1_epi32(-(1 << (log_range_out - 1)));
            clamp_hi                = _mm256_set1_epi32((1 << (log_range_out - 1)) - 1);
            __m256i offset          = _mm256_set1_epi32((1 << out_shift) >> 1);
            in[0]                   = _mm256_add_epi32(in[0], offset);
            in[0]                   = _mm256_sra_epi32(in[0], _mm_cvtsi32_si128(out_shift));
        }
        in[0]   = _mm256_max_epi32(in[0], clamp_lo);
        in[0]   = _mm256_min_epi32(in[0], clamp_hi);
        out[0]  = in[0];
        out[1]  = in[0];
        out[2]  = in[0];
        out[3]  = in[0];
        out[4]  = in[0];
        out[5]  = in[0];
        out[6]  = in[0];
        out[7]  = in[0];
        out[8]  = in[0];
        out[9]  = in[0];
        out[10] = in[0];
        out[11] = in[0];
        out[12] = in[0];
        out[13] = in[0];
        out[14] = in[0];
        out[15] = in[0];
    }
}

static void idct16_low8_avx2(__m256i* in, __m256i* out, int32_t bit, int32_t do_cols, int32_t bd, int32_t out_shift) {
    const int32_t* cospi     = cospi_arr(bit);
    const __m256i  cospi60   = _mm256_set1_epi32(cospi[60]);
    const __m256i  cospi28   = _mm256_set1_epi32(cospi[28]);
    const __m256i  cospi44   = _mm256_set1_epi32(cospi[44]);
    const __m256i  cospi20   = _mm256_set1_epi32(cospi[20]);
    const __m256i  cospi12   = _mm256_set1_epi32(cospi[12]);
    const __m256i  cospi4    = _mm256_set1_epi32(cospi[4]);
    const __m256i  cospi56   = _mm256_set1_epi32(cospi[56]);
    const __m256i  cospi24   = _mm256_set1_epi32(cospi[24]);
    const __m256i  cospim40  = _mm256_set1_epi32(-cospi[40]);
    const __m256i  cospi8    = _mm256_set1_epi32(cospi[8]);
    const __m256i  cospi32   = _mm256_set1_epi32(cospi[32]);
    const __m256i  cospi48   = _mm256_set1_epi32(cospi[48]);
    const __m256i  cospi16   = _mm256_set1_epi32(cospi[16]);
    const __m256i  cospim16  = _mm256_set1_epi32(-cospi[16]);
    const __m256i  cospim48  = _mm256_set1_epi32(-cospi[48]);
    const __m256i  cospim36  = _mm256_set1_epi32(-cospi[36]);
    const __m256i  cospim52  = _mm256_set1_epi32(-cospi[52]);
    const __m256i  rnding    = _mm256_set1_epi32(1 << (bit - 1));
    const int32_t  log_range = AOMMAX(16, bd + (do_cols ? 6 : 8));
    const __m256i  clamp_lo  = _mm256_set1_epi32(-(1 << (log_range - 1)));
    const __m256i  clamp_hi  = _mm256_set1_epi32((1 << (log_range - 1)) - 1);
    __m256i        u[16], x, y;

    {
        // stage 0
        // stage 1
        u[0]  = in[0];
        u[2]  = in[4];
        u[4]  = in[2];
        u[6]  = in[6];
        u[8]  = in[1];
        u[10] = in[5];
        u[12] = in[3];
        u[14] = in[7];

        // stage 2
        u[15] = half_btf_0_avx2(&cospi4, &u[8], &rnding, bit);
        u[8]  = half_btf_0_avx2(&cospi60, &u[8], &rnding, bit);

        u[9]  = half_btf_0_avx2(&cospim36, &u[14], &rnding, bit);
        u[14] = half_btf_0_avx2(&cospi28, &u[14], &rnding, bit);

        u[13] = half_btf_0_avx2(&cospi20, &u[10], &rnding, bit);
        u[10] = half_btf_0_avx2(&cospi44, &u[10], &rnding, bit);

        u[11] = half_btf_0_avx2(&cospim52, &u[12], &rnding, bit);
        u[12] = half_btf_0_avx2(&cospi12, &u[12], &rnding, bit);

        // stage 3
        u[7] = half_btf_0_avx2(&cospi8, &u[4], &rnding, bit);
        u[4] = half_btf_0_avx2(&cospi56, &u[4], &rnding, bit);
        u[5] = half_btf_0_avx2(&cospim40, &u[6], &rnding, bit);
        u[6] = half_btf_0_avx2(&cospi24, &u[6], &rnding, bit);

        addsub_avx2(u[8], u[9], &u[8], &u[9], &clamp_lo, &clamp_hi);
        addsub_avx2(u[11], u[10], &u[11], &u[10], &clamp_lo, &clamp_hi);
        addsub_avx2(u[12], u[13], &u[12], &u[13], &clamp_lo, &clamp_hi);
        addsub_avx2(u[15], u[14], &u[15], &u[14], &clamp_lo, &clamp_hi);

        // stage 4
        x    = _mm256_mullo_epi32(u[0], cospi32);
        u[0] = _mm256_add_epi32(x, rnding);
        u[0] = _mm256_srai_epi32(u[0], bit);
        u[1] = u[0];

        u[3] = half_btf_0_avx2(&cospi16, &u[2], &rnding, bit);
        u[2] = half_btf_0_avx2(&cospi48, &u[2], &rnding, bit);

        addsub_avx2(u[4], u[5], &u[4], &u[5], &clamp_lo, &clamp_hi);
        addsub_avx2(u[7], u[6], &u[7], &u[6], &clamp_lo, &clamp_hi);

        x     = half_btf_avx2(&cospim16, &u[9], &cospi48, &u[14], &rnding, bit);
        u[14] = half_btf_avx2(&cospi48, &u[9], &cospi16, &u[14], &rnding, bit);
        u[9]  = x;
        y     = half_btf_avx2(&cospim48, &u[10], &cospim16, &u[13], &rnding, bit);
        u[13] = half_btf_avx2(&cospim16, &u[10], &cospi48, &u[13], &rnding, bit);
        u[10] = y;

        // stage 5
        addsub_avx2(u[0], u[3], &u[0], &u[3], &clamp_lo, &clamp_hi);
        addsub_avx2(u[1], u[2], &u[1], &u[2], &clamp_lo, &clamp_hi);

        x    = _mm256_mullo_epi32(u[5], cospi32);
        y    = _mm256_mullo_epi32(u[6], cospi32);
        u[5] = _mm256_sub_epi32(y, x);
        u[5] = _mm256_add_epi32(u[5], rnding);
        u[5] = _mm256_srai_epi32(u[5], bit);

        u[6] = _mm256_add_epi32(y, x);
        u[6] = _mm256_add_epi32(u[6], rnding);
        u[6] = _mm256_srai_epi32(u[6], bit);

        addsub_avx2(u[8], u[11], &u[8], &u[11], &clamp_lo, &clamp_hi);
        addsub_avx2(u[9], u[10], &u[9], &u[10], &clamp_lo, &clamp_hi);
        addsub_avx2(u[15], u[12], &u[15], &u[12], &clamp_lo, &clamp_hi);
        addsub_avx2(u[14], u[13], &u[14], &u[13], &clamp_lo, &clamp_hi);

        // stage 6
        addsub_avx2(u[0], u[7], &u[0], &u[7], &clamp_lo, &clamp_hi);
        addsub_avx2(u[1], u[6], &u[1], &u[6], &clamp_lo, &clamp_hi);
        addsub_avx2(u[2], u[5], &u[2], &u[5], &clamp_lo, &clamp_hi);
        addsub_avx2(u[3], u[4], &u[3], &u[4], &clamp_lo, &clamp_hi);

        x     = _mm256_mullo_epi32(u[10], cospi32);
        y     = _mm256_mullo_epi32(u[13], cospi32);
        u[10] = _mm256_sub_epi32(y, x);
        u[10] = _mm256_add_epi32(u[10], rnding);
        u[10] = _mm256_srai_epi32(u[10], bit);

        u[13] = _mm256_add_epi32(x, y);
        u[13] = _mm256_add_epi32(u[13], rnding);
        u[13] = _mm256_srai_epi32(u[13], bit);

        x     = _mm256_mullo_epi32(u[11], cospi32);
        y     = _mm256_mullo_epi32(u[12], cospi32);
        u[11] = _mm256_sub_epi32(y, x);
        u[11] = _mm256_add_epi32(u[11], rnding);
        u[11] = _mm256_srai_epi32(u[11], bit);

        u[12] = _mm256_add_epi32(x, y);
        u[12] = _mm256_add_epi32(u[12], rnding);
        u[12] = _mm256_srai_epi32(u[12], bit);
        // stage 7
        addsub_avx2(u[0], u[15], out + 0, out + 15, &clamp_lo, &clamp_hi);
        addsub_avx2(u[1], u[14], out + 1, out + 14, &clamp_lo, &clamp_hi);
        addsub_avx2(u[2], u[13], out + 2, out + 13, &clamp_lo, &clamp_hi);
        addsub_avx2(u[3], u[12], out + 3, out + 12, &clamp_lo, &clamp_hi);
        addsub_avx2(u[4], u[11], out + 4, out + 11, &clamp_lo, &clamp_hi);
        addsub_avx2(u[5], u[10], out + 5, out + 10, &clamp_lo, &clamp_hi);
        addsub_avx2(u[6], u[9], out + 6, out + 9, &clamp_lo, &clamp_hi);
        addsub_avx2(u[7], u[8], out + 7, out + 8, &clamp_lo, &clamp_hi);

        if (!do_cols) {
            const int     log_range_out = AOMMAX(16, bd + 6);
            const __m256i clamp_lo_out  = _mm256_set1_epi32(-(1 << (log_range_out - 1)));
            const __m256i clamp_hi_out  = _mm256_set1_epi32((1 << (log_range_out - 1)) - 1);
            round_shift_8x8_avx2(out, out_shift);
            highbd_clamp_epi32_avx2(out, out, &clamp_lo_out, &clamp_hi_out, 16);
        }
    }
}

static void idct16_avx2(__m256i* in, __m256i* out, int32_t bit, int32_t do_cols, int32_t bd, int32_t out_shift) {
    const int32_t* cospi     = cospi_arr(bit);
    const __m256i  cospi60   = _mm256_set1_epi32(cospi[60]);
    const __m256i  cospim4   = _mm256_set1_epi32(-cospi[4]);
    const __m256i  cospi28   = _mm256_set1_epi32(cospi[28]);
    const __m256i  cospim36  = _mm256_set1_epi32(-cospi[36]);
    const __m256i  cospi44   = _mm256_set1_epi32(cospi[44]);
    const __m256i  cospi20   = _mm256_set1_epi32(cospi[20]);
    const __m256i  cospim20  = _mm256_set1_epi32(-cospi[20]);
    const __m256i  cospi12   = _mm256_set1_epi32(cospi[12]);
    const __m256i  cospim52  = _mm256_set1_epi32(-cospi[52]);
    const __m256i  cospi52   = _mm256_set1_epi32(cospi[52]);
    const __m256i  cospi36   = _mm256_set1_epi32(cospi[36]);
    const __m256i  cospi4    = _mm256_set1_epi32(cospi[4]);
    const __m256i  cospi56   = _mm256_set1_epi32(cospi[56]);
    const __m256i  cospim8   = _mm256_set1_epi32(-cospi[8]);
    const __m256i  cospi24   = _mm256_set1_epi32(cospi[24]);
    const __m256i  cospim40  = _mm256_set1_epi32(-cospi[40]);
    const __m256i  cospi40   = _mm256_set1_epi32(cospi[40]);
    const __m256i  cospi8    = _mm256_set1_epi32(cospi[8]);
    const __m256i  cospi32   = _mm256_set1_epi32(cospi[32]);
    const __m256i  cospi48   = _mm256_set1_epi32(cospi[48]);
    const __m256i  cospi16   = _mm256_set1_epi32(cospi[16]);
    const __m256i  cospim16  = _mm256_set1_epi32(-cospi[16]);
    const __m256i  cospim48  = _mm256_set1_epi32(-cospi[48]);
    const __m256i  rnding    = _mm256_set1_epi32(1 << (bit - 1));
    const int32_t  log_range = AOMMAX(16, bd + (do_cols ? 6 : 8));
    const __m256i  clamp_lo  = _mm256_set1_epi32(-(1 << (log_range - 1)));
    const __m256i  clamp_hi  = _mm256_set1_epi32((1 << (log_range - 1)) - 1);
    __m256i        u[16], v[16], x, y;

    {
        // stage 0
        // stage 1
        u[0]  = in[0];
        u[1]  = in[8];
        u[2]  = in[4];
        u[3]  = in[12];
        u[4]  = in[2];
        u[5]  = in[10];
        u[6]  = in[6];
        u[7]  = in[14];
        u[8]  = in[1];
        u[9]  = in[9];
        u[10] = in[5];
        u[11] = in[13];
        u[12] = in[3];
        u[13] = in[11];
        u[14] = in[7];
        u[15] = in[15];

        // stage 2
        v[0] = u[0];
        v[1] = u[1];
        v[2] = u[2];
        v[3] = u[3];
        v[4] = u[4];
        v[5] = u[5];
        v[6] = u[6];
        v[7] = u[7];

        v[8]  = half_btf_avx2(&cospi60, &u[8], &cospim4, &u[15], &rnding, bit);
        v[9]  = half_btf_avx2(&cospi28, &u[9], &cospim36, &u[14], &rnding, bit);
        v[10] = half_btf_avx2(&cospi44, &u[10], &cospim20, &u[13], &rnding, bit);
        v[11] = half_btf_avx2(&cospi12, &u[11], &cospim52, &u[12], &rnding, bit);
        v[12] = half_btf_avx2(&cospi52, &u[11], &cospi12, &u[12], &rnding, bit);
        v[13] = half_btf_avx2(&cospi20, &u[10], &cospi44, &u[13], &rnding, bit);
        v[14] = half_btf_avx2(&cospi36, &u[9], &cospi28, &u[14], &rnding, bit);
        v[15] = half_btf_avx2(&cospi4, &u[8], &cospi60, &u[15], &rnding, bit);

        // stage 3
        u[0] = v[0];
        u[1] = v[1];
        u[2] = v[2];
        u[3] = v[3];
        u[4] = half_btf_avx2(&cospi56, &v[4], &cospim8, &v[7], &rnding, bit);
        u[5] = half_btf_avx2(&cospi24, &v[5], &cospim40, &v[6], &rnding, bit);
        u[6] = half_btf_avx2(&cospi40, &v[5], &cospi24, &v[6], &rnding, bit);
        u[7] = half_btf_avx2(&cospi8, &v[4], &cospi56, &v[7], &rnding, bit);
        addsub_avx2(v[8], v[9], &u[8], &u[9], &clamp_lo, &clamp_hi);
        addsub_avx2(v[11], v[10], &u[11], &u[10], &clamp_lo, &clamp_hi);
        addsub_avx2(v[12], v[13], &u[12], &u[13], &clamp_lo, &clamp_hi);
        addsub_avx2(v[15], v[14], &u[15], &u[14], &clamp_lo, &clamp_hi);

        // stage 4
        x    = _mm256_mullo_epi32(u[0], cospi32);
        y    = _mm256_mullo_epi32(u[1], cospi32);
        v[0] = _mm256_add_epi32(x, y);
        v[0] = _mm256_add_epi32(v[0], rnding);
        v[0] = _mm256_srai_epi32(v[0], bit);

        v[1] = _mm256_sub_epi32(x, y);
        v[1] = _mm256_add_epi32(v[1], rnding);
        v[1] = _mm256_srai_epi32(v[1], bit);

        v[2] = half_btf_avx2(&cospi48, &u[2], &cospim16, &u[3], &rnding, bit);
        v[3] = half_btf_avx2(&cospi16, &u[2], &cospi48, &u[3], &rnding, bit);
        addsub_avx2(u[4], u[5], &v[4], &v[5], &clamp_lo, &clamp_hi);
        addsub_avx2(u[7], u[6], &v[7], &v[6], &clamp_lo, &clamp_hi);
        v[8]  = u[8];
        v[9]  = half_btf_avx2(&cospim16, &u[9], &cospi48, &u[14], &rnding, bit);
        v[10] = half_btf_avx2(&cospim48, &u[10], &cospim16, &u[13], &rnding, bit);
        v[11] = u[11];
        v[12] = u[12];
        v[13] = half_btf_avx2(&cospim16, &u[10], &cospi48, &u[13], &rnding, bit);
        v[14] = half_btf_avx2(&cospi48, &u[9], &cospi16, &u[14], &rnding, bit);
        v[15] = u[15];

        // stage 5
        addsub_avx2(v[0], v[3], &u[0], &u[3], &clamp_lo, &clamp_hi);
        addsub_avx2(v[1], v[2], &u[1], &u[2], &clamp_lo, &clamp_hi);
        u[4] = v[4];

        x    = _mm256_mullo_epi32(v[5], cospi32);
        y    = _mm256_mullo_epi32(v[6], cospi32);
        u[5] = _mm256_sub_epi32(y, x);
        u[5] = _mm256_add_epi32(u[5], rnding);
        u[5] = _mm256_srai_epi32(u[5], bit);

        u[6] = _mm256_add_epi32(y, x);
        u[6] = _mm256_add_epi32(u[6], rnding);
        u[6] = _mm256_srai_epi32(u[6], bit);

        u[7] = v[7];
        addsub_avx2(v[8], v[11], &u[8], &u[11], &clamp_lo, &clamp_hi);
        addsub_avx2(v[9], v[10], &u[9], &u[10], &clamp_lo, &clamp_hi);
        addsub_avx2(v[15], v[12], &u[15], &u[12], &clamp_lo, &clamp_hi);
        addsub_avx2(v[14], v[13], &u[14], &u[13], &clamp_lo, &clamp_hi);

        // stage 6
        addsub_avx2(u[0], u[7], &v[0], &v[7], &clamp_lo, &clamp_hi);
        addsub_avx2(u[1], u[6], &v[1], &v[6], &clamp_lo, &clamp_hi);
        addsub_avx2(u[2], u[5], &v[2], &v[5], &clamp_lo, &clamp_hi);
        addsub_avx2(u[3], u[4], &v[3], &v[4], &clamp_lo, &clamp_hi);
        v[8] = u[8];
        v[9] = u[9];

        x     = _mm256_mullo_epi32(u[10], cospi32);
        y     = _mm256_mullo_epi32(u[13], cospi32);
        v[10] = _mm256_sub_epi32(y, x);
        v[10] = _mm256_add_epi32(v[10], rnding);
        v[10] = _mm256_srai_epi32(v[10], bit);

        v[13] = _mm256_add_epi32(x, y);
        v[13] = _mm256_add_epi32(v[13], rnding);
        v[13] = _mm256_srai_epi32(v[13], bit);

        x     = _mm256_mullo_epi32(u[11], cospi32);
        y     = _mm256_mullo_epi32(u[12], cospi32);
        v[11] = _mm256_sub_epi32(y, x);
        v[11] = _mm256_add_epi32(v[11], rnding);
        v[11] = _mm256_srai_epi32(v[11], bit);

        v[12] = _mm256_add_epi32(x, y);
        v[12] = _mm256_add_epi32(v[12], rnding);
        v[12] = _mm256_srai_epi32(v[12], bit);

        v[14] = u[14];
        v[15] = u[15];

        // stage 7
        addsub_avx2(v[0], v[15], out + 0, out + 15, &clamp_lo, &clamp_hi);
        addsub_avx2(v[1], v[14], out + 1, out + 14, &clamp_lo, &clamp_hi);
        addsub_avx2(v[2], v[13], out + 2, out + 13, &clamp_lo, &clamp_hi);
        addsub_avx2(v[3], v[12], out + 3, out + 12, &clamp_lo, &clamp_hi);
        addsub_avx2(v[4], v[11], out + 4, out + 11, &clamp_lo, &clamp_hi);
        addsub_avx2(v[5], v[10], out + 5, out + 10, &clamp_lo, &clamp_hi);
        addsub_avx2(v[6], v[9], out + 6, out + 9, &clamp_lo, &clamp_hi);
        addsub_avx2(v[7], v[8], out + 7, out + 8, &clamp_lo, &clamp_hi);

        if (!do_cols) {
            const int     log_range_out = AOMMAX(16, bd + 6);
            const __m256i clamp_lo_out  = _mm256_set1_epi32(-(1 << (log_range_out - 1)));
            const __m256i clamp_hi_out  = _mm256_set1_epi32((1 << (log_range_out - 1)) - 1);
            round_shift_8x8_avx2(out, out_shift);
            highbd_clamp_epi32_avx2(out, out, &clamp_lo_out, &clamp_hi_out, 16);
        }
    }
}

static void iadst16_low1_avx2(__m256i* in, __m256i* out, int32_t bit, int32_t do_cols, int32_t bd, int32_t out_shift) {
    const int32_t* cospi   = cospi_arr(bit);
    const __m256i  cospi2  = _mm256_set1_epi32(cospi[2]);
    const __m256i  cospi62 = _mm256_set1_epi32(cospi[62]);
    const __m256i  cospi8  = _mm256_set1_epi32(cospi[8]);
    const __m256i  cospi56 = _mm256_set1_epi32(cospi[56]);
    const __m256i  cospi48 = _mm256_set1_epi32(cospi[48]);
    const __m256i  cospi16 = _mm256_set1_epi32(cospi[16]);
    const __m256i  cospi32 = _mm256_set1_epi32(cospi[32]);
    const __m256i  rnding  = _mm256_set1_epi32(1 << (bit - 1));
    const __m256i  zero    = _mm256_setzero_si256();
    __m256i        v[16], x, y, temp1, temp2;

    // Calculate the column 0, 1, 2, 3
    {
        // stage 0
        // stage 1
        // stage 2
        x    = _mm256_mullo_epi32(in[0], cospi62);
        v[0] = _mm256_add_epi32(x, rnding);
        v[0] = _mm256_srai_epi32(v[0], bit);

        x    = _mm256_mullo_epi32(in[0], cospi2);
        v[1] = _mm256_sub_epi32(zero, x);
        v[1] = _mm256_add_epi32(v[1], rnding);
        v[1] = _mm256_srai_epi32(v[1], bit);

        // stage 3
        v[8] = v[0];
        v[9] = v[1];

        // stage 4
        temp1 = _mm256_mullo_epi32(v[8], cospi8);
        x     = _mm256_mullo_epi32(v[9], cospi56);
        temp1 = _mm256_add_epi32(temp1, x);
        temp1 = _mm256_add_epi32(temp1, rnding);
        temp1 = _mm256_srai_epi32(temp1, bit);

        temp2 = _mm256_mullo_epi32(v[8], cospi56);
        x     = _mm256_mullo_epi32(v[9], cospi8);
        temp2 = _mm256_sub_epi32(temp2, x);
        temp2 = _mm256_add_epi32(temp2, rnding);
        temp2 = _mm256_srai_epi32(temp2, bit);
        v[8]  = temp1;
        v[9]  = temp2;

        // stage 5
        v[4]  = v[0];
        v[5]  = v[1];
        v[12] = v[8];
        v[13] = v[9];

        // stage 6
        temp1 = _mm256_mullo_epi32(v[4], cospi16);
        x     = _mm256_mullo_epi32(v[5], cospi48);
        temp1 = _mm256_add_epi32(temp1, x);
        temp1 = _mm256_add_epi32(temp1, rnding);
        temp1 = _mm256_srai_epi32(temp1, bit);

        temp2 = _mm256_mullo_epi32(v[4], cospi48);
        x     = _mm256_mullo_epi32(v[5], cospi16);
        temp2 = _mm256_sub_epi32(temp2, x);
        temp2 = _mm256_add_epi32(temp2, rnding);
        temp2 = _mm256_srai_epi32(temp2, bit);
        v[4]  = temp1;
        v[5]  = temp2;

        temp1 = _mm256_mullo_epi32(v[12], cospi16);
        x     = _mm256_mullo_epi32(v[13], cospi48);
        temp1 = _mm256_add_epi32(temp1, x);
        temp1 = _mm256_add_epi32(temp1, rnding);
        temp1 = _mm256_srai_epi32(temp1, bit);

        temp2 = _mm256_mullo_epi32(v[12], cospi48);
        x     = _mm256_mullo_epi32(v[13], cospi16);
        temp2 = _mm256_sub_epi32(temp2, x);
        temp2 = _mm256_add_epi32(temp2, rnding);
        temp2 = _mm256_srai_epi32(temp2, bit);
        v[12] = temp1;
        v[13] = temp2;

        // stage 7
        v[2]  = v[0];
        v[3]  = v[1];
        v[6]  = v[4];
        v[7]  = v[5];
        v[10] = v[8];
        v[11] = v[9];
        v[14] = v[12];
        v[15] = v[13];

        // stage 8
        y    = _mm256_mullo_epi32(v[2], cospi32);
        x    = _mm256_mullo_epi32(v[3], cospi32);
        v[2] = _mm256_add_epi32(y, x);
        v[2] = _mm256_add_epi32(v[2], rnding);
        v[2] = _mm256_srai_epi32(v[2], bit);

        v[3] = _mm256_sub_epi32(y, x);
        v[3] = _mm256_add_epi32(v[3], rnding);
        v[3] = _mm256_srai_epi32(v[3], bit);

        y    = _mm256_mullo_epi32(v[6], cospi32);
        x    = _mm256_mullo_epi32(v[7], cospi32);
        v[6] = _mm256_add_epi32(y, x);
        v[6] = _mm256_add_epi32(v[6], rnding);
        v[6] = _mm256_srai_epi32(v[6], bit);

        v[7] = _mm256_sub_epi32(y, x);
        v[7] = _mm256_add_epi32(v[7], rnding);
        v[7] = _mm256_srai_epi32(v[7], bit);

        y     = _mm256_mullo_epi32(v[10], cospi32);
        x     = _mm256_mullo_epi32(v[11], cospi32);
        v[10] = _mm256_add_epi32(y, x);
        v[10] = _mm256_add_epi32(v[10], rnding);
        v[10] = _mm256_srai_epi32(v[10], bit);

        v[11] = _mm256_sub_epi32(y, x);
        v[11] = _mm256_add_epi32(v[11], rnding);
        v[11] = _mm256_srai_epi32(v[11], bit);

        y     = _mm256_mullo_epi32(v[14], cospi32);
        x     = _mm256_mullo_epi32(v[15], cospi32);
        v[14] = _mm256_add_epi32(y, x);
        v[14] = _mm256_add_epi32(v[14], rnding);
        v[14] = _mm256_srai_epi32(v[14], bit);

        v[15] = _mm256_sub_epi32(y, x);
        v[15] = _mm256_add_epi32(v[15], rnding);
        v[15] = _mm256_srai_epi32(v[15], bit);

        // stage 9
        if (do_cols) {
            out[0]  = v[0];
            out[1]  = _mm256_sub_epi32(_mm256_setzero_si256(), v[8]);
            out[2]  = v[12];
            out[3]  = _mm256_sub_epi32(_mm256_setzero_si256(), v[4]);
            out[4]  = v[6];
            out[5]  = _mm256_sub_epi32(_mm256_setzero_si256(), v[14]);
            out[6]  = v[10];
            out[7]  = _mm256_sub_epi32(_mm256_setzero_si256(), v[2]);
            out[8]  = v[3];
            out[9]  = _mm256_sub_epi32(_mm256_setzero_si256(), v[11]);
            out[10] = v[15];
            out[11] = _mm256_sub_epi32(_mm256_setzero_si256(), v[7]);
            out[12] = v[5];
            out[13] = _mm256_sub_epi32(_mm256_setzero_si256(), v[13]);
            out[14] = v[9];
            out[15] = _mm256_sub_epi32(_mm256_setzero_si256(), v[1]);
        } else {
            const int32_t log_range_out = AOMMAX(16, bd + 6);
            const __m256i clamp_lo_out  = _mm256_set1_epi32(-(1 << (log_range_out - 1)));
            const __m256i clamp_hi_out  = _mm256_set1_epi32((1 << (log_range_out - 1)) - 1);

            neg_shift_avx2(v[0], v[8], out + 0, out + 1, &clamp_lo_out, &clamp_hi_out, out_shift);
            neg_shift_avx2(v[12], v[4], out + 2, out + 3, &clamp_lo_out, &clamp_hi_out, out_shift);
            neg_shift_avx2(v[6], v[14], out + 4, out + 5, &clamp_lo_out, &clamp_hi_out, out_shift);
            neg_shift_avx2(v[10], v[2], out + 6, out + 7, &clamp_lo_out, &clamp_hi_out, out_shift);
            neg_shift_avx2(v[3], v[11], out + 8, out + 9, &clamp_lo_out, &clamp_hi_out, out_shift);
            neg_shift_avx2(v[15], v[7], out + 10, out + 11, &clamp_lo_out, &clamp_hi_out, out_shift);
            neg_shift_avx2(v[5], v[13], out + 12, out + 13, &clamp_lo_out, &clamp_hi_out, out_shift);
            neg_shift_avx2(v[9], v[1], out + 14, out + 15, &clamp_lo_out, &clamp_hi_out, out_shift);
        }
    }
}

static void iadst16_low8_avx2(__m256i* in, __m256i* out, int32_t bit, int32_t do_cols, int32_t bd, int32_t out_shift) {
    const int32_t* cospi     = cospi_arr(bit);
    const __m256i  cospi2    = _mm256_set1_epi32(cospi[2]);
    const __m256i  cospi62   = _mm256_set1_epi32(cospi[62]);
    const __m256i  cospi10   = _mm256_set1_epi32(cospi[10]);
    const __m256i  cospi54   = _mm256_set1_epi32(cospi[54]);
    const __m256i  cospi18   = _mm256_set1_epi32(cospi[18]);
    const __m256i  cospi46   = _mm256_set1_epi32(cospi[46]);
    const __m256i  cospi26   = _mm256_set1_epi32(cospi[26]);
    const __m256i  cospi38   = _mm256_set1_epi32(cospi[38]);
    const __m256i  cospi34   = _mm256_set1_epi32(cospi[34]);
    const __m256i  cospi30   = _mm256_set1_epi32(cospi[30]);
    const __m256i  cospi42   = _mm256_set1_epi32(cospi[42]);
    const __m256i  cospi22   = _mm256_set1_epi32(cospi[22]);
    const __m256i  cospi50   = _mm256_set1_epi32(cospi[50]);
    const __m256i  cospi14   = _mm256_set1_epi32(cospi[14]);
    const __m256i  cospi58   = _mm256_set1_epi32(cospi[58]);
    const __m256i  cospi6    = _mm256_set1_epi32(cospi[6]);
    const __m256i  cospi8    = _mm256_set1_epi32(cospi[8]);
    const __m256i  cospi56   = _mm256_set1_epi32(cospi[56]);
    const __m256i  cospi40   = _mm256_set1_epi32(cospi[40]);
    const __m256i  cospi24   = _mm256_set1_epi32(cospi[24]);
    const __m256i  cospim56  = _mm256_set1_epi32(-cospi[56]);
    const __m256i  cospim24  = _mm256_set1_epi32(-cospi[24]);
    const __m256i  cospi48   = _mm256_set1_epi32(cospi[48]);
    const __m256i  cospi16   = _mm256_set1_epi32(cospi[16]);
    const __m256i  cospim48  = _mm256_set1_epi32(-cospi[48]);
    const __m256i  cospi32   = _mm256_set1_epi32(cospi[32]);
    const __m256i  rnding    = _mm256_set1_epi32(1 << (bit - 1));
    const int32_t  log_range = AOMMAX(16, bd + (do_cols ? 6 : 8));
    const __m256i  clamp_lo  = _mm256_set1_epi32(-(1 << (log_range - 1)));
    const __m256i  clamp_hi  = _mm256_set1_epi32((1 << (log_range - 1)) - 1);
    __m256i        u[16], x, y;

    {
        // stage 0
        // stage 1
        // stage 2
        __m256i zero = _mm256_setzero_si256();
        x            = _mm256_mullo_epi32(in[0], cospi62);
        u[0]         = _mm256_add_epi32(x, rnding);
        u[0]         = _mm256_srai_epi32(u[0], bit);

        x    = _mm256_mullo_epi32(in[0], cospi2);
        u[1] = _mm256_sub_epi32(zero, x);
        u[1] = _mm256_add_epi32(u[1], rnding);
        u[1] = _mm256_srai_epi32(u[1], bit);

        x    = _mm256_mullo_epi32(in[2], cospi54);
        u[2] = _mm256_add_epi32(x, rnding);
        u[2] = _mm256_srai_epi32(u[2], bit);

        x    = _mm256_mullo_epi32(in[2], cospi10);
        u[3] = _mm256_sub_epi32(zero, x);
        u[3] = _mm256_add_epi32(u[3], rnding);
        u[3] = _mm256_srai_epi32(u[3], bit);

        x    = _mm256_mullo_epi32(in[4], cospi46);
        u[4] = _mm256_add_epi32(x, rnding);
        u[4] = _mm256_srai_epi32(u[4], bit);

        x    = _mm256_mullo_epi32(in[4], cospi18);
        u[5] = _mm256_sub_epi32(zero, x);
        u[5] = _mm256_add_epi32(u[5], rnding);
        u[5] = _mm256_srai_epi32(u[5], bit);

        x    = _mm256_mullo_epi32(in[6], cospi38);
        u[6] = _mm256_add_epi32(x, rnding);
        u[6] = _mm256_srai_epi32(u[6], bit);

        x    = _mm256_mullo_epi32(in[6], cospi26);
        u[7] = _mm256_sub_epi32(zero, x);
        u[7] = _mm256_add_epi32(u[7], rnding);
        u[7] = _mm256_srai_epi32(u[7], bit);

        u[8] = _mm256_mullo_epi32(in[7], cospi34);
        u[8] = _mm256_add_epi32(u[8], rnding);
        u[8] = _mm256_srai_epi32(u[8], bit);

        u[9] = _mm256_mullo_epi32(in[7], cospi30);
        u[9] = _mm256_add_epi32(u[9], rnding);
        u[9] = _mm256_srai_epi32(u[9], bit);

        u[10] = _mm256_mullo_epi32(in[5], cospi42);
        u[10] = _mm256_add_epi32(u[10], rnding);
        u[10] = _mm256_srai_epi32(u[10], bit);

        u[11] = _mm256_mullo_epi32(in[5], cospi22);
        u[11] = _mm256_add_epi32(u[11], rnding);
        u[11] = _mm256_srai_epi32(u[11], bit);

        u[12] = _mm256_mullo_epi32(in[3], cospi50);
        u[12] = _mm256_add_epi32(u[12], rnding);
        u[12] = _mm256_srai_epi32(u[12], bit);

        u[13] = _mm256_mullo_epi32(in[3], cospi14);
        u[13] = _mm256_add_epi32(u[13], rnding);
        u[13] = _mm256_srai_epi32(u[13], bit);

        u[14] = _mm256_mullo_epi32(in[1], cospi58);
        u[14] = _mm256_add_epi32(u[14], rnding);
        u[14] = _mm256_srai_epi32(u[14], bit);

        u[15] = _mm256_mullo_epi32(in[1], cospi6);
        u[15] = _mm256_add_epi32(u[15], rnding);
        u[15] = _mm256_srai_epi32(u[15], bit);

        // stage 3
        addsub_avx2(u[0], u[8], &u[0], &u[8], &clamp_lo, &clamp_hi);
        addsub_avx2(u[1], u[9], &u[1], &u[9], &clamp_lo, &clamp_hi);
        addsub_avx2(u[2], u[10], &u[2], &u[10], &clamp_lo, &clamp_hi);
        addsub_avx2(u[3], u[11], &u[3], &u[11], &clamp_lo, &clamp_hi);
        addsub_avx2(u[4], u[12], &u[4], &u[12], &clamp_lo, &clamp_hi);
        addsub_avx2(u[5], u[13], &u[5], &u[13], &clamp_lo, &clamp_hi);
        addsub_avx2(u[6], u[14], &u[6], &u[14], &clamp_lo, &clamp_hi);
        addsub_avx2(u[7], u[15], &u[7], &u[15], &clamp_lo, &clamp_hi);

        // stage 4
        y    = _mm256_mullo_epi32(u[8], cospi56);
        x    = _mm256_mullo_epi32(u[9], cospi56);
        u[8] = _mm256_mullo_epi32(u[8], cospi8);
        u[8] = _mm256_add_epi32(u[8], x);
        u[8] = _mm256_add_epi32(u[8], rnding);
        u[8] = _mm256_srai_epi32(u[8], bit);

        x    = _mm256_mullo_epi32(u[9], cospi8);
        u[9] = _mm256_sub_epi32(y, x);
        u[9] = _mm256_add_epi32(u[9], rnding);
        u[9] = _mm256_srai_epi32(u[9], bit);

        x     = _mm256_mullo_epi32(u[11], cospi24);
        y     = _mm256_mullo_epi32(u[10], cospi24);
        u[10] = _mm256_mullo_epi32(u[10], cospi40);
        u[10] = _mm256_add_epi32(u[10], x);
        u[10] = _mm256_add_epi32(u[10], rnding);
        u[10] = _mm256_srai_epi32(u[10], bit);

        x     = _mm256_mullo_epi32(u[11], cospi40);
        u[11] = _mm256_sub_epi32(y, x);
        u[11] = _mm256_add_epi32(u[11], rnding);
        u[11] = _mm256_srai_epi32(u[11], bit);

        x     = _mm256_mullo_epi32(u[13], cospi8);
        y     = _mm256_mullo_epi32(u[12], cospi8);
        u[12] = _mm256_mullo_epi32(u[12], cospim56);
        u[12] = _mm256_add_epi32(u[12], x);
        u[12] = _mm256_add_epi32(u[12], rnding);
        u[12] = _mm256_srai_epi32(u[12], bit);

        x     = _mm256_mullo_epi32(u[13], cospim56);
        u[13] = _mm256_sub_epi32(y, x);
        u[13] = _mm256_add_epi32(u[13], rnding);
        u[13] = _mm256_srai_epi32(u[13], bit);

        x     = _mm256_mullo_epi32(u[15], cospi40);
        y     = _mm256_mullo_epi32(u[14], cospi40);
        u[14] = _mm256_mullo_epi32(u[14], cospim24);
        u[14] = _mm256_add_epi32(u[14], x);
        u[14] = _mm256_add_epi32(u[14], rnding);
        u[14] = _mm256_srai_epi32(u[14], bit);

        x     = _mm256_mullo_epi32(u[15], cospim24);
        u[15] = _mm256_sub_epi32(y, x);
        u[15] = _mm256_add_epi32(u[15], rnding);
        u[15] = _mm256_srai_epi32(u[15], bit);

        // stage 5
        addsub_avx2(u[0], u[4], &u[0], &u[4], &clamp_lo, &clamp_hi);
        addsub_avx2(u[1], u[5], &u[1], &u[5], &clamp_lo, &clamp_hi);
        addsub_avx2(u[2], u[6], &u[2], &u[6], &clamp_lo, &clamp_hi);
        addsub_avx2(u[3], u[7], &u[3], &u[7], &clamp_lo, &clamp_hi);
        addsub_avx2(u[8], u[12], &u[8], &u[12], &clamp_lo, &clamp_hi);
        addsub_avx2(u[9], u[13], &u[9], &u[13], &clamp_lo, &clamp_hi);
        addsub_avx2(u[10], u[14], &u[10], &u[14], &clamp_lo, &clamp_hi);
        addsub_avx2(u[11], u[15], &u[11], &u[15], &clamp_lo, &clamp_hi);

        // stage 6
        x    = _mm256_mullo_epi32(u[5], cospi48);
        y    = _mm256_mullo_epi32(u[4], cospi48);
        u[4] = _mm256_mullo_epi32(u[4], cospi16);
        u[4] = _mm256_add_epi32(u[4], x);
        u[4] = _mm256_add_epi32(u[4], rnding);
        u[4] = _mm256_srai_epi32(u[4], bit);

        x    = _mm256_mullo_epi32(u[5], cospi16);
        u[5] = _mm256_sub_epi32(y, x);
        u[5] = _mm256_add_epi32(u[5], rnding);
        u[5] = _mm256_srai_epi32(u[5], bit);

        x    = _mm256_mullo_epi32(u[7], cospi16);
        y    = _mm256_mullo_epi32(u[6], cospi16);
        u[6] = _mm256_mullo_epi32(u[6], cospim48);
        u[6] = _mm256_add_epi32(u[6], x);
        u[6] = _mm256_add_epi32(u[6], rnding);
        u[6] = _mm256_srai_epi32(u[6], bit);

        x    = _mm256_mullo_epi32(u[7], cospim48);
        u[7] = _mm256_sub_epi32(y, x);
        u[7] = _mm256_add_epi32(u[7], rnding);
        u[7] = _mm256_srai_epi32(u[7], bit);

        x     = _mm256_mullo_epi32(u[13], cospi48);
        y     = _mm256_mullo_epi32(u[12], cospi48);
        u[12] = _mm256_mullo_epi32(u[12], cospi16);
        u[12] = _mm256_add_epi32(u[12], x);
        u[12] = _mm256_add_epi32(u[12], rnding);
        u[12] = _mm256_srai_epi32(u[12], bit);

        x     = _mm256_mullo_epi32(u[13], cospi16);
        u[13] = _mm256_sub_epi32(y, x);
        u[13] = _mm256_add_epi32(u[13], rnding);
        u[13] = _mm256_srai_epi32(u[13], bit);

        x     = _mm256_mullo_epi32(u[15], cospi16);
        y     = _mm256_mullo_epi32(u[14], cospi16);
        u[14] = _mm256_mullo_epi32(u[14], cospim48);
        u[14] = _mm256_add_epi32(u[14], x);
        u[14] = _mm256_add_epi32(u[14], rnding);
        u[14] = _mm256_srai_epi32(u[14], bit);

        x     = _mm256_mullo_epi32(u[15], cospim48);
        u[15] = _mm256_sub_epi32(y, x);
        u[15] = _mm256_add_epi32(u[15], rnding);
        u[15] = _mm256_srai_epi32(u[15], bit);

        // stage 7
        addsub_avx2(u[0], u[2], &u[0], &u[2], &clamp_lo, &clamp_hi);
        addsub_avx2(u[1], u[3], &u[1], &u[3], &clamp_lo, &clamp_hi);
        addsub_avx2(u[4], u[6], &u[4], &u[6], &clamp_lo, &clamp_hi);
        addsub_avx2(u[5], u[7], &u[5], &u[7], &clamp_lo, &clamp_hi);
        addsub_avx2(u[8], u[10], &u[8], &u[10], &clamp_lo, &clamp_hi);
        addsub_avx2(u[9], u[11], &u[9], &u[11], &clamp_lo, &clamp_hi);
        addsub_avx2(u[12], u[14], &u[12], &u[14], &clamp_lo, &clamp_hi);
        addsub_avx2(u[13], u[15], &u[13], &u[15], &clamp_lo, &clamp_hi);

        // stage 8
        y    = _mm256_mullo_epi32(u[2], cospi32);
        x    = _mm256_mullo_epi32(u[3], cospi32);
        u[2] = _mm256_add_epi32(y, x);
        u[2] = _mm256_add_epi32(u[2], rnding);
        u[2] = _mm256_srai_epi32(u[2], bit);

        u[3] = _mm256_sub_epi32(y, x);
        u[3] = _mm256_add_epi32(u[3], rnding);
        u[3] = _mm256_srai_epi32(u[3], bit);
        y    = _mm256_mullo_epi32(u[6], cospi32);
        x    = _mm256_mullo_epi32(u[7], cospi32);
        u[6] = _mm256_add_epi32(y, x);
        u[6] = _mm256_add_epi32(u[6], rnding);
        u[6] = _mm256_srai_epi32(u[6], bit);

        u[7] = _mm256_sub_epi32(y, x);
        u[7] = _mm256_add_epi32(u[7], rnding);
        u[7] = _mm256_srai_epi32(u[7], bit);

        y     = _mm256_mullo_epi32(u[10], cospi32);
        x     = _mm256_mullo_epi32(u[11], cospi32);
        u[10] = _mm256_add_epi32(y, x);
        u[10] = _mm256_add_epi32(u[10], rnding);
        u[10] = _mm256_srai_epi32(u[10], bit);

        u[11] = _mm256_sub_epi32(y, x);
        u[11] = _mm256_add_epi32(u[11], rnding);
        u[11] = _mm256_srai_epi32(u[11], bit);

        y     = _mm256_mullo_epi32(u[14], cospi32);
        x     = _mm256_mullo_epi32(u[15], cospi32);
        u[14] = _mm256_add_epi32(y, x);
        u[14] = _mm256_add_epi32(u[14], rnding);
        u[14] = _mm256_srai_epi32(u[14], bit);

        u[15] = _mm256_sub_epi32(y, x);
        u[15] = _mm256_add_epi32(u[15], rnding);
        u[15] = _mm256_srai_epi32(u[15], bit);

        // stage 9
        if (do_cols) {
            out[0]  = u[0];
            out[1]  = _mm256_sub_epi32(_mm256_setzero_si256(), u[8]);
            out[2]  = u[12];
            out[3]  = _mm256_sub_epi32(_mm256_setzero_si256(), u[4]);
            out[4]  = u[6];
            out[5]  = _mm256_sub_epi32(_mm256_setzero_si256(), u[14]);
            out[6]  = u[10];
            out[7]  = _mm256_sub_epi32(_mm256_setzero_si256(), u[2]);
            out[8]  = u[3];
            out[9]  = _mm256_sub_epi32(_mm256_setzero_si256(), u[11]);
            out[10] = u[15];
            out[11] = _mm256_sub_epi32(_mm256_setzero_si256(), u[7]);
            out[12] = u[5];
            out[13] = _mm256_sub_epi32(_mm256_setzero_si256(), u[13]);
            out[14] = u[9];
            out[15] = _mm256_sub_epi32(_mm256_setzero_si256(), u[1]);
        } else {
            const int32_t log_range_out = AOMMAX(16, bd + 6);
            const __m256i clamp_lo_out  = _mm256_set1_epi32(-(1 << (log_range_out - 1)));
            const __m256i clamp_hi_out  = _mm256_set1_epi32((1 << (log_range_out - 1)) - 1);

            neg_shift_avx2(u[0], u[8], out + 0, out + 1, &clamp_lo_out, &clamp_hi_out, out_shift);
            neg_shift_avx2(u[12], u[4], out + 2, out + 3, &clamp_lo_out, &clamp_hi_out, out_shift);
            neg_shift_avx2(u[6], u[14], out + 4, out + 5, &clamp_lo_out, &clamp_hi_out, out_shift);
            neg_shift_avx2(u[10], u[2], out + 6, out + 7, &clamp_lo_out, &clamp_hi_out, out_shift);
            neg_shift_avx2(u[3], u[11], out + 8, out + 9, &clamp_lo_out, &clamp_hi_out, out_shift);
            neg_shift_avx2(u[15], u[7], out + 10, out + 11, &clamp_lo_out, &clamp_hi_out, out_shift);
            neg_shift_avx2(u[5], u[13], out + 12, out + 13, &clamp_lo_out, &clamp_hi_out, out_shift);
            neg_shift_avx2(u[9], u[1], out + 14, out + 15, &clamp_lo_out, &clamp_hi_out, out_shift);
        }
    }
}

static void iadst16_avx2(__m256i* in, __m256i* out, int32_t bit, int32_t do_cols, int32_t bd, int32_t out_shift) {
    const int32_t* cospi     = cospi_arr(bit);
    const __m256i  cospi2    = _mm256_set1_epi32(cospi[2]);
    const __m256i  cospi62   = _mm256_set1_epi32(cospi[62]);
    const __m256i  cospi10   = _mm256_set1_epi32(cospi[10]);
    const __m256i  cospi54   = _mm256_set1_epi32(cospi[54]);
    const __m256i  cospi18   = _mm256_set1_epi32(cospi[18]);
    const __m256i  cospi46   = _mm256_set1_epi32(cospi[46]);
    const __m256i  cospi26   = _mm256_set1_epi32(cospi[26]);
    const __m256i  cospi38   = _mm256_set1_epi32(cospi[38]);
    const __m256i  cospi34   = _mm256_set1_epi32(cospi[34]);
    const __m256i  cospi30   = _mm256_set1_epi32(cospi[30]);
    const __m256i  cospi42   = _mm256_set1_epi32(cospi[42]);
    const __m256i  cospi22   = _mm256_set1_epi32(cospi[22]);
    const __m256i  cospi50   = _mm256_set1_epi32(cospi[50]);
    const __m256i  cospi14   = _mm256_set1_epi32(cospi[14]);
    const __m256i  cospi58   = _mm256_set1_epi32(cospi[58]);
    const __m256i  cospi6    = _mm256_set1_epi32(cospi[6]);
    const __m256i  cospi8    = _mm256_set1_epi32(cospi[8]);
    const __m256i  cospi56   = _mm256_set1_epi32(cospi[56]);
    const __m256i  cospi40   = _mm256_set1_epi32(cospi[40]);
    const __m256i  cospi24   = _mm256_set1_epi32(cospi[24]);
    const __m256i  cospim56  = _mm256_set1_epi32(-cospi[56]);
    const __m256i  cospim24  = _mm256_set1_epi32(-cospi[24]);
    const __m256i  cospi48   = _mm256_set1_epi32(cospi[48]);
    const __m256i  cospi16   = _mm256_set1_epi32(cospi[16]);
    const __m256i  cospim48  = _mm256_set1_epi32(-cospi[48]);
    const __m256i  cospi32   = _mm256_set1_epi32(cospi[32]);
    const __m256i  rnding    = _mm256_set1_epi32(1 << (bit - 1));
    const int32_t  log_range = AOMMAX(16, bd + (do_cols ? 6 : 8));
    const __m256i  clamp_lo  = _mm256_set1_epi32(-(1 << (log_range - 1)));
    const __m256i  clamp_hi  = _mm256_set1_epi32((1 << (log_range - 1)) - 1);
    __m256i        u[16], v[16], x, y;

    {
        // stage 0
        // stage 1
        // stage 2
        v[0] = _mm256_mullo_epi32(in[15], cospi2);
        x    = _mm256_mullo_epi32(in[0], cospi62);
        v[0] = _mm256_add_epi32(v[0], x);
        v[0] = _mm256_add_epi32(v[0], rnding);
        v[0] = _mm256_srai_epi32(v[0], bit);

        v[1] = _mm256_mullo_epi32(in[15], cospi62);
        x    = _mm256_mullo_epi32(in[0], cospi2);
        v[1] = _mm256_sub_epi32(v[1], x);
        v[1] = _mm256_add_epi32(v[1], rnding);
        v[1] = _mm256_srai_epi32(v[1], bit);

        v[2] = _mm256_mullo_epi32(in[13], cospi10);
        x    = _mm256_mullo_epi32(in[2], cospi54);
        v[2] = _mm256_add_epi32(v[2], x);
        v[2] = _mm256_add_epi32(v[2], rnding);
        v[2] = _mm256_srai_epi32(v[2], bit);

        v[3] = _mm256_mullo_epi32(in[13], cospi54);
        x    = _mm256_mullo_epi32(in[2], cospi10);
        v[3] = _mm256_sub_epi32(v[3], x);
        v[3] = _mm256_add_epi32(v[3], rnding);
        v[3] = _mm256_srai_epi32(v[3], bit);

        v[4] = _mm256_mullo_epi32(in[11], cospi18);
        x    = _mm256_mullo_epi32(in[4], cospi46);
        v[4] = _mm256_add_epi32(v[4], x);
        v[4] = _mm256_add_epi32(v[4], rnding);
        v[4] = _mm256_srai_epi32(v[4], bit);

        v[5] = _mm256_mullo_epi32(in[11], cospi46);
        x    = _mm256_mullo_epi32(in[4], cospi18);
        v[5] = _mm256_sub_epi32(v[5], x);
        v[5] = _mm256_add_epi32(v[5], rnding);
        v[5] = _mm256_srai_epi32(v[5], bit);

        v[6] = _mm256_mullo_epi32(in[9], cospi26);
        x    = _mm256_mullo_epi32(in[6], cospi38);
        v[6] = _mm256_add_epi32(v[6], x);
        v[6] = _mm256_add_epi32(v[6], rnding);
        v[6] = _mm256_srai_epi32(v[6], bit);

        v[7] = _mm256_mullo_epi32(in[9], cospi38);
        x    = _mm256_mullo_epi32(in[6], cospi26);
        v[7] = _mm256_sub_epi32(v[7], x);
        v[7] = _mm256_add_epi32(v[7], rnding);
        v[7] = _mm256_srai_epi32(v[7], bit);

        v[8] = _mm256_mullo_epi32(in[7], cospi34);
        x    = _mm256_mullo_epi32(in[8], cospi30);
        v[8] = _mm256_add_epi32(v[8], x);
        v[8] = _mm256_add_epi32(v[8], rnding);
        v[8] = _mm256_srai_epi32(v[8], bit);

        v[9] = _mm256_mullo_epi32(in[7], cospi30);
        x    = _mm256_mullo_epi32(in[8], cospi34);
        v[9] = _mm256_sub_epi32(v[9], x);
        v[9] = _mm256_add_epi32(v[9], rnding);
        v[9] = _mm256_srai_epi32(v[9], bit);

        v[10] = _mm256_mullo_epi32(in[5], cospi42);
        x     = _mm256_mullo_epi32(in[10], cospi22);
        v[10] = _mm256_add_epi32(v[10], x);
        v[10] = _mm256_add_epi32(v[10], rnding);
        v[10] = _mm256_srai_epi32(v[10], bit);

        v[11] = _mm256_mullo_epi32(in[5], cospi22);
        x     = _mm256_mullo_epi32(in[10], cospi42);
        v[11] = _mm256_sub_epi32(v[11], x);
        v[11] = _mm256_add_epi32(v[11], rnding);
        v[11] = _mm256_srai_epi32(v[11], bit);

        v[12] = _mm256_mullo_epi32(in[3], cospi50);
        x     = _mm256_mullo_epi32(in[12], cospi14);
        v[12] = _mm256_add_epi32(v[12], x);
        v[12] = _mm256_add_epi32(v[12], rnding);
        v[12] = _mm256_srai_epi32(v[12], bit);

        v[13] = _mm256_mullo_epi32(in[3], cospi14);
        x     = _mm256_mullo_epi32(in[12], cospi50);
        v[13] = _mm256_sub_epi32(v[13], x);
        v[13] = _mm256_add_epi32(v[13], rnding);
        v[13] = _mm256_srai_epi32(v[13], bit);

        v[14] = _mm256_mullo_epi32(in[1], cospi58);
        x     = _mm256_mullo_epi32(in[14], cospi6);
        v[14] = _mm256_add_epi32(v[14], x);
        v[14] = _mm256_add_epi32(v[14], rnding);
        v[14] = _mm256_srai_epi32(v[14], bit);

        v[15] = _mm256_mullo_epi32(in[1], cospi6);
        x     = _mm256_mullo_epi32(in[14], cospi58);
        v[15] = _mm256_sub_epi32(v[15], x);
        v[15] = _mm256_add_epi32(v[15], rnding);
        v[15] = _mm256_srai_epi32(v[15], bit);

        // stage 3
        addsub_avx2(v[0], v[8], &u[0], &u[8], &clamp_lo, &clamp_hi);
        addsub_avx2(v[1], v[9], &u[1], &u[9], &clamp_lo, &clamp_hi);
        addsub_avx2(v[2], v[10], &u[2], &u[10], &clamp_lo, &clamp_hi);
        addsub_avx2(v[3], v[11], &u[3], &u[11], &clamp_lo, &clamp_hi);
        addsub_avx2(v[4], v[12], &u[4], &u[12], &clamp_lo, &clamp_hi);
        addsub_avx2(v[5], v[13], &u[5], &u[13], &clamp_lo, &clamp_hi);
        addsub_avx2(v[6], v[14], &u[6], &u[14], &clamp_lo, &clamp_hi);
        addsub_avx2(v[7], v[15], &u[7], &u[15], &clamp_lo, &clamp_hi);

        // stage 4
        v[0] = u[0];
        v[1] = u[1];
        v[2] = u[2];
        v[3] = u[3];
        v[4] = u[4];
        v[5] = u[5];
        v[6] = u[6];
        v[7] = u[7];

        v[8] = _mm256_mullo_epi32(u[8], cospi8);
        x    = _mm256_mullo_epi32(u[9], cospi56);
        v[8] = _mm256_add_epi32(v[8], x);
        v[8] = _mm256_add_epi32(v[8], rnding);
        v[8] = _mm256_srai_epi32(v[8], bit);

        v[9] = _mm256_mullo_epi32(u[8], cospi56);
        x    = _mm256_mullo_epi32(u[9], cospi8);
        v[9] = _mm256_sub_epi32(v[9], x);
        v[9] = _mm256_add_epi32(v[9], rnding);
        v[9] = _mm256_srai_epi32(v[9], bit);

        v[10] = _mm256_mullo_epi32(u[10], cospi40);
        x     = _mm256_mullo_epi32(u[11], cospi24);
        v[10] = _mm256_add_epi32(v[10], x);
        v[10] = _mm256_add_epi32(v[10], rnding);
        v[10] = _mm256_srai_epi32(v[10], bit);

        v[11] = _mm256_mullo_epi32(u[10], cospi24);
        x     = _mm256_mullo_epi32(u[11], cospi40);
        v[11] = _mm256_sub_epi32(v[11], x);
        v[11] = _mm256_add_epi32(v[11], rnding);
        v[11] = _mm256_srai_epi32(v[11], bit);

        v[12] = _mm256_mullo_epi32(u[12], cospim56);
        x     = _mm256_mullo_epi32(u[13], cospi8);
        v[12] = _mm256_add_epi32(v[12], x);
        v[12] = _mm256_add_epi32(v[12], rnding);
        v[12] = _mm256_srai_epi32(v[12], bit);

        v[13] = _mm256_mullo_epi32(u[12], cospi8);
        x     = _mm256_mullo_epi32(u[13], cospim56);
        v[13] = _mm256_sub_epi32(v[13], x);
        v[13] = _mm256_add_epi32(v[13], rnding);
        v[13] = _mm256_srai_epi32(v[13], bit);

        v[14] = _mm256_mullo_epi32(u[14], cospim24);
        x     = _mm256_mullo_epi32(u[15], cospi40);
        v[14] = _mm256_add_epi32(v[14], x);
        v[14] = _mm256_add_epi32(v[14], rnding);
        v[14] = _mm256_srai_epi32(v[14], bit);

        v[15] = _mm256_mullo_epi32(u[14], cospi40);
        x     = _mm256_mullo_epi32(u[15], cospim24);
        v[15] = _mm256_sub_epi32(v[15], x);
        v[15] = _mm256_add_epi32(v[15], rnding);
        v[15] = _mm256_srai_epi32(v[15], bit);

        // stage 5
        addsub_avx2(v[0], v[4], &u[0], &u[4], &clamp_lo, &clamp_hi);
        addsub_avx2(v[1], v[5], &u[1], &u[5], &clamp_lo, &clamp_hi);
        addsub_avx2(v[2], v[6], &u[2], &u[6], &clamp_lo, &clamp_hi);
        addsub_avx2(v[3], v[7], &u[3], &u[7], &clamp_lo, &clamp_hi);
        addsub_avx2(v[8], v[12], &u[8], &u[12], &clamp_lo, &clamp_hi);
        addsub_avx2(v[9], v[13], &u[9], &u[13], &clamp_lo, &clamp_hi);
        addsub_avx2(v[10], v[14], &u[10], &u[14], &clamp_lo, &clamp_hi);
        addsub_avx2(v[11], v[15], &u[11], &u[15], &clamp_lo, &clamp_hi);

        // stage 6
        v[0] = u[0];
        v[1] = u[1];
        v[2] = u[2];
        v[3] = u[3];

        v[4] = _mm256_mullo_epi32(u[4], cospi16);
        x    = _mm256_mullo_epi32(u[5], cospi48);
        v[4] = _mm256_add_epi32(v[4], x);
        v[4] = _mm256_add_epi32(v[4], rnding);
        v[4] = _mm256_srai_epi32(v[4], bit);

        v[5] = _mm256_mullo_epi32(u[4], cospi48);
        x    = _mm256_mullo_epi32(u[5], cospi16);
        v[5] = _mm256_sub_epi32(v[5], x);
        v[5] = _mm256_add_epi32(v[5], rnding);
        v[5] = _mm256_srai_epi32(v[5], bit);

        v[6] = _mm256_mullo_epi32(u[6], cospim48);
        x    = _mm256_mullo_epi32(u[7], cospi16);
        v[6] = _mm256_add_epi32(v[6], x);
        v[6] = _mm256_add_epi32(v[6], rnding);
        v[6] = _mm256_srai_epi32(v[6], bit);

        v[7] = _mm256_mullo_epi32(u[6], cospi16);
        x    = _mm256_mullo_epi32(u[7], cospim48);
        v[7] = _mm256_sub_epi32(v[7], x);
        v[7] = _mm256_add_epi32(v[7], rnding);
        v[7] = _mm256_srai_epi32(v[7], bit);

        v[8]  = u[8];
        v[9]  = u[9];
        v[10] = u[10];
        v[11] = u[11];

        v[12] = _mm256_mullo_epi32(u[12], cospi16);
        x     = _mm256_mullo_epi32(u[13], cospi48);
        v[12] = _mm256_add_epi32(v[12], x);
        v[12] = _mm256_add_epi32(v[12], rnding);
        v[12] = _mm256_srai_epi32(v[12], bit);

        v[13] = _mm256_mullo_epi32(u[12], cospi48);
        x     = _mm256_mullo_epi32(u[13], cospi16);
        v[13] = _mm256_sub_epi32(v[13], x);
        v[13] = _mm256_add_epi32(v[13], rnding);
        v[13] = _mm256_srai_epi32(v[13], bit);

        v[14] = _mm256_mullo_epi32(u[14], cospim48);
        x     = _mm256_mullo_epi32(u[15], cospi16);
        v[14] = _mm256_add_epi32(v[14], x);
        v[14] = _mm256_add_epi32(v[14], rnding);
        v[14] = _mm256_srai_epi32(v[14], bit);

        v[15] = _mm256_mullo_epi32(u[14], cospi16);
        x     = _mm256_mullo_epi32(u[15], cospim48);
        v[15] = _mm256_sub_epi32(v[15], x);
        v[15] = _mm256_add_epi32(v[15], rnding);
        v[15] = _mm256_srai_epi32(v[15], bit);

        // stage 7
        addsub_avx2(v[0], v[2], &u[0], &u[2], &clamp_lo, &clamp_hi);
        addsub_avx2(v[1], v[3], &u[1], &u[3], &clamp_lo, &clamp_hi);
        addsub_avx2(v[4], v[6], &u[4], &u[6], &clamp_lo, &clamp_hi);
        addsub_avx2(v[5], v[7], &u[5], &u[7], &clamp_lo, &clamp_hi);
        addsub_avx2(v[8], v[10], &u[8], &u[10], &clamp_lo, &clamp_hi);
        addsub_avx2(v[9], v[11], &u[9], &u[11], &clamp_lo, &clamp_hi);
        addsub_avx2(v[12], v[14], &u[12], &u[14], &clamp_lo, &clamp_hi);
        addsub_avx2(v[13], v[15], &u[13], &u[15], &clamp_lo, &clamp_hi);

        // stage 8
        v[0] = u[0];
        v[1] = u[1];

        y    = _mm256_mullo_epi32(u[2], cospi32);
        x    = _mm256_mullo_epi32(u[3], cospi32);
        v[2] = _mm256_add_epi32(y, x);
        v[2] = _mm256_add_epi32(v[2], rnding);
        v[2] = _mm256_srai_epi32(v[2], bit);

        v[3] = _mm256_sub_epi32(y, x);
        v[3] = _mm256_add_epi32(v[3], rnding);
        v[3] = _mm256_srai_epi32(v[3], bit);

        v[4] = u[4];
        v[5] = u[5];

        y    = _mm256_mullo_epi32(u[6], cospi32);
        x    = _mm256_mullo_epi32(u[7], cospi32);
        v[6] = _mm256_add_epi32(y, x);
        v[6] = _mm256_add_epi32(v[6], rnding);
        v[6] = _mm256_srai_epi32(v[6], bit);

        v[7] = _mm256_sub_epi32(y, x);
        v[7] = _mm256_add_epi32(v[7], rnding);
        v[7] = _mm256_srai_epi32(v[7], bit);

        v[8] = u[8];
        v[9] = u[9];

        y     = _mm256_mullo_epi32(u[10], cospi32);
        x     = _mm256_mullo_epi32(u[11], cospi32);
        v[10] = _mm256_add_epi32(y, x);
        v[10] = _mm256_add_epi32(v[10], rnding);
        v[10] = _mm256_srai_epi32(v[10], bit);

        v[11] = _mm256_sub_epi32(y, x);
        v[11] = _mm256_add_epi32(v[11], rnding);
        v[11] = _mm256_srai_epi32(v[11], bit);

        v[12] = u[12];
        v[13] = u[13];

        y     = _mm256_mullo_epi32(u[14], cospi32);
        x     = _mm256_mullo_epi32(u[15], cospi32);
        v[14] = _mm256_add_epi32(y, x);
        v[14] = _mm256_add_epi32(v[14], rnding);
        v[14] = _mm256_srai_epi32(v[14], bit);

        v[15] = _mm256_sub_epi32(y, x);
        v[15] = _mm256_add_epi32(v[15], rnding);
        v[15] = _mm256_srai_epi32(v[15], bit);

        // stage 9
        if (do_cols) {
            out[0]  = v[0];
            out[1]  = _mm256_sub_epi32(_mm256_setzero_si256(), v[8]);
            out[2]  = v[12];
            out[3]  = _mm256_sub_epi32(_mm256_setzero_si256(), v[4]);
            out[4]  = v[6];
            out[5]  = _mm256_sub_epi32(_mm256_setzero_si256(), v[14]);
            out[6]  = v[10];
            out[7]  = _mm256_sub_epi32(_mm256_setzero_si256(), v[2]);
            out[8]  = v[3];
            out[9]  = _mm256_sub_epi32(_mm256_setzero_si256(), v[11]);
            out[10] = v[15];
            out[11] = _mm256_sub_epi32(_mm256_setzero_si256(), v[7]);
            out[12] = v[5];
            out[13] = _mm256_sub_epi32(_mm256_setzero_si256(), v[13]);
            out[14] = v[9];
            out[15] = _mm256_sub_epi32(_mm256_setzero_si256(), v[1]);
        } else {
            const int32_t log_range_out = AOMMAX(16, bd + 6);
            const __m256i clamp_lo_out  = _mm256_set1_epi32(-(1 << (log_range_out - 1)));
            const __m256i clamp_hi_out  = _mm256_set1_epi32((1 << (log_range_out - 1)) - 1);

            neg_shift_avx2(v[0], v[8], out + 0, out + 1, &clamp_lo_out, &clamp_hi_out, out_shift);
            neg_shift_avx2(v[12], v[4], out + 2, out + 3, &clamp_lo_out, &clamp_hi_out, out_shift);
            neg_shift_avx2(v[6], v[14], out + 4, out + 5, &clamp_lo_out, &clamp_hi_out, out_shift);
            neg_shift_avx2(v[10], v[2], out + 6, out + 7, &clamp_lo_out, &clamp_hi_out, out_shift);
            neg_shift_avx2(v[3], v[11], out + 8, out + 9, &clamp_lo_out, &clamp_hi_out, out_shift);
            neg_shift_avx2(v[15], v[7], out + 10, out + 11, &clamp_lo_out, &clamp_hi_out, out_shift);
            neg_shift_avx2(v[5], v[13], out + 12, out + 13, &clamp_lo_out, &clamp_hi_out, out_shift);
            neg_shift_avx2(v[9], v[1], out + 14, out + 15, &clamp_lo_out, &clamp_hi_out, out_shift);
        }
    }
}

static void iidentity16_avx2(__m256i* in, __m256i* out, int32_t bit, int32_t do_cols, int32_t bd, int32_t out_shift) {
    (void)bit;
    const int32_t log_range = AOMMAX(16, bd + (do_cols ? 6 : 8));
    const __m256i clamp_lo  = _mm256_set1_epi32(-(1 << (log_range - 1)));
    const __m256i clamp_hi  = _mm256_set1_epi32((1 << (log_range - 1)) - 1);
    __m256i       v[16];
    __m256i       fact   = _mm256_set1_epi32(2 * new_sqrt2);
    __m256i       offset = _mm256_set1_epi32(1 << (new_sqrt2_bits - 1));
    __m256i       a0, a1, a2, a3;

    for (int32_t i = 0; i < 16; i += 8) {
        a0       = _mm256_mullo_epi32(in[i], fact);
        a1       = _mm256_mullo_epi32(in[i + 1], fact);
        a0       = _mm256_add_epi32(a0, offset);
        a1       = _mm256_add_epi32(a1, offset);
        v[i]     = _mm256_srai_epi32(a0, new_sqrt2_bits);
        v[i + 1] = _mm256_srai_epi32(a1, new_sqrt2_bits);

        a2       = _mm256_mullo_epi32(in[i + 2], fact);
        a3       = _mm256_mullo_epi32(in[i + 3], fact);
        a2       = _mm256_add_epi32(a2, offset);
        a3       = _mm256_add_epi32(a3, offset);
        v[i + 2] = _mm256_srai_epi32(a2, new_sqrt2_bits);
        v[i + 3] = _mm256_srai_epi32(a3, new_sqrt2_bits);

        a0       = _mm256_mullo_epi32(in[i + 4], fact);
        a1       = _mm256_mullo_epi32(in[i + 5], fact);
        a0       = _mm256_add_epi32(a0, offset);
        a1       = _mm256_add_epi32(a1, offset);
        v[i + 4] = _mm256_srai_epi32(a0, new_sqrt2_bits);
        v[i + 5] = _mm256_srai_epi32(a1, new_sqrt2_bits);

        a2       = _mm256_mullo_epi32(in[i + 6], fact);
        a3       = _mm256_mullo_epi32(in[i + 7], fact);
        a2       = _mm256_add_epi32(a2, offset);
        a3       = _mm256_add_epi32(a3, offset);
        v[i + 6] = _mm256_srai_epi32(a2, new_sqrt2_bits);
        v[i + 7] = _mm256_srai_epi32(a3, new_sqrt2_bits);
    }

    if (!do_cols) {
        const int32_t log_range_out = AOMMAX(16, bd + 6);
        const __m256i clamp_lo_out  = _mm256_set1_epi32(
            AOMMAX(-(1 << (log_range_out - 1)), -(1 << (log_range - 1 - out_shift))));
        const __m256i clamp_hi_out = _mm256_set1_epi32(
            AOMMIN((1 << (log_range_out - 1)) - 1, (1 << (log_range - 1 - out_shift))));

        shift_avx2(v, out, &clamp_lo_out, &clamp_hi_out, out_shift, 16);
    } else {
        highbd_clamp_epi32_avx2(v, out, &clamp_lo, &clamp_hi, 16);
    }
}

static void idct32_low1_avx2(__m256i* in, __m256i* out, int32_t bit, int32_t do_cols, int32_t bd, int32_t out_shift) {
    const int32_t* cospi     = cospi_arr(bit);
    const __m256i  cospi32   = _mm256_set1_epi32(cospi[32]);
    const __m256i  rounding  = _mm256_set1_epi32(1 << (bit - 1));
    const int32_t  log_range = AOMMAX(16, bd + (do_cols ? 6 : 8));
    __m256i        clamp_lo  = _mm256_set1_epi32(-(1 << (log_range - 1)));
    __m256i        clamp_hi  = _mm256_set1_epi32((1 << (log_range - 1)) - 1);
    __m256i        x;
    // stage 0
    // stage 1
    // stage 2
    // stage 3
    // stage 4
    // stage 5
    x = _mm256_mullo_epi32(in[0], cospi32);
    x = _mm256_add_epi32(x, rounding);
    x = _mm256_srai_epi32(x, bit);

    // stage 6
    // stage 7
    // stage 8
    // stage 9
    if (!do_cols) {
        const int log_range_out = AOMMAX(16, bd + 6);
        __m256i   offset        = _mm256_set1_epi32((1 << out_shift) >> 1);
        clamp_lo                = _mm256_set1_epi32(-(1 << (log_range_out - 1)));
        clamp_hi                = _mm256_set1_epi32((1 << (log_range_out - 1)) - 1);
        x                       = _mm256_add_epi32(offset, x);
        x                       = _mm256_sra_epi32(x, _mm_cvtsi32_si128(out_shift));
    }
    x = _mm256_max_epi32(x, clamp_lo);
    x = _mm256_min_epi32(x, clamp_hi);

    out[0]  = x;
    out[1]  = x;
    out[2]  = x;
    out[3]  = x;
    out[4]  = x;
    out[5]  = x;
    out[6]  = x;
    out[7]  = x;
    out[8]  = x;
    out[9]  = x;
    out[10] = x;
    out[11] = x;
    out[12] = x;
    out[13] = x;
    out[14] = x;
    out[15] = x;
    out[16] = x;
    out[17] = x;
    out[18] = x;
    out[19] = x;
    out[20] = x;
    out[21] = x;
    out[22] = x;
    out[23] = x;
    out[24] = x;
    out[25] = x;
    out[26] = x;
    out[27] = x;
    out[28] = x;
    out[29] = x;
    out[30] = x;
    out[31] = x;
}

static void idct32_low8_avx2(__m256i* in, __m256i* out, int32_t bit, int32_t do_cols, int32_t bd, int32_t out_shift) {
    const int32_t* cospi     = cospi_arr(bit);
    const __m256i  cospi62   = _mm256_set1_epi32(cospi[62]);
    const __m256i  cospi14   = _mm256_set1_epi32(cospi[14]);
    const __m256i  cospi54   = _mm256_set1_epi32(cospi[54]);
    const __m256i  cospi6    = _mm256_set1_epi32(cospi[6]);
    const __m256i  cospi10   = _mm256_set1_epi32(cospi[10]);
    const __m256i  cospi2    = _mm256_set1_epi32(cospi[2]);
    const __m256i  cospim58  = _mm256_set1_epi32(-cospi[58]);
    const __m256i  cospim50  = _mm256_set1_epi32(-cospi[50]);
    const __m256i  cospi60   = _mm256_set1_epi32(cospi[60]);
    const __m256i  cospi12   = _mm256_set1_epi32(cospi[12]);
    const __m256i  cospi4    = _mm256_set1_epi32(cospi[4]);
    const __m256i  cospim52  = _mm256_set1_epi32(-cospi[52]);
    const __m256i  cospi56   = _mm256_set1_epi32(cospi[56]);
    const __m256i  cospi24   = _mm256_set1_epi32(cospi[24]);
    const __m256i  cospi40   = _mm256_set1_epi32(cospi[40]);
    const __m256i  cospi8    = _mm256_set1_epi32(cospi[8]);
    const __m256i  cospim40  = _mm256_set1_epi32(-cospi[40]);
    const __m256i  cospim8   = _mm256_set1_epi32(-cospi[8]);
    const __m256i  cospim56  = _mm256_set1_epi32(-cospi[56]);
    const __m256i  cospim24  = _mm256_set1_epi32(-cospi[24]);
    const __m256i  cospi32   = _mm256_set1_epi32(cospi[32]);
    const __m256i  cospim32  = _mm256_set1_epi32(-cospi[32]);
    const __m256i  cospi48   = _mm256_set1_epi32(cospi[48]);
    const __m256i  cospim48  = _mm256_set1_epi32(-cospi[48]);
    const __m256i  cospi16   = _mm256_set1_epi32(cospi[16]);
    const __m256i  cospim16  = _mm256_set1_epi32(-cospi[16]);
    const __m256i  rounding  = _mm256_set1_epi32(1 << (bit - 1));
    const int32_t  log_range = AOMMAX(16, bd + (do_cols ? 6 : 8));
    const __m256i  clamp_lo  = _mm256_set1_epi32(-(1 << (log_range - 1)));
    const __m256i  clamp_hi  = _mm256_set1_epi32((1 << (log_range - 1)) - 1);
    __m256i        bf1[32];

    {
        // stage 0
        // stage 1
        bf1[0]  = in[0];
        bf1[4]  = in[4];
        bf1[8]  = in[2];
        bf1[12] = in[6];
        bf1[16] = in[1];
        bf1[20] = in[5];
        bf1[24] = in[3];
        bf1[28] = in[7];

        // stage 2
        bf1[31] = half_btf_0_avx2(&cospi2, &bf1[16], &rounding, bit);
        bf1[16] = half_btf_0_avx2(&cospi62, &bf1[16], &rounding, bit);
        bf1[19] = half_btf_0_avx2(&cospim50, &bf1[28], &rounding, bit);
        bf1[28] = half_btf_0_avx2(&cospi14, &bf1[28], &rounding, bit);
        bf1[27] = half_btf_0_avx2(&cospi10, &bf1[20], &rounding, bit);
        bf1[20] = half_btf_0_avx2(&cospi54, &bf1[20], &rounding, bit);
        bf1[23] = half_btf_0_avx2(&cospim58, &bf1[24], &rounding, bit);
        bf1[24] = half_btf_0_avx2(&cospi6, &bf1[24], &rounding, bit);

        // stage 3
        bf1[15] = half_btf_0_avx2(&cospi4, &bf1[8], &rounding, bit);
        bf1[8]  = half_btf_0_avx2(&cospi60, &bf1[8], &rounding, bit);

        bf1[11] = half_btf_0_avx2(&cospim52, &bf1[12], &rounding, bit);
        bf1[12] = half_btf_0_avx2(&cospi12, &bf1[12], &rounding, bit);
        bf1[17] = bf1[16];
        bf1[18] = bf1[19];
        bf1[21] = bf1[20];
        bf1[22] = bf1[23];
        bf1[25] = bf1[24];
        bf1[26] = bf1[27];
        bf1[29] = bf1[28];
        bf1[30] = bf1[31];

        // stage 4
        bf1[7] = half_btf_0_avx2(&cospi8, &bf1[4], &rounding, bit);
        bf1[4] = half_btf_0_avx2(&cospi56, &bf1[4], &rounding, bit);

        bf1[9]  = bf1[8];
        bf1[10] = bf1[11];
        bf1[13] = bf1[12];
        bf1[14] = bf1[15];

        idct32_stage4_avx2(
            bf1, &cospim8, &cospi56, &cospi8, &cospim56, &cospim40, &cospi24, &cospi40, &cospim24, &rounding, bit);

        // stage 5
        bf1[0] = half_btf_0_avx2(&cospi32, &bf1[0], &rounding, bit);
        bf1[1] = bf1[0];
        bf1[5] = bf1[4];
        bf1[6] = bf1[7];

        idct32_stage5_avx2(bf1, &cospim16, &cospi48, &cospi16, &cospim48, &clamp_lo, &clamp_hi, &rounding, bit);

        // stage 6
        bf1[3] = bf1[0];
        bf1[2] = bf1[1];

        idct32_stage6_avx2(
            bf1, &cospim32, &cospi32, &cospim16, &cospi48, &cospi16, &cospim48, &clamp_lo, &clamp_hi, &rounding, bit);

        // stage 7
        idct32_stage7_avx2(bf1, &cospim32, &cospi32, &clamp_lo, &clamp_hi, &rounding, bit);

        // stage 8
        idct32_stage8_avx2(bf1, &cospim32, &cospi32, &clamp_lo, &clamp_hi, &rounding, bit);

        // stage 9
        idct32_stage9_avx2(bf1, out, do_cols, bd, out_shift, &clamp_lo, &clamp_hi);
    }
}

static void idct32_low16_avx2(__m256i* in, __m256i* out, int32_t bit, int32_t do_cols, int32_t bd, int32_t out_shift) {
    const int32_t* cospi     = cospi_arr(bit);
    const __m256i  cospi62   = _mm256_set1_epi32(cospi[62]);
    const __m256i  cospi30   = _mm256_set1_epi32(cospi[30]);
    const __m256i  cospi46   = _mm256_set1_epi32(cospi[46]);
    const __m256i  cospi14   = _mm256_set1_epi32(cospi[14]);
    const __m256i  cospi54   = _mm256_set1_epi32(cospi[54]);
    const __m256i  cospi22   = _mm256_set1_epi32(cospi[22]);
    const __m256i  cospi38   = _mm256_set1_epi32(cospi[38]);
    const __m256i  cospi6    = _mm256_set1_epi32(cospi[6]);
    const __m256i  cospi26   = _mm256_set1_epi32(cospi[26]);
    const __m256i  cospi10   = _mm256_set1_epi32(cospi[10]);
    const __m256i  cospi18   = _mm256_set1_epi32(cospi[18]);
    const __m256i  cospi2    = _mm256_set1_epi32(cospi[2]);
    const __m256i  cospim58  = _mm256_set1_epi32(-cospi[58]);
    const __m256i  cospim42  = _mm256_set1_epi32(-cospi[42]);
    const __m256i  cospim50  = _mm256_set1_epi32(-cospi[50]);
    const __m256i  cospim34  = _mm256_set1_epi32(-cospi[34]);
    const __m256i  cospi60   = _mm256_set1_epi32(cospi[60]);
    const __m256i  cospi28   = _mm256_set1_epi32(cospi[28]);
    const __m256i  cospi44   = _mm256_set1_epi32(cospi[44]);
    const __m256i  cospi12   = _mm256_set1_epi32(cospi[12]);
    const __m256i  cospi20   = _mm256_set1_epi32(cospi[20]);
    const __m256i  cospi4    = _mm256_set1_epi32(cospi[4]);
    const __m256i  cospim52  = _mm256_set1_epi32(-cospi[52]);
    const __m256i  cospim36  = _mm256_set1_epi32(-cospi[36]);
    const __m256i  cospi56   = _mm256_set1_epi32(cospi[56]);
    const __m256i  cospi24   = _mm256_set1_epi32(cospi[24]);
    const __m256i  cospi40   = _mm256_set1_epi32(cospi[40]);
    const __m256i  cospi8    = _mm256_set1_epi32(cospi[8]);
    const __m256i  cospim40  = _mm256_set1_epi32(-cospi[40]);
    const __m256i  cospim8   = _mm256_set1_epi32(-cospi[8]);
    const __m256i  cospim56  = _mm256_set1_epi32(-cospi[56]);
    const __m256i  cospim24  = _mm256_set1_epi32(-cospi[24]);
    const __m256i  cospi32   = _mm256_set1_epi32(cospi[32]);
    const __m256i  cospim32  = _mm256_set1_epi32(-cospi[32]);
    const __m256i  cospi48   = _mm256_set1_epi32(cospi[48]);
    const __m256i  cospim48  = _mm256_set1_epi32(-cospi[48]);
    const __m256i  cospi16   = _mm256_set1_epi32(cospi[16]);
    const __m256i  cospim16  = _mm256_set1_epi32(-cospi[16]);
    const __m256i  rounding  = _mm256_set1_epi32(1 << (bit - 1));
    const int32_t  log_range = AOMMAX(16, bd + (do_cols ? 6 : 8));
    const __m256i  clamp_lo  = _mm256_set1_epi32(-(1 << (log_range - 1)));
    const __m256i  clamp_hi  = _mm256_set1_epi32((1 << (log_range - 1)) - 1);
    __m256i        bf1[32];

    {
        // stage 0
        // stage 1
        bf1[0]  = in[0];
        bf1[2]  = in[8];
        bf1[4]  = in[4];
        bf1[6]  = in[12];
        bf1[8]  = in[2];
        bf1[10] = in[10];
        bf1[12] = in[6];
        bf1[14] = in[14];
        bf1[16] = in[1];
        bf1[18] = in[9];
        bf1[20] = in[5];
        bf1[22] = in[13];
        bf1[24] = in[3];
        bf1[26] = in[11];
        bf1[28] = in[7];
        bf1[30] = in[15];

        // stage 2
        bf1[31] = half_btf_0_avx2(&cospi2, &bf1[16], &rounding, bit);
        bf1[16] = half_btf_0_avx2(&cospi62, &bf1[16], &rounding, bit);
        bf1[17] = half_btf_0_avx2(&cospim34, &bf1[30], &rounding, bit);
        bf1[30] = half_btf_0_avx2(&cospi30, &bf1[30], &rounding, bit);
        bf1[29] = half_btf_0_avx2(&cospi18, &bf1[18], &rounding, bit);
        bf1[18] = half_btf_0_avx2(&cospi46, &bf1[18], &rounding, bit);
        bf1[19] = half_btf_0_avx2(&cospim50, &bf1[28], &rounding, bit);
        bf1[28] = half_btf_0_avx2(&cospi14, &bf1[28], &rounding, bit);
        bf1[27] = half_btf_0_avx2(&cospi10, &bf1[20], &rounding, bit);
        bf1[20] = half_btf_0_avx2(&cospi54, &bf1[20], &rounding, bit);
        bf1[21] = half_btf_0_avx2(&cospim42, &bf1[26], &rounding, bit);
        bf1[26] = half_btf_0_avx2(&cospi22, &bf1[26], &rounding, bit);
        bf1[25] = half_btf_0_avx2(&cospi26, &bf1[22], &rounding, bit);
        bf1[22] = half_btf_0_avx2(&cospi38, &bf1[22], &rounding, bit);
        bf1[23] = half_btf_0_avx2(&cospim58, &bf1[24], &rounding, bit);
        bf1[24] = half_btf_0_avx2(&cospi6, &bf1[24], &rounding, bit);

        // stage 3
        bf1[15] = half_btf_0_avx2(&cospi4, &bf1[8], &rounding, bit);
        bf1[8]  = half_btf_0_avx2(&cospi60, &bf1[8], &rounding, bit);
        bf1[9]  = half_btf_0_avx2(&cospim36, &bf1[14], &rounding, bit);
        bf1[14] = half_btf_0_avx2(&cospi28, &bf1[14], &rounding, bit);
        bf1[13] = half_btf_0_avx2(&cospi20, &bf1[10], &rounding, bit);
        bf1[10] = half_btf_0_avx2(&cospi44, &bf1[10], &rounding, bit);
        bf1[11] = half_btf_0_avx2(&cospim52, &bf1[12], &rounding, bit);
        bf1[12] = half_btf_0_avx2(&cospi12, &bf1[12], &rounding, bit);

        addsub_avx2(bf1[16], bf1[17], bf1 + 16, bf1 + 17, &clamp_lo, &clamp_hi);
        addsub_avx2(bf1[19], bf1[18], bf1 + 19, bf1 + 18, &clamp_lo, &clamp_hi);
        addsub_avx2(bf1[20], bf1[21], bf1 + 20, bf1 + 21, &clamp_lo, &clamp_hi);
        addsub_avx2(bf1[23], bf1[22], bf1 + 23, bf1 + 22, &clamp_lo, &clamp_hi);
        addsub_avx2(bf1[24], bf1[25], bf1 + 24, bf1 + 25, &clamp_lo, &clamp_hi);
        addsub_avx2(bf1[27], bf1[26], bf1 + 27, bf1 + 26, &clamp_lo, &clamp_hi);
        addsub_avx2(bf1[28], bf1[29], bf1 + 28, bf1 + 29, &clamp_lo, &clamp_hi);
        addsub_avx2(bf1[31], bf1[30], bf1 + 31, bf1 + 30, &clamp_lo, &clamp_hi);

        // stage 4
        bf1[7] = half_btf_0_avx2(&cospi8, &bf1[4], &rounding, bit);
        bf1[4] = half_btf_0_avx2(&cospi56, &bf1[4], &rounding, bit);
        bf1[5] = half_btf_0_avx2(&cospim40, &bf1[6], &rounding, bit);
        bf1[6] = half_btf_0_avx2(&cospi24, &bf1[6], &rounding, bit);

        addsub_avx2(bf1[8], bf1[9], bf1 + 8, bf1 + 9, &clamp_lo, &clamp_hi);
        addsub_avx2(bf1[11], bf1[10], bf1 + 11, bf1 + 10, &clamp_lo, &clamp_hi);
        addsub_avx2(bf1[12], bf1[13], bf1 + 12, bf1 + 13, &clamp_lo, &clamp_hi);
        addsub_avx2(bf1[15], bf1[14], bf1 + 15, bf1 + 14, &clamp_lo, &clamp_hi);

        idct32_stage4_avx2(
            bf1, &cospim8, &cospi56, &cospi8, &cospim56, &cospim40, &cospi24, &cospi40, &cospim24, &rounding, bit);

        // stage 5
        bf1[0] = half_btf_0_avx2(&cospi32, &bf1[0], &rounding, bit);
        bf1[1] = bf1[0];
        bf1[3] = half_btf_0_avx2(&cospi16, &bf1[2], &rounding, bit);
        bf1[2] = half_btf_0_avx2(&cospi48, &bf1[2], &rounding, bit);

        addsub_avx2(bf1[4], bf1[5], bf1 + 4, bf1 + 5, &clamp_lo, &clamp_hi);
        addsub_avx2(bf1[7], bf1[6], bf1 + 7, bf1 + 6, &clamp_lo, &clamp_hi);

        idct32_stage5_avx2(bf1, &cospim16, &cospi48, &cospi16, &cospim48, &clamp_lo, &clamp_hi, &rounding, bit);

        // stage 6
        addsub_avx2(bf1[0], bf1[3], bf1 + 0, bf1 + 3, &clamp_lo, &clamp_hi);
        addsub_avx2(bf1[1], bf1[2], bf1 + 1, bf1 + 2, &clamp_lo, &clamp_hi);

        idct32_stage6_avx2(
            bf1, &cospim32, &cospi32, &cospim16, &cospi48, &cospi16, &cospim48, &clamp_lo, &clamp_hi, &rounding, bit);

        // stage 7
        idct32_stage7_avx2(bf1, &cospim32, &cospi32, &clamp_lo, &clamp_hi, &rounding, bit);

        // stage 8
        idct32_stage8_avx2(bf1, &cospim32, &cospi32, &clamp_lo, &clamp_hi, &rounding, bit);

        // stage 9
        idct32_stage9_avx2(bf1, out, do_cols, bd, out_shift, &clamp_lo, &clamp_hi);
    }
}

static void idct32_avx2_new(__m256i* in, __m256i* out, int32_t bit, int32_t do_cols, int32_t bd, int32_t out_shift) {
    const int32_t* cospi     = cospi_arr(bit);
    const __m256i  cospi62   = _mm256_set1_epi32(cospi[62]);
    const __m256i  cospi30   = _mm256_set1_epi32(cospi[30]);
    const __m256i  cospi46   = _mm256_set1_epi32(cospi[46]);
    const __m256i  cospi14   = _mm256_set1_epi32(cospi[14]);
    const __m256i  cospi54   = _mm256_set1_epi32(cospi[54]);
    const __m256i  cospi22   = _mm256_set1_epi32(cospi[22]);
    const __m256i  cospi38   = _mm256_set1_epi32(cospi[38]);
    const __m256i  cospi6    = _mm256_set1_epi32(cospi[6]);
    const __m256i  cospi58   = _mm256_set1_epi32(cospi[58]);
    const __m256i  cospi26   = _mm256_set1_epi32(cospi[26]);
    const __m256i  cospi42   = _mm256_set1_epi32(cospi[42]);
    const __m256i  cospi10   = _mm256_set1_epi32(cospi[10]);
    const __m256i  cospi50   = _mm256_set1_epi32(cospi[50]);
    const __m256i  cospi18   = _mm256_set1_epi32(cospi[18]);
    const __m256i  cospi34   = _mm256_set1_epi32(cospi[34]);
    const __m256i  cospi2    = _mm256_set1_epi32(cospi[2]);
    const __m256i  cospim58  = _mm256_set1_epi32(-cospi[58]);
    const __m256i  cospim26  = _mm256_set1_epi32(-cospi[26]);
    const __m256i  cospim42  = _mm256_set1_epi32(-cospi[42]);
    const __m256i  cospim10  = _mm256_set1_epi32(-cospi[10]);
    const __m256i  cospim50  = _mm256_set1_epi32(-cospi[50]);
    const __m256i  cospim18  = _mm256_set1_epi32(-cospi[18]);
    const __m256i  cospim34  = _mm256_set1_epi32(-cospi[34]);
    const __m256i  cospim2   = _mm256_set1_epi32(-cospi[2]);
    const __m256i  cospi60   = _mm256_set1_epi32(cospi[60]);
    const __m256i  cospi28   = _mm256_set1_epi32(cospi[28]);
    const __m256i  cospi44   = _mm256_set1_epi32(cospi[44]);
    const __m256i  cospi12   = _mm256_set1_epi32(cospi[12]);
    const __m256i  cospi52   = _mm256_set1_epi32(cospi[52]);
    const __m256i  cospi20   = _mm256_set1_epi32(cospi[20]);
    const __m256i  cospi36   = _mm256_set1_epi32(cospi[36]);
    const __m256i  cospi4    = _mm256_set1_epi32(cospi[4]);
    const __m256i  cospim52  = _mm256_set1_epi32(-cospi[52]);
    const __m256i  cospim20  = _mm256_set1_epi32(-cospi[20]);
    const __m256i  cospim36  = _mm256_set1_epi32(-cospi[36]);
    const __m256i  cospim4   = _mm256_set1_epi32(-cospi[4]);
    const __m256i  cospi56   = _mm256_set1_epi32(cospi[56]);
    const __m256i  cospi24   = _mm256_set1_epi32(cospi[24]);
    const __m256i  cospi40   = _mm256_set1_epi32(cospi[40]);
    const __m256i  cospi8    = _mm256_set1_epi32(cospi[8]);
    const __m256i  cospim40  = _mm256_set1_epi32(-cospi[40]);
    const __m256i  cospim8   = _mm256_set1_epi32(-cospi[8]);
    const __m256i  cospim56  = _mm256_set1_epi32(-cospi[56]);
    const __m256i  cospim24  = _mm256_set1_epi32(-cospi[24]);
    const __m256i  cospi32   = _mm256_set1_epi32(cospi[32]);
    const __m256i  cospim32  = _mm256_set1_epi32(-cospi[32]);
    const __m256i  cospi48   = _mm256_set1_epi32(cospi[48]);
    const __m256i  cospim48  = _mm256_set1_epi32(-cospi[48]);
    const __m256i  cospi16   = _mm256_set1_epi32(cospi[16]);
    const __m256i  cospim16  = _mm256_set1_epi32(-cospi[16]);
    const __m256i  rounding  = _mm256_set1_epi32(1 << (bit - 1));
    const int32_t  log_range = AOMMAX(16, bd + (do_cols ? 6 : 8));
    const __m256i  clamp_lo  = _mm256_set1_epi32(-(1 << (log_range - 1)));
    const __m256i  clamp_hi  = _mm256_set1_epi32((1 << (log_range - 1)) - 1);
    __m256i        bf1[32], bf0[32];

    {
        // stage 0
        // stage 1
        bf1[0]  = in[0];
        bf1[1]  = in[16];
        bf1[2]  = in[8];
        bf1[3]  = in[24];
        bf1[4]  = in[4];
        bf1[5]  = in[20];
        bf1[6]  = in[12];
        bf1[7]  = in[28];
        bf1[8]  = in[2];
        bf1[9]  = in[18];
        bf1[10] = in[10];
        bf1[11] = in[26];
        bf1[12] = in[6];
        bf1[13] = in[22];
        bf1[14] = in[14];
        bf1[15] = in[30];
        bf1[16] = in[1];
        bf1[17] = in[17];
        bf1[18] = in[9];
        bf1[19] = in[25];
        bf1[20] = in[5];
        bf1[21] = in[21];
        bf1[22] = in[13];
        bf1[23] = in[29];
        bf1[24] = in[3];
        bf1[25] = in[19];
        bf1[26] = in[11];
        bf1[27] = in[27];
        bf1[28] = in[7];
        bf1[29] = in[23];
        bf1[30] = in[15];
        bf1[31] = in[31];

        // stage 2
        bf0[0]  = bf1[0];
        bf0[1]  = bf1[1];
        bf0[2]  = bf1[2];
        bf0[3]  = bf1[3];
        bf0[4]  = bf1[4];
        bf0[5]  = bf1[5];
        bf0[6]  = bf1[6];
        bf0[7]  = bf1[7];
        bf0[8]  = bf1[8];
        bf0[9]  = bf1[9];
        bf0[10] = bf1[10];
        bf0[11] = bf1[11];
        bf0[12] = bf1[12];
        bf0[13] = bf1[13];
        bf0[14] = bf1[14];
        bf0[15] = bf1[15];
        bf0[16] = half_btf_avx2(&cospi62, &bf1[16], &cospim2, &bf1[31], &rounding, bit);
        bf0[17] = half_btf_avx2(&cospi30, &bf1[17], &cospim34, &bf1[30], &rounding, bit);
        bf0[18] = half_btf_avx2(&cospi46, &bf1[18], &cospim18, &bf1[29], &rounding, bit);
        bf0[19] = half_btf_avx2(&cospi14, &bf1[19], &cospim50, &bf1[28], &rounding, bit);
        bf0[20] = half_btf_avx2(&cospi54, &bf1[20], &cospim10, &bf1[27], &rounding, bit);
        bf0[21] = half_btf_avx2(&cospi22, &bf1[21], &cospim42, &bf1[26], &rounding, bit);
        bf0[22] = half_btf_avx2(&cospi38, &bf1[22], &cospim26, &bf1[25], &rounding, bit);
        bf0[23] = half_btf_avx2(&cospi6, &bf1[23], &cospim58, &bf1[24], &rounding, bit);
        bf0[24] = half_btf_avx2(&cospi58, &bf1[23], &cospi6, &bf1[24], &rounding, bit);
        bf0[25] = half_btf_avx2(&cospi26, &bf1[22], &cospi38, &bf1[25], &rounding, bit);
        bf0[26] = half_btf_avx2(&cospi42, &bf1[21], &cospi22, &bf1[26], &rounding, bit);
        bf0[27] = half_btf_avx2(&cospi10, &bf1[20], &cospi54, &bf1[27], &rounding, bit);
        bf0[28] = half_btf_avx2(&cospi50, &bf1[19], &cospi14, &bf1[28], &rounding, bit);
        bf0[29] = half_btf_avx2(&cospi18, &bf1[18], &cospi46, &bf1[29], &rounding, bit);
        bf0[30] = half_btf_avx2(&cospi34, &bf1[17], &cospi30, &bf1[30], &rounding, bit);
        bf0[31] = half_btf_avx2(&cospi2, &bf1[16], &cospi62, &bf1[31], &rounding, bit);

        // stage 3
        bf1[0]  = bf0[0];
        bf1[1]  = bf0[1];
        bf1[2]  = bf0[2];
        bf1[3]  = bf0[3];
        bf1[4]  = bf0[4];
        bf1[5]  = bf0[5];
        bf1[6]  = bf0[6];
        bf1[7]  = bf0[7];
        bf1[8]  = half_btf_avx2(&cospi60, &bf0[8], &cospim4, &bf0[15], &rounding, bit);
        bf1[9]  = half_btf_avx2(&cospi28, &bf0[9], &cospim36, &bf0[14], &rounding, bit);
        bf1[10] = half_btf_avx2(&cospi44, &bf0[10], &cospim20, &bf0[13], &rounding, bit);
        bf1[11] = half_btf_avx2(&cospi12, &bf0[11], &cospim52, &bf0[12], &rounding, bit);
        bf1[12] = half_btf_avx2(&cospi52, &bf0[11], &cospi12, &bf0[12], &rounding, bit);
        bf1[13] = half_btf_avx2(&cospi20, &bf0[10], &cospi44, &bf0[13], &rounding, bit);
        bf1[14] = half_btf_avx2(&cospi36, &bf0[9], &cospi28, &bf0[14], &rounding, bit);
        bf1[15] = half_btf_avx2(&cospi4, &bf0[8], &cospi60, &bf0[15], &rounding, bit);

        addsub_avx2(bf0[16], bf0[17], bf1 + 16, bf1 + 17, &clamp_lo, &clamp_hi);
        addsub_avx2(bf0[19], bf0[18], bf1 + 19, bf1 + 18, &clamp_lo, &clamp_hi);
        addsub_avx2(bf0[20], bf0[21], bf1 + 20, bf1 + 21, &clamp_lo, &clamp_hi);
        addsub_avx2(bf0[23], bf0[22], bf1 + 23, bf1 + 22, &clamp_lo, &clamp_hi);
        addsub_avx2(bf0[24], bf0[25], bf1 + 24, bf1 + 25, &clamp_lo, &clamp_hi);
        addsub_avx2(bf0[27], bf0[26], bf1 + 27, bf1 + 26, &clamp_lo, &clamp_hi);
        addsub_avx2(bf0[28], bf0[29], bf1 + 28, bf1 + 29, &clamp_lo, &clamp_hi);
        addsub_avx2(bf0[31], bf0[30], bf1 + 31, bf1 + 30, &clamp_lo, &clamp_hi);

        // stage 4
        bf0[0] = bf1[0];
        bf0[1] = bf1[1];
        bf0[2] = bf1[2];
        bf0[3] = bf1[3];
        bf0[4] = half_btf_avx2(&cospi56, &bf1[4], &cospim8, &bf1[7], &rounding, bit);
        bf0[5] = half_btf_avx2(&cospi24, &bf1[5], &cospim40, &bf1[6], &rounding, bit);
        bf0[6] = half_btf_avx2(&cospi40, &bf1[5], &cospi24, &bf1[6], &rounding, bit);
        bf0[7] = half_btf_avx2(&cospi8, &bf1[4], &cospi56, &bf1[7], &rounding, bit);

        addsub_avx2(bf1[8], bf1[9], bf0 + 8, bf0 + 9, &clamp_lo, &clamp_hi);
        addsub_avx2(bf1[11], bf1[10], bf0 + 11, bf0 + 10, &clamp_lo, &clamp_hi);
        addsub_avx2(bf1[12], bf1[13], bf0 + 12, bf0 + 13, &clamp_lo, &clamp_hi);
        addsub_avx2(bf1[15], bf1[14], bf0 + 15, bf0 + 14, &clamp_lo, &clamp_hi);

        bf0[16] = bf1[16];
        bf0[17] = half_btf_avx2(&cospim8, &bf1[17], &cospi56, &bf1[30], &rounding, bit);
        bf0[18] = half_btf_avx2(&cospim56, &bf1[18], &cospim8, &bf1[29], &rounding, bit);
        bf0[19] = bf1[19];
        bf0[20] = bf1[20];
        bf0[21] = half_btf_avx2(&cospim40, &bf1[21], &cospi24, &bf1[26], &rounding, bit);
        bf0[22] = half_btf_avx2(&cospim24, &bf1[22], &cospim40, &bf1[25], &rounding, bit);
        bf0[23] = bf1[23];
        bf0[24] = bf1[24];
        bf0[25] = half_btf_avx2(&cospim40, &bf1[22], &cospi24, &bf1[25], &rounding, bit);
        bf0[26] = half_btf_avx2(&cospi24, &bf1[21], &cospi40, &bf1[26], &rounding, bit);
        bf0[27] = bf1[27];
        bf0[28] = bf1[28];
        bf0[29] = half_btf_avx2(&cospim8, &bf1[18], &cospi56, &bf1[29], &rounding, bit);
        bf0[30] = half_btf_avx2(&cospi56, &bf1[17], &cospi8, &bf1[30], &rounding, bit);
        bf0[31] = bf1[31];

        // stage 5
        bf1[0] = half_btf_avx2(&cospi32, &bf0[0], &cospi32, &bf0[1], &rounding, bit);
        bf1[1] = half_btf_avx2(&cospi32, &bf0[0], &cospim32, &bf0[1], &rounding, bit);
        bf1[2] = half_btf_avx2(&cospi48, &bf0[2], &cospim16, &bf0[3], &rounding, bit);
        bf1[3] = half_btf_avx2(&cospi16, &bf0[2], &cospi48, &bf0[3], &rounding, bit);
        addsub_avx2(bf0[4], bf0[5], bf1 + 4, bf1 + 5, &clamp_lo, &clamp_hi);
        addsub_avx2(bf0[7], bf0[6], bf1 + 7, bf1 + 6, &clamp_lo, &clamp_hi);
        bf1[8]  = bf0[8];
        bf1[9]  = half_btf_avx2(&cospim16, &bf0[9], &cospi48, &bf0[14], &rounding, bit);
        bf1[10] = half_btf_avx2(&cospim48, &bf0[10], &cospim16, &bf0[13], &rounding, bit);
        bf1[11] = bf0[11];
        bf1[12] = bf0[12];
        bf1[13] = half_btf_avx2(&cospim16, &bf0[10], &cospi48, &bf0[13], &rounding, bit);
        bf1[14] = half_btf_avx2(&cospi48, &bf0[9], &cospi16, &bf0[14], &rounding, bit);
        bf1[15] = bf0[15];
        addsub_avx2(bf0[16], bf0[19], bf1 + 16, bf1 + 19, &clamp_lo, &clamp_hi);
        addsub_avx2(bf0[17], bf0[18], bf1 + 17, bf1 + 18, &clamp_lo, &clamp_hi);
        addsub_avx2(bf0[23], bf0[20], bf1 + 23, bf1 + 20, &clamp_lo, &clamp_hi);
        addsub_avx2(bf0[22], bf0[21], bf1 + 22, bf1 + 21, &clamp_lo, &clamp_hi);
        addsub_avx2(bf0[24], bf0[27], bf1 + 24, bf1 + 27, &clamp_lo, &clamp_hi);
        addsub_avx2(bf0[25], bf0[26], bf1 + 25, bf1 + 26, &clamp_lo, &clamp_hi);
        addsub_avx2(bf0[31], bf0[28], bf1 + 31, bf1 + 28, &clamp_lo, &clamp_hi);
        addsub_avx2(bf0[30], bf0[29], bf1 + 30, bf1 + 29, &clamp_lo, &clamp_hi);

        // stage 6
        addsub_avx2(bf1[0], bf1[3], bf0 + 0, bf0 + 3, &clamp_lo, &clamp_hi);
        addsub_avx2(bf1[1], bf1[2], bf0 + 1, bf0 + 2, &clamp_lo, &clamp_hi);
        bf0[4] = bf1[4];
        bf0[5] = half_btf_avx2(&cospim32, &bf1[5], &cospi32, &bf1[6], &rounding, bit);
        bf0[6] = half_btf_avx2(&cospi32, &bf1[5], &cospi32, &bf1[6], &rounding, bit);
        bf0[7] = bf1[7];
        addsub_avx2(bf1[8], bf1[11], bf0 + 8, bf0 + 11, &clamp_lo, &clamp_hi);
        addsub_avx2(bf1[9], bf1[10], bf0 + 9, bf0 + 10, &clamp_lo, &clamp_hi);
        addsub_avx2(bf1[15], bf1[12], bf0 + 15, bf0 + 12, &clamp_lo, &clamp_hi);
        addsub_avx2(bf1[14], bf1[13], bf0 + 14, bf0 + 13, &clamp_lo, &clamp_hi);
        bf0[16] = bf1[16];
        bf0[17] = bf1[17];
        bf0[18] = half_btf_avx2(&cospim16, &bf1[18], &cospi48, &bf1[29], &rounding, bit);
        bf0[19] = half_btf_avx2(&cospim16, &bf1[19], &cospi48, &bf1[28], &rounding, bit);
        bf0[20] = half_btf_avx2(&cospim48, &bf1[20], &cospim16, &bf1[27], &rounding, bit);
        bf0[21] = half_btf_avx2(&cospim48, &bf1[21], &cospim16, &bf1[26], &rounding, bit);
        bf0[22] = bf1[22];
        bf0[23] = bf1[23];
        bf0[24] = bf1[24];
        bf0[25] = bf1[25];
        bf0[26] = half_btf_avx2(&cospim16, &bf1[21], &cospi48, &bf1[26], &rounding, bit);
        bf0[27] = half_btf_avx2(&cospim16, &bf1[20], &cospi48, &bf1[27], &rounding, bit);
        bf0[28] = half_btf_avx2(&cospi48, &bf1[19], &cospi16, &bf1[28], &rounding, bit);
        bf0[29] = half_btf_avx2(&cospi48, &bf1[18], &cospi16, &bf1[29], &rounding, bit);
        bf0[30] = bf1[30];
        bf0[31] = bf1[31];

        // stage 7
        addsub_avx2(bf0[0], bf0[7], bf1 + 0, bf1 + 7, &clamp_lo, &clamp_hi);
        addsub_avx2(bf0[1], bf0[6], bf1 + 1, bf1 + 6, &clamp_lo, &clamp_hi);
        addsub_avx2(bf0[2], bf0[5], bf1 + 2, bf1 + 5, &clamp_lo, &clamp_hi);
        addsub_avx2(bf0[3], bf0[4], bf1 + 3, bf1 + 4, &clamp_lo, &clamp_hi);
        bf1[8]  = bf0[8];
        bf1[9]  = bf0[9];
        bf1[10] = half_btf_avx2(&cospim32, &bf0[10], &cospi32, &bf0[13], &rounding, bit);
        bf1[11] = half_btf_avx2(&cospim32, &bf0[11], &cospi32, &bf0[12], &rounding, bit);
        bf1[12] = half_btf_avx2(&cospi32, &bf0[11], &cospi32, &bf0[12], &rounding, bit);
        bf1[13] = half_btf_avx2(&cospi32, &bf0[10], &cospi32, &bf0[13], &rounding, bit);
        bf1[14] = bf0[14];
        bf1[15] = bf0[15];
        addsub_avx2(bf0[16], bf0[23], bf1 + 16, bf1 + 23, &clamp_lo, &clamp_hi);
        addsub_avx2(bf0[17], bf0[22], bf1 + 17, bf1 + 22, &clamp_lo, &clamp_hi);
        addsub_avx2(bf0[18], bf0[21], bf1 + 18, bf1 + 21, &clamp_lo, &clamp_hi);
        addsub_avx2(bf0[19], bf0[20], bf1 + 19, bf1 + 20, &clamp_lo, &clamp_hi);
        addsub_avx2(bf0[31], bf0[24], bf1 + 31, bf1 + 24, &clamp_lo, &clamp_hi);
        addsub_avx2(bf0[30], bf0[25], bf1 + 30, bf1 + 25, &clamp_lo, &clamp_hi);
        addsub_avx2(bf0[29], bf0[26], bf1 + 29, bf1 + 26, &clamp_lo, &clamp_hi);
        addsub_avx2(bf0[28], bf0[27], bf1 + 28, bf1 + 27, &clamp_lo, &clamp_hi);

        // stage 8
        addsub_avx2(bf1[0], bf1[15], bf0 + 0, bf0 + 15, &clamp_lo, &clamp_hi);
        addsub_avx2(bf1[1], bf1[14], bf0 + 1, bf0 + 14, &clamp_lo, &clamp_hi);
        addsub_avx2(bf1[2], bf1[13], bf0 + 2, bf0 + 13, &clamp_lo, &clamp_hi);
        addsub_avx2(bf1[3], bf1[12], bf0 + 3, bf0 + 12, &clamp_lo, &clamp_hi);
        addsub_avx2(bf1[4], bf1[11], bf0 + 4, bf0 + 11, &clamp_lo, &clamp_hi);
        addsub_avx2(bf1[5], bf1[10], bf0 + 5, bf0 + 10, &clamp_lo, &clamp_hi);
        addsub_avx2(bf1[6], bf1[9], bf0 + 6, bf0 + 9, &clamp_lo, &clamp_hi);
        addsub_avx2(bf1[7], bf1[8], bf0 + 7, bf0 + 8, &clamp_lo, &clamp_hi);
        bf0[16] = bf1[16];
        bf0[17] = bf1[17];
        bf0[18] = bf1[18];
        bf0[19] = bf1[19];
        bf0[20] = half_btf_avx2(&cospim32, &bf1[20], &cospi32, &bf1[27], &rounding, bit);
        bf0[21] = half_btf_avx2(&cospim32, &bf1[21], &cospi32, &bf1[26], &rounding, bit);
        bf0[22] = half_btf_avx2(&cospim32, &bf1[22], &cospi32, &bf1[25], &rounding, bit);
        bf0[23] = half_btf_avx2(&cospim32, &bf1[23], &cospi32, &bf1[24], &rounding, bit);
        bf0[24] = half_btf_avx2(&cospi32, &bf1[23], &cospi32, &bf1[24], &rounding, bit);
        bf0[25] = half_btf_avx2(&cospi32, &bf1[22], &cospi32, &bf1[25], &rounding, bit);
        bf0[26] = half_btf_avx2(&cospi32, &bf1[21], &cospi32, &bf1[26], &rounding, bit);
        bf0[27] = half_btf_avx2(&cospi32, &bf1[20], &cospi32, &bf1[27], &rounding, bit);
        bf0[28] = bf1[28];
        bf0[29] = bf1[29];
        bf0[30] = bf1[30];
        bf0[31] = bf1[31];

        // stage 9
        addsub_avx2(bf0[0], bf0[31], out + 0, out + 31, &clamp_lo, &clamp_hi);
        addsub_avx2(bf0[1], bf0[30], out + 1, out + 30, &clamp_lo, &clamp_hi);
        addsub_avx2(bf0[2], bf0[29], out + 2, out + 29, &clamp_lo, &clamp_hi);
        addsub_avx2(bf0[3], bf0[28], out + 3, out + 28, &clamp_lo, &clamp_hi);
        addsub_avx2(bf0[4], bf0[27], out + 4, out + 27, &clamp_lo, &clamp_hi);
        addsub_avx2(bf0[5], bf0[26], out + 5, out + 26, &clamp_lo, &clamp_hi);
        addsub_avx2(bf0[6], bf0[25], out + 6, out + 25, &clamp_lo, &clamp_hi);
        addsub_avx2(bf0[7], bf0[24], out + 7, out + 24, &clamp_lo, &clamp_hi);
        addsub_avx2(bf0[8], bf0[23], out + 8, out + 23, &clamp_lo, &clamp_hi);
        addsub_avx2(bf0[9], bf0[22], out + 9, out + 22, &clamp_lo, &clamp_hi);
        addsub_avx2(bf0[10], bf0[21], out + 10, out + 21, &clamp_lo, &clamp_hi);
        addsub_avx2(bf0[11], bf0[20], out + 11, out + 20, &clamp_lo, &clamp_hi);
        addsub_avx2(bf0[12], bf0[19], out + 12, out + 19, &clamp_lo, &clamp_hi);
        addsub_avx2(bf0[13], bf0[18], out + 13, out + 18, &clamp_lo, &clamp_hi);
        addsub_avx2(bf0[14], bf0[17], out + 14, out + 17, &clamp_lo, &clamp_hi);
        addsub_avx2(bf0[15], bf0[16], out + 15, out + 16, &clamp_lo, &clamp_hi);
        if (!do_cols) {
            const int     log_range_out = AOMMAX(16, bd + 6);
            const __m256i clamp_lo_out  = _mm256_set1_epi32(-(1 << (log_range_out - 1)));
            const __m256i clamp_hi_out  = _mm256_set1_epi32((1 << (log_range_out - 1)) - 1);
            round_shift_8x8_avx2(out, out_shift);
            round_shift_8x8_avx2(out + 16, out_shift);
            highbd_clamp_epi32_avx2(out, out, &clamp_lo_out, &clamp_hi_out, 32);
        }
    }
}

static void iidentity32_avx2(__m256i* in, __m256i* out, int32_t bit, int32_t do_cols, int32_t bd, int32_t out_shift) {
    (void)bit;
    const int32_t log_range = AOMMAX(16, bd + (do_cols ? 6 : 8));
    const __m256i clamp_lo  = _mm256_set1_epi32(-(1 << (log_range - 1)));
    const __m256i clamp_hi  = _mm256_set1_epi32((1 << (log_range - 1)) - 1);
    __m256i       v[64];
    for (int32_t i = 0; i < 32; i += 16) {
        v[i]      = _mm256_slli_epi32(in[i], 2);
        v[i + 1]  = _mm256_slli_epi32(in[i + 1], 2);
        v[i + 2]  = _mm256_slli_epi32(in[i + 2], 2);
        v[i + 3]  = _mm256_slli_epi32(in[i + 3], 2);
        v[i + 4]  = _mm256_slli_epi32(in[i + 4], 2);
        v[i + 5]  = _mm256_slli_epi32(in[i + 5], 2);
        v[i + 6]  = _mm256_slli_epi32(in[i + 6], 2);
        v[i + 7]  = _mm256_slli_epi32(in[i + 7], 2);
        v[i + 8]  = _mm256_slli_epi32(in[i + 8], 2);
        v[i + 9]  = _mm256_slli_epi32(in[i + 9], 2);
        v[i + 10] = _mm256_slli_epi32(in[i + 10], 2);
        v[i + 11] = _mm256_slli_epi32(in[i + 11], 2);
        v[i + 12] = _mm256_slli_epi32(in[i + 12], 2);
        v[i + 13] = _mm256_slli_epi32(in[i + 13], 2);
        v[i + 14] = _mm256_slli_epi32(in[i + 14], 2);
        v[i + 15] = _mm256_slli_epi32(in[i + 15], 2);
    }

    if (!do_cols) {
        const int32_t log_range_out = AOMMAX(16, bd + 6);
        const __m256i clamp_lo_out  = _mm256_set1_epi32(
            AOMMAX(-(1 << (log_range_out - 1)), -(1 << (log_range - 1 - out_shift))));
        const __m256i clamp_hi_out = _mm256_set1_epi32(
            AOMMIN((1 << (log_range_out - 1)) - 1, (1 << (log_range - 1 - out_shift))));
        shift_avx2(v, out, &clamp_lo_out, &clamp_hi_out, out_shift, 32);
    } else {
        highbd_clamp_epi32_avx2(v, out, &clamp_lo, &clamp_hi, 32);
    }
}

static void idct64_low1_avx2(__m256i* in, __m256i* out, int32_t bit, int32_t do_cols, int32_t bd, int32_t out_shift) {
    const int32_t* cospi     = cospi_arr(bit);
    const __m256i  rnding    = _mm256_set1_epi32(1 << (bit - 1));
    const int32_t  log_range = AOMMAX(16, bd + (do_cols ? 6 : 8));
    __m256i        clamp_lo  = _mm256_set1_epi32(-(1 << (log_range - 1)));
    __m256i        clamp_hi  = _mm256_set1_epi32((1 << (log_range - 1)) - 1);

    const __m256i cospi32 = _mm256_set1_epi32(cospi[32]);

    {
        __m256i x;

        // stage 1
        // stage 2
        // stage 3
        // stage 4
        // stage 5
        // stage 6
        x = half_btf_0_avx2(&cospi32, &in[0], &rnding, bit);

        // stage 8
        // stage 9
        // stage 10
        // stage 11
        if (!do_cols) {
            const int log_range_out = AOMMAX(16, bd + 6);
            clamp_lo                = _mm256_set1_epi32(-(1 << (log_range_out - 1)));
            clamp_hi                = _mm256_set1_epi32((1 << (log_range_out - 1)) - 1);
            if (out_shift != 0) {
                __m256i offset = _mm256_set1_epi32((1 << out_shift) >> 1);
                x              = _mm256_add_epi32(x, offset);
                x              = _mm256_sra_epi32(x, _mm_cvtsi32_si128(out_shift));
            }
        }
        x       = _mm256_max_epi32(x, clamp_lo);
        x       = _mm256_min_epi32(x, clamp_hi);
        out[0]  = x;
        out[1]  = x;
        out[2]  = x;
        out[3]  = x;
        out[4]  = x;
        out[5]  = x;
        out[6]  = x;
        out[7]  = x;
        out[8]  = x;
        out[9]  = x;
        out[10] = x;
        out[11] = x;
        out[12] = x;
        out[13] = x;
        out[14] = x;
        out[15] = x;
        out[16] = x;
        out[17] = x;
        out[18] = x;
        out[19] = x;
        out[20] = x;
        out[21] = x;
        out[22] = x;
        out[23] = x;
        out[24] = x;
        out[25] = x;
        out[26] = x;
        out[27] = x;
        out[28] = x;
        out[29] = x;
        out[30] = x;
        out[31] = x;
        out[32] = x;
        out[33] = x;
        out[34] = x;
        out[35] = x;
        out[36] = x;
        out[37] = x;
        out[38] = x;
        out[39] = x;
        out[40] = x;
        out[41] = x;
        out[42] = x;
        out[43] = x;
        out[44] = x;
        out[45] = x;
        out[46] = x;
        out[47] = x;
        out[48] = x;
        out[49] = x;
        out[50] = x;
        out[51] = x;
        out[52] = x;
        out[53] = x;
        out[54] = x;
        out[55] = x;
        out[56] = x;
        out[57] = x;
        out[58] = x;
        out[59] = x;
        out[60] = x;
        out[61] = x;
        out[62] = x;
        out[63] = x;
    }
}

static void idct64_low8_avx2(__m256i* in, __m256i* out, int32_t bit, int32_t do_cols, int32_t bd, int32_t out_shift) {
    const int32_t* cospi     = cospi_arr(bit);
    const __m256i  rnding    = _mm256_set1_epi32(1 << (bit - 1));
    const int32_t  log_range = AOMMAX(16, bd + (do_cols ? 6 : 8));
    const __m256i  clamp_lo  = _mm256_set1_epi32(-(1 << (log_range - 1)));
    const __m256i  clamp_hi  = _mm256_set1_epi32((1 << (log_range - 1)) - 1);

    const __m256i cospi1   = _mm256_set1_epi32(cospi[1]);
    const __m256i cospi2   = _mm256_set1_epi32(cospi[2]);
    const __m256i cospi3   = _mm256_set1_epi32(cospi[3]);
    const __m256i cospi4   = _mm256_set1_epi32(cospi[4]);
    const __m256i cospi6   = _mm256_set1_epi32(cospi[6]);
    const __m256i cospi8   = _mm256_set1_epi32(cospi[8]);
    const __m256i cospi12  = _mm256_set1_epi32(cospi[12]);
    const __m256i cospi16  = _mm256_set1_epi32(cospi[16]);
    const __m256i cospi20  = _mm256_set1_epi32(cospi[20]);
    const __m256i cospi24  = _mm256_set1_epi32(cospi[24]);
    const __m256i cospi28  = _mm256_set1_epi32(cospi[28]);
    const __m256i cospi32  = _mm256_set1_epi32(cospi[32]);
    const __m256i cospi40  = _mm256_set1_epi32(cospi[40]);
    const __m256i cospi44  = _mm256_set1_epi32(cospi[44]);
    const __m256i cospi48  = _mm256_set1_epi32(cospi[48]);
    const __m256i cospi56  = _mm256_set1_epi32(cospi[56]);
    const __m256i cospi60  = _mm256_set1_epi32(cospi[60]);
    const __m256i cospim4  = _mm256_set1_epi32(-cospi[4]);
    const __m256i cospim8  = _mm256_set1_epi32(-cospi[8]);
    const __m256i cospim12 = _mm256_set1_epi32(-cospi[12]);
    const __m256i cospim16 = _mm256_set1_epi32(-cospi[16]);
    const __m256i cospim20 = _mm256_set1_epi32(-cospi[20]);
    const __m256i cospim24 = _mm256_set1_epi32(-cospi[24]);
    const __m256i cospim28 = _mm256_set1_epi32(-cospi[28]);
    const __m256i cospim32 = _mm256_set1_epi32(-cospi[32]);
    const __m256i cospim36 = _mm256_set1_epi32(-cospi[36]);
    const __m256i cospim40 = _mm256_set1_epi32(-cospi[40]);
    const __m256i cospim48 = _mm256_set1_epi32(-cospi[48]);
    const __m256i cospim52 = _mm256_set1_epi32(-cospi[52]);
    const __m256i cospim56 = _mm256_set1_epi32(-cospi[56]);
    const __m256i cospi63  = _mm256_set1_epi32(cospi[63]);
    const __m256i cospim57 = _mm256_set1_epi32(-cospi[57]);
    const __m256i cospi7   = _mm256_set1_epi32(cospi[7]);
    const __m256i cospi5   = _mm256_set1_epi32(cospi[5]);
    const __m256i cospi59  = _mm256_set1_epi32(cospi[59]);
    const __m256i cospim61 = _mm256_set1_epi32(-cospi[61]);
    const __m256i cospim58 = _mm256_set1_epi32(-cospi[58]);
    const __m256i cospi62  = _mm256_set1_epi32(cospi[62]);

    {
        __m256i u[64];

        // stage 1
        u[0]  = in[0];
        u[8]  = in[4];
        u[16] = in[2];
        u[24] = in[6];
        u[32] = in[1];
        u[40] = in[5];
        u[48] = in[3];
        u[56] = in[7];

        // stage 2
        u[63] = half_btf_0_avx2(&cospi1, &u[32], &rnding, bit);
        u[32] = half_btf_0_avx2(&cospi63, &u[32], &rnding, bit);
        u[39] = half_btf_0_avx2(&cospim57, &u[56], &rnding, bit);
        u[56] = half_btf_0_avx2(&cospi7, &u[56], &rnding, bit);
        u[55] = half_btf_0_avx2(&cospi5, &u[40], &rnding, bit);
        u[40] = half_btf_0_avx2(&cospi59, &u[40], &rnding, bit);
        u[47] = half_btf_0_avx2(&cospim61, &u[48], &rnding, bit);
        u[48] = half_btf_0_avx2(&cospi3, &u[48], &rnding, bit);

        // stage 3
        u[31] = half_btf_0_avx2(&cospi2, &u[16], &rnding, bit);
        u[16] = half_btf_0_avx2(&cospi62, &u[16], &rnding, bit);
        u[23] = half_btf_0_avx2(&cospim58, &u[24], &rnding, bit);
        u[24] = half_btf_0_avx2(&cospi6, &u[24], &rnding, bit);
        u[33] = u[32];
        u[38] = u[39];
        u[41] = u[40];
        u[46] = u[47];
        u[49] = u[48];
        u[54] = u[55];
        u[57] = u[56];
        u[62] = u[63];

        // stage 4
        __m256i temp1, temp2;
        u[15] = half_btf_0_avx2(&cospi4, &u[8], &rnding, bit);
        u[8]  = half_btf_0_avx2(&cospi60, &u[8], &rnding, bit);
        u[17] = u[16];
        u[22] = u[23];
        u[25] = u[24];
        u[30] = u[31];

        temp1 = half_btf_avx2(&cospim4, &u[33], &cospi60, &u[62], &rnding, bit);
        u[62] = half_btf_avx2(&cospi60, &u[33], &cospi4, &u[62], &rnding, bit);
        u[33] = temp1;

        temp2 = half_btf_avx2(&cospim36, &u[38], &cospi28, &u[57], &rnding, bit);
        u[38] = half_btf_avx2(&cospim28, &u[38], &cospim36, &u[57], &rnding, bit);
        u[57] = temp2;

        temp1 = half_btf_avx2(&cospim20, &u[41], &cospi44, &u[54], &rnding, bit);
        u[54] = half_btf_avx2(&cospi44, &u[41], &cospi20, &u[54], &rnding, bit);
        u[41] = temp1;

        temp2 = half_btf_avx2(&cospim12, &u[46], &cospim52, &u[49], &rnding, bit);
        u[49] = half_btf_avx2(&cospim52, &u[46], &cospi12, &u[49], &rnding, bit);
        u[46] = temp2;

        // stage 5
        u[9]  = u[8];
        u[14] = u[15];

        temp1 = half_btf_avx2(&cospim8, &u[17], &cospi56, &u[30], &rnding, bit);
        u[30] = half_btf_avx2(&cospi56, &u[17], &cospi8, &u[30], &rnding, bit);
        u[17] = temp1;

        temp2 = half_btf_avx2(&cospim24, &u[22], &cospim40, &u[25], &rnding, bit);
        u[25] = half_btf_avx2(&cospim40, &u[22], &cospi24, &u[25], &rnding, bit);
        u[22] = temp2;

        u[35] = u[32];
        u[34] = u[33];
        u[36] = u[39];
        u[37] = u[38];
        u[43] = u[40];
        u[42] = u[41];
        u[44] = u[47];
        u[45] = u[46];
        u[51] = u[48];
        u[50] = u[49];
        u[52] = u[55];
        u[53] = u[54];
        u[59] = u[56];
        u[58] = u[57];
        u[60] = u[63];
        u[61] = u[62];

        // stage 6
        temp1 = half_btf_0_avx2(&cospi32, &u[0], &rnding, bit);
        u[1]  = half_btf_0_avx2(&cospi32, &u[0], &rnding, bit);
        u[0]  = temp1;

        temp2 = half_btf_avx2(&cospim16, &u[9], &cospi48, &u[14], &rnding, bit);
        u[14] = half_btf_avx2(&cospi48, &u[9], &cospi16, &u[14], &rnding, bit);
        u[9]  = temp2;
        u[19] = u[16];
        u[18] = u[17];
        u[20] = u[23];
        u[21] = u[22];
        u[27] = u[24];
        u[26] = u[25];
        u[28] = u[31];
        u[29] = u[30];

        temp1 = half_btf_avx2(&cospim8, &u[34], &cospi56, &u[61], &rnding, bit);
        u[61] = half_btf_avx2(&cospi56, &u[34], &cospi8, &u[61], &rnding, bit);
        u[34] = temp1;
        temp2 = half_btf_avx2(&cospim8, &u[35], &cospi56, &u[60], &rnding, bit);
        u[60] = half_btf_avx2(&cospi56, &u[35], &cospi8, &u[60], &rnding, bit);
        u[35] = temp2;
        temp1 = half_btf_avx2(&cospim56, &u[36], &cospim8, &u[59], &rnding, bit);
        u[59] = half_btf_avx2(&cospim8, &u[36], &cospi56, &u[59], &rnding, bit);
        u[36] = temp1;
        temp2 = half_btf_avx2(&cospim56, &u[37], &cospim8, &u[58], &rnding, bit);
        u[58] = half_btf_avx2(&cospim8, &u[37], &cospi56, &u[58], &rnding, bit);
        u[37] = temp2;
        temp1 = half_btf_avx2(&cospim40, &u[42], &cospi24, &u[53], &rnding, bit);
        u[53] = half_btf_avx2(&cospi24, &u[42], &cospi40, &u[53], &rnding, bit);
        u[42] = temp1;
        temp2 = half_btf_avx2(&cospim40, &u[43], &cospi24, &u[52], &rnding, bit);
        u[52] = half_btf_avx2(&cospi24, &u[43], &cospi40, &u[52], &rnding, bit);
        u[43] = temp2;
        temp1 = half_btf_avx2(&cospim24, &u[44], &cospim40, &u[51], &rnding, bit);
        u[51] = half_btf_avx2(&cospim40, &u[44], &cospi24, &u[51], &rnding, bit);
        u[44] = temp1;
        temp2 = half_btf_avx2(&cospim24, &u[45], &cospim40, &u[50], &rnding, bit);
        u[50] = half_btf_avx2(&cospim40, &u[45], &cospi24, &u[50], &rnding, bit);
        u[45] = temp2;

        // stage 7
        u[3]  = u[0];
        u[2]  = u[1];
        u[11] = u[8];
        u[10] = u[9];
        u[12] = u[15];
        u[13] = u[14];

        temp1 = half_btf_avx2(&cospim16, &u[18], &cospi48, &u[29], &rnding, bit);
        u[29] = half_btf_avx2(&cospi48, &u[18], &cospi16, &u[29], &rnding, bit);
        u[18] = temp1;
        temp2 = half_btf_avx2(&cospim16, &u[19], &cospi48, &u[28], &rnding, bit);
        u[28] = half_btf_avx2(&cospi48, &u[19], &cospi16, &u[28], &rnding, bit);
        u[19] = temp2;
        temp1 = half_btf_avx2(&cospim48, &u[20], &cospim16, &u[27], &rnding, bit);
        u[27] = half_btf_avx2(&cospim16, &u[20], &cospi48, &u[27], &rnding, bit);
        u[20] = temp1;
        temp2 = half_btf_avx2(&cospim48, &u[21], &cospim16, &u[26], &rnding, bit);
        u[26] = half_btf_avx2(&cospim16, &u[21], &cospi48, &u[26], &rnding, bit);
        u[21] = temp2;
        for (unsigned i = 32; i < 64; i += 16) {
            for (unsigned j = i; j < i + 4; j++) {
                addsub_avx2(u[j], u[j ^ 7], &u[j], &u[j ^ 7], &clamp_lo, &clamp_hi);
                addsub_avx2(u[j ^ 15], u[j ^ 8], &u[j ^ 15], &u[j ^ 8], &clamp_lo, &clamp_hi);
            }
        }

        // stage 8
        u[7] = u[0];
        u[6] = u[1];
        u[5] = u[2];
        u[4] = u[3];

        idct64_stage8_avx2(
            u, &cospim32, &cospi32, &cospim16, &cospi48, &cospi16, &cospim48, &clamp_lo, &clamp_hi, &rnding, bit);

        // stage 9
        idct64_stage9_avx2(u, &cospim32, &cospi32, &clamp_lo, &clamp_hi, &rnding, bit);

        // stage 10
        idct64_stage10_avx2(u, &cospim32, &cospi32, &clamp_lo, &clamp_hi, &rnding, bit);

        // stage 11
        idct64_stage11_avx2(u, out, do_cols, bd, out_shift, &clamp_lo, &clamp_hi);
    }
}

static void idct64_low16_avx2(__m256i* in, __m256i* out, int32_t bit, int32_t do_cols, int32_t bd, int32_t out_shift) {
    const int32_t* cospi     = cospi_arr(bit);
    const __m256i  rnding    = _mm256_set1_epi32(1 << (bit - 1));
    const int32_t  log_range = AOMMAX(16, bd + (do_cols ? 6 : 8));
    const __m256i  clamp_lo  = _mm256_set1_epi32(-(1 << (log_range - 1)));
    const __m256i  clamp_hi  = _mm256_set1_epi32((1 << (log_range - 1)) - 1);

    const __m256i cospi1  = _mm256_set1_epi32(cospi[1]);
    const __m256i cospi2  = _mm256_set1_epi32(cospi[2]);
    const __m256i cospi3  = _mm256_set1_epi32(cospi[3]);
    const __m256i cospi4  = _mm256_set1_epi32(cospi[4]);
    const __m256i cospi5  = _mm256_set1_epi32(cospi[5]);
    const __m256i cospi6  = _mm256_set1_epi32(cospi[6]);
    const __m256i cospi7  = _mm256_set1_epi32(cospi[7]);
    const __m256i cospi8  = _mm256_set1_epi32(cospi[8]);
    const __m256i cospi9  = _mm256_set1_epi32(cospi[9]);
    const __m256i cospi10 = _mm256_set1_epi32(cospi[10]);
    const __m256i cospi11 = _mm256_set1_epi32(cospi[11]);
    const __m256i cospi12 = _mm256_set1_epi32(cospi[12]);
    const __m256i cospi13 = _mm256_set1_epi32(cospi[13]);
    const __m256i cospi14 = _mm256_set1_epi32(cospi[14]);
    const __m256i cospi15 = _mm256_set1_epi32(cospi[15]);
    const __m256i cospi16 = _mm256_set1_epi32(cospi[16]);
    const __m256i cospi20 = _mm256_set1_epi32(cospi[20]);
    const __m256i cospi24 = _mm256_set1_epi32(cospi[24]);
    const __m256i cospi28 = _mm256_set1_epi32(cospi[28]);
    const __m256i cospi32 = _mm256_set1_epi32(cospi[32]);
    const __m256i cospi36 = _mm256_set1_epi32(cospi[36]);
    const __m256i cospi40 = _mm256_set1_epi32(cospi[40]);
    const __m256i cospi44 = _mm256_set1_epi32(cospi[44]);
    const __m256i cospi48 = _mm256_set1_epi32(cospi[48]);
    const __m256i cospi51 = _mm256_set1_epi32(cospi[51]);
    const __m256i cospi52 = _mm256_set1_epi32(cospi[52]);
    const __m256i cospi54 = _mm256_set1_epi32(cospi[54]);
    const __m256i cospi55 = _mm256_set1_epi32(cospi[55]);
    const __m256i cospi56 = _mm256_set1_epi32(cospi[56]);
    const __m256i cospi59 = _mm256_set1_epi32(cospi[59]);
    const __m256i cospi60 = _mm256_set1_epi32(cospi[60]);
    const __m256i cospi62 = _mm256_set1_epi32(cospi[62]);
    const __m256i cospi63 = _mm256_set1_epi32(cospi[63]);

    const __m256i cospim4  = _mm256_set1_epi32(-cospi[4]);
    const __m256i cospim8  = _mm256_set1_epi32(-cospi[8]);
    const __m256i cospim12 = _mm256_set1_epi32(-cospi[12]);
    const __m256i cospim16 = _mm256_set1_epi32(-cospi[16]);
    const __m256i cospim20 = _mm256_set1_epi32(-cospi[20]);
    const __m256i cospim24 = _mm256_set1_epi32(-cospi[24]);
    const __m256i cospim28 = _mm256_set1_epi32(-cospi[28]);
    const __m256i cospim32 = _mm256_set1_epi32(-cospi[32]);
    const __m256i cospim36 = _mm256_set1_epi32(-cospi[36]);
    const __m256i cospim40 = _mm256_set1_epi32(-cospi[40]);
    const __m256i cospim44 = _mm256_set1_epi32(-cospi[44]);
    const __m256i cospim48 = _mm256_set1_epi32(-cospi[48]);
    const __m256i cospim49 = _mm256_set1_epi32(-cospi[49]);
    const __m256i cospim50 = _mm256_set1_epi32(-cospi[50]);
    const __m256i cospim52 = _mm256_set1_epi32(-cospi[52]);
    const __m256i cospim53 = _mm256_set1_epi32(-cospi[53]);
    const __m256i cospim56 = _mm256_set1_epi32(-cospi[56]);
    const __m256i cospim57 = _mm256_set1_epi32(-cospi[57]);
    const __m256i cospim58 = _mm256_set1_epi32(-cospi[58]);
    const __m256i cospim60 = _mm256_set1_epi32(-cospi[60]);
    const __m256i cospim61 = _mm256_set1_epi32(-cospi[61]);

    {
        __m256i u[64];
        __m256i tmp1, tmp2, tmp3, tmp4;
        // stage 1
        u[0]  = in[0];
        u[32] = in[1];
        u[36] = in[9];
        u[40] = in[5];
        u[44] = in[13];
        u[48] = in[3];
        u[52] = in[11];
        u[56] = in[7];
        u[60] = in[15];
        u[16] = in[2];
        u[20] = in[10];
        u[24] = in[6];
        u[28] = in[14];
        u[4]  = in[8];
        u[8]  = in[4];
        u[12] = in[12];

        // stage 2
        u[63] = half_btf_0_avx2(&cospi1, &u[32], &rnding, bit);
        u[32] = half_btf_0_avx2(&cospi63, &u[32], &rnding, bit);
        u[35] = half_btf_0_avx2(&cospim49, &u[60], &rnding, bit);
        u[60] = half_btf_0_avx2(&cospi15, &u[60], &rnding, bit);
        u[59] = half_btf_0_avx2(&cospi9, &u[36], &rnding, bit);
        u[36] = half_btf_0_avx2(&cospi55, &u[36], &rnding, bit);
        u[39] = half_btf_0_avx2(&cospim57, &u[56], &rnding, bit);
        u[56] = half_btf_0_avx2(&cospi7, &u[56], &rnding, bit);
        u[55] = half_btf_0_avx2(&cospi5, &u[40], &rnding, bit);
        u[40] = half_btf_0_avx2(&cospi59, &u[40], &rnding, bit);
        u[43] = half_btf_0_avx2(&cospim53, &u[52], &rnding, bit);
        u[52] = half_btf_0_avx2(&cospi11, &u[52], &rnding, bit);
        u[47] = half_btf_0_avx2(&cospim61, &u[48], &rnding, bit);
        u[48] = half_btf_0_avx2(&cospi3, &u[48], &rnding, bit);
        u[51] = half_btf_0_avx2(&cospi13, &u[44], &rnding, bit);
        u[44] = half_btf_0_avx2(&cospi51, &u[44], &rnding, bit);

        // stage 3
        u[31] = half_btf_0_avx2(&cospi2, &u[16], &rnding, bit);
        u[16] = half_btf_0_avx2(&cospi62, &u[16], &rnding, bit);
        u[19] = half_btf_0_avx2(&cospim50, &u[28], &rnding, bit);
        u[28] = half_btf_0_avx2(&cospi14, &u[28], &rnding, bit);
        u[27] = half_btf_0_avx2(&cospi10, &u[20], &rnding, bit);
        u[20] = half_btf_0_avx2(&cospi54, &u[20], &rnding, bit);
        u[23] = half_btf_0_avx2(&cospim58, &u[24], &rnding, bit);
        u[24] = half_btf_0_avx2(&cospi6, &u[24], &rnding, bit);
        u[33] = u[32];
        u[34] = u[35];
        u[37] = u[36];
        u[38] = u[39];
        u[41] = u[40];
        u[42] = u[43];
        u[45] = u[44];
        u[46] = u[47];
        u[49] = u[48];
        u[50] = u[51];
        u[53] = u[52];
        u[54] = u[55];
        u[57] = u[56];
        u[58] = u[59];
        u[61] = u[60];
        u[62] = u[63];

        // stage 4
        u[15] = half_btf_0_avx2(&cospi4, &u[8], &rnding, bit);
        u[8]  = half_btf_0_avx2(&cospi60, &u[8], &rnding, bit);
        u[11] = half_btf_0_avx2(&cospim52, &u[12], &rnding, bit);
        u[12] = half_btf_0_avx2(&cospi12, &u[12], &rnding, bit);

        u[17] = u[16];
        u[18] = u[19];
        u[21] = u[20];
        u[22] = u[23];
        u[25] = u[24];
        u[26] = u[27];
        u[29] = u[28];
        u[30] = u[31];

        tmp1  = half_btf_avx2(&cospim4, &u[33], &cospi60, &u[62], &rnding, bit);
        tmp2  = half_btf_avx2(&cospim60, &u[34], &cospim4, &u[61], &rnding, bit);
        tmp3  = half_btf_avx2(&cospim36, &u[37], &cospi28, &u[58], &rnding, bit);
        tmp4  = half_btf_avx2(&cospim28, &u[38], &cospim36, &u[57], &rnding, bit);
        u[57] = half_btf_avx2(&cospim36, &u[38], &cospi28, &u[57], &rnding, bit);
        u[58] = half_btf_avx2(&cospi28, &u[37], &cospi36, &u[58], &rnding, bit);
        u[61] = half_btf_avx2(&cospim4, &u[34], &cospi60, &u[61], &rnding, bit);
        u[62] = half_btf_avx2(&cospi60, &u[33], &cospi4, &u[62], &rnding, bit);
        u[33] = tmp1;
        u[34] = tmp2;
        u[37] = tmp3;
        u[38] = tmp4;

        tmp1  = half_btf_avx2(&cospim20, &u[41], &cospi44, &u[54], &rnding, bit);
        tmp2  = half_btf_avx2(&cospim44, &u[42], &cospim20, &u[53], &rnding, bit);
        tmp3  = half_btf_avx2(&cospim52, &u[45], &cospi12, &u[50], &rnding, bit);
        tmp4  = half_btf_avx2(&cospim12, &u[46], &cospim52, &u[49], &rnding, bit);
        u[49] = half_btf_avx2(&cospim52, &u[46], &cospi12, &u[49], &rnding, bit);
        u[50] = half_btf_avx2(&cospi12, &u[45], &cospi52, &u[50], &rnding, bit);
        u[53] = half_btf_avx2(&cospim20, &u[42], &cospi44, &u[53], &rnding, bit);
        u[54] = half_btf_avx2(&cospi44, &u[41], &cospi20, &u[54], &rnding, bit);
        u[41] = tmp1;
        u[42] = tmp2;
        u[45] = tmp3;
        u[46] = tmp4;

        // stage 5
        u[7] = half_btf_0_avx2(&cospi8, &u[4], &rnding, bit);
        u[4] = half_btf_0_avx2(&cospi56, &u[4], &rnding, bit);

        u[9]  = u[8];
        u[10] = u[11];
        u[13] = u[12];
        u[14] = u[15];

        tmp1  = half_btf_avx2(&cospim8, &u[17], &cospi56, &u[30], &rnding, bit);
        tmp2  = half_btf_avx2(&cospim56, &u[18], &cospim8, &u[29], &rnding, bit);
        tmp3  = half_btf_avx2(&cospim40, &u[21], &cospi24, &u[26], &rnding, bit);
        tmp4  = half_btf_avx2(&cospim24, &u[22], &cospim40, &u[25], &rnding, bit);
        u[25] = half_btf_avx2(&cospim40, &u[22], &cospi24, &u[25], &rnding, bit);
        u[26] = half_btf_avx2(&cospi24, &u[21], &cospi40, &u[26], &rnding, bit);
        u[29] = half_btf_avx2(&cospim8, &u[18], &cospi56, &u[29], &rnding, bit);
        u[30] = half_btf_avx2(&cospi56, &u[17], &cospi8, &u[30], &rnding, bit);
        u[17] = tmp1;
        u[18] = tmp2;
        u[21] = tmp3;
        u[22] = tmp4;

        for (unsigned i = 32; i < 64; i += 8) {
            addsub_avx2(u[i + 0], u[i + 3], &u[i + 0], &u[i + 3], &clamp_lo, &clamp_hi);
            addsub_avx2(u[i + 1], u[i + 2], &u[i + 1], &u[i + 2], &clamp_lo, &clamp_hi);

            addsub_avx2(u[i + 7], u[i + 4], &u[i + 7], &u[i + 4], &clamp_lo, &clamp_hi);
            addsub_avx2(u[i + 6], u[i + 5], &u[i + 6], &u[i + 5], &clamp_lo, &clamp_hi);
        }

        // stage 6
        tmp1 = half_btf_0_avx2(&cospi32, &u[0], &rnding, bit);
        u[1] = half_btf_0_avx2(&cospi32, &u[0], &rnding, bit);
        u[0] = tmp1;
        u[5] = u[4];
        u[6] = u[7];

        tmp1  = half_btf_avx2(&cospim16, &u[9], &cospi48, &u[14], &rnding, bit);
        u[14] = half_btf_avx2(&cospi48, &u[9], &cospi16, &u[14], &rnding, bit);
        u[9]  = tmp1;
        tmp2  = half_btf_avx2(&cospim48, &u[10], &cospim16, &u[13], &rnding, bit);
        u[13] = half_btf_avx2(&cospim16, &u[10], &cospi48, &u[13], &rnding, bit);
        u[10] = tmp2;

        for (unsigned i = 16; i < 32; i += 8) {
            addsub_avx2(u[i + 0], u[i + 3], &u[i + 0], &u[i + 3], &clamp_lo, &clamp_hi);
            addsub_avx2(u[i + 1], u[i + 2], &u[i + 1], &u[i + 2], &clamp_lo, &clamp_hi);

            addsub_avx2(u[i + 7], u[i + 4], &u[i + 7], &u[i + 4], &clamp_lo, &clamp_hi);
            addsub_avx2(u[i + 6], u[i + 5], &u[i + 6], &u[i + 5], &clamp_lo, &clamp_hi);
        }

        tmp1  = half_btf_avx2(&cospim8, &u[34], &cospi56, &u[61], &rnding, bit);
        tmp2  = half_btf_avx2(&cospim8, &u[35], &cospi56, &u[60], &rnding, bit);
        tmp3  = half_btf_avx2(&cospim56, &u[36], &cospim8, &u[59], &rnding, bit);
        tmp4  = half_btf_avx2(&cospim56, &u[37], &cospim8, &u[58], &rnding, bit);
        u[58] = half_btf_avx2(&cospim8, &u[37], &cospi56, &u[58], &rnding, bit);
        u[59] = half_btf_avx2(&cospim8, &u[36], &cospi56, &u[59], &rnding, bit);
        u[60] = half_btf_avx2(&cospi56, &u[35], &cospi8, &u[60], &rnding, bit);
        u[61] = half_btf_avx2(&cospi56, &u[34], &cospi8, &u[61], &rnding, bit);
        u[34] = tmp1;
        u[35] = tmp2;
        u[36] = tmp3;
        u[37] = tmp4;

        tmp1  = half_btf_avx2(&cospim40, &u[42], &cospi24, &u[53], &rnding, bit);
        tmp2  = half_btf_avx2(&cospim40, &u[43], &cospi24, &u[52], &rnding, bit);
        tmp3  = half_btf_avx2(&cospim24, &u[44], &cospim40, &u[51], &rnding, bit);
        tmp4  = half_btf_avx2(&cospim24, &u[45], &cospim40, &u[50], &rnding, bit);
        u[50] = half_btf_avx2(&cospim40, &u[45], &cospi24, &u[50], &rnding, bit);
        u[51] = half_btf_avx2(&cospim40, &u[44], &cospi24, &u[51], &rnding, bit);
        u[52] = half_btf_avx2(&cospi24, &u[43], &cospi40, &u[52], &rnding, bit);
        u[53] = half_btf_avx2(&cospi24, &u[42], &cospi40, &u[53], &rnding, bit);
        u[42] = tmp1;
        u[43] = tmp2;
        u[44] = tmp3;
        u[45] = tmp4;

        // stage 7
        u[3] = u[0];
        u[2] = u[1];
        tmp1 = half_btf_avx2(&cospim32, &u[5], &cospi32, &u[6], &rnding, bit);
        u[6] = half_btf_avx2(&cospi32, &u[5], &cospi32, &u[6], &rnding, bit);
        u[5] = tmp1;
        addsub_avx2(u[8], u[11], &u[8], &u[11], &clamp_lo, &clamp_hi);
        addsub_avx2(u[9], u[10], &u[9], &u[10], &clamp_lo, &clamp_hi);
        addsub_avx2(u[15], u[12], &u[15], &u[12], &clamp_lo, &clamp_hi);
        addsub_avx2(u[14], u[13], &u[14], &u[13], &clamp_lo, &clamp_hi);

        tmp1  = half_btf_avx2(&cospim16, &u[18], &cospi48, &u[29], &rnding, bit);
        tmp2  = half_btf_avx2(&cospim16, &u[19], &cospi48, &u[28], &rnding, bit);
        tmp3  = half_btf_avx2(&cospim48, &u[20], &cospim16, &u[27], &rnding, bit);
        tmp4  = half_btf_avx2(&cospim48, &u[21], &cospim16, &u[26], &rnding, bit);
        u[26] = half_btf_avx2(&cospim16, &u[21], &cospi48, &u[26], &rnding, bit);
        u[27] = half_btf_avx2(&cospim16, &u[20], &cospi48, &u[27], &rnding, bit);
        u[28] = half_btf_avx2(&cospi48, &u[19], &cospi16, &u[28], &rnding, bit);
        u[29] = half_btf_avx2(&cospi48, &u[18], &cospi16, &u[29], &rnding, bit);
        u[18] = tmp1;
        u[19] = tmp2;
        u[20] = tmp3;
        u[21] = tmp4;

        for (unsigned i = 32; i < 64; i += 16) {
            for (unsigned j = i; j < i + 4; j++) {
                addsub_avx2(u[j], u[j ^ 7], &u[j], &u[j ^ 7], &clamp_lo, &clamp_hi);
                addsub_avx2(u[j ^ 15], u[j ^ 8], &u[j ^ 15], &u[j ^ 8], &clamp_lo, &clamp_hi);
            }
        }

        // stage 8
        for (unsigned i = 0; i < 4; ++i) {
            addsub_avx2(u[i], u[7 - i], &u[i], &u[7 - i], &clamp_lo, &clamp_hi);
        }
        idct64_stage8_avx2(
            u, &cospim32, &cospi32, &cospim16, &cospi48, &cospi16, &cospim48, &clamp_lo, &clamp_hi, &rnding, bit);

        // stage 9
        idct64_stage9_avx2(u, &cospim32, &cospi32, &clamp_lo, &clamp_hi, &rnding, bit);

        // stage 10
        idct64_stage10_avx2(u, &cospim32, &cospi32, &clamp_lo, &clamp_hi, &rnding, bit);

        // stage 11
        idct64_stage11_avx2(u, out, do_cols, bd, out_shift, &clamp_lo, &clamp_hi);
    }
}

static void idct64_avx2(__m256i* in, __m256i* out, int32_t bit, int32_t do_cols, int32_t bd, int32_t out_shift) {
    const int32_t* cospi     = cospi_arr(bit);
    const __m256i  rnding    = _mm256_set1_epi32(1 << (bit - 1));
    const int32_t  log_range = AOMMAX(16, bd + (do_cols ? 6 : 8));
    const __m256i  clamp_lo  = _mm256_set1_epi32(-(1 << (log_range - 1)));
    const __m256i  clamp_hi  = _mm256_set1_epi32((1 << (log_range - 1)) - 1);

    const __m256i cospi1  = _mm256_set1_epi32(cospi[1]);
    const __m256i cospi2  = _mm256_set1_epi32(cospi[2]);
    const __m256i cospi3  = _mm256_set1_epi32(cospi[3]);
    const __m256i cospi4  = _mm256_set1_epi32(cospi[4]);
    const __m256i cospi5  = _mm256_set1_epi32(cospi[5]);
    const __m256i cospi6  = _mm256_set1_epi32(cospi[6]);
    const __m256i cospi7  = _mm256_set1_epi32(cospi[7]);
    const __m256i cospi8  = _mm256_set1_epi32(cospi[8]);
    const __m256i cospi9  = _mm256_set1_epi32(cospi[9]);
    const __m256i cospi10 = _mm256_set1_epi32(cospi[10]);
    const __m256i cospi11 = _mm256_set1_epi32(cospi[11]);
    const __m256i cospi12 = _mm256_set1_epi32(cospi[12]);
    const __m256i cospi13 = _mm256_set1_epi32(cospi[13]);
    const __m256i cospi14 = _mm256_set1_epi32(cospi[14]);
    const __m256i cospi15 = _mm256_set1_epi32(cospi[15]);
    const __m256i cospi16 = _mm256_set1_epi32(cospi[16]);
    const __m256i cospi17 = _mm256_set1_epi32(cospi[17]);
    const __m256i cospi18 = _mm256_set1_epi32(cospi[18]);
    const __m256i cospi19 = _mm256_set1_epi32(cospi[19]);
    const __m256i cospi20 = _mm256_set1_epi32(cospi[20]);
    const __m256i cospi21 = _mm256_set1_epi32(cospi[21]);
    const __m256i cospi22 = _mm256_set1_epi32(cospi[22]);
    const __m256i cospi23 = _mm256_set1_epi32(cospi[23]);
    const __m256i cospi24 = _mm256_set1_epi32(cospi[24]);
    const __m256i cospi25 = _mm256_set1_epi32(cospi[25]);
    const __m256i cospi26 = _mm256_set1_epi32(cospi[26]);
    const __m256i cospi27 = _mm256_set1_epi32(cospi[27]);
    const __m256i cospi28 = _mm256_set1_epi32(cospi[28]);
    const __m256i cospi29 = _mm256_set1_epi32(cospi[29]);
    const __m256i cospi30 = _mm256_set1_epi32(cospi[30]);
    const __m256i cospi31 = _mm256_set1_epi32(cospi[31]);
    const __m256i cospi32 = _mm256_set1_epi32(cospi[32]);
    const __m256i cospi35 = _mm256_set1_epi32(cospi[35]);
    const __m256i cospi36 = _mm256_set1_epi32(cospi[36]);
    const __m256i cospi38 = _mm256_set1_epi32(cospi[38]);
    const __m256i cospi39 = _mm256_set1_epi32(cospi[39]);
    const __m256i cospi40 = _mm256_set1_epi32(cospi[40]);
    const __m256i cospi43 = _mm256_set1_epi32(cospi[43]);
    const __m256i cospi44 = _mm256_set1_epi32(cospi[44]);
    const __m256i cospi46 = _mm256_set1_epi32(cospi[46]);
    const __m256i cospi47 = _mm256_set1_epi32(cospi[47]);
    const __m256i cospi48 = _mm256_set1_epi32(cospi[48]);
    const __m256i cospi51 = _mm256_set1_epi32(cospi[51]);
    const __m256i cospi52 = _mm256_set1_epi32(cospi[52]);
    const __m256i cospi54 = _mm256_set1_epi32(cospi[54]);
    const __m256i cospi55 = _mm256_set1_epi32(cospi[55]);
    const __m256i cospi56 = _mm256_set1_epi32(cospi[56]);
    const __m256i cospi59 = _mm256_set1_epi32(cospi[59]);
    const __m256i cospi60 = _mm256_set1_epi32(cospi[60]);
    const __m256i cospi62 = _mm256_set1_epi32(cospi[62]);
    const __m256i cospi63 = _mm256_set1_epi32(cospi[63]);

    const __m256i cospim4  = _mm256_set1_epi32(-cospi[4]);
    const __m256i cospim8  = _mm256_set1_epi32(-cospi[8]);
    const __m256i cospim12 = _mm256_set1_epi32(-cospi[12]);
    const __m256i cospim16 = _mm256_set1_epi32(-cospi[16]);
    const __m256i cospim20 = _mm256_set1_epi32(-cospi[20]);
    const __m256i cospim24 = _mm256_set1_epi32(-cospi[24]);
    const __m256i cospim28 = _mm256_set1_epi32(-cospi[28]);
    const __m256i cospim32 = _mm256_set1_epi32(-cospi[32]);
    const __m256i cospim33 = _mm256_set1_epi32(-cospi[33]);
    const __m256i cospim34 = _mm256_set1_epi32(-cospi[34]);
    const __m256i cospim36 = _mm256_set1_epi32(-cospi[36]);
    const __m256i cospim37 = _mm256_set1_epi32(-cospi[37]);
    const __m256i cospim40 = _mm256_set1_epi32(-cospi[40]);
    const __m256i cospim41 = _mm256_set1_epi32(-cospi[41]);
    const __m256i cospim42 = _mm256_set1_epi32(-cospi[42]);
    const __m256i cospim44 = _mm256_set1_epi32(-cospi[44]);
    const __m256i cospim45 = _mm256_set1_epi32(-cospi[45]);
    const __m256i cospim48 = _mm256_set1_epi32(-cospi[48]);
    const __m256i cospim49 = _mm256_set1_epi32(-cospi[49]);
    const __m256i cospim50 = _mm256_set1_epi32(-cospi[50]);
    const __m256i cospim52 = _mm256_set1_epi32(-cospi[52]);
    const __m256i cospim53 = _mm256_set1_epi32(-cospi[53]);
    const __m256i cospim56 = _mm256_set1_epi32(-cospi[56]);
    const __m256i cospim57 = _mm256_set1_epi32(-cospi[57]);
    const __m256i cospim58 = _mm256_set1_epi32(-cospi[58]);
    const __m256i cospim60 = _mm256_set1_epi32(-cospi[60]);
    const __m256i cospim61 = _mm256_set1_epi32(-cospi[61]);

    {
        __m256i u[64], v[64];

        // stage 1
        u[32] = in[1];
        u[34] = in[17];
        u[36] = in[9];
        u[38] = in[25];
        u[40] = in[5];
        u[42] = in[21];
        u[44] = in[13];
        u[46] = in[29];
        u[48] = in[3];
        u[50] = in[19];
        u[52] = in[11];
        u[54] = in[27];
        u[56] = in[7];
        u[58] = in[23];
        u[60] = in[15];
        u[62] = in[31];

        v[16] = in[2];
        v[18] = in[18];
        v[20] = in[10];
        v[22] = in[26];
        v[24] = in[6];
        v[26] = in[22];
        v[28] = in[14];
        v[30] = in[30];

        u[8]  = in[4];
        u[10] = in[20];
        u[12] = in[12];
        u[14] = in[28];

        v[4] = in[8];
        v[6] = in[24];

        u[0] = in[0];
        u[2] = in[16];

        // stage 2
        v[32] = half_btf_0_avx2(&cospi63, &u[32], &rnding, bit);
        v[33] = half_btf_0_avx2(&cospim33, &u[62], &rnding, bit);
        v[34] = half_btf_0_avx2(&cospi47, &u[34], &rnding, bit);
        v[35] = half_btf_0_avx2(&cospim49, &u[60], &rnding, bit);
        v[36] = half_btf_0_avx2(&cospi55, &u[36], &rnding, bit);
        v[37] = half_btf_0_avx2(&cospim41, &u[58], &rnding, bit);
        v[38] = half_btf_0_avx2(&cospi39, &u[38], &rnding, bit);
        v[39] = half_btf_0_avx2(&cospim57, &u[56], &rnding, bit);
        v[40] = half_btf_0_avx2(&cospi59, &u[40], &rnding, bit);
        v[41] = half_btf_0_avx2(&cospim37, &u[54], &rnding, bit);
        v[42] = half_btf_0_avx2(&cospi43, &u[42], &rnding, bit);
        v[43] = half_btf_0_avx2(&cospim53, &u[52], &rnding, bit);
        v[44] = half_btf_0_avx2(&cospi51, &u[44], &rnding, bit);
        v[45] = half_btf_0_avx2(&cospim45, &u[50], &rnding, bit);
        v[46] = half_btf_0_avx2(&cospi35, &u[46], &rnding, bit);
        v[47] = half_btf_0_avx2(&cospim61, &u[48], &rnding, bit);
        v[48] = half_btf_0_avx2(&cospi3, &u[48], &rnding, bit);
        v[49] = half_btf_0_avx2(&cospi29, &u[46], &rnding, bit);
        v[50] = half_btf_0_avx2(&cospi19, &u[50], &rnding, bit);
        v[51] = half_btf_0_avx2(&cospi13, &u[44], &rnding, bit);
        v[52] = half_btf_0_avx2(&cospi11, &u[52], &rnding, bit);
        v[53] = half_btf_0_avx2(&cospi21, &u[42], &rnding, bit);
        v[54] = half_btf_0_avx2(&cospi27, &u[54], &rnding, bit);
        v[55] = half_btf_0_avx2(&cospi5, &u[40], &rnding, bit);
        v[56] = half_btf_0_avx2(&cospi7, &u[56], &rnding, bit);
        v[57] = half_btf_0_avx2(&cospi25, &u[38], &rnding, bit);
        v[58] = half_btf_0_avx2(&cospi23, &u[58], &rnding, bit);
        v[59] = half_btf_0_avx2(&cospi9, &u[36], &rnding, bit);
        v[60] = half_btf_0_avx2(&cospi15, &u[60], &rnding, bit);
        v[61] = half_btf_0_avx2(&cospi17, &u[34], &rnding, bit);
        v[62] = half_btf_0_avx2(&cospi31, &u[62], &rnding, bit);
        v[63] = half_btf_0_avx2(&cospi1, &u[32], &rnding, bit);

        // stage 3
        u[16] = half_btf_0_avx2(&cospi62, &v[16], &rnding, bit);
        u[17] = half_btf_0_avx2(&cospim34, &v[30], &rnding, bit);
        u[18] = half_btf_0_avx2(&cospi46, &v[18], &rnding, bit);
        u[19] = half_btf_0_avx2(&cospim50, &v[28], &rnding, bit);
        u[20] = half_btf_0_avx2(&cospi54, &v[20], &rnding, bit);
        u[21] = half_btf_0_avx2(&cospim42, &v[26], &rnding, bit);
        u[22] = half_btf_0_avx2(&cospi38, &v[22], &rnding, bit);
        u[23] = half_btf_0_avx2(&cospim58, &v[24], &rnding, bit);
        u[24] = half_btf_0_avx2(&cospi6, &v[24], &rnding, bit);
        u[25] = half_btf_0_avx2(&cospi26, &v[22], &rnding, bit);
        u[26] = half_btf_0_avx2(&cospi22, &v[26], &rnding, bit);
        u[27] = half_btf_0_avx2(&cospi10, &v[20], &rnding, bit);
        u[28] = half_btf_0_avx2(&cospi14, &v[28], &rnding, bit);
        u[29] = half_btf_0_avx2(&cospi18, &v[18], &rnding, bit);
        u[30] = half_btf_0_avx2(&cospi30, &v[30], &rnding, bit);
        u[31] = half_btf_0_avx2(&cospi2, &v[16], &rnding, bit);

        for (unsigned i = 32; i < 64; i += 4) {
            addsub_avx2(v[i + 0], v[i + 1], &u[i + 0], &u[i + 1], &clamp_lo, &clamp_hi);
            addsub_avx2(v[i + 3], v[i + 2], &u[i + 3], &u[i + 2], &clamp_lo, &clamp_hi);
        }

        // stage 4
        v[8]  = half_btf_0_avx2(&cospi60, &u[8], &rnding, bit);
        v[9]  = half_btf_0_avx2(&cospim36, &u[14], &rnding, bit);
        v[10] = half_btf_0_avx2(&cospi44, &u[10], &rnding, bit);
        v[11] = half_btf_0_avx2(&cospim52, &u[12], &rnding, bit);
        v[12] = half_btf_0_avx2(&cospi12, &u[12], &rnding, bit);
        v[13] = half_btf_0_avx2(&cospi20, &u[10], &rnding, bit);
        v[14] = half_btf_0_avx2(&cospi28, &u[14], &rnding, bit);
        v[15] = half_btf_0_avx2(&cospi4, &u[8], &rnding, bit);

        for (unsigned i = 16; i < 32; i += 4) {
            addsub_avx2(u[i + 0], u[i + 1], &v[i + 0], &v[i + 1], &clamp_lo, &clamp_hi);
            addsub_avx2(u[i + 3], u[i + 2], &v[i + 3], &v[i + 2], &clamp_lo, &clamp_hi);
        }

        for (unsigned i = 32; i < 64; i += 4) {
            v[i + 0] = u[i + 0];
            v[i + 3] = u[i + 3];
        }

        v[33] = half_btf_avx2(&cospim4, &u[33], &cospi60, &u[62], &rnding, bit);
        v[34] = half_btf_avx2(&cospim60, &u[34], &cospim4, &u[61], &rnding, bit);
        v[37] = half_btf_avx2(&cospim36, &u[37], &cospi28, &u[58], &rnding, bit);
        v[38] = half_btf_avx2(&cospim28, &u[38], &cospim36, &u[57], &rnding, bit);
        v[41] = half_btf_avx2(&cospim20, &u[41], &cospi44, &u[54], &rnding, bit);
        v[42] = half_btf_avx2(&cospim44, &u[42], &cospim20, &u[53], &rnding, bit);
        v[45] = half_btf_avx2(&cospim52, &u[45], &cospi12, &u[50], &rnding, bit);
        v[46] = half_btf_avx2(&cospim12, &u[46], &cospim52, &u[49], &rnding, bit);
        v[49] = half_btf_avx2(&cospim52, &u[46], &cospi12, &u[49], &rnding, bit);
        v[50] = half_btf_avx2(&cospi12, &u[45], &cospi52, &u[50], &rnding, bit);
        v[53] = half_btf_avx2(&cospim20, &u[42], &cospi44, &u[53], &rnding, bit);
        v[54] = half_btf_avx2(&cospi44, &u[41], &cospi20, &u[54], &rnding, bit);
        v[57] = half_btf_avx2(&cospim36, &u[38], &cospi28, &u[57], &rnding, bit);
        v[58] = half_btf_avx2(&cospi28, &u[37], &cospi36, &u[58], &rnding, bit);
        v[61] = half_btf_avx2(&cospim4, &u[34], &cospi60, &u[61], &rnding, bit);
        v[62] = half_btf_avx2(&cospi60, &u[33], &cospi4, &u[62], &rnding, bit);

        // stage 5
        u[4] = half_btf_0_avx2(&cospi56, &v[4], &rnding, bit);
        u[5] = half_btf_0_avx2(&cospim40, &v[6], &rnding, bit);
        u[6] = half_btf_0_avx2(&cospi24, &v[6], &rnding, bit);
        u[7] = half_btf_0_avx2(&cospi8, &v[4], &rnding, bit);

        for (unsigned i = 8; i < 16; i += 4) {
            addsub_avx2(v[i + 0], v[i + 1], &u[i + 0], &u[i + 1], &clamp_lo, &clamp_hi);
            addsub_avx2(v[i + 3], v[i + 2], &u[i + 3], &u[i + 2], &clamp_lo, &clamp_hi);
        }

        for (unsigned i = 16; i < 32; i += 4) {
            u[i + 0] = v[i + 0];
            u[i + 3] = v[i + 3];
        }

        u[17] = half_btf_avx2(&cospim8, &v[17], &cospi56, &v[30], &rnding, bit);
        u[18] = half_btf_avx2(&cospim56, &v[18], &cospim8, &v[29], &rnding, bit);
        u[21] = half_btf_avx2(&cospim40, &v[21], &cospi24, &v[26], &rnding, bit);
        u[22] = half_btf_avx2(&cospim24, &v[22], &cospim40, &v[25], &rnding, bit);
        u[25] = half_btf_avx2(&cospim40, &v[22], &cospi24, &v[25], &rnding, bit);
        u[26] = half_btf_avx2(&cospi24, &v[21], &cospi40, &v[26], &rnding, bit);
        u[29] = half_btf_avx2(&cospim8, &v[18], &cospi56, &v[29], &rnding, bit);
        u[30] = half_btf_avx2(&cospi56, &v[17], &cospi8, &v[30], &rnding, bit);

        for (unsigned i = 32; i < 64; i += 8) {
            addsub_avx2(v[i + 0], v[i + 3], &u[i + 0], &u[i + 3], &clamp_lo, &clamp_hi);
            addsub_avx2(v[i + 1], v[i + 2], &u[i + 1], &u[i + 2], &clamp_lo, &clamp_hi);

            addsub_avx2(v[i + 7], v[i + 4], &u[i + 7], &u[i + 4], &clamp_lo, &clamp_hi);
            addsub_avx2(v[i + 6], v[i + 5], &u[i + 6], &u[i + 5], &clamp_lo, &clamp_hi);
        }

        // stage 6
        v[0] = half_btf_0_avx2(&cospi32, &u[0], &rnding, bit);
        v[1] = half_btf_0_avx2(&cospi32, &u[0], &rnding, bit);
        v[2] = half_btf_0_avx2(&cospi48, &u[2], &rnding, bit);
        v[3] = half_btf_0_avx2(&cospi16, &u[2], &rnding, bit);

        addsub_avx2(u[4], u[5], &v[4], &v[5], &clamp_lo, &clamp_hi);
        addsub_avx2(u[7], u[6], &v[7], &v[6], &clamp_lo, &clamp_hi);

        for (unsigned i = 8; i < 16; i += 4) {
            v[i + 0] = u[i + 0];
            v[i + 3] = u[i + 3];
        }

        v[9]  = half_btf_avx2(&cospim16, &u[9], &cospi48, &u[14], &rnding, bit);
        v[10] = half_btf_avx2(&cospim48, &u[10], &cospim16, &u[13], &rnding, bit);
        v[13] = half_btf_avx2(&cospim16, &u[10], &cospi48, &u[13], &rnding, bit);
        v[14] = half_btf_avx2(&cospi48, &u[9], &cospi16, &u[14], &rnding, bit);

        for (unsigned i = 16; i < 32; i += 8) {
            addsub_avx2(u[i + 0], u[i + 3], &v[i + 0], &v[i + 3], &clamp_lo, &clamp_hi);
            addsub_avx2(u[i + 1], u[i + 2], &v[i + 1], &v[i + 2], &clamp_lo, &clamp_hi);

            addsub_avx2(u[i + 7], u[i + 4], &v[i + 7], &v[i + 4], &clamp_lo, &clamp_hi);
            addsub_avx2(u[i + 6], u[i + 5], &v[i + 6], &v[i + 5], &clamp_lo, &clamp_hi);
        }

        for (unsigned i = 32; i < 64; i += 8) {
            v[i + 0] = u[i + 0];
            v[i + 1] = u[i + 1];
            v[i + 6] = u[i + 6];
            v[i + 7] = u[i + 7];
        }

        v[34] = half_btf_avx2(&cospim8, &u[34], &cospi56, &u[61], &rnding, bit);
        v[35] = half_btf_avx2(&cospim8, &u[35], &cospi56, &u[60], &rnding, bit);
        v[36] = half_btf_avx2(&cospim56, &u[36], &cospim8, &u[59], &rnding, bit);
        v[37] = half_btf_avx2(&cospim56, &u[37], &cospim8, &u[58], &rnding, bit);
        v[42] = half_btf_avx2(&cospim40, &u[42], &cospi24, &u[53], &rnding, bit);
        v[43] = half_btf_avx2(&cospim40, &u[43], &cospi24, &u[52], &rnding, bit);
        v[44] = half_btf_avx2(&cospim24, &u[44], &cospim40, &u[51], &rnding, bit);
        v[45] = half_btf_avx2(&cospim24, &u[45], &cospim40, &u[50], &rnding, bit);
        v[50] = half_btf_avx2(&cospim40, &u[45], &cospi24, &u[50], &rnding, bit);
        v[51] = half_btf_avx2(&cospim40, &u[44], &cospi24, &u[51], &rnding, bit);
        v[52] = half_btf_avx2(&cospi24, &u[43], &cospi40, &u[52], &rnding, bit);
        v[53] = half_btf_avx2(&cospi24, &u[42], &cospi40, &u[53], &rnding, bit);
        v[58] = half_btf_avx2(&cospim8, &u[37], &cospi56, &u[58], &rnding, bit);
        v[59] = half_btf_avx2(&cospim8, &u[36], &cospi56, &u[59], &rnding, bit);
        v[60] = half_btf_avx2(&cospi56, &u[35], &cospi8, &u[60], &rnding, bit);
        v[61] = half_btf_avx2(&cospi56, &u[34], &cospi8, &u[61], &rnding, bit);

        // stage 7
        addsub_avx2(v[0], v[3], &u[0], &u[3], &clamp_lo, &clamp_hi);
        addsub_avx2(v[1], v[2], &u[1], &u[2], &clamp_lo, &clamp_hi);

        u[4] = v[4];
        u[7] = v[7];
        u[5] = half_btf_avx2(&cospim32, &v[5], &cospi32, &v[6], &rnding, bit);
        u[6] = half_btf_avx2(&cospi32, &v[5], &cospi32, &v[6], &rnding, bit);

        addsub_avx2(v[8], v[11], &u[8], &u[11], &clamp_lo, &clamp_hi);
        addsub_avx2(v[9], v[10], &u[9], &u[10], &clamp_lo, &clamp_hi);
        addsub_avx2(v[15], v[12], &u[15], &u[12], &clamp_lo, &clamp_hi);
        addsub_avx2(v[14], v[13], &u[14], &u[13], &clamp_lo, &clamp_hi);

        for (unsigned i = 16; i < 32; i += 8) {
            u[i + 0] = v[i + 0];
            u[i + 1] = v[i + 1];
            u[i + 6] = v[i + 6];
            u[i + 7] = v[i + 7];
        }

        u[18] = half_btf_avx2(&cospim16, &v[18], &cospi48, &v[29], &rnding, bit);
        u[19] = half_btf_avx2(&cospim16, &v[19], &cospi48, &v[28], &rnding, bit);
        u[20] = half_btf_avx2(&cospim48, &v[20], &cospim16, &v[27], &rnding, bit);
        u[21] = half_btf_avx2(&cospim48, &v[21], &cospim16, &v[26], &rnding, bit);
        u[26] = half_btf_avx2(&cospim16, &v[21], &cospi48, &v[26], &rnding, bit);
        u[27] = half_btf_avx2(&cospim16, &v[20], &cospi48, &v[27], &rnding, bit);
        u[28] = half_btf_avx2(&cospi48, &v[19], &cospi16, &v[28], &rnding, bit);
        u[29] = half_btf_avx2(&cospi48, &v[18], &cospi16, &v[29], &rnding, bit);

        for (unsigned i = 32; i < 64; i += 16) {
            for (unsigned j = i; j < i + 4; j++) {
                addsub_avx2(v[j], v[j ^ 7], &u[j], &u[j ^ 7], &clamp_lo, &clamp_hi);
                addsub_avx2(v[j ^ 15], v[j ^ 8], &u[j ^ 15], &u[j ^ 8], &clamp_lo, &clamp_hi);
            }
        }

        // stage 8
        for (unsigned i = 0; i < 4; ++i) {
            addsub_avx2(u[i], u[7 - i], &v[i], &v[7 - i], &clamp_lo, &clamp_hi);
        }
        v[8]  = u[8];
        v[9]  = u[9];
        v[14] = u[14];
        v[15] = u[15];

        v[10] = half_btf_avx2(&cospim32, &u[10], &cospi32, &u[13], &rnding, bit);
        v[11] = half_btf_avx2(&cospim32, &u[11], &cospi32, &u[12], &rnding, bit);
        v[12] = half_btf_avx2(&cospi32, &u[11], &cospi32, &u[12], &rnding, bit);
        v[13] = half_btf_avx2(&cospi32, &u[10], &cospi32, &u[13], &rnding, bit);

        for (unsigned i = 16; i < 20; ++i) {
            addsub_avx2(u[i], u[i ^ 7], &v[i], &v[i ^ 7], &clamp_lo, &clamp_hi);
            addsub_avx2(u[i ^ 15], u[i ^ 8], &v[i ^ 15], &v[i ^ 8], &clamp_lo, &clamp_hi);
        }

        for (unsigned i = 32; i < 36; ++i) {
            v[i]      = u[i];
            v[i + 12] = u[i + 12];
            v[i + 16] = u[i + 16];
            v[i + 28] = u[i + 28];
        }

        v[36] = half_btf_avx2(&cospim16, &u[36], &cospi48, &u[59], &rnding, bit);
        v[37] = half_btf_avx2(&cospim16, &u[37], &cospi48, &u[58], &rnding, bit);
        v[38] = half_btf_avx2(&cospim16, &u[38], &cospi48, &u[57], &rnding, bit);
        v[39] = half_btf_avx2(&cospim16, &u[39], &cospi48, &u[56], &rnding, bit);
        v[40] = half_btf_avx2(&cospim48, &u[40], &cospim16, &u[55], &rnding, bit);
        v[41] = half_btf_avx2(&cospim48, &u[41], &cospim16, &u[54], &rnding, bit);
        v[42] = half_btf_avx2(&cospim48, &u[42], &cospim16, &u[53], &rnding, bit);
        v[43] = half_btf_avx2(&cospim48, &u[43], &cospim16, &u[52], &rnding, bit);
        v[52] = half_btf_avx2(&cospim16, &u[43], &cospi48, &u[52], &rnding, bit);
        v[53] = half_btf_avx2(&cospim16, &u[42], &cospi48, &u[53], &rnding, bit);
        v[54] = half_btf_avx2(&cospim16, &u[41], &cospi48, &u[54], &rnding, bit);
        v[55] = half_btf_avx2(&cospim16, &u[40], &cospi48, &u[55], &rnding, bit);
        v[56] = half_btf_avx2(&cospi48, &u[39], &cospi16, &u[56], &rnding, bit);
        v[57] = half_btf_avx2(&cospi48, &u[38], &cospi16, &u[57], &rnding, bit);
        v[58] = half_btf_avx2(&cospi48, &u[37], &cospi16, &u[58], &rnding, bit);
        v[59] = half_btf_avx2(&cospi48, &u[36], &cospi16, &u[59], &rnding, bit);

        // stage 9
        for (unsigned i = 0; i < 8; ++i) {
            addsub_avx2(v[i], v[15 - i], &u[i], &u[15 - i], &clamp_lo, &clamp_hi);
        }
        for (unsigned i = 16; i < 20; ++i) {
            u[i]      = v[i];
            u[i + 12] = v[i + 12];
        }

        u[20] = half_btf_avx2(&cospim32, &v[20], &cospi32, &v[27], &rnding, bit);
        u[21] = half_btf_avx2(&cospim32, &v[21], &cospi32, &v[26], &rnding, bit);
        u[22] = half_btf_avx2(&cospim32, &v[22], &cospi32, &v[25], &rnding, bit);
        u[23] = half_btf_avx2(&cospim32, &v[23], &cospi32, &v[24], &rnding, bit);
        u[24] = half_btf_avx2(&cospi32, &v[23], &cospi32, &v[24], &rnding, bit);
        u[25] = half_btf_avx2(&cospi32, &v[22], &cospi32, &v[25], &rnding, bit);
        u[26] = half_btf_avx2(&cospi32, &v[21], &cospi32, &v[26], &rnding, bit);
        u[27] = half_btf_avx2(&cospi32, &v[20], &cospi32, &v[27], &rnding, bit);

        for (unsigned i = 32; i < 40; i++) {
            addsub_avx2(v[i], v[i ^ 15], &u[i], &u[i ^ 15], &clamp_lo, &clamp_hi);
        }
        for (unsigned i = 48; i < 56; i++) {
            addsub_avx2(v[i ^ 15], v[i], &u[i ^ 15], &u[i], &clamp_lo, &clamp_hi);
        }
        // stage 10
        for (unsigned i = 0; i < 16; i++) {
            addsub_avx2(u[i], u[31 - i], &v[i], &v[31 - i], &clamp_lo, &clamp_hi);
        }
        for (unsigned i = 32; i < 40; i++) {
            v[i] = u[i];
        }

        v[40] = half_btf_avx2(&cospim32, &u[40], &cospi32, &u[55], &rnding, bit);
        v[41] = half_btf_avx2(&cospim32, &u[41], &cospi32, &u[54], &rnding, bit);
        v[42] = half_btf_avx2(&cospim32, &u[42], &cospi32, &u[53], &rnding, bit);
        v[43] = half_btf_avx2(&cospim32, &u[43], &cospi32, &u[52], &rnding, bit);
        v[44] = half_btf_avx2(&cospim32, &u[44], &cospi32, &u[51], &rnding, bit);
        v[45] = half_btf_avx2(&cospim32, &u[45], &cospi32, &u[50], &rnding, bit);
        v[46] = half_btf_avx2(&cospim32, &u[46], &cospi32, &u[49], &rnding, bit);
        v[47] = half_btf_avx2(&cospim32, &u[47], &cospi32, &u[48], &rnding, bit);
        v[48] = half_btf_avx2(&cospi32, &u[47], &cospi32, &u[48], &rnding, bit);
        v[49] = half_btf_avx2(&cospi32, &u[46], &cospi32, &u[49], &rnding, bit);
        v[50] = half_btf_avx2(&cospi32, &u[45], &cospi32, &u[50], &rnding, bit);
        v[51] = half_btf_avx2(&cospi32, &u[44], &cospi32, &u[51], &rnding, bit);
        v[52] = half_btf_avx2(&cospi32, &u[43], &cospi32, &u[52], &rnding, bit);
        v[53] = half_btf_avx2(&cospi32, &u[42], &cospi32, &u[53], &rnding, bit);
        v[54] = half_btf_avx2(&cospi32, &u[41], &cospi32, &u[54], &rnding, bit);
        v[55] = half_btf_avx2(&cospi32, &u[40], &cospi32, &u[55], &rnding, bit);

        for (unsigned i = 56; i < 64; i++) {
            v[i] = u[i];
        }

        // stage 11
        for (unsigned i = 0; i < 32; i++) {
            addsub_avx2(v[i], v[63 - i], &out[(i)], &out[(63 - i)], &clamp_lo, &clamp_hi);
        }
        if (!do_cols) {
            const int     log_range_out = AOMMAX(16, bd + 6);
            const __m256i clamp_lo_out  = _mm256_set1_epi32(-(1 << (log_range_out - 1)));
            const __m256i clamp_hi_out  = _mm256_set1_epi32((1 << (log_range_out - 1)) - 1);

            round_shift_8x8_avx2(out, out_shift);
            round_shift_8x8_avx2(out + 16, out_shift);
            round_shift_8x8_avx2(out + 32, out_shift);
            round_shift_8x8_avx2(out + 48, out_shift);
            highbd_clamp_epi32_avx2(out, out, &clamp_lo_out, &clamp_hi_out, 64);
        }
    }
}

typedef void (*Transform1dAvx2)(__m256i* in, __m256i* out, int32_t bit, int32_t do_cols, int32_t bd, int32_t out_shift);

static const Transform1dAvx2 highbd_txfm_all_1d_zeros_w8_arr[TX_SIZES][ITX_TYPES_1D][4] = {
    {
        {NULL, NULL, NULL, NULL},
        {NULL, NULL, NULL, NULL},
        {NULL, NULL, NULL, NULL},
    },
    {
        {idct8x8_low1_avx2, idct8x8_avx2, NULL, NULL},
        {iadst8x8_low1_avx2, iadst8x8_avx2, NULL, NULL},
        {iidentity8_avx2, iidentity8_avx2, iidentity8_avx2, iidentity8_avx2},
    },
    {
        {idct16_low1_avx2, idct16_low8_avx2, idct16_avx2, NULL},
        {iadst16_low1_avx2, iadst16_low8_avx2, iadst16_avx2, NULL},
        {iidentity16_avx2, iidentity16_avx2, iidentity16_avx2, iidentity16_avx2},
    },
    {{idct32_low1_avx2, idct32_low8_avx2, idct32_low16_avx2, idct32_avx2_new},
     {NULL, NULL, NULL, NULL},
     {iidentity32_avx2, iidentity32_avx2, iidentity32_avx2, iidentity32_avx2}},
    {{idct64_low1_avx2, idct64_low8_avx2, idct64_low16_avx2, idct64_avx2},
     {NULL, NULL, NULL, NULL},
     {NULL, NULL, NULL, NULL}}};

static void highbd_inv_txfm2d_add_no_identity_avx2(const int32_t* input, uint16_t* output_r, int32_t stride_r,
                                                   uint16_t* output_w, int32_t stride_w, TxType tx_type, TxSize tx_size,
                                                   int32_t eob, const int32_t bd) {
    __m256i buf1[64 * 8];
    int32_t eobx, eoby;
    get_eobx_eoby_scan_default(&eobx, &eoby, tx_size, eob);
    const int8_t* shift                   = svt_aom_inv_txfm_shift_ls[tx_size];
    const int32_t txw_idx                 = get_txw_idx(tx_size);
    const int32_t txh_idx                 = get_txh_idx(tx_size);
    const int32_t txfm_size_col           = tx_size_wide[tx_size];
    const int32_t txfm_size_row           = tx_size_high[tx_size];
    const int32_t buf_size_w_div8         = txfm_size_col >> 3;
    const int32_t buf_size_nonzero_w_div8 = (eobx + 8) >> 3;
    const int32_t buf_size_nonzero_h_div8 = (eoby + 8) >> 3;
    const int32_t input_stride            = AOMMIN(32, txfm_size_col);
    const int32_t rect_type               = get_rect_tx_log_ratio(txfm_size_col, txfm_size_row);
    ASSERT(eobx < 32);
    ASSERT(eoby < 32);
    const int32_t         fun_idx_x = lowbd_txfm_all_1d_zeros_idx[eobx];
    const int32_t         fun_idx_y = lowbd_txfm_all_1d_zeros_idx[eoby];
    const Transform1dAvx2 row_txfm  = highbd_txfm_all_1d_zeros_w8_arr[txw_idx][hitx_1d_tab[tx_type]][fun_idx_x];
    const Transform1dAvx2 col_txfm  = highbd_txfm_all_1d_zeros_w8_arr[txh_idx][vitx_1d_tab[tx_type]][fun_idx_y];

    assert(col_txfm != NULL);
    assert(row_txfm != NULL);
    int32_t ud_flip, lr_flip;
    get_flip_cfg(tx_type, &ud_flip, &lr_flip);

    // 1st stage: column transform
    for (int32_t i = 0; i < buf_size_nonzero_h_div8; i++) {
        __m256i        buf0[64];
        const int32_t* input_row = input + i * input_stride * 8;
        for (int32_t j = 0; j < buf_size_nonzero_w_div8; ++j) {
            __m256i* buf0_cur = buf0 + j * 8;
            load_buffer_32x32_new(input_row + j * 8, buf0_cur, input_stride, 8);

            transpose_8x8_avx2(&buf0_cur[0], &buf0_cur[0]);
        }
        if (rect_type == 1 || rect_type == -1) {
            av1_round_shift_rect_array_32_avx2(buf0, buf0, buf_size_nonzero_w_div8 << 3, 0, new_inv_sqrt2);
        }
        row_txfm(buf0, buf0, inv_cos_bit_row[txw_idx][txh_idx], 0, bd, -shift[0]);

        __m256i* _buf1 = buf1 + i * 8;
        if (lr_flip) {
            for (int32_t j = 0; j < buf_size_w_div8; ++j) {
                transpose_8x8_flip_avx2(&buf0[j * 8], &_buf1[(buf_size_w_div8 - 1 - j) * txfm_size_row]);
            }
        } else {
            for (int32_t j = 0; j < buf_size_w_div8; ++j) {
                transpose_8x8_avx2(&buf0[j * 8], &_buf1[j * txfm_size_row]);
            }
        }
    }
    // 2nd stage: column transform
    for (int32_t i = 0; i < buf_size_w_div8; i++) {
        col_txfm(buf1 + i * txfm_size_row, buf1 + i * txfm_size_row, inv_cos_bit_col[txw_idx][txh_idx], 1, bd, 0);

        av1_round_shift_array_32_avx2(buf1 + i * txfm_size_row, buf1 + i * txfm_size_row, txfm_size_row, -shift[1]);
    }

    // write to buffer
    if (txfm_size_col >= 16) {
        for (int32_t i = 0; i < (txfm_size_col >> 4); i++) {
            highbd_write_buffer_16xn_avx2(buf1 + i * txfm_size_row * 2,
                                          output_r + 16 * i,
                                          stride_r,
                                          output_w + 16 * i,
                                          stride_w,
                                          ud_flip,
                                          txfm_size_row,
                                          bd);
        }
    } else if (txfm_size_col == 8) {
        highbd_write_buffer_8xn_avx2(buf1, output_r, stride_r, output_w, stride_w, ud_flip, txfm_size_row, bd);
    }
}

static void highbd_inv_txfm2d_add_idtx_avx2(const int32_t* input, uint16_t* output_r, int32_t stride_r,
                                            uint16_t* output_w, int32_t stride_w, TxType tx_type, TxSize tx_size,
                                            int32_t eob, const int8_t bd) {
    (void)eob;
    __m256i buf1[64 * 2];

    const int8_t* shift         = svt_aom_inv_txfm_shift_ls[tx_size];
    const int32_t txw_idx       = get_txw_idx(tx_size);
    const int32_t txh_idx       = get_txh_idx(tx_size);
    const int32_t txfm_size_col = tx_size_wide[tx_size];
    const int32_t txfm_size_row = tx_size_high[tx_size];

    const int32_t input_stride = AOMMIN(32, txfm_size_col);
    const int32_t row_max      = AOMMIN(32, txfm_size_row);
    const int32_t rect_type    = get_rect_tx_log_ratio(txfm_size_col, txfm_size_row);

    const Transform1dAvx2 row_txfm = highbd_txfm_all_1d_zeros_w8_arr[txw_idx][hitx_1d_tab[tx_type]][0];
    const Transform1dAvx2 col_txfm = highbd_txfm_all_1d_zeros_w8_arr[txh_idx][vitx_1d_tab[tx_type]][0];

    int32_t ud_flip, lr_flip;
    get_flip_cfg(tx_type, &ud_flip, &lr_flip);

    // 1st stage: row transform
    for (int32_t i = 0; i < (row_max >> 3); ++i) {
        __m256i        buf0[32];
        const int32_t* input_row = input + i * input_stride * 8;
        for (int32_t j = 0; j < (input_stride >> 3); ++j) {
            __m256i* buf0_cur = buf0 + j * 8;
            load_buffer_32x32_new(input_row + j * 8, buf0_cur, input_stride, 8);
        }
        if (rect_type == 1 || rect_type == -1) {
            av1_round_shift_rect_array_32_avx2(buf0, buf0, input_stride, 0, new_inv_sqrt2);
        }
        row_txfm(buf0, buf0, inv_cos_bit_row[txw_idx][txh_idx], 0, bd, -shift[0]);

        __m256i* _buf1 = buf1 + i * 8;

        for (int32_t j = 0; j < (input_stride >> 3); ++j) {
            _buf1[j * txfm_size_row + 0] = buf0[j * 8 + 0];
            _buf1[j * txfm_size_row + 1] = buf0[j * 8 + 1];
            _buf1[j * txfm_size_row + 2] = buf0[j * 8 + 2];
            _buf1[j * txfm_size_row + 3] = buf0[j * 8 + 3];
            _buf1[j * txfm_size_row + 4] = buf0[j * 8 + 4];
            _buf1[j * txfm_size_row + 5] = buf0[j * 8 + 5];
            _buf1[j * txfm_size_row + 6] = buf0[j * 8 + 6];
            _buf1[j * txfm_size_row + 7] = buf0[j * 8 + 7];
        }
    }
    // 2nd stage: column transform
    for (int32_t i = 0; i < (input_stride >> 3); i++) {
        col_txfm(buf1 + i * txfm_size_row, buf1 + i * txfm_size_row, inv_cos_bit_col[txw_idx][txh_idx], 1, bd, 0);

        av1_round_shift_array_32_avx2(buf1 + i * txfm_size_row, buf1 + i * txfm_size_row, txfm_size_row, -shift[1]);
    }

    // write to buffer
    if (txfm_size_col >= 16) {
        for (int32_t i = 0; i < (txfm_size_col >> 4); i++) {
            highbd_write_buffer_16xn_avx2(buf1 + i * txfm_size_row * 2,
                                          output_r + 16 * i,
                                          stride_r,
                                          output_w + 16 * i,
                                          stride_w,
                                          ud_flip,
                                          txfm_size_row,
                                          bd);
        }
    } else if (txfm_size_col == 8) {
        highbd_write_buffer_8xn_avx2(buf1, output_r, stride_r, output_w, stride_w, ud_flip, txfm_size_row, bd);
    }
}

static void highbd_inv_txfm2d_add_v_identity_avx2(const int32_t* input, uint16_t* output_r, int32_t stride_r,
                                                  uint16_t* output_w, int32_t stride_w, TxType tx_type, TxSize tx_size,
                                                  int32_t eob, const int8_t bd) {
    __m256i buf1[64];
    int32_t eobx, eoby;
    get_eobx_eoby_scan_v_identity(&eobx, &eoby, tx_size, eob);
    const int8_t*         shift           = svt_aom_inv_txfm_shift_ls[tx_size];
    const int32_t         txw_idx         = get_txw_idx(tx_size);
    const int32_t         txh_idx         = get_txh_idx(tx_size);
    const int32_t         txfm_size_col   = tx_size_wide[tx_size];
    const int32_t         txfm_size_row   = tx_size_high[tx_size];
    const int32_t         input_stride    = AOMMIN(32, txfm_size_col);
    const int32_t         buf_size_w_div4 = input_stride >> 3;
    const int32_t         buf_size_h_div8 = (eoby + 8) >> 3;
    const int32_t         rect_type       = get_rect_tx_log_ratio(txfm_size_col, txfm_size_row);
    const int32_t         fun_idx         = lowbd_txfm_all_1d_zeros_idx[eoby];
    const Transform1dAvx2 row_txfm        = highbd_txfm_all_1d_zeros_w8_arr[txw_idx][hitx_1d_tab[tx_type]][0];
    const Transform1dAvx2 col_txfm        = highbd_txfm_all_1d_zeros_w8_arr[txh_idx][vitx_1d_tab[tx_type]][fun_idx];
    int32_t               ud_flip, lr_flip;
    get_flip_cfg(tx_type, &ud_flip, &lr_flip);

    for (int32_t i = 0; i < (buf_size_h_div8 /*<< 1*/); ++i) {
        __m256i        buf0[16];
        const int32_t* input_row = input + i * input_stride * 8;
        for (int32_t j = 0; j < buf_size_w_div4; ++j) {
            __m256i* buf0_cur = buf0 + j * 8;
            load_buffer_32x32_new(input_row + j * 8, buf0_cur, input_stride, 8);
        }
        if (rect_type == 1 || rect_type == -1) {
            av1_round_shift_rect_array_32_avx2(buf0, buf0, input_stride, 0, new_inv_sqrt2);
        }
        row_txfm(buf0, buf0, inv_cos_bit_row[txw_idx][txh_idx], 0, bd, -shift[0]);

        __m256i* _buf1 = buf1 + i * 8;

        for (int32_t j = 0; j < buf_size_w_div4; ++j) {
            _buf1[j * txfm_size_row + 0] = buf0[j * 8 + 0];
            _buf1[j * txfm_size_row + 1] = buf0[j * 8 + 1];
            _buf1[j * txfm_size_row + 2] = buf0[j * 8 + 2];
            _buf1[j * txfm_size_row + 3] = buf0[j * 8 + 3];
            _buf1[j * txfm_size_row + 4] = buf0[j * 8 + 4];
            _buf1[j * txfm_size_row + 5] = buf0[j * 8 + 5];
            _buf1[j * txfm_size_row + 6] = buf0[j * 8 + 6];
            _buf1[j * txfm_size_row + 7] = buf0[j * 8 + 7];
        }
    }
    for (int32_t i = 0; i < buf_size_w_div4; i++) {
        col_txfm(buf1 + i * txfm_size_row, buf1 + i * txfm_size_row, inv_cos_bit_col[txw_idx][txh_idx], 1, bd, 0);

        av1_round_shift_array_32_avx2(buf1 + i * txfm_size_row, buf1 + i * txfm_size_row, txfm_size_row, -shift[1]);
    }

    // write to buffer
    if (txfm_size_col >= 16) {
        for (int32_t i = 0; i < (txfm_size_col >> 4); i++) {
            highbd_write_buffer_16xn_avx2(buf1 + i * txfm_size_row * 2,
                                          output_r + 16 * i,
                                          stride_r,
                                          output_w + 16 * i,
                                          stride_w,
                                          ud_flip,
                                          txfm_size_row,
                                          bd);
        }
    } else if (txfm_size_col == 8) {
        highbd_write_buffer_8xn_avx2(buf1, output_r, stride_r, output_w, stride_w, ud_flip, txfm_size_row, bd);
    }
}

static void highbd_inv_txfm2d_add_h_identity_avx2(const int32_t* input, uint16_t* output_r, int32_t stride_r,
                                                  uint16_t* output_w, int32_t stride_w, TxType tx_type, TxSize tx_size,
                                                  int32_t eob, const int32_t bd) {
    __m256i buf1[32];
    int32_t eobx, eoby;
    get_eobx_eoby_scan_h_identity(&eobx, &eoby, tx_size, eob);
    const int8_t*         shift                   = svt_aom_inv_txfm_shift_ls[tx_size];
    const int32_t         txw_idx                 = get_txw_idx(tx_size);
    const int32_t         txh_idx                 = get_txh_idx(tx_size);
    const int32_t         txfm_size_col           = tx_size_wide[tx_size];
    const int32_t         txfm_size_row           = tx_size_high[tx_size];
    const int32_t         input_stride            = AOMMIN(32, txfm_size_col);
    const int32_t         buf_size_w_div8         = input_stride >> 3;
    const int32_t         row_max                 = AOMMIN(32, txfm_size_row);
    const int32_t         buf_size_nonzero_w_div8 = (eobx + 8) >> 3;
    const int32_t         rect_type               = get_rect_tx_log_ratio(txfm_size_col, txfm_size_row);
    const int32_t         fun_idx                 = lowbd_txfm_all_1d_zeros_idx[eobx];
    const Transform1dAvx2 row_txfm = highbd_txfm_all_1d_zeros_w8_arr[txw_idx][hitx_1d_tab[tx_type]][fun_idx];
    const Transform1dAvx2 col_txfm = highbd_txfm_all_1d_zeros_w8_arr[txh_idx][vitx_1d_tab[tx_type]][0];
    int32_t               ud_flip, lr_flip;
    get_flip_cfg(tx_type, &ud_flip, &lr_flip);

    for (int32_t i = 0; i < (row_max >> 3); ++i) {
        __m256i        buf0[32];
        const int32_t* input_row = input + i * input_stride * 8;
        for (int32_t j = 0; j < buf_size_nonzero_w_div8; ++j) {
            __m256i* buf0_cur = buf0 + j * 8;
            load_buffer_32x32_new(input_row + j * 8, buf0_cur, input_stride, 8);

            transpose_8x8_avx2(&buf0_cur[0], &buf0_cur[0]);
        }
        if (rect_type == 1 || rect_type == -1) {
            av1_round_shift_rect_array_32_avx2(buf0, buf0, (buf_size_nonzero_w_div8 << 3), 0, new_inv_sqrt2);
        }
        row_txfm(buf0, buf0, inv_cos_bit_row[txw_idx][txh_idx], 0, bd, -shift[0]);

        __m256i* _buf1 = buf1 + i * 8;
        if (lr_flip) {
            for (int32_t j = 0; j < buf_size_w_div8; ++j) {
                transpose_8x8_flip_avx2(&buf0[j * 8], &_buf1[(buf_size_w_div8 - 1 - j) * txfm_size_row]);
            }
        } else {
            for (int32_t j = 0; j < buf_size_w_div8; ++j) {
                transpose_8x8_avx2(&buf0[j * 8], &_buf1[j * txfm_size_row]);
            }
        }
    }
    for (int32_t i = 0; i < buf_size_w_div8; i++) {
        col_txfm(buf1 + i * txfm_size_row, buf1 + i * txfm_size_row, inv_cos_bit_col[txw_idx][txh_idx], 1, bd, 0);

        av1_round_shift_array_32_avx2(buf1 + i * txfm_size_row, buf1 + i * txfm_size_row, txfm_size_row, -shift[1]);
    }

    // write to buffer
    if (txfm_size_col >= 16) {
        for (int32_t i = 0; i < (txfm_size_col >> 4); i++) {
            highbd_write_buffer_16xn_avx2(buf1 + i * txfm_size_row * 2,
                                          output_r + 16 * i,
                                          stride_r,
                                          output_w + 16 * i,
                                          stride_w,
                                          ud_flip,
                                          txfm_size_row,
                                          bd);
        }
    } else if (txfm_size_col == 8) {
        highbd_write_buffer_8xn_avx2(buf1, output_r, stride_r, output_w, stride_w, ud_flip, txfm_size_row, bd);
    }
}

void svt_av1_highbd_inv_txfm2d_add_universe_avx2(const int32_t* input, uint16_t* output_r, int32_t stride_r,
                                                 uint16_t* output_w, int32_t stride_w, TxType tx_type, TxSize tx_size,
                                                 int32_t eob, const int32_t bd) {
    switch (tx_type) {
    case DCT_DCT:
    case ADST_DCT:
    case DCT_ADST:
    case ADST_ADST:
    case FLIPADST_DCT:
    case DCT_FLIPADST:
    case FLIPADST_FLIPADST:
    case ADST_FLIPADST:
    case FLIPADST_ADST:
        highbd_inv_txfm2d_add_no_identity_avx2(
            input, output_r, stride_r, output_w, stride_w, tx_type, tx_size, eob, bd);
        break;
    case IDTX:
        highbd_inv_txfm2d_add_idtx_avx2(input, output_r, stride_r, output_w, stride_w, tx_type, tx_size, eob, bd);
        break;
    case V_DCT:
    case V_ADST:
    case V_FLIPADST:
        highbd_inv_txfm2d_add_v_identity_avx2(input, output_r, stride_r, output_w, stride_w, tx_type, tx_size, eob, bd);
        break;
    case H_DCT:
    case H_ADST:
    case H_FLIPADST:
        highbd_inv_txfm2d_add_h_identity_avx2(input, output_r, stride_r, output_w, stride_w, tx_type, tx_size, eob, bd);
        break;
    default:
        break;
    }
}

void svt_av1_highbd_inv_txfm_add_avx2(const int32_t* input, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                                      int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t eob, int32_t bd) {
    //assert(av1_ext_tx_used[txfm_param->tx_set_type][txfm_param->tx_type]);

    svt_av1_highbd_inv_txfm2d_add_universe_avx2(
        input, output_r, stride_r, output_w, stride_w, tx_type, tx_size, eob, bd);
}

static void load_buffer_64x64_lower_32x32_avx2(const int32_t* coeff, __m256i* in) {
    int32_t i, j;

    __m256i zero = _mm256_setzero_si256();

    for (i = 0; i < 32; ++i) {
        for (j = 0; j < 4; ++j) {
            in[8 * i + j]     = _mm256_loadu_si256((const __m256i*)(coeff + 32 * i + 8 * j));
            in[8 * i + j + 4] = zero;
        }
    }

    for (i = 0; i < 256; ++i) {
        in[256 + i] = zero;
    }
}

static INLINE void transpose_8nx8n(const __m256i* input, __m256i* output, const int32_t width, const int32_t height) {
    const int32_t numcol = height >> 3;
    const int32_t numrow = width >> 3;
    __m256i       out1[8];
    for (int32_t j = 0; j < numrow; j++) {
        for (int32_t i = 0; i < numcol; i++) {
            TRANSPOSE_4X4_AVX2(input[i * width + j + (numrow * 0)],
                               input[i * width + j + (numrow * 1)],
                               input[i * width + j + (numrow * 2)],
                               input[i * width + j + (numrow * 3)],
                               out1[0],
                               out1[1],
                               out1[4],
                               out1[5]);
            TRANSPOSE_4X4_AVX2(input[i * width + j + (numrow * 4)],
                               input[i * width + j + (numrow * 5)],
                               input[i * width + j + (numrow * 6)],
                               input[i * width + j + (numrow * 7)],
                               out1[2],
                               out1[3],
                               out1[6],
                               out1[7]);
            output[j * height + i + (numcol * 0)] = yy_unpacklo_epi128(out1[0], out1[2]);
            output[j * height + i + (numcol * 1)] = yy_unpacklo_epi128(out1[1], out1[3]);
            output[j * height + i + (numcol * 2)] = yy_unpacklo_epi128(out1[4], out1[6]);
            output[j * height + i + (numcol * 3)] = yy_unpacklo_epi128(out1[5], out1[7]);
            output[j * height + i + (numcol * 4)] = yy_unpackhi_epi128(out1[0], out1[2]);
            output[j * height + i + (numcol * 5)] = yy_unpackhi_epi128(out1[1], out1[3]);
            output[j * height + i + (numcol * 6)] = yy_unpackhi_epi128(out1[4], out1[6]);
            output[j * height + i + (numcol * 7)] = yy_unpackhi_epi128(out1[5], out1[7]);
        }
    }
}

static void round_shift_64x64_avx2(__m256i* in, const int8_t shift) {
    uint8_t ushift = (uint8_t)shift;
    __m256i rnding = _mm256_set1_epi32(1 << (ushift - 1));

    for (int32_t i = 0; i < 512; i += 4) {
        in[i]     = _mm256_add_epi32(in[i], rnding);
        in[i + 1] = _mm256_add_epi32(in[i + 1], rnding);
        in[i + 2] = _mm256_add_epi32(in[i + 2], rnding);
        in[i + 3] = _mm256_add_epi32(in[i + 3], rnding);

        in[i]     = _mm256_srai_epi32(in[i], ushift);
        in[i + 1] = _mm256_srai_epi32(in[i + 1], ushift);
        in[i + 2] = _mm256_srai_epi32(in[i + 2], ushift);
        in[i + 3] = _mm256_srai_epi32(in[i + 3], ushift);
    }
}

static void assign_32x32_input_from_64x64(const __m256i* in, __m256i* in32x32, int32_t col) {
    int32_t i;
    for (i = 0; i < 32 * 32 / 8; i += 4) {
        in32x32[i]     = in[col];
        in32x32[i + 1] = in[col + 1];
        in32x32[i + 2] = in[col + 2];
        in32x32[i + 3] = in[col + 3];
        col += 8;
    }
}

static void assign_16x16_input_from_32x32(const __m256i* in, __m256i* in16x16, int32_t col) {
    int32_t i;
    for (i = 0; i < 32; i += 2) {
        in16x16[i]     = in[col];
        in16x16[i + 1] = in[col + 1];
        col += 4;
    }
}

static void write_buffer_32x32_new(__m256i* in, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                                   int32_t stride_w, int32_t bd) {
    __m256i   in16x16[32];
    uint16_t* left_up_r    = &output_r[0];
    uint16_t* right_up_r   = &output_r[16];
    uint16_t* left_down_r  = &output_r[16 * stride_r];
    uint16_t* right_down_r = &output_r[16 * stride_r + 16];
    uint16_t* left_up_w    = &output_w[0];
    uint16_t* right_up_w   = &output_w[16];
    uint16_t* left_down_w  = &output_w[16 * stride_w];
    uint16_t* right_down_w = &output_w[16 * stride_w + 16];

    // Left-up quarter
    assign_16x16_input_from_32x32(in, in16x16, 0);
    write_buffer_16x16_new(in16x16, left_up_r, stride_r, left_up_w, stride_w, bd);

    // Right-up quarter
    assign_16x16_input_from_32x32(in, in16x16, 32 / 2 / 8);
    write_buffer_16x16_new(in16x16, right_up_r, stride_r, right_up_w, stride_w, bd);

    // Left-down quarter
    assign_16x16_input_from_32x32(in, in16x16, 32 * 32 / 2 / 8);
    write_buffer_16x16_new(in16x16, left_down_r, stride_r, left_down_w, stride_w, bd);

    // Right-down quarter
    assign_16x16_input_from_32x32(in, in16x16, 32 * 32 / 2 / 8 + 32 / 2 / 8);
    write_buffer_16x16_new(in16x16, right_down_r, stride_r, right_down_w, stride_w, bd);
}

static void write_buffer_64x64_avx2(__m256i* in, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                                    int32_t stride_w, int32_t bd) {
    __m256i   in32x32[32 * 32 / 8];
    uint16_t* left_up_r    = &output_r[0];
    uint16_t* right_up_r   = &output_r[32];
    uint16_t* left_down_r  = &output_r[32 * stride_r];
    uint16_t* right_down_r = &output_r[32 * stride_r + 32];
    uint16_t* left_up_w    = &output_w[0];
    uint16_t* right_up_w   = &output_w[32];
    uint16_t* left_down_w  = &output_w[32 * stride_w];
    uint16_t* right_down_w = &output_w[32 * stride_w + 32];

    // Left-up quarter
    assign_32x32_input_from_64x64(in, in32x32, 0);
    write_buffer_32x32_new(in32x32, left_up_r, stride_r, left_up_w, stride_w, bd);

    // Right-up quarter
    assign_32x32_input_from_64x64(in, in32x32, 64 / 2 / 8);
    write_buffer_32x32_new(in32x32, right_up_r, stride_r, right_up_w, stride_w, bd);

    // Left-down quarter
    assign_32x32_input_from_64x64(in, in32x32, 64 * 64 / 2 / 8);
    write_buffer_32x32_new(in32x32, left_down_r, stride_r, left_down_w, stride_w, bd);

    // Right-down quarter
    assign_32x32_input_from_64x64(in, in32x32, 64 * 64 / 2 / 8 + 64 / 2 / 8);
    write_buffer_32x32_new(in32x32, right_down_r, stride_r, right_down_w, stride_w, bd);
}

static void idct64x64_avx2(__m256i* in, __m256i* out, int32_t bit, int32_t do_cols, int32_t bd) {
    int32_t        i, j;
    const int32_t* cospi     = cospi_arr(bit);
    const __m256i  rnding    = _mm256_set1_epi32(1 << (bit - 1));
    const int32_t  log_range = AOMMAX(16, bd + (do_cols ? 6 : 8));
    const __m256i  clamp_lo  = _mm256_set1_epi32(-(1 << (log_range - 1)));
    const __m256i  clamp_hi  = _mm256_set1_epi32((1 << (log_range - 1)) - 1);
    int32_t        col;

    const __m256i cospi1  = _mm256_set1_epi32(cospi[1]);
    const __m256i cospi2  = _mm256_set1_epi32(cospi[2]);
    const __m256i cospi3  = _mm256_set1_epi32(cospi[3]);
    const __m256i cospi4  = _mm256_set1_epi32(cospi[4]);
    const __m256i cospi5  = _mm256_set1_epi32(cospi[5]);
    const __m256i cospi6  = _mm256_set1_epi32(cospi[6]);
    const __m256i cospi7  = _mm256_set1_epi32(cospi[7]);
    const __m256i cospi8  = _mm256_set1_epi32(cospi[8]);
    const __m256i cospi9  = _mm256_set1_epi32(cospi[9]);
    const __m256i cospi10 = _mm256_set1_epi32(cospi[10]);
    const __m256i cospi11 = _mm256_set1_epi32(cospi[11]);
    const __m256i cospi12 = _mm256_set1_epi32(cospi[12]);
    const __m256i cospi13 = _mm256_set1_epi32(cospi[13]);
    const __m256i cospi14 = _mm256_set1_epi32(cospi[14]);
    const __m256i cospi15 = _mm256_set1_epi32(cospi[15]);
    const __m256i cospi16 = _mm256_set1_epi32(cospi[16]);
    const __m256i cospi17 = _mm256_set1_epi32(cospi[17]);
    const __m256i cospi18 = _mm256_set1_epi32(cospi[18]);
    const __m256i cospi19 = _mm256_set1_epi32(cospi[19]);
    const __m256i cospi20 = _mm256_set1_epi32(cospi[20]);
    const __m256i cospi21 = _mm256_set1_epi32(cospi[21]);
    const __m256i cospi22 = _mm256_set1_epi32(cospi[22]);
    const __m256i cospi23 = _mm256_set1_epi32(cospi[23]);
    const __m256i cospi24 = _mm256_set1_epi32(cospi[24]);
    const __m256i cospi25 = _mm256_set1_epi32(cospi[25]);
    const __m256i cospi26 = _mm256_set1_epi32(cospi[26]);
    const __m256i cospi27 = _mm256_set1_epi32(cospi[27]);
    const __m256i cospi28 = _mm256_set1_epi32(cospi[28]);
    const __m256i cospi29 = _mm256_set1_epi32(cospi[29]);
    const __m256i cospi30 = _mm256_set1_epi32(cospi[30]);
    const __m256i cospi31 = _mm256_set1_epi32(cospi[31]);
    const __m256i cospi32 = _mm256_set1_epi32(cospi[32]);
    const __m256i cospi35 = _mm256_set1_epi32(cospi[35]);
    const __m256i cospi36 = _mm256_set1_epi32(cospi[36]);
    const __m256i cospi38 = _mm256_set1_epi32(cospi[38]);
    const __m256i cospi39 = _mm256_set1_epi32(cospi[39]);
    const __m256i cospi40 = _mm256_set1_epi32(cospi[40]);
    const __m256i cospi43 = _mm256_set1_epi32(cospi[43]);
    const __m256i cospi44 = _mm256_set1_epi32(cospi[44]);
    const __m256i cospi46 = _mm256_set1_epi32(cospi[46]);
    const __m256i cospi47 = _mm256_set1_epi32(cospi[47]);
    const __m256i cospi48 = _mm256_set1_epi32(cospi[48]);
    const __m256i cospi51 = _mm256_set1_epi32(cospi[51]);
    const __m256i cospi52 = _mm256_set1_epi32(cospi[52]);
    const __m256i cospi54 = _mm256_set1_epi32(cospi[54]);
    const __m256i cospi55 = _mm256_set1_epi32(cospi[55]);
    const __m256i cospi56 = _mm256_set1_epi32(cospi[56]);
    const __m256i cospi59 = _mm256_set1_epi32(cospi[59]);
    const __m256i cospi60 = _mm256_set1_epi32(cospi[60]);
    const __m256i cospi62 = _mm256_set1_epi32(cospi[62]);
    const __m256i cospi63 = _mm256_set1_epi32(cospi[63]);

    const __m256i cospim4  = _mm256_set1_epi32(-cospi[4]);
    const __m256i cospim8  = _mm256_set1_epi32(-cospi[8]);
    const __m256i cospim12 = _mm256_set1_epi32(-cospi[12]);
    const __m256i cospim16 = _mm256_set1_epi32(-cospi[16]);
    const __m256i cospim20 = _mm256_set1_epi32(-cospi[20]);
    const __m256i cospim24 = _mm256_set1_epi32(-cospi[24]);
    const __m256i cospim28 = _mm256_set1_epi32(-cospi[28]);
    const __m256i cospim32 = _mm256_set1_epi32(-cospi[32]);
    const __m256i cospim33 = _mm256_set1_epi32(-cospi[33]);
    const __m256i cospim34 = _mm256_set1_epi32(-cospi[34]);
    const __m256i cospim36 = _mm256_set1_epi32(-cospi[36]);
    const __m256i cospim37 = _mm256_set1_epi32(-cospi[37]);
    const __m256i cospim40 = _mm256_set1_epi32(-cospi[40]);
    const __m256i cospim41 = _mm256_set1_epi32(-cospi[41]);
    const __m256i cospim42 = _mm256_set1_epi32(-cospi[42]);
    const __m256i cospim44 = _mm256_set1_epi32(-cospi[44]);
    const __m256i cospim45 = _mm256_set1_epi32(-cospi[45]);
    const __m256i cospim48 = _mm256_set1_epi32(-cospi[48]);
    const __m256i cospim49 = _mm256_set1_epi32(-cospi[49]);
    const __m256i cospim50 = _mm256_set1_epi32(-cospi[50]);
    const __m256i cospim52 = _mm256_set1_epi32(-cospi[52]);
    const __m256i cospim53 = _mm256_set1_epi32(-cospi[53]);
    const __m256i cospim56 = _mm256_set1_epi32(-cospi[56]);
    const __m256i cospim57 = _mm256_set1_epi32(-cospi[57]);
    const __m256i cospim58 = _mm256_set1_epi32(-cospi[58]);
    const __m256i cospim60 = _mm256_set1_epi32(-cospi[60]);
    const __m256i cospim61 = _mm256_set1_epi32(-cospi[61]);

    for (col = 0; col < (do_cols ? 64 / 8 : 32 / 8); ++col) {
        __m256i u[64], v[64];

        // stage 1
        u[32] = in[1 * 8 + col];
        u[34] = in[17 * 8 + col];
        u[36] = in[9 * 8 + col];
        u[38] = in[25 * 8 + col];
        u[40] = in[5 * 8 + col];
        u[42] = in[21 * 8 + col];
        u[44] = in[13 * 8 + col];
        u[46] = in[29 * 8 + col];
        u[48] = in[3 * 8 + col];
        u[50] = in[19 * 8 + col];
        u[52] = in[11 * 8 + col];
        u[54] = in[27 * 8 + col];
        u[56] = in[7 * 8 + col];
        u[58] = in[23 * 8 + col];
        u[60] = in[15 * 8 + col];
        u[62] = in[31 * 8 + col];

        v[16] = in[2 * 8 + col];
        v[18] = in[18 * 8 + col];
        v[20] = in[10 * 8 + col];
        v[22] = in[26 * 8 + col];
        v[24] = in[6 * 8 + col];
        v[26] = in[22 * 8 + col];
        v[28] = in[14 * 8 + col];
        v[30] = in[30 * 8 + col];

        u[8]  = in[4 * 8 + col];
        u[10] = in[20 * 8 + col];
        u[12] = in[12 * 8 + col];
        u[14] = in[28 * 8 + col];

        v[4] = in[8 * 8 + col];
        v[6] = in[24 * 8 + col];

        u[0] = in[0 * 8 + col];
        u[2] = in[16 * 8 + col];

        // stage 2
        v[32] = half_btf_0_avx2(&cospi63, &u[32], &rnding, bit);
        v[33] = half_btf_0_avx2(&cospim33, &u[62], &rnding, bit);
        v[34] = half_btf_0_avx2(&cospi47, &u[34], &rnding, bit);
        v[35] = half_btf_0_avx2(&cospim49, &u[60], &rnding, bit);
        v[36] = half_btf_0_avx2(&cospi55, &u[36], &rnding, bit);
        v[37] = half_btf_0_avx2(&cospim41, &u[58], &rnding, bit);
        v[38] = half_btf_0_avx2(&cospi39, &u[38], &rnding, bit);
        v[39] = half_btf_0_avx2(&cospim57, &u[56], &rnding, bit);
        v[40] = half_btf_0_avx2(&cospi59, &u[40], &rnding, bit);
        v[41] = half_btf_0_avx2(&cospim37, &u[54], &rnding, bit);
        v[42] = half_btf_0_avx2(&cospi43, &u[42], &rnding, bit);
        v[43] = half_btf_0_avx2(&cospim53, &u[52], &rnding, bit);
        v[44] = half_btf_0_avx2(&cospi51, &u[44], &rnding, bit);
        v[45] = half_btf_0_avx2(&cospim45, &u[50], &rnding, bit);
        v[46] = half_btf_0_avx2(&cospi35, &u[46], &rnding, bit);
        v[47] = half_btf_0_avx2(&cospim61, &u[48], &rnding, bit);
        v[48] = half_btf_0_avx2(&cospi3, &u[48], &rnding, bit);
        v[49] = half_btf_0_avx2(&cospi29, &u[46], &rnding, bit);
        v[50] = half_btf_0_avx2(&cospi19, &u[50], &rnding, bit);
        v[51] = half_btf_0_avx2(&cospi13, &u[44], &rnding, bit);
        v[52] = half_btf_0_avx2(&cospi11, &u[52], &rnding, bit);
        v[53] = half_btf_0_avx2(&cospi21, &u[42], &rnding, bit);
        v[54] = half_btf_0_avx2(&cospi27, &u[54], &rnding, bit);
        v[55] = half_btf_0_avx2(&cospi5, &u[40], &rnding, bit);
        v[56] = half_btf_0_avx2(&cospi7, &u[56], &rnding, bit);
        v[57] = half_btf_0_avx2(&cospi25, &u[38], &rnding, bit);
        v[58] = half_btf_0_avx2(&cospi23, &u[58], &rnding, bit);
        v[59] = half_btf_0_avx2(&cospi9, &u[36], &rnding, bit);
        v[60] = half_btf_0_avx2(&cospi15, &u[60], &rnding, bit);
        v[61] = half_btf_0_avx2(&cospi17, &u[34], &rnding, bit);
        v[62] = half_btf_0_avx2(&cospi31, &u[62], &rnding, bit);
        v[63] = half_btf_0_avx2(&cospi1, &u[32], &rnding, bit);

        // stage 3
        u[16] = half_btf_0_avx2(&cospi62, &v[16], &rnding, bit);
        u[17] = half_btf_0_avx2(&cospim34, &v[30], &rnding, bit);
        u[18] = half_btf_0_avx2(&cospi46, &v[18], &rnding, bit);
        u[19] = half_btf_0_avx2(&cospim50, &v[28], &rnding, bit);
        u[20] = half_btf_0_avx2(&cospi54, &v[20], &rnding, bit);
        u[21] = half_btf_0_avx2(&cospim42, &v[26], &rnding, bit);
        u[22] = half_btf_0_avx2(&cospi38, &v[22], &rnding, bit);
        u[23] = half_btf_0_avx2(&cospim58, &v[24], &rnding, bit);
        u[24] = half_btf_0_avx2(&cospi6, &v[24], &rnding, bit);
        u[25] = half_btf_0_avx2(&cospi26, &v[22], &rnding, bit);
        u[26] = half_btf_0_avx2(&cospi22, &v[26], &rnding, bit);
        u[27] = half_btf_0_avx2(&cospi10, &v[20], &rnding, bit);
        u[28] = half_btf_0_avx2(&cospi14, &v[28], &rnding, bit);
        u[29] = half_btf_0_avx2(&cospi18, &v[18], &rnding, bit);
        u[30] = half_btf_0_avx2(&cospi30, &v[30], &rnding, bit);
        u[31] = half_btf_0_avx2(&cospi2, &v[16], &rnding, bit);

        for (i = 32; i < 64; i += 4) {
            addsub_avx2(v[i + 0], v[i + 1], &u[i + 0], &u[i + 1], &clamp_lo, &clamp_hi);
            addsub_avx2(v[i + 3], v[i + 2], &u[i + 3], &u[i + 2], &clamp_lo, &clamp_hi);
        }

        // stage 4
        v[8]  = half_btf_0_avx2(&cospi60, &u[8], &rnding, bit);
        v[9]  = half_btf_0_avx2(&cospim36, &u[14], &rnding, bit);
        v[10] = half_btf_0_avx2(&cospi44, &u[10], &rnding, bit);
        v[11] = half_btf_0_avx2(&cospim52, &u[12], &rnding, bit);
        v[12] = half_btf_0_avx2(&cospi12, &u[12], &rnding, bit);
        v[13] = half_btf_0_avx2(&cospi20, &u[10], &rnding, bit);
        v[14] = half_btf_0_avx2(&cospi28, &u[14], &rnding, bit);
        v[15] = half_btf_0_avx2(&cospi4, &u[8], &rnding, bit);

        for (i = 16; i < 32; i += 4) {
            addsub_avx2(u[i + 0], u[i + 1], &v[i + 0], &v[i + 1], &clamp_lo, &clamp_hi);
            addsub_avx2(u[i + 3], u[i + 2], &v[i + 3], &v[i + 2], &clamp_lo, &clamp_hi);
        }

        for (i = 32; i < 64; i += 4) {
            v[i + 0] = u[i + 0];
            v[i + 3] = u[i + 3];
        }

        v[33] = half_btf_avx2(&cospim4, &u[33], &cospi60, &u[62], &rnding, bit);
        v[34] = half_btf_avx2(&cospim60, &u[34], &cospim4, &u[61], &rnding, bit);
        v[37] = half_btf_avx2(&cospim36, &u[37], &cospi28, &u[58], &rnding, bit);
        v[38] = half_btf_avx2(&cospim28, &u[38], &cospim36, &u[57], &rnding, bit);
        v[41] = half_btf_avx2(&cospim20, &u[41], &cospi44, &u[54], &rnding, bit);
        v[42] = half_btf_avx2(&cospim44, &u[42], &cospim20, &u[53], &rnding, bit);
        v[45] = half_btf_avx2(&cospim52, &u[45], &cospi12, &u[50], &rnding, bit);
        v[46] = half_btf_avx2(&cospim12, &u[46], &cospim52, &u[49], &rnding, bit);
        v[49] = half_btf_avx2(&cospim52, &u[46], &cospi12, &u[49], &rnding, bit);
        v[50] = half_btf_avx2(&cospi12, &u[45], &cospi52, &u[50], &rnding, bit);
        v[53] = half_btf_avx2(&cospim20, &u[42], &cospi44, &u[53], &rnding, bit);
        v[54] = half_btf_avx2(&cospi44, &u[41], &cospi20, &u[54], &rnding, bit);
        v[57] = half_btf_avx2(&cospim36, &u[38], &cospi28, &u[57], &rnding, bit);
        v[58] = half_btf_avx2(&cospi28, &u[37], &cospi36, &u[58], &rnding, bit);
        v[61] = half_btf_avx2(&cospim4, &u[34], &cospi60, &u[61], &rnding, bit);
        v[62] = half_btf_avx2(&cospi60, &u[33], &cospi4, &u[62], &rnding, bit);

        // stage 5
        u[4] = half_btf_0_avx2(&cospi56, &v[4], &rnding, bit);
        u[5] = half_btf_0_avx2(&cospim40, &v[6], &rnding, bit);
        u[6] = half_btf_0_avx2(&cospi24, &v[6], &rnding, bit);
        u[7] = half_btf_0_avx2(&cospi8, &v[4], &rnding, bit);

        for (i = 8; i < 16; i += 4) {
            addsub_avx2(v[i + 0], v[i + 1], &u[i + 0], &u[i + 1], &clamp_lo, &clamp_hi);
            addsub_avx2(v[i + 3], v[i + 2], &u[i + 3], &u[i + 2], &clamp_lo, &clamp_hi);
        }

        for (i = 16; i < 32; i += 4) {
            u[i + 0] = v[i + 0];
            u[i + 3] = v[i + 3];
        }

        u[17] = half_btf_avx2(&cospim8, &v[17], &cospi56, &v[30], &rnding, bit);
        u[18] = half_btf_avx2(&cospim56, &v[18], &cospim8, &v[29], &rnding, bit);
        u[21] = half_btf_avx2(&cospim40, &v[21], &cospi24, &v[26], &rnding, bit);
        u[22] = half_btf_avx2(&cospim24, &v[22], &cospim40, &v[25], &rnding, bit);
        u[25] = half_btf_avx2(&cospim40, &v[22], &cospi24, &v[25], &rnding, bit);
        u[26] = half_btf_avx2(&cospi24, &v[21], &cospi40, &v[26], &rnding, bit);
        u[29] = half_btf_avx2(&cospim8, &v[18], &cospi56, &v[29], &rnding, bit);
        u[30] = half_btf_avx2(&cospi56, &v[17], &cospi8, &v[30], &rnding, bit);

        for (i = 32; i < 64; i += 8) {
            addsub_avx2(v[i + 0], v[i + 3], &u[i + 0], &u[i + 3], &clamp_lo, &clamp_hi);
            addsub_avx2(v[i + 1], v[i + 2], &u[i + 1], &u[i + 2], &clamp_lo, &clamp_hi);

            addsub_avx2(v[i + 7], v[i + 4], &u[i + 7], &u[i + 4], &clamp_lo, &clamp_hi);
            addsub_avx2(v[i + 6], v[i + 5], &u[i + 6], &u[i + 5], &clamp_lo, &clamp_hi);
        }

        // stage 6
        v[0] = half_btf_0_avx2(&cospi32, &u[0], &rnding, bit);
        v[1] = half_btf_0_avx2(&cospi32, &u[0], &rnding, bit);
        v[2] = half_btf_0_avx2(&cospi48, &u[2], &rnding, bit);
        v[3] = half_btf_0_avx2(&cospi16, &u[2], &rnding, bit);

        addsub_avx2(u[4], u[5], &v[4], &v[5], &clamp_lo, &clamp_hi);
        addsub_avx2(u[7], u[6], &v[7], &v[6], &clamp_lo, &clamp_hi);

        for (i = 8; i < 16; i += 4) {
            v[i + 0] = u[i + 0];
            v[i + 3] = u[i + 3];
        }

        v[9]  = half_btf_avx2(&cospim16, &u[9], &cospi48, &u[14], &rnding, bit);
        v[10] = half_btf_avx2(&cospim48, &u[10], &cospim16, &u[13], &rnding, bit);
        v[13] = half_btf_avx2(&cospim16, &u[10], &cospi48, &u[13], &rnding, bit);
        v[14] = half_btf_avx2(&cospi48, &u[9], &cospi16, &u[14], &rnding, bit);

        for (i = 16; i < 32; i += 8) {
            addsub_avx2(u[i + 0], u[i + 3], &v[i + 0], &v[i + 3], &clamp_lo, &clamp_hi);
            addsub_avx2(u[i + 1], u[i + 2], &v[i + 1], &v[i + 2], &clamp_lo, &clamp_hi);

            addsub_avx2(u[i + 7], u[i + 4], &v[i + 7], &v[i + 4], &clamp_lo, &clamp_hi);
            addsub_avx2(u[i + 6], u[i + 5], &v[i + 6], &v[i + 5], &clamp_lo, &clamp_hi);
        }

        for (i = 32; i < 64; i += 8) {
            v[i + 0] = u[i + 0];
            v[i + 1] = u[i + 1];
            v[i + 6] = u[i + 6];
            v[i + 7] = u[i + 7];
        }

        v[34] = half_btf_avx2(&cospim8, &u[34], &cospi56, &u[61], &rnding, bit);
        v[35] = half_btf_avx2(&cospim8, &u[35], &cospi56, &u[60], &rnding, bit);
        v[36] = half_btf_avx2(&cospim56, &u[36], &cospim8, &u[59], &rnding, bit);
        v[37] = half_btf_avx2(&cospim56, &u[37], &cospim8, &u[58], &rnding, bit);
        v[42] = half_btf_avx2(&cospim40, &u[42], &cospi24, &u[53], &rnding, bit);
        v[43] = half_btf_avx2(&cospim40, &u[43], &cospi24, &u[52], &rnding, bit);
        v[44] = half_btf_avx2(&cospim24, &u[44], &cospim40, &u[51], &rnding, bit);
        v[45] = half_btf_avx2(&cospim24, &u[45], &cospim40, &u[50], &rnding, bit);
        v[50] = half_btf_avx2(&cospim40, &u[45], &cospi24, &u[50], &rnding, bit);
        v[51] = half_btf_avx2(&cospim40, &u[44], &cospi24, &u[51], &rnding, bit);
        v[52] = half_btf_avx2(&cospi24, &u[43], &cospi40, &u[52], &rnding, bit);
        v[53] = half_btf_avx2(&cospi24, &u[42], &cospi40, &u[53], &rnding, bit);
        v[58] = half_btf_avx2(&cospim8, &u[37], &cospi56, &u[58], &rnding, bit);
        v[59] = half_btf_avx2(&cospim8, &u[36], &cospi56, &u[59], &rnding, bit);
        v[60] = half_btf_avx2(&cospi56, &u[35], &cospi8, &u[60], &rnding, bit);
        v[61] = half_btf_avx2(&cospi56, &u[34], &cospi8, &u[61], &rnding, bit);

        // stage 7
        addsub_avx2(v[0], v[3], &u[0], &u[3], &clamp_lo, &clamp_hi);
        addsub_avx2(v[1], v[2], &u[1], &u[2], &clamp_lo, &clamp_hi);

        u[4] = v[4];
        u[7] = v[7];
        u[5] = half_btf_avx2(&cospim32, &v[5], &cospi32, &v[6], &rnding, bit);
        u[6] = half_btf_avx2(&cospi32, &v[5], &cospi32, &v[6], &rnding, bit);

        addsub_avx2(v[8], v[11], &u[8], &u[11], &clamp_lo, &clamp_hi);
        addsub_avx2(v[9], v[10], &u[9], &u[10], &clamp_lo, &clamp_hi);
        addsub_avx2(v[15], v[12], &u[15], &u[12], &clamp_lo, &clamp_hi);
        addsub_avx2(v[14], v[13], &u[14], &u[13], &clamp_lo, &clamp_hi);

        for (i = 16; i < 32; i += 8) {
            u[i + 0] = v[i + 0];
            u[i + 1] = v[i + 1];
            u[i + 6] = v[i + 6];
            u[i + 7] = v[i + 7];
        }

        u[18] = half_btf_avx2(&cospim16, &v[18], &cospi48, &v[29], &rnding, bit);
        u[19] = half_btf_avx2(&cospim16, &v[19], &cospi48, &v[28], &rnding, bit);
        u[20] = half_btf_avx2(&cospim48, &v[20], &cospim16, &v[27], &rnding, bit);
        u[21] = half_btf_avx2(&cospim48, &v[21], &cospim16, &v[26], &rnding, bit);
        u[26] = half_btf_avx2(&cospim16, &v[21], &cospi48, &v[26], &rnding, bit);
        u[27] = half_btf_avx2(&cospim16, &v[20], &cospi48, &v[27], &rnding, bit);
        u[28] = half_btf_avx2(&cospi48, &v[19], &cospi16, &v[28], &rnding, bit);
        u[29] = half_btf_avx2(&cospi48, &v[18], &cospi16, &v[29], &rnding, bit);

        for (i = 32; i < 64; i += 16) {
            for (j = i; j < i + 4; j++) {
                addsub_avx2(v[j], v[j ^ 7], &u[j], &u[j ^ 7], &clamp_lo, &clamp_hi);
                addsub_avx2(v[j ^ 15], v[j ^ 8], &u[j ^ 15], &u[j ^ 8], &clamp_lo, &clamp_hi);
            }
        }

        // stage 8
        for (i = 0; i < 4; ++i) {
            addsub_avx2(u[i], u[7 - i], &v[i], &v[7 - i], &clamp_lo, &clamp_hi);
        }
        v[8]  = u[8];
        v[9]  = u[9];
        v[14] = u[14];
        v[15] = u[15];

        v[10] = half_btf_avx2(&cospim32, &u[10], &cospi32, &u[13], &rnding, bit);
        v[11] = half_btf_avx2(&cospim32, &u[11], &cospi32, &u[12], &rnding, bit);
        v[12] = half_btf_avx2(&cospi32, &u[11], &cospi32, &u[12], &rnding, bit);
        v[13] = half_btf_avx2(&cospi32, &u[10], &cospi32, &u[13], &rnding, bit);

        for (i = 16; i < 20; ++i) {
            addsub_avx2(u[i], u[i ^ 7], &v[i], &v[i ^ 7], &clamp_lo, &clamp_hi);
            addsub_avx2(u[i ^ 15], u[i ^ 8], &v[i ^ 15], &v[i ^ 8], &clamp_lo, &clamp_hi);
        }

        for (i = 32; i < 36; ++i) {
            v[i]      = u[i];
            v[i + 12] = u[i + 12];
            v[i + 16] = u[i + 16];
            v[i + 28] = u[i + 28];
        }

        v[36] = half_btf_avx2(&cospim16, &u[36], &cospi48, &u[59], &rnding, bit);
        v[37] = half_btf_avx2(&cospim16, &u[37], &cospi48, &u[58], &rnding, bit);
        v[38] = half_btf_avx2(&cospim16, &u[38], &cospi48, &u[57], &rnding, bit);
        v[39] = half_btf_avx2(&cospim16, &u[39], &cospi48, &u[56], &rnding, bit);
        v[40] = half_btf_avx2(&cospim48, &u[40], &cospim16, &u[55], &rnding, bit);
        v[41] = half_btf_avx2(&cospim48, &u[41], &cospim16, &u[54], &rnding, bit);
        v[42] = half_btf_avx2(&cospim48, &u[42], &cospim16, &u[53], &rnding, bit);
        v[43] = half_btf_avx2(&cospim48, &u[43], &cospim16, &u[52], &rnding, bit);
        v[52] = half_btf_avx2(&cospim16, &u[43], &cospi48, &u[52], &rnding, bit);
        v[53] = half_btf_avx2(&cospim16, &u[42], &cospi48, &u[53], &rnding, bit);
        v[54] = half_btf_avx2(&cospim16, &u[41], &cospi48, &u[54], &rnding, bit);
        v[55] = half_btf_avx2(&cospim16, &u[40], &cospi48, &u[55], &rnding, bit);
        v[56] = half_btf_avx2(&cospi48, &u[39], &cospi16, &u[56], &rnding, bit);
        v[57] = half_btf_avx2(&cospi48, &u[38], &cospi16, &u[57], &rnding, bit);
        v[58] = half_btf_avx2(&cospi48, &u[37], &cospi16, &u[58], &rnding, bit);
        v[59] = half_btf_avx2(&cospi48, &u[36], &cospi16, &u[59], &rnding, bit);

        // stage 9
        for (i = 0; i < 8; ++i) {
            addsub_avx2(v[i], v[15 - i], &u[i], &u[15 - i], &clamp_lo, &clamp_hi);
        }
        for (i = 16; i < 20; ++i) {
            u[i]      = v[i];
            u[i + 12] = v[i + 12];
        }

        u[20] = half_btf_avx2(&cospim32, &v[20], &cospi32, &v[27], &rnding, bit);
        u[21] = half_btf_avx2(&cospim32, &v[21], &cospi32, &v[26], &rnding, bit);
        u[22] = half_btf_avx2(&cospim32, &v[22], &cospi32, &v[25], &rnding, bit);
        u[23] = half_btf_avx2(&cospim32, &v[23], &cospi32, &v[24], &rnding, bit);
        u[24] = half_btf_avx2(&cospi32, &v[23], &cospi32, &v[24], &rnding, bit);
        u[25] = half_btf_avx2(&cospi32, &v[22], &cospi32, &v[25], &rnding, bit);
        u[26] = half_btf_avx2(&cospi32, &v[21], &cospi32, &v[26], &rnding, bit);
        u[27] = half_btf_avx2(&cospi32, &v[20], &cospi32, &v[27], &rnding, bit);

        for (i = 32; i < 40; i++) {
            addsub_avx2(v[i], v[i ^ 15], &u[i], &u[i ^ 15], &clamp_lo, &clamp_hi);
        }
        for (i = 48; i < 56; i++) {
            addsub_avx2(v[i ^ 15], v[i], &u[i ^ 15], &u[i], &clamp_lo, &clamp_hi);
        }
        // stage 10
        for (i = 0; i < 16; i++) {
            addsub_avx2(u[i], u[31 - i], &v[i], &v[31 - i], &clamp_lo, &clamp_hi);
        }
        for (i = 32; i < 40; i++) {
            v[i] = u[i];
        }

        v[40] = half_btf_avx2(&cospim32, &u[40], &cospi32, &u[55], &rnding, bit);
        v[41] = half_btf_avx2(&cospim32, &u[41], &cospi32, &u[54], &rnding, bit);
        v[42] = half_btf_avx2(&cospim32, &u[42], &cospi32, &u[53], &rnding, bit);
        v[43] = half_btf_avx2(&cospim32, &u[43], &cospi32, &u[52], &rnding, bit);
        v[44] = half_btf_avx2(&cospim32, &u[44], &cospi32, &u[51], &rnding, bit);
        v[45] = half_btf_avx2(&cospim32, &u[45], &cospi32, &u[50], &rnding, bit);
        v[46] = half_btf_avx2(&cospim32, &u[46], &cospi32, &u[49], &rnding, bit);
        v[47] = half_btf_avx2(&cospim32, &u[47], &cospi32, &u[48], &rnding, bit);
        v[48] = half_btf_avx2(&cospi32, &u[47], &cospi32, &u[48], &rnding, bit);
        v[49] = half_btf_avx2(&cospi32, &u[46], &cospi32, &u[49], &rnding, bit);
        v[50] = half_btf_avx2(&cospi32, &u[45], &cospi32, &u[50], &rnding, bit);
        v[51] = half_btf_avx2(&cospi32, &u[44], &cospi32, &u[51], &rnding, bit);
        v[52] = half_btf_avx2(&cospi32, &u[43], &cospi32, &u[52], &rnding, bit);
        v[53] = half_btf_avx2(&cospi32, &u[42], &cospi32, &u[53], &rnding, bit);
        v[54] = half_btf_avx2(&cospi32, &u[41], &cospi32, &u[54], &rnding, bit);
        v[55] = half_btf_avx2(&cospi32, &u[40], &cospi32, &u[55], &rnding, bit);

        for (i = 56; i < 64; i++) {
            v[i] = u[i];
        }

        // stage 11
        for (i = 0; i < 32; i++) {
            addsub_avx2(v[i], v[63 - i], &out[8 * (i) + col], &out[8 * (63 - i) + col], &clamp_lo, &clamp_hi);
        }
    }
}

void svt_av1_inv_txfm2d_add_64x64_avx2(const int32_t* input, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                                       int32_t stride_w, TxType tx_type, int32_t bd) {
    __m256i       in[64 * 64 / 8], out[64 * 64 / 8];
    const int8_t* shift   = svt_aom_inv_txfm_shift_ls[TX_64X64];
    const int32_t txw_idx = tx_size_wide_log2[TX_64X64] - tx_size_wide_log2[0];
    const int32_t txh_idx = tx_size_high_log2[TX_64X64] - tx_size_high_log2[0];

    switch (tx_type) {
    case DCT_DCT:
        load_buffer_64x64_lower_32x32_avx2(input, in);
        transpose_8nx8n(in, out, 64, 64);
        idct64x64_avx2(out, in, inv_cos_bit_row[txw_idx][txh_idx], 0, bd);
        transpose_8nx8n(in, out, 64, 64);
        round_shift_64x64_avx2(out, -shift[0]);
        idct64x64_avx2(out, in, inv_cos_bit_col[txw_idx][txh_idx], 1, bd);
        round_shift_64x64_avx2(in, -shift[1]);
        write_buffer_64x64_avx2(in, output_r, stride_r, output_w, stride_w, bd);
        break;

    default:
        svt_av1_inv_txfm2d_add_64x64_c(input, output_r, stride_r, output_w, stride_w, tx_type, bd);
        break;
    }
}

void svt_dav1d_inv_txfm2d_add_4x4_avx2(const int32_t* input, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                                       int32_t stride_w, TxType tx_type, int32_t bd) {
    DECLARE_ALIGNED(32, int32_t, coeff[MAX_TX_SQUARE]);

    //transpose coeff
    transpose_32bit_4x4((__m128i*)input, (__m128i*)coeff);

    if (output_r != output_w) {
        for (int32_t i = 0; i < tx_size_high[TX_4X4]; i++) {
            ((uint64_t*)(output_w + i * stride_w))[0] = ((uint64_t*)(output_r + i * stride_r))[0];
        }
    }

    switch (tx_type) {
    case DCT_DCT:
        svt_dav1d_inv_txfm_add_dct_dct_4x4_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case DCT_ADST:
        svt_dav1d_inv_txfm_add_adst_dct_4x4_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case ADST_DCT:
        svt_dav1d_inv_txfm_add_dct_adst_4x4_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case ADST_ADST:
        svt_dav1d_inv_txfm_add_adst_adst_4x4_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case DCT_FLIPADST:
        svt_dav1d_inv_txfm_add_flipadst_dct_4x4_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case FLIPADST_DCT:
        svt_dav1d_inv_txfm_add_dct_flipadst_4x4_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case FLIPADST_FLIPADST:
        svt_dav1d_inv_txfm_add_flipadst_flipadst_4x4_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case FLIPADST_ADST:
        svt_dav1d_inv_txfm_add_adst_flipadst_4x4_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case ADST_FLIPADST:
        svt_dav1d_inv_txfm_add_flipadst_adst_4x4_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case IDTX:
        svt_dav1d_inv_txfm_add_identity_identity_4x4_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case H_DCT:
        svt_dav1d_inv_txfm_add_dct_identity_4x4_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case V_DCT:
        svt_dav1d_inv_txfm_add_identity_dct_4x4_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    default:
        svt_av1_inv_txfm2d_add_4x4_c(input, output_r, stride_r, output_w, stride_w, tx_type, bd);
    }

    if (bd == 8) {
        __m128i max_val = _mm_set1_epi16(255);
        for (int32_t i = 0; i < tx_size_high[TX_4X4]; i++) {
            _mm_storel_epi64((__m128i*)(output_w + i * stride_w),
                             _mm_min_epi16(_mm_loadl_epi64((__m128i*)(output_w + i * stride_w)), max_val));
        }
    }
}

void svt_dav1d_inv_txfm2d_add_4x8_avx2(const int32_t* input, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                                       int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t bd) {
    DECLARE_ALIGNED(32, int32_t, coeff[MAX_TX_SQUARE]);

    //transpose coeff
    int32_t w = tx_size_wide[TX_4X8];
    int32_t h = tx_size_high[TX_4X8];
    h         = h > 32 ? 32 : h;
    w         = w > 32 ? 32 : w;
    for (int32_t r = 0; r < h; ++r) {
        for (int32_t c = 0; c < w; ++c) {
            coeff[c * h + r] = input[r * w + c];
        }
    }

    if (output_r != output_w) {
        for (int32_t i = 0; i < tx_size_high[TX_4X8]; i++) {
            ((uint64_t*)(output_w + i * stride_w))[0] = ((uint64_t*)(output_r + i * stride_r))[0];
        }
    }

    switch (tx_type) {
    case DCT_DCT:
        svt_dav1d_inv_txfm_add_dct_dct_4x8_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case DCT_ADST:
        svt_dav1d_inv_txfm_add_adst_dct_4x8_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case ADST_DCT:
        svt_dav1d_inv_txfm_add_dct_adst_4x8_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case ADST_ADST:
        svt_dav1d_inv_txfm_add_adst_adst_4x8_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case DCT_FLIPADST:
        svt_dav1d_inv_txfm_add_flipadst_dct_4x8_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case FLIPADST_DCT:
        svt_dav1d_inv_txfm_add_dct_flipadst_4x8_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case FLIPADST_FLIPADST:
        svt_dav1d_inv_txfm_add_flipadst_flipadst_4x8_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case FLIPADST_ADST:
        svt_dav1d_inv_txfm_add_adst_flipadst_4x8_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case ADST_FLIPADST:
        svt_dav1d_inv_txfm_add_flipadst_adst_4x8_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case IDTX:
        svt_dav1d_inv_txfm_add_identity_identity_4x8_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case H_DCT:
        svt_dav1d_inv_txfm_add_dct_identity_4x8_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case V_DCT:
        svt_dav1d_inv_txfm_add_identity_dct_4x8_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    default:
        svt_av1_inv_txfm2d_add_4x8_c(input, output_r, stride_r, output_w, stride_w, tx_type, tx_size, bd);
    }

    if (bd == 8) {
        __m128i max_val = _mm_set1_epi16(255);
        for (int32_t i = 0; i < tx_size_high[TX_4X8]; i++) {
            _mm_storel_epi64((__m128i*)(output_w + i * stride_w),
                             _mm_min_epi16(_mm_loadl_epi64((__m128i*)(output_w + i * stride_w)), max_val));
        }
    }
}

void svt_dav1d_inv_txfm2d_add_4x16_avx2(const int32_t* input, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                                        int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t bd) {
    DECLARE_ALIGNED(32, int32_t, coeff[MAX_TX_SQUARE]);

    //transpose coeff
    int32_t w = tx_size_wide[TX_4X16];
    int32_t h = tx_size_high[TX_4X16];
    h         = h > 32 ? 32 : h;
    w         = w > 32 ? 32 : w;
    for (int32_t r = 0; r < h; ++r) {
        for (int32_t c = 0; c < w; ++c) {
            coeff[c * h + r] = input[r * w + c];
        }
    }

    if (output_r != output_w) {
        for (int32_t i = 0; i < tx_size_high[TX_4X16]; i++) {
            ((uint64_t*)(output_w + i * stride_w))[0] = ((uint64_t*)(output_r + i * stride_r))[0];
        }
    }

    switch (tx_type) {
    case DCT_DCT:
        svt_dav1d_inv_txfm_add_dct_dct_4x16_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case DCT_ADST:
        svt_dav1d_inv_txfm_add_adst_dct_4x16_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case ADST_DCT:
        svt_dav1d_inv_txfm_add_dct_adst_4x16_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case ADST_ADST:
        svt_dav1d_inv_txfm_add_adst_adst_4x16_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case DCT_FLIPADST:
        svt_dav1d_inv_txfm_add_flipadst_dct_4x16_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case FLIPADST_DCT:
        svt_dav1d_inv_txfm_add_dct_flipadst_4x16_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case FLIPADST_FLIPADST:
        svt_dav1d_inv_txfm_add_flipadst_flipadst_4x16_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case FLIPADST_ADST:
        svt_dav1d_inv_txfm_add_adst_flipadst_4x16_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case ADST_FLIPADST:
        svt_dav1d_inv_txfm_add_flipadst_adst_4x16_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case IDTX:
        svt_dav1d_inv_txfm_add_identity_identity_4x16_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case H_DCT:
        svt_dav1d_inv_txfm_add_dct_identity_4x16_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case V_DCT:
        svt_dav1d_inv_txfm_add_identity_dct_4x16_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    default:
        svt_av1_inv_txfm2d_add_4x16_c(input, output_r, stride_r, output_w, stride_w, tx_type, tx_size, bd);
    }

    if (bd == 8) {
        __m128i max_val = _mm_set1_epi16(255);
        for (int32_t i = 0; i < tx_size_high[TX_4X16]; i++) {
            _mm_storel_epi64((__m128i*)(output_w + i * stride_w),
                             _mm_min_epi16(_mm_loadl_epi64((__m128i*)(output_w + i * stride_w)), max_val));
        }
    }
}

void svt_dav1d_inv_txfm2d_add_8x4_avx2(const int32_t* input, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                                       int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t bd) {
    DECLARE_ALIGNED(32, int32_t, coeff[MAX_TX_SQUARE]);

    //transpose coeff
    int32_t w = tx_size_wide[TX_8X4];
    int32_t h = tx_size_high[TX_8X4];
    h         = h > 32 ? 32 : h;
    w         = w > 32 ? 32 : w;
    for (int32_t r = 0; r < h; ++r) {
        for (int32_t c = 0; c < w; ++c) {
            coeff[c * h + r] = input[r * w + c];
        }
    }

    if (output_r != output_w) {
        for (int32_t i = 0; i < tx_size_high[TX_8X4]; i++) {
            _mm_storeu_si128((__m128i*)(output_w + i * stride_w), _mm_loadu_si128((__m128i*)(output_r + i * stride_r)));
        }
    }

    switch (tx_type) {
    case DCT_DCT:
        svt_dav1d_inv_txfm_add_dct_dct_8x4_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case DCT_ADST:
        svt_dav1d_inv_txfm_add_adst_dct_8x4_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case ADST_DCT:
        svt_dav1d_inv_txfm_add_dct_adst_8x4_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case ADST_ADST:
        svt_dav1d_inv_txfm_add_adst_adst_8x4_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case DCT_FLIPADST:
        svt_dav1d_inv_txfm_add_flipadst_dct_8x4_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case FLIPADST_DCT:
        svt_dav1d_inv_txfm_add_dct_flipadst_8x4_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case FLIPADST_FLIPADST:
        svt_dav1d_inv_txfm_add_flipadst_flipadst_8x4_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case FLIPADST_ADST:
        svt_dav1d_inv_txfm_add_adst_flipadst_8x4_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case ADST_FLIPADST:
        svt_dav1d_inv_txfm_add_flipadst_adst_8x4_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case IDTX:
        svt_dav1d_inv_txfm_add_identity_identity_8x4_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case H_DCT:
        svt_dav1d_inv_txfm_add_dct_identity_8x4_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case V_DCT:
        svt_dav1d_inv_txfm_add_identity_dct_8x4_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    default:
        svt_av1_inv_txfm2d_add_8x4_c(input, output_r, stride_r, output_w, stride_w, tx_type, tx_size, bd);
    }

    if (bd == 8) {
        __m128i max_val = _mm_set1_epi16(255);
        for (int32_t i = 0; i < tx_size_high[TX_8X4]; i++) {
            _mm_storeu_si128((__m128i*)(output_w + i * stride_w),
                             _mm_min_epi16(_mm_loadu_si128((__m128i*)(output_w + i * stride_w)), max_val));
        }
    }
}

void svt_dav1d_inv_txfm2d_add_16x4_avx2(const int32_t* input, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                                        int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t bd) {
    DECLARE_ALIGNED(32, int32_t, coeff[MAX_TX_SQUARE]);

    //transpose coeff
    int32_t w = tx_size_wide[TX_16X4];
    int32_t h = tx_size_high[TX_16X4];
    h         = h > 32 ? 32 : h;
    w         = w > 32 ? 32 : w;
    for (int32_t r = 0; r < h; ++r) {
        for (int32_t c = 0; c < w; ++c) {
            coeff[c * h + r] = input[r * w + c];
        }
    }

    if (output_r != output_w) {
        for (int32_t i = 0; i < tx_size_high[TX_16X4]; i++) {
            _mm256_storeu_si256((__m256i*)(output_w + i * stride_w),
                                _mm256_loadu_si256((__m256i*)(output_r + i * stride_r)));
        }
    }

    switch (tx_type) {
    case DCT_DCT:
        svt_dav1d_inv_txfm_add_dct_dct_16x4_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case DCT_ADST:
        svt_dav1d_inv_txfm_add_adst_dct_16x4_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case ADST_DCT:
        svt_dav1d_inv_txfm_add_dct_adst_16x4_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case ADST_ADST:
        svt_dav1d_inv_txfm_add_adst_adst_16x4_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case DCT_FLIPADST:
        svt_dav1d_inv_txfm_add_flipadst_dct_16x4_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case FLIPADST_DCT:
        svt_dav1d_inv_txfm_add_dct_flipadst_16x4_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case FLIPADST_FLIPADST:
        svt_dav1d_inv_txfm_add_flipadst_flipadst_16x4_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case FLIPADST_ADST:
        svt_dav1d_inv_txfm_add_adst_flipadst_16x4_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case ADST_FLIPADST:
        svt_dav1d_inv_txfm_add_flipadst_adst_16x4_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case IDTX:
        svt_dav1d_inv_txfm_add_identity_identity_16x4_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case H_DCT:
        svt_dav1d_inv_txfm_add_dct_identity_16x4_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case V_DCT:
        svt_dav1d_inv_txfm_add_identity_dct_16x4_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    default:
        svt_av1_inv_txfm2d_add_16x4_c(input, output_r, stride_r, output_w, stride_w, tx_type, tx_size, bd);
    }

    if (bd == 8) {
        __m256i max_val = _mm256_set1_epi16(255);
        for (int32_t i = 0; i < tx_size_high[TX_16X4]; i++) {
            _mm256_storeu_si256((__m256i*)(output_w + i * stride_w),
                                _mm256_min_epi16(_mm256_loadu_si256((__m256i*)(output_w + i * stride_w)), max_val));
        }
    }
}

void svt_dav1d_inv_txfm2d_add_8x8_avx2(const int32_t* input, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                                       int32_t stride_w, TxType tx_type, int32_t bd) {
    DECLARE_ALIGNED(32, int32_t, coeff[MAX_TX_SQUARE]);

    //transpose coeff
    transpose_8x8_avx2((__m256i*)input, (__m256i*)coeff);

    if (output_r != output_w) {
        for (int32_t i = 0; i < tx_size_high[TX_8X8]; i++) {
            _mm_storeu_si128((__m128i*)(output_w + i * stride_w), _mm_loadu_si128((__m128i*)(output_r + i * stride_r)));
        }
    }

    switch (tx_type) {
    case DCT_DCT:
        svt_dav1d_inv_txfm_add_dct_dct_8x8_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case DCT_ADST:
        svt_dav1d_inv_txfm_add_adst_dct_8x8_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case ADST_DCT:
        svt_dav1d_inv_txfm_add_dct_adst_8x8_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case ADST_ADST:
        svt_dav1d_inv_txfm_add_adst_adst_8x8_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case DCT_FLIPADST:
        svt_dav1d_inv_txfm_add_flipadst_dct_8x8_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case FLIPADST_DCT:
        svt_dav1d_inv_txfm_add_dct_flipadst_8x8_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case FLIPADST_FLIPADST:
        svt_dav1d_inv_txfm_add_flipadst_flipadst_8x8_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case FLIPADST_ADST:
        svt_dav1d_inv_txfm_add_adst_flipadst_8x8_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case ADST_FLIPADST:
        svt_dav1d_inv_txfm_add_flipadst_adst_8x8_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case IDTX:
        svt_dav1d_inv_txfm_add_identity_identity_8x8_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case H_DCT:
        svt_dav1d_inv_txfm_add_dct_identity_8x8_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case V_DCT:
        svt_dav1d_inv_txfm_add_identity_dct_8x8_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    default:
        svt_av1_inv_txfm2d_add_8x8_c(input, output_r, stride_r, output_w, stride_w, tx_type, bd);
    }

    if (bd == 8) {
        __m128i max_val = _mm_set1_epi16(255);
        for (int32_t i = 0; i < tx_size_high[TX_8X8]; i++) {
            _mm_storeu_si128((__m128i*)(output_w + i * stride_w),
                             _mm_min_epi16(_mm_loadu_si128((__m128i*)(output_w + i * stride_w)), max_val));
        }
    }
}

void svt_dav1d_inv_txfm2d_add_16x16_avx2(const int32_t* input, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                                         int32_t stride_w, TxType tx_type, int32_t bd) {
    DECLARE_ALIGNED(32, int32_t, coeff[MAX_TX_SQUARE]);

    //transpose coeff
    transpose_16x16_avx2((__m256i*)input, (__m256i*)coeff);

    if (output_r != output_w) {
        for (int32_t i = 0; i < tx_size_high[TX_16X16]; i++) {
            _mm256_storeu_si256((__m256i*)(output_w + i * stride_w),
                                _mm256_loadu_si256((__m256i*)(output_r + i * stride_r)));
        }
    }

    switch (tx_type) {
    case DCT_DCT:
        svt_dav1d_inv_txfm_add_dct_dct_16x16_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case DCT_ADST:
        svt_dav1d_inv_txfm_add_adst_dct_16x16_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case ADST_DCT:
        svt_dav1d_inv_txfm_add_dct_adst_16x16_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case ADST_ADST:
        svt_dav1d_inv_txfm_add_adst_adst_16x16_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case DCT_FLIPADST:
        svt_dav1d_inv_txfm_add_flipadst_dct_16x16_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case FLIPADST_DCT:
        svt_dav1d_inv_txfm_add_dct_flipadst_16x16_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case FLIPADST_FLIPADST:
        svt_dav1d_inv_txfm_add_flipadst_flipadst_16x16_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case FLIPADST_ADST:
        svt_dav1d_inv_txfm_add_adst_flipadst_16x16_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case ADST_FLIPADST:
        svt_dav1d_inv_txfm_add_flipadst_adst_16x16_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case IDTX:
        svt_dav1d_inv_txfm_add_identity_identity_16x16_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case H_DCT:
        svt_dav1d_inv_txfm_add_dct_identity_16x16_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    case V_DCT:
        svt_dav1d_inv_txfm_add_identity_dct_16x16_10bpc_avx2(output_w, stride_w * 2, coeff, 256 /*eob*/, bd);
        break;
    default:
        svt_av1_inv_txfm2d_add_16x16_c(input, output_r, stride_r, output_w, stride_w, tx_type, bd);
    }

    if (bd == 8) {
        __m256i max_val = _mm256_set1_epi16(255);
        for (int32_t i = 0; i < tx_size_high[TX_16X16]; i++) {
            _mm256_storeu_si256((__m256i*)(output_w + i * stride_w),
                                _mm256_min_epi16(_mm256_loadu_si256((__m256i*)(output_w + i * stride_w)), max_val));
        }
    }
}

void svt_dav1d_inv_txfm2d_add_32x32_avx2(const int32_t* input, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                                         int32_t stride_w, TxType tx_type, int32_t bd) {
    DECLARE_ALIGNED(32, int32_t, coeff[MAX_TX_SQUARE]);

    //transpose coeff
    transpose_32x32((__m256i*)input, (__m256i*)coeff);

    if (output_r != output_w) {
        for (int32_t i = 0; i < tx_size_high[TX_32X32]; i++) {
            _mm256_storeu_si256((__m256i*)(output_w + i * stride_w),
                                _mm256_loadu_si256((__m256i*)(output_r + i * stride_r)));
            _mm256_storeu_si256((__m256i*)(output_w + i * stride_w + 16),
                                _mm256_loadu_si256((__m256i*)(output_r + i * stride_r + 16)));
        }
    }

    switch (tx_type) {
    case DCT_DCT:
        svt_dav1d_inv_txfm_add_dct_dct_32x32_10bpc_avx2(output_w, stride_w * 2, coeff, 1024 /*eob*/, bd);
        break;
    case IDTX:
        svt_dav1d_inv_txfm_add_identity_identity_32x32_10bpc_avx2(output_w, stride_w * 2, coeff, 1024 /*eob*/, bd);
        break;
    default:
        svt_av1_inv_txfm2d_add_32x32_c(input, output_r, stride_r, output_w, stride_w, tx_type, bd);
        break;
    }

    if (bd == 8) {
        __m256i max_val = _mm256_set1_epi16(255);
        for (int32_t i = 0; i < tx_size_high[TX_32X32]; i++) {
            _mm256_storeu_si256((__m256i*)(output_w + i * stride_w),
                                _mm256_min_epi16(_mm256_loadu_si256((__m256i*)(output_w + i * stride_w)), max_val));
            _mm256_storeu_si256(
                (__m256i*)(output_w + i * stride_w + 16),
                _mm256_min_epi16(_mm256_loadu_si256((__m256i*)(output_w + i * stride_w + 16)), max_val));
        }
    }
}

void svt_dav1d_inv_txfm2d_add_64x64_avx2(const int32_t* input, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                                         int32_t stride_w, TxType tx_type, int32_t bd) {
    DECLARE_ALIGNED(32, int32_t, coeff[MAX_TX_SQUARE]);

    //transpose coeff
    transpose_32x32((__m256i*)input, (__m256i*)coeff);

    if (output_r != output_w) {
        for (int32_t i = 0; i < tx_size_high[TX_64X64]; i++) {
            _mm256_storeu_si256((__m256i*)(output_w + i * stride_w),
                                _mm256_loadu_si256((__m256i*)(output_r + i * stride_r)));
            _mm256_storeu_si256((__m256i*)(output_w + i * stride_w + 16),
                                _mm256_loadu_si256((__m256i*)(output_r + i * stride_r + 16)));
            _mm256_storeu_si256((__m256i*)(output_w + i * stride_w + 32),
                                _mm256_loadu_si256((__m256i*)(output_r + i * stride_r + 32)));
            _mm256_storeu_si256((__m256i*)(output_w + i * stride_w + 48),
                                _mm256_loadu_si256((__m256i*)(output_r + i * stride_r + 48)));
        }
    }

    switch (tx_type) {
    case DCT_DCT:
        svt_dav1d_inv_txfm_add_dct_dct_64x64_10bpc_avx2(output_w, stride_w * 2, coeff, 1024 /*eob*/, bd);
        break;
    default:
        svt_av1_inv_txfm2d_add_64x64_c(input, output_r, stride_r, output_w, stride_w, tx_type, bd);
        break;
    }

    if (bd == 8) {
        __m256i max_val = _mm256_set1_epi16(255);
        for (int32_t i = 0; i < tx_size_high[TX_64X64]; i++) {
            _mm256_storeu_si256((__m256i*)(output_w + i * stride_w),
                                _mm256_min_epi16(_mm256_loadu_si256((__m256i*)(output_w + i * stride_w)), max_val));
            _mm256_storeu_si256(
                (__m256i*)(output_w + i * stride_w + 16),
                _mm256_min_epi16(_mm256_loadu_si256((__m256i*)(output_w + i * stride_w + 16)), max_val));
            _mm256_storeu_si256(
                (__m256i*)(output_w + i * stride_w + 32),
                _mm256_min_epi16(_mm256_loadu_si256((__m256i*)(output_w + i * stride_w + 32)), max_val));
            _mm256_storeu_si256(
                (__m256i*)(output_w + i * stride_w + 48),
                _mm256_min_epi16(_mm256_loadu_si256((__m256i*)(output_w + i * stride_w + 48)), max_val));
        }
    }
}

void svt_av1_inv_txf_add_8x16_dav1d_hbd(int32_t* dqcoeff, uint16_t* dst, int32_t stride, TxType tx_type, int eob,
                                        int32_t bd) {
    switch (tx_type) {
    case DCT_DCT:
        svt_dav1d_inv_txfm_add_dct_dct_8x16_10bpc_avx2(dst, stride, dqcoeff, eob, bd);
        break;
    case DCT_ADST:
        svt_dav1d_inv_txfm_add_adst_dct_8x16_10bpc_avx2(dst, stride, dqcoeff, eob, bd);
        break;
    case ADST_DCT:
        svt_dav1d_inv_txfm_add_dct_adst_8x16_10bpc_avx2(dst, stride, dqcoeff, eob, bd);
        break;
    case ADST_ADST:
        svt_dav1d_inv_txfm_add_adst_adst_8x16_10bpc_avx2(dst, stride, dqcoeff, eob, bd);
        break;
    case DCT_FLIPADST:
        svt_dav1d_inv_txfm_add_flipadst_dct_8x16_10bpc_avx2(dst, stride, dqcoeff, eob, bd);
        break;
    case FLIPADST_DCT:
        svt_dav1d_inv_txfm_add_dct_flipadst_8x16_10bpc_avx2(dst, stride, dqcoeff, eob, bd);
        break;
    case FLIPADST_FLIPADST:
        svt_dav1d_inv_txfm_add_flipadst_flipadst_8x16_10bpc_avx2(dst, stride, dqcoeff, eob, bd);
        break;
    case FLIPADST_ADST:
        svt_dav1d_inv_txfm_add_adst_flipadst_8x16_10bpc_avx2(dst, stride, dqcoeff, eob, bd);
        break;
    case ADST_FLIPADST:
        svt_dav1d_inv_txfm_add_flipadst_adst_8x16_10bpc_avx2(dst, stride, dqcoeff, eob, bd);
        break;
    case IDTX:
        svt_dav1d_inv_txfm_add_identity_identity_8x16_10bpc_avx2(dst, stride, dqcoeff, eob, bd);
        break;
    case H_DCT:
        svt_dav1d_inv_txfm_add_dct_identity_8x16_10bpc_avx2(dst, stride, dqcoeff, eob, bd);
        break;
    case V_DCT:
        svt_dav1d_inv_txfm_add_identity_dct_8x16_10bpc_avx2(dst, stride, dqcoeff, eob, bd);
        break;
    case H_ADST:
        svt_dav1d_inv_txfm_add_adst_identity_8x16_10bpc_avx2(dst, stride, dqcoeff, eob, bd);
        break;
    case V_ADST:
        svt_dav1d_inv_txfm_add_identity_adst_8x16_10bpc_avx2(dst, stride, dqcoeff, eob, bd);
        break;
    case H_FLIPADST:
        svt_dav1d_inv_txfm_add_flipadst_identity_8x16_10bpc_avx2(dst, stride, dqcoeff, eob, bd);
        break;
    case V_FLIPADST:
        svt_dav1d_inv_txfm_add_identity_flipadst_8x16_10bpc_avx2(dst, stride, dqcoeff, eob, bd);
        break;
    default:
        assert(0);
    }
}

void svt_av1_inv_txf_add_16x8_dav1d_hbd(int32_t* dqcoeff, uint16_t* dst, int32_t stride, TxType tx_type, int eob,
                                        int32_t bd) {
    switch (tx_type) {
    case DCT_DCT:
        svt_dav1d_inv_txfm_add_dct_dct_16x8_10bpc_avx2(dst, stride, dqcoeff, eob, bd);
        break;
    case DCT_ADST:
        svt_dav1d_inv_txfm_add_adst_dct_16x8_10bpc_avx2(dst, stride, dqcoeff, eob, bd);
        break;
    case ADST_DCT:
        svt_dav1d_inv_txfm_add_dct_adst_16x8_10bpc_avx2(dst, stride, dqcoeff, eob, bd);
        break;
    case ADST_ADST:
        svt_dav1d_inv_txfm_add_adst_adst_16x8_10bpc_avx2(dst, stride, dqcoeff, eob, bd);
        break;
    case DCT_FLIPADST:
        svt_dav1d_inv_txfm_add_flipadst_dct_16x8_10bpc_avx2(dst, stride, dqcoeff, eob, bd);
        break;
    case FLIPADST_DCT:
        svt_dav1d_inv_txfm_add_dct_flipadst_16x8_10bpc_avx2(dst, stride, dqcoeff, eob, bd);
        break;
    case FLIPADST_FLIPADST:
        svt_dav1d_inv_txfm_add_flipadst_flipadst_16x8_10bpc_avx2(dst, stride, dqcoeff, eob, bd);
        break;
    case FLIPADST_ADST:
        svt_dav1d_inv_txfm_add_adst_flipadst_16x8_10bpc_avx2(dst, stride, dqcoeff, eob, bd);
        break;
    case ADST_FLIPADST:
        svt_dav1d_inv_txfm_add_flipadst_adst_16x8_10bpc_avx2(dst, stride, dqcoeff, eob, bd);
        break;
    case IDTX:
        svt_dav1d_inv_txfm_add_identity_identity_16x8_10bpc_avx2(dst, stride, dqcoeff, eob, bd);
        break;
    case H_DCT:
        svt_dav1d_inv_txfm_add_dct_identity_16x8_10bpc_avx2(dst, stride, dqcoeff, eob, bd);
        break;
    case V_DCT:
        svt_dav1d_inv_txfm_add_identity_dct_16x8_10bpc_avx2(dst, stride, dqcoeff, eob, bd);
        break;
    case H_ADST:
        svt_dav1d_inv_txfm_add_adst_identity_16x8_10bpc_avx2(dst, stride, dqcoeff, eob, bd);
        break;
    case V_ADST:
        svt_dav1d_inv_txfm_add_identity_adst_16x8_10bpc_avx2(dst, stride, dqcoeff, eob, bd);
        break;
    case H_FLIPADST:
        svt_dav1d_inv_txfm_add_flipadst_identity_16x8_10bpc_avx2(dst, stride, dqcoeff, eob, bd);
        break;
    case V_FLIPADST:
        svt_dav1d_inv_txfm_add_identity_flipadst_16x8_10bpc_avx2(dst, stride, dqcoeff, eob, bd);
        break;
    default:
        assert(0);
    }
}

void svt_dav1d_highbd_inv_txfm_add_avx2(const int32_t* input, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                                        int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t eob, int32_t bd) {
    //assert(av1_ext_tx_used[txfm_param->tx_set_type][txfm_param->tx_type]);

    DECLARE_ALIGNED(32, int32_t, coeff[MAX_TX_SQUARE]);

    //transpose coeff
    int32_t w = tx_size_wide[tx_size];
    int32_t h = tx_size_high[tx_size];
    h         = h > 32 ? 32 : h;
    w         = w > 32 ? 32 : w;
    for (int32_t r = 0; r < h; ++r) {
        for (int32_t c = 0; c < w; ++c) {
            coeff[c * h + r] = input[r * w + c];
        }
    }

    if (output_r != output_w) {
        switch (tx_size) {
        case TX_8X16:
        case TX_8X32:
            for (int32_t i = 0; i < tx_size_high[tx_size]; i++) {
                _mm_storeu_si128((__m128i*)(output_w + i * stride_w),
                                 _mm_loadu_si128((__m128i*)(output_r + i * stride_r)));
            }
            break;
        case TX_16X8:
        case TX_16X32:
        case TX_16X64:
            for (int32_t i = 0; i < tx_size_high[tx_size]; i++) {
                _mm256_storeu_si256((__m256i*)(output_w + i * stride_w),
                                    _mm256_loadu_si256((__m256i*)(output_r + i * stride_r)));
            }
            break;
        case TX_32X8:
        case TX_32X16:
        case TX_32X64:
            for (int32_t i = 0; i < tx_size_high[tx_size]; i++) {
                _mm256_storeu_si256((__m256i*)(output_w + i * stride_w),
                                    _mm256_loadu_si256((__m256i*)(output_r + i * stride_r)));
                _mm256_storeu_si256((__m256i*)(output_w + i * stride_w + 16),
                                    _mm256_loadu_si256((__m256i*)(output_r + i * stride_r + 16)));
            }
            break;
        case TX_64X16:
        case TX_64X32:
            for (int32_t i = 0; i < tx_size_high[tx_size]; i++) {
                _mm256_storeu_si256((__m256i*)(output_w + i * stride_w),
                                    _mm256_loadu_si256((__m256i*)(output_r + i * stride_r)));
                _mm256_storeu_si256((__m256i*)(output_w + i * stride_w + 16),
                                    _mm256_loadu_si256((__m256i*)(output_r + i * stride_r + 16)));
                _mm256_storeu_si256((__m256i*)(output_w + i * stride_w + 32),
                                    _mm256_loadu_si256((__m256i*)(output_r + i * stride_r + 32)));
                _mm256_storeu_si256((__m256i*)(output_w + i * stride_w + 48),
                                    _mm256_loadu_si256((__m256i*)(output_r + i * stride_r + 48)));
            }
            break;
        default:
            assert(0);
        }
    }

    switch (tx_size) {
    case TX_8X16:
        svt_av1_inv_txf_add_8x16_dav1d_hbd(coeff, output_w, stride_w * 2, tx_type, eob, bd);
        break;
    case TX_8X32:
        switch (tx_type) {
        case DCT_DCT:
            svt_dav1d_inv_txfm_add_dct_dct_8x32_10bpc_avx2(output_w, stride_w * 2, coeff, eob, bd);
            break;
        case IDTX:
            svt_dav1d_inv_txfm_add_identity_identity_8x32_10bpc_avx2(output_w, stride_w * 2, coeff, eob, bd);
            break;
        default:
            svt_av1_highbd_inv_txfm2d_add_universe_avx2(
                input, output_r, stride_r, output_w, stride_w, tx_type, tx_size, eob, bd);
        }
        break;
    case TX_16X8:
        svt_av1_inv_txf_add_16x8_dav1d_hbd(coeff, output_w, stride_w * 2, tx_type, eob, bd);
        break;
    case TX_16X32:
        switch (tx_type) {
        case DCT_DCT:
            svt_dav1d_inv_txfm_add_dct_dct_16x32_10bpc_avx2(output_w, stride_w * 2, coeff, eob, bd);
            break;
        case IDTX:
            svt_dav1d_inv_txfm_add_identity_identity_16x32_10bpc_avx2(output_w, stride_w * 2, coeff, eob, bd);
            break;
        default:
            svt_av1_highbd_inv_txfm2d_add_universe_avx2(
                input, output_r, stride_r, output_w, stride_w, tx_type, tx_size, eob, bd);
        }
        break;
    case TX_16X64:
        if (tx_type == DCT_DCT) {
            svt_dav1d_inv_txfm_add_dct_dct_16x64_10bpc_avx2(output_w, stride_w * 2, coeff, eob, bd);
        } else {
            svt_av1_highbd_inv_txfm2d_add_universe_avx2(
                input, output_r, stride_r, output_w, stride_w, tx_type, tx_size, eob, bd);
        }
        break;
    case TX_32X8:
        switch (tx_type) {
        case DCT_DCT:
            svt_dav1d_inv_txfm_add_dct_dct_32x8_10bpc_avx2(output_w, stride_w * 2, coeff, eob, bd);
            break;
        case IDTX:
            svt_dav1d_inv_txfm_add_identity_identity_32x8_10bpc_avx2(output_w, stride_w * 2, coeff, eob, bd);
            break;
        default:
            svt_av1_highbd_inv_txfm2d_add_universe_avx2(
                input, output_r, stride_r, output_w, stride_w, tx_type, tx_size, eob, bd);
        }
        break;
    case TX_32X16:
        switch (tx_type) {
        case DCT_DCT:
            svt_dav1d_inv_txfm_add_dct_dct_32x16_10bpc_avx2(output_w, stride_w * 2, coeff, eob, bd);
            break;
        case IDTX:
            svt_dav1d_inv_txfm_add_identity_identity_32x16_10bpc_avx2(output_w, stride_w * 2, coeff, eob, bd);
            break;
        default:
            svt_av1_highbd_inv_txfm2d_add_universe_avx2(
                input, output_r, stride_r, output_w, stride_w, tx_type, tx_size, eob, bd);
        }
        break;
    case TX_32X64:
        if (tx_type == DCT_DCT) {
            svt_dav1d_inv_txfm_add_dct_dct_32x64_10bpc_avx2(output_w, stride_w * 2, coeff, eob, bd);
        } else {
            svt_av1_highbd_inv_txfm2d_add_universe_avx2(
                input, output_r, stride_r, output_w, stride_w, tx_type, tx_size, eob, bd);
        }
        break;
    case TX_64X16:
        if (tx_type == DCT_DCT) {
            svt_dav1d_inv_txfm_add_dct_dct_64x16_10bpc_avx2(output_w, stride_w * 2, coeff, eob, bd);
        } else {
            svt_av1_highbd_inv_txfm2d_add_universe_avx2(
                input, output_r, stride_r, output_w, stride_w, tx_type, tx_size, eob, bd);
        }
        break;
    case TX_64X32:
        if (tx_type == DCT_DCT) {
            svt_dav1d_inv_txfm_add_dct_dct_64x32_10bpc_avx2(output_w, stride_w * 2, coeff, eob, bd);
        } else {
            svt_av1_highbd_inv_txfm2d_add_universe_avx2(
                input, output_r, stride_r, output_w, stride_w, tx_type, tx_size, eob, bd);
        }
        break;
    default:
        svt_av1_highbd_inv_txfm2d_add_universe_avx2(
            input, output_r, stride_r, output_w, stride_w, tx_type, tx_size, eob, bd);
        return;
    }

    if (bd == 8) {
        __m256i max_val = _mm256_set1_epi16(255);
        switch (tx_size) {
        case TX_8X16:
        case TX_8X32:
            for (int32_t i = 0; i < tx_size_high[tx_size]; i++) {
                _mm_storeu_si128(
                    (__m128i*)(output_w + i * stride_w),
                    _mm_min_epi16(_mm_loadu_si128((__m128i*)(output_w + i * stride_w)), _mm_set1_epi16(255)));
            }
            break;
        case TX_16X8:
        case TX_16X32:
        case TX_16X64:
            for (int32_t i = 0; i < tx_size_high[tx_size]; i++) {
                _mm256_storeu_si256((__m256i*)(output_w + i * stride_w),
                                    _mm256_min_epi16(_mm256_loadu_si256((__m256i*)(output_w + i * stride_w)), max_val));
            }
            break;
        case TX_32X8:
        case TX_32X16:
        case TX_32X64:
            for (int32_t i = 0; i < tx_size_high[tx_size]; i++) {
                _mm256_storeu_si256((__m256i*)(output_w + i * stride_w),
                                    _mm256_min_epi16(_mm256_loadu_si256((__m256i*)(output_w + i * stride_w)), max_val));
                _mm256_storeu_si256(
                    (__m256i*)(output_w + i * stride_w + 16),
                    _mm256_min_epi16(_mm256_loadu_si256((__m256i*)(output_w + i * stride_w + 16)), max_val));
            }
            break;
        case TX_64X16:
        case TX_64X32:
            for (int32_t i = 0; i < tx_size_high[tx_size]; i++) {
                _mm256_storeu_si256((__m256i*)(output_w + i * stride_w),
                                    _mm256_min_epi16(_mm256_loadu_si256((__m256i*)(output_w + i * stride_w)), max_val));
                _mm256_storeu_si256(
                    (__m256i*)(output_w + i * stride_w + 16),
                    _mm256_min_epi16(_mm256_loadu_si256((__m256i*)(output_w + i * stride_w + 16)), max_val));
                _mm256_storeu_si256(
                    (__m256i*)(output_w + i * stride_w + 32),
                    _mm256_min_epi16(_mm256_loadu_si256((__m256i*)(output_w + i * stride_w + 32)), max_val));
                _mm256_storeu_si256(
                    (__m256i*)(output_w + i * stride_w + 48),
                    _mm256_min_epi16(_mm256_loadu_si256((__m256i*)(output_w + i * stride_w + 48)), max_val));
            }
            break;
        default:
            assert(0);
        }
    }
}
