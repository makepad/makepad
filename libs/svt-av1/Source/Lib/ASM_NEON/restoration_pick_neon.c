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
#include "restoration.h"

static inline void svt_calc_proj_params_r0_r1_lowbd_neon(const uint8_t* src8, int32_t width, int32_t height,
                                                         int32_t src_stride, const uint8_t* dat8, int32_t dat_stride,
                                                         int32_t* flt0, int32_t flt0_stride, int32_t* flt1,
                                                         int32_t flt1_stride, double H[2][2], double C[2]) {
    const int32_t size   = width * height;
    int64x2_t     h00_lo = vdupq_n_s64(0);
    int64x2_t     h00_hi = vdupq_n_s64(0);
    int64x2_t     h11_lo = vdupq_n_s64(0);
    int64x2_t     h11_hi = vdupq_n_s64(0);
    int64x2_t     h01_lo = vdupq_n_s64(0);
    int64x2_t     h01_hi = vdupq_n_s64(0);
    int64x2_t     c0_lo  = vdupq_n_s64(0);
    int64x2_t     c0_hi  = vdupq_n_s64(0);
    int64x2_t     c1_lo  = vdupq_n_s64(0);
    int64x2_t     c1_hi  = vdupq_n_s64(0);

    assert(width % 8 == 0);
    do {
        const uint8_t* src_ptr  = src8;
        const uint8_t* dat_ptr  = dat8;
        int32_t*       flt0_ptr = flt0;
        int32_t*       flt1_ptr = flt1;
        int            w        = width;
        do {
            uint8x8_t u     = vld1_u8(dat_ptr);
            uint8x8_t s     = vld1_u8(src_ptr);
            int32x4_t f0_lo = vld1q_s32(flt0_ptr);
            int32x4_t f0_hi = vld1q_s32(flt0_ptr + 4);
            int32x4_t f1_lo = vld1q_s32(flt1_ptr);
            int32x4_t f1_hi = vld1q_s32(flt1_ptr + 4);

            int16x8_t u_s16 = vreinterpretq_s16_u16(vshll_n_u8(u, SGRPROJ_RST_BITS));
            int16x8_t s_s16 = vreinterpretq_s16_u16(vshll_n_u8(s, SGRPROJ_RST_BITS));

            int32x4_t s_lo = vsubl_s16(vget_low_s16(s_s16), vget_low_s16(u_s16));
            int32x4_t s_hi = vsubl_s16(vget_high_s16(s_s16), vget_high_s16(u_s16));
            f0_lo          = vsubw_s16(f0_lo, vget_low_s16(u_s16));
            f0_hi          = vsubw_s16(f0_hi, vget_high_s16(u_s16));
            f1_lo          = vsubw_s16(f1_lo, vget_low_s16(u_s16));
            f1_hi          = vsubw_s16(f1_hi, vget_high_s16(u_s16));

            h00_lo = vmlal_s32(h00_lo, vget_low_s32(f0_lo), vget_low_s32(f0_lo));
            h00_lo = vmlal_s32(h00_lo, vget_high_s32(f0_lo), vget_high_s32(f0_lo));
            h00_hi = vmlal_s32(h00_hi, vget_low_s32(f0_hi), vget_low_s32(f0_hi));
            h00_hi = vmlal_s32(h00_hi, vget_high_s32(f0_hi), vget_high_s32(f0_hi));

            h11_lo = vmlal_s32(h11_lo, vget_low_s32(f1_lo), vget_low_s32(f1_lo));
            h11_lo = vmlal_s32(h11_lo, vget_high_s32(f1_lo), vget_high_s32(f1_lo));
            h11_hi = vmlal_s32(h11_hi, vget_low_s32(f1_hi), vget_low_s32(f1_hi));
            h11_hi = vmlal_s32(h11_hi, vget_high_s32(f1_hi), vget_high_s32(f1_hi));

            h01_lo = vmlal_s32(h01_lo, vget_low_s32(f0_lo), vget_low_s32(f1_lo));
            h01_lo = vmlal_s32(h01_lo, vget_high_s32(f0_lo), vget_high_s32(f1_lo));
            h01_hi = vmlal_s32(h01_hi, vget_low_s32(f0_hi), vget_low_s32(f1_hi));
            h01_hi = vmlal_s32(h01_hi, vget_high_s32(f0_hi), vget_high_s32(f1_hi));

            c0_lo = vmlal_s32(c0_lo, vget_low_s32(f0_lo), vget_low_s32(s_lo));
            c0_lo = vmlal_s32(c0_lo, vget_high_s32(f0_lo), vget_high_s32(s_lo));
            c0_hi = vmlal_s32(c0_hi, vget_low_s32(f0_hi), vget_low_s32(s_hi));
            c0_hi = vmlal_s32(c0_hi, vget_high_s32(f0_hi), vget_high_s32(s_hi));

            c1_lo = vmlal_s32(c1_lo, vget_low_s32(f1_lo), vget_low_s32(s_lo));
            c1_lo = vmlal_s32(c1_lo, vget_high_s32(f1_lo), vget_high_s32(s_lo));
            c1_hi = vmlal_s32(c1_hi, vget_low_s32(f1_hi), vget_low_s32(s_hi));
            c1_hi = vmlal_s32(c1_hi, vget_high_s32(f1_hi), vget_high_s32(s_hi));

            src_ptr += 8;
            dat_ptr += 8;
            flt0_ptr += 8;
            flt1_ptr += 8;
            w -= 8;
        } while (w != 0);

        src8 += src_stride;
        dat8 += dat_stride;
        flt0 += flt0_stride;
        flt1 += flt1_stride;
    } while (--height != 0);

    H[0][0] = (double)vaddvq_s64(vaddq_s64(h00_lo, h00_hi));
    H[0][1] = (double)vaddvq_s64(vaddq_s64(h01_lo, h01_hi));
    H[1][1] = (double)vaddvq_s64(vaddq_s64(h11_lo, h11_hi));
    C[0]    = (double)vaddvq_s64(vaddq_s64(c0_lo, c0_hi));
    C[1]    = (double)vaddvq_s64(vaddq_s64(c1_lo, c1_hi));

    H[0][0] /= size;
    H[0][1] /= size;
    H[1][1] /= size;
    H[1][0] = H[0][1];
    C[0] /= size;
    C[1] /= size;
}

