/*
 * Copyright (c) 2023, Alliance for Open Media. All rights reserved.
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
#include "neon_sve_bridge.h"
#include "utility.h"
#include "warp_plane_neon.h"
#include "warp_plane_neon_i8mm.h"

static AOM_FORCE_INLINE void vertical_filter_4x1_f4(const int16x8_t* src, int32x4_t* res, int sy, int gamma) {
    int16x8_t s0, s1, s2, s3;
    transpose_elems_s16_4x8(vget_low_s16(src[0]),
                            vget_low_s16(src[1]),
                            vget_low_s16(src[2]),
                            vget_low_s16(src[3]),
                            vget_low_s16(src[4]),
                            vget_low_s16(src[5]),
                            vget_low_s16(src[6]),
                            vget_low_s16(src[7]),
                            &s0,
                            &s1,
                            &s2,
                            &s3);

    int16x8_t f[4];
    load_filters_4(f, sy, gamma);

    int64x2_t m0 = svt_sdotq_s16(vdupq_n_s64(0), s0, f[0]);
    int64x2_t m1 = svt_sdotq_s16(vdupq_n_s64(0), s1, f[1]);
    int64x2_t m2 = svt_sdotq_s16(vdupq_n_s64(0), s2, f[2]);
    int64x2_t m3 = svt_sdotq_s16(vdupq_n_s64(0), s3, f[3]);

    int64x2_t m01 = vpaddq_s64(m0, m1);
    int64x2_t m23 = vpaddq_s64(m2, m3);

    *res = vcombine_s32(vmovn_s64(m01), vmovn_s64(m23));
}

static AOM_FORCE_INLINE void vertical_filter_8x1_f8(const int16x8_t* src, int32x4_t* res_low, int32x4_t* res_high,
                                                    int sy, int gamma) {
    int16x8_t s0 = src[0];
    int16x8_t s1 = src[1];
    int16x8_t s2 = src[2];
    int16x8_t s3 = src[3];
    int16x8_t s4 = src[4];
    int16x8_t s5 = src[5];
    int16x8_t s6 = src[6];
    int16x8_t s7 = src[7];
    transpose_elems_inplace_s16_8x8(&s0, &s1, &s2, &s3, &s4, &s5, &s6, &s7);

    int16x8_t f[8];
    load_filters_8(f, sy, gamma);

    int64x2_t m0 = svt_sdotq_s16(vdupq_n_s64(0), s0, f[0]);
    int64x2_t m1 = svt_sdotq_s16(vdupq_n_s64(0), s1, f[1]);
    int64x2_t m2 = svt_sdotq_s16(vdupq_n_s64(0), s2, f[2]);
    int64x2_t m3 = svt_sdotq_s16(vdupq_n_s64(0), s3, f[3]);
    int64x2_t m4 = svt_sdotq_s16(vdupq_n_s64(0), s4, f[4]);
    int64x2_t m5 = svt_sdotq_s16(vdupq_n_s64(0), s5, f[5]);
    int64x2_t m6 = svt_sdotq_s16(vdupq_n_s64(0), s6, f[6]);
    int64x2_t m7 = svt_sdotq_s16(vdupq_n_s64(0), s7, f[7]);

    int64x2_t m01 = vpaddq_s64(m0, m1);
    int64x2_t m23 = vpaddq_s64(m2, m3);
    int64x2_t m45 = vpaddq_s64(m4, m5);
    int64x2_t m67 = vpaddq_s64(m6, m7);

    *res_low  = vcombine_s32(vmovn_s64(m01), vmovn_s64(m23));
    *res_high = vcombine_s32(vmovn_s64(m45), vmovn_s64(m67));
}

void svt_av1_warp_affine_sve(const int32_t* mat, const uint8_t* ref, int width, int height, int stride, uint8_t* pred,
                             int p_col, int p_row, int p_width, int p_height, int p_stride, int subsampling_x,
                             int subsampling_y, ConvolveParams* conv_params, int16_t alpha, int16_t beta, int16_t gamma,
                             int16_t delta) {
    const int       w0                    = conv_params->fwd_offset;
    const int       w1                    = conv_params->bck_offset;
    const int       is_compound           = conv_params->is_compound;
    uint16_t* const dst                   = conv_params->dst;
    const int       dst_stride            = conv_params->dst_stride;
    const int       do_average            = conv_params->do_average;
    const int       use_dist_wtd_comp_avg = conv_params->use_dist_wtd_comp_avg;

    assert(IMPLIES(is_compound, dst != NULL));
    assert(IMPLIES(do_average, is_compound));

    for (int i = 0; i < p_height; i += 8) {
        for (int j = 0; j < p_width; j += 8) {
            const int32_t src_x = (p_col + j + 4) << subsampling_x;
            const int32_t src_y = (p_row + i + 4) << subsampling_y;
            const int64_t dst_x = (int64_t)mat[2] * src_x + (int64_t)mat[3] * src_y + (int64_t)mat[0];
            const int64_t dst_y = (int64_t)mat[4] * src_x + (int64_t)mat[5] * src_y + (int64_t)mat[1];

            const int64_t x4 = dst_x >> subsampling_x;
            const int64_t y4 = dst_y >> subsampling_y;

            int16x8_t tmp[15];
            warp_affine_horizontal_neon_i8mm(
                ref, width, height, stride, p_width, p_height, alpha, beta, x4, y4, i, tmp);
            warp_affine_vertical(pred,
                                 p_width,
                                 p_height,
                                 p_stride,
                                 is_compound,
                                 dst,
                                 dst_stride,
                                 do_average,
                                 use_dist_wtd_comp_avg,
                                 gamma,
                                 delta,
                                 y4,
                                 i,
                                 j,
                                 tmp,
                                 w0,
                                 w1);
        }
    }
}
