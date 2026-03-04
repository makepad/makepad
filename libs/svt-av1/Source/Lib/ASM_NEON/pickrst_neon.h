/*
 * Copyright (c) 2023, Alliance for Open Media. All rights reserved
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at www.aomedia.org/license/software. If the Alliance for Open
 * Media Patent License 1.0 was not distributed with this source code in the
 * PATENTS file, you can obtain it at www.aomedia.org/license/patent.
 */

#ifndef PICKRST_NEON_H
#define PICKRST_NEON_H

#include <arm_neon.h>

#include "aom_dsp_rtcd.h"
#include "definitions.h"
#include "mem_neon.h"
#include "restoration.h"
#include "restoration_pick.h"
#include "sum_neon.h"
#include "transpose_neon.h"
#include "utility.h"

#define WIN_7 ((WIENER_WIN - 1) * 2)
#define WIN_CHROMA ((WIENER_WIN_CHROMA - 1) * 2)

// clang-format off
// Constant pool to act as a mask to zero n top elements in an int16x8_t vector.
// The index we load from depends on n.
static const int16_t mask_16bit[32] = {
  0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff,
  0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff,
       0,      0,      0,      0,      0,      0,      0,      0,
       0,      0,      0,      0,      0,      0,      0,      0,
};
// clang-format on

static inline void madd_neon_pairwise(int32x4_t* sum, const int16x8_t src, const int16x8_t dgd) {
    const int32x4_t sd = vpaddq_s32(vmull_s16(vget_low_s16(src), vget_low_s16(dgd)),
                                    vmull_s16(vget_high_s16(src), vget_high_s16(dgd)));
    *sum               = vaddq_s32(*sum, sd);
}

static inline void madd_neon(int32x4_t* sum, const int16x8_t src, const int16x8_t dgd) {
    *sum = vmlal_s16(*sum, vget_low_s16(src), vget_low_s16(dgd));
    *sum = vmlal_s16(*sum, vget_high_s16(src), vget_high_s16(dgd));
}

static inline void msub_neon(int32x4_t* sum, const int16x8_t src, const int16x8_t dgd) {
    *sum = vmlsl_s16(*sum, vget_low_s16(src), vget_low_s16(dgd));
    *sum = vmlsl_s16(*sum, vget_high_s16(src), vget_high_s16(dgd));
}

static inline void compute_delta_step3_win7(int32x4_t* sum, const int16x8_t src0, const int16x8_t src1,
                                            const int16x8_t dgd0, const int16x8_t dgd1) {
    sum[0] = vmlsl_s16(sum[0], vget_low_s16(src0), vget_low_s16(dgd0));
    sum[0] = vmlal_s16(sum[0], vget_low_s16(src1), vget_low_s16(dgd1));
    sum[1] = vmlsl_s16(sum[1], vget_high_s16(src0), vget_high_s16(dgd0));
    sum[1] = vmlal_s16(sum[1], vget_high_s16(src1), vget_high_s16(dgd1));
}

static inline int32x4_t hadd_four_32_neon(const int32x4_t src0, const int32x4_t src1, const int32x4_t src2,
                                          const int32x4_t src3) {
    int32x4_t src[4] = {src0, src1, src2, src3};
    return horizontal_add_4d_s32x4(src);
}

static inline void load_more_32_neon(const int16_t* const src, const int32_t width, int32x4_t dst[2]) {
    int32x4_t s0 = vld1q_dup_s32((int32_t*)src);
    int32x4_t s1 = vld1q_dup_s32((int32_t*)(src + width));
    dst[0]       = vextq_s32(dst[0], s0, 1);
    dst[1]       = vextq_s32(dst[1], s1, 1);
}

static inline void update_4_stats_neon(const int64_t* const src, const int32x4_t delta, int64_t* const dst) {
    const int64x2_t s1 = vld1q_s64(src);
    const int64x2_t s2 = vld1q_s64(src + 2);

    const int64x2_t d1 = vaddw_s32(s1, vget_low_s32(delta));
    const int64x2_t d2 = vaddw_s32(s2, vget_high_s32(delta));

    vst1q_s64(dst, d1);
    vst1q_s64(dst + 2, d2);
}

static inline void load_more_16_neon(const int16_t* const src, const int32_t width, const int16x8_t org[2],
                                     int16x8_t dst[2]) {
    int16x8_t s0 = vld1q_dup_s16(src);
    int16x8_t s1 = vld1q_dup_s16(src + width);
    dst[0]       = vextq_s16(org[0], s0, 1);
    dst[1]       = vextq_s16(org[1], s1, 1);
}

static inline void update_2_stats_neon(const int64_t* const src, const int64x2_t delta, int64_t* const dst) {
    const int64x2_t s = vld1q_s64(src);
    const int64x2_t d = vaddq_s64(s, delta);
    vst1q_s64(dst, d);
}

static inline void stats_top_win5_neon(const int16x8_t src[2], const int16x8_t dgd[2], const int16_t* const d,
                                       const int32_t d_stride, int32x4_t* sum_m, int32x4_t* sum_h) {
    int16x8_t dgds[WIENER_WIN_CHROMA * 2];

    load_s16_8x5(d + 0, d_stride, &dgds[0], &dgds[2], &dgds[4], &dgds[6], &dgds[8]);
    load_s16_8x5(d + 8, d_stride, &dgds[1], &dgds[3], &dgds[5], &dgds[7], &dgds[9]);

    madd_neon(&sum_m[0], src[0], dgds[0]);
    madd_neon(&sum_m[0], src[1], dgds[1]);
    madd_neon(&sum_m[1], src[0], dgds[2]);
    madd_neon(&sum_m[1], src[1], dgds[3]);
    madd_neon(&sum_m[2], src[0], dgds[4]);
    madd_neon(&sum_m[2], src[1], dgds[5]);
    madd_neon(&sum_m[3], src[0], dgds[6]);
    madd_neon(&sum_m[3], src[1], dgds[7]);
    madd_neon(&sum_m[4], src[0], dgds[8]);
    madd_neon(&sum_m[4], src[1], dgds[9]);

    madd_neon(&sum_h[0], dgd[0], dgds[0]);
    madd_neon(&sum_h[0], dgd[1], dgds[1]);
    madd_neon(&sum_h[1], dgd[0], dgds[2]);
    madd_neon(&sum_h[1], dgd[1], dgds[3]);
    madd_neon(&sum_h[2], dgd[0], dgds[4]);
    madd_neon(&sum_h[2], dgd[1], dgds[5]);
    madd_neon(&sum_h[3], dgd[0], dgds[6]);
    madd_neon(&sum_h[3], dgd[1], dgds[7]);
    madd_neon(&sum_h[4], dgd[0], dgds[8]);
    madd_neon(&sum_h[4], dgd[1], dgds[9]);
}

static inline void stats_left_win5_neon(const int16x8_t src[2], const int16_t* d, const int32_t d_stride,
                                        int32x4_t* sum) {
    int16x8_t dgds[WIN_CHROMA];

    load_s16_8x4(d + d_stride + 0, d_stride, &dgds[0], &dgds[2], &dgds[4], &dgds[6]);
    load_s16_8x4(d + d_stride + 8, d_stride, &dgds[1], &dgds[3], &dgds[5], &dgds[7]);

    madd_neon(&sum[0], src[0], dgds[0]);
    madd_neon(&sum[0], src[1], dgds[1]);
    madd_neon(&sum[1], src[0], dgds[2]);
    madd_neon(&sum[1], src[1], dgds[3]);
    madd_neon(&sum[2], src[0], dgds[4]);
    madd_neon(&sum[2], src[1], dgds[5]);
    madd_neon(&sum[3], src[0], dgds[6]);
    madd_neon(&sum[3], src[1], dgds[7]);
}

