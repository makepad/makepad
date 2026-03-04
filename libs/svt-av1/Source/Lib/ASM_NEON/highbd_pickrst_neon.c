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

#include <arm_neon.h>

#include "aom_dsp_rtcd.h"
#include "definitions.h"
#include "mem_neon.h"
#include "pickrst_neon.h"
#include "restoration.h"
#include "restoration_pick.h"
#include "sum_neon.h"
#include "transpose_neon.h"
#include "utility.h"

#if CONFIG_ENABLE_HIGH_BIT_DEPTH
static inline void compute_stats_win5_highbd_neon(const int16_t* const d, const int32_t d_stride,
                                                  const int16_t* const s, const int32_t s_stride, const int32_t width,
                                                  const int32_t height, int64_t* const M, int64_t* const H,
                                                  EbBitDepth bit_depth) {
    const int32_t     wiener_win  = WIENER_WIN_CHROMA;
    const int32_t     wiener_win2 = wiener_win * wiener_win;
    const int32_t     w16         = width & ~15;
    const int32_t     h8          = height & ~7;
    const int16x8x2_t mask        = vld1q_s16_x2(&(mask_16bit[16]) - width % 16);
    int32_t           i, j, x, y;

    if (bit_depth == EB_EIGHT_BIT) {
        // Step 1: Calculate the top edge of the whole matrix, i.e., the top
        // edge of each triangle and square on the top row.
        j = 0;
        do {
            const int16_t* s_t                      = s;
            const int16_t* d_t                      = d;
            int32x4_t      sum_m[WIENER_WIN_CHROMA] = {vdupq_n_s32(0)};
            int32x4_t      sum_h[WIENER_WIN_CHROMA] = {vdupq_n_s32(0)};
            int16x8_t      src[2], dgd[2];

            y = height;
            do {
                x = 0;
                while (x < w16) {
                    src[0] = vld1q_s16(s_t + x + 0);
                    src[1] = vld1q_s16(s_t + x + 8);
                    dgd[0] = vld1q_s16(d_t + x + 0);
                    dgd[1] = vld1q_s16(d_t + x + 8);
                    stats_top_win5_neon(src, dgd, d_t + j + x, d_stride, sum_m, sum_h);
                    x += 16;
                }

                if (w16 != width) {
                    src[0] = vld1q_s16(s_t + w16 + 0);
                    src[1] = vld1q_s16(s_t + w16 + 8);
                    dgd[0] = vld1q_s16(d_t + w16 + 0);
                    dgd[1] = vld1q_s16(d_t + w16 + 8);
                    src[0] = vandq_s16(src[0], mask.val[0]);
                    src[1] = vandq_s16(src[1], mask.val[1]);
                    dgd[0] = vandq_s16(dgd[0], mask.val[0]);
                    dgd[1] = vandq_s16(dgd[1], mask.val[1]);
                    stats_top_win5_neon(src, dgd, d_t + j + w16, d_stride, sum_m, sum_h);
                }

                s_t += s_stride;
                d_t += d_stride;
            } while (--y);

            int32x4_t sum_m_s64 = horizontal_add_4d_s32x4(sum_m);
            vst1q_s64(&M[wiener_win * j + 0], vmovl_s32(vget_low_s32(sum_m_s64)));
            vst1q_s64(&M[wiener_win * j + 2], vmovl_s32(vget_high_s32(sum_m_s64)));
            M[wiener_win * j + 4] = vaddlvq_s32(sum_m[4]);

            int32x4_t sum_h_s64 = horizontal_add_4d_s32x4(sum_h);
            vst1q_s64(&H[wiener_win * j + 0], vmovl_s32(vget_low_s32(sum_h_s64)));
            vst1q_s64(&H[wiener_win * j + 2], vmovl_s32(vget_high_s32(sum_h_s64)));
            H[wiener_win * j + 4] = vaddlvq_s32(sum_h[4]);
        } while (++j < wiener_win);

        // Step 2: Calculate the left edge of each square on the top row.
        j = 1;
        do {
            const int16_t* d_t                          = d;
            int32x4_t      sum_h[WIENER_WIN_CHROMA - 1] = {vdupq_n_s32(0)};
            int16x8_t      dgd[2];

            y = height;
            do {
                x = 0;
                while (x < w16) {
                    dgd[0] = vld1q_s16(d_t + j + x + 0);
                    dgd[1] = vld1q_s16(d_t + j + x + 8);
                    stats_left_win5_neon(dgd, d_t + x, d_stride, sum_h);
                    x += 16;
                }

                if (w16 != width) {
                    dgd[0] = vld1q_s16(d_t + j + x + 0);
                    dgd[1] = vld1q_s16(d_t + j + x + 8);
                    dgd[0] = vandq_s16(dgd[0], mask.val[0]);
                    dgd[1] = vandq_s16(dgd[1], mask.val[1]);
                    stats_left_win5_neon(dgd, d_t + x, d_stride, sum_h);
                }

                d_t += d_stride;
            } while (--y);

            sum_h[0]             = vpaddq_s32(sum_h[0], sum_h[1]);
            sum_h[2]             = vpaddq_s32(sum_h[2], sum_h[3]);
            int64x2_t sum_h0_s64 = vpaddlq_s32(sum_h[0]);
            int64x2_t sum_h1_s64 = vpaddlq_s32(sum_h[2]);
            vst1_s64(&H[1 * wiener_win2 + j * wiener_win], vget_low_s64(sum_h0_s64));
            vst1_s64(&H[2 * wiener_win2 + j * wiener_win], vget_high_s64(sum_h0_s64));
            vst1_s64(&H[3 * wiener_win2 + j * wiener_win], vget_low_s64(sum_h1_s64));
            vst1_s64(&H[4 * wiener_win2 + j * wiener_win], vget_high_s64(sum_h1_s64));

        } while (++j < wiener_win);
    } else {
        const int32_t num_bit_left = 32 - 1 /* sign */ - 2 * bit_depth /* energy */ + 2 /* SIMD */;
        const int32_t h_allowed    = (1 << num_bit_left) / (w16 + ((w16 != width) ? 16 : 0));

        // Step 1: Calculate the top edge of the whole matrix, i.e., the top
        // edge of each triangle and square on the top row.
        j = 0;
        do {
            const int16_t* s_t                      = s;
            const int16_t* d_t                      = d;
            int32_t        height_t                 = 0;
            int64x2_t      sum_m[WIENER_WIN_CHROMA] = {vdupq_n_s64(0)};
            int64x2_t      sum_h[WIENER_WIN_CHROMA] = {vdupq_n_s64(0)};
            int16x8_t      src[2], dgd[2];

            do {
                const int32_t h_t = ((height - height_t) < h_allowed) ? (height - height_t) : h_allowed;
                int32x4_t     row_m[WIENER_WIN_CHROMA] = {vdupq_n_s32(0)};
                int32x4_t     row_h[WIENER_WIN_CHROMA] = {vdupq_n_s32(0)};

                y = h_t;
                do {
                    x = 0;
                    while (x < w16) {
                        src[0] = vld1q_s16(s_t + x + 0);
                        src[1] = vld1q_s16(s_t + x + 8);
                        dgd[0] = vld1q_s16(d_t + x + 0);
                        dgd[1] = vld1q_s16(d_t + x + 8);
                        stats_top_win5_neon(src, dgd, d_t + j + x, d_stride, row_m, row_h);
                        x += 16;
                    }

                    if (w16 != width) {
                        src[0] = vld1q_s16(s_t + w16 + 0);
                        src[1] = vld1q_s16(s_t + w16 + 8);
                        dgd[0] = vld1q_s16(d_t + w16 + 0);
                        dgd[1] = vld1q_s16(d_t + w16 + 8);
                        src[0] = vandq_s16(src[0], mask.val[0]);
                        src[1] = vandq_s16(src[1], mask.val[1]);
                        dgd[0] = vandq_s16(dgd[0], mask.val[0]);
                        dgd[1] = vandq_s16(dgd[1], mask.val[1]);
                        stats_top_win5_neon(src, dgd, d_t + j + w16, d_stride, row_m, row_h);
                    }

                    s_t += s_stride;
                    d_t += d_stride;
                } while (--y);

                sum_m[0] = vpadalq_s32(sum_m[0], row_m[0]);
                sum_m[1] = vpadalq_s32(sum_m[1], row_m[1]);
                sum_m[2] = vpadalq_s32(sum_m[2], row_m[2]);
                sum_m[3] = vpadalq_s32(sum_m[3], row_m[3]);
                sum_m[4] = vpadalq_s32(sum_m[4], row_m[4]);
                sum_h[0] = vpadalq_s32(sum_h[0], row_h[0]);
                sum_h[1] = vpadalq_s32(sum_h[1], row_h[1]);
                sum_h[2] = vpadalq_s32(sum_h[2], row_h[2]);
                sum_h[3] = vpadalq_s32(sum_h[3], row_h[3]);
                sum_h[4] = vpadalq_s32(sum_h[4], row_h[4]);

                height_t += h_t;
            } while (height_t < height);

            int64x2_t sum_m0 = vpaddq_s64(sum_m[0], sum_m[1]);
            int64x2_t sum_m2 = vpaddq_s64(sum_m[2], sum_m[3]);
            vst1q_s64(&M[wiener_win * j + 0], sum_m0);
            vst1q_s64(&M[wiener_win * j + 2], sum_m2);
            M[wiener_win * j + 4] = vaddvq_s64(sum_m[4]);

            int64x2_t sum_h0 = vpaddq_s64(sum_h[0], sum_h[1]);
            int64x2_t sum_h2 = vpaddq_s64(sum_h[2], sum_h[3]);
            vst1q_s64(&H[wiener_win * j + 0], sum_h0);
            vst1q_s64(&H[wiener_win * j + 2], sum_h2);
            H[wiener_win * j + 4] = vaddvq_s64(sum_h[4]);
        } while (++j < wiener_win);

        // Step 2: Calculate the left edge of each square on the top row.
        j = 1;
        do {
            const int16_t* d_t                          = d;
            int32_t        height_t                     = 0;
            int64x2_t      sum_h[WIENER_WIN_CHROMA - 1] = {vdupq_n_s64(0)};
            int16x8_t      dgd[2];

            do {
                const int32_t h_t = ((height - height_t) < h_allowed) ? (height - height_t) : h_allowed;
                int32x4_t     row_h[WIENER_WIN_CHROMA - 1] = {vdupq_n_s32(0)};

                y = h_t;
                do {
                    x = 0;
                    while (x < w16) {
                        dgd[0] = vld1q_s16(d_t + j + x + 0);
                        dgd[1] = vld1q_s16(d_t + j + x + 8);
                        stats_left_win5_neon(dgd, d_t + x, d_stride, row_h);
                        x += 16;
                    }

                    if (w16 != width) {
                        dgd[0] = vld1q_s16(d_t + j + x + 0);
                        dgd[1] = vld1q_s16(d_t + j + x + 8);
                        dgd[0] = vandq_s16(dgd[0], mask.val[0]);
                        dgd[1] = vandq_s16(dgd[1], mask.val[1]);
                        stats_left_win5_neon(dgd, d_t + x, d_stride, row_h);
                    }

                    d_t += d_stride;
                } while (--y);

                sum_h[0] = vpadalq_s32(sum_h[0], row_h[0]);
                sum_h[1] = vpadalq_s32(sum_h[1], row_h[1]);
                sum_h[2] = vpadalq_s32(sum_h[2], row_h[2]);
                sum_h[3] = vpadalq_s32(sum_h[3], row_h[3]);

                height_t += h_t;
            } while (height_t < height);

            int64x2_t sum_h0 = vpaddq_s64(sum_h[0], sum_h[1]);
            int64x2_t sum_h1 = vpaddq_s64(sum_h[2], sum_h[3]);
            vst1_s64(&H[1 * wiener_win2 + j * wiener_win], vget_low_s64(sum_h0));
            vst1_s64(&H[2 * wiener_win2 + j * wiener_win], vget_high_s64(sum_h0));
            vst1_s64(&H[3 * wiener_win2 + j * wiener_win], vget_low_s64(sum_h1));
            vst1_s64(&H[4 * wiener_win2 + j * wiener_win], vget_high_s64(sum_h1));
        } while (++j < wiener_win);
    }

    // Step 3: Derive the top edge of each triangle along the diagonal. No
    // triangle in top row.
    {
        const int16_t* d_t = d;

        int32x4_t deltas[WIENER_WIN_CHROMA] = {vdupq_n_s32(0)};
        int16x8_t ds[WIENER_WIN_CHROMA + 1];

        ds[0] = load_s16_4x2(d_t + 0 * d_stride, width);
        ds[1] = load_s16_4x2(d_t + 1 * d_stride, width);
        ds[2] = load_s16_4x2(d_t + 2 * d_stride, width);
        ds[3] = load_s16_4x2(d_t + 3 * d_stride, width);

        step3_win5_neon(d_t + 4 * d_stride, d_stride, width, height, ds, deltas);

        transpose_s32_4x4(&deltas[0], &deltas[1], &deltas[2], &deltas[3]);

        update_5_stats_neon(H + 0 * wiener_win * wiener_win2 + 0 * wiener_win,
                            deltas[0],
                            vgetq_lane_s32(deltas[4], 0),
                            H + 1 * wiener_win * wiener_win2 + 1 * wiener_win);

        update_5_stats_neon(H + 1 * wiener_win * wiener_win2 + 1 * wiener_win,
                            deltas[1],
                            vgetq_lane_s32(deltas[4], 1),
                            H + 2 * wiener_win * wiener_win2 + 2 * wiener_win);

        update_5_stats_neon(H + 2 * wiener_win * wiener_win2 + 2 * wiener_win,
                            deltas[2],
                            vgetq_lane_s32(deltas[4], 2),
                            H + 3 * wiener_win * wiener_win2 + 3 * wiener_win);

        update_5_stats_neon(H + 3 * wiener_win * wiener_win2 + 3 * wiener_win,
                            deltas[3],
                            vgetq_lane_s32(deltas[4], 3),
                            H + 4 * wiener_win * wiener_win2 + 4 * wiener_win);
    }

    // Step 4: Derive the top and left edge of each square. No square in top and
    // bottom row.
    {
        y = h8;

        int16x4_t      d_s[12];
        int16x4_t      d_e[12];
        const int16_t* d_t   = d;
        int16x4_t      zeros = vdup_n_s16(0);
        load_s16_4x4(d_t, d_stride, &d_s[0], &d_s[1], &d_s[2], &d_s[3]);
        load_s16_4x4(d_t + width, d_stride, &d_e[0], &d_e[1], &d_e[2], &d_e[3]);
        int32x4_t deltas[6][18] = {{vdupq_n_s32(0)}, {vdupq_n_s32(0)}};

        while (y >= 8) {
            load_s16_4x8(
                d_t + 4 * d_stride, d_stride, &d_s[4], &d_s[5], &d_s[6], &d_s[7], &d_s[8], &d_s[9], &d_s[10], &d_s[11]);
            load_s16_4x8(d_t + width + 4 * d_stride,
                         d_stride,
                         &d_e[4],
                         &d_e[5],
                         &d_e[6],
                         &d_e[7],
                         &d_e[8],
                         &d_e[9],
                         &d_e[10],
                         &d_e[11]);

            int16x8_t s_tr[8], e_tr[8];
            transpose_elems_s16_4x8(
                d_s[0], d_s[1], d_s[2], d_s[3], d_s[4], d_s[5], d_s[6], d_s[7], &s_tr[0], &s_tr[1], &s_tr[2], &s_tr[3]);
            transpose_elems_s16_4x8(
                d_s[8], d_s[9], d_s[10], d_s[11], zeros, zeros, zeros, zeros, &s_tr[4], &s_tr[5], &s_tr[6], &s_tr[7]);

            transpose_elems_s16_4x8(
                d_e[0], d_e[1], d_e[2], d_e[3], d_e[4], d_e[5], d_e[6], d_e[7], &e_tr[0], &e_tr[1], &e_tr[2], &e_tr[3]);
            transpose_elems_s16_4x8(
                d_e[8], d_e[9], d_e[10], d_e[11], zeros, zeros, zeros, zeros, &e_tr[4], &e_tr[5], &e_tr[6], &e_tr[7]);

            int16x8_t start_col0[5], start_col1[5], start_col2[5], start_col3[5];
            start_col0[0] = s_tr[0];
            start_col0[1] = vextq_s16(s_tr[0], s_tr[4], 1);
            start_col0[2] = vextq_s16(s_tr[0], s_tr[4], 2);
            start_col0[3] = vextq_s16(s_tr[0], s_tr[4], 3);
            start_col0[4] = vextq_s16(s_tr[0], s_tr[4], 4);

            start_col1[0] = s_tr[1];
            start_col1[1] = vextq_s16(s_tr[1], s_tr[5], 1);
            start_col1[2] = vextq_s16(s_tr[1], s_tr[5], 2);
            start_col1[3] = vextq_s16(s_tr[1], s_tr[5], 3);
            start_col1[4] = vextq_s16(s_tr[1], s_tr[5], 4);

            start_col2[0] = s_tr[2];
            start_col2[1] = vextq_s16(s_tr[2], s_tr[6], 1);
            start_col2[2] = vextq_s16(s_tr[2], s_tr[6], 2);
            start_col2[3] = vextq_s16(s_tr[2], s_tr[6], 3);
            start_col2[4] = vextq_s16(s_tr[2], s_tr[6], 4);

            start_col3[0] = s_tr[3];
            start_col3[1] = vextq_s16(s_tr[3], s_tr[7], 1);
            start_col3[2] = vextq_s16(s_tr[3], s_tr[7], 2);
            start_col3[3] = vextq_s16(s_tr[3], s_tr[7], 3);
            start_col3[4] = vextq_s16(s_tr[3], s_tr[7], 4);

            // i = 1, j = 2;
            sub_deltas_step4(start_col0, start_col1, deltas[0]);

            // i = 1, j = 3;
            sub_deltas_step4(start_col0, start_col2, deltas[1]);

            // i = 1, j = 4
            sub_deltas_step4(start_col0, start_col3, deltas[2]);

            // i = 2, j =3
            sub_deltas_step4(start_col1, start_col2, deltas[3]);

            // i = 2, j = 4
            sub_deltas_step4(start_col1, start_col3, deltas[4]);

            // i = 3, j = 4
            sub_deltas_step4(start_col2, start_col3, deltas[5]);

            int16x8_t end_col0[5], end_col1[5], end_col2[5], end_col3[5];
            end_col0[0] = e_tr[0];
            end_col0[1] = vextq_s16(e_tr[0], e_tr[4], 1);
            end_col0[2] = vextq_s16(e_tr[0], e_tr[4], 2);
            end_col0[3] = vextq_s16(e_tr[0], e_tr[4], 3);
            end_col0[4] = vextq_s16(e_tr[0], e_tr[4], 4);

            end_col1[0] = e_tr[1];
            end_col1[1] = vextq_s16(e_tr[1], e_tr[5], 1);
            end_col1[2] = vextq_s16(e_tr[1], e_tr[5], 2);
            end_col1[3] = vextq_s16(e_tr[1], e_tr[5], 3);
            end_col1[4] = vextq_s16(e_tr[1], e_tr[5], 4);

            end_col2[0] = e_tr[2];
            end_col2[1] = vextq_s16(e_tr[2], e_tr[6], 1);
            end_col2[2] = vextq_s16(e_tr[2], e_tr[6], 2);
            end_col2[3] = vextq_s16(e_tr[2], e_tr[6], 3);
            end_col2[4] = vextq_s16(e_tr[2], e_tr[6], 4);

            end_col3[0] = e_tr[3];
            end_col3[1] = vextq_s16(e_tr[3], e_tr[7], 1);
            end_col3[2] = vextq_s16(e_tr[3], e_tr[7], 2);
            end_col3[3] = vextq_s16(e_tr[3], e_tr[7], 3);
            end_col3[4] = vextq_s16(e_tr[3], e_tr[7], 4);

            // i = 1, j = 2;
            add_deltas_step4(end_col0, end_col1, deltas[0]);

            // i = 1, j = 3;
            add_deltas_step4(end_col0, end_col2, deltas[1]);

            // i = 1, j = 4
            add_deltas_step4(end_col0, end_col3, deltas[2]);

            // i = 2, j =3
            add_deltas_step4(end_col1, end_col2, deltas[3]);

            // i = 2, j = 4
            add_deltas_step4(end_col1, end_col3, deltas[4]);

            // i = 3, j = 4
            add_deltas_step4(end_col2, end_col3, deltas[5]);

            d_s[0] = d_s[8];
            d_s[1] = d_s[9];
            d_s[2] = d_s[10];
            d_s[3] = d_s[11];
            d_e[0] = d_e[8];
            d_e[1] = d_e[9];
            d_e[2] = d_e[10];
            d_e[3] = d_e[11];

            d_t += 8 * d_stride;
            y -= 8;
        }

        if (h8 != height) {
            const int16x8_t mask_h = vld1q_s16(&mask_16bit[16] - (height % 8));

            load_s16_4x8(
                d_t + 4 * d_stride, d_stride, &d_s[4], &d_s[5], &d_s[6], &d_s[7], &d_s[8], &d_s[9], &d_s[10], &d_s[11]);
            load_s16_4x8(d_t + width + 4 * d_stride,
                         d_stride,
                         &d_e[4],
                         &d_e[5],
                         &d_e[6],
                         &d_e[7],
                         &d_e[8],
                         &d_e[9],
                         &d_e[10],
                         &d_e[11]);
            int16x8_t s_tr[8], e_tr[8];
            transpose_elems_s16_4x8(
                d_s[0], d_s[1], d_s[2], d_s[3], d_s[4], d_s[5], d_s[6], d_s[7], &s_tr[0], &s_tr[1], &s_tr[2], &s_tr[3]);
            transpose_elems_s16_4x8(
                d_s[8], d_s[9], d_s[10], d_s[11], zeros, zeros, zeros, zeros, &s_tr[4], &s_tr[5], &s_tr[6], &s_tr[7]);
            transpose_elems_s16_4x8(
                d_e[0], d_e[1], d_e[2], d_e[3], d_e[4], d_e[5], d_e[6], d_e[7], &e_tr[0], &e_tr[1], &e_tr[2], &e_tr[3]);
            transpose_elems_s16_4x8(
                d_e[8], d_e[9], d_e[10], d_e[11], zeros, zeros, zeros, zeros, &e_tr[4], &e_tr[5], &e_tr[6], &e_tr[7]);

            int16x8_t start_col0[5], start_col1[5], start_col2[5], start_col3[5];
            start_col0[0] = vandq_s16(s_tr[0], mask_h);
            start_col0[1] = vandq_s16(vextq_s16(s_tr[0], s_tr[4], 1), mask_h);
            start_col0[2] = vandq_s16(vextq_s16(s_tr[0], s_tr[4], 2), mask_h);
            start_col0[3] = vandq_s16(vextq_s16(s_tr[0], s_tr[4], 3), mask_h);
            start_col0[4] = vandq_s16(vextq_s16(s_tr[0], s_tr[4], 4), mask_h);

            start_col1[0] = vandq_s16(s_tr[1], mask_h);
            start_col1[1] = vandq_s16(vextq_s16(s_tr[1], s_tr[5], 1), mask_h);
            start_col1[2] = vandq_s16(vextq_s16(s_tr[1], s_tr[5], 2), mask_h);
            start_col1[3] = vandq_s16(vextq_s16(s_tr[1], s_tr[5], 3), mask_h);
            start_col1[4] = vandq_s16(vextq_s16(s_tr[1], s_tr[5], 4), mask_h);

            start_col2[0] = vandq_s16(s_tr[2], mask_h);
            start_col2[1] = vandq_s16(vextq_s16(s_tr[2], s_tr[6], 1), mask_h);
            start_col2[2] = vandq_s16(vextq_s16(s_tr[2], s_tr[6], 2), mask_h);
            start_col2[3] = vandq_s16(vextq_s16(s_tr[2], s_tr[6], 3), mask_h);
            start_col2[4] = vandq_s16(vextq_s16(s_tr[2], s_tr[6], 4), mask_h);

            start_col3[0] = vandq_s16(s_tr[3], mask_h);
            start_col3[1] = vandq_s16(vextq_s16(s_tr[3], s_tr[7], 1), mask_h);
            start_col3[2] = vandq_s16(vextq_s16(s_tr[3], s_tr[7], 2), mask_h);
            start_col3[3] = vandq_s16(vextq_s16(s_tr[3], s_tr[7], 3), mask_h);
            start_col3[4] = vandq_s16(vextq_s16(s_tr[3], s_tr[7], 4), mask_h);

            // i = 1, j = 2;
            sub_deltas_step4(start_col0, start_col1, deltas[0]);

            // i = 1, j = 3;
            sub_deltas_step4(start_col0, start_col2, deltas[1]);

            // i = 1, j = 4
            sub_deltas_step4(start_col0, start_col3, deltas[2]);

            // i = 2, j = 3
            sub_deltas_step4(start_col1, start_col2, deltas[3]);

            // i = 2, j = 4
            sub_deltas_step4(start_col1, start_col3, deltas[4]);

            // i = 3, j = 4
            sub_deltas_step4(start_col2, start_col3, deltas[5]);

            int16x8_t end_col0[5], end_col1[5], end_col2[5], end_col3[5];
            end_col0[0] = vandq_s16(e_tr[0], mask_h);
            end_col0[1] = vandq_s16(vextq_s16(e_tr[0], e_tr[4], 1), mask_h);
            end_col0[2] = vandq_s16(vextq_s16(e_tr[0], e_tr[4], 2), mask_h);
            end_col0[3] = vandq_s16(vextq_s16(e_tr[0], e_tr[4], 3), mask_h);
            end_col0[4] = vandq_s16(vextq_s16(e_tr[0], e_tr[4], 4), mask_h);

            end_col1[0] = vandq_s16(e_tr[1], mask_h);
            end_col1[1] = vandq_s16(vextq_s16(e_tr[1], e_tr[5], 1), mask_h);
            end_col1[2] = vandq_s16(vextq_s16(e_tr[1], e_tr[5], 2), mask_h);
            end_col1[3] = vandq_s16(vextq_s16(e_tr[1], e_tr[5], 3), mask_h);
            end_col1[4] = vandq_s16(vextq_s16(e_tr[1], e_tr[5], 4), mask_h);

            end_col2[0] = vandq_s16(e_tr[2], mask_h);
            end_col2[1] = vandq_s16(vextq_s16(e_tr[2], e_tr[6], 1), mask_h);
            end_col2[2] = vandq_s16(vextq_s16(e_tr[2], e_tr[6], 2), mask_h);
            end_col2[3] = vandq_s16(vextq_s16(e_tr[2], e_tr[6], 3), mask_h);
            end_col2[4] = vandq_s16(vextq_s16(e_tr[2], e_tr[6], 4), mask_h);

            end_col3[0] = vandq_s16(e_tr[3], mask_h);
            end_col3[1] = vandq_s16(vextq_s16(e_tr[3], e_tr[7], 1), mask_h);
            end_col3[2] = vandq_s16(vextq_s16(e_tr[3], e_tr[7], 2), mask_h);
            end_col3[3] = vandq_s16(vextq_s16(e_tr[3], e_tr[7], 3), mask_h);
            end_col3[4] = vandq_s16(vextq_s16(e_tr[3], e_tr[7], 4), mask_h);

            // i = 1, j = 2;
            add_deltas_step4(end_col0, end_col1, deltas[0]);

            // i = 1, j = 3;
            add_deltas_step4(end_col0, end_col2, deltas[1]);

            // i = 1, j = 4
            add_deltas_step4(end_col0, end_col3, deltas[2]);

            // i = 2, j =3
            add_deltas_step4(end_col1, end_col2, deltas[3]);

            // i = 2, j = 4
            add_deltas_step4(end_col1, end_col3, deltas[4]);

            // i = 3, j = 4
            add_deltas_step4(end_col2, end_col3, deltas[5]);
        }

        int32x4_t delta[6][2];
        int32_t   single_delta[6];

        delta[0][0] = horizontal_add_4d_s32x4(&deltas[0][0]);
        delta[1][0] = horizontal_add_4d_s32x4(&deltas[1][0]);
        delta[2][0] = horizontal_add_4d_s32x4(&deltas[2][0]);
        delta[3][0] = horizontal_add_4d_s32x4(&deltas[3][0]);
        delta[4][0] = horizontal_add_4d_s32x4(&deltas[4][0]);
        delta[5][0] = horizontal_add_4d_s32x4(&deltas[5][0]);

        delta[0][1] = horizontal_add_4d_s32x4(&deltas[0][5]);
        delta[1][1] = horizontal_add_4d_s32x4(&deltas[1][5]);
        delta[2][1] = horizontal_add_4d_s32x4(&deltas[2][5]);
        delta[3][1] = horizontal_add_4d_s32x4(&deltas[3][5]);
        delta[4][1] = horizontal_add_4d_s32x4(&deltas[4][5]);
        delta[5][1] = horizontal_add_4d_s32x4(&deltas[5][5]);

        single_delta[0] = vaddvq_s32(deltas[0][4]);
        single_delta[1] = vaddvq_s32(deltas[1][4]);
        single_delta[2] = vaddvq_s32(deltas[2][4]);
        single_delta[3] = vaddvq_s32(deltas[3][4]);
        single_delta[4] = vaddvq_s32(deltas[4][4]);
        single_delta[5] = vaddvq_s32(deltas[5][4]);

        int idx = 0;
        for (i = 1; i < wiener_win - 1; i++) {
            for (j = i + 1; j < wiener_win; j++) {
                update_4_stats_neon(H + (i - 1) * wiener_win * wiener_win2 + (j - 1) * wiener_win,
                                    delta[idx][0],
                                    H + i * wiener_win * wiener_win2 + j * wiener_win);
                H[i * wiener_win * wiener_win2 + j * wiener_win + 4] =
                    H[(i - 1) * wiener_win * wiener_win2 + (j - 1) * wiener_win + 4] + single_delta[idx];

                H[(i * wiener_win + 1) * wiener_win2 + j * wiener_win] =
                    H[((i - 1) * wiener_win + 1) * wiener_win2 + (j - 1) * wiener_win] +
                    vgetq_lane_s32(delta[idx][1], 0);
                H[(i * wiener_win + 2) * wiener_win2 + j * wiener_win] =
                    H[((i - 1) * wiener_win + 2) * wiener_win2 + (j - 1) * wiener_win] +
                    vgetq_lane_s32(delta[idx][1], 1);
                H[(i * wiener_win + 3) * wiener_win2 + j * wiener_win] =
                    H[((i - 1) * wiener_win + 3) * wiener_win2 + (j - 1) * wiener_win] +
                    vgetq_lane_s32(delta[idx][1], 2);
                H[(i * wiener_win + 4) * wiener_win2 + j * wiener_win] =
                    H[((i - 1) * wiener_win + 4) * wiener_win2 + (j - 1) * wiener_win] +
                    vgetq_lane_s32(delta[idx][1], 3);

                idx++;
            }
        }
    }

    // Step 5: Derive other points of each square. No square in bottom row.
    i = 0;
    do {
        const int16_t* const di = d + i;

        j = i + 1;
        do {
            const int16_t* const dj                                        = d + j;
            int32x4_t deltas[WIENER_WIN_CHROMA - 1][WIENER_WIN_CHROMA - 1] = {{vdupq_n_s32(0)}, {vdupq_n_s32(0)}};
            int16x8_t d_is[WIN_CHROMA], d_ie[WIN_CHROMA];
            int16x8_t d_js[WIN_CHROMA], d_je[WIN_CHROMA];

            x = 0;
            while (x < w16) {
                load_square_win5_neon(di + x, dj + x, d_stride, height, d_is, d_ie, d_js, d_je);
                derive_square_win5_neon(d_is, d_ie, d_js, d_je, deltas);
                x += 16;
            }

            if (w16 != width) {
                load_square_win5_neon(di + x, dj + x, d_stride, height, d_is, d_ie, d_js, d_je);
                d_is[0] = vandq_s16(d_is[0], mask.val[0]);
                d_is[1] = vandq_s16(d_is[1], mask.val[1]);
                d_is[2] = vandq_s16(d_is[2], mask.val[0]);
                d_is[3] = vandq_s16(d_is[3], mask.val[1]);
                d_is[4] = vandq_s16(d_is[4], mask.val[0]);
                d_is[5] = vandq_s16(d_is[5], mask.val[1]);
                d_is[6] = vandq_s16(d_is[6], mask.val[0]);
                d_is[7] = vandq_s16(d_is[7], mask.val[1]);
                d_ie[0] = vandq_s16(d_ie[0], mask.val[0]);
                d_ie[1] = vandq_s16(d_ie[1], mask.val[1]);
                d_ie[2] = vandq_s16(d_ie[2], mask.val[0]);
                d_ie[3] = vandq_s16(d_ie[3], mask.val[1]);
                d_ie[4] = vandq_s16(d_ie[4], mask.val[0]);
                d_ie[5] = vandq_s16(d_ie[5], mask.val[1]);
                d_ie[6] = vandq_s16(d_ie[6], mask.val[0]);
                d_ie[7] = vandq_s16(d_ie[7], mask.val[1]);
                derive_square_win5_neon(d_is, d_ie, d_js, d_je, deltas);
            }

            hadd_update_4_stats_neon(H + (i * wiener_win + 0) * wiener_win2 + j * wiener_win,
                                     deltas[0],
                                     H + (i * wiener_win + 1) * wiener_win2 + j * wiener_win + 1);
            hadd_update_4_stats_neon(H + (i * wiener_win + 1) * wiener_win2 + j * wiener_win,
                                     deltas[1],
                                     H + (i * wiener_win + 2) * wiener_win2 + j * wiener_win + 1);
            hadd_update_4_stats_neon(H + (i * wiener_win + 2) * wiener_win2 + j * wiener_win,
                                     deltas[2],
                                     H + (i * wiener_win + 3) * wiener_win2 + j * wiener_win + 1);
            hadd_update_4_stats_neon(H + (i * wiener_win + 3) * wiener_win2 + j * wiener_win,
                                     deltas[3],
                                     H + (i * wiener_win + 4) * wiener_win2 + j * wiener_win + 1);
        } while (++j < wiener_win);
    } while (++i < wiener_win - 1);

    // Step 6: Derive other points of each upper triangle along the diagonal.
    i = 0;
    do {
        const int16_t* const di                                = d + i;
        int32x4_t            deltas[WIENER_WIN_CHROMA * 2 + 1] = {vdupq_n_s32(0)};
        int16x8_t            d_is[WIN_CHROMA], d_ie[WIN_CHROMA];

        x = 0;
        while (x < w16) {
            load_triangle_win5_neon(di + x, d_stride, height, d_is, d_ie);
            derive_triangle_win5_neon(d_is, d_ie, deltas);
            x += 16;
        }

        if (w16 != width) {
            load_triangle_win5_neon(di + x, d_stride, height, d_is, d_ie);
            d_is[0] = vandq_s16(d_is[0], mask.val[0]);
            d_is[1] = vandq_s16(d_is[1], mask.val[1]);
            d_is[2] = vandq_s16(d_is[2], mask.val[0]);
            d_is[3] = vandq_s16(d_is[3], mask.val[1]);
            d_is[4] = vandq_s16(d_is[4], mask.val[0]);
            d_is[5] = vandq_s16(d_is[5], mask.val[1]);
            d_is[6] = vandq_s16(d_is[6], mask.val[0]);
            d_is[7] = vandq_s16(d_is[7], mask.val[1]);
            d_ie[0] = vandq_s16(d_ie[0], mask.val[0]);
            d_ie[1] = vandq_s16(d_ie[1], mask.val[1]);
            d_ie[2] = vandq_s16(d_ie[2], mask.val[0]);
            d_ie[3] = vandq_s16(d_ie[3], mask.val[1]);
            d_ie[4] = vandq_s16(d_ie[4], mask.val[0]);
            d_ie[5] = vandq_s16(d_ie[5], mask.val[1]);
            d_ie[6] = vandq_s16(d_ie[6], mask.val[0]);
            d_ie[7] = vandq_s16(d_ie[7], mask.val[1]);
            derive_triangle_win5_neon(d_is, d_ie, deltas);
        }

        // Row 1: 4 points
        hadd_update_4_stats_neon(H + (i * wiener_win + 0) * wiener_win2 + i * wiener_win,
                                 deltas,
                                 H + (i * wiener_win + 1) * wiener_win2 + i * wiener_win + 1);

        // Row 2: 3 points
        deltas[4] = horizontal_add_4d_s32x4(&deltas[4]);

        int64x2_t src = vld1q_s64(H + (i * wiener_win + 1) * wiener_win2 + i * wiener_win + 1);
        int64x2_t dst = vaddw_s32(src, vget_low_s32(deltas[4]));
        vst1q_s64(H + (i * wiener_win + 2) * wiener_win2 + i * wiener_win + 2, dst);

        H[(i * wiener_win + 2) * wiener_win2 + i * wiener_win + 4] =
            H[(i * wiener_win + 1) * wiener_win2 + i * wiener_win + 3] + vgetq_lane_s32(deltas[4], 2);

        // Row 3: 2 points
        deltas[7] = horizontal_add_4d_s32x4(&deltas[7]);
        vst1q_s64(H + (i * wiener_win + 3) * wiener_win2 + i * wiener_win + 3, vaddw_s32(dst, vget_low_s32(deltas[7])));

        // Row 4: 1 point
        H[(i * wiener_win + 4) * wiener_win2 + i * wiener_win + 4] =
            H[(i * wiener_win + 3) * wiener_win2 + i * wiener_win + 3] + vgetq_lane_s32(deltas[7], 2);
    } while (++i < wiener_win);
}

