/*
 * Copyright (c) 2025, Alliance for Open Media. All rights reserved
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
#include "me_context.h"
#include "mem_neon.h"

void svt_compute_interm_var_four8x8_neon_dotprod(uint8_t* input_samples, uint16_t input_stride,
                                                 uint64_t* mean_of8x8_blocks, uint64_t* mean_of_squared8x8_blocks) {
    uint8x16_t s0, s1, s2, s3;
    // First and second 8x8 block.
    load_u8_16x4(input_samples, 2 * input_stride, &s0, &s1, &s2, &s3);

    uint32x4_t acc[2];
    uint32x4_t sq_acc[2];
    uint8x16_t acc_factor = vdupq_n_u8(1 << ((VARIANCE_PRECISION >> 1) - 5));

    acc[0]    = vdotq_u32(vdupq_n_u32(0), s0, acc_factor);
    acc[0]    = vdotq_u32(acc[0], s1, acc_factor);
    acc[0]    = vdotq_u32(acc[0], s2, acc_factor);
    acc[0]    = vdotq_u32(acc[0], s3, acc_factor);
    sq_acc[0] = vdotq_u32(vdupq_n_u32(0), s0, s0);
    sq_acc[0] = vdotq_u32(sq_acc[0], s1, s1);
    sq_acc[0] = vdotq_u32(sq_acc[0], s2, s2);
    sq_acc[0] = vdotq_u32(sq_acc[0], s3, s3);

    // Third and fourth 8x8 block.
    load_u8_16x4(input_samples + 16, 2 * input_stride, &s0, &s1, &s2, &s3);

    acc[1]    = vdotq_u32(vdupq_n_u32(0), s0, acc_factor);
    acc[1]    = vdotq_u32(acc[1], s1, acc_factor);
    acc[1]    = vdotq_u32(acc[1], s2, acc_factor);
    acc[1]    = vdotq_u32(acc[1], s3, acc_factor);
    sq_acc[1] = vdotq_u32(vdupq_n_u32(0), s0, s0);
    sq_acc[1] = vdotq_u32(sq_acc[1], s1, s1);
    sq_acc[1] = vdotq_u32(sq_acc[1], s2, s2);
    sq_acc[1] = vdotq_u32(sq_acc[1], s3, s3);

    uint32x4_t mean_acc = vpaddq_u32(acc[0], acc[1]);
    vst1q_u64(mean_of8x8_blocks + 0, vmovl_u32(vget_low_u32(mean_acc)));
    vst1q_u64(mean_of8x8_blocks + 2, vmovl_u32(vget_high_u32(mean_acc)));

    uint32x4_t mean_sq_acc      = vpaddq_u32(sq_acc[0], sq_acc[1]);
    uint64x2_t mean_sq_acc_low  = vshll_n_u32(vget_low_u32(mean_sq_acc), VARIANCE_PRECISION - 5);
    uint64x2_t mean_sq_acc_high = vshll_n_u32(vget_high_u32(mean_sq_acc), VARIANCE_PRECISION - 5);
    vst1q_u64(mean_of_squared8x8_blocks + 0, mean_sq_acc_low);
    vst1q_u64(mean_of_squared8x8_blocks + 2, mean_sq_acc_high);
}
