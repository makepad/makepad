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

#include "common_dsp_rtcd.h"
#include "definitions.h"

static inline uint64_t svt_aom_sum_squares_i16_4xn_neon(const int16_t* src, uint32_t n) {
    uint64x2_t sum_u64 = vdupq_n_u64(0);

    do {
        int16x4_t  s0  = vld1_s16(src);
        uint32x4_t sum = vreinterpretq_u32_s32(vmull_s16(s0, s0));

        sum_u64 = vpadalq_u32(sum_u64, sum);

        src += 4;
        n -= 4;
    } while (n >= 4);

    return vaddvq_u64(sum_u64);
}

static inline uint64_t svt_aom_sum_squares_i16_8xn_neon(const int16_t* src, uint32_t n) {
    uint64x2_t sum_u64[2] = {vdupq_n_u64(0), vdupq_n_u64(0)};

    do {
        uint32x4_t sum[2];
        int16x8_t  s0 = vld1q_s16(src);

        sum[0] = vreinterpretq_u32_s32(vmull_s16(vget_low_s16(s0), vget_low_s16(s0)));
        sum[1] = vreinterpretq_u32_s32(vmull_s16(vget_high_s16(s0), vget_high_s16(s0)));

        sum_u64[0] = vpadalq_u32(sum_u64[0], sum[0]);
        sum_u64[1] = vpadalq_u32(sum_u64[1], sum[1]);

        src += 8;
        n -= 8;
    } while (n >= 8);

    return vaddvq_u64(vaddq_u64(sum_u64[0], sum_u64[1]));
}

uint64_t svt_aom_sum_squares_i16_neon(const int16_t* src, uint32_t n) {
    // This function seems to be called only for values of N >= 64. See
    // Source/Lib/Codec/enc_inter_prediction.c.
    if (EB_LIKELY(n >= 8)) {
        return svt_aom_sum_squares_i16_8xn_neon(src, n);
    }
    if (n >= 4) {
        return svt_aom_sum_squares_i16_4xn_neon(src, n);
    }
    return svt_aom_sum_squares_i16_c(src, n);
}
