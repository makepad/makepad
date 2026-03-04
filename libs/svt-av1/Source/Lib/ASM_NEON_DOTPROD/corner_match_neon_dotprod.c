/*
 * Copyright (c) 2025, Alliance for Open Media. All rights reserved.
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

static const uint8_t mask_table[] = {255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
                                     255, 0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0};

double svt_av1_compute_cross_correlation_neon_dotprod(unsigned char* im1, int stride1, int x1, int y1,
                                                      unsigned char* im2, int stride2, int x2, int y2,
                                                      uint8_t match_sz) {
    // match_sz must be an odd number between 3 and 15.
    assert(match_sz % 2 == 1);
    assert(match_sz <= 15);
    assert(match_sz >= 3);

    if (match_sz == 3) {
        return svt_av1_compute_cross_correlation_neon(im1, stride1, x1, y1, im2, stride2, x2, y2, match_sz);
    }

    const uint8_t match_sz_by2 = (match_sz - 1) / 2;
    const uint8_t match_sz_sq  = match_sz * match_sz;

    const uint8x16_t mask = vld1q_u8(mask_table + 15 - match_sz);

    uint8_t* im1_start = im1 + (y1 - match_sz_by2) * stride1 + x1 - match_sz_by2;
    uint8_t* im2_start = im2 + (y2 - match_sz_by2) * stride2 + x2 - match_sz_by2;

    uint32_t sum1, sum2, cross, sumsq2;

    if (match_sz < 8) {
        uint32x4_t sum1_2_u32      = vdupq_n_u32(0);
        uint32x4_t cross_sumsq_u32 = vdupq_n_u32(0);

        int i = match_sz;
        do {
            uint8x8_t im1_u8 = vand_u8(vld1_u8(im1_start), vget_low_u8(mask));
            uint8x8_t im2_u8 = vand_u8(vld1_u8(im2_start), vget_low_u8(mask));

            uint8x16_t im2_im2 = vcombine_u8(im2_u8, im2_u8);
            uint8x16_t im1_im2 = vcombine_u8(im1_u8, im2_u8);

            cross_sumsq_u32 = vdotq_u32(cross_sumsq_u32, im2_im2, im1_im2);
            sum1_2_u32      = vdotq_u32(sum1_2_u32, im1_im2, vdupq_n_u8(1));

            im1_start += stride1;
            im2_start += stride2;
        } while (--i != 0);

        uint32x4_t all = vpaddq_u32(sum1_2_u32, cross_sumsq_u32);
        sum1           = vgetq_lane_u32(all, 0);
        sum2           = vgetq_lane_u32(all, 1);
        cross          = vgetq_lane_u32(all, 2);
        sumsq2         = vgetq_lane_u32(all, 3);

    } else {
        uint32x4_t sum1_u32   = vdupq_n_u32(0);
        uint32x4_t sum2_u32   = vdupq_n_u32(0);
        uint32x4_t sumsq2_u32 = vdupq_n_u32(0);
        uint32x4_t cross_u32  = vdupq_n_u32(0);

        int i = match_sz;
        do {
            uint8x16_t im1_u8 = vandq_u8(vld1q_u8(im1_start), mask);
            uint8x16_t im2_u8 = vandq_u8(vld1q_u8(im2_start), mask);

            sumsq2_u32 = vdotq_u32(sumsq2_u32, im2_u8, im2_u8);
            cross_u32  = vdotq_u32(cross_u32, im1_u8, im2_u8);

            sum1_u32 = vdotq_u32(sum1_u32, im1_u8, vdupq_n_u8(1));
            sum2_u32 = vdotq_u32(sum2_u32, im2_u8, vdupq_n_u8(1));

            im1_start += stride1;
            im2_start += stride2;
        } while (--i != 0);

        sum1   = vaddvq_u32(sum1_u32);
        sum2   = vaddvq_u32(sum2_u32);
        sumsq2 = vaddvq_u32(sumsq2_u32);
        cross  = vaddvq_u32(cross_u32);
    }

    int var2 = sumsq2 * match_sz_sq - sum2 * sum2;
    int cov  = cross * match_sz_sq - sum1 * sum2;
    return cov < 0 ? 0 : ((double)cov * cov) / ((double)var2);
}