static inline void derive_square_win5_neon(const int16x8_t* d_is, const int16x8_t* d_ie, const int16x8_t* d_js,
                                           const int16x8_t* d_je,
                                           int32x4_t        deltas[WIENER_WIN_CHROMA - 1][WIENER_WIN_CHROMA - 1]) {
    msub_neon(&deltas[0][0], d_is[0], d_js[0]);
    msub_neon(&deltas[0][0], d_is[1], d_js[1]);
    msub_neon(&deltas[0][1], d_is[0], d_js[2]);
    msub_neon(&deltas[0][1], d_is[1], d_js[3]);
    msub_neon(&deltas[0][2], d_is[0], d_js[4]);
    msub_neon(&deltas[0][2], d_is[1], d_js[5]);
    msub_neon(&deltas[0][3], d_is[0], d_js[6]);
    msub_neon(&deltas[0][3], d_is[1], d_js[7]);

    msub_neon(&deltas[1][0], d_is[2], d_js[0]);
    msub_neon(&deltas[1][0], d_is[3], d_js[1]);
    msub_neon(&deltas[1][1], d_is[2], d_js[2]);
    msub_neon(&deltas[1][1], d_is[3], d_js[3]);
    msub_neon(&deltas[1][2], d_is[2], d_js[4]);
    msub_neon(&deltas[1][2], d_is[3], d_js[5]);
    msub_neon(&deltas[1][3], d_is[2], d_js[6]);
    msub_neon(&deltas[1][3], d_is[3], d_js[7]);

    msub_neon(&deltas[2][0], d_is[4], d_js[0]);
    msub_neon(&deltas[2][0], d_is[5], d_js[1]);
    msub_neon(&deltas[2][1], d_is[4], d_js[2]);
    msub_neon(&deltas[2][1], d_is[5], d_js[3]);
    msub_neon(&deltas[2][2], d_is[4], d_js[4]);
    msub_neon(&deltas[2][2], d_is[5], d_js[5]);
    msub_neon(&deltas[2][3], d_is[4], d_js[6]);
    msub_neon(&deltas[2][3], d_is[5], d_js[7]);

    msub_neon(&deltas[3][0], d_is[6], d_js[0]);
    msub_neon(&deltas[3][0], d_is[7], d_js[1]);
    msub_neon(&deltas[3][1], d_is[6], d_js[2]);
    msub_neon(&deltas[3][1], d_is[7], d_js[3]);
    msub_neon(&deltas[3][2], d_is[6], d_js[4]);
    msub_neon(&deltas[3][2], d_is[7], d_js[5]);
    msub_neon(&deltas[3][3], d_is[6], d_js[6]);
    msub_neon(&deltas[3][3], d_is[7], d_js[7]);

    madd_neon(&deltas[0][0], d_ie[0], d_je[0]);
    madd_neon(&deltas[0][0], d_ie[1], d_je[1]);
    madd_neon(&deltas[0][1], d_ie[0], d_je[2]);
    madd_neon(&deltas[0][1], d_ie[1], d_je[3]);
    madd_neon(&deltas[0][2], d_ie[0], d_je[4]);
    madd_neon(&deltas[0][2], d_ie[1], d_je[5]);
    madd_neon(&deltas[0][3], d_ie[0], d_je[6]);
    madd_neon(&deltas[0][3], d_ie[1], d_je[7]);

    madd_neon(&deltas[1][0], d_ie[2], d_je[0]);
    madd_neon(&deltas[1][0], d_ie[3], d_je[1]);
    madd_neon(&deltas[1][1], d_ie[2], d_je[2]);
    madd_neon(&deltas[1][1], d_ie[3], d_je[3]);
    madd_neon(&deltas[1][2], d_ie[2], d_je[4]);
    madd_neon(&deltas[1][2], d_ie[3], d_je[5]);
    madd_neon(&deltas[1][3], d_ie[2], d_je[6]);
    madd_neon(&deltas[1][3], d_ie[3], d_je[7]);

    madd_neon(&deltas[2][0], d_ie[4], d_je[0]);
    madd_neon(&deltas[2][0], d_ie[5], d_je[1]);
    madd_neon(&deltas[2][1], d_ie[4], d_je[2]);
    madd_neon(&deltas[2][1], d_ie[5], d_je[3]);
    madd_neon(&deltas[2][2], d_ie[4], d_je[4]);
    madd_neon(&deltas[2][2], d_ie[5], d_je[5]);
    madd_neon(&deltas[2][3], d_ie[4], d_je[6]);
    madd_neon(&deltas[2][3], d_ie[5], d_je[7]);

    madd_neon(&deltas[3][0], d_ie[6], d_je[0]);
    madd_neon(&deltas[3][0], d_ie[7], d_je[1]);
    madd_neon(&deltas[3][1], d_ie[6], d_je[2]);
    madd_neon(&deltas[3][1], d_ie[7], d_je[3]);
    madd_neon(&deltas[3][2], d_ie[6], d_je[4]);
    madd_neon(&deltas[3][2], d_ie[7], d_je[5]);
    madd_neon(&deltas[3][3], d_ie[6], d_je[6]);
    madd_neon(&deltas[3][3], d_ie[7], d_je[7]);
}

static inline void load_square_win5_neon(const int16_t* const di, const int16_t* const dj, const int32_t d_stride,
                                         const int32_t height, int16x8_t* d_is, int16x8_t* d_ie, int16x8_t* d_js,
                                         int16x8_t* d_je) {
    load_s16_8x4(di + 0, d_stride, &d_is[0], &d_is[2], &d_is[4], &d_is[6]);
    load_s16_8x4(di + 8, d_stride, &d_is[1], &d_is[3], &d_is[5], &d_is[7]);
    load_s16_8x4(dj + 0, d_stride, &d_js[0], &d_js[2], &d_js[4], &d_js[6]);
    load_s16_8x4(dj + 8, d_stride, &d_js[1], &d_js[3], &d_js[5], &d_js[7]);

    load_s16_8x4(di + height * d_stride + 0, d_stride, &d_ie[0], &d_ie[2], &d_ie[4], &d_ie[6]);
    load_s16_8x4(di + height * d_stride + 8, d_stride, &d_ie[1], &d_ie[3], &d_ie[5], &d_ie[7]);
    load_s16_8x4(dj + height * d_stride + 0, d_stride, &d_je[0], &d_je[2], &d_je[4], &d_je[6]);
    load_s16_8x4(dj + height * d_stride + 8, d_stride, &d_je[1], &d_je[3], &d_je[5], &d_je[7]);
}

static inline void hadd_update_4_stats_neon(const int64_t* const src, const int32x4_t* deltas, int64_t* const dst) {
    int32x4_t delta = horizontal_add_4d_s32x4(deltas);

    int64x2_t src0 = vld1q_s64(src);
    int64x2_t src1 = vld1q_s64(src + 2);
    vst1q_s64(dst, vaddw_s32(src0, vget_low_s32(delta)));
    vst1q_s64(dst + 2, vaddw_s32(src1, vget_high_s32(delta)));
}