static inline void compute_stats_win7_highbd_neon(const int16_t* const d, const int32_t d_stride,
                                                  const int16_t* const s, const int32_t s_stride, const int32_t width,
                                                  const int32_t height, int64_t* const M, int64_t* const H,
                                                  EbBitDepth bit_depth) {
    const int32_t     wiener_win  = WIENER_WIN;
    const int32_t     wiener_win2 = wiener_win * wiener_win;
    const int32_t     w16         = width & ~15;
    const int32_t     h8          = height & ~7;
    const int16x8x2_t mask        = vld1q_s16_x2(&(mask_16bit[16]) - width % 16);
    int32_t           i, j, x, y;

    if (bit_depth == EB_EIGHT_BIT) {
        // Step 1: Calculate the top edge of the whole matrix, i.e., the top
        // edge of each triangle and square on the top row.
        j = 0;
        do {
            const int16_t* s_t = s;
            const int16_t* d_t = d;
            // Allocate an extra 0 register to allow reduction as 2x4 rather than 4 + 3.
            int32x4_t sum_m[WIENER_WIN + 1] = {vdupq_n_s32(0)};
            int32x4_t sum_h[WIENER_WIN + 1] = {vdupq_n_s32(0)};
            int16x8_t src[2], dgd[2];

            y = height;
            do {
                x = 0;
                while (x < w16) {
                    src[0] = vld1q_s16(s_t + x + 0);
                    src[1] = vld1q_s16(s_t + x + 8);
                    dgd[0] = vld1q_s16(d_t + x + 0);
                    dgd[1] = vld1q_s16(d_t + x + 8);
                    stats_top_win7_neon(src, dgd, d_t + j + x, d_stride, sum_m, sum_h);
                    x += 16;
                }

                if (w16 != width) {
                    src[0] = vld1q_s16(s_t + w16 + 0);
                    src[1] = vld1q_s16(s_t + w16 + 8);
                    dgd[0] = vld1q_s16(d_t + w16 + 0);
                    dgd[1] = vld1q_s16(d_t + w16 + 8);
                    src[0] = vandq_s16(src[0], mask.val[0]);
                    src[1] = vandq_s16(src[1], mask.val[1]);
                    dgd[0] = vandq_s16(dgd[0], mask.val[0]);
                    dgd[1] = vandq_s16(dgd[1], mask.val[1]);
                    stats_top_win7_neon(src, dgd, d_t + j + w16, d_stride, sum_m, sum_h);
                }

                s_t += s_stride;
                d_t += d_stride;
            } while (--y);

            int32x4_t m0123 = horizontal_add_4d_s32x4(&sum_m[0]);
            vst1q_s64(M + wiener_win * j + 0, vmovl_s32(vget_low_s32(m0123)));
            vst1q_s64(M + wiener_win * j + 2, vmovl_s32(vget_high_s32(m0123)));
            int32x4_t m456 = horizontal_add_4d_s32x4(&sum_m[4]);
            vst1q_s64(M + wiener_win * j + 4, vmovl_s32(vget_low_s32(m456)));
            vst1_s64(M + wiener_win * j + 6, vget_low_s64(vmovl_s32(vget_high_s32(m456))));

            int32x4_t h0123 = horizontal_add_4d_s32x4(&sum_h[0]);
            vst1q_s64(H + wiener_win * j + 0, vmovl_s32(vget_low_s32(h0123)));
            vst1q_s64(H + wiener_win * j + 2, vmovl_s32(vget_high_s32(h0123)));
            int32x4_t h456 = horizontal_add_4d_s32x4(&sum_h[4]);
            vst1q_s64(H + wiener_win * j + 4, vmovl_s32(vget_low_s32(h456)));
            vst1_s64(H + wiener_win * j + 6, vget_low_s64(vmovl_s32(vget_high_s32(h456))));
        } while (++j < wiener_win);

        // Step 2: Calculate the left edge of each square on the top row.
        j = 1;
        do {
            const int16_t* d_t                   = d;
            int32x4_t      sum_h[WIENER_WIN - 1] = {vdupq_n_s32(0)};
            int16x8_t      dgd[2];

            y = height;
            do {
                x = 0;
                while (x < w16) {
                    dgd[0] = vld1q_s16(d_t + j + x + 0);
                    dgd[1] = vld1q_s16(d_t + j + x + 8);
                    stats_left_win7_neon(dgd, d_t + x, d_stride, sum_h);
                    x += 16;
                }

                if (w16 != width) {
                    dgd[0] = vld1q_s16(d_t + j + x + 0);
                    dgd[1] = vld1q_s16(d_t + j + x + 8);
                    dgd[0] = vandq_s16(dgd[0], mask.val[0]);
                    dgd[1] = vandq_s16(dgd[1], mask.val[1]);
                    stats_left_win7_neon(dgd, d_t + x, d_stride, sum_h);
                }

                d_t += d_stride;
            } while (--y);

            int64x2_t sum_h0_s64 = vpaddlq_s32(vpaddq_s32(sum_h[0], sum_h[1]));
            int64x2_t sum_h2_s64 = vpaddlq_s32(vpaddq_s32(sum_h[2], sum_h[3]));
            int64x2_t sum_h4_s64 = vpaddlq_s32(vpaddq_s32(sum_h[4], sum_h[5]));
            vst1_s64(&H[1 * wiener_win2 + j * wiener_win], vget_low_s64(sum_h0_s64));
            vst1_s64(&H[2 * wiener_win2 + j * wiener_win], vget_high_s64(sum_h0_s64));
            vst1_s64(&H[3 * wiener_win2 + j * wiener_win], vget_low_s64(sum_h2_s64));
            vst1_s64(&H[4 * wiener_win2 + j * wiener_win], vget_high_s64(sum_h2_s64));
            vst1_s64(&H[5 * wiener_win2 + j * wiener_win], vget_low_s64(sum_h4_s64));
            vst1_s64(&H[6 * wiener_win2 + j * wiener_win], vget_high_s64(sum_h4_s64));
        } while (++j < wiener_win);
    } else {
        const int32_t num_bit_left = 32 - 1 /* sign */ - 2 * bit_depth /* energy */ + 2 /* SIMD */;
        const int32_t h_allowed    = (1 << num_bit_left) / (w16 + ((w16 != width) ? 16 : 0));

        // Step 1: Calculate the top edge of the whole matrix, i.e., the top
        // edge of each triangle and square on the top row.
        j = 0;
        do {
            const int16_t* s_t               = s;
            const int16_t* d_t               = d;
            int32_t        height_t          = 0;
            int64x2_t      sum_m[WIENER_WIN] = {vdupq_n_s64(0)};
            int64x2_t      sum_h[WIENER_WIN] = {vdupq_n_s64(0)};
            int16x8_t      src[2], dgd[2];

            do {
                const int32_t h_t = ((height - height_t) < h_allowed) ? (height - height_t) : h_allowed;
                int32x4_t     row_m[WIENER_WIN * 2] = {vdupq_n_s32(0)};
                int32x4_t     row_h[WIENER_WIN * 2] = {vdupq_n_s32(0)};

                y = h_t;
                do {
                    x = 0;
                    while (x < w16) {
                        src[0] = vld1q_s16(s_t + x + 0);
                        src[1] = vld1q_s16(s_t + x + 8);
                        dgd[0] = vld1q_s16(d_t + x + 0);
                        dgd[1] = vld1q_s16(d_t + x + 8);
                        stats_top_win7_neon(src, dgd, d_t + j + x, d_stride, row_m, row_h);
                        x += 16;
                    }

                    if (w16 != width) {
                        src[0] = vld1q_s16(s_t + w16 + 0);
                        src[1] = vld1q_s16(s_t + w16 + 8);
                        dgd[0] = vld1q_s16(d_t + w16 + 0);
                        dgd[1] = vld1q_s16(d_t + w16 + 8);
                        src[0] = vandq_s16(src[0], mask.val[0]);
                        src[1] = vandq_s16(src[1], mask.val[1]);
                        dgd[0] = vandq_s16(dgd[0], mask.val[0]);
                        dgd[1] = vandq_s16(dgd[1], mask.val[1]);
                        stats_top_win7_neon(src, dgd, d_t + j + w16, d_stride, row_m, row_h);
                    }

                    s_t += s_stride;
                    d_t += d_stride;
                } while (--y);

                sum_m[0] = vpadalq_s32(sum_m[0], row_m[0]);
                sum_m[1] = vpadalq_s32(sum_m[1], row_m[1]);
                sum_m[2] = vpadalq_s32(sum_m[2], row_m[2]);
                sum_m[3] = vpadalq_s32(sum_m[3], row_m[3]);
                sum_m[4] = vpadalq_s32(sum_m[4], row_m[4]);
                sum_m[5] = vpadalq_s32(sum_m[5], row_m[5]);
                sum_m[6] = vpadalq_s32(sum_m[6], row_m[6]);

                sum_h[0] = vpadalq_s32(sum_h[0], row_h[0]);
                sum_h[1] = vpadalq_s32(sum_h[1], row_h[1]);
                sum_h[2] = vpadalq_s32(sum_h[2], row_h[2]);
                sum_h[3] = vpadalq_s32(sum_h[3], row_h[3]);
                sum_h[4] = vpadalq_s32(sum_h[4], row_h[4]);
                sum_h[5] = vpadalq_s32(sum_h[5], row_h[5]);
                sum_h[6] = vpadalq_s32(sum_h[6], row_h[6]);

                height_t += h_t;
            } while (height_t < height);

            int64x2_t m01 = vpaddq_s64(sum_m[0], sum_m[1]);
            int64x2_t m23 = vpaddq_s64(sum_m[2], sum_m[3]);
            int64x2_t m45 = vpaddq_s64(sum_m[4], sum_m[5]);
            vst1q_s64(M + wiener_win * j + 0, m01);
            vst1q_s64(M + wiener_win * j + 2, m23);
            vst1q_s64(M + wiener_win * j + 4, m45);
            M[wiener_win * j + 6] = vaddvq_s64(sum_m[6]);

            int64x2_t h01 = vpaddq_s64(sum_h[0], sum_h[1]);
            int64x2_t h23 = vpaddq_s64(sum_h[2], sum_h[3]);
            int64x2_t h45 = vpaddq_s64(sum_h[4], sum_h[5]);
            vst1q_s64(H + wiener_win * j + 0, h01);
            vst1q_s64(H + wiener_win * j + 2, h23);
            vst1q_s64(H + wiener_win * j + 4, h45);
            H[wiener_win * j + 6] = vaddvq_s64(sum_h[6]);
        } while (++j < wiener_win);

        // Step 2: Calculate the left edge of each square on the top row.
        j = 1;
        do {
            const int16_t* d_t                   = d;
            int32_t        height_t              = 0;
            int64x2_t      sum_h[WIENER_WIN - 1] = {vdupq_n_s64(0)};
            int16x8_t      dgd[2];

            do {
                const int32_t h_t = ((height - height_t) < h_allowed) ? (height - height_t) : h_allowed;
                int32x4_t     row_h[WIENER_WIN - 1] = {vdupq_n_s32(0)};

                y = h_t;
                do {
                    x = 0;
                    while (x < w16) {
                        dgd[0] = vld1q_s16(d_t + j + x + 0);
                        dgd[1] = vld1q_s16(d_t + j + x + 8);
                        stats_left_win7_neon(dgd, d_t + x, d_stride, row_h);
                        x += 16;
                    }

                    if (w16 != width) {
                        dgd[0] = vld1q_s16(d_t + j + x + 0);
                        dgd[1] = vld1q_s16(d_t + j + x + 8);
                        dgd[0] = vandq_s16(dgd[0], mask.val[0]);
                        dgd[1] = vandq_s16(dgd[1], mask.val[1]);
                        stats_left_win7_neon(dgd, d_t + x, d_stride, row_h);
                    }

                    d_t += d_stride;
                } while (--y);

                sum_h[0] = vpadalq_s32(sum_h[0], row_h[0]);
                sum_h[1] = vpadalq_s32(sum_h[1], row_h[1]);
                sum_h[2] = vpadalq_s32(sum_h[2], row_h[2]);
                sum_h[3] = vpadalq_s32(sum_h[3], row_h[3]);
                sum_h[4] = vpadalq_s32(sum_h[4], row_h[4]);
                sum_h[5] = vpadalq_s32(sum_h[5], row_h[5]);

                height_t += h_t;
            } while (height_t < height);

            int64x2_t sum_h0 = vpaddq_s64(sum_h[0], sum_h[1]);
            int64x2_t sum_h2 = vpaddq_s64(sum_h[2], sum_h[3]);
            int64x2_t sum_h4 = vpaddq_s64(sum_h[4], sum_h[5]);
            vst1_s64(&H[1 * wiener_win2 + j * wiener_win], vget_low_s64(sum_h0));
            vst1_s64(&H[2 * wiener_win2 + j * wiener_win], vget_high_s64(sum_h0));
            vst1_s64(&H[3 * wiener_win2 + j * wiener_win], vget_low_s64(sum_h2));
            vst1_s64(&H[4 * wiener_win2 + j * wiener_win], vget_high_s64(sum_h2));
            vst1_s64(&H[5 * wiener_win2 + j * wiener_win], vget_low_s64(sum_h4));
            vst1_s64(&H[6 * wiener_win2 + j * wiener_win], vget_high_s64(sum_h4));

        } while (++j < wiener_win);
    }

    // Step 3: Derive the top edge of each triangle along the diagonal. No
    // triangle in top row.
    {
        const int16_t* d_t = d;
        // Pad to call transpose function.
        int32x4_t deltas[(WIENER_WIN + 1) * 2] = {vdupq_n_s32(0)};
        int16x8_t ds[WIENER_WIN * 2];

        load_s16_8x6(d_t, d_stride, &ds[0], &ds[2], &ds[4], &ds[6], &ds[8], &ds[10]);
        load_s16_8x6(d_t + width, d_stride, &ds[1], &ds[3], &ds[5], &ds[7], &ds[9], &ds[11]);

        d_t += 6 * d_stride;

        step3_win7_neon(d_t, d_stride, width, height, ds, deltas);

        transpose_32bit_8x8_neon(deltas, deltas);

        update_8_stats_neon(H + 0 * wiener_win * wiener_win2 + 0 * wiener_win,
                            &deltas[0],
                            H + 1 * wiener_win * wiener_win2 + 1 * wiener_win);
        update_8_stats_neon(H + 1 * wiener_win * wiener_win2 + 1 * wiener_win,
                            &deltas[2],
                            H + 2 * wiener_win * wiener_win2 + 2 * wiener_win);
        update_8_stats_neon(H + 2 * wiener_win * wiener_win2 + 2 * wiener_win,
                            &deltas[4],
                            H + 3 * wiener_win * wiener_win2 + 3 * wiener_win);
        update_8_stats_neon(H + 3 * wiener_win * wiener_win2 + 3 * wiener_win,
                            &deltas[6],
                            H + 4 * wiener_win * wiener_win2 + 4 * wiener_win);
        update_8_stats_neon(H + 4 * wiener_win * wiener_win2 + 4 * wiener_win,
                            &deltas[8],
                            H + 5 * wiener_win * wiener_win2 + 5 * wiener_win);
        update_8_stats_neon(H + 5 * wiener_win * wiener_win2 + 5 * wiener_win,
                            &deltas[10],
                            H + 6 * wiener_win * wiener_win2 + 6 * wiener_win);
    }

    // Step 4: Derive the top and left edge of each square. No square in top and
    // bottom row.

    i = 1;
    do {
        j = i + 1;
        do {
            const int16_t* di                               = d + i - 1;
            const int16_t* dj                               = d + j - 1;
            int32x4_t      deltas[(2 * WIENER_WIN - 1) * 2] = {vdupq_n_s32(0)};
            int16x8_t      dd[WIENER_WIN * 2], ds[WIENER_WIN * 2];

            dd[5]                      = vdupq_n_s16(0); // Initialize to avoid warning.
            const int16_t dd0_values[] = {di[0 * d_stride],
                                          di[1 * d_stride],
                                          di[2 * d_stride],
                                          di[3 * d_stride],
                                          di[4 * d_stride],
                                          di[5 * d_stride],
                                          0,
                                          0};
            dd[0]                      = vld1q_s16(dd0_values);
            const int16_t dd1_values[] = {di[0 * d_stride + width],
                                          di[1 * d_stride + width],
                                          di[2 * d_stride + width],
                                          di[3 * d_stride + width],
                                          di[4 * d_stride + width],
                                          di[5 * d_stride + width],
                                          0,
                                          0};
            dd[1]                      = vld1q_s16(dd1_values);
            const int16_t ds0_values[] = {dj[0 * d_stride],
                                          dj[1 * d_stride],
                                          dj[2 * d_stride],
                                          dj[3 * d_stride],
                                          dj[4 * d_stride],
                                          dj[5 * d_stride],
                                          0,
                                          0};
            ds[0]                      = vld1q_s16(ds0_values);
            int16_t ds1_values[]       = {dj[0 * d_stride + width],
                                          dj[1 * d_stride + width],
                                          dj[2 * d_stride + width],
                                          dj[3 * d_stride + width],
                                          dj[4 * d_stride + width],
                                          dj[5 * d_stride + width],
                                          0,
                                          0};
            ds[1]                      = vld1q_s16(ds1_values);

            y = 0;
            while (y < h8) {
                // 00s 10s 20s 30s 40s 50s 60s 70s  00e 10e 20e 30e 40e 50e 60e 70e
                dd[0] = vsetq_lane_s16(di[6 * d_stride], dd[0], 6);
                dd[0] = vsetq_lane_s16(di[7 * d_stride], dd[0], 7);
                dd[1] = vsetq_lane_s16(di[6 * d_stride + width], dd[1], 6);
                dd[1] = vsetq_lane_s16(di[7 * d_stride + width], dd[1], 7);

                // 00s 10s 20s 30s 40s 50s 60s 70s  00e 10e 20e 30e 40e 50e 60e 70e
                // 01s 11s 21s 31s 41s 51s 61s 71s  01e 11e 21e 31e 41e 51e 61e 71e
                ds[0] = vsetq_lane_s16(dj[6 * d_stride], ds[0], 6);
                ds[0] = vsetq_lane_s16(dj[7 * d_stride], ds[0], 7);
                ds[1] = vsetq_lane_s16(dj[6 * d_stride + width], ds[1], 6);
                ds[1] = vsetq_lane_s16(dj[7 * d_stride + width], ds[1], 7);

                load_more_16_neon(di + 8 * d_stride, width, &dd[0], &dd[2]);
                load_more_16_neon(dj + 8 * d_stride, width, &ds[0], &ds[2]);
                load_more_16_neon(di + 9 * d_stride, width, &dd[2], &dd[4]);
                load_more_16_neon(dj + 9 * d_stride, width, &ds[2], &ds[4]);
                load_more_16_neon(di + 10 * d_stride, width, &dd[4], &dd[6]);
                load_more_16_neon(dj + 10 * d_stride, width, &ds[4], &ds[6]);
                load_more_16_neon(di + 11 * d_stride, width, &dd[6], &dd[8]);
                load_more_16_neon(dj + 11 * d_stride, width, &ds[6], &ds[8]);
                load_more_16_neon(di + 12 * d_stride, width, &dd[8], &dd[10]);
                load_more_16_neon(dj + 12 * d_stride, width, &ds[8], &ds[10]);
                load_more_16_neon(di + 13 * d_stride, width, &dd[10], &dd[12]);
                load_more_16_neon(dj + 13 * d_stride, width, &ds[10], &ds[12]);

                madd_neon(&deltas[0], dd[0], ds[0]);
                madd_neon(&deltas[1], dd[1], ds[1]);
                madd_neon(&deltas[2], dd[0], ds[2]);
                madd_neon(&deltas[3], dd[1], ds[3]);
                madd_neon(&deltas[4], dd[0], ds[4]);
                madd_neon(&deltas[5], dd[1], ds[5]);
                madd_neon(&deltas[6], dd[0], ds[6]);
                madd_neon(&deltas[7], dd[1], ds[7]);
                madd_neon(&deltas[8], dd[0], ds[8]);
                madd_neon(&deltas[9], dd[1], ds[9]);
                madd_neon(&deltas[10], dd[0], ds[10]);
                madd_neon(&deltas[11], dd[1], ds[11]);
                madd_neon(&deltas[12], dd[0], ds[12]);
                madd_neon(&deltas[13], dd[1], ds[13]);
                madd_neon(&deltas[14], dd[2], ds[0]);
                madd_neon(&deltas[15], dd[3], ds[1]);
                madd_neon(&deltas[16], dd[4], ds[0]);
                madd_neon(&deltas[17], dd[5], ds[1]);
                madd_neon(&deltas[18], dd[6], ds[0]);
                madd_neon(&deltas[19], dd[7], ds[1]);
                madd_neon(&deltas[20], dd[8], ds[0]);
                madd_neon(&deltas[21], dd[9], ds[1]);
                madd_neon(&deltas[22], dd[10], ds[0]);
                madd_neon(&deltas[23], dd[11], ds[1]);
                madd_neon(&deltas[24], dd[12], ds[0]);
                madd_neon(&deltas[25], dd[13], ds[1]);

                dd[0] = vextq_s16(dd[12], vdupq_n_s16(0), 2);
                dd[1] = vextq_s16(dd[13], vdupq_n_s16(0), 2);
                ds[0] = vextq_s16(ds[12], vdupq_n_s16(0), 2);
                ds[1] = vextq_s16(ds[13], vdupq_n_s16(0), 2);

                di += 8 * d_stride;
                dj += 8 * d_stride;
                y += 8;
            }

            deltas[0] = hadd_four_32_neon(deltas[0], deltas[2], deltas[4], deltas[6]);
            deltas[1] = hadd_four_32_neon(deltas[1], deltas[3], deltas[5], deltas[7]);
            deltas[2] = hadd_four_32_neon(deltas[8], deltas[10], deltas[12], deltas[12]);
            deltas[3] = hadd_four_32_neon(deltas[9], deltas[11], deltas[13], deltas[13]);
            deltas[4] = hadd_four_32_neon(deltas[14], deltas[16], deltas[18], deltas[20]);
            deltas[5] = hadd_four_32_neon(deltas[15], deltas[17], deltas[19], deltas[21]);
            deltas[6] = hadd_four_32_neon(deltas[22], deltas[24], deltas[22], deltas[24]);
            deltas[7] = hadd_four_32_neon(deltas[23], deltas[25], deltas[23], deltas[25]);
            deltas[0] = vsubq_s32(deltas[1], deltas[0]);
            deltas[1] = vsubq_s32(deltas[3], deltas[2]);
            deltas[2] = vsubq_s32(deltas[5], deltas[4]);
            deltas[3] = vsubq_s32(deltas[7], deltas[6]);

            if (h8 != height) {
                const int16_t ds0_vals[] = {dj[0 * d_stride],
                                            dj[0 * d_stride + width],
                                            dj[1 * d_stride],
                                            dj[1 * d_stride + width],
                                            dj[2 * d_stride],
                                            dj[2 * d_stride + width],
                                            dj[3 * d_stride],
                                            dj[3 * d_stride + width]};
                ds[0]                    = vld1q_s16(ds0_vals);

                ds[1]                    = vsetq_lane_s16(dj[4 * d_stride], ds[1], 0);
                ds[1]                    = vsetq_lane_s16(dj[4 * d_stride + width], ds[1], 1);
                ds[1]                    = vsetq_lane_s16(dj[5 * d_stride], ds[1], 2);
                ds[1]                    = vsetq_lane_s16(dj[5 * d_stride + width], ds[1], 3);
                const int16_t dd4_vals[] = {-di[1 * d_stride],
                                            di[1 * d_stride + width],
                                            -di[2 * d_stride],
                                            di[2 * d_stride + width],
                                            -di[3 * d_stride],
                                            di[3 * d_stride + width],
                                            -di[4 * d_stride],
                                            di[4 * d_stride + width]};
                dd[4]                    = vld1q_s16(dd4_vals);

                dd[5] = vsetq_lane_s16(-di[5 * d_stride], dd[5], 0);
                dd[5] = vsetq_lane_s16(di[5 * d_stride + width], dd[5], 1);
                do {
                    dd[0] = vdupq_n_s16(-di[0 * d_stride]);
                    dd[2] = dd[3] = vdupq_n_s16(di[0 * d_stride + width]);
                    dd[0] = dd[1] = vzip1q_s16(dd[0], dd[2]);

                    ds[4] = vdupq_n_s16(dj[0 * d_stride]);
                    ds[6] = ds[7] = vdupq_n_s16(dj[0 * d_stride + width]);
                    ds[4] = ds[5] = vzip1q_s16(ds[4], ds[6]);

                    dd[5] = vsetq_lane_s16(-di[6 * d_stride], dd[5], 2);
                    dd[5] = vsetq_lane_s16(di[6 * d_stride + width], dd[5], 3);
                    ds[1] = vsetq_lane_s16(dj[6 * d_stride], ds[1], 4);
                    ds[1] = vsetq_lane_s16(dj[6 * d_stride + width], ds[1], 5);

                    madd_neon_pairwise(&deltas[0], dd[0], ds[0]);
                    madd_neon_pairwise(&deltas[1], dd[1], ds[1]);
                    madd_neon_pairwise(&deltas[2], dd[4], ds[4]);
                    madd_neon_pairwise(&deltas[3], dd[5], ds[5]);

                    int32_t tmp0 = vgetq_lane_s32(vreinterpretq_s32_s16(ds[0]), 0);
                    ds[0]        = vextq_s16(ds[0], ds[1], 2);
                    ds[1]        = vextq_s16(ds[1], ds[0], 2);
                    ds[1]        = vreinterpretq_s16_s32(vsetq_lane_s32(tmp0, vreinterpretq_s32_s16(ds[1]), 3));
                    int32_t tmp1 = vgetq_lane_s32(vreinterpretq_s32_s16(dd[4]), 0);
                    dd[4]        = vextq_s16(dd[4], dd[5], 2);
                    dd[5]        = vextq_s16(dd[5], dd[4], 2);
                    dd[5]        = vreinterpretq_s16_s32(vsetq_lane_s32(tmp1, vreinterpretq_s32_s16(dd[5]), 3));
                    di += d_stride;
                    dj += d_stride;
                } while (++y < height);
            }

            // Writing one more element on the top edge of a square falls to
            // the next square in the same row or the first element in the next
            // row, which will just be overwritten later.
            update_8_stats_neon(H + (i - 1) * wiener_win * wiener_win2 + (j - 1) * wiener_win,
                                &deltas[0],
                                H + i * wiener_win * wiener_win2 + j * wiener_win);

            H[(i * wiener_win + 1) * wiener_win2 + j * wiener_win] =
                H[((i - 1) * wiener_win + 1) * wiener_win2 + (j - 1) * wiener_win] + vgetq_lane_s32(deltas[2], 0);
            H[(i * wiener_win + 2) * wiener_win2 + j * wiener_win] =
                H[((i - 1) * wiener_win + 2) * wiener_win2 + (j - 1) * wiener_win] + vgetq_lane_s32(deltas[2], 1);
            H[(i * wiener_win + 3) * wiener_win2 + j * wiener_win] =
                H[((i - 1) * wiener_win + 3) * wiener_win2 + (j - 1) * wiener_win] + vgetq_lane_s32(deltas[2], 2);
            H[(i * wiener_win + 4) * wiener_win2 + j * wiener_win] =
                H[((i - 1) * wiener_win + 4) * wiener_win2 + (j - 1) * wiener_win] + vgetq_lane_s32(deltas[2], 3);
            H[(i * wiener_win + 5) * wiener_win2 + j * wiener_win] =
                H[((i - 1) * wiener_win + 5) * wiener_win2 + (j - 1) * wiener_win] + vgetq_lane_s32(deltas[3], 0);
            H[(i * wiener_win + 6) * wiener_win2 + j * wiener_win] =
                H[((i - 1) * wiener_win + 6) * wiener_win2 + (j - 1) * wiener_win] + vgetq_lane_s32(deltas[3], 1);
        } while (++j < wiener_win);
    } while (++i < wiener_win - 1);

    // Step 5: Derive other points of each square. No square in bottom row.
    i = 0;
    do {
        const int16_t* const di = d + i;

        j = i + 1;
        do {
            const int16_t* const dj                            = d + j;
            int32x4_t            deltas[WIENER_WIN - 1][WIN_7] = {{vdupq_n_s32(0)}, {vdupq_n_s32(0)}};
            int16x8_t            d_is[WIN_7];
            int16x8_t            d_ie[WIN_7];
            int16x8_t            d_js[WIN_7];
            int16x8_t            d_je[WIN_7];

            x = 0;
            while (x < w16) {
                load_square_win7_neon(di + x, dj + x, d_stride, height, d_is, d_ie, d_js, d_je);
                derive_square_win7_neon(d_is, d_ie, d_js, d_je, deltas);
                x += 16;
            }

            if (w16 != width) {
                load_square_win7_neon(di + x, dj + x, d_stride, height, d_is, d_ie, d_js, d_je);
                d_is[0]  = vandq_s16(d_is[0], mask.val[0]);
                d_is[1]  = vandq_s16(d_is[1], mask.val[1]);
                d_is[2]  = vandq_s16(d_is[2], mask.val[0]);
                d_is[3]  = vandq_s16(d_is[3], mask.val[1]);
                d_is[4]  = vandq_s16(d_is[4], mask.val[0]);
                d_is[5]  = vandq_s16(d_is[5], mask.val[1]);
                d_is[6]  = vandq_s16(d_is[6], mask.val[0]);
                d_is[7]  = vandq_s16(d_is[7], mask.val[1]);
                d_is[8]  = vandq_s16(d_is[8], mask.val[0]);
                d_is[9]  = vandq_s16(d_is[9], mask.val[1]);
                d_is[10] = vandq_s16(d_is[10], mask.val[0]);
                d_is[11] = vandq_s16(d_is[11], mask.val[1]);
                d_ie[0]  = vandq_s16(d_ie[0], mask.val[0]);
                d_ie[1]  = vandq_s16(d_ie[1], mask.val[1]);
                d_ie[2]  = vandq_s16(d_ie[2], mask.val[0]);
                d_ie[3]  = vandq_s16(d_ie[3], mask.val[1]);
                d_ie[4]  = vandq_s16(d_ie[4], mask.val[0]);
                d_ie[5]  = vandq_s16(d_ie[5], mask.val[1]);
                d_ie[6]  = vandq_s16(d_ie[6], mask.val[0]);
                d_ie[7]  = vandq_s16(d_ie[7], mask.val[1]);
                d_ie[8]  = vandq_s16(d_ie[8], mask.val[0]);
                d_ie[9]  = vandq_s16(d_ie[9], mask.val[1]);
                d_ie[10] = vandq_s16(d_ie[10], mask.val[0]);
                d_ie[11] = vandq_s16(d_ie[11], mask.val[1]);
                derive_square_win7_neon(d_is, d_ie, d_js, d_je, deltas);
            }

            hadd_update_6_stats_neon(H + (i * wiener_win + 0) * wiener_win2 + j * wiener_win,
                                     deltas[0],
                                     H + (i * wiener_win + 1) * wiener_win2 + j * wiener_win + 1);
            hadd_update_6_stats_neon(H + (i * wiener_win + 1) * wiener_win2 + j * wiener_win,
                                     deltas[1],
                                     H + (i * wiener_win + 2) * wiener_win2 + j * wiener_win + 1);
            hadd_update_6_stats_neon(H + (i * wiener_win + 2) * wiener_win2 + j * wiener_win,
                                     deltas[2],
                                     H + (i * wiener_win + 3) * wiener_win2 + j * wiener_win + 1);
            hadd_update_6_stats_neon(H + (i * wiener_win + 3) * wiener_win2 + j * wiener_win,
                                     deltas[3],
                                     H + (i * wiener_win + 4) * wiener_win2 + j * wiener_win + 1);
            hadd_update_6_stats_neon(H + (i * wiener_win + 4) * wiener_win2 + j * wiener_win,
                                     deltas[4],
                                     H + (i * wiener_win + 5) * wiener_win2 + j * wiener_win + 1);
            hadd_update_6_stats_neon(H + (i * wiener_win + 5) * wiener_win2 + j * wiener_win,
                                     deltas[5],
                                     H + (i * wiener_win + 6) * wiener_win2 + j * wiener_win + 1);
        } while (++j < wiener_win);
    } while (++i < wiener_win - 1);

    // Step 6: Derive other points of each upper triangle along the diagonal.
    i = 0;
    do {
        const int16_t* const di                     = d + i;
        int32x4_t            deltas[3 * WIENER_WIN] = {vdupq_n_s32(0)};
        int16x8_t            d_is[WIN_7], d_ie[WIN_7];

        x = 0;
        while (x < w16) {
            load_triangle_win7_neon(di + x, d_stride, height, d_is, d_ie);
            derive_triangle_win7_neon(d_is, d_ie, deltas);
            x += 16;
        }

        if (w16 != width) {
            load_triangle_win7_neon(di + x, d_stride, height, d_is, d_ie);
            d_is[0]  = vandq_s16(d_is[0], mask.val[0]);
            d_is[1]  = vandq_s16(d_is[1], mask.val[1]);
            d_is[2]  = vandq_s16(d_is[2], mask.val[0]);
            d_is[3]  = vandq_s16(d_is[3], mask.val[1]);
            d_is[4]  = vandq_s16(d_is[4], mask.val[0]);
            d_is[5]  = vandq_s16(d_is[5], mask.val[1]);
            d_is[6]  = vandq_s16(d_is[6], mask.val[0]);
            d_is[7]  = vandq_s16(d_is[7], mask.val[1]);
            d_is[8]  = vandq_s16(d_is[8], mask.val[0]);
            d_is[9]  = vandq_s16(d_is[9], mask.val[1]);
            d_is[10] = vandq_s16(d_is[10], mask.val[0]);
            d_is[11] = vandq_s16(d_is[11], mask.val[1]);
            d_ie[0]  = vandq_s16(d_ie[0], mask.val[0]);
            d_ie[1]  = vandq_s16(d_ie[1], mask.val[1]);
            d_ie[2]  = vandq_s16(d_ie[2], mask.val[0]);
            d_ie[3]  = vandq_s16(d_ie[3], mask.val[1]);
            d_ie[4]  = vandq_s16(d_ie[4], mask.val[0]);
            d_ie[5]  = vandq_s16(d_ie[5], mask.val[1]);
            d_ie[6]  = vandq_s16(d_ie[6], mask.val[0]);
            d_ie[7]  = vandq_s16(d_ie[7], mask.val[1]);
            d_ie[8]  = vandq_s16(d_ie[8], mask.val[0]);
            d_ie[9]  = vandq_s16(d_ie[9], mask.val[1]);
            d_ie[10] = vandq_s16(d_ie[10], mask.val[0]);
            d_ie[11] = vandq_s16(d_ie[11], mask.val[1]);
            derive_triangle_win7_neon(d_is, d_ie, deltas);
        }

        // Row 1: 6 points
        hadd_update_6_stats_neon(H + (i * wiener_win + 0) * wiener_win2 + i * wiener_win,
                                 deltas,
                                 H + (i * wiener_win + 1) * wiener_win2 + i * wiener_win + 1);

        int32x4_t delta_reduced = hadd_four_32_neon(deltas[17], deltas[10], deltas[15], deltas[16]);

        // Row 2: 5 points
        hadd_update_4_stats_neon(H + (i * wiener_win + 1) * wiener_win2 + i * wiener_win + 1,
                                 deltas + 6,
                                 H + (i * wiener_win + 2) * wiener_win2 + i * wiener_win + 2);
        H[(i * wiener_win + 2) * wiener_win2 + i * wiener_win + 6] =
            H[(i * wiener_win + 1) * wiener_win2 + i * wiener_win + 5] + vgetq_lane_s32(delta_reduced, 1);

        // Row 3: 4 points
        hadd_update_4_stats_neon(H + (i * wiener_win + 2) * wiener_win2 + i * wiener_win + 2,
                                 deltas + 11,
                                 H + (i * wiener_win + 3) * wiener_win2 + i * wiener_win + 3);

        // Row 4: 3 points
        int64x2_t h0 = vld1q_s64(H + (i * wiener_win + 3) * wiener_win2 + i * wiener_win + 3);
        vst1q_s64(H + (i * wiener_win + 4) * wiener_win2 + i * wiener_win + 4,
                  vaddw_s32(h0, vget_high_s32(delta_reduced)));
        H[(i * wiener_win + 4) * wiener_win2 + i * wiener_win + 6] =
            H[(i * wiener_win + 3) * wiener_win2 + i * wiener_win + 5] + vgetq_lane_s32(delta_reduced, 0);

        int32x4_t delta_reduced2 = hadd_four_32_neon(deltas[18], deltas[19], deltas[20], deltas[20]);

        // Row 5: 2 points
        int64x2_t h1 = vld1q_s64(H + (i * wiener_win + 4) * wiener_win2 + i * wiener_win + 4);
        vst1q_s64(H + (i * wiener_win + 5) * wiener_win2 + i * wiener_win + 5,
                  vaddw_s32(h1, vget_low_s32(delta_reduced2)));

        // Row 6: 1 points
        H[(i * wiener_win + 6) * wiener_win2 + i * wiener_win + 6] =
            H[(i * wiener_win + 5) * wiener_win2 + i * wiener_win + 5] + vgetq_lane_s32(delta_reduced2, 2);
    } while (++i < wiener_win);
}

