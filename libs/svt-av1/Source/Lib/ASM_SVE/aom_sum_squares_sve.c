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
#include <arm_sve.h>

#include "aom_dsp_rtcd.h"
#include "neon_sve_bridge.h"

uint64_t svt_aom_sum_squares_i16_sve(const int16_t* src, uint32_t n) {
    // This function seems to be called only for values of N >= 64.
    // See Source/Lib/Codec/enc_inter_prediction.c. Additionally,
    // because N = width x height for width and height being the
    // standard block sizes, N will also be a multiple of 64.
    if (EB_UNLIKELY(n % 64 != 0)) {
        return svt_aom_sum_squares_i16_c(src, n);
    }

    int64x2_t sum[4] = {vdupq_n_s64(0), vdupq_n_s64(0), vdupq_n_s64(0), vdupq_n_s64(0)};
    do {
        int16x8_t s0 = vld1q_s16(src);
        int16x8_t s1 = vld1q_s16(src + 8);
        int16x8_t s2 = vld1q_s16(src + 16);
        int16x8_t s3 = vld1q_s16(src + 24);

        sum[0] = svt_sdotq_s16(sum[0], s0, s0);
        sum[1] = svt_sdotq_s16(sum[1], s1, s1);
        sum[2] = svt_sdotq_s16(sum[2], s2, s2);
        sum[3] = svt_sdotq_s16(sum[3], s3, s3);

        src += 32;
        n -= 32;
    } while (n != 0);

    sum[0] = vaddq_s64(sum[0], sum[1]);
    sum[2] = vaddq_s64(sum[2], sum[3]);
    sum[0] = vaddq_s64(sum[0], sum[2]);
    return vaddvq_s64(sum[0]);
}
