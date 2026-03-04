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

#include <immintrin.h>
#include "common_dsp_rtcd.h"
#include "bitstream_unit.h"
#include "cdef.h"
#include "definitions.h"
#include "memory_avx2.h"

/* partial A is a 16-bit vector of the form:
 [x8 x7 x6 x5 x4 x3 x2 x1] and partial b has the form:
 [0  y1 y2 y3 y4 y5 y6 y7].
 This function computes (x1^2+y1^2)*C1 + (x2^2+y2^2)*C2 + ...
 (x7^2+y2^7)*C7 + (x8^2+0^2)*C8 where the C1..C8 constants are in const1
 and const2. */
static INLINE __m256i fold_mul_and_sum(__m256i partial, __m256i const_var) {
    partial = _mm256_shuffle_epi8(
        partial,
        _mm256_set_epi32(
            0x0f0e0100, 0x03020504, 0x07060908, 0x0b0a0d0c, 0x0f0e0d0c, 0x0b0a0908, 0x07060504, 0x03020100));
    partial = _mm256_permutevar8x32_epi32(partial, _mm256_set_epi32(7, 3, 6, 2, 5, 1, 4, 0));
    partial = _mm256_shuffle_epi8(
        partial,
        _mm256_set_epi32(
            0x0f0e0b0a, 0x0d0c0908, 0x07060302, 0x05040100, 0x0f0e0b0a, 0x0d0c0908, 0x07060302, 0x05040100));
    partial = _mm256_madd_epi16(partial, partial);
    partial = _mm256_mullo_epi32(partial, const_var);
    return partial;
}

// Mask used to shuffle the elements present in 256bit register.
const int svt_shuffle_reg_256bit[8] = {
    0x0b0a0d0c, 0x07060908, 0x03020504, 0x0f0e0100, 0x0b0a0d0c, 0x07060908, 0x03020504, 0x0f0e0100};

/* partial A is a 16-bit vector of the form:
[x8 - - x1 | x16 - - x9] and partial B has the form:
[0  y1 - y7 | 0 y9 - y15].
This function computes (x1^2+y1^2)*C1 + (x2^2+y2^2)*C2 + ...
(x7^2+y2^7)*C7 + (x8^2+0^2)*C8 on each 128-bit lane. Here the C1..C8 constants
are in const1 and const2. */
static INLINE __m256i fold_mul_and_sum_dual(__m256i* partiala, __m256i* partialb, const __m256i* const1,
                                            const __m256i* const2) {
    __m256i tmp;
    /* Reverse partial B. */
    *partialb = _mm256_shuffle_epi8(*partialb, _mm256_loadu_si256((const __m256i*)svt_shuffle_reg_256bit));

    /* Interleave the x and y values of identical indices and pair x8 with 0. */
    tmp       = *partiala;
    *partiala = _mm256_unpacklo_epi16(*partiala, *partialb);
    *partialb = _mm256_unpackhi_epi16(tmp, *partialb);

    /* Square and add the corresponding x and y values. */
    *partiala = _mm256_madd_epi16(*partiala, *partiala);
    *partialb = _mm256_madd_epi16(*partialb, *partialb);
    /* Multiply by constant. */
    *partiala = _mm256_mullo_epi32(*partiala, *const1);
    *partialb = _mm256_mullo_epi32(*partialb, *const2);
    /* Sum all results. */
    *partiala = _mm256_add_epi32(*partiala, *partialb);
    return *partiala;
}

static INLINE __m128i hsum4(__m128i x0, __m128i x1, __m128i x2, __m128i x3) {
    __m128i t0, t1, t2, t3;
    t0 = _mm_unpacklo_epi32(x0, x1);
    t1 = _mm_unpacklo_epi32(x2, x3);
    t2 = _mm_unpackhi_epi32(x0, x1);
    t3 = _mm_unpackhi_epi32(x2, x3);
    x0 = _mm_unpacklo_epi64(t0, t1);
    x1 = _mm_unpackhi_epi64(t0, t1);
    x2 = _mm_unpacklo_epi64(t2, t3);
    x3 = _mm_unpackhi_epi64(t2, t3);
    return _mm_add_epi32(_mm_add_epi32(x0, x1), _mm_add_epi32(x2, x3));
}

static INLINE __m256i hsum4_dual(__m256i* x0, __m256i* x1, __m256i* x2, __m256i* x3) {
    const __m256i t0 = _mm256_unpacklo_epi32(*x0, *x1);
    const __m256i t1 = _mm256_unpacklo_epi32(*x2, *x3);
    const __m256i t2 = _mm256_unpackhi_epi32(*x0, *x1);
    const __m256i t3 = _mm256_unpackhi_epi32(*x2, *x3);

    *x0 = _mm256_unpacklo_epi64(t0, t1);
    *x1 = _mm256_unpackhi_epi64(t0, t1);
    *x2 = _mm256_unpacklo_epi64(t2, t3);
    *x3 = _mm256_unpackhi_epi64(t2, t3);
    return _mm256_add_epi32(_mm256_add_epi32(*x0, *x1), _mm256_add_epi32(*x2, *x3));
}

/* Computes cost for directions 0, 5, 6 and 7. We can call this function again
to compute the remaining directions. */
static INLINE void compute_directions(__m128i lines[8], int32_t tmp_cost1[4]) {
    __m128i partial6;
    __m128i tmp;

    __m256i partial4;
    __m256i partial5;
    __m256i partial7;
    __m256i tmp_avx2;
    /* Partial sums for lines 0 and 1. */
    partial4 = _mm256_setr_m128i(_mm_slli_si128(lines[0], 14), _mm_srli_si128(lines[0], 2));
    tmp_avx2 = _mm256_setr_m128i(_mm_slli_si128(lines[1], 12), _mm_srli_si128(lines[1], 4));
    partial4 = _mm256_add_epi16(partial4, tmp_avx2);
    tmp      = _mm_add_epi16(lines[0], lines[1]);
    partial5 = _mm256_setr_m128i(_mm_slli_si128(tmp, 10), _mm_srli_si128(tmp, 6));
    partial7 = _mm256_setr_m128i(_mm_slli_si128(tmp, 4), _mm_srli_si128(tmp, 12));
    partial6 = tmp;

    /* Partial sums for lines 2 and 3. */
    tmp_avx2 = _mm256_setr_m128i(_mm_slli_si128(lines[2], 10), _mm_srli_si128(lines[2], 6));
    partial4 = _mm256_add_epi16(partial4, tmp_avx2);
    tmp_avx2 = _mm256_setr_m128i(_mm_slli_si128(lines[3], 8), _mm_srli_si128(lines[3], 8));
    partial4 = _mm256_add_epi16(partial4, tmp_avx2);
    tmp      = _mm_add_epi16(lines[2], lines[3]);
    tmp_avx2 = _mm256_setr_m128i(_mm_slli_si128(tmp, 8), _mm_srli_si128(tmp, 8));
    partial5 = _mm256_add_epi16(partial5, tmp_avx2);
    tmp_avx2 = _mm256_setr_m128i(_mm_slli_si128(tmp, 6), _mm_srli_si128(tmp, 10));
    partial7 = _mm256_add_epi16(partial7, tmp_avx2);
    partial6 = _mm_add_epi16(partial6, tmp);

    /* Partial sums for lines 4 and 5. */
    tmp_avx2 = _mm256_setr_m128i(_mm_slli_si128(lines[4], 6), _mm_srli_si128(lines[4], 10));
    partial4 = _mm256_add_epi16(partial4, tmp_avx2);
    tmp_avx2 = _mm256_setr_m128i(_mm_slli_si128(lines[5], 4), _mm_srli_si128(lines[5], 12));
    partial4 = _mm256_add_epi16(partial4, tmp_avx2);
    tmp      = _mm_add_epi16(lines[4], lines[5]);
    tmp_avx2 = _mm256_setr_m128i(_mm_slli_si128(tmp, 6), _mm_srli_si128(tmp, 10));
    partial5 = _mm256_add_epi16(partial5, tmp_avx2);
    tmp_avx2 = _mm256_setr_m128i(_mm_slli_si128(tmp, 8), _mm_srli_si128(tmp, 8));
    partial7 = _mm256_add_epi16(partial7, tmp_avx2);
    partial6 = _mm_add_epi16(partial6, tmp);

    /* Partial sums for lines 6 and 7. */
    tmp_avx2 = _mm256_setr_m128i(_mm_slli_si128(lines[6], 2), _mm_srli_si128(lines[6], 14));
    partial4 = _mm256_add_epi16(partial4, tmp_avx2);
    tmp_avx2 = _mm256_insertf128_si256(_mm256_setzero_si256(), lines[7], 0x0);
    partial4 = _mm256_add_epi16(partial4, tmp_avx2);
    tmp      = _mm_add_epi16(lines[6], lines[7]);
    tmp_avx2 = _mm256_setr_m128i(_mm_slli_si128(tmp, 4), _mm_srli_si128(tmp, 12));
    partial5 = _mm256_add_epi16(partial5, tmp_avx2);
    tmp_avx2 = _mm256_setr_m128i(_mm_slli_si128(tmp, 10), _mm_srli_si128(tmp, 6));
    partial7 = _mm256_add_epi16(partial7, tmp_avx2);
    partial6 = _mm_add_epi16(partial6, tmp);

    /* Compute costs in terms of partial sums. */
    partial4 = fold_mul_and_sum(partial4, _mm256_set_epi32(105, 120, 140, 168, 210, 280, 420, 840));
    partial7 = fold_mul_and_sum(partial7, _mm256_set_epi32(105, 105, 105, 140, 210, 420, 0, 0));
    partial5 = fold_mul_and_sum(partial5, _mm256_set_epi32(105, 105, 105, 140, 210, 420, 0, 0));
    partial6 = _mm_madd_epi16(partial6, partial6);
    partial6 = _mm_mullo_epi32(partial6, _mm_set1_epi32(105));
    __m128i a, b, c;
    a = _mm_add_epi32(_mm256_castsi256_si128(partial4), _mm256_extracti128_si256(partial4, 1));
    b = _mm_add_epi32(_mm256_castsi256_si128(partial5), _mm256_extracti128_si256(partial5, 1));
    c = _mm_add_epi32(_mm256_castsi256_si128(partial7), _mm256_extracti128_si256(partial7, 1));

    _mm_storeu_si128((__m128i*)tmp_cost1, hsum4(a, b, partial6, c));
}