static inline void svt_calc_proj_params_r0_r1_highbd_neon(const uint8_t* src8, int32_t width, int32_t height,
                                                          int32_t src_stride, const uint8_t* dat8, int32_t dat_stride,
                                                          int32_t* flt0, int32_t flt0_stride, int32_t* flt1,
                                                          int32_t flt1_stride, double H[2][2], double C[2]) {
    const int32_t   size   = width * height;
    const uint16_t* src    = CONVERT_TO_SHORTPTR(src8);
    const uint16_t* dat    = CONVERT_TO_SHORTPTR(dat8);
    int64x2_t       h00_lo = vdupq_n_s64(0);
    int64x2_t       h00_hi = vdupq_n_s64(0);
    int64x2_t       h11_lo = vdupq_n_s64(0);
    int64x2_t       h11_hi = vdupq_n_s64(0);
    int64x2_t       h01_lo = vdupq_n_s64(0);
    int64x2_t       h01_hi = vdupq_n_s64(0);
    int64x2_t       c0_lo  = vdupq_n_s64(0);
    int64x2_t       c0_hi  = vdupq_n_s64(0);
    int64x2_t       c1_lo  = vdupq_n_s64(0);
    int64x2_t       c1_hi  = vdupq_n_s64(0);

    assert(width % 8 == 0);
    do {
        const uint16_t* src_ptr  = src;
        const uint16_t* dat_ptr  = dat;
        int32_t*        flt0_ptr = flt0;
        int32_t*        flt1_ptr = flt1;
        int             w        = width;
        do {
            uint16x8_t u     = vld1q_u16(dat_ptr);
            uint16x8_t s     = vld1q_u16(src_ptr);
            int32x4_t  f0_lo = vld1q_s32(flt0_ptr);
            int32x4_t  f0_hi = vld1q_s32(flt0_ptr + 4);
            int32x4_t  f1_lo = vld1q_s32(flt1_ptr);
            int32x4_t  f1_hi = vld1q_s32(flt1_ptr + 4);

            int32x4_t u_lo = vreinterpretq_s32_u32(vshll_n_u16(vget_low_u16(u), SGRPROJ_RST_BITS));
            int32x4_t u_hi = vreinterpretq_s32_u32(vshll_n_u16(vget_high_u16(u), SGRPROJ_RST_BITS));
            int32x4_t s_lo = vreinterpretq_s32_u32(vshll_n_u16(vget_low_u16(s), SGRPROJ_RST_BITS));
            int32x4_t s_hi = vreinterpretq_s32_u32(vshll_n_u16(vget_high_u16(s), SGRPROJ_RST_BITS));

            s_lo  = vsubq_s32(s_lo, u_lo);
            s_hi  = vsubq_s32(s_hi, u_hi);
            f0_lo = vsubq_s32(f0_lo, u_lo);
            f0_hi = vsubq_s32(f0_hi, u_hi);
            f1_lo = vsubq_s32(f1_lo, u_lo);
            f1_hi = vsubq_s32(f1_hi, u_hi);

            h00_lo = vmlal_s32(h00_lo, vget_low_s32(f0_lo), vget_low_s32(f0_lo));
            h00_lo = vmlal_s32(h00_lo, vget_high_s32(f0_lo), vget_high_s32(f0_lo));
            h00_hi = vmlal_s32(h00_hi, vget_low_s32(f0_hi), vget_low_s32(f0_hi));
            h00_hi = vmlal_s32(h00_hi, vget_high_s32(f0_hi), vget_high_s32(f0_hi));

            h11_lo = vmlal_s32(h11_lo, vget_low_s32(f1_lo), vget_low_s32(f1_lo));
            h11_lo = vmlal_s32(h11_lo, vget_high_s32(f1_lo), vget_high_s32(f1_lo));
            h11_hi = vmlal_s32(h11_hi, vget_low_s32(f1_hi), vget_low_s32(f1_hi));
            h11_hi = vmlal_s32(h11_hi, vget_high_s32(f1_hi), vget_high_s32(f1_hi));

            h01_lo = vmlal_s32(h01_lo, vget_low_s32(f0_lo), vget_low_s32(f1_lo));
            h01_lo = vmlal_s32(h01_lo, vget_high_s32(f0_lo), vget_high_s32(f1_lo));
            h01_hi = vmlal_s32(h01_hi, vget_low_s32(f0_hi), vget_low_s32(f1_hi));
            h01_hi = vmlal_s32(h01_hi, vget_high_s32(f0_hi), vget_high_s32(f1_hi));

            c0_lo = vmlal_s32(c0_lo, vget_low_s32(f0_lo), vget_low_s32(s_lo));
            c0_lo = vmlal_s32(c0_lo, vget_high_s32(f0_lo), vget_high_s32(s_lo));
            c0_hi = vmlal_s32(c0_hi, vget_low_s32(f0_hi), vget_low_s32(s_hi));
            c0_hi = vmlal_s32(c0_hi, vget_high_s32(f0_hi), vget_high_s32(s_hi));

            c1_lo = vmlal_s32(c1_lo, vget_low_s32(f1_lo), vget_low_s32(s_lo));
            c1_lo = vmlal_s32(c1_lo, vget_high_s32(f1_lo), vget_high_s32(s_lo));
            c1_hi = vmlal_s32(c1_hi, vget_low_s32(f1_hi), vget_low_s32(s_hi));
            c1_hi = vmlal_s32(c1_hi, vget_high_s32(f1_hi), vget_high_s32(s_hi));

            src_ptr += 8;
            dat_ptr += 8;
            flt0_ptr += 8;
            flt1_ptr += 8;
            w -= 8;
        } while (w != 0);

        src += src_stride;
        dat += dat_stride;
        flt0 += flt0_stride;
        flt1 += flt1_stride;
    } while (--height != 0);

    H[0][0] = (double)vaddvq_s64(vaddq_s64(h00_lo, h00_hi));
    H[0][1] = (double)vaddvq_s64(vaddq_s64(h01_lo, h01_hi));
    H[1][1] = (double)vaddvq_s64(vaddq_s64(h11_lo, h11_hi));
    C[0]    = (double)vaddvq_s64(vaddq_s64(c0_lo, c0_hi));
    C[1]    = (double)vaddvq_s64(vaddq_s64(c1_lo, c1_hi));

    H[0][0] /= size;
    H[0][1] /= size;
    H[1][1] /= size;
    H[1][0] = H[0][1];
    C[0] /= size;
    C[1] /= size;
}