static inline void sub_avg_block_highbd_neon(const uint16_t* src, const int32_t src_stride, const uint16_t avg,
                                             const int32_t width, const int32_t height, int16_t* dst,
                                             const int32_t dst_stride) {
    const uint16x8_t a = vdupq_n_u16(avg);

    int32_t i = height + 1;
    do {
        int32_t j = 0;
        while (j < width) {
            const uint16x8_t s = vld1q_u16(src + j);
            const uint16x8_t d = vsubq_u16(s, a);
            vst1q_s16(dst + j, vreinterpretq_s16_u16(d));
            j += 8;
        }

        src += src_stride;
        dst += dst_stride;
    } while (--i);
}

static inline uint16_t highbd_find_average_neon(const uint16_t* src, int src_stride, int width, int height) {
    assert(width > 0);
    assert(height > 0);

    uint64x2_t       sum_u64 = vdupq_n_u64(0);
    uint64_t         sum     = 0;
    const uint16x8_t mask    = vreinterpretq_u16_s16(vld1q_s16(&mask_16bit[16] - (width % 8)));

    int h = height;
    do {
        uint32x4_t sum_u32[2] = {vdupq_n_u32(0), vdupq_n_u32(0)};

        int             w   = width;
        const uint16_t* row = src;
        while (w >= 32) {
            uint16x8_t s0 = vld1q_u16(row + 0);
            uint16x8_t s1 = vld1q_u16(row + 8);
            uint16x8_t s2 = vld1q_u16(row + 16);
            uint16x8_t s3 = vld1q_u16(row + 24);

            s0         = vaddq_u16(s0, s1);
            s2         = vaddq_u16(s2, s3);
            sum_u32[0] = vpadalq_u16(sum_u32[0], s0);
            sum_u32[1] = vpadalq_u16(sum_u32[1], s2);

            row += 32;
            w -= 32;
        }

        if (w >= 16) {
            uint16x8_t s0 = vld1q_u16(row + 0);
            uint16x8_t s1 = vld1q_u16(row + 8);

            s0         = vaddq_u16(s0, s1);
            sum_u32[0] = vpadalq_u16(sum_u32[0], s0);

            row += 16;
            w -= 16;
        }

        if (w >= 8) {
            uint16x8_t s0 = vld1q_u16(row);
            sum_u32[1]    = vpadalq_u16(sum_u32[1], s0);

            row += 8;
            w -= 8;
        }

        if (w) {
            uint16x8_t s0 = vandq_u16(vld1q_u16(row), mask);
            sum_u32[1]    = vpadalq_u16(sum_u32[1], s0);

            row += 8;
            w -= 8;
        }

        sum_u64 = vpadalq_u32(sum_u64, vaddq_u32(sum_u32[0], sum_u32[1]));

        src += src_stride;
    } while (--h != 0);

    return (uint16_t)((vaddvq_u64(sum_u64) + sum) / (height * width));
}

