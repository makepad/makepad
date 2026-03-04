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

double svt_av1_compute_cross_correlation_neon(unsigned char* im1, int stride1, int x1, int y1, unsigned char* im2,
                                              int stride2, int x2, int y2, uint8_t match_sz) {
    // match_sz must be an odd number between 3 and 15.
    assert(match_sz % 2 == 1);
    assert(match_sz <= 15);
    assert(match_sz >= 3);

    const uint8_t match_sz_by2 = (match_sz - 1) / 2;
    const uint8_t match_sz_sq  = match_sz * match_sz;

    const uint8x16_t mask = vld1q_u8(mask_table + 15 - match_sz);

    uint8_t* im1_start = im1 + (y1 - match_sz_by2) * stride1 + x1 - match_sz_by2;
    uint8_t* im2_start = im2 + (y2 - match_sz_by2) * stride2 + x2 - match_sz_by2;

    uint16x8_t sum1_u16   = vdupq_n_u16(0);
    uint16x8_t sum2_u16   = vdupq_n_u16(0);
    uint32x4_t sumsq2_u32 = vdupq_n_u32(0);
    uint32x4_t cross_u32  = vdupq_n_u32(0);

    if (match_sz < 8) {
        int i = match_sz;
        do {
            uint8x8_t im1_u8 = vand_u8(vld1_u8(im1_start), vget_low_u8(mask));
            uint8x8_t im2_u8 = vand_u8(vld1_u8(im2_start), vget_low_u8(mask));

            sumsq2_u32 = vpadalq_u16(sumsq2_u32, vmull_u8(im2_u8, im2_u8));
            cross_u32  = vpadalq_u16(cross_u32, vmull_u8(im1_u8, im2_u8));

            sum1_u16 = vaddw_u8(sum1_u16, im1_u8);
            sum2_u16 = vaddw_u8(sum2_u16, im2_u8);

            im1_start += stride1;
            im2_start += stride2;
        } while (--i != 0);
    } else {
        int i = match_sz;
        do {
            uint8x16_t im1_u8 = vandq_u8(vld1q_u8(im1_start), mask);
            uint8x16_t im2_u8 = vandq_u8(vld1q_u8(im2_start), mask);

            sumsq2_u32 = vpadalq_u16(sumsq2_u32, vmull_u8(vget_low_u8(im2_u8), vget_low_u8(im2_u8)));
            sumsq2_u32 = vpadalq_u16(sumsq2_u32, vmull_u8(vget_high_u8(im2_u8), vget_high_u8(im2_u8)));

            cross_u32 = vpadalq_u16(cross_u32, vmull_u8(vget_low_u8(im1_u8), vget_low_u8(im2_u8)));
            cross_u32 = vpadalq_u16(cross_u32, vmull_u8(vget_high_u8(im1_u8), vget_high_u8(im2_u8)));

            sum1_u16 = vpadalq_u8(sum1_u16, im1_u8);
            sum2_u16 = vpadalq_u8(sum2_u16, im2_u8);

            im1_start += stride1;
            im2_start += stride2;
        } while (--i != 0);
    }

    uint16_t sum1   = vaddvq_u16(sum1_u16);
    uint16_t sum2   = vaddvq_u16(sum2_u16);
    uint32_t sumsq2 = vaddvq_u32(sumsq2_u32);
    uint32_t cross  = vaddvq_u32(cross_u32);

    int var2 = sumsq2 * match_sz_sq - sum2 * sum2;
    int cov  = cross * match_sz_sq - sum1 * sum2;
    return cov < 0 ? 0 : ((double)cov * cov) / ((double)var2);
}