static inline void update_5_stats_neon(const int64_t* const src, const int32x4_t delta, const int64_t delta4,
                                       int64_t* const dst) {
    update_4_stats_neon(src + 0, delta, dst + 0);
    dst[4] = src[4] + delta4;
}

static inline void compute_delta_step3_win5(int32x4_t* sum, const int16x8_t src, const int16x8_t dgd) {
    *sum = vmlsl_s16(*sum, vget_low_s16(src), vget_low_s16(dgd));
    *sum = vmlal_s16(*sum, vget_high_s16(src), vget_high_s16(dgd));
}

static inline void step3_win5_neon(const int16_t* d, const int32_t d_stride, const int32_t width, const int32_t height,
                                   int16x8_t* ds, int32x4_t* deltas) {
    int32_t y = height;
    do {
        ds[4] = load_s16_4x2(d + 0 * d_stride, width);
        ds[5] = load_s16_4x2(d + 1 * d_stride, width);

        compute_delta_step3_win5(&deltas[0], ds[0], ds[0]);
        compute_delta_step3_win5(&deltas[1], ds[0], ds[1]);
        compute_delta_step3_win5(&deltas[2], ds[0], ds[2]);
        compute_delta_step3_win5(&deltas[3], ds[0], ds[3]);
        compute_delta_step3_win5(&deltas[4], ds[0], ds[4]);

        compute_delta_step3_win5(&deltas[0], ds[1], ds[1]);
        compute_delta_step3_win5(&deltas[1], ds[1], ds[2]);
        compute_delta_step3_win5(&deltas[2], ds[1], ds[3]);
        compute_delta_step3_win5(&deltas[3], ds[1], ds[4]);
        compute_delta_step3_win5(&deltas[4], ds[1], ds[5]);

        ds[0] = ds[2];
        ds[1] = ds[3];
        ds[2] = ds[4];
        ds[3] = ds[5];

        d += 2 * d_stride;
        y -= 2;
    } while (y > 1);

    if (y) {
        ds[4] = load_s16_4x2(d + 0 * d_stride, width);

        compute_delta_step3_win5(&deltas[0], ds[0], ds[0]);
        compute_delta_step3_win5(&deltas[1], ds[0], ds[1]);
        compute_delta_step3_win5(&deltas[2], ds[0], ds[2]);
        compute_delta_step3_win5(&deltas[3], ds[0], ds[3]);
        compute_delta_step3_win5(&deltas[4], ds[0], ds[4]);
    }
}

static inline void derive_triangle_win5_neon(const int16x8_t* d_is, const int16x8_t* d_ie, int32x4_t* deltas) {
    msub_neon(&deltas[0], d_is[0], d_is[0]);
    msub_neon(&deltas[0], d_is[1], d_is[1]);
    msub_neon(&deltas[1], d_is[0], d_is[2]);
    msub_neon(&deltas[1], d_is[1], d_is[3]);
    msub_neon(&deltas[2], d_is[0], d_is[4]);
    msub_neon(&deltas[2], d_is[1], d_is[5]);
    msub_neon(&deltas[3], d_is[0], d_is[6]);
    msub_neon(&deltas[3], d_is[1], d_is[7]);
    msub_neon(&deltas[4], d_is[2], d_is[2]);
    msub_neon(&deltas[4], d_is[3], d_is[3]);
    msub_neon(&deltas[5], d_is[2], d_is[4]);
    msub_neon(&deltas[5], d_is[3], d_is[5]);
    msub_neon(&deltas[6], d_is[2], d_is[6]);
    msub_neon(&deltas[6], d_is[3], d_is[7]);
    msub_neon(&deltas[7], d_is[4], d_is[4]);
    msub_neon(&deltas[7], d_is[5], d_is[5]);
    msub_neon(&deltas[8], d_is[4], d_is[6]);
    msub_neon(&deltas[8], d_is[5], d_is[7]);
    msub_neon(&deltas[9], d_is[6], d_is[6]);
    msub_neon(&deltas[9], d_is[7], d_is[7]);

    madd_neon(&deltas[0], d_ie[0], d_ie[0]);
    madd_neon(&deltas[0], d_ie[1], d_ie[1]);
    madd_neon(&deltas[1], d_ie[0], d_ie[2]);
    madd_neon(&deltas[1], d_ie[1], d_ie[3]);
    madd_neon(&deltas[2], d_ie[0], d_ie[4]);
    madd_neon(&deltas[2], d_ie[1], d_ie[5]);
    madd_neon(&deltas[3], d_ie[0], d_ie[6]);
    madd_neon(&deltas[3], d_ie[1], d_ie[7]);
    madd_neon(&deltas[4], d_ie[2], d_ie[2]);
    madd_neon(&deltas[4], d_ie[3], d_ie[3]);
    madd_neon(&deltas[5], d_ie[2], d_ie[4]);
    madd_neon(&deltas[5], d_ie[3], d_ie[5]);
    madd_neon(&deltas[6], d_ie[2], d_ie[6]);
    madd_neon(&deltas[6], d_ie[3], d_ie[7]);
    madd_neon(&deltas[7], d_ie[4], d_ie[4]);
    madd_neon(&deltas[7], d_ie[5], d_ie[5]);
    madd_neon(&deltas[8], d_ie[4], d_ie[6]);
    madd_neon(&deltas[8], d_ie[5], d_ie[7]);
    madd_neon(&deltas[9], d_ie[6], d_ie[6]);
    madd_neon(&deltas[9], d_ie[7], d_ie[7]);
}

static inline void load_triangle_win5_neon(const int16_t* const di, const int32_t d_stride, const int32_t height,
                                           int16x8_t* d_is, int16x8_t* d_ie) {
    load_s16_8x4(di + 0, d_stride, &d_is[0], &d_is[2], &d_is[4], &d_is[6]);
    load_s16_8x4(di + 8, d_stride, &d_is[1], &d_is[3], &d_is[5], &d_is[7]);

    load_s16_8x4(di + height * d_stride + 0, d_stride, &d_ie[0], &d_ie[2], &d_ie[4], &d_ie[6]);
    load_s16_8x4(di + height * d_stride + 8, d_stride, &d_ie[1], &d_ie[3], &d_ie[5], &d_ie[7]);
}

static inline void sub_deltas_step4(int16x8_t* A, int16x8_t* B, int32x4_t* deltas) {
    deltas[0] = vmlsl_s16(deltas[0], vget_low_s16(A[0]), vget_low_s16(B[0]));
    deltas[0] = vmlsl_s16(deltas[0], vget_high_s16(A[0]), vget_high_s16(B[0]));
    deltas[1] = vmlsl_s16(deltas[1], vget_low_s16(A[0]), vget_low_s16(B[1]));
    deltas[1] = vmlsl_s16(deltas[1], vget_high_s16(A[0]), vget_high_s16(B[1]));
    deltas[2] = vmlsl_s16(deltas[2], vget_low_s16(A[0]), vget_low_s16(B[2]));
    deltas[2] = vmlsl_s16(deltas[2], vget_high_s16(A[0]), vget_high_s16(B[2]));
    deltas[3] = vmlsl_s16(deltas[3], vget_low_s16(A[0]), vget_low_s16(B[3]));
    deltas[3] = vmlsl_s16(deltas[3], vget_high_s16(A[0]), vget_high_s16(B[3]));
    deltas[4] = vmlsl_s16(deltas[4], vget_low_s16(A[0]), vget_low_s16(B[4]));
    deltas[4] = vmlsl_s16(deltas[4], vget_high_s16(A[0]), vget_high_s16(B[4]));
    deltas[5] = vmlsl_s16(deltas[5], vget_low_s16(A[1]), vget_low_s16(B[0]));
    deltas[5] = vmlsl_s16(deltas[5], vget_high_s16(A[1]), vget_high_s16(B[0]));
    deltas[6] = vmlsl_s16(deltas[6], vget_low_s16(A[2]), vget_low_s16(B[0]));
    deltas[6] = vmlsl_s16(deltas[6], vget_high_s16(A[2]), vget_high_s16(B[0]));
    deltas[7] = vmlsl_s16(deltas[7], vget_low_s16(A[3]), vget_low_s16(B[0]));
    deltas[7] = vmlsl_s16(deltas[7], vget_high_s16(A[3]), vget_high_s16(B[0]));
    deltas[8] = vmlsl_s16(deltas[8], vget_low_s16(A[4]), vget_low_s16(B[0]));
    deltas[8] = vmlsl_s16(deltas[8], vget_high_s16(A[4]), vget_high_s16(B[0]));
}