void svt_av1_compute_stats_highbd_neon(int32_t wiener_win, const uint8_t* dgd8, const uint8_t* src8, int32_t h_start,
                                       int32_t h_end, int32_t v_start, int32_t v_end, int32_t dgd_stride,
                                       int32_t src_stride, int64_t* M, int64_t* H, EbBitDepth bit_depth) {
    if (bit_depth == EB_TWELVE_BIT) {
        svt_av1_compute_stats_highbd_c(
            wiener_win, dgd8, src8, h_start, h_end, v_start, v_end, dgd_stride, src_stride, M, H, bit_depth);
        return;
    }

    const int32_t   wiener_win2    = wiener_win * wiener_win;
    const int32_t   wiener_halfwin = (wiener_win >> 1);
    const uint16_t* src            = CONVERT_TO_SHORTPTR(src8);
    const uint16_t* dgd            = CONVERT_TO_SHORTPTR(dgd8);
    const int32_t   width          = h_end - h_start;
    const int32_t   height         = v_end - v_start;
    const int32_t   d_stride       = (width + 2 * wiener_halfwin + 15) & ~15;
    const int32_t   s_stride       = (width + 15) & ~15;
    int16_t *       d, *s;

    const uint16_t* dgd_start = dgd + h_start + v_start * dgd_stride;
    const uint16_t* src_start = src + h_start + v_start * src_stride;
    const uint16_t  avg       = highbd_find_average_neon(dgd_start, dgd_stride, width, height);

    // The maximum input size is width * height, which is
    // (9 / 4) * RESTORATION_UNITSIZE_MAX * RESTORATION_UNITSIZE_MAX. Enlarge to
    // 3 * RESTORATION_UNITSIZE_MAX * RESTORATION_UNITSIZE_MAX considering
    // paddings.
    d = svt_aom_memalign(32, sizeof(*d) * 6 * RESTORATION_UNITSIZE_MAX * RESTORATION_UNITSIZE_MAX);
    s = d + 3 * RESTORATION_UNITSIZE_MAX * RESTORATION_UNITSIZE_MAX;

    sub_avg_block_highbd_neon(src_start, src_stride, avg, width, height, s, s_stride);
    sub_avg_block_highbd_neon(dgd + (v_start - wiener_halfwin) * dgd_stride + h_start - wiener_halfwin,
                              dgd_stride,
                              avg,
                              width + 2 * wiener_halfwin,
                              height + 2 * wiener_halfwin,
                              d,
                              d_stride);

    if (wiener_win == WIENER_WIN) {
        compute_stats_win7_highbd_neon(d, d_stride, s, s_stride, width, height, M, H, bit_depth);
    } else {
        assert(wiener_win == WIENER_WIN_CHROMA);
        compute_stats_win5_highbd_neon(d, d_stride, s, s_stride, width, height, M, H, bit_depth);
    }

    // H is a symmetric matrix, so we only need to fill out the upper triangle.
    // We can copy it down to the lower triangle outside the (i, j) loops.
    if (bit_depth == EB_EIGHT_BIT) {
        diagonal_copy_stats_neon(wiener_win2, H);
    } else { //bit_depth == EB_TEN_BIT
        const int32_t k4 = wiener_win2 & ~3;

        int32_t k = 0;
        do {
            int64x2_t dst = div4_neon(vld1q_s64(M + k));
            vst1q_s64(M + k, dst);
            dst = div4_neon(vld1q_s64(M + k + 2));
            vst1q_s64(M + k + 2, dst);
            H[k * wiener_win2 + k] /= 4;
            k += 4;
        } while (k < k4);

        H[k * wiener_win2 + k] /= 4;

        for (; k < wiener_win2; ++k) {
            M[k] /= 4;
        }

        div4_diagonal_copy_stats_neon(wiener_win2, H);
    }

    svt_aom_free(d);
}
#endif // CONFIG_ENABLE_HIGH_BIT_DEPTH
