/*
 * Copyright (c) 2024, Alliance for Open Media. All rights reserved.
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at www.aomedia.org/license/software. If the Alliance for Open
 * Media Patent License 1.0 was not distributed with this source code in the
 * PATENTS file, you can obtain it at www.aomedia.org/license/patent.
 */

#include <arm_neon.h>

#include "common_dsp_rtcd.h"
#include "definitions.h"
#include "highbd_convolve_sve.h"
#include "highbd_jnt_convolve_neon.h"
#include "highbd_jnt_convolve_sve.h"
#include "mem_neon.h"
#include "neon_sve_bridge.h"
#include "utility.h"

static inline void highbd_dist_wtd_convolve_x_8tap_sve(const uint16_t* src, int src_stride, uint16_t* dst,
                                                       int dst_stride, int width, int height,
                                                       const int16_t* x_filter_ptr, const int bd) {
    const int64x1_t offset_vec = vcreate_s64((1 << (bd + FILTER_BITS)) + (1 << (bd + FILTER_BITS - 1)));
    const int64x2_t offset     = vcombine_s64(offset_vec, vdup_n_s64(0));

    const int16x8_t filter = vld1q_s16(x_filter_ptr);

    do {
        const int16_t* s = (const int16_t*)src;
        uint16_t*      d = dst;
        int            w = width;

        do {
            int16x8_t s0[8], s1[8], s2[8], s3[8];
            load_s16_8x8(s + 0 * src_stride, 1, &s0[0], &s0[1], &s0[2], &s0[3], &s0[4], &s0[5], &s0[6], &s0[7]);
            load_s16_8x8(s + 1 * src_stride, 1, &s1[0], &s1[1], &s1[2], &s1[3], &s1[4], &s1[5], &s1[6], &s1[7]);
            load_s16_8x8(s + 2 * src_stride, 1, &s2[0], &s2[1], &s2[2], &s2[3], &s2[4], &s2[5], &s2[6], &s2[7]);
            load_s16_8x8(s + 3 * src_stride, 1, &s3[0], &s3[1], &s3[2], &s3[3], &s3[4], &s3[5], &s3[6], &s3[7]);

            uint16x8_t d0 = highbd_convolve8_8_x(s0, filter, offset);
            uint16x8_t d1 = highbd_convolve8_8_x(s1, filter, offset);
            uint16x8_t d2 = highbd_convolve8_8_x(s2, filter, offset);
            uint16x8_t d3 = highbd_convolve8_8_x(s3, filter, offset);

            store_u16_8x4(d, dst_stride, d0, d1, d2, d3);

            s += 8;
            d += 8;
            w -= 8;
        } while (w != 0);
        src += 4 * src_stride;
        dst += 4 * dst_stride;
        height -= 4;
    } while (height != 0);
}

static inline void highbd_dist_wtd_convolve_x_4tap_sve(const uint16_t* src, int src_stride, uint16_t* dst,
                                                       int dst_stride, int width, int height,
                                                       const int16_t* x_filter_ptr, const int bd) {
    const int64x2_t offset = vdupq_n_s64((1 << (bd + FILTER_BITS)) + (1 << (bd + FILTER_BITS - 1)));

    const int16x4_t x_filter = vld1_s16(x_filter_ptr + 2);
    const int16x8_t filter   = vcombine_s16(x_filter, vdup_n_s16(0));

    if (width == 4) {
        uint16x8x2_t permute_tbl = vld1q_u16_x2(svt_kHbdDotProdTbl);

        const int16_t* s = (const int16_t*)(src);

        do {
            int16x8_t s0, s1, s2, s3;
            load_s16_8x4(s, src_stride, &s0, &s1, &s2, &s3);

            uint16x4_t d0 = highbd_convolve4_4_x(s0, filter, offset, permute_tbl);
            uint16x4_t d1 = highbd_convolve4_4_x(s1, filter, offset, permute_tbl);
            uint16x4_t d2 = highbd_convolve4_4_x(s2, filter, offset, permute_tbl);
            uint16x4_t d3 = highbd_convolve4_4_x(s3, filter, offset, permute_tbl);

            store_u16_4x4(dst, dst_stride, d0, d1, d2, d3);

            s += 4 * src_stride;
            dst += 4 * dst_stride;
            height -= 4;
        } while (height != 0);
    } else {
        uint16x8_t idx = vld1q_u16(svt_kDeinterleaveTbl);

        do {
            const int16_t* s = (const int16_t*)(src);
            uint16_t*      d = dst;
            int            w = width;

            do {
                int16x8_t s0[4], s1[4], s2[4], s3[4];
                load_s16_8x4(s + 0 * src_stride, 1, &s0[0], &s0[1], &s0[2], &s0[3]);
                load_s16_8x4(s + 1 * src_stride, 1, &s1[0], &s1[1], &s1[2], &s1[3]);
                load_s16_8x4(s + 2 * src_stride, 1, &s2[0], &s2[1], &s2[2], &s2[3]);
                load_s16_8x4(s + 3 * src_stride, 1, &s3[0], &s3[1], &s3[2], &s3[3]);

                uint16x8_t d0 = highbd_convolve4_8_x(s0, filter, offset, idx);
                uint16x8_t d1 = highbd_convolve4_8_x(s1, filter, offset, idx);
                uint16x8_t d2 = highbd_convolve4_8_x(s2, filter, offset, idx);
                uint16x8_t d3 = highbd_convolve4_8_x(s3, filter, offset, idx);

                store_u16_8x4(d, dst_stride, d0, d1, d2, d3);

                s += 8;
                d += 8;
                w -= 8;
            } while (w != 0);
            src += 4 * src_stride;
            dst += 4 * dst_stride;
            height -= 4;
        } while (height != 0);
    }
}

