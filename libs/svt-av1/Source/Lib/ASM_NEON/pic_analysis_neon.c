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
#include "sum_neon.h"

void svt_compute_interm_var_four8x8_neon(uint8_t* input_samples, uint16_t input_stride, uint64_t* mean_of8x8_blocks,
                                         uint64_t* mean_of_squared8x8_blocks) {
    // First 8x8 block.
    uint8x8_t s0, s1, s2, s3;
    load_u8_8x4(input_samples, 2 * input_stride, &s0, &s1, &s2, &s3);

    uint16x8_t acc[4];
    acc[0] = vaddq_u16(vaddl_u8(s0, s1), vaddl_u8(s2, s3));

    uint16x8_t sq0 = vmull_u8(s0, s0);
    uint16x8_t sq1 = vmull_u8(s1, s1);
    uint16x8_t sq2 = vmull_u8(s2, s2);
    uint16x8_t sq3 = vmull_u8(s3, s3);

    uint32x4_t sq_acc[4];
    sq_acc[0] = vpaddlq_u16(sq0);
    sq_acc[0] = vpadalq_u16(sq_acc[0], sq1);
    sq_acc[0] = vpadalq_u16(sq_acc[0], sq2);
    sq_acc[0] = vpadalq_u16(sq_acc[0], sq3);

    // Second 8x8 block.
    load_u8_8x4(input_samples + 8, 2 * input_stride, &s0, &s1, &s2, &s3);
    acc[1] = vaddq_u16(vaddl_u8(s0, s1), vaddl_u8(s2, s3));

    sq0 = vmull_u8(s0, s0);
    sq1 = vmull_u8(s1, s1);
    sq2 = vmull_u8(s2, s2);
    sq3 = vmull_u8(s3, s3);

    sq_acc[1] = vpaddlq_u16(sq0);
    sq_acc[1] = vpadalq_u16(sq_acc[1], sq1);
    sq_acc[1] = vpadalq_u16(sq_acc[1], sq2);
    sq_acc[1] = vpadalq_u16(sq_acc[1], sq3);

    // Third 8x8 block.
    load_u8_8x4(input_samples + 16, 2 * input_stride, &s0, &s1, &s2, &s3);
    acc[2] = vaddq_u16(vaddl_u8(s0, s1), vaddl_u8(s2, s3));

    sq0 = vmull_u8(s0, s0);
    sq1 = vmull_u8(s1, s1);
    sq2 = vmull_u8(s2, s2);
    sq3 = vmull_u8(s3, s3);

    sq_acc[2] = vpaddlq_u16(sq0);
    sq_acc[2] = vpadalq_u16(sq_acc[2], sq1);
    sq_acc[2] = vpadalq_u16(sq_acc[2], sq2);
    sq_acc[2] = vpadalq_u16(sq_acc[2], sq3);

    // Fourth 8x8 block.
    load_u8_8x4(input_samples + 24, 2 * input_stride, &s0, &s1, &s2, &s3);
    acc[3] = vaddq_u16(vaddl_u8(s0, s1), vaddl_u8(s2, s3));

    sq0 = vmull_u8(s0, s0);
    sq1 = vmull_u8(s1, s1);
    sq2 = vmull_u8(s2, s2);
    sq3 = vmull_u8(s3, s3);

    sq_acc[3] = vpaddlq_u16(sq0);
    sq_acc[3] = vpadalq_u16(sq_acc[3], sq1);
    sq_acc[3] = vpadalq_u16(sq_acc[3], sq2);
    sq_acc[3] = vpadalq_u16(sq_acc[3], sq3);

    uint32x4_t mean_acc  = horizontal_add_4d_u16x8(acc);
    uint64x2_t mean_low  = vshll_n_u32(vget_low_u32(mean_acc), (VARIANCE_PRECISION >> 1) - 5);
    uint64x2_t mean_high = vshll_n_u32(vget_high_u32(mean_acc), (VARIANCE_PRECISION >> 1) - 5);
    vst1q_u64(mean_of8x8_blocks + 0, mean_low);
    vst1q_u64(mean_of8x8_blocks + 2, mean_high);

    uint32x4_t mean_sq_acc      = horizontal_add_4d_u32x4(sq_acc);
    uint64x2_t mean_sq_acc_low  = vshll_n_u32(vget_low_u32(mean_sq_acc), VARIANCE_PRECISION - 5);
    uint64x2_t mean_sq_acc_high = vshll_n_u32(vget_high_u32(mean_sq_acc), VARIANCE_PRECISION - 5);
    vst1q_u64(mean_of_squared8x8_blocks + 0, mean_sq_acc_low);
    vst1q_u64(mean_of_squared8x8_blocks + 2, mean_sq_acc_high);
}