static inline void svt_calc_proj_params_r0_lowbd_neon(const uint8_t* src8, int32_t width, int32_t height,
                                                      int32_t src_stride, const uint8_t* dat8, int32_t dat_stride,
                                                      int32_t* flt0, int32_t flt0_stride, double H[2][2], double C[2]) {
    const int32_t size   = width * height;
    int64x2_t     h00_lo = vdupq_n_s64(0);
    int64x2_t     h00_hi = vdupq_n_s64(0);
    int64x2_t     c0_lo  = vdupq_n_s64(0);
    int64x2_t     c0_hi  = vdupq_n_s64(0);

    assert(width % 8 == 0);
    do {
        const uint8_t* src_ptr  = src8;
        const uint8_t* dat_ptr  = dat8;
        int32_t*       flt0_ptr = flt0;
        int            w        = width;
        do {
            uint8x8_t u     = vld1_u8(dat_ptr);
            uint8x8_t s     = vld1_u8(src_ptr);
            int32x4_t f0_lo = vld1q_s32(flt0_ptr);
            int32x4_t f0_hi = vld1q_s32(flt0_ptr + 4);

            int16x8_t u_s16 = vreinterpretq_s16_u16(vshll_n_u8(u, SGRPROJ_RST_BITS));
            int16x8_t s_s16 = vreinterpretq_s16_u16(vshll_n_u8(s, SGRPROJ_RST_BITS));

            int32x4_t s_lo = vsubl_s16(vget_low_s16(s_s16), vget_low_s16(u_s16));
            int32x4_t s_hi = vsubl_s16(vget_high_s16(s_s16), vget_high_s16(u_s16));
            f0_lo          = vsubw_s16(f0_lo, vget_low_s16(u_s16));
            f0_hi          = vsubw_s16(f0_hi, vget_high_s16(u_s16));

            h00_lo = vmlal_s32(h00_lo, vget_low_s32(f0_lo), vget_low_s32(f0_lo));
            h00_lo = vmlal_s32(h00_lo, vget_high_s32(f0_lo), vget_high_s32(f0_lo));
            h00_hi = vmlal_s32(h00_hi, vget_low_s32(f0_hi), vget_low_s32(f0_hi));
            h00_hi = vmlal_s32(h00_hi, vget_high_s32(f0_hi), vget_high_s32(f0_hi));

            c0_lo = vmlal_s32(c0_lo, vget_low_s32(f0_lo), vget_low_s32(s_lo));
            c0_lo = vmlal_s32(c0_lo, vget_high_s32(f0_lo), vget_high_s32(s_lo));
            c0_hi = vmlal_s32(c0_hi, vget_low_s32(f0_hi), vget_low_s32(s_hi));
            c0_hi = vmlal_s32(c0_hi, vget_high_s32(f0_hi), vget_high_s32(s_hi));

            src_ptr += 8;
            dat_ptr += 8;
            flt0_ptr += 8;
            w -= 8;
        } while (w != 0);

        src8 += src_stride;
        dat8 += dat_stride;
        flt0 += flt0_stride;
    } while (--height != 0);

    H[0][0] = (double)vaddvq_s64(vaddq_s64(h00_lo, h00_hi));
    C[0]    = (double)vaddvq_s64(vaddq_s64(c0_lo, c0_hi));

    H[0][0] /= size;
    C[0] /= size;
}