/* transpose and reverse the order of the lines -- equivalent to a 90-degree
counter-clockwise rotation of the pixels. */
static INLINE void array_reverse_transpose_8x8(__m128i* in, __m128i* res) {
    const __m128i tr0_0 = _mm_unpacklo_epi16(in[0], in[1]);
    const __m128i tr0_1 = _mm_unpacklo_epi16(in[2], in[3]);
    const __m128i tr0_2 = _mm_unpackhi_epi16(in[0], in[1]);
    const __m128i tr0_3 = _mm_unpackhi_epi16(in[2], in[3]);
    const __m128i tr0_4 = _mm_unpacklo_epi16(in[4], in[5]);
    const __m128i tr0_5 = _mm_unpacklo_epi16(in[6], in[7]);
    const __m128i tr0_6 = _mm_unpackhi_epi16(in[4], in[5]);
    const __m128i tr0_7 = _mm_unpackhi_epi16(in[6], in[7]);

    const __m128i tr1_0 = _mm_unpacklo_epi32(tr0_0, tr0_1);
    const __m128i tr1_1 = _mm_unpacklo_epi32(tr0_4, tr0_5);
    const __m128i tr1_2 = _mm_unpackhi_epi32(tr0_0, tr0_1);
    const __m128i tr1_3 = _mm_unpackhi_epi32(tr0_4, tr0_5);
    const __m128i tr1_4 = _mm_unpacklo_epi32(tr0_2, tr0_3);
    const __m128i tr1_5 = _mm_unpacklo_epi32(tr0_6, tr0_7);
    const __m128i tr1_6 = _mm_unpackhi_epi32(tr0_2, tr0_3);
    const __m128i tr1_7 = _mm_unpackhi_epi32(tr0_6, tr0_7);

    res[7] = _mm_unpacklo_epi64(tr1_0, tr1_1);
    res[6] = _mm_unpackhi_epi64(tr1_0, tr1_1);
    res[5] = _mm_unpacklo_epi64(tr1_2, tr1_3);
    res[4] = _mm_unpackhi_epi64(tr1_2, tr1_3);
    res[3] = _mm_unpacklo_epi64(tr1_4, tr1_5);
    res[2] = _mm_unpackhi_epi64(tr1_4, tr1_5);
    res[1] = _mm_unpacklo_epi64(tr1_6, tr1_7);
    res[0] = _mm_unpackhi_epi64(tr1_6, tr1_7);
}

uint8_t svt_aom_cdef_find_dir_avx2(const uint16_t* img, int32_t stride, int32_t* var, int32_t coeff_shift) {
    int32_t cost[8];
    int32_t best_cost = 0;
    uint8_t i;
    uint8_t best_dir = 0;
    __m128i lines[8];
    __m128i const_128 = _mm_set1_epi16(128);
    for (i = 0; i < 8; i++) {
        lines[i] = _mm_lddqu_si128((__m128i*)&img[i * stride]);
        lines[i] = _mm_sub_epi16(_mm_sra_epi16(lines[i], _mm_cvtsi32_si128(coeff_shift)), const_128);
    }

    /* Compute "mostly vertical" directions. */
    compute_directions(lines, cost + 4);

    array_reverse_transpose_8x8(lines, lines);

    /* Compute "mostly horizontal" directions. */
    compute_directions(lines, cost);

    for (i = 0; i < 8; i++) {
        if (cost[i] > best_cost) {
            best_cost = cost[i];
            best_dir  = i;
        }
    }

    /* Difference between the optimal variance and the variance along the
    orthogonal direction. Again, the sum(x^2) terms cancel out. */
    *var = best_cost - cost[(best_dir + 4) & 7];
    /* We'd normally divide by 840, but dividing by 1024 is close enough
    for what we're going to do with this. */
    *var >>= 10;
    return best_dir;
}

/* Computes cost for directions 0, 5, 6 and 7. We can call this function again
to compute the remaining directions. */
static INLINE __m256i compute_directions_dual(__m256i* lines, int32_t cost_frist_8x8[4], int32_t cost_second_8x8[4]) {
    __m256i partial4a, partial4b, partial5a, partial5b, partial7a, partial7b;
    __m256i partial6;
    __m256i tmp;
    /* Partial sums for lines 0 and 1. */
    partial4a = _mm256_slli_si256(lines[0], 14);
    partial4b = _mm256_srli_si256(lines[0], 2);
    partial4a = _mm256_add_epi16(partial4a, _mm256_slli_si256(lines[1], 12));
    partial4b = _mm256_add_epi16(partial4b, _mm256_srli_si256(lines[1], 4));
    tmp       = _mm256_add_epi16(lines[0], lines[1]);
    partial5a = _mm256_slli_si256(tmp, 10);
    partial5b = _mm256_srli_si256(tmp, 6);
    partial7a = _mm256_slli_si256(tmp, 4);
    partial7b = _mm256_srli_si256(tmp, 12);
    partial6  = tmp;

    /* Partial sums for lines 2 and 3. */
    partial4a = _mm256_add_epi16(partial4a, _mm256_slli_si256(lines[2], 10));
    partial4b = _mm256_add_epi16(partial4b, _mm256_srli_si256(lines[2], 6));
    partial4a = _mm256_add_epi16(partial4a, _mm256_slli_si256(lines[3], 8));
    partial4b = _mm256_add_epi16(partial4b, _mm256_srli_si256(lines[3], 8));
    tmp       = _mm256_add_epi16(lines[2], lines[3]);
    partial5a = _mm256_add_epi16(partial5a, _mm256_slli_si256(tmp, 8));
    partial5b = _mm256_add_epi16(partial5b, _mm256_srli_si256(tmp, 8));
    partial7a = _mm256_add_epi16(partial7a, _mm256_slli_si256(tmp, 6));
    partial7b = _mm256_add_epi16(partial7b, _mm256_srli_si256(tmp, 10));
    partial6  = _mm256_add_epi16(partial6, tmp);

    /* Partial sums for lines 4 and 5. */
    partial4a = _mm256_add_epi16(partial4a, _mm256_slli_si256(lines[4], 6));
    partial4b = _mm256_add_epi16(partial4b, _mm256_srli_si256(lines[4], 10));
    partial4a = _mm256_add_epi16(partial4a, _mm256_slli_si256(lines[5], 4));
    partial4b = _mm256_add_epi16(partial4b, _mm256_srli_si256(lines[5], 12));
    tmp       = _mm256_add_epi16(lines[4], lines[5]);
    partial5a = _mm256_add_epi16(partial5a, _mm256_slli_si256(tmp, 6));
    partial5b = _mm256_add_epi16(partial5b, _mm256_srli_si256(tmp, 10));
    partial7a = _mm256_add_epi16(partial7a, _mm256_slli_si256(tmp, 8));
    partial7b = _mm256_add_epi16(partial7b, _mm256_srli_si256(tmp, 8));
    partial6  = _mm256_add_epi16(partial6, tmp);

    /* Partial sums for lines 6 and 7. */
    partial4a = _mm256_add_epi16(partial4a, _mm256_slli_si256(lines[6], 2));
    partial4b = _mm256_add_epi16(partial4b, _mm256_srli_si256(lines[6], 14));
    partial4a = _mm256_add_epi16(partial4a, lines[7]);
    tmp       = _mm256_add_epi16(lines[6], lines[7]);
    partial5a = _mm256_add_epi16(partial5a, _mm256_slli_si256(tmp, 4));
    partial5b = _mm256_add_epi16(partial5b, _mm256_srli_si256(tmp, 12));
    partial7a = _mm256_add_epi16(partial7a, _mm256_slli_si256(tmp, 10));
    partial7b = _mm256_add_epi16(partial7b, _mm256_srli_si256(tmp, 6));
    partial6  = _mm256_add_epi16(partial6, tmp);

    const __m256i const_reg_1 = _mm256_set_epi32(210, 280, 420, 840, 210, 280, 420, 840);
    const __m256i const_reg_2 = _mm256_set_epi32(105, 120, 140, 168, 105, 120, 140, 168);
    const __m256i const_reg_3 = _mm256_set_epi32(210, 420, 0, 0, 210, 420, 0, 0);
    const __m256i const_reg_4 = _mm256_set_epi32(105, 105, 105, 140, 105, 105, 105, 140);

    /* Compute costs in terms of partial sums. */
    partial4a = fold_mul_and_sum_dual(&partial4a, &partial4b, &const_reg_1, &const_reg_2);
    partial7a = fold_mul_and_sum_dual(&partial7a, &partial7b, &const_reg_3, &const_reg_4);
    partial5a = fold_mul_and_sum_dual(&partial5a, &partial5b, &const_reg_3, &const_reg_4);
    partial6  = _mm256_madd_epi16(partial6, partial6);
    partial6  = _mm256_mullo_epi32(partial6, _mm256_set1_epi32(105));

    partial4a = hsum4_dual(&partial4a, &partial5a, &partial6, &partial7a);
    _mm_storeu_si128((__m128i*)cost_frist_8x8, _mm256_castsi256_si128(partial4a));
    _mm_storeu_si128((__m128i*)cost_second_8x8, _mm256_extractf128_si256(partial4a, 1));

    return partial4a;
}