void svt_av1_highbd_jnt_convolve_x_sve(const uint16_t* src, int src_stride, uint16_t* dst, int dst_stride, int w, int h,
                                       const InterpFilterParams* filter_params_x,
                                       const InterpFilterParams* filter_params_y, const int subpel_x_qn,
                                       const int subpel_y_qn, ConvolveParams* conv_params, int bd) {
    if (h == 2 || w == 2) {
        svt_av1_highbd_jnt_convolve_x_c(src,
                                        src_stride,
                                        dst,
                                        dst_stride,
                                        w,
                                        h,
                                        filter_params_x,
                                        filter_params_y,
                                        subpel_x_qn,
                                        subpel_y_qn,
                                        conv_params,
                                        bd);
        return;
    }

    DECLARE_ALIGNED(16, uint16_t, im_block[(MAX_SB_SIZE + MAX_FILTER_TAP) * MAX_SB_SIZE]);
    CONV_BUF_TYPE* dst16         = conv_params->dst;
    const int      x_filter_taps = get_filter_tap(filter_params_x, subpel_x_qn);

    if (x_filter_taps == 6) {
        svt_av1_highbd_jnt_convolve_x_neon(src,
                                           src_stride,
                                           dst,
                                           dst_stride,
                                           w,
                                           h,
                                           filter_params_x,
                                           filter_params_y,
                                           subpel_x_qn,
                                           subpel_y_qn,
                                           conv_params,
                                           bd);
        return;
    }

    int       dst16_stride = conv_params->dst_stride;
    const int im_stride    = MAX_SB_SIZE;
    const int horiz_offset = filter_params_x->taps / 2 - 1;
    assert(FILTER_BITS == COMPOUND_ROUND1_BITS);

    const int16_t* x_filter_ptr = av1_get_interp_filter_subpel_kernel(*filter_params_x, subpel_x_qn & SUBPEL_MASK);

    src -= horiz_offset;

    if (conv_params->do_average) {
        if (x_filter_taps <= 4) {
            highbd_dist_wtd_convolve_x_4tap_sve(src + 2, src_stride, im_block, im_stride, w, h, x_filter_ptr, bd);
        } else {
            highbd_dist_wtd_convolve_x_8tap_sve(src, src_stride, im_block, im_stride, w, h, x_filter_ptr, bd);
        }

        if (conv_params->use_jnt_comp_avg) {
            highbd_jnt_comp_avg_neon(im_block, im_stride, dst, dst_stride, w, h, conv_params, bd);
        } else {
            highbd_comp_avg_neon(im_block, im_stride, dst, dst_stride, w, h, conv_params, bd);
        }
    } else {
        if (x_filter_taps <= 4) {
            highbd_dist_wtd_convolve_x_4tap_sve(src + 2, src_stride, dst16, dst16_stride, w, h, x_filter_ptr, bd);
        } else {
            highbd_dist_wtd_convolve_x_8tap_sve(src, src_stride, dst16, dst16_stride, w, h, x_filter_ptr, bd);
        }
    }
}