static inline void svt_calc_proj_params_r0_highbd_neon(const uint8_t* src8, int32_t width, int32_t height,
                                                       int32_t src_stride, const uint8_t* dat8, int32_t dat_stride,
                                                       int32_t* flt0, int32_t flt0_stride, double H[2][2],
                                                       double C[2]) {
    const int32_t   size   = width * height;
    const uint16_t* src    = CONVERT_TO_SHORTPTR(src8);
    const uint16_t* dat    = CONVERT_TO_SHORTPTR(dat8);
    int64x2_t       h00_lo = vdupq_n_s64(0);
    int64x2_t       h00_hi = vdupq_n_s64(0);
    int64x2_t       c0_lo  = vdupq_n_s64(0);
    int64x2_t       c0_hi  = vdupq_n_s64(0);

    assert(width % 8 == 0);
    do {
        const uint16_t* src_ptr  = src;
        const uint16_t* dat_ptr  = dat;
        int32_t*        flt0_ptr = flt0;
        int             w        = width;
        do {
            uint16x8_t u     = vld1q_u16(dat_ptr);
            uint16x8_t s     = vld1q_u16(src_ptr);
            int32x4_t  f0_lo = vld1q_s32(flt0_ptr);
            int32x4_t  f0_hi = vld1q_s32(flt0_ptr + 4);

            int32x4_t u_lo = vreinterpretq_s32_u32(vshll_n_u16(vget_low_u16(u), SGRPROJ_RST_BITS));
            int32x4_t u_hi = vreinterpretq_s32_u32(vshll_n_u16(vget_high_u16(u), SGRPROJ_RST_BITS));
            int32x4_t s_lo = vreinterpretq_s32_u32(vshll_n_u16(vget_low_u16(s), SGRPROJ_RST_BITS));
            int32x4_t s_hi = vreinterpretq_s32_u32(vshll_n_u16(vget_high_u16(s), SGRPROJ_RST_BITS));

            s_lo  = vsubq_s32(s_lo, u_lo);
            s_hi  = vsubq_s32(s_hi, u_hi);
            f0_lo = vsubq_s32(f0_lo, u_lo);
            f0_hi = vsubq_s32(f0_hi, u_hi);

            h00_lo = vmlal_s32(h00_lo, vget_low_s32(f0_lo), vget_low_s32(f0_lo));
            h00_lo = vmlal_s32(h00_lo, vget_high_s32(f0_lo), vget_high_s32(f0_lo));
            h00_hi = vmlal_s32(h00_hi, vget_low_s32(f0_hi), vget_low_s32(f0_hi));
            h00_hi = vmlal_s32(h00_hi, vget_high_s32(f0_hi), vget_high_s32(f0_hi));

            c0_lo = vmlal_s32(c0_lo, vget_low_s32(f0_lo), vget_low_s32(s_lo));
            c0_lo = vmlal_s32(c0_lo, vget_high_s32(f0_lo), vget_high_s32(s_lo));
            c0_hi = vmlal_s32(c0_hi, vget_low_s32(f0_hi), vget_low_s32(s_hi));
            c0_hi = vmlal_s32(c0_hi, vget_high_s32(f0_hi), vget_high_s32(s_hi));

            src_ptr += 8;
            dat_ptr += 8;
            flt0_ptr += 8;
            w -= 8;
        } while (w != 0);

        src += src_stride;
        dat += dat_stride;
        flt0 += flt0_stride;
    } while (--height != 0);

    H[0][0] = (double)vaddvq_s64(vaddq_s64(h00_lo, h00_hi));
    C[0]    = (double)vaddvq_s64(vaddq_s64(c0_lo, c0_hi));

    H[0][0] /= size;
    C[0] /= size;
}