/* transpose and reverse the order of the lines -- equivalent to a 90-degree
counter-clockwise rotation of the pixels. */
static INLINE void array_reverse_transpose_8x8_dual(__m256i* in, __m256i* res) {
    const __m256i tr0_0 = _mm256_unpacklo_epi16(in[0], in[1]);
    const __m256i tr0_1 = _mm256_unpacklo_epi16(in[2], in[3]);
    const __m256i tr0_2 = _mm256_unpackhi_epi16(in[0], in[1]);
    const __m256i tr0_3 = _mm256_unpackhi_epi16(in[2], in[3]);
    const __m256i tr0_4 = _mm256_unpacklo_epi16(in[4], in[5]);
    const __m256i tr0_5 = _mm256_unpacklo_epi16(in[6], in[7]);
    const __m256i tr0_6 = _mm256_unpackhi_epi16(in[4], in[5]);
    const __m256i tr0_7 = _mm256_unpackhi_epi16(in[6], in[7]);

    const __m256i tr1_0 = _mm256_unpacklo_epi32(tr0_0, tr0_1);
    const __m256i tr1_1 = _mm256_unpacklo_epi32(tr0_4, tr0_5);
    const __m256i tr1_2 = _mm256_unpackhi_epi32(tr0_0, tr0_1);
    const __m256i tr1_3 = _mm256_unpackhi_epi32(tr0_4, tr0_5);
    const __m256i tr1_4 = _mm256_unpacklo_epi32(tr0_2, tr0_3);
    const __m256i tr1_5 = _mm256_unpacklo_epi32(tr0_6, tr0_7);
    const __m256i tr1_6 = _mm256_unpackhi_epi32(tr0_2, tr0_3);
    const __m256i tr1_7 = _mm256_unpackhi_epi32(tr0_6, tr0_7);

    res[7] = _mm256_unpacklo_epi64(tr1_0, tr1_1);
    res[6] = _mm256_unpackhi_epi64(tr1_0, tr1_1);
    res[5] = _mm256_unpacklo_epi64(tr1_2, tr1_3);
    res[4] = _mm256_unpackhi_epi64(tr1_2, tr1_3);
    res[3] = _mm256_unpacklo_epi64(tr1_4, tr1_5);
    res[2] = _mm256_unpackhi_epi64(tr1_4, tr1_5);
    res[1] = _mm256_unpacklo_epi64(tr1_6, tr1_7);
    res[0] = _mm256_unpackhi_epi64(tr1_6, tr1_7);
}

void svt_aom_cdef_find_dir_dual_avx2(const uint16_t* img1, const uint16_t* img2, int stride, int32_t* var_out_1st,
                                     int32_t* var_out_2nd, int32_t coeff_shift, uint8_t* out_dir_1st_8x8,
                                     uint8_t* out_dir_2nd_8x8) {
    int32_t cost_first_8x8[8];
    int32_t cost_second_8x8[8];
    // Used to store the best cost for 2 8x8's.
    int32_t best_cost[2] = {0};
    // Best direction for 2 8x8's.
    uint8_t best_dir[2] = {0};

    const __m128i const_coeff_shift_reg = _mm_cvtsi32_si128(coeff_shift);
    const __m256i const_128_reg         = _mm256_set1_epi16(128);
    __m256i       lines[8];
    for (int i = 0; i < 8; i++) {
        const __m128i src_1 = _mm_loadu_si128((const __m128i*)&img1[i * stride]);
        const __m128i src_2 = _mm_loadu_si128((const __m128i*)&img2[i * stride]);

        lines[i] = _mm256_insertf128_si256(_mm256_castsi128_si256(src_1), src_2, 1);
        lines[i] = _mm256_sub_epi16(_mm256_sra_epi16(lines[i], const_coeff_shift_reg), const_128_reg);
    }

    /* Compute "mostly vertical" directions. */
    const __m256i dir47 = compute_directions_dual(lines, cost_first_8x8 + 4, cost_second_8x8 + 4);

    /* Transpose and reverse the order of the lines. */
    array_reverse_transpose_8x8_dual(lines, lines);

    /* Compute "mostly horizontal" directions. */
    const __m256i dir03 = compute_directions_dual(lines, cost_first_8x8, cost_second_8x8);

    __m256i max = _mm256_max_epi32(dir03, dir47);
    max         = _mm256_max_epi32(max, _mm256_or_si256(_mm256_srli_si256(max, 8), _mm256_slli_si256(max, 16 - (8))));
    max         = _mm256_max_epi32(max, _mm256_or_si256(_mm256_srli_si256(max, 4), _mm256_slli_si256(max, 16 - (4))));

    const __m128i first_8x8_output  = _mm256_castsi256_si128(max);
    const __m128i second_8x8_output = _mm256_extractf128_si256(max, 1);
    const __m128i cmpeg_res_00      = _mm_cmpeq_epi32(first_8x8_output, _mm256_castsi256_si128(dir47));
    const __m128i cmpeg_res_01      = _mm_cmpeq_epi32(first_8x8_output, _mm256_castsi256_si128(dir03));
    const __m128i cmpeg_res_10      = _mm_cmpeq_epi32(second_8x8_output, _mm256_extractf128_si256(dir47, 1));
    const __m128i cmpeg_res_11      = _mm_cmpeq_epi32(second_8x8_output, _mm256_extractf128_si256(dir03, 1));
    const __m128i t_first_8x8       = _mm_packs_epi32(cmpeg_res_01, cmpeg_res_00);
    const __m128i t_second_8x8      = _mm_packs_epi32(cmpeg_res_11, cmpeg_res_10);

    best_cost[0] = _mm_cvtsi128_si32(_mm256_castsi256_si128(max));
    best_cost[1] = _mm_cvtsi128_si32(second_8x8_output);
    best_dir[0]  = _mm_movemask_epi8(_mm_packs_epi16(t_first_8x8, t_first_8x8));
    best_dir[0]  = get_msb(best_dir[0] ^ (best_dir[0] - 1)); // Count trailing zeros
    best_dir[1]  = _mm_movemask_epi8(_mm_packs_epi16(t_second_8x8, t_second_8x8));
    best_dir[1]  = get_msb(best_dir[1] ^ (best_dir[1] - 1)); // Count trailing zeros

    /* Difference between the optimal variance and the variance along the
       orthogonal direction. Again, the sum(x^2) terms cancel out. */
    *var_out_1st = best_cost[0] - cost_first_8x8[(best_dir[0] + 4) & 7];
    *var_out_2nd = best_cost[1] - cost_second_8x8[(best_dir[1] + 4) & 7];

    /* We'd normally divide by 840, but dividing by 1024 is close enough
    for what we're going to do with this. */
    *var_out_1st >>= 10;
    *var_out_2nd >>= 10;
    *out_dir_1st_8x8 = best_dir[0];
    *out_dir_2nd_8x8 = best_dir[1];
}

// sign(a-b) * min(abs(a-b), max(0, threshold - (abs(a-b) >> adjdamp)))
static INLINE __m256i constrain16(const __m256i in0, const __m256i in1, const __m256i threshold,
                                  const uint32_t adjdamp) {
    const __m256i diff = _mm256_sub_epi16(in0, in1);
    const __m256i sign = _mm256_srai_epi16(diff, 15);
    const __m256i a    = _mm256_abs_epi16(diff);
    const __m256i l    = _mm256_srl_epi16(a, _mm_cvtsi32_si128(adjdamp));
    const __m256i s    = _mm256_subs_epu16(threshold, l);
    const __m256i m    = _mm256_min_epi16(a, s);
    const __m256i d    = _mm256_add_epi16(sign, m);
    return _mm256_xor_si256(d, sign);
}

//16 bit
static INLINE void cdef_filter_block_8xn_16_pri_avx2(const uint16_t* const in, const int32_t pri_damping,
                                                     const int32_t po, const __m256i row,
                                                     const __m256i pri_strength_256, const __m256i pri_taps,
                                                     __m256i* const max, __m256i* const min, __m256i* const sum,
                                                     uint8_t subsampling_factor) {
    const __m256i large = _mm256_set1_epi16(CDEF_VERY_LARGE);
    const __m256i p0    = loadu_u16_8x2_avx2(in + po, subsampling_factor * CDEF_BSTRIDE);
    const __m256i p1    = loadu_u16_8x2_avx2(in - po, subsampling_factor * CDEF_BSTRIDE);

    *max = _mm256_max_epi16(_mm256_max_epi16(*max, _mm256_andnot_si256(_mm256_cmpeq_epi16(p0, large), p0)),
                            _mm256_andnot_si256(_mm256_cmpeq_epi16(p1, large), p1));
    *min = _mm256_min_epi16(_mm256_min_epi16(*min, p0), p1);

    const __m256i q0 = constrain16(p0, row, pri_strength_256, pri_damping);
    const __m256i q1 = constrain16(p1, row, pri_strength_256, pri_damping);

    // sum += pri_taps * (p0 + p1)
    *sum = _mm256_add_epi16(*sum, _mm256_mullo_epi16(pri_taps, _mm256_add_epi16(q0, q1)));
}