static inline void add_deltas_step4(int16x8_t* A, int16x8_t* B, int32x4_t* deltas) {
    deltas[0] = vmlal_s16(deltas[0], vget_low_s16(A[0]), vget_low_s16(B[0]));
    deltas[0] = vmlal_s16(deltas[0], vget_high_s16(A[0]), vget_high_s16(B[0]));
    deltas[1] = vmlal_s16(deltas[1], vget_low_s16(A[0]), vget_low_s16(B[1]));
    deltas[1] = vmlal_s16(deltas[1], vget_high_s16(A[0]), vget_high_s16(B[1]));
    deltas[2] = vmlal_s16(deltas[2], vget_low_s16(A[0]), vget_low_s16(B[2]));
    deltas[2] = vmlal_s16(deltas[2], vget_high_s16(A[0]), vget_high_s16(B[2]));
    deltas[3] = vmlal_s16(deltas[3], vget_low_s16(A[0]), vget_low_s16(B[3]));
    deltas[3] = vmlal_s16(deltas[3], vget_high_s16(A[0]), vget_high_s16(B[3]));
    deltas[4] = vmlal_s16(deltas[4], vget_low_s16(A[0]), vget_low_s16(B[4]));
    deltas[4] = vmlal_s16(deltas[4], vget_high_s16(A[0]), vget_high_s16(B[4]));
    deltas[5] = vmlal_s16(deltas[5], vget_low_s16(A[1]), vget_low_s16(B[0]));
    deltas[5] = vmlal_s16(deltas[5], vget_high_s16(A[1]), vget_high_s16(B[0]));
    deltas[6] = vmlal_s16(deltas[6], vget_low_s16(A[2]), vget_low_s16(B[0]));
    deltas[6] = vmlal_s16(deltas[6], vget_high_s16(A[2]), vget_high_s16(B[0]));
    deltas[7] = vmlal_s16(deltas[7], vget_low_s16(A[3]), vget_low_s16(B[0]));
    deltas[7] = vmlal_s16(deltas[7], vget_high_s16(A[3]), vget_high_s16(B[0]));
    deltas[8] = vmlal_s16(deltas[8], vget_low_s16(A[4]), vget_low_s16(B[0]));
    deltas[8] = vmlal_s16(deltas[8], vget_high_s16(A[4]), vget_high_s16(B[0]));
}

static inline void stats_top_win7_neon(const int16x8_t src[2], const int16x8_t dgd[2], const int16_t* const d,
                                       const int32_t d_stride, int32x4_t* sum_m, int32x4_t* sum_h) {
    int16x8_t dgds[WIENER_WIN * 2];

    load_s16_8x7(d + 0, d_stride, &dgds[0], &dgds[2], &dgds[4], &dgds[6], &dgds[8], &dgds[10], &dgds[12]);
    load_s16_8x7(d + 8, d_stride, &dgds[1], &dgds[3], &dgds[5], &dgds[7], &dgds[9], &dgds[11], &dgds[13]);

    madd_neon(&sum_m[0], src[0], dgds[0]);
    madd_neon(&sum_m[0], src[1], dgds[1]);
    madd_neon(&sum_m[1], src[0], dgds[2]);
    madd_neon(&sum_m[1], src[1], dgds[3]);
    madd_neon(&sum_m[2], src[0], dgds[4]);
    madd_neon(&sum_m[2], src[1], dgds[5]);
    madd_neon(&sum_m[3], src[0], dgds[6]);
    madd_neon(&sum_m[3], src[1], dgds[7]);
    madd_neon(&sum_m[4], src[0], dgds[8]);
    madd_neon(&sum_m[4], src[1], dgds[9]);
    madd_neon(&sum_m[5], src[0], dgds[10]);
    madd_neon(&sum_m[5], src[1], dgds[11]);
    madd_neon(&sum_m[6], src[0], dgds[12]);
    madd_neon(&sum_m[6], src[1], dgds[13]);

    madd_neon(&sum_h[0], dgd[0], dgds[0]);
    madd_neon(&sum_h[0], dgd[1], dgds[1]);
    madd_neon(&sum_h[1], dgd[0], dgds[2]);
    madd_neon(&sum_h[1], dgd[1], dgds[3]);
    madd_neon(&sum_h[2], dgd[0], dgds[4]);
    madd_neon(&sum_h[2], dgd[1], dgds[5]);
    madd_neon(&sum_h[3], dgd[0], dgds[6]);
    madd_neon(&sum_h[3], dgd[1], dgds[7]);
    madd_neon(&sum_h[4], dgd[0], dgds[8]);
    madd_neon(&sum_h[4], dgd[1], dgds[9]);
    madd_neon(&sum_h[5], dgd[0], dgds[10]);
    madd_neon(&sum_h[5], dgd[1], dgds[11]);
    madd_neon(&sum_h[6], dgd[0], dgds[12]);
    madd_neon(&sum_h[6], dgd[1], dgds[13]);
}