static inline void svt_calc_proj_params_r1_lowbd_neon(const uint8_t* src8, int32_t width, int32_t height,
                                                      int32_t src_stride, const uint8_t* dat8, int32_t dat_stride,
                                                      int32_t* flt1, int32_t flt1_stride, double H[2][2], double C[2]) {
    const int32_t size   = width * height;
    int64x2_t     h11_lo = vdupq_n_s64(0);
    int64x2_t     h11_hi = vdupq_n_s64(0);
    int64x2_t     c1_lo  = vdupq_n_s64(0);
    int64x2_t     c1_hi  = vdupq_n_s64(0);

    assert(width % 8 == 0);
    do {
        const uint8_t* src_ptr  = src8;
        const uint8_t* dat_ptr  = dat8;
        int32_t*       flt1_ptr = flt1;
        int            w        = width;
        do {
            uint8x8_t u     = vld1_u8(dat_ptr);
            uint8x8_t s     = vld1_u8(src_ptr);
            int32x4_t f1_lo = vld1q_s32(flt1_ptr);
            int32x4_t f1_hi = vld1q_s32(flt1_ptr + 4);

            int16x8_t u_s16 = vreinterpretq_s16_u16(vshll_n_u8(u, SGRPROJ_RST_BITS));
            int16x8_t s_s16 = vreinterpretq_s16_u16(vshll_n_u8(s, SGRPROJ_RST_BITS));

            int32x4_t s_lo = vsubl_s16(vget_low_s16(s_s16), vget_low_s16(u_s16));
            int32x4_t s_hi = vsubl_s16(vget_high_s16(s_s16), vget_high_s16(u_s16));
            f1_lo          = vsubw_s16(f1_lo, vget_low_s16(u_s16));
            f1_hi          = vsubw_s16(f1_hi, vget_high_s16(u_s16));

            h11_lo = vmlal_s32(h11_lo, vget_low_s32(f1_lo), vget_low_s32(f1_lo));
            h11_lo = vmlal_s32(h11_lo, vget_high_s32(f1_lo), vget_high_s32(f1_lo));
            h11_hi = vmlal_s32(h11_hi, vget_low_s32(f1_hi), vget_low_s32(f1_hi));
            h11_hi = vmlal_s32(h11_hi, vget_high_s32(f1_hi), vget_high_s32(f1_hi));

            c1_lo = vmlal_s32(c1_lo, vget_low_s32(f1_lo), vget_low_s32(s_lo));
            c1_lo = vmlal_s32(c1_lo, vget_high_s32(f1_lo), vget_high_s32(s_lo));
            c1_hi = vmlal_s32(c1_hi, vget_low_s32(f1_hi), vget_low_s32(s_hi));
            c1_hi = vmlal_s32(c1_hi, vget_high_s32(f1_hi), vget_high_s32(s_hi));

            src_ptr += 8;
            dat_ptr += 8;
            flt1_ptr += 8;
            w -= 8;
        } while (w != 0);

        src8 += src_stride;
        dat8 += dat_stride;
        flt1 += flt1_stride;
    } while (--height != 0);

    H[1][1] = (double)vaddvq_s64(vaddq_s64(h11_lo, h11_hi));
    C[1]    = (double)vaddvq_s64(vaddq_s64(c1_lo, c1_hi));
    H[1][1] /= size;
    C[1] /= size;
}