static INLINE void cdef_filter_block_8xn_16_sec_avx2(const uint16_t* const in, const int32_t sec_damping,
                                                     const int32_t so1, const int32_t so2, const __m256i row,
                                                     const __m256i sec_strength_256, const __m256i sec_taps,
                                                     __m256i* const max, __m256i* const min, __m256i* const sum,
                                                     uint8_t subsampling_factor) {
    const __m256i large = _mm256_set1_epi16(CDEF_VERY_LARGE);
    const __m256i p0    = loadu_u16_8x2_avx2(in + so1, subsampling_factor * CDEF_BSTRIDE);
    const __m256i p1    = loadu_u16_8x2_avx2(in - so1, subsampling_factor * CDEF_BSTRIDE);
    const __m256i p2    = loadu_u16_8x2_avx2(in + so2, subsampling_factor * CDEF_BSTRIDE);
    const __m256i p3    = loadu_u16_8x2_avx2(in - so2, subsampling_factor * CDEF_BSTRIDE);

    *max = _mm256_max_epi16(_mm256_max_epi16(*max, _mm256_andnot_si256(_mm256_cmpeq_epi16(p0, large), p0)),
                            _mm256_andnot_si256(_mm256_cmpeq_epi16(p1, large), p1));
    *max = _mm256_max_epi16(_mm256_max_epi16(*max, _mm256_andnot_si256(_mm256_cmpeq_epi16(p2, large), p2)),
                            _mm256_andnot_si256(_mm256_cmpeq_epi16(p3, large), p3));
    *min = _mm256_min_epi16(_mm256_min_epi16(_mm256_min_epi16(_mm256_min_epi16(*min, p0), p1), p2), p3);

    const __m256i q0 = constrain16(p0, row, sec_strength_256, sec_damping);
    const __m256i q1 = constrain16(p1, row, sec_strength_256, sec_damping);
    const __m256i q2 = constrain16(p2, row, sec_strength_256, sec_damping);
    const __m256i q3 = constrain16(p3, row, sec_strength_256, sec_damping);

    // sum += sec_taps * (p0 + p1 + p2 + p3)
    *sum = _mm256_add_epi16(
        *sum, _mm256_mullo_epi16(sec_taps, _mm256_add_epi16(_mm256_add_epi16(q0, q1), _mm256_add_epi16(q2, q3))));
}

// subsampling_factor of 1 means no subsampling
// requires height/subsampling_factor >= 2
void svt_cdef_filter_block_8xn_16_avx2(const uint16_t* const in, const int32_t pri_strength, const int32_t sec_strength,
                                       const int32_t dir, int32_t pri_damping, int32_t sec_damping,
                                       const int32_t coeff_shift, uint16_t* const dst, const int32_t dstride,
                                       uint8_t height, uint8_t subsampling_factor) {
    const int32_t po1  = svt_aom_eb_cdef_directions[dir][0];
    const int32_t po2  = svt_aom_eb_cdef_directions[dir][1];
    const int32_t s1o1 = svt_aom_eb_cdef_directions[(dir + 2)][0];
    const int32_t s1o2 = svt_aom_eb_cdef_directions[(dir + 2)][1];
    const int32_t s2o1 = svt_aom_eb_cdef_directions[(dir - 2)][0];
    const int32_t s2o2 = svt_aom_eb_cdef_directions[(dir - 2)][1];
    // SSE CHKN
    const int32_t* pri_taps = svt_aom_eb_cdef_pri_taps[(pri_strength >> coeff_shift) & 1];
    const int32_t* sec_taps = svt_aom_eb_cdef_sec_taps[(pri_strength >> coeff_shift) & 1];
    int32_t        i;
    const __m256i  pri_taps_0       = _mm256_set1_epi16(pri_taps[0]);
    const __m256i  pri_taps_1       = _mm256_set1_epi16(pri_taps[1]);
    const __m256i  sec_taps_0       = _mm256_set1_epi16(sec_taps[0]);
    const __m256i  sec_taps_1       = _mm256_set1_epi16(sec_taps[1]);
    const __m256i  duplicate_8      = _mm256_set1_epi16(8);
    const __m256i  pri_strength_256 = _mm256_set1_epi16(pri_strength);
    const __m256i  sec_strength_256 = _mm256_set1_epi16(sec_strength);

    if (pri_strength) {
        pri_damping = AOMMAX(0, pri_damping - get_msb(pri_strength));
    }
    if (sec_strength) {
        sec_damping = AOMMAX(0, sec_damping - get_msb(sec_strength));
    }

    for (i = 0; i < height; i += (2 * subsampling_factor)) {
        const __m256i row = loadu_u16_8x2_avx2(in + i * CDEF_BSTRIDE, subsampling_factor * CDEF_BSTRIDE);
        __m256i       sum, res, max, min;

        min = max = row;
        sum       = _mm256_setzero_si256();

        // Primary near taps
        cdef_filter_block_8xn_16_pri_avx2(in + i * CDEF_BSTRIDE,
                                          pri_damping,
                                          po1,
                                          row,
                                          pri_strength_256,
                                          pri_taps_0,
                                          &max,
                                          &min,
                                          &sum,
                                          subsampling_factor);

        // Primary far taps
        cdef_filter_block_8xn_16_pri_avx2(in + i * CDEF_BSTRIDE,
                                          pri_damping,
                                          po2,
                                          row,
                                          pri_strength_256,
                                          pri_taps_1,
                                          &max,
                                          &min,
                                          &sum,
                                          subsampling_factor);

        // Secondary near taps
        cdef_filter_block_8xn_16_sec_avx2(in + i * CDEF_BSTRIDE,
                                          sec_damping,
                                          s1o1,
                                          s2o1,
                                          row,
                                          sec_strength_256,
                                          sec_taps_0,
                                          &max,
                                          &min,
                                          &sum,
                                          subsampling_factor);

        // Secondary far taps
        cdef_filter_block_8xn_16_sec_avx2(in + i * CDEF_BSTRIDE,
                                          sec_damping,
                                          s1o2,
                                          s2o2,
                                          row,
                                          sec_strength_256,
                                          sec_taps_1,
                                          &max,
                                          &min,
                                          &sum,
                                          subsampling_factor);

        // res = row + ((sum - (sum < 0) + 8) >> 4)
        sum = _mm256_add_epi16(sum, _mm256_cmpgt_epi16(_mm256_setzero_si256(), sum));
        res = _mm256_add_epi16(sum, duplicate_8);
        res = _mm256_srai_epi16(res, 4);
        res = _mm256_add_epi16(row, res);
        res = _mm256_min_epi16(_mm256_max_epi16(res, min), max);
        _mm_storeu_si128((__m128i*)&dst[i * dstride], _mm256_castsi256_si128(res));
        _mm_storeu_si128((__m128i*)&dst[(i + subsampling_factor) * dstride], _mm256_extracti128_si256(res, 1));
    }
}

