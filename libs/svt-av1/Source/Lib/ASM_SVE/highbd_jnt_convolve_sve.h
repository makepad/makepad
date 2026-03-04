/*
 * Copyright (c) 2024, Alliance for Open Media. All rights reserved
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at www.aomedia.org/license/software. If the Alliance for Open
 * Media Patent License 1.0 was not distributed with this source code in the
 * PATENTS file, you can obtain it at www.aomedia.org/license/patent.
 */

#ifndef HIGHBD_JNT_CONVOLVE_SVE_H
#define HIGHBD_JNT_CONVOLVE_SVE_H

#include <arm_neon.h>

#include "neon_sve_bridge.h"

#include "definitions.h"

static inline uint16x8_t highbd_convolve8_8_x(int16x8_t s0[8], int16x8_t filter, int64x2_t offset) {
    int64x2_t sum[8];
    sum[0] = svt_sdotq_s16(offset, s0[0], filter);
    sum[1] = svt_sdotq_s16(offset, s0[1], filter);
    sum[2] = svt_sdotq_s16(offset, s0[2], filter);
    sum[3] = svt_sdotq_s16(offset, s0[3], filter);
    sum[4] = svt_sdotq_s16(offset, s0[4], filter);
    sum[5] = svt_sdotq_s16(offset, s0[5], filter);
    sum[6] = svt_sdotq_s16(offset, s0[6], filter);
    sum[7] = svt_sdotq_s16(offset, s0[7], filter);

    sum[0] = vpaddq_s64(sum[0], sum[1]);
    sum[2] = vpaddq_s64(sum[2], sum[3]);
    sum[4] = vpaddq_s64(sum[4], sum[5]);
    sum[6] = vpaddq_s64(sum[6], sum[7]);

    int32x4_t sum0123 = vcombine_s32(vmovn_s64(sum[0]), vmovn_s64(sum[2]));
    int32x4_t sum4567 = vcombine_s32(vmovn_s64(sum[4]), vmovn_s64(sum[6]));

    return vcombine_u16(vqrshrun_n_s32(sum0123, ROUND0_BITS), vqrshrun_n_s32(sum4567, ROUND0_BITS));
}

static inline uint16x4_t highbd_convolve4_4_x(int16x8_t s0, int16x8_t filter, int64x2_t offset,
                                              uint16x8x2_t permute_tbl) {
    int16x8_t permuted_samples0 = svt_tbl_s16(s0, permute_tbl.val[0]);
    int16x8_t permuted_samples1 = svt_tbl_s16(s0, permute_tbl.val[1]);

    int64x2_t sum01 = svt_svdot_lane_s16(offset, permuted_samples0, filter, 0);
    int64x2_t sum23 = svt_svdot_lane_s16(offset, permuted_samples1, filter, 0);

    int32x4_t sum0123 = vcombine_s32(vmovn_s64(sum01), vmovn_s64(sum23));

    return vqrshrun_n_s32(sum0123, ROUND0_BITS);
}

static inline uint16x8_t highbd_convolve4_8_x(int16x8_t s0[4], int16x8_t filter, int64x2_t offset, uint16x8_t tbl) {
    int64x2_t sum04 = svt_svdot_lane_s16(offset, s0[0], filter, 0);
    int64x2_t sum15 = svt_svdot_lane_s16(offset, s0[1], filter, 0);
    int64x2_t sum26 = svt_svdot_lane_s16(offset, s0[2], filter, 0);
    int64x2_t sum37 = svt_svdot_lane_s16(offset, s0[3], filter, 0);

    int32x4_t sum0415 = vcombine_s32(vmovn_s64(sum04), vmovn_s64(sum15));
    int32x4_t sum2637 = vcombine_s32(vmovn_s64(sum26), vmovn_s64(sum37));

    uint16x8_t res = vcombine_u16(vqrshrun_n_s32(sum0415, ROUND0_BITS), vqrshrun_n_s32(sum2637, ROUND0_BITS));
    return svt_tbl_u16(res, tbl);
}

#endif // HIGHBD_JNT_CONVOLVE_SVE_H