static inline void svt_calc_proj_params_r1_highbd_neon(const uint8_t* src8, int32_t width, int32_t height,
                                                       int32_t src_stride, const uint8_t* dat8, int32_t dat_stride,
                                                       int32_t* flt1, int32_t flt1_stride, double H[2][2],
                                                       double C[2]) {
    const int32_t   size   = width * height;
    const uint16_t* src    = CONVERT_TO_SHORTPTR(src8);
    const uint16_t* dat    = CONVERT_TO_SHORTPTR(dat8);
    int64x2_t       h11_lo = vdupq_n_s64(0);
    int64x2_t       h11_hi = vdupq_n_s64(0);
    int64x2_t       c1_lo  = vdupq_n_s64(0);
    int64x2_t       c1_hi  = vdupq_n_s64(0);

    assert(width % 8 == 0);
    do {
        const uint16_t* src_ptr  = src;
        const uint16_t* dat_ptr  = dat;
        int32_t*        flt1_ptr = flt1;
        int             w        = width;
        do {
            uint16x8_t u     = vld1q_u16(dat_ptr);
            uint16x8_t s     = vld1q_u16(src_ptr);
            int32x4_t  f1_lo = vld1q_s32(flt1_ptr);
            int32x4_t  f1_hi = vld1q_s32(flt1_ptr + 4);

            int32x4_t u_lo = vreinterpretq_s32_u32(vshll_n_u16(vget_low_u16(u), SGRPROJ_RST_BITS));
            int32x4_t u_hi = vreinterpretq_s32_u32(vshll_n_u16(vget_high_u16(u), SGRPROJ_RST_BITS));
            int32x4_t s_lo = vreinterpretq_s32_u32(vshll_n_u16(vget_low_u16(s), SGRPROJ_RST_BITS));
            int32x4_t s_hi = vreinterpretq_s32_u32(vshll_n_u16(vget_high_u16(s), SGRPROJ_RST_BITS));

            s_lo  = vsubq_s32(s_lo, u_lo);
            s_hi  = vsubq_s32(s_hi, u_hi);
            f1_lo = vsubq_s32(f1_lo, u_lo);
            f1_hi = vsubq_s32(f1_hi, u_hi);

            h11_lo = vmlal_s32(h11_lo, vget_low_s32(f1_lo), vget_low_s32(f1_lo));
            h11_lo = vmlal_s32(h11_lo, vget_high_s32(f1_lo), vget_high_s32(f1_lo));
            h11_hi = vmlal_s32(h11_hi, vget_low_s32(f1_hi), vget_low_s32(f1_hi));
            h11_hi = vmlal_s32(h11_hi, vget_high_s32(f1_hi), vget_high_s32(f1_hi));

            c1_lo = vmlal_s32(c1_lo, vget_low_s32(f1_lo), vget_low_s32(s_lo));
            c1_lo = vmlal_s32(c1_lo, vget_high_s32(f1_lo), vget_high_s32(s_lo));
            c1_hi = vmlal_s32(c1_hi, vget_low_s32(f1_hi), vget_low_s32(s_hi));
            c1_hi = vmlal_s32(c1_hi, vget_high_s32(f1_hi), vget_high_s32(s_hi));

            src_ptr += 8;
            dat_ptr += 8;
            flt1_ptr += 8;
            w -= 8;
        } while (w != 0);

        src += src_stride;
        dat += dat_stride;
        flt1 += flt1_stride;
    } while (--height != 0);

    H[1][1] = (double)vaddvq_s64(vaddq_s64(h11_lo, h11_hi));
    C[1]    = (double)vaddvq_s64(vaddq_s64(c1_lo, c1_hi));
    H[1][1] /= size;
    C[1] /= size;
}