// subsampling_factor of 1 means no subsampling
// requires height/subsampling_factor >= 4
static void svt_cdef_filter_block_4xn_16_avx2(uint16_t* dst, int32_t dstride, const uint16_t* in, int32_t pri_strength,
                                              int32_t sec_strength, int32_t dir, int32_t pri_damping,
                                              int32_t sec_damping, int32_t coeff_shift, uint8_t height,
                                              uint8_t subsampling_factor) {
    __m256i        p0, p1, p2, p3, sum, row, res;
    __m256i        max, min, large = _mm256_set1_epi16(CDEF_VERY_LARGE);
    const int32_t  po1              = svt_aom_eb_cdef_directions[dir][0];
    const int32_t  po2              = svt_aom_eb_cdef_directions[dir][1];
    const int32_t  s1o1             = svt_aom_eb_cdef_directions[(dir + 2)][0];
    const int32_t  s1o2             = svt_aom_eb_cdef_directions[(dir + 2)][1];
    const int32_t  s2o1             = svt_aom_eb_cdef_directions[(dir - 2)][0];
    const int32_t  s2o2             = svt_aom_eb_cdef_directions[(dir - 2)][1];
    const int32_t* pri_taps         = svt_aom_eb_cdef_pri_taps[(pri_strength >> coeff_shift) & 1];
    const int32_t* sec_taps         = svt_aom_eb_cdef_sec_taps[(pri_strength >> coeff_shift) & 1];
    __m256i        pri_strength_256 = _mm256_set1_epi16(pri_strength);
    __m256i        sec_strength_256 = _mm256_set1_epi16(sec_strength);

    if (pri_strength) {
        pri_damping = AOMMAX(0, pri_damping - get_msb(pri_strength));
    }
    if (sec_strength) {
        sec_damping = AOMMAX(0, sec_damping - get_msb(sec_strength));
    }
    for (uint32_t i = 0; i < height; i += (4 * subsampling_factor)) {
        sum = _mm256_setzero_si256();
        row = _mm256_set_epi64x(*(uint64_t*)(in + i * CDEF_BSTRIDE),
                                *(uint64_t*)(in + (i + (1 * subsampling_factor)) * CDEF_BSTRIDE),
                                *(uint64_t*)(in + (i + (2 * subsampling_factor)) * CDEF_BSTRIDE),
                                *(uint64_t*)(in + (i + (3 * subsampling_factor)) * CDEF_BSTRIDE));
        min = max = row;

        // Primary near taps
        p0 = _mm256_set_epi64x(*(uint64_t*)(in + i * CDEF_BSTRIDE + po1),
                               *(uint64_t*)(in + (i + (1 * subsampling_factor)) * CDEF_BSTRIDE + po1),
                               *(uint64_t*)(in + (i + (2 * subsampling_factor)) * CDEF_BSTRIDE + po1),
                               *(uint64_t*)(in + (i + (3 * subsampling_factor)) * CDEF_BSTRIDE + po1));
        p1 = _mm256_set_epi64x(*(uint64_t*)(in + i * CDEF_BSTRIDE - po1),
                               *(uint64_t*)(in + (i + (1 * subsampling_factor)) * CDEF_BSTRIDE - po1),
                               *(uint64_t*)(in + (i + (2 * subsampling_factor)) * CDEF_BSTRIDE - po1),
                               *(uint64_t*)(in + (i + (3 * subsampling_factor)) * CDEF_BSTRIDE - po1));

        max = _mm256_max_epi16(_mm256_max_epi16(max, _mm256_andnot_si256(_mm256_cmpeq_epi16(p0, large), p0)),
                               _mm256_andnot_si256(_mm256_cmpeq_epi16(p1, large), p1));
        min = _mm256_min_epi16(_mm256_min_epi16(min, p0), p1);
        p0  = constrain16(p0, row, pri_strength_256, pri_damping);
        p1  = constrain16(p1, row, pri_strength_256, pri_damping);

        // sum += pri_taps[0] * (p0 + p1)
        sum = _mm256_add_epi16(sum, _mm256_mullo_epi16(_mm256_set1_epi16(pri_taps[0]), _mm256_add_epi16(p0, p1)));

        // Primary far taps
        p0  = _mm256_set_epi64x(*(uint64_t*)(in + i * CDEF_BSTRIDE + po2),
                               *(uint64_t*)(in + (i + (1 * subsampling_factor)) * CDEF_BSTRIDE + po2),
                               *(uint64_t*)(in + (i + (2 * subsampling_factor)) * CDEF_BSTRIDE + po2),
                               *(uint64_t*)(in + (i + (3 * subsampling_factor)) * CDEF_BSTRIDE + po2));
        p1  = _mm256_set_epi64x(*(uint64_t*)(in + i * CDEF_BSTRIDE - po2),
                               *(uint64_t*)(in + (i + (1 * subsampling_factor)) * CDEF_BSTRIDE - po2),
                               *(uint64_t*)(in + (i + (2 * subsampling_factor)) * CDEF_BSTRIDE - po2),
                               *(uint64_t*)(in + (i + (3 * subsampling_factor)) * CDEF_BSTRIDE - po2));
        max = _mm256_max_epi16(_mm256_max_epi16(max, _mm256_andnot_si256(_mm256_cmpeq_epi16(p0, large), p0)),
                               _mm256_andnot_si256(_mm256_cmpeq_epi16(p1, large), p1));
        min = _mm256_min_epi16(_mm256_min_epi16(min, p0), p1);
        p0  = constrain16(p0, row, pri_strength_256, pri_damping);
        p1  = constrain16(p1, row, pri_strength_256, pri_damping);

        // sum += pri_taps[1] * (p0 + p1)
        sum = _mm256_add_epi16(sum, _mm256_mullo_epi16(_mm256_set1_epi16(pri_taps[1]), _mm256_add_epi16(p0, p1)));

        // Secondary near taps
        p0  = _mm256_set_epi64x(*(uint64_t*)(in + i * CDEF_BSTRIDE + s1o1),
                               *(uint64_t*)(in + (i + (1 * subsampling_factor)) * CDEF_BSTRIDE + s1o1),
                               *(uint64_t*)(in + (i + (2 * subsampling_factor)) * CDEF_BSTRIDE + s1o1),
                               *(uint64_t*)(in + (i + (3 * subsampling_factor)) * CDEF_BSTRIDE + s1o1));
        p1  = _mm256_set_epi64x(*(uint64_t*)(in + i * CDEF_BSTRIDE - s1o1),
                               *(uint64_t*)(in + (i + (1 * subsampling_factor)) * CDEF_BSTRIDE - s1o1),
                               *(uint64_t*)(in + (i + (2 * subsampling_factor)) * CDEF_BSTRIDE - s1o1),
                               *(uint64_t*)(in + (i + (3 * subsampling_factor)) * CDEF_BSTRIDE - s1o1));
        p2  = _mm256_set_epi64x(*(uint64_t*)(in + i * CDEF_BSTRIDE + s2o1),
                               *(uint64_t*)(in + (i + (1 * subsampling_factor)) * CDEF_BSTRIDE + s2o1),
                               *(uint64_t*)(in + (i + (2 * subsampling_factor)) * CDEF_BSTRIDE + s2o1),
                               *(uint64_t*)(in + (i + (3 * subsampling_factor)) * CDEF_BSTRIDE + s2o1));
        p3  = _mm256_set_epi64x(*(uint64_t*)(in + i * CDEF_BSTRIDE - s2o1),
                               *(uint64_t*)(in + (i + (1 * subsampling_factor)) * CDEF_BSTRIDE - s2o1),
                               *(uint64_t*)(in + (i + (2 * subsampling_factor)) * CDEF_BSTRIDE - s2o1),
                               *(uint64_t*)(in + (i + (3 * subsampling_factor)) * CDEF_BSTRIDE - s2o1));
        max = _mm256_max_epi16(_mm256_max_epi16(max, _mm256_andnot_si256(_mm256_cmpeq_epi16(p0, large), p0)),
                               _mm256_andnot_si256(_mm256_cmpeq_epi16(p1, large), p1));
        max = _mm256_max_epi16(_mm256_max_epi16(max, _mm256_andnot_si256(_mm256_cmpeq_epi16(p2, large), p2)),
                               _mm256_andnot_si256(_mm256_cmpeq_epi16(p3, large), p3));
        min = _mm256_min_epi16(_mm256_min_epi16(_mm256_min_epi16(_mm256_min_epi16(min, p0), p1), p2), p3);
        p0  = constrain16(p0, row, sec_strength_256, sec_damping);
        p1  = constrain16(p1, row, sec_strength_256, sec_damping);
        p2  = constrain16(p2, row, sec_strength_256, sec_damping);
        p3  = constrain16(p3, row, sec_strength_256, sec_damping);

        // sum += sec_taps[0] * (p0 + p1 + p2 + p3)
        sum = _mm256_add_epi16(
            sum,
            _mm256_mullo_epi16(_mm256_set1_epi16(sec_taps[0]),
                               _mm256_add_epi16(_mm256_add_epi16(p0, p1), _mm256_add_epi16(p2, p3))));

        // Secondary far taps
        p0  = _mm256_set_epi64x(*(uint64_t*)(in + i * CDEF_BSTRIDE + s1o2),
                               *(uint64_t*)(in + (i + (1 * subsampling_factor)) * CDEF_BSTRIDE + s1o2),
                               *(uint64_t*)(in + (i + (2 * subsampling_factor)) * CDEF_BSTRIDE + s1o2),
                               *(uint64_t*)(in + (i + (3 * subsampling_factor)) * CDEF_BSTRIDE + s1o2));
        p1  = _mm256_set_epi64x(*(uint64_t*)(in + i * CDEF_BSTRIDE - s1o2),
                               *(uint64_t*)(in + (i + (1 * subsampling_factor)) * CDEF_BSTRIDE - s1o2),
                               *(uint64_t*)(in + (i + (2 * subsampling_factor)) * CDEF_BSTRIDE - s1o2),
                               *(uint64_t*)(in + (i + (3 * subsampling_factor)) * CDEF_BSTRIDE - s1o2));
        p2  = _mm256_set_epi64x(*(uint64_t*)(in + i * CDEF_BSTRIDE + s2o2),
                               *(uint64_t*)(in + (i + (1 * subsampling_factor)) * CDEF_BSTRIDE + s2o2),
                               *(uint64_t*)(in + (i + (2 * subsampling_factor)) * CDEF_BSTRIDE + s2o2),
                               *(uint64_t*)(in + (i + (3 * subsampling_factor)) * CDEF_BSTRIDE + s2o2));
        p3  = _mm256_set_epi64x(*(uint64_t*)(in + i * CDEF_BSTRIDE - s2o2),
                               *(uint64_t*)(in + (i + (1 * subsampling_factor)) * CDEF_BSTRIDE - s2o2),
                               *(uint64_t*)(in + (i + (2 * subsampling_factor)) * CDEF_BSTRIDE - s2o2),
                               *(uint64_t*)(in + (i + (3 * subsampling_factor)) * CDEF_BSTRIDE - s2o2));
        max = _mm256_max_epi16(_mm256_max_epi16(max, _mm256_andnot_si256(_mm256_cmpeq_epi16(p0, large), p0)),
                               _mm256_andnot_si256(_mm256_cmpeq_epi16(p1, large), p1));
        max = _mm256_max_epi16(_mm256_max_epi16(max, _mm256_andnot_si256(_mm256_cmpeq_epi16(p2, large), p2)),
                               _mm256_andnot_si256(_mm256_cmpeq_epi16(p3, large), p3));
        min = _mm256_min_epi16(_mm256_min_epi16(_mm256_min_epi16(_mm256_min_epi16(min, p0), p1), p2), p3);
        p0  = constrain16(p0, row, sec_strength_256, sec_damping);
        p1  = constrain16(p1, row, sec_strength_256, sec_damping);
        p2  = constrain16(p2, row, sec_strength_256, sec_damping);
        p3  = constrain16(p3, row, sec_strength_256, sec_damping);

        // sum += sec_taps[1] * (p0 + p1 + p2 + p3)
        sum = _mm256_add_epi16(
            sum,
            _mm256_mullo_epi16(_mm256_set1_epi16(sec_taps[1]),
                               _mm256_add_epi16(_mm256_add_epi16(p0, p1), _mm256_add_epi16(p2, p3))));

        // res = row + ((sum - (sum < 0) + 8) >> 4)
        sum = _mm256_add_epi16(sum, _mm256_cmpgt_epi16(_mm256_setzero_si256(), sum));
        res = _mm256_add_epi16(sum, _mm256_set1_epi16(8));
        res = _mm256_srai_epi16(res, 4);
        res = _mm256_add_epi16(row, res);
        res = _mm256_min_epi16(_mm256_max_epi16(res, min), max);

        *(uint64_t*)(dst + i * dstride)                              = _mm256_extract_epi64(res, 3);
        *(uint64_t*)(dst + (i + (1 * subsampling_factor)) * dstride) = _mm256_extract_epi64(res, 2);
        *(uint64_t*)(dst + (i + (2 * subsampling_factor)) * dstride) = _mm256_extract_epi64(res, 1);
        *(uint64_t*)(dst + (i + (3 * subsampling_factor)) * dstride) = _mm256_extract_epi64(res, 0);
    }
}