static inline void derive_square_win7_neon(const int16x8_t* d_is, const int16x8_t* d_ie, const int16x8_t* d_js,
                                           const int16x8_t* d_je, int32x4_t deltas[][WIN_7]) {
    msub_neon(&deltas[0][0], d_is[0], d_js[0]);
    msub_neon(&deltas[0][0], d_is[1], d_js[1]);
    msub_neon(&deltas[0][1], d_is[0], d_js[2]);
    msub_neon(&deltas[0][1], d_is[1], d_js[3]);
    msub_neon(&deltas[0][2], d_is[0], d_js[4]);
    msub_neon(&deltas[0][2], d_is[1], d_js[5]);
    msub_neon(&deltas[0][3], d_is[0], d_js[6]);
    msub_neon(&deltas[0][3], d_is[1], d_js[7]);
    msub_neon(&deltas[0][4], d_is[0], d_js[8]);
    msub_neon(&deltas[0][4], d_is[1], d_js[9]);
    msub_neon(&deltas[0][5], d_is[0], d_js[10]);
    msub_neon(&deltas[0][5], d_is[1], d_js[11]);

    msub_neon(&deltas[1][0], d_is[2], d_js[0]);
    msub_neon(&deltas[1][0], d_is[3], d_js[1]);
    msub_neon(&deltas[1][1], d_is[2], d_js[2]);
    msub_neon(&deltas[1][1], d_is[3], d_js[3]);
    msub_neon(&deltas[1][2], d_is[2], d_js[4]);
    msub_neon(&deltas[1][2], d_is[3], d_js[5]);
    msub_neon(&deltas[1][3], d_is[2], d_js[6]);
    msub_neon(&deltas[1][3], d_is[3], d_js[7]);
    msub_neon(&deltas[1][4], d_is[2], d_js[8]);
    msub_neon(&deltas[1][4], d_is[3], d_js[9]);
    msub_neon(&deltas[1][5], d_is[2], d_js[10]);
    msub_neon(&deltas[1][5], d_is[3], d_js[11]);

    msub_neon(&deltas[2][0], d_is[4], d_js[0]);
    msub_neon(&deltas[2][0], d_is[5], d_js[1]);
    msub_neon(&deltas[2][1], d_is[4], d_js[2]);
    msub_neon(&deltas[2][1], d_is[5], d_js[3]);
    msub_neon(&deltas[2][2], d_is[4], d_js[4]);
    msub_neon(&deltas[2][2], d_is[5], d_js[5]);
    msub_neon(&deltas[2][3], d_is[4], d_js[6]);
    msub_neon(&deltas[2][3], d_is[5], d_js[7]);
    msub_neon(&deltas[2][4], d_is[4], d_js[8]);
    msub_neon(&deltas[2][4], d_is[5], d_js[9]);
    msub_neon(&deltas[2][5], d_is[4], d_js[10]);
    msub_neon(&deltas[2][5], d_is[5], d_js[11]);

    msub_neon(&deltas[3][0], d_is[6], d_js[0]);
    msub_neon(&deltas[3][0], d_is[7], d_js[1]);
    msub_neon(&deltas[3][1], d_is[6], d_js[2]);
    msub_neon(&deltas[3][1], d_is[7], d_js[3]);
    msub_neon(&deltas[3][2], d_is[6], d_js[4]);
    msub_neon(&deltas[3][2], d_is[7], d_js[5]);
    msub_neon(&deltas[3][3], d_is[6], d_js[6]);
    msub_neon(&deltas[3][3], d_is[7], d_js[7]);
    msub_neon(&deltas[3][4], d_is[6], d_js[8]);
    msub_neon(&deltas[3][4], d_is[7], d_js[9]);
    msub_neon(&deltas[3][5], d_is[6], d_js[10]);
    msub_neon(&deltas[3][5], d_is[7], d_js[11]);

    msub_neon(&deltas[4][0], d_is[8], d_js[0]);
    msub_neon(&deltas[4][0], d_is[9], d_js[1]);
    msub_neon(&deltas[4][1], d_is[8], d_js[2]);
    msub_neon(&deltas[4][1], d_is[9], d_js[3]);
    msub_neon(&deltas[4][2], d_is[8], d_js[4]);
    msub_neon(&deltas[4][2], d_is[9], d_js[5]);
    msub_neon(&deltas[4][3], d_is[8], d_js[6]);
    msub_neon(&deltas[4][3], d_is[9], d_js[7]);
    msub_neon(&deltas[4][4], d_is[8], d_js[8]);
    msub_neon(&deltas[4][4], d_is[9], d_js[9]);
    msub_neon(&deltas[4][5], d_is[8], d_js[10]);
    msub_neon(&deltas[4][5], d_is[9], d_js[11]);

    msub_neon(&deltas[5][0], d_is[10], d_js[0]);
    msub_neon(&deltas[5][0], d_is[11], d_js[1]);
    msub_neon(&deltas[5][1], d_is[10], d_js[2]);
    msub_neon(&deltas[5][1], d_is[11], d_js[3]);
    msub_neon(&deltas[5][2], d_is[10], d_js[4]);
    msub_neon(&deltas[5][2], d_is[11], d_js[5]);
    msub_neon(&deltas[5][3], d_is[10], d_js[6]);
    msub_neon(&deltas[5][3], d_is[11], d_js[7]);
    msub_neon(&deltas[5][4], d_is[10], d_js[8]);
    msub_neon(&deltas[5][4], d_is[11], d_js[9]);
    msub_neon(&deltas[5][5], d_is[10], d_js[10]);
    msub_neon(&deltas[5][5], d_is[11], d_js[11]);

    madd_neon(&deltas[0][0], d_ie[0], d_je[0]);
    madd_neon(&deltas[0][0], d_ie[1], d_je[1]);
    madd_neon(&deltas[0][1], d_ie[0], d_je[2]);
    madd_neon(&deltas[0][1], d_ie[1], d_je[3]);
    madd_neon(&deltas[0][2], d_ie[0], d_je[4]);
    madd_neon(&deltas[0][2], d_ie[1], d_je[5]);
    madd_neon(&deltas[0][3], d_ie[0], d_je[6]);
    madd_neon(&deltas[0][3], d_ie[1], d_je[7]);
    madd_neon(&deltas[0][4], d_ie[0], d_je[8]);
    madd_neon(&deltas[0][4], d_ie[1], d_je[9]);
    madd_neon(&deltas[0][5], d_ie[0], d_je[10]);
    madd_neon(&deltas[0][5], d_ie[1], d_je[11]);

    madd_neon(&deltas[1][0], d_ie[2], d_je[0]);
    madd_neon(&deltas[1][0], d_ie[3], d_je[1]);
    madd_neon(&deltas[1][1], d_ie[2], d_je[2]);
    madd_neon(&deltas[1][1], d_ie[3], d_je[3]);
    madd_neon(&deltas[1][2], d_ie[2], d_je[4]);
    madd_neon(&deltas[1][2], d_ie[3], d_je[5]);
    madd_neon(&deltas[1][3], d_ie[2], d_je[6]);
    madd_neon(&deltas[1][3], d_ie[3], d_je[7]);
    madd_neon(&deltas[1][4], d_ie[2], d_je[8]);
    madd_neon(&deltas[1][4], d_ie[3], d_je[9]);
    madd_neon(&deltas[1][5], d_ie[2], d_je[10]);
    madd_neon(&deltas[1][5], d_ie[3], d_je[11]);

    madd_neon(&deltas[2][0], d_ie[4], d_je[0]);
    madd_neon(&deltas[2][0], d_ie[5], d_je[1]);
    madd_neon(&deltas[2][1], d_ie[4], d_je[2]);
    madd_neon(&deltas[2][1], d_ie[5], d_je[3]);
    madd_neon(&deltas[2][2], d_ie[4], d_je[4]);
    madd_neon(&deltas[2][2], d_ie[5], d_je[5]);
    madd_neon(&deltas[2][3], d_ie[4], d_je[6]);
    madd_neon(&deltas[2][3], d_ie[5], d_je[7]);
    madd_neon(&deltas[2][4], d_ie[4], d_je[8]);
    madd_neon(&deltas[2][4], d_ie[5], d_je[9]);
    madd_neon(&deltas[2][5], d_ie[4], d_je[10]);
    madd_neon(&deltas[2][5], d_ie[5], d_je[11]);

    madd_neon(&deltas[3][0], d_ie[6], d_je[0]);
    madd_neon(&deltas[3][0], d_ie[7], d_je[1]);
    madd_neon(&deltas[3][1], d_ie[6], d_je[2]);
    madd_neon(&deltas[3][1], d_ie[7], d_je[3]);
    madd_neon(&deltas[3][2], d_ie[6], d_je[4]);
    madd_neon(&deltas[3][2], d_ie[7], d_je[5]);
    madd_neon(&deltas[3][3], d_ie[6], d_je[6]);
    madd_neon(&deltas[3][3], d_ie[7], d_je[7]);
    madd_neon(&deltas[3][4], d_ie[6], d_je[8]);
    madd_neon(&deltas[3][4], d_ie[7], d_je[9]);
    madd_neon(&deltas[3][5], d_ie[6], d_je[10]);
    madd_neon(&deltas[3][5], d_ie[7], d_je[11]);

    madd_neon(&deltas[4][0], d_ie[8], d_je[0]);
    madd_neon(&deltas[4][0], d_ie[9], d_je[1]);
    madd_neon(&deltas[4][1], d_ie[8], d_je[2]);
    madd_neon(&deltas[4][1], d_ie[9], d_je[3]);
    madd_neon(&deltas[4][2], d_ie[8], d_je[4]);
    madd_neon(&deltas[4][2], d_ie[9], d_je[5]);
    madd_neon(&deltas[4][3], d_ie[8], d_je[6]);
    madd_neon(&deltas[4][3], d_ie[9], d_je[7]);
    madd_neon(&deltas[4][4], d_ie[8], d_je[8]);
    madd_neon(&deltas[4][4], d_ie[9], d_je[9]);
    madd_neon(&deltas[4][5], d_ie[8], d_je[10]);
    madd_neon(&deltas[4][5], d_ie[9], d_je[11]);

    madd_neon(&deltas[5][0], d_ie[10], d_je[0]);
    madd_neon(&deltas[5][0], d_ie[11], d_je[1]);
    madd_neon(&deltas[5][1], d_ie[10], d_je[2]);
    madd_neon(&deltas[5][1], d_ie[11], d_je[3]);
    madd_neon(&deltas[5][2], d_ie[10], d_je[4]);
    madd_neon(&deltas[5][2], d_ie[11], d_je[5]);
    madd_neon(&deltas[5][3], d_ie[10], d_je[6]);
    madd_neon(&deltas[5][3], d_ie[11], d_je[7]);
    madd_neon(&deltas[5][4], d_ie[10], d_je[8]);
    madd_neon(&deltas[5][4], d_ie[11], d_je[9]);
    madd_neon(&deltas[5][5], d_ie[10], d_je[10]);
    madd_neon(&deltas[5][5], d_ie[11], d_je[11]);
}

