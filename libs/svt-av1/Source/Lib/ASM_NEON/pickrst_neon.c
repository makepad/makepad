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
#include "pickrst_neon.h"
#include "restoration.h"
#include "restoration_pick.h"
#include "sum_neon.h"
#include "transpose_neon.h"
#include "utility.h"

static inline uint8_t find_average_neon(const uint8_t* src, int src_stride, int width, int height) {
    uint64_t sum = 0;

    if (width >= 16) {
        int h = 0;
        // We can accumulate up to 257 8-bit values in a 16-bit value, given
        // that each 16-bit vector has 8 elements, that means we can process up to
        // int(257*8/width) rows before we need to widen to 32-bit vector
        // elements.
        int        h_overflow = 257 * 8 / width;
        int        h_limit    = height > h_overflow ? h_overflow : height;
        uint32x4_t avg_u32    = vdupq_n_u32(0);
        do {
            uint16x8_t avg_u16 = vdupq_n_u16(0);
            do {
                int            j       = width;
                const uint8_t* src_ptr = src;
                do {
                    uint8x16_t s = vld1q_u8(src_ptr);
                    avg_u16      = vpadalq_u8(avg_u16, s);
                    j -= 16;
                    src_ptr += 16;
                } while (j >= 16);
                if (j >= 8) {
                    uint8x8_t s = vld1_u8(src_ptr);
                    avg_u16     = vaddw_u8(avg_u16, s);
                    j -= 8;
                    src_ptr += 8;
                }
                // Scalar tail case.
                while (j > 0) {
                    sum += src[width - j];
                    j--;
                }
                src += src_stride;
            } while (++h < h_limit);
            avg_u32 = vpadalq_u16(avg_u32, avg_u16);

            h_limit += h_overflow;
            h_limit = height > h_overflow ? h_overflow : height;
        } while (h < height);
        return (uint8_t)((vaddlvq_u32(avg_u32) + sum) / (width * height));
    }
    if (width >= 8) {
        int h = 0;
        // We can accumulate up to 257 8-bit values in a 16-bit value, given
        // that each 16-bit vector has 4 elements, that means we can process up to
        // int(257*4/width) rows before we need to widen to 32-bit vector
        // elements.
        int        h_overflow = 257 * 4 / width;
        int        h_limit    = height > h_overflow ? h_overflow : height;
        uint32x2_t avg_u32    = vdup_n_u32(0);
        do {
            uint16x4_t avg_u16 = vdup_n_u16(0);
            do {
                int            j       = width;
                const uint8_t* src_ptr = src;
                uint8x8_t      s       = vld1_u8(src_ptr);
                avg_u16                = vpadal_u8(avg_u16, s);
                j -= 8;
                src_ptr += 8;
                // Scalar tail case.
                while (j > 0) {
                    sum += src[width - j];
                    j--;
                }
                src += src_stride;
            } while (++h < h_limit);
            avg_u32 = vpadal_u16(avg_u32, avg_u16);

            h_limit += h_overflow;
            h_limit = height > h_overflow ? h_overflow : height;
        } while (h < height);
        return (uint8_t)((vaddlv_u32(avg_u32) + sum) / (width * height));
    }
    int i = height;
    do {
        int j = 0;
        do {
            sum += src[j];
        } while (++j < width);
        src += src_stride;
    } while (--i != 0);
    return (uint8_t)(sum / (width * height));
}

static inline void compute_sub_avg(const uint8_t* buf, int buf_stride, int avg, int16_t* buf_avg, int buf_avg_stride,
                                   int width, int height) {
    uint8x8_t avg_u8 = vdup_n_u8(avg);

    if (width > 8) {
        int i = 0;
        do {
            int            j           = width;
            const uint8_t* buf_ptr     = buf;
            int16_t*       buf_avg_ptr = buf_avg;
            do {
                uint8x8_t d = vld1_u8(buf_ptr);
                vst1q_s16(buf_avg_ptr, vreinterpretq_s16_u16(vsubl_u8(d, avg_u8)));

                j -= 8;
                buf_ptr += 8;
                buf_avg_ptr += 8;
            } while (j >= 8);
            while (j > 0) {
                *buf_avg_ptr = (int16_t)buf[width - j] - (int16_t)avg;
                buf_avg_ptr++;
                j--;
            }
            buf += buf_stride;
            buf_avg += buf_avg_stride;
        } while (++i < height);
    } else {
        // For width < 8, don't use Neon.
        for (int i = 0; i < height; i++) {
            for (int j = 0; j < width; j++) {
                buf_avg[j] = (int16_t)buf[j] - (int16_t)avg;
            }
            buf += buf_stride;
            buf_avg += buf_avg_stride;
        }
    }
}