//8bit
// subsampling_factor of 1 means no subsampling
// requires height/subsampling_factor >= 4
static void svt_cdef_filter_block_4xn_8_avx2(uint8_t* dst, int32_t dstride, const uint16_t* in, int32_t pri_strength,
                                             int32_t sec_strength, int32_t dir, int32_t pri_damping,
                                             int32_t sec_damping, int32_t coeff_shift, uint8_t height,
                                             uint8_t subsampling_factor) {
    __m256i       p0, p1, p2, p3, sum, row, res;
    __m256i       max, min, large = _mm256_set1_epi16(CDEF_VERY_LARGE);
    const int32_t po1  = svt_aom_eb_cdef_directions[dir][0];
    const int32_t po2  = svt_aom_eb_cdef_directions[dir][1];
    const int32_t s1o1 = svt_aom_eb_cdef_directions[(dir + 2)][0];
    const int32_t s1o2 = svt_aom_eb_cdef_directions[(dir + 2)][1];
    const int32_t s2o1 = svt_aom_eb_cdef_directions[(dir - 2)][0];
    const int32_t s2o2 = svt_aom_eb_cdef_directions[(dir - 2)][1];

    const int32_t* pri_taps         = svt_aom_eb_cdef_pri_taps[(pri_strength >> coeff_shift) & 1];
    const int32_t* sec_taps         = svt_aom_eb_cdef_sec_taps[(pri_strength >> coeff_shift) & 1];
    __m256i        pri_strength_256 = _mm256_set1_epi16(pri_strength);
    __m256i        sec_strength_256 = _mm256_set1_epi16(sec_strength);

    if (pri_strength) {
        pri_damping = AOMMAX(0, pri_damping - get_msb(pri_strength));
    }
    if (sec_strength) {
        sec_damping = AOMMAX(0, sec_damping - get_msb(sec_strength));
    }

    for (uint32_t i = 0; i < height; i += (4 * subsampling_factor)) {
        sum = _mm256_setzero_si256();
        row = _mm256_set_epi64x(*(uint64_t*)(in + i * CDEF_BSTRIDE),
                                *(uint64_t*)(in + (i + (1 * subsampling_factor)) * CDEF_BSTRIDE),
                                *(uint64_t*)(in + (i + (2 * subsampling_factor)) * CDEF_BSTRIDE),
                                *(uint64_t*)(in + (i + (3 * subsampling_factor)) * CDEF_BSTRIDE));
        min = max = row;

        if (pri_strength) {
            // Primary near taps
            p0 = _mm256_set_epi64x(*(uint64_t*)(in + i * CDEF_BSTRIDE + po1),
                                   *(uint64_t*)(in + (i + (1 * subsampling_factor)) * CDEF_BSTRIDE + po1),
                                   *(uint64_t*)(in + (i + (2 * subsampling_factor)) * CDEF_BSTRIDE + po1),
                                   *(uint64_t*)(in + (i + (3 * subsampling_factor)) * CDEF_BSTRIDE + po1));
            p1 = _mm256_set_epi64x(*(uint64_t*)(in + i * CDEF_BSTRIDE - po1),
                                   *(uint64_t*)(in + (i + (1 * subsampling_factor)) * CDEF_BSTRIDE - po1),
                                   *(uint64_t*)(in + (i + (2 * subsampling_factor)) * CDEF_BSTRIDE - po1),
                                   *(uint64_t*)(in + (i + (3 * subsampling_factor)) * CDEF_BSTRIDE - po1));

            max = _mm256_max_epi16(_mm256_max_epi16(max, _mm256_andnot_si256(_mm256_cmpeq_epi16(p0, large), p0)),
                                   _mm256_andnot_si256(_mm256_cmpeq_epi16(p1, large), p1));
            min = _mm256_min_epi16(_mm256_min_epi16(min, p0), p1);
            p0  = constrain16(p0, row, pri_strength_256, pri_damping);
            p1  = constrain16(p1, row, pri_strength_256, pri_damping);

            // sum += pri_taps[0] * (p0 + p1)
            sum = _mm256_add_epi16(sum, _mm256_mullo_epi16(_mm256_set1_epi16(pri_taps[0]), _mm256_add_epi16(p0, p1)));

            // Primary far taps
            p0  = _mm256_set_epi64x(*(uint64_t*)(in + i * CDEF_BSTRIDE + po2),
                                   *(uint64_t*)(in + (i + (1 * subsampling_factor)) * CDEF_BSTRIDE + po2),
                                   *(uint64_t*)(in + (i + (2 * subsampling_factor)) * CDEF_BSTRIDE + po2),
                                   *(uint64_t*)(in + (i + (3 * subsampling_factor)) * CDEF_BSTRIDE + po2));
            p1  = _mm256_set_epi64x(*(uint64_t*)(in + i * CDEF_BSTRIDE - po2),
                                   *(uint64_t*)(in + (i + (1 * subsampling_factor)) * CDEF_BSTRIDE - po2),
                                   *(uint64_t*)(in + (i + (2 * subsampling_factor)) * CDEF_BSTRIDE - po2),
                                   *(uint64_t*)(in + (i + (3 * subsampling_factor)) * CDEF_BSTRIDE - po2));
            max = _mm256_max_epi16(_mm256_max_epi16(max, _mm256_andnot_si256(_mm256_cmpeq_epi16(p0, large), p0)),
                                   _mm256_andnot_si256(_mm256_cmpeq_epi16(p1, large), p1));
            min = _mm256_min_epi16(_mm256_min_epi16(min, p0), p1);
            p0  = constrain16(p0, row, pri_strength_256, pri_damping);
            p1  = constrain16(p1, row, pri_strength_256, pri_damping);

            // sum += pri_taps[1] * (p0 + p1)
            sum = _mm256_add_epi16(sum, _mm256_mullo_epi16(_mm256_set1_epi16(pri_taps[1]), _mm256_add_epi16(p0, p1)));
        }

        if (sec_strength) {
            // Secondary near taps
            p0  = _mm256_set_epi64x(*(uint64_t*)(in + i * CDEF_BSTRIDE + s1o1),
                                   *(uint64_t*)(in + (i + (1 * subsampling_factor)) * CDEF_BSTRIDE + s1o1),
                                   *(uint64_t*)(in + (i + (2 * subsampling_factor)) * CDEF_BSTRIDE + s1o1),
                                   *(uint64_t*)(in + (i + (3 * subsampling_factor)) * CDEF_BSTRIDE + s1o1));
            p1  = _mm256_set_epi64x(*(uint64_t*)(in + i * CDEF_BSTRIDE - s1o1),
                                   *(uint64_t*)(in + (i + (1 * subsampling_factor)) * CDEF_BSTRIDE - s1o1),
                                   *(uint64_t*)(in + (i + (2 * subsampling_factor)) * CDEF_BSTRIDE - s1o1),
                                   *(uint64_t*)(in + (i + (3 * subsampling_factor)) * CDEF_BSTRIDE - s1o1));
            p2  = _mm256_set_epi64x(*(uint64_t*)(in + i * CDEF_BSTRIDE + s2o1),
                                   *(uint64_t*)(in + (i + (1 * subsampling_factor)) * CDEF_BSTRIDE + s2o1),
                                   *(uint64_t*)(in + (i + (2 * subsampling_factor)) * CDEF_BSTRIDE + s2o1),
                                   *(uint64_t*)(in + (i + (3 * subsampling_factor)) * CDEF_BSTRIDE + s2o1));
            p3  = _mm256_set_epi64x(*(uint64_t*)(in + i * CDEF_BSTRIDE - s2o1),
                                   *(uint64_t*)(in + (i + (1 * subsampling_factor)) * CDEF_BSTRIDE - s2o1),
                                   *(uint64_t*)(in + (i + (2 * subsampling_factor)) * CDEF_BSTRIDE - s2o1),
                                   *(uint64_t*)(in + (i + (3 * subsampling_factor)) * CDEF_BSTRIDE - s2o1));
            max = _mm256_max_epi16(_mm256_max_epi16(max, _mm256_andnot_si256(_mm256_cmpeq_epi16(p0, large), p0)),
                                   _mm256_andnot_si256(_mm256_cmpeq_epi16(p1, large), p1));
            max = _mm256_max_epi16(_mm256_max_epi16(max, _mm256_andnot_si256(_mm256_cmpeq_epi16(p2, large), p2)),
                                   _mm256_andnot_si256(_mm256_cmpeq_epi16(p3, large), p3));
            min = _mm256_min_epi16(_mm256_min_epi16(_mm256_min_epi16(_mm256_min_epi16(min, p0), p1), p2), p3);
            p0  = constrain16(p0, row, sec_strength_256, sec_damping);
            p1  = constrain16(p1, row, sec_strength_256, sec_damping);
            p2  = constrain16(p2, row, sec_strength_256, sec_damping);
            p3  = constrain16(p3, row, sec_strength_256, sec_damping);

            // sum += sec_taps[0] * (p0 + p1 + p2 + p3)
            sum = _mm256_add_epi16(
                sum,
                _mm256_mullo_epi16(_mm256_set1_epi16(sec_taps[0]),
                                   _mm256_add_epi16(_mm256_add_epi16(p0, p1), _mm256_add_epi16(p2, p3))));

            // Secondary far taps
            p0  = _mm256_set_epi64x(*(uint64_t*)(in + i * CDEF_BSTRIDE + s1o2),
                                   *(uint64_t*)(in + (i + (1 * subsampling_factor)) * CDEF_BSTRIDE + s1o2),
                                   *(uint64_t*)(in + (i + (2 * subsampling_factor)) * CDEF_BSTRIDE + s1o2),
                                   *(uint64_t*)(in + (i + (3 * subsampling_factor)) * CDEF_BSTRIDE + s1o2));
            p1  = _mm256_set_epi64x(*(uint64_t*)(in + i * CDEF_BSTRIDE - s1o2),
                                   *(uint64_t*)(in + (i + (1 * subsampling_factor)) * CDEF_BSTRIDE - s1o2),
                                   *(uint64_t*)(in + (i + (2 * subsampling_factor)) * CDEF_BSTRIDE - s1o2),
                                   *(uint64_t*)(in + (i + (3 * subsampling_factor)) * CDEF_BSTRIDE - s1o2));
            p2  = _mm256_set_epi64x(*(uint64_t*)(in + i * CDEF_BSTRIDE + s2o2),
                                   *(uint64_t*)(in + (i + (1 * subsampling_factor)) * CDEF_BSTRIDE + s2o2),
                                   *(uint64_t*)(in + (i + (2 * subsampling_factor)) * CDEF_BSTRIDE + s2o2),
                                   *(uint64_t*)(in + (i + (3 * subsampling_factor)) * CDEF_BSTRIDE + s2o2));
            p3  = _mm256_set_epi64x(*(uint64_t*)(in + i * CDEF_BSTRIDE - s2o2),
                                   *(uint64_t*)(in + (i + (1 * subsampling_factor)) * CDEF_BSTRIDE - s2o2),
                                   *(uint64_t*)(in + (i + (2 * subsampling_factor)) * CDEF_BSTRIDE - s2o2),
                                   *(uint64_t*)(in + (i + (3 * subsampling_factor)) * CDEF_BSTRIDE - s2o2));
            max = _mm256_max_epi16(_mm256_max_epi16(max, _mm256_andnot_si256(_mm256_cmpeq_epi16(p0, large), p0)),
                                   _mm256_andnot_si256(_mm256_cmpeq_epi16(p1, large), p1));
            max = _mm256_max_epi16(_mm256_max_epi16(max, _mm256_andnot_si256(_mm256_cmpeq_epi16(p2, large), p2)),
                                   _mm256_andnot_si256(_mm256_cmpeq_epi16(p3, large), p3));
            min = _mm256_min_epi16(_mm256_min_epi16(_mm256_min_epi16(_mm256_min_epi16(min, p0), p1), p2), p3);
            p0  = constrain16(p0, row, sec_strength_256, sec_damping);
            p1  = constrain16(p1, row, sec_strength_256, sec_damping);
            p2  = constrain16(p2, row, sec_strength_256, sec_damping);
            p3  = constrain16(p3, row, sec_strength_256, sec_damping);

            // sum += sec_taps[1] * (p0 + p1 + p2 + p3)
            sum = _mm256_add_epi16(
                sum,
                _mm256_mullo_epi16(_mm256_set1_epi16(sec_taps[1]),
                                   _mm256_add_epi16(_mm256_add_epi16(p0, p1), _mm256_add_epi16(p2, p3))));
        }

        // res = row + ((sum - (sum < 0) + 8) >> 4)
        sum = _mm256_add_epi16(sum, _mm256_cmpgt_epi16(_mm256_setzero_si256(), sum));
        res = _mm256_add_epi16(sum, _mm256_set1_epi16(8));
        res = _mm256_srai_epi16(res, 4);
        res = _mm256_add_epi16(row, res);
        res = _mm256_min_epi16(_mm256_max_epi16(res, min), max);
        res = _mm256_packus_epi16(res, res);

        *(int32_t*)(dst + i * dstride)                              = _mm256_extract_epi32(res, 5);
        *(int32_t*)(dst + (i + (1 * subsampling_factor)) * dstride) = _mm256_extract_epi32(res, 4);
        *(int32_t*)(dst + (i + (2 * subsampling_factor)) * dstride) = _mm256_extract_epi32(res, 1);
        *(int32_t*)(dst + (i + (3 * subsampling_factor)) * dstride) = _mm256_cvtsi256_si32(res);
    }
}