static inline void hadd_update_6_stats_neon(const int64_t* const src, const int32x4_t* deltas, int64_t* const dst) {
    int32x4_t delta0123 = horizontal_add_4d_s32x4(&deltas[0]);
    int32x4_t delta2345 = horizontal_add_4d_s32x4(&deltas[2]);

    int64x2_t src0 = vld1q_s64(src);
    int64x2_t src1 = vld1q_s64(src + 2);
    int64x2_t src2 = vld1q_s64(src + 4);

    vst1q_s64(dst, vaddw_s32(src0, vget_low_s32(delta0123)));
    vst1q_s64(dst + 2, vaddw_s32(src1, vget_high_s32(delta0123)));
    vst1q_s64(dst + 4, vaddw_s32(src2, vget_high_s32(delta2345)));
}

static inline void update_8_stats_neon(const int64_t* const src, const int32x4_t* delta, int64_t* const dst) {
    update_4_stats_neon(src + 0, delta[0], dst + 0);
    update_4_stats_neon(src + 4, delta[1], dst + 4);
}

static inline void load_square_win7_neon(const int16_t* const di, const int16_t* const dj, const int32_t d_stride,
                                         const int32_t height, int16x8_t* d_is, int16x8_t* d_ie, int16x8_t* d_js,
                                         int16x8_t* d_je) {
    load_s16_8x6(di + 0, d_stride, &d_is[0], &d_is[2], &d_is[4], &d_is[6], &d_is[8], &d_is[10]);
    load_s16_8x6(di + 8, d_stride, &d_is[1], &d_is[3], &d_is[5], &d_is[7], &d_is[9], &d_is[11]);
    load_s16_8x6(dj + 0, d_stride, &d_js[0], &d_js[2], &d_js[4], &d_js[6], &d_js[8], &d_js[10]);
    load_s16_8x6(dj + 8, d_stride, &d_js[1], &d_js[3], &d_js[5], &d_js[7], &d_js[9], &d_js[11]);

    load_s16_8x6(di + height * d_stride + 0, d_stride, &d_ie[0], &d_ie[2], &d_ie[4], &d_ie[6], &d_ie[8], &d_ie[10]);
    load_s16_8x6(di + height * d_stride + 8, d_stride, &d_ie[1], &d_ie[3], &d_ie[5], &d_ie[7], &d_ie[9], &d_ie[11]);
    load_s16_8x6(dj + height * d_stride + 0, d_stride, &d_je[0], &d_je[2], &d_je[4], &d_je[6], &d_je[8], &d_je[10]);
    load_s16_8x6(dj + height * d_stride + 8, d_stride, &d_je[1], &d_je[3], &d_je[5], &d_je[7], &d_je[9], &d_je[11]);
}

static inline void load_triangle_win7_neon(const int16_t* const di, const int32_t d_stride, const int32_t height,
                                           int16x8_t* d_is, int16x8_t* d_ie) {
    load_s16_8x6(di, d_stride, &d_is[0], &d_is[2], &d_is[4], &d_is[6], &d_is[8], &d_is[10]);
    load_s16_8x6(di + 8, d_stride, &d_is[1], &d_is[3], &d_is[5], &d_is[7], &d_is[9], &d_is[11]);

    load_s16_8x6(di + height * d_stride, d_stride, &d_ie[0], &d_ie[2], &d_ie[4], &d_ie[6], &d_ie[8], &d_ie[10]);
    load_s16_8x6(di + height * d_stride + 8, d_stride, &d_ie[1], &d_ie[3], &d_ie[5], &d_ie[7], &d_ie[9], &d_ie[11]);
}

static inline void stats_left_win7_neon(const int16x8_t src[2], const int16_t* d, const int32_t d_stride,
                                        int32x4_t* sum) {
    int16x8_t dgds[WIN_7];

    load_s16_8x6(d + d_stride + 0, d_stride, &dgds[0], &dgds[2], &dgds[4], &dgds[6], &dgds[8], &dgds[10]);
    load_s16_8x6(d + d_stride + 8, d_stride, &dgds[1], &dgds[3], &dgds[5], &dgds[7], &dgds[9], &dgds[11]);

    madd_neon(&sum[0], src[0], dgds[0]);
    madd_neon(&sum[0], src[1], dgds[1]);
    madd_neon(&sum[1], src[0], dgds[2]);
    madd_neon(&sum[1], src[1], dgds[3]);
    madd_neon(&sum[2], src[0], dgds[4]);
    madd_neon(&sum[2], src[1], dgds[5]);
    madd_neon(&sum[3], src[0], dgds[6]);
    madd_neon(&sum[3], src[1], dgds[7]);
    madd_neon(&sum[4], src[0], dgds[8]);
    madd_neon(&sum[4], src[1], dgds[9]);
    madd_neon(&sum[5], src[0], dgds[10]);
    madd_neon(&sum[5], src[1], dgds[11]);
}