// The function calls 3 subfunctions for the following cases :
// 1) When params->r[0] > 0 and params->r[1] > 0. In this case all elements
//    of C and H need to be computed.
// 2) When only params->r[0] > 0. In this case only H[0][0] and C[0] are
//    non-zero and need to be computed.
// 3) When only params->r[1] > 0. In this case only H[1][1] and C[1] are
//    non-zero and need to be computed.
static inline void av1_calc_proj_params_lowbd_neon(const uint8_t* src8, int32_t width, int32_t height,
                                                   int32_t src_stride, const uint8_t* dat8, int32_t dat_stride,
                                                   int32_t* flt0, int32_t flt0_stride, int32_t* flt1,
                                                   int32_t flt1_stride, double H[2][2], double C[2],
                                                   const SgrParamsType* params) {
    if (params->r[0] > 0 && params->r[1] > 0) {
        svt_calc_proj_params_r0_r1_lowbd_neon(
            src8, width, height, src_stride, dat8, dat_stride, flt0, flt0_stride, flt1, flt1_stride, H, C);
    } else if (params->r[0] > 0) {
        svt_calc_proj_params_r0_lowbd_neon(src8, width, height, src_stride, dat8, dat_stride, flt0, flt0_stride, H, C);
    } else if (params->r[1] > 0) {
        svt_calc_proj_params_r1_lowbd_neon(src8, width, height, src_stride, dat8, dat_stride, flt1, flt1_stride, H, C);
    }
}