// subsampling_factor of 1 means no subsampling
// requires height/subsampling_factor >= 2
static void svt_cdef_filter_block_8xn_8_avx2(uint8_t* dst, int32_t dstride, const uint16_t* in, int32_t pri_strength,
                                             int32_t sec_strength, int32_t dir, int32_t pri_damping,
                                             int32_t sec_damping, int32_t coeff_shift, uint8_t height,
                                             uint8_t subsampling_factor) {
    int32_t       i;
    __m256i       sum, p0, p1, p2, p3, row, res;
    __m256i       max, min, large = _mm256_set1_epi16(CDEF_VERY_LARGE);
    const int32_t po1  = svt_aom_eb_cdef_directions[dir][0];
    const int32_t po2  = svt_aom_eb_cdef_directions[dir][1];
    const int32_t s1o1 = svt_aom_eb_cdef_directions[(dir + 2)][0];
    const int32_t s1o2 = svt_aom_eb_cdef_directions[(dir + 2)][1];
    const int32_t s2o1 = svt_aom_eb_cdef_directions[(dir - 2)][0];
    const int32_t s2o2 = svt_aom_eb_cdef_directions[(dir - 2)][1];
    // SSE CHKN
    const int32_t* pri_taps         = svt_aom_eb_cdef_pri_taps[(pri_strength >> coeff_shift) & 1];
    const int32_t* sec_taps         = svt_aom_eb_cdef_sec_taps[(pri_strength >> coeff_shift) & 1];
    __m256i        pri_taps_0       = _mm256_set1_epi16(pri_taps[0]);
    __m256i        pri_taps_1       = _mm256_set1_epi16(pri_taps[1]);
    __m256i        sec_taps_0       = _mm256_set1_epi16(sec_taps[0]);
    __m256i        sec_taps_1       = _mm256_set1_epi16(sec_taps[1]);
    __m256i        duplicate_8      = _mm256_set1_epi16(8);
    __m256i        pri_strength_256 = _mm256_set1_epi16(pri_strength);
    __m256i        sec_strength_256 = _mm256_set1_epi16(sec_strength);

    if (pri_strength) {
        pri_damping = AOMMAX(0, pri_damping - get_msb(pri_strength));
    }
    if (sec_strength) {
        sec_damping = AOMMAX(0, sec_damping - get_msb(sec_strength));
    }

    for (i = 0; i < height; i += (2 * subsampling_factor)) {
        sum = _mm256_setzero_si256();
        row = _mm256_setr_m128i(_mm_loadu_si128((__m128i*)(in + (i + subsampling_factor) * CDEF_BSTRIDE)),
                                _mm_loadu_si128((__m128i*)(in + i * CDEF_BSTRIDE)));

        min = max = row;
        if (pri_strength) {
            // Primary near taps
            p0  = _mm256_setr_m128i(_mm_loadu_si128((__m128i*)(in + (i + subsampling_factor) * CDEF_BSTRIDE + po1)),
                                   _mm_loadu_si128((__m128i*)(in + i * CDEF_BSTRIDE + po1)));
            p1  = _mm256_setr_m128i(_mm_loadu_si128((__m128i*)(in + (i + subsampling_factor) * CDEF_BSTRIDE - po1)),
                                   _mm_loadu_si128((__m128i*)(in + i * CDEF_BSTRIDE - po1)));
            max = _mm256_max_epi16(_mm256_max_epi16(max, _mm256_andnot_si256(_mm256_cmpeq_epi16(p0, large), p0)),
                                   _mm256_andnot_si256(_mm256_cmpeq_epi16(p1, large), p1));
            min = _mm256_min_epi16(_mm256_min_epi16(min, p0), p1);
            p0  = constrain16(p0, row, pri_strength_256, pri_damping);
            p1  = constrain16(p1, row, pri_strength_256, pri_damping);

            // sum += pri_taps[0] * (p0 + p1)
            sum = _mm256_add_epi16(sum, _mm256_mullo_epi16(pri_taps_0, _mm256_add_epi16(p0, p1)));

            // Primary far taps
            p0  = _mm256_setr_m128i(_mm_loadu_si128((__m128i*)(in + (i + subsampling_factor) * CDEF_BSTRIDE + po2)),
                                   _mm_loadu_si128((__m128i*)(in + i * CDEF_BSTRIDE + po2)));
            p1  = _mm256_setr_m128i(_mm_loadu_si128((__m128i*)(in + (i + subsampling_factor) * CDEF_BSTRIDE - po2)),
                                   _mm_loadu_si128((__m128i*)(in + i * CDEF_BSTRIDE - po2)));
            max = _mm256_max_epi16(_mm256_max_epi16(max, _mm256_andnot_si256(_mm256_cmpeq_epi16(p0, large), p0)),
                                   _mm256_andnot_si256(_mm256_cmpeq_epi16(p1, large), p1));
            min = _mm256_min_epi16(_mm256_min_epi16(min, p0), p1);
            p0  = constrain16(p0, row, pri_strength_256, pri_damping);
            p1  = constrain16(p1, row, pri_strength_256, pri_damping);

            // sum += pri_taps[1] * (p0 + p1)
            sum = _mm256_add_epi16(sum, _mm256_mullo_epi16(pri_taps_1, _mm256_add_epi16(p0, p1)));
        }

        if (sec_strength) {
            // Secondary near taps
            p0  = _mm256_setr_m128i(_mm_loadu_si128((__m128i*)(in + (i + subsampling_factor) * CDEF_BSTRIDE + s1o1)),
                                   _mm_loadu_si128((__m128i*)(in + i * CDEF_BSTRIDE + s1o1)));
            p1  = _mm256_setr_m128i(_mm_loadu_si128((__m128i*)(in + (i + subsampling_factor) * CDEF_BSTRIDE - s1o1)),
                                   _mm_loadu_si128((__m128i*)(in + i * CDEF_BSTRIDE - s1o1)));
            p2  = _mm256_setr_m128i(_mm_loadu_si128((__m128i*)(in + (i + subsampling_factor) * CDEF_BSTRIDE + s2o1)),
                                   _mm_loadu_si128((__m128i*)(in + i * CDEF_BSTRIDE + s2o1)));
            p3  = _mm256_setr_m128i(_mm_loadu_si128((__m128i*)(in + (i + subsampling_factor) * CDEF_BSTRIDE - s2o1)),
                                   _mm_loadu_si128((__m128i*)(in + i * CDEF_BSTRIDE - s2o1)));
            max = _mm256_max_epi16(_mm256_max_epi16(max, _mm256_andnot_si256(_mm256_cmpeq_epi16(p0, large), p0)),
                                   _mm256_andnot_si256(_mm256_cmpeq_epi16(p1, large), p1));
            max = _mm256_max_epi16(_mm256_max_epi16(max, _mm256_andnot_si256(_mm256_cmpeq_epi16(p2, large), p2)),
                                   _mm256_andnot_si256(_mm256_cmpeq_epi16(p3, large), p3));
            min = _mm256_min_epi16(_mm256_min_epi16(_mm256_min_epi16(_mm256_min_epi16(min, p0), p1), p2), p3);
            p0  = constrain16(p0, row, sec_strength_256, sec_damping);
            p1  = constrain16(p1, row, sec_strength_256, sec_damping);
            p2  = constrain16(p2, row, sec_strength_256, sec_damping);
            p3  = constrain16(p3, row, sec_strength_256, sec_damping);

            // sum += sec_taps[0] * (p0 + p1 + p2 + p3)
            sum = _mm256_add_epi16(
                sum,
                _mm256_mullo_epi16(sec_taps_0, _mm256_add_epi16(_mm256_add_epi16(p0, p1), _mm256_add_epi16(p2, p3))));

            // Secondary far taps
            p0  = _mm256_setr_m128i(_mm_loadu_si128((__m128i*)(in + (i + subsampling_factor) * CDEF_BSTRIDE + s1o2)),
                                   _mm_loadu_si128((__m128i*)(in + i * CDEF_BSTRIDE + s1o2)));
            p1  = _mm256_setr_m128i(_mm_loadu_si128((__m128i*)(in + (i + subsampling_factor) * CDEF_BSTRIDE - s1o2)),
                                   _mm_loadu_si128((__m128i*)(in + i * CDEF_BSTRIDE - s1o2)));
            p2  = _mm256_setr_m128i(_mm_loadu_si128((__m128i*)(in + (i + subsampling_factor) * CDEF_BSTRIDE + s2o2)),
                                   _mm_loadu_si128((__m128i*)(in + i * CDEF_BSTRIDE + s2o2)));
            p3  = _mm256_setr_m128i(_mm_loadu_si128((__m128i*)(in + (i + subsampling_factor) * CDEF_BSTRIDE - s2o2)),
                                   _mm_loadu_si128((__m128i*)(in + i * CDEF_BSTRIDE - s2o2)));
            max = _mm256_max_epi16(_mm256_max_epi16(max, _mm256_andnot_si256(_mm256_cmpeq_epi16(p0, large), p0)),
                                   _mm256_andnot_si256(_mm256_cmpeq_epi16(p1, large), p1));
            max = _mm256_max_epi16(_mm256_max_epi16(max, _mm256_andnot_si256(_mm256_cmpeq_epi16(p2, large), p2)),
                                   _mm256_andnot_si256(_mm256_cmpeq_epi16(p3, large), p3));
            min = _mm256_min_epi16(_mm256_min_epi16(_mm256_min_epi16(_mm256_min_epi16(min, p0), p1), p2), p3);
            p0  = constrain16(p0, row, sec_strength_256, sec_damping);
            p1  = constrain16(p1, row, sec_strength_256, sec_damping);
            p2  = constrain16(p2, row, sec_strength_256, sec_damping);
            p3  = constrain16(p3, row, sec_strength_256, sec_damping);

            // sum += sec_taps[1] * (p0 + p1 + p2 + p3)
            sum = _mm256_add_epi16(
                sum,
                _mm256_mullo_epi16(sec_taps_1, _mm256_add_epi16(_mm256_add_epi16(p0, p1), _mm256_add_epi16(p2, p3))));
        }

        // res = row + ((sum - (sum < 0) + 8) >> 4)
        sum                            = _mm256_add_epi16(sum, _mm256_cmpgt_epi16(_mm256_setzero_si256(), sum));
        res                            = _mm256_add_epi16(sum, duplicate_8);
        res                            = _mm256_srai_epi16(res, 4);
        res                            = _mm256_add_epi16(row, res);
        res                            = _mm256_min_epi16(_mm256_max_epi16(res, min), max);
        res                            = _mm256_packus_epi16(res, res);
        *(int64_t*)(dst + i * dstride) = _mm256_extract_epi64(res, 2);
        *(int64_t*)(dst + (i + subsampling_factor) * dstride) = _mm256_extract_epi64(res, 0);
    }
}