static inline void compute_stats_win5_neon(const int16_t* const d, const int32_t d_stride, const int16_t* const s,
                                           const int32_t s_stride, const int32_t width, const int32_t height,
                                           int64_t* const M, int64_t* const H) {
    const int32_t     wiener_win  = WIENER_WIN_CHROMA;
    const int32_t     wiener_win2 = wiener_win * wiener_win;
    const int32_t     w16         = width & ~15;
    const int32_t     h8          = height & ~7;
    const int16x8x2_t mask        = vld1q_s16_x2(&(mask_16bit[16]) - width % 16);
    int32_t           i, j, x, y;

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

static inline void compute_stats_win7_neon(const int16_t* const d, const int32_t d_stride, const int16_t* const s,
                                           const int32_t s_stride, const int32_t width, const int32_t height,
                                           int64_t* const M, int64_t* const H) {
    const int32_t     wiener_win  = WIENER_WIN;
    const int32_t     wiener_win2 = wiener_win * wiener_win;
    const int32_t     w16         = width & ~15;
    const int32_t     h8          = height & ~7;
    const int16x8x2_t mask        = vld1q_s16_x2(&(mask_16bit[16]) - width % 16);
    int32_t           i, j, x, y;

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

void svt_av1_compute_stats_neon(int32_t wiener_win, const uint8_t* dgd, const uint8_t* src, int32_t h_start,
                                int32_t h_end, int32_t v_start, int32_t v_end, int32_t dgd_stride, int32_t src_stride,
                                int64_t* M, int64_t* H) {
    const int32_t wiener_win2    = wiener_win * wiener_win;
    const int32_t wiener_halfwin = (wiener_win >> 1);
    const int32_t width          = h_end - h_start;
    const int32_t height         = v_end - v_start;
    const int32_t d_stride       = (width + 2 * wiener_halfwin + 15) & ~15;
    const int32_t s_stride       = (width + 15) & ~15;
    int16_t *     d, *s;

    const uint8_t* dgd_start = dgd + h_start + v_start * dgd_stride;
    const uint8_t* src_start = src + h_start + v_start * src_stride;
    const uint16_t avg       = find_average_neon(dgd_start, dgd_stride, width, height);

    // The maximum input size is width * height, which is
    // (9 / 4) * RESTORATION_UNITSIZE_MAX * RESTORATION_UNITSIZE_MAX. Enlarge to
    // 3 * RESTORATION_UNITSIZE_MAX * RESTORATION_UNITSIZE_MAX considering
    // paddings.
    d = svt_aom_memalign(32, sizeof(*d) * 6 * RESTORATION_UNITSIZE_MAX * RESTORATION_UNITSIZE_MAX);
    s = d + 3 * RESTORATION_UNITSIZE_MAX * RESTORATION_UNITSIZE_MAX;

    compute_sub_avg(src_start, src_stride, avg, s, s_stride, width, height);
    compute_sub_avg(dgd + (v_start - wiener_halfwin) * dgd_stride + h_start - wiener_halfwin,
                    dgd_stride,
                    avg,
                    d,
                    d_stride,
                    width + 2 * wiener_halfwin,
                    height + 2 * wiener_halfwin);

    if (wiener_win == WIENER_WIN) {
        compute_stats_win7_neon(d, d_stride, s, s_stride, width, height, M, H);
    } else {
        assert(wiener_win == WIENER_WIN_CHROMA);
        compute_stats_win5_neon(d, d_stride, s, s_stride, width, height, M, H);
    }

    // H is a symmetric matrix, so we only need to fill out the upper triangle.
    // We can copy it down to the lower triangle outside the (i, j) loops.
    diagonal_copy_stats_neon(wiener_win2, H);

    svt_aom_free(d);
}

int64_t svt_av1_lowbd_pixel_proj_error_neon(const uint8_t* src8, int32_t width, int32_t height, int32_t src_stride,
                                            const uint8_t* dat8, int32_t dat_stride, int32_t* flt0, int32_t flt0_stride,
                                            int32_t* flt1, int32_t flt1_stride, const int32_t xq[2],
                                            const SgrParamsType* params) {
    if (width % 16 != 0) {
        return svt_av1_lowbd_pixel_proj_error_c(
            src8, width, height, src_stride, dat8, dat_stride, flt0, flt0_stride, flt1, flt1_stride, xq, params);
    }

    int64x2_t sse_s64 = vdupq_n_s64(0);

    if (params->r[0] > 0 && params->r[1] > 0) {
        int32x2_t xq_v     = vld1_s32(xq);
        int16x4_t xq_sum_v = vreinterpret_s16_s32(vshl_n_s32(vpadd_s32(xq_v, xq_v), SGRPROJ_RST_BITS));

        do {
            int       j       = 0;
            int32x4_t sse_s32 = vdupq_n_s32(0);

            do {
                const uint8x8_t d      = vld1_u8(&dat8[j]);
                const uint8x8_t s      = vld1_u8(&src8[j]);
                int32x4_t       flt0_0 = vld1q_s32(&flt0[j]);
                int32x4_t       flt0_1 = vld1q_s32(&flt0[j + 4]);
                int32x4_t       flt1_0 = vld1q_s32(&flt1[j]);
                int32x4_t       flt1_1 = vld1q_s32(&flt1[j + 4]);

                int32x4_t offset = vdupq_n_s32(1 << (SGRPROJ_RST_BITS + SGRPROJ_PRJ_BITS - 1));
                int32x4_t v0     = vmlaq_lane_s32(offset, flt0_0, xq_v, 0);
                int32x4_t v1     = vmlaq_lane_s32(offset, flt0_1, xq_v, 0);

                v0 = vmlaq_lane_s32(v0, flt1_0, xq_v, 1);
                v1 = vmlaq_lane_s32(v1, flt1_1, xq_v, 1);

                int16x8_t d_s16 = vreinterpretq_s16_u16(vmovl_u8(d));
                v0              = vmlsl_lane_s16(v0, vget_low_s16(d_s16), xq_sum_v, 0);
                v1              = vmlsl_lane_s16(v1, vget_high_s16(d_s16), xq_sum_v, 0);

                int16x4_t vr0 = vshrn_n_s32(v0, SGRPROJ_RST_BITS + SGRPROJ_PRJ_BITS);
                int16x4_t vr1 = vshrn_n_s32(v1, SGRPROJ_RST_BITS + SGRPROJ_PRJ_BITS);

                int16x8_t diff = vreinterpretq_s16_u16(vsubl_u8(d, s));
                int16x8_t e    = vaddq_s16(vcombine_s16(vr0, vr1), diff);

                sse_s32 = vmlal_s16(sse_s32, vget_low_s16(e), vget_low_s16(e));
                sse_s32 = vmlal_s16(sse_s32, vget_high_s16(e), vget_high_s16(e));

                j += 8;
            } while (j != width);

            sse_s64 = vpadalq_s32(sse_s64, sse_s32);

            dat8 += dat_stride;
            src8 += src_stride;
            flt0 += flt0_stride;
            flt1 += flt1_stride;
        } while (--height != 0);
    } else if (params->r[0] > 0 || params->r[1] > 0) {
        const int32_t  xq_active  = (params->r[0] > 0) ? xq[0] : xq[1];
        const int32_t* flt        = (params->r[0] > 0) ? flt0 : flt1;
        const int32_t  flt_stride = (params->r[0] > 0) ? flt0_stride : flt1_stride;
        int32x2_t      xq_v       = vdup_n_s32(xq_active);

        do {
            int32x4_t sse_s32 = vdupq_n_s32(0);
            int       j       = 0;

            do {
                const uint8x8_t d     = vld1_u8(&dat8[j]);
                const uint8x8_t s     = vld1_u8(&src8[j]);
                int32x4_t       flt_0 = vld1q_s32(&flt[j]);
                int32x4_t       flt_1 = vld1q_s32(&flt[j + 4]);
                int16x8_t       d_s16 = vreinterpretq_s16_u16(vshll_n_u8(d, SGRPROJ_RST_BITS));

                int32x4_t sub_0 = vsubw_s16(flt_0, vget_low_s16(d_s16));
                int32x4_t sub_1 = vsubw_s16(flt_1, vget_high_s16(d_s16));

                int32x4_t offset = vdupq_n_s32(1 << (SGRPROJ_RST_BITS + SGRPROJ_PRJ_BITS - 1));
                int32x4_t v0     = vmlaq_lane_s32(offset, sub_0, xq_v, 0);
                int32x4_t v1     = vmlaq_lane_s32(offset, sub_1, xq_v, 0);

                int16x4_t vr0 = vshrn_n_s32(v0, SGRPROJ_RST_BITS + SGRPROJ_PRJ_BITS);
                int16x4_t vr1 = vshrn_n_s32(v1, SGRPROJ_RST_BITS + SGRPROJ_PRJ_BITS);

                int16x8_t diff = vreinterpretq_s16_u16(vsubl_u8(d, s));
                int16x8_t e    = vaddq_s16(vcombine_s16(vr0, vr1), diff);

                sse_s32 = vmlal_s16(sse_s32, vget_low_s16(e), vget_low_s16(e));
                sse_s32 = vmlal_s16(sse_s32, vget_high_s16(e), vget_high_s16(e));

                j += 8;
            } while (j != width);

            sse_s64 = vpadalq_s32(sse_s64, sse_s32);

            dat8 += dat_stride;
            src8 += src_stride;
            flt += flt_stride;
        } while (--height != 0);
    } else {
        uint32x4_t sse_s32 = vdupq_n_u32(0);

        do {
            int j = 0;

            do {
                const uint8x16_t d = vld1q_u8(&dat8[j]);
                const uint8x16_t s = vld1q_u8(&src8[j]);

                uint8x16_t diff    = vabdq_u8(d, s);
                uint8x8_t  diff_lo = vget_low_u8(diff);
                uint8x8_t  diff_hi = vget_high_u8(diff);

                sse_s32 = vpadalq_u16(sse_s32, vmull_u8(diff_lo, diff_lo));
                sse_s32 = vpadalq_u16(sse_s32, vmull_u8(diff_hi, diff_hi));

                j += 16;
            } while (j != width);

            dat8 += dat_stride;
            src8 += src_stride;
        } while (--height != 0);

        sse_s64 = vreinterpretq_s64_u64(vpaddlq_u32(sse_s32));
    }

    return vaddvq_s64(sse_s64);
}

#if CONFIG_ENABLE_HIGH_BIT_DEPTH
int64_t svt_av1_highbd_pixel_proj_error_neon(const uint8_t* src8, int32_t width, int32_t height, int32_t src_stride,
                                             const uint8_t* dat8, int32_t dat_stride, int32_t* flt0,
                                             int32_t flt0_stride, int32_t* flt1, int32_t flt1_stride,
                                             const int32_t xq[2], const SgrParamsType* params) {
    if (width % 8 != 0) {
        return svt_av1_highbd_pixel_proj_error_c(
            src8, width, height, src_stride, dat8, dat_stride, flt0, flt0_stride, flt1, flt1_stride, xq, params);
    }
    const uint16_t* src     = CONVERT_TO_SHORTPTR(src8);
    const uint16_t* dat     = CONVERT_TO_SHORTPTR(dat8);
    int64x2_t       sse_s64 = vdupq_n_s64(0);

    if (params->r[0] > 0 && params->r[1] > 0) {
        int32x2_t  xq_v     = vld1_s32(xq);
        uint16x4_t xq_sum_v = vreinterpret_u16_s32(vshl_n_s32(vpadd_s32(xq_v, xq_v), 4));

        do {
            int       j       = 0;
            int32x4_t sse_s32 = vdupq_n_s32(0);

            do {
                const uint16x8_t d      = vld1q_u16(&dat[j]);
                const uint16x8_t s      = vld1q_u16(&src[j]);
                int32x4_t        flt0_0 = vld1q_s32(&flt0[j]);
                int32x4_t        flt0_1 = vld1q_s32(&flt0[j + 4]);
                int32x4_t        flt1_0 = vld1q_s32(&flt1[j]);
                int32x4_t        flt1_1 = vld1q_s32(&flt1[j + 4]);

                int32x4_t d_s32_lo = vreinterpretq_s32_u32(vmull_lane_u16(vget_low_u16(d), xq_sum_v, 0));
                int32x4_t d_s32_hi = vreinterpretq_s32_u32(vmull_lane_u16(vget_high_u16(d), xq_sum_v, 0));

                int32x4_t v0 = vsubq_s32(vdupq_n_s32(1 << (SGRPROJ_RST_BITS + SGRPROJ_PRJ_BITS - 1)), d_s32_lo);
                int32x4_t v1 = vsubq_s32(vdupq_n_s32(1 << (SGRPROJ_RST_BITS + SGRPROJ_PRJ_BITS - 1)), d_s32_hi);

                v0 = vmlaq_lane_s32(v0, flt0_0, xq_v, 0);
                v1 = vmlaq_lane_s32(v1, flt0_1, xq_v, 0);
                v0 = vmlaq_lane_s32(v0, flt1_0, xq_v, 1);
                v1 = vmlaq_lane_s32(v1, flt1_1, xq_v, 1);

                int16x4_t vr0 = vshrn_n_s32(v0, SGRPROJ_RST_BITS + SGRPROJ_PRJ_BITS);
                int16x4_t vr1 = vshrn_n_s32(v1, SGRPROJ_RST_BITS + SGRPROJ_PRJ_BITS);

                int16x8_t e = vaddq_s16(vcombine_s16(vr0, vr1), vreinterpretq_s16_u16(vsubq_u16(d, s)));

                sse_s32 = vmlal_s16(sse_s32, vget_low_s16(e), vget_low_s16(e));
                sse_s32 = vmlal_s16(sse_s32, vget_high_s16(e), vget_high_s16(e));

                j += 8;
            } while (j != width);

            sse_s64 = vpadalq_s32(sse_s64, sse_s32);

            dat += dat_stride;
            src += src_stride;
            flt0 += flt0_stride;
            flt1 += flt1_stride;
        } while (--height != 0);
    } else if (params->r[0] > 0 || params->r[1] > 0) {
        int       xq_active  = (params->r[0] > 0) ? xq[0] : xq[1];
        int32_t*  flt        = (params->r[0] > 0) ? flt0 : flt1;
        int       flt_stride = (params->r[0] > 0) ? flt0_stride : flt1_stride;
        int32x4_t xq_v       = vdupq_n_s32(xq_active);

        do {
            int       j       = 0;
            int32x4_t sse_s32 = vdupq_n_s32(0);

            do {
                const uint16x8_t d0     = vld1q_u16(&dat[j]);
                const uint16x8_t s0     = vld1q_u16(&src[j]);
                int32x4_t        flt0_0 = vld1q_s32(&flt[j]);
                int32x4_t        flt0_1 = vld1q_s32(&flt[j + 4]);

                uint16x8_t d_u16 = vshlq_n_u16(d0, 4);
                int32x4_t  sub0  = vreinterpretq_s32_u32(vsubw_u16(vreinterpretq_u32_s32(flt0_0), vget_low_u16(d_u16)));
                int32x4_t  sub1 = vreinterpretq_s32_u32(vsubw_u16(vreinterpretq_u32_s32(flt0_1), vget_high_u16(d_u16)));

                int32x4_t v0 = vmlaq_s32(vdupq_n_s32(1 << (SGRPROJ_RST_BITS + SGRPROJ_PRJ_BITS - 1)), sub0, xq_v);
                int32x4_t v1 = vmlaq_s32(vdupq_n_s32(1 << (SGRPROJ_RST_BITS + SGRPROJ_PRJ_BITS - 1)), sub1, xq_v);

                int16x4_t vr0 = vshrn_n_s32(v0, SGRPROJ_RST_BITS + SGRPROJ_PRJ_BITS);
                int16x4_t vr1 = vshrn_n_s32(v1, SGRPROJ_RST_BITS + SGRPROJ_PRJ_BITS);

                int16x8_t e = vaddq_s16(vcombine_s16(vr0, vr1), vreinterpretq_s16_u16(vsubq_u16(d0, s0)));

                sse_s32 = vmlal_s16(sse_s32, vget_low_s16(e), vget_low_s16(e));
                sse_s32 = vmlal_s16(sse_s32, vget_high_s16(e), vget_high_s16(e));

                j += 8;
            } while (j != width);

            sse_s64 = vpadalq_s32(sse_s64, sse_s32);

            dat += dat_stride;
            flt += flt_stride;
            src += src_stride;
        } while (--height != 0);
    } else {
        do {
            int j = 0;

            do {
                const uint16x8_t d = vld1q_u16(&dat[j]);
                const uint16x8_t s = vld1q_u16(&src[j]);

                uint16x8_t diff    = vabdq_u16(d, s);
                uint16x4_t diff_lo = vget_low_u16(diff);
                uint16x4_t diff_hi = vget_high_u16(diff);

                uint32x4_t sqr_lo = vmull_u16(diff_lo, diff_lo);
                uint32x4_t sqr_hi = vmull_u16(diff_hi, diff_hi);

                sse_s64 = vpadalq_s32(sse_s64, vreinterpretq_s32_u32(sqr_lo));
                sse_s64 = vpadalq_s32(sse_s64, vreinterpretq_s32_u32(sqr_hi));

                j += 8;
            } while (j != width);

            dat += dat_stride;
            src += src_stride;
        } while (--height != 0);
    }

    return vaddvq_s64(sse_s64);
}
#endif