static inline void av1_calc_proj_params_highbd_neon(const uint8_t* src8, int32_t width, int32_t height,
                                                    int32_t src_stride, const uint8_t* dat8, int32_t dat_stride,
                                                    int32_t* flt0, int32_t flt0_stride, int32_t* flt1,
                                                    int32_t flt1_stride, double H[2][2], double C[2],
                                                    const SgrParamsType* params) {
    if (params->r[0] > 0 && params->r[1] > 0) {
        svt_calc_proj_params_r0_r1_highbd_neon(
            src8, width, height, src_stride, dat8, dat_stride, flt0, flt0_stride, flt1, flt1_stride, H, C);
    } else if (params->r[0] > 0) {
        svt_calc_proj_params_r0_highbd_neon(src8, width, height, src_stride, dat8, dat_stride, flt0, flt0_stride, H, C);
    } else if (params->r[1] > 0) {
        svt_calc_proj_params_r1_highbd_neon(src8, width, height, src_stride, dat8, dat_stride, flt1, flt1_stride, H, C);
    }
}

void svt_get_proj_subspace_neon(const uint8_t* src8, int32_t width, int32_t height, int32_t src_stride,
                                const uint8_t* dat8, int32_t dat_stride, int32_t use_highbitdepth, int32_t* flt0,
                                int32_t flt0_stride, int32_t* flt1, int32_t flt1_stride, int32_t* xq,
                                const SgrParamsType* params) {
    double H[2][2] = {{0, 0}, {0, 0}};
    double C[2]    = {0, 0};
    double det;
    double x[2];

    if (width % 8 != 0) {
        svt_get_proj_subspace_c(src8,
                                width,
                                height,
                                src_stride,
                                dat8,
                                dat_stride,
                                use_highbitdepth,
                                flt0,
                                flt0_stride,
                                flt1,
                                flt1_stride,
                                xq,
                                params);
        return;
    }

    // Default
    xq[0] = 0;
    xq[1] = 0;

    if (!use_highbitdepth) {
        av1_calc_proj_params_lowbd_neon(
            src8, width, height, src_stride, dat8, dat_stride, flt0, flt0_stride, flt1, flt1_stride, H, C, params);
    } else {
        av1_calc_proj_params_highbd_neon(
            src8, width, height, src_stride, dat8, dat_stride, flt0, flt0_stride, flt1, flt1_stride, H, C, params);
    }

    if (params->r[0] > 0 && params->r[1] > 0) {
        det = (H[0][0] * H[1][1] - H[0][1] * H[1][0]);
        if (det < 1e-8) {
            return; // ill-posed, return default values
        }
        x[0] = (H[1][1] * C[0] - H[0][1] * C[1]) / det;
        x[1] = (H[0][0] * C[1] - H[1][0] * C[0]) / det;

        xq[0] = (int32_t)rint(x[0] * (1 << SGRPROJ_PRJ_BITS));
        xq[1] = (int32_t)rint(x[1] * (1 << SGRPROJ_PRJ_BITS));
    } else if (params->r[0] > 0) {
        // H matrix is now only the scalar H[0][0]
        // C vector is now only the scalar C[0]
        det = H[0][0];
        if (det < 1e-8) {
            return; // ill-posed, return default values
        }
        x[0] = C[0] / det;
        x[1] = 0;

        xq[0] = (int32_t)rint(x[0] * (1 << SGRPROJ_PRJ_BITS));
        xq[1] = 0;
    } else if (params->r[1] > 0) {
        // H matrix is now only the scalar H[1][1]
        // C vector is now only the scalar C[1]
        det = H[1][1];
        if (det < 1e-8) {
            return; // ill-posed, return default values
        }
        x[0] = 0;
        x[1] = C[1] / det;

        xq[0] = 0;
        xq[1] = (int32_t)rint(x[1] * (1 << SGRPROJ_PRJ_BITS));
    }
}