// subsampling_factor of 1 means no subsampling
// Currently supports subsampling_factor values 1 and 2
void svt_cdef_filter_block_avx2(uint8_t* dst8, uint16_t* dst16, int32_t dstride, const uint16_t* in,
                                int32_t pri_strength, int32_t sec_strength, int32_t dir, int32_t pri_damping,
                                int32_t sec_damping, int32_t bsize, int32_t coeff_shift, uint8_t subsampling_factor) {
    if (dst8) {
        if (bsize == BLOCK_8X8) {
            svt_cdef_filter_block_8xn_8_avx2(dst8,
                                             dstride,
                                             in,
                                             pri_strength,
                                             sec_strength,
                                             dir,
                                             pri_damping,
                                             sec_damping,
                                             coeff_shift,
                                             8,
                                             subsampling_factor);
        } else if (bsize == BLOCK_4X8) {
            svt_cdef_filter_block_4xn_8_avx2(dst8,
                                             dstride,
                                             in,
                                             pri_strength,
                                             sec_strength,
                                             dir,
                                             pri_damping,
                                             sec_damping,
                                             coeff_shift,
                                             8,
                                             subsampling_factor);
        } else if (bsize == BLOCK_8X4) {
            svt_cdef_filter_block_8xn_8_avx2(dst8,
                                             dstride,
                                             in,
                                             pri_strength,
                                             sec_strength,
                                             dir,
                                             pri_damping,
                                             sec_damping,
                                             coeff_shift,
                                             4,
                                             subsampling_factor);
        } else {
            svt_cdef_filter_block_4xn_8_avx2(dst8,
                                             dstride,
                                             in,
                                             pri_strength,
                                             sec_strength,
                                             dir,
                                             pri_damping,
                                             sec_damping,
                                             coeff_shift,
                                             4,
                                             1); // subsampling facotr of 1 b/c can't subsample 4x4 - done as one shot
        }
    } else {
        if (bsize == BLOCK_8X8) {
            //When subsampling_factor is 4 then we cannot use AVX512 kernel because it load 4 lines(block height 16 in this case)
            if (subsampling_factor == 4) {
                svt_cdef_filter_block_8xn_16_avx2(in,
                                                  pri_strength,
                                                  sec_strength,
                                                  dir,
                                                  pri_damping,
                                                  sec_damping,
                                                  coeff_shift,
                                                  dst16,
                                                  dstride,
                                                  8,
                                                  subsampling_factor);
            } else {
                svt_cdef_filter_block_8xn_16(in,
                                             pri_strength,
                                             sec_strength,
                                             dir,
                                             pri_damping,
                                             sec_damping,
                                             coeff_shift,
                                             dst16,
                                             dstride,
                                             8,
                                             subsampling_factor);
            }
        } else if (bsize == BLOCK_4X8) {
            svt_cdef_filter_block_4xn_16_avx2(dst16,
                                              dstride,
                                              in,
                                              pri_strength,
                                              sec_strength,
                                              dir,
                                              pri_damping,
                                              sec_damping,
                                              coeff_shift,
                                              8,
                                              subsampling_factor);
        } else if (bsize == BLOCK_8X4) {
            svt_cdef_filter_block_8xn_16_avx2(in,
                                              pri_strength,
                                              sec_strength,
                                              dir,
                                              pri_damping,
                                              sec_damping,
                                              coeff_shift,
                                              dst16,
                                              dstride,
                                              4,
                                              subsampling_factor);
        } else {
            assert(bsize == BLOCK_4X4);
            svt_cdef_filter_block_4xn_16_avx2(dst16,
                                              dstride,
                                              in,
                                              pri_strength,
                                              sec_strength,
                                              dir,
                                              pri_damping,
                                              sec_damping,
                                              coeff_shift,
                                              4,
                                              1); // subsampling facotr of 1 b/c can't subsample 4x4 - done as one shot
        }
    }
}

void svt_aom_copy_rect8_8bit_to_16bit_avx2(uint16_t* dst, int32_t dstride, const uint8_t* src, int32_t sstride,
                                           int32_t v, int32_t h) {
    int i = 0, j = 0;
    int remaining_width = h;

    // Process multiple 16 pixels at a time.
    if (h > 15) {
        for (i = 0; i < v; i++) {
            for (j = 0; j < h - 15; j += 16) {
                __m128i row = _mm_loadu_si128((__m128i*)&src[i * sstride + j]);
                _mm256_storeu_si256((__m256i*)&dst[i * dstride + j], _mm256_cvtepu8_epi16(row));
            }
        }
        remaining_width = h & 0xe;
    }

    // Process multiple 8 pixels at a time.
    if (remaining_width > 7) {
        for (i = 0; i < v; i++) {
            __m128i row = _mm_loadl_epi64((__m128i*)&src[i * sstride + j]);
            _mm_storeu_si128((__m128i*)&dst[i * dstride + j], _mm_unpacklo_epi8(row, _mm_setzero_si128()));
        }
        remaining_width = h & 0x7;
        j += 8;
    }

    // Process the remaining pixels.
    if (remaining_width) {
        for (i = 0; i < v; i++) {
            for (int k = j; k < h; k++) {
                dst[i * dstride + k] = src[i * sstride + k];
            }
        }
    }
}