static inline void step3_win7_neon(const int16_t* d, const int32_t d_stride, const int32_t width, const int32_t height,
                                   int16x8_t* ds, int32x4_t* deltas) {
    int32_t y = height;
    do {
        ds[12] = vld1q_s16(d);
        ds[13] = vld1q_s16(d + width);

        compute_delta_step3_win7(&deltas[0], ds[0], ds[1], ds[0], ds[1]);
        compute_delta_step3_win7(&deltas[2], ds[0], ds[1], ds[2], ds[3]);
        compute_delta_step3_win7(&deltas[4], ds[0], ds[1], ds[4], ds[5]);
        compute_delta_step3_win7(&deltas[6], ds[0], ds[1], ds[6], ds[7]);
        compute_delta_step3_win7(&deltas[8], ds[0], ds[1], ds[8], ds[9]);
        compute_delta_step3_win7(&deltas[10], ds[0], ds[1], ds[10], ds[11]);
        compute_delta_step3_win7(&deltas[12], ds[0], ds[1], ds[12], ds[13]);

        ds[0]  = ds[2];
        ds[1]  = ds[3];
        ds[2]  = ds[4];
        ds[3]  = ds[5];
        ds[4]  = ds[6];
        ds[5]  = ds[7];
        ds[6]  = ds[8];
        ds[7]  = ds[9];
        ds[8]  = ds[10];
        ds[9]  = ds[11];
        ds[10] = ds[12];
        ds[11] = ds[13];

        d += d_stride;
    } while (--y);
}

static inline void derive_triangle_win7_neon(const int16x8_t* d_is, const int16x8_t* d_ie, int32x4_t* deltas) {
    msub_neon(&deltas[0], d_is[0], d_is[0]);
    msub_neon(&deltas[0], d_is[1], d_is[1]);
    msub_neon(&deltas[1], d_is[0], d_is[2]);
    msub_neon(&deltas[1], d_is[1], d_is[3]);
    msub_neon(&deltas[2], d_is[0], d_is[4]);
    msub_neon(&deltas[2], d_is[1], d_is[5]);
    msub_neon(&deltas[3], d_is[0], d_is[6]);
    msub_neon(&deltas[3], d_is[1], d_is[7]);
    msub_neon(&deltas[4], d_is[0], d_is[8]);
    msub_neon(&deltas[4], d_is[1], d_is[9]);
    msub_neon(&deltas[5], d_is[0], d_is[10]);
    msub_neon(&deltas[5], d_is[1], d_is[11]);

    msub_neon(&deltas[6], d_is[2], d_is[2]);
    msub_neon(&deltas[6], d_is[3], d_is[3]);
    msub_neon(&deltas[7], d_is[2], d_is[4]);
    msub_neon(&deltas[7], d_is[3], d_is[5]);
    msub_neon(&deltas[8], d_is[2], d_is[6]);
    msub_neon(&deltas[8], d_is[3], d_is[7]);
    msub_neon(&deltas[9], d_is[2], d_is[8]);
    msub_neon(&deltas[9], d_is[3], d_is[9]);
    msub_neon(&deltas[10], d_is[2], d_is[10]);
    msub_neon(&deltas[10], d_is[3], d_is[11]);

    msub_neon(&deltas[11], d_is[4], d_is[4]);
    msub_neon(&deltas[11], d_is[5], d_is[5]);
    msub_neon(&deltas[12], d_is[4], d_is[6]);
    msub_neon(&deltas[12], d_is[5], d_is[7]);
    msub_neon(&deltas[13], d_is[4], d_is[8]);
    msub_neon(&deltas[13], d_is[5], d_is[9]);
    msub_neon(&deltas[14], d_is[4], d_is[10]);
    msub_neon(&deltas[14], d_is[5], d_is[11]);

    msub_neon(&deltas[15], d_is[6], d_is[6]);
    msub_neon(&deltas[15], d_is[7], d_is[7]);
    msub_neon(&deltas[16], d_is[6], d_is[8]);
    msub_neon(&deltas[16], d_is[7], d_is[9]);
    msub_neon(&deltas[17], d_is[6], d_is[10]);
    msub_neon(&deltas[17], d_is[7], d_is[11]);

    msub_neon(&deltas[18], d_is[8], d_is[8]);
    msub_neon(&deltas[18], d_is[9], d_is[9]);
    msub_neon(&deltas[19], d_is[8], d_is[10]);
    msub_neon(&deltas[19], d_is[9], d_is[11]);

    msub_neon(&deltas[20], d_is[10], d_is[10]);
    msub_neon(&deltas[20], d_is[11], d_is[11]);

    madd_neon(&deltas[0], d_ie[0], d_ie[0]);
    madd_neon(&deltas[0], d_ie[1], d_ie[1]);
    madd_neon(&deltas[1], d_ie[0], d_ie[2]);
    madd_neon(&deltas[1], d_ie[1], d_ie[3]);
    madd_neon(&deltas[2], d_ie[0], d_ie[4]);
    madd_neon(&deltas[2], d_ie[1], d_ie[5]);
    madd_neon(&deltas[3], d_ie[0], d_ie[6]);
    madd_neon(&deltas[3], d_ie[1], d_ie[7]);
    madd_neon(&deltas[4], d_ie[0], d_ie[8]);
    madd_neon(&deltas[4], d_ie[1], d_ie[9]);
    madd_neon(&deltas[5], d_ie[0], d_ie[10]);
    madd_neon(&deltas[5], d_ie[1], d_ie[11]);

    madd_neon(&deltas[6], d_ie[2], d_ie[2]);
    madd_neon(&deltas[6], d_ie[3], d_ie[3]);
    madd_neon(&deltas[7], d_ie[2], d_ie[4]);
    madd_neon(&deltas[7], d_ie[3], d_ie[5]);
    madd_neon(&deltas[8], d_ie[2], d_ie[6]);
    madd_neon(&deltas[8], d_ie[3], d_ie[7]);
    madd_neon(&deltas[9], d_ie[2], d_ie[8]);
    madd_neon(&deltas[9], d_ie[3], d_ie[9]);
    madd_neon(&deltas[10], d_ie[2], d_ie[10]);
    madd_neon(&deltas[10], d_ie[3], d_ie[11]);

    madd_neon(&deltas[11], d_ie[4], d_ie[4]);
    madd_neon(&deltas[11], d_ie[5], d_ie[5]);
    madd_neon(&deltas[12], d_ie[4], d_ie[6]);
    madd_neon(&deltas[12], d_ie[5], d_ie[7]);
    madd_neon(&deltas[13], d_ie[4], d_ie[8]);
    madd_neon(&deltas[13], d_ie[5], d_ie[9]);
    madd_neon(&deltas[14], d_ie[4], d_ie[10]);
    madd_neon(&deltas[14], d_ie[5], d_ie[11]);

    madd_neon(&deltas[15], d_ie[6], d_ie[6]);
    madd_neon(&deltas[15], d_ie[7], d_ie[7]);
    madd_neon(&deltas[16], d_ie[6], d_ie[8]);
    madd_neon(&deltas[16], d_ie[7], d_ie[9]);
    madd_neon(&deltas[17], d_ie[6], d_ie[10]);
    madd_neon(&deltas[17], d_ie[7], d_ie[11]);

    madd_neon(&deltas[18], d_ie[8], d_ie[8]);
    madd_neon(&deltas[18], d_ie[9], d_ie[9]);
    madd_neon(&deltas[19], d_ie[8], d_ie[10]);
    madd_neon(&deltas[19], d_ie[9], d_ie[11]);

    madd_neon(&deltas[20], d_ie[10], d_ie[10]);
    madd_neon(&deltas[20], d_ie[11], d_ie[11]);
}

static inline void diagonal_copy_stats_neon(const int32_t wiener_win2, int64_t* const H) {
    for (int32_t i = 0; i < wiener_win2 - 1; i += 4) {
        int64x2_t in[8], out[8];

        in[0] = vld1q_s64(H + (i + 0) * wiener_win2 + i + 1);
        in[1] = vld1q_s64(H + (i + 0) * wiener_win2 + i + 3);
        in[2] = vld1q_s64(H + (i + 1) * wiener_win2 + i + 1);
        in[3] = vld1q_s64(H + (i + 1) * wiener_win2 + i + 3);
        in[4] = vld1q_s64(H + (i + 2) * wiener_win2 + i + 1);
        in[5] = vld1q_s64(H + (i + 2) * wiener_win2 + i + 3);
        in[6] = vld1q_s64(H + (i + 3) * wiener_win2 + i + 1);
        in[7] = vld1q_s64(H + (i + 3) * wiener_win2 + i + 3);

        transpose_s64_4x4_neon(in, out);

        vst1_s64(H + (i + 1) * wiener_win2 + i + 0, vget_low_s64(out[0]));
        vst1q_s64(H + (i + 2) * wiener_win2 + i + 0, out[2]);
        vst1q_s64(H + (i + 3) * wiener_win2 + i + 0, out[4]);
        vst1q_s64(H + (i + 3) * wiener_win2 + i + 2, out[5]);
        vst1q_s64(H + (i + 4) * wiener_win2 + i + 0, out[6]);
        vst1q_s64(H + (i + 4) * wiener_win2 + i + 2, out[7]);

        for (int32_t j = i + 5; j < wiener_win2; j += 4) {
            in[0] = vld1q_s64(H + (i + 0) * wiener_win2 + j + 0);
            in[1] = vld1q_s64(H + (i + 0) * wiener_win2 + j + 2);
            in[2] = vld1q_s64(H + (i + 1) * wiener_win2 + j + 0);
            in[3] = vld1q_s64(H + (i + 1) * wiener_win2 + j + 2);
            in[4] = vld1q_s64(H + (i + 2) * wiener_win2 + j + 0);
            in[5] = vld1q_s64(H + (i + 2) * wiener_win2 + j + 2);
            in[6] = vld1q_s64(H + (i + 3) * wiener_win2 + j + 0);
            in[7] = vld1q_s64(H + (i + 3) * wiener_win2 + j + 2);

            transpose_s64_4x4_neon(in, out);

            vst1q_s64(H + (j + 0) * wiener_win2 + i + 0, out[0]);
            vst1q_s64(H + (j + 0) * wiener_win2 + i + 2, out[1]);
            vst1q_s64(H + (j + 1) * wiener_win2 + i + 0, out[2]);
            vst1q_s64(H + (j + 1) * wiener_win2 + i + 2, out[3]);
            vst1q_s64(H + (j + 2) * wiener_win2 + i + 0, out[4]);
            vst1q_s64(H + (j + 2) * wiener_win2 + i + 2, out[5]);
            vst1q_s64(H + (j + 3) * wiener_win2 + i + 0, out[6]);
            vst1q_s64(H + (j + 3) * wiener_win2 + i + 2, out[7]);
        }
    }
}

static inline int64x2_t div4_neon(const int64x2_t src) {
    uint64x2_t sign = vcltzq_s64(src);
    int64x2_t  dst  = vabsq_s64(src);
    // divide by 4
    dst = vshrq_n_s64(dst, 2);
    // re-apply sign
    return vbslq_s64(sign, vnegq_s64(dst), dst);
}

static inline void div4_4x4_neon(const int32_t wiener_win2, int64_t* const H, int64x2_t out[8]) {
    out[0] = vld1q_s64(H + 0 * wiener_win2 + 0);
    out[1] = vld1q_s64(H + 0 * wiener_win2 + 2);
    out[2] = vld1q_s64(H + 1 * wiener_win2 + 0);
    out[3] = vld1q_s64(H + 1 * wiener_win2 + 2);
    out[4] = vld1q_s64(H + 2 * wiener_win2 + 0);
    out[5] = vld1q_s64(H + 2 * wiener_win2 + 2);
    out[6] = vld1q_s64(H + 3 * wiener_win2 + 0);
    out[7] = vld1q_s64(H + 3 * wiener_win2 + 2);

    out[0] = div4_neon(out[0]);
    out[1] = div4_neon(out[1]);
    out[2] = div4_neon(out[2]);
    out[3] = div4_neon(out[3]);
    out[4] = div4_neon(out[4]);
    out[5] = div4_neon(out[5]);
    out[6] = div4_neon(out[6]);
    out[7] = div4_neon(out[7]);

    vst1q_s64(H + 0 * wiener_win2 + 0, out[0]);
    vst1q_s64(H + 0 * wiener_win2 + 2, out[1]);
    vst1q_s64(H + 1 * wiener_win2 + 0, out[2]);
    vst1q_s64(H + 1 * wiener_win2 + 2, out[3]);
    vst1q_s64(H + 2 * wiener_win2 + 0, out[4]);
    vst1q_s64(H + 2 * wiener_win2 + 2, out[5]);
    vst1q_s64(H + 3 * wiener_win2 + 0, out[6]);
    vst1q_s64(H + 3 * wiener_win2 + 2, out[7]);
}

static inline void div4_diagonal_copy_stats_neon(const int32_t wiener_win2, int64_t* const H) {
    for (int32_t i = 0; i < wiener_win2 - 1; i += 4) {
        int64x2_t in[8], out[8];

        div4_4x4_neon(wiener_win2, H + i * wiener_win2 + i + 1, in);
        transpose_s64_4x4_neon(in, out);

        vst1_s64(H + (i + 1) * wiener_win2 + i + 0, vget_low_s64(out[0]));
        vst1q_s64(H + (i + 2) * wiener_win2 + i + 0, out[2]);
        vst1q_s64(H + (i + 3) * wiener_win2 + i + 0, out[4]);
        vst1q_s64(H + (i + 3) * wiener_win2 + i + 2, out[5]);
        vst1q_s64(H + (i + 4) * wiener_win2 + i + 0, out[6]);
        vst1q_s64(H + (i + 4) * wiener_win2 + i + 2, out[7]);

        for (int32_t j = i + 5; j < wiener_win2; j += 4) {
            div4_4x4_neon(wiener_win2, H + i * wiener_win2 + j, in);
            transpose_s64_4x4_neon(in, out);

            vst1q_s64(H + (j + 0) * wiener_win2 + i + 0, out[0]);
            vst1q_s64(H + (j + 0) * wiener_win2 + i + 2, out[1]);
            vst1q_s64(H + (j + 1) * wiener_win2 + i + 0, out[2]);
            vst1q_s64(H + (j + 1) * wiener_win2 + i + 2, out[3]);
            vst1q_s64(H + (j + 2) * wiener_win2 + i + 0, out[4]);
            vst1q_s64(H + (j + 2) * wiener_win2 + i + 2, out[5]);
            vst1q_s64(H + (j + 3) * wiener_win2 + i + 0, out[6]);
            vst1q_s64(H + (j + 3) * wiener_win2 + i + 2, out[7]);
        }
    }
}

#endif // PICKRST_NEON_H
